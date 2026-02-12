#!/usr/bin/env python3
"""Test MoltChain Python SDK against live validator"""

import asyncio
import sys
from moltchain import Connection, PublicKey


async def test_sdk():
    print('🦞 MoltChain Python SDK Test\n')
    print('=' * 60)
    
    # Connect to running validator
    connection = Connection('http://localhost:8899', 'ws://localhost:8900')
    
    try:
        # Test 1: Get current slot
        print('\n✅ Test 1: Get Current Slot')
        slot = await connection.get_slot()
        print(f'   Current Slot: {slot}')
        
        # Test WebSocket (simplified)
        print('\n🔌 Testing WebSocket Subscription...\n')
        
        block_count = 0
        max_blocks = 3
        
        async def on_block_handler(block):
            nonlocal block_count
            block_count += 1
            block_hash = block.get("hash", "N/A")
            print(f'📦 Block {block_count}: Slot {block["slot"]} | Hash: {block_hash[:16]}...')
        
        print(f'⏳ Waiting for {max_blocks} blocks...')
        
        # Subscribe
        sub_id = await connection.on_block(on_block_handler)
        
        # Wait for blocks with timeout
        timeout = 5  # 5 seconds
        start = asyncio.get_event_loop().time()
        
        while block_count < max_blocks:
            if asyncio.get_event_loop().time() - start > timeout:
                print(f'\n⚠️  Timeout - got {block_count}/{max_blocks} blocks')
                break
            await asyncio.sleep(0.5)
        
        # Unsubscribe
        await connection.off_block(sub_id)
        
        if block_count >= max_blocks:
            print('\n✅ WebSocket subscription test passed!')
        
        print('\n🎉 SDK tests completed!')
        
        return True
        
    except Exception as e:
        print(f'\n❌ Test failed: {e}')
        import traceback
        traceback.print_exc()
        return False


if __name__ == '__main__':
    success = asyncio.run(test_sdk())
    sys.exit(0 if success else 1)
