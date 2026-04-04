#!/usr/bin/env python3
"""Mint WBNB using dynamic address from symbol registry."""
import asyncio, sys, os, struct, base64
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
sys.path.insert(0, os.path.dirname(__file__))
os.chdir(os.path.join(os.path.dirname(__file__), '..'))
from pathlib import Path
from lichen import Connection, Keypair, PublicKey
from deploy_dex import call_contract_raw, load_genesis_keypair

SPORES = 1_000_000_000

async def main():
    conn = Connection('http://127.0.0.1:8899')
    admin = load_genesis_keypair('genesis-primary')
    reserve = load_genesis_keypair('reserve_pool')
    admin_bytes = bytes(admin.address().to_bytes())
    reserve_bytes = bytes(reserve.address().to_bytes())

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

    print(f"Minting 5,000 WBNB to {reserve.address()}...")
    sig = await call_contract_raw(conn, admin, wbnb, 'mint', args)
    print(f"Mint tx: {sig}")

    await asyncio.sleep(3)

    args_b64 = base64.b64encode(reserve_bytes).decode()
    r2 = await conn._rpc('callContract', [wbnb_addr, 'balance_of', args_b64, str(reserve.address())])
    data = base64.b64decode(r2.get('result', ''))
    bal = struct.unpack('<Q', data[:8])[0] if len(data) >= 8 else 0
    print(f"WBNB balance: {bal / SPORES:,.3f}")

asyncio.run(main())
