# Archive V2 Segmented Storage And Validator Roles Plan

**Created:** 2026-07-21
**Status:** Owner-approved architecture direction; v0.5.227 emergency/restart-recovery candidate pending signed release gates; Archive V2 implementation pending
**Scope:** Testnet, future mainnet, archive-capable validators, constrained validator agents, historical RPC, sync, backup, recovery, and capacity operations
**Related policy:** [TESTNET_STATE_AND_SYNC_POLICY.md](TESTNET_STATE_AND_SYNC_POLICY.md)
**Current incident baseline:** [ARCHIVE_PARITY_REPAIR_PLAN_2026-07-09.md](ARCHIVE_PARITY_REPAIR_PLAN_2026-07-09.md)

## 1. Executive Decision

Lichen will replace the indefinitely growing RocksDB-only public-history layout
with a versioned, immutable, content-addressed segmented archive. Current
consensus state and a bounded recent-history window remain in the hot RocksDB.
Finalized older history moves into independently verifiable, seekable Zstandard
segments with compact indexes. Historical RPC reads remain transparent.

The design supports three explicit validator roles:

1. **Full archive validator:** holds current consensus state and every verified
   archive segment locally.
2. **Verified-cache validator:** holds current consensus state, recent history,
   all archive manifests/index roots, and a bounded local segment cache; older
   segments are fetched from authenticated peers or object storage and verified
   before use.
3. **Consensus validator:** holds the state and recent history required for
   validation, production, replay, and safe recovery, but does not advertise
   deep historical RPC service.

The public RPC fleet must always contain enough independent full archive
replicas to survive the configured failure budget. A constrained agent may
participate in consensus without pretending to be a complete local archive.

Compression is not a substitute for capacity. If every validator must
physically retain every historical byte forever, every validator requires an
ever-growing disk. Archive V2 reduces the growth factor, removes avoidable
duplication, supports verified remote access, and makes capacity predictable;
it cannot make unbounded history fit permanently on a fixed disk.

## 2. Immediate Emergency Bridge

The live `lichen-testnet-1` fleet reached the signed v0.5.225 10 GiB runtime
floor. The owner has explicitly authorized this temporary bridge while Archive
V2 is implemented. v0.5.226 contains the storage settings but was deliberately
not deployed after the restart defect in section 2.1 was found; v0.5.227
supersedes it before the fleet changes:

- lower the **testnet-only** runtime safety reserve from 10 GiB to 5 GiB;
- keep the production/mainnet default at 10 GiB until the adaptive guard in
  this plan is implemented and approved;
- reduce the default hot-history retention window from 100,000 to 50,000 slots;
- use the existing source-preserving, write-first, WAL-synced, bounded
  hot-to-cold migration path;
- migrate one stopped validator at a time from a fixed recorded cutoff;
- never replace a live binary with a locally built candidate;
- deploy only a signed release artifact whose installed and running hashes
  match on every validator;
- do not delete state, archives, WAL, keys, identities, rollback artifacts,
  access configuration, provider backups, or unique incident evidence.

The 5 GiB threshold is a temporary availability trade-off. It is not a mainnet
capacity approval, not permission to consume the final bytes, and not a durable
substitute for larger disks. Operators must alert before 8 GiB and treat 6 GiB
as an immediate capacity incident so that the validator is not routinely run at
the new hard floor.

The 50,000-slot change moves more old block/transaction/index data from the hot
LZ4-oriented database into the Zstd cold database. It does not rewrite the
existing 169-173 GB cold archive into a smaller representation. Archive V2 is
required for that larger saving.

The bridge acts only on families the current cold-store reader supports. The
stopped-node bounded command migrates and compacts blocks, transactions, and
transaction-to-slot rows. After that validator restarts, the runtime separately
migrates eligible account-transaction history, account snapshots, events,
token transfers, and program calls. The other public-history categories remain
in their current compatible storage until Archive V2 implements and verifies
their segment indexes. Operators must report physical `df` change by family and
must not describe the 50,000-slot boundary as having compacted all 21 manifest
categories.

### 2.1 Restart incident discovered during the bridge

The US seed/public-RPC validator exited correctly when available space fell
below the old 10 GiB hard floor. EU, SEA, and IN retained finality. Explorer
availability briefly followed the failed US origin before the public edge
failed over to the three healthy RPC origins. Safe cleanup was limited to
bounded journals, package caches, and generated diagnostics; validator state,
the cold archive, WAL, keys, identities, access configuration, rollback
artifacts, and provider backups were preserved.
The provider backups remain mounted read-only/no-recovery: US's retained backup
is from July 9 at tip `8,525,115`, and the other retained backups are from July
12. They are recovery assets, not free-space targets. After bounded cleanup,
the healthy hosts had roughly 11-13 GB available; US had 12,065,382,400 bytes
available at the latest read-only fleet check.

Restarting US on the exact signed v0.5.225 binary exposed a separate defect.
The startup post-effects cursor was initialized to `activation_slot - 1`, but
the initial-scan test used strict `<` rather than `<=`. Equality therefore took
the resume branch and began scanning at the activation slot instead of the
bounded recent window. Markers for old blocks had been intentionally pruned, so
startup treated old economic effects as missing and replayed 137,711 stored
blocks. This was not a cold-migration or fixed-tip defect. It was a restart-only
post-effects recovery boundary error.

US then rejected canonical child slot `9,830,992`. That child's authenticated
parent certificate requires post-state root
`937e9d82772121e1f8a180c7bc3e7edaeb2a2538649d9c3e0287fb084078ec4c`,
while US computed
`24084cf1e8b1094bc5c77515d272a8fe1b0f2d98f04ca2b4dcedee3001dc51cb`
for parent slot `9,830,991`. The fixed parent block hash is
`6620bb63abec9f52897d97576ead011c31bd913eebc030000131c68c9494bdf0`.
Its header pre-effects root is
`0526d5ea50f3098b11c2ceac1f5240a664cef9881941a5e3f0999384b0b37c1b`,
its producer is SEA, its timestamp is `1,784,639,918`, and it contains one
consensus transaction with zero user fee.
All three healthy validators returned identical slot-bound account values. A
disposable diagnostic checkpoint proved the complete correction below produces
the exact child-certified root; it was then deleted, leaving no multiply-linked
diagnostic SST files and making no write to the live database.

| Account | US before spores/spendable | Certified value | Delta |
| --- | ---: | ---: | ---: |
| Treasury `6JhhxYKc5tmXMttnrCNTCPnMkMWRQ96US3LtNRiFJjW` | 49,599,406,026,359,855 | 49,599,406,026,859,855 | +500,000 |
| EU `6RMeoigHdJWB47pEZEMSj5gvT7nbJPYSfPqjcur9vMJ` | 592,701,782,223,010 | 592,701,782,198,010 | -25,000 |
| SEA `6TghL7ioQz5R8pfrX1Qcfy8rNMzRP5F2pndmmRQ2sPm` | 592,708,284,068,010 | 592,708,283,743,010 | -325,000 |
| IN `6XhsGituXoWSd1wLtutZgdJve6gLrdSi7YhEx1ZDFHW` | 395,395,283,317,961 | 395,395,283,292,961 | -25,000 |
| US `7LFPJ8gqmAtjbhfRg1P4VXmTQJV4AeZxzws3UsA6SVq` | 592,702,797,313,010 | 592,702,797,288,010 | -25,000 |
| Community `8i6Y9q1i2bKJwBXfzWrAfKMwbdeZxFxH3U4HJRJEEri` | 123,900,002,499,080,000 | 123,900,002,498,980,000 | -100,000 |

The six deltas sum to zero. SEA's stake entry changes from reward slot
`9,817,994`, `2,666,367` produced blocks, and
`592,708,279,052,495` total claimed to reward slot `9,830,991`, `2,665,940`
produced blocks, and `592,708,278,752,495` total claimed. The before stake-pool
hash is
`b8baf242cca480cce86de1e2043ddf8be25893770d48ea4bce7abe480bf19d85`.
For the production-counter proof, EU's observed comparison tip was `9,833,704`.
Canonical slots `9,830,992..9,833,704` contained 907 EU, 885 SEA, 921 IN, and
zero US blocks. Healthy SEA reported `2,666,825` at that later tip, so subtracting
the 885 post-target blocks gives the fixed target `2,665,940`; US's drifted
value was 427 too high. The claimed-reward delta is independently fixed by the
six source-corroborated account deltas and the complete root projection.
The account root after correction is
`352937863a0badbccb4f433a9689dd9455e2768e5aa2dbc22919bcb515a25955`;
the complete state root is the authenticated root above.

The healthy fleet's certified target retains the one historical compatibility
application that occurred when the three surviving databases first crossed the
v0.5.225 recovery boundary. The US incident was a second application on a later
restart. The emergency repair removes only that second drift so US matches the
already-certified chain; it does not invent a counterfactual root or rewrite
signed history.

The replay also advanced non-root validator activity/reputation counters on US.
Those fields do not contribute to the authenticated state root, validator stake
power, leader selection, or the account/stake correction above, so they must
not be guessed inside the emergency root repair. After all four validators are
live, export the complete operational validator profile at a fixed slot,
reconstruct any target values from canonical block producers and commit
signatures, and reconcile only with a separately reviewed source-bound command.
Until that audit closes, activity lifetime counters are not a fleet-parity
claim; fixed-slot state roots, certificates, stake powers, fresh activity, and
public-history manifests remain the recovery gates.

v0.5.227 makes the initial scan condition inclusive, factors the boundary
choice into a tested helper, and adds a one-purpose stopped-node repair. The
repair is restricted to chain ID `lichen-testnet-1`, exact slot and block hash,
exact before state and stake-pool hashes, and the six exact before images. A
dry run must project the authenticated after root. Execution requires explicit
confirmation, stages all accounts, account snapshots, and stake state in one
RocksDB batch, rebuilds the sparse commitment, restores the parent sidecar
anchor, and verifies the after root. Unknown or partial states fail closed. A
second execute pass completes any interrupted derived-cache/anchor work and is
otherwise idempotent.

The deployment consequence is strict: do not deploy or restart v0.5.226, and
do not restart healthy v0.5.225 validators before the corrected signed binary
is staged. v0.5.227 requires the full release gates and a coordinated stop so no
mixed recovery behavior enters consensus. US remains stopped until its signed
v0.5.227 dry run proves the exact target. The repair uses US's own preserved
state; no peer state snapshot or validator database is copied.

### 2.2 Completed bounded US hot-to-cold preparation

With US stopped at tip `9,830,991`, the fixed 50,000-slot cutoff was
`9,780,991`. The preflight found 50,000 eligible hot blocks and 101,545
eligible transaction/index rows, with zero decode, hash, cursor, integrity, or
cold conflicts. Five write-first batches migrated all 50,000 blocks. The
post-run dry audit found zero eligible rows and zero errors/conflicts, and
available space rose from 11,249,733,632 to 12,066,140,160 bytes. This is
bounded preparation only; runtime migration of the other currently supported
history families still has to be observed after the signed restart.
At that point US used approximately 3.96 GB for hot state and 174.86 GB for its
cold archive. The size is why the 50,000-slot bridge cannot replace Archive V2:
it reduces hot duplication and working pressure but does not re-encode the
existing large cold RocksDB into immutable deduplicated segments.

## 3. Current Storage Model And Its Limits

### 3.1 Current hot database

The hot RocksDB contains both consensus-critical current state and a mixture of
recent and historical public data. Current-state families include accounts,
contract storage, validator/stake state, restrictions, shielded state, program
state, token balances, DEX order state, and state-commitment caches. Public
history includes blocks, transactions, slot mappings, account activity,
events, transfers, program calls, EVM history, shielded history, NFT activity,
market activity, DEX trades, transaction metadata, and account snapshots.

Most hot families use LZ4 because the active state favors low-latency reads and
writes. Old public data is eligible for migration after the configured hot
window.

### 3.2 Current cold database

The current cold RocksDB uses Zstandard compression and transparently backs old
reads. It currently mirrors these families:

- blocks;
- transactions;
- transaction-to-slot mappings;
- account transaction history;
- account snapshots;
- events;
- token transfers;
- program calls.

The runtime writes cold data first, checks an existing key byte-for-byte,
flushes the cold WAL, and only then deletes the hot copy. Historical block and
transaction reads fall through from hot to cold automatically.

### 3.3 Why the cold RocksDB remains large

- A block contains its transaction bodies, while transaction bodies are also
  stored independently for direct transaction lookup.
- Multiple public indexes retain keys or payloads that can instead reference a
  canonical segment location.
- Account snapshots and activity histories contain repeated prefixes and state
  values that can be delta encoded.
- LSM trees retain bloom filters, indexes, manifests, WALs, level overlap, and
  compaction write amplification.
- Keys such as block hashes are not ordered chronologically, so a slot range is
  not one contiguous RocksDB key range.
- A normal full compaction creates replacement SST data before obsolete SSTs
  are unlinked. It cannot safely rewrite a 170 GB archive with only a few GiB
  free.
- Existing cold rows are mostly append-only and idempotently inserted, so an
  ordinary compaction may have little obsolete-value garbage to reclaim.

### 3.4 Current public-history parity surface

Archive V2 must preserve the current canonical public-history manifest surface:

1. `slots`
2. `blocks`
3. `transactions`
4. `tx_by_slot`
5. `tx_to_slot`
6. `tx_meta`
7. `account_txs`
8. `events_by_slot`
9. `events`
10. `token_transfers`
11. `program_calls`
12. `evm_txs`
13. `evm_receipts`
14. `evm_logs_by_slot`
15. `shielded_txs`
16. `nft_activity`
17. `market_activity`
18. `dex_trades_by_pair`
19. `dex_trades_by_taker`
20. `dex_trades_by_pair_taker`
21. `account_snapshots`

State snapshot and join correctness also cover the broader current-state
category set in `STATE_SNAPSHOT_CATEGORIES` plus validator set, stake pool, and
MossStake pool domain snapshots. Archive V2 does not narrow that state surface.

## 4. Requirements And Invariants

### 4.1 Data integrity

- Every archived block must retain its exact canonical signed body and header.
- Parent linkage, block hash, transaction Merkle root, state root, oracle data,
  fee data, and canonical consensus envelope must verify before promotion.
- Node-local commit-certificate presentation fields may differ where current
  parity policy permits, but canonical body identity must not differ.
- No row may be synthesized to fill missing history.
- The existing `lichen-testnet-1` legacy-loss waiver remains explicit and does
  not transfer to a fresh testnet, mainnet, or another network.
- Same-key semantic differences abort conversion or import.
- Conversion is additive, idempotent, resumable, and conflict-aborting.
- No original row is retired until a verified replacement is durable and the
  configured replica acknowledgement policy is satisfied.

### 4.2 Consensus isolation

- Archive segment creation and reads must not change deterministic state
  transition results.
- Segment manifests are operational/history commitments, not a host-local
  input into proposer selection, voting, rewards, timing, or membership.
- A historical RPC failure must not mutate consensus state.
- Current state, validator keys, identity, signer state, consensus WAL, and
  recent blockhash data remain local and independently owned by each validator.

### 4.3 Availability

- One failed archive validator must not make the public RPC edge unavailable.
- A full archive validator must answer every backed historical request locally.
- A verified-cache validator may answer after retrieving and verifying the
  required frame; it must report bounded `archive_fetching` or a typed upstream
  error rather than returning false `not found`.
- A consensus-only validator must not advertise archive readiness.
- The edge must route deep historical RPC only to an origin that advertises the
  required local or fetch-capable archive role.

### 4.4 Rollback and compatibility

- The segment format is explicitly versioned.
- A reader must reject unsupported major versions and unknown mandatory flags.
- During migration, the new release reads legacy hot/cold RocksDB and Archive
  V2; the old signed rollback continues to read the preserved legacy store.
- Legacy cold rows remain intact throughout the rollback window.
- An irreversible legacy deletion is prohibited until a new signed rollback
  anchor that reads Archive V2 is published, deployed, and recovery-tested.
- RocksDB SST format remains pinned to the current rollback-compatible format
  while the v0.5.223 engine remains a supported rollback source.

## 5. Target Storage Architecture

### 5.1 Layer A: consensus state

Each validator retains an independently writable local RocksDB containing:

- current accounts and balances;
- current contract storage and deployed programs;
- validator, stake, reward, governance, restriction, shielded, and DEX state;
- current deterministic state-commitment data;
- consensus activation markers and canonical current cursors;
- current and recent transaction admission data;
- consensus WAL and validator-owned sidecars.

Consensus-state storage must have a reserved capacity pool that archive writes
cannot consume.

### 5.2 Layer B: recent hot history

The hot database retains a configurable number of recent finalized slots. The
emergency target is 50,000 slots. Archive V2 will make the default a policy
value measured in both slots and time/bytes, with a minimum required replay and
blockhash window.

Recent history remains optimized for:

- block propagation and catch-up;
- recent transaction and account queries;
- reorg/fork and checkpoint repair boundaries;
- DEX, explorer, wallet, and exchange low-latency paths;
- segment construction before a range is sealed.

### 5.3 Layer C: immutable Archive V2 segments

Finalized ranges older than the hot window are encoded as immutable segment
sets. A logical segment covers a fixed slot interval and consists of a manifest,
data frames, and index files. Files are content-addressed and never modified in
place.

Suggested initial sizing, subject to benchmark:

- logical segment span: 50,000 slots;
- independently compressed frame target: 4 MiB uncompressed;
- maximum physical object target: 2 GiB so object stores, mirrors, and repair
  tools do not require very large atomic transfers;
- Zstandard level: 9 for the initial benchmark baseline;
- optional 64-128 KiB trained dictionary identified by hash in the manifest;
- checksum: SHA-256 for object identity and a fast per-frame checksum for early
  corruption detection;
- integrity tree: Merkle root over ordered frame hashes and index hashes.

No compression level, dictionary, or frame size becomes permanent until the
benchmark gate compares at least levels 3, 6, 9, 12, and 15 and frame sizes
1, 4, and 16 MiB on representative early, middle, and recent history.

### 5.4 Layer D: manifests and catalog

Every validator retains the complete, small manifest catalog locally even when
segment payloads are cached remotely. The catalog maps network and slot ranges
to immutable segment roots and contains continuity commitments.

The catalog is append-only. Replacing a segment requires a versioned superseding
record that points to the prior manifest, proves the same logical history, and
passes fleet approval. Silent mutation is forbidden.

### 5.5 Layer E: optional verified remote backing

A verified-cache validator may retrieve immutable objects from:

- another full archive validator;
- a dedicated archive gateway;
- versioned object storage;
- an operator-approved local network mirror.

The source is never trusted for correctness. Object hash, manifest membership,
network ID, slot range, and block/transaction commitments are verified locally.
TLS and authenticated transport protect availability and confidentiality of
operational metadata; content addressing protects data correctness.

## 6. Segment Format

### 6.1 File set

Each segment directory or object prefix contains:

```text
manifest.cbor
frames/00000000.zst
frames/00000001.zst
...
indexes/slot.idx
indexes/block-hash.idx
indexes/transaction.idx
indexes/account-history.idx
indexes/event.idx
indexes/program-call.idx
indexes/token-transfer.idx
indexes/evm.idx
indexes/shielded.idx
indexes/nft-market-dex.idx
indexes/account-snapshot.idx
COMPLETE
```

`COMPLETE` is written last and contains the manifest hash. A directory without
a valid `COMPLETE` marker is staging data and is never served.

### 6.2 Manifest fields

The canonical manifest includes at least:

- magic and format major/minor version;
- network/chain ID and genesis hash;
- first and last slot, inclusive;
- first block hash, last block hash, and predecessor hash;
- finalized slot and canonical archive-watermark evidence used to build it;
- canonical row counts by category;
- legacy-loss declarations, if and only if allowed for the existing testnet;
- serialization schema identifiers;
- compression algorithm, level, window, dictionary hash, and frame target;
- ordered frame names, compressed/uncompressed lengths, checksums, and hashes;
- ordered index names, schemas, lengths, checksums, and hashes;
- frame/index Merkle root;
- canonical block-range digest;
- public-history category digest map;
- builder release, source database identity, and build timestamp as
  non-consensus provenance;
- prior segment manifest hash and prior last block hash;
- optional fleet-attestation set;
- manifest hash and detached PQ signature where operator policy requires it.

Provenance timestamps and host names are excluded from canonical logical-history
identity. The same history built independently must produce the same canonical
root even when provenance differs.

### 6.3 Canonical block record

A block body is stored once. The record contains the exact canonical block
encoding required to reproduce `getBlock`, followed by a compact record table
that identifies transaction positions and optional per-block public index
fragments.

The independent legacy `transactions` body is not repeated. A transaction hash
index resolves to `(segment, frame, block_record_offset, transaction_ordinal)`.
`getTransaction` decompresses the relevant frame, validates the block record,
and extracts the transaction at that ordinal.

### 6.4 Frame encoding

Each frame contains:

- frame magic/version;
- segment logical ID;
- first/last slot included;
- ordered block record count;
- offset table for records;
- canonical block records;
- optional local index fragments;
- uncompressed payload hash.

Frames are independently compressed. A reader never needs to decompress a full
50,000-slot segment for one query. Oversized blocks receive a dedicated frame.

### 6.5 Index encoding

Indexes contain references, not duplicated canonical payloads.

- Slot index: delta-encoded slot to frame/record offset.
- Block-hash index: hash to slot/frame/record offset.
- Transaction index: hash to slot and transaction ordinal.
- Account history: account prefix plus delta-varint slot/transaction references.
- Events/transfers/program calls: typed key prefix plus delta-varint record
  references and minimal query-order metadata.
- EVM/shielded/NFT/market/DEX indexes: canonical query key to record references.
- Account snapshots: periodic full anchors plus ordered deltas where exact
  semantic equivalence is proven; otherwise exact snapshot records remain.

Indexes are deterministic and rebuildable from canonical segment content where
possible. An index may be discarded and rebuilt only if its rebuild proof
matches the manifest digest. Canonical block bodies are never treated as a
disposable derived index.

### 6.6 Historical state strategy

Current state remains fully materialized. Historical account inspection uses
one of these exact representations, selected only after equivalence tests:

1. exact archived account snapshots;
2. periodic account anchors plus ordered deterministic deltas;
3. segment-local state-change journals with verified anchor roots.

The initial Archive V2 release should preserve exact snapshot semantics. Delta
conversion is a later optimization because historical balance and ownership
queries must remain byte-for-byte equivalent before old snapshots are retired.

## 7. Build, Seal, And Promotion Protocol

### 7.1 Eligibility

A range is eligible only when:

- every slot is finalized and older than the hot retention boundary;
- the complete range is canonical and parent-linked;
- every block-referenced transaction and public index row is available;
- the archive watermark covers the range;
- no unresolved state repair, snapshot activation, or rollback is active;
- enough staging and runtime headroom is available.

### 7.2 Build sequence

1. Acquire the archive-maintenance lock without blocking canonical block
   application longer than a bounded metadata snapshot.
2. Record fixed first/last slots, hashes, state root, archive watermark, and
   source category digests.
3. Stream canonical rows through total-order or slot-bounded readers.
4. Encode frames and indexes into a unique staging directory.
5. Fsync each completed object and its parent directory.
6. Compute object hashes, category digests, and the segment Merkle root.
7. Re-read and validate every staged object through the production reader.
8. Compare the staged logical manifest against at least one independent
   validator or the fixed-tip fleet proof during release/migration.
9. Write and fsync `manifest.cbor`.
10. Write and fsync `COMPLETE` last.
11. Atomically rename the staging directory into the content-addressed path.
12. Update and fsync the local catalog atomically.
13. Replicate and collect the configured acknowledgements.
14. Only then mark legacy rows eligible for bounded retirement.

### 7.3 Retirement sequence

1. Confirm the promoted segment still verifies.
2. Confirm required independent replicas and backups still exist.
3. Confirm the rollback policy permits retirement.
4. Delete only rows proven represented in that segment.
5. WAL-sync deletes.
6. Flush affected hot/cold families.
7. Compact bounded key ranges with a measured transient budget.
8. Re-run category manifests and representative RPC queries.
9. Record freed allocated bytes, not only logical bytes.

### 7.4 Crash recovery

- Staging without `COMPLETE`: resume or remove only after inventory; never serve.
- Complete object missing catalog entry: verify and idempotently attach.
- Catalog entry missing object: report `archive_incomplete`; fetch a verified
  replica; never return false `not found`.
- Crash during legacy deletion: segment remains authoritative; idempotent
  retirement resumes from the durable progress cursor.
- Hash mismatch: quarantine the object, fetch another replica, and produce an
  incident record.
- Replica disagreement: stop retirement and run fixed-tip parity; do not choose
  a majority payload without validating canonical chain commitments.

## 8. Read Path And RPC Semantics

### 8.1 Lookup order

1. Hot RocksDB.
2. Local legacy cold RocksDB during compatibility period.
3. Local Archive V2 segment.
4. Verified local cache populated from an approved remote source.
5. Typed unavailable/fetching response according to RPC method policy.

During cutover, a debug parity mode reads both legacy cold and Archive V2 and
compares canonical results without serving duplicate work to ordinary clients.

### 8.2 Verification

Before decoding a fetched frame, the reader verifies:

- object hash;
- manifest hash and network identity;
- frame membership in the manifest Merkle root;
- frame uncompressed payload hash;
- slot and block hash requested;
- block/transaction canonical commitments.

Verification failures are never translated into `Block not found`.

### 8.3 Caching

- Cache key is immutable object hash.
- Cache entries are written to a temporary file, verified, fsynced, and
  atomically renamed.
- An LRU/clock policy enforces a byte quota.
- Pinned in-flight frames cannot be evicted.
- Cache eviction never affects the manifest catalog or current state.
- Negative cache entries have short TTLs and distinguish missing source,
  timeout, and integrity failure.

### 8.4 RPC compatibility

The following must remain semantically unchanged:

- `getBlock`;
- `getTransaction`;
- `getTransactionsByAddress`;
- explorer slot/range/history endpoints;
- account history and historical balance inspection;
- events, transfers, program calls, EVM receipts/logs, shielded history,
  NFT/market activity, and DEX trade/candle-backed queries;
- public-history manifests and fixed historical probes.

Latency may differ by role. RPC responses should expose archive role, local
cache status, and fetch timing in health/metrics, not in consensus payloads.

## 9. Validator Roles And Admission

### 9.1 Full archive validator

Requirements:

- current state and recent hot history locally;
- every backed Archive V2 segment locally;
- complete manifest catalog;
- archive parity with the fleet;
- enough free space for two bounded segment frames, compaction peak, current
  state growth, and the adaptive reserve;
- historical RPC advertised.

### 9.2 Verified-cache validator

Requirements:

- complete current state locally;
- recent hot history locally;
- complete manifest catalog and trusted genesis/catalog anchor;
- at least two independent remote archive sources;
- configured local cache quota;
- no false claim that every segment is local;
- historical RPC advertised only as fetch-capable.

This role can independently validate every fetched byte. It reduces local disk
requirements without trusting an archive operator for chain correctness.

### 9.3 Consensus validator

Requirements:

- complete current consensus state;
- consensus WAL, identity, key, recent blockhash/replay window, and recent
  canonical blocks;
- ability to sync and recover from supported snapshots/peers;
- no deep-history RPC advertisement.

Consensus validator admission must not depend on an operator lying about
archive capacity. Role selection is explicit, signed/configured, and visible.

### 9.4 Public network policy

The current policy requiring every public validator to be a full archive node
remains in force until Archive V2 roles are implemented, tested, and explicitly
activated. The role design does not silently weaken v0.5.225 behavior.

Future network policy should require:

- at least three full archive replicas;
- at least two independent providers/failure domains;
- at least three regions;
- one versioned object-store replica;
- one offline or provider-native recovery copy;
- strict edge routing based on role/readiness;
- enough consensus validators to preserve finality when archive nodes are
  offline.

## 10. Replication, Backup, And Disaster Recovery

### 10.1 Replication acknowledgement

Legacy rows may be retired only after the configured policy confirms the same
segment hash on the required destinations. Testnet migration should require all
four current validators plus object storage while legacy rollback is retained.
Future production may define an explicit quorum, but no quorum replaces
canonical verification.

### 10.2 Object storage

- bucket versioning and deletion protection enabled;
- immutable/content-addressed object names;
- least-privilege write credentials kept outside validator processes;
- validators normally read with separate read-only credentials or signed URLs;
- lifecycle rules must not delete the last copy of an active manifest;
- inventory and restore drills run periodically;
- provider checksums are supplementary, not replacements for Lichen hashes.

### 10.3 Backup

RAID is not backup. Required recovery layers are:

- local RAID1 for disk-failure continuity on dedicated full validators;
- independent regional full archive replicas;
- versioned object storage;
- provider snapshots of current state and validator-owned sidecars;
- offline preservation of keys and signed release/rollback artifacts;
- documented restore drills that verify state root, manifest catalog, segment
  hashes, and historical RPC.

### 10.4 Restore

A restored validator uses its own identity, key, WAL policy, and state. Segment
payloads may be copied because they are immutable public content addressed by
hash; live mutable RocksDB state, another validator's WAL, identity, and peer
cache remain prohibited.

## 11. Capacity And Filesystem Design

### 11.1 Dedicated servers

For two 960 GB NVMe devices, use software RAID1 for approximately 960 GB decimal
usable capacity. RAID0 is not approved for validator state because one device
failure loses the local state/archive copy. RAID1 does not replace off-host
backup.

Recommended layout:

- boot/system volume or reserved system allocation;
- hot state/WAL filesystem with protected free-space reserve;
- immutable segment filesystem or logical volume;
- staging allocation/quota;
- bounded log allocation;
- monitoring for device health, RAID degradation, filesystem errors, inode
  pressure, and write latency.

If hot state and archive share one RAID filesystem initially, use project
quotas or LVM allocation so archive growth cannot consume the hot-state reserve.

### 11.2 Capacity formula

For a full archive validator, approved capacity is based on:

```text
required = system
         + hot_state_peak
         + hot_history_window
         + all_segment_bytes
         + segment_staging_peak
         + bounded_compaction_peak
         + rollback_copy_peak
         + logs_and_evidence
         + adaptive_runtime_reserve
```

For a verified-cache validator, replace `all_segment_bytes` with the configured
cache quota and retain remote-source redundancy.

### 11.3 Adaptive disk guard

Archive V2 replaces the universal fixed threshold with per-filesystem budgets.
The hard reserve should be the maximum of:

- configured absolute minimum;
- a percentage of filesystem capacity;
- maximum WAL plus aggregate mutable memtable allowance;
- two bounded segment/frame staging objects;
- measured bounded compaction overlap;
- a configured number of hours of recent observed growth;
- emergency log/evidence allowance.

Hot-state and archive-volume health are reported separately. Archive pressure
must stop segment construction and remote caching before it consumes the
hot-state reserve. Failure to persist consensus state remains a fatal stop;
failure to cache a remote historical segment does not corrupt or alter
consensus.

## 12. Security And Threat Model

Archive V2 assumes peers, gateways, disks, caches, and object stores can be
faulty or malicious.

Required defenses:

- content hashes and Merkle inclusion verification;
- chain ID/genesis binding;
- canonical parent/hash/root verification;
- bounded compressed and uncompressed sizes to prevent decompression bombs;
- bounded index counts, offsets, recursion, allocation, and frame lengths;
- no path traversal or manifest-controlled absolute paths;
- atomic writes and fsync boundaries;
- quarantine instead of overwriting a conflicting object;
- rate limits and concurrency limits on remote fetch;
- TLS and authenticated origins;
- separate read and write credentials;
- no private key or validator signer access in the segment builder;
- deterministic parser fuzzing for manifests, indexes, and frames;
- supply-chain and dependency audits for new compression/encoding crates.

Shielded public-history records remain encrypted payloads as they are today.
Archive conversion must not log sensitive operational secrets or place bulk
payloads through privileged sudo I/O auditing.

## 13. Observability And Operations

Expose at least:

- configured validator role;
- hot state bytes and live-data estimate;
- hot history first/last slot and retained slot count;
- legacy cold bytes during migration;
- Archive V2 local segment count/bytes;
- complete catalog first/last slot and manifest root;
- missing, corrupt, quarantined, and fetching segment counts;
- cache quota, used bytes, hit/miss/eviction rates;
- segment build slot range, phase, bytes, ETA, and last error;
- replica acknowledgement counts;
- retirement/compaction cursor and reclaimed allocated bytes;
- hot/archive filesystem available bytes and calculated reserves;
- RPC hot/local-segment/remote-segment latency histograms;
- checksum and parity failure counters.

Alerts:

- archive continuity behind canonical finalized tip;
- fewer than required independent replicas;
- hot filesystem below warning reserve;
- archive filesystem below build reserve;
- corrupt or conflicting segment;
- object-store inventory drift;
- cache-fetch source exhaustion;
- segment build or retirement stalled;
- RAID degraded/device health failure;
- role advertised inconsistently with actual data.

## 14. Legacy-To-V2 Migration

### 14.1 Preconditions

- larger or temporary writable storage is provisioned;
- exact signed release and rollback artifacts are preserved;
- provider/current-state backups are identified and verified;
- all four validators prove fixed-tip public-history parity;
- representative legacy cold reads pass;
- enough capacity exists for legacy plus staged V2 data and bounded
  compaction;
- the V2 reader passes against independently built identical segments.

### 14.2 Phased rollout

**Phase A — format and benchmark**

- implement codec, manifest, frame, index, and corruption tests;
- benchmark representative 1,000,000-slot samples;
- record compression, build time, decompression latency, memory, and index size;
- select version-1 parameters from evidence.

**Phase B — reader-only release**

- ship Archive V2 reader and health metrics;
- keep all writes and RPC reads on legacy storage;
- validate supplied test segments and rejection behavior.

**Phase C — dual-build**

- build segments from fixed finalized ranges without deleting legacy rows;
- independently build/compare on multiple validators;
- upload immutable replicas;
- run legacy-versus-V2 RPC parity.

**Phase D — dual-read canary**

- serve one canary origin from V2 with legacy fallback;
- compare responses, latency, errors, and cache behavior;
- expand canary only after parity.

**Phase E — V2 primary reads**

- read V2 first for sealed ranges;
- retain legacy fallback and rollback stores;
- prove full edge/fleet behavior and recovery.

**Phase F — new rollback anchor**

- publish and deploy a signed release that reads V2;
- complete four- and ten-validator restart/rejoin tests;
- declare that release the rollback anchor only after approval.

**Phase G — bounded legacy retirement**

- retire one verified range/category at a time;
- compact with bounded transient space;
- prove manifests and RPC after each unit;
- stop on any discrepancy.

**Phase H — role activation**

- activate full, verified-cache, and consensus roles through explicit network
  policy;
- update edge routing and admission checks;
- prove archive-node loss and remote-source loss drills.

## 15. Implementation Work Breakdown

### AV2-001: format specification

- Freeze canonical CBOR or an equivalently deterministic manifest encoding.
- Define integer widths, byte order, ordering, duplicate rejection, and
  canonical map rules.
- Define forward/backward compatibility rules and test vectors.

### AV2-010: segment codec

- Frame writer/reader with bounded allocations.
- Zstd parameters and dictionary support.
- Oversized-record handling.
- Hash/Merkle construction.
- Round-trip, truncation, bit-flip, bomb, and fuzz tests.

### AV2-020: canonical block/transaction deduplication

- Store block transaction bodies once.
- Resolve transaction hashes to block/ordinal.
- Preserve exact block and transaction RPC serialization.
- Prove legacy transaction/index parity.

### AV2-030: public indexes

- Implement every current public-history category mapping.
- Delta/prefix encode ordered postings.
- Build deterministic rebuild proofs.
- Benchmark index lookup and rebuild.

### AV2-040: manifest catalog

- Atomic append/update.
- Continuity and supersession verification.
- Complete catalog root.
- Import/export and disaster recovery.

### AV2-050: segment builder

- Fixed finalized-range selection.
- Read-only source snapshot semantics.
- Resumable staged build.
- Replica acknowledgements.
- Crash-safe promotion.

### AV2-060: read integration

- Hot/legacy/V2/fetch lookup chain.
- RPC parity instrumentation.
- Typed corruption/fetching/unavailable errors.
- Bounded verified cache.

### AV2-070: migration and retirement

- Dry-run byte/row accounting.
- Write-first segment creation.
- Legacy-to-segment equivalence proof.
- Bounded tombstone/compaction.
- Durable progress journal and idempotent resume.

### AV2-080: replication

- Peer/object-store transport.
- Immutable upload/download.
- Hash verification and source failover.
- Inventory and replica policy.

### AV2-090: validator roles

- Explicit role configuration.
- Startup admission and health semantics.
- P2P capability advertisement.
- Edge routing integration.
- Role transition safeguards.

### AV2-100: adaptive capacity guard

- Separate hot/archive capacity measurements.
- Measured growth and operation-peak budgets.
- Warning/critical/fatal thresholds.
- Checkpoint, segment, cache, and consensus priority ordering.

### AV2-110: snapshot and join

- Catalog and segment discovery in genesis/checkpoint sync.
- Full archive join and verified-cache join.
- No copied mutable state.
- Identity-preserved rejoin tests.

### AV2-120: operations and tooling

- Status, verify, repair, mirror, benchmark, and restore commands.
- Fleet manifest/segment parity verifier.
- Metrics dashboards and alerts.
- Capacity forecasting and role inventory.

### AV2-130: documentation and release

- Operator runbooks.
- Format specification and threat model.
- Upgrade/rollback/restore drills.
- Public node requirements.
- Signed release and artifact provenance.

## 16. Test And Release Gates

### 16.1 Unit/property/fuzz

- deterministic build from identical history;
- every supported category round-trip;
- arbitrary frame boundary and oversized block;
- malformed/truncated manifest, index, and frame rejection;
- duplicate/out-of-order key rejection;
- decompression-size enforcement;
- hash, Merkle, chain, slot, transaction, and state-root mismatch rejection;
- crash at every promotion/retirement fsync boundary;
- idempotent resume;
- cache eviction races and concurrent readers;
- malicious remote source and conflicting replica;
- old/new format compatibility.

### 16.2 Storage compatibility

- v0.5.225 legacy hot/cold opens read-only under the candidate;
- candidate writes remain readable by the current rollback engine until the
  rollback anchor changes;
- Archive V2 legacy data is never deleted while the rollback cannot read V2;
- ext4/XFS and RAID-degraded recovery drills;
- full disk, inode exhaustion, read-only remount, short write, and fsync error.

### 16.3 Local network

- required four-validator gate with hot/cold/segment parity;
- ten-validator expansion with two validators stopped;
- full archive, verified-cache, and consensus roles;
- same-process material gap and catch-up;
- one-validator outage and own-state restart;
- seed outage and restart;
- all-validator preserved-state restart;
- archive peer loss, object-store loss, and cache miss;
- segment corruption and recovery from another replica;
- new full archive join from immutable segments plus network state sync;
- no copied RocksDB, WAL, identity, or key material.

### 16.4 RPC and product

- all current historical JSON-RPC and REST surfaces;
- Explorer, wallet, DEX, developer portal, exchange, custody, and faucet
  journeys where applicable;
- WebSocket freshness during archive maintenance;
- response parity legacy versus V2;
- latency and resource budgets;
- correct edge failover and role routing.

### 16.5 Live migration

- fixed-tip manifest before migration;
- provider/backup evidence;
- exact signed artifact and running hash parity;
- dual-build with no legacy deletion;
- independent segment-root equality;
- canary dual-read parity;
- bounded retirement evidence;
- final genesis-to-tip catalog and representative historical probes;
- installed/running binary parity and no deleted executable;
- no state, archive, WAL, key, identity, or rollback loss.

## 17. Performance Targets

Initial targets, to be validated by benchmark:

- local segment `getBlock` p95 below 100 ms after filesystem cache warm-up;
- local segment `getTransaction` p95 below 150 ms;
- verified remote cold request p95 below 2 seconds under normal source health;
- no consensus-slot latency regression from background building;
- bounded builder CPU, memory, I/O, and open-file usage;
- segment build resumable without restarting the validator;
- cache and builder I/O priority below consensus-state durability work;
- compression choice based on total storage plus CPU cost, not ratio alone.

No storage-reduction percentage is promised until the representative benchmark
measures block-body deduplication, index encoding, account history, and legacy
encoding separately. Compression-only comparison against the current Zstd cold
DB is insufficient because the major win is removing duplicate payloads and
LSM overhead.

## 18. Emergency 5 GiB / 50,000-Slot Release Procedure

This bridge is separate from Archive V2 implementation.

1. Change only the testnet runtime reserve to 5 GiB; preserve 10 GiB for
   mainnet/production defaults.
2. Change the default cold retention constant to 50,000 slots.
3. Update tests for inclusive thresholds, checkpoint reclamation, snapshot
   capacity, public-network automatic cold storage, and migration integrity.
4. Run formatting, strict Clippy, locked workspace tests, audit/deny,
   standalone workspaces, contracts, static QA, and the required local
   four-validator hot/cold drill.
5. Commit from a clean worktree, tag the next version, and wait for hosted
   release gates.
6. Verify detached PQ checksums and the exact Linux validator hash.
7. Stage the signed artifact on all validators without installing from local
   builds.
8. Because restart recovery and storage behavior change, do not use the normal
   rolling restart helper. Stage and verify the signed artifact on all hosts,
   then stop EU/SEA/IN together at a recorded healthy tip; US is already stopped.
9. Remove the temporary US runtime retention override before any validator
   start. Preserve its contents in incident evidence first so the change is
   auditable.
10. Do not execute the v0.5.227 US lineage repair. Its first live dry run
    correctly failed before writing because the embedded validator-account
    image conflated total spores with spendable spores and did not preserve the
    separately bonded 100,000 LICN. Supersede only that repair path with signed
    v0.5.228. Require fixed tip `9,830,991`, fixed block hash, before root,
    before stake-pool hash, six exact account total/spendable images, conserved
    totals, equal per-account deltas, and the projected root from section 2.1.
    Execute only with the v2 confirmation, then rerun the command for
    idempotence and require the exact after root and repaired post-state anchor.
11. Record one fixed cutoff per validator from its stopped canonical tip. For a
    50,000-slot window, cutoff is `fixed_tip - 50,000`. US's completed cutoff is
    recorded in section 2.2; do not repeat that bounded migration.
12. On each other stopped validator, run the signed binary's cold-migration dry
    run with service-equivalent file-descriptor limits. Parallel execution is
    permitted because stores are host-local, but every report remains separate.
13. Require zero decode, hash, cursor, transaction, index, or cold conflicts,
    no multiply-linked active hot SST checkpoint files, and enough bounded
    transient headroom on every host.
14. Execute write-first bounded migration for blocks, transactions, and
    transaction-to-slot rows, then rerun dry run and fixed historical probes.
    Record physical available bytes before and after every bounded compaction
    batch. Any unexpected value aborts before services start.
15. Atomically promote the verified v0.5.227 executable on all four hosts while
    preserving v0.5.225 as signed pre-change evidence. It is not a restart-safe
    rollback on this mature activated chain. Prove installed SHA-256 parity
    before starting any validator.
16. Start EU, SEA, and IN from their own preserved canonical state while US
    remains stopped for v0.5.228. Confirm the runtime's first
    50,000-slot maintenance pass also migrates supported account-transaction,
    account-snapshot, event, token-transfer, and program-call indexes. Any
    repair, replay, archive, or root error stops the recovery.
17. Require those three validators to converge their preserved BFT WAL rounds,
    finalize fresh blocks with three-of-four stake, retain unchanged
    identities/keys/WAL, and execute the exact signed v0.5.227 binary hash.
    Require local and public RPC health plus the Explorer route before declaring
    temporary availability restored.
18. Run every release gate for v0.5.228, publish signed artifacts, verify the
    detached PQ checksum signature and attestation, and stage the exact signed
    artifacts on all four hosts. Stop EU/SEA/IN together at a recorded tip,
    execute the signed v0.5.228 US dry run and repair, promote v0.5.228 on all
    four, and start the complete fleet together from node-owned state.
19. Require all four to catch up, report the same canonical block/hash and
    state-root evidence at a fixed slot, produce fresh blocks, retain unchanged
    identities/keys/WAL, and execute the exact signed binary hash.
20. At a common checkpoint, stop all four once more for the strict fixed-tip
    hot/cold public-history manifest parity gate; then restart from preserved
    state and repeat liveness/root probes.
21. Verify strict edge health, origin failover, WebSocket delivery, Explorer,
    installed/running hashes, disk bytes, zero systemd restart loops, and a
    production sample containing every validator.

If staging or checksum verification fails, abort before the coordinated stop.
After the v0.5.228 fleet stop or US correction begins, recovery fails forward
using the same signed v0.5.228 artifact; restarting v0.5.225 is prohibited.
Signed v0.5.227 remains the restart-safe temporary anchor for already-canonical
nodes but is not an allowed US repair executable. The old binaries and legacy
stores remain preserved for evidence and format compatibility, and the
50,000-slot migration remains nondestructive because all bridge releases
understand the current cold RocksDB format. No Archive V2 retirement occurs in
this bridge.

### 18.1 Emergency bridge execution record (2026-07-21)

- Safe package/journal cleanup left approximately 12.1 GB free on US, 11.9 GB
  on EU, 13.0 GB on SEA, and 14.0 GB on IN. No state, archive, WAL, key,
  identity, access configuration, backup, or signed rollback artifact was
  deleted.
- Stopped-node write-first migration established a 50,000-slot hot boundary:
  US migrated 50,000 blocks at cutoff `9,780,991`; EU migrated 50,026 blocks
  and SEA/IN 50,000 blocks at cutoff `9,785,779`. Post-migration v0.5.227 dry
  runs on all four reported zero eligible rows, decode errors, hash mismatches,
  missing/conflicting cursors, missing/mismatched transaction indexes,
  integrity errors, and conflicts.
- The complete signed v0.5.227 executable set was staged and promoted on all
  four while validators were stopped. The running validator SHA-256 on the
  three active nodes is
  `73355b766adeade94fd4a4aec460d51a1d839f780cc2edb10ec38fd7140062d3`;
  each host retains the complete pre-v0.5.227 executable set.
- US remains stopped at slot `9,830,991`. Its v0.5.227 repair dry run rejected
  the first bonded validator account and wrote nothing. EU/SEA/IN resumed from
  common slot `9,835,779`; their preserved WALs converged at BFT round 21 and
  committed block `9,835,780` with hash prefix `7327be51` before returning to
  normal round-0/1 production.
- Public `testnet-rpc.lichen.network`, `testnet-api.lichen.network`, and the
  Explorer same-origin `/api/testnet` route returned HTTP 200 with advancing
  slots. `getHealth` returned `status=ok`, and the Explorer root returned HTTP
  200. This is temporary three-validator availability pending the signed
  v0.5.228 repair and complete four-validator parity gate.

## 19. Acceptance Criteria

Archive V2 is complete only when:

- every backed canonical history row is represented and verified;
- the full public-history category digests match the legacy source;
- every full archive validator has identical logical segment roots;
- verified-cache validators can fetch from multiple sources and reject
  corruption;
- consensus-only validators cannot be mistaken for archive origins;
- one archive validator and one archive provider may fail concurrently without
  losing historical RPC availability under the approved failure budget;
- current state survives every stop/restart/rejoin drill independently;
- no mutable database, WAL, identity, or key is copied between validators;
- a new full validator can obtain public immutable segments and sync its own
  current state through supported network paths;
- legacy retirement is crash-safe and rollback-approved;
- capacity forecasting demonstrates the dedicated RAID1 hosts remain above
  adaptive reserves for the approved planning horizon;
- mainnet passes a fresh, no-waiver genesis-to-tip archive proof.

## 20. Explicit Non-Solutions

- Lowering the disk floor indefinitely.
- Restarting a validator repeatedly after status 78.
- Deleting old blocks, transactions, account history, events, or indexes.
- Keeping only state roots while discarding the signed bodies required for
  public history and replay.
- Copying another validator's live RocksDB or WAL.
- RAID0 without an independently approved loss/recovery design.
- Treating RAID1 as backup.
- One shared mutable network filesystem mounted as every validator's archive.
- Trusting an object-store response without content verification.
- Calling a partial source set archive parity.
- Returning `not found` for a corrupt, unavailable, or not-yet-fetched segment.
- Forcing a full cold RocksDB compaction on the current nearly full disks.
- Deleting legacy cold storage before a V2-capable signed rollback anchor exists.

## 21. Decisions Still Requiring Benchmark Or Owner Approval

- exact segment slot span;
- exact frame size and Zstd level/dictionary;
- canonical manifest encoding;
- account-snapshot anchor/delta strategy;
- full archive replica count for mainnet;
- verified-cache quota and remote request timeout;
- object-store provider and retention policy;
- role admission/governance mechanism;
- adaptive reserve percentages and growth horizon;
- when the V2-capable release becomes the rollback anchor;
- when legacy RocksDB retirement may begin.

None of these open parameters permits weakening current state or archive
preservation. Unknowns are resolved through benchmarks and failure drills, not
assumption.
