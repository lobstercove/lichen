# ABI Conformance Audit Report — 9 WASM Smart Contracts

**Date:** Audit conducted on full source of all 9 contracts  
**Scope:** Read-only analysis of `#[no_mangle] pub extern "C"` exported functions, parameter memory layout, return codes, `get_caller()` usage, variable-length data, and ABI compatibility  
**Runtime ABI Reference:**
- **0xAB JSON encoder:** base58 → 32-byte pubkey (stride 32); non-base58 string → UTF-8 padded to 32; number u8→stride 1, u16→stride 2, >u16→stride 4; bool→stride 1; array→padded to 32. I64 params: 8-byte LE.
- **Default (binary) mode:** every WASM I32 param = pointer advancing 32 bytes.
- **Soft-failure detection:** non-zero return + no storage changes = soft failure.

---

## Table of Contents

1. [dex\_margin](#1-dex_margin)
2. [dex\_rewards](#2-dex_rewards)
3. [dex\_router](#3-dex_router)
4. [lobsterlend](#4-lobsterlend)
5. [moltauction](#5-moltauction)
6. [moltbridge](#6-moltbridge)
7. [moltcoin](#7-moltcoin)
8. [moltdao](#8-moltdao)
9. [moltmarket](#9-moltmarket)
10. [Cross-Contract Summary of Issues](#10-cross-contract-summary)

---

## 1. dex\_margin

**File:** `contracts/dex_margin/src/lib.rs` (1680 lines)

### A. Exported Functions

| # | Export | Signature | Notes |
|---|--------|-----------|-------|
| 1 | `call()` | `fn call()` — no params | **Only** `#[no_mangle]` export. Gated by `#[cfg(target_arch = "wasm32")]`. |

All logic is dispatched internally through opcode matching on `args[0]`:

| Opcode | Internal Function | Parameters (from args byte array) |
|--------|-------------------|-----------------------------------|
| 0 | `initialize` | admin\[32\] |
| 1 | `set_mark_price` | caller\[32\], pair\_id:u64, price:u64 |
| 2 | `open_position` | trader\[32\], pair\_id:u64, side:u8, size:u64, leverage:u64, margin:u64 |
| 3 | `close_position` | caller\[32\], pos\_id:u64 |
| 4 | `add_margin` | caller\[32\], pos\_id:u64, amount:u64 |
| 5 | `remove_margin` | caller\[32\], pos\_id:u64, amount:u64 |
| 6 | `liquidate` | liquidator\[32\], pos\_id:u64 |
| 7 | `set_max_leverage` | caller\[32\], pair\_id:u64, max\_lev:u64 |
| 8 | `set_maintenance_margin` | caller\[32\], bps:u64 |
| 9 | `withdraw_insurance` | caller\[32\], amount:u64, recipient\[32\] |
| 10 | `set_moltcoin_address` | caller\[32\], addr\[32\] |
| 11 | `emergency_pause` | caller\[32\] |
| 12 | `emergency_unpause` | caller\[32\] |
| 20-24 | Query functions | Various |

### B. How Parameters Are Read

All parameters are read from the `args` byte vector returned by `moltchain_sdk::get_args()`:
- 32-byte addresses: `args[offset..offset+32].as_ptr()` → passed to internal function as `*const u8` → `copy_nonoverlapping(ptr, buf, 32)`
- u64 values: `bytes_to_u64(&args[offset..offset+8])` — 8-byte little-endian
- u8 values: `args[offset]` — single byte

### C. Return Codes

- **Convention:** 0 = success (NORMAL)
- All internal functions return u32; `call()` wraps result in `set_return_data(&u64_to_bytes(r as u64))`
- Specific error codes: 1 = already initialized, 2 = unauthorized, 200 = caller mismatch

### D. `get_caller()` Usage

**Yes** — all mutating internal functions verify `get_caller().0 == caller` (AUDIT-FIX pattern). Returns 200 on mismatch.

### E. Variable-Length Data

None.

### F. ABI Issues

| Severity | Issue |
|----------|-------|
| ⚠️ **STRUCTURAL** | Individual function names (e.g., `open_position`) are **NOT exported** as WASM symbols. External callers MUST use the opcode-based `call()` entry point with binary-encoded args. The JSON ABI encoder cannot target named functions — it must encode the opcode byte + params into a single flat byte array. |
| ℹ️ INFO | `call()` is `#[cfg(target_arch = "wasm32")]` — does not exist in native test builds. |

---

## 2. dex\_rewards

**File:** `contracts/dex_rewards/src/lib.rs` (1025 lines)

### A. Exported Functions

| # | Export | Signature | Target |
|---|--------|-----------|--------|
| 1 | `initialize` | `fn initialize(admin: *const u8) -> u32` | Always compiled |
| 2 | `call()` | `fn call()` — no params | `#[cfg(target_arch = "wasm32")]` only |

`call()` dispatcher opcodes:

| Opcode | Internal Function | Parameters |
|--------|-------------------|------------|
| 0 | `initialize` | admin\[32\] |
| 1 | `record_trade` | trader\[32\], fee\_paid:u64, volume:u64 |
| 2 | `claim_rewards` | trader\[32\] |
| 3 | `set_referral` | referrer\[32\], referee\[32\] |
| 4 | `claim_referral_rewards` | referrer\[32\] |
| 5 | `set_lp_reward_rate` | caller\[32\], rate:u64 |
| 6 | `distribute_epoch_rewards` | caller\[32\] |
| 7 | `set_tier_multiplier` | caller\[32\], tier:u64, multiplier:u64 |
| 10 | `emergency_pause` | caller\[32\] |
| 11 | `emergency_unpause` | caller\[32\] |
| 20-24 | Query functions | Various |

### B. How Parameters Are Read

- `initialize`: `copy_nonoverlapping(admin, addr, 32)` — standard 32-byte pointer read
- `call()` dispatcher: same byte-slicing pattern as dex\_margin

### C. Return Codes

- **Convention:** 0 = success (NORMAL)
- 1 = already initialized, 5 = unauthorized caller

### D. `get_caller()` Usage

`initialize`: NO explicit `get_caller()` check.  
`record_trade`: uses `is_authorized_caller()` for inter-contract auth.  
Admin functions in `call()`: uses `get_caller()` with AUDIT-FIX pattern.

### E. Variable-Length Data

None.

### F. ABI Issues

| Severity | Issue |
|----------|-------|
| ⚠️ **STRUCTURAL** | Same `call()` dispatcher pattern as dex\_margin — named functions not individually callable from WASM. |
| ⚠️ MODERATE | `initialize` is exported BOTH as a standalone `#[no_mangle]` function AND as opcode 0 in `call()`. Dual entry points — calling via the standalone function skips `call()` wrapping (no `set_return_data` wrapper). |

---

## 3. dex\_router

**File:** `contracts/dex_router/src/lib.rs` (1157 lines)

### A. Exported Functions

| # | Export | Signature | Target |
|---|--------|-----------|--------|
| 1 | `call()` | `fn call()` — no params | `#[cfg(target_arch = "wasm32")]` only |

`call()` dispatcher opcodes:

| Opcode | Internal Function | Parameters |
|--------|-------------------|------------|
| 0 | `initialize` | admin\[32\] |
| 1 | `set_addresses` | caller\[32\], core\[32\], amm\[32\], legacy\[32\] |
| 2 | `swap` | trader\[32\], token\_in\[32\], token\_out\[32\], amount\_in:u64, min\_out:u64, deadline:u64 |
| 3 | `multi_hop_swap` | trader\[32\], path\_ptr (variable), path\_count:u64, amount\_in:u64, min\_out:u64, deadline:u64 |
| 4 | `register_route` | caller\[32\], token\_in\[32\], token\_out\[32\], type:u8, pool\_id:u64, sec\_id:u64, split\_pct:u8 |
| 5 | `add_liquidity` | provider\[32\], pool\_id:u64, token\_a\[32\], token\_b\[32\], amount\_a:u64, amount\_b:u64 |
| 6 | `remove_liquidity` | provider\[32\], pool\_id:u64, lp\_amount:u64 |
| 20-22 | Query functions | Various |

### B. How Parameters Are Read

Same byte-slicing from `get_args()`. The `multi_hop_swap` function reads a variable-length path:
- `path_count`: u64 count of pool IDs
- Path data: `path_count × 8` bytes, each read as `bytes_to_u64()`

### C. Return Codes

- **Convention:** 0 = success (NORMAL)

### D. `get_caller()` Usage

Yes — admin functions check `get_caller()`.

### E. Variable-Length Data

**Yes** — `multi_hop_swap` reads a variable-length array of u64 pool IDs. The path is embedded inline in the args byte array, sized by `path_count`.

### F. ABI Issues

| Severity | Issue |
|----------|-------|
| ⚠️ **STRUCTURAL** | Same `call()` dispatcher pattern — no named WASM exports. |
| ⚠️ MODERATE | `multi_hop_swap` variable-length path array: the JSON encoder must correctly encode `path_count` u64 values contiguously in the args buffer. Path data is NOT a fixed-stride parameter — the total byte length depends on `path_count`. |

---

## 4. lobsterlend

**File:** `contracts/lobsterlend/src/lib.rs` (1436 lines)

### A. Exported Functions

| # | Function | WASM Signature | Rust Signature |
|---|----------|----------------|----------------|
| 1 | `initialize` | `(i32) -> i32` | `(admin_ptr: *const u8) -> u32` |
| 2 | `deposit` | `(i32, i64) -> i32` | `(user_ptr: *const u8, amount: u64) -> u32` |
| 3 | `withdraw` | `(i32, i64) -> i32` | `(user_ptr: *const u8, amount: u64) -> u32` |
| 4 | `borrow` | `(i32, i64) -> i32` | `(user_ptr: *const u8, amount: u64) -> u32` |
| 5 | `repay` | `(i32, i64) -> i32` | `(user_ptr: *const u8, amount: u64) -> u32` |
| 6 | `liquidate` | `(i32, i32, i64) -> i32` | `(liquidator_ptr: *const u8, borrower_ptr: *const u8, repay_amount: u64) -> u32` |
| 7 | `get_account_info` | `(i32, i32) -> i32` | `(user_ptr: *const u8, result_ptr: *mut u8) -> u32` |
| 8 | `get_protocol_stats` | `(i32) -> i32` | `(result_ptr: *mut u8) -> u32` |
| 9 | `flash_borrow` | `(i32, i64) -> i32` | `(borrower_ptr: *const u8, amount: u64) -> u32` |
| 10 | `flash_repay` | `(i32, i64) -> i32` | `(borrower_ptr: *const u8, amount: u64) -> u32` |
| 11 | `pause` | `(i32) -> i32` | `(caller_ptr: *const u8) -> u32` |
| 12 | `unpause` | `(i32) -> i32` | `(caller_ptr: *const u8) -> u32` |
| 13 | `set_deposit_cap` | `(i32, i64) -> i32` | `(caller_ptr: *const u8, cap: u64) -> u32` |
| 14 | `set_reserve_factor` | `(i32, i64) -> i32` | `(caller_ptr: *const u8, factor: u64) -> u32` |
| 15 | `withdraw_reserves` | `(i32, i64, i32) -> i32` | `(caller_ptr: *const u8, amount: u64, recipient_ptr: *const u8) -> u32` |
| 16 | `get_interest_rate` | `() -> i64` | `() -> u64` |
| 17 | `get_deposit_count` | `() -> i64` | `() -> u64` |
| 18 | `get_borrow_count` | `() -> i64` | `() -> u64` |
| 19 | `get_liquidation_count` | `() -> i64` | `() -> u64` |
| 20 | `get_platform_stats` | `(i32) -> i32` | `(result_ptr: *mut u8) -> u32` |

### B. How Parameters Are Read

- All `*const u8` params: `copy_nonoverlapping(ptr, buf, 32)` — reads 32 bytes at pointer
- `u64` params: passed directly as WASM I64 values
- `liquidate` reads TWO 32-byte pointers (liquidator + borrower) sequentially

Under default ABI (every I32 = pointer advancing 32 bytes):
- `deposit(user_ptr, amount)`: I32 at offset 0 → 32-byte read, I64 as second param ✅
- `liquidate(liquidator, borrower, amount)`: I32 at offset 0, I32 at offset 32, I64 ✅
- `withdraw_reserves(caller, amount, recipient)`: I32 at offset 0, I64, I32 at offset 32 ✅

### C. Return Codes

- **Convention:** 0 = success (NORMAL ✅)
- Error codes: 1-5 = specific errors, 20 = paused, 21 = reentrancy, 200 = caller mismatch

### D. `get_caller()` Usage

**Yes** — all mutating functions verify `get_caller().0 == caller` with AUDIT-FIX pattern. Returns 200 on mismatch.

### E. Variable-Length Data

None.

### F. ABI Issues

| Severity | Issue |
|----------|-------|
| ⚠️ MODERATE | `get_account_info(user_ptr, result_ptr)` and `get_protocol_stats(result_ptr)` and `get_platform_stats(result_ptr)` take **output pointers** (`*mut u8`). The JSON ABI encoder produces input buffers only — it cannot allocate writable output memory for the contract to write into. These query functions require the caller to pre-allocate an output buffer and pass its address. |
| ✅ GOOD | All mutating functions follow 0=success convention. |
| ✅ GOOD | All mutating functions check `get_caller()`. |
| ✅ GOOD | Named functions are individually exported — standard ABI-compatible. |

---

## 5. moltauction

**File:** `contracts/moltauction/src/lib.rs` (1315 lines)

### A. Exported Functions

| # | Function | Rust Signature | Return |
|---|----------|----------------|--------|
| 1 | `initialize` | `(admin_ptr: *const u8) -> u32` | **1** always |
| 2 | `create_auction` | `(seller_ptr, nft_contract_ptr, token_id:u64, min_bid:u64, payment_token_ptr, duration:u64) -> u32` | **1**=success, **0**=failure |
| 3 | `place_bid` | `(bidder_ptr, auction_id:u64, bid_amount:u64) -> u32` | **1**=success, **0**=failure |
| 4 | `finalize_auction` | `(caller_ptr, auction_id:u64) -> u32` | **1**=sold, **2**=reserve not met, **0**=failure |
| 5 | `make_offer` | `(offerer_ptr, nft_contract_ptr, token_id:u64, offer_amount:u64, payment_token_ptr, duration:u64) -> u32` | **1**=success |
| 6 | `accept_offer` | `(seller_ptr, nft_contract_ptr, token_id:u64, offerer_ptr) -> u32` | **1**=success |
| 7 | `set_royalty` | `(admin_ptr, collection_ptr, royalty_bps:u64, royalty_recipient_ptr) -> u32` | **1**=success, **0**=failure |
| 8 | `update_collection_stats` | `(admin_ptr, collection_ptr, floor_price:u64, total_volume:u64, listing_count:u64) -> u32` | **1**=success |
| 9 | `get_collection_stats` | `(collection_ptr, result_ptr: *mut u8) -> u32` | **1**=found, **0**=not found |
| 10 | `set_reserve_price` | `(caller_ptr, auction_id:u64, reserve_price:u64) -> u32` | **0**=success |
| 11 | `cancel_auction` | `(caller_ptr, auction_id:u64) -> u32` | **0**=success |
| 12 | `initialize_ma_admin` | `(admin_ptr) -> u32` | **0**=success |
| 13 | `ma_pause` | `(caller_ptr) -> u32` | **0**=success |
| 14 | `ma_unpause` | `(caller_ptr) -> u32` | **0**=success |
| 15 | `get_auction_info` | `(auction_id:u64) -> u32` | **0**=success (uses set\_return\_data) |
| 16 | `get_auction_stats` | `() -> u32` | **0**=success (uses set\_return\_data) |

### B. How Parameters Are Read

- `*const u8` pointers: `copy_nonoverlapping(ptr, buf, 32)` — 32-byte reads
- `u64` values: passed directly as WASM I64
- `create_auction`: 3 pointers (seller, nft\_contract, payment\_token) + 3 u64s (token\_id, min\_bid, duration)
- `make_offer`: 3 pointers + 3 u64s
- `accept_offer`: 3 pointers + 1 u64

### C. Return Codes

**🔴 CRITICAL: MIXED convention within the same contract!**

| Functions | Convention |
|-----------|-----------|
| `create_auction`, `place_bid`, `finalize_auction`, `make_offer`, `accept_offer`, `set_royalty`, `update_collection_stats`, `get_collection_stats`, `initialize` | **1 = success** (INVERTED) |
| `set_reserve_price`, `cancel_auction`, `initialize_ma_admin`, `ma_pause`, `ma_unpause`, `get_auction_info`, `get_auction_stats` | **0 = success** (NORMAL) |

### D. `get_caller()` Usage

Yes — used in admin functions (`set_reserve_price`, `cancel_auction`, `ma_pause`, `ma_unpause`). Also used in `place_bid`, `accept_offer` with AUDIT-FIX pattern.

### E. Variable-Length Data

None.

### F. ABI Issues

| Severity | Issue |
|----------|-------|
| 🔴 **CRITICAL** | **Mixed return code conventions.** Functions returning 1 on success (e.g., `create_auction`) will be misinterpreted by any tooling/SDK expecting 0=success. |
| 🔴 **CRITICAL** | `get_collection_stats(collection_ptr, result_ptr: *mut u8)` takes an **output pointer**. The JSON encoder cannot produce output buffer addresses. |
| ⚠️ MODERATE | Two different initialization functions (`initialize` returns 1, `initialize_ma_admin` returns 0) with different conventions — confusing for integrators. |

---

## 6. moltbridge

**File:** `contracts/moltbridge/src/lib.rs` (2079 lines)

### A. Exported Functions

| # | Function | Rust Signature | Return Convention |
|---|----------|----------------|-------------------|
| 1 | `initialize` | `(owner_ptr: *const u8) -> u32` | 0=success |
| 2 | `add_bridge_validator` | `(caller_ptr, validator_ptr) -> u32` | 0=success |
| 3 | `remove_bridge_validator` | `(caller_ptr, validator_ptr) -> u32` | 0=success |
| 4 | `set_required_confirmations` | `(caller_ptr, confirmations: u64) -> u32` | 0=success |
| 5 | `set_request_timeout` | `(caller_ptr, timeout: u64) -> u32` | 0=success |
| 6 | `lock_tokens` | `(sender_ptr, amount:u64, dest_chain_ptr, dest_address_ptr) -> u32` | 0=success |
| 7 | `submit_mint` | `(caller_ptr, recipient_ptr, amount:u64, source_chain_ptr, source_tx_ptr) -> u32` | 0=success |
| 8 | `confirm_mint` | `(caller_ptr, nonce:u64) -> u32` | 0=success |
| 9 | `submit_unlock` | `(caller_ptr, recipient_ptr, amount:u64, burn_proof_ptr) -> u32` | 0=success |
| 10 | `confirm_unlock` | `(caller_ptr, nonce:u64) -> u32` | 0=success |
| 11 | `cancel_expired_request` | `(nonce: u64) -> u32` | 0=success |
| 12 | `get_bridge_status` | `(nonce: u64) -> u32` | 0=found, 1=not found |
| 13 | `has_confirmed_mint` | `(validator_ptr, nonce:u64) -> u32` | 0 (result via set\_return\_data) |
| 14 | `has_confirmed_unlock` | `(validator_ptr, nonce:u64) -> u32` | 0 (result via set\_return\_data) |
| 15 | `is_source_tx_used` | `(source_tx_ptr) -> u32` | 0 (result via set\_return\_data) |
| 16 | `is_burn_proof_used` | `(burn_proof_ptr) -> u32` | 0 (result via set\_return\_data) |
| 17 | `set_moltyid_address` | `(caller_ptr, moltyid_addr_ptr) -> u32` | 0=success |
| 18 | `set_identity_gate` | `(caller_ptr, min_reputation:u64) -> u32` | 0=success |
| 19 | `mb_pause` | `(caller_ptr) -> u32` | 0=success |
| 20 | `mb_unpause` | `(caller_ptr) -> u32` | 0=success |

### B. How Parameters Are Read

- `*const u8` pointers: `copy_nonoverlapping(ptr, buf, 32)` — 32-byte reads
- `u64` values: passed directly as WASM I64
- `lock_tokens`: 3 pointers (sender, dest\_chain, dest\_address) + 1 u64 (amount). WASM sig: `(i32, i64, i32, i32) -> i32`
- `submit_mint`: 4 pointers + 1 u64. WASM sig: `(i32, i32, i64, i32, i32) -> i32`
- `cancel_expired_request`: just 1 u64, no pointers. WASM sig: `(i64) -> i32`

### C. Return Codes

- **Convention:** 0 = success (NORMAL ✅)
- Error codes: 1=already init, 2=unauthorized, 3=invalid param, 4=duplicate, 5=zero address, 6=zero recipient, 7=expired, 8=already confirmed, 10=identity gate failed, 200=caller mismatch

### D. `get_caller()` Usage

**Yes** — all mutating functions (except `cancel_expired_request`) verify `get_caller()` with AUDIT-FIX pattern.

### E. Variable-Length Data

None.

### F. ABI Issues

| Severity | Issue |
|----------|-------|
| ⚠️ MODERATE | `cancel_expired_request(nonce: u64)` takes **only a u64 param** with no pointer. Under default ABI mode (every I32 = pointer advancing 32 bytes), this works correctly since u64 maps to WASM I64, not I32. However, it's the only function in this contract with no caller pointer — **no `get_caller()` auth check**. Anyone can cancel expired requests. This is by design (expired requests should be cancellable by anyone), but worth noting. |
| ⚠️ MODERATE | `lock_tokens` param order is `(sender_ptr, amount:u64, dest_chain_ptr, dest_address_ptr)` — the u64 is sandwiched between pointers. Under default ABI, I32 params advance the pointer by 32. The WASM signature is `(i32, i64, i32, i32)`. If the JSON encoder sees params as \[base58, number, base58, base58\], it should encode: 32 bytes, 8 bytes LE, 32 bytes, 32 bytes. The descriptor must correctly specify strides \[32, 8, 32, 32\] — if it defaults to stride-32 for the u64, the offset math will be wrong. |
| ✅ GOOD | Consistent 0=success convention throughout. |
| ✅ GOOD | Named functions individually exported. |

---

## 7. moltcoin

**File:** `contracts/moltcoin/src/lib.rs` (~250 lines of code)

### A. Exported Functions

| # | Function | Rust Signature | Return Type | Return Convention |
|---|----------|----------------|-------------|-------------------|
| 1 | `initialize` | `(owner_ptr: *const u8)` | **void** | ⚠️ No return |
| 2 | `balance_of` | `(account_ptr: *const u8) -> u64` | **u64** | Balance value |
| 3 | `transfer` | `(from_ptr, to_ptr, amount: u64) -> u32` | u32 | **1**=success, **0**=failure |
| 4 | `mint` | `(caller_ptr, to_ptr, amount: u64) -> u32` | u32 | **1**=success, **0**=failure |
| 5 | `burn` | `(from_ptr, amount: u64) -> u32` | u32 | **1**=success, **0**=failure |
| 6 | `approve` | `(owner_ptr, spender_ptr, amount: u64) -> u32` | u32 | **1**=success, **0**=failure |
| 7 | `total_supply` | `() -> u64` | **u64** | Supply value |

### B. How Parameters Are Read

- `*const u8` pointers: `copy_nonoverlapping(ptr, buf, 32)` — 32-byte reads
- `u64 amount`: passed directly as WASM I64
- `transfer(from, to, amount)`: 2 pointers + 1 u64. WASM sig: `(i32, i32, i64) -> i32`

### C. Return Codes

**🔴 CRITICAL: INVERTED + VOID**

| Function | Returns |
|----------|---------|
| `initialize` | **void** — no success/failure indication |
| `transfer` | **1** = success, **0** = failure |
| `mint` | **1** = success, **0** = failure |
| `burn` | **1** = success, **0** = failure |
| `approve` | **1** = success, **0** = failure |
| `balance_of` | u64 balance (different type) |
| `total_supply` | u64 supply (different type) |

### D. `get_caller()` Usage

**Yes** — `transfer`, `mint`, `burn`, `approve` verify `get_caller()`. However, `mint` returns **0** on caller mismatch (not 200 like most other contracts).

### E. Variable-Length Data

None.

### F. ABI Issues

| Severity | Issue |
|----------|-------|
| 🔴 **CRITICAL** | `initialize` returns **void** (`-> ()`). The runtime cannot determine success/failure. If external tooling checks the return value, it will get undefined/0. |
| 🔴 **CRITICAL** | All mutating functions return **1 on success** (INVERTED). SDKs and CLI tools expecting 0=success will misinterpret every successful call as an error. |
| ⚠️ MODERATE | `balance_of` and `total_supply` return **u64** (WASM I64), not u32 (I32). These have different WASM return types from the u32-returning functions. Callers must handle both return types. |
| ⚠️ MODERATE | `mint` returns 0 on caller mismatch instead of 200, unlike most other contracts. Inconsistent error signaling. |

---

## 8. moltdao

**File:** `contracts/moltdao/src/lib.rs` (1381 lines)

### A. Exported Functions

| # | Function | Rust Signature | Return Convention |
|---|----------|----------------|-------------------|
| 1 | `initialize_dao` | `(governance_token_ptr, treasury_address_ptr, min_proposal_threshold:u64) -> u32` | **1**=success (INVERTED) |
| 2 | `create_proposal` | `(proposer_ptr, title_ptr, title_len:u32, description_ptr, description_len:u32, target_contract_ptr, action_ptr, action_len:u32) -> u32` | proposal\_id (non-zero on success) |
| 3 | `create_proposal_typed` | same as above + `proposal_type:u8` | proposal\_id |
| 4 | `vote` | `(voter_ptr, proposal_id:u64, support:u8, _voting_power:u64) -> u32` | **1**=success (INVERTED) |
| 5 | `vote_with_reputation` | `(voter_ptr, proposal_id:u64, support:u8, _token_balance:u64, reputation:u64) -> u32` | **1**=success |
| 6 | `execute_proposal` | `(executor_ptr, proposal_id:u64, action_ptr, action_len:u32) -> u32` | **1**=executed (INVERTED) |
| 7 | `veto_proposal` | `(voter_ptr, proposal_id:u64, _token_balance:u64, _reputation:u64) -> u32` | **1**=success |
| 8 | `cancel_proposal` | `(canceller_ptr, proposal_id:u64) -> u32` | **1**=cancelled |
| 9 | `treasury_transfer` | `(proposal_id:u64, token_ptr, recipient_ptr, amount:u64) -> u32` | **1**=success |
| 10 | `get_treasury_balance` | `(token_ptr, result_ptr: *mut u8) -> u32` | **1** always |
| 11 | `get_proposal` | `(proposal_id:u64, result_ptr: *mut u8) -> u32` | **1**=found, **0**=not found |
| 12 | `get_dao_stats` | `(result_ptr: *mut u8) -> u32` | **1** always |
| 13 | `get_active_proposals` | `(result_ptr: *mut u8, max_results:u32) -> u32` | active\_count (variable) |
| 14 | `initialize` | alias for `initialize_dao` | same |
| 15 | `cast_vote` | alias for `vote` | same |
| 16 | `finalize_proposal` | `(caller_ptr, proposal_id:u64) -> u32` | **⚠️ calls execute\_proposal with 2 of 4 args** |
| 17 | `get_proposal_count` | `() -> u64` | proposal count |
| 18 | `get_vote` | `(proposal_id:u64, voter_ptr) -> u32` | 1=voted, 0=not voted |
| 19 | `get_vote_count` | `(proposal_id:u64) -> u64` | total votes |
| 20 | `get_total_supply` | `() -> u64` | supply |
| 21 | `set_quorum` | `(caller_ptr, quorum:u64) -> u32` | **0**=success (NORMAL) |
| 22 | `set_voting_period` | `(caller_ptr, period:u64) -> u32` | **0**=success (NORMAL) |
| 23 | `set_timelock_delay` | `(caller_ptr, delay:u64) -> u32` | **0**=success (NORMAL) |
| 24 | `dao_pause` | `(caller_ptr) -> u32` | **0**=success (NORMAL) |
| 25 | `dao_unpause` | `(caller_ptr) -> u32` | **0**=success (NORMAL) |

### B. How Parameters Are Read

- `*const u8` pointers: `copy_nonoverlapping(ptr, buf, 32)` — 32-byte reads
- `u64` values: passed directly as WASM I64
- `u32` values (title\_len, description\_len, action\_len): passed as WASM I32
- `u8` values (support, proposal\_type): passed as WASM I32 (widened)
- **Variable-length reads:** `create_proposal` reads `title_len` bytes from `title_ptr`, `description_len` bytes from `description_ptr`, `action_len` bytes from `action_ptr`

### C. Return Codes

**🔴 CRITICAL: MIXED convention**

| Functions | Convention |
|-----------|-----------|
| Core DAO functions (`initialize_dao`, `create_proposal`, `vote`, `execute_proposal`, `veto_proposal`, `cancel_proposal`, `treasury_transfer`) | **1 = success** or **non-zero = success** (INVERTED) |
| Alias admin functions (`set_quorum`, `set_voting_period`, `set_timelock_delay`, `dao_pause`, `dao_unpause`) | **0 = success** (NORMAL) |
| `create_proposal` | Returns **proposal\_id** (1, 2, 3...) — any non-zero value on success |
| `get_active_proposals` | Returns **count** of active proposals — variable |

### D. `get_caller()` Usage

- `cancel_proposal`: Yes — verifies `get_caller()`, but returns **0** on mismatch (not 200).
- `set_quorum`, `set_voting_period`, `set_timelock_delay`, `dao_pause`, `dao_unpause`: **NO `get_caller()` check** — only checks stored `dao_owner` against the caller pointer param (spoofable).
- `treasury_transfer`: **NO `get_caller()` check** — relies on proposal execution state.
- `vote`, `vote_with_reputation`: **NO `get_caller()` check** on the voter identity.

### E. Variable-Length Data

**🔴 CRITICAL — YES**

`create_proposal` and `create_proposal_typed` take three variable-length byte arrays:

```
(proposer_ptr, title_ptr, title_len:u32, description_ptr, description_len:u32,
 target_contract_ptr, action_ptr, action_len:u32)
```

- `title_ptr` + `title_len`: reads `title_len` bytes from `title_ptr`
- `description_ptr` + `description_len`: reads `description_len` bytes from `description_ptr`
- `action_ptr` + `action_len`: reads `action_len` bytes from `action_ptr`

WASM signature: `(i32, i32, i32, i32, i32, i32, i32, i32) -> i32`

Under the default ABI (every I32 = pointer advancing 32 bytes), the encoder would allocate 32 bytes for `title_len` — **WRONG**, it's a u32 integer length, not a pointer.

The JSON encoder with 0xAB descriptor would encode `title_len` as a small number (stride 1-4), but the contract reads `title_len` bytes starting from `title_ptr`. The actual title data must be at the address `title_ptr` points to.

**This is fundamentally incompatible with stride-based encoding.** The encoder would need to:
1. Place the title bytes at a known memory address
2. Pass that address as `title_ptr` (I32)
3. Pass the length as `title_len` (I32)

The stride-based encoder treats each I32 param independently and does not support "this param points to data whose length is specified by the next param."

`execute_proposal` similarly takes `(action_ptr, action_len:u32)` for variable-length action data.

### F. ABI Issues

| Severity | Issue |
|----------|-------|
| 🔴 **CRITICAL** | **`finalize_proposal` calls `execute_proposal(caller_ptr, proposal_id)` with only 2 of 4 required arguments.** `execute_proposal` requires `(executor_ptr, proposal_id, action_ptr, action_len)`. This is a **Rust compile error** — the code cannot compile as written. Either this is dead code or there is a missing overload. |
| 🔴 **CRITICAL** | **Variable-length (ptr, len) parameter pairs** in `create_proposal` / `create_proposal_typed` / `execute_proposal`. The JSON ABI encoder cannot encode pointer-to-variable-length-data parameters. The stride model is fundamentally incompatible with (ptr, len) semantics. |
| 🔴 **CRITICAL** | **Mixed return code conventions.** Core DAO functions return 1=success while admin aliases return 0=success. `create_proposal` returns the proposal ID (could be any positive integer). |
| 🔴 **CRITICAL** | `get_treasury_balance`, `get_proposal`, `get_dao_stats`, `get_active_proposals` all take **output pointers** (`*mut u8`). Not encodable by JSON encoder. |
| ⚠️ **HIGH** | **Missing `get_caller()` checks** on `set_quorum`, `set_voting_period`, `set_timelock_delay`, `dao_pause`, `dao_unpause`. These only compare `caller_ptr` content against stored `dao_owner`, but don't verify the ptr content matches the actual transaction signer. A malicious caller could pass the owner's address as `caller_ptr` to bypass auth. |
| ⚠️ **HIGH** | **Missing `get_caller()` on `vote` and `vote_with_reputation`**. Anyone can submit votes on behalf of other addresses by passing their pubkey as `voter_ptr`. |
| ⚠️ MODERATE | `treasury_transfer` first param is `proposal_id:u64` (WASM I64), not a pointer. The function has no caller auth, relying entirely on proposal execution state. |

---

## 9. moltmarket

**File:** `contracts/moltmarket/src/lib.rs` (944 lines)

### A. Exported Functions

| # | Function | Rust Signature | Return Convention |
|---|----------|----------------|-------------------|
| 1 | `initialize` | `(owner_ptr, fee_addr_ptr)` | **void** ⚠️ |
| 2 | `list_nft` | `(seller_ptr, nft_contract_ptr, token_id:u64, price:u64, payment_token_ptr) -> u32` | **1**=success, **0**=failure, **200**=caller mismatch |
| 3 | `buy_nft` | `(buyer_ptr, nft_contract_ptr, token_id:u64) -> u32` | **1**=success, **0**=failure, **200**=caller mismatch |
| 4 | `cancel_listing` | `(seller_ptr, nft_contract_ptr, token_id:u64) -> u32` | **1**=success, **0**=failure |
| 5 | `get_listing` | `(nft_contract_ptr, token_id:u64, out_ptr: *mut u8) -> u32` | **1**=found, **0**=not found |
| 6 | `set_marketplace_fee` | `(caller_ptr, new_fee:u64) -> u32` | **1**=success, **0**=failure |
| 7 | `list_nft_with_royalty` | `(seller_ptr, nft_contract_ptr, token_id:u64, price:u64, payment_token_ptr, royalty_recipient_ptr) -> u32` | **1**=success |
| 8 | `make_offer` | `(offerer_ptr, nft_contract_ptr, token_id:u64, price:u64, payment_token_ptr) -> u32` | **1**=success |
| 9 | `cancel_offer` | `(offerer_ptr, nft_contract_ptr, token_id:u64) -> u32` | **1**=success, **0**=failure |
| 10 | `accept_offer` | `(seller_ptr, nft_contract_ptr, token_id:u64, offerer_ptr) -> u32` | **1**=success, **200**=caller mismatch |
| 11 | `get_marketplace_stats` | `() -> u32` | **0** (uses set\_return\_data) |
| 12 | `mm_pause` | `(caller_ptr) -> u32` | **0**=success, **200**=caller mismatch |
| 13 | `mm_unpause` | `(caller_ptr) -> u32` | **0**=success, **200**=caller mismatch |

### B. How Parameters Are Read

- `*const u8` pointers: `copy_nonoverlapping(ptr, buf, 32)` or `parse_address(ptr)` — 32-byte reads
- `u64` values: passed directly as WASM I64
- `list_nft_with_royalty`: 4 pointers + 2 u64s

### C. Return Codes

**🔴 CRITICAL: MIXED + VOID**

| Functions | Convention |
|-----------|-----------|
| `initialize` | **void** — no return |
| `list_nft`, `buy_nft`, `cancel_listing`, `set_marketplace_fee`, `list_nft_with_royalty`, `make_offer`, `cancel_offer`, `accept_offer`, `get_listing` | **1 = success** (INVERTED) |
| `get_marketplace_stats`, `mm_pause`, `mm_unpause` | **0 = success** (NORMAL) |

### D. `get_caller()` Usage

- `list_nft`, `buy_nft`, `accept_offer`, `mm_pause`, `mm_unpause`: **Yes** — AUDIT-FIX pattern, returns 200.
- `cancel_listing`: **NO `get_caller()` check** — only compares seller\_ptr bytes with stored listing seller. Spoofable.
- `cancel_offer`: **NO `get_caller()` check** — only compares offerer\_ptr with stored offer. Spoofable.
- `set_marketplace_fee`: **NO `get_caller()` check** — only compares caller\_ptr with stored owner. Spoofable.
- `make_offer`: **NO `get_caller()` check**.

### E. Variable-Length Data

None.

### F. ABI Issues

| Severity | Issue |
|----------|-------|
| 🔴 **CRITICAL** | `initialize` returns **void** — no success/failure indication. |
| 🔴 **CRITICAL** | **Inverted return codes** (1=success) for all marketplace operations. SDKs expecting 0=success will misinterpret all successful operations. |
| 🔴 **CRITICAL** | `get_listing(nft_contract_ptr, token_id, out_ptr: *mut u8)` takes an **output pointer**. Not encodable by JSON encoder. |
| ⚠️ **HIGH** | **Missing `get_caller()` checks** on `cancel_listing`, `cancel_offer`, `set_marketplace_fee`, `make_offer`. These can be spoofed by passing the expected address as the caller pointer without being the actual transaction signer. |
| ⚠️ MODERATE | `get_marketplace_stats` returns 0=success while all other mutating functions return 1=success — inconsistent within the contract. |

---

## 10. Cross-Contract Summary

### Issue Frequency Matrix

| Issue | dex\_margin | dex\_rewards | dex\_router | lobsterlend | moltauction | moltbridge | moltcoin | moltdao | moltmarket |
|-------|:-----------:|:------------:|:-----------:|:-----------:|:-----------:|:----------:|:--------:|:-------:|:----------:|
| `call()` dispatcher (no named exports) | 🔴 | 🔴 | 🔴 | — | — | — | — | — | — |
| Inverted return codes (1=success) | — | — | — | — | 🔴 | — | 🔴 | 🔴 | 🔴 |
| Mixed return conventions (same contract) | — | — | — | — | 🔴 | — | — | 🔴 | 🔴 |
| `void` return (initialize) | — | — | — | — | — | — | 🔴 | — | 🔴 |
| Output pointers (`*mut u8`) | — | — | — | ⚠️ | ⚠️ | — | — | 🔴 | ⚠️ |
| Variable-length (ptr, len) params | — | — | ⚠️ | — | — | — | — | 🔴 | — |
| Missing `get_caller()` on auth functions | — | — | — | — | — | — | — | 🔴 | 🔴 |
| Compile error (wrong arg count) | — | — | — | — | — | — | — | 🔴 | — |
| Sandwiched u64 between pointers | — | — | — | — | — | ⚠️ | — | — | — |

### Critical Issues Ranked by Impact

1. **Inverted Return Codes (4 contracts: moltcoin, moltmarket, moltauction, moltdao):** Return 1 on success instead of 0. Any SDK, CLI tool, or cross-contract caller checking `result == 0` for success will misidentify every successful call as a failure.

2. **`call()` Dispatcher Pattern (3 contracts: dex\_margin, dex\_rewards, dex\_router):** Do NOT export individual function names. They only export a single `call()` entry point with opcode-based dispatch. The JSON ABI encoder cannot target named functions — it must construct opcode + binary args as a flat byte buffer.

3. **Variable-Length Parameters (2 contracts: moltdao, dex\_router):** Accept variable-length data via (pointer, length) pairs. The stride-based ABI encoder cannot represent (ptr, len) semantics where data must be placed at the pointer's target address.

4. **Output Pointers (4 contracts: lobsterlend, moltauction, moltdao, moltmarket):** Functions taking `*mut u8` output pointers. The JSON encoder is input-only — it cannot allocate writable output buffers.

5. **`void` Return (2 contracts: moltcoin, moltmarket):** `initialize` functions return void. No way for the caller to know if initialization succeeded.

6. **Compile Error (1 contract: moltdao):** `finalize_proposal` calls `execute_proposal(caller_ptr, proposal_id)` passing 2 arguments to a 4-parameter function. This is a Rust type error and will not compile.

7. **Missing `get_caller()` Auth (2 contracts: moltdao, moltmarket):** Multiple admin/user functions don't verify the transaction signer via `get_caller()`, making them vulnerable to address spoofing through crafted `caller_ptr` values.

### Contracts by ABI Safety

| Rating | Contract | Notes |
|--------|----------|-------|
| ✅ Safe | **lobsterlend** | Standard named exports, 0=success, get\_caller() everywhere. Only issue: output pointers on query functions. |
| ✅ Safe | **moltbridge** | Standard named exports, 0=success, get\_caller() everywhere. Minor: sandwiched u64, cancel\_expired has no auth (by design). |
| ⚠️ Non-standard | **dex\_margin** | Opcode-based call() dispatcher. Internally sound but requires custom encoding. |
| ⚠️ Non-standard | **dex\_rewards** | Same call() pattern + duplicate initialize export. |
| ⚠️ Non-standard | **dex\_router** | Same call() pattern + variable-length path in multi\_hop\_swap. |
| 🔴 Broken | **moltcoin** | Inverted returns, void initialize. |
| 🔴 Broken | **moltmarket** | Inverted returns, void initialize, missing get\_caller(). |
| 🔴 Broken | **moltauction** | Mixed return conventions within same contract. |
| 🔴 Broken | **moltdao** | Variable-length params, mixed returns, compile error, missing get\_caller(), output pointers. Most issues of any contract. |
