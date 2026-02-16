# MOLTCHAIN PRODUCTION READINESS AUDIT (REVALIDATED)

**Audit revalidation date**: February 16, 2026  
**Repository**: `lobstercove/moltchain` (`main`)  
**Method**: Code inspection of claimed files/functions only (no fixes applied)  
**Purpose**: Verify whether each previously listed finding is still true in current code

---

## Executive Verdict

The previous report is **partly stale and overstated in several areas**. Many claimed critical/high issues are now fixed in code, but a smaller subset of consensus/economic risks remain, mostly around **fork/rollback completeness**, **validator bootstrap trust flow**, and **sync/fork trust assumptions**.

### Revalidated Status Summary (all listed tasks)

- **Critical (12):** `Fixed/Mitigated: 10`, `Partially Open: 1`, `Mostly Mitigated but feature-risk remains: 1`
- **High (18):** `Fixed/Mitigated: 12`, `Partially Open: 2`, `Outdated/Not Present: 4`
- **Medium (20):** `Fixed/Mitigated: 10`, `Open/Partially Open: 6`, `Outdated/Not Verifiable from listed claim: 4`
- **Low (7):** Mostly process/operational; only partially code-verifiable

> This repo is in significantly better shape than the prior document suggests, but not yet “all clear” for large-scale adversarial mainnet.

---

## Legend

- **Fixed**: Claim no longer matches current code behavior
- **Mitigated**: Core exploit path blocked, residual risk may remain
- **Partially Open**: Some fixes landed, but root safety property still incomplete
- **Open**: Claim still materially true
- **Outdated/Not Present**: Referenced path/behavior no longer exists as described
- **Not Verifiable**: Requires runtime/integration evidence beyond static inspection

---

## CRITICAL ISSUES (C1–C12)

| ID | Revalidated Status | Assessment vs Current Code |
|---|---|---|
| C1 | **Fixed** | Voter pubkeys are deduped and deterministically sorted before remainder allocation in `validator/src/main.rs` fee distribution. |
| C2 | **Fixed** | `delegate()` now increases `total_staked`; `undelegate()` decreases it in `core/src/consensus.rs`. Denominator/numerator mismatch claim is stale. |
| C3 | **Fixed** | Batch and non-batch EVM tx/receipt writes now use `bincode` and match read path in `core/src/state.rs`. |
| C4 | **Fixed** | AUDIT-FIX C4/H13 (commit bcc34e9): Bootstrap flow restructured — treasury debit now happens BEFORE stake pool credit. Self-funded validators stake immediately; bootstrap validators only get stake pool entry after successful treasury debit, with rollback on failure. |
| C5 | **Fixed** | AUDIT-FIX C5 (commit bcc34e9): Blocks from non-member validators are now rejected before `note_seen` and fork-choice. Validator-set membership check added after signature/structure validation, before equivocation detection and sync target update. |
| C6 | **Fixed** | `verify_threshold()` deduplicates signers via `HashSet` before threshold check in `core/src/multisig.rs`. |
| C7 | **Partially Open** | Fork rollback now includes `revert_block_transactions`, but it is best-effort and explicitly logs non-revertible instructions (contracts/NFT/staking) as unsafe without full snapshot rollback. |
| C8 | **Mitigated** | Key derivation moved to HMAC(master_seed, path) and production enforces `CUSTODY_MASTER_SEED` (unless explicit insecure dev flag). Not full HSM/BIP32 hardening yet. |
| C9 | **Fixed** | Signer requests now include bearer auth (`bearer_auth`) with per-signer token fallback in `custody/src/main.rs`. |
| C10 | **Mitigated** | Placeholder proof verifier is disabled by default (`allow_placeholder_proofs = false`). Privacy module still compiled (not feature-gated), but exploit path is blocked unless explicitly enabled in code/runtime. |
| C11 | **Fixed** | EVM execution path uses `transact()` + deferred state changes committed via `StateBatch`; no `transact_commit()` path in core EVM execution. |
| C12 | **Mitigated** | System instruction types 2–5 are now authorization-checked in execution path (`sender == treasury`), blocking free privileged transfer/mint abuse. Fee-calculation path remains zero for these kinds by design. |

---

## HIGH ISSUES (H1–H18)

| ID | Revalidated Status | Assessment vs Current Code |
|---|---|---|
| H1 | **Mitigated** | Same as C12; execution path now restricts types 2–5 to treasury sender. |
| H2 | **Outdated (as written)** | EVM fee does not go “producer only”; fee split logic is applied via existing fee pipeline. However, there is still possible accounting mismatch because block-level distribution uses computed fee estimate for EVM tx types. |
| H3 | **Fixed** | Same as C11. |
| H4 | **Outdated/Not Present (as written)** | Referenced `process_block` overwrite scenario in `core/src/processor.rs` does not match current architecture/pathing. |
| H5 | **Fixed** | Native transfer now uses atomic write batch semantics in `core/src/state.rs` (`transfer`). |
| H6 | **Partially Open** | Some counters are atomicized, but `next_event_seq` remains read-modify-write without CAS/merge atomicity under concurrency in `core/src/state.rs`. |
| H7 | **Outdated/Not Present (as written)** | Referenced dirty-marker pruning logic in `validator/src/sync.rs` is not present as described. |
| H8 | **Fixed** | Vote equivocation prevention exists via `(slot, validator)` index in `VoteAggregator` (`core/src/consensus.rs`). |
| H9 | **Fixed** | Delegation commission now credited to validator (not silently burned) in delegation reward distribution. |
| H10 | **Fixed** | Weighted leader selection uses non-zero weight floor and `u128` intermediates; truncation-to-zero claim is stale for current logic. |
| H11 | **Fixed** | `UNSTAKE_COOLDOWN_SLOTS` is `1_512_000` in `core/src/consensus.rs`. |
| H12 | **Fixed/Mitigated** | Reward application path now handles liquid/debt split correctly and avoids earlier liquid==0 fallback issue in validator reward credit logic. |
| H13 | **Fixed** | AUDIT-FIX C4/H13 (commit bcc34e9): Stake pool bootstrap entry is now gated on successful treasury debit. If treasury debit fails, no stake pool entry is created. If stake pool entry fails after debit, treasury is reversed. |
| H14 | **Fixed** | CORS now performs exact hostname allowlisting in `rpc/src/lib.rs` (no `starts_with` bypass pattern). |
| H15 | **Fixed** | Solana tx cache insertion now occurs only after successful submit in `handle_solana_send_transaction`. |
| H16 | **Outdated/Not Present (as written)** | `force_slot_advance` / `inject_transaction` endpoints are not present. Related state-mutating admin RPC exists but is token-protected; single-validator guard is applied to some, not all mutators. |
| H17 | **Partially Open** | Dead peers are not always removed immediately on connection error; cleanup/score paths eventually remove them. Risk reduced but not fully eliminated. |
| H18 | **Fixed** | Deserialization failure counter disconnects peers after threshold in P2P connection handler. |

---

## MEDIUM ISSUES (M1–M20)

| ID | Revalidated Status | Assessment vs Current Code |
|---|---|---|
| M1 | **Fixed** | Symbol normalization + uniqueness checks exist in both batch and non-batch registration paths. |
| M2 | **Fixed** | EVM mapping includes reverse mapping (`reverse:` keys) and registration helpers in both batch/state paths. |
| M3 | **Mitigated** | NFT operations are mostly batch-mediated now; claim of routine non-atomic splits appears stale, but full adversarial proof needs integration tests. |
| M4 | **Fixed** | Fees are charged before instruction execution (`charge_fee_direct`), so failed tx no longer compute for free. |
| M5 | **Fixed** | Unstake request keyed by `(validator, staker)` and claim path enforces that pairing. |
| M6 | **Fixed** | NFT token-id secondary indexing exists (`index_nft_token_id`, `nft_token_id_exists`). |
| M7 | **Partially Open** | `SlashingTracker` is serializable, but explicit durable persistence/reload wiring is still incomplete for full restart-proof evidence continuity. |
| M8 | **Partially Mitigated** | Many stake math paths now use `u128`/saturating ops, but not every multiplication path is uniformly hardened. |
| M9 | **Outdated/Partially Mitigated** | WASM runtime has metering and deploy-time validation; blanket “unmetered compile DoS” claim appears stale for current runtime design. |
| M10 | **Partially Open** | Market order behavior still allows aggressive fills (`price == 0` for market path); explicit user-side slippage bounds are limited. |
| M11 | **Fixed** | Order expiry validation is present at place/match time in `contracts/dex_core/src/lib.rs`. |
| M12 | **Open** | Emergency pause/unpause appears immediate admin action; no timelock mechanism observed. |
| M13 | **Mitigated** | Reserve ledger updates use explicit lock strategy (`RESERVE_LOCK` design notes + mutexed flows), reducing race risk. |
| M14 | **Partially Open** | Rebalance is not pure 1:1 anymore, but full market-price/slippage correctness depends on external quote execution paths and runtime behavior. |
| M15 | **Fixed** | Solana deposit watcher now processes multiple signatures (not only first). |
| M16 | **Fixed** | EVM token sweep funds gas to deposit address before transfer and waits for confirmation. |
| M17 | **Mitigated** | Withdrawal endpoint now requires bearer auth token and includes rate limits; still centralized API-auth model rather than per-user cryptographic request auth. |
| M18 | **Fixed** | DashMap guard-across-await issue addressed by cloning handles before async I/O. |
| M19 | **Fixed** | Block range requests are bounded (`range_size > 1000` rejected; response truncation safeguards). |
| M20 | **Open/Not Implemented** | No explicit oracle staleness guard found in `dex_core`; if oracle-based pricing is relied upon externally, staleness policy should be explicit. |

---

## LOW ISSUES (L1–L7)

| ID | Revalidated Status | Assessment vs Current Code |
|---|---|---|
| L1 | **Partially True** | Dead code / `allow(dead_code)` still exists in places. |
| L2 | **Partially True** | Logging quality varies by module and path. |
| L3 | **Partially True** | Safety invariants are better documented than before, but still inconsistent. |
| L4 | **Not Fully Verifiable** | Cache tuning requires workload profiling; static code confirms configurable caches exist. |
| L5 | **Partially True** | No unified Prometheus-grade metrics surface across all services. |
| L6 | **Partially True** | Error context quality is mixed; many improved, some generic remain. |
| L7 | **Partially True** | Cargo versions are semver ranges by default (not strict pinning). This is policy-sensitive, not a direct exploit by itself. |

---

## Prioritized Execution Checklist (Test-First, No Code Fixes Yet)

Only `Partially Open` / `Open` items are included below. Every item has mandatory verification tests that must be written/executed **before** any fix PR.

### Priority 0 — Consensus Safety (Ship Blockers)

1. **`C7` Fork rollback completeness (best-effort revert still unsafe)**
	 - **Pre-fix tests (must fail today):**
		 - `consensus_fork_revert_contract_state`: create fork where reverted block contains contract storage writes; assert post-fork state exactly matches canonical chain.
		 - `consensus_fork_revert_nft_state`: reverted block mints/transfers NFT; assert ownership and collection counters are canonical after fork switch.
		 - `consensus_fork_revert_staking_state`: reverted block mutates stake/unstake; assert pool/account parity after reorg.
	 - **Pass criteria:** byte-for-byte canonical state root parity across validators after N forced reorgs.

2. **`C4/H13` Validator announce/bootstrap trust coupling**
	 - **Pre-fix tests (must fail today):**
		 - `announce_bootstrap_without_treasury_funds`: treasury below `MIN_VALIDATOR_STAKE`; assert no validator admission and no stake-pool inflation.
		 - `announce_bootstrap_atomicity`: fail treasury debit path intentionally; assert validator-set + stake-pool + account state remain unchanged.
		 - `announce_prefunded_vs_bootstrap_path_consistency`: compare funded and unfunded announces; assert stake accounting invariants hold.
	 - **Pass criteria:** no path can increase active stake without corresponding funded on-chain account state.

3. **`C5` Highest-seen/fork-pressure from block stream**
	 - **Pre-fix tests (must fail today):**
		 - `sync_high_slot_untrusted_block_rejected`: malicious peer sends very high-slot block with valid structure but unadmitted producer; assert `highest_seen` and fork-choice are unaffected.
		 - `fork_choice_weight_not_overridden_by_untrusted_seen`: higher vote-weight canonical block must win even with spammed high slots.
	 - **Pass criteria:** fork replacement decisions depend only on validated/admitted chain evidence.

### Priority 1 — Economic and Integrity Hardening

4. **`H6` Sequence counter concurrency (`next_event_seq` RMW race)**
	 - **Pre-fix tests (must fail/flaky today):**
		 - `event_seq_concurrent_uniqueness`: 32+ threads write same `(program, slot)` concurrently; assert strictly unique, contiguous sequence IDs.
		 - `event_seq_no_overwrite_under_contention`: verify no event key collisions in RocksDB.
	 - **Pass criteria:** zero duplicate sequence IDs across stress iterations.

5. **`H17` Dead peer lifecycle cleanup not immediate on all error paths**
	 - **Pre-fix tests (must fail today):**
		 - `peer_disconnect_cleanup_on_stream_error`: force stream failure; assert peer removed within bounded interval.
		 - `peer_set_no_zombie_growth`: long soak with repeated connect/drop; assert active peers map remains bounded.
	 - **Pass criteria:** no unbounded zombie peer accumulation.

6. **`M7` Slashing evidence durability across restart**
	 - **Pre-fix tests (must fail today):**
		 - `slashing_evidence_persists_restart`: add evidence, restart validator, verify evidence still loaded and slash decision unchanged.
		 - `slashing_prune_consistency_restart`: verify pruned vs retained evidence windows remain correct after restart.
	 - **Pass criteria:** restart does not erase adjudication-critical evidence.

### Priority 2 — Policy / Contract Controls

7. **`M12` DEX admin timelock missing**
	 - **Pre-fix tests (must fail today):**
		 - `dex_admin_action_requires_delay`: pause/unpause should not execute in same block/slot as scheduling.
		 - `dex_admin_cancellation_flow`: scheduled action can be canceled before execution window.
	 - **Pass criteria:** admin emergency controls follow explicit timelock policy.

8. **`M20` Oracle staleness guard absent in DEX policy path**
	 - **Pre-fix tests (must fail today):**
		 - `oracle_price_staleness_rejected`: stale timestamp beyond policy threshold must reject pricing-dependent operation.
		 - `oracle_freshness_boundary`: values just inside/outside staleness bound produce deterministic allow/deny behavior.
	 - **Pass criteria:** stale oracle data cannot be used for execution-critical pricing.

9. **`M10` Market-order slippage control is limited**
	 - **Pre-fix tests (must fail today):**
		 - `market_order_worst_price_bound`: market order with bound should fail when execution exceeds bound.
		 - `market_order_sandwich_resilience_sim`: adversarial book movement should not execute beyond caller-declared tolerance.
	 - **Pass criteria:** explicit caller slippage constraints are always enforced.

10. **`M14` Rebalance pricing/slippage correctness remains runtime-dependent**
		- **Pre-fix tests (must fail/unstable today):**
			- `rebalance_quote_execution_parity`: quoted output vs executed output within tolerance.
			- `rebalance_extreme_slippage_abort`: operation aborts when realized slippage exceeds configured max.
		- **Pass criteria:** rebalance never assumes fixed 1:1 behavior and always enforces slippage bounds.

---

## Test-First Delivery Order

1. Build deterministic reorg harness (`C7`, `C5`)  
2. Add bootstrap/announce atomicity tests (`C4`, `H13`)  
3. Add concurrency stress suite (`H6`, `H17`)  
4. Add persistence/restart suite (`M7`)  
5. Add contract policy tests (`M12`, `M20`, `M10`, `M14`)

No code fixes should start until the above tests exist and are reproducibly red for their targeted gaps.

---

## Old Audit Comparison & Consolidation

Compared legacy audit artifacts against this revalidated file. Findings were overlapping, stale, or superseded by current-code verification.

Legacy files compared:
- `FINAL_AUDIT_FEB13.md`
- `CONTRACTS_AUDIT_REPORT.md`
- `DEX_WASM_AUDIT.md`
- `SECURITY_AUDIT_LEGACY_CONTRACTS.md`
- `docs/DEFINITIVE_AUDIT_FEB12.md`
- `docs/COMPREHENSIVE_AUDIT_FEB8.md`
- `docs/CODE_AUDIT_PROOF_POINTS.md`
- `AUDIT_SWEEP_PLAN.md`

Consolidation policy: **single source of truth = this file** (`PRODUCTION_READINESS_AUDIT.md`).

---

## Reassessed Production Readiness (Based on This Revalidation)

### What was exaggerated in prior audits

- Broad claims that critical issues remained broadly unfixed are no longer accurate for many C/H findings.
- Several previously flagged vulnerabilities are now patched in code paths validated in this review.
- Some old reports reference outdated endpoints/flows no longer present.

### What still blocks confident adversarial launch

1. Fork/reorg rollback correctness for non-transfer side effects (`C7`).
2. ~~Bootstrap/announce atomic trust and accounting coupling (`C4/H13`).~~ **FIXED** (commit bcc34e9)
3. ~~Highest-seen/fork-choice trust boundary hardening (`C5`).~~ **FIXED** (commit bcc34e9)
4. Policy/security hardening (timelock, staleness, persistence, concurrency) (`M12`, `M20`, `M7`, `H6`, `H17`, plus `M10/M14`).

---

**Confidence in this revalidation:** Medium-High for static code correctness claims; Medium for distributed/runtime behavior pending test-first verification above.
