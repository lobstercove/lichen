# Mainnet Readiness Full-Pass Plan - 2026-04-25

Durable root-level artifact for the repo-wide mainnet readiness audit restart.

## Purpose

This is the execution plan for the final repo-wide Lichen readiness pass before
mainnet work resumes.

The goal is not to rewrite a working testnet. The goal is to prove, with fresh
evidence, that the repository is internally consistent, production-shaped,
security hardened, and credible to experienced blockchain developers reviewing
the code, contracts, RPC/API surfaces, frontends, documentation, and operator
flows.

No regression is acceptable. All work must proceed in small, reversible slices.
Full workspace builds/tests are intentionally excluded from the default flow
because this machine can run out of memory. Use module-scoped checks first, then
only widen deliberately.

## Current Baseline

- Date: 2026-04-25
- Branch: `main`
- HEAD at reconnaissance start: `7889301 (HEAD -> main, origin/main, origin/HEAD) Contracts`
- Worktree at reconnaissance start: clean
- Active repo status docs and handovers center on `v0.5.9`
- Live testnet handover records 3 VPS validators running the exact published
  `v0.5.9` Linux validator release artifact
- Cargo workspace members:
  - `core`
  - `validator`
  - `rpc`
  - `cli`
  - `p2p`
  - `faucet-service`
  - `custody`
  - `genesis`
- Contracts:
  - 29 directories under `contracts/`
  - 28 contracts in `GENESIS_CONTRACT_CATALOG`
  - `mt20_token` remains in-tree but outside genesis
- Frontend portals:
  - `wallet`
  - `explorer`
  - `dex`
  - `marketplace`
  - `developers`
  - `programs`
  - `monitoring`
  - `faucet`
  - `website`

## Reconnaissance Evidence

Commands run during this planning pass:

```bash
git status --short --branch
git ls-files | awk -F/ '{count[$1]++} END {for (d in count) print count[d], d}' | sort -nr
rg --files -g 'Cargo.toml' -g 'package.json' -g 'wrangler.toml' -g 'vite.config.*' -g 'tsconfig*.json' -g 'pyproject.toml' -g 'requirements*.txt' -g 'Dockerfile*' -g 'docker-compose*.yml' -g '.github/workflows/*.yml'
find contracts -mindepth 2 -maxdepth 2 -name Cargo.toml | sed 's#/Cargo.toml##' | sort
cargo metadata --no-deps --format-version 1
cargo fmt --all -- --check
python3 scripts/qa/update-expected-contracts.py --check
npm run test-frontend-assets
git ls-files | rg '\.(js|mjs)$' | rg -v '(^dex/charting_library/|package-lock\.json$)' | xargs -n 1 node --check
node --check scripts/qa/audit_frontend_rpc_parity.js
npm run audit-frontend-rpc-parity
```

Low-memory validation results:

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `python3 scripts/qa/update-expected-contracts.py --check` | Passed, 28 discovered / 28 locked |
| `npm run test-frontend-assets` | Passed, 244 asset checks + 2 shared-helper drift checks |
| `node --check` over first-party JS/MJS | Failed on `sdk/js/test_cross_sdk_compat.js` duplicate `encodeBytes` declaration |
| `node --check scripts/qa/audit_frontend_rpc_parity.js` | Passed |
| `npm run audit-frontend-rpc-parity` | Expected audit failure: 8 unknown live frontend RPC methods remain |

Part 1 close-out validation after the SDK and checker patches:

| Check | Result |
| --- | --- |
| `node --check sdk/js/test_cross_sdk_compat.js` | Passed |
| `node sdk/js/test_cross_sdk_compat.js` | Passed |
| `cd sdk/js && npm run test` | Passed |
| `python3 scripts/qa/update-expected-contracts.py --check` | Passed |
| `npm run test-frontend-assets` | Passed |
| first-party `node --check` sweep | Passed |
| `git diff --check` | Passed |
| `npm run audit-frontend-rpc-parity` | Expected audit failure until 8 unknown live frontend RPC methods are resolved |

Initial scale inventory from tracked first-party source/docs:

| Area | Approx lines | Files counted |
| --- | ---: | ---: |
| `contracts` | 76,187 | 66 |
| `docs` | 63,483 | 139 |
| `core` | 61,639 | 91 |
| `wallet` | 42,636 | 68 |
| `rpc` | 35,686 | 12 |
| `developers` | 27,811 | 24 |
| `dex` | 26,722 | 11 |
| `programs` | 24,495 | 17 |
| `sdk` | 22,804 | 78 |
| `validator` | 22,082 | 10 |
| `explorer` | 20,395 | 32 |
| `custody` | 16,709 | 127 |
| `marketplace` | 14,360 | 20 |
| `monitoring` | 14,330 | 13 |
| `scripts` | 12,036 | 41 |

Large-file maintainability hotspots observed in tracked source:

- `rpc/src/lib.rs` - 18,732 lines
- `validator/src/main.rs` - 16,418 lines
- `core/src/processor.rs` - 9,652 lines
- `programs/js/playground-complete.js` - 9,225 lines
- `dex/dex.js` - 8,026 lines
- `contracts/lichenid/src/lib.rs` - 7,793 lines
- `core/src/consensus.rs` - 6,235 lines
- `contracts/prediction_market/src/lib.rs` - 6,136 lines
- `contracts/dex_core/src/lib.rs` - 5,481 lines
- `wallet/js/wallet.js` - 4,933 lines

## External Standards To Use As Fresh Reference

Use current primary sources during execution, not stale memory:

- NIST FIPS 203, ML-KEM: <https://csrc.nist.gov/pubs/fips/203/final>
- NIST FIPS 204, ML-DSA: <https://csrc.nist.gov/pubs/fips/204/final>
- NIST FIPS 205, SLH-DSA: <https://csrc.nist.gov/pubs/fips/205/final>
- NIST PQC announcement: <https://www.nist.gov/news-events/news/2024/08/nist-releases-first-3-finalized-post-quantum-encryption-standards>
- Tendermint consensus spec: <https://docs.tendermint.com/master/spec/consensus/>
- Tendermint BFT time: <https://docs.tendermint.com/master/spec/consensus/bft-time.html>
- Ethereum execution JSON-RPC spec: <https://ethereum.github.io/execution-apis/>
- Solana RPC docs: <https://solana.com/docs/rpc/http>
- OWASP API Security Top 10 2023: <https://owasp.org/API-Security/editions/2023/en/0x11-t10/>
- OWASP ASVS: <https://github.com/OWASP/ASVS>
- OWASP WSTG: <https://owasp.org/www-project-web-security-testing-guide/>
- SLSA 1.2: <https://slsa.dev/spec/v1.2/about>
- OpenSSF Scorecard: <https://github.com/ossf/scorecard>

## Non-Negotiable Guardrails

1. Keep the worktree clean between slices unless a deliberate patch is in
   progress.
2. Do not run `cargo check --workspace`, `cargo test --workspace`, `make test`,
   or all-contract builds by default.
3. Prefer one crate, one contract family, or one portal per validation step.
4. If a check needs long-running local validators, use
   `scripts/start-local-stack.sh testnet` for production-parity flows and keep
   its parent shell alive.
5. Do not touch live VPS services during the repo audit unless a later explicit
   deployment task is opened.
6. Do not change deployment status docs unless deployment reality changes.
7. Treat `monitoring/shared/` as canonical for shared frontend helpers.
8. Every discovered issue gets severity, evidence, blast radius, and a scoped
   remediation proposal before code changes.
9. Any code change gets focused validation before moving to the next module.
10. Keep durable notes after each part so compaction or session loss does not
    discard context.

## Initial Findings From Reconnaissance

These are not the final audit findings. They are known items to carry into the
execution tracker.

### R-01 - JS SDK Golden-Vector Test Has A Syntax Error

- Severity: Medium
- Location: `sdk/js/test_cross_sdk_compat.js:43` and `sdk/js/test_cross_sdk_compat.js:81`
- Evidence: `node --check` fails with `SyntaxError: Identifier 'encodeBytes' has already been declared`
- Impact: this is not in the deployed portals, but it weakens SDK release and
  cross-SDK compatibility evidence.
- First action: remove the duplicate helper or consolidate it, then run:

```bash
node --check sdk/js/test_cross_sdk_compat.js
node sdk/js/test_cross_sdk_compat.js
cd sdk/js && npm run test
```

### R-02 - Wallet Extension Readiness Plan Still Lists Open Gaps

- Severity: High for mainnet sign-off, Medium for current public testnet
- Location: `wallet/EXTENSION_PRODUCTION_READINESS_PLAN.md`
- Open items recorded there:
  - bundled local QR generator still missing
  - full-page extension token send parity is incomplete
  - several sensitive flows still use browser prompts instead of secure modals
  - shielded extension flows need live RPC validation
  - popup/full-page visual parity is partial
- First action: verify whether each item is still current before treating the
  wallet extension as mainnet-ready.

### R-03 - Documentation Recency Is Uneven

- Severity: Medium
- Evidence:
  - `memories/repo/current-state.md` and latest handovers say `v0.5.9`
  - `DEPLOYMENT_STATUS.md` is updated to 2026-04-23 but still contains older
    TODO rows in early deployment phases
  - `docs/foundation/ROADMAP.md` is February-oriented and stale relative to the
    live 3-VPS testnet status
  - `docs/strategy/PHASE2_ACTIVATION_PLAN.md` begins from February assumptions
- Impact: public or developer-facing claims can be technically correct in one
  file and stale in another.
- First action: build a doc/date reconciliation matrix before editing docs.

### R-04 - Large Monolith Files Remain A Maintainability Risk

- Severity: Medium
- Evidence: file-size inventory above
- Impact: auditability, regression risk, and new-contributor credibility suffer
  even if current behavior is correct.
- First action: do not refactor during the first audit pass; instead record
  module-specific extraction candidates with tests required to preserve behavior.

### R-05 - RPC/Frontend Method Parity Needs A Purpose-Built Static Checker

- Severity: Medium
- Evidence: `scripts/qa/audit_frontend_rpc_parity.js` now parses current
  server dispatch tables and scans tracked first-party frontend JavaScript.
- Impact: the current repo has strong frontend asset checks, but no precise
  automated proof that every live portal action maps to an implemented backend
  endpoint with documented response and error handling.
- Current checker result: `npm run audit-frontend-rpc-parity` exits nonzero on
  current code with 8 unknown live frontend RPC methods:
  - `getMarketplaceConfig`
  - `getShieldedNotes`
  - `sendShieldedTransaction`
  - `submitProgramVerification`
  - `submitShieldTransaction`
  - `submitUnshieldTransaction`
  - `submitShieldedTransfer`
- False positives already ruled out:
  - config URL helpers such as `LICHEN_CONFIG.rpc('mainnet')`
  - WebSocket keepalive `ping`
  - HTTP method strings such as `POST`
  - wallet-provider local APIs such as `licn_*` and `wallet_*`
- Next action: decide whether each unknown should become server RPC support,
  frontend wiring to an existing supported RPC/contract path, or unavailable UI
  with explicit handling.
- The checker classifies calls as:
  - native JSON-RPC
  - Solana-compat RPC
  - EVM RPC
  - WebSocket subscription
  - custody/faucet REST
  - wallet-provider local API
  - contract method name
  - documentation-only example

## Execution Parts

Each part should end with a short durable note in either this file, a dedicated
tracker, or `memories/repo/session-handovers/` if non-obvious context was
created.

### Part 1 - Audit Harness And Evidence Ledger

Deliverables:

- Create `MAINNET_READINESS_EXECUTION_TRACKER_2026-04-25.md` at repo root.
  New files under `docs/` and `memories/` are ignored in this repo, so the
  root location keeps the artifact visible in `git status`.
- Record every command run, elapsed result, and whether it was read-only.
- Add a narrowly scoped RPC/frontend parity checker.
- Fix `sdk/js/test_cross_sdk_compat.js` if the duplicate helper is confirmed as
  a simple no-regression patch.

Validation:

```bash
git status --short --branch
node --check sdk/js/test_cross_sdk_compat.js
node sdk/js/test_cross_sdk_compat.js
python3 scripts/qa/update-expected-contracts.py --check
npm run test-frontend-assets
node --check scripts/qa/audit_frontend_rpc_parity.js
npm run audit-frontend-rpc-parity
```

### Part 2 - Consensus, State, Transactions, And PQ Cryptography

Scope:

- `core/src/block.rs`
- `core/src/consensus.rs`
- `core/src/transaction.rs`
- `core/src/state.rs` and `core/src/state/*`
- `core/src/processor.rs` and `core/src/processor/*`
- `core/src/keypair_file.rs`
- `core/src/zk/*`
- `validator/src/consensus.rs`
- `validator/src/block_producer.rs`
- `validator/src/block_receiver.rs`
- `validator/src/wal.rs`

Audit questions:

- Are block signatures, validator-set hashes, commit certificates, and finality
  commitments self-contained and independently verifiable?
- Are replay protection, durable nonce behavior, transaction hash semantics, fee
  charging, rent, burn/mint accounting, and slashing internally consistent?
- Is ML-DSA usage aligned with FIPS 204 naming, parameter choices, domain
  separation, serialization, and failure behavior?
- Is ML-KEM usage isolated to transport/session establishment and aligned with
  the intended threat model?
- Is SLH-DSA fallback/support accurately documented and tested where claimed?
- Are shielded proof envelopes, commitments, nullifiers, note encryption, and
  Merkle paths impossible to bypass through malformed RPC or transaction input?
- Are state-root, checkpoint, archive, and account-proof semantics precise
  enough for light-client credibility?

Validation slices:

```bash
cargo check -p lobstercove-lichen-core --tests
cargo test -p lobstercove-lichen-core --test production_readiness -- --nocapture
cargo test -p lobstercove-lichen-core --test wire_format -- --nocapture
cargo test -p lobstercove-lichen-core --test state_commitment -- --nocapture
cargo test -p lobstercove-lichen-core --test zk_lifecycle -- --nocapture
cargo test -p lichen-validator --lib consensus -- --nocapture
```

### Part 3 - Validator, P2P, Sync, Warp Checkpoints, And Operations

Scope:

- `validator/src/main.rs`
- `validator/src/sync.rs`
- `validator/src/updater.rs`
- `validator/src/threshold_signer.rs`
- `p2p/src/*`
- `deploy/*`
- `scripts/start-local-3validators.sh`
- `scripts/start-local-stack.sh`
- `scripts/clean-slate-redeploy.sh`
- `scripts/health-check.sh`
- `scripts/validators.json`
- `seeds.json`

Audit questions:

- Does startup fail closed for unknown network, missing genesis, plaintext keys,
  and unsafe helper use outside local dev?
- Are P2P node identity, validator identity, handshake auth, ban policy, peer
  diversity, gossip validation, and request throttling coherent?
- Does sync avoid accepting spoofed checkpoints, mismatched roots, stale peers,
  or unauthenticated state chunks?
- Does auto-update require the intended release signature and canary discipline?
- Are Caddy, firewall, systemd, raw port exposure, and environment files aligned
  with the current deployment runbook?

Validation slices:

```bash
cargo check -p lichen-validator -p lichen-p2p --tests
cargo test -p lichen-validator -- --nocapture
cargo test -p lichen-p2p -- --nocapture
bash scripts/qa/test_local_helper_guards.sh
```

### Part 4 - RPC, REST, WebSocket, Compatibility APIs, And Rate Limits

Scope:

- `rpc/src/lib.rs`
- `rpc/src/ws.rs`
- `rpc/src/dex.rs`
- `rpc/src/dex_ws.rs`
- `rpc/src/shielded.rs`
- `rpc/src/launchpad.rs`
- `rpc/src/prediction.rs`
- `rpc/tests/*`
- `scripts/qa/e2e-rpc-coverage.js`
- `scripts/qa/test-rpc-comprehensive.sh`

Audit questions:

- Is every public method intentionally categorized as native, Solana-compatible,
  EVM-compatible, REST, or WebSocket?
- Are admin mutations hard-disabled on public networks as intended?
- Are rate-limit tiers appropriate for reads, index scans, simulation, writes,
  shielded proof helpers, and bridge-deposit creation?
- Are response schemas stable, documented, and reflected in developer portal
  pages and SDKs?
- Are CORS and error responses safe for public exposure?
- Are Solana/EVM compatibility claims accurate and not overstated?

Validation slices:

```bash
cargo check -p lichen-rpc --tests
cargo test -p lichen-rpc --test rpc_full_coverage -- --nocapture
cargo test -p lichen-rpc --test shielded_handlers -- --nocapture
node scripts/qa/e2e-rpc-coverage.js # only with local stack running
```

### Part 5 - Contracts And Genesis Catalog

Scope:

- `contracts/*/src/lib.rs`
- contract-local tests
- `genesis/src/lib.rs`
- `scripts/build-all-contracts.sh`
- `scripts/qa/expected-contracts.json`
- `docs/contracts/*`
- `developers/contract-reference.html`

Audit questions:

- Does each genesis contract have documented exports/opcodes, tests, and frontend
  or RPC surfaces where applicable?
- Is `mt20_token` intentionally excluded from genesis and documented that way?
- Are all admin paths gated, pause controls real, fee parameters bounded,
  custody addresses explicit, and arithmetic overflow-safe?
- Are token/reserve/bridge/oracle/prediction/DEX/storage contracts economically
  conservative under adversarial ordering?
- Are unsafe pointer reads in WASM entrypoints bounded and uniform enough to
  defend in a professional audit?

Validation slices:

```bash
python3 scripts/qa/update-expected-contracts.py --check
cd contracts/<name> && cargo check
cd contracts/<name> && cargo test --release
cd contracts/<name> && cargo build --target wasm32-unknown-unknown --release
```

Contract-family order:

1. Tokens and wrapped assets: `lusd_token`, `weth_token`, `wsol_token`, `wbnb_token`, `mt20_token`
2. Core DeFi: `dex_core`, `dex_amm`, `dex_router`, `dex_margin`, `dex_rewards`, `dex_governance`, `dex_analytics`
3. Markets: `lichenswap`, `thalllend`, `prediction_market`, `sporepump`
4. Identity/governance: `lichenid`, `lichendao`, `lichenoracle`
5. NFT/marketplace: `lichenmarket`, `lichenauction`, `lichenpunks`
6. Agent/storage/payments: `bountyboard`, `compute_market`, `moss_storage`, `sporepay`, `sporevault`
7. Privacy/bridge: `shielded_pool`, `lichenbridge`

### Part 6 - Custody, Faucet, Bridge, And External-Chain Boundary

Scope:

- `custody/src/*`
- `faucet-service/src/*`
- `faucet/*`
- `docs/deployment/CUSTODY_DEPLOYMENT.md`
- `docs/guides/CUSTODY_MULTISIG_SETUP.md`
- `docs/strategy/CUSTODY_ORACLE_TRUST_MODEL.md`

Audit questions:

- Do bridge deposit and withdrawal auth flows fail closed under missing signer,
  replay, stale timestamp, bad chain, or incident-mode conditions?
- Are custody signer thresholds, PQ signer identities, EVM/Solana key systems,
  and external treasury boundaries clearly separated?
- Are withdrawal velocity limits, operator confirmation gates, daily caps,
  post-burn delays, webhooks, and audit events test-covered?
- Does faucet rate limiting match docs, UI copy, and processor/RPC caps?
- Are non-US faucet probes and port assumptions still intentional after the
  `9100/tcp` live fix?

Validation slices:

```bash
cargo check -p lichen-custody -p lichen-faucet --tests
cargo test -p lichen-custody -- --nocapture
cargo test -p lichen-faucet -- --nocapture
node faucet/faucet.test.js
```

### Part 7 - SDKs, CLI, And Developer Portal Contract

Scope:

- `cli/src/*`
- `sdk/rust/src/*`
- `sdk/js/src/*`
- `sdk/python/lichen/*`
- `developers/*`
- `docs/api/*`
- `docs/guides/*`
- `SKILL.md` relevant RPC/CLI sections only

Audit questions:

- Are CLI commands aligned with current RPC methods, transaction wire format,
  PQ key handling, contract deploy/upgrade rules, and public network controls?
- Are Rust, JS, and Python SDK transaction serialization and signatures
  cross-compatible with core golden vectors?
- Are SDK package names, install commands, versions, examples, and docs current?
- Does the developer portal document every live action accurately, including
  success shapes, error shapes, confirmation semantics, and compatibility caveats?
- Are docs honest about current testnet/mainnet state and Phase 2 not-yet-wired
  features?

Validation slices:

```bash
cargo check -p lichen-cli --tests
cargo test -p lichen-cli -- --nocapture
cd sdk/rust && cargo check --tests && cargo test -- --nocapture
cd sdk/js && npm run test && npm run build
node sdk/js/test_cross_sdk_compat.js
cd sdk/python && python3 -m pytest -q
```

### Part 8 - Frontends And User-Facing Wiring

Scope:

- `monitoring/shared/*`
- `wallet/*`
- `wallet/extension/*`
- `explorer/*`
- `dex/*`
- `marketplace/*`
- `developers/*`
- `programs/*`
- `monitoring/*`
- `faucet/*`
- `website/*`
- portal `_headers`
- `scripts/sync_frontend_shared_helpers.js`
- `scripts/deploy-cloudflare-pages.sh`

Audit questions:

- Does every clickable action either execute a real backend flow or show an
  honest disabled/coming-soon state?
- Are production network defaults, custom endpoint restrictions, signed metadata
  trust, and local/testnet/mainnet selectors consistent across portals?
- Are all RPC, REST, WebSocket, custody, and faucet calls handled with loading,
  retry, empty, and error states?
- Are XSS, URL sanitization, CSP, storage of wallet secrets, extension CSP, QR,
  and shielded data handling acceptable for mainnet?
- Are all portal docs and nav links free of dead links and stale package names?
- Does Mission Control continue to match actual RPC field names and live network
  semantics?

Validation slices:

```bash
npm run test-frontend-assets
npm run test-wallet
npm run test-wallet-extension
node --check monitoring/js/monitoring.js
node --check wallet/js/wallet.js
node --check dex/dex.js
node --check explorer/js/explorer.js
```

Manual local-stack checks after static cleanup:

```bash
export LICHEN_KEYPAIR_PASSWORD='local-e2e-secret'
./scripts/start-local-stack.sh testnet
node scripts/qa/e2e-rpc-coverage.js
bash scripts/qa/test-rpc-comprehensive.sh
bash scripts/qa/test-cli-comprehensive.sh
```

### Part 9 - Supply Chain, CI, Release, And Packaging

Scope:

- `.github/workflows/*`
- `deny.toml`
- `.cargo/audit.toml` if present
- `Cargo.lock` files
- `package-lock.json` files
- `sdk/python/pyproject.toml`
- release scripts
- Dockerfiles and compose files

Audit questions:

- Does CI cover every workspace crate, every contract, frontend asset drift,
  wallet extension packaging, cargo-audit across all lockfiles, cargo-deny,
  SBOM generation, and release artifact creation?
- Are advisory ignores documented with real mitigation or accepted-risk notes?
- Are JS/Python dependency checks represented, or are they a current gap?
- Are release assets reproducible enough, signed correctly, and documented for
  operators?
- Is SLSA-style provenance and OpenSSF Scorecard posture sufficient for mainnet
  credibility, or should provenance/signing be strengthened before public launch?

Validation slices:

```bash
cargo deny check --config deny.toml advisories licenses sources
find . -path '*/target' -prune -o -name Cargo.lock -print | sort
# audit each lockfile in small batches rather than one memory-heavy sweep
cd sdk/js && npm audit --omit=dev
cd dex/sdk && npm audit --omit=dev
cd sdk/python && python3 -m pip check
```

### Part 10 - Documentation, Public Claims, And Mainnet Decision Ledger

Scope:

- `README.md`
- `DEPLOYMENT_STATUS.md`
- `docs/deployment/PRODUCTION_DEPLOYMENT.md`
- `docs/foundation/*`
- `docs/strategy/*`
- `docs/audits/*`
- `developers/*`
- `website/*`
- `memories/repo/*`

Audit questions:

- Which docs are authoritative, dated snapshots, active runbooks, or archived
  historical context?
- Do public claims about finality, TPS, fees, compatibility, PQ cryptography,
  shielded runtime, contracts, validators, and mainnet state match code?
- Are testnet/mainnet states and Phase 2 not-yet-wired features described
  honestly?
- Does the developer portal expose all current actions and response shapes
  without phantom endpoints or stale examples?

Validation:

```bash
rg -n "v0\\.5\\.|v0\\.4\\.|instant finality|32-slot|mainnet ready|production ready|TODO|not wired|localhost|@lobstercove|lichen-client-sdk|cargo install" README.md docs developers website
npm run test-frontend-assets
```

## Mainnet-Ready Exit Criteria

The repo can be called mainnet-ready only when all of these are true:

1. No Critical or High findings remain open, except explicitly accepted external
   dependencies or future features not claimed as live.
2. All Medium findings are either fixed, moved to a dated post-mainnet backlog,
   or documented as non-blocking with rationale.
3. Module-scoped Rust checks pass for every workspace crate.
4. Contract-local check/test/WASM build passes for every genesis contract and
   the non-genesis `mt20_token` status is documented.
5. Frontend asset, shared-helper, wallet, and extension audits pass.
6. JS, Rust, and Python SDK serialization/golden-vector checks pass.
7. RPC/frontend/developer portal parity checker has no unexplained live-action
   gaps.
8. Local production-parity stack runs from clean state and passes RPC/CLI E2E.
9. CI/release/supply-chain posture has no unexplained lockfile, advisory,
   artifact-signing, or provenance gaps.
10. Public docs and developer portal claims match the code and live testnet
    reality.

## Compaction-Safe Working Model

Use this sequence to avoid losing context:

1. Work in numbered parts from this plan.
2. At the start of each part, copy the part heading into the execution tracker.
3. Record files read, commands run, and provisional findings immediately.
4. Before any code edit, record the intended write set.
5. After each validation slice, record pass/fail and exact command.
6. At the end of a substantial part, add a concise handover under
   `memories/repo/session-handovers/` only if non-obvious state was created.

## First Recommended Work Slice

Start with the smallest no-regression repair and harness hardening:

1. Fix `sdk/js/test_cross_sdk_compat.js` duplicate `encodeBytes`.
2. Run the SDK JS syntax/golden-vector validation slice.
3. Create the execution tracker.
4. Build the precise RPC/frontend/developer-doc method classifier.
5. Use that classifier to drive the frontend and developer portal audit instead
   of relying on broad text searches.
