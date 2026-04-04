#!/usr/bin/env python3
"""Fund genesis-primary and reserve_pool with LICN for tx fees."""
import sys, os, asyncio
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
sys.path.insert(0, os.path.dirname(__file__))
from lichen import Connection
from deploy_dex import load_genesis_keypair

SPORES = 1_000_000_000
RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')
NETWORK = os.environ.get('LICHEN_NETWORK', 'testnet')


def extract_spores(balance):
    if isinstance(balance, dict):
        return balance.get('spendable', balance.get('spores', 0))
    return balance or 0


async def wait_for_balance(conn, address, minimum_spores, label, attempts=80, delay=0.25):
    for _ in range(attempts):
        spores = extract_spores(await conn._rpc('getBalance', [str(address)]))
        if spores >= minimum_spores:
            return spores
        await asyncio.sleep(delay)

    raise RuntimeError(
        f"Timed out waiting for {label} balance to reach {minimum_spores} spores"
    )

async def main():
    conn = Connection(RPC)
    treasury = load_genesis_keypair('treasury', NETWORK)
    admin = load_genesis_keypair('genesis-primary', NETWORK)
    reserve = load_genesis_keypair('reserve_pool', NETWORK)

    print(f"Treasury: {treasury.address()}")
    print(f"Admin:    {admin.address()}")
    print(f"Reserve:  {reserve.address()}")

    # Check treasury balance
    bal_spores = extract_spores(await conn._rpc('getBalance', [str(treasury.address())]))
    admin_before = extract_spores(await conn._rpc('getBalance', [str(admin.address())]))
    reserve_before = extract_spores(await conn._rpc('getBalance', [str(reserve.address())]))
    print(f"Treasury balance: {bal_spores} spores ({bal_spores / SPORES:.3f} LICN)")

    # Fund admin with 10 LICN
    sig = await conn.transfer(treasury, admin.address(), 10 * SPORES)
    print(f"Funded admin with 10 LICN: {sig[:20]}...")

    # Fund reserve_pool with 100 LICN (needs fees for approve + order txs)
    sig2 = await conn.transfer(treasury, reserve.address(), 100 * SPORES)
    print(f"Funded reserve with 100 LICN: {sig2[:20]}...")

    # Verify after the transfers are reflected in state.
    a_spores = await wait_for_balance(conn, admin.address(), admin_before + (10 * SPORES), 'admin')
    r_spores = await wait_for_balance(conn, reserve.address(), reserve_before + (100 * SPORES), 'reserve_pool')
    print(f"\nAdmin balance:   {a_spores} spores ({a_spores / SPORES:.3f} LICN)")
    print(f"Reserve balance: {r_spores} spores ({r_spores / SPORES:.3f} LICN)")

asyncio.run(main())
