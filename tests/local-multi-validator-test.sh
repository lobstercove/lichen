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
SKIP_JOINER_RESTART_CHECK="${LICHEN_SKIP_JOINER_RESTART_CHECK:-0}"
KEEP_CLUSTER_ON_SUCCESS="${LICHEN_KEEP_CLUSTER_ON_SUCCESS:-0}"
RUN_LAUNCHPAD_E2E="${LICHEN_RUN_LAUNCHPAD_E2E:-0}"
RUN_VOLUME_E2E="${LICHEN_RUN_VOLUME_E2E:-0}"
SKIP_LOCAL_GATE_BUILD="${LICHEN_SKIP_LOCAL_GATE_BUILD:-0}"
LIVE_PAUSE_GAP_SLOTS="${LICHEN_LIVE_PAUSE_GAP_SLOTS:-140}"

export LICHEN_LOCAL_DEV=1
export LICHEN_LOCAL_ARCHIVE_COLD="${LICHEN_LOCAL_ARCHIVE_COLD:-1}"
export LICHEN_COLD_RETENTION_SLOTS="${LICHEN_COLD_RETENTION_SLOTS:-20}"
export LICHEN_COLD_MIGRATION_INTERVAL_SECS="${LICHEN_COLD_MIGRATION_INTERVAL_SECS:-5}"
CHECKPOINT_INTERVAL_SLOTS=1000

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

get_slot() {
    rpc_query "$1" "getSlot" | python3 -c "import json,sys; print(json.load(sys.stdin).get('result',0))" 2>/dev/null || echo 0
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
    s1=$(get_slot "$rpc")
    sleep "$seconds"
    s2=$(get_slot "$rpc")
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
    for binary in lichen lichen-genesis lichen-validator; do
        [[ -x "$REPO_ROOT/target/release/$binary" ]] \
            || fail "LICHEN_SKIP_LOCAL_GATE_BUILD=1 requires target/release/$binary"
    done
    warn "Skipping release rebuild for a diagnostic run; this does not qualify as a release gate"
else
    log "Building the exact release candidate used by this gate..."
    "$REPO_ROOT/scripts/build-all-contracts.sh"
    cargo build --release --locked --bin lichen --bin lichen-genesis --bin lichen-validator
    ok "Release binaries and contract WASM are current"
fi

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

V1_RPC=$(rpc_port 1)
V1_LOG="$(log_path 1)"

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
declare -a ALL_PUBKEYS=("$V1_PUBKEY")
declare -a VALIDATOR_PIDS=()
VALIDATOR_PIDS[1]="$V1_PID"

for V_NUM in $(seq 2 "$MAX_VALIDATORS"); do
    log "═══════════════════════════════════════════════════════════"
    log "PHASE ${V_NUM}: Adding V${V_NUM} to network"
    log "═══════════════════════════════════════════════════════════"

    V_RPC=$(rpc_port $V_NUM)
    V_LOG="$(log_path $V_NUM)"

    assert_joiner_starts_without_copied_chain_state "$V_NUM"

    LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet "$V_NUM" \
        > "$V_LOG" 2>&1 &
    V_PID=$!
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
    for i in $(seq 1 300); do
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

        [[ $i -lt 300 ]] || {
            warn "V${V_NUM} log tail:"
            tail -40 "$V_LOG"
            fail "V${V_NUM} did not register after 600s"
        }
    done

    # Verify chain didn't stall
    verify_chain_producing "during V${V_NUM} registration" "$V1_RPC" 10

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

    if [[ "$KEEP_CLUSTER_ON_SUCCESS" == "1" || "$RUN_LAUNCHPAD_E2E" == "1" || "$RUN_VOLUME_E2E" == "1" ]]; then
        log "Restarting all validators from preserved state after offline parity..."
        for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
            E2E_LOG="/tmp/lichen-testnet/v${V_NUM}-post-parity.log"
            if [[ "$KEEP_CLUSTER_ON_SUCCESS" == "1" ]]; then
                nohup env LICHEN_DISABLE_SUPERVISOR=1 \
                    "$REPO_ROOT/run-validator.sh" testnet "$V_NUM" \
                    </dev/null > "$E2E_LOG" 2>&1 &
            else
                LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet "$V_NUM" \
                    > "$E2E_LOG" 2>&1 &
            fi
            VALIDATOR_PIDS[$V_NUM]=$!
        done
        if ! wait_for_existing_cluster_healthy 180; then
            fail "Post-parity validator restart did not restore a healthy cluster"
        fi
        verify_chain_producing "after post-parity restart" "$V1_RPC" 10
    fi

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
                nohup env LICHEN_DISABLE_SUPERVISOR=1 \
                    "$REPO_ROOT/run-validator.sh" testnet "$V_NUM" \
                    </dev/null > "$FINAL_E2E_LOG" 2>&1 &
                VALIDATOR_PIDS[$V_NUM]=$!
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
