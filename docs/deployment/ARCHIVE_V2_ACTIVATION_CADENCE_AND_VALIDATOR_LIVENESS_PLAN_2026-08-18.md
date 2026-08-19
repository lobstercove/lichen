# Archive V2 Activation, Cadence Recovery, And Validator Liveness Plan

**Date:** 2026-08-18
**Status:** Authoritative execution plan; Track A implementation in progress
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

The fleet is safe and live, but it is not yet in the intended final state.

- All four validators are active, locally committing, advancing, on the same
  signed v0.5.250 artifact, and have `NRestarts=0`.
- The normal round-zero cadence is about 300-400 ms, but recurring round-one
  and round-two BFT escalations still create 1-8 second tails.
- The Explorer's `Observed ... ms avg` value is not an arithmetic average. It
  is the upper median of at most 120 observer-side normalized block-arrival
  samples. It can therefore move from roughly 300 ms to roughly 1,000 ms when
  a sustained timeout burst occupies more than half of the rolling window.
- The Archive V2 binary and role implementation are present, but the live
  validators do not currently run an Archive V2 role. Their command lines have
  no `--archive-v2-role`, and live health reports
  `archive_v2.enabled=false`.
- The existing read-only FUSE SST mounts are an emergency, dual-R2-backed
  legacy archive offload. They are not the final full-archive,
  verified-cache, and consensus role topology.
- Current disk free space is runway, not a permanent capacity solution. The
  last verified figures were approximately 28.09 GB US, 8.77 GB EU, 13.15 GB
  SEA, and 13.18 GB IN.

Accordingly:

1. Do not call the recovery 100% complete while recurring BFT timeout bursts
   remain unexplained or while Archive V2 role activation is disabled.
2. Do not start reclaim attempt 799 or another emergency FUSE batch merely to
   conceal the capacity constraint.
3. First ship an observability and background-work isolation release, identify
   the exact cadence trigger, and prove clean four-validator performance.
4. Provision the intended storage plane and complete the Archive V2 migration,
   rollback, role, and edge-routing gates.
5. Treat offline-validator quarantine as a separate versioned consensus change.

## 2. Authoritative Current Evidence

### 2.1 Fleet integrity

The last verified safety state is:

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

### 2.2 Cadence evidence

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

### 2.3 What the source diff proves

The v0.5.229-to-v0.5.250 comparison found no change in:

- Explorer cadence rendering;
- the cadence sampling algorithm;
- consensus timeout helpers;
- proposer selection;
- deployed `400/2000/1000/1000/60000 ms` timing configuration.

The current block proposal path uses the hot transaction index and does not
perform deep Archive V2 reads. No live `state-testnet` SST is backed by an
Archive V2 FUSE mount. Active Archive V2 segment migration is also disabled.
This rules out a direct Archive V2 state-read regression.

It does not yet rule out indirect contention from the emergency recovery
topology, legacy archive access, scheduling, networking, or other periodic
tasks.

### 2.4 Background faults already identified

Two background conditions must be removed or contained:

- Each validator performs a checkpoint/full-replay retry approximately every
  15 seconds. US checkpoint creation repeatedly fails when attempting to link
  an SST into a checkpoint with `Operation not permitted`. Peers then report no
  checkpoint and continue bounded full-replay behavior.
- Legacy cold maintenance wakes approximately every five minutes and is
  terminally unable to progress because the reclaim queue is at or near its
  4,096-range limit.

Sixty minutes of correlation falsified the five-minute cold-maintenance wake as
the recurring BFT burst trigger: it also runs during quiet windows. The
15-second checkpoint retry loop is synchronized background load and a viable
amplifier, but it also exists in quiet windows and is not proven to initiate
the bursts.

The exact trigger remains unresolved because current logs do not record enough
proposal-receive, vote-receive, executor-delay, and storage-latency detail.

### 2.5 Current Cloudflare R2 custody

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

### 2.6 Exact catalog and source boundary found on 2026-08-18

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
| US | `verified_cache` | read-only primary and replica R2 source mounts | hard quota |
| EU | `verified_cache` | read-only primary and replica R2 source mounts | hard quota |
| SEA | `consensus` | none | none |
| IN | `consensus` | none | none |

All four hosts receive the same immutable catalog and therefore agree on the
same Archive V2 identity, catalog root, covered ranges, and loss declaration.
Their local object inventories intentionally differ by role; byte-for-byte
duplication of the 54.5 GB corpus on every 200 GB validator would defeat the
storage design. US and EU serve verified deep-history reads. SEA and IN keep
only the catalog commitment and the local hot window, and cannot advertise
deep-history readiness.

This is a testnet exception, not the final production archive policy:

- the two R2 buckets are one-provider failure domains and do not count as two
  independent providers;
- no VPS may claim `full_archive` without every segment on approved persistent
  local archive storage;
- R2 loss may make historical RPC unavailable, but the implementation and
  acceptance tests must prove it cannot affect proposal, validation, voting,
  hot-state persistence, or BFT admission;
- a scheduled, signed tail-publisher must keep the catalog ahead of every
  restart admission boundary; it publishes objects and manifests to both R2
  domains and publishes the catalog last only after dual read-back;
- every host must pass `role-preflight` with capacity action `Normal`; the
  current low-space EU/SEA/IN state must first be repaired with exact bounded
  archive-only offloads or larger storage;
- the transition is reversible through the preserved baseline environment and
  signed v0.5.250/v0.5.240/v0.5.238 artifacts.

The later dedicated archive plane in Section 3.2 replaces this exception. It
does not block honest Archive V2 role activation on the current testnet once
the catalog, capacity, source, and rollback gates are proven.

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

- preserve every signed release and rollback artifact;
- preserve validator keys, signer keys, identities, WALs, hot state, legacy
  cold data, FUSE plans, dual-R2 markers, signed retirement authorizations,
  retirement journals, operations logs, and sealed evidence;
- prohibit attempt 799 and unplanned additional FUSE offloads;
- verify all four validators locally commit and share a new common slot/hash;
- verify zero broken links, failed FUSE units, recovery jobs, and watchdogs;
- record root free space, memory, pressure, FUSE fingerprints, open archive
  descriptors, and exact service properties.

No later phase may weaken these gates.

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
   - Checkpoint construction must use only approved persistent local state or
     a tested copy/reflink fallback; it must never mutate FUSE-backed SSTs.
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

### A5. Provision the final storage plane

For the bounded existing-testnet transition in Section 3.5, first:

- restore enough per-host free space for each exact role preflight to return
  adaptive capacity action `Normal` (approximately one additional 10 GiB
  archive-only offload on SEA and IN and two on EU at the measured boundary;
  the exact plans and released bytes remain runtime gates, not assumptions);
- mount the canonical primary and replica prefixes read-only as distinct source
  roots on US and EU using root-owned credentials and bounded 1 MiB buffering;
- place the same catalog locally on all four hosts and separately quota US/EU
  caches so eviction cannot consume the hot-state/WAL reserve;
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
US/EU `verified_cache`, SEA/IN `consensus`. This keeps every voting process on
the Archive V2 admission/read path without falsely advertising any current VPS
as a local full archive. The later dedicated archive plane removes historical
RPC from voting hosts entirely.

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
