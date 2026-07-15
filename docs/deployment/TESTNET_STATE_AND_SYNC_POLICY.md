# Testnet State and Validator Sync Policy

**Created:** 2026-05-02
**Status:** Mandatory operator policy for testnet, staging, and mainnet-like environments

This policy exists because an active testnet is shared infrastructure. Developers may have wallets, contracts, tests, indexes, and automation attached to the running chain. Treat testnet state as persistent unless the network owner explicitly approves a reset.

## Non-Negotiables

- No testnet, staging, or mainnet reset without explicit owner approval for that exact action.
- No validator recovery plan may assume "reset from genesis" as the default fix.
- If a validator shows signs of partial/bootstrap-local state (for example: chain-tip exists but no local genesis or no slot 0), do not patch individual RocksDB files and do not copy a peer database. The supported single-node recovery is to preserve evidence, let the built-in partial-bootstrap scrub remove only that validator's incomplete local chain DB while preserving its own identity and seed config, then let it sync from peers. For the current July chain, an apparent fleet-wide gap is an incident blocker: preserve every store and locate exact backed data; do not reset the chain.
- No validator may be brought online by copying another validator's live RocksDB state directory.
- No validator may receive another validator's consensus WAL, peer cache, node identity, validator keypair, signer state, or mutable runtime state.
- No deployment, hotfix, rollback, or incident response may silently invalidate developer-created testnet state.
- Public native transaction submission has one format: a base64-encoded
  `lichen_tx_v1` envelope. Raw bincode, transaction JSON, alternate RPC names,
  and signatures without the native chain-ID domain are not supported.
- Historical storage decoders are not public compatibility APIs. They exist
  only where required to replay the current testnet from genesis, read its
  persisted blocks/receipts/state, or open a backed pre-candidate cold archive.
  They must never be used as fallback admission paths for new transactions,
  P2P messages, CLI commands, RPC methods, or WebSocket subscriptions.
- Every new or returning validator must prove it can start from its own state directory and sync from the network.
- A running validator that falls materially behind must return from LiveSync to
  bounded sequential catch-up before processing future blocks. Checkpoint,
  state-root, fork, and BFT-commit repair paths must make the same transition.
  Raw block receipt and pending queues are not progress: only canonical commits
  and accepted verified snapshot chunks may refresh the liveness watchdog.
- Account activity is a persistent user-facing index. Do not run destructive
  `account_txs` rebuilds on a pruned/checkpoint node unless every indexed row
  can be proven from retained canonical block bodies. Snapshot/export paths
  must preserve both hot and cold `account_txs`, `transactions`, `tx_to_slot`,
  and block data needed for wallet and explorer history.
- Public-history repairs must be additive and source-backed. When the source
  has a separate archive/cold store, `--merge-public-history-from-source` must
  be paired with `--source-cold-store` on both dry-run and execute so hot and
  cold backed history are restored together.
- When the verified source is another live VPS rather than a locally mounted
  DB, use `scripts/stream-public-history-repair.sh`. It exports whitelisted
  public-history pages from the source and imports them into one stopped target
  at a time; it must not copy the source validator's RocksDB, WAL, identity, or
  peer cache into the target.
- Block repair must always provide explicit slot bounds. Before any write, the
  candidate's `--verify-contiguous-block-range` command must prove the source
  has every canonical body in that range with valid slot and parent linkage.
  `--from-slot` and `--to-slot` are inclusive, and the source enforces the upper
  bound inside canonical slot and transaction iteration so pages cannot cross
  into later slots or truncate transactions in the final slot.
  The repair helper also requires current provider-backup confirmation,
  identical candidate hashes, a mandatory complete target dry run with zero
  conflicts, and measured free space covering 150% of the missing key/value
  bytes plus the 10 GiB runtime reserve before the target is stopped.
- Large block-body ranges must use the script's binary framed stream mode
  (`--public-history-page-format binary --stream-pages` under the hood) rather
  than JSON/base64 pages. The stream mode still performs additive imports and
  conflict aborts, but it keeps one source and one target process open for the
  range and avoids retaining huge raw page payloads as evidence.
- Public-history parity is a validator-set invariant. If one validator can
  serve backed historical `getBlock`, `getTransaction`, or
  `getTransactionsByAddress` data that another validator cannot serve, the
  fleet is not archive-ready even if consensus health and state roots match.
- Archive readiness must be proven with the canonical public-history manifest
  command in `PRODUCTION_DEPLOYMENT.md`, not by checking only `getHealth`,
  `getSlot`, or current checkpoint roots.
- Fleet readiness must also run `scripts/verify-testnet-archive-parity.sh`
  against US, EU, SEA, and IN. The live read-only pass is useful for diagnosis;
  the strict release gate uses `--stop-for-manifest` with the exact confirmation
  string so every validator manifest is computed at one fixed tip before the
  services are restarted. The verifier reuses one SSH control connection per
  host; do not replace that with repeated short sessions because the VPS UFW
  policy deliberately limits six new SSH connections in 30 seconds.
- Canonical genesis bootstrap must carry the full state/public-history snapshot
  surface needed by network joiners. A joiner that imports block 0 must not miss
  slot-0 explorer rows such as program calls, events, transaction indexes, or any
  other public-history category that the genesis creator recorded before storing
  the genesis block.
- Do not delete the richest verified public-history source to make disk usage
  look uniform. Repair the missing validators from that source, then verify
  archive parity across the fleet.
- Public validators are not allowed to specialize as partial archive nodes.
  Capacity is approved from measured retained hot/cold bytes, the operation's
  bounded write/compaction peak, and the runtime reserve. A nominal disk-size
  threshold is not proof that a validator can or cannot hold the same
  genesis-to-tip surface.
- Every non-development testnet and mainnet validator runs archive-backed
  public history automatically. The validator derives the cold store as the
  `archive-<network>` sibling of its configured state directory, enables
  archive retention without an operator flag, and rejects both `--archive-mode`
  and `--cold-store`. Those flags are development/admin controls only;
  public-network correctness must never depend on an optional service argument.
- A validator that reaches the 10 GiB runtime safety floor must stop with
  persistent exit status 78 and remain stopped until capacity is expanded.
  Checkpoints are skipped below 20 GiB free. Restart loops and history deletion
  are not capacity-management mechanisms.
- A verified snapshot must not create rollback state or clear any live category
  until measured free space covers the staged replacement, compaction peak, and
  runtime reserve. Nominal disk size and hardlink checkpoint apparent size are
  not capacity proofs.
- Any full-column-family scan over point-lookup- or prefix-optimized RocksDB
  storage must explicitly enable total-order iteration. The stopped-node cold
  migration audit validates every raw hot block hash, canonical slot cursor,
  block-referenced transaction body, and exact transaction-to-slot value across
  hot and cold storage. Execute mode writes cold first, syncs its WAL before hot
  deletion, and compacts bounded hash ranges so migration never requires a
  second full archive's free space.
- Sparse Merkle node and leaf column families are derived current-state caches,
  not historical block or contract storage. Canonical root updates must retire
  superseded rooted nodes atomically with their replacements so cache size is
  bounded by the reachable current tree. Operators must never remove these
  files directly. A repair uses only the typed stopped-node sparse rebuild,
  reconstructs from canonical accounts and contract storage, verifies the
  computed and stored roots, and proves slot, identity, genesis, and archive
  evidence unchanged before restart. Older roots remain available only through
  independently retained verified checkpoints, whose hard links are released
  by normal checkpoint retention after a new checkpoint is created.
- Before starting from any provider-restored database or native checkpoint,
  verify every active hot/cold descendant is owned by the validator service
  account and that the service account can create a same-filesystem hard link
  from a representative immutable file. Provider restores must normalize and
  then re-inventory ownership without changing file contents. A failed ownership
  or hard-link preflight is a stop condition, never a restart-loop trigger.
- Snapshot activation is complete only when the target slot and state root match
  and the combined hot/cold public history is contiguous from genesis through
  the target. Root equality alone is not success. A root-valid target with
  missing public history must restore every exact pre-apply hot category from
  the local rollback profile, including public-history rows and account-history
  counters, while preserving the validator's independent cold archive. The
  recovered checkpoint must be persisted and synced before the durable recovery
  marker is removed; any cleanup failure is fatal.
- Consensus v1 post-block repair and chain-domain transaction verification use
  one durable upgrade activation boundary, not marker absence by itself. Fresh
  chains persist slot 1. For an existing public chain, stop every validator at
  one exact tip/hash and dry-run then execute
  `--prepare-consensus-v1-activation --activation-slot <common-tip+1>` on every
  independent database with confirmation
  `consensus-v1-activation:<network>:<slot>`. Each write must WAL-sync and
  read back the same value. Startup without it exits 78; it must never choose a
  node-local height. A missing marker before that slot is legacy or pruned state
  and must never trigger replay; transactions below it retain the bounded
  `v0.5.223` chain-domain-then-legacy transition policy. At and above the
  boundary, native transactions must be signed for the canonical chain ID with
  no fallback. Inside the activated interval, repair
  requires the exact canonical block and fails closed when its body is
  unavailable. Analytics and every coordinated state-projection migration must
  wait for the same boundary. An operator must not invent, backdate, change, or
  delete it.
- For an existing public chain, the complete genesis configuration embedded in
  slot 0 is the runtime authority. Cached genesis files may be durably repaired
  from that payload; explicitly supplied conflicting files must fail startup.
  Consensus timeouts must never drift by host-local configuration.
- Consensus restart rounds may be recovered only from durable signed WAL or
  authenticated peer vote evidence. Block age, process start time, database-open
  duration, and local wall clocks must not choose a BFT round.
- Local liveness observations must not change validator membership. Mainnet
  downtime/jailing requires a certificate or signing-window commitment agreed in
  canonical blocks and applied at deterministic height or epoch boundaries.
- A block's locally collected quorum-signature subset is not canonical history
  merely because it verifies against the same block hash. Blocks after height 1
  commit the parent certificate, exact parent-height validator powers, and child
  fee/oracle metadata in a versioned consensus transaction at index 0. Proposal,
  sync, mainnet startup, and RPC paths verify that certificate against the parent
  header's `validators_hash`. The same child certificate commits the parent's
  complete post-effects state root. Checkpoint metadata proves that exact
  certificate is transaction 0 of a signed and finalized child header with a
  Merkle inclusion proof and the child's authenticated historical powers.
  Archive/RPC parity uses this canonical child representation. Mainnet rejects a missing envelope. Historical testnet blocks
  from before the coordinated upgrade remain readable but are not accepted as a
  new mainnet format.
- Fresh chains must atomically advance a genesis-to-tip archive watermark with
  canonical block storage. Verified snapshots may set that watermark only after
  complete block, parent-link, transaction-body, and transaction-index
  verification. Mainnet startup must fail if the proof differs from the tip and
  must re-read the complete hot/cold chain behind the proof before consensus.
- `getHealth.archive_contiguous_slot` and
  `getHealth.archive_contiguous_hash` are operational evidence, not substitutes
  for fixed-tip fleet manifests. Once present, a watermark behind the tip is an
  `archive_incomplete` health failure.
- Do not treat an incomplete source set as archive parity. If every live
  validator returns `Block not found` for a historical slot, the fleet has a
  backed-source gap. Release is blocked until a real backup/archive/source is
  found and repaired from. The current July chain must not be reset, replaced,
  or synthesized to hide such a gap.
- Multi-source repair targets must remain stopped until
  `scripts/verify-testnet-archive-parity.sh --offline-repair-gate` proves the
  same fixed-tip genesis-to-tip manifest on every validator. This gate never
  restarts validators; start all services coordinately only after it exits zero.
- If a broad public-history dry-run reports conflicts in block body or slot
  cursor column families but reports zero conflicts in transaction/account
  history indexes, use `--merge-public-history-indexes-from-source` with
  `--confirm public-history-index-merge:v1`. Do not force a broad merge or
  replace live RocksDB state to recover wallet/explorer activity.

## Allowed Artifacts

A validator may receive only these network artifacts:

- The release binary and matching contract bundle.
- Its own validator keypair and node identity.
- Service configuration, bootstrap peer addresses, and network ID.
- The canonical genesis descriptor or genesis hash for the target network.
- Governance-approved service secrets needed for that operator's own services.

The canonical genesis descriptor is not a copy of a live validator database. It is the public network starting point. Any post-genesis chain state must be obtained and verified through the node's normal sync path.

## Prohibited Artifacts

Do not distribute any of these to make a validator catch up:

- `/var/lib/lichen/state-*`
- RocksDB SST files copied from another validator
- `consensus_wal*` or any validator WAL data
- `peer_identities.json`, peer cache, TOFU cache, or banlist from another host
- another validator's `home/node_identity.json`
- another validator's `validator-keypair.json`
- `genesis-wallet.json`, `genesis-keys/`, or admin signer material

## Joining Validator Procedure

1. Install the same release binary and contract bundle used by the active network.
2. Create or install only the joining validator's own keypair and node identity.
3. Configure bootstrap peers and the canonical network/genesis identifier.
4. Start with an empty validator state directory, except for the joining validator's own local identity files.
5. Let the validator obtain canonical genesis information and chain blocks through RPC/P2P sync.
6. Verify catch-up by comparing:
   - `getHealth`
   - `getSlot`
   - `getHealth.archive_contiguous_slot` against the canonical tip
   - recent block hashes at the same heights
   - fixed-tip hot/cold public-history manifests
   - validator logs for sync completion and absence of state-root mismatch
7. Record the slot/hash evidence in the deployment or incident notes.

## Incident Response Rule

During an incident, the recovery order is:

1. Preserve evidence and backups.
2. Diagnose logs and deterministic failure height.
3. Patch code without changing chain state when possible.
4. Restart or rolling-restart validators in place.
5. Let lagging validators resync from peers.
6. Escalate to an owner decision only if the active chain is unrecoverable.

A reset is a last resort and requires explicit owner approval before execution, even on testnet.

## Test Gate

Before declaring any sync-related change production-ready, run or document an equivalent gate:

- one validator produces blocks,
- a second validator joins from empty state and catches up through sync,
- the joiner crosses at least one epoch boundary or the current configured epoch verification point,
- the joiner restarts and resumes from its own local state,
- one running validator is stopped, the remaining validators keep finalizing,
  and the stopped validator rejoins from its own state without a state copy,
- one LiveSync validator process is paused while the remaining quorum advances
  across a material slot gap, then the same process resumes, catches up within
  the configured drift bound, and retains the same hot/cold manifest as its
  peers,
- the seed or primary bootstrap validator is stopped and restarted from its own
  state, with the expected quorum behavior documented for the active topology,
- the configured seed and its direct RPC are offline while the surviving quorum
  restarts together from preserved state; durable peer discovery, signed
  active-validator tip evidence, and BFT must recover without changing seed
  files, copying state, or starting the offline validator,
- the full local validator set is stopped from a healthy tip and restarted from
  preserved state, proving stale same-tip recovery does not require wiping
  RocksDB, deleting WAL, replacing archives, or regenerating identities,
- canonical transaction execution, receipts, block body, transaction-to-slot
  indexes, and finalized cursors survive a close/reopen as one durable boundary,
- startup post-block recovery followed by sync accepts every transaction whose
  blockhash remains inside the canonical recent window; a partial in-memory
  cache warm-up must trigger a storage-backed window load rather than reject
  the block,
- startup and the pre-BFT parent gate complete every missing block-hash-bound
  producer and deterministic post-block effect for an activated, recently stored
  canonical parent before the next-height state root is read; a second pass is
  a no-op, and pre-activation marker absence never replays effects,
- checkpoint verification succeeds after validator-set expansion without using
  the receiver's current stake denominator and rejects detached root/proof tampering,
- hot/cold archive reads for old `getBlock`, `getTransaction`, and account
  history survive migration and restart,
- archive parity manifests and representative historical RPC probes match
  across the full local validator set,
- no copied RocksDB state or WAL is used at any point.

This gate must be repeated after consensus, state-root, replay, genesis, P2P sync, checkpoint, or validator lifecycle changes.

Final activity evidence is freshness evidence, not a lifetime counter check.
Each validator's own RPC tip and canonical `last_active` slot must remain within
the configured release-gate drift bound of the common reference tip before
offline archive manifests are compared.

Current configured verification point: `SyncManager::checkpoint_interval()` is the short-form checkpoint gate for local and VPS sync verification when waiting for the full 432,000-slot epoch boundary is impractical. The local validator gate in `tests/local-multi-validator-test.sh` starts joiners from keypair-only state, rejects copied RocksDB/WAL/genesis-wallet artifacts before join, waits through activation warmup, restarts a joiner from its own state, and rejects restart logs that re-import genesis instead of resuming.
