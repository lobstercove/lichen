#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

RPC_URL="http://localhost:8899"

log() {
  echo "[$(date +"%H:%M:%S")] $*"
}

log "Resetting blockchain state..."
./reset-blockchain.sh

log "Building release binaries..."
cargo build --release

log "Starting validators (1/2/3)..."
./run-validator.sh 1 > /tmp/moltchain-v1.log 2>&1 &
V1_PID=$!

sleep 2
./run-validator.sh 2 > /tmp/moltchain-v2.log 2>&1 &
V2_PID=$!

sleep 2
./run-validator.sh 3 > /tmp/moltchain-v3.log 2>&1 &
V3_PID=$!

log "Waiting for RPC..."
for i in {1..30}; do
  if curl -s "$RPC_URL" -X POST -H "Content-Type: application/json" \
    -d '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}' >/dev/null; then
    break
  fi
  sleep 1
  if [ "$i" -eq 30 ]; then
    echo "RPC not ready after 30s. See /tmp/moltchain-v1.log"
    exit 1
  fi
done

KEY_DIR="$REPO_ROOT/data/state-7001/genesis-keys"
GENESIS_KEY="$(ls -1 "$KEY_DIR"/genesis-primary-*.json 2>/dev/null | head -n 1)"

if [ -z "$GENESIS_KEY" ]; then
  echo "Genesis keypair not found in $KEY_DIR"
  exit 1
fi

log "Seeding marketplace demo data..."
log "Using keypair: $GENESIS_KEY"

cargo run -p molt-cli --bin marketplace-demo -- \
  --rpc-url "$RPC_URL" \
  --keypair "$GENESIS_KEY" \
  --collections 3 \
  --mints-per-collection 4

log "Done. Validator logs: /tmp/moltchain-v1.log /tmp/moltchain-v2.log /tmp/moltchain-v3.log"
log "To stop validators: kill $V1_PID $V2_PID $V3_PID"
