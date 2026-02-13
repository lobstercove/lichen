# MoltChain Full Codebase Audit & Fix Plan

**Date:** February 14, 2026
**Scope:** core/, contracts/ (26), validator/, rpc/, custody/
**Total Findings:** 106 issues (18 CRITICAL, 24 HIGH, 37 MEDIUM, 27 LOW)

---

## Table of Contents

1. [Phase 0 — CRITICAL Fixes (Ship-Blockers)](#phase-0--critical-fixes-ship-blockers)
2. [Phase 1 — HIGH Fixes (Pre-Launch Required)](#phase-1--high-fixes-pre-launch-required)
3. [Phase 2 — MEDIUM Fixes (Hardening)](#phase-2--medium-fixes-hardening)
4. [Phase 3 — LOW Fixes (Polish)](#phase-3--low-fixes-polish)
5. [Full Finding Index](#full-finding-index)

---

## Phase 0 — CRITICAL Fixes (Ship-Blockers)

These must be fixed before any public deployment. Each represents an exploitable vulnerability or a consensus-breaking bug.

### 0.1 — Contract Re-initialization (5 contracts)
**Files:** `contracts/{moltcoin,moltoracle,moltswap,moltmarket,moltdao}/src/lib.rs`
**Impact:** Ownership takeover — anyone can call `initialize()` again and become admin/owner.

| # | Contract | Function | Fix |
|---|----------|----------|-----|
| 0.1a | moltcoin | `initialize()` | Add `if storage_get(b"owner").is_some() { return 1; }` before writing owner |
| 0.1b | moltoracle | `initialize_oracle()` | Add `if storage_get(b"oracle_owner").is_some() { return 0; }` before writing |
| 0.1c | moltswap | `initialize()` | Add `if storage_get(b"pool_initialized").is_some() { return; }` guard |
| 0.1d | moltmarket | `initialize()` | Add `if storage_get(b"marketplace_owner").is_some() { return; }` guard |
| 0.1e | moltdao | `initialize_dao()` | Add `if storage_get(b"governance_token").is_some() { return; }` guard |

**Pattern:** Copy the guard already used correctly in `musd_token`, `wsol_token`, `weth_token`, `clawpump`, `clawvault`, `lobsterlend`, and `moltbridge`.

---

### 0.2 — Missing Access Control on Reward Functions
**File:** `contracts/dex_rewards/src/lib.rs`
**Impact:** Unlimited fraudulent reward farming — anyone can inflate volume/LP rewards.

| # | Function | Fix |
|---|----------|-----|
| 0.2a | `record_trade()` | Add caller whitelist check: `get_caller()` must be dex_core or dex_amm contract address |
| 0.2b | `accrue_lp_rewards()` | Add caller whitelist check: `get_caller()` must be dex_amm contract address |

**Implementation:** Store authorized caller addresses in contract storage during `initialize()`. Check `get_caller()` against whitelist at function entry.

---

### 0.3 — Validator Block Replay Causes State Divergence
**File:** `validator/src/main.rs` (~L814–830)
**Impact:** Non-producing validators never apply block state changes. Fees double-charged. **Breaks multi-validator consensus fundamentally.**

**Fix:** Create a `replay_transaction()` method that:
- Skips fee charging (producer already charged)
- Skips signature verification (producer already verified)
- Skips blockhash validation
- Skips duplicate tx hash check
- ONLY applies state mutations (transfers, contract calls, staking ops)

This is the single highest-priority fix in the entire audit.

---

### 0.4 — Slashing Has No Persistent Effect
**File:** `validator/src/main.rs` (~L5622–5660)
**Impact:** Economic punishment is theater — slashed validators restart with full stake.

**Fix:**
1. After `slasher.apply_economic_slashing()`, write updated `Account` back to `StateStore`:
   - Debit `account.staked` by `slashed_amount`
   - Debit `account.shells` by `slashed_amount`
   - Call `state.put_account(&validator_pubkey, &account)`
2. Persist updated `StakePool` immediately after slash
3. Emit a `SlashEvent` for indexing

---

### 0.5 — Block Revert Only Handles System Transfers
**File:** `validator/src/main.rs` (~L873–970)
**Impact:** Fork switches corrupt contract state irreversibly. DEX trades, bridge locks, DAO votes from reverted blocks remain in state.

**Fix:** Implement state snapshots before block application:
1. Before `apply_block_effects()`, create a RocksDB checkpoint/snapshot
2. On revert, restore from snapshot instead of trying to undo individual transactions
3. Alternative: maintain a write-ahead undo log for ALL state mutations during block processing

---

### 0.6 — Non-Atomic Block State Mutations
**File:** `validator/src/main.rs` (~L1002–1285)
**Impact:** Crash during block processing = permanent fund loss or inconsistent state.

**Fix:** Wrap all state mutations for a single block in a `WriteBatch`:
```rust
let mut batch = state.begin_batch();
// ... all put_account, put_program, etc. calls go through batch ...
state.commit_batch(batch);
```
The processor already has `begin_batch`/`commit_batch` — extend to `apply_block_effects`.

---

### 0.7 — EVM Transactions Don't Charge Fees
**File:** `core/src/processor.rs`
**Impact:** Free compute on EVM batch failure — DoS vector via costless EVM calls.

**Fix:** Add `charge_fee_direct()` call for EVM instruction type (type 7) before executing the EVM batch. Match the pattern used for types 0–6.

---

### 0.8 — Fee Split Overflow
**File:** `core/src/processor.rs` + `core/src/genesis.rs`
**Impact:** If burn + producer + voters > 100%, treasury calculation underflows to near-`u64::MAX`, minting ~18 quintillion shells.

**Fix:**
1. In `genesis.rs`, validate that `burn + producer + voters + treasury == 100`
2. In `processor.rs`, use `checked_sub` for `treasury_amount = fee - burn - producer - voters`
3. Add per-field validation: each percentage must be 0..=100

---

### 0.9 — Custody: Solana Keypair Derivation Uses Plain SHA256
**File:** `custody/src/main.rs` (~L2763)
**Impact:** Full theft of all SPL-token sweep deposit addresses. The master seed is not incorporated — anyone who knows the path format can derive private keys.

**Fix:**
1. Replace `Sha256::digest(path.as_bytes())` with `hmac_sha256(&self.master_seed, path.as_bytes())`
2. Rotate ALL affected deposit addresses after deployment
3. Sweep any remaining funds from old addresses

---

### 0.10 — Custody: Unauthenticated Withdrawals When Env Var Missing
**File:** `custody/src/main.rs` (~L3020)
**Impact:** If `CUSTODY_API_AUTH_TOKEN` env var is unset, withdrawal endpoint is fully open. Anyone with network access drains reserves.

**Fix:**
1. Make `CUSTODY_API_AUTH_TOKEN` **mandatory** — `panic!()` at startup if unset
2. Same treatment for `CUSTODY_MASTER_SEED` (currently falls back to hardcoded insecure default)

---

### 0.11 — Custody: Deposit Event Double-Processing
**File:** `custody/src/main.rs` (~L905–960)
**Impact:** Same deposit triggers duplicate sweep jobs → double-crediting user on MoltChain, inflating supply.

**Fix:**
1. Before enqueueing sweep job, check if one already exists for this `deposit_id`
2. Use `WriteBatch` to atomically write sweep job + status update
3. On startup, reconcile "confirmed" deposits against sweep job queue

---

### 0.12 — Custody: Non-Constant-Time Auth Token Comparison
**File:** `custody/src/main.rs` (~L3025)
**Impact:** Timing side-channel leaks auth token byte-by-byte.

**Fix:** Replace `token == expected_token` with `constant_time_eq::constant_time_eq(token.as_bytes(), expected_token.as_bytes())`. The crate is already in the dependency tree.

---

### 0.13 — RPC: Unbounded getTransaction Full-Chain Scan
**File:** `rpc/src/lib.rs` (~L2660–2680)
**Impact:** Single request for non-existent tx hash scans every slot from tip to genesis. DoS with a few concurrent requests.

**Fix:** Apply the same 1000-slot scan cap as the native `handle_get_transaction`:
```rust
let scan_start = last_slot.saturating_sub(1000);
for slot_num in (scan_start..=last_slot).rev() { ... }
```

---

## Phase 1 — HIGH Fixes (Pre-Launch Required)

### 1.1 — Unchecked Integer Overflow in Account Operations
**File:** `core/src/account.rs`

| # | Function | Issue | Fix |
|---|----------|-------|-----|
| 1.1a | `stake()` | `self.staked += amount` wraps in release | Use `checked_add().ok_or(Error)?` |
| 1.1b | `unstake()` | `self.staked -= amount` wraps | Use `checked_sub().ok_or(Error)?` |
| 1.1c | `lock()` | `self.locked += amount` wraps | Use `checked_add().ok_or(Error)?` |
| 1.1d | `unlock()` | `self.locked -= amount` wraps | Use `checked_sub().ok_or(Error)?` |
| 1.1e | `deduct_spendable()` | `self.shells -= amount` unchecked | Use `checked_sub().ok_or(Error)?` |

---

### 1.2 — Unchecked Overflow in Consensus StakePool
**File:** `core/src/consensus.rs`

| # | Function | Issue | Fix |
|---|----------|-------|-----|
| 1.2a | `StakeInfo::add_reward()` | `earned_rewards +=` unchecked | `checked_add` |
| 1.2b | `claim_rewards()` | `claimed +=` unchecked | `checked_add` |
| 1.2c | `total_stake()` | Sum of all stakes can overflow | `checked_add` in fold |
| 1.2d | `has_supermajority()` | `stake * 3` can overflow u64 | Cast to u128 for comparison |

---

### 1.3 — Epoch/Reward Constants Disagree
**File:** `core/src/consensus.rs` + `core/src/genesis.rs`

| # | Constant | Value | Issue |
|---|----------|-------|-------|
| 1.3a | `SLOTS_PER_EPOCH` | 432,000 | Genesis config `epoch_slots` = 216,000. 2x disagreement. |
| 1.3b | `TRANSACTION_BLOCK_REWARD` | 0.9 MOLT | Genesis `validator_reward_per_block` = 0.01 MOLT. 90x difference. Genesis config value is dead — always uses the constant. |

**Fix:** Remove one source of truth. Either constants drive everything (delete genesis config fields) or genesis config drives everything (read from config at startup, delete constants).

---

### 1.4 — Unstake Request Keyed by Validator, Not (Validator, Staker)
**File:** `core/src/consensus.rs`
**Impact:** One staker can block ALL other stakers from unstaking from a validator.

**Fix:** Change unstake request key from `validator_pubkey` to `(validator_pubkey, staker_pubkey)`.

---

### 1.5 — Validator Announcements Not Verified / Treasury Drain
**File:** `validator/src/main.rs` (~L4245–4370)
**Impact:** Fake validator announcements drain treasury at 100K MOLT each (up to 100M MOLT for 1000 fakes).

| # | Fix Step |
|---|----------|
| 1.5a | Verify announcement signature against `announcement.pubkey` |
| 1.5b | Check `StateStore` for actual staked balance before granting bootstrap |
| 1.5c | Add per-epoch cap on bootstrap grants |
| 1.5d | Add rate limiting on announcement processing |

---

### 1.6 — P2P Transactions Added to Mempool Unvalidated
**File:** `validator/src/main.rs` (~L4140–4155)
**Impact:** Attackers fill mempool with garbage, blocking legitimate transactions.

**Fix:** Before `pool.add_transaction()`:
1. Validate transaction structure
2. Verify at least one signature
3. Check sender has minimum balance for fee

---

### 1.7 — No Double-Block Detection
**File:** `validator/src/main.rs` (~L3680–3730)
**Impact:** Malicious leader broadcasts conflicting blocks for same slot → chain split.

**Fix:** Track `(slot, validator_pubkey)` pairs. Reject second block from same producer for same slot. Record `SlashingEvidence::DoubleBlock`.

---

### 1.8 — moltcoin Transfer/Burn Missing Caller Verification
**File:** `contracts/moltcoin/src/lib.rs`
**Impact:** If SDK doesn't enforce, anyone can transfer/burn anyone's tokens.

| # | Function | Fix |
|---|----------|-----|
| 1.8a | `transfer()` | Verify `get_caller() == from` or use `get_caller()` as source |
| 1.8b | `burn()` | Verify `get_caller() == account` before burning |

---

### 1.9 — moltdao veto_proposal Trusts Caller-Provided Balance
**File:** `contracts/moltdao/src/lib.rs` (~L500)
**Impact:** Single user vetoes any proposal by claiming `u64::MAX` balance.

**Fix:** Replace caller-provided `token_balance` and `reputation` with `call_token_balance()` on-chain query, matching the pattern in `vote_with_reputation()`.

---

### 1.10 — moltswap Reentrancy Guard Broken
**File:** `contracts/moltswap/src/lib.rs` (~L210)
**Impact:** `add_liquidity()` calls `reentrancy_exit()` without `reentrancy_enter()` — clears guard for other functions.

**Fix:** Add `if !reentrancy_enter() { return 0; }` at top of `add_liquidity()`.

---

### 1.11 — moltswap Protocol Fee Not Deducted From Output
**File:** `contracts/moltswap/src/lib.rs` (~L165)
**Impact:** Fee bookkeeping decoupled from actual tokens — protocol fee withdrawal taxes LPs double.

**Fix:** Return `amount_out - protocol_cut` from `accrue_protocol_fee()`.

---

### 1.12 — moltmarket buy_nft Non-Atomic Cross-Contract Calls
**File:** `contracts/moltmarket/src/lib.rs` (~L113)
**Impact:** If payment succeeds but NFT transfer fails, buyer loses funds.

**Fix:** Implement escrow pattern: hold payment until NFT transfer confirms, revert on any failure.

---

### 1.13 — moltauction Refund Failure Treated as Non-Critical
**File:** `contracts/moltauction/src/lib.rs` (~L191)
**Impact:** Previous bidder permanently loses escrowed bid.

**Fix:** Revert entire bid (including new bid acceptance) if refund fails.

---

### 1.14 — moltoracle submit_attestation Has No Authorization
**File:** `contracts/moltoracle/src/lib.rs` (~L495)
**Impact:** Anyone can submit attestations to pass any verification threshold.

**Fix:** Implement authorized attester whitelist. Check `get_caller()` against it.

---

### 1.15 — NFT token_id TOCTOU Race
**File:** `core/src/processor.rs`
**Impact:** Same NFT token_id can be minted twice in the same block.

**Fix:** Check uniqueness against the batch state, not committed state.

---

### 1.16 — RPC deployContract/setContractAbi Bypass Consensus
**File:** `rpc/src/lib.rs` (~L4220, ~L3980)
**Impact:** In multi-validator mode, direct state writes cause permanent fork.

**Fix:** Route through consensus pipeline as transactions. Disable these endpoints entirely when `validator_count > 1`.

---

### 1.17 — Custody: Master Seed Fallback to Hardcoded Value
**File:** `custody/src/main.rs` (~L850)
**Impact:** All custody keys derivable by reading source code.

**Fix:** `panic!()` if `CUSTODY_MASTER_SEED` is not set when `network_id == "mainnet"`. Remove the fallback entirely.

---

### 1.18 — Custody: Solana Confirmation Count Ignored
**File:** `custody/src/main.rs` (~L4560)
**Impact:** Deposits credited after 1 confirmation — reorg can revert the deposit.

**Fix:** Parse `confirmationStatus` and require `"finalized"` status. Compare against `required_confirmations`.

---

### 1.19 — Custody: No RPC Client Timeouts
**File:** `custody/src/main.rs` (~L1220–1460)
**Impact:** Hung upstream RPC freezes entire custody pipeline.

**Fix:** Build clients with `.timeout(Duration::from_secs(30)).connect_timeout(Duration::from_secs(10))`.

---

### 1.20 — Custody: No Withdrawal Rate Limiting
**File:** `custody/src/main.rs` (~L3000–3100)
**Impact:** Valid token → rapid reserve drain.

**Fix:** Add global rate limit (N withdrawals/minute, max value/hour). Per-address velocity checks.

---

### 1.21 — Custody: Threshold Signature Assembly is a Stub
**File:** `custody/src/main.rs` (~L4490–4510)
**Impact:** Multi-signer mode silently produces invalid transactions.

**Fix:** Assert exactly 1 signer at startup. Document limitation. Implement proper MPC assembly before enabling multi-signer.

---

### 1.22 — Custody: Single Auth Token for All Signers
**File:** `custody/src/main.rs` (~L1810)
**Impact:** Compromise of one signer compromises auth to all signers.

**Fix:** Configure per-signer auth tokens: `signer_endpoints: Vec<(url, token)>`.

---

### 1.23 — Lock Ordering Risks Deadlock
**File:** `validator/src/main.rs` (various)
**Impact:** Validator deadlock under load → watchdog restart loop.

**Fix:** Establish global lock order: `validator_set` → `stake_pool` → `vote_aggregator` → `slashing_tracker`. Audit all acquisition sites.

---

## Phase 2 — MEDIUM Fixes (Hardening)

### 2.1 — Host Functions Don't Charge Compute
**File:** `core/src/contract.rs`

| # | Function | Fix |
|---|----------|-----|
| 2.1a | `host_storage_read_result` | Charge `result.len() * BYTE_COST` |
| 2.1b | `host_get_args` | Charge `args.len() * BYTE_COST` |
| 2.1c | `host_get_caller` | Charge flat cost (e.g. 100) |
| 2.1d | `host_set_return_data` | Charge `data.len() * BYTE_COST` |

---

### 2.2 — No Storage Entry Limit Per Contract
**File:** `core/src/contract.rs`
**Impact:** Unbounded disk growth.

**Fix:** Add a storage entry cap (e.g. 10,000 entries per contract). Return error when exceeded.

---

### 2.3 — Dual Compute Budget (WASM + Host)
**File:** `core/src/contract.rs`
**Impact:** Effective budget is 2.4M (1.4M WASM metering + 1M host compute), not the intended 1M.

**Fix:** Unify into a single budget: deduct WASM fuel consumption from the host compute meter at each host call boundary.

---

### 2.4 — Fee-Free DoS via Instruction Types 2–5
**File:** `core/src/processor.rs`
**Impact:** Non-treasury accounts can submit certain instruction types without paying fees.

**Fix:** Charge fees for all instruction types. Add `charge_fee_direct()` for types 2–5 matching the pattern for types 0/1/6.

---

### 2.5 — index_program / register_symbol Bypass Batch
**File:** `core/src/processor.rs`
**Impact:** On batch rollback, phantom program index entries persist.

**Fix:** Route index writes through the batch mechanism.

---

### 2.6 — SlashingTracker Not Serializable
**File:** `core/src/consensus.rs`
**Impact:** Slashing evidence lost on restart.

**Fix:** Add `#[derive(Serialize, Deserialize)]` to `SlashingTracker` and all sub-types. Persist to RocksDB.

---

### 2.7 — Identity Gates Fail Open
**Files:** `contracts/{bountyboard,compute_market,clawpay,reef_storage}/src/lib.rs`
**Impact:** MoltyID outage → all gated functions ungated → Sybil attacks.

**Fix:** Return `false` (gate closed) when `call_contract()` for reputation lookup fails.

---

### 2.8 — moltbridge: Validators Can Be Removed Below Confirmation Threshold
**File:** `contracts/moltbridge/src/lib.rs` (~L295)
**Impact:** Bridge permanently stuck — no one can reach confirmation threshold.

**Fix:** Add `if count - 1 < get_required_confirmations() { return error; }` before removing.

---

### 2.9 — moltoracle Storage Key Collision
**File:** `contracts/moltoracle/src/lib.rs` (~L517)
**Impact:** Non-UTF8 data hashes all map to `"attestation_?"` — attestations overwrite each other.

**Fix:** Use `hex_encode()` (already in contract) for the storage key.

---

### 2.10 — moltauction Admin Never Initialized
**File:** `contracts/moltauction/src/lib.rs`
**Impact:** Admin functions permanently inoperable. Escrow address is a hardcoded placeholder.

**Fix:** Add `initialize()` function that sets admin and marketplace address.

---

### 2.11 — Snapshot Sync Trusts Single Peer
**File:** `validator/src/main.rs` (~L4760–4870)
**Impact:** Malicious peer corrupts stake pool / validator set state.

**Fix:** Require matching snapshots from 2+ peers, or validate against block-embedded state root.

---

### 2.12 — Validator Watchdog Uses process::exit
**File:** `validator/src/main.rs` (~L5360)
**Impact:** Skips Drop handlers, potentially leaving RocksDB dirty.

**Fix:** Use `tokio::sync::watch` shutdown signal for graceful shutdown.

---

### 2.13 — Leader Election View-Change Slot Mismatch
**File:** `validator/src/main.rs` (~L5677)
**Impact:** View > 0 leader produces block for wrong slot — potential equivocation.

**Fix:** Produce block for `leader_slot = slot + view` or verify leadership at actual slot.

---

### 2.14 — CORS Allows Any *.moltchain.io Subdomain
**File:** `rpc/src/lib.rs` (~L778)
**Impact:** Subdomain takeover enables cross-origin attacks.

**Fix:** Maintain explicit allowlist of known subdomains.

---

### 2.15 — eth_sendRawTransaction Empty Signatures & Zero Blockhash
**File:** `rpc/src/lib.rs` (~L5401–5420)
**Impact:** Replay protection relies solely on EVM nonce system.

**Fix:** Extract ECDSA sig from RLP, use recent blockhash. Document security assumptions.

---

### 2.16 — RPC Query Endpoints Missing Limit Caps
**File:** `rpc/src/lib.rs`
**Impact:** `get_token_transfers` / `get_contract_events` with `limit: 999999999` → OOM.

**Fix:** Cap all user-supplied limits to 1000 maximum.

---

### 2.17 — RPC Rate Limiter Uses Blocking Mutex
**File:** `rpc/src/lib.rs` (~L213)
**Impact:** Under extreme load, blocks Tokio worker threads.

**Fix:** Replace `std::sync::Mutex` with `tokio::sync::Mutex` or `dashmap` with atomics.

---

### 2.18 — Custody: Reserve Ledger Race Condition
**File:** `custody/src/main.rs` (~L3280)
**Impact:** Multi-instance deployment loses reserve balance updates.

**Fix:** Use RocksDB transactions or enforce single-instance via lock file.

---

### 2.19 — Custody: Deposit Event Cleanup Wrong Key Prefix
**File:** `custody/src/main.rs` (~L3460)
**Impact:** Unbounded growth of deposit event storage.

**Fix:** Key events as `deposit:{deposit_id}:{event_id}` for prefix scan compatibility.

---

### 2.20 — revert_block_effects TOCTOU Race
**File:** `validator/src/main.rs` (~L838–870)
**Impact:** Incorrect fund amounts during fork switches.

**Fix:** Read all needed accounts once, compute all deltas, write once.

---

### 2.21 — moltdao Proposal "Hashing" Is Just Truncation
**File:** `contracts/moltdao/src/lib.rs` (~L168)
**Impact:** Proposals with identical first 32 bytes of title collide.

**Fix:** Use actual SHA-256 hashing.

---

### 2.22 — moltoracle Inconsistent Return Value
**File:** `contracts/moltoracle/src/lib.rs` (~L26)
**Impact:** Callers may misinterpret success as failure. Combined with no re-init guard = retry re-inits.

**Fix:** Return 0 on success for consistency.

---

### 2.23 — musd/wsol/weth approve() Missing Reentrancy Guard
**Files:** `contracts/{musd_token,wsol_token,weth_token}/src/lib.rs`
**Impact:** Potential reentrancy through approve in cross-contract calls.

**Fix:** Add `reentrancy_enter()`/`reentrancy_exit()` to `approve()` matching other state-changing functions.

---

## Phase 3 — LOW Fixes (Polish)

### 3.1 — Zero-Amount Guard Missing on Account Operations
**File:** `core/src/account.rs`
**Fix:** Add `if amount == 0 { return Ok(()); }` at top of `stake`, `unstake`, `lock`, `unlock`.

---

### 3.2 — shells_to_molt Truncation
**File:** `core/src/account.rs`
**Fix:** Document or add `shells_to_molt_rounded_up()` variant for fee calculations.

---

### 3.3 — Floating-Point Reward Calculation
**File:** `core/src/consensus.rs`
**Fix:** Replace `as f64` with fixed-point arithmetic using u128 intermediates.

---

### 3.4 — Multiplicative vs Additive Slashing
**File:** `core/src/consensus.rs`
**Fix:** Document the design choice. If additive is intended, change `remaining_stake * slash_pct / 100` to `original_stake * slash_pct / 100`.

---

### 3.5 — JSON Array for WASM Code (3-4x Storage Bloat)
**File:** `core/src/contract.rs`
**Fix:** Store WASM as base64 string instead of JSON byte array.

---

### 3.6 — Double WASM Compilation on Deploy
**File:** `core/src/contract.rs`
**Fix:** Compile once, cache the module.

---

### 3.7 — Stale ABI on Contract Upgrade
**File:** `core/src/processor.rs`
**Fix:** Clear old ABI on upgrade, require new ABI in upgrade transaction.

---

### 3.8 — Deploy Fee Inconsistency
**File:** `core/src/processor.rs`
**Fix:** Use a single source of truth for deployment fee (either constant or genesis config).

---

### 3.9 — Rent Evasion via Zero-Balance Accounts
**File:** `core/src/processor.rs`
**Fix:** Implement minimum balance requirement or account cleanup.

---

### 3.10 — O(n²) Signature Verification
**File:** `core/src/processor.rs`
**Fix:** Batch-verify signatures using `ed25519_dalek::verify_batch`.

---

### 3.11 — Staking to Non-Existent Validators
**File:** `core/src/processor.rs`
**Fix:** Check validator exists in `StakePool`/`ValidatorSet` before accepting stake.

---

### 3.12 — REWARD_POOL_MOLT Legacy Path Inconsistency
**File:** `validator/src/main.rs` (~L47)
**Fix:** Remove legacy single-treasury path or reconcile with whitepaper distribution.

---

### 3.13 — Fee Split Log Shows Hardcoded Values
**File:** `validator/src/main.rs` (~L5089)
**Fix:** Log actual values from `fee_config`.

---

### 3.14 — record_block_activity Swallows Errors
**File:** `validator/src/main.rs` (~L286–380)
**Fix:** Count errors, halt if threshold reached.

---

### 3.15 — Peer Store Written Without fsync
**File:** `validator/src/main.rs` (~L2380)
**Fix:** Add `file.sync_all()` after write.

---

### 3.16 — RPC Admin Token Not Rotatable
**File:** `rpc/src/lib.rs` (~L140)
**Fix:** Support SIGHUP-triggered token reload.

---

### 3.17 — Non-Saturating Event Counters (musd/wsol/weth)
**Files:** Token contracts
**Fix:** Use `saturating_add(1)` for event counter.

---

### 3.18 — moltpunks Re-init Guard Depends on SDK
**File:** `contracts/moltpunks/src/lib.rs`
**Fix:** Add explicit guard like other contracts.

---

### 3.19 — moltswap TWAP wrapping_add
**File:** `contracts/moltswap/src/lib.rs`
**Fix:** Document the wrapping behavior for TWAP consumers.

---

### 3.20 — moltswap Double Pool Load in Swap Functions
**File:** `contracts/moltswap/src/lib.rs` (~L259)
**Fix:** Load pool once, reuse for both price check and swap.

---

### 3.21 — `is_multiple_of` Needs Nightly Rust
**File:** `core/src/lib.rs`
**Fix:** Replace with `value % divisor == 0`.

---

### 3.22 — Genesis testnet/mainnet Configs Identical
**File:** `core/src/genesis.rs`
**Fix:** Differentiate configs (lower stakes, faster epochs for testnet).

---

### 3.23 — Genesis Fee Percentages Not Individually Validated
**File:** `core/src/genesis.rs`
**Fix:** Add `0 <= pct <= 100` for each individual fee percentage.

---

### 3.24 — setFeeConfig Allows 0% Burn
**File:** `rpc/src/lib.rs` (~L1460)
**Fix:** Add minimum burn percentage if it's a protocol invariant. Document if intentional.

---

### 3.25 — RPC getProgramStorage Exposes Raw Contract State
**File:** `rpc/src/lib.rs` (~L4700)
**Fix:** Document that contract storage is public, or add access controls.

---

---

## Full Finding Index

| ID | Severity | Area | Summary |
|----|----------|------|---------|
| 0.1a-e | CRITICAL | Contracts | 5 contracts missing re-init guard |
| 0.2a-b | CRITICAL | Contracts | dex_rewards no access control |
| 0.3 | CRITICAL | Validator | replay_block_transactions breaks consensus |
| 0.4 | CRITICAL | Validator | Slashing not persisted |
| 0.5 | CRITICAL | Validator | Block revert only handles system transfers |
| 0.6 | CRITICAL | Validator | Non-atomic block state mutations |
| 0.7 | CRITICAL | Core | EVM tx no fee charge |
| 0.8 | CRITICAL | Core | Fee split overflow |
| 0.9 | CRITICAL | Custody | SHA256 keypair derivation (no master seed) |
| 0.10 | CRITICAL | Custody | Unauthenticated withdrawals |
| 0.11 | CRITICAL | Custody | Deposit double-processing |
| 0.12 | CRITICAL | Custody | Non-constant-time auth comparison |
| 0.13 | CRITICAL | RPC | Unbounded getTransaction scan |
| 1.1a-e | HIGH | Core | Account overflow on stake/unstake/lock/unlock |
| 1.2a-d | HIGH | Core | Consensus StakePool overflow |
| 1.3a-b | HIGH | Core | Epoch/reward constant disagreement |
| 1.4 | HIGH | Core | Unstake request key collision |
| 1.5 | HIGH | Validator | Unsigned announcements drain treasury |
| 1.6 | HIGH | Validator | Unvalidated mempool transactions |
| 1.7 | HIGH | Validator | No double-block detection |
| 1.8a-b | HIGH | Contracts | moltcoin transfer/burn caller unverified |
| 1.9 | HIGH | Contracts | moltdao veto trusts caller balance |
| 1.10 | HIGH | Contracts | moltswap broken reentrancy guard |
| 1.11 | HIGH | Contracts | moltswap fee not deducted |
| 1.12 | HIGH | Contracts | moltmarket non-atomic buy_nft |
| 1.13 | HIGH | Contracts | moltauction lost refunds |
| 1.14 | HIGH | Contracts | moltoracle attestation no auth |
| 1.15 | HIGH | Core | NFT token_id TOCTOU |
| 1.16 | HIGH | RPC | deployContract bypasses consensus |
| 1.17 | HIGH | Custody | Hardcoded master seed fallback |
| 1.18 | HIGH | Custody | Solana confirmations ignored |
| 1.19 | HIGH | Custody | No RPC timeouts |
| 1.20 | HIGH | Custody | No withdrawal rate limiting |
| 1.21 | HIGH | Custody | Threshold sig stub |
| 1.22 | HIGH | Custody | Shared signer auth token |
| 1.23 | HIGH | Validator | Lock ordering deadlock risk |
| 2.1-2.23 | MEDIUM | Various | 23 hardening items |
| 3.1-3.25 | LOW | Various | 25 polish items |

---

## Execution Order

**Week 1 — Phase 0 (CRITICAL):** Fix all 13 critical items. Each is a potential exploit or consensus failure.
- Day 1: 0.1 (re-init guards, 5 contracts — mechanical fix)
- Day 1: 0.2 (dex_rewards access control)
- Day 2: 0.3 (replay_transaction — largest refactor)
- Day 2: 0.7 (EVM fee charge)
- Day 3: 0.4 (persist slashing)
- Day 3: 0.8 (fee overflow)
- Day 4: 0.5 (block revert snapshots) + 0.6 (atomic WriteBatch)
- Day 5: 0.9–0.12 (custody fixes)
- Day 5: 0.13 (RPC scan cap)

**Week 2 — Phase 1 (HIGH):** Fix all 24 high-severity items.
- Days 1-2: Core overflow fixes (1.1, 1.2 — mechanical)
- Day 2: Constants reconciliation (1.3)
- Day 3: Validator fixes (1.4–1.7)
- Day 4: Contract fixes (1.8–1.14)
- Day 5: RPC + Custody fixes (1.15–1.23)

**Week 3 — Phase 2 (MEDIUM):** Harden compute metering, storage limits, gate fixes.

**Week 4 — Phase 3 (LOW):** Polish, documentation, optimization.

**After each day:** Rebuild all 26 contracts + release binary. Run existing test suites. Commit with `AUDIT-FIX: <description>`.
