# MoltChain Autonomous Agent Skillbook (`skill.md`)

This file is the **root, no-UI, autonomous operations skill** for agents on MoltChain.
It is written so an agent can execute workflows end-to-end from terminal/API only.

---

## 0) Mission + Mode

- Goal: give an agent everything needed to operate on MoltChain without touching UI.
- Control planes: `molt` CLI, JSON-RPC, WebSocket subscriptions, SDKs, scripts.
- Scope: validator operation, wallet/identity, token lifecycle, governance, DeFi/NFT/market actions, contract ABI-driven calls, and real-time monitoring via RPC/WS/API.

---

## 1) Environment Bootstrap (mandatory, binary-first)

> Distribution model: agents receive prebuilt binaries + official docs URLs. Full repo clone is optional and not required for runtime autonomy.

## 1.0 Official URL registry (fill with canonical links)

```bash
export MOLT_OFFICIAL_VALIDATOR_URL="<OFFICIAL_VALIDATOR_GUIDE_URL>"
export MOLT_OFFICIAL_RPC_URL="<OFFICIAL_RPC_REFERENCE_URL>"
export MOLT_OFFICIAL_CLI_URL="<OFFICIAL_CLI_REFERENCE_URL>"
export MOLT_OFFICIAL_SDK_JS_URL="<OFFICIAL_SDK_JS_URL>"
export MOLT_OFFICIAL_SDK_PY_URL="<OFFICIAL_SDK_PY_URL>"
export MOLT_OFFICIAL_SDK_RS_URL="<OFFICIAL_SDK_RS_URL>"
```

Agents should treat these as source-of-truth docs and keep command signatures aligned.

## 1.1 Install prebuilt binaries

Set official artifact URLs (replace placeholders once published):

```bash
export MOLT_RELEASE_BASE_URL="<OFFICIAL_RELEASE_BASE_URL>"
export MOLT_BIN_URL="$MOLT_RELEASE_BASE_URL/molt"
export VALIDATOR_BIN_URL="$MOLT_RELEASE_BASE_URL/moltchain-validator"
```

Install locally:

```bash
mkdir -p "$HOME/.moltchain/bin"
curl -fsSL "$MOLT_BIN_URL" -o "$HOME/.moltchain/bin/molt"
curl -fsSL "$VALIDATOR_BIN_URL" -o "$HOME/.moltchain/bin/moltchain-validator"
chmod +x "$HOME/.moltchain/bin/"*
export PATH="$HOME/.moltchain/bin:$PATH"
```

Expected commands:

- `molt`
- `moltchain-validator`

## 1.2 Set runtime env

```bash
export MOLT_BIN="$(command -v molt)"
export VALIDATOR_BIN="$(command -v moltchain-validator)"
export RPC_URL="http://localhost:8899"
export WS_URL="ws://localhost:8900"
export API_URL="${RPC_URL}/api/v1"
export DEPOSIT_API_URL="http://localhost:9105"
```

## 1.3 Health check

```bash
curl -sS -X POST "$RPC_URL" \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}' | jq
```

## 1.4 Validator bring-up (agent runtime)

Single validator start (local/default ports):

```bash
$VALIDATOR_BIN \
  --network testnet \
  --rpc-port 8899 \
  --ws-port 8900 \
  --p2p-port 7001 \
  --db-path ./data/state-testnet-7001
```

Join existing network:

```bash
$VALIDATOR_BIN \
  --network testnet \
  --rpc-port 8899 \
  --ws-port 8900 \
  --p2p-port 7001 \
  --db-path ./data/state-testnet-7001 \
  --bootstrap-peers <HOST:PORT>
```

Post-start checks:

```bash
curl -sS -X POST "$RPC_URL" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}' | jq

$MOLT_BIN --rpc-url "$RPC_URL" validators
```

---

## 2) CLI Command Surface (authoritative)

Top-level commands (`molt --help`):

- `identity`
- `wallet`
- `init`
- `generate-keypair` (deprecated)
- `pubkey` (deprecated)
- `balance`
- `transfer`
- `airdrop`
- `deploy`
- `call`
- `block`
- `latest`
- `slot`
- `blockhash`
- `burned`
- `validators`
- `network`
- `validator`
- `stake`
- `account`
- `contract`
- `status`
- `metrics`
- `token`
- `gov`

Global option:

- `--rpc-url <URL>`

---

## 3) Autonomous Quickstart Flows

## 3.1 Identity + wallet creation/access

```bash
# Identity
$MOLT_BIN --rpc-url "$RPC_URL" identity new
$MOLT_BIN --rpc-url "$RPC_URL" identity show

# Wallets
$MOLT_BIN --rpc-url "$RPC_URL" wallet create agent-main
$MOLT_BIN --rpc-url "$RPC_URL" wallet list
$MOLT_BIN --rpc-url "$RPC_URL" wallet show agent-main
$MOLT_BIN --rpc-url "$RPC_URL" wallet balance agent-main
```

## 3.2 Fund + transfer

```bash
# Airdrop to default identity
$MOLT_BIN --rpc-url "$RPC_URL" airdrop 100

# Airdrop to explicit address
$MOLT_BIN --rpc-url "$RPC_URL" airdrop 100 --pubkey <BASE58_ADDR>

# Native transfer
$MOLT_BIN --rpc-url "$RPC_URL" transfer <TO_BASE58> 1.25
```

## 3.3 Token lifecycle (create → mint → send → inspect)

```bash
$MOLT_BIN --rpc-url "$RPC_URL" token create "Agent Token" AGT --supply 1000000 --decimals 9
$MOLT_BIN --rpc-url "$RPC_URL" token list
$MOLT_BIN --rpc-url "$RPC_URL" token info <TOKEN_ADDR_OR_SYMBOL>
$MOLT_BIN --rpc-url "$RPC_URL" token mint <TOKEN_ADDR> 1000 --to <RECIPIENT_BASE58>
$MOLT_BIN --rpc-url "$RPC_URL" token send <TOKEN_ADDR> <RECIPIENT_BASE58> 10
$MOLT_BIN --rpc-url "$RPC_URL" token balance <TOKEN_ADDR> --address <BASE58_ADDR>
```

## 3.4 Contract deploy + execute

```bash
# Deploy WASM contract
$MOLT_BIN --rpc-url "$RPC_URL" deploy ./target/wasm32-unknown-unknown/release/my_contract.wasm

# Call function (JSON args array)
$MOLT_BIN --rpc-url "$RPC_URL" call <CONTRACT_ADDR> initialize --args '["arg1", 123]'
$MOLT_BIN --rpc-url "$RPC_URL" call <CONTRACT_ADDR> get_value --args '[]'
```

## 3.5 Governance lifecycle

```bash
$MOLT_BIN --rpc-url "$RPC_URL" gov propose "Title" "Description" --proposal-type standard
$MOLT_BIN --rpc-url "$RPC_URL" gov list
$MOLT_BIN --rpc-url "$RPC_URL" gov info <PROPOSAL_ID>
$MOLT_BIN --rpc-url "$RPC_URL" gov vote <PROPOSAL_ID> yes
$MOLT_BIN --rpc-url "$RPC_URL" gov execute <PROPOSAL_ID>
$MOLT_BIN --rpc-url "$RPC_URL" gov veto <PROPOSAL_ID>
```

## 3.6 Validator + staking

```bash
$MOLT_BIN --rpc-url "$RPC_URL" validators
$MOLT_BIN --rpc-url "$RPC_URL" validator list
$MOLT_BIN --rpc-url "$RPC_URL" validator info <VALIDATOR_BASE58>
$MOLT_BIN --rpc-url "$RPC_URL" validator performance <VALIDATOR_BASE58>

$MOLT_BIN --rpc-url "$RPC_URL" stake add <AMOUNT_SHELLS>
$MOLT_BIN --rpc-url "$RPC_URL" stake status
$MOLT_BIN --rpc-url "$RPC_URL" stake rewards
$MOLT_BIN --rpc-url "$RPC_URL" stake remove <AMOUNT_SHELLS>
```

## 3.7 Wallet deposit address lifecycle (symbol + network)

This is the exact wallet deposit flow for bridged assets: request unique deposit address, send funds on source chain, poll status until `credited`.

Supported source chain values:

- `solana` (or `sol`)
- `ethereum`

Common source asset values:

- `usdc`
- `usdt`

Get your MoltChain wallet address (used as `user_id`):

```bash
WALLET_ADDR="$($MOLT_BIN --rpc-url "$RPC_URL" wallet show agent-main | awk '/Address:/ {print $2; exit}')"
echo "$WALLET_ADDR"
```

Request a deposit address (example: USDC on Solana):

```bash
DEPOSIT_RESPONSE=$(curl -sS -X POST "$DEPOSIT_API_URL/deposits" \
  -H 'Content-Type: application/json' \
  -d "{\"user_id\":\"$WALLET_ADDR\",\"chain\":\"solana\",\"asset\":\"usdc\"}")

DEPOSIT_ID=$(echo "$DEPOSIT_RESPONSE" | jq -r '.deposit_id')
DEPOSIT_ADDRESS=$(echo "$DEPOSIT_RESPONSE" | jq -r '.address')

echo "deposit_id=$DEPOSIT_ID"
echo "deposit_address=$DEPOSIT_ADDRESS"
```

Poll status (wait for `credited`):

```bash
while true; do
  STATUS=$(curl -sS "$DEPOSIT_API_URL/deposits/$DEPOSIT_ID" | jq -r '.status')
  echo "status=$STATUS"
  if [ "$STATUS" = "credited" ] || [ "$STATUS" = "expired" ]; then
    break
  fi
  sleep 5
done
```

After `credited`, verify on-chain balance:

```bash
$MOLT_BIN --rpc-url "$RPC_URL" balance
```

---

## 4) MoltyID Autonomous Operations

MoltyID is a contract interaction surface; use `molt call` + ABI/RPC introspection.

## 4.1 Find MoltyID contract address

```bash
curl -sS -X POST "$RPC_URL" \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getAllContracts","params":[]}' \
| jq -r '.result.contracts[] | select((tostring|ascii_downcase|contains("moltyid"))) | (.program_id // .address // .id)'
```

## 4.2 Inspect ABI before calling

```bash
curl -sS -X POST "$RPC_URL" \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getContractAbi","params":["<MOLTYID_ADDR>"]}' | jq
```

## 4.3 Core MoltyID actions

- `register_identity`
- `set_endpoint`
- `set_metadata`
- `set_availability`
- `set_rate`
- `add_skill`
- `vouch`
- `set_delegate`
- `revoke_delegate`
- `get_agent_profile`

Example pattern:

```bash
$MOLT_BIN --rpc-url "$RPC_URL" call <MOLTYID_ADDR> register_identity --args '["<OWNER>",1,"agentname",9]'
$MOLT_BIN --rpc-url "$RPC_URL" call <MOLTYID_ADDR> set_endpoint --args '["<OWNER>","https://agent.endpoint",22]'
$MOLT_BIN --rpc-url "$RPC_URL" call <MOLTYID_ADDR> add_skill --args '["<OWNER>","rust",4]'
```

## 4.4 Name actions (`.molt`) when ABI exposes them

From portal docs, name APIs include:

- `register_name`
- `resolve_name`
- `reverse_resolve`
- `transfer_name`
- `renew_name`
- `release_name`

Always verify exact signature from `getContractAbi` before invoking.

---

## 5) JSON-RPC Method Catalog (source-verified from `rpc/src/lib.rs`)

Primary endpoint:

- `POST http://localhost:8899/`

Compatibility endpoints:

- `POST http://localhost:8899/solana`
- `POST http://localhost:8899/evm`

## 5.1 Canonical Molt RPC (`/`)

Chain/core:

- `health`
- `getSlot`
- `getLatestBlock`
- `getRecentBlockhash`
- `getMetrics`
- `getChainStatus`
- `getTotalBurned`

Account/tx/history:

- `getBalance`
- `getAccount`
- `getAccountInfo`
- `getAccountTxCount`
- `getBlock`
- `getTransaction`
- `getTransactionsByAddress`
- `getRecentTransactions`
- `getTransactionHistory`
- `getTokenAccounts`
- `sendTransaction`
- `confirmTransaction`
- `simulateTransaction`

Network/validator:

- `getValidators`
- `getValidatorInfo`
- `getValidatorPerformance`
- `getPeers`
- `getNetworkInfo`
- `getClusterInfo`

Fee/rent/treasury/genesis:

- `getFeeConfig`
- `setFeeConfig`
- `getRentParams`
- `setRentParams`
- `getTreasuryInfo`
- `getGenesisAccounts`

Staking + ReefStake:

- `stake`
- `unstake`
- `getStakingStatus`
- `getStakingRewards`
- `stakeToReefStake`
- `unstakeFromReefStake`
- `claimUnstakedTokens`
- `getStakingPosition`
- `getReefStakePoolInfo`
- `getUnstakingQueue`
- `getRewardAdjustmentInfo`

Contracts/programs:

- `getContractInfo`
- `getContractLogs`
- `getContractAbi`
- `setContractAbi`
- `getAllContracts`
- `deployContract`
- `upgradeContract`
- `getContractEvents`
- `getProgram`
- `getPrograms`
- `getProgramStats`
- `getProgramCalls`
- `getProgramStorage`

MoltyID + names + identity directories:

- `getMoltyIdIdentity`
- `getMoltyIdReputation`
- `getMoltyIdSkills`
- `getMoltyIdVouches`
- `getMoltyIdAchievements`
- `getMoltyIdProfile`
- `resolveMoltName`
- `reverseMoltName`
- `batchReverseMoltNames`
- `searchMoltNames`
- `getMoltyIdAgentDirectory`
- `getMoltyIdStats`
- `getEvmRegistration`
- `lookupEvmAddress`

Registry/token/NFT/market:

- `getSymbolRegistry`
- `getSymbolRegistryByProgram`
- `getAllSymbolRegistry`
- `getTokenBalance`
- `getTokenHolders`
- `getTokenTransfers`
- `getCollection`
- `getNFT`
- `getNFTsByOwner`
- `getNFTsByCollection`
- `getNFTActivity`
- `getMarketListings`
- `getMarketSales`

Prediction + platform stats:

- `getPredictionMarketStats`
- `getPredictionMarkets`
- `getPredictionMarket`
- `getPredictionPositions`
- `getPredictionTraderStats`
- `getPredictionLeaderboard`
- `getPredictionTrending`
- `getPredictionMarketAnalytics`
- `getDexCoreStats`
- `getDexAmmStats`
- `getDexMarginStats`
- `getDexRewardsStats`
- `getDexRouterStats`
- `getDexAnalyticsStats`
- `getDexGovernanceStats`
- `getMoltswapStats`
- `getLobsterLendStats`
- `getClawPayStats`
- `getBountyBoardStats`
- `getComputeMarketStats`
- `getReefStorageStats`
- `getMoltMarketStats`
- `getMoltAuctionStats`
- `getMoltPunksStats`

Testnet utility:

- `requestAirdrop`

## 5.2 Solana-compatible RPC (`/solana`)

- `getLatestBlockhash`
- `getRecentBlockhash`
- `getBalance`
- `getAccountInfo`
- `getBlock`
- `getBlockHeight`
- `getSignaturesForAddress`
- `getSignatureStatuses`
- `getSlot`
- `getTransaction`
- `sendTransaction`
- `getHealth`
- `getVersion`

## 5.3 EVM-compatible RPC (`/evm`)

- `eth_getBalance`
- `eth_sendRawTransaction`
- `eth_call`
- `eth_chainId`
- `eth_blockNumber`
- `eth_getTransactionReceipt`
- `eth_getTransactionByHash`
- `eth_accounts`
- `net_version`

---

## 6) WebSocket Subscriptions (source-verified from `rpc/src/ws.rs` + `rpc/src/dex_ws.rs`)

Endpoint:

- `ws://localhost:8900`

Core subscriptions:

- `subscribeSlots` / `unsubscribeSlots` (aliases: `slotSubscribe`, `slotUnsubscribe`)
- `subscribeBlocks` / `unsubscribeBlocks`
- `subscribeTransactions` / `unsubscribeTransactions`
- `subscribeAccount` / `unsubscribeAccount`
- `subscribeLogs` / `unsubscribeLogs`
- `subscribeProgramUpdates` / `unsubscribeProgramUpdates`
- `subscribeProgramCalls` / `unsubscribeProgramCalls`
- `subscribeNftMints` / `unsubscribeNftMints`
- `subscribeNftTransfers` / `unsubscribeNftTransfers`
- `subscribeMarketListings` / `unsubscribeMarketListings`
- `subscribeMarketSales` / `unsubscribeMarketSales`
- `subscribeBridgeLocks` / `unsubscribeBridgeLocks`
- `subscribeBridgeMints` / `unsubscribeBridgeMints`

Newer subscriptions + aliases:

- `subscribeSignatureStatus` / `unsubscribeSignatureStatus` (aliases: `signatureSubscribe`, `signatureUnsubscribe`)
- `subscribeValidators` / `unsubscribeValidators` (aliases: `validatorSubscribe`, `validatorUnsubscribe`)
- `subscribeTokenBalance` / `unsubscribeTokenBalance` (aliases: `tokenBalanceSubscribe`, `tokenBalanceUnsubscribe`)
- `subscribeEpochs` / `unsubscribeEpochs` (aliases: `epochSubscribe`, `epochUnsubscribe`)
- `subscribeGovernance` / `unsubscribeGovernance` (aliases: `governanceSubscribe`, `governanceUnsubscribe`)

DEX stream multiplexing:

- `subscribeDex` / `unsubscribeDex`
- DEX channel formats:
  - `orderbook:<pair_id>`
  - `trades:<pair_id>`
  - `ticker:<pair_id>`
  - `candles:<pair_id>:<interval>`
  - `orders:<trader_addr>`
  - `positions:<trader_addr>`

Prediction stream multiplexing:

- `subscribePrediction` / `unsubscribePrediction`
- `subscribePredictionMarket` / `unsubscribePredictionMarket`
- Prediction channel formats:
  - `all` or `markets` (all markets)
  - `market:<market_id>`
  - `<market_id>`

Use WS streams for event-driven bots, alerting, copy-trading triggers, and execution monitoring.

---

## 7) Full Contract Interaction Sweep (source-verified from `developers/contract-reference.html`)

Always do ABI-first before any write:

```bash
CONTRACT_ID=<CONTRACT_ADDR>

curl -sS -X POST "$RPC_URL" -H 'Content-Type: application/json' \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getContractAbi\",\"params\":[\"$CONTRACT_ID\"]}" | jq
```

Read/write execution pattern (all contracts):

```bash
# Write (state change)
$MOLT_BIN --rpc-url "$RPC_URL" call <CONTRACT_ADDR> <FUNCTION_NAME> --args '<JSON_ARRAY_ARGS>'

# Read (view-style call)
$MOLT_BIN --rpc-url "$RPC_URL" call <CONTRACT_ADDR> <FUNCTION_NAME> --args '<JSON_ARRAY_ARGS>'

# Verify post-state
curl -sS -X POST "$RPC_URL" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getProgramCalls","params":["<CONTRACT_ADDR>"]}' | jq
```

Autonomous trading examples:

```bash
# DEX Core: place limit order
$MOLT_BIN --rpc-url "$RPC_URL" call <DEX_CORE_ADDR> place_order --args '["<TRADER>",1,0,0,1000000000,10000,0]'

# DEX AMM: exact-in swap
$MOLT_BIN --rpc-url "$RPC_URL" call <DEX_AMM_ADDR> swap_exact_in --args '["<TRADER>",1,true,1000,0,0]'

# DEX Margin: open leveraged position
$MOLT_BIN --rpc-url "$RPC_URL" call <DEX_MARGIN_ADDR> open_margin_position --args '["<TRADER>",1,0,1000000000,2,300000000]'

# Prediction Market: buy shares
$MOLT_BIN --rpc-url "$RPC_URL" call <PREDICTION_MARKET_ADDR> buy_shares --args '["<TRADER>",<MARKET_ID>,<OUTCOME>,<AMOUNT>]'
```

Canonical deployed contract surfaces:

- `moltcoin`: `initialize`, `balance_of`, `transfer`, `mint`, `burn`, `approve`, `total_supply`
- `moltswap`: `initialize`, `add_liquidity`, `remove_liquidity`, `swap_a_for_b`, `swap_b_for_a`, `get_quote`, `get_reserves`, `get_liquidity_balance`, `get_total_liquidity`, `flash_loan_borrow`, `flash_loan_repay`, `flash_loan_abort`, `get_flash_loan_fee`, `set_identity_admin`, `set_moltyid_address`, `set_reputation_discount`
- `lobsterlend`: `initialize`, `deposit`, `withdraw`, `borrow`, `repay`, `liquidate`, `get_account_info`, `get_protocol_stats`
- `clawpump`: `initialize`, `create_token`, `buy`, `sell`, `get_token_info`, `get_buy_quote`, `get_token_count`, `get_platform_stats`
- `clawpay`: `create_stream`, `withdraw_from_stream`, `cancel_stream`, `get_stream`, `get_withdrawable`, `set_identity_admin`, `set_moltyid_address`, `set_identity_gate`
- `clawvault`: `initialize`, `add_strategy`, `deposit`, `withdraw`, `harvest`, `get_vault_stats`, `get_user_position`, `get_strategy_info`
- `moltyid` (33): `initialize`, `register_identity`, `get_identity`, `update_reputation`, `update_reputation_typed`, `add_skill`, `get_skills`, `vouch`, `get_reputation`, `deactivate_identity`, `get_identity_count`, `update_agent_type`, `get_vouches`, `award_contribution_achievement`, `get_achievements`, `attest_skill`, `get_attestations`, `revoke_attestation`, `register_name`, `resolve_name`, `reverse_resolve`, `transfer_name`, `renew_name`, `release_name`, `set_endpoint`, `get_endpoint`, `set_metadata`, `get_metadata`, `set_availability`, `get_availability`, `set_rate`, `get_rate`, `get_agent_profile`
- `moltdao`: `initialize_dao`, `create_proposal`, `create_proposal_typed`, `vote`, `vote_with_reputation`, `execute_proposal`, `veto_proposal`, `cancel_proposal`, `treasury_transfer`, `get_treasury_balance`, `get_proposal`, `get_dao_stats`
- `moltpunks`: `initialize`, `mint`, `transfer`, `owner_of`, `balance_of`, `approve`, `transfer_from`, `burn`, `total_minted`
- `moltmarket`: `initialize`, `list_nft`, `buy_nft`, `cancel_listing`, `get_listing`, `set_marketplace_fee`
- `moltauction`: `initialize`, `create_auction`, `place_bid`, `finalize_auction`, `make_offer`, `accept_offer`, `set_royalty`, `update_collection_stats`, `get_collection_stats`
- `moltoracle`: `initialize_oracle`, `add_price_feeder`, `submit_price`, `get_price`, `commit_randomness`, `reveal_randomness`, `request_randomness`, `get_randomness`, `submit_attestation`, `verify_attestation`, `get_attestation_data`, `query_oracle`, `get_aggregated_price`, `get_oracle_stats`
- `moltbridge`: `initialize`, `add_bridge_validator`, `lock_tokens`, `mint_bridged`, `unlock_tokens`, `get_bridge_status`, `set_moltyid_address`, `set_identity_gate`
- `bountyboard`: `create_bounty`, `submit_work`, `approve_work`, `cancel_bounty`, `get_bounty`, `set_identity_admin`, `set_moltyid_address`, `set_identity_gate`
- `compute`: `register_provider`, `submit_job`, `claim_job`, `complete_job`, `dispute_job`, `get_job`, `set_identity_admin`, `set_moltyid_address`, `set_identity_gate`
- `storage`: `store_data`, `confirm_storage`, `get_storage_info`, `register_provider`, `claim_storage_rewards`
- `dex-core`: `create_pair`, `update_pair_fees`, `pause_pair`, `unpause_pair`, `place_order`, `cancel_order`, `cancel_all_orders`, `modify_order`, `match_order`, `settle_trade`, `get_order`, `get_open_orders`, `get_order_book`, `get_best_bid`, `get_best_ask`, `get_spread`, `get_trade_history`, `get_pair_info`
- `dex-amm`: `create_pool`, `add_liquidity`, `remove_liquidity`, `collect_fees`, `swap_exact_in`, `swap_exact_out`, `get_pool_info`, `quote_swap`
- `dex-router`: `swap`, `swap_exact_out`, `get_best_route`, `multi_hop_swap`
- `dex-governance`: `propose_new_pair`, `vote_on_pair`, `execute_pair_proposal`, `propose_fee_change`, `vote_on_fee`, `execute_fee_proposal`, `set_listing_requirements`, `emergency_delist`
- `dex-rewards`: `claim_trading_rewards`, `claim_lp_rewards`, `get_pending_rewards`, `set_reward_rate`, `register_referral`, `get_trading_tier`
- `dex-margin`: `open_margin_position`, `close_margin_position`, `add_margin`, `remove_margin`, `liquidate`, `get_margin_ratio`, `set_max_leverage`, `get_liquidatable_positions`
- `dex-analytics`: `record_trade`, `get_ohlcv`, `get_24h_stats`, `get_all_pairs_stats`, `get_trader_stats`, `get_leaderboard`, `update_price_feed`
- `prediction-market`: opcode-dispatch ABI, includes `create_market`, `buy_shares`, `sell_shares`, `add_liquidity`, `add_initial_liquidity`, `mint_complete_set`, `redeem_complete_set`, `get_market`, `submit_resolution`, `challenge_resolution`, `finalize_resolution`, `dao_resolve`, `dao_void`, `redeem_shares`, `reclaim_collateral`, `withdraw_liquidity`, `close_market`
- `musd-token`: `initialize`, `mint`, `burn`, `transfer`, `approve`, `transfer_from`, `total_supply`, `balance_of`, `get_reserves`
- `weth-token`: `initialize`, `mint`, `burn`, `transfer`, `approve`, `transfer_from`, `total_supply`, `balance_of`, `get_reserves`
- `wsol-token`: `initialize`, `mint`, `burn`, `transfer`, `approve`, `transfer_from`, `total_supply`, `balance_of`, `get_reserves`

For any contract write, always confirm runtime function signature and arg order with `getContractAbi` before building `molt call`.

---

## 8) REST Data Planes + Custody/Cross-Chain

RPC REST services:

- DEX base: `GET/POST http://localhost:8899/api/v1/*`
- Prediction base: `GET/POST http://localhost:8899/api/v1/prediction-market/*`

DEX REST routes (`rpc/src/dex.rs`):

- `/pairs`
- `/pairs/:id`
- `/pairs/:id/orderbook`
- `/pairs/:id/trades`
- `/pairs/:id/candles`
- `/pairs/:id/stats`
- `/pairs/:id/ticker`
- `/tickers`
- `/orders` (GET/POST)
- `/orders/:id` (GET/DELETE)
- `/router/swap` (POST)
- `/router/quote` (POST)
- `/routes`
- `/pools`
- `/pools/:id`
- `/pools/positions`
- `/margin/open` (POST)
- `/margin/close` (POST)
- `/margin/positions`
- `/margin/positions/:id`
- `/margin/info`
- `/leaderboard`
- `/traders/:addr/stats`
- `/rewards/:addr`
- `/governance/proposals` (GET/POST)
- `/governance/proposals/:id`
- `/governance/proposals/:id/vote` (POST)
- `/stats/core`
- `/stats/amm`
- `/stats/margin`
- `/stats/router`
- `/stats/rewards`
- `/stats/analytics`
- `/stats/governance`
- `/stats/moltswap`

Prediction REST routes (`rpc/src/prediction.rs`):

- `/stats`
- `/markets`
- `/markets/:id`
- `/markets/:id/price-history`
- `/markets/:id/analytics`
- `/positions`
- `/traders/:addr/stats`
- `/leaderboard`
- `/trending`
- `/trade` (POST)
- `/create` (POST)

Custody API (`moltchain-custody`, default `http://localhost:9105`):

- `GET /health`
- `GET /status`
- `POST /deposits`
- `GET /deposits/:deposit_id`
- `POST /withdrawals`
- `PUT /withdrawals/:job_id/burn`
- `GET /reserves`

Custody notes for autonomous agents:

- deposit flow: `issued -> confirmed -> swept -> credited`
- withdrawals require burn signature submission via `PUT /withdrawals/:job_id/burn`
- no webhook endpoint is present; use polling (`/deposits/:deposit_id`, `/status`) and/or WS subscriptions for notifications

---

## 8.1 Missing Coverage / Functionality Report (do not assume unsupported features)

Observed gaps between live code and docs:

- `developers/rpc-reference.html` does not document many live RPC methods in `rpc/src/lib.rs` (MoltyID RPC set, ReefStake methods, prediction stats methods, symbol registry, contract mutation methods, extended platform stats).
- `developers/ws-reference.html` does not document `subscribeDex`/`unsubscribeDex` and `subscribePrediction*`/`unsubscribePrediction*` channel families present in `rpc/src/ws.rs`.
- custody docs (`CUSTODY_DEPLOYMENT.md`) omit `PUT /withdrawals/:job_id/burn`, but endpoint exists and is required in `custody/src/main.rs`.
- hook/webhook callback endpoints are not implemented in custody service code; agents must use polling + WS instead.
- prediction market contract reference shows opcode-dispatch contract but only partially enumerates opcodes in docs (listed opcodes are sparse vs declared “24 fns”).

---

## 9) Autonomous Discovery (to never miss new live features)

Use this on running networks.

## 9.1 Enumerate CLI dynamically

```bash
$MOLT_BIN --help
$MOLT_BIN wallet --help
$MOLT_BIN identity --help
$MOLT_BIN stake --help
$MOLT_BIN validator --help
$MOLT_BIN token --help
$MOLT_BIN gov --help
$MOLT_BIN network --help
$MOLT_BIN contract --help
$MOLT_BIN account --help
```

## 9.2 Enumerate validator/network state

```bash
$MOLT_BIN --rpc-url "$RPC_URL" network
$MOLT_BIN --rpc-url "$RPC_URL" validators
```

## 9.3 Enumerate deployed contracts + ABIs (quick raw dump)

```bash
CONTRACTS=$(curl -sS -X POST "$RPC_URL" -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getAllContracts","params":[]}' | jq -r '.result.contracts[] | (.program_id // .address // .id)')

for c in $CONTRACTS; do
  curl -sS -X POST "$RPC_URL" -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getContractAbi\",\"params\":[\"$c\"]}" > "abi_$c.json"
done
```

For structured, machine-ingestible output, use Appendix A exporter.

---

## 10) Non-UI SDK Mode (agents as code)

Use official SDK docs + packages (from `MOLT_OFFICIAL_SDK_JS_URL`, `MOLT_OFFICIAL_SDK_PY_URL`, `MOLT_OFFICIAL_SDK_RS_URL`).

Core autonomous loop pattern:

1. connect RPC + WS
2. load keypair
3. health check
4. fetch balances/state
5. choose action policy
6. submit tx/call
7. await confirmation
8. verify post-state (`getProgramCalls`, `getContractEvents`, balance deltas)
9. log + retry with backoff on failure

---

## 11) Safety + Operational Guardrails

- Never expose private key files in logs.
- Verify chain health before writes.
- Simulate when possible before `sendTransaction`.
- Confirm tx finality and post-state before next dependent action.
- Keep deterministic state snapshots for bot idempotency.
- Before production writes, require explicit pre-trade risk limits and max-loss constraints in agent policy.

---

## 12) Canonical References (official runtime docs)

- `MOLT_OFFICIAL_VALIDATOR_URL`
- `MOLT_OFFICIAL_RPC_URL`
- `MOLT_OFFICIAL_CLI_URL`
- `MOLT_OFFICIAL_SDK_JS_URL`
- `MOLT_OFFICIAL_SDK_PY_URL`
- `MOLT_OFFICIAL_SDK_RS_URL`

---

## Appendix A) Generated Contract ABI Manifest (machine-readable)

Purpose: dump all currently deployed contract ABIs into one JSON manifest for direct autonomous-agent ingestion.

### A.1 Run exporter

```bash
python3 scripts/export_contract_abi_manifest.py \
  --rpc-url "$RPC_URL" \
  --out ./artifacts/contract-abi-manifest.json
```

### A.2 Output schema (stable)

Top-level keys:

- `generated_at`
- `rpc_url`
- `chain_status`
- `contract_count`
- `success_count`
- `failure_count`
- `contracts` (array)

Per-contract keys:

- `contract_id`
- `name`
- `source`
- `abi_methods` (normalized method/function names)
- `abi` (full raw ABI payload)
- `error` (nullable; populated when ABI retrieval fails)

### A.3 Agent ingestion guidance

Use `abi_methods` for fast tool selection and `abi` for full argument/signature fidelity before `molt call` construction.

---

This `skill.md` is intentionally designed for **agent autonomy**: if an agent can read files, run shell commands, and hit RPC/WS endpoints, it can operate the full MoltChain stack without UI interaction.
