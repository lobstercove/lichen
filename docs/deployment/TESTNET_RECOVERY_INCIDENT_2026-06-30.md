# Testnet Recovery Incident - 2026-06-30

## Scope

This incident covers the June 30, 2026 public testnet liveness failure observed
after validator release/recovery work. Mainnet was not live and was not part of
the incident scope.

## Root Cause

The failing live node had an incomplete live snapshot rollback marker. Startup
rollback recovery was unsafe for archive-backed validators whose old public
history had moved from hot RocksDB into the local cold archive:

- rollback recovery tried to reason about checkpointed state without the cold
  archive attached;
- full archive categories could require hot block bodies even when those bodies
  correctly lived in cold storage;
- a later startup partial-genesis recovery check could treat missing hot slot-0
  block evidence as fresh-node evidence and scrub local hot state.

Hot/cold storage is only a storage optimization. It must never change the
validator lifecycle contract: a stopped validator must restart from its own
state, archive, keypair, and node identity unless an operator explicitly runs
the owner-approved clean-slate path.

## Fix

The recovery patch in `validator/src/main.rs` enforces these invariants:

- destructive partial-genesis recovery returns `Result<bool, String>` and only
  runs from positive local evidence; database read errors no longer default to
  "slot 0, scrub";
- public-network nodes with a non-zero tip or stored genesis block clear stale
  bootstrap markers instead of wiping state;
- cold storage is attached before startup classifies local chain state;
- live snapshot rollback restores state-root-bearing categories plus the
  canonical `slots` cursor/index, but does not require `blocks` or
  `transactions` to be present in the hot checkpoint;
- archive/history reconciliation remains a separate guarded operator path.

The storage patch in `core/src/state.rs` adds a fresh-target regression proving
that a source with old canonical history in cold storage and recent canonical
history in hot storage can export/import both into a clean target with the same
state root, account state, canonical snapshot digests, tip, block bodies,
transactions, and account history.

The deployment runbook now has a mandatory local deployment drill and an
explicit restart/rejoin invariant requiring validators to preserve state,
archive, keys, node identity, known-peer evidence, secrets, and release
evidence across stops, restarts, and release rollouts.

## Validation

Local validation passed:

- `cargo fmt --check`
- `cargo test -p lobstercove-lichen-core --lib -- --nocapture`
  - `942 passed; 0 failed`
- `cargo test -p lichen-validator --bin lichen-validator -- --nocapture`
  - `335 passed; 0 failed`
- `cargo test -p lichen-validator snapshot -- --nocapture`
  - `44 passed; 0 failed`
- `cargo test -p lichen-validator sync -- --nocapture`
  - `66 passed; 0 failed`
- `cargo test -p lichen-validator pre_consensus -- --nocapture`
  - `2 passed; 0 failed`
- `cargo test -p lichen-validator bft -- --nocapture`
  - `13 passed; 0 failed`
- `cargo test -p lobstercove-lichen-core cold_ --lib -- --nocapture`
  - `15 passed; 0 failed`
- `cargo test -p lobstercove-lichen-core archive_ --lib -- --nocapture`
  - `14 passed; 0 failed`
- `cargo test -p lobstercove-lichen-core fresh_snapshot_import_restores_history_from_source_hot_and_cold --lib -- --nocapture`
  - `1 passed; 0 failed`; verifies matching state root plus matching
    `accounts`, `blocks`, `transactions`, `tx_by_slot`, `tx_to_slot`,
    `account_txs`, and `slots` snapshot digests after fresh import from a
    mixed hot/cold source.
- `node scripts/qa/test_deployment_env_examples.js`
- `node scripts/qa/test_rolling_release_custody_sequence.js`
- `bash tests/local-multi-validator-test.sh 3`
  - V1, V2, and V3 all produced blocks;
  - V2 and V3 joined without copied RocksDB, WAL, or genesis-wallet artifacts;
  - V3 restarted from its own local state, preserved its validator keypair, did
    not fresh-join, did not reimport genesis, caught up with drift `3`, and the
    chain continued producing;
  - final local slot `474`, validators `3`.
- `bash tests/local-multi-validator-test.sh 4`
  - V1, V2, V3, and V4 all produced blocks;
  - V2, V3, and V4 joined without copied RocksDB, WAL, or genesis-wallet
    artifacts;
  - V4 restarted from its own local state, preserved its validator keypair, did
    not fresh-join, did not reimport genesis, caught up with drift `0`, and the
    chain continued producing;
  - final local slot `654`, validators `4`.

## Artifacts

Emergency Linux validator artifact:

- path: `evidence/exchange-readiness/live-20260630T0009Z/emergency-validator-linux-20260630T0104Z/lichen-validator`
- hash: `cc9f8bac542b8346de6e8424fd79ae97c0f51b3b9409a6841c9cb6f07153cdb2`
- file type: ELF 64-bit LSB pie executable, x86-64, GNU/Linux
- amd64 Debian smoke: `lichen-validator 0.5.215`

Local macOS `v0.5.216` release-candidate harness binary:

- path: `target/release/lichen-validator`
- hash: `dcbf8a4737fac3087986aea7f5d26d394de53677dddca13ccb76cd17cb24e184`
- version: `lichen-validator 0.5.216`

## Current Live State Before Signed Recovery Release

Deployment SSH on TCP `2222` is reachable on all four public testnet hosts.
Raw public TCP `8899` is not exposed externally; public RPC is served through
`https://testnet-rpc.lichen.network`.

All four validator services are active but stale at slot `6715444`, and all four
currently run the same non-published validator binary hash:

- `/usr/local/bin/lichen-validator` hash:
  `f151f34529c4de147edebf3166871fdb3a6829a730884f3169a0a4ab6a707eeb`
- local health: `status=behind`, `reason=stale_tip`
- public health: `status=behind`, `reason=stale_tip`

The published `v0.5.215` release verifier rejects this installed hash. The live
recovery path is therefore a new signed GitHub release, installed through
`scripts/rolling-release-deploy.sh`, so every validator ends on the same
release-archive hash with state, cold archive, keypair, node identity, known-peer
evidence, service secrets, and release evidence preserved. Do not reset or copy
validator state as a substitute for the verified recovery release.
