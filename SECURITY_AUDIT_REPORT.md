# MoltChain Smart Contract Security Audit Report

**Scope**: All 27 contracts in `/contracts/`  
**Lines audited**: ~42,149 across bountyboard, clawpay, clawpump, clawvault, lobsterlend, moltcoin, compute_market, dex_core (partial), dex_margin, moltbridge, prediction_market, moltyid, musd_token, moltdao, moltoracle, moltauction, moltswap, reef_storage + remaining contracts reviewed for pattern issues.

---

## TOP 20 CRITICAL ISSUES

---

### #1 — CRITICAL | compute_market | Lines ~1655–1757 | Missing `get_caller()` Verification on All Admin Functions

**Type**: Missing Caller Verification  
**File**: `contracts/compute_market/src/lib.rs`

**Description**: Five admin-gated functions — `set_identity_gate`, `set_moltyid_address`, `cm_pause`, `cm_unpause`, and `set_platform_fee` — accept a `caller: *const u8` parameter and check `is_admin(&caller)`, but **never call `get_caller()`** to verify the parameter matches the actual transaction signer. An attacker who knows the admin address can pass it as the `caller` parameter, bypassing all access control and pausing the market, changing fees, or swapping the identity gate.

**Affected lines** (approximate):
- `set_identity_gate` ~1680: `if caller[..] != admin[..]` — no `get_caller()`
- `set_moltyid_address` ~1655: same
- `cm_pause` ~1748: same
- `cm_unpause` ~1757: same
- `set_platform_fee` ~1730: same

**Fix**:
```rust
pub fn cm_pause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    // ADD THIS:
    let real_caller = get_caller();
    if real_caller.0 != caller { return 200; }
    if !is_admin(&caller) { return 1; }
    ...
}
```

---

### #2 — CRITICAL | moltcoin | Line ~186 | `approve()` Has No Caller Verification — Anyone Can Set Allowances

**Type**: Missing Caller Verification  
**File**: `contracts/moltcoin/src/lib.rs`

**Description**: The `approve(owner_ptr, spender_ptr, amount)` function sets `allowance[owner][spender] = amount` with **zero verification** that the caller is the owner. Any account can call `approve(victim, attacker, u64::MAX)` and immediately drain the victim's tokens via `transfer_from`. This is a complete authorization bypass on the most sensitive token approval function.

**Fix**:
```rust
pub fn approve(owner_ptr: *const u8, spender: *const u8, amount: u64) -> u32 {
    let mut owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32); }
    // ADD THIS:
    let real_caller = get_caller();
    if real_caller.0 != owner { return 200; }
    ...
}
```

---

### #3 — CRITICAL | moltcoin | Line ~155 | `mint()` Uses Parameter as Caller — Owner Address Can Be Spoofed

**Type**: Missing Caller Verification  
**File**: `contracts/moltcoin/src/lib.rs`

**Description**: `mint(owner_ptr, to_ptr, amount)` copies `owner_ptr` into a local array and passes it to `token.mint(to, amount, caller, owner)`. The internal check is `caller == owner`, where **both values come from the parameter**, not from `get_caller()`. Any account can call `mint(real_owner_addr, attacker_addr, max_u64)` with the real owner's address as the first argument, satisfying the check and minting unlimited tokens.

**Fix**: Replace the parameter-sourced `caller_array` with `get_caller().0` before the ownership check.

---

### #4 — HIGH | compute_market | Line ~1820 | `resolve_dispute()` Calculates Split But Never Transfers Funds

**Type**: Token Accounting Error / Dead Code  
**File**: `contracts/compute_market/src/lib.rs`

**Description**: In `resolve_dispute`, the function computes `_to_requester` and `_to_provider` (the payout split), clears the escrow storage, but **never calls `call_token_transfer`** to actually pay either party. Both variables are prefixed with `_` (unused), confirming the omission. Disputed jobs are settled with zero payout — all escrowed funds are permanently locked/lost.

**Fix**:
```rust
// After computing _to_requester and _to_provider:
if _to_requester > 0 {
    call_token_transfer(token_addr, escrow_addr, requester_addr, _to_requester)?;
}
if _to_provider > 0 {
    call_token_transfer(token_addr, escrow_addr, provider_addr, _to_provider)?;
}
```

---

### #5 — HIGH | compute_market | Line ~880 | `cancel_job` Timeout Measured From `created_slot`, Not `claim_slot`

**Type**: Logic Error  
**File**: `contracts/compute_market/src/lib.rs`

**Description**: When checking if a claimed job has timed out, the code computes `created_slot + complete_timeout` (bytes 145..153 of job data). It should use the **claim slot** (stored separately). If a job sat as PENDING for 900 slots before being claimed, and `complete_timeout` is 1000 slots, the provider has only 100 slots to complete after claiming — far less than intended. Providers can be penalized for timeouts they couldn't have met.

**Fix**: Store the claim timestamp separately and compute `claim_slot + complete_timeout` for the deadline check.

---

### #6 — HIGH | clawvault | Line ~430 | `get_user_position` Silent u64 Overflow

**Type**: Integer Overflow  
**File**: `contracts/clawvault/src/lib.rs`

**Description**: The vault share-to-asset conversion uses `shares * total_assets / total_shares` with **plain u64 arithmetic**. If both `shares` and `total_assets` are in the range of ~1e15, the multiplication silently overflows u64 (max ~1.8e19), producing a wildly incorrect (and likely tiny) asset value. Users querying their position or withdrawing could receive orders of magnitude less than they are owed.

**Fix**:
```rust
let assets = (shares as u128 * total_assets as u128 / total_shares as u128) as u64;
```

---

### #7 — HIGH | clawvault | Line ~370 | `harvest()` u64 Overflow in Fee Calculation

**Type**: Integer Overflow  
**File**: `contracts/clawvault/src/lib.rs`

**Description**: The `harvest` function computes `total_assets * allocation / 100` in plain u64. For a vault with `total_assets` near 1e18 (large but realistic in 9-decimal token units), the product overflows before division, producing a catastrophically wrong harvest amount. This can drain the vault or record incorrect yield.

**Fix**:
```rust
let yield_amount = (total_assets as u128 * allocation as u128 / 100) as u64;
```

---

### #8 — HIGH | clawpump | Lines ~380–420 | Graduation Partial Cross-Call Failure Not Reverted

**Type**: Unchecked Return Values / Token Accounting Error  
**File**: `contracts/clawpump/src/lib.rs`

**Description**: When a token graduates to the DEX, three cross-contract calls are made (`create_pair`, `create_pool`, `add_liquidity`). Their results are checked only for logging. If `create_pair` succeeds but `add_liquidity` fails, the token is **marked as graduated** (`data[64] = 1`) and the liquidity MOLT is permanently locked with no DEX pair. Users can no longer sell, and the tokens are effectively frozen.

**Fix**: Revert graduation status on any cross-call failure:
```rust
if pair_ok != 0 || pool_ok != 0 || seed_ok != 0 {
    log_info("Graduation failed — manual intervention required");
    // Do NOT set data[64] = 1; revert the state
    return 500;
}
data[64] = 1; // Only mark graduated on full success
```

---

### #9 — HIGH | moltdao | Lines ~330–400 | `execute_proposal` Does Not Actually Execute Anything

**Type**: Dead Code / Logic Error  
**File**: `contracts/moltdao/src/lib.rs`

**Description**: After passing all governance checks (quorum, approval, time-lock), `execute_proposal` logs "Proposal approved!" and sets `proposal[192] = 1`, but **makes no cross-contract call** to the `target_contract` with the `action` payload. The DAO is a governance theater — proposals pass votes and get marked executed, but no on-chain action is ever taken. Treasury transfers require a separate manual `treasury_transfer` call.

**Fix**: Add a `call_contract` dispatch to the target contract with the stored action payload after the approval checks pass.

---

### #10 — MEDIUM | clawpay | Line ~358 | `create_stream_with_cliff` Missing Reentrancy Guard

**Type**: Missing Reentrancy Guard  
**File**: `contracts/clawpay/src/lib.rs`

**Description**: `create_stream` correctly wraps its logic in `reentrancy_enter()` / `reentrancy_exit()`. The sibling function `create_stream_with_cliff` (line ~358), which also calls `call_token_transfer`, has **no reentrancy guard**. A malicious token contract's `transfer` callback could re-enter `create_stream_with_cliff` and create duplicate streams before the first completes, potentially double-locking tokens or creating invalid accounting.

**Fix**: Add `if !reentrancy_enter() { return 5; }` at the start and `reentrancy_exit()` before all return paths.

---

### #11 — MEDIUM | moltdao | Lines ~470–499 | `cancel_proposal` Missing `get_caller()` Check

**Type**: Missing Caller Verification  
**File**: `contracts/moltdao/src/lib.rs`

**Description**: `cancel_proposal(canceller_ptr, proposal_id)` checks that `canceller[..] == proposer[..]` but never calls `get_caller()` to verify the `canceller_ptr` parameter is actually the transaction signer. An attacker who knows the proposer's address can cancel any proposal on their behalf. Pattern: `let mut canceller = [0u8; 32]; unsafe { copy... }; if canceller[..] != proposer[..] { return 0; }` — no `get_caller()` anywhere in the function.

**Fix**: Add `let real_caller = get_caller(); if real_caller.0 != canceller { return 0; }` after reading the parameter.

---

### #12 — MEDIUM | lobsterlend | Line ~225 | `withdraw` Blocked During Pause — Funds Can Be Trapped Indefinitely

**Type**: Logic Error  
**File**: `contracts/lobsterlend/src/lib.rs`

**Description**: `withdraw` checks `is_paused()` at its start and returns early — blocking all withdrawals. However, `repay` does **not** check `is_paused()`. This asymmetry means: during a pause, borrowers can repay their loans but depositors cannot retrieve their collateral. If an admin pauses and keys are lost, or a governance dispute prevents unpausing, all depositors' funds are permanently locked.

**Fix**: Either (a) allow `withdraw` during pause (only block new deposits/borrows), or (b) add a governance-enforced maximum pause duration with automated unpause.

---

### #13 — MEDIUM | clawpay | Line ~453 | `transfer_stream` Missing Reentrancy Guard

**Type**: Missing Reentrancy Guard  
**File**: `contracts/clawpay/src/lib.rs`

**Description**: `transfer_stream` modifies stream ownership in storage. If the new owner is a contract with a `receive` hook that calls back into `withdraw_from_stream` before the ownership update is written, it could claim funds under the old owner. No `reentrancy_enter()` / `reentrancy_exit()` is present.

**Fix**: Wrap the function body with the reentrancy guard pattern used in `create_stream`.

---

### #14 — MEDIUM | moltoracle | Lines ~150–190 | `submit_price` Accepts Parameter-Provided Feeder Without `get_caller()` Verification

**Type**: Missing Caller Verification  
**File**: `contracts/moltoracle/src/lib.rs`

**Description**: `submit_price(feeder_ptr, asset_ptr, ...)` loads the authorized feeder for the asset and checks `feeder[..] != authorized_feeder[..]`, where `feeder` comes from the parameter, not from `get_caller()`. An attacker who monitors the chain can call `submit_price(authorized_feeder_addr, asset, manipulated_price, ...)` and inject arbitrary prices. The oracle controls prices for all DEX pairs, margin positions, and prediction markets.

**Fix**:
```rust
let real_caller = get_caller();
if real_caller.0 != feeder {
    return 0; // reject — caller isn't the declared feeder
}
// Then check feeder == authorized_feeder as before
```

---

### #15 — MEDIUM | clawpay | Line ~295 | `cancel_stream` Reports Refund Amount But Makes No Token Transfer

**Type**: Token Accounting Error  
**File**: `contracts/clawpay/src/lib.rs`

**Description**: `cancel_stream` computes the refund due to the stream creator (unstreamed amount) and sets it as return data via `set_return_data`, but **never calls `call_token_transfer`** to actually send the tokens back. The stream is marked cancelled and the locked tokens are permanently stranded in the contract. Callers reading the return data will think the refund succeeded.

**Fix**: After computing `refund_amount`, add:
```rust
if refund_amount > 0 {
    call_token_transfer(token_addr, contract_addr, creator_addr, refund_amount)?;
}
```

---

### #16 — MEDIUM | bountyboard | Line ~440 | `cancel_bounty` Makes No Token Transfer Back to Creator

**Type**: Token Accounting Error  
**File**: `contracts/bountyboard/src/lib.rs`

**Description**: `cancel_bounty` marks the bounty as `BOUNTY_CANCELLED` and sets `set_return_data` with the reward amount, but performs no `call_token_transfer` to return the reward to the bounty creator. The tokens intended as the bounty remain locked in the contract indefinitely.

**Fix**: Add the actual transfer back to the bounty creator before returning.

---

### #17 — MEDIUM | lobsterlend | Line ~370 | `borrow` Collateral Check Uses Plain u64 Multiplication — Overflow Risk

**Type**: Integer Overflow  
**File**: `contracts/lobsterlend/src/lib.rs`

**Description**: The maximum borrow computation is `deposit_val * COLLATERAL_FACTOR_PERCENT / 100`. If `deposit_val` is large (> ~1.8e17 in microunits), `deposit_val * COLLATERAL_FACTOR_PERCENT` silently overflows u64, producing a wrong (likely tiny) borrow limit. Borrowers with large deposits could be incorrectly blocked from borrowing their full allowed amount.

**Fix**:
```rust
let max_borrow = (deposit_val as u128 * COLLATERAL_FACTOR_PERCENT as u128 / 100) as u64;
```

---

### #18 — MEDIUM | moltauction | Lines ~140–155 | `create_auction` Missing `get_caller()` Check — Anyone Can Create Auctions as Any Seller

**Type**: Missing Caller Verification  
**File**: `contracts/moltauction/src/lib.rs`

**Description**: `create_auction(seller_ptr, ...)` does not call `get_caller()` to verify the `seller_ptr` parameter matches the transaction signer. The function only checks that the seller owns the NFT (via `call_nft_owner`). An attacker who knows the NFT owner's address can front-run and create an auction on their behalf with fraudulent parameters (e.g., minimum bid of 1 shell), stealing the NFT for essentially nothing when they win.

**Fix**:
```rust
let real_caller = get_caller();
if real_caller.0 != seller { return 0; }
```

---

### #19 — LOW | clawpump | Lines ~244, 337, 508 | `u64`-returning Functions Use `200` as Error Code (Ambiguous with Valid Token IDs)

**Type**: Logic Error  
**File**: `contracts/clawpump/src/lib.rs`

**Description**: Three functions with return type `u64` return `200` on a caller mismatch error: `create_token` (returns `token_id`), `buy` (returns "tokens received"), and `sell` (returns "refund amount"). Any caller checking `if result == 0 { /* error */ }` will interpret `200` as a successful return of 200 tokens/shells, masking the error. Callers cannot distinguish an unauthorized call from a successful small transaction.

**Fix**: Return `0` for all error conditions in `u64`-returning functions, consistent with the documented interface. Alternatively, use a separate error-signaling mechanism (e.g., set_return_data with an error code).

---

### #20 — LOW | ALL 27 CONTRACTS | No Event/Log System for Critical State Changes

**Type**: Missing Events  
**File**: All contracts

**Description**: No contract emits structured, machine-readable events for critical state changes: token mints/burns/transfers, parameter changes, graduations, liquidations, bridge operations, or governance executions. Only free-text `log_info()` strings are used. Off-chain indexers, wallets, and explorers cannot reliably monitor contract state without a structured event format, making it impossible to build reliable notification systems or audit trails.

**Fix**: Define a standardized event encoding schema (e.g., `emit_event(event_type: u8, data: &[u8])` stored to a per-block log key) and call it on every critical state transition.

---

## Summary Table

| # | Contract | Line | Type | Severity |
|---|----------|------|------|----------|
| 1 | compute_market | ~1655–1757 | Missing `get_caller()` on 5 admin fns | CRITICAL |
| 2 | moltcoin | ~186 | `approve()` — no auth, anyone can set allowances | CRITICAL |
| 3 | moltcoin | ~155 | `mint()` — owner address spoofable via parameter | CRITICAL |
| 4 | compute_market | ~1820 | `resolve_dispute` — no token transfer ever executes | HIGH |
| 5 | compute_market | ~880 | `cancel_job` — timeout from wrong base slot | HIGH |
| 6 | clawvault | ~430 | `get_user_position` — `shares * total_assets` u64 overflow | HIGH |
| 7 | clawvault | ~370 | `harvest` — `total_assets * allocation` u64 overflow | HIGH |
| 8 | clawpump | ~380 | Graduation partial DEX failure leaves tokens locked | HIGH |
| 9 | moltdao | ~330 | `execute_proposal` marks executed but dispatches no action | HIGH |
| 10 | clawpay | ~358 | `create_stream_with_cliff` — no reentrancy guard | MEDIUM |
| 11 | moltdao | ~470 | `cancel_proposal` — no `get_caller()` check | MEDIUM |
| 12 | lobsterlend | ~225 | `withdraw` blocked during pause — funds trapped | MEDIUM |
| 13 | clawpay | ~453 | `transfer_stream` — no reentrancy guard | MEDIUM |
| 14 | moltoracle | ~150 | `submit_price` — feeder address spoofable | MEDIUM |
| 15 | clawpay | ~295 | `cancel_stream` — no actual token transfer | MEDIUM |
| 16 | bountyboard | ~440 | `cancel_bounty` — no actual token transfer | MEDIUM |
| 17 | lobsterlend | ~370 | `borrow` — collateral check u64 overflow | MEDIUM |
| 18 | moltauction | ~140 | `create_auction` — no `get_caller()` check | MEDIUM |
| 19 | clawpump | ~244,337,508 | `u64` fns return `200` as error (ambiguous) | LOW |
| 20 | ALL (27) | — | No structured event system | LOW |
