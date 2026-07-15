# Changelog

All notable changes to the Lichen blockchain project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.224] - 2026-07-15

### Known Testnet Limitation
- The existing `lichen-testnet-1` source set irrecoverably lacks signed block
  bodies `2,872,006..4,298,999`. This release does not hide or synthesize that
  legacy interval. The owner accepted it only to upgrade the testnet that found
  the archive-design defect. Mainnet startup remains fail-closed unless its
  durable archive proof covers and independently verifies every linked signed
  block body and required transaction index from genesis through canonical tip.

### Fixed
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
- Adds `scripts/verify-testnet-archive-parity.sh` for fleet-level archive
  evidence across US, EU, SEA, and IN, including strict stopped-validator
  manifest comparison for the release gate.
- Adds page-level public-history export/import admin commands plus
  `scripts/stream-public-history-repair.sh`, so live repair can stream verified
  history from EU/source into targets without copying another validator DB.
- Adds binary framed public-history page streams for large block-body repairs,
  avoiding JSON/base64 page overhead while preserving source-backed additive
  imports and same-key conflict aborts.
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
- Completed the final July 15 locked workspace all-target/all-feature suite and
  strict workspace Clippy with `-D warnings`. The final four-validator
  archive-cold gate passed restart/resume, one-validator outage, proposal/vote,
  checkpoint parity, 140/140 volume journeys, and 104/104
  launchpad/governance/graduation journeys; transcript SHA-256
  `f73e134b...7fce9b`.
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
