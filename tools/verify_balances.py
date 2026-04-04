#!/usr/bin/env python3
"""Verify all token balances on reserve_pool"""
import asyncio, sys, os, base64
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection, PublicKey
from deploy_dex import load_genesis_keypair

SPORES = 1_000_000_000

async def main():
    conn = Connection('http://127.0.0.1:8899')
    reserve = load_genesis_keypair('reserve_pool')
    reserve_bytes = bytes(reserve.address().to_bytes())
    reserve_b58 = str(reserve.address())

    r = await conn._rpc('getBalance', [reserve_b58])
    licn = r.get('balance', 0) / SPORES
    print(f'Reserve: {reserve_b58}')
    print(f'  LICN:  {licn:>15,.3f}')

    reg = await conn._rpc('getAllSymbolRegistry')
    for sym in ['LUSD', 'WSOL', 'WETH', 'WBNB']:
        for e in reg.get('entries', []):
            if e.get('symbol') == sym:
                addr = e['program']
                args_b64 = base64.b64encode(reserve_bytes).decode()
                r = await conn._rpc('callContract', [addr, 'balance_of', args_b64, reserve_b58])
                bal = r.get('returnCode', 0) / SPORES
                print(f'  {sym}:  {bal:>15,.3f}')
                break

asyncio.run(main())
