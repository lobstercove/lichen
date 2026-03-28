#!/usr/bin/env python3
"""Mint WBNB using dynamic address from symbol registry."""
import asyncio, sys, os, struct, base64
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
sys.path.insert(0, os.path.dirname(__file__))
os.chdir(os.path.join(os.path.dirname(__file__), '..'))
from pathlib import Path
from lichen import Connection, Keypair, PublicKey
from deploy_dex import call_contract_raw

SPORES = 1_000_000_000

async def main():
    conn = Connection('http://127.0.0.1:8899')
    admin = Keypair.load(Path('data/state-testnet/genesis-keys/genesis-primary-lichen-testnet-1.json'))
    reserve = Keypair.load(Path('data/state-testnet/genesis-keys/reserve_pool-lichen-testnet-1.json'))
    admin_bytes = bytes(admin.public_key().to_bytes())
    reserve_bytes = bytes(reserve.public_key().to_bytes())

    # Get WBNB address from symbol registry
    r = await conn._rpc('getAllSymbolRegistry')
    wbnb_addr = None
    for e in r.get('entries', []):
        if e.get('symbol') == 'WBNB':
            wbnb_addr = e['program']
            break
    if not wbnb_addr:
        print("ERROR: WBNB not found in registry")
        return
    print(f"WBNB contract: {wbnb_addr}")
    wbnb = PublicKey.from_base58(wbnb_addr)

    amount = 5_000 * SPORES
    args = list(admin_bytes) + list(reserve_bytes) + list(struct.pack('<Q', amount))

    print(f"Minting 5,000 WBNB to {reserve.public_key()}...")
    sig = await call_contract_raw(conn, admin, wbnb, 'mint', args)
    print(f"Mint tx: {sig}")

    await asyncio.sleep(3)

    args_b64 = base64.b64encode(reserve_bytes).decode()
    r2 = await conn._rpc('callContract', [wbnb_addr, 'balance_of', args_b64, str(reserve.public_key())])
    data = base64.b64decode(r2.get('result', ''))
    bal = struct.unpack('<Q', data[:8])[0] if len(data) >= 8 else 0
    print(f"WBNB balance: {bal / SPORES:,.3f}")

asyncio.run(main())
