# Lichen Exchange Listing Readiness Plan

**Created:** 2026-06-29
**Status:** Testnet technical gates green; local gates, public testnet exchange validation, developer-portal publication, GitHub CI, and public technical readiness have passed. The current package is testnet-only until mainnet launch, and operations approval plus final signed external package publication remain open.
**Current signed testnet recovery release:** `v0.5.219`; keep `v0.5.215` as the rollback anchor until a newer signed rollback point is explicitly recorded.
**Scope:** Native LICN exchange integration package, public RPC/WebSocket behavior, archive/history guarantees, exchange deposit/withdrawal operations, SDK/CLI/docs parity, and listing operations pack.

## Executive Position

Lichen has enough protocol and ecosystem pieces to become exchange-integratable, but it is not yet packaged like an exchange-facing network. The work must produce an externally reviewable integration package with exact operational instructions, verified RPC behavior, stable archive guarantees, finality policy, chain metadata, incident contacts, signed-release evidence, and a local exchange simulation that proves deposits, withdrawals, reconciliation, and cleanup on a three-validator testnet.

The priority is credibility. Do not contact serious exchanges with a partial guide, stale SDK versions, vague finality language, or unproven history/archive behavior.

## Current Repo Facts To Carry Forward

These facts were updated for the 2026-06-30 signed recovery release and must be reconciled before any external package is published:

- Core protocol crates are at `0.5.219`: `core`, `rpc`, `validator`, and `cli`.
- The root `README.md` and production deployment runbook now describe `v0.5.219` as the current signed testnet recovery release while keeping `v0.5.215` as the rollback anchor.
- The mainnet launch runbook remains anchored to `v0.5.215` because mainnet is not live and is outside the public testnet recovery scope.
- `sdk/rust/Cargo.toml` package version is `0.1.5` and now depends on `lobstercove-lichen-core = "=0.5.219"` while using the local `../../core` path.
- `sdk/js/package.json` is `1.0.5`; `sdk/python/pyproject.toml` is `1.0.0`.
- Mainnet launch docs already require public RPC validators to run `--archive-mode --cold-store /var/lib/lichen/archive-mainnet` and now require a post-launch exchange handoff before any mainnet exchange package is published.
- Testnet state policy already treats account activity and transaction history as persistent user-facing indexes.
- Local full-stack testing is supported by `scripts/start-local-stack.sh testnet`, which starts the local three-validator cluster plus custody/faucet/source-chain mocks; cleanup is `scripts/stop-local-stack.sh testnet`.

## Readiness Standard

Exchange readiness means an integration engineer can perform the full LICN lifecycle without private help:

1. Generate or assign a deposit address.
2. Detect a native LICN deposit from public RPC/archive data.
3. Decide when the deposit is final enough to credit.
4. Credit an internal user ledger using 9-decimal LICN spores.
5. Build, sign, broadcast, and track a withdrawal.
6. Retry safely without duplicate credit or duplicate withdrawal.
7. Reconcile hot wallet, cold wallet, internal ledger, and on-chain state.
8. Query old blocks and transactions indefinitely through an archive-backed endpoint.
9. Find status, incident contacts, release signatures, upgrade policy, and rollback policy without asking the core team.

## Non-Negotiables

- No serious exchange outreach before the exchange simulation passes locally and on the public testnet target included in the package.
- No public testnet release for exchange work before the local e2e pass, four-validator restart/rejoin gate, and cleanup are complete.
- No exchange guide may claim archive support until `getTransaction`, `getTransactionsByAddress`, `getBlock`, latest block, and account history are verified against hot and cold archive-backed data.
- No docs may publish guessed address regexes, chain IDs, fee units, logo URLs, or finality buffers. Values must come from source code, deployed configuration, or signed release artifacts.
- No public docs may expose secrets, private RPC provider URLs, hot wallet key material, custody seeds, private contacts beyond approved incident aliases, or filled production env files.
- Any release after this work keeps `v0.5.215` as the rollback anchor until a newer signed rollback point is explicitly recorded.

## Deliverables

### 1. Exchange Integration Guide

Target files:

- `docs/guides/EXCHANGE_INTEGRATION.md`
- `developers/exchange-integration.html`

Current draft:

- [../guides/EXCHANGE_INTEGRATION.md](../guides/EXCHANGE_INTEGRATION.md)
- [../../developers/exchange-integration.html](../../developers/exchange-integration.html)

Required content:

- Network overview: chain name, ticker, native asset, 9-decimal spores model, fee unit, base RPC/WebSocket endpoints, explorer URLs.
- Address handling: Base58 account format, EVM-format mapping availability, exact regex and validation rules sourced from code.
- Native LICN deposits: deposit-address strategy, no memo/tag policy (exchange derivation for management), polling method, WebSocket option, retry behavior, idempotency keys, and credit rules.
- Native LICN withdrawals: hot wallet signing model, cold wallet sweep model, transaction construction, broadcast, confirmation polling, retry, stuck transaction handling, and reconciliation.
- Finality and confirmations: deterministic BFT finality, exact operational buffer recommendation, and high-value/manual-review policy.
- Archive requirements: exchange nodes and public RPC dependencies must be archive-backed; pruned/state-only nodes are not acceptable for listing operations.
- RPC examples: `getSlot`, `getLatestBlock`, `getBlock`, `getTransaction`, `getTransactionsByAddress`, `getBalance`, send/broadcast flow, and WebSocket subscriptions.
- Failure handling: RPC timeout, null transaction, future slot, stale endpoint, divergent endpoints, chain halt, upgrade window, and incident mode.

### 2. Checklist And Tracker

Target file:

- `docs/strategy/EXCHANGE_LISTING_READINESS_TRACKER.md`

Current tracker:

- [EXCHANGE_LISTING_READINESS_TRACKER.md](./EXCHANGE_LISTING_READINESS_TRACKER.md)

Required structure:

- One row per gate with owner, source file, validation command, evidence path, status, and release blocker flag.
- Separate sections for docs, RPC/archive, SDK/CLI, explorer, custody/DEX/oracle dependencies, operational pack, and exchange simulation.

### 3. Chain Metadata Sheet

Target file:

- `docs/guides/EXCHANGE_CHAIN_METADATA.md`

Current draft:

- [../guides/EXCHANGE_CHAIN_METADATA.md](../guides/EXCHANGE_CHAIN_METADATA.md)
- [../guides/EXCHANGE_ADDRESS_VALIDATION_VECTORS.md](../guides/EXCHANGE_ADDRESS_VALIDATION_VECTORS.md)

Required fields:

| Field                 | Required source                                                      |
| --------------------- | -------------------------------------------------------------------- |
| Chain name            | Canonical product docs                                               |
| Network names         | `seeds.json`, deployment docs, developer portal config               |
| Ticker                | Tokenomics/foundation docs                                           |
| Native asset          | Core/account docs and RPC examples                                   |
| Decimals              | 9, validated from spores conversion paths                            |
| Fee unit              | Native LICN spores                                                   |
| Address regex         | Source-derived validator, not guessed                                |
| EVM mapping           | `core/src/evm.rs` and RPC/EVM compatibility docs                     |
| RPC URLs              | `seeds.json`, developer portal shared config, public deployment docs |
| WebSocket URLs        | developer portal shared config and deployment docs                   |
| Explorer URLs         | public site config                                                   |
| Logo URL              | public asset path with cache/version policy                          |
| Status page           | operational pack                                                     |
| Release-signature URL | GitHub release and trust-anchor docs                                 |

### 4. Operational Listing Pack

Target files:

- `docs/deployment/EXCHANGE_OPERATIONS_PACK.md`
- developer portal link from the exchange guide

Current draft:

- [../deployment/EXCHANGE_OPERATIONS_PACK.md](../deployment/EXCHANGE_OPERATIONS_PACK.md)

Required content:

- Status page URL and service availability policy.
- Incident contact aliases and escalation windows.
- Upgrade policy: release cadence, maintenance windows, breaking-change notice period, and emergency exception process.
- Release verification: GitHub release URLs, `SHA256SUMS`, detached signatures, attestations, trust anchor, and binary hash evidence.
- Rollback policy: current rollback anchor `v0.5.215`, rollback decision criteria, service restart order, and expected exchange-facing impact.
- Archive policy: retention, backup/restore, repair, and public-history merge rules.
- Chain halt / delayed finality communication policy.

### 5. Local Exchange Simulation

Target implementation:

- New e2e harness or documented manual harness under `tests/` or `scripts/qa/`.
- Evidence stored under an ignored local path such as `evidence/exchange-readiness/<date>/`.

Required flow:

1. Start clean local production-parity testnet:
   `scripts/start-local-stack.sh testnet`
2. Confirm all three validators are healthy:
   `scripts/status-local-stack.sh testnet`
3. Generate exchange wallets:
   - hot wallet
   - cold wallet
   - user deposit wallet
   - withdrawal destination wallet
4. Fund the deposit source with test LICN.
5. Send native LICN to the exchange deposit wallet.
6. Detect the deposit by polling RPC and, separately, by account history.
7. Wait for finality plus the selected operational buffer.
8. Credit a simulated internal account exactly once.
9. Sweep from deposit wallet to hot wallet when applicable.
10. Withdraw from hot wallet to the user destination.
11. Poll `getTransaction` and `getTransactionsByAddress` until final.
12. Reconcile on-chain balances, internal ledger, fee spend, tx IDs, and slots.
13. Restart one validator and prove archive/history queries still work.
14. Stop and clean up:
    `scripts/stop-local-stack.sh testnet`
15. Verify no local validator, custody, faucet, or mock source-chain process remains.

Pass criteria:

- No duplicated deposits across retries.
- No duplicate withdrawal broadcast across retries.
- Exact spores arithmetic: 1 LICN equals 1,000,000,000 spores.
- Final balances reconcile to initial balances minus fees.
- `getTransaction` works for the deposit, sweep, and withdrawal after restart.
- `getTransactionsByAddress` returns complete deposit-wallet and hot-wallet history.
- The local stack is cleaned up after the run.

## Workstreams And Gates

### Phase 0: Source Map And Reality Check

Status: completed as source map in [EXCHANGE_LISTING_READINESS_TRACKER.md](./EXCHANGE_LISTING_READINESS_TRACKER.md).

Purpose: establish source-backed facts before writing external docs.

Tasks:

- Map native account/address code in `core/src/account.rs`, `core/src/signing.rs`, `core/src/transaction.rs`, and `core/src/evm.rs`.
- Map RPC/history/archive behavior in `rpc/src/lib.rs`, `rpc/src/ws.rs`, `core/src/state/*`, and archive/cold-store paths.
- Map CLI transfer/history commands in `cli/src/*transfer*`, `cli/src/*transaction*`, and account history modules.
- Map DEX/custody/oracle surfaces that may appear in listing questions: `custody/`, `rpc/src/dex.rs`, `core/src/dex.rs`, and `contracts/dex_*`.
- Record all version drift across `Cargo.toml`, SDK packages, README, developer portal, and RPC docs.

Exit gate:

- Source map and version-drift list are checked into the tracker.
- Every chain metadata field has a named source of truth.

### Phase 1: Documentation Architecture

Status: documentation architecture created, linked, deployed to the developer
portal, and verified. External exchange use remains blocked only by the
operator-approval and final package-publication gates tracked below.

Purpose: create the external package skeleton before code changes.

Tasks:

- Create the GitHub exchange guide and developer portal page.
- Create the tracker and chain metadata sheet.
- Add developer portal navigation without burying the page.
- Scope the current exchange docs as testnet-only until mainnet launch, and label unfinished mainnet values clearly.

Exit gate:

- Docs are internally linked and renderable.
- All placeholders are visible as blockers, not hidden TODOs.

### Phase 2: Finality And Confirmation Policy

Status: completed locally; public endpoint validation remains part of Phase 7.

Purpose: turn deterministic finality into exchange-operational policy.

Working policy until validated:

- A finalized block is deterministic under the BFT commitment model.
- Exchanges should still use an operational buffer after finality to protect against endpoint lag, monitoring delay, archive lag, and internal retry races.
- Candidate default: credit standard LICN deposits after finality plus 8 finalized slots.
- Candidate high-value policy: finality plus 32 finalized slots or manual review.

Tasks:

- Verify the current finalized-slot semantics through RPC and source code.
- Test finality reporting during a local validator restart.
- Confirm WebSocket transaction notifications and JSON-RPC `getTransaction` agree.
- Document exact values and when an exchange should use stricter buffers.

Exit gate:

- Finality policy is explicit in the exchange guide.
- Local e2e evidence shows the policy does not credit before finality.

### Phase 3: Archive And History Guarantee

Status: completed locally for hot/cold migration and reopen; public archive
deployment proof remains part of Phase 7.

Purpose: prove exchanges can query old transaction data indefinitely.

Tasks:

- Verify public/exchange RPC nodes run with `--archive-mode --cold-store`.
- Define "archive node" requirements for exchange-operated nodes.
- Add or run tests for:
  - `getBlock(0)`
  - `getBlock(latest)`
  - `getTransaction` for old, recent, and post-restart transactions
  - `getTransactionsByAddress` pagination and `before_slot`
  - account transaction count
  - null/future/garbage transaction handling
- Verify cold-store backed data survives restart and does not rely only on hot state.
- Document repair expectations and public-history merge rules.

Exit gate:

- Archive/history behavior is verified locally and represented in the integration guide.
- Any RPC gaps become release blockers.

### Phase 4: Local Three-Validator Exchange Simulation

Status: completed locally with cleanup evidence.

Purpose: prove the exact deposit/withdrawal cookbook before public testnet release.

Required command shape:

```bash
scripts/start-local-stack.sh testnet
scripts/status-local-stack.sh testnet
# run exchange simulation
scripts/stop-local-stack.sh testnet
```

Tasks:

- Build or document the exchange simulation runner.
- Use fresh local validator state by default.
- Exercise deposit, detection, credit, sweep, withdrawal, polling, retry, reconciliation, validator restart, and cleanup.
- Capture tx IDs, slots, balances, internal ledger events, RPC responses, and process cleanup evidence.

Exit gate:

- Simulation passes from a clean local stack.
- Cleanup evidence shows no local runtime process remains.
- Results are added to the tracker before any public testnet release.

### Phase 5: SDK, CLI, Explorer, And Developer Portal Parity

Status: completed for SDK, CLI, developer portal, explorer, and public testnet
RPC/WS checks. JavaScript remains excluded from exchange accounting until its
lossless u64 boundary is approved; Python, Rust, CLI, and raw JSON-RPC paths are
the current exchange-accounting surfaces.

Purpose: remove integration drift before exchanges read the package.

Tasks:

- Align crate/package versions or explicitly document compatibility.
- Update stale docs that still mention old release targets or RPC versions.
- Verify JS, Python, Rust SDK examples for:
  - balance
  - transfer
  - transaction confirmation
  - history lookup
  - WebSocket transaction/slot subscription
- Verify CLI examples match actual command syntax.
- Verify explorer transaction/account URLs match the metadata sheet.
- Add docs QA so the developer portal and GitHub docs do not diverge.

Exit gate:

- Version matrix is consistent and checked.
- At least one SDK and the CLI can complete the exchange simulation primitives.
- Developer portal and GitHub docs present the same exchange-facing facts.

### Phase 6: DEX, Custody, Wrapped Assets, And Oracle Cross-Check

Status: completed for native LICN scope separation; optional ecosystem claims
remain excluded from exchange-facing listing material until separately verified.

Purpose: be ready for exchange questions beyond native LICN without mixing them into the native listing path.

Tasks:

- Summarize DEX pair availability, liquidity model, and wrapped-asset custody model.
- Confirm wrapped assets and DEX routes are described as ecosystem infrastructure, not a prerequisite for native LICN deposits.
- Verify custody/oracle status endpoints and incident controls are documented in the operations pack.
- Confirm bridge/custody route failures cannot invalidate native LICN exchange deposit/withdrawal guidance.

Exit gate:

- Exchange package cleanly separates native LICN integration from DEX/wrapped-asset optional context.
- DEX/custody/oracle claims are source-backed and do not overstate live liquidity.

### Phase 7: Public Testnet Release Gate

Status: complete for testnet after signed `v0.5.219`; external package remains
blocked on Phase 8 items.

Purpose: publish only after local proof.

Preconditions:

- Local three-validator exchange simulation passed and cleaned up.
- Archive/history tests passed.
- Docs package passes link/static checks.
- SDK/CLI examples are current.
- Rollback anchor `v0.5.215` is recorded.
- Public testnet RPC is healthy and not stale/readiness-gated.
- Mainnet RPC/WebSocket/archive checks are deferred because the package is
  explicitly scoped to testnet-only integration testing before mainnet launch.
  They become blocking again during the mainnet launch exchange handoff.
- Public developer portal exchange page serves exchange-specific testnet-only
  content.
- Status page, incident aliases, and target exchange-package release tag are
  approved.

Tasks:

- Prepare signed release or deployment candidate.
- Deploy to public testnet only after local gates pass.
- Re-run the exchange simulation against public testnet with tiny amounts.
- Record rollback command path and expected service impact.

2026-06-30 result:

- Signed `v0.5.219` was published, checksum/signature verified, and deployed
  through the rolling release runbook.
- All four live testnet validators and CLIs reported `0.5.219`; runbook
  verify-only completed `RELEASE VERIFY COMPLETE`.
- Public RPC, WebSocket `subscribeSlots`, faucet health/status, DEX
  oracle/candle smoke, and faucet-backed exchange simulation passed.

Exit gate:

- Public testnet evidence matches local evidence.
- Remaining release blockers are explicitly external-package blockers, not
  testnet recovery blockers.

### Phase 8: External Listing Package

Status: blocked on operations approval and final external package publication.
Developer-portal publication is complete. Mainnet is excluded from the current
package until the mainnet launch exchange handoff closes; EVM wording is
reconciled for native listings.

Purpose: produce the package an exchange can review without backchannel dependency.

Contents:

- Exchange integration guide URL.
- Chain metadata sheet.
- RPC/WebSocket endpoint sheet.
- Explorer URL patterns.
- Logo assets.
- Status page and incident contacts.
- Upgrade and rollback policy.
- Release verification instructions.
- Archive node requirements.
- Deposit/withdrawal cookbook.
- Finality and confirmation policy.
- Known limitations and support escalation path.

Exit gate:

- Package reviewed internally.
- No stale version references remain.
- Go/no-go decision is documented before outreach.

Mainnet handoff after launch:

- Verify mainnet public RPC, WebSocket, archive/history, native chain ID, `/evm`
  `eth_chainId`, explorer routes, status page coverage, and incident contacts.
- Run `scripts/qa/exchange_public_readiness.py --scope full`.
- Perform operator-approved mainnet dust deposit, withdrawal, and reconciliation
  through approved exchange test accounts. There is no mainnet faucet.
- Replace the testnet-only package label only after signed evidence is recorded
  and a new external package tag is selected.

## Risk Register

| Risk                                  | Impact                                                            | Mitigation                                                          |
| ------------------------------------- | ----------------------------------------------------------------- | ------------------------------------------------------------------- |
| Stale release/docs version references | Exchanges see inconsistency and lose confidence                   | Version matrix and docs QA before publication                       |
| Address regex guessed incorrectly     | Deposits rejected or misrouted                                    | Derive validation rules from source and test vectors                |
| Archive/history gap                   | Exchange cannot reconcile deposits or customer disputes           | Archive-mode gates, cold-store verification, restart tests          |
| Finality policy too vague             | Exchange chooses unsafe or overly conservative confirmation count | Source-backed finality docs plus explicit operational buffer        |
| SDK package drift                     | Integration examples fail                                         | Version consistency pass and smoke tests                            |
| DEX/custody confusion                 | Native LICN listing appears dependent on bridge routes            | Separate native LICN integration from optional ecosystem context    |
| Retry/idempotency gaps                | Duplicate credits or withdrawals                                  | Simulation must include retries and ledger idempotency checks       |
| Rollback unproven                     | Incident response looks improvised                                | Keep `v0.5.215` rollback anchor until superseded by signed evidence |

## Initial Tracker

| Gate                                 | Status      | Release blocker | Evidence                         |
| ------------------------------------ | ----------- | --------------- | -------------------------------- |
| Source map completed                 | Done        | No              | Tracker                          |
| Version drift resolved or documented | Done        | No              | Version matrix                   |
| Exchange guide skeleton              | Done        | Yes             | GitHub docs and developer portal |
| Chain metadata sheet                 | Done        | Yes             | Docs                             |
| Finality policy validated            | Done        | No              | Local e2e evidence               |
| Archive/history validated            | Done        | No              | Local e2e evidence               |
| Local exchange simulation passed     | Done        | No              | Local evidence directory         |
| Local cleanup verified               | Done        | No              | Process/status evidence          |
| SDK/CLI examples verified            | Done        | No              | Test output                      |
| Explorer URL patterns verified       | Done        | No              | Metadata sheet                   |
| Operational listing pack completed   | In progress | Yes             | Deployment docs                  |
| Public testnet exchange run passed   | Done        | No              | Testnet evidence                 |
| External listing package reviewed    | Blocked     | Yes             | Go/no-go record                  |

## Go/No-Go Rule

The only acceptable "go" state is boring: docs current, tests passing, local and public testnet evidence recorded, archive behavior verified, rollback known, and no unresolved release-blocker row in the tracker. Anything else is a no-go for exchange outreach.
