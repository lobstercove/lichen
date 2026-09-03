# v0.5.270 Testnet Deployment Incident

Date: 2026-09-02  
Scope: `lichen-testnet-1` preserved-chain release and Archive V2 activation  
Status: deployment stopped safely; `v0.5.271` later failed its release gate,
signed `v0.5.272` is installed, and the current recovery successor is
`v0.5.274`

## Executive record

Signed `v0.5.270` passed protected CI and its complete hosted release matrix,
including the four-validator hot/cold Archive V2, CLOB, AMM, prediction,
governance, launchpad, outage, own-state restart, and public-history parity
gates. Its coordinated production deployment nevertheless found two
production-shaped integration defects before any validator was restarted:

1. The deploy helper passed `--data-dir` to a maintenance command whose parser
   accepts `--db-path`, so it attempted to open the service user's default path
   and failed with permission denied.
2. After correcting that argument manually for a read-only dry-run, the real
   preserved contract account could not decode because its historical ABI used
   the field `name`; the current `ContractAbi` decoder accepted only the
   canonical serialized field `contract`.

The deployment failed closed. All four validators remained stopped. No DEX
repair write, validator restart, chain reset, RocksDB copy, state deletion,
legacy archive deletion, R2 deletion, key change, identity change, or WAL
replacement occurred.

## Shutdown delay observed

The Singapore validator's old process temporarily remained in uninterruptible
kernel wait while a read-only legacy R2 FUSE request completed. The original
deployer used a blocking `systemctl stop`, which prevented its bounded kill
fallback from being reached. The request completed naturally, the old process
exited, its ports became free, and the same read-only mount, source, unit, and
configuration remained intact. No FUSE abort or unmount was performed.

## Preserved evidence and rollback

- Staged signed release: `v0.5.270` at
  `ddf68df162926ac44c59b70204724d2e444c3620`.
- Signed Linux x86 validator SHA-256:
  `9fb460130e3f9e1274d0adf38bee29c1d54b48dc29934aff399bcbdf8832fece`.
- Signed Archive V2 utility SHA-256:
  `d3f7adfe72dbf9e495d73b693d769c0c13b6405554cb3690fecb2dffaf52f71a`.
- Signed contract bundle digest:
  `f4015500a36b456b876c30b3e3fb8ca216e08f2e2bc5c5a129e3d94a93f406d3`.
- Canonical stopped-chain catalog SHA-256:
  `81b83ea65345f8c029307900de7025e92e5fbbfe4f94ea3a175c98a05df7bcd5`.
- Canonical public-history root:
  `acdc89eadd45fb6f3592c72cc8fb2bac9d76de211c1d5278072487354b59dbb2`.
- Catalog coverage: 381 segments covering slots `0..12,221,149`, with the
  existing non-transferable Testnet waiver for signed block bodies
  `2,872,006..4,298,999`.
- The signed `v0.5.265` rollback anchor remains hard-linked on every host under
  `/var/lib/lichen/releases/v0.5.265/rollback-anchor`.
- Operator-side terminal evidence remains outside the release commit under
  `memories/repo/evidence/v240-terminal/v05270-rolling-release-deploy.log.next`.

## Root cause and escaped-test analysis

The release gates exercised fresh-state contract accounts, which serialize the
canonical `contract` ABI key. They did not include a fixture copied from the
historical Testnet serialization shape. The deployment QA checked the guarded
repair sequence and confirmation but did not assert the maintenance parser's
actual state-path flag. The service-stop test checked stop-before-install but
did not model a process left in the unit control group after systemd changed
the unit state.

These are release-boundary coverage failures, not Archive V2 data corruption
and not evidence that a fresh network would create the legacy ABI shape. They
do affect any preserved network containing historical `name` ABIs and could
affect normal runtime reads of those accounts. That is why `v0.5.270` must not
be started on the preserved Testnet even though its signed artifacts are valid.

## v0.5.271 prevention controls

- `ContractAbi` accepts `name` as a decode-only alias and continues to emit
  only `contract`; a core test proves both directions.
- The DEX repair test stores an account with the production legacy ABI shape,
  runs the repair, and proves owner and storage preservation.
- The deployer passes `--db-path`, and release QA rejects a return of the wrong
  `--data-dir` argument.
- Coordinated service stop is nonblocking, checks the full systemd cgroup,
  reads `cgroup.procs` membership directly rather than trusting pseudo-file
  size, performs a bounded control-group kill, and fails closed if any process
  remains.
- Independent release gates start concurrently for recovery speed, but
  checksum generation remains blocked on quality/security, Archive V2 parity,
  contract, compiler, and every platform build.
- Deployment remains all-four stop/install/repair/start from immutable signed
  artifacts. It requires per-database repair idempotence and cross-validator
  contract/storage evidence before restart.

## v0.5.272 production findings

Signed `v0.5.272` repaired all 17 preserved DEX contracts idempotently and
restored a three-validator quorum from each host's own state. EU remains
fail-closed at slot `12,263,310` on a real parent post-state-root mismatch and
requires an authenticated checkpoint agreed by two independent sources; no
reset or cross-validator RocksDB copy is authorized.

At slot `12,272,000`, production checkpointing exposed two assumptions absent
from the 30,000-slot release fixture. Full-archive packaging cannot include the
legacy cold symlink placement, and the pre-activation hot-repair estimator
charged twice for `113,117,166,227` bytes of inherited public-history SSTs even
though the raw checkpoint hard-links those files. Its computed
`226,234,332,454`-byte write peak can never pass on the current 200 GB roots.
Immutable `v0.5.273` corrected those two paths, but its first strict Archive V2
tag gate failed when fresh verified-cache V3 exited immediately after verified
checkpoint activation. It was neither signed nor deployed. `v0.5.274` retains
the production-scale accounting correction, keeps imported state non-live
until its durable rollback transaction is cleaned, and makes a recurrence
report the real child exit status, sidecars, and disk state.

## Completion criteria

The incident is closed only after signed `v0.5.274` is installed and running on
all four validators from each host's own preserved state; block production,
finality, RPC, WebSocket, DEX/AMM/CLOB, prediction, governance, and launchpad
smokes pass; all four Archive V2 manifests prove the same genesis-to-tip
logical history; and signed range-bound retirement reclaims legacy disk without
removing the rollback anchor or deleting R2 objects before mirrored parity is
recorded.
