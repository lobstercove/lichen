#!/usr/bin/env python3
"""Test Lichen Python SDK against live validator"""

import asyncio
import os
import sys
from lichen import Connection, PublicKey

try:
    import pytest
except ImportError:  # Keep direct script execution dependency-light.
    pytest = None


def _live_sdk_tests_enabled() -> bool:
    return os.environ.get("LICHEN_RUN_LIVE_SDK_TESTS") == "1"


async def run_sdk():
    print('🦞 Lichen Python SDK Test\n')
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


if pytest is not None:
    @pytest.mark.skipif(
        not _live_sdk_tests_enabled(),
        reason="requires local validator; set LICHEN_RUN_LIVE_SDK_TESTS=1",
    )
    @pytest.mark.asyncio
    async def test_sdk():
        assert await run_sdk()


if __name__ == '__main__':
    success = asyncio.run(run_sdk())
    sys.exit(0 if success else 1)
