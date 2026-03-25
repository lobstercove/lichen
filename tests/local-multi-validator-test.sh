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
#
# Data dirs: $REPO_ROOT/data/state-{port}
#
# Usage: bash tests/local-multi-validator-test.sh [max_validators]
#   Default: 3 validators.
# ═══════════════════════════════════════════════════════════════
set -euo pipefail

# Disable pagers to prevent interactive hangs in CI/automated runs
export PAGER=cat
export GIT_PAGER=cat
export LESS='-FRX'

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MAX_VALIDATORS="${1:-3}"
WARMUP_SLOTS=100  # Must match ACTIVATION_WARMUP in validator/src/main.rs

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

# Port calculations (must match run-validator.sh)
p2p_port()  { echo $((7000 + $1)); }
rpc_port()  { echo $((8899 + 2 * ($1 - 1))); }
db_path()   { echo "$REPO_ROOT/data/state-$(p2p_port $1)"; }
log_path()  { echo "/tmp/lichen-testnet/v${1}.log"; }

cleanup() {
    log "Cleaning up..."
    for n in $(seq 1 "$MAX_VALIDATORS"); do
        pidfile="$(db_path $n)/validator.pid"
        if [[ -f "$pidfile" ]]; then
            kill "$(cat "$pidfile")" 2>/dev/null || true
        fi
    done
    pkill -f "lichen-validator.*dev-mode" 2>/dev/null || true
    sleep 2
    log "Cleanup done"
}
trap cleanup EXIT

# ── Preflight ──
[[ -x "$REPO_ROOT/target/release/lichen-validator" ]] || fail "Build first: cargo build --release"
[[ -x "$REPO_ROOT/run-validator.sh" ]] || fail "run-validator.sh not found"

# ── RPC helpers ──
rpc_query() {
    local port=$1 method=$2
    curl -sf --max-time 3 "http://127.0.0.1:${port}" -X POST \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\"}" 2>/dev/null || echo '{}'
}

get_slot() {
    rpc_query "$1" "getSlot" | python3 -c "import json,sys; print(json.load(sys.stdin).get('result',0))" 2>/dev/null || echo 0
}

get_validator_count() {
    rpc_query "$1" "getValidators" | python3 -c "import json,sys; r=json.load(sys.stdin).get('result',{}); print(len(r.get('validators',[])) if isinstance(r,dict) else 0)" 2>/dev/null || echo 0
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

# ═══════════════════════════════════════════════════════════════
# FLUSH: Clean all local state
# ═══════════════════════════════════════════════════════════════
log "Flushing local state..."
pkill -f "lichen-validator" 2>/dev/null || true
sleep 2
for n in $(seq 1 "$MAX_VALIDATORS"); do
    local_db="$(db_path $n)"
    if [[ -d "$local_db" ]]; then
        rm -rf "$local_db"
        log "  Flushed $local_db"
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

for V_NUM in $(seq 2 "$MAX_VALIDATORS"); do
    log "═══════════════════════════════════════════════════════════"
    log "PHASE ${V_NUM}: Adding V${V_NUM} to network"
    log "═══════════════════════════════════════════════════════════"

    V_RPC=$(rpc_port $V_NUM)
    V_LOG="$(log_path $V_NUM)"

    LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet "$V_NUM" \
        > "$V_LOG" 2>&1 &
    V_PID=$!
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

    # Wait for activation warmup (500 slots after registration)
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
for V_NUM in $(seq 1 "$MAX_VALIDATORS"); do
    V_PUBKEY="${ALL_PUBKEYS[$((V_NUM - 1))]}"
    V_LOG="$(log_path $V_NUM)"

    PRODUCED=$(/usr/bin/grep -c "Produced block" "$V_LOG" 2>/dev/null || echo 0)

    if [[ "$PRODUCED" -gt 0 ]]; then
        ok "V${V_NUM} ($V_PUBKEY): produced=$PRODUCED blocks"
    else
        # Check if V1 saw blocks from this validator
        PROPOSED=$(grep "proposer=$V_PUBKEY" "$(log_path 1)" 2>/dev/null | wc -l | tr -d ' ')
        if [[ "$PROPOSED" -gt 0 ]]; then
            ok "V${V_NUM} ($V_PUBKEY): proposed=$PROPOSED blocks (seen on V1)"
        else
            warn "V${V_NUM} ($V_PUBKEY): produced=0, proposed=0 — NOT producing!"
            tail -20 "$V_LOG"
            PASS=false
        fi
    fi
done

# ═══════════════════════════════════════════════════════════════
# REPORT
# ═══════════════════════════════════════════════════════════════
echo ""
log "═══════════════════════════════════════════════════════════"
FINAL_SLOT=$(get_slot $V1_RPC)
FINAL_VCNT=$(get_validator_count $V1_RPC)
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
