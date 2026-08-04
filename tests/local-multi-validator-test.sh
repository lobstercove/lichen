#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════
# Local Multi-Validator Test
# ═══════════════════════════════════════════════════════════════
# Uses run-validator.sh — the SAME script used 2000+ times locally.
#
# Port assignments (from run-validator.sh):
#   V1: p2p=7001  rpc=8899  ws=8900
#   V2: p2p=7002  rpc=8901  ws=8902
#   V3: p2p=7003  rpc=8903  ws=8904
#   V4: p2p=7004  rpc=8905  ws=8906
#
# Data dirs: $REPO_ROOT/data/state-{port}
#
# Usage: bash tests/local-multi-validator-test.sh [max_validators]
#   Default: 4 validators.
# Reuse mode: set LICHEN_REUSE_EXISTING_CLUSTER=1 to validate a healthy
# already-running local cluster without flushing state or killing validators.
# Set LICHEN_KEEP_CLUSTER_ON_SUCCESS=1 to leave a newly verified cluster running
# for follow-on E2E journeys. Failed runs always clean up.
# ═══════════════════════════════════════════════════════════════
set -euo pipefail

# Disable pagers to prevent interactive hangs in CI/automated runs
export PAGER=cat
export GIT_PAGER=cat
export LESS='-FRX'

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MAX_VALIDATORS="${1:-4}"
export LICHEN_LOCAL_VALIDATOR_COUNT="$MAX_VALIDATORS"
WARMUP_SLOTS=100  # Must match ACTIVATION_WARMUP in validator/src/main.rs
REUSE_EXISTING_CLUSTER="${LICHEN_REUSE_EXISTING_CLUSTER:-0}"
REUSE_HEALTH_TIMEOUT_SECS="${LICHEN_REUSE_HEALTH_TIMEOUT_SECS:-120}"
USING_EXISTING_CLUSTER=false
RESUME_AFTER_PHASE2="${LICHEN_RESUME_LOCAL_GATE_AFTER_PHASE2:-0}"
RESUME_AFTER_PUBLIC_PARITY="${LICHEN_RESUME_LOCAL_GATE_AFTER_PUBLIC_PARITY:-0}"
RESUME_PUBLIC_PARITY_CHECKPOINT="${LICHEN_RESUME_LOCAL_GATE_CHECKPOINT_SLOT:-}"
SKIP_JOINER_RESTART_CHECK="${LICHEN_SKIP_JOINER_RESTART_CHECK:-0}"
KEEP_CLUSTER_ON_SUCCESS="${LICHEN_KEEP_CLUSTER_ON_SUCCESS:-0}"
RUN_LAUNCHPAD_E2E="${LICHEN_RUN_LAUNCHPAD_E2E:-0}"
RUN_VOLUME_E2E="${LICHEN_RUN_VOLUME_E2E:-0}"
SKIP_LOCAL_GATE_BUILD="${LICHEN_SKIP_LOCAL_GATE_BUILD:-0}"
LIVE_PAUSE_GAP_SLOTS="${LICHEN_LIVE_PAUSE_GAP_SLOTS:-140}"
ARCHIVE_V2_HTTPS_SOURCE_PID=""
ARCHIVE_V2_HTTPS_SOURCE_ROOT=""
ARCHIVE_V2_HTTPS_SOURCE_CA=""
ARCHIVE_V2_HTTPS_SOURCE_CERT=""
ARCHIVE_V2_HTTPS_SOURCE_KEY=""
ARCHIVE_V2_HTTPS_SOURCE_LOG="/tmp/lichen-testnet/archive-v2-https-source.log"
ARCHIVE_V2_HTTPS_SOURCE_PORT=9443
ARCHIVE_V2_HTTPS_SOURCE_TOKEN="local-archive-v2-gate-token"

export LICHEN_LOCAL_DEV=1
export LICHEN_LOCAL_ARCHIVE_COLD="${LICHEN_LOCAL_ARCHIVE_COLD:-1}"
export LICHEN_COLD_RETENTION_SLOTS="${LICHEN_COLD_RETENTION_SLOTS:-50000}"
export LICHEN_COLD_MIGRATION_INTERVAL_SECS="${LICHEN_COLD_MIGRATION_INTERVAL_SECS:-5}"
export LICHEN_LOCAL_SLOT_DURATION_MS="${LICHEN_LOCAL_SLOT_DURATION_MS:-5}"
# Deep accelerated joins must retain the immutable checkpoint for the entire
# exact verification + transfer. The production default remains two; eight is
# within the validator's validated maximum and is local-gate-only.
export LICHEN_CHECKPOINT_KEEP_COUNT="${LICHEN_CHECKPOINT_KEEP_COUNT:-8}"
CHECKPOINT_INTERVAL_SLOTS=1000
ARCHIVE_V2_RETENTION_PROOF_SLOT=$((LICHEN_COLD_RETENTION_SLOTS + 2500))
ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS="${LICHEN_ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS:-10000}"
ARCHIVE_V2_FRESH_JOIN_CATALOG_HEADROOM_SLOTS="${LICHEN_ARCHIVE_V2_FRESH_JOIN_CATALOG_HEADROOM_SLOTS:-40000}"
ARCHIVE_V2_RETENTION_TIMEOUT_SECS="${LICHEN_ARCHIVE_V2_RETENTION_TIMEOUT_SECS:-21600}"
ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS="${LICHEN_ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS:-1800}"
ARCHIVE_V2_DEEP_HISTORY_RPC_TIMEOUT_SECS="${LICHEN_ARCHIVE_V2_DEEP_HISTORY_RPC_TIMEOUT_SECS:-300}"
if (( ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS <= 0
    || ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS >= LICHEN_COLD_RETENTION_SLOTS )); then
    echo "LICHEN_ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS must be within 1..$((LICHEN_COLD_RETENTION_SLOTS - 1))" >&2
    exit 2
fi
if (( ARCHIVE_V2_FRESH_JOIN_CATALOG_HEADROOM_SLOTS <= 0
    || ARCHIVE_V2_FRESH_JOIN_CATALOG_HEADROOM_SLOTS >= LICHEN_COLD_RETENTION_SLOTS )); then
    echo "LICHEN_ARCHIVE_V2_FRESH_JOIN_CATALOG_HEADROOM_SLOTS must be within 1..$((LICHEN_COLD_RETENTION_SLOTS - 1))" >&2
    exit 2
fi
if [[ ! "$ARCHIVE_V2_RETENTION_TIMEOUT_SECS" =~ ^[1-9][0-9]*$ ]]; then
    echo "LICHEN_ARCHIVE_V2_RETENTION_TIMEOUT_SECS must be a positive integer" >&2
    exit 2
fi
if [[ "$RESUME_AFTER_PHASE2" != "0" && "$RESUME_AFTER_PHASE2" != "1" ]]; then
    echo "LICHEN_RESUME_LOCAL_GATE_AFTER_PHASE2 must be 0 or 1" >&2
    exit 2
fi
if [[ "$RESUME_AFTER_PUBLIC_PARITY" != "0" && "$RESUME_AFTER_PUBLIC_PARITY" != "1" ]]; then
    echo "LICHEN_RESUME_LOCAL_GATE_AFTER_PUBLIC_PARITY must be 0 or 1" >&2
    exit 2
fi
if [[ "$RESUME_AFTER_PHASE2" == "1" && "$RESUME_AFTER_PUBLIC_PARITY" == "1" ]]; then
    echo "Only one exact-gate resume boundary may be selected" >&2
    exit 2
fi
if [[ "$RESUME_AFTER_PUBLIC_PARITY" == "1"
    && ( ! "$RESUME_PUBLIC_PARITY_CHECKPOINT" =~ ^[1-9][0-9]*$
        || "$MAX_VALIDATORS" -ne 4 ) ]]; then
    echo "Public-parity resume requires four validators and an explicit positive LICHEN_RESUME_LOCAL_GATE_CHECKPOINT_SLOT" >&2
    exit 2
fi
if [[ ! "$ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS" =~ ^[1-9][0-9]*$ \
    || "$ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS" -lt 2 ]]; then
    echo "LICHEN_ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS must be an integer of at least 2" >&2
    exit 2
fi
if [[ ! "$ARCHIVE_V2_DEEP_HISTORY_RPC_TIMEOUT_SECS" =~ ^[1-9][0-9]*$ ]]; then
    echo "LICHEN_ARCHIVE_V2_DEEP_HISTORY_RPC_TIMEOUT_SECS must be a positive integer" >&2
    exit 2
fi

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

log() { echo -e "${CYAN}[TEST]${NC} $*"; }
ok()  { echo -e "${GREEN}[OK]${NC} $*"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
fail() { echo -e "${RED}[FAIL]${NC} $*"; exit 1; }

stop_local_processes() {
    pkill -CONT -f "lichen-validator" 2>/dev/null || true
    pkill -CONT -f "run-validator.sh testnet" 2>/dev/null || true
    if [[ -x "$REPO_ROOT/scripts/stop-local-stack.sh" ]]; then
        "$REPO_ROOT/scripts/stop-local-stack.sh" testnet >/dev/null 2>&1 || true
    fi

    pkill -f "validator-supervisor.sh" 2>/dev/null || true
    pkill -f "run-validator.sh testnet" 2>/dev/null || true
    pkill -f "lichen-validator" 2>/dev/null || true
    pkill -f "lichen-custody" 2>/dev/null || true
    pkill -f "lichen-faucet" 2>/dev/null || true
    pkill -f "first-boot-deploy.sh" 2>/dev/null || true
    sleep 2
}

stop_validator_pid() {
    local pid=$1
    [[ -n "$pid" ]] || return 0

    local pids child
    pids="$pid"
    while read -r child; do
        [[ -n "$child" ]] || continue
        pids="$child $pids"
    done < <(
        queue="$pid"
        while [[ -n "$queue" ]]; do
            next_queue=""
            for parent in $queue; do
                for child in $(pgrep -P "$parent" 2>/dev/null || true); do
                    echo "$child"
                    next_queue="$next_queue $child"
                done
            done
            queue="$next_queue"
        done
    )

    for child in $pids; do
        kill "$child" 2>/dev/null || true
    done
    for _ in $(seq 1 10); do
        local alive=0 stat
        for child in $pids; do
            stat="$(ps -p "$child" -o stat= 2>/dev/null || true)"
            stat="${stat//[[:space:]]/}"
            if [[ -n "$stat" && "$stat" != Z* ]]; then
                alive=1
                break
            fi
        done
        if [[ "$alive" -eq 0 ]]; then
            wait "$pid" 2>/dev/null || true
            return 0
        fi
        sleep 1
    done

    for child in $pids; do
        kill -9 "$child" 2>/dev/null || true
    done
    wait "$pid" 2>/dev/null || true
}

signal_validator_pid_tree() {
    local pid=$1
    local signal="${2:-TERM}"
    [[ -n "$pid" ]] || return 0
    if ! kill -0 "$pid" 2>/dev/null; then
        return 0
    fi

    local pids child
    pids="$pid"
    while read -r child; do
        [[ -n "$child" ]] || continue
        pids="$child $pids"
    done < <(
        queue="$pid"
        while [[ -n "$queue" ]]; do
            next_queue=""
            for parent in $queue; do
                for child in $(pgrep -P "$parent" 2>/dev/null || true); do
                    echo "$child"
                    next_queue="$next_queue $child"
                done
            done
            queue="$next_queue"
        done
    )

    for child in $pids; do
        kill -"$signal" "$child" 2>/dev/null || true
    done
}

# Port calculations (must match run-validator.sh)
p2p_port()  { echo $((7000 + $1)); }
rpc_port()  { echo $((8899 + 2 * ($1 - 1))); }
ws_port()   { echo $((8900 + 2 * ($1 - 1))); }
db_path()   { echo "$REPO_ROOT/data/state-$(p2p_port $1)"; }
cold_path() { echo "$REPO_ROOT/data/archive-$(p2p_port $1)"; }
log_path()  { echo "/tmp/lichen-testnet/v${1}.log"; }
restart_log_path() { echo "/tmp/lichen-testnet/v${1}-restart.log"; }
all_restart_log_path() { echo "/tmp/lichen-testnet/v${1}-all-restart.log"; }

wait_validator_resources_released() {
    local validator_num=$1
    local p2p rpc ws busy
    p2p="$(p2p_port "$validator_num")"
    rpc="$(rpc_port "$validator_num")"
    ws="$(ws_port "$validator_num")"

    for _ in $(seq 1 45); do
        busy=0
        pgrep -f "run-validator.sh testnet ${validator_num}" >/dev/null 2>&1 && busy=1
        pgrep -f "lichen-validator.*--p2p-port ${p2p}" >/dev/null 2>&1 && busy=1
        if command -v lsof >/dev/null 2>&1; then
            lsof -tiTCP:"$p2p" -sTCP:LISTEN >/dev/null 2>&1 && busy=1
            lsof -tiTCP:"$rpc" -sTCP:LISTEN >/dev/null 2>&1 && busy=1
            lsof -tiTCP:"$ws" -sTCP:LISTEN >/dev/null 2>&1 && busy=1
        fi

        [[ "$busy" -eq 0 ]] && return 0
        sleep 1
    done

    return 1
}

cleanup() {
    local exit_status=$?
    if [[ "$USING_EXISTING_CLUSTER" == "true" ]]; then
        log "Reused existing cluster — skipping cleanup"
        return
    fi
    if [[ "$KEEP_CLUSTER_ON_SUCCESS" == "1" && "$exit_status" -eq 0 ]]; then
        log "Verified cluster retained for follow-on E2E journeys"
        return
    fi

    log "Cleaning up..."
    if [[ -n "$ARCHIVE_V2_HTTPS_SOURCE_PID" ]]; then
        stop_validator_pid "$ARCHIVE_V2_HTTPS_SOURCE_PID"
        ARCHIVE_V2_HTTPS_SOURCE_PID=""
    fi
    stop_local_processes
    log "Cleanup done"
}
trap cleanup EXIT

# ── Preflight ──
[[ -x "$REPO_ROOT/run-validator.sh" ]] || fail "run-validator.sh not found"

# ── RPC helpers ──
rpc_query() {
    local port=$1 method=$2
    curl -sf --max-time 3 "http://127.0.0.1:${port}" -X POST \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\"}" 2>/dev/null || echo '{}'
}

rpc_query_params() {
    local port=$1 method=$2 params=$3
    curl -sf --max-time 5 "http://127.0.0.1:${port}" -X POST \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":${params}}" 2>/dev/null || echo '{}'
}

rpc_query_params_with_timeout() {
    local port=$1 method=$2 params=$3 timeout_secs=$4
    curl -sf --max-time "$timeout_secs" "http://127.0.0.1:${port}" -X POST \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":${params}}" 2>/dev/null || echo '{}'
}

get_slot() {
    rpc_query "$1" "getSlot" | python3 -c "import json,sys; print(json.load(sys.stdin).get('result',0))" 2>/dev/null || echo 0
}

get_slot_with_retry() {
    local rpc=$1 attempts=${2:-5}
    local response slot attempt
    for attempt in $(seq 1 "$attempts"); do
        response="$(rpc_query "$rpc" "getSlot")"
        slot="$(
            python3 -c '
import json
import sys

result = json.load(sys.stdin).get("result")
if not isinstance(result, int) or isinstance(result, bool) or result < 0:
    raise SystemExit(1)
print(result)
' <<< "$response" 2>/dev/null
        )" || slot=""
        if [[ "$slot" =~ ^[0-9]+$ ]]; then
            printf '%s\n' "$slot"
            return 0
        fi
        (( attempt < attempts )) && sleep 1
    done
    return 1
}

get_validator_count() {
    rpc_query "$1" "getValidators" | python3 -c "import json,sys; r=json.load(sys.stdin).get('result',{}); print(len(r.get('validators',[])) if isinstance(r,dict) else 0)" 2>/dev/null || echo 0
}

assert_joiner_starts_without_copied_chain_state() {
    local validator_num=$1
    local joiner_dir
    joiner_dir="$(db_path "$validator_num")"

    [[ -d "$joiner_dir" ]] || {
        ok "V${validator_num} starts without copied RocksDB, WAL, or genesis-wallet artifacts"
        return
    }

    if find "$joiner_dir" -maxdepth 1 \
        \( -name 'CURRENT' \
        -o -name 'LOCK' \
        -o -name 'MANIFEST-*' \
        -o -name 'OPTIONS-*' \
        -o -name '*.sst' \
        -o -name '*.log' \
        -o -name 'consensus_wal*' \
        -o -name 'genesis-wallet.json' \) \
        -print -quit | grep -q .; then
        find "$joiner_dir" -maxdepth 1 \
            \( -name 'CURRENT' \
            -o -name 'LOCK' \
            -o -name 'MANIFEST-*' \
            -o -name 'OPTIONS-*' \
            -o -name '*.sst' \
            -o -name '*.log' \
            -o -name 'consensus_wal*' \
            -o -name 'genesis-wallet.json' \) \
            -print
        fail "V${validator_num} joiner state contains copied chain-state, WAL, or genesis-wallet artifacts before sync"
    fi

    ok "V${validator_num} starts without copied RocksDB, WAL, or genesis-wallet artifacts"
}

# Count validators with actual stake (not just P2P routing entries with 0 stake)
get_staked_validator_count() {
    rpc_query "$1" "getValidators" | python3 -c "
import json,sys
try:
    r=json.load(sys.stdin).get('result',{})
    vs=r.get('validators',[]) if isinstance(r,dict) else []
    print(len([v for v in vs if v.get('stake',0) > 0]))
except: print(0)
" 2>/dev/null || echo 0
}

cluster_log_path() {
    local validator_num=$1
    local local_stack_log="/tmp/lichen-local-testnet/validator-${validator_num}.log"
    local harness_log
    harness_log="$(log_path "$validator_num")"
    if [[ -f "$local_stack_log" ]]; then
        echo "$local_stack_log"
    else
        echo "$harness_log"
    fi
}

existing_cluster_status_line() {
    local primary_rpc
    primary_rpc="$(rpc_port 1)"
    local statuses=()

    for n in $(seq 1 "$MAX_VALIDATORS"); do
        local rpc health status
        rpc="$(rpc_port "$n")"
        health="$(rpc_query "$rpc" "getHealth")"
        status="$(echo "$health" | python3 -c '
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
')"
        statuses+=("V${n}=${status:-unreachable}")
    done

    local staked
    staked="$(get_staked_validator_count "$primary_rpc")"
    echo "${statuses[*]} staked=${staked}/${MAX_VALIDATORS}"
}

wait_for_existing_cluster_healthy() {
    local timeout_seconds=${1:-$REUSE_HEALTH_TIMEOUT_SECS}

    for second in $(seq 1 "$timeout_seconds"); do
        if existing_cluster_healthy; then
            return 0
        fi

        if [[ $((second % 5)) -eq 0 ]]; then
            log "Waiting for existing-cluster readiness: $(existing_cluster_status_line)"
        fi

        sleep 1
    done

    return 1
}

existing_cluster_healthy() {
    local primary_rpc
    primary_rpc="$(rpc_port 1)"

    for n in $(seq 1 "$MAX_VALIDATORS"); do
        local rpc health
        rpc="$(rpc_port "$n")"
        health="$(rpc_query "$rpc" "getHealth")"
        echo "$health" | python3 -c "
import json,sys
try:
    result=json.load(sys.stdin).get('result', {})
    status=result.get('status') if isinstance(result, dict) else result
    raise SystemExit(0 if status == 'ok' else 1)
except Exception:
    raise SystemExit(1)
" >/dev/null 2>&1 || return 1
    done

    [[ "$(get_staked_validator_count "$primary_rpc")" -ge "$MAX_VALIDATORS" ]]
}

load_existing_cluster_pubkeys() {
    local primary_rpc=$1

    ALL_PUBKEYS=()
    while IFS= read -r pubkey; do
        [[ -n "$pubkey" ]] && ALL_PUBKEYS+=("$pubkey")
    done < <(rpc_query "$primary_rpc" "getValidators" | python3 -c '
import json
import sys

limit = int(sys.argv[1])
result = json.load(sys.stdin).get("result", {})
validators = result.get("validators", []) if isinstance(result, dict) else []
staked = [validator for validator in validators if validator.get("stake", 0) > 0][:limit]
for validator in staked:
    pubkey = validator.get("pubkey")
    if pubkey:
        print(pubkey)
' "$MAX_VALIDATORS")

    [[ "${#ALL_PUBKEYS[@]}" -ge "$MAX_VALIDATORS" ]]
}

validator_activity_lines() {
    local primary_rpc=$1

    rpc_query "$primary_rpc" "getValidators" | python3 -c '
import json
import sys

limit = int(sys.argv[1])
result = json.load(sys.stdin).get("result", {})
validators = result.get("validators", []) if isinstance(result, dict) else []
staked = [validator for validator in validators if validator.get("stake", 0) > 0][:limit]
for validator in staked:
    produced = validator.get("blocks_proposed", validator.get("_blocks_produced", 0))
    votes = validator.get("votes_cast", 0)
    last_active = validator.get("last_active_slot", 0)
    print("{}|{}|{}|{}".format(validator.get("pubkey", ""), produced, votes, last_active))
' "$MAX_VALIDATORS"
}

validator_activity_for_pubkey() {
    local primary_rpc=$1
    local expected_pubkey=$2

    rpc_query "$primary_rpc" "getValidators" | python3 -c '
import json
import sys

expected = sys.argv[1]
result = json.load(sys.stdin).get("result", {})
validators = result.get("validators", []) if isinstance(result, dict) else []
for validator in validators:
    if validator.get("pubkey") == expected:
        produced = validator.get("blocks_proposed", validator.get("_blocks_produced", 0))
        votes = validator.get("votes_cast", 0)
        last_active = validator.get("last_active_slot", 0)
        print("{}|{}|{}".format(produced, votes, last_active))
        raise SystemExit(0)
raise SystemExit(1)
' "$expected_pubkey"
}

verify_chain_producing() {
    local label=$1 rpc=$2 seconds=${3:-10}
    log "Verifying chain produces blocks ($label)..."
    local s1 s2 diff
    if ! s1="$(get_slot_with_retry "$rpc")"; then
        fail "RPC getSlot remained unavailable before the liveness window ($label)"
    fi
    sleep "$seconds"
    if ! s2="$(get_slot_with_retry "$rpc")"; then
        fail "RPC getSlot remained unavailable after the liveness window ($label)"
    fi
    diff=$((s2 - s1))
    if [[ "$diff" -lt 2 ]]; then
        for n in $(seq 1 "$MAX_VALIDATORS"); do
            local lp
            lp="$(log_path $n)"
            [[ -f "$lp" ]] && { warn "V${n} log tail:"; tail -20 "$lp"; }
        done
        fail "Chain stalled ($label)! Only $diff blocks in ${seconds}s (slot $s1 → $s2)"
    fi
    ok "Chain alive ($label): $diff blocks in ${seconds}s (slot $s1 → $s2)"
}

verify_chain_recovers_after_registration() {
    local label=$1 rpc=$2
    local initial_window_secs=10 recovery_window_secs=50 recovery_min_blocks=10
    local s1 s2 diff

    log "Verifying chain produces blocks ($label)..."
    s1=$(get_slot "$rpc")
    sleep "$initial_window_secs"
    s2=$(get_slot "$rpc")
    diff=$((s2 - s1))
    if [[ "$diff" -ge 2 ]]; then
        ok "Chain alive ($label): $diff blocks in ${initial_window_secs}s (slot $s1 → $s2)"
        return 0
    fi

    warn "Chain entered bounded BFT recovery after registration: ${diff} block(s) in ${initial_window_secs}s"
    # A membership transition can land while peers occupy different BFT
    # rounds. Each protocol phase backs off to a 5s cap, so permit enough time
    # for several complete rounds, then require a sustained recovery burst
    # rather than accepting one late block.
    for _ in $(seq 1 $((recovery_window_secs / 2))); do
        sleep 2
        s2=$(get_slot "$rpc")
        diff=$((s2 - s1))
        if [[ "$diff" -ge "$recovery_min_blocks" ]]; then
            ok "Chain recovered after registration: $diff blocks in at most $((initial_window_secs + recovery_window_secs))s (slot $s1 → $s2)"
            return 0
        fi
    done

    for n in $(seq 1 "$MAX_VALIDATORS"); do
        local lp
        lp="$(log_path $n)"
        [[ -f "$lp" ]] && { warn "V${n} log tail:"; tail -20 "$lp"; }
    done
    fail "Chain did not recover after registration: only $diff blocks in $((initial_window_secs + recovery_window_secs))s (slot $s1 → $s2)"
}

verify_canonical_commit_parity() {
    local min_slot=999999999999 target_slot baseline="" fingerprint response
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        local slot
        slot="$(get_slot "$(rpc_port "$V_NUM")")"
        [[ "$slot" -lt "$min_slot" ]] && min_slot="$slot"
    done
    [[ "$min_slot" -ge 2 ]] || fail "Cannot verify canonical commit parity below slot 2"
    target_slot=$((min_slot - 1))
    log "Verifying canonical child-committed certificate parity at slot ${target_slot}..."

    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        response="$(rpc_query_params "$(rpc_port "$V_NUM")" getBlockCommit "[${target_slot}]")"
        fingerprint="$(python3 -c '
import hashlib
import json
import sys

data = json.load(sys.stdin)
result = data.get("result")
if not isinstance(result, dict) or result.get("commit_source") != "canonical_child":
    raise SystemExit(1)
print(hashlib.sha256(json.dumps(result, sort_keys=True, separators=(",", ":")).encode()).hexdigest())
' <<< "$response")" || fail "V${V_NUM} did not serve a canonical child certificate for slot ${target_slot}"
        if [[ -z "$baseline" ]]; then
            baseline="$fingerprint"
        elif [[ "$fingerprint" != "$baseline" ]]; then
            fail "Canonical commit drift at slot ${target_slot}: V${V_NUM} fingerprint ${fingerprint} differs from ${baseline}"
        fi
    done
    ok "Canonical commit certificate matches across ${MAX_VALIDATORS} validators at slot ${target_slot}: ${baseline}"
}

public_history_manifest_root() {
    local validator_num=$1
    local mode="${2:-live}"
    local checkpoint_slot="${3:-}"
    local manifest_file="/tmp/lichen-testnet/public-history-v${validator_num}.json"
    local secondary_dir="/tmp/lichen-testnet/public-history-secondary-v${validator_num}"
    local manifest_db_path
    local manifest_cold_path
    if [[ -n "$checkpoint_slot" ]]; then
        manifest_db_path="$(db_path "$validator_num")/checkpoints/slot-${checkpoint_slot}"
        manifest_cold_path="$manifest_db_path/cold"
    else
        manifest_db_path="$(db_path "$validator_num")"
        manifest_cold_path="$(cold_path "$validator_num")"
    fi
    local args=(
        "$REPO_ROOT/target/release/lichen-validator"
        --network testnet
        --dev-mode
        --db-path "$manifest_db_path"
        --cold-store "$manifest_cold_path"
        --cache-size-mb 128
        --public-history-manifest
    )

    if [[ "$mode" == "live" ]]; then
        rm -rf "$secondary_dir"
        args+=(--secondary-dir "$secondary_dir")
    fi

    "${args[@]}" > "$manifest_file"

    python3 -c '
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as fh:
    data = json.load(fh)
print(data["manifest_root"])
' "$manifest_file"
}

verify_public_history_manifest_parity() {
    local mode="${1:-live}"
    local checkpoint_slot="${2:-}"

    if [[ "${LICHEN_LOCAL_ARCHIVE_COLD:-0}" != "1" ]]; then
        warn "Skipping public-history manifest parity; LICHEN_LOCAL_ARCHIVE_COLD is not enabled"
        return
    fi

    local scope="${mode}"
    [[ -n "$checkpoint_slot" ]] && scope="checkpoint slot ${checkpoint_slot}"
    log "Verifying public-history manifest parity across hot+cold local validators (${scope})..."
    local baseline_root=""
    local root
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        if [[ ! -d "$(cold_path "$V_NUM")" ]]; then
            fail "V${V_NUM} cold archive path is missing: $(cold_path "$V_NUM")"
        fi
        if [[ -n "$checkpoint_slot" && ! -f "$(db_path "$V_NUM")/checkpoints/slot-${checkpoint_slot}/checkpoint_meta.json" ]]; then
            fail "V${V_NUM} checkpoint ${checkpoint_slot} is missing"
        fi
        root="$(public_history_manifest_root "$V_NUM" "$mode" "$checkpoint_slot")"
        ok "V${V_NUM} public-history manifest root: $root"
        if [[ -z "$baseline_root" ]]; then
            baseline_root="$root"
        elif [[ "$root" != "$baseline_root" ]]; then
            fail "Public-history manifest drift: V${V_NUM} root $root differs from baseline $baseline_root"
        fi
    done
    ok "Public-history manifests match across $MAX_VALIDATORS validators"
}

archive_v2_genesis_hash() {
    local response
    response="$(rpc_query_params "$V1_RPC" getBlock "[0]")"
    python3 -c '
import json
import sys

payload = json.load(sys.stdin)
result = payload.get("result")
if not isinstance(result, dict):
    raise SystemExit(1)
value = result.get("hash") or result.get("blockhash")
if not isinstance(value, str) or len(value) != 64:
    raise SystemExit(1)
print(value)
' <<< "$response"
}

verify_archive_v2_offline_matrix() {
    local checkpoint_slot=$1
    local genesis_hash=$2
    local archive_finality_depth=$((LICHEN_COLD_RETENTION_SLOTS - ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS))
    local build_end=$((checkpoint_slot - archive_finality_depth - 5))
    local baseline_root=""
    local archive_root replica_root mirror_root restore_root build_json status_json catalog_root
    local av2="$REPO_ROOT/target/release/lichen-archive-v2"

    [[ "$build_end" -ge 0 ]] || fail "Archive V2 test range is empty at checkpoint ${checkpoint_slot}"
    log "Building and independently verifying Archive V2 range 0..${build_end} on all ${MAX_VALIDATORS} node-owned states..."
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        archive_root="/tmp/lichen-testnet/archive-v2-v${V_NUM}"
        replica_root="/tmp/lichen-testnet/archive-v2-replica-v${V_NUM}"
        mirror_root="/tmp/lichen-testnet/archive-v2-mirror-v${V_NUM}"
        restore_root="/tmp/lichen-testnet/archive-v2-restore-v${V_NUM}"
        rm -rf "$archive_root" "$replica_root" "$mirror_root" "$restore_root"

        build_json="$(
            "$av2" build \
                --state-dir "$(db_path "$V_NUM")/checkpoints/slot-${checkpoint_slot}" \
                --cold-store "$(db_path "$V_NUM")/checkpoints/slot-${checkpoint_slot}/cold" \
                --root "$archive_root" \
                --network-id lichen-testnet-1 \
                --genesis-hash "$genesis_hash" \
                --start-slot 0 \
                --end-slot "$build_end" \
                --finality-depth-slots "$archive_finality_depth" \
                --zstd-level 6 \
                --frame-bytes 1048576 \
                --replica-root "$replica_root" \
                --required-replicas 1
        )" || fail "V${V_NUM} Archive V2 deterministic build failed"
        status_json="$("$av2" status --root "$archive_root")" \
            || fail "V${V_NUM} Archive V2 status failed"
        "$av2" verify --root "$archive_root" --max-objects 10000 >/dev/null \
            || fail "V${V_NUM} Archive V2 full verification failed"
        catalog_root="$(python3 -c '
import json
import sys
print(json.load(sys.stdin)["catalog_root"])
' <<< "$status_json")" || fail "V${V_NUM} Archive V2 status did not contain a catalog root"
        if [[ -z "$baseline_root" ]]; then
            baseline_root="$catalog_root"
        elif [[ "$catalog_root" != "$baseline_root" ]]; then
            fail "Archive V2 catalog drift: V${V_NUM} root ${catalog_root} differs from ${baseline_root}"
        fi

        "$av2" mirror \
            --root "$archive_root" \
            --destination "mirror-v${V_NUM}:region-${V_NUM}:${mirror_root}" \
            --required-replicas 1 \
            --required-failure-domains 1 \
            --max-objects 1000 \
            --max-bytes 17179869184 >/dev/null \
            || fail "V${V_NUM} Archive V2 mirror failed"
        "$av2" restore \
            --root "$restore_root" \
            --source "mirror-v${V_NUM}:region-${V_NUM}:${mirror_root}" \
            --network-id lichen-testnet-1 \
            --genesis-hash "$genesis_hash" \
            --max-objects 1000 \
            --max-bytes 17179869184 >/dev/null \
            || fail "V${V_NUM} Archive V2 restore failed"
        "$av2" verify --root "$restore_root" --max-objects 10000 >/dev/null \
            || fail "V${V_NUM} restored Archive V2 verification failed"
        ok "V${V_NUM} Archive V2 build/mirror/restore root: ${catalog_root}"
        [[ -n "$build_json" ]] || fail "V${V_NUM} Archive V2 build emitted no evidence"

        # Retain only the exact role inputs consumed by the runtime matrix:
        # V1/V4 local objects for full-archive service, V2's independent
        # replica for verified-cache fetch, V3's catalog for consensus-role
        # denial, and V4's replica for corruption repair. Promoted staging
        # objects and unused replicas are reproducible proof intermediates.
        # Keeping all of them until V4 makes independent builds consume
        # cumulative staging space and can incorrectly trip the adaptive
        # capacity gate.
        rm -rf "$mirror_root" "$restore_root"
        find "$archive_root/staging" -type f -name '*.av2s' -delete
        case "$V_NUM" in
            1)
                rm -rf "$replica_root"
                ;;
            2)
                find "$archive_root/objects" -type f -name '*.av2s' -delete
                ;;
            3)
                find "$archive_root/objects" -type f -name '*.av2s' -delete
                rm -rf "$replica_root"
                ;;
        esac
    done
    ok "Archive V2 deterministic catalog root matches across all validators: ${baseline_root}"
}

archive_v2_runtime_role() {
    case "$1" in
        2) echo "verified-cache" ;;
        3) echo "consensus" ;;
        *) echo "full-archive" ;;
    esac
}

start_archive_v2_validator() {
    local validator_num=$1
    local output_log=$2
    local detached="${3:-0}"
    local role root role_override root_override cache_override source_override
    local source_url_override source_ca_override source_token_override source_url
    local -a role_env
    role_override="LICHEN_LOCAL_ARCHIVE_V2_ROLE_V${validator_num}"
    root_override="LICHEN_LOCAL_ARCHIVE_V2_ROOT_V${validator_num}"
    cache_override="LICHEN_LOCAL_ARCHIVE_V2_CACHE_ROOT_V${validator_num}"
    source_override="LICHEN_LOCAL_ARCHIVE_V2_SOURCE_DIRS_V${validator_num}"
    source_url_override="LICHEN_LOCAL_ARCHIVE_V2_SOURCE_URLS_V${validator_num}"
    source_ca_override="LICHEN_LOCAL_ARCHIVE_V2_SOURCE_CA_CERT_V${validator_num}"
    source_token_override="LICHEN_LOCAL_ARCHIVE_V2_SOURCE_BEARER_TOKEN_V${validator_num}"
    role="${!role_override:-$(archive_v2_runtime_role "$validator_num")}"
    root="${!root_override:-/tmp/lichen-testnet/archive-v2-v${validator_num}}"
    role_env=(
        "LICHEN_DISABLE_SUPERVISOR=1"
        "LICHEN_LOCAL_ARCHIVE_V2_ROLE=${role}"
        "LICHEN_LOCAL_ARCHIVE_V2_ROOT=${root}"
        "LICHEN_LOCAL_ARCHIVE_V2_RECENT_HISTORY_SLOTS=50000"
    )
    if [[ "$role" == "verified-cache" ]]; then
        role_env+=(
            "LICHEN_LOCAL_ARCHIVE_V2_CACHE_ROOT=${!cache_override:-/tmp/lichen-testnet/archive-v2-cache-v${validator_num}}"
            "LICHEN_LOCAL_ARCHIVE_V2_CACHE_QUOTA_BYTES=2147483648"
        )
        source_url="${!source_url_override:-}"
        if [[ -n "$source_url" ]]; then
            role_env+=("LICHEN_LOCAL_ARCHIVE_V2_SOURCE_URLS=${source_url}")
        else
            role_env+=(
                "LICHEN_LOCAL_ARCHIVE_V2_SOURCE_DIRS=${!source_override:-/tmp/lichen-testnet/archive-v2-replica-v${validator_num}}"
            )
        fi
        if [[ -n "${!source_ca_override:-}" ]]; then
            role_env+=("LICHEN_ARCHIVE_V2_SOURCE_CA_CERT=${!source_ca_override}")
        fi
        if [[ -n "${!source_token_override:-}" ]]; then
            role_env+=("LICHEN_ARCHIVE_V2_SOURCE_BEARER_TOKEN=${!source_token_override}")
        fi
    fi

    if [[ "$detached" == "1" ]]; then
        nohup env "${role_env[@]}" \
            "$REPO_ROOT/run-validator.sh" testnet "$validator_num" \
            </dev/null > "$output_log" 2>&1 &
    else
        env "${role_env[@]}" \
            "$REPO_ROOT/run-validator.sh" testnet "$validator_num" \
            > "$output_log" 2>&1 &
    fi
    ARCHIVE_V2_STARTED_PID=$!
}

stop_archive_v2_https_source() {
    if [[ -n "$ARCHIVE_V2_HTTPS_SOURCE_PID" ]]; then
        stop_validator_pid "$ARCHIVE_V2_HTTPS_SOURCE_PID"
        ARCHIVE_V2_HTTPS_SOURCE_PID=""
    fi
}

start_archive_v2_https_source() {
    local source_root=$1
    local tls_root="/tmp/lichen-testnet/archive-v2-https-tls"
    local status

    command -v openssl >/dev/null 2>&1 \
        || fail "Archive V2 HTTPS retrieval gate requires openssl"
    command -v python3 >/dev/null 2>&1 \
        || fail "Archive V2 HTTPS retrieval gate requires python3"
    [[ -f "$source_root/catalog.av2" ]] \
        || fail "Archive V2 HTTPS source has no replicated catalog"

    stop_archive_v2_https_source
    if [[ "$ARCHIVE_V2_HTTPS_SOURCE_ROOT" != "$source_root"
        || ! -f "$tls_root/ca.pem"
        || ! -f "$tls_root/server.pem"
        || ! -f "$tls_root/server.key" ]]; then
        rm -rf "$tls_root"
        mkdir -p "$tls_root"
        openssl req -x509 -newkey rsa:2048 -nodes \
            -keyout "$tls_root/ca.key" \
            -out "$tls_root/ca.pem" \
            -subj "/CN=Lichen Archive V2 Local Gate CA" \
            -days 1 >/dev/null 2>&1 \
            || fail "Could not create Archive V2 local gate CA"
        openssl req -newkey rsa:2048 -nodes \
            -keyout "$tls_root/server.key" \
            -out "$tls_root/server.csr" \
            -subj "/CN=127.0.0.1" \
            -addext "subjectAltName=IP:127.0.0.1,DNS:localhost" >/dev/null 2>&1 \
            || fail "Could not create Archive V2 local gate TLS request"
        openssl x509 -req \
            -in "$tls_root/server.csr" \
            -CA "$tls_root/ca.pem" \
            -CAkey "$tls_root/ca.key" \
            -CAcreateserial \
            -out "$tls_root/server.pem" \
            -days 1 \
            -sha256 \
            -copy_extensions copy >/dev/null 2>&1 \
            || fail "Could not sign Archive V2 local gate TLS certificate"
    fi

    ARCHIVE_V2_HTTPS_SOURCE_ROOT="$source_root"
    ARCHIVE_V2_HTTPS_SOURCE_CA="$tls_root/ca.pem"
    ARCHIVE_V2_HTTPS_SOURCE_CERT="$tls_root/server.pem"
    ARCHIVE_V2_HTTPS_SOURCE_KEY="$tls_root/server.key"
    : > "$ARCHIVE_V2_HTTPS_SOURCE_LOG"
    LICHEN_TEST_ARCHIVE_BEARER_TOKEN="$ARCHIVE_V2_HTTPS_SOURCE_TOKEN" \
        python3 "$REPO_ROOT/tests/archive-v2-https-source.py" \
            --root "$ARCHIVE_V2_HTTPS_SOURCE_ROOT" \
            --cert "$ARCHIVE_V2_HTTPS_SOURCE_CERT" \
            --key "$ARCHIVE_V2_HTTPS_SOURCE_KEY" \
            --port "$ARCHIVE_V2_HTTPS_SOURCE_PORT" \
            > "$ARCHIVE_V2_HTTPS_SOURCE_LOG" 2>&1 &
    ARCHIVE_V2_HTTPS_SOURCE_PID=$!
    for _ in $(seq 1 40); do
        if ! kill -0 "$ARCHIVE_V2_HTTPS_SOURCE_PID" 2>/dev/null; then
            tail -40 "$ARCHIVE_V2_HTTPS_SOURCE_LOG"
            fail "Archive V2 HTTPS source exited during startup"
        fi
        status="$(curl -sS -o /dev/null -w '%{http_code}' \
            --cacert "$ARCHIVE_V2_HTTPS_SOURCE_CA" \
            -H "Authorization: Bearer $ARCHIVE_V2_HTTPS_SOURCE_TOKEN" \
            "https://127.0.0.1:${ARCHIVE_V2_HTTPS_SOURCE_PORT}/objects/0000000000000000000000000000000000000000000000000000000000000000.av2s" \
            2>/dev/null || true)"
        if [[ "$status" == "404" ]]; then
            ok "Authenticated Archive V2 HTTPS source is ready"
            return
        fi
        sleep 1
    done
    tail -40 "$ARCHIVE_V2_HTTPS_SOURCE_LOG"
    fail "Archive V2 HTTPS source did not become ready"
}

prepare_archive_v2_fresh_join_roots() {
    local checkpoint_slot current_slot build_end finality_depth genesis_hash
    local av2="$REPO_ROOT/target/release/lichen-archive-v2"
    local source_root="/tmp/lichen-testnet/archive-v2-fresh-source"
    local replica_root="/tmp/lichen-testnet/archive-v2-fresh-replica"
    local full_root="/tmp/lichen-testnet/archive-v2-fresh-full-v3"
    local cache_root="/tmp/lichen-testnet/archive-v2-fresh-cache-catalog-v3"
    local consensus_root="/tmp/lichen-testnet/archive-v2-fresh-consensus-v3"

    current_slot="$(get_slot "$V1_RPC")"
    checkpoint_slot=$((current_slot / CHECKPOINT_INTERVAL_SLOTS * CHECKPOINT_INTERVAL_SLOTS))
    for _ in $(seq 1 120); do
        if [[ -f "$(db_path 1)/checkpoints/slot-${checkpoint_slot}/checkpoint_meta.json"
            && -f "$(db_path 2)/checkpoints/slot-${checkpoint_slot}/checkpoint_meta.json" ]]; then
            break
        fi
        sleep 1
    done
    [[ -f "$(db_path 1)/checkpoints/slot-${checkpoint_slot}/checkpoint_meta.json" ]] \
        || fail "V1 did not retain checkpoint ${checkpoint_slot} for fresh Archive V2 joins"
    [[ -f "$(db_path 2)/checkpoints/slot-${checkpoint_slot}/checkpoint_meta.json" ]] \
        || fail "V2 did not retain checkpoint ${checkpoint_slot} for fresh Archive V2 joins"

    # The immutable source must remain complete while a fresh validator
    # downloads and verifies a multi-GB checkpoint, then replays the live gap.
    # Keep this accelerated-gate overlap separate from the smaller offline
    # matrix range; runtime role admission must continue to fail closed if the
    # catalog actually falls behind the hot-window boundary.
    finality_depth=$((LICHEN_COLD_RETENTION_SLOTS - ARCHIVE_V2_FRESH_JOIN_CATALOG_HEADROOM_SLOTS))
    build_end=$((checkpoint_slot - finality_depth - 5))
    [[ "$build_end" -ge 0 ]] \
        || fail "Fresh-join Archive V2 range is empty at checkpoint ${checkpoint_slot}"
    genesis_hash="$(archive_v2_genesis_hash)" \
        || fail "Could not capture genesis hash for fresh Archive V2 role joins"
    rm -rf "$source_root" "$replica_root" "$full_root" "$cache_root" "$consensus_root"

    log "Stopping V1/V2 at node-owned checkpoints for immutable fresh-join source construction..."
    for validator_num in 1 2; do
        stop_validator_pid "${VALIDATOR_PIDS[$validator_num]:-}"
        wait_validator_resources_released "$validator_num" \
            || fail "V${validator_num} did not release resources for fresh-join source construction"
    done
    "$av2" build \
        --state-dir "$(db_path 1)/checkpoints/slot-${checkpoint_slot}" \
        --cold-store "$(cold_path 1)" \
        --root "$source_root" \
        --network-id lichen-testnet-1 \
        --genesis-hash "$genesis_hash" \
        --start-slot 0 \
        --end-slot "$build_end" \
        --finality-depth-slots "$finality_depth" \
        --zstd-level 6 \
        --frame-bytes 1048576 \
        --replica-root "$replica_root" \
        --required-replicas 1 >/dev/null \
        || fail "Could not build immutable Archive V2 source for fresh joins"
    "$av2" restore \
        --root "$full_root" \
        --source "fresh-source:local-a:${replica_root}" \
        --network-id lichen-testnet-1 \
        --genesis-hash "$genesis_hash" \
        --max-objects 1000 \
        --max-bytes 17179869184 >/dev/null \
        || fail "Could not restore complete immutable Archive V2 root for fresh full join"
    start_archive_v2_https_source "$replica_root"
    mkdir -p "$cache_root" "$consensus_root"
    install -m 0644 "$source_root/catalog.av2" "$cache_root/catalog.av2"
    install -m 0644 "$source_root/catalog.av2" "$consensus_root/catalog.av2"

    for validator_num in 1 2; do
        restart_log="/tmp/lichen-testnet/v${validator_num}-post-fresh-source.log"
        LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet "$validator_num" \
            > "$restart_log" 2>&1 &
        VALIDATOR_PIDS[$validator_num]=$!
    done
    SOURCE_RESTARTED=false
    for _ in $(seq 1 180); do
        for validator_num in 1 2; do
            if ! kill -0 "${VALIDATOR_PIDS[$validator_num]}" 2>/dev/null; then
                tail -100 "/tmp/lichen-testnet/v${validator_num}-post-fresh-source.log"
                fail "V${validator_num} exited after immutable fresh-join source construction"
            fi
        done
        v1_slot="$(get_slot "$(rpc_port 1)")"
        v2_slot="$(get_slot "$(rpc_port 2)")"
        if [[ "$v1_slot" -gt 0 && "$v2_slot" -gt 0
            && $((v1_slot - v2_slot)) -le 20 && $((v2_slot - v1_slot)) -le 20 ]]; then
            SOURCE_RESTARTED=true
            break
        fi
        sleep 2
    done
    $SOURCE_RESTARTED \
        || fail "V1/V2 did not restart in sync after immutable fresh-join source construction"
    verify_chain_producing "after immutable fresh-join source construction" "$V1_RPC" 10

    LICHEN_LOCAL_ARCHIVE_V2_ROLE_V3="full-archive"
    LICHEN_LOCAL_ARCHIVE_V2_ROOT_V3="$full_root"
    LICHEN_LOCAL_ARCHIVE_V2_CACHE_ROOT_V3="/tmp/lichen-testnet/archive-v2-fresh-cache-v3"
    LICHEN_LOCAL_ARCHIVE_V2_SOURCE_DIRS_V3="$replica_root"
    LICHEN_LOCAL_ARCHIVE_V2_SOURCE_URLS_V3="https://127.0.0.1:${ARCHIVE_V2_HTTPS_SOURCE_PORT}/"
    LICHEN_LOCAL_ARCHIVE_V2_SOURCE_CA_CERT_V3="$ARCHIVE_V2_HTTPS_SOURCE_CA"
    LICHEN_LOCAL_ARCHIVE_V2_SOURCE_BEARER_TOKEN_V3="$ARCHIVE_V2_HTTPS_SOURCE_TOKEN"
    FRESH_ARCHIVE_V2_FULL_ROOT="$full_root"
    FRESH_ARCHIVE_V2_CACHE_CATALOG_ROOT="$cache_root"
    FRESH_ARCHIVE_V2_CONSENSUS_ROOT="$consensus_root"
    FRESH_ARCHIVE_V2_GENESIS_HASH="$genesis_hash"
    ok "Prepared immutable Archive V2 fresh-join roots through slot ${build_end} from checkpoint ${checkpoint_slot}"
}

archive_v2_health_role() {
    rpc_query "$1" getHealth | python3 -c '
import json
import sys

status = json.load(sys.stdin).get("result", {}).get("archive_v2", {}).get("status")
role = status.get("role", "") if isinstance(status, dict) else ""
print(role.replace("_", "-"))
' 2>/dev/null
}

archive_v2_health_admitted_after_fresh_sync() {
    rpc_query "$1" getHealth | python3 -c '
import json
import sys

status = json.load(sys.stdin).get("result", {}).get("archive_v2", {}).get("status")
print("true" if isinstance(status, dict) and status.get("admitted_after_fresh_sync") is True else "false")
' 2>/dev/null
}

wait_for_archive_v2_role_catchup() {
    local validator_num=$1 expected_role=$2 pid=$3 output_log=$4
    local rpc network_slot local_slot drift observed_role attempts
    rpc="$(rpc_port "$validator_num")"
    attempts=$(((ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS + 1) / 2))
    for i in $(seq 1 "$attempts"); do
        sleep 2
        if ! kill -0 "$pid" 2>/dev/null; then
            tail -100 "$output_log"
            fail "V${validator_num} exited during fresh ${expected_role} join"
        fi
        network_slot="$(get_slot "$V1_RPC")"
        local_slot="$(get_slot "$rpc")"
        drift=$((network_slot - local_slot))
        observed_role="$(archive_v2_health_role "$rpc")"
        if [[ "$local_slot" -gt 0 && "$drift" -le 20 && "$observed_role" == "$expected_role" ]]; then
            ok "Fresh V${validator_num} ${expected_role} role admitted at slot ${local_slot} (network=${network_slot}, drift=${drift})"
            return 0
        fi
        if [[ $((i % 15)) -eq 0 ]]; then
            log "  Fresh ${expected_role} V${validator_num}: slot=${local_slot} network=${network_slot} drift=${drift} admitted=${observed_role:-no}"
        fi
    done
    tail -120 "$output_log"
    fail "V${validator_num} did not complete fresh ${expected_role} role admission within ${ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS}s"
}

reset_fresh_role_state_with_identity() {
    local validator_num=$1 identity_file=$2 node_identity_file=$3
    local state_dir cold_dir
    state_dir="$(db_path "$validator_num")"
    cold_dir="$(cold_path "$validator_num")"
    rm -rf "$state_dir" "$cold_dir"
    mkdir -p "$state_dir/home/.lichen"
    install -m 0600 "$identity_file" "$state_dir/validator-keypair.json"
    if [[ -f "$node_identity_file" ]]; then
        install -m 0600 "$node_identity_file" "$state_dir/home/.lichen/node_identity.json"
    fi
    assert_joiner_starts_without_copied_chain_state "$validator_num"
}

verify_fresh_archive_v2_role_rejoins() {
    local validator_num=3
    local original_state="/tmp/lichen-testnet/v3-full-role-state"
    local original_cold="/tmp/lichen-testnet/v3-full-role-cold"
    local identity_file="/tmp/lichen-testnet/v3-role-validator-keypair.json"
    local node_identity_file="/tmp/lichen-testnet/v3-role-node-identity.json"
    local role_log pid recent_slot error_message restored_pubkey

    [[ "$(archive_v2_health_role "$(rpc_port "$validator_num")")" == "full-archive" ]] \
        || fail "Initial fresh V3 join was not admitted as full-archive"
    [[ "$(archive_v2_health_admitted_after_fresh_sync "$(rpc_port "$validator_num")")" == "true" ]] \
        || fail "Fresh full-archive V3 did not exercise deferred post-sync admission"
    [[ "$(archive_v2_rpc_block_hash "$(rpc_port "$validator_num")" 0)" == "$FRESH_ARCHIVE_V2_GENESIS_HASH" ]] \
        || fail "Fresh full-archive V3 did not serve verified genesis history"
    ok "Fresh full-archive join served verified deep history"

    stop_validator_pid "${VALIDATOR_PIDS[$validator_num]:-}"
    wait_validator_resources_released "$validator_num" \
        || fail "V3 did not release resources before fresh role rejoin matrix"
    rm -rf "$original_state" "$original_cold"
    install -m 0600 "$(db_path "$validator_num")/validator-keypair.json" "$identity_file"
    if [[ -f "$(db_path "$validator_num")/home/.lichen/node_identity.json" ]]; then
        install -m 0600 \
            "$(db_path "$validator_num")/home/.lichen/node_identity.json" \
            "$node_identity_file"
    else
        rm -f "$node_identity_file"
    fi
    mv "$(db_path "$validator_num")" "$original_state"
    if [[ -d "$(cold_path "$validator_num")" ]]; then
        mv "$(cold_path "$validator_num")" "$original_cold"
    fi

    reset_fresh_role_state_with_identity "$validator_num" "$identity_file" "$node_identity_file"
    LICHEN_LOCAL_ARCHIVE_V2_ROLE_V3="verified-cache"
    LICHEN_LOCAL_ARCHIVE_V2_ROOT_V3="$FRESH_ARCHIVE_V2_CACHE_CATALOG_ROOT"
    rm -rf "$LICHEN_LOCAL_ARCHIVE_V2_CACHE_ROOT_V3"
    role_log="/tmp/lichen-testnet/v3-fresh-verified-cache.log"
    start_archive_v2_validator "$validator_num" "$role_log"
    pid="$ARCHIVE_V2_STARTED_PID"
    VALIDATOR_PIDS[$validator_num]="$pid"
    wait_for_archive_v2_role_catchup "$validator_num" "verified-cache" "$pid" "$role_log"
    [[ "$(archive_v2_rpc_block_hash "$(rpc_port "$validator_num")" 0)" == "$FRESH_ARCHIVE_V2_GENESIS_HASH" ]] \
        || fail "Fresh verified-cache V3 did not fetch verified genesis history"
    find "$LICHEN_LOCAL_ARCHIVE_V2_CACHE_ROOT_V3/objects" -type f -name '*.av2s' -print -quit \
        | grep -q . \
        || fail "Fresh verified-cache V3 did not persist its fetched object"
    grep -q 'GET /objects/.* 200 ' "$ARCHIVE_V2_HTTPS_SOURCE_LOG" \
        || fail "Fresh verified-cache V3 did not use the authenticated HTTPS source"
    ok "Fresh verified-cache join fetched, verified, and persisted deep history over authenticated HTTPS"

    stop_validator_pid "${VALIDATOR_PIDS[$validator_num]:-}"
    wait_validator_resources_released "$validator_num" \
        || fail "V3 did not release resources before authenticated source outage"
    if [[ -d "$LICHEN_LOCAL_ARCHIVE_V2_CACHE_ROOT_V3/objects" ]]; then
        while IFS= read -r cached_object; do
            rm -f -- "$cached_object"
        done < <(
            find "$LICHEN_LOCAL_ARCHIVE_V2_CACHE_ROOT_V3/objects" \
                -type f -name '*.av2s' -print
        )
    fi
    stop_archive_v2_https_source
    role_log="/tmp/lichen-testnet/v3-fresh-verified-cache-source-outage.log"
    start_archive_v2_validator "$validator_num" "$role_log"
    pid="$ARCHIVE_V2_STARTED_PID"
    VALIDATOR_PIDS[$validator_num]="$pid"
    wait_for_archive_v2_role_catchup "$validator_num" "verified-cache" "$pid" "$role_log"
    error_message="$(archive_v2_rpc_error_message "$(rpc_port "$validator_num")" 0 || true)"
    [[ "$error_message" == *"source"* || "$error_message" == *"segment"* ]] \
        || fail "Fresh verified-cache V3 did not fail closed during HTTPS source outage: ${error_message:-none}"
    verify_chain_producing "during authenticated Archive V2 HTTPS source outage" "$V1_RPC" 10
    start_archive_v2_https_source "$ARCHIVE_V2_HTTPS_SOURCE_ROOT"
    HTTPS_RECOVERED=false
    for _ in $(seq 1 60); do
        if [[ "$(archive_v2_rpc_block_hash "$(rpc_port "$validator_num")" 0 2>/dev/null || true)" == "$FRESH_ARCHIVE_V2_GENESIS_HASH" ]]; then
            HTTPS_RECOVERED=true
            break
        fi
        sleep 1
    done
    $HTTPS_RECOVERED \
        || fail "Fresh verified-cache V3 did not recover after authenticated HTTPS source returned"
    find "$LICHEN_LOCAL_ARCHIVE_V2_CACHE_ROOT_V3/objects" -type f -name '*.av2s' -print -quit \
        | grep -q . \
        || fail "Fresh verified-cache V3 did not persist the post-outage refetch"
    ok "Authenticated HTTPS source outage failed deep history closed without stopping consensus, then refetched"

    stop_validator_pid "${VALIDATOR_PIDS[$validator_num]:-}"
    wait_validator_resources_released "$validator_num" \
        || fail "V3 did not release resources before fresh consensus rejoin"
    reset_fresh_role_state_with_identity "$validator_num" "$identity_file" "$node_identity_file"
    LICHEN_LOCAL_ARCHIVE_V2_ROLE_V3="consensus"
    LICHEN_LOCAL_ARCHIVE_V2_ROOT_V3="$FRESH_ARCHIVE_V2_CONSENSUS_ROOT"
    role_log="/tmp/lichen-testnet/v3-fresh-consensus.log"
    start_archive_v2_validator "$validator_num" "$role_log"
    pid="$ARCHIVE_V2_STARTED_PID"
    VALIDATOR_PIDS[$validator_num]="$pid"
    wait_for_archive_v2_role_catchup "$validator_num" "consensus" "$pid" "$role_log"
    error_message="$(archive_v2_rpc_error_message "$(rpc_port "$validator_num")" 0 || true)"
    [[ "$error_message" == *"consensus"* ]] \
        || fail "Fresh consensus V3 served deep history or returned the wrong denial: ${error_message:-none}"
    recent_slot="$(get_slot "$(rpc_port "$validator_num")")"
    (( recent_slot > 1 )) || fail "Fresh consensus V3 has no recent history"
    archive_v2_rpc_block_hash "$(rpc_port "$validator_num")" "$((recent_slot - 1))" >/dev/null \
        || fail "Fresh consensus V3 did not serve recent hot history"
    ok "Fresh consensus join denied deep history and served its local recent window"

    stop_validator_pid "${VALIDATOR_PIDS[$validator_num]:-}"
    wait_validator_resources_released "$validator_num" \
        || fail "V3 did not release resources before restoring its full node-owned state"
    rm -rf "$(db_path "$validator_num")" "$(cold_path "$validator_num")"
    mv "$original_state" "$(db_path "$validator_num")"
    if [[ -d "$original_cold" ]]; then
        mv "$original_cold" "$(cold_path "$validator_num")"
    fi
    unset \
        LICHEN_LOCAL_ARCHIVE_V2_ROLE_V3 \
        LICHEN_LOCAL_ARCHIVE_V2_ROOT_V3 \
        LICHEN_LOCAL_ARCHIVE_V2_CACHE_ROOT_V3 \
        LICHEN_LOCAL_ARCHIVE_V2_SOURCE_DIRS_V3 \
        LICHEN_LOCAL_ARCHIVE_V2_SOURCE_URLS_V3 \
        LICHEN_LOCAL_ARCHIVE_V2_SOURCE_CA_CERT_V3 \
        LICHEN_LOCAL_ARCHIVE_V2_SOURCE_BEARER_TOKEN_V3
    role_log="/tmp/lichen-testnet/v3-restored-full-state.log"
    LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet "$validator_num" \
        > "$role_log" 2>&1 &
    pid=$!
    VALIDATOR_PIDS[$validator_num]="$pid"
    for i in $(seq 1 360); do
        sleep 2
        if ! kill -0 "$pid" 2>/dev/null; then
            tail -100 "$role_log"
            fail "V3 exited while restoring its original full node-owned state"
        fi
        recent_slot="$(get_slot "$(rpc_port "$validator_num")")"
        network_slot="$(get_slot "$V1_RPC")"
        if [[ "$recent_slot" -gt 0 && $((network_slot - recent_slot)) -le 20 ]]; then
            break
        fi
        [[ "$i" -lt 360 ]] || {
            tail -120 "$role_log"
            fail "V3 did not catch up after restoring its original full node-owned state"
        }
    done
    restored_pubkey="$(grep -m1 '"publicKeyBase58"' "$(db_path "$validator_num")/validator-keypair.json" \
        | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
    [[ "$restored_pubkey" == "${ALL_PUBKEYS[$((validator_num - 1))]}" ]] \
        || fail "V3 validator identity changed across fresh Archive V2 role rejoins"
    verify_chain_producing "after fresh Archive V2 role rejoin matrix" "$V1_RPC" 10
    ok "Fresh full/cache/consensus joins preserved validator identity and copied no mutable peer state"
}

archive_v2_rpc_block_hash() {
    local rpc=$1
    local slot=$2
    rpc_query_params_with_timeout \
        "$rpc" \
        getBlock \
        "[${slot}]" \
        "$ARCHIVE_V2_DEEP_HISTORY_RPC_TIMEOUT_SECS" | python3 -c '
import json
import sys

payload = json.load(sys.stdin)
result = payload.get("result")
if not isinstance(result, dict):
    raise SystemExit(1)
value = result.get("hash") or result.get("blockhash")
if not isinstance(value, str) or len(value) != 64:
    raise SystemExit(1)
print(value)
' 2>/dev/null
}

archive_v2_rpc_block_hash_with_retry() {
    local rpc=$1 slot=$2 attempts=${3:-10}
    local value
    for attempt in $(seq 1 "$attempts"); do
        value="$(archive_v2_rpc_block_hash "$rpc" "$slot" 2>/dev/null || true)"
        if [[ "$value" =~ ^[0-9a-fA-F]{64}$ ]]; then
            printf '%s\n' "$value"
            return 0
        fi
        (( attempt < attempts )) && sleep 1
    done
    return 1
}

archive_v2_rpc_error_message() {
    local rpc=$1
    local slot=$2
    rpc_query_params_with_timeout \
        "$rpc" \
        getBlock \
        "[${slot}]" \
        "$ARCHIVE_V2_DEEP_HISTORY_RPC_TIMEOUT_SECS" | python3 -c '
import json
import sys

payload = json.load(sys.stdin)
error = payload.get("error")
if not isinstance(error, dict):
    raise SystemExit(1)
message = error.get("message")
if not isinstance(message, str):
    raise SystemExit(1)
print(message)
' 2>/dev/null
}

restart_archive_v2_validator() {
    local validator_num=$1
    local reason=$2
    local restart_log="/tmp/lichen-testnet/v${validator_num}-archive-v2-${reason}.log"

    stop_validator_pid "${VALIDATOR_PIDS[$validator_num]:-}"
    wait_validator_resources_released "$validator_num" \
        || fail "V${validator_num} did not release resources for Archive V2 ${reason}"
    start_archive_v2_validator "$validator_num" "$restart_log" "$KEEP_CLUSTER_ON_SUCCESS"
    VALIDATOR_PIDS[$validator_num]="$ARCHIVE_V2_STARTED_PID"
    wait_for_existing_cluster_healthy 180 || {
        tail -100 "$restart_log"
        fail "V${validator_num} failed Archive V2 ${reason} restart"
    }
}

verify_archive_v2_runtime_role_matrix() {
    local genesis_hash=$1
    local v2_root="/tmp/lichen-testnet/archive-v2-v2"
    local v2_cache="/tmp/lichen-testnet/archive-v2-cache-v2"
    local v2_source="/tmp/lichen-testnet/archive-v2-replica-v2"
    local v2_source_offline="/tmp/lichen-testnet/archive-v2-replica-v2.offline"
    local v4_root="/tmp/lichen-testnet/archive-v2-v4"
    local v4_source="/tmp/lichen-testnet/archive-v2-replica-v4"
    local response error_message recent_slot before_slot after_slot corrupt_object
    local consensus_migrated_deep_slot=$((LICHEN_COLD_RETENTION_SLOTS / 2))

    [[ "$MAX_VALIDATORS" -ge 4 ]] \
        || fail "Archive V2 runtime role matrix requires the mandatory four-validator topology"
    rm -rf "$v2_cache" "$v2_source_offline"
    mkdir -p "$v2_root/objects"
    find "$v2_root/objects" -type f -name '*.av2s' -delete

    log "Starting Archive V2 role matrix: V1/V4 full, V2 verified-cache, V3 consensus..."
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        ROLE_LOG="/tmp/lichen-testnet/v${V_NUM}-archive-v2-role.log"
        start_archive_v2_validator "$V_NUM" "$ROLE_LOG" "$KEEP_CLUSTER_ON_SUCCESS"
        VALIDATOR_PIDS[$V_NUM]="$ARCHIVE_V2_STARTED_PID"
    done
    if ! wait_for_existing_cluster_healthy 180; then
        for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
            tail -80 "/tmp/lichen-testnet/v${V_NUM}-archive-v2-role.log"
        done
        fail "Archive V2 role matrix did not become healthy"
    fi

    [[ "$(archive_v2_rpc_block_hash "$(rpc_port 1)" 0)" == "$genesis_hash" ]] \
        || fail "Full-archive V1 did not serve verified genesis history"
    [[ "$(archive_v2_rpc_block_hash "$(rpc_port 4)" 0)" == "$genesis_hash" ]] \
        || fail "Full-archive V4 did not serve verified genesis history"

    stop_validator_pid "${VALIDATOR_PIDS[4]:-}"
    wait_validator_resources_released 4 \
        || fail "V4 did not release resources before segment-corruption recovery drill"
    corrupt_object="$(find "$v4_root/objects" -type f -name '*.av2s' -print -quit)"
    [[ -n "$corrupt_object" ]] || fail "V4 has no Archive V2 object to corrupt"
    truncate -s 64 "$corrupt_object"
    start_archive_v2_validator 4 "/tmp/lichen-testnet/v4-archive-v2-corrupt-segment.log" "$KEEP_CLUSTER_ON_SUCCESS"
    VALIDATOR_PIDS[4]="$ARCHIVE_V2_STARTED_PID"
    wait_for_existing_cluster_healthy 180 || {
        tail -100 /tmp/lichen-testnet/v4-archive-v2-corrupt-segment.log
        fail "V4 did not restart for the segment-corruption drill"
    }
    response="$(archive_v2_rpc_block_hash "$(rpc_port 4)" 0)" \
        || fail "Full-archive V4 did not preserve canonical legacy fallback while its Archive V2 segment was corrupt"
    [[ "$response" == "$genesis_hash" ]] \
        || fail "Full-archive V4 legacy fallback returned the wrong genesis hash while its Archive V2 segment was corrupt"
    find "$v4_root/quarantine" -type f -print -quit | grep -q . \
        || fail "Full-archive V4 did not quarantine its corrupt segment"
    ok "Corrupt full-archive segment was quarantined while the pre-retirement legacy source served matching canonical history"
    verify_chain_producing "while one full-archive segment is corrupt" "$V1_RPC" 10

    stop_validator_pid "${VALIDATOR_PIDS[4]:-}"
    wait_validator_resources_released 4 \
        || fail "V4 did not release resources before replica-backed segment repair"
    "$REPO_ROOT/target/release/lichen-archive-v2" mirror \
        --root "$v4_root" \
        --source "repair-source:region-source:${v4_source}" \
        --destination "repair-target:region-target:${v4_root}" \
        --journal "$v4_root/staging/repair-cli.journal" \
        --required-replicas 1 \
        --required-failure-domains 1 \
        --max-objects 1000 \
        --max-bytes 17179869184 >/dev/null \
        || fail "V4 segment repair from an independent replica failed"
    start_archive_v2_validator 4 "/tmp/lichen-testnet/v4-archive-v2-repaired-segment.log" "$KEEP_CLUSTER_ON_SUCCESS"
    VALIDATOR_PIDS[4]="$ARCHIVE_V2_STARTED_PID"
    wait_for_existing_cluster_healthy 180 || {
        tail -100 /tmp/lichen-testnet/v4-archive-v2-repaired-segment.log
        fail "V4 did not restart after replica-backed segment repair"
    }
    [[ "$(archive_v2_rpc_block_hash "$(rpc_port 4)" 0)" == "$genesis_hash" ]] \
        || fail "Full-archive V4 did not recover deep history from the repaired replica"
    ok "Corrupt full-archive segment was quarantined and recovered from another replica"

    recent_slot="$(get_slot "$(rpc_port 3)")"
    [[ "$recent_slot" -gt 0 ]] || fail "Consensus V3 has no recent hot-history tip"
    archive_v2_rpc_block_hash "$(rpc_port 3)" "$recent_slot" >/dev/null \
        || fail "Consensus V3 did not serve recent hot history"
    # This matrix reuses an established migration state, which deliberately
    # remains hot-first until it carries a durable fresh-sync admission marker.
    # Exercise a migrated deep slot here; the earlier fresh-role matrix
    # separately proves that an admitted consensus node denies genesis even
    # when bootstrap-only hot bytes remain.
    error_message="$(
        archive_v2_rpc_error_message \
            "$(rpc_port 3)" \
            "$consensus_migrated_deep_slot" \
            || true
    )"
    [[ "$error_message" == *"consensus"* ]] \
        || fail "Consensus V3 served migrated deep slot ${consensus_migrated_deep_slot} or returned the wrong denial: ${error_message:-none}"

    [[ "$(archive_v2_rpc_block_hash "$(rpc_port 2)" 0)" == "$genesis_hash" ]] \
        || fail "Verified-cache V2 did not fetch genesis from its authenticated source"
    find "$v2_cache/objects" -type f -name '*.av2s' -print -quit | grep -q . \
        || fail "Verified-cache V2 did not persist a verified cached object"

    stop_validator_pid "${VALIDATOR_PIDS[2]:-}"
    wait_validator_resources_released 2 \
        || fail "V2 did not release resources before cache-corruption drill"
    corrupt_object="$(find "$v2_cache/objects" -type f -name '*.av2s' -print -quit)"
    [[ -n "$corrupt_object" ]] || fail "V2 has no verified cached object to corrupt"
    truncate -s 64 "$corrupt_object"
    start_archive_v2_validator 2 "/tmp/lichen-testnet/v2-archive-v2-corrupt-cache.log" "$KEEP_CLUSTER_ON_SUCCESS"
    VALIDATOR_PIDS[2]="$ARCHIVE_V2_STARTED_PID"
    wait_for_existing_cluster_healthy 180 || {
        tail -100 /tmp/lichen-testnet/v2-archive-v2-corrupt-cache.log
        fail "V2 did not restart for cache-corruption recovery"
    }
    [[ "$(archive_v2_rpc_block_hash "$(rpc_port 2)" 0)" == "$genesis_hash" ]] \
        || fail "Verified-cache V2 did not quarantine and refetch its corrupt object"
    find "$v2_cache/quarantine" -type f -print -quit | grep -q . \
        || fail "Verified-cache V2 did not preserve corrupt cache evidence in quarantine"
    ok "Verified-cache corruption was quarantined and refetched from an authenticated source"

    mv "$v2_source" "$v2_source_offline"
    restart_archive_v2_validator 2 "cached-source-outage"
    [[ "$(archive_v2_rpc_block_hash "$(rpc_port 2)" 0)" == "$genesis_hash" ]] \
        || fail "Verified-cache V2 did not serve its verified disk cache during source outage"

    stop_validator_pid "${VALIDATOR_PIDS[2]:-}"
    wait_validator_resources_released 2 \
        || fail "V2 did not release resources before empty-cache source-outage test"
    find "$v2_cache/objects" -type f -name '*.av2s' -delete
    start_archive_v2_validator 2 "/tmp/lichen-testnet/v2-archive-v2-empty-cache-outage.log" "$KEEP_CLUSTER_ON_SUCCESS"
    VALIDATOR_PIDS[2]="$ARCHIVE_V2_STARTED_PID"
    wait_for_existing_cluster_healthy 180 || {
        tail -100 /tmp/lichen-testnet/v2-archive-v2-empty-cache-outage.log
        fail "V2 did not restart with an unavailable archive source"
    }
    response="$(rpc_query_params "$(rpc_port 2)" getBlock "[0]")"
    if python3 -c 'import json,sys; raise SystemExit(0 if isinstance(json.load(sys.stdin).get("result"), dict) else 1)' <<< "$response"; then
        fail "Verified-cache V2 served deep history with both cache and source unavailable"
    fi
    before_slot="$(get_slot "$V1_RPC")"
    verify_chain_producing "during verified-cache source outage" "$V1_RPC" 10
    after_slot="$(get_slot "$V1_RPC")"
    [[ "$after_slot" -gt "$before_slot" ]] \
        || fail "Consensus did not advance independently of the Archive V2 source outage"

    mv "$v2_source_offline" "$v2_source"
    [[ "$(archive_v2_rpc_block_hash "$(rpc_port 2)" 0)" == "$genesis_hash" ]] \
        || fail "Verified-cache V2 did not recover after its authenticated source returned"
    ok "Archive V2 full/cache/consensus roles, cache persistence, source outage isolation, and recovery passed"
}

wait_for_common_checkpoint() {
    local phase="${1:-parity}"
    local current_slot target_slot deadline all_ready
    current_slot="$(get_slot "$V1_RPC")"
    target_slot=$(( ((current_slot / CHECKPOINT_INTERVAL_SLOTS) + 1) * CHECKPOINT_INTERVAL_SLOTS ))
    deadline=$((SECONDS + 600))
    log "Advancing to common ${phase} checkpoint slot ${target_slot}..."

    while (( SECONDS < deadline )); do
        all_ready=1
        for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
            if [[ ! -f "$(db_path "$V_NUM")/checkpoints/slot-${target_slot}/checkpoint_meta.json" ]]; then
                all_ready=0
                break
            fi
        done
        if [[ "$all_ready" == "1" ]]; then
            COMMON_CHECKPOINT_SLOT="$target_slot"
            ok "All validators persisted ${phase} checkpoint slot ${target_slot}"
            return 0
        fi
        sleep 2
    done
    fail "Timed out waiting for all validators to persist ${phase} checkpoint slot ${target_slot}"
}

wait_for_archive_v2_retention_boundary() {
    local deadline=$((SECONDS + ARCHIVE_V2_RETENTION_TIMEOUT_SECS))
    local current_slot=0
    local progress_bucket=-1
    local available_kib=0
    log "Advancing beyond the exact ${LICHEN_COLD_RETENTION_SLOTS}-slot hot-retention boundary to slot ${ARCHIVE_V2_RETENTION_PROOF_SLOT}..."
    while (( SECONDS < deadline )); do
        current_slot="$(get_slot "$V1_RPC")"
        if [[ "$current_slot" -ge "$ARCHIVE_V2_RETENTION_PROOF_SLOT" ]]; then
            ok "Network crossed the Archive V2 retention boundary at slot ${current_slot}"
            return 0
        fi
        if (( current_slot / 5000 > progress_bucket )); then
            progress_bucket=$((current_slot / 5000))
            available_kib="$(df -Pk "$REPO_ROOT" | awk 'NR == 2 { print $4 }')"
            if [[ -z "$available_kib" || "$available_kib" -lt 15728640 ]]; then
                fail "Local disk headroom fell below the 15 GiB retention-gate floor"
            fi
            log "  Retention boundary progress: ${current_slot}/${ARCHIVE_V2_RETENTION_PROOF_SLOT}"
        fi
        sleep 2
    done
    fail "Timed out before the network crossed the Archive V2 retention boundary"
}

verify_bounded_cold_migration_progress() {
    local deadline=$((SECONDS + 180))
    local v1_status="" v2_status=""
    local v1_cursor v1_migrated v1_scanned v1_success v1_paused
    local v2_cursor v2_migrated v2_scanned v2_success v2_paused
    log "Verifying bounded cold migration continues independently of bounded reclaim..."

    while (( SECONDS < deadline )); do
        v1_status="$(rpc_query "$(rpc_port 1)" getHealth)"
        v2_status="$(rpc_query "$(rpc_port 2)" getHealth)"
        read -r v1_cursor v1_migrated v1_scanned v1_success v1_paused < <(
            python3 -c '
import json
import sys

status = json.load(sys.stdin).get("result", {}).get("archive_migration", {}).get("status", {})
print(
    status.get("cursor_slot") if status.get("cursor_slot") is not None else -1,
    status.get("migrated_rows", 0),
    status.get("scanned_rows", 0),
    status.get("last_success_unix_millis", 0) or 0,
    str(status.get("paused", True)).lower(),
)
' <<< "$v1_status"
        )
        read -r v2_cursor v2_migrated v2_scanned v2_success v2_paused < <(
            python3 -c '
import json
import sys

status = json.load(sys.stdin).get("result", {}).get("archive_migration", {}).get("status", {})
print(
    status.get("cursor_slot") if status.get("cursor_slot") is not None else -1,
    status.get("migrated_rows", 0),
    status.get("scanned_rows", 0),
    status.get("last_success_unix_millis", 0) or 0,
    str(status.get("paused", True)).lower(),
)
' <<< "$v2_status"
        )
        if [[ "$v1_cursor" -ge 0 && "$v2_cursor" -ge 0
            && "$v1_migrated" -gt 0 && "$v2_migrated" -gt 0
            && "$v1_scanned" -ge "$v1_migrated" && "$v2_scanned" -ge "$v2_migrated"
            && "$v1_success" -gt 0 && "$v2_success" -gt 0
            && "$v1_paused" == "false" && "$v2_paused" == "false" ]]; then
            local success_delta=$((v1_success - v2_success))
            (( success_delta < 0 )) && success_delta=$((-success_delta))
            [[ "$success_delta" -ge 100 ]] \
                || fail "V1/V2 cold migration completions synchronized within ${success_delta}ms"
            ok "Bounded cold migration advanced independently: V1 cursor=${v1_cursor} rows=${v1_migrated}; V2 cursor=${v2_cursor} rows=${v2_migrated}; completion delta=${success_delta}ms"
            verify_chain_producing "during bounded cold migration and deferred reclaim" "$V1_RPC" 10
            return 0
        fi
        sleep 2
    done

    warn "V1 Archive migration status: $v1_status"
    warn "V2 Archive migration status: $v2_status"
    fail "Bounded cold migration did not advance both durable cursors within 180s"
}

report_reused_cluster() {
    local primary_rpc
    primary_rpc="$(rpc_port 1)"
    local pass=true
    local activity_lines_found=0

    log "Reusing existing local cluster on RPC ports $(rpc_port 1), $(rpc_port 2), $(rpc_port 3)"

    if ! load_existing_cluster_pubkeys "$primary_rpc"; then
        fail "Could not load $MAX_VALIDATORS staked validator pubkeys from the running cluster"
    fi

    for n in $(seq 1 "$MAX_VALIDATORS"); do
        verify_chain_producing "existing cluster V${n}" "$(rpc_port "$n")" 5
    done
    COMMON_CHECKPOINT_SLOT=""
    wait_for_common_checkpoint "reused-cluster parity"
    verify_public_history_manifest_parity offline "$COMMON_CHECKPOINT_SLOT"

    while IFS='|' read -r pubkey produced votes last_active; do
        [[ -n "$pubkey" ]] || continue
        activity_lines_found=$((activity_lines_found + 1))
        if [[ "$produced" -gt 0 || "$votes" -gt 0 || "$last_active" -gt 0 ]]; then
            ok "Validator $pubkey active: proposed=$produced votes=$votes last_active=$last_active"
        else
            warn "Validator $pubkey has no observed activity on the running cluster"
            pass=false
        fi
    done < <(validator_activity_lines "$primary_rpc")

    if [[ "$activity_lines_found" -lt "$MAX_VALIDATORS" ]]; then
        fail "Could not load activity stats for all $MAX_VALIDATORS validators from the running cluster"
    fi

    echo ""
    log "═══════════════════════════════════════════════════════════"
    local final_slot final_vcnt
    final_slot=$(get_slot "$primary_rpc")
    final_vcnt=$(get_validator_count "$primary_rpc")
    ok "Slot: $final_slot"
    ok "Validators: $final_vcnt"
    for v_num in $(seq 1 "$MAX_VALIDATORS"); do
        ok "  V${v_num}: ${ALL_PUBKEYS[$((v_num - 1))]}"
    done
    echo ""
    if $pass; then
        ok "═══════════════════════════════════════════════════════════"
        ok "ALL TESTS PASSED: reused running $MAX_VALIDATORS-validator cluster"
        ok "═══════════════════════════════════════════════════════════"
    else
        fail "TEST FAILED: Running cluster does not show activity for every validator"
    fi
}

if [[ "$REUSE_EXISTING_CLUSTER" == "1" ]]; then
    if wait_for_existing_cluster_healthy "$REUSE_HEALTH_TIMEOUT_SECS"; then
        USING_EXISTING_CLUSTER=true
        declare -a ALL_PUBKEYS=()
        report_reused_cluster
        exit 0
    fi

    warn "Existing-cluster reuse never became healthy: $(existing_cluster_status_line)"
    for n in $(seq 1 "$MAX_VALIDATORS"); do
        local_log="$(cluster_log_path "$n")"
        if [[ -f "$local_log" ]]; then
            warn "V${n} log tail (${local_log}):"
            tail -20 "$local_log"
        fi
    done
    fail "Requested existing-cluster reuse, but the local stack did not become healthy within ${REUSE_HEALTH_TIMEOUT_SECS}s"
fi

if [[ "$SKIP_LOCAL_GATE_BUILD" == "1" ]]; then
    for binary in lichen lichen-genesis lichen-validator lichen-archive-v2; do
        [[ -x "$REPO_ROOT/target/release/$binary" ]] \
            || fail "LICHEN_SKIP_LOCAL_GATE_BUILD=1 requires target/release/$binary"
    done
    warn "Skipping release rebuild for a diagnostic run; this does not qualify as a release gate"
else
    log "Building the exact release candidate used by this gate..."
    "$REPO_ROOT/scripts/build-all-contracts.sh"
    cargo build --release --locked --bin lichen --bin lichen-genesis --bin lichen-validator --bin lichen-archive-v2
    ok "Release binaries and contract WASM are current"
fi

declare -a ALL_PUBKEYS=()
declare -a VALIDATOR_PIDS=()
V1_RPC="$(rpc_port 1)"
V1_LOG="$(log_path 1)"
VCNT=0
SLOT=0
STAKED_CNT=0
mkdir -p /tmp/lichen-testnet

if [[ "$RESUME_AFTER_PUBLIC_PARITY" == "1" ]]; then
    log "Resuming the exact gate from four independently owned states after proven immutable public-history parity..."
    stop_local_processes

    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        state_dir="$(db_path "$validator_num")"
        [[ -f "$state_dir/CURRENT" ]] \
            || fail "Public-parity resume requires V${validator_num}'s RocksDB CURRENT file"
        [[ -f "$state_dir/validator-keypair.json" ]] \
            || fail "Public-parity resume requires V${validator_num}'s node-owned validator identity"
        [[ -f "$state_dir/checkpoints/slot-${RESUME_PUBLIC_PARITY_CHECKPOINT}/checkpoint_meta.json" ]] \
            || fail "Public-parity resume requires V${validator_num} checkpoint ${RESUME_PUBLIC_PARITY_CHECKPOINT}"

        validator_pubkey="$(
            grep -m1 '"publicKeyBase58"' "$state_dir/validator-keypair.json" \
                | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/'
        )"
        [[ -n "$validator_pubkey" ]] \
            || fail "Public-parity resume could not read V${validator_num}'s validator identity"
        if [[ "$validator_num" -eq 1 ]]; then
            ALL_PUBKEYS=("$validator_pubkey")
        else
            for existing in "${ALL_PUBKEYS[@]}"; do
                [[ "$existing" != "$validator_pubkey" ]] \
                    || fail "Public-parity resume found duplicate validator identity $validator_pubkey"
            done
            ALL_PUBKEYS+=("$validator_pubkey")
        fi
    done
    V1_PUBKEY="${ALL_PUBKEYS[0]}"

    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        resume_log="/tmp/lichen-testnet/v${validator_num}-public-parity-resume.log"
        LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet "$validator_num" \
            > "$resume_log" 2>&1 &
        VALIDATOR_PIDS[$validator_num]=$!
    done
    if ! wait_for_existing_cluster_healthy "$ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS"; then
        for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
            tail -100 "/tmp/lichen-testnet/v${validator_num}-public-parity-resume.log"
        done
        fail "Public-parity resume did not restore a healthy four-validator cluster"
    fi

    ARCHIVE_V2_GENESIS_HASH="$(archive_v2_genesis_hash)" \
        || fail "Public-parity resume could not capture canonical genesis hash"
    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        [[ "$(archive_v2_rpc_block_hash_with_retry "$(rpc_port "$validator_num")" 0)" == "$ARCHIVE_V2_GENESIS_HASH" ]] \
            || fail "Public-parity resume found a V${validator_num} genesis mismatch"
    done
    verify_chain_producing "after public-parity exact-gate resume" "$V1_RPC" 10

    log "Stopping resumed validators before immutable public-history and Archive V2 tail checks..."
    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        signal_validator_pid_tree "${VALIDATOR_PIDS[$validator_num]:-}"
    done
    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        stop_validator_pid "${VALIDATOR_PIDS[$validator_num]:-}"
        wait_validator_resources_released "$validator_num" \
            || fail "V${validator_num} did not release resources for public-parity resume"
    done

    verify_public_history_manifest_parity offline "$RESUME_PUBLIC_PARITY_CHECKPOINT"
    verify_archive_v2_offline_matrix "$RESUME_PUBLIC_PARITY_CHECKPOINT" "$ARCHIVE_V2_GENESIS_HASH"
    verify_archive_v2_runtime_role_matrix "$ARCHIVE_V2_GENESIS_HASH"
    verify_chain_producing "after resumed Archive V2 role admission" "$V1_RPC" 10

    echo ""
    log "═══════════════════════════════════════════════════════════"
    ok "Slot: $RESUME_PUBLIC_PARITY_CHECKPOINT"
    ok "Validators: $MAX_VALIDATORS"
    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        ok "  V${validator_num}: ${ALL_PUBKEYS[$((validator_num - 1))]}"
    done
    echo ""
    ok "═══════════════════════════════════════════════════════════"
    ok "ALL RESUMED TAIL TESTS PASSED: immutable parity and Archive V2 role matrix"
    ok "═══════════════════════════════════════════════════════════"
    exit 0
elif [[ "$RESUME_AFTER_PHASE2" == "1" ]]; then
    log "Resuming the exact gate from independently owned V1/V2 state after proven Phase 2..."
    stop_local_processes

    for validator_num in 1 2; do
        state_dir="$(db_path "$validator_num")"
        [[ -f "$state_dir/CURRENT" ]] \
            || fail "Resume requires an existing V${validator_num} RocksDB CURRENT file"
        [[ -f "$state_dir/validator-keypair.json" ]] \
            || fail "Resume requires V${validator_num}'s node-owned validator identity"
    done
    for validator_num in $(seq 3 "$MAX_VALIDATORS"); do
        [[ ! -f "$(db_path "$validator_num")/CURRENT" ]] \
            || fail "Resume refuses copied or previously joined V${validator_num} chain state"
    done

    for validator_num in 1 2; do
        resume_log="$(log_path "$validator_num")"
        LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet "$validator_num" \
            >> "$resume_log" 2>&1 &
        VALIDATOR_PIDS[$validator_num]=$!
        validator_pubkey="$(
            grep -m1 '"publicKeyBase58"' "$(db_path "$validator_num")/validator-keypair.json" \
                | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/'
        )"
        [[ -n "$validator_pubkey" ]] \
            || fail "Resume could not read V${validator_num}'s validator identity"
        if [[ "$validator_num" -eq 1 ]]; then
            ALL_PUBKEYS=("$validator_pubkey")
        else
            for existing in "${ALL_PUBKEYS[@]}"; do
                [[ "$existing" != "$validator_pubkey" ]] \
                    || fail "Resume found duplicate validator identity $validator_pubkey"
            done
            ALL_PUBKEYS+=("$validator_pubkey")
        fi
    done
    V1_PUBKEY="${ALL_PUBKEYS[0]}"

    RESUME_READY=false
    resume_deadline=$((SECONDS + ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS))
    while (( SECONDS < resume_deadline )); do
        for validator_num in 1 2; do
            if ! kill -0 "${VALIDATOR_PIDS[$validator_num]}" 2>/dev/null; then
                tail -100 "$(log_path "$validator_num")"
                fail "V${validator_num} exited while resuming the exact gate"
            fi
        done
        v1_slot="$(get_slot "$(rpc_port 1)")"
        v2_slot="$(get_slot "$(rpc_port 2)")"
        staked_count="$(get_staked_validator_count "$V1_RPC")"
        v1_health="$(
            rpc_query "$(rpc_port 1)" getHealth \
                | python3 -c 'import json,sys; print(json.load(sys.stdin).get("result", {}).get("status", ""))' \
                2>/dev/null || true
        )"
        v2_health="$(
            rpc_query "$(rpc_port 2)" getHealth \
                | python3 -c 'import json,sys; print(json.load(sys.stdin).get("result", {}).get("status", ""))' \
                2>/dev/null || true
        )"
        if [[ "$v1_slot" -gt 0 && "$v2_slot" -gt 0
            && "$staked_count" -eq 2
            && "$v1_health" == "ok" && "$v2_health" == "ok"
            && $((v1_slot - v2_slot)) -le 20 && $((v2_slot - v1_slot)) -le 20 ]]; then
            RESUME_READY=true
            break
        fi
        sleep 2
    done
    $RESUME_READY \
        || fail "V1/V2 did not restore healthy, two-staked-validator Phase 2 state"

    v1_genesis_hash="$(archive_v2_rpc_block_hash "$(rpc_port 1)" 0)"
    v2_genesis_hash="$(archive_v2_rpc_block_hash "$(rpc_port 2)" 0)"
    [[ -n "$v1_genesis_hash" && "$v1_genesis_hash" == "$v2_genesis_hash" ]] \
        || fail "Resumed V1/V2 genesis identity differs"
    VCNT="$(get_validator_count "$V1_RPC")"
    [[ "$VCNT" -eq 2 ]] \
        || fail "Resume expected exactly two registered validators before fresh joins, found $VCNT"
    SLOT="$v1_slot"
    verify_chain_producing "after exact-gate Phase 2 resume" "$V1_RPC" 10
    ok "Resumed V1/V2 at node-owned state with matching genesis and healthy consensus"

    wait_for_archive_v2_retention_boundary
    verify_bounded_cold_migration_progress
    prepare_archive_v2_fresh_join_roots
    JOIN_START=3
else
# ═══════════════════════════════════════════════════════════════
# FLUSH: Clean all local state
# ═══════════════════════════════════════════════════════════════
log "Flushing local state..."
stop_local_processes
for n in $(seq 1 "$MAX_VALIDATORS"); do
    local_db="$(db_path $n)"
    if [[ -d "$local_db" ]]; then
        rm -rf "$local_db"
        log "  Flushed $local_db"
    fi
    local_cold="$(cold_path $n)"
    if [[ -d "$local_cold" ]]; then
        rm -rf "$local_cold"
        log "  Flushed $local_cold"
    fi
done
mkdir -p /tmp/lichen-testnet
ok "State flushed"

# ═══════════════════════════════════════════════════════════════
# PHASE 1: Start V1 (genesis)
# ═══════════════════════════════════════════════════════════════
log "═══════════════════════════════════════════════════════════"
log "PHASE 1: Starting V1 (genesis validator)"
log "═══════════════════════════════════════════════════════════"

LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet 1 \
    > "$V1_LOG" 2>&1 &
V1_PID=$!
log "V1 started (PID: $V1_PID)"

# Wait for V1 to produce blocks
log "Waiting for V1 to produce blocks..."
for i in $(seq 1 60); do
    sleep 2
    if ! kill -0 $V1_PID 2>/dev/null; then
        warn "V1 crashed! Log tail:"
        tail -30 "$V1_LOG"
        fail "V1 crashed during startup"
    fi
    SLOT=$(get_slot $V1_RPC)
    if [[ "$SLOT" -gt 3 ]]; then
        ok "V1 producing blocks! Slot: $SLOT"
        break
    fi
    [[ $i -lt 60 ]] || fail "V1 failed to produce blocks after 120s"
done

# Wait for V1 keypair to exist
for w in $(seq 1 10); do
    [[ -f "$(db_path 1)/validator-keypair.json" ]] && break
    sleep 1
done

# Extract V1 pubkey
V1_PUBKEY=$(grep -m1 '"publicKeyBase58"' "$(db_path 1)/validator-keypair.json" \
    | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')
ok "V1 pubkey: $V1_PUBKEY"

VCNT=$(get_validator_count $V1_RPC)
SLOT=$(get_slot $V1_RPC)
ok "Phase 1 complete: validators=$VCNT, slot=$SLOT"

if [[ "$VCNT" -ne 1 ]]; then
    warn "Expected 1 validator at genesis, got $VCNT"
    warn "This means the local node is leaking to production seeds!"
    fail "Validator count mismatch — check seeds.json isolation"
fi

if [[ "$MAX_VALIDATORS" -lt 2 ]]; then
    ok "PASS: Single validator test complete"
    exit 0
fi

# ═══════════════════════════════════════════════════════════════
# PHASE 2+: Add joining validators
# ═══════════════════════════════════════════════════════════════
ALL_PUBKEYS=("$V1_PUBKEY")
VALIDATOR_PIDS[1]="$V1_PID"
JOIN_START=2
fi

for V_NUM in $(seq "$JOIN_START" "$MAX_VALIDATORS"); do
    log "═══════════════════════════════════════════════════════════"
    log "PHASE ${V_NUM}: Adding V${V_NUM} to network"
    log "═══════════════════════════════════════════════════════════"

    V_RPC=$(rpc_port $V_NUM)
    V_LOG="$(log_path $V_NUM)"

    assert_joiner_starts_without_copied_chain_state "$V_NUM"

    role_override="LICHEN_LOCAL_ARCHIVE_V2_ROLE_V${V_NUM}"
    if [[ -n "${!role_override:-}" ]]; then
        start_archive_v2_validator "$V_NUM" "$V_LOG"
        V_PID="$ARCHIVE_V2_STARTED_PID"
    else
        LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet "$V_NUM" \
            > "$V_LOG" 2>&1 &
        V_PID=$!
    fi
    VALIDATOR_PIDS[$V_NUM]="$V_PID"
    log "V${V_NUM} started (PID: $V_PID)"

    # Wait for keypair file to be created
    V_KEYPAIR="$(db_path $V_NUM)/validator-keypair.json"
    for w in $(seq 1 30); do
        [[ -f "$V_KEYPAIR" ]] && break
        sleep 1
    done
    V_PUBKEY=$(grep -m1 '"publicKeyBase58"' "$V_KEYPAIR" 2>/dev/null \
        | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/' || echo "")

    if [[ -z "$V_PUBKEY" ]]; then
        fail "Could not extract V${V_NUM} pubkey"
    fi

    # Verify unique
    for existing in "${ALL_PUBKEYS[@]}"; do
        if [[ "$existing" == "$V_PUBKEY" ]]; then
            fail "V${V_NUM} has DUPLICATE pubkey $V_PUBKEY!"
        fi
    done
    ALL_PUBKEYS+=("$V_PUBKEY")
    ok "V${V_NUM} pubkey: $V_PUBKEY (unique)"

    # Wait for registration (staked, not just P2P routing entry)
    log "Waiting for V${V_NUM} to sync and register (with stake)..."
    REGISTERED=false
    REG_SLOT=0
    for i in $(seq 1 900); do
        sleep 2

        if ! kill -0 $V_PID 2>/dev/null; then
            warn "V${V_NUM} crashed! Log tail:"
            tail -30 "$V_LOG"
            fail "V${V_NUM} crashed"
        fi

        # Use STAKED count — validators with actual bootstrap grant, not routing entries
        STAKED_CNT=$(get_staked_validator_count $V1_RPC)
        VCNT=$(get_validator_count $V1_RPC)
        if [[ "$STAKED_CNT" -ge "$V_NUM" ]] && ! $REGISTERED; then
            REG_SLOT=$(get_slot $V1_RPC)
            ok "V${V_NUM} registered at slot ~$REG_SLOT! Staked: $STAKED_CNT, Routing: $VCNT"
            REGISTERED=true
            break
        fi

        # Progress every 30s
        if [[ $((i % 15)) -eq 0 ]]; then
            V_SLOT=$(get_slot $V_RPC)
            NET_SLOT=$(get_slot $V1_RPC)
            log "  V${V_NUM} slot=$V_SLOT network=$NET_SLOT staked=$STAKED_CNT routing=$VCNT"
        fi

        [[ $i -lt 900 ]] || {
            warn "V${V_NUM} log tail:"
            tail -40 "$V_LOG"
            fail "V${V_NUM} did not register after 1800s"
        }
    done

    # Verify chain didn't stall
    verify_chain_recovers_after_registration "during V${V_NUM} registration" "$V1_RPC"

    # Wait for activation warmup after registration.
    ACTIVATION_SLOT=$((REG_SLOT + WARMUP_SLOTS + 10))
    log "Waiting for warmup: activation after slot ~$ACTIVATION_SLOT..."
    for i in $(seq 1 600); do
        sleep 1
        NET_SLOT=$(get_slot $V1_RPC)
        if [[ "$NET_SLOT" -ge "$ACTIVATION_SLOT" ]]; then
            ok "Warmup done! Slot $NET_SLOT >= $ACTIVATION_SLOT"
            break
        fi
        if [[ $((i % 30)) -eq 0 ]]; then
            log "  Warmup: slot $NET_SLOT / $ACTIVATION_SLOT"
        fi
        if ! kill -0 $V_PID 2>/dev/null; then
            warn "V${V_NUM} crashed during warmup! Log tail:"
            tail -30 "$V_LOG"
            fail "V${V_NUM} crashed during warmup"
        fi
        [[ $i -lt 600 ]] || fail "Warmup exceeded 600s (slot $NET_SLOT / $ACTIVATION_SLOT)"
    done

    verify_chain_producing "V${V_NUM} post-activation" "$V1_RPC" 15

    ok "PHASE ${V_NUM} PASSED"
    if [[ "$V_NUM" -eq 2 && "$MAX_VALIDATORS" -ge 4 ]]; then
        wait_for_archive_v2_retention_boundary
        verify_bounded_cold_migration_progress
        prepare_archive_v2_fresh_join_roots
    elif [[ "$V_NUM" -eq 3 && "$MAX_VALIDATORS" -ge 4 ]]; then
        verify_fresh_archive_v2_role_rejoins
    fi
done

if [[ "$MAX_VALIDATORS" -ge 4 && "$SKIP_JOINER_RESTART_CHECK" != "1" ]]; then
    PAUSE_VALIDATOR_NUM="$MAX_VALIDATORS"
    PAUSE_PID="${VALIDATOR_PIDS[$PAUSE_VALIDATOR_NUM]:-}"
    PAUSE_RPC="$(rpc_port "$PAUSE_VALIDATOR_NUM")"
    PAUSE_LOG="$(log_path "$PAUSE_VALIDATOR_NUM")"

    log "═══════════════════════════════════════════════════════════"
    log "RG-401C: Pausing V${PAUSE_VALIDATOR_NUM} in LiveSync across a material gap"
    log "═══════════════════════════════════════════════════════════"

    PAUSE_START_SLOT="$(get_slot "$V1_RPC")"
    PAUSE_TARGET_SLOT=$((PAUSE_START_SLOT + LIVE_PAUSE_GAP_SLOTS))
    signal_validator_pid_tree "$PAUSE_PID" STOP

    PAUSE_GAP_READY=false
    for i in $(seq 1 180); do
        sleep 1
        if ! kill -0 "$PAUSE_PID" 2>/dev/null; then
            fail "V${PAUSE_VALIDATOR_NUM} exited while process-paused"
        fi
        NET_SLOT="$(get_slot "$V1_RPC")"
        if [[ "$NET_SLOT" -ge "$PAUSE_TARGET_SLOT" ]]; then
            PAUSE_GAP_READY=true
            break
        fi
        if [[ $((i % 20)) -eq 0 ]]; then
            log "  Paused V${PAUSE_VALIDATOR_NUM}: network=$NET_SLOT target=$PAUSE_TARGET_SLOT"
        fi
    done
    $PAUSE_GAP_READY || fail "Three-validator quorum did not advance across the material pause gap"

    signal_validator_pid_tree "$PAUSE_PID" CONT
    PAUSE_CAUGHT_UP=false
    for i in $(seq 1 180); do
        sleep 2
        if ! kill -0 "$PAUSE_PID" 2>/dev/null; then
            tail -60 "$PAUSE_LOG"
            fail "V${PAUSE_VALIDATOR_NUM} exited while recovering from the live pause gap"
        fi
        PAUSED_SLOT="$(get_slot "$PAUSE_RPC")"
        NET_SLOT="$(get_slot "$V1_RPC")"
        DRIFT=$((NET_SLOT - PAUSED_SLOT))
        if [[ "$PAUSED_SLOT" -gt "$PAUSE_START_SLOT" && "$DRIFT" -le 20 ]]; then
            PAUSE_CAUGHT_UP=true
            break
        fi
        if [[ $((i % 15)) -eq 0 ]]; then
            log "  Live-pause catch-up: V${PAUSE_VALIDATOR_NUM}=$PAUSED_SLOT network=$NET_SLOT drift=$DRIFT"
        fi
    done
    $PAUSE_CAUGHT_UP || {
        tail -80 "$PAUSE_LOG"
        fail "V${PAUSE_VALIDATOR_NUM} did not catch up in place after the live pause gap"
    }
    if grep -q "Sync phase: LiveSync -> InitialSync (material canonical gap)" "$PAUSE_LOG"; then
        ok "V${PAUSE_VALIDATOR_NUM} observed a material gap and entered bounded catch-up"
    else
        ok "V${PAUSE_VALIDATOR_NUM} consumed the retained contiguous P2P backlog without a gap transition"
    fi
    ok "V${PAUSE_VALIDATOR_NUM} caught up in the same process after a ${LIVE_PAUSE_GAP_SLOTS}-slot live gap"
    verify_chain_producing "after V${PAUSE_VALIDATOR_NUM} live-pause catch-up" "$V1_RPC" 10
fi

if [[ "$MAX_VALIDATORS" -ge 2 && "$SKIP_JOINER_RESTART_CHECK" != "1" ]]; then
    RESTART_VALIDATOR_NUM="$MAX_VALIDATORS"
    RESTART_RPC=$(rpc_port "$RESTART_VALIDATOR_NUM")
    RESTART_LOG="$(restart_log_path "$RESTART_VALIDATOR_NUM")"
    RESTART_KEYPAIR="$(db_path "$RESTART_VALIDATOR_NUM")/validator-keypair.json"
    RESTART_PUBKEY="${ALL_PUBKEYS[$((RESTART_VALIDATOR_NUM - 1))]}"
    OLD_PID="${VALIDATOR_PIDS[$RESTART_VALIDATOR_NUM]:-}"

    log "═══════════════════════════════════════════════════════════"
    log "RG-402A: Restarting V${RESTART_VALIDATOR_NUM} from its own local state"
    log "═══════════════════════════════════════════════════════════"

    BEFORE_NET_SLOT=$(get_slot "$V1_RPC")
    stop_validator_pid "$OLD_PID"
    if ! wait_validator_resources_released "$RESTART_VALIDATOR_NUM"; then
        tail -40 "$(log_path "$RESTART_VALIDATOR_NUM")"
        fail "V${RESTART_VALIDATOR_NUM} did not fully release process/port resources before restart"
    fi

    LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet "$RESTART_VALIDATOR_NUM" \
        > "$RESTART_LOG" 2>&1 &
    RESTART_PID=$!
    VALIDATOR_PIDS[$RESTART_VALIDATOR_NUM]="$RESTART_PID"

    RESTARTED=false
    for i in $(seq 1 120); do
        sleep 2

        if ! kill -0 "$RESTART_PID" 2>/dev/null; then
            warn "V${RESTART_VALIDATOR_NUM} crashed during restart! Log tail:"
            tail -40 "$RESTART_LOG"
            fail "V${RESTART_VALIDATOR_NUM} crashed during own-state restart"
        fi

        RESTART_SLOT=$(get_slot "$RESTART_RPC")
        NET_SLOT=$(get_slot "$V1_RPC")
        DRIFT=$((NET_SLOT - RESTART_SLOT))
        if [[ "$RESTART_SLOT" -gt 0 && "$DRIFT" -le 20 ]]; then
            ok "V${RESTART_VALIDATOR_NUM} restarted from own state and caught up: slot=$RESTART_SLOT network=$NET_SLOT drift=$DRIFT"
            RESTARTED=true
            break
        fi

        if [[ $((i % 15)) -eq 0 ]]; then
            log "  Restart catch-up: V${RESTART_VALIDATOR_NUM} slot=$RESTART_SLOT network=$NET_SLOT drift=$DRIFT"
        fi
    done

    $RESTARTED || {
        tail -40 "$RESTART_LOG"
        fail "V${RESTART_VALIDATOR_NUM} did not catch up from own local state after restart"
    }

    RESTARTED_PUBKEY=$(grep -m1 '"publicKeyBase58"' "$RESTART_KEYPAIR" \
        | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')
    [[ "$RESTARTED_PUBKEY" == "$RESTART_PUBKEY" ]] || fail "V${RESTART_VALIDATOR_NUM} pubkey changed after restart"

    if grep -q "Fresh node — will sync from existing network" "$RESTART_LOG"; then
        fail "V${RESTART_VALIDATOR_NUM} restart used fresh-join mode instead of resuming its own state"
    fi
    if grep -q "Applied canonical genesis state bundle from block 0" "$RESTART_LOG"; then
        fail "V${RESTART_VALIDATOR_NUM} restart re-imported genesis instead of resuming local state"
    fi

    ok "V${RESTART_VALIDATOR_NUM} restart preserved validator keypair and did not rejoin from copied or fresh state (network was at slot $BEFORE_NET_SLOT before restart)"
    verify_chain_producing "after V${RESTART_VALIDATOR_NUM} own-state restart" "$V1_RPC" 10
fi

if [[ "$MAX_VALIDATORS" -ge 4 && "$SKIP_JOINER_RESTART_CHECK" != "1" ]]; then
    SEED_RESTART_LOG="$(restart_log_path 1)"
    SEED_KEYPAIR="$(db_path 1)/validator-keypair.json"
    SEED_PUBKEY="${ALL_PUBKEYS[0]}"
    OLD_SEED_PID="${VALIDATOR_PIDS[1]:-}"
    V2_RPC=$(rpc_port 2)

    log "═══════════════════════════════════════════════════════════"
    log "RG-402B: Restarting V1 seed from its own local state"
    log "═══════════════════════════════════════════════════════════"

    BEFORE_SEED_STOP_SLOT=$(get_slot "$V2_RPC")
    stop_validator_pid "$OLD_SEED_PID"
    if ! wait_validator_resources_released 1; then
        tail -40 "$(log_path 1)"
        fail "V1 seed did not fully release process/port resources before restart"
    fi

    verify_chain_producing "while V1 seed is stopped" "$V2_RPC" 10

    LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet 1 \
        > "$SEED_RESTART_LOG" 2>&1 &
    SEED_RESTART_PID=$!
    VALIDATOR_PIDS[1]="$SEED_RESTART_PID"

    SEED_RESTARTED=false
    for i in $(seq 1 120); do
        sleep 2

        if ! kill -0 "$SEED_RESTART_PID" 2>/dev/null; then
            warn "V1 seed crashed during restart! Log tail:"
            tail -40 "$SEED_RESTART_LOG"
            fail "V1 seed crashed during own-state restart"
        fi

        SEED_SLOT=$(get_slot "$V1_RPC")
        NET_SLOT=$(get_slot "$V2_RPC")
        DRIFT=$((NET_SLOT - SEED_SLOT))
        if [[ "$SEED_SLOT" -gt 0 && "$DRIFT" -le 20 ]]; then
            ok "V1 seed restarted from own state and caught up: slot=$SEED_SLOT network=$NET_SLOT drift=$DRIFT"
            SEED_RESTARTED=true
            break
        fi

        if [[ $((i % 15)) -eq 0 ]]; then
            log "  Seed restart catch-up: V1 slot=$SEED_SLOT network=$NET_SLOT drift=$DRIFT"
        fi
    done

    $SEED_RESTARTED || {
        tail -40 "$SEED_RESTART_LOG"
        fail "V1 seed did not catch up from own local state after restart"
    }

    SEED_RESTARTED_PUBKEY=$(grep -m1 '"publicKeyBase58"' "$SEED_KEYPAIR" \
        | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')
    [[ "$SEED_RESTARTED_PUBKEY" == "$SEED_PUBKEY" ]] || fail "V1 seed pubkey changed after restart"

    if grep -q "Fresh node — will sync from existing network" "$SEED_RESTART_LOG"; then
        fail "V1 seed restart used fresh-join mode instead of resuming its own state"
    fi
    if grep -q "Applied canonical genesis state bundle from block 0" "$SEED_RESTART_LOG"; then
        fail "V1 seed restart re-imported genesis instead of resuming local state"
    fi

    ok "V1 seed restart preserved validator keypair and did not rejoin from copied or fresh state (network was at slot $BEFORE_SEED_STOP_SLOT before restart)"
    verify_chain_producing "after V1 seed own-state restart" "$V1_RPC" 10

    log "═══════════════════════════════════════════════════════════"
    log "RG-402C: Restarting all validators from the same preserved tip"
    log "═══════════════════════════════════════════════════════════"

    BEFORE_ALL_STOP_SLOT=$(get_slot "$V1_RPC")
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        stop_validator_pid "${VALIDATOR_PIDS[$V_NUM]:-}"
    done
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        if ! wait_validator_resources_released "$V_NUM"; then
            tail -40 "$(cluster_log_path "$V_NUM")"
            fail "V${V_NUM} did not release process/port resources before all-validator restart"
        fi
    done

    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        V_ALL_LOG="$(all_restart_log_path "$V_NUM")"
        LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet "$V_NUM" \
            > "$V_ALL_LOG" 2>&1 &
        VALIDATOR_PIDS[$V_NUM]=$!
        log "V${V_NUM} all-restart PID: ${VALIDATOR_PIDS[$V_NUM]}"
    done

    ALL_RESTARTED=false
    for i in $(seq 1 180); do
        sleep 2

        MAX_SLOT=0
        MIN_SLOT=999999999999
        LIVE_COUNT=0
        for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
            PID="${VALIDATOR_PIDS[$V_NUM]:-}"
            if ! kill -0 "$PID" 2>/dev/null; then
                warn "V${V_NUM} crashed during all-validator restart! Log tail:"
                tail -40 "$(all_restart_log_path "$V_NUM")"
                fail "V${V_NUM} crashed during all-validator own-state restart"
            fi

            V_SLOT=$(get_slot "$(rpc_port "$V_NUM")")
            if [[ "$V_SLOT" -gt 0 ]]; then
                LIVE_COUNT=$((LIVE_COUNT + 1))
                [[ "$V_SLOT" -gt "$MAX_SLOT" ]] && MAX_SLOT="$V_SLOT"
                [[ "$V_SLOT" -lt "$MIN_SLOT" ]] && MIN_SLOT="$V_SLOT"
            fi
        done

        SPREAD=$((MAX_SLOT - MIN_SLOT))
        if [[ "$LIVE_COUNT" -eq "$MAX_VALIDATORS" && "$MAX_SLOT" -gt "$BEFORE_ALL_STOP_SLOT" && "$SPREAD" -le 20 ]]; then
            ok "All validators restarted from preserved state and resumed finality: before=$BEFORE_ALL_STOP_SLOT max_slot=$MAX_SLOT min_slot=$MIN_SLOT spread=$SPREAD"
            ALL_RESTARTED=true
            break
        fi

        if [[ $((i % 15)) -eq 0 ]]; then
            log "  All-restart catch-up: live=$LIVE_COUNT/$MAX_VALIDATORS before=$BEFORE_ALL_STOP_SLOT max=$MAX_SLOT min=$MIN_SLOT spread=$SPREAD"
        fi
    done

    $ALL_RESTARTED || {
        for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
            warn "V${V_NUM} all-restart log tail:"
            tail -40 "$(all_restart_log_path "$V_NUM")"
        done
        fail "All-validator restart did not resume finality from preserved local state"
    }

    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        V_KEYPAIR="$(db_path "$V_NUM")/validator-keypair.json"
        EXPECTED_PUBKEY="${ALL_PUBKEYS[$((V_NUM - 1))]}"
        RESTARTED_PUBKEY=$(grep -m1 '"publicKeyBase58"' "$V_KEYPAIR" \
            | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')
        [[ "$RESTARTED_PUBKEY" == "$EXPECTED_PUBKEY" ]] || fail "V${V_NUM} pubkey changed after all-validator restart"

        V_ALL_LOG="$(all_restart_log_path "$V_NUM")"
        if grep -q "Fresh node — will sync from existing network" "$V_ALL_LOG"; then
            fail "V${V_NUM} all-validator restart used fresh-join mode instead of resuming its own state"
        fi
        if grep -q "Applied canonical genesis state bundle from block 0" "$V_ALL_LOG"; then
            fail "V${V_NUM} all-validator restart re-imported genesis instead of resuming local state"
        fi
    done

    verify_chain_producing "after all-validator own-state restart" "$V1_RPC" 10
fi

if [[ "$MAX_VALIDATORS" -ge 10 && "$SKIP_JOINER_RESTART_CHECK" != "1" ]]; then
    log "═══════════════════════════════════════════════════════════"
    log "RG-402D: Stopping V9 and V10 together; proving 10/8 liveness"
    log "═══════════════════════════════════════════════════════════"

    BEFORE_DOUBLE_STOP_SLOT=$(get_slot "$V1_RPC")
    for V_NUM in 9 10; do
        stop_validator_pid "${VALIDATOR_PIDS[$V_NUM]:-}"
    done
    for V_NUM in 9 10; do
        if ! wait_validator_resources_released "$V_NUM"; then
            tail -40 "$(cluster_log_path "$V_NUM")"
            fail "V${V_NUM} did not release resources for the 10/8 liveness gate"
        fi
    done

    verify_chain_producing "with V9 and V10 stopped (8/10 validators online)" "$V1_RPC" 15

    for V_NUM in 9 10; do
        DOUBLE_RESTART_LOG="/tmp/lichen-testnet/v${V_NUM}-double-restart.log"
        LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet "$V_NUM" \
            > "$DOUBLE_RESTART_LOG" 2>&1 &
        VALIDATOR_PIDS[$V_NUM]=$!
    done

    DOUBLE_RESTARTED=false
    for i in $(seq 1 150); do
        sleep 2
        BOTH_READY=true
        NET_SLOT=$(get_slot "$V1_RPC")
        for V_NUM in 9 10; do
            PID="${VALIDATOR_PIDS[$V_NUM]:-}"
            if ! kill -0 "$PID" 2>/dev/null; then
                tail -40 "/tmp/lichen-testnet/v${V_NUM}-double-restart.log"
                fail "V${V_NUM} crashed while recovering from the 10/8 liveness gate"
            fi
            V_SLOT=$(get_slot "$(rpc_port "$V_NUM")")
            DRIFT=$((NET_SLOT - V_SLOT))
            if [[ "$V_SLOT" -le 0 || "$DRIFT" -gt 20 ]]; then
                BOTH_READY=false
            fi
        done
        if $BOTH_READY; then
            DOUBLE_RESTARTED=true
            break
        fi
        if [[ $((i % 15)) -eq 0 ]]; then
            log "  Dual restart catch-up: V9=$(get_slot "$(rpc_port 9)") V10=$(get_slot "$(rpc_port 10)") network=$NET_SLOT"
        fi
    done

    $DOUBLE_RESTARTED || {
        tail -40 /tmp/lichen-testnet/v9-double-restart.log
        tail -40 /tmp/lichen-testnet/v10-double-restart.log
        fail "V9 and V10 did not recover from their preserved local states"
    }

    for V_NUM in 9 10; do
        EXPECTED_PUBKEY="${ALL_PUBKEYS[$((V_NUM - 1))]}"
        RESTARTED_PUBKEY=$(grep -m1 '"publicKeyBase58"' "$(db_path "$V_NUM")/validator-keypair.json" \
            | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')
        [[ "$RESTARTED_PUBKEY" == "$EXPECTED_PUBKEY" ]] || fail "V${V_NUM} pubkey changed after simultaneous restart"
        DOUBLE_RESTART_LOG="/tmp/lichen-testnet/v${V_NUM}-double-restart.log"
        if grep -q "Fresh node — will sync from existing network" "$DOUBLE_RESTART_LOG"; then
            fail "V${V_NUM} simultaneous restart used fresh-join mode"
        fi
        if grep -q "Applied canonical genesis state bundle from block 0" "$DOUBLE_RESTART_LOG"; then
            fail "V${V_NUM} simultaneous restart re-imported genesis"
        fi
    done

    ok "V9 and V10 resumed with preserved identities after 8/10 finality advanced from slot ${BEFORE_DOUBLE_STOP_SLOT}"
    verify_chain_producing "after V9 and V10 own-state recovery" "$V1_RPC" 10
    verify_canonical_commit_parity
fi

# ═══════════════════════════════════════════════════════════════
# FINAL: Verify ALL validators produce blocks
# ═══════════════════════════════════════════════════════════════
echo ""
log "═══════════════════════════════════════════════════════════"
log "FINAL: Verifying all validators produce blocks"
log "═══════════════════════════════════════════════════════════"

log "Letting network run 30s to accumulate production..."
sleep 30

PASS=true
FINAL_ACTIVITY_REFERENCE_SLOT=$(get_slot "$V1_RPC")
for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
    V_PUBKEY="${ALL_PUBKEYS[$((V_NUM - 1))]}"
    V_LOG="$(log_path $V_NUM)"
    V_RPC_SLOT=$(get_slot "$(rpc_port "$V_NUM")")
    V_RPC_DRIFT=$((FINAL_ACTIVITY_REFERENCE_SLOT - V_RPC_SLOT))
    if [[ "$V_RPC_DRIFT" -lt 0 ]]; then
        V_RPC_DRIFT=0
    fi
    if [[ "$V_RPC_SLOT" -le 0 || "$V_RPC_DRIFT" -gt 20 ]]; then
        warn "V${V_NUM} ($V_PUBKEY): stale RPC tip=$V_RPC_SLOT reference=$FINAL_ACTIVITY_REFERENCE_SLOT drift=$V_RPC_DRIFT"
        PASS=false
        continue
    fi
    ACTIVITY="$(validator_activity_for_pubkey "$V1_RPC" "$V_PUBKEY" || true)"

    IFS='|' read -r PRODUCED VOTES LAST_ACTIVE <<< "$ACTIVITY"
    PRODUCED="${PRODUCED:-0}"
    VOTES="${VOTES:-0}"
    LAST_ACTIVE="${LAST_ACTIVE:-0}"

    ACTIVITY_DRIFT=$((FINAL_ACTIVITY_REFERENCE_SLOT - LAST_ACTIVE))
    if [[ "$ACTIVITY_DRIFT" -lt 0 ]]; then
        ACTIVITY_DRIFT=0
    fi
    if [[ "$LAST_ACTIVE" -gt 0 && "$ACTIVITY_DRIFT" -gt 20 ]]; then
        warn "V${V_NUM} ($V_PUBKEY): stale consensus activity last_active=$LAST_ACTIVE reference=$FINAL_ACTIVITY_REFERENCE_SLOT drift=$ACTIVITY_DRIFT"
        PASS=false
    elif [[ "$PRODUCED" -gt 0 || "$VOTES" -gt 0 || "$LAST_ACTIVE" -gt 0 ]]; then
        ok "V${V_NUM} ($V_PUBKEY): proposed=$PRODUCED votes=$VOTES last_active=$LAST_ACTIVE"
    else
        PRODUCED=$(/usr/bin/grep -c "Produced block" "$V_LOG" 2>/dev/null || true)
        PRODUCED="${PRODUCED:-0}"
        if [[ "$PRODUCED" -gt 0 ]]; then
            ok "V${V_NUM} ($V_PUBKEY): produced=$PRODUCED blocks"
            continue
        fi

        # Log fallback for older validator builds that do not expose activity counters.
        PROPOSED=$(grep "proposer=$V_PUBKEY" "$(log_path 1)" 2>/dev/null | wc -l | tr -d ' ')
        if [[ "$PROPOSED" -gt 0 ]]; then
            ok "V${V_NUM} ($V_PUBKEY): proposed=$PROPOSED blocks (seen on V1)"
        else
            warn "V${V_NUM} ($V_PUBKEY): proposed=0 votes=0 last_active=0 — NOT producing!"
            tail -20 "$V_LOG"
            PASS=false
        fi
    fi
done

FINAL_SLOT=$(get_slot $V1_RPC)
FINAL_VCNT=$(get_validator_count $V1_RPC)
verify_canonical_commit_parity

if [[ "$USING_EXISTING_CLUSTER" == "true" ]]; then
    COMMON_CHECKPOINT_SLOT=""
    wait_for_common_checkpoint "reused-cluster parity"
    verify_public_history_manifest_parity offline "$COMMON_CHECKPOINT_SLOT"
else
    COMMON_CHECKPOINT_SLOT=""
    wait_for_common_checkpoint "pre-journey parity"
    PRE_JOURNEY_CHECKPOINT_SLOT="$COMMON_CHECKPOINT_SLOT"
    ARCHIVE_V2_GENESIS_HASH="$(archive_v2_genesis_hash)" \
        || fail "Could not capture canonical genesis hash for Archive V2 verification"
    log "Stopping validators before final public-history manifest parity check..."
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        signal_validator_pid_tree "${VALIDATOR_PIDS[$V_NUM]:-}"
    done
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        stop_validator_pid "${VALIDATOR_PIDS[$V_NUM]:-}"
    done
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        if ! wait_validator_resources_released "$V_NUM"; then
            fail "V${V_NUM} did not release process/port resources before offline archive parity check"
        fi
    done
    verify_public_history_manifest_parity offline "$PRE_JOURNEY_CHECKPOINT_SLOT"
    verify_archive_v2_offline_matrix "$PRE_JOURNEY_CHECKPOINT_SLOT" "$ARCHIVE_V2_GENESIS_HASH"
    verify_archive_v2_runtime_role_matrix "$ARCHIVE_V2_GENESIS_HASH"
    verify_chain_producing "after Archive V2 role admission" "$V1_RPC" 10

    if [[ "$RUN_VOLUME_E2E" == "1" ]]; then
        log "Running strict volume/user-journey E2E first so LP exercises an unfunded genesis AMM..."
        node "$REPO_ROOT/tests/e2e-volume.js"
        ok "Strict volume/user-journey E2E passed on ${MAX_VALIDATORS} validators"
    fi

    if [[ "$RUN_LAUNCHPAD_E2E" == "1" ]]; then
        log "Running launchpad graduation E2E against the verified ${MAX_VALIDATORS}-validator cluster..."
        node "$REPO_ROOT/tests/e2e-launchpad.js"
        ok "Launchpad graduation E2E passed on ${MAX_VALIDATORS} validators"
    fi

    if [[ "$MAX_VALIDATORS" -ge 4 && "$SKIP_JOINER_RESTART_CHECK" != "1" && ( "$RUN_LAUNCHPAD_E2E" == "1" || "$RUN_VOLUME_E2E" == "1" ) ]]; then
        POST_ACTIVITY_VALIDATOR_NUM="$MAX_VALIDATORS"
        POST_ACTIVITY_RPC="$(rpc_port "$POST_ACTIVITY_VALIDATOR_NUM")"
        POST_ACTIVITY_LOG="/tmp/lichen-testnet/v${POST_ACTIVITY_VALIDATOR_NUM}-post-activity-restart.log"
        POST_ACTIVITY_KEYPAIR="$(db_path "$POST_ACTIVITY_VALIDATOR_NUM")/validator-keypair.json"
        POST_ACTIVITY_PUBKEY="${ALL_PUBKEYS[$((POST_ACTIVITY_VALIDATOR_NUM - 1))]}"
        POST_ACTIVITY_OLD_PID="${VALIDATOR_PIDS[$POST_ACTIVITY_VALIDATOR_NUM]:-}"

        log "RG-402D: Restarting V${POST_ACTIVITY_VALIDATOR_NUM} from its own state after user activity"
        POST_ACTIVITY_START_SLOT="$(get_slot "$V1_RPC")"
        stop_validator_pid "$POST_ACTIVITY_OLD_PID"
        if ! wait_validator_resources_released "$POST_ACTIVITY_VALIDATOR_NUM"; then
            fail "V${POST_ACTIVITY_VALIDATOR_NUM} did not release resources before its post-activity restart"
        fi

        POST_ACTIVITY_ADVANCE_TARGET=$((POST_ACTIVITY_START_SLOT + 20))
        POST_ACTIVITY_ADVANCED=false
        for _ in $(seq 1 90); do
            NET_SLOT="$(get_slot "$V1_RPC")"
            if [[ "$NET_SLOT" -ge "$POST_ACTIVITY_ADVANCE_TARGET" ]]; then
                POST_ACTIVITY_ADVANCED=true
                break
            fi
            sleep 1
        done
        $POST_ACTIVITY_ADVANCED || fail "Three-validator quorum did not advance during the post-activity outage"

        start_archive_v2_validator "$POST_ACTIVITY_VALIDATOR_NUM" "$POST_ACTIVITY_LOG"
        POST_ACTIVITY_PID="$ARCHIVE_V2_STARTED_PID"
        VALIDATOR_PIDS[$POST_ACTIVITY_VALIDATOR_NUM]="$POST_ACTIVITY_PID"

        POST_ACTIVITY_CAUGHT_UP=false
        for i in $(seq 1 120); do
            sleep 2
            if ! kill -0 "$POST_ACTIVITY_PID" 2>/dev/null; then
                tail -80 "$POST_ACTIVITY_LOG"
                fail "V${POST_ACTIVITY_VALIDATOR_NUM} exited during its post-activity restart"
            fi
            RESTART_SLOT="$(get_slot "$POST_ACTIVITY_RPC")"
            NET_SLOT="$(get_slot "$V1_RPC")"
            DRIFT=$((NET_SLOT - RESTART_SLOT))
            if [[ "$RESTART_SLOT" -gt "$POST_ACTIVITY_START_SLOT" && "$DRIFT" -le 20 ]]; then
                POST_ACTIVITY_CAUGHT_UP=true
                break
            fi
            if [[ $((i % 15)) -eq 0 ]]; then
                log "  Post-activity restart catch-up: V${POST_ACTIVITY_VALIDATOR_NUM}=$RESTART_SLOT network=$NET_SLOT drift=$DRIFT"
            fi
        done
        $POST_ACTIVITY_CAUGHT_UP || {
            tail -100 "$POST_ACTIVITY_LOG"
            fail "V${POST_ACTIVITY_VALIDATOR_NUM} did not catch up from its own post-activity state"
        }

        RESTARTED_PUBKEY=$(grep -m1 '"publicKeyBase58"' "$POST_ACTIVITY_KEYPAIR" \
            | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')
        [[ "$RESTARTED_PUBKEY" == "$POST_ACTIVITY_PUBKEY" ]] || fail "V${POST_ACTIVITY_VALIDATOR_NUM} pubkey changed after post-activity restart"
        if grep -q "Fresh node — will sync from existing network" "$POST_ACTIVITY_LOG"; then
            fail "V${POST_ACTIVITY_VALIDATOR_NUM} post-activity restart used fresh-join mode"
        fi
        if grep -q "Applied canonical genesis state bundle from block 0" "$POST_ACTIVITY_LOG"; then
            fail "V${POST_ACTIVITY_VALIDATOR_NUM} post-activity restart re-imported genesis"
        fi
        verify_chain_producing "after post-activity own-state restart" "$V1_RPC" 10
        verify_canonical_commit_parity
        ok "V${POST_ACTIVITY_VALIDATOR_NUM} restarted after real user activity, caught up from preserved state, and retained canonical parity"
    fi

    if [[ "$RUN_LAUNCHPAD_E2E" == "1" || "$RUN_VOLUME_E2E" == "1" ]]; then
        FINAL_SLOT=$(get_slot "$V1_RPC")
        FINAL_VCNT=$(get_validator_count "$V1_RPC")
        COMMON_CHECKPOINT_SLOT=""
        wait_for_common_checkpoint "post-journey parity"
        POST_JOURNEY_CHECKPOINT_SLOT="$COMMON_CHECKPOINT_SLOT"
        log "Stopping validators for post-journey public-history parity..."
        for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
            signal_validator_pid_tree "${VALIDATOR_PIDS[$V_NUM]:-}"
        done
        for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
            stop_validator_pid "${VALIDATOR_PIDS[$V_NUM]:-}"
        done
        for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
            if ! wait_validator_resources_released "$V_NUM"; then
                fail "V${V_NUM} did not release resources before post-journey archive parity"
            fi
        done
        verify_public_history_manifest_parity offline "$POST_JOURNEY_CHECKPOINT_SLOT"
        ok "Post-journey public-history manifests match across ${MAX_VALIDATORS} validators"

        if [[ "$KEEP_CLUSTER_ON_SUCCESS" == "1" ]]; then
            log "Restarting verified cluster after post-journey parity..."
            for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
                FINAL_E2E_LOG="/tmp/lichen-testnet/v${V_NUM}-post-journey.log"
                start_archive_v2_validator "$V_NUM" "$FINAL_E2E_LOG" 1
                VALIDATOR_PIDS[$V_NUM]="$ARCHIVE_V2_STARTED_PID"
            done
            if ! wait_for_existing_cluster_healthy 180; then
                fail "Post-journey parity restart did not restore a healthy cluster"
            fi
            verify_chain_producing "after post-journey parity restart" "$V1_RPC" 10
        fi
    fi
fi

# ═══════════════════════════════════════════════════════════════
# REPORT
# ═══════════════════════════════════════════════════════════════
if [[ -n "${POST_JOURNEY_CHECKPOINT_SLOT:-}" ]]; then
    FINAL_SLOT="$POST_JOURNEY_CHECKPOINT_SLOT"
elif [[ "$KEEP_CLUSTER_ON_SUCCESS" == "1" ]]; then
    FINAL_SLOT=$(get_slot "$V1_RPC")
    FINAL_VCNT=$(get_validator_count "$V1_RPC")
fi
echo ""
log "═══════════════════════════════════════════════════════════"
ok "Slot: $FINAL_SLOT"
ok "Validators: $FINAL_VCNT"
for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
    ok "  V${V_NUM}: ${ALL_PUBKEYS[$((V_NUM - 1))]}"
done
echo ""
if $PASS; then
    ok "═══════════════════════════════════════════════════════════"
    ok "ALL TESTS PASSED: $MAX_VALIDATORS validators, ALL producing"
    ok "═══════════════════════════════════════════════════════════"
else
    fail "TEST FAILED: Not all validators are producing blocks!"
fi
