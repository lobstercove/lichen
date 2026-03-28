#!/usr/bin/env python3
"""Fund genesis-primary and reserve_pool with LICN for tx fees."""
import sys, os, asyncio
from pathlib import Path
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, Keypair

SPORES = 1_000_000_000
RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')
NETWORK = os.environ.get('LICHEN_NETWORK', 'testnet')

async def main():
    conn = Connection(RPC)
    keys_dir = Path(__file__).resolve().parent.parent / f'data/state-{NETWORK}/genesis-keys'

    treasury = Keypair.load(keys_dir / f'treasury-lichen-{NETWORK}-1.json')
    admin = Keypair.load(keys_dir / f'genesis-primary-lichen-{NETWORK}-1.json')
    reserve = Keypair.load(keys_dir / f'reserve_pool-lichen-{NETWORK}-1.json')

    print(f"Treasury: {treasury.public_key()}")
    print(f"Admin:    {admin.public_key()}")
    print(f"Reserve:  {reserve.public_key()}")

    # Check treasury balance
    bal = await conn._rpc('getBalance', [str(treasury.public_key())])
    print(f"Treasury balance: {bal} spores")

    # Fund admin with 10 LICN
    sig = await conn.transfer(treasury, admin.public_key(), 10 * SPORES)
    print(f"Funded admin with 10 LICN: {sig[:20]}...")

    # Fund reserve_pool with 100 LICN (needs fees for approve + order txs)
    sig2 = await conn.transfer(treasury, reserve.public_key(), 100 * SPORES)
    print(f"Funded reserve with 100 LICN: {sig2[:20]}...")

    # Verify
    admin_bal = await conn._rpc('getBalance', [str(admin.public_key())])
    reserve_bal = await conn._rpc('getBalance', [str(reserve.public_key())])
    a_spores = admin_bal.get('spores', admin_bal) if isinstance(admin_bal, dict) else admin_bal
    r_spores = reserve_bal.get('spores', reserve_bal) if isinstance(reserve_bal, dict) else reserve_bal
    print(f"\nAdmin balance:   {a_spores} spores ({a_spores / SPORES:.3f} LICN)")
    print(f"Reserve balance: {r_spores} spores ({r_spores / SPORES:.3f} LICN)")

asyncio.run(main())
