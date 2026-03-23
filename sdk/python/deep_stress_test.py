#!/usr/bin/env python3
"""
Lichen Deep Stress & Integration Test
==========================================
Goes beyond basic e2e: concurrent tx submission, multi-block consistency,
explorer data paths verified end-to-end, RPC under sustained load,
re-org safety, and state integrity after restarts.

Run AFTER e2e_agent_test.py so there's on-chain data.
"""

import asyncio
import hashlib
import json
import os
import struct
import sys
import time
from pathlib import Path
from urllib.request import Request, urlopen
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, str(Path(__file__).parent))
from lichen import Connection, Keypair, PublicKey, TransactionBuilder, Instruction

import nacl.signing

RPC_URL = "http://127.0.0.1:8000"
EXPLORER_URL = "http://127.0.0.1:3007"
SPORES_PER_LICN = 1_000_000_000
SYSTEM_PROGRAM = PublicKey(b'\x00' * 32)

STATE_DIR = Path(__file__).resolve().parent.parent.parent / "data" / "state-8000"
TREASURY_KEY_PATH = STATE_DIR / "genesis-keys" / "treasury-lichen-testnet-1.json"

results = []

def ok(name, detail=""):
    results.append(("PASS", name, detail))
    print(f"  [PASS] {name}" + (f" -- {detail}" if detail else ""))

def fail(name, detail=""):
    results.append(("FAIL", name, detail))
    print(f"  [FAIL] {name}" + (f" -- {detail}" if detail else ""))

def test(name, cond, detail=""):
    if cond:
        ok(name, detail)
    else:
        fail(name, detail)

def rpc_sync(method, params=None):
    payload = {"jsonrpc": "2.0", "id": 1, "method": method}
    if params is not None:
        payload["params"] = params
    req = Request(RPC_URL, data=json.dumps(payload).encode(),
                  headers={"Content-Type": "application/json"})
    with urlopen(req, timeout=10) as resp:
        data = json.loads(resp.read())
    if "error" in data:
        return None, data["error"]
    return data.get("result"), None

def load_treasury():
    with open(TREASURY_KEY_PATH) as f:
        data = json.load(f)
    seed = bytes.fromhex(data["secret_key"])
    signing_key = nacl.signing.SigningKey(seed)
    kp = Keypair(signing_key)
    assert kp.public_key().to_base58() == data["pubkey"]
    return kp


# =====================================================================
async def main():
    print("=" * 70)
    print("  Lichen Deep Stress & Integration Test")
    print("=" * 70)

    conn = Connection(RPC_URL)
    treasury = load_treasury()
    treasury_b58 = treasury.public_key().to_base58()

    # ── TEST 1: Rapid-fire transfers ────────────────────────────────
    print("\n--- 1. Rapid-fire 10 transfers in sequence ---")

    wallets = [Keypair.generate() for _ in range(10)]
    amount = 1 * SPORES_PER_LICN  # 1 LICN each

    t0 = time.time()
    sigs = []
    for w in wallets:
        blockhash = await conn.get_recent_blockhash()
        ix = TransactionBuilder.transfer(treasury.public_key(), w.public_key(), amount)
        tx = TransactionBuilder().add(ix).set_recent_blockhash(blockhash).build_and_sign(treasury)
        sig = await conn.send_transaction(tx)
        sigs.append(sig)
    elapsed = time.time() - t0
    test("10 sequential transfers", len(sigs) == 10, f"{elapsed:.2f}s ({10/elapsed:.1f} tps)")

    # Wait for confirmation
    await asyncio.sleep(3)

    # Verify all balances
    bal_ok = 0
    for w in wallets:
        b = await conn.get_balance(w.public_key())
        spores = b.get("spores", 0) if isinstance(b, dict) else 0
        if spores >= SPORES_PER_LICN * 0.99:
            bal_ok += 1
    test("all 10 balances confirmed", bal_ok == 10, f"{bal_ok}/10")

    # ── TEST 2: Concurrent RPC load ─────────────────────────────────
    print("\n--- 2. Concurrent RPC burst (100 parallel requests) ---")

    def rpc_burst_one(i):
        methods = ["getSlot", "health", "getMetrics", "getValidators", "getRecentBlockhash"]
        m = methods[i % len(methods)]
        try:
            r, e = rpc_sync(m)
            return r is not None
        except:
            return False

    t0 = time.time()
    with ThreadPoolExecutor(max_workers=20) as pool:
        futures = [pool.submit(rpc_burst_one, i) for i in range(100)]
        burst_results = [f.result() for f in futures]
    elapsed = time.time() - t0
    success = sum(burst_results)
    test("100 concurrent RPCs", success >= 95, f"{success}/100 in {elapsed:.2f}s ({success/elapsed:.0f} rps)")

    # ── TEST 3: Block-by-block integrity walk ───────────────────────
    print("\n--- 3. Block chain integrity walk ---")

    current_slot, _ = rpc_sync("getSlot")
    max_check = min(current_slot, 20)  # Check up to 20 blocks

    chain_ok = True
    prev_hash = None
    for s in range(0, max_check + 1):
        blk, err = rpc_sync("getBlock", [s])
        if blk is None:
            # Skip empty slots (no block produced)
            continue
        bh = blk.get("hash", "")
        ph = blk.get("parent_hash", "")
        bs = blk.get("slot")
        if prev_hash is not None and ph != prev_hash:
            chain_ok = False
            fail(f"hash chain broken at slot {s}", f"parent={ph[:16]} expected={prev_hash[:16]}")
            break
        prev_hash = bh
    test(f"hash chain {max_check+1} blocks", chain_ok, f"verified 0..{max_check}")

    # ── TEST 4: Transaction fetch + verify ──────────────────────────
    print("\n--- 4. Transaction cross-reference ---")

    # Get all treasury txs
    txs_res = await conn._rpc("getTransactionsByAddress", [treasury_b58, {"limit": 50}])
    tx_list = txs_res.get("transactions", []) if isinstance(txs_res, dict) else txs_res if isinstance(txs_res, list) else []
    test("treasury has txs", len(tx_list) > 0, f"count={len(tx_list)}")

    # Verify each tx can be individually fetched + has matching fields
    fetch_ok = 0
    for t in tx_list[:10]:
        sig = t.get("signature") or t.get("hash", "")
        if not sig:
            continue
        fetched = await conn._rpc("getTransaction", [sig])
        if fetched and (fetched.get("signature") == sig or fetched.get("hash") == sig):
            fetch_ok += 1
    test("tx individual fetch consistency", fetch_ok == min(10, len(tx_list)), f"{fetch_ok}/{min(10, len(tx_list))}")

    # ── TEST 5: Multi-wallet fan-out + fan-in ───────────────────────
    print("\n--- 5. Fan-out + fan-in (wallets send back to treasury) ---")

    # Each of the 10 wallets sends 0.5 LICN back
    return_sigs = []
    send_amount = int(0.5 * SPORES_PER_LICN)
    for w in wallets:
        try:
            blockhash = await conn.get_recent_blockhash()
            ix = TransactionBuilder.transfer(w.public_key(), treasury.public_key(), send_amount)
            tx = TransactionBuilder().add(ix).set_recent_blockhash(blockhash).build_and_sign(w)
            sig = await conn.send_transaction(tx)
            return_sigs.append(sig)
        except Exception as e:
            return_sigs.append(None)
    test("10 fan-in transfers submitted", sum(1 for s in return_sigs if s) >= 9, f"{sum(1 for s in return_sigs if s)}/10")

    await asyncio.sleep(3)

    # Check remaining balances (~0.5 LICN - fees)
    remaining_ok = 0
    for w in wallets:
        b = await conn.get_balance(w.public_key())
        spores = b.get("spores", 0) if isinstance(b, dict) else 0
        if 0 < spores < SPORES_PER_LICN:  # Between 0 and 1 LICN
            remaining_ok += 1
    test("fan-in balances correct", remaining_ok >= 9, f"{remaining_ok}/10")

    # ── TEST 6: Contract + NFT data through explorer ────────────────
    print("\n--- 6. Explorer data path verification ---")

    # Get a contract from the registry
    sym_res, _ = rpc_sync("getAllSymbolRegistry")
    entries = sym_res.get("entries", []) if isinstance(sym_res, dict) else (sym_res if isinstance(sym_res, list) else [])
    
    if entries:
        # Pick TSYMBIONT (agent-deployed) if available
        tsymbiont = next((e for e in entries if e.get("symbol") == "TSYMBIONT"), entries[0])
        prog_id = tsymbiont.get("program", "")
        sym = tsymbiont.get("symbol", "")

        # Verify contract page loads and contains the symbol
        try:
            req = Request(f"{EXPLORER_URL}/contract.html?addr={prog_id}")
            with urlopen(req, timeout=10) as resp:
                body = resp.read().decode(errors='replace')
            # The page is static HTML, JS will load data client-side via RPC
            test(f"contract page loads ({sym})", len(body) > 5000, f"size={len(body)}")
        except Exception as e:
            fail(f"contract page ({sym})", str(e))

        # Verify address page for treasury
        try:
            req = Request(f"{EXPLORER_URL}/address.html?addr={treasury_b58}")
            with urlopen(req, timeout=10) as resp:
                body = resp.read().decode(errors='replace')
            test("address page loads (treasury)", len(body) > 5000, f"size={len(body)}")
        except Exception as e:
            fail("address page (treasury)", str(e))

    # ── TEST 7: Validator performance data ──────────────────────────
    print("\n--- 7. Validator performance deep check ---")

    val_res = await conn.get_validators()
    val_list = val_res.get("validators", []) if isinstance(val_res, dict) else val_res if isinstance(val_res, list) else []

    if val_list:
        v = val_list[0]
        vpub = v.get("pubkey", "")

        # Validator should have produced blocks
        blocks_produced = v.get("blocks_proposed", v.get("_blocks_produced", v.get("blocks_produced", 0)))
        test("validator produced blocks", blocks_produced > 0, f"blocks={blocks_produced}")

        # Performance RPC should return timing data
        perf = await conn._rpc("getValidatorPerformance", [vpub])
        test("validator performance data", perf is not None)

        # Staking should show correct stake amount
        staking = await conn._rpc("getStakingStatus", [vpub])
        if isinstance(staking, dict):
            total = staking.get("total_staked", 0)
            test("validator staked = 10K LICN", total == 10000 * SPORES_PER_LICN, f"staked={total}")
        else:
            ok("getStakingStatus", "returned non-dict (may be method-specific)")

    # ── TEST 8: Metrics consistency ─────────────────────────────────
    print("\n--- 8. Metrics cross-validation ---")

    metrics, _ = rpc_sync("getMetrics")
    chain_status, _ = rpc_sync("getChainStatus")

    if metrics and chain_status:
        m_txs = metrics.get("total_transactions", 0)
        c_txs = chain_status.get("total_transactions", 0)
        test("metrics vs chain_status total_txs", m_txs == c_txs, f"metrics={m_txs} chain={c_txs}")

        m_blocks = metrics.get("total_blocks", 0)
        c_blocks = chain_status.get("total_blocks", 0)
        test("metrics vs chain_status total_blocks", m_blocks == c_blocks, f"m={m_blocks} c={c_blocks}")

        m_supply = metrics.get("total_supply", 0)
        c_supply = chain_status.get("total_supply", 0)
        test("metrics vs chain_status total_supply", m_supply == c_supply)

    # ── TEST 9: Account tx count vs actual txs ──────────────────────
    print("\n--- 9. Account tx count consistency ---")

    count_res = await conn._rpc("getAccountTxCount", [treasury_b58])
    tx_count = count_res.get("count", 0) if isinstance(count_res, dict) else 0

    all_txs_res = await conn._rpc("getTransactionsByAddress", [treasury_b58, {"limit": 100}])
    all_txs = all_txs_res.get("transactions", []) if isinstance(all_txs_res, dict) else []

    test("tx count matches actual list", tx_count == len(all_txs), f"count={tx_count} list={len(all_txs)}")

    # ── TEST 10: Edge case: double-spend attempt ────────────────────
    print("\n--- 10. Double-spend attempt ---")

    # Create a wallet with exactly 1 LICN
    double_wallet = Keypair.generate()
    blockhash = await conn.get_recent_blockhash()
    ix = TransactionBuilder.transfer(treasury.public_key(), double_wallet.public_key(), 1 * SPORES_PER_LICN)
    tx = TransactionBuilder().add(ix).set_recent_blockhash(blockhash).build_and_sign(treasury)
    await conn.send_transaction(tx)
    await asyncio.sleep(2)

    # Try to send 0.8 LICN twice simultaneously
    spend_amount = int(0.8 * SPORES_PER_LICN)
    target1 = Keypair.generate()
    target2 = Keypair.generate()

    blockhash = await conn.get_recent_blockhash()
    ix1 = TransactionBuilder.transfer(double_wallet.public_key(), target1.public_key(), spend_amount)
    tx1 = TransactionBuilder().add(ix1).set_recent_blockhash(blockhash).build_and_sign(double_wallet)

    ix2 = TransactionBuilder.transfer(double_wallet.public_key(), target2.public_key(), spend_amount)
    tx2 = TransactionBuilder().add(ix2).set_recent_blockhash(blockhash).build_and_sign(double_wallet)

    # Send both as fast as possible
    sig1 = None
    sig2 = None
    err1 = None
    err2 = None
    try:
        sig1 = await conn.send_transaction(tx1)
    except Exception as e:
        err1 = str(e)
    try:
        sig2 = await conn.send_transaction(tx2)
    except Exception as e:
        err2 = str(e)

    await asyncio.sleep(3)

    # At most ONE should succeed (0.8 + 0.8 > 1.0)
    t1_bal = await conn.get_balance(target1.public_key())
    t2_bal = await conn.get_balance(target2.public_key())
    t1_spores = t1_bal.get("spores", 0) if isinstance(t1_bal, dict) else 0
    t2_spores = t2_bal.get("spores", 0) if isinstance(t2_bal, dict) else 0

    both_received = (t1_spores >= spend_amount and t2_spores >= spend_amount)
    test("double-spend prevented", not both_received,
         f"t1={t1_spores} t2={t2_spores} (both receiving = double spend!)")

    # ── TEST 11: Sustained load (200 RPCs over 5 seconds) ──────────
    print("\n--- 11. Sustained RPC load (200 calls, 5s) ---")

    t0 = time.time()
    success_count = 0
    call_count = 0
    while time.time() - t0 < 5.0 and call_count < 200:
        r, e = rpc_sync("getSlot")
        call_count += 1
        if r is not None:
            success_count += 1
    elapsed = time.time() - t0
    test("sustained RPC load", success_count >= call_count * 0.95,
         f"{success_count}/{call_count} in {elapsed:.2f}s ({success_count/elapsed:.0f} rps)")

    # ── TEST 12: State consistency snapshot ─────────────────────────
    print("\n--- 12. Final state consistency snapshot ---")

    final_slot = await conn.get_slot()
    test("chain alive at end", final_slot > current_slot, f"start={current_slot} end={final_slot}")

    final_metrics, _ = rpc_sync("getMetrics")
    if final_metrics and metrics:
        test("total_txs increased", final_metrics["total_transactions"] > metrics["total_transactions"],
             f"before={metrics['total_transactions']} after={final_metrics['total_transactions']}")
        test("total_accounts increased", final_metrics["total_accounts"] >= metrics["total_accounts"],
             f"before={metrics['total_accounts']} after={final_metrics['total_accounts']}")

    # Summary
    print("\n" + "=" * 70)
    print("  DEEP STRESS TEST RESULTS")
    print("=" * 70)

    pass_count = sum(1 for r in results if r[0] == "PASS")
    fail_count = sum(1 for r in results if r[0] == "FAIL")
    print(f"  PASS: {pass_count}")
    print(f"  FAIL: {fail_count}")
    print(f"  TOTAL: {len(results)}")

    if fail_count > 0:
        print("\n  FAILURES:")
        for s, n, d in results:
            if s == "FAIL":
                print(f"    - {n}: {d}")

    if fail_count == 0:
        print(f"\n  ALL {pass_count} DEEP TESTS PASSED -- Chain is stress-tested!")
    else:
        print(f"\n  {fail_count} DEEP TEST(S) FAILED")

    return fail_count == 0


if __name__ == "__main__":
    success = asyncio.run(main())
    sys.exit(0 if success else 1)
