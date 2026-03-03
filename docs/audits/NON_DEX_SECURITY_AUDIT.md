# Moltchain Non-DEX Smart Contract Security Audit
**Scope:** 15 contracts + rpc/src/lib.rs RPC wiring  
**Auditor:** GitHub Copilot  
**Methodology:** Full source read of every contract + SDK

---

## Severity Legend
| Level | Meaning |
|---|---|
| **CRITICAL** | Exploitable loss of funds / complete DoS / broken security assumption |
| **HIGH** | Significant functional breakage or strong attack surface |
| **MEDIUM** | Correctness or logic flaw; limited exploitability but materially wrong |
| **LOW** | Minor edge case, code smell, or latent risk requiring specific conditions |
| **INFO** | Informational / improvement recommendation |

---

## Summary of Findings

| ID | Contract | Severity | Title |
|---|---|---|---|
| NC-01 | moltoracle | **CRITICAL** | Staleness threshold 3.6s not 1 hour — unit mismatch |
| NC-02 | moltoracle | **CRITICAL** | `get_aggregated_price` same staleness unit bug |
| NC-03 | shielded_pool | **CRITICAL** | No reentrancy guard on `shield`/`unshield`/`transfer` |
| NC-04 | shielded_pool | **CRITICAL** | No caller/owner verification on state-mutating WASM ABI |
| NC-05 | shielded_pool | **CRITICAL** | No pause/emergency-stop mechanism |
| NC-06 | lobsterlend | **HIGH** | Health factor computation overflows u64 for large deposits |
| NC-07 | shielded_pool | **HIGH** | Unbounded JSON commitment vector causes state bloat / DoS |
| NC-08 | moltoracle | **MEDIUM** | Legacy `request_randomness` is front-runnable / predictable |
| NC-09 | moltdao | **MEDIUM** | `PROPOSAL_SIZE` constant is 210 but actual layout is 212 bytes |
| NC-10 | clawvault | **MEDIUM** | Strategy total allocation can exceed 100% — over-commit funds |
| NC-11 | clawvault | **MEDIUM** | `harvest()` silently skips if protocol addresses unconfigured |
| NC-12 | compute_market | **MEDIUM** | No pause function — cannot halt market in emergency |
| NC-13 | clawpump | **LOW** | `transfer_molt_out` graceful degradation silently loses user funds |
| NC-14 | moltswap | **LOW** | Reputation bonus double-loads pool (TOCTOU code smell) |
| NC-15 | lobsterlend | **LOW** | Block-time assumption in interest accrual (`/ 400` ms per slot) |
| NC-16 | bountyboard | **LOW** | Submission count stored as `u8` — silent overflow at 255 |
| NC-17 | clawpay | **LOW** | Silent failure if `self_addr` or `token_addr` is None |
| NC-18 | moltbridge | **INFO** | Identity gate defaults to allow when MoltyID addr unset |
| NC-19 | shielded_pool | **INFO** | `empty_merkle_root()` recomputes 32 SHA-256 rounds every call |
| RPC-01 | rpc/lib.rs | **INFO** | moltcoin has no dedicated RPC method |
| RPC-02 | rpc/lib.rs | **INFO** | ClawPump not accessible via JSON-RPC (REST only) |
| RPC-03 | rpc/lib.rs | **INFO** | MoltOracle has no `getOraclePrice` / `getOracleAttestation` endpoint |
| RPC-04 | rpc/lib.rs | **INFO** | LobsterLend has no per-account position endpoint |
| RPC-05 | rpc/lib.rs | **INFO** | ClawPay has no per-stream query endpoint |

---

## Detailed Findings

---

### NC-01 · CRITICAL · moltoracle — Staleness threshold 3.6 seconds, not 1 hour

**File:** [contracts/moltoracle/src/lib.rs](contracts/moltoracle/src/lib.rs#L201-L204)

```rust
let now = get_timestamp();       // returns MILLISECONDS
if now - timestamp > 3600 {      // BUG: 3600 ms = 3.6 seconds
    return 2;                    // marks price stale
}
```

**Root Cause:** `get_timestamp()` (SDK `sdk/src/lib.rs:346`) returns Unix time in **milliseconds**. The intended staleness window of "1 hour" requires the threshold `3_600_000` (ms), not `3600`. As written, any price submitted more than 3.6 seconds ago is immediately rejected as stale, making `get_price()` completely non-functional under normal network latency.

Every consumer of oracle prices (`moltoracle::get_price`, AMMs, prediction markets, bridge collateral checks) will receive return code `2` ("stale") for virtually every query, silently breaking all price-dependent functionality without a revert.

**Evidence from codebase — consistent ms usage elsewhere:**
```
contracts/moltyid/src/lib.rs: VOUCH_COOLDOWN_MS = 3_600_000  (1 hour in ms)
contracts/moltswap/src/lib.rs: deadline > now  (deadlines stored as ms timestamps)
```

**Fix:**
```rust
// contracts/moltoracle/src/lib.rs:L203
if now - timestamp > 3_600_000 {   // 1 hour in milliseconds
```

**Impact:** All oracle price consumers receive stale errors. Any system that aborts on stale (e.g., moltdao balance lookup, prediction market resolution) is fully broken; any that falls back silently gives wrong output.

---

### NC-02 · CRITICAL · moltoracle — `get_aggregated_price` same staleness unit bug

**File:** [contracts/moltoracle/src/lib.rs](contracts/moltoracle/src/lib.rs#L829-L832)

```rust
let now = get_timestamp();
if now - timestamp <= 3600 {   // same bug: 3.6s freshness window
    total_price += price as u128;
    valid_feeds += 1;
}
```

`get_aggregated_price` applies the same incorrect `<= 3600` ms threshold. All feeds submitted more than 3.6 seconds before the query are excluded; in practice `valid_feeds` will always be 0 and the function returns `2` ("all feeds stale").

**Fix:**
```rust
if now - timestamp <= 3_600_000 {   // L832
```

---

### NC-03 · CRITICAL · shielded_pool — No reentrancy guard on state-mutating WASM ABI

**File:** [contracts/shielded_pool/src/lib.rs](contracts/shielded_pool/src/lib.rs#L608-L700)

```rust
#[no_mangle]
pub extern "C" fn shield(args_ptr: *const u8, args_len: u32) -> u32 {
    // NO reentrancy_enter() here
    let mut state = load_state();          // deserializes full JSON blob
    match state.shield(&request, slot) {
        Ok(index) => {
            save_state(&state);            // if reentrant call occurs between load and save
            ...                            // second load reads stale state, both saves corrupt
        }
    }
}
```

All three state-modifying WASM ABI functions (`shield`, `unshield`, `transfer`) load the entire pool state from a JSON blob, mutate it, then save it back — without any reentrancy guard. If a reentrant call is triggered between `load_state()` and `save_state()`, the second invocation loads the pre-mutation state; both executions then write back conflicting versions, last writer wins. This could be used to double-spend nullifiers.

Every other contract in the codebase uses `reentrancy_enter()` / `reentrancy_exit()` from the SDK. Shielded pool is the sole exception.

**Fix:**
```rust
pub extern "C" fn shield(args_ptr: *const u8, args_len: u32) -> u32 {
    if !moltchain_sdk::reentrancy_enter() { return 99; }
    // ... existing logic ...
    moltchain_sdk::reentrancy_exit();
    0
}
```
Apply identically to `unshield` and `transfer`.

---

### NC-04 · CRITICAL · shielded_pool — No caller/owner verification on mutation functions

**File:** [contracts/shielded_pool/src/lib.rs](contracts/shielded_pool/src/lib.rs#L608)  
**File:** [contracts/shielded_pool/src/lib.rs](contracts/shielded_pool/src/lib.rs#L645)  
**File:** [contracts/shielded_pool/src/lib.rs](contracts/shielded_pool/src/lib.rs#L672)

`shield()`, `unshield()`, and `transfer()` accept arbitrary callers without any authentication. The security model assumes the **processor** verifies a Groth16 ZK proof before dispatching to the contract. If that assumption holds, direct calls cannot forge proofs.

However, the contract itself provides **zero enforcement** of this invariant. An attacker who finds a way to call the WASM ABI directly (e.g. through a future RPC change, a cross-contract call path, or a processor bug) can:
- Add arbitrary commitments to the Merkle tree
- Mark nullifiers as spent (grief users)
- Manipulate the Merkle root

The `initialize` function (line 547) stores an owner, but this owner is never checked in `shield`/`unshield`/`transfer`.

**Fix:** At minimum, verify that the caller is the designated processor address on every mutation:
```rust
pub extern "C" fn shield(args_ptr: *const u8, args_len: u32) -> u32 {
    let processor = storage_get(OWNER_KEY).unwrap_or_default();
    let caller = get_caller();
    if caller.0[..] != processor[..] { return 2; }  // unauthorized
    ...
}
```

---

### NC-05 · CRITICAL · shielded_pool — No pause/emergency-stop mechanism

**File:** [contracts/shielded_pool/src/lib.rs](contracts/shielded_pool/src/lib.rs)

The entire shielded pool WASM ABI module has no pause function and no pause check. Every other contract in the codebase (moltcoin, moltyid, moltbridge, moltdao, moltoracle, moltswap, lobsterlend, bountyboard, clawpay, clawpump, clawvault, prediction_market, compute_market, musd_token) implements `pause()/unpause()` guarded by owner and checked at the top of every mutation.

In the event of a ZK circuit vulnerability, processor compromise, or discovered exploit, there is no administrative mechanism to halt shielded deposits/withdrawals. Privacy pools hold custodied MOLT; inability to pause is a critical operational security gap.

**Fix:** Add `sp_pause`/`sp_unpause` functions (owner-gated, storing `sp_paused = [1]`) and add a pause check at the top of `shield`, `unshield`, and `transfer`.

---

### NC-06 · HIGH · lobsterlend — Health factor computation overflows u64

**File:** [contracts/lobsterlend/src/lib.rs](contracts/lobsterlend/src/lib.rs#L750)

```rust
let health_factor = if borrow == 0 {
    u64::MAX
} else {
    deposit * LIQUIDATION_THRESHOLD_PERCENT * 100 / borrow
    // = deposit * 85 * 100 = deposit * 8500
};
```

`LIQUIDATION_THRESHOLD_PERCENT = 85`, so the numerator is `deposit × 8500`. This overflows `u64` when `deposit > u64::MAX / 8500 ≈ 2.168 × 10¹⁵` shells (≈ 2,168,000 MOLT).

Any user with more than ~2.17 million MOLT deposited will receive a corrupted (wrapped) health factor from `get_account_info`. Front-ends, liquidation bots, and the RPC layer all rely on this value to determine liquidation eligibility. A silently-wrong health factor could:
- Report an extremely low health factor on a healthy account → spurious liquidation
- Report an extremely high health factor on an unhealthy account → missed liquidation

**Fix:**
```rust
// contracts/lobsterlend/src/lib.rs:L750
let health_factor = if borrow == 0 {
    u64::MAX
} else {
    let numerator = (deposit as u128) * (LIQUIDATION_THRESHOLD_PERCENT as u128) * 100u128;
    (numerator / (borrow as u128)).min(u64::MAX as u128) as u64
};
```

---

### NC-07 · HIGH · shielded_pool — Unbounded JSON commitment vector causes state bloat

**File:** [contracts/shielded_pool/src/lib.rs](contracts/shielded_pool/src/lib.rs#L50)

```rust
pub struct ShieldedPoolState {
    pub commitments: Vec<CommitmentEntry>,   // grows without bound
    pub nullifiers: Vec<[u8; 32]>,           // grows without bound
    ...
}
```

The entire pool state is serialized to a single JSON blob stored under the key `pool_state`. Every `shield()` appends one `CommitmentEntry`; every `unshield()` appends one 32-byte nullifier. There is no pruning, archival, or merkle-path-only storage.

At 1,000 shields, assuming ~200 bytes per entry: ~200 KB single-key state. At 10,000 shields: ~2 MB. WASM memory limits will cause OOM panics well before the pool reaches meaningful scale, effectively creating a permanent DoS through organic usage, or an immediate DoS through a low-cost griefing attack where an attacker shields and immediately unshields 1-shell amounts in a loop.

All other contracts store individual entries under separate per-key keys (e.g. `dep:{hex}`, `borrow:{hex}`). Shielded pool is the only one using a monolithic blob.

**Fix:** Store each commitment at an indexed key (`commit_{index}`) and each nullifier at a hash key (`null_{hex}`); store only `commitment_count` and `merkle_root` in the root record.

---

### NC-08 · MEDIUM · moltoracle — Legacy `request_randomness` is front-runnable

**File:** [contracts/moltoracle/src/lib.rs](contracts/moltoracle/src/lib.rs#L530)

```rust
// Labeled "legacy mode" in source
pub extern "C" fn request_randomness(requester_ptr: *const u8, seed: u64) -> u32 {
    // randomness derived from: requester + seed + timestamp
    // All three values are known at request-submission time
```

The legacy VRF path derives randomness from inputs that are all known (or predictable) at request time: the requester address (public), the seed (passed as a param, visible in the transaction), and the block timestamp (knowable by validators, often predictable). An adversary can:
1. Front-run the transaction with a different seed to land their preferred randomness output
2. As a validator, grind `seed` values until the resulting randomness is favorable

The contract already has a correct commit-reveal scheme (`commit_randomness` / `reveal_randomness`). The legacy function should be removed or restricted to admin-only.

**Fix:** Remove `request_randomness` or gate it with `return 99; // deprecated`.

---

### NC-09 · MEDIUM · moltdao — `PROPOSAL_SIZE` constant incorrect (210 vs actual 212)

**File:** [contracts/moltdao/src/lib.rs](contracts/moltdao/src/lib.rs#L317)

```rust
const PROPOSAL_SIZE: usize = 210;
```

The proposal layout as built at lines 482–497:
```
bytes   0-31:   proposer           (32)
bytes  32-63:   title_hash         (32)
bytes  64-95:   description_hash   (32)
bytes  96-127:  target_contract    (32)
bytes 128-159:  action_hash        (32)
bytes 160-167:  start_time         (8)
bytes 168-175:  end_time           (8)
bytes 176-183:  votes_for          (8)
bytes 184-191:  votes_against      (8)
byte  192:      executed           (1)
byte  193:      cancelled          (1)
byte  194:      quorum_met         (1)
byte  195:      proposal_type      (1)
bytes 196-203:  veto_votes         (8)
bytes 204-211:  stake_amount       (8)
── total ──────────────────────────── 212 bytes
```

The vector is 212 bytes when `stake_amount` is appended; the `while proposal.len() < PROPOSAL_SIZE` padding loop at [L500](contracts/moltdao/src/lib.rs#L500) is unreachable (212 > 210, loop never executes). The read-back guard at [L852](contracts/moltdao/src/lib.rs#L852) correctly uses `proposal.len() > 211` to read stake_amount, so the functional behavior is correct — but the constant misleads future developers and causes the minimum-size guard at [L600](contracts/moltdao/src/lib.rs#L600) to accept a truncated 210-byte proposal that is missing `stake_amount` (returns 0, skipping stake refund on execute/cancel).

**Fix:**
```rust
// contracts/moltdao/src/lib.rs:L317
const PROPOSAL_SIZE: usize = 212;
```

---

### NC-10 · MEDIUM · clawvault — No total allocation cap when adding strategies

**File:** [contracts/clawvault/src/lib.rs](contracts/clawvault/src/lib.rs)

`add_strategy()` validates that the new strategy's `allocation_percent` is ≤ 100, but does not check that the sum of all existing strategy allocations plus the new one stays ≤ 100%. An admin can register five strategies each with 100% allocation, producing a total of 500% committed. During `rebalance()`, the vault would attempt to move 5× its total assets — all five protocol calls would fail (or overdraw), leaving funds stuck in mid-rebalance state.

**Fix:** In `add_strategy()`, iterate existing strategy allocations and assert:
```rust
let total_existing: u8 = /* sum of stored strategy allocations */;
if total_existing + allocation_percent > 100 { return 3; }  // over-allocated
```

---

### NC-11 · MEDIUM · clawvault — `harvest()` silently skips yield collection without notification

**File:** [contracts/clawvault/src/lib.rs](contracts/clawvault/src/lib.rs)

`harvest()` performs cross-contract calls to LobsterLend and MoltSwap addresses loaded from storage keys (`LOBSTERLEND_ADDRESS_KEY`, `MOLTSWAP_ADDRESS_KEY`). If either address is not set (returns `None`), the harvest for that strategy silently skips. The function still returns `1` (success). Depositors have no way to detect that yield has not been collected.

This is additionally concerning because `harvest()` updates the vault's `total_assets` accounting based on what was actually collected — but if addresses are unset on first deployment (common during staged rollout), share prices will not reflect any accrued yield until addresses are configured, creating a discontinuous share price jump when harvest is finally called.

**Fix:** Return a non-zero error code (e.g. `7`) if any strategy's protocol address is unconfigured, or emit a distinct return-data payload distinguishing "partial harvest" from "full harvest".

---

### NC-12 · MEDIUM · compute_market — No pause function

**File:** [contracts/compute_market/src/lib.rs](contracts/compute_market/src/lib.rs)

Unlike all 14 other contracts in scope, `compute_market` has no `pause()`/`unpause()` functions and no pause guard on state-mutating entry points. An exploited admin key cannot halt the market, and there is no emergency response path.

**Fix:** Add:
```rust
#[no_mangle]
pub extern "C" fn cm_pause(caller_ptr: *const u8) -> u32 {
    // verify caller == owner, then storage_set(b"cm_paused", &[1u8])
}
```
Add `if storage_get(b"cm_paused").is_some_and(|v| v == [1]) { return 97; }` at the top of `submit_job`, `claim_job`, `complete_job`, `release_payment`, `dispute_job`, and `resolve_dispute`.

---

### NC-13 · LOW · clawpump — `transfer_molt_out` silently skips transfers when unconfigured

**File:** [contracts/clawpump/src/lib.rs](contracts/clawpump/src/lib.rs#L190)

```rust
fn transfer_molt_out(to: &[u8; 32], amount: u64) -> bool {
    let Some(token_addr) = get_molt_token_address() else {
        log_info("MOLT token not configured, skipping transfer");
        return true;   // BUG: reports success with no transfer
    };
    ...
}
```

When the MOLT token address is not configured, `sell()` calls this function, the function returns `true`, the seller's token balance is deducted from the bonding curve state, and no MOLT is sent. The seller loses their position with zero compensation. This was labeled "graceful degradation" but it is a silent fund loss path.

**Fix:** Return `false` when the token address is not configured:
```rust
let Some(token_addr) = get_molt_token_address() else {
    log_info("MOLT token not configured");
    return false;  // failure — caller will abort
};
```

---

### NC-14 · LOW · moltswap — Reputation bonus double-loads pool (TOCTOU smell)

**File:** [contracts/moltswap/src/lib.rs](contracts/moltswap/src/lib.rs#L399)

```rust
let out = compute_swap_out(&pool, amount_in, fee_bps);  // uses pool
pool.reserve_a = ...;  // mutates pool
save_pool(&pool);

// Later for reputation bonus:
let pool2 = load_pool();  // loads pool again from storage
let bonus = pool2.reserve_b * REP_BONUS_BPS / 10000;
```

`pool2` is loaded after the main swap has written updated reserves back to storage. The bonus calculation therefore reads the post-swap state, not the pre-swap state. Within the current single-transaction reentrancy model this is safe, but it introduces a subtle dependency on write-ordering that future refactors could silently break. The double-load is unnecessary.

**Fix:** Capture the bonus calculation before the `save_pool()` call, using the already-in-memory `pool` variable.

---

### NC-15 · LOW · lobsterlend — Block-time hard-coded at 400ms in interest accrual

**File:** [contracts/lobsterlend/src/lib.rs](contracts/lobsterlend/src/lib.rs)

`accrue_interest()` converts elapsed milliseconds to slots via integer division `/ 400`. This assumes a fixed 400ms block time. During network congestion, chain forks, or validator downtime, the actual ms-per-slot can exceed 400ms, causing interest to accrue more slowly than the model expects. This is a known limitation of slot-based interest models; the risk is low but should be documented.

**Recommendation:** Store and use `get_slot()` directly for interest accrual rather than converting from `get_timestamp()`.

---

### NC-16 · LOW · bountyboard — Submission count silently overflows at 255

**File:** [contracts/bountyboard/src/lib.rs](contracts/bountyboard/src/lib.rs)

The `submission_count` field in the bounty record is stored in byte 81 of the 91-byte layout (a `u8`). After 255 submissions, incrementing wraps to 0. The 256th submitter would overwrite submission slot 0, replacing the original earliest submission. There is no `MAX_SUBMISSIONS` guard in the current `submit_work()` path.

**Fix:**
```rust
if submission_count >= 255 { return 8; }  // max submissions reached
```

---

### NC-17 · LOW · clawpay — Silent failure when token or self address unset

**File:** [contracts/clawpay/src/lib.rs](contracts/clawpay/src/lib.rs)

`withdraw_from_stream()` and `cancel_stream()` call `call_token_transfer(token_addr, self_addr, recipient, amount)`. Both `token_addr` and `self_addr` are loaded via `get_token_address()` / `get_self_address()` which return `None` if storage key absent. If either is `None`, the transfer is skipped and the function may still return success, leaving the recipient's withdrawable balance decremented without receiving tokens.

**Fix:** Validate both addresses at the start of withdrawal/cancel:
```rust
let token_addr = get_token_address().ok_or(9)?;  // error code 9 = not initialized
let self_addr = get_self_address().ok_or(10)?;
```

---

### NC-18 · INFO · moltbridge — Identity gate defaults to allow when MoltyID unset

**File:** [contracts/moltbridge/src/lib.rs](contracts/moltbridge/src/lib.rs#L495)

`check_identity_gate()` is called in `lock_tokens()`. If the MoltyID contract address is not configured (`MOLTCOIN_ADDRESS_KEY` absent), the gate returns `Ok(())` — allowing all callers through. This is intentional for phased deployment but should be documented, and a flag to enable strict mode should be available.

---

### NC-19 · INFO · shielded_pool — `empty_merkle_root()` recomputes 32 SHA-256 rounds on every call

**File:** [contracts/shielded_pool/src/lib.rs](contracts/shielded_pool/src/lib.rs#L415)

`empty_merkle_root()` executes a 32-iteration loop of SHA-256 hashing on every invocation. This constant should be precomputed and stored as a `const [u8; 32]`.

---

## RPC Wiring Findings

### Confirmed Wired ✅

| Contract | JSON-RPC Methods |
|---|---|
| MoltyID | getMoltyIdIdentity, getMoltyIdReputation, getMoltyIdSkills, getMoltyIdVouches, getMoltyIdAchievements, getMoltyIdProfile, resolveMoltName, reverseMoltName, batchReverseMoltNames, searchMoltNames, getMoltyIdAgentDirectory, getMoltyIdStats, getNameAuction |
| MoltSwap | getMoltswapStats |
| LobsterLend | getLobsterLendStats |
| ClawPay | getClawPayStats |
| BountyBoard | getBountyBoardStats |
| ComputeMarket | getComputeMarketStats |
| mUSD | getMusdStats |
| ClawVault | getClawVaultStats |
| MoltBridge | getMoltBridgeStats, createBridgeDeposit, getBridgeDeposit, getBridgeDepositsByRecipient |
| MoltDAO | getMoltDaoStats |
| MoltOracle | getMoltOracleStats |
| PredictionMarket | getPredictionMarketStats, getPredictionMarkets, getPredictionMarket, getPredictionPositions, getPredictionTraderStats, getPredictionLeaderboard, getPredictionTrending, getPredictionMarketAnalytics |
| ShieldedPool | getShieldedPoolState, getShieldedMerkleRoot, getShieldedMerklePath, isNullifierSpent, getShieldedCommitments |
| ClawPump | REST: `/api/v1/launchpad/` (separate router) |

---

### RPC-01 · INFO — moltcoin has no dedicated RPC method

MoltCoin balances are only accessible via a generic `getTokenBalance` call. Add:
- `getMoltCoinBalance(address)` — returns balance in shells + formatted MOLT
- `getMoltCoinInfo()` — returns total supply, max supply, owner

---

### RPC-02 · INFO — ClawPump accessible only via REST, not JSON-RPC

ClawPump has a dedicated REST router at `/api/v1/launchpad/` but no JSON-RPC method. SDK clients using the standard JSON-RPC transport cannot query it. Add:
- `getClawPumpTokens(offset, limit)` — token list with bonding curve state
- `getClawPumpToken(token_id)` — individual token stats
- `getClawPumpGraduationInfo(token_id)` — graduation progress

---

### RPC-03 · INFO — MoltOracle has no price or attestation query endpoint

`getMoltOracleStats` returns aggregate usage counters only. Add:
- `getOraclePrice(asset)` — current price, timestamp, feeder
- `getOracleAttestation(hash)` — attestation details and verifier list
- `getOracleRandomness(request_id)` — VRF result

---

### RPC-04 · INFO — LobsterLend has no per-account position endpoint

`getLobsterLendStats` returns protocol-wide totals only. Add:
- `getLobsterLendPosition(address)` — deposit, borrow, health factor, interest owed

---

### RPC-05 · INFO — ClawPay has no per-stream query endpoint

`getClawPayStats` returns protocol-wide totals only. Add:
- `getStream(stream_id)` — stream details, withdrawable amount, cliff status
- `getStreamsByRecipient(address)` — all streams for a given recipient

---

## Per-Contract Scorecard

| Contract | A: Complete | B: Security | C: Token Math | D: State | E: RPC | F: Gaps |
|---|---|---|---|---|---|---|
| **moltcoin** | ✅ | ✅ | ✅ | ✅ | ⚠️ No dedicated RPC | — |
| **moltyid** | ✅ | ✅ | ✅ | ✅ | ✅ Full suite | — |
| **moltbridge** | ✅ | ✅ | ✅ | ✅ | ✅ | NC-18 |
| **moltdao** | ✅ | ✅ | ✅ | ⚠️ NC-09 | ✅ | — |
| **moltoracle** | ✅ | ⚠️ NC-08 | — | — | ⚠️ RPC-03 | NC-01 NC-02 |
| **moltswap** | ✅ | ⚠️ NC-14 | ✅ | ✅ | ✅ | — |
| **bountyboard** | ✅ | ⚠️ NC-16 | ✅ | ✅ | ✅ | — |
| **clawpay** | ✅ | ⚠️ NC-17 | ✅ | ✅ | ⚠️ RPC-05 | — |
| **clawpump** | ✅ | ⚠️ NC-13 | ✅ | ✅ | ⚠️ RPC-02 | — |
| **clawvault** | ⚠️ NC-11 | ⚠️ NC-10 | ✅ | ✅ | ✅ | NC-10 NC-11 |
| **lobsterlend** | ✅ | 🔴 NC-06 | ⚠️ NC-06 | ✅ | ⚠️ RPC-04 | NC-15 |
| **prediction_market** | ✅ | ✅ | ✅ | ✅ | ✅ Full suite | — |
| **shielded_pool** | ⚠️ NC-05 | 🔴 NC-03 NC-04 | N/A | 🔴 NC-07 | ✅ | NC-03–07 NC-19 |
| **compute_market** | ⚠️ NC-12 | ⚠️ NC-12 | ✅ | ✅ | ✅ | NC-12 |
| **musd_token** | ✅ | ✅ | ✅ | ✅ | ✅ | — |

---

## Prioritized Fix Order

### Must Fix Before Mainnet

1. **NC-01 + NC-02** (`moltoracle`) — One-line fix each. Oracle is broken by default. All price-dependent contracts fail.
2. **NC-03** (`shielded_pool`) — Add reentrancy guards. ~6 lines per function.
3. **NC-04** (`shielded_pool`) — Add caller == processor check. ~4 lines per function.
4. **NC-05** (`shielded_pool`) — Add pause mechanism. ~20 lines total.
5. **NC-06** (`lobsterlend`) — Cast through u128 in health factor. One-line fix.

### Fix Before Production Load

6. **NC-07** (`shielded_pool`) — Requires architectural change to per-key storage. High effort but necessary for any meaningful usage volume.
7. **NC-10** (`clawvault`) — Add allocation sum check. ~8 lines.
8. **NC-12** (`compute_market`) — Add pause infrastructure. ~30 lines.
9. **NC-09** (`moltdao`) — Correct constant. One-line fix; validate no truncated proposals exist.
10. **NC-13** (`clawpump`) — Change `return true` to `return false`. One-line fix.

### Fix When Convenient

11. **NC-08** (`moltoracle`) — Deprecate/remove legacy randomness function.
12. **NC-11** (`clawvault`) — Return error code on partial harvest.
13. **NC-14** (`moltswap`) — Code cleanup.
14. **NC-15** (`lobsterlend`) — Use slot-based accrual.
15. **NC-16** (`bountyboard`) — Add MAX_SUBMISSIONS guard.
16. **NC-17** (`clawpay`) — Return error on unset addresses.
17. **NC-18** (`moltbridge`) — Document or add strict-mode flag.
18. **NC-19** (`shielded_pool`) — Precompute constant.

---

*End of audit report. 15 contracts + RPC layer reviewed. Total findings: 5 CRITICAL, 1 HIGH, 5 MEDIUM, 5 LOW, 7 INFO.*
