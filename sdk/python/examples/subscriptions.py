"""Subscription example for MoltChain Python SDK"""

import asyncio
import signal
from moltchain import Connection, PublicKey


# Global flag for graceful shutdown
running = True


def signal_handler(signum, frame):
    """Handle Ctrl+C"""
    global running
    running = False
    print('\n\n🛑 Shutting down...')


async def main():
    global running
    
    connection = Connection(
        'http://localhost:8899',
        'ws://localhost:8900'
    )
    
    print('🦞 MoltChain Subscription Example\n')
    print('Press Ctrl+C to exit\n')
    
    # Track subscription IDs
    subscriptions = []
    
    # 1. Subscribe to slots
    print('📡 Subscribing to slots...')
    async def on_slot(slot):
        print(f"⏱️  Slot {slot}")
    
    slot_sub = await connection.on_slot(on_slot)
    subscriptions.append(('slot', slot_sub))
    
    # 2. Subscribe to blocks
    print('📡 Subscribing to blocks...')
    async def on_block(block):
        print(f"📦 Block #{block['slot']}: {block['transactions']} TXs, Hash: {block['hash'][:16]}...")
    
    block_sub = await connection.on_block(on_block)
    subscriptions.append(('block', block_sub))
    
    # 3. Subscribe to transactions
    print('📡 Subscribing to transactions...')
    async def on_transaction(tx):
        sigs = tx.get('signatures', [])
        if sigs:
            print(f"💸 Transaction: {sigs[0][:16]}...")
    
    tx_sub = await connection.on_transaction(on_transaction)
    subscriptions.append(('transaction', tx_sub))
    
    # 4. Subscribe to account changes
    print('📡 Subscribing to account changes...')
    pubkey = PublicKey('YourPublicKeyHere...')
    async def on_account(account):
        print(f"👤 Account {account['pubkey'][:12]}... balance: {account['molt']} MOLT")
    
    account_sub = await connection.on_account_change(pubkey, on_account)
    subscriptions.append(('account', account_sub))
    
    # 5. Subscribe to all contract logs
    print('📡 Subscribing to contract logs...')
    async def on_logs(log):
        print(f"📝 Log from {log['contract'][:12]}...: {log['message']}")
    
    logs_sub = await connection.on_logs(on_logs)
    subscriptions.append(('logs', logs_sub))
    
    print('\n✅ All subscriptions active!\n')
    print('Listening for events...\n')
    
    # Register signal handler
    signal.signal(signal.SIGINT, signal_handler)
    
    # Keep running until Ctrl+C
    try:
        while running:
            await asyncio.sleep(1)
    except KeyboardInterrupt:
        pass
    
    # Cleanup
    print('Unsubscribing...')
    for sub_type, sub_id in subscriptions:
        if sub_type == 'slot':
            await connection.off_slot(sub_id)
        elif sub_type == 'block':
            await connection.off_block(sub_id)
        elif sub_type == 'transaction':
            await connection.off_transaction(sub_id)
        elif sub_type == 'account':
            await connection.off_account_change(sub_id)
        elif sub_type == 'logs':
            await connection.off_logs(sub_id)
    
    await connection.close()
    print('✅ Disconnected')


if __name__ == '__main__':
    asyncio.run(main())
