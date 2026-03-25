#!/usr/bin/env python3
"""
Lichen Load Test — 5,000 Concurrent Traders

Simulates 5,000 traders submitting transactions concurrently to stress-test:
- Mempool capacity (50K tx limit)
- RPC channel throughput (50K)
- Transaction processing under load
- Validator consensus under pressure

Usage:
    python tests/load-test-5k-traders.py [--traders N] [--txs-per-trader N]
"""

import asyncio
import json
import os
import random
import struct
import sys
import time
from pathlib import Path
from typing import Any, Dict, List

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "sdk" / "python"))

from lichen import Connection, Instruction, Keypair, PublicKey, TransactionBuilder


def load_keypair_flexible(path: Path) -> Keypair:
    """Load keypair handling all genesis key formats."""
    try:
        return Keypair.load(path)
    except Exception:
        pass
    raw = json.loads(path.read_text(encoding="utf-8"))
    if isinstance(raw, dict):
        sk = raw.get("secret_key") or raw.get("privateKey") or raw.get("seed")
        if isinstance(sk, str):
            h = sk.strip().lower().removeprefix("0x")
            if len(h) == 64:
                return Keypair.from_seed(bytes.fromhex(h))
        if isinstance(sk, list) and len(sk) == 32:
            return Keypair.from_seed(bytes(sk))
    raise ValueError(f"unsupported keypair format: {path}")

RPC_ENDPOINTS = [
    "http://127.0.0.1:8899",
    "http://127.0.0.1:8901",
    "http://127.0.0.1:8903",
]

CONTRACT_PROGRAM = PublicKey(b"\xff" * 32)
DEPLOYER_PATH = os.getenv("AGENT_KEYPAIR") or str(ROOT / "keypairs" / "deployer.json")
REQUIRE_LOAD_TEST_BUDGET = os.getenv("REQUIRE_LOAD_TEST_BUDGET", "0") == "1"

# Parse CLI args
import argparse
parser = argparse.ArgumentParser(description="Lichen 5K Concurrent Traders Load Test")
parser.add_argument("--traders", type=int, default=5000, help="Number of concurrent traders")
parser.add_argument("--txs-per-trader", type=int, default=3, help="Transactions per trader")
parser.add_argument("--batch-size", type=int, default=500, help="Traders per concurrent batch")
parser.add_argument("--fund-amount", type=int, default=10, help="LICN per trader (funding)")
args = parser.parse_args()

NUM_TRADERS = args.traders
TXS_PER_TRADER = args.txs_per_trader
BATCH_SIZE = args.batch_size
FUND_AMOUNT_SPORES = args.fund_amount * 1_000_000_000  # Convert to spores

# Counters
sent_ok = 0
sent_fail = 0
latencies: List[float] = []


def get_conn(index: int = 0) -> Connection:
    """Round-robin across validator RPCs."""
    url = RPC_ENDPOINTS[index % len(RPC_ENDPOINTS)]
    return Connection(url)


async def fund_trader(deployer: Keypair, trader: Keypair, conn: Connection, amount: int) -> bool:
    """Fund a trader account from deployer."""
    try:
        blockhash = await conn.get_recent_blockhash()
        ix = TransactionBuilder.transfer(
            deployer.public_key(), trader.public_key(), amount
        )
        tx = (
            TransactionBuilder()
            .add(ix)
            .set_recent_blockhash(blockhash)
            .build_and_sign(deployer)
        )
        await conn.send_transaction(tx)
        return True
    except Exception:
        return False


async def trader_session(trader_id: int, trader_kp: Keypair, deployer_pubkey: PublicKey) -> Dict[str, Any]:
    """Simulate a single trader's session: N transactions."""
    global sent_ok, sent_fail

    conn = get_conn(trader_id)
    results = {"id": trader_id, "sent": 0, "failed": 0, "latencies": []}

    for tx_num in range(TXS_PER_TRADER):
        try:
            t0 = time.monotonic()
            blockhash = await conn.get_recent_blockhash()

            # Simulate different trade types
            action = tx_num % 3
            if action == 0:
                # Transfer back to deployer (small amount)
                ix = TransactionBuilder.transfer(
                    trader_kp.public_key(), deployer_pubkey, 1_000_000  # 0.001 LICN
                )
            elif action == 1:
                # Contract call (DEX-like swap instruction)
                data = struct.pack("<B", 7)  # opcode 7 = swap
                data += struct.pack("<Q", random.randint(100, 10000))  # amount
                data += struct.pack("<Q", random.randint(1, 100))  # min_out
                ix = Instruction(CONTRACT_PROGRAM, [trader_kp.public_key()], data)
            else:
                # Another transfer (peer-to-peer)
                peer_pubkey = PublicKey(random.randbytes(32))
                ix = TransactionBuilder.transfer(
                    trader_kp.public_key(), peer_pubkey, 100_000  # 0.0001 LICN
                )

            tx = (
                TransactionBuilder()
                .add(ix)
                .set_recent_blockhash(blockhash)
                .build_and_sign(trader_kp)
            )
            await conn.send_transaction(tx)
            elapsed = time.monotonic() - t0

            results["sent"] += 1
            results["latencies"].append(elapsed)
            sent_ok += 1
        except Exception as e:
            results["failed"] += 1
            sent_fail += 1

    return results


async def main():
    global sent_ok, sent_fail, latencies, NUM_TRADERS

    print("=" * 70)
    print(f"  Lichen Load Test — {NUM_TRADERS} Concurrent Traders")
    print(f"  TXs per trader: {TXS_PER_TRADER}  |  Total target: {NUM_TRADERS * TXS_PER_TRADER}")
    print(f"  Batch size: {BATCH_SIZE}  |  RPC endpoints: {len(RPC_ENDPOINTS)}")
    print("=" * 70)
    print()

    # Load deployer
    deployer = load_keypair_flexible(Path(DEPLOYER_PATH))
    conn = get_conn(0)

    # Check deployer balance
    bal = await conn.get_balance(str(deployer.public_key()))
    deployer_spores = bal.get("spores", 0) if isinstance(bal, dict) else int(bal)
    deployer_licn = deployer_spores / 1_000_000_000
    needed_licn = (NUM_TRADERS * FUND_AMOUNT_SPORES) / 1_000_000_000
    print(f"  Deployer: {deployer.public_key()}")
    print(f"  Balance: {deployer_licn:.2f} LICN  |  Need: {needed_licn:.2f} LICN")

    if deployer_spores < NUM_TRADERS * FUND_AMOUNT_SPORES:
        print(f"  ⚠ Insufficient balance. Need {needed_licn:.0f} LICN, have {deployer_licn:.0f}")
        print(f"  Reducing traders to fit budget...")
        max_traders = int(deployer_spores / FUND_AMOUNT_SPORES) - 10  # Reserve some for fees
        if max_traders < 10:
            if REQUIRE_LOAD_TEST_BUDGET:
                print("  ✗ Not enough LICN to run load test")
                sys.exit(1)
            print("  ⚠ Not enough LICN to run load test in current environment; skipping in relaxed mode")
            return
        NUM_TRADERS = max_traders
        print(f"  Adjusted to {NUM_TRADERS} traders")

    # Generate trader keypairs
    print(f"\n  Generating {NUM_TRADERS} trader keypairs...")
    t0 = time.monotonic()
    traders = [Keypair.generate() for _ in range(NUM_TRADERS)]
    print(f"  Generated in {time.monotonic() - t0:.2f}s")

    # Fund traders in batches
    print(f"\n  Funding {NUM_TRADERS} traders ({FUND_AMOUNT_SPORES / 1e9:.1f} LICN each)...")
    funded = 0
    fund_t0 = time.monotonic()

    for batch_start in range(0, NUM_TRADERS, BATCH_SIZE):
        batch_end = min(batch_start + BATCH_SIZE, NUM_TRADERS)
        batch = traders[batch_start:batch_end]

        # Fund sequentially (deployer nonce must be sequential)
        for trader_kp in batch:
            ok = await fund_trader(deployer, trader_kp, conn, FUND_AMOUNT_SPORES)
            if ok:
                funded += 1

        pct = (batch_end / NUM_TRADERS) * 100
        print(f"    [{pct:5.1f}%] Funded {funded}/{batch_end} traders")
        # Small delay between batches to avoid mempool overflow
        await asyncio.sleep(0.5)

    fund_elapsed = time.monotonic() - fund_t0
    print(f"  Funded {funded}/{NUM_TRADERS} in {fund_elapsed:.1f}s "
          f"({funded / fund_elapsed:.0f} fund-TXs/s)")

    # Wait for funding TXs to be confirmed
    print(f"\n  Waiting 5s for funding confirmations...")
    await asyncio.sleep(5)

    # Execute trading sessions
    print(f"\n  🚀 Launching {NUM_TRADERS} concurrent trader sessions...")
    print(f"     Target: {NUM_TRADERS * TXS_PER_TRADER} total transactions")
    trade_t0 = time.monotonic()
    all_results: List[Dict[str, Any]] = []

    # Run in batches to avoid overwhelming asyncio
    for batch_start in range(0, NUM_TRADERS, BATCH_SIZE):
        batch_end = min(batch_start + BATCH_SIZE, NUM_TRADERS)
        batch = traders[batch_start:batch_end]

        tasks = [
            trader_session(batch_start + i, kp, deployer.public_key())
            for i, kp in enumerate(batch)
        ]
        batch_results = await asyncio.gather(*tasks, return_exceptions=True)

        for r in batch_results:
            if isinstance(r, dict):
                all_results.append(r)
                latencies.extend(r.get("latencies", []))
            else:
                sent_fail += 1

        elapsed = time.monotonic() - trade_t0
        pct = (batch_end / NUM_TRADERS) * 100
        total_sent = sent_ok + sent_fail
        tps = total_sent / elapsed if elapsed > 0 else 0
        print(f"    [{pct:5.1f}%] {total_sent:>6} TXs  |  "
              f"OK: {sent_ok}  FAIL: {sent_fail}  |  {tps:.0f} TX/s")

    trade_elapsed = time.monotonic() - trade_t0
    total_txs = sent_ok + sent_fail
    avg_tps = total_txs / trade_elapsed if trade_elapsed > 0 else 0

    # Stats
    print()
    print("=" * 70)
    print("  LOAD TEST RESULTS")
    print("=" * 70)
    print(f"  Traders:          {NUM_TRADERS}")
    print(f"  TXs per trader:   {TXS_PER_TRADER}")
    print(f"  Total TXs sent:   {total_txs}")
    print(f"  Successful:       {sent_ok} ({sent_ok / total_txs * 100:.1f}%)" if total_txs > 0 else "")
    print(f"  Failed:           {sent_fail} ({sent_fail / total_txs * 100:.1f}%)" if total_txs > 0 else "")
    print(f"  Duration:         {trade_elapsed:.2f}s")
    print(f"  Throughput:       {avg_tps:.1f} TX/s")

    if latencies:
        import statistics
        latencies.sort()
        print(f"\n  LATENCY STATS ({len(latencies)} TXs)")
        print(f"    Min:     {min(latencies)*1000:.0f}ms")
        print(f"    Avg:     {statistics.mean(latencies)*1000:.0f}ms")
        print(f"    Median:  {statistics.median(latencies)*1000:.0f}ms")
        print(f"    P95:     {latencies[int(len(latencies)*0.95)]*1000:.0f}ms")
        print(f"    P99:     {latencies[int(len(latencies)*0.99)]*1000:.0f}ms")
        print(f"    Max:     {max(latencies)*1000:.0f}ms")

    print()
    print("=" * 70)

    # Pass/fail threshold
    success_rate = sent_ok / total_txs * 100 if total_txs > 0 else 0
    if success_rate >= 80 and avg_tps >= 50:
        print(f"  ✅ PASS — {success_rate:.1f}% success rate, {avg_tps:.0f} TX/s")
    else:
        print(f"  ⚠ BELOW TARGET — {success_rate:.1f}% success rate, {avg_tps:.0f} TX/s")
        print(f"     Target: ≥80% success rate, ≥50 TX/s")

    # Save report
    report = {
        "traders": NUM_TRADERS,
        "txs_per_trader": TXS_PER_TRADER,
        "total_txs": total_txs,
        "successful": sent_ok,
        "failed": sent_fail,
        "success_rate_pct": round(success_rate, 1),
        "duration_s": round(trade_elapsed, 2),
        "throughput_tps": round(avg_tps, 1),
        "latency_ms": {
            "min": round(min(latencies) * 1000, 1) if latencies else 0,
            "avg": round(statistics.mean(latencies) * 1000, 1) if latencies else 0,
            "median": round(statistics.median(latencies) * 1000, 1) if latencies else 0,
            "p95": round(latencies[int(len(latencies) * 0.95)] * 1000, 1) if latencies else 0,
            "p99": round(latencies[int(len(latencies) * 0.99)] * 1000, 1) if latencies else 0,
            "max": round(max(latencies) * 1000, 1) if latencies else 0,
        },
        "funded_traders": funded,
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
    }
    report_path = ROOT / "tests" / "artifacts" / "load-test-report.json"
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(json.dumps(report, indent=2))
    print(f"\n  Report: {report_path}")


if __name__ == "__main__":
    asyncio.run(main())
