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
# Restart-readiness acceptance consumes INFO-level validator admission markers.
# Keep that evidence deterministic instead of inheriting a quieter operator or
# CI shell; callers can still request a more verbose test filter explicitly.
export RUST_LOG="${LICHEN_TEST_RUST_LOG:-info}"

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUILD_TARGET_DIR="${CARGO_TARGET_DIR:-${REPO_ROOT}/target}"
if [[ "$BUILD_TARGET_DIR" != /* ]]; then
    BUILD_TARGET_DIR="${REPO_ROOT}/${BUILD_TARGET_DIR}"
fi
RELEASE_BIN_DIR="${BUILD_TARGET_DIR}/release"
MAX_VALIDATORS="${1:-4}"
export LICHEN_LOCAL_VALIDATOR_COUNT="$MAX_VALIDATORS"
export LICHEN_LOCAL_GENESIS_VALIDATOR_COUNT="$MAX_VALIDATORS"
WARMUP_SLOTS=100  # Must match ACTIVATION_WARMUP in validator/src/main.rs
REUSE_EXISTING_CLUSTER="${LICHEN_REUSE_EXISTING_CLUSTER:-0}"
REUSE_HEALTH_TIMEOUT_SECS="${LICHEN_REUSE_HEALTH_TIMEOUT_SECS:-120}"
USING_EXISTING_CLUSTER=false
RESUME_AFTER_PHASE2="${LICHEN_RESUME_LOCAL_GATE_AFTER_PHASE2:-0}"
RESUME_AFTER_RETENTION="${LICHEN_RESUME_LOCAL_GATE_AFTER_RETENTION:-0}"
RESUME_AFTER_RESILIENCE="${LICHEN_RESUME_LOCAL_GATE_AFTER_RESILIENCE:-0}"
RESUME_AFTER_PUBLIC_PARITY="${LICHEN_RESUME_LOCAL_GATE_AFTER_PUBLIC_PARITY:-0}"
RESUME_AFTER_ARCHIVE_V2_OFFLINE_MATRIX="${LICHEN_RESUME_LOCAL_GATE_AFTER_ARCHIVE_V2_OFFLINE_MATRIX:-0}"
RESUME_AFTER_ARCHIVE_V2_RUNTIME_MATRIX="${LICHEN_RESUME_LOCAL_GATE_AFTER_ARCHIVE_V2_RUNTIME_MATRIX:-0}"
RESUME_AFTER_ARCHIVE_V2_COMMON_CATALOG="${LICHEN_RESUME_LOCAL_GATE_AFTER_ARCHIVE_V2_COMMON_CATALOG:-0}"
RESUME_AFTER_ARCHIVE_V2_CHECKPOINT="${LICHEN_RESUME_LOCAL_GATE_AFTER_ARCHIVE_V2_CHECKPOINT:-0}"
RESUME_PUBLIC_PARITY_CHECKPOINT="${LICHEN_RESUME_LOCAL_GATE_CHECKPOINT_SLOT:-}"
RESUME_EXPECTED_ARCHIVE_V2_ROOT="${LICHEN_RESUME_LOCAL_GATE_EXPECTED_ARCHIVE_V2_ROOT:-}"
SKIP_JOINER_RESTART_CHECK="${LICHEN_SKIP_JOINER_RESTART_CHECK:-0}"
KEEP_CLUSTER_ON_SUCCESS="${LICHEN_KEEP_CLUSTER_ON_SUCCESS:-0}"
RUN_LAUNCHPAD_E2E="${LICHEN_RUN_LAUNCHPAD_E2E:-0}"
RUN_VOLUME_E2E="${LICHEN_RUN_VOLUME_E2E:-0}"
SKIP_LOCAL_GATE_BUILD="${LICHEN_SKIP_LOCAL_GATE_BUILD:-0}"
LIVE_PAUSE_GAP_SLOTS="${LICHEN_LIVE_PAUSE_GAP_SLOTS:-140}"
BACKLOG_REGRESSION_TXS="${LICHEN_BACKLOG_REGRESSION_TXS:-96}"
ARCHIVE_V2_HTTPS_SOURCE_PID=""
ARCHIVE_V2_HTTPS_SOURCE_ROOT=""
ARCHIVE_V2_HTTPS_SOURCE_CA=""
ARCHIVE_V2_HTTPS_SOURCE_CERT=""
ARCHIVE_V2_HTTPS_SOURCE_KEY=""
ARCHIVE_V2_HTTPS_SOURCE_LOG="/tmp/lichen-testnet/archive-v2-https-source.log"
ARCHIVE_V2_HTTPS_SOURCE_PORT=9443
ARCHIVE_V2_HTTPS_SOURCE_TOKEN="local-archive-v2-gate-token"
LOCAL_GATE_LOCK_DIR="${TMPDIR:-/tmp}/lichen-local-multi-validator-test.lock"
LOCAL_GATE_LOCK_HELD=0
LOCAL_GATE_SUCCESS=0
GENESIS_QUORUM_BOOTSTRAP=0
FRESH_ROLE_SWAP_ACTIVE=0
FRESH_ROLE_SWAP_VALIDATOR_NUM=""
FRESH_ROLE_ORIGINAL_STATE=""
FRESH_ROLE_ORIGINAL_COLD=""
ARCHIVE_V2_CHECKPOINT_CATALOG_END=""
ARCHIVE_V2_PLANNED_CHECKPOINT_SLOT=""
ARCHIVE_V2_FRESH_JOIN_SOURCE_ROOT=""
ARCHIVE_V2_FRESH_JOIN_REPLICA_ROOT=""
ARCHIVE_V2_CHECKPOINT_BOUND_CATALOGS=0
CHECKPOINT_SOURCE_PEERS_ACTIVE=0

export LICHEN_LOCAL_DEV=1
export LICHEN_LOCAL_ARCHIVE_COLD="${LICHEN_LOCAL_ARCHIVE_COLD:-1}"
export LICHEN_COLD_RETENTION_SLOTS="${LICHEN_COLD_RETENTION_SLOTS:-50000}"
export LICHEN_COLD_MIGRATION_INTERVAL_SECS="${LICHEN_COLD_MIGRATION_INTERVAL_SECS:-5}"
export LICHEN_LOCAL_SLOT_DURATION_MS="${LICHEN_LOCAL_SLOT_DURATION_MS:-5}"
export LICHEN_CHECKPOINT_HOT_REPAIR_PREACTIVATION=1
# Four independent validators must not each auto-size their RocksDB cache from
# the host's full memory. The gate owns a bounded aggregate cache budget so its
# Archive V2 snapshot/join phases remain valid on a 16 GB development host.
export LICHEN_LOCAL_CACHE_SIZE_MB="${LICHEN_LOCAL_CACHE_SIZE_MB:-64}"
if [[ ! "$LICHEN_LOCAL_CACHE_SIZE_MB" =~ ^[0-9]+$ ]] \
    || (( 10#$LICHEN_LOCAL_CACHE_SIZE_MB < 16 || 10#$LICHEN_LOCAL_CACHE_SIZE_MB > 4096 )); then
    echo "LICHEN_LOCAL_CACHE_SIZE_MB must be an integer within 16..4096" >&2
    exit 2
fi
# Deep accelerated joins must retain the immutable checkpoint for the entire
# exact verification + transfer. The production default remains two; eight is
# within the validator's validated maximum and is local-gate-only.
export LICHEN_CHECKPOINT_KEEP_COUNT="${LICHEN_CHECKPOINT_KEEP_COUNT:-8}"
# The gate explicitly opts into the bounded hot-repair profile before Archive
# V2 role admission. Preactivation checkpoints remain reachable on the legacy
# 1,000-slot cadence; catalog-bound checkpoints use the 10,000-slot production
# cadence to bound physical compaction frequency.
PREACTIVATION_CHECKPOINT_INTERVAL_SLOTS=1000
CHECKPOINT_INTERVAL_SLOTS=10000
ARCHIVE_V2_PUBLIC_MIN_RECENT_HISTORY_SLOTS=50000
ARCHIVE_V2_RETENTION_PROOF_SLOT=$((LICHEN_COLD_RETENTION_SLOTS + 2500))
ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS="${LICHEN_ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS:-10000}"
ARCHIVE_V2_FRESH_JOIN_CATALOG_HEADROOM_SLOTS="${LICHEN_ARCHIVE_V2_FRESH_JOIN_CATALOG_HEADROOM_SLOTS:-40000}"
# Production validators retain the 50,000-slot default. This accelerated
# local-dev gate scales the same overlap invariant to half of one 10,000-slot
# catalog-bound checkpoint interval unless a caller requests a larger window.
ARCHIVE_V2_FRESH_JOIN_RECENT_HISTORY_SLOTS="${LICHEN_ARCHIVE_V2_FRESH_JOIN_RECENT_HISTORY_SLOTS:-5000}"
ARCHIVE_V2_RETENTION_TIMEOUT_SECS="${LICHEN_ARCHIVE_V2_RETENTION_TIMEOUT_SECS:-21600}"
ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS="${LICHEN_ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS:-1800}"
ARCHIVE_V2_CHECKPOINT_MATERIALIZATION_TIMEOUT_SECS="${LICHEN_ARCHIVE_V2_CHECKPOINT_MATERIALIZATION_TIMEOUT_SECS:-1800}"
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
if (( ARCHIVE_V2_FRESH_JOIN_RECENT_HISTORY_SLOTS < CHECKPOINT_INTERVAL_SLOTS / 2 )); then
    echo "LICHEN_ARCHIVE_V2_FRESH_JOIN_RECENT_HISTORY_SLOTS must cover at least half of one ${CHECKPOINT_INTERVAL_SLOTS}-slot checkpoint interval" >&2
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
if [[ "$RESUME_AFTER_RETENTION" != "0" && "$RESUME_AFTER_RETENTION" != "1" ]]; then
    echo "LICHEN_RESUME_LOCAL_GATE_AFTER_RETENTION must be 0 or 1" >&2
    exit 2
fi
if [[ "$RESUME_AFTER_RESILIENCE" != "0" && "$RESUME_AFTER_RESILIENCE" != "1" ]]; then
    echo "LICHEN_RESUME_LOCAL_GATE_AFTER_RESILIENCE must be 0 or 1" >&2
    exit 2
fi
if [[ "$RESUME_AFTER_PUBLIC_PARITY" != "0" && "$RESUME_AFTER_PUBLIC_PARITY" != "1" ]]; then
    echo "LICHEN_RESUME_LOCAL_GATE_AFTER_PUBLIC_PARITY must be 0 or 1" >&2
    exit 2
fi
if [[ "$RESUME_AFTER_ARCHIVE_V2_OFFLINE_MATRIX" != "0" && "$RESUME_AFTER_ARCHIVE_V2_OFFLINE_MATRIX" != "1" ]]; then
    echo "LICHEN_RESUME_LOCAL_GATE_AFTER_ARCHIVE_V2_OFFLINE_MATRIX must be 0 or 1" >&2
    exit 2
fi
if [[ "$RESUME_AFTER_ARCHIVE_V2_RUNTIME_MATRIX" != "0" && "$RESUME_AFTER_ARCHIVE_V2_RUNTIME_MATRIX" != "1" ]]; then
    echo "LICHEN_RESUME_LOCAL_GATE_AFTER_ARCHIVE_V2_RUNTIME_MATRIX must be 0 or 1" >&2
    exit 2
fi
if [[ "$RESUME_AFTER_ARCHIVE_V2_COMMON_CATALOG" != "0" && "$RESUME_AFTER_ARCHIVE_V2_COMMON_CATALOG" != "1" ]]; then
    echo "LICHEN_RESUME_LOCAL_GATE_AFTER_ARCHIVE_V2_COMMON_CATALOG must be 0 or 1" >&2
    exit 2
fi
if [[ "$RESUME_AFTER_ARCHIVE_V2_CHECKPOINT" != "0" && "$RESUME_AFTER_ARCHIVE_V2_CHECKPOINT" != "1" ]]; then
    echo "LICHEN_RESUME_LOCAL_GATE_AFTER_ARCHIVE_V2_CHECKPOINT must be 0 or 1" >&2
    exit 2
fi
if (( 10#$RESUME_AFTER_PHASE2 + 10#$RESUME_AFTER_RETENTION + 10#$RESUME_AFTER_RESILIENCE + 10#$RESUME_AFTER_PUBLIC_PARITY + 10#$RESUME_AFTER_ARCHIVE_V2_OFFLINE_MATRIX + 10#$RESUME_AFTER_ARCHIVE_V2_RUNTIME_MATRIX + 10#$RESUME_AFTER_ARCHIVE_V2_COMMON_CATALOG + 10#$RESUME_AFTER_ARCHIVE_V2_CHECKPOINT > 1 )); then
    echo "Only one exact-gate resume boundary may be selected" >&2
    exit 2
fi
if [[ ( "$RESUME_AFTER_RETENTION" == "1" || "$RESUME_AFTER_RESILIENCE" == "1" )
    && "$MAX_VALIDATORS" -ne 4 ]]; then
    echo "Post-retention and post-resilience resumes require exactly four validators" >&2
    exit 2
fi
if [[ ( "$RESUME_AFTER_PUBLIC_PARITY" == "1" || "$RESUME_AFTER_ARCHIVE_V2_CHECKPOINT" == "1" )
    && ( ! "$RESUME_PUBLIC_PARITY_CHECKPOINT" =~ ^[1-9][0-9]*$
        || "$MAX_VALIDATORS" -ne 4 ) ]]; then
    echo "Archive resume requires four validators and an explicit positive LICHEN_RESUME_LOCAL_GATE_CHECKPOINT_SLOT" >&2
    exit 2
fi
if [[ ( "$RESUME_AFTER_ARCHIVE_V2_OFFLINE_MATRIX" == "1" || "$RESUME_AFTER_ARCHIVE_V2_COMMON_CATALOG" == "1" )
    && ( ! "$RESUME_EXPECTED_ARCHIVE_V2_ROOT" =~ ^[0-9a-f]{64}$
        || "$MAX_VALIDATORS" -ne 4 ) ]]; then
    echo "Archive V2 catalog resume requires four validators and an exact lowercase 64-hex LICHEN_RESUME_LOCAL_GATE_EXPECTED_ARCHIVE_V2_ROOT" >&2
    exit 2
fi
if [[ "$RESUME_AFTER_ARCHIVE_V2_RUNTIME_MATRIX" == "1" && "$MAX_VALIDATORS" -ne 4 ]]; then
    echo "Post-Archive-V2-runtime-matrix resume requires exactly four validators" >&2
    exit 2
fi
if [[ ! "$ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS" =~ ^[1-9][0-9]*$ \
    || "$ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS" -lt 2 ]]; then
    echo "LICHEN_ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS must be an integer of at least 2" >&2
    exit 2
fi
if [[ ! "$ARCHIVE_V2_CHECKPOINT_MATERIALIZATION_TIMEOUT_SECS" =~ ^[1-9][0-9]*$ ]]; then
    echo "LICHEN_ARCHIVE_V2_CHECKPOINT_MATERIALIZATION_TIMEOUT_SECS must be a positive integer" >&2
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

acquire_local_gate_lock() {
    local owner_pid=""

    if mkdir "$LOCAL_GATE_LOCK_DIR" 2>/dev/null; then
        if ! printf '%s\n' "$$" >"$LOCAL_GATE_LOCK_DIR/pid"; then
            rmdir "$LOCAL_GATE_LOCK_DIR" 2>/dev/null || true
            fail "Unable to record local multi-validator gate lock ownership"
        fi
        LOCAL_GATE_LOCK_HELD=1
        return 0
    fi

    if [[ -r "$LOCAL_GATE_LOCK_DIR/pid" ]]; then
        owner_pid="$(tr -cd '0-9' <"$LOCAL_GATE_LOCK_DIR/pid")"
    fi
    if [[ -n "$owner_pid" ]] && kill -0 "$owner_pid" 2>/dev/null; then
        fail "Another local multi-validator gate is already running (PID $owner_pid)"
    fi

    # Only reclaim a lock whose recorded owner is no longer alive. Removal is
    # deliberately limited to the exact lock files rather than a recursive path.
    if [[ -n "$owner_pid" ]]; then
        rm "$LOCAL_GATE_LOCK_DIR/pid" 2>/dev/null || true
        rmdir "$LOCAL_GATE_LOCK_DIR" 2>/dev/null || true
    fi
    if ! mkdir "$LOCAL_GATE_LOCK_DIR" 2>/dev/null; then
        fail "Local multi-validator gate lock exists without a live, reclaimable owner"
    fi
    if ! printf '%s\n' "$$" >"$LOCAL_GATE_LOCK_DIR/pid"; then
        rmdir "$LOCAL_GATE_LOCK_DIR" 2>/dev/null || true
        fail "Unable to record local multi-validator gate lock ownership"
    fi
    LOCAL_GATE_LOCK_HELD=1
}

release_local_gate_lock() {
    local owner_pid=""

    [[ "$LOCAL_GATE_LOCK_HELD" -eq 1 ]] || return 0
    if [[ -r "$LOCAL_GATE_LOCK_DIR/pid" ]]; then
        owner_pid="$(tr -cd '0-9' <"$LOCAL_GATE_LOCK_DIR/pid")"
    fi
    if [[ "$owner_pid" == "$$" ]]; then
        rm "$LOCAL_GATE_LOCK_DIR/pid" 2>/dev/null || true
        rmdir "$LOCAL_GATE_LOCK_DIR" 2>/dev/null || true
    fi
    LOCAL_GATE_LOCK_HELD=0
}

arm_fresh_role_restore() {
    FRESH_ROLE_SWAP_VALIDATOR_NUM=$1
    FRESH_ROLE_ORIGINAL_STATE=$2
    FRESH_ROLE_ORIGINAL_COLD=$3
    FRESH_ROLE_SWAP_ACTIVE=1
}

disarm_fresh_role_restore() {
    FRESH_ROLE_SWAP_ACTIVE=0
    FRESH_ROLE_SWAP_VALIDATOR_NUM=""
    FRESH_ROLE_ORIGINAL_STATE=""
    FRESH_ROLE_ORIGINAL_COLD=""
}

restore_interrupted_fresh_role_state() {
    local validator_num state_dir cold_dir
    [[ "$FRESH_ROLE_SWAP_ACTIVE" -eq 1 ]] || return 0
    validator_num="$FRESH_ROLE_SWAP_VALIDATOR_NUM"
    state_dir="$(db_path "$validator_num")"
    cold_dir="$(cold_path "$validator_num")"

    if [[ -f "$FRESH_ROLE_ORIGINAL_STATE/CURRENT" ]]; then
        discard_fresh_role_candidate_state "$state_dir" "$cold_dir"
        mv "$FRESH_ROLE_ORIGINAL_STATE" "$state_dir"
    elif [[ ! -f "$state_dir/CURRENT" ]]; then
        warn "Interrupted fresh-role cleanup could not find V${validator_num}'s original state"
        return 1
    fi
    if [[ -d "$FRESH_ROLE_ORIGINAL_COLD" ]]; then
        rm -rf "$cold_dir"
        mv "$FRESH_ROLE_ORIGINAL_COLD" "$cold_dir"
    fi
    disarm_fresh_role_restore
    ok "Restored V${validator_num}'s original state after interrupted fresh-role verification"
}

discard_fresh_role_candidate_state() {
    local state_dir=$1 cold_dir=$2

    # Snapshot apply transactions live beside the RocksDB directory rather
    # than inside it. A fresh-role swap must therefore discard the candidate
    # database and all of its transaction sidecars before restoring the
    # original database at the same path. Otherwise an interrupted candidate
    # apply can attach its rollback marker to the restored original state and
    # deterministically rewind that state on its next startup.
    rm -rf \
        "$state_dir" \
        "$cold_dir" \
        "${state_dir}.snapshot-live-rollback" \
        "${state_dir}.proposal-staging" \
        "${state_dir}.replay-staging"
    rm -f \
        "${state_dir}.snapshot-live-rollback.json" \
        "${state_dir}.snapshot-live-rollback.json.tmp"
}

assert_fresh_role_original_has_no_snapshot_transaction() {
    local state_dir=$1

    [[ ! -e "${state_dir}.snapshot-live-rollback.json" \
        && ! -e "${state_dir}.snapshot-live-rollback" ]] \
        || fail "Cannot swap ${state_dir}: a live snapshot rollback transaction is still pending"
}

cleanup() {
    local exit_status="${1:-$?}"
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
    rm -rf /tmp/lichen-testnet/public-history-archive-cache-v{1,2,3,4}
    if [[ "$CHECKPOINT_SOURCE_PEERS_ACTIVE" == "1" ]]; then
        unset \
            LICHEN_LOCAL_ARCHIVE_V2_ROOT_V1 \
            LICHEN_LOCAL_ARCHIVE_V2_RECENT_HISTORY_SLOTS_V1 \
            LICHEN_LOCAL_ARCHIVE_V2_ROOT_V4 \
            LICHEN_LOCAL_ARCHIVE_V2_RECENT_HISTORY_SLOTS_V4
        CHECKPOINT_SOURCE_PEERS_ACTIVE=0
    fi
    restore_interrupted_fresh_role_state \
        || warn "Original fresh-role state remains in its explicit backup path for manual recovery"
    log "Cleanup done"
}

cleanup_and_release_local_gate_lock() {
    gate_exit_status=$?
    trap - EXIT

    # Bash reports status 0 to an EXIT trap for some shell-internal failures,
    # notably an unset-variable expansion under `set -u`. Require an explicit
    # success marker so cleanup can never turn an aborted gate into a green
    # result.
    if [[ "$gate_exit_status" -eq 0 && "$LOCAL_GATE_SUCCESS" -ne 1 ]]; then
        warn "Local multi-validator gate exited without its success marker"
        gate_exit_status=1
    fi

    cleanup "$gate_exit_status"
    release_local_gate_lock
    exit "$gate_exit_status"
}

acquire_local_gate_lock
trap cleanup_and_release_local_gate_lock EXIT

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

get_finalized_slot() {
    rpc_query_params "$1" "getSlot" '["finalized"]' \
        | python3 -c "import json,sys; print(json.load(sys.stdin).get('result',0))" 2>/dev/null \
        || echo 0
}

get_health_frontier_with_retry() {
    local rpc=$1 attempts=${2:-3} timeout_seconds=${3:-10}
    local response frontier attempt

    for attempt in $(seq 1 "$attempts"); do
        response="$(rpc_query_params_with_timeout "$rpc" "getHealth" '[]' "$timeout_seconds")"
        frontier="$(
            python3 -c '
import json
import sys

result = json.load(sys.stdin).get("result", {})
slot = result.get("slot") if isinstance(result, dict) else None
finalized = result.get("finalized_slot") if isinstance(result, dict) else None
valid = (
    result.get("status") == "ok"
    and isinstance(slot, int)
    and not isinstance(slot, bool)
    and isinstance(finalized, int)
    and not isinstance(finalized, bool)
    and slot > 0
    and finalized > 0
    and finalized <= slot
)
if not valid:
    raise SystemExit(1)
print(slot, finalized)
' <<< "$response" 2>/dev/null
        )" || frontier=""
        if [[ "$frontier" =~ ^[1-9][0-9]*\ [1-9][0-9]*$ ]]; then
            printf '%s\n' "$frontier"
            return 0
        fi
        (( attempt < attempts )) && sleep 1
    done
    return 1
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

wait_for_cluster_slot_spread() {
    local max_spread=$1
    local timeout_seconds=$2
    local deadline=$((SECONDS + timeout_seconds))
    local maximum minimum slot spread live validator_num

    while (( SECONDS < deadline )); do
        maximum=0
        minimum=999999999999
        live=0
        for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
            slot="$(get_slot "$(rpc_port "$validator_num")")"
            if [[ "$slot" =~ ^[1-9][0-9]*$ ]]; then
                (( slot > maximum )) && maximum=$slot
                (( slot < minimum )) && minimum=$slot
                live=$((live + 1))
            fi
        done
        spread=$((maximum - minimum))
        if (( live == MAX_VALIDATORS && spread <= max_spread )); then
            ok "All validators are within ${spread} slots before coordinated Archive V2 stop"
            return 0
        fi
        sleep 1
    done
    warn "Validator slot spread did not converge within ${timeout_seconds}s (live=${live}/${MAX_VALIDATORS}, min=${minimum}, max=${maximum}, spread=${spread})"
    return 1
}

wait_for_cluster_finalized_spread() {
    local max_spread=$1
    local timeout_seconds=$2
    local deadline=$((SECONDS + timeout_seconds))
    local maximum_finalized minimum_finalized finalized_spread maximum_lag
    local processed finalized lag live validator_num sample=0 sample_dir
    local -a probe_pids=()

    sample_dir="$(mktemp -d /tmp/lichen-finalized-frontiers.XXXXXX)"
    [[ "$sample_dir" == /tmp/lichen-finalized-frontiers.* && -d "$sample_dir" ]] \
        || fail "Could not create bounded finalized-frontier sample directory"

    while (( SECONDS < deadline )); do
        maximum_finalized=0
        minimum_finalized=999999999999
        maximum_lag=0
        live=0
        for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
            : > "$sample_dir/$validator_num"
            (
                get_health_frontier_with_retry "$(rpc_port "$validator_num")"
            ) > "$sample_dir/$validator_num" &
            probe_pids[validator_num]=$!
        done
        for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
            wait "${probe_pids[validator_num]}" || true
        done
        for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
            read -r processed finalized < "$sample_dir/$validator_num" || {
                processed=0
                finalized=0
            }
            if [[ "$processed" =~ ^[1-9][0-9]*$ \
                && "$finalized" =~ ^[1-9][0-9]*$ \
                && "$finalized" -le "$processed" ]]; then
                (( finalized > maximum_finalized )) && maximum_finalized=$finalized
                (( finalized < minimum_finalized )) && minimum_finalized=$finalized
                lag=$((processed - finalized))
                (( lag > maximum_lag )) && maximum_lag=$lag
                live=$((live + 1))
            fi
        done
        finalized_spread=$((maximum_finalized - minimum_finalized))
        if (( live == MAX_VALIDATORS \
            && finalized_spread <= max_spread \
            && maximum_lag <= max_spread )); then
            ok "All validators' finalized frontiers are within ${finalized_spread} slots with maximum tip lag ${maximum_lag} before coordinated Archive V2 stop"
            rm -rf -- "$sample_dir"
            return 0
        fi
        sample=$((sample + 1))
        if (( sample % 15 == 0 )); then
            log "Waiting for finalized-frontier convergence: live=${live}/${MAX_VALIDATORS} min=${minimum_finalized} max=${maximum_finalized} spread=${finalized_spread} max_tip_lag=${maximum_lag}"
        fi
        sleep 1
    done
    warn "Validator finalized frontiers did not converge within ${timeout_seconds}s (live=${live}/${MAX_VALIDATORS}, min=${minimum_finalized}, max=${maximum_finalized}, spread=${finalized_spread}, max_tip_lag=${maximum_lag})"
    rm -rf -- "$sample_dir"
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

get_epoch_active_validator_count() {
    rpc_query "$1" "getValidators" | python3 -c "
import json,sys
try:
    r=json.load(sys.stdin).get('result',{})
    vs=r.get('validators',[]) if isinstance(r,dict) else []
    print(len([v for v in vs if v.get('staking_v2_epoch_active') is True]))
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
    local validator_num managed_pid

    for second in $(seq 1 "$timeout_seconds"); do
        for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
            managed_pid="${VALIDATOR_PIDS[$validator_num]:-}"
            if [[ -n "$managed_pid" ]] && ! kill -0 "$managed_pid" 2>/dev/null; then
                warn "V${validator_num} managed process ${managed_pid} exited while waiting for cluster readiness"
                return 1
            fi
        done
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

verify_loaded_backlog_liveness() {
    [[ "$MAX_VALIDATORS" -ge 4 ]] || return 0
    [[ "$BACKLOG_REGRESSION_TXS" =~ ^[1-9][0-9]*$ ]] \
        || fail "LICHEN_BACKLOG_REGRESSION_TXS must be a positive integer"

    local wallet_metadata wallet recipient accepted=0 amount output signature
    local first_failure=""
    local before_stall after_stall resume_slot target_slot current_slot
    local signatures_file="/tmp/lichen-testnet/rg-403-signatures.txt"
    local -a backlog_logs=()
    local -a backlog_log_starts=()
    wallet_metadata="$(db_path 1)/genesis-wallet.json"
    wallet="$(python3 -c '
import json
import pathlib
import sys

metadata = pathlib.Path(sys.argv[1]).resolve()
base = metadata.parent
wallet = json.loads(metadata.read_text())
relative = next(
    (
        entry.get("keypair_path")
        for entry in wallet.get("distribution_wallets", [])
        if entry.get("role") == "builder_grants"
    ),
    None,
)
if not isinstance(relative, str) or not relative:
    raise SystemExit("missing canonical funded distribution keypair path")
candidate = (base / relative).resolve()
candidate.relative_to(base)
print(candidate)
' "$wallet_metadata")" || fail "RG-403 could not resolve V1's canonical funded distribution keypair"
    recipient="${ALL_PUBKEYS[1]}"
    [[ -f "$wallet" ]] || fail "RG-403 requires V1's canonical funded distribution keypair"
    : > "$signatures_file"

    log "═══════════════════════════════════════════════════════════"
    log "RG-403: Recovering a ${BACKLOG_REGRESSION_TXS}-transaction mempool backlog"
    log "═══════════════════════════════════════════════════════════"

    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        local active_log
        active_log="${VALIDATOR_LOGS[$validator_num]:-$(log_path "$validator_num")}"
        [[ -f "$active_log" ]] \
            || fail "RG-403 cannot inspect V${validator_num}'s active validator log: $active_log"
        backlog_logs[$validator_num]="$active_log"
        backlog_log_starts[$validator_num]="$(( $(wc -l < "$active_log") + 1 ))"
    done

    for validator_num in 2 3 4; do
        signal_validator_pid_tree "${VALIDATOR_PIDS[$validator_num]:-}" STOP
    done

    before_stall="$(get_slot "$V1_RPC")"
    sleep 2
    after_stall="$(get_slot "$V1_RPC")"
    if (( after_stall - before_stall > 2 )); then
        for validator_num in 2 3 4; do
            signal_validator_pid_tree "${VALIDATOR_PIDS[$validator_num]:-}" CONT
        done
        fail "RG-403 could not establish a no-quorum backlog window"
    fi

    for tx_num in $(seq 1 "$BACKLOG_REGRESSION_TXS"); do
        amount="$(printf '0.%06d' "$tx_num")"
        if output="$("$RELEASE_BIN_DIR/lichen" transfer \
            "$recipient" "$amount" \
            --keypair "$wallet" \
            --rpc-url "http://127.0.0.1:${V1_RPC}" 2>&1)"; then
            signature="$(sed -nE 's/^.*Signature:[[:space:]]*([0-9a-f]+).*$/\1/p' <<< "$output" | tail -n 1)"
            if [[ -n "$signature" ]]; then
                printf '%s\n' "$signature" >> "$signatures_file"
                accepted=$((accepted + 1))
            fi
        elif [[ -z "$first_failure" ]]; then
            first_failure="$output"
        fi
    done

    if (( accepted != BACKLOG_REGRESSION_TXS )); then
        for validator_num in 2 3 4; do
            signal_validator_pid_tree "${VALIDATOR_PIDS[$validator_num]:-}" CONT
        done
        [[ -z "$first_failure" ]] || warn "RG-403 first admission failure: $(tail -n 1 <<< "$first_failure")"
        fail "RG-403 admitted ${accepted}/${BACKLOG_REGRESSION_TXS} backlog transactions"
    fi
    ok "RG-403 admitted all ${accepted} transactions while finality was paused"

    resume_slot="$(get_slot "$V1_RPC")"
    target_slot=$((resume_slot + accepted + 20))
    for validator_num in 2 3 4; do
        signal_validator_pid_tree "${VALIDATOR_PIDS[$validator_num]:-}" CONT
    done

    local recovered=false
    for _ in $(seq 1 60); do
        sleep 1
        current_slot="$(get_slot "$V1_RPC")"
        if (( current_slot >= target_slot )); then
            recovered=true
            break
        fi
    done
    $recovered || fail "RG-403 did not drain the bounded backlog while advancing finality"

    local confirmed=0 response
    while IFS= read -r signature; do
        response="$(rpc_query_params "$V1_RPC" getTransaction "[\"${signature}\"]")"
        if python3 -c '
import json
import sys
result = json.load(sys.stdin).get("result")
raise SystemExit(0 if isinstance(result, dict) else 1)
' <<< "$response"; then
            confirmed=$((confirmed + 1))
        fi
    done < "$signatures_file"
    (( confirmed == accepted )) \
        || fail "RG-403 finalized ${confirmed}/${accepted} admitted transactions"

    local max_block_txs=0 observed_max
    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        observed_max="$({
            tail -n "+${backlog_log_starts[$validator_num]}" \
                "${backlog_logs[$validator_num]}" \
                | awk '/COMMITTED/' \
                | sed -nE 's/^.*txs:[[:space:]]*([0-9]+).*$/\1/p'
            } | sort -nr | head -n 1)"
        observed_max="${observed_max:-0}"
        (( observed_max > max_block_txs )) && max_block_txs="$observed_max"
    done
    # The production BFT budget admits at most fourteen default-budget user
    # transfers plus the mandatory parent commit certificate.
    (( max_block_txs <= 15 )) \
        || fail "RG-403 observed a ${max_block_txs}-transaction block above the bounded default-compute proposal limit"
    (( max_block_txs > 1 )) \
        || fail "RG-403 did not observe a transaction-bearing backlog block in the active validator logs"

    wait_for_cluster_slot_spread 20 60 \
        || fail "RG-403 validators did not reconverge after backlog recovery"
    verify_chain_producing "after bounded backlog recovery" "$V1_RPC" 5
    ok "RG-403 finalized ${confirmed} transactions through count-and-compute-bounded proposals (max block txs=${max_block_txs})"
}

verify_chain_recovers_within_bft_window() {
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

    warn "Chain entered bounded BFT recovery ($label): ${diff} block(s) in ${initial_window_secs}s"
    # A membership transition or rapid own-state restart can land while peers
    # occupy different BFT rounds. Each protocol phase backs off to a 5s cap,
    # so permit enough time for several complete rounds, then require a
    # sustained recovery burst rather than accepting one late block.
    for _ in $(seq 1 $((recovery_window_secs / 2))); do
        sleep 2
        s2=$(get_slot "$rpc")
        diff=$((s2 - s1))
        if [[ "$diff" -ge "$recovery_min_blocks" ]]; then
            ok "Chain recovered ($label): $diff blocks in at most $((initial_window_secs + recovery_window_secs))s (slot $s1 → $s2)"
            return 0
        fi
    done

    for n in $(seq 1 "$MAX_VALIDATORS"); do
        local lp
        lp="$(log_path $n)"
        [[ -f "$lp" ]] && { warn "V${n} log tail:"; tail -20 "$lp"; }
    done
    fail "Chain did not recover ($label): only $diff blocks in $((initial_window_secs + recovery_window_secs))s (slot $s1 → $s2)"
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
    local profile_kind=""
    local profile_catalog_bound="0"
    if [[ -n "$checkpoint_slot" ]]; then
        manifest_db_path="$(db_path "$validator_num")/checkpoints/slot-${checkpoint_slot}"
        manifest_cold_path="$manifest_db_path/cold"
        read -r profile_kind profile_catalog_bound < <(python3 -c '
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as fh:
    profile = json.load(fh).get("snapshot_profile", {})
root = profile.get("archive_v2_catalog_root")
print(profile.get("kind", "full_archive_v1"), int(isinstance(root, list) and len(root) == 32))
' "$manifest_db_path/checkpoint_meta.json")
    else
        manifest_db_path="$(db_path "$validator_num")"
        manifest_cold_path="$(cold_path "$validator_num")"
    fi

    if [[ "$profile_kind" == "hot_repair_v1" && "$profile_catalog_bound" == "1" ]]; then
        local archive_root="/tmp/lichen-testnet/archive-v2-v${validator_num}"
        local cache_root="/tmp/lichen-testnet/public-history-archive-cache-v${validator_num}"
        [[ -f "$archive_root/catalog.av2" ]] \
            || fail "V${validator_num} Archive V2 catalog is missing for logical checkpoint parity: ${archive_root}"
        rm -rf "$cache_root"
        local archive_args=(
            "$RELEASE_BIN_DIR/lichen-archive-v2"
            public-history-manifest
            --state-dir "$manifest_db_path"
            --root "$archive_root"
            --cache-root "$cache_root"
            --cache-quota-bytes 2147483648
            --chunk-size 1000
        )
        local source_num
        for source_num in $(seq 1 "$MAX_VALIDATORS"); do
            local source_root="/tmp/lichen-testnet/archive-v2-v${source_num}"
            [[ -f "$source_root/catalog.av2" ]] && archive_args+=(--source-dir "$source_root")
        done
        if ! "${archive_args[@]}" > "$manifest_file"; then
            rm -rf "$cache_root"
            fail "V${validator_num} composed Archive V2 plus hot-checkpoint manifest failed"
        fi
        rm -rf "$cache_root"
        python3 -c '
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as fh:
    data = json.load(fh)
print(data["manifest_root"])
' "$manifest_file"
        return
    fi

    local args=(
        "$RELEASE_BIN_DIR/lichen-validator"
        --network testnet
        --dev-mode
        --db-path "$manifest_db_path"
        --cache-size-mb 128
        --public-history-manifest
    )
    if [[ -d "$manifest_cold_path" ]]; then
        args+=(--cold-store "$manifest_cold_path")
    elif [[ -n "$checkpoint_slot" ]]; then
        [[ "$profile_kind" == "hot_repair_v1" && "$profile_catalog_bound" == "0" ]] \
            || fail "V${validator_num} full-archive checkpoint ${checkpoint_slot} has no cold tree"
    else
        fail "V${validator_num} live cold archive path is missing: ${manifest_cold_path}"
    fi

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

verify_archive_v2_hot_checkpoint_profile() {
    local selected_checkpoint_slot="${1:-}"
    local baseline_profile=""
    local baseline_manifest_root=""
    local meta profile profile_root profile_start status_json catalog_root handoff_root catalog_end manifest_root catalog_verification_root
    if [[ -n "$selected_checkpoint_slot" ]]; then
        COMMON_CHECKPOINT_SLOT="$selected_checkpoint_slot"
    else
        COMMON_CHECKPOINT_SLOT=""
        wait_for_common_checkpoint "Archive V2 hot-repair profile"
    fi
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        meta="$(db_path "$V_NUM")/checkpoints/slot-${COMMON_CHECKPOINT_SLOT}/checkpoint_meta.json"
        profile="$(python3 -c '
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as fh:
    meta = json.load(fh)
profile = meta.get("snapshot_profile", {})
if profile.get("kind") != "hot_repair_v1":
    raise SystemExit(1)
start = profile.get("history_start_slot")
root = profile.get("archive_v2_catalog_root")
if not isinstance(start, int) or start < 0 or not isinstance(root, list) or len(root) != 32:
    raise SystemExit(1)
print(json.dumps(profile, sort_keys=True, separators=(",", ":")))
' "$meta")" || fail "V${V_NUM} checkpoint ${COMMON_CHECKPOINT_SLOT} is not a valid catalog-bound hot_repair_v1 checkpoint"
        [[ ! -e "$(db_path "$V_NUM")/checkpoints/slot-${COMMON_CHECKPOINT_SLOT}/cold" ]] \
            || fail "V${V_NUM} Archive V2 hot checkpoint unexpectedly contains legacy cold storage"
        [[ -z "$(find "$(db_path "$V_NUM")/checkpoints/slot-${COMMON_CHECKPOINT_SLOT}" -type l -print -quit)" ]] \
            || fail "V${V_NUM} Archive V2 hot checkpoint contains a symbolic link"
        if [[ -z "$baseline_profile" ]]; then
            baseline_profile="$profile"
        elif [[ "$profile" != "$baseline_profile" ]]; then
            fail "Archive V2 checkpoint profile drift: V${V_NUM} differs from V1"
        fi
        read -r profile_start profile_root < <(python3 -c '
import json
import sys
profile = json.loads(sys.argv[1])
print(profile["history_start_slot"], bytes(profile["archive_v2_catalog_root"]).hex())
' "$profile") \
            || fail "V${V_NUM} checkpoint ${COMMON_CHECKPOINT_SLOT} profile cannot be decoded"
        catalog_verification_root="/tmp/lichen-testnet/archive-v2-v${V_NUM}"
        if [[ "$ARCHIVE_V2_CHECKPOINT_BOUND_CATALOGS" == "1" ]]; then
            catalog_verification_root="/tmp/lichen-testnet/archive-v2-checkpoint-bound-v${V_NUM}"
        fi
        status_json="$("$RELEASE_BIN_DIR/lichen-archive-v2" status \
            --root "$catalog_verification_root" \
            --history-start-slot "$profile_start")" \
            || fail "V${V_NUM} Archive V2 checkpoint catalog status failed"
        read -r catalog_root handoff_root catalog_end < <(python3 -c '
import json
import sys
status = json.load(sys.stdin)
slot_range = status.get("slot_range")
handoff_root = status.get("checkpoint_handoff_root")
if not isinstance(slot_range, list) or len(slot_range) != 2 or not isinstance(handoff_root, str):
    raise SystemExit(1)
print(status["catalog_root"], handoff_root, slot_range[1])
' <<< "$status_json") \
            || fail "V${V_NUM} Archive V2 checkpoint catalog metadata is invalid"
        [[ "$handoff_root" == "$profile_root" ]] \
            || fail "V${V_NUM} checkpoint handoff root ${profile_root} differs from admitted handoff ${handoff_root} (catalog ${catalog_root})"
        (( catalog_end >= profile_start - 1 )) \
            || fail "V${V_NUM} Archive V2 catalog ends at ${catalog_end}, before checkpoint predecessor $((profile_start - 1))"

        "$RELEASE_BIN_DIR/lichen-validator" \
            --network testnet \
            --dev-mode \
            --db-path "$(db_path "$V_NUM")/checkpoints/slot-${COMMON_CHECKPOINT_SLOT}" \
            --cache-size-mb 128 \
            --checkpoint-snapshot-manifest \
            > "/tmp/lichen-testnet/checkpoint-snapshot-v${V_NUM}.json" \
            || fail "V${V_NUM} hot checkpoint snapshot manifest failed"
        manifest_root="$(python3 -c '
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as fh:
    print(json.load(fh)["manifest_root"])
' "/tmp/lichen-testnet/checkpoint-snapshot-v${V_NUM}.json")" \
            || fail "V${V_NUM} hot checkpoint snapshot manifest is invalid"
        if [[ -z "$baseline_manifest_root" ]]; then
            baseline_manifest_root="$manifest_root"
        elif [[ "$manifest_root" != "$baseline_manifest_root" ]]; then
            fail "V${V_NUM} hot checkpoint snapshot root ${manifest_root} differs from ${baseline_manifest_root}"
        fi
    done
    ok "Archive V2 catalog-bound hot checkpoints are self-contained, symlink-free, and identical across all validators: ${baseline_manifest_root}"
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
    log "Verifying partition-independent public-history parity across Archive V2 plus hot checkpoint state (${scope})..."
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
    # A common checkpoint is considered durable as soon as every metadata file
    # is published, but deep-history RPC can remain briefly unavailable while
    # RocksDB releases checkpoint resources. Use the same bounded retry path as
    # every subsequent cross-validator genesis comparison instead of racing a
    # single getBlock(0) request against that recovery window.
    archive_v2_rpc_block_hash_with_retry "$V1_RPC" 0
}

rebuild_archive_v2_checkpoint_catalog_evidence() {
    local history_start_slot=$1
    local expected_handoff_root=$2
    local expected_genesis_hash=$3
    local end_slot=$((history_start_slot - 1))
    local av2="$RELEASE_BIN_DIR/lichen-archive-v2"
    local baseline_catalog_root=""
    local root replica status_json catalog_root handoff_root catalog_end genesis_hash

    (( history_start_slot > 0 )) \
        || fail "Checkpoint-bound Archive V2 catalog reconstruction requires a positive history start"
    log "Reconstructing the exact checkpoint-bound Archive V2 catalog 0..${end_slot} from all four authoritative states..."
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        root="/tmp/lichen-testnet/archive-v2-checkpoint-bound-v${V_NUM}"
        replica="/tmp/lichen-testnet/archive-v2-checkpoint-bound-replica-v${V_NUM}"
        rm -rf "$root" "$replica"
        "$av2" build \
            --state-dir "$(db_path "$V_NUM")" \
            --cold-store "$(cold_path "$V_NUM")" \
            --root "$root" \
            --network-id lichen-testnet-1 \
            --genesis-hash "$expected_genesis_hash" \
            --start-slot 0 \
            --end-slot "$end_slot" \
            --finality-depth-slots "$LICHEN_COLD_RETENTION_SLOTS" \
            --zstd-level 6 \
            --frame-bytes 1048576 \
            --replica-root "$replica" \
            --required-replicas 1 >/dev/null \
            || fail "V${V_NUM} checkpoint-bound Archive V2 catalog reconstruction failed"
        status_json="$("$av2" status --root "$root" --history-start-slot "$history_start_slot")" \
            || fail "V${V_NUM} checkpoint-bound Archive V2 status failed"
        read -r catalog_root handoff_root catalog_end genesis_hash < <(python3 -c '
import json
import sys
status = json.load(sys.stdin)
slot_range = status.get("slot_range")
if not isinstance(slot_range, list) or len(slot_range) != 2:
    raise SystemExit(1)
print(status["catalog_root"], status["checkpoint_handoff_root"], slot_range[1], status["genesis_hash"])
' <<< "$status_json") \
            || fail "V${V_NUM} checkpoint-bound Archive V2 metadata is invalid"
        [[ "$handoff_root" == "$expected_handoff_root" ]] \
            || fail "V${V_NUM} reconstructed handoff ${handoff_root} differs from checkpoint binding ${expected_handoff_root}"
        [[ "$catalog_end" == "$end_slot" && "$genesis_hash" == "$expected_genesis_hash" ]] \
            || fail "V${V_NUM} reconstructed checkpoint catalog range or genesis identity drifted"
        if [[ -z "$baseline_catalog_root" ]]; then
            baseline_catalog_root="$catalog_root"
        elif [[ "$catalog_root" != "$baseline_catalog_root" ]]; then
            fail "V${V_NUM} reconstructed checkpoint catalog ${catalog_root} differs from ${baseline_catalog_root}"
        fi
        "$av2" verify --root "$root" --max-objects 10000 >/dev/null \
            || fail "V${V_NUM} reconstructed checkpoint catalog failed full verification"
        "$av2" verify --root "$replica" --max-objects 10000 >/dev/null \
            || fail "V${V_NUM} reconstructed checkpoint replica failed full verification"
        ok "V${V_NUM} independently reconstructed checkpoint catalog ${catalog_root} with handoff ${handoff_root}"
    done

    ARCHIVE_V2_FRESH_JOIN_SOURCE_ROOT="/tmp/lichen-testnet/archive-v2-checkpoint-bound-v1"
    ARCHIVE_V2_FRESH_JOIN_REPLICA_ROOT="/tmp/lichen-testnet/archive-v2-checkpoint-bound-replica-v4"
    ARCHIVE_V2_CHECKPOINT_BOUND_CATALOGS=1
    rm -rf \
        /tmp/lichen-testnet/archive-v2-checkpoint-bound-replica-v1 \
        /tmp/lichen-testnet/archive-v2-checkpoint-bound-replica-v2 \
        /tmp/lichen-testnet/archive-v2-checkpoint-bound-replica-v3
    ok "Four-way checkpoint catalog reconstruction retained four node proofs and one independent replica: ${baseline_catalog_root}"
}

archive_v2_source_finalized_slot() {
    local validator_num=$1
    local profile_json
    profile_json="$(
        "$RELEASE_BIN_DIR/lichen-archive-v2" profile-source \
            --state-dir "$(db_path "$validator_num")" \
            --cold-store "$(cold_path "$validator_num")" \
            --start-slot 0 \
            --end-slot 0 \
            --top-blocks 1
    )" || fail "V${validator_num} Archive V2 source profile failed"
    python3 -c '
import json
import sys
print(json.load(sys.stdin)["finalized_slot"])
' <<< "$profile_json"
}

verify_archive_v2_offline_matrix() {
    local checkpoint_slot=$1
    local genesis_hash=$2
    local archive_finality_depth=$((LICHEN_COLD_RETENTION_SLOTS - ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS))
    local minimum_finalized_slot=""
    local maximum_finalized_slot=0
    local validator_finalized_slot stopped_spread build_end required_archive_end
    local baseline_root=""
    local archive_root replica_root mirror_root restore_root build_json status_json catalog_root
    local av2="$RELEASE_BIN_DIR/lichen-archive-v2"

    # The validators advance beyond the common checkpoint while its immutable
    # public-history parity is being computed. Build from the actual stopped
    # sources, not the older checkpoint. A coordinated stop can leave a small
    # finalized spread, so the catalog deliberately overlaps the retained hot
    # suffix by a bounded test-only headroom and must cover the highest tip.
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        validator_finalized_slot="$(archive_v2_source_finalized_slot "$V_NUM")"
        [[ "$validator_finalized_slot" =~ ^[0-9]+$ ]] \
            || fail "V${V_NUM} returned an invalid finalized slot: ${validator_finalized_slot}"
        if [[ -z "$minimum_finalized_slot" || "$validator_finalized_slot" -lt "$minimum_finalized_slot" ]]; then
            minimum_finalized_slot="$validator_finalized_slot"
        fi
        if (( validator_finalized_slot > maximum_finalized_slot )); then
            maximum_finalized_slot=$validator_finalized_slot
        fi
    done
    stopped_spread=$((maximum_finalized_slot - minimum_finalized_slot))
    (( stopped_spread <= ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS )) \
        || fail "Stopped Archive V2 finalized spread ${stopped_spread} exceeds catalog headroom ${ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS}"
    build_end=$((minimum_finalized_slot - archive_finality_depth))
    required_archive_end=$((maximum_finalized_slot - LICHEN_COLD_RETENTION_SLOTS))
    [[ "$build_end" -ge 0 ]] \
        || fail "Archive V2 test range is empty at finalized slot ${minimum_finalized_slot}"
    (( build_end >= required_archive_end )) \
        || fail "Archive V2 catalog end ${build_end} does not cover highest stopped requirement ${required_archive_end}"
    log "Building and independently verifying Archive V2 range 0..${build_end} from stopped finalized range ${minimum_finalized_slot}..${maximum_finalized_slot} (checkpoint evidence ${checkpoint_slot})..."
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        archive_root="/tmp/lichen-testnet/archive-v2-v${V_NUM}"
        replica_root="/tmp/lichen-testnet/archive-v2-replica-v${V_NUM}"
        mirror_root="/tmp/lichen-testnet/archive-v2-mirror-v${V_NUM}"
        restore_root="/tmp/lichen-testnet/archive-v2-restore-v${V_NUM}"
        rm -rf "$archive_root" "$replica_root" "$mirror_root" "$restore_root"

        build_json="$(
            "$av2" build \
                --state-dir "$(db_path "$V_NUM")" \
                --cold-store "$(cold_path "$V_NUM")" \
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

refresh_archive_v2_runtime_catalog() {
    local validator_num=$1
    local root=$2
    local expected_root="/tmp/lichen-testnet/archive-v2-v${validator_num}"
    local replica_root="/tmp/lichen-testnet/archive-v2-replica-v${validator_num}"
    local offline_replica_root="${replica_root}.offline"
    local transient_replica=0
    local status_json genesis_hash catalog_end finalized_slot required_end build_start
    local av2="$RELEASE_BIN_DIR/lichen-archive-v2"

    # Fresh-role joins use separately prepared immutable catalogs. The runtime
    # matrix uses these node-owned roots and must append every newly finalized
    # legacy hot/cold range before a deliberate restart can be admitted.
    [[ "$root" == "$expected_root" && -f "$root/catalog.av2" ]] || return 0
    status_json="$("$av2" status --root "$root")" \
        || fail "V${validator_num} Archive V2 pre-start status failed"
    genesis_hash="$(python3 -c '
import json
import sys
print(json.load(sys.stdin)["genesis_hash"])
' <<< "$status_json")"
    catalog_end="$(python3 -c '
import json
import sys
value = json.load(sys.stdin)["slot_range"]
print(-1 if value is None else value[1])
' <<< "$status_json")"
    finalized_slot="$(archive_v2_source_finalized_slot "$validator_num")"
    (( finalized_slot >= LICHEN_COLD_RETENTION_SLOTS )) || return 0
    required_end=$((finalized_slot - LICHEN_COLD_RETENTION_SLOTS))
    if [[ -n "${ARCHIVE_V2_RUNTIME_REFRESH_REQUIRED_END:-}" ]]; then
        [[ "$ARCHIVE_V2_RUNTIME_REFRESH_REQUIRED_END" =~ ^[0-9]+$ ]] \
            || fail "Pinned Archive V2 runtime refresh end is invalid"
        (( ARCHIVE_V2_RUNTIME_REFRESH_REQUIRED_END <= required_end )) \
            || fail "Pinned Archive V2 runtime refresh end exceeds V${validator_num}'s finalized safety boundary"
        required_end="$ARCHIVE_V2_RUNTIME_REFRESH_REQUIRED_END"
    fi
    (( required_end > catalog_end )) || return 0
    build_start=$((catalog_end + 1))

    # Preserve a deliberately unavailable V2 source during its outage drill.
    # Full/consensus nodes do not consume replicas at runtime, so a temporary
    # acknowledgement destination avoids retaining unused duplicate objects.
    if [[ ! -d "$replica_root" && -d "$offline_replica_root" ]]; then
        replica_root="$offline_replica_root"
    elif [[ ! -d "$replica_root" ]]; then
        transient_replica=1
    fi
    "$av2" build \
        --state-dir "$(db_path "$validator_num")" \
        --cold-store "$(cold_path "$validator_num")" \
        --root "$root" \
        --network-id lichen-testnet-1 \
        --genesis-hash "$genesis_hash" \
        --start-slot "$build_start" \
        --end-slot "$required_end" \
        --finality-depth-slots "$LICHEN_COLD_RETENTION_SLOTS" \
        --zstd-level 6 \
        --frame-bytes 1048576 \
        --replica-root "$replica_root" \
        --required-replicas 1 >/dev/null \
        || fail "V${validator_num} Archive V2 pre-start catalog append failed"
    if [[ "$transient_replica" == "1" ]]; then
        rm -rf "$replica_root"
    fi
    ok "V${validator_num} Archive V2 catalog advanced ${build_start}..${required_end} before restart"
}

start_archive_v2_validator() {
    local validator_num=$1
    local output_log=$2
    local detached="${3:-0}"
    local role root role_override root_override cache_override source_override recent_override recent_history_slots
    local source_url_override source_ca_override source_token_override source_url
    local -a role_env
    role_override="LICHEN_LOCAL_ARCHIVE_V2_ROLE_V${validator_num}"
    root_override="LICHEN_LOCAL_ARCHIVE_V2_ROOT_V${validator_num}"
    cache_override="LICHEN_LOCAL_ARCHIVE_V2_CACHE_ROOT_V${validator_num}"
    source_override="LICHEN_LOCAL_ARCHIVE_V2_SOURCE_DIRS_V${validator_num}"
    source_url_override="LICHEN_LOCAL_ARCHIVE_V2_SOURCE_URLS_V${validator_num}"
    source_ca_override="LICHEN_LOCAL_ARCHIVE_V2_SOURCE_CA_CERT_V${validator_num}"
    source_token_override="LICHEN_LOCAL_ARCHIVE_V2_SOURCE_BEARER_TOKEN_V${validator_num}"
    recent_override="LICHEN_LOCAL_ARCHIVE_V2_RECENT_HISTORY_SLOTS_V${validator_num}"
    role="${!role_override:-$(archive_v2_runtime_role "$validator_num")}"
    root="${!root_override:-/tmp/lichen-testnet/archive-v2-v${validator_num}}"
    recent_history_slots="${!recent_override:-$LICHEN_COLD_RETENTION_SLOTS}"
    refresh_archive_v2_runtime_catalog "$validator_num" "$root"
    role_env=(
        "LICHEN_DISABLE_SUPERVISOR=1"
        # Tell run-validator.sh that the gate itself owns the process. This
        # enters the validator worker directly and avoids a second admission
        # racing one slot behind the just-refreshed immutable catalog.
        "LICHEN_SUPERVISED=1"
        "LICHEN_LOCAL_ARCHIVE_V2_ROLE=${role}"
        "LICHEN_LOCAL_ARCHIVE_V2_ROOT=${root}"
        # Role admission must prove the same hot suffix that this gate's cold
        # migrator is configured to retain. The hosted accelerated gate uses
        # 20 slots; production-like local runs keep the larger default above.
        "LICHEN_LOCAL_ARCHIVE_V2_RECENT_HISTORY_SLOTS=${recent_history_slots}"
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
    VALIDATOR_LOGS[$validator_num]="$output_log"
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
    local genesis_hash source_status source_catalog_root source_catalog_end
    local checkpoint_profile_start expected_profile_start full_status full_catalog_root
    local av2="$RELEASE_BIN_DIR/lichen-archive-v2"
    local source_root="${ARCHIVE_V2_FRESH_JOIN_SOURCE_ROOT:-/tmp/lichen-testnet/archive-v2-v1}"
    # V1 owns the full node-local source. V4 deliberately retains the matching
    # independent repair replica after checkpoint reconciliation; using it here
    # avoids keeping a redundant V1 replica solely for fresh-join testing.
    local replica_root="${ARCHIVE_V2_FRESH_JOIN_REPLICA_ROOT:-/tmp/lichen-testnet/archive-v2-replica-v4}"
    local full_root="/tmp/lichen-testnet/archive-v2-fresh-full-v3"
    local cache_root="/tmp/lichen-testnet/archive-v2-fresh-cache-catalog-v3"
    local consensus_root="/tmp/lichen-testnet/archive-v2-fresh-consensus-v3"

    genesis_hash="$(archive_v2_genesis_hash)" \
        || fail "Could not capture genesis hash for fresh Archive V2 role joins"
    [[ -f "$source_root/catalog.av2" && -f "$replica_root/catalog.av2" ]] \
        || fail "Fresh joins require the reconciled node-owned Archive V2 catalog and replica"
    source_status="$("$av2" status --root "$source_root")" \
        || fail "Could not read the reconciled Archive V2 source status"
    read -r source_catalog_root source_catalog_end < <(python3 -c '
import json
import sys
status = json.load(sys.stdin)
if status.get("genesis_hash") != sys.argv[1]:
    raise SystemExit(1)
slot_range = status.get("slot_range")
if not isinstance(slot_range, list) or len(slot_range) != 2:
    raise SystemExit(1)
print(status["catalog_root"], slot_range[1])
' "$genesis_hash" <<< "$source_status") \
        || fail "Reconciled Archive V2 source has the wrong genesis identity"
    [[ "$COMMON_CHECKPOINT_SLOT" =~ ^[1-9][0-9]*$ ]] \
        || fail "Fresh Archive V2 joins require an exact common checkpoint slot"
    expected_profile_start=$((COMMON_CHECKPOINT_SLOT - ARCHIVE_V2_FRESH_JOIN_RECENT_HISTORY_SLOTS + 1))
    (( expected_profile_start > 0 )) \
        || fail "Fresh Archive V2 checkpoint is below the configured hot-window length"
    checkpoint_profile_start="$(python3 -c '
import json
import sys
with open(sys.argv[1], "r", encoding="utf-8") as fh:
    print(json.load(fh)["snapshot_profile"]["history_start_slot"])
' "$(db_path 1)/checkpoints/slot-${COMMON_CHECKPOINT_SLOT}/checkpoint_meta.json")" \
        || fail "Fresh Archive V2 checkpoint profile cannot be read"
    (( checkpoint_profile_start <= expected_profile_start )) \
        || fail "Fresh Archive V2 checkpoint retained ${checkpoint_profile_start}..${COMMON_CHECKPOINT_SLOT}, below the configured ${ARCHIVE_V2_FRESH_JOIN_RECENT_HISTORY_SLOTS}-slot hot suffix"
    (( source_catalog_end >= checkpoint_profile_start - 1 )) \
        || fail "Fresh Archive V2 catalog ends at ${source_catalog_end}, before checkpoint handoff $((checkpoint_profile_start - 1))"
    "$av2" verify --root "$source_root" --max-objects 10000 >/dev/null \
        || fail "Reconciled Archive V2 fresh-join source verification failed"

    rm -rf "$full_root" "$cache_root" "$consensus_root"
    "$av2" restore \
        --root "$full_root" \
        --source "fresh-source:local-a:${replica_root}" \
        --network-id lichen-testnet-1 \
        --genesis-hash "$genesis_hash" \
        --max-objects 1000 \
        --max-bytes 17179869184 >/dev/null \
        || fail "Could not restore complete immutable Archive V2 root for fresh full join"
    full_status="$("$av2" status --root "$full_root")" \
        || fail "Could not read restored full-archive fresh-join status"
    full_catalog_root="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["catalog_root"])' \
        <<< "$full_status")" \
        || fail "Restored full-archive fresh-join catalog is invalid"
    [[ "$full_catalog_root" == "$source_catalog_root" ]] \
        || fail "Restored fresh-join catalog ${full_catalog_root} differs from reconciled ${source_catalog_root}"
    start_archive_v2_https_source "$replica_root"
    mkdir -p "$cache_root" "$consensus_root"
    install -m 0644 "$source_root/catalog.av2" "$cache_root/catalog.av2"
    install -m 0644 "$source_root/catalog.av2" "$consensus_root/catalog.av2"

    LICHEN_LOCAL_ARCHIVE_V2_ROLE_V3="full-archive"
    LICHEN_LOCAL_ARCHIVE_V2_ROOT_V3="$full_root"
    # A fresh role syncs against a moving network tip. Keep its admission
    # suffix at the explicit gate checkpoint scale instead of reusing the
    # synthetic 20-slot cold-migration window; the immutable catalog remains
    # responsible for every older slot and admission still fails closed. The
    # public binaries keep the 50,000-slot minimum outside double-gated local
    # development mode.
    LICHEN_LOCAL_ARCHIVE_V2_RECENT_HISTORY_SLOTS_V3="$ARCHIVE_V2_FRESH_JOIN_RECENT_HISTORY_SLOTS"
    LICHEN_LOCAL_ARCHIVE_V2_CACHE_ROOT_V3="/tmp/lichen-testnet/archive-v2-fresh-cache-v3"
    LICHEN_LOCAL_ARCHIVE_V2_SOURCE_DIRS_V3="$replica_root"
    LICHEN_LOCAL_ARCHIVE_V2_SOURCE_URLS_V3="https://127.0.0.1:${ARCHIVE_V2_HTTPS_SOURCE_PORT}/"
    LICHEN_LOCAL_ARCHIVE_V2_SOURCE_CA_CERT_V3="$ARCHIVE_V2_HTTPS_SOURCE_CA"
    LICHEN_LOCAL_ARCHIVE_V2_SOURCE_BEARER_TOKEN_V3="$ARCHIVE_V2_HTTPS_SOURCE_TOKEN"
    FRESH_ARCHIVE_V2_FULL_ROOT="$full_root"
    FRESH_ARCHIVE_V2_CACHE_CATALOG_ROOT="$cache_root"
    FRESH_ARCHIVE_V2_CONSENSUS_ROOT="$consensus_root"
    FRESH_ARCHIVE_V2_GENESIS_HASH="$genesis_hash"
    ok "Prepared immutable Archive V2 fresh-join roots from reconciled catalog ${source_catalog_root}"
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
    local rpc network_slot local_slot drift observed_role attempts exit_status
    rpc="$(rpc_port "$validator_num")"
    attempts=$(((ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS + 1) / 2))
    for i in $(seq 1 "$attempts"); do
        sleep 2
        if ! kill -0 "$pid" 2>/dev/null; then
            if wait "$pid"; then
                exit_status=0
            else
                exit_status=$?
            fi
            tail -100 "$output_log"
            for sidecar in \
                "$(db_path "$validator_num").snapshot-live-rollback.json" \
                "$(db_path "$validator_num").snapshot-live-rollback"; do
                if [[ -e "$sidecar" ]]; then
                    ls -ld "$sidecar" || true
                    du -sh "$sidecar" || true
                fi
            done
            df -h "$REPO_ROOT" /tmp || true
            fail "V${validator_num} exited with status ${exit_status} during fresh ${expected_role} join"
        fi
        network_slot="$(get_slot "$V1_RPC")"
        local_slot="$(get_slot "$rpc")"
        drift=$((network_slot - local_slot))
        observed_role="$(archive_v2_health_role "$rpc")"
        if [[ "$local_slot" -gt 0 && "$drift" -le 20 && "$observed_role" == "$expected_role" ]]; then
            assert_fresh_role_original_has_no_snapshot_transaction \
                "$(db_path "$validator_num")"
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
    discard_fresh_role_candidate_state "$state_dir" "$cold_dir"
    mkdir -p "$state_dir/home/.lichen"
    install -m 0600 "$identity_file" "$state_dir/validator-keypair.json"
    if [[ -f "$node_identity_file" ]]; then
        install -m 0600 "$node_identity_file" "$state_dir/home/.lichen/node_identity.json"
    fi
    assert_joiner_starts_without_copied_chain_state "$validator_num"
}

activate_checkpoint_bound_source_peer() {
    local validator_num source_log pid bound_root
    [[ "$ARCHIVE_V2_CHECKPOINT_BOUND_CATALOGS" == "1" ]] || return 0
    [[ -f "$ARCHIVE_V2_FRESH_JOIN_SOURCE_ROOT/catalog.av2" ]] \
        || fail "Checkpoint-bound source peer has no verified Archive V2 catalog"
    [[ -f /tmp/lichen-testnet/archive-v2-checkpoint-bound-v4/catalog.av2 ]] \
        || fail "Checkpoint-bound corroborating V4 peer has no independently verified Archive V2 catalog"

    log "Restarting V1 and V4 as independent checkpoint-bound full-archive transfer sources..."
    # Each source owns the complete legacy hot/cold tail after the older bound
    # catalog. Keep both declarations at the public-network minimum so the
    # whole accelerated chain is physically verified and the fresh node can
    # require two independently signed checkpoint corroborations.
    for validator_num in 1 4; do
        stop_validator_pid "${VALIDATOR_PIDS[$validator_num]:-}"
        wait_validator_resources_released "$validator_num" \
            || fail "V${validator_num} did not release resources before checkpoint-bound source activation"
        if [[ "$validator_num" == "1" ]]; then
            bound_root="$ARCHIVE_V2_FRESH_JOIN_SOURCE_ROOT"
            LICHEN_LOCAL_ARCHIVE_V2_ROOT_V1="$bound_root"
            LICHEN_LOCAL_ARCHIVE_V2_RECENT_HISTORY_SLOTS_V1="$ARCHIVE_V2_PUBLIC_MIN_RECENT_HISTORY_SLOTS"
        else
            bound_root="/tmp/lichen-testnet/archive-v2-checkpoint-bound-v4"
            LICHEN_LOCAL_ARCHIVE_V2_ROOT_V4="$bound_root"
            LICHEN_LOCAL_ARCHIVE_V2_RECENT_HISTORY_SLOTS_V4="$ARCHIVE_V2_PUBLIC_MIN_RECENT_HISTORY_SLOTS"
        fi
        CHECKPOINT_SOURCE_PEERS_ACTIVE=1
        source_log="/tmp/lichen-testnet/v${validator_num}-checkpoint-bound-source.log"
        start_archive_v2_validator "$validator_num" "$source_log"
        pid="$ARCHIVE_V2_STARTED_PID"
        VALIDATOR_PIDS[$validator_num]="$pid"
        VALIDATOR_LOGS[$validator_num]="$source_log"
        wait_for_existing_cluster_healthy "$ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS" \
            || fail "Checkpoint-bound V${validator_num} source did not restore a healthy four-validator cluster"
        [[ "$(archive_v2_health_role "$(rpc_port "$validator_num")")" == "full-archive" ]] \
            || fail "Checkpoint-bound V${validator_num} source did not admit the full-archive role"
        [[ "$(archive_v2_rpc_block_hash_with_retry "$(rpc_port "$validator_num")" 0)" == "$FRESH_ARCHIVE_V2_GENESIS_HASH" ]] \
            || fail "Checkpoint-bound V${validator_num} source did not serve verified genesis history"
        ok "V${validator_num} is serving the exact catalog binding required by checkpoint ${COMMON_CHECKPOINT_SLOT}"
    done
}

restore_current_archive_v2_source_peer() {
    local validator_num source_log pid
    [[ "$CHECKPOINT_SOURCE_PEERS_ACTIVE" == "1" ]] || return 0

    log "Restoring V1 and V4 to their current append-complete Archive V2 catalogs..."
    for validator_num in 1 4; do
        stop_validator_pid "${VALIDATOR_PIDS[$validator_num]:-}"
        wait_validator_resources_released "$validator_num" \
            || fail "V${validator_num} did not release resources before current-catalog restoration"
        if [[ "$validator_num" == "1" ]]; then
            unset \
                LICHEN_LOCAL_ARCHIVE_V2_ROOT_V1 \
                LICHEN_LOCAL_ARCHIVE_V2_RECENT_HISTORY_SLOTS_V1
        else
            unset \
                LICHEN_LOCAL_ARCHIVE_V2_ROOT_V4 \
                LICHEN_LOCAL_ARCHIVE_V2_RECENT_HISTORY_SLOTS_V4
        fi
        source_log="/tmp/lichen-testnet/v${validator_num}-current-catalog-restored.log"
        start_archive_v2_validator "$validator_num" "$source_log"
        pid="$ARCHIVE_V2_STARTED_PID"
        VALIDATOR_PIDS[$validator_num]="$pid"
        VALIDATOR_LOGS[$validator_num]="$source_log"
        wait_for_existing_cluster_healthy "$ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS" \
            || fail "V${validator_num} current-catalog restoration did not recover a healthy four-validator cluster"
        [[ "$(archive_v2_health_role "$(rpc_port "$validator_num")")" == "full-archive" ]] \
            || fail "V${validator_num} did not restore its current full-archive role"
        ok "V${validator_num} restored its current append-complete Archive V2 catalog"
    done
    CHECKPOINT_SOURCE_PEERS_ACTIVE=0
}

verify_fresh_archive_v2_role_rejoins() {
    local validator_num=3
    local original_state="/tmp/lichen-testnet/v3-full-role-state"
    local original_cold="/tmp/lichen-testnet/v3-full-role-cold"
    local identity_file="/tmp/lichen-testnet/v3-role-validator-keypair.json"
    local node_identity_file="/tmp/lichen-testnet/v3-role-node-identity.json"
    local role_log pid recent_slot network_slot error_message restored_pubkey original_role
    local original_state_saved=false

    activate_checkpoint_bound_source_peer

    original_role="$(archive_v2_health_role "$(rpc_port "$validator_num")")"
    if [[ "$original_role" != "full-archive" ]]; then
        stop_validator_pid "${VALIDATOR_PIDS[$validator_num]:-}"
        wait_validator_resources_released "$validator_num" \
            || fail "V3 did not release resources before its fresh full-archive join"
        rm -rf "$original_state" "$original_cold"
        install -m 0600 "$(db_path "$validator_num")/validator-keypair.json" "$identity_file"
        if [[ -f "$(db_path "$validator_num")/home/.lichen/node_identity.json" ]]; then
            install -m 0600 \
                "$(db_path "$validator_num")/home/.lichen/node_identity.json" \
                "$node_identity_file"
        else
            rm -f "$node_identity_file"
        fi
        assert_fresh_role_original_has_no_snapshot_transaction "$(db_path "$validator_num")"
        mv "$(db_path "$validator_num")" "$original_state"
        if [[ -d "$(cold_path "$validator_num")" ]]; then
            mv "$(cold_path "$validator_num")" "$original_cold"
        fi
        arm_fresh_role_restore "$validator_num" "$original_state" "$original_cold"
        original_state_saved=true

        reset_fresh_role_state_with_identity \
            "$validator_num" "$identity_file" "$node_identity_file"
        role_log="/tmp/lichen-testnet/v3-fresh-full-archive.log"
        start_archive_v2_validator "$validator_num" "$role_log"
        pid="$ARCHIVE_V2_STARTED_PID"
        VALIDATOR_PIDS[$validator_num]="$pid"
        wait_for_archive_v2_role_catchup \
            "$validator_num" "full-archive" "$pid" "$role_log"
    fi

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
    if ! $original_state_saved; then
        rm -rf "$original_state" "$original_cold"
        install -m 0600 "$(db_path "$validator_num")/validator-keypair.json" "$identity_file"
        if [[ -f "$(db_path "$validator_num")/home/.lichen/node_identity.json" ]]; then
            install -m 0600 \
                "$(db_path "$validator_num")/home/.lichen/node_identity.json" \
                "$node_identity_file"
        else
            rm -f "$node_identity_file"
        fi
        assert_fresh_role_original_has_no_snapshot_transaction "$(db_path "$validator_num")"
        mv "$(db_path "$validator_num")" "$original_state"
        if [[ -d "$(cold_path "$validator_num")" ]]; then
            mv "$(cold_path "$validator_num")" "$original_cold"
        fi
        arm_fresh_role_restore "$validator_num" "$original_state" "$original_cold"
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
    discard_fresh_role_candidate_state \
        "$(db_path "$validator_num")" \
        "$(cold_path "$validator_num")"
    mv "$original_state" "$(db_path "$validator_num")"
    if [[ -d "$original_cold" ]]; then
        mv "$original_cold" "$(cold_path "$validator_num")"
    fi
    disarm_fresh_role_restore
    unset \
        LICHEN_LOCAL_ARCHIVE_V2_ROLE_V3 \
        LICHEN_LOCAL_ARCHIVE_V2_ROOT_V3 \
        LICHEN_LOCAL_ARCHIVE_V2_CACHE_ROOT_V3 \
        LICHEN_LOCAL_ARCHIVE_V2_SOURCE_DIRS_V3 \
        LICHEN_LOCAL_ARCHIVE_V2_SOURCE_URLS_V3 \
        LICHEN_LOCAL_ARCHIVE_V2_SOURCE_CA_CERT_V3 \
        LICHEN_LOCAL_ARCHIVE_V2_SOURCE_BEARER_TOKEN_V3 \
        LICHEN_LOCAL_ARCHIVE_V2_RECENT_HISTORY_SLOTS_V3
    role_log="/tmp/lichen-testnet/v3-restored-full-state.log"
    if [[ -n "$original_role" ]]; then
        start_archive_v2_validator "$validator_num" "$role_log"
        pid="$ARCHIVE_V2_STARTED_PID"
    else
        LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet "$validator_num" \
            > "$role_log" 2>&1 &
        pid=$!
    fi
    VALIDATOR_PIDS[$validator_num]="$pid"
    VALIDATOR_LOGS[$validator_num]="$role_log"
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
    if [[ -n "$original_role" ]]; then
        [[ "$(archive_v2_health_role "$(rpc_port "$validator_num")")" == "$original_role" ]] \
            || fail "V3 did not restore its original ${original_role} Archive V2 role"
    fi
    restore_current_archive_v2_source_peer
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
    wait_for_archive_v2_runtime_convergence "V${validator_num} ${reason} restart"
    wait_for_returning_validator_voting_readiness \
        "$validator_num" \
        "$restart_log" \
        "V${validator_num} ${reason} restart"
    verify_chain_recovers_within_bft_window \
        "after V${validator_num} Archive V2 ${reason} restart" \
        "$V1_RPC"
}

wait_for_returning_validator_voting_readiness() {
    local validator_num=$1 output_log=$2 phase=$3
    local rpc start_slot local_slot network_slot drift stable_samples=0
    rpc="$(rpc_port "$validator_num")"

    for _ in $(seq 1 300); do
        if grep -q 'Returning validator completed guarded BFT readiness' "$output_log" \
            && grep -q 'Entering BFT consensus' "$output_log"; then
            start_slot="$(get_slot "$rpc")"
            break
        fi
        sleep 1
    done
    [[ -n "${start_slot:-}" ]] || {
        tail -120 "$output_log"
        fail "${phase} never completed guarded BFT-readiness admission"
    }

    # Admission itself exercises BFT startup readiness. Prove that the node
    # continues advancing after it starts voting so a post-admission storage
    # stall cannot silently reduce a four-validator set to an effective 2/4.
    for _ in $(seq 1 30); do
        sleep 1
        local_slot="$(get_slot "$rpc")"
        network_slot="$(get_slot "$V1_RPC")"
        drift=$((network_slot - local_slot))
        (( drift < 0 )) && drift=$((-drift))
        if (( drift <= ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS )); then
            stable_samples=$((stable_samples + 1))
        else
            stable_samples=0
        fi
        if (( local_slot >= start_slot + 3 && stable_samples >= 3 )); then
            ok "${phase} stayed tip-aligned after voting admission (${start_slot} -> ${local_slot}, drift=${drift})"
            return 0
        fi
    done

    tail -120 "$output_log"
    fail "${phase} stalled or drifted after voting admission"
}

wait_for_archive_v2_runtime_convergence() {
    local phase=$1
    wait_for_cluster_slot_spread "$ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS" 180 \
        || fail "Archive V2 role matrix did not reconverge after ${phase}"
    wait_for_cluster_finalized_spread "$ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS" 180 \
        || fail "Archive V2 finalized frontiers did not reconverge after ${phase}"
}

verify_archive_v2_runtime_role_matrix() {
    local genesis_hash=$1
    local v2_root="/tmp/lichen-testnet/archive-v2-v2"
    local v2_cache="/tmp/lichen-testnet/archive-v2-cache-v2"
    local v2_source="/tmp/lichen-testnet/archive-v2-replica-v2"
    local v2_source_offline="/tmp/lichen-testnet/archive-v2-replica-v2.offline"
    local v4_root="/tmp/lichen-testnet/archive-v2-v4"
    local v4_source="/tmp/lichen-testnet/archive-v2-replica-v4"
    local response error_message recent_slot before_slot after_slot corrupt_object candidate
    local consensus_migrated_deep_slot=""

    [[ "$MAX_VALIDATORS" -ge 4 ]] \
        || fail "Archive V2 runtime role matrix requires the mandatory four-validator topology"

    # The offline matrix proves immutable catalog/object parity, but an
    # established validator must still bind that role to its own stopped
    # state before catalog-covered reads become exclusively Archive V2. This
    # is the production migration boundary: without it, legacy hot/cold reads
    # intentionally remain available and a corruption drill would test the
    # pre-activation fallback rather than the admitted V2 role.
    log "Persisting stopped-state Archive V2 role admission before the runtime matrix..."
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        wait_validator_resources_released "$V_NUM" \
            || fail "V${V_NUM} still owns runtime resources before Archive V2 role bootstrap"
        bootstrap_established_archive_v2_role "$V_NUM"
    done

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
    wait_for_archive_v2_runtime_convergence "initial role admission"

    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        [[ "$(archive_v2_health_admitted_after_fresh_sync "$(rpc_port "$V_NUM")")" == "true" ]] \
            || fail "V${V_NUM} did not restore its stopped-state Archive V2 admission before the runtime matrix"
    done

    [[ "$(archive_v2_rpc_block_hash_with_retry "$(rpc_port 1)" 0 30)" == "$genesis_hash" ]] \
        || fail "Full-archive V1 did not serve verified genesis history"
    [[ "$(archive_v2_rpc_block_hash_with_retry "$(rpc_port 4)" 0 30)" == "$genesis_hash" ]] \
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
    wait_for_archive_v2_runtime_convergence "V4 corrupt-segment restart"
    wait_for_returning_validator_voting_readiness \
        4 \
        /tmp/lichen-testnet/v4-archive-v2-corrupt-segment.log \
        "V4 corrupt-segment restart"
    error_message="$(archive_v2_rpc_error_message "$(rpc_port 4)" 0 || true)"
    [[ "$error_message" == *"Archive V2 segment unavailable"* \
        || "$error_message" == *"not locally readable"* ]] \
        || fail "Full-archive V4 did not fail closed on its corrupt Archive V2 segment: ${error_message:-no RPC error}"
    find "$v4_root/quarantine" -type f -print -quit | grep -q . \
        || fail "Full-archive V4 did not quarantine its corrupt segment"
    ok "Corrupt full-archive segment was quarantined and catalog-covered history failed closed without legacy fallback"
    verify_chain_producing "while one full-archive segment is corrupt" "$V1_RPC" 10

    stop_validator_pid "${VALIDATOR_PIDS[4]:-}"
    wait_validator_resources_released 4 \
        || fail "V4 did not release resources before replica-backed segment repair"
    "$RELEASE_BIN_DIR/lichen-archive-v2" mirror \
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
    wait_for_archive_v2_runtime_convergence "V4 replica-backed repair"
    wait_for_returning_validator_voting_readiness \
        4 \
        /tmp/lichen-testnet/v4-archive-v2-repaired-segment.log \
        "V4 replica-backed repair"
    verify_chain_producing "after V4 replica-backed Archive V2 repair" "$V1_RPC" 10
    [[ "$(archive_v2_rpc_block_hash "$(rpc_port 4)" 0)" == "$genesis_hash" ]] \
        || fail "Full-archive V4 did not recover deep history from the repaired replica"
    ok "Corrupt full-archive segment was quarantined and recovered from another replica"

    recent_slot="$(get_slot "$(rpc_port 3)")"
    [[ "$recent_slot" -gt 0 ]] || fail "Consensus V3 has no recent hot-history tip"
    archive_v2_rpc_block_hash "$(rpc_port 3)" "$recent_slot" >/dev/null \
        || fail "Consensus V3 did not serve recent hot history"
    # This matrix reuses an established migration state after the durable,
    # state-bound role bootstrap above has made catalog-covered reads V2-only.
    # The migrated boundary depends on the actual bounded migration progress;
    # a fixed fraction of retention can still be hot and must remain servable.
    # Discover a non-genesis canonical block that full-archive V1 serves and
    # consensus V3 explicitly denies. The bounded scan fails closed if no such
    # migrated deep-history block exists.
    for candidate in $(seq 1 256); do
        response="$(archive_v2_rpc_block_hash "$(rpc_port 1)" "$candidate" 2>/dev/null || true)"
        [[ "$response" =~ ^[0-9a-fA-F]{64}$ ]] || continue
        error_message="$(
            archive_v2_rpc_error_message \
                "$(rpc_port 3)" \
                "$candidate" \
                || true
        )"
        if [[ "$error_message" == *"consensus"* ]]; then
            consensus_migrated_deep_slot="$candidate"
            break
        fi
    done
    [[ -n "$consensus_migrated_deep_slot" ]] \
        || fail "Could not prove consensus V3 denial for a canonical migrated deep-history block"
    ok "Consensus V3 denied canonical migrated deep slot ${consensus_migrated_deep_slot} while serving its recent hot tip"

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
    wait_for_archive_v2_runtime_convergence "V2 cache-corruption restart"
    wait_for_returning_validator_voting_readiness \
        2 \
        /tmp/lichen-testnet/v2-archive-v2-corrupt-cache.log \
        "V2 cache-corruption restart"
    verify_chain_producing "after V2 verified-cache corruption recovery" "$V1_RPC" 10
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
    wait_for_archive_v2_runtime_convergence "V2 empty-cache source outage"
    wait_for_returning_validator_voting_readiness \
        2 \
        /tmp/lichen-testnet/v2-archive-v2-empty-cache-outage.log \
        "V2 empty-cache source-outage restart"
    response="$(rpc_query_params "$(rpc_port 2)" getBlock "[0]")"
    if python3 -c 'import json,sys; raise SystemExit(0 if isinstance(json.load(sys.stdin).get("result"), dict) else 1)' <<< "$response"; then
        fail "Verified-cache V2 served deep history with both cache and source unavailable"
    fi
    before_slot="$(get_slot "$V1_RPC")"
    verify_chain_recovers_within_bft_window \
        "during verified-cache source outage" \
        "$V1_RPC"
    after_slot="$(get_slot "$V1_RPC")"
    [[ "$after_slot" -gt "$before_slot" ]] \
        || fail "Consensus did not advance independently of the Archive V2 source outage"

    mv "$v2_source_offline" "$v2_source"
    [[ "$(archive_v2_rpc_block_hash "$(rpc_port 2)" 0)" == "$genesis_hash" ]] \
        || fail "Verified-cache V2 did not recover after its authenticated source returned"
    ok "Archive V2 full/cache/consensus roles, cache persistence, source outage isolation, and recovery passed"
}

bootstrap_established_archive_v2_role() {
    local validator_num=$1
    local role root cache_root replica_root dry_json publish_json
    local -a args
    role="$(archive_v2_runtime_role "$validator_num")"
    root="/tmp/lichen-testnet/archive-v2-v${validator_num}"
    cache_root="/tmp/lichen-testnet/archive-v2-cache-v${validator_num}"
    replica_root="/tmp/lichen-testnet/archive-v2-replica-v${validator_num}"
    dry_json="/tmp/lichen-testnet/v${validator_num}-archive-v2-role-bootstrap-dry.json"
    publish_json="/tmp/lichen-testnet/v${validator_num}-archive-v2-role-bootstrap-publish.json"
    args=(
        role-bootstrap
        --state-dir "$(db_path "$validator_num")"
        --cold-store "$(cold_path "$validator_num")"
        --root "$root"
        --role "$role"
        --recent-history-slots "$LICHEN_COLD_RETENTION_SLOTS"
        --wal "$(db_path "$validator_num")/consensus.wal"
        --identity-file "$(db_path "$validator_num")/validator-keypair.json"
        --recovery-file "$(db_path "$validator_num")/genesis.json"
        --acknowledge-stopped-validator
        --acknowledge-low-space-legacy-retirement
        --allow-local-dev-short-history
    )
    if [[ "$role" == "verified-cache" ]]; then
        mkdir -p "$cache_root"
        args+=(
            --cache-root "$cache_root"
            --cache-quota-bytes 2147483648
            --source-root "$replica_root"
        )
    fi

    "$RELEASE_BIN_DIR/lichen-archive-v2" "${args[@]}" --dry-run > "$dry_json" \
        || fail "V${validator_num} stopped-state Archive V2 role-bootstrap dry run failed"
    "$RELEASE_BIN_DIR/lichen-archive-v2" "${args[@]}" > "$publish_json" \
        || fail "V${validator_num} stopped-state Archive V2 role-bootstrap publish failed"
    python3 - "$dry_json" "$publish_json" <<'PY' \
        || fail "Stopped-state Archive V2 role-bootstrap evidence is invalid"
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as fh:
    dry = json.load(fh)
with open(sys.argv[2], "r", encoding="utf-8") as fh:
    publish = json.load(fh)

if dry.get("operation") != "role_bootstrap" or dry.get("dry_run") is not True:
    raise SystemExit(1)
if dry.get("bootstrap_authorized") is not True or dry.get("state_admission_created") is not False:
    raise SystemExit(1)
if publish.get("operation") != "role_bootstrap" or publish.get("dry_run") is not False:
    raise SystemExit(1)
if publish.get("bootstrap_authorized") is not True or publish.get("state_admission_persisted") is not True:
    raise SystemExit(1)
if dry.get("state_admission_persisted") is False and publish.get("state_admission_created") is not True:
    raise SystemExit(1)
for field in (
    "role",
    "network_id",
    "genesis_hash",
    "catalog_root",
    "catalog_segments",
    "catalog_end_slot",
    "finalized_slot",
    "required_archive_end",
    "hot_start_slot",
):
    if dry.get(field) != publish.get(field):
        raise SystemExit(1)
PY
    ok "V${validator_num} stopped-state Archive V2 ${role} admission is durable and state-bound"
}

reconcile_archive_v2_checkpoint_catalogs() {
    local genesis_hash=$1
    local av2="$RELEASE_BIN_DIR/lichen-archive-v2"
    local minimum_finalized_slot=""
    local maximum_finalized_slot=0
    local finalized_slot stopped_spread finality_depth common_end
    local planned_checkpoint_slot minimum_checkpoint_slot
    local archive_root replica_root stage_root stage_replica status_json catalog_root catalog_end
    local existing_catalog_root existing_network existing_genesis existing_start existing_end
    local common_catalog_root=""

    log "Stopping all validators to publish one immutable Archive V2 checkpoint catalog..."
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        signal_validator_pid_tree "${VALIDATOR_PIDS[$V_NUM]:-}"
    done
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        stop_validator_pid "${VALIDATOR_PIDS[$V_NUM]:-}"
        wait_validator_resources_released "$V_NUM" \
            || fail "V${V_NUM} did not release resources for common Archive V2 catalog publication"
    done

    # Runtime caches and roots from a prior fresh-join drill are reproducible,
    # non-authoritative inputs. Retiring them before the stopped-state rebuild
    # prevents disposable test data from competing with the adaptive archive
    # reserve while all validator state and source history remain untouched.
    rm -rf \
        /tmp/lichen-testnet/archive-v2-cache-v2 \
        /tmp/lichen-testnet/archive-v2-fresh-cache-catalog-v3 \
        /tmp/lichen-testnet/archive-v2-fresh-cache-v3 \
        /tmp/lichen-testnet/archive-v2-fresh-consensus-v3 \
        /tmp/lichen-testnet/archive-v2-fresh-full-v3

    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        finalized_slot="$(archive_v2_source_finalized_slot "$V_NUM")"
        [[ "$finalized_slot" =~ ^[0-9]+$ ]] \
            || fail "V${V_NUM} returned an invalid stopped finalized slot: ${finalized_slot}"
        if [[ -z "$minimum_finalized_slot" || "$finalized_slot" -lt "$minimum_finalized_slot" ]]; then
            minimum_finalized_slot="$finalized_slot"
        fi
        if (( finalized_slot > maximum_finalized_slot )); then
            maximum_finalized_slot=$finalized_slot
        fi
    done
    stopped_spread=$((maximum_finalized_slot - minimum_finalized_slot))
    (( stopped_spread <= ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS )) \
        || fail "Stopped Archive V2 finalized spread ${stopped_spread} exceeds publication headroom ${ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS}"
    # Publish enough finalized overlap that a bounded checkpoint can retain the
    # configured hot suffix and a fresh node can download, replay,
    # and deliberately restart without its immutable catalog falling behind.
    # The slowest stopped validator owns the range bound; no faster peer may
    # authorize history beyond that independently finalized frontier.
    finality_depth=$((LICHEN_COLD_RETENTION_SLOTS - ARCHIVE_V2_FRESH_JOIN_CATALOG_HEADROOM_SLOTS))
    common_end=$((minimum_finalized_slot - finality_depth))
    (( common_end >= 0 )) \
        || fail "Common Archive V2 checkpoint catalog range is empty at finalized slot ${maximum_finalized_slot}"
    (( common_end <= minimum_finalized_slot - finality_depth )) \
        || fail "Common Archive V2 checkpoint catalog end ${common_end} violates the ${finality_depth}-slot finality depth"
    # Established low-space roles require the catalog through their own hot
    # boundary. Do not shorten that catalog to manufacture a checkpoint tail;
    # instead select the first cadence boundary far enough beyond the immutable
    # catalog for the independently configured fresh-join suffix.
    minimum_checkpoint_slot=$((common_end + ARCHIVE_V2_FRESH_JOIN_RECENT_HISTORY_SLOTS))
    if (( minimum_checkpoint_slot <= maximum_finalized_slot )); then
        minimum_checkpoint_slot=$((maximum_finalized_slot + 1))
    fi
    planned_checkpoint_slot=$(( ((minimum_checkpoint_slot + CHECKPOINT_INTERVAL_SLOTS - 1) / CHECKPOINT_INTERVAL_SLOTS) * CHECKPOINT_INTERVAL_SLOTS ))
    ARCHIVE_V2_CHECKPOINT_CATALOG_END="$common_end"
    ARCHIVE_V2_PLANNED_CHECKPOINT_SLOT="$planned_checkpoint_slot"

    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        archive_root="/tmp/lichen-testnet/archive-v2-v${V_NUM}"
        replica_root="/tmp/lichen-testnet/archive-v2-replica-v${V_NUM}"
        stage_root="${archive_root}.checkpoint-stage"
        stage_replica="${replica_root}.checkpoint-stage"
        rm -rf "$stage_root" "$stage_replica"

        # A capacity-gated or interrupted multi-node publication may already
        # have atomically promoted this exact deterministic range for an
        # earlier validator. Authenticate and reuse that node instead of
        # rebuilding a second temporary copy and consuming the same bounded
        # disk envelope again. Mixed or incomplete roots are never accepted.
        if [[ -f "$archive_root/catalog.av2" ]] \
            && status_json="$("$av2" status --root "$archive_root" 2>/dev/null)" \
            && read -r existing_catalog_root existing_network existing_genesis existing_start existing_end < <(python3 -c '
import json
import sys
status = json.load(sys.stdin)
slot_range = status.get("slot_range")
if not isinstance(slot_range, list) or len(slot_range) != 2:
    raise SystemExit(1)
print(
    status.get("catalog_root", ""),
    status.get("network_id", ""),
    status.get("genesis_hash", ""),
    slot_range[0],
    slot_range[1],
)
' <<< "$status_json") \
            && [[ "$existing_network" == "lichen-testnet-1"
                && "$existing_genesis" == "$genesis_hash"
                && "$existing_start" == "0"
                && "$existing_end" == "$common_end" ]]; then
            case "$V_NUM" in
                1)
                    "$av2" verify --root "$archive_root" --max-objects 10000 >/dev/null \
                        || fail "V1 existing common Archive V2 node copy failed resumed verification"
                    ;;
                2)
                    [[ -f "$replica_root/catalog.av2" ]] \
                        || fail "V2 existing common Archive V2 catalog has no source replica"
                    "$av2" verify --root "$replica_root" --max-objects 10000 >/dev/null \
                        || fail "V2 existing common Archive V2 replica failed resumed verification"
                    ;;
                3)
                    ;;
                4)
                    [[ -f "$replica_root/catalog.av2" ]] \
                        || fail "V4 existing common Archive V2 catalog has no repair replica"
                    "$av2" verify --root "$archive_root" --max-objects 10000 >/dev/null \
                        || fail "V4 existing common Archive V2 node copy failed resumed verification"
                    "$av2" verify --root "$replica_root" --max-objects 10000 >/dev/null \
                        || fail "V4 existing common Archive V2 replica failed resumed verification"
                    ;;
            esac
            if [[ -z "$common_catalog_root" ]]; then
                common_catalog_root="$existing_catalog_root"
            elif [[ "$existing_catalog_root" != "$common_catalog_root" ]]; then
                fail "V${V_NUM} existing common Archive V2 root ${existing_catalog_root} differs from ${common_catalog_root}"
            fi
            ok "V${V_NUM} reused fully verified common Archive V2 catalog ${existing_catalog_root}"
            continue
        fi

        "$av2" build \
            --state-dir "$(db_path "$V_NUM")" \
            --cold-store "$(cold_path "$V_NUM")" \
            --root "$stage_root" \
            --network-id lichen-testnet-1 \
            --genesis-hash "$genesis_hash" \
            --start-slot 0 \
            --end-slot "$common_end" \
            --finality-depth-slots "$finality_depth" \
            --zstd-level 6 \
            --frame-bytes 1048576 \
            --replica-root "$stage_replica" \
            --required-replicas 1 >/dev/null \
            || fail "V${V_NUM} common Archive V2 checkpoint catalog build failed"
        status_json="$("$av2" status --root "$stage_root")" \
            || fail "V${V_NUM} common Archive V2 checkpoint catalog status failed"
        read -r catalog_root catalog_end < <(python3 -c '
import json
import sys
status = json.load(sys.stdin)
slot_range = status.get("slot_range")
if not isinstance(slot_range, list) or len(slot_range) != 2:
    raise SystemExit(1)
print(status["catalog_root"], slot_range[1])
' <<< "$status_json") \
            || fail "V${V_NUM} common Archive V2 checkpoint catalog metadata is invalid"
        [[ "$catalog_end" == "$common_end" ]] \
            || fail "V${V_NUM} common Archive V2 catalog ended at ${catalog_end}, expected ${common_end}"
        if [[ -z "$common_catalog_root" ]]; then
            common_catalog_root="$catalog_root"
        elif [[ "$catalog_root" != "$common_catalog_root" ]]; then
            fail "V${V_NUM} independently built Archive V2 catalog root ${catalog_root}, expected ${common_catalog_root}"
        fi

        # Fully authenticate both independently written copies before replacing
        # the older role inputs. Promote one validator at a time and retain only
        # the artifacts its runtime role consumes. This bounds peak storage to
        # one node/replica build pair instead of eight cumulative objects while
        # preserving four independent deterministic-build proofs.
        "$av2" verify --root "$stage_root" --max-objects 10000 >/dev/null \
            || fail "V${V_NUM} common Archive V2 node copy failed full verification"
        "$av2" verify --root "$stage_replica" --max-objects 10000 >/dev/null \
            || fail "V${V_NUM} common Archive V2 replica copy failed full verification"
        rm -rf "$archive_root" "$replica_root"
        mv "$stage_root" "$archive_root"
        mv "$stage_replica" "$replica_root"
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
        ok "V${V_NUM} independently verified and promoted common Archive V2 catalog ${catalog_root}"
    done

    # Existing validators do not receive the fresh-join marker merely by
    # attaching a catalog. Exercise the same stopped-state bootstrap used by
    # low-space production migration: exact state/catalog/WAL/identity proof,
    # dry-run/publish parity, then one durable role-bound admission marker.
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        bootstrap_established_archive_v2_role "$V_NUM"
    done

    log "Restarting all Archive V2 roles on catalog ${common_catalog_root} through slot ${common_end}..."
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        ROLE_LOG="/tmp/lichen-testnet/v${V_NUM}-archive-v2-checkpoint-catalog.log"
        start_archive_v2_validator "$V_NUM" "$ROLE_LOG" "$KEEP_CLUSTER_ON_SUCCESS"
        VALIDATOR_PIDS[$V_NUM]="$ARCHIVE_V2_STARTED_PID"
    done
    if ! wait_for_existing_cluster_healthy 180; then
        for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
            tail -100 "/tmp/lichen-testnet/v${V_NUM}-archive-v2-checkpoint-catalog.log"
        done
        fail "Archive V2 roles did not recover after common catalog publication"
    fi
    wait_for_archive_v2_runtime_convergence "common checkpoint catalog publication"
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        [[ "$(archive_v2_health_admitted_after_fresh_sync "$(rpc_port "$V_NUM")")" == "true" ]] \
            || fail "V${V_NUM} did not restore its stopped-state Archive V2 admission"
    done
    verify_chain_producing "after common Archive V2 checkpoint catalog publication" "$V1_RPC" 10
    ok "All validators admitted common immutable Archive V2 catalog ${common_catalog_root} through slot ${common_end}"
}

wait_for_common_checkpoint() {
    local phase="${1:-parity}"
    local current_slot target_slot target_interval minimum_target remaining_slots slot_budget_secs deadline all_ready all_captured
    local materialization_deadline_armed=0
    local validator_pid validator_log log_size log_start
    local -a checkpoint_log_offsets=()
    current_slot="$(get_slot "$V1_RPC")"
    target_interval="$CHECKPOINT_INTERVAL_SLOTS"
    if [[ ! "$ARCHIVE_V2_CHECKPOINT_CATALOG_END" =~ ^[0-9]+$ ]]; then
        # Before a catalog handoff, the validator deliberately emits reachable
        # hot-repair checkpoints every 1,000 slots. Select that actual cadence
        # so the accelerated shared-disk gate does not wait for a 10,000-slot
        # boundary that individual nodes may skip while an earlier bounded
        # materialization is still active.
        target_interval="$PREACTIVATION_CHECKPOINT_INTERVAL_SLOTS"
    fi
    target_slot=$(( ((current_slot / target_interval) + 1) * target_interval ))
    # A checkpoint older than its configured hot suffix can never satisfy a
    # fresh join. Select a valid checkpoint before the long wait instead of
    # discovering the impossible combination after materialization. Public
    # runs retain the 50k default; hosted local-dev release gates explicitly
    # scale the same invariant to half of one 10k checkpoint interval.
    minimum_target="$ARCHIVE_V2_FRESH_JOIN_RECENT_HISTORY_SLOTS"
    if [[ "$ARCHIVE_V2_CHECKPOINT_CATALOG_END" =~ ^[0-9]+$ ]]; then
        minimum_target=$((ARCHIVE_V2_CHECKPOINT_CATALOG_END + ARCHIVE_V2_FRESH_JOIN_RECENT_HISTORY_SLOTS))
    fi
    if (( target_slot < minimum_target )); then
        target_slot=$(( ((minimum_target + CHECKPOINT_INTERVAL_SLOTS - 1) / CHECKPOINT_INTERVAL_SLOTS) * CHECKPOINT_INTERVAL_SLOTS ))
    fi
    if [[ "$ARCHIVE_V2_PLANNED_CHECKPOINT_SLOT" =~ ^[1-9][0-9]*$ \
        && "$ARCHIVE_V2_PLANNED_CHECKPOINT_SLOT" -gt "$current_slot" \
        && "$target_slot" -lt "$ARCHIVE_V2_PLANNED_CHECKPOINT_SLOT" ]]; then
        target_slot="$ARCHIVE_V2_PLANNED_CHECKPOINT_SLOT"
    fi
    remaining_slots=$((target_slot - current_slot))
    # Four-node BFT cadence is intentionally slower than the configured raw
    # slot timer. Budget conservatively for four committed slots per second,
    # then leave ten minutes for checkpoint materialization and fsync. A fixed
    # 600-second deadline cannot cover a production 10,000-slot cadence.
    slot_budget_secs=$(((remaining_slots + 3) / 4))
    deadline=$((SECONDS + slot_budget_secs + 600))
    log "Advancing to common ${phase} checkpoint slot ${target_slot} (timeout budget $((slot_budget_secs + 600))s)..."

    # Ignore historical failures in reused logs while still surfacing any new
    # fail-closed checkpoint decision immediately instead of waiting out the
    # whole slot/materialization budget.
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        validator_log="${VALIDATOR_LOGS[$V_NUM]:-$(log_path "$V_NUM")}"
        if [[ -f "$validator_log" ]]; then
            checkpoint_log_offsets[$V_NUM]="$(wc -c < "$validator_log" | tr -d '[:space:]')"
        else
            checkpoint_log_offsets[$V_NUM]=0
        fi
    done

    while (( SECONDS < deadline )); do
        all_ready=1
        all_captured=1
        for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
            validator_pid="${VALIDATOR_PIDS[$V_NUM]:-}"
            validator_log="${VALIDATOR_LOGS[$V_NUM]:-$(log_path "$V_NUM")}"
            if [[ -z "$validator_pid" ]] || ! kill -0 "$validator_pid" 2>/dev/null; then
                [[ ! -f "$validator_log" ]] || tail -80 "$validator_log"
                fail "V${V_NUM} exited while waiting for ${phase} checkpoint slot ${target_slot}"
            fi
            if [[ -f "$validator_log" ]]; then
                log_size="$(wc -c < "$validator_log" | tr -d '[:space:]')"
                log_start="${checkpoint_log_offsets[$V_NUM]:-0}"
                if (( log_size < log_start )); then
                    log_start=0
                fi
                if grep -Eq \
                    "Skipping checkpoint at slot ${target_slot}([ :]|$)|Failed to create checkpoint at slot ${target_slot}:|Periodic checkpoint background task failed at slot ${target_slot}:|terminally paused.*slot=${target_slot}([ ,]|$)" \
                    < <(tail -c "+$((log_start + 1))" "$validator_log"); then
                    tail -80 "$validator_log"
                    fail "V${V_NUM} failed closed while creating ${phase} checkpoint slot ${target_slot}"
                fi
                if ! grep -Fq \
                    "Captured exact raw checkpoint at slot ${target_slot};" \
                    < <(tail -c "+$((log_start + 1))" "$validator_log"); then
                    all_captured=0
                fi
            else
                all_captured=0
            fi
            if [[ ! -f "$(db_path "$V_NUM")/checkpoints/slot-${target_slot}/checkpoint_meta.json" ]]; then
                all_ready=0
            fi
        done
        if [[ "$all_ready" == "1" ]]; then
            COMMON_CHECKPOINT_SLOT="$target_slot"
            ok "All validators persisted ${phase} checkpoint slot ${target_slot}"
            return 0
        fi
        if [[ "$all_captured" == "1" && "$materialization_deadline_armed" == "0" ]]; then
            deadline=$((SECONDS + ARCHIVE_V2_CHECKPOINT_MATERIALIZATION_TIMEOUT_SECS))
            materialization_deadline_armed=1
            log "All validators captured exact slot ${target_slot}; allowing ${ARCHIVE_V2_CHECKPOINT_MATERIALIZATION_TIMEOUT_SECS}s for serialized bounded-history materialization"
        fi
        sleep 2
    done
    for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
        validator_log="${VALIDATOR_LOGS[$V_NUM]:-$(log_path "$V_NUM")}"
        if [[ -f "$validator_log" ]]; then
            warn "V${V_NUM} checkpoint materialization evidence:"
            grep -E \
                "Captured exact raw checkpoint at slot ${target_slot}|hot-repair checkpoint materialization|bounded hot-repair checkpoint category|checkpoint.*slot ${target_slot}" \
                "$validator_log" | tail -30 || true
        fi
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
            # Both schedulers can legitimately observe the first newly cold
            # slot in the same millisecond. Wait for durable state or timing
            # divergence instead of failing that safe boundary coincidence.
            if [[ "$success_delta" -ge 100
                || "$v1_cursor" -ne "$v2_cursor"
                || "$v1_migrated" -ne "$v2_migrated"
                || "$v1_scanned" -ne "$v2_scanned" ]]; then
                ok "Bounded cold migration advanced independently: V1 cursor=${v1_cursor} rows=${v1_migrated}; V2 cursor=${v2_cursor} rows=${v2_migrated}; completion delta=${success_delta}ms"
                verify_chain_producing "during bounded cold migration and deferred reclaim" "$V1_RPC" 10
                return 0
            fi
        fi
        sleep 2
    done

    warn "V1 Archive migration status: $v1_status"
    warn "V2 Archive migration status: $v2_status"
    fail "Bounded cold migration did not advance divergent durable state on both validators within 180s"
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

run_requested_user_journeys_and_post_parity() {
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
}

if [[ "$REUSE_EXISTING_CLUSTER" == "1" ]]; then
    if wait_for_existing_cluster_healthy "$REUSE_HEALTH_TIMEOUT_SECS"; then
        USING_EXISTING_CLUSTER=true
        declare -a ALL_PUBKEYS=()
        report_reused_cluster
        LOCAL_GATE_SUCCESS=1
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
        [[ -x "$RELEASE_BIN_DIR/$binary" ]] \
            || fail "LICHEN_SKIP_LOCAL_GATE_BUILD=1 requires $RELEASE_BIN_DIR/$binary"
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
declare -a VALIDATOR_LOGS=()
V1_RPC="$(rpc_port 1)"
V1_LOG="$(log_path 1)"
VCNT=0
SLOT=0
STAKED_CNT=0
mkdir -p /tmp/lichen-testnet

if [[ "$RESUME_AFTER_ARCHIVE_V2_RUNTIME_MATRIX" == "1" ]]; then
    log "Resuming after the exact Archive V2 runtime role and failure-recovery matrix..."
    stop_local_processes
    baseline_genesis_hash=""

    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        state_dir="$(db_path "$validator_num")"
        archive_root="/tmp/lichen-testnet/archive-v2-v${validator_num}"
        [[ -f "$state_dir/CURRENT" && -f "$state_dir/validator-keypair.json" ]] \
            || fail "Runtime-matrix resume requires V${validator_num}'s owned state and identity"
        [[ -f "$archive_root/catalog.av2" ]] \
            || fail "Runtime-matrix resume requires V${validator_num}'s authenticated Archive V2 catalog"

        read -r archive_network archive_genesis_hash < <(
            "$RELEASE_BIN_DIR/lichen-archive-v2" status --root "$archive_root" \
                | python3 -c '
import json
import sys
status = json.load(sys.stdin)
print(status["network_id"], status["genesis_hash"])
'
        ) || fail "V${validator_num} runtime-matrix resume catalog status is invalid"
        [[ "$archive_network" == "lichen-testnet-1" ]] \
            || fail "V${validator_num} runtime-matrix resume catalog has the wrong network"
        if [[ -z "$baseline_genesis_hash" ]]; then
            baseline_genesis_hash="$archive_genesis_hash"
        else
            [[ "$archive_genesis_hash" == "$baseline_genesis_hash" ]] \
                || fail "V${validator_num} Archive V2 genesis identity drifted after the runtime matrix"
        fi

        validator_pubkey="$(
            grep -m1 '"publicKeyBase58"' "$state_dir/validator-keypair.json" \
                | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/'
        )"
        [[ -n "$validator_pubkey" ]] \
            || fail "Runtime-matrix resume could not read V${validator_num}'s validator identity"
        for existing in "${ALL_PUBKEYS[@]:-}"; do
            [[ -z "$existing" || "$existing" != "$validator_pubkey" ]] \
                || fail "Runtime-matrix resume found duplicate validator identity $validator_pubkey"
        done
        ALL_PUBKEYS+=("$validator_pubkey")
    done
    V1_PUBKEY="${ALL_PUBKEYS[0]}"
    ARCHIVE_V2_GENESIS_HASH="$baseline_genesis_hash"

    reconcile_archive_v2_checkpoint_catalogs "$ARCHIVE_V2_GENESIS_HASH"
    verify_archive_v2_hot_checkpoint_profile
    prepare_archive_v2_fresh_join_roots
    verify_fresh_archive_v2_role_rejoins
    run_requested_user_journeys_and_post_parity

    echo ""
    ok "═══════════════════════════════════════════════════════════"
    ok "ALL RUNTIME-MATRIX-RESUMED TESTS PASSED: common catalog, stable checkpoint, fresh roles, and journeys"
    ok "═══════════════════════════════════════════════════════════"
    LOCAL_GATE_SUCCESS=1
    exit 0
elif [[ "$RESUME_AFTER_ARCHIVE_V2_COMMON_CATALOG" == "1" ]]; then
    log "Resuming from an exact four-way common Archive V2 catalog proof..."
    stop_local_processes
    baseline_genesis_hash=""
    baseline_catalog_end=""

    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        state_dir="$(db_path "$validator_num")"
        archive_root="/tmp/lichen-testnet/archive-v2-v${validator_num}"
        replica_root="/tmp/lichen-testnet/archive-v2-replica-v${validator_num}"
        [[ -f "$state_dir/CURRENT" && -f "$state_dir/validator-keypair.json" ]] \
            || fail "Common-catalog resume requires V${validator_num}'s owned state and identity"
        [[ -f "$archive_root/catalog.av2" ]] \
            || fail "Common-catalog resume requires V${validator_num}'s retained node catalog"
        if [[ "$validator_num" -eq 2 || "$validator_num" -eq 4 ]]; then
            [[ -f "$replica_root/catalog.av2" ]] \
                || fail "Common-catalog resume requires V${validator_num}'s retained role replica"
        fi

        read -r archive_catalog_root archive_genesis_hash archive_catalog_end < <(
            "$RELEASE_BIN_DIR/lichen-archive-v2" status --root "$archive_root" \
                | python3 -c '
import json
import sys
status = json.load(sys.stdin)
slot_range = status.get("slot_range")
if not isinstance(slot_range, list) or len(slot_range) != 2 or slot_range[0] != 0:
    raise SystemExit(1)
print(status["catalog_root"], status["genesis_hash"], slot_range[1])
'
        ) || fail "V${validator_num} common Archive V2 catalog status is invalid"
        [[ "$archive_catalog_root" == "$RESUME_EXPECTED_ARCHIVE_V2_ROOT" ]] \
            || fail "V${validator_num} Archive V2 root ${archive_catalog_root} differs from resumed common proof ${RESUME_EXPECTED_ARCHIVE_V2_ROOT}"
        if [[ -z "$baseline_genesis_hash" ]]; then
            baseline_genesis_hash="$archive_genesis_hash"
            baseline_catalog_end="$archive_catalog_end"
        else
            [[ "$archive_genesis_hash" == "$baseline_genesis_hash" ]] \
                || fail "V${validator_num} Archive V2 genesis identity drifted at common-catalog resume"
            [[ "$archive_catalog_end" == "$baseline_catalog_end" ]] \
                || fail "V${validator_num} Archive V2 catalog end drifted at common-catalog resume"
        fi

        case "$validator_num" in
            1)
                "$RELEASE_BIN_DIR/lichen-archive-v2" verify --root "$archive_root" --max-objects 10000 >/dev/null \
                    || fail "V1 full-archive common root failed resumed verification"
                ;;
            2)
                "$RELEASE_BIN_DIR/lichen-archive-v2" verify --root "$replica_root" --max-objects 10000 >/dev/null \
                    || fail "V2 source replica common root failed resumed verification"
                ;;
            3)
                # Consensus intentionally owns only the authenticated catalog.
                ;;
            4)
                "$RELEASE_BIN_DIR/lichen-archive-v2" verify --root "$archive_root" --max-objects 10000 >/dev/null \
                    || fail "V4 full-archive common root failed resumed verification"
                "$RELEASE_BIN_DIR/lichen-archive-v2" verify --root "$replica_root" --max-objects 10000 >/dev/null \
                    || fail "V4 repair replica common root failed resumed verification"
                ;;
        esac

        validator_pubkey="$(
            grep -m1 '"publicKeyBase58"' "$state_dir/validator-keypair.json" \
                | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/'
        )"
        [[ -n "$validator_pubkey" ]] \
            || fail "Common-catalog resume could not read V${validator_num}'s validator identity"
        for existing in "${ALL_PUBKEYS[@]:-}"; do
            [[ -z "$existing" || "$existing" != "$validator_pubkey" ]] \
                || fail "Common-catalog resume found duplicate validator identity $validator_pubkey"
        done
        ALL_PUBKEYS+=("$validator_pubkey")
        ok "V${validator_num} common Archive V2 proof root verified: ${archive_catalog_root}"
    done
    V1_PUBKEY="${ALL_PUBKEYS[0]}"
    ARCHIVE_V2_GENESIS_HASH="$baseline_genesis_hash"
    ARCHIVE_V2_CHECKPOINT_CATALOG_END="$baseline_catalog_end"

    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        bootstrap_established_archive_v2_role "$validator_num"
    done
    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        resume_log="/tmp/lichen-testnet/v${validator_num}-archive-v2-common-catalog-resume.log"
        start_archive_v2_validator "$validator_num" "$resume_log"
        VALIDATOR_PIDS[$validator_num]="$ARCHIVE_V2_STARTED_PID"
    done
    wait_for_existing_cluster_healthy "$ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS" \
        || fail "Common-catalog resume did not restore a healthy four-validator cluster"
    wait_for_archive_v2_runtime_convergence "common-catalog exact-gate resume"
    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        [[ "$(archive_v2_health_admitted_after_fresh_sync "$(rpc_port "$validator_num")")" == "true" ]] \
            || fail "V${validator_num} did not restore its common-catalog Archive V2 admission"
    done
    [[ "$(archive_v2_rpc_block_hash_with_retry "$(rpc_port 1)" 0 30)" == "$ARCHIVE_V2_GENESIS_HASH" ]] \
        || fail "Resumed full-archive V1 did not serve verified genesis history"
    [[ "$(archive_v2_rpc_block_hash_with_retry "$(rpc_port 2)" 0 30)" == "$ARCHIVE_V2_GENESIS_HASH" ]] \
        || fail "Resumed verified-cache V2 did not serve verified genesis history"
    [[ "$(archive_v2_rpc_block_hash_with_retry "$(rpc_port 4)" 0 30)" == "$ARCHIVE_V2_GENESIS_HASH" ]] \
        || fail "Resumed full-archive V4 did not serve verified genesis history"
    consensus_error="$(archive_v2_rpc_error_message "$(rpc_port 3)" 0 || true)"
    [[ "$consensus_error" == *"consensus"* ]] \
        || fail "Resumed consensus V3 did not deny deep history: ${consensus_error:-none}"
    verify_chain_producing "after common Archive V2 catalog resume" "$V1_RPC" 10

    verify_archive_v2_hot_checkpoint_profile
    prepare_archive_v2_fresh_join_roots
    verify_fresh_archive_v2_role_rejoins
    run_requested_user_journeys_and_post_parity

    echo ""
    ok "═══════════════════════════════════════════════════════════"
    ok "ALL COMMON-CATALOG-RESUMED TESTS PASSED: stable checkpoint and fresh full/cache/consensus joins"
    ok "═══════════════════════════════════════════════════════════"
    LOCAL_GATE_SUCCESS=1
    exit 0
elif [[ "$RESUME_AFTER_ARCHIVE_V2_OFFLINE_MATRIX" == "1" ]]; then
    log "Resuming after an exact independently verified Archive V2 offline matrix..."
    stop_local_processes
    baseline_genesis_hash=""

    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        state_dir="$(db_path "$validator_num")"
        archive_root="/tmp/lichen-testnet/archive-v2-v${validator_num}"
        replica_root="/tmp/lichen-testnet/archive-v2-replica-v${validator_num}"
        [[ -f "$state_dir/CURRENT" && -f "$state_dir/validator-keypair.json" ]] \
            || fail "Archive V2 offline-matrix resume requires V${validator_num}'s owned state and identity"
        [[ -f "$archive_root/catalog.av2" ]] \
            || fail "Archive V2 offline-matrix resume requires V${validator_num}'s retained node catalog"
        if [[ "$validator_num" -eq 2 || "$validator_num" -eq 4 ]]; then
            [[ -f "$replica_root/catalog.av2" ]] \
                || fail "Archive V2 offline-matrix resume requires V${validator_num}'s retained role replica"
        fi

        read -r archive_catalog_root archive_genesis_hash < <(
            "$RELEASE_BIN_DIR/lichen-archive-v2" status --root "$archive_root" \
                | python3 -c '
import json
import sys
status = json.load(sys.stdin)
print(status["catalog_root"], status["genesis_hash"])
'
        ) || fail "V${validator_num} Archive V2 resume status is invalid"
        [[ "$archive_catalog_root" == "$RESUME_EXPECTED_ARCHIVE_V2_ROOT" ]] \
            || fail "V${validator_num} Archive V2 root ${archive_catalog_root} differs from resumed proof ${RESUME_EXPECTED_ARCHIVE_V2_ROOT}"
        if [[ "$validator_num" -eq 2 || "$validator_num" -eq 4 ]]; then
            replica_catalog_root="$(
                "$RELEASE_BIN_DIR/lichen-archive-v2" status --root "$replica_root" \
                    | python3 -c 'import json,sys; print(json.load(sys.stdin)["catalog_root"])'
            )" || fail "V${validator_num} Archive V2 replica status is invalid"
            [[ "$replica_catalog_root" == "$RESUME_EXPECTED_ARCHIVE_V2_ROOT" ]] \
                || fail "V${validator_num} replica root ${replica_catalog_root} differs from resumed proof ${RESUME_EXPECTED_ARCHIVE_V2_ROOT}"
        fi
        if [[ -z "$baseline_genesis_hash" ]]; then
            baseline_genesis_hash="$archive_genesis_hash"
        else
            [[ "$archive_genesis_hash" == "$baseline_genesis_hash" ]] \
                || fail "V${validator_num} Archive V2 genesis identity drifted at offline-matrix resume"
        fi

        case "$validator_num" in
            1)
                "$RELEASE_BIN_DIR/lichen-archive-v2" verify --root "$archive_root" --max-objects 10000 >/dev/null \
                    || fail "V1 full-archive node root failed resumed full verification"
                ;;
            2)
                "$RELEASE_BIN_DIR/lichen-archive-v2" verify --root "$replica_root" --max-objects 10000 >/dev/null \
                    || fail "V2 verified-cache source replica failed resumed full verification"
                ;;
            3)
                # Consensus role intentionally retains only the authenticated
                # catalog; it must not own or serve migrated deep-history data.
                ;;
            4)
                "$RELEASE_BIN_DIR/lichen-archive-v2" verify --root "$archive_root" --max-objects 10000 >/dev/null \
                    || fail "V4 full-archive node root failed resumed full verification"
                "$RELEASE_BIN_DIR/lichen-archive-v2" verify --root "$replica_root" --max-objects 10000 >/dev/null \
                    || fail "V4 repair replica failed resumed full verification"
                ;;
        esac

        validator_pubkey="$(
            grep -m1 '"publicKeyBase58"' "$state_dir/validator-keypair.json" \
                | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/'
        )"
        [[ -n "$validator_pubkey" ]] \
            || fail "Archive V2 offline-matrix resume could not read V${validator_num}'s validator identity"
        for existing in "${ALL_PUBKEYS[@]:-}"; do
            [[ -z "$existing" || "$existing" != "$validator_pubkey" ]] \
                || fail "Archive V2 offline-matrix resume found duplicate validator identity $validator_pubkey"
        done
        ALL_PUBKEYS+=("$validator_pubkey")
        ok "V${validator_num} resumed Archive V2 proof root verified: ${archive_catalog_root}"
    done
    V1_PUBKEY="${ALL_PUBKEYS[0]}"
    ARCHIVE_V2_GENESIS_HASH="$baseline_genesis_hash"

    verify_archive_v2_runtime_role_matrix "$ARCHIVE_V2_GENESIS_HASH"
    verify_chain_producing "after resumed Archive V2 role admission" "$V1_RPC" 10
    reconcile_archive_v2_checkpoint_catalogs "$ARCHIVE_V2_GENESIS_HASH"
    verify_archive_v2_hot_checkpoint_profile
    prepare_archive_v2_fresh_join_roots
    verify_fresh_archive_v2_role_rejoins
    run_requested_user_journeys_and_post_parity

    echo ""
    ok "═══════════════════════════════════════════════════════════"
    ok "ALL OFFLINE-MATRIX-RESUMED TESTS PASSED: roles, stable checkpoint, and fresh joins"
    ok "═══════════════════════════════════════════════════════════"
    LOCAL_GATE_SUCCESS=1
    exit 0
elif [[ "$RESUME_AFTER_ARCHIVE_V2_CHECKPOINT" == "1" ]]; then
    log "Resuming diagnostic fresh-role verification from an already proven Archive V2 hot checkpoint..."
    stop_local_processes
    COMMON_CHECKPOINT_SLOT="$RESUME_PUBLIC_PARITY_CHECKPOINT"
    baseline_manifest_root=""
    baseline_profile_root=""
    baseline_handoff_root=""
    baseline_archive_genesis_hash=""
    baseline_archive_catalog_end=""
    baseline_profile_start=""
    legacy_profile_count=0
    insufficient_profile_count=0
    checkpoint_catalog_rebuild_required=0

    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        state_dir="$(db_path "$validator_num")"
        checkpoint_dir="$state_dir/checkpoints/slot-${RESUME_PUBLIC_PARITY_CHECKPOINT}"
        checkpoint_meta="$checkpoint_dir/checkpoint_meta.json"
        checkpoint_manifest="/tmp/lichen-testnet/checkpoint-snapshot-v${validator_num}.json"
        archive_root="/tmp/lichen-testnet/archive-v2-v${validator_num}"
        [[ -f "$state_dir/CURRENT" ]] \
            || fail "Archive-checkpoint resume requires V${validator_num}'s RocksDB CURRENT file"
        [[ -f "$state_dir/validator-keypair.json" ]] \
            || fail "Archive-checkpoint resume requires V${validator_num}'s node-owned validator identity"
        [[ -f "$checkpoint_meta" && -f "$checkpoint_manifest" ]] \
            || fail "Archive-checkpoint resume requires V${validator_num}'s checkpoint ${RESUME_PUBLIC_PARITY_CHECKPOINT} and prior full manifest evidence"
        [[ -f "$archive_root/catalog.av2" ]] \
            || fail "Archive-checkpoint resume requires V${validator_num}'s reconciled Archive V2 catalog"

        validator_pubkey="$(
            grep -m1 '"publicKeyBase58"' "$state_dir/validator-keypair.json" \
                | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/'
        )"
        [[ -n "$validator_pubkey" ]] \
            || fail "Archive-checkpoint resume could not read V${validator_num}'s validator identity"
        for existing in "${ALL_PUBKEYS[@]:-}"; do
            [[ -z "$existing" || "$existing" != "$validator_pubkey" ]] \
                || fail "Archive-checkpoint resume found duplicate validator identity $validator_pubkey"
        done
        ALL_PUBKEYS+=("$validator_pubkey")

        read -r profile_start profile_root manifest_root < <(python3 - "$checkpoint_meta" "$checkpoint_manifest" "$RESUME_PUBLIC_PARITY_CHECKPOINT" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as fh:
    meta = json.load(fh)
with open(sys.argv[2], "r", encoding="utf-8") as fh:
    manifest = json.load(fh)
slot = int(sys.argv[3])
profile = meta.get("snapshot_profile", {})
root = profile.get("archive_v2_catalog_root")
if (
    meta.get("slot") != slot
    or profile.get("kind") != "hot_repair_v1"
    or not isinstance(profile.get("history_start_slot"), int)
    or not isinstance(root, list)
    or len(root) != 32
    or manifest.get("slot") != slot
    or manifest.get("snapshot_profile") != profile
):
    raise SystemExit(1)
print(profile["history_start_slot"], bytes(root).hex(), manifest["manifest_root"])
PY
        ) || fail "V${validator_num} checkpoint evidence is not a valid catalog-bound hot-repair profile"
        required_profile_start=$((RESUME_PUBLIC_PARITY_CHECKPOINT - ARCHIVE_V2_FRESH_JOIN_RECENT_HISTORY_SLOTS + 1))
        if (( required_profile_start <= 0 || profile_start > required_profile_start )); then
            insufficient_profile_count=$((insufficient_profile_count + 1))
        fi
        read -r catalog_root handoff_root catalog_end archive_genesis_hash < <(
            "$RELEASE_BIN_DIR/lichen-archive-v2" status \
                --root "$archive_root" \
                --history-start-slot "$profile_start" \
                | python3 -c '
import json
import sys
status = json.load(sys.stdin)
slot_range = status.get("slot_range")
handoff_root = status.get("checkpoint_handoff_root")
if not isinstance(slot_range, list) or len(slot_range) != 2 or not isinstance(handoff_root, str):
    raise SystemExit(1)
print(status["catalog_root"], handoff_root, slot_range[1], status["genesis_hash"])
'
        ) || fail "V${validator_num} reconciled Archive V2 catalog status is invalid"
        (( catalog_end >= profile_start - 1 )) \
            || fail "V${validator_num} catalog ends before its checkpoint predecessor"
        if [[ "$profile_root" != "$handoff_root" ]]; then
            legacy_profile_count=$((legacy_profile_count + 1))
            checkpoint_catalog_rebuild_required=1
        fi
        if [[ -z "$baseline_manifest_root" ]]; then
            baseline_manifest_root="$manifest_root"
            baseline_profile_root="$profile_root"
            baseline_handoff_root="$handoff_root"
            baseline_archive_genesis_hash="$archive_genesis_hash"
            baseline_archive_catalog_end="$catalog_end"
            baseline_profile_start="$profile_start"
        else
            [[ "$manifest_root" == "$baseline_manifest_root" ]] \
                || fail "Archive-checkpoint resume found V${validator_num} manifest-root drift"
            [[ "$profile_root" == "$baseline_profile_root" ]] \
                || fail "Archive-checkpoint resume found V${validator_num} checkpoint-profile root drift"
            [[ "$handoff_root" == "$baseline_handoff_root" ]] \
                || fail "Archive-checkpoint resume found V${validator_num} append-stable handoff-root drift"
            [[ "$archive_genesis_hash" == "$baseline_archive_genesis_hash" \
                && "$catalog_end" == "$baseline_archive_catalog_end" \
                && "$profile_start" == "$baseline_profile_start" ]] \
                || fail "Archive-checkpoint resume found V${validator_num} genesis, catalog-end, or history-start drift"
        fi
    done
    V1_PUBKEY="${ALL_PUBKEYS[0]}"
    # The admitted roles are already catalog-bound. Restore the catalog end
    # that a from-scratch run records during reconciliation so later checkpoint
    # selection keeps using the production 10,000-slot cadence instead of the
    # preactivation 1,000-slot cadence.
    ARCHIVE_V2_CHECKPOINT_CATALOG_END="$baseline_archive_catalog_end"
    ok "Revalidated four-way checkpoint ${RESUME_PUBLIC_PARITY_CHECKPOINT}: manifest=${baseline_manifest_root} profile=${baseline_profile_root} current_handoff=${baseline_handoff_root} legacy_extensions=${legacy_profile_count} insufficient_profiles=${insufficient_profile_count}"
    if [[ "$checkpoint_catalog_rebuild_required" == "1" ]]; then
        rebuild_archive_v2_checkpoint_catalog_evidence \
            "$baseline_profile_start" \
            "$baseline_profile_root" \
            "$baseline_archive_genesis_hash"
    fi

    resume_minimum_finalized=""
    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        resume_finalized="$(archive_v2_source_finalized_slot "$validator_num")"
        [[ "$resume_finalized" =~ ^[0-9]+$ ]] \
            || fail "V${validator_num} returned an invalid finalized slot before Archive V2 resume"
        if [[ -z "$resume_minimum_finalized" || "$resume_finalized" -lt "$resume_minimum_finalized" ]]; then
            resume_minimum_finalized="$resume_finalized"
        fi
    done
    if (( resume_minimum_finalized >= LICHEN_COLD_RETENTION_SLOTS )); then
        ARCHIVE_V2_RUNTIME_REFRESH_REQUIRED_END=$((resume_minimum_finalized - LICHEN_COLD_RETENTION_SLOTS))
        log "Pinned four-way Archive V2 runtime refresh end at ${ARCHIVE_V2_RUNTIME_REFRESH_REQUIRED_END}"
    fi

    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        resume_log="/tmp/lichen-testnet/v${validator_num}-archive-checkpoint-resume.log"
        start_archive_v2_validator "$validator_num" "$resume_log"
        VALIDATOR_PIDS[$validator_num]="$ARCHIVE_V2_STARTED_PID"
    done
    wait_for_existing_cluster_healthy "$ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS" \
        || fail "Archive-checkpoint resume did not restore a healthy four-validator cluster"
    ARCHIVE_V2_GENESIS_HASH="$(archive_v2_genesis_hash)" \
        || fail "Archive-checkpoint resume could not capture canonical genesis hash"
    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        resume_role="$(archive_v2_health_role "$(rpc_port "$validator_num")")"
        if [[ "$resume_role" == "consensus" ]]; then
            resume_error="$(archive_v2_rpc_error_message "$(rpc_port "$validator_num")" 0 || true)"
            [[ "$resume_error" == *"consensus"* ]] \
                || fail "Archive-checkpoint resume found a V${validator_num} consensus deep-history policy mismatch"
        else
            [[ "$(archive_v2_rpc_block_hash_with_retry "$(rpc_port "$validator_num")" 0)" == "$ARCHIVE_V2_GENESIS_HASH" ]] \
                || fail "Archive-checkpoint resume found a V${validator_num} genesis mismatch"
        fi
    done
    verify_chain_producing "after Archive V2 checkpoint exact-gate resume" "$V1_RPC" 10
    wait_for_cluster_finalized_spread \
        "$ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS" \
        "$ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS" \
        || fail "Archive-checkpoint resume finalized frontiers did not converge before fresh-role verification"

    if (( insufficient_profile_count > 0 )); then
        # A profile shorter than the requested hot suffix cannot prove
        # fresh-role admission. Reconcile a current four-way catalog and
        # capture a new checkpoint that commits a complete append-stable
        # handoff before joining.
        ARCHIVE_V2_FRESH_JOIN_SOURCE_ROOT=""
        ARCHIVE_V2_FRESH_JOIN_REPLICA_ROOT=""
        ARCHIVE_V2_CHECKPOINT_BOUND_CATALOGS=0
        reconcile_archive_v2_checkpoint_catalogs "$ARCHIVE_V2_GENESIS_HASH"
        wait_for_common_checkpoint "stable Archive V2 handoff"
    else
        # This checkpoint already commits the same append-stable handoff on all
        # four validators. Rebuilding the catalog would add no evidence and can
        # double temporary storage during an exact diagnostic resume.
        COMMON_CHECKPOINT_SLOT="$RESUME_PUBLIC_PARITY_CHECKPOINT"
        ok "Reusing proven append-stable Archive V2 checkpoint ${COMMON_CHECKPOINT_SLOT} for fresh-role verification"
    fi
    verify_archive_v2_hot_checkpoint_profile "$COMMON_CHECKPOINT_SLOT"
    prepare_archive_v2_fresh_join_roots
    verify_fresh_archive_v2_role_rejoins
    run_requested_user_journeys_and_post_parity

    echo ""
    ok "═══════════════════════════════════════════════════════════"
    ok "ALL CHECKPOINT-RESUMED FRESH ROLE TESTS PASSED: full archive, verified cache, authenticated source outage, and consensus"
    ok "═══════════════════════════════════════════════════════════"
    LOCAL_GATE_SUCCESS=1
    exit 0
elif [[ "$RESUME_AFTER_PUBLIC_PARITY" == "1" ]]; then
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
        VALIDATOR_LOGS[$validator_num]="$resume_log"
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
    wait_for_cluster_finalized_spread \
        "$ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS" \
        "$ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS" \
        || fail "Public-parity resume finalized frontiers did not converge before the Archive V2 stop"

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
    reconcile_archive_v2_checkpoint_catalogs "$ARCHIVE_V2_GENESIS_HASH"
    verify_archive_v2_hot_checkpoint_profile
    prepare_archive_v2_fresh_join_roots
    verify_fresh_archive_v2_role_rejoins
    run_requested_user_journeys_and_post_parity

    echo ""
    log "═══════════════════════════════════════════════════════════"
    ok "Slot: $RESUME_PUBLIC_PARITY_CHECKPOINT"
    ok "Validators: $MAX_VALIDATORS"
    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        ok "  V${validator_num}: ${ALL_PUBKEYS[$((validator_num - 1))]}"
    done
    echo ""
    ok "═══════════════════════════════════════════════════════════"
    ok "ALL RESUMED TAIL TESTS PASSED: immutable parity, Archive V2 roles, hot checkpoint, and fresh joins"
    ok "═══════════════════════════════════════════════════════════"
    LOCAL_GATE_SUCCESS=1
    exit 0
elif [[ "$RESUME_AFTER_RETENTION" == "1" || "$RESUME_AFTER_RESILIENCE" == "1" ]]; then
    if [[ "$RESUME_AFTER_RESILIENCE" == "1" ]]; then
        log "Resuming the exact gate from four independently owned post-resilience states..."
    else
        log "Resuming the exact gate from four independently owned post-retention states..."
    fi
    stop_local_processes

    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        state_dir="$(db_path "$validator_num")"
        [[ -f "$state_dir/CURRENT" ]] \
            || fail "Post-retention resume requires V${validator_num}'s RocksDB CURRENT file"
        [[ -f "$state_dir/validator-keypair.json" ]] \
            || fail "Post-retention resume requires V${validator_num}'s node-owned validator identity"

        validator_pubkey="$(
            grep -m1 '"publicKeyBase58"' "$state_dir/validator-keypair.json" \
                | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/'
        )"
        [[ -n "$validator_pubkey" ]] \
            || fail "Post-retention resume could not read V${validator_num}'s validator identity"
        for existing in "${ALL_PUBKEYS[@]:-}"; do
            [[ -z "$existing" || "$existing" != "$validator_pubkey" ]] \
                || fail "Post-retention resume found duplicate validator identity $validator_pubkey"
        done
        ALL_PUBKEYS+=("$validator_pubkey")

        resume_log="/tmp/lichen-testnet/v${validator_num}-post-retention-resume.log"
        LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet "$validator_num" \
            > "$resume_log" 2>&1 &
        VALIDATOR_PIDS[$validator_num]=$!
        VALIDATOR_LOGS[$validator_num]="$resume_log"
    done
    V1_PUBKEY="${ALL_PUBKEYS[0]}"

    if ! wait_for_existing_cluster_healthy "$ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS"; then
        for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
            tail -100 "${VALIDATOR_LOGS[$validator_num]}"
        done
        fail "Post-retention resume did not restore a healthy four-validator cluster"
    fi

    baseline_genesis_hash="$(archive_v2_rpc_block_hash "$(rpc_port 1)" 0)"
    [[ -n "$baseline_genesis_hash" ]] \
        || fail "Post-retention resume could not read V1 genesis hash"
    MIN_SLOT=999999999999
    MAX_SLOT=0
    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        resume_genesis_hash="$(archive_v2_rpc_block_hash "$(rpc_port "$validator_num")" 0)"
        [[ "$resume_genesis_hash" == "$baseline_genesis_hash" ]] \
            || fail "Post-retention resume found a V${validator_num} genesis mismatch"
        validator_slot="$(get_slot "$(rpc_port "$validator_num")")"
        (( validator_slot < MIN_SLOT )) && MIN_SLOT="$validator_slot"
        (( validator_slot > MAX_SLOT )) && MAX_SLOT="$validator_slot"
    done
    STAKED_CNT="$(get_staked_validator_count "$V1_RPC")"
    EPOCH_ACTIVE_CNT="$(get_epoch_active_validator_count "$V1_RPC")"
    VCNT="$(get_validator_count "$V1_RPC")"
    [[ "$STAKED_CNT" -eq "$MAX_VALIDATORS"
        && "$EPOCH_ACTIVE_CNT" -eq "$MAX_VALIDATORS"
        && "$VCNT" -eq "$MAX_VALIDATORS" ]] \
        || fail "Post-retention resume requires four staked, epoch-active, registered validators"
    [[ "$MIN_SLOT" -ge "$ARCHIVE_V2_RETENTION_PROOF_SLOT" ]] \
        || fail "Post-retention resume minimum slot ${MIN_SLOT} is below required ${ARCHIVE_V2_RETENTION_PROOF_SLOT}"
    [[ $((MAX_SLOT - MIN_SLOT)) -le 20 ]] \
        || fail "Post-retention resume head spread exceeds 20 slots: min=${MIN_SLOT} max=${MAX_SLOT}"
    verify_chain_producing "after post-retention exact-gate resume" "$V1_RPC" 10
    ok "Resumed four owned states at min=${MIN_SLOT} max=${MAX_SLOT} with matching genesis and healthy consensus"

    if [[ "$RESUME_AFTER_RESILIENCE" == "1" ]]; then
        # The preserved state has already passed loaded-backlog recovery,
        # one-validator live catch-up, own-state restarts, and a coordinated
        # all-validator restart. Continue at parity/checkpoint qualification
        # without replaying those confirmed phases.
        GENESIS_QUORUM_BOOTSTRAP=0
        SKIP_JOINER_RESTART_CHECK=1
    else
        GENESIS_QUORUM_BOOTSTRAP=1
    fi
    JOIN_START=$((MAX_VALIDATORS + 1))
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
        VALIDATOR_LOGS[$validator_num]="$resume_log"
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
# PHASE 1: Start the frozen-epoch genesis quorum
# ═══════════════════════════════════════════════════════════════
log "═══════════════════════════════════════════════════════════"
log "PHASE 1: Starting ${MAX_VALIDATORS}-validator genesis quorum"
log "═══════════════════════════════════════════════════════════"

# V1 owns genesis creation. The launcher pre-generates every local validator
# identity and includes each one in the frozen Staking V2 genesis epoch. Start
# the remaining validators as soon as genesis exists so no single-validator
# shortcut can make a nominal four-validator gate pass without BFT quorum.
LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet 1 \
    > "$V1_LOG" 2>&1 &
V1_PID=$!
VALIDATOR_PIDS[1]="$V1_PID"
VALIDATOR_LOGS[1]="$V1_LOG"
log "V1 started (PID: $V1_PID)"

GENESIS_READY=false
for i in $(seq 1 120); do
    sleep 1
    if ! kill -0 "$V1_PID" 2>/dev/null; then
        warn "V1 crashed! Log tail:"
        tail -40 "$V1_LOG"
        fail "V1 crashed during genesis creation"
    fi

    ALL_IDENTITIES_READY=true
    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        [[ -f "$(db_path "$validator_num")/validator-keypair.json" ]] \
            || ALL_IDENTITIES_READY=false
    done
    if [[ -f "$(db_path 1)/CURRENT" && -f "$(db_path 1)/genesis.json" ]] \
        && $ALL_IDENTITIES_READY; then
        GENESIS_READY=true
        break
    fi
done
$GENESIS_READY || fail "Genesis and all frozen-epoch validator identities were not ready within 120s"

GENESIS_CONFIGS_PROVISIONED=false
for i in $(seq 1 30); do
    ALL_GENESIS_CONFIGS_MATCH=true
    for validator_num in $(seq 2 "$MAX_VALIDATORS"); do
        cmp -s "$(db_path 1)/genesis.json" "$(db_path "$validator_num")/genesis.json" \
            || ALL_GENESIS_CONFIGS_MATCH=false
    done
    if $ALL_GENESIS_CONFIGS_MATCH; then
        GENESIS_CONFIGS_PROVISIONED=true
        break
    fi
    if ! kill -0 "$V1_PID" 2>/dev/null; then
        tail -40 "$V1_LOG"
        fail "V1 exited before provisioning the frozen-epoch genesis configs"
    fi
    sleep 1
done
$GENESIS_CONFIGS_PROVISIONED \
    || fail "The authoritative genesis config was not provisioned across all validators"
ok "Authoritative genesis config is provisioned across the frozen-epoch quorum"

ALL_PUBKEYS=()
for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
    validator_pubkey="$(grep -m1 '"publicKeyBase58"' "$(db_path "$validator_num")/validator-keypair.json" \
        | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
    [[ -n "$validator_pubkey" ]] || fail "Could not extract V${validator_num} genesis identity"
    if (( validator_num > 1 )); then
        for existing_index in $(seq 0 $((validator_num - 2))); do
            [[ "${ALL_PUBKEYS[$existing_index]}" != "$validator_pubkey" ]] \
                || fail "V${validator_num} has duplicate genesis identity $validator_pubkey"
        done
    fi
    ALL_PUBKEYS+=("$validator_pubkey")
    ok "V${validator_num} genesis pubkey: $validator_pubkey (unique)"
done
V1_PUBKEY="${ALL_PUBKEYS[0]}"

for validator_num in $(seq 2 "$MAX_VALIDATORS"); do
    assert_joiner_starts_without_copied_chain_state "$validator_num"
    validator_log="$(log_path "$validator_num")"
    LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet "$validator_num" \
        > "$validator_log" 2>&1 &
    VALIDATOR_PIDS[$validator_num]=$!
    VALIDATOR_LOGS[$validator_num]="$validator_log"
    log "V${validator_num} started from independent empty state (PID: ${VALIDATOR_PIDS[$validator_num]})"
done

log "Waiting for all frozen-epoch validators to sync and establish BFT finality..."
GENESIS_CLUSTER_READY=false
for i in $(seq 1 900); do
    sleep 2
    LIVE_COUNT=0
    MAX_SLOT=0
    MIN_SLOT=999999999999
    for validator_num in $(seq 1 "$MAX_VALIDATORS"); do
        validator_pid="${VALIDATOR_PIDS[$validator_num]:-}"
        if ! kill -0 "$validator_pid" 2>/dev/null; then
            warn "V${validator_num} crashed during genesis-quorum startup! Log tail:"
            tail -60 "${VALIDATOR_LOGS[$validator_num]}"
            fail "V${validator_num} crashed during genesis-quorum startup"
        fi
        validator_slot="$(get_slot "$(rpc_port "$validator_num")")"
        if [[ "$validator_slot" -gt 0 ]]; then
            LIVE_COUNT=$((LIVE_COUNT + 1))
            (( validator_slot > MAX_SLOT )) && MAX_SLOT="$validator_slot"
            (( validator_slot < MIN_SLOT )) && MIN_SLOT="$validator_slot"
        fi
    done
    STAKED_CNT="$(get_staked_validator_count "$V1_RPC")"
    EPOCH_ACTIVE_CNT="$(get_epoch_active_validator_count "$V1_RPC")"
    VCNT="$(get_validator_count "$V1_RPC")"
    SPREAD=$((MAX_SLOT - MIN_SLOT))
    if [[ "$LIVE_COUNT" -eq "$MAX_VALIDATORS"
        && "$STAKED_CNT" -eq "$MAX_VALIDATORS"
        && "$EPOCH_ACTIVE_CNT" -eq "$MAX_VALIDATORS"
        && "$VCNT" -eq "$MAX_VALIDATORS"
        && "$MIN_SLOT" -gt 3 && "$SPREAD" -le 20 ]]; then
        GENESIS_CLUSTER_READY=true
        break
    fi
    if [[ $((i % 15)) -eq 0 ]]; then
        log "  Genesis quorum: live=$LIVE_COUNT/$MAX_VALIDATORS staked=$STAKED_CNT epoch-active=$EPOCH_ACTIVE_CNT registered=$VCNT min=$MIN_SLOT max=$MAX_SLOT spread=$SPREAD"
    fi
done
$GENESIS_CLUSTER_READY || fail "The frozen-epoch genesis quorum did not become healthy within 1800s"

SLOT="$(get_slot "$V1_RPC")"
ok "Genesis quorum active: validators=$VCNT epoch-active=$EPOCH_ACTIVE_CNT slot=$SLOT spread=$SPREAD"
verify_chain_producing "with the complete frozen-epoch genesis quorum" "$V1_RPC" 10

if [[ "$MAX_VALIDATORS" -lt 2 ]]; then
    ok "PASS: Single validator test complete"
    LOCAL_GATE_SUCCESS=1
    exit 0
fi

# All validators joined from independently owned state during genesis-quorum
# startup. Later phases still remove and rebuild V3 across full/cache/consensus
# roles, so fresh high-tip archive admission remains covered.
GENESIS_QUORUM_BOOTSTRAP=1
JOIN_START=$((MAX_VALIDATORS + 1))
fi

if (( JOIN_START <= MAX_VALIDATORS )); then
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
        VALIDATOR_LOGS[$V_NUM]="$V_LOG"
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
    verify_chain_recovers_within_bft_window "during V${V_NUM} registration" "$V1_RPC"

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
    fi
done
fi

if [[ "$GENESIS_QUORUM_BOOTSTRAP" == "1" && "$MAX_VALIDATORS" -ge 4 ]]; then
    wait_for_archive_v2_retention_boundary
    verify_bounded_cold_migration_progress
fi

if [[ "$MAX_VALIDATORS" -ge 4 && "$SKIP_JOINER_RESTART_CHECK" != "1" ]]; then
    verify_loaded_backlog_liveness
fi

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
    wait_for_cluster_finalized_spread \
        "$ARCHIVE_V2_TEST_CATALOG_HEADROOM_SLOTS" \
        "$ARCHIVE_V2_FRESH_ROLE_TIMEOUT_SECS" \
        || fail "Validator finalized frontiers did not converge before the Archive V2 stop"
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
    reconcile_archive_v2_checkpoint_catalogs "$ARCHIVE_V2_GENESIS_HASH"
    verify_archive_v2_hot_checkpoint_profile
    prepare_archive_v2_fresh_join_roots
    verify_fresh_archive_v2_role_rejoins
    run_requested_user_journeys_and_post_parity
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
    LOCAL_GATE_SUCCESS=1
else
    fail "TEST FAILED: Not all validators are producing blocks!"
fi
