#!/bin/bash
# Multi-Validator Test Setup
# Runs 3 validators for consensus testing

set -e

echo "🦞 MoltChain Multi-Validator Test Setup"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Stop any running validators
echo "1️⃣  Stopping existing validators..."
killall -9 moltchain-validator 2>/dev/null || true
sleep 2

# Clean data directories (optional - comment out to preserve state)
echo "2️⃣  Cleaning data directories..."
rm -rf ~/.moltchain/data-8000 || true
rm -rf ~/.moltchain/data-8001 || true
rm -rf ~/.moltchain/data-8002 || true

# Build validator
echo "3️⃣  Building validator..."
cargo build --release --bin moltchain-validator 2>&1 | tail -3

# Start Validator 1 (Genesis/Bootstrap node)
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "4️⃣  Starting Validator 1 (Genesis Node)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
./target/release/moltchain-validator \
  --data-dir ~/.moltchain/data-8000 \
  --rpc-port 8899 \
  --p2p-port 8000 \
  > /tmp/validator-1.log 2>&1 &
VAL1_PID=$!
echo "✅ Validator 1 started (PID: $VAL1_PID)"
echo "   RPC: http://localhost:8899"
echo "   P2P: 127.0.0.1:8000"

# Wait for genesis creation
echo "   ⏳ Waiting for genesis creation..."
sleep 5

# Check if validator 1 is healthy
if curl -s http://localhost:8899 >/dev/null 2>&1; then
    echo "   ✅ RPC server responding"
else
    echo "   ❌ RPC server not responding!"
    tail -20 /tmp/validator-1.log
    exit 1
fi

# Start Validator 2 (joining network)
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "5️⃣  Starting Validator 2 (Joining Network)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
./target/release/moltchain-validator \
  --data-dir ~/.moltchain/data-8001 \
  --rpc-port 8901 \
  --p2p-port 8001 \
  --bootstrap-peers 127.0.0.1:8000 \
  > /tmp/validator-2.log 2>&1 &
VAL2_PID=$!
echo "✅ Validator 2 started (PID: $VAL2_PID)"
echo "   RPC: http://localhost:8901"
echo "   P2P: 127.0.0.1:8001"
echo "   Bootstrap: 127.0.0.1:8000"

# Wait for peer connection
echo "   ⏳ Waiting for peer connection..."
sleep 5

# Start Validator 3 (joining network)
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "6️⃣  Starting Validator 3 (Joining Network)"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
./target/release/moltchain-validator \
  --data-dir ~/.moltchain/data-8002 \
  --rpc-port 8902 \
  --p2p-port 8002 \
  --bootstrap-peers 127.0.0.1:8000,127.0.0.1:8001 \
  > /tmp/validator-3.log 2>&1 &
VAL3_PID=$!
echo "✅ Validator 3 started (PID: $VAL3_PID)"
echo "   RPC: http://localhost:8902"
echo "   P2P: 127.0.0.1:8002"
echo "   Bootstrap: 127.0.0.1:8000,127.0.0.1:8001"

# Wait for network stabilization
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "7️⃣  Waiting for Network Stabilization (10 seconds)..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
sleep 10

# Check network health
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "8️⃣  Network Health Check"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

echo "Validator 1 Status:"
curl -s -X POST http://localhost:8899/ -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getNetworkInfo","params":[]}' | jq -r '
  "  Chain ID: \(.result.chain_id)",
  "  Slot: \(.result.current_slot)",
  "  Validators: \(.result.validator_count)",
  "  Peers: \(.result.peer_count)"
'

echo ""
echo "Validator 2 Status:"
curl -s -X POST http://localhost:8901/ -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getNetworkInfo","params":[]}' | jq -r '
  "  Chain ID: \(.result.chain_id)",
  "  Slot: \(.result.current_slot)",
  "  Validators: \(.result.validator_count)",
  "  Peers: \(.result.peer_count)"
'

echo ""
echo "Validator 3 Status:"
curl -s -X POST http://localhost:8902/ -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getNetworkInfo","params":[]}' | jq -r '
  "  Chain ID: \(.result.chain_id)",
  "  Slot: \(.result.current_slot)",
  "  Validators: \(.result.validator_count)",
  "  Peers: \(.result.peer_count)"
'

# Get validator list
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "9️⃣  Active Validators"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
curl -s -X POST http://localhost:8899/ -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getValidators","params":[]}' | jq '.result.validators[] | "  \(.pubkey) - \(.stake / 1000000000) MOLT - Rep: \(.reputation)"'

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ Multi-Validator Network Ready!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "📊 Quick Commands:"
echo "  ./target/release/molt status --rpc-url http://localhost:8899"
echo "  ./target/release/molt validators --rpc-url http://localhost:8899"
echo "  tail -f /tmp/validator-1.log"
echo "  tail -f /tmp/validator-2.log"
echo "  tail -f /tmp/validator-3.log"
echo ""
echo "🛑 Stop all validators:"
echo "  killall -9 moltchain-validator"
echo ""
