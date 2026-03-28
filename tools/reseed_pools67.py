#!/usr/bin/env python3
"""Re-seed AMM pools 6 and 7 (wBNB/lUSD and wBNB/LICN) after WBNB mint."""
import asyncio, sys, os, struct, math
from pathlib import Path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
sys.path.insert(0, os.path.dirname(__file__))
os.chdir(os.path.join(os.path.dirname(__file__), '..'))

from lichen import Connection, Keypair, PublicKey
from deploy_dex import call_contract_raw

SPORES = 1_000_000_000

def price_to_tick(p, tick_spacing=60):
    raw = int(math.log(p) / math.log(1.0001))
    return (raw // tick_spacing) * tick_spacing

async def approve_token(conn, caller, token_contract, spender_pubkey, amount):
    owner_bytes = bytes(caller.public_key().to_bytes())
    spender_bytes = bytes(spender_pubkey.to_bytes())
    args = list(owner_bytes + spender_bytes + struct.pack('<Q', amount))
    return await call_contract_raw(conn, caller, token_contract, 'approve', args)

async def add_amm_liquidity(conn, caller, dex_amm, pool_id, lower_tick, upper_tick, amount_a, amount_b, value=0):
    caller_bytes = bytes(caller.public_key().to_bytes())
    args = (
        bytes([3]) +
        caller_bytes +
        struct.pack('<Q', pool_id) +
        struct.pack('<i', lower_tick) +
        struct.pack('<i', upper_tick) +
        struct.pack('<Q', amount_a) +
        struct.pack('<Q', amount_b)
    )
    return await call_contract_raw(conn, caller, dex_amm, 'call', list(args), value=value)

async def main():
    conn = Connection('http://127.0.0.1:8899')
    reserve = Keypair.load(Path('data/state-testnet/genesis-keys/reserve_pool-lichen-testnet-1.json'))
    print(f"Reserve: {reserve.public_key()}")

    r = await conn._rpc('getAllSymbolRegistry')
    contracts = {}
    for e in r.get('entries', []):
        sym = e.get('symbol', '')
        prog = e.get('program', '')
        if sym == 'DEXAMM':
            contracts['dex_amm'] = PublicKey.from_base58(prog)
        elif sym == 'DEXCORE' or sym == 'DEX':
            contracts['dex_core'] = PublicKey.from_base58(prog)
        elif sym == 'WBNB':
            contracts['wbnb'] = PublicKey.from_base58(prog)

    dex_amm = contracts['dex_amm']
    dex_core = contracts['dex_core']
    wbnb = contracts['wbnb']
    print(f"dex_amm: {dex_amm}")
    print(f"wbnb: {wbnb}")

    # Approve WBNB for dex_amm and dex_core
    MAX_APPROVE = 2**63 - 1
    print("\nApproving WBNB...")
    sig = await approve_token(conn, reserve, wbnb, dex_core, MAX_APPROVE)
    print(f"  wBNB -> dex_core: {sig[:16]}...")
    sig = await approve_token(conn, reserve, wbnb, dex_amm, MAX_APPROVE)
    print(f"  wBNB -> dex_amm: {sig[:16]}...")

    await asyncio.sleep(2)

    # Pool 6: wBNB/lUSD
    bnb = 617.08
    licn = 0.10

    # Pool 6
    low6 = bnb * 0.7
    high6 = bnb * 1.4
    lt6 = price_to_tick(low6)
    raw_ut6 = int(math.log(high6) / math.log(1.0001))
    ut6 = ((raw_ut6 // 60) + 1) * 60 if raw_ut6 % 60 != 0 else raw_ut6
    a6 = 100 * SPORES
    b6 = 50_000 * SPORES
    print(f"\nPool 6 (wBNB/lUSD): ticks=[{lt6}, {ut6}] amounts={100}/{50_000}")
    sig = await add_amm_liquidity(conn, reserve, dex_amm, 6, lt6, ut6, a6, b6, value=0)
    print(f"  sig: {sig[:16]}...")

    await asyncio.sleep(1)

    # Pool 7: wBNB/LICN
    price7 = bnb / licn
    low7 = price7 * 0.6
    high7 = price7 * 1.5
    lt7 = price_to_tick(low7)
    raw_ut7 = int(math.log(high7) / math.log(1.0001))
    ut7 = ((raw_ut7 // 60) + 1) * 60 if raw_ut7 % 60 != 0 else raw_ut7
    a7 = 100 * SPORES
    b7 = 500_000 * SPORES
    print(f"\nPool 7 (wBNB/LICN): ticks=[{lt7}, {ut7}] amounts={100}/{500_000}")
    sig = await add_amm_liquidity(conn, reserve, dex_amm, 7, lt7, ut7, a7, b7, value=b7)
    print(f"  sig: {sig[:16]}...")

    await asyncio.sleep(3)
    print("\nDone. Run check_amm_pools.py to verify.")

asyncio.run(main())
