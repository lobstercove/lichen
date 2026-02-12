#!/bin/bash
# MoltChain Multi-Validator Test Script
# Starts 3 validators and monitors their consensus

set -e

echo "🦞⚡ MoltChain Multi-Validator Test"
echo "===================================="
echo ""

# Clean up any running validators
echo "Stopping existing validators..."
pkill -f moltchain-validator 2>/dev/null || true
sleep 1

# Clean data
echo "Cleaning data directories..."
rm -rf ./data
mkdir -p ./data

echo ""
echo "Building MoltChain..."
cargo build --release --quiet

echo ""
echo "🦞 Starting 3 validators..."
echo ""

# Start Validator 1 (Seed) on port 8000
echo "Starting Validator 1 (Seed) on port 8000..."
./target/release/moltchain-validator 8000 > ./data/val1.log 2>&1 &
VAL1_PID=$!
echo "  → PID: $VAL1_PID"

# Wait for validator 1 to initialize
sleep 3

# Start Validator 2 on port 8001
echo "Starting Validator 2 on port 8001 (connects to 8000)..."
./target/release/moltchain-validator 8001 127.0.0.1:8000 > ./data/val2.log 2>&1 &
VAL2_PID=$!
echo "  → PID: $VAL2_PID"

# Wait a bit
sleep 2

# Start Validator 3 on port 8002
echo "Starting Validator 3 on port 8002 (connects to 8000, 8001)..."
./target/release/moltchain-validator 8002 127.0.0.1:8000 127.0.0.1:8001 > ./data/val3.log 2>&1 &
VAL3_PID=$!
echo "  → PID: $VAL3_PID"

# Save PIDs
echo "$VAL1_PID $VAL2_PID $VAL3_PID" > ./data/validators.pid

echo ""
echo "✅ All validators started!"
echo ""
echo "PIDs: $VAL1_PID, $VAL2_PID, $VAL3_PID"
echo ""
echo "Monitor logs:"
echo "  tail -f ./data/val1.log"
echo "  tail -f ./data/val2.log"
echo "  tail -f ./data/val3.log"
echo ""
echo "Stop all validators:"
echo "  kill $VAL1_PID $VAL2_PID $VAL3_PID"
echo "  or: pkill -f moltchain-validator"
echo ""
echo "Waiting 10 seconds for consensus to stabilize..."
sleep 10

echo ""
echo "📊 Validator Status Check:"
echo "------------------------"
echo ""

echo "Validator 1 (last 5 blocks):"
tail -5 ./data/val1.log | grep "Block " || echo "  (no blocks yet)"

echo ""
echo "Validator 2 (last 5 blocks):"
tail -5 ./data/val2.log | grep "Block " || echo "  (no blocks yet)"

echo ""
echo "Validator 3 (last 5 blocks):"
tail -5 ./data/val3.log | grep "Block " || echo "  (no blocks yet)"

echo ""
echo "🌐 P2P Network Status:"
echo "--------------------"
grep "P2P:" ./data/val1.log | tail -5
grep "P2P:" ./data/val2.log | tail -5
grep "P2P:" ./data/val3.log | tail -5

echo ""
echo "Press Ctrl+C to stop monitoring..."
echo "Following all validator logs..."
echo ""

# Follow all logs
tail -f ./data/val1.log ./data/val2.log ./data/val3.log
