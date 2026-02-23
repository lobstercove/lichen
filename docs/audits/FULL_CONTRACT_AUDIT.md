# MoltyChain Smart Contract Suite — Complete Production Readiness Audit

**Scope:** All 27 contracts (47,246 lines of Rust/WASM)  
**Date:** 2025  
**Methodology:** Manual source review of every exported function, storage pattern, financial flow, error path, and security guard in every contract.

---

## Severity Definitions

| Severity | Meaning |
|----------|---------|
| **[CRITICAL]** | Exploitable vulnerability that can cause loss of funds, bypass core security, or break protocol invariants. Must fix before any deployment. |
| **[HIGH]** | Serious deficiency that will cause incorrect behavior, silent fund loss under realistic conditions, or missing core functionality. |
| **[MEDIUM]** | Design flaw, incomplete implementation, or degraded security that should be addressed before production. |
| **[LOW]** | Minor issue, code smell, dead code, or suboptimal pattern that does not directly threaten correctness but should be cleaned up. |

---

## Table of Contents

1. [moltcoin (493 lines)](#1-moltcoin)
2. [moltpunks (698 lines)](#2-moltpunks)
3. [weth_token (853 lines)](#3-weth_token)
4. [wsol_token (853 lines)](#4-wsol_token)
5. [moltmarket (988 lines)](#5-moltmarket)
6. [dex_rewards (1033 lines)](#6-dex_rewards)
7. [musd_token (1178 lines)](#7-musd_token)
8. [dex_router (1183 lines)](#8-dex_router)
9. [bountyboard (1254 lines)](#9-bountyboard)
10. [dex_analytics (1279 lines)](#10-dex_analytics)
11. [moltoracle (1372 lines)](#11-moltoracle)
12. [moltauction (1375 lines)](#12-moltauction)
13. [reef_storage (1434 lines)](#13-reef_storage)
14. [clawvault (1446 lines)](#14-clawvault)
15. [lobsterlend (1450 lines)](#15-lobsterlend)
16. [moltswap (1454 lines)](#16-moltswap)
17. [dex_governance (1503 lines)](#17-dex_governance)
18. [dex_amm (1508 lines)](#18-dex_amm)
19. [moltdao (1618 lines)](#19-moltdao)
20. [clawpump (1687 lines)](#20-clawpump)
21. [compute_market (2017 lines)](#21-compute_market)
22. [clawpay (2043 lines)](#22-clawpay)
23. [dex_margin (2055 lines)](#23-dex_margin)
24. [moltbridge (2247 lines)](#24-moltbridge)
25. [dex_core (3440 lines)](#25-dex_core)
26. [prediction_market (4581 lines)](#26-prediction_market)
27. [moltyid (6204 lines)](#27-moltyid)
28. [Cross-Cutting Findings](#28-cross-cutting-findings)
29. [Summary Statistics](#29-summary-statistics)

---

## 1. moltcoin

**File:** `contracts/moltcoin/src/lib.rs` (493 lines)  
**Purpose:** MT-20 fungible token (MOLT). Supply cap 10B.

### Architecture
- `#[no_mangle] pub extern "C"` direct exports: `initialize`, `mint`, `transfer`, `burn`, `approve`, `transfer_from`, `balance_of`, `total_supply`, `allowance`, `get_admin`.
- SDK `Token` type. Reentrancy guard (storage-based flag). Re-init guard. Caller verification via `get_caller()`.

### Findings

*No critical or high findings.* This is one of the cleanest contracts in the suite.

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 1.1 | **[LOW]** | ~45-50 | `balance_of` and `total_supply` do not use reentrancy guard. Acceptable for read-only functions, but inconsistent with the pattern used elsewhere. |
| 1.2 | **[LOW]** | ~120 | `mint` checks `new_supply > CAP` but the cap check is `>` not `>=`, meaning exactly 10B tokens can be minted. This is standard but worth confirming as intentional. |
| 1.3 | **[LOW]** | ~85-90 | `transfer` has `caller == from` check after loading caller — correct, but the pointer parameter `from_ptr` is redundant since it must always equal the transaction signer. |

---

## 2. moltpunks

**File:** `contracts/moltpunks/src/lib.rs` (698 lines)  
**Purpose:** MT-721 NFT collection. Max supply cap, pause mechanism.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 2.1 | **[MEDIUM]** | ~13-19 | `get_minter()` calls `storage_get(b"minter")` and panics via `unwrap()` if minter is not set. Any pre-initialization call to any function that invokes `get_minter()` will crash the WASM module. Should return a default or error code instead of panicking. |
| 2.2 | **[MEDIUM]** | entire file | **No reentrancy guard.** Unlike moltcoin and most other contracts, moltpunks has no reentrancy protection. If `call_nft_transfer` or any cross-contract call re-enters, state could be corrupted. |
| 2.3 | **[LOW]** | ~350-400 | Test-compatibility alias functions (`nft_owner`, `nft_metadata`) exist alongside the primary exports. Dead code in production. |
| 2.4 | **[LOW]** | ~200-220 | `mint` checks pause but `transfer_nft` also checks pause — correct layering, but `list_for_sale` (if present) does not check pause. |

---

## 3. weth_token

**File:** `contracts/weth_token/src/lib.rs` (853 lines)  
**Purpose:** Wrapped ETH receipt token. Reserve attestation, circuit breaker, epoch rate limiting.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 3.1 | **[MEDIUM]** | ~157-163, ~230-236 | **Double epoch reset.** `check_epoch_cap()` (called by `mint()`) resets the epoch counter when a new epoch is detected. Then `mint()` itself also contains epoch-check-and-reset logic in its body. If both trigger in the same call, the epoch counter is reset twice, potentially allowing double the intended epoch cap. |
| 3.2 | **[LOW]** | ~400-420 | `attest_reserves` has no reentrancy guard. While it only writes attestation data (no transfers), inconsistency with other state-changing functions. |
| 3.3 | **[LOW]** | entire file | 9-decimal precision. All arithmetic uses `u64`, giving a max representable value of ~18.4 ETH at 9-decimal precision. For a wrapped ETH token, this limits total supply. Consider `u128` if larger amounts are expected. Actually: 18,446,744,073 / 1e9 = ~18.4 billion tokens. This is sufficient. No issue. |

---

## 4. wsol_token

**File:** `contracts/wsol_token/src/lib.rs` (853 lines)  
**Purpose:** Wrapped SOL receipt token. Epoch cap 50K SOL.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 4.1 | **[MEDIUM]** | ~157-163, ~230-236 | **Same double epoch reset as weth_token.** Identical code, identical bug. |
| 4.2 | **[LOW]** | entire file | **DRY violation.** wsol_token is a near-identical copy of weth_token (different constants only). Refactoring into a shared wrapped-token template would eliminate duplicated bugs and reduce maintenance burden. |

---

## 5. moltmarket

**File:** `contracts/moltmarket/src/lib.rs` (988 lines)  
**Purpose:** NFT Marketplace. Listings, offers, royalties, 2.5% platform fee.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 5.1 | **[HIGH]** | ~580-600 | **`accept_offer` is non-atomic.** Pays seller first via `call_token_transfer`, then attempts NFT transfer to buyer. If NFT transfer fails, the seller has already received payment but the buyer gets no NFT. The function logs the failure but returns success (1), leaving the marketplace in an inconsistent state. |
| 5.2 | **[HIGH]** | ~480-510 | **`buy_nft` step-3 failure returns success.** If the NFT transfer fails after payment, the function logs an error message but returns `1` (which the contract treats as a non-zero "warning" rather than hard failure). Buyer loses tokens, seller keeps NFT. |
| 5.3 | **[MEDIUM]** | ~200-240 | `list_nft` and `list_nft_with_royalty` do not check the pause flag. Users can create new listings even when the marketplace is paused. Only `buy_nft` and `accept_offer` check pause. |
| 5.4 | **[LOW]** | ~145 | Listing layout is 145 bytes. Listing record stores `price` as u64 — max price ~18.4 billion micro-tokens. Sufficient for foreseeable use. |

---

## 6. dex_rewards

**File:** `contracts/dex_rewards/src/lib.rs` (1033 lines)  
**Purpose:** Trading incentives, tier multipliers, LP rewards, referral program.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 6.1 | **[HIGH]** | ~278-285 | **`claim_trading_rewards` / `claim_lp_rewards` silently clears balance without paying.** If the MOLT token address is not configured, the function zeroes out the user's pending rewards (marking them as claimed) but never actually transfers any tokens. The user's rewards are permanently lost with no error returned. |
| 6.2 | **[MEDIUM]** | ~290-300 | `record_trade` at Diamond tier gives a 3x multiplier on the trading fee as the reward. For large trades, this means the protocol pays out 3x the fee amount in MOLT rewards — potentially exceeding the fee revenue. Economic sustainability concern. |
| 6.3 | **[MEDIUM]** | ~350-360 | `lp_pending_key` is keyed by `position_id` only, with no owner verification. Any caller who knows a valid `position_id` can claim LP rewards for that position, regardless of whether they own it. |
| 6.4 | **[LOW]** | ~100-120 | WASM `call()` dispatcher pattern. All functions are `pub fn` routed through a single `call()` entry point. Consistent with other DEX contracts. |

---

## 7. musd_token

**File:** `contracts/musd_token/src/lib.rs` (1178 lines)  
**Purpose:** Treasury-backed stablecoin. Same architecture as weth/wsol.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 7.1 | **[MEDIUM]** | ~157-163, ~230-236 | **Same double epoch reset** as weth_token and wsol_token. Triplicated bug across all three wrapped/stable tokens. |
| 7.2 | **[LOW]** | entire file | **Triple DRY violation.** musd_token is the third near-identical copy of the weth/wsol pattern. All three share the same bugs and could be unified. |

---

## 8. dex_router

**File:** `contracts/dex_router/src/lib.rs` (1183 lines)  
**Purpose:** Smart order router. Route registry, multi-hop routing, deadline/slippage checks.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 8.1 | **[MEDIUM]** | ~400-430 | `execute_swap` validates slippage and deadline, but the actual cross-contract calls to pool contracts (`call_contract`) do not propagate these constraints. The router checks `amount_out >= min_amount_out` after the swap simulation, but if the underlying pool state changes between simulation and execution (MEV/frontrunning), the check may pass with stale data. |
| 8.2 | **[MEDIUM]** | ~200-210 | Route registry has a hard cap of 200 routes (96-byte layout each). No mechanism to remove or update routes once registered beyond admin override. Registry could fill up permanently. |
| 8.3 | **[LOW]** | ~180 | 5 route types defined but route type validation is a simple range check `<= 4`. Adding new types requires code changes. |
| 8.4 | **[LOW]** | ~500-520 | Swap records stored for analytics but never cleaned up. Unbounded storage growth over time. |

---

## 9. bountyboard

**File:** `contracts/bountyboard/src/lib.rs` (1254 lines)  
**Purpose:** Bounty/task management with cross-contract payment.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 9.1 | **[MEDIUM]** | ~450-470 | `approve_submission` performs cross-contract token transfer to pay the bounty hunter. If the transfer fails, the function returns an error but the submission status may have already been updated to "approved" in storage. Non-atomic state change. |
| 9.2 | **[MEDIUM]** | ~300-320 | MoltyID reputation gating reads from a local storage key `rep:{hex_pubkey}` — this only works if pred_market/bountyboard shares storage with MoltyID, which would not be the case for separate contract deployments. Cross-contract reputation checks need `call_contract`. |
| 9.3 | **[LOW]** | ~100-120 | Bounty creation requires a minimum amount but does not enforce a maximum. Very large bounties could lock excessive tokens in the contract. |

---

## 10. dex_analytics

**File:** `contracts/dex_analytics/src/lib.rs` (1279 lines)  
**Purpose:** On-chain OHLCV candles, 24h rolling stats, trader PnL, leaderboard.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 10.1 | **[MEDIUM]** | ~200-250 | `record_trade` is the core data ingestion function. It requires `is_authorized_caller()` check but the authorized caller is set by admin and is a single address. If the authorized caller contract is compromised or the address is misconfigured, all analytics data can be poisoned. No multi-source validation. |
| 10.2 | **[LOW]** | ~600-650 | Leaderboard (top 100) uses a linear scan of all tracked traders followed by insertion sort. O(n) per query where n = total traders tracked. Could become expensive with thousands of traders. |
| 10.3 | **[LOW]** | ~350-370 | 9 candle intervals defined (1m, 5m, 15m, 30m, 1h, 4h, 1d, 1w, 1M). 1-minute candles stored indefinitely — no pruning mechanism. Unbounded storage growth. |

---

## 11. moltoracle

**File:** `contracts/moltoracle/src/lib.rs` (1372 lines)  
**Purpose:** Decentralized oracle. Price feeds, VRF commit-reveal, attestations, SHA-256 implementation.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 11.1 | **[MEDIUM]** | ~484-500 | **`request_randomness` legacy single-step mode is predictable.** When `use_legacy` flag is set, randomness is derived from seed + slot + timestamp — all publicly known values. Any observer can predict the "random" value before it's committed. The commit-reveal path is secure, but the legacy path is not. |
| 11.2 | **[MEDIUM]** | ~144 | **Single feeder per asset.** Price feeds are keyed by `(asset, feeder)` with a single authorized feeder per asset. No multi-feeder redundancy, no median aggregation across feeders. If the single feeder goes offline or is compromised, the oracle has no fallback. |
| 11.3 | **[LOW]** | ~1137 | `get_aggregated_price` computes an arithmetic mean, not a median. Outlier-resistant aggregation would use median. With only one feeder per asset this is moot, but if multi-feeder support is added, mean is inferior. |
| 11.4 | **[LOW]** | ~900-920 | `get_oracle_stats` reads stats counters (`total_updates`, `total_requests`) that are never incremented anywhere in the contract. Always returns zero. Dead code. |
| 11.5 | **[LOW]** | ~800-810 | `submit_attestation` has no reentrancy guard and no pause check. Low risk since attestations don't involve transfers, but inconsistent with other state-changing functions. |
| 11.6 | **[LOW]** | ~750 | Attestation `signatures_count` stored as u8, capping at 255 signatures per attestation. Sufficient for foreseeable use but a hard limit. |

---

## 12. moltauction

**File:** `contracts/moltauction/src/lib.rs` (1375 lines)  
**Purpose:** NFT auction system. English auctions, anti-sniping, reserve prices, royalties.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 12.1 | **[HIGH]** | ~355 | **`finalize_auction` is non-atomic.** Pays seller via `call_token_transfer`, then transfers NFT to winner. If NFT transfer fails after payment, seller has funds but buyer gets no NFT. Auction is marked finalized regardless. |
| 12.2 | **[HIGH]** | ~490-520 | **`accept_offer` is non-atomic.** Same pattern: pays seller first, then transfers NFT. Payment succeeds → NFT transfer fails → inconsistent state. |
| 12.3 | **[MEDIUM]** | ~241 | `place_bid` only calls `reentrancy_exit()` on the success path. If the function returns early due to an error after `reentrancy_enter()`, the reentrancy flag remains set, permanently locking the contract until admin intervention. |
| 12.4 | **[MEDIUM]** | ~100-130 | `create_auction` and `place_bid` do not check the pause flag. Only `finalize_auction` checks pause. Users can create auctions and place bids during emergency pause. |
| 12.5 | **[LOW]** | ~300-310 | Anti-sniping extends auction by a fixed number of slots. Extension count is not capped — theoretically, repeated last-second bids could extend an auction indefinitely. In practice, attackers pay increasing bids, limiting abuse. |

---

## 13. reef_storage

**File:** `contracts/reef_storage/src/lib.rs` (1434 lines)  
**Purpose:** Decentralized storage. Data hosting, challenge-response verification, staking.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 13.1 | **[CRITICAL]** | ~820 | **`respond_challenge` has placeholder verification.** The challenge-response verification logic contains a `// TODO: implement actual proof verification` comment. The function accepts ANY response as valid, meaning storage providers can claim to store data without actually storing it. Core protocol invariant is broken. |
| 13.2 | **[HIGH]** | ~465 | **`claim_storage_rewards` zeroes balance without token transfer.** The function sets the provider's pending rewards to zero but never calls `call_token_transfer` to actually pay out. Providers lose all accumulated rewards when they "claim." |
| 13.3 | **[HIGH]** | ~380-400 | **`stake_collateral` records stake amount in storage but performs no actual token transfer.** The provider's stake balance increases on paper, but no tokens are moved from the provider's account to the contract. Staking is purely cosmetic. |
| 13.4 | **[MEDIUM]** | ~600-620 | `slash_provider` decrements the stake balance in storage but does not transfer the slashed tokens anywhere (treasury, burn, etc.). Slashing is accounting-only with no economic consequence. |

---

## 14. clawvault

**File:** `contracts/clawvault/src/lib.rs` (1446 lines)  
**Purpose:** ERC-4626 yield aggregator. Deposit/withdraw, share accounting, yield strategies.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 14.1 | **[HIGH]** | ~395-435 | **`harvest()` uses simulated yield.** The yield calculation is `balance * apy_bps / 10000 * elapsed / YEAR` computed purely from stored values — no actual DeFi strategy integration. The contract mints yield from nothing, inflating shares without real backing. This is acceptable for testnet but fatal for production. |
| 14.2 | **[MEDIUM]** | ~260-290 | **`deposit` has a race condition.** Share calculation reads `total_supply` and `total_assets` separately from storage. Between reading total_assets and completing the deposit, another transaction could change the values, leading to incorrect share pricing. No atomic snapshot mechanism. |
| 14.3 | **[LOW]** | ~580-590 | Risk tier is stored per vault configuration but never enforced. The `risk_tier` field (e.g., conservative, moderate, aggressive) is set during initialization but has no effect on allowed strategies or deposit limits. Dead field. |

---

## 15. lobsterlend

**File:** `contracts/lobsterlend/src/lib.rs` (1450 lines)  
**Purpose:** Lending protocol. Collateral factor 75%, liquidation threshold 85%, kinked interest rate, flash loans.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 15.1 | **[MEDIUM]** | ~720 | **Flash loan missing reentrancy guard on `flash_borrow`.** The flash loan function does not call `reentrancy_enter()` / `reentrancy_exit()`. While the flash loan protocol inherently involves a callback (borrow → use → repay in one tx), the lack of reentrancy protection means a malicious callback could re-enter other contract functions during the flash loan. |
| 15.2 | **[MEDIUM]** | ~350-360 | `liquidate` intentionally does not check the pause flag (comment: "liquidations must always be possible for protocol safety"). This is a valid design choice. However, `liquidate` also lacks a separate "liquidation pause" for extreme scenarios where the oracle is compromised. |
| 15.3 | **[LOW]** | ~200-220 | Interest rate model uses a kinked curve with hardcoded parameters. No admin function to update the kink point, base rate, or slope after deployment. Rate model changes require redeployment. |
| 15.4 | **[LOW]** | ~100 | Flash loan fee is 0.09% (9 bps). Hardcoded, not configurable. |

---

## 16. moltswap

**File:** `contracts/moltswap/src/lib.rs` (1454 lines)  
**Purpose:** AMM DEX. Constant-product pools, reputation bonuses.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 16.1 | **[HIGH]** | ~720 | **`set_protocol_fee` uses wrong admin key.** The function checks `IDENTITY_ADMIN_KEY` (the identity/user admin) instead of `MS_ADMIN_KEY` (the moltswap admin). This means the moltswap admin cannot change fees, and the identity admin (a different role) can. Authorization boundary violation. |
| 16.2 | **[HIGH]** | ~290-300 | **Reputation bonus drains LP reserves.** Users with high MoltyID reputation receive a bonus on swaps (e.g., better exchange rate). This bonus comes from the pool's reserves, not from a separate incentive fund. Over time, reputation bonuses drain LP value, causing losses for liquidity providers who did not consent to subsidizing high-reputation traders. |
| 16.3 | **[MEDIUM]** | ~250-260 | `swap_a_for_b` loads pool data twice from storage — once to validate, once to execute. Between the two reads, pool state could change, causing the swap to execute against stale data. Should load once and reuse. |
| 16.4 | **[LOW]** | ~400-420 | `add_liquidity` computes LP tokens using the standard `min(dx/X, dy/Y) * L` formula. No minimum liquidity check for the first deposit (could be front-run to manipulate initial price). |

---

## 17. dex_governance

**File:** `contracts/dex_governance/src/lib.rs` (1503 lines)  
**Purpose:** DEX governance. Proposals, voting, parameter changes.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 17.1 | **[MEDIUM]** | ~80 | **`MIN_QUORUM` is 3 voters.** A governance proposal can pass with only 3 votes. For a public DEX, this is trivially gameable — a single entity with 3 addresses controls governance. |
| 17.2 | **[MEDIUM]** | ~200-300 | Most functions are `pub fn` but not `#[no_mangle] pub extern "C"`. They're only accessible through the WASM `call()` dispatcher. However, several governance helper functions are public without access control — they rely on the dispatcher routing to enforce caller checks. Any direct WASM call could bypass routing. |
| 17.3 | **[LOW]** | ~500-520 | Proposal execution uses a simple dispatch table for supported parameter changes. No generic "execute arbitrary call" capability, limiting governance to pre-defined actions. This is actually a security benefit (constrains governance power). |

---

## 18. dex_amm

**File:** `contracts/dex_amm/src/lib.rs` (1508 lines)  
**Purpose:** Concentrated liquidity AMM. Tick-based pricing, range orders.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 18.1 | **[HIGH]** | ~350 | **`tick_to_sqrt_price` uses a linear approximation** instead of the correct exponential formula (`1.0001^(tick/2)`). The linear approximation diverges significantly at extreme tick values (far from tick 0), causing mispricing for assets with large price differentials. This breaks concentrated liquidity math for volatile pairs. |
| 18.2 | **[HIGH]** | ~585-610 | **`accrue_fees_to_positions` is O(n) per swap.** Every swap iterates over ALL active positions in the affected tick range to distribute fees pro-rata. With hundreds of positions, this creates an O(n) gas cost per swap, making the pool economically unusable as position count grows. Uniswap V3 solves this with `feeGrowthGlobal` accumulators. |
| 18.3 | **[MEDIUM]** | ~700-720 | `swap_exact_out` uses binary search with 64 iterations to find the exact input amount. While this converges, 64 iterations of the swap simulation function inside a binary search is computationally expensive and may exceed gas limits for complex paths. |
| 18.4 | **[LOW]** | ~450-470 | Position liquidity stored as u64 — max ~18.4 billion units. For tokens with high decimal precision, this limits maximum position size. |

---

## 19. moltdao

**File:** `contracts/moltdao/src/lib.rs` (1618 lines)  
**Purpose:** DAO governance. Proposals, voting, timelocked execution, treasury.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 19.1 | **[HIGH]** | ~469 | **`execute_proposal` marks proposal as executed BEFORE dispatching the action, and continues on dispatch failure.** If the governance action (e.g., treasury transfer) fails, the proposal is still marked "executed" and cannot be retried. Governance decisions can silently fail with no recourse. |
| 19.2 | **[MEDIUM]** | ~296-303 | **`set_quorum` / `set_voting_period` / `set_timelock_delay` store new values in configuration storage, but `create_proposal_typed` reads hardcoded defaults instead.** Admin config changes have no effect. Governance parameters are immutable despite the admin functions suggesting otherwise. |
| 19.3 | **[MEDIUM]** | ~357 | **`vote_with_reputation` trusts caller-provided reputation parameter.** The function accepts a `reputation` parameter from the caller without verifying it against MoltyID. Comment says `// TODO: cross-contract reputation query`. Any caller can claim arbitrary reputation to amplify their vote weight. |
| 19.4 | **[LOW]** | ~600-610 | `get_treasury_balance` reads key `dao_treasury_balance` which is never written to by any function. Always returns 0. Dead code. |

---

## 20. clawpump

**File:** `contracts/clawpump/src/lib.rs` (1687 lines)  
**Purpose:** Token launchpad. Bonding curve token sales, graduation to DEX.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 20.1 | **[MEDIUM]** | ~382-395 | **Graduation revenue tracked before success check.** Revenue counters are incremented before verifying that the graduation cross-contract call succeeded. If graduation fails, revenue statistics are inflated with phantom graduation fees. |
| 20.2 | **[MEDIUM]** | ~450-465 | **`withdraw_fees` only decrements the fee counter without performing a token transfer.** Admin "withdraws" fees by zeroing the counter, but no tokens are actually moved. Fees remain in the contract with no way to extract them. |
| 20.3 | **[LOW]** | ~100-110 | Creation fee log message says "0.1 MOLT" but `CREATION_FEE` constant is `10_000_000_000` (10 MOLT at 9-decimal precision). Misleading log. |
| 20.4 | **[LOW]** | ~300-320 | Bonding curve uses a simple linear curve: `price = base_price + (supply * slope)`. No curve type selection or configurability. |

---

## 21. compute_market

**File:** `contracts/compute_market/src/lib.rs` (2017 lines)  
**Purpose:** Decentralized compute marketplace. Job posting, claiming, escrow, dispute resolution.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 21.1 | **[HIGH]** | ~747 | **`release_payment` clears escrow without actual token transfer.** The function zeroes the job's escrow balance in storage but never calls `call_token_transfer` to pay the compute provider. Provider completes work, client releases payment, but tokens remain locked in the contract forever. |
| 21.2 | **[HIGH]** | ~800-820 | **`cancel_job` clears escrow without refund.** Same pattern: escrow balance set to zero in storage, but no token transfer to refund the job creator. Tokens trapped in contract. |
| 21.3 | **[HIGH]** | ~850-870 | **`resolve_dispute` conditionally transfers.** Dispute resolution only calls `call_token_transfer` if `cm_token_address` is configured. If not configured, the resolution clears the escrow balance with no transfer. Unlike the unconditional early-returns in 21.1 and 21.2, this at least checks configuration — but the default (unconfigured) path silently loses funds. |
| 21.4 | **[MEDIUM]** | ~700-710 | **Cancel timeout uses `created_slot` not `claimed_slot`.** The cancellation timeout is calculated from when the job was created, not when it was claimed by a provider. A job created long ago but claimed recently could be immediately cancellable, denying the provider their work window. |

---

## 22. clawpay

**File:** `contracts/clawpay/src/lib.rs` (2043 lines)  
**Purpose:** Streaming payments (Sablier-style). Linear vesting, cancellation, withdrawal.

### Findings

This is one of the best-implemented contracts in the suite. Proper checks-effects-interactions pattern with rollback on transfer failure.

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 22.1 | **[MEDIUM]** | ~800-820 | **`cancel_stream` legacy path marks stream as cancelled without performing refund/payout transfers** when token address or self address is not configured. Stream state changes to CANCELLED and admin can no longer re-attempt the cancellation, but neither party receives their tokens. |
| 22.2 | **[LOW]** | ~400-420 | `withdraw_from_stream` properly implements rollback: if `call_token_transfer` fails, the `withdrawn` counter is reverted. This is the gold standard pattern that other contracts should follow. |
| 22.3 | **[LOW]** | ~100-110 | Stream record is a large fixed-size layout. No compaction or cleanup of completed/cancelled streams. Over time, storage grows unboundedly. |

---

## 23. dex_margin

**File:** `contracts/dex_margin/src/lib.rs` (2055 lines)  
**Purpose:** Margin trading. Leveraged positions, liquidation engine, insurance fund.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 23.1 | **[HIGH]** | ~565 | **`withdraw_insurance` transfers from admin's account, not the contract.** The function calls `call_token_transfer(token, admin_address, recipient, amount)` — this sends tokens FROM the admin's personal balance, not from the contract's insurance fund. The insurance fund balance in storage is decremented, but the tokens come from the wrong source. If admin's personal balance is insufficient, the withdrawal silently fails. |
| 23.2 | **[MEDIUM]** | ~430 | **`close_position` returns full margin when oracle is down.** If the mark price is unavailable (oracle failure), the function returns the trader's full margin regardless of PnL. During an oracle outage, traders with underwater positions can exit at par, socializing losses to the insurance fund. |
| 23.3 | **[MEDIUM]** | ~200-210 | **`set_mark_price` is admin-only.** The mark price for all margin positions is set by a single admin address. This creates a centralization risk — the admin can manipulate mark prices to trigger (or prevent) liquidations. Should integrate with moltoracle for decentralized price feeds. |
| 23.4 | **[LOW]** | ~300-310 | Maximum leverage is 20x (hardcoded). No per-asset leverage limits — a highly volatile token gets the same 20x max as a stablecoin pair. |

---

## 24. moltbridge

**File:** `contracts/moltbridge/src/lib.rs` (2247 lines)  
**Purpose:** Cross-chain bridge. Lock/mint/unlock pattern, multi-validator confirmations.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 24.1 | **[HIGH]** | ~300-500 | **Bridge operations never transfer tokens.** `lock_tokens`, `submit_mint`, `confirm_mint`, `submit_unlock`, `confirm_unlock` — NONE of these call `call_token_transfer`. They only increment/decrement accounting counters in storage. Users "lock" tokens by updating a counter but the tokens remain in their wallet. The bridge "mints" by updating a counter but no tokens are created. The entire bridge is an accounting ledger with no actual asset movement. |
| 24.2 | **[MEDIUM]** | ~300-310 | **Only `lock_tokens` has reentrancy guard.** `submit_mint`, `confirm_mint`, `submit_unlock`, `confirm_unlock` all lack `reentrancy_enter()` / `reentrancy_exit()`. If any of these are called via cross-contract callback, re-entrancy could corrupt bridge state (double mints, etc.). |
| 24.3 | **[LOW]** | ~150-160 | No check that `required_confirmations <= validator_count`. If `required_confirmations` is set higher than the number of registered validators, no bridge operation can ever be confirmed, permanently locking the bridge. |
| 24.4 | **[LOW]** | ~120-130 | Bridge fee is hardcoded. No admin function to adjust bridge fees after deployment. |

---

## 25. dex_core

**File:** `contracts/dex_core/src/lib.rs` (3440 lines)  
**Purpose:** Central Limit Order Book (CLOB) + matching engine. 128-byte Order, 112-byte TradingPair, 80-byte Trade. 30 WASM opcodes.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 25.1 | **[HIGH]** | ~1875 | **`check_triggers` is O(n) over ALL orders.** The trigger-check function iterates every stored order to find stop-loss/take-profit orders that should execute. With thousands of orders, this becomes a DoS vector — the gas cost of checking triggers scales linearly with total order count, eventually making the function uncallable. |
| 25.2 | **[HIGH]** | ~1200-1210 | **`user_order_count` never decrements.** The per-user order counter increments on every `place_order` but never decrements on cancel, fill, or expiry. After 100 orders (the hardcoded max), a user is permanently unable to place new orders. The counter must be decremented when orders are removed. |
| 25.3 | **[MEDIUM]** | ~1719-1731 | **`modify_order` is non-atomic.** The function cancels the old order (calling `reentrancy_exit()`), then places a new order. Between cancel and place, another transaction could interact with the order book in the gap where the reentrancy guard is down. |
| 25.4 | **[MEDIUM]** | ~1564 | **Fee collection is best-effort.** Trade execution proceeds even if the fee transfer via `call_token_transfer` fails. The trade record is written and balances updated, but the protocol collects no fee. Over time, failed fee collection erodes protocol revenue with no alerting. |
| 25.5 | **[MEDIUM]** | ~1400 | **Balance validation is fail-open.** The `validate_balance` check queries the user's token balance, but if the cross-contract call fails (network issue, malformed response), the function defaults to allowing the trade rather than rejecting it. Users could trade without sufficient balance during cross-contract failures. |
| 25.6 | **[MEDIUM]** | ~1720-1730 | **`modify_order` drops `trigger_price` for stop-limit orders.** When modifying a stop-limit order, the trigger_price is not carried over to the replacement order. The modified order becomes a regular limit order, losing its stop-loss/take-profit behavior. |
| 25.7 | **[LOW]** | ~1800-1820 | **Cancelled orders accumulate in book price levels.** When an order is cancelled, it's marked as cancelled in storage but not removed from the price level's order list. The matching engine must skip cancelled orders during iteration, increasing gas costs over time. |
| 25.8 | **[LOW]** | ~900-910 | **Daily volume day boundaries are fixed from slot 0.** The "trading day" for volume calculations starts at slot 0 and repeats every `SLOTS_PER_DAY`. This means a "day" starts at an arbitrary time (genesis), not at midnight UTC. Minor UX issue. |

---

## 26. prediction_market

**File:** `contracts/prediction_market/src/lib.rs` (4581 lines)  
**Purpose:** Binary and multi-outcome CPMM prediction markets. Full market lifecycle, LP system, dispute resolution, circuit breakers. 38 WASM opcodes.

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 26.1 | **[HIGH]** | ~183-193 | **`transfer_musd_out()` silently succeeds when addresses not configured.** The helper function returns `true` (success) when mUSD token address or self address is not set. This means `redeem_shares` and `reclaim_collateral` will clear the user's position state (zeroing their shares/LP) WITHOUT actually transferring any mUSD to them. Users believe they were paid, positions are deleted, but no tokens move. |
| 26.2 | **[HIGH]** | ~2467-2472 | **`challenge_resolution` never escrows the dispute bond.** The function records the challenger's bond amount in storage and checks `bond >= DISPUTE_BOND` (100 mUSD), but never calls any transfer function to actually collect the bond. Disputes are free — anyone can challenge a resolution at no cost, defeating the economic disincentive for frivolous challenges. |
| 26.3 | **[HIGH]** | ~2336 | **`submit_resolution` never escrows the resolver's bond.** Same as 26.2 — the resolution bond amount is validated and recorded in storage, but no tokens are transferred from the resolver. Resolution has no skin-in-the-game, allowing spam. |
| 26.4 | **[MEDIUM]** | ~2122 | **`sell_shares` returns `musd_returned as u32` — truncated return value.** For amounts exceeding ~4,294 mUSD (u32::MAX micro-units), the return value wraps. Actual state changes use the full u64 value, so internal accounting is correct, but callers reading the return value see a truncated amount. |
| 26.5 | **[MEDIUM]** | ~2014 | **`buy_shares` returns `shares_received as u32` — same u32 truncation.** |
| 26.6 | **[MEDIUM]** | ~2642 | **`withdraw_liquidity` returns `musd_returned as u32` — same u32 truncation.** LP withdrawals > $4,294 truncate the return value. |
| 26.7 | **[MEDIUM]** | ~1774 | **`add_liquidity` returns `new_lp as u32` — same u32 truncation.** |
| 26.8 | **[MEDIUM]** | ~2588-2595 | **`reclaim_collateral` for voided markets: traders who are also LPs get only the larger of `cost_basis_refund` and `lp_share_refund`, not the sum.** Code: `if lp_share > refund { refund = lp_share }`. Users who are both traders and LPs lose the smaller of their two entitlements. |
| 26.9 | **[MEDIUM]** | ~1067-1073 | **MoltyID reputation check reads local storage, not MoltyID contract storage.** `create_market` reads `rep:{hex_pubkey}` from its own contract storage. In a multi-contract deployment, each contract has its own storage namespace, so this key would never contain data written by MoltyID. Reputation gating is non-functional. |
| 26.10 | **[LOW]** | ~2692 | `redeem_shares` transfers mUSD to user BEFORE clearing position state. Comment acknowledges this: "checks-effects-interactions is reversed here, but reentrancy guard protects us." The reentrancy guard does mitigate, but best practice is effects-before-interactions. |
| 26.11 | **[LOW]** | ~3600-3620 | `get_leaderboard` scans up to 500 traders and performs selection sort — O(500 * 50) = O(25,000) per query. Expensive but query-only (no state changes). |
| 26.12 | **[LOW]** | ~1800-1810 | `calculate_sell` for multi-outcome markets uses a simplified equal-partition approach. Not optimal for large sells, but not incorrect — users receive slightly less than a perfect solver would compute. |

---

## 27. moltyid

**File:** `contracts/moltyid/src/lib.rs` (6204 lines)  
**Purpose:** Agent identity and reputation system. Registration, skills, vouching, .molt naming, social recovery, delegation, achievements, attestations, agent discovery.

### Architecture
- Mixed export styles: core functions use `#[no_mangle] pub extern "C"` direct exports. Some functions (naming, delegation, discovery) also use direct exports. No WASM `call()` dispatcher.
- 127-byte identity record layout. Skills, vouches, attestations stored separately.
- FNV-1a 128-bit skill name hashing with legacy backward compatibility.
- Social recovery: 3-of-5 guardian scheme.
- .molt naming system: 3-32 char domains, premium (3-4 char) auction-only.
- Delegation: 4 permission types, time-bounded.
- Comprehensive test suite (~50 tests).

### Findings

| # | Severity | Lines | Finding |
|---|----------|-------|---------|
| 27.1 | **[MEDIUM]** | ~1920-2000 (execute_recovery) | **Social recovery does not migrate skills, vouches, or achievements.** `execute_recovery` copies the identity record (which includes `skill_count` and `vouch_count`) to the new owner key, but skills are stored at `skill:{hex(old_owner)}:{index}` and vouches at `vouch:{hex(old_owner)}:{index}`. After recovery, the new identity record reports N skills/vouches but `get_skills(new_owner)` and `get_vouches(new_owner)` look under the new owner's hex prefix — finding nothing. The user's skill and vouch history is effectively lost. |
| 27.2 | **[MEDIUM]** | ~4258-4270 (admin_register_reserved_name) | **`admin_register_reserved_name` does not verify caller via `get_caller()`.** It reads the admin address from the `args` buffer provided by the caller and checks `is_mid_admin(admin)`. If the args buffer is caller-controlled (which it is in typical WASM environments), anyone could craft args starting with the admin's address to pass the check. All other admin functions in this contract verify `get_caller().0 == caller`. |
| 27.3 | **[MEDIUM]** | ~2780-2860 (bid_name_auction) | **`bid_name_auction` has no reentrancy guard.** This function calls `call_token_transfer` to refund the previous highest bidder. If the refund triggers a callback that re-enters `bid_name_auction`, the auction record could be corrupted (e.g., double-refund or state inconsistency). Other financial functions in this contract use reentrancy guards. |
| 27.4 | **[MEDIUM]** | ~2870-2940 (finalize_name_auction) | **`finalize_name_auction` does not verify caller identity.** The function reads `caller_ptr` into `_caller` (leading underscore = unused variable) and never checks it against `get_caller()` or admin. Any address can finalize any ended auction. While this is possibly by design (permissionless finalization after auction end), it differs from the access model of other admin-gated functions. |
| 27.5 | **[MEDIUM]** | ~2870-2940 (finalize_name_auction) | **`finalize_name_auction` has no reentrancy guard.** It modifies name records, identity records, and auction state. Without reentrancy protection, a reentrant call could create duplicate name registrations. |
| 27.6 | **[LOW]** | ~2360-2415 (revoke_attestation) | `revoke_attestation` has no reentrancy guard. Low risk since it only modifies attestation storage (no transfers), but inconsistent with `attest_skill` which does use a reentrancy guard. |
| 27.7 | **[LOW]** | ~2460-2555 (register_name) | `register_name` has no reentrancy guard. Calls `get_value()` which reads payment, but does not call `call_token_transfer`. Lower risk but inconsistent with identity registration which uses the guard. |
| 27.8 | **[LOW]** | ~374-380 (has_vouched_for) | `has_vouched_for` iterates all vouches for a vouchee (up to MAX_VOUCHES=64) doing storage reads for each. Called by `set_recovery_guardians` for each of 5 guardians — worst case 320 storage reads. Expensive but bounded. |
| 27.9 | **[LOW]** | ~2040-2050 | `set_endpoint`, `set_metadata`, `set_availability`, `set_rate` — none use reentrancy guards. Low risk as simple setter functions with no transfers. |
| 27.10 | **[LOW]** | ~1800 | Achievement IDs `ACHIEVEMENT_ENDORSED` (10+ vouches auto-check) is defined but not automatically awarded in `check_achievements`. Only awarded via `award_contribution_achievement` admin call. The "endorsed" check is manual despite being easily automatable. |

---

## 28. Cross-Cutting Findings

### 28.1 [HIGH] — Escrow Without Actual Transfers (Pattern)

**Affected contracts:** reef_storage, dex_rewards, compute_market, clawpump, moltbridge, prediction_market (bonds)

Multiple contracts implement "escrow" or "payment" by modifying storage counters without calling `call_token_transfer`. Users' balances are decremented on paper, but no tokens move. This is the single most pervasive critical pattern across the codebase.

**Contracts with this pattern:**
- **reef_storage**: `claim_storage_rewards` (line ~465), `stake_collateral` (line ~380)
- **dex_rewards**: `claim_trading_rewards` / `claim_lp_rewards` (line ~280)
- **compute_market**: `release_payment` (line ~747), `cancel_job` (line ~800)
- **clawpump**: `withdraw_fees` (line ~450)
- **moltbridge**: ALL bridge operations — `lock_tokens`, `submit_mint`, `confirm_mint`, `submit_unlock`, `confirm_unlock` (lines ~300-500)
- **prediction_market**: `challenge_resolution` bond (line ~2467), `submit_resolution` bond (line ~2336)

### 28.2 [HIGH] — Non-Atomic Multi-Step Operations (Pattern)

**Affected contracts:** moltmarket, moltauction, moltdao

Multiple contracts perform payment first, then asset transfer. If the second step fails, the first step has already committed, leaving the protocol in an inconsistent state.

**Contracts with this pattern:**
- **moltmarket**: `accept_offer` (line ~580), `buy_nft` (line ~480)
- **moltauction**: `finalize_auction` (line ~355), `accept_offer` (line ~490)
- **moltdao**: `execute_proposal` (line ~469, marks executed before dispatch)

### 28.3 [MEDIUM] — "Graceful Degradation" That Silently Fails (Pattern)

**Affected contracts:** prediction_market, clawpay, dex_rewards

Several contracts have a "graceful degradation" pattern where missing configuration (token address, self address) causes financial operations to silently succeed without moving tokens. This is worse than failing — users believe the operation completed.

- **prediction_market**: `transfer_musd_out()` returns true when unconfigured (line ~183)
- **clawpay**: `cancel_stream` legacy path (line ~800)
- **dex_rewards**: `claim_*` functions zero balance when MOLT not configured (line ~280)

### 28.4 [MEDIUM] — MoltyID Reputation Cross-Contract Reads (Pattern)

**Affected contracts:** prediction_market, bountyboard, moltdao

Multiple contracts attempt to read MoltyID reputation from local storage (`rep:{hex_pubkey}`) instead of performing a cross-contract call to the MoltyID contract. In a multi-contract deployment where each contract has its own storage namespace, these reads always return None, making reputation gating non-functional.

- **prediction_market**: `create_market` (line ~1067)
- **bountyboard**: submission approval (line ~300)
- **moltdao**: `vote_with_reputation` accepts caller-provided value (line ~357)

### 28.5 [LOW] — Triplicated Wrapped Token Code (DRY)

**Affected contracts:** weth_token, wsol_token, musd_token

Three contracts share ~95% identical code with only constant changes. All three have the same double-epoch-reset bug. A shared wrapped-token library would eliminate triplicated maintenance.

### 28.6 [LOW] — Unbounded Storage Growth (Pattern)

**Affected contracts:** dex_analytics (candle data), dex_core (cancelled orders), dex_router (swap records), clawpay (completed streams), prediction_market (resolved markets)

No contract implements storage cleanup or pruning for historical data. Over long enough operation, storage costs grow monotonically.

---

## 29. Summary Statistics

### Finding Counts by Severity

| Severity | Count |
|----------|-------|
| **CRITICAL** | 1 |
| **HIGH** | 19 |
| **MEDIUM** | 33 |
| **LOW** | 32 |
| **TOTAL** | 85 |

### CRITICAL Findings (Must Fix)

| # | Contract | Finding |
|---|----------|---------|
| 13.1 | reef_storage | `respond_challenge` placeholder verification — any response accepted |

### HIGH Findings (Should Fix Before Production)

| # | Contract | Finding |
|---|----------|---------|
| 5.1 | moltmarket | `accept_offer` non-atomic: pays seller, NFT transfer may fail |
| 5.2 | moltmarket | `buy_nft` returns success on NFT transfer failure |
| 6.1 | dex_rewards | `claim_*` silently zeroes balance without token transfer |
| 12.1 | moltauction | `finalize_auction` non-atomic: payment before NFT transfer |
| 12.2 | moltauction | `accept_offer` non-atomic |
| 13.2 | reef_storage | `claim_storage_rewards` no token transfer |
| 13.3 | reef_storage | `stake_collateral` no token transfer |
| 14.1 | clawvault | `harvest()` simulated yield, not real DeFi |
| 16.1 | moltswap | `set_protocol_fee` wrong admin key |
| 16.2 | moltswap | Reputation bonus drains LP reserves |
| 18.1 | dex_amm | `tick_to_sqrt_price` linear approximation diverges |
| 18.2 | dex_amm | `accrue_fees_to_positions` O(n) per swap |
| 19.1 | moltdao | `execute_proposal` marks executed before dispatch, no retry |
| 21.1 | compute_market | `release_payment` escrow cleared, no transfer |
| 21.2 | compute_market | `cancel_job` escrow cleared, no refund |
| 23.1 | dex_margin | `withdraw_insurance` transfers from admin, not contract |
| 24.1 | moltbridge | Bridge operations never transfer tokens |
| 25.1 | dex_core | `check_triggers` O(n) DoS risk |
| 25.2 | dex_core | `user_order_count` never decrements — permanent lockout |
| 26.1 | prediction_market | `transfer_musd_out` silently succeeds when unconfigured |
| 26.2 | prediction_market | `challenge_resolution` bond never escrowed |
| 26.3 | prediction_market | `submit_resolution` bond never escrowed |

### Contracts with No Critical/High Findings

- **moltcoin** — Clean token implementation
- **clawpay** — Best-in-suite checks-effects-interactions with rollback
- **dex_router** — Functional routing with minor issues
- **dex_analytics** — Query-focused, low risk
- **dex_governance** — Constrained governance (low quorum concern is MEDIUM)
- **moltyid** — Comprehensive identity system (data migration concern is MEDIUM)

### Top Priority Remediation

1. **Fix escrow-without-transfer pattern** (13 affected operations across 6 contracts) — Add actual `call_token_transfer` calls where tokens are supposed to move.
2. **Fix non-atomic operations** (5 affected operations across 3 contracts) — Implement checks-effects-interactions or rollback (see clawpay for reference implementation).
3. **Implement reef_storage proof verification** — The single CRITICAL finding.
4. **Add `call_token_transfer` to moltbridge** — The bridge is non-functional without actual transfers.
5. **Fix dex_core order count decrement** — Users are permanently locked out after 100 orders.
6. **Fix dex_amm tick pricing** — Linear approximation breaks concentrated liquidity math.
