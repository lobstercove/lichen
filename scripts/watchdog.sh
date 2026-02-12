#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────
#  MoltChain Validator Watchdog — auto-restart stale or crashed nodes
# ─────────────────────────────────────────────────────────────────────
#
# Usage:
#   ./scripts/watchdog.sh                     # use defaults (config below)
#   ./scripts/watchdog.sh --config validators.json
#
# The watchdog polls each validator's RPC every CHECK_INTERVAL seconds.
# A validator is considered STALE if:
#   1. Its RPC is unreachable (process crashed), OR
#   2. Its slot hasn't advanced for STALE_THRESHOLD consecutive checks.
#
# When a stale validator is detected the watchdog:
#   1. Kills the old process (if still running)
#   2. Restarts it with the original launch command
#   3. Logs the event to WATCHDOG_LOG
#
# Configuration can be provided via:
#   a) Environment variables (see defaults below)
#   b) A JSON config file (--config flag)
#   c) Inline VALIDATORS array below
# ─────────────────────────────────────────────────────────────────────

set -euo pipefail

# ── Defaults ─────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
BINARY="${PROJECT_DIR}/target/release/moltchain-validator"

CHECK_INTERVAL="${WATCHDOG_INTERVAL:-15}"       # seconds between checks
STALE_THRESHOLD="${WATCHDOG_STALE:-5}"          # consecutive stale checks before restart
MAX_RESTARTS="${WATCHDOG_MAX_RESTARTS:-10}"      # max restarts per validator before giving up
WATCHDOG_LOG="${WATCHDOG_LOG:-/tmp/moltchain-watchdog.log}"

# ── Validator definitions ────────────────────────────────────────────
# Each entry: "name|rpc_port|p2p_port|extra_args|log_file"
# Override by exporting WATCHDOG_VALIDATORS as a newline-separated string,
# or pass --config <json_file>.

DEFAULT_VALIDATORS=(
  "val1|8899|8000||/tmp/val1.log"
  "val2|8901|8001|--bootstrap-peers 127.0.0.1:8000|/tmp/val2.log"
  "val3|8902|8002|--bootstrap 127.0.0.1:8000 --bootstrap 127.0.0.1:8001|/tmp/val3.log"
)

# ── Parse arguments ──────────────────────────────────────────────────
CONFIG_FILE=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --config) CONFIG_FILE="$2"; shift 2 ;;
    --interval) CHECK_INTERVAL="$2"; shift 2 ;;
    --threshold) STALE_THRESHOLD="$2"; shift 2 ;;
    --help|-h)
      echo "Usage: $0 [--config validators.json] [--interval N] [--threshold N]"
      echo ""
      echo "Options:"
      echo "  --config FILE    JSON config with validator definitions"
      echo "  --interval N     Seconds between health checks (default: 15)"
      echo "  --threshold N    Stale checks before restart (default: 5)"
      echo ""
      echo "Environment:"
      echo "  WATCHDOG_INTERVAL       Same as --interval"
      echo "  WATCHDOG_STALE          Same as --threshold"
      echo "  WATCHDOG_MAX_RESTARTS   Max restarts per validator (default: 10)"
      echo "  WATCHDOG_LOG            Log file (default: /tmp/moltchain-watchdog.log)"
      exit 0
      ;;
    *) echo "Unknown arg: $1"; exit 1 ;;
  esac
done

# ── Load validators from JSON config if provided ────────────────────
declare -a VALIDATORS
if [[ -n "$CONFIG_FILE" && -f "$CONFIG_FILE" ]]; then
  mapfile -t VALIDATORS < <(python3 -c "
import json, sys
with open('$CONFIG_FILE') as f:
    cfg = json.load(f)
for v in cfg.get('validators', []):
    extras = v.get('extra_args', '')
    log = v.get('log_file', '/tmp/' + v['name'] + '.log')
    print(f\"{v['name']}|{v['rpc_port']}|{v['p2p_port']}|{extras}|{log}\")
")
  echo "Loaded ${#VALIDATORS[@]} validators from $CONFIG_FILE"
else
  VALIDATORS=("${DEFAULT_VALIDATORS[@]}")
fi

# ── State tracking (parallel arrays) ────────────────────────────────
declare -a LAST_SLOTS      # last known slot per validator
declare -a STALE_COUNTS    # consecutive stale checks
declare -a RESTART_COUNTS  # total restarts
declare -a PIDS            # current PID (0 = unknown/not managed)

for i in "${!VALIDATORS[@]}"; do
  LAST_SLOTS[$i]=0
  STALE_COUNTS[$i]=0
  RESTART_COUNTS[$i]=0
  PIDS[$i]=0
done

# ── Logging ──────────────────────────────────────────────────────────
log() {
  local ts
  ts="$(date '+%Y-%m-%d %H:%M:%S')"
  echo "[$ts] $*" | tee -a "$WATCHDOG_LOG"
}

# ── Helper: get slot from RPC ────────────────────────────────────────
get_slot() {
  local port="$1"
  local result
  result=$(curl -sf --max-time 5 \
    http://127.0.0.1:"$port" \
    -X POST \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getSlot"}' 2>/dev/null) || { echo "UNREACHABLE"; return; }
  
  echo "$result" | python3 -c "import sys,json; print(json.load(sys.stdin).get('result', 'ERROR'))" 2>/dev/null || echo "ERROR"
}

# ── Helper: find PID by p2p port ─────────────────────────────────────
find_pid() {
  local p2p_port="$1"
  ps aux | grep "moltchain-validator" | grep -v grep | grep "\-\-p2p-port $p2p_port\b" | awk '{print $2}' | head -1
}

# ── Helper: start a validator ────────────────────────────────────────
start_validator() {
  local idx="$1"
  IFS='|' read -r name rpc_port p2p_port extra_args log_file <<< "${VALIDATORS[$idx]}"

  log "🚀 Starting $name (RPC:$rpc_port, P2P:$p2p_port)"

  # Build command
  local cmd="MOLTCHAIN_SIGNER_BIND=off $BINARY --p2p-port $p2p_port --rpc-port $rpc_port --network testnet"
  if [[ -n "$extra_args" ]]; then
    cmd="$cmd $extra_args"
  fi

  # Launch in background
  eval "$cmd" >> "$log_file" 2>&1 &
  local new_pid=$!
  PIDS[$idx]=$new_pid

  log "✅ $name started (PID: $new_pid, log: $log_file)"
}

# ── Helper: kill a validator ─────────────────────────────────────────
kill_validator() {
  local idx="$1"
  IFS='|' read -r name rpc_port p2p_port extra_args log_file <<< "${VALIDATORS[$idx]}"
  
  # Try to find the actual PID
  local pid
  pid=$(find_pid "$p2p_port")
  
  if [[ -n "$pid" ]]; then
    log "🔪 Killing $name (PID: $pid)"
    kill "$pid" 2>/dev/null || true
    sleep 2
    # Force kill if still alive
    if kill -0 "$pid" 2>/dev/null; then
      kill -9 "$pid" 2>/dev/null || true
      log "⚠️  Force-killed $name (PID: $pid)"
    fi
  else
    log "ℹ️  No running process found for $name"
  fi
  PIDS[$idx]=0
}

# ── Helper: restart a validator ──────────────────────────────────────
restart_validator() {
  local idx="$1"
  IFS='|' read -r name rpc_port p2p_port extra_args log_file <<< "${VALIDATORS[$idx]}"

  RESTART_COUNTS[$idx]=$(( ${RESTART_COUNTS[$idx]} + 1 ))
  local count=${RESTART_COUNTS[$idx]}

  if (( count > MAX_RESTARTS )); then
    log "❌ $name exceeded max restarts ($MAX_RESTARTS) — GIVING UP"
    return
  fi

  log "🔄 Restarting $name (attempt $count/$MAX_RESTARTS)"
  kill_validator "$idx"
  sleep 3
  start_validator "$idx"
  STALE_COUNTS[$idx]=0
  LAST_SLOTS[$idx]=0
}

# ── Discover existing PIDs ──────────────────────────────────────────
discover_existing() {
  for i in "${!VALIDATORS[@]}"; do
    IFS='|' read -r name rpc_port p2p_port extra_args log_file <<< "${VALIDATORS[$i]}"
    local pid
    pid=$(find_pid "$p2p_port")
    if [[ -n "$pid" ]]; then
      PIDS[$i]=$pid
      log "📍 Discovered $name already running (PID: $pid)"
    fi
  done
}

# ── Signal handling ──────────────────────────────────────────────────
cleanup() {
  log "🛑 Watchdog shutting down"
  exit 0
}
trap cleanup SIGINT SIGTERM

# ── Main loop ────────────────────────────────────────────────────────
log "════════════════════════════════════════════════════════════════"
log "🐺 MoltChain Watchdog started"
log "   Validators: ${#VALIDATORS[@]}"
log "   Check interval: ${CHECK_INTERVAL}s"
log "   Stale threshold: ${STALE_THRESHOLD} checks ($(( CHECK_INTERVAL * STALE_THRESHOLD ))s)"
log "   Max restarts: ${MAX_RESTARTS}"
log "   Log: ${WATCHDOG_LOG}"
log "════════════════════════════════════════════════════════════════"

# Pick up already-running validators
discover_existing

# Start any validators that aren't already running
for i in "${!VALIDATORS[@]}"; do
  IFS='|' read -r name rpc_port p2p_port extra_args log_file <<< "${VALIDATORS[$i]}"
  if [[ "${PIDS[$i]}" == "0" ]]; then
    local_pid=$(find_pid "$p2p_port")
    if [[ -z "$local_pid" ]]; then
      log "⚠️  $name not running — starting it"
      start_validator "$i"
      sleep 3  # stagger starts
    fi
  fi
done

while true; do
  sleep "$CHECK_INTERVAL"

  for i in "${!VALIDATORS[@]}"; do
    IFS='|' read -r name rpc_port p2p_port extra_args log_file <<< "${VALIDATORS[$i]}"

    # Skip if we've given up on this validator
    if (( ${RESTART_COUNTS[$i]} > MAX_RESTARTS )); then
      continue
    fi

    # Check health
    local_slot=$(get_slot "$rpc_port")

    if [[ "$local_slot" == "UNREACHABLE" || "$local_slot" == "ERROR" ]]; then
      STALE_COUNTS[$i]=$(( ${STALE_COUNTS[$i]} + 1 ))
      log "⚠️  $name RPC unreachable (stale: ${STALE_COUNTS[$i]}/$STALE_THRESHOLD)"
    elif [[ "$local_slot" == "${LAST_SLOTS[$i]}" ]]; then
      STALE_COUNTS[$i]=$(( ${STALE_COUNTS[$i]} + 1 ))
      log "⚠️  $name stuck at slot $local_slot (stale: ${STALE_COUNTS[$i]}/$STALE_THRESHOLD)"
    else
      # Healthy — reset stale counter
      if (( ${STALE_COUNTS[$i]} > 0 )); then
        log "✅ $name recovered (slot: $local_slot)"
      fi
      STALE_COUNTS[$i]=0
      LAST_SLOTS[$i]="$local_slot"
    fi

    # Restart if stale threshold exceeded
    if (( ${STALE_COUNTS[$i]} >= STALE_THRESHOLD )); then
      log "🚨 $name STALE for ${STALE_COUNTS[$i]} checks — triggering restart"
      restart_validator "$i"
    fi
  done
done
