# MoltChain Production Audit Findings (Documentation Only)

Date: 2026-02-22  
Scope reviewed: core runtime, contracts, RPC/WS paths, custody service, wallet frontend extension  
Constraint honored: findings and recommendations only, no code changes

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
