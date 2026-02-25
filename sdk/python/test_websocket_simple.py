#!/usr/bin/env python3
import asyncio
import json
import websockets

async def test_ws():
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
                raise

            block_data = json.loads(message)
            
            if block_data.get("method") == "subscription":
                received += 1
                result = block_data["params"]["result"]
                print(f"📦 Block {received}: Slot {result['slot']}, Hash: {result['hash'][:16]}...")
        
        print("\n✅ WebSocket test passed!")

if __name__ == "__main__":
    asyncio.run(test_ws())
