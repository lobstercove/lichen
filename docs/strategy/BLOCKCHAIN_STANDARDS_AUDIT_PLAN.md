# MoltChain Blockchain Standards Audit Plan

> Repo-wide credibility and standards audit for the post-v0.4.5 codebase.
> This document starts a new workstream after the original alignment plan was closed.

**Created:** 2026-03-19
**Status:** DRAFT — audit started, remediation planning in progress
**Goal:** identify protocol, runtime, contract, RPC, storage, oracle, bridge, and operational gaps that would make experienced blockchain engineers question MoltChain's seriousness.

---

## Why A New Audit Exists

The original [BLOCKCHAIN_ALIGNMENT_PLAN.md](./BLOCKCHAIN_ALIGNMENT_PLAN.md) closed a large amount of foundational work. That plan materially improved the chain.

This follow-up audit exists for a different reason: **credibility hardening**.

The repo is now strong enough that the biggest remaining risks are not generic bugs. They are places where:

- protocol claims are stronger than what the code can currently prove,
- trust boundaries are still transitional,
- core execution models are internally inconsistent,
- or operations still look like an advanced prototype rather than a hardened public L1.

That is the layer that outside reviewers, exchange engineers, infra teams, protocol researchers, and serious users will judge most harshly.

---

## Audit Scope

This audit is focused on standards alignment relative to serious L1 expectations, not feature brainstorming.

Primary scope:

- `core/`
- `validator/`
- `rpc/`
- `p2p/`
- `genesis/`
- `contracts/`
- `compiler/`
- `sdk/`
- custody / oracle / deployment trust boundaries

Reference standards used implicitly in this audit:

- CometBFT / Tendermint style BFT safety and evidence
- Ethereum / beacon-chain style signed header integrity and light-client proofs
- Solana / SVM style transaction and program model clarity
- CosmWasm / NEAR style contract ABI, result semantics, and deterministic genesis discipline
- mature open-source blockchain operational expectations around custody, secrets, governance, and public claims

---

## Executive Summary

MoltChain is no longer a toy codebase. The core architecture now contains real BFT machinery, commit evidence, validator lifecycle work, state-proof primitives, and a growing contract platform.

The remaining credibility risks cluster in five areas:

1. **Authenticated sync and signed consensus boundaries are still weaker than the public BFT / light-client story implies.**
2. **Finality and proof claims are stronger than the implementation currently guarantees.**
3. **The contract platform does not yet expose one coherent execution, ABI, and storage model.**
4. **Bridge custody, oracle authority, and admin mutation paths still rely on transitional trust assumptions.**
5. **Operational transparency and repo hygiene still mix public-chain posture with private or local-only processes.**

If these remain unresolved, the project risks being seen as technically ambitious but architecturally uneven.

---

## Findings Summary

| ID | Severity | Area | Finding |
|---|---|---|---|
| C-1 | CRITICAL | Consensus | `validators_hash` is present in headers but not covered by the signed block hash |
| C-2 | CRITICAL | Sync | Initial sync and warp sync still trust peer-provided data too much |
| C-3 | CRITICAL | Contracts | Cross-contract dispatch is inconsistent and appears structurally broken in key protocol flows |
| H-1 | HIGH | Consensus | Commit certificates are not self-contained enough for exact independent verification |
| H-2 | HIGH | Finality | Public BFT/finality claims still do not match the commitment model exposed in code and RPC |
| H-3 | HIGH | State Proofs | Current account proofs are useful but over-claimed relative to production authenticated-state standards |
| H-4 | HIGH | Genesis | Genesis creation is still privileged and not cleanly reproducible from fully deterministic artifacts |
| H-5 | HIGH | Contracts | Contract state is split between serialized account storage and a separate storage backend |
| H-6 | HIGH | Custody | Threshold / multisig custody claims exceed the production-ready implementation |
| H-7 | HIGH | Oracle | Oracle authority is split between validator-attested and legacy feeder / Binance paths |
| M-1 | MEDIUM | Storage | So-called atomic block commit still spans multiple writes |
| M-2 | MEDIUM | RPC/Admin | Bearer-token admin mutation RPCs remain a non-standard control plane |
| M-3 | MEDIUM | P2P | Discovery improved, but dissemination still behaves mostly like flat broadcast |
| M-4 | MEDIUM | Security Ops | Secrets, deployment status, and some critical docs still live in local-only or ignored paths |

---

## Detailed Findings

### C-1. Unsigned Validator-Set Commitment

**Severity:** CRITICAL

**Problem**

The chain now carries `validators_hash` in the block header, but the signable block hash still excludes it. That means the field exists as metadata without full signature-domain protection.

**Why this matters**

A validator-set commitment that is not fully signed does not give light clients or auditors the property it claims to provide.

**Primary files**

- `core/src/block.rs`
- `validator/src/main.rs`

**Required direction**

- include `validators_hash` in the canonical signed header domain,
- version-gate legacy blocks if necessary,
- expose the signed commitment consistently through RPC.

### C-2. Sync Trust Boundary Still Too Soft

**Severity:** CRITICAL

**Problem**

Initial sync, header-only sync, and warp-sync paths still accept or self-consistency-check peer-provided data in places where a serious L1 should require authenticated commit evidence.

**Why this matters**

This is one of the fastest ways to lose credibility with protocol engineers: a joining node should not need to trust arbitrary peers more than the protocol itself.

**Primary files**

- `validator/src/sync.rs`
- `validator/src/main.rs`

**Required direction**

- require verified commit certificates before state application,
- anchor warp sync to trusted finalized headers,
- remove warning-only verification failures in authenticated paths.

### C-3. Contract Call Surface Is Not One Coherent Platform

**Severity:** CRITICAL

**Problem**

The runtime mixes named-export dispatch, opcode dispatch, partial ABI discovery, and ad hoc cross-contract call expectations. Several contracts appear to call exports that do not exist.

**Why this matters**

An open L1 contract platform must define one credible external call model. If not, the ecosystem looks improvised, and high-value contract families become hard to trust.

**Primary files**

- `core/src/contract.rs`
- `contracts/dex_core/src/lib.rs`
- `contracts/dex_governance/src/lib.rs`
- `contracts/dex_router/src/lib.rs`
- `contracts/dex_margin/src/lib.rs`

**Required direction**

- choose one canonical invocation model,
- standardize result envelopes and errors,
- align SDK codegen and runtime dispatch to the same contract ABI story.

### H-1. Commit Certificates Need Exact Verification Semantics

**Severity:** HIGH

**Problem**

Commit verification still depends on guessed or external round context rather than self-contained block evidence.

**Why this matters**

Independent verification should be exact. Guessing rounds is not standards-grade BFT evidence handling.

**Required direction**

- persist commit round in committed metadata,
- expose it via RPC,
- make verifier logic strict rather than heuristic.

### H-2. Finality Story Still Overstates What The Implementation Means

**Severity:** HIGH

**Problem**

The repo still mixes BFT-final messaging with a processed / confirmed / finalized model that includes a 32-slot depth layer.

**Why this matters**

Outside users will treat this as either imprecision or marketing drift unless the model is simplified or described honestly.

**Required direction**

- either adopt a cleaner BFT-final semantics model,
- or update README, SDK docs, RPC docs, and product claims to match the actual commitment model.

### H-3. State Proofs Exist But Are Over-Claimed

**Severity:** HIGH

**Problem**

Current account proofs are closer to ordered-current-state inclusion proofs than to a mature authenticated-state scheme with strong existence and non-existence semantics.

**Why this matters**

Light-client and proof claims are judged by exact proof properties, not by rough equivalence.

**Required direction**

- either narrow the public claim,
- or complete the move to a production authenticated-state structure with height-anchored proofs.

### H-4. Genesis Still Does Too Much Privileged, Live-Time Work

**Severity:** HIGH

**Problem**

Genesis still depends on live HTTP data, wall-clock conditions, and direct privileged contract-state mutation.

**Why this matters**

Serious L1 genesis should be reproducible from versioned inputs, not partly assembled from live side effects.

**Primary files**

- `genesis/src/main.rs`
- `genesis/src/lib.rs`

**Required direction**

- eliminate live network I/O from genesis,
- freeze manifests and inputs,
- restrict privileged initialization to explicit, auditable genesis-only hooks.

### H-5. Contract Storage Has Two Sources Of Truth

**Severity:** HIGH

**Problem**

The runtime and processor currently rely on both serialized `ContractAccount` storage and a separate contract-storage backend.

**Why this matters**

Dual-authority storage models are fragile, hard to audit, and likely to drift.

**Required direction**

- define one canonical contract state representation,
- ensure execution and persistence both read from the same source of truth,
- derive caches from canonical storage instead of mirroring live authority.

### H-6. Custody Trust Model Is Over-Claimed

**Severity:** HIGH

**Problem**

The public posture still implies threshold / multisig custody readiness, while the production path still depends on unsafe or effectively centralized assumptions.

**Why this matters**

Bridge and custody trust models are among the first external credibility tests any chain fails or passes.

**Required direction**

- either downgrade all public claims immediately,
- or complete the threshold signing architecture and remove unsafe production overrides.

### H-7. Oracle Authority Is Transitional And Split

**Severity:** HIGH

**Problem**

The codebase contains both validator-attested oracle machinery and legacy feeder/Binance-dependent paths, with important economics-sensitive logic still reading transitional sources.

**Why this matters**

If nobody can answer “what is the canonical source of truth for price data?” in one sentence, the oracle layer is not mature enough.

**Required direction**

- select one authoritative oracle model,
- route all consensus/economic reads through it,
- define degraded-mode behavior explicitly.

### M-1. Storage Atomicity Still Overclaims

**Severity:** MEDIUM

**Problem**

The block commit path is improved, but still not truly atomic in the strict storage sense implied by comments and architecture claims.

**Required direction**

- move tip updates and finality metadata into the same durable batch as block persistence,
- or stop describing the current path as atomic.

### M-2. Bearer-Token Admin Mutation RPCs Remain Non-Standard

**Severity:** MEDIUM

**Problem**

Several admin actions still mutate protocol state through bearer-token RPCs instead of signed transactions or governed actions.

**Required direction**

- remove these from public RPC,
- constrain any remaining admin paths to offline or localhost-only maintenance flows,
- migrate state-changing actions to transaction or governance semantics.

### M-3. P2P Discovery Is Better, Dissemination Still Looks Transitional

**Severity:** MEDIUM

**Problem**

Discovery has improved, but block and transaction propagation still resembles flat broadcast more than a mature overlay.

**Required direction**

- preserve simplicity where latency matters,
- but add bounded-fanout or explicit overlay behavior for non-consensus dissemination.

### M-4. Openness And Operational Hygiene Need Hardening

**Severity:** MEDIUM

**Problem**

Critical operational docs are partly ignored or local-only, some deployment/security context is not first-class public repo state, and secret handling patterns still lean too much on repo-adjacent storage.

**Required direction**

- publish redacted but versioned architecture and ops docs,
- enforce no-secrets-in-worktree discipline,
- separate public trust model docs from private credentials.

---

## Phase Plan

### Phase 0 — Claim Corrections And Trust-Boundary Cleanup

**Objective:** stop overstating what the chain currently guarantees.

Tasks:

- align README, website, docs, and SDK wording with actual finality semantics,
- downgrade custody claims until threshold signing is real,
- narrow oracle claims until one canonical path is authoritative,
- clearly label current proof support as inclusion-proof primitives if full authenticated-state semantics are not complete.

### Phase 1 — Consensus And Sync Hardening

**Objective:** make the chain defensible to protocol engineers.

Tasks:

- sign all consensus-relevant header fields,
- make sync verification mandatory and exact,
- make warp sync anchored to authenticated checkpoints,
- persist self-contained commit evidence.

### Phase 2 — Contract Platform Unification

**Objective:** turn the contract layer into one coherent public platform.

Tasks:

- choose one dispatch model,
- standardize call success/failure and result encoding,
- replace fake system-address CCC paths with first-class host/runtime APIs,
- unify canonical contract storage,
- make ABI/IDL generation real and codegen-backed.

### Phase 3 — Deterministic Genesis And Authenticated State

**Objective:** eliminate privileged bootstrapping surprises.

Tasks:

- remove live network I/O and wall-clock dependency from genesis,
- freeze genesis manifests and deterministic deployment inputs,
- complete authenticated state-proof semantics with height anchoring,
- align storage comments, commit paths, and actual atomicity.

### Phase 4 — Service Trust Model Hardening

**Objective:** ensure non-consensus components do not undermine the chain.

Tasks:

- complete or downgrade custody threshold signing,
- converge to one canonical oracle architecture,
- remove bearer-token mutation RPCs from production posture,
- harden release, key, and secrets workflows.

### Phase 5 — Public Transparency And Open-Source Credibility

**Objective:** make the public repo tell the truth cleanly.

Tasks:

- track redacted but complete deployment/security docs in Git,
- publish a standards-oriented architecture overview,
- maintain an explicit claims-vs-code checklist for every major subsystem,
- close local-only documentation gaps that prevent independent review.

---

## Recommended Execution Order

If the goal is credibility first, not feature velocity first, the most important order is:

1. **Phase 0** — correct claims and trust-model language immediately.
2. **Phase 1** — make sync and signed-header semantics defensible.
3. **Phase 2** — unify the contract platform before adding more protocol surface.
4. **Phase 4** — harden custody/oracle/admin trust boundaries.
5. **Phase 3** — complete deterministic genesis and authenticated-state rigor.
6. **Phase 5** — publish the operational and architectural truth cleanly.

---

## What Success Looks Like

This audit is successful when an external protocol engineer can say all of the following without qualification:

- joining nodes do not trust unauthenticated peer state,
- signed headers cover all consensus-relevant commitments,
- finality semantics are precise and honest,
- contract execution has one coherent ABI and storage model,
- custody and oracle trust assumptions are explicit and justified,
- operational docs in the public repo are sufficient to evaluate the system seriously.

Until then, the highest-risk failures are not necessarily bugs. They are **credibility failures**.
