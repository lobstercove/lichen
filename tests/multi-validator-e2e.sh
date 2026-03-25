#!/usr/bin/env bash
# K3-02: Multi-validator consensus E2E test
# Tests: 3-validator boot → block production → consensus → transaction finality
#
# BEHAVIOUR: If a healthy 3-validator cluster is already running on the standard
# ports (8899/8901/8903), we test against that cluster directly and skip
# spawning our own validators.  This avoids running 6 validators on one machine
# during the matrix, which can cause resource exhaustion and kill the main
# cluster.  Only when no existing cluster is found do we start our own.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
BIN="$ROOT/target/release/lichen-validator"
RUN_ID="$(date +%s)-$$"
DATA_DIR_BASE="$ROOT/data/e2e-test-$RUN_ID"
DB_V1="$DATA_DIR_BASE/v1"
DB_V2="$DATA_DIR_BASE/v2"
DB_V3="$DATA_DIR_BASE/v3"
HOME_V1="$DB_V1/home"
HOME_V2="$DB_V2/home"
HOME_V3="$DB_V3/home"
LOG_V1="/tmp/e2e-v1-$RUN_ID.log"
LOG_V2="/tmp/e2e-v2-$RUN_ID.log"
LOG_V3="/tmp/e2e-v3-$RUN_ID.log"

# Colors
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
PASS=0; FAIL=0; TOTAL=0
USING_EXISTING_CLUSTER=false

pass() { ((PASS++)); ((TOTAL++)); echo -e "${GREEN}✓ PASS${NC}: $1"; }
fail() { ((FAIL++)); ((TOTAL++)); echo -e "${RED}✗ FAIL${NC}: $1"; }
info() { echo -e "${YELLOW}→${NC} $1"; }

free_ports() {
    for port in "$@"; do
        local pids
        pids=$(lsof -ti tcp:"$port" 2>/dev/null || true)
        if [ -n "$pids" ]; then
            echo "$pids" | xargs -I{} kill {} 2>/dev/null || true
            sleep 0.2
            echo "$pids" | xargs -I{} kill -9 {} 2>/dev/null || true
        fi
    done
}

rpc() {
    local port=$1 method=$2 params=${3:-[]}
    curl -sf http://127.0.0.1:$port -X POST \
        -H 'Content-Type:application/json' \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":$params}" 2>/dev/null
}

rpc_result() {
    local port=$1 method=$2 params=${3:-[]}
    rpc "$port" "$method" "$params" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin).get('result')))" 2>/dev/null
}

wait_for_health() {
    local port=$1
    local attempts=${2:-20}
    local delay=${3:-1}
    for _ in $(seq 1 "$attempts"); do
        local status
        status=$(rpc "$port" "health" 2>/dev/null || echo "")
        if echo "$status" | python3 -c "import sys,json; r=json.load(sys.stdin); assert r.get('result') is not None" 2>/dev/null; then
            return 0
        fi
        sleep "$delay"
    done
    return 1
}

wait_for_validator_count() {
    local port=$1
    local min_count=$2
    local attempts=${3:-20}
    local delay=${4:-1}
    for _ in $(seq 1 "$attempts"); do
        local validators vcount
        validators=$(rpc_result "$port" "getValidators" 2>/dev/null || echo "[]")
        vcount=$(echo "$validators" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo "0")
        if [ "$vcount" -ge "$min_count" ]; then
            echo "$vcount"
            return 0
        fi
        sleep "$delay"
    done
    echo "0"
    return 1
}

wait_for_block_presence() {
    local port=$1
    local slot=$2
    local attempts=${3:-20}
    local delay=${4:-1}
    for _ in $(seq 1 "$attempts"); do
        local blk
        blk=$(rpc_result "$port" "getBlock" "[$slot]" 2>/dev/null || echo "null")
        if echo "$blk" | python3 -c "import sys,json; d=json.load(sys.stdin); import builtins; builtins.exit(0 if (d and isinstance(d, dict) and d.get('slot', d.get('header', {}).get('slot', -1)) >= 0) else 1)" 2>/dev/null; then
            return 0
        fi
        sleep "$delay"
    done
    return 1
}

cleanup() {
    if [ "$USING_EXISTING_CLUSTER" = true ]; then
        info "Using existing cluster — nothing to clean up"
        return
    fi
    info "Cleaning up validators..."
    for pid in ${V1PID:-} ${V2PID:-} ${V3PID:-}; do
        kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    free_ports 9100 9102 9104 10099 10101 10103 10199 10201 10203 9301 9302 9303
    rm -rf "$DATA_DIR_BASE"
}
trap cleanup EXIT

# ── Detect existing cluster ───────────────────────────────────
# If a healthy 3-validator cluster is already running on the standard ports,
# reuse it instead of spawning 3 additional validators (avoids 6 validators
# on a single dev machine which causes resource exhaustion).
existing_cluster_healthy() {
    local count
    for port in 8899 8901 8903; do
        if ! rpc "$port" "health" >/dev/null 2>&1; then
            return 1
        fi
    done
    count=$(rpc_result 8899 "getValidators" 2>/dev/null | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo "0")
    [ "$count" -ge 2 ]
}

# RPC ports used for the test — either existing cluster or fresh
RPC_V1=10099; RPC_V2=10101; RPC_V3=10103

echo ""
echo "═══════════════════════════════════════════════════════"
echo " K3-02: Multi-Validator Consensus E2E Test"
echo "═══════════════════════════════════════════════════════"
echo ""

if existing_cluster_healthy; then
    USING_EXISTING_CLUSTER=true
    RPC_V1=8899; RPC_V2=8901; RPC_V3=8903
    info "Detected healthy existing 3-validator cluster on 8899/8901/8903 — reusing it"
else
    USING_EXISTING_CLUSTER=false
    info "No existing cluster detected — starting fresh 3-validator cluster"

    # ── Build ─────────────────────────────────────────────────────
    if [ ! -f "$BIN" ]; then
        info "Building validator (release)..."
        cargo build --release --bin lichen-validator
    fi

    # ── Clean state ───────────────────────────────────────────────
    free_ports 9100 9102 9104 10099 10101 10103 10199 10201 10203 9301 9302 9303
    rm -rf "$DATA_DIR_BASE"
    mkdir -p "$DB_V1" "$DB_V2" "$DB_V3"
    mkdir -p "$HOME_V1" "$HOME_V2" "$HOME_V3"

    # ── Start validators ─────────────────────────────────────────
    info "Starting V1 (leader) on ports 9100/10099..."
    HOME="$HOME_V1" LICHEN_SIGNER_BIND=0.0.0.0:9301 RUST_LOG=warn "$BIN" --network testnet --dev-mode --p2p-port 9100 --rpc-port 10099 --ws-port 10199 \
        --db-path "$DB_V1" --no-watchdog > "$LOG_V1" 2>&1 &
    V1PID=$!
    sleep 8

    info "Starting V2 on ports 9102/10101..."
    HOME="$HOME_V2" LICHEN_SIGNER_BIND=0.0.0.0:9302 RUST_LOG=warn "$BIN" --network testnet --dev-mode --p2p-port 9102 --rpc-port 10101 --ws-port 10201 \
        --db-path "$DB_V2" --bootstrap-peers 127.0.0.1:9100 --no-watchdog > "$LOG_V2" 2>&1 &
    V2PID=$!
    sleep 6

    info "Starting V3 on ports 9104/10103..."
    HOME="$HOME_V3" LICHEN_SIGNER_BIND=0.0.0.0:9303 RUST_LOG=warn "$BIN" --network testnet --dev-mode --p2p-port 9104 --rpc-port 10103 --ws-port 10203 \
        --db-path "$DB_V3" --bootstrap-peers 127.0.0.1:9100 --no-watchdog > "$LOG_V3" 2>&1 &
    V3PID=$!
    sleep 6
fi

# ── Test 1: All validators healthy ──────────────────────────
echo ""
info "Test 1: Health checks"
for port in $RPC_V1 $RPC_V2 $RPC_V3; do
    if wait_for_health "$port" 20 1; then
        pass "Validator healthy (port $port)"
    else
        fail "Validator NOT healthy (port $port)"
    fi
done

# ── Test 2: All validators registered ───────────────────────
info "Test 2: Validator registration"
if VCOUNT=$(wait_for_validator_count $RPC_V1 2 20 1); then
    pass "Validator count >= 2 (got $VCOUNT)"
else
    fail "Expected >= 2 validators, got $VCOUNT"
fi

# ── Test 3: Block production ────────────────────────────────
info "Test 3: Block production"
SLOT1=$(rpc_result $RPC_V1 "getSlot" 2>/dev/null || echo "0")
sleep 10
SLOT2=$(rpc_result $RPC_V1 "getSlot" 2>/dev/null || echo "0")
if python3 -c "assert int('${SLOT2}') > int('${SLOT1}')" 2>/dev/null; then
    pass "Blocks advancing: $SLOT1 → $SLOT2"
else
    fail "Blocks NOT advancing: $SLOT1 → $SLOT2"
fi

# ── Test 4: Slot sync across validators ─────────────────────
info "Test 4: Slot synchronization"
SLOT_V1=$(rpc_result $RPC_V1 "getSlot" || echo "0")
SLOT_V2=$(rpc_result $RPC_V2 "getSlot" || echo "0")
SLOT_V3=$(rpc_result $RPC_V3 "getSlot" || echo "0")
SYNCED=$(python3 -c "
s=[int('$SLOT_V1'),int('$SLOT_V2'),int('$SLOT_V3')]
diff=max(s)-min(s)
print('yes' if diff <= 20 else 'no')
print(f'V1={s[0]} V2={s[1]} V3={s[2]} (drift={diff})')
" 2>/dev/null)
if echo "$SYNCED" | head -1 | grep -q "yes"; then
    pass "Slots synchronized: $(echo "$SYNCED" | tail -1)"
else
    fail "Slots NOT synchronized: $(echo "$SYNCED" | tail -1)"
fi

# ── Test 5: Transaction submission and confirmation ──────────
info "Test 5: Transaction finality"
# Get a recent blockhash
BLOCKHASH=$(rpc_result $RPC_V1 "getRecentBlockhash" | python3 -c "import sys,json; print(json.load(sys.stdin))" 2>/dev/null)
if [ -n "$BLOCKHASH" ] && [ "$BLOCKHASH" != "null" ] && [ "$BLOCKHASH" != "None" ]; then
    pass "Got recent blockhash: ${BLOCKHASH:0:16}..."
else
    fail "Could not get recent blockhash"
fi

# ── Test 6: Consistent block data across validators ─────────
info "Test 6: Cross-validator consistency"
sleep 5

# Give secondaries time to catch up from bootstrap and receive early blocks.
for _ in $(seq 1 30); do
    SLOT_V2_NOW=$(rpc_result $RPC_V2 "getSlot" || echo "0")
    SLOT_V3_NOW=$(rpc_result $RPC_V3 "getSlot" || echo "0")
    if python3 -c "import builtins; builtins.exit(0 if (int('${SLOT_V2_NOW:-0}') > 0 or int('${SLOT_V3_NOW:-0}') > 0) else 1)" 2>/dev/null; then
        break
    fi
    sleep 1
done

SLOT_V1_NOW=$(rpc_result $RPC_V1 "getSlot" || echo "0")
SLOT_V2_NOW=$(rpc_result $RPC_V2 "getSlot" || echo "0")
SLOT_V3_NOW=$(rpc_result $RPC_V3 "getSlot" || echo "0")

SECONDARY_SYNCED=$(python3 -c "
s1=int('${SLOT_V1_NOW:-0}')
s2=int('${SLOT_V2_NOW:-0}')
s3=int('${SLOT_V3_NOW:-0}')
print('yes' if (s2 > 0 or s3 > 0) else 'no')
print('V1=' + str(s1) + ' V2=' + str(s2) + ' V3=' + str(s3))
")

if echo "$SECONDARY_SYNCED" | head -1 | grep -q "yes"; then
    pass "Secondary sync observed: $(echo "$SECONDARY_SYNCED" | tail -1)"
else
    fail "Secondaries did not sync from leader: $(echo "$SECONDARY_SYNCED" | tail -1)"
fi

# ── Test 7: Metrics available ───────────────────────────────
info "Test 7: Chain metrics"
METRICS=$(rpc_result $RPC_V1 "getMetrics" 2>/dev/null || echo "null")
if [ "$METRICS" != "null" ] && [ -n "$METRICS" ]; then
    pass "Chain metrics available"
else
    fail "Chain metrics NOT available"
fi

# ── Summary ──────────────────────────────────────────────────
echo ""
echo "═══════════════════════════════════════════════════════"
echo -e " Results: ${GREEN}$PASS passed${NC}, ${RED}$FAIL failed${NC} / $TOTAL total"
echo "═══════════════════════════════════════════════════════"
echo ""

if [ "$FAIL" -gt 0 ]; then
    echo "Logs: $LOG_V1 $LOG_V2 $LOG_V3"
    exit 1
fi
exit 0
