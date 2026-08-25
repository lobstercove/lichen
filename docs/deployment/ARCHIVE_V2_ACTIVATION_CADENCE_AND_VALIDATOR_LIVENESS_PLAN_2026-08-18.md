# Archive V2 Activation, Cadence Recovery, And Validator Liveness Plan

**Date:** 2026-08-18
**Last updated:** 2026-08-25
**Status:** Authoritative execution plan; signed v0.5.259 is live on all four
validators with Archive V2 roles temporarily disabled, its moving-tip rejoin
gate failed live, v0.5.260 qualification is in progress, and final Archive V2 tail extension, role
activation, legacy retirement, and R2 cleanup remain open
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

The fleet has safely recovered from the frozen v0.5.255 boundary but is not yet
in the intended final state.

- Signed v0.5.259 is installed and running on all four hosts from the exact
  release-workflow artifact. Every validator preserves its own key, signer,
  WAL, and state; all four serve the same advancing chain with zero service
  restarts. The live cadence still has approximately 300-400 ms medians but
  unacceptable p99 and multi-second tails, so cadence acceptance remains open.
  Signed v0.5.258 remains the coordinated rollback baseline until v0.5.260
  passes moving-network rejoin acceptance.
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
- The Explorer's `Observed ... ms avg` value is not an arithmetic average. It
  is the upper median of at most 120 observer-side normalized block-arrival
  samples. It can therefore move from roughly 300 ms to roughly 1,000 ms when
  a sustained timeout burst occupies more than half of the rolling window.
- The Archive V2 binary and role implementation are present. All four runtime
  roles are temporarily disabled because the former 317-segment catalog is
  stale relative to the current tip; signed admission correctly fails closed
  rather than claiming incomplete genesis-to-tip-minus-headroom coverage. The
  intended bounded testnet matrix remains US/EU `verified_cache` and SEA/IN
  `consensus`. A mainnet launch additionally requires approved persistent
  `full_archive` capacity in at least three independent failure domains.
- The existing read-only FUSE SST mounts are an emergency, dual-R2-backed
  legacy archive offload. They are not the final full-archive,
  verified-cache, and consensus role topology.
- Current disk free space is runway, not completion. The 2026-08-23 read-only
  audit measured approximately 41.7 GB free on US and 15.8-16.5 GB on the other
  three hosts. EU and SEA still carry roughly 83-89 GB of legacy archive each,
  in addition to roughly 84-86 GB of hot state. Those bytes are reclaimed only
  after fleet-wide Archive V2 admission and fixed-tip parity, not before.

Accordingly:

1. Do not call the recovery 100% complete while recurring BFT timeout bursts
   remain unexplained or while Archive V2 role activation is disabled.
2. Do not start reclaim attempt 799 or another emergency FUSE batch merely to
   conceal the capacity constraint.
3. Complete the v0.5.260 hard gates, publish it through the signed release
   workflow, verify its detached post-quantum checksum signature, and deploy it
   through one coordinated four-host stop/install/start.
4. Extend the canonical Archive V2 tail from a stopped immutable hot snapshot,
   activate the exact role matrix, prove fixed-tip parity and cadence, and only
   then retire legacy rows/FUSE bridges and reclaim disk.
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
This rules out a direct Archive V2 state-read regression. The later live A/B
recovery also ruled it out as the cause of the halt: holding the high-frequency
oracle ingress restored cadence without changing the Archive V2 canary, source
mounts, or legacy archive layout. Archive and checkpoint work can still amplify
host pressure and must pass isolation gates, but the reproduced liveness defect
is an unbounded proposal workload fed by overly aggressive oracle defaults.

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
but activation remains blocked until v0.5.260 passes rejoin acceptance, the
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

Current execution order is fixed:

1. Keep the now-converged signed v0.5.259 fleet advancing with Archive V2 roles
   disabled; immediately before the transition, prove a common recent
   slot/hash again.
2. Finish every v0.5.260 local and repository hard gate, merge the clean commit,
   publish the signed tag, and verify release artifacts and rollback artifacts.
3. Install only signed v0.5.260 artifacts in one coordinated four-host
   stop/install/start, then prove convergence, artifact parity, effective
   30/60/10 oracle defaults, bounded proposal transaction counts, and no
   mempool accumulation.
4. Run the moving-network outage/rejoin canary: stop one validator while the
   other three continue, restart it, and prove automatic BFT re-entry, commits,
   and authorship without freezing the surviving quorum.
5. Stop the selected snapshot source, capture its final WAL boundary only after
   the stop, and select it by an exact source-backed range audit. Create the
   bounded immutable hot snapshot only from that proven complete validator;
   never assume the public seed is the complete source.
6. While the network runs, build and dual-publish only the missing catalog tail
   from the stopped snapshot, then stage the resulting catalog on every host.
7. Perform one coordinated role activation as US/EU `verified_cache` and SEA/IN
   `consensus`; prove deep-history parity, source-loss isolation, fixed-tip
   equality, restart, and outage behavior.
8. Retire legacy cold rows and emergency FUSE bridges in bounded host order,
   verifying BFT, capacity, and cadence after each unit. Restore checkpoint
   generation once each host exceeds its safety floor.
9. Re-inventory R2 only after bridge retirement, seal the exact obsolete-object
   deletion manifest, obtain explicit operator approval for that manifest
   SHA-256, delete only those keys, and prove retained canonical inventory
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
US/EU `verified_cache`, SEA/IN `consensus`. Activation uses the newly extended
catalog and the normal 100,000-slot hot window. US's temporary 200,000-slot
bridge is not the final configuration. Start cache roles first, then consensus
roles, and require all four to share a post-transition fixed slot/hash before
retirement. This keeps every voting process on the Archive V2 admission/read
path without falsely advertising any current VPS as a local full archive. The
later dedicated archive plane removes historical RPC from voting hosts
entirely.

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
