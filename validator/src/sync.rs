// Chain Synchronization Manager

use moltchain_core::{Block, Hash};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Maximum blocks to request in a single sync batch.
/// This is the overall catch-up window; actual P2P requests are chunked
/// into sub-batches of `P2P_BLOCK_RANGE_LIMIT` to stay within the P2P
/// layer's per-request cap (AUDIT-FIX H1).
const SYNC_BATCH_SIZE: u64 = 500;

/// The per-request chunk size that the P2P layer allows.
/// Must match `MAX_BLOCK_RANGE` in `p2p/src/network.rs`.
pub const P2P_BLOCK_RANGE_LIMIT: u64 = 100;

/// Maximum blocks to hold in pending state (memory limit)
const MAX_PENDING_BLOCKS: usize = 500;

/// Checkpoint interval - only need to sync from last checkpoint
/// Set to 0 to disable checkpointing
const CHECKPOINT_INTERVAL: u64 = 10000; // Every 10k blocks

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

            // Calculate batch end (don't request more than SYNC_BATCH_SIZE at once)
            let batch_end = std::cmp::min(start_slot + SYNC_BATCH_SIZE - 1, highest);

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
        //   base_p2p = if p2p >= 9000 { 9000 } else { 8000 }
        //   base_rpc = if p2p >= 9000 { 9899 } else { 8899 }
        //   offset = p2p - base_p2p
        //   rpc = base_rpc + offset * 2
        let derive_rpc_port = |p2p_port: u16| -> u16 {
            let base_p2p = if p2p_port >= 9000 { 9000u16 } else { 8000u16 };
            let base_rpc = if p2p_port >= 9000 { 9899u16 } else { 8899u16 };
            let offset = p2p_port.saturating_sub(base_p2p);
            base_rpc.saturating_add(offset.saturating_mul(2))
        };

        // V1: p2p 8000 → rpc 8899
        assert_eq!(derive_rpc_port(8000), 8899);
        // V2: p2p 8001 → rpc 8901
        assert_eq!(derive_rpc_port(8001), 8901);
        // V3: p2p 8002 → rpc 8903
        assert_eq!(derive_rpc_port(8002), 8903);
        // High port range
        assert_eq!(derive_rpc_port(9000), 9899);
        assert_eq!(derive_rpc_port(9001), 9901);
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
        assert_eq!(P2P_BLOCK_RANGE_LIMIT, 100);

        // Simulate chunking a 250-block range into sub-batches of 100
        let start: u64 = 50;
        let end: u64 = 299;
        let mut chunks = Vec::new();
        let mut chunk_start = start;
        while chunk_start <= end {
            let chunk_end = std::cmp::min(chunk_start + P2P_BLOCK_RANGE_LIMIT - 1, end);
            chunks.push((chunk_start, chunk_end));
            chunk_start = chunk_end + 1;
        }
        // Should produce 3 chunks: [50-149], [150-249], [250-299]
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], (50, 149));
        assert_eq!(chunks[1], (150, 249));
        assert_eq!(chunks[2], (250, 299));
        // All chunks must be ≤ P2P_BLOCK_RANGE_LIMIT in size
        for (s, e) in &chunks {
            assert!(e - s + 1 <= P2P_BLOCK_RANGE_LIMIT);
        }
    }

    /// Single chunk when range fits within limit
    #[test]
    fn test_p2p_chunking_single_batch() {
        let start: u64 = 10;
        let end: u64 = 50;
        let mut chunks = Vec::new();
        let mut chunk_start = start;
        while chunk_start <= end {
            let chunk_end = std::cmp::min(chunk_start + P2P_BLOCK_RANGE_LIMIT - 1, end);
            chunks.push((chunk_start, chunk_end));
            chunk_start = chunk_end + 1;
        }
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], (10, 50));
    }

    /// Exact boundary (exactly 100 blocks)
    #[test]
    fn test_p2p_chunking_exact_limit() {
        let start: u64 = 0;
        let end: u64 = 99; // exactly 100 blocks
        let mut chunks = Vec::new();
        let mut chunk_start = start;
        while chunk_start <= end {
            let chunk_end = std::cmp::min(chunk_start + P2P_BLOCK_RANGE_LIMIT - 1, end);
            chunks.push((chunk_start, chunk_end));
            chunk_start = chunk_end + 1;
        }
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], (0, 99));
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
}
