#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════
# Local Production Topology Test
# ═══════════════════════════════════════════════════════════════════════
#
# Simulates the EXACT production topology:
#
#   PHASE 1 — SEED/RELAY NETWORK (3 VPSes)
#     V1 (US)   Genesis validator, solo start      [run-validator.sh]
#     V2 (EU)   Seed file points at V1             [run-validator.sh]
#     V3 (SEA)  Seed file points at V1             [run-validator.sh]
#     → Wait for 3 staked validators, all producing blocks
#
#   PHASE 2 — AGENT VALIDATORS (3 agents joining)
#     V4 (Agent-1)  Seed file points at V1,V2,V3   [binary direct]
#     V5 (Agent-2)  Seed file points at V1,V2,V3   [binary direct]
#     V6 (Agent-3)  Seed file points at V1,V2,V3   [binary direct]
#     → Wait for 6 staked validators, all producing blocks
#
# Why two launch modes:
#   - Seed/relay VPSes use run-validator.sh (matches systemd deployment)
#   - Agent validators use the binary directly (agents don't use our
#     launcher — they download the binary and configure their own
#     seeds.json pointing at the seed network)
#
# Port scheme:
#   V1: p2p=7001  rpc=8899  ws=8900  (seed - US)
#   V2: p2p=7002  rpc=8901  ws=8902  (seed - EU)
#   V3: p2p=7003  rpc=8903  ws=8904  (seed - SEA)
#   V4: p2p=7004  rpc=8905  ws=8906  (agent-1)
#   V5: p2p=7005  rpc=8907  ws=8908  (agent-2)
#   V6: p2p=7006  rpc=8909  ws=8910  (agent-3)
#
# Data: $REPO_ROOT/data/state-{p2p_port}
#
# Usage:
#   bash tests/local-production-test.sh              # 3 seeds + 3 agents
#   bash tests/local-production-test.sh seeds         # 3 seeds only
#   bash tests/local-production-test.sh 2 0           # 2 seeds, 0 agents
#   bash tests/local-production-test.sh 3 2           # 3 seeds, 2 agents
# ═══════════════════════════════════════════════════════════════════════
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WARMUP_SLOTS=500

export LICHEN_LOCAL_DEV=1

# Parse arguments
if [[ "${1:-}" == "seeds" ]]; then
    SEED_COUNT=3
    AGENT_COUNT=0
elif [[ $# -ge 2 ]]; then
    SEED_COUNT="${1}"
    AGENT_COUNT="${2}"
elif [[ $# -eq 1 ]]; then
    SEED_COUNT="${1}"
    AGENT_COUNT=0
else
    SEED_COUNT=3
    AGENT_COUNT=3
fi

TOTAL_VALIDATORS=$((SEED_COUNT + AGENT_COUNT))

if [[ "$SEED_COUNT" -lt 1 ]]; then
    echo "Need at least 1 seed validator"
    exit 1
fi

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

log()  { echo -e "${CYAN}[TEST]${NC} $*"; }
ok()   { echo -e "${GREEN}[PASS]${NC} $*"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $*"; }
fail() { echo -e "${RED}[FAIL]${NC} $*"; exit 1; }
hdr()  { echo -e "\n${BOLD}${CYAN}$*${NC}"; }

# ── Port calculations ──
p2p_port() { echo $((7000 + $1)); }
rpc_port() { echo $((8899 + 2 * ($1 - 1))); }
ws_port()  { echo $((8900 + 2 * ($1 - 1))); }
db_path()  { echo "$REPO_ROOT/data/state-$(p2p_port $1)"; }
log_path() { echo "/tmp/lichen-prodtest/v${1}.log"; }

# ── Cleanup ──
cleanup() {
    log "Cleaning up all validators..."
    pkill -f "lichen-validator.*dev-mode" 2>/dev/null || true
    for n in $(seq 1 "$TOTAL_VALIDATORS"); do
        local pidfile
        pidfile="$(db_path $n)/validator.pid"
        if [[ -f "$pidfile" ]]; then
            kill "$(cat "$pidfile")" 2>/dev/null || true
        fi
    done
    sleep 2
    log "Cleanup done"
}
trap cleanup EXIT

# ── Preflight ──
[[ -x "$REPO_ROOT/target/release/lichen-validator" ]] || fail "Build first: cargo build --release"
[[ -x "$REPO_ROOT/target/release/lichen" ]]           || fail "Build first: cargo build --release"
[[ -x "$REPO_ROOT/run-validator.sh" ]]                 || fail "run-validator.sh not found"

# Real user home for ZK keys
REAL_HOME="$HOME"

# ═══════════════════════════════════════════════════════════════════════
# RPC HELPERS
# ═══════════════════════════════════════════════════════════════════════

rpc_call() {
    local port=$1 method=$2 params="${3:-null}"
    curl -sf --max-time 5 "http://127.0.0.1:${port}" -X POST \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":${params}}" 2>/dev/null || echo '{}'
}

get_slot() {
    rpc_call "$1" "getSlot" | python3 -c "import json,sys; print(json.load(sys.stdin).get('result',0))" 2>/dev/null || echo 0
}

# Count validators with actual stake (staked via bootstrap grant)
get_staked_count() {
    rpc_call "$1" "getValidators" | python3 -c "
import json,sys
try:
    r=json.load(sys.stdin).get('result',{})
    vs=r.get('validators',[]) if isinstance(r,dict) else []
    print(len([v for v in vs if v.get('stake',0) > 0]))
except: print(0)
" 2>/dev/null || echo 0
}

# Count ALL validators including routing-only entries
get_routing_count() {
    rpc_call "$1" "getValidators" | python3 -c "
import json,sys
try:
    r=json.load(sys.stdin).get('result',{})
    vs=r.get('validators',[]) if isinstance(r,dict) else []
    print(len(vs))
except: print(0)
" 2>/dev/null || echo 0
}

# Get a specific validator's stake by pubkey
get_validator_stake() {
    local port=$1 pubkey=$2
    rpc_call "$port" "getValidators" | python3 -c "
import json,sys
try:
    r=json.load(sys.stdin).get('result',{})
    vs=r.get('validators',[]) if isinstance(r,dict) else []
    match=[v for v in vs if v.get('pubkey','') == '${pubkey}']
    print(match[0].get('stake',0) if match else 0)
except: print(0)
" 2>/dev/null || echo 0
}

# Get balance for a pubkey
get_balance() {
    rpc_call "$1" "getBalance" "[\"$2\"]" | python3 -c "import json,sys; print(json.load(sys.stdin).get('result',{}).get('value',0))" 2>/dev/null || echo 0
}

verify_chain_producing() {
    local label=$1 rpc=$2 seconds=${3:-10}
    log "Verifying chain production ($label)..."
    local s1 s2 diff
    s1=$(get_slot "$rpc")
    sleep "$seconds"
    s2=$(get_slot "$rpc")
    diff=$((s2 - s1))
    if [[ "$diff" -lt 2 ]]; then
        warn "Chain may be stalled ($label): only $diff blocks in ${seconds}s"
        for n in $(seq 1 "$TOTAL_VALIDATORS"); do
            local lp
            lp="$(log_path $n)"
            [[ -f "$lp" ]] && { warn "=== V${n} log tail ==="; tail -25 "$lp"; }
        done
        fail "Chain stalled ($label)! $diff blocks in ${seconds}s (slot $s1 → $s2)"
    fi
    ok "Chain alive ($label): $diff blocks in ${seconds}s (slot $s1 → $s2)"
}

# Extract pubkey from keypair JSON file
extract_pubkey() {
    local file=$1
    grep -m1 '"publicKeyBase58"' "$file" 2>/dev/null \
        | sed -E 's/.*"publicKeyBase58"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/' || echo ""
}

# ═══════════════════════════════════════════════════════════════════════
# FLUSH: Kill everything, clean all state
# ═══════════════════════════════════════════════════════════════════════
hdr "═══════════════════════════════════════════════════════════"
hdr "FLUSH: Cleaning all local validator state"
hdr "═══════════════════════════════════════════════════════════"

pkill -f "lichen-validator" 2>/dev/null || true
sleep 2

for n in $(seq 1 "$TOTAL_VALIDATORS"); do
    local_db="$(db_path $n)"
    if [[ -d "$local_db" ]]; then
        rm -rf "$local_db"
        log "  Flushed $local_db"
    fi
done
# Also clean any leftover keypairs in HOME that might cause identity reuse
for n in $(seq 1 "$TOTAL_VALIDATORS"); do
    port=$(p2p_port $n)
    rm -f "$HOME/.lichen/validators/validator-${port}.json" 2>/dev/null || true
    rm -f "$HOME/.lichen/validators/validator-testnet.json" 2>/dev/null || true
done
mkdir -p /tmp/lichen-prodtest
ok "All state flushed"

echo ""
log "Test topology: ${SEED_COUNT} seed/relay + ${AGENT_COUNT} agent = ${TOTAL_VALIDATORS} total"
log "Warmup: ${WARMUP_SLOTS} slots per validator"
echo ""

# Track all pubkeys and PIDs
declare -a ALL_PUBKEYS=()
declare -a ALL_PIDS=()

# ═══════════════════════════════════════════════════════════════════════
# PHASE 1: SEED/RELAY NETWORK
# ═══════════════════════════════════════════════════════════════════════
# Simulates the 3 VPS seed/relay validators.
# V1 = genesis (US), V2 = joins V1 (EU), V3 = joins V1 (SEA)
# Uses run-validator.sh — same as production systemd deployment.
# ═══════════════════════════════════════════════════════════════════════

hdr "═══════════════════════════════════════════════════════════"
hdr "PHASE 1: Starting seed/relay network (${SEED_COUNT} VPS validators)"
hdr "═══════════════════════════════════════════════════════════"

# ── V1: Genesis validator ──
log "Starting V1 (US genesis)..."
V1_RPC=$(rpc_port 1)
V1_P2P=$(p2p_port 1)
V1_LOG="$(log_path 1)"

LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet 1 \
    > "$V1_LOG" 2>&1 &
V1_PID=$!
ALL_PIDS+=("$V1_PID")
log "  V1 PID=$V1_PID  RPC=:$V1_RPC  P2P=:$V1_P2P"

# Wait for V1 to produce blocks
log "Waiting for V1 genesis + block production..."
for i in $(seq 1 90); do
    sleep 2
    if ! kill -0 $V1_PID 2>/dev/null; then
        warn "V1 crashed! Log tail:"
        tail -40 "$V1_LOG"
        fail "V1 crashed during startup"
    fi
    SLOT=$(get_slot $V1_RPC)
    if [[ "$SLOT" -gt 3 ]]; then
        ok "V1 producing blocks (slot $SLOT)"
        break
    fi
    [[ $i -lt 90 ]] || fail "V1 failed to produce blocks after 180s"
done

# Extract V1 pubkey
for w in $(seq 1 10); do
    [[ -f "$(db_path 1)/validator-keypair.json" ]] && break
    sleep 1
done
V1_PUBKEY=$(extract_pubkey "$(db_path 1)/validator-keypair.json")
[[ -n "$V1_PUBKEY" ]] || fail "Could not extract V1 pubkey"
ALL_PUBKEYS+=("$V1_PUBKEY")
ok "V1 pubkey: $V1_PUBKEY"

# Sanity: exactly 1 validator at genesis
SCNT=$(get_staked_count $V1_RPC)
[[ "$SCNT" -eq 1 ]] || fail "Expected 1 staked validator at genesis, got $SCNT (seed isolation broken?)"
ok "Phase 1.1: V1 genesis confirmed (1 staked validator)"

# ── V2, V3: Joining seed validators ──
for V_NUM in $(seq 2 "$SEED_COUNT"); do
    ROLE="EU"
    [[ "$V_NUM" -eq 3 ]] && ROLE="SEA"

    log ""
    log "Starting V${V_NUM} (${ROLE} seed, bootstrapping from V1)..."
    V_RPC=$(rpc_port $V_NUM)
    V_P2P=$(p2p_port $V_NUM)
    V_LOG="$(log_path $V_NUM)"

    LICHEN_DISABLE_SUPERVISOR=1 "$REPO_ROOT/run-validator.sh" testnet "$V_NUM" \
        > "$V_LOG" 2>&1 &
    V_PID=$!
    ALL_PIDS+=("$V_PID")
    log "  V${V_NUM} PID=$V_PID  RPC=:$V_RPC  P2P=:$V_P2P"

    # Wait for keypair
    V_KEYPAIR="$(db_path $V_NUM)/validator-keypair.json"
    for w in $(seq 1 30); do
        [[ -f "$V_KEYPAIR" ]] && break
        sleep 1
    done
    V_PUBKEY=$(extract_pubkey "$V_KEYPAIR")
    [[ -n "$V_PUBKEY" ]] || fail "Could not extract V${V_NUM} pubkey"

    # Verify unique
    for existing in "${ALL_PUBKEYS[@]}"; do
        [[ "$existing" != "$V_PUBKEY" ]] || fail "V${V_NUM} DUPLICATE pubkey: $V_PUBKEY"
    done
    ALL_PUBKEYS+=("$V_PUBKEY")
    ok "V${V_NUM} pubkey: $V_PUBKEY (unique)"

    # Wait for registration with actual stake
    log "Waiting for V${V_NUM} to sync + register (checking stake on V1 RPC)..."
    REGISTERED=false
    REG_SLOT=0
    for i in $(seq 1 300); do
        sleep 2

        if ! kill -0 $V_PID 2>/dev/null; then
            warn "V${V_NUM} crashed! Log tail:"
            tail -40 "$V_LOG"
            fail "V${V_NUM} crashed"
        fi

        STAKED=$(get_staked_count $V1_RPC)
        if [[ "$STAKED" -ge "$V_NUM" ]]; then
            REG_SLOT=$(get_slot $V1_RPC)

            # Double-check: verify THIS validator has stake (not just count increase)
            V_STAKE=$(get_validator_stake $V1_RPC "$V_PUBKEY")
            if [[ "$V_STAKE" -gt 0 ]]; then
                ok "V${V_NUM} registered at slot ~$REG_SLOT (stake: $V_STAKE spores)"
                REGISTERED=true
                break
            fi
        fi

        # Progress every 30s
        if [[ $((i % 15)) -eq 0 ]]; then
            V_SLOT=$(get_slot $V_RPC 2>/dev/null || echo "?")
            NET_SLOT=$(get_slot $V1_RPC)
            log "  V${V_NUM}: local=$V_SLOT network=$NET_SLOT staked=$STAKED"
        fi

        [[ $i -lt 300 ]] || {
            warn "V${V_NUM} log tail:"
            tail -50 "$V_LOG"
            fail "V${V_NUM} did not register after 600s"
        }
    done

    $REGISTERED || fail "V${V_NUM} registration was not confirmed"

    # Verify chain still producing during registration
    verify_chain_producing "V${V_NUM} registration" "$V1_RPC" 10

    # Wait for activation warmup
    ACTIVATION_TARGET=$((REG_SLOT + WARMUP_SLOTS + 20))
    log "Waiting for V${V_NUM} warmup: activation after slot ~$ACTIVATION_TARGET..."
    for i in $(seq 1 600); do
        sleep 1
        NET_SLOT=$(get_slot $V1_RPC)
        if [[ "$NET_SLOT" -ge "$ACTIVATION_TARGET" ]]; then
            ok "V${V_NUM} warmup complete (slot $NET_SLOT >= $ACTIVATION_TARGET)"
            break
        fi
        if [[ $((i % 30)) -eq 0 ]]; then
            log "  Warmup: slot $NET_SLOT / $ACTIVATION_TARGET"
        fi
        if ! kill -0 $V_PID 2>/dev/null; then
            warn "V${V_NUM} crashed during warmup!"
            tail -30 "$V_LOG"
            fail "V${V_NUM} crashed during warmup"
        fi
        [[ $i -lt 600 ]] || fail "Warmup exceeded 600s (slot $NET_SLOT / $ACTIVATION_TARGET)"
    done

    # Post-activation: chain must still produce
    verify_chain_producing "V${V_NUM} post-activation" "$V1_RPC" 15
done

# ── Seed network summary ──
echo ""
hdr "SEED NETWORK ESTABLISHED"
SEED_SLOT=$(get_slot $V1_RPC)
SEED_STAKED=$(get_staked_count $V1_RPC)
SEED_ROUTING=$(get_routing_count $V1_RPC)
log "  Slot: $SEED_SLOT"
log "  Staked validators: $SEED_STAKED"
log "  Routing entries: $SEED_ROUTING"
for i in $(seq 0 $((SEED_COUNT - 1))); do
    log "  V$((i+1)): ${ALL_PUBKEYS[$i]}"
done

if [[ "$SEED_STAKED" -ne "$SEED_COUNT" ]]; then
    fail "Expected $SEED_COUNT staked validators, got $SEED_STAKED"
fi
ok "Phase 1 PASSED: ${SEED_COUNT} seed/relay validators active"

if [[ "$AGENT_COUNT" -eq 0 ]]; then
    # Skip to final verification
    echo ""
    hdr "═══════════════════════════════════════════════════════════"
    hdr "Skipping Phase 2 (no agent validators requested)"
    hdr "═══════════════════════════════════════════════════════════"
else

# ═══════════════════════════════════════════════════════════════════════
# PHASE 2: AGENT VALIDATORS
# ═══════════════════════════════════════════════════════════════════════
# Simulates agents joining the seed network.
# Agent validators do NOT use run-validator.sh — they use the binary
# directly with a state-scoped seeds.json pointing to ALL seed nodes.
# This matches production: agents download the binary and configure
# their own seeds.json.
# ═══════════════════════════════════════════════════════════════════════

echo ""
hdr "═══════════════════════════════════════════════════════════"
hdr "PHASE 2: Starting agent validators (${AGENT_COUNT} agents)"
hdr "═══════════════════════════════════════════════════════════"

# Build RPC endpoints for seed verification
SEED_RPCS=""
for s in $(seq 1 "$SEED_COUNT"); do
    [[ -n "$SEED_RPCS" ]] && SEED_RPCS="${SEED_RPCS},"
    SEED_RPCS="${SEED_RPCS}http://127.0.0.1:$(rpc_port $s)"
done

for A_IDX in $(seq 1 "$AGENT_COUNT"); do
    V_NUM=$((SEED_COUNT + A_IDX))
    V_P2P=$(p2p_port $V_NUM)
    V_RPC=$(rpc_port $V_NUM)
    V_WS=$(ws_port $V_NUM)
    V_DB="$(db_path $V_NUM)"
    V_HOME="${V_DB}/home"
    V_LOG="$(log_path $V_NUM)"
    V_SIGNER_PORT=$((9200 + V_NUM))

    log ""
    log "Starting V${V_NUM} (Agent-${A_IDX}, bootstrapping from seed network)..."

    # Set up isolated data directory (same as run-validator.sh does)
    mkdir -p "$V_HOME"

    # Write a seeds.json for this agent — lists ALL seed nodes
    # In production, agents would have the repo's seeds.json with VPS addresses.
    # Locally, we write one with localhost addresses for the seed network.
    cat > "${V_DB}/seeds.json" <<SEEDEOF
{
  "testnet": {
    "network_id": "lichen-testnet-local",
    "chain_id": "lichen-testnet-1",
    "seeds": [],
    "bootstrap_peers": [
$(for s in $(seq 1 "$SEED_COUNT"); do
    p=$(p2p_port $s)
    comma=""
    [[ $s -lt $SEED_COUNT ]] && comma=","
    echo "      \"127.0.0.1:${p}\"${comma}"
done)
    ],
    "rpc_endpoints": [
$(for s in $(seq 1 "$SEED_COUNT"); do
    r=$(rpc_port $s)
    comma=""
    [[ $s -lt $SEED_COUNT ]] && comma=","
    echo "      \"http://127.0.0.1:${r}\"${comma}"
done)
    ],
    "explorers": [],
    "faucets": []
  }
}
SEEDEOF

    # Set HOME isolation (same as run-validator.sh)
    # This ensures unique keypair, unique P2P identity, unique fingerprint
    export HOME="$V_HOME"
    export LICHEN_SIGNER_BIND="0.0.0.0:${V_SIGNER_PORT}"

    # Launch the validator binary DIRECTLY — this is how agents run in production.
    # No run-validator.sh, no systemd — just the binary with proper args.
    "$REPO_ROOT/target/release/lichen-validator" \
        --network testnet \
        --rpc-port "$V_RPC" \
        --ws-port "$V_WS" \
        --p2p-port "$V_P2P" \
        --listen-addr 127.0.0.1 \
        --db-path "$V_DB" \
        --dev-mode \
        > "$V_LOG" 2>&1 &
    V_PID=$!
    ALL_PIDS+=("$V_PID")

    # Restore HOME for the test script itself
    export HOME="$REAL_HOME"

    log "  V${V_NUM} PID=$V_PID  RPC=:$V_RPC  P2P=:$V_P2P  seeds.json=seed-network"

    # Wait for keypair to be auto-generated
    V_KEYPAIR="${V_DB}/validator-keypair.json"
    for w in $(seq 1 30); do
        [[ -f "$V_KEYPAIR" ]] && break
        sleep 1
    done
    V_PUBKEY=$(extract_pubkey "$V_KEYPAIR")
    [[ -n "$V_PUBKEY" ]] || fail "Could not extract V${V_NUM} pubkey"

    # Verify unique against ALL existing pubkeys
    for existing in "${ALL_PUBKEYS[@]}"; do
        [[ "$existing" != "$V_PUBKEY" ]] || fail "V${V_NUM} DUPLICATE pubkey: $V_PUBKEY"
    done
    ALL_PUBKEYS+=("$V_PUBKEY")
    ok "V${V_NUM} pubkey: $V_PUBKEY (unique)"

    # Wait for registration with actual stake — check on V1 (seed) RPC
    log "Waiting for V${V_NUM} (Agent-${A_IDX}) to sync + register..."
    REGISTERED=false
    REG_SLOT=0
    for i in $(seq 1 300); do
        sleep 2

        if ! kill -0 $V_PID 2>/dev/null; then
            warn "V${V_NUM} crashed! Log tail:"
            tail -40 "$V_LOG"
            fail "V${V_NUM} (Agent-${A_IDX}) crashed"
        fi

        # Check THIS specific validator's stake on V1
        V_STAKE=$(get_validator_stake $V1_RPC "$V_PUBKEY")
        if [[ "$V_STAKE" -gt 0 ]]; then
            REG_SLOT=$(get_slot $V1_RPC)
            ok "V${V_NUM} (Agent-${A_IDX}) registered at slot ~$REG_SLOT (stake: $V_STAKE spores)"
            REGISTERED=true
            break
        fi

        # Progress every 30s
        if [[ $((i % 15)) -eq 0 ]]; then
            V_SLOT=$(get_slot $V_RPC 2>/dev/null || echo "?")
            NET_SLOT=$(get_slot $V1_RPC)
            STAKED=$(get_staked_count $V1_RPC)
            log "  V${V_NUM}: local=$V_SLOT network=$NET_SLOT staked=$STAKED agent_stake=$V_STAKE"
        fi

        [[ $i -lt 300 ]] || {
            warn "V${V_NUM} log tail:"
            tail -50 "$V_LOG"
            STAKED=$(get_staked_count $V1_RPC)
            ROUTING=$(get_routing_count $V1_RPC)
            fail "V${V_NUM} (Agent-${A_IDX}) did not register after 600s (staked=$STAKED routing=$ROUTING)"
        }
    done

    $REGISTERED || fail "V${V_NUM} (Agent-${A_IDX}) registration was not confirmed"

    # Verify chain still producing
    verify_chain_producing "Agent-${A_IDX} registration" "$V1_RPC" 10

    # Wait for activation warmup
    ACTIVATION_TARGET=$((REG_SLOT + WARMUP_SLOTS + 20))
    log "Waiting for V${V_NUM} warmup: activation after slot ~$ACTIVATION_TARGET..."
    for i in $(seq 1 600); do
        sleep 1
        NET_SLOT=$(get_slot $V1_RPC)
        if [[ "$NET_SLOT" -ge "$ACTIVATION_TARGET" ]]; then
            ok "V${V_NUM} (Agent-${A_IDX}) warmup complete (slot $NET_SLOT >= $ACTIVATION_TARGET)"
            break
        fi
        if [[ $((i % 30)) -eq 0 ]]; then
            log "  Warmup: slot $NET_SLOT / $ACTIVATION_TARGET"
        fi
        if ! kill -0 $V_PID 2>/dev/null; then
            warn "V${V_NUM} crashed during warmup!"
            tail -30 "$V_LOG"
            fail "V${V_NUM} (Agent-${A_IDX}) crashed during warmup"
        fi
        [[ $i -lt 600 ]] || fail "Warmup exceeded 600s (slot $NET_SLOT / $ACTIVATION_TARGET)"
    done

    # Post-activation: chain must still produce
    verify_chain_producing "Agent-${A_IDX} post-activation" "$V1_RPC" 15
    ok "V${V_NUM} (Agent-${A_IDX}) ACTIVATED"
done

fi  # end AGENT_COUNT > 0

# ═══════════════════════════════════════════════════════════════════════
# FINAL VERIFICATION: All validators produce blocks
# ═══════════════════════════════════════════════════════════════════════

echo ""
hdr "═══════════════════════════════════════════════════════════"
hdr "FINAL: Verifying all ${TOTAL_VALIDATORS} validators produce blocks"
hdr "═══════════════════════════════════════════════════════════"

# Let the full network run for a while to accumulate block production
WAIT_SECS=$((TOTAL_VALIDATORS * 15 + 15))
log "Letting full network run ${WAIT_SECS}s to accumulate production..."
sleep "$WAIT_SECS"

# Check each validator's block production
PASS=true
for V_NUM in $(seq 1 "$TOTAL_VALIDATORS"); do
    V_PUBKEY="${ALL_PUBKEYS[$((V_NUM - 1))]}"
    V_LOG="$(log_path $V_NUM)"
    V_RPC=$(rpc_port $V_NUM)

    # Method 1: Check own logs for "Produced block"
    PRODUCED=$(grep -c "Produced block" "$V_LOG" 2>/dev/null || true)
    PRODUCED="${PRODUCED:-0}"

    # Method 2: Check V1 logs for this validator as proposer
    PROPOSED=$(grep "proposer=${V_PUBKEY}" "$(log_path 1)" 2>/dev/null | wc -l | tr -d ' ')

    # Method 3: Check validator's own slot is advancing
    V_SLOT=$(get_slot $V_RPC 2>/dev/null || echo 0)

    LABEL="seed"
    [[ $V_NUM -gt $SEED_COUNT ]] && LABEL="agent-$((V_NUM - SEED_COUNT))"

    if [[ "$PRODUCED" -gt 0 ]]; then
        ok "V${V_NUM} ($LABEL): produced=$PRODUCED blocks, slot=$V_SLOT"
    elif [[ "$PROPOSED" -gt 0 ]]; then
        ok "V${V_NUM} ($LABEL): proposed=$PROPOSED blocks (seen by V1), slot=$V_SLOT"
    else
        warn "V${V_NUM} ($LABEL): produced=0, proposed=0, slot=$V_SLOT — NOT producing!"
        tail -25 "$V_LOG"
        PASS=false
    fi
done

# ═══════════════════════════════════════════════════════════════════════
# CROSS-VALIDATION: Verify validators see each other
# ═══════════════════════════════════════════════════════════════════════

echo ""
log "Cross-validating: every node sees ${TOTAL_VALIDATORS} staked validators..."
CROSS_PASS=true
for V_NUM in $(seq 1 "$TOTAL_VALIDATORS"); do
    V_RPC=$(rpc_port $V_NUM)
    STAKED=$(get_staked_count $V_RPC 2>/dev/null || echo 0)
    if [[ "$STAKED" -ge "$TOTAL_VALIDATORS" ]]; then
        ok "V${V_NUM} sees $STAKED staked validators"
    else
        warn "V${V_NUM} sees only $STAKED staked validators (expected $TOTAL_VALIDATORS)"
        CROSS_PASS=false
    fi
done

# ═══════════════════════════════════════════════════════════════════════
# REPORT
# ═══════════════════════════════════════════════════════════════════════

echo ""
hdr "═══════════════════════════════════════════════════════════"
hdr "TEST REPORT"
hdr "═══════════════════════════════════════════════════════════"

FINAL_SLOT=$(get_slot $V1_RPC)
FINAL_STAKED=$(get_staked_count $V1_RPC)
FINAL_ROUTING=$(get_routing_count $V1_RPC)

echo ""
log "Network: $FINAL_SLOT slots produced"
log "Staked validators: $FINAL_STAKED"
log "Routing entries: $FINAL_ROUTING"
log "Topology: ${SEED_COUNT} seed/relay + ${AGENT_COUNT} agent"
echo ""
for V_NUM in $(seq 1 "$TOTAL_VALIDATORS"); do
    LABEL="seed"
    [[ $V_NUM -gt $SEED_COUNT ]] && LABEL="agent-$((V_NUM - SEED_COUNT))"
    log "  V${V_NUM} ($LABEL): ${ALL_PUBKEYS[$((V_NUM - 1))]}"
done
echo ""

if $PASS && $CROSS_PASS; then
    ok "═══════════════════════════════════════════════════════════"
    ok "ALL TESTS PASSED"
    ok "  ${SEED_COUNT} seed/relay validators: PRODUCING"
    ok "  ${AGENT_COUNT} agent validators: PRODUCING"
    ok "  Cross-validation: ALL nodes agree on validator set"
    ok "═══════════════════════════════════════════════════════════"
    exit 0
else
    fail "TEST FAILED: Not all validators producing or cross-validation failed"
fi
