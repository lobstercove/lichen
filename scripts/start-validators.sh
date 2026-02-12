#!/bin/bash
cd /Users/johnrobin/.openclaw/workspace/moltchain

# Start Validator 1
./target/release/moltchain-validator \
  --p2p-port 8000 --rpc-port 8899 --ws-port 8900 \
  > /tmp/v1.log 2>&1 &
V1_PID=$!
echo "✅ Started Validator 1 (PID: $V1_PID)"
sleep 3

# Start Validator 2
./target/release/moltchain-validator \
  --p2p-port 8001 --rpc-port 8901 --ws-port 8902 \
  --seed-peer /ip4/127.0.0.1/udp/8000/quic-v1 \
  > /tmp/v2.log 2>&1 &
V2_PID=$!
echo "✅ Started Validator 2 (PID: $V2_PID)"
sleep 3

# Start Validator 3
./target/release/moltchain-validator \
  --p2p-port 8002 --rpc-port 8903 --ws-port 8904 \
  --seed-peer /ip4/127.0.0.1/udp/8000/quic-v1 \
  > /tmp/v3.log 2>&1 &
V3_PID=$!
echo "✅ Started Validator 3 (PID: $V3_PID)"
sleep 3

echo ""
echo "📊 Validator Status:"
ps aux | grep moltchain-validator | grep -v grep | awk '{print "  - PID " $2 ": " $13 " " $14}'

echo ""
echo "🔍 Checking network..."
curl -s http://localhost:8899 -X POST -d '{"jsonrpc":"2.0","id":1,"method":"getValidators","params":[]}' | python3 -c "
import json, sys
try:
    d = json.load(sys.stdin)
    if 'result' in d:
        print(f'Network has {d[\"result\"][\"count\"]} validators')
        for v in d['result']['validators']:
            print(f'  - {v[\"pubkey\"][:20]}... (blocks: {v[\"blocks_proposed\"]}, rep: {v[\"reputation\"]})')
except:
    print('RPC not ready yet')
"
