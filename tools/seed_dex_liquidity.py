#!/usr/bin/env python3
"""Seed DEX with CLOB orders and AMM liquidity from reserve_pool.

Per DEX_LIQUIDITY_STRATEGY.md:
  Phase 1:  Graduated LICN/lUSD sell-wall + buy-wall on CLOB (25 levels each)
  Phase 1b: Orders on all 11 trading pairs at oracle cross-rates
  Phase 2:  Concentrated liquidity positions on all 11 AMM pools

Uses the reserve_pool wallet (50M LICN) as the initial protocol market maker.
Pre-requisite: run mint_protocol_lusd.py first to mint lUSD + testnet wrapped tokens.

Usage:
  python tools/seed_dex_liquidity.py
  LICHEN_RPC_URL=http://host:8899 python tools/seed_dex_liquidity.py
"""
import sys, os, struct, asyncio, json, math, urllib.parse, urllib.request
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, Keypair, PublicKey

sys.path.insert(0, os.path.dirname(__file__))
from deploy_dex import call_contract_raw, find_genesis_keypair_path

SPORES = 1_000_000_000  # 1 token = 1B spores
RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')
NETWORK = os.environ.get('LICHEN_NETWORK', 'testnet')

# Genesis pair IDs (order from genesis_auto_pairs_and_pools)
PAIR_IDS = {
    "LICN/lUSD": 1, "wSOL/lUSD": 2, "wETH/lUSD": 3,
    "wSOL/LICN": 4, "wETH/LICN": 5, "wBNB/lUSD": 6, "wBNB/LICN": 7,
    "wNEO/lUSD": 8, "wNEO/LICN": 9, "wGAS/lUSD": 10, "wGAS/LICN": 11,
}
POOL_IDS = PAIR_IDS  # Same ordering

# CLOB constants
SIDE_BUY = 0
SIDE_SELL = 1
ORDER_LIMIT = 0
EXPIRY_SLOTS = 2_592_000  # ~30 days at 400ms/slot
SEED_COMPUTE_BUDGET = int(os.environ.get("LICHEN_DEX_SEED_COMPUTE_BUDGET", "1400000"))


async def with_retry(coro_fn, retries=3, delay=0.5):
    """Retry an async call with delay between attempts."""
    for attempt in range(retries):
        try:
            return await coro_fn()
        except Exception as e:
            if attempt < retries - 1:
                await asyncio.sleep(delay * (attempt + 1))
            else:
                raise e


def fetch_external_prices():
    """Fetch external prices with deterministic fallbacks."""
    defaults = {
        'SOL': 85.8,
        'ETH': 2119.179999,
        'BNB': 650.06,
        'NEO': 3.075,
        'GAS': 1.65,
    }
    symbols = ["SOLUSDT", "ETHUSDT", "BNBUSDT", "NEOUSDT", "GASUSDT"]
    urls = [
        'https://api.binance.us/api/v3/ticker/price?symbols=' + urllib.parse.quote(json.dumps(symbols)),
        'https://api.binance.com/api/v3/ticker/price?symbols=' + urllib.parse.quote(json.dumps(symbols)),
    ]
    sym_map = {
        'SOLUSDT': 'SOL',
        'ETHUSDT': 'ETH',
        'BNBUSDT': 'BNB',
        'NEOUSDT': 'NEO',
        'GASUSDT': 'GAS',
    }
    prices = {}
    for url in urls:
        try:
            req = urllib.request.Request(url, headers={'User-Agent': 'Lichen/1.0'})
            with urllib.request.urlopen(req, timeout=10) as resp:
                for item in json.loads(resp.read()):
                    if item['symbol'] in sym_map:
                        prices[sym_map[item['symbol']]] = float(item['price'])
            if len(prices) >= len(sym_map):
                break
        except Exception:
            continue
    for sym, fallback in defaults.items():
        prices[sym] = prices.get(sym, fallback)
    prices['LICN'] = 0.10
    return prices


async def fetch_chain_prices(conn):
    """Read the chain's active oracle prices so launch orders satisfy DEX bands."""
    try:
        result = await conn._rpc("getOraclePrices")
    except Exception:
        return {}

    mapping = {
        "LICN": "LICN",
        "wSOL": "SOL",
        "wETH": "ETH",
        "wBNB": "BNB",
        "wNEO": "NEO",
        "wGAS": "GAS",
    }
    prices = {}
    for chain_sym, local_sym in mapping.items():
        value = result.get(chain_sym)
        if isinstance(value, (int, float)) and value > 0:
            prices[local_sym] = float(value)
    return prices


def apply_price_overrides(prices):
    """Apply explicit operator overrides last."""
    prices = dict(prices)
    for sym in ("SOL", "ETH", "BNB", "NEO", "GAS"):
        env_key = f"LICHEN_{sym}_USD_PRICE"
        if env_key in os.environ:
            prices[sym] = float(os.environ[env_key])
    prices['LICN'] = float(os.environ.get('LICHEN_USD_PRICE', prices.get('LICN', 0.10)))
    return prices


def price_to_tick(p, tick_spacing=60):
    """Convert price to concentrated liquidity tick index, snapped to tick_spacing."""
    if p <= 0:
        return -443636
    raw = int(math.log(p) / math.log(1.0001))
    # Snap down to nearest multiple of tick_spacing
    return (raw // tick_spacing) * tick_spacing


TICK_RATIOS = [
    18447666410913237093,
    18448588794233782755,
    18450433699234678119,
    18454124062740255875,
    18461507004283223312,
    18476281749631266690,
    18505866722479494652,
    18565178862011796984,
    18684374044615830753,
    18925065152102488741,
    19415789018386924678,
    20435739862829184269,
    22639196493023416200,
    27784481413182840296,
    41848979110613408870,
    94940171859194227663,
    488630199271840203185,
    12943176892702717671113,
    9081593337360425506718466,
]


def mul_q64(a, b):
    """Match dex_amm mul_q64 for Q64.64 values."""
    mask64 = (1 << 64) - 1
    mask128 = (1 << 128) - 1
    a_hi, a_lo = a >> 64, a & mask64
    b_hi, b_lo = b >> 64, b & mask64
    ll = a_lo * b_lo
    hl = a_hi * b_lo
    lh = a_lo * b_hi
    hh = a_hi * b_hi
    ll_hi = ll >> 64
    sum1 = hl + lh
    carry = sum1 >> 128
    sum1 &= mask128
    sum2 = sum1 + ll_hi
    carry += sum2 >> 128
    sum2 &= mask128
    return (((hh + carry) & mask128) << 64) + sum2


def tick_to_sqrt_price(tick):
    """Mirror dex_amm tick_to_sqrt_price so launch math matches contract math."""
    abs_tick = -tick if tick < 0 else tick
    acc = 1 << 64
    for k, ratio in enumerate(TICK_RATIOS):
        if abs_tick & (1 << k):
            acc = mul_q64(acc, ratio)
    if tick < 0:
        acc = ((1 << 128) - 1) // acc + 1
    result = acc >> 32
    if result == 0:
        return 1
    return min(result, (1 << 64) - 1)


def compute_liquidity(amount_a, amount_b, sqrt_lower, sqrt_upper, sqrt_current):
    if sqrt_lower >= sqrt_upper or (amount_a == 0 and amount_b == 0):
        return 0
    if sqrt_current >= sqrt_upper:
        liq_a = 0
    else:
        sqrt_l = sqrt_current if sqrt_current > sqrt_lower else sqrt_lower
        delta = sqrt_upper - sqrt_l
        liq_a = amount_a * sqrt_l * sqrt_upper // (delta * (1 << 32)) if delta else 0
    if sqrt_current <= sqrt_lower:
        liq_b = 0
    else:
        sqrt_u = sqrt_current if sqrt_current < sqrt_upper else sqrt_upper
        delta = sqrt_u - sqrt_lower
        liq_b = amount_b * (1 << 32) // delta if delta else 0
    if liq_a == 0:
        return liq_b
    if liq_b == 0:
        return liq_a
    return min(liq_a, liq_b)


def compute_amounts_from_liquidity(liquidity, sqrt_lower, sqrt_upper, sqrt_current):
    if liquidity == 0 or sqrt_lower >= sqrt_upper:
        return 0, 0
    if sqrt_current >= sqrt_upper:
        amount_a = 0
    else:
        eff = sqrt_current if sqrt_current > sqrt_lower else sqrt_lower
        delta = sqrt_upper - eff
        denom = eff * sqrt_upper // (1 << 32)
        amount_a = liquidity * delta // denom if denom else 0
    if sqrt_current <= sqrt_lower:
        amount_b = 0
    else:
        eff = sqrt_current if sqrt_current < sqrt_upper else sqrt_upper
        delta = eff - sqrt_lower
        amount_b = liquidity * delta // (1 << 32)
    return amount_a, amount_b


def fetch_pool_sqrt_price(pool_id, fallback_price):
    """Read the live AMM pool sqrt price; fall back to the planned launch price."""
    try:
        url = f"{RPC.rstrip('/')}/api/v1/pools/{pool_id}"
        with urllib.request.urlopen(url, timeout=10) as resp:
            payload = json.loads(resp.read())
        data = payload.get("data") if isinstance(payload, dict) else None
        sqrt_price = data.get("sqrtPrice") if isinstance(data, dict) else None
        if isinstance(sqrt_price, int) and sqrt_price > 0:
            return sqrt_price
    except Exception:
        pass
    return int(math.sqrt(fallback_price) * (1 << 32))


def align_whole_token_a_amount(name, pool_id, current_price, lower_tick, upper_tick, amount_a, amount_b):
    """Adjust amount_b so AMM math pulls whole wNEO lots for wNEO-as-token-A pools."""
    if not name.startswith("wNEO/"):
        return amount_a, amount_b

    sqrt_current = fetch_pool_sqrt_price(pool_id, current_price)
    sqrt_lower = tick_to_sqrt_price(lower_tick)
    sqrt_upper = tick_to_sqrt_price(upper_tick)
    liquidity = compute_liquidity(amount_a, amount_b, sqrt_lower, sqrt_upper, sqrt_current)
    actual_a, _ = compute_amounts_from_liquidity(
        liquidity, sqrt_lower, sqrt_upper, sqrt_current)
    if actual_a == 0 or actual_a % SPORES == 0:
        return amount_a, amount_b

    target_units = actual_a // SPORES
    for units in range(target_units, max(target_units - 50, 0), -1):
        target = units * SPORES
        lo, hi = 0, amount_b
        candidate = None
        while lo <= hi:
            mid = (lo + hi) // 2
            liquidity = compute_liquidity(amount_a, mid, sqrt_lower, sqrt_upper, sqrt_current)
            candidate_a, candidate_b = compute_amounts_from_liquidity(
                liquidity, sqrt_lower, sqrt_upper, sqrt_current)
            if candidate_a >= target:
                if candidate_a == target and candidate_b > 0:
                    candidate = mid
                hi = mid - 1
            else:
                lo = mid + 1
        if candidate is not None:
            reduction = (amount_b - candidate) / SPORES
            quote_symbol = name.split("/", 1)[1] if "/" in name else "token-B"
            print(f"    {name}: adjusted {quote_symbol} side by -{reduction:.9f} so wNEO pull is {units:,} whole NEO")
            return amount_a, candidate

    raise RuntimeError(f"{name}: unable to align AMM wNEO pull to a whole-NEO amount")


# ── Contract call helpers ────────────────────────────────────────────────

async def approve_token(conn, caller, token_contract, spender_pubkey, amount):
    """Approve spender to transfer tokens on behalf of caller.
    Named export: approve(owner[32B], spender[32B], amount[u64])"""
    owner_bytes = bytes(caller.address().to_bytes())
    spender_bytes = bytes(spender_pubkey.to_bytes())
    args = list(owner_bytes + spender_bytes + struct.pack('<Q', amount))
    return await call_contract_raw(
        conn, caller, token_contract, 'approve', args,
        compute_budget=SEED_COMPUTE_BUDGET,
    )


async def place_order(conn, caller, dex_core, pair_id, side, price_spores, qty_spores,
                      is_base_native=False, is_quote_native=False, taker_fee_bps=5):
    """Place a limit order on CLOB. dex_core opcode 2.
    For native LICN escrow: sends value with the transaction."""
    caller_bytes = bytes(caller.address().to_bytes())
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

    # Calculate value to send for native LICN escrow
    value = 0
    if side == SIDE_SELL and is_base_native:
        # Selling native LICN: escrow = quantity
        value = qty_spores
    elif side == SIDE_BUY and is_quote_native:
        # Buying with native LICN: escrow = notional + taker_fee
        notional = price_spores * qty_spores // SPORES
        fee = max(notional * taker_fee_bps // 10_000, 1)
        value = notional + fee

    return await call_contract_raw(
        conn, caller, dex_core, 'call', list(args), value=value,
        compute_budget=SEED_COMPUTE_BUDGET,
    )


async def add_amm_liquidity(conn, caller, dex_amm, pool_id, lower_tick, upper_tick, amount_a, amount_b, value=0):
    """Add concentrated liquidity. dex_amm opcode 3.
    value: native LICN (spores) to send with the tx when one side is LICN."""
    caller_bytes = bytes(caller.address().to_bytes())
    deadline = 0
    args = (
        bytes([3])                                +  # opcode 3
        caller_bytes                              +  # provider (32B)
        struct.pack('<Q', pool_id)                +  # pool_id
        struct.pack('<i', lower_tick)             +  # lower_tick (i32)
        struct.pack('<i', upper_tick)             +  # upper_tick (i32)
        struct.pack('<Q', amount_a)               +  # amount_a
        struct.pack('<Q', amount_b)               +  # amount_b
        struct.pack('<Q', deadline)                  # deadline = 0 (no expiry)
    )
    return await call_contract_raw(
        conn, caller, dex_amm, 'call', list(args), value=value,
        compute_budget=SEED_COMPUTE_BUDGET,
    )


# ── Main ─────────────────────────────────────────────────────────────────

async def main():
    conn = Connection(RPC)
    repo = Path(__file__).resolve().parent.parent

    # Load reserve_pool keypair (protocol market maker)
    try:
        rp_path = find_genesis_keypair_path("reserve_pool", NETWORK)
    except FileNotFoundError as exc:
        print(f"ERROR: {exc}")
        sys.exit(1)
    reserve = Keypair.load(rp_path)
    print(f"  Market maker:  {reserve.address()}")

    # Discover contracts from symbol registry
    result = await conn._rpc("getAllSymbolRegistry")
    entries = result.get("entries", [])
    contracts = {}
    for e in entries:
        sym = e.get("symbol", "")
        prog = e.get("program", "")
        if not prog:
            continue
        if sym == "DEX":
            contracts["dex_core"] = PublicKey.from_base58(prog)
        elif sym == "DEXAMM":
            contracts["dex_amm"] = PublicKey.from_base58(prog)
        elif sym == "LUSD":
            contracts["lusd"] = PublicKey.from_base58(prog)
        elif sym == "WSOL":
            contracts["wsol"] = PublicKey.from_base58(prog)
        elif sym == "WETH":
            contracts["weth"] = PublicKey.from_base58(prog)
        elif sym == "WBNB":
            contracts["wbnb"] = PublicKey.from_base58(prog)
        elif sym == "WNEO":
            contracts["wneo"] = PublicKey.from_base58(prog)
        elif sym == "WGAS":
            contracts["wgas"] = PublicKey.from_base58(prog)

    dex_core = contracts.get("dex_core")
    dex_amm = contracts.get("dex_amm")
    if not dex_core or not dex_amm:
        print(f"  ERROR: Missing contracts — dex_core={dex_core}, dex_amm={dex_amm}")
        sys.exit(1)
    print(f"  dex_core:      {dex_core}")
    print(f"  dex_amm:       {dex_amm}")

    lusd = contracts.get("lusd")
    wsol = contracts.get("wsol")
    weth = contracts.get("weth")
    wbnb = contracts.get("wbnb")
    wneo = contracts.get("wneo")
    wgas = contracts.get("wgas")
    print(f"  lusd:          {lusd}")
    print(f"  wsol:          {wsol}")
    print(f"  weth:          {weth}")
    print(f"  wbnb:          {wbnb}")
    print(f"  wneo:          {wneo}")
    print(f"  wgas:          {wgas}")

    # ── Pre-approve DEX contracts to escrow tokens from reserve_pool ──
    MAX_APPROVE = 2**63 - 1  # max u64-safe approval amount
    print(f"\n  Approving DEX contracts to spend reserve_pool tokens...")
    for label, token in [
        ("lUSD", lusd),
        ("wSOL", wsol),
        ("wETH", weth),
        ("wBNB", wbnb),
        ("wNEO", wneo),
        ("wGAS", wgas),
    ]:
        if not token:
            print(f"    {label}: contract not found, skipping")
            continue
        approve_amount = MAX_APPROVE
        if label == "wNEO":
            approve_amount -= approve_amount % SPORES
        try:
            sig = await approve_token(conn, reserve, token, dex_core, approve_amount)
            print(f"    {label} → dex_core ✓  (sig: {sig[:16]}...)")
        except Exception as e:
            print(f"    {label} → dex_core FAILED: {e}")
        if dex_amm:
            try:
                sig = await approve_token(conn, reserve, token, dex_amm, approve_amount)
                print(f"    {label} → dex_amm  ✓  (sig: {sig[:16]}...)")
            except Exception as e:
                print(f"    {label} → dex_amm  FAILED: {e}")

    # Prefer chain oracle prices so seed orders match active DEX price bands.
    prices = fetch_external_prices()
    prices.update(await fetch_chain_prices(conn))
    prices = apply_price_overrides(prices)
    print(f"\n  Live prices:")
    for sym, usd in sorted(prices.items()):
        print(f"    {sym}/USD = ${usd:,.2f}")

    licn = prices['LICN']
    sol = prices['SOL']
    eth = prices['ETH']
    bnb = prices['BNB']
    neo = prices['NEO']
    gas = prices['GAS']

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
            sig = await with_retry(lambda p=p, q=q: place_order(
                conn, reserve, dex_core, pair_id, SIDE_SELL,
                int(p * SPORES), q * SPORES, is_base_native=True))
            print(f"    SELL {q:>10,} LICN @ ${p:.3f}  ✓")
            total_orders += 1
        except Exception as e:
            print(f"    SELL @ ${p:.3f}: {e}")
        await asyncio.sleep(0.2)

    for p, q in buy_levels:
        try:
            sig = await with_retry(lambda p=p, q=q: place_order(
                conn, reserve, dex_core, pair_id, SIDE_BUY,
                int(p * SPORES), q * SPORES))
            print(f"    BUY  {q:>10,} LICN @ ${p:.3f}  ✓")
            total_orders += 1
        except Exception as e:
            print(f"    BUY  @ ${p:.3f}: {e}")
        await asyncio.sleep(0.2)

    # ═════════════════════════════════════════════════════════════════════
    #  Phase 1b: CLOB Orders on Wrapped Token Pairs
    # ═════════════════════════════════════════════════════════════════════
    print(f"\n{'═' * 60}")
    print(f"  Phase 1b: Wrapped Token Pair CLOB Seeding")
    print(f"{'═' * 60}")

    # (pair_name, pair_id, base_price_in_quote, lot_size_tokens, num_levels, is_base_native, is_quote_native)
    wrapped_pairs = [
        ("wSOL/lUSD", 2, sol,       50,  10, False, False),
        ("wETH/lUSD", 3, eth,       5,   10, False, False),
        ("wBNB/lUSD", 6, bnb,       20,  10, False, False),
        ("wNEO/lUSD", 8, neo,       500, 10, False, False),
        ("wGAS/lUSD", 10, gas,      1000, 10, False, False),
        ("wSOL/LICN", 4, sol / licn, 50,  10, False, True),
        ("wETH/LICN", 5, eth / licn, 5,   10, False, True),
        ("wBNB/LICN", 7, bnb / licn, 20,  10, False, True),
        ("wNEO/LICN", 9, neo / licn, 500, 10, False, True),
        ("wGAS/LICN", 11, gas / licn, 1000, 10, False, True),
    ]

    for name, pid, base_price, lot, nlevels, base_native, quote_native in wrapped_pairs:
        pair_orders = 0
        pair_failures = {}
        spread_step = base_price * 0.01  # 1% per level
        for i in range(nlevels):
            sell_p = base_price + (i + 1) * spread_step
            buy_p = base_price - (i + 1) * spread_step
            if buy_p <= 0:
                continue
            qty = lot * SPORES
            try:
                await with_retry(lambda sp=sell_p, q=qty, pid=pid, bn=base_native, qn=quote_native: place_order(
                    conn, reserve, dex_core, pid, SIDE_SELL,
                    int(sp * SPORES), q,
                    is_base_native=bn, is_quote_native=qn))
                pair_orders += 1
            except Exception as e:
                key = str(e).splitlines()[0]
                pair_failures[key] = pair_failures.get(key, 0) + 1
            await asyncio.sleep(0.15)
            try:
                await with_retry(lambda bp=buy_p, q=qty, pid=pid, bn=base_native, qn=quote_native: place_order(
                    conn, reserve, dex_core, pid, SIDE_BUY,
                    int(bp * SPORES), q,
                    is_base_native=bn, is_quote_native=qn))
                pair_orders += 1
            except Exception as e:
                key = str(e).splitlines()[0]
                pair_failures[key] = pair_failures.get(key, 0) + 1
            await asyncio.sleep(0.15)
        total_orders += pair_orders
        print(f"    {name}: {pair_orders} orders placed")
        for reason, count in sorted(pair_failures.items()):
            print(f"      {count} failed: {reason}")

    print(f"\n  Total CLOB orders: {total_orders}")

    # ═════════════════════════════════════════════════════════════════════
    #  Phase 2: AMM Concentrated Liquidity
    # ═════════════════════════════════════════════════════════════════════
    print(f"\n{'═' * 60}")
    print(f"  Phase 2: AMM Concentrated Liquidity Seeding")
    print(f"{'═' * 60}")

    # (name, pool_id, current_price, range_low, range_high, amount_a_tokens, amount_b_tokens, licn_side)
    # licn_side: "a" if token_a is native LICN, "b" if token_b is, None if neither
    amm_pools = [
        ("LICN/lUSD", 1, licn,      licn * 0.5,      licn * 2.5,      5_000_000, 500_000,  "a"),
        ("wSOL/lUSD", 2, sol,        sol * 0.7,        sol * 1.4,       500,       50_000,   None),
        ("wETH/lUSD", 3, eth,        eth * 0.7,        eth * 1.4,       25,        50_000,   None),
        ("wSOL/LICN", 4, sol / licn, sol / licn * 0.6, sol / licn * 1.5, 500,      500_000,  "b"),
        ("wETH/LICN", 5, eth / licn, eth / licn * 0.6, eth / licn * 1.5, 25,       500_000,  "b"),
        ("wBNB/lUSD", 6, bnb,        bnb * 0.7,        bnb * 1.4,       100,       50_000,   None),
        ("wBNB/LICN", 7, bnb / licn, bnb / licn * 0.6, bnb / licn * 1.5, 100,      500_000,  "b"),
        ("wNEO/lUSD", 8, neo,        neo * 0.7,        neo * 1.4,       2_500,     8_000,    None),
        ("wNEO/LICN", 9, neo / licn, neo / licn * 0.6, neo / licn * 1.5, 2_500,    75_000,   "b"),
        ("wGAS/lUSD", 10, gas,       gas * 0.7,        gas * 1.4,       10_000,    16_500,   None),
        ("wGAS/LICN", 11, gas / licn, gas / licn * 0.6, gas / licn * 1.5, 10_000,  165_000,  "b"),
    ]

    pools_seeded = 0
    for name, pid, price, low, high, amt_a, amt_b, licn_side in amm_pools:
        lt = price_to_tick(low)  # snaps down
        ut = price_to_tick(high)
        # Snap upper tick UP to ensure range covers the high price
        raw_ut = int(math.log(high) / math.log(1.0001))
        if raw_ut % 60 != 0:
            ut = ((raw_ut // 60) + 1) * 60
        a_spores = amt_a * SPORES
        b_spores = amt_b * SPORES
        a_spores, b_spores = align_whole_token_a_amount(
            name, pid, price, lt, ut, a_spores, b_spores)
        # Native LICN must be sent as tx value, not via cross-contract transfer
        value = a_spores if licn_side == "a" else b_spores if licn_side == "b" else 0
        try:
            sig = await with_retry(lambda p=pid, l=lt, u=ut, a=a_spores, b=b_spores, v=value:
                add_amm_liquidity(conn, reserve, dex_amm, p, l, u, a, b, value=v))
            print(f"    ✅ {name}: {amt_a:>12,} / {amt_b:>12,}  ticks=[{lt}, {ut}]")
            pools_seeded += 1
        except Exception as e:
            print(f"    ❌ {name}: {e}")
        await asyncio.sleep(0.5)

    expected_pools = len(amm_pools)
    print(f"\n  {pools_seeded}/{expected_pools} AMM pools seeded")

    # ── Summary ──
    print(f"\n{'═' * 60}")
    print(f"  DEX Liquidity Seeding Complete")
    print(f"{'═' * 60}")
    print(f"  CLOB orders placed:  {total_orders}")
    print(f"  AMM pools seeded:    {pools_seeded}/{expected_pools}")
    print(f"  Market maker wallet: {reserve.address()}")


asyncio.run(main())
