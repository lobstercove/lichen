# MoltChain Production Audit Findings (Documentation Only)

Date: 2026-02-22  
Scope reviewed: core runtime, contracts, RPC/WS paths, custody service, wallet frontend extension  
Constraint honored: findings and recommendations only, no code changes

## 2026-02-23 Implementation Addendum

Scope updated per follow-up request to implement and verify fixes.

### Completed fixes

- Placeholder ZK gate hardened in [core/src/privacy.rs](core/src/privacy.rs) by making placeholder-proof toggle non-public and test-only via `#[cfg(test)]` helper.
- Consensus-bypass prediction create path removed from live mutation flow in [rpc/src/prediction.rs](rpc/src/prediction.rs); endpoint now rejects direct state writes and instructs transaction-based creation.
- Wallet popup RPC-derived rendering hardened in [wallet/extension/src/popup/popup.js](wallet/extension/src/popup/popup.js) by escaping dynamic content in activity and staking templates.
- Custody RocksDB CF tuning implemented in [custody/src/main.rs](custody/src/main.rs) with shared block cache, bloom/index caching, prefix extractors, and per-CF option presets.
- MoltyID given-vouch reverse index implemented in [contracts/moltyid/src/lib.rs](contracts/moltyid/src/lib.rs) and consumed in [rpc/src/lib.rs](rpc/src/lib.rs) to replace hot-path O(n²) scans (with compatibility fallback).

### Verification performed

- `cargo check -p moltchain-core` ✅
- `cargo check -p moltchain-rpc` ✅
- `cargo check -p moltchain-custody` ✅
- `cargo test -p moltchain-rpc --lib` ✅ (7 passed)
- `cargo test -p moltchain-custody` ✅ (38 passed)
- `cargo test -p moltchain-core privacy --lib` ✅ (module compiles; no matching test filter cases)

### Notes

- Existing workspace warnings in unrelated files remain (validator/core test warnings), but no new compile/test failures were introduced by these fixes.

## 2026-02-23 Follow-up Addendum (Template Flow + Contract/RPC Alignment)

Implemented per follow-up request: non-disruptive transaction-template path for prediction market creation, plus targeted alignment review of contract call conventions.

### New implementation

- Added `POST /api/v1/prediction-market/create-template` in [rpc/src/prediction.rs](rpc/src/prediction.rs).
- Endpoint validates input, resolves `PREDICT` program from symbol registry, builds `ContractInstruction::Call { function: "call", ... }` payload (opcode `1` for `create_market`), and returns an unsigned transaction template for wallet signing.
- Returned payload is `sendTransaction`-ready (`message.instructions[].data` as byte array, `message.blockhash` set from latest block hash).
- Existing `POST /api/v1/prediction-market/create` remains disabled for direct writes (consensus safety preserved).

### Alignment checks performed

- Confirmed contract ABI dispatch for prediction creation in [contracts/prediction_market/src/lib.rs](contracts/prediction_market/src/lib.rs#L3403-L3416) matches template builder layout.
- Confirmed runtime contract invocation semantics in [core/src/processor.rs](core/src/processor.rs#L2699-L2728): `CONTRACT_PROGRAM_ID` + accounts `[caller, contract]` + serialized `ContractInstruction`.
- Confirmed RPC JSON transaction parser accepts wallet-style shape (`program_id` base58 string, `accounts` base58 strings, `data` byte array, `blockhash` hex) in [rpc/src/lib.rs](rpc/src/lib.rs#L2960-L3055).
- Confirmed DEX frontend contract-call pattern remains consistent (no bypass reintroduced) in [dex/dex.js](dex/dex.js#L274-L306).

### Validation

- `cargo check -p moltchain-rpc` ✅
- `cargo test -p moltchain-rpc parse_json_transaction -- --nocapture` ✅ (0 selected, parser path still compiles and links)

### Remaining non-blocking gap (documented)

- `initialLiquidity` cannot be safely auto-bundled in a single deterministic template without race-prone market-id assumptions; current template intentionally separates create and add-liquidity into sequential transactions.

## 2026-02-23 Additional Sweep Addendum (Fee Semantics + Reorg Correctness)

### Fee-to-voters semantics (confirmed)

- `Fee to Voters` is distributed to validators who actually voted for the block at that slot, not burned/retained centrally.
- Distribution is stake-weighted among eligible voters (active validators in the vote set), not equal split by voter count.
- Implementation references:
  - [validator/src/main.rs](validator/src/main.rs#L2500-L2553) — voter set collection, deterministic ordering, stake-weighted share formula.
  - [validator/src/main.rs](validator/src/main.rs#L2561-L2579) — per-voter account crediting.

### New production-readiness finding

- **HIGH — Fork/reorg reversal remains economically approximate and can mis-refund fees**
  - Evidence:
    - [validator/src/main.rs](validator/src/main.rs#L1794-L1799) explicitly documents fee reversal as approximate.
    - [validator/src/main.rs](validator/src/main.rs#L2002-L2005) refunds fee via `TxProcessor::compute_transaction_fee` during tx reversal.
    - [core/src/processor.rs](core/src/processor.rs#L764-L767) actual charged fee uses reputation-discounted value (`apply_reputation_fee_discount`).
  - Why this matters:
    - On reorg, refund path can diverge from the originally charged amount for discounted payers, creating over/under-refund drift.
    - Combined with approximate fee-effects reversal, this weakens strict economic determinism under fork stress.
  - Recommendation:
    - Persist `fee_paid` per transaction at execution and use that exact value for rollback/refund.
    - Extend `revert_block_effects` to reverse voter/community allocations exactly (or via deterministic ledgered deltas), not approximately.

## 2026-02-23 Implementation Addendum (Exact Fee Rollback + Deep Contracts/Custody Sweep)

### Implemented now

- **Exact fee persistence on block metadata**
  - Added `tx_fees_paid: Vec<u64>` to [core/src/block.rs](core/src/block.rs) with `#[serde(default)]` for backward compatibility with legacy blocks.
  - Producer now stores exact charged fee per successful tx into block metadata in [validator/src/main.rs](validator/src/main.rs).

- **Exact fee replay during apply/reorg**
  - Fee distribution now sums exact charged fees when present (legacy fallback preserved) in [validator/src/main.rs](validator/src/main.rs).
  - Reorg fee-effect reversal now uses exact total fee when present in [validator/src/main.rs](validator/src/main.rs).
  - Per-tx reorg refund now replays exact `fee_paid` by tx index when present (legacy fallback preserved) in [validator/src/main.rs](validator/src/main.rs).

- **Custody hardening**
  - Removed generated signer auth token value from logs to prevent secret leakage in [custody/src/main.rs](custody/src/main.rs).
  - Added fail-closed guard for unsupported multi-signer production mode unless explicitly overridden with `CUSTODY_ALLOW_UNSAFE_MULTISIGNER=1` in [custody/src/main.rs](custody/src/main.rs).

- **Contract accounting fix**
  - In [contracts/clawpump/src/lib.rs](contracts/clawpump/src/lib.rs), graduation platform-revenue accounting now updates only after full successful DEX migration (`create_pair` + `create_pool` + `add_liquidity`).

### Validation

- `cargo check -p moltchain-core` ✅
- `cargo check -p moltchain-custody` ✅
- `cargo check --manifest-path contracts/clawpump/Cargo.toml` ✅
- `cargo check -p moltchain-rpc` ✅
- `cargo check -p moltchain-validator` ✅

### Deep sweep — additional production gaps (contracts + custody)

- **HIGH — ClawPump graduation cross-contract migration remains non-atomic**
  - Evidence: [contracts/clawpump/src/lib.rs](contracts/clawpump/src/lib.rs) executes `create_pair`, `create_pool`, `add_liquidity` as three independent external calls.
  - Risk: partial success can leave cross-protocol divergence (e.g., pair exists without pool/liquidity).
  - Recommendation: move to idempotent 2-phase migration with explicit phase markers + compensation actions, or a router-side atomic orchestration contract.

- **HIGH — Custody multi-signer path still functionally incomplete for production FROST flow**
  - Evidence: runtime already documents/warns this in [custody/src/main.rs](custody/src/main.rs), and now fail-closes by default.
  - Risk: invalid signatures / failed withdrawals if enabled without full 2-round integration.
  - Recommendation: complete sweep/withdraw wiring for true 2-round FROST and add end-to-end multi-node signing tests before removing fail-closed guard.

- **MEDIUM — Contract action handlers still rely heavily on raw pointer copies (`unsafe`) without uniform bounded-input policy**
  - Evidence: multiple contracts under [contracts/](contracts/) perform direct pointer reads with variable lengths.
  - Risk: inconsistent failure semantics and memory-pressure/trap surfaces under malformed input.
  - Recommendation: standardize max input lengths per opcode/function across all contracts and enforce via shared helper macros + adversarial tests.

### Optimization + extended features backlog (recommended)

- Add deterministic action receipts for cross-contract actions (status, error code, side-effect hash) so retry/compensation is safe and auditable.
- Add a contract-level migration framework for long-running workflows (state machine phases + resumable idempotent transitions).
- Add custody signer liveness telemetry and quorum health endpoint (`ready_signers`, `threshold`, `last_successful_round`) to surface pre-failure conditions.

## 2026-02-23 Hardening Follow-up Addendum (Custody + Contracts)

### Additional implemented safeguards

- **Custody webhook fan-out bounded**
  - Added bounded concurrency for webhook deliveries in [custody/src/main.rs](custody/src/main.rs) via `Semaphore` guard in dispatcher path.
  - New env control: `CUSTODY_WEBHOOK_MAX_INFLIGHT` (default `64`, capped at `1024`).
  - Effect: prevents unbounded `tokio::spawn` fan-out under bursty event loads.

- **MoltDAO proposal payload bounds enforced**
  - Added strict maximum sizes in [contracts/moltdao/src/lib.rs](contracts/moltdao/src/lib.rs):
    - title ≤ 256 bytes
    - description ≤ 8 KiB
    - action payload ≤ 16 KiB
  - Validation now runs before dynamic allocations/copies in proposal creation path.

### Additional deep-sweep findings (new)

- **MEDIUM — Custody webhook endpoint validation still lacks destination allowlist / egress policy**
  - Evidence: [custody/src/main.rs](custody/src/main.rs#L6845-L6863) accepts HTTPS (and localhost dev) but does not enforce org allowlist/private-network denylist.
  - Risk: authenticated operator error/misuse could route signed event payloads to unintended destinations.
  - Recommendation: add optional hostname allowlist + private CIDR deny-by-default policy for production.

- **MEDIUM — MoltDAO still performs raw pointer copies in multiple entrypoints**
  - Evidence: [contracts/moltdao/src/lib.rs](contracts/moltdao/src/lib.rs#L317-L333) and other call paths use direct pointer copies from external args.
  - Risk: while bounded for proposal path now, similar input-bound protections are not yet standardized across all handlers.
  - Recommendation: introduce shared bounded-copy helpers/macros and apply consistently contract-wide.

## 2026-02-23 Continuation Addendum (Bounded Input Rollout + Webhook Destination Policy)

### Newly implemented

- **Contract pointer-read hardening rolled out in priority contracts**
  - Added internal safe address-read helper (`read_address32`) and replaced direct external pointer copies in:
    - [contracts/moltdao/src/lib.rs](contracts/moltdao/src/lib.rs)
    - [contracts/compute_market/src/lib.rs](contracts/compute_market/src/lib.rs)
    - [contracts/clawpay/src/lib.rs](contracts/clawpay/src/lib.rs)
  - MoltDAO now also uses bounded variable-length reader (`read_bounded_bytes`) for proposal and execute paths, with max-size enforcement before allocation/copy.

- **Custody webhook destination allowlist added**
  - Added optional config `CUSTODY_WEBHOOK_ALLOWED_HOSTS` to [custody/src/main.rs](custody/src/main.rs).
  - Webhook registration now validates destination host membership when allowlist is configured (while preserving local dev `http://localhost` support).
  - Added URL host parser/validator helpers and integrated validation into `POST /webhooks` path.

### Validation run

- `cargo check --manifest-path contracts/moltdao/Cargo.toml` ✅
- `cargo check --manifest-path contracts/compute_market/Cargo.toml` ✅
- `cargo check --manifest-path contracts/clawpay/Cargo.toml` ✅
- `cargo check --manifest-path custody/Cargo.toml` ✅

### Residual note

- Contract pointer safety is now materially improved in the three highest-risk contract surfaces, but full standardization across all contract crates remains a recommended follow-up for complete uniformity.

## Executive Summary

The codebase contains strong hardening work in several areas (notably `core/src/state.rs` RocksDB tuning and runtime compute controls), but there are still production blockers and scale risks:

- 1 critical crypto-integrity risk (placeholder ZK path, default-off but present)
- 3 high risks (consensus-bypass write path, frontend XSS surface, custody storage tuning gap)
- 6 medium risks (scan complexity, unbounded growth, contract ABI consistency, contract input bounds)
- 1 low risk (wallet provider message-channel trust boundary)

---

## Findings

## 1) CRITICAL — Placeholder ZK proof path exists in production code

- Evidence:
  - [core/src/privacy.rs](core/src/privacy.rs#L1-L6)
  - [core/src/privacy.rs](core/src/privacy.rs#L23-L24)
  - [core/src/privacy.rs](core/src/privacy.rs#L68-L76)
- Why this matters:
  - The implementation explicitly states proof verification is placeholder/forgeable logic.
  - Although default behavior rejects proofs unless opt-in, this still represents a critical integrity hazard if toggled or reused incorrectly in production workflows.
- Recommendation:
  - Keep shielded operations hard-disabled at feature-gate level until real Groth16/PLONK verification is integrated.
  - Add a compile-time production guard that forbids enabling placeholder verification in release builds.

## 2) HIGH — RPC endpoint performs direct contract-storage writes (consensus bypass path)

- Evidence:
  - [rpc/src/prediction.rs](rpc/src/prediction.rs#L675-L684)
  - [rpc/src/prediction.rs](rpc/src/prediction.rs#L783-L823)
- Why this matters:
  - Endpoint writes directly to contract storage and documents this as consensus bypass.
  - Guarding by single-validator mode reduces risk but makes correctness depend on deployment mode and operator discipline.
- Recommendation:
  - Restrict all state mutation to transaction execution path only.
  - Keep this endpoint as admin/dev-only tooling behind explicit compile/runtime feature flags, disabled on multi-validator and production profiles.

## 3) HIGH — Wallet popup injects unescaped RPC-derived values into innerHTML

- Evidence:
  - [wallet/extension/src/popup/popup.js](wallet/extension/src/popup/popup.js#L594-L605)
  - [wallet/extension/src/popup/popup.js](wallet/extension/src/popup/popup.js#L668-L689)
- Why this matters:
  - Dynamic fields such as transaction signature/type/address are rendered through template strings to innerHTML.
  - If upstream data is malicious or compromised, this creates extension-context XSS risk.
- Recommendation:
  - Apply strict HTML escaping for all interpolated fields or switch to DOM node construction with textContent.
  - Add a shared sanitizer utility and lint rule to block unsafe template-to-innerHTML patterns.

## 4) HIGH — Custody RocksDB column families use default options despite heavy indexed/query workload

- Evidence:
  - [custody/src/main.rs](custody/src/main.rs#L753-L776)
  - [custody/src/main.rs](custody/src/main.rs#L779)
- Why this matters:
  - All custody CFs are opened with Options::default (no prefix extractor, no bloom tuning, no per-CF compaction/write tuning).
  - This diverges from production-grade state-store tuning already used in core.
- Recommendation:
  - Introduce per-CF options by access pattern (point read vs prefix scan vs write-heavy), including prefix extractors for index keys and cache tuning.
  - Establish benchmark baselines for deposit/job/event workloads before mainnet traffic.

## 5) MEDIUM — Multiple custody flows still rely on full-table iterator scans

- Evidence:
  - [custody/src/main.rs](custody/src/main.rs#L3577-L3586)
  - [custody/src/main.rs](custody/src/main.rs#L3651-L3661)
  - [custody/src/main.rs](custody/src/main.rs#L3690-L3700)
  - [custody/src/main.rs](custody/src/main.rs#L4720-L4730)
  - [custody/src/main.rs](custody/src/main.rs#L4835-L4848)
  - [custody/src/main.rs](custody/src/main.rs#L4933-L4944)
  - [custody/src/main.rs](custody/src/main.rs#L6933-L6944)
- Why this matters:
  - Even with status-index improvements, fallback and some operational paths remain O(total records).
  - This risks latency spikes and resource contention as custody volume grows.
- Recommendation:
  - Add migration completion markers to retire fallback scans once indexed state is complete.
  - Introduce dedicated secondary indexes for reserve/webhook/event queries that are currently full-scan.

## 6) MEDIUM — RPC MoltyID vouch retrieval contains acknowledged O(n²) scan path

- Evidence:
  - [rpc/src/lib.rs](rpc/src/lib.rs#L6896-L6901)
  - [rpc/src/lib.rs](rpc/src/lib.rs#L6902-L6921)
- Why this matters:
  - Complexity grows with identities × vouches; this is a direct scalability bottleneck.
- Recommendation:
  - Implement reverse index keyspace for given-vouches, as already noted in TODO comment.
  - Add performance SLO threshold tests for identity graph endpoints.

## 7) MEDIUM — RPC stats endpoint performs large storage sweep for aggregation

- Evidence:
  - [rpc/src/lib.rs](rpc/src/lib.rs#L7406-L7413)
- Why this matters:
  - Endpoint requests up to 100,000 storage entries for derived statistics.
  - Under growth, this becomes expensive and can affect RPC tail latency.
- Recommendation:
  - Maintain contract-side aggregate counters or cached materialized stats refreshed by slot/event.

## 8) MEDIUM — Runtime documentation says cross-contract calls are stubbed, but implementation is active

- Evidence:
  - [core/src/contract.rs](core/src/contract.rs#L629-L632)
  - [core/src/contract.rs](core/src/contract.rs#L1597-L1610)
- Why this matters:
  - This is a high-risk documentation/code drift in a sensitive execution surface.
  - Operators/auditors may make wrong assumptions about call graph, reentrancy, or risk boundaries.
- Recommendation:
  - Update runtime security docs and threat model to match implemented behavior and depth limits.

## 9) MEDIUM — Contract success semantics are intentionally inconsistent (0/1/value), increasing integration risk

- Evidence:
  - [core/src/contract.rs](core/src/contract.rs#L1073-L1081)
- Why this matters:
  - Runtime records return code but does not enforce a single success convention.
  - SDKs and external integrations can mis-handle edge cases across contracts.
- Recommendation:
  - Define one canonical ABI success contract and provide adapter layer for legacy contracts.
  - Enforce at CI with ABI conformance tests.

## 10) MEDIUM — No account eviction path in rent logic allows persistent state growth

- Evidence:
  - [core/src/processor.rs](core/src/processor.rs#L3039-L3042)
- Why this matters:
  - Zero-balance/low-value accounts can persist indefinitely.
  - Long-term chain state growth impacts storage and node sync costs.
- Recommendation:
  - Define deterministic, consensus-safe state pruning policy for tombstoned/zero-data accounts.

## 11) MEDIUM — Contract host-pointer reads include unbounded input lengths in at least one production contract

- Evidence:
  - [contracts/moltoracle/src/lib.rs](contracts/moltoracle/src/lib.rs#L46-L57)
- Why this matters:
  - Caller-controlled lengths are used for allocation/copy with no explicit upper bound.
  - Risk includes memory pressure, trap behavior, and inconsistent failure surfaces.
- Recommendation:
  - Enforce strict max lengths for every pointer+length host entrypoint before allocation/copy.
  - Add fuzz/property tests for oversized inputs.

## 12) LOW — Wallet provider bridge uses wildcard postMessage channels

- Evidence:
  - [wallet/extension/src/content/content-script.js](wallet/extension/src/content/content-script.js#L10-L16)
  - [wallet/extension/src/content/content-script.js](wallet/extension/src/content/content-script.js#L128-L143)
  - [wallet/extension/src/content/inpage-provider.js](wallet/extension/src/content/inpage-provider.js#L56-L63)
- Why this matters:
  - Pattern is common for wallet bridges, but relies heavily on strict method gating and approval logic.
  - Broad target origin can increase attack surface if surrounding validation regresses.
- Recommendation:
  - Keep strict request validation/approval allowlists and add regression tests around origin/method enforcement.

---

## Positive Notes (Observed)

- Core state store has production-oriented RocksDB tuning and CF strategy:
  - [core/src/state.rs](core/src/state.rs#L655-L667)
  - [core/src/state.rs](core/src/state.rs#L676-L813)
- Contract runtime has explicit compute metering and unified budget checks:
  - [core/src/contract.rs](core/src/contract.rs#L600-L626)
  - [core/src/contract.rs](core/src/contract.rs#L1047-L1061)

---

## Prioritized Remediation Order (No code changes applied)

1. Remove placeholder-ZK production possibility (finding 1).
2. Eliminate or hard-disable consensus-bypass write paths in production (finding 2).
3. Fix wallet UI injection surfaces (finding 3).
4. Apply custody RocksDB CF tuning + index migration completion plan (findings 4, 5).
5. Address RPC asymptotic bottlenecks (findings 6, 7).
6. Resolve interface/documentation drift and ABI consistency gaps (findings 8, 9).
7. Add deterministic state-growth controls and contract input bounds (findings 10, 11).
