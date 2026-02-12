#!/bin/bash

set -e

NETWORK=${1:-testnet}
NETWORK=$(echo "$NETWORK" | tr '[:upper:]' '[:lower:]')
SOLANA_RPC_URL=${2:-${CUSTODY_SOLANA_RPC_URL:-}}
EVM_RPC_URL=${3:-${CUSTODY_EVM_RPC_URL:-}}

case $NETWORK in
  testnet)
    BASE_P2P=7001
    BASE_RPC=8899
    ;;
  mainnet)
    BASE_P2P=8001
    BASE_RPC=9899
    ;;
  *)
    echo "Usage: $0 [testnet|mainnet]"
    exit 1
    ;;
esac

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="${SCRIPT_DIR}/.."
cd "$REPO_ROOT" || exit 1

LOG_DIR="/tmp/moltchain-local-${NETWORK}"
mkdir -p "$LOG_DIR"

CHAIN_ID="moltchain-${NETWORK}-1"
TREASURY_KEYPAIR="./data/state-${NETWORK}-${BASE_P2P}/genesis-keys/treasury-${CHAIN_ID}.json"

export CUSTODY_MOLT_RPC_URL="http://127.0.0.1:${BASE_RPC}"
export CUSTODY_TREASURY_KEYPAIR="$TREASURY_KEYPAIR"
if [ -n "$SOLANA_RPC_URL" ]; then
  export CUSTODY_SOLANA_RPC_URL="$SOLANA_RPC_URL"
fi
if [ -n "$EVM_RPC_URL" ]; then
  export CUSTODY_EVM_RPC_URL="$EVM_RPC_URL"
fi

if [ ! -x "./target/release/moltchain-validator" ]; then
  cargo build --release
fi

./skills/validator/run-validator.sh "$NETWORK" 1 >"${LOG_DIR}/validator-1.log" 2>&1 &
V1_PID=$!

sleep 2

./skills/validator/run-validator.sh "$NETWORK" 2 >"${LOG_DIR}/validator-2.log" 2>&1 &
V2_PID=$!

sleep 2

./skills/validator/run-validator.sh "$NETWORK" 3 >"${LOG_DIR}/validator-3.log" 2>&1 &
V3_PID=$!

sleep 2

./skills/custody/run-custody.sh "$NETWORK" >"${LOG_DIR}/custody.log" 2>&1 &
CUSTODY_PID=$!

# ── First-boot contract deployment ──
# Wait 5s for validators to stabilize, then deploy all contracts if not yet deployed
echo "🔧 Running first-boot contract deployment..."
sleep 5
"${SCRIPT_DIR}/first-boot-deploy.sh" --rpc "http://127.0.0.1:${BASE_RPC}" --skip-build >"${LOG_DIR}/first-boot-deploy.log" 2>&1 &
DEPLOY_PID=$!

echo "🦞 MoltChain local stack started"
echo "Network: $NETWORK"
echo "Validator PIDs: $V1_PID $V2_PID $V3_PID"
echo "Custody PID: $CUSTODY_PID"
echo "Deploy PID: $DEPLOY_PID"
if [ -n "$SOLANA_RPC_URL" ]; then
  echo "Solana RPC: $SOLANA_RPC_URL"
fi
if [ -n "$EVM_RPC_URL" ]; then
  echo "EVM RPC: $EVM_RPC_URL"
fi
echo "Logs: $LOG_DIR"
