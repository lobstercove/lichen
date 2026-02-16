# MoltChain Legacy Contracts — Security Audit Report

**Date:** 2026-02-11  
**Auditor:** OpenClaw Security Module  
**Scope:** 11 legacy smart contracts  
**Methodology:** Static analysis across 8 checklist dimensions per contract

---

## EXECUTIVE SUMMARY

| Contract | Lines | Reentrancy Guard | Pause Mechanism | #[no_mangle] | Emoji in log_info | from_raw_parts | Vec/alloc | Severity |
|----------|-------|-----------------|-----------------|---------------|-------------------|----------------|-----------|----------|
| moltcoin | 346 | **NO** | **NO** | 7 | 0 | 7 | None | LOW |
| moltpunks | 453 | **NO** | **NO** | 9 | 0 | 12 | None | LOW |
| bountyboard | 866 | **NO** | **NO** | 9 | 5 types | 9 | Vec | MEDIUM |
| moltoracle | 1080 | **NO** | **NO** | 15 | 9 types | 15 | Vec + vec! | **HIGH** |
| moltdao | 1139 | **NO** | **NO** | 13 | 11 types | 14 | Vec | MEDIUM |
| reef_storage | 1212 | **NO** | **NO** | 15 | 6 types | 15 | Vec | LOW |
| moltbridge | 1931 | **NO** | **NO** | 18 | 6 types | 18 | Vec | **HIGH** |
| moltyid | 3126 | **NO** | YES | 39 | 12 types | 39+ | Vec | MEDIUM |
| clawpump | 1491 | **YES** | YES | 19 | 6+ types | 5+ | Vec | LOW |
| compute_market | 1687 | **NO** | **NO** | 21 | 9 types | 21 | Vec | MEDIUM |
| dex_analytics | 1003 | **NO** | YES | 1 (WASM) | 1 | 0 (ptr::copy) | Vec | LOW |

**Critical findings:** 3 bugs, 8/11 contracts missing reentrancy guards, 8/11 missing pause mechanisms.

---

## 1. MOLTCOIN (`moltcoin/src/lib.rs` — 346 lines)

### 1.1 First 40 Lines
```rust
#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
use moltchain_sdk::{storage_get, storage_set, log_info, bytes_to_u64, u64_to_bytes};
```
- No `extern crate alloc`, no heap allocation. Pure stack-based.
- Imports: `storage_get`, `storage_set`, `log_info`, `bytes_to_u64`, `u64_to_bytes`
- Constants: balance key prefix, allowance key prefix, hex lookup table

### 1.2 Reentrancy Guard: **NONE**
No reentrancy protection. Low risk because moltcoin makes no cross-contract calls.

### 1.3 Pause Mechanism: **NONE**
No pause/unpause capability. Cannot halt operations in an emergency.

### 1.4 `#[no_mangle]` Count: **7**
Lines: 32, 64, 78, 111, 140, 169, 192  
Functions: `initialize`, `balance_of`, `transfer`, `mint`, `burn`, `approve`, `total_supply`

### 1.5 Emoji in `log_info`: **NONE**
All log messages are plain ASCII.

### 1.6 `slice::from_raw_parts` Count: **7**
Every extern function uses `core::slice::from_raw_parts` to convert pointer args.

### 1.7 Overflow-Prone Math: **LOW RISK**
- Line ~38: `initial_supply = 1_000_000 * 1_000_000_000` — compiles to a constant, safe in u64.
- All balance arithmetic uses checked patterns (compare-before-subtract).

### 1.8 `vec!`/`alloc::vec!` Usage: **NONE**
No heap allocation at all.

### Vulnerabilities Found
- **[INFO]** No pause mechanism — cannot stop operations if a vulnerability is discovered post-deployment.

---

## 2. MOLTPUNKS (`moltpunks/src/lib.rs` — 453 lines)

### 2.1 First 40 Lines
```rust
#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
use moltchain_sdk::{storage_get, storage_set, log_info, bytes_to_u64, u64_to_bytes};
```
- MT-721 NFT collection. No alloc.
- Re-initialization guard at line 34.

### 2.2 Reentrancy Guard: **NONE**

### 2.3 Pause Mechanism: **NONE**

### 2.4 `#[no_mangle]` Count: **9**
Lines: 28, 56, 102, 132, 147, 160, 181, 218  
Functions: `initialize`, `mint`, `transfer`, `owner_of`, `balance_of`, `approve`, `transfer_from`, `burn`, `total_minted`

### 2.5 Emoji in `log_info`: **NONE**

### 2.6 `slice::from_raw_parts` Count: **12**
Includes both `from_raw_parts` and `from_raw_parts_mut` calls.

### 2.7 Overflow-Prone Math: **NONE**
Token IDs are sequential u64 counters; no multiplication chains.

### 2.8 `vec!`/`alloc::vec!` Usage: **NONE**

### Vulnerabilities Found
- **[INFO]** No pause mechanism.

---

## 3. BOUNTYBOARD (`bountyboard/src/lib.rs` — 866 lines)

### 3.1 First 40 Lines
```rust
#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::vec::Vec;
use moltchain_sdk::{...};
```
- Bounty management with MoltyID integration.
- Uses `alloc::vec::Vec` for dynamic data.

### 3.2 Reentrancy Guard: **NONE**
**VULNERABILITY:** `approve_work` (line ~288) performs a cross-contract `call_token_transfer` to pay the bounty hunter. No reentrancy guard protects this flow.

### 3.3 Pause Mechanism: **NONE**

### 3.4 `#[no_mangle]` Count: **9**
Lines: 138, 208, 288, 395, 447, 477, 493, 513, 532  
Functions: `create_bounty`, `submit_work`, `approve_work`, `cancel_bounty`, `get_bounty`, `set_identity_admin`, `set_moltyid_address`, `set_identity_gate`, `set_token_address`

### 3.5 Emoji in `log_info`: **5 types**
📋, ❌, ✅, 📝, ⚠️

### 3.6 `slice::from_raw_parts` Count: **9**

### 3.7 Overflow-Prone Math: **LOW RISK**
Bounty amounts are stored as u64; no multiplication chains found.

### 3.8 `vec!`/`alloc::vec!` Usage: **YES**
Uses `alloc::vec::Vec` throughout for building storage keys and bounty records.

### Vulnerabilities Found
- **[MEDIUM]** `approve_work` (line ~288): Marks bounty as `STATUS_COMPLETED` **before** calling `call_token_transfer`. On `Err`, it reverts status — but on `Ok(false)` (transfer returned false), the bounty remains completed without payment. Should check `Ok(true)` explicitly.
- **[LOW]** No reentrancy guard on cross-contract call path.
- **[INFO]** No pause mechanism.

---

## 4. MOLTORACLE (`moltoracle/src/lib.rs` — 1080 lines)

### 4.1 First 40 Lines
```rust
#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;
use moltchain_sdk::{...};
```
- Decentralized oracle: price feeds, VRF commit-reveal, attestations.
- Full SHA-256 implementation (NIST FIPS 180-4).

### 4.2 Reentrancy Guard: **NONE**

### 4.3 Pause Mechanism: **NONE**
Oracle has no emergency stop. Price feeders cannot be halted.

### 4.4 `#[no_mangle]` Count: **15**
Lines: 23, 44, 74, 102, 158, 208, 237, 443, 471, 509, 585, 617, 648, 693, 756  
Functions: `initialize_oracle`, `add_price_feeder`, `set_authorized_attester`, `submit_price`, `get_price`, `commit_randomness`, `reveal_randomness`, `request_randomness`, `get_randomness`, `submit_attestation`, `verify_attestation`, `get_attestation_data`, `query_oracle`, `get_aggregated_price`, `get_oracle_stats`

### 4.5 Emoji in `log_info`: **9 types**
🔮, ✅, 👤, ❌, 📊, 🎲, 📝, ⚠️, 📈

### 4.6 `slice::from_raw_parts` Count: **15**

### 4.7 Overflow-Prone Math: **LOW RISK**
SHA-256 uses u32 wrapping arithmetic (`.wrapping_add()`) — correct by design.

### 4.8 `vec!`/`alloc::vec!` Usage: **YES**
- `alloc::vec::Vec` used extensively
- `vec![0u8; 32 - data.len()]` at line ~509 for padding

### Vulnerabilities Found
- **[CRITICAL] `verify_attestation` key mismatch (line ~517):** Uses `core::str::from_utf8(data_hash)` to build the lookup key, while `submit_attestation` uses `hex_encode(data_hash)`. For any data hash containing non-UTF8 bytes (which is most SHA-256 hashes), the key will either produce a different string or error with `Err`, causing **all attestation verifications to fail**. This completely breaks the attestation verification system.
  ```rust
  // submit_attestation (CORRECT):
  key.extend_from_slice(&hex_encode(data_hash));
  
  // verify_attestation (BROKEN):
  key.extend_from_slice(core::str::from_utf8(data_hash).unwrap_or("").as_bytes());
  ```
- **[LOW]** `get_oracle_stats` uses `.and_then(|d| Some(...))` instead of `.map(...)` — style issue only.
- **[INFO]** No pause mechanism; no reentrancy guard.

---

## 5. MOLTDAO (`moltdao/src/lib.rs` — 1139 lines)

### 5.1 First 40 Lines
```rust
#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::vec::Vec;
use moltchain_sdk::{...};
```
- DAO with 3 proposal types, quadratic voting, veto mechanism.
- Includes full SHA-256 and `isqrt` implementations.

### 5.2 Reentrancy Guard: **NONE**
**VULNERABILITY:** `treasury_transfer` (line ~721) and `execute_proposal` (line ~592) make cross-contract calls via `call_token_transfer` without reentrancy protection.

### 5.3 Pause Mechanism: **NONE**

### 5.4 `#[no_mangle]` Count: **13**
Lines: 48, 226, 243, 342, 358, 461, 592, 676, 721, 774, 809, 831, 860  
Functions: `initialize_dao`, `create_proposal`, `create_proposal_typed`, `vote`, `vote_with_reputation`, `execute_proposal`, `veto_proposal`, `cancel_proposal`, `treasury_transfer`, `get_treasury_balance`, `get_proposal`, `get_dao_stats`, `get_active_proposals`

### 5.5 Emoji in `log_info`: **11 types**
🏛️, 📝, ✅, ❌, 🗳️, ⚡, 🚫, 🎉, 💰, 📊, 📋

### 5.6 `slice::from_raw_parts` Count: **14**

### 5.7 Overflow-Prone Math: **MEDIUM RISK**
- **Line ~196** in `governance_voting_power`:
  ```rust
  (base as u128 * capped as u128 / 1000) as u64
  ```
  The u128 intermediate is safe, but the final `as u64` truncates without checking if the result exceeds `u64::MAX`. For extremely large token balances + 3x multiplier, this could silently truncate.

### 5.8 `vec!`/`alloc::vec!` Usage: **YES**
`Vec::with_capacity` and `Vec` used for proposal data and key construction.

### Vulnerabilities Found
- **[MEDIUM]** Voting power truncation at line ~196: `as u64` cast from u128 without overflow check.
- **[LOW]** No reentrancy guard on `treasury_transfer` and `execute_proposal` cross-contract calls.
- **[INFO]** No pause mechanism.

---

## 6. REEF_STORAGE (`reef_storage/src/lib.rs` — 1212 lines)

### 6.1 First 40 Lines
```rust
#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::vec::Vec;
use moltchain_sdk::{...};
```
- Decentralized storage v2 with proof-of-storage, slashing, marketplace pricing.

### 6.2 Reentrancy Guard: **NONE**

### 6.3 Pause Mechanism: **NONE**
Has admin-only functions but no global pause.

### 6.4 `#[no_mangle]` Count: **15**
Lines: 216, 291, 407, 437, 477, 510, 524, 539, 559, 594, 613, 625, 651, 731, 787  
Functions: `store_data`, `confirm_storage`, `get_storage_info`, `register_provider`, `claim_storage_rewards`, `initialize`, `set_challenge_window`, `set_slash_percent`, `stake_collateral`, `set_storage_price`, `get_storage_price`, `get_provider_stake`, `issue_challenge`, `respond_challenge`, `slash_provider`

### 6.5 Emoji in `log_info`: **6 types**
📦, ❌, ✅, 🔌, 💰, ⚡

### 6.6 `slice::from_raw_parts` Count: **15**

### 6.7 Overflow-Prone Math: **SAFE**
Uses `saturating_mul`, `saturating_add`, `saturating_sub` throughout — excellent pattern.

### 6.8 `vec!`/`alloc::vec!` Usage: **YES**
`alloc::vec::Vec` for key construction and data serialization.

### Vulnerabilities Found
- **[INFO]** No reentrancy guard (low risk since no cross-contract token transfers in main flow).
- **[INFO]** No pause mechanism.
- **[POSITIVE]** Excellent use of saturating arithmetic throughout.

---

## 7. MOLTBRIDGE (`moltbridge/src/lib.rs` — 1931 lines)

### 7.1 First 40 Lines
```rust
#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::vec::Vec;
use moltchain_sdk::{...};
```
- Cross-chain bridge v2 with multi-validator confirmation, expiry, MoltyID gating.

### 7.2 Reentrancy Guard: **NONE**
**VULNERABILITY:** Bridge handles large token amounts across chains. `lock_tokens` reduces balances without re-entry protection.

### 7.3 Pause Mechanism: **NONE**
A **bridge** with no emergency stop is a critical gap.

### 7.4 `#[no_mangle]` Count: **18**
Lines: 228, 259, 301, 351, 377, 411, 491, 590, 676, 785, 873, 922, 944, 965, 985, 1005, 1030, 1046  
Functions: `initialize`, `add_bridge_validator`, `remove_bridge_validator`, `set_required_confirmations`, `set_request_timeout`, `lock_tokens`, `submit_mint`, `confirm_mint`, `submit_unlock`, `confirm_unlock`, `cancel_expired_request`, `get_bridge_status`, `has_confirmed_mint`, `has_confirmed_unlock`, `is_source_tx_used`, `is_burn_proof_used`, `set_moltyid_address`, `set_identity_gate`

### 7.5 Emoji in `log_info`: **6 types**
🌉, ❌, ✅, 🔒, 📝, ✍️

### 7.6 `slice::from_raw_parts` Count: **18**

### 7.7 Overflow-Prone Math: **LOW RISK**
Bridge amounts are stored/compared as u64; locked_amount tracking uses simple add/subtract with checks.

### 7.8 `vec!`/`alloc::vec!` Usage: **YES**
`Vec::with_capacity` for key construction and cross-contract call args.

### Vulnerabilities Found
- **[CRITICAL] Wrong storage key in `remove_bridge_validator` (line ~301):** The function reads `bridge_required_confirmations` to get the required confirmation count, but `set_required_confirmations` stores it as `bridge_required_confirms`. This means `remove_bridge_validator` will always read `None` for the required confirmations, causing it to use the default value instead of the configured one. If the default is lower than the actual configured value, this can weaken multi-sig security during validator removal.
  ```rust
  // set_required_confirmations (stores):
  storage_set(b"bridge_required_confirms", &u64_to_bytes(count));
  
  // remove_bridge_validator (reads wrong key):
  storage_get(b"bridge_required_confirmations")  // ← KEY MISMATCH
  ```
- **[HIGH]** No pause mechanism on a **bridge** — the highest-risk contract type. A vulnerability in production would be unexploitable without redeployment.
- **[MEDIUM]** No reentrancy guard despite handling substantial token locks/unlocks.

---

## 8. MOLTYID (`moltyid/src/lib.rs` — 3126 lines)

### 8.1 First 40 Lines
```rust
#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::vec::Vec;
use moltchain_sdk::{...};
```
- Agent identity & reputation system with .MOLT naming, achievements, attestations, discovery registry.

### 8.2 Reentrancy Guard: **NONE**
No cross-contract token calls in the main identity flows, so risk is low.

### 8.3 Pause Mechanism: **YES** ✅
- `MID_PAUSE_KEY` storage key
- `is_mid_paused()` check function
- `mid_pause()` / `mid_unpause()` admin-only functions (lines 2211, 2223)
- Blocks: `register_identity`, `vouch`

### 8.4 `#[no_mangle]` Count: **39**
Lines: 308, 341, 445, 486, 591, 675, 752, 790, 927, 953, 1000, 1023, 1069, 1167, 1199, 1297, 1379, 1418, 1476, 1589, 1622, 1662, 1743, 1805, 1860, 1890, 1913, 1943, 1965, 1992, 2015, 2037, 2065, 2172, 2211, 2223, 2235 (+ 2 more)  
Functions include: `initialize`, `register_identity`, `get_identity`, `update_reputation_typed`, `update_reputation`, `add_skill`, `get_skills`, `vouch`, `get_reputation`, `deactivate_identity`, `get_identity_count`, `update_agent_type`, `get_vouches`, `award_contribution_achievement`, `get_achievements`, `attest_skill`, `get_attestations`, `revoke_attestation`, `register_name`, `resolve_name`, `reverse_resolve`, `transfer_name`, `renew_name`, `release_name`, `set_endpoint`, `get_endpoint`, `set_metadata`, `get_metadata`, `set_availability`, `get_availability`, `set_rate`, `get_rate`, `get_agent_profile`, `get_trust_tier`, `mid_pause`, `mid_unpause`, `transfer_admin`

### 8.5 Emoji in `log_info`: **12 types**
🪪, ❌, ✅, 🏅, 🏆, 🔤, 🔄, ⏸️, ▶️, 📛 (+ others)

### 8.6 `slice::from_raw_parts` Count: **39+**
Every extern function converts pointer args. Highest count of any contract.

### 8.7 Overflow-Prone Math: **NONE**
Reputation arithmetic uses simple add/subtract with bounds checking.

### 8.8 `vec!`/`alloc::vec!` Usage: **YES**
Heavy use of `Vec::with_capacity` for key construction, profile assembly, and achievement data.

### Vulnerabilities Found
- **[LOW]** `skill_name_hash` (line ~1167 area) uses first 8 bytes of skill name as hash — collision-prone for skills starting with the same 8 characters (e.g., "solidity-" and "solidity_" would collide).
- **[POSITIVE]** Has pause mechanism, admin transfer, vouch cooldown, registration cooldown.

---

## 9. CLAWPUMP (`clawpump/src/lib.rs` — 1491 lines)

### 9.1 First 40 Lines
```rust
#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::vec::Vec;
use moltchain_sdk::{...};
```
- Token launchpad with bonding curves, DEX graduation, anti-manipulation.

### 9.2 Reentrancy Guard: **YES** ✅
- `REENTRANCY_KEY` storage key
- `reentrancy_enter()` / `reentrancy_exit()` pattern
- Applied to `buy` and `sell` functions

### 9.3 Pause Mechanism: **YES** ✅
- `PAUSE_KEY` storage key
- `is_paused()` check
- `pause()` / `unpause()` admin-only functions (lines 675, 692)
- Token-level freeze: `freeze_token()` / `unfreeze_token()` (lines 703, 714)

### 9.4 `#[no_mangle]` Count: **19**
Lines: 200, 223, 310, 523, 615, 641, 669, 675, 692, 703, 714, 726, 738, 747, 756, 766, 776, 790, 816  
Functions: `initialize`, `create_token`, `buy`, `sell`, `get_token_info`, `get_buy_quote`, `get_token_count`, `get_platform_stats`, `pause`, `unpause`, `freeze_token`, `unfreeze_token`, `set_buy_cooldown`, `set_sell_cooldown`, `set_max_buy`, `set_creator_royalty`, `withdraw_fees`, `set_dex_addresses`, `get_graduation_info`

### 9.5 Emoji in `log_info`: **6+ types**
🚀, ❌, ✅, 🎓, ⏸️, 🧊

### 9.6 `slice::from_raw_parts` Count: **5+**
Lower count because many functions take scalar parameters (token_id, amount).

### 9.7 Overflow-Prone Math: **SAFE**
- Bonding curve uses `u128` intermediates for all multiplications:
  ```rust
  let cost = (supply_sold as u128 * SLOPE as u128 / SLOPE_SCALE as u128) as u64;
  ```
- `current_price` function uses u64 multiplication that could overflow for very large supply values, but it's always called within u128 context.

### 9.8 `vec!`/`alloc::vec!` Usage: **YES**
`alloc::vec::Vec` for key construction and cross-contract call args.

### Vulnerabilities Found
- **[POSITIVE]** Best-hardened contract in the suite. Has reentrancy guard, pause, freeze, cooldowns, max buy limits, creator royalties.
- **[LOW]** `buy`/`sell` return 0 for both error and graduated/paused/frozen states — callers cannot distinguish between "buy returned 0 tokens" and "buy was blocked". Consider distinct error codes.

---

## 10. COMPUTE_MARKET (`compute_market/src/lib.rs` — 1687 lines)

### 10.1 First 40 Lines
```rust
#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::vec::Vec;
use moltchain_sdk::{...};
```
- Decentralized compute marketplace v2 with escrow, timeouts, arbitration, MoltyID gating.

### 10.2 Reentrancy Guard: **NONE**
**VULNERABILITY:** `release_payment` and `resolve_dispute` release escrowed funds via `call_token_transfer` cross-contract calls without reentrancy protection.

### 10.3 Pause Mechanism: **NONE**

### 10.4 `#[no_mangle]` Count: **21**
Lines: 242, 297, 368, 425, 492, 542, 562, 575, 591, 607, 623, 640, 666, 737, 796, 859, 886, 913, 945, 970, 986, 1006  
Functions: `register_provider`, `submit_job`, `claim_job`, `complete_job`, `dispute_job`, `get_job`, `initialize`, `set_claim_timeout`, `set_complete_timeout`, `set_challenge_period`, `add_arbitrator`, `remove_arbitrator`, `cancel_job`, `release_payment`, `resolve_dispute`, `deactivate_provider`, `reactivate_provider`, `update_provider`, `get_escrow`, `set_identity_admin`, `set_moltyid_address`, `set_identity_gate`

### 10.5 Emoji in `log_info`: **9 types**
🖥️, ❌, ✅, 📋, 🤝, ⚠️, 🚫, 💰, ⚖️

### 10.6 `slice::from_raw_parts` Count: **21**

### 10.7 Overflow-Prone Math: **LOW RISK**
Escrow amounts are simple u64 values; dispute resolution splits use percentage (0-100).

### 10.8 `vec!`/`alloc::vec!` Usage: **YES**
`alloc::vec::Vec` for key construction and cross-contract calls.

### Vulnerabilities Found
- **[MEDIUM]** No reentrancy guard on `release_payment` (line ~737) and `resolve_dispute` (line ~796) which both call `call_token_transfer`.
- **[INFO]** No pause mechanism.

---

## 11. DEX_ANALYTICS (`dex_analytics/src/lib.rs` — 1003 lines)

### 11.1 First 40 Lines
```rust
// DEX Analytics — On-Chain OHLCV, Volume Tracking, Leaderboards (DEEP hardened)
#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
extern crate alloc;
use alloc::vec::Vec;
use moltchain_sdk::{bytes_to_u64, get_slot, log_info, storage_get, storage_set, u64_to_bytes};
```
- OHLCV candle aggregation (9 intervals: 1m→1y), 24h rolling stats, trader stats, leaderboards.
- All prices denominated in mUSD (6 decimals).

### 11.2 Reentrancy Guard: **NONE**
Low risk — analytics contract performs no token transfers.

### 11.3 Pause Mechanism: **YES** ✅
- `PAUSED_KEY` storage key
- `is_paused()` function
- `emergency_pause()` / `emergency_unpause()` admin-only functions
- **NOTE:** `record_trade` does **not** check `is_paused()` — the pause flag exists but is unused in the main write path!

### 11.4 `#[no_mangle]` Count: **1** (WASM dispatcher)
Line ~592: Single `#[cfg(target_arch = "wasm32")] #[no_mangle] pub extern "C" fn call()` — uses an ABI dispatch pattern (match on args[0]) rather than individual exports.

### 11.5 Emoji in `log_info`: **1**
Only "DEX Analytics initialized" and "DEX Analytics: EMERGENCY PAUSE" — no emoji.

### 11.6 `slice::from_raw_parts` Count: **0**
Uses `core::ptr::copy_nonoverlapping` instead — safer pattern for fixed-size copies.

### 11.7 Overflow-Prone Math: **LOW RISK**
- `vol += volume` in `update_24h_stats` — could overflow on extremely high-volume pairs (u64::MAX = 18.4 quintillion). Low practical risk.
- `count + 1` in `record_trade` — increment without overflow check. Practically safe.

### 11.8 `vec!`/`alloc::vec!` Usage: **YES**
- `alloc::vec![b'0']` in `u64_to_decimal`
- `Vec::with_capacity` extensively for key construction and candle encoding.

### Vulnerabilities Found
- **[MEDIUM]** `record_trade` (line ~340) does **not** check `is_paused()`. The emergency pause mechanism exists but has no effect on the primary write function. An attacker could continue recording trades during a pause.
- **[LOW]** Candle retention policy (`get_retention()`) is implemented but **never enforced** — old candles are never pruned. Storage will grow unbounded over time.
- **[LOW]** `update_24h_stats` volume uses simple addition without `saturating_add` — could theoretically overflow.

---

## CROSS-CONTRACT VULNERABILITY SUMMARY

### CRITICAL (2)

| # | Contract | Issue | Impact |
|---|----------|-------|--------|
| 1 | **moltoracle** | `verify_attestation` uses `str::from_utf8` vs `hex_encode` for key — keys never match | All attestation verification broken |
| 2 | **moltbridge** | `remove_bridge_validator` reads `bridge_required_confirmations` instead of `bridge_required_confirms` | May use wrong confirmation threshold during validator removal |

### HIGH (1)

| # | Contract | Issue | Impact |
|---|----------|-------|--------|
| 3 | **moltbridge** | No pause mechanism on a cross-chain bridge | Cannot halt bridge operations during active exploit |

### MEDIUM (5)

| # | Contract | Issue | Impact |
|---|----------|-------|--------|
| 4 | **bountyboard** | `approve_work` does not check `Ok(true)` from token transfer | Bounty marked completed without successful payment |
| 5 | **moltdao** | `governance_voting_power` truncates u128→u64 without check | Silent voting power truncation for whale accounts |
| 6 | **moltdao** | No reentrancy guard on `treasury_transfer` cross-contract call | Potential reentrant treasury drain |
| 7 | **compute_market** | No reentrancy guard on `release_payment`/`resolve_dispute` | Potential reentrant escrow drain |
| 8 | **dex_analytics** | `record_trade` ignores pause flag | Trades recorded during emergency pause |

### LOW (5)

| # | Contract | Issue | Impact |
|---|----------|-------|--------|
| 9 | **dex_analytics** | Candle retention never enforced — unbounded storage growth | Storage bloat over time |
| 10 | **moltyid** | `skill_name_hash` uses first 8 bytes — collision prone | Skills with same 8-byte prefix collide |
| 11 | **clawpump** | `buy`/`sell` return 0 for both error and success with 0 tokens | Ambiguous return values |
| 12 | **bountyboard** | No reentrancy guard on cross-contract token transfer | Low-risk reentry vector |
| 13 | **moltbridge** | No reentrancy guard on lock/unlock token flows | Reentry during high-value operations |

---

## RECOMMENDATIONS

1. **Immediate:** Fix the moltoracle `verify_attestation` key encoding (CRITICAL bug — change `str::from_utf8` to `hex_encode`).
2. **Immediate:** Fix the moltbridge storage key mismatch (`bridge_required_confirmations` → `bridge_required_confirms`).
3. **Urgent:** Add pause mechanism to moltbridge — this is a bridge contract handling cross-chain value.
4. **High Priority:** Add reentrancy guards to bountyboard, moltdao, compute_market wherever `call_token_transfer` or `call_contract` is invoked.
5. **Medium Priority:** Fix dex_analytics `record_trade` to respect `is_paused()`.
6. **Medium Priority:** Add overflow check to moltdao `governance_voting_power` u128→u64 cast.
7. **Low Priority:** Implement candle retention pruning in dex_analytics.
8. **Low Priority:** Add pause mechanisms to remaining contracts (moltcoin, moltpunks, bountyboard, moltoracle, moltdao, compute_market, reef_storage).

---

*End of audit report.*
