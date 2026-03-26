#!/usr/bin/env python3
"""
02_place_sell_orders.py — Place graduated LICN sell orders on the LICN/lUSD CLOB.

The reserve_pool wallet places limit sell orders at increasing price levels,
creating visible sell-side depth for users wanting to buy LICN with lUSD.

This uses the real contract call flow:
  1. Load reserve_pool keypair
  2. Resolve dex_core contract address
  3. Build place_order binary instruction (opcode 0x02)
  4. Sign and send transaction via RPC
  5. Orders propagate to all validators via consensus

Price levels are spread in $0.002 increments for realistic order book depth.

Usage:
  python3 02_place_sell_orders.py --rpc http://15.204.229.189:8899 --network testnet
  python3 02_place_sell_orders.py --rpc http://15.204.229.189:8899 --network testnet --dry-run
"""

import argparse
import asyncio
import json
import struct
import sys
import time
from pathlib import Path
from typing import List, Tuple

ROOT = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(ROOT / "sdk" / "python"))

from lichen import Connection, Instruction, Keypair, PublicKey, TransactionBuilder

CONTRACT_PROGRAM = PublicKey(b"\xff" * 32)
SPORES_PER_LICN = 1_000_000_000  # 1 LICN = 1e9 spores
PRICE_SCALE = 1_000_000_000      # prices stored as u64 scaled by 1e9
TX_CONFIRM_TIMEOUT = 20

# LICN/lUSD pair ID (pair 1, created at genesis)
LICN_LUSD_PAIR_ID = 1

# ═══════════════════════════════════════════════════════════════════════════════
# Sell order levels — graduated price/quantity
# Spread across $0.002 increments for realistic depth
# Total: 10,000,000 LICN across 25 levels ($0.100 → $0.148)
# ═══════════════════════════════════════════════════════════════════════════════

SELL_LEVELS: List[Tuple[float, float]] = [
    # (price_usd, quantity_licn)
    # ── Dense zone near genesis price ($0.100–$0.110) ──
    (0.100, 800_000),
    (0.102, 800_000),
    (0.104, 700_000),
    (0.106, 700_000),
    (0.108, 600_000),
    (0.110, 600_000),
    # ── Mid zone ($0.112–$0.126) ──
    (0.112, 500_000),
    (0.114, 500_000),
    (0.116, 400_000),
    (0.118, 400_000),
    (0.120, 400_000),
    (0.122, 350_000),
    (0.124, 350_000),
    (0.126, 300_000),
    # ── Upper zone ($0.128–$0.148) ──
    (0.128, 300_000),
    (0.130, 300_000),
    (0.132, 250_000),
    (0.134, 250_000),
    (0.136, 200_000),
    (0.138, 200_000),
    (0.140, 200_000),
    (0.142, 200_000),
    (0.144, 150_000),
    (0.146, 150_000),
    (0.148, 150_000),
]


# ═══════════════════════════════════════════════════════════════════════════════
# Keypair loading
# ═══════════════════════════════════════════════════════════════════════════════

def load_keypair(path: Path) -> Keypair:
    raw = json.loads(path.read_text(encoding="utf-8"))
    if "secret_key" in raw:
        return Keypair.from_seed(bytes.fromhex(raw["secret_key"]))
    return Keypair.load(path)


def find_keypair(network: str, role: str) -> Path:
    for base in [
        ROOT / "artifacts" / network / "genesis-keys",
        ROOT / "data" / f"state-{network}" / "genesis-keys",
    ]:
        if base.exists():
            for f in base.glob("*.json"):
                if f.name.startswith(role):
                    return f
    raise FileNotFoundError(f"No keypair for '{role}' on {network}")


# ═══════════════════════════════════════════════════════════════════════════════
# Contract helpers
# ═══════════════════════════════════════════════════════════════════════════════

async def resolve_contract(conn: Connection, symbol: str) -> PublicKey:
    result = await conn._rpc("getAllSymbolRegistry", [100])
    entries = result.get("entries", []) if isinstance(result, dict) else result
    for entry in entries:
        if entry.get("symbol") == symbol:
            addr = entry.get("program", "")
            if addr:
                return PublicKey.from_base58(addr)
    raise ValueError(f"Contract '{symbol}' not found in registry")


def build_place_order(
    caller_pubkey: bytes,
    pair_id: int,
    side: int,       # 0=buy, 1=sell
    order_type: int,  # 0=limit
    price: int,       # scaled by PRICE_SCALE
    quantity: int,    # in spores
    expiry: int = 0,  # 0 = GTC (good-til-cancelled)
    trigger_price: int = 0,
) -> bytes:
    """Build binary instruction for dex_core.place_order (opcode 0x02).

    Layout (75 bytes):
      [0]      opcode = 0x02
      [1:33]   caller pubkey (32 bytes)
      [33:41]  pair_id (u64 LE)
      [41]     side (u8)
      [42]     order_type (u8)
      [43:51]  price (u64 LE)
      [51:59]  quantity (u64 LE)
      [59:67]  expiry (u64 LE)
      [67:75]  trigger_price (u64 LE)
    """
    buf = bytearray(75)
    buf[0] = 0x02  # place_order opcode
    buf[1:33] = caller_pubkey[:32]
    struct.pack_into("<Q", buf, 33, pair_id)
    buf[41] = side
    buf[42] = order_type
    struct.pack_into("<Q", buf, 43, price)
    struct.pack_into("<Q", buf, 51, quantity)
    struct.pack_into("<Q", buf, 59, expiry)
    struct.pack_into("<Q", buf, 67, trigger_price)
    return bytes(buf)


async def send_dex_order(
    conn: Connection,
    signer: Keypair,
    dex_addr: PublicKey,
    order_bytes: bytes,
) -> str:
    """Send a dex_core contract call (dispatcher pattern)."""
    # Dispatcher contracts use function="call" in the envelope
    envelope = json.dumps({
        "Call": {
            "function": "call",
            "args": list(order_bytes),
            "value": 0,
        }
    })
    data = envelope.encode("utf-8")

    ix = Instruction(
        CONTRACT_PROGRAM,
        [signer.public_key(), dex_addr],
        data,
    )

    tb = TransactionBuilder()
    tb.add(ix)
    latest = await conn.get_latest_block()
    blockhash = latest.get("hash", latest.get("blockhash", "0" * 64))
    tb.set_recent_blockhash(blockhash)
    tx = tb.build_and_sign(signer)
    return await conn.send_transaction(tx)


async def wait_for_tx(conn: Connection, sig: str, timeout: int = TX_CONFIRM_TIMEOUT):
    for _ in range(timeout * 5):
        await asyncio.sleep(0.2)
        try:
            info = await conn.get_transaction(sig)
            if info:
                return info
        except Exception:
            pass
    return None


# ═══════════════════════════════════════════════════════════════════════════════
# Main
# ═══════════════════════════════════════════════════════════════════════════════

async def main():
    parser = argparse.ArgumentParser(description="Place LICN sell orders on LICN/lUSD CLOB")
    parser.add_argument("--rpc", default="http://127.0.0.1:8899", help="RPC endpoint")
    parser.add_argument("--network", default="testnet", choices=["testnet", "mainnet"])
    parser.add_argument("--reserve-key", type=str, default=None,
                        help="Path to reserve_pool keypair")
    parser.add_argument("--dry-run", action="store_true",
                        help="Print orders without sending")
    parser.add_argument("--delay", type=float, default=1.0,
                        help="Seconds between orders (default: 1.0)")
    parser.add_argument("--max-orders", type=int, default=None,
                        help="Maximum number of orders to place (for testing)")
    args = parser.parse_args()

    total_licn = sum(qty for _, qty in SELL_LEVELS)
    total_value = sum(price * qty for price, qty in SELL_LEVELS)

    print(f"{'='*60}")
    print(f"  LICN Sell Orders — {args.network}")
    print(f"{'='*60}")
    print(f"  RPC:          {args.rpc}")
    print(f"  Pair:         LICN/lUSD (ID {LICN_LUSD_PAIR_ID})")
    print(f"  Levels:       {len(SELL_LEVELS)}")
    print(f"  Total LICN:   {total_licn:,.0f}")
    print(f"  Total value:  ${total_value:,.0f} lUSD")
    print(f"  Price range:  ${SELL_LEVELS[0][0]:.3f} → ${SELL_LEVELS[-1][0]:.3f}")
    print()

    conn = Connection(args.rpc)

    # Load reserve_pool keypair
    reserve_path = Path(args.reserve_key) if args.reserve_key else find_keypair(args.network, "reserve_pool")
    reserve_kp = load_keypair(reserve_path)
    caller_bytes = reserve_kp.public_key().to_bytes()

    print(f"  Reserve pool: {reserve_kp.public_key()}")

    # Check balance
    bal = await conn.get_balance(reserve_kp.public_key())
    available = bal.get("spendable", bal.get("spores", 0))
    available_licn = available / SPORES_PER_LICN
    print(f"  LICN balance: {available_licn:,.4f}")

    if available_licn < total_licn:
        print(f"  ⚠️  Insufficient balance! Need {total_licn:,.0f}, have {available_licn:,.0f}")
        return

    # Resolve dex_core contract
    dex_addr = await resolve_contract(conn, "DEX")
    print(f"  DEX contract: {dex_addr}")
    print()

    if args.dry_run:
        print("  [DRY RUN] Orders that would be placed:")
        print(f"  {'Level':>5} {'Price':>10} {'Quantity':>15} {'Value (lUSD)':>15}")
        print(f"  {'-'*5} {'-'*10} {'-'*15} {'-'*15}")
        for i, (price, qty) in enumerate(SELL_LEVELS):
            value = price * qty
            print(f"  {i+1:>5} ${price:>9.3f} {qty:>14,.0f} ${value:>14,.0f}")
        print(f"\n  Total: {total_licn:,.0f} LICN, ${total_value:,.0f} lUSD")
        return

    # Place orders
    orders_log = []
    limit = args.max_orders or len(SELL_LEVELS)

    print(f"  Placing {min(limit, len(SELL_LEVELS))} sell orders...")
    print(f"  {'#':>3} {'Price':>10} {'Qty (LICN)':>14} {'Status':>10} {'TX Sig':>20}")
    print(f"  {'-'*3} {'-'*10} {'-'*14} {'-'*10} {'-'*20}")

    for i, (price, qty) in enumerate(SELL_LEVELS[:limit]):
        price_scaled = int(round(price * PRICE_SCALE))
        qty_spores = int(qty * SPORES_PER_LICN)

        order_bytes = build_place_order(
            caller_pubkey=caller_bytes,
            pair_id=LICN_LUSD_PAIR_ID,
            side=1,          # sell
            order_type=0,    # limit
            price=price_scaled,
            quantity=qty_spores,
        )

        try:
            sig = await send_dex_order(conn, reserve_kp, dex_addr, order_bytes)
            short_sig = sig[:12] + "..." if len(sig) > 12 else sig

            # Wait for confirmation
            result = await wait_for_tx(conn, sig)
            status = "✅" if result else "⏳"

            print(f"  {i+1:>3} ${price:>9.3f} {qty:>13,.0f} {status:>10} {short_sig}")

            orders_log.append({
                "level": i + 1,
                "price": price,
                "quantity": qty,
                "side": "sell",
                "pair": "LICN/lUSD",
                "tx_sig": sig,
                "confirmed": result is not None,
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            })

        except Exception as e:
            print(f"  {i+1:>3} ${price:>9.3f} {qty:>13,.0f} {'❌':>10} {str(e)[:40]}")
            orders_log.append({
                "level": i + 1,
                "price": price,
                "quantity": qty,
                "side": "sell",
                "pair": "LICN/lUSD",
                "error": str(e),
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            })

        # Delay between orders to avoid nonce/blockhash issues
        if i < limit - 1:
            await asyncio.sleep(args.delay)

    # Save order log
    log_file = Path(__file__).parent / f"orders_sell_{args.network}.json"
    log_file.write_text(json.dumps(orders_log, indent=2))
    print(f"\n  Order log saved to: {log_file}")

    confirmed = sum(1 for o in orders_log if o.get("confirmed"))
    failed = sum(1 for o in orders_log if "error" in o)
    print(f"\n  Summary: {confirmed} confirmed, {len(orders_log) - confirmed - failed} pending, {failed} failed")


if __name__ == "__main__":
    asyncio.run(main())
