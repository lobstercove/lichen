#!/usr/bin/env python3
"""Retry the two orders that failed due to server disconnect:
1. SELL 700,000 LICN @ $0.100 on pair 1
2. LICN/lUSD AMM pool
"""
import sys, os, struct, asyncio, math
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, Keypair, PublicKey

sys.path.insert(0, os.path.dirname(__file__))
from deploy_dex import call_contract_raw

SPORES = 1_000_000_000

def price_to_tick(p):
    if p <= 0:
        return -443636
    return int(math.log(p) / math.log(1.0001))

async def main():
    conn = Connection('http://127.0.0.1:8899')
    keys = Path('data/state-testnet/genesis-keys')
    reserve = Keypair.load(keys / 'reserve_pool-lichen-testnet-1.json')
    caller_bytes = bytes(reserve.public_key().to_bytes())

    result = await conn._rpc("getAllSymbolRegistry")
    dex_core_addr = dex_amm_addr = None
    for e in result.get("entries", []):
        if e.get("symbol") == "DEX":
            dex_core_addr = e.get("program")
        elif e.get("symbol") == "DEXAMM":
            dex_amm_addr = e.get("program")
    dex_core = PublicKey.from_base58(dex_core_addr)
    dex_amm = PublicKey.from_base58(dex_amm_addr)

    # 1. Retry SELL 700K LICN @ $0.100 on pair 1
    print("Retrying SELL 700,000 LICN @ $0.100 on pair 1...")
    price_spores = int(0.100 * SPORES)
    qty_spores = 700_000 * SPORES
    args = (
        bytes([2]) + caller_bytes +
        struct.pack('<Q', 1) +          # pair_id=1
        bytes([1]) +                     # SIDE_SELL
        bytes([0]) +                     # ORDER_LIMIT
        struct.pack('<Q', price_spores) +
        struct.pack('<Q', qty_spores) +
        struct.pack('<Q', 2_592_000)     # expiry
    )
    try:
        sig = await call_contract_raw(conn, reserve, dex_core, 'call', list(args), value=qty_spores)
        print(f"  SELL @ $0.100 ✓ (sig: {sig[:16]}...)")
    except Exception as e:
        print(f"  SELL @ $0.100 FAILED: {e}")

    # 2. Retry LICN/lUSD AMM pool (smaller: 1M LICN + 100K lUSD)
    print("Retrying LICN/lUSD AMM pool (1M LICN + 100K lUSD)...")
    licn = 0.10
    lt = price_to_tick(licn * 0.5)
    ut = price_to_tick(licn * 2.5)
    a = 1_000_000 * SPORES
    b = 100_000 * SPORES
    args2 = (
        bytes([3]) + caller_bytes +
        struct.pack('<Q', 1) +
        struct.pack('<i', lt) +
        struct.pack('<i', ut) +
        struct.pack('<Q', a) +
        struct.pack('<Q', b)
    )
    try:
        sig2 = await call_contract_raw(conn, reserve, dex_amm, 'call', list(args2))
        print(f"  AMM pool ✓ (sig: {sig2[:16]}...)")
    except Exception as e:
        print(f"  AMM pool FAILED: {e}")

asyncio.run(main())
