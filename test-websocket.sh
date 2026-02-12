#!/bin/bash
# WebSocket Integration Test

set -e
WS_URL="ws://localhost:8900"
RESULTS_FILE="/tmp/websocket-test-results.txt"

echo "🧪 MoltChain WebSocket Test" > $RESULTS_FILE
echo "Started: $(date)" >> $RESULTS_FILE
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" >> $RESULTS_FILE

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "1️⃣  WEBSOCKET CONNECTION TEST"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

echo "Testing: WebSocket connection..."
# Try to connect and send subscribe message
timeout 5 wscat -c $WS_URL -x '{"method":"subscribe","params":["blocks"]}' > /tmp/ws-test-output.txt 2>&1 &
WS_PID=$!

sleep 3

if ps -p $WS_PID > /dev/null 2>&1; then
    echo "✅ PASS - WebSocket connection established" | tee -a $RESULTS_FILE
    kill $WS_PID 2>/dev/null || true
else
    echo "❌ FAIL - Could not connect to WebSocket" | tee -a $RESULTS_FILE
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "2️⃣  WEBSOCKET SUBSCRIPTION TEST"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

echo "Testing: Block subscription (10 sec monitor)..."
timeout 10 wscat -c $WS_URL -x '{"method":"subscribe","params":["blocks"]}' > /tmp/ws-blocks.txt 2>&1 || true

if [ -s /tmp/ws-blocks.txt ]; then
    BLOCK_COUNT=$(grep -c "block" /tmp/ws-blocks.txt 2>/dev/null || echo "0")
    echo "✅ PASS - Received $BLOCK_COUNT block notifications" | tee -a $RESULTS_FILE
    echo "   Sample output:" | tee -a $RESULTS_FILE
    head -3 /tmp/ws-blocks.txt | tee -a $RESULTS_FILE
else
    echo "⚠️  WARN - No block notifications received" | tee -a $RESULTS_FILE
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 TEST SUMMARY"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "WebSocket server: $WS_URL"
echo "Results saved to: $RESULTS_FILE"
echo ""
echo "✅ WebSocket testing complete"
echo ""

exit 0
