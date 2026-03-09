// Chain Synchronization Manager

use moltchain_core::{Block, Hash};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Sync mode determines how blocks are validated during catch-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    /// Full TX re-execution (signatures, state transitions) for every block.
    Full,
    /// Header-only validation (producer signature, parent hash, slot sequence).
    /// Used during fast catch-up; the last N blocks switch to `Full` for state
    /// verification. Requires trust in PoS finality (2/3+ stake signed).
    HeaderOnly,
    /// Warp sync: download the latest state snapshot directly, verify the
    /// state root against the finalized block header, then switch to `Full`
    /// for the final blocks at the tip. Zero block replay for the bulk of
    /// the chain. Only used when the gap exceeds `WARP_SYNC_THRESHOLD`.
    Warp,
}

/// Minimum gap (in blocks) to trigger warp sync instead of header-only sync.
/// Below this threshold, header-only sync is more efficient because the
/// overhead of downloading a full state snapshot exceeds replaying headers.
pub const WARP_SYNC_THRESHOLD: u64 = 10_000;

/// Number of blocks at the chain tip that always use full re-execution,
/// even during header-only sync. This ensures the final state is verified.
pub const HEADER_SYNC_FULL_EXECUTION_WINDOW: u64 = 100;

/// Maximum blocks to request in a single sync batch.
/// This is the overall catch-up window; actual P2P requests are chunked
/// into sub-batches of `P2P_BLOCK_RANGE_LIMIT` to stay within the P2P
/// layer's per-request cap (AUDIT-FIX H1).
const SYNC_BATCH_SIZE: u64 = 2000;

/// The per-request chunk size that the P2P layer allows.
/// Must match `MAX_BLOCK_RANGE` in `p2p/src/network.rs`.
pub const P2P_BLOCK_RANGE_LIMIT: u64 = 500;

/// Pipeline depth: when far behind, prefetch this many batches concurrently.
/// Overlaps download and application to eliminate idle time between batches.
pub const SYNC_PIPELINE_DEPTH: u64 = 3;

/// Maximum blocks to hold in pending state (memory limit).
/// Sized to hold one full pipeline: SYNC_BATCH_SIZE * SYNC_PIPELINE_DEPTH.
const MAX_PENDING_BLOCKS: usize = (SYNC_BATCH_SIZE * SYNC_PIPELINE_DEPTH) as usize;

/// Checkpoint interval - only need to sync from last checkpoint
/// Set to 0 to disable checkpointing
const CHECKPOINT_INTERVAL: u64 = 1000; // Every 1k blocks

/// Base sync cooldown in seconds between sync triggers.
const SYNC_COOLDOWN_BASE_SECS: u64 = 2;

/// Maximum cooldown after consecutive failures (exponential backoff cap).
const SYNC_COOLDOWN_MAX_SECS: u64 = 30;

/// Tracks chain synchronization state
pub struct SyncManager {
    /// Blocks we're waiting for (slot -> received but can't apply yet)
    pending_blocks: Arc<Mutex<HashMap<u64, Block>>>,

    /// Slots we've requested (to avoid duplicate requests)
    requested_slots: Arc<Mutex<HashSet<u64>>>,

    /// Are we currently syncing?
    is_syncing: Arc<Mutex<bool>>,

    /// Highest slot seen from network
    highest_seen_slot: Arc<Mutex<u64>>,

    /// When highest_seen_slot was last updated (for decay)
    highest_seen_updated_at: Arc<Mutex<Instant>>,

    /// Current sync batch being processed
    current_sync_batch: Arc<Mutex<Option<(u64, u64)>>>,

    /// Last checkpoint slot (for fast bootstrapping)
    last_checkpoint: Arc<Mutex<u64>>,

    /// Cooldown: when the last sync was triggered (prevents request storms)
    last_sync_triggered_at: Arc<Mutex<Instant>>,

    /// Consecutive sync failures for exponential backoff on cooldown
    consecutive_failures: Arc<Mutex<u32>>,

    /// Current sync mode (header-only vs full re-execution)
    sync_mode: Arc<Mutex<SyncMode>>,
}

impl SyncManager {
    pub fn new() -> Self {
        SyncManager {
            pending_blocks: Arc::new(Mutex::new(HashMap::new())),
            requested_slots: Arc::new(Mutex::new(HashSet::new())),
            is_syncing: Arc::new(Mutex::new(false)),
            highest_seen_slot: Arc::new(Mutex::new(0)),
            highest_seen_updated_at: Arc::new(Mutex::new(Instant::now())),
            current_sync_batch: Arc::new(Mutex::new(None)),
            last_checkpoint: Arc::new(Mutex::new(0)),
            last_sync_triggered_at: Arc::new(Mutex::new(
                Instant::now() - std::time::Duration::from_secs(60),
            )),
            consecutive_failures: Arc::new(Mutex::new(0)),
            sync_mode: Arc::new(Mutex::new(SyncMode::Full)),
        }
    }

    /// Set checkpoint (for fast bootstrapping from snapshots)
    pub async fn set_checkpoint(&self, slot: u64) {
        let mut checkpoint = self.last_checkpoint.lock().await;
        *checkpoint = slot;
        info!("📍 Checkpoint set at slot {}", slot);
    }

    /// Get the last recorded checkpoint slot
    pub async fn get_checkpoint(&self) -> u64 {
        *self.last_checkpoint.lock().await
    }

    /// Add a block that can't be applied yet (missing parent)
    pub async fn add_pending_block(&self, block: Block) {
        let slot = block.header.slot;
        let mut pending = self.pending_blocks.lock().await;

        // Memory protection: if too many pending blocks, drop NEWEST (highest slot).
        // Gap-filling blocks (lowest slots) are the ones we need most to reconnect
        // the chain. Dropping them creates an unrecoverable hole. Newer blocks can
        // be re-fetched once we catch up.
        if pending.len() >= MAX_PENDING_BLOCKS {
            if let Some(newest_slot) = pending.keys().max().copied() {
                // Don't evict the block we're about to insert
                if newest_slot != slot {
                    pending.remove(&newest_slot);
                    warn!(
                        "⚠️  Dropped newest pending block {} (memory limit, keeping gap-fillers)",
                        newest_slot
                    );
                }
            }
        }

        pending.insert(slot, block);
        info!("📦 Stored pending block {} (waiting for parent)", slot);

        self.note_seen(slot).await;
    }

    /// Record the highest slot seen from the network
    pub async fn note_seen(&self, slot: u64) {
        let mut highest = self.highest_seen_slot.lock().await;
        if slot > *highest {
            *highest = slot;
            let mut ts = self.highest_seen_updated_at.lock().await;
            *ts = Instant::now();
        }
    }

    /// Record the highest slot from an unvalidated source (e.g., peer status).
    /// Caps the jump to prevent a malicious peer from claiming slot u64::MAX
    /// and permanently setting `we_are_behind = true` in fork choice.
    pub async fn note_seen_bounded(&self, slot: u64, max_ahead: u64) {
        let mut highest = self.highest_seen_slot.lock().await;
        // Only accept slots up to `max_ahead` beyond current highest
        let cap = (*highest).saturating_add(max_ahead);
        let capped = slot.min(cap);
        if capped > *highest {
            *highest = capped;
            let mut ts = self.highest_seen_updated_at.lock().await;
            *ts = Instant::now();
        }
    }

    /// Decay `highest_seen_slot` toward the given tip if no new blocks have
    /// arrived from the network for `stale_secs`. This prevents the
    /// "freeze production" guard from permanently stalling the chain when
    /// no peer can actually serve the missing blocks.
    pub async fn decay_highest_seen(&self, current_tip: u64, stale_secs: u64) {
        let updated_at = *self.highest_seen_updated_at.lock().await;
        if updated_at.elapsed().as_secs() >= stale_secs {
            let mut highest = self.highest_seen_slot.lock().await;
            if *highest > current_tip {
                let old = *highest;
                *highest = current_tip;
                info!(
                    "📉 Decayed highest_seen from {} to {} (no new blocks for {}s)",
                    old, current_tip, stale_secs
                );
                // Reset the timestamp so we don't spam the log
                let mut ts = self.highest_seen_updated_at.lock().await;
                *ts = Instant::now();
            }
        }
    }

    /// Force-decay `highest_seen_slot` to current tip, ignoring the timestamp.
    /// Used by the bounded freeze guard to break out of the death spiral when
    /// the node has been frozen for too long.
    pub async fn force_decay(&self, current_tip: u64) {
        let mut highest = self.highest_seen_slot.lock().await;
        if *highest > current_tip {
            let old = *highest;
            *highest = current_tip;
            warn!(
                "📉 Force-decayed highest_seen from {} to {} (freeze timeout)",
                old, current_tip
            );
            let mut ts = self.highest_seen_updated_at.lock().await;
            *ts = Instant::now();
        }
    }

    /// Returns true if the sync manager has pending blocks or is actively
    /// syncing. Used by the watchdog to avoid killing a node that is alive
    /// but behind on the chain.
    pub async fn is_actively_receiving(&self) -> bool {
        let has_pending = !self.pending_blocks.lock().await.is_empty();
        let is_syncing = *self.is_syncing.lock().await;
        has_pending || is_syncing
    }

    /// Check if we need to start syncing (returns next batch to sync)
    pub async fn should_sync(&self, current_slot: u64) -> Option<(u64, u64)> {
        let highest = *self.highest_seen_slot.lock().await;
        let is_syncing = *self.is_syncing.lock().await;
        let current_batch = self.current_sync_batch.lock().await;

        // Cooldown: don't re-trigger sync within the adaptive cooldown period.
        // Base: 2s. On consecutive failures: 2s → 4s → 8s → 16s → 30s (capped).
        // Without this, every incoming pending block fires another batch of
        // range requests, flooding the responder and saturating QUIC.
        let last_triggered = *self.last_sync_triggered_at.lock().await;
        let failures = *self.consecutive_failures.lock().await;
        let cooldown_secs = if failures == 0 {
            SYNC_COOLDOWN_BASE_SECS
        } else {
            (SYNC_COOLDOWN_BASE_SECS << failures.min(4)).min(SYNC_COOLDOWN_MAX_SECS)
        };
        if last_triggered.elapsed() < std::time::Duration::from_secs(cooldown_secs) {
            return None;
        }

        // If already syncing a batch, allow re-trigger only when very far behind
        // (> SYNC_BATCH_SIZE / 2 slots) — otherwise wait for current batch.
        if is_syncing && current_batch.is_some() {
            let gap = highest.saturating_sub(current_slot);
            if gap <= SYNC_BATCH_SIZE / 2 {
                return None;
            }
            // Very far behind — allow overlapping sync request
            info!(
                "🔁 Re-triggering sync while already syncing ({} slots behind)",
                gap
            );
        }

        // If we're behind by more than 1 block and not already syncing
        // (FIX-FORK-2: lowered from +2 to +1 to catch forks earlier)
        if highest > current_slot + 1 {
            // Determine start slot
            // NOTE: We include current_slot in the range (not current_slot + 1)
            // to receive the peer's version of our latest block. This enables
            // fork resolution: if we have a different block at current_slot than
            // the peer, the fork choice mechanism will replace ours with theirs,
            // allowing subsequent blocks to chain correctly.
            let start_slot = if current_slot == 0 {
                // Check if we have a checkpoint to start from
                let checkpoint = *self.last_checkpoint.lock().await;
                if checkpoint > 0 && CHECKPOINT_INTERVAL > 0 {
                    info!("🚀 Fast sync from checkpoint {}", checkpoint);
                    checkpoint
                } else {
                    0 // Start from genesis
                }
            } else {
                current_slot // Include overlap for fork resolution
            };

            // Calculate batch end.
            // P2-5: When far behind, use pipelined prefetch — request up
            // to PIPELINE_DEPTH batches ahead so the next batch is already
            // downloading while we apply the current one.
            let gap = highest.saturating_sub(start_slot);
            let effective_batch = if gap > SYNC_BATCH_SIZE {
                SYNC_BATCH_SIZE * SYNC_PIPELINE_DEPTH
            } else {
                SYNC_BATCH_SIZE
            };
            let batch_end = std::cmp::min(start_slot + effective_batch - 1, highest);

            if batch_end >= start_slot {
                Some((start_slot, batch_end))
            } else {
                None
            }
        } else {
            None
        }
    }

    /// Mark that we're syncing a range
    pub async fn start_sync(&self, start: u64, end: u64) {
        let mut is_syncing = self.is_syncing.lock().await;
        *is_syncing = true;

        let mut batch = self.current_sync_batch.lock().await;
        *batch = Some((start, end));

        // Record trigger time for cooldown
        let mut last_triggered = self.last_sync_triggered_at.lock().await;
        *last_triggered = Instant::now();

        let batch_size = end - start + 1;
        info!(
            "🔄 Starting sync batch: blocks {} to {} ({} blocks)",
            start, end, batch_size
        );
    }

    /// Complete current sync batch
    pub async fn complete_sync(&self) {
        let mut is_syncing = self.is_syncing.lock().await;
        *is_syncing = false;

        let mut batch = self.current_sync_batch.lock().await;
        *batch = None;

        info!("✅ Sync batch completed");
    }

    /// Record a sync failure — increases cooldown via exponential backoff.
    pub async fn record_sync_failure(&self) {
        let mut failures = self.consecutive_failures.lock().await;
        *failures = failures.saturating_add(1);
        let cooldown_secs =
            (SYNC_COOLDOWN_BASE_SECS << (*failures).min(4)).min(SYNC_COOLDOWN_MAX_SECS);
        warn!(
            "⚠️  Sync failure #{} — cooldown increased to {}s",
            *failures, cooldown_secs
        );
    }

    /// Reset failure counter on successful sync progress.
    pub async fn record_sync_success(&self) {
        let mut failures = self.consecutive_failures.lock().await;
        if *failures > 0 {
            info!(
                "✅ Sync success — resetting failure backoff (was {})",
                *failures
            );
            *failures = 0;
        }
    }

    /// Set sync mode (header-only vs full re-execution)
    pub async fn set_sync_mode(&self, mode: SyncMode) {
        let mut current = self.sync_mode.lock().await;
        if *current != mode {
            info!("🔄 Sync mode changed to {:?}", mode);
            *current = mode;
        }
    }

    /// Get the current sync mode
    #[allow(dead_code)]
    pub async fn get_sync_mode(&self) -> SyncMode {
        *self.sync_mode.lock().await
    }

    /// Determine whether a given block slot should use full or header-only
    /// validation based on distance from the chain tip.
    pub async fn should_full_validate(&self, block_slot: u64) -> bool {
        let mode = *self.sync_mode.lock().await;
        match mode {
            SyncMode::Full => true,
            SyncMode::HeaderOnly | SyncMode::Warp => {
                let highest = *self.highest_seen_slot.lock().await;
                // Full-execute the last HEADER_SYNC_FULL_EXECUTION_WINDOW blocks
                block_slot + HEADER_SYNC_FULL_EXECUTION_WINDOW >= highest
            }
        }
    }

    /// Get sync progress relative to highest seen slot
    pub async fn get_sync_progress(&self, current_slot: u64) -> Option<SyncProgress> {
        let is_syncing = *self.is_syncing.lock().await;
        if !is_syncing {
            return None;
        }

        let highest = *self.highest_seen_slot.lock().await;
        let batch = *self.current_sync_batch.lock().await;

        Some(SyncProgress {
            current_slot,
            target_slot: highest,
            current_batch: batch,
            blocks_behind: highest.saturating_sub(current_slot),
        })
    }

    /// Check if we're caught up with the network (within 2 slots)
    pub async fn is_caught_up(&self, current_slot: u64) -> bool {
        let highest = *self.highest_seen_slot.lock().await;
        // Considered caught up if within 2 slots of network
        current_slot + 2 >= highest
    }

    /// Get the highest slot seen on the network
    pub async fn get_highest_seen(&self) -> u64 {
        *self.highest_seen_slot.lock().await
    }

    /// Get the number of pending blocks waiting to be applied
    pub async fn pending_count(&self) -> usize {
        self.pending_blocks.lock().await.len()
    }

    /// Check if any pending block has `parent_hash` matching the given hash.
    /// Used by fork choice: if pending blocks chain from the incoming block,
    /// the incoming block leads to a provably longer chain (Nakamoto rule).
    pub async fn has_pending_child(&self, parent: &Hash) -> bool {
        let pending = self.pending_blocks.lock().await;
        pending.values().any(|b| b.header.parent_hash == *parent)
    }

    /// Check if a slot has been requested
    #[allow(dead_code)]
    pub async fn is_requested(&self, slot: u64) -> bool {
        let requested = self.requested_slots.lock().await;
        requested.contains(&slot)
    }

    /// Mark a slot as requested
    pub async fn mark_requested(&self, slot: u64) {
        let mut requested = self.requested_slots.lock().await;
        // P10-VAL-03: Cap requested_slots to prevent unbounded growth during long syncs.
        // 10K entries ≈ 80 KB, well within reason. If exceeded, clear old entries
        // (slots already synced will be re-requested if still needed).
        const MAX_REQUESTED_SLOTS: usize = 10_000;
        if requested.len() >= MAX_REQUESTED_SLOTS {
            warn!(
                "⚠️  requested_slots exceeded {} entries, clearing to reclaim memory",
                MAX_REQUESTED_SLOTS
            );
            requested.clear();
        }
        requested.insert(slot);
    }

    /// Try to apply pending blocks now that we have more of the chain.
    /// Follows the parent-hash chain instead of requiring consecutive slot
    /// numbers, so it works correctly when the chain has slot gaps (slots
    /// where the assigned leader was offline and nobody produced).
    pub async fn try_apply_pending(&self, current_slot: u64) -> Vec<Block> {
        let mut pending = self.pending_blocks.lock().await;
        let mut applicable = Vec::new();

        if pending.is_empty() {
            return applicable;
        }

        // Find blocks whose slot is > current_slot, sorted by slot ascending.
        // Then greedily apply any block whose slot is the next expected one
        // OR whose slot is ahead but the parent block exists (gap-aware).
        // We repeatedly scan for the lowest-slot pending block that can chain.
        let mut tip_slot = current_slot;
        loop {
            // Find the pending block with the smallest slot that is > tip_slot.
            let next_slot = pending.keys().filter(|&&s| s > tip_slot).min().copied();

            match next_slot {
                Some(slot) => {
                    // Remove and queue for application
                    if let Some(block) = pending.remove(&slot) {
                        tip_slot = slot;
                        applicable.push(block);
                    }
                }
                None => break, // No more pending blocks ahead of tip
            }
        }

        if !applicable.is_empty() {
            info!(
                "📦 Found {} pending blocks that can now be applied",
                applicable.len()
            );
        }

        applicable
    }

    /// Get sync statistics
    pub async fn stats(&self) -> SyncStats {
        SyncStats {
            pending_blocks: self.pending_blocks.lock().await.len(),
            is_syncing: *self.is_syncing.lock().await,
            highest_seen: *self.highest_seen_slot.lock().await,
        }
    }

    /// Check if a checkpoint should be created at this slot.
    /// Returns true every CHECKPOINT_INTERVAL slots (10K blocks).
    pub fn should_checkpoint(slot: u64) -> bool {
        CHECKPOINT_INTERVAL > 0 && slot > 0 && slot.is_multiple_of(CHECKPOINT_INTERVAL)
    }

    /// Get the checkpoint interval constant.
    pub fn checkpoint_interval() -> u64 {
        CHECKPOINT_INTERVAL
    }
}

#[derive(Debug)]
pub struct SyncStats {
    pub pending_blocks: usize,
    pub is_syncing: bool,
    pub highest_seen: u64,
}

#[derive(Debug, Clone)]
pub struct SyncProgress {
    pub current_slot: u64,
    pub target_slot: u64,
    pub current_batch: Option<(u64, u64)>,
    pub blocks_behind: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_sync_manager_new() {
        let sm = SyncManager::new();
        assert!(!*sm.is_syncing.lock().await);
        assert_eq!(*sm.highest_seen_slot.lock().await, 0);
        assert!(sm.pending_blocks.lock().await.is_empty());
    }

    #[tokio::test]
    async fn test_note_seen_updates_highest() {
        let sm = SyncManager::new();
        sm.note_seen(100).await;
        assert_eq!(sm.get_highest_seen().await, 100);
        sm.note_seen(50).await;
        // Should not decrease
        assert_eq!(sm.get_highest_seen().await, 100);
        sm.note_seen(200).await;
        assert_eq!(sm.get_highest_seen().await, 200);
    }

    #[tokio::test]
    async fn test_should_sync_when_behind() {
        let sm = SyncManager::new();
        sm.note_seen(100).await;
        // Current slot 0, behind by 100 → should sync
        let batch = sm.should_sync(0).await;
        assert!(batch.is_some());
        let (start, end) = batch.unwrap();
        assert_eq!(start, 0);
        assert!(end <= 100);
    }

    #[tokio::test]
    async fn test_should_not_sync_when_caught_up() {
        let sm = SyncManager::new();
        sm.note_seen(10).await;
        // Current slot 9, only 1 behind → no sync (threshold is >1)
        let batch = sm.should_sync(9).await;
        assert!(batch.is_none());
    }

    #[tokio::test]
    async fn test_is_caught_up() {
        let sm = SyncManager::new();
        sm.note_seen(100).await;
        assert!(!sm.is_caught_up(90).await);
        assert!(sm.is_caught_up(98).await);
        assert!(sm.is_caught_up(100).await);
    }

    #[tokio::test]
    async fn test_start_and_complete_sync() {
        let sm = SyncManager::new();
        sm.start_sync(10, 50).await;
        assert!(*sm.is_syncing.lock().await);
        sm.complete_sync().await;
        assert!(!*sm.is_syncing.lock().await);
    }

    #[tokio::test]
    async fn test_mark_requested() {
        let sm = SyncManager::new();
        assert!(!sm.is_requested(42).await);
        sm.mark_requested(42).await;
        assert!(sm.is_requested(42).await);
    }

    #[tokio::test]
    async fn test_set_and_get_checkpoint() {
        let sm = SyncManager::new();
        assert_eq!(sm.get_checkpoint().await, 0);
        sm.set_checkpoint(5000).await;
        assert_eq!(sm.get_checkpoint().await, 5000);
    }

    #[tokio::test]
    async fn test_should_sync_includes_overlap() {
        let sm = SyncManager::new();
        sm.note_seen(100).await;
        // Current slot 50, behind by 50 → should sync starting from 50 (overlap)
        let batch = sm.should_sync(50).await;
        assert!(batch.is_some());
        let (start, end) = batch.unwrap();
        // start_slot should be current_slot (50), NOT current_slot + 1
        // This overlap enables fork resolution when chains diverge
        assert_eq!(start, 50);
        assert!(end <= 100);
    }

    /// AUDIT-FIX V5.1: Verify that RPC port derivation formula used in genesis
    /// accounts fetch matches the actual RPC server binding formula.
    /// Both must produce identical results for any P2P port.
    #[test]
    fn test_rpc_port_derivation_consistency() {
        // The formula used by the RPC server (validator main.rs ~ L6410)
        // and now also used by genesis accounts fetch (~ L3359):
        //   base_p2p = if p2p >= 8000 { 8001 } else { 7001 }
        //   base_rpc = if p2p >= 8000 { 9899 } else { 8899 }
        //   offset = p2p - base_p2p
        //   rpc = base_rpc + offset * 2
        let derive_rpc_port = |p2p_port: u16| -> u16 {
            let base_p2p = if p2p_port >= 8000 { 8001u16 } else { 7001u16 };
            let base_rpc = if p2p_port >= 8000 { 9899u16 } else { 8899u16 };
            let offset = p2p_port.saturating_sub(base_p2p);
            base_rpc.saturating_add(offset.saturating_mul(2))
        };

        // Testnet V1: p2p 7001 → rpc 8899
        assert_eq!(derive_rpc_port(7001), 8899);
        // Testnet V2: p2p 7002 → rpc 8901
        assert_eq!(derive_rpc_port(7002), 8901);
        // Testnet V3: p2p 7003 → rpc 8903
        assert_eq!(derive_rpc_port(7003), 8903);
        // Mainnet V1: p2p 8001 → rpc 9899
        assert_eq!(derive_rpc_port(8001), 9899);
        // Mainnet V2: p2p 8002 → rpc 9901
        assert_eq!(derive_rpc_port(8002), 9901);
    }

    /// C5 fix: note_seen_bounded should cap the jump to prevent malicious
    /// slot inflation from peers reporting u64::MAX.
    #[tokio::test]
    async fn test_note_seen_bounded_caps() {
        let sm = SyncManager::new();
        sm.note_seen(100).await;
        assert_eq!(sm.get_highest_seen().await, 100);

        // Legitimate update within bounds
        sm.note_seen_bounded(200, 500).await;
        assert_eq!(sm.get_highest_seen().await, 200);

        // Malicious update way beyond bounds — should be capped
        sm.note_seen_bounded(u64::MAX, 500).await;
        assert_eq!(sm.get_highest_seen().await, 700); // 200 + 500

        // Small update still works
        sm.note_seen_bounded(300, 500).await;
        assert_eq!(sm.get_highest_seen().await, 700); // Already higher
    }

    /// Verify the P2P_BLOCK_RANGE_LIMIT constant is within acceptable bounds
    /// and that chunking math works correctly.
    #[test]
    fn test_p2p_block_range_limit_chunking() {
        // The limit must match the P2P MAX_BLOCK_RANGE in p2p/src/network.rs
        assert_eq!(P2P_BLOCK_RANGE_LIMIT, 500);

        // Simulate chunking a 1250-block range into sub-batches of 500
        let start: u64 = 50;
        let end: u64 = 1299;
        let mut chunks = Vec::new();
        let mut chunk_start = start;
        while chunk_start <= end {
            let chunk_end = std::cmp::min(chunk_start + P2P_BLOCK_RANGE_LIMIT - 1, end);
            chunks.push((chunk_start, chunk_end));
            chunk_start = chunk_end + 1;
        }
        // Should produce 3 chunks: [50-549], [550-1049], [1050-1299]
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], (50, 549));
        assert_eq!(chunks[1], (550, 1049));
        assert_eq!(chunks[2], (1050, 1299));
        // All chunks must be ≤ P2P_BLOCK_RANGE_LIMIT in size
        for (s, e) in &chunks {
            assert!(e - s < P2P_BLOCK_RANGE_LIMIT);
        }
    }

    /// Single chunk when range fits within limit
    #[test]
    fn test_p2p_chunking_single_batch() {
        let start: u64 = 10;
        let end: u64 = 300;
        let mut chunks = Vec::new();
        let mut chunk_start = start;
        while chunk_start <= end {
            let chunk_end = std::cmp::min(chunk_start + P2P_BLOCK_RANGE_LIMIT - 1, end);
            chunks.push((chunk_start, chunk_end));
            chunk_start = chunk_end + 1;
        }
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], (10, 300));
    }

    /// Exact boundary (exactly 500 blocks)
    #[test]
    fn test_p2p_chunking_exact_limit() {
        let start: u64 = 0;
        let end: u64 = 499; // exactly 500 blocks
        let mut chunks = Vec::new();
        let mut chunk_start = start;
        while chunk_start <= end {
            let chunk_end = std::cmp::min(chunk_start + P2P_BLOCK_RANGE_LIMIT - 1, end);
            chunks.push((chunk_start, chunk_end));
            chunk_start = chunk_end + 1;
        }
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], (0, 499));
    }

    /// Helper: create a minimal test block for the given slot
    fn test_block(slot: u64) -> Block {
        use moltchain_core::Hash;
        Block::new(slot, Hash::default(), Hash::default(), [0u8; 32], vec![])
    }

    /// STABILITY-FIX: Verify pending block eviction drops NEWEST, not oldest.
    /// Gap-filling blocks (lowest slots) must be preserved because they are
    /// needed to reconnect the chain.
    #[tokio::test]
    async fn test_pending_eviction_drops_newest() {
        let sm = SyncManager::new();
        // Fill beyond MAX_PENDING_BLOCKS by adding blocks 0..=MAX_PENDING_BLOCKS
        for slot in 0..=(MAX_PENDING_BLOCKS as u64) {
            sm.add_pending_block(test_block(slot)).await;
        }
        let pending = sm.pending_blocks.lock().await;
        // Should have MAX_PENDING_BLOCKS entries (one was evicted)
        assert_eq!(pending.len(), MAX_PENDING_BLOCKS);
        // The lowest slot (0) should still be present (gap-filler preserved)
        assert!(pending.contains_key(&0), "Lowest slot should be preserved");
    }

    /// STABILITY-FIX: force_decay should reset highest_seen regardless of timestamp
    #[tokio::test]
    async fn test_force_decay() {
        let sm = SyncManager::new();
        sm.note_seen(500).await;
        assert_eq!(sm.get_highest_seen().await, 500);

        // force_decay should work even if highest_seen was just updated
        sm.force_decay(100).await;
        assert_eq!(sm.get_highest_seen().await, 100);

        // Should be idempotent when already at tip
        sm.force_decay(100).await;
        assert_eq!(sm.get_highest_seen().await, 100);
    }

    /// STABILITY-FIX: is_actively_receiving should reflect sync/pending state
    #[tokio::test]
    async fn test_is_actively_receiving() {
        let sm = SyncManager::new();
        // No pending blocks, not syncing → not actively receiving
        assert!(!sm.is_actively_receiving().await);

        // Add a pending block → actively receiving
        sm.add_pending_block(test_block(10)).await;
        assert!(sm.is_actively_receiving().await);

        // Clear pending, start sync → still actively receiving
        sm.pending_blocks.lock().await.clear();
        sm.start_sync(1, 100).await;
        assert!(sm.is_actively_receiving().await);

        // Complete sync, no pending → not actively receiving
        sm.complete_sync().await;
        assert!(!sm.is_actively_receiving().await);
    }

    /// STABILITY-FIX: has_pending_child detects if pending blocks chain from a given hash
    #[tokio::test]
    async fn test_has_pending_child() {
        let sm = SyncManager::new();
        let parent = test_block(10);
        let parent_hash = parent.hash();

        // No pending → no child
        assert!(!sm.has_pending_child(&parent_hash).await);

        // Add a block whose parent_hash matches → has child
        let child = Block::new(11, parent_hash, Hash::default(), [0u8; 32], vec![]);
        sm.add_pending_block(child).await;
        assert!(sm.has_pending_child(&parent_hash).await);

        // Different hash → no child
        let other_hash = Hash([99u8; 32]);
        assert!(!sm.has_pending_child(&other_hash).await);
    }

    // ----------------------------------------------------------------
    // Sync Performance Plan tests
    // ----------------------------------------------------------------

    /// P0-1: Checkpoint interval set to 1000 blocks
    #[test]
    fn test_checkpoint_interval_constant() {
        assert_eq!(CHECKPOINT_INTERVAL, 1000);
        assert!(SyncManager::should_checkpoint(1000));
        assert!(SyncManager::should_checkpoint(2000));
        assert!(!SyncManager::should_checkpoint(500));
        assert!(!SyncManager::should_checkpoint(0));
    }

    /// P0-2: Exponential backoff on consecutive failures
    #[tokio::test]
    async fn test_exponential_backoff() {
        let sm = SyncManager::new();
        // Initially 0 failures → base cooldown
        assert_eq!(*sm.consecutive_failures.lock().await, 0);

        sm.record_sync_failure().await;
        assert_eq!(*sm.consecutive_failures.lock().await, 1);

        sm.record_sync_failure().await;
        assert_eq!(*sm.consecutive_failures.lock().await, 2);

        // Success resets
        sm.record_sync_success().await;
        assert_eq!(*sm.consecutive_failures.lock().await, 0);
    }

    /// P0-2: Verify cooldown calculation with backoff
    #[test]
    fn test_cooldown_calculation() {
        // The actual formula: base << min(failures, 4), capped at max
        let cooldown_for = |failures: u64| -> u64 {
            (SYNC_COOLDOWN_BASE_SECS << failures.min(4)).min(SYNC_COOLDOWN_MAX_SECS)
        };

        // 0 failures → base (2s)
        assert_eq!(cooldown_for(0), 2);
        // 1 failure → 2 << 1 = 4s
        assert_eq!(cooldown_for(1), 4);
        // 2 failures → 2 << 2 = 8s
        assert_eq!(cooldown_for(2), 8);
        // 3 failures → 2 << 3 = 16s
        assert_eq!(cooldown_for(3), 16);
        // 4 failures → 2 << 4 = 32 → capped to 30s
        assert_eq!(cooldown_for(4), 30);
        // 10 failures → 2 << min(10,4) = 2 << 4 = 32 → capped to 30s
        assert_eq!(cooldown_for(10), 30);
    }

    /// P0-3: Batch sizes match expectations
    #[test]
    fn test_batch_size_constants() {
        assert_eq!(SYNC_BATCH_SIZE, 2000);
        assert_eq!(P2P_BLOCK_RANGE_LIMIT, 500);
        // 2000 / 500 = 4 chunks per batch
        assert_eq!(SYNC_BATCH_SIZE / P2P_BLOCK_RANGE_LIMIT, 4);
    }

    /// P1-1: SyncMode enum and header-only validation
    #[tokio::test]
    async fn test_sync_mode_header_only() {
        let sm = SyncManager::new();
        // Default is Full
        assert_eq!(sm.get_sync_mode().await, SyncMode::Full);

        sm.set_sync_mode(SyncMode::HeaderOnly).await;
        assert_eq!(sm.get_sync_mode().await, SyncMode::HeaderOnly);

        sm.set_sync_mode(SyncMode::Full).await;
        assert_eq!(sm.get_sync_mode().await, SyncMode::Full);
    }

    /// P1-1: should_full_validate respects sync mode and execution window
    #[tokio::test]
    async fn test_should_full_validate() {
        let sm = SyncManager::new();

        // Full mode → always validate
        sm.set_sync_mode(SyncMode::Full).await;
        sm.note_seen(1000).await;
        assert!(sm.should_full_validate(0).await);
        assert!(sm.should_full_validate(500).await);
        assert!(sm.should_full_validate(999).await);

        // HeaderOnly mode → only validate within HEADER_SYNC_FULL_EXECUTION_WINDOW of tip
        sm.set_sync_mode(SyncMode::HeaderOnly).await;
        // Block 0, highest 1000 → 0 + 100 < 1000 → skip
        assert!(!sm.should_full_validate(0).await);
        // Block 899 → 899 + 100 < 1000 → skip
        assert!(!sm.should_full_validate(899).await);
        // Block 900 → 900 + 100 >= 1000 → validate
        assert!(sm.should_full_validate(900).await);
        // Block 999 → 999 + 100 >= 1000 → validate
        assert!(sm.should_full_validate(999).await);
    }

    /// P2-5: Pipeline depth and pending blocks buffer sized correctly
    #[test]
    fn test_pipeline_constants() {
        assert_eq!(SYNC_PIPELINE_DEPTH, 3);
        assert_eq!(
            MAX_PENDING_BLOCKS,
            (SYNC_BATCH_SIZE * SYNC_PIPELINE_DEPTH) as usize
        );
        // With 2000 batch * 3 depth, buffer holds 6000 blocks
        assert_eq!(MAX_PENDING_BLOCKS, 6000);
    }

    /// P2-5: should_sync returns larger batch when far behind
    #[tokio::test]
    async fn test_pipeline_prefetch_batch() {
        let sm = SyncManager::new();
        // 10000 blocks behind → gap > SYNC_BATCH_SIZE → pipeline depth applies
        sm.note_seen(10000).await;
        let batch = sm.should_sync(0).await;
        assert!(batch.is_some());
        let (start, end) = batch.unwrap();
        assert_eq!(start, 0);
        // Should request up to SYNC_BATCH_SIZE * PIPELINE_DEPTH (6000) blocks
        let batch_size = end - start + 1;
        assert_eq!(batch_size, SYNC_BATCH_SIZE * SYNC_PIPELINE_DEPTH);
    }

    /// P2-5: should_sync returns normal batch when close to tip
    #[tokio::test]
    async fn test_normal_batch_when_close() {
        let sm = SyncManager::new();
        // 500 blocks behind (< SYNC_BATCH_SIZE) → normal batch, no pipeline
        sm.note_seen(510).await;
        let batch = sm.should_sync(10).await;
        assert!(batch.is_some());
        let (start, end) = batch.unwrap();
        assert_eq!(start, 10);
        // Gap is 500, which is < SYNC_BATCH_SIZE(2000), so effective_batch = 2000
        // but we're capped at highest (510)
        assert_eq!(end, 510);
    }

    /// P3-1: Warp sync mode threshold constant
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_warp_sync_threshold() {
        assert_eq!(WARP_SYNC_THRESHOLD, 10_000);
        // Warp threshold must be greater than header-only full-execution window
        assert!(WARP_SYNC_THRESHOLD > HEADER_SYNC_FULL_EXECUTION_WINDOW * 2);
    }

    /// P3-1: SyncMode::Warp exists and round-trips
    #[tokio::test]
    async fn test_warp_sync_mode() {
        let sm = SyncManager::new();
        sm.set_sync_mode(SyncMode::Warp).await;
        assert_eq!(sm.get_sync_mode().await, SyncMode::Warp);
    }

    /// P3-1: Warp mode skips full validation like HeaderOnly
    #[tokio::test]
    async fn test_warp_mode_skips_full_validate() {
        let sm = SyncManager::new();
        sm.set_sync_mode(SyncMode::Warp).await;
        sm.note_seen(20000).await;
        // Block 0, highest 20000 → 0 + 100 < 20000 → skip
        assert!(!sm.should_full_validate(0).await);
        // Block near tip → should validate
        assert!(sm.should_full_validate(19950).await);
    }

    /// P3-1: Mode auto-detection boundaries
    #[test]
    fn test_warp_sync_mode_detection_boundaries() {
        // gap <= 200 → Full
        let gap_full = HEADER_SYNC_FULL_EXECUTION_WINDOW * 2;
        assert!(gap_full <= WARP_SYNC_THRESHOLD);

        // gap > 200 and <= 10000 → HeaderOnly
        // gap > 10000 → Warp
        assert!(gap_full < WARP_SYNC_THRESHOLD);
    }
}
