#!/usr/bin/env bash
set -e
cd /Users/johnrobin/.openclaw/workspace/moltchain
BIN="$PWD/target/release/lichen-validator"

echo "=== Starting V1 (leader) ==="
RUST_LOG=info "$BIN" --dev-mode --p2p-port 7001 --rpc-port 8899 \
  --db-path "$PWD/data/state-7001" > /tmp/v1.log 2>&1 &
V1PID=$!
echo "V1 PID=$V1PID"
sleep 8

echo "=== Health check V1 ==="
curl -sf http://127.0.0.1:8899 -X POST -H 'Content-Type:application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"health"}' || { echo "V1 FAILED"; kill $V1PID; exit 1; }
echo ""

echo "=== Starting V2 ==="
RUST_LOG=info "$BIN" --dev-mode --p2p-port 7002 --rpc-port 8901 \
  --db-path "$PWD/data/state-7002" --bootstrap 127.0.0.1:7001 > /tmp/v2.log 2>&1 &
V2PID=$!
echo "V2 PID=$V2PID"
sleep 6

echo "=== Starting V3 ==="
RUST_LOG=info "$BIN" --dev-mode --p2p-port 7003 --rpc-port 8903 \
  --db-path "$PWD/data/state-7003" --bootstrap 127.0.0.1:7001 > /tmp/v3.log 2>&1 &
V3PID=$!
echo "V3 PID=$V3PID"
sleep 6

echo "=== Health checks ==="
for port in 8899 8901 8903; do
  STATUS=$(curl -sf http://127.0.0.1:$port -X POST -H 'Content-Type:application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"health"}' 2>/dev/null || echo '{"error":"DOWN"}')
  echo "  Port $port: $STATUS"
done

echo "=== Validator count ==="
curl -s http://127.0.0.1:8899 -X POST -H 'Content-Type:application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getValidators","params":[]}' | \
  python3 -c "import sys,json;r=json.load(sys.stdin).get('result',[]);print(f'  {len(r)} validators registered')"

echo ""
echo "=== 3-validator cluster ready ==="
echo "PIDs: V1=$V1PID V2=$V2PID V3=$V3PID"
echo "$V1PID $V2PID $V3PID" > /tmp/validator-pids.txt
wait
