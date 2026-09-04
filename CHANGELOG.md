# Changelog

All notable changes to the Lichen blockchain project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.277] - 2026-09-04

### Fixed

- Admit a full-archive node from the first slot not covered by its authenticated
  local Archive V2 catalog. Catalog-owned slots no longer have to remain
  duplicated in legacy hot/cold storage merely because a fresh-join test uses
  a larger recent-history setting than the checkpoint source.
- Preserve the stricter configured hot-window check for verified-cache and
  consensus roles, whose recent consensus history must remain physically hot.
- Retry npm audits only for bounded, recognized registry/network availability
  failures. Dependency findings still fail immediately, and three unavailable
  audit responses still block the release.

### Safety

- Immutable `v0.5.276` was not signed, published, or deployed. Its exact-tag
  four-validator gate imported the authenticated slot-10,000 hot checkpoint
  covering `7,999..10,000`, then incorrectly demanded local legacy copies for
  catalog-covered slots beginning at `5,001`. The quality job separately
  failed closed when npm's audit service returned HTTP 503.
- This correction changes full-archive admission partitioning only. It does
  not change consensus, state transitions, checkpoint/catalog schemas,
  Archive V2 objects, migration, compaction, retention, or preserved state.

### Verified

- The exact `catalog 0..7,998 + hot checkpoint 7,999..10,000 + configured
  5,000-slot window` regression fails with the v0.5.276 diagnostic and passes
  after the correction, including an authenticated read of slot 5,001 from
  Archive V2 after admission.
- Focused Archive V2 validator tests pass. Protected PR and immutable tag gates
  remain mandatory before signing, deployment, or legacy retirement.

## [0.5.276] - 2026-09-03

### Fixed

- Serialize deferred Archive V2 role admission after a fresh checkpoint import
  against canonical block application, so admission verifies one stable
  hot/cold handoff instead of racing the live block receiver.
- Keep cold migration paused until deferred role admission succeeds. The
  pending flag is cleared only after the capability has been admitted.
- Terminate with a synchronous diagnostic and nonzero status when deferred
  admission fails, preventing the release harness from receiving a silent
  successful exit.
- Restore the admitted Archive V2 catalog end when the exact gate resumes from
  an already-proven checkpoint, so later checkpoint selection retains the
  10,000-slot catalog-bound cadence instead of selecting an unreachable
  preactivation boundary.

### Safety

- Immutable `v0.5.275` passed protected CI and every tag-workflow job except
  the exact four-validator Archive V2 gate. Fresh full-archive V3 imported and
  verified its slot-10,000 checkpoint, rebuilt bounded categories, and then
  exited with status 0 during deferred role admission. It was not signed,
  published, or deployed.
- This correction changes lifecycle serialization and failure reporting only.
  It does not change consensus, migration data, checkpoint schema, catalog
  schema, Archive V2 object format, retention, or the preserved testnet state.

### Verified

- A focused regression proves cold migration remains paused through network
  catch-up until deferred Archive V2 admission completes.
- Preserved-state four-validator verification completed the retention and
  migration boundary, loaded backlog, outage and own-state restart matrix,
  independent Archive V2 build/restore, fresh full/cache/consensus role joins,
  source-outage behavior, strict DEX/prediction journey (147/147), strict
  launchpad/governance/graduation journey (104/104), and post-activity restart.
- All four independently composed Archive V2 plus hot-checkpoint public-history
  manifests matched at post-journey slot 40,000. The immutable tag workflow's
  uninterrupted exact gate remains mandatory before signing or deployment.

## [0.5.275] - 2026-09-03

### Fixed

- Build bounded hot-repair `account_txs` checkpoint rows directly from the
  canonical blocks in the advertised recent-history window, reusing the same
  source-backed derivation and conflict checks as Archive V2 segment export.
- Process the block window in 1,000-slot chunks so checkpoint memory remains
  bounded and no out-of-window account-history row can trigger a legacy cold
  block lookup.
- Guarantee maintenance fairness by forcing Archive V2 migration after one
  successful reclaim pass, so a non-empty reclaim queue cannot starve
  migration indefinitely.
- Split oversized legacy reclaim ranges at real RocksDB live-file boundaries
  before bounded compaction, while refusing non-reducing splits.
- Release the node-local archive-maintenance lock while a checkpoint waits for
  the shared-disk materialization lock, then reacquire it before reading the
  durable hot/cold source. This prevents co-located validators from forming a
  maintenance-lock convoy.
- Set the controlled LICN/USD reference to `$0.15` across validator
  attestations, future testnet/mainnet genesis defaults, DEX and wallet
  fallbacks, liquidity tooling, and operator documentation. Genesis now honors
  the launchers' `GENESIS_LICN_USD` input instead of silently replacing it with
  the compiled default.

### Safety

- The first live `v0.5.274` checkpoint reached slot `12,298,000` and remained
  on category 7 while each active validator traversed about 6.7 million
  account-history rows and performed old-block validation through the
  emergency R2/FUSE bridge. Consensus continued advancing and no state was
  reset, but this unbounded pre-activation I/O is not accepted for production.
- The output is unchanged: every derived account-history key must exist in the
  hot or legacy-cold source, identical duplicates are accepted, conflicts and
  missing rows fail closed, and the checkpoint remains budgeted and atomically
  published.

### Verified

- Focused checkpoint regressions prove that corrupt out-of-window block data is
  never consulted, bounded account-history rows remain source-backed, missing
  canonical keys fail closed, and existing hot-repair budget/publication tests
  continue to pass. A preserved-state four-validator run also passed bounded
  migration, loaded backlog, outage/restart, common catalog parity, identical
  slot-70,000 checkpoints, authenticated source outage, and clean
  full/cache/consensus joins. The immutable tag workflow remains authoritative
  for the complete release and four-validator Archive V2 matrix.

## [0.5.274] - 2026-09-03

### Fixed

- Keep an imported verified snapshot out of live sync bookkeeping until its
  durable pre-import rollback checkpoint and marker have been removed.
- Retry transient rollback-cleanup failures five times before failing closed,
  and synchronously emit the final error so `process::exit` cannot lose it in
  the non-blocking tracing buffer.
- Make the four-validator Archive V2 gate capture a fresh joiner's exact exit
  status, rollback sidecars, and filesystem headroom, and reject admission if a
  snapshot transaction remains pending.

### Safety

- Immutable `v0.5.273` passed its protected PR and all release quality,
  security, contract, compiler, and platform-build jobs, but its first strict
  Archive V2 tag gate failed when fresh verified-cache V3 exited immediately
  after checkpoint activation. It was not signed or deployed.
- Snapshot import remains source-authenticated, root-verified, transactionally
  rollback-protected, and restart-recoverable. This release does not reset or
  copy validator state and does not authorize legacy or R2 deletion before
  four-way Archive V2 parity.

### Verified

- Focused rollback tests cover temporary cleanup obstruction and prove the
  durable marker remains until checkpoint removal succeeds. The immutable tag
  workflow remains authoritative for the complete four-validator Archive V2,
  DEX/CLOB/AMM, prediction, governance, launchpad, restart, security, contract,
  and platform matrix.

## [0.5.273] - 2026-09-03

### Fixed

- Make explicit pre-activation `HotRepairV1` checkpoints reachable at the
  ordinary 1,000-slot checkpoint boundary while retaining the 10,000-slot
  cadence for catalog-bound Archive V2 compaction.
- Replace the production checkpoint preflight that charged twice for every
  inherited public-history SST with fail-closed per-page accounting for only
  newly materialized bounded rows, including conservative row, WAL, and
  compaction overhead.
- Keep inherited SSTs hard-linked, enforce the physical-write budget before
  every import page, and remove unpublished staging if the budget is exceeded.
- Rebind the preserved-chain DEX repair authorization to the exact v0.5.273
  release and align the deployment, exchange, README, and developer surfaces.

### Safety

- Signed `v0.5.272` restored three-validator production and preserved EU's own
  state fail-closed at a real parent post-state-root mismatch. It did not reset
  the chain or copy another validator's RocksDB state.
- The first production checkpoint exposed two scale/placement assumptions that
  the 30,000-slot release fixture did not model: legacy cold symlinks cannot be
  packaged as a full checkpoint, and the old estimator treated 113 GB of
  hard-linked inputs as 226 GB of new writes. This release fixes those paths;
  it does not waive authenticated checkpoint proofs or history completeness.
- No legacy or R2 deletion is authorized before four-validator Archive V2
  genesis-to-tip parity and signed range-bound retirement evidence exist.

### Verified

- Focused core regressions prove exact bounded-write accounting and ensure a
  rejected build publishes no checkpoint. Validator regressions prove separate
  pre-activation and catalog-bound cadence.
- The immutable tag workflow remains authoritative for the complete quality,
  security, contracts, four-validator Archive V2, DEX/CLOB/AMM, prediction,
  governance, launchpad, restart, and platform matrix.

## [0.5.272] - 2026-09-03

### Fixed

- Establish and read back a fresh pair-1 price band from the active validator
  oracle quorum immediately before every strict CLOB trading phase. This makes
  release qualification independent of hosted-runner slot throughput without
  weakening the production 750-slot stale-price rejection.
- Require at least two validator attestations and an exact DEX-band/margin-mark
  source-slot match before the volume journey places an order.
- Add static release QA proving that all three pair-1 trading phases refresh
  after setup and immediately before their CLOB work.

### Safety

- Immutable `v0.5.271` reached the complete four-validator DEX journey but its
  release workflow failed closed with six ABI code-11 order rejections after
  setup consumed the existing oracle freshness window. It produced no
  deployable release and was not installed on the Testnet fleet.
- The production stale-oracle guard behaved correctly. This release changes
  only test readiness and release documentation; it does not bypass oracle
  quorum, expand the freshness window, reset chain state, copy RocksDB, retire
  legacy history, or delete R2 objects.

### Verified

- JavaScript syntax and Archive V2 release-asset QA cover the new ordering and
  quorum/source-slot checks. The immutable tag workflow remains authoritative
  for the full four-validator Archive V2, DEX, prediction, governance,
  launchpad, restart, security, contract, and platform matrix.

## [0.5.271] - 2026-09-02

### Fixed

- Accept the legacy deployed contract ABI key `name` when decoding preserved
  contract accounts while continuing to serialize the canonical `contract`
  key. This restores backward compatibility without changing canonical output.
- Target the validator's real `--db-path` during the guarded Testnet DEX
  contract repair. The repair still dry-runs first, requires an exact
  version-bound confirmation, and proves `contracts=17` and `changed=0` before
  any validator starts.
- Make coordinated service shutdown nonblocking and bounded, verify the entire
  systemd control group is empty, attempt a control-group SIGKILL after the
  grace period, and fail closed if an uninterruptible process remains.
- Start independent signed-release gates in parallel while retaining the
  quality, Archive V2 parity, contract, compiler, and all-platform build gates
  as mandatory checksum and release prerequisites.

### Safety

- Signed `v0.5.270` passed its complete release matrix and produced verified
  artifacts, but its preserved-chain deployment stopped before repair or
  restart when the deployment command used the wrong state-path flag and the
  real Testnet database exposed the legacy ABI key. All four validators were
  left stopped, their own state/WAL/identities and Archive V2 inputs were
  preserved, and the signed `v0.5.265` rollback anchors remain available.
- No chain reset, cross-validator RocksDB copy, legacy-history deletion, R2
  deletion, or locally built production binary is authorized by this release.

### Verified

- A production-shaped core regression decodes the legacy `name` ABI and proves
  canonical reserialization. The guarded repair regression loads that same
  shape, updates code and ABI, and proves owner and storage preservation.
- The complete `v0.5.270` four-validator DEX, AMM, CLOB, prediction,
  governance, launchpad, outage/restart, and Archive V2 evidence remains valid
  for unchanged code. The `v0.5.271` release must additionally pass its signed
  tag gates and exact live-fleet acceptance before it is declared deployed.

## [0.5.270] - 2026-09-02

### Fixed

- Refresh the controlled LICN/USD oracle price through signed attestations from
  the active validator quorum immediately before the strict WebSocket trade,
  then read back and require the exact DEX band price and source slot. This
  keeps the 750-slot stale-price rejection intact while making the gate
  independent of runner slot throughput.
- Include bounded return-code, compute-use, and final contract-log diagnostics
  when RPC transaction preflight rejects a contract call.
- Make CLOB custody settlement atomic: pull taker escrow before matching, abort
  every failed settlement, refund expired/self-trading makers and filled-order
  residuals, fund maker rebates from taker fees, reject unfunded positive maker
  fees, and replace the lifetime order cap with a migrated live-order count.
- Make CLOB market-sell quotes report only executable depth, bind stop triggers
  to the canonical last trade, and expose a read-only exact-input quote for
  venue selection.
- Make the production DEX client and volume journey reserve Core's exact
  fragmented-fill buy bound: aggregate proportional fee plus one minimal unit
  per possible lot. This prevents correctly backed native-quote orders from
  being rejected a few base units short after custody enforcement became real.
- Make concentrated-liquidity quotes read-only, use the same tick-crossing
  curve for exact-output swaps, reject accounting overflow before custody
  moves, and authorize only the trader or the immutable configured router.
- Make the router direction-aware and venue-aware. It now supports both sides
  of each pair, permits one CLOB and one AMM route per direction, validates
  split/multi-hop token continuity, quotes both direct venues live, selects the
  best output, and rejects route/counter arithmetic overflow.
- Bind the router into DEX Core and DEX AMM at fresh genesis, and register all
  13 launch pairs in both directions on both venues. SporePump graduation now
  registers directional AMM routes after its existing atomic liquidity checks.
- Make DEX governance reject invalid pairs, unfunded maker fees, and rebates
  larger than taker fees; make emergency delisting fail closed on Core pause;
  stop advertising an unsupported requirements-only listing path; and preserve
  finalized rejected proposals with declared ABI success semantics.
- Cap referral and trading rewards against one shared epoch emission budget so
  referral payouts cannot bypass the monthly ceiling.
- Correct prediction-market complete-set cost allocation, maintain exact
  aggregate trader cost, snapshot immutable trader/LP void pools, make reclaim
  order-independent and pro-rata, and close the exact-slot trading boundary.
- Preserve the prior governed ABI during immediate and timelocked contract
  upgrades until an explicit `SetContractAbi` action replaces it, preventing an
  upgrade transaction gap from dropping declared child-call failure semantics.
- Align DEX UI governance validation and delisting text with the actual custody
  and venue boundaries, and document all newly exposed DEX ABI functions.

### Safety

- Immutable `v0.5.269` passed protected CI and the complete Archive V2 release
  matrix, then failed closed after that matrix when its controlled DEX price
  band expired on the faster hosted runner. It produced no artifacts and was
  not deployed. A matching successful local order used 47,543 of the default
  200,000 compute units, ruling out compute exhaustion.
- Existing Testnet DEX, wrapped-asset, oracle, prediction, and launchpad
  contracts are updated only while all validators are stopped, from the signed
  release's contract bundle. The guarded Testnet-only repair is additive and
  idempotent: it preserves addresses, owners, balances, storage, and chain
  history while replacing code and ABI consistently on every validator.

### Verified

- Passed the exact clean-build four-validator release gate through slot 30,000.
  Volume and prediction/DEX journeys passed 144/144, launchpad/governance
  passed 104/104, fresh full/cache/consensus joins and failure recovery passed,
  and all four final public-history manifests matched
  `27b5825f7635945d7b63e649015b3c9c85d642cc18225d1bdfcbf527a09bc397`.

## [0.5.269] - 2026-09-02

### Fixed

- Publish the validator's authoritative finalized frontier in `getHealth`,
  using the same live finality tracker or durable fallback already exposed by
  `getSlot("finalized")`.
- Sample all four validators' processed and finalized frontiers concurrently
  with bounded retries in the Archive V2 release gate. This prevents one
  transiently starved RPC from consuming the serial polling window while
  retaining the exact finalized-spread and tip-lag requirements.

### Safety

- Immutable `v0.5.268` failed closed in both Archive V2 workflow attempts
  before platform artifacts or deployment. The validators were not mutated,
  and the tag is not rerun or rewritten.

## [0.5.268] - 2026-09-02

### Fixed

- Refresh the controlled LICN/USD margin mark through signed native
  attestations from the active validator quorum immediately before the strict
  volume E2E opens positions. This advances the canonical consensus-oracle
  source slot and deterministic margin mirror while keeping the contract's
  750-slot stale-price rejection intact after the intentionally long Archive
  V2 and validator-restart matrix.

### Verified

- Passed the exact clean-build four-validator release gate through slot 30,000.
  Volume passed 141/141 checks, launchpad/governance passed 104/104 checks,
  fresh full/cache/consensus joins passed, and all four final logical history
  manifests matched
  `b83b9a31280ca546b6c45d89a36342df65792e64b840985961196f35be95279c`.

## [0.5.267] - 2026-09-01

### Fixed

- Build the JavaScript SDK on the clean release runner before wallet security
  audits import its generated distribution modules. A workflow regression gate
  now enforces install, build, and audit ordering so a locally retained
  `sdk/js/dist` directory cannot mask this release-only dependency.

## [0.5.266] - 2026-09-01

### Added

- Add a checkpoint-manifest inspection mode and source-backed Archive V2
  handoff selection that starts at the earliest unadmitted tail and remains
  bounded by the configured extension window.
- Ship `lichen-moss-provider` in signed Linux release archives and enforce the
  service bundle in release QA.
- Add a source-checked developer service reference and fail the release when a
  native RPC method, contract ABI function, primary CLI surface, service
  binary, or SDK version is absent from the developer portal.
- Add Moss Storage pricing-v3 requests that bind initial on-chain provider
  confirmations to the exact unique provider roster whose ML-DSA-signed upload
  receipts the owner accepted. A slashed assigned provider still opens
  permissionless replacement so replication can heal without the owner.
- Derive every pricing-v3 storage request ID from its owner, original content
  commitment, and fresh nonzero request nonce, while preserving the raw
  commitment for `moss://` retrieval and challenge proofs. Copied hashes cannot
  block the owner, and one wallet can create independent repeat requests.

### Fixed

- Project contract logs only from the exact newly committed slot and feed DEX
  WebSocket events through one shared, monotonic cursor on every canonical
  block-application path. This removes repeated Archive V2 history scans from
  event fanout, prevents duplicate trade projection, and restores bounded
  trade, ticker, and order-book delivery after BFT, peer sync, pending-chain,
  and fork-choice commits.
- Make stopped-validator Archive V2 role bootstrap persist a durable,
  chain-and-role-bound state admission fingerprint only after the external role
  marker is verified. Dry runs remain read-only, conflicting state markers fail
  before publication, retries are idempotent, and migrated validators can use
  catalog-bound hot checkpoints without weakening runtime capacity admission.
- Materialize every bounded public-history category from the coherent hot/cold
  view into hot-only checkpoint staging before Archive V2 construction, so a
  cold-migrated block body cannot disappear from a checkpoint.
- Permit a fresh validator to verify a catalog-bound hot-repair checkpoint
  against its exact configured Archive V2 catalog before role admission,
  without attaching a public-history reader. Runtime activation still requires
  the restored hot suffix to chain exactly from the catalog tip. A successfully
  applied bounded checkpoint activates that deferred catalog before the fresh
  node's public-genesis readiness gate, avoiding a catalog/genesis circularity
  while still failing closed on a root, coverage, or parent-hash mismatch.
- Normalize node-local commit-round and signature presentation while exporting
  checkpoint block bodies, producing deterministic Archive V2 segment bytes
  for validators with the same canonical history.
- Exclude the node-local cold-migration cursor from network snapshot statistics
  so operational progress cannot create cross-validator snapshot drift.
- Physically rebuild and compact hot-repair checkpoints to their advertised
  bounded public-history window, remove non-portable operational metadata, and
  budget the temporary rewrite allocation before construction. Checkpoint
  materialization uses a bounded 128 MiB RocksDB cache and the four-validator
  harness discards an interrupted candidate's sibling snapshot-rollback
  transaction before restoring the original validator state. The common
  catalog is range-bound by the slowest stopped validator while reserving a
  finalized transfer-and-restart overlap, and every fresh join proves that its
  checkpoint retains the exact 50,000-slot production suffix beginning at the
  catalog handoff. Bounded hot-repair checkpoints use a 10,000-slot cadence
  while legacy checkpoint profiles retain their existing 1,000-slot cadence.
- Hash final public-history parity over the complete logical Archive V2 prefix
  plus the authenticated hot checkpoint suffix, rather than over the physical
  hot partition alone. Validators with different retirement handoffs now prove
  one partition-independent genesis-to-tip manifest, while a missing,
  overlapping, or discontinuous catalog/checkpoint boundary fails closed.
- Sign every public Moss upload receipt with the provider identity, verify its
  exact owner, owner-scoped storage ID, request nonce, gateway, object commitment, size,
  price, and staging state before the storage call. Failed/idempotent uploads
  refund their quota charge, and providers retain durable per-request
  associations so closing one owner request cannot delete content still
  assigned to another owner.
- Reserve logical provider capacity for every signed but not-yet-confirmed
  upload association and reconstruct that reservation after restart. Receipt
  issuance fails before overbooking even when immutable object bytes deduplicate.
- Bind dual-R2 catalog replacement to a stable authenticated preflight ETag
  using `If-Match`, so concurrent publication aborts instead of overwriting a
  newer catalog after immutable objects have been verified in both buckets.
- Keep historical shielded proof scheme `0x01` fail-closed; no shield, transfer,
  or unshield mutation is accepted until a constrained versioned successor and
  custody transition are reviewed and activated.
- Remove the obsolete `RUSTSEC-2024-0370` exception after `proc-macro-error`
  left every locked workspace dependency graph.

## [0.5.265] - 2026-08-29

### Fixed

- Include the standalone `mt20_token` workspace in the canonical locked
  contract builder and enforce complete in-tree builder coverage in release
  QA, preventing a stale tracked template WASM from entering a contract bundle.
- Keeps the production host-wide CPU-pressure guard unchanged while exempting
  accelerated `LICHEN_LOCAL_DEV` multi-validator clusters from applying that
  single-validator threshold independently in every colocated process. This
  prevents the Linux release gate from pausing every bounded cold migration
  indefinitely under its intentional 5 ms test cadence; disk, memory,
  consensus-latency, Archive V2 capacity, and all non-development CPU guards
  remain fail-closed.
- Adds platform-independent regression coverage for the exact hosted failure
  (`load_one=8.54`, four CPUs), the production threshold boundary, local-dev
  behavior, and the defensive zero-CPU fallback.

## [0.5.264] - 2026-08-28

### Added
- Adds a stopped-validator Archive V2 `role-bootstrap` command and hash-pinned
  operator wrapper for the circular low-space legacy-retirement boundary. It
  performs a no-write dry run, proves canonical genesis/catalog/hot/source and
  custody prerequisites, preserves the absolute storage floor, and publishes
  the exact checksummed marker with create-new semantics.
- Adds bounded shared-collateral Cross Margin V2 with at most 32 active
  positions per account, aggregate equity and tier-weighted requirements,
  shared funding settlement, exact deposit/withdraw controls, portfolio-aware
  liquidation, RPC state, and DEX UI support.
- Adds a checksum-committed, resumable DEX Margin V2 migration tool and
  fail-closed runbook. Governance freezes position mutations before manifest
  capture, activates Funding V2 and Cross V2 atomically, retains the lock
  through post-activation parity, and reopens trading in one final action.
- Adds SporePay Accounting V3 with exact active/deferred escrow liability,
  stream-by-stream reconstruction, custody-solvency activation, a sealed
  manifest/resumable migration CLI, bounded account stream indexes, and a
  fail-closed operator runbook.
- Adds SporePump Accounting V3 with separately collateralized curve reserve,
  creator royalty, graduation revenue, and withdrawable platform-fee ledgers;
  exact custody/surplus proofs; and a checksum-sealed, contiguous, resumable
  migration CLI whose manifest aggregates are independently rederived and whose
  sealed manifests/receipts are durably published without partial-file windows.
- Adds protected SporePump buy/sell execution, exact REST quotes, token metadata,
  creator royalty claims, two-step administration, governed graduation
  configuration/status, deterministic legacy metadata backfill, strict symbol
  indexes, and complete JS, Python, and Rust client surfaces.
- Adds a source-bound LichenMarket V3 migration that independently replays
  archived calls into exact per-token sales and fees, inventories all active
  settlement/custody rows, splits execution by the real admin, treasury,
  offerer, and seller authorities, verifies resumable receipts on-chain, and
  keeps activation paused through exhaustive post-state verification.
- Adds a checksum-sealed SporeVault Accounting V2 migration utility and
  fail-closed runbook. It captures exact native custody, protocol fees, every
  legacy strategy row, legacy shares, and the real indexed ThallLend claim;
  emits source-bound governed retirement/activation payloads; and verifies the
  finalized vault while it remains paused.
- Adds Compute Market Accounting V3 with exact escrow, unpaid-provider, and
  withdrawable platform-fee ledgers; custody-solvency health; a source-bound,
  checksum-sealed, resumable migration CLI; and a fail-closed operator runbook.
- Adds BountyBoard Accounting V2 with exact active-escrow and realized-fee
  ledgers, custody-solvency health, immutable payment terms, source-bound legacy
  snapshot records, an atomic checksum-sealed resumable migration CLI, and a
  fail-closed operator runbook.
- Adds LichenAuction Accounting V3 with exact active-bid, active-offer, unpaid-
  payout, and platform-fee liabilities; immutable royalty snapshots; contract-
  owned escrow; a source-bound, checksum-sealed, resumable migration CLI; and a
  fail-closed operator runbook.
- Adds Staking V2 behind a future epoch-boundary activation marker: explicit
  self-bond, delegation, and MossStake ownership; one dynamic security budget;
  deterministic MossStake allocation; validator concentration limits;
  performance weighting; and delayed bounded commission changes. Shipping the
  implementation does not activate it on an existing network.

### Fixed
- Allows a bounded hot store without local slot 0 to activate Archive V2 only
  from a regular checksummed role marker that matches the exact catalog
  identity and requested runtime policy. Fresh unmarked nodes still defer;
  corrupt, mismatched, unsupported, or symlinked markers fail closed.
- Publishes the same canonical block, transaction, account, program, NFT, and
  slot WebSocket fanout for peer-synced, pending-gap, and fork-adopted blocks as
  for local BFT commits. A validator that catches up by one block no longer
  creates an Explorer slot gap or drops that block's live transaction count.
- Creates one canonical WebSocket event broadcaster before the block receiver
  starts while retaining the listener's existing bind point and readiness
  timing.
- Reports Explorer cadence as `Live <N>ms`, calculates live TPS from a rolling
  60-second window of canonical WebSocket block summaries, and reports total
  stake as validator stake plus Moss stake.
- Uses Binance's documented combined-stream WebSocket protocol for one
  multiplexed SOL/ETH/BNB/NEO/GAS/BTC feed, including combined-message parsing,
  event-time deduplication, per-symbol freshness, bounded unlimited reconnect,
  and a rate-limited REST fallback that cannot overwrite fresh WebSocket data.
- Stops attesting or broadcasting indefinitely cached external prices after a
  bounded source-staleness interval. Binance US remains an explicit US-host
  override while other regions use the default Binance.com feed.
- Uses exact fixed-point oracle units and source slots throughout consensus,
  LichenOracle, genesis, DEX marks, RPC, wallet surfaces, and ThallLend. Native
  LICN lending no longer multiplies same-asset collateral by an 8-decimal USD
  quote; oracle health remains a freshness circuit breaker for that market.
- Corrects ThallLend's legacy timestamp/rate mismatch: contract timestamps are
  canonical slots, not milliseconds, and the 400ms target has 78,894,000 slots
  per Julian year. The base rate now deterministically annualizes to 200 basis
  points instead of the unintended roughly 50 basis points, without rewriting
  previously accrued balances.
- Makes ThallLend repayments and liquidations consume only the debt actually
  retired, atomically refunds unused native LICN, caps liquidations by both the
  50% close factor and collateral available for the bonus, rejects unconfigured
  custody and malformed oracle state, and fails closed on liability underflow.
- Wires fresh-genesis ThallLend deployments to the canonical live `LICN` oracle
  feed and exposes exact rate scales, annualized rate, market configuration,
  liquidity, utilization, repayment, custody, and oracle-health metrics through
  contract views, RPC, and the JavaScript, Python, and Rust SDKs.
- Replaces SporeVault's simulated strategy accounting with exact idle custody
  plus its real ThallLend supplier claim, realizes performance and management
  fees into liquid protocol custody, forwards native strategy value correctly,
  requires exact deposit value, and rejects malformed immutable configuration,
  inconsistent share bootstrap, corrupt strategy frontiers, and unsupported
  adapters. Fresh genesis now binds ThallLend and activates one conservative
  33% lending strategy as mandatory dependencies.
- Exposes complete SporeVault accounting, custody coverage, fee/risk policy,
  dependency health, operational status, rebalancing, administration, and
  migration controls through contract views, O(1) RPC metrics, and the
  JavaScript, Python, and Rust SDKs.
- Binds Compute Market administration to the protocol initializer, makes the
  LichenID and payment-token dependencies immutable and exact, rejects
  malformed policy, lifecycle, provider, job, and accounting state, and removes
  overlapping deadline transitions. Disputes remain resolvable while paused.
- Makes Compute Market settlement conserve exact escrow across provider pay,
  requester refund, platform fee, and deferred unpaid liabilities; snapshots
  prospective fees and timeouts per job; bounds provider capacity; and rejects
  zero code/result/policy/action hashes, zero arbitrators, replayed agent
  actions, and non-increasing policy versions before value movement.
- Exposes Compute Market jobs, timing, provider capacity, agent policy, exact
  liabilities, real custody, migration state, solvency, and effective pause
  through O(1) contract views, RPC, and complete JavaScript, Python, and Rust
  clients. Fresh genesis now requires canonical LICN and LichenID bindings.
- Makes BountyBoard native and token custody exact and self-contained,
  snapshots reward asset and prospective fee per bounty, settles and refunds
  atomically, rejects duplicate/zero-proof/self-award paths and malformed
  control, row, or counter state, requires asset-exact attached value, protects
  submitted work from in-window cancellation, adds revocable two-step
  administrator rotation, and gates value mutation on Accounting V2.
- Exposes BountyBoard submissions, payment terms, fee balances, migration
  cursor, liabilities, real custody, solvency, dependency health, and effective
  pause through exact contract views, O(1) RPC, and complete JavaScript, Python,
  and Rust clients. Fresh genesis now explicitly binds native LICN and LichenID.
- Makes margin exits and liquidations realize PnL before any penalty, credits
  insurance only with collectible loss, records uncollateralized loss as
  explicit bad debt, and prevents insurance governance withdrawals from
  breaching 1:1 current open-interest coverage.
- Replaces per-position funding scans with bounded global indexes and a
  pool-backed claim/debt ledger, applies funding once to notional without a
  second leverage multiplier, and uses an 8-hour 72,000-slot interval.
- Restores the on-chain WASM dispatch paths for DEX Margin opcodes 36 through
  52. These operations existed in source but were absent from the dispatch
  length table and therefore would have rejected real WASM calls.
- Makes SporePay cancellation fail closed when custody configuration is absent,
  treats cliffs as true vesting boundaries, preserves restricted recipient
  payouts as claimable liabilities, rejects counter/accounting overflow before
  value movement, and prevents recipient transfers from bypassing pause,
  reentrancy, zero-address, or LichenID policy.
- Corrects `getSporePayStats` to read the contract's canonical `sp_*` keys
  instead of nonexistent `cp_*` keys and exposes accounting/migration state.
- Charges and collateralizes the configured SporePump creator royalty on both
  buys and sells without consuming curve principal, refunds unused capped-buy
  input exactly, and blocks partial or underfunded graduation completion.
- Makes malformed SporePump pause, migration-lock, token-freeze, trade-config,
  token-row, and accounting state fail closed across WASM, RPC, UI, SDK, and
  migration tooling. Public quotes now refuse execution while Accounting V3 is
  inactive or a token is frozen; buy quotes also honor pause and max-buy state.
- Changes the Rust SDK read-only contract return code to a signed integer so
  negative contract errors deserialize and remain inspectable.
- Escrows the full native or MT-20 value of NFT and collection offers when they
  are created, rejects ambiguous replacement and attached-value mismatches,
  preserves funded offers across failed NFT transfers, and gates every legacy
  offer, auction, listing, payout, and mixed-token metric behind a sealed V3
  migration boundary.
- Disables the unsound legacy shielded proof scheme `0x01` for every private
  operation before proof decoding. Public pool/history reads remain available,
  while validator RPC/REST, CLI, web-wallet, and extension private-action
  surfaces fail closed until a separately versioned ownership-binding proof
  system is reviewed and activated.
- Keeps next-epoch validator registrations visible without treating them as
  current voting power or available delegation capacity, and derives legacy
  and Staking V2 APY projections from their respective active reward budgets.
- Predeclares all requested local validators in genesis so the four-validator
  gate begins with four independently owned epoch-active voters and exercises
  genuine quorum loss, recovery, restart, and proposer rotation.

### Tests
- Adds Archive V2 role-marker checksum, no-overwrite, idempotency, dry-run,
  explicit-acknowledgement, fresh-unmarked deferral, exact-policy match, and
  corrupt-marker startup regressions.
- Adds functional block-before-slot fanout coverage and source-order guards for
  direct sync, pending-gap application, and fork adoption. Each path must emit
  exactly once after deterministic post-block effects complete.
- Adds raw and combined Binance frame parsing, decimal bounds, malformed/control
  frame rejection, event-time ordering, WS-versus-REST freshness, and endpoint
  shape regression coverage.
- Adds exact margin conservation, underwater bad-debt, negative realized-PnL,
  insurance-solvency, funding symmetry, shared Cross V2 portfolio, liquidation,
  withdrawal, migration, RPC, frontend, and every-opcode WASM dispatcher
  regressions. The focused margin gate passes 145 unit and 28 adversarial tests,
  strict Clippy, and a release WASM build.
- Adds SporePay lifecycle-liability, partial-settlement recovery, cliff cancel,
  identity fail-closed, transfer-policy, arithmetic-boundary, resumable
  migration, solvency, account-index parity, RPC-key, ABI-export, SDK, and
  release-WASM coverage.
- Adds SporePump curve-integral parity, buy/sell liability conservation,
  slippage, malformed-control, custody, creator claim, graduation atomicity,
  migration manifest/receipt, exact RPC shape, ABI export, DEX UI, and
  cross-language SDK regressions. The focused contract gate passes 73 tests;
  the canonical release WASM is 66,002 bytes with SHA-256
  `9e2209e5c371ad8808aca3b5d2e4448daadac4653588b5a9f68b6758fcbfdd7f`.
- Adds LichenMarket funded-offer, custody, migration-lock, source-replay,
  token-metric, dynamic-fee, malformed-layout, receipt-binding, and exact
  migration-status regressions. The focused contract gate passes 61 tests and
  the migration CLI passes 5 tests with strict Clippy.
- Adds ThallLend canonical-slot annualization, exact overpayment/refund,
  close-factor, collateral-cap, malformed-oracle, unconfigured-custody,
  accounting-underflow, rate/status view, genesis wiring, RPC, ABI, and
  cross-language SDK regressions. The focused contract gate passes 88 tests and
  strict Clippy; the release WASM is 41,761 bytes with SHA-256
  `e6f10dd685ea5dcc46d46193ecf732173c5d50fb16f909d867162a798dcc8806`.
- Adds BountyBoard exact custody/accounting, malformed-state, overflow,
  settlement-retry, immutable-term, identity, source-bound migration, RPC,
  ABI-export, genesis-wiring, cross-language SDK, and release-WASM regressions.
  The focused contract gate passes 53 tests; the reproducible release WASM is
  50,080 bytes with SHA-256
  `966a5382bf6f66797b095597eaaef7e17fee92a177164844e08695bf9141b4b3`.
- Adds SporeVault exact-custody yield, fee accrual, native/MT-20 value,
  reentrancy, malformed-state, source-bound legacy retirement, migration,
  strategy, genesis, RPC, ABI, and cross-language SDK regressions. The focused
  contract gate passes 70 tests and strict Clippy; the reproducible release
  WASM is 45,159 bytes with SHA-256
  `001d9b5ccfc39389c2e9cb051f63bf690a1966fb2a1d6b550fa8b5c217a0601c`.
- Adds Compute Market identity, immutable dependency, exact-deadline,
  settlement-conservation, deferred payout, capacity, agent replay/policy,
  malformed-state, Accounting V3 migration, custody, genesis, RPC, ABI, and
  cross-language SDK regressions. The focused contract gate passes 79 tests and
  strict Clippy; the reproducible release WASM is 70,735 bytes with SHA-256
  `38762cb50c36085878bf049ca523dc848553384caf545a83feabf68f3da65486`.
- Passes the exact clean-build four-validator release gate with four genesis
  voters, every validator observed as proposer, bounded hot/cold archival mode,
  an independently initialized fresh join, corruption quarantine and replica
  repair, source outages, a 96-transaction quorum-loss backlog, a 140-slot live
  gap, own-state and coordinated restarts, strict volume and launchpad journeys,
  and terminal slot-7,000 parity. All validators matched public-history root
  `58460e87a6eb3a3ac41b1c823ba6fb0cd916cdec259f11debcd793f97a89dbcd`
  and state root
  `503ff1270327d1af8ab75afa1e3be34e3b39cf1a6e6c6373245a2b927e91dc9f`.

## [0.5.263] - 2026-08-26

### Fixed
- Makes cold-migration status retrieval cache-only. Public `getHealth` and
  `getMetrics` requests can no longer synchronously inspect RocksDB
  column-family metadata or touch legacy R2/FUSE-backed cold SSTs on a
  validator's Tokio consensus runtime.
- Samples archival storage metadata once during pre-network startup and then
  only from the existing bounded cold-maintenance blocking pool. This retains
  operator telemetry without allowing monitoring traffic to create proposal,
  prevote, or precommit stalls.

### Tests
- Adds a core regression proving status reads do not create or advance a
  storage sample and that an explicit maintenance refresh populates a stable
  cached snapshot.
- Extends RPC readiness coverage to require `getHealth` to return cached
  archive telemetry without triggering an SST metadata refresh.
- Passes the complete clean four-validator gate through empty-state joins,
  bounded cold migration, authenticated Archive V2 history, all runtime roles,
  corruption quarantine and replica repair, source outages, a 96-transaction
  paused-finality backlog, live-gap recovery, individual and coordinated
  restarts, and post-activity public-history parity. The strict volume journey
  passed 140/140 checks and the launchpad journey passed 104/104 checks. All
  validators persisted slot 11,000 and matched final public-history root
  `f10274262fff36833a766b9556810a134a5f456e862d5e81b4c2404a91895c60`.

### Operations
- Records the signed v0.5.262 four-host deployment and outage/rejoin success,
  the failed live cadence gate, the measured uncached-versus-cached health
  latency, the current Archive V2/R2/capacity boundary, and the revised
  release, catalog-tail, capacity-bootstrap, role-activation, retirement, and
  final Explorer execution order.

## [0.5.262] - 2026-08-26

### Fixed
- Emits one structured `returning_validator_guarded_bft_readiness` event only
  after the returning validator has verified the hash-bound canonical
  post-effects frontier and completed either sustained moving-tip admission or
  exact-tip stalled-quorum recovery. This makes the shared safety boundary
  directly observable without treating either valid admission mode as the
  other.
- Makes the four-validator outage/corruption/repair gate require that shared
  guarded-readiness event, BFT entry, and continued tip-aligned advancement.
  The v0.5.261 release-hosted gate correctly blocked publication because its
  assertion recognized only the sustained-moving-tip message even when the
  repaired validator used the separately defined exact-tip recovery path and
  was actively committing.

### Tests
- Retains the one-slot drift, empty receive queue, ten-second sustained
  tracking, three-slot advance, exact-tip stalled recovery, canonical
  post-effects frontier, and post-admission progress requirements.
- Adds source-ordering coverage proving the common guarded-readiness event is
  emitted after the final canonical frontier gate and before BFT voting.
- Passes the complete hosted-equivalent four-validator gate through fresh joins,
  Archive V2 build/mirror/restore and mixed-role admission, corruption repair,
  source outages, moving-network rejoin, restart, volume, and launchpad paths.
  All four validators produced through slot 11,000 and matched final
  public-history manifest root
  `d35cf2631b99e65decae045251a5ad888b4e5d9472c181daa99652f84dd6a7c5`.

## [0.5.261] - 2026-08-25

### Fixed
- Replaces live BFT and moving-rejoin historical post-effect scans with a
  durable `(slot, block hash)` readiness frontier. The comprehensive completion
  marker and frontier now commit atomically only after every deterministic
  post-block effect succeeds, binding voting readiness to the exact canonical
  branch rather than a process-local scan cursor.
- Runs bounded historical crash recovery only during startup, certifies its
  result against the canonical tip, and uses constant-time, hash-bound,
  fail-closed frontier checks under the canonical apply lock before BFT starts,
  before each height, after commit, and at final moving-tip admission. This
  removes repeated historical reads and stake-pool reloads from the steady-state
  consensus path while preserving the v0.5.260 passive stability, drift, and
  pending-work admission boundaries.
- Makes a failed atomic BFT block-store operation fatal before post-block
  effects or the readiness frontier can advance, preventing a validator from
  certifying effects for a block body that was not durably stored.

### Tests
- Adds fork-replacement, atomic marker/frontier, malformed/missing frontier,
  startup recovery, snapshot round-trip, moving-admission ordering, no-live-
  historical-scan, and BFT store-failure ordering regressions.
- Bounds the four-validator gate to a 64 MB RocksDB cache per process instead
  of allowing every validator to auto-size from the host's full memory. This
  keeps the 50,000-slot Archive V2 join/restart gate within an explicit 256 MB
  aggregate block-cache budget on 16 GB development machines.
- Honors `CARGO_TARGET_DIR` in the local validator launcher and four-validator
  gate so a clean release worktree uses one explicit build directory rather
  than silently rebuilding or launching stale hard-coded binaries.
- Stages the contract-gate WASM bundle before every signed platform binary is
  compiled, ensuring the tested validator and shipped genesis/runtime binaries
  embed the same contract bytes.
- Passes all 463 validator consensus, sync, WAL, Archive V2, restart, proposal
  workload, and state-transition tests plus the full workspace suite.

## [0.5.260] - 2026-08-25

### Fixed
- Allows an already-staked returning validator to complete its ten-second
  passive voting-readiness proof while a healthy chain continues moving one
  slot ahead. The node must advance at least three canonical slots, remain
  inside the existing one-slot tracking bound, and have no queued block work;
  material drift resets the proof.
- Keeps fresh joins, post-registration admission, and stalled-quorum recovery
  on exact canonical-tip parity. The bounded moving-tip allowance is restricted
  to a known returning validator that has completed the canonical post-effects
  readiness pass.
- Prevents a drained one-block sync guard from being treated as outstanding
  work inside that bounded tracking window while retaining fail-closed behavior
  for a non-empty receive queue or a tip deficit beyond the configured bound.

### Tests
- Adds a continuous moving-network simulation covering every second of the
  passive proof, one-slot drift admission, two-slot rejection, queued-work
  rejection, and exact-tip-only stalled recovery.
- Retains all 460 validator consensus, sync, WAL, Archive V2, restart, proposal
  workload, and state-transition tests and the mandatory clean four-validator
  Archive V2 outage/rejoin gate.

## [0.5.259] - 2026-08-24

### Fixed
- Allows a returning validator whose canonical local tip exactly matches the
  authenticated network tip to complete passive voting-readiness admission
  when the block receive queue is empty, even if the sync manager still carries
  a drained batch guard. A pending block or any local-tip deficit continues to
  fail closed outside BFT.
- Applies the same drained-batch distinction before initial readiness, after
  the canonical post-effects readiness pass, and after fresh-validator
  registration so all validator roles use one exact admission rule.

### Tests
- Adds a regression proving an active but drained sync guard cannot strand an
  exact-tip validator, while queued blocks and a one-slot deficit still prevent
  voting admission.
- Retains the mandatory four-validator own-state outage/rejoin, coordinated
  restart, Archive V2 role, source-loss, corruption-repair, and immutable
  public-history parity gates.

## [0.5.258] - 2026-08-24

### Fixed
- Bounds every live BFT proposal to at most 16 user transactions, 17 total
  transaction entries including the mandatory parent certificate, and 2.8
  million aggregate declared compute units. The same limits are enforced
  before signature verification or execution of a received proposal, so
  neither a recovered backlog nor a faulty proposer can consume the proposal
  timeout and halt finality.
- Restores the validator oracle-attestation defaults to a 30-second minimum,
  60-second maximum staleness, and 10-basis-point change threshold. Explicit
  operator overrides remain supported.
- Keeps slot-addressed public block reads on slot-addressed hot/cold/Archive V2
  fallbacks. A missing recent replay body outside catalog coverage can no
  longer trigger an unbounded Archive V2 block-by-hash scan across every
  segment and monopolize validator runtime workers.
- Replays an immutable cached response for an exact duplicate snapshot-chunk
  request instead of re-exporting the same expensive category ahead of the
  receiver's next chunk. Snapshot transfers also retain their commit-certified
  source across authenticated reconnect announcement gaps, reject node or
  validator identity changes, ignore late metadata while pinned, invalidate
  pruned checkpoint advertisements, and restart replacement-source discovery
  from the short retry window.

### Tests
- Adds a four-validator regression gate that pauses finality, admits a
  96-transaction backlog, resumes the quorum, verifies every transaction
  finalizes through bounded proposals, and requires all validators to
  reconverge and continue advancing.
- Adds a state-level regression proving a missing recent block body outside the
  catalog returns without reading or decoding any Archive V2 object.
- Extends the exact four-validator gate through fresh full-archive,
  verified-cache, and consensus joins; source loss; corrupt segment/cache
  quarantine and repair; own-state and coordinated restarts; and immutable
  public-history and Archive V2 catalog parity.

## [0.5.257] - 2026-08-23

### Fixed
- Keeps returning validators passive until post-block effects are verified,
  block sync is idle, and sustained near-tip observations prove that the local
  node is stable; restart admission can no longer make a stale validator vote
  or propose while it is still catching up.
- Processes authenticated future-round proposals and votes immediately and
  retries no-progress block ranges without an artificial suppression window,
  allowing the active quorum to move past unavailable leaders and allowing a
  returning validator to converge without one-block retry stalls.
- Splits block-range responses at the exact P2P codec boundary so large sync
  ranges cannot be rejected after serialization.
- Restricts live commit notifications to consensus-active transaction metadata
  instead of falling through to Archive V2 on a current receipt miss. This
  removes multi-second event-fanout pauses from the canonical commit path.
- Keeps RocksDB reads and compact-block matching outside the shared mempool
  lock and emits per-stage slow-commit telemetry for cadence diagnosis.

### Tests
- Strengthens the four-validator Archive V2 gate so every corrupt-segment,
  repaired-segment, cache-corruption, cached-source-outage, and empty-cache
  restart must advance its own tip after voting admission while remaining
  aligned with the active network.

## [0.5.256] - 2026-08-20

### Fixed
- Authenticates Archive V2 cache objects through their catalog-pinned object
  hash and seekable envelope without implicitly decoding every block,
  transaction, and public-index frame into a live validator's heap.
- Keeps slot-addressed block and signature-addressed transaction reads on the
  seekable frame path and lowers the default explicit whole-segment decoded
  cache from eight entries to one, preventing ordinary verified-cache RPC
  activity from starving consensus memory/CPU.

## [0.5.255] - 2026-08-20

### Security
- Replaces the yanked registry `arrayref 0.3.9` package with its previously
  locked, reviewed runtime source vendored in-tree. The untrusted 0.3.10
  replacement is not consumed; runtime behavior is unchanged from v0.5.254.

## [0.5.254] - 2026-08-20

### Fixed
- Builds periodic full checkpoints in a same-filesystem hidden staging
  directory and publishes them only after the hot database, cold database,
  completion metadata, and directory entries are durable. Failed cold-store
  hardlinks no longer expose or retain a partial `slot-*` checkpoint.
- Removes only recognized incomplete numeric checkpoints and atomic checkpoint
  staging directories before opening live state, while preserving valid,
  operator-named, unknown, and symlinked entries fail-closed.
- Treats the cold-checkpoint `Operation not permitted` failure as the existing
  terminal unsupported-link condition, preventing repeated checkpoint attempts
  for the remainder of the validator invocation.

## [0.5.253] - 2026-08-20

### Fixed
- Captures the local genesis block before Archive V2 attaches its public
  historical reader, so verified-cache validators can start P2P, RPC, and BFT
  without synchronously fetching a deep-history object during startup.
- Reuses that local bootstrap snapshot for startup-mode selection and
  deterministic timestamp initialization while preserving fail-closed public
  deep-history reads after Archive V2 admission.
- Extends the verified-cache source-outage regression test to prove startup
  control-plane reads do not trigger remote archive fetches or source failures.

## [0.5.252] - 2026-08-19

### Fixed
- Makes `snapshot-hot` open only an isolated staging clone of the stopped hot
  RocksDB. Mutable metadata and WAL files plus SST symlink targets are copied
  under the existing byte bound, while immutable regular SSTs are hard-linked.
  RocksDB recovery and checkpoint writes can no longer change live state-file
  ownership or contents when the command is run by a privileged operator.
- Publishes a snapshot only after the staging source is removed, the
  self-contained checkpoint has no SST symlinks, capacity reserves pass, and
  the checkpoint directory is atomically renamed and fsynced.
- Makes bounded tail construction continue from the catalog's exact next slot
  instead of incorrectly assuming that every established catalog boundary is
  aligned to slot zero.
- Bounds Archive V2 segment-build memory by never retaining both deterministic
  encodings at once: the first immutable object is staged and released before
  a second encoding is hash-compared and discarded.

## [0.5.251] - 2026-08-18

### Fixed
- Backs checkpoint metadata retries off exponentially with deterministic
  validator jitter, and terminally pauses the known unsupported FUSE-SST
  hardlink checkpoint path instead of repeating synchronized 15-second work.
- Terminally pauses legacy cold maintenance when the reclaim queue reaches its
  immutable 4,096-range capacity instead of rescanning the same blocked work
  every five minutes.
- Emits bounded structured cadence evidence for proposal build phases and BFT
  phase timeouts, including received voting power and missing validator IDs.
- Adds a fail-closed, read-only Archive V2 role preflight covering exact
  catalog/hot-window continuity, role inventory, capacity, WAL/identity, and
  source-catalog parity.
- Adds a bounded stopped-validator hot snapshot command that materializes
  checkpoint-only SST symlinks, allowing deterministic Archive V2 tail builds
  from a stable RocksDB view without reading a concurrently compacting live
  database.
- Adds bounded catalog-tail construction and dual-R2 publication helpers that
  verify every new segment, publish immutable objects and manifests to both
  buckets before the catalog, read back every write, and resume only from the
  exact previous or exact new catalog hash.

## [0.5.250] - 2026-08-18

### Fixed
- Releases a completed catch-up guard immediately before consensus admission
  when the pending queue is empty and the local tip has reached the exact
  active batch target. This prevents a restarted validator on a continuously
  advancing network from remaining trapped in one-block catch-up cycles while
  preserving the existing fail-closed path for pending or incomplete batches.
- Serializes hot and cold RocksDB checkpoints with Archive V2 maintenance so a
  bounded cold migration cannot be captured after its hot deletion but before
  the matching cold row is visible. Checkpoints now preserve complete public
  history across the migration boundary.

## [0.5.249] - 2026-08-13

### Fixed
- Splits an oversized Archive V2 physical-reclaim range only at exact live-SST
  key boundaries until a subrange fits both the signed 32 GiB input cap and
  the configured filesystem reserve. Every split preserves the original
  covered keyspace, is durably recorded in the existing replay-compatible
  journal before compaction, and fails closed when no reducing boundary
  exists.
- Treats an already-absent `tx_by_slot` row as a skipped deterministic
  secondary index during retirement, after the signed Archive V2 block and
  transaction categories have passed complete equivalence. Present rows must
  still match byte-for-byte, conflicts still abort, and missing canonical
  history remains a hard failure.

## [0.5.248] - 2026-08-13

### Fixed
- Allows a signed Archive V2 retirement authorization to cover an explicitly
  bounded slot window inside one verified segment. Each window remains bound to
  the full segment content root, dual-failure-domain replica evidence, exact
  category proofs, stopped-validator acknowledgement, point verification,
  synchronous deletion journal, and reclaim gates. Full-segment manifests keep
  their existing encoding and behavior; older tools fail closed on subset
  windows.

## [0.5.247] - 2026-08-13

### Fixed
- Uses RocksDB batched multi-gets, bounded to 4,096 rows and 64 MiB, for Archive
  V2 retirement equivalence checks, tombstone selection, and pending-journal
  revalidation. The signed manifest, per-row canonical conflict checks,
  synchronous deletion journal, pass limits, and crash-recovery boundaries
  remain unchanged while RocksDB can coalesce remote-backed table reads.
- Upgrades `lru` to 0.18.2 to incorporate the upstream panic-safety fix for
  cache eviction and satisfy the fail-closed RustSec release gate.

## [0.5.246] - 2026-08-11

### Fixed
- Extends canonical transaction replay detection into the attached local cold
  store during speculative block execution. This prevents a transaction that
  crossed an accelerated hot-retention boundary from being included a second
  time while preserving Archive V2 and remote object stores as non-consensus
  inputs.

## [0.5.245] - 2026-08-11

### Fixed
- Adds an explicitly bounded `--max-passes-per-open` retirement option. Up to
  16 independently journaled passes may reuse one stopped-node RocksDB open,
  while every pass retains the existing 100,000-row, 1 GiB, and 60-second
  limits. This removes repeated remote-SST reopen scans without weakening
  source equivalence, crash recovery, or disk-reserve checks.

## [0.5.244] - 2026-08-11

### Fixed
- Raises the still-bounded Archive V2 retirement reclaim input ceiling from
  8 GiB to 32 GiB for wide legacy transaction families. The configured disk
  reserve and estimated two-copy compaction-peak admission remain mandatory,
  so the larger ceiling cannot bypass physical headroom checks.

## [0.5.243] - 2026-08-10

### Fixed
- Advances across multiple Archive V2 retirement categories and deletion
  batches within one bounded pass, while retaining the same aggregate row,
  byte, wall-time, synchronous-journal, and fault-recovery limits. This avoids
  reopening the stopped validator databases for every category without
  weakening source equivalence or deletion safety.
- Reclaims the first queued RocksDB range that fits the signed input and
  two-copy headroom limits instead of parking behind one larger range. Queue
  ordering remains canonical, and the original first blocked range and reason
  remain preserved when no queued range can safely advance.

## [0.5.242] - 2026-08-10

### Fixed
- Builds signed Archive V2 retirement category proofs directly from the
  already-verified segment instead of repeatedly scanning unrelated legacy
  history for every segment. Before the first journal or deletion is created,
  the destructive pass still point-checks every represented row against the
  hot and cold source stores and fails closed on a missing or conflicting row.

## [0.5.241] - 2026-08-09

### Fixed
- Derives each Archive V2 `account_txs` segment from its exact canonical block
  range and point-checks the derived keys against hot and cold source storage,
  avoiding a full account-history rescan for every segment while retaining
  missing-row and conflict detection.

## [0.5.240] - 2026-08-06

### Fixed
- Keeps bursty 30-second filesystem growth measurements as an Archive V2
  planning signal that stops optional archival/checkpoint work, without
  replacing the fixed mutable-state, WAL, compaction, and disk-floor reserve
  used for consensus-fatal shutdown.
- Raises the still-headroom-checked Archive V2 retirement reclaim input bound
  from 4 GiB to 8 GiB so an indivisible live RocksDB SST can be compacted while
  preserving the configured network floor and the estimated two-copy peak.
- Keeps the absolute network floor for a stopped builder's read-only legacy
  source without applying a writable-filesystem percentage reserve to storage
  the build cannot grow; destination staging and percentage reserves remain
  mandatory.

## [0.5.239] - 2026-08-05

### Fixed
- Compares Archive V2 block rows and legacy block rows with the same canonical
  public-history normalization during retirement authorization, re-verification,
  and journaled deletion. Locally collected commit-certificate subsets remain
  validated on import but no longer create a false mismatch against the
  deterministic Archive V2 body.
- Keeps an in-flight, fully validated block-range replay admissible when peers
  report that no verified checkpoint is available. Checkpoint discovery keeps
  retrying independently, while an explicit state-root/authenticity repair still
  pauses block replay fail-closed.

## [0.5.238] - 2026-08-05

### Fixed
- Allows bounded Archive V2 construction on the exact existing
  `lichen-testnet-1` database, which predates the atomic genesis-to-tip archive
  watermark, only when the operator supplies
  `--acknowledge-exact-testnet-missing-watermark`. The acknowledgement is
  rejected for every other network or genesis; canonical source blocks,
  parent-link continuity, finality depth, catalog ordering, deterministic
  encoding, and replicas remain mandatory.

## [0.5.237] - 2026-08-05

### Added
- Adds catalog-schema-3 legacy-loss commitments for the exact
  `lichen-testnet-1` signed-body interval `2,872,006..4,298,999`, pinned to the
  live genesis and both surviving boundary hashes. The waiver cannot be used
  by another network, genesis, interval, or boundary, and the unavailable
  bodies are never synthesized.
- Allows deterministic Archive V2 segment construction to resume after that
  exact root-committed interval while still requiring every source block in
  each constructed range and preserving segment codec V2.
- Exposes signed, bounded, journaled `retirement-authorize`,
  `retirement-pass`, and `retirement-reclaim` operator commands. Destructive
  passes require explicit stopped-validator and V2-only-rollback
  acknowledgements; signed retirement anchors bind both segment and catalog
  format versions, and physical reclaim enforces the network capacity floor.

### Fixed
- Disables inherited SSH connection multiplexing in the fleet deployment
  script so parallel hosts cannot share a socket and silently stage an
  artifact on the wrong validator.

## [0.5.236] - 2026-08-05

### Fixed
- Gives fresh Archive V2 role joins the real 50,000-slot public-network recent
  history window instead of reusing the accelerated gate's synthetic 20-slot
  cold-migration boundary, so immutable catalogs remain valid while the live
  head advances during state sync.
- Keeps canonical recent account snapshots enabled for full-archive,
  verified-cache, and consensus roles while independently disabling legacy
  cold migration where required, preserving post-journey public-history
  parity without making remote archive availability consensus-critical.

## [0.5.235] - 2026-08-04

### Fixed
- Bounds startup's hot-only recent post-block recovery to the configured
  hot-retention window, excluding only the older prefix already migrated to
  cold storage while still failing closed on any missing retained block.
- Keeps the explicit offline repair path on verified public history so
  retroactive repairs can still cross the legacy hot/cold boundary.
- Aligns Archive V2 role admission in the accelerated four-validator gate with
  that gate's configured hot-retention window.
- Anchors the deterministic Archive V2 runtime catalog to the validators'
  actual stopped finalized range with bounded hot/catalog overlap, closing the
  gap that could form while checkpoint parity was computed before admission.
- Preserves the 50,000-slot public-network role minimum while allowing an
  explicit local `--dev-mode` gate to exercise the identical hot/archive
  admission boundary with accelerated retention.
- Makes the role/restart matrix append newly finalized legacy history to its
  immutable catalogs before each direct worker start, proving restart safety
  without a supervisor race or a stale catalog waiver.
- Pins the authenticated genesis hash for validator-announcement capability
  checks so consensus-role denial or a verified-cache source outage cannot
  reject validator peers or stall quorum through an unrelated deep read.
- Keeps an established nonzero validator state out of the fresh-join genesis
  wait when an admitted verified cache correctly fails a public slot-0 read
  during source outage; fresh joiners still require canonical genesis sync.
- Requires a sustained block-production burst within the bounded BFT recovery
  window between sequential Archive V2 repair and source-outage restarts, so
  each recovery reaches live consensus before the next deliberate interruption.

## [0.5.234] - 2026-08-04

### Fixed
- Keeps consensus-critical recent post-block recovery on the physically
  verified hot window after Archive V2 admission, so a verified-cache source
  outage cannot prevent validator startup merely because the Archive V2
  catalog also covers those slots.
- Preserves fail-closed public deep-history reads during source loss; the new
  hot-only lookup is limited to internal recent-window recovery and still
  aborts if the required canonical hot block is absent.

## [0.5.233] - 2026-08-04

### Security
- Upgrades the smart-contract runtime from Wasmer 4.4.0 to 5.0.6 so its
  transitive archive validation dependency moves from vulnerable `rkyv 0.7.46`
  (`RUSTSEC-2026-0235`) to patched `rkyv 0.8.17`.
- Adds an owned, expiring informational-advisory exception for Wasmer's
  build-time-only `proc-macro-error2` helper, which remains required through
  the latest Wasmer release and is not linked into validator runtime code.

## [0.5.232] - 2026-08-04

### Fixed
- Aligns the Archive V2 corruption drill with the documented pre-retirement
  dual-reader policy: corrupt V2 objects must be quarantined while an exact
  canonical legacy fallback remains available.
- Retries transient post-start genesis RPC reads during exact-gate resume while
  still aborting on any persistent hash mismatch.

## [0.5.231] - 2026-08-04

### Fixed
- Tracks the authenticated Archive V2 HTTPS fixture required by the exact-tag
  four-validator fresh full/verified-cache/consensus join and source-outage
  release gate.
- Extends release static QA so every non-JavaScript support asset invoked by
  the archive parity harness must exist, be Git-tracked, and remain unignored.

## [0.5.230] - 2026-08-04

### Added
- Introduces Archive V2 as a versioned, content-addressed segmented history
  format with deterministic catalogs, seekable Zstandard frames, canonical
  transaction deduplication, authenticated replicas, verified cache reads,
  corruption quarantine, repair, and restore tooling.
- Adds a retroactive builder that reads already-existing legacy hot and cold
  history in fixed resumable ranges. Every segment is reconstructed and
  verified against the canonical block, transaction, index, and public-history
  commitments before it can be admitted or replicated.
- Adds explicit full-archive, verified-cache, and consensus validator roles,
  P2P capability advertisement, role-bound storage markers, capacity telemetry,
  and fail-closed fresh-sync admission.
- Ships the `lichen-archive-v2` migration, mirror, repair, and restore CLI in
  every signed release archive and verifies the installed tool against the
  release checksums during deployment.

### Changed
- Makes legacy cold migration resumable and capacity-aware with durable
  chain-bound cursors, bounded row/byte/time passes, write-before-delete
  recovery, validator-specific scheduling jitter, and bounded physical reclaim.
- Integrates Archive V2 dual reads into validator, RPC, snapshot, checkpoint,
  and public-history paths without changing consensus, wire encoding, block or
  transaction identity, signatures, state roots, or public RPC objects.

### Safety
- Keeps legacy history authoritative during the initial deployment. Legacy
  retirement remains disabled until exact parity, replica acknowledgements,
  authenticated restore drills, and a signed dual-reader rollback anchor are
  proven for the live chain.
- Updates the transitive `ruint` dependency to 1.20.0, resolving
  RUSTSEC-2026-0220 in checked, saturating, and overflowing shift operations.
- Pins the root JavaScript toolchain to patched `undici` 7.29.0 and raises the
  Python SDK cryptography floor to 50.0.0 for the current npm and Python audit
  advisories.
- Requires adequate writable staging and compaction headroom for retroactive
  conversion. The release does not authorize deleting empty blocks, mutable
  validator state, WALs, keys, identities, or provider backups, and it does not
  weaken the configured disk reserve.

## [0.5.229] - 2026-07-22

### Fixed
- Canonicalizes the six exact `lichen-testnet-1` account-snapshot before
  images left by the legacy replay-drift incident. The compatibility path is
  gated by the exact chain ID, slot, account keys, serialized-value hashes,
  and independently sourced before/after balances; every unknown row remains
  unchanged and therefore still fails archive parity closed.
- Applies the same canonicalization to public-history manifests, exports,
  conflict classification, imports, and point-in-time account reads. The raw
  US before images remain untouched on disk as recovery provenance, while the
  logical public-history view matches the three independently agreeing
  validators.
- Aligns RPC/Explorer disk readiness with the explicit 5 GiB available-space
  floor instead of also applying a hidden 95%-used threshold. Percentage use
  remains visible as telemetry; the validator runtime still enforces its
  network-specific reserve and keeps the 10 GiB floor outside testnet.

### Operations
- Allows the six source-backed repair-slot snapshot rows already present on US
  to be imported additively into EU, SEA, and IN. No account snapshot, state,
  WAL, key, identity, block, transaction, or archive row is deleted.
- Adds a read-only, slot-bounded public-history category manifest so the five
  SEA-only canonical transaction-metadata rows can be isolated by digest and
  repaired from their exact source without transferring or rewriting unrelated
  history.
- Retains the testnet-only 5 GiB runtime reserve and 50,000-slot hot-history
  boundary while Archive V2 segmented compression is implemented.

## [0.5.228] - 2026-07-21

### Fixed
- Corrects the legacy testnet replay-drift repair so each source-bound account
  before image validates `spores` and `spendable` independently. Validator
  accounts retain their exact 100,000 LICN bonded difference while both fields
  receive the same zero-sum replay correction. The repair still projects and
  requires the exact child-certified target root before its single atomic
  commit.
- Bumps the write confirmation and durable completion marker to v2 so the
  invalid v0.5.227 repair path cannot be confused with the corrected operation.
  Total-spore and spendable conservation, per-account delta equality, fixed
  chain/tip/block/root checks, stake-pool before-image checks, and post-commit
  sparse-root verification all fail closed.

### Operations
- Supersedes v0.5.227 for the affected US validator repair. The v0.5.227 repair
  dry-run stopped at the first bonded validator account before any live write;
  v0.5.227 must not be used to execute that repair.
- v0.5.227 remains restart-safe for validators already on canonical state. It
  restored the EU, SEA, and IN quorum while US remained stopped, preserving the
  US database, WAL, keys, identity, and archives for this corrected signed
  release.
- Retains the testnet-only 5 GiB runtime reserve and the 50,000-slot default
  hot-history window. These remain an emergency bridge to Archive V2, not a
  mainnet capacity approval.

## [0.5.227] - 2026-07-21

### Fixed
- Corrects the initial post-block-effects recovery boundary. A validator whose
  durable verified cursor is exactly `activation_slot - 1` now scans only the
  configured recent recovery window instead of treating the activation slot as
  a resume cursor. This prevents a restart from interpreting intentionally
  pruned historical markers as unapplied economic effects and replaying old
  rewards a second time.
- Adds a chain-, tip-, block-, root-, account-, and stake-pool-bound repair for
  the one `lichen-testnet-1` validator affected before the boundary defect was
  found. The command defaults to dry-run, requires an exact confirmation for
  writes, projects the complete child-certified target root before one atomic
  account/stake commit, rebuilds the sparse commitment, repairs the sidecar
  anchor, and is safe to rerun. Any unknown byte, value, tip, or root aborts.

### Changed
- Retains the v0.5.226 emergency bridge: the exact `testnet` selector uses a
  temporary 5 GiB hard runtime reserve, other production selectors retain 10
  GiB, and the default hot-history window is 50,000 slots with write-first
  transparent cold migration.
- Extends the mandatory four-validator release drill with an own-state outage
  and catch-up after real volume/launchpad activity, followed by canonical
  certificate and stopped hot/cold archive parity checks.

### Operations
- Supersedes v0.5.226 before fleet deployment. The affected validator is
  repaired from its own preserved database using exact canonical evidence; no
  peer database, snapshot, WAL, key, identity, or synthetic history is copied.
- Requires a coordinated signed-artifact deployment because restart recovery
  and default storage behavior change. Signed v0.5.225 is preserved as the
  pre-change artifact but is not a restart-safe rollback because it contains
  the replay-boundary defect. Once signed, v0.5.227 becomes the first safe
  restart anchor; staging failures abort before the fleet stop, and post-stop
  recovery fails forward on the same verified v0.5.227 artifact.

### Known issue
- The legacy replay-drift repair command in this release incorrectly expects a
  validator account's spendable balance to equal its total balance. Its dry-run
  fails closed on the first bonded validator account and writes nothing. Do not
  execute the v0.5.227 repair command; use v0.5.228 or later. Normal validator
  restart recovery in v0.5.227 is unaffected.

## [0.5.226] - 2026-07-21

### Changed
- Temporarily lowers the hard runtime disk reserve from 10 GiB to 5 GiB only
  for validators started with the exact `testnet` network selector. Mainnet,
  unclassified production, and missing-network invocations retain the 10 GiB
  floor. Startup, runtime monitoring, checkpoint reclamation, and verified
  snapshot capacity checks all use the same selected reserve.
- Reduces the default recent hot-history window from 100,000 to 50,000 slots.
  Eligible history continues through the existing cold archive's write-first,
  same-key-verifying, WAL-synced migration path; this release does not prune or
  weaken backed historical RPC data.

### Operations
- Adds the complete Archive V2 segmented-storage plan covering immutable
  seekable Zstandard segments, canonical indexes, full/verified-cache/consensus
  validator roles, replication, restore, legacy migration, adaptive capacity,
  security, observability, benchmarks, failure drills, and rollout gates.
- Originally retained signed `v0.5.225` as the immediate rollback point. The
  later restart incident proved that assumption unsafe on the mature activated
  testnet; v0.5.226 was therefore superseded before deployment and must not be
  started there. The 5 GiB floor is an emergency testnet availability bridge,
  not a mainnet capacity approval or a substitute for planned larger storage.

## [0.5.225] - 2026-07-20

### Fixed
- Serializes periodic validator-set and stake-pool reconciliation with canonical
  block application. A reconciler can no longer read an older persisted pool,
  wait while catch-up commits newer blocks, and then overwrite the live pool
  with that stale snapshot.
- Treats a quorum-authenticated parent post-state mismatch as a typed recovery
  condition instead of retrying the same block forever. A single missing
  parent-producer counter may be repaired only when an uncommitted candidate
  produces the authenticated child's exact expected root; every other mismatch
  transitions to verified checkpoint repair.
- Prevents the block receiver from signing votes for already-committed catch-up
  blocks. Historical sync still verifies certificates, transaction execution,
  post-block effects, and archive writes, but voting resumes only through live
  consensus paths.
- Prevents a full hot/cold RocksDB checkpoint from pinning obsolete SST files
  until the validator reaches its runtime disk safety exit. Checkpoint pressure
  now uses allocated blocks and hard-link ownership to measure bytes that can
  actually be reclaimed; active hot/cold SST links are never counted.
- Reclaims complete and interrupted derived checkpoint directories proactively
  when periodic checkpoint creation encounters its 20 GiB floor. A replacement
  is created only after free space is rechecked. Startup and runtime reclaim
  only when exclusive checkpoint bytes can restore the 10 GiB floor, otherwise
  they preserve data and fail closed with status 78.
- Serializes checkpoint creation with runtime checkpoint reclamation and skips
  duplicate same-slot creation, so disk-pressure maintenance cannot remove a
  checkpoint while another block-processing path is constructing it.
- Updates release trust-anchor QA to parse the current mainnet runbook wording,
  fixing the false hosted-CI failure after the signed `v0.5.224` rollout.

### Operations
- Recovers IN's exact one-counter post-state divergence without copying another
  validator database or changing public history. Canonical producer counts prove
  that slot `9,736,991` is missing only SEA's one production increment; the
  signed child at `9,736,992` supplies the required target root.
- Recovers IN from its own preserved `v0.5.224` database by removing only the
  derived `slot-9683000` checkpoint. No hot state, cold archive, identity, key,
  WAL, or canonical history was removed or copied.
- Keeps `v0.5.224` as the immediate signed rollback for this release. Exact
  source-backed repair closes the former incomplete slot `5,276,000`; the only
  remaining existing-testnet waiver is unavailable signed bodies
  `2,872,006..4,298,999`, and it does not apply to fresh networks or mainnet.
- Advances publish candidates to `lichen-contract-sdk 1.0.4`,
  `lichen-client-sdk 0.1.7`, and `@lobstercove/lichen-sdk 1.0.7` alongside
  `lobstercove-lichen-core` and `lichen-cli 0.5.225`.

## [0.5.224] - 2026-07-17

### Known Testnet Limitation
- The existing `lichen-testnet-1` source set irrecoverably lacks signed block
  bodies `2,872,006..4,298,999`. This release does not hide or synthesize that
  legacy interval. The owner accepted it only to upgrade the testnet that found
  the archive-design defect. Mainnet startup remains fail-closed unless its
  durable archive proof covers and independently verifies every linked signed
  block body and required transaction index from genesis through canonical tip.

### Fixed
- Bounds canonical transaction-history pagination to the current page frontier.
  Repair exports no longer rescan the entire remaining `tx_by_slot` archive
  after every page, and rows already sourced from canonical block bodies avoid
  duplicate block lookups. This changes a multi-million-slot transaction/index
  repair from quadratic archive work to cursor-bounded work while preserving
  the source-backed fallback path for indexes whose block body is unavailable.
- Moves bounded public-history repair pages directly between source and target
  VPSes through a pinned transient SSH relay when a local agent is available.
  The helper forwards only the selected identity, never copies a private key or
  changes system SSH access. It opens one pinned source-to-target SSH control
  connection per target and reuses it for every page, avoiding the deliberate
  VPS new-connection rate limit. Source/target SHA-256 and byte counts must
  match before atomic page promotion, and every control, relay, and page file
  is removed on exit. Local bounded transfer remains the automatic no-agent
  fallback. Export, compression, decompression, and import now run entirely
  inside the unprivileged `lichen` subprocess, so a host with `sudo` I/O
  auditing records only bounded command/report metadata instead of duplicating
  block payloads into `/var/log/sudo-io` and exhausting the validator disk.
- Uses the guarded consensus-v1 activation slot as the deterministic native
  transaction signature boundary. Historical blocks below the boundary retain
  the bounded `v0.5.223` chain-domain-then-legacy transition policy, while
  blocks at and above it require the canonical chain-ID domain with no fallback.
  Existing public chains use
  `--prepare-consensus-v1-activation` at one common stopped `tip + 1`; fresh
  chains activate at slot 1, and malformed activated metadata fails closed.
- Makes custody wrapped-credit mint transactions fetch `getNetworkInfo.chain_id`
  and sign for that exact network. A confirmed bridge deposit can no longer
  reach a legacy-signed mint that strict V1 RPC admission will reject.
- Commits canonical transaction execution, durable receipts, block body,
  transaction slot indexes, archive watermark, and tip/finality cursors in one
  RocksDB batch. Validator oracle-attestation projections join the same batch,
  removing the restart window where a transaction was processed but its block
  was absent.
- Makes checkpoint finality independent of the receiver's current validator
  set. The checkpoint proof uses exact historical parent and child power
  denominators, commits the parent post-effects root in the child certificate,
  proves certificate inclusion at child transaction index 0, and verifies the
  signed/finalized child header before snapshot bytes can be imported.
- Consolidates the July validator liveness release line with archive parity
  hardening so stalled sync retries keep accepting delayed block-range
  responses while still retrying stale requests.
- Returns a live validator to bounded sequential catch-up when it observes a
  material canonical gap or enters checkpoint repair. Raw future-block receipt
  no longer counts as watchdog progress: only a canonical commit or accepted
  verified snapshot chunk can refresh liveness. This prevents an active process
  from remaining indefinitely stalled behind a stream of unchainable future
  blocks after checkpoint activation.
- Validates the complete binary ABI layout descriptor before entering layout
  mode. A raw account or contract address beginning with the marker byte
  `0xAB` can no longer be misread as descriptor metadata and corrupt the
  authenticated cross-contract caller. Rust contract SDK helpers and DEX token
  custody calls now emit canonical descriptors explicitly; unit, nested-call,
  preserved-state, and full trading regressions cover the collision.
- Restores the signed `v0.5.223` future-round proposal replay path that was
  absent from `main`, so a proposal received before its round is reached is
  processed immediately when that round becomes current.
- Keeps restarted validators out of BFT voting until their canonical tip has
  reached the observed network tip and active catch-up/pending parent-gap work
  has drained. This fixes the four-validator seed-failover stall where a
  restarted validator was counted in the 4-validator quorum while still syncing,
  leaving only two effective voters after the seed stopped.
- Separates local tip initialization from authenticated peer-tip evidence in the
  resumed-validator startup gate. A configured bootstrap RPC that is offline no
  longer deadlocks an already connected surviving quorum at equal tip, while a
  node with no direct or signed on-chain peer observation still waits and an
  ahead peer still forces canonical catch-up before voting.
- Tracks the exact canonical slot range represented by the in-memory recent
  blockhash cache. Re-indexing only a recovered tip after restart can no longer
  make that one hash masquerade as the complete replay-protection window and
  reject a valid synced transaction that references an older still-recent hash.
- Repairs validator mesh maintenance so same-IP/different-port validators are
  discovered as distinct peers and reconnect pressure fills available peer
  capacity from the durable peer store instead of depending on the seed.
- Clears block-range request markers when no connected peer actually received a
  sync request, so initial catch-up retries immediately once peers connect
  instead of waiting for a stale in-flight marker TTL.
- Makes deterministic epoch post-block effects fail-closed and commit the
  reward marker only after account rewards, stake pool state, MossStake state,
  governance parameter changes, and mint counters are staged in one atomic
  batch.
- Normalizes RPC, P2P, and validator-forwarded transaction admission around the
  canonical chain-id signature verifier and execution-equivalent recent
  blockhash / durable-nonce freshness check before mempool or gossip.
- Requires peer checkpoint snapshots to carry the complete state and hot/cold
  public-history surface, including historical account snapshots, and rejects
  checkpoints with missing, header-only, or non-parent-linked blocks between
  genesis and the checkpoint slot.
- Separates incomplete-snapshot crash rollback from peer archive sync. Snapshot
  completion now requires the expected slot/root and complete contiguous public
  history; rollback restores every exact pre-apply hot category and account
  history counter while preserving the validator's independent cold archive,
  persists a recovered checkpoint, and removes the recovery marker last.
- Upgrades `crossbeam-epoch` in the root, compiler, fuzz, contract SDK, and Rust
  client SDK locks to clear RUSTSEC-2026-0204 without an advisory exception.
- Pins the prerelease PKCS#8 API required by the current ML-DSA and SLH-DSA
  crates. Fresh standalone compiler and SDK lock resolution can no longer select
  the incompatible `pkcs8 0.11.0` final API, and CI now verifies the manifest
  compatibility anchor before release.
- Builds and tests all contracts through one shared Cargo cache in the
  non-runtime `target/contract-build` namespace, preventing per-contract cache
  duplication without shadowing the shipped `contracts/` tree. Development
  genesis discovery now searches working-directory ancestors before executable
  ancestors, and its global environment test guard restores state after panics.
- Removes synchronous genesis-to-checkpoint archive rescans from snapshot chunk
  requests. Requests now use only exact background-verified cache entries, and
  immutable manifests are reused only for an unchanged primary checkpoint.
- Enforces a numeric non-root compiler sandbox identity and completes the C
  toolchain with the WASM linker used by the Rust/C/AssemblyScript smoke gate.
- Restricts the `lichen-contract-sdk` crate archive to its Rust source instead
  of implicitly publishing the JavaScript and Python development trees.
- Removes the duplicate DEX analytics producer that counted every matched trade
  in both `dex_core` and the validator bridge. Committed `dex_trade_*` rows are
  now the sole source for an atomic, restart-safe analytics projection.
- Adds a deterministic analytics v2 migration that rebuilds counters, trader
  stats, leaderboard and 24-hour activity from canonical history, compacts
  timestamp candles, rejects missing trade/block history, and advances its
  cursor in the same batch as the repaired state.
- Uses committed per-trade block timestamps during bridge catch-up and bounds
  all candle intervals with a shared zero-based ring, eliminating duplicate
  periods, sparse indexes and unbounded candle storage growth.
- Adds block-hash-bound producer and comprehensive post-effects markers covering
  reward, stake, vesting, oracle, validator activation, analytics, SL/TP,
  rollover and governed activation. Existing public chains must stop and align
  every validator at one exact tip, then use the guarded activation command to
  WAL-sync the same `tip + 1` boundary on each database. Startup exits with
  persistent status 78 when that boundary is absent instead of choosing a
  node-local height. Fresh chains initialize slot 1. Missing markers before the
  boundary are unverifiable and are never replayed; activated missing markers
  are repaired only from a present canonical block, while a missing block fails
  closed. Analytics v2 also waits for the shared boundary, so a lagging joiner
  cannot migrate at an earlier historical slot. Repeated passes are exact
  no-ops, and offline repair execute refuses a database without the boundary.
- Commits oracle mirrors, candle metadata, 24-hour rollover, SL/TP order and
  margin settlement, insurance accounting, trader payout and replay cursors in
  canonical-slot atomic batches instead of independent fail-open writes.
- Makes fee configuration and required treasury reads fail closed, uses checked
  fee allocation/debits, and advances founding vesting from every canonical
  block timestamp even when the block has no fees.
- Restores exact candle API limit semantics so a request never returns more
  items than requested or more than the retained ring contains.
- Makes public-history range repair exactly inclusive at `--to-slot` inside the
  canonical slot and transaction iterators. Large pages can no longer import
  later slots, and a final slot with more transactions than the page size is no
  longer truncated.
- Makes contract-storage, stake-pool, and state-commitment inspection strictly
  read-only and permits them to run through a disposable RocksDB secondary.
  Diagnostic root reporting no longer invokes the cold-start sparse rebuild,
  and sparse rebuild/activation commands reject secondary mode so an operator
  cannot mistake an inspection path for a writable repair.
- Serializes canonical state writes with sparse commitment mutation at the
  `StateStore` boundary. A root computation can no longer delete a dirty marker
  belonging to a newer same-key contract or account write and leave a stale
  sparse root behind. Startup now verifies every supposedly clean active sparse
  commitment against canonical accounts and contract storage, rebuilds on any
  mismatch or untrusted marker state, and verifies the rebuilt result before
  tip anchoring or BFT startup.
- Bounds sparse Merkle cache storage by atomically deleting superseded rooted
  path nodes with each canonical root update. The stopped-node rebuild clears
  only derived sparse node/leaf caches with a bounded range tombstone and scoped
  compaction before reconstructing them from canonical accounts and contract
  storage; repeated-update, current-proof, and checkpoint-root regressions cover
  both account and contract trees.
- Makes hot-to-cold migration use total-order RocksDB iteration so point-lookup
  tuning cannot silently hide old hot rows. Stopped-node audit and both
  migration paths now fail closed unless every raw block hash, canonical slot
  cursor, block-referenced transaction body, and exact transaction-to-slot row
  is valid in hot or cold storage. Migration WAL-syncs before hot deletion and
  compacts bounded hash ranges to avoid requiring a second full archive's free
  space.
- Makes execute-mode fleet history repair run its own complete read-only target
  dry run before stopping any validator. Import reports include missing
  key/value bytes; any conflict aborts before writes, and capacity must cover
  150% of measured missing bytes plus the runtime reserve instead of satisfying
  an unrelated nominal disk-size threshold.
- Reuses one multiplexed SSH transport per validator during fleet archive
  verification. Read-only health and historical probes no longer trip the
  hosts' intentional six-new-connections-per-30-seconds UFW limit, and a failed
  initial connection waits through that firewall window before retrying. The
  verifier logs directly to its evidence file and closes every control master
  in one explicit exit path, so successful checks leave no local shell/logger
  processes behind.
- Makes archive-backed hot/cold storage automatic for every non-development
  testnet and mainnet validator. The cold archive is the canonical
  `archive-<network>` sibling of its state directory; public runtime archive
  flags are rejected, so new and resumed public validators cannot silently
  operate as state-only nodes.
- Removes query-string custody WebSocket credentials and generic cross-chain
  route configuration in favor of header-only authentication and route-specific
  RPC, treasury, multisig, token, and confirmation settings.
- Removes obsolete public command, RPC, response, reserve-proof, explorer,
  marketplace, and contract-host aliases so clients and operators have one
  current interface instead of silent compatibility fallbacks.
- Repairs the clean local release launcher to use `lichen identity new` and to
  generate the requested validator identity count instead of hard-coding three.
- Moves every maintained E2E transaction sender to the chain-bound V1 binary
  envelope and canonical positional `callContract` parameters. A source guard
  prevents JSON transaction transport from returning to user journeys.

### Changed
- The local release gate now requires every validator's own RPC tip and
  consensus `last_active` slot to remain within 20 slots of the final reference
  tip. Lifetime proposal/vote counters no longer allow a stalled validator to
  pass the final activity check before archive parity detects the drift.
- The release workflow now runs the four-validator hot/cold public-history
  parity gate plus the complete volume/user and launchpad/governance journeys
  before publishing binaries, and the rolling deploy script treats uninspectable
  or consensus/sync/archive-touching releases as consensus-critical by default.
- The tag workflow now verifies the tag against every deployed crate version,
  runs locked formatter/Clippy/workspace/security gates, and tests all contracts
  before staging the genesis contract bundle.
- Local Make, SDK, contract, E2E, and piped QA commands now propagate failures
  instead of reporting success after a failed child command.
- Makes running-chain CI wait for an actually healthy validator instead of any
  HTTP `getHealth` response, and makes the comprehensive CLI integration suite
  return nonzero whenever its own failure counter is nonzero. A stale local
  database can no longer be mistaken for a ready release test chain or leave a
  counted CLI regression hidden behind a successful job exit.
- Adds `scripts/verify-testnet-archive-parity.sh` for fleet-level archive
  evidence across US, EU, SEA, and IN, including strict stopped-validator
  manifest comparison for the release gate.
- Adds page-level public-history export/import admin commands plus
  `scripts/stream-public-history-repair.sh`, so live repair can stream verified
  history from EU/source into targets without copying another validator DB.
- Adds bounded binary public-history pages for large block-body repairs,
  avoiding JSON/base64 overhead while recording each page checksum and
  preserving source-backed additive imports and same-key conflict aborts.
- Makes fleet SSH retries preserve nonzero exit status and atomically replace
  per-attempt files, and makes comprehensive TypeScript SDK compilation/tests
  propagate their original failures instead of returning success through `!`.
- Runs each bounded page's read-only target validation concurrently across the
  independent fleet while keeping all execute imports sequential and quiesced.
- Sends compressed repair pages over SSH and decompresses under target-side
  `pipefail`, avoiding redundant raw block-body uploads to every validator.
- Makes public-network fleet verification and streamed history repair derive the
  same canonical cold archive as the runtime; neither operator path passes the
  now-rejected development archive flags.
- Runs remote archive-parity and stream-repair admin commands under an explicit
  high file-descriptor limit so RocksDB-heavy inspections do not fail from a
  low interactive shell default.
- Adds read-only contiguous block-range proof with canonical body, header,
  parent-link, and deterministic digest checks. Live stream repair now refuses
  unbounded block writes, mixed candidate hashes, missing current backups,
  incomplete/conflicting target dry runs, or insufficient measured write
  headroom.
- Adds an offline fleet repair gate that compares fixed-tip manifests while
  deliberately leaving every validator stopped; restart is a separate,
  coordinated action after parity succeeds.
- Treats a validator-set-wide historical `Block not found` range as a release
  blocker, not a parity success; the current July chain must be repaired from
  exact backed bodies and must not be reset or synthesized to hide the gap.
- Adds locked standalone compiler, contract SDK, Rust client SDK, fuzz, and
  compiler-container gates to CI and the release workflow, with target cleanup
  between workspaces to stay within hosted-runner storage limits.
- Serializes the release container's final LTO build by default through the
  configurable `CARGO_BUILD_JOBS` build argument, preventing an 8 GiB builder
  from killing the validator link while other release binaries link in parallel.
- Advances publish candidates to `lichen-contract-sdk 1.0.3`,
  `lichen-client-sdk 0.1.6`, and `@lobstercove/lichen-sdk 1.0.6`; publication
  remains gated with the unreleased `0.5.224` core/CLI release.
- Treats analytics v2 as a coordinated state-projection upgrade: mixed-version
  rolling deployment is prohibited and complete canonical DEX trade/block
  history is a precondition for activation.

### Verified
- Rebuilt the exact candidate after the ABI marker-collision repair and passed
  the authoritative four-local validator gate. V4 resumed in the same process
  after a 140-slot pause, V4 and V1 restarted from their own state, the chain
  finalized with V1 offline, and all four restarted from one preserved tip.
  Canonical certificate parity matched at slot `930`
  (`e4152bd9...103d5ae`); checkpoint-1000 hot/cold manifests matched root
  `de87f503...174589d`; volume/trading passed 140/140; launchpad, governance,
  and graduation passed 104/104; and post-journey checkpoint-3000 manifests
  matched root `3b764af7...34c55be`. Transcript:
  `evidence/post-block-effects-recovery/testnet-20260715T-final/four-local-abi-marker-final.log`,
  SHA-256 `72eafc1a75cfc15e4c03d482fb057536e019cc306a7968a7fa844dcb95046fb5`.
- Passed the exact-source ten-validator expansion. V2 through V10 joined from
  empty stores, V10 closed a 140-slot gap in the same process, individual and
  coordinated preserved-state restarts resumed finality, and the chain
  advanced 42 blocks in 15 seconds with V9 and V10 stopped. All ten proposed
  and voted, certificate parity matched at slot `2718`
  (`3a74f369...043a0d8`), and checkpoint-3000 hot/cold manifests matched root
  `c85f3ca9...6750a2`. Transcript:
  `evidence/post-block-effects-recovery/testnet-20260715T-final/ten-validator-abi-marker-final.log`,
  SHA-256 `4f226b6efea98edb41f783dd8da1c913fb39420c900ea6b3963f610ee85701b7`.
- Completed the final locked workspace all-target/all-feature test suite and
  strict workspace Clippy with `-D warnings` after the ABI hardening. Transcript
  SHA-256 values are `841323c0...34004a` for tests and
  `40f66495...04364` for Clippy.
- Passed the final uninterrupted standalone release matrix after aligning
  contract tests with the canonical SDK layout: compiler 30/30, contract SDK
  28/28, Rust client SDK 88/88 plus docs/examples, all fuzz binaries, every one
  of the 34 native contract workspaces, 33 active/genesis WASM builds, the
  standalone MT-20 WASM build, and helper guards 12/12. Transcript SHA-256:
  `baa5788d4c9b96272c7aec434f7cb69316c80560e18b39931e8e4815e506d39f`.
- Passed the exact candidate compiler sandbox as numeric non-root UID 10001 for
  Rust, C, and AssemblyScript WASM (transcript SHA-256
  `b61fe3061c035428bab6d3650a9d8c39d9da4ce6887f755b37e11916249bd2fe`)
  and generated valid CycloneDX JSON for all eight root workspace packages.
- Passed a fresh isolated running-chain gate while preserving and restoring the
  existing local databases: comprehensive RPC 146/0/1, CLI 29/0,
  deterministic E2E 25/0, and full RPC/DEX REST 146/0/1. Transcript SHA-256:
  `91b1c392f64fc329ea91856652bafd8799dd543ec107578f2d0543798f155898`.
- Rebuilt IN's unbounded v0.5.223 derived sparse cache offline with the audited
  exact-tag maintenance binary. Typed root verification passed at stopped slot
  `9,180,291`, protected and cold-archive metadata hashes were unchanged, and
  checkpoint `9,181,000` raised free space from about 17.2 GB to 70.1 GB. The
  post-restart four-host verifier passed at a 47-slot spread with identical
  fixed-block digests and zero service warnings/restarts; no candidate binary
  was installed.
- Passed the exact final measured-repair source through the locked full
  workspace all-target/all-feature suite. Transcript SHA-256:
  `919514bc0917f063530684262aaac1d69478e6044de1ec203d36c41f58827bbf`.
  The authoritative four-validator archive-cold gate also passed from the same
  source: V2/V3/V4 joined from empty stores; V4, V1, and then all validators
  resumed their own preserved state at spread 0; the chain finalized with V1
  offline; all four matched canonical certificate
  `e62465d66e9468ddd32b0f8fe97cea11f3ac1af9a2efdd17195069408231661b`
  at slot 752; checkpoint-1000 hot/cold manifests matched root
  `74636686878627a9515433b21b429ef048ad1ba44803104948dce7f564174bae`;
  volume/user journeys passed 140/140; launchpad/governance/graduation passed
  104/104; and checkpoint-3000 post-journey manifests matched root
  `952221eaf8e975987ce93ec9abd3794d5e84faa16b3928dd1c17aac5d34c104e`.
  Complete four-validator transcript SHA-256:
  `7de7f95b3999fac9b5792394874fa96305f2a70fa24bcebd23702f1adaa6ad2f`.
  The exact final Linux artifact and EU repeat audit remain release gates.
- The previous no-cache Bookworm `linux/amd64` candidate SHA-256
  `6b5f79d16654c02990c2c9b40e4ca8656a29a5106048e1872b72fcac9ca62325`
  passed Core 983/983 plus integrations, Validator 396/396, the full locked
  workspace, strict Clippy, helper guards 12/12, and the authoritative
  four-validator gate through checkpoint 3000. It is superseded by the
  total-order migration and measured import-preflight fixes and must not be
  installed. Final full gates and a clean Linux build are required again.
- Completed the guarded EU rollback to slot `8,915,275` and root
  `cbf7770f...03d3a` without changing protected sidecars. Provider-restored
  file ownership was normalized with content hashes unchanged. A focused-tested
  full-replay-compatible `v0.5.223` bridge advanced approximately 1,650 slots
  without a snapshot marker, staging residue, crash, or restart, then was stopped
  when measured free space approached the 10 GiB floor. A corrected total-order
  dry run then found 2,453,338 old hot blocks (60,658,298,656 bytes), 1,467,110
  transaction rows (7,965,048,674 bytes), and 1,467,110 transaction-slot rows
  (11,736,880 bytes), all missing from cold with zero conflicts. The final
  bounded execute migrated all 2,453,338 eligible blocks in 246 compaction
  batches, raised free space from 10.94 GB to 20.79 GB, and passed a zero-row
  post dry run plus a 6,513,019-row raw integrity audit. This supersedes the
  fixed 500 GiB conclusion; catch-up then exposed the separate derived sparse
  cache retention issue fixed above.
- Built the exact-tag `v0.5.223` sparse-maintenance/full-replay bridge as a
  stripped Linux x86-64 binary with SHA-256 `9b71e7a9...ccee`; its optimized
  cache and replay-selection regressions pass. On stopped EU it rebuilt only
  derived sparse caches at preserved slot `8,953,695`, reduced contract-node
  SST bytes from 42.02 GB to 246.46 MB, passed computed/stored root verification
  with protected identity/genesis/key/archive evidence unchanged, and then
  created checkpoint `8,954,000`. Normal retention pruned the old hard-linked
  checkpoint and restored 58.72 GB free while preserved-state catch-up continued
  with zero systemd restarts. The signed installed `v0.5.223` binary remains
  unchanged and no `0.5.224` candidate has been installed.
- Before the read-only inspection correction, passed Core 981/981 plus every
  package integration suite, production readiness 102/102, Validator 393/393,
  strict workspace Clippy, formatter, shell syntax, helper guards 12/12, the
  focused snapshot rollback suite 5/5, and all 33 contract WASM builds. That
  locked Bookworm `linux/amd64` validator (SHA-256
  `6b4989cdd74ec01b13f366ea89e3d742466b180dc55795e1c30f1d44be57a2f1`)
  and clean platform image manifest
  `sha256:21c76ad0300c369365fea800bfe0530b5fbe822234a3599e17413058977eb1bb`
  are now superseded and must not be installed. Full gates and a clean exact
  Linux build must be repeated on the read-only inspection source before final
  multi-platform release archive checksums are recorded by the tag workflow.
- Built a corrected Linux/amd64 audit-only validator with SHA-256
  `e82cd6f5b875e47e8e9d8f4542ee2919d94f3e3d81c3e737acb083b900059201`.
  Through disposable RocksDB secondaries it inspected the pristine, read-only
  EU July 12 provider rollback at slot `8,915,275`, returned exact current and
  cached root `cbf7770f...03d3a`, exact four-validator stake-pool digest
  `3ea8c6c5...37747`, and reported `state_root_recompute=read_only`. It was not
  installed and is not the final release artifact; complete release reruns and
  the final no-cache build remain mandatory.
- Passed the earlier clean 10-validator scale/fault gate through slot 2448: all ten
  joined without copied state, V10 and V1 resumed from their own state, all ten
  resumed together, 8-of-10 finality advanced with V9/V10 stopped, both
  recovered with preserved identities, every final RPC/activity tip was fresh,
  canonical certificate parity matched, and all ten offline hot/cold manifests
  matched root `027d802a1c4e6fb2f1682b295e75e75864e8c73cd924d65bb465e9a5d065ef5a`.
- Passed an earlier authoritative final-source four-validator gate through checkpoint slot 3000:
  V2/V3/V4 independent joins, V4 own-state restart, 3-of-4 finality with V1
  offline (22 blocks in 10 seconds), V1 own-state recovery, coordinated restart
  at spread 0, fresh per-validator activity, canonical certificate parity at
  slot 757 with child-certificate hash
  `8f78532332dd188813289056971c5e4c49fe60fba85b3975f1d50a485ee74f7b`,
  volume/user journeys 140/140, launchpad/governance/graduation 104/104,
  checkpoint-1000 manifest root
  `57f0f483988a753b9c6da7afe2a672aba104b64fc4ad40620dc0c2ecaee2a70b`,
  and matching checkpoint-3000 post-journey manifest root
  `5b68f9a28917f10460f3578bde7991c84099d7906034fb75d4137cc29ae3e7a4`.
  The captured transcript SHA-256 is
  `e976d981f254d42382c46f331558d67de4c3a8cbebdc9a23956f55b10b2e9438`.
  The complete gate transcript SHA-256 is
  `6fbbbe90d7ff109b1b81d08f84329067a8b64099bd3ee01870ab0c733d9e2bad`.
- Reproduced the stalled four-validator state at slot 582 with V1 and its
  configured bootstrap RPC offline. V2/V3/V4 reconnected solely through their
  durable peer stores, accepted signed active-validator tip announcements,
  resumed 3-of-4 finality from slot 583, and reached common slot 677 with the
  same archive-contiguous hash.
- Earlier subsystem evidence also passed all 32
  genesis contract tests and WASM builds plus the separate MT20 test/build,
  all-target/all-feature workspace Clippy,
  frontend/RPC/wallet/extension/exchange gates, JS SDK tests and npm audit,
  Cargo audit/deny, Trivy, and `cargo audit -D warnings` across all 39 Cargo
  lockfiles.
- Earlier four-validator archive-cold evidence, now superseded by the final run
  above, covered:
  V2/V3/V4 empty-state joins, V4 own-state restart and catch-up, V1 seed
  stopped while V2/V3/V4 produced 23 blocks in 10 seconds, V1 own-state restart
  at drift 0, all-validator restart followed by 42 blocks in 10 seconds, all
  four validators producing through slot 754, and matching offline hot/cold
  public-history manifest root
  `f285096ce50ce3422d8cd52a130ea1fe387293d2d790dc4569ea6499502707d5`.
- Live release remains blocked by current-chain archive evidence
  `evidence/archive-parity/testnet-20260709T181442Z`: US and IN had local cold
  block bodies with missing slot cursors for later subranges and those cursors
  were repaired. The US July 9 provider copy proves and preserves slot
  `5,275,999`, but no audited current VPS source yet proves
  `2,872,006..4,298,999`. The EU July 12 provider copy decoded 6,510,346
  rollback-hot-plus-cold rows without integrity errors and found zero bodies in
  that range. Its separate `5,275,999` singleton scan also decoded all
  6,510,346 rows without errors and found zero matching bodies; transcript
  SHA-256 is
  `c50c99a0984fdc24e88c6442717d5ac6e655800d3b33f454327c227e4cbffd9e`.
- Live signed `v0.5.223` reproduced the stale-parent producer-effect fault at
  canonical tip `9,000,624`: US missed the producer update for parent slot
  `9,000,623`, while SEA and IN agree. This second occurrence is preserved under
  `evidence/post-block-effects-recovery/testnet-20260713T-live` and is the live
  regression anchor for the candidate's startup and pre-BFT parent gates.

## [0.5.222] - 2026-07-04

### Fixed
- Stops live catch-up and parent-gap recovery from broadcasting overlapping
  block-range requests to every peer. Validators now claim unrequested slot
  ranges centrally, request each claimed range from one scored peer with
  fallback, expire stale request markers, and clear completed or snapshot-jumped
  ranges.
- Records peer-advertised tips from signed validator announcements and status
  responses, then prefers peers that have advertised enough height to serve the
  requested block range. This prevents restarted validators from repeatedly
  asking stale peers for the next missing slot after a same-tip fleet restart.
- Converts recoverable live replay and BFT commit consistency faults into the
  verified checkpoint repair path instead of exiting the validator process.
  Startup/configuration/genesis/snapshot/WAL fatal exits remain fail-closed.

### Verified
- Passed `cargo fmt --check`, `git diff --check`,
  `cargo check --workspace --release --locked`, locked release binary build,
  `cargo test -p lichen-validator --locked`, `cargo test -p lichen-p2p --locked`,
  and `bash tests/local-multi-validator-test.sh 4`.
- The 4-validator drill covered empty-state V2/V3/V4 joins, single-validator
  own-state restarts, seed restart, and same-tip all-validator restart from
  preserved local state; after the all-validator restart the cluster advanced
  42 blocks in 10 seconds and finished with all four validators active.

## [0.5.221] - 2026-07-01

### Fixed
- Removes the sub-slot remote-proposer timeout from the active BFT path so
  validators wait the configured proposal window before nil-voting when the
  designated proposer is delayed by catch-up or archive-range traffic.
- Reduces block-range response chunks from the protocol cap to the
  initial-sync window so validator catch-up cannot monopolize multi-megabyte
  QUIC streams while live proposal/vote messages are in flight.

### Verified
- Passed `cargo fmt --check`, `cargo check --workspace --release`,
  `cargo deny check`, focused validator regression tests, the full
  `cargo test -p lichen-validator --release` suite, release binary build, and
  a local 4-validator stop/restart/rejoin matrix covering a joiner restart,
  seed restart with the other three validators finalizing, and same-tip
  all-validator restart from preserved local state.

## [0.5.220] - 2026-07-01

### Fixed
- Restores the bounded missed-proposer grace used by the stable `v0.5.215`
  timing profile while preserving the full configured propose timeout for the
  selected proposer.
- Adds startup-only stale-height WAL round rendezvous so a restarted validator
  can rejoin an already-stale BFT height without signing skipped intermediate
  rounds or replaying hours of obsolete timeout history.

### Verified
- Passed focused validator timing/restart tests, consensus tests, multi-crate
  checks, Cargo Deny, deployment-env QA, local 3-validator stop/restart/rejoin,
  local 4-validator topology restart, signed release verification, rolling
  testnet deployment, runbook verify-only, public RPC cadence, DEX/oracle smoke,
  and public faucet-backed exchange simulation.

## [0.5.219] - 2026-06-30

### Fixed
- Makes the testnet faucet service sign native LICN funding transfers with its
  configured `FAUCET_KEYPAIR` instead of proxying to validator `requestAirdrop`,
  so validators do not need treasury signing material and public faucet funding
  works after non-genesis validator restarts/upgrades.
- Fails faucet requests closed when the configured faucet keypair is missing or
  does not match the chain treasury account reported by RPC.
- Passes the local cluster keypair password into the local faucet process so
  `scripts/start-local-stack.sh testnet` exercises encrypted treasury keypairs.

### Verified
- Passed `cargo fmt --check`, `cargo check -p lichen-validator -p lichen-cli
  -p lichen-custody -p lichen-faucet`, `cargo deny check`, `cargo audit`,
  `cargo test -p lichen-faucet`, validator unit tests, the 4-validator
  restart/rejoin local gate, and the local faucet-funded exchange simulation.

## [0.5.206] - 2026-06-25

### Fixed
- Pins Cargo network retry/sparse-registry settings inside the Docker build so
  the Docker CI job uses the same crates.io transport hardening as the rest of
  GitHub Actions.
- Supersedes `v0.5.205` before VPS rollout and moves the guarded June 2026
  testnet governed-signer recovery boundary to slot `5,980,000` to preserve
  deployment runway.

### Verified
- Reuses the green `v0.5.205` code/test gate results for recovery, governed
  transfers, local 3-validator clean start, and release artifact generation;
  `v0.5.206` adds the Docker CI transport hardening before deployment.

## [0.5.205] - 2026-06-25

### Added
- Adds a chain-id, treasury-wallet, and slot-guarded June 2026 testnet governed
  signer recovery activation so the live testnet can rotate missing governed
  signer configs without changing balances, history, contract storage, or
  distribution wallet addresses.
- Adds governed key custody verification and mainnet runbook gates requiring
  live signer verification plus private/offline backups before key cleanup.

### Fixed
- Removes the final legacy project-name residue from tracked documentation.

### Verified
- Passed focused recovery guard tests, governed-transfer core tests, validator
  check/clippy, deployment-doc QA, and a clean local 3-validator `start-reset`
  run before release.

## [0.5.204] - 2026-06-25

### Fixed
- Aggregates homogeneous batched shielded unshield instructions in RPC
  transaction summaries, so wallet/explorer Activity and privacy list views
  report the full transaction amount instead of only the first note.
- Restores the web wallet shield confirmation flow after the batched-unshield
  UX change, and keeps shield MAX from selecting more than the spendable amount
  after network and ZK compute fees.
- Aligns web wallet and extension staking/shield flows with MAX controls and
  inline password retry errors that clear the bad password field without
  closing the action modal.

### Verified
- Passed RPC library tests, focused batched-unshield coverage, JavaScript syntax
  checks, wallet QA, extension QA, and diff hygiene before release.

## [0.5.203] - 2026-06-24

### Fixed
- Allows matured MossStake unstake claims to pay the base transaction fee from
  the claim proceeds when the account has no spendable LICN, avoiding a
  zero-spendable claim deadlock while still charging the normal network fee.
- Aligns RPC `sendTransaction` preflight with the same matured-claim fee rule,
  so wallet simulation and node admission agree.
- Updates wallet and extension MossStake claim buttons to stay enabled for
  matured claims when the fee will be deducted from claimed LICN.
- Fixes shielded MAX/full-balance unshield for exact sums across multiple notes
  by requesting the required per-note compute budget and splitting oversized
  note batches under the protocol compute cap in both wallet and extension.

### Verified
- Passed focused MossStake claim and RPC preflight regression tests, JavaScript
  syntax checks, SDK checks, plus wallet and extension QA.

## [0.5.202] - 2026-06-24

### Fixed
- Adds a guarded public-history index-only merge for source-backed account
  activity repairs where block bodies or slot cursors conflict but transaction,
  account, and slot transaction indexes are conflict-free.
- Keeps the broad public-history merge conflict checks intact, so operators do
  not replace block bodies, balances, contract storage, validator state, or tip
  cursors to restore wallet/explorer Activity.

### Verified
- Passed focused public-history merge tests, tx-index account rebuild tests,
  validator check, workspace check, SDK checks, and deployment-doc QA.

## [0.5.201] - 2026-06-23

### Fixed
- Supersedes the `v0.5.200` canary by restoring the restarted-validator
  pre-consensus entry tolerance to the stable five-slot window used by
  `v0.5.199`. A restarted validator that is more than the voting-ready window
  behind must remain in sync catch-up instead of entering live BFT and consuming
  future votes without advancing its local tip.
- Keeps the archive-backed public-history merge improvements from `v0.5.200`,
  including read-only source cold-store attachment for restoring real
  block/transaction/account-history rows from backed data.

### Verified
- Passed focused pre-consensus catch-up coverage and guarded source-cold
  public-history merge coverage before the broader release gate.

## [0.5.200] - 2026-06-23

### Fixed
- Restores public-history repair from archive backups whose historical
  block/transaction/account indexes have already migrated into cold storage.
  The guarded merge path can now attach a source cold store read-only and still
  refuses conflicting historical rows.
- Opens cold stores read-only for account-history inspection and dry-run
  account transaction rebuilds, so diagnostics can run against live or mounted
  archive sources without taking the writer lock.
- Widens the restarted-validator pre-consensus entry tolerance while keeping the
  live BFT stale-vote guard unchanged. Near-tip validators can re-enter the BFT
  loop instead of chasing an advancing head forever, but stale validators still
  yield before voting or proposing.

### Verified
- Passed focused read-only cold-store attach coverage, guarded source-cold
  public-history merge coverage, pre-consensus catch-up tolerance coverage, and
  live BFT stale-tip guard coverage.

## [0.5.199] - 2026-06-23

### Fixed
- Allows restarted near-tip validators to leave the pre-consensus sync gate once
  they are within the explicit voting-ready tolerance of the moving live tip,
  instead of chasing exact tip equality forever and syncing one block at a time
  without re-entering proposer rotation.
- Resets successful LiveSync catch-up batches against the active LiveSync
  cooldown instead of the initial-sync cooldown. A restarted near-tip validator
  can now immediately request the next small catch-up gap after a successful
  live batch, so it does not remain a vote-only follower while proposer turns
  move past it.

### Verified
- Passed focused LiveSync follow-up batch regression coverage, the full
  validator sync test module, live BFT catch-up guard tests, and BFT timeout
  validation tests.

## [0.5.197] - 2026-06-23

### Fixed
- Tunes the default 400ms-slot BFT view-change timers to avoid multi-second
  stalls when an active staked proposer is offline. New genesis defaults use
  800ms propose, 500ms prevote, 500ms precommit, and a 5s max phase timeout.
- Documents the required consensus timing check in the mainnet launch runbook so
  future networks do not keep the old multi-second timeout profile.

### Verified
- Passed focused consensus timeout validation tests, local clean 3-validator
  startup from the deployment runbook, deterministic E2E smoke, and a local
  one-validator-down fault sample that held roughly 400ms slots.

## [0.5.196] - 2026-06-22

### Fixed
- Keeps resumed validators in live BFT proposer rotation by letting observed
  peer tips use the configured `LIVE_BFT_CATCH_UP_GAP` before yielding, instead
  of hard-pausing the BFT loop on a fixed two-slot observation gap. This prevents
  an active restarted validator from remaining a vote-only follower while its
  proposer turns fall into slower consensus rounds.

### Verified
- Passed focused live BFT catch-up guard and mature-validator resume tests.

## [0.5.189] - 2026-06-22

### Fixed
- Preserves wallet and explorer account activity through clean validator
  rejoin, state-repair snapshots, and resumed sync by carrying backed public
  transaction-history indexes alongside canonical state.
- Rebuilds account transaction counters from existing backed rows before
  applying new block deltas, so `getAccountTxCount` and
  `getTransactionsByAddress` remain consistent immediately after snapshot
  import or history repair.
- Keeps full/fresh snapshot imports from merging stale target history while
  allowing repair snapshots to merge verified public history indexes.
- Adds guarded dry-run/write account-history rebuild tooling from retained
  block archives or `tx_by_slot` transaction indexes, with source inspection
  for proving what a node can and cannot reconstruct.

### Verified
- Passed full core tests, full validator tests, validator clippy with warnings
  denied, and focused account-history snapshot/rebuild regressions.
- Passed a clean local 3-validator runbook test where V2/V3 were wiped to empty
  chain state, rejoined from V1 without copied RocksDB state, and preserved a
  pre-rejoin account transaction on all three RPCs with matching block roots.

## [0.5.188] - 2026-06-21

### Fixed
- Restores fair BFT proposer rotation for four-validator and larger validator
  sets by deriving the leader-selection slot from `height + round` instead of
  `height * 1000 + round`. The old mapping collapsed the effective weighted
  round-robin window for four validators, allowing a validator to remain online
  and voting while not being selected to propose.

### Verified
- Passed focused BFT leader-slot regression tests and weighted leader-selection
  fairness tests.
- Passed the validator consensus test suite.
- Passed a clean local 4-validator run: V2/V3/V4 joined without copied state,
  V4 restarted from its own state, and all four validators produced blocks.

## [0.5.187] - 2026-06-21

### Fixed
- Preserves wallet/explorer Activity rows during snapshot export by merging hot
  and cold `account_txs` indexes instead of exporting only the hot RocksDB view.
- Refuses destructive `account_txs` rebuilds unless every existing activity row
  can be proven from retained canonical block bodies, preventing pruned or
  checkpoint-joined validators from wiping address history.
- Clears stale `atxc:` counters when replacing the account-activity snapshot
  category so imported rows and `getAccountTxCount` cannot diverge.
- Exports canonical blocks through the hot/cold block reader so checkpoint
  snapshots do not silently omit canonical blocks that moved out of hot storage.

### Verified
- Passed focused `account_txs` snapshot/rebuild regressions.
- Passed `cargo test -p lobstercove-lichen-core --lib` and core clippy with
  warnings denied.
- Passed a clean local 3-validator run: fresh V2/V3 joins, V3 own-state restart
  with zero drift, and all three validators producing before local cleanup.

## [0.5.186] - 2026-06-21

### Fixed
- Stops validator startup from rebuilding the wallet activity `account_txs`
  index from locally retained block archives, preserving stored address history
  on checkpoint-joined or pruned validators.
- Restores checkpoint snapshot export of `account_txs` from the stored account
  transaction index while still filtering provably stale rows when the
  canonical block is locally available.
- Keeps fresh and resumed validators from erasing wallet/explorer Activity rows
  that cannot be reconstructed from pruned block archives.

### Verified
- Passed focused core account activity, canonical snapshot, validator snapshot,
  and RPC `getTransactionsByAddress`/`getAccountTxCount` tests.
- Passed `cargo fmt --check`, `git diff --check`, and clippy for core,
  validator, and RPC with warnings denied.
- Passed a clean local 3-validator `start-reset` smoke: all three validators
  healthy, online, and advancing at the same slot before local cleanup.

## [0.5.183] - 2026-06-20

### Fixed
- Makes account activity queries merge hot and cold `account_txs` indexes so
  older wallet and extension history remains visible after archive migration,
  validator restart, or canonical activity-index rebuild.
- Makes `getAccountTxCount` cold-storage aware and duplicate-safe when a node
  has both live and archived account activity rows.
- Keeps the legacy `get_account_tx_signatures` and paginated activity APIs on
  the same merged read path so RPC callers cannot disagree.
- Adds regression coverage for account activity migrated to cold storage,
  hot-index clearing, and rebuild recovery.

### Verified
- Passed focused core account-index regressions and RPC
  `getTransactionsByAddress` coverage locally before release gating.

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
- Removed the obsolete `getTransactionHistory` alias; use `getTransactionsByAddress`.
- `getAllSymbolRegistry` is the only symbol-registry list method.
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
- Legacy egg-info artifacts removed from source tree
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
