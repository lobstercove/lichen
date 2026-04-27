---
name: lichen
description: Complete operational reference for autonomous agents working in the Lichen repository.
---

# Lichen — Agent Skill Book

> Complete operational reference for autonomous agents on Lichen.
> Covers every contract, RPC endpoint, WebSocket subscription, CLI command, transaction type,
> wallet operation, DEX strategy, identity system, achievement, ZK privacy flow, and deployment procedure.

---

## Table of Contents

1. Quick Reference
2. Architecture
3. Native Transaction Types
4. Contract Call Format
5. Contract Surface (28 Contracts)
6. DEX Contracts — Full Opcode Reference
7. LichenID Identity System
8. Achievement System (90+ Achievements)
9. Staking & MossStake
10. ZK Shielded Transactions
11. RPC Methods
12. REST API Endpoints
13. WebSocket Subscriptions
14. JavaScript SDK
15. Wallet Operations
16. CLI Reference
17. Validator Operations
18. Contract Development
19. Build & Test

---

## 1. Quick Reference

| Property | Value |
|----------|-------|
| Chain | Lichen (custom L1) |
| Consensus | Tendermint-style BFT (PoS with contributory stake) |
| Block time | ~800ms heartbeat, ~400ms with transactions (adaptive) |
| Native token | LICN (1 LICN = 1 000 000 000 spores) |
| Signing | ML-DSA-65 (native) |
| Smart contracts | WASM (Rust → wasm32-unknown-unknown) |
| ZK proofs | Transparent STARKs (Plonky3/FRI, Poseidon2-backed) |
| RPC | JSON-RPC 2.0 on port 8899 |
| Solana-format RPC | `POST /solana-compat` on port 8899 (legacy alias: `/solana`) |
| EVM compat RPC | `POST /evm` on port 8899 |
| WebSocket | Port 8900 |
| Explorer | Port 3001 |
| DEX | Port 8080 |
| Wallet | Port 3000 |
| Faucet | Port 9100 |
| Custody | Port 9105 |
| Monitoring | Port 9100 (Prometheus metrics) |
| Contracts deployed at genesis | 28 |
| Trading pairs at genesis | 7 |
| Total JSON-RPC method names | 155 (native + Solana compat + EVM compat) |
| Total REST routes | 72 |
| Total contract opcodes | 147 (DEX) + named exports (22 contracts) |
| Achievements | 90+ auto-detected |

### Production Endpoints

| Service | URL | Network |
|---------|-----|---------|
| RPC (Mainnet) | `https://rpc.lichen.network` | Mainnet |
| WebSocket (Mainnet) | `wss://rpc.lichen.network/ws` | Mainnet |
| RPC (Testnet) | `https://testnet-rpc.lichen.network` | Testnet |
| WebSocket (Testnet) | `wss://testnet-rpc.lichen.network/ws` | Testnet |
| Custody Bridge | `https://custody.lichen.network` | — |
| Faucet | `https://faucet.lichen.network` | Testnet |
| Explorer | `https://explorer.lichen.network` | — |
| DEX (SporeSwap) | `https://dex.lichen.network` | — |
| Wallet | `https://wallet.lichen.network` | — |
| Developer Portal | `https://developers.lichen.network` | — |
| Marketplace | `https://marketplace.lichen.network` | — |
| Programs IDE | `https://programs.lichen.network` | — |
| Monitoring | `https://monitoring.lichen.network` | — |

### Official Links

| Resource | URL |
|---------|-----|
| Website | `https://lichen.network` |
| Documentation | `https://developers.lichen.network` |
| GitHub | `https://github.com/lobstercove/lichen` |
| Email | `hello@lichen.network` |
| Discord | `https://discord.gg/gkQmsHXRXp` |
| X | `https://x.com/LichenHQ` |
| Telegram | `https://t.me/lichenhq` |

### Canonical Reference Entry Points

Use these pages when you need the public developer surface documentation rather than the exhaustive implementation inventory in this skill book.

| Surface | Primary entry point |
|---------|---------------------|
| JSON-RPC | `developers/rpc-reference.html` |
| WebSocket | `developers/ws-reference.html` |
| SDKs | `developers/sdk-js.html`, `developers/sdk-python.html`, `developers/sdk-rust.html` |
| Contracts | `developers/contracts.html`, `developers/contract-reference.html` |
| CLI | `developers/cli-reference.html` |
| Validator Ops | `developers/validator.html` |
| Identity & Privacy | `developers/lichenid.html`, `developers/zk-privacy.html` |
| Wallet / DEX / Explorer | `https://wallet.lichen.network`, `https://dex.lichen.network`, `https://explorer.lichen.network` |
| Marketplace / Programs / Faucet | `https://marketplace.lichen.network`, `https://programs.lichen.network`, `https://faucet.lichen.network` |

### Seed Validators

| Region | Host |
|--------|------|
| US (Virginia) | `seed-01.lichen.network` |
| EU (France) | `seed-02.lichen.network` |
| SEA (Singapore) | `seed-03.lichen.network` |

---

## 2. Architecture

### Core Components

| Component | Crate | Purpose |
|-----------|-------|---------|
| Core | `lobstercove-lichen-core` | State machine, accounts, transactions, WASM VM, ZK verifier, consensus |
| RPC | `lichen-rpc` | JSON-RPC server, REST API, WebSocket subscriptions |
| P2P | `lichen-p2p` | Gossip protocol, block propagation, validator announce |
| Validator | `lichen-validator` | Block production, slot scheduling, auto-update |
| CLI | `lichen-cli` | Command-line wallet tool |
| Compiler | `lichen-compiler` | Rust → WASM contract compilation pipeline |
| Custody | `lichen-custody` | Bridge custody with threshold treasury withdrawals on supported Solana/EVM paths and fail-closed multi-signer deposit issuance unless local sweeps are explicitly allowed |

### Transaction Format

```
Transaction {
  from: [u8; 32],           // Versioned native address digest
    recent_blockhash: [u8; 32],
    instructions: Vec<Instruction>,
  signatures: Vec<PqSignature>, // Self-contained PQ signatures
}

Instruction {
    program_id: [u8; 32],     // System (all zeros) or Contract (all 0xFF) or specific program
    accounts: Vec<[u8; 32]>,  // Account pubkeys involved
    data: Vec<u8>,            // Opcode + args (system) or JSON (contract)
}
```

**Wire format:** Bincode serialization → base64 for RPC transport.
**Routing:** First-byte heuristic distinguishes bincode vs JSON payloads.

### Special Program IDs

| Program | Address | Description |
|---------|---------|-------------|
| System Program | `11111111111111111111111111111111` (32 × `0x00`) | Native instructions (transfer, stake, NFT, ZK) |
| Contract Program | 32 × `0xFF` | WASM contract calls (Deploy, Call, Upgrade, Close) |

### Economic Parameters

| Parameter | Value |
|-----------|-------|
| Total supply | 500,000,000 LICN genesis + inflationary minting (4% initial, 0.15% floor) |
| Base fee | 0.001 LICN (1,000,000 spores) |
| Fee exemption | Protocol DEX contracts (dex_core, dex_amm, dex_router, dex_margin, dex_rewards, dex_governance, dex_analytics, lichenswap) are exempt from tx base fees — enables zero-LICN users to trade after bridging |
| Fee distribution | 40% burn, 30% block producer, 10% voters, 10% treasury, 10% community |
| DEX taker fee | 5 bps (0.05%) on notional, charged in quote token at fill |
| DEX maker rebate | −1 bps (0.01%) on notional, accrued and claimable via `claim_rebate` |
| DEX fee split | 60% protocol treasury, 20% LPs, 20% stakers |
| AMM swap fee | 1–100 bps per pool (fee tier), deducted from input token |
| AMM protocol fee | Configurable % of swap fee → protocol treasury (set at genesis via `set_pool_protocol_fee`) |
| Contract deploy fee | 25 LICN |
| Contract upgrade fee | 10 LICN |
| NFT mint fee | 0.5 LICN |
| NFT collection fee | 1,000 LICN |
| Slots per day | 216,000 |
| Slots per year | 78,840,000 |
| Epoch length | 432,000 slots (~2 days) |

---

## 3. Native Transaction Types

All system instructions use `program_id = System Program (all zeros)`. The first byte of `data` is the type tag.

| Type | Function | Data Layout | Accounts | Description |
|------|----------|-------------|----------|-------------|
| **0** | `system_transfer` | `[0x00, amount:u64 LE]` | `[from, to]` | Transfer LICN. Blocked for governed wallets (use 21/22). |
| **1** | `system_create_account` | `[0x01]` | `[pubkey]` | Create a new account. Fails if exists. |
| **2-5** | `system_transfer` (treasury) | Same as 0 | `[treasury, recipient]` | Fee-free internal transfers. Treasury-only. |
| **6** | `system_create_collection` | `[0x06, json_data...]` | `[creator, collection]` | Create NFT collection. |
| **7** | `system_mint_nft` | `[0x07, mint_data...]` | `[minter, collection, token, owner]` | Mint NFT. Enforces supply cap. |
| **8** | `system_transfer_nft` | `[0x08]` | `[owner, token, recipient]` | Transfer NFT ownership. |
| **9** | `system_stake` | `[0x09, amount:u64 LE]` | `[staker, validator]` | Stake LICN to validator. |
| **10** | `system_request_unstake` | `[0x0A, amount:u64 LE]` | `[staker, validator]` | Request unstake (staked → locked). |
| **11** | `system_claim_unstake` | `[0x0B]` | `[staker, validator]` | Claim after cooldown (locked → spendable). |
| **12** | `system_register_evm_address` | `[0x0C, evm_addr:20B]` | `[native_pubkey]` | Map EVM address to native key. One-to-one. |
| **13** | `system_mossstake_deposit` | `[0x0D, amount:u64 LE, tier:u8?]` | `[depositor]` | Liquid staking deposit. Mints stLICN. |
| **14** | `system_mossstake_unstake` | `[0x0E, st_licn_amount:u64 LE]` | `[user]` | Request stLICN unstake. 7-day cooldown. |
| **15** | `system_mossstake_claim` | `[0x0F]` | `[user]` | Claim unstaked LICN after cooldown. |
| **16** | `system_mossstake_transfer` | `[0x10, st_licn_amount:u64 LE]` | `[from, to]` | Transfer stLICN between accounts. |
| **17** | `system_deploy_contract` | `[0x11, code_len:u32 LE, code..., init...]` | `[deployer, treasury]` | Deploy WASM contract. Max 512KB. |
| **18** | `system_set_contract_abi` | `[0x12, abi_json...]` | `[owner, contract_id]` | Set contract ABI. Owner-only. |
| **19** | `system_faucet_airdrop` | `[0x13, amount_spores:u64 LE]` | `[treasury, recipient]` | Testnet faucet. Cap: 10 LICN/request, 150 LICN/day. |
| **20** | `system_register_symbol` | `[0x14, json...]` | `[owner, contract_id]` | Register contract symbol in registry. |
| **21** | `system_propose_governed_transfer` | `[0x15, amount:u64 LE]` | `[proposer, governed_wallet, recipient]` | Multi-sig transfer proposal. |
| **22** | `system_approve_governed_transfer` | `[0x16, proposal_id:u64 LE]` | `[approver]` | Approve multi-sig. Auto-executes at threshold. |
| **23** | `system_shield_deposit` | 169 bytes (see §10) | `[sender]` | ZK shield: transparent → shielded pool. |
| **24** | `system_unshield_withdraw` | 233 bytes (see §10) | `[recipient]` | ZK unshield: shielded pool → transparent. |
| **25** | `system_shielded_transfer` | 289 bytes (see §10) | (none — fully private) | ZK transfer: shielded → shielded. |
| **26** | `system_register_validator` | `[0x1A, stake:u64 LE]` | `[validator]` | Register new validator (internal, fee-free). |
| **27** | `system_slash_validator` | `[0x1B, evidence...]` | `[validator]` | Slash validator for equivocation (internal, fee-free). |
| **28** | `system_nonce` | `[0x1C, sub_opcode, ...]` | `[authority, nonce_account]` | Durable nonce (sub: 0=Initialize, 1=Advance, 2=Withdraw, 3=Authorize). |
| **29** | `system_governance_param_change` | `[0x1D, param_id:u8, value:u64 LE]` | `[governance_authority]` | Queue governance parameter change (applied at epoch boundary). |
| **30** | `system_oracle_attestation` | `[0x1E, asset_name_len:u8, asset_name, price:u64 LE]` | `[validator]` | Submit oracle price attestation (quorum >2/3 stake). |

---

## 4. Contract Call Format

WASM contracts are invoked via the Contract Program (`program_id = [0xFF; 32]`).

### Instruction Data (JSON)

```json
{
  "Call": {
    "function": "transfer",
    "args": [116, 111, ...],
    "value": 0
  }
}
```

- `function`: The WASM export name to call
- `args`: UTF-8 bytes of JSON-encoded arguments (e.g., `Array.from(TextEncoder.encode(JSON.stringify({to: [...], amount: 1000})))`)
- `value`: Spores to transfer from caller to contract **before** execution (0 for read-only)

### Other Contract Instructions

```json
{ "Deploy": { "code": [...wasm_bytes], "init_data": [...] } }
{ "Upgrade": { "code": [...wasm_bytes] } }
{ "Close": null }
```

### Account Layout for Contract Calls

- `accounts[0]` = caller (signer, pays fees)
- `accounts[1]` = contract address (target program)
- Additional accounts optional

### Two Dispatch Styles

| Style | Used By | Mechanism |
|-------|---------|-----------|
| **Named exports** | 22 standalone contracts | Each function is a `#[no_mangle] pub extern "C" fn name()` WASM export |
| **Opcode dispatch** | 7 DEX contracts | Single `call(args_ptr, args_len)` export; first byte = opcode |

---

## 5. Contract Surface (28 Contracts)

### Native Coin

**LICN** — Native coin (like ETH on Ethereum). Not a contract. Transferred via system program's native transfer instruction. Address sentinel: `[0u8; 32]` (zero address). 1 LICN = 1,000,000,000 spores.

### Token Contracts

**lusd_token** — Stablecoin (lUSD) with reserve attestation:
`initialize`, `mint`, `burn`, `transfer`, `approve`, `transfer_from`, `attest_reserves`, `balance_of`, `allowance`, `total_supply`, `total_minted`, `total_burned`, `get_reserve_ratio`, `get_last_attestation_slot`, `get_attestation_count`, `get_epoch_remaining`, `get_transfer_count`, `emergency_pause`, `emergency_unpause`, `transfer_admin`

**weth_token** / **wsol_token** / **wbnb_token** — Wrapped assets with reserve attestation:
Same exports as lusd_token.

### DeFi Contracts

**lichenswap** — LichenSwap AMM with flash loans and TWAP:
`initialize`, `add_liquidity`, `remove_liquidity`, `swap_a_for_b`, `swap_b_for_a`, `swap_a_for_b_with_deadline`, `swap_b_for_a_with_deadline`, `get_quote`, `get_reserves`, `get_liquidity_balance`, `get_total_liquidity`, `flash_loan_borrow`, `flash_loan_repay`, `flash_loan_abort`, `get_flash_loan_fee`, `get_twap_cumulatives`, `get_twap_snapshot_count`, `set_protocol_fee`, `get_protocol_fees`, `set_identity_admin`, `set_lichenid_address`, `set_reputation_discount`, `ms_pause`, `ms_unpause`, `create_pool`, `swap`, `get_pool_info`, `get_pool_count`, `set_platform_fee`, `get_swap_count`, `get_total_volume`, `get_swap_stats`

**thalllend** — Lending/borrowing with flash loans:
`initialize`, `deposit`, `withdraw`, `borrow`, `repay`, `liquidate`, `get_account_info`, `get_protocol_stats`, `flash_borrow`, `flash_repay`, `pause`, `unpause`, `set_deposit_cap`, `set_reserve_factor`, `withdraw_reserves`, `set_lichencoin_address`, `get_interest_rate`, `get_deposit_count`, `get_borrow_count`, `get_liquidation_count`, `get_platform_stats`

**sporepay** — Token streaming / vesting:
`create_stream`, `withdraw_from_stream`, `cancel_stream`, `get_stream`, `get_withdrawable`, `create_stream_with_cliff`, `transfer_stream`, `initialize_cp_admin`, `set_token_address`, `set_self_address`, `pause`, `unpause`, `get_stream_info`, `set_identity_admin`, `set_lichenid_address`, `set_identity_gate`, `get_stream_count`, `get_platform_stats`

**sporepump** — Token launchpad with bonding curves:
`initialize`, `create_token`, `buy`, `sell`, `get_token_info`, `get_buy_quote`, `get_token_count`, `get_platform_stats`, `pause`, `unpause`, `freeze_token`, `unfreeze_token`, `set_buy_cooldown`, `set_sell_cooldown`, `set_max_buy`, `set_creator_royalty`, `withdraw_fees`, `set_licn_token`, `set_dex_addresses`, `get_graduation_info`

**sporevault** — Yield vault with multi-strategy allocation:
`initialize`, `add_strategy`, `deposit`, `withdraw`, `set_protocol_addresses`, `set_licn_token`, `harvest`, `get_vault_stats`, `get_user_position`, `get_strategy_info`, `cv_pause`, `cv_unpause`, `set_deposit_fee`, `set_withdrawal_fee`, `set_deposit_cap`, `set_risk_tier`, `remove_strategy`, `withdraw_protocol_fees`, `update_strategy_allocation`

### Bridge & Cross-Chain

**lichenbridge** — Cross-chain bridge with multi-validator consensus:
`initialize`, `add_bridge_validator`, `remove_bridge_validator`, `set_required_confirmations`, `set_request_timeout`, `lock_tokens`, `submit_mint`, `confirm_mint`, `submit_unlock`, `confirm_unlock`, `cancel_expired_request`, `get_bridge_status`, `has_confirmed_mint`, `has_confirmed_unlock`, `is_source_tx_used`, `is_burn_proof_used`, `set_lichenid_address`, `set_identity_gate`, `set_token_address`, `mb_pause`, `mb_unpause`

### Oracle

**lichenoracle** — LichenOracle price feeds, randomness, attestation:
`initialize_oracle`, `add_price_feeder`, `set_authorized_attester`, `submit_price`, `get_price`, `commit_randomness`, `reveal_randomness`, `request_randomness`, `get_randomness`, `submit_attestation`, `verify_attestation`, `get_attestation_data`, `query_oracle`, `get_aggregated_price`, `get_oracle_stats`, `initialize`, `register_feed`, `get_feed_count`, `get_feed_list`, `add_reporter`, `remove_reporter`, `set_update_interval`, `mo_pause`, `mo_unpause`

### NFT & Marketplace

**lichenpunks** — LichenPunks NFT collection (ERC-721 equivalent):
`initialize`, `mint`, `transfer`, `owner_of`, `balance_of`, `approve`, `transfer_from`, `burn`, `total_minted`, `mint_punk`, `transfer_punk`, `get_owner_of`, `get_total_supply`, `get_punk_metadata`, `get_punks_by_owner`, `set_base_uri`, `set_max_supply`, `set_royalty`, `mp_pause`, `mp_unpause`, `get_collection_stats`

**lichenmarket** — LichenMarket NFT marketplace with offers, auctions, and collection offers:
`initialize`, `list_nft`, `buy_nft`, `cancel_listing`, `get_listing`, `set_marketplace_fee`, `list_nft_with_royalty`, `make_offer`, `cancel_offer`, `accept_offer`, `get_marketplace_stats`, `set_nft_attributes`, `get_nft_attributes`, `get_offer_count`, `update_listing_price`, `create_auction`, `place_bid`, `settle_auction`, `cancel_auction`, `get_auction`, `make_collection_offer`, `accept_collection_offer`, `cancel_collection_offer`, `make_offer_with_expiry`, `mm_pause`, `mm_unpause`

**lichenauction** — LichenAuction auction house:
`create_auction`, `place_bid`, `finalize_auction`, `make_offer`, `accept_offer`, `set_royalty`, `update_collection_stats`, `get_collection_stats`, `initialize`, `set_reserve_price`, `cancel_auction`, `initialize_ma_admin`, `ma_pause`, `ma_unpause`, `get_auction_info`, `get_auction_stats`

### Governance

**lichendao** — LichenDAO governance with treasury:
`initialize_dao`, `create_proposal`, `create_proposal_typed`, `vote`, `vote_with_reputation`, `execute_proposal`, `veto_proposal`, `cancel_proposal`, `treasury_transfer`, `get_treasury_balance`, `get_proposal`, `get_dao_stats`, `get_active_proposals`, `initialize`, `cast_vote`, `finalize_proposal`, `get_proposal_count`, `get_vote`, `get_vote_count`, `get_total_supply`, `set_quorum`, `set_voting_period`, `set_timelock_delay`, `dao_pause`, `dao_unpause`, `set_lichenid_address`

### Infrastructure

**bountyboard** — Decentralized bounty marketplace:
`create_bounty`, `submit_work`, `approve_work`, `cancel_bounty`, `get_bounty`, `set_identity_admin`, `set_lichenid_address`, `set_identity_gate`, `set_token_address`, `initialize`, `approve_submission`, `get_bounty_count`, `set_platform_fee`, `bb_pause`, `bb_unpause`, `get_platform_stats`

**compute_market** — Decentralized compute jobs:
`register_provider`, `submit_job`, `claim_job`, `complete_job`, `dispute_job`, `get_job`, `initialize`, `set_claim_timeout`, `set_complete_timeout`, `set_challenge_period`, `add_arbitrator`, `remove_arbitrator`, `set_token_address`, `cancel_job`, `release_payment`, `resolve_dispute`, `deactivate_provider`, `reactivate_provider`, `update_provider`, `get_escrow`, `set_identity_admin`, `set_lichenid_address`, `set_identity_gate`, `create_job`, `accept_job`, `submit_result`, `confirm_result`, `get_job_info`, `get_job_count`, `get_provider_info`, `set_platform_fee`, `cm_pause`, `cm_unpause`, `get_platform_stats`

**moss_storage** — Decentralized storage with staking and challenges:
`store_data`, `confirm_storage`, `get_storage_info`, `register_provider`, `claim_storage_rewards`, `initialize`, `set_licn_token`, `set_challenge_window`, `set_slash_percent`, `stake_collateral`, `set_storage_price`, `get_storage_price`, `get_provider_stake`, `issue_challenge`, `respond_challenge`, `slash_provider`, `get_platform_stats`

### Privacy

**shielded_pool** — ZK shielded transaction pool (WASM contract):
`initialize`, `shield`, `unshield`, `transfer`, `get_pool_stats`, `get_merkle_root`, `check_nullifier`, `get_commitments`, `pause`, `unpause`

Note: Shield/unshield/transfer also operate as native instruction types 23/24/25 in the processor with native transparent proof verification. The WASM contract provides queryable on-chain state.

### Prediction

**prediction_market** — Binary outcome prediction markets:
`initialize`, `call` (opcode-based dispatch)

### Identity

**lichenid** — LichenID identity system (see §7 for complete reference):
59 exported functions covering identity, names, reputation, vouches, achievements, skills, attestations, agent profiles, delegation, recovery, trust, and admin.

---

## 6. DEX Contracts — Full Opcode Reference

All 7 DEX contracts use binary opcode dispatch via a single `call(args_ptr, args_len)` WASM export. First byte = opcode.

### dex_core — Central Limit Order Book (34 opcodes)

Order types: Limit(0), Market(1), StopLimit(2), PostOnly(3). ReduceOnly flag: 0x80.
Fee defaults: maker −1bps rebate, taker 5bps. Distribution: 60% protocol / 20% LPs / 20% stakers.

| Op | Name | Args |
|----|------|------|
| 0x00 | `initialize` | `[admin 32B]` |
| 0x01 | `create_pair` | `[caller 32B][base 32B][quote 32B][tick_size 8B][lot_size 8B][min_order 8B]` |
| 0x02 | `place_order` | `[trader 32B][pair_id 8B][side 1B][order_type 1B][price 8B][quantity 8B][trigger_price 8B]` |
| 0x03 | `cancel_order` | `[trader 32B][order_id 8B]` |
| 0x04 | `set_preferred_quote` | `[caller 32B][quote_addr 32B]` |
| 0x05 | `get_pair_count` | `[]` |
| 0x06 | `get_preferred_quote` | `[]` |
| 0x07 | `update_pair_fees` | `[caller 32B][pair_id 8B][maker_fee 2B][taker_fee 2B]` |
| 0x08 | `emergency_pause` | `[caller 32B]` |
| 0x09 | `emergency_unpause` | `[caller 32B]` |
| 0x0A | `get_best_bid` | `[pair_id 8B]` |
| 0x0B | `get_best_ask` | `[pair_id 8B]` |
| 0x0C | `get_spread` | `[pair_id 8B]` |
| 0x0D | `get_pair_info` | `[pair_id 8B]` |
| 0x0E | `get_trade_count` | `[pair_id 8B]` |
| 0x0F | `get_fee_treasury` | `[]` |
| 0x10 | `modify_order` | `[trader 32B][order_id 8B][new_price 8B][new_quantity 8B]` |
| 0x11 | `cancel_all_orders` | `[trader 32B][pair_id 8B]` |
| 0x12 | `pause_pair` | `[caller 32B][pair_id 8B]` |
| 0x13 | `unpause_pair` | `[caller 32B][pair_id 8B]` |
| 0x14 | `get_order` | `[order_id 8B]` |
| 0x15 | `add_allowed_quote` | `[caller 32B][quote_addr 32B]` |
| 0x16 | `remove_allowed_quote` | `[caller 32B][quote_addr 32B]` |
| 0x17 | `get_allowed_quote_count` | `[]` |
| 0x18 | `execute_unpause` | `[caller 32B][pair_id 8B]` |
| 0x19 | `get_total_volume` | `[pair_id 8B]` |
| 0x1A | `get_user_orders` | `[trader 32B][pair_id 8B]` |
| 0x1B | `get_open_order_count` | `[pair_id 8B]` |
| 0x1C | `set_analytics_address` | `[caller 32B][analytics_addr 32B]` |
| 0x1D | `check_triggers` | `[pair_id 8B][current_price 8B]` |
| 0x1E | `set_margin_address` | `[caller 32B][margin_addr 32B]` |
| 0x1F | `get_check_triggers_result` | `[pair_id 8B]` |
| 0x20 | `set_fee_treasury_address` | `[caller 32B][treasury 32B]` |
| 0x21 | `claim_rebate` | `[caller 32B][pair_id 8B]` |

### dex_amm — Concentrated Liquidity AMM (22 opcodes)

Uniswap V3-style with Q32.32 fixed-point sqrt prices. Fee tiers: 1bps (tick 1), 5bps (tick 10), 30bps (tick 60), 100bps (tick 200). MAX_TICK: ±443,636.

| Op | Name | Args |
|----|------|------|
| 0x00 | `initialize` | `[admin 32B]` |
| 0x01 | `create_pool` | `[caller 32B][tokenA 32B][tokenB 32B][fee_tier 1B][initial_sqrt_price 8B]` |
| 0x02 | `set_pool_protocol_fee` | `[caller 32B][pool_id 8B][fee_bps 2B]` |
| 0x03 | `add_liquidity` | `[provider 32B][pool_id 8B][lower_tick 4B][upper_tick 4B][amountA 8B][amountB 8B]` |
| 0x04 | `remove_liquidity` | `[provider 32B][position_id 8B][amountA 8B][amountB 8B]` |
| 0x05 | `collect_fees` | `[provider 32B][position_id 8B]` |
| 0x06 | `swap_exact_in` | `[trader 32B][pool_id 8B][is_token_a_in 1B][amount_in 8B][min_out 8B][deadline 8B]` |
| 0x07 | `swap_exact_out` | `[trader 32B][pool_id 8B][is_token_a_in 1B][amount_out 8B][max_in 8B][deadline 8B]` |
| 0x08 | `emergency_pause` | `[caller 32B]` |
| 0x09 | `emergency_unpause` | `[caller 32B]` |
| 0x0A | `get_pool_info` | `[pool_id 8B]` |
| 0x0B | `get_position` | `[position_id 8B]` |
| 0x0C | `get_pool_count` | `[]` |
| 0x0D | `get_position_count` | `[]` |
| 0x0E | `get_tvl` | `[pool_id 8B]` |
| 0x0F | `quote_swap` | `[pool_id 8B][is_token_a_in 1B][amount 8B]` |
| 0x10 | `get_total_volume` | `[]` |
| 0x11 | `get_swap_count` | `[]` |
| 0x12 | `get_total_fees_collected` | `[]` |
| 0x13 | `get_amm_stats` | `[]` |
| 0x14 | `set_fee_treasury_address` | `[caller 32B][treasury 32B]` |
| 0x15 | `collect_protocol_fees` | `[caller 32B][pool_id 8B]` |

### dex_margin — Margin Trading (29 opcodes)

Leverage up to 100x with 7 tiered parameter sets. Funding: 8h intervals (28,800 slots).

**Leverage Tiers:**

| Tier | Max Leverage | Init Margin | Maint Margin | Liquidation Fee |
|------|-------------|-------------|--------------|-----------------|
| 1 | ≤2x | 50% | 25% | 3% |
| 2 | ≤3x | 33% | 16% | 4% |
| 3 | ≤5x | 20% | 10% | 5% |
| 4 | ≤10x | 10% | 5% | 7% |
| 5 | ≤25x | 4% | 2% | 10% |
| 6 | ≤50x | 2% | 1% | 12% |
| 7 | ≤100x | 1% | 0.5% | 15% |

| Op | Name | Args |
|----|------|------|
| 0x00 | `initialize` | `[admin 32B]` |
| 0x01 | `set_mark_price` | `[caller 32B][pair_id 8B][price 8B]` |
| 0x02 | `open_position` | `[trader 32B][pair_id 8B][side 1B][size 8B][leverage 8B][margin 8B][margin_mode 1B?]` |
| 0x03 | `close_position` | `[trader 32B][position_id 8B]` |
| 0x04 | `add_margin` | `[trader 32B][position_id 8B][amount 8B]` |
| 0x05 | `remove_margin` | `[trader 32B][position_id 8B][amount 8B]` |
| 0x06 | `liquidate` | `[liquidator 32B][position_id 8B]` |
| 0x07 | `set_max_leverage` | `[caller 32B][pair_id 8B][max_lev 8B]` |
| 0x08 | `set_maintenance_margin` | `[caller 32B][pair_id 8B][margin 8B]` |
| 0x09 | `withdraw_insurance` | `[caller 32B][amount 8B]` |
| 0x0A | `get_position_info` | `[position_id 8B]` |
| 0x0B | `get_margin_ratio` | `[position_id 8B]` |
| 0x0C | `get_tier_info` | `[tier 1B]` |
| 0x0D | `emergency_pause` | `[caller 32B]` |
| 0x0E | `emergency_unpause` | `[caller 32B]` |
| 0x0F | `set_lichencoin_address` | `[caller 32B][addr 32B]` |
| 0x10 | `get_total_volume` | `[]` |
| 0x11 | `get_user_positions` | `[trader 32B]` |
| 0x12 | `get_total_pnl` | `[]` |
| 0x13 | `get_liquidation_count` | `[]` |
| 0x14 | `get_margin_stats` | `[]` |
| 0x15 | `enable_margin_pair` | `[caller 32B][pair_id 8B]` |
| 0x16 | `disable_margin_pair` | `[caller 32B][pair_id 8B]` |
| 0x17 | `is_margin_enabled` | `[pair_id 8B]` |
| 0x18 | `set_position_sl_tp` | `[caller 32B][position_id 8B][sl_price 8B][tp_price 8B]` |
| 0x19 | `partial_close` | `[caller 32B][position_id 8B][close_amount 8B]` |
| 0x1A | `query_user_open_position` | `[trader 32B][pair_id 8B]` |
| 0x1B | `close_position_limit` | `[trader 32B][position_id 8B][limit_price 8B]` |
| 0x1C | `partial_close_limit` | `[caller 32B][position_id 8B][close_amount 8B][limit_price 8B]` |

### dex_router — Smart Order Routing (14 opcodes)

Route types: DIRECT_CLOB(0), DIRECT_AMM(1), SPLIT(2), MULTI_HOP(3). Max hops: 4. Max split legs: 3. Max slippage: 500bps (5%).

| Op | Name | Args |
|----|------|------|
| 0x00 | `initialize` | `[admin 32B]` |
| 0x01 | `set_addresses` | `[caller 32B][core_addr 32B][amm_addr 32B][legacy_addr 32B]` |
| 0x02 | `register_route` | `[caller 32B][tokenIn 32B][tokenOut 32B][type 1B][poolId 8B][secId 8B][splitPct 1B]` |
| 0x03 | `swap` | `[trader 32B][tokenIn 32B][tokenOut 32B][amountIn 8B][minOut 8B][deadline 8B]` |
| 0x04 | `set_route_enabled` | `[caller 32B][routeId 8B][enabled 1B]` |
| 0x05 | `get_best_route` | `[tokenIn 32B][tokenOut 32B]` |
| 0x06 | `get_route_info` | `[routeId 8B]` |
| 0x07 | `emergency_pause` | `[caller 32B]` |
| 0x08 | `emergency_unpause` | `[caller 32B]` |
| 0x09 | `multi_hop_swap` | `[trader 32B][path Vec<32B>][pathCount 1B][amountIn 8B][minOut 8B][deadline 8B]` |
| 0x0A | `get_route_count` | `[]` |
| 0x0B | `get_swap_count` | `[]` |
| 0x0C | `get_total_volume_routed` | `[]` |
| 0x0D | `get_router_stats` | `[]` |

### dex_governance — Governance (20 opcodes)

Voting: 48h (172,800 slots), 66% approval, MIN_QUORUM=3, 1h timelock. Min reputation: 500 (LichenID). Min listing liquidity: 10,000 LICN.

| Op | Name | Args |
|----|------|------|
| 0x00 | `initialize` | `[admin 32B]` |
| 0x01 | `propose_new_pair` | `[caller 32B][base 32B][quote 32B][tick_size 8B][lot_size 8B][min_order 8B]` |
| 0x02 | `vote` | `[caller 32B][proposalId 8B][approve 1B]` |
| 0x03 | `finalize_proposal` | `[caller 32B][proposalId 8B]` |
| 0x04 | `execute_proposal` | `[caller 32B][proposalId 8B]` |
| 0x05 | `set_preferred_quote` | `[caller 32B][quote_addr 32B]` |
| 0x06 | `get_preferred_quote` | `[]` |
| 0x07 | `get_proposal_count` | `[]` |
| 0x08 | `get_proposal_info` | `[proposalId 8B]` |
| 0x09 | `propose_fee_change` | `[caller 32B][pairId 8B][maker 2B][taker 2B]` |
| 0x0A | `emergency_delist` | `[caller 32B][pairId 8B]` |
| 0x0B | `set_listing_requirements` | `[caller 32B][min_liq 8B][min_rep 8B]` |
| 0x0C | `emergency_pause` | `[caller 32B]` |
| 0x0D | `emergency_unpause` | `[caller 32B]` |
| 0x0E | `set_lichenid_address` | `[caller 32B][addr 32B]` |
| 0x0F | `add_allowed_quote` | `[caller 32B][quote_addr 32B]` |
| 0x10 | `remove_allowed_quote` | `[caller 32B][quote_addr 32B]` |
| 0x11 | `get_allowed_quote_count` | `[]` |
| 0x12 | `get_governance_stats` | `[]` |
| 0x13 | `get_voter_count` | `[]` |

### dex_rewards — Trading Rewards (20 opcodes)

Fee mining, LP mining, referral program. Reward pool: 100K LICN/month (1,200,000 LICN total).

**Trading Tiers:**

| Tier | Volume Threshold | Multiplier |
|------|-----------------|------------|
| Bronze | < 100K | 1.0× |
| Silver | 100K – 1M | 1.5× |
| Gold | 1M – 10M | 2.0× |
| Diamond | > 10M | 3.0× |

**Referral:** 10% default (max 30%), 5% discount to referee, 15% for LichenID-verified.

| Op | Name | Args |
|----|------|------|
| 0x00 | `initialize` | `[admin 32B]` |
| 0x01 | `record_trade` | `[trader 32B][fee 8B][volume 8B]` |
| 0x02 | `claim_trading_rewards` | `[trader 32B]` |
| 0x03 | `claim_lp_rewards` | `[provider 32B]` |
| 0x04 | `register_referral` | `[trader 32B][referrer 32B]` |
| 0x05 | `set_reward_rate` | `[caller 32B][rate 8B]` |
| 0x06 | `accrue_lp_rewards` | `[provider 32B][amount 8B]` |
| 0x07 | `get_pending_rewards` | `[trader 32B]` |
| 0x08 | `get_trading_tier` | `[trader 32B]` |
| 0x09 | `emergency_pause` | `[caller 32B]` |
| 0x0A | `emergency_unpause` | `[caller 32B]` |
| 0x0B | `set_referral_rate` | `[caller 32B][rate 8B]` |
| 0x0C | `set_lichencoin_address` | `[caller 32B][addr 32B]` |
| 0x0D | `set_rewards_pool` | `[caller 32B][pool_addr 32B]` |
| 0x0E | `get_referral_rate` | `[]` |
| 0x0F | `get_total_distributed` | `[]` |
| 0x10 | `get_trader_count` | `[]` |
| 0x11 | `get_total_volume` | `[]` |
| 0x12 | `get_reward_stats` | `[]` |
| 0x13 | `claim_referral_rewards` | `[trader 32B]` |

### dex_analytics — OHLCV Aggregation (13 opcodes)

9 candle intervals: 1m/5m/15m/1h/4h/1d/3d/1w/1y.

| Interval | Retention | Max Candles |
|----------|-----------|-------------|
| 1m | 24h | 1,440 |
| 5m | 7d | 2,016 |
| 15m | 30d | 2,880 |
| 1h | 90d | 2,160 |
| 4h | 365d | 2,190 |
| 1d | 3y | 1,095 |

| Op | Name | Args |
|----|------|------|
| 0x00 | `initialize` | `[admin 32B]` |
| 0x01 | `record_trade` | `[pair_id 8B][price 8B][volume 8B][trader 32B]` |
| 0x02 | `get_ohlcv` | `[pair_id 8B][interval 8B][count 8B]` |
| 0x03 | `get_24h_stats` | `[pair_id 8B]` |
| 0x04 | `get_trader_stats` | `[trader 32B]` |
| 0x05 | `get_last_price` | `[pair_id 8B]` |
| 0x06 | `get_record_count` | `[]` |
| 0x07 | `emergency_pause` | `[caller 32B]` |
| 0x08 | `emergency_unpause` | `[caller 32B]` |
| 0x09 | `get_trader_count` | `[]` |
| 0x0A | `get_global_stats` | `[]` |
| 0x0B | `set_authorized_caller` | `[caller 32B][auth_addr 32B]` |
| 0x0C | `record_pnl` | `[trader 32B][pnl_biased 8B]` |

---

## 7. LichenID Identity System

LichenID is the decentralized identity layer. 51 WASM exports covering identity, naming, reputation, vouches, achievements, skills, attestations, agent profiles, delegation, and social recovery.

### Identity Registration

1. Call `register_identity(owner_ptr, agent_type, name_ptr, name_len)`
2. Validates: not paused, name 1-64 bytes, valid agent type, no duplicate
3. Initial reputation: **100**, cooldown: 60s between registrations
4. Record: 127 bytes — `[owner(32), agent_type(1), name_len(2), name(64), reputation(8), created_at(8), updated_at(8), skill_count(1), vouch_count(2)+flags(1), is_active(1)]`

### Agent Types

| ID | Type |
|----|------|
| 0 | Unknown |
| 1 | Trading |
| 2 | Development |
| 3 | Analysis |
| 4 | Creative |
| 5 | Infrastructure |
| 6 | Governance |
| 7 | Oracle |
| 8 | Storage |
| 9 | General |
| 10 | Personal |

### .lichen Name System

| Name Length | Cost | Mechanism |
|-------------|------|-----------|
| 3 chars | 500 LICN | Auction-only |
| 4 chars | 100 LICN | Auction-only |
| 5+ chars | 20 LICN | Direct registration |

- **Duration:** 1-10 years, cost = base × years
- **Expiry:** `current_slot + (78,840,000 × years)`
- **Validation:** 3-32 chars, lowercase a-z, 0-9, hyphens; no leading/trailing/consecutive hyphens
- **One name per identity**
- **~35 reserved names** (lichen, treasury, dex, admin, system, etc.)
- **Premium auctions:** 1-14 days (216,000–3,024,000 slots)

### Reputation System

| Parameter | Value |
|-----------|-------|
| Initial score | 100 |
| Minimum | 0 |
| Maximum | 100,000 |
| Decay period | 90 days |
| Decay rate | 5% per period |

**Contribution types:**

| Type | Action | Delta |
|------|--------|-------|
| 0 | successful_tx | +10 |
| 1 | governance_participation | +50 |
| 2 | program_deployed | +100 |
| 3 | uptime_hour | +1 |
| 4 | peer_endorsement | +25 |
| 5 | failed_tx | −5 |
| 6 | slashing_event | −100 |

### Vouch System

| Parameter | Value |
|-----------|-------|
| Voucher cost | −5 rep (voucher pays) |
| Vouchee reward | +10 rep (vouchee gains) |
| Max vouches/identity | 64 |
| Cooldown | 1 hour |
| Self-vouch | Forbidden |
| Duplicate vouch | Forbidden |

### Skills & Attestations

- Max 16 skills, max 32-byte name, proficiency 0-100
- +10 rep per skill added
- Third-party attestations: level 1-5
- Cannot self-attest; both parties need LichenID

### Delegation System

Owner grants bitfield permissions to delegates with TTL (max 1 year):
- `0b0001` — PROFILE (endpoint, metadata, availability, rate)
- `0b0010` — AGENT_TYPE
- `0b0100` — SKILLS
- `0b1000` — NAMING (transfer, renew, release names)

### Recovery Guardians

- **5 guardians, 3-of-5 threshold**
- Each guardian must have previously vouched for the target
- Flow: `set_recovery_guardians` → 3× `approve_recovery` → `execute_recovery`
- Old identity deactivated, new owner inherits everything

### Complete LichenID Exports (59 functions)

**Identity:** `initialize`, `register_identity`, `get_identity`, `deactivate_identity`, `get_identity_count`
**Reputation:** `update_reputation`, `update_reputation_typed`, `get_reputation`
**Agent type:** `update_agent_type`, `update_agent_type_as`
**Vouches:** `vouch`, `get_vouches`
**Achievements:** `award_contribution_achievement`, `get_achievements`
**Skills:** `add_skill`, `add_skill_as`, `get_skills`, `attest_skill`, `get_attestations`, `revoke_attestation`
**Names:** `register_name`, `resolve_name`, `reverse_resolve`, `create_name_auction`, `bid_name_auction`, `finalize_name_auction`, `get_name_auction`, `transfer_name`, `renew_name`, `release_name`, `transfer_name_as`, `renew_name_as`, `release_name_as`, `admin_register_reserved_name`
**Agent profile:** `set_endpoint`, `get_endpoint`, `set_metadata`, `get_metadata`, `set_availability`, `get_availability`, `set_rate`, `get_rate`, `get_agent_profile`
**Trust:** `get_trust_tier`
**Delegation:** `set_delegate`, `revoke_delegate`, `get_delegate`, `set_endpoint_as`, `set_metadata_as`, `set_availability_as`, `set_rate_as`
**Recovery:** `set_recovery_guardians`, `approve_recovery`, `execute_recovery`
**Admin:** `mid_pause`, `mid_unpause`, `transfer_admin`, `set_mid_token_address`, `set_mid_self_address`
---

## 8. Achievement System (90+ Achievements)

Achievements are auto-detected by `detect_and_award_achievements()` in the processor after every successful transaction. They require the sender to have a LichenID identity.

### General

| ID | Name | Trigger |
|----|------|---------|
| 1 | First Transaction | Any successful tx |
| 106 | Big Spender | Transfer ≥100 LICN |
| 107 | Whale Transfer | Transfer ≥1,000 LICN |
| 124 | Contract Interactor | Any contract call |

### DEX

| ID | Name | Trigger |
|----|------|---------|
| 13 | First Trade | Any swap on DEX/LICHENSWAP |
| 14 | LP Provider | Add liquidity |
| 15 | LP Withdrawal | Remove liquidity |
| 16 | DEX User | Any DEX interaction |
| 17 | Multi-hop Trader | Use DEX_ROUTER |
| 18 | Margin Trader | Open margin position |
| 19 | Position Closer | Close margin position |
| 20 | Yield Farmer | Claim DEX rewards |
| 21 | Analytics Explorer | Use DEX_ANALYTICS |

### Lending

| ID | Name | Trigger |
|----|------|---------|
| 31 | First Lend | Deposit to THALLLEND |
| 32 | First Borrow | Borrow from THALLLEND |
| 33 | Loan Repaid | Repay loan |
| 34 | Liquidator | Liquidate position |
| 35 | Withdrawal Expert | Withdraw from THALLLEND |

### Stablecoin

| ID | Name | Trigger |
|----|------|---------|
| 36 | Stablecoin Minter | Mint lUSD |
| 37 | Stablecoin Redeemer | Burn lUSD |
| 38 | Stable Sender | Transfer lUSD |

### Staking

| ID | Name | Trigger |
|----|------|---------|
| 41 | First Stake | System stake |
| 42 | Unstaked | System unstake |
| 43 | MossStake Pioneer | First MossStake deposit |
| 44 | Locked Staker | Deposit with tier ≥1 |
| 45 | Diamond Hands | 365-day lock tier |
| 46 | Whale Staker | Deposit ≥10,000 LICN |
| 47 | Reward Harvester | Claim MossStake rewards |
| 48 | stLICN Transferrer | Transfer stLICN |

### Bridge & Cross-Chain

| ID | Name | Trigger |
|----|------|---------|
| 51 | Bridge Pioneer (In) | LICHENBRIDGE deposit/lock |
| 52 | Bridge Out | LICHENBRIDGE withdraw/claim |
| 53 | Bridge User | Any LICHENBRIDGE call |
| 54 | Wrapper | Wrap WETH/WBNB/WSOL |
| 55 | Unwrapper | Unwrap WETH/WBNB/WSOL |
| 56 | Cross-chain Trader | Transfer wrapped asset |
| 108 | EVM Connected | Register EVM address |

### Privacy (ZK)

| ID | Name | Trigger |
|----|------|---------|
| 57 | Privacy Pioneer | First shield deposit |
| 58 | Unshielded | First unshield |
| 59 | Shadow Sender | First shielded transfer |
| 60 | ZK Privacy User | Interact with SHIELDED_POOL |

### NFT & Marketplace

| ID | Name | Trigger |
|----|------|---------|
| 63 | Collection Creator | Create NFT collection |
| 64 | First Mint | Mint NFT |
| 65 | NFT Trader | Transfer NFT |
| 66 | First Listing | List on LICHENMARKET |
| 67 | First Purchase | Buy on LICHENMARKET |
| 68 | Bidder | Bid on LICHENMARKET |
| 69 | Deal Maker | Accept offer on LICHENMARKET |
| 70 | Punk Collector | Interact with LICHENPUNKS |

### Governance

| ID | Name | Trigger |
|----|------|---------|
| 2 | Governance Voter | Any DEX_GOVERNANCE/LICHENDAO vote |
| 3 | Program Builder | Deploy a contract |
| 71 | Proposal Creator | Create proposal |
| 72 | First Vote | Cast first vote |
| 73 | Delegator | Delegate votes |

### Oracle & Storage

| ID | Name | Trigger |
|----|------|---------|
| 81 | Oracle Reporter | Submit price to LICHENORACLE |
| 82 | Oracle User | Any LICHENORACLE call |
| 86 | File Uploader | Upload to MOSS_STORAGE |
| 87 | Data Retriever | Download from MOSS_STORAGE |
| 88 | Storage User | Any MOSS_STORAGE call |

### Auction & Bounty

| ID | Name | Trigger |
|----|------|---------|
| 91 | Auctioneer | Create LICHENAUCTION |
| 92 | Auction Bidder | Bid on LICHENAUCTION |
| 93 | Auction Winner | Claim/settle LICHENAUCTION |
| 96 | Bounty Poster | Post on BOUNTYBOARD |
| 97 | Bounty Hunter | Submit work on BOUNTYBOARD |
| 98 | Bounty Judge | Approve submission |

### Prediction Markets

| ID | Name | Trigger |
|----|------|---------|
| 101 | Market Maker | Create prediction market |
| 102 | First Prediction | Place prediction bet |
| 103 | Oracle Resolver | Resolve market |
| 104 | Prediction Winner | Claim winnings |

### Payments & Tokens

| ID | Name | Trigger |
|----|------|---------|
| 115 | Payment Creator | Create SporePay stream |
| 116 | First Payment | Send SporePay payment |
| 117 | Subscription Creator | Create SporePay subscription |
| 118 | Token Launcher | Launch token on SporePump |
| 119 | Early Buyer | Buy on SporePump |
| 120 | Token Seller | Sell on SporePump |
| 121 | Vault Depositor | Deposit to SporeVault |
| 122 | Vault Withdrawer | Withdraw from SporeVault |
| 123 | Token Contract User | Interact with a token contract |

### Compute & Identity

| ID | Name | Trigger |
|----|------|---------|
| 113 | Compute Provider | Register as compute provider |
| 114 | Compute Consumer | Submit compute job |
| 109 | Identity Created | Register LichenID identity |
| 110 | Profile Customizer | Update profile |
| 111 | Voucher | Give a vouch |
| 112 | Agent Creator | Create agent |
| 9 | Name Registrar | Register .lichen name |
| 12 | First Name | Register first .lichen name |

### Contract-Awarded (Reputation Milestones)

| ID | Name | Trigger |
|----|------|---------|
| 4 | Trusted Agent | Rep ≥500 |
| 5 | Veteran Agent | Rep ≥1,000 |
| 6 | Legendary Agent | Rep ≥5,000 |
| 7 | Well Endorsed | ≥10 vouches received |
| 10 | Skill Master | ≥5 skills |
| 11 | Social Butterfly | ≥3 vouches received |

---

## 9. Staking & MossStake

### Basic Validator Staking

| Operation | Type | Data | Accounts |
|-----------|------|------|----------|
| Stake | 9 | `[0x09, amount:u64 LE]` | `[staker, validator]` |
| Request unstake | 10 | `[0x0A, amount:u64 LE]` | `[staker, validator]` |
| Claim unstake | 11 | `[0x0B]` | `[staker, validator]` |

### MossStake — Liquid Staking

| Operation | Type | Data | Accounts |
|-----------|------|------|----------|
| Deposit | 13 | `[0x0D, amount:u64 LE, tier:u8?]` | `[depositor]` |
| Unstake | 14 | `[0x0E, st_licn_amount:u64 LE]` | `[user]` |
| Claim | 15 | `[0x0F]` | `[user]` |
| Transfer stLICN | 16 | `[0x10, st_licn_amount:u64 LE]` | `[from, to]` |

### Lock Tiers

| Tier | Byte | Lock Duration | APY Multiplier | Target APY |
|------|------|---------------|----------------|------------|
| Flexible | 0 | None (7-day unstake cooldown) | 1.0× | ~5% |
| 30-Day | 1 | 6,480,000 slots | 1.6× | ~8% |
| 180-Day | 2 | 38,880,000 slots | 2.4× | ~12% |
| 365-Day | 3 | 78,840,000 slots | 3.6× | ~18% |

### stLICN Mechanics

- Exchange rate: fixed-point with 1e9 precision, starts at 1.0
- Minting: `st_licn = (licn × PRECISION) / exchange_rate`
- Redemption: `licn = (st_licn × exchange_rate) / PRECISION`
- Auto-compound: `distribute_rewards()` increases exchange rate
- Block reward share: 10% of block rewards → MossStake pool
- Tier change: must withdraw and re-stake (no in-place change)

---

## 10. ZK Shielded Transactions

### Architecture

- **Proof system:** Transparent Plonky3 FRI (`plonky3-fri-poseidon2`)
- **Hash / commitment domain:** Poseidon2 over canonical 32-byte values
- **Commitments / nullifiers:** `Poseidon2(value, blinding)` and `Poseidon2(serial, spending_key)`
- **Merkle tree depth:** 20 (supports ~1M commitments)
- **Note encryption:** ChaCha20-Poly1305

### Runtime Model

The live shielded stack is fully native Plonky3/FRI. Validators, RPC endpoints, and wallet-facing pool stats do not require proving-key or verification-key bundles at boot. A fresh node only needs the standard binaries, contract artifacts, and state directories.

### Compute Costs

| Operation | Compute Units |
|-----------|---------------|
| Shield | 100,000 |
| Unshield | 150,000 |
| Transfer | 200,000 |

### Type 23 — Shield Deposit (transparent → shielded)

```
Data (41-byte prefix + variable proof payload):
  [0]       = 0x17 (23)
  [1..9]    = amount (u64 LE, spores)
  [9..41]   = commitment (32B, Poseidon2 hash of value‖blinding)
  [41..]    = Plonky3 proof payload (variable length)

Public inputs: canonical Goldilocks words for [amount, commitment]
Accounts: [sender]
```

Debits sender balance. Inserts commitment into Merkle tree. Increments `total_shielded`.

### Type 24 — Unshield Withdraw (shielded → transparent)

```
Data (105-byte prefix + variable proof payload):
  [0]        = 0x18 (24)
  [1..9]     = amount (u64 LE, spores)
  [9..41]    = nullifier (32B)
  [41..73]   = merkle_root (32B)
  [73..105]  = recipient_hash (32B, derived from recipient account bytes)
  [105..]    = Plonky3 proof payload (variable length)

Public inputs: [merkle_root, nullifier, amount, recipient_hash]
Accounts: [recipient]
```

Verifies root matches, nullifier unspent, recipient bound. Credits recipient. Marks nullifier spent.

### Type 25 — Shielded Transfer (shielded → shielded)

```
Data (161-byte prefix + variable proof payload):
  [0]         = 0x19 (25)
  [1..33]     = nullifier_a (32B)
  [33..65]    = nullifier_b (32B)
  [65..97]    = commitment_c (32B, output 0)
  [97..129]   = commitment_d (32B, output 1)
  [129..161]  = merkle_root (32B)
  [161..]     = Plonky3 proof payload (variable length)

Public inputs: [merkle_root, nullifier_a, nullifier_b, commitment_c, commitment_d]
Accounts: (none — fully private)
```

2-in-2-out private transfer. Spends two notes, creates two new commitments. Value conservation enforced by ZK circuit.

### Wallet-Side Key Derivation

```
spending_key = SHA-256(seed ‖ "lichen-shielded-spending-key-v1")
viewing_key  = SHA-256(spending_key ‖ "lichen-viewing-key-v1")
```

Note decryption: XOR cipher with viewing key, 104-byte notes.

---

## 11. RPC Methods

### Native Lichen JSON-RPC (`POST /`)

#### Core Blockchain

| Method | Params | Returns |
|--------|--------|---------|
| `getBalance` | `[pubkey]` | `{balance, spendable, staked, spores, licn}` |
| `getAccount` | `[pubkey]` | `{address, balance, owner, executable, nonce}` |
| `getBlock` | `[slot]` | `{slot, hash, parent_hash, transactions}` |
| `getLatestBlock` | none | Latest block JSON |
| `getSlot` | `[{commitment?}]` | Current slot (u64) |
| `getTransaction` | `[hash_hex]` | `{hash, status, slot, from, to, amount, return_code?, return_data?, contract_logs?}` |
| `getTransactionsByAddress` | `[pubkey, {limit?, before_slot?}]` | Array of tx summaries |
| `getTransactionProof` | `[hash_hex]` | `{slot, tx_index, tx_hash, root, proof: [{hash, direction}]}` |
| `getAccountTxCount` | `[pubkey]` | `{count}` |
| `getRecentTransactions` | `[{limit?}]` | Array of recent txs |
| `getTokenAccounts` | `[pubkey]` | Token accounts for owner |
| `sendTransaction` | `[base64_tx]` | `{signature}` |
| `confirmTransaction` | `[hash_hex]` | `{confirmed, slot, status}` |
| `simulateTransaction` | `[base64_tx]` | `{success, logs, error?}` |
| `getTotalBurned` | none | `{total_burned_spores}` |
| `getRecentBlockhash` | none | `{blockhash, slot}` |
| `health` | none | `{"status": "ok"}` |

#### Validators & Network

| Method | Params | Returns |
|--------|--------|---------|
| `getValidators` | none | Validator array (pubkey, stake, blocks) |
| `getValidatorInfo` | `[pubkey]` | Single validator detail |
| `getValidatorPerformance` | `[pubkey]` | Performance metrics |
| `getChainStatus` | none | `{slot, epoch, validators, tps}` |
| `getMetrics` | none | Full chain metrics |
| `getTreasuryInfo` | none | Treasury balances |
| `getPeers` | none | Connected peers |
| `getNetworkInfo` | none | `{peer_count, node_version, chain_id}` |
| `getClusterInfo` | none | Cluster topology |

#### Staking

| Method | Params | Returns |
|--------|--------|---------|
| `stake` | `[pubkey, amount]` | `{success, staked}` |
| `unstake` | `[pubkey, amount]` | `{success, unstaked}` |
| `getStakingStatus` | `[pubkey]` | `{staked, rewards, validator}` |
| `getStakingRewards` | `[pubkey]` | Reward history and unclaimed |
| `getStakingPosition` | `[user_pubkey]` | `{st_licn_amount, current_value, lock_tier}` |
| `getMossStakePoolInfo` | none | `{total_supply_st_licn, exchange_rate, apy, tiers}` |
| `getUnstakingQueue` | `[user_pubkey]` | `{pending_requests[], total_claimable}` |
| `getRewardAdjustmentInfo` | none | `{decay, APY, fee_split}` |

#### Configuration (Admin)

| Method | Params | Returns |
|--------|--------|---------|
| `getFeeConfig` | none | `{base_fee_spores, contract_deploy_fee_spores, nft_mint_fee_spores, fee_burn_percent, ...}` |
| `setFeeConfig` | `{base_fee_spores?, ...}` (admin-gated, percentages must sum to 100) | `{status: "ok"}` |
| `getRentParams` | none | `{rent_rate_spores_per_kb_month, rent_free_kb}` |
| `setRentParams` | `{rent_rate_spores_per_kb_month?, rent_free_kb?}` (admin-gated) | `{status: "ok"}` |

#### Contracts

| Method | Params | Returns |
|--------|--------|---------|
| `callContract` | `{contract, function, args?, from?}` or `[contract_base58, function_name, args_base64?, from?]` | `{success, returnData, returnCode, logs, error, computeUsed}` |
| `deployContract` | `[deployer_base58, code_base64, init_data_json, signature_hex]` (admin-gated) | `{program_id, deployer, code_size, deploy_fee, deploy_fee_licn}` |
| `upgradeContract` | `[owner_base58, contract_base58, code_base64, signature_hex]` (admin-gated) | `{program_id, owner, version, previous_version, code_size, upgrade_fee}` |
| `getContractInfo` | `[program_id]` | `{symbol, owner, version, abi}` |
| `getContractLogs` | `[program_id, {limit?}]` | Contract logs |
| `getContractAbi` | `[program_id]` | ABI JSON |
| `getAllContracts` | none | All deployed contracts |
| `getProgram` | `[program_id]` | Program metadata |
| `getProgramStats` | `[program_id]` | `{call_count, storage_size}` |
| `getPrograms` | `[{limit?, offset?}]` | Paginated list |
| `getProgramCalls` | `[program_id, {limit?}]` | Recent calls |
| `getProgramStorage` | `[program_id, key]` | Raw storage value |

#### LichenID (Identity)

| Method | Params | Returns |
|--------|--------|---------|
| `getLichenIdIdentity` | `[pubkey]` | `{name, avatar, bio, verified}` |
| `getLichenIdReputation` | `[pubkey]` | `{score, level}` |
| `getLichenIdSkills` | `[pubkey]` | `{skills: []}` |
| `getLichenIdVouches` | `[pubkey]` | `{vouches: []}` |
| `getLichenIdAchievements` | `[pubkey]` | `{achievements: []}` |
| `getLichenIdProfile` | `[pubkey]` | Full composite profile |
| `resolveLichenName` | `[name]` | `{pubkey}` |
| `reverseLichenName` | `[pubkey]` | `{name}` |
| `batchReverseLichenNames` | `[pubkey_array]` | `{names: {pubkey: name}}` |
| `searchLichenNames` | `[query, {limit?}]` | Matching names |
| `getLichenIdAgentDirectory` | `[{limit?, offset?}]` | Agent directory |
| `getLichenIdStats` | none | `{total_identities, total_vouches}` |
| `getNameAuction` | `[name]` | Auction state |

#### NFT & Marketplace

| Method | Params | Returns |
|--------|--------|---------|
| `getCollection` | `[id]` | Collection metadata |
| `getNFT` | `[collection_id, token_id]` | NFT metadata, owner |
| `getNFTsByOwner` | `[owner, {limit?}]` | Array of NFTs |
| `getNFTsByCollection` | `[collection_id, {limit?}]` | NFTs in collection |
| `getNFTActivity` | `[collection_id, {limit?}]` | Activity history |
| `getMarketListings` | `[{limit?}]` | Active listings |
| `getMarketSales` | `[{limit?}]` | Recent sales |

#### Tokens

| Method | Params | Returns |
|--------|--------|---------|
| `getTokenBalance` | `[token_program, holder]` | `{balance, decimals, symbol}` |
| `getTokenHolders` | `[token_program, limit?]` | `{holders: [{holder, balance}]}` |
| `getTokenTransfers` | `[token_program, limit?]` | `{transfers: [{from, to, amount}]}` |
| `getContractEvents` | `[program_id, limit?]` | `{events: [{name, data, slot}]}` |

#### Shielded Pool

| Method | Params | Returns |
|--------|--------|---------|
| `getShieldedPoolState` | none | `{merkle_root, commitment_count, total_shielded, zk_scheme, counters}` |
| `getShieldedPoolStats` | none | Alias of `getShieldedPoolState` |
| `getShieldedMerkleRoot` | none | `{merkle_root, commitment_count}` |
| `getShieldedMerklePath` | `[index]` | `{siblings, path_bits, root}` |
| `isNullifierSpent` | `[hex_hash]` | `{nullifier, spent: bool}` |
| `checkNullifier` | `[hex_hash]` | Alias of `isNullifierSpent` |
| `getShieldedCommitments` | `[{from?, limit?}]` | Paginated commitments (max 1000) |
| `computeShieldCommitment` | `[{amount, blinding}]` | `{amount, blinding, commitment}` |
| `generateShieldProof` | `[{amount, blinding}]` | `{type:"shield", amount, blinding, commitment, proof}` |
| `generateUnshieldProof` | `[{amount, merkle_root, recipient, blinding, serial, spending_key, merkle_path?, path_bits?}]` | `{type:"unshield", nullifier, recipient_hash, proof}` |
| `generateTransferProof` | `[{merkle_root, inputs:[{amount, blinding, serial, spending_key, merkle_path, path_bits} ×2], outputs:[{amount, blinding} ×2]}]` | `{type:"transfer", nullifiers[], output_commitments[], proof}` |

#### Platform Stats (per-contract)

| Method | Returns |
|--------|---------|
| `getDexCoreStats` | pair_count, order_count, trade_count, total_volume |
| `getDexAmmStats` | pool_count, position_count, swap_count |
| `getDexMarginStats` | position_count, total_volume, liquidation_count |
| `getDexRewardsStats` | trade_count, total_distributed |
| `getDexRouterStats` | route_count, swap_count, total_volume |
| `getDexAnalyticsStats` | record_count, total_candles |
| `getDexGovernanceStats` | proposal_count, total_votes |
| `getLichenSwapStats` | swap_count, volume_a, volume_b |
| `getThallLendStats` | deposits, borrows, reserves |
| `getSporePayStats` | stream_count, total_streamed |
| `getBountyBoardStats` | bounty_count, reward_volume |
| `getComputeMarketStats` | job_count, payment_volume |
| `getMossStorageStats` | data_count, total_bytes |
| `getLichenMarketStats` | listing_count, sale_volume |
| `getLichenAuctionStats` | auction_count, total_volume |
| `getLichenPunksStats` | total_minted, transfer_count |
| `getLusdStats` / `getWethStats` / `getWsolStats` | supply, minted, burned |
| `getSporeVaultStats` | total_assets, strategy_count |
| `getLichenBridgeStats` | validator_count, locked_amount |
| `getLichenDaoStats` | proposal_count |
| `getLichenOracleStats` | queries, feeds |

#### Prediction Markets

| Method | Params | Returns |
|--------|--------|---------|
| `getPredictionMarketStats` | none | `{total_markets, open_markets, volume}` |
| `getPredictionMarkets` | `[{category?, status?, limit?}]` | Paginated list |
| `getPredictionMarket` | `[market_id]` | Market + outcomes |
| `getPredictionPositions` | `[address]` | User positions |
| `getPredictionTraderStats` | `[address]` | `{volume, trade_count}` |
| `getPredictionLeaderboard` | `[{limit?}]` | Top traders |
| `getPredictionTrending` | none | Top 10 markets by 24h vol |

#### Proofs, Estimation & Archive (v0.4.0)

| Method | Params | Returns |
|--------|--------|---------|
| `getBlockCommit` | `[slot]` | `{slot, block_hash, commit_signatures[], commit_validator_count, bft_timestamp}` |
| `getAccountProof` | `[pubkey]` | `{pubkey, account, proof: {leaf_hash, siblings, path}, state_root}` |
| `estimateTransactionFee` | `[base64_tx]` | `{fee_spores, fee_licn, compute_units}` |
| `getAccountAtSlot` | `[pubkey, slot]` | Account state at historical slot (validator `--archive-mode`; pair with `--cold-store` for bounded retention) |
| `getOraclePrices` | none | `{prices: [{asset, price, confidence, slot, attestations}]}` |

#### EVM & Symbol Registry

| Method | Params |
|--------|--------|
| `getEvmRegistration` | `[pubkey]` |
| `lookupEvmAddress` | `[evm_addr]` |
| `getSymbolRegistry` | `[symbol]` |
| `getAllSymbolRegistry` | `[{limit?, offset?}]` |

#### Bridge Deposits (Custody Proxy)

| Method | Params | Returns |
|--------|--------|---------|
| `createBridgeDeposit` | `[{user_id, chain, asset, auth}]` where `auth={issued_at, expires_at, signature}` and the signature covers the bridge access message for `user_id`; sign a fresh auth for each new deposit request | Deposit object (address, status) |
| `getBridgeDeposit` | `[{deposit_id, user_id, auth}]` or `[deposit_id, {user_id, auth}]`; the current unexpired bridge auth can be reused for status lookups | Deposit object scoped to the authenticated `user_id` |

### Solana-Format JSON-RPC (`POST /solana-compat`)

> Accepts Lichen transactions in Solana wire format. Does **not** accept native Solana transactions.
> Legacy alias `/solana` is supported for backward compatibility.

| Method | Description |
|--------|-------------|
| `getLatestBlockhash` | `{blockhash, lastValidBlockHeight}` |
| `getRecentBlockhash` | Alias of `getLatestBlockhash` |
| `getBalance` | Lamports-style balance |
| `getAccountInfo` | Solana-format account |
| `getBlock` | Solana-format block |
| `getBlockHeight` | Block height |
| `getSignaturesForAddress` | Signature records |
| `getSignatureStatuses` | Status per signature |
| `getSlot` | Current slot |
| `getTransaction` | Solana-format tx |
| `sendTransaction` | Submit base64 tx |
| `getHealth` | `"ok"` |
| `getVersion` | `{"solana-core": "lichen"}` |

### Ethereum-Compatible JSON-RPC (`POST /evm`)

| Method | Description |
|--------|-------------|
| `eth_getBalance` | Hex balance in wei |
| `eth_sendRawTransaction` | Submit hex tx |
| `eth_call` | Simulate call |
| `eth_chainId` | Hex chain ID |
| `eth_blockNumber` | Hex block number |
| `eth_getTransactionReceipt` | Receipt |
| `eth_getTransactionByHash` | Tx details |
| `eth_accounts` | Returns `[]` (no server-side accounts) |
| `eth_gasPrice` | `"0x1"` (1 spore/gas) |
| `eth_maxPriorityFeePerGas` | Returns `"0x0"` |
| `eth_estimateGas` | Gas estimate |
| `eth_getCode` | Bytecode |
| `eth_getTransactionCount` | Nonce |
| `eth_getBlockByNumber` / `eth_getBlockByHash` | Block |
| `eth_getLogs` | Event logs (Keccak-256 topics) |
| `eth_getStorageAt` | Storage slot |
| `net_version` / `net_listening` | Network info |
| `web3_clientVersion` | `"Lichen/{version}"` |

---

## 12. REST API Endpoints

### DEX (`/api/v1/*`)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/pairs` | All trading pairs |
| GET | `/api/v1/pairs/:id` | Single pair |
| GET | `/api/v1/pairs/:id/orderbook` | Order book (bids/asks) |
| GET | `/api/v1/pairs/:id/trades` | Recent trades |
| GET | `/api/v1/pairs/:id/candles` | OHLCV candles |
| GET | `/api/v1/pairs/:id/stats` | 24h stats |
| GET | `/api/v1/pairs/:id/ticker` | Current ticker |
| GET | `/api/v1/tickers` | All tickers |
| GET | `/api/v1/orders?trader=&pair_id=` | Orders |
| POST | `/api/v1/orders` | Place order (builds tx) |
| GET | `/api/v1/orders/:id` | Single order |
| DELETE | `/api/v1/orders/:id` | Cancel order (builds tx) |
| GET | `/api/v1/pools` | AMM pools |
| GET | `/api/v1/pools/:id` | Pool detail |
| GET | `/api/v1/pools/positions?owner=` | LP positions |
| GET | `/api/v1/margin/positions?trader=` | Margin positions |
| GET | `/api/v1/margin/positions/:id` | Single position |
| POST | `/api/v1/margin/open` | Open margin position (builds tx) |
| POST | `/api/v1/margin/close` | Close margin position (builds tx) |
| GET | `/api/v1/margin/info` | Margin params |
| GET | `/api/v1/margin/enabled-pairs` | Margin-enabled pairs |
| GET | `/api/v1/margin/funding-rate` | Funding rates |
| POST | `/api/v1/router/swap` | Execute swap (builds tx) |
| POST | `/api/v1/router/quote` | Quote best route |
| GET | `/api/v1/routes` | All configured routes |
| GET | `/api/v1/leaderboard` | Top traders |
| GET | `/api/v1/traders/:addr/stats` | Trader stats |
| GET | `/api/v1/rewards/:addr` | Claimable rewards |
| GET | `/api/v1/governance/proposals` | Governance proposals |
| POST | `/api/v1/governance/proposals` | Create governance proposal (builds tx) |
| GET | `/api/v1/governance/proposals/:id` | Single proposal detail |
| POST | `/api/v1/governance/proposals/:id/vote` | Vote on proposal (builds tx) |
| GET | `/api/v1/stats/core` | CLOB stats |
| GET | `/api/v1/stats/amm` | AMM stats |
| GET | `/api/v1/stats/margin` | Margin stats |
| GET | `/api/v1/stats/router` | Router stats |
| GET | `/api/v1/stats/rewards` | Reward stats |
| GET | `/api/v1/stats/analytics` | Analytics stats |
| GET | `/api/v1/stats/governance` | Governance stats |
| GET | `/api/v1/stats/lichenswap` | LichenSwap stats |
| GET | `/api/v1/oracle/prices` | Oracle price feeds |

### Prediction Market (`/api/v1/prediction-market/*`)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/prediction-market/config` | Platform configuration |
| GET | `/api/v1/prediction-market/stats` | Market stats |
| GET | `/api/v1/prediction-market/markets` | Paginated markets |
| GET | `/api/v1/prediction-market/markets/:id` | Single market |
| GET | `/api/v1/prediction-market/markets/:id/price-history` | Price history |
| GET | `/api/v1/prediction-market/markets/:id/analytics` | Market analytics |
| GET | `/api/v1/prediction-market/positions?address=` | User positions |
| GET | `/api/v1/prediction-market/trades` | Recent trades |
| GET | `/api/v1/prediction-market/traders/:addr/stats` | Trader stats |
| GET | `/api/v1/prediction-market/leaderboard` | Top traders |
| GET | `/api/v1/prediction-market/trending` | Top 10 active |
| POST | `/api/v1/prediction-market/trade` | Execute trade (builds tx) |
| POST | `/api/v1/prediction-market/create` | Create market (builds tx) |
| POST | `/api/v1/prediction-market/create-template` | Create from template (builds tx) |

### Launchpad (`/api/v1/launchpad/*`)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/launchpad/config` | Platform configuration |
| GET | `/api/v1/launchpad/stats` | Platform stats |
| GET | `/api/v1/launchpad/tokens` | Token list |
| GET | `/api/v1/launchpad/tokens/:id` | Token detail |
| GET | `/api/v1/launchpad/tokens/:id/quote?amount=` | Buy quote |
| GET | `/api/v1/launchpad/tokens/:id/holders` | Holder lookup |

### Shielded Pool (`/api/v1/shielded/*`)

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/shielded/pool` | Pool state |
| GET | `/api/v1/shielded/merkle-root` | Merkle root |
| GET | `/api/v1/shielded/merkle-path/:index` | Merkle proof |
| GET | `/api/v1/shielded/nullifier/:hash` | Nullifier check |
| GET | `/api/v1/shielded/commitments` | Commitment list |
| POST | `/api/v1/shielded/shield` | Submit shield tx |
| POST | `/api/v1/shielded/unshield` | Submit unshield tx |
| POST | `/api/v1/shielded/transfer` | Submit private transfer tx |

---

## 13. WebSocket Subscriptions

Connect to `ws://localhost:8900` (local) or `wss://rpc.lichen.network/ws` (production). Send JSON-RPC subscribe messages.

### Core

| Subscribe | Unsubscribe | Params | Events |
|-----------|-------------|--------|--------|
| `subscribeSlots` / `slotSubscribe` | `unsubscribeSlots` | none | Slot notifications |
| `subscribeBlocks` | `unsubscribeBlocks` | none | Block notifications |
| `subscribeTransactions` | `unsubscribeTransactions` | none | Tx notifications |
| `subscribeAccount` | `unsubscribeAccount` | `{pubkey}` | Balance changes |
| `subscribeLogs` | `unsubscribeLogs` | `{program_id?}` | Contract logs |
| `subscribeSignatureStatus` | `unsubscribeSignatureStatus` | `{signature_hex}` | Tx confirmation |
| `subscribeEpochs` | `unsubscribeEpochs` | none | Epoch boundaries |

### Programs

| Subscribe | Params | Events |
|-----------|--------|--------|
| `subscribeProgramUpdates` | none | Deploy/upgrade events |
| `subscribeProgramCalls` | `{program_id?}` | Invocation events |

### NFT & Marketplace

| Subscribe | Params | Events |
|-----------|--------|--------|
| `subscribeNftMints` | `{collection_id?}` | Mint events |
| `subscribeNftTransfers` | `{collection_id?}` | Transfer events |
| `subscribeMarketListings` | none | New listings |
| `subscribeMarketSales` | none | Sale events |

### Bridge

| Subscribe | Events |
|-----------|--------|
| `subscribeBridgeLocks` | Lock events |
| `subscribeBridgeMints` | Mint events |

### DEX

| Subscribe | Params |
|-----------|--------|
| `subscribeDex` | `{channel: "orderbook:<pair_id>"}` |
| `subscribeDex` | `{channel: "trades:<pair_id>"}` |
| `subscribeDex` | `{channel: "ticker:<pair_id>"}` |
| `subscribeDex` | `{channel: "candles:<pair_id>:<interval>"}` |
| `subscribeDex` | `{channel: "orders:<trader_addr>"}` |
| `subscribeDex` | `{channel: "positions:<trader_addr>"}` |

### Prediction Markets

| Subscribe | Params |
|-----------|--------|
| `subscribePrediction` | `{channel: "all"}` or `{channel: "market:<id>"}` |

### Validators, Tokens, Governance

| Subscribe | Params | Events |
|-----------|--------|--------|
| `subscribeValidators` | none | Validator set changes |
| `subscribeTokenBalance` | `{owner, mint?}` | Token balance changes |
| `subscribeGovernance` | none | Governance events |

---

## 14. JavaScript SDK

**Package:** `@lobstercove/lichen-sdk` in `sdk/js/`

### Key Classes

```typescript
import { PublicKey, Keypair, Connection, TransactionBuilder } from '@lobstercove/lichen-sdk';

// Create wallet
const kp = Keypair.generate();
const pub = kp.pubkey();  // PublicKey

// Connect
// Local:   new Connection('http://localhost:8899', 'ws://localhost:8900');
// Production:
const conn = new Connection('https://rpc.lichen.network', 'wss://rpc.lichen.network/ws');

// Query
const balance = await conn.getBalance(pub.toBase58());
const slot = await conn.getSlot();

// Transfer
const tx = TransactionBuilder.transfer(kp.pubkey(), toPub, 1_000_000_000);
tx.setRecentBlockhash(await conn.getRecentBlockhash());
const signed = tx.buildAndSign(kp);
const sig = await conn.sendTransaction(btoa(JSON.stringify(signed)));

// Contract call
const callTx = new TransactionBuilder();
callTx.add({
  programId: Array.from(new Uint8Array(32).fill(0xFF)),  // Contract Program
  accounts: [Array.from(kp.pubkey().toBytes()), Array.from(contractPub.toBytes())],
  data: Array.from(new TextEncoder().encode(JSON.stringify({
    Call: { function: "transfer", args: Array.from(new TextEncoder().encode(
      JSON.stringify({ to: [...recipientBytes], amount: 1000 })
    )), value: 0 }
  })))
});

// WebSocket subscriptions
conn.onSlot((slot) => console.log('New slot:', slot));
conn.onBlock((block) => console.log('New block:', block));
conn.onAccountChange(address, (account) => console.log('Changed:', account));
```

### Wire Format (Bincode)

```
Transaction: [u64 sig_count][sig₁ 64B]...[u64 ix_count][ix₁]...[blockhash 32B]
Instruction: [programId 32B][u64 acct_count][acct₁ 32B]...[u64 data_len][data...]
```

---

## 15. Wallet Operations

### Key Generation

1. BIP39 12-word mnemonic (128-bit entropy)
2. PBKDF2-HMAC-SHA512 (mnemonic, "mnemonic" + passphrase, 2048 iterations)
3. First 32 bytes → ML-DSA-65 seed → keypair
4. Private key encrypted with AES-256-GCM via Web Crypto API

### Trust Boundaries

- Generic chain reads and transaction submission use the active RPC transport.
- Token registry, symbol resolution, and other browser control-plane metadata must go through `signedMetadataRpcCall()` / `getSignedMetadataManifest` and be verified against the pinned release signer in `monitoring/shared/utils.js` before the UI trusts them.
- Bridge control-plane reads stay pinned to trusted network endpoints and require wallet-signed bridge access proofs; they do not follow arbitrary custom RPC overrides.

### Transaction Building Flow

```
1. latestBlock = await rpc.getLatestBlock()
2. Build instruction data: Uint8Array with [opcode, ...args]
3. message = { instructions: [{program_id, accounts, data}], blockhash }
4. privateKey = LichenCrypto.decryptPrivateKey(encryptedKey, password)
5. messageBytes = serializeMessageBincode(message)
6. signature = LichenCrypto.signTransaction(privateKey, messageBytes)
7. tx = { signatures: [signature], message }
8. base64 = btoa(JSON.stringify(tx))
9. rpc.sendTransaction(base64)
```

### Token Transfers (lUSD, wSOL, wETH, wBNB)

```javascript
program_id = [0xFF × 32]  // CONTRACT_PROGRAM_ID
accounts   = [from_pubkey, token_contract_pubkey]
data       = JSON.stringify({
  Call: {
    function: "transfer",
    args: TextEncoder.encode(JSON.stringify({ to: [...toPubkey], amount: rawAmount })),
    value: 0
  }
})
```

### EVM Address Registration

Auto-derives 20-byte address via `Keccak256(pubkey)[12:32]` and sends type 12 tx on wallet creation/login. Cached in localStorage.

### Bridge Deposits (via Custody)

1. `POST /deposits` on the trusted custody endpoint with a freshly signed bridge access payload → deposit address
2. Poll `GET /deposits/:id` every 5s
3. Status: `issued → pending → confirmed → swept → credited`
4. Supported: Solana (SOL, USDC, USDT), Ethereum (ETH, USDC, USDT), BSC (BNB)

---

## 16. CLI Reference

Binary: **`lichen`**. Global: `--rpc-url` (default `http://localhost:8899`).

### Core Commands

```bash
lichen balance [address]                       # Check balance
lichen transfer <to> <amount>                  # Transfer LICN
lichen deploy <contract.wasm>                  # Deploy WASM contract
                                                 # flags: --symbol, --name, --template,
                                                 #   --decimals, --supply, --metadata (JSON)
lichen upgrade <address> <contract.wasm>       # Upgrade contract
lichen call <contract> <function> --args '[...]'  # Call contract function
lichen block <slot>                            # Get block info
lichen latest                                  # Get latest block
lichen slot                                    # Current slot
lichen blockhash                               # Recent blockhash
lichen burned                                  # Total burned LICN
lichen validators                              # List validators
lichen status                                  # Chain status
lichen metrics                                 # Performance metrics
```

### Wallet Management

```bash
lichen wallet create [name]                    # Create wallet
lichen wallet import <name> --keypair <path>   # Import wallet
lichen wallet list                             # List wallets
lichen wallet show <name>                      # Show details
lichen wallet balance <name>                   # Get balance
lichen wallet remove <name>                    # Remove wallet
```

### Identity & Keypair

```bash
lichen identity new --output <path>            # Create identity
lichen identity show --keypair <path>          # Show identity
lichen identity export --keypair <path>        # Decrypt + show pubkey/EVM address
lichen identity export --keypair <path> --reveal-seed  # Also show private seed (hex)
lichen init --output <path>                    # Init validator keypair (deprecated)
```

### Staking

```bash
lichen stake add <amount>                      # Stake LICN
lichen stake remove <amount>                   # Unstake LICN
lichen stake status                            # Staking status
lichen stake rewards                           # View rewards
```

### Governance

```bash
lichen gov propose <title> <desc>              # Create proposal
lichen gov vote <id> <yes/no/abstain>          # Vote
lichen gov list [--all]                        # List proposals
lichen gov info <id>                           # Proposal details
lichen gov execute <id>                        # Execute proposal
lichen gov veto <id>                           # Veto proposal
```

### Tokens

```bash
lichen token create <name> <symbol> --wasm <path>  # Deploy token WASM + register
lichen token info <token>                      # Token info
lichen token mint <token> <amount>             # Mint tokens
lichen token send <token> <to> <amount>        # Send tokens
lichen token balance <token>                   # Check balance
lichen token list                              # List tokens
```

### Account & Contract Inspection

```bash
lichen account info <address>                  # Account details
lichen account history <address> --limit N     # Transaction history
lichen contract info <address>                 # Contract details
lichen contract logs <address> --limit N       # Contract logs
lichen contract list                           # All contracts
lichen contract register <addr> --symbol X     # Register in symbol registry
lichen network status                          # Network status
lichen network peers                           # Connected peers
lichen validator info <address>                # Validator details
lichen validator performance <address>         # Validator performance
```

---

## 17. Validator Operations

### Agent Default: 1-Minute Install And Run

If an agent is asked to install or start a validator on a fresh machine, the default public path is:

1. Download the latest signed release bundle.
2. Extract `lichen-validator`, `lichen-genesis`, `lichen`, `zk-prove`, `seeds.json`, and the bundled `contracts/` WASM artifacts.
3. Create a writable state directory.
4. Set `LICHEN_KEYPAIR_PASSWORD`, install the bundled `seeds.json` into the state directory, and start mainnet with `--auto-update=apply`.

Do not default to `git clone` unless the user explicitly wants source checkout, development, or to modify code.

Linux x86_64:

```bash
VERSION=$(curl -fsSL https://api.github.com/repos/lobstercove/lichen/releases/latest | jq -r .tag_name)
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/lichen-validator-linux-x86_64.tar.gz"
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/SHA256SUMS"
grep 'lichen-validator-linux-x86_64.tar.gz' SHA256SUMS | sha256sum -c -
tar xzf lichen-validator-linux-x86_64.tar.gz --strip-components=1
chmod +x lichen-validator lichen-genesis lichen zk-prove
mkdir -p "$HOME/.lichen/state-mainnet"
cp seeds.json "$HOME/.lichen/state-mainnet/seeds.json"
export LICHEN_KEYPAIR_PASSWORD='set-a-long-random-secret-before-first-start'
./lichen-validator \
  --network mainnet \
  --p2p-port 8001 \
  --rpc-port 9899 \
  --ws-port 9900 \
  --db-path "$HOME/.lichen/state-mainnet" \
  --auto-update=apply
```

macOS Apple Silicon:

```bash
VERSION=$(curl -fsSL https://api.github.com/repos/lobstercove/lichen/releases/latest | jq -r .tag_name)
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/lichen-validator-darwin-aarch64.tar.gz"
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/SHA256SUMS"
grep 'lichen-validator-darwin-aarch64.tar.gz' SHA256SUMS | shasum -a 256 -c -
tar xzf lichen-validator-darwin-aarch64.tar.gz --strip-components=1
chmod +x lichen-validator lichen-genesis lichen zk-prove
mkdir -p "$HOME/.lichen/state-mainnet"
cp seeds.json "$HOME/.lichen/state-mainnet/seeds.json"
export LICHEN_KEYPAIR_PASSWORD='set-a-long-random-secret-before-first-start'
./lichen-validator \
  --network mainnet \
  --p2p-port 8001 \
  --rpc-port 9899 \
  --ws-port 9900 \
  --db-path "$HOME/.lichen/state-mainnet" \
  --auto-update=apply
```

Windows x64 (PowerShell):

```powershell
$version = (Invoke-RestMethod https://api.github.com/repos/lobstercove/lichen/releases/latest).tag_name
Invoke-WebRequest -Uri "https://github.com/lobstercove/lichen/releases/download/$version/lichen-validator-windows-x86_64.tar.gz" -OutFile "lichen-validator-windows-x86_64.tar.gz"
tar -xzf .\lichen-validator-windows-x86_64.tar.gz --strip-components=1
New-Item -ItemType Directory -Force -Path "$HOME\.lichen\state-mainnet" | Out-Null
Copy-Item .\seeds.json "$HOME\.lichen\state-mainnet\seeds.json" -Force
$env:LICHEN_KEYPAIR_PASSWORD = 'set-a-long-random-secret-before-first-start'
.\lichen-validator.exe `
  --network mainnet `
  --p2p-port 8001 `
  --rpc-port 9899 `
  --ws-port 9900 `
  --db-path "$HOME\.lichen\state-mainnet" `
  --auto-update=apply
```

### Join the Mainnet (Quick Start for Agents)

If the machine already has a `lichen-validator` binary, agents do not need the full repo checkout. They can join mainnet immediately with the binary plus a writable state directory:

Release bundles ship `lichen-validator`, `lichen-genesis`, `lichen`, `zk-prove`, `seeds.json`, and the genesis contract WASM bundle, and `--auto-update=apply` refreshes `{db-path}/seeds.json` from newer release archives during apply-mode upgrades.

Fresh validators do not need copied RocksDB state, `genesis-wallet.json`, `genesis-keys/`, custody keys, faucet keys, or metadata bundles. On first start the validator fetches the authoritative genesis config from the seed RPC endpoint in `seeds.json`, persists `{db-path}/genesis.json`, imports and verifies the opcode-41 block-0 state bundle, then syncs/replays later blocks from peers with its own validator identity.

```bash
mkdir -p "$HOME/.lichen/state-mainnet"
cp seeds.json "$HOME/.lichen/state-mainnet/seeds.json"
export LICHEN_KEYPAIR_PASSWORD='set-a-long-random-secret-before-first-start'

lichen-validator \
  --network mainnet \
  --p2p-port 8001 \
  --rpc-port 9899 \
  --ws-port 9900 \
  --db-path "$HOME/.lichen/state-mainnet" \
    --auto-update=apply
```

### Download A Release Instead Of Cloning

Preferred agent flow:
1. Detect platform.
2. Download the matching GitHub Release archive.
3. Extract `lichen-validator`, `lichen-genesis`, `lichen`, `zk-prove`, `seeds.json`, and the bundled `contracts/` WASM artifacts.
4. Create a writable state directory.
5. Set `LICHEN_KEYPAIR_PASSWORD`, copy the bundled `seeds.json` into that state directory, and start with `--auto-update=apply`.

Release URL template:

```text
https://github.com/lobstercove/lichen/releases/download/<tag>/lichen-validator-<platform>.tar.gz
```

Current platform names:
- `linux-x86_64`
- `linux-aarch64`
- `darwin-x86_64`
- `darwin-aarch64`
- `windows-x86_64`

Linux x86_64:

```bash
VERSION=$(curl -fsSL https://api.github.com/repos/lobstercove/lichen/releases/latest | jq -r .tag_name)
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/lichen-validator-linux-x86_64.tar.gz"
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/SHA256SUMS"
grep 'lichen-validator-linux-x86_64.tar.gz' SHA256SUMS | sha256sum -c -
tar xzf lichen-validator-linux-x86_64.tar.gz --strip-components=1
chmod +x lichen-validator lichen-genesis lichen zk-prove
mkdir -p "$HOME/.lichen/state-mainnet"
cp seeds.json "$HOME/.lichen/state-mainnet/seeds.json"
export LICHEN_KEYPAIR_PASSWORD='set-a-long-random-secret-before-first-start'
./lichen-validator \
  --network mainnet \
  --p2p-port 8001 \
  --rpc-port 9899 \
  --ws-port 9900 \
  --db-path "$HOME/.lichen/state-mainnet" \
  --auto-update=apply
```

macOS Apple Silicon:

```bash
VERSION=$(curl -fsSL https://api.github.com/repos/lobstercove/lichen/releases/latest | jq -r .tag_name)
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/lichen-validator-darwin-aarch64.tar.gz"
curl -LO "https://github.com/lobstercove/lichen/releases/download/${VERSION}/SHA256SUMS"
grep 'lichen-validator-darwin-aarch64.tar.gz' SHA256SUMS | shasum -a 256 -c -
tar xzf lichen-validator-darwin-aarch64.tar.gz --strip-components=1
chmod +x lichen-validator lichen-genesis lichen zk-prove
mkdir -p "$HOME/.lichen/state-mainnet"
cp seeds.json "$HOME/.lichen/state-mainnet/seeds.json"
export LICHEN_KEYPAIR_PASSWORD='set-a-long-random-secret-before-first-start'
./lichen-validator \
  --network mainnet \
  --p2p-port 8001 \
  --rpc-port 9899 \
  --ws-port 9900 \
  --db-path "$HOME/.lichen/state-mainnet" \
  --auto-update=apply
```

Windows x64 (PowerShell):

```powershell
$version = (Invoke-RestMethod https://api.github.com/repos/lobstercove/lichen/releases/latest).tag_name
Invoke-WebRequest -Uri "https://github.com/lobstercove/lichen/releases/download/$version/lichen-validator-windows-x86_64.tar.gz" -OutFile "lichen-validator-windows-x86_64.tar.gz"
tar -xzf .\lichen-validator-windows-x86_64.tar.gz --strip-components=1
New-Item -ItemType Directory -Force -Path "$HOME\.lichen\state-mainnet" | Out-Null
Copy-Item .\seeds.json "$HOME\.lichen\state-mainnet\seeds.json" -Force
$env:LICHEN_KEYPAIR_PASSWORD = 'set-a-long-random-secret-before-first-start'
.\lichen-validator.exe `
  --network mainnet `
  --p2p-port 8001 `
  --rpc-port 9899 `
  --ws-port 9900 `
  --db-path "$HOME\.lichen\state-mainnet" `
  --auto-update=apply
```

If a release tag predates Windows packaging, agents should fall back to the source-build workflow on Windows.

### What The Validator Does On First Start

Given a fresh `--db-path`, the validator:

1. Creates the state directory.
2. Generates or imports the validator identity there.
3. Persists validator-local runtime material in that directory.
4. Loads `seeds.json` from `{db-path}`, `/etc/lichen`, or the current directory.
5. Fetches and persists the authoritative `genesis.json` from the seed RPC endpoint when starting from an empty state directory.
6. Imports and verifies the opcode-41 block-0 state bundle against the block state root, then syncs/replays later blocks and peer graph from the network without copied RocksDB state or seed-node secrets.
7. Reuses that same identity and resumes from local state on restart if the state directory is preserved.

Agents should treat the state directory as persistent machine-local validator state. The repo checkout is optional; the state directory is not.

For unattended updates, agents should prefer running the validator under a restart supervisor or service manager. `--auto-update=apply` stages the signed replacement binary and requests a restart; it is not a substitute for process supervision.

If the machine only has source code, use the repo workflow below:

```bash
# 1. Prerequisites: Rust 1.88+
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# 2. Clone and build
git clone https://github.com/lobstercove/lichen.git
cd lichen
cargo build --release

# 3. Start a mainnet validator (bootstraps from seed nodes automatically)
cp ./seeds.json ./data/state-mainnet/seeds.json
./target/release/lichen-validator \
    --p2p-port 8001 \
    --rpc-port 9899 \
    --ws-port 9900 \
  --db-path ./data/state-mainnet
```

On first start the validator:
- Generates a keypair → saved to `./data/state-mainnet/validator-keypair.json`
- Syncs genesis + state from seed nodes
- Receives a 100K LICN bootstrap grant (for the first 200 validators)
- Begins block production after sync completes

The identity persists across restarts (stored in the data directory, not `$HOME`).

### Configuration Reference

| Flag | Default | Description |
|------|---------|-------------|
| `--p2p-port` | 7001 | P2P gossip port |
| `--rpc-port` | 8899 | JSON-RPC HTTP port |
| `--ws-port` | 8900 | WebSocket port |
| `--db-path` | `./data/state-{p2p_port}` | State database directory |
| `--keypair` | auto | Path to validator keypair JSON |
| `--import-key` | — | Import keypair from another machine |
| `--genesis` | — | Path to genesis config (only for genesis node) |
| `--listen-addr` | `0.0.0.0` | Bind address for all servers |
| `--dev-mode` | off | Disable machine fingerprint (testnet only) |
| `--auto-update` | check | `check` / `apply` / `off` |

Peer discovery is driven by `seeds.json` under `{db-path}`, `/etc/lichen/seeds.json`, or the extracted working directory.
| `--max-restarts` | 50 | Max auto-restarts before giving up |

### Seed Validators (Mainnet)

| Region | Address |
|--------|---------|
| US East (Virginia) | `seed-01.lichen.network:8001` |
| EU West (France) | `seed-02.lichen.network:8001` |
| AP Southeast (Singapore) | `seed-03.lichen.network:8001` |

Prefer domains over raw IPs in agent prompts and operational scripts. DNS lets bootstrap infrastructure move without changing the validator command or republishing the binary.

### RPC Endpoints (Mainnet Production)

| Service | URL |
|---------|-----|
| RPC | `https://rpc.lichen.network` |
| WebSocket | `wss://rpc.lichen.network/ws` |

### Testnet (for Development)

```bash
./target/release/lichen-validator \
    --p2p-port 7001 \
    --rpc-port 8899 \
    --ws-port 8900 \
    --db-path ./data/state-testnet \
  --dev-mode
```

### Mainnet

```bash
./target/release/lichen-validator \
    --p2p-port 8001 \
    --rpc-port 9899 \
    --ws-port 9900 \
  --db-path ./data/state-mainnet
```

### Start / Stop / Reset (Scripts)

```bash
bash scripts/start-local-3validators.sh start-reset   # Fresh local 3-validator cluster
bash scripts/start-local-3validators.sh start         # Reuse local state
bash scripts/start-local-3validators.sh status        # Cluster health
bash scripts/start-local-3validators.sh stop          # Stop cluster
bash scripts/start-local-stack.sh testnet             # Local full stack (validators + custody + faucet)
bash scripts/stop-local-stack.sh testnet              # Stop local full stack
bash reset-blockchain.sh                              # Full reset of local chain state
```

### Identity & Keypair Management

Validator identity keypair location (in priority order):
1. `--keypair <path>` CLI argument
2. `LICHEN_VALIDATOR_KEYPAIR` env var
3. `{db-path}/validator-keypair.json` (data directory — recommended)
4. `~/.lichen/validators/validator-{port}.json` (legacy)
5. Auto-generated if none found

To migrate a validator to a new machine:
```bash
# Export keypair from old machine
scp old-machine:path/to/data/state-mainnet/validator-keypair.json ./my-validator.json

# Import on new machine
./target/release/lichen-validator \
    --import-key ./my-validator.json \
  --p2p-port 8001
```

### Auto-Update

```bash
./target/release/lichen-validator --auto-update=check           # Check only
./target/release/lichen-validator --auto-update=apply           # Download + restart
./target/release/lichen-validator --update-check-interval=300   # Custom interval (seconds)
./target/release/lichen-validator --update-channel=beta         # Channel selection
```

Exit code 75 → restart with new binary. Rollback: 3 crashes within 60s → automatic rollback.

### Docker

```bash
docker-compose up -d
```

### Systemd (VPS Deployment)

```bash
sudo bash deploy/setup.sh testnet mainnet   # Creates lichen user, env files, systemd units
sudo systemctl enable lichen-validator-testnet lichen-validator-mainnet
sudo systemctl start lichen-validator-testnet lichen-validator-mainnet
```

**CRITICAL — VPS State Directory Convention:**

All systemd services on production VPSes use `/var/lib/lichen/` as the base directory:

| Path | Purpose |
|------|---------|
| `/var/lib/lichen/state-testnet` | Testnet chain state (RocksDB) |
| `/var/lib/lichen/state-mainnet` | Mainnet chain state (RocksDB) |
| `/var/lib/lichen/contracts/` | Compiled WASM contract artifacts |
| `/var/lib/lichen/faucet-keypair-testnet.json` | Faucet signing keypair |
| `/var/lib/lichen/airdrops.json` | Faucet airdrop tracking |
| `/var/lib/lichen/custody-db` | Custody bridge state (testnet) |
| `/var/lib/lichen/custody-db-mainnet` | Custody bridge state (mainnet) |
| `/usr/local/bin/lichen-*` | Installed binaries |

**Never** run validators manually from `~/lichen/data/state-*` on VPSes — this creates a separate state directory from what systemd uses and causes split-brain / network stalls. The systemd unit sets `HOME=/var/lib/lichen` and `LICHEN_HOME=/var/lib/lichen`.

Or manually:
```bash
sudo cp deploy/lichen-validator.service /etc/systemd/system/
sudo systemctl enable lichen-validator
sudo systemctl start lichen-validator
```

### macOS LaunchAgent

Create wrapper script and LaunchAgent plist for an auto-starting background service. The wrapper downloads the latest release by semver (not GitHub "latest" publish date).

```bash
# 1. Create wrapper script
mkdir -p ~/.lichen/bin ~/.lichen/state-mainnet ~/.lichen/logs
cat > ~/.lichen/bin/lichen-service.sh << 'SCRIPT'
#!/bin/bash
set -euo pipefail
INSTALL_DIR="$HOME/.lichen/bin"
BINARY="$INSTALL_DIR/lichen-validator"
REPO="lobstercove/lichen"
ARCH=$(uname -m)
case "$ARCH" in
    arm64|aarch64) ASSET="lichen-validator-darwin-aarch64.tar.gz" ;;
    x86_64)        ASSET="lichen-validator-darwin-x86_64.tar.gz" ;;
esac
mkdir -p "$INSTALL_DIR" "$HOME/.lichen/state-mainnet" "$HOME/.lichen/logs"
if [ ! -x "$BINARY" ]; then
    TMPDIR=$(mktemp -d); trap 'rm -rf "$TMPDIR"' EXIT
    VERSION=$(/usr/bin/curl -fsSL "https://api.github.com/repos/$REPO/releases" \
        | /usr/bin/python3 -c "
import sys, json
releases = [r for r in json.load(sys.stdin) if not r['draft'] and not r['prerelease']]
releases.sort(key=lambda r: [int(x) for x in r['tag_name'].lstrip('v').split('.')], reverse=True)
print(releases[0]['tag_name'])")
    /usr/bin/curl -fSL -o "$TMPDIR/$ASSET" "https://github.com/$REPO/releases/download/${VERSION}/${ASSET}"
    /usr/bin/curl -fSL -o "$TMPDIR/SHA256SUMS" "https://github.com/$REPO/releases/download/${VERSION}/SHA256SUMS"
    cd "$TMPDIR"; grep "$ASSET" SHA256SUMS | shasum -a 256 -c -
    tar xzf "$ASSET" --strip-components=1
    mv lichen-validator "$BINARY"
    mv zk-prove "$INSTALL_DIR/zk-prove"
    mv lichen-genesis "$INSTALL_DIR/lichen-genesis"
    mv lichen "$INSTALL_DIR/lichen"
    cp -R contracts "$INSTALL_DIR/contracts"
    chmod +x "$BINARY" "$INSTALL_DIR/zk-prove" "$INSTALL_DIR/lichen-genesis" "$INSTALL_DIR/lichen"
    cp seeds.json "$HOME/.lichen/state-mainnet/seeds.json"
    trap - EXIT; rm -rf "$TMPDIR"
fi
exec "$BINARY" --network mainnet --p2p-port 8001 --rpc-port 9899 --ws-port 9900 \
    --db-path "$HOME/.lichen/state-mainnet" \
    --auto-update=apply
SCRIPT
chmod +x ~/.lichen/bin/lichen-service.sh

# 2. Create LaunchAgent plist
cat > ~/Library/LaunchAgents/network.lichen.validator.plist << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>network.lichen.validator</string>
    <key>ProgramArguments</key><array><string>/bin/bash</string><string>$HOME/.lichen/bin/lichen-service.sh</string></array>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>
    <key>ThrottleInterval</key><integer>15</integer>
    <key>StandardOutPath</key><string>$HOME/.lichen/logs/validator.log</string>
    <key>StandardErrorPath</key><string>$HOME/.lichen/logs/validator.log</string>
    <key>EnvironmentVariables</key><dict><key>RUST_LOG</key><string>info</string></dict>
</dict>
</plist>
EOF

# 3. Load
launchctl load ~/Library/LaunchAgents/network.lichen.validator.plist
```

Service management:
```bash
launchctl load ~/Library/LaunchAgents/network.lichen.validator.plist     # Start
launchctl unload ~/Library/LaunchAgents/network.lichen.validator.plist   # Stop
tail -f ~/.lichen/logs/validator.log                                     # Logs
launchctl list | grep lichen                                             # Status
# Force re-download after new release:
launchctl unload ~/Library/LaunchAgents/network.lichen.validator.plist
rm ~/.lichen/bin/lichen-validator
launchctl load ~/Library/LaunchAgents/network.lichen.validator.plist
```

macOS file layout:
| Path | Purpose |
|------|---------|
| `~/.lichen/bin/lichen-validator` | Binary |
| `~/.lichen/bin/lichen-service.sh` | Wrapper script |
| `~/.lichen/state-mainnet/` | Blockchain state DB |
| `~/.lichen/logs/validator.log` | Log file |
| `~/Library/LaunchAgents/network.lichen.validator.plist` | LaunchAgent plist |

### Windows Service (NSSM)

```powershell
# 1. Download and extract
$Version = (Invoke-RestMethod https://api.github.com/repos/lobstercove/lichen/releases/latest).tag_name
$InstallDir = "$env:LOCALAPPDATA\Lichen"
New-Item -ItemType Directory -Force -Path "$InstallDir\bin","$InstallDir\state-mainnet","$InstallDir\logs"
$Url = "https://github.com/lobstercove/lichen/releases/download/$Version/lichen-validator-windows-x86_64.tar.gz"
Invoke-WebRequest -Uri $Url -OutFile "$env:TEMP\lichen.tar.gz"
tar -xzf "$env:TEMP\lichen.tar.gz" -C "$InstallDir\bin" --strip-components=1

# 2. Install NSSM (https://nssm.cc/download) and create service
nssm install LichenValidator "$InstallDir\bin\lichen-validator.exe"
Copy-Item "$InstallDir\bin\seeds.json" "$InstallDir\state-mainnet\seeds.json" -Force
nssm set LichenValidator AppParameters "--network mainnet --p2p-port 8001 --rpc-port 9899 --ws-port 9900 --db-path $InstallDir\state-mainnet --auto-update=apply"
nssm set LichenValidator AppDirectory "$InstallDir"
nssm set LichenValidator AppStdout "$InstallDir\logs\validator.log"
nssm set LichenValidator AppStderr "$InstallDir\logs\validator.log"
nssm set LichenValidator AppEnvironmentExtra "RUST_LOG=info" "USERPROFILE=$InstallDir"
nssm set LichenValidator Start SERVICE_AUTO_START
nssm set LichenValidator AppRestartDelay 10000

# 3. Start
nssm start LichenValidator
```

Windows service management:
```powershell
nssm status LichenValidator
nssm start LichenValidator
nssm stop LichenValidator
nssm restart LichenValidator
nssm remove LichenValidator confirm     # Uninstall
Get-Content "$env:LOCALAPPDATA\Lichen\logs\validator.log" -Tail 50 -Wait
```

### Shielded Runtime

Shielded verification is self-contained in the native Plonky3 proof envelope. Validators do not load external `pk_*` or `vk_*` files, and `getShieldedPoolState` now reports the active scheme directly via `zkScheme` / `zk_scheme`.

### Health Check

```bash
curl -s http://localhost:9899 -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' | jq .
# → {"status":"ok","slot":12345}
```

---

## 18. Contract Development

See `docs/guides/CONTRACT_DEVELOPMENT.md` for the complete guide.

### Two SDKs (Different Packages)

| SDK | Package | Purpose | Environment |
|-----|---------|---------|-------------|
| Contract SDK | `lichen-contract-sdk` | Write on-chain WASM contracts | `#![no_std]`, `wasm32-unknown-unknown` |
| Client SDK (Rust) | `lichen-client-sdk` | Call RPC from Rust apps | `tokio`, `reqwest` |

### `lichen deploy` vs `lichen token create`

| Command | What it does | Fee |
|---------|-------------|-----|
| `lichen deploy contract.wasm` | Deploy WASM contract (no symbol registration) | 25.001 LICN |
| `lichen deploy contract.wasm --symbol X --name Y --template mt20` | Deploy + auto-register in symbol registry | 25.001 LICN |
| `lichen deploy contract.wasm --metadata '{"website":"...","logo_url":"..."}'` | Deploy with custom registry metadata JSON | 25.001 LICN |
| `lichen token create "Name" SYM --wasm token.wasm --initial-supply 1000000` | Deploy standard MT-20 token WASM + register + initialize + optional owner mint | 25.001 LICN |
| `lichen contract register <addr> --symbol X` | Retroactively register deployed contract in symbol registry | 0.001 LICN |

All tokens on Lichen are WASM contracts. Use `lichen deploy --symbol` when you need custom init logic or fully custom metadata. Use `lichen token create --wasm` for standard MT-20 named-export contracts; it initializes owner state and can mint the initial owner supply on-chain. There is no metadata-only `lichen deploy --supply` path. Use `lichen contract register` to fix contracts deployed without symbol metadata.

`--metadata <json>` attaches flat scalar registry metadata (string/number/boolean values, 1024 bytes max) to the symbol registry entry. After deploy, the CLI automatically verifies the contract is live on-chain by polling `getAccountInfo`.

### Template Categories

The `--template` flag sets the contract category in the symbol registry. The explorer uses this to filter and display contracts. Common values (matching genesis):

| Template | Explorer Category | Used By |
|----------|------------------|---------|
| `mt20` | Token | Fungible tokens (standard MT-20; `token` remains a legacy alias) |
| `wrapped` | Wrapped | Bridge-wrapped tokens (LUSD, WSOL, WETH, WBNB) |
| `nft` | NFT | NFT collections (`mt721` is also accepted) |
| `marketplace` | NFT | NFT marketplaces |
| `auction` | NFT | Auction contracts |
| `dex` | DEX | Exchange contracts |
| `defi` | DeFi | General DeFi protocols |
| `lending` | DeFi | Lending protocols |
| `bridge` | DeFi | Cross-chain bridges |
| `oracle` | DeFi | Price oracles |
| `governance` | Governance | DAO governance |
| `identity` | Infra | Identity/DID |
| `launchpad` | Infra | Token launchpads |
| `vault` | Infra | Vault contracts |
| `payments` | Infra | Payment processors |
| `bounty` | Infra | Bounty boards |
| `compute` | Infra | Compute markets |
| `storage` | Infra | Storage protocols |

The template is a free-form string — any value is accepted. Unrecognized values default to "Infra" in the explorer. The Programs Playground uses `mt20` (→ Token) and `mt721` (→ NFT) as aliases.

### Contract Function Convention

```rust
#[no_mangle]
pub extern "C" fn my_function(addr_ptr: *const u8, amount: u64) -> u32 {
    // addr_ptr = 32-byte address pointer
  // Returns a contract-specific status code (document your convention)
    1
}
```

### Deploy Fee Refund

If deployment fails (invalid WASM, duplicate address, etc.), the 25 LICN deploy premium is **automatically refunded** on-chain. Only the 0.001 LICN base fee is kept. This includes failures in `b_put_transaction` and `commit_batch` paths — the premium refund is guaranteed regardless of which stage fails. Failed transactions are stored on-chain and queryable via `getTransaction`.

### Deploy Output & Auto-Register Fallback

When you deploy with `--symbol`, the CLI:
1. Sends the deploy transaction (Phase 1)
2. Polls up to 10 times (1s apart) to verify the contract account is live on-chain (Phase 2)
3. Checks if the symbol was registered in the symbol registry (Phase 3)
4. If the symbol is **not found** after 3 retries, the CLI **automatically sends a register-symbol transaction** as a fallback
5. Reports final status with the contract address and symbol registration result

This means user-deployed contracts get the same name, category, and metadata in the explorer as genesis contracts — no manual `lichen contract register` step needed.

### Contract SDK Modules

| Module | Key Functions |
|--------|--------------|
| `storage` | `storage_get(key)`, `storage_set(key, val)`, `storage::remove(key)` |
| `contract` | `contract::args()`, `contract::set_return(data)` |
| `event` | `event::emit(json_str)` |
| `log` | `log::info(msg)` |
| `token` | `Token::new(name, symbol, decimals, prefix)` — MT-20 |
| `nft` | `NFT::new(name, symbol)` — MT-721 |
| `crosscall` | `CrossCall::new(target, fn, args)`, `call_contract(call)` |
| `dex` | `Pool::new(token_a, token_b)` — AMM |
| `crypto` | `poseidon_hash(left: &[u8;32], right: &[u8;32]) -> [u8;32]` — Poseidon2 runtime hash |
| `test_mock` | Thread-local mocks for native testing |

### Quick Start

```bash
# Add WASM target
rustup target add wasm32-unknown-unknown

# Build contract
cargo build --target wasm32-unknown-unknown --release

# Test locally
cargo test

# Deploy with symbol registration (need 25.001 LICN)
lichen deploy target/wasm32-unknown-unknown/release/my_contract.wasm \
  --symbol MYTK --name "My Token" --template mt20 --decimals 9 \
  --metadata '{"website":"https://example.com","description":"Community token"}'

# Standard MT-20 launch with owner mint
lichen token create "My Token" MYTK \
  --wasm target/wasm32-unknown-unknown/release/mt20_token.wasm \
  --decimals 9 --initial-supply 1000000 \
  --website https://example.com

# Or deploy without registration
lichen deploy target/wasm32-unknown-unknown/release/my_contract.wasm

# Retroactively register an already-deployed contract
lichen contract register <address> --symbol MYTK --name "My Token" --template mt20

# Call a function
lichen call <address> <function_name> [args]
```

---

## 19. Build & Test

### Build

```bash
cargo build --release                                    # Full workspace
cargo build --release -p lichen-validator              # Single crate
rustup target add wasm32-unknown-unknown                  # WASM target
bash scripts/build-all-contracts.sh                       # All 28 contracts
```

### Test Suites

The `tests/` harness referenced below is local-private operator tooling and is intentionally not present in the public clone. Public automation skips those commands when the private harness is absent.

| Suite | Command | Tests |
|-------|---------|-------|
| Core unit | `cargo test -p lobstercove-lichen-core` | Rust unit tests |
| RPC unit | `cargo test -p lichen-rpc` | RPC tests |
| Validator unit | `cargo test -p lichen-validator` | Includes auto-update |
| All Cargo | `cargo test --workspace` | ~1,296 tests |
| DEX unit | `node dex/dex.test.js` | 1,877 JS tests |
| E2E transactions | `node tests/e2e-transactions.js` | 26 tests |
| E2E production | `node tests/e2e-production.js` | 180 tests |
| E2E DEX | `node tests/e2e-dex.js` | 87 tests |
| E2E volume | `node tests/e2e-volume.js` | 115+ tests |
| E2E launchpad | `node tests/e2e-launchpad.js` | 48 tests |
| E2E prediction | `node tests/e2e-prediction.js` | 49 tests |
| Contracts write | `python tests/contracts-write-e2e.py` | 157 scenarios |

All E2E tests require a running validator (`bash scripts/start-local-3validators.sh start-reset` or `bash scripts/start-local-stack.sh testnet`).

---

*Last updated: June 2025*
