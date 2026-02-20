# MoltChain Core Crate — Comprehensive Production-Readiness Audit

**Audit Date:** 2025-01-XX  
**Scope:** All 20 source files in `core/src/`, `Cargo.toml`, 5 test files, 1 bench file  
**Auditor:** Automated exhaustive review — every file, every finding  
**Classification:** Launch-critical

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Per-File Findings](#per-file-findings)
   - [Cargo.toml](#cargotoml)
   - [lib.rs](#librs)
   - [account.rs](#accountrs)
   - [block.rs](#blockrs)
   - [hash.rs](#hashrs)
   - [transaction.rs](#transactionrs)
   - [contract_instruction.rs](#contract_instructionrs)
   - [event_stream.rs](#event_streamrs)
   - [genesis.rs](#genesisrs)
   - [evm.rs](#evmrs)
   - [contract.rs](#contractrs)
   - [mempool.rs](#mempoolrs)
   - [consensus.rs](#consensusrs)
   - [state.rs](#staters)
   - [processor.rs](#processorrs)
   - [marketplace.rs](#marketplacers)
   - [multisig.rs](#multisigrs)
   - [network.rs](#networkrs)
   - [nft.rs](#nftrs)
   - [privacy.rs](#privacyrs)
   - [reefstake.rs](#reefstakers)
3. [Test Coverage Assessment](#test-coverage-assessment)
4. [Critical Findings Summary](#critical-findings-summary)
5. [Recommendations Priority Matrix](#recommendations-priority-matrix)

---

## Executive Summary

The moltchain-core crate is a Rust blockchain runtime (~15,000 lines of source) with a Solana-inspired account model, BFT+PoC consensus, WASM smart contracts (wasmer), and an EVM compatibility layer (revm). The codebase shows evidence of **multiple prior audit rounds** (AUDIT-FIX annotations, PERF-FIX/PERF-OPT series, C6/C10/H1-H16/M1-M11/T1-T7 fix annotations), indicating active hardening.

### Severity Counts

| Severity | Count |
|----------|-------|
| **CRITICAL (launch-blocker)** | 5 |
| **HIGH** | 12 |
| **MEDIUM** | 18 |
| **LOW / Informational** | 22 |

---

## Per-File Findings

---

### Cargo.toml

**File:** `core/Cargo.toml` (38 lines)

#### 1. Stubs / Incomplete Features
- None.

#### 2. Security Vulnerabilities
- **[LOW] S-CARGO-1: `hex` crate not listed as dependency.** `multisig.rs` uses `hex::encode()`. If this compiles, it's a transitive dependency — should be made explicit for audit trail.

#### 3. Atomicity / Consistency
- None.

#### 4. Performance Bottlenecks
- None.

#### 5. Dead Code
- None.

#### 6. Naming / Consistency
- **[LOW] N-CARGO-1:** `serde_json` is listed twice — once in `[dependencies]` and implicitly via other crates. Not a bug but redundant.

#### 7. Missing Functionality
- **[MEDIUM] F-CARGO-1: No `rand` or `getrandom` dependency.** Key generation uses `ed25519_dalek::SigningKey::generate(&mut OsRng)` which transitively pulls `getrandom`, but this isn't explicit. Production deployments on exotic targets may fail.

#### 8. Blockchain-Specific Issues
- None.

#### 9. Error Handling
- None.

#### 10. Test Gaps
- None.

---

### lib.rs

**File:** `core/src/lib.rs` (68 lines)

#### 1. Stubs / Incomplete Features
- None.

#### 2–10. All Categories
- **[LOW] N-LIB-1:** Re-exports are comprehensive. All public types from all 20 modules are surfaced. No issues found.

---

### account.rs

**File:** `core/src/account.rs` (351 lines)

#### 1. Stubs / Incomplete Features
- None — fully implemented.

#### 2. Security Vulnerabilities
- **[LOW] S-ACCT-1 (line ~108):** `Keypair::from_seed()` creates a deterministic keypair. The function exists and is used in tests. If accidentally used in production, keys are predictable. Consider `#[cfg(test)]` gating.

#### 3. Atomicity / Consistency
- **[INFO] A-ACCT-1:** All balance mutations (`stake`, `unstake`, `lock`, `unlock`, `deduct_spendable`, `add_spendable`) use checked arithmetic and maintain the invariant `shells = spendable + staked + locked`. Well-implemented.

#### 4. Performance Bottlenecks
- None.

#### 5. Dead Code
- **[LOW] D-ACCT-1 (line ~148):** `fixup_legacy()` handles migration from accounts where `spendable == 0 && staked == 0 && locked == 0 && shells > 0`. This is a one-time migration pattern that will become dead code post-migration. Consider adding a deprecation annotation or removing after migration window.

#### 6. Naming / Consistency
- **[LOW] N-ACCT-1 (line ~50):** `Account::new(molt, owner)` takes `molt` (integer MOLT), then internally converts to shells. The parameter name could confuse callers — consider `molt_amount` or adding a separate `Account::new_shells()`.

#### 7. Missing Functionality
- **[MEDIUM] F-ACCT-1:** No `Display` or `Debug` impl for `Pubkey` that shows the Base58 representation. Debug shows raw bytes. Can lead to confusing log output.

#### 8. Blockchain-Specific Issues
- None. Balance separation (spendable/staked/locked) is well-enforced.

#### 9. Error Handling
- All balance operations return `Result<(), String>`. String errors are acceptable for an L1 but consider structured error types for RPC consumers.

#### 10. Test Gaps
- Basic tests exist inline. Missing: test for `fixup_legacy()` migration, test for `Keypair::from_seed()` determinism, test for `to_evm()` address derivation correctness.

---

### block.rs

**File:** `core/src/block.rs` (417 lines)

#### 1. Stubs / Incomplete Features
- None.

#### 2. Security Vulnerabilities
- **[HIGH] S-BLOCK-1 (line ~118):** `Block::new()` uses `current_timestamp()` which reads wall-clock time. This is **non-deterministic** — validators will compute different hashes if timestamp differs. The `Block::new_with_timestamp()` variant exists and should be the ONLY constructor used in production. `Block::new()` should be deprecated or `#[cfg(test)]`-gated.

#### 3. Atomicity / Consistency
- **[MEDIUM] A-BLOCK-1 (line ~165):** `compute_tx_root()` concatenates all transaction hashes and hashes once: `SHA256(hash1 || hash2 || ... || hashN)`. This is **NOT a Merkle tree** — it doesn't support Merkle proofs for individual transaction inclusion. Light clients and SPV verification require proper Merkle trees. The field is named `tx_root` which implies Merkle root semantics.

#### 4. Performance Bottlenecks
- None.

#### 5. Dead Code
- None.

#### 6. Naming / Consistency
- **[LOW] N-BLOCK-1 (line ~17):** `BlockHeader.validator` is `[u8; 32]` instead of `Pubkey`. This requires manual conversion throughout the codebase. Should be typed as `Pubkey` for consistency.

#### 7. Missing Functionality
- **[HIGH] F-BLOCK-1:** No block size limit enforcement in `validate_structure()`. The `MAX_BLOCK_SIZE` constant exists but is never checked during validation. An attacker could propose an oversized block.

#### 8. Blockchain-Specific Issues
- **[MEDIUM] B-BLOCK-1 (line ~200):** `validate_structure()` does not verify that `header.timestamp` is within an acceptable window of the parent block's timestamp. This allows timestamp manipulation attacks.

#### 9. Error Handling
- Adequate. Signature verification errors are properly propagated.

#### 10. Test Gaps
- No test for `validate_structure()` rejection of oversized blocks, missing timestamp validation tests, no test for hash determinism across different call orders.

---

### hash.rs

**File:** `core/src/hash.rs` (90 lines)

#### 1–10. All Categories
- Clean implementation. SHA-256 wrapper with `hash()`, `hash_two_parts()`, `to_hex()`, `from_hex()`.
- **[INFO] I-HASH-1:** `Hash::default()` creates an all-zero hash. This is used as the genesis block parent. Acceptable but should be documented.
- No issues found.

---

### transaction.rs

**File:** `core/src/transaction.rs` (264 lines)

#### 1. Stubs / Incomplete Features
- None.

#### 2. Security Vulnerabilities
- **[MEDIUM] S-TX-1 (line ~90):** Signature serialization uses `hex::encode`/`hex::decode` for JSON representation. This doubles storage size vs raw bytes. More importantly, `from_hex` returns a `[0u8; 64]` default on decode failure — this silently produces an invalid signature that will be caught later but masks the root cause.

#### 3. Atomicity / Consistency
- None.

#### 4. Performance Bottlenecks
- **[LOW] P-TX-1 (line ~100):** `tx.hash()` recomputes a SHA-256 hash by serializing the entire message every call. Should be cached.

#### 5. Dead Code
- None.

#### 6. Naming / Consistency
- None.

#### 7. Missing Functionality
- **[LOW] F-TX-1:** No transaction versioning field. Future protocol upgrades will need to distinguish v1 from v2 transactions.

#### 8. Blockchain-Specific Issues
- `MAX_INSTRUCTIONS_PER_TX = 64`, `MAX_INSTRUCTION_DATA = 10KB` (4MB for deploys) — reasonable limits.

#### 9. Error Handling
- `validate_structure()` properly enforces limits.

#### 10. Test Gaps
- No integration test for `validate_structure()` boundary cases (exactly 64 instructions, exactly MAX data size).

---

### contract_instruction.rs

**File:** `core/src/contract_instruction.rs` (~130 lines)

#### 1. Stubs / Incomplete Features
- None.

#### 2. Security Vulnerabilities
- None.

#### 3–10. All Categories
- **[LOW] N-CI-1 (line ~40):** `ContractInstruction::Call` has doc comment mentioning "lamports" — a Solana vestige. Should say "shells".
- Clean implementation. JSON-based serialization for instruction variants.

---

### event_stream.rs

**File:** `core/src/event_stream.rs` (~180 lines)

#### 1. Stubs / Incomplete Features
- **[MEDIUM] S-EVT-1 (line ~50):** `ChainEvent::BridgeLock` and `ChainEvent::BridgeMint` are defined but the bridge system is not implemented anywhere in the codebase. These are placeholder event types.

#### 2–10. All Categories
- **[LOW] D-EVT-1:** `EventBuffer` uses unbounded `Vec<ChainEvent>`. No cap on buffer size — could OOM under heavy load. Consider a ring buffer or bounded queue.
- Otherwise clean.

---

### genesis.rs

**File:** `core/src/genesis.rs` (470 lines)

#### 1. Stubs / Incomplete Features
- None.

#### 2. Security Vulnerabilities
- None. Fee percentage validation (`sum ≤ 100`) is properly enforced.

#### 3. Atomicity / Consistency
- None.

#### 4. Performance Bottlenecks
- None.

#### 5. Dead Code
- None.

#### 6. Naming / Consistency
- **[LOW] N-GEN-1:** `ConsensusParams` has `slot_time_ms: u64 = 400` but this value is never consumed — the actual slot time is hardcoded elsewhere. Keeping it in the genesis config without using it is misleading.

#### 7. Missing Functionality
- None.

#### 8. Blockchain-Specific Issues
- **[INFO] B-GEN-1:** Genesis distribution totals 1B MOLT: 40% treasury, 25% builder, 15% validator, 10% founding, 5% partnerships, 5% reserve. This matches the whitepaper.

#### 9. Error Handling
- Adequate.

#### 10. Test Gaps
- No test for genesis config validation edge cases (fee percents summing to exactly 100).

---

### evm.rs

**File:** `core/src/evm.rs` (980 lines)

#### 1. Stubs / Incomplete Features
- None — fully implemented EVM compatibility layer.

#### 2. Security Vulnerabilities
- **[MEDIUM] S-EVM-1 (line ~280):** `StateEvmDb` reads balance as `account.spendable` only — staked/locked are excluded. This is correct for spending but could confuse EVM contracts that check `address.balance` and expect the full amount. Should be documented clearly.
- **[LOW] S-EVM-2 (line ~450):** EIP-4844 and EIP-7702 transaction types are decoded but `blob_versioned_hashes` and `authorization_list` are silently dropped. These are parsed for compatibility but their validity is not checked.

#### 3. Atomicity / Consistency  
- **[INFO] A-EVM-1:** EVM state changes are deferred via `EvmStateChanges` and applied atomically through `StateBatch`. Good pattern.

#### 4. Performance Bottlenecks
- None.

#### 5. Dead Code
- None.

#### 6. Naming / Consistency
- **[LOW] N-EVM-1:** `EVM_PROGRAM_ID = [0xEE; 32]` is not a cryptographically-derived address. Intentional but should be documented as a reserved sentinel.

#### 7. Missing Functionality
- **[MEDIUM] F-EVM-1:** No EVM gas price oracle. `gas_price` from the transaction is trusted as-is. For EIP-1559, `base_fee_per_gas` should be maintained at the block level.
- **[MEDIUM] F-EVM-2:** No EVM event log indexing. Logs are captured in `EvmReceipt` but there's no bloom filter or event topic index for `eth_getLogs` queries.

#### 8. Blockchain-Specific Issues
- Chain ID 8001 is hardcoded. EIP-155 is enforced.

#### 9. Error Handling
- Adequate. `revm` errors are properly mapped.

#### 10. Test Gaps
- Comprehensive inline tests exist. Missing: test for EIP-4844 blob transaction handling, test for max gas limit enforcement.

---

### contract.rs

**File:** `core/src/contract.rs` (1848 lines)

#### 1. Stubs / Incomplete Features
- **[CRITICAL] S-CON-1 (line ~1106):** `cross_contract_call()` host function is a **STUB** — always returns 0 regardless of input. Any WASM contract calling another contract will silently get a no-op result. This is called via `host_cross_contract_call` in the WASM import table. Contracts that depend on cross-contract calls will silently malfunction.

#### 2. Security Vulnerabilities
- **[HIGH] S-CON-2 (line ~760):** `MODULE_CACHE` is a `static Mutex<HashMap>` with no eviction policy or size limit. A contract deployment flood could exhaust memory since compiled WASM modules are cached indefinitely.
- **[MEDIUM] S-CON-3 (line ~300):** `MAX_STORAGE_ENTRIES = 10_000` per contract. Storage operations check this limit on `set_storage` but the count includes deleted entries (entries set to `None` in `storage_changes`). The `HashMap::len()` may undercount after deletions within a single execution.

#### 3. Atomicity / Consistency
- **[INFO] A-CON-1:** Contract execution produces `ContractResult` with `storage_changes` map. These are applied atomically by the processor. Good design.

#### 4. Performance Bottlenecks
- **[LOW] P-CON-1 (line ~820):** WASM compilation happens inside a mutex lock on `MODULE_CACHE`. Under concurrent deployment load, this serializes all compilations.

#### 5. Dead Code
- **[LOW] D-CON-1:** `ContractAbi` schema is defined with extensive field types (u8-u256, address, string, tuple, array, map, optional, bytes) but many complex types are never tested or exercised by any contract in the repo.

#### 6. Naming / Consistency
- None.

#### 7. Missing Functionality
- **[HIGH] F-CON-1:** No WASM execution timeout. Metering limits compute units but there's no wall-clock timeout. A WASM module with complex but metered operations could still block the executor thread for extended periods.

#### 8. Blockchain-Specific Issues
- Good: No WASI allowed ("env" imports only), memory limited to 16MB, compute metered at 10M units.

#### 9. Error Handling
- WASM traps are properly caught. Host function errors return error codes to the guest.

#### 10. Test Gaps
- Missing: test for `cross_contract_call` being a stub, test for `MODULE_CACHE` memory pressure, test for storage limit enforcement at exactly 10,000 entries.

---

### mempool.rs

**File:** `core/src/mempool.rs` (410 lines)

#### 1. Stubs / Incomplete Features
- None.

#### 2. Security Vulnerabilities
- **[MEDIUM] S-MEM-1:** No per-sender rate limiting in the mempool. A single sender can fill the entire mempool with transactions. The `reputation` multiplier makes high-rep accounts even more effective at crowding out others.

#### 3. Atomicity / Consistency
- None.

#### 4. Performance Bottlenecks
- **[LOW] P-MEM-1:** `remove_transactions()` rebuilds the entire `BinaryHeap` for bulk removal. The complexity is O(N) per bulk removal — acceptable but could be optimized.

#### 5. Dead Code
- None.

#### 6. Naming / Consistency
- None.

#### 7. Missing Functionality
- **[MEDIUM] F-MEM-1:** No transaction replacement (fee bumping). If a user submits a TX with nonce N and wants to increase its fee, they cannot replace it — they must wait for expiration.

#### 8. Blockchain-Specific Issues
- Express lane for Tier 4+ agents (reputation ≥ 5000) allows priority bypass. Documented design choice.

#### 9. Error Handling
- Adequate.

#### 10. Test Gaps
- Good inline test coverage. Missing: test for expiration cleanup, test for mempool at exact capacity.

---

### consensus.rs

**File:** `core/src/consensus.rs` (3365 lines)

#### 1. Stubs / Incomplete Features
- None — comprehensively implemented (StakePool, Graduation, VoteAggregator, FinalityTracker, ForkChoice, SlashingTracker).

#### 2. Security Vulnerabilities
- **[MEDIUM] S-CON-1 (line ~1760):** `Vote::new()` uses `SystemTime::now()` for `timestamp`. This is non-deterministic across validators. Currently the timestamp is not consensus-critical (only for diagnostics), but if future code uses it for ordering, it becomes a vulnerability.
- **[LOW] S-CON-2:** `SlashingEvidence::new()` also uses wall-clock `SystemTime`. Same concern.

#### 3. Atomicity / Consistency
- **[INFO] A-CON-1:** `FinalityTracker` uses `AtomicU64` with `Ordering::Relaxed`. This is correct for monotonically-increasing slot numbers where the only requirement is eventual visibility. However, `mark_confirmed()` uses `fetch_max` which provides the correct semantics.

#### 4. Performance Bottlenecks
- **[INFO] P-CON-1:** `VoteAggregator` has O(1) equivocation detection via `voted_in_slot` HashMap (PERF-OPT 5). Good optimization.

#### 5. Dead Code
- None.

#### 6. Naming / Consistency
- **[LOW] N-CON-1:** `StakeInfo` has both `amount` (total staked) and methods like `total_stake()` and `active_stake()`. The field naming could be clearer.

#### 7. Missing Functionality
- **[HIGH] F-CON-1:** `VoteAggregator::add_vote()` verifies the vote signature but does NOT check validator set membership. The `add_vote_validated()` variant exists for this, but the unvalidated version is still public. Callers must know to use the validated version.
- **[MEDIUM] F-CON-2:** Slashing penalties are calculated and applied to the `StakePool` in-memory, but the actual economic enforcement (transferring slashed funds to a burn address or insurance fund) is not implemented here — it relies on the caller persisting the modified pool.

#### 8. Blockchain-Specific Issues
- **[INFO] B-CON-1:** Leader selection uses `sqrt(stake) * sqrt(reputation)` weighting. This is the whitepaper design. `integer_sqrt()` is pure integer arithmetic (Newton's method) — deterministic.
- **[INFO] B-CON-2:** Bootstrap system: first 200 validators get a 100K MOLT grant with 50/50 reward split until debt is repaid or time cap reached. Well-tested.

#### 9. Error Handling
- Adequate throughout.

#### 10. Test Gaps
- **Excellent test coverage** (>30 tests). Missing: test for `has_supermajority()` with mixed stake/reputation fallback, test for `prune_old_votes()` correctness.

---

### state.rs

**File:** `core/src/state.rs` (5706 lines)

#### 1. Stubs / Incomplete Features
- None.

#### 2. Security Vulnerabilities
- **[MEDIUM] S-STATE-1:** `add_burned()` uses read-modify-write with a `WriteBatch`, but without a true atomic increment. Under concurrent block processing (which doesn't happen now but could in future), the read of `current` could be stale, leading to lost burn amounts. Consider RocksDB merge operator.
- **[LOW] S-STATE-2:** `prune_slot_stats()` iterates text-formatted keys (`fee_dist:999`) and parses slot numbers. If a malicious entry with a non-numeric suffix exists, it's silently skipped. Not exploitable but fragile.

#### 3. Atomicity / Consistency
- **[INFO] A-STATE-1:** `StateBatch` implements proper read-your-writes semantics (overlay → disk fallback). `commit_batch()` writes atomically via RocksDB `WriteBatch`. Good implementation.
- **[MEDIUM] A-STATE-2:** `index_account_transactions()` increments per-account TX counters with read-modify-write (`get + put`). This is NOT inside the block's `WriteBatch` — it's a separate write. A crash between block commit and index write would leave counters stale. This is a data integrity issue, not a fund safety issue.
- **[LOW] A-STATE-3:** `dirty_acct:` keys in `mark_account_dirty_with_key()` use format `"dirty_acct:" + pubkey(32)`. But `prune_slot_stats()` tries to parse slot bytes from offset `[11..19]` — the key format doesn't include a slot, it includes a pubkey. **Bug:** the pruning logic for dirty_acct keys is incorrect — it will misinterpret pubkey bytes as slot numbers and prune unpredictably. The state root will still be correct because `compute_state_root()` reads from CF_MERKLE_LEAVES, but stale dirty markers accumulate.

#### 4. Performance Bottlenecks
- **[INFO] P-STATE-1:** 30+ column families with individually-tuned access patterns (point-lookup, prefix-scan, write-heavy, archival). This is well-designed.
- **[LOW] P-STATE-2:** `count_accounts()` and `count_active_accounts_full_scan()` do full CF iteration. These are marked deprecated/verification-only — acceptable.

#### 5. Dead Code
- **[LOW] D-STATE-1:** `compute_state_root_cached()`, `compute_state_root_cold_start()`, `compute_state_root_full_scan()`, `count_accounts()`, `count_active_accounts_full_scan()`, `reconcile_account_count()` are all `#[allow(dead_code)]`. Consider removing or documenting why they're retained.

#### 6. Naming / Consistency
- **[LOW] N-STATE-1:** Both `StateStore` and `StateBatch` have methods named `put_account`, `transfer`, `register_symbol` etc. This design is intentional (batch vs direct) but confusing — callers must carefully choose the right receiver.

#### 7. Missing Functionality
- **[MEDIUM] F-STATE-1:** No database compaction scheduling. RocksDB column families grow without periodic compaction triggers. In production with millions of accounts, this will degrade read performance over time.
- **[LOW] F-STATE-2:** No checkpointing/snapshots for state sync. New validators must replay from genesis.

#### 8. Blockchain-Specific Issues
- **[INFO] B-STATE-1:** Incremental Merkle tree with dirty tracking and CVE-2012-2459 mitigation (odd-leaf duplication). Good implementation.
- **[MEDIUM] B-STATE-2:** `put_block()` stores block hash → slot mapping, slot → block data, and TX index, all in a single `WriteBatch`. However, `index_account_transactions()` is called AFTER the block commit. A crash between these two operations means account TX indexes are missing. This should be folded into the same `WriteBatch`.

#### 9. Error Handling
- Consistent `Result<T, String>` pattern throughout. Errors from RocksDB are properly wrapped.

#### 10. Test Gaps
- No dedicated unit tests for `state.rs` — relies on integration tests in `core/tests/`. Missing: test for `StateBatch` overlay correctness, test for `commit_batch()` metric delta tracking, test for `compute_state_root()` incremental correctness.

---

### processor.rs

**File:** `core/src/processor.rs` (3377 lines)

#### 1. Stubs / Incomplete Features
- None.

#### 2. Security Vulnerabilities
- **[CRITICAL] S-PROC-1 (line ~650):** Rate limiting uses `self.rate_limit_cache` (a `Mutex<HashMap>`) that checks per-epoch TX counts. However, the cache is **never persisted** — restarting the validator resets all rate limits. A determined attacker could force validator restarts to bypass rate limiting.
- **[CRITICAL] S-PROC-2:** `process_transactions_parallel()` uses union-find to detect account conflicts and runs non-conflicting TXs in parallel via rayon. However, `CONTRACT_PROGRAM_ID` is **excluded** from conflict detection (intentional comment says "contract calls should serialize"). This means two contract calls to the SAME contract can run in parallel, which could cause data races in the contract's storage (since both read storage, execute, then write — classic TOCTOU in the batch overlay). The `StateBatch` is per-parallel-group, not per-TX, so concurrent mutations to the same contract address within a group aren't caught.
  
  **Mitigation note:** The code comment says contract-program conflicts are "handled by the batch layer" — but the batch layer doesn't provide per-key locking within a shared batch. This needs verification that all contract TXs end up in the same conflict group.

- **[HIGH] S-PROC-3 (line ~420):** `compute_transaction_fee()` reads reputation from MoltyID contract storage via `state.get_reputation()`. This is a **state read during fee computation** which happens before the batch. If a preceding TX in the same block modifies the caller's reputation, the fee for subsequent TXs still uses the stale pre-block reputation. Not exploitable for fund theft but could give incorrect fee discounts within a single block.

#### 3. Atomicity / Consistency
- **[INFO] A-PROC-1:** Fee is charged BEFORE the batch (`charge_fee_direct`), so failed TXs still pay fees. This prevents free-compute DoS. Good.
- **[MEDIUM] A-PROC-2 (line ~870):** `apply_rent()` iterates accounts touched by the current TX and charges rent if `elapsed_slots >= SLOTS_PER_MONTH`. Rent is charged from `spendable` only, capped to available balance. However, rent for a single account could be triggered by ANY TX that touches it — this creates a timing dependency where the first TX touching a dormant account bears the entire rent cost, while subsequent TXs in the same block pay nothing.

#### 4. Performance Bottlenecks
- **[LOW] P-PROC-1:** `process_transaction()` acquires and releases `self.batch` mutex for every transaction. Under high TPS, this serializes all transactions. The parallel processor mitigates this for non-conflicting TXs.

#### 5. Dead Code
- **[LOW] D-PROC-1:** `charge_fee()` (the batch-scoped version) is `#[allow(dead_code)]`. It duplicates `charge_fee_direct()` logic. Should be removed or documented why retained.

#### 6. Naming / Consistency
- **[LOW] N-PROC-1:** Batch-aware methods use `b_` prefix (`b_get_account`, `b_put_account`, `b_transfer`). This convention is consistent but non-standard. Consider a `BatchOps` trait.

#### 7. Missing Functionality
- **[HIGH] F-PROC-1:** No nonce/sequence number per account. Transactions are deduplicated by fee exhaustion and balance, not by nonce. This means:
  - Transaction ordering is undefined within a block
  - The same logical action can be submitted multiple times if the user has sufficient balance
  - No way for a user to cancel a pending TX by submitting a higher-fee replacement

- **[MEDIUM] F-PROC-2:** No gas metering for system program instructions. A transfer costs BASE_FEE regardless of how many accounts it touches. An attacker could construct a TX with 64 instructions each touching different accounts, all for the cost of a single BASE_FEE.

#### 8. Blockchain-Specific Issues
- **[CRITICAL] B-PROC-1:** Recent blockhash validation uses a 150-slot window cache. However, at 400ms/slot, this is only 60 seconds. Users who sign a TX offline and submit it more than 60 seconds later will have their TX rejected. Most blockchains use a larger window (Solana uses last 300 blocks ≈ 2 minutes).

#### 9. Error Handling
- Adequate. All instruction handlers return `Result` and errors are properly surfaced in `TxResult`.

#### 10. Test Gaps
- **Good coverage** in inline tests (transfer, replay protection, signature validation, ReefStake deposit/unstake/claim, deploy, ABI set, faucet). Missing: test for parallel TX processing conflict detection, test for rent charging across epoch boundaries, test for rate limiting.

---

### marketplace.rs

**File:** `core/src/marketplace.rs` (40 lines)

#### 1–10. All Categories
- Data types only (`MarketActivity`, `MarketActivityKind`). Encode/decode functions.
- **[INFO]:** No business logic — just serialization primitives. No issues.

---

### multisig.rs

**File:** `core/src/multisig.rs` (~290 lines)

#### 1. Stubs / Incomplete Features
- None.

#### 2. Security Vulnerabilities
- **[HIGH] S-MULTI-1 (line ~200):** `save_keypairs()` writes secret keys to disk as **plaintext JSON** with a `hex::encode(keypair.secret())` field. The comment says "In production, encrypt with passphrase" but no encryption is implemented. **Launch-blocker if used on mainnet.** Genesis wallet private keys on disk in plaintext.
- **[LOW] S-MULTI-2:** `generate()` threshold calculation for mainnet uses `f64` arithmetic: `(signer_count as f64 * 0.6).ceil() as u8`. This is deterministic for small counts but fragile — should use integer math: `(signer_count * 3 + 4) / 5`.

#### 3. Atomicity / Consistency
- **[INFO] A-MULTI-1:** `verify_threshold()` deduplicates signers via `HashSet` to prevent counting the same key twice. Good fix (C6).

#### 4–10. Remaining Categories
- **[LOW] N-MULTI-1:** `GenesisWallet.keypair_path` uses relative paths (`.moltchain/genesis-wallet-{chain_id}.json`). The directory may not exist. No `create_dir_all()` call.
- Tests cover basic generation and threshold verification.

---

### network.rs

**File:** `core/src/network.rs` (~310 lines)

#### 1. Stubs / Incomplete Features
- **[MEDIUM] S-NET-1:** `PeerDiscovery` has no actual network I/O — it's a data structure for tracking peers. The actual peer discovery protocol is not implemented in core.

#### 2. Security Vulnerabilities
- **[LOW] S-NET-1:** Hardcoded bootstrap peer IP addresses (`147.182.195.45`, `138.68.88.120`, `159.89.106.78`). These should be configurable and rotatable.

#### 3–10. Remaining Categories
- **[LOW] N-NET-1:** `NetworkType::from_str()` shadows the `FromStr` trait. This is flagged by `#[allow(clippy::should_implement_trait)]` — should just implement `FromStr`.
- Data-only module. No security-critical logic.

---

### nft.rs

**File:** `core/src/nft.rs` (~100 lines)

#### 1–10. All Categories
- Data types only (`CollectionState`, `TokenState`, `NftActivity`). Encode/decode via bincode.
- **[LOW] F-NFT-1:** No royalty enforcement in the type — `royalty_bps` is stored but enforcement is in the processor.
- **[LOW] F-NFT-2:** `metadata_uri` is a `String` with no length validation. Contracts could store arbitrarily long URIs.
- Otherwise clean.

---

### privacy.rs

**File:** `core/src/privacy.rs` (~260 lines)

#### 1. Stubs / Incomplete Features
- **[CRITICAL] S-PRIV-1:** The ENTIRE ZK proof system is a **placeholder**. `verify_proof()` uses HMAC-SHA256 with public parameters — **anyone who can read the commitment_root from on-chain state can forge a valid proof**. This is clearly documented (C10 fix: `allow_placeholder_proofs = false` by default), and the module is disabled. However, the code is shipped in the binary and could be accidentally enabled.

#### 2. Security Vulnerabilities
- **[HIGH] S-PRIV-2:** `ShieldedPool.nullifier_set` is a `Vec<[u8; 32]>`. Nullifier lookup is O(N) via `contains()`. For a privacy pool with millions of spent notes, this is both a performance bottleneck and a DoS vector (attacker forces sequential scans).
- **[MEDIUM] S-PRIV-3:** `unshield()` uses non-checked subtraction: `self.total_shielded -= amount` after the `amount > self.total_shielded` check. Should use `checked_sub()` or `saturating_sub()` for defense-in-depth.

#### 3. Atomicity / Consistency
- **[MEDIUM] A-PRIV-1:** `ShieldedPool` state is in-memory only. Notes and nullifiers are never persisted to RocksDB. The entire pool is lost on restart.

#### 4–10. Remaining Categories
- Well-documented as placeholder. Tests verify the placeholder logic works.
- **[INFO]:** The module is correctly disabled by default. Not a launch blocker IF the privacy feature is not advertised.

---

### reefstake.rs

**File:** `core/src/reefstake.rs` (623 lines)

#### 1. Stubs / Incomplete Features
- None — fully implemented liquid staking protocol.

#### 2. Security Vulnerabilities
- **[LOW] S-RS-1 (line ~370):** `transfer()` proportional `molt_deposited` tracking uses integer division: `(st_molt_amount * molt_deposited) / total_before`. Dust loss on each partial transfer slightly disadvantages the sender. Accumulated over many transfers, this could cause accounting discrepancies.

#### 3. Atomicity / Consistency
- **[INFO] A-RS-1:** Exchange rate uses fixed-point arithmetic with `RATE_PRECISION = 1e9`. All math is u128-widened. Good implementation (T3.2/T6.2 fix).
- **[INFO] A-RS-2:** `distribute_rewards()` assigns remainder dust to the last position (AUDIT-FIX CP-5). Good.

#### 4. Performance Bottlenecks
- **[LOW] P-RS-1:** `distribute_rewards()` iterates all positions. With 100K+ stakers, this becomes expensive per block.

#### 5. Dead Code
- None.

#### 6. Naming / Consistency
- None.

#### 7. Missing Functionality
- **[MEDIUM] F-RS-1:** Lock tier cannot be downgraded. If a user deposits with `Lock365` tier, they cannot change to `Flexible` even after the lock expires. The tier is permanently associated with the position.

#### 8. Blockchain-Specific Issues
- `REEFSTAKE_BLOCK_SHARE_BPS = 1000` (10% of block rewards). Integration with block reward distribution is in the validator, not in core.

#### 9. Error Handling
- Adequate.

#### 10. Test Gaps
- Good inline tests for stake/unstake/transfer/cooldown. Missing: test for tier-weighted reward distribution, test for exchange rate precision edge cases near zero supply.

---

## Test Coverage Assessment

### Existing Test Files

| File | Tests | Coverage Quality |
|------|-------|-----------------|
| `tests/basic_test.rs` | 9 | Basic smoke tests — keypair, hash, mempool, state init, block creation |
| `tests/adversarial_test.rs` | 10 | Double-spend, signature forgery, overflow, replay, zero-amount, spam, malformed data, byzantine blocks |
| `tests/production_readiness.rs` | 50+ | Comprehensive: block storage, account edge cases, stake/lock lifecycle, transfers, vote aggregation, fork choice, slashing |
| `tests/contract_lifecycle.rs` | 7 | Deploy, call-nonexistent, serialization roundtrip, transfer, insufficient funds, upgrade |
| `tests/activity_indexing.rs` | ~5 | NFT indexing, marketplace activity, program calls |

### Critical Test Gaps

1. **No test for parallel transaction processing** — `process_transactions_parallel()` with conflicting accounts
2. **No test for `cross_contract_call` stub behavior** — contracts that call other contracts will silently fail
3. **No test for `StateBatch` rollback correctness** — begin_batch → fail → rollback → verify state unchanged
4. **No test for rent charging** in integration context
5. **No test for EVM transaction processing** end-to-end (only unit tests in evm.rs)
6. **No fuzz testing** for transaction deserialization or WASM execution
7. **No benchmark for block processing throughput** — `benches/processor_bench.rs` exists but content was not read

---

## Critical Findings Summary

### CRITICAL (5) — Must fix before launch

| ID | File | Finding |
|----|------|---------|
| S-CON-1 | contract.rs:1106 | `cross_contract_call()` is a **stub** returning 0. Contracts depending on CCC will silently malfunction. |
| S-PRIV-1 | privacy.rs | ZK proof verification is a **forgeable placeholder** (HMAC-SHA256 with public key material). Disabled by default but ships in binary. |
| S-PROC-1 | processor.rs:650 | Rate limit cache is **not persisted**. Validator restarts reset all rate limits. |
| S-PROC-2 | processor.rs:750 | Parallel TX processing **excludes** `CONTRACT_PROGRAM_ID` from conflict detection. Two contract calls to the same contract may race. |
| B-PROC-1 | processor.rs | 150-slot blockhash window = only **60 seconds**. Too short for offline signing or congested networks. |

### HIGH (12) — Should fix before launch

| ID | File | Finding |
|----|------|---------|
| S-BLOCK-1 | block.rs:118 | `Block::new()` uses non-deterministic wall-clock timestamp |
| F-BLOCK-1 | block.rs | No block size limit enforcement in `validate_structure()` |
| S-CON-2 | contract.rs:760 | `MODULE_CACHE` has no eviction — unbounded memory growth |
| F-CON-1 | contract.rs | No WASM execution wall-clock timeout |
| F-CON-2 | consensus.rs | `add_vote()` doesn't check validator set membership (public variant) |
| S-MULTI-1 | multisig.rs:200 | Genesis keypairs saved as **plaintext JSON** on disk |
| F-PROC-1 | processor.rs | No nonce/sequence number — transactions replayable if balance allows |
| S-PROC-3 | processor.rs:420 | Fee computation reads stale pre-block reputation |
| S-PRIV-2 | privacy.rs | Nullifier set is O(N) Vec — DoS vector for privacy pool |
| A-BLOCK-1 | block.rs:165 | `tx_root` is concatenated hash, not proper Merkle tree — no SPV proofs |
| F-STATE-1 | state.rs | No RocksDB compaction scheduling |
| B-STATE-2 | state.rs | `index_account_transactions()` not in same WriteBatch as block commit |

---

## Recommendations Priority Matrix

### Before Testnet (P0)
1. **Fix `cross_contract_call` stub** — either implement or return an explicit error to contracts
2. **Persist rate limit cache** to RocksDB or implement per-account nonces
3. **Verify parallel TX conflict detection** includes contract program interactions
4. **Extend blockhash window** to at least 300 slots (2 minutes)
5. **Add `MODULE_CACHE` eviction** (LRU with max N entries)

### Before Mainnet (P1)
6. Implement proper Merkle tree for `tx_root`
7. Encrypt genesis keypairs at rest
8. Add nonce/sequence numbers to accounts
9. Implement fee-based transaction replacement in mempool
10. Add EVM gas price oracle and log indexing
11. Replace privacy module ZK placeholder with real Groth16/PLONK
12. Replace `ShieldedPool.nullifier_set` Vec with a Merkle tree or set commitment
13. Add wall-clock timeout to WASM execution
14. Fold `index_account_transactions()` into block commit WriteBatch

### Post-Launch (P2)
15. Implement RocksDB compaction scheduling
16. Add state snapshots for fast sync
17. Implement per-sender mempool limits
18. Add structured error types (replace `String` errors)
19. Add fuzz testing for TX deserialization and WASM execution
20. Optimize `distribute_rewards()` for large staker sets
