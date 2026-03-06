# MoltChain Sync & Network Performance Plan

> **Status**: Planning  
> **Created**: 2026-03-06  
> **Scope**: P2P sync, block propagation, RPC/WS performance, database tuning  
> **Goal**: Handle millions of blocks with fast validator catch-up and high-throughput networking

---

## Table of Contents

1. [Current Architecture](#current-architecture)
2. [P0 — Quick Wins (Constant Changes)](#p0--quick-wins)
3. [P1 — Structural Improvements](#p1--structural-improvements)
4. [P2 — Advanced Optimizations](#p2--advanced-optimizations)
5. [P3 — Future Scale (100+ Validators)](#p3--future-scale)
6. [Progress Tracking](#progress-tracking)

---

## Current Architecture

### System Overview

| Layer | Implementation | Key Numbers | Source |
|-------|---------------|-------------|--------|
| **P2P Transport** | QUIC (quinn) with mTLS | 20 peers max (validator), 30s idle timeout | `p2p/src/peer.rs:188-205` |
| **Gossip** | 10s interval, 50 peers/round | Exponential backoff 5s → 5min | `p2p/src/gossip.rs:170-245` |
| **Block Sync** | 500-block batches, 100-block chunks | 3-peer fanout, 10s cooldown | `validator/src/sync.rs:13-18` |
| **Block Propagation** | Push (fire-and-forget broadcast) | Pre-serialized, parallel sends | `p2p/src/peer.rs:491-530` |
| **DB** | RocksDB, 30+ column families | 512MB LRU cache, LZ4/Zstd | `core/src/state.rs:650-800` |
| **RPC** | Axum HTTP/1.1 | 8,192 concurrent, tiered rate limits | `rpc/src/lib.rs:1654-1755` |
| **WebSocket** | Axum WS | 500 global, 10/IP, 4,096 event buffer | `rpc/src/ws.rs:397-430` |
| **Serialization** | Bincode (P2P), JSON (RPC) | 16MB P2P message limit | `p2p/src/message.rs:1-245` |
| **Checkpoints** | RocksDB hardlinks every 10K blocks | Keeps 3 most recent | `validator/src/sync.rs:23-25` |
| **Sync Validation** | Full block re-execution | Parallel TX processing | `validator/src/main.rs:2276-2295` |

### Block Sync Flow (Current)

```
New validator joins:
  1. Connect to bootstrap peers via QUIC
  2. Discover highest_seen_slot from gossip (10s interval)
  3. If checkpoint exists → start from last checkpoint (skip to block N)
  4. Request blocks in SYNC_BATCH_SIZE=500 range
  5. Chunk into P2P_BLOCK_RANGE_LIMIT=100 per request
  6. Send to top-3 peers (SYNC_REQUEST_FANOUT=3) in parallel
  7. Receive blocks individually (1 block per message — NAT-friendly)
  8. Full TX re-execution per block (signatures, state transitions)
  9. 10s cooldown between sync triggers
  10. Repeat until caught up
```

### Block Propagation Flow (Current)

```
Leader produces block:
  1. Block stored locally
  2. Serialize once to bincode (Arc<Vec<u8>>)
  3. tokio::spawn parallel sends to all connected peers
  4. Each peer: open QUIC unidirectional stream → write → close
  5. ~50ms for 500 peers (parallel vs 2.5s sequential)
```

### RPC Architecture (Current)

```
Client request → Axum HTTP/1.1 → ConcurrencyLimit(8192)
  → Tiered rate limiter (cheap/moderate/expensive)
  → JSON-RPC dispatch → RocksDB read (bloom + LRU cache)
  → serde_json serialize → HTTP response
```

### WebSocket Architecture (Current)

```
Client connect → Axum WS upgrade → Per-IP limit check (10/IP, 500 global)
  → Spawn 3 tasks per connection:
    1. send_task: queued notifications + 15s ping
    2. event_task: broadcast channel → filter subscriptions → queue  
    3. recv_task: client messages (subscribe/unsubscribe)
  → Event broadcast channels: 4096 (standard), 2048 (DEX), 1024 (prediction)
```

### Database Architecture (Current — RocksDB)

| CF Type | Write Buffer | Compression | Block Size | Use |
|---------|-------------|-------------|------------|-----|
| Point-Lookup | 64 MB, 3 buffers | LZ4 | 16 KB | accounts, txs, blocks |
| Prefix-Scan | 32 MB | LZ4 | 16 KB | account_txs, nft_by_* |
| Write-Heavy | 128 MB, 4 buffers | LZ4 | 128 MB SSTs | events, token_transfers |
| Small/Singleton | 4 MB | LZ4 | 16 KB | stats, validators |
| Archival | 32 MB | Zstd | 32 KB | old data, higher compression |

**Global**: `max_open_files: 4096`, `max_total_wal_size: 256MB`, `parallelism: min(cpus, 8)`, `max_background_jobs: 4`  
**Shared block cache**: 512MB LRU (all CFs share)  
**Bloom filters**: 10 bits/key on point-lookup CFs

### Sync State Machine (Current)

| State | Logic | Source |
|-------|-------|--------|
| **Detect** | Compare `highest_seen_slot` vs `current_slot` | `validator/src/sync.rs:195-240` |
| **Request** | 500-block ranges, chunked to 100/request | `validator/src/sync.rs:16-19` |
| **Queue** | `pending_blocks` HashMap (max 500) | `validator/src/sync.rs:80-110` |
| **Apply** | `try_apply_pending()` — lowest-slot first, follow parent chain | `validator/src/sync.rs:340-390` |
| **Complete** | `is_syncing = false`, reset batch | `validator/src/sync.rs:280-290` |
| **Cooldown** | 10s between sync triggers | `validator/src/sync.rs:185-192` |

### Rate Limiting (Current)

**P2P Block Serving** (`validator/src/main.rs:7090-7115`):
- Token bucket per peer: burst=1200 blocks, refill=200 blocks/sec

**RPC Tiered Model** (`rpc/src/lib.rs:700-900`):
- **Cheap** (getBalance, getSlot): 100% of global cap
- **Moderate** (getBlock, getTransactionsByAddress): 40% of global cap
- **Expensive** (sendTransaction, deployContract): 10% of global cap (min 50 req/s)
- Implementation: `std::sync::Mutex<HashMap<IpAddr, (u64, Instant)>>`
- Stale entry pruning: every 30s

### Caching Layers (Current)

| Cache | Size/TTL | Purpose | Source |
|-------|----------|---------|--------|
| RocksDB block cache | 512MB LRU | SST block reads | `core/src/state.rs:690` |
| Blockhash cache | ~300 entries | TX validation (avoid 300 DB reads) | `core/src/state.rs:6208-6233` |
| Solana TX cache | LRU 10,000 | Hash → SolanaTxRecord | RPC layer |
| Program list cache | 512 entries, 1s TTL | Contract listings | RPC layer |
| Validator cache | 400ms TTL | Validator set queries | RPC layer |
| Metrics cache | 400ms TTL | getMetrics responses | RPC layer |
| DEX orderbook cache | Per-pair, 1s TTL | Aggregated order views | RPC layer |

### Metrics: Incremental (No Scans)

Metrics are **NOT** computed via DB scans. They are atomic in-memory counters (`MetricsStore`) updated per-block and persisted once per slot:

```
MetricsStore {
    total_accounts: Mutex<u64>,    // Incremented on new account creation
    active_accounts: Mutex<u64>,   // Balance > 0
    total_transactions: Mutex<u64>,
    total_blocks: Mutex<u64>,
    daily_transactions: Mutex<u64>,  // Resets at UTC midnight
    peak_tps: Mutex<f64>,
}
```

Source: `core/src/state.rs:118-156`, `core/src/state.rs:1348-1356`

---

## P0 — Quick Wins

Constant/config changes only. Minimal code, maximum impact. Each is a single-line change.

### P0-1: Reduce Checkpoint Interval (10K → 1K blocks)

**Current**: `CHECKPOINT_INTERVAL: u64 = 10000` — checkpoints every ~4 hours  
**Proposed**: `CHECKPOINT_INTERVAL: u64 = 1000` — checkpoints every ~400 seconds (~7 min)  
**Source**: `validator/src/sync.rs:23`

**Why**: RocksDB checkpoints are O(1) hardlinks — essentially free. No data is copied. More frequent checkpoints means joining validators replay at most 1K blocks instead of 10K. At 400ms/slot with ~2 txs/block, 1K blocks replay in seconds vs minutes for 10K.

**Risk**: None. Disk usage is unchanged (hardlinks share SST files). Pruning keeps only 3 most recent checkpoints.

- [ ] Change constant
- [ ] Verify checkpoint creation logs
- [ ] Test validator catch-up from checkpoint

---

### P0-2: Reduce Sync Cooldown (10s → 2s)

**Current**: 10s minimum between sync triggers  
**Proposed**: 2s with exponential backoff on failure (2s → 4s → 8s → max 30s)  
**Source**: `validator/src/sync.rs:185-192`

**Why**: At 1M blocks behind, the 10s cooldown alone adds ~5.5 hours of idle wait time between sync batches (1M blocks / 500 per batch = 2000 batches × 10s = 20,000s ≈ 5.5 hours of just waiting). Reducing to 2s cuts this to ~1.1 hours. With exponential backoff on failure, we avoid flooding peers if sync stalls.

**Risk**: Minimal. Peers have their own rate limiting (token bucket: 1200 burst, 200/sec refill). The cooldown is client-side throttling.

- [ ] Change cooldown constant  
- [ ] Add exponential backoff on consecutive failures
- [ ] Test under high sync load

---

### P0-3: Increase Batch Sizes (500→2000, 100→500)

**Current**: `SYNC_BATCH_SIZE: 500`, `P2P_BLOCK_RANGE_LIMIT: 100`  
**Proposed**: `SYNC_BATCH_SIZE: 2000`, `P2P_BLOCK_RANGE_LIMIT: 500`  
**Source**: `validator/src/sync.rs:13-18`

**Why**: Each sync batch triggers at most `SYNC_BATCH_SIZE / P2P_BLOCK_RANGE_LIMIT` requests. Current: 5 chunk requests per batch. Proposed: 4 chunk requests per batch but each carrying 5× more blocks. QUIC handles congestion control natively — no need for conservative chunks.

**Risk**: Low. P2P messages are already capped at 16MB. 500 empty blocks ≈ 0.5MB serialized. 500 blocks with ~5 txs each ≈ 5-10MB. Well within limits. The `pending_blocks` HashMap cap should increase from 500 to 2000 to match.

- [ ] Increase `SYNC_BATCH_SIZE` to 2000
- [ ] Increase `P2P_BLOCK_RANGE_LIMIT` to 500
- [ ] Increase `pending_blocks` cap to 2000
- [ ] Increase `requested_slots` cap proportionally
- [ ] Test sync with large block ranges

---

### P0-4: Increase Serving Rate Limit

**Current**: Token bucket burst=1200, refill=200/sec per peer  
**Proposed**: burst=5000, refill=1000/sec per peer  
**Source**: `validator/src/main.rs:7090-7115`

**Why**: With P0-3 increasing request sizes up to 500 blocks, the serving rate limit needs to accommodate larger responses. At 200 blocks/sec, a 500-block request takes 2.5s just at the rate limiter.

**Risk**: Low. The refill rate caps sustained load. Burst allows initial large sync requests. Per-peer, so one greedy peer can't starve others.

- [ ] Increase burst and refill constants
- [ ] Verify under multi-peer sync

---

## P1 — Structural Improvements

Architecture changes with significant sync/throughput impact.

### P1-1: Header-First Sync (Skip TX Re-Execution)

**Current**: Every synced block is fully re-executed — all TX signatures verified, all state transitions replayed.  
**Proposed**: During catch-up, validate block headers only (producer signature, parent hash, slot sequence). Trust PoS finality: if 2/3+ of stake signed the block, state transitions are valid. Only full-execute the last N blocks (e.g., 100) for local state verification.  
**Source**: `validator/src/main.rs:2276-2295` (replay_block_transactions)

**Why**: TX re-execution is the CPU bottleneck during sync. A block with 100 txs requires 100 signature verifications + 100 state transitions. With header-only sync, verification is O(1) per block (just the producer sig). This could be 10-100× faster for TX-heavy blocks.

**Design**:
```
Sync mode:
  blocks 0..N-100:    Header validation only (sig, parent_hash, slot)
  blocks N-100..N:    Full TX re-execution (verify final state)
```

**State concern**: If we skip TX execution, how do we build account state? Two approaches:
1. **Apply only header + state root**: Trust peers' state without replaying. Final 100 blocks verify the chain tip matches.
2. **Paired with state snapshot transfer (P2-1)**: Download state snapshot, then header-sync blocks since snapshot, then full-exec the last 100.

**Risk**: Medium. Requires careful handling of the trust boundary. A malicious supermajority could produce invalid state transitions. Mitigated by the final full-execution window.

- [ ] Add `SyncMode::HeaderOnly` vs `SyncMode::Full`
- [ ] Modify block validation path for header-only mode
- [ ] Define the full-execution window size
- [ ] Add state root verification at window boundary
- [ ] Test catch-up performance comparison

---

### P1-2: Adaptive Block Batching for Sync

**Current**: Blocks sent individually in `BlockRangeResponse` (1 block per message) to avoid NAT/fragmentation issues. See `validator/src/main.rs:7153-7170`.  
**Proposed**: Detect if peer is syncing vs live. For syncing peers, batch 10-50 blocks per message. For live peers behind NAT, keep individual sends. Use the `is_syncing` flag from the request.  
**Source**: `validator/src/main.rs:7153-7170`

**Why**: Individual block sends add per-message overhead: serialization framing, QUIC stream setup, message parsing. At 500 blocks, that's 500 stream opens. Batching reduces this to 10-50 stream opens.

**Design**:
```
BlockRangeResponse handling:
  if request.is_large_range (>100 blocks):
    batch = 50 blocks per message
  else:
    batch = 1 block per message (NAT-safe)
```

**Risk**: Low. Only affects the serving side. Receiving peer already handles multi-block messages. Just need to respect the 16MB P2P message limit.

- [ ] Add `batch_size` field to `BlockRangeRequest`
- [ ] Modify serving loop to batch responses  
- [ ] Add size check before batching (stay under 16MB)
- [ ] Test with home NAT validators

---

### P1-3: Configurable RocksDB Cache Size

**Current**: 512MB fixed LRU cache (`Cache::new_lru_cache(512 * 1024 * 1024)`)  
**Proposed**: CLI flag `--cache-size-mb` (default 512). Auto-detect if not specified: use 25% of available RAM up to 4GB.  
**Source**: `core/src/state.rs:690`

**Why**: At 10M+ accounts, 512MB isn't enough to keep hot data in cache. Cache misses on RocksDB mean disk I/O which is orders of magnitude slower. Operators with 32GB RAM should be able to use 4-8GB for cache.

**Implementation**:
```rust
let cache_mb = config.cache_size_mb.unwrap_or_else(|| {
    let total_mb = sys_info::mem_info().ok().map(|m| m.total / 1024).unwrap_or(4096);
    (total_mb / 4).min(4096).max(256)
});
```

**Risk**: None. Cache is a pure read optimization.

- [ ] Add `--cache-size-mb` CLI flag
- [ ] Pass through to StateStore constructor
- [ ] Add auto-detection fallback 
- [ ] Log cache size on startup

---

### P1-4: HTTP/2 + Response Compression for RPC

**Current**: HTTP/1.1 via Axum. No response compression.  
**Proposed**: Enable HTTP/2 + gzip/brotli response compression.  
**Source**: `rpc/src/lib.rs:1654-1755`

**Why**: HTTP/2 multiplexes multiple requests over a single TCP connection (no head-of-line blocking). Header compression (HPACK) reduces overhead. Response compression (gzip/br) shrinks JSON payloads 5-10×. Both are low-effort changes with Axum's tower middleware.

**Implementation**:
```rust
use tower_http::compression::CompressionLayer;

let app = Router::new()
    // ... routes ...
    .layer(CompressionLayer::new())
    .layer(ConcurrencyLimitLayer::new(8192));
```

HTTP/2 is automatic with Axum when clients negotiate it (no code change needed for axum with `http2` feature).

**Risk**: None. HTTP/2 is backward-compatible (clients that don't support it fall back to HTTP/1.1). Compression adds minor CPU overhead but the bandwidth savings dominate.

- [ ] Add `tower-http` compression feature to Cargo.toml
- [ ] Add `CompressionLayer` to RPC router
- [ ] Verify HTTP/2 negotiation works
- [ ] Benchmark RPC throughput before/after

---

### P1-5: Rate Limiter Upgrade (Mutex → DashMap)

**Current**: `std::sync::Mutex<HashMap<IpAddr, (u64, Instant)>>` for per-IP rate limiting  
**Proposed**: Replace with `DashMap` for lock-free concurrent access  
**Source**: `rpc/src/lib.rs:700-900`

**Why**: The mutex serializes all RPC requests briefly on every call. At 10K req/s, contention becomes measurable. DashMap provides concurrent read/write with shard-level locking (typically 16+ shards), effectively eliminating contention.

**Risk**: None. DashMap is a drop-in replacement with the same API patterns.

- [ ] Replace `Mutex<HashMap>` with `DashMap`
- [ ] Update pruning logic for DashMap iteration
- [ ] Benchmark under load

---

## P2 — Advanced Optimizations

Larger engineering efforts with high impact at scale.

### P2-1: State Snapshot Transfer

**Current**: Joining validators replay all blocks from the last checkpoint. Checkpoints are local only — not transferable.  
**Proposed**: Allow validators to request and download the latest state snapshot (RocksDB SST files) from peers over QUIC. Validator restores snapshot locally, then only replays blocks since that snapshot.

**Why**: This turns hours of catch-up into minutes of file transfer. A full state at 10M accounts is roughly 2-5GB (RocksDB with LZ4 compression). At 100 Mbps, that's 3-7 minutes vs hours of block replay.

**Design**:
```
New validator:
  1. Connect to peers
  2. Request latest snapshot metadata (slot, state_root, size)
  3. Verify state_root matches chain at that slot (from peers' headers)
  4. Download SST files in chunks over QUIC bidirectional streams
  5. Restore RocksDB from downloaded checkpoint
  6. Resume normal sync from snapshot slot
```

**New P2P messages**:
- `SnapshotMetadataRequest` → `SnapshotMetadataResponse { slot, state_root, total_bytes, file_list }`
- `SnapshotChunkRequest { file_name, offset, length }` → `SnapshotChunkResponse { data }`

**Security**: State root from the finalized block at the snapshot slot is verified against multiple peers. File integrity via chunk hashing.

- [ ] Design snapshot transfer protocol
- [ ] Implement snapshot serving (checkpoint → SST list → chunk serving)
- [ ] Implement snapshot downloading with integrity verification
- [ ] Handle checkpoint rotation during transfer
- [ ] Test with various state sizes

---

### P2-2: P2P Message Compression (LZ4)

**Current**: Blocks are bincode-serialized and sent raw over QUIC.  
**Proposed**: Add LZ4 frame compression to P2P messages above a size threshold (e.g., >1KB). Blocks with transactions compress 3-5× typically.  
**Source**: `p2p/src/peer.rs:491-530`

**Why**: Less bandwidth = faster sync + lower network costs. LZ4 compression is ~2GB/s on modern CPUs — negligible latency. For sync, where you're downloading millions of blocks, 3-5× bandwidth savings is massive.

**Design**:
```
Message envelope:
  [ protocol_version: u8 ][ compressed: bool ][ payload_len: u32 ][ payload ]
  
If compressed=true:
  payload = lz4_frame::decompress(raw)
Else:
  payload = raw
```

**Risk**: Low. LZ4 is already used for RocksDB compression, so the library is already linked. Just need to add it to the P2P layer.

- [ ] Add message compression flag to P2P envelope
- [ ] Compress on send for messages > threshold
- [ ] Decompress on receive
- [ ] Benchmark bandwidth savings

---

### P2-3: Separate Hot/Cold Storage

**Current**: All data shares the same RocksDB instance with CF-level tuning.  
**Proposed**: After N slots (configurable, e.g., 1M), automatically migrate old blocks and dormant accounts to a separate archival DB with Zstd compression and larger block sizes.

**Why**: Hot data (recent blocks, active accounts) benefits from fast SSDs and aggressive caching. Cold data (old blocks, dormant accounts) benefits from better compression ratios. Splitting reduces compaction pressure on the hot DB.

**Risk**: Medium. Requires careful query routing — some queries span hot+cold (e.g., historical TX lookups).

- [ ] Design hot/cold boundary (slot-based vs access-based)
- [ ] Implement background migration worker
- [ ] Route reads across hot/cold DBs
- [ ] Add monitoring for migration progress

---

### P2-4: Binary RPC Format Option

**Current**: All RPC responses are JSON (serde_json).  
**Proposed**: Offer bincode or MessagePack as alternative response formats via `Accept` header or query parameter. JSON remains default for browser clients.

**Why**: JSON serialization of large blocks is CPU-intensive and produces 5-10× larger payloads than binary. SDK clients (Rust, Python, JS) can use binary natively. For high-frequency trading bots hitting the RPC, binary format is a significant latency reduction.

**Design**:
```
Accept: application/octet-stream → bincode response
Accept: application/msgpack → MessagePack response
Accept: application/json (default) → JSON response
```

**Risk**: Low. JSON remains default. Binary is opt-in for SDK clients.

- [ ] Add content-type negotiation middleware
- [ ] Implement bincode/msgpack serializers for RPC responses
- [ ] Update SDK clients to use binary format
- [ ] Benchmark latency comparison

---

### P2-5: Parallel Block Download Pipeline

**Current**: Sync requests chunks sequentially — download chunk → apply → download next.  
**Proposed**: Pipeline downloads — while applying chunk N, concurrently download chunks N+1 and N+2.

**Why**: Network latency and block application are independent operations. Pipelining overlaps them, effectively doubling throughput.

**Design**:
```
Pipeline depth = 3:
  Task 1: Download chunk N+2 from peer A
  Task 2: Download chunk N+1 from peer B (in buffer)
  Task 3: Apply chunk N (from previous download)
```

**Risk**: Low. Requires a small buffer for pre-downloaded chunks. Memory bounded by pipeline depth × chunk size.

- [ ] Implement download pipeline with configurable depth
- [ ] Add download buffer with ordering
- [ ] Coordinate with pending_blocks HashMap
- [ ] Benchmark sync speed improvement

---

## P3 — Future Scale

For 100+ validators and 10M+ blocks. Long-term roadmap.

### P3-1: Warp Sync (Snap Sync)

Like Ethereum's snap sync: download the latest state trie directly, verify it against the state root in the latest finalized block header. Zero block replay. The ultimate catch-up solution.

**Requires**: Merkleized state trie (partially exists — state roots are computed). Full implementation means the trie structure must be reconstructible from downloads.

- [ ] Design state trie download protocol
- [ ] Implement state trie verification
- [ ] Handle trie updates during download

---

### P3-2: Structured Overlay (Kademlia DHT)

Replace flat gossip with Kademlia DHT for O(log N) peer routing. Each peer maintains K-buckets with log(N) buckets. Block and TX propagation goes from O(N) to O(log N) hops.

**Why needed**: At 100+ validators, flat gossip (broadcast to all 20 peers) creates redundant traffic. Kademlia ensures efficient routing with minimal redundancy.

- [ ] Implement Kademlia routing table
- [ ] Replace gossip broadcast with DHT-based routing for blocks
- [ ] Maintain flat broadcast for votes (latency critical)

---

### P3-3: Turbo Block Propagation (Compact Blocks)

Like Bitcoin Compact Blocks: instead of sending full blocks, send header + short TX IDs. Receiving peer reconstructs the block from its mempool. Only request missing TXs individually. 90%+ bandwidth savings for live block propagation.

**Why**: As block sizes grow with more TXs, full block propagation becomes bandwidth-heavy. Most TXs are already in the receiver's mempool from prior gossip.

- [ ] Implement short TX ID scheme
- [ ] Add mempool-based block reconstruction
- [ ] Fallback to full block for high miss rate

---

### P3-4: Erasure Coding for Block Download

Split blocks into K data + M parity shards (Reed-Solomon). Peers only need any K shards from any source to reconstruct. Massively parallelizes block download — get shards from multiple peers simultaneously.

- [ ] Implement Reed-Solomon encoder/decoder
- [ ] Add shard-based block requests
- [ ] Coordinate shard distribution across peers

---

### P3-5: Validator-Tier Peering

Distinguish between validator peers (high-priority, full mesh) and observer nodes. Validators maintain direct connections to all other validators for instant vote/block propagation. Observers connect through relay nodes.

**Why**: Vote latency directly affects finality time. Validators need the fastest path. Observers can tolerate slightly higher latency.

- [ ] Implement peer role classification
- [ ] Maintain validator peer mesh
- [ ] Route votes through validator mesh only

---

### P3-6: NAT Traversal (QUIC Hole Punching)

Enable QUIC NAT traversal so home validators behind NAT can accept inbound connections without port forwarding. Uses QUIC's connection migration and the relay infrastructure.

- [ ] Implement QUIC NAT traversal
- [ ] Add relay-assisted hole punching
- [ ] Test with various NAT types

---

## Network Security & Resilience

Cross-cutting concerns that apply at all priority levels.

### Eclipse Attack Resistance

Ensure peer selection is diverse by IP subnet. Don't let one ASN/subnet dominate the peer table. At minimum, limit peers from the same /24 prefix to 2.

- [ ] Add IP diversity check in peer selection
- [ ] Limit peers per /24 subnet

### Peer Scoring Refinement

Add latency-based scoring — prefer peers that respond to block requests fastest. Current scoring is basic (reputation i64). Add response time tracking and factor it into peer selection for sync requests.

- [ ] Track per-peer response latency (rolling average)
- [ ] Factor latency into sync peer selection
- [ ] Deprioritize high-latency peers during fast sync

### Bandwidth Metering

Track bytes/sec per peer. Detect and throttle peers that consume disproportionate bandwidth (leeching without contributing blocks/votes).

- [ ] Add per-peer bandwidth tracking
- [ ] Implement throttling for high-bandwidth consumers
- [ ] Log bandwidth stats for monitoring

---

## Progress Tracking

| ID | Item | Priority | Status | Notes |
|----|------|----------|--------|-------|
| P0-1 | Reduce checkpoint interval | P0 | Not Started | `sync.rs:23` — 10000 → 1000 |
| P0-2 | Reduce sync cooldown | P0 | Not Started | `sync.rs:185` — 10s → 2s + backoff |
| P0-3 | Increase batch sizes | P0 | Not Started | `sync.rs:13-18` — 500/100 → 2000/500 |
| P0-4 | Increase serving rate limit | P0 | Not Started | `main.rs:7090` — burst 5000, refill 1000 |
| P1-1 | Header-first sync | P1 | Not Started | Skip TX re-exec during catch-up |
| P1-2 | Adaptive block batching | P1 | Not Started | 50 blocks/msg for sync peers |
| P1-3 | Configurable cache size | P1 | Not Started | `--cache-size-mb` flag |
| P1-4 | HTTP/2 + compression | P1 | Not Started | Axum + tower-http |
| P1-5 | Rate limiter DashMap | P1 | Not Started | Mutex → DashMap |
| P2-1 | State snapshot transfer | P2 | Not Started | Download RocksDB checkpoint |
| P2-2 | P2P message compression | P2 | Not Started | LZ4 frame compression |
| P2-3 | Hot/cold storage split | P2 | Not Started | Auto-migrate old data |
| P2-4 | Binary RPC format | P2 | Not Started | bincode/msgpack option |
| P2-5 | Parallel download pipeline | P2 | Not Started | Overlap download + apply |
| P3-1 | Warp sync | P3 | Not Started | State trie download |
| P3-2 | Kademlia DHT | P3 | Not Started | O(log N) routing |
| P3-3 | Compact blocks | P3 | Not Started | TX-ID-based propagation |
| P3-4 | Erasure coding | P3 | Not Started | Reed-Solomon sharding |
| P3-5 | Validator-tier peering | P3 | Not Started | Full mesh for validators |
| P3-6 | NAT traversal | P3 | Not Started | QUIC hole punching |
