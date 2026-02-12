#!/bin/bash
# MoltChain Launch Verification Test
# Final comprehensive check before launch

set -e

echo "🦞 MoltChain Launch Verification"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "Testing all systems before launch..."
echo ""

MOLT="./target/release/molt"
RPC_URL="http://localhost:8899"
VALIDATOR_ADDR="B21dUmYNBTHCBgdemEXYRu6voEsECC4fD77D94ienMcN"

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m' # No Color

pass_count=0
fail_count=0

test_item() {
    local name="$1"
    local command="$2"
    
    echo -n "Testing: $name... "
    if eval "$command" > /dev/null 2>&1; then
        echo -e "${GREEN}✅ PASS${NC}"
        ((pass_count++))
    else
        echo -e "${RED}❌ FAIL${NC}"
        ((fail_count++))
    fi
}

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "1️⃣  CLI COMMANDS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

test_item "status command" "$MOLT status"
test_item "metrics command" "$MOLT metrics"
test_item "validators command" "$MOLT validators"
test_item "balance command" "$MOLT balance $VALIDATOR_ADDR"
test_item "account info command" "$MOLT account info $VALIDATOR_ADDR"
test_item "network info command" "$MOLT network info"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "2️⃣  RPC ENDPOINTS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

test_item "getBalance" "curl -s -X POST $RPC_URL -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getBalance\",\"params\":[\"$VALIDATOR_ADDR\"]}' | jq -e '.result.shells'"
test_item "getAccountInfo" "curl -s -X POST $RPC_URL -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccountInfo\",\"params\":[\"$VALIDATOR_ADDR\"]}' | jq -e '.result.pubkey'"
test_item "getValidators" "curl -s -X POST $RPC_URL -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getValidators\",\"params\":[]}' | jq -e '.result.validators'"
test_item "getStakingRewards" "curl -s -X POST $RPC_URL -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getStakingRewards\",\"params\":[\"$VALIDATOR_ADDR\"]}' | jq -e '.result.bootstrap_debt'"
test_item "getNetworkInfo" "curl -s -X POST $RPC_URL -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getNetworkInfo\",\"params\":[]}' | jq -e '.result.chain_id'"
test_item "getChainStatus" "curl -s -X POST $RPC_URL -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getChainStatus\",\"params\":[]}' | jq -e '.result.current_slot'"
test_item "getTotalSupply" "curl -s -X POST $RPC_URL -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getTotalSupply\",\"params\":[]}' | jq -e '.result'"
test_item "getTotalStaked" "curl -s -X POST $RPC_URL -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getTotalStaked\",\"params\":[]}' | jq -e '.result'"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "3️⃣  BALANCE SEPARATION"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Get balance breakdown
balance_result=$(curl -s -X POST $RPC_URL -d '{"jsonrpc":"2.0","id":1,"method":"getBalance","params":["'$VALIDATOR_ADDR'"]}')

spendable=$(echo $balance_result | jq -r '.result.spendable')
staked=$(echo $balance_result | jq -r '.result.staked')
locked=$(echo $balance_result | jq -r '.result.locked')
total=$(echo $balance_result | jq -r '.result.shells')

echo "Balance breakdown:"
echo "  Spendable: $(echo "scale=4; $spendable / 1000000000" | bc) MOLT"
echo "  Staked:    $(echo "scale=4; $staked / 1000000000" | bc) MOLT"
echo "  Locked:    $(echo "scale=4; $locked / 1000000000" | bc) MOLT"
echo "  Total:     $(echo "scale=4; $total / 1000000000" | bc) MOLT"
echo ""

# Verify invariant: total = spendable + staked + locked
calculated_total=$((spendable + staked + locked))
if [ "$calculated_total" -eq "$total" ]; then
    echo -e "${GREEN}✅ PASS: Balance invariant maintained (total = spendable + staked + locked)${NC}"
    ((pass_count++))
else
    echo -e "${RED}❌ FAIL: Balance invariant broken! $calculated_total != $total${NC}"
    ((fail_count++))
fi

# Verify staked amount is correct (10K MOLT)
expected_staked=10000000000000
if [ "$staked" -eq "$expected_staked" ]; then
    echo -e "${GREEN}✅ PASS: Bootstrap stake correct (10,000 MOLT)${NC}"
    ((pass_count++))
else
    echo -e "${RED}❌ FAIL: Bootstrap stake incorrect! Expected 10,000 MOLT${NC}"
    ((fail_count++))
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "4️⃣  STAKEPOOL INTEGRATION"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

rewards_result=$(curl -s -X POST $RPC_URL -d '{"jsonrpc":"2.0","id":1,"method":"getStakingRewards","params":["'$VALIDATOR_ADDR'"]}')
bootstrap_debt=$(echo $rewards_result | jq -r '.result.bootstrap_debt')

echo "StakePool data:"
echo "  Bootstrap debt: $(echo "scale=4; $bootstrap_debt / 1000000000" | bc) MOLT"
echo ""

if [ "$bootstrap_debt" -eq "$expected_staked" ]; then
    echo -e "${GREEN}✅ PASS: StakePool correctly wired to RPC${NC}"
    ((pass_count++))
else
    echo -e "${RED}❌ FAIL: StakePool not properly wired${NC}"
    ((fail_count++))
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "5️⃣  MULTI-VALIDATOR"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

validator_count=$(curl -s -X POST $RPC_URL -d '{"jsonrpc":"2.0","id":1,"method":"getNetworkInfo","params":[]}' | jq -r '.result.validator_count')

echo "Network validators: $validator_count"
echo ""

if [ "$validator_count" -ge "2" ]; then
    echo -e "${GREEN}✅ PASS: Multi-validator cluster operational${NC}"
    ((pass_count++))
else
    echo -e "${RED}⚠️  WARN: Only $validator_count validator(s) running${NC}"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "6️⃣  SDK VERIFICATION"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

if [ -f "js-sdk/package.json" ] && [ -f "js-sdk/src/index.ts" ]; then
    echo -e "${GREEN}✅ PASS: JavaScript SDK complete${NC}"
    ((pass_count++))
else
    echo -e "${RED}❌ FAIL: JavaScript SDK missing${NC}"
    ((fail_count++))
fi

if [ -f "python-sdk/setup.py" ] && [ -f "python-sdk/moltchain/__init__.py" ]; then
    echo -e "${GREEN}✅ PASS: Python SDK complete${NC}"
    ((pass_count++))
else
    echo -e "${RED}❌ FAIL: Python SDK missing${NC}"
    ((fail_count++))
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 FINAL RESULTS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "Tests passed: $pass_count"
echo "Tests failed: $fail_count"
echo ""

if [ "$fail_count" -eq "0" ]; then
    echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${GREEN}✅ ALL SYSTEMS GO - READY FOR LAUNCH! 🚀${NC}"
    echo -e "${GREEN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    exit 0
else
    echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${RED}⚠️  SOME TESTS FAILED - REVIEW REQUIRED${NC}"
    echo -e "${RED}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    exit 1
fi
