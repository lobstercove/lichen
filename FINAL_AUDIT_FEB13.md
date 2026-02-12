# MoltChain Final Code Audit — February 13, 2026

**Scope**: All 7 crates (core, validator, rpc, p2p, cli, faucet, custody), ~39K lines of Rust  
**Baseline**: 301/301 tests passing, 3 prior audits (188 issues fixed)  
**Method**: Line-by-line review of every `.rs` source file, cross-referenced against prior FIX_PLAN, FIX_PLAN_V2, and PRODUCTION_AUDIT_COMPLETE

---

## Executive Summary

| Severity | Count | Status |
|----------|-------|--------|
| CRITICAL | 10 | New — must fix before mainnet |
| HIGH     | 18 | New — should fix before mainnet |
| MEDIUM   | 20 | New — fix or accept with justification |
| LOW      | 7  | Informational / low risk |
| **Total** | **55** | |

Previous audits resolved 188 issues. This final pass found **55 new issues** missed by earlier reviews, predominantly in cross-component interactions, serialization consistency, and economic invariant enforcement.

---

## CRITICAL — 10 Findings

### C1. Non-deterministic voter fee distribution → consensus divergence
**File**: [validator/src/main.rs](validator/src/main.rs#L1122-L1128)  
**Category**: Consensus Safety  
**Description**: Voter pubkeys are collected via `HashSet::into_iter()` which has **randomized iteration order** (Rust's hash seed differs per process). When dividing `voters_share` by stake weight, the last voter gets rounding remainder. Different validators assign the remainder to different voters → different state roots for the same block.  
**Impact**: Every block with >1 voter causes silent state divergence across all validators. Fundamental consensus invariant violated.  
**Fix**: Sort `voter_pubkeys` deterministically by pubkey bytes before distributing.

### C2. Epoch reward over-distribution when delegations exist
**File**: [core/src/consensus.rs](core/src/consensus.rs#L796-L816)  
**Category**: Economic / Inflation  
**Description**: `distribute_epoch_rewards()` uses `active_stake()` (= `total_staked`) as denominator but `stake_info.total_stake()` (= `amount + delegated_amount`) as numerator. The `delegate()` function at [L607](core/src/consensus.rs#L607) increments `delegated_amount` but **never** adds it to `total_staked`. Sum of individual shares can exceed 100% of reward pool.  
**Impact**: Unbounded reward inflation. Every delegation amplifies rewards beyond the intended emission schedule.  
**Fix**: `delegate()` must add `amount` to `total_staked`; `undelegate()` must subtract it.

### C3. EVM batch serialization mismatch — data corruption
**File**: [core/src/state.rs](core/src/state.rs#L2511-L2534) (write) vs [core/src/state.rs](core/src/state.rs#L3591-L3615) (read)  
**Category**: Data Corruption  
**Description**: `StateBatch::put_evm_tx` serializes with `serde_json`, but `StateStore::get_evm_tx` deserializes with `bincode`. Same for `put_evm_receipt/get_evm_receipt`. All EVM transactions committed through the atomic batch path produce unreadable data.  
**Impact**: EVM transaction lookups return deserialization errors. Data permanently lost.  
**Fix**: Change `StateBatch::put_evm_tx` and `put_evm_receipt` to use `bincode::serialize`.

### C4. Validator announce creates tokens ex nihilo
**File**: [validator/src/main.rs](validator/src/main.rs#L3243-L3272)  
**Category**: Security / Inflation  
**Description**: When a `ValidatorAnnounce` is received, bootstrap account created with `shells: MIN_VALIDATOR_STAKE` without deducting from treasury. Compare to local bootstrap at [L2313-2343](validator/src/main.rs#L2313-L2343) which properly deducts.  
**Impact**: Attacker sends up to 1,000 fake announces → creates 10,000,000 MOLT. Inflates supply and controls stake weighting.  
**Fix**: Only accept announces from validators whose on-chain stake can be verified, or deduct from treasury like the local path does.

### C5. Malicious `highest_seen` slot inflation forces arbitrary block replacement
**File**: [validator/src/main.rs](validator/src/main.rs#L2892-L2896)  
**Category**: Security / Chain Rewrite  
**Description**: Fork choice uses `we_are_behind = highest_seen > current_slot + 5`. The `highest_seen` is set from incoming block slots at [L2623](validator/src/main.rs#L2623) **before validation**. Attacker sends blocks with slot u64::MAX → `we_are_behind` permanently true → any block replacement accepted regardless of vote weight.  
**Impact**: Complete chain rewrite capability, enabling double-spends on finalized blocks.  
**Fix**: Only update `highest_seen` from validated, accepted blocks.

### C6. Multisig duplicate-signer bypass
**File**: [core/src/multisig.rs](core/src/multisig.rs#L40-L47)  
**Category**: Security / Auth Bypass  
**Description**: `verify_threshold` checks length and membership but not uniqueness. A single compromised key repeated N times satisfies N-of-M threshold.  
**Impact**: 3-of-5 treasury multisig drained with one compromised key.  
**Fix**: Deduplicate `signed_by` (e.g., collect into `HashSet`) before checking threshold.

### C7. Incomplete block reversal — user transactions double-applied during fork choice
**File**: [validator/src/main.rs](validator/src/main.rs#L829-L890)  
**Category**: Data Corruption  
**Description**: `revert_block_effects()` only reverses rewards and producer fee share. User transaction effects (transfers, NFT mints, contract calls) are NOT reversed. Fork resolution applies new block's transactions on top of old block's still-applied state.  
**Impact**: Double-spending, phantom balances, incorrect NFT ownership after any fork resolution involving blocks with user transactions.  
**Fix**: Replay old block's transactions in reverse to undo their effects, or maintain checkpoint state for rollback.

### C8. Custody deposit key derivation is predictable — funds stealable
**File**: [custody/src/main.rs](custody/src/main.rs#L2436-L2444) (Solana), [custody/src/main.rs](custody/src/main.rs#L2519-L2527) (EVM)  
**Category**: Cryptographic Weakness  
**Description**: Private keys derived as `SHA256("molt/solana/usdc/{user_id}/{index}")`. Anyone who guesses user_id + index derives the private key.  
**Impact**: Total loss of all custodied bridge funds.  
**Fix**: Use proper HD key derivation (BIP-32/BIP-44) with a secret master seed from HSM/secure enclave.

### C9. Custody signing requests missing auth header — sweep pipeline broken
**File**: [custody/src/main.rs](custody/src/main.rs#L1766-L1777)  
**Category**: Integration Bug  
**Description**: `collect_signatures()` posts to threshold signers without `Authorization: Bearer <token>` header. The signer at [threshold_signer.rs](validator/src/threshold_signer.rs#L110-L125) enforces bearer auth → all signing silently fails.  
**Impact**: Entire deposit sweep and withdrawal pipeline non-functional. Deposited funds never swept.  
**Fix**: Include `Authorization: Bearer {token}` header in signing requests.

### C10. Privacy/ZK proofs are forgeable — HMAC keyed with public data
**File**: [core/src/privacy.rs](core/src/privacy.rs#L68-L99)  
**Category**: Broken Cryptography  
**Description**: "ZK proofs" are HMAC-SHA256 keyed with `commitment_root`, which is public state. Anyone can compute valid proofs.  
**Impact**: Unlimited shielded token minting, drain entire shielded pool. Zero privacy.  
**Fix**: Replace with actual ZK proof system (Groth16, PLONK) or acknowledge as placeholder with runtime-disabled feature flag.

---

## HIGH — 18 Findings

### H1. Fee-free instruction types (2–5) usable by anyone
**File**: [core/src/processor.rs](core/src/processor.rs#L131-L136) + [core/src/processor.rs](core/src/processor.rs#L1148-L1155)  
**Category**: Security / DoS  
Types 2–5 return fee=0 and route to `system_transfer` with no authorization. Any user signs type-2 tx → free transfer.  
**Fix**: Check sender == treasury/genesis authority for types 2–5.

### H2. EVM transactions skip native fee burn/split and rent
**File**: [core/src/processor.rs](core/src/processor.rs#L457-L459)  
EVM path returns early before `charge_fee` → no native 50/30/10/10 split, no rent.  
**Fix**: Apply fee split + rent after EVM execution.

### H3. EVM state committed before atomic batch (atomicity violation)
**File**: [core/src/processor.rs](core/src/processor.rs#L997-L1023)  
REVM's `transact_commit` writes state before batch starts. If batch fails, EVM state persisted without tx record.  
**Fix**: Use REVM `transact` (not `transact_commit`), apply state changes through batch.

### H4. `begin_batch` silently overwrites active batch (concurrency corruption)
**File**: [core/src/processor.rs](core/src/processor.rs#L427-L429)  
Under concurrent calls, Thread B's `begin_batch` discards Thread A's in-progress batch.  
**Fix**: Assert no active batch exists, or use per-transaction batch ownership.

### H5. Non-atomic `StateStore::transfer` — crash loses funds
**File**: [core/src/state.rs](core/src/state.rs#L1227-L1241)  
Two separate `put_account` calls without WriteBatch. Crash between = funds destroyed.  
**Fix**: Use WriteBatch for both puts.

### H6. Race conditions on sequence counters (events, transfers, tx-by-slot)
**File**: [core/src/state.rs](core/src/state.rs#L3850-L3868), [core/src/state.rs](core/src/state.rs#L4058-L4076), [core/src/state.rs](core/src/state.rs#L4141-L4159)  
Non-atomic read-modify-write → duplicate sequences → silent data loss.  
**Fix**: Use RocksDB `Merge` operator or atomic increment.

### H7. `prune_slot_stats` deletes unprocessed dirty-account markers → stale state root
**File**: [core/src/state.rs](core/src/state.rs#L3375-L3396)  
Prune can run between account modifications and `compute_state_root`, deleting unprocessed dirty markers.  
**Fix**: Only prune markers after confirming state root was computed.

### H8. Vote equivocation not prevented in VoteAggregator
**File**: [core/src/consensus.rs](core/src/consensus.rs#L1195-L1203)  
Validator can vote for different blocks at same slot — both accepted into separate map entries. Both count toward supermajority.  
**Fix**: Track `(slot, validator)` → reject second vote for same slot regardless of block hash.

### H9. Delegation commission silently burned (never credited to validator)
**File**: [core/src/consensus.rs](core/src/consensus.rs#L840-L843)  
`commission` amount calculated and subtracted from `delegation_share` but never credited anywhere.  
**Fix**: Return commission amount with delegation distributions, credit to validator.

### H10. Integer truncation in weighted leader selection
**File**: [core/src/consensus.rs](core/src/consensus.rs#L1138)  
`reputation / 100` integer division creates extreme step-function: 199→200 rep doubles weight.  
**Fix**: `base_weight * reputation / 100` preserves granularity.

### H11. Unstake cooldown constant wrong (2.8 days, not 7)
**File**: [core/src/consensus.rs](core/src/consensus.rs#L149)  
`604_800` = seconds in 7 days, not slots. At 400ms/slot, 7 days = `1,512,000` slots.  
**Fix**: `UNSTAKE_COOLDOWN_SLOTS: u64 = 1_512_000`

### H12. Reward credit when `liquid == 0` gives full reward as spendable
**File**: [validator/src/main.rs](validator/src/main.rs#L999-L1002)  
When `liquid == 0` (all goes to debt), fallback sets `debit_amount = credit_amount = reward_total` → producer gets spendable tokens that should be locked.  
**Fix**: When `liquid == 0`, set `credit_amount = 0` (only debt repayment occurs).

### H13. Block producer auto-registers in validator set without authorization
**File**: [validator/src/main.rs](validator/src/main.rs#L912-L924)  
Any node producing a signed block gets added to validator set. No stake verification.  
**Fix**: Require on-chain MIN_VALIDATOR_STAKE before accepting new validator.

### H14. CORS `starts_with` allows origin bypass
**File**: [rpc/src/lib.rs](rpc/src/lib.rs#L777-L782)  
`starts_with("http://localhost")` matches `http://localhost.evil.com`.  
**Fix**: Parse origin URL and compare host exactly.

### H15. Solana TX cache poisoned on failed submission
**File**: [rpc/src/lib.rs](rpc/src/lib.rs#L2674-L2690)  
TX inserted into cache before `submit_transaction`. If submit fails, cache reports it as "finalized."  
**Fix**: Insert into cache only after successful submission.

### H16. State-mutating RPC endpoints bypass consensus
**File**: [rpc/src/lib.rs](rpc/src/lib.rs#L3854-L3888) (`setContractAbi`), [rpc/src/lib.rs](rpc/src/lib.rs#L3937-L4127) (`deployContract`), [rpc/src/lib.rs](rpc/src/lib.rs#L5839-L5929) (`requestAirdrop`)  
Direct state mutations (not through mempool/consensus) → divergence in multi-validator network.  
**Fix**: Convert these to transaction submissions through mempool, or designate as admin-only with explicit single-validator mode check.

### H17. P2P: Dead peers never removed after connection error
**File**: [p2p/src/peer.rs](p2p/src/peer.rs#L183-L190)  
Ghost entries accumulate → `broadcast()` wastes time, `MAX_PEERS` blocks legit connections.  
**Fix**: Remove peer from DashMap in the spawned task's `if let Err` handler.

### H18. P2P: No rate limit on deserialization failures → unlimited DoS
**File**: [p2p/src/peer.rs](p2p/src/peer.rs#L336-L343)  
Malicious peer sends unlimited garbage streams (up to 2MB each) — never penalized or disconnected.  
**Fix**: Count failures per peer, call `record_violation` after N failures, disconnect.

---

## MEDIUM — 20 Findings

### M1. `reconcile_active_account_count` is a no-op
**File**: [core/src/state.rs](core/src/state.rs#L4192-L4197)  
Reads in-memory counter and writes it back. Should call `count_active_accounts_full_scan()`.

### M2. `StateBatch::register_symbol` skips validation (alphanumeric, length)
**File**: [core/src/state.rs](core/src/state.rs#L2577-L2607)  
Only trims/uppercases. No alphanumeric or ≤10 char check. Invalid symbols possible through batch path.

### M3. `StateStore::register_evm_address` missing reverse mapping
**File**: [core/src/state.rs](core/src/state.rs#L3410-L3423)  
Batch version writes both forward+reverse mappings; non-batch version only writes forward.

### M4. Failed transactions roll back fees → free compute DoS
**File**: [core/src/processor.rs](core/src/processor.rs#L603-L612)  
Attacker submits expensive intentionally-failing TXs → validator compute consumed, fee=0 returned.

### M5. `claim_unstake` has no staker identity → cross-user claim risk
**File**: [core/src/processor.rs](core/src/processor.rs#L1466-L1468)  
Only validator pubkey passed, no staker scoping.

### M6. NFT `token_id` written outside atomic batch
**File**: [core/src/processor.rs](core/src/processor.rs#L1358-L1362)  
Direct `state.index_nft_token_id` call not through `b_*` accessor → phantom entries on rollback.

### M7. Downtime evidence not deduplicated → compound slashing
**File**: [core/src/consensus.rs](core/src/consensus.rs#L1549-L1573)  
All offense types deduplicated except `Downtime`. Same evidence submitted repeatedly.

### M8. `voting_power()` sum can exceed 100%
**File**: [core/src/consensus.rs](core/src/consensus.rs#L522-L535)  
Numerator includes delegated stake, denominator (`active_stake`) does not. Same root cause as C2.

### M9. EVM commit `u64::MAX` saturation → silent inflation
**File**: [core/src/evm.rs](core/src/evm.rs#L449-L451)  
`u256_to_u64` failure → `u64::MAX`. An EVM contract could trigger this to mint max native shells.

### M10. ReefStake exchange rate manipulation via unstake timing
**File**: [core/src/reefstake.rs](core/src/reefstake.rs#L218-L238)  
`total_supply` decremented immediately but `total_molt_staked` held until claim. Rate inflated during cooldown.

### M11. Account deserialization breaks invariant for legacy accounts
**File**: [core/src/account.rs](core/src/account.rs#L130-L138)  
`#[serde(default)]` on `spendable/staked/locked` → old accounts get `spendable=0`, funds unspendable.

### M12. WASM `F32/F64` mapped to `U32/U64` in ABI extraction
**File**: [core/src/contract.rs](core/src/contract.rs#L189-L196)  
Float params incorrectly typed in auto-extracted ABIs.

### M13. Custody: race condition in reserve ledger (concurrent read-modify-write)
**File**: [custody/src/main.rs](custody/src/main.rs#L3060-L3098)  
Multiple background workers can race on same key.

### M14. Custody: rebalance assumes 1:1 swap output
**File**: [custody/src/main.rs](custody/src/main.rs#L3406-L3411)  
Credits exact input amount as output. Real DEX has slippage → ledger overcount.

### M15. Custody: only first Solana deposit signature processed
**File**: [custody/src/main.rs](custody/src/main.rs#L839)  
`limit: 1` on signature fetch → subsequent deposits to same address lost.

### M16. Custody: ERC-20 sweep has no gas funding mechanism
**File**: [custody/src/main.rs](custody/src/main.rs#L2001-L2044)  
Derived deposit addresses only receive tokens, never ETH for gas → all EVM ERC-20 sweeps fail.

### M17. Custody: withdrawal endpoint has no authentication
**File**: [custody/src/main.rs](custody/src/main.rs#L2837-L2943)  
Anyone can create withdrawals for arbitrary `user_id`.

### M18. P2P: DashMap guard held across `.await` → stalls
**File**: [p2p/src/peer.rs](p2p/src/peer.rs#L202)  
`send_to_peer` holds shard read guard during network I/O → peer management blocked.

### M19. P2P: `BlockRangeRequest` no range validation
**File**: [p2p/src/network.rs](p2p/src/network.rs#L295-L310)  
Peer can request range `0..u64::MAX`. No bounds check at P2P layer.

### M20. RPC: unbounded reverse block scan on missing tx → DoS
**File**: [rpc/src/lib.rs](rpc/src/lib.rs#L1573-L1586)  
Fallback linear scan from `last_slot` to 0 for non-existent transactions.

---

## LOW — 7 Findings

| # | File | Issue |
|---|------|-------|
| L1 | [core/src/state.rs](core/src/state.rs#L1036-L1044) | Merkle odd-leaf promotion without rehash (weak 2nd preimage) |
| L2 | [core/src/mempool.rs](core/src/mempool.rs#L52-L56) | `PartialEq` based on priority, not identity |
| L3 | [core/src/reefstake.rs](core/src/reefstake.rs#L338-L342) | Rounding dust loss on reward distribution |
| L4 | [p2p/src/gossip.rs](p2p/src/gossip.rs#L131-L139) | Gossip fabricates `last_seen` timestamps |
| L5 | [p2p/src/peer_store.rs](p2p/src/peer_store.rs#L50-L72) | Sync Mutex held during file I/O in async context |
| L6 | [faucet/src/main.rs](faucet/src/main.rs#L50-L165) | Unbounded in-memory growth (airdrops vec, rate limiter maps) |
| L7 | [validator/src/main.rs](validator/src/main.rs#L1432-L1480) | Supervisor backoff never resets |

---

## Recommended Fix Priority

### Phase 1 — Consensus Safety (must fix, blocks mainnet)
| ID | Effort | Description |
|----|--------|-------------|
| C1 | 5 min | Sort voter pubkeys before fee distribution |
| C2 | 15 min | Track delegated amounts in `total_staked` |
| C7 | 2 hr | Implement proper block revert (undo user TXs) |
| H8 | 15 min | Prevent vote equivocation (per-slot dedup) |
| H11 | 1 min | Fix UNSTAKE_COOLDOWN_SLOTS to 1,512,000 |

### Phase 2 — Economic Security (must fix, supply integrity)
| ID | Effort | Description |
|----|--------|-------------|
| C4 | 30 min | Deduct from treasury in ValidatorAnnounce handler |
| H1 | 10 min | Auth-check instruction types 2–5 |
| H2 | 30 min | Apply native fee split to EVM transactions |
| H9 | 10 min | Credit commission to validator in delegation rewards |
| H12 | 5 min | Fix `liquid==0` reward crediting |
| H10 | 5 min | Fix integer truncation in leader selection |

### Phase 3 — Data Integrity (should fix)
| ID | Effort | Description |
|----|--------|-------------|
| C3 | 5 min | Switch EVM batch serialization to bincode |
| H3 | 1 hr | Use REVM `transact` instead of `transact_commit` |
| H5 | 15 min | Wrap StateStore::transfer in WriteBatch |
| H6 | 30 min | Use RocksDB Merge for sequence counters |
| H7 | 15 min | Guard dirty-marker pruning |

### Phase 4 — Security Hardening
| ID | Effort | Description |
|----|--------|-------------|
| C5 | 10 min | Only update highest_seen from validated blocks |
| C6 | 5 min | Deduplicate multisig signers |
| C10 | 30 min | Disable privacy module or add real ZK |
| H13 | 15 min | Require on-chain stake for validator registration |
| H14 | 10 min | Parse CORS origin URL properly |
| H15 | 5 min | Insert TX cache only after successful submit |
| H16 | 30 min | Route state-mutating RPCs through mempool |

### Phase 5 — Custody (before bridge launch)
| ID | Effort | Description |
|----|--------|-------------|
| C8 | 2 hr | Replace key derivation with HD wallet + HSM |
| C9 | 5 min | Add auth header to signing requests |
| M13–M17 | 2 hr | Reserve ledger locking, gas funding, multi-deposit, auth |

### Phase 6 — P2P Hardening
| ID | Effort | Description |
|----|--------|-------------|
| H17 | 10 min | Remove dead peers from DashMap |
| H18 | 15 min | Rate-limit deserialization failures |
| M18 | 30 min | Clone connection before releasing DashMap ref |
| M19 | 5 min | Validate BlockRangeRequest bounds |

---

## Cross-Reference with Prior Audits

| Prior Fix | Current Finding | Note |
|-----------|----------------|------|
| H13 "Bootstrap grant ex nihilo" — FIXED for local path | **C4** still present in P2P announce handler | Fix didn't cover remote path |
| 2.3 "Validator bootstrap grants free tokens via P2P" — marked FIXED | **C4** same issue, still exists in code at L3243 | Fix was incomplete |
| 2.7 "CORS fully permissive" — FIXED | **H14** uses `starts_with` which is bypassable | Fix was insufficient |
| H5 "Privacy HMAC not ZK" — marked FALSE POSITIVE | **C10** this is genuinely broken crypto | Should be reconsidered |
| 1.4 "No transaction atomicity" — FIXED via StateBatch | **C3** batch uses wrong serialization format | Introduced during the fix |
| 3.8 "EVM balance commit" — FIXED | **H3** REVM still commits before batch | Root cause remains |

---

## Test Baseline

```
301 tests passing (0 failures)
  core:      101 + 100 + 14 + 16 = 231
  validator: 7
  rpc:       18
  cli:       12
  p2p:       10
  faucet:    5
  custody:   3 + 9 + 4 + 2 = 18
```

All findings above are **code-verified** with exact line numbers from the current codebase.
