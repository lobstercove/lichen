#!/bin/bash

set -euo pipefail

NETWORK=${1:-testnet}
NETWORK=$(echo "$NETWORK" | tr '[:upper:]' '[:lower:]')
SOLANA_RPC_URL=${2:-${CUSTODY_SOLANA_RPC_URL:-}}
EVM_RPC_URL=${3:-${CUSTODY_EVM_RPC_URL:-}}

case $NETWORK in
  testnet)
    BASE_P2P=7001
    BASE_RPC=8899
    CUSTODY_PORT=9105
    ;;
  mainnet)
    BASE_P2P=8001
    BASE_RPC=9899
    CUSTODY_PORT=9106
    ;;
  *)
    echo "Usage: $0 [testnet|mainnet]"
    exit 1
    ;;
esac

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="${SCRIPT_DIR}/.."
cd "$REPO_ROOT" || exit 1

generate_local_token() {
  if command -v python3 >/dev/null 2>&1; then
    python3 - <<'PY'
import secrets
print(secrets.token_hex(24))
PY
    return 0
  fi

  if command -v openssl >/dev/null 2>&1; then
    openssl rand -hex 24
    return 0
  fi

  echo "python3 or openssl is required to generate local auth tokens" >&2
  exit 1
}

clear_local_peer_trust_state() {
  local p2p_port
  for p2p_port in "$BASE_P2P" "$((BASE_P2P + 1))" "$((BASE_P2P + 2))"; do
    local state_dir="$REPO_ROOT/data/state-${p2p_port}"
    rm -f "$state_dir/known-peers.json" 2>/dev/null || true
    rm -f "$state_dir/home/.lichen/peer_identities.json" 2>/dev/null || true
    rm -rf "$state_dir/home/.lichen/validators" 2>/dev/null || true
  done
}

any_path_newer_than() {
  local target=$1
  shift

  if [ ! -e "$target" ]; then
    return 0
  fi

  local path newer_file
  for path in "$@"; do
    if [ ! -e "$path" ]; then
      continue
    fi

    if [ -d "$path" ]; then
      newer_file=$(find "$path" -type f -newer "$target" -print -quit)
      if [ -n "$newer_file" ]; then
        return 0
      fi
    elif [ "$path" -nt "$target" ]; then
      return 0
    fi
  done

  return 1
}

ensure_runtime_binaries() {
  local runtime_sources=(
    ./Cargo.toml
    ./Cargo.lock
    ./core
    ./validator
    ./rpc
    ./p2p
    ./cli
    ./genesis
    ./custody
    ./faucet-service
  )

  if any_path_newer_than "./target/release/lichen-validator" "${runtime_sources[@]}" \
    || any_path_newer_than "./target/release/lichen-custody" "${runtime_sources[@]}" \
    || any_path_newer_than "./target/release/lichen-faucet" "${runtime_sources[@]}"; then
    echo "🔨 Rebuilding local runtime release binaries..."
    cargo build --release -p lichen-validator -p lichen-custody -p lichen-faucet
  fi
}

refresh_changed_contract_wasm() {
  local contract_dir manifest contract_name root_wasm target_wasm

  for contract_dir in ./contracts/*; do
    [ -d "$contract_dir" ] || continue

    manifest="$contract_dir/Cargo.toml"
    [ -f "$manifest" ] || continue

    contract_name=$(basename "$contract_dir")
    root_wasm="$contract_dir/${contract_name}.wasm"
    target_wasm="$contract_dir/target/wasm32-unknown-unknown/release/${contract_name}.wasm"

    if any_path_newer_than "$root_wasm" "$manifest" "$contract_dir/Cargo.lock" "$contract_dir/src"; then
      echo "🔨 Refreshing ${contract_name}.wasm..."
      cargo build --manifest-path "$manifest" --target wasm32-unknown-unknown --release
      cp "$target_wasm" "$root_wasm"
    fi
  done
}

export LICHEN_LOCAL_DEV=1
export LICHEN_SIGNER_AUTH_TOKEN="${LICHEN_SIGNER_AUTH_TOKEN:-$(generate_local_token)}"
if [ -z "${CUSTODY_SIGNER_AUTH_TOKENS:-}" ] && [ -z "${CUSTODY_SIGNER_AUTH_TOKEN:-}" ]; then
  export CUSTODY_SIGNER_AUTH_TOKEN="$LICHEN_SIGNER_AUTH_TOKEN"
fi
export CUSTODY_API_AUTH_TOKEN="${CUSTODY_API_AUTH_TOKEN:-$(generate_local_token)}"
export CUSTODY_URL="${CUSTODY_URL:-http://127.0.0.1:${CUSTODY_PORT}}"

LOG_DIR="/tmp/lichen-local-${NETWORK}"
mkdir -p "$LOG_DIR"

CHAIN_ID="lichen-${NETWORK}-1"
GENESIS_KEYS_DIR="./data/state-${BASE_P2P}/genesis-keys"
GENESIS_PRIMARY_KEYPAIR="${GENESIS_KEYS_DIR}/genesis-primary-${CHAIN_ID}.json"
GENESIS_TREASURY_KEYPAIR="${GENESIS_KEYS_DIR}/treasury-${CHAIN_ID}.json"
LOCAL_DEPLOYER_KEYPAIR="./keypairs/deployer.json"
RPC_CANDIDATES=("${BASE_RPC}" "$((BASE_RPC + 2))" "$((BASE_RPC + 4))")
if [ -n "$SOLANA_RPC_URL" ]; then
  export CUSTODY_SOLANA_RPC_URL="$SOLANA_RPC_URL"
fi
if [ -n "$EVM_RPC_URL" ]; then
  export CUSTODY_EVM_RPC_URL="$EVM_RPC_URL"
fi

ensure_runtime_binaries
refresh_changed_contract_wasm

clear_local_peer_trust_state

wait_for_file() {
  local file_path=$1
  local label=$2
  local timeout_seconds=${3:-90}

  for _ in $(seq 1 "$timeout_seconds"); do
    if [ -f "$file_path" ]; then
      return 0
    fi
    sleep 1
  done

  echo "❌ Timed out waiting for ${label}: ${file_path}" >&2
  exit 1
}

wait_for_healthy_rpc() {
  local timeout_seconds=${1:-60}

  for _ in $(seq 1 "$timeout_seconds"); do
    for rpc_port in "${RPC_CANDIDATES[@]}"; do
      local response
      response=$(curl -s -X POST "http://127.0.0.1:${rpc_port}" \
        -H 'Content-Type: application/json' \
        -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}' 2>/dev/null || true)
      if echo "$response" | grep -q '"status":"ok"'; then
        echo "http://127.0.0.1:${rpc_port}"
        return 0
      fi
    done
    sleep 1
  done

  echo "❌ Timed out waiting for a healthy validator RPC" >&2
  exit 1
}

./run-validator.sh "$NETWORK" 1 >"${LOG_DIR}/validator-1.log" 2>&1 &
V1_PID=$!

sleep 2

./run-validator.sh "$NETWORK" 2 >"${LOG_DIR}/validator-2.log" 2>&1 &
V2_PID=$!

sleep 2

./run-validator.sh "$NETWORK" 3 >"${LOG_DIR}/validator-3.log" 2>&1 &
V3_PID=$!

sleep 2

wait_for_file "$GENESIS_TREASURY_KEYPAIR" "genesis treasury keypair"
wait_for_file "$GENESIS_PRIMARY_KEYPAIR" "genesis primary keypair"

mkdir -p ./keypairs
cp "$GENESIS_PRIMARY_KEYPAIR" "$LOCAL_DEPLOYER_KEYPAIR"
export CUSTODY_TREASURY_KEYPAIR="${CUSTODY_TREASURY_KEYPAIR:-$LOCAL_DEPLOYER_KEYPAIR}"

CLUSTER_RPC_URL="$(wait_for_healthy_rpc 90)"
export CUSTODY_LICHEN_RPC_URL="$CLUSTER_RPC_URL"
export CUSTODY_ALLOW_INSECURE_SEED="${CUSTODY_ALLOW_INSECURE_SEED:-1}"

./scripts/run-custody.sh "$NETWORK" >"${LOG_DIR}/custody.log" 2>&1 &
CUSTODY_PID=$!

FAUCET_PID=""
FAUCET_PORT=9100
if [ "$NETWORK" = "testnet" ]; then
  # The faucet currently serves from the genesis treasury on local networks.
  PORT=$FAUCET_PORT RPC_URL="$CLUSTER_RPC_URL" NETWORK="$NETWORK" \
    TRUSTED_PROXY="127.0.0.1,::1" \
    FAUCET_KEYPAIR="$GENESIS_TREASURY_KEYPAIR" \
    ./target/release/lichen-faucet >"${LOG_DIR}/faucet.log" 2>&1 &
  FAUCET_PID=$!
fi

# ── First-boot contract deployment ──
# Wait 5s for validators to stabilize, then deploy all contracts if not yet deployed
echo "🔧 Running post-genesis bootstrap..."
sleep 5
"${SCRIPT_DIR}/first-boot-deploy.sh" --rpc "$CLUSTER_RPC_URL" --skip-build >"${LOG_DIR}/first-boot-deploy.log" 2>&1 &
DEPLOY_PID=$!

echo "🦞 Lichen local stack started"
echo "Network: $NETWORK"
echo "Cluster RPC: $CLUSTER_RPC_URL"
echo "Validator PIDs: $V1_PID $V2_PID $V3_PID"
echo "Custody PID: $CUSTODY_PID"
if [ -n "$FAUCET_PID" ]; then
  echo "Faucet PID: $FAUCET_PID (port $FAUCET_PORT)"
fi
echo "Bootstrap PID: $DEPLOY_PID"
if [ -n "$SOLANA_RPC_URL" ]; then
  echo "Solana RPC: $SOLANA_RPC_URL"
fi
if [ -n "$EVM_RPC_URL" ]; then
  echo "EVM RPC: $EVM_RPC_URL"
fi
echo "Logs: $LOG_DIR"
