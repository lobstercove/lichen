# Moltchain Smart Contract Pattern Audit Report
**Date:** 2026-02-14  
**Scope:** All 27 contracts in `contracts/`  
**Gold Standard:** `dex_core` model (pub fn + single `call()` dispatch)

---

## EXECUTIVE SUMMARY

| Category | Count | Contracts |
|----------|-------|-----------|
| **dex_core model** (pub fn + dispatch) | 8 | dex_core, dex_amm, dex_router, dex_governance, dex_margin, dex_rewards, dex_analytics, prediction_market |
| **musd_token model** (extern C every fn, `copy_nonoverlapping`) | 3 | musd_token, wsol_token, weth_token |
| **Legacy model** (extern C every fn, `from_raw_parts`) | 16 | moltcoin, moltdao, moltswap, moltbridge, moltmarket, moltoracle, moltauction, moltpunks, moltyid, lobsterlend, clawpay, clawpump, clawvault, bountyboard, compute_market, reef_storage |

---

## 1. FUNCTION EXPORT PATTERN

### dex_core model (8 contracts) ✅ GOLD STANDARD
Single `#[cfg(target_arch = "wasm32")] #[no_mangle] pub extern "C" fn call()` entry point.  
All business logic in `pub fn` functions (not exported individually).

| Contract | `#[no_mangle]` | `pub extern "C"` | `pub fn` (top) | `fn call()` |
|----------|:-:|:-:|:-:|:-:|
| dex_core | 1 | 1 | 21 | 1 |
| dex_amm | 1 | 1 | 16 | 1 |
| dex_router | 1 | 1 | 12 | 1 |
| dex_governance | 1 | 1 | 15 | 1 |
| dex_margin | 1 | 1 | 20 | 1 |
| dex_rewards | 1 | 1 | 18 | 1 |
| dex_analytics | 1 | 1 | 9 | 1 |
| prediction_market | 1 | 1 | 35 | 1 |

### musd_token model (3 contracts) ⚠️ DIVERGENT
Every function individually exported as `#[no_mangle] pub extern "C" fn`. No dispatch.

| Contract | `#[no_mangle]` | `pub extern "C"` | `fn call()` |
|----------|:-:|:-:|:-:|
| musd_token | 20 | 20 | 0 |
| wsol_token | 20 | 20 | 0 |
| weth_token | 20 | 20 | 0 |

### Legacy model (16 contracts) ⚠️ DIVERGENT
Same as musd_token model — every function individually exported. No dispatch.

| Contract | `#[no_mangle]` | `pub extern "C"` | `fn call()` |
|----------|:-:|:-:|:-:|
| moltcoin | 7 | 7 | 0 |
| moltdao | 13 | 13 | 0 |
| moltswap | 22 | 22 | 0 |
| moltbridge | 18 | 18 | 0 |
| moltmarket | 11 | 11 | 0 |
| moltoracle | 15 | 15 | 0 |
| moltauction | 15 | 15 | 0 |
| moltpunks | 9 | 9 | 0 |
| moltyid | 37 | 37 | 0 |
| lobsterlend | 16 | 16 | 0 |
| clawpay | 14 | 14 | 0 |
| clawpump | 19 | 19 | 0 |
| clawvault | 18 | 18 | 0 |
| bountyboard | 9 | 9 | 0 |
| compute_market | 22 | 22 | 0 |
| reef_storage | 15 | 15 | 0 |

---

## 2. POINTER HANDLING

| Pattern | Contracts |
|---------|-----------|
| **`copy_nonoverlapping` + `[0u8; 32]` locals** ✅ | dex_core (15), dex_amm (10), dex_router (18), dex_governance (14), dex_margin (14), dex_rewards (19), dex_analytics (5), prediction_market (33), musd_token (20), wsol_token (20), weth_token (20) |
| **`from_raw_parts`** ⚠️ | moltcoin (9), moltdao (15), moltswap (15), moltbridge (26), moltmarket (9), moltoracle (21), moltauction (25), moltpunks (14), moltyid (56), lobsterlend (15), clawpay (15), clawpump (16), clawvault (17), bountyboard (12), compute_market (24), reef_storage (21) |
| **MIXED (both)** ⚠️ | prediction_market: 33 `copy_nonoverlapping` + 2 residual `from_raw_parts` at L1184, L1374 (variable-length slices for question text and odds array) |
| moltdao (mixed) | 15 `from_raw_parts` + 7 `copy_nonoverlapping` |
| moltauction (mixed) | 25 `from_raw_parts` + 1 `copy_nonoverlapping` |
| moltoracle (mixed) | 21 `from_raw_parts` + 8 `copy_nonoverlapping` |

**Note:** prediction_market's 2 `from_raw_parts` are for *variable-length* data (question string and odds BPS array) where `copy_nonoverlapping` into a fixed `[0u8; 32]` local isn't applicable. This is acceptable.

---

## 3. OPCODE FORMAT (dispatch contracts only)

| Contract | Opcode Format | Opcode Range | Default `_ =>` arm |
|----------|:---:|:---:|:---:|
| dex_core | **Decimal** (0, 1, 2, ...) | 0–20 | `_ => {}` (silent) ✅ |
| dex_amm | **Decimal** | 0–1 | `_ => {}` (silent) ✅ |
| dex_router | **Decimal** | 0–11 | `_ => {}` (silent) ✅ |
| dex_governance | **Decimal** | 0–14 | `_ => {}` (silent) ✅ |
| dex_margin | **Decimal** | 0–15 | `_ => {}` (silent) ✅ |
| dex_rewards | **Decimal** | 0–12 | `_ => {}` (silent) ✅ |
| dex_analytics | **Decimal** | 0–8 | `_ => {}` (silent) ✅ |
| prediction_market | **Decimal** | 0–33 | `_ => {}` (silent) ✅ |

**All 8 dispatch contracts are consistent:** decimal opcodes, silent `_ => {}` default. No hex opcodes found anywhere.

---

## 4. LOGGING

| Contract | `log_info` calls | Emoji lines | Notes |
|----------|:---:|:---:|-------|
| **dex_core** | **8** | **0** | ✅ Gold standard — minimal, no emoji (1 comment has ✓) |
| dex_amm | 7 | 0 | ✅ Consistent with gold standard |
| dex_router | 5 | 0 | ✅ |
| dex_governance | 13 | 0 | ✅ |
| dex_margin | 7 | 0 | ✅ |
| dex_rewards | 9 | 0 | ✅ |
| dex_analytics | 3 | 0 | ✅ |
| prediction_market | 17 | 0 | ✅ |
| musd_token | 10 | 0 | ✅ |
| wsol_token | 10 | 0 | ✅ |
| weth_token | 10 | 0 | ✅ |
| moltcoin | 12 | 0 | OK (no emoji) |
| **moltdao** | **71** | **47** | ⚠️ Heavy emoji: 🏛️ ✅ 📝 🗳️ etc |
| **moltswap** | **37** | **24** | ⚠️ Emoji in logs |
| **moltbridge** | **61** | **60** | ⚠️ Nearly every log has emoji: 🌉 ❌ ✅ |
| moltmarket | 32 | 2 | Minimal emoji |
| **moltoracle** | **52** | **37** | ⚠️ 🔮 ✅ ❌ 👤 emoji |
| **moltauction** | **64** | **51** | ⚠️ 🎯 ✅ ❌ ⚠️ emoji |
| **moltyid** | **104** | **104** | ⚠️ EVERY log has emoji: 🪪 ❌ ✅ |
| **lobsterlend** | **42** | **41** | ⚠️ 🦞 ❌ ✅ emoji |
| **clawpay** | **32** | **31** | ⚠️ 💸 ❌ ✅ emoji |
| **clawpump** | **33** | **32** | ⚠️ 🚀 ❌ ✅ emoji |
| **clawvault** | **22** | **21** | ⚠️ 🏦 ❌ ✅ emoji |
| **bountyboard** | **34** | **33** | ⚠️ 📋 📝 ❌ ✅ emoji |
| **compute_market** | **73** | **72** | ⚠️ 🖥️ 📋 ❌ ✅ emoji |
| **reef_storage** | **39** | **38** | ⚠️ 📦 ❌ ✅ emoji |

**Finding:** ALL 16 legacy contracts use emoji in `log_info` (except moltcoin). No dex_* or token contracts use emoji. Emoji wastes WASM bytes.

---

## 5. CLIPPY / CODE QUALITY CONCERNS

| Issue | Affected Contracts |
|-------|--------------------|
| **No `saturating_sub`/`checked_add`/`checked_mul` anywhere** | ALL 27 contracts ⚠️ |
| Empty `_ => {}` blocks (acceptable in dispatch) | All 8 dispatch contracts |
| Heavy `alloc::format!` in logs (WASM bloat) | moltdao, moltauction, moltoracle, clawpay, bountyboard, compute_market |
| `alloc::vec!` vs bare `vec!` inconsistency | See §8 below |
| Variable-length `from_raw_parts` without bounds check | prediction_market L1184, L1374 (mitigated by dispatch-level length check) |

**Critical:** Zero use of safe arithmetic methods (`saturating_sub`, `checked_add`, etc.) across the entire codebase. All arithmetic is raw `+`, `-`, `*`, `/`. In a financial smart contract environment, this is a **HIGH SEVERITY** finding.

---

## 6. SECURITY PATTERNS

| Contract | Reentrancy Guard | Pause Mechanism | Admin Checks | `get_caller()` |
|----------|:---:|:---:|:---:|:---:|
| **dex_core** | ✅ (46 refs) | ✅ (50 refs) | ✅ | — |
| **dex_amm** | ✅ (44) | ✅ (31) | ✅ | — |
| **dex_router** | ✅ (29) | ✅ (28) | ✅ | — |
| **dex_governance** | ✅ (23) | ✅ (27) | ✅ | — |
| **dex_margin** | ✅ (40) | ✅ (23) | ✅ | — |
| **dex_rewards** | ✅ (21) | ✅ (22) | ✅ | ✅ (2) |
| **dex_analytics** | ❌ **MISSING** | ✅ (21) | ✅ | — |
| **prediction_market** | ✅ (150) | ✅ (41) | ✅ | ✅ (21) |
| **musd_token** | ✅ (35) | ✅ (21) | ✅ | — |
| **wsol_token** | ✅ (35) | ✅ (18) | ✅ | — |
| **weth_token** | ✅ (35) | ✅ (18) | ✅ | — |
| moltcoin | ❌ | ❌ | Partial (owner check) | ✅ (3) |
| moltdao | ❌ | ❌ | Partial | — |
| **moltswap** | ✅ (28) | ❌ | Partial | ✅ (2) |
| moltbridge | ❌ | ❌ | Partial (owner check via ptr) | — |
| moltmarket | ❌ | Mentioned (1 ref) | Partial | — |
| moltoracle | ❌ | ❌ | ✅ (owner check) | ✅ (4) |
| **moltauction** | ❌ | ✅ (28) | ✅ `get_caller()` | ✅ (2) |
| moltpunks | ❌ | ❌ | Partial (minter check) | — |
| moltyid | ❌ | ✅ (38) | ✅ | — |
| **lobsterlend** | ✅ (21) | ✅ (47) | ✅ | — |
| clawpay | ❌ | ✅ (40) | ✅ | — |
| **clawpump** | ✅ (10) | ✅ (26) | ✅ | — |
| clawvault | ❌ | ✅ (31) | ✅ | — |
| bountyboard | ❌ | ❌ | Partial | — |
| compute_market | ❌ | ❌ | Partial | — |
| reef_storage | ❌ | ❌ | Partial | — |

### Security Summary

| Feature | dex_core model (8) | musd_token model (3) | Legacy (16) |
|---------|:---:|:---:|:---:|
| Reentrancy guard | 7/8 ⚠️ | 3/3 ✅ | 3/16 ❌ |
| Pause mechanism | 8/8 ✅ | 3/3 ✅ | 6/16 ⚠️ |
| Admin authorization | 8/8 ✅ | 3/3 ✅ | ~16/16 (partial) |
| `get_caller()` | 2/8 | 0/3 | 3/16 |

**Critical gaps:**
- **dex_analytics**: NO reentrancy guard (only dispatch contract missing it)
- **10 legacy contracts** lack BOTH reentrancy and pause: moltcoin, moltdao, moltbridge, moltmarket, moltpunks, bountyboard, compute_market, reef_storage, moltoracle, clawpay (has pause but no reentrancy), clawvault (has pause but no reentrancy)

---

## 7. `cfg(target_arch = "wasm32")` GATE

| Present (✅) | Missing (❌) |
|-------------|-------------|
| dex_core, dex_amm, dex_router, dex_governance, dex_margin, dex_rewards, dex_analytics, prediction_market (all 8 dispatch contracts) | ALL 19 non-dispatch contracts (musd_token, wsol_token, weth_token, all 16 legacy) |

**Finding:** Non-dispatch contracts export every function unconditionally (no WASM gate). This means all functions are exported when compiled for non-WASM targets too — which is fine for testing but means they can't conditionally exclude exports.

---

## 8. `vec!` MACRO USAGE

| Contract | `alloc::vec!` (qualified) | bare `vec!` | Total |
|----------|:---:|:---:|:---:|
| dex_core | 1 | 0 | 1 ✅ |
| dex_amm | 1 | 0 | 1 ✅ |
| dex_router | 1 | 0 | 1 ✅ |
| dex_governance | 1 | 0 | 1 ✅ |
| dex_margin | 1 | 0 | 1 ✅ |
| dex_rewards | 1 | 0 | 1 ✅ |
| dex_analytics | 1 | 0 | 1 ✅ |
| prediction_market | 3 | 0 | 3 ✅ |
| musd_token | 1 | 0 | 1 ✅ |
| wsol_token | 1 | 0 | 1 ✅ |
| weth_token | 1 | 0 | 1 ✅ |
| moltmarket | 5 | 0 | 5 ✅ (all `alloc::vec!`) |
| moltbridge | 0 | 6 | 6 ⚠️ **bare `vec!`** |
| moltoracle | 0 | 1 | 1 ⚠️ **bare `vec!`** |
| clawpay | 1 | 0 | 1 ✅ |
| All others (12) | 0 | 0 | 0 (no vec usage) |

**Finding:** `moltbridge` (6 uses) and `moltoracle` (1 use) use bare `vec![]` instead of `alloc::vec![]`. This works because of `#[macro_use] extern crate alloc` but is inconsistent with the rest of the codebase.

---

## 9. CARGO.TOML RELEASE PROFILES

**ALL 27 contracts have identical profiles** ✅:

```toml
[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = true
```

No divergence found. This is good.

---

## 10. WASM BINARY SIZES

| Contract | WASM Size (bytes) | Category |
|----------|------------------:|----------|
| moltauction | 70,391 | Legacy |
| prediction_market | 59,655 | dex_core |
| moltyid | 47,230 | Legacy |
| moltswap | 40,534 | Legacy |
| moltdao | 37,362 | Legacy |
| **dex_core** | **35,176** | **dex_core** |
| moltpunks | 32,717 | Legacy |
| moltoracle | 31,948 | Legacy |
| dex_margin | 30,698 | dex_core |
| moltmarket | 29,565 | Legacy |
| moltbridge | 29,513 | Legacy |
| moltcoin | 29,012 | Legacy |
| compute_market | 28,869 | Legacy |
| clawpump | 27,672 | Legacy |
| clawpay | 26,286 | Legacy |
| dex_rewards | 26,030 | dex_core |
| reef_storage | 25,941 | Legacy |
| dex_router | 25,887 | dex_core |
| clawvault | 25,876 | Legacy |
| dex_governance | 25,382 | dex_core |
| musd_token | 23,538 | musd_token |
| wsol_token | 23,538 | musd_token |
| weth_token | 23,522 | musd_token |
| bountyboard | 22,870 | Legacy |
| dex_analytics | 22,633 | dex_core |
| lobsterlend | 21,695 | Legacy |
| dex_amm | 19,543 | dex_core |

### Size Statistics by Category

| Category | Min | Max | Median |
|----------|----:|----:|-------:|
| dex_core model (8) | 19,543 | 59,655 | 26,009 |
| musd_token model (3) | 23,522 | 23,538 | 23,538 |
| Legacy model (16) | 21,695 | 70,391 | 29,263 |

**Note:** prediction_market is the largest dex_core model contract at 59,655 bytes (3,312 lines of source, 34 opcodes). The musd_token model produces the most compact WASMs (≈23.5 KB) since all three tokens are structurally identical.

---

## DIVERGENCE SUMMARY FROM GOLD STANDARD (dex_core)

### What makes the dex_core model the gold standard:
1. Single `call()` entry point with `#[cfg(target_arch = "wasm32")]` guard
2. `pub fn` for business logic (testable without WASM)
3. `copy_nonoverlapping` for safe pointer handling into stack locals
4. Decimal opcodes with silent `_ => {}`
5. Reentrancy guard + pause mechanism + admin authorization
6. `alloc::vec![]` (fully qualified)
7. Minimal `log_info` calls, zero emoji
8. Optimized Cargo.toml release profile

### Per-contract divergence from gold standard:

| Contract | Divergences |
|----------|-------------|
| **dex_amm** | Only 2 opcodes dispatched (minimal dispatch) — otherwise perfect |
| **dex_router** | ✅ Fully compliant |
| **dex_governance** | ✅ Fully compliant |
| **dex_margin** | ✅ Fully compliant |
| **dex_rewards** | ✅ Fully compliant |
| **dex_analytics** | ❌ **MISSING reentrancy guard** — only dispatch contract without one |
| **prediction_market** | 2 residual `from_raw_parts` (L1184, L1374 — acceptable for var-length data); uses `get_caller()` extensively (21 refs) — arguably better authorization than gold standard |
| **musd_token** | No `call()` dispatch; no `cfg(wasm32)` gate; all fns individually exported |
| **wsol_token** | Same as musd_token (structurally identical to musd_token) |
| **weth_token** | Same as musd_token (structurally identical to wsol_token) |
| **moltcoin** | No dispatch; `from_raw_parts`; no reentrancy; no pause; no emoji |
| **moltdao** | No dispatch; `from_raw_parts` + some `copy_nonoverlapping`; no reentrancy; no pause; 47 emoji log lines |
| **moltswap** | No dispatch; `from_raw_parts`; HAS reentrancy; no pause; 24 emoji lines |
| **moltbridge** | No dispatch; `from_raw_parts`; no reentrancy; no pause; **bare `vec!`** (6); 60 emoji lines |
| **moltmarket** | No dispatch; `from_raw_parts`; no reentrancy; minimal pause; `alloc::vec!` (5); 2 emoji |
| **moltoracle** | No dispatch; `from_raw_parts` + `copy_nonoverlapping`; no reentrancy; no pause; **bare `vec!`** (1); 37 emoji |
| **moltauction** | No dispatch; `from_raw_parts` + 1 `copy_nonoverlapping`; no reentrancy; HAS pause; 51 emoji |
| **moltpunks** | No dispatch; `from_raw_parts`; no reentrancy; no pause; no emoji |
| **moltyid** | No dispatch; `from_raw_parts`; no reentrancy; HAS pause; 104 emoji lines (**worst offender**) |
| **lobsterlend** | No dispatch; `from_raw_parts`; HAS reentrancy; HAS pause; 41 emoji |
| **clawpay** | No dispatch; `from_raw_parts`; no reentrancy; HAS pause; 31 emoji |
| **clawpump** | No dispatch; `from_raw_parts`; HAS reentrancy; HAS pause; 32 emoji |
| **clawvault** | No dispatch; `from_raw_parts`; no reentrancy; HAS pause; 21 emoji |
| **bountyboard** | No dispatch; `from_raw_parts`; no reentrancy; no pause; 33 emoji |
| **compute_market** | No dispatch; `from_raw_parts`; no reentrancy; no pause; 72 emoji |
| **reef_storage** | No dispatch; `from_raw_parts`; no reentrancy; no pause; 38 emoji |

---

## PRIORITY ACTION ITEMS

### P0 – Critical Security
1. **Add reentrancy guard to `dex_analytics`** — only dispatch contract missing it
2. **Add safe arithmetic** (`saturating_sub`, `checked_mul`) — zero contracts use it
3. **Add reentrancy guards** to: moltcoin, moltdao, moltbridge, moltmarket, moltoracle, moltauction, moltpunks, moltyid, clawpay, clawvault, bountyboard, compute_market, reef_storage (13 contracts)

### P1 – Architecture Consistency
4. **Refactor 3 token contracts** (musd_token, wsol_token, weth_token) to dex_core dispatch model
5. **Refactor 16 legacy contracts** to dex_core dispatch model (ordered by risk/usage)
6. **Add pause mechanism** to: moltcoin, moltdao, moltswap, moltbridge, moltpunks, bountyboard, compute_market, reef_storage, moltoracle (9 contracts)

### P2 – Code Quality
7. **Remove emoji from `log_info` calls** — 14 contracts affected (saves WASM bytes)
8. **Replace `from_raw_parts` with `copy_nonoverlapping`** in all legacy contracts
9. **Replace bare `vec!` with `alloc::vec!`** in moltbridge (6), moltoracle (1)
10. **Add `cfg(target_arch = "wasm32")` gate** to all contracts being refactored

### P3 – Optimization
11. **Reduce `log_info` density** in legacy contracts (moltyid: 104, compute_market: 73, moltdao: 71)
12. **Reduce `alloc::format!` usage** in hot paths (WASM allocator pressure)

---

## SOURCE LINE COUNTS

| Contract | Lines | Category |
|----------|------:|----------|
| prediction_market | 3,312 | dex_core |
| moltyid | 3,125 | Legacy |
| dex_core | 2,661 | dex_core |
| moltbridge | 1,930 | Legacy |
| compute_market | 1,686 | Legacy |
| clawpump | 1,490 | Legacy |
| dex_margin | 1,449 | Legacy |
| dex_amm | 1,241 | dex_core |
| clawvault | 1,229 | Legacy |
| moltauction | 1,228 | Legacy |
| dex_governance | 1,207 | dex_core |
| reef_storage | 1,212 | Legacy |
| lobsterlend | 1,202 | Legacy |
| clawpay | 1,191 | Legacy |
| moltswap | 1,190 | Legacy |
| moltdao | 1,139 | Legacy |
| musd_token | 1,081 | musd_token |
| moltoracle | 1,080 | Legacy |
| dex_router | 1,063 | dex_core |
| dex_analytics | 1,002 | dex_core |
| dex_rewards | 896 | dex_core |
| bountyboard | 866 | Legacy |
| wsol_token | 782 | musd_token |
| weth_token | 782 | musd_token |
| moltmarket | 759 | Legacy |
| moltpunks | 453 | Legacy |
| moltcoin | 346 | Legacy |

**Total:** ~37,440 lines of Rust across 27 contracts.
