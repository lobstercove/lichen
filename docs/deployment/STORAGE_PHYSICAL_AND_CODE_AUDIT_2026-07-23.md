# Lichen Testnet Storage Physical And Code Audit

**Date:** 2026-07-23
**Code audited:** signed `v0.5.229` source at
`feb0a97bcc9e0cb8055e8e8c2abd5f78a8f41d80`; the audit documentation commit
above that tag does not change runtime code
**Live artifact audited:** validator SHA-256
`56ca8642d52b78f8ff166c733254a9b9a1da2d354c7d85261f77e12f3a03ab60`
on US, EU, SEA, and IN
**Architecture plan:**
[ARCHIVE_V2_SEGMENTED_STORAGE_PLAN_2026-07-21.md](ARCHIVE_V2_SEGMENTED_STORAGE_PLAN_2026-07-21.md)
**Purpose:** explain the current disk consumption and repeated validator stops,
identify storage and runtime amplification in code, and turn the approved
Archive V2 and validator-role direction into an evidence-backed execution
order.

## 1. Executive Outcome

The immediate stops were caused by the signed testnet runtime guard doing its
job: US and EU crossed below the temporary 5 GiB free-space floor and exited
with status 78 to protect RocksDB. That is the trigger, not the complete root
cause.

The underlying capacity problem has seven parts:

1. Lichen's self-contained post-quantum signatures are inherently large.
2. A block stores complete transaction bodies, and the current ledger layout
   stores each complete transaction a second time in the transaction column
   family.
3. Old history is in a mutable LSM database designed for ongoing writes, not a
   compact immutable chronological archive.
4. Every five minutes, every validator scans slot cursors from slot zero to the
   current cold cutoff because the runtime migration has no durable high-water
   cursor.
5. Cold migration performs conflict checks and writes one cold row at a time
   instead of using a bounded cold write batch.
6. Only eight historical families have cold equivalents; other current public
   history/index families do not yet have a bounded archival representation.
7. Every non-development testnet/mainnet validator is currently forced into the
   same full-archive behavior. Full archive, verified-cache, and
   consensus-only roles are designed but not implemented.

The cold archive **is already compressed with Zstandard**. Compression is not
absent. It is simply operating on the wrong physical representation for
long-lived history. Recent physical table samples showed only about 1.23-1.25×
compression for block and transaction values, which is expected when much of
the payload consists of high-entropy cryptographic material.

The durable solution is the already approved Archive V2 design:

- keep consensus state and 50,000 recent slots hot;
- move finalized history into immutable, content-addressed, seekable Zstandard
  segments;
- store transaction bodies and repeated keys once per canonical representation;
- keep compact, deterministic indexes and reconstruct exact RPC responses;
- make every node independently retain the state needed for consensus, while
  allowing only full-archive nodes to retain every historical payload locally;
- preserve at least three independent full-archive replicas plus verified
  remote/object and offline copies.

The temporary cache/swap bridge restored the four-validator fleet, but the
current free-space margin remains measured in hours, not days, at the recent
growth rate. Additional writable capacity is still the immediate operational
priority. Archive V2 cannot be safely built, verified, released, and used to
retire legacy data inside that remaining margin.

## 2. Live Incident And Recovery Evidence

### 2.1 Exact stop conditions

On 2026-07-23:

- US exited at 13:03:30 UTC because available space was
  `5,363,884,032` bytes, below the 5 GiB requirement of
  `5,368,709,120` bytes.
- EU exited at 12:04:29 UTC because available space was
  `5,368,438,784` bytes, also below the requirement.
- both exited with the intentional fatal-startup status 78;
- SEA and IN remained processes, but two of four equal-stake validators cannot
  finalize a 3-of-4 commit, so the chain stopped advancing.

The validator guard checks every 30 seconds. It intentionally stops without a
restart when the floor is breached, preventing a restart loop from consuming
the last filesystem blocks.

### 2.2 Safe cleanup result

The following disposable host data was cleaned on all four VPSes:

- APT package cache;
- old journal data, bounded to 128 MiB;
- normal rotated logs.

This reclaimed only about 154-254 MiB per host. No state, archive, WAL,
validator key, identity, peer database, rollback binary, provider backup,
repair evidence, or access configuration was removed. The result proves that
cache cleanup is useful hygiene but cannot solve this archive-capacity problem.

The separate provider-backup filesystems remain read-only recovery evidence.
They were not treated as scratch space. At the incident sample their free space
was approximately:

| Host | Provider-backup free space |
| --- | ---: |
| US | 92.8 GB |
| EU | 0.2 GB |
| SEA | 55.2 GB |
| IN | 33.6 GB |

Those filesystems may be retired or repurposed only after an explicit
evidence-retention replacement and restore proof. Their existence does not
make the active RocksDB filesystem safe.

### 2.3 Temporary runtime bridge

The installed and running validator hashes were verified as the exact signed
`v0.5.229` artifact before changing runtime settings. The following reversible
bridge was then applied to every host:

- preserve `/etc/lichen/env-testnet` as
  `/etc/lichen/env-testnet.before-20260723-storage-bridge`;
- set
  `LICHEN_EXTRA_ARGS=--auto-update=off --cache-size-mb 1024`;
- reduce `/swapfile` from 2 GiB to 1 GiB;
- stop and start the consensus fleet from a shared exact slot boundary;
- preserve all node-owned databases, consensus WALs, keys, and identities.

The hot RocksDB default cache had been automatically selected as 4 GiB from
host RAM. Reducing it to 1 GiB does not delete chain data; it trades read-cache
hit rate for RAM. That allowed 1 GiB of swap-backed disk to be returned to the
filesystem without removing state. The current cold database still creates
separate default per-column-family block caches; this bridge controls the hot
shared cache only.

### 2.4 Four-validator proof

After a coordinated exact-boundary start, all four services were active with
zero systemd restarts. A consecutive 40-block author sample contained:

| Validator | Blocks authored |
| --- | ---: |
| US | 10 |
| EU | 11 |
| SEA | 10 |
| IN | 9 |

This is stronger evidence than an eventually refreshed explorer counter: it
proves every validator was actively participating in block production during
the sampled range. Public RPC metrics also returned
`validator_count = 4` and nonzero block/transaction counts.

A later all-origin fixed-block proof at slot `10,078,200` returned the same
block hash
`daeeb87ab821de45d72ff530d3b024687edc24544903c7c3ed81568555622562`,
state root
`28ad3976932cc96aaa1318bf65e1ff2f09a9ad8198addf916d464af2d3abbdf8`,
and three-signature commit certificate on US, EU, SEA, and IN. The canonical
Explorer root returned HTTP 200, Explorer `/api/testnet` returned four
validators and nonzero metrics, and the canonical testnet API returned an
advancing slot.

### 2.5 Current physical footprint

The following byte-exact sample was taken at approximately 14:29 UTC after
recovery. `du` is logical filesystem occupancy at the named directory and
`df` is available space on the active Lichen filesystem.

| Host | Hot state bytes | Cold archive bytes | Available bytes | Available GiB |
| --- | ---: | ---: | ---: | ---: |
| US | 4,641,857,633 | 187,079,446,772 | 6,703,644,672 | 6.24 |
| EU | 4,773,373,471 | 189,227,682,435 | 6,325,411,840 | 5.89 |
| SEA | 4,742,293,670 | 188,305,246,290 | 7,577,743,360 | 7.06 |
| IN | 4,715,794,339 | 188,255,963,690 | 8,061,632,512 | 7.51 |

These values fluctuate during RocksDB flush and compaction. The meaningful
margin is space above the 5 GiB floor, not total `df` free space. EU had only
about 0.89 GiB above the floor in this sample.

The prior sustained sample was approximately 0.7 GB/hour per host. A
first-principles recent-record sample also implies roughly 0.4 GB/hour of cold
payload alone at current cadence, before hot data, indexes, WAL, and LSM write
amplification. A prudent near-term planning range is therefore 0.4-0.8 GB/hour.
At that range, the smallest margin above the safety floor is roughly 1-3
hours. Compaction can temporarily improve or worsen `df`; it must not be
mistaken for guaranteed capacity.

## 3. What Is Stored Today

### 3.1 Hot database

The hot database is RocksDB and holds:

- current consensus/account/contract state;
- slot cursors and tips;
- the most recent 50,000 blocks and transactions;
- recent and current public-history indexes;
- consensus-adjacent caches and metadata;
- public-history families that do not yet have a cold representation.

Hot column families use workload-specific profiles. The major historical
families use LZ4-oriented options and a shared block cache. LZ4 is appropriate
for frequently accessed mutable data, but it is not intended to maximize
long-term archival density.

The measured major hot column-family SST sizes on US were approximately:

| Hot family | SST size |
| --- | ---: |
| blocks | 1.78 GiB |
| transactions | 1.11 GiB |
| tx_by_slot | 196.75 MiB |
| tx_to_slot | 3.14 MiB |
| account_txs | 1.27 MiB |

The remainder is current state, other indexes, manifests, WAL/current files,
and LSM overhead.

### 3.2 Cold database

The cold database is a second RocksDB with eight column families:

1. blocks;
2. transactions;
3. transaction-to-slot;
4. account transactions;
5. account snapshots;
6. events;
7. token transfers;
8. program calls.

Every cold family uses:

- RocksDB Zstandard compression;
- 32 KiB table blocks;
- format version 5 for rollback compatibility;
- a 10-bit Bloom filter;
- a 32 MiB write buffer.

No explicit Zstandard level, trained dictionary, or shared cold-cache budget is
configured. More importantly, the data remains in a mutable LSM with WAL,
memtables, levels, overlapping SST history, Bloom filters, and compaction.

The measured cold SST breakdown on US was:

| Cold family | SST size | Share of measured cold SST |
| --- | ---: | ---: |
| blocks | 147.78 GiB | 84.85% |
| transactions | 25.98 GiB | 14.92% |
| tx_to_slot | 204.66 MiB | 0.11% |
| account_txs | 189.77 MiB | 0.11% |
| account_snapshots | 3.35 KiB | negligible |
| events, program calls | negligible | negligible |
| token transfers | zero in sample | zero |

Blocks and duplicate transaction values account for approximately 99.77% of
the measured cold SST bytes. Optimizing tiny index families first would not
materially change the capacity horizon.

### 3.3 Physical compression sample

Recent cold table properties gave these representative ratios:

| Records | Raw value bytes | SST bytes | Approximate ratio |
| --- | ---: | ---: | ---: |
| 382 blocks | 15,581,602 | 12,458,073 | 1.25× |
| 584 transactions | 7,374,446 | 5,989,161 | 1.23× |

Together these samples were about 48 KiB of cold block/transaction payload per
new block at the sampled transaction load, before hot rows, indexes, WAL, and
LSM overhead.

These ratios are observations, not the final Archive V2 benchmark. Archive V2
must benchmark representative old, recent, busy, and nearly empty ranges with
multiple levels, frame sizes, and dictionaries before selecting permanent
codec parameters.

## 4. Why The Data Is Large

### 4.1 Self-contained post-quantum signatures

`PqSignature` contains:

- a scheme byte;
- the complete public verifying key;
- the complete signature.

For ML-DSA-65, the code constants are:

- public key: 1,952 bytes;
- signature: 3,309 bytes.

That is 5,261 bytes before serialization metadata for every self-contained
signature. A normal finalized block carries:

- one producer signature in its header;
- three commit signatures in the 3-of-4 certificate.

An otherwise nearly empty four-validator block therefore carries roughly
21 KiB of repeated PQ key/signature material. Transactions also carry
self-contained PQ signatures. Cryptographic signature bytes are intentionally
high entropy and do not compress like ordinary text or repeated structured
fields.

The protocol and historical RPC semantics must remain exact. Archive V2 may
losslessly dictionary repeated public keys and reference them by validator/key
ID inside a segment, then reconstruct the canonical object on read. It must not
drop signatures or weaken verification.

### 4.2 Complete transactions are stored twice

`Block` contains `Vec<Transaction>`. When a block is committed,
`stage_canonical_block_anchor` serializes and stores the complete block. It then
iterates over the same transactions and serializes each complete transaction
again into the transaction column family, plus two index entries.

The independent cold transaction copy currently occupies about 25.98 GiB, or
14.92% of measured cold SST. Archive V2 can store the canonical transaction
body once and have transaction lookup resolve to:

`segment -> frame -> block record -> transaction ordinal`.

Removing this independent full-value copy offers a current upper-bound saving
near that 25.98 GiB before accounting for the compact replacement index and
segment encoding. The exact realized saving must be measured in dual-build
benchmarks.

### 4.3 Mutable LSM overhead is paid for immutable history

Old finalized blocks do not change, but RocksDB must still provide facilities
for mutable key-value workloads:

- write-ahead logging;
- memtable flushes;
- multiple levels;
- table indexes and filters;
- obsolete/overlapping SST files until compaction;
- read and write amplification;
- tombstone processing after hot rows are deleted.

Chronological immutable segments need none of the steady-state rewrite
behavior. They can be built once, hashed, replicated, memory-mapped/read
directly, and deleted only as complete verified objects under policy.

### 4.4 Key order loses chronological locality

Blocks are keyed by block hash in the block column family, while slot-to-hash
cursors are chronological. Hash ordering makes adjacent slots physically
unrelated in the main value store. That reduces opportunities for delta
encoding, compact chronological indexes, bounded range reads, and efficient
range retirement.

Archive V2 organizes a fixed finalized slot range together. Repeated validator
keys, parent relationships, slots, timestamps, and common record structure can
then be represented compactly without changing canonical meaning.

## 5. Code-Path Audit

| Area | Code path | Finding | Effect |
| --- | --- | --- | --- |
| PQ encoding | `core/src/account.rs::PqSignature` | Every signature embeds its full public key; ML-DSA-65 is 1,952 + 3,309 bytes | Large high-entropy payload repeated in blocks and transactions |
| Block model | `core/src/block.rs::Block` and `CommitSignature` | Block includes transactions, producer signature, and commit signatures | Canonical block bodies are necessarily large |
| Canonical write | `core/src/state/ledger_state.rs::stage_canonical_block_anchor` | Stores serialized block, then serializes and stores every transaction again | About 25.98 GiB current cold duplication |
| Cold options | `core/src/state/storage_bootstrap.rs::cold_archival_cf_options` | Zstd is enabled, but with 32 KiB blocks, no explicit level/dictionary, and mutable LSM machinery | Compression exists but is not archival-layout optimization |
| Runtime migration | `core/src/state/cold_storage.rs::migrate_to_cold` | Starts slot iteration at `0u64` on every call; already migrated blocks are discovered by failed hot block lookups | Repeated O(chain height) scan every five minutes |
| Migration writes | `copy_cold_row_checked` | Performs a cold read and individual `put_cf` per missing row | Read/write/WAL amplification and high syscall/API overhead |
| Durability boundary | `migrate_to_cold` | Syncs cold WAL before batched hot deletion | Correct write-first safety, but cold writes are not themselves batched |
| Hot reclaim | runtime `migrate_to_cold` | Deletes hot rows but does not run bounded online reclamation | Tombstoned bytes may remain until normal compaction |
| Maintenance reclaim | `migrate_to_cold_with_bounded_compaction` | Provides stopped-node bounded compaction in hash order | Useful maintenance path, not the recurring runtime path |
| Index migration | `migrate_indexes_to_cold` | Covers five per-slot families only and scans each whole family | Other families remain unbounded; repeated scans grow over time |
| Scheduling | `validator/src/main.rs` cold migration task | Five-minute default; first Tokio interval tick is immediate; every node uses the same cadence | Synchronized startup and periodic I/O pressure |
| Retention | `core/src/state.rs::COLD_RETENTION_SLOTS` | Already 50,000 slots | Hot-window reduction is live; it does not rewrite legacy cold bytes |
| Role selection | `public_archive_network`, `resolve_runtime_cold_store_path`, `configure_archive_mode` | Public testnet/mainnet force automatic cold archive and archive mode | No runtime full/cache/consensus role separation yet |
| Disk guard | `spawn_runtime_disk_guard` | Testnet floor 5 GiB, other production 10 GiB, checked every 30 seconds | Protects RocksDB but a fixed floor does not model workload peaks |
| Checkpoints | checkpoint minimum 20 GiB | Checkpoint work is suppressed at current free space | Recovery features lose operating room before the runtime floor |

### 5.1 Measured scan behavior

On the recovered US validator:

- hot RocksDB opened at 14:22:44 UTC;
- cold RocksDB opened at 14:22:46 UTC;
- the immediate migration completed at 14:23:22 UTC after moving 145 blocks;
- the next periodic pass completed at 14:28:30 UTC after moving 806 blocks.

Because the scheduler's first tick is immediate and the interval is five
minutes, these completion times imply about 35 seconds for the startup pass and
about 43 seconds for the next pass. The work is performed independently by
every validator, with nearly aligned schedules.

The recurring scan is not the old fixed-tip parity scanner and is not CI. It is
normal runtime code. A release or restart makes it more visible because every
node immediately performs the pass after opening its databases.

### 5.2 Correctness properties that must be preserved

The current migration deliberately:

- validates decoded block slot and hash;
- compares an existing cold value and aborts on conflict;
- writes cold before deleting hot;
- flushes the cold WAL before committing hot deletion;
- accepts an identical cold row idempotently;
- refuses to synthesize missing transaction or index data.

The optimized implementation must retain these properties. Performance work
must not weaken source-backed, conflict-aborting, resumable migration.

## 6. Prioritized Remediation

### P0: capacity before code

Do immediately:

1. add or expand writable active storage on all four VPSes;
2. keep active state/archive and read-only provider backups logically separate;
3. alert at 8 GiB free and treat 6 GiB as an active incident;
4. do not lower the 5 GiB floor again;
5. do not run full parity scans, checkpoint builds, package builds, or broad
   manual compaction on the nearly full active filesystem;
6. preserve signed rollback artifacts and all node-owned state.

The future `2 × 960 GB` soft RAID must be capacity-planned by RAID mode:

- RAID1 provides approximately 960 GB usable before filesystem overhead, not
  1.92 TB;
- RAID0 provides capacity but loses the array on one-device failure and is not
  acceptable as the only full archive;
- a full archive server should preferably separate hot mutable state from the
  immutable archive/staging filesystem and retain remote/offline replicas.

Even 960 GB is a longer runway, not infinite retention. Capacity monitoring and
roles remain necessary.

### P1: urgent migration-efficiency release

Implement and release these changes before the complete Archive V2 transition:

1. **Durable high-water cursor**
   - persist the highest slot whose block/transaction rows were fully migrated;
   - bind the cursor to chain/network identity and last migrated block hash;
   - resume from the cursor, not slot zero;
   - on a gap, conflict, reorg outside finality assumptions, or invalid cursor,
     stop and require a bounded audit rather than skipping data.
2. **Bounded startup work**
   - do not run an unbounded full migration pass on the Tokio interval's
     immediate first tick;
   - cap rows/bytes/time per pass;
   - report backlog and schedule the next slice.
3. **Staggered scheduling**
   - derive a stable per-validator jitter from public identity;
   - avoid all validators beginning archival I/O simultaneously;
   - never use jitter in consensus state transitions.
4. **Cold write batches**
   - conflict-check a bounded set;
   - place missing cold values into a `WriteBatch`;
   - commit and sync the cold batch;
   - only then commit the corresponding hot delete batch;
   - persist the cursor in the same safe phase boundary.
5. **Bounded hot reclaim**
   - collect exact affected ranges where key order permits;
   - flush/compact with rate limiting and foreground-consensus priority;
   - never launch a full-CF compaction under low headroom.
6. **Migration telemetry**
   - scanned cursor count;
   - migrated blocks/transactions/bytes;
   - identical/conflicting/missing rows;
   - scan/write/flush/delete duration;
   - backlog slots;
   - physical size and growth per CF;
   - headroom consumed during a pass.

This release reduces recurring I/O, restart latency, and write amplification.
It will not transform the existing approximately 187-189 GB cold RocksDB into
a compact archive and must not be sold as the final capacity solution.

### P2: Archive V2 codec benchmark

Build a read-only benchmark prototype against copied fixed ranges. Test:

- Zstandard levels 3, 6, 9, 12, and 15;
- 1, 4, and 16 MiB seekable frames;
- no dictionary and trained 64-128 KiB dictionaries;
- key dictionaries for validator/transaction public keys;
- transaction stored once with ordinal lookup;
- compact/delta slot, timestamp, and parent references;
- compact rebuildable secondary indexes;
- representative old, recent, busy, sparse, and oversized-block ranges.

Measure:

- bytes per block and transaction;
- compression ratio by record category;
- build CPU/RAM/scratch peak;
- random `getBlock` and `getTransaction` latency;
- sequential sync/replay throughput;
- exact round-trip reconstruction;
- deterministic byte-identical output on independent hosts.

Codec selection is a release decision. No legacy row may be deleted from
benchmark output.

### P3: reader and exact reconstruction

Implement a versioned Archive V2 reader before a writer is authoritative:

- manifest and catalog validation;
- object and frame hash validation;
- segment-root and continuity proof;
- block, transaction, account, event, transfer, and program-call lookup;
- reconstruction of exact current RPC/domain objects;
- quarantining of corrupt objects;
- verified peer/object-store fetch;
- no fallback that returns unverified bytes.

Reader priority during transition:

1. hot state/recent history;
2. legacy cold RocksDB;
3. local Archive V2 segment;
4. verified cache/remote segment only for roles allowed to fetch.

### P4: dual-build and fleet proof

Build V2 segments from finalized legacy history while retaining all legacy
rows. Require:

- deterministic identical segment hashes from independent builders;
- genesis-to-tip manifest/category parity, subject only to the existing
  testnet legacy-loss waiver;
- exact block/transaction round trips;
- fault-injection for crash at every build/promote boundary;
- no capacity-floor violation;
- replication to all required full archives and remote/offline targets.

### P5: canary reads

Enable V2 reads for bounded ranges on one non-critical RPC/canary while keeping
legacy fallback. Compare every response and record discrepancies. Do not move a
consensus validator to a reduced-storage role during this phase.

### P6: V2 primary and bounded legacy retirement

After signed release gates pass:

1. make V2 primary for proven ranges;
2. verify every segment locally and on required replicas;
3. record a signed retirement manifest;
4. retire legacy rows segment by segment, never by broad age deletion;
5. compact only bounded retired ranges with sufficient staging headroom;
6. verify RPC, sync, state root, catalog continuity, and physical `df` after
   each batch;
7. retain the ability to stop and resume idempotently.

### P7: new rollback anchor

The current rollback binaries cannot read a world where required legacy rows
have been retired. Before first deletion:

- release a signed binary that reads both legacy and V2;
- deploy and prove it on all roles;
- designate that release as the new rollback anchor;
- retain its artifacts, detached PQ checksum signature, and restore runbook.

No legacy deletion is permitted while rollback would require the deleted
representation.

### P8: validator roles

Introduce an explicit, versioned role policy only after reader, replication,
and recovery gates pass. Role changes must be deliberate configuration changes,
reported in readiness and peer metadata, and rejected if required data or
capacity is absent.

## 7. Validator Role Model

### 7.1 What every validator must keep

Every consensus-participating validator must independently keep:

- validator identity/key and consensus WAL;
- current deterministic account/contract/validator state;
- state Merkle data required to verify and execute transitions;
- the configured recent replay/reorg window, initially 50,000 slots;
- finalized tip and Archive V2 catalog/root commitments;
- enough recovery/checkpoint material for its role;
- no dependency on another validator's mutable RocksDB.

This is the meaning of preserving state and independent validation. It does not
require every small validator to retain every old block payload locally.

### 7.2 Role matrix

| Capability | Full archive | Verified-cache | Consensus |
| --- | --- | --- | --- |
| Current consensus state | Required, local | Required, local | Required, local |
| Recent 50,000 slots | Required, local | Required, local | Required, local |
| Complete catalog/root commitments | Required | Required | Required |
| Every historical segment payload | Required, local | No; bounded verified cache | No |
| Deep historical RPC | Local | Fetch, verify, cache, then answer | Not advertised |
| Can serve archive segments to peers | Yes | Cached objects only, policy-bound | No |
| Can produce/vote | Yes if configured validator | Yes if configured validator | Yes |
| Safe when remote archive is unavailable | Consensus continues; deep local reads work | Consensus continues; uncached deep reads fail closed | Consensus continues |
| Primary storage objective | durability and complete local history | bounded disk with verified retrieval | consensus availability |

### 7.3 Fleet policy

Initial post-activation policy:

- at least three full archive replicas;
- at least three failure-independent regions;
- at least two independent online storage providers;
- one offline or separately administered backup;
- public RPC routes deep history only to full/archive-capable origins;
- verified-cache nodes may serve a response only after object, segment, and
  catalog verification;
- consensus nodes never claim archive completeness.

For the current four-node fleet, all four remain full legacy archive nodes
until Archive V2 and role activation are signed and proven. A reasonable first
role canary after that is three full archives and one verified-cache node,
provided object storage/offline replication and the outage test pass. Future
agent validators should default to verified-cache or consensus roles according
to their RPC duties and disk quota.

### 7.4 Why not renormalize consensus when a validator disappears

Consensus stake cannot be renormalized from “currently connected” peers.
Different partitions see different peer sets; renormalizing each view could
allow both partitions to believe they have quorum and finalize conflicting
blocks.

With four equal validators, finality needs three signatures. One failure leaves
exactly quorum and no further redundancy. The failed validator's proposer slots
also incur timeout delay. With ten equal validators, finality needs seven;
losing one leaves nine and only that validator's approximate proposer share is
missed. More validators improve failure margin, but archive role is separate
from consensus membership: a consensus-only validator counts fully in BFT
without storing all deep history.

## 8. Adaptive Capacity Policy

A single fixed free-space number is not the final design. Archive V2 must
calculate separate requirements for the hot and archive filesystems.

For hot state:

`required_hot_free = runtime_write_peak + WAL_peak + bounded_compaction_peak
                     + checkpoint_or_snapshot_peak + operating_reserve`

For a full archive:

`required_archive_free = largest_segment_staging_peak
                         + verification_copy_peak
                         + replication_retry_peak
                         + filesystem_reserve`

For verified-cache:

`required_cache_free = bounded_fetch_staging + configured_cache_quota
                       + eviction_margin`

Priority under pressure:

1. preserve consensus state/WAL integrity;
2. stop new segment construction;
3. stop or evict verified cache;
4. stop checkpoint/snapshot work;
5. preserve existing verified archive objects;
6. stop the validator before mutable database safety is lost.

Readiness must expose the calculated components, not only a boolean and a
percentage-used value.

## 9. Acceptance Gates

No storage optimization is complete until it proves:

- formatting, strict Clippy, locked tests, audit, and deny gates;
- standalone contract/genesis/frontend/SDK/deployment QA;
- exact four-validator local test with hot/cold/segment mode;
- fresh full-archive join;
- fresh verified-cache join;
- consensus-role join;
- one-validator outage with unchanged remaining-validator state roots;
- own-state restart and all-validator restart;
- genesis-to-tip public-history parity;
- exact deterministic segment hashes on independent builders;
- corrupt/truncated/wrong-network/wrong-root segment rejection;
- peer/object-store outage without consensus failure;
- cache eviction and re-fetch;
- crash recovery at every build/promote/retire phase;
- installed/running signed artifact parity;
- live author participation from every configured validator;
- capacity and latency targets from the main Archive V2 plan.

The existing testnet waiver for unavailable signed block bodies
`2,872,006..4,298,999` is explicit and non-transferable. Archive V2 cannot
recreate unavailable history. Fresh networks and mainnet fail closed on any
genesis-to-tip gap.

## 10. Rollback

### Temporary cache/swap bridge

If the 1 GiB cache causes unacceptable memory/read behavior after capacity is
expanded:

1. coordinate a consensus stop;
2. restore the preserved pre-bridge env file;
3. recreate the intended 2 GiB swap safely and verify `swapon`;
4. verify sufficient filesystem headroom remains;
5. restart all consensus-critical validators from the same exact boundary;
6. prove artifact hash, state root, tip, authorship, and public RPC.

Do not restore 2 GiB swap while the active filesystem lacks the extra 1 GiB
above all runtime and compaction reserves.

### Migration-efficiency release

The cursor/batch release must keep a legacy full-audit command. If a cursor is
invalid or results differ:

- stop migration, not consensus;
- preserve hot and cold rows;
- run a bounded read-only audit;
- clear/rebuild only the cursor after source-backed proof;
- roll back only to a signed binary that understands all still-authoritative
  representations.

### Archive V2

Before legacy retirement, rollback means disabling V2-primary reads while
legacy remains authoritative. After retirement begins, rollback is only to the
new dual-reader rollback anchor or by restoring verified legacy data from a
preserved backup. The old v0.5.228/v0.5.229 binaries are not valid rollback
targets for a V2-only range.

## 11. Decisions Still Requiring Owner Selection

The architecture does not require these choices before P1, but they must be
selected and documented before role activation:

- primary and secondary object-storage providers;
- offline-backup owner and restore-drill frequency;
- verified-cache default/max quota;
- whether dedicated archive servers use RAID1 plus remote replicas or another
  failure-independent layout;
- exact full-archive placement as the validator fleet grows;
- historical RPC retention/SLA for cache nodes;
- codec/frame/dictionary result selected from benchmark evidence.

## 12. Immediate Next Actions

1. Provision writable capacity before the active margin reaches the 5 GiB
   floor again.
2. Keep the four-validator author and disk monitor running.
3. Open the P1 implementation branch for durable migration cursor, bounded
   batches, staggered scheduling, bounded reclaim, and metrics.
4. Run its full signed release gates; do not patch live binaries.
5. In parallel after capacity exists, build the read-only Archive V2 benchmark
   without deleting legacy rows.
6. Execute P2-P8 in order and use the detailed AV2-001 through AV2-130 backlog
   in the main Archive V2 plan as the authoritative engineering checklist.

## 13. Candidate Follow-Up — 2026-07-27

This section is an additive follow-up. It does not rewrite the signed
`v0.5.229` physical/code baseline or retroactively describe the local candidate
as deployed.

### 13.1 Local implementation

P1 and P2-P8 now have a complete dual-reader local candidate:

- versioned chain-bound migration cursors, bounded scheduling and batching,
  write-before-delete durability recovery, bounded reclaim, per-family metrics,
  and reserve-component readiness;
- deterministic content-addressed Archive V2 format, seekable Zstandard codec,
  transaction deduplication, all public-history category commitments, catalog,
  resumable builder, verified reader/cache, authenticated replication,
  quarantine, retirement journal, roles, adaptive capacity, joins, and
  operations tooling;
- exact local four-validator coverage extended across the real 50,000-slot
  retention boundary, fresh role joins, source/corruption outages, restarts,
  and complete public-history parity.

The clean exact local gate and final workspace Rust gates are still running as
of this update. Detailed status, benchmark evidence, regression evidence, live
capacity, provenance, rollback, and the explicit no-go decision are in
[ARCHIVE_V2_RELEASE_READINESS_2026-07-27.md](ARCHIVE_V2_RELEASE_READINESS_2026-07-27.md).

### 13.2 Live fleet changed after the audit sample

A read-only sample at `2026-07-27T07:37Z` found the VPS testnet stalled at slot
`10,100,391`. US and EU had intentionally failed with startup status 78; SEA
and IN remained active, but two equal-stake validators cannot form the
unchanged three-of-four commit quorum. Writable root free space was only
approximately 5.24 GiB US, 5.23 GiB EU, 5.58 GiB SEA, and 6.05 GiB IN.

This confirms the audit's warning: the 200 GB roots and temporary cache/swap
bridge did not provide durable runway. There is still no safe in-place restart,
Archive V2 build, checkpoint, dual-build, or bounded compaction plan on those
roots. The retained `sdb` provider backups remain recovery assets and cannot be
repurposed without explicit owner authorization plus replacement evidence and
restore proof.

The immediate external prerequisite is enlarged or new writable storage.
Software completion alone cannot restore quorum safely and cannot recreate the
testnet's explicitly waived unavailable legacy block bodies.
