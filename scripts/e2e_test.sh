#!/usr/bin/env bash
set -euo pipefail

# ════════════════════════════════════════════════════════════════════════════════
# MoltChain 3-Validator E2E Test Suite
# Tests every RPC endpoint across all 3 API layers:
#   /       — Native Molt RPC
#   /solana — Solana-compatible RPC
#   /evm    — Ethereum-compatible RPC
#   /api/v1 — DEX REST API
# ════════════════════════════════════════════════════════════════════════════════

RPC1="http://127.0.0.1:8899"
RPC2="http://127.0.0.1:8901"
RPC3="http://127.0.0.1:8903"
FAUCET="http://127.0.0.1:9100"
CUSTODY="http://127.0.0.1:9105"

PASS=0
FAIL=0
TOTAL=0

# ── Helpers ──────────────────────────────────────────────────────────────────

rpc() {
    local url="$1"
    local method="$2"
    local params="${3:-[]}"
    curl -s -m 5 -X POST -H "Content-Type: application/json" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":$params}" \
        "$url" 2>/dev/null || echo '{"error":"curl_timeout"}'
}

solana_rpc() {
    rpc "$1/solana" "$2" "${3:-[]}"
}

evm_rpc() {
    rpc "$1/evm" "$2" "${3:-[]}"
}

dex_get() {
    curl -s -m 5 "$1" 2>/dev/null || echo '{"error":"curl_timeout"}'
}

# Extract .result from JSON-RPC response
jq_result() {
    python3 -c "
import sys,json
try:
    d=json.load(sys.stdin)
    r=d.get('result')
    if r is None: print('')
    elif isinstance(r,dict): print(json.dumps(r))
    elif isinstance(r,list): print(json.dumps(r))
    else: print(r)
except: print('')
" 2>/dev/null
}

# Extract a specific key from .result object
jq_key() {
    local key="$1"
    python3 -c "
import sys,json
try:
    d=json.load(sys.stdin)
    r=d.get('result',{})
    if isinstance(r,dict): print(r.get('$key',''))
    else: print('')
except: print('')
" 2>/dev/null
}

# Check if response has .result (not error)
has_result() {
    python3 -c "
import sys,json
try:
    d=json.load(sys.stdin)
    print('ok' if 'result' in d and d.get('error') is None else 'fail')
except: print('fail')
" 2>/dev/null
}

# Check if response has .error
has_error() {
    python3 -c "
import sys,json
try:
    d=json.load(sys.stdin)
    print('ok' if 'error' in d and d['error'] is not None else 'fail')
except: print('fail')
" 2>/dev/null
}

# Check if response is valid JSON-RPC (result or error)
is_valid_rpc() {
    python3 -c "
import sys,json
try:
    d=json.load(sys.stdin)
    print('ok' if 'result' in d or 'error' in d else 'fail')
except: print('fail')
" 2>/dev/null
}

section() {
    echo ""
    echo "▶ $1"
}

check() {
    local name="$1"
    local condition="$2"
    TOTAL=$((TOTAL + 1))
    if eval "$condition" 2>/dev/null; then
        echo "  ✅ $name"
        PASS=$((PASS + 1))
    else
        echo "  ❌ $name"
        FAIL=$((FAIL + 1))
    fi
}

echo "════════════════════════════════════════════════════════════════"
echo " MoltChain E2E Test Suite — 3-Validator Testnet"
echo "════════════════════════════════════════════════════════════════"

# ═════════════════════════════════════════════════════════════════════════════
# PART A: NATIVE MOLT RPC (/)
# ═════════════════════════════════════════════════════════════════════════════

# ─── 1. Connectivity ────────────────────────────────────────────
section "1. RPC Connectivity"

SLOT1=$(rpc "$RPC1" "getSlot" | jq_result)
SLOT2=$(rpc "$RPC2" "getSlot" | jq_result)
SLOT3=$(rpc "$RPC3" "getSlot" | jq_result)

check "V1 responding (slot=$SLOT1)" "[[ -n '$SLOT1' && $SLOT1 -gt 0 ]]"
check "V2 responding (slot=$SLOT2)" "[[ -n '$SLOT2' && $SLOT2 -gt 0 ]]"
check "V3 responding (slot=$SLOT3)" "[[ -n '$SLOT3' && $SLOT3 -gt 0 ]]"
check "All validators advancing" "[[ $SLOT1 -gt 0 && $SLOT2 -gt 0 && $SLOT3 -gt 0 ]]"

# ─── 2. Health ──────────────────────────────────────────────────
section "2. Health"

H1=$(rpc "$RPC1" "health" | jq_key "status")
H2=$(rpc "$RPC2" "health" | jq_key "status")
H3=$(rpc "$RPC3" "health" | jq_key "status")
check "V1 healthy (status=$H1)" "[[ '$H1' == 'ok' ]]"
check "V2 healthy (status=$H2)" "[[ '$H2' == 'ok' ]]"
check "V3 healthy (status=$H3)" "[[ '$H3' == 'ok' ]]"

# ─── 3. Block Production ───────────────────────────────────────
section "3. Block Production"

B1=$(rpc "$RPC1" "getBlock" "[1]" | has_result)
check "Block 1 exists" "[[ '$B1' == 'ok' ]]"

LB_SLOT=$(rpc "$RPC1" "getLatestBlock" | jq_key "slot")
check "getLatestBlock (slot=$LB_SLOT)" "[[ -n '$LB_SLOT' && $LB_SLOT -gt 0 ]]"

LB_HASH=$(rpc "$RPC1" "getLatestBlock" | jq_key "hash")
check "Latest block has hash (${LB_HASH:0:12}...)" "[[ ${#LB_HASH} -ge 16 ]]"

LB_VALIDATOR=$(rpc "$RPC1" "getLatestBlock" | jq_key "validator")
check "Latest block has validator pubkey" "[[ ${#LB_VALIDATOR} -ge 20 ]]"

SLOT_BEFORE=$SLOT1
sleep 6
SLOT_AFTER=$(rpc "$RPC1" "getSlot" | jq_result)
check "Blocks advancing ($SLOT_BEFORE → $SLOT_AFTER)" "[[ $SLOT_AFTER -gt $SLOT_BEFORE ]]"

# ─── 4. Chain Status ───────────────────────────────────────────
section "4. Chain Status"

CS_RAW=$(rpc "$RPC1" "getChainStatus" | jq_result)
cs_get() { echo "$CS_RAW" | python3 -c "import sys,json; print(json.loads(sys.stdin.read()).get('$1',''))" 2>/dev/null; }

CS_SLOT=$(cs_get slot)
CS_EPOCH=$(cs_get epoch)
CS_HEALTHY=$(cs_get is_healthy)
CS_CHAIN=$(cs_get chain_id)
CS_TXS=$(cs_get total_transactions)

check "Chain slot > 0 (=$CS_SLOT)" "[[ $CS_SLOT -gt 0 ]]"
check "Chain has epoch (=$CS_EPOCH)" "[[ '$CS_EPOCH' != '' ]]"
check "Chain is_healthy=True" "[[ '$CS_HEALTHY' == 'True' ]]"
check "Chain ID set (=$CS_CHAIN)" "[[ -n '$CS_CHAIN' ]]"
check "Total transactions >= 0 (=$CS_TXS)" "[[ '$CS_TXS' != '' ]]"

# ─── 5. Account & Balance ──────────────────────────────────────
section "5. Account & Balance"

NULL_ACCT=$(rpc "$RPC1" "getAccountInfo" '["11111111111111111111111111111111"]' | has_result)
check "getAccountInfo works" "[[ '$NULL_ACCT' == 'ok' ]]"

BAL=$(rpc "$RPC1" "getBalance" '["11111111111111111111111111111111"]' | has_result)
check "getBalance works" "[[ '$BAL' == 'ok' ]]"

BAL_SHELLS=$(rpc "$RPC1" "getBalance" '["11111111111111111111111111111111"]' | jq_key "shells")
check "Balance has shells field (=$BAL_SHELLS)" "[[ '$BAL_SHELLS' != '' ]]"

TX_CNT=$(rpc "$RPC1" "getAccountTxCount" '["11111111111111111111111111111111"]' | jq_key "count")
check "getAccountTxCount works (=$TX_CNT)" "[[ '$TX_CNT' != '' ]]"

TOKEN_ACCTS=$(rpc "$RPC1" "getTokenAccounts" '["11111111111111111111111111111111"]' | has_result)
check "getTokenAccounts works" "[[ '$TOKEN_ACCTS' == 'ok' ]]"

# ─── 6. Airdrop (Testnet Faucet) ──────────────────────────────
section "6. Airdrop (Testnet Faucet)"

AIRDROP_ERR=$(rpc "$RPC1" "requestAirdrop" '["11111111111111111111111111111111", 1000000000]' | has_error)
check "Rejects invalid amount (>100 MOLT)" "[[ '$AIRDROP_ERR' == 'ok' ]]"

AIRDROP_OK=$(rpc "$RPC1" "requestAirdrop" '["11111111111111111111111111111111", 10]' | jq_key "success")
check "Airdrop 10 MOLT succeeds" "[[ '$AIRDROP_OK' == 'True' ]]"

sleep 2
AIRDROP_BAL=$(rpc "$RPC1" "getBalance" '["11111111111111111111111111111111"]' | jq_key "shells")
check "Balance > 0 after airdrop (=$AIRDROP_BAL)" "[[ '$AIRDROP_BAL' != '' && $AIRDROP_BAL -gt 0 ]]"

# ─── 7. Blockhash & Mempool ───────────────────────────────────
section "7. Blockhash & Mempool"

BH=$(rpc "$RPC1" "getRecentBlockhash" | jq_key "blockhash")
check "getRecentBlockhash (${BH:0:8}...)" "[[ ${#BH} -ge 16 ]]"

RECENT=$(rpc "$RPC1" "getRecentTransactions" "[10]" | has_result)
check "getRecentTransactions works" "[[ '$RECENT' == 'ok' ]]"

# ─── 8. Validators & Staking ──────────────────────────────────
section "8. Validators & Staking"

VAL_CNT=$(rpc "$RPC1" "getValidators" | jq_key "count")
check "getValidators count >= 1 (=$VAL_CNT)" "[[ '$VAL_CNT' -ge 1 ]]"

REEFSTAKE=$(rpc "$RPC1" "getReefStakePoolInfo" | has_result)
check "getReefStakePoolInfo works" "[[ '$REEFSTAKE' == 'ok' ]]"

RS_RATE=$(rpc "$RPC1" "getReefStakePoolInfo" | jq_key "exchange_rate")
check "ReefStake exchange_rate set (=$RS_RATE)" "[[ -n '$RS_RATE' ]]"

# ─── 9. Economics ──────────────────────────────────────────────
section "9. Economics"

TREASURY=$(rpc "$RPC1" "getTreasuryInfo" | jq_key "treasury_balance")
check "Treasury balance > 0 (=$TREASURY)" "[[ '$TREASURY' != '' && $TREASURY -gt 0 ]]"

SUPPLY=$(rpc "$RPC1" "getMetrics" | jq_key "total_supply")
check "Total supply > 0 ($SUPPLY)" "[[ '$SUPPLY' != '' && $SUPPLY -gt 0 ]]"

CIRC=$(rpc "$RPC1" "getMetrics" | jq_key "circulating_supply")
check "Circulating supply > 0 ($CIRC)" "[[ '$CIRC' != '' && $CIRC -gt 0 ]]"

BURN=$(rpc "$RPC1" "getTotalBurned" | has_result)
check "getTotalBurned works" "[[ '$BURN' == 'ok' ]]"

BASE_FEE=$(rpc "$RPC1" "getFeeConfig" | jq_key "base_fee_shells")
check "Fee config: base_fee (=$BASE_FEE)" "[[ '$BASE_FEE' -gt 0 ]]"

RENT_KB=$(rpc "$RPC1" "getRentParams" | jq_key "rent_free_kb")
check "Rent params: rent_free_kb (=$RENT_KB)" "[[ '$RENT_KB' -ge 0 ]]"

REWARD=$(rpc "$RPC1" "getRewardAdjustmentInfo" | jq_key "transactionBlockReward")
check "Reward adjustment: blockReward (=$REWARD)" "[[ '$REWARD' -gt 0 ]]"

# ─── 10. Network & Peers ──────────────────────────────────────
section "10. Network & Peers"

NET_VER=$(rpc "$RPC1" "getNetworkInfo" | jq_key "version")
check "Network version (=$NET_VER)" "[[ -n '$NET_VER' ]]"

NET_CHAIN=$(rpc "$RPC1" "getNetworkInfo" | jq_key "chain_id")
check "Network chain_id (=$NET_CHAIN)" "[[ -n '$NET_CHAIN' ]]"

PEERS=$(rpc "$RPC1" "getPeers" | has_result)
check "getPeers works" "[[ '$PEERS' == 'ok' ]]"

# ─── 11. NFT Endpoints ────────────────────────────────────────
section "11. NFT Endpoints"

NFTS=$(rpc "$RPC1" "getNFTsByOwner" '["11111111111111111111111111111111"]' | has_result)
check "getNFTsByOwner works" "[[ '$NFTS' == 'ok' ]]"

LISTINGS=$(rpc "$RPC1" "getMarketListings" '["11111111111111111111111111111111"]' | has_result)
check "getMarketListings works" "[[ '$LISTINGS' == 'ok' ]]"

SALES=$(rpc "$RPC1" "getMarketSales" '["11111111111111111111111111111111"]' | has_result)
check "getMarketSales works" "[[ '$SALES' == 'ok' ]]"

# ─── 12. Contract & Program ───────────────────────────────────
section "12. Contracts & Programs"

CONTRACTS=$(rpc "$RPC1" "getAllContracts" | has_result)
check "getAllContracts works" "[[ '$CONTRACTS' == 'ok' ]]"

PROGS=$(rpc "$RPC1" "getPrograms" "[]" | has_result)
check "getPrograms works" "[[ '$PROGS' == 'ok' ]]"

# ─── 13. Symbol Registry ──────────────────────────────────────
section "13. Symbol Registry"

SYM=$(rpc "$RPC1" "getAllSymbolRegistry" "[]" | has_result)
check "getAllSymbolRegistry works" "[[ '$SYM' == 'ok' ]]"

# ═════════════════════════════════════════════════════════════════════════════
# PART B: SOLANA-COMPATIBLE RPC (/solana)
# ═════════════════════════════════════════════════════════════════════════════

# ─── 14. Solana Health & Version ───────────────────────────────
section "14. Solana-Compat: Health & Version"

SOL_H=$(solana_rpc "$RPC1" "getHealth" | jq_result)
check "getHealth = ok" "[[ '$SOL_H' == 'ok' ]]"

SOL_V=$(solana_rpc "$RPC1" "getVersion" | jq_key "solana-core")
check "getVersion = moltchain" "[[ '$SOL_V' == 'moltchain' ]]"

# ─── 15. Solana Slot & Block Height ───────────────────────────
section "15. Solana-Compat: Slot & Block"

SOL_SLOT=$(solana_rpc "$RPC1" "getSlot" | jq_result)
check "getSlot > 0 (=$SOL_SLOT)" "[[ '$SOL_SLOT' -gt 0 ]]"

SOL_HGT=$(solana_rpc "$RPC1" "getBlockHeight" | jq_result)
check "getBlockHeight > 0 (=$SOL_HGT)" "[[ '$SOL_HGT' -gt 0 ]]"

# ─── 16. Solana Blockhash ─────────────────────────────────────
section "16. Solana-Compat: Blockhash"

SOL_LBH=$(solana_rpc "$RPC1" "getLatestBlockhash" | jq_result | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); print(d.get('value',{}).get('blockhash',''))" 2>/dev/null)
check "getLatestBlockhash (${SOL_LBH:0:8}...)" "[[ ${#SOL_LBH} -ge 16 ]]"

SOL_RBH=$(solana_rpc "$RPC1" "getRecentBlockhash" | jq_result | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); print(d.get('value',{}).get('blockhash',''))" 2>/dev/null)
check "getRecentBlockhash alias (${SOL_RBH:0:8}...)" "[[ ${#SOL_RBH} -ge 16 ]]"

# ─── 17. Solana Balance & Account ─────────────────────────────
section "17. Solana-Compat: Balance & Account"

SOL_BAL=$(solana_rpc "$RPC1" "getBalance" '["11111111111111111111111111111111"]' | has_result)
check "getBalance works" "[[ '$SOL_BAL' == 'ok' ]]"

SOL_ACCT=$(solana_rpc "$RPC1" "getAccountInfo" '["11111111111111111111111111111111"]' | has_result)
check "getAccountInfo works" "[[ '$SOL_ACCT' == 'ok' ]]"

# ─── 18. Solana Block & Tx ────────────────────────────────────
section "18. Solana-Compat: Block & Transaction"

SOL_BLK=$(solana_rpc "$RPC1" "getBlock" "[1]" | has_result)
check "getBlock(1) works" "[[ '$SOL_BLK' == 'ok' ]]"

SOL_SEND=$(solana_rpc "$RPC1" "sendTransaction" '["invalid_base64"]' | has_error)
check "sendTransaction rejects bad input" "[[ '$SOL_SEND' == 'ok' ]]"

SOL_UNK=$(solana_rpc "$RPC1" "nonExistent" | has_error)
check "Unknown method returns error" "[[ '$SOL_UNK' == 'ok' ]]"

# ═════════════════════════════════════════════════════════════════════════════
# PART C: EVM-COMPATIBLE RPC (/evm)
# ═════════════════════════════════════════════════════════════════════════════

# ─── 19. EVM Chain & Block ─────────────────────────────────────
section "19. EVM-Compat: Chain & Block"

EVM_CHAIN=$(evm_rpc "$RPC1" "eth_chainId" | jq_result)
check "eth_chainId returns hex (=$EVM_CHAIN)" "[[ '$EVM_CHAIN' == 0x* ]]"

EVM_BLK=$(evm_rpc "$RPC1" "eth_blockNumber" | jq_result)
check "eth_blockNumber returns hex (=$EVM_BLK)" "[[ '$EVM_BLK' == 0x* ]]"

EVM_NET=$(evm_rpc "$RPC1" "net_version" | jq_result)
check "net_version returns ID (=$EVM_NET)" "[[ -n '$EVM_NET' ]]"

# ─── 20. EVM Accounts & Balance ───────────────────────────────
section "20. EVM-Compat: Accounts & Balance"

EVM_ACCTS=$(evm_rpc "$RPC1" "eth_accounts" | jq_result)
check "eth_accounts returns array" "[[ '$EVM_ACCTS' == '[]' ]]"

EVM_BAL=$(evm_rpc "$RPC1" "eth_getBalance" '["0x0000000000000000000000000000000000000001","latest"]' | has_result)
check "eth_getBalance works" "[[ '$EVM_BAL' == 'ok' ]]"

# ─── 21. EVM Errors ───────────────────────────────────────────
section "21. EVM-Compat: Errors"

EVM_UNK=$(evm_rpc "$RPC1" "eth_nonexistent" | has_error)
check "Unknown EVM method returns error" "[[ '$EVM_UNK' == 'ok' ]]"

# ═════════════════════════════════════════════════════════════════════════════
# PART D: DEX REST API (/api/v1)
# ═════════════════════════════════════════════════════════════════════════════

# ─── 22. DEX Market Data ──────────────────────────────────────
section "22. DEX REST API"

dex_ok() {
    python3 -c "import sys,json; d=json.loads(sys.stdin.read()); print('ok' if d.get('success') else 'fail')" 2>/dev/null
}

DEX_P=$(dex_get "$RPC1/api/v1/pairs" | dex_ok)
check "/pairs returns success" "[[ '$DEX_P' == 'ok' ]]"

DEX_T=$(dex_get "$RPC1/api/v1/tickers" | dex_ok)
check "/tickers returns success" "[[ '$DEX_T' == 'ok' ]]"

DEX_PL=$(dex_get "$RPC1/api/v1/pools" | dex_ok)
check "/pools returns success" "[[ '$DEX_PL' == 'ok' ]]"

# ═════════════════════════════════════════════════════════════════════════════
# PART E: MULTI-VALIDATOR CONSISTENCY
# ═════════════════════════════════════════════════════════════════════════════

# ─── 23. Consistency ──────────────────────────────────────────
section "23. Multi-Validator Consistency"

# Health
MH1=$(rpc "$RPC1" "health" | jq_key "status")
MH2=$(rpc "$RPC2" "health" | jq_key "status")
MH3=$(rpc "$RPC3" "health" | jq_key "status")
check "All healthy" "[[ '$MH1' == 'ok' && '$MH2' == 'ok' && '$MH3' == 'ok' ]]"

# Slots
MS1=$(rpc "$RPC1" "getSlot" | jq_result)
MS2=$(rpc "$RPC2" "getSlot" | jq_result)
MS3=$(rpc "$RPC3" "getSlot" | jq_result)
check "All slots > 0 (V1=$MS1 V2=$MS2 V3=$MS3)" "[[ $MS1 -gt 0 && $MS2 -gt 0 && $MS3 -gt 0 ]]"

# Chain IDs
MC1=$(rpc "$RPC1" "getNetworkInfo" | jq_key "chain_id")
MC2=$(rpc "$RPC2" "getNetworkInfo" | jq_key "chain_id")
MC3=$(rpc "$RPC3" "getNetworkInfo" | jq_key "chain_id")
check "Same chain_id" "[[ '$MC1' == '$MC2' && '$MC2' == '$MC3' ]]"

# Supply
MSP1=$(rpc "$RPC1" "getMetrics" | jq_key "total_supply")
MSP2=$(rpc "$RPC2" "getMetrics" | jq_key "total_supply")
MSP3=$(rpc "$RPC3" "getMetrics" | jq_key "total_supply")
check "Supply consistent ($MSP1)" "[[ '$MSP1' == '$MSP2' && '$MSP2' == '$MSP3' ]]"

# Solana health on all 3
SH1=$(solana_rpc "$RPC1" "getHealth" | jq_result)
SH2=$(solana_rpc "$RPC2" "getHealth" | jq_result)
SH3=$(solana_rpc "$RPC3" "getHealth" | jq_result)
check "Solana health all ok" "[[ '$SH1' == 'ok' && '$SH2' == 'ok' && '$SH3' == 'ok' ]]"

# EVM chain IDs
EC1=$(evm_rpc "$RPC1" "eth_chainId" | jq_result)
EC2=$(evm_rpc "$RPC2" "eth_chainId" | jq_result)
EC3=$(evm_rpc "$RPC3" "eth_chainId" | jq_result)
check "EVM chain_id consistent" "[[ '$EC1' == '$EC2' && '$EC2' == '$EC3' ]]"

# ═════════════════════════════════════════════════════════════════════════════
# PART F: ERROR HANDLING
# ═════════════════════════════════════════════════════════════════════════════

# ─── 24. Error Handling ───────────────────────────────────────
section "24. Error Handling"

ERR1=$(rpc "$RPC1" "nonExistentMethod" | has_error)
check "Invalid Molt method → error" "[[ '$ERR1' == 'ok' ]]"

ERR2=$(rpc "$RPC1" "getBalance" '["not-a-valid-pubkey"]' | is_valid_rpc)
check "Bad pubkey → valid RPC response" "[[ '$ERR2' == 'ok' ]]"

ERR3=$(rpc "$RPC1" "getBlock" "[999999999]" | is_valid_rpc)
check "Non-existent block → valid RPC response" "[[ '$ERR3' == 'ok' ]]"

ERR4=$(rpc "$RPC1" "getBalance" "[]" | is_valid_rpc)
check "Missing params → valid RPC response" "[[ '$ERR4' == 'ok' ]]"

ERR5=$(solana_rpc "$RPC1" "getBlock" "[999999999]" | is_valid_rpc)
check "Solana non-existent block → valid" "[[ '$ERR5' == 'ok' ]]"

# ─── 25. Metrics ──────────────────────────────────────────────
section "25. Metrics & Performance"

M_RAW=$(rpc "$RPC1" "getMetrics" | jq_result)
m_get() { echo "$M_RAW" | python3 -c "import sys,json; print(json.loads(sys.stdin.read()).get('$1',''))" 2>/dev/null; }

M_TPS=$(m_get tps)
M_BLKS=$(m_get total_blocks)
M_ACTS=$(m_get total_accounts)
M_ABT=$(m_get average_block_time)

check "TPS available (=$M_TPS)" "[[ -n '$M_TPS' ]]"
check "total_blocks > 0 (=$M_BLKS)" "[[ $M_BLKS -gt 0 ]]"
check "total_accounts > 0 (=$M_ACTS)" "[[ $M_ACTS -gt 0 ]]"
check "avg_block_time > 0 (=$M_ABT)" "[[ $(echo '$M_ABT > 0' | bc 2>/dev/null || echo 1) -eq 1 ]]"

# ═════════════════════════════════════════════════════════════════════════════
# PART G: FAUCET SERVICE (standalone HTTP)
# ═════════════════════════════════════════════════════════════════════════════

# ─── 26. Faucet Service ───────────────────────────────────────
section "26. Faucet Service"

FAUCET_HEALTH=$(curl -s -m 3 "$FAUCET/health" 2>/dev/null || echo "")
check "Faucet /health returns OK" "[[ '$FAUCET_HEALTH' == 'OK' ]]"

FAUCET_AIRDROP=$(curl -s -m 5 -X POST -H "Content-Type: application/json" \
    -d '{"address":"11111111111111111111111111111111","amount":5}' \
    "$FAUCET/faucet/request" 2>/dev/null | python3 -c "
import sys,json
try:
    d=json.load(sys.stdin); print('ok' if d.get('success') else 'fail')
except: print('fail')
" 2>/dev/null)
check "Faucet airdrop 5 MOLT succeeds" "[[ '$FAUCET_AIRDROP' == 'ok' ]]"

FAUCET_BAD_ADDR=$(curl -s -m 5 -X POST -H "Content-Type: application/json" \
    -d '{"address":"invalid","amount":5}' \
    "$FAUCET/faucet/request" 2>/dev/null | python3 -c "
import sys,json
try:
    d=json.load(sys.stdin); print('ok' if d.get('error') else 'fail')
except: print('fail')
" 2>/dev/null)
check "Faucet rejects invalid address" "[[ '$FAUCET_BAD_ADDR' == 'ok' ]]"

FAUCET_TOO_MUCH=$(curl -s -m 5 -X POST -H "Content-Type: application/json" \
    -d '{"address":"11111111111111111111111111111111","amount":999}' \
    "$FAUCET/faucet/request" 2>/dev/null | python3 -c "
import sys,json
try:
    d=json.load(sys.stdin); print('ok' if d.get('error') else 'fail')
except: print('fail')
" 2>/dev/null)
check "Faucet rejects excess amount" "[[ '$FAUCET_TOO_MUCH' == 'ok' ]]"

FAUCET_LIST=$(curl -s -m 3 "$FAUCET/faucet/airdrops" 2>/dev/null | python3 -c "
import sys,json
try:
    d=json.load(sys.stdin); print('ok' if isinstance(d, list) else 'fail')
except: print('fail')
" 2>/dev/null)
check "Faucet /airdrops returns list" "[[ '$FAUCET_LIST' == 'ok' ]]"

FAUCET_NOT_FOUND=$(curl -s -m 3 -o /dev/null -w '%{http_code}' "$FAUCET/faucet/airdrop/doesntexist" 2>/dev/null)
check "Faucet /airdrop/:sig 404 for missing" "[[ '$FAUCET_NOT_FOUND' == '404' ]]"

# ═════════════════════════════════════════════════════════════════════════════
# PART H: CUSTODY BRIDGE SERVICE (standalone HTTP)
# ═════════════════════════════════════════════════════════════════════════════

# ─── 27. Custody Service ──────────────────────────────────────
section "27. Custody Bridge Service"

CUST_HEALTH=$(curl -s -m 3 "$CUSTODY/health" 2>/dev/null | python3 -c "
import sys,json
try:
    d=json.load(sys.stdin); print(d.get('status',''))
except: print('')
" 2>/dev/null)
check "Custody /health = ok" "[[ '$CUST_HEALTH' == 'ok' ]]"

CUST_STATUS=$(curl -s -m 3 "$CUSTODY/status" 2>/dev/null | python3 -c "
import sys,json
try:
    d=json.load(sys.stdin); print('ok' if 'signers' in d and 'sweeps' in d else 'fail')
except: print('fail')
" 2>/dev/null)
check "Custody /status has signers + sweeps" "[[ '$CUST_STATUS' == 'ok' ]]"

CUST_RESERVES=$(curl -s -m 3 "$CUSTODY/reserves" 2>/dev/null | python3 -c "
import sys,json
try:
    d=json.load(sys.stdin); print('ok' if 'reserves' in d else 'fail')
except: print('fail')
" 2>/dev/null)
check "Custody /reserves works" "[[ '$CUST_RESERVES' == 'ok' ]]"

# Create EVM deposit (doesn't require Solana RPC)
CUST_DEPOSIT=$(curl -s -m 5 -X POST -H "Content-Type: application/json" \
    -d '{"user_id":"e2e_test","chain":"ethereum","asset":"usdt"}' \
    "$CUSTODY/deposits" 2>/dev/null | python3 -c "
import sys,json
try:
    d=json.load(sys.stdin); print(d.get('deposit_id',''))
except: print('')
" 2>/dev/null)
check "Custody create EVM deposit (id=${CUST_DEPOSIT:0:8}...)" "[[ -n '$CUST_DEPOSIT' && ${#CUST_DEPOSIT} -ge 8 ]]"

# Lookup deposit
if [[ -n "$CUST_DEPOSIT" && ${#CUST_DEPOSIT} -ge 8 ]]; then
    CUST_LOOKUP=$(curl -s -m 3 "$CUSTODY/deposits/$CUST_DEPOSIT" 2>/dev/null | python3 -c "
import sys,json
try:
    d=json.load(sys.stdin); print(d.get('status',''))
except: print('')
" 2>/dev/null)
    check "Custody deposit lookup (status=$CUST_LOOKUP)" "[[ '$CUST_LOOKUP' == 'issued' ]]"
else
    check "Custody deposit lookup (skipped — no deposit)" "false"
fi

# Withdrawal insufficient reserves
CUST_WITHDRAW=$(curl -s -m 5 -X POST -H "Content-Type: application/json" \
    -d '{"user_id":"e2e_test","asset":"mUSD","amount":100000,"dest_chain":"ethereum","dest_address":"0x0000000000000000000000000000000000000001"}' \
    "$CUSTODY/withdrawals" 2>/dev/null | python3 -c "
import sys,json
try:
    d=json.load(sys.stdin); print('ok' if 'error' in d and 'insufficient' in d.get('error','') else 'fail')
except: print('fail')
" 2>/dev/null)
check "Custody withdrawal rejected (insufficient reserves)" "[[ '$CUST_WITHDRAW' == 'ok' ]]"

# ─────────────────────────────────────────────────────────────────
# SUMMARY
# ─────────────────────────────────────────────────────────────────
echo ""
echo "════════════════════════════════════════════════════════════════"
printf " E2E RESULTS: %d/%d passed, %d failed\n" "$PASS" "$TOTAL" "$FAIL"
echo "════════════════════════════════════════════════════════════════"

if [[ $FAIL -eq 0 ]]; then
    echo " 🎉 ALL TESTS PASSED"
    exit 0
else
    echo " ⚠️  $FAIL tests failed — review above"
    exit 1
fi
