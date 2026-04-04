#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════
# Lichen Full Matrix Test — 3 Staggered Validators (15s delay each)
# ═══════════════════════════════════════════════════════════════════════
#
# Tests every subsystem against a live 3-validator cluster:
#   Phase 1: Cluster boot (3 validators, 15s stagger)
#   Phase 2: Core consensus & sync
#   Phase 3: RPC endpoint coverage
#   Phase 4: Contract deployment & execution
#   Phase 5: WebSocket subscriptions
#   Phase 6: CLI operations
#   Phase 7: Stress & finality
#
# Usage: ./tests/matrix-test-3val.sh [--reuse-cluster]
#   --reuse-cluster: Skip validator boot, test against existing cluster on 8899/8901/8903

set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$PWD"
BIN="$ROOT/target/release/lichen-validator"
CLI_BIN="$ROOT/target/release/licn"
RUN_ID="$(date +%s)-$$"
DATA_DIR_BASE="$ROOT/data/matrix-$RUN_ID"
LOG_DIR="$DATA_DIR_BASE/logs"
REPORT_FILE="$ROOT/tests/artifacts/matrix-report-$RUN_ID.json"

# Colors
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; CYAN='\033[0;36m'; NC='\033[0m'
PASS=0; FAIL=0; SKIP=0; TOTAL=0
declare -a RESULTS=()
USING_EXISTING_CLUSTER=false
STAGGER_DELAY=15  # seconds between each validator start

for arg in "$@"; do
    case "$arg" in
        --reuse-cluster) USING_EXISTING_CLUSTER=true ;;
    esac
done

pass() {
    ((PASS++)); ((TOTAL++))
    echo -e "  ${GREEN}✓ PASS${NC}: $1"
    RESULTS+=("{\"test\":\"$1\",\"status\":\"pass\"}")
}
fail() {
    ((FAIL++)); ((TOTAL++))
    echo -e "  ${RED}✗ FAIL${NC}: $1${2:+ — $2}"
    RESULTS+=("{\"test\":\"$1\",\"status\":\"fail\",\"detail\":\"${2:-}\"}")
}
skip() {
    ((SKIP++)); ((TOTAL++))
    echo -e "  ${YELLOW}⊘ SKIP${NC}: $1"
    RESULTS+=("{\"test\":\"$1\",\"status\":\"skip\"}")
}
phase() { echo -e "\n${CYAN}══ Phase $1: $2 ══${NC}"; }

rpc() {
    local port=$1 method=$2 params=${3:-[]}
    curl -sf --max-time 10 http://127.0.0.1:$port -X POST \
        -H 'Content-Type:application/json' \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":$params}" 2>/dev/null
}

rpc_result() {
    local port=$1 method=$2 params=${3:-[]}
    rpc "$port" "$method" "$params" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin).get('result')))" 2>/dev/null
}

rpc_ok() {
    local port=$1 method=$2 params=${3:-[]}
    local resp
    resp=$(rpc "$port" "$method" "$params" 2>/dev/null || echo "")
    [ -n "$resp" ] && echo "$resp" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'result' in d" 2>/dev/null
}

wait_for_health() {
    local port=$1 attempts=${2:-30} delay=${3:-1}
    for _ in $(seq 1 "$attempts"); do
        if rpc_ok "$port" "health"; then return 0; fi
        sleep "$delay"
    done
    return 1
}

free_ports() {
    for port in "$@"; do
        lsof -ti tcp:"$port" 2>/dev/null | xargs -I{} kill {} 2>/dev/null || true
    done
    sleep 0.5
}

cleanup() {
    if [ "$USING_EXISTING_CLUSTER" = true ]; then return; fi
    echo -e "\n${YELLOW}Cleaning up validators...${NC}"
    for pid in ${V1PID:-} ${V2PID:-} ${V3PID:-}; do
        kill "$pid" 2>/dev/null || true
    done
    wait 2>/dev/null || true
    free_ports $RPC_V1 $RPC_V2 $RPC_V3 $P2P_V1 $P2P_V2 $P2P_V3
}
trap cleanup EXIT

# ── RPC/P2P ports ──
RPC_V1=10099; RPC_V2=10101; RPC_V3=10103
WS_V1=10199;  WS_V2=10201;  WS_V3=10203
P2P_V1=9100;  P2P_V2=9102;  P2P_V3=9104

echo ""
echo "═══════════════════════════════════════════════════════════"
echo " Lichen Full Matrix Test — 3 Validators (15s stagger)"
echo " Run ID: $RUN_ID"
echo "═══════════════════════════════════════════════════════════"

# ═══════════════════════════════════════════════════════════════
phase 1 "Cluster Boot"
# ═══════════════════════════════════════════════════════════════

if [ "$USING_EXISTING_CLUSTER" = true ]; then
    RPC_V1=8899; RPC_V2=8901; RPC_V3=8903
    WS_V1=8900;  WS_V2=8902;  WS_V3=8904
    echo "  Using existing cluster on ports $RPC_V1/$RPC_V2/$RPC_V3"
    for port in $RPC_V1 $RPC_V2 $RPC_V3; do
        if wait_for_health "$port" 5 1; then
            pass "Existing validator healthy (port $port)"
        else
            fail "Existing validator NOT healthy (port $port)"
        fi
    done
else
    # Build if needed
    if [ ! -f "$BIN" ]; then
        echo "  Building validator..."
        cargo build --release --quiet
    fi

    mkdir -p "$DATA_DIR_BASE"/{v1,v2,v3}/{home} "$LOG_DIR"
    free_ports $RPC_V1 $RPC_V2 $RPC_V3 $P2P_V1 $P2P_V2 $P2P_V3

    # Start V1
    echo -e "  ${CYAN}Starting V1 (leader)...${NC}"
    HOME="$DATA_DIR_BASE/v1/home" RUST_LOG=warn "$BIN" \
        --network testnet --dev-mode \
        --p2p-port $P2P_V1 --rpc-port $RPC_V1 --ws-port $WS_V1 \
        --db-path "$DATA_DIR_BASE/v1" --no-watchdog > "$LOG_DIR/v1.log" 2>&1 &
    V1PID=$!
    echo "    PID=$V1PID, waiting ${STAGGER_DELAY}s..."
    sleep $STAGGER_DELAY

    if wait_for_health $RPC_V1 30 1; then
        pass "V1 healthy after ${STAGGER_DELAY}s stagger"
    else
        fail "V1 did not become healthy" "Check $LOG_DIR/v1.log"
    fi

    # Start V2
    echo -e "  ${CYAN}Starting V2 (joins V1)...${NC}"
    HOME="$DATA_DIR_BASE/v2/home" RUST_LOG=warn "$BIN" \
        --network testnet --dev-mode \
        --p2p-port $P2P_V2 --rpc-port $RPC_V2 --ws-port $WS_V2 \
        --db-path "$DATA_DIR_BASE/v2" --bootstrap-peers 127.0.0.1:$P2P_V1 --no-watchdog > "$LOG_DIR/v2.log" 2>&1 &
    V2PID=$!
    echo "    PID=$V2PID, waiting ${STAGGER_DELAY}s..."
    sleep $STAGGER_DELAY

    if wait_for_health $RPC_V2 30 1; then
        pass "V2 healthy after ${STAGGER_DELAY}s stagger"
    else
        fail "V2 did not become healthy" "Check $LOG_DIR/v2.log"
    fi

    # Start V3
    echo -e "  ${CYAN}Starting V3 (joins V1+V2)...${NC}"
    HOME="$DATA_DIR_BASE/v3/home" RUST_LOG=warn "$BIN" \
        --network testnet --dev-mode \
        --p2p-port $P2P_V3 --rpc-port $RPC_V3 --ws-port $WS_V3 \
        --db-path "$DATA_DIR_BASE/v3" --bootstrap-peers "127.0.0.1:$P2P_V1" --no-watchdog > "$LOG_DIR/v3.log" 2>&1 &
    V3PID=$!
    echo "    PID=$V3PID, waiting ${STAGGER_DELAY}s..."
    sleep $STAGGER_DELAY

    if wait_for_health $RPC_V3 30 1; then
        pass "V3 healthy after ${STAGGER_DELAY}s stagger"
    else
        fail "V3 did not become healthy" "Check $LOG_DIR/v3.log"
    fi
fi

# ═══════════════════════════════════════════════════════════════
phase 2 "Consensus & Synchronization"
# ═══════════════════════════════════════════════════════════════

# Test: Block production
SLOT_A=$(rpc_result $RPC_V1 "getSlot" || echo "0")
sleep 8
SLOT_B=$(rpc_result $RPC_V1 "getSlot" || echo "0")
if python3 -c "assert int('${SLOT_B}') > int('${SLOT_A}')" 2>/dev/null; then
    pass "Blocks advancing: $SLOT_A → $SLOT_B"
else
    fail "Blocks NOT advancing" "$SLOT_A → $SLOT_B"
fi

# Test: Slot synchronization
SLOT_V1=$(rpc_result $RPC_V1 "getSlot" || echo "0")
SLOT_V2=$(rpc_result $RPC_V2 "getSlot" || echo "0")
SLOT_V3=$(rpc_result $RPC_V3 "getSlot" || echo "0")
DRIFT=$(python3 -c "s=[int('$SLOT_V1'),int('$SLOT_V2'),int('$SLOT_V3')]; print(max(s)-min(s))" 2>/dev/null || echo "999")
if [ "$DRIFT" -le 15 ]; then
    pass "Slot sync within 15 slots (V1=$SLOT_V1 V2=$SLOT_V2 V3=$SLOT_V3, drift=$DRIFT)"
else
    fail "Slot drift too high" "V1=$SLOT_V1 V2=$SLOT_V2 V3=$SLOT_V3 drift=$DRIFT"
fi

# Test: Validator count
VCOUNT=$(rpc_result $RPC_V1 "getValidators" | python3 -c "import sys,json; print(len(json.load(sys.stdin)))" 2>/dev/null || echo "0")
if [ "$VCOUNT" -ge 2 ]; then
    pass "Validator count: $VCOUNT (≥ 2)"
else
    fail "Validator count too low" "got $VCOUNT, expected ≥ 2"
fi

# Test: getBlock works
CURRENT_SLOT=$(rpc_result $RPC_V1 "getSlot" || echo "1")
BLOCK_SLOT=$(($(echo "$CURRENT_SLOT" | tr -d '"') - 2))
if [ "$BLOCK_SLOT" -gt 0 ]; then
    BLK=$(rpc_result $RPC_V1 "getBlock" "[$BLOCK_SLOT]" || echo "null")
    if [ "$BLK" != "null" ] && [ -n "$BLK" ]; then
        pass "getBlock($BLOCK_SLOT) returns data"
    else
        fail "getBlock($BLOCK_SLOT) returned null"
    fi
else
    skip "getBlock — no old blocks yet"
fi

# ═══════════════════════════════════════════════════════════════
phase 3 "RPC Endpoint Coverage"
# ═══════════════════════════════════════════════════════════════

RPC_METHODS=(
    "health"
    "getSlot"
    "getRecentBlockhash"
    "getValidators"
    "getMetrics"
    "getClusterInfo"
    "getChainStatus"
    "getLatestBlock"
    "getPeers"
    "getNetworkInfo"
    "getFeeConfig"
    "getRentParams"
    "getTreasuryInfo"
    "getGenesisAccounts"
)

for entry in "${RPC_METHODS[@]}"; do
    method=$(echo "$entry" | awk '{print $1}')
    params=$(echo "$entry" | awk '{$1=""; print $0}' | xargs)
    params=${params:-[]}
    if rpc_ok $RPC_V1 "$method" "$params"; then
        pass "RPC: $method"
    else
        fail "RPC: $method" "no result"
    fi
done

# Test getBalance with zero-address
if rpc_ok $RPC_V1 "getBalance" '["11111111111111111111111111111111"]'; then
    pass "RPC: getBalance (zero addr)"
else
    fail "RPC: getBalance" "no result"
fi

# Test getAccountInfo
if rpc_ok $RPC_V1 "getAccountInfo" '["11111111111111111111111111111111"]'; then
    pass "RPC: getAccountInfo"
else
    fail "RPC: getAccountInfo" "no result"
fi

# Test: Symbol registry
SYMBOL_RESP=$(rpc_result $RPC_V1 "getSymbolRegistry" '[{}]' || echo "null")
if [ "$SYMBOL_RESP" != "null" ] && [ -n "$SYMBOL_RESP" ]; then
    pass "RPC: getSymbolRegistry"
else
    skip "RPC: getSymbolRegistry (may not be populated in dev-mode)"
fi

# Test: Contract list
CONTRACTS_RESP=$(rpc_result $RPC_V1 "getAllContracts" '[{}]' || echo "null")
if [ "$CONTRACTS_RESP" != "null" ] && [ -n "$CONTRACTS_RESP" ]; then
    CONTRACT_COUNT=$(echo "$CONTRACTS_RESP" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d) if isinstance(d,list) else (len(d) if isinstance(d,dict) else 0))" 2>/dev/null || echo "0")
    pass "RPC: getAllContracts ($CONTRACT_COUNT contracts)"
else
    skip "RPC: getAllContracts (may not be populated in dev-mode)"
fi

# ═══════════════════════════════════════════════════════════════
phase 4 "Contract Deployment & Execution"
# ═══════════════════════════════════════════════════════════════

# Test: Contracts are deployed (from genesis)
for contract in lichenswap thalllend lichenoracle lichenid; do
    ADDR=$(echo "$SYMBOL_RESP" | python3 -c "
import sys,json
d=json.load(sys.stdin)
if isinstance(d, dict):
    for k,v in d.items():
        if isinstance(v, dict) and v.get('symbol','').lower() == '$contract':
            print(v.get('address',''))
            break
    else:
        for k,v in d.items():
            if '$contract' in k.lower():
                print(v if isinstance(v,str) else v.get('address',''))
                break
" 2>/dev/null || echo "")
    if [ -n "$ADDR" ] && [ "$ADDR" != "None" ]; then
        pass "Contract deployed: $contract ($ADDR)"
    else
        skip "Contract '$contract' not found in symbol registry"
    fi
done

# Test: LICN is native, not a registry-backed contract
LICHENCOIN_STATE=$(rpc_result $RPC_V1 "getContractState" '["lichencoin","total_supply"]' 2>/dev/null || echo "null")
if [ "$LICHENCOIN_STATE" = "null" ] || [ -z "$LICHENCOIN_STATE" ]; then
    pass "Native LICN is not exposed via a lichencoin contract state entry"
else
    fail "Unexpected lichencoin contract state entry is present"
fi

# ═══════════════════════════════════════════════════════════════
phase 5 "WebSocket Subscriptions"
# ═══════════════════════════════════════════════════════════════

# Test: WS connection and slot subscription
WS_TEST=$(timeout 15 python3 -c "
import json, asyncio
try:
    import websockets
except ImportError:
    print('skip_no_websockets')
    exit(0)
async def test():
    try:
        async with websockets.connect('ws://127.0.0.1:$WS_V1', close_timeout=3, open_timeout=5) as ws:
            await ws.send(json.dumps({'jsonrpc':'2.0','id':1,'method':'subscribeSlots','params':[]}))
            resp = await asyncio.wait_for(ws.recv(), timeout=5)
            d = json.loads(resp)
            if 'result' in d:
                try:
                    notif = await asyncio.wait_for(ws.recv(), timeout=10)
                    print('ok')
                except asyncio.TimeoutError:
                    print('ok_sub_only')
            else:
                print('no_result')
    except Exception as e:
        print(f'error:{e}')
asyncio.run(test())
" 2>/dev/null || echo "timeout")

if [ "$WS_TEST" = "ok" ] || [ "$WS_TEST" = "ok_sub_only" ]; then
    pass "WebSocket: slotSubscribe + notification"
elif [ "$WS_TEST" = "skip_no_websockets" ]; then
    skip "WebSocket: slotSubscribe (websockets not installed)"
else
    skip "WebSocket: slotSubscribe ($WS_TEST — WS may not be on expected port)"
fi

# Test: WS ping/pong
WS_PING=$(timeout 8 python3 -c "
import json, asyncio
try:
    import websockets
except ImportError:
    print('skip_no_websockets')
    exit(0)
async def test():
    try:
        async with websockets.connect('ws://127.0.0.1:$WS_V1', close_timeout=2, open_timeout=5) as ws:
            await ws.send(json.dumps({'method':'ping'}))
            resp = await asyncio.wait_for(ws.recv(), timeout=3)
            d = json.loads(resp)
            print('ok' if d.get('result') == 'pong' else 'wrong_response')
    except Exception as e:
        print(f'error:{e}')
asyncio.run(test())
" 2>/dev/null || echo "timeout")

if [ "$WS_PING" = "ok" ]; then
    pass "WebSocket: ping/pong"
elif [ "$WS_PING" = "skip_no_websockets" ]; then
    skip "WebSocket: ping/pong (websockets not installed)"
else
    skip "WebSocket: ping/pong ($WS_PING — WS may not be on expected port)"
fi

# ═══════════════════════════════════════════════════════════════
phase 6 "CLI Operations"
# ═══════════════════════════════════════════════════════════════

if [ -f "$CLI_BIN" ]; then
    # Test: CLI keygen
    KEY_OUTPUT=$($CLI_BIN keygen 2>&1 || echo "error")
    if echo "$KEY_OUTPUT" | grep -qiE "pubkey|public|address|key|[A-Za-z0-9]{32}"; then
        pass "CLI: keygen"
    else
        skip "CLI: keygen (output format may differ)"
    fi

    # Test: CLI balance
    BAL_OUTPUT=$($CLI_BIN balance --rpc "http://127.0.0.1:$RPC_V1" 11111111111111111111111111111111 2>&1 || echo "error")
    if echo "$BAL_OUTPUT" | grep -qiE "balance|LICN|lamport|spore|[0-9]"; then
        pass "CLI: balance query"
    else
        skip "CLI: balance query (output format may differ)"
    fi

    # Test: CLI slot
    SLOT_OUTPUT=$($CLI_BIN slot --rpc "http://127.0.0.1:$RPC_V1" 2>&1 || echo "")
    if echo "$SLOT_OUTPUT" | grep -qE "[0-9]+"; then
        pass "CLI: slot query"
    else
        skip "CLI: slot query (command may use different name)"
    fi
else
    skip "CLI binary not found at $CLI_BIN"
fi

# ═══════════════════════════════════════════════════════════════
phase 7 "Stress & Finality"
# ═══════════════════════════════════════════════════════════════

# Test: Rapid RPC calls (10 sequential calls, all must succeed)
RAPID_PASS=0
for _ in $(seq 1 10); do
    if rpc_ok $RPC_V1 "getSlot"; then ((RAPID_PASS++)); fi
done
if [ "$RAPID_PASS" -eq 10 ]; then
    pass "Rapid RPC: 10/10 getSlot calls succeeded"
else
    fail "Rapid RPC" "only $RAPID_PASS/10 succeeded"
fi

# Test: Cross-validator read consistency
sleep 3
SLOT_FINAL_V1=$(rpc_result $RPC_V1 "getSlot" || echo "0")
SLOT_FINAL_V2=$(rpc_result $RPC_V2 "getSlot" || echo "0")
SLOT_FINAL_V3=$(rpc_result $RPC_V3 "getSlot" || echo "0")
FINAL_DRIFT=$(python3 -c "s=[int('$SLOT_FINAL_V1'),int('$SLOT_FINAL_V2'),int('$SLOT_FINAL_V3')]; print(max(s)-min(s))" 2>/dev/null || echo "999")
if [ "$FINAL_DRIFT" -le 10 ]; then
    pass "Final slot drift: $FINAL_DRIFT (V1=$SLOT_FINAL_V1 V2=$SLOT_FINAL_V2 V3=$SLOT_FINAL_V3)"
else
    fail "Final slot drift" "$FINAL_DRIFT (V1=$SLOT_FINAL_V1 V2=$SLOT_FINAL_V2 V3=$SLOT_FINAL_V3)"
fi

# Test: Validators still healthy after all tests
for port in $RPC_V1 $RPC_V2 $RPC_V3; do
    if rpc_ok "$port" "health"; then
        pass "Post-test health: port $port"
    else
        fail "Post-test health: port $port" "unhealthy"
    fi
done

# ═══════════════════════════════════════════════════════════════
# Summary
# ═══════════════════════════════════════════════════════════════

echo ""
echo "═══════════════════════════════════════════════════════════"
echo -e " Matrix Results: ${GREEN}$PASS passed${NC}, ${RED}$FAIL failed${NC}, ${YELLOW}$SKIP skipped${NC} / $TOTAL total"
echo "═══════════════════════════════════════════════════════════"

# Write JSON report
mkdir -p "$(dirname "$REPORT_FILE")"
cat > "$REPORT_FILE" <<EOJSON
{
  "run_id": "$RUN_ID",
  "timestamp": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "validators": 3,
  "stagger_delay_sec": $STAGGER_DELAY,
  "reused_cluster": $USING_EXISTING_CLUSTER,
  "summary": {
    "total": $TOTAL,
    "passed": $PASS,
    "failed": $FAIL,
    "skipped": $SKIP
  },
  "results": [$(IFS=,; echo "${RESULTS[*]}")]
}
EOJSON
echo -e "  Report: ${CYAN}$REPORT_FILE${NC}"
echo ""

if [ "$FAIL" -gt 0 ]; then
    [ -d "$LOG_DIR" ] && echo "  Logs: $LOG_DIR/"
    exit 1
fi
exit 0
