# Lichen Full Codebase Audit — 2026-03-26

> Deep line-by-line audit of every subsystem. All findings verified against source code.

---

## Executive Summary

**18 findings total** — 4 Critical, 5 High, 5 Medium, 4 Low.

The **immediate cause** of empty orderbooks on EU/SEA (the frontends showing no data) is a single root cause:

> **The genesis producer auto-funds the deployer wallet with 10,000 LICN from treasury via a direct state write. This write is NOT a block transaction and is NOT replicated by joining nodes. Result: genesis wallet has 0 LICN on joiners → all post-genesis transactions from that wallet fail.**

This cascades: mint calls fail → no lUSD on EU/SEA → DEX orders fail → empty orderbook.

---

## Finding Index

| # | Severity | Component | Finding |
|---|----------|-----------|---------|
| F-01 | **CRITICAL** | Genesis/Sync | Auto-fund not replicated on joining nodes |
| F-02 | **CRITICAL** | Block Receiver | FIX-FORK-2 allows duplicate block processing |
| F-03 | **CRITICAL** | State Persistence | Block commit and effects are non-atomic |
| F-04 | **CRITICAL** | Ghost Purge | Non-deterministic direct state write at startup |
| F-05 | HIGH | Validator Stats | Reputation updated twice on double-apply |
| F-06 | HIGH | P2P | 8+ message types silently dropped (no handler) |
| F-07 | HIGH | P2P | Orphaned blocks never re-requested |
| F-08 | HIGH | Deployment | LICHEN_CONTRACTS_DIR not exported in start script |
| F-09 | HIGH | Sync Bootstrap | Bootstrap grant replication timing-dependent |
| F-10 | MEDIUM | Fork Adoption | Crash during revert+apply creates partial state |
| F-11 | MEDIUM | WebSocket | No backpressure — slow clients cause memory growth |
| F-12 | MEDIUM | Paths | Database paths inconsistent across environments |
| F-13 | MEDIUM | RPC | Legacy admin endpoints bypass consensus |
| F-14 | MEDIUM | Orderbook | O(n) reconstruction from full order scan |
| F-15 | LOW | Logging | Double-replay warnings pollute logs |
| F-16 | LOW | Metrics | flush_metrics() saves counters, not account state |
| F-17 | LOW | P2P | Peer scoring not enforced (always 100) |
| F-18 | LOW | Config | Hardcoded 5% oracle band was too narrow |

---

## Detailed Findings

### F-01 — CRITICAL: Genesis Auto-Fund Not Replicated on Joining Nodes

**Files:**
- [genesis/src/main.rs](../../genesis/src/main.rs) lines 660–695 (producer path)
- [validator/src/main.rs](../../validator/src/main.rs) lines 7587–7596 (joiner path)

**Description:**
The genesis producer (`lichen-genesis`) auto-funds the deployer wallet with 10,000 LICN from treasury at genesis/src/main.rs:660-695:

```rust
// Auto-fund genesis/deployer with 10K LICN from treasury
let ops_fund_licn: u64 = 10_000;
let ops_fund_spores = Account::licn_to_spores(ops_fund_licn);
// ... debit treasury, credit genesis wallet via state.put_account() ...
```

This is a **direct state write** — it modifies two accounts (treasury and genesis wallet) without creating a transaction in the genesis block.

When joining nodes process the genesis block (validator/src/main.rs:7587-7596), they correctly compute:

```rust
genesis_account.spores = total_spores.saturating_sub(total_distributed_spores); // = 0
genesis_account.spendable = 0; // correct math: 500M - 500M = 0
```

But they **never apply the 10K LICN auto-fund from treasury**. Result:

- **US (genesis producer)**: genesis wallet = 10,000 LICN ✓
- **EU (joiner)**: genesis wallet = 0 LICN ✗
- **SEA (joiner)**: genesis wallet = 0 LICN ✗

**Impact:** ALL transactions from the genesis wallet fail on joiners with:
```
Fee error: Insufficient spendable balance: 0 < 1000000
```
This means lUSD minting, DEX orders, and all contract calls from the genesis wallet produce nothing on EU/SEA.

**Evidence:** EU logs at 13:30:55 UTC — slot 67 replay fails with exactly this error.

---

### F-02 — CRITICAL: FIX-FORK-2 Double Block Processing

**File:** [validator/src/main.rs](../../validator/src/main.rs) lines 7325–7344

**Description:**
The block receiver loop receives blocks from two channels:
1. `sync_block_rx` — responses to sync requests (priority via biased select)
2. `block_rx` — live BFT blocks from P2P gossip

During sync, the same block arrives on BOTH channels. A dedup mechanism at line 7307 checks `seen_blocks`, but the FIX-FORK-2 gate at line 7335 allows re-processing when:
```rust
if block_slot <= current && has_pending {
    // "Let through to fork choice for re-evaluation"
}
```

During initial sync, `has_pending` is almost always true → every block gets processed twice.

**Impact:**
- EU logs confirm: slots 76, 77, 80, 82, 83, 84, 88+ all show duplicate "Replaying" entries
- First replay succeeds (or fails due to F-01), second fails with "Transaction already processed"
- Double reputation updates (F-05)
- Wasted CPU, I/O, and log pollution
- US node also shows double replays at slots 177, 182 (harmless there since first replay succeeds)

**Evidence:** EU and US validator logs both show `✅ Tx replay OK` followed immediately by `⚠️ Tx replay failed ... (Transaction already processed)` for the same transaction hash.

---

### F-03 — CRITICAL: Non-Atomic Block Commit + Effects

**File:** [validator/src/main.rs](../../validator/src/main.rs) lines 3142–3500, 8088–8635

**Description:**
Block storage and effect application are separate operations:
1. `put_block_atomic()` — writes block + slot index to RocksDB (atomic WriteBatch)
2. `apply_block_effects()` — writes rewards, validator set, stake pool (3+ separate puts)

If the process crashes between step 1 and step 2, or during step 2:
- Block is on disk and tip advanced
- Rewards not yet distributed to accounts
- Validator set not updated
- Stake pool not updated

On restart, the node sees the block as already applied but effects are missing.

**Impact:** State divergence after crash recovery. Rewards permanently lost for that slot. Validator stats incomplete.

---

### F-04 — CRITICAL: Ghost Validator Purge — Non-Deterministic State Write

**File:** [validator/src/main.rs](../../validator/src/main.rs) lines 6283–6297

**Description:**
During startup initialization, if `current_slot > 100`, the code identifies "ghost validators" (registered but no blocks produced) and removes them, returning their bootstrap grants to treasury:

```rust
treasury.add_spendable(ghost_account.staked).ok();
state.put_account(&tpk, &treasury);
state.put_account(ghost_pk, &zeroed).ok();
```

This is a **direct state write** that depends on:
- When each node started
- Which validators were "ghost" at that point in time
- Whether the node was ever restarted

**Impact:** Different nodes can have different treasury balances and different validator sets depending on their startup history. This is a consensus-level state divergence that accumulates over time.

---

### F-05 — HIGH: Reputation Updated Twice on Double-Apply

**File:** [validator/src/main.rs](../../validator/src/main.rs) line 3215

**Description:**
In `apply_block_effects()`, `blocks_proposed` and `transactions_processed` are correctly guarded by `reward_already`. But:

```rust
val_info.last_active_slot = slot;   // harmless (idempotent)
val_info.update_reputation(true);    // NOT guarded — adds positive signal twice
```

When FIX-FORK-2 causes double processing, `update_reputation(true)` is called twice per block.

**Impact:** Validators that produce blocks during sync periods get inflated reputation scores. Minor but accumulates.

---

### F-06 — HIGH: 8+ P2P Message Types Have No Handler

**File:** P2P message handling in validator/src/main.rs block receiver

**Description:**
These message types are defined in the protocol but silently dropped by the receiver:
- `LivePong`, `HeartbeatResponse`, `RejectBlock`, `ListPeers`
- `CurrentHeight`, `SyncResponse`, `Pong`, `ListPeersResponse`

**Impact:** Network protocol incomplete. Peer discovery degraded. Missing block rejection could mask equivocation.

---

### F-07 — HIGH: Orphaned Blocks Never Re-Requested

**File:** Sync manager in validator/src/main.rs

**Description:**
When a block arrives whose parent hash doesn't match any known block, it's stored as "pending." But the missing parent block is never explicitly requested from peers. The sync system relies on periodic 5-second probes to eventually fill gaps.

**Impact:** During network partitions or high latency, blocks can remain pending indefinitely. Chain progression stalls until the gap is discovered by the next periodic sync.

---

### F-08 — HIGH: LICHEN_CONTRACTS_DIR Not Exported in Start Script

**File:** [lichen-start.sh](../../lichen-start.sh)

**Description:**
The `genesis_auto_deploy()` function resolves WASM contract paths using `LICHEN_CONTRACTS_DIR` environment variable. The start script does not export this variable, forcing the fallback search logic which looks in several locations.

This works on US (where contracts/ dir exists in the right relative path) but could fail silently on joiners if the directory structure differs.

**Impact:** Contract auto-deploy could fail silently on some nodes. Currently masked because contracts are deployed during genesis processing which finds them via fallback paths.

---

### F-09 — HIGH: Sync Bootstrap Grant Replication Is Timing-Dependent

**File:** [validator/src/main.rs](../../validator/src/main.rs) lines 7978–8009

**Description:**
During block sync, when a joining node encounters a block from a validator not yet in the stake pool, it replicates the bootstrap grant (treasury → validator) via direct state writes. This depends on:
- Whether the validator's pool entry is missing at the time the block is processed
- The order in which blocks are received and processed

Different join timing can produce different intermediate states.

**Impact:** Minor state inconsistency during sync that should self-correct once all blocks are processed. But could cause temporary divergence in treasury balance.

---

### F-10 — MEDIUM: Fork Adoption Has Crash-Unsafe Revert+Apply Window

**File:** [validator/src/main.rs](../../validator/src/main.rs) lines 8616–8697

**Description:**
When fork choice replaces a block:
1. Revert old block effects
2. Replay new block transactions
3. Write new block to disk (`put_block_atomic`)
4. Apply new block effects

Crash between steps 1 and 3: old block reverted but new block not on disk.
Crash between steps 3 and 4: new block on disk but effects not applied.

**Impact:** State corruption on crash during fork adoption. Rare but unrecoverable.

---

### F-11 — MEDIUM: WebSocket Lacks Backpressure

**File:** RPC WebSocket implementation

**Description:**
WebSocket subscriptions use unbounded channels. A slow client that can't keep up with block production rate will cause the server to buffer messages indefinitely.

**Impact:** Memory growth proportional to (block_rate × slow_client_count × message_size). Could cause OOM under load.

---

### F-12 — MEDIUM: Database Paths Inconsistent Across Environments

**Files:** Various config files, scripts, and source code

**Description:**
Multiple path conventions coexist:
- Source code: `./data/state-{testnet,mainnet}/`
- Deploy scripts: `/var/lib/lichen/state-*/`
- Docker: `/data/state`
- Actual VPS: `~/lichen/data/state-testnet/`

**Impact:** Configuration confusion. New deployments could create state in wrong location.

---

### F-13 — MEDIUM: Legacy Admin RPC Endpoints Bypass Consensus

**File:** RPC handler

**Description:**
Admin methods like `adminSetOraclePrice` modify state directly without going through the transaction/block pipeline.

**Impact:** State modified on one node only. Other nodes don't see the change. Useful for testing but dangerous in production.

---

### F-14 — MEDIUM: Orderbook Reconstruction Is O(n) Per Request

**File:** RPC orderbook handler

**Description:**
Each `getOrderbook` RPC call reconstructs the book from all open orders. No in-memory cache.

**Impact:** Slow responses under high order counts. DEX frontend polling every few seconds amplifies the cost.

---

### F-15 through F-18 — LOW

- **F-15**: Double replay warnings pollute logs, making real issues harder to spot
- **F-16**: `flush_metrics()` only saves internal counters, not account state — misleading name
- **F-17**: Peer reputation scoring exists but always returns 100 (never enforced)
- **F-18**: Oracle band was 5% (market) / 10% (limit) — too narrow for volatile pairs. **Already fixed** in current session (widened to 10%/50%)

---

## What's Working Correctly

These subsystems passed audit with no significant issues:

1. **requestAirdrop / Faucet** — Proper transaction pipeline (type 19 instruction → mempool → block → replay)
2. **Fee distribution** — Idempotency guard checks fee_distribution_hash BEFORE distributing. Correct.
3. **Epoch rewards** — Deterministic calculation, guard hash prevents double-crediting. Correct.
4. **RPC server architecture** — Single shared StateStore, no stale instances. Correct.
5. **Contract execution** — WASM VM, StateBatch overlay, opcode dispatch. All correct.
6. **Transaction serialization** — Bincode → base64 roundtrip. Correct.
7. **Ed25519 signing** — Standard implementation. Correct.
8. **Fork choice rule** — Weight-based + longest-chain fallback. Sound.
9. **Slot advancement** — Automatic via put_block_atomic. Gap-filling works via sync.
10. **Block atomicity** — put_block stores block + indexes in single WriteBatch. Correct.

---

## Root Cause Chain (Why Frontends Show Empty Data)

```
1. Genesis producer auto-funds deployer with 10K LICN (direct state write)
   ↓
2. Joining nodes DON'T replicate this auto-fund
   ↓
3. Genesis wallet has 0 LICN on EU/SEA
   ↓
4. All transactions FROM genesis wallet fail ("Insufficient spendable balance: 0 < 1000000")
   ↓
5. lUSD mint at slot 67 fails → no lUSD tokens on EU/SEA
   ↓
6. DEX orders from later slots fail → no orderbook entries on EU/SEA
   ↓
7. Cloudflare routes to EU (nearest to user) → frontends see empty state
   ↓
8. Explorer, DEX, wallet all show nothing
```

---

## Audit Methodology

- **Genesis/Sync path**: Line-by-line comparison of genesis/src/main.rs (producer) vs validator/src/main.rs (joiner)
- **P2P block propagation**: Trace of all message types, channel architecture, dedup mechanism
- **RPC/WS endpoints**: 70+ methods checked, WebSocket subscription model reviewed
- **Contract execution**: WASM VM dispatch, StateBatch overlay, named-export vs opcode patterns
- **Faucet/Airdrop**: Full transaction lifecycle traced from HTTP request to state update
- **Validator main loop**: Block receiver select!, apply_block_effects idempotency, state persistence
- **State persistence**: RocksDB WriteBatch usage, multi-write windows, crash scenarios
- **Live verification**: EU/SEA logs analyzed, account balances checked via RPC, transaction replay traced

---

*Audit conducted 2026-03-26. All line numbers reference current HEAD.*
