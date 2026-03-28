#!/usr/bin/env python3
"""Mint WBNB tokens to reserve_pool"""
import sys, os, struct, asyncio, base64
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, Keypair, PublicKey

sys.path.insert(0, os.path.dirname(__file__))
from deploy_dex import call_contract_raw

SPORES = 1_000_000_000

async def main():
    conn = Connection('http://127.0.0.1:8899')
    keys = Path('data/state-testnet/genesis-keys')
    admin = Keypair.load(keys / 'genesis-primary-lichen-testnet-1.json')
    reserve = Keypair.load(keys / 'reserve_pool-lichen-testnet-1.json')
    admin_bytes = bytes(admin.public_key().to_bytes())
    reserve_bytes = bytes(reserve.public_key().to_bytes())

    wbnb_addr = 'CXbUDJPqjyo3T6pmGAUB3FhS1CpZQVmbST91JHpQ4pTX'
    wbnb_pk = PublicKey.from_base58(wbnb_addr)
    amount = 5_000 * SPORES

    args = list(admin_bytes) + list(reserve_bytes) + list(struct.pack('<Q', amount))

    print(f"Minting 5,000 WBNB to reserve_pool...")
    try:
        sig = await call_contract_raw(conn, admin, wbnb_pk, 'mint', args)
        print(f"WBNB mint tx: {sig}")
    except Exception as e:
        print(f"WBNB mint failed: {e}")
        return

    await asyncio.sleep(3)

    # Check balance
    args_b64 = base64.b64encode(reserve_bytes).decode()
    r = await conn._rpc('callContract', [wbnb_addr, 'balance_of', args_b64, str(reserve.public_key())])
    rc = r.get('returnCode', 0)
    print(f"WBNB balance: {rc / SPORES:,.3f} tokens")

asyncio.run(main())
