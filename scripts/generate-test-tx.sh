#!/bin/bash
# Quick Transaction Generator
# Creates test transactions to verify explorer

echo "🦞 Generating Test Transactions"
echo "================================"
echo ""

# Create test wallet if doesn't exist
if [ ! -f ~/.moltchain/test-sender.json ]; then
    echo "Creating sender wallet..."
    ./target/release/molt identity new --output ~/.moltchain/test-sender.json
fi

if [ ! -f ~/.moltchain/test-receiver.json ]; then
    echo "Creating receiver wallet..."
    ./target/release/molt identity new --output ~/.moltchain/test-receiver.json
fi

SENDER=$(cat ~/.moltchain/test-sender.json | grep -A1 'publicKeyBase58' | tail -1 | cut -d'"' -f2)
RECEIVER=$(cat ~/.moltchain/test-receiver.json | grep -A1 'publicKeyBase58' | tail -1 | cut -d'"' -f2)

echo "Sender: $SENDER"
echo "Receiver: $RECEIVER"
echo ""

# Try to send transaction (will fail without balance, but creates activity)
echo "Attempting test transaction..."
./target/release/molt transfer --from ~/.moltchain/test-sender.json --to $RECEIVER --amount 1.0 || echo "⚠️ Transaction failed (expected - no balance)\n"

echo "✅ Transaction attempt complete"
echo ""
echo "Check explorer at: http://localhost:8080"
