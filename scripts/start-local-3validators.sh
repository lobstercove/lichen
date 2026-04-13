#!/usr/bin/env bash
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

cd "$(dirname "$0")/.."
ROOT="$PWD"
RUNNER="$ROOT/run-validator.sh"
ART_DIR="$ROOT/tests/artifacts/local_cluster"
PID_FILE="$ART_DIR/pids.txt"
LOG1="$ART_DIR/v1.log"
LOG2="$ART_DIR/v2.log"
LOG3="$ART_DIR/v3.log"
LOCAL_CUSTODY_TOKEN_FILE="$ART_DIR/custody-api-auth-token"
STAGGER_SECS="${LICN_LOCAL_STAGGER_SECS:-15}"
NETWORK="${LICN_LOCAL_NETWORK:-testnet}"
MANIFEST_FILE="$ROOT/signed-metadata-manifest-${NETWORK}.json"
SIGNED_METADATA_KEYPAIR="${SIGNED_METADATA_KEYPAIR:-$ROOT/keypairs/release-signing-key.json}"
RPC_WAIT_SECS="${LICN_LOCAL_RPC_WAIT_SECS:-900}"
STATE1_DIR=""
STATE2_DIR=""
STATE3_DIR=""

export LICHEN_LOCAL_DEV=1

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
    "$ROOT/Cargo.toml"
    "$ROOT/Cargo.lock"
    "$ROOT/core"
    "$ROOT/validator"
    "$ROOT/rpc"
    "$ROOT/p2p"
    "$ROOT/cli"
    "$ROOT/genesis"
  )

  if any_path_newer_than "$ROOT/target/release/lichen" "${runtime_sources[@]}" \
    || any_path_newer_than "$ROOT/target/release/lichen-genesis" "${runtime_sources[@]}" \
    || any_path_newer_than "$ROOT/target/release/lichen-validator" "${runtime_sources[@]}"; then
    echo "[local-3validators] rebuilding required release binaries"
    cargo build --release --bin lichen --bin lichen-genesis --bin lichen-validator
  fi
}

refresh_changed_contract_wasm() {
  local contract_dir manifest contract_name root_wasm target_wasm

  for contract_dir in "$ROOT"/contracts/*; do
    [ -d "$contract_dir" ] || continue

    manifest="$contract_dir/Cargo.toml"
    [ -f "$manifest" ] || continue

    contract_name=$(basename "$contract_dir")
    root_wasm="$contract_dir/${contract_name}.wasm"
    target_wasm="$contract_dir/target/wasm32-unknown-unknown/release/${contract_name}.wasm"

    if any_path_newer_than "$root_wasm" "$manifest" "$contract_dir/Cargo.lock" "$contract_dir/src"; then
      echo "[local-3validators] refreshing ${contract_name}.wasm"
      cargo build --manifest-path "$manifest" --target wasm32-unknown-unknown --release
      cp "$target_wasm" "$root_wasm"
    fi
  done
}

case "$NETWORK" in
  testnet)
    RPC1=8899; RPC2=8901; RPC3=8903
    WS1=8900; WS2=8902; WS3=8904
    P2P1=7001; P2P2=7002; P2P3=7003
    SIGNED_METADATA_NETWORK="local-testnet"
    ;;
  mainnet)
    RPC1=9899; RPC2=9901; RPC3=9903
    WS1=9900; WS2=9902; WS3=9904
    P2P1=8001; P2P2=8002; P2P3=8003
    SIGNED_METADATA_NETWORK="local-mainnet"
    ;;
  *)
    echo "[local-3validators] ERROR: unsupported network '$NETWORK' (expected testnet|mainnet)"
    exit 1
    ;;
esac

STATE1_DIR="$ROOT/data/state-${P2P1}"
STATE2_DIR="$ROOT/data/state-${P2P2}"
STATE3_DIR="$ROOT/data/state-${P2P3}"

mkdir -p "$ART_DIR"

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

  echo "[local-3validators] ERROR: python3 or openssl is required to generate a local custody auth token" >&2
  return 1
}

prepare_local_bridge_env() {
  local custody_port
  case "$NETWORK" in
    testnet) custody_port=9105 ;;
    mainnet) custody_port=9106 ;;
    *)
      echo "[local-3validators] ERROR: unsupported network '$NETWORK' for local bridge config" >&2
      return 1
      ;;
  esac

  export CUSTODY_URL="${CUSTODY_URL:-http://127.0.0.1:${custody_port}}"

  if [[ -n "${CUSTODY_API_AUTH_TOKEN:-}" ]]; then
    printf '%s' "$CUSTODY_API_AUTH_TOKEN" > "$LOCAL_CUSTODY_TOKEN_FILE"
    chmod 600 "$LOCAL_CUSTODY_TOKEN_FILE" 2>/dev/null || true
    return 0
  fi

  if [[ -f "$LOCAL_CUSTODY_TOKEN_FILE" ]]; then
    export CUSTODY_API_AUTH_TOKEN="$(cat "$LOCAL_CUSTODY_TOKEN_FILE")"
    return 0
  fi

  export CUSTODY_API_AUTH_TOKEN="$(generate_local_token)"
  printf '%s' "$CUSTODY_API_AUTH_TOKEN" > "$LOCAL_CUSTODY_TOKEN_FILE"
  chmod 600 "$LOCAL_CUSTODY_TOKEN_FILE" 2>/dev/null || true
}

clear_local_peer_trust_state() {
  local state_dir
  for state_dir in "$ROOT/data/state-${P2P1}" "$ROOT/data/state-${P2P2}" "$ROOT/data/state-${P2P3}"; do
    rm -f "$state_dir/known-peers.json" 2>/dev/null || true
    rm -f "$state_dir/home/.lichen/peer_identities.json" 2>/dev/null || true
    rm -rf "$state_dir/home/.lichen/validators" 2>/dev/null || true
  done
}

rpc_ok() {
  local port="$1"
  curl -sf "http://127.0.0.1:${port}" \
    -X POST -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getHealth","params":[]}' | grep -q '"status":"ok"'
}

wait_rpc() {
  local port="$1"
  local attempts="${2:-60}"
  local delay="${3:-1}"
  for _ in $(seq 1 "$attempts"); do
    if rpc_ok "$port"; then
      return 0
    fi
    sleep "$delay"
  done
  return 1
}

wait_rpc_down() {
  local port="$1"
  local attempts="${2:-30}"
  local delay="${3:-1}"
  for _ in $(seq 1 "$attempts"); do
    if ! rpc_ok "$port"; then
      return 0
    fi
    sleep "$delay"
  done
  return 1
}

start_validator() {
  local validator_num="$1"
  local signer_bind="$2"
  local log_file="$3"
  local append_mode="${4:-0}"
  local pid

  if [[ "$append_mode" == "1" ]]; then
    LICHEN_SIGNER_BIND="$signer_bind" RUST_LOG=warn "$RUNNER" "$NETWORK" "$validator_num" --dev-mode >>"$log_file" 2>&1 &
  else
    LICHEN_SIGNER_BIND="$signer_bind" RUST_LOG=warn "$RUNNER" "$NETWORK" "$validator_num" --dev-mode >"$log_file" 2>&1 &
  fi
  pid=$!
  echo "$pid"
}

stop_pid() {
  local pid="$1"
  if [[ -z "$pid" ]]; then
    return 0
  fi

  kill "$pid" 2>/dev/null || true
  for _ in $(seq 1 10); do
    if ! kill -0 "$pid" 2>/dev/null; then
      return 0
    fi
    sleep 1
  done

  kill -9 "$pid" 2>/dev/null || true
}

sync_seed_state_to_joiners() {
  local joiner_dir

  if [[ ! -f "$STATE1_DIR/CURRENT" ]]; then
    echo "[local-3validators] ERROR: seed state is not initialized at $STATE1_DIR"
    return 1
  fi

  if ! command -v rsync >/dev/null 2>&1; then
    echo "[local-3validators] ERROR: rsync is required to snapshot seed state to local joiners"
    return 1
  fi

  for joiner_dir in "$STATE2_DIR" "$STATE3_DIR"; do
    mkdir -p "$joiner_dir"
    rsync -a --delete \
      --exclude='validator-keypair.json' \
      --exclude='signer-keypair.json' \
      --exclude='seeds.json' \
      --exclude='LOCK' \
      --exclude='IDENTITY' \
      --exclude='LOG' \
      --exclude='LOG.old.*' \
      --exclude='known-peers.json' \
      --exclude='logs/' \
      --exclude='home/' \
      "$STATE1_DIR/" "$joiner_dir/"
    rm -f "$joiner_dir/known-peers.json" 2>/dev/null || true
    rm -f "$joiner_dir/seeds.json" 2>/dev/null || true
    rm -rf "$joiner_dir/home" 2>/dev/null || true
  done
}

generate_signed_metadata_manifest() {
  if ! command -v node >/dev/null 2>&1; then
    echo "[local-3validators] ERROR: Node.js is required to generate ${MANIFEST_FILE}"
    return 1
  fi

  if [[ ! -f "$SIGNED_METADATA_KEYPAIR" ]]; then
    echo "[local-3validators] ERROR: signing keypair not found at $SIGNED_METADATA_KEYPAIR"
    return 1
  fi

  echo "[local-3validators] generating signed metadata manifest via RPC ${RPC1}"
  node "$ROOT/scripts/generate-signed-metadata-manifest.js" \
    --rpc "http://127.0.0.1:${RPC1}" \
    --network "$SIGNED_METADATA_NETWORK" \
    --keypair "$SIGNED_METADATA_KEYPAIR" \
    --out "$MANIFEST_FILE"
}

kill_port_listener() {
  local port="$1"
  local pids
  pids="$(lsof -ti tcp:"$port" 2>/dev/null || true)"
  if [[ -n "$pids" ]]; then
    for pid in $pids; do
      kill "$pid" 2>/dev/null || true
    done
    sleep 1
    pids="$(lsof -ti tcp:"$port" 2>/dev/null || true)"
    if [[ -n "$pids" ]]; then
      for pid in $pids; do
        kill -9 "$pid" 2>/dev/null || true
      done
    fi
  fi
}

stop_cluster() {
  if [[ -f "$PID_FILE" ]]; then
    for pid in $(cat "$PID_FILE"); do
      kill "$pid" 2>/dev/null || true
    done
    sleep 1
    for pid in $(cat "$PID_FILE"); do
      kill -9 "$pid" 2>/dev/null || true
    done
    rm -f "$PID_FILE"
  fi

  pkill -f "$ROOT/scripts/validator-supervisor.sh ${NETWORK}-v" 2>/dev/null || true
  pkill -f "$ROOT/run-validator.sh ${NETWORK} " 2>/dev/null || true
  pkill -f "lichen-validator.*--network ${NETWORK}.*--p2p-port ${P2P1}" 2>/dev/null || true
  pkill -f "lichen-validator.*--network ${NETWORK}.*--p2p-port ${P2P2}" 2>/dev/null || true
  pkill -f "lichen-validator.*--network ${NETWORK}.*--p2p-port ${P2P3}" 2>/dev/null || true
  sleep 1

  for port in "$RPC1" "$RPC2" "$RPC3" "$WS1" "$WS2" "$WS3" "$P2P1" "$P2P2" "$P2P3" 9301 9302 9303; do
    kill_port_listener "$port"
  done
}

status_cluster() {
  local up=0
  for port in "$RPC1" "$RPC2" "$RPC3"; do
    if rpc_ok "$port"; then
      up=$((up+1))
    fi
  done

  if [[ "$up" -eq 3 ]]; then
    echo "[local-3validators] status=up network=$NETWORK rpc=${RPC1},${RPC2},${RPC3} p2p=${P2P1},${P2P2},${P2P3} data=data/state-{${P2P1},${P2P2},${P2P3}}"
    return 0
  fi

  echo "[local-3validators] status=down reachable_rpc=$up/3"
  return 1
}

start_cluster() {
  local reset="${1:-0}"

  if [[ ! -x "$RUNNER" ]]; then
    echo "[local-3validators] ERROR: run-validator.sh not executable at $RUNNER"
    exit 1
  fi

  ensure_runtime_binaries
  refresh_changed_contract_wasm

  stop_cluster

  prepare_local_bridge_env

  if [[ "$reset" == "1" ]]; then
    bash "$ROOT/reset-blockchain.sh" "$NETWORK" >/dev/null
  fi

  clear_local_peer_trust_state

  echo "[local-3validators] starting V1 via run-validator.sh ($NETWORK)"
  V1PID="$(start_validator 1 127.0.0.1:9301 "$LOG1")"
  sleep "$STAGGER_SECS"

  echo "[local-3validators] starting V2 via run-validator.sh ($NETWORK)"
  V2PID="$(start_validator 2 127.0.0.1:9302 "$LOG2")"
  sleep "$STAGGER_SECS"

  echo "[local-3validators] starting V3 via run-validator.sh ($NETWORK)"
  V3PID="$(start_validator 3 127.0.0.1:9303 "$LOG3")"

  if ! wait_rpc "$RPC1" "$RPC_WAIT_SECS" 1 || ! wait_rpc "$RPC2" "$RPC_WAIT_SECS" 1 || ! wait_rpc "$RPC3" "$RPC_WAIT_SECS" 1; then
    echo "[local-3validators] ERROR: cluster did not become healthy"
    stop_cluster
    exit 1
  fi

  if ! generate_signed_metadata_manifest; then
    echo "[local-3validators] ERROR: failed to prepare signed metadata manifest"
    stop_cluster
    exit 1
  fi

  echo "$V1PID $V2PID $V3PID" > "$PID_FILE"
  echo "[local-3validators] ready pids=$V1PID,$V2PID,$V3PID"
}

start_seed_only() {
  local reset="${1:-0}"
  local v1pid

  if [[ ! -x "$RUNNER" ]]; then
    echo "[local-3validators] ERROR: run-validator.sh not executable at $RUNNER"
    exit 1
  fi

  ensure_runtime_binaries
  refresh_changed_contract_wasm

  stop_cluster

  prepare_local_bridge_env

  if [[ "$reset" == "1" ]]; then
    bash "$ROOT/reset-blockchain.sh" "$NETWORK" >/dev/null
  fi

  clear_local_peer_trust_state

  echo "[local-3validators] starting seed validator V1 via run-validator.sh ($NETWORK)"
  v1pid="$(start_validator 1 127.0.0.1:9301 "$LOG1")"

  if ! wait_rpc "$RPC1" "$RPC_WAIT_SECS" 1; then
    echo "[local-3validators] ERROR: seed validator did not become healthy"
    stop_cluster
    exit 1
  fi

  echo "$v1pid" > "$PID_FILE"
  echo "[local-3validators] seed-ready pid=$v1pid rpc=$RPC1"
}

promote_joiners_from_seed_snapshot() {
  local existing_pids v1pid v1restart_pid v2pid v3pid

  if [[ ! -f "$PID_FILE" ]]; then
    echo "[local-3validators] ERROR: seed validator is not running; start it first"
    exit 1
  fi

  existing_pids="$(cat "$PID_FILE")"
  v1pid="${existing_pids%% *}"

  if ! rpc_ok "$RPC1"; then
    echo "[local-3validators] ERROR: seed validator RPC is not healthy on $RPC1"
    exit 1
  fi

  echo "[local-3validators] stopping V1 for a clean local seed snapshot"
  stop_pid "$v1pid"
  if ! wait_rpc_down "$RPC1" 30 1; then
    echo "[local-3validators] ERROR: seed validator did not stop cleanly"
    stop_cluster
    exit 1
  fi

  echo "[local-3validators] syncing seed state into V2/V3 data directories"
  if ! sync_seed_state_to_joiners; then
    stop_cluster
    exit 1
  fi

  echo "[local-3validators] restarting V1 after snapshot"
  v1restart_pid="$(start_validator 1 127.0.0.1:9301 "$LOG1" 1)"
  sleep "$STAGGER_SECS"

  echo "[local-3validators] starting V2 from seed snapshot"
  v2pid="$(start_validator 2 127.0.0.1:9302 "$LOG2")"
  sleep "$STAGGER_SECS"

  echo "[local-3validators] starting V3 from seed snapshot"
  v3pid="$(start_validator 3 127.0.0.1:9303 "$LOG3")"

  if ! wait_rpc "$RPC1" "$RPC_WAIT_SECS" 1 || ! wait_rpc "$RPC2" "$RPC_WAIT_SECS" 1 || ! wait_rpc "$RPC3" "$RPC_WAIT_SECS" 1; then
    echo "[local-3validators] ERROR: snapshot-provisioned cluster did not become healthy"
    stop_cluster
    exit 1
  fi

  if ! generate_signed_metadata_manifest; then
    echo "[local-3validators] ERROR: failed to prepare signed metadata manifest after joiner promotion"
    stop_cluster
    exit 1
  fi

  echo "$v1restart_pid $v2pid $v3pid" > "$PID_FILE"
  echo "[local-3validators] joiners-ready pids=$v1restart_pid,$v2pid,$v3pid"
}

cmd="${1:-status}"
case "$cmd" in
  start)
    start_cluster 0
    ;;
  start-reset)
    start_cluster 1
    ;;
  start-seed)
    start_seed_only 0
    ;;
  start-reset-seed)
    start_seed_only 1
    ;;
  start-joiners-from-seed-snapshot)
    promote_joiners_from_seed_snapshot
    ;;
  stop)
    stop_cluster
    echo "[local-3validators] stopped"
    ;;
  status)
    status_cluster
    ;;
  *)
    echo "usage: $0 {start|start-reset|start-seed|start-reset-seed|start-joiners-from-seed-snapshot|stop|status}"
    exit 2
    ;;
esac
