#!/usr/bin/env python3
"""Seed AMM pools using the deploy manifest with real-time prices from Binance."""
import asyncio, sys, os, json, struct, math, urllib.request

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'tools'))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))

from deploy_dex import load_or_create_deployer, call_contract_raw
from moltchain import Connection, PublicKey

RPC = os.environ.get('CUSTODY_MOLT_RPC_URL', 'http://127.0.0.1:8899')
MANIFEST = os.path.join(os.path.dirname(__file__), '..', 'deploy-manifest.json')

# ── Price helpers ────────────────────────────────────────────────────────
# sqrt_price is Q32.32 fixed-point: sqrt_price = floor(sqrt(price) * 2^32)
Q32 = 1 << 32  # 4_294_967_296

def price_to_sqrt_price(price: float) -> int:
    """Convert a real price (token_a per token_b) to Q32.32 sqrt_price."""
    return int(math.sqrt(price) * Q32)

def fetch_binance_prices() -> dict:
    """Fetch current USD prices from Binance (or Binance US).
    Returns dict like {'SOL': 145.50, 'ETH': 2650.0, 'BNB': 620.0, 'MOLT': 0.10}
    """
    # Try Binance US first (geo-unblocked), fall back to global
    binance_urls = [
        'https://api.binance.us/api/v3/ticker/price?symbols=["SOLUSDT","ETHUSDT","BNBUSDT"]',
        'https://api.binance.com/api/v3/ticker/price?symbols=["SOLUSDT","ETHUSDT","BNBUSDT"]',
    ]

    symbol_map = {'SOLUSDT': 'SOL', 'ETHUSDT': 'ETH', 'BNBUSDT': 'BNB'}
    prices = {}

    for url in binance_urls:
        try:
            req = urllib.request.Request(url, headers={'User-Agent': 'MoltChain/1.0'})
            with urllib.request.urlopen(req, timeout=10) as resp:
                data = json.loads(resp.read())
                for item in data:
                    sym = symbol_map.get(item['symbol'])
                    if sym:
                        prices[sym] = float(item['price'])
            if len(prices) >= 3:
                break
        except Exception as e:
            print(f'  Warning: Binance API ({url[:40]}...): {e}')
            continue

    if not prices:
        print('  ERROR: Could not fetch any prices from Binance, using fallback')
        prices = {'SOL': 145.0, 'ETH': 2600.0, 'BNB': 620.0}

    # MOLT doesn't trade on Binance — use env var or default
    prices['MOLT'] = float(os.environ.get('MOLT_USD_PRICE', '0.10'))

    return prices


async def seed_pools():
    conn = Connection(RPC)
    deployer = load_or_create_deployer()
    manifest = json.load(open(MANIFEST))
    addrs = manifest.get('contracts', {})

    amm_str = addrs.get('dex_amm')
    if not amm_str:
        print('dex_amm not in manifest, skipping pool seeding')
        return

    amm_pk = PublicKey.from_base58(amm_str)
    deployer_bytes = bytes(deployer.public_key().to_bytes())

    token_names = {
        'MOLT': 'moltcoin',
        'mUSD': 'musd_token',
        'wSOL': 'wsol_token',
        'wETH': 'weth_token',
        'wBNB': 'wbnb_token',
    }

    token_pks = {}
    for sym, contract_name in token_names.items():
        addr_str = addrs.get(contract_name)
        if addr_str:
            token_pks[sym] = bytes(PublicKey.from_base58(addr_str).to_bytes())
        else:
            print(f'  Warning: {contract_name} not in manifest, skipping pools with {sym}')

    # ── Fetch real prices ────────────────────────────────────────────
    print('  Fetching real-time prices from Binance...')
    prices = fetch_binance_prices()
    for sym, usd in sorted(prices.items()):
        print(f'    {sym}/USD = ${usd:,.2f}')

    # ── Build pool list with live sqrt_prices ────────────────────────
    # Price = how many of token_b you get per 1 token_a
    # For X/mUSD pools: price = USD price of X (since mUSD ≈ $1)
    # For X/MOLT pools: price = USD price of X / USD price of MOLT
    molt_usd = prices['MOLT']

    pools = [
        # token_a / token_b pools (price = token_a denominated in token_b)
        ('MOLT', 'mUSD', 30, price_to_sqrt_price(molt_usd)),
        ('wSOL', 'mUSD', 30, price_to_sqrt_price(prices['SOL'])),
        ('wETH', 'mUSD', 30, price_to_sqrt_price(prices['ETH'])),
        ('wBNB', 'mUSD', 30, price_to_sqrt_price(prices['BNB'])),
        ('wSOL', 'MOLT', 30, price_to_sqrt_price(prices['SOL'] / molt_usd)),
        ('wETH', 'MOLT', 30, price_to_sqrt_price(prices['ETH'] / molt_usd)),
        ('wBNB', 'MOLT', 30, price_to_sqrt_price(prices['BNB'] / molt_usd)),
    ]

    for (sym_a, sym_b, fee_tier, sqrt_p) in pools:
        real_price = (sqrt_p / Q32) ** 2
        print(f'    {sym_a}/{sym_b}: sqrt_price={sqrt_p:,} (price={real_price:,.4f})')

    created = 0
    for (sym_a, sym_b, fee_tier, sqrt_price) in pools:
        if sym_a not in token_pks or sym_b not in token_pks:
            print(f'  Skipping pool {sym_a}/{sym_b}: token not deployed')
            continue
        data = (bytes([1]) + deployer_bytes + token_pks[sym_a] +
                token_pks[sym_b] + bytes([fee_tier]) + struct.pack('<Q', sqrt_price))
        try:
            sig = await call_contract_raw(conn, deployer, amm_pk, 'call', list(data))
            print(f'  Pool {sym_a}/{sym_b} created (fee={fee_tier}bps) sig={sig[:16]}...')
            created += 1
        except Exception as e:
            print(f'  Pool {sym_a}/{sym_b}: {e}')

    print(f'\n{created}/{len(pools)} pools seeded successfully')

asyncio.run(seed_pools())
