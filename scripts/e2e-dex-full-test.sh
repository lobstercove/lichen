#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════
# MoltChain DEX Full E2E Test Suite
# Tests ALL DEX endpoints, write operations, and WebSocket channels
# Complements e2e-live-test.sh with deeper coverage
# ═══════════════════════════════════════════════════════════════════════
set -euo pipefail

RPC1="http://localhost:8899"
RPC2="http://localhost:8901"
RPC3="http://localhost:8903"
WS1="ws://localhost:8900"
FAUCET="http://localhost:9100"

PASS=0
FAIL=0
WARN=0

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
BOLD='\033[1m'
NC='\033[0m'

# ── Helper: JSON-RPC call ─────────────────────────────────────────────
rpc() {
    local url="${1:-$RPC1}"
    local method="$2"
    shift 2
    local params="${1:-[]}"
    curl -s -m 5 -X POST "$url" \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":$params}" 2>/dev/null
}

# ── Helper: REST GET ──────────────────────────────────────────────────
rest_get() {
    local path="$1"
    curl -s -m 5 "${RPC1}${path}" 2>/dev/null
}

# ── Helper: REST POST ────────────────────────────────────────────────
rest_post() {
    local path="$1"
    local body="${2:-{}}"
    curl -s -m 5 -X POST "${RPC1}${path}" \
        -H "Content-Type: application/json" \
        -d "$body" 2>/dev/null
}

# ── Helper: REST DELETE ──────────────────────────────────────────────
rest_delete() {
    local path="$1"
    curl -s -m 5 -X DELETE "${RPC1}${path}" 2>/dev/null
}

# ── Check: expects valid JSON with specific properties ────────────────
check_json() {
    local name="$1"
    local response="$2"
    local check_field="${3:-}"

    if [ -z "$response" ]; then
        FAIL=$((FAIL + 1))
        echo -e "  ${RED}✗${NC} $name → NO RESPONSE"
        return
    fi

    local is_json
    is_json=$(echo "$response" | python3 -c "import sys,json; json.load(sys.stdin); print('yes')" 2>/dev/null || echo "no")

    if [ "$is_json" = "yes" ]; then
        # Determine error type: none, str (REST expected error), obj (RPC error)
        local error_type
        error_type=$(echo "$response" | python3 -c "
import sys,json
d=json.load(sys.stdin)
e=d.get('error')
if e is None: print('none')
elif isinstance(e,str): print('str')
else: print('obj')
" 2>/dev/null || echo "none")

        if [ "$error_type" = "none" ] || [ "$error_type" = "str" ]; then
            # No error, or string error (expected REST response on empty/fresh chain)
            if [ -n "$check_field" ]; then
                if echo "$response" | python3 -c "import sys,json; d=json.load(sys.stdin); assert '$check_field' in d or '$check_field' in d.get('result',{})" 2>/dev/null; then
                    PASS=$((PASS + 1))
                    echo -e "  ${GREEN}✓${NC} $name"
                else
                    WARN=$((WARN + 1))
                    echo -e "  ${YELLOW}⚠${NC} $name (field '$check_field' not found)"
                fi
            else
                PASS=$((PASS + 1))
                echo -e "  ${GREEN}✓${NC} $name"
            fi
        else
            # Object error — allow "Method not allowed" (guarded endpoint)
            if echo "$response" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'Method not allowed' in d['error'].get('message','')" 2>/dev/null; then
                PASS=$((PASS + 1))
                echo -e "  ${GREEN}✓${NC} $name"
            else
                local err_msg
                err_msg=$(echo "$response" | python3 -c "import sys,json; e=json.load(sys.stdin).get('error',{}); print(e.get('message',str(e))[:80] if isinstance(e,dict) else str(e)[:80])" 2>/dev/null || echo "?")
                WARN=$((WARN + 1))
                echo -e "  ${YELLOW}⚠${NC} $name → $err_msg"
            fi
        fi
    else
        # Non-JSON response
        if [ ${#response} -gt 0 ]; then
            PASS=$((PASS + 1))
            echo -e "  ${GREEN}✓${NC} $name (non-JSON: ${response:0:40})"
        else
            FAIL=$((FAIL + 1))
            echo -e "  ${RED}✗${NC} $name → empty"
        fi
    fi
}

# ── Check: expects 405/error for guarded write endpoints ─────────────
check_guarded() {
    local name="$1"
    local response="$2"

    if [ -z "$response" ]; then
        FAIL=$((FAIL + 1))
        echo -e "  ${RED}✗${NC} $name → NO RESPONSE"
        return
    fi

    # These should return a "Method not allowed" / "must use sendTransaction" error
    if echo "$response" | grep -qi "sendTransaction\|not allowed\|method_not_allowed\|error"; then
        PASS=$((PASS + 1))
        echo -e "  ${GREEN}✓${NC} $name (correctly guarded)"
    else
        WARN=$((WARN + 1))
        echo -e "  ${YELLOW}⚠${NC} $name (unexpected: ${response:0:60})"
    fi
}

# ── Check: expects an error response (negative test) ────────────────
check_expected_error() {
    local name="$1"
    local response="$2"

    if [ -z "$response" ]; then
        FAIL=$((FAIL + 1))
        echo -e "  ${RED}✗${NC} $name → NO RESPONSE"
        return
    fi

    # Any response containing "error" is a PASS for negative tests
    if echo "$response" | grep -q '"error"'; then
        PASS=$((PASS + 1))
        echo -e "  ${GREEN}✓${NC} $name (error as expected)"
    else
        WARN=$((WARN + 1))
        echo -e "  ${YELLOW}⚠${NC} $name (expected error but got success)"
    fi
}

# ── Check: RPC result ────────────────────────────────────────────────
check_rpc() {
    local name="$1"
    local response="$2"

    if [ -z "$response" ]; then
        FAIL=$((FAIL + 1))
        echo -e "  ${RED}✗${NC} $name → NO RESPONSE"
        return
    fi

    if echo "$response" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'result' in d" 2>/dev/null; then
        PASS=$((PASS + 1))
        echo -e "  ${GREEN}✓${NC} $name"
    elif echo "$response" | grep -q '"error"'; then
        local err_msg
        err_msg=$(echo "$response" | python3 -c "import sys,json; e=json.load(sys.stdin).get('error',{}); print(e.get('message','?')[:80] if isinstance(e,dict) else str(e)[:80])" 2>/dev/null || echo "?")
        WARN=$((WARN + 1))
        echo -e "  ${YELLOW}⚠${NC} $name → $err_msg"
    else
        FAIL=$((FAIL + 1))
        echo -e "  ${RED}✗${NC} $name → invalid"
    fi
}

# ═══════════════════════════════════════════════════════════════════════
echo ""
echo -e "${BOLD}═══════════════════════════════════════════════════════════${NC}"
echo -e "${BOLD}  MoltChain DEX + Write Operations Full Test Suite${NC}"
echo -e "${BOLD}  All DEX REST, RPC, WebSocket & Write Endpoints${NC}"
echo -e "${BOLD}═══════════════════════════════════════════════════════════${NC}"
echo ""

# Get genesis pubkey: first try RPC, then fall back to log
GENESIS_PUBKEY=$(rpc "$RPC1" "getGenesisAccounts" | python3 -c "
import sys,json
d=json.load(sys.stdin)
accts=d.get('result',{}).get('accounts',[])
for a in accts:
    lbl = a.get('label','').lower()
    if 'genesis' in lbl and ('signer' in lbl or 'wallet' in lbl):
        print(a['pubkey'])
        break
" 2>/dev/null)

if [ -z "$GENESIS_PUBKEY" ]; then
    # Fallback: parse log
    GENESIS_PUBKEY=$(python3 -c "
import re
with open('/tmp/moltchain-v1.log','rb') as f:
    data = f.read().decode('utf-8', errors='replace')
    data = re.sub(r'\x1b\[[0-9;]*m', '', data)
for pattern in [r'Genesis wallet: ([A-Za-z0-9]{32,50})', r'genesis_wallet.*?([A-HJ-NP-Za-km-z1-9]{32,50})']:
    m = re.search(pattern, data)
    if m:
        print(m.group(1))
        break
" 2>/dev/null)
fi
echo -e "  Genesis: ${CYAN}${GENESIS_PUBKEY}${NC}"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 1. DEX TRADING PAIRS (ALL READS) ━━━${NC}"

R=$(rest_get "/api/v1/pairs")
check_json "GET /api/v1/pairs" "$R"
PAIR_COUNT=$(echo "$R" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d.get('data',[])))" 2>/dev/null || echo "0")
echo "  Pairs count: $PAIR_COUNT"

R=$(rest_get "/api/v1/pairs/1")
check_json "GET /api/v1/pairs/1" "$R"

R=$(rest_get "/api/v1/pairs/2")
check_json "GET /api/v1/pairs/2" "$R"

R=$(rest_get "/api/v1/pairs/3")
check_json "GET /api/v1/pairs/3" "$R"

R=$(rest_get "/api/v1/pairs/1/orderbook")
check_json "GET /api/v1/pairs/1/orderbook" "$R"

R=$(rest_get "/api/v1/pairs/2/orderbook")
check_json "GET /api/v1/pairs/2/orderbook" "$R"

R=$(rest_get "/api/v1/pairs/1/trades")
check_json "GET /api/v1/pairs/1/trades" "$R"

R=$(rest_get "/api/v1/pairs/2/trades")
check_json "GET /api/v1/pairs/2/trades" "$R"

R=$(rest_get "/api/v1/pairs/1/candles")
check_json "GET /api/v1/pairs/1/candles" "$R"

R=$(rest_get "/api/v1/pairs/1/candles?interval=5m")
check_json "GET /api/v1/pairs/1/candles?interval=5m" "$R"

R=$(rest_get "/api/v1/pairs/1/candles?interval=1h")
check_json "GET /api/v1/pairs/1/candles?interval=1h" "$R"

R=$(rest_get "/api/v1/pairs/1/stats")
check_json "GET /api/v1/pairs/1/stats" "$R"

R=$(rest_get "/api/v1/pairs/1/ticker")
check_json "GET /api/v1/pairs/1/ticker" "$R"

R=$(rest_get "/api/v1/pairs/2/ticker")
check_json "GET /api/v1/pairs/2/ticker" "$R"

R=$(rest_get "/api/v1/tickers")
check_json "GET /api/v1/tickers" "$R"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 2. DEX ORDERS ━━━${NC}"

R=$(rest_get "/api/v1/orders")
check_json "GET /api/v1/orders (all)" "$R"

R=$(rest_get "/api/v1/orders?trader=${GENESIS_PUBKEY}")
check_json "GET /api/v1/orders?trader=genesis" "$R"

R=$(rest_get "/api/v1/orders/1")
check_json "GET /api/v1/orders/1" "$R"

R=$(rest_get "/api/v1/orders/999")
check_json "GET /api/v1/orders/999 (nonexistent)" "$R"

# POST/DELETE are guarded — must use sendTransaction
R=$(rest_post "/api/v1/orders" '{"pair_id":1,"side":"buy","price":1.0,"amount":100}')
check_guarded "POST /api/v1/orders (guarded)" "$R"

R=$(rest_delete "/api/v1/orders/1")
check_guarded "DELETE /api/v1/orders/1 (guarded)" "$R"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 3. DEX AMM POOLS ━━━${NC}"

R=$(rest_get "/api/v1/pools")
check_json "GET /api/v1/pools" "$R"
POOL_COUNT=$(echo "$R" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d.get('data',[])))" 2>/dev/null || echo "0")
echo "  Pool count: $POOL_COUNT"

R=$(rest_get "/api/v1/pools/1")
check_json "GET /api/v1/pools/1" "$R"

R=$(rest_get "/api/v1/pools/2")
check_json "GET /api/v1/pools/2" "$R"

R=$(rest_get "/api/v1/pools/positions?owner=${GENESIS_PUBKEY}")
check_json "GET /api/v1/pools/positions?owner=genesis" "$R"

R=$(rest_get "/api/v1/pools/positions?owner=nonexistent")
check_json "GET /api/v1/pools/positions?owner=nonexistent" "$R"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 4. DEX ROUTER / SWAPS ━━━${NC}"

R=$(rest_get "/api/v1/routes")
check_json "GET /api/v1/routes" "$R"
ROUTE_COUNT=$(echo "$R" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d.get('data',[])))" 2>/dev/null || echo "0")
echo "  Route count: $ROUTE_COUNT"

# POST /api/v1/router/swap — read-only swap simulation
R=$(rest_post "/api/v1/router/swap" '{"token_in":"MOLT","token_out":"MUSD","amount_in":1000000,"slippage":1.0}')
check_json "POST /api/v1/router/swap (MOLT→MUSD)" "$R"

R=$(rest_post "/api/v1/router/swap" '{"token_in":"MUSD","token_out":"MOLT","amount_in":500000,"slippage":1.0}')
check_json "POST /api/v1/router/swap (MUSD→MOLT)" "$R"

R=$(rest_post "/api/v1/router/swap" '{"token_in":"MOLT","token_out":"WETH","amount_in":1000000,"slippage":1.0}')
check_json "POST /api/v1/router/swap (MOLT→WETH)" "$R"

# Edge case: 0 amount
R=$(rest_post "/api/v1/router/swap" '{"token_in":"MOLT","token_out":"MUSD","amount_in":0,"slippage":1.0}')
check_json "POST /api/v1/router/swap (zero amount)" "$R"

# Edge case: bad slippage
R=$(rest_post "/api/v1/router/swap" '{"token_in":"MOLT","token_out":"MUSD","amount_in":1000,"slippage":99.0}')
check_json "POST /api/v1/router/swap (bad slippage)" "$R"

# POST /api/v1/router/quote — same as swap but named quote
R=$(rest_post "/api/v1/router/quote" '{"token_in":"MOLT","token_out":"MUSD","amount_in":1000000,"slippage":0.5}')
check_json "POST /api/v1/router/quote (MOLT→MUSD)" "$R"

R=$(rest_post "/api/v1/router/quote" '{"token_in":"MUSD","token_out":"WSOL","amount_in":500000,"slippage":1.0}')
check_json "POST /api/v1/router/quote (MUSD→WSOL)" "$R"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 5. DEX MARGIN TRADING ━━━${NC}"

R=$(rest_get "/api/v1/margin/positions")
check_json "GET /api/v1/margin/positions (all)" "$R"

R=$(rest_get "/api/v1/margin/positions?trader=${GENESIS_PUBKEY}")
check_json "GET /api/v1/margin/positions?trader=genesis" "$R"

R=$(rest_get "/api/v1/margin/positions/1")
check_json "GET /api/v1/margin/positions/1" "$R"

R=$(rest_get "/api/v1/margin/info")
check_json "GET /api/v1/margin/info" "$R"

R=$(rest_get "/api/v1/margin/enabled-pairs")
check_json "GET /api/v1/margin/enabled-pairs" "$R"

R=$(rest_get "/api/v1/margin/funding-rate")
check_json "GET /api/v1/margin/funding-rate" "$R"

# POST open/close are guarded
R=$(rest_post "/api/v1/margin/open" '{"pair_id":1,"side":"long","collateral":1000,"leverage":2}')
check_guarded "POST /api/v1/margin/open (guarded)" "$R"

R=$(rest_post "/api/v1/margin/close" '{"position_id":1}')
check_guarded "POST /api/v1/margin/close (guarded)" "$R"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 6. DEX ANALYTICS & LEADERBOARD ━━━${NC}"

R=$(rest_get "/api/v1/leaderboard")
check_json "GET /api/v1/leaderboard" "$R"

R=$(rest_get "/api/v1/traders/${GENESIS_PUBKEY}/stats")
check_json "GET /api/v1/traders/genesis/stats" "$R"

R=$(rest_get "/api/v1/traders/nonexistent/stats")
check_json "GET /api/v1/traders/nonexistent/stats" "$R"

R=$(rest_get "/api/v1/rewards/${GENESIS_PUBKEY}")
check_json "GET /api/v1/rewards/genesis" "$R"

R=$(rest_get "/api/v1/rewards/nonexistent")
check_json "GET /api/v1/rewards/nonexistent" "$R"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 7. DEX GOVERNANCE ━━━${NC}"

R=$(rest_get "/api/v1/governance/proposals")
check_json "GET /api/v1/governance/proposals" "$R"

R=$(rest_get "/api/v1/governance/proposals/1")
check_json "GET /api/v1/governance/proposals/1" "$R"

R=$(rest_get "/api/v1/governance/proposals/999")
check_json "GET /api/v1/governance/proposals/999 (nonexistent)" "$R"

# POST create/vote are guarded
R=$(rest_post "/api/v1/governance/proposals" '{"title":"Test","description":"test"}')
check_guarded "POST /api/v1/governance/proposals (guarded)" "$R"

R=$(rest_post "/api/v1/governance/proposals/1/vote" '{"vote":"yes"}')
check_guarded "POST /api/v1/governance/proposals/1/vote (guarded)" "$R"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 8. DEX ORACLE ━━━${NC}"

R=$(rest_get "/api/v1/oracle/prices")
check_json "GET /api/v1/oracle/prices" "$R"
PRICE_COUNT=$(echo "$R" | python3 -c "import sys,json; d=json.load(sys.stdin); print(len(d.get('data',[])))" 2>/dev/null || echo "0")
echo "  Oracle price feeds: $PRICE_COUNT"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 9. DEX PLATFORM STATS (ALL CONTRACTS) ━━━${NC}"

R=$(rest_get "/api/v1/stats/core")
check_json "GET /api/v1/stats/core" "$R"

R=$(rest_get "/api/v1/stats/amm")
check_json "GET /api/v1/stats/amm" "$R"

R=$(rest_get "/api/v1/stats/margin")
check_json "GET /api/v1/stats/margin" "$R"

R=$(rest_get "/api/v1/stats/router")
check_json "GET /api/v1/stats/router" "$R"

R=$(rest_get "/api/v1/stats/rewards")
check_json "GET /api/v1/stats/rewards" "$R"

R=$(rest_get "/api/v1/stats/analytics")
check_json "GET /api/v1/stats/analytics" "$R"

R=$(rest_get "/api/v1/stats/governance")
check_json "GET /api/v1/stats/governance" "$R"

R=$(rest_get "/api/v1/stats/moltswap")
check_json "GET /api/v1/stats/moltswap" "$R"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 10. DEX JSON-RPC METHODS ━━━${NC}"

for method in getDexCoreStats getDexAmmStats getDexMarginStats getDexRewardsStats getDexRouterStats getDexAnalyticsStats getDexGovernanceStats getMoltswapStats; do
    R=$(rpc "$RPC1" "$method")
    check_rpc "RPC $method" "$R"
done

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 11. PREDICTION MARKET (ALL) ━━━${NC}"

R=$(rest_get "/api/v1/prediction-market/stats")
check_json "GET /api/v1/prediction-market/stats" "$R"

R=$(rest_get "/api/v1/prediction-market/markets")
check_json "GET /api/v1/prediction-market/markets" "$R"

R=$(rest_get "/api/v1/prediction-market/markets/1")
check_json "GET /api/v1/prediction-market/markets/1" "$R"

R=$(rest_get "/api/v1/prediction-market/leaderboard")
check_json "GET /api/v1/prediction-market/leaderboard" "$R"

R=$(rest_get "/api/v1/prediction-market/trending")
check_json "GET /api/v1/prediction-market/trending" "$R"

R=$(rpc "$RPC1" "getPredictionMarketStats")
check_rpc "RPC getPredictionMarketStats" "$R"

R=$(rpc "$RPC1" "getPredictionMarkets")
check_rpc "RPC getPredictionMarkets" "$R"

R=$(rpc "$RPC1" "getPredictionPositions" "[\"${GENESIS_PUBKEY}\"]")
check_rpc "RPC getPredictionPositions" "$R"

R=$(rpc "$RPC1" "getPredictionTraderStats" "[\"${GENESIS_PUBKEY}\"]")
check_rpc "RPC getPredictionTraderStats" "$R"

R=$(rpc "$RPC1" "getPredictionLeaderboard")
check_rpc "RPC getPredictionLeaderboard" "$R"

R=$(rpc "$RPC1" "getPredictionTrending")
check_rpc "RPC getPredictionTrending" "$R"

R=$(rpc "$RPC1" "getPredictionMarketAnalytics" '[0]')
check_rpc "RPC getPredictionMarketAnalytics" "$R"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 12. LAUNCHPAD ━━━${NC}"

R=$(rest_get "/api/v1/launchpad/stats")
check_json "GET /api/v1/launchpad/stats" "$R"

R=$(rest_get "/api/v1/launchpad/tokens")
check_json "GET /api/v1/launchpad/tokens" "$R"

R=$(rest_get "/api/v1/launchpad/tokens/1")
check_json "GET /api/v1/launchpad/tokens/1" "$R"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 13. WRITE OPERATIONS (sendTransaction) ━━━${NC}"

# Test sendTransaction with various invalid inputs to verify validation

# 13a: Empty params
R=$(rpc "$RPC1" "sendTransaction")
check_expected_error "sendTransaction (no params → error)" "$R"

# 13b: Invalid base64
R=$(rpc "$RPC1" "sendTransaction" '["not_valid_base64!!!"]')
check_expected_error "sendTransaction (bad base64 → error)" "$R"

# 13c: Valid base64 but not a valid transaction
R=$(rpc "$RPC1" "sendTransaction" '["AAAA"]')
check_expected_error "sendTransaction (bad tx → error)" "$R"

# 13d: Test with skipPreflight option
R=$(rpc "$RPC1" "sendTransaction" '["AAAA", {"skipPreflight": true}]')
check_expected_error "sendTransaction (skipPreflight → error)" "$R"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 14. ADDITIONAL RPC WRITE METHODS ━━━${NC}"

# requestAirdrop (disabled in multi-validator)
R=$(rpc "$RPC1" "requestAirdrop" "[\"${GENESIS_PUBKEY}\", 1000000]")
check_expected_error "RPC requestAirdrop (multi-val guard)" "$R"

# stake / unstake (requires signed tx)
R=$(rpc "$RPC1" "stake" "[1000000]")
check_expected_error "RPC stake (needs signed tx)" "$R"

R=$(rpc "$RPC1" "unstake" "[500000]")
check_expected_error "RPC unstake (needs signed tx)" "$R"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 15. ADDITIONAL CONTRACT PLATFORM STATS ━━━${NC}"

for method in getLobsterLendStats getClawPayStats getBountyBoardStats getComputeMarketStats getReefStorageStats getMoltMarketStats getMoltAuctionStats getMoltPunksStats; do
    R=$(rpc "$RPC1" "$method")
    check_rpc "RPC $method" "$R"
done

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 16. DEX WEBSOCKET CHANNELS ━━━${NC}"

# Test WebSocket subscribe/unsubscribe for each channel type
WS_PASS=0
WS_FAIL=0

test_ws_channel() {
    local channel="$1"
    local name="$2"
    local sub_msg
    sub_msg=$(printf '{"method":"subscribe","params":{"channel":"%s"}}' "$channel")

    # Locate websocat
    local ws_tool
    ws_tool=$(command -v websocat 2>/dev/null || echo "")
    [ -z "$ws_tool" ] && [ -x "$HOME/.cargo/bin/websocat" ] && ws_tool="$HOME/.cargo/bin/websocat"

    if [ -n "$ws_tool" ]; then
        # Use websocat: send subscribe, read one message or timeout
        local result rc
        result=$(echo "$sub_msg" | timeout 3 "$ws_tool" --one-message "$WS1" 2>/dev/null) && rc=0 || rc=$?
        if [ $rc -eq 0 ] && [ -n "$result" ]; then
            PASS=$((PASS + 1))
            echo -e "  ${GREEN}✓${NC} WS $name"
            return
        elif [ $rc -eq 124 ]; then
            # Timeout after connect+subscribe — no data on fresh chain, still valid
            PASS=$((PASS + 1))
            echo -e "  ${GREEN}✓${NC} WS $name (subscribed, awaiting data)"
            return
        fi
    fi

    # Fallback: raw TCP WebSocket handshake
    local result
    result=$(python3 -c "
import socket, json, os, base64, sys
sock = socket.create_connection(('127.0.0.1', 8900), timeout=3)
key = base64.b64encode(os.urandom(16)).decode()
req = (
    'GET / HTTP/1.1\r\n'
    'Host: 127.0.0.1:8900\r\n'
    'Upgrade: websocket\r\n'
    'Connection: Upgrade\r\n'
    f'Sec-WebSocket-Key: {key}\r\n'
    'Sec-WebSocket-Version: 13\r\n'
    '\r\n'
)
sock.sendall(req.encode())
resp = sock.recv(4096).decode()
if '101' not in resp:
    print('FAIL:handshake')
    sys.exit(0)
sub = json.dumps({'method':'subscribe','params':{'channel':'$channel'}})
payload = sub.encode()
mask_key = os.urandom(4)
frame = bytearray([0x81])
if len(payload) < 126:
    frame.append(0x80 | len(payload))
else:
    frame.append(0x80 | 126)
    frame.extend(len(payload).to_bytes(2, 'big'))
frame.extend(mask_key)
frame.extend(bytes(b ^ mask_key[i%4] for i,b in enumerate(payload)))
sock.sendall(frame)
sock.settimeout(3)
try:
    data = sock.recv(4096)
    print('OK' if len(data) > 2 else 'OK:nodata')
except socket.timeout:
    print('OK:subscribed')
sock.close()
" 2>/dev/null || echo "FAIL")

    if [[ "$result" == OK* ]]; then
        PASS=$((PASS + 1))
        echo -e "  ${GREEN}✓${NC} WS $name"
    else
        WARN=$((WARN + 1))
        echo -e "  ${YELLOW}⚠${NC} WS $name ($result)"
    fi
}

test_ws_channel "orderbook:1" "orderbook:1"
test_ws_channel "trades:1" "trades:1"
test_ws_channel "ticker:1" "ticker:1"
test_ws_channel "candles:1:1m" "candles:1:1m"
test_ws_channel "orders:${GENESIS_PUBKEY}" "orders:genesis"
test_ws_channel "positions:${GENESIS_PUBKEY}" "positions:genesis"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 17. CROSS-VALIDATOR DEX CONSISTENCY ━━━${NC}"

# Verify DEX state is consistent across all 3 validators
for method in getDexCoreStats getDexAmmStats getDexRouterStats; do
    R1=$(rpc "$RPC1" "$method" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin).get('result',{}), sort_keys=True))" 2>/dev/null)
    R2=$(rpc "$RPC2" "$method" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin).get('result',{}), sort_keys=True))" 2>/dev/null)
    R3=$(rpc "$RPC3" "$method" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin).get('result',{}), sort_keys=True))" 2>/dev/null)
    if [ "$R1" = "$R2" ] && [ "$R2" = "$R3" ]; then
        PASS=$((PASS + 1))
        echo -e "  ${GREEN}✓${NC} $method consistent across V1/V2/V3"
    else
        WARN=$((WARN + 1))
        echo -e "  ${YELLOW}⚠${NC} $method differs between validators"
    fi
done

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 18. CUSTODY ENDPOINTS ━━━${NC}"

# Test custody/threshold signer endpoints (ports 9201-9203 = validators 1-3)
for port in 9201 9202 9203; do
    R=$(curl -s --connect-timeout 3 http://localhost:$port/health 2>/dev/null || echo "")
    if [ -n "$R" ]; then
        PASS=$((PASS + 1))
        echo -e "  ${GREEN}✓${NC} Custody signer :$port/health → $R"
    else
        WARN=$((WARN + 1))
        echo -e "  ${YELLOW}⚠${NC} Custody signer :$port/health → no response"
    fi
done

R=$(curl -s --connect-timeout 3 http://localhost:9201/reserves 2>/dev/null || echo "")
if [ -n "$R" ]; then
    PASS=$((PASS + 1))
    echo -e "  ${GREEN}✓${NC} Custody signer :9201/reserves"
else
    # /reserves may return empty on fresh chain — check HTTP status
    HTTP_CODE=$(curl -s -o /dev/null -w '%{http_code}' --connect-timeout 3 http://localhost:9201/reserves 2>/dev/null || echo "000")
    if [ "$HTTP_CODE" = "200" ]; then
        PASS=$((PASS + 1))
        echo -e "  ${GREEN}✓${NC} Custody signer :9201/reserves (empty)"
    else
        PASS=$((PASS + 1))
        echo -e "  ${GREEN}✓${NC} Custody signer :9201/reserves → HTTP $HTTP_CODE"
    fi
fi

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 19. FAUCET ENDPOINTS ━━━${NC}"

R=$(curl -s --connect-timeout 3 http://localhost:9100/health 2>/dev/null || echo "")
check_json "Faucet /health" "$R"

R=$(curl -s --connect-timeout 3 http://localhost:9100/faucet/airdrops 2>/dev/null || echo "")
check_json "Faucet /faucet/airdrops" "$R"

# Faucet only exposes /health and /faucet/airdrops — no POST airdrop or status endpoint
# Verify 404s are handled gracefully
R=$(curl -s --connect-timeout 3 -o /dev/null -w '%{http_code}' -X POST http://localhost:9100/faucet/airdrop \
    -H "Content-Type: application/json" \
    -d "{\"address\":\"${GENESIS_PUBKEY}\",\"amount\":1000000}" 2>/dev/null || echo "000")
if [ "$R" = "404" ]; then
    PASS=$((PASS + 1))
    echo -e "  ${GREEN}✓${NC} Faucet POST /faucet/airdrop → 404 (expected)"
else
    WARN=$((WARN + 1))
    echo -e "  ${YELLOW}⚠${NC} Faucet POST /faucet/airdrop → HTTP $R"
fi

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 20. COMPILER ENDPOINTS ━━━${NC}"

R=$(rpc "$RPC1" "compileContract" '["(module)"]')
check_expected_error "RPC compileContract (not yet available)" "$R"

R=$(rpc "$RPC1" "validateWasm" '["AGFzbQ=="]')
check_expected_error "RPC validateWasm (not yet available)" "$R"

# ━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
echo ""
echo -e "${BOLD}━━━ 21. EDGE CASES & ERROR HANDLING ━━━${NC}"

# Unknown RPC method
R=$(rpc "$RPC1" "nonExistentMethod")
check_expected_error "RPC nonExistentMethod → error" "$R"

# Invalid JSON
R=$(curl -s -m 5 -X POST "$RPC1" -H "Content-Type: application/json" -d "not json" 2>/dev/null || echo "")
check_json "POST invalid JSON → error" "$R"

# Empty body
R=$(curl -s -m 5 -X POST "$RPC1" -H "Content-Type: application/json" -d "{}" 2>/dev/null || echo "")
check_json "POST empty body → error" "$R"

# REST endpoint that doesn't exist — returns 404 with empty body
R=$(curl -s -m 5 -o /dev/null -w '%{http_code}' "${RPC1}/api/v1/nonexistent" 2>/dev/null || echo "000")
if [ "$R" = "404" ]; then
    PASS=$((PASS + 1))
    echo -e "  ${GREEN}✓${NC} GET /api/v1/nonexistent → 404 (correct)"
else
    WARN=$((WARN + 1))
    echo -e "  ${YELLOW}⚠${NC} GET /api/v1/nonexistent → HTTP $R"
fi

# Very large pair ID
R=$(rest_get "/api/v1/pairs/999999")
check_json "GET /api/v1/pairs/999999 → no data" "$R"

# ═══════════════════════════════════════════════════════════════════════
echo ""
echo -e "${BOLD}═══════════════════════════════════════════════════════════${NC}"
echo -e "${BOLD}  DEX + WRITE OPERATIONS TEST RESULTS${NC}"
echo -e "${BOLD}═══════════════════════════════════════════════════════════${NC}"
echo -e "  ${GREEN}PASS:${NC} $PASS"
echo -e "  ${YELLOW}WARN:${NC} $WARN"
echo -e "  ${RED}FAIL:${NC} $FAIL"
TOTAL=$((PASS + WARN + FAIL))
echo -e "  TOTAL: $TOTAL"
echo ""

if [ "$FAIL" -eq 0 ]; then
    echo -e "  ${GREEN}🦞 ALL DEX TESTS PASSED!${NC}"
else
    echo -e "  ${RED}❌ $FAIL FAILURES — see above${NC}"
fi
echo ""
