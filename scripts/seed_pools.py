#!/usr/bin/env python3
"""Seed AMM pools using the deploy manifest."""
import asyncio, sys, os, json, struct

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'tools'))
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))

from deploy_dex import load_or_create_deployer, call_contract_raw
from moltchain import Connection, PublicKey

RPC = os.environ.get('CUSTODY_MOLT_RPC_URL', 'http://127.0.0.1:8899')
MANIFEST = os.path.join(os.path.dirname(__file__), '..', 'deploy-manifest.json')

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
    }

    token_pks = {}
    for sym, contract_name in token_names.items():
        addr_str = addrs.get(contract_name)
        if addr_str:
            token_pks[sym] = bytes(PublicKey.from_base58(addr_str).to_bytes())
        else:
            print(f'  Warning: {contract_name} not in manifest, skipping pools with {sym}')

    pools = [
        ('MOLT', 'mUSD', 30, 1_358_187_913),
        ('wSOL', 'mUSD', 30, 55_999_522_252),
        ('wETH', 'mUSD', 30, 214_748_364_800),
        ('wSOL', 'MOLT', 30, 177_086_038_199),
        ('wETH', 'MOLT', 30, 679_093_956_564),
    ]

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
