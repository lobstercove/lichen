// Chain Synchronization Manager

use moltchain_core::Block;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Maximum blocks to request in a single sync batch
/// This prevents memory exhaustion and allows progressive sync
const SYNC_BATCH_SIZE: u64 = 100;

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
            current_sync_batch: Arc::new(Mutex::new(None)),
            last_checkpoint: Arc::new(Mutex::new(0)),
        }
    }

    /// Set checkpoint (for fast bootstrapping from snapshots)
    #[allow(dead_code)]
    pub async fn set_checkpoint(&self, slot: u64) {
        let mut checkpoint = self.last_checkpoint.lock().await;
        *checkpoint = slot;
        info!("📍 Checkpoint set at slot {}", slot);
    }

    /// Get last checkpoint slot
    #[allow(dead_code)]
    pub async fn get_checkpoint(&self) -> u64 {
        *self.last_checkpoint.lock().await
    }

    /// Add a block that can't be applied yet (missing parent)
    pub async fn add_pending_block(&self, block: Block) {
        let slot = block.header.slot;
        let mut pending = self.pending_blocks.lock().await;

        // Memory protection: if too many pending blocks, drop oldest
        if pending.len() >= MAX_PENDING_BLOCKS {
            if let Some(oldest_slot) = pending.keys().min().copied() {
                pending.remove(&oldest_slot);
                warn!(
                    "⚠️  Dropped old pending block {} (memory limit)",
                    oldest_slot
                );
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
        }
    }

    /// Check if we need to start syncing (returns next batch to sync)
    pub async fn should_sync(&self, current_slot: u64) -> Option<(u64, u64)> {
        let highest = *self.highest_seen_slot.lock().await;
        let is_syncing = *self.is_syncing.lock().await;
        let current_batch = self.current_sync_batch.lock().await;

        // If already syncing a batch, don't start another
        if is_syncing && current_batch.is_some() {
            return None;
        }

        // If we're behind by more than 5 blocks and not already syncing
        if highest > current_slot + 5 {
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

    /// Get progress info for sync
    #[allow(dead_code)]
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

    /// Check if we're caught up with the network (within 3 slots)
    pub async fn is_caught_up(&self, current_slot: u64) -> bool {
        let highest = *self.highest_seen_slot.lock().await;
        // Considered caught up if within 3 slots of network
        current_slot + 3 >= highest
    }

    /// Get the highest slot seen on the network
    pub async fn get_highest_seen(&self) -> u64 {
        *self.highest_seen_slot.lock().await
    }

    /// Check if we've already requested this slot
    #[allow(dead_code)]
    pub async fn is_requested(&self, slot: u64) -> bool {
        let requested = self.requested_slots.lock().await;
        requested.contains(&slot)
    }

    /// Mark a slot as requested
    pub async fn mark_requested(&self, slot: u64) {
        let mut requested = self.requested_slots.lock().await;
        requested.insert(slot);
    }

    /// Try to apply pending blocks now that we have more of the chain
    pub async fn try_apply_pending(&self, current_slot: u64) -> Vec<Block> {
        let mut pending = self.pending_blocks.lock().await;
        let mut applicable = Vec::new();

        // Find blocks that can now be applied (sequential from current_slot)
        let mut next_slot = current_slot + 1;
        while let Some(block) = pending.remove(&next_slot) {
            applicable.push(block);
            next_slot += 1;
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
    #[allow(dead_code)]
    pub async fn stats(&self) -> SyncStats {
        SyncStats {
            pending_blocks: self.pending_blocks.lock().await.len(),
            is_syncing: *self.is_syncing.lock().await,
            highest_seen: *self.highest_seen_slot.lock().await,
        }
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct SyncStats {
    pub pending_blocks: usize,
    pub is_syncing: bool,
    pub highest_seen: u64,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
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
        // Current slot 8, only 2 behind → no sync (threshold is 5)
        let batch = sm.should_sync(8).await;
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
}
