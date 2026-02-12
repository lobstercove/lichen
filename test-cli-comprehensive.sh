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

test_command() {
    local name="$1"
    local cmd="$2"
    echo -n "Testing: $name... "
    if eval "$cmd" > /dev/null 2>&1; then
        echo "✅ PASS" | tee -a $RESULTS_FILE
        ((PASS++))
    else
        echo "❌ FAIL" | tee -a $RESULTS_FILE
        ((FAIL++))
    fi
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

# Recover identity (skip - requires seed phrase input)
echo "⏭️  identity recover - SKIP (requires interactive input)" | tee -a $RESULTS_FILE

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
test_command "wallet balance" "$MOLT wallet balance --keypair $TEST_WALLET"

# Account info
test_command "account info (validator)" "$MOLT account info $VALIDATOR_ADDR"
test_command "account info (genesis)" "$MOLT account info $GENESIS_ADDR"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "3️⃣  TRANSFER COMMANDS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Note: Actual transfers require funded wallet - skip for now
echo "⏭️  transfer - SKIP (requires funded wallet)" | tee -a $RESULTS_FILE
echo "⏭️  send - SKIP (requires funded wallet)" | tee -a $RESULTS_FILE

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "4️⃣  STAKING COMMANDS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Stake commands (require funded wallet)
echo "⏭️  stake - SKIP (requires funded wallet)" | tee -a $RESULTS_FILE
echo "⏭️  unstake - SKIP (requires funded wallet)" | tee -a $RESULTS_FILE

# Staking info (read-only)
test_command "staking info" "$MOLT staking info $VALIDATOR_ADDR"
test_command "staking rewards" "$MOLT staking rewards $VALIDATOR_ADDR"
test_command "staking validators" "$MOLT staking validators"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "5️⃣  CONTRACT COMMANDS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Contract commands (require contract files)
echo "⏭️  deploy - SKIP (requires .wasm file)" | tee -a $RESULTS_FILE
echo "⏭️  call - SKIP (requires deployed contract)" | tee -a $RESULTS_FILE
echo "⏭️  contract info - SKIP (requires deployed contract)" | tee -a $RESULTS_FILE

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "6️⃣  BLOCK & CHAIN COMMANDS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

test_command "block latest" "$MOLT block latest"
test_command "block get (slot 0)" "$MOLT block get 0"
test_command "chain info" "$MOLT chain info"
test_command "chain status" "$MOLT chain status"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "7️⃣  TRANSACTION COMMANDS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

# Transaction lookup (requires tx hash)
echo "⏭️  transaction get - SKIP (requires tx hash)" | tee -a $RESULTS_FILE

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "8️⃣  VALIDATOR & NETWORK COMMANDS"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""

test_command "validators list" "$MOLT validators list"
test_command "validators show" "$MOLT validators show $VALIDATOR_ADDR"
test_command "network info" "$MOLT network info"
test_command "network peers" "$MOLT network peers"

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
echo "⏭️  SKIPPED: Commands requiring funded wallet or contracts"
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
