#!/usr/bin/env python3
"""Seed DEX with CLOB orders and AMM liquidity from reserve_pool.

Per DEX_LIQUIDITY_STRATEGY.md:
  Phase 1:  Graduated LICN/lUSD sell-wall + buy-wall on CLOB (25 levels each)
  Phase 1b: Orders on all 7 trading pairs at oracle cross-rates
  Phase 2:  Concentrated liquidity positions on all 7 AMM pools

Uses the reserve_pool wallet (50M LICN) as the initial protocol market maker.
Pre-requisite: run mint_protocol_lusd.py first to mint lUSD + testnet wrapped tokens.

Usage:
  python tools/seed_dex_liquidity.py
  LICHEN_RPC_URL=http://host:8899 python tools/seed_dex_liquidity.py
"""
import sys, os, struct, asyncio, json, math, urllib.request
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, Keypair, PublicKey

sys.path.insert(0, os.path.dirname(__file__))
from deploy_dex import call_contract_raw

SPORES = 1_000_000_000  # 1 token = 1B spores
RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')
NETWORK = os.environ.get('LICHEN_NETWORK', 'testnet')

# Genesis pair IDs (order from genesis_auto_pairs_and_pools)
PAIR_IDS = {
    "LICN/lUSD": 1, "wSOL/lUSD": 2, "wETH/lUSD": 3,
    "wSOL/LICN": 4, "wETH/LICN": 5, "wBNB/lUSD": 6, "wBNB/LICN": 7,
}
POOL_IDS = PAIR_IDS  # Same ordering

# CLOB constants
SIDE_BUY = 0
SIDE_SELL = 1
ORDER_LIMIT = 0
EXPIRY_SLOTS = 2_592_000  # ~30 days at 400ms/slot


def fetch_prices():
    """Fetch live prices from Binance."""
    urls = [
        'https://api.binance.us/api/v3/ticker/price?symbols=["SOLUSDT","ETHUSDT","BNBUSDT"]',
        'https://api.binance.com/api/v3/ticker/price?symbols=["SOLUSDT","ETHUSDT","BNBUSDT"]',
    ]
    sym_map = {'SOLUSDT': 'SOL', 'ETHUSDT': 'ETH', 'BNBUSDT': 'BNB'}
    prices = {}
    for url in urls:
        try:
            req = urllib.request.Request(url, headers={'User-Agent': 'Lichen/1.0'})
            with urllib.request.urlopen(req, timeout=10) as resp:
                for item in json.loads(resp.read()):
                    if item['symbol'] in sym_map:
                        prices[sym_map[item['symbol']]] = float(item['price'])
            if len(prices) >= 3:
                break
        except Exception:
            continue
    if not prices:
        prices = {'SOL': 145.0, 'ETH': 2600.0, 'BNB': 620.0}
    prices['LICN'] = float(os.environ.get('LICHEN_USD_PRICE', '0.10'))
    return prices


def price_to_tick(p):
    """Convert price to concentrated liquidity tick index."""
    if p <= 0:
        return -443636
    return int(math.log(p) / math.log(1.0001))


# ── Contract call helpers ────────────────────────────────────────────────

async def place_order(conn, caller, dex_core, pair_id, side, price_spores, qty_spores):
    """Place a limit order on CLOB. dex_core opcode 2."""
    caller_bytes = bytes(caller.public_key().to_bytes())
    args = (
        bytes([2])                                +  # opcode 2
        caller_bytes                              +  # trader (32B)
        struct.pack('<Q', pair_id)                +  # pair_id
        bytes([side])                             +  # side
        bytes([ORDER_LIMIT])                      +  # order_type = limit
        struct.pack('<Q', price_spores)           +  # price
        struct.pack('<Q', qty_spores)             +  # quantity
        struct.pack('<Q', EXPIRY_SLOTS)              # expiry
    )
    return await call_contract_raw(conn, caller, dex_core, 'call', list(args))


async def add_amm_liquidity(conn, caller, dex_amm, pool_id, lower_tick, upper_tick, amount_a, amount_b):
    """Add concentrated liquidity. dex_amm opcode 3."""
    caller_bytes = bytes(caller.public_key().to_bytes())
    args = (
        bytes([3])                                +  # opcode 3
        caller_bytes                              +  # provider (32B)
        struct.pack('<Q', pool_id)                +  # pool_id
        struct.pack('<i', lower_tick)             +  # lower_tick (i32)
        struct.pack('<i', upper_tick)             +  # upper_tick (i32)
        struct.pack('<Q', amount_a)               +  # amount_a
        struct.pack('<Q', amount_b)                  # amount_b
    )
    return await call_contract_raw(conn, caller, dex_amm, 'call', list(args))


# ── Main ─────────────────────────────────────────────────────────────────

async def main():
    conn = Connection(RPC)
    repo = Path(__file__).resolve().parent.parent

    # Load reserve_pool keypair (protocol market maker)
    rp_path = repo / f"data/state-{NETWORK}/genesis-keys/reserve_pool-lichen-{NETWORK}-1.json"
    if not rp_path.exists():
        print(f"ERROR: reserve_pool keypair not found at {rp_path}")
        sys.exit(1)
    reserve = Keypair.load(rp_path)
    print(f"  Market maker:  {reserve.public_key()}")

    # Discover dex_core and dex_amm from symbol registry
    result = await conn._rpc("getAllSymbolRegistry")
    entries = result.get("entries", [])
    contracts = {}
    for e in entries:
        sym = e.get("symbol", "")
        prog = e.get("program", "")
        if sym == "DEX" and prog:
            contracts["dex_core"] = PublicKey.from_base58(prog)
        elif sym == "DEXAMM" and prog:
            contracts["dex_amm"] = PublicKey.from_base58(prog)

    dex_core = contracts.get("dex_core")
    dex_amm = contracts.get("dex_amm")
    if not dex_core or not dex_amm:
        print(f"  ERROR: Missing contracts — dex_core={dex_core}, dex_amm={dex_amm}")
        sys.exit(1)
    print(f"  dex_core:      {dex_core}")
    print(f"  dex_amm:       {dex_amm}")

    # Fetch live prices
    prices = fetch_prices()
    print(f"\n  Live prices:")
    for sym, usd in sorted(prices.items()):
        print(f"    {sym}/USD = ${usd:,.2f}")

    licn = prices['LICN']
    sol = prices['SOL']
    eth = prices['ETH']
    bnb = prices['BNB']

    total_orders = 0

    # ═════════════════════════════════════════════════════════════════════
    #  Phase 1: CLOB Graduated Orders — LICN/lUSD
    # ═════════════════════════════════════════════════════════════════════
    print(f"\n{'═' * 60}")
    print(f"  Phase 1: LICN/lUSD CLOB Order Seeding")
    print(f"{'═' * 60}")

    pair_id = PAIR_IDS["LICN/lUSD"]

    # ── Sell wall: 25 levels from $0.100 to $0.148 ($0.002 increments) ──
    # Dense near genesis price, thinner further out (~8.6M LICN total)
    sell_levels = []
    for i in range(25):
        p = 0.100 + i * 0.002
        if i < 6:
            q = 700_000      # ~4.2M LICN in tight zone
        elif i < 13:
            q = 457_000      # ~3.2M LICN in mid zone
        else:
            q = 183_000      # ~2.2M LICN in upper zone
        sell_levels.append((p, q))

    # ── Buy wall: 25 levels from $0.098 to $0.050 ($0.002 decrements) ──
    buy_levels = []
    for i in range(25):
        p = 0.098 - i * 0.002
        if p <= 0:
            break
        if i < 6:
            q = 358_000      # ~2.15M LICN tight support
        elif i < 13:
            q = 250_000      # ~1.75M LICN mid support
        else:
            q = 137_000      # ~1.65M LICN deep support
        buy_levels.append((p, q))

    total_sell_licn = sum(q for _, q in sell_levels)
    total_buy_licn = sum(q for _, q in buy_levels)
    total_buy_lusd = sum(p * q for p, q in buy_levels)
    print(f"  Sell wall: {total_sell_licn:,.0f} LICN across {len(sell_levels)} levels")
    print(f"  Buy wall:  {total_buy_licn:,.0f} LICN / ~{total_buy_lusd:,.0f} lUSD across {len(buy_levels)} levels")

    for p, q in sell_levels:
        try:
            sig = await place_order(conn, reserve, dex_core, pair_id, SIDE_SELL,
                                    int(p * SPORES), q * SPORES)
            print(f"    SELL {q:>10,} LICN @ ${p:.3f}  ✓")
            total_orders += 1
        except Exception as e:
            print(f"    SELL @ ${p:.3f}: {e}")

    for p, q in buy_levels:
        try:
            sig = await place_order(conn, reserve, dex_core, pair_id, SIDE_BUY,
                                    int(p * SPORES), q * SPORES)
            print(f"    BUY  {q:>10,} LICN @ ${p:.3f}  ✓")
            total_orders += 1
        except Exception as e:
            print(f"    BUY  @ ${p:.3f}: {e}")

    # ═════════════════════════════════════════════════════════════════════
    #  Phase 1b: CLOB Orders on Wrapped Token Pairs
    # ═════════════════════════════════════════════════════════════════════
    print(f"\n{'═' * 60}")
    print(f"  Phase 1b: Wrapped Token Pair CLOB Seeding")
    print(f"{'═' * 60}")

    # (pair_name, pair_id, base_price_in_quote, lot_size_tokens, num_levels)
    wrapped_pairs = [
        ("wSOL/lUSD", 2, sol,       50,  10),
        ("wETH/lUSD", 3, eth,       5,   10),
        ("wBNB/lUSD", 6, bnb,       20,  10),
        ("wSOL/LICN", 4, sol / licn, 50,  10),
        ("wETH/LICN", 5, eth / licn, 5,   10),
        ("wBNB/LICN", 7, bnb / licn, 20,  10),
    ]

    for name, pid, base_price, lot, nlevels in wrapped_pairs:
        pair_orders = 0
        spread_step = base_price * 0.01  # 1% per level
        for i in range(nlevels):
            sell_p = base_price + (i + 1) * spread_step
            buy_p = base_price - (i + 1) * spread_step
            if buy_p <= 0:
                continue
            qty = lot * SPORES
            try:
                await place_order(conn, reserve, dex_core, pid, SIDE_SELL,
                                  int(sell_p * SPORES), qty)
                pair_orders += 1
            except Exception:
                pass
            try:
                await place_order(conn, reserve, dex_core, pid, SIDE_BUY,
                                  int(buy_p * SPORES), qty)
                pair_orders += 1
            except Exception:
                pass
        total_orders += pair_orders
        print(f"    {name}: {pair_orders} orders placed")

    print(f"\n  Total CLOB orders: {total_orders}")

    # ═════════════════════════════════════════════════════════════════════
    #  Phase 2: AMM Concentrated Liquidity
    # ═════════════════════════════════════════════════════════════════════
    print(f"\n{'═' * 60}")
    print(f"  Phase 2: AMM Concentrated Liquidity Seeding")
    print(f"{'═' * 60}")

    # (name, pool_id, current_price, range_low, range_high, amount_a_tokens, amount_b_tokens)
    amm_pools = [
        ("LICN/lUSD", 1, licn,      licn * 0.5,      licn * 2.5,      5_000_000, 500_000),
        ("wSOL/lUSD", 2, sol,        sol * 0.7,        sol * 1.4,       500,       50_000),
        ("wETH/lUSD", 3, eth,        eth * 0.7,        eth * 1.4,       25,        50_000),
        ("wSOL/LICN", 4, sol / licn, sol / licn * 0.6, sol / licn * 1.5, 500,      500_000),
        ("wETH/LICN", 5, eth / licn, eth / licn * 0.6, eth / licn * 1.5, 25,       500_000),
        ("wBNB/lUSD", 6, bnb,        bnb * 0.7,        bnb * 1.4,       100,       50_000),
        ("wBNB/LICN", 7, bnb / licn, bnb / licn * 0.6, bnb / licn * 1.5, 100,      500_000),
    ]

    pools_seeded = 0
    for name, pid, price, low, high, amt_a, amt_b in amm_pools:
        lt = price_to_tick(low)
        ut = price_to_tick(high)
        a_spores = amt_a * SPORES
        b_spores = amt_b * SPORES
        try:
            sig = await add_amm_liquidity(conn, reserve, dex_amm, pid,
                                          lt, ut, a_spores, b_spores)
            print(f"    ✅ {name}: {amt_a:>12,} / {amt_b:>12,}  ticks=[{lt}, {ut}]")
            pools_seeded += 1
        except Exception as e:
            print(f"    ❌ {name}: {e}")

    print(f"\n  {pools_seeded}/7 AMM pools seeded")

    # ── Summary ──
    print(f"\n{'═' * 60}")
    print(f"  DEX Liquidity Seeding Complete")
    print(f"{'═' * 60}")
    print(f"  CLOB orders placed:  {total_orders}")
    print(f"  AMM pools seeded:    {pools_seeded}/7")
    print(f"  Market maker wallet: {reserve.public_key()}")


asyncio.run(main())
