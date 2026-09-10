# Archive V2 deployment and recovery gates

## Operating sequence

This guide defines gates, not current fleet status. The dated rollout notes below
are historical evidence. Before acting, read the latest execution checkpoint and
verify the actual services, installed/running hashes, own WAL identities, catalog,
source inventories and available resources. Use the per-run deployment record identified by the release qualification. A historical
"complete" receipt authorizes neither a new operation nor replay of an old one.

Run each one-validator maintenance lifecycle as a supervised, ordered sequence:
fresh inspection, stop, bounded native work, own-WAL restart, full-fleet health,
auxiliary restoration and guard removal. The sequence must reserve enough time
for startup and acceptance before its maintenance guard expires. Do not leave
these dependent steps waiting for a conversational continuation. A lost SSH
reply requires inspection of durable intents, native outcomes and the current
process before any further mutation; it does not authorize a repeated stop,
native command or start. Read-only readiness probes may retry bounded transient
connection or capacity errors while preserving each failed observation.

Verify the actual SSH route before the first stop. The September 10 continuation
replaced failing multiplexed relay routes with individually verified fresh,
direct SSH sessions, retaining strict host-key checks and existing credentials.
Bind transport changes in the new operator's evidence; preserve previously
qualified sources. Full R2 readback may take much longer than the upload: inspect
transfer progress and retain the publication deadline. Never equate an uploaded
object with a verified copy or a partial publication with an adopted catalog.
For the September 10 catalog-467 publication, one replica IPv6 download took
about 15 minutes for 142.6 MB. A separate IPv4 range read returned the exact
expected 1 MiB in 0.4 seconds. The reviewed adjustment appended only `ipv4` to
the two exact temporary curl configurations, preserving endpoint, credentials,
TLS verification, retries, timeouts, full readback and catalog CAS. It neither
restarted the in-flight transfer nor dispatched an R2 write. The before/after
configuration hashes and mutation receipt are retained under
`14e22df6916900ced8a40b122e8bfd6216c41e09cae5a1363a9f46050ed04566`.
Future deployments must qualify the actual route and include its choice in the
publication plan; do not copy ephemeral credential files into deployment notes.

1. **Bind the release.** Use a clean checkout and immutable tag. Check every
   version consumer and locked workspace before tagging; require full CI and the
   tag workflow, complete four-validator matrix, platform provenance, checksum
   hashes and detached PQ signature before installation. Measure the actual signed
   tar members and selected binaries. Resolve draft assets with `gh release view`
   and compare their immutable metadata; exercise the actual download transport.
2. **Bind the operation.** Record the network/genesis, each host's identity,
   installed and running binaries, invocation/start identity, service state, own
   state/WAL, catalog and source bindings. Log configuration hashes rather than
   credentials. Inspect active native readers before any stop. A process from an
   earlier receipt cannot stand in for the current process.
3. **Reconcile capacity across every consumer.** Read the native calculation and
   all wrapper defaults. Distinguish installation, catalog refresh, checkpoint,
   archive building, manifest verification and retirement. Account for peak
   allocations, retained rollback inputs and recovery, on the actual filesystem.
   Credit an existing cache only after a stopped, authenticated inventory proves
   its allocation and immutability. Test exact saved inputs, including one-byte
   failure boundaries, before dispatch. Projected deletion is not free space.
   List native reserves, each allocation and additional operator padding as
   separate terms. Do not silently reuse padding from a different maintenance
   mode. The September9 Testnet continuation retains512MiB extra where admission
   depends on derived-copy removal, and256MiB extra where it uses measured free
   space plus declared regenerable cache. Both additionally charge the complete
   native reserve, cache quota, overlapping catalogs, records, signed payload and
   adoption. These are operation-specific margins, not Mainnet/Devnet defaults.
   A change to any term requires new exact-plan and one-byte-short qualification;
   actual post-prune, post-cache, staging and native admission gates still apply.
4. **Preserve recovery before changing files.** Keep every validator's own keys,
   state and WAL, the required signed rollback artifacts, canonical captures,
   active archives and provider backups. Bind any derived-checkpoint cleanup to
   its exact completed generation and references. Preserve a useful newer capture
   before a later runtime checkpoint can retire its periodic directory.
5. **Coordinate transitions.** For a coordinated upgrade, establish the complete
   stopped fleet barrier before installation and the complete staged barrier
   before replacing binaries. A staged barrier must accept the old installed
   release and subsequent partial install progress; it verifies staged and
   rollback artifacts. Require all new installed binaries before admission and
   startup. Exercise the complete four-host phase sequence in controller tests,
   including these version transitions. Keep a finite halt guard. Inspect partial phases;
   never overwrite their receipts or blindly replay a completed stop or prune.
   Renew a maintenance guard only with fresh matching stopped-state evidence.
6. **Use one history rule for CLI and runtime.** Both admission paths must derive
   the same catalog handoff and physically verify the entire required local
   suffix. A recent nominal window alone is insufficient. The existing bounded
   unpublished extension is 50,000 slots; excess lag fails closed. Checkpoint
   metadata's `archive_v2_catalog_root` stores the append-stable handoff digest;
   compare it with the native handoff calculation, not directly with catalog root.
7. **Separate verification stages.** Source inventory, source-index validation,
   whole-object hashes, local handoff verification and full public-history parity
   prove different things. Require native admission against the selected signed
   artifact. Initial placement also requires its bootstrap dry-run; an existing
   role after cold retirement uses the marker-authorized restart path below.
   A successful TLS probe in one client does not
   prove another client's acceptance; verify the actual runtime client when TLS
   is implicated. Do not change a CA based only on a historical error field.
8. **Prove recovery and live behavior.** Wait for RPC and BFT readiness after
   startup. Require matching installed/running artifacts, advancing slots,
   authorship by all four validators and common fixed-slot canonical finality.
   Verify exact transaction-counter deltas against new canonical transactions,
   restore the prior auxiliary service states, then clear maintenance guards from
   fresh matching evidence. Never replay an already applied metrics backfill.
9. **Finish retirement and publication.** Archive V2 activation alone does not
   release the legacy files. Continue source-backed, journaled eligible retirement
   and bounded physical reclaim; distinguish logical deletion, family shrinkage
   and observed filesystem space. Preserve original journal roots and pending
   work across catalog upgrades. Demonstrate continued archive publication,
   restart/outage recovery, deep reads and full fixed-tip history parity before
   declaring the rollout complete. Apply only the documented existing-Testnet
   body-loss waiver; fresh networks and Mainnet have none.
10. **Close the release.** Verify application behavior and served PWA/extension
    versions, publish accurate release/deployment records, then remove only
    qualified obsolete artifacts or R2 objects. Active primary/replica archives,
    source captures and protected backups remain part of the deployment.

Account Activity acceptance must exercise the complete `getTransactionsByAddress`
handler, including the extra row used for `has_more`, receipts, timestamps and
the returned signature cursor. Verify a full recent page without unrelated
archive fetches, a page crossing into archive storage, multiple transactions in
one block, overlapping hot/archive rows and unavailable required history.
Compare cold/warm latency and exact returned rows on the same sampled account
before and after a signed change. A fast UI fixture does not measure live RPC
latency. See the [account pagination audit](../audits/V0.5.291_ACCOUNT_ACTIVITY_PAGINATION_2026-09-09.md).

Compare dashboard counters with canonical transactions excluding consensus
transactions, matching `MetricsState::track_block_at`. The public-history
manifest's transaction category includes consensus transactions. Its raw count
is therefore not a replacement value for the dashboard's total.

A verifier stopping at its reserve guard means its planned peak/recovery envelope
was no longer available. Archive V2 saves space after verified legacy retirement
and physical reclamation; migration can temporarily retain both representations.
Identify the actual occupied files, references and peak consumers, then qualify a
fitting plan. Repeating the same failed scan or lowering its floor does not prove
the deployment can finish or recover.

Native maintenance units must also declare and verify their descriptor limits.
Do not inherit systemd's default soft `LimitNOFILE`: the September 9 US live
database contained 3,318 SST files while an isolated verifier inherited 1,024
descriptors. It failed with `Too many open files`; the validator itself remained
healthy with a 65,536 limit. The corrected read-only verifier declares
`LimitNOFILE=65536`, verifies both effective limits in the actual namespace,
opens 2,048 harmless descriptors as a preflight, and checks the SST inventory
against its descriptor budget before opening RocksDB. Disk, memory, deadline
and read-only filesystem gates remain independent requirements. A derived
checkpoint with fewer files does not qualify a live database's resource limits.

Before tagging a new source version, check the runtime crate manifests, path
dependency version constraints and all five Cargo lockfiles. Also update the
candidate references in README, the developer CLI and getting-started guides,
developer changelog, exchange portal/package readiness checker and current
activation plan. Preserve historical release entries. Run public-claims,
exchange-release-assets and the full frontend/deployment static QA before CI
publication so a version bump cannot leave these consumers on the previous line.

## Native restart and archive publication

### Restart after legacy cold retirement

Distinguish initial role bootstrap from restarting an already admitted validator.
In v0.5.291, the maintenance CLI's `role-preflight` reads canonical genesis from
local hot/cold storage, and `role-bootstrap` explicitly requires `--cold-store`.
Removing that argument does not make either command suitable for a retired cold
store. On September 9 this incorrect wrapper change rejected all four stopped
validators with `local state has no canonical genesis block`; no startup had
been attempted and the genesis block remained in the verified Archive V2 data.

The signed validator supports an existing checksummed `role-config-v1.bin` when
slot zero is outside the hot window. Its startup validates marker identity and
role, verifies the complete hot canonical range and archive handoff, applies
capacity admission, and validates any persisted state-admission fingerprint.
Keep immutable source verification separate from short-lived process barriers.
Record the complete source inventory with its catalog hash, observation time,
host and own stopped WAL. Apply an explicit observation lifetime and recheck
current catalog identities. Refresh the complete stopped/paused process barrier
immediately before each sequential action. Do not run expensive source inventory
inside a shared barrier whose lifetime cannot cover that work. Preserve partial
completion and use an explicitly bound continuation instead of replaying it.

Use that native restart path only for the exact previously admitted state,
preserved marker, own WAL, verified catalog and signed artifact. Check both
source inventories, authenticated indexes and the complete recovery reserve
before startup; require actual marker activation and native admission before
allowing the fleet to proceed. Preserve initial-bootstrap failures as failures.
Never fabricate a successful CLI admission record or create a dummy cold store.

After all nodes pass native startup, verify advancing authorship, fixed-block
and commit parity, genesis/deep reads, counters and application queries before
clearing the maintenance guards. Missing, corrupt or conflicting markers and
incomplete or conflicting hot history must still stop startup. Fresh networks
must complete their initial admission and receive no historical Testnet waiver.

### Publication cadence

Read the checkpoint interval and publication depth from the selected release.
In v0.5.291, periodic hot-repair checkpoints run at 10,000-slot boundaries;
the archive builder's retained finality depth is 50,000 slots. These are separate
limits. A source checkpoint at slot 12,850,000 can therefore publish through
12,800,000. A later live RPC finality response does not advance the stored slot
inside that immutable source. Do not relabel checkpoint metadata or lower the
builder's finality depth to extend its publication range.

Calculate the next restart deadline from the admitted catalog end, configured
recent-history window and native unpublished-extension bound. Include measured
source verification, cache import, native admission and coordinated startup time
before stopping validators. If that complete transition cannot fit, preserve a
newer completed own checkpoint, publish a verified append to both sources, and
qualify the local catalog transition first. A running reader does not automatically
adopt an externally replaced catalog in this release.

Record both observed checkpoint publication and the procedure responsible for
continued archive publication. Enabling the Archive V2 reader or publishing one
append does not establish ongoing retention. Recovery of free space must also
restore checkpoint cadence; successful block production alone does not establish
that recovery.

## Full verifier I/O and completion

The signed public-history verifier performs a canonical ledger walk and separate
globally ordered merges for the other public categories. The canonical walk
finishing is not completion of the full manifest. Review the final native JSON,
all category counts and digests, source slot, handoff, state root, catalog and
documented network-specific gaps before accepting its result.

Measure the exact verifier configuration, including its index-cache behavior.
With a local object directory and no cache, the reader extracts authenticated
indexes from object files for each category. A remote filesystem can make those
repeated range reads expensive even when scratch usage is small. A configured
authenticated index cache can avoid those repeated extractions, but must be
included in the actual filesystem, ownership, namespace and peak-capacity plan.
For `public-history-manifest`, `--cache-root` and a nonzero `--cache-quota-bytes`
require at least one `--source-dir`; setting a cache argument alone is rejected.
Do not change the configuration or deadline of an already running verifier.

After an interrupted transport, inspect the existing unit, native output and
completion record. Never repeat an expensive native operation merely because
its local reply was lost. A runtime deadline or reserve failure remains a failed
verification, even when earlier stages succeeded; preserve its evidence and
qualify a fitting successor before retrying.

## Exact legacy-copy and R2 retirement

Archive V2 activation and obsolete-file retirement are separate operations.
Before retiring a detached legacy database, verify complete replacement history,
independent archive copies and each validator's own recovery path. Check that
runtime and remaining maintenance commands no longer require that database.
Preserve the original retirement journals, authorizations and catalog bindings;
do not rebind a pending journal to a later catalog. `retirement-reclaim` opens
the optional cold store only when `--cold-store` is supplied, so inspect the
actual journal families before adding that dependency to a hot-only reclaim.

Bind deletion to exact file identities, preserve and fsync database control files,
scan local filesystems including `/dev/shm`, descriptors and mappings, and use a
durable no-replace detach/removal ledger. Unlinking an SST symlink must never
remove its target. A file-only budget must fund both its bounded operations and
the unchanged native recovery reserve after removal. Confirm the latter using
observed free space; estimated exclusive allocations are only a preflight input.

For legacy R2 mounts, verify the running mount's actual backend and key prefix,
configuration hash, unit identity and remaining dependencies. A familiar mount
name alone is insufficient. Complete and verify local retirement and normal
unmounting before treating those keys as obsolete. Account for background
integrity scanners as well as validator descriptors. Preserve active Archive V2
primary and replica objects, recovery copies, signed rollbacks and unclassified
prefixes. Both buckets remain part of the deployed archive; cleanup must never
mean emptying them.

Inspect every unit's actual restart and stop policy. The September 9 fleet has
empty `ExecStop` command arrays that `systemctl show` omits even with `--all`.
Request all scalar properties explicitly and verify the empty command array
through systemd's typed D-Bus `Service.ExecStop` property. A missing property
alone is not proof that a stop command is absent. Exercise the actual property
reader against every deployed unit before authorizing the first unmount.

The September 9 fleet has
three legacy variants: no post-stop hook, an ignored `fusermount3 -uz` hook, and
two ignored hooks ending in `umount -l`. Stopping these units first could hide a
busy mount. Verify that a normal `umount -- EXACT_TARGET` refuses open references,
then confirm successful daemon exit without an automatic restart. Preserve the
transient unit fragment before systemd removes it. Qualify each distinct rclone
and mount-helper binary set; India differs from the other three hosts.

Place isolated mount fixtures under an existing permitted path, such as a private
directory under `/mnt`. This fleet's fusermount3 AppArmor policy does not permit
arbitrary mounts under `/var/lib/lichen`; do not weaken the policy to fit a test.
Use the `lichen` account name in native maintenance units and verify permissions
on each host. The US UID is 999 and the other hosts use 997. Probe the real
read-only source paths and writable cache in the exact service namespace before
stopping validators; a root-only probe does not prove service-account access.

Before any R2 deletion, take a fresh complete listing and bind the exact bucket,
key, size and ETag set to the accepted retirement and recovery evidence. Reject
scope changes, new dependencies and unmatched keys. Keep durable per-object
results and verify the post-delete listing and live history again. Report actual
deleted bytes separately from candidates, local disk gains and preserved data.

The deletion transport must not retry a request whose outcome is uncertain.
An HTTP error alone does not establish that every selected object survived.
Preserve the exact intent, HTTP response and per-key result. Reconcile an
ambiguous batch against complete fresh inventories, including the protected set,
before deciding whether a new request is justified. On September 10, a 429 batch
was reconciled using two separated inventories showing all 1,000 objects still
present and unchanged; the rejected receipts were retained separately before
the later request returned an actual successful per-key result. No synthetic
success record was used. A new failed batch requires its own reconciliation.

A failed read-only gate before a delete request is a different continuation
boundary. Repeat fresh acceptance only after validating all completed batch
ledgers and proving there is no pending uncertain request. Record finite retry
limits and terminal receipts. An RPC busy response is not evidence of stopped
consensus; inspect the response and current finality, and retain the acceptance
gate. Do not raise RPC concurrency or weaken disk reserves to make cleanup pass.

## Retiring superseded diagnostic checkpoint copies

Inventory allocated blocks and hardlink references before selecting a copy.
Use `os.scandir` with `entry.stat(follow_symlinks=False)` for local-device walks:
`os.walk(followlinks=False)` can still follow file symlinks while classifying
entries and stall on remote SST metadata. A timed-out inventory provides no
capacity credit. Shared checkpoint SSTs provide no physical saving until their
last link is removed; never count logical size as recovered space.

Bind a selected diagnostic copy to its original completed capture, exact inode
identities and control hashes. Verify its covered history against both immutable
archive sources and preserve a qualified newer capture. Keep parent capture,
publication and qualification records, produced archive objects, and a hashed
copy of checkpoint controls. Check live descriptors, mappings and references
before a no-replace detach. Remove only the sealed file set through a resumable
ledger, preserving live hardlinks and rejecting changed or extra files. Keep
resource guards anchored to the filesystem root so they remain valid after the
selected directory is renamed or removed. Require fresh process-bound fleet
health and test rename races, partial removals and control corruption on Linux.

Measure actual filesystem space afterward. This retires a derived diagnostic
copy; it does not prove legacy row retirement or full public-history parity.

A read-only bind preserves source modes. A public destination parent does not
make a bound root-owned `0700` directory or `0600` SST readable by `lichen`.
Inspect every file in the actual capture before creating a verifier view; an
older capture contained two private SSTs among 124 readable SSTs. Where needed,
copy only the enumerated small private files within a measured byte ceiling and
bind readable SSTs individually read-only. Probe directory traversal, controls,
private SST copies and readable SST bindings inside the actual native service
user's read-only namespace before opening the database. Verify unchanged source
hashes, modes, identities and link counts afterward. Keep recovery parents
private, and qualify the complete saved inventory rather than a fixture that
assumes every SST has the same mode.

A superseded periodic checkpoint needs the same exact-generation and reference
checks. Preserve a completed newer own checkpoint and the selected generation's
hashed control records; never select the live state directory. A file-only
operation that opens no database has its own bounded metadata and growth budget.
It must demonstrate that selected exclusive allocations cover the unchanged
native recovery envelope plus an explicit margin, then measure that full native
reserve after removal. Do not apply a lower file-only floor to a later stop,
native verifier, catalog refresh or retirement pass. Re-run the actual fleet
preflight after reclamation, since ongoing compaction can change free space.

## Historical receipt read acceptance

Before accepting a release, time full block responses as well as commit-only
responses for genesis and representative archived blocks. Include transactions
with absent legacy receipts: a fast block-body lookup does not establish bounded
metadata work. Known canonical-slot receipt queries must read only their owning
authenticated segment. Preserve legacy receipt formats and source failures;
do not treat an unavailable owning segment as proof that metadata is absent.
Regression tests must observe source access, since a handler that defaults after
an archive error can return a plausible response after scanning unrelated data.
Include native and Solana JSON/base64/base58 block responses, current transaction
feeds and runtime progress. Pace production probes and retain timed-out results.

The dated checkpoints below are historical. Read the latest sealed execution
evidence and verify the live services before operating; none authorizes replay
of completed phases or replacement from an unsigned local build.

Historical checkpoint, September8 20:04UTC: signed v0.5.288 and Archive V2 catalog
430 are live on all four validators. All four authors, common finalized block
and commit, installed/running signed artifacts, and exact live transaction
counters passed. Previously active auxiliary services were restored and the
catalog maintenance guards cleared. Catalog430 ends at12,679,000. Legacy
physical retirement, full history verification and qualified obsolete R2 cleanup
remain open. Active primary/replica objects and remaining legacy references must
be preserved. Temporary legacy mount credentials are being renewed separately.

One returning validator remained in catch-up because its readiness loop compared
a local tip read before slow bootstrap RPC calls with a newer network tip.
The qualified bounded process pause admitted it through the existing exact-tip
stalled-quorum recovery path; all original processes resumed. Candidate v0.5.289
reads the local slot after asynchronous observations in both readiness loops.
It is not deployed. Preserve voting, drift, finality and post-effects gates.
Mobile header polish remains explicitly deferred until full release acceptance.

Historical checkpoint, September8 13:53UTC: all4 signed287 binaries are installed;
validators are STOPPED with each own WAL and signed284 rollback preserved.
Native287 preflight rejected catalog freshness: finalized12773078 requires
12673078 under its nominal100000-slot calculation, while427 ends12649000.
Capacity is Normal and both authenticated source inventories match. The runtime
already verifies a bounded unpublished hot suffix starting12649001, but the
CLI used a different boundary. Candidate v0.5.288 moves the unchanged runtime
calculation into a shared core helper and makes CLI admission physically verify
the same complete suffix. The50,000-slot extension cap, configuration, state
admission identity and consensus/storage formats remain unchanged. This is a
candidate fix, not deployment acceptance. Full release workflow and signature
verification are required before replacing any VPS artifact. Preserve the failed
preflight receipts and close/renew the active maintenance timers using exact
stopped process/WAL/artifact checks. Mobile header polish remains deferred.

Historical checkpoint, September8 09:46UTC: all four validators were LIVE on signed
v0.5.284 and catalog427. The refresh, own-state rejoin, all4 authorship, common
finality and exact live oracle counter checks completed; guards were cleared.
The signed286 composed history verifier is reading the preserved EU12700000
checkpoint under the sole fleet lock. Its inputs are read-only, scratch is capped
at3GiB, native RAM at8GiB, and the full recovery floor remains enforced.
At09:46 the EU validator process was unchanged, native stderr empty, scratch
245,547,008B and free disk27,964,911,616B. This is progress, not full history parity.
No286/287 validator is installed. Full physical retirement and qualified R2
cleanup remain open; preserve active primary/replica archives and all rollback.

The web wallet and DEX were published from cleanb22664cf with the popup session
fix and wallet0.1.10 responsive layouts. Four web viewport and three extension
browser checks, password/signing tests and PWA cache migration passed. Signed
v0.5.287 was the candidate that consolidated these frontend changes with the
already-qualified286 runtime source. It still requires exact workflow, artifact,
PQ signature and live-fleet acceptance. See `WALLET_LAYOUT_ACCEPTANCE.md` and
`WEB_WALLET_POPUP_SESSION.md` for application behavior and repeatable gates.

Historical checkpoint, September8 06:45UTC: all four validators were stopped for
the qualified427 refresh.427 is published and fully read back in both R2 stores, through12,649,000.
Its40,000 new blocks contain46,499 transactions. The completed build/publication
receipts supersede the05:40 in-progress note below. All4 stages, preflights,
coordinated stops and native append checks passed; all4 catalog files are427.
Index verification passed on US/EU and is running on SEA/IN. Each own WAL is
sealed and halt guards are active. Refresh plan d0737186… passed36 local and36
Linux cases against current inputs. Runtime restart/admission remain outstanding.
It reserves26,125,176,355 bytes before staging,25,989,958,042 afterward, and the
entire2GiB prewarm quota although authenticated427 indexes need728,583,649bytes.
Stopped native memory remains8GiB plus7GiB for the host. The423 checkpoint
boundary12,709,000 was passed; all four skipped it.427's conservative boundary
is12,749,000. Neither publication nor initial admission proves sustained cadence.

The candidate composed verifier requires private writable temporary storage;
ordinary segment builders and checkpoint exporters retain their prior filesystem
behavior. Draft PR69 has1196 core/85 Archive V2 cases passing. The largest real
segment passed two optimized traversals in24.14seconds at4,371,824,640bytes peak
RSS on Mac. Provision scratch index/compaction space and qualify the complete
Linux process before using this measurement for production admission. The
candidate is not deployed; signed284 remains installed on all four validators.

Before creating a release tag, update and check every version consumer: the
nine runtime manifests, exact core pins in CLI/SDK manifests, all five Cargo
lockfiles, README candidate line, developer CLI examples and changelog, current
execution-plan candidate and exchange package tag/archive references. Public
claim checks must derive the candidate version from the runtime manifest.
Keep installed-release evidence and older release entries separate.

Install locked Node dependencies with `npm ci --ignore-scripts` and
`npm --prefix sdk/js ci --ignore-scripts`, then build the SDK with
`npm --prefix sdk/js run build`. Run every command in the CI Expected Contract
Lockfile job and the Release quality gate's Verify release policy assets step,
as well as locked Cargo resolution for the root/compiler/SDK/Rust SDK/fuzz
workspaces, formatting and diff checks. Inspect every exit status before
committing and tagging. Missing local build dependencies are not test passes.
Full workspace/security/contract/matrix gates and detached signature/provenance
verification remain mandatory before deployment. Never move a failed tag.

The v0.5.285 tag was created before checking its developer-facing version
consumers. CI rejected the stale CLI example; the affected workflows were
canceled and no285 binary was deployed. The separately prepared286 successor
corrects these inputs and runs the complete local static gates before tagging.

September8 04:57UTC: all four are LIVE on423. The signed append-only check,
all423 cached/source indexes, both native admission checks and coordinated
startup passed. Each own WAL was preserved. Qualified IN rejoin completed;
EU/SEA resumed their own paused WALs. All4 authors advanced and agreed on
finalized slot12,696,007. Exact counters then increased7,866,566→7,866,597,
matching31 new oracle transactions on every origin. Auxiliary services are
restored and all refresh/rejoin guards are cleared.
Prewarm took US317.3/EU599.5/SEA1111.7/IN1318.7seconds; paired admission checks
took US798.8/EU1135.9/SEA1146.9/IN1426.1seconds. There was no failed native phase
or replay. Index verification is distinct from whole-object/history parity.
Full legacy retirement, sustained capacity/publication and remaining application/
release gates are still open. No blanket100% deployment claim is supported.
The current phase and exact receipts are in `memories/repo/current-state.md`.

September8 05:40UTC: EU completed authentic HotRepair checkpoint12700000 and
its immutable files were separately preserved without a validator restart.
US/SEA/IN still skip checkpoints under adaptive capacity; one EU checkpoint
does not establish sustained fleet capacity. Preservation charged the full
25,989,958,042-byte runtime/prewarm reserve plus4,056,100,973 retained SST
bytes,3,418,827 copied controls and1MiB records.13 fixture cases passed locally
and on Linux. Four signed source profiles then passed for40,000 blocks and
46,499 transactions through12649000, with50000-slot finality depth retained.

The427 builder reserves every remaining output using the conservative bound
derived from the signed codec and those actual profiles:474615477,473119723,
473452113 and474072573 bytes. Both copies,16MiB per manifest, both seeds/four
catalog allocations and256MiB records are added to the unchanged25.990GB
runtime reserve and6GiB producer peak. The initial37,566,159,352-byte floor
passed against37,769,744,384 available. Ten actual-input/interrupt tests passed
locally and Linux. This build and both-R2 publication are now complete;427
runtime activation remains outstanding. Retained source files already allocated by the completed capture
are not charged again as newly created files in a subsequent build.

September8 publication:423 is published and fully read back in both R2 stores,
through12,609,000. Its three new segments contain30,000 blocks and35,030
transactions from EU's preserved own snapshot finalized12,667,579. All native
builds and both local verifications passed. Independent parsing authenticated
all423 manifests, the unchanged420 prefix and unchanged Testnet loss waiver.
Catalog SHA390905725d8a4e9a13ff186a9eb3c96046b3577eaaa594564a88be1f0f6f6165;
publication proofc681746b…; parsed proofb9ab91ac…. No R2 deletion occurred.
Before this refresh, all four ran420; staging has now completed on every host.
EU's capture, own restart, bounded rejoin and guard clearing completed; current
four-way finality and authorship passed afterward. All snapshot/WAL/source and
publication receipts are in current-state. This is not full retirement acceptance.

The423 refresh reserves the full2GiB configured prewarm quota:25,989,958,042
bytes after staging and26,124,488,954 before all134,530,912 bytes of catalog
copies. All423 authenticated index files require726,627,950 bytes; the outer
plan still uses the larger full-quota consumer. Both native admission checks,
15GiB available memory before each stopped native job (8GiB native+7GiB host),
each own stopped WAL and the coordinated startup barriers remain mandatory.
The live pre-stop memory projection counts only the smaller of the validator's
private dirty and anonymous resident bytes as releasable. Require8GiB available
for the live host/operator and16GiB projected after stop; independently recheck
the15GiB native floor after the process has actually exited. Do not count clean
or shared RSS twice.36 actual-input cases passed locally and on Linux.

The earlier12679000 checkpoint planning boundary was passed. The replacement
catalog's conservative checkpoint boundary is12709000, and the423 plan must stop
before it. Signed checkpoint construction permits50,000 unpublished slots before
the nominal retained window; startup and checkpoint retention are distinct
consumers. Keep their actual signed coverage checks. Do not relabel a missed
publication window as sustained checkpoint success or edit an old sealed limit.

After a source capture completes, its already allocated files are included in
measured free space. A read-only profile creates no new SST links/copies and
uses a separate26,057,066,906-byte native/runtime-plus-record floor. A build from
that existing source separately reserves the full25.923GB runtime/prewarm floor,
the signed6GiB producer peak, all remaining output copies/manifests and seeds/
catalogs/records. The actual423 build required40,121,341,986 bytes and started
with43,662,086,144. Do not inherit the completed capture's prospective source
retention allocation as a new profile/build allocation. Source preservation
remains mandatory, and these finite operations do not prove sustained growth.
EU recovery records remain root-only0700; service-owned native snapshot work
uses a separate0700 directory under `/var/lib/lichen`, avoiding an inaccessible
private ancestor without changing its permissions. The real Linux unit fixture
proved service-account hardlinks and unchanged source bytes before live capture.

September7 23:27UTC update: all four signed284 validators are live on420.
Both native admission checks, coordinated startup, current authorship and common
canonical-child finality12,648,106 passed. India's qualified bounded rejoin
completed with EU/SEA resumed and their own paused WALs preserved. All four
counter windows matched28 new oracle transactions,7,858,616→7,858,644.
Auxiliaries are restored and all refresh guards are cleared. The public CLI
probe remains blocked by Cloudflare1010/HTTP403, so this is origin acceptance,
not browser-route acceptance. There was no TLS error.
The current420 catalog is published and read back on BOTH R2 stores, through
12,579,000, SHA `1efa3b92b87df3bf69aa7b731a91a2e93666ecbb41a2daab5e247c28dc048b52`.
Its three new segments contain30,000 blocks and35,878 transactions from India's
preserved checkpoint12,630,000; signed native build and verification passed for
both local copies. The420 catalog is installed on all four hosts, and all420
indexes have been authenticated and prewarmed on each host. Sustained capacity
and remaining retirement are not yet complete: free space is US33.772/EU43.609/
SEA30.722/IN27.796GB, with current adaptive decisions StopCheckpointWork on all
four. Native startup Normal does not substitute for sustained growth reserves.
The next conservative unpublished-history boundary is12,679,000.

September8 00:30UTC cleanup update:101 exact temporary local archive objects
were removed after full local and both-source hashes, absence of readers/maps,
file/parent identity checks and per-file intent/removal journaling. Allocated
bytes released: EU10,763,251,712; US2,390,360,064; IN2,000,363,520. SEA pending
reclaim staging was excluded. Both full R2 copies and every own state/WAL,
checkpoint, provider backup, manifest and signed artifact remain preserved.
This completed copy cleanup is distinct from logical legacy retirement. Its
source inventory is consumed and its old producers must never be replayed.
Postconditions verified all101 exact absences and unchanged validator processes.
Free space then measured US35.695/EU52.735/SEA30.355/IN29.154GB. US/EU logged
Normal; sustained capacity and checkpoint acceptance remain open. All four
authors and common finality12,660,675 passed, followed by exact live oracle
counter windows. Receipts and full checksums are in current-state.

A new raw source capture must separately charge every potentially retained SST
byte, even when a live source already has multiple links: periodic checkpoints
can remove their links later. The proposed EU capture scope includes the signed
25,922,849,178-byte startup/prewarm consumer reserve, a21GiB full source bound,
128MiB records and the entire1GiB native copied-control envelope. This is a
capture-only admission; each subsequent segment build needs independent actual
input and allocation checks. The old50GiB whole-producer wrapper is immutable
and must not be rerun with a smaller literal. Keep50,000-slot source finality
depth; a source snapshot's finalized slot cannot be inferred from the live tip.
Run native writable isolated snapshot work as the service account. Source and
staging require one shared mount for hardlinks; separate read-only state binds
cause EXDEV. Verify source inode/content/ownership and own stopped WAL before
and after capture, and require fresh four-validator finality after own restart.

For a **catalog-only refresh**, derive each reserve from the actual signed
startup, role-preflight and index-cache consumers. Both signed284 capacity
calculations already include8GiB checkpoint peak,1GiB mutable writes,1GiB WAL,
2GiB compaction and the larger of5GiB or5% of filesystem size. The current
207,071,854,592-byte filesystems require23,238,494,618 bytes for that native
Normal calculation. Retain the existing512MiB operator margin, giving a
23,775,365,530-byte target. A refresh that captures no new snapshot must not
silently inherit an additional snapshot-copy allocation from another operation.
The420 outer procedure reserves the ENTIRE725,164,927-byte index cache,
64MiB records, and all133,963,317 bytes of catalog copies before staging.
The signed `prewarm-indexes` command independently requires the full configured
2GiB quota above its23,775,365,530-byte reserve:25,922,849,178 bytes. Every actual
preflight input satisfied that stronger minimum plus64MiB records (India's
remaining margin was1,712,662,118 bytes). Evidence `b5cef4ec…` records the check.
Future outer plans must include this full-quota consumer explicitly: at least
26,123,921,359 bytes before catalog staging and25,989,958,042 afterward, rather
than using only the smaller index allocation. Do not edit a sealed in-progress
plan; preserve its binding and the signed command's independent fail-closed gate.
The420 procedure performs no explicit cache-body deletion and requires both
stopped native admission checks to report Normal. Native cache management can
evict disposable bodies while prewarming; index verification is not whole-object
verification of every historical segment.
This leaves all native/adaptive reserves intact and does not establish sustained
checkpoint capacity. Prior completed operation plans remain immutable.

Compute index-cache size from the signed layout:8-byte cache magic, actual
segment header (including any dictionary),32-byte segment trailer,53-byte frame
header, and compressed index frame. Do not guess the header/trailer sizes. The
420 calculation must reproduce the prior native417 result723,649,206 bytes
before accepting its1,515,721-byte append. Actual filesystem and CLI boundary
tests passed32 cases locally and the same32 on Linux before420 staging.
Current phase receipts and exact invocation bindings are in
`memories/repo/current-state.md`.

Prior September7 19:14UTC verification: the417 catalog refresh is complete and all
four signed284 validators are live with verified-cache roles. Index preparation,
both native admission checks, coordinated startup, current authorship and common
canonical-child finality passed. India followed the tip without committing after
restart; the qualified bounded EU/SEA pause let it enter BFT without another
restart or a WAL replacement. Both peers resumed with their own paused WALs
unchanged. All four then agreed on finalized block12,615,063. Exact live counter
windows matched28 new oracle transactions,7,853,320→7,853,348, on every origin.
Auxiliaries are restored and all refresh/rejoin guards are cleared. Full release
acceptance and physical legacy retirement remain incomplete. Updated19:44UTC:
EU completed checkpoint12,620,000 at19:42:08 with history start12,520,001 and
handoff root `e0bcb726764646e6805766d5e4e9a247039fb5470643845496f5ecdde9d82cda`.
US/SEA/IN skipped that checkpoint. The latest capacity decision is
StopCheckpointWork on all four; EU fluctuated through Normal immediately before.
Measured free space is US35.144/EU51.556/SEA32.622/IN31.428GB. Preserve all
completed checkpoints; a single EU completion is not sustained fleet acceptance.
The417 catalog is published on both R2 stores through12,549,000; its SHA is
`9568fd7a926f61eb05361722293e35718e9e00621be1960094f5610922b5d255`,
root `757b007add80d5c60f3f76980a760c1a62c29fdc323b62e9abadae24fbd75332`.
The active plan and phase receipts are in `memories/repo/current-state.md`.

The19:27–19:34 cadence observation failed: SEA spent3.08seconds in checkpoint
admission at12,618,000, followed by a Propose timeout on the other three peers
and round-one recovery at12,618,001. Signed284 resolves the catalog-bound
profile every1000slots before checking its10000-slot cadence, and nested
handoff calls redundantly validate the immutable catalog. Preserve failed proof
`ed75ce4c…` and diagnostic `e2634e49…`; do not discard the timeout to pass a
later subwindow. A correction is under qualification in the separate
`lichen-checkpoint-catalog-admission` checkout; it is not deployed. Any candidate
must preserve validated catalog identity, gap policy, coverage and handoff roots,
and pass release and sustained live gates before replacing signed artifacts.

Credential renewal must distinguish Archive gateway credentials from legacy
R2 headroom mounts. At20:07UTC on September7, the retained legacy mounts numbered
49US/15EU/20SEA/16IN (100total), with their current config hashes matching grants
expiring21:55:51/21:57:08/21:58:43/22:00:14UTC respectively. The shared Archive
gateway config has no session token; those legacy expiry times do not establish
its expiry. Evidence `2a81c0e8…` binds the live config fingerprints to issuance.
Use an actual mount/reference inventory and exact current path for each renewal;
do not reuse older94-unit expectations or credential success receipts. Preserve
all retained cold links and backups. The September7 online renewal successor
has now passed39 host cases and7 actual100-mount input cases both locally and
on Linux. It keeps signed284 validators running and treats the legacy mounts
as a distinct source from the active Archive gateway. Its binding requires the
current417 role, active PID and artifact, protected configuration, each own
state-directory identity, unchanged retained cold files/links and no process
holding a legacy descriptor or mapping during remount. All four hosts must have
verified rollback copies before any credential changes. Fresh host-prefix grants
are object-read-only and transferred privately through stdin.

EU and SEA have scheduled AIDE integrity scans. Preserve their definitions and
enabled state; arm a bounded independent restore timer before temporarily
stopping their original timer and scan. Actual systemd timers omit MainPID;
normalize that timer-only property to zero. A deliberately terminated oneshot
can report `failed`, `Result=signal`, `ExecMainCode=2`, `ExecMainStatus=15` even
after a successful `systemctl stop`, with MainPID zero. Preserve this exact
observed state before resetting that service's failure flag; require its original
invocation, an armed restore timer and zero legacy readers. Other failure states
require inspection. Reset only the deliberately stopped AIDE service, then
recheck the hold. Restore its original schedule and scan after all renewed mounts
pass authenticated object and boundary-byte checks. US/IN have no AIDE units.

Online renewal receipts must say `validators_stopped=false` and must not claim
an unchanged live WAL hash: consensus legitimately appends to each own WAL.
Do not use the old stopped-host worker for this operation. Current exact plans,
credential expiries and completed phase receipts are in `current-state.md`;
preparation or issuance alone does not prove a completed live renewal.

Completed September7 21:07UTC: all100 legacy mounts renewed and verified, all
four signed284 processes unchanged. New host grants expire September8 between
20:46:48 and20:46:59UTC. Fresh four-way canonical finality passed at12,635,351;
all origins counted exactly28 new oracle transactions,7,856,438→7,856,466.
EU/SEA integrity scans and original timers are restored; temporary restore guards
are cleared. No validator stop, state/WAL replacement, gateway change or R2
deletion occurred. This completed renewal remains separate from unfinished
physical legacy retirement and sustained Archive acceptance.

Completed September7 17:39UTC evidence, before this maintenance: Archive V2 was
live on all four signed v0.5.284
validators. All414 indexes and both native admission checks passed on every host.
Legacy cold directories were preserved, the staged active configuration installed,
and all four signed processes started and released through the paused barrier.
India required the documented bounded peer pause to enter BFT; its state stayed
in place, both peers resumed with their own paused WALs preserved, and fresh
four-way health passed. All four report verified_cache, healthy RPC, advancing
blocks, current authorship and matching canonical-child finality. No forbidden
FUSE descriptors or mappings were found. Previously active auxiliaries returned;
exact live counter windows each matched28 oracle transactions, with totals above
7.85million. Activation halt guards are cleared. Longer query/checkpoint/memory/
cadence observation is active; full history/source-outage, eligible retirement,
restart and final release/cleanup acceptance remain pending. Exact receipts and
active jobs are in `memories/repo/current-state.md`.

The16:27UTC sustained check found `StopCheckpointWork` on US/SEA/IN:
the adaptive growth reserve required48.73/47.83/46.10GB with36.63/33.20/32.67GB
available. Consensus and Archive reads remain active. EU had62.88GB available.
A successful initial Normal admission does not establish sustained checkpoint
capacity. Inspect current-invocation capacity decisions and completed checkpoint
metadata throughout a live interval; `getHealth.status=ok` alone does not expose
this checkpoint pause. Preserve all existing completed checkpoints while
qualifying reclamation. Do not reduce the reserve to make acceptance pass.
Catalog-bound checkpoints use10000-slot cadence; preactivation checkpoints use
1000. Observation deadlines must account for the next actual boundary and build
time. Fresh checkpoint preservation must bind active configuration hashes,
current signed process, exact source metadata and the measured SST inventory;
an older preactivation source wrapper is not valid for an Archive checkpoint.

At16:34UTC EU attempted checkpoint12600000 and terminally paused because
Linux refused to hard-link root-owned0644 SST230908.sst. The file is regular,
not a symlink; the runtime's `unsupported_hot_sst_link` classification must be
checked against actual ownership and filesystem metadata. A readable RocksDB
file is not necessarily linkable by the service user. Writable native maintenance
must use the validator service account and verify ownership of its resulting
database files before restart. The completed metrics-repair wrapper omitted the
systemd User property. The correction is restricted to inventoried immutable
SSTs, preserves byte hashes/inodes/modes and the WAL, and records each metadata
change. Do not disable Linux hardlink protection. A terminal checkpoint pause
still requires an own-state restart and an observed successful new checkpoint;
ownership correction alone is not checkpoint acceptance. No EU12600000
checkpoint was published, so proposed jobs bound to that checkpoint must not run.

All eight affected SST ownership corrections completed with unchanged contents,
inodes and modes. EU then restarted from its own state; all four active roles,
current authors and common finality passed again. A fresh counter window matched
exactly32 oracle transactions on every origin. EU's maintenance guard is cleared.
At17:33:45UTC EU successfully captured checkpoint12610000 and began background
history materialization. The completed checkpoint was subsequently observed with
history start12510001 and its nonzero Archive V2 handoff root. Its separately
preserved raw snapshot finalized at12605010 and produced three verified new
archive segments through12549000. This raw snapshot has no catalog-bound checkpoint
metadata. Never fabricate metadata or substitute it for a periodic HotRepair
checkpoint. Dual publication of the417-segment catalog completed; active
runtime catalogs remain414 until the separately verified refresh and restart.

For an append-only catalog refresh, preserve the old catalog and recheck the
running release, current invocation, active arguments, protected configuration,
state and cold-directory identities. Linux `/proc/PID/cmdline` ends with NUL;
compare parsed option values, including duplicate and missing-value rejection,
instead of relying on a raw suffix. Keep a fresh all-four health and stopped-WAL
barrier. Each stopped phase retains its own resource checks; renew a halt lease
only after all four stopped processes, native readers and own WALs are verified.
Bound the entire sequence of native commands and sequential starts in the outer
controller. The417 catalog requires723,649,206 index bytes, an increase of
1,464,436; already allocated414 indexes must not be counted as new allocation.
Before deleting a disposable cache body for temporary headroom, stop the reader,
bind its exact file inventory, and hash-check both independent R2 copies. Preserve
all indexes, R2 objects, database files and retirement evidence. Require measured
free space after eviction before catalog installation, native cache preparation,
admission or restart. Cache eviction is not physical legacy-history retirement.

Size full-history verification independently from runtime admission. In signed284,
`compute_archive_v2_checkpoint_public_history_manifest` eagerly collects the
entire Archive prefix's serialized blocks and other rows. The417-segment Testnet
catalog describes about84.76GB of uncompressed block frames. Do not launch that
whole-history materialization under the8GiB maintenance limit or treat a passed
40000-slot matrix as proof of production-scale memory use. Full logical parity
remains open until an equivalent bounded path is established and qualified.
The legacy validator manifest command opens hot/cold storage without attaching
an Archive V2 reader; substituting it alone would not prove the composed Archive
prefix plus catalog-bound checkpoint suffix. Preserve the exact checkpoint
handoff metadata and compare all required categories at the same fixed tip.

The following baseline
observations are historical. All four validators received
separately qualified allocator trials and recovered from their own state. The
latest completed1001-commit baseline window averaged348.20–348.27ms with
p95422.51–476.18ms, all four authors and matching finality. This does not close
post-Archive acceptance. The frozen Total Transactions defect is corrected in
signed v0.5.284; its mandatory tag workflow passed all11 jobs, including the
full40000-slot four-validator matrix and exact transaction-counter deltas before
and after Archive activation. All15 PR checks passed. The coordinated284 upgrade
completed with all8 installed/running artifact parity, each host's own state and
WAL preserved, all4 authors and matching canonical finality. Auxiliary services
were restored and halt-only guards cleared. The all4 live counter test passed:
5,923,943→5,923,971 exactly matched28 OracleAttestation transactions at each
origin; daily counters advanced too. Historical backfill has now completed on all
four validators; post-Archive counter acceptance also passed as recorded above.
This is origin
evidence, not a visual browser check.
After US cold placement and its own-state restart, all four origins passed again:
US increased5,927,298→5,927,329, exactly31new OracleAttestation transactions;
the other observed windows matched30,31and32 transactions respectively. Fresh
four-way authorship/finality and restored signed auxiliaries passed before the
US maintenance guard was cleared.

The historical source audit is complete through slot12,561,000:251archive segments
cover2,808,712blocks and1,907,416user transactions; the signed native profile of
72,000recent blocks adds12,155. Their boundary hashes match, and all four live
validators independently match the final checkpoint hash. The retained historical
prefix and source evidence are recorded separately from missing legacy bodies.
US historical reconciliation is complete: signed exclusive repair at12,567,285
set total7,844,442 and daily19,646. Complete canonical block verification then
matched404new oracle transactions to7,844,846; a separate live window matched
another3. Its maintenance guard was cleared after fresh all4 health and signed
auxiliary checks. EU has also completed its backfill: total7,844,894 at12,569,599,
followed by exactly308canonical oracle transactions to7,845,202 and a separate
live increment of2. India's repair is also complete: total7,845,520 at12,571,709,
followed by exactly194canonical oracle transactions to7,845,714 and a separate
live increment of4. Singapore's repair completed at12,578,029 with total7,846,914
and daily22,118. Absolute verification matched216new oracle transactions to
7,847,130, then a separate live window matched another8to7,847,139. All four
metrics guards are cleared after fresh fleet health, stable signed processes
and restored auxiliaries. Singapore completion proof:
`7a84c0bc58de1c77b33b08064e0cd3f0c8df5c0ab23c6a1feab7ab80aef3ceef`.
Source verification alone never establishes a corrected live total.

The previous dual-source published catalog had414 segments through12,519,000,
SHA `54453ebc6d12266a2325a3f88f857beef3f14081a18b80e8c06da72ede67d1fe`,
root `e4e946e80a85d2d085e18d04ecf8436e82a36a98f4d4713379df2210a673681f`.
Its three new10,000-block segments retain50,000slots of finality, passed native
verification on both copies, and were published with twelve verified immutable
writes and two conditional catalog updates. Publication proof:
`115b742dcec2b4c4cc9ea7ad5f208b77c8bb1c132eee47e323439b99ffcf104b`.
Catalog preflight and staging completed on all four hosts, proof
`0f8033a6414ce9046e511a97bf0bfef0bd755fefee019cf77d5e30f28f30d1f5`.
The eight mounted source inventories passed, proof
`7fa0602be25fbdca3816f183fe03a6ae00d672a7aa5207644bb23995c522c0b8`.
This establishes complete object/manifest inventory and catalog identity;
it does not claim whole-object hash verification or active runtime roles. The
existing pending-retirement inputs still bind their original388-segment catalog;
never replace it with the newest publication. Current process/WAL bindings and
active maintenance are recorded in the timestamped execution plan.

Signed US-primary index authentication passed for all414segments, measuring
722,184,770bytes against the2GiB quota; proof
`aed3beddb9d9f4a3b5af9f016fe655e440e3c223ece8ef07eb169898ec1bd7d2`.
It wrote no cache entries. Activation and rollback configuration files were then
staged on all four hosts, proof
`de24da2c7995ef0dba7947367ad1b384ca214b9723762db92c906d527cf5dce5`.
The stage contains actual nested old-catalog/catalog.av2 and new-catalog/catalog.av2
paths consumed by native extension checks. It preserves current TLS trust and
allocator settings. That staging step left live arguments unchanged; the later
completed configuration and startup phases activated the staged Archive V2 role.

Treat each completed phase receipt as immutable. Before every dependent stopped
phase, verify all four services are stopped, no signed native reader remains,
and every host still has its own stopped WAL. Complete index preparation before
running admission. The signed prewarm command first authenticates every source
index, then reads/imports those indexes and checks the complete resulting cache;
zero cache files during its first pass does not prove a stalled process. Observe
the exact process, cache progress and resource reserves without launching a
duplicate. Preserve partial cache/output evidence if its bounded execution fails.

The controller must establish its own SSH relay and downstream masters
sequentially before parallel requests. Another process's connections cannot prime
its private control sockets. Renew startup halt timers only after an independently
verified all-four stopped/WAL barrier; the renewed lease must cover every
sequential startup timeout. Require each native signed process to report Archive
admission and its starting slot before pausing it, then verify all four paused
PIDs, invocation IDs, start ticks and hashes before coordinated continue.

Post-activation verification must consume the active configuration hashes and
Archive-enabled RPC schema. An old baseline verifier requiring Archive OFF cannot
test an active deployment. Check signed installed/running parity, preserved keys,
source trust and state identity, four current authors, common canonical finality,
active verified-cache roles and no forbidden FUSE descriptors or mappings.
Restore only auxiliaries recorded active before this maintenance. Verify their
signed processes and exact live transaction-counter deltas on all four origins
before clearing the activation halt guards. Preserve partial guard cleanup: a
transient unit stop returning5 is acceptable only if it is independently observed
not-found, inactive and without a process.

The separate activation stop budget is33,154,593,756bytes: Normal23,775,365,530,
the full8GiB checkpoint allowance, all722,184,770 measured index bytes and64MiB
for phase records. Configuration staging already occupies disk and is not counted
again as future growth. All four passed that actual preflight; every consumer must
still remeasure before its own mutation. A failed final capacity check after a
halt timer is armed must cancel that timer before returning without stopping any
service. Activation/start/continue must reject an expired or absent halt timer.
Native admission invokes two bounded1800-second commands; its outer controller
timeout must cover both, including termination and evidence handling. Qualify
fleet barriers and recovery before the first coordinated stop. These preparatory
checks do not establish completed activation or legacy retirement.

The bounded16GiB bridge now has256 immutable SST copies on each of the US,
Singapore and India provider volumes, in new operation directories. All1536
dual-R2 backup copies were hash-readback verified first. Each provider mount and
all recorded block devices returned to their exact read-only state; old backup
entries are unchanged. US subsequently completed all256 live-reference replacements
and released the corresponding local hardlinks while stopped under its current
own WAL. Actual free space increased17,190,424,576B to38,689,783,808B. The
persistent provider copies remain read-only; both R2 backups remain preserved.
India also completed all256placements with17,192,734,720B actual free-space gain
to34,422,018,048B. Its own-state restart, restored signed faucet, freshall4
authorship/finality and exact live counter deltas passed before its guard was
cleared. Singapore also completed all256reference replacements, gaining
17,192,558,592B of actual free space to34,661,728,256B. Its own-state restart,
signed faucet, all4 authorship/finality and exact live counter acceptance passed
before its placement guard was cleared. Completion proof:
`0643764a24fe9fc8597043f5cd11921d4eea6012e910bd2aa2c781405c813180`.
The signed archive utility, running as `lichen`, passed a100-block before/after
query comparison through an SST symlink to a disposable read-only ext4 loop.
That fixture qualifies reader compatibility. The separately completed US placement
passed own-state restart and fresh all4 acceptance; it does not constitute legacy
history retirement or Archive V2 activation.

Mount identity checks must observe the host mount namespace. A systemd sandbox
with `ProtectSystem=strict` and `PrivateNetwork=yes` introduced a private mount
view with an additional `nosuid` option. The unchanged exact mount check correctly
refused that view. The corrected placement inspector verifies its mount namespace
equals PID1's and retains resource limits and `NoNewPrivileges=yes`. Do not weaken
the UUID, mount-option or block-device read-only comparisons to accept a sandbox
view. Qualify the actual child-unit properties, not only fixtures in its parent.

Keep capacity scopes explicit. The remaining stopped placement installs no
binaries and produces no blocks while stopped. Its qualified preflight and stop
both retain5GiBhard reserve,8GiBin-flight checkpoint reserve and64MiBjournal
overhead. They additionally require projected checkpoint admission after the
exact verified allocated-byte release, and space to restore every local file
while retaining the hard reserve. Check actual inode/link/allocated identities
before relying on the projected release, then require actual checkpoint capacity
after placement before restarting. The signed baseline-install plan and Archive
Normal/checkpoint limits remain unchanged. A checkpoint peak can still refuse
placement; do not remove protected backups to make that preflight pass.

When checkpoint publication briefly opens sufficient capacity, run the qualified
preflight and stop consecutively in one bounded operation. Refresh fleet health,
then re-sample actual capacity; an earlier free-space reading is only a trigger.
Both phases must retain their independent capacity and process checks. On any
failure or lost stop response, end the runner and inspect the service, guard and
stop receipt before continuing. Do not automatically retry a mutation. Singapore's
September7 first stop was refused before service changes because a new checkpoint
consumed the headroom during the delay after preflight. The bounded runner then
passed both checks and sealed its own stop at12:57:52UTC. Its file placement and
subsequent acceptance are separate operations, not implied by a successful stop.

Hot-repair checkpoints omit genesis. Native source profiling therefore needs
the authenticated archive genesis object and exact catalog identity as well as
the immutable hot source. Never treat a missing genesis error as an empty count.
The efficient historical audit pairs signed native full-object decoding with an
independent scan of the exact same object: whole-object hashes, frame hashes,
transaction hashes, Merkle roots, canonical continuity, transaction type and UTC
day accounting. Its100-block sample exactly matched the native profiler. The
fleet-wide historical repair is complete as recorded above. Native decoder peaks measured
3.5–3.6GiB; the continuing audit uses a5GiB job cap plus a separate7GiB checkpoint/
host reserve, zero swap and a reserve-triggered job stop. Preserve completed
receipts across resource changes with explicit source and original-plan provenance.

Historical September7 observations below explain the memory investigation;
they are not current service or catalog authority. The signed v0.5.283 rollout reopened memory acceptance. Archive
V2 remains off and full release acceptance is open. The earlier 901-second
memory observation did not establish sustained stability. Singapore and India
required separate clean protective stops and own-state recoveries. Europe later
required the same protection at17.57GB RSS and1.91GB available; consult the
timestamped execution record for current process identities and acceptance.
The published and separately staged catalog has401 segments through12,398,000,
SHA `e038eb527a205c6fa6042911c8762c6aa3cb96a43f261265ed39a81fc5d856af`.
Both stores and all eight mounted catalog copies match, and all eight complete
source inventories passed. Later observations at00:51–00:57 UTC reopened the
memory gate: validator RSS reached roughly10–13.5GB while all four continued
advancing without restart. SEA and India had only about5GB available. Reclaim
staging stopped at its10GiB memory floor before native verification on SEA;
US staging completed. Singapore later reached15.18GB RSS with only1.95GB
available. Its clean stop preserved the exact WAL and signed artifacts.
Isolated signed checkpoint-manifest probes with default and bounded GNU
allocator settings both passed with identical manifests and approximately302MiB
peak RSS. This diagnostic did not reproduce live memory growth; it does not
qualify a production allocator change. The cause remains unconfirmed.
India subsequently reached15.5GB RSS with2.8GB available. Its stopped-state
allocator trial passed six local boundaries, five real Linux fixtures, the
actual systemd parser, exact signed-artifact checks and stopped-WAL binding.
The trial changed only three allocator environment settings on India and Europe
and verified them in each restarted native process. Both recovered from their
own stopped state and WAL. Fresh four-validator health after Europe's recovery
passed at fixed slot12,478,342, with all four authors and identical canonical
finality. Sustained memory acceptance remains open; this trial is not a
deployment default for any other network. Singapore subsequently passed its
own actual service-user and systemd fixtures, received the same separate trial,
and recovered from its current own WAL. The post-Singapore fleet check passed
all four authors and identical canonical finality at slot 12,492,926. Its trial
still needed observation through repeated checkpoints. US subsequently completed
its own qualified allocator trial as recorded in the current status above.
The earlier short observation is historical evidence. These observations do
not replace fresh process, capacity, source inventory and stopped-state gates.

The measured September7 platform inventory is heterogeneous. Record this per
host before every deployment; do not infer RAM or allocator behavior from a
different validator or the CI runner:

| Hosts | OS | Kernel | libc | Physical RAM |
| --- | --- | --- | --- | --- |
| US, EU, Singapore | Ubuntu25.04 | 6.14.0-37-generic | glibc2.41 | about24.60GB each |
| India | Ubuntu26.04 LTS | 7.0.0-27-generic | glibc2.43 | about24.60GB |

All four report x86_64. The initial isolated signed manifest comparison was on
EU/glibc2.41; the first live allocator trial was on India/glibc2.43. Keep those
scopes distinct. The trial cadence passed1001 consecutive common round-zero
commits, averaging330.55–330.60ms with p95399.63–472.35ms, plus all4 authorship
and canonical finality. This is baseline trial evidence, not post-Archive
acceptance. All nine pending SEA reclaim-input objects and the existing US
object are staged and verified. The subsequent US bounded batch advanced its
journal through16 successful physical-reclaim passes, with five ranges still
pending. It processed5,906,311,727 estimated input bytes and reclaimed only69,036
additional physical bytes. Queue splits mean remaining range count is not a
completion percentage. US recovered from its own state and restored its faucet;
fresh four-validator authorship and canonical finality passed at12,488,123.
SEA's nine pending journals remain unchanged. Archive V2 is still off.

## Local and CI capacity preflight

Check every downstream native requirement before starting a long matrix. The
shell retention test's15GiB floor alone does not admit Archive V2 building or
checkpoint work. On the current tagged implementation, the writable filesystem
reserve is the maximum of the network floor (5GiB for `lichen-testnet-1`,10GiB
otherwise), five percent of actual filesystem total bytes rounded up, and the
512MiB evidence reserve. A segment build adds three2GiB envelopes. Runtime
checkpoint admission adds1GiB mutable writes,1GiB WAL,2GiB compaction and8GiB
checkpoint space. Cache admission also includes the actual configured maximum
source object and eviction margin. Compare all applicable requirements with
available bytes, then add measured build and whole-matrix growth headroom.
Derive constants from the exact release source and retain native runtime checks;
do not treat these values as permanent defaults for future versions.

The September7 Mac failure had26,390,167,552B available and required
31,161,690,727B for the native build. The earlier25GiB outer check was insufficient;
the checkpoint requirement on that same filesystem is37,604,141,671B before
additional matrix growth. Unused compiler artifacts may be reclaimed only after
checking their ownership, exact paths and open files. Preserve validator state,
signed releases and evidence. When local capacity is insufficient, run the full
unchanged matrix on the existing isolated release runner. Its successful result
is required before signing, publishing or installing the release; a partial local
pass is never full acceptance.

The first US reclaim attempt on September7 failed before journal progress:
`ArchiveV2Reader::open` creates `root/quarantine`, which was absent beneath the
root-owned0750 staging root. Object verification had passed because `verify`
decodes files directly and never constructs that reader. The original journal
and stopped WAL remained unchanged; US recovered from its own state, re-entered
consensus and restored its recorded faucet. Fresh all4 finality passed at12483026.

For every deployment, qualify the exact native command's directory lifecycle as
the real service user. For this reclaim layout, precreate `root/quarantine` with
explicit root:lichen0770 ownership/mode while keeping the verified catalog,
objects and manifests protected. Require a real native fixture using isolated
databases and copies of the actual journal/authorization, including both the
missing-directory rejection and the corrected path. The signed283 US fixture
reproduced exit2 before the correction and passed the bounded native reclaim
afterward. It changed only its copied journal. This qualifies reader startup and
journal I/O, not live SST amplification, sustained memory or full deployment.
Never interpret an object verification or systemd-parser test as this full path.

Check this lifecycle for every new staging root, including authorization-only
readers. On September 9 the grouped v0.5.291 authorization repeated this omission:
the objects and catalog were readable, but `quarantine` was absent inside the
read-only mount namespace. The first native call failed before writing a signed
record. Preserve that attempt's exact output, precreate the empty directory with
explicit service access, and use an exclusive continuation output directory.
Keep the catalog, objects and manifests read-only. A successful native
authorization establishes reader startup and signing; the first deletion pass
must still perform its own complete hot/archive equivalence verification.

When a finite maintenance batch ends, preserve its completed journal and exact
per-pass evidence. A later batch must use a new operation identity, the latest
journal hash linked to the previous sealed result, and a fresh clean stop with
the current own WAL. Never rewrite the original staging completion record to
pretend it contained the newer journal, restore its backup over progress, or
replay an exhausted operation. Recheck actual free capacity after recovery;
estimated compaction input is not a promise of bytes freed.

## Sustained memory and allocator trials

Record actual physical RAM, available RAM, process RSS, shared/tmpfs use, process
identity, restart count, and checkpoint completion over time. A configured block
cache limit is not a process memory limit. During this incident the hot RocksDB
cache remained near1GiB while old processes exceeded12–15GB RSS. Anonymous
mappings alone do not establish whether memory is retained by the allocator or
still owned by application data structures. Reproduce the relevant write and
checkpoint lifecycle; a read-only manifest scan is a narrower workload.

An initial900-second observation cannot close a failure that appeared hours
later. Compare the same workload and checkpoint generations through the earlier
failure window, including query traffic and all4 cadence. Keep resource-heavy
operations behind their own unchanged RAM/disk margins while this gate is open.

The India, Europe and subsequent Singapore qualification trials used this separate systemd drop-in, with no change
to ExecStart, network, state path, cache size, signed executable or existing
environment files:

```ini
[Service]
Environment="MALLOC_ARENA_MAX=2"
Environment="MALLOC_MMAP_THRESHOLD_=131072"
Environment="MALLOC_TRIM_THRESHOLD_=131072"
```

These settings bound GNU allocator arenas and fix mmap/trim thresholds;
[GNU's glibc2.43 allocator documentation](https://sourceware.org/glibc/manual/2.43/html_node/Memory-Allocation-Tunables.html)
describes their behavior. They do not establish an application memory ceiling.
The native India process reports glibc2.43; its complete allocator/preload
environment contains exactly the three settings above, with no hidden override.
Do not transfer this Testnet trial to mainnet or a fresh network without its own
completed qualification. Preserve the exact before/after unit-input hashes,
drop-in bytes, signed binary hashes, own stopped WAL and runtime environment
proof. Test the actual service user's reads and systemd parser before applying.

For rollback, cleanly stop the current trial process and capture its latest own
WAL under a new recovery operation. Require unchanged baseline files and the
exact trial drop-in hash, preserve that drop-in in the recovery record, remove
only that file, reload systemd, and verify the original effective environment
before starting from the current own state. Never restore the pre-trial WAL.
Re-establish BFT entry, all4 authorship/finality and auxiliary state after either
direction of the configuration change.

## Deployment record and ownership

Before operating, create one dated deployment record containing the network ID,
genesis hash, immutable release tag/commit, complete workflow results, artifact
and detached PQ-signature verification, validator/archive-tool hashes, compatible
rollback hashes, fleet addresses, and exact operator source hashes. Record the
single controller identity and recovery mechanism. On resume, inspect actual
processes, services, timers, journals, and evidence before starting any writer.
An old success file or historical runbook version is not a fresh preflight.

Before merging a qualified release change, inspect both repository merge options
and the target branch's protection. Repository-level `allow_merge_commit=true`
does not override `required_linear_history=true` on `main`. This repository's
protected main branch requires linear history; use the allowed squash merge,
match the exact reviewed head, and require all current protected checks. Verify
the resulting commit's tree against the qualified source. If an immutable tag
already exists on that qualified source, preserve its original commit and bind
every artifact/operator to that tag commit; never move it to the squash commit.

Before each retry, review the complete selected execution path and run its
non-mutating preflight against the actual saved inputs. Check producer and
consumer bounds together, including minimum/maximum bytes, file counts,
capacity margins, inherited defaults, and existing-output behavior. Fixtures
must cover the observed failure and boundary values; shell syntax checks alone
do not establish that the plan is executable. On resume, compare immutable
WAL/key/configuration/file identities exactly, but evaluate changing measurements
such as filesystem free space against their explicit capacity gates. Seal the
reviewed source hashes and input hashes before starting the production operator.

A tool returning a session ID means the command is still running. Before starting
any dependent phase, close the original session with exit zero and verify the
inner operation's success, complete fleet membership, phase, release and input
hashes. A successful transport call alone is insufficient. If a separate audit
assertion fails, resolve it before launching the dependent action, even when an
earlier preflight passed. Keep one mutation controller active at a time and
preserve partial results rather than starting a duplicate command.

All required release gates remain mandatory: formatting, locked workspace
clippy/tests, audit/deny, standalone contracts and genesis WASM, static
frontend/SDK/deployment QA, and the exact release's four-validator hot/cold,
fresh-join, outage, own-state restart, coordinated restart, and history parity
matrix. Deploy only signed tag-workflow artifacts from a clean release source.

For GitHub draft releases, an authenticated tag-specific REST lookup can return
404 even while the draft exists. Use the authenticated release listing or
`gh release view`, select the exact immutable tag, and verify every asset digest,
attestation, checksum and detached signature. Retrieve private draft assets with
the authenticated asset API; keep its temporary download URL in memory and out
of logs. A local verification marker does not prove four-host artifact staging.

Require two fleet-wide barriers for binary changes: every validator and affected
auxiliary binary process must be stopped before any replacement; every installed
target set and stopped WAL must match before any start. Fixture-test failed-host
responses at both barriers, then repeat the complete preflight on actual signed
stages. Bind any existing signer key, seeds and unit files as well as validator
identity, genesis and protected environment. Preserve their bytes and ownership.
During partial installation, watchdogs may halt the fleet but must never restart
individual hosts on mixed versions. Budget all sequential startup deadlines,
SSH timeouts and recovery leases together. Where loading times differ, load and
pause one validator at a time at its own invocation's established startup
boundary; verify all four paused identities before continuing them together.
Recovery must continue a paused process before requesting a normal service stop.
Do not clear restart history to make a preflight pass.

If an unstable old release restarts between preflight and the stop command,
retain the failed attempt and every host's actual completed stop record. The
September6 v281 US/EU processes OOM-restarted during this interval while SEA/IN
stopped successfully. A continuation must verify all staged signed artifacts,
the still-active recovery guards, current identities, and the exact existing
stopped WALs. Refresh bindings only for hosts that are still active, perform
their fresh preflight and stop, and re-prove all four stopped before sealing
the fleet stop proof. Never restart already stopped peers or reuse the old
process binding merely to replay the original controller. Test partial-stop,
lost-response, conflicting-record and final-barrier failures before resuming.

Exercise startup readiness on Linux, including process creation before exec and
an empty current-invocation journal. `Type=simple` may report an active MainPID
before the native executable is ready. Within a short deadline, require that
same service PID/invocation to expose the exact signed executable hash before
waiting for its startup marker. Never accept the initial process hash merely
because the unit is active. `journalctl --grep` returns exit 1 when no entries
match; only an empty stdout/stderr result is pending readiness. Permission,
transport, journal and persistent artifact errors still fail closed. After an
interrupted start, inspect the actual process and preserve its current WAL;
resume an already loaded, verified process without replaying installation or
restoring an older WAL. Test these boundaries with a real fork/exec fixture and
the host's actual journal behavior, not only mocked service responses.

An SSH failure can occur after the host has completed its startup pause. Inspect
the real PID, invocation, executable hash, process state, startup slot and saved
pause record before retrying. Check for an orphaned startup worker. If all four
hosts already have matching, completed pause records, revalidate the entire
four-host pause barrier and continue those exact processes together. Preserve
the failed controller response and seal the resumed result; do not call the
installation or startup phases again. Exercise each failed-host barrier locally
to prove that it prevents every continue signal.

Clear transient recovery timers before their associated service. Stopping the
timer may let systemd unload the inactive transient service immediately; a
subsequent stop can return exit 5. Treat that as completed only after explicit
`LoadState=not-found`, `ActiveState=inactive`, and zero/no MainPID checks.
Other stop errors or an active process still fail. Recheck the validator's
unchanged service identity after guard removal; never use `reset-failed` to
hide deployment or restart history.

Perform an ordinary signed baseline upgrade and verify four-way current finality
before a separately qualified Archive V2 configuration transition. A binary
operator that pins baseline environment hashes cannot also start after those
environment bytes have changed. Each transition needs its own fresh stopped
WAL, configuration-preservation and recovery proof.

Record UTC timestamps, source hashes and content-addressed evidence for each
gate. Keep failed logs and partial native results; do not replace them with a
later success. Record application acceptance separately from service health.

## Internal gateway TLS

The Archive V2 gateway is distinct from public RPC/WSS ingress. Its validator
client loads `LICHEN_ARCHIVE_V2_SOURCE_CA_CERT` as a trust anchor and sends the
protected bearer token to configured HTTPS object sources. The gateway must
serve a separate server certificate, not the root CA certificate itself.

1. Inspect the actual running gateway configuration and certificate on every
   host without logging credentials. Check validity dates, issuer/signature,
   SAN matching the configured URL, CA:FALSE basic constraints, serverAuth EKU,
   appropriate key usage, and the leaf's public-key match to its private key.
   A CA:TRUE certificate belongs in the trust store, not the server-leaf role.
2. Validate the staged gateway config with its protected environment. Preserve
   the old config and certificates, record hashes/ownership, and arm a bounded
   rollback before changing the active gateway. For ordinary renewal, sign a
   new server key/certificate with the existing CA and preserve validator trust
   configuration. CA rotation requires a separately qualified trust migration.
3. Wait for the actual HTTPS listener with a bounded retry deadline. A systemd
   Type=simple service can be active before its socket accepts connections.
   Retry startup connection failures only within the deadline; wrong HTTP
   status, certificate errors, configuration drift, and process restarts fail
   the gate. Do not disable certificate or hostname verification.
4. Run a read-only request using the exact release's reqwest/Rustls dependencies
   and features, with the configured CA and source URL. An unauthenticated
   request must complete TLS and return the expected 401. Bind the probe source,
   dependency lock, and build identity to the deployment record. A local probe
   is a diagnostic client; it is never a replacement production validator.
   Verify the diagnostic connection method before gateway mutation. US disables
   SSH TCP forwarding; use the authorized SSH command channel to its loopback
   gateway when necessary, without changing SSH access configuration. A tunnel
   connection reset does not establish a certificate failure.
5. From each validator host, authenticate to both sources using protected
   credentials and read complete catalog-selected object bytes. Require 200,
   exact byte count, and the catalog's object SHA-256. Include the historical
   object used by the deep-history acceptance test, not only a recent object.
   After activation, also require the signed validator's actual historical RPC
   read and source-outage/refetch tests. Transport success alone is insufficient.
6. Apply the config through a supported gateway reload or gateway-only restart.
   Check actual configuration as well as systemd capabilities: the September 6
   gateway had an ExecReload command but Caddy `admin off`, so reload failed.
   That configuration requires a gateway-only restart. Keep its admin interface
   disabled and repeat bounded listener readiness after the restart.
   Verify the operation did not change validator PID/invocation, binary hashes,
   state-directory identity, keys, genesis, or validator/source configuration.
   Clear the rollback timer only after the checks pass. On failure restore the
   exact old config, apply it through the supported gateway operation, verify service state, and preserve the
   failed evidence. Reject rollback if current or backup bytes have drifted.

On the current Testnet the internal endpoint is loopback port 19443 and URLs
use `/primary/objects/<sha256>.av2s` and `/replica/objects/<sha256>.av2s`.
The configured method is GET. HEAD and manifest routes return 404 by design;
use a bounded GET range with exact 206/Content-Range/Content-Length checks for
a small transport probe. This does not replace complete-object verification.
Unauthenticated 401 is expected. Authenticated `/primary/catalog.av2` can return
404 because this HTTPS route exposes objects only; catalog availability is a
separate configured-source check. Derive routes from the actual config on a
new network rather than copying this deployment's assumptions.

The September 6 incident reproduced `CaUsedAsEndEntity` with the exact-tag
client against the live EU gateway. The old certificate had CA:TRUE and was
served directly. Earlier Python/OpenSSL full-object checks accepted it and
therefore missed the incompatibility. Local regression checks demonstrated
rejection of that certificate and a wrong-name leaf, and acceptance of a
CA:FALSE leaf signed by the existing trust model. The failed activation did not
record its precise failing shell line; do not claim a missing trace exists.

The historical `memories/repo/lichen-v262-archive-v2-https-gateway-stage.sh`
and `lichen-v262-archive-v2-https-gateway-stage-all4.sh` generate CA:TRUE
certificates and configure those same certificates as gateway server leaves.
They are incident provenance, not suitable fresh-network provisioning commands.
A future gateway installer must generate and configure a separate CA:FALSE
server leaf and pass the exact-client gate above before validators depend on it.

The gateway correction subsequently passed on all four Testnet hosts: eight
exact-client TLS/401 checks and eight complete, authenticated historical-object
readbacks, with unchanged validator processes and common finalized history.
Its evidence is SHA-256
`8561e91bdb866e9591a0fdaf17750408963b09600c0aa652bfde2bab982f6179`.
This closes the gateway certificate correction, not Archive V2 activation or
the remaining release-acceptance gates.

For renewal, record leaf and CA expiration, renewal owner and automation,
monitoring/alert thresholds, and a tested rollback. Renew before the remaining
validity becomes shorter than the deployment/recovery window. A 90-day leaf
requires renewal monitoring; installing it alone does not establish automation.
Run the exact-client and complete-object checks after every renewal.

## Capacity and catalog freshness

Run the signed native role preflight on every host against its actual disk and
role. Include checkpoint staging and steady-state requirements. A consensus
bootstrap reserve is not the checkpoint/Normal-capacity target. Record free
bytes, calculated required bytes, role, catalog hash/root/end, and finalized tip.
Recheck after long preflights or ordinary chain growth before activation.

Apply this complete-path check before the long local validator test as well.
Its retention-loop floor and download reserve may be lower than the native
archive builder requirement. The September 6 Mac builder needed 31,161,690,727
bytes and rejected 28,994,560,000 available after public-history parity. Check
the actual native requirement plus run-growth margin first. Reclaim only unused
compiler cache after open-file/process checks, preserving test evidence and
owned checkpoints for the harness's supported exact resume.

Include both retained checkpoints and concurrent replacement staging in the
disk inventory. After the September 6 TLS correction, completed checkpoints
and replacement staging accounted for about 4–9.5 GB per host, leaving US,
SEA and India below the earlier admission target. Check filesystem free bytes
alongside checkpoint directories and active exports; `du` can double-count SST
hard links shared with live state. Do not delete active staging, pinned exports,
or live state files to make a preflight pass. A startup-time success must also
leave room for subsequent checkpoint replacement and archive reads.

Measure at least one complete publication cycle when free space oscillates.
Before Archive V2 activation, a catalog-unbound HotRepairV1 checkpoint can still
use the 1,000-slot interval even when catalog-bound checkpoints use 10,000 slots.
On September 6, the old release repeatedly materialized 100,000 slots of history;
replacement staging temporarily exceeded the roughly 4.2 GB completed output.
Check actual configured behavior, unique allocated blocks, and open descriptors.
An idle measurement is not the peak requirement, and a native publication
reclaim must be observed rather than assumed. Do not keep retrying the same
artifact preflight while the measured capacity is below its unchanged threshold.

Artifact staging has its own complete byte budget and an explicit deployment
mode. The original v0.5.283 wrapper incorrectly imposed the future Archive V2
Normal target on a binary-only upgrade with Archive V2 off. The signed runtime
uses its ordinary disk guard when no Archive V2 role/root is configured; the
adaptive Archive V2 capacity guard belongs to the later role transition.

The corrected, separately qualified Testnet baseline mode requires **16 GiB
(17,179,869,184 bytes)** remaining reserve: the unchanged 5 GiB native safety
floor, 8 GiB for checkpoint replacement and 3 GiB for maintenance growth. The
observed replacement peak was 5,481,463,808 bytes. Stage and install consumers
bind this exact component budget, require explicit `--network=testnet`, reject
every `--archive-v2-*` runtime argument, and preserve the checked service,
environment, keys, own state, and signed artifacts. This mode cannot authorize
mainnet, devnet, role activation or retirement, and is not a universal default.

For this baseline, add 63,391,836 compressed artifact bytes, 143,137,573 expanded
bytes, **106,691,280 actual installed rollback binary bytes** and 67,108,864 bytes
of staging margin: **17,560,198,737 bytes before a new stage**. Measure the eight
installed binaries from the current inventory: the older 143,127,418-byte
rollback estimate overcounted them and must not be copied into a future record.
After staging require the full 16 GiB again; before atomic install add the actual
replacement binary bytes and 64 MiB. Existing complete stages require identity
and hash checks and remaining reserve, not a second payload copy. The **Archive
V2 Normal target 23,775,365,530 bytes and activation/checkpoint target
32,365,300,122 bytes remain unchanged**. Recompute future deployment budgets from
their exact artifacts, native policy and measured growth; do not carry either
an activation budget or a Testnet exception into an unrelated phase or network.

Catalog-only staging while this explicit Testnet baseline remains Archive-OFF
uses the same16GiB reserve plus two actual43,558,193-byte catalogs and64MiB
margin:17,334,094,434 bytes before copying. Validate both sources and the native
catalog identity on every host before any persistent copy. Repeat capacity and
process checks immediately before each copy; compare free space to the bound,
not to its earlier value. Stage under a separate recovery directory, fsync the
files and proof, and publish the completed directory atomically. A lost response
requires inspecting that exact directory; a partial stage is preserved for
review. This budget permits no live configuration, cold placement or role change.

Verify staging access under the actual utility user before running a native
command. A restrictive `umask 077` filters `mkdir(0750)` to0700; explicitly set
and check the final mode on newly created, owned staging directories. For an
interrupted stage, verify its saved plan and ownership before repairing its
permissions. Check every ancestor and execute a read-access test as `lichen`.
Preserve the native exit code, stdout and stderr even when verification fails.
Do not change unrelated recovery directories or access configuration to make
the utility run. Exercise the restrictive mask, service-user reads, interrupted
0700 directories, symlink rejection and unknown permissions on Linux.

Budget source scans from observed metadata latency as well as byte throughput.
On September7 the401-segment mounted-source inventory required up to592 seconds
for a source's native status and manifest checks; the original180-second native
deadline interrupted a healthy scan. Directory enumeration and bounded600-second
native calls completed all eight source inventories. Save each host's completed
result separately, preserve failed attempts, and distinguish metadata inventory
from whole-object verification. A longer read-only deadline changes no capacity,
memory, retirement or history gate. Do not diagnose CA or credentials from a
timeout alone; inspect sanitized mount errors and direct source reads.

Pin the completed snapshot's actual file inventory and finalized metadata.
Creation reports may count staged input files rather than final output files:
the September 6 EU snapshot report counted 124 input SST hardlinks, while the
completed RocksDB checkpoint contained 126 SSTs after isolated staging opened
and flushed. Verify final filenames, sizes, identities and control-file hashes;
do not substitute an input-stage count or assume checkpoint metadata filenames.

For this v0.5.280 Testnet plan the four-host Normal target is 23,775,365,530
bytes: 23,238,494,618 required plus 536,870,912 margin. The prior SEA
15,185,430,938 bootstrap target was insufficient. This is deployment-specific,
not a universal constant for future networks. Preserve the 5,368,709,120-byte
reserve, 536,870,912 estimated-input cap, and one-range reclaim limit. Do not
raise limits or delete unsupported history to force admission.

Check every consumer of catalog coverage: native preflight, runtime admission,
and periodic checkpoint construction can have different recent-tail windows.
For the fixed v0.5.280 catalog ending at 12,290,190, native preflight with the
100,000-slot window and the observed default checkpoint settings reach their
coverage boundary at 12,390,190. Runtime admission's additional verified local
tail does not extend those other gates. Recompute from the selected release
source and actual configuration; publish/verify newer coverage and perform the
required coordinated catalog reload before crossing the earliest boundary.

Record each read-only object-store grant's actual issue/expiry time. An
idempotent refresh script can return an old grant's evidence. Renew at a safe
maintenance boundary and verify the new grant. Never remount under a native
maintenance process or delete R2 objects. Temporary headroom relocation is not
native archive reclaim and is not durable Mainnet storage provisioning.

Inventory the actual mount units, source prefixes and private config fingerprints
before planning renewal. The September6 fleet has100 headroom mounts, while94
older grants share the earliest expiry; neither count is a replacement for the
current inventory. Use the existing config section (`r2` here) and endpoint,
and bind each grant to read-only access under its host's retained-object prefix.
For the stopped-host procedure, collect samples only after its proved
all-validator/all-native-writer stop. The qualified online procedure instead
requires its active Archive role and legacy-reader barriers described above. Verify
one whole authenticated object per host and bounded bytes on every referenced
mount after renewal; renew unreferenced mounts too while retaining their data.

Before replacing any active credential file, save and verify every original
config on all hosts. For offline renewal, bind to the signed transition's
completed stop proof and repeat stop/WAL/identity checks before each host
mutation. For online renewal, bind to its active signed process and role,
retained-source identities and guarded zero-reader barrier instead.
Save newly issued payloads privately for resumption. A lost response must trigger
read-only inspection of the remote prepared/completed state, not another grant
or duplicate backup. Preserve interrupted writes; reject unknown config drift
and partial preparation. Hold a host-level exclusive operation lock across each
phase so a timed-out SSH connection cannot leave an old worker overlapping its
retry. An offline renewal keeps the fleet stopped until mount verification and
own-WAL preservation pass, between the coordinated stop and binary install.
An online renewal keeps validators active and restores held integrity monitoring
after renewed-source bytes and fleet health pass. Record the actual lifecycle;
neither procedure alone establishes release acceptance.

## Coordinated activation and recovery

1. Finish TLS, source-byte, signed-artifact, catalog and rollback checks, and
   measure actual capacity while validators are still available. Pin the
   qualified operator sources and verify there is no other controller or
   maintenance writer. The qualified v0.5.280 activation runs its full native
   role preflight during the coordinated stop, before the role switch. Live
   capacity measurements do not replace that admission gate. Account for this
   preflight time in the maintenance window and recovery lease.
2. For consensus-critical deployment, stage first, stop all validators, prove
   they are stopped, then install the signed artifacts on all hosts. Preserve
   each host's state/WAL/keys, cold storage, identity and access configuration.
   Archive-role-only activation still follows its qualified coordinated plan.
3. Arm bounded recovery before mutation. Capture stopped WAL and directory
   identities. Retry existing gateway units only if the complete expected set
   is byte-identical, active, and stable. Partial units, symlinks, and unknown
   drift fail closed; do not overwrite them to make a retry pass.
4. Start all four from their own state. Require Archive V2 admission, explicit
   BFT entry, local COMMITTED events and current authorship on every validator.
   A startup message, advancing RPC tip, or three active producers does not
   prove the fourth is participating.
5. If three producers outpace the fourth's startup catch-up, diagnose the actual
   process state. Use only a qualified, bounded catch-up procedure with fresh
   PID/start/invocation pins, independent automatic resume guards, preserved
   stopped WAL, and verified restoration. Never reuse an earlier run's PIDs or
   weaken the signed admission gate.
6. Require a common finalized block and canonical commit certificate, deep
   historical RPC reads, both-source parity and outage/refetch, and absence of
   forbidden legacy FUSE runtime descriptors. Only the complete activation
   success marker closes this phase. Admission alone is not acceptance.
7. On failure capture the phase, host, failing line/exit status, sanitized RPC
   error and relevant service journal before recovery. Never log tokens,
   private keys, or environment contents. Restore the qualified baseline,
   validate signed installed/running parity and all four local commits, then
   seal recovery evidence. Keep the final activation gate open.

## Runtime query regression found on September 6

Signed v0.5.280 passed native admission and exact-client TLS, but the latest
activation failed its runtime gate and recovered to the preserved baseline.
During the failed run, the cache fetched large historical segments in ascending
order while fresh RPC timed out. Local reproduction against exact tag
`162a4c2c51385d6a1307490f749126c633d24040` confirms that
`get_recent_txs_paginated_exact_filtered` reads Archive V2 from slot zero even
when the requested page is already filled by newer hot transactions. Ordinary
`getRecentTransactions` traffic reaches all four validators. This is a confirmed
query defect and a supported explanation for the observed scan; the stripped
production stack alone does not identify the exact initiating request.

Before the next activation, qualify recent-query traffic with a populated hot
suffix and a much older catalog. Verify that a complete hot page fetches no
strictly older archive object, while partial pages, exclusive cursors and
overlapping boundary slots retain exact results. Run traffic during finality
and source-outage checks, and bound both request latency and archive work.
Also review other aggregate-history paths; a fix for recent transactions does
not by itself establish bounded account counts, activity queries or deep pages.
Any code correction requires a fresh signed workflow release and its complete
gates; never install the local diagnostic build or retag v0.5.280.

The corrected slow-source regression also confirms executor starvation on the
original tag: with a genuinely hot tip at slot 1 and a delayed authenticated
archive source at slot 0, an account-history query prevents a single-worker
runtime from answering getSlot within 250 ms. The candidate isolates database,
archive and VM RPC work in four bounded blocking jobs; its permit remains held
after a client disconnect until the job finishes. Capacity exhaustion returns
503 with JSON-RPC code -32005. This is candidate behavior, not an installed
v0.5.280 capability. Verify transaction submission and proof-lock behavior under
load as well as liveness before accepting a successor.

Aggregate queries must filter account/program/pair prefixes before accumulating
results across the catalog. The candidate authenticates disposable public-index
bundles against the catalog's original envelope, dictionary and frame hash.
Index and full-object cache files share one quota. An index cache hit does not
prove that an unavailable block body exists or that every byte of a complete
object is valid; whole-object verification and public-history parity remain
independent mandatory gates.

For a successor containing `prewarm-indexes`, use the signed archive utility
while the validator is stopped. Pin the actual network ID, genesis hash and
catalog root; provide separate existing catalog/source/cache directories,
`--cache-quota-bytes` and `--reserve-bytes`. Run the complete command first
with `--dry-run`, then with `--acknowledge-stopped-validator`. Both paths
authenticate every selected source index before writing. The command rejects
insufficient quota, requires free space for quota plus reserve, and execution
checks every cached entry after import. Preserve the native JSON results and
exact command/input hashes. Do not confuse the acknowledgment with an actual
service check: the controller must independently prove the validator is stopped.
The installed signed v0.5.283 utility includes this command. Earlier v0.5.280
does not; select the utility from the verified release descriptor, never from a
historical command example or a locally compiled candidate.

Recompute index size from every active manifest, including cache envelopes;
do not carry a preceding deployment's quota into a new plan. The September 6
388-segment catalog has 708,442,599 compressed public-index frame bytes, before
cache envelopes, and 1,129,847,239 uncompressed compact bytes. This fits the
current 2 GiB cache but does not establish a perpetual growth budget. Warming
indexes must be followed by timed aggregate/deep queries, normal transaction
traffic, current finality, and source-outage checks on the final signed release.

When collecting Caddy diagnostics, parse on the host and emit only approved
fields such as timestamp, status, duration and route. Caddy error records can
contain origin-auth headers: never return raw request/header records. Method
traffic diagnostics may retain method counts only, without bodies or headers.

## Retirement and full acceptance

Include memory in the deployment budget and acceptance evidence. Record the
actual configured hot/cold/object caches, process anonymous RSS, host
MemAvailable, service cgroup memory, swap and restart history. Cgroup memory
includes file cache and cannot substitute for process RSS or host availability.
Observe representative query load across a fully published checkpoint; completed
category messages alone do not prove publication. Preserve OOM timestamps and
old/new invocation IDs, diagnose the initiating allocation, and do not infer
that a runtime query fix also fixes a separately observed OOM. A recovered
service is not by itself sustained-memory acceptance. On September 6 SEA and
India automatically restarted after OOM kills on signed v0.5.280; their new
identities require explicit fresh bindings, rather than silently resetting
NRestarts or continuing with a previous PID.

Signed v0.5.281 subsequently passed its initial four-validator baseline check,
then EU and India were OOM-killed again at 16:07:06 and 16:01:38 UTC on
September 6. The activation preparation rejected India's changed invocation
before any configuration stage or write. Preserve that failed preflight and
the earlier successful baseline proof as separate events. Refresh all four
service bindings and repeat the complete actual-input preflight after a restart;
neither clearing restart counters nor replaying completed backups is a recovery
procedure. The later signed v0.5.283 checkpoint changes passed the full release
matrix and the 901-second live query/checkpoint observation on all four hosts.
That short observation initially passed the baseline memory gate. The later
September7 RSS increase reopens it; retain both results and diagnose the new
evidence before adding memory-heavy work. Archive V2 memory and query acceptance
also require observation after activation.

Inventory every database instance, including read-only checkpoint verification
and snapshot serving. The live `--cache-size-mb` argument does not necessarily
reach those consumers: the v0.5.281 checkpoint-opening path passes no explicit
cache size and derives a separate limit from host memory. Record those defaults
and their concurrency in the memory budget. A logged 1 GiB live block cache
does not establish a 1 GiB process limit. Measure process RSS independently and
use resource-bounded diagnostics on a preserved immutable checkpoint; do not
stress a nearly exhausted validator host or treat a cache hypothesis as a
confirmed OOM cause.

For the disposable Archive V2 disk cache, inventory existing `objects` and
`indexes` together. Native eviction shares one quota between both directories;
warming indexes can replace already allocated objects. Record actual occupancy
and maximum additional growth rather than adding the entire quota a second
time. Retain the native prewarm free-space check and the separate checkpoint
peak requirement. Count physical checkpoint reclaim conservatively: hard links
shared with live state do not yield their logical file size when unlinked.

After activation, execute the qualified source-backed, additive, resumable,
conflict-aborting retirement sequence. Preserve native journals and interrupted
results. Count only measured physical reclaim as reclaimed disk. Resume after a
safe operation boundary, not by interrupting a healthy native body pass. If the
plan freezes finality during maintenance, state that explicitly in the record.

Require four-validator finality/cadence, all-four authorship, one-validator
outage/rejoin, own-state restart, all-validator restart, installed/running
artifact parity, adequate disk capacity, deep historical reads and complete
genesis-to-tip logical public-history parity. The existing `lichen-testnet-1`
waiver covers only unavailable signed block bodies 2,872,006..4,298,999. It is
not transferable to devnet, fresh testnet or mainnet. Those networks fail closed
on incomplete history. Current 200 GB Testnet hosts are not approved for
Mainnet or indefinite archive growth.

Complete live DEX/CLOB/AMM, prediction, governance, launchpad/listing/graduation,
and SDK/frontend/platform acceptance with authorized test funds. Verify the
requested LICN price on the live chain and 0.15 defaults for new-network genesis;
never rewrite an existing genesis. Publish release status/frontends and perform
reference-checked cleanup only when the applicable acceptance gates pass.

The final deployment record must state passed, failed, and pending gates;
evidence paths/hashes; exact deployed identities; rollback; certificate and
credential renewal; capacity/catalog deadlines; and the next permitted action.
This makes a resumed deployment verifiable without treating a narrative status
or an old success marker as current fleet evidence.

## Transaction metrics acceptance and backfill

Signed284 qualification covers failed durable writes, replay, both canonical
storage orders, UTC rollover and restart recovery. Keep the exact live delta
gate in the mandatory four-validator matrix before and after Archive admission.
Repeat it on every deployed origin and the explorer route after maintenance.
Record source SHA, observed slot range, transaction types and exact total/daily
deltas; never accept a changing row list as proof of aggregate accounting.

Coordinated maintenance can lose a capacity window between preflight and stop
while a checkpoint writes. Use the exact artifact-derived stage/install budgets
and inspect completed versus active checkpoint generations. Preserve successful
stops and own WALs when another host refuses its check; independently inspect
the remaining host and complete only its unfinished stop after capacity returns.
Do not replay the all-host stop or reinstall a partially upgraded fleet blindly.
The September7 continuation retained all original checks and installed only after
the renewed four-host stopped/WAL barrier passed.

A changing Latest Transactions list does not prove that Total Transactions or
its UTC daily counter is correct. Canonical execution must commit its metrics
with the first durable block anchor. Completing secondary indexes must neither
skip counting nor count twice. Require crash/failed-write/replay/restart tests,
including native oracle, EVM and consensus-only blocks. Observe actual transaction
traffic; an idle window cannot establish this gate.

Run `python3 tests/live-transaction-metrics.py --rpc-url <origin-RPC>` against
each origin and the explorer's same-origin API using its existing authorized
access. The check compares total/daily/block deltas with every canonical block
in the observed range, verifies continuity and rejects frozen or double counts.
Repeat after own-state/all-validator restart and after Archive V2 activation.
If an access layer rejects a local probe, retain that failure separately; it is
not a successful counter sample and does not justify changing TLS or access policy.

For a backfill, preserve the old durable values and bind a fixed canonical
frontier. Independently verify the complete transaction source and reconstructed
count before writing; record the source, prior values and applied correction
atomically and make retries idempotent/conflict-aborting. Do not infer the
correction from block-count differences, sum unverified secondary indexes, or
add client-side increments to conceal a server undercount. The Testnet signed-body
waiver cannot silently turn missing history into zero transactions. A historical
prefix counter needs exact source and slot provenance. Fresh networks/mainnet
have no waiver. Verify the repaired count after restart and through the explorer.

Use this ordered procedure for each validator's historical correction:

1. Preserve a checksum-bound historical basis, including its canonical end hash
   and UTC-day subtotals. Qualify the signed native reader against that host's
   actual hot/cold source and genesis before stopping it. Close the native reader
   before entering the stop barrier.
2. Sample atomic `getMetrics`, including `last_observed_block_slot`, then stop
   only that validator under a fresh operation identity. Preserve its own WAL,
   service identity, signed artifacts and prior auxiliary states. Do not reuse a
   stop receipt from an earlier upgrade or placement.
3. Profile the observed anchor, the complete suffix from the audited basis to
   the actual stopped tip, and any blocks committed after the RPC sample. Check
   genesis, both boundary hashes, coverage and UTC sums. Derive the expected old
   durable counters from the sampled values plus that final canonical delta.
4. Independently review the resulting exact plan and source manifest. Invoke the
   signed `metrics-reconcile` tool exclusively on that stopped own database.
   Require the exact expected old counters, current tip/hash/date and an atomic
   applied receipt. Preserve the unchanged WAL and the receipt before restarting.
5. Restart its own signed state, restore only previously active auxiliaries and
   establish fresh four-validator authorship/finality. Verify the repaired
   absolute total against all subsequent canonical bodies, then verify a fresh
   live transaction window. Only then clear that operation's maintenance guard.

Pace verification reads. A temporary429/503 may be retried with bounded backoff;
permanent errors and incomplete bodies fail the check. Long absolute windows
must retain every block and boundary while using the signed verifier's unchanged
2048-slot chunks. Intermediate cumulative counts derive only from canonical
bodies; the final counts must equal the actual atomic RPC sample. Qualify gaps,
boundary conflicts, wrong totals/daily counts and exhausted retries before use.
Keep operator evidence separate from a browser check of the explorer route.

## Catalog refresh and publication timing

Record the local role catalog and both published source catalogs separately.
Publishing an append changes the source bindings before it changes the running
reader. An old operation whose immutable inputs include the previous source
catalog cannot authorize the refresh. Preserve the old local catalog, verify the
exact append with the signed CLI, and qualify the recovery path against the new
source catalogs before stopping. Changing only a wrapper's expected root or
segment count is insufficient.

If reconstructible cache cleanup is needed, authenticate the stopped cache and
its immutable sources first, retain an exact intent inventory, and measure free
space after cleanup. Charge the full future cache quota unless a separately
qualified calculation accounts for the preserved, authenticated allocation.
Charge every simultaneous catalog copy, cache metadata, operation records and
the unchanged native recovery reserve. A successful online projection does not
authorize the next disk consumer.

Exercise fresh fleet barriers between long phases and before each coordinated
startup. Measure complete prewarm and admission duration: source validation,
cache import and final cache validation are distinct work, and an empty cache
during initial source validation is not itself a failure. Inspect retained output
and the current unit before retrying any phase. Clear maintenance guards only
after fresh process-bound finality, canonical transaction-counter and auxiliary
service checks.

Barrier freshness uses each host's observation time, not the time the controller
finally receives all replies. A slow peer can age out earlier observations.
Preserve every host's result, refresh only aged observations within a bounded
retry policy, and require the unchanged native freshness limit before mutation.

Exercise both positive and negative host/controller clock skew before the first
stop. A strict nonnegative receipt age can reject a valid observation when the
remote clock is slightly ahead. Use bounded waiting for genuine observation
timestamps, including enough settling time for a receiving host whose clock is
behind. Never rewrite timestamps, extend freshness limits or make an old record
fresh. Reject skew outside the qualified bound and retain the failed receipt.
The September9 adapter admits at most2s observed skew and waits3s at receipt and
barrier boundaries; Linux/controller failure cases and the real all4 stopped/WAL
barrier passed before use. These bounds are operation-specific, not a substitute
for synchronized clocks or qualification on a new network.
Qualification must exercise a delayed last peer as well as immediate replies.

A lost SSH reply does not prove that the remote operation failed. Inspect the
specific service, operation ledger and retained native output first. If the work
completed, collect and authenticate its existing receipt without rerunning it.
Any continuation must name the exact failed operation and verify the already
completed hosts. Keep finite guard expiry and local control-machine availability
visible throughout maintenance; do not carry an old-version guard-renewal worker
across a binary installation without qualifying it for the new installed state.

For file maintenance, measure the real guard and reference-scan duration as well
as transfer speed. Charge how often they execute across the complete selected
file set; a deadline for a smaller selection cannot qualify a larger pass.
Journal each replacement before mutation and verify its completed receipt before
skipping it on continuation. Reject gaps, changed file identities, unowned
temporary links and changed completion records. Preserve finite pass and health
limits; use a fresh health proof before starting the next bounded continuation.

Before starting a long verifier or retirement window, calculate catalog runway
from the native admission boundary, current finalized slot and measured slot
rate. Include the time needed for publication, verification and the next
coordinated refresh. Publish a sufficiently large verified extension from an
eligible preserved source; a small successful append is not proof of sustained
operation. Source-index prewarming alone is neither whole-object verification
nor full public-history parity. Renew expiring legacy-source credentials while
their references remain, separately from active Archive V2 source credentials.

Calculate a catalog adoption's restart window from the **incoming verified
catalog**. Calculate a retirement window from the already adopted catalog.
For signed291, admission uses the configured recent-history window and permits
at most50,000additional unpublished slots. On this Testnet, the actual runtime
argument is100,000recent slots. The qualified operator retains10,000slots for
recovery: latest stop = catalog coverage end +100,000+50,000−10,000. Verify the
actual argument and signed source, reject missing/duplicate values, and retain
the native startup check. For catalog474 ending13,110,000, this gives13,250,000.
Do not carry a previous catalog's literal stop slot into a later adoption or
change a runtime retention limit to make an expired operation pass.

After a bounded physical-reclamation pause, compose and verify the exact
accepted journal hashes before another lifecycle. Completed tombstoning passes
must not run again. A signed native report can record successful range splits
with zero compacted ranges and a larger queue; those splits are progress toward
the unchanged input limit. Keep the same finite time, range, memory and disk
limits. A report with neither compaction nor splitting and no smaller queue is
a pause requiring inspection. Logical deletion totals are not measured disk
recovery, and a successful source check does not imply compaction completed.

## Completing a temporary R2 migration

Archive V2 activation and R2 retirement are separate acceptance results. On
September 10 the owner confirmed that R2 is temporary and must be emptied after
migration. Four healthy `verified_cache` validators whose authenticated gateways
still read R2 do not satisfy that completion requirement. A dated prefix such as
`archive-v2/v0.5.238/lichen-testnet-1` can contain the current active catalog;
never classify its contents as obsolete by version or age alone.

Before changing archive sources, inventory actual mount devices and available
bytes on every proposed destination. A directory named `ovh-backup-*` does not
prove a separate mounted volume. Preserve retained provider recovery data and
charge complete archive copies, indexes, migration scratch, checkpoint peaks,
growth and the native reserve. Reclamation means measured released filesystem
blocks, not merely tombstoned logical bytes. Existing 200 GB Testnet disks are
not an approval for mainnet or indefinite history growth.

Place complete content-addressed archives on independently verified persistent
destinations, retaining current sources until every destination passes full
object, manifest and catalog verification. Record actual source endpoints and
failure domains, qualify the supported source configuration change, and verify
deep-history reads and source outage recovery from every public RPC origin.
The ongoing tail publisher must write and verify the replacement destinations;
moving only the existing prefix leaves a future R2 dependency.

After acceptance, retire exact R2 gateway, mount, publisher and credential
consumers. Check live process descriptors, mappings, configuration and rollback
references before deletion. Preserve immutable evidence and required recovery
copies. Delete from an exact-key, size-and-hash-bound manifest using durable
per-batch receipts; an uncertain request requires outcome review before any
continuation. Finish with both buckets empty, no incomplete multipart uploads,
no consumers able to recreate objects, and fresh fleet/history acceptance.
Dashboard storage totals alone do not prove completion.

## Archive migration capacity and verifier admission

The combined diagnostic and deployment SSH rate must fit the host firewall's
actual connection policy. On September 10, the US server limited new SSH
connections to fewer than six within 30 seconds. A complete peer-health batch
can consume five connections to its proposal-sampling host; overlapping
diagnostics or the next batch can therefore receive `Connection refused` even
while the validator and SSH service remain healthy. Consolidate reads and allow
35 seconds between these batches. Do not weaken the firewall. A five-second
retry loop can keep refreshing the limit window rather than allowing recovery.

If such a failure occurs before a target mutation, verify the controller's exact
dispatch order and the target's durable records before continuing. The India
catalog adoption retained its successful stop and catalog installation when a
peer read failed before prewarming. Continuation required an unchanged stopped
WAL, exact completed-stage receipt, active original guard, absent prewarm intent
and native service, and sufficient remaining guard time. Repeating a successful
stop or catalog installation would be incorrect.

An active verified-cache role proves that Archive V2 serves history. It does not
prove that legacy hot/cold data has been retired or that the expected disk space
has been reclaimed. Track those as separate outcomes, including unique allocated
bytes before and after eligible retirement and retained checkpoint hardlinks.

For a long verifier, account for the validator's concurrent mutable writes and
SST rewrites, plus blocks retained by checkpoints and separately preserved
sources. On September8 the fixed-source verifier stopped after5244seconds when
free disk crossed its26,057,066,906-byte guard; scratch stayed below450MB and
memory remained within its bound. No complete manifest resulted. Retain the
source and terminal receipt, inspect the actual allocation change, and change
the qualified execution conditions before retrying. Do not lower a guard solely
because a sampled free-space figure would pass it.

A binary replacement using an unchanged catalog is a separate operation from
catalog publication and index prewarming. Derive each phase from its real
consumers: download, expansion, retained rollback, atomic installation, cache
growth, records, native memory and live recovery. Charge shared filesystems
together. Existing cache occupancy may reduce future allocation only after a
stopped service/WAL/reader barrier and authenticated stable file inventory;
reject corrupt, shared, sparse, unexpected or quarantined entries. Preserve the
normal native reserve and account separately for physical allocation overhead.
A helper's arithmetic tests do not qualify the complete stop/install/recovery
procedure. Never reuse the older Archive-OFF budget for an active Archive V2 role.

When a compaction batch exceeds available headroom, changing the actual native
input cap is a separate, reviewable plan change. Derive both the outer admission
threshold and every inner native command from that same cap; retain the runtime
reserve, recovery headroom and journal allowance. For example, the September10
US continuation changed the native cap from1GiB to256MiB and kept the26,777,786,778
byte runtime reserve. Its complete threshold became29,529,250,202 bytes
(reserve + twice256MiB +2GiB +64MiB). All nine commands, report validators and
insufficient-space boundaries were checked, followed by67 tests on Linux/ext4
as the actual service user. Lowering only a wrapper threshold is not equivalent.
A smaller cap may leave indivisible work pending; preserve that native result
and recover the validator before planning another batch.

Resuming a partial group requires a fresh operation identity and the exact
accepted journal frontier. Verify the directory contains precisely the completed
journals, hash each private service-owned file, reject symlinks/shared or unknown
files, and keep unstarted entries absent. Distinguish the staged catalog bound
to existing retirement authorizations from the newer live reader catalog.
Recheck physical presence only for the remaining untombstoned ranges, then let
the signed native retirement code revalidate each authorization. Neither an
empty-directory assumption nor replaying completed tombstones is a valid resume.

Qualify filesystem checks at the actual target. On this fleet US `/tmp` is a
separate tmpfs, while validator data uses ext4. A test of allocation behavior on
`/tmp` cannot establish ext4 behavior or data-volume headroom. Run disposable
fixtures on their intended filesystem, bind the device identity, use the actual
service account and resolve executable paths before restricting the child PATH.
Set `TMPDIR` for the tested process and assert the device and filesystem of
`tempfile.gettempdir()` inside it. Merely placing the test script on the data
volume does not move its temporary fixtures off a separate `/tmp` tmpfs.

Before hot retirement, prove actual local source rows for every one of the 21
public-history categories. A contiguous slot index and repaired account index
can coexist with canonical block bodies already absent locally. Native
retirement equivalence checks both hot and cold sources and rejects that case
before its first journal or tombstone. Exercise this failure against the actual
candidate ranges before a long maintenance window. An archive object's complete
proof establishes the replacement's contents; it does not establish which
physical local rows remain. Preserve failed native output and inspect every
expected journal before deciding whether any deletion happened. Never create a
journal, metadata row or block body just to make the admission check pass.

Use read-only physical checks before scheduling the stop. The signed validator's
binary public-history stream can feed its import checker with `--dry-run`,
without `--execute`, a secondary database or a cold-store attachment. Run the
checker in a namespace where the actual live source is read-only. Authenticate
the expected rows against the catalog commitments first; a successful export
alone can derive secondary rows from blocks and does not prove those rows are
stored locally. Require zero conflicts and incomplete-block upgrades, and require
every row to be identical rather than reported as an insertion. Apply only the
missing-row exception explicitly supported by the signed retirement code; v291
permits an already-absent deterministic `tx_by_slot` row after verifying its
canonical block and transaction. It permits no other absent category.

For canonical streams, preserve native page cursors and transaction ordinals,
bind each signature to its authenticated segment index, normalize only the
certificate fields defined by the native codec, sort by the category's canonical
key order and compare the complete catalog commitment. Charge the retained rows,
parser buffers and both native processes to the memory limit. Keep partial
per-category reports after interruption, and do not convert a partially completed
scan into a complete segment proof. The first native retirement pass must still
run its own full equivalence check before its journal or tombstones.

Catalog-only adoption after legacy cold removal uses the already verified role
marker and own stopped WAL. Initial role bootstrap requires local genesis and
must not be reapplied to this state. Require an exact append-only native catalog
check, both complete source inventories, authenticated cache prewarming and
actual marker-based runtime admission. Bind published and running catalogs
separately while adoption is incomplete. The old runtime catalog can remain
behind a newly published catalog; the consumer checks must describe both actual
states instead of assuming publication also updated each process.

For the September 10 catalog 462 operation, the unchanged native reserve was
26,777,786,778 bytes. The complete adoption floor was 29,730,576,794 bytes after
charging a 2 GiB index-cache budget, four 64 MiB catalog allowances and a 512 MiB
operation margin. The immutable-source builder separately retained a 40 GiB
floor and charged its producer, both outputs and seed objects. These are dated
operation calculations, not reusable defaults: derive bounds again from the
actual next candidate and test every wrapper consumer before the first write.

A composed public-history acceptance must retain the completed full native
prefix evidence, exact append-only catalogs, independent native tail manifests
and the native handoff binding. Check key overlap wherever native composition
can reject conflicting rows across that boundary. For signature-keyed `tx_meta`,
the September 10 read-only check authenticated all 460 relevant source indexes,
reproduced the native tail digest, and joined all prefix/tail keys using bounded
SQLite scratch. Other categories require their own key/slot reasoning; empty
tail categories and slot-keyed rows must be demonstrated from actual manifests
and source code. Such a proof does not produce a new native manifest hash and
does not independently authorize R2 deletion. Keep current fleet recovery,
exact obsolete-prefix mapping, fresh zero-reference scans and deletion metadata
checks as explicit acceptance requirements.

## Returning validator readiness and auxiliary scan holds

RPC health alone does not establish voting readiness. Require all four current
authors to advance and compare a fixed canonical block and commit certificate
across the fleet. If a returning node receives blocks but never proposes, inspect
its current invocation's pre-consensus messages and time every configured
bootstrap endpoint. A local tip captured before slow asynchronous RPC requests
can appear behind even when block application has caught up. Read the local tip
after those observations; retain the existing drift, post-effects and finality
checks. Apply this to both returning-node and post-registration readiness.

The signed runtime has an exact-tip stalled-quorum recovery path. Any temporary
process pause used for recovery must have its own qualified scope, original
process and binary bindings, independently armed automatic resume timers, and
unconditional resume on controller failure. Verify actual voting and finality
afterward. A successful recovery does not qualify an unshipped code correction.

When holding integrity scans for legacy mount credential renewal, a deliberately
terminated scan can finish with systemd `ActiveState=failed` and `MainPID=0`.
Preserve its exit evidence. Before clearing that stopped status, verify the exact
invocation and expected termination, empty cgroup, no legacy file consumers, and
the original integrity-restore timer. `reset-failed` only clears unit status; it
does not establish that readers stopped. Resume the qualified hold checks and
restore the original scan scheduling after authenticated mount verification.
