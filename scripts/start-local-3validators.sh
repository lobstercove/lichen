#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
RUNNER="$ROOT/run-validator.sh"
ART_DIR="$ROOT/tests/artifacts/local_cluster"
PID_FILE="$ART_DIR/pids.txt"
LOG1="$ART_DIR/v1.log"
LOG2="$ART_DIR/v2.log"
LOG3="$ART_DIR/v3.log"
STAGGER_SECS="${LICN_LOCAL_STAGGER_SECS:-15}"
NETWORK="${LICN_LOCAL_NETWORK:-testnet}"

case "$NETWORK" in
  testnet)
    RPC1=8899; RPC2=8901; RPC3=8903
    WS1=8900; WS2=8902; WS3=8904
    P2P1=7001; P2P2=7002; P2P3=7003
    ;;
  mainnet)
    RPC1=9899; RPC2=9901; RPC3=9903
    WS1=9900; WS2=9902; WS3=9904
    P2P1=8001; P2P2=8002; P2P3=8003
    ;;
  *)
    echo "[local-3validators] ERROR: unsupported network '$NETWORK' (expected testnet|mainnet)"
    exit 1
    ;;
esac

mkdir -p "$ART_DIR"

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

  stop_cluster

  if [[ "$reset" == "1" ]]; then
    bash "$ROOT/reset-blockchain.sh" "$NETWORK" >/dev/null
  fi

  echo "[local-3validators] starting V1 via run-validator.sh ($NETWORK)"
  LICHEN_SIGNER_BIND=127.0.0.1:9301 RUST_LOG=warn "$RUNNER" "$NETWORK" 1 --dev-mode >"$LOG1" 2>&1 &
  V1PID=$!
  sleep "$STAGGER_SECS"

  echo "[local-3validators] starting V2 via run-validator.sh ($NETWORK)"
  LICHEN_SIGNER_BIND=127.0.0.1:9302 RUST_LOG=warn "$RUNNER" "$NETWORK" 2 --dev-mode >"$LOG2" 2>&1 &
  V2PID=$!
  sleep "$STAGGER_SECS"

  echo "[local-3validators] starting V3 via run-validator.sh ($NETWORK)"
  LICHEN_SIGNER_BIND=127.0.0.1:9303 RUST_LOG=warn "$RUNNER" "$NETWORK" 3 --dev-mode >"$LOG3" 2>&1 &
  V3PID=$!

  if ! wait_rpc "$RPC1" 90 1 || ! wait_rpc "$RPC2" 90 1 || ! wait_rpc "$RPC3" 90 1; then
    echo "[local-3validators] ERROR: cluster did not become healthy"
    stop_cluster
    exit 1
  fi

  echo "$V1PID $V2PID $V3PID" > "$PID_FILE"
  echo "[local-3validators] ready pids=$V1PID,$V2PID,$V3PID"
}

cmd="${1:-status}"
case "$cmd" in
  start)
    start_cluster 0
    ;;
  start-reset)
    start_cluster 1
    ;;
  stop)
    stop_cluster
    echo "[local-3validators] stopped"
    ;;
  status)
    status_cluster
    ;;
  *)
    echo "usage: $0 {start|start-reset|stop|status}"
    exit 2
    ;;
esac
