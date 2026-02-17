#!/bin/bash
# Start validator from the correct working directory
cd "$(dirname "$0")/.."
rm -rf data/state-8000
mkdir -p data/state-8000
exec env RUST_LOG=info ./target/release/moltchain-validator \
  --dev-mode \
  --p2p-port 8000 \
  --rpc-port 8899 \
  --db-path "$PWD/data/state-8000"
