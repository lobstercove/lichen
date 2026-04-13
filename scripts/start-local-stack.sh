#!/bin/bash

set -euo pipefail

# Restore a sane tool PATH when the caller shell exported a stripped environment.
BOOTSTRAP_PATH="/opt/homebrew/bin:/opt/homebrew/sbin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
if [ -n "${HOME:-}" ] && [ -d "${HOME}/.cargo/bin" ]; then
  BOOTSTRAP_PATH="${HOME}/.cargo/bin:${BOOTSTRAP_PATH}"
fi
if [ -n "${HOME:-}" ] && [ -d "${HOME}/.local/bin" ]; then
  BOOTSTRAP_PATH="${HOME}/.local/bin:${BOOTSTRAP_PATH}"
fi
PATH="${PATH:+${PATH}:}${BOOTSTRAP_PATH}"
export PATH

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
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "$REPO_ROOT" || exit 1
LOCAL_CLUSTER_SCRIPT="$REPO_ROOT/scripts/start-local-3validators.sh"
LOCAL_CLUSTER_RESET="${LICHEN_LOCAL_RESET_CLUSTER:-1}"

LOCAL_SIGNED_METADATA_KEYPAIR_DEFAULT="$REPO_ROOT/keypairs/release-signing-key.json"

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
  local contract_dir manifest contract_name artifact_name root_wasm target_wasm

  contract_artifact_name() {
    local manifest_path="$1"

    awk -F'"' '
      /^\[package\]/ { in_package = 1; in_lib = 0; next }
      /^\[lib\]/ { in_lib = 1; in_package = 0; next }
      /^\[/ { in_package = 0; in_lib = 0; next }
      in_package && $1 ~ /^[[:space:]]*name[[:space:]]*=[[:space:]]*$/ && package_name == "" { package_name = $2 }
      in_lib && $1 ~ /^[[:space:]]*name[[:space:]]*=[[:space:]]*$/ && lib_name == "" { lib_name = $2 }
      END {
        name = lib_name
        if (name == "") {
          name = package_name
        }
        gsub(/-/, "_", name)
        print name
      }
    ' "$manifest_path"
  }

  for contract_dir in ./contracts/*; do
    [ -d "$contract_dir" ] || continue

    manifest="$contract_dir/Cargo.toml"
    [ -f "$manifest" ] || continue

    contract_name=$(basename "$contract_dir")
    artifact_name=$(contract_artifact_name "$manifest")
    [ -n "$artifact_name" ] || artifact_name="$contract_name"
    root_wasm="$contract_dir/${contract_name}.wasm"
    target_wasm="$contract_dir/target/wasm32-unknown-unknown/release/${artifact_name}.wasm"

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
if [ -z "${SIGNED_METADATA_KEYPAIR:-}" ] && [ -f "$LOCAL_SIGNED_METADATA_KEYPAIR_DEFAULT" ]; then
  export SIGNED_METADATA_KEYPAIR="$LOCAL_SIGNED_METADATA_KEYPAIR_DEFAULT"
fi
export CUSTODY_API_AUTH_TOKEN="${CUSTODY_API_AUTH_TOKEN:-$(generate_local_token)}"
export CUSTODY_URL="${CUSTODY_URL:-http://127.0.0.1:${CUSTODY_PORT}}"
LOCAL_HEALTH_TIMEOUT_SECS="${LICHEN_LOCAL_HEALTH_TIMEOUT_SECS:-900}"

LOG_DIR="/tmp/lichen-local-${NETWORK}"
mkdir -p "$LOG_DIR"
CUSTODY_PID=""

SERVICE_FLEET_CONFIG_FILE="${LOG_DIR}/service-fleet-config.json"
SERVICE_FLEET_STATUS_FILE="${LOG_DIR}/service-fleet-status.json"
export LICHEN_SERVICE_FLEET_CONFIG_FILE="$SERVICE_FLEET_CONFIG_FILE"
export LICHEN_SERVICE_FLEET_STATUS_FILE="$SERVICE_FLEET_STATUS_FILE"

write_local_service_fleet_config() {
  local expected_faucet=true
  if [ "$NETWORK" != "testnet" ]; then
    expected_faucet=false
  fi

  cat > "$SERVICE_FLEET_CONFIG_FILE" <<EOF
{
  "schema_version": 1,
  "network": "local-${NETWORK}",
  "probe_timeout_ms": 1500,
  "hosts": [
    {
      "id": "local",
      "label": "Local Stack",
      "services": [
        {
          "id": "custody",
          "label": "Custody",
          "service": "custody",
          "probe": {
            "kind": "http",
            "url": "http://127.0.0.1:${CUSTODY_PORT}/health",
            "body_contains_any": ["\"status\":\"ok\"", "\"status\": \"ok\""]
          }
        },
        {
          "id": "faucet",
          "label": "Faucet",
          "service": "faucet",
          "expected": ${expected_faucet},
          "intentionally_absent_message": "The faucet only runs on local testnet.",
          "probe": {
            "kind": "http",
            "url": "http://127.0.0.1:9100/health",
            "body_contains_any": ["OK", "\"status\":\"ok\"", "\"status\": \"ok\""]
          }
        }
      ]
    }
  ]
}
EOF
}

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

write_local_service_fleet_config

ensure_runtime_binaries
refresh_changed_contract_wasm

clear_local_peer_trust_state

cleanup_started_processes() {
  "$LOCAL_CLUSTER_SCRIPT" stop >/dev/null 2>&1 || true
  if [ -n "${CUSTODY_PID:-}" ]; then
    kill "$CUSTODY_PID" 2>/dev/null || true
  fi
  if [ -n "${FAUCET_PID:-}" ]; then
    kill "$FAUCET_PID" 2>/dev/null || true
  fi
}

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
  return 1
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
  return 1
}

validator_health_status() {
  local rpc_port=$1
  local response
  response=$(curl -s -X POST "http://127.0.0.1:${rpc_port}" \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}' 2>/dev/null || true)
  echo "$response" | python3 -c '
import json
import sys

try:
    result = json.load(sys.stdin).get("result", {})
    if isinstance(result, dict):
        print(result.get("status", "unknown"))
    else:
        print(result)
except Exception:
    print("unreachable")
'
}

count_staked_validators() {
  local rpc_port=$1
  local response
  response=$(curl -s -X POST "http://127.0.0.1:${rpc_port}" \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getValidators","params":[]}' 2>/dev/null || true)
  echo "$response" | python3 -c '
import json
import sys

try:
    result = json.load(sys.stdin).get("result", {})
    validators = result.get("validators", []) if isinstance(result, dict) else []
    print(sum(1 for validator in validators if validator.get("stake", 0) > 0))
except Exception:
    print(0)
'
}

wait_for_http_health() {
  local url=$1
  local label=$2
  local timeout_seconds=${3:-60}

  for _ in $(seq 1 "$timeout_seconds"); do
    local body
    body=$(curl -sf --max-time 2 "$url" 2>/dev/null || true)
    if echo "$body" | grep -Eq 'OK|"status"[[:space:]]*:[[:space:]]*"ok"'; then
      return 0
    fi
    sleep 1
  done

  echo "❌ Timed out waiting for ${label} health at ${url}" >&2
  return 1
}

wait_for_validator_cluster_ready() {
  local timeout_seconds=${1:-90}
  local expected_validators=${#RPC_CANDIDATES[@]}
  local primary_rpc=${RPC_CANDIDATES[0]}

  for second in $(seq 1 "$timeout_seconds"); do
    local all_healthy=true
    local statuses=()
    for rpc_port in "${RPC_CANDIDATES[@]}"; do
      local status
      status=$(validator_health_status "$rpc_port")
      statuses+=("${rpc_port}:${status}")
      if [ "$status" != "ok" ]; then
        all_healthy=false
      fi
    done

    local staked
    staked=$(count_staked_validators "$primary_rpc")
    if $all_healthy && [ "$staked" -ge "$expected_validators" ]; then
      return 0
    fi

    if [ $((second % 5)) -eq 0 ]; then
      echo "⏳ Waiting for validator cluster readiness... ${statuses[*]} staked=${staked}/${expected_validators}"
    fi
    sleep 1
  done

  echo "❌ Timed out waiting for the full local validator cluster to become healthy" >&2
  return 1
}

if [ "$LOCAL_CLUSTER_RESET" = "1" ]; then
  LOCAL_CLUSTER_BOOTSTRAP_CMD="start-reset-seed"
else
  LOCAL_CLUSTER_BOOTSTRAP_CMD="start-seed"
fi

echo "🦞 Starting seed validator for local production-parity stack..."
if ! LICN_LOCAL_NETWORK="$NETWORK" "$LOCAL_CLUSTER_SCRIPT" "$LOCAL_CLUSTER_BOOTSTRAP_CMD"; then
  cleanup_started_processes
  exit 1
fi

if ! wait_for_file "$GENESIS_TREASURY_KEYPAIR" "genesis treasury keypair"; then
  cleanup_started_processes
  exit 1
fi
if ! wait_for_file "$GENESIS_PRIMARY_KEYPAIR" "genesis primary keypair"; then
  cleanup_started_processes
  exit 1
fi

mkdir -p ./keypairs
install -m 600 "$GENESIS_PRIMARY_KEYPAIR" "$LOCAL_DEPLOYER_KEYPAIR"
export CUSTODY_TREASURY_KEYPAIR="${CUSTODY_TREASURY_KEYPAIR:-$LOCAL_DEPLOYER_KEYPAIR}"

if ! CLUSTER_RPC_URL="$(wait_for_healthy_rpc "$LOCAL_HEALTH_TIMEOUT_SECS")"; then
  cleanup_started_processes
  exit 1
fi
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
# Wait 5s for validators to stabilize, then rebuild manifest + signed metadata.
# The local stack is not DEX-ready until this completes.
echo "🔧 Running post-genesis bootstrap..."
sleep 5
if "${SCRIPT_DIR}/first-boot-deploy.sh" --rpc "$CLUSTER_RPC_URL" --skip-build >"${LOG_DIR}/first-boot-deploy.log" 2>&1; then
  echo "✅ Post-genesis bootstrap complete"
else
  echo "❌ Post-genesis bootstrap failed; see ${LOG_DIR}/first-boot-deploy.log" >&2
  cleanup_started_processes
  exit 1
fi

echo "🦞 Provisioning joiner validators from the seed snapshot..."
if ! LICN_LOCAL_NETWORK="$NETWORK" "$LOCAL_CLUSTER_SCRIPT" start-joiners-from-seed-snapshot; then
  cleanup_started_processes
  exit 1
fi

if ! wait_for_validator_cluster_ready "$LOCAL_HEALTH_TIMEOUT_SECS"; then
  cleanup_started_processes
  exit 1
fi

if ! wait_for_http_health "http://127.0.0.1:${CUSTODY_PORT}/health" "custody" "$LOCAL_HEALTH_TIMEOUT_SECS"; then
  cleanup_started_processes
  exit 1
fi

if [ -n "$FAUCET_PID" ]; then
  if ! wait_for_http_health "http://127.0.0.1:${FAUCET_PORT}/health" "faucet" "$LOCAL_HEALTH_TIMEOUT_SECS"; then
    cleanup_started_processes
    exit 1
  fi
fi

echo "🦞 Lichen local stack started"
echo "Network: $NETWORK"
echo "Cluster RPC: $CLUSTER_RPC_URL"
echo "Validator launcher: $LOCAL_CLUSTER_SCRIPT start"
echo "Custody PID: $CUSTODY_PID"
if [ -n "$FAUCET_PID" ]; then
  echo "Faucet PID: $FAUCET_PID (port $FAUCET_PORT)"
fi
if [ -n "$SOLANA_RPC_URL" ]; then
  echo "Solana RPC: $SOLANA_RPC_URL"
fi
if [ -n "$EVM_RPC_URL" ]; then
  echo "EVM RPC: $EVM_RPC_URL"
fi
echo "Logs: $LOG_DIR"
