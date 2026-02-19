#!/bin/bash
# Start validator from the correct working directory
# Usage: ./start-validator.sh [--keep-state]
cd "$(dirname "$0")/.."
if [[ "$1" != "--keep-state" ]]; then
    echo "⚠️  Wiping state in data/state-8000 (use --keep-state to preserve)"
    rm -rf data/state-8000
fi
mkdir -p data/state-8000
exec env RUST_LOG=info ./target/release/moltchain-validator \
  --dev-mode \
  --p2p-port 8000 \
  --rpc-port 8899 \
  --db-path "$PWD/data/state-8000"
