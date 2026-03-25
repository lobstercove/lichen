#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
BIN="$ROOT/target/release/lichen-validator"
LAUNCHER="$ROOT/run-validator.sh"

FORCE_MANAGED_MATRIX_CLUSTER="${FORCE_MANAGED_MATRIX_CLUSTER:-0}"
RESET_MATRIX_STATE="${RESET_MATRIX_STATE:-0}"
MATRIX_BUILD_FIRST="${MATRIX_BUILD_FIRST:-0}"
MATRIX_STAGGER_SECS="${MATRIX_STAGGER_SECS:-15}"
MATRIX_MIN_VALIDATORS="${MATRIX_MIN_VALIDATORS:-3}"

DATA1="$ROOT/data/state-7001"
DATA2="$ROOT/data/state-7002"
DATA3="$ROOT/data/state-7003"
LEGACY_MATRIX_DATA_GLOB="$ROOT/data/matrix-sdk-state-*"

ARTIFACT_DIR="$ROOT/tests/artifacts/full_matrix_feb24_2026/sdk_cluster"
PID_FILE="$ARTIFACT_DIR/pids.txt"
MODE_FILE="$ARTIFACT_DIR/mode.txt"
LOG1="$ARTIFACT_DIR/v1.log"
LOG2="$ARTIFACT_DIR/v2.log"
LOG3="$ARTIFACT_DIR/v3.log"

mkdir -p "$ARTIFACT_DIR"

kill_stale_validator_supervisors() {
  pkill -f 'scripts/validator-supervisor\.sh .*run-validator\.sh testnet 1' 2>/dev/null || true
  pkill -f 'scripts/validator-supervisor\.sh .*run-validator\.sh testnet 2' 2>/dev/null || true
  pkill -f 'scripts/validator-supervisor\.sh .*run-validator\.sh testnet 3' 2>/dev/null || true
  pkill -f 'scripts/validator-supervisor\.sh .*run-validator\.sh mainnet 1' 2>/dev/null || true
  pkill -f 'scripts/validator-supervisor\.sh .*run-validator\.sh mainnet 2' 2>/dev/null || true
  pkill -f 'scripts/validator-supervisor\.sh .*run-validator\.sh mainnet 3' 2>/dev/null || true
  pkill -f 'target/(release|debug)/lichen-validator .*--p2p-port (7001|7002|7003|8001|8002|8003)' 2>/dev/null || true
}

rpc_ok() {
  local port="$1"
  curl -sf --connect-timeout 1 --max-time 2 "http://127.0.0.1:${port}" \
    -X POST -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getSlot"}' >/dev/null 2>&1
}

validator_count() {
  curl -sf --connect-timeout 1 --max-time 2 "http://127.0.0.1:8899" \
    -X POST -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getValidators","params":[]}' \
    | python3 -c 'import sys,json; d=json.load(sys.stdin).get("result",[]); v=d.get("validators", d) if isinstance(d, dict) else d; print(len(v) if isinstance(v, list) else 0)' 2>/dev/null \
    || echo 0
}

wait_cluster_ready() {
  local attempts="${1:-60}"
  local delay="${2:-1}"
  for _ in $(seq 1 "$attempts"); do
    if rpc_ok 8899; then
      local vcount
      vcount="$(validator_count)"
      if [[ "$vcount" -ge "$MATRIX_MIN_VALIDATORS" ]]; then
        return 0
      fi
    fi
    sleep "$delay"
  done
  return 1
}

wait_rpc() {
  local port="$1"
  local attempts="${2:-40}"
  local delay="${3:-1}"
  for _ in $(seq 1 "$attempts"); do
    if rpc_ok "$port"; then
      return 0
    fi
    sleep "$delay"
  done
  return 1
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

read_pids() {
  if [[ -f "$PID_FILE" ]]; then
    cat "$PID_FILE"
    return 0
  fi
  return 1
}

stop_cluster() {
  local mode="managed"
  if [[ -f "$MODE_FILE" ]]; then
    mode="$(cat "$MODE_FILE" 2>/dev/null || echo managed)"
  fi
  if [[ "$mode" == "external" ]]; then
    rm -f "$PID_FILE" "$MODE_FILE"
    return 0
  fi

  if read_pids >/dev/null 2>&1; then
    local pids
    pids="$(read_pids || true)"
    for pid in $pids; do
      kill "$pid" 2>/dev/null || true
    done
    sleep 1
    for pid in $pids; do
      kill -9 "$pid" 2>/dev/null || true
    done
    rm -f "$PID_FILE"
  fi

  kill_stale_validator_supervisors
  for port in 8899 8901 8903 8000 8001 8002 9201 9202 9203; do
    kill_port_listener "$port"
  done

  rm -f "$MODE_FILE"
}

purge_legacy_matrix_dirs() {
  shopt -s nullglob
  local dirs=($LEGACY_MATRIX_DATA_GLOB)
  shopt -u nullglob
  if [[ ${#dirs[@]} -gt 0 ]]; then
    echo "[matrix-sdk-cluster] purging legacy matrix dirs: ${dirs[*]}"
    rm -rf "${dirs[@]}"
  fi
}

managed_cluster_healthy() {
  if [[ ! -f "$PID_FILE" ]]; then
    return 1
  fi

  local mode="managed"
  if [[ -f "$MODE_FILE" ]]; then
    mode="$(cat "$MODE_FILE" 2>/dev/null || echo managed)"
  fi
  if [[ "$mode" == "external" ]]; then
    return 1
  fi

  local pids alive
  pids="$(read_pids || true)"
  alive=0
  for pid in $pids; do
    if kill -0 "$pid" 2>/dev/null; then
      alive=$((alive + 1))
    fi
  done

  if [[ "$alive" -ne 3 ]]; then
    return 1
  fi

  if ! rpc_ok 8899 || ! rpc_ok 8901 || ! rpc_ok 8903; then
    return 1
  fi

  local vcount
  vcount="$(validator_count)"
  [[ "$vcount" -ge "$MATRIX_MIN_VALIDATORS" ]]
}

start_cluster() {
  purge_legacy_matrix_dirs

  if [[ "$FORCE_MANAGED_MATRIX_CLUSTER" != "1" && "$RESET_MATRIX_STATE" != "1" ]] && managed_cluster_healthy; then
    echo "[matrix-sdk-cluster] reusing existing managed cluster (already healthy)"
    return 0
  fi

  if [[ "$FORCE_MANAGED_MATRIX_CLUSTER" != "1" ]] && wait_cluster_ready 20 1; then
    echo "external" > "$MODE_FILE"
    echo "[matrix-sdk-cluster] reusing existing external cluster on :8899 (validators>=$MATRIX_MIN_VALIDATORS)"
    return 0
  fi

  stop_cluster

  if [[ "$FORCE_MANAGED_MATRIX_CLUSTER" == "1" ]]; then
    echo "[matrix-sdk-cluster] forcing managed mode (evict/rebind :8899/:8901/:8903)"
    kill_stale_validator_supervisors
    kill_port_listener 8899
    kill_port_listener 8901
    kill_port_listener 8903
    kill_port_listener 8000
    kill_port_listener 8001
    kill_port_listener 8002
    kill_port_listener 8004
    kill_port_listener 9201
    kill_port_listener 9202
    kill_port_listener 9203
  fi

  if [[ "$RESET_MATRIX_STATE" == "1" ]]; then
    echo "[matrix-sdk-cluster] resetting canonical testnet state via reset-blockchain.sh"
    bash "$ROOT/reset-blockchain.sh" testnet >/dev/null
  fi

  if [[ "$MATRIX_BUILD_FIRST" == "1" || ! -x "$BIN" ]]; then
    echo "[matrix-sdk-cluster] building validator binary..."
    cargo build --release --bin lichen-validator >/dev/null
  fi

  if [[ ! -x "$LAUNCHER" ]]; then
    echo "[matrix-sdk-cluster] ERROR: launcher not found or not executable: $LAUNCHER"
    exit 1
  fi

  echo "[matrix-sdk-cluster] starting V1 (production path: run-validator.sh testnet 1)"
  LICHEN_SIGNER_BIND=0.0.0.0:9301 RUST_LOG=${MATRIX_RUST_LOG:-warn} "$LAUNCHER" testnet 1 --dev-mode >"$LOG1" 2>&1 &
  V1PID=$!
  sleep "$MATRIX_STAGGER_SECS"

  echo "[matrix-sdk-cluster] starting V2 (production path: run-validator.sh testnet 2)"
  LICHEN_SIGNER_BIND=0.0.0.0:9302 RUST_LOG=warn "$LAUNCHER" testnet 2 --dev-mode >"$LOG2" 2>&1 &
  V2PID=$!
  sleep "$MATRIX_STAGGER_SECS"

  echo "[matrix-sdk-cluster] starting V3 (production path: run-validator.sh testnet 3)"
  LICHEN_SIGNER_BIND=0.0.0.0:9303 RUST_LOG=warn "$LAUNCHER" testnet 3 --dev-mode >"$LOG3" 2>&1 &
  V3PID=$!

  echo "$V1PID $V2PID $V3PID" > "$PID_FILE"
  echo "managed" > "$MODE_FILE"

  if ! wait_rpc 8899 60 1; then
    echo "[matrix-sdk-cluster] ERROR: 8899 did not become healthy"
    stop_cluster
    exit 1
  fi
  if ! wait_rpc 8901 60 1; then
    echo "[matrix-sdk-cluster] ERROR: 8901 did not become healthy"
    stop_cluster
    exit 1
  fi
  if ! wait_rpc 8903 60 1; then
    echo "[matrix-sdk-cluster] ERROR: 8903 did not become healthy"
    stop_cluster
    exit 1
  fi

  if ! wait_cluster_ready 90 1; then
    if rpc_ok 8899 && rpc_ok 8901 && rpc_ok 8903; then
      echo "[matrix-sdk-cluster] WARN: quorum check timed out, but all validator RPC endpoints are healthy; continuing"
    else
      echo "[matrix-sdk-cluster] ERROR: cluster did not reach validator quorum >= $MATRIX_MIN_VALIDATORS"
      stop_cluster
      exit 1
    fi
  fi

  echo "[matrix-sdk-cluster] ready pids=$V1PID,$V2PID,$V3PID"
}

status_cluster() {
  if ! read_pids >/dev/null 2>&1; then
    if rpc_ok 8899; then
      local vcount
      vcount="$(validator_count)"
      if [[ "$vcount" -ge "$MATRIX_MIN_VALIDATORS" ]]; then
        echo "[matrix-sdk-cluster] status=up mode=external validators=$vcount"
        return 0
      fi
      echo "[matrix-sdk-cluster] status=degraded mode=external validators=$vcount/<$MATRIX_MIN_VALIDATORS"
      return 1
    fi
    echo "[matrix-sdk-cluster] status=down (no pid file)"
    return 1
  fi
  local mode="managed"
  if [[ -f "$MODE_FILE" ]]; then
    mode="$(cat "$MODE_FILE" 2>/dev/null || echo managed)"
  fi
  if [[ "$mode" == "external" ]]; then
    if rpc_ok 8899; then
      local vcount
      vcount="$(validator_count)"
      if [[ "$vcount" -ge "$MATRIX_MIN_VALIDATORS" ]]; then
        echo "[matrix-sdk-cluster] status=up mode=external validators=$vcount"
        return 0
      fi
      echo "[matrix-sdk-cluster] status=degraded mode=external validators=$vcount/<$MATRIX_MIN_VALIDATORS"
      return 1
    fi
    echo "[matrix-sdk-cluster] status=down mode=external"
    return 1
  fi
  local pids
  pids="$(read_pids || true)"
  local alive=0
  for pid in $pids; do
    if kill -0 "$pid" 2>/dev/null; then
      alive=$((alive + 1))
    fi
  done

  local vcount
  vcount="$(validator_count)"
  if [[ "$alive" -eq 3 ]] && rpc_ok 8899 && [[ "$vcount" -ge "$MATRIX_MIN_VALIDATORS" ]]; then
    echo "[matrix-sdk-cluster] status=up pids=$pids validators=$vcount"
    return 0
  fi

  echo "[matrix-sdk-cluster] status=degraded alive=$alive pids=$pids validators=$vcount/<$MATRIX_MIN_VALIDATORS"
  return 1
}

cmd="${1:-status}"
case "$cmd" in
  start) start_cluster ;;
  stop) stop_cluster ; echo "[matrix-sdk-cluster] stopped" ;;
  status) status_cluster ;;
  restart) stop_cluster; start_cluster ;;
  *)
    echo "usage: $0 {start|stop|status|restart}"
    exit 2
    ;;
esac
