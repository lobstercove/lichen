# MoltChain Smart Contract Production-Readiness Audit Report

**Date:** 2026-02-25  
**Scope:** All 29 smart contracts in `contracts/*/src/lib.rs`  
**Auditor:** Automated deep analysis  

---

## Executive Summary

All 29 contracts were reviewed. Each contract has a `#[cfg(test)] mod tests` module with inline tests. The codebase shows evidence of multiple prior audit passes ("AUDIT-FIX", "SECURITY-FIX") and uses consistent patterns: reentrancy guards, admin access control, caller verification via `get_caller()`, pause mechanisms, and `u128` intermediate arithmetic. **No TODO/FIXME comments** were found anywhere.

**Findings:** 14 actionable findings across 11 contracts.

| Severity | Count |
|----------|-------|
| Critical | 1     |
| High     | 3     |
| Medium   | 5     |
| Low      | 5     |

---

## Findings

### Finding 1 — CRITICAL: Placeholder Challenge Verification in reef_storage

**Contract:** reef_storage  
**File:** `contracts/reef_storage/src/lib.rs`  
**Lines:** 918–967  
**Category:** Incomplete Logic / Stub  

The `respond_challenge` function accepts **any non-zero 32-byte response** as a valid proof-of-storage response. Real Merkle proof verification is not implemented.

```rust
// Verify response is non-zero (placeholder; real impl would check merkle proof)
if response.iter().all(|&b| b == 0) {
    log_info("Invalid response (all zeros)");
    return 4;
}
```

**Impact:** A storage provider can submit any 32 random bytes to pass a challenge, rendering the proof-of-storage system non-functional. Providers are never actually proven to store data, so slashing has no teeth and the storage guarantee is void.

**Severity:** **Critical** — the core security mechanism of the storage protocol is bypassed.

---

### Finding 2 — HIGH: Graceful Degradation Silently Skips Token Transfers (prediction_market)

**Contract:** prediction_market  
**File:** `contracts/prediction_market/src/lib.rs`  
**Lines:** 224–234  
**Category:** Security Issue / Silent Failure  

```rust
fn transfer_musd_out(recipient: &[u8], amount: u64) -> bool {
    let musd_addr = load_addr(MUSD_ADDR_KEY);
    if is_zero(&musd_addr) {
        log_info("mUSD address not configured — skipping transfer");
        return true; // graceful degradation for unconfigured deployments
    }
    let self_addr = load_self_addr();
    if is_zero(&self_addr) {
        log_info("Self address not configured — skipping transfer");
        return true; // graceful degradation for unconfigured deployments
    }
    ...
}
```

**Impact:** If the mUSD token address or the contract's self-address is not configured, all market settlement payouts (winning bets, refunds on voided markets) silently report success without transferring any funds. Users' balances are debited in contract state but no tokens move.

**Severity:** **High** — direct loss of user funds if deployed without proper address configuration.

---

### Finding 3 — HIGH: Graceful Degradation Silently Skips Token Transfers (clawvault)

**Contract:** clawvault  
**File:** `contracts/clawvault/src/lib.rs`  
**Lines:** 457–470  
**Category:** Security Issue / Silent Failure  

```rust
fn transfer_molt_out(to: &[u8; 32], amount: u64) -> bool {
    if amount == 0 {
        return true;
    }
    let token_data = storage_get(MOLT_TOKEN_KEY);
    if token_data.is_none() || token_data.as_ref().unwrap().len() < 32 {
        return true; // graceful degradation: token not configured yet
    }
    ...
}
```

**Impact:** Vault withdrawals succeed in bookkeeping (reducing user shares/deposits) without actually transferring MOLT tokens if the token address is not configured. Users lose their deposited value.

**Severity:** **High** — direct loss of user funds in unconfigured deployments.

---

### Finding 4 — HIGH: Graceful Degradation Silently Skips Token Transfers (reef_storage)

**Contract:** reef_storage  
**File:** `contracts/reef_storage/src/lib.rs`  
**Lines:** 74–84  
**Category:** Security Issue / Silent Failure  

```rust
fn transfer_molt_out(to: &[u8; 32], amount: u64) -> bool {
    ...
    let token_data = storage_get(MOLT_TOKEN_KEY);
    if token_data.is_none() || token_data.as_ref().unwrap().len() < 32 {
        return true; // graceful degradation: token not configured yet
    }
    ...
}
```

**Impact:** Provider reward claims and deposit withdrawals silently succeed without moving tokens when MOLT address is unconfigured. Same pattern as clawvault.

**Severity:** **High** — direct loss of user funds in unconfigured deployments.

**Note:** `clawpump` had the same pattern but was already fixed:
```rust
// AUDIT-FIX CON-05: MUST fail when MOLT token address is not configured.
log_info("CRITICAL: MOLT token address not configured — transfer REJECTED");
return false;
```
The fix applied to clawpump (line 199) should be replicated in **clawvault** and **reef_storage**.

---

### Finding 5 — MEDIUM: Hardcoded Placeholder Address `[0x4D; 32]` in moltmarket

**Contract:** moltmarket  
**File:** `contracts/moltmarket/src/lib.rs`  
**Lines:** 290–291, 1086–1087, 1172–1173, 1424–1425  
**Category:** Hardcoded Data / Placeholder  

```rust
let fee_addr_bytes = storage_get(b"marketplace_fee_addr")
    .unwrap_or_else(|| alloc::vec![0x4Du8; 32]); // fallback
let marketplace_addr = Address(fee_addr_bytes.as_slice().try_into().unwrap_or([0x4D; 32]));
```

**Impact:** If `marketplace_fee_addr` is not configured, marketplace fees and escrow payments are sent to `Address([0x4D; 32])` — 32 bytes of the ASCII character 'M'. This is a deterministic but effectively uncontrolled address. Any funds sent there are likely unrecoverable. This fallback appears in **4 separate locations** (purchase, accept_offer, and two other escrow paths).

**Severity:** **Medium** — fees are lost rather than failing safely. The primary purchase flow still works (NFT + payment transfer), but the 2.5% fee goes to a dead address.

---

### Finding 6 — MEDIUM: Hardcoded Placeholder Address `[0x4D; 32]` in moltauction

**Contract:** moltauction  
**File:** `contracts/moltauction/src/lib.rs`  
**Line:** 92  
**Category:** Hardcoded Data / Placeholder  

```rust
Address([0x4D; 32]) // 'M' repeated — identifiable placeholder
```

**Impact:** Same pattern as moltmarket. If the marketplace address is not stored, the fallback is an uncontrolled address. Funds routed there are lost.

**Severity:** **Medium** — auction settlement fees may go to a dead address.

---

### Finding 7 — MEDIUM: Missing Caller Verification in initialize() (moltcoin, moltpunks, moltswap)

**Contract:** moltcoin, moltpunks, moltswap  
**Files:**
- `contracts/moltcoin/src/lib.rs` line 47
- `contracts/moltpunks/src/lib.rs` line 40
- `contracts/moltswap/src/lib.rs` line 211  
**Category:** Missing Validation  

```rust
// moltcoin
pub extern "C" fn initialize(owner_ptr: *const u8) {
    if storage_get(b"owner").is_some() { return; }
    let mut owner_array = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner_array.as_mut_ptr(), 32); }
    // NOTE: no get_caller() verification
    storage_set(b"owner", &owner.0);
    ...
}
```

These three contracts accept any address as admin/owner without verifying `get_caller()` matches the provided pointer. While re-initialization guards prevent re-calling, the **first** call can set an arbitrary address as owner.

**Contrast:** 26 other contracts verify `get_caller().0 == addr` in their `initialize()`. `moltswap` partially mitigates this by setting `get_caller()` as admin (line 236), but `moltcoin` and `moltpunks` do not.

**Severity:** **Medium** — exploitable only at deployment time (single opportunity). If the deployer is honest, no risk.

---

### Finding 8 — MEDIUM: Missing Caller Verification in initialize() (wbnb_token, weth_token, wsol_token)

**Contract:** wbnb_token, weth_token, wsol_token  
**Files:**
- `contracts/wbnb_token/src/lib.rs` line 199
- `contracts/weth_token/src/lib.rs` line 199
- `contracts/wsol_token/src/lib.rs` line 199  
**Category:** Missing Validation  

```rust
pub extern "C" fn initialize(admin: *const u8) -> u32 {
    let existing = load_addr(ADMIN_KEY);
    if !is_zero(&existing) { return 1; }
    let mut addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(admin, addr.as_mut_ptr(), 32); }
    if is_zero(&addr) { return 2; }
    // NOTE: no get_caller() check
    storage_set(ADMIN_KEY, &addr);
    ...
}
```

Same pattern as Finding 7. These three wrapped token contracts are identical templates. They check for zero address and existing admin but don't verify `get_caller()`.

**Severity:** **Medium** — same rationale as Finding 7.

---

### Finding 9 — MEDIUM: `respond_challenge` Returns Success (0) on Caller Mismatch

**Contract:** reef_storage  
**File:** `contracts/reef_storage/src/lib.rs`  
**Lines:** 941–944  
**Category:** Incorrect Return Code  

```rust
let real_caller = get_caller();
if real_caller.0 != prov_arr {
    log_info("respond_challenge rejected: caller mismatch");
    return 0;  // BUG: returns 0 (success) instead of an error code
}
```

**Impact:** When a non-provider calls `respond_challenge`, the function correctly rejects the call (no state mutation), but returns `0` which is the success code. Callers and front-ends will incorrectly interpret this as a successful challenge response.

**Severity:** **Medium** — no state corruption, but misleading API behavior.

---

### Finding 10 — LOW: Lobsterlend Oracle Price Integration Is a Placeholder

**Contract:** lobsterlend  
**File:** `contracts/lobsterlend/src/lib.rs`  
**Line:** 13  
**Category:** Incomplete Logic / Missing Feature  

```rust
//   - Oracle price integration placeholder
```

The lending protocol's liquidation logic uses a simple `deposit * threshold / 100` check against borrow amount (line ~612), with no oracle-based collateral pricing. Collateral and debt are treated as having equal value. This means the protocol cannot support multi-asset lending where collateral and borrow tokens have different prices.

**Impact:** Limited to same-asset or same-price-pair lending. Not a vulnerability in the current single-asset design, but a functional limitation noted for production readiness.

**Severity:** **Low** — functional limitation, not a security issue.

---

### Finding 11 — LOW: dex_core Balance Validation Uses Fail-Open Pattern

**Contract:** dex_core  
**File:** `contracts/dex_core/src/lib.rs`  
**Lines:** 1108–1149  
**Category:** Security Issue (Mitigated)  

```rust
// F19.11a: Balance validation via cross-contract call to token contract
let bal_result = call_contract(call);
if let Ok(bal_bytes) = bal_result {
    // ... validates balance
}
// If call failed or returned empty, cross-contract calls not yet supported — allow trade (fail-open)
```

**Impact:** If the cross-contract call to the token contract fails or is unavailable, the order is placed without balance validation. This is documented as intentional for runtime compatibility but means orders can be placed without sufficient funds.

**Severity:** **Low** — documented trade-off; the balance check is defense-in-depth additional to the host-level transfer that must succeed at settlement.

---

### Finding 12 — LOW: dex_core Fee Transfer Uses Best-Effort Pattern

**Contract:** dex_core  
**File:** `contracts/dex_core/src/lib.rs`  
**Line:** 1553  
**Category:** Security Issue (Mitigated)  

```rust
let _ = call_contract(call); // best-effort
```

**Impact:** Taker fees may not actually be collected if the cross-contract transfer fails. The fee is recorded in the internal treasury counter but may not correspond to actual token movements.

**Severity:** **Low** — fee accounting discrepancy; does not affect user funds.

---

### Finding 13 — LOW: shielded_pool ZK Proof Verification Is Delegated

**Contract:** shielded_pool  
**File:** `contracts/shielded_pool/src/lib.rs`  
**Lines:** 356–413  
**Category:** Security Design Note  

```rust
fn verify_shield_proof(&self, proof: &[u8], ...) -> Result<(), ShieldedPoolError> {
    if proof.len() != 128 {
        return Err(ShieldedPoolError::InvalidProof(...));
    }
    // Proof was already cryptographically verified by the processor.
    Ok(())
}
```

All three proof verification functions (`verify_shield_proof`, `verify_unshield_proof`, `verify_transfer_proof`) only check that the proof is exactly 128 bytes. The actual Groth16/BN254 cryptographic verification is assumed to be done by the "TxProcessor" at the host layer.

**Impact:** If the TxProcessor is bypassed or misconfigured, any 128-byte sequence would be accepted as a valid ZK proof. This is architecturally defensible (keeping heavy crypto in the host) but creates a strong dependency on the processor layer.

**Severity:** **Low** — architectural decision documented in comments; security depends on host-layer implementation.

---

### Finding 14 — LOW: prediction_market Reads MoltyID Storage Directly

**Contract:** prediction_market  
**File:** `contracts/prediction_market/src/lib.rs`  
**Lines:** 1473–1478  
**Category:** Hardcoded Assumption  

```rust
// Since cross-contract calls are stubs on MoltChain, we read MoltyID's storage
// directly using the known key format: "rep:{hex_encoded_pubkey}"
```

**Impact:** The prediction market contract reads MoltyID's internal storage directly, bypassing cross-contract call APIs. This creates a tight coupling to MoltyID's internal storage key format. If MoltyID changes its key schema, the reputation check breaks silently (returns 0, which may be below the threshold, blocking market creation).

**Severity:** **Low** — coupling risk; operational rather than security concern.

---

## Contracts With No Findings

The following 18 contracts had no actionable findings:

| Contract | Lines | Notes |
|----------|-------|-------|
| bountyboard | 1390 | Reentrancy guard, MoltyID gate, caller verification ✓ |
| clawpay | 2115 | Streaming payments, all functions verify caller ✓ |
| clawpump | 1997 | Fixed graceful degradation (returns false), caller verification ✓ |
| compute_market | 2324 | Escrow/disputes, reentrancy guard, caller verification ✓ |
| dex_amm | 1851 | Concentrated liquidity, Q32.32 math with u128 intermediates ✓ |
| dex_analytics | 1280 | OHLCV candles, admin-only writes ✓ |
| dex_governance | 1780 | Proposal governance, caller verification ✓ |
| dex_margin | 3177 | All operations verify caller, tiered liquidation, oracle freshness check ✓ |
| dex_rewards | 1327 | Trading rewards/referrals, caller verification ✓ |
| dex_router | 1194 | Smart order routing, admin-only route management ✓ |
| moltbridge | 2654 | Multi-sig bridge, source TX deduplication, caller verification ✓ |
| moltdao | 1841 | Quadratic voting DAO, SHA-256 proposal IDs ✓ |
| moltyid | 6689 | Identity/reputation, social recovery, name auctions ✓ |
| moltoracle | 1342 | Oracle with staleness checks, SHA-256 VRF ✓ |
| musd_token | 1179 | Treasury-backed stablecoin, circuit breaker, epoch caps ✓ |
| wbnb_token† | 854 | (Finding 8 only — init issue; all other functions verify caller) |
| weth_token† | 854 | (Finding 8 only) |
| wsol_token† | 854 | (Finding 8 only) |

† Listed both here (for operational completeness) and in findings.

---

## Summary of Recommendations

1. **Critical (1):** Implement real Merkle proof verification in `reef_storage::respond_challenge`.
2. **High (3):** Change `transfer_molt_out` / `transfer_musd_out` in `clawvault`, `reef_storage`, and `prediction_market` to return `false` when the token address is not configured (matching the fix already applied in `clawpump`).
3. **Medium (5):** Add `get_caller()` verification to all 6 `initialize()` functions missing it. Fix `respond_challenge` return code from `0` to an error code. Remove `[0x4D; 32]` fallback addresses.
4. **Low (5):** Document or accept the fail-open patterns, oracle placeholder, storage coupling, and ZK proof delegation as known limitations.
