# Changelog

All notable changes to the Lichen blockchain project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.182] - 2026-06-20

### Fixed
- Rebuilds the `account_txs` activity index and its `atxc:` counters from
  canonical blocks once per validator after upgrade, matching the existing
  `tx_by_slot` canonical index repair path.
- Fixes account activity pagination to seek from a full account-index key and
  use a total-order RocksDB scan, avoiding empty wallet activity pages when
  account transaction counters are nonzero.
- Adds a regression test that reproduces the stale-count/missing-account-index
  failure mode and verifies canonical rebuild restores paginated activity.

### Verified
- Passed focused core account-index regression coverage, RPC
  `getTransactionsByAddress` coverage, validator compile checks, wallet audit,
  and extension audit.

## [0.5.181] - 2026-06-20

### Fixed
- Aligns the mainnet launch runbook and release-signer QA with the current
  `v0.5.179` signed rollback point so the release check suite stays green.
- Adds canonical MossStake unstake queue status to RPC responses, including
  current slot, cooldown slots, claimable state, remaining slots, and estimated
  remaining seconds so wallet surfaces do not recompute claimability
  inconsistently.
- Updates web wallet and extension MossStake views to consume the canonical
  unstake queue state before falling back to local slot checks.
- Changes the packaged wallet extension default network to public testnet and
  migrates the old implicit localhost default when no custom local RPC is set.
- Stops the web wallet from rendering RPC/index failures as a false "No
  activity yet" empty state.
- Makes extension provider `eth_getTransactionCount` use the canonical account
  transaction count RPC instead of a capped activity page length.

### Verified
- Passed focused RPC queue tests, wallet audit, extension audit, extension
  signing/provider E2E, JavaScript syntax checks, and a clean local
  3-validator reset smoke with all three validators healthy and producing
  matching slots.

## [0.5.179] - 2026-06-19

### Fixed
- Canonicalizes ledger snapshot export for block, transaction, slot, metadata,
  and account-transaction indexes by deriving exported rows from canonical slot
  mappings instead of raw hot column-family history.
- Prevents stale noncanonical block and transaction records retained by a
  source RocksDB from being propagated to fresh or resumed validators through
  checkpoint snapshots.
- Makes account transaction index derivation deterministic so canonical
  snapshot replay is stable across validators.

### Verified
- Passed focused canonical ledger snapshot regression tests, full
  `lobstercove-lichen-core` tests, validator snapshot and sync regressions,
  clippy for core and validator targets, and a clean local 3-validator
  post-checkpoint rejoin rehearsal before release gating.

## [0.5.178] - 2026-06-19

### Fixed
- Preserves BFT message delivery after validator reconnects by relaying
  consensus-critical traffic to all healthy connected peers, while keeping the
  existing degraded-peer score filter and consensus signature checks.
- Prevents checkpoint metadata quorum double-counting when one physical peer is
  first seen by authenticated node identity and later promoted to validator
  identity.
- Prunes stale pending blocks through an imported verified snapshot checkpoint
  slot before marking sync caught up.
- Allows pending catch-up to skip stale lower-slot candidates when a higher
  block still chains from the canonical tip hash.
- Makes local 3-validator rehearsal wait for the seed RPC to become healthy
  before joiners start, avoiding accidental independent local genesis startup.
- Documents explicit reserved-peer pinning for validator meshes.

### Verified
- Passed focused mempool, P2P, validator sync, checkpoint, and oracle
  replacement regressions locally before release gating.

## [0.5.177] - 2026-06-19

### Fixed
- Reduces resumed and fresh-join initial sync idle time between bounded
  block-range batches by allowing the next catch-up request immediately after
  the previous target slot is applied, while keeping live-sync retry throttling
  intact.
- Serves catch-up block-range responses at the existing 500-block protocol cap
  instead of splitting large restart ranges into unnecessary smaller messages.
- Cleans stale `staging-snapshot-<slot>` directories on validator startup,
  tears down active snapshot staging on receiver shutdown, and prunes checkpoint
  retention after verified snapshot imports as well as periodic checkpoints.
- Keeps the change scoped to restart/sync and checkpoint housekeeping; no
  consensus state schema, reward accounting, contract ABI, or genesis catalog
  behavior changes are introduced.

### Verified
- Passed focused validator sync/checkpoint regressions, full
  `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`,
  standalone contract tests, WASM contract builds, release-doc QA, and
  CI-style RPC/CLI/deterministic local coverage.
- Passed a clean local 3-validator reset/join rehearsal and a no-reset resume
  rehearsal before release.

## [0.5.176] - 2026-06-18

### Fixed
- Adds ABI `failure_codes` for value-returning contracts with explicit sentinel
  errors, and aligns bundled contract ABIs with source return behavior.
- Fixes prediction-market dispatcher return-code propagation for query opcodes
  and renames the bundled ABI entry to `set_lusd_address`.
- Aligns CLI, wallet, extension, SDK, and developer surfaces with current RPC
  response envelopes for contracts, token accounts, NFTs, marketplace activity,
  validators, transactions, staking, burned supply, and block/network status.
- Tightens cross-contract token transfer success handling so wrapped-token
  callers follow the ABI-declared `0` success code instead of accepting stale
  success markers.
- Adds missing SporePay escrow configuration ABI exports and keeps all bundled
  contract functions covered by explicit result semantics.
- Updates deployment runbooks, release docs, host-function docs, and developer
  portal examples to the current `v0.5.176` release candidate and `v0.5.161`
  rollback reference.

### Verified
- Focused ABI, CLI, RPC, SDK, wallet/extension, deployment-doc, formatting,
  clippy, and release checks are rerun before tag.

## [0.5.169] - 2026-06-17

### Fixed
- Pauses block-range catch-up while a verified checkpoint snapshot transfer is
  active, preventing resumed validators from starving their own snapshot repair
  by flooding source peers with range replay requests.

### Verified
- Passed focused validator sync-action and snapshot retry regressions.
- Passed `cargo clippy -p lichen-validator --all-targets -- -D warnings`.

## [0.5.168] - 2026-06-17

### Fixed
- Preserves the newest RocksDB checkpoint during size-cap pruning so far-behind
  or resuming validators always have at least one checkpoint snapshot source,
  even when a single logical checkpoint exceeds `LICHEN_CHECKPOINT_MAX_BYTES`.

### Verified
- Passed focused core checkpoint pruning regressions.

## [0.5.167] - 2026-06-16

### Fixed
- Bounds stalled checkpoint snapshot retries so a resuming validator abandons an unservable source/slot/root after repeated no-progress retries, clears staging state, and requests fresh checkpoint metadata instead of looping indefinitely on a stale advertised checkpoint.
- Invalidates an exact stale checkpoint advertisement on the provider when a state snapshot request can no longer be authorized from local checkpoint storage, preventing upgraded validators from re-advertising pruned checkpoint snapshots.
- Updates the deployment runbook target to `v0.5.167` with `v0.5.164` as the signed rollback point.

### Verified
- Passed focused stalled snapshot retry/cache invalidation tests plus the validator checkpoint and snapshot test filters.

## [0.5.166] - 2026-06-16

### Fixed
- Bounds RocksDB checkpoint retention by total logical size in addition to count, preventing hard-linked checkpoint directories from pinning hundreds of gigabytes of obsolete SST files on long-running validators. `LICHEN_CHECKPOINT_MAX_BYTES` defaults to 8 GiB and can be raised or disabled explicitly by operators.
- Reduces catch-up block-range fanout to one primary peer per chunk with fallback on send failure, avoiding duplicate range floods when a stale validator is replaying a large parent gap.
- Extends the P2P sync block queue send timeout so valid range responses are less likely to be dropped while the validator replay path is under catch-up pressure.
- Updates the root and JavaScript SDK npm lockfiles to `ws` 8.21.0 so release CI passes the production dependency audits.
- Updates the Python SDK runtime lockfile to `cryptography` 48.0.1 so release CI passes the Python dependency audit.
- Updates the deployment runbook target to `v0.5.166` with `v0.5.164` as the signed rollback point.

### Verified
- Passed focused checkpoint-pruning and validator sync request tests plus `cargo clippy -p lobstercove-lichen-core -p lichen-p2p -p lichen-validator --all-targets -- -D warnings`.

## [0.5.163] - 2026-06-15

### Fixed
- Honors `RUST_LOG` for the validator supervisor and child validator process instead of hardcoding INFO-level tracing, so production `RUST_LOG=warn` suppresses high-volume BFT/P2P INFO logs and prevents avoidable syslog/journal growth.
- Updates current release and deployment runbook rollback references to use `v0.5.161` as the signed rollback point.

### Verified
- Passed `cargo test --workspace`, `cargo clippy --workspace -- -D warnings`, deployment-doc QA, and the focused validator logging-filter regression test.

## [0.5.152] - 2026-06-12

### Fixed
- Drains stale pre-consensus BFT proposal/vote queues while fresh or resumed validators wait for genesis sync, validator discovery, registration, and exact-tip catch-up, preventing bounded P2P BFT queues from filling with obsolete messages before the node joins consensus.
- Keeps fresh-join initial sync on the existing batched block-range requester instead of issuing overlapping parent-gap broadcasts for every pending block, while preserving immediate parent-gap repair for live validators.

### Verified
- Passed the full validator unit suite and a clean local 3-validator seed-plus-empty-joiners run with zero BFT channel-full warnings, zero block-range request channel-full warnings, 301/301 recent blocks committed in BFT round 0, and 400 ms observed block intervals across all validators.

## [0.5.151] - 2026-06-12

### Fixed
- Exempts anchored state snapshot chunk requests from the generic expensive-request throttle while keeping P2P admission validation and the validator snapshot serve token bucket, so clean joiners can download full checkpoint snapshots without being penalized as abusive peers.
- Publishes a shared snapshot entries payload limit below the outer P2P message limit and aligns encrypted transport reads with the secure frame limit, preventing valid nested snapshot payloads from being accepted by the snapshot codec and rejected by the P2P envelope or transport receiver.
- Verifies commit certificates and BFT timestamp medians with `StakeInfo::total_stake()` so replay, checkpoint, sync, and live consensus use the same delegated stake weight.
- Rejects instructions with no accounts during transaction structure validation, ensuring mempool sender indexing cannot panic after accepting a structurally malformed transaction.
- Updates mainnet deployment runbooks and sync test names/comments to reflect the four-validator `v0.5.151` target with `v0.5.150` retained as rollback and the current always-full-validate sync behavior.

### Verified
- Local unit coverage passed for full core, full P2P, full validator, RPC library, and genesis suites before local validator rehearsal.

## [0.5.135] - 2026-06-09

### Fixed
- Extends guarded shielded-state bundle export/import to include the transaction records referenced by the shielded transaction index, so repaired or checkpoint-joined validators can serve shielded pool metrics and shielded transaction history consistently even when they do not retain the original historical block archive locally.
- Transfers hot historical/archive/index RocksDB column families through checkpoint warp snapshots instead of excluding them, so checkpoint-joined validators serve the same public RPC history and indexes as validators that replayed from genesis.
- Updates snapshot coverage tests so only rebuildable sparse Merkle cache families are excluded; every other hot column family must be transferred or handled by a typed special snapshot category.
- Adds a regression test that imports a shielded bundle into a destination without block history and verifies both `get_recent_shielded_txs` and `get_transaction` resolve the shielded transaction.
- Makes snapshot chunk encoding fail closed instead of serializing oversized or invalid chunks to empty payloads.
- Uses smaller request chunks for archive-heavy snapshot categories so block and transaction history transfers stay below the snapshot message codec limit.

## [0.5.132] - 2026-06-09

### Fixed
- Routes BFT-committed blocks through the same deterministic post-store hook wrapper as network-applied blocks, so local proposers and followers complete stake-pool, oracle, activation, analytics, MossStake, and post-state anchor effects through one canonical path.
- Adds a regression guard that BFT commit stores the block before shared post-block hooks, applies that wrapper exactly once, and no longer calls lower-level post-block helpers directly.

## [0.5.131] - 2026-06-09

### Fixed
- Audits recent stored canonical blocks at validator startup and completes any missing deterministic post-block effects before the node participates, so a restarted validator cannot keep a stale stake-pool producer counter while its block is already stored.
- Covers the stale-parent-block recovery case with a validator regression test where tip-only recovery is insufficient and recent-window recovery repairs the stake-pool singleton exactly once.
- Documents that normal testnet validator onboarding is bootstrap-recovery registration; explicit self-funded registration remains a hidden advanced command and must not be used for standard testnet joins.

## [0.5.130] - 2026-06-09

### Fixed
- Keeps lichen-testnet-1 validator onboarding on bootstrap-recovery grants after the repair slot; the stake-pool grant counter, not a historical slot cutoff, is the live cap.
- Changes the normal `lichen validator register` command back to bootstrap-grant registration so new validators follow the same schedule as the original testnet validators.
- Adds a signed `ReclassifyValidatorBootstrap` system instruction for exact 100,000 LICN explicit-funded validator entries that must enter normal bootstrap-recovery accounting, without moving funds or editing RocksDB out of band.
- Adds `lichen validator reclassify-bootstrap` so operators can submit the correction with the validator key through the same signed transaction path as validator registration.
- Covers the correction path with consensus tests for successful reclassification, already-bootstrapped rejection, and non-exact-stake rejection.

## [0.5.129] - 2026-06-09

### Fixed
- Keeps RocksDB checkpoint creation on the cheap native-checkpoint path by writing checkpoint metadata from the already committed cached/sparse state root instead of forcing a cold Merkle rebuild on the live validator.
- Centralizes the full state-snapshot column-family surface so P2P admission, snapshot export/import, and local coverage tests agree on every transferred hot column family.
- Clears stale live snapshot categories before verified checkpoint import so fresh or repaired validators cannot retain old data in a column family omitted from the incoming snapshot.

## [0.5.128] - 2026-06-09

### Fixed
- Makes checkpoint snapshot serving fail closed on RocksDB iterator/export errors instead of returning empty chunks.
- Makes verified checkpoint live commits require every canonical snapshot category and valid singleton payloads before any live state import, with fatal handling on commit failure.
- Validates and rate-limits `StateSnapshotRequest` messages at P2P admission with bounded chunk sizes and the canonical snapshot category allowlist.
- Applies pending validator-change queue writes through the transaction batch and schedules shielded pool operations in one parallel conflict group.
- Makes achievement storage updates deterministic by failing the transaction if the canonical post-execution hook cannot persist its batched state.
- Carries simulated contract storage changes across multi-instruction simulation calls, including cross-contract storage deltas.

### Verified
- Local clean 3-validator deployment passed with V2/V3 joining from empty chain state.
- Two local restart/resume cycles passed with identical slots, block hashes, state roots, validator count, and shielded roots across all three validators.

## [0.5.127] - 2026-06-08

### Fixed
- Includes shielded RocksDB column families in warp snapshots so fresh or repaired validators import the privacy pool, commitments, note payloads, nullifiers, and shielded transaction index instead of serving an empty shielded RPC state from a synced checkpoint.
- Adds an explicit guarded shielded-state rebuild command that reconstructs only shielded pool/index column families from local canonical blocks, with dry-run output and write confirmation, so hollow RPC origins can be repaired without copying another validator's RocksDB state.
- Adds a guarded shielded-state bundle export/import command for testnet operators to replace only the shielded column families from an archive origin when a checkpoint-joined validator lacks the historical shielded transaction blocks needed for local replay.
- Adds a `sparse_shielded_v2` state commitment schema that can include shielded state in future block roots after explicit activation, plus cache invalidation and diagnostics for shielded state-root components.

## [0.5.123] - 2026-06-08

### Fixed
- Starts initial block catch-up from the first missing descendant instead of re-requesting the already canonical local tip, preventing fresh validators from looping on duplicate-tip responses while pending children are available.

## [0.5.122] - 2026-06-08

### Fixed
- Prevents stale same-slot checkpoint repair checks from comparing a replayed live database against an older checkpoint after later blocks already exist locally.
- Keeps speculative BFT proposal, prevote, and precommit heights out of durable block-sync targets so catch-up only chases blocks that peers have actually advertised or served.

## [0.5.121] - 2026-06-08

### Fixed
- Keeps sync recovery deterministic when a synced block replays to a state root different from the committed header: the block is rejected, live consensus root checks stay fatal, and the node pivots to authenticated checkpoint metadata so it can import a verified full checkpoint instead of restarting on the same divergent local replay.
- Routes warp-sync checkpoint metadata probes through one peer-selection helper and sends far parent-gap recovery to verified checkpoint sync after the bootstrap prefix, avoiding doomed large block-range replay on already-divergent state.

## [0.5.120] - 2026-06-08

### Fixed
- Repairs same-slot root divergence through the normal verified checkpoint snapshot path: singleton mismatches now request authenticated checkpoint metadata, stage the full checkpoint snapshot, verify the staged state root, and only then replace local state.
- Stops serving or applying legacy one-kind validator-set and stake-pool snapshots, so those consensus singletons are imported only through block replay or full checkpoint snapshots.
- Removes the local-history stake-pool production-counter repair command and deployment instructions.

## [0.5.119] - 2026-06-08

### Fixed
- Completes deterministic post-block effects before accepting a duplicate BFT commit at the current tip, preventing stored-block/reward-counter races from leaving validators with matching blocks but different stake-pool bytes.
- Adds a confirmed operator repair command for legacy testnet stake-pool production counters, rebuilding `blocks_produced` and `last_reward_slot` from canonical stored blocks with before/after hashes.

## [0.5.118] - 2026-06-08

### Fixed
- Uses total-order RocksDB scans for sparse state rebuilds and warp snapshot exports so prefix-indexed contract storage roundtrips to the exact checkpoint state root for fresh validator sync.
- Writes checkpoint metadata from a fresh checkpoint-root recomputation and adds a read-only warp snapshot roundtrip diagnostic for validator recovery checks.
- Imports warp snapshot categories in canonical order so `stats` is applied before root-bearing singleton pools.

## [0.5.117] - 2026-06-08

### Added
- Adds `lichen validator fingerprint` and `lichen validator register` for post-bootstrap validator admission through the chain's `RegisterValidator` consensus instruction.

## [0.5.116] - 2026-06-08

### Fixed
- Embeds the fourth public seed in fallback testnet and mainnet network defaults, keeping fresh nodes aligned even before `seeds.json` is loaded.
- Updates the production deployment QA expectation to the current signed release target so release CI checks the same runbook version operators deploy.

## [0.5.115] - 2026-06-08

### Fixed
- Rebuilds the sparse state commitment in staging before warp snapshot state-root verification, so fresh validators can import a corroborated sparse snapshot from empty state without rejecting the canonical root.

## [0.5.95] - 2026-06-05

### Fixed
- Keeps the BTC/wBTC release CI-clean by replacing the Bitcoin withdrawal builder's long parameter list with a typed request object and removing an unnecessary cloned output in SegWit signing.
- Updates wallet bridge audit coverage so BTC remains allowed in the deposit validator while preserving Neo X GAS/NEO route checks.

## [0.5.94] - 2026-06-05

### Added
- Adds WBTC as a first-class wrapped asset for future genesis, including contract artifact, symbol registry entry, oracle feed, DEX pairs, wallet/extension route surfaces, and developer documentation.
- Adds real Bitcoin custody support for BTC deposits, deterministic native SegWit addresses, Bitcoin Core-backed UTXO detection, signed P2WPKH sweeps, wBTC mint credits, wBTC burns, and BTC withdrawals.
- Adds a repeatable Bitcoin Core regtest smoke that exercises `createBridgeDeposit`, BTC deposit, sweep, wBTC mint, burn, BTC withdrawal, and confirmation end to end.

### Fixed
- Allows the RPC custody proxy to forward `bitcoin:btc` bridge deposit requests.
- Treats unconfirmed Bitcoin sweep and withdrawal transactions as pending until the configured confirmation threshold is reached.
- Corrects Bitcoin SegWit v0 signing by hashing BIP143 outputs without the transaction output-count prefix and normalizing ECDSA signatures to low-S form.

## [0.5.93] - 2026-06-04

### Changed
- Makes MossStake redemption use authoritative position accounting (`licn_deposited + rewards_earned`) so tier-weighted rewards are what users actually receive when unstaking.
- Computes MossStake tier APY estimates from the live weighted pool composition instead of multiplying the pool average by a tier multiplier.
- Enforces MossStake lock tiers and unstake cooldowns against block Unix timestamps instead of target-slot assumptions, so 7-day/30-day/180-day/365-day durations remain honest even when the chain is faster than 400 ms.
- Updates wallet and extension MossStake wording to show accrued rewards included in redeemable value and to explain that boosted locked tiers are position-bound.

### Fixed
- Lazily backfills legacy MossStake positions and pending unstake requests from historical block timestamps on current testnet, preserving existing state while moving enforcement to wall-clock deadlines.
- Carries principal and accrued reward backing pro-rata when transferable Flexible stLICN is sent to another account.
- Rejects transfers from boosted locked MossStake tiers, preventing locked positions from bypassing the lock by moving stLICN to a fresh flexible position.
- Reports `getBalance` and `getStakingPosition` MossStake values through the same position-value path used by unstake redemption.

## [0.5.91] - 2026-06-04

### Added
- Adds active same-route bridge deposit reservations in custody so fresh route-bound bridge authorizations reuse an existing issued/pending deposit address until it is confirmed, credited, swept, or expired.
- Adds QR-code bridge deposit displays to the web wallet and extension to match the regular receive flow.

### Changed
- Updates wallet and explorer MossStake wording to show redeemable liquid-staking value and exchange-rate gain without implying rewards are separately additive.
- Makes web wallet and extension approval popups scroll on constrained windows.

### Fixed
- Serializes optional compute-budget fields into the wallet/DEX signed transaction message bytes, fixing valid DEX order submissions that previously failed chain-side signature verification.
- Decodes token approve and DEX place-order signing intents in wallet authorization prompts instead of showing unknown contract data.

## [0.5.90] - 2026-06-04

### Changed
- Makes active sparse-state startup repair idempotent: validators skip the full sparse commitment rebuild on clean trusted sparse metadata, while still forcing one repair for older or dirty stores.
- Commits account and contract-storage sparse dirty markers in the same RocksDB batch as the state mutation, closing the startup crash window that previously required a full sparse rebuild every restart.

### Fixed
- Marks sparse commitment metadata not-ready/untrusted before rebuild and trusted only after successful full rebuild, so interrupted repairs cannot be reused as clean startup state.
- Clears stale dirty markers during full sparse account and contract commitment rebuilds and avoids a second full state-root rebuild when reporting the rebuilt sparse root.

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
