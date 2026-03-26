# Lichen RPC API Reference

**Complete API Surface**: ~172 Methods (170 unique + 2 aliases)  
**Version**: 0.4.8  
**Protocol**: JSON-RPC 2.0  
**Default Port**: 8899 (testnet) / 9899 (mainnet)  
**Status**: ✅ Production Ready

---

## 🔌 Connection

```bash
# Default endpoint
http://localhost:8899/

# Production endpoints
https://rpc.lichen.network/

# Custom endpoint (CLI)
lichen --rpc-url http://custom-host:8899 <command>
```

## 🔁 Compatibility Adapters

Lichen exposes canonical RPC methods on the root endpoint and provides
compatibility adapters on dedicated routes for external tooling.

- Canonical Lichen RPC: `http://localhost:8899/`
- Solana-format compatibility: `http://localhost:8899/solana-compat` (legacy alias: `/solana`)
- EVM compatibility: `http://localhost:8899/evm`

## ✅ Release-Verified Operator Baseline

This baseline is kept consistent with `docs/consensus/VALIDATOR_SETUP.md`,
`developers/rpc-reference.html`, and `developers/ws-reference.html`.

- Canonical JSON-RPC endpoint: `http://localhost:8899`
- Canonical WebSocket endpoint: `ws://localhost:8900`

### Core RPC baseline

`health`, `getSlot`, `getValidators`, `getChainStatus`, `getNetworkInfo`

### Staking/economics RPC baseline

`getStakingStatus`, `getStakingRewards`, `getTreasuryInfo`, `getGenesisAccounts`, `getTotalBurned`, `getMossStakePoolInfo`

### Core WebSocket baseline

`subscribeSlots`, `subscribeBlocks`, `subscribeTransactions`, `subscribeAccount`, `subscribeLogs`, `subscribeValidators`, `subscribeDex`, `subscribePrediction`

---

## 🧰 Custody Service (REST)

The custody service runs separately from the validator RPC server and exposes
REST endpoints on port 9105 by default.

Base URL: `http://localhost:9105`

### `GET /health`
Basic health check.

**Returns**:
```json
{
  "status": "ok"
}
```

### `GET /status`
Operational status for sweeps and credits.

**Returns**:
```json
{
  "signers": {
    "configured": 3,
    "threshold": 2
  },
  "sweeps": {
    "total": 4,
    "by_status": {
      "queued": 1,
      "signed": 1,
      "sweep_submitted": 2
    }
  },
  "credits": {
    "total": 2,
    "by_status": {
      "queued": 1,
      "submitted": 1
    }
  }
}
```

### `POST /deposits`
Issue a one-time deposit address.

**Body**:
```json
{
  "user_id": "<licn_address>",
  "chain": "solana",
  "asset": "sol"
}
```

**Returns**:
```json
{
  "deposit_id": "uuid",
  "address": "<chain_address>"
}
```

### `GET /deposits/:deposit_id`
Fetch a deposit request.

---

## 📚 All Endpoints

### Core Queries (22 methods)

#### `getBalance`
Get account balance in spores and LICN.

**Params**: `[pubkey: string]`  
**Returns**:
```json
{ "spores": 1000000000, "licn": 1.0 }
```

#### `getAccount`
Get complete account information.

**Params**: `[pubkey: string]`  
**Returns**:
```json
{ "spores": 1000000000, "owner": "System...", "executable": false, "data": "0x..." }
```

#### `getAccountProof`
Get an anchored inclusion proof for an existing account.

This method returns an inclusion proof over the current cached account leaf set and
binds that proof to the requested block commitment context. It is not a full
authenticated-state scheme with non-existence proofs.

**Params**: `[pubkey: string, { commitment?: "processed" | "confirmed" | "finalized" }]`  
**Returns**:
```json
{
  "pubkey": "...",
  "account_data": "deadbeef...",
  "inclusion_proof": {
    "leaf_hash": "...",
    "siblings": ["..."],
    "path": [true, false]
  },
  "anchor": {
    "slot": 123,
    "commitment": "finalized",
    "state_root": "...",
    "block_hash": "...",
    "commit_round": 0,
    "commit_signatures": ["..."]
  }
}
```

**Notes**:
- Returns an error if the account does not exist or if the proof cannot be anchored to the requested block context.
- Intended as an anchored inclusion-proof surface, not as a complete light-client or non-existence-proof protocol.

#### `getBlock`
Get block by slot number.

**Params**: `[slot: number]`  
**Returns**: Block data with transactions

#### `getLatestBlock`
Get most recent block.

**Params**: None  
**Returns**: Latest block data

#### `getSlot`
Get current slot number.

**Params**: None  
**Returns**: `{ "slot": 12345 }`

#### `getTransaction`
Get transaction by signature.

**Params**: `[signature: string]`  
**Returns**: Transaction data including `status`, `slot`, `from`, `to`, `amount`, `fee`, `compute_units`, `confirmation_status`, `confirmations`.  
For contract calls, also includes: `return_code` (u32, omitted when null), `return_data` (base64, omitted when empty), `contract_logs` (string array, omitted when empty).

#### `getTransactionsByAddress`
Get all transactions for an address.

**Params**: `[pubkey: string, limit?: number]`  
**Returns**: `{ "transactions": [...], "count": 5 }`

#### `getAccountTxCount`
Get total transaction count for an account.

**Params**: `[pubkey: string]`  
**Returns**: `{ "count": 42 }`

#### `getRecentTransactions`
Get the most recently confirmed transactions.

**Params**: `[limit?: number]`  
**Returns**: `{ "transactions": [...] }`

#### `getTokenAccounts`
Get token accounts held by an address.

**Params**: `[pubkey: string]`  
**Returns**: `{ "token_accounts": [...] }`

#### `sendTransaction`
Submit a signed transaction to the mempool.

**Params**: `[transaction: base64]`  
**Returns**: `{ "signature": "..." }`

#### `confirmTransaction`
Check if a transaction has been confirmed.

**Params**: `[signature: string]`  
**Returns**: `{ "confirmed": true, "slot": 12345 }`

#### `simulateTransaction`
Simulate a transaction without submitting.

**Params**: `[transaction: base64]`  
**Returns**: `{ "success": true, "logs": [...] }`

#### `callContract`
Read-only contract call (no state mutation). Loads storage from canonical DB.

**Params**: `[contract_id: string, method: string, args?: object]` or `{contract, function, args?, from?}`  
- `from` / `caller` (optional): Base58 pubkey to use as the caller context. Defaults to zero address if omitted.

**Returns**: `{success, returnData, returnCode, logs, error, computeUsed}`

#### `getTotalBurned`
Get total LICN burned.

**Params**: None  
**Returns**: `{ "spores": 1000000, "licn": 0.001 }`

#### `getValidators`
List all validators.

**Params**: None  
**Returns**:
```json
{
  "validators": [{
    "pubkey": "...", "stake": 100000000000, "reputation": 985,
    "blocks_proposed": 150, "votes_cast": 1200, "correct_votes": 1185,
    "last_active_slot": 12340
  }],
  "count": 5
}
```

#### `getMetrics`
Get performance metrics.

**Params**: None  
**Returns**:
```json
{
  "tps": 1250.5,
  "peak_tps": 1840.0,
  "total_transactions": 1500000,
  "daily_transactions": 42000,
  "total_blocks": 12345,
  "average_block_time": 0.8,
  "avg_block_time_ms": 800.0,
  "avg_txs_per_block": 121.5,
  "total_accounts": 18420,
  "active_accounts": 6912,
  "total_supply": 500000000000000000,
  "projected_supply": 500000012500000000,
  "circulating_supply": 421500000000000000,
  "total_burned": 2200000000,
  "total_minted": 12500000000,
  "total_staked": 185000000000000,
  "treasury_balance": 32000000000000,
  "total_contracts": 29,
  "validator_count": 5,
  "slot_duration_ms": 800,
  "fee_burn_percent": 40,
  "current_epoch": 12,
  "slots_into_epoch": 3456,
  "inflation_rate_bps": 395
}
```

#### `getTreasuryInfo`
Get treasury address and balances.

**Params**: None  
**Returns**: `{ "treasury_pubkey": "...", "treasury_balance": 0, "treasury_balance_licn": 0.0 }`

#### `getGenesisAccounts`
List genesis accounts and allocation metadata.

**Params**: None  
**Returns**: `{ "accounts": [{ "role": "genesis", "pubkey": "...", "amount_licn": 0 }] }`

#### `getGovernedProposal`
Get governance proposal details.

**Params**: `[proposal_id: string]`  
**Returns**: Proposal object with status, votes, description

#### `getRecentBlockhash`
Get the most recent blockhash for transaction construction.

**Params**: None  
**Returns**: `{ "blockhash": "...", "slot": 12345 }`

#### `health` / `getHealth`
Health check with block staleness detection.

**Params**: None  
**Returns**: `{ "status": "ok", "slot": 12345 }`

---

### Fee & Rent Config (4 methods)

#### `getFeeConfig`
Get current fee configuration.

**Params**: None  
**Returns**: `{ "base_fee": 1000000, "burn_pct": 40, "producer_pct": 30, "voter_pct": 10, "treasury_pct": 10, "community_pct": 10 }`

#### `setFeeConfig`
Update fee configuration (admin only).

**Params**: `[config: object]`  
**Returns**: `{ "success": true }`

#### `getRentParams`
Get rent exemption parameters.

**Params**: None  
**Returns**: `{ "lamports_per_byte_year": ..., "exemption_threshold": ... }`

#### `setRentParams`
Update rent parameters (admin only).

**Params**: `[params: object]`  
**Returns**: `{ "success": true }`

---

### Network (3 methods)

#### `getPeers`
List connected P2P peers.

**Params**: None  
**Returns**:
```json
{ "peers": [{ "peer_id": "12D3KooW...", "address": "/ip4/127.0.0.1/tcp/8001" }], "count": 1 }
```

#### `getNetworkInfo`
Get network metadata.

**Params**: None  
**Returns**:
```json
{ "chain_id": "lichen-mainnet", "version": "0.4.8", "current_slot": 12345, "validator_count": 5, "peer_count": 3 }
```

#### `getClusterInfo`
Get cluster-wide information including all nodes.

**Params**: None  
**Returns**: `{ "nodes": [...], "cluster_id": "...", "validators": 5 }`

---

### Validator (3 methods)

#### `getValidatorInfo`
Get detailed validator information.

**Params**: `[pubkey: string]`  
**Returns**:
```json
{
  "pubkey": "...", "stake": 100000000000, "reputation": 985,
  "blocks_proposed": 150, "votes_cast": 1200, "correct_votes": 1185,
  "commission_rate": 0, "is_active": true
}
```

#### `getValidatorPerformance`
Get validator performance metrics.

**Params**: `[pubkey: string]`  
**Returns**:
```json
{ "pubkey": "...", "blocks_proposed": 150, "vote_accuracy": 98.75, "reputation": 985, "uptime": 99.5 }
```

#### `getChainStatus`
Get comprehensive chain status.

**Params**: None  
**Returns**:
```json
{
  "slot": 12345,
  "_slot": 12345,
  "epoch": 12,
  "_epoch": 12,
  "block_height": 12345,
  "_block_height": 12345,
  "current_slot": 12345,
  "latest_block": 12345,
  "validator_count": 5,
  "validators": 5,
  "_validators": 5,
  "total_stake": 185000000000000,
  "total_staked": 185000000000000,
  "tps": 1250.5,
  "peak_tps": 1840.0,
  "total_transactions": 1500000,
  "total_blocks": 12345,
  "average_block_time": 0.8,
  "block_time_ms": 800.0,
  "total_supply": 500000000000000000,
  "projected_supply": 500000012500000000,
  "total_burned": 2200000000,
  "total_minted": 12500000000,
  "peer_count": 8,
  "chain_id": "lichen-mainnet",
  "network": "mainnet",
  "is_healthy": true,
  "inflation_rate_bps": 395
}
```

---

### Staking (4 methods)

#### `stake`
Create stake transaction.

**Params**: `[from: string, validator: string, amount: number]`  
**Returns**: `{ "signature": "...", "amount": 100000000000, "validator": "...", "status": "pending" }`

#### `unstake`
Create unstake transaction.

**Params**: `[from: string, validator: string, amount: number]`  
**Returns**: `{ "signature": "...", "amount": 100000000000, "status": "pending", "unlock_epoch": 1706972400 }`

#### `getStakingStatus`
Get account staking status.

**Params**: `[pubkey: string]`  
**Returns**: `{ "is_validator": true, "total_staked": 100000000000, "delegations": [], "status": "active" }`

#### `getStakingRewards`
Get staking rewards info.

**Params**: `[pubkey: string]`  
**Returns**: `{ "total_rewards": 0, "pending_rewards": 0, "projected_pending": 0, "projected_epoch_reward": 0, "claimed_rewards": 0, "liquid_claimed_rewards": 0, "claimed_total_rewards": 0, "reward_rate": 5.0 }`

`pending_rewards` reflects settled-but-unclaimed rewards already accounted to the validator. `projected_pending` and `projected_epoch_reward` are current-epoch estimates exposed before the next epoch-boundary mint finalizes. `claimed_rewards` and `liquid_claimed_rewards` report only the liquid portion that became spendable to the validator. `claimed_total_rewards` includes both liquid claims and any bootstrap debt repayment already credited through prior claims. `reward_rate` is a projected base per-slot accrual signal, not a continuously minted payout stream.

---

### MossStake Liquid Staking (6 methods)

#### `stakeToMossStake`
Deprecated write RPC. Use `sendTransaction` with system instruction type `13` (MossStake deposit).

**Params**: legacy-only `[from: string, amount: number]`  
**Returns**: JSON-RPC error `-32601` with guidance to submit a signed transaction using data `[13, amount_le_bytes(8)]` and accounts `[depositor_pubkey]`.

#### `unstakeFromMossStake`
Deprecated write RPC. Use `sendTransaction` with system instruction type `14` (MossStake unstake).

**Params**: legacy-only `[from: string, amount: number]`  
**Returns**: JSON-RPC error `-32601` with guidance to submit a signed transaction using data `[14, st_licn_amount_le_bytes(8)]` and accounts `[user_pubkey]`.

#### `claimUnstakedTokens`
Deprecated write RPC. Use `sendTransaction` with system instruction type `15` (MossStake claim).

**Params**: legacy-only `[from: string]`  
**Returns**: JSON-RPC error `-32601` with guidance to submit a signed transaction using data `[15]` and accounts `[user_pubkey]`.

#### `getStakingPosition`
Get a user's MossStake position.

**Params**: `[pubkey: string]`  
**Returns**:
```json
{
  "owner": "...",
  "st_licn_amount": 0,
  "licn_deposited": 0,
  "current_value_licn": 0,
  "rewards_earned": 0,
  "deposited_at": 0,
  "lock_tier": 0,
  "lock_tier_name": "Flexible",
  "lock_until": 0,
  "reward_multiplier": 1.0
}
```

#### `getMossStakePoolInfo`
Get MossStake pool-level metrics.

**Params**: None  
**Returns**:
```json
{
  "total_supply_st_licn": 0,
  "total_licn_staked": 0,
  "exchange_rate": 1.0,
  "total_validators": 3,
  "average_apy_percent": 0.0,
  "total_stakers": 0,
  "tiers": [
    { "id": 0, "name": "Flexible", "lock_days": 0, "multiplier": 1.0, "apy_percent": 0.0 }
  ],
  "cooldown_days": 7
}
```

#### `getUnstakingQueue`
Get pending unstaking requests for an account.

**Params**: `[pubkey: string]`  
**Returns**:
```json
{
  "owner": "...",
  "pending_requests": [
    {
      "st_licn_amount": 0,
      "licn_to_receive": 0,
      "requested_at": 0,
      "claimable_at": 0
    }
  ],
  "total_claimable": 0
}
```

---

### Price-Based Rewards (1 method)

#### `getRewardAdjustmentInfo`
Get current reward adjustment parameters based on LICN price.

**Params**: None  
**Returns**: `{ "current_multiplier": 1.0, "target_price": ..., "current_price": ..., "totalSupply": ..., "projectedSupply": ..., "totalMinted": ..., "inflationRateBps": ... }`

`projectedSupply` can exceed `totalSupply` during an epoch because it includes the current epoch's projected mint before settlement. `totalMinted` changes when the epoch-boundary mint is applied on-chain.

---

### Account (2 methods)

#### `getAccountInfo`
Get enhanced account information.

**Params**: `[pubkey: string]`  
**Returns**:
```json
{ "pubkey": "...", "balance": 1000000000, "licn": 1.0, "exists": true, "is_validator": false, "is_executable": false }
```

#### `getTransactionHistory`
Get account transaction history.

**Params**: `[pubkey: string, limit?: number]`  
**Returns**: `{ "transactions": [], "count": 0, "limit": 10 }`

---

### Contracts (7 methods)

#### `getContractInfo`
Get contract metadata. For tokens registered in the symbol registry, includes `total_supply` resolved from on-chain storage (key `{symbol_lowercase}_supply`) with a fallback to registry metadata.

**Params**: `[contract_id: string]`  
**Returns**: `{ "contract_id": "...", "owner": "...", "code_size": 4096, "is_executable": true, "symbol": "MYTK", "total_supply": 1000000000000000, "decimals": 9, "holders": 42 }`

#### `getContractLogs`
Get contract execution logs.

**Params**: `[contract_id: string]`  
**Returns**: `{ "logs": [], "count": 0 }`

#### `getContractAbi`
Get stored ABI for a contract.

**Params**: `[contract_id: string]`  
**Returns**: `{ "abi": { "methods": [...] } }`

#### `setContractAbi`
Store an ABI for a contract (owner only).

**Params**: `[contract_id: string, abi: object]`  
**Returns**: `{ "success": true }`

#### `getAllContracts`
List all deployed contracts.

**Params**: `[offset?: number, limit?: number]`  
**Returns**: `{ "contracts": [...], "count": 29 }`

#### `deployContract`
Deploy a WASM contract.

**Params**: `[transaction: base64]`  
**Returns**: `{ "signature": "...", "contract_id": "..." }`

#### `upgradeContract`
Upgrade an existing contract's WASM code.

**Params**: `[transaction: base64]`  
**Returns**: `{ "signature": "...", "contract_id": "..." }`

---

### Programs (5 methods)

#### `getProgram`
Get program metadata by program ID.

**Params**: `[program_id: string]`  
**Returns**: `{ "program_id": "...", "owner": "...", "code_size": ..., "name": "..." }`

#### `getProgramStats`
Get execution statistics for a program.

**Params**: `[program_id: string]`  
**Returns**: `{ "total_calls": ..., "unique_callers": ..., "last_called_slot": ... }`

#### `getPrograms`
List all programs with optional filtering.

**Params**: `[offset?: number, limit?: number]`  
**Returns**: `{ "programs": [...], "count": ... }`

#### `getProgramCalls`
Get recent call history for a program.

**Params**: `[program_id: string, limit?: number]`  
**Returns**: `{ "calls": [...] }`

#### `getProgramStorage`
Get a program's persistent storage entries.

**Params**: `[program_id: string, keys?: string[]]`  
**Returns**: `{ "entries": { "key": "value", ... } }`

---

### LichenID & Names (14 methods)

#### `getLichenIdIdentity`
Get a LichenID identity record.

**Params**: `[pubkey: string]`  
**Returns**: `{ "pubkey": "...", "name": "alice.lichen", "agent_type": "...", "created_at": ... }`

#### `getLichenIdReputation`
Get reputation score for an identity.

**Params**: `[pubkey: string]`  
**Returns**: `{ "score": 850, "level": "trusted", "history": [...] }`

#### `getLichenIdSkills`
Get registered skills for an identity.

**Params**: `[pubkey: string]`  
**Returns**: `{ "skills": ["rust", "wasm", "defi"], "endorsements": ... }`

#### `getLichenIdVouches`
Get vouch records for an identity.

**Params**: `[pubkey: string]`  
**Returns**: `{ "vouches": [{ "from": "...", "weight": 10 }] }`

#### `getLichenIdAchievements`
Get achievements earned by an identity.

**Params**: `[pubkey: string]`  
**Returns**: `{ "achievements": [{ "id": "...", "name": "...", "earned_at": ... }] }`

#### `getLichenIdProfile`
Get combined profile view for an identity.

**Params**: `[pubkey: string]`  
**Returns**: Full profile with identity, reputation, skills, achievements

#### `resolveLichenName`
Resolve a `.lichen` name to a public key.

**Params**: `[name: string]`  
**Returns**: `{ "pubkey": "...", "name": "alice.lichen" }`

#### `reverseLichenName`
Reverse-resolve a public key to a `.lichen` name.

**Params**: `[pubkey: string]`  
**Returns**: `{ "name": "alice.lichen" }`

#### `batchReverseLichenNames`
Batch reverse-resolve multiple pubkeys to names.

**Params**: `[pubkeys: string[]]`  
**Returns**: `{ "results": { "pubkey1": "alice.lichen", "pubkey2": null } }`

#### `searchLichenNames`
Search for `.lichen` names by prefix or pattern.

**Params**: `[query: string, limit?: number]`  
**Returns**: `{ "results": [{ "name": "alice.lichen", "pubkey": "..." }] }`

#### `getLichenIdAgentDirectory`
Browse the agent directory.

**Params**: `[filter?: object, limit?: number]`  
**Returns**: `{ "agents": [...], "count": ... }`

#### `getLichenIdStats`
Get LichenID system-wide statistics.

**Params**: None  
**Returns**: `{ "total_identities": ..., "total_names": ..., "total_vouches": ... }`

#### `getNameAuction`
Get current auction for a premium `.lichen` name.

**Params**: `[name: string]`  
**Returns**: `{ "name": "...", "highest_bid": ..., "bidder": "...", "ends_at": ... }`

---

### EVM Address Registry (2 methods)

#### `getEvmRegistration`
Get EVM address registration for a Lichen pubkey.

**Params**: `[pubkey: string]`  
**Returns**: `{ "evm_address": "0x...", "pubkey": "..." }`

#### `lookupEvmAddress`
Look up a Lichen pubkey by EVM address.

**Params**: `[evm_address: string]`  
**Returns**: `{ "pubkey": "...", "evm_address": "0x..." }`

---

### Symbol Registry (3 methods)

#### `getSymbolRegistry`
Get symbol registry entry by ticker.

**Params**: `[symbol: string]`  
**Returns**: `{ "symbol": "LICN", "program_id": "...", "decimals": 9 }`

#### `getSymbolRegistryByProgram`
Get symbol registration for a specific program.

**Params**: `[program_id: string]`  
**Returns**: `{ "symbol": "...", "program_id": "...", "decimals": ... }`

#### `getAllSymbolRegistry`
Get all registered symbols.

**Params**: `[offset?: number, limit?: number]`  
**Returns**: `{ "symbols": [...], "count": ... }`

---

### NFT & Marketplace (9 methods)

#### `getCollection`
Get NFT collection metadata.

**Params**: `[collection_id: string]`  
**Returns**: `{ "collection_id": "...", "name": "...", "creator": "...", "total_supply": ... }`

#### `getNFT`
Get individual NFT metadata.

**Params**: `[nft_id: string]`  
**Returns**: `{ "nft_id": "...", "owner": "...", "collection": "...", "metadata_uri": "..." }`

#### `getNFTsByOwner`
Get all NFTs owned by an address.

**Params**: `[pubkey: string, limit?: number]`  
**Returns**: `{ "nfts": [...], "count": ... }`

#### `getNFTsByCollection`
Get all NFTs in a collection.

**Params**: `[collection_id: string, limit?: number]`  
**Returns**: `{ "nfts": [...], "count": ... }`

#### `getNFTActivity`
Get recent activity for an NFT.

**Params**: `[nft_id: string]`  
**Returns**: `{ "activity": [{ "type": "transfer", "from": "...", "to": "...", "slot": ... }] }`

#### `getMarketListings`
Get active marketplace listings.

**Params**: `[collection_id?: string, limit?: number]`  
**Returns**: `{ "listings": [...], "count": ... }`

#### `getMarketSales`
Get recent marketplace sales.

**Params**: `[collection_id?: string, limit?: number]`  
**Returns**: `{ "sales": [...], "count": ... }`

#### `getMarketOffers`
Get active offers on NFTs.

**Params**: `[nft_id?: string, limit?: number]`  
**Returns**: `{ "offers": [...], "count": ... }`

#### `getMarketAuctions`
Get active auctions.

**Params**: `[collection_id?: string, limit?: number]`  
**Returns**: `{ "auctions": [...], "count": ... }`

---

### Token (4 methods)

#### `getTokenBalance`
Get token balance for an account on a specific token contract.

**Params**: `[pubkey: string, token_program: string]`  
**Returns**: `{ "balance": ..., "decimals": 9 }`

#### `getTokenHolders`
Get all holders of a token.

**Params**: `[token_program: string, limit?: number]`  
**Returns**: `{ "holders": [{ "pubkey": "...", "balance": ... }], "count": ... }`

#### `getTokenTransfers`
Get transfer history for a token.

**Params**: `[token_program: string, limit?: number]`  
**Returns**: `{ "transfers": [...], "count": ... }`

#### `getContractEvents`
Get emitted events from a contract.

**Params**: `[contract_id: string, limit?: number]`  
**Returns**: `{ "events": [...], "count": ... }`

---

### Faucet (1 method)

#### `requestAirdrop`
Request testnet LICN airdrop. Rate-limited to 1 request per 60 seconds per IP.

**Params**: `[pubkey: string, amount?: number]`  
**Returns**: `{ "signature": "...", "amount": 1000000000 }`

---

### Prediction Market (8 methods)

#### `getPredictionMarketStats`
Get system-wide prediction market statistics.

**Params**: None  
**Returns**: `{ "total_markets": ..., "total_volume": ..., "active_markets": ... }`

#### `getPredictionMarkets`
List prediction markets with optional filtering.

**Params**: `[status?: string, limit?: number]`  
**Returns**: `{ "markets": [...], "count": ... }`

#### `getPredictionMarket`
Get a single prediction market by ID.

**Params**: `[market_id: string]`  
**Returns**: `{ "market_id": "...", "question": "...", "outcomes": [...], "total_volume": ... }`

#### `getPredictionPositions`
Get a user's positions across prediction markets.

**Params**: `[pubkey: string]`  
**Returns**: `{ "positions": [{ "market_id": "...", "outcome": ..., "shares": ... }] }`

#### `getPredictionTraderStats`
Get trading statistics for a prediction market user.

**Params**: `[pubkey: string]`  
**Returns**: `{ "total_trades": ..., "total_volume": ..., "pnl": ... }`

#### `getPredictionLeaderboard`
Get prediction market leaderboard.

**Params**: `[limit?: number]`  
**Returns**: `{ "traders": [{ "pubkey": "...", "pnl": ..., "win_rate": ... }] }`

#### `getPredictionTrending`
Get trending prediction markets.

**Params**: None  
**Returns**: `{ "markets": [...] }`

#### `getPredictionMarketAnalytics`
Get detailed analytics for a prediction market.

**Params**: `[market_id: string]`  
**Returns**: `{ "market_id": "...", "volume_history": [...], "price_history": [...] }`

---

### DEX & Platform Stats (24 methods)

#### `getDexCoreStats`
Stats for the DEX core order book contract.

**Params**: None · **Returns**: `{ "total_orders": ..., "total_volume": ..., "pairs": ... }`

#### `getDexAmmStats`
Stats for the DEX AMM (automated market maker) contract.

**Params**: None · **Returns**: `{ "pools": ..., "tvl": ..., "volume_24h": ... }`

#### `getDexMarginStats`
Stats for the DEX margin trading contract.

**Params**: None · **Returns**: `{ "open_positions": ..., "total_collateral": ... }`

#### `getDexRewardsStats`
Stats for DEX reward distribution contract.

**Params**: None · **Returns**: `{ "total_distributed": ..., "active_stakers": ... }`

#### `getDexRouterStats`
Stats for the DEX aggregation router contract.

**Params**: None · **Returns**: `{ "total_routes": ..., "volume_routed": ... }`

#### `getDexAnalyticsStats`
Stats for the DEX analytics contract.

**Params**: None · **Returns**: `{ "tracked_pairs": ..., "data_points": ... }`

#### `getDexGovernanceStats`
Stats for DEX governance contract.

**Params**: None · **Returns**: `{ "proposals": ..., "voters": ..., "quorum": ... }`

#### `getLichenSwapStats`
Stats for LichenSwap AMM contract.

**Params**: None · **Returns**: `{ "pools": ..., "tvl": ..., "total_swaps": ... }`

#### `getThallLendStats`
Stats for ThallLend lending protocol contract.

**Params**: None · **Returns**: `{ "total_deposited": ..., "total_borrowed": ..., "utilization": ... }`

#### `getSporePayStats`
Stats for SporePay payments contract.

**Params**: None · **Returns**: `{ "total_payments": ..., "total_volume": ... }`

#### `getBountyBoardStats`
Stats for BountyBoard contract.

**Params**: None · **Returns**: `{ "open_bounties": ..., "total_posted": ..., "total_claimed": ... }`

#### `getComputeMarketStats`
Stats for Compute Market contract.

**Params**: None · **Returns**: `{ "providers": ..., "jobs_completed": ... }`

#### `getMossStorageStats`
Stats for Moss Storage decentralized storage contract.

**Params**: None · **Returns**: `{ "total_stored_bytes": ..., "providers": ... }`

#### `getLichenMarketStats`
Stats for LichenMarket NFT marketplace contract.

**Params**: None · **Returns**: `{ "total_listings": ..., "total_volume": ... }`

#### `getLichenAuctionStats`
Stats for LichenAuction contract.

**Params**: None · **Returns**: `{ "active_auctions": ..., "total_auctions": ... }`

#### `getLichenPunksStats`
Stats for LichenPunks NFT collection contract.

**Params**: None · **Returns**: `{ "total_supply": ..., "minted": ..., "floor_price": ... }`

#### `getMusdStats`
Stats for lUSD stablecoin contract.

**Params**: None · **Returns**: `{ "total_supply": ..., "holders": ... }`

#### `getWethStats`
Stats for Wrapped ETH (wETH) contract.

**Params**: None · **Returns**: `{ "total_supply": ..., "holders": ... }`

#### `getWsolStats`
Stats for Wrapped SOL (wSOL) contract.

**Params**: None · **Returns**: `{ "total_supply": ..., "holders": ... }`

#### `getWbnbStats`
Stats for Wrapped BNB (wBNB) contract.

**Params**: None · **Returns**: `{ "total_supply": ..., "holders": ... }`

#### `getSporeVaultStats`
Stats for SporeVault yield aggregator contract.

**Params**: None · **Returns**: `{ "vaults": ..., "tvl": ... }`

#### `getLichenBridgeStats`
Stats for LichenBridge cross-chain bridge contract.

**Params**: None · **Returns**: `{ "total_deposits": ..., "total_withdrawals": ..., "supported_chains": ... }`

#### `getLichenDaoStats`
Stats for LichenDAO governance contract.

**Params**: None · **Returns**: `{ "proposals": ..., "members": ..., "treasury_balance": ... }`

#### `getLichenOracleStats`
Stats for LichenOracle price feed contract.

**Params**: None · **Returns**: `{ "feeds": ..., "last_update_slot": ... }`

---

### Bridge (3 methods)

#### `createBridgeDeposit`
Create a cross-chain bridge deposit.

**Params**: `[from: string, chain: string, asset: string, amount: number]`  
**Returns**: `{ "deposit_id": "...", "status": "pending" }`

#### `getBridgeDeposit`
Get a bridge deposit by ID.

**Params**: `[deposit_id: string]`  
**Returns**: `{ "deposit_id": "...", "chain": "...", "amount": ..., "status": "..." }`

#### `getBridgeDepositsByRecipient`
Get all bridge deposits for a recipient.

**Params**: `[pubkey: string]`  
**Returns**: `{ "deposits": [...] }`

---

### Wallet Price Feeds (2 methods)

#### `getDexPairs`
Get all DEX trading pairs with current prices.

**Params**: None  
**Returns**: `{ "pairs": [{ "base": "LICN", "quote": "lUSD", "price": ..., "volume_24h": ... }] }`

#### `getOraclePrices`
Get latest oracle price feeds.

**Params**: None  
**Returns**: `{ "prices": { "LICN": ..., "ETH": ..., "SOL": ..., "BNB": ... } }`

---

### Shielded Pool / ZK Privacy (11 methods)

#### `getShieldedPoolState`
Get shielded pool state for a token.

**Params**: `[token?: string]`  
**Returns**: `{ "pool_size": ..., "total_shielded": ..., "merkle_root": "..." }`

#### `getShieldedPoolStats`
Get shielded pool statistics.

**Params**: `[token?: string]`  
**Returns**: `{ "total_deposits": ..., "total_withdrawals": ..., "current_balance": ... }`

#### `getShieldedMerkleRoot`
Get current Merkle root of the shielded pool.

**Params**: `[token?: string]`  
**Returns**: `{ "root": "0x..." }`

#### `getShieldedMerklePath`
Get Merkle inclusion proof for a commitment.

**Params**: `[commitment: string, token?: string]`  
**Returns**: `{ "path": [...], "index": ..., "root": "0x..." }`

#### `isNullifierSpent` / `checkNullifier`
Check if a ZK nullifier has been spent.

**Params**: `[nullifier: string, token?: string]`  
**Returns**: `{ "spent": false }`

#### `getShieldedCommitments`
Get recent shielded commitments.

**Params**: `[token?: string, limit?: number]`  
**Returns**: `{ "commitments": [...] }`

#### `computeShieldCommitment`
Compute a shield commitment from inputs.

**Params**: `[amount: number, randomness: string, recipient?: string]`  
**Returns**: `{ "commitment": "0x..." }`

#### `generateShieldProof`
Generate a Groth16 proof for a shield (deposit) operation.

**Params**: `[amount: number, randomness: string, recipient: string]`  
**Returns**: `{ "proof": "0x...", "public_inputs": [...] }`

#### `generateUnshieldProof`
Generate a Groth16 proof for an unshield (withdraw) operation.

**Params**: `[commitment: string, nullifier: string, amount: number, recipient: string]`  
**Returns**: `{ "proof": "0x...", "public_inputs": [...] }`

#### `generateTransferProof`
Generate a Groth16 proof for a private transfer.

**Params**: `[input_commitments: string[], output_commitments: string[], ...]`  
**Returns**: `{ "proof": "0x...", "public_inputs": [...] }`

---

## 💡 Usage Examples

### cURL
```bash
# Health check
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"health","params":[]}'

# Get network info
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getNetworkInfo","params":[]}'

# Get balance
curl -X POST http://localhost:8899/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"getBalance","params":["YOUR_PUBKEY"]}'
```

### CLI
```bash
# Network operations
lichen network status
lichen network peers
lichen network info

# Validator operations
lichen validator info <pubkey>
lichen validator performance <pubkey>
lichen validator list

# Staking operations
lichen stake add <from> <validator> <amount>
lichen stake remove <from> <validator> <amount>
lichen stake status <pubkey>
lichen stake rewards <pubkey>

# Account operations
lichen account info <pubkey>
lichen account history <pubkey> 10

# Contract operations
lichen contract info <contract_id>
lichen contract logs <contract_id>
lichen contract list
```

### JavaScript (using fetch)
```javascript
async function rpcCall(method, params = []) {
  const response = await fetch('http://localhost:8899/', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method,
      params
    })
  });
  
  const data = await response.json();
  return data.result;
}

// Get network info
const networkInfo = await rpcCall('getNetworkInfo');
console.log(networkInfo);

// Get balance
const balance = await rpcCall('getBalance', ['YOUR_PUBKEY']);
console.log(balance);
```

---

## 🔒 Error Codes

Standard JSON-RPC 2.0 error codes:

| Code | Message | Description |
|------|---------|-------------|
| -32700 | Parse error | Invalid JSON |
| -32600 | Invalid request | Not JSON-RPC 2.0 |
| -32601 | Method not found | Unknown method |
| -32602 | Invalid params | Invalid parameters |
| -32000 | Database error | State store error |
| -32001 | Not found | Resource not found |

**Error Response Format**:
```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32602,
    "message": "Invalid params: expected [pubkey]"
  }
}
```

---

**Documentation Version**: 2.0  
**Last Updated**: March 2026  
**Status**: Complete ✅
