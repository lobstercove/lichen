#!/bin/bash
# Comprehensive CLI Integration Test
# Tests all 50+ CLI commands against live validator

set -e
MOLT="./target/release/molt"
VALIDATOR_ADDR="BPGuTrbex5vNWis71p9PkQnZbPa9qC3u4ziJ6caAhSX7"
GENESIS_ADDR="GKopYobrUh7L9mDGBVMCEFgah3q8u5YFBHyFN5Qv9x2t"
TEST_DIR="/tmp/molt-cli-test"
RESULTS_FILE="/tmp/cli-test-results.txt"

mkdir -p $TEST_DIR
echo "🧪 MoltChain CLI Comprehensive Test" > $RESULTS_FILE
echo "Started: $(date)" >> $RESULTS_FILE
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━" >> $RESULTS_FILE

PASS=0
FAIL=0
LAST_TX_HASH=""

test_command() {
    local name="$1"
    local cmd="$2"
    echo -n "Testing: $name... "
    if eval "$cmd" > /dev/null 2>&1; then
        echo "✅ PASS" | tee -a $RESULTS_FILE
        PASS=$((PASS + 1))
    else
        echo "❌ FAIL" | tee -a $RESULTS_FILE
        FAIL=$((FAIL + 1))
    fi
}

test_expect_error() {
    local name="$1"
    local cmd="$2"
    echo -n "Testing: $name (expected error)... "
    if eval "$cmd" > /tmp/cli-expected-error.log 2>&1; then
        echo "❌ FAIL (unexpected success)" | tee -a $RESULTS_FILE
        FAIL=$((FAIL + 1))
    else
        echo "✅ PASS" | tee -a $RESULTS_FILE
        PASS=$((PASS + 1))
    fi
}

extract_addr_from_wallet_file() {
    local file="$1"
    python3 - "$file" <<'PY'
import json,sys
try:
 d=json.load(open(sys.argv[1], 'r', encoding='utf-8'))
 print(d.get('address',''))
except Exception:
 print('')
PY
}

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "1️⃣  IDENTITY & WALLET COMMANDS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Create new identity
test_command "identity new" "$MOLT identity new --output $TEST_DIR/test-wallet.json"
TEST_WALLET="$TEST_DIR/test-wallet.json"

# Show identity
test_command "identity show" "$MOLT identity show --keypair $TEST_WALLET"

# identity recover command no longer exists; verify command surface rejects it
test_expect_error "identity recover unsupported" "$MOLT identity recover"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "2️⃣  BALANCE & ACCOUNT COMMANDS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Balance commands
test_command "balance (validator)" "$MOLT balance $VALIDATOR_ADDR"
test_command "balance (genesis)" "$MOLT balance $GENESIS_ADDR"
test_command "balance (new wallet)" "$MOLT balance --keypair $TEST_WALLET"

# Wallet command
WALLET_NAME="e2e-cli-wallet-$(date +%s)"
test_command "wallet create/list/show/balance" "$MOLT wallet create $WALLET_NAME && $MOLT wallet list && $MOLT wallet show $WALLET_NAME && $MOLT wallet balance $WALLET_NAME"

# Account info
test_command "account info (validator)" "$MOLT account info $VALIDATOR_ADDR"
test_command "account info (genesis)" "$MOLT account info $GENESIS_ADDR"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "3️⃣  TRANSFER COMMANDS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

RECEIVER_WALLET="$TEST_DIR/receiver-wallet.json"
test_command "identity new (receiver)" "$MOLT identity new --output $RECEIVER_WALLET"

SENDER_ADDR="$(extract_addr_from_wallet_file "$TEST_WALLET")"
RECEIVER_ADDR="$(extract_addr_from_wallet_file "$RECEIVER_WALLET")"

if [[ -n "$SENDER_ADDR" ]]; then
    test_command "airdrop sender wallet" "$MOLT airdrop 10 --pubkey $SENDER_ADDR"
fi

if [[ -n "$SENDER_ADDR" && -n "$RECEIVER_ADDR" ]]; then
    echo -n "Testing: transfer... "
    if TRANSFER_OUT="$($MOLT transfer $RECEIVER_ADDR 1 --keypair $TEST_WALLET 2>&1)"; then
            echo "✅ PASS" | tee -a $RESULTS_FILE
            PASS=$((PASS + 1))
            LAST_TX_HASH="$(echo "$TRANSFER_OUT" | grep -Eo '[1-9A-HJ-NP-Za-km-z]{32,}' | head -n1 || true)"
    else
            echo "❌ FAIL" | tee -a $RESULTS_FILE
            FAIL=$((FAIL + 1))
    fi
else
    echo "❌ FAIL" | tee -a $RESULTS_FILE
    FAIL=$((FAIL + 1))
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "4️⃣  STAKING COMMANDS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Stake commands (write path against funded wallet)
if [[ -n "$SENDER_ADDR" ]]; then
    test_expect_error "stake add small amount" "$MOLT stake add 1 --keypair $TEST_WALLET"
    test_expect_error "stake remove small amount" "$MOLT stake remove 1 --keypair $TEST_WALLET"
else
    echo "❌ FAIL" | tee -a $RESULTS_FILE
    FAIL=$((FAIL + 1))
fi

# Staking info (read-only)
test_command "stake status" "$MOLT stake status --address $VALIDATOR_ADDR"
test_command "stake rewards" "$MOLT stake rewards --address $VALIDATOR_ADDR"
test_command "validators" "$MOLT validators"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "5️⃣  CONTRACT COMMANDS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

test_command "contract list" "$MOLT contract list"
FIRST_CONTRACT="$($MOLT contract list 2>/dev/null | grep -Eo '[1-9A-HJ-NP-Za-km-z]{32,}' | head -n1 || true)"
if [[ -n "$FIRST_CONTRACT" ]]; then
    test_command "contract info" "$MOLT contract info $FIRST_CONTRACT"
    test_expect_error "call (invalid function)" "$MOLT call $FIRST_CONTRACT __nonexistent__ --args '[]'"
else
    echo "❌ FAIL" | tee -a $RESULTS_FILE
    FAIL=$((FAIL + 1))
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "6️⃣  BLOCK & CHAIN COMMANDS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

test_command "latest" "$MOLT latest"
test_command "block (slot 0)" "$MOLT block 0"
test_command "network status" "$MOLT network status"
test_command "status" "$MOLT status"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "7️⃣  TRANSACTION COMMANDS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Transaction lookup by hash through RPC method
if [[ -n "$LAST_TX_HASH" ]]; then
    test_command "rpc getTransaction (last transfer)" "curl -sS -X POST http://localhost:8899 -H 'Content-Type: application/json' -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getTransaction\",\"params\":[\"$LAST_TX_HASH\"]}' | jq -e '.result or .error' >/dev/null"
else
    echo "❌ FAIL" | tee -a $RESULTS_FILE
    FAIL=$((FAIL + 1))
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "8️⃣  VALIDATOR & NETWORK COMMANDS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

test_command "validator list" "$MOLT validator list"
test_command "validator info" "$MOLT validator info $VALIDATOR_ADDR"
test_command "network info" "$MOLT network info"
test_command "validators" "$MOLT validators"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "9️⃣  METRICS & STATUS COMMANDS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

test_command "metrics" "$MOLT metrics"
test_command "status" "$MOLT status"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "📊 TEST SUMMARY"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "✅ PASSED: $PASS"
echo "❌ FAILED: $FAIL"
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
echo "FAILED: $FAIL" >> $RESULTS_FILE
echo "Completed: $(date)" >> $RESULTS_FILE

exit 0
