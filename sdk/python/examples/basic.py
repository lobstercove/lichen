"""Basic usage example for MoltChain Python SDK"""

import asyncio
from moltchain import Connection, PublicKey


async def main():
    # Connect to MoltChain
    connection = Connection('http://localhost:8899', 'ws://localhost:8900')
    
    print('🦞 MoltChain Python SDK Example\n')
    
    # 1. Get network info
    print('📡 Network Information:')
    network_info = await connection.get_network_info()
    print(f"  Chain ID: {network_info['chainId']}")
    print(f"  Version: {network_info['version']}")
    print(f"  Current Slot: {network_info['currentSlot']}")
    print(f"  Validators: {network_info['validatorCount']}\n")
    
    # 2. Get account balance
    print('💰 Account Balance:')
    pubkey = PublicKey('YourPublicKeyHere...')
    try:
        balance = await connection.get_balance(pubkey)
        print(f"  Balance: {balance['molt']} MOLT ({balance['shells']} shells)\n")
    except Exception as e:
        print(f"  Error: {e}\n")
    
    # 3. Get chain status
    print('⛓️  Chain Status:')
    status = await connection.get_chain_status()
    print(f"  TPS: {status['tps']}")
    print(f"  Total Blocks: {status['totalBlocks']}")
    print(f"  Total Transactions: {status['totalTransactions']}")
    print(f"  Healthy: {status['isHealthy']}\n")
    
    # 4. Get all validators
    print('🔒 Validators:')
    validators = await connection.get_validators()
    print(f"  Total: {len(validators)}")
    for i, v in enumerate(validators[:5], 1):
        print(f"  {i}. {v['pubkey'][:12]}... (Stake: {v['stake'] / 1e9} MOLT)")
    print()
    
    # 5. Subscribe to real-time events
    print('🔔 Subscribing to real-time events...\n')
    
    # Subscribe to blocks
    async def on_block(block):
        print(f"📦 New Block #{block['slot']}: {block['transactions']} transactions")
    
    block_sub = await connection.on_block(on_block)
    
    # Subscribe to slots
    async def on_slot(slot):
        print(f"⏱️  Slot: {slot}")
    
    slot_sub = await connection.on_slot(on_slot)
    
    # Wait for events for 30 seconds
    print('Listening for 30 seconds...\n')
    await asyncio.sleep(30)
    
    # Cleanup
    await connection.off_block(block_sub)
    await connection.off_slot(slot_sub)
    await connection.close()
    
    print('\n✅ Example complete!')


if __name__ == '__main__':
    asyncio.run(main())
