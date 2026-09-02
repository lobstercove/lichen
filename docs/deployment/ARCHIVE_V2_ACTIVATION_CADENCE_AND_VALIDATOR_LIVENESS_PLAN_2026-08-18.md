# Archive V2 Activation, Cadence Recovery, And Validator Liveness Plan

**Date:** 2026-08-18
**Last updated:** 2026-09-01
**Status:** Authoritative execution plan. Signed `v0.5.265` is installed on all
four validators from its release-workflow artifact. A read-only 2026-08-30
sample found the same installed validator SHA-256 on every host,
`7ea2159569cd1cf1ae2ce3eca9d483a2c337580fd34862c3b378cd0d0b3f3afc`.
US and Singapore are active but stale at slot `12,271,149`; EU is active and
quarantined behind at slot `12,263,310`; India is failed with no RPC. The
network is halted below quorum. Root-space availability is 21,401,096,192 bytes
US, 5,633,310,720 EU, 8,808,546,304 Singapore, and 5,346,226,176 India.
Keys, signers, environments, WAL, state, archives, and rollback evidence remain
preserved.

Immutable tag `v0.5.266` failed closed in the release quality job because its
clean runner audited wallet code before building the JavaScript SDK distribution
modules; it published no installable release assets and must not be retagged.
Immutable tag `v0.5.267` corrected that ordering and passed its protected-main
CI and release quality gate, but published no installable release assets. Its
first Archive V2 attempt exposed bounded RPC starvation after the intentional
V4 corrupt-segment restart. The evidence-based failed-job retry passed the
complete Archive V2, corruption, repair, source-outage, checkpoint, and fresh
role-join matrix, then failed the strict volume journey because the genesis
margin mark price had correctly expired after 750 slots. `v0.5.267` must not be
retagged or rerun blindly.

Immutable `v0.5.268` preserved the exact reviewed runtime and
release-runner build correction, and refreshed the controlled
LICN/USD margin test price through signed native attestations from the active
validator quorum immediately before the strict margin journey. This advances
the canonical consensus-oracle source slot and deterministic margin mirror; it
does not weaken the runtime stale-price rejection. Its tag workflow passed the
quality, security, compiler, genesis, manifest-parity, deterministic Archive V2
root, stopped-admission, restart, and production checks reached before the
runtime role matrix. Both bounded attempts then failed closed because serial
`getSlot("finalized")` probes were intermittently unavailable under admission
load even while processed slots and block production advanced. No platform
artifacts or deployment were produced, and the immutable tag must not be
rewritten or rerun again.

`v0.5.269` is the only successor candidate. It carries the complete v0.5.268
runtime and journey correction, publishes the same authoritative finalized
frontier through the always-available health response, and probes all four
health frontiers concurrently with bounded retries. The strict finalized
spread and processed-to-finalized lag requirements remain unchanged.
The inherited runtime adds a bounded, catalog-bound
hot-repair checkpoint profile for Archive V2, reconciles a common catalog before
checkpoint selection, normalizes node-local block commit evidence during
history export, excludes the local cold-migration cursor from state snapshot
identity, and provides signed validator-admin inspection for checkpoint
manifests. The clean local four-validator gate passed the real 50,000-slot cold
retention boundary, fresh full/cache/consensus joins, one-validator outage,
own-state restart, coordinated restart, and strict logical public-history
manifest parity at terminal slot 30,000. The candidate is not deployable until
its exact clean commit passes protected and post-merge CI, the immutable tag
workflow, provenance attestations, checksums, detached post-quantum signature,
and exact live preflight.

Archive V2 roles and irreversible retirement remain incomplete on the live
fleet. The complete US source-backed recovery tail is preserved at
`/var/lib/lichen/recovery/v265-archive-tail-20260829`; its 381 segments cover
`[0, 12,221,149]`. Its full catalog and public-history roots are recorded in the
sealed source evidence and must be copied exactly from that evidence into every
command. The US tail must remain until all four validators prove exact V2 parity. R2 remains a
temporary replicated recovery/archive source, not the permanent mainnet
storage design, and deletion of any R2 object remains unauthorized.

The signed `v0.5.265` release is the sole immediate rollback anchor for
`v0.5.269`. Once `v0.5.269` is running and four-way V2 parity plus rollback
rehearsal are recorded, validator hosts keep only those two signed release
installations. Legacy history is retired only by signed, source-backed,
range-bound Archive V2 retirement and compaction; low disk space does not
authorize ad-hoc deletion.

**Scope:** `lichen-testnet-1`, the Archive V2 production topology, current
four-validator cadence, and a future deterministic offline-validator design

This plan records two separate work tracks:

1. Complete the Archive V2 deployment that has been implemented and locally
   qualified but is not active on the live fleet, while isolating and fixing
   the remaining BFT cadence regression.
2. Design and later implement deterministic quarantine and re-admission for
   unavailable validators without allowing local peer observations to change
   consensus membership.

The first track is current operational work. The second is a consensus-design
project for a later coordinated release. They must not be combined into an
ad-hoc production change.

## 1. Executive Decision

The fleet state and rollback artifacts remain preserved on signed `v0.5.265`,
but the testnet is halted below quorum and is not mainnet-ready. Archive V2
roles and legacy retirement remain open, current 200 GB root volumes are not
approved for indefinite archive growth, and `v0.5.269` still requires local and
hosted signed-artifact, coordinated-deployment, and live acceptance gates.

### 1.1 Current decision

- Preserve the completed cache-only archive-status correction and its proof
  that neither `getHealth` nor `getMetrics` performs RocksDB/FUSE metadata I/O.
- Preserve the completed clean four-validator runtime evidence and create a
  new immutable release only if every protected quality, security, contract,
  wallet, exchange, outage, rejoin, and Archive V2 gate independently passes.
- Publish only tag-workflow artifacts with provenance, checksums, and the
  detached PQ signature. Coordinated-stop/install/start all four validators;
  do not use a mixed-version rolling restart and do not install a local build.
- Restore four-way own-state convergence first. Repair EU and India only from
  exact source-backed history and their preserved identities/WALs; do not reset
  genesis or copy another validator's RocksDB state.
- Extend the dual-R2 catalog only from the stopped, immutable, fully audited US
  source-backed tail. Do not perform another genesis rebuild or an unbounded R2
  readback.
- Bootstrap and retire legacy cold/FUSE data one validator at a time, retaining
  the other three-vote quorum and proving exact rejoin after each host. Activate
  all four validators as `verified_cache` with the same 2 GiB hard quota only
  after every capacity decision passes without weakening reserves.
- Repeat the strict 1,000-commit cadence gate after legacy/FUSE retirement, then
  publish the matching wallet, exchange, developer-portal, and frontend release
  surfaces only after the stable live evidence is attached.
- Preserve both R2 buckets. Their later cleanup requires a separate exact-key,
  content-hashed deletion manifest and explicit approval; it is outside the
  current authorized execution.

### 1.2 Recovery chronology retained for audit

- Signed v0.5.260 is installed and running on all four hosts from the exact
  release-workflow artifact. Every validator preserves its own key, signer,
  WAL, and state. After India was OOM-killed and restarted, all four RPCs again
  tracked the same advancing chain, but only US, EU, and SEA remained current
  BFT voters. India's `6Xhs...` identity remained at last active slot
  12,043,989 while the other identities advanced beyond 12,048,000. Signed
  v0.5.258 remains the coordinated rollback baseline until v0.5.262 passes
  moving-network rejoin acceptance.
- The initial current-commit Archive V2 receipt fallback was real and
  v0.5.257 removed it from event fanout, but it was not the complete explanation
  for the production halt and multi-second cadence.
- The decisive live recovery evidence showed 97-185 pending transactions while
  the built-in oracle feeder used 5-second/15-second/1-bps defaults. Git history
  dates those aggressive defaults to the May 2026 cadence work, before Archive
  V2; they are a latent recovery-load amplifier, not an Archive V2 format
  change. The earlier 30-second/60-second/10-bps policy is restored here.
  A recovered proposer blindly selected as many as 2,000 pending transactions;
  proposal construction and validation then consumed 4-15 seconds and could
  exhaust the proposal round.
- Disabling only the feeder through a temporary, identical systemd drop-in on
  all four hosts immediately restored empty-block production. With US stopped,
  round-zero intervals were approximately 0.28-0.45 seconds and each slot for
  which US was proposer incurred the expected approximately 2.5-second
  round-one timeout. This separates the backlog defect from the offline-proposer
  behavior and from Archive V2 reads.
- v0.5.258 restores the established oracle defaults and bounds every live BFT
  proposal to at most 16 pending user transactions, 17 total entries including
  the mandatory parent commit certificate, and 2.8 million aggregate declared
  compute units. The same limits reject an oversized received proposal before
  signature verification or execution. Its
  release gate explicitly creates a 96-transaction backlog while quorum is
  paused and requires bounded drain, complete finalization, convergence, and
  continued production after quorum returns.
- The v0.5.258 live outage drill proved that US, EU, and SEA continue finality
  while IN is offline, but also exposed a returning-validator admission defect:
  a drained one-block sync batch can leave the sync-manager guard active while
  its pending queue is empty. On a continuously advancing 300 ms chain, that
  stale guard can strand an exact-tip validator outside BFT until the network
  itself stalls. v0.5.259 distinguishes that drained guard from real sync work
  while retaining exact tip parity and a zero-pending-block requirement.
- The v0.5.259 US outage canary then proved that exact-tip parity itself is not
  a viable ten-second admission invariant on the production 300-ms moving
  chain. US continuously applied authenticated blocks and kept an empty pending
  queue, but the observed tip stayed one to three slots ahead; US entered BFT
  only after a bounded frozen-tip recovery. No snapshot, catalog, or R2 object
  was changed. v0.5.260 applies the already-defined one-slot passive tracking
  bound while preserving exact parity for fresh joins and stalled recovery.
- The v0.5.260 SEA capacity canary then exposed a distinct architectural
  defect. After SEA reclaimed 16,102,768,640 local bytes and caught the moving
  tip, the deterministic 4,096-block post-effects readiness scan still lived in
  the moving admission and BFT lifecycle. During rejoin, that local historical
  scan could consume the one-slot admission window before the passive proof
  began, forcing the node back into catch-up on a continuously moving chain.
  The same helper also performed a redundant stake-pool reload as each new tip
  was verified. v0.5.261 removes historical scans from the live path. Startup
  alone performs bounded crash recovery and certifies a durable
  `(slot, block hash)` frontier; the marker and frontier advance atomically
  after complete post-block effects, and BFT/rejoin check that frontier in
  constant time under the canonical lock. The existing one-slot bound,
  zero-pending requirement, ten-second duration, and three-slot advance
  requirement remain unchanged.
- The 2026-08-25 capacity incident provided a second live separation of causes.
  India stopped fail-closed at the 5 GiB floor, then EU independently stopped
  137 MB below that floor. With only two of four validators admitted, the chain
  correctly could not finalize. India reclaimed 5,373,990,339 bytes through an
  80-file archive-only activation of the already dual-readback R2 batch; no hot
  state and no new R2 object operation were involved. EU reclaimed
  4,023,726,080 bytes by removing only two unmounted and unopened v0.5.253/
  v0.5.255 candidate caches after all 24 content-addressed objects were proven
  present in the sealed full R2 inventory in both buckets. A later exact audit
  removed 12 files from the stale, disabled EU
  `archive-v2-consensus-testnet` candidate cache only after their names, sizes,
  and payload identities matched both sealed R2 inventories and no runtime or
  filesystem reference existed. That second cleanup reclaimed 1,422,680,064
  physical bytes while preserving its catalog and retirement receipts; both R2
  copies remain untouched. EU and India then entered BFT from their preserved
  WALs. Existing future-round evidence made
  them skip to the witnessed round, and all four committed slot 12,018,618
  with hash prefix `b1bda05d` before normal round-zero production resumed.
  This was a disk-capacity/no-quorum recovery, not an Archive V2 consensus
  format failure and not an authorization to weaken the disk guard.
- A later 2026-08-25 live audit tied the renewed cadence regression to both
  rejoin state and the emergency FUSE bridge. The kernel recorded a global OOM
  kill of India's validator at approximately 2,572,208 KiB anonymous RSS.
  Systemd restarted the same signed binary and the node continued applying
  authenticated blocks, but it never re-entered BFT. Peer state and prevote/
  precommit timeout evidence showed `6Xhs...` missing while the other three
  validators were current; repeated assigned leader slots therefore waited for
  the proposal timeout. At the same time `/proc/meminfo` reported shared memory
  of 13,111,268 KiB on US, 2,444,216 KiB on EU, 14,394,652 KiB on SEA, and
  20,165,704 KiB on India, with 51, 17, 17, and 13 rclone/FUSE mounts
  respectively. US and SEA had exhausted swap; India had no swap and only
  about 1.5 GiB available. This is not an R2 object-read latency hypothesis:
  the temporary bridge itself is consuming unsafe host memory and must be
  retired after Archive V2 admission. Until then, the coordinated release uses
  a measured RocksDB cache budget rather than the current 1,024 MiB setting.
- The first recovered v0.5.261 local gate crossed the 50,000-slot retention
  boundary and passed immutable Archive V2 source, full-archive, verified-cache,
  consensus-role, source-outage, and restored-own-state V3 acceptance before a
  fourth concurrent join exhausted the 16 GiB development host. The reboot
  purged the temporary worktree and chain data, but the exact 30-file candidate
  was recovered from the local session record into durable storage and passed
  all 463 validator tests, full-workspace tests, strict clippy, cargo-audit,
  cargo-deny, standalone compiler/SDK/fuzz checks, all 33 contract suites and
  WASM builds, release/signer/archive/static QA, and diff checks again. Release
  platform binaries now stage that exact tested WASM bundle before compilation,
  closing the prior tested-versus-shipped genesis-byte ordering gap. The gate
  assigns each validator a validated 64 MiB cache, an explicit 256 MiB aggregate
  block-cache budget, before the clean four-validator rerun.
- The clean memory-bounded rerun completed the former crash boundary, all four
  fresh joins and role joins, the 96-transaction backlog recovery, moving-gap
  rejoin, own-state restarts, all-validator restart, public-history parity, and
  deterministic Archive V2 build/mirror/restore parity. It then failed the
  final runtime role matrix after correctly quarantining a deliberately
  truncated full-archive segment: V4 remained voting and tip-aligned, but its
  genesis RPC did not use the still-present canonical legacy cold fallback.
  The root cause is a role-policy contradiction in slot routing, not consensus
  or object-store latency. The admitted fresh-sync shortcut forced every role
  directly to V2, while the separate legacy policy explicitly permits only
  `full_archive` to retain hot -> cold -> V2 fallback until retirement. The
  correction makes exclusive catalog routing apply only to `verified_cache`
  and `consensus`; a focused regression and all nine Archive V2 state tests
  pass. The guarded checkpoint-78,000 tail resume subsequently passed with
  exit code 0: the deliberately corrupted full-archive object was quarantined,
  canonical cold history remained available before retirement, replica repair
  restored the object, and the chain produced 78 blocks in 10 seconds during
  the fault. Section 2.9 records the complete evidence.
- The tagged v0.5.261 release workflow then reproduced the exact four-validator
  scenario from clean hosted state. Quality/security, standalone workspaces,
  dependency policy, genesis bundle, and compiler sandbox passed. During the
  replica-backed V4 repair, V4 entered BFT and continued committing from slot
  15,942 through at least 24,572, but the shell gate waited only for the
  sustained-moving-tip log and ignored the exact-tip stalled-quorum recovery
  outcome implemented by the same guarded admission state machine. The release
  correctly failed before platform builds or draft artifacts. v0.5.262 emits
  one structured guarded-readiness event after the canonical frontier check and
  before BFT for either safe outcome; the gate still requires BFT entry and
  post-admission tip-aligned advancement, so no queue, drift, duration, state,
  or progress safety condition is weakened.
- The Explorer's `Observed ... ms avg` value is not an arithmetic average. It
  is the upper median of at most 120 observer-side normalized block-arrival
  samples. It can therefore move from roughly 300 ms to roughly 1,000 ms when
  a sustained timeout burst occupies more than half of the rolling window.
- The Archive V2 binary and role implementation are present. All four runtime
  roles are temporarily disabled because the former 317-segment catalog is
  stale relative to the current tip; signed admission correctly fails closed
  rather than claiming incomplete genesis-to-tip-minus-headroom coverage. The
  intended bounded testnet matrix is four equal-policy `verified_cache`
  validators. A mainnet launch additionally requires approved persistent
  `full_archive` capacity in at least three independent failure domains.
- The existing read-only FUSE SST mounts are an emergency, dual-R2-backed
  legacy archive offload. They are not the final full-archive,
  verified-cache, and consensus role topology.
- Current disk free space is runway, not completion. The latest 2026-08-25
  read-only audit measured approximately 26.0 GB free on US, 9.34 GB on EU,
  19.0 GB on SEA, and 9.31 GB on India. All four services tracked the advancing
  chain, but India was not a current BFT voter; service health must not be
  reported as four-validator consensus health. India retained zero hot-state
  symlinks.
  Legacy archive bytes are reclaimed permanently only after fleet-wide Archive
  V2 admission and fixed-tip parity, not before.
- The first stable live window after backlog drainage contained 31 commits over
  30 intervals in 10,639 ms, an arithmetic average of 354.6 ms. Individual
  round-zero intervals returned to roughly 288-442 ms. Explorer windows that
  include the preceding no-quorum interval are expected to remain temporarily
  inflated and are not release acceptance evidence.

Accordingly:

1. Do not call the recovery 100% complete while recurring BFT timeout bursts
   remain unexplained or while Archive V2 role activation is disabled.
2. Use only the bounded, content-hashed emergency headroom pass required to
   recover a validator that reached the fail-closed disk floor. Do not delete
   R2 objects or treat that temporary bridge as Archive V2 activation.
3. Preserve the completed `v0.5.266` runtime evidence and publish `v0.5.269`
   through the
   signed release workflow, verify provenance plus its detached post-quantum
   checksum signature, and deploy it through one coordinated four-host
   stop/install/start. Keep signed `v0.5.265` as the only immediate rollback.
4. Restore four-way own-state convergence, extend the canonical Archive V2 tail
   from the stopped immutable US recovery source, activate the exact role
   matrix, prove fixed-tip parity and cadence, and only then retire legacy
   rows/FUSE bridges through signed exact-range receipts and reclaim disk.
5. Generate a new exact R2 deletion manifest only after all live references are
   gone; require explicit approval of that manifest before deleting proven
   obsolete temporary objects.
6. Treat offline-validator quarantine as a separate versioned consensus change.

## 2. Authoritative Current Evidence

### 2.1 Historical pre-freeze fleet integrity baseline

The earlier pre-freeze safety baseline was:

- four active/running validators;
- identical installed and running v0.5.250 validator and Archive V2 hashes;
- exact v0.5.238 validator and Archive V2 rollbacks and exact v0.5.240 Archive
  V2 rollback on all four hosts;
- regular, non-empty validator keys, signer keys, and consensus WALs;
- zero broken links, zero failed FUSE units, zero active recovery watchdogs,
  and zero Archive V2/reclaim jobs;
- active FUSE unit/mount counts of US `43/43`, EU `9/9`, SEA `12/12`, and IN
  `10/10`.

The recovery evidence and exact artifact hashes are recorded in
`memories/repo/current-state.md` and the terminal addendum in
`memories/repo/2026-08-13-v05247-retirement-recovery-handover.md`.

### 2.2 Historical and current cadence evidence

The most recent synchronized ten-minute sample showed:

| Host | Median | p95 | p99 | Maximum |
| --- | ---: | ---: | ---: | ---: |
| US | 331 ms | 996 ms | 3.524 s | 7.793 s |
| EU | 341 ms | 953 ms | 3.457 s | 8.144 s |
| SEA | 322 ms | 1.161 s | 3.474 s | 7.706 s |
| IN | 324 ms | 1.120 s | 3.291 s | 5.183 s |

The same period contained cross-host bursts of round-one and round-two
commits. Phase-level reconstruction proved two different tail classes:

1. Some IN and SEA proposals spent approximately 700-1,400 ms in block build
   or speculative execution before votes began.
2. Other blocks were built quickly, including one in approximately 30 ms, but
   still crossed proposal, prevote, or precommit deadlines and escalated to a
   later round.

Therefore the observed slowdown is real. Four services being online proves
neither round-zero proposal delivery nor timely quorum participation.

The later recovery run supplied the missing causal evidence. A 97-transaction
proposal took multiple seconds to execute and validate. After the oracle feeder
was disabled, the same binaries resumed 300-400 ms-class round-zero empty-block
production. When only three validators were participating, the remaining
approximately 2.5-second spikes mapped to slots assigned to the catching-up US
validator and disappeared from other proposer slots. The release correction
therefore has two independent obligations: bound transaction work per proposal,
and restore US before measuring steady-state four-validator cadence.

### 2.3 What the source and storage diff proves

The v0.5.229-to-v0.5.250 comparison found no change in:

- Explorer cadence rendering;
- the cadence sampling algorithm;
- consensus timeout helpers;
- proposer selection;
- deployed `400/2000/1000/1000/60000 ms` timing configuration.

The current block proposal path uses the hot transaction index and does not
perform deep Archive V2 reads. The 2026-08-25 fleet placement audit found zero
symlinked SSTs in `state-testnet` on every host; the emergency R2/FUSE links are
confined to `archive-testnet` (US `1,896`, EU `1,523`, SEA `1,679`, India
`1,436` before the recovery and `1,516` after the verified 80-file archive-only
activation). Active Archive V2 segment migration is also disabled. This rules out
R2 as a direct dependency of the current hot-state BFT readiness reads. A
proposed India headroom plan that selected 154 hot-state SSTs was therefore
aborted before local replacement, and all further emergency headroom plans are
explicitly archive-only.

The post-effect safety gate has a separate source-level regression history:

- v0.5.224 introduced a process-local `verified_tip` cursor and a bounded
  4,096-block recovery scan before BFT;
- v0.5.234 constrained that scan to hot-retained blocks, correctly preventing
  consensus recovery from falling through to cold history;
- v0.5.257-v0.5.260 reused the scan inside returning-validator moving-tip
  admission, where a complete local scan could outlive the one-slot drift
  allowance and repeatedly send an otherwise caught-up validator back to sync;
- each newly verified tip also reloaded the full stake pool even when no repair
  occurred.

v0.5.261 preserves the safety purpose but replaces the process-local cursor in
live BFT with a persistent hash-bound frontier. Startup performs the historical
crash-recovery scan once and certifies the frontier. Each successful block then
commits its comprehensive effects marker and frontier in one RocksDB batch.
Moving admission and BFT check only the exact canonical tip, its completion
marker, and the frontier. A fork replacement, missing marker, malformed
frontier, or failed block store fails closed.

This gives three independently testable cadence classes rather than one vague
"Archive V2 slowdown": proposal-work tails fixed in v0.5.258; moving-rejoin
admission fixed in v0.5.261; and deterministic timeout slots while a validator
is genuinely offline, which remain expected until the later versioned
offline-validator design is implemented.

### 2.4 Background faults already identified

Two separate background conditions must still be removed or contained:

- Checkpoint creation fails on hosts that have hot-state SST symlinks into
  `/dev/shm`, because RocksDB's hardlink-based checkpoint operation returns
  `Operation not permitted`. The 2026-08-23 inventory found seven such links on
  US totaling 3,673,754,324 bytes, three on SEA totaling roughly 273 MB, four on
  IN totaling roughly 343 MB, and none on EU. During the coordinated v0.5.258
  stop, each exact link target must be copied to a regular same-filesystem file,
  size/hash verified, and atomically substituted before restart.
- EU and SEA currently skip new checkpoints because free space is below the
  20 GiB checkpoint safety floor. This is resolved by qualified Archive V2
  legacy retirement and reclamation, not by weakening the reserve.
- Legacy cold maintenance wakes approximately every five minutes and is
  terminally unable to progress because the reclaim queue is at or near its
  4,096-range limit.
- The live US `verified_cache` canary exposed an additional slot-fallback bug
  after catch-up. A recent `getBlock` request whose replay body was absent but
  whose slot index was present fell through to Archive V2 block-by-hash lookup.
  Because the catalog has no block-hash-to-segment filter, that path repeatedly
  read and decoded segments newest-to-oldest. `strace -yy` proved repeated
  237,888,524-byte reads of cached object
  `2ed2c026206a545dc5529aa11df658f8c86e8747d57cb2c53f539ade43c7f9a5.av2s`,
  one runtime thread at approximately 100% CPU, another blocked in
  `fuse_file_read_iter`, and an accumulating RPC accept queue. v0.5.258 keeps
  known-slot fallback on hot-by-slot, legacy-cold-by-known-hash, and finally
  Archive-V2-by-slot; a known slot outside catalog coverage therefore returns
  without a segment scan.

Sixty minutes of correlation falsified the five-minute cold-maintenance wake as
the recurring BFT burst trigger: it also runs during quiet windows. Checkpoint
failures remain an operational defect and possible load amplifier, but the
current halt trigger is resolved by the measured oracle backlog and unbounded
proposal execution described above. The separate US canary read starvation is
also resolved in source and has its own no-object-read regression test; both
fixes must pass the final four-validator release gate.

### 2.5 Cloudflare R2 custody baseline

The two current buckets are separate failure domains inside one Cloudflare
account/provider, not independent providers:

- `lichen-testnet-archive-v2-primary` in ENAM;
- `lichen-testnet-archive-v2-replica` in APAC.

The last complete immutable canonical V2 inventory proved 368 objects and
54,530,842,357 bytes in each bucket: 183 segment/manifest pairs, one catalog,
and one retirement receipt. Those canonical `catalog.av2`, `.av2s`, `.av2m`,
and `.av2r` objects are authoritative and are not temporary delete candidates.
The largest currently sealed segment is 553,817,286 bytes. The bounded
testnet verified-cache configuration therefore uses a 1 GiB per-object fetch
limit and a 2 GiB cache quota; every newly built tail segment must independently
fit that limit before its catalog can be published.

The emergency FUSE namespace is also live production storage, not disposable
backup while its mounts remain in use. The sealed inventory contains 74 live
batches, 9,190 SSTs, and 617,994,670,424 bytes per bucket. The primary bucket
is the active rclone source and the replica is its recovery copy. Deleting
either copy now would break or remove the only recovery source for live
archive links.

Before mainnet or final testnet acceptance, copy and independently verify the
canonical V2 corpus into another provider or an offline recovery system.
Two buckets under one Cloudflare account do not satisfy the independent-provider
requirement.

### 2.6 Exact catalog and source boundary

The fleet does not need a genesis rebuild. All four hosts carry the same valid
terminal catalog root
`cb0fa65a8eda5bcdb7306998bf8bedf2ee6d9eaa9773fa6292c1cf2f1a939112`:

- 183 deterministic segments;
- coverage from slot 0 through 10,248,999;
- the exact testnet-only loss declaration for 2,872,006 through 4,298,999;
- catalog file SHA-256
  `d8d08798d4437beb5a47fd47566efcf91de3ef3481365d2ecc4d5459fbd57dd4`;
- the same catalog hash in both Cloudflare buckets.

At the discovery tip, the missing admission tail was approximately 940,000
slots, or about nineteen 50,000-slot segments. A read-only profile proved the
first missing range, 10,249,000 through 10,298,999, is complete and
parent-contiguous: 50,000 blocks, 93,764 transactions, and 2,133,601,104
encoded source bytes.

The first isolated build attempt failed safely before publishing anything. It
proved that opening the live RocksDB through a second process is not a valid
build snapshot: concurrent primary compaction removed `160376.sst` after the
read-only builder had captured an older superversion. That SST is not lost;
its exact 67,192,599-byte object, SHA-256
`e1039b46e644bd29a400b624888a118a1c9cf178e2210febcd1ba70538c97818`,
was independently read back from both R2 buckets. The failed scratch build and
journal remain preserved and must not be resumed against the live DB.

Seven subsequent bounded snapshot/publish runs extended that same canonical
chain to:

- 317 deterministic segments covering through slot 11,588,999;
- catalog root
  `8eb1e234063af96017a0615817baedaf4162fac0f5e36ff5444236fb9ad7cf36`;
- catalog SHA-256
  `afdead0267ed5543736381d0e614b2a3e76c33b66ad8489fcc9e7e7d416c0dad`;
- independently verified publication to both current R2 buckets.

This catalog was sufficient to admit US with a temporary 200,000-slot hot
bridge, but it is not self-maintaining. At a live tip around 11.781 million it
has only several thousand slots of admission headroom. The next coordinated
release stop must create one immutable, self-contained hot snapshot at a fixed
tip. The network may resume immediately on the signed release while bounded
10,000-slot tail segments are encoded from that stopped snapshot, uploaded and
read back from both R2 copies, and appended to a new catalog through at least
the stopped tip minus 50,000 slots. No live RocksDB iterator is permitted.

v0.5.251 added a bounded `snapshot-hot` operation, but its first production
run exposed that opening the stopped live RocksDB as root can perform recovery
writes and change live-file ownership. The live contents were unchanged and
the exact metadata was repaired before coordinated re-entry; v0.5.251 is not
approved for deployment. v0.5.252 fixes the boundary by copying mutable
RocksDB files and SST-symlink targets into a protected isolated staging source,
hard-linking only immutable regular SSTs, and opening only that staging source.
It publishes the self-contained checkpoint atomically after removing staging
and passing the materialization and capacity gates. Tail segment construction
then reads this stable snapshot plus the terminally paused read-only legacy
cold store while the validator is back online. No live RocksDB iterator or
writable RocksDB open is used for a long-running build. v0.5.252 also stages
and releases the first deterministic segment encoding before independently
re-encoding and hash-comparing it, preventing two complete encoded segments
from occupying memory simultaneously on the bounded 200 GB validator hosts.

v0.5.253 closes the activation-time startup deadlock found by the coordinated
four-validator role trial. A resumed validator now snapshots its local genesis
block before the Archive V2 public reader is attached and reuses that snapshot
for startup mode and deterministic timestamp initialization. Consequently a
verified-cache node starts P2P, RPC, and BFT without a synchronous deep-history
source fetch, while public historical reads remain fail-closed after role
admission. Deployment acceptance must prove zero startup remote fetches before
P2P initialization and successful four-validator BFT entry with the configured
Archive V2 roles.

v0.5.254 closes the checkpoint-retention failure discovered during the live
capacity gate. Periodic full checkpoints are now assembled under a hidden,
same-filesystem staging name and published only after the hot database, cold
database, completion metadata, and directory entries are durable. Startup
removes only recognized incomplete numeric checkpoints and staging names before
opening live state. A failed cold-store hardlink is terminally paused for the
remainder of that validator invocation, so it cannot repeatedly pin new SST
generations. v0.5.255 carries the identical checkpoint fix with refreshed
dependency locks after `arrayref 0.3.9` was yanked. v0.5.256 additionally keeps
verified-cache point reads on the seekable frame path instead of implicitly
materializing a complete multi-GiB decoded segment in the validator process,
and defaults explicit whole-segment caching to one entry. v0.5.257 isolates
live commit notifications from Archive V2 receipt fallback, bounds shared
mempool lock scope, accepts authenticated future-round evidence, and keeps a
returning validator passive until sustained near-tip stability is proven.
The signed v0.5.258 deployment removed the earlier runtime cadence regression,
but activation remains blocked until v0.5.262 passes rejoin acceptance, the
catalog tail reaches the current admission boundary, all four capacity
decisions are `Normal`, and live role acceptance proves that historical reads
do not perturb four-validator cadence.

### 2.7 v0.5.258 deployment and v0.5.259 execution boundary

The clean, from-scratch hard gate completed on 2026-08-23 with INFO-level
admission evidence enabled and no reused mutable validator state:

- four validators joined from their own state and all proposed and voted;
- a 140-slot paused-validator gap, individual own-state restarts, and a
  coordinated all-validator restart resumed finality without copied keys,
  RocksDB, WAL, or genesis-wallet artifacts;
- full-archive, verified-cache, and consensus-only role boundaries passed;
- corrupt full-archive segments and cache objects were quarantined, repaired or
  refetched, and the network sustained 72-81 blocks per 10 seconds throughout
  the live Archive V2 restart/outage matrix;
- authenticated-source outage with an empty cache denied deep history while
  consensus remained live;
- the strict volume journey passed 140/140 checks and the launchpad graduation
  journey passed 104/104 checks, including live WebSocket transaction, trade,
  ticker, slot, and orderbook fanout;
- all four final public-history manifests matched at checkpoint 12,000 with
  root `5da0147be91ba3624868a4338e48b7501eb826196a07eed5853071cd65e3cb89`;
- the pre-journey Archive V2 build/mirror/restore catalog root matched across
  all four validators at
  `e35fa837dcdfb989b1c0839d79c41bc575ecad84ff70b09ecae9a146b9c57da2`.

That qualification produced signed v0.5.257 at commit
`66979e24` and the signed release validator is installed on all four hosts.
The recovery nevertheless exposed the separate proposal-workload defect, so
v0.5.257 is the immediate signed rollback for the v0.5.258 transition rather
than the final Track A anchor.

The v0.5.258 candidate must not reach production until all of these hold:

- the full four-validator local test passes hot/cold Archive V2 mode, fresh
  join, a 96-transaction stopped-quorum backlog, one-validator outage,
  own-state restart, coordinated all-validator restart, and strict final
  public-history manifest parity;
- all workspace tests, clippy, audit, deny, contract/genesis builds, SDK,
  frontend, and deployment static gates pass from a clean candidate commit;
- the tag workflow publishes release binaries, `SHA256SUMS`, and the detached
  post-quantum checksum signature for that exact commit;
- every installed and running binary is sourced from that signed release and
  matches the verified hashes.

The final local v0.5.258 qualification completed from clean genesis on
2026-08-24. All four validators produced and voted; fresh full-archive,
verified-cache, and consensus joins passed; source loss did not stop consensus;
corrupt full-archive and cache objects were quarantined and repaired or
refetched; individual and coordinated own-state restarts resumed from preserved
state; and all four public-history manifests matched at checkpoint 76,000 with
root `d1f415c452e51ffd08e31b6889c846650676a10aac313f865ceefb7b5c935924`.
Independent Archive V2 build, mirror, and restore roots also matched at
`ac4f20ae47de96f36a396114468ff6530a7aef453a838e1739fb2637cce8ee35`.
The role/outage matrix ended at 78-79 blocks per 10 seconds. The stopped-quorum
backlog gate admitted and finalized all 96 transfers, and the active validator
logs proved repeated 15-entry blocks: fourteen default-budget transfers plus
the mandatory parent certificate. The harness now tracks the active replacement
log for each restarted validator and fails if it does not observe a
multi-transaction backlog block, closing the original zero-evidence reporting
gap.

Signed v0.5.258 then passed its tag workflow, detached post-quantum checksum
verification, exact four-host artifact staging, and coordinated deployment.
The temporary oracle-disable hold is removed, all four services run the exact
signed binary with zero restarts, fixed-slot hashes agree, and live cadence is
again in the expected 300-400 ms class.

The strict live outage test proved continued three-validator finality but
failed normal rejoin: IN remained passive with an empty pending queue while a
drained sync batch guard was continuously renewed. Freezing the producing tip
allowed the existing stalled-network fallback to admit IN and restored four-way
finality without deleting or editing WAL. That fallback proves safety and
recovery, but it does not satisfy uninterrupted rejoin acceptance. Therefore
v0.5.259 must pass the same repository gates and a moving-network outage/rejoin
test before it can become the Track A anchor.

The exact R2 audit remains a separate destructive gate. The full inventory is
recorded in `memories/repo/evidence/v240-terminal/v05256-r2-full-inventory.tsv`.
The current deletion-candidate manifest contains 24,794 objects totaling
1,695,934,359,284 bytes and has SHA-256
`698c13f6478459be55f91a9e4f5c34fba97fb1315671df264cca0c490ec03bca`;
the retained canonical set contains 1,272 objects totaling
152,744,629,494 bytes. No object may be deleted until the live fleet no longer
depends on legacy/FUSE data and the complete, unhashed-shortened candidate
manifest SHA is presented for explicit operator approval.

### 2.8 v0.5.259 live canary and v0.5.260 boundary

Signed v0.5.259 is installed and running from the exact release artifact on all
four validators. Post-deployment integrity sealed all seven executable hashes,
the running validator hash, signed payloads, v0.5.258 rollbacks, keys, signer
keys, environments, zero state symlinks, zero restarts, and a common fixed-slot
hash. It also measured 150, 101, 100, and 95 validator-owned descriptors into
the legacy R2/FUSE bridge on US, EU, SEA, and IN respectively. Cadence therefore
remains explicitly unaccepted until that bridge is retired after Archive V2
role admission.

The first US-only immutable-snapshot canary stopped before creating a snapshot
because its exact stopped-WAL guard observed one final live commit. Its recovery
timer restarted US without changing the catalog or R2. The more important live
result was deterministic: US continuously followed the network with an empty
receive queue but could not hold exact observed-tip equality for ten seconds,
so v0.5.259 never admitted it to BFT on the moving chain. A bounded SEA/IN pause
froze the tip, US entered BFT through the existing exact-tip stalled-network
path, SEA and IN restarted together, and all four validators resumed committing
the same chain with zero service restarts. This is recovery evidence, not
acceptance of v0.5.259.

v0.5.260 corrects only returning-validator passive admission:

- an already-staked validator must complete the canonical post-effects
  readiness pass, track the authenticated moving tip for at least ten seconds,
  and advance at least three canonical slots;
- zero queued blocks remains mandatory and drift beyond one slot resets the
  proof;
- a drained sync-manager batch guard is ignored only inside that bounded
  one-slot tracking window;
- fresh joins, post-registration admission, and stalled-network quorum recovery
  retain exact tip parity;
- live acceptance must stop one validator while the other three continue,
  prove automatic BFT re-entry and authorship without freezing them, and then
  repeat the bounded snapshot procedure before Archive V2 tail construction.

### 2.9 v0.5.261 clean-gate and resumed-tail qualification evidence

The 2026-08-25 clean four-validator rerun used the exact release build, a
64 MiB RocksDB cache per validator, the 50,000-slot hot/cold boundary, and
authenticated local HTTPS Archive V2 sources. It established the following
initial evidence before the final role-matrix failure:

- retention passed at slot 52,508; bounded V1/V2 cold migrations completed
  independently and the chain continued producing;
- fresh V3 `full_archive`, `verified_cache`, and `consensus` joins started
  without copied RocksDB, WAL, or genesis-wallet state, verified snapshot
  manifests and state roots, reached the moving tip, preserved identity, and
  enforced their deep-history capabilities;
- authenticated source loss failed deep history closed without stopping
  consensus and refetched after restoration;
- fresh V4 crossed the former 16 GiB host-crash point, registered as the fourth
  staked/routing validator, and produced after activation;
- a 96-transaction paused-finality backlog fully finalized through bounded
  proposals with a maximum observed block transaction count of 15;
- V4 consumed a retained 140-slot live gap in the same process, then V4, V1,
  and all four validators passed own-state restart checks;
- all four public-history manifests matched root
  `db92d3e5193768d55bc44f51399c823b1eb2e718da8cdb81f4fdfe1a10704846`;
- all four stopped-state Archive V2 build/mirror/restore derivations matched
  root `04deefdf7019e248376729c8a1eeaf8a83af20df2336906f2d9ccea350a6b3e4`.

The first runtime role-matrix attempt then truncated one V4 segment by design.
Startup quarantined object
`7d0f66bfc4e219b09bb1bd8d4f259f60d833f111898cff26d6fa47717e6e30c7`,
and V4 stayed voting and zero-drift, but `getBlock(0)` returned Archive V2
unavailable instead of the canonical pre-retirement cold copy. The release
was held before commit, tag, or deployment. The correction is deliberately
role-scoped: admitted
`verified_cache` and `consensus` nodes still force verified V2 capability
semantics, while `full_archive` retains canonical hot/cold fallback until the
authorized retirement phase removes those rows. After retirement, absence of
legacy rows naturally returns the original fail-closed V2 error.

Requalification then completed in the fixed order:

1. the focused full-archive fallback regression, all nine Archive V2 state
   tests, formatting, strict all-target/all-feature clippy, and the complete
   workspace suite passed;
2. the gate rebuilt the exact release binaries and resumed only after proving
   all four independently owned states and checkpoint-78,000 public-history
   root
   `db92d3e5193768d55bc44f51399c823b1eb2e718da8cdb81f4fdfe1a10704846`;
3. the resumed cluster produced 75 blocks in 10 seconds and converged within
   one slot before the stopped-state audit;
4. all four independently rebuilt, mirrored, and restored the safe immutable
   range `0..28,208` at stopped tip `78,203`, yielding deterministic Archive V2
   root
   `4ff3435ba6626a0813cb4f02fc6e186070c4b3586b449f6c7091b13266e14162`;
5. the mixed role matrix (`V1/V4 full_archive`, `V2 verified_cache`,
   `V3 consensus`) converged, and the deliberately truncated V4 segment was
   quarantined while the canonical pre-retirement cold copy served matching
   genesis history; V4 remained zero-drift and the chain produced 78 blocks in
   10 seconds during the corruption;
6. replica-backed repair restored the full-archive object, V4 again admitted
   at zero drift, and the chain produced 77 blocks in 10 seconds;
7. verified-cache corruption was quarantined and refetched from its
   authenticated source. Cached-source-outage and empty-cache-source-outage
   restarts both remained zero-drift while deep uncached history failed closed;
   the chain produced 70 and 56 blocks in 10 seconds in those scenarios;
8. full/cache/consensus capabilities, cache persistence and bounds, source
   outage isolation and recovery, catalog advancement, and final role
   admission all passed. The final cadence check produced 55 blocks in 10
   seconds, and the gate exited 0 after controlled cleanup.

The v0.5.261 candidate was therefore locally qualified and merged, but not
released or deployed. Its tagged hosted gate later blocked on the
admission-mode-specific log assertion described above even though V4 was
actively committing. v0.5.262 retains the complete v0.5.261 state-machine and
Archive V2 corrections and changes only the shared guarded-readiness evidence
and its gate assertion.

The exact hosted-equivalent v0.5.262 four-validator gate completed on
2026-08-26 with exit 0. It repeated the full fresh-join, authenticated-source,
corruption/refetch, source-outage, bounded backlog, live-gap, own-state restart,
all-validator restart, Archive V2 build/mirror/restore, mixed-role, volume, and
launchpad journeys. Both repaired-V4 admission paths emitted the shared guarded
readiness evidence, entered BFT, remained within one slot, and continued
committing; the chain produced 80 and 79 blocks in their respective ten-second
checks. Launchpad completed 104 checks with zero failures, the post-activity V4
restart regained canonical certificate parity, and all validators persisted
slot 11,000. Their final hot+cold public-history manifests matched root
`d35cf2631b99e65decae045251a5ad888b4e5d9472c181daa99652f84dd6a7c5`.
The independent stopped-state Archive V2 build/mirror/restore root was
`5dd51309a239822199de4bd6092b2e647ba1c43a8c89ffe7c317efdac48d514a`.

That v0.5.262 qualification was followed by all-green CI, merge, detached
PQ-signed release artifacts, and one coordinated four-host install. Artifact
parity and the live outage/rejoin gate passed; the strict live cadence gate then
failed and led to the cache-only telemetry correction qualified below.

### 2.10 v0.5.263 cache-only telemetry qualification evidence

The v0.5.263 candidate isolates the final measured request-path regression:
public archive status reads return the cached snapshot, the initial storage
sample runs before networking starts, and later RocksDB metadata refreshes run
only inside the existing bounded cold-maintenance blocking pool. Consensus,
wire formats, proposer selection, and timeout policy are unchanged.

The candidate passed formatting, all-target/all-feature clippy, the full
workspace test suite, dependency audit and policy, standalone compiler/SDK/fuzz
checks, all 33 contract builds and tests, frontend and deployment QA, and the
release build. Its complete clean four-validator gate then exited 0 on
2026-08-26 and proved:

1. empty-state V2, V3, and V4 joins, unique identities, guarded admission, and
   moving-tip catch-up;
2. bounded cold migration and authenticated immutable Archive V2 history while
   finality continued;
3. full-archive, verified-cache, and consensus-only role behavior, including
   deep-history boundaries, source outage isolation, corrupt segment/cache
   quarantine, and authenticated replica repair;
4. complete finalization of a 96-transaction paused-finality backlog with at
   most 15 transactions in a committed block;
5. a 140-slot same-process live-gap recovery, V4 and V1 own-state restarts, a
   coordinated four-validator restart, and a post-user-activity V4 restart;
6. 140/140 strict volume checks and 104/104 launchpad/governance checks;
7. equal canonical certificates after restart and identical post-journey
   hot+cold public-history manifest root
   `f10274262fff36833a766b9556810a134a5f456e862d5e81b4c2404a91895c60`
   across all four validators at checkpoint slot 11,000; and
8. deterministic stopped-state Archive V2 build/mirror/restore root
   `458dc65fafc0254f81ae3e604a747b180b45f143740c29ece4a8e19ca09aed8f`
   across all four validators.

An earlier invocation of the same candidate was rejected as invalid evidence
after transaction receipt waits observed synchronized stale tips. All four
validator logs contained identical 660-, 463-, 992-, and 931-second wall-clock
gaps. macOS power logs independently recorded matching 662-, 465-, 995-, and
934-second sleep intervals. Normal block execution resumed immediately after
each wake. The qualifying rerun therefore started from clean genesis under an
explicit system-sleep inhibitor and completed without stale-tip or receipt
failures; no timeout was raised and no blockchain code was changed to mask the
invalid host-sleep run.

The remaining order is: commit from the clean release worktree, obtain all-
green hosted CI, merge, tag, and verify detached PQ-signed v0.5.263 artifacts;
perform one coordinated four-host install; then execute live artifact, cadence,
outage/rejoin, catalog-tail, capacity-bootstrap, role-activation, FUSE
retirement, and final stability gates A0-A10. No live mutation or R2 deletion
is authorized by local evidence alone.

### 2.11 v0.5.266 bounded-checkpoint qualification complete

The resumed four-validator diagnostic on 2026-08-30 proved the corrected
bounded checkpoint profile through the exact 90,000-slot boundary. All four
validators independently built the same Archive V2 catalog root
`1aa56b034739242077976500b42a780e83d2ba9e2f918a3dcac1a2f42a919e62`
through slot 34,988 and published symlink-free, hot-only, catalog-bound
checkpoints with identical manifest root
`45728bf6de3b26b19161d46620148c1d9617086320eb3cf088227651f4f667cf8`.
Each checkpoint builder used a 128 MiB RocksDB cache, and all four checkpoint
roles, including `consensus`, completed in about three and a half minutes under
the 15-minute watchdog. The independent immutable build, mirror, restore,
corruption, cache, and source-outage matrix also passed.

That diagnostic then exposed a real fresh-join circularity: the empty
`consensus` join correctly had no admitted Archive V2 reader yet, but checkpoint
verification required an already admitted catalog root. The correction keeps
the configured, identity-validated catalog as an in-memory checkpoint-only
trust input. It authorizes only an exact catalog root with complete predecessor
coverage; it does not enable history reads or mark the role admitted. After the
checkpoint restores the bounded hot suffix, runtime activation independently
requires the first hot block to chain from the catalog tip before attaching the
reader or persisting role admission. That activation runs before the fresh
node's public-genesis readiness gate because admitted Archive V2 is the source
of slot 0 once the bounded checkpoint has intentionally removed it. Focused tests cover exact acceptance,
wrong-root and incomplete-coverage rejection, missing catalog-tip hot data, and
parent-hash divergence. The local gate trap also restores an interrupted fresh
role's original state and cold archive before deleting temporary join data. It
now treats the sibling live-snapshot rollback marker and rollback checkpoint as
part of that swap boundary: candidate state and transaction sidecars are
discarded before the original path is restored, so an interrupted candidate
cannot rewind the original validator on its next start. The common publication
range is also bounded by the slowest stopped validator and reserves finalized
catalog overlap for checkpoint transfer, replay, and deliberate restart. Fresh
role preparation fails unless the checkpoint preserves the exact 50,000-slot
production hot suffix and its predecessor is covered by that catalog.

After those corrections, the mandatory clean from-genesis four-validator gate
passed with volume and launchpad E2E enabled. It rebuilt all 34 contracts,
proved independently owned validators, fresh full/cache/consensus joins,
authenticated source outage and corruption recovery, 96 queued transactions,
one-validator outage, own-state and coordinated restarts, 140/140 volume
checks, 104/104 launchpad/governance/graduation checks, and exact terminal slot
30,000 on all four validators. Their partition-independent logical Archive V2
manifests matched at
`46481e3d6d7417b8c094564a421f474f174bdf3a9024136110f94baab916294c`;
the common catalog through slot 11,031 was
`781366349c03f24495a8d061e03c5665aeafeea20c4fd86513b324b17630ab5d`;
and the common slot-20,000 checkpoint root was
`010c2bc0fab681b288d5dc49cff84e6d35ed1b76283d5ffd63da1db431462e74`.
Workspace and standalone Rust gates are also green. This completes local
qualification; protected CI, immutable release artifacts, signature,
coordinated deployment, and live acceptance remain mandatory.

## 3. Target Archive V2 Architecture

Archive role and consensus membership are independent concerns. The preferred
production architecture separates them physically and operationally.

### 3.1 Consensus plane

Voting validators retain only:

- independently writable local consensus state;
- consensus WAL, identity, and validator signer material;
- the recent replay/blockhash window;
- recent canonical blocks required for ordinary sync;
- bounded local caches that cannot consume the state/WAL reserve.

Consensus validators must not advertise deep-history readiness and must not
depend on remote archive availability to propose, validate, vote, restart, or
recover within the supported recent window.

### 3.2 Archive plane

The durable target is:

- at least three independent full-archive replicas;
- at least two providers/failure domains and three regions;
- dedicated large writable local or block storage;
- two verified immutable object-store domains;
- one independent offline or provider-native recovery copy;
- exact segment, manifest, and catalog root parity;
- no voting or public-RPC dependency on a single archive origin.

Dedicated full-archive hosts should use two 960 GB or larger NVMe devices in
RAID1 as the current minimum planned class. RAID1 is availability, not backup.
Capacity must be recalculated from measured growth before purchase or mainnet
approval.

### 3.3 Verified-cache plane

Verified-cache origins retain:

- consensus/recent data required by their role;
- the Archive V2 catalog and trusted source roots;
- a hard, separately measured cache quota;
- typed `archive_fetching`, source-unavailable, and verification-failure
  responses;
- two-source verification policy for immutable fetches.

Cache eviction or an upstream archive outage must never stop consensus or
consume the consensus-state reserve.

### 3.4 Edge routing

The RPC edge must route by advertised, authenticated role and readiness:

- current-state and recent-history requests may use ordinary consensus/RPC
  origins;
- deep historical requests use only full-archive or healthy verified-cache
  origins;
- a consensus-only validator is never treated as an archive origin;
- loss of one archive origin is tested without changing consensus membership.

For the future ten-validator network, the preferred arrangement is ten
consensus-only voting processes plus a separate non-voting archive/RPC plane.
If archive and voting identities must initially share hosts, archive data must
still use a distinct filesystem, quota, cgroup/resource budget, and readiness
surface.

### 3.5 Bounded testnet transition on the existing VPS fleet

The final three-local-full-replica architecture cannot fit on the present four
200 GB system disks. That does not justify leaving Archive V2 disabled. The
approved bounded testnet transition is therefore:

| Host | Archive V2 role | Deep-history source | Local cache |
| --- | --- | --- | --- |
| US | `verified_cache` | authenticated primary and replica R2 HTTPS gateways | 2 GiB hard quota |
| EU | `verified_cache` | authenticated primary and replica R2 HTTPS gateways | 2 GiB hard quota |
| SEA | `verified_cache` | authenticated primary and replica R2 HTTPS gateways | 2 GiB hard quota |
| IN | `verified_cache` | authenticated primary and replica R2 HTTPS gateways | 2 GiB hard quota |

All four hosts receive the same immutable catalog, role, 100,000-slot hot
retention policy, source policy, object-size bound, and 2 GiB cache quota. They
therefore agree on the same Archive V2 identity, catalog root, covered ranges,
loss declaration, and admission fingerprint, and all four can serve verified
deep-history reads. Cache contents may differ only through deterministic quota
enforcement and request-driven eviction; that physical placement is not a
logical-history difference. Byte-for-byte duplication of the 54.5 GB corpus on
every 200 GB validator would defeat the storage design.

This is a testnet exception, not the final production archive policy:

- the two R2 buckets are one-provider failure domains and do not count as two
  independent providers;
- no VPS may claim `full_archive` without every segment on approved persistent
  local archive storage;
- R2 loss may make historical RPC unavailable, but the implementation and
  acceptance tests must prove it cannot affect proposal, validation, voting,
  hot-state persistence, or BFT admission;
- source access must use bounded authenticated HTTPS object reads on all four
  validators, not mounted
  R2 filesystems or remote SSTs; the emergency rclone/FUSE bridge is retired
  after role activation and is not part of the final Archive V2 topology;
- a scheduled, signed tail-publisher must keep the catalog ahead of every
  restart admission boundary; it publishes objects and manifests to both R2
  domains and publishes the catalog last only after dual read-back. Each
  catalog replacement is bound to the stable preflight ETag with `If-Match`,
  so a concurrent publisher fails the operation instead of being overwritten;
- `scripts/archive-v2-r2-dual-publish.sh` requires separate scoped temporary
  credentials for the primary and replica domains:
  `R2_PRIMARY_ACCESS_KEY_ID`, `R2_PRIMARY_SECRET_ACCESS_KEY`,
  `R2_PRIMARY_SESSION_TOKEN`, `R2_REPLICA_ACCESS_KEY_ID`,
  `R2_REPLICA_SECRET_ACCESS_KEY`, and `R2_REPLICA_SESSION_TOKEN`. Never reuse
  one credential set across both domains or fall back to ambient AWS
  credentials;
- every host must pass `role-preflight` with capacity action `Normal`; the
  current low-space EU/SEA/IN state must first be repaired through signed
  Archive V2 retirement or larger storage, not another emergency offload;
- the transition is reversible through the preserved baseline environment and
  signed `v0.5.265` immediate rollback artifact. Older releases remain Git and
  audit history, not installed rollback binaries after `v0.5.269` acceptance.

The later dedicated archive plane in Section 3.2 replaces this exception. It
does not block honest Archive V2 role activation on the current testnet once
the catalog, capacity, source, and rollback gates are proven.

### 3.5.1 Low-space role-marker bootstrap boundary

The 2026-08-27 stopped-node proof found a circular dependency in v0.5.263:

1. a bounded hot store may no longer contain slot 0;
2. runtime role activation can recover the genesis MossStake replay mode from
   a checksummed `role-config-v1.bin` marker;
3. startup nevertheless deferred Archive V2 whenever hot slot 0 was absent,
   without consulting that marker;
4. the marker was created only by successful runtime activation; and
5. low-space nodes could not reach runtime activation because the adaptive
   reserve correctly returned `StopValidator` while legacy cold still occupied
   the disk.

`v0.5.265` introduced the role-bootstrap correction, and `v0.5.269` carries it
forward without changing consensus, Archive V2 object format, catalog format,
or capacity policy:

- the role marker codec and create-new durable writer are shared by the
  validator and signed `lichen-archive-v2` utility;
- `role-bootstrap` opens hot state and legacy cold read-only, proves exact
  catalog/genesis identity, canonical slot 0, genesis replay mode, hot-window
  completeness, catalog coverage and tip parity, WAL/identity/recovery
  presence, role admission, and complete matching source inventories;
- the command requires explicit stopped-validator and low-space-retirement
  acknowledgements, supports a no-write dry run, refuses to overwrite a role
  marker, and still requires the network's absolute mutable-storage floor;
- after the external role marker is verified, the publish pass writes and
  WAL-syncs the same chain-and-role-bound node-local admission fingerprint used
  by a successful fresh role sync. A missing fingerprint is created exactly
  once; malformed or conflicting state fails before role-marker publication;
  this permits V2-primary reads and catalog-bound hot-checkpoint handoff only
  after the complete stopped-state proof;
- a non-Normal capacity result may authorize only the marker needed for bounded
  offline retirement. It is reported as `runtime_admitted=false` and does not
  authorize startup;
- startup may bypass fresh-sync deferral only when a regular, checksummed marker
  matches the exact catalog identity, chain ID, role, retention, and cache
  policy requested on the command line. Missing markers still defer; corrupt,
  mismatched, symlinked, or unsupported markers fail closed; and
- `scripts/archive-v2-low-space-role-bootstrap.sh` pins the signed utility hash,
  requires the validator service to be stopped, compares dry-run and publish
  evidence for both markers, and performs no validator start or legacy
  deletion.

The role marker and state admission fingerprint are prerequisites, not a
retirement receipt. Each subsequent
retirement unit still requires its signed source-backed retirement manifest,
bounded tombstone/reclaim pass, physical-space proof, post-unit archive parity,
and own-state validator rejoin. Runtime activation remains prohibited until
ordinary `role-preflight` reports `admitted=true` with capacity action
`Normal`.

### 3.6 Filesystem and capacity isolation

Approved capacity is computed as:

```text
required = system
         + hot_state_peak
         + hot_history_window
         + archive_segments_or_cache_quota
         + segment_staging_peak
         + bounded_compaction_peak
         + rollback_copy_peak
         + logs_and_evidence
         + adaptive_runtime_reserve
```

The hot-state/WAL reserve and archive reserve must be separate filesystems,
logical volumes, or enforced project quotas. Archive work stops first. Failure
to cache or build archive data may degrade historical RPC, but must not make a
validator unable to persist consensus state.

## 4. Track A: Complete Archive V2 And Restore Clean Cadence

### A0. Preserve the current boundary

Before source or fleet work:

- preserve the current signed `v0.5.265` rollback artifact and immutable audit
  evidence for prior releases;
- preserve validator keys, signer keys, identities, WALs, hot state, legacy
  cold data, FUSE plans, dual-R2 markers, signed retirement authorizations,
  retirement journals, operations logs, and sealed evidence;
- prohibit attempt 799 and unplanned additional FUSE offloads;
- verify all four validators locally commit and share a new common slot/hash;
- verify zero broken links, failed FUSE units, recovery jobs, and watchdogs;
- record root free space, memory, pressure, FUSE fingerprints, open archive
  descriptors, and exact service properties.

No later phase may weaken these gates.

Current execution order is fixed:

1. Preserve the signed `v0.5.265` deployment, exact four-host failure evidence,
   US recovery tail, and prior outage/rejoin and RPC/FUSE diagnostics. Do not
   weaken timeouts, history requirements, or storage reserves.
2. Preserve the completed clean runtime gate and `v0.5.268` hosted evidence,
   including both fail-closed finalized-RPC polling attempts, and qualify the
   narrow `v0.5.269` concurrent health-frontier correction through every
   affected local gate.
3. Commit through protected `main`, create the immutable `v0.5.269` tag, wait
   for every hosted hard gate, attach the detached PQ checksum signature, verify
   provenance and every binary hash, and publish the public validator release.
4. Capture a stopped preflight on all four hosts, prove identities/WALs/state,
   preserve `v0.5.265`, install only the signed workflow artifact, and perform
   one coordinated four-host stop/install/start. Prove installed/running hash
   parity and four-way own-state convergence before any retirement.
5. Freeze and audit the selected US recovery source, capture its final WAL
   boundary only after stop, and extend only the missing immutable catalog tail.
   Dual-publish objects and manifests with independent temporary credentials,
   verify both buckets, and publish each bucket's catalog last.
6. Stage the exact common catalog on every host. Bootstrap low-space hosts and
   retire legacy cold/FUSE dependencies one validator at a time with signed,
   source-backed, range-bound receipts and bounded compaction. Preserve
   three-vote finality and require own-WAL rejoin plus normal capacity admission
   after every unit.
7. Perform one coordinated role activation with all four validators using the
   same `verified_cache` policy and 2 GiB hard quota; prove deep-history parity,
   source-loss isolation, fixed-tip
   equality, restart, outage behavior, and zero legacy/FUSE descriptors.
8. Repeat the 1,000-consecutive-commit live cadence gate, require all four
   validators to author, measure reclaimed bytes per host, and complete the
   documented stable-observation window.
9. Publish and deploy the matching wallet `0.1.9`, exchange
   `exchange-testnet-v0.5.269`, developer portal, README, and frontend surfaces
   only after their live readiness evidence is attached. Keep only the new
   signed validator release and signed `v0.5.265` rollback installation on each
   VPS; remove obsolete caches, staging, superseded checkpoints, and redundant
   recovery copies without touching the preserved US tail before parity.
10. Re-inventory R2 only after bridge retirement and stable acceptance. Prepare
   an exact obsolete-object deletion manifest, but do not execute it without
   explicit operator approval of that manifest SHA-256. Any later authorized
   cleanup must delete only those keys and prove retained canonical inventory
   unchanged. No R2 deletion is authorized by this execution phase.

### A1. Ship cadence observability before guessing

Create a signed, coordinated diagnostic release that records monotonic phase
timestamps without changing consensus decisions.

Required per-height/per-round events:

- round start and selected proposer;
- proposer intent sent and received;
- proposal build start/end with separate mempool selection, speculative
  execution, state-root, serialization, and signing durations;
- proposal first byte/full payload received by peer and source peer identity;
- local prevote sent;
- each unique prevote received and cumulative eligible voting power;
- polka reached;
- local precommit sent;
- each unique precommit received and cumulative eligible voting power;
- commit reached/applied;
- timeout creation, firing, cancellation, and event-loop delay at firing;
- sync-manager state, pending batch, highest observed slot, and admission gate;
- validator-set hash and effective timeout configuration.

Required host correlation telemetry:

- Tokio/event-loop scheduling latency;
- process and cgroup CPU, run queue, throttling, steal, and context switches;
- local-state RocksDB get/write/flush/compaction latency by column family;
- archive/FUSE read count, bytes, latency, timeout, and error count;
- disk device latency/utilization and PSI;
- QUIC per-peer RTT, retransmit/loss, send-queue age, and gossip queue depth;
- checkpoint, replay, cold-maintenance, compaction, and archive-task spans with
  a stable task ID;
- Explorer cadence sample count, head staleness, window reset reason, and
  selected RPC origin.

Telemetry itself must be non-blocking and bounded: fixed-size preallocated
events, bounded queues, explicit drop counters, no synchronous fsync/network/
R2/FUSE operation on a consensus executor, a CPU/I/O budget, and a telemetry-off
A/B control. Observability that perturbs consensus cannot be used as evidence.

Logs must make it possible to classify every interval above 800 ms as one of:

- proposer build/speculative execution;
- proposal transport/delivery;
- missing or late prevote;
- missing or late precommit;
- local event-loop/scheduler delay;
- state storage latency;
- archive/background contention;
- sync/admission interference;
- explicitly unknown with the missing evidence named.

### A2. Remove known background failure loops

The same signed release, or a separately reviewed release if risk warrants,
must:

1. Stop the 15-second checkpoint/full-replay retry storm.
   - Consensus-only nodes must not repeatedly attempt archive-style checkpoint
     work.
   - A known unsupported link/checkpoint operation becomes a typed state with
     exponential backoff and jitter, not an immediate fixed-period retry.
   - During the coordinated stop, replace only the audited hot-state SST links
     to `/dev/shm` with verified regular files on the state filesystem. The
     procedure is per-file: resolve a fixed link, reject an unexpected target,
     copy without opening RocksDB, compare byte count and SHA-256, fsync the
     replacement, atomically rename it over that exact link, and fsync the
     directory. Abort before restart on any mismatch.
   - Checkpoint construction must use only approved persistent local state; it
     must never mutate or checkpoint through volatile/FUSE-backed SSTs.
   - One node's inability to create a checkpoint must not cause every peer to
     repeat full-replay discovery every 15 seconds.
2. Stop legacy cold migration when Archive V2 runtime is disabled or when its
   reclaim queue is terminally blocked.
   - The task records one durable paused reason and backs off.
   - It must not rescan unchanged data every five minutes.
   - It remains outside the consensus executor and retains row/byte/time limits.
3. Apply identity-staggered scheduling to all remaining maintenance work.
4. Enforce CPU, I/O, memory, and wall-time budgets for archive/background jobs.
5. Make every background task report whether it touched hot state, legacy
   archive, V2 segments, FUSE, checkpoint storage, or no storage at all.

These changes require the full release gates. They must not be applied as
untracked environment edits on one live host.

### A3. Reproduce and isolate the cadence defect

Run the strict four-validator local test with production-equivalent:

- cross-region latency, jitter, packet loss, and bandwidth limits;
- the current hot/cold database shape;
- the current number and memory profile of FUSE/rclone mounts;
- public RPC load and empty-mempool operation;
- checkpoint and maintenance scheduling;
- validator restart/catch-up and one-validator outage scenarios.

The mandatory RG-403 liveness case additionally stops validators 2-4, proves
that validator 1 cannot advance without quorum, admits 96 uniquely signed
transfers, resumes validators 2-4, and requires all 96 transactions to finalize
while each committed block respects the count-and-compute proposal budget (at
most fourteen default-budget transfers plus one parent commit certificate).
The cluster must reconverge and continue producing.

Execute controlled comparisons:

1. baseline v0.5.250 behavior;
2. telemetry-only behavior;
3. checkpoint retry disabled/contained;
4. terminal cold maintenance disabled/contained;
5. historical RPC isolated from voting validators;
6. Archive V2 role topology enabled on properly provisioned storage.

Change one factor per comparison. Preserve raw phase events and compute results
by slot and proposer, not only by host-local wall-clock windows.

### A4. Cadence correction gate

Do not select a fix only because the Explorer returns to 300 ms. A fix must
explain and remove the recorded phase failure.

Candidate fixes may include:

- event-loop or blocking-work isolation;
- QUIC/gossip queue prioritization for proposals and votes;
- bounded proposal-build work and speculative-execution cache correction;
- state RocksDB scheduling/compaction isolation;
- historical RPC isolation;
- checkpoint/replay backoff and role gating;
- corrected timer handling where instrumentation proves a late or starved
  timeout future.

Do not simply lower the full proposal timeout. Honest proposals have already
taken about 1.4 seconds to build, and measured cross-region RTT can approach
300 ms. A globally short full-proposal timeout would manufacture more round
changes.

For v0.5.258, the selected correction is deliberately bounded and
consensus-compatible with the existing transaction/block wire format:

- at most 16 pending user transactions, 17 total entries including the parent
  certificate, and 2.8 million aggregate declared compute units per live BFT
  proposal (fourteen default-budget transactions);
- the parent commit certificate remains separate and mandatory;
- the same count and compute limits are applied at every live proposal
  construction site and before any received proposal is executed;
- excess mempool work remains queued for later blocks rather than entering the
  current round's speculative execution and peer validation budget;
- built-in oracle ingress defaults return to 30-second minimum interval,
  60-second maximum staleness, and 10-bps minimum price movement;
- explicit operator overrides remain available, but production acceptance
  records the effective values and rejects the former 5/15/1 defaults.

This deliberately favors finality over unbounded burst TPS while retaining a
useful multi-transaction block. Raising either consensus limit requires
adversarial cost benchmarks and cross-host deterministic acceptance first.

v0.5.259 attempted to retain exact voting-ready tip parity:

- the local canonical tip must still be at or ahead of the authenticated
  observed network tip before voting admission can progress;
- any queued block remains blocking even if the tips momentarily compare equal;
- an active sync-manager batch guard is non-blocking only when its receive queue
  is empty and local canonical tip parity is exact;
- the same rule is enforced before the post-effects readiness scan, after that
  scan, and after fresh-validator registration;
- the existing 10-second passive tracking proof remains required on a moving
  network, and the stalled-network recovery path remains a bounded quorum
  restoration fallback rather than the normal rejoin path;
- live acceptance must stop one validator while the other three advance, then
  prove the returning validator enters BFT and commits without pausing the
  surviving quorum.

The live canary falsified exact parity as a moving-network invariant. v0.5.260
therefore uses the existing one-slot passive tracking bound, but does not make
it an unconditional voting bypass:

- only an already-staked returning validator can use the bounded tracking
  path;
- canonical readiness runs before the stability timer;
- the node must advance at least three slots over at least ten seconds;
- any queued block or drift beyond one slot blocks admission, and material
  drift resets the timer;
- fresh joins, registration, and stalled-network recovery still require exact
  parity;
- the clean local gate and live outage/rejoin gate must both prove automatic
  BFT entry, local commits, and authorship while the other three validators
  continue moving.

This is compatible with voting-only `consensus`, remote-backed
`verified_cache`, and persistent `full_archive` validators because consensus
admission depends only on canonical hot-state readiness, not deep-history
availability. Archive source loss may fail deep reads closed, but it must not
alter voting membership or finality readiness.

### A5. Provision the final storage plane

For the bounded existing-testnet transition in Section 3.5, first:

- preserve the current free-space runway and reject any role transition whose
  exact adaptive-capacity preflight is not `Normal`;
- use the signed Archive V2 retirement path, rather than another unbounded
  emergency FUSE batch, to release the large legacy archives after parity;
- configure the canonical primary and replica HTTPS prefixes as distinct
  authenticated source roots on all four validators using root-owned
  credentials and bounded 1 MiB buffering;
- place the same catalog locally on all four hosts and separately quota every
  validator cache at 2 GiB so eviction cannot consume the hot-state/WAL reserve;
- prove R2 source loss and cache eviction do not alter consensus readiness;
- retain every emergency bridge until the replacement/no-reference deletion
  gates in A9 pass.

Before mainnet or final production acceptance:

- provision at least three approved full-archive failure domains;
- mount and verify dedicated RAID1/block storage;
- reserve hot state/WAL capacity separately from archive/staging capacity;
- measure read/write/fsync/compaction performance and degraded-RAID behavior;
- verify trim, mount flags, ownership, restart persistence, and monitoring;
- configure two object-store domains and an independent recovery copy;
- prove restore into a fresh host from immutable segments and network state;
- retain enough space for legacy plus staged V2 data through the rollback
  window.

The current 200 GB roots and volatile emergency SST links are approved only for
the bounded testnet transition; they are not an approved mainnet or final
production base.

### A6. Complete Archive V2 data-plane qualification

For every backed historical range and category:

- build deterministic V2 segments;
- verify canonical block/body/header, parent, transaction, state-root, oracle,
  fee, and consensus-envelope semantics;
- upload to both object-store domains;
- independently read back and hash every object;
- build and sign manifests/catalog roots;
- prove full-archive local inventory;
- prove verified-cache two-source fetch and corruption rejection;
- compare V2, legacy hot/cold, and public RPC responses;
- abort on any same-key semantic conflict or incomplete source history;
- keep the existing testnet legacy-loss waiver explicit and non-transferable.

No original row is retired in this phase.

For the immediate tail extension, use approximately 10,000-slot segments from
11,589,000 through at least the stopped snapshot tip minus 50,000. Every segment
must fit the 1 GiB source-object bound, be deterministically re-encoded and
hash-compared, be verified after upload to both buckets, and advance the catalog
by exactly one predecessor root. Publishing or local staging is resumable;
same-key conflicts, a source gap, parent mismatch, catalog fork, or capacity
decision other than `Normal` aborts the run.

### A7. Canary V2 reads and publish a rollback anchor

Proceed in order:

1. Enable V2 dual-read on a non-voting/canary archive origin.
2. Compare exact responses, latency, cache behavior, and typed failures.
3. Make V2 primary for sealed ranges with legacy fallback.
4. Produce a clean commit and pass all release gates.
5. Publish a signed release that can read the activated Archive V2 topology.
6. Prove four- and ten-validator restart, fresh join, archive-origin loss,
   object-store loss, and rollback drills.
7. Declare that signed release the new rollback anchor only after evidence is
   sealed.

The v0.5.238 rollback remains preserved until the new anchor is explicitly
approved. Irreversible legacy deletion before a V2-capable rollback anchor is
prohibited.

### A8. Activate roles and edge routing

Activation is a coordinated, signed configuration transition:

- every process has an explicit role and versioned role configuration;
- every host's Archive V2 adaptive capacity decision is `Normal` before stop
  and after restart;
- archive/cache filesystems, quotas, and cgroup/resource budgets match the
  signed role configuration;
- terminal legacy maintenance is disabled or durably backed off;
- tests prove that proposal construction, block validation, voting, and hot
  state persistence cannot reach a V2/FUSE reader path;
- role capability is signed and visible in health/RPC/P2P;
- consensus-only nodes cannot advertise deep history;
- verified-cache nodes enforce cache quotas and two-source verification;
- full archives prove complete local inventory and reserve headroom;
- edge routing is updated only after origins pass role readiness;
- removing an archive origin does not alter the validator set;
- all validators share a fixed common pre-transition slot/hash and subsequently
  enter BFT and commit a new common slot/hash.

On the existing four-validator testnet, activate the exact Section 3.5 matrix:
all four validators use `verified_cache`, the same newly extended catalog, the
normal 100,000-slot hot window, the same authenticated primary/replica source
policy, and a 2 GiB hard cache quota. US's temporary 200,000-slot bridge is not
the final configuration. Require all four to pass the same role and capacity
preflight and share a post-transition fixed slot/hash before retirement. This
keeps every voting process on one Archive V2 admission/read topology without
falsely advertising any current VPS as a local full archive. The later
dedicated archive plane removes historical RPC from voting hosts entirely.

### A9. Retire legacy data and remove emergency bridges

After role activation and the new rollback anchor:

- retire one signed, authorized range/category at a time;
- use one bounded tombstone or reclaim call per admission;
- preserve exact pre-resume backups and journals;
- verify both object-store copies and full/archive/cache RPC parity;
- enforce source, staging, compaction, and release-byte bounds;
- restore the validator and prove local BFT before sealing each unit;
- move FUSE-linked legacy SSTs back to approved persistent placement or retire
  them only after their V2 replacement is independently proven;
- remove zero-link and obsolete FUSE services only with exact inventory and
  rollback evidence;
- never delete keys, WAL, state SSTs, signed artifacts, or unreplicated history.

R2 deletion is per-object and manifest-driven, never prefix-wide. Before any
delete, create a signed deletion manifest containing bucket, exact key, size,
SHA-256, originating plan/evidence SHA, replacement V2 object/catalog root,
rollback-retention expiry, and approval identity. Require an independent
review/explicit approval of that exact manifest. Enable object-lock/versioning
or an equivalent recovery window where available. After execution, record
per-object API results and prove a negative list on both buckets. Preserve the
manifest, signatures, provider receipts, and post-delete inventory as sealed
evidence.

The historical broad `emergency-sst/v0.5.240/` cleanup helper is not approved
for reuse. A live batch becomes eligible only after no symlink, open file
descriptor, mount, unit, rclone configuration, rollback, or recovery evidence
depends on it and its replacement passes fixed-tip full/cache/legacy RPC parity.
Canonical V2 segments, manifests, catalogs, and retirement receipts are not
deletion candidates under this plan.

The existing 1.85 TB provider total is therefore reduced only at the end of the
dependency chain. First remove every live legacy SST reference and unit, then
refresh both bucket inventories, subtract the retained canonical V2 chain and
all still-required rollback objects, and produce a new full-key deletion
manifest. The operator must approve that exact manifest SHA-256. Execution must
use the fail-closed exact-delete helper, record every object result, and prove
both a post-delete negative inventory for deleted keys and an unchanged positive
inventory for every retained canonical key.

### A10. Performance and completion criteria

Track A is complete only after all of the following pass:

- at least 24 hours of four-validator testnet observation with no unexplained
  synchronized timeout burst;
- all four validators continuously locally commit and author their expected
  stake-weighted share;
- rolling 120-sample median stays at or below the 400 ms target outside an
  explicitly injected fault;
- normal-operation p95 is at most 600 ms and p99 at most 1,000 ms, unless a
  separately approved measured WAN bound supersedes these provisional values;
- round greater than zero is below the approved SLO and every occurrence above
  the slow-slot threshold is classified by telemetry;
- no checkpoint/replay retry storm or terminal maintenance rescan remains;
- no consensus state SST is remote/FUSE-backed;
- Archive V2 roles are enabled and correctly advertised;
- full-archive/catalog parity holds from genesis to the fixed tip, subject only
  to the explicit existing testnet waiver;
- archive/cache-origin loss does not affect consensus cadence;
- all hard release, rollback, restart, join, outage, and public-history gates
  pass on the signed artifact;
- emergency FUSE dependence is removed or has an explicitly approved bounded
  residual use with persistent restart-safe mounts;
- capacity forecasts retain the approved hot, archive, staging, rollback, and
  runtime reserves.

A strict guarantee that every Internet-spanning slot is below 400 ms is not a
realistic safety property. The acceptance target is stable 400 ms-class
round-zero production, tightly bounded and explained tails, and no persistent
timeout cascade.

## 5. Track B: Deterministic Offline-Validator Quarantine

This track is design-only until Track A is stable and a separate consensus
release is approved.

### B1. Goals

- Do not repeatedly select known unavailable validators as proposers.
- Do not count deterministically quarantined stake in the live quorum
  denominator.
- Do not allow one node's local ping, clock, or network partition to mutate the
  validator set.
- Let an unavailable validator remain sync-only and return safely.
- Bound flapping and denial-of-service behavior.
- Preserve BFT quorum intersection and deterministic replay.
- Reduce the immediate cost of an offline proposer before quarantine becomes
  effective.

### B2. Non-goals

- Do not stop, restart, or signal another operator's VPS.
- Do not treat a TCP/QUIC disconnect or missed ping as consensus evidence.
- Do not slash stake solely for ordinary downtime.
- Do not permit local proposer skipping.
- Do not attempt automatic reconfiguration after the old active set loses the
  quorum required to finalize the transition.
- Do not combine archive role loss with validator membership loss.

### B3. Consensus state model

Introduce a versioned roster with four states:

```text
registered -> pending_activation -> active
active -> quarantine_pending -> quarantined
quarantined -> rejoin_pending -> active
```

Consensus state contains:

- `active_set_version`;
- `active_set_hash`;
- ordered validator identities and effective voting power;
- availability window/checkpoint number;
- pending quarantine/reactivation transitions and their effective boundary;
- signed evidence/certificate hash;
- cooldown and consecutive-readiness counters.

Only `active` validators participate in proposer selection or the quorum
denominator. The set and denominator remain frozen between deterministic
boundaries.

### B4. Availability periods

Do not reuse the current 432,000-slot economic epoch as the only liveness
boundary. At 400 ms per slot it lasts approximately 48 hours, which would leave
offline leaders in rotation far too long.

Introduce a separate consensus `availability_period`, initially benchmarked in
the 256-1,024 slot range:

- 256 slots is approximately 102 seconds;
- 1,024 slots is approximately 6.8 minutes.

Provisional policy for simulation:

- collect evidence for two consecutive periods before quarantine;
- apply the transition at the next availability checkpoint;
- require at least two consecutive healthy periods before reactivation;
- apply a longer cooldown after repeated flap cycles;
- leave economic stake/reward epochs at 432,000 slots.

The final constants must come from partition, delay, and censorship testing.

### B5. Deterministic availability evidence

Local `last_seen`, ping, RPC, or wall-clock state is advisory only. Canonical
evidence must be signed, chain-bound, and replayable.

Define a signed `AvailabilityHeartbeat` containing:

- chain ID and protocol version;
- validator identity;
- availability period;
- latest finalized slot and block hash;
- active-set version and hash;
- signer/WAL continuity commitment where appropriate;
- monotonic sequence number;
- expiration boundary.

Heartbeats are gossiped and may be included by any proposer. Peers may issue
signed receipts so a validator can prove timely dissemination even if one
proposer censors it.

Canonical participation also derives from commit certificates, proposal
intents, proposals, prevotes, and precommits already accepted by consensus.

Define `QuarantineCertificate` as an old-active-set quorum authorization over:

- target validator;
- completed evidence windows;
- canonical participation summary;
- absence of a valid positive availability certificate;
- old active-set version/hash;
- proposed new active-set version/hash;
- effective checkpoint.

The transition is valid only if a stake-weighted quorum of the old active set,
pinned to a specific finalized height and active-set hash, supplies the same
threshold required by consensus. A local observation can propose a certificate
but cannot apply it. Heartbeats and receipts require bounded canonical storage,
strict chain/height/window/set binding, expiration, replay rejection,
equivocation handling, and an anti-censorship path through multiple proposers.

### B6. Quarantine transition

At the activation boundary:

1. The old active set finalizes the transition block/certificate.
2. The block commits the next ordered set, voting powers, version, and hash.
3. The old set remains authoritative for that transition block.
4. The new set becomes authoritative only at the specified next height or
   checkpoint.
5. All votes, proposals, and blocks bind the effective set version/hash.

Quarantine is non-punitive by default:

- the validator stops earning participation rewards;
- it is excluded from leader selection and quorum calculation;
- stake is not destroyed for downtime;
- Byzantine evidence such as double-signing remains a separate slashing path.

Limit transition churn:

- never remove enough voting power to make the new set violate configured
  minimum stake/count/diversity policy;
- bound removals and power changes per checkpoint;
- reject conflicting transitions for the same set version;
- retain a deterministic rollback/recovery path for an invalid transition.

### B7. Peer and ping policy

Consensus membership and network traffic are separate:

- active peers receive consensus gossip and normal liveness traffic;
- unreachable active peers use bounded exponential reconnect backoff;
- quarantined peers are removed from the active consensus dial set;
- quarantined nodes may remain connected through a rate-limited sync/readiness
  channel or submit an inbound rejoin request;
- discovery checks use long backoff and jitter rather than a five-second
  broadcast ping;
- peer routing changes never alter consensus state without the on-chain
  transition.

This stops wasting consensus traffic on quarantined nodes while preserving a
safe return path.

### B8. Safe re-admission

A quarantined validator submits a signed `RejoinIntent` bound to:

- chain ID;
- current finalized slot/hash;
- current active-set version/hash;
- validator identity and stake account;
- software/release compatibility policy;
- a new monotonic sequence number.

It then operates in observer/sync-only mode and must prove:

- exact supported release/protocol compatibility;
- successful sync to the finalized checkpoint;
- validator key and WAL continuity without conflicting signatures;
- healthy participation/readiness heartbeats for consecutive periods;
- required network reachability and resource/capacity policy;
- no active quarantine cooldown.

The old active set finalizes a `ReactivationCertificate`. The validator becomes
eligible only at the next deterministic availability checkpoint. It must never
be reinserted immediately because one peer received a ping.

Economic-epoch-only reactivation is safe but can delay a recovered validator
for up to roughly 48 hours. The preferred design uses the shorter availability
checkpoint for roster changes while keeping economic accounting at the long
epoch.

### B9. Fast path for an offline proposer

Quarantine takes multiple evidence windows. Reduce the cost before it applies
with a two-stage proposal protocol.

At the start of a height/round, the deterministic proposer immediately signs
and broadcasts `ProposalIntent` containing:

- chain ID;
- height and round;
- parent block hash;
- active-set version/hash;
- proposer identity;
- optional bounded build metadata;
- expiration/deadline.

Behavior:

1. Start a short intent deadline, provisionally 350-500 ms and finalized only
   after WAN testing.
2. If a valid intent arrives, retain the full proposal deadline so an honest
   slow build can finish.
3. If no intent arrives, validators follow the ordinary deterministic nil
   prevote/round-change path.
4. If intent arrives but the block does not, record an attributable liveness
   fault and follow the full timeout path.
5. Never locally substitute a different round-zero proposer.

Intent messages have fixed maximum size and per-height/round admission limits.
Only the selected proposer may issue one, duplicates are deduplicated, and an
intent never extends the existing full proposal deadline.

The short timer detects absence; it does not replace the full block-build
deadline. This avoids setting a 300-500 ms full proposal timeout when observed
honest block construction has occasionally taken around 1.4 seconds.

### B10. Ten-validator, eight-online behavior

For ten equal-stake validators with two unavailable:

1. The eight live validators retain 80% of old-set voting power and can commit.
2. Before quarantine, an unavailable selected proposer causes a bounded intent
   or proposal timeout and round change.
3. After two evidence periods, the old eight-validator quorum may finalize a
   transition removing the unavailable two from the active roster.
4. At the effective checkpoint, proposer selection and quorum calculation use
   only the eight-member active set.
5. The two quarantined nodes receive no normal consensus pings and remain
   observer/sync-only.
6. A returning node proves readiness and is reactivated only at a later
   deterministic checkpoint.

If live old-set stake falls below the required threshold, the chain cannot
safely finalize its own membership repair. The valid options are active/passive
validator identity failover with single-signer/WAL protection or an explicit
governance/recovery procedure. Local auto-skipping cannot solve this safely.

### B11. Validator identity high availability

For important identities, add active/passive availability independently of
roster quarantine:

- one active signer lease at a time;
- replicated or remotely protected slashing/WAL continuity state;
- fencing that makes concurrent signing impossible;
- deterministic failover health and release gates;
- no shared writable RocksDB between active and standby processes;
- recovery drills for lease loss, network partition, stale standby, and signer
  unavailability.

This prevents many proposer absences without changing the quorum denominator,
but it must never permit double-signing.

### B12. Implementation phases

1. **B0 specification:** freeze encodings, signatures, set-version semantics,
   transition rules, parameters, and adversarial model.
2. **B1 telemetry:** deploy availability/participation reporting with no state
   changes.
3. **B2 shadow certificates:** construct and compare proposed certificates on
   all nodes without applying them.
4. **B3 inactive state machine:** implement replay, snapshots, RPC, and tests
   behind an unactivated version gate.
5. **B4 proposal intent:** test the two-stage timeout under real WAN and build
   distributions.
6. **B5 local networks:** four- and ten-validator tests with offline, partitioned,
   censored, flapping, stale, and Byzantine nodes.
7. **B6 signed coordinated release:** activate only at a declared boundary with
   a frozen old-set hash and rollback plan.
8. **B7 observation:** run at least one complete economic epoch and multiple
   quarantine/reactivation cycles before mainnet consideration.

### B13. Mandatory tests

- 4 validators: 1 offline, 1 partitioned, 1 flapping, and 1 slow proposer.
- 10 validators: 2 offline, 3 offline, unequal stake, and geographically
  correlated failures.
- Exactly-threshold and below-threshold liveness cases.
- Conflicting local ping views with identical deterministic outcome.
- Heartbeat censorship and delayed receipt propagation.
- False proposal intent, intent equivocation, proposal equivocation, and
  double-vote evidence.
- Rejoin with stale state, wrong set hash, wrong release, missing WAL
  continuity, and valid fully synchronized state.
- Old-set/new-set boundary restart, snapshot, replay, rollback, and fresh join.
- Active/passive fencing and double-signer prevention.
- Archive-origin loss with no validator-set change.
- Deterministic state root and validator-set hash parity on every node.

## 6. Release And Operational Gates

Every consensus or Archive V2 release must pass the workspace hard gates,
including formatting, Clippy, all-feature tests, RustSec/cargo-deny, contract and
genesis builds, frontend/SDK/deployment QA, the strict local multi-validator
suite, live artifact parity, and genesis-to-tip archive parity.

Additionally require:

- clean committed worktree before tag/build;
- signed workflow artifacts and detached PQ checksum signature;
- coordinated install for consensus-critical changes;
- fixed-tip pre/post common block hashes;
- installed/running binary parity;
- exact rollback artifacts on every host;
- no mixed active-set or Archive V2 role-config version;
- independent start watchdogs for bounded coordinated restarts;
- local BFT entry and recent authorship, not merely RPC tip advancement;
- atomic, fsynced, immutable evidence sealing.

## 7. Explicit Prohibitions

- No local ping-based validator removal.
- No local quorum denominator adjustment.
- No immediate rejoin based on one successful connection.
- No lowering the full proposal timeout without phase evidence and WAN tests.
- No new retirement authorization reused for a different slot range.
- No attempt 799 until a new admission is independently designed and approved.
- No additional emergency FUSE offload as a substitute for storage provisioning.
- No state SST, WAL, key, identity, or signer placement on remote/FUSE archive
  storage.
- No legacy deletion before dual-replica V2 proof and a signed V2-capable
  rollback anchor.
- No claim of completion based only on four green service indicators or one
  300 ms Explorer sample.

## 8. Deliverables And Exit Ownership

| Deliverable | Track | Exit evidence |
| --- | --- | --- |
| Phase-level cadence telemetry | A | Every slow slot classified from signed release logs |
| Checkpoint/replay and cold-maintenance containment | A | No fixed retry storm; bounded backoff/status tests |
| Dedicated archive capacity | A | Storage, RAID, reserve, failure, and restore evidence |
| Complete V2 catalogs and replicas | A | Exact local/two-domain/offline-copy roots |
| V2-capable rollback anchor | A | Signed artifact plus restart/rollback drills |
| Explicit role and edge activation | A | Full/cache/consensus readiness and route-loss drills |
| Legacy/FUSE retirement | A | Per-range signed evidence and persistent restart-safe layout |
| Stable cadence acceptance | A | 24-hour SLO and classified-tail report |
| Availability protocol specification | B | Reviewed canonical encoding and transition document |
| Shadow availability certificates | B | All validators derive identical inactive results |
| Proposal-intent fast path | B | WAN/partition/slow-build safety and latency results |
| Quarantine/rejoin state machine | B | Deterministic replay, boundary, snapshot, and adversarial tests |
| Ten-validator activation | B | Signed coordinated release and multi-cycle observation |

Track A is the prerequisite for declaring the current recovery complete.
Track B remains unactivated until its own specification, implementation,
testing, audit, signed release, and boundary transition are complete.

## 9. References

- `docs/deployment/ARCHIVE_V2_SEGMENTED_STORAGE_PLAN_2026-07-21.md`
- `docs/deployment/STORAGE_PHYSICAL_AND_CODE_AUDIT_2026-07-23.md`
- `memories/repo/current-state.md`
- `memories/repo/2026-08-13-v05247-retirement-recovery-handover.md`
- `core/src/state/metrics_state.rs`
- `core/src/consensus.rs`
- `validator/src/consensus.rs`
- `validator/src/main.rs`
- CometBFT proposer selection:
  <https://docs.cometbft.com/v0.37/spec/consensus/proposer-selection>
- CometBFT consensus rounds:
  <https://docs.cometbft.com/v0.38/spec/consensus/consensus>
- CometBFT validator-set state transitions:
  <https://docs.cometbft.com/v0.38/spec/core/state>
- HotStuff protocol background: <https://arxiv.org/abs/1803.05069>
