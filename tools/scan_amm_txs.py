#!/usr/bin/env python3
"""Scan blocks for AMM contract call transactions and show return codes."""
import asyncio, sys, os
sys.path.insert(0, os.path.join(os.path.dirname(__file__), '..', 'sdk', 'python'))
from lichen import Connection

RPC = os.environ.get('LICHEN_RPC_URL', 'http://127.0.0.1:8899')

async def main():
    conn = Connection(RPC)
    slot = await conn._rpc('getSlot', [])
    print(f'Current slot: {slot}')
    
    contract_txs = []
    for s in range(1, slot + 1):
        try:
            block = await conn._rpc('getBlock', [s])
            if block and 'transactions' in block:
                for tx in block['transactions']:
                    if tx.get('type') == 'ContractCall':
                        contract_txs.append((s, tx))
        except:
            pass
    
    print(f'Total ContractCall txs: {len(contract_txs)}')
    print(f'Last 30:')
    for s, tx in contract_txs[-30:]:
        rc = tx.get('return_code', '?')
        status = tx.get('status', '?')
        sig = tx.get('signature', '?')[:16]
        fee = tx.get('fee', '?')
        print(f'  slot={s} rc={rc} status={status} fee={fee} sig={sig}...')

asyncio.run(main())
