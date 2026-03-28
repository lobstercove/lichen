#!/usr/bin/env python3
"""Scan ALL blocks for AMM-related transactions and show details."""
import asyncio, sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection

RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')

async def main():
    conn = Connection(RPC)
    slot = await conn._rpc('getSlot', [])
    print(f'Current slot: {slot}')
    
    # Get dex_amm address
    r = await conn._rpc('getAllSymbolRegistry')
    dex_amm = None
    for e in r.get('entries', []):
        if e.get('symbol') == 'DEXAMM':
            dex_amm = e['program']
    print(f'dex_amm: {dex_amm}')
    
    # Scan all blocks for contract calls to dex_amm
    amm_txs = []
    for s in range(1, slot + 1):
        try:
            block = await conn._rpc('getBlock', [s])
            if block and 'transactions' in block:
                for tx in block['transactions']:
                    if tx.get('type') == 'ContractCall' and tx.get('to') == dex_amm:
                        amm_txs.append((s, tx))
        except:
            pass
    
    print(f'\nTotal AMM txs in blocks: {len(amm_txs)}')
    for s, tx in amm_txs:
        status = tx.get('status', '?')
        value = tx.get('amount_spores', tx.get('amount', '?'))
        sig = tx.get('signature', '?')[:20]
        print(f'  slot={s} status={status} value={value} sig={sig}...')

asyncio.run(main())
