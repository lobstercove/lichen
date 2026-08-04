# Archive V2 Release Readiness — 2026-07-27

**2026-07-27 decision:** **LOCAL QUALIFICATION PASS; NO-GO for tag or live
deployment.**
The complete uncommitted local candidate passed the exact four-validator
qualification, final workspace-wide Rust gates, supply-chain gates, and the
quiet-host selected-codec latency rerun. The live testnet independently lacks
the writable staging/runtime capacity required for this migration. No clean
candidate commit, CI proof for that commit, release tag, tag-workflow artifact,
checksum set, detached PQ signature, or new rollback anchor exists.

This report records the final local result. It is not release or deployment
authorization.

## 2026-08-04 release-preparation update

Release preparation resumed on an isolated branch at runtime version
`0.5.230`. The candidate now includes the retroactive fixed-range builder and
dual reader described below, dependency advisory updates, an exact-version
lockfile set, and release/deployment packaging for the `lichen-archive-v2`
operator binary. The short tag-workflow topology has explicit Archive V2
catalog headroom values compatible with its accelerated 20-slot legacy cold
boundary; the default local qualification still crosses the production-like
50,000-slot boundary.

The following clean rerun gates passed on 2026-08-04 before the candidate
commit:

- workspace formatting, all-target/all-feature Clippy, and all-feature tests;
- compiler, contract SDK, Rust client SDK, and fuzz standalone workspaces;
- all 39 Cargo lockfile audits and the root cargo-deny policy;
- npm and Python dependency audits, release-policy static QA, and all 33
  contract test/build suites.

This update authorizes an intentional clean candidate commit and CI. It does
not bypass exact-commit CI, the tag workflow, artifact attestations, detached
PQ signing, or live capacity gates. The August 4 read-only fleet audit found
only about 5--6 GiB writable root headroom per 200 GB host and no approved
staging filesystem. Retroactive conversion and any legacy retirement remain
prohibited until additional writable capacity is physically present, the
converted genesis-to-tip range has exact parity, independent replicas and an
authenticated restore are proven, and the signed dual-reader rollback anchor
is installed.

## 1. Candidate scope

The working tree implements both release boundaries:

- durable, chain-bound legacy cold-migration cursors; bounded row/byte/time
  passes; validator-specific scheduling jitter; cold `WriteBatch` durability;
  write-before-delete recovery; bounded physical reclaim; capacity accounting;
  and health/readiness telemetry;
- versioned content-addressed Archive V2 segments, deterministic manifests and
  catalogs, seekable Zstandard frames, canonical transaction deduplication,
  deterministic public indexes, fixed-range resumable builders, verified
  readers and cache, corrupt-object quarantine, authenticated local/HTTPS
  replication, bounded retirement journals, explicit validator roles, adaptive
  capacity policy, checkpoint/catalog joins, CLI operations, RPC/P2P
  integration, and exact local-network coverage.

Archive V2 remains an internal storage representation. The candidate does not
change consensus or wire encoding, canonical block/transaction identities,
signatures, state roots, commit certificates, or public RPC domain objects.
It remains dual-reader and does not authorize live legacy deletion.

## 2. Regression found and prevention

A late local fresh full-archive join exposed one real checkpoint-boundary bug.
Snapshot import had correctly restored the checkpoint block and its transaction
indexes, but activation then replaced that complete block with the independently
authenticated header-only anchor. Later segment construction correctly failed
its payload commitment at that slot.

The activation path now authenticates the imported complete checkpoint block
against the independently verified anchor header and certificate, validates
its transaction, fee, and oracle payload commitments, fails closed on any
mismatch, and persists the complete body. The focused regression test
`checkpoint_snapshot_activation_preserves_authenticated_block_body`, all five
snapshot archive-completeness tests, and the public-history block-body
normalization test passed before the clean qualification rerun.

Failed diagnostic manifests and logs are retained under:

`evidence/archive-v2/local-20260727-checkpoint-boundary-failure/`

## 3. Codec benchmark

The required 60-candidate matrix covers Zstandard levels 3, 6, 9, 12, and 15;
1, 4, and 16 MiB frames; and no, repeated-public-key, trained-64-KiB, and
trained-128-KiB dictionaries. It was executed on old, recent, busy, sparse,
and oversized PQ-signed ranges. Every successful candidate produced
byte-identical repeated output and exact reconstruction.

The conservative version-1 default is Zstandard level 6, 4 MiB target frames,
64 MiB maximum oversized frame, and no dictionary. The benchmark does not make
a production storage-saving promise from small local ranges; live
representative migration remains prohibited until staging capacity exists.

| Range | Blocks / unique tx | Source bytes | Segment bytes | Ratio | Build | Block p95 | Tx p95 | Sequential decode |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| old PQ | 1,000 / 1,064 | 38,914,146 | 10,410,667 | 3.738x | 940 ms | 161.449 ms | 123.427 ms | 44.98 MiB/s |
| quiet-host selected old PQ rerun | 1,000 / 1,065 | 38,925,203 | 10,415,005 | 3.737x | 194 ms | 34.130 ms | 25.089 ms | 223.63 MiB/s |
| recent PQ | 1,000 / 1,077 | 39,057,887 | 10,456,753 | 3.735x | 195 ms | 32.155 ms | 24.973 ms | 225.75 MiB/s |
| busy PQ | 128 / 139 | 5,012,493 | 1,348,222 | 3.718x | 120 ms | 30.080 ms | 24.198 ms | 46.41 MiB/s |
| sparse PQ | 128 / 131 | 4,924,037 | 1,320,988 | 3.728x | 26 ms | 6.646 ms | 5.224 ms | 204.17 MiB/s |
| oversized genesis PQ | 128 / 147 | 4,650,397 | 1,747,380 | 2.661x | 27 ms | 7.325 ms | 6.139 ms | 184.79 MiB/s |

The original old-range result was captured while the exact network gate was
contending for the host and remains preserved. The quiet-host rerun selected
the same level-6, 4 MiB, no-dictionary codec, produced deterministic bytes and
exact reconstruction twice, and brought block p95 to 34.130 ms and transaction
p95 to 25.089 ms. This closes the local 100 ms/150 ms latency target; it does
not substitute for a live representative capacity measurement. Benchmark
evidence is under:

`evidence/archive-v2/local-20260724/`

`evidence/archive-v2/local-20260727-final/`

## 4. Qualification status

Passed:

- focused Archive V2 codec/catalog/builder/reader/replication/retirement,
  cursor, capacity, role, checkpoint, RPC, and P2P tests;
- exact `tests/local-multi-validator-test.sh 4`, including the 50,000-slot
  hot-retention crossing, independently owned validator state, fresh
  full/verified-cache/consensus joins, one-validator outage, own-state restart,
  all-validator restart, authenticated source outage, corruption quarantine
  and repair/refetch, and strict public-history parity;
- immutable slot-90,000 public-history manifest root
  `9b8fc018545d5b171f937f8572bb34ef4e1c288965ef8f983624afc9840241d5`
  on all four validators;
- independently built/mirrored/restored Archive V2 range `0..49,995` catalog
  root
  `0b1ffacd708c2b0312d2d621dd2980e0c45c1cc09cb3d503f67f2c3c7b7e8f9b`
  on all four validators;
- final local liveness through slot `91,295`, including continued finality
  through archive corruption and authenticated-source outage drills;
- `cargo fmt --all -- --check`;
- `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`;
- `cargo test --workspace --all-features --locked`;
- final `cargo audit -D warnings` and `cargo deny check`;
- quiet-host selected-codec determinism, exact reconstruction, and local
  reader latency targets;
- all 33 standalone genesis WASM contract builds and tests;
- Rust, Python, TypeScript, and cross-SDK tests;
- frontend assets and RPC parity QA;
- deployment documentation QA;
- GitHub Actions supply-chain, legacy-codec, Rust dependency, RustSec policy,
  archive-parity-assets, local-helper-guard, and expected-contract static QA.

Still pending outside local candidate qualification:

- CI on an intentional clean commit;
- exact tag workflow, release artifacts, attestations, `SHA256SUMS`, and
  detached PQ checksum signature;
- adequate live writable/staging capacity and a separately approved,
  coordinated deployment.

No waiver, ignored test, warning suppression, reduced retention boundary, or
copied mutable validator state was introduced.

The final local evidence bundle is
`evidence/archive-v2/local-20260727-final/`. Its exact resumed-tail transcript
SHA-256 is
`0b97d17cd42eed3a837195ae92da787af5248e8ad0c1e23f8cd4c607a3b98b82`.
The local slots in that transcript are generated by the disposable local
four-validator chain and are unrelated to the live VPS testnet slot.

After evidence preservation, the disposable local states, Archive V2 replicas,
harness temporary tree, empty staging directories, and Rust debug incremental
cache were deleted. Free space increased from 39 GiB to 61 GiB, all enumerated
targets were confirmed absent, and no local validator process or test-port
listener remained.

On 2026-07-29, a second reproducible-output cleanup ran `cargo clean` for the
root workspace and Rust SDK. Cargo removed 24.9 GiB and 5.1 GiB respectively;
macOS reported 86 GiB available afterward. Candidate source and retained
qualification, parity, repair, recovery, signed-release, and rollback evidence
were not removed.

## 5. Live testnet state and capacity

The fleet recovered on the same signed `v0.5.229` artifact at
`2026-07-27T19:49Z`. A bounded reclaim removed the almost-unused, reproducible
1 GiB swapfile on each host after backing up and commenting its exact fstab
entry. Package caches and journals were bounded. No state, WAL, hot/cold
history, key, identity, rollback artifact, or provider backup was modified.

US and EU restarted from their independently preserved states. EU caught up
from slot `10,088,410`, after which two short coordinated fixed-tip pauses
aligned all four nodes and returned every identity to proposer rotation. The
final public sample advanced `10,101,521..10,101,558`; the latest 100 blocks
contained all four authors. Fixed slot `10,101,160` matched everywhere with
block hash
`4b869970ba11a01d03ae0d294b6a26737f83ac8d72cf7108ad29d1569cebb117`
and state root
`53a7205812dd0535e65af652adb44b131256ca52f3401b669360af4b66cb65ec`.
All four installed/running hashes match the signed release, services are
active/enabled with zero restarts, strict edge health is green at zero lag,
and Explorer/canonical RPC and WebSocket acceptance passed.

| VPS | Service | Final sampled tip | Writable root free |
| --- | --- | ---: | ---: |
| US | active, zero restarts | 10,101,461 | 7,095,869,440 bytes |
| EU | active, zero restarts | 10,101,459 | 6,437,752,832 bytes |
| SEA | active, zero restarts | 10,101,454 | 7,430,459,392 bytes |
| IN | active, zero restarts | 10,101,460 | 8,423,038,976 bytes |

This is emergency operating runway, not durable capacity. Swap is intentionally
disabled, and the separate `sdb` filesystems remain read-only provider backups,
not migration scratch or replacement storage.

The current 200 GB roots have no unallocated capacity. Before any live
Archive V2 build, checkpoint, dual-build, compaction, or release rollout, the
owner must provide enlarged/new writable storage while preserving the backup
and restore posture. Exact staging needs must then be calculated from the live
largest segment, checkpoint, verification copy, compaction peak, retry budget,
and filesystem reserve; local benchmark ratios are not a capacity approval.

Read-only evidence:

- `evidence/live-health/testnet-20260727T073700Z/`
- `evidence/live-health/testnet-20260727T163045Z-final-readonly/`
- `evidence/archive-parity/testnet-20260727T073159Z/`

Recovery and acceptance evidence:

- `evidence/live-recovery/testnet-20260727T193009Z-headroom-reclaim/`
- `evidence/live-recovery/testnet-20260727T193628Z-eu-catchup-watch.log`
- `evidence/live-recovery/testnet-20260727T194404Z-four-validator-rejoin/`
- `evidence/live-recovery/testnet-20260727T195037Z-final-acceptance/`

## 6. Provenance and rollback

The uncommitted candidate is currently based on local `main` HEAD
`8fe66a9d7ec14966063213d379cc9ff1d5e989db`; HEAD has no exact tag. The working
tree is intentionally dirty while implementation and qualification evidence
are being completed, so there is no candidate commit hash or releasable source
state yet.

The current live signed release remains `v0.5.229` at commit
`feb0a97bcc9e0cb8055e8e8c2abd5f78a8f41d80`. Its installed/running validator
SHA-256 is
`56ca8642d52b78f8ff166c733254a9b9a1da2d354c7d85261f77e12f3a03ab60`.
The immediate signed in-place rollback remains `v0.5.228` at
`da501f084a63cb7eb764eaf03dec02c7d48b0f8d`; the deeper anchor remains
`v0.5.223` at `fa4a7d3d`.

Those old binaries cannot be used after required legacy rows are retired.
Before the first such deletion, a signed dual-reader release must be deployed,
fully proven, and explicitly designated as the new rollback anchor. Until
then, rollback means disabling candidate V2 reads while legacy hot/cold state
remains authoritative. Keys, identities, consensus WALs, state, cold archives,
provider backups, access configuration, incident evidence, and signed rollback
artifacts must remain preserved.

## 7. Explicit release decision

Do not tag, publish, deploy the local candidate, or retire legacy rows from
this report's current state. The live signed fleet is recovered; local
qualification is complete.
Reconsider the release only after CI is green on an intentional clean commit,
a signed tag-workflow artifact exists, and the rollback anchor is explicit.
Reconsider live deployment only after adequate writable/staging capacity is
physically present and a coordinated exact-boundary deployment has separate
owner approval.
