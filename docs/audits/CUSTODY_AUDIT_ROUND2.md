# Custody System Audit Report — Round 2

**Date**: 2025-07
**Scope**: `custody/src/main.rs` (~11,600 lines), `contracts/lichenbridge/src/lib.rs` (~1,450 lines), RPC proxy endpoints
**Version**: v0.4.29 → v0.4.30
**Prior Audit**: `CUSTODY_AUDIT_REPORT.md` (February 2025, 5,755-line codebase)

## Summary

Full security and correctness audit of the Lichen custody bridge system covering:
- Deposit address generation and key derivation
- Chain watchers (Solana, Ethereum, BSC)
- Sweep, credit, and withdrawal worker pipelines
- Reserve ledger management and rebalance logic
- FROST Ed25519 and Gnosis Safe EVM threshold signing
- Bridge contract (lock/mint/unlock multi-call confirmation)
- RPC custody proxy endpoints
- WebSocket events, webhooks, and API authentication

**Findings**: 6 issues (2 critical, 3 medium, 1 low). All fixed.
**Tests**: 72 custody tests passing. Bridge WASM build clean (0 warnings).

---

## Findings

### CUST-01 — CRITICAL: Reserve Ledger Unit Mismatch

**File**: `custody/src/main.rs`
**Impact**: Withdrawal reserve checks and debits used spore amounts (9 decimals) against a reserve ledger tracked in source-chain units (e.g., 6 decimals for ETH USDT). This caused:
- Valid withdrawals rejected as "insufficient reserves" (ETH USDT: 1e9 spores vs 1e6 reserve)
- Incorrect reserve debit amounts (1000x overdecrement for ETH, underflow for BSC 18-dec tokens)

**Root Cause**: Deposits credit the reserve in source-chain units via `adjust_reserve_balance()`, but `create_withdrawal` compared raw `req.amount` (spores) against the reserve, and withdrawal confirmation decremented by `job.amount` (spores).

**Fix**: Applied `spores_to_chain_amount()` conversion at 3 locations:
1. `create_withdrawal` reserve sufficiency check — convert to chain units before comparing
2. `create_withdrawal` deficit calculation — use converted `chain_amount_u64` for rebalance trigger
3. Withdrawal confirmation reserve decrement — convert `job.amount` before calling `adjust_reserve_balance`

### CUST-02 — CRITICAL: Bridge Reentrancy on submit_unlock / confirm_unlock

**File**: `contracts/lichenbridge/src/lib.rs`
**Impact**: `submit_unlock` and `confirm_unlock` were missing `reentrancy_enter()`/`reentrancy_exit()` guards that `submit_mint`/`confirm_mint` already had. Both functions call `transfer_out()` which makes cross-contract calls, creating a theoretical reentrancy vector.

**Root Cause**: When AUDIT-FIX C-7 added reentrancy guards to submit_mint/confirm_mint, the submit_unlock/confirm_unlock functions were not updated.

**Fix**: Added `reentrancy_enter()` after the pause check and `reentrancy_exit()` before every return path in both functions. Pattern matches submit_mint/confirm_mint exactly.

### CUST-03 — MEDIUM: Rebalance Threshold Check Misses BSC

**File**: `custody/src/main.rs`, `check_rebalance_thresholds()`
**Impact**: The rebalance monitor only iterated `["solana", "ethereum"]`, completely ignoring BSC. USDT/USDC reserves on BSC could drift arbitrarily without automatic rebalancing.

**Fix**: Added `"bsc"` to the chain iterator: `["solana", "ethereum", "bsc"]`.

### CUST-04 — MEDIUM: derive_evm_address Missing Zeroize

**File**: `custody/src/main.rs`, `derive_evm_address()`
**Impact**: The intermediate HMAC seed used to derive the EVM signing key was not zeroized after use, leaving key material in memory. The adjacent `derive_evm_signing_key()` function already had proper zeroize.

**Fix**: Added `seed.as_mut_slice().zeroize()` after creating the signing key, matching the existing pattern.

### CUST-05 — MEDIUM: Credit Worker Silently Skips Without Logging

**File**: `custody/src/main.rs`, `process_credit_jobs()` and `build_credit_job()`
**Impact**: When `licn_rpc_url` or `treasury_keypair_path` is None, both functions returned Ok() silently. Credit jobs would accumulate in "queued" state indefinitely with no indication of misconfiguration.

**Fix**: Added `tracing::warn!()` messages before each early return to surface the misconfiguration.

### CUST-06 — LOW: build_credit_job Silent u128→u64 Truncation

**File**: `custody/src/main.rs`, `build_credit_job()`
**Impact**: Decimal conversion used `as u64` which silently truncates on overflow. While practically unreachable (would require >18.4 billion ETH deposit), it violates defensive coding for a custody system.

**Fix**: Replaced all three `as u64` casts with `u64::try_from(...).map_err(...)` to produce explicit errors on overflow.

---

## Items Verified Correct (No Fix Needed)

- **Key derivation**: HMAC-SHA256(master_seed, BIP-44 path) for both Solana (Ed25519) and EVM (secp256k1)
- **PDA derivation**: Correct Solana canonical algorithm (SHA-256 + "ProgramDerivedAddress" suffix + bump scan + off-curve check)
- **Decimal mapping**: `source_chain_decimals()` correct for all chain/asset pairs
- **FROST signing**: Two-round commitment/signature protocol with proper nonce handling
- **EVM Safe signing**: Gnosis Safe packed signature format with sorted signers
- **Burn verification**: 4-check validation (contract, caller, method, amount) before withdrawals
- **TX intent log**: Pre-broadcast intent recording for crash recovery
- **API authentication**: Constant-time comparison via `verify_api_auth()`
- **Webhook signatures**: HMAC-SHA256 with per-webhook secrets
- **Rate limiting**: Per-IP deposit creation limits
- **Bridge pause**: Emergency circuit breaker on all validator operations
- **Source TX / burn proof dedup**: Prevents double-processing
- **RPC proxy**: Server-side Bearer token, input validation (base58, UUID, limit cap)
