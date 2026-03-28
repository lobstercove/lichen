#!/usr/bin/env python3
"""Retry LICN/lUSD AMM pool with smaller amount"""
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
    # Discover dex_amm from registry
    result = await conn._rpc("getAllSymbolRegistry")
    dex_amm_addr = None
    for e in result.get("entries", []):
        if e.get("symbol") == "DEXAMM":
            dex_amm_addr = e.get("program")
            break
    if not dex_amm_addr:
        print("ERROR: DEXAMM not found in registry")
        return
    dex_amm = PublicKey.from_base58(dex_amm_addr)

    licn = 0.10
    lt = price_to_tick(licn * 0.5)
    ut = price_to_tick(licn * 2.5)

    # Smaller initial: 1M LICN + 100K lUSD
    a = 1_000_000 * SPORES
    b = 100_000 * SPORES

    caller_bytes = bytes(reserve.public_key().to_bytes())
    args = (
        bytes([3]) + caller_bytes +
        struct.pack('<Q', 1) +
        struct.pack('<i', lt) +
        struct.pack('<i', ut) +
        struct.pack('<Q', a) +
        struct.pack('<Q', b)
    )
    print(f'Adding LICN/lUSD AMM: 1M LICN + 100K lUSD, ticks=[{lt},{ut}]')
    try:
        sig = await call_contract_raw(conn, reserve, dex_amm, 'call', list(args))
        print(f'Success! sig: {sig}')
    except Exception as e:
        print(f'Failed: {e}')
        print('Trying even smaller: 100K LICN + 10K lUSD')
        a2 = 100_000 * SPORES
        b2 = 10_000 * SPORES
        args2 = (
            bytes([3]) + caller_bytes +
            struct.pack('<Q', 1) +
            struct.pack('<i', lt) +
            struct.pack('<i', ut) +
            struct.pack('<Q', a2) +
            struct.pack('<Q', b2)
        )
        try:
            sig = await call_contract_raw(conn, reserve, dex_amm, 'call', list(args2))
            print(f'Success with smaller amount! sig: {sig}')
        except Exception as e2:
            print(f'Also failed: {e2}')

asyncio.run(main())
