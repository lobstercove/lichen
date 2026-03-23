"""Test Lichen Python SDK against live validator"""

import asyncio
import sys
from lichen import Connection, PublicKey


async def test_sdk():
    print('🦞 Lichen Python SDK Test\n')
    print('=' * 60)
    
    # Connect to running validator
    connection = Connection('http://localhost:8899', 'ws://localhost:8900')
    
    try:
        # Test 1: Get current slot
        print('\n✅ Test 1: Get Current Slot')
        slot = await connection.get_slot()
        print(f'   Current Slot: {slot}')
        assert slot > 0, "Slot should be greater than 0"
        
        # Test 2: Get latest block
        print('\n✅ Test 2: Get Latest Block')
        block = await connection.get_latest_block()
        print(f'   Block Slot: {block["slot"]}')
        print(f'   Block Hash: {block["hash"][:16]}...')
        print(f'   Parent Hash: {block.get("parent_hash", block.get("parentHash", "N/A"))[:16]}...')
        print(f'   Transactions: {block.get("transaction_count", len(block.get("transactions", [])))}')
        print(f'   Validator: {block.get("validator", "N/A")[:20]}...')
        
        # Test 3: Get specific block
        print('\n✅ Test 3: Get Specific Block')
        specific_block = await connection.get_block(slot)
        print(f'   Retrieved block for slot {slot}')
        print(f'   Hash: {specific_block.get("hash", "N/A")[:16]}...')
        
        # Test 4: Get network info
        print('\n✅ Test 4: Get Network Info')
        network_info = await connection.get_network_info()
        print(f'   Chain ID: {network_info["chain_id"]}')
        print(f'   Version: {network_info["version"]}')
        print(f'   Validators: {network_info["validator_count"]}')
        
        # Test 5: Get chain status
        print('\n✅ Test 5: Get Chain Status')
        status = await connection.get_chain_status()
        print(f'   Current Slot: {status["current_slot"]}')
        print(f'   Total Blocks: {status["total_blocks"]}')
        print(f'   Total Transactions: {status["total_transactions"]}')
        print(f'   TPS: {status["tps"]}')
        print(f'   Healthy: {status["is_healthy"]}')
        
        # Test 6: Get validators
        print('\n✅ Test 6: Get Validators')
        validators = await connection.get_validators()
        print(f'   Total Validators: {len(validators)}')
        for i, v in enumerate(validators[:3], 1):
            print(f'   {i}. {v["pubkey"][:20]}... (Stake: {v["stake"] / 1e9:.2f} LICN)')
        
        # Test 7: Health check
        print('\n✅ Test 7: Health Check')
        health = await connection.health()
        print(f'   Status: {health["status"]}')
        
        # Test 8: Get metrics
        print('\n✅ Test 8: Get Performance Metrics')
        metrics = await connection.get_metrics()
        print(f'   Blocks Produced: {metrics.get("blocks_produced", "N/A")}')
        print(f'   Transactions Processed: {metrics.get("transactions_processed", "N/A")}')
        
        # Test 9: Get total burned
        print('\n✅ Test 9: Get Total Burned')
        burned = await connection.get_total_burned()
        print(f'   Total Burned: {burned["licn"] / 1e9:.6f} LICN')
        
        # Test 10: Get balance (will likely fail for non-existent account)
        print('\n✅ Test 10: Get Balance')
        # Use the validator's pubkey from the validators list
        if validators:
            validator_pubkey = PublicKey(validators[0]["pubkey"])
            try:
                balance = await connection.get_balance(validator_pubkey)
                print(f'   Validator Balance: {balance["licn"] / 1e9:.6f} LICN')
            except Exception as e:
                print(f'   Expected error for query: {e}')
        
        print('\n' + '=' * 60)
        print('✅ All basic RPC tests passed!')
        print('=' * 60 + '\n')
        
        # Test WebSocket subscriptions
        print('🔌 Testing WebSocket Subscriptions...\n')
        
        block_count = 0
        max_blocks = 5
        
        async def on_block_handler(block):
            nonlocal block_count
            block_count += 1
            block_hash = block.get("hash", "N/A")
            print(f'📦 New Block: Slot {block["slot"]} | Hash: {block_hash[:16]}...')
        
        print(f'⏳ Subscribing to next {max_blocks} blocks...\n')
        
        # Subscribe and wait for blocks
        sub_id = await connection.on_block(on_block_handler)
        
        # Wait for blocks
        while block_count < max_blocks:
            await asyncio.sleep(0.5)
        
        # Unsubscribe
        await connection.off_block(sub_id)
        
        print('\n✅ WebSocket subscription test passed!')
        print('\n🎉 All SDK tests completed successfully!')
        
        return True
        
    except Exception as e:
        print(f'\n❌ Test failed: {e}')
        import traceback
        traceback.print_exc()
        return False


if __name__ == '__main__':
    success = asyncio.run(test_sdk())
    sys.exit(0 if success else 1)
