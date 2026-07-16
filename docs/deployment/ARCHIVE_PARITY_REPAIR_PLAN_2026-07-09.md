# Archive Parity Repair Plan - 2026-07-09

**Status:** Public archive storage is an automatic non-dev testnet/mainnet invariant; EU/IN capacity recovery and redundant public RPC ingress are complete; the owner accepted the irrecoverable AP-060 gap for this existing testnet only, while mainnet remains fail-closed. AP-073 four- and ten-validator local release gates pass on the final source. US/EU/SEA/IN are currently healthy on `v0.5.223`; EU still runs the audited exact-tag sparse-maintenance bridge while the other three run the signed rollback artifact. Hosted CI, signed artifacts, complete source-backed fleet-union repair, coordinated activation/deployment, fixed-tip live parity, and publication remain.
**Scope:** Public testnet archive parity, validator resume parity, and release gate
**Rule:** No live VPS rollout until the local four-validator testnet gate proves the repair.

## Problem Statement

The July 2026 public testnet fleet reached consensus with four validators, but
archive data drifted. All validators reported the same current checkpoint slot
and state root, while EU could serve older public history that US, SEA, and IN
could not serve. The observed failure mode was `getBlock` returning `Block not
found` on some validators for old mid-chain slots that another validator could
serve.

That is not acceptable for a public archive testnet. Consensus validity alone is
not enough. Public history must be a replicated, verified data surface.

The next fix is not deleting EU data or shrinking EU until it looks like the
others. EU currently holds public history that the rest of the fleet is missing.
The repair is to replicate and verify that backed history into US, SEA, and IN,
then add a parity check so the fleet cannot silently drift again.

## 2026-07-09 Root Cause Findings

Local four-validator reproduction isolated two archive-parity failures:

1. The genesis state bundle exported only a hand-picked state subset. The genesis
   creator recorded slot-0 explorer history such as `program_calls` during
   contract auto-deploy/init, but joiners imported the bundle and skipped local
   genesis contract replay. Result: joiners had the same state root but missed
   slot-0 public-history rows.
2. Block manifest comparison included local commit-certificate metadata. Blocks
   with the same canonical body and hash can contain different 2/3 quorum
   subsets depending on which precommit certificates a validator collected first.
   Those certificates must remain stored for sync authenticity, but archive
   parity must compare the canonical block body and transaction payloads.

Implemented fixes:

- Genesis bundle export now uses the full `STATE_SNAPSHOT_CATEGORIES` surface,
  so slot-0 public history, current state, indexes, and archive-relevant rows are
  embedded in block 0 for network joiners.
- Canonical block storage/import still preserves commit certificates, while the
  public-history manifest digest normalizes commit certificate metadata out of
  block body comparison.
- The local multi-validator harness now stops all validators before waiting on
  any one validator for offline manifest comparison. This keeps the final
  genesis-to-tip manifest check strict without creating a false one-slot drift
  while only part of the local fleet is still running.

## 2026-07-11 Account Snapshot Retention Regression

A strengthened post-journey check compared all four validators at the same
persisted checkpoint slot instead of comparing moving live tips. At checkpoint
slot `1000`, blocks, transactions, events, contract state, and every other
public-history category matched. `account_snapshots` did not: V2 retained 56
rows while V1/V3/V4 retained 49, with the seven extra rows all at slot `979`.

The cause was destructive and asynchronous. The cold-storage task migrated
blocks and selected indexes, but deleted old account snapshots from the hot DB
without first writing them to cold storage. Each validator ran that task on its
own timer, so archive manifests and historical-balance RPC behavior depended on
local scheduling even when canonical execution was identical.

The candidate fix makes account snapshots a first-class cold public-history
column family. Migration now writes cold first, verifies existing same-key
values byte-for-byte, fsyncs the cold WAL, aborts on conflict, and only then
deletes the hot row.
Historical reads, manifests, snapshot transfer, guarded repair, and source/target
merge all combine hot and cold snapshot rows. Runtime retention no longer calls
the destructive snapshot-prune path, and archive-backed stores reject direct
snapshot pruning. Existing testnet cold DBs created before the account-snapshot
column family gain it on normal writable startup. Read-only repair sources
without that column family remain openable so their backed blocks and indexes
are not discarded.

## 2026-07-09 Live Source Inventory

The July 9 source inventory below was collected while the fleet was producing.
On July 11 the fleet stalled at slot `8,924,000`; EU had already reached ENOSPC
at slot `8,912,000` and is intentionally stopped to avoid another RocksDB
restart loop. Signed `v0.5.223` later advanced US, SEA, and IN to slot
`9,000,624`, where they stalled again on the producer-accounting divergence
recorded below. Those three validators subsequently recovered without a manual
state write and were healthy at common slot `9,005,607` on the July 13
read-only baseline. EU remains intentionally stopped, so the fleet is not yet
four-validator healthy and is still not archive-complete.
Read-only RPC and candidate-admin probes on July 9, 2026 found:

- US (`15.204.229.189`) originally served genesis/current history and recent
  blocks, with first old canonical slot-index row after slot 10,000 at
  `6,715,000`. A guarded raw-block audit then found cold block bodies for
  `4,864,001..5,275,998` with missing hot slot cursors. Those cursors were
  repaired in execute mode from the local raw block bodies; US now serves sample
  blocks in that recovered range.
- EU (`37.59.97.61`) is the richest live source for early history: it serves
  slots `10,000`, `100,000`, `1,000,000`, `2,000,000`, and `2,872,005`.
  Its slot index is contiguous only through `2,872,005`; asking for
  `2,872,006` jumps to `5,276,000`.
- SEA (`15.235.142.253`) has slot-index rows from `4,638,000`, but raw-block
  audit found those are orphan cursors in the missing range: there are no local
  block bodies behind them.
- IN (`148.113.43.247`) originally started backed history at `5,276,000`. A
  guarded raw-block audit found cold block bodies for `4,299,000..5,176,463`
  with missing hot slot cursors. Those cursors were repaired in execute mode
  from the local raw block bodies; IN now serves sample blocks in that recovered
  range.
- All four validators return `Block not found` for `2,872,006` and
  `4,298,999`; current live validators therefore do not contain a backed source
  for the range `2,872,006..4,298,999`. Slot `5,275,999` was absent from the
  live slot indexes, but the July 9 US provider copy subsequently proved and
  preserved its signed body as recorded below.

Evidence:

```text
evidence/archive-parity/testnet-20260709T181442Z
```

The four current VPS filesystems were searched read-only for every mounted disk,
current state/archive directory, checkpoint, and local backup path. The initial
guest inventory showed no extra disk because an earlier operator had removed
EU's provider-attached recovery disk from the guest device table without
detaching it at the provider. The July 13 re-audit below recovered and exhausted
that source. Recovery now operates only on the current July stores and any newly
identified exact provider backup of those stores.

This originally blocked the release gate. A partial EU-to-US/SEA/IN repair would improve
some historical RPC coverage, but it would not satisfy the blockchain-standard
requirement that validators can serve public history from genesis to tip. This
incident has one accepted completion path: locate exact signed block bodies for
`2,872,006..4,298,999`, additively repair that range and the recovered US
singleton into the current chain, and prove every current validator has complete
genesis-to-tip history. Reset, new genesis, placeholder blocks, and synthetic
reconstruction are prohibited.

On 2026-07-15 the owner explicitly accepted the unrecoverable range as a
testnet-only legacy loss so the archive-safety fixes can be deployed to the
chain that exposed the defect. This waiver applies only to the existing
`lichen-testnet-1` history and does not declare the missing bodies repaired.
The range remains unavailable, must stay visible in health/incident records,
and must never be synthesized. It does not waive any mainnet gate: a fresh
mainnet must establish and continuously advance a verified genesis-to-tip
archive proof, and startup/join/snapshot verification must fail closed before
consensus whenever that proof is absent, behind tip, hash-inconsistent, or
cannot be independently replayed through complete signed bodies and indexes.

## 2026-07-11 Capacity And Restart Incident

EU's 193 GiB filesystem contained approximately 106 GiB of hot state and 68 GiB
of cold archive. Its slot-8,911,000 checkpoint was mostly hard links and only
about 3 GiB unique, so checkpoint cleanup was not the capacity fix. US, SEA, and
IN also had materially different hot block sizes, which was further evidence of
archive-layout drift rather than acceptable node specialization.

The initial incident response conservatively required 500 GiB before recovery.
The July 14 raw-CF audit supersedes that conclusion: a point-lookup-optimized
RocksDB iterator had falsely hidden 68.64 GB of migration-eligible hot history.
The corrected, bounded migration writes and WAL-syncs small cold batches before
deleting hot copies, then compacts only each processed hash range. The current
200 GB EU filesystem is therefore usable for in-place layout repair; a fixed
500 GiB expansion is not a prerequisite for this recovery.

Before public-history replication or candidate deployment:

1. Measure each validator's hot/cold rows and bytes with total-order raw scans;
   capacity approval must include the bounded write/compaction peak and the
   post-repair runtime floor rather than an arbitrary disk-size constant.
2. Preserve EU's canonical hot/cold data and keys; do not delete history to make
   its row counts resemble incomplete peers.
3. Restore production only after EU passes the corrected integrity audit and
   bounded migration, then all four hosts pass storage and fixed-tip parity
   verification.
4. Keep EU stopped while it has less than the candidate's 10 GiB runtime floor.

The candidate skips checkpoint creation below 20 GiB free, exits with persistent
safety status 78 below 10 GiB, and uses one hosted supervisor. Systemd will not
restart status 78, preventing another ENOSPC reopen loop. These safeguards stop
corruption; capacity expansion and archive repair remain mandatory.

Final read-only preflight on 2026-07-11 confirmed the blocker is still active:
US, SEA, and IN report `behind` at slot `8,924,000`; EU remains inactive with
about 3.4 GiB free. Approximate free space is 59 GiB on US, 53 GiB on SEA, and
32 GiB on IN, and each host has only about 193 GiB total. All installed
validator binaries still report `0.5.222`; no candidate was installed.

## 2026-07-12 Current-Chain Recovery Tracker

Only the current July 12 hot and cold stores are recovery targets. Source
inventory work does not redefine an older preserved directory as current chain
state. Every live store, key, identity, WAL, and archive remains in place until
provider-level backups are recorded and the additive repair is complete.

Measured `/var/lib/lichen` usage is:

| Region | Host | Total bytes used | Hot state | Cold archive | Free filesystem space |
| --- | --- | ---: | ---: | ---: | ---: |
| US | `15.204.229.189` | 125,506,936,832 | 36,166,574,080 | 88,404,209,664 | about 63.5 GB |
| EU | `37.59.97.61` | 183,226,740,736 | 110,371,000,320 | 72,672,870,400 | about 3.6 GB |
| SEA | `15.235.142.253` | 134,964,514,816 | 85,685,690,368 | 49,147,224,064 | about 55.8 GB |
| IN | `148.113.43.247` | 158,489,583,616 | 84,616,568,832 | 73,862,070,272 | about 33.3 GB |

Provider instance UUIDs for backup and resize tracking are US
`bd74bcf5-0918-4680-a600-285a9e6a1ab4`, EU
`12860ef2-b347-47e2-8022-c54880f33f6f`, SEA
`b1ccc42a-f6fe-48ad-a6cc-53b8e57d1a50`, and IN
`3411a393-f8cd-4627-b686-5f6ccbe3ef7f`.

Each VPS has one approximately 200 GB OpenStack root disk, no LVM, no second
disk, and no unused partition capacity. Log cleanup cannot provide the union
space or RocksDB compaction headroom. The current recovery must use the existing
200 GB roots: first preserve the stopped EU disk in the provider backup, then
move byte-identical public history from hot state to its existing cold archive
with the guarded maintenance command below. Measure the resulting runtime floor
instead of assuming a larger provider disk is required. If EU remains below the
10 GiB safety floor after that verified migration and scoped compaction, storage
expansion becomes a release blocker; do not delete chain data, keys, identities,
WAL, or archives to make the repair fit.

The signed rollback anchor is `v0.5.223`. At the 2026-07-09 inventory, the
installed validator executables reported `0.5.222` and had region-dependent
hashes; that inconsistency was subsequently repaired as recorded in the
2026-07-12 baseline below. An earlier `0.5.224` candidate was staged identically
on all four hosts at `/tmp/lichen-validator-0.5.224-candidate`, SHA-256
`bba68bc4fb89d837be75598912560182c0f7d4eae37e1c0c1170fcfb4fba291a`.
That staged binary is superseded, is not the final locally tested candidate,
and must not be installed. At this inventory US, SEA, and IN were stalled at
slot `8,927,000`; EU remained stopped below the runtime free-space floor. No
service is to be restarted until storage and the offline repair gates pass.

The current source union proves:

- EU: genesis through `2,872,005`, and `5,276,000` through its retained tip.
- IN: `4,299,000..5,176,463`.
- US: `4,864,001..5,275,998`.
- US July 9 provider copy: a decoded, hash-matching raw body for slot
  `5,275,999` whose hot slot cursor was missing. A temporary hot-DB working copy
  now contains the deterministic cursor; the provider copy remains read-only.
- SEA: no additional backed body range inside the unresolved interval.

The exact signed bodies for `2,872,006..4,298,999` remain the hard blocker.
Slot `5,275,999` has been exported from the verified US raw body after linking
its deterministic cursor in the temporary working copy. Its block hash is
`f5c43786e569601ec91f7a62a97dcfeeddad5cbbcd9534c4807e8afc7a067ccf`;
the block contains zero transactions, a zero transaction root, and three commit
signatures. The canonical block page, slot page, decoded block JSON, and
contiguous-range proof are preserved with SHA-256 hashes under
`evidence/archive-parity/testnet-20260712-current-readonly/us-recovered-slot-5275999`.
It still must be imported additively on all four stopped validators after the
fleet backup and fixed-tip conflict gates pass. Index rows and slot cursors are
not block bodies and cannot be used to manufacture the unresolved range.

## 2026-07-13 Repeated Producer-Accounting Stall

After one legacy producer-counter repair let signed `v0.5.223` advance 222
blocks, US, SEA, and IN stalled again at the same canonical tip, slot
`9,000,624`, block hash
`c3dd5fabea98af4faf3887be457e31f9de1dc92e848edc6893f8fae4a1cb258d`.
The signed header state root is
`97c6c2d4082da443bf693e5df0dd976b40017b180aa54816767cc6ea4386e06c`.
SEA and IN agree on stake-pool hash
`09eae4e439ec8a61656ae0b33387e9d3f04e59877b8948e8cdde647866ba1eda`;
US has
`b868875e558d6410b14fef9afffce6f75e8edcac617e25267a1b329ba0981962`.

The exact differing row belongs to the US producer of parent block
`9,000,623`. SEA and IN record `last_reward_slot=9000623` and
`blocks_produced=2525506`; US records `last_reward_slot=9000622` and
`blocks_produced=2525505`. The next proposal therefore computes different roots
on the two sides and cannot receive a valid quorum. P2P is connected and all
three nodes have the same canonical tip; this is deterministic post-block state
drift, not a peer-discovery or timeout incident.

This second occurrence proves that repeated operator counter repair on
`v0.5.223` is not an acceptable recovery. The candidate binds both the
producer-effect marker and the comprehensive post-effects marker to the
canonical block hash, audits a bounded recent window at startup, and runs the
same parent-completion gate before BFT reads or proposes the next height. The
regressions `startup_recent_recovery_completes_parent_stake_pool_effects` and
`bft_post_effect_gate_repairs_stale_parent_before_next_height` prove exact-once
repair and a no-op second pass. Live evidence is under
`evidence/post-block-effects-recovery/testnet-20260713T-live`.

## 2026-07-13 Detached Backup Re-Audit

EU kernel history showed that a 201 GiB `/dev/sdb` recovery disk was attached
through July 9, then removed only from the running guest through
`/sys/block/sdb/device/delete`. A read-only SCSI rescan recovered the still
provider-attached volume without requiring a provider mutation:

- provider volume serial: `9878038e-a359-4b03-ae00-8ddd76be2c57`;
- root filesystem UUID: `ee3ba3cb-0144-4a70-9e09-35216f42bc9a`;
- mount: `/mnt/ovh-backup-20260607T2158`, `ro,norecovery`;
- captured EU instance UUID: `12860ef2-b347-47e2-8022-c54880f33f6f`;
- captured RocksDB: 57,719,127,390 bytes plus a 640,930,044-byte WAL;
- canonical tip: slot `2,872,005`, hash
  `74e23fbbf02a56763497ada2c40606b94f6a24504764926adc1e40d080c7bd84`.

The candidate's secondary/read-only block verifier confirms slots
`2,872,003..2,872,005` and rejects `2,872,006` as a missing canonical body. The
new checksum-verifying `tools/rocksdb_wal_inventory.py` parser audited all 25
logical batches in the captured 612 MiB WAL. They contain 7,819,087 state/index
operations but zero `blocks` or `slots` column-family operations, so the WAL
cannot extend the canonical range. SCSI rescans on US, SEA, and IN found no
hidden provider-attached disks. Read-only ext4 deleted-inode inventories found
no recoverable large SST inode on EU, US, or SEA and no applicable inode on IN.

The detached disk therefore independently proves the existing EU range but does
not fill any part of `2,872,006..4,298,999` or slot `5,275,999`. Keep it mounted
read-only until the incident is closed; do not reuse its free space because that
would overwrite potential filesystem evidence.

OVH's authenticated VPS API subsequently confirmed that the detached disk is
the EU automated-backup restore point from `2026-06-07T21:58:37Z`, not an
unmanaged volume. The current provider inventory is:

| Region | OVH service | Provider zone | Restore point under test | Provider state |
| --- | --- | --- | --- | --- |
| US | `vps-cdb47b12.vps.ovh.us` | `os-us-east-va-2` | `2026-07-09T14:48:36Z` | task `3212186` done; attached and mounted read-only |
| EU | `vps-210edd4a.vps.ovh.net` | `os-gra6` | `2026-07-12T23:16:38Z` | task `82572062` done; attached and mounted read-only |
| SEA | `vps-df7100d5.vps.ovh.ca` | `os-sgp2` | `2026-07-12T15:51:07Z` | task `82559886` done; attached and mounted read-only |
| IN | `vps-8709ee62.vps.ovh.ca` | `os-ap-south-mum-vps-1` | `2026-07-12T17:34:45Z` | task `82559902` done; attached and mounted read-only |

All four automated-backup services are enabled with rotation `1`. Every current
restore point is now attached in `file` mode and mounted `ro,norecovery`; the
guest block-device read-only flag is also enforced. This table records provider
availability only; it does not claim that a restore point contains either
unresolved canonical range. Run the same canonical-range, raw-body, and WAL
inventory before authorizing any additive merge. Never invoke a `full` restore
against a validator. The temporary VPS-scoped credentials used for this
inventory are not validator runtime credentials and must not be copied to any
VPS.

The July 14 credential renewal reconfirmed the provider ownership boundary. The
OVH US account exposes one VPS, US, through `api.us.ovhcloud.com`; the OVH EU
account exposes three VPSes whose IPs match EU, SEA, and IN through
`eu.api.ovh.com`. Read-only automated-backup metadata is accessible for all
four. A `/cloud/project/...` permission is not proof of access to this fleet:
renewal must verify the expected `/vps` count, region IP mapping, and
`/vps/{service}/automatedBackup` read before recovery work continues.

The IN file copy completed at 2026-07-13 11:49 UTC. Its provider disk serial is
`bb844f8f-c115-4d22-9ab8-b3005daab835`; root filesystem UUID
`b46b0591-3e69-4fea-8d7c-85fc7e99ff94` is mounted at
`/mnt/ovh-backup-20260712T1734` with `ro,norecovery`, and the guest block-device
read-only flag is also set. The copy contains 84,240,052,224 bytes of hot state,
73,863,106,560 bytes of cold archive, and a checkpoint at slot `8,951,000`.
Read-only canonical-body probes prove slot `0`, `4,299,000..5,176,463`, and
`5,276,001` onward at the sampled boundaries, but reject `2,872,005`,
`2,872,006`, `4,298,999`, `5,176,464`, `5,275,998`, and `5,275,999` as missing;
slot `5,276,000` is header-only. The checkpoint view gives the same results.
The copy's only nontrivial WAL is 45,440,572 bytes; checksum-verified parsing
finds 333 complete canonical blocks and slot rows exclusively in
`8,951,001..8,951,333`. IN therefore preserves a current July filesystem but
does not source either unresolved canonical-body gap.

The US file copy completed at 2026-07-13 12:11 UTC. Its provider disk serial is
`02c3ed25-5866-4336-9bbf-f5a7cd2d6a84`; root filesystem UUID
`724880aa-006c-48e0-a068-1a059c4e2153` is mounted at
`/mnt/ovh-backup-20260709T1448` with `ro,norecovery`, and the guest block-device
read-only flag is also set. It contains 14,750,957,568 bytes of hot state,
81,887,027,200 bytes of cold archive, and a checkpoint at slot `8,525,000`.
Ordinary current/checkpoint probes do not resolve either incident gap, and its
hot WAL contains only `8,525,001..8,525,115`. A raw-body dry-run independently
decoded all 3,528,421 hot/cold block rows with zero decode, hash, duplicate, or
cursor conflict errors. It found no body in `2,872,006..4,298,999`, but found
exactly one body at `5,275,999`; the missing slot cursor is deterministically
repairable. The body is in cold storage, not the hot DB. The provider copy
remains unchanged while a 14,750,380,032-byte temporary hot-DB working copy at
`/var/tmp/lichen-recovery-us-20260709-state` is used to add only that cursor and
export the exact body.

The SEA file copy completed and is mounted at
`/mnt/ovh-backup-20260712T1551` with both block-device and filesystem read-only
enforcement. It contains 86,216,335,360 bytes of hot state, 49,147,424,768
bytes of cold archive, and a checkpoint at slot `8,947,000`. Ordinary current
and checkpoint probes reject the sampled incident-gap boundaries. A raw-body
dry-run decoded all 3,555,378 hot/cold block rows with zero decode, hash,
duplicate, or cursor-conflict errors and found no body in
`2,872,006..4,298,999`. The exact `5,275,999` singleton scan also found no
body: SEA retains one orphan slot cursor for that slot, with no hash-matching
hot or cold block row behind it. It is not a reconstruction source.

The EU July 12 file copy completed and is mounted at
`/mnt/ovh-backup-20260712T2316` with both block-device and filesystem read-only
enforcement. Its filesystem UUID is
`802dfb8e-58e0-4afe-8039-c0a15c6a06a8`. It contains 113,879,437,312 bytes of
active hot state, 72,670,900,224 bytes of cold archive, a checkpoint at slot
`8,915,000`, and the preserved pre-apply rollback checkpoint at slot
`8,915,275`. Boundary probes reproduce EU's known contiguous range through
`2,872,005`, the gap at `2,872,006`, the header-only `5,276,000`, and valid
history from `5,276,001` onward. Its active WAL is checksum-valid and contains
49 batches with 490,000 deletes only in column family 25; it contains no block
put/delete and no recoverable body. A read-only raw-body scan decoded all
6,510,346 rollback-hot-plus-cold block rows with zero decode, hash, duplicate,
or cursor-conflict errors and found zero bodies in
`2,872,006..4,298,999`. Evidence SHA-256 is
`7269dbbf7b07534db12e961ccd56183b6e840054f8e66980ef06348adae72a6e`.
The separate `5,275,999` singleton raw scan also completed. It decoded the same
6,510,346 rows, found zero matching bodies, and reported zero decode, hash,
duplicate, or cursor-conflict errors. Evidence SHA-256 is
`c50c99a0984fdc24e88c6442717d5ac6e655800d3b33f454327c227e4cbffd9e`.

## 2026-07-12 Preservation Baseline And Recovery Order

No destructive recovery is authorized. EU must remain stopped until its current
provider backup is attached and verified and the guarded migration restores the
measured runtime floor. Do not remove or
replace its hot state, cold archive, snapshot rollback marker, validator
identity, keypair, WAL, or genesis configuration.

The fleet currently demonstrates why consensus parity is not archive parity.
US, SEA, and IN agree on current finalized block hashes and state roots and can
therefore vote together, but representative local RPC reads on all three return
`Block not found` for slots `10,000`, `1,000,000`, `2,872,005`, `2,872,006`,
`4,298,999`, and `5,275,999`. Reaching the current state root did not require
old releases to retain every historical block body, transaction/index row, or
account snapshot. A validator must not be reset on the assumption that peers
can restore history until the fixed-tip public-history gate proves that source.

All four hosts now have the same installed signed rollback binary and canonical
genesis descriptor:

- validator `v0.5.223` SHA-256
  `e956a8fb039745e132ecd4e5232c6fdef011d154d98efa65d7ba30fcece2e810`
- `state-testnet/genesis.json` SHA-256
  `5614768f34073910f79be9c2d9b2d449ceff7e6ccf3d10a30ea98e3e7d83e4b5`
- superseded staged, never installed `0.5.224` candidate SHA-256
  `bba68bc4fb89d837be75598912560182c0f7d4eae37e1c0c1170fcfb4fba291a`

Every recorded `0.5.224` hash above is retained only as incident provenance and
is superseded. The release source also hardens snapshot crash rollback, the
block-hash-bound parent post-effects gate, and sparse-state serialization and
startup verification, then passed the authoritative local four-validator run
recorded under AP-050. The later no-cache `linux/amd64` candidate had SHA-256
`6b5f79d16654c02990c2c9b40e4ca8656a29a5106048e1872b72fcac9ca62325`,
platform image manifest
`sha256:16817fc0f115ec07e71069952de93c2d93b1e484719f1a7a6be85ddc0826cf57`,
manifest-list digest
`sha256:6ec8c0b312047007e79353e1f1160b14c6a9887053e25fea9a4c5258d329a310`,
and builder platform manifest
`sha256:7dd6ac14638d223be9f47eb60634df930e8e93e9b8110b07fce107274ba7964a`.
It is also superseded by AP-057Q's total-order and measured import-preflight
changes. The final exact Linux hash is pending repeat full gates and rebuild.
No `0.5.224` candidate has been installed on the fleet.

EU was inactive with approximately 35 MB available on its only approximately
200 GB root disk when preservation began; scoped non-chain cleanup later raised
that to 5.3 GiB. While OVH task `82572062` began materializing the July 12
file copy, the provider rebooted EU and systemd automatically re-enabled the
validator. Signed `v0.5.223` then retried the incomplete live snapshot apply,
repeatedly appended RocksDB WAL data, and failed with `ENOSPC`. The service was
stopped and disabled before any further provider reboot; it must remain disabled
until the provider copy is fully audited, then be explicitly re-enabled only
after stopped maintenance and recovery gates pass. Only archived system
journals were vacuumed, reclaiming about 48 MB; no chain, identity, key, WAL,
rollback, or archive file was removed. Its durable live-apply marker records rollback slot
`8,915,275`, rollback root
`cbf7770fcb1e6a873bf0459e078692d5cafbbc41b040b31f1824edb68a203d3a`,
target slot `8,927,000`, and target root
`48aec1905275b23987a30d36eea546258b19d2dd6fefd7c5c58a58a3af073b96`.
This checkpoint was created by RocksDB's native checkpoint API during the July
12 live snapshot apply; it is not the earlier mounted OVH restore volume.

Before the first guarded restore attempt, the rollback checkpoint had
108,645,629,952 apparent bytes but only 20,508,672 bytes exclusive to that
directory; its immutable SSTs were otherwise hard-linked to the incomplete
current target. The old diagnostic path then rewrote derived sparse files before
the guard rejected the root and reversed the swap. Restoring the exact rollback
from the read-only provider copy changed that physical allocation: the rollback
now has 108,645,474,304 allocated bytes when counted first, and 3,333,300,224
bytes are exclusive when the incomplete target is counted first. Conversely,
the incomplete target has 8,925,753,344 bytes not already charged to the restored
rollback, including approximately 135 MB of sidecars that the recovery script
preserves. The current exact free space is 2,374,594,560 bytes. Retiring the
provider-preserved incomplete target after sidecar transfer is therefore
expected to leave only about 11.17 GB, roughly 0.43 GB above the 10 GiB runtime
floor. The script must use its measured post-delete check, and unrelated safe
cleanup must increase the catch-up margin before service start.

The July 13 stopped-DB RocksDB manifest audit identifies the current EU hot
footprint rather than inferring it from directory size. The largest live column
families are `blocks` at 59,868,978,592 bytes, `contract_merkle_nodes` at
38,546,622,353 bytes, `transactions` at 7,552,370,965 bytes, and `slots` at
840,000,527 bytes. Contract Merkle state and slot authority remain hot. Old
block and transaction bodies are intended to move to the attached cold archive;
EU was stopped during its post-snapshot compaction before that migration could
remove the restored hot copies.

## 2026-07-14 Sparse Merkle Cache Retention Incident

The corrected hot-to-cold migration completed with all integrity counters at
zero and raised EU free space from 10.94 GB to 20.79 GB. Controlled full replay
then advanced EU from slot `8,918,xxx` to persisted slot `8,953,695`, but free
space fell to 15.15 GB. The service stopped cleanly with status 0 and no systemd
restart before reaching the runtime floor.

This second capacity loss was not block, transaction, account, contract, or
archive growth. RocksDB properties measured EU's `contract_merkle_nodes` at
42,022,557,879 SST bytes and 37,666,222,957 estimated live bytes while canonical
`contract_storage` was only 25,463,366 SST bytes. The same derived column family
had already diverged materially across the fleet: approximately 36.8 GB on US,
42.0 GB on EU, 2.0 GB on SEA, and 48.6 GB on IN. Validators therefore held the
same current state with host-dependent amounts of unreachable sparse-cache
history.

Root cause: sparse account and contract updates wrote every new
content-addressed path node but never deleted the superseded path nodes. Those
old node records are not historical contract storage and cannot answer an old
state query by themselves; they contain hashes and current-leaf references,
while the canonical account and contract column families remain authoritative.
Native RocksDB checkpoints legitimately keep their point-in-time node files
alive through hard links until checkpoint retention prunes them.

The production fix is two-part:

1. Canonical sparse-root updates track prior rooted nodes replaced by each
   change and delete them in the same RocksDB `WriteBatch` that writes the
   replacement nodes, leaf-cache entries, dirty-marker cleanup, and new root.
   Speculative proposal-root computation remains read-only.
2. The stopped-node rebuild clears only the derived node/leaf cache column
   families with a bounded RocksDB range tombstone, flushes and compacts that
   exact range, and reconstructs both sparse trees from canonical accounts and
   contract storage. It never deletes chain history or canonical state.

Regressions repeatedly update account and contract paths, require physical
node-row count to equal the freshly verified reachable-node count, verify a
current account proof, and prove a checkpoint created before live-node
retirement can still resolve its prior sparse root. Current candidate Core
tests pass `994/994`. The exact `v0.5.223` recovery bridge carries the same
ongoing node retirement plus the bounded one-time rebuild so EU cannot regrow
the cache while replaying. It is staged separately and does not replace the
installed signed rollback binary.

The exact-tag bridge was built from tag commit
`fa4a7d3d2b24ecaaf349441f7b644065f860affc` plus the four-file audited recovery
patch. Optimized Linux tests for range clearing, ongoing node retirement,
incremental-root equivalence, cold-schema compatibility, and forced full-replay
selection passed. The stripped Linux x86-64 binary is still version `0.5.223`
and has SHA-256
`9b71e7a9a95a928af58721c71dbd69c6ff584769addc9c2bded9aed21a46ccee`.
It remains a staged recovery artifact; `/usr/local/bin/lichen-validator` is the
unchanged signed rollback binary with SHA-256
`e956a8fb039745e132ecd4e5232c6fdef011d154d98efa65d7ba30fcece2e810`.

EU's stopped rebuild completed successfully on 2026-07-14 at persisted slot
`8,953,695`. The contract-node column family fell from `42,022,557,879` to
`246,455,989` SST bytes, and the account-node family measured 17,464 SST bytes.
Computed and stored account/contract roots match, the typed show command exits
zero, and identity, genesis, signer, validator, and cold-archive evidence is
unchanged. Replay then crossed slot `8,954,000`, created a new checkpoint,
pruned the old hard-linked checkpoint under the 8 GiB size cap, and raised free
space to `58,719,334,400` bytes. Catch-up continues with zero systemd restarts;
fixed-tip and sustained-runtime checks remain open.

EU execution order is fail closed: keep the service stopped and boot-disabled;
record slot, root, canonical CF sizes, checkpoint, binary hash, and free space;
run the typed sparse rebuild as the `lichen` user; require computed/stored root
verification and unchanged slot/identity/genesis/archive evidence; resume full
replay with restarts disabled; cross the next 1,000-slot checkpoint boundary;
and require normal checkpoint size-cap pruning to release the old hard-linked
cache files. Stop immediately if free space approaches 10 GiB or any root,
RocksDB, or replay error appears.

The candidate provides a stopped-node maintenance command for this condition.
Dry-run opens both databases read-only and audits every candidate block,
transaction, and transaction-to-slot row as identical, missing, or conflicting
in cold storage. It validates decoded block hashes, canonical slot cursors,
every block-referenced transaction body, and each exact transaction-to-slot
value before execute mode can open either database for writes.

The first EU dry-run at fixed cutoff `8,815,275` was invalid evidence. It used a
default iterator on a point-lookup-optimized column family, stopped after the
3,957,007 cold-backed segment, and falsely reported zero old hot rows. A raw
total-order audit then decoded 6,513,019 hot-plus-cold block rows and proved
6,410,345 canonical body/cursor pairs: 2,556,012 hot and 3,957,007 cold, with
zero decode, hash, duplicate, orphan, or cursor errors.

The corrected total-order cold-migration dry-run found the real retained-window
layout below cutoff:

- 2,453,338 hot blocks, 60,658,298,656 bytes, all missing from cold
- 1,467,110 hot transaction rows, 7,965,048,674 bytes, all missing from cold
- 1,467,110 hot `tx_to_slot` rows, 11,736,880 bytes, all missing from cold
- zero decode errors, hash mismatches, missing/conflicting block cursors, or
  hot/cold byte conflicts

The report SHA-256 is
`6239c5785344a57114389f2c47629ceafb2b722af530735104ed6b4b47a21bff`.
The source report predates the additional block-referenced transaction presence
checks. The corrected exact binary repeated the read-only audit with every new
integrity counter at zero, then execute migrated all 2,453,338 eligible blocks
in 246 bounded compaction batches. Available space rose from 10.94 GB to
20.79 GB. A post-migration dry run reported zero eligible rows and zero errors;
the final total-order raw audit again decoded 6,513,019 hot-plus-cold rows and
proved 6,410,345 canonical body/cursor pairs with every integrity counter zero.

```bash
# Run only with the validator stopped. Use the fixed retained-window cutoff
# recorded from the preserved tip; do not use a moving live tip.
sudo -u lichen /tmp/lichen-validator-0.5.224-candidate \
  --network testnet \
  --db-path /var/lib/lichen/state-testnet \
  --cache-size-mb 256 \
  --migrate-public-history-to-cold \
  --to-slot CUTOFF_SLOT

# Execute only when the exact final dry-run reports zero integrity/conflict rows,
# after the current
# provider backup is attached in file mode, mounted ro,norecovery, and verified,
# and after every multiply-linked checkpoint is provider-preserved and retired.
sudo -u lichen /tmp/lichen-validator-0.5.224-candidate \
  --network testnet \
  --db-path /var/lib/lichen/state-testnet \
  --cache-size-mb 256 \
  --migrate-public-history-to-cold \
  --to-slot CUTOFF_SLOT \
  --execute \
  --confirm public-history-cold-migration:v1
```

This command is a storage-layout repair, not history deletion: hot rows are
removed only after their exact bytes are durable in the local cold archive.
The verified EU execution WAL-synced at most 1,000
copied blocks before each hot deletion and flushes/compacts each bounded 10,000
block hash range before proceeding, so it does not duplicate the entire 68.64
GB migration set at once. EU also uses the
fail-closed incident script at
`evidence/post-block-effects-recovery/testnet-20260713T-live/eu-restore-own-snapshot-rollback.sh`.
The script requires the exact live/provider marker hash, read-only backup mount,
stopped and disabled service, final candidate hash, same-filesystem checkpoint,
slot `8,915,275`, state root
`cbf7770fcb1e6a873bf0459e078692d5cafbbc41b040b31f1824edb68a203d3a`,
and unchanged validator sidecars. It atomically makes EU's own checkpoint live,
retires the provider-preserved incomplete target only after verification, and
leaves the validator stopped.

The guarded restore has now completed successfully. The active EU database is
the verified rollback at slot `8,915,275` and root
`cbf7770fcb1e6a873bf0459e078692d5cafbbc41b040b31f1824edb68a203d3a`;
all protected sidecar hashes matched before and after the swap, the incomplete
target and rollback marker were retired only after verification, and EU remains
stopped and boot-disabled. The provider-restored checkpoint contained 21
root-owned files (20 SSTs and `known-peers.json`). Ordinary signed `v0.5.223`
selected warp sync for the large gap, and its internal supervisor retried after
the hard-link protection rejected those files even though systemd had
`Restart=no`. No new rollback marker or corruption was produced. Ownership was
normalized to the `lichen` service account with byte-for-byte hashes unchanged,
and the guard now performs and verifies that normalization before startup.

The only tested catch-up bridge is an exact-tag `v0.5.223` recovery build whose
sole source delta makes `LICHEN_FORCE_FULL_REPLAY=1` select full replay instead
of warp sync. Its stripped Linux x86-64 SHA-256 is
`df815f513b11dd64b9abf5b51e61da0bd7b918f0fddfc4641bae0c329e8f98bf`;
the focused release-mode selection test passed, and the signed installed
`v0.5.223` binary remains unchanged. A controlled run advanced about 1,650
slots with no snapshot marker, staging directory, crash, or systemd restart,
but reduced free space to within approximately 40 MB of the 10 GiB stop floor.
The run was stopped, its generated checkpoint was inventoried and removed, and
EU was left inactive and disabled. This proved replay was unsafe before the
hidden old hot copies were migrated; it did not prove the 200 GB volume itself
was insufficient. After the corrected migration, a second controlled replay
reached persisted slot `8,953,695` before the independent unbounded sparse-cache
retention bug recorded above consumed approximately 5.6 GB. EU again stopped
cleanly with no systemd restart. No chain data was removed to make space: every
migrated byte remains in EU's attached cold archive, and AP-057R rebuilds only
derived sparse caches before replay resumes.

Mandatory execution order and current status:

1. Done: preserve provider copies read-only, complete the guarded rollback,
   normalize ownership with unchanged hashes, and verify no marker/staging
   residue.
2. Done: repeat the corrected integrity audit, execute the bounded write-first
   hot-to-cold migration, and prove the post-migration dry run and full raw audit
   are clean.
3. In progress: the exact-tag maintenance bridge rebuilt only the derived sparse
   caches, roots and unchanged canonical evidence passed, and the next
   checkpoint released the retired cache files. Catch EU up with the same
   tested full-replay-compatible `v0.5.223` bridge,
   `LICHEN_FORCE_FULL_REPLAY=1`, systemd `Restart=no`, internal restarts disabled,
   and continuous slot/disk/marker monitoring. Do not start ordinary signed
   `v0.5.223`; it selects the defective warp path for this gap. Do not start
   candidate `0.5.224` before coordinated activation.
4. Stop EU after catch-up and prove current slot/hash/state-root parity. Remove
   the runtime-only recovery override before coordinated candidate activation.
5. Stop the fleet at one fixed tip and compute complete hot/cold manifests.
6. Replicate only exact source-backed history through additive, idempotent,
   conflict-aborting repair.
7. Locate exact backed bodies for every fleet-wide source gap and rerun the
   fixed-tip genesis-to-tip gate.
8. Only after provider backups and four-node parity, prove one identity-preserved
   empty-state rejoin from peers; retain the old database until that proof
   passes.
9. Complete the restart/configuration/offline-proposer/capacity fixes and rerun
   four- and ten-validator release gates before tagging or deployment.

### No-Provider-Capacity Alternative Feasibility

A July 12 read-only export measurement opened EU's stopped rollback checkpoint
through RocksDB's read-only API and attached the live cold archive read-only.
It streamed EU's verified `0..2,872,005` block range through the candidate's
binary framed public-history encoder into `wc -c`; it did not create a
secondary database or write an export file. The resulting block-body stream was
54,398,937,036 bytes.

This range alone exceeds IN's approximately 34.2 GB free space and would leave
only about 2 GB on SEA or 9 GB on US before transaction, address, event, trade,
snapshot, WAL, and compaction allocations. Splitting the range across hosts
would preserve fragments but would not produce independently complete archive
validators and is prohibited as a release or mainnet design. Copying raw SSTs,
mounting another validator's live database over the network, or deleting EU's
source before replication are also prohibited.

That July 12 measurement did not account for the later bounded hot-to-cold
migration or tens of gigabytes of unreachable sparse-cache nodes on US, EU, and
IN. The current 200 GB filesystems are therefore not declared insufficient.
Before any backed-history import, each target must complete its own verified
hot/cold migration and stopped sparse-cache rebuild, then pass the measured
missing-byte plus compaction and runtime-reserve preflight. Storage expansion is
required only if that post-reclamation measurement still fails; nominal disk
size alone is not a blocker or an approval.

### Candidate Hardening After The July 12 Incident

The unreleased `0.5.224` worktree now includes these fail-closed changes. They
remain local until the complete release gate passes:

- Snapshot live apply measures the verified staging database's allocated bytes
  before creating a rollback checkpoint or clearing a live category. Available
  space must cover one replacement copy, one RocksDB compaction copy, and the
  10 GiB runtime reserve. A failed proof abandons the staged snapshot without
  touching live state.
- Snapshot completion now requires both the expected slot/state root and a full
  genesis-to-target contiguous public-history proof. A root-valid target with
  missing history is incomplete and must roll back. The local rollback profile
  contains every exact pre-apply hot category, including public-history and
  account-transaction counters, while deliberately leaving the independent
  cold archive untouched. Recovery restores that complete hot profile, writes
  and fsyncs the recovered checkpoint, and removes the durable recovery marker
  last; cleanup failure is fatal.
- Public-validator startup treats the complete `GenesisConfig` embedded in slot
  0 as authoritative. A drifted cached `state-*/genesis.json` is atomically and
  durably repaired from slot 0; an explicitly supplied conflicting file is a
  fatal error. Public startup also fails if an existing slot-0 block does not
  contain the embedded config.
- BFT restart no longer derives an unsigned round from local block wall-clock
  age. Startup advances only past rounds protected by this validator's durable
  signed WAL; cross-validator convergence remains driven by authenticated
  prevote/precommit round evidence.
- Canonical block writes atomically advance a durable genesis-to-tip archive
  watermark only across linked block bodies. Verified snapshot activation sets
  the watermark only after its full archive walk succeeds. Mainnet startup
  requires the watermark to equal the canonical tip and then verifies every
  block body, parent link, standalone transaction record, `tx_by_slot` row, and
  `tx_to_slot` row through the combined hot/cold read path before entering
  consensus.
- `getHealth` exposes `archive_contiguous_slot` and
  `archive_contiguous_hash`. Once a store has an established watermark, a tip
  ahead of that proof reports `archive_incomplete` instead of `ok`.
- Slot-bounded public-history export applies the inclusive upper bound inside
  the canonical slot and transaction iterators. It cannot leak later slots in a
  large page or truncate transactions when the final slot spans multiple pages;
  unsupported bounded categories fail closed.

These changes prevent a repeat of the EU ENOSPC live-apply failure, the fleet's
800/500/500 versus 2000/1000/1000 timeout drift, and adjacent unsigned restart
rounds caused by different RocksDB open durations.

### Canonical Commit Certificates And Historical Validator Powers

The audit found that a block's local `commit_round` and `commit_signatures` are
attached after precommit and are not included in `Block::hash()`. Different
validators may therefore retain different valid quorum subsets for the same
block hash. That remains valid local consensus evidence, but it is not a
canonical public-history record and cannot drive deterministic liveness state.

The `0.5.224` candidate implements a version-2 canonical commit envelope. Every
block after height 1 carries one internal consensus transaction at index 0. The
transaction commits the parent block's sorted quorum certificate, complete
sorted parent-height validator powers, parent `validators_hash`, and the exact
parent post-effects state root plus child fee/oracle metadata hash under the
child's transaction root. Before each
height, the validator durably stores the complete power snapshot by
`validators_hash`; proposal creation fails if the parent snapshot is missing.
Proposal validation, network-sync ingestion, mainnet genesis-to-tip startup,
and RPC serving independently verify the certificate against the stored parent
block and the embedded parent-height denominator.

Checkpoint advertisements carry a Merkle proof that this exact certificate is
transaction 0 of the signed child header, plus the child's own commit signatures
and complete historical power denominator. A receiver therefore verifies the
checkpoint root and child finality without substituting its current validator
set or trusting a detached peer-supplied root.

The first activation test exposed why the denominator must be height-scoped:
the child at a validator-set activation boundary originally checked its parent
certificate against the newly active child set. A valid old-set commit then
appeared to have only 50% power and stalled every node. Version 2 authenticates
the exact parent powers with the parent header, so child membership changes
cannot reinterpret parent finality. `getBlockCommit` now exposes certificate
version, `validators_hash`, exact powers, signatures, and canonical/local source
so independent clients can verify the same threshold.

Only the canonical child certificate may drive deterministic signing-window or
epoch-boundary liveness state. Local peer reachability, announcements, wall
clocks, and locally observed signature subsets never alter proposer or voting
membership.

Required gates for that protocol work:

1. Four validators with one stopped continue finalizing and produce identical
   canonical certificate/history manifests; the stopped validator resumes from
   its own state.
2. Ten validators with two stopped continue finalizing with bounded round-one
   latency and identical certificate/history manifests.
3. Validator-set and stake changes at a height/epoch boundary verify parent
   certificates against the previous frozen power snapshot, not the new one.
4. Staggered full-cluster restarts converge from signed WAL/peer evidence with
   deliberately different database-open delays and no wall-clock round input.
5. Mainnet genesis explicitly configures signing-window, jail duration, rejoin,
   and timeout policy; no testnet hard-coded recovery hook is reachable by a
   mainnet chain ID.

The same protocol revision must commit every serialized field that can affect
execution, RPC/archive identity, or compact-block reconstruction. The current
header hash commits the transaction Merkle root but not the separately carried
fee metadata, oracle metadata, commit round, or commit-signature vector. Fee
metadata is re-derived and checked and legacy oracle metadata is not trusted for
execution, but an uncommitted serialized field is still malleable archive data.
Mainnet approval therefore requires a versioned block-data commitment or an
equivalent protocol-native consensus envelope, with historical testnet decoding
kept separate from the mainnet validation rule.

## Non-Negotiables

- Do not delete EU public history while it is the richest verified source.
- Do not reset the current testnet, generate a new genesis, synthesize a block,
  or replace a validator's state with another validator's RocksDB.
- Do not copy another validator's live RocksDB consensus state, WAL, keypairs,
  node identity, peer cache, signer state, or runtime state.
- Public-history repair must be additive, source-backed, idempotent, resumable,
  and conflict-checked.
- Any same-key byte mismatch in repaired public-history column families must
  abort execution and produce evidence.
- Large historical payloads must land in the configured cold/archive store when
  one is attached. Small ordering indexes may remain in the hot state DB.
- A validator resumed from its own state must serve the same public history as
  the rest of the fleet after repair.
- A new validator joining from network sync must be able to recover public
  history from peers without requiring an out-of-band copy of a validator DB.
- Local four-validator testnet validation is mandatory before any live VPS
  deployment, tag, or release announcement.

## Parity Definition

For a validator set to be archive-parity clean:

1. Every validator reports the same current slot, checkpoint state root, and
   chain identity for the target network.
2. Every validator can serve the same canonical public history from genesis to
   tip through hot plus cold stores.
3. Public history includes at least:
   - `slots`
   - `blocks`
   - `transactions`
   - `tx_by_slot`
   - `tx_to_slot`
   - `tx_meta`
   - `account_txs`
   - `events_by_slot`
   - `events`
   - `token_transfers`
   - `program_calls`
   - DEX trade and market activity indexes that back trades and candles
   - current balances through state-root parity, and historical balance
     inspection through account snapshots where archive mode records them
   - governance/proposal visibility through backed transactions, events,
     program calls, and current state-rooted governance storage
   - EVM, shielded, NFT, and market activity public indexes when present
4. Representative RPC probes return the same result on every validator:
   - `getBlock` for genesis, early, mid-chain, recent, and tip slots
   - `getTransaction` for sampled historical transactions
   - `getTransactionsByAddress` for known historical addresses
5. Archive manifests match across validators for the repaired range. A manifest
   is a deterministic digest plus row-count summary over public-history column
   families and slot ranges.

## Implementation Plan

### 1. Archive Parity Manifest

Add an operator command that scans public-history column families in deterministic
pages and emits JSON evidence:

- network, db path, cold-store path, binary version, and chain identity
- category name
- slot range or key range
- row count
- first and last key
- deterministic digest of keys and values
- missing canonical block or transaction references
- hot rows, cold rows, and combined totals where applicable

The command must work on a live node using a read-only secondary or on a stopped
node using the primary DB. It must be cheap enough to run by range and complete
enough to prove genesis-to-tip parity in batches.

Implemented command:

```bash
sudo -u lichen /usr/local/bin/lichen-validator \
  --network testnet \
  --db-path /var/lib/lichen/state-testnet \
  --secondary-dir /tmp/lichen-public-history-manifest \
  --cache-size-mb 256 \
  --public-history-manifest \
  > /tmp/lichen-public-history-manifest.json
```

Use `--categories slots,blocks,transactions` only for narrow diagnostics. The
release gate must use the default full public-history category list.

### 2. Source-Backed Repair

Add an operator repair path that imports public history from a verified peer or
source archive without copying consensus state:

- stream or page the same public-history categories used by snapshot export
- write blocks, transaction bodies, account transaction rows, events, token
  transfers, and program calls into the target cold store when attached
- write slot cursors and ordering indexes into the target hot DB
- skip existing identical rows
- abort on existing conflicting rows
- persist progress so the repair can resume after interruption
- clear derived account-history counters only after successful writes
- produce dry-run and execute evidence with inserted, skipped, and conflicted
  row counts

This extends the existing guarded public-history merge concept from a mounted
local source into an operational fleet repair that can be driven from a verified
peer/source.

Implemented source-backed command:

```bash
# Live fleet path: prove and stream one exact source-backed range without
# copying RocksDB. Dry-run first; this records per-page conflict evidence.
bash scripts/stream-public-history-repair.sh \
  --source SOURCE --targets "TARGETS" \
  --categories slots,blocks,transactions,tx_by_slot,tx_to_slot,tx_meta \
  --from-slot FIRST --to-slot LAST

# Execute requires recorded current provider backups, identical candidate
# hashes, a contiguous source proof, a complete conflict-free dry run, measured
# missing-byte headroom, and exact confirmations. Targets remain stopped across
# subsequent source ranges.
export LICHEN_PUBLIC_HISTORY_BACKUP_CONFIRM='current-backups-verified:testnet:15.204.229.189,37.59.97.61,15.235.142.253,148.113.43.247'
export LICHEN_PUBLIC_HISTORY_STREAM_CONFIRM='stream-public-history-repair:testnet:SOURCE:TARGETS_CSV'
bash scripts/stream-public-history-repair.sh --execute --leave-target-stopped \
  --source SOURCE --targets "TARGETS" \
  --categories slots,blocks,transactions,tx_by_slot,tx_to_slot,tx_meta \
  --from-slot FIRST --to-slot LAST

# Then merge the remaining source-backed public indexes without block-range
# flags. Repeat both phases for every verified source in the complete union.
bash scripts/stream-public-history-repair.sh --execute --leave-target-stopped \
  --source SOURCE --targets "TARGETS" \
  --categories account_txs,events_by_slot,events,token_transfers,program_calls,evm_txs,evm_receipts,evm_logs_by_slot,shielded_txs,nft_activity,market_activity,dex_trades_by_pair,dex_trades_by_taker,dex_trades_by_pair_taker,account_snapshots

# Page-level primitives used by the streaming repair script.
sudo -u lichen /tmp/lichen-validator-0.5.224-candidate \
  --network testnet \
  --db-path /var/lib/lichen/state-testnet \
  --secondary-dir /tmp/lichen-public-history-export \
  --export-public-history-category blocks \
  --chunk-size 1000 \
  > /tmp/blocks-page.json

sudo -u lichen /tmp/lichen-validator-0.5.224-candidate \
  --network testnet \
  --db-path /var/lib/lichen/state-testnet \
  --import-public-history-category blocks \
  --execute \
  --confirm public-history-repair:v1 \
  < /tmp/blocks-page.json

# Large block-body ranges use binary framed streams instead of JSON pages.
# This keeps one source process and one target process open for the range and
# avoids copying RocksDB, WAL, identities, or peer state.
ssh -p 2222 ubuntu@SOURCE \
  "sudo -u lichen /tmp/lichen-validator-0.5.224-candidate \
    --network testnet \
    --db-path /var/lib/lichen/state-testnet \
    --secondary-dir /tmp/lichen-public-history-export \
    --public-history-page-format binary \
    --stream-pages \
    --export-public-history-category blocks \
    --after-key-hex 0000000000419897 \
    --to-slot 5176463" \
| ssh -p 2222 ubuntu@TARGET \
  "sudo -u lichen /tmp/lichen-validator-0.5.224-candidate \
    --network testnet \
    --db-path /var/lib/lichen/state-testnet \
    --secondary-dir /tmp/lichen-public-history-import \
    --public-history-page-format binary \
    --stream-pages \
    --import-public-history-category blocks \
    --dry-run"

# Dry-run first. This may use a RocksDB secondary for the target.
sudo -u lichen /usr/local/bin/lichen-validator \
  --network testnet \
  --db-path /var/lib/lichen/state-testnet \
  --secondary-dir /tmp/lichen-public-history-repair-check \
  --cache-size-mb 256 \
  --repair-public-history-from-source /mnt/verified-source/state-testnet \
  --source-cold-store /mnt/verified-source/archive-testnet \
  --dry-run \
  > /tmp/lichen-public-history-repair-dry-run.json

# Execute only with the target validator stopped and conflict_rows=0 from dry-run.
sudo systemctl stop lichen-validator-testnet
sudo -u lichen /usr/local/bin/lichen-validator \
  --network testnet \
  --db-path /var/lib/lichen/state-testnet \
  --cache-size-mb 256 \
  --repair-public-history-from-source /mnt/verified-source/state-testnet \
  --source-cold-store /mnt/verified-source/archive-testnet \
  --execute \
  --confirm public-history-repair:v1 \
  > /tmp/lichen-public-history-repair-execute.json
sudo systemctl start lichen-validator-testnet
```

The mounted-source command remains available when the verified source is already
attached locally and there is enough scratch capacity. For live VPS repair, use
the streaming script because US/SEA/IN do not have enough free space to hold a
full EU source DB copy.

Implemented source/target parity command:

```bash
sudo -u lichen /usr/local/bin/lichen-validator \
  --network testnet \
  --db-path /var/lib/lichen/state-testnet \
  --secondary-dir /tmp/lichen-public-history-parity-check \
  --cache-size-mb 256 \
  --verify-public-history-parity-with-source /mnt/verified-source/state-testnet \
  --source-cold-store /mnt/verified-source/archive-testnet \
  > /tmp/lichen-public-history-parity.json
```

### 3. Resume And Joiner Safeguard

Snapshot and sync paths must preserve public history, not only current consensus
state. A resumed validator and a new joiner must import the archive/public
history categories needed for historical RPC reads. After import, the validator
must run the parity verifier before it is considered archive-ready.

### 4. Fleet Verification

Add an operator verifier that compares archive manifests and RPC probes across
US, EU, SEA, and IN. The verifier must fail if any validator cannot serve a
sampled historical slot, transaction, or account-history query that another
validator can serve.

### 5. Local Four-Validator Gate

Before live deployment:

1. Start a clean local testnet with four validators.
2. Produce enough blocks and transactions to create public history.
3. Force hot-to-cold migration locally.
4. Simulate an archive drift case where one validator has backed history that
   the other validators lack.
5. Run dry-run parity audit and prove the drift is detected.
6. Run repair from the backed source into the missing validators.
7. Restart all four validators from their own state.
8. Verify `getBlock`, `getTransaction`, and `getTransactionsByAddress` parity
   across all four validators from genesis through current tip.
9. Run `bash tests/local-multi-validator-test.sh 4` or the updated replacement
   gate and preserve the output as release evidence.

The local multi-validator gate now enables archive-backed validators by default
for this check:

```bash
LICHEN_LOCAL_ARCHIVE_COLD=1 \
LICHEN_COLD_RETENTION_SLOTS=20 \
LICHEN_COLD_MIGRATION_INTERVAL_SECS=5 \
bash tests/local-multi-validator-test.sh 4
```

The harness flushes local hot state and local cold archive directories, restarts
joiners from their own state, restarts the seed, restarts all validators from a
preserved tip, and compares the canonical public-history manifest root across
all local validators before passing.

### 6. Live Rollout Gate

After local proof only:

1. Prove that the source set contains backed public history for every slot from
   genesis to current tip. A source with a gap is not a release source.
2. Record provider backup/snapshot IDs for all four current disks.
3. Run the mandatory full import dry run for every target and prove free space
   covers 150% of measured missing bytes plus the 10 GiB reserve.
4. Stop all validators and keep every target stopped during every source import.
5. Run bounded source-range proof and repair dry-runs before execute imports.
6. Repair missing validators only from verified backed sources. Any conflict
   aborts; no target is restarted after a partial import.
7. Run `--offline-repair-gate`; it compares fixed-tip manifests and deliberately
   leaves every validator stopped even when the gate passes.
8. Install the same candidate hash and start all four validators coordinately
   from their own preserved state, genesis, keys, identities, and archives.
9. Run full archive parity, fresh-tip liveness, explorer, wallet, DEX,
   governance, balance, transaction, trade, and candle journeys.
10. Keep the parity verifier in the runbook as a mandatory post-deploy and
   periodic operations check.

Fleet verifier:

```bash
# Live read-only fleet pass. This records host health, release hash, disk usage,
# manifest evidence through RocksDB secondaries, and historical RPC probes.
bash scripts/verify-testnet-archive-parity.sh

# Repair gate: fixed-tip manifests are compared while every service remains
# stopped. A separate coordinated start is allowed only after this exits zero.
export LICHEN_ARCHIVE_PARITY_STOP_CONFIRM='archive-parity-stop:testnet:15.204.229.189,37.59.97.61,15.235.142.253,148.113.43.247'
bash scripts/verify-testnet-archive-parity.sh --offline-repair-gate

# Strict release gate. This stops all validators, compares the full fixed-tip
# manifests, and restarts only after the offline equality precheck succeeds.
export LICHEN_ARCHIVE_PARITY_STOP_CONFIRM='archive-parity-stop:testnet:15.204.229.189,37.59.97.61,15.235.142.253,148.113.43.247'
bash scripts/verify-testnet-archive-parity.sh --stop-for-manifest
```

## 2026-07-15 Public RPC Edge Recovery

Live validator health and public ingress health are separate release surfaces.
All four validators were producing while the former single-origin public RPC
hostname was unavailable from at least one region. The public explorer now uses
the same-origin path `https://explorer.lichen.network/api/testnet`, backed by the
checked-in `edge/testnet-rpc` Cloudflare Worker.

The canonical public endpoint is `https://testnet-api.lichen.network`; the old
hostname remains only as a transition route for already deployed `v0.5.223`
clients. The Worker has four independent HTTPS origins in US, EU, SEA, and IN. Every
origin has a unique Wrangler secret and matching root-owned Caddy token file;
direct unauthenticated origin requests return HTTP 403. Normal RPC and WebSocket
requests use deterministic origin affinity, probe health before forwarding, and
fail over to another independently authenticated origin on health, network, or
gateway failure. Replayable request bodies are bounded at 2 MiB. The strict
`/edge-health` endpoint checks all four origins and returns HTTP 503 if any is
unhealthy or more than 64 slots behind the highest origin.

Recovery evidence:

- Worker unit tests pass 7/7 and the Wrangler production bundle dry-run passes.
- Frontend asset/CSP integrity passes 377/377; DEX readiness passes 35/35,
  wallet passes 136/136, wallet extension passes 135/135, extension signing E2E
  passes 9/9, and frontend RPC parity reports zero unknown live calls.
- All ten Pages projects were deployed through Wrangler and every custom domain
  returned HTTP 200. Canonical and explorer same-origin RPC/WS smokes pass.
- Twenty-four live RPC requests reached all four regions through the canonical
  edge: US 5, EU 6, SEA 6, and IN 7.
- Strict health reported all four origins ready with a one-slot spread in the
  final July 15 sample.
- Fixed slot `9,139,000` returned block hash
  `ac7be90e...be6e` independently from US, EU, SEA, and IN.
- `lichen-validator-testnet.service` is active and enabled on every VPS with
  `NRestarts=0`. US/SEA/IN run the signed `v0.5.223` rollback binary
  (`e956a8fb...2e810`). A later process-image audit found that EU still runs
  the audited exact-tag `v0.5.223` sparse-maintenance bridge
  (`9b71e7a9...6ccee`) through its temporary catch-up override even though the
  installed canonical binary is signed `v0.5.223`; the earlier identical-image
  claim was incorrect. No `v0.5.224` candidate is installed or running.
- The final post-maintenance multiplexed read-only fleet pass completed without
  stopping any service. US/EU/SEA/IN were healthy at slots
  `9,182,142..9,182,188`, a 46-slot spread, and all four returned digest
  `6c15273f...8cdd88` for fixed block `9,139,000`. Evidence:
  `evidence/archive-parity/testnet-20260715T-final-clean-exit`; summary SHA-256
  `17123769...3b59f2` and run-log SHA-256 `ccfeef98...129bf1`.
- The same pass exposed an operations-script defect without a chain outage:
  repeated short SSH sessions reached IN's intentional UFW limit of six new
  connections per 30 seconds. The verifier now keeps one multiplexed transport
  per host and waits through the firewall window before retrying an initial
  connection.
- The pass found IN's v0.5.223 `contract_merkle_nodes` derived cache growing
  again. RocksDB recorded a 2.66 GB interval compaction in that cache, and IN
  reached about 101.9 GB of stopped hot state with 17.2 GB free. The exact
  audited `v0.5.223` maintenance binary (`9b71e7a9...ccee`) rebuilt only the
  sparse node/leaf caches at stopped slot `9,180,291`; typed verification
  reported 124,764 canonical contract leaves and 249,527 reachable contract
  nodes. Protected identity, genesis, signer, validator, and cold-archive
  metadata hashes were byte-for-byte unchanged before restart. After IN crossed
  checkpoint `9,181,000`, normal retention released the obsolete hard-linked
  cache files: hot state fell to about 49.0 GB and free space rose to about
  70.1 GB. The signed installed binary remained `e956a8fb...2e810`, and service
  warnings and `NRestarts` remained zero. Evidence:
  `evidence/post-block-effects-recovery/testnet-20260715T-final/in-sparse-cache-rebuild`.
  This is rebuildable sparse-Merkle cache, not unique canonical
  block/transaction history. AP-057R is the permanent bounded-cache release
  fix; no history directory was or may be removed to reclaim this space.

This recovery proves current liveness, common-chain reads, and public ingress
redundancy. It does not waive AP-060 or claim full genesis archive parity.

## 2026-07-15 Rust Dependency Reproducibility Audit

A fresh standalone compiler/SDK build exposed a dependency-resolution defect
that the root workspace lock had masked. `ml-dsa 0.1.0-rc.8` and
`slh-dsa 0.2.0-rc.4` declare a prerelease PKCS#8 dependency range that Cargo can
advance to `pkcs8 0.11.0` final, whose error API is incompatible with those PQ
crate releases. The workspace now pins `pkcs8 0.11.0-rc.11`, and core carries a
direct workspace compatibility anchor so every standalone path consumer gets
the same constraint.

All 40 committed Rust lockfiles were regenerated from their manifests and pass
locked metadata plus cargo-audit. Cargo-deny passes advisories, bans, licenses,
and sources. The previously failing standalone gates now pass: compiler 30/30,
contract SDK 28/28, Rust client SDK 88/88 plus all examples, and all 14 fuzz
targets compile. All 34 contract workspaces pass native tests and locked WASM
builds: 32 genesis catalog contracts, the genesis-bound `launchpad_token`
graduation template, and the standalone `mt20_token` template. CI and release
workflows run `test_rust_dependency_policy.js` to
guard the PQ compatibility anchor and shared contract target directory. The
shared Cargo cache is isolated at `target/contract-build`; genesis development
discovery walks working-directory ancestors before executable ancestors so a
build cache cannot shadow the shipped `contracts/` tree. Genesis tests restore
the global contracts-directory environment even after a panic, preventing one
primary failure from poisoning the rest of the suite.

The final-candidate AP-050 rerun also passed on July 15. V2/V3/V4 joined from
empty stores; V4 and the seed resumed their own state at zero drift; production
continued with the seed offline; all four restarted from one preserved tip and
then produced 42 blocks in 10 seconds. Every validator proposed and voted, and
the canonical certificate matched at slot 754 (`fbbf64dc...d9f2b3`). The
checkpoint-1000 hot/cold root matched on all four (`200e1d4c...f1f3b2`), volume
journeys passed 140/140, launchpad/governance/graduation passed 104/104, and the
final checkpoint-3000 hot/cold root matched on all four
(`54d0e997...43db8a6`). Transcript:
`evidence/post-block-effects-recovery/testnet-20260715T-final/four-local-final.log`,
SHA-256 `f73e134bac9e6bbfe477dc08db86461d998fe49402f2ea2e2181c29d377fce9b`.

## 2026-07-15 SEA LiveSync Stall And Source-Backed Gap

Signed `v0.5.223` SEA detected a state-root mismatch at slot `9,236,790`,
verified and activated checkpoint `9,238,000`, and caught up to slot
`9,238,795`. The process and systemd service then remained active while the
canonical tip stopped advancing. It continued receiving future blocks and
requesting parent gaps, so the old receipt-based watchdog considered the node
active and never restarted it. A controlled restart from the same SEA database,
identity, keypair, WAL, and cold archive immediately resumed catch-up. No state
copy or reset was used.

The permanent candidate behavior is phase-based rather than restart-based. A
LiveSync node that observes a material canonical gap, or enters checkpoint,
state-root, fork, or BFT-commit repair, returns to bounded InitialSync before
future-block handling. Only canonical block application or an accepted verified
snapshot chunk refreshes the watchdog; pending or unchainable network blocks do
not hide a stalled canonical tip. After the 120-second progress timeout, six
five-second confirmation checks bound supervisor recovery without shortening
the normal sync window.

The final exact-source four-local gate rebuilt the candidate after the ABI
marker-collision repair and paused V4 in
LiveSync while the other three advanced 140 slots. The same V4 process caught
up within the 20-slot drift bound; subsequent V4-only, seed-only, and
all-validator preserved-state restarts passed. The final checkpoint-3000
hot/cold manifest root was identical on all four validators
(`3b764af7...34c55be`) after 244/244 user journeys. Certificate parity matched
at slot `930` (`e4152bd9...103d5ae`), and the checkpoint-1000 root was
`de87f503...174589d`. Transcript:
`evidence/post-block-effects-recovery/testnet-20260715T-final/four-local-abi-marker-final.log`,
SHA-256 `72eafc1a75cfc15e4c03d482fb057536e019cc306a7968a7fa844dcb95046fb5`.

The final exact-source ten-validator expansion passed on the same candidate. V2 through V10
joined from empty stores, V10 caught up in the same process after a 140-slot
pause, individual and coordinated preserved-state restarts resumed finality,
and the network continued with both V9 and V10 stopped before they recovered
from their own identities and databases. Every validator proposed and voted;
the canonical certificate matched at slot `2718`
(`3a74f369...043a0d8`), and all ten checkpoint-3000 hot/cold manifests matched
root `c85f3ca9...6750a2`. Transcript:
`evidence/post-block-effects-recovery/testnet-20260715T-final/ten-validator-abi-marker-final.log`,
SHA-256 `4f226b6efea98edb41f783dd8da1c913fb39420c900ea6b3963f610ee85701b7`.

The four-validator user journey also caught a randomized contract address whose
first byte was `0xAB`. The runtime previously treated that byte alone as a
binary ABI layout marker, so raw DEX token-custody arguments were reinterpreted
as strides and the authenticated nested caller was corrupted. Layout mode now
requires one valid stride for every WASM parameter and exact payload coverage;
otherwise the bytes remain raw. SDK and DEX cross-calls emit canonical layouts
explicitly. Focused runtime/SDK/DEX tests, a nested caller identity regression,
a preserved failed-chain restart, and the final 244/244 journey rerun pass.

The final standalone matrix then passed compiler 30/30, contract SDK 28/28,
Rust client SDK 88/88 plus documentation/examples, every fuzz binary, all 34
native contract workspaces, all 34 locked WASM builds, and helper guards 12/12.
Its transcript SHA-256 is
`baa5788d4c9b96272c7aec434f7cb69316c80560e18b39931e8e4815e506d39f`.
The numeric non-root compiler sandbox passed Rust, C, and AssemblyScript WASM
with transcript SHA-256
`b61fe3061c035428bab6d3650a9d8c39d9da4ce6887f755b37e11916249bd2fe`,
and valid CycloneDX JSON was generated for every root workspace package.

A fresh isolated running chain passed comprehensive RPC 146/0/1, CLI 29/0,
deterministic E2E 25/0, and full RPC/DEX REST 146/0/1; the preserved default
local databases were restored after the run. Transcript SHA-256:
`91b1c392f64fc329ea91856652bafd8799dd543ec107578f2d0543798f155898`.
That run also proved the CI readiness probe previously accepted a stale
`getHealth` response and the CLI suite returned zero despite counted failures.
Both gates now fail closed on non-healthy status or any CLI failure.

Direct fixed-slot comparison also identified the historical side effect of the
live repair: SEA serves the common block at `9,236,789`, lacks canonical bodies
`9,236,790..9,237,999`, has a different checkpoint block at `9,238,000`, and
matches US/EU/IN again from `9,238,001`. US, EU, and IN are verified sources for
that bounded interval. Before final live archive parity, SEA must be stopped and
repaired additively from one of those sources through the guarded stream repair
path, then all four must pass the fixed-tip offline manifest gate. The repair
must not copy another validator database or synthesize a block body.

## 2026-07-15 Historical Transaction-Domain Replay Audit

A read-only replay of the first block rejected by US, canonical slot
`9,273,160`, exposed a release-blocking candidate regression before tag or
deployment. The block contains three valid native oracle transactions signed
with the historical raw-message domain used by signed `v0.5.223`. Each verifies
with `verify_required_signatures()` and fails the new chain-ID verifier. The
candidate Core processor selected chain-ID verification whenever `chain_id`
metadata existed, without considering the block slot, so it would reject valid
pre-upgrade transactions during catch-up or diagnostic replay.

Consensus v1 now uses the same durable, guarded activation slot for
crash-complete post-block effects, analytics migration, and native transaction
signature domains. A block below that boundary reproduces the deployed
`v0.5.223` transition policy: verify the chain-ID domain first, then the legacy
domain required by historical internal transactions. At or above the boundary,
execution requires a nonempty canonical chain ID and accepts only chain-domain
signatures. Missing activation metadata retains that bounded historical replay
behavior for an unmodified pre-upgrade database; malformed metadata fails
closed. Existing
public chains must align at one exact stopped tip and persist the same next-slot
boundary with `--prepare-consensus-v1-activation`; fresh chains persist slot 1.
RPC and validator ingress continue to require chain-domain signatures for all
new transactions, with no verification fallback.

The same signing audit found that custody wrapped-credit minting still signed
raw message bytes. It now fetches the canonical chain ID from `getNetworkInfo`
and signs the mint transaction for that exact network before V1 wire encoding.
A regression proves the mint verifies only on the selected chain. No candidate
binary was installed and no live database was mutated during this audit.

The exact AP-073 release gates then passed on the final source. The
four-validator run recovered V4 in the same process after a 140-slot LiveSync
pause, restarted V4 and V1 from their own state, resumed all four from one
preserved tip, and matched the canonical certificate at slot `946`. Its
checkpoint-1000 hot/cold manifest root was identical on every validator, the
volume/user journeys passed 140/140, the launchpad/governance/graduation
journeys passed 104/104, and every checkpoint-3000 manifest matched root
`b7232593...1bae1de`. Transcript:
`evidence/post-block-effects-recovery/testnet-20260715T-final/four-validator-ap073-final.log`,
SHA-256 `cf8e487ca2aeea98b7c6c1c8757b7c37803542d2f9de58988ceadac1a46204a5`.

The ten-validator expansion independently joined V2 through V10 from empty
hot/cold stores, recovered V10 in the same process after the same 140-slot
pause, passed individual and coordinated preserved-state restarts, and kept
finalizing with V9 and V10 stopped before both rejoined with their original
identities and databases. Every validator proposed and voted, the canonical
certificate matched at slot `2600`, and all ten checkpoint-3000 manifests
matched root `48cb5614...649255`. Transcript:
`evidence/post-block-effects-recovery/testnet-20260715T-final/ten-validator-ap073-final.log`,
SHA-256 `13642d7d3bfb618091ffaf6d65c5af3e0cfbf00f965de52c651a4dd1c91a31fc`.
The final locked workspace suite, strict Clippy, dependency policy/security,
all 34 contract native/WASM builds, SDK/compiler/fuzz suites, deployment/static
QA, developer portal 244/244, and marketplace 390/390 also pass on this source.

## 2026-07-16 Live Source-Union Preflight

A strict-host-key, read-only fleet pass found all four validators active,
enabled, producing current blocks, and at `NRestarts=0`. Slots were
`9,358,361..9,358,413`, a 52-slot spread, and recent fixed-block digests
matched. Free space was approximately 63 GB US, 49 GB EU, 80 GB SEA, and 56 GB
IN. US/SEA/IN executed the signed `v0.5.223` image; EU continued to execute the
previously audited exact-tag sparse-maintenance bridge.

The automatic midpoint probe selected slot `4,679,179`. IN returned its signed
block body while US, EU, and SEA returned `Block not found`. This is inside the
already inventoried IN source range `4,299,000..5,176,463`; it confirms that the
backed source union has not yet been replicated across the fleet. It is not part
of the accepted irrecoverable interval. AP-060 and AP-070 therefore require the
stopped-fleet repair to union every proven current/provider source range,
including the IN and US historical ranges, recovered US slot `5,275,999`, and
SEA's recent `9,236,790..9,237,999` hole, before fixed-tip parity can pass.
Evidence is under `evidence/archive-parity/testnet-20260716T022033Z`.

## Tracker

| ID | Work item | Evidence | Status |
| --- | --- | --- | --- |
| AP-001 | Document archive parity incident, invariants, and release gate | This file plus runbook links | Done |
| AP-010 | Add deterministic public-history manifest/audit command | JSON manifest over hot+cold public history | Done in core/validator admin |
| AP-020 | Add source-backed public-history repair from verified source | Dry-run and execute counters, conflict aborts | Done for mounted/read-only source DBs, SSH-streamed public-history pages, and binary framed block streams |
| AP-030 | Add source/target archive parity verifier | JSON category diff and nonzero exit on mismatch | Done for mounted/read-only source DBs |
| AP-035 | Add peer/fleet archive parity automation so a new joiner can prove archive readiness from any validator | `tests/local-multi-validator-test.sh` and `scripts/verify-testnet-archive-parity.sh` output | Done for local harness plus VPS fleet verifier. The verifier reuses one SSH control transport per host so its probes cannot trip the intentional VPS connection-rate limit; live execution evidence is tracked under AP-060. |
| AP-040 | Add regression tests for hot+cold archive import, repair, restart, and conflict handling | Cargo/test harness output | Done locally, including account-snapshot hot/cold manifest invariance, historical reads, idempotence, conflict preservation, exact inclusive range pagination, stopped-node migration audit/compaction, prune refusal, existing cold-DB schema upgrade, snapshot rollback completeness, and cold-source guarded repair; updated Core 981/981, all integrations, Validator 393/393, helper guards 12/12, and strict Clippy pass |
| AP-050 | Run local four-validator testnet archive parity gate | `LICHEN_RUN_LAUNCHPAD_E2E=1 LICHEN_RUN_VOLUME_E2E=1 LICHEN_LOCAL_ARCHIVE_COLD=1 LICHEN_COLD_RETENTION_SLOTS=20 LICHEN_COLD_MIGRATION_INTERVAL_SECS=5 bash tests/local-multi-validator-test.sh 4` | Done on the exact final candidate. V2/V3/V4 joined from empty stores; V4 resumed in the same process after a 140-slot LiveSync pause; V4 and V1 resumed their own state; the network finalized with V1 offline; and all four resumed from preserved state at spread `3`. Certificate parity matched at slot `930` (`e4152bd9...103d5ae`); checkpoint-1000 hot/cold manifests matched root `de87f503...174589d`; volume/user journeys passed 140/140; launchpad/governance/graduation passed 104/104; and final checkpoint-3000 manifests matched root `3b764af7...34c55be`. Transcript SHA-256: `72eafc1a75cfc15e4c03d482fb057536e019cc306a7968a7fa844dcb95046fb5`. |
| AP-055 | Inventory current and detached backed public-history sources and gap boundaries | `evidence/archive-parity/testnet-20260709T181442Z`, July 13 read-only volume/WAL/ext4 audits | Done: recovered hidden EU volume `9878038e...2c57` proves exactly through `2,872,005`; IN proves `4,299,000..5,176,463`; US proves `4,864,001..5,275,998`; the US July 9 provider copy contains a decoded, hash-matching orphan body for `5,275,999`; no audited source yet proves `2,872,006..4,298,999` |
| AP-057 | Add fail-closed source-range, binary, capacity, backup, and stopped-fleet gates | Validator range-proof JSON plus helper guard suite | Done: final Validator 397/397 and strict Clippy pass; helper guards pass 12/12; live read-only proof accepted EU `0..10` and IN `4,299,000..4,299,010`, and rejected missing EU slot `2,872,006` before any write |
| AP-057A | Refuse snapshot live apply before mutation unless measured replacement, compaction, runtime capacity, and complete target history are available; restore every exact pre-apply hot category on interrupted apply | Unit tests plus local snapshot roundtrip/ENOSPC/history-loss gate | Done locally; focused snapshot rollback 5/5, Core 981/981 plus integrations, Validator 393/393, strict Clippy, and the slot-3000 four-validator restart/archive gate pass |
| AP-057B | Enforce slot-0 genesis config authority and remove local wall-clock BFT restart rounds | Drift repair/conflict tests plus staggered restart gate | Done locally; final Validator 393/393, equal-stake 4/3 and 10/8 unit gates, V4/V1 staggered restarts, and the coordinated four-node restart pass |
| AP-057D | Maintain and expose a durable archive-contiguity proof; fail mainnet startup on missing blocks or transaction indexes | Mainnet startup tests, `getHealth` fields, full archive walk | Done locally; final Core 981/981 plus integrations, Validator 393/393, RPC `326+241`, strict Clippy, and final four- and earlier ten-node offline manifest gates pass |
| AP-057C | Canonically commit finality certificates before using them for archive parity or validator liveness state | 4/3 and 10/8 certificate/parity/lifecycle tests | Done: final 4/3 and 10/8 gates, own-state/coordinated restarts, certificate parity, and archive parity pass. |
| AP-057E | Commit every consensus-relevant serialized block field under a versioned mainnet validation rule | Mutation tests for fees, oracle metadata, certificates, compact/full reconstruction, and historical testnet decode | Done in the version-2 envelope; mutation/layout/reconstruction/sync/historical-decode tests and final 4/10 release gates pass. |
| AP-057F | Make canonical execution, receipts, block body, transaction indexes, tip/finality cursors, and oracle-attestation projection crash-atomic | Reopen-after-commit tests and the 10-validator restart reproduction | Done. One RocksDB batch commits canonical execution and its block anchor; oracle-attestation derived records join that boundary. The final V10 own-state and coordinated restart gates pass. |
| AP-057G | Make checkpoint roots independently verifiable across validator-set changes | Historical 4-to-10 denominator, detached-root tamper, Merkle inclusion, child-finality, and checkpoint cache tests | Done. Checkpoint tests 48/48 and final 4/10-node checkpoint bootstrap, certificate, and manifest gates pass. |
| AP-057H | Prevent an offline configured seed RPC from deadlocking a surviving same-tip quorum after restart | Peer-tip provenance tests plus preserved-state 3-of-4 seed-offline reproduction | Done. Preserved-state reproduction and final exact-release 3/4 plus 9/10 seed-offline gates pass without state or seed changes. |
| AP-057I | Make recent-blockhash cache warm-up range-proven and make the release harness reject stale validators | Reopen/partial-warmup regression plus per-validator final RPC/activity freshness checks | Done. The first 10-node run exposed V2 stalled at slot 2248; after the range-coverage fix, Core 981/981 plus integrations, Validator 393/393, strict Clippy, the earlier 10-node freshness/manifest root `027d802a...65ef5a`, and final four-node gate all pass. |
| AP-057J | Recover stopped archive nodes whose safe runtime floor prevents automatic hot-to-cold migration | Read-only row audit, conflict refusal, hard-link refusal, write-first migration, scoped compaction | Done on EU. The original iterator produced false-zero evidence; total-order raw scanning found 2,453,338 old hot blocks plus 1,467,110 transaction/index rows. The final audit reported every integrity counter zero; bounded execute migrated all eligible rows in 246 compaction batches, raised free space from 10.94 GB to 20.79 GB, and the post dry run plus full raw audit reported zero errors. |
| AP-057K | Make `--from-slot`/`--to-slot` repair exports exactly inclusive across every page | Final-slot multi-transaction pagination and later-slot exclusion tests | Done locally: bounded exports for slots, blocks, transactions, `tx_by_slot`, `tx_to_slot`, and `tx_meta` retain every final-slot row with chunk size 1, exclude the next slot, and reject unsupported categories; updated Core 981/981, all integrations, Validator 393/393, helper guards 12/12, strict Clippy, and final AP-050 four-validator gate pass. |
| AP-057L | Prevent a complete canonical parent with an incomplete producer/post-effects projection from wedging the next BFT height | Block-hash-bound recent startup recovery, pre-BFT parent gate, exact-once regressions, live state digest | Done locally. Two separate `v0.5.223` incidents reproduced the fault, including slot `9,000,623`; candidate startup and BFT-gate regressions repair the stale parent exactly once before the next-height root is read, and AP-050 passes. |
| AP-057M | Prevent legacy/pruned marker absence from replaying already-applied post-block effects | Durable activation-boundary persistence, pre-activation no-replay, activated missing-body failure, and offline-admin refusal tests | Done locally. Every startup/BFT/duplicate-tip/admin repair path is activation-bounded. Pre-activation slots are never inferred from marker absence, and the superseded candidate was never installed or allowed to write live state. |
| AP-057N | Prevent a lagging existing-chain validator from selecting an earlier upgrade height | Exact stopped-tip activation dry-run/execute, status-78 absent-boundary startup, shared analytics gate, and fresh-genesis slot-1 tests | Done locally; Validator 396/396, strict workspace Clippy, full workspace tests, and final AP-050 pass. Existing public databases never auto-select `local tip + 1`. The guarded command accepts only the exact next slot, requires `consensus-v1-activation:<network>:<slot>`, WAL-syncs/readbacks the durable marker, and analytics v2 waits for it. All four must stop at one exact tip/hash before the candidate activation command runs. |
| AP-057O | Make every recovery inspection command provably non-mutating | Read-only root APIs, disposable RocksDB secondary, primary-before/after sparse verification, and secondary mutation refusal | Done locally and against the provider rollback. Contract-storage, stake-pool, and commitment-schema inspection use read-only root computation and support `--secondary-dir`; sparse rebuild and activation refuse secondary mode. The first guarded EU attempt exposed the old mutating cold-start call and reversed its swap before service start. The local rollback was restored from the read-only provider copy and reconciled by checksum/inventory. Corrected Linux/amd64 binary `e82cd6f5...59201` then opened the pristine, read-only July 12 provider rollback only through disposable secondaries and returned exact slot `8,915,275`, current/cached root `cbf7770f...03d3a`, stake digest `3ea8c6c5...37747`, four active validators, and `state_root_recompute=read_only`. Core 983/983 plus integrations, Validator 396/396, the full locked workspace, strict Clippy, helper guards 12/12, and AP-050 pass. |
| AP-057P | Serialize canonical state writes with derived sparse commitment mutation and self-repair stale clean metadata at startup | Same-key commit/root race, lost-marker startup reconstruction, full Core/Validator suites, AP-050 | Done locally. One state-level lock covers canonical batch/direct writes and every mutating root/rebuild path. Startup verifies clean metadata from canonical accounts/contracts and rebuilds then re-verifies before BFT on mismatch. The 1,000-commit/1,000-root race and clean-looking lost-marker regressions pass, as do Core 983/983 plus integrations, Validator 396/396, the full locked workspace, strict Clippy, helper guards 12/12, and AP-050 through checkpoint 3000 with identical hot/cold roots on all four validators. |
| AP-057Q | Prevent point-lookup RocksDB iterators from silently skipping migration rows and fail closed on missing block-referenced transaction indexes | Large-store total-order audit, absent/corrupt/cold-only transaction tests, bounded compaction test | Done in source. Runtime and stopped-node block scans plus auxiliary-history scans use total-order iteration. Audit and both migration paths require every referenced transaction and exact slot mapping in hot or cold storage. Focused cold migration 12/12, strict Core/Validator Clippy, the locked full workspace all-target/all-feature suite, and the final AP-050 four-validator gate all pass. Exact-final Linux build and live read-only repeat audit remain before execute. |
| AP-057R | Bound sparse Merkle cache storage and provide a stopped-node canonical rebuild | Repeated-update node-count/proof/checkpoint regressions, exact-tag Linux recovery build, EU/IN pre/post evidence | Done. Current Core passes 994/994. Exact-tag optimized Linux tests passed. Recovery binary `9b71e7a9...ccee` rebuilt EU at slot `8,953,695`, reducing contract-node SST bytes from `42,022,557,879` to `246,455,989`, and rebuilt IN at slot `9,180,291` to 249,527 reachable contract nodes. Typed verification passed on both; protected and cold-archive metadata hashes were unchanged. Checkpoints `8,954,000` and `9,181,000` pruned the obsolete hard-linked files, leaving about 58.7 GB and 70.1 GB free respectively. Ongoing boundedness is in the candidate and exact-tag replay bridge. |
| AP-058 | Restore EU runtime headroom and prove restart-loop suppression | Current provider backup mount, stopped rollback evidence, corrected total-order audit, bounded migration, sparse-cache rebuild, `df`, `getHealth.disk`, systemd status | Done without volume expansion. The guarded restore, full integrity audit, bounded hot-to-cold migration, derived-cache rebuild, root proof, and checkpoint rollover completed. Free space rose from 15.12 GB before rebuild to 58.72 GB after old-checkpoint pruning and remained about 53 GB in the July 15 live sample. EU caught up, serves fresh blocks through the authenticated edge, and `lichen-validator-testnet.service` is active/enabled with zero systemd restarts. The 500 GiB blocker is superseded. |
| AP-060 | Repair the current July chain on US, EU, SEA, and IN from verified source history | Per-range proof, per-host dry-run/execute evidence, fixed-tip manifests | Testnet-only legacy-loss exception accepted; backed-union repair still pending and remains a release gate. No current/provider copy contains bodies `2,872,006..4,298,999`; signed bodies must never be synthesized. The owner explicitly accepted only that irrecoverable interval on 2026-07-15 for upgrading the existing `lichen-testnet-1`. The July 16 read-only midpoint probe proved IN has slot `4,679,179` while US/EU/SEA do not, confirming known source-backed ranges still need additive replication. The stopped-fleet repair must union every proven EU/IN/US/current range, recovered US slot `5,275,999`, and SEA's recent bounded hole, then pass conflict-free fixed-tip manifests. Mainnet startup, joining, and snapshot import remain fail-closed on any absent, behind, or mismatched genesis-to-tip archive proof. |
| AP-065 | Recover four-validator live production and prove fresh-tip liveness | Same current slot/hash, fresh block age, all four producers observed | Done for liveness. US/EU/SEA/IN are active and enabled with zero systemd restarts; the July 15 read-only sample found healthy slots `9,295,963..9,295,967` at block age `0..1` seconds. US/SEA/IN execute signed `v0.5.223` hash `e956a8fb...2e810`; EU executes the audited exact-tag sparse-maintenance bridge `9b71e7a9...6ccee` through a temporary catch-up override while its installed canonical binary remains signed `v0.5.223`. The override and image mismatch must be retired by the coordinated AP-070 deployment. No `0.5.224` candidate is installed. AP-060 remains the separate full-genesis archive blocker. |
| AP-066 | Remove the public RPC single-origin failure mode | Four authenticated origins, strict fleet health, failover tests, explorer same-origin smoke | Done. `testnet-api.lichen.network` is canonical. `edge/testnet-rpc` routes across US/EU/SEA/IN with unique origin credentials, bounded body replay, deterministic affinity, and failover. Caddy is active/enabled on every origin, direct unauthenticated origin requests return 403, Worker tests pass 7/7, frontend assets pass 377/377, all ten Pages projects are live on the canonical configuration, RPC/WS smokes pass, and a 24-request sample reached every region. |
| AP-067 | Make every committed Rust graph independently reproducible and bound contract build-cache growth | Fresh lock regeneration, locked metadata/audit, standalone compiler/SDK/fuzz tests, dependency policy QA | Done. All 40 lockfiles regenerate with the exact PQ-compatible PKCS#8 prerelease and pass locked metadata plus cargo-audit. Cargo-deny passes. Compiler 30/30, contract SDK 28/28, Rust client SDK 88/88, all examples, all 14 fuzz targets, and all 34 contract workspaces pass native tests and locked WASM builds. The 34 comprise 32 genesis catalog contracts, the genesis-bound `launchpad_token` graduation template, and the standalone `mt20_token` template. The final locked workspace all-target/all-feature suite, strict workspace Clippy with `-D warnings`, compiler sandbox, and Rust SBOM generation also pass. CI/release enforce the manifest anchor and shared non-runtime contract target directory. |
| AP-068 | Recover IN disk headroom without deleting canonical state or public history | Protected hash comparison, typed sparse proof, checkpoint pruning, final fleet preflight | Done. IN stopped cleanly at `9,180,291`; the audited derived-cache rebuild passed, protected/cold metadata hashes matched, checkpoint `9,181,000` released obsolete cache files, free space rose to about 70.1 GB, and the clean-exit four-host verifier passed at a 46-slot spread with identical fixed-block digest. |
| AP-069 | Make archive-backed hot/cold retention an automatic public-network invariant | Runtime/admin flag rejection tests, environment contracts, four-validator archive gate | Done in source. Every non-dev testnet/mainnet runtime and public-history admin command derives `archive-<network>` beside `state-<network>`, enables archive retention automatically, and rejects `--archive-mode` or `--cold-store`. Disposable `--dev-mode` clusters retain explicit controls. The production verifier and stream-repair path pass neither flag; env contracts and maintained runbooks use no public archive opt-in. |
| AP-071 | Prevent post-checkpoint LiveSync stalls and repair SEA's bounded July 15 archive gap | Same-process material-gap recovery, progress-watchdog tests, source range proof, fixed-tip fleet manifests | Code and exact local gates done; live repair is part of AP-070 deployment. Material gaps and every checkpoint-repair entry return to bounded InitialSync, and only canonical or verified snapshot progress feeds the watchdog. The final four- and ten-validator gates passed the 140-slot same-process pause and identical checkpoint-3000 manifests. SEA range `9,236,790..9,237,999` remains to be additively repaired from US/EU/IN before final live parity. |
| AP-072 | Prevent binary ABI marker collisions from corrupting nested caller identity or custody arguments | Canonical descriptor parser, explicit SDK layouts, nested caller regression, preserved-chain recovery, exact release gates | Done. Layout mode requires descriptor/signature/payload agreement instead of a leading `0xAB` byte. The old DEX WASM succeeds on the preserved failed database under the corrected runtime, focused Core/SDK/DEX tests pass, and exact four- and ten-validator release gates pass. |
| AP-073 | Preserve historical native transaction replay while activating strict chain-domain signatures and repair custody mint signing | Exact live block `9,273,160`, activation-boundary unit/execution tests, custody cross-chain rejection, full local gates | Done locally. Core 998/998 and custody 172/172 pass. Below the shared durable consensus-v1 slot, execution reproduces the bounded `v0.5.223` chain-domain-then-legacy transition policy; at/above activation it requires strict chain-ID verification. Malformed activated metadata fails closed. The operator command is `--prepare-consensus-v1-activation` with no legacy alias. The exact four-validator gate passed 140/140 volume journeys, 104/104 launchpad journeys, same-process gap recovery, own-state restarts, and root `b7232593...1bae1de`; the exact ten-validator expansion passed 8/10 liveness, preserved-state recovery, and root `48cb5614...649255`. Workspace, strict Clippy, dependency, contract, SDK, frontend, and static gates pass; hosted CI remains part of AP-070. |
| AP-074 | Make the hosted archive gate self-contained instead of relying on workstation files or dependencies | Clean tag checkout, recursive JavaScript dependency closure, Git tracking/ignore/lock assertions, exact four-validator rerun | Done locally. The first `v0.5.224` tag attempt passed consensus recovery, certificate parity, and four-node hot/cold manifest parity, then correctly failed before volume journeys because `tests/e2e-volume.js` was absent from the clean checkout. A prior repository cleanup had ignored and untracked `tests/`; only the shell harness was later force-added. The volume and launchpad suites plus their five helper modules are now explicit tracked release inputs. The second tag attempt proved those assets and reached identical four-node checkpoint-1000 manifest root `9ce0e454...71e`, then failed before the first journey because that separate release job had not installed `ws` or `@noble/post-quantum` from the root lockfile. The job now installs Node.js 22 and runs `npm ci --ignore-scripts` before the harness. `test_archive_parity_gate_assets.js` recursively verifies every local `require`, discovers bare `require` and dynamic `import` packages, proves they are declared and locked, checks file tracking and ignore policy, and enforces dependency installation before the release harness. The clean-closure local rerun passed same-process 140-slot recovery, own-state and coordinated restarts, 140/140 volume journeys, 104/104 launchpad/governance/graduation journeys, and four-node checkpoint-3000 manifest root `7609ff7a...6bc1376`; transcript SHA-256 `eac37545...e5fd9f66`. Neither failed tag attempt created a release or distributable artifact. |
| AP-070 | Cut new tag and publish release artifacts | Tag, checksums, crates/npm status | Blocked on hosted CI, signed artifacts, complete source-backed fleet-union repair, coordinated deployment/activation, fixed-tip fleet parity, and package publication. AP-058, AP-065 through AP-069, and AP-073 local release gates are complete. Mainnet release remains prohibited until a fresh chain proves complete genesis-to-tip archives without any waiver. |

## Release Acceptance

A new testnet release is not production-ready until:

- local four-validator archive parity passes after stop/restart/resume;
- a ten-validator expansion passes coordinated restart and continues finality
  with two validators stopped before both rejoin from their own state;
- every VPS returns the same historical RPC results for the selected genesis,
  early, mid-chain, recent, and tip probes;
- archive manifests match for the repaired slot ranges;
- no validator has unique public history that the others cannot provide;
- no slot range is missing from every backed source in the validator set (the
  explicit `lichen-testnet-1` AP-060 legacy-loss waiver is non-transferable and
  is not valid for mainnet or any fresh network);
- the runbook contains the exact parity command set used for future deploys.
