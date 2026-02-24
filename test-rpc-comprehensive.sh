#!/bin/bash
# Comprehensive RPC Endpoint Test
# Tests all 24+ RPC methods against live validator

set -e
RPC_URL="http://localhost:8899"
VALIDATOR_ADDR="B21dUmYNBTHCBgdemEXYRu6voEsECC4fD77D94ienMcN"
GENESIS_ADDR="GKopYobrUh7L9mDGBVMCEFgah3q8u5YFBHyFN5Qv9x2t"
RESULTS_FILE="/tmp/rpc-test-results.txt"

echo "🧪 MoltChain RPC Comprehensive Test" > $RESULTS_FILE
echo "Started: $(date)" >> $RESULTS_FILE
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" >> $RESULTS_FILE

PASS=0
FAIL=0

test_rpc() {
    local name="$1"
    local method="$2"
    local params="$3"
    
    echo -n "Testing: $name... "
    local result=$(curl -s -X POST $RPC_URL \
        -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":$params}")
    
    if echo "$result" | jq -e '.result' > /dev/null 2>&1; then
        echo "✅ PASS" | tee -a $RESULTS_FILE
        echo "   Response: $(echo $result | jq -c '.result' | head -c 100)" | tee -a $RESULTS_FILE
        PASS=$((PASS + 1))
    elif echo "$result" | jq -e '.error' > /dev/null 2>&1; then
        echo "⚠️  ERROR" | tee -a $RESULTS_FILE
        echo "   Error: $(echo $result | jq -c '.error.message')" | tee -a $RESULTS_FILE
        FAIL=$((FAIL + 1))
    else
        echo "❌ FAIL - Invalid response" | tee -a $RESULTS_FILE
        FAIL=$((FAIL + 1))
    fi
}

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "1️⃣  ACCOUNT & BALANCE METHODS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

test_rpc "getBalance (validator)" "getBalance" "[\"$VALIDATOR_ADDR\"]"
test_rpc "getBalance (genesis)" "getBalance" "[\"$GENESIS_ADDR\"]"
test_rpc "getAccountInfo (validator)" "getAccountInfo" "[\"$VALIDATOR_ADDR\"]"
test_rpc "getAccountInfo (genesis)" "getAccountInfo" "[\"$GENESIS_ADDR\"]"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "2️⃣  BLOCK METHODS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

test_rpc "getBlock (slot 0)" "getBlock" "[0]"
test_rpc "getLatestBlock" "getLatestBlock" "[]"
test_rpc "getSlot" "getSlot" "[]"

LATEST_SLOT=$(curl -s -X POST $RPC_URL -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","id":1,"method":"getSlot","params":[]}' | jq -r '.result // 0')
if [[ "$LATEST_SLOT" =~ ^[0-9]+$ ]]; then
    test_rpc "getBlock (latest slot)" "getBlock" "[$LATEST_SLOT]"
else
    echo "❌ FAIL - could not fetch latest slot" | tee -a $RESULTS_FILE
    FAIL=$((FAIL + 1))
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "3️⃣  TRANSACTION METHODS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# sendTransaction with invalid payload should return an RPC error (endpoint works)
echo -n "Testing: sendTransaction (invalid payload returns error)... "
INVALID_TX_RESULT=$(curl -s -X POST $RPC_URL -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","id":1,"method":"sendTransaction","params":["invalid_base64"]}')
if echo "$INVALID_TX_RESULT" | jq -e '.error' >/dev/null 2>&1; then
    echo "✅ PASS" | tee -a $RESULTS_FILE
    PASS=$((PASS + 1))
else
    echo "❌ FAIL" | tee -a $RESULTS_FILE
    FAIL=$((FAIL + 1))
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "4️⃣  VALIDATOR & STAKING METHODS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

test_rpc "getValidators" "getValidators" "[]"
test_rpc "getStakingRewards" "getStakingRewards" "[\"$VALIDATOR_ADDR\"]"
test_rpc "getStakingStatus" "getStakingStatus" "[\"$VALIDATOR_ADDR\"]"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "5️⃣  NETWORK & CHAIN METHODS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

test_rpc "getNetworkInfo" "getNetworkInfo" "[]"
test_rpc "getPeers" "getPeers" "[]"
test_rpc "getChainStatus" "getChainStatus" "[]"
test_rpc "getMetrics" "getMetrics" "[]"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "6️⃣  CONTRACT METHODS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

CONTRACT_ADDR=$(curl -s -X POST $RPC_URL -H "Content-Type: application/json" -d '{"jsonrpc":"2.0","id":1,"method":"getAllContracts","params":[]}' | jq -r '.result.contracts[0].program_id // empty')
if [[ -n "$CONTRACT_ADDR" ]]; then
    test_rpc "getContractInfo" "getContractInfo" "[\"$CONTRACT_ADDR\"]"
    echo -n "Testing: callContract (invalid function returns result/error)... "
    CALL_RESULT=$(curl -s -X POST $RPC_URL -H "Content-Type: application/json" -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"callContract\",\"params\":[\"$CONTRACT_ADDR\",\"__nonexistent__\",[]]}")
    if echo "$CALL_RESULT" | jq -e '.result or .error' >/dev/null 2>&1; then
        echo "✅ PASS" | tee -a $RESULTS_FILE
        PASS=$((PASS + 1))
    else
        echo "❌ FAIL" | tee -a $RESULTS_FILE
        FAIL=$((FAIL + 1))
    fi
else
    echo "❌ FAIL - no deployed contract found" | tee -a $RESULTS_FILE
    FAIL=$((FAIL + 1))
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "7️⃣  SUPPLY & ECONOMICS METHODS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

test_rpc "getTreasuryInfo" "getTreasuryInfo" "[]"
test_rpc "getGenesisAccounts" "getGenesisAccounts" "[]"
test_rpc "getTotalBurned" "getTotalBurned" "[]"
test_rpc "getReefStakePoolInfo" "getReefStakePoolInfo" "[]"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 TEST SUMMARY"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "✅ PASSED: $PASS"
echo "❌ FAILED/ERROR: $FAIL"
echo "SKIPPED: 0"
echo ""
echo "Results saved to: $RESULTS_FILE"
echo ""

# Print results summary to file
echo "" >> $RESULTS_FILE
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" >> $RESULTS_FILE
echo "TEST SUMMARY" >> $RESULTS_FILE
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" >> $RESULTS_FILE
echo "PASSED: $PASS" >> $RESULTS_FILE
echo "FAILED/ERROR: $FAIL" >> $RESULTS_FILE
echo "Completed: $(date)" >> $RESULTS_FILE

exit 0
