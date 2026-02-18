#!/usr/bin/env bash
# ═══════════════════════════════════════════════════════════════════════════════
# Comprehensive DEX REST API Test Suite
# Tests all DEX endpoints against a live validator with fresh genesis state.
# Expects validator running on localhost:8899 with 7 pairs + 7 pools.
# ═══════════════════════════════════════════════════════════════════════════════
set -euo pipefail

BASE="http://localhost:8899/api/v1"
PASS=0
FAIL=0
SKIP=0

pass() { echo "  ✅ PASS: $1"; PASS=$((PASS+1)); }
fail() { echo "  ❌ FAIL: $1 — $2"; FAIL=$((FAIL+1)); }
skip() { echo "  ⏭️  SKIP: $1"; SKIP=$((SKIP+1)); }

check_json() {
    local desc="$1" url="$2" jq_expr="$3" expected="$4"
    local raw
    raw=$(curl -sf "$url" 2>/dev/null) || { fail "$desc" "HTTP error"; return; }
    local actual
    actual=$(echo "$raw" | python3 -c "import sys,json; d=json.load(sys.stdin); print($jq_expr)" 2>/dev/null) || { fail "$desc" "parse error"; return; }
    if [[ "$actual" == "$expected" ]]; then
        pass "$desc"
    else
        fail "$desc" "expected '$expected', got '$actual'"
    fi
}

check_json_contains() {
    local desc="$1" url="$2" jq_expr="$3" substring="$4"
    local raw
    raw=$(curl -sf "$url" 2>/dev/null) || { fail "$desc" "HTTP error"; return; }
    local actual
    actual=$(echo "$raw" | python3 -c "import sys,json; d=json.load(sys.stdin); print($jq_expr)" 2>/dev/null) || { fail "$desc" "parse error"; return; }
    if echo "$actual" | grep -q "$substring"; then
        pass "$desc"
    else
        fail "$desc" "expected to contain '$substring', got '$actual'"
    fi
}

check_status() {
    local desc="$1" url="$2"
    local code
    code=$(curl -sf -o /dev/null -w "%{http_code}" "$url" 2>/dev/null) || code="000"
    if [[ "$code" == "200" ]]; then
        pass "$desc"
    else
        fail "$desc" "HTTP $code"
    fi
}

echo "═══════════════════════════════════════════════════════"
echo "  DEX REST API Comprehensive Test Suite"
echo "═══════════════════════════════════════════════════════"
echo ""

# ─── 1. Core Stats ───
echo "── 1. Core Stats ──"
check_json "stats/core pair_count=7" "$BASE/stats/core" "d['data']['pair_count']" "7"
check_json "stats/core order_count=0" "$BASE/stats/core" "d['data']['order_count']" "0"
check_json "stats/core trade_count=0" "$BASE/stats/core" "d['data']['trade_count']" "0"

# ─── 2. AMM Stats ───
echo "── 2. AMM Stats ──"
check_json "stats/amm pool_count=7" "$BASE/stats/amm" "d['data']['pool_count']" "7"
check_json "stats/amm swap_count=0" "$BASE/stats/amm" "d['data']['swap_count']" "0"

# ─── 3. GET /pairs — list all trading pairs ───
echo "── 3. Trading Pairs ──"
check_json "pairs returns 7" "$BASE/pairs" "len(d['data'])" "7"
check_json "pairs[0] has pairId" "$BASE/pairs" "d['data'][0]['pairId']" "1"
check_json "pairs[0] symbol=MOLT/mUSD" "$BASE/pairs" "d['data'][0]['symbol']" "MOLT/mUSD"
check_json "pairs[0] baseSymbol=MOLT" "$BASE/pairs" "d['data'][0]['baseSymbol']" "MOLT"
check_json "pairs[0] quoteSymbol=mUSD" "$BASE/pairs" "d['data'][0]['quoteSymbol']" "mUSD"
check_json "pairs[0] status=active" "$BASE/pairs" "d['data'][0]['status']" "active"
check_json "pairs[0] has tickSize" "$BASE/pairs" "d['data'][0]['tickSize']" "1"
check_json "pairs[0] has lotSize" "$BASE/pairs" "d['data'][0]['lotSize']" "1000000"
check_json "pairs[0] has minOrder" "$BASE/pairs" "d['data'][0]['minOrder']" "1000"
check_json "pairs[0] camelCase dailyVolume" "$BASE/pairs" "str(d['data'][0]['dailyVolume'])" "0"
check_json "pairs[0] camelCase makerFeeBps" "$BASE/pairs" "str(d['data'][0]['makerFeeBps'])" "-1"
check_json "pairs[0] camelCase takerFeeBps" "$BASE/pairs" "str(d['data'][0]['takerFeeBps'])" "5"

# Verify all 7 pair symbols
check_json "pair1=MOLT/mUSD" "$BASE/pairs" "d['data'][0]['symbol']" "MOLT/mUSD"
check_json "pair2=wSOL/mUSD" "$BASE/pairs" "d['data'][1]['symbol']" "wSOL/mUSD"
check_json "pair3=wETH/mUSD" "$BASE/pairs" "d['data'][2]['symbol']" "wETH/mUSD"
check_json "pair4=REEF/mUSD" "$BASE/pairs" "d['data'][3]['symbol']" "REEF/mUSD"
check_json "pair5=wSOL/MOLT" "$BASE/pairs" "d['data'][4]['symbol']" "wSOL/MOLT"
check_json "pair6=wETH/MOLT" "$BASE/pairs" "d['data'][5]['symbol']" "wETH/MOLT"
check_json "pair7=REEF/MOLT" "$BASE/pairs" "d['data'][6]['symbol']" "REEF/MOLT"

# ─── 4. GET /pairs/:id — single pair ───
echo "── 4. Single Pair ──"
check_json "pair/1 symbol=MOLT/mUSD" "$BASE/pairs/1" "d['data']['symbol']" "MOLT/mUSD"
check_json "pair/1 baseSymbol=MOLT" "$BASE/pairs/1" "d['data']['baseSymbol']" "MOLT"
check_json "pair/1 quoteSymbol=mUSD" "$BASE/pairs/1" "d['data']['quoteSymbol']" "mUSD"
check_json "pair/7 symbol=REEF/MOLT" "$BASE/pairs/7" "d['data']['symbol']" "REEF/MOLT"
# Non-existent pair → 404 with success=false
NF_CODE=$(curl -s -o /tmp/dex_nf.json -w "%{http_code}" "$BASE/pairs/99")
NF_SUCCESS=$(python3 -c "import json; print(json.load(open('/tmp/dex_nf.json'))['success'])" 2>/dev/null || echo "?")
if [[ "$NF_CODE" == "404" && "$NF_SUCCESS" == "False" ]]; then
    pass "pair/99 → 404 not found"
else
    fail "pair/99 → not found" "code=$NF_CODE success=$NF_SUCCESS"
fi

# ─── 5. GET /pairs/:id/orderbook ───
echo "── 5. Orderbook ──"
check_json "orderbook/1 has pairId" "$BASE/pairs/1/orderbook" "d['data']['pairId']" "1"
check_json "orderbook/1 bids=[]" "$BASE/pairs/1/orderbook" "len(d['data']['bids'])" "0"
check_json "orderbook/1 asks=[]" "$BASE/pairs/1/orderbook" "len(d['data']['asks'])" "0"
check_json "orderbook/1 has slot" "$BASE/pairs/1/orderbook" "type(d['data']['slot']).__name__" "int"

# ─── 6. GET /pairs/:id/trades ───
echo "── 6. Recent Trades ──"
check_json "trades/1 success" "$BASE/pairs/1/trades" "d['success']" "True"
check_json "trades/1 empty" "$BASE/pairs/1/trades" "len(d['data'])" "0"

# ─── 7. GET /pairs/:id/candles ───
echo "── 7. Candles ──"
check_json "candles/1 success" "$BASE/pairs/1/candles?interval=60&limit=10" "d['success']" "True"
check_json "candles/1 empty" "$BASE/pairs/1/candles?interval=60&limit=10" "len(d['data'])" "0"

# ─── 8. GET /pairs/:id/ticker ───
echo "── 8. Ticker ──"
check_json "ticker/1 pairId=1" "$BASE/pairs/1/ticker" "d['data']['pairId']" "1"
check_json "ticker/1 camelCase lastPrice" "$BASE/pairs/1/ticker" "str(d['data']['lastPrice'])" "0.0"
check_json "ticker/1 camelCase volume24h" "$BASE/pairs/1/ticker" "str(d['data']['volume24h'])" "0"
check_json "ticker/1 camelCase change24h" "$BASE/pairs/1/ticker" "str(d['data']['change24h'])" "0.0"
check_json "ticker/1 camelCase high24h" "$BASE/pairs/1/ticker" "str(d['data']['high24h'])" "0.0"
check_json "ticker/1 camelCase low24h" "$BASE/pairs/1/ticker" "str(d['data']['low24h'])" "0.0"
check_json "ticker/1 camelCase trades24h" "$BASE/pairs/1/ticker" "str(d['data']['trades24h'])" "0"
# has bid and ask fields
check_json "ticker/1 has bid" "$BASE/pairs/1/ticker" "'bid' in d['data']" "True"
check_json "ticker/1 has ask" "$BASE/pairs/1/ticker" "'ask' in d['data']" "True"

# ─── 9. GET /tickers — all tickers ───
echo "── 9. All Tickers ──"
check_json "tickers returns 7" "$BASE/tickers" "len(d['data'])" "7"
check_json "tickers[0] camelCase pairId" "$BASE/tickers" "d['data'][0]['pairId']" "1"
check_json "tickers[0] camelCase lastPrice" "$BASE/tickers" "'lastPrice' in d['data'][0]" "True"
check_json "tickers[0] camelCase volume24h" "$BASE/tickers" "'volume24h' in d['data'][0]" "True"
check_json "tickers[0] camelCase change24h" "$BASE/tickers" "'change24h' in d['data'][0]" "True"

# ─── 10. GET /pools — AMM pools ───
echo "── 10. AMM Pools ──"
check_json "pools returns 7" "$BASE/pools" "len(d['data'])" "7"
check_json "pool[0] camelCase poolId" "$BASE/pools" "d['data'][0]['poolId']" "1"
check_json "pool[0] tokenASymbol=MOLT" "$BASE/pools" "d['data'][0]['tokenASymbol']" "MOLT"
check_json "pool[0] tokenBSymbol=mUSD" "$BASE/pools" "d['data'][0]['tokenBSymbol']" "mUSD"
check_json "pool[0] camelCase tokenA" "$BASE/pools" "len(d['data'][0]['tokenA']) > 0" "True"
check_json "pool[0] camelCase tokenB" "$BASE/pools" "len(d['data'][0]['tokenB']) > 0" "True"
check_json "pool[0] camelCase feeTier" "$BASE/pools" "'feeTier' in d['data'][0]" "True"
check_json "pool[0] camelCase sqrtPrice" "$BASE/pools" "'sqrtPrice' in d['data'][0]" "True"

# Verify all pool symbols
check_json "pool1=MOLT/mUSD" "$BASE/pools" "f\"{d['data'][0]['tokenASymbol']}/{d['data'][0]['tokenBSymbol']}\"" "MOLT/mUSD"
check_json "pool7=REEF/MOLT" "$BASE/pools" "f\"{d['data'][6]['tokenASymbol']}/{d['data'][6]['tokenBSymbol']}\"" "REEF/MOLT"

# ─── 11. GET /pools/:id ───
echo "── 11. Single Pool ──"
check_status "pool/1 responds 200" "$BASE/pools/1"
check_json "pool/1 poolId=1" "$BASE/pools/1" "d['data']['poolId']" "1"

# ─── 12. Orders endpoints ───
echo "── 12. Orders & Positions Endpoints ──"
TRADER="deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
check_json "GET /orders?trader= success" "$BASE/orders?trader=$TRADER" "d['success']" "True"
check_json "GET /orders?trader= data=[]" "$BASE/orders?trader=$TRADER" "len(d['data'])" "0"
check_json "GET /pools/positions?owner= success" "$BASE/pools/positions?owner=$TRADER" "d['success']" "True"
check_json "GET /margin/positions?trader= success" "$BASE/margin/positions?trader=$TRADER" "d['success']" "True"

# ─── 13. Margin endpoints ───
echo "── 13. Margin Endpoints ──"
check_status "margin/info responds" "$BASE/margin/info"

# ─── 14. Leaderboard ───
echo "── 14. Leaderboard ──"
check_json "leaderboard success" "$BASE/leaderboard" "d['success']" "True"
check_json "leaderboard empty" "$BASE/leaderboard" "len(d['data'])" "0"

# ─── 15. Governance ───
echo "── 15. Governance ──"
check_json "governance proposals success" "$BASE/governance/proposals" "d['success']" "True"

# ─── 16. Rewards ───
echo "── 16. Rewards ──"
check_status "rewards/info responds" "$BASE/rewards/info"

# ─── 17. Platform stats ───
echo "── 17. Platform Stats ──"
check_status "stats/margin responds" "$BASE/stats/margin"
check_status "stats/router responds" "$BASE/stats/router"
check_status "stats/rewards responds" "$BASE/stats/rewards"
check_status "stats/analytics responds" "$BASE/stats/analytics"
check_status "stats/governance responds" "$BASE/stats/governance"
check_status "stats/moltswap responds" "$BASE/stats/moltswap"

# ─── 18. camelCase verification — NO snake_case fields in REST responses ───
echo "── 18. camelCase Verification ──"
RAW_PAIRS=$(curl -sf "$BASE/pairs" 2>/dev/null)
if echo "$RAW_PAIRS" | grep -q '"pair_id"'; then
    fail "pairs: snake_case pair_id found" "should be pairId"
elif echo "$RAW_PAIRS" | grep -q '"pairId"'; then
    pass "pairs: uses camelCase pairId"
else
    skip "pairs: pairId field not found"
fi

if echo "$RAW_PAIRS" | grep -q '"base_token"'; then
    fail "pairs: snake_case base_token found" "should be baseToken"
elif echo "$RAW_PAIRS" | grep -q '"baseToken"'; then
    pass "pairs: uses camelCase baseToken"
else
    skip "pairs: baseToken field not found"
fi

if echo "$RAW_PAIRS" | grep -q '"quote_token"'; then
    fail "pairs: snake_case quote_token found" "should be quoteToken"
elif echo "$RAW_PAIRS" | grep -q '"quoteToken"'; then
    pass "pairs: uses camelCase quoteToken"
else
    skip "pairs: quoteToken field not found"
fi

if echo "$RAW_PAIRS" | grep -q '"tick_size"'; then
    fail "pairs: snake_case tick_size found" "should be tickSize"
elif echo "$RAW_PAIRS" | grep -q '"tickSize"'; then
    pass "pairs: uses camelCase tickSize"
else
    skip "pairs: tickSize field not found"
fi

if echo "$RAW_PAIRS" | grep -q '"maker_fee_bps"'; then
    fail "pairs: snake_case maker_fee_bps found" "should be makerFeeBps"
elif echo "$RAW_PAIRS" | grep -q '"makerFeeBps"'; then
    pass "pairs: uses camelCase makerFeeBps"
else
    skip "pairs: makerFeeBps field not found"
fi

RAW_TICKER=$(curl -sf "$BASE/pairs/1/ticker" 2>/dev/null)
if echo "$RAW_TICKER" | grep -q '"last_price"'; then
    fail "ticker: snake_case last_price" "should be lastPrice"
elif echo "$RAW_TICKER" | grep -q '"lastPrice"'; then
    pass "ticker: uses camelCase lastPrice"
else
    skip "ticker: lastPrice not found"
fi

if echo "$RAW_TICKER" | grep -q '"volume_24h"'; then
    fail "ticker: snake_case volume_24h" "should be volume24h"
elif echo "$RAW_TICKER" | grep -q '"volume24h"'; then
    pass "ticker: uses camelCase volume24h"
else
    skip "ticker: volume24h not found"
fi

if echo "$RAW_TICKER" | grep -q '"change_24h"'; then
    fail "ticker: snake_case change_24h" "should be change24h"
elif echo "$RAW_TICKER" | grep -q '"change24h"'; then
    pass "ticker: uses camelCase change24h"
else
    skip "ticker: change24h not found"
fi

RAW_POOLS=$(curl -sf "$BASE/pools" 2>/dev/null)
if echo "$RAW_POOLS" | grep -q '"pool_id"'; then
    fail "pools: snake_case pool_id" "should be poolId"
elif echo "$RAW_POOLS" | grep -q '"poolId"'; then
    pass "pools: uses camelCase poolId"
else
    skip "pools: poolId not found"
fi

if echo "$RAW_POOLS" | grep -q '"token_a"'; then
    fail "pools: snake_case token_a" "should be tokenA"
elif echo "$RAW_POOLS" | grep -q '"tokenA"'; then
    pass "pools: uses camelCase tokenA"
else
    skip "pools: tokenA not found"
fi

RAW_OB=$(curl -sf "$BASE/pairs/1/orderbook" 2>/dev/null)
if echo "$RAW_OB" | grep -q '"pair_id"'; then
    fail "orderbook: snake_case pair_id" "should be pairId"
elif echo "$RAW_OB" | grep -q '"pairId"'; then
    pass "orderbook: uses camelCase pairId"
else
    skip "orderbook: pairId not found"
fi

# ─── 19. Symbol enrichment — base/quote addresses resolve to names ───
echo "── 19. Symbol Enrichment ──"
check_json "pair/2 baseSymbol=wSOL" "$BASE/pairs/2" "d['data']['baseSymbol']" "wSOL"
check_json "pair/2 quoteSymbol=mUSD" "$BASE/pairs/2" "d['data']['quoteSymbol']" "mUSD"
check_json "pair/3 baseSymbol=wETH" "$BASE/pairs/3" "d['data']['baseSymbol']" "wETH"
check_json "pair/4 baseSymbol=REEF" "$BASE/pairs/4" "d['data']['baseSymbol']" "REEF"
check_json "pair/5 quoteSymbol=MOLT" "$BASE/pairs/5" "d['data']['quoteSymbol']" "MOLT"
check_json "pool/2 tokenASymbol=wSOL" "$BASE/pools" "d['data'][1]['tokenASymbol']" "wSOL"
check_json "pool/3 tokenASymbol=wETH" "$BASE/pools" "d['data'][2]['tokenASymbol']" "wETH"
check_json "pool/4 tokenASymbol=REEF" "$BASE/pools" "d['data'][3]['tokenASymbol']" "REEF"

# ─── 20. Cross-endpoint consistency ───
echo "── 20. Cross-Endpoint Consistency ──"
PAIRS_COUNT=$(curl -sf "$BASE/pairs" | python3 -c "import sys,json; print(len(json.load(sys.stdin)['data']))")
TICKERS_COUNT=$(curl -sf "$BASE/tickers" | python3 -c "import sys,json; print(len(json.load(sys.stdin)['data']))")
POOLS_COUNT=$(curl -sf "$BASE/pools" | python3 -c "import sys,json; print(len(json.load(sys.stdin)['data']))")
STATS_PAIRS=$(curl -sf "$BASE/stats/core" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['pair_count'])")
STATS_POOLS=$(curl -sf "$BASE/stats/amm" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['pool_count'])")

if [[ "$PAIRS_COUNT" == "$TICKERS_COUNT" ]]; then
    pass "pairs count ($PAIRS_COUNT) == tickers count ($TICKERS_COUNT)"
else
    fail "pairs/tickers mismatch" "$PAIRS_COUNT != $TICKERS_COUNT"
fi

if [[ "$PAIRS_COUNT" == "$STATS_PAIRS" ]]; then
    pass "pairs count ($PAIRS_COUNT) == stats pair_count ($STATS_PAIRS)"
else
    fail "pairs/stats mismatch" "$PAIRS_COUNT != $STATS_PAIRS"
fi

if [[ "$POOLS_COUNT" == "$STATS_POOLS" ]]; then
    pass "pools count ($POOLS_COUNT) == stats pool_count ($STATS_POOLS)"
else
    fail "pools/stats mismatch" "$POOLS_COUNT != $STATS_POOLS"
fi

echo ""
echo "═══════════════════════════════════════════════════════"
echo "  RESULTS: $PASS PASS / $FAIL FAIL / $SKIP SKIP"
echo "═══════════════════════════════════════════════════════"

if [[ $FAIL -gt 0 ]]; then
    exit 1
fi
