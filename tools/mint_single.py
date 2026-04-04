#!/usr/bin/env python3
"""Mint a specific token to reserve_pool"""
import sys, os, struct, asyncio, base64
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, Keypair, PublicKey

sys.path.insert(0, os.path.dirname(__file__))
from deploy_dex import call_contract_raw, load_genesis_keypair

SPORES = 1_000_000_000

async def main():
    symbol = sys.argv[1] if len(sys.argv) > 1 else "WETH"
    amount = int(sys.argv[2]) if len(sys.argv) > 2 else 500

    conn = Connection('http://127.0.0.1:8899')
    admin = load_genesis_keypair('genesis-primary')
    reserve = load_genesis_keypair('reserve_pool')
    admin_bytes = bytes(admin.address().to_bytes())
    reserve_bytes = bytes(reserve.address().to_bytes())

    result = await conn._rpc("getAllSymbolRegistry")
    token_addr = None
    for e in result.get("entries", []):
        if e.get("symbol") == symbol:
            token_addr = e.get("program")
            break

    if not token_addr:
        print(f"Contract for {symbol} not found")
        return

    token_pk = PublicKey.from_base58(token_addr)
    args = list(admin_bytes) + list(reserve_bytes) + list(struct.pack('<Q', amount * SPORES))

    print(f"Minting {amount:,} {symbol} to reserve_pool...")
    try:
        sig = await call_contract_raw(conn, admin, token_pk, 'mint', args)
        print(f"Mint tx: {sig}")
    except Exception as e:
        print(f"Failed: {e}")
        return

    await asyncio.sleep(2)
    args_b64 = base64.b64encode(reserve_bytes).decode()
    r = await conn._rpc('callContract', [token_addr, 'balance_of', args_b64, str(reserve.address())])
    rc = r.get('returnCode', 0)
    print(f"{symbol} balance: {rc / SPORES:,.3f} tokens")

asyncio.run(main())
