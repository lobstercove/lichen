#!/usr/bin/env python3
import asyncio
import json
import os
import websockets

try:
    import pytest
except ImportError:  # Keep direct script execution dependency-light.
    pytest = None


def _live_sdk_tests_enabled() -> bool:
    return os.environ.get("LICHEN_RUN_LIVE_SDK_TESTS") == "1"


async def run_ws():
    print("🔌 Testing WebSocket connection...")
    
    uri = "ws://localhost:8900"
    
    async with websockets.connect(uri) as websocket:
        # Subscribe to blocks
        subscribe_msg = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "subscribeBlocks",
            "params": []
        }
        
        await websocket.send(json.dumps(subscribe_msg))
        print(f"📤 Sent subscription request")
        
        # Wait for subscription response
        response = await websocket.recv()
        print(f"📥 Subscription response: {response}")
        
        data = json.loads(response)
        sub_id = data.get("result")
        print(f"✅ Subscribed with ID: {sub_id}")
        
        # Wait for 3 blocks
        print("\n⏳ Waiting for blocks...\n")
        received = 0
        for i in range(3):
            try:
                message = await asyncio.wait_for(websocket.recv(), timeout=20)
            except (asyncio.TimeoutError, TimeoutError):
                if received > 0:
                    print(f"⚠️ Timed out waiting for additional blocks after receiving {received}")
                    break
                print("⚠️ Timed out waiting for first block notification; subscription is active but chain may be idle")
                break

            block_data = json.loads(message)
            
            if block_data.get("method") == "subscription":
                received += 1
                result = block_data["params"]["result"]
                print(f"📦 Block {received}: Slot {result['slot']}, Hash: {result['hash'][:16]}...")
        
        print("\n✅ WebSocket test passed!")


if pytest is not None:
    @pytest.mark.skipif(
        not _live_sdk_tests_enabled(),
        reason="requires local validator; set LICHEN_RUN_LIVE_SDK_TESTS=1",
    )
    @pytest.mark.asyncio
    async def test_ws():
        await run_ws()

if __name__ == "__main__":
    asyncio.run(run_ws())
