#!/bin/bash
# Run 3 Lichen validators for multi-validator testing

echo "🦞 Lichen Multi-Validator Test"
echo "=================================="
echo ""

# Clean up old data
echo "Cleaning old data directories..."
rm -rf ./data/state-*

echo ""
echo "Starting 3 validators..."
echo ""

# Start validator 1 (seed) in background
echo "🦞 Starting Validator 1 on port 7001 (seed)..."
cargo run --package lichen-validator -- 7001 > validator1.log 2>&1 &
VALIDATOR1_PID=$!
echo "   PID: $VALIDATOR1_PID"

# Wait for validator 1 to start
sleep 3

# Start validator 2 (connects to seed)
echo "🦞 Starting Validator 2 on port 7002..."
cargo run --package lichen-validator -- 7002 127.0.0.1:7001 > validator2.log 2>&1 &
VALIDATOR2_PID=$!
echo "   PID: $VALIDATOR2_PID"

# Wait a bit
sleep 2

# Start validator 3 (connects to both)
echo "🦞 Starting Validator 3 on port 7003..."
cargo run --package lichen-validator -- 7003 127.0.0.1:7001 127.0.0.1:7002 > validator3.log 2>&1 &
VALIDATOR3_PID=$!
echo "   PID: $VALIDATOR3_PID"

echo ""
echo "✅ All validators started!"
echo ""
echo "Monitor logs:"
echo "  tail -f validator1.log"
echo "  tail -f validator2.log"
echo "  tail -f validator3.log"
echo ""
echo "To stop all validators:"
echo "  kill $VALIDATOR1_PID $VALIDATOR2_PID $VALIDATOR3_PID"
echo ""

# Save PIDs for cleanup
echo "$VALIDATOR1_PID $VALIDATOR2_PID $VALIDATOR3_PID" > validators.pid

echo "Press Ctrl+C to stop monitoring, validators will continue running..."
echo ""

# Follow logs
tail -f validator1.log validator2.log validator3.log
