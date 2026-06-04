# Changelog

All notable changes to the Lichen blockchain project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.89] - 2026-06-04

### Added
- Adds durable DEX pair/orderbook/trade read indexes with startup backfill, so orderbook, recent trade, trader history, and quote reads use canonical persisted snapshots instead of repeated contract-storage scans.
- Adds a bounded slot-aware native RPC read cache for deterministic heavy reads, keyed by method, canonical params, and anchoring slot.
- Adds lightweight WebSocket block and transaction fanout summaries so explorer subscriptions no longer broadcast cloned full blocks or transactions.

### Changed
- Reloads the in-memory stake pool after block execution only for stake-pool-mutating system instructions, while failing open for unknown future system opcodes.

### Fixed
- Keeps DEX read APIs aligned with the same execution state while allowing current testnet nodes to backfill the new persisted indexes without reset.

## [0.5.88] - 2026-06-04

### Changed
- Speeds up DEX pair and trader trade-history REST reads with a rebuildable in-memory trade index derived from canonical `dex_trade_{id}` storage, avoiding repeated global trade scans while preserving matching, settlement, and state semantics.

### Fixed
- Ensures pair-specific recent trade reads return the requested pair's latest trades even when other pairs dominate the most recent global trade IDs.
- Removes the 1,000-global-trade lookback cap from trader trade-history reads by using the same canonical trade read model.

## [0.5.87] - 2026-06-03

### Fixed
- Repairs active sparse state-commitment metadata during validator startup before writing the tip `post_state_v1` sidecar, preventing stale sparse roots from being anchored after an upgrade or restart.
- Lets `getAccountProof` repair a missing current-tip `post_state_v1` sidecar under the canonical apply barrier only when the proof root equals the current DB root and the requested commitment slot is exactly the local tip.
- Makes sparse state-commitment verification reporting compute the displayed current root from verified full-scan roots without using the mutating cold-start rebuild path.

## [0.5.86] - 2026-06-03

### Fixed
- Makes validator-embedded `getAccountProof` reads wait behind the canonical block-apply barrier, so finalized proofs are generated only from a fully post-applied state root anchored by a stored block header or durable `post_state_v1` sidecar.
- Adds regression coverage for the proof read barrier to prevent mid-commit hybrid state roots from leaking through public RPC under the 400 ms block cadence.

## [0.5.85] - 2026-06-03

### Fixed
- Recomputes the post-block state root when writing `post_state_v1` account-proof anchors, avoiding stale composite-root cache reuse on sparse-active validators.
- Keeps finalized account-proof anchoring stable when durable finalized metadata is ahead of the in-memory finality cursor during block commit.

## [0.5.84] - 2026-06-03

### Added
- Adds durable post-state commitment anchors keyed by finalized block slot and block hash so current testnet account proofs can anchor to the deterministic post-block state root without rewriting historical signed headers.

### Fixed
- Fixes `getAccountProof` on sparse-active testnet by accepting verified `post_state_v1` anchors when the block header state root represents the pre-post-hook transition boundary.

## [0.5.83] - 2026-06-03

### Changed
- Defaults new genesis configs to `state_commitment_schema="sparse_v1"` so reset testnets, local testnets, and future mainnet launches start sparse from slot 0 unless a legacy compatibility chain explicitly opts into `ordered_v0`.

## [0.5.82] - 2026-06-03

### Added
- Adds `sparse_v1` account proof generation and RPC serialization so sparse-active nodes return `proof_type=sparse_v1` inclusion proofs instead of dropping account proof support.
- Adds sparse state-commitment admin output for active schema, current computed state root, latest stored block state root, and latest slot so coordinated activation can be verified unambiguously on stopped validator DBs.

### Fixed
- Fixes sparse state-commitment verification reporting so `--show-state-commitment-schema` reports `active=true` / `activated=true` when the sparse schema is actually persisted.
- Keeps account proof anchoring fail-closed when the current local state root is not committed by a stored block header, avoiding unauthenticated proof responses.
- Clarifies sparse rollout docs for existing signed chains: historical block headers are not rewritten, while reset testnets and mainnet genesis can start with `sparse_v1` at slot 0.

## [0.5.81] - 2026-06-03

### Added
- Adds `sparse_v1`, a compact sparse state commitment for account and contract-storage roots, with deterministic rebuild/backfill, dirty-key incremental updates, pre-activation shadow maintenance, and guarded validator admin commands for testnet rollout.
- Adds `state_commitment_schema` genesis support so reset testnets, local testnets, and future mainnet launches can start directly with `sparse_v1` instead of migrating after slot 0.
- Adds a sparse state commitment rollout runbook covering local gates, rolling backfill, coordinated activation, genesis/reset configuration, and the temporary ordered-proof caveat.

### Changed
- Explorer cadence now reports observed block interval separately from the configured 400ms target so public status stays honest during production tuning.

### Fixed
- Fixes DEX numeric input resets, data-synced governance defaults, configurable proposal voting periods, and the DEX governance WASM/ABI needed for the current testnet upgrade.
- Drops buffered proposals, prevotes, and precommits while a live validator is catching up to a higher peer tip, preventing lagging nodes from validating or voting against stale parent state.
- Extends rolling release verification so every shipped binary installed from a release archive, including `lichen-custody` and `lichen-faucet`, must match the signed archive hash before rollout continues.
- Adds DEX Pages deployment gates for signed metadata trust anchors, versioned metadata-critical assets, and custom-domain cache-control evidence so stale frontend bundles cannot hide a healthy symbol registry.

## [0.5.80] - 2026-06-02

### Fixed
- Avoids full contract-storage Merkle scans for account-only proposal blocks by caching canonical account/contract subroots and fast-pathing empty batch overlays while preserving the existing state-root format.
- Prevents stale composite state-root reuse by checking durable dirty markers, invalidating cached composite roots on stake-pool and MossStake writes, and recomputing restriction-schema roots instead of trusting stale cache metadata.
- Skips stale BFT proposal validation and proposal builds when the canonical tip has already advanced under the apply barrier, reducing parent/state-root mismatch risk during sync catch-up.
- Adds regression coverage for account-only proposal roots over populated contract storage, contract-storage proposal roots, dirty-marker cache drift, stake-pool cache invalidation, and BFT proposal apply-barrier scope.

## [0.5.79] - 2026-06-02

### Fixed
- Guards the remaining BFT pending-proposal validation path after commit-height catch-up, preventing buffered future proposals from being checked against partially settled parent state on lagging validators.
- Tightens validator regression coverage so every BFT proposal-validation site must hold the canonical apply lock before reading state roots.

## [0.5.78] - 2026-06-02

### Fixed
- Prevents BFT from validating or proposing the next height from partial parent state by waiting for canonical post-block effects before waking the BFT loop and before proposal state reads/builds.
- Adds validator regression coverage for the chainable sync notification order and the BFT canonical-apply barrier so state-root mismatch fixes cannot be dropped silently.
- Documents the testnet checkpoint hard-link ownership issue found during rollout diagnostics; operators should keep live RocksDB SST files owned by the validator service user so checkpoints can be created under Linux protected-hardlink policy.

## [0.5.77] - 2026-06-01

### Added
- Adds a guarded testnet-only DEX contract repair path to `lichen-validator` for coordinated stopped-state replacement of stale registry-backed DEX WASM/ABI payloads without resetting chain history or contract storage.

### Fixed
- Preserves DEX contract ownership, storage, version history, and previous-code hash evidence while repairing the live testnet DEX, wrapped-asset, oracle, prediction, and launchpad contract code to the release artifacts.

## [0.5.76] - 2026-06-01

### Fixed
- Restores the release CI gates after the DEX margin lUSD collateral upgrade by updating adversarial margin setup to configure the collateral token, self-custody address, and insurance liquidity.
- Gates the validator marketplace parser test helper to test builds so workspace Clippy passes with `-D warnings`.

## [0.5.75] - 2026-06-01

### Fixed
- Hardens the DEX production surface for mainnet readiness: lUSD-backed margin collateral, governance/proposal wiring, launchpad refund behavior, rewards/genesis custody wiring, and full trade/prediction/pool/launch/rewards validation.
- Aligns marketplace offers, collection offers, and auctions with deployed contract ABI and slot-based expiry semantics, including secure NFT/collection randomness and marketplace activity indexing for array-shaped frontend calls.
- Adds preflight simulation to wallet, extension, programs SDK, monitoring, website, and explorer transaction paths, and fixes explorer LichenID contract calls to encode ordered WASM ABI arguments.

## [0.5.69] - 2026-05-28

### Fixed
- Makes contract WASM relinks explicit by allowing unresolved host imports for `wasm32-unknown-unknown`, so the release gate does not depend on stale contract build caches.

## [0.5.68] - 2026-05-28

### Fixed
- Fixes the governed-transfer CLI helper shape so the full workspace Clippy release gate passes under `-D warnings`.

## [0.5.67] - 2026-05-28

### Fixed
- Makes governed-transfer dry runs execute governed proposal checks against a rollback batch so timelocks, approvals, cancellation, and daily-cap failures match block execution before a transaction is broadcast.
- Exposes governed proposal execution policy fields through RPC, including `execute_after_epoch`, velocity tier, daily cap, and cancellation state, so CLI/operator views show the effective on-chain policy.

## [0.5.66] - 2026-05-28

### Added
- Adds generic governed native-wallet transfer CLI operations for proposing, approving, executing, cancelling, and inspecting governed wallet transfers without embedding operation-specific defaults.

### Fixed
- Increases encrypted P2P transport frame capacity so warp state snapshot chunks fit inside the transport frame.
- Exempts state snapshot chunk requests from the expensive-request throttle and keeps snapshot serving pinned to a verified checkpoint export session during warp sync.
- Hardens validator warp catch-up retry, duplicate chunk handling, and staging cleanup so a stale validator can rejoin from checkpoint state without mutating live state prematurely.

## [0.5.44] - 2026-05-17

### Fixed
- Fixes the Rust stable Clippy release gate on the clean Neo/GAS release candidate by using Rust 1.95-compatible exact-multiple checks, portable disk stat conversions, and grouped Neo oracle helper inputs without changing oracle, DEX, WebSocket, or custody behavior.

## [0.5.43] - 2026-05-17

### Added
- Adds the clean Neo/GAS product release on top of the stable `v0.5.37` base: wNEO and wGAS wrapped contracts, Neo X custody route configuration, genesis catalog wiring, DEX pairs, wallet/explorer/developer surfaces, GAS rewards vault support, liquidity corridor gates, reserve/liability proof services, and agent/compute policy gates.
- Adds local Neo-compatible genesis and local-stack support so fresh three-validator rehearsals can exercise Neo/GAS prices, route mocks, and public beta gates without touching VPSes.

### Changed
- Existing-chain Neo activation is fail-closed: validators may ship the Neo-capable binary first, but Neo oracle/DEX side effects are emitted only after the wrapped symbols exist on-chain and public activation approvals are complete.

## [0.5.30] - 2026-05-11

### Fixed
- Validator sync now validates replay on a staging checkpoint before mutating canonical RocksDB, preventing bad or locally divergent synced blocks from corrupting live state.
- Post-genesis initial sync retries start at slot 1 instead of re-requesting genesis after block 0 is already imported.
- Sync timeouts keep partially advancing batches active until their requested target is reached, reducing overlapping retry storms during catch-up.
- Genesis import refreshes in-memory stake and validator views authoritatively, and historical sync no longer runs pre-chainability validator activation or direct genesis-bootstrap state writes outside block replay.

## [0.5.14] - 2026-04-26

### Fixed
- Bridge genesis now embeds the planned validator committee and enforces a BFT-style threshold (`2-of-3` on the standard three-validator fleet) before deployment passes.
- Oracle genesis now authorizes planned operators, seeds all launch feeds through the contract, and exposes operational stats that distinguish contract feeds from native consensus feeds.
- Clean-slate local and hosted reset flows now pre-generate validator identities before genesis and verify bridge/oracle readiness during post-genesis bootstrap.

## [0.5.13] - 2026-04-26

### Fixed
- Removed the flawed post-effects state-root startup marker that was recorded before later deterministic post-block hooks finished, causing false `STATE INTEGRITY` warnings after clean snapshot restarts.
- Startup now logs state-root observations only at debug level; authoritative state-root enforcement remains in block import and BFT commit paths at the pre-effects boundary.

## [0.5.12] - 2026-04-26

### Fixed
- Clean-slate redeploy no longer restarts the validator after installing the signed metadata manifest. RPC reads the configured manifest file on demand, and the restart could interrupt an in-flight proposal during rollout.
- Hosted deployment now keeps the post-genesis validator running until the controlled snapshot stop, reducing restart-induced orphan proposal state during fresh fleet rebuilds.

## [0.5.11] - 2026-04-26

### Fixed
- Removed the validator background stake-pool persistence task that could overwrite a freshly committed stake pool with a stale in-memory snapshot, causing the next block to fail state-root verification and take a validator offline.
- Block-production stake-pool effects are now idempotent when a node has already persisted the slot update but has not yet written the reward completion marker.
- Validators now persist and check post-effects state roots for startup integrity instead of comparing post-effects RocksDB state to the block header's pre-effects state root.

## [0.5.10] - 2026-04-26

### Fixed
- Validator catch-up now keeps competing block candidates per slot and applies the candidate that chains from the current tip, preventing a wrong-parent candidate from poisoning sync after epoch transitions or validator restarts.
- Validator identity admission is now stake-backed only: block headers and validator announcements can no longer create unbacked validator-set entries, and startup prunes persisted unbacked validator metadata.
- P2P validator announcements now carry peer addresses without directly granting validator routing status, so reconnecting peers do not leave stale validator identities behind.

## [0.5.9] - 2026-04-23

### Fixed
- Mission Control now derives block cadence from observer-side wall-clock telemetry instead of coarse block-header second timestamps.
- Cluster monitoring now uses propagated `last_observed_block_slot` and `last_observed_block_at_ms` signals so cadence and freshness are grounded in real validator activity across the 3-node view.
- Public testnet validators and monitoring were rolled forward together on a single canonical Linux artifact so live RPC and Cloudflare Pages serve the same cadence model.

## [0.5.8] - 2026-04-23

### Fixed
- Warp checkpoint verification now accepts finalized checkpoint contents authenticated by a signed committed header while corroborating checkpoint roots by verified validator identity instead of peer socket address.
- Warp snapshot serving now includes validator and stake singleton state, avoids repeated full-column scans while paginating snapshot chunks, and falls back to the newest valid checkpoint when the latest checkpoint metadata is bad.
- Catch-up sync no longer overlaps in-flight ranges prematurely and completes batches only once the requested target slot is actually reached.
- Monitoring incident controls no longer present unsupported production RPC kill switches, and the LichenSwap stats RPC method name now matches the backend.
- RPC validator liveness status is now computed consistently across cluster and validator endpoints.

## [0.5.6] - 2026-04-10

### Added
- `lichen identity export` CLI command: decrypt and display validator/wallet keypair info. Supports `--reveal-seed` for private key export and `--output json` for agent-friendly output.
- Hosted operator setup now auto-generates `LICHEN_KEYPAIR_PASSWORD` if not previously set, eliminating a manual step that could be missed during deployment.

### Fixed
- Block timestamp drift: added `wall_clock_safe_delay()` to prevent block timestamps from racing ahead of wall clock time during fast BFT rounds. Previously, second-precision timestamps with 400ms slot time caused ~0.6s drift per block, triggering the 120s future-block rejection threshold after ~200 blocks.
- Signed metadata manifest generation is now mandatory in hosted deployment. Missing manifest data was the root cause of DEX "Missing contract addresses" errors on deployed frontends.

### Changed
- Hosted deployment docs now cover `LICHEN_KEYPAIR_PASSWORD` generation and `lichen identity export` usage for validator key access.
- Joining validators now receive the signed metadata manifest during hosted bootstrap.

## [0.5.5] - 2026-04-07

### Changed
- Removed validator bootstrap flag and environment override paths in favor of seed-file-only peer discovery.
- Updated local test harnesses, deployment setup, and operator docs to stage and consume `seeds.json` directly.
- Changed release archives to ship `zk-prove` with validator bundles and dropped faucet/custody binaries from the public agent install path.

## [0.5.4] - 2026-04-06

### Changed
- Bumped Rust crate versions for the testnet recovery and redeploy cycle.
- Aligned the testnet custody ingress hostname with `custody-testnet.lichen.network`.

## [0.4.37] - 2026-03-29

### Changed
- SDK versions bumped to 1.0.0 (JavaScript, Python, Rust contract SDK)
- Python SDK migrated from `setup.py` to `pyproject.toml` (PEP 517/518)
- CLI `--template` now validates against known categories
- CLI `init` command deprecated in favor of `identity new`
- CLI help text no longer hardcodes fee amounts; directs users to `lichen fees`
- Deprecated staking methods (`stakeToMossStake`, `unstakeFromMossStake`, `claimUnstakedTokens`) now return error code `-32000` (deprecated) instead of `-32601` (method not found)
- Solana compatibility layer returns descriptive error with supported method list for unsupported methods
- `getTransactionsByAddress` and `getTransactionHistory` consolidated to single handler (both names still work)
- `getAllSymbols` added as alias for `getAllSymbolRegistry`
- JS SDK `Connection` now supports configurable request timeout (default: 30s)
- Makefile `build-sdk` no longer suppresses TypeScript stderr
- **BREAKING**: `compute_tx_root` now uses a binary Merkle tree (domain-separated SHA-256) instead of flat concatenated hash. Blocks produced by v0.4.37+ are not compatible with older validators.

### Added
- `CHANGELOG.md` — this file
- `SECURITY.md` — responsible disclosure policy
- Binary Merkle tree for transaction root: `merkle_tx_root_from_hashes`, `merkle_tx_proof`, `verify_merkle_tx_proof` (Plan D — PR-02/BS-01)
- `getTransactionProof` RPC method — returns Merkle inclusion proof for any transaction
- JS SDK `getTransactionProof()` and static `verifyTransactionProof()` methods with `ProofStep` and `TransactionProof` types
- `lichen contract generate-client` CLI command — generates typed TypeScript or Python client from contract ABI (Plan E — DX-01)
- `allowance()` export added to lichencoin contract (Plan B — BS-03)
- Dual dispatch pattern documented in developer portal contract reference (Plan C — BS-04)

### Fixed
- JS SDK `package.json` repository URL corrected to `lobstercove/lichen`

### Removed
- MoltChain egg-info artifacts removed from source tree
- Python virtual environment removed from source tree
- JS SDK `dist/` removed from source tracking

## [0.4.36] - 2026-03-28

### Added
- Production readiness audit
- Security audit

## [0.4.35] - 2026-03-27

### Changed
- Clean-slate redeploy: all frontends, contracts, and genesis regenerated
- BFT consensus stabilized across the initial hosted validator set

## [0.4.34] - 2026-03-26

### Fixed
- Validator auto-update and built-in supervisor
- Genesis `initial_validators` BFT fix

## [0.4.33] - 2026-03-25

### Added
- Cross-margin DEX design
- Prediction market contracts and RPC endpoints

### Changed
- WASM contracts rebuilt for deterministic genesis
