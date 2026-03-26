#!/usr/bin/env python3
"""
03_place_buy_orders.py — Place graduated lUSD buy orders on the LICN/lUSD CLOB.

The reserve_pool wallet places limit buy orders (buying LICN with lUSD) at
decreasing price levels, creating a buy wall for users wanting to sell LICN.

Requires: lUSD already minted into reserve_pool (run 01_mint_lusd.py first).

Usage:
  python3 03_place_buy_orders.py --rpc http://15.204.229.189:8899 --network testnet
  python3 03_place_buy_orders.py --rpc http://15.204.229.189:8899 --network testnet --dry-run
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
SPORES_PER_LICN = 1_000_000_000
PRICE_SCALE = 1_000_000_000
TX_CONFIRM_TIMEOUT = 20
LICN_LUSD_PAIR_ID = 1

# ═══════════════════════════════════════════════════════════════════════════════
# Buy order levels — graduated price/quantity (buying LICN with lUSD)
# Spread across $0.002 decrements below genesis price
# Total: ~2,500,000 lUSD across 25 levels ($0.098 → $0.050)
# ═══════════════════════════════════════════════════════════════════════════════

BUY_LEVELS: List[Tuple[float, float]] = [
    # (price_usd, quantity_licn)
    # quantity_licn = how much LICN we want to buy at this price
    # cost in lUSD = price * quantity
    # ── Tight support zone ($0.098–$0.088) ──
    (0.098, 400_000),   # $39,200
    (0.096, 400_000),   # $38,400
    (0.094, 350_000),   # $32,900
    (0.092, 350_000),   # $32,200
    (0.090, 350_000),   # $31,500
    (0.088, 300_000),   # $26,400
    # ── Mid support ($0.086–$0.074) ──
    (0.086, 300_000),   # $25,800
    (0.084, 300_000),   # $25,200
    (0.082, 250_000),   # $20,500
    (0.080, 250_000),   # $20,000
    (0.078, 250_000),   # $19,500
    (0.076, 200_000),   # $15,200
    (0.074, 200_000),   # $14,800
    # ── Deep support ($0.072–$0.050) ──
    (0.072, 200_000),   # $14,400
    (0.070, 200_000),   # $14,000
    (0.068, 150_000),   # $10,200
    (0.066, 150_000),   # $9,900
    (0.064, 150_000),   # $9,600
    (0.062, 150_000),   # $9,300
    (0.060, 150_000),   # $9,000
    (0.058, 100_000),   # $5,800
    (0.056, 100_000),   # $5,600
    (0.054, 100_000),   # $5,400
    (0.052, 100_000),   # $5,200
    (0.050, 100_000),   # $5,000
]


# ═══════════════════════════════════════════════════════════════════════════════
# Shared helpers (same as 02_place_sell_orders.py)
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
    side: int,
    order_type: int,
    price: int,
    quantity: int,
    expiry: int = 0,
    trigger_price: int = 0,
) -> bytes:
    buf = bytearray(75)
    buf[0] = 0x02
    buf[1:33] = caller_pubkey[:32]
    struct.pack_into("<Q", buf, 33, pair_id)
    buf[41] = side
    buf[42] = order_type
    struct.pack_into("<Q", buf, 43, price)
    struct.pack_into("<Q", buf, 51, quantity)
    struct.pack_into("<Q", buf, 59, expiry)
    struct.pack_into("<Q", buf, 67, trigger_price)
    return bytes(buf)


async def send_dex_order(conn, signer, dex_addr, order_bytes):
    envelope = json.dumps({
        "Call": {
            "function": "call",
            "args": list(order_bytes),
            "value": 0,
        }
    })
    data = envelope.encode("utf-8")
    ix = Instruction(CONTRACT_PROGRAM, [signer.public_key(), dex_addr], data)
    tb = TransactionBuilder()
    tb.add(ix)
    latest = await conn.get_latest_block()
    blockhash = latest.get("hash", latest.get("blockhash", "0" * 64))
    tb.set_recent_blockhash(blockhash)
    tx = tb.build_and_sign(signer)
    return await conn.send_transaction(tx)


async def wait_for_tx(conn, sig, timeout=TX_CONFIRM_TIMEOUT):
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
    parser = argparse.ArgumentParser(description="Place LICN buy orders on LICN/lUSD CLOB")
    parser.add_argument("--rpc", default="http://127.0.0.1:8899", help="RPC endpoint")
    parser.add_argument("--network", default="testnet", choices=["testnet", "mainnet"])
    parser.add_argument("--reserve-key", type=str, default=None)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--delay", type=float, default=1.0)
    parser.add_argument("--max-orders", type=int, default=None)
    args = parser.parse_args()

    total_licn = sum(qty for _, qty in BUY_LEVELS)
    total_lusd = sum(price * qty for price, qty in BUY_LEVELS)

    print(f"{'='*60}")
    print(f"  LICN Buy Orders — {args.network}")
    print(f"{'='*60}")
    print(f"  RPC:          {args.rpc}")
    print(f"  Pair:         LICN/lUSD (ID {LICN_LUSD_PAIR_ID})")
    print(f"  Levels:       {len(BUY_LEVELS)}")
    print(f"  Total LICN:   {total_licn:,.0f} (to buy)")
    print(f"  Total lUSD:   ${total_lusd:,.0f} (cost)")
    print(f"  Price range:  ${BUY_LEVELS[0][0]:.3f} → ${BUY_LEVELS[-1][0]:.3f}")
    print()

    conn = Connection(args.rpc)

    reserve_path = Path(args.reserve_key) if args.reserve_key else find_keypair(args.network, "reserve_pool")
    reserve_kp = load_keypair(reserve_path)
    caller_bytes = reserve_kp.public_key().to_bytes()

    print(f"  Reserve pool: {reserve_kp.public_key()}")

    bal = await conn.get_balance(reserve_kp.public_key())
    available = bal.get("spendable", bal.get("spores", 0))
    print(f"  LICN balance: {available / SPORES_PER_LICN:,.4f}")

    dex_addr = await resolve_contract(conn, "DEX")
    print(f"  DEX contract: {dex_addr}")
    print()

    if args.dry_run:
        print("  [DRY RUN] Orders that would be placed:")
        print(f"  {'Level':>5} {'Price':>10} {'Qty (LICN)':>14} {'Cost (lUSD)':>14}")
        print(f"  {'-'*5} {'-'*10} {'-'*14} {'-'*14}")
        for i, (price, qty) in enumerate(BUY_LEVELS):
            cost = price * qty
            print(f"  {i+1:>5} ${price:>9.3f} {qty:>13,.0f} ${cost:>13,.0f}")
        print(f"\n  Total: {total_licn:,.0f} LICN, ${total_lusd:,.0f} lUSD")
        return

    orders_log = []
    limit = args.max_orders or len(BUY_LEVELS)

    print(f"  Placing {min(limit, len(BUY_LEVELS))} buy orders...")
    print(f"  {'#':>3} {'Price':>10} {'Qty (LICN)':>14} {'Status':>10} {'TX Sig':>20}")
    print(f"  {'-'*3} {'-'*10} {'-'*14} {'-'*10} {'-'*20}")

    for i, (price, qty) in enumerate(BUY_LEVELS[:limit]):
        price_scaled = int(round(price * PRICE_SCALE))
        qty_spores = int(qty * SPORES_PER_LICN)

        order_bytes = build_place_order(
            caller_pubkey=caller_bytes,
            pair_id=LICN_LUSD_PAIR_ID,
            side=0,          # buy
            order_type=0,    # limit
            price=price_scaled,
            quantity=qty_spores,
        )

        try:
            sig = await send_dex_order(conn, reserve_kp, dex_addr, order_bytes)
            short_sig = sig[:12] + "..." if len(sig) > 12 else sig
            result = await wait_for_tx(conn, sig)
            status = "✅" if result else "⏳"
            print(f"  {i+1:>3} ${price:>9.3f} {qty:>13,.0f} {status:>10} {short_sig}")
            orders_log.append({
                "level": i + 1, "price": price, "quantity": qty,
                "side": "buy", "pair": "LICN/lUSD", "tx_sig": sig,
                "confirmed": result is not None,
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            })
        except Exception as e:
            print(f"  {i+1:>3} ${price:>9.3f} {qty:>13,.0f} {'❌':>10} {str(e)[:40]}")
            orders_log.append({
                "level": i + 1, "price": price, "quantity": qty,
                "side": "buy", "pair": "LICN/lUSD", "error": str(e),
                "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            })

        if i < limit - 1:
            await asyncio.sleep(args.delay)

    log_file = Path(__file__).parent / f"orders_buy_{args.network}.json"
    log_file.write_text(json.dumps(orders_log, indent=2))
    print(f"\n  Order log saved to: {log_file}")

    confirmed = sum(1 for o in orders_log if o.get("confirmed"))
    failed = sum(1 for o in orders_log if "error" in o)
    print(f"\n  Summary: {confirmed} confirmed, {len(orders_log) - confirmed - failed} pending, {failed} failed")


if __name__ == "__main__":
    asyncio.run(main())
