// MoltChain Mempool - Transaction Priority Queue with MoltyID Trust Tier Integration

use crate::hash::Hash;
use crate::processor::get_trust_tier;
use crate::transaction::Transaction;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

/// Reputation threshold for express-lane inclusion (Tier 4+)
const EXPRESS_LANE_MIN_REPUTATION: u64 = 5_000;

/// Transaction with priority metadata
#[derive(Clone, Debug)]
struct PrioritizedTransaction {
    transaction: Transaction,
    fee: u64,
    reputation: u64,
    timestamp: u64,
    hash: Hash,
}

impl PrioritizedTransaction {
    /// Compute reputation-weighted priority.
    /// Trust tier provides a bonus multiplier on effective priority:
    ///   Tier 0: 1.0x (no bonus)
    ///   Tier 1: 1.1x
    ///   Tier 2: 1.25x
    ///   Tier 3: 1.5x
    ///   Tier 4: 2.0x
    ///   Tier 5: 3.0x
    fn effective_priority(&self) -> u64 {
        let tier = get_trust_tier(self.reputation);
        let multiplier_bps: u64 = match tier {
            5 => 30_000, // 3.0x
            4 => 20_000, // 2.0x
            3 => 15_000, // 1.5x
            2 => 12_500, // 1.25x
            1 => 11_000, // 1.1x
            _ => 10_000, // 1.0x
        };
        self.fee.saturating_mul(multiplier_bps) / 10_000
    }
}

impl PartialEq for PrioritizedTransaction {
    fn eq(&self, other: &Self) -> bool {
        // L2 fix: compare by transaction hash (identity), not priority
        self.hash == other.hash
    }
}

impl Eq for PrioritizedTransaction {}

impl PartialOrd for PrioritizedTransaction {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PrioritizedTransaction {
    fn cmp(&self, other: &Self) -> Ordering {
        // Compute effective priority: fee + reputation bonus
        // Higher trust tier = bigger multiplier on effective priority
        let self_priority = self.effective_priority();
        let other_priority = other.effective_priority();

        match self_priority.cmp(&other_priority) {
            Ordering::Equal => {
                // If priorities equal, older timestamp = higher priority (FIFO)
                other.timestamp.cmp(&self.timestamp)
            }
            ord => ord,
        }
    }
}

/// Transaction mempool with priority queue and express lane for high-reputation agents
pub struct Mempool {
    /// Priority queue of transactions
    queue: BinaryHeap<PrioritizedTransaction>,

    /// Express lane for Tier 4+ agents with guaranteed block inclusion
    express_queue: BinaryHeap<PrioritizedTransaction>,

    /// Transaction hash -> transaction (for deduplication)
    transactions: HashMap<Hash, Transaction>,

    /// Maximum mempool size
    max_size: usize,

    /// Expiration time (seconds)
    expiration_time: u64,
}

impl Mempool {
    /// Create new mempool
    pub fn new(max_size: usize, expiration_time: u64) -> Self {
        Mempool {
            queue: BinaryHeap::new(),
            express_queue: BinaryHeap::new(),
            transactions: HashMap::new(),
            max_size,
            expiration_time,
        }
    }

    /// Add transaction to mempool
    pub fn add_transaction(
        &mut self,
        transaction: Transaction,
        fee: u64,
        reputation: u64,
    ) -> Result<(), String> {
        let tx_hash = transaction.hash();

        // Check if already in mempool
        if self.transactions.contains_key(&tx_hash) {
            return Err("Transaction already in mempool".to_string());
        }

        // Check mempool size
        if self.transactions.len() >= self.max_size {
            return Err("Mempool full".to_string());
        }

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let prioritized = PrioritizedTransaction {
            transaction: transaction.clone(),
            fee,
            reputation,
            timestamp,
            hash: tx_hash,
        };

        // Tier 4+ agents go to express lane for guaranteed block inclusion
        if reputation >= EXPRESS_LANE_MIN_REPUTATION {
            self.express_queue.push(prioritized);
        } else {
            self.queue.push(prioritized);
        }
        self.transactions.insert(tx_hash, transaction);

        Ok(())
    }

    /// Get top N transactions by priority.
    /// Express lane transactions are drained first, then regular queue fills remaining slots.
    pub fn get_top_transactions(&mut self, count: usize) -> Vec<Transaction> {
        let mut result = Vec::new();
        let mut temp_express = Vec::new();
        let mut temp_regular = Vec::new();

        // First: drain express queue
        while result.len() < count {
            if let Some(ptx) = self.express_queue.pop() {
                result.push(ptx.transaction.clone());
                temp_express.push(ptx);
            } else {
                break;
            }
        }

        // Then: fill remaining from regular queue
        while result.len() < count {
            if let Some(ptx) = self.queue.pop() {
                result.push(ptx.transaction.clone());
                temp_regular.push(ptx);
            } else {
                break;
            }
        }

        // Put them back in the queues
        for ptx in temp_express {
            self.express_queue.push(ptx);
        }
        for ptx in temp_regular {
            self.queue.push(ptx);
        }

        result
    }

    /// Remove transaction from mempool (after inclusion in block)
    pub fn remove_transaction(&mut self, tx_hash: &Hash) {
        if self.transactions.remove(tx_hash).is_some() {
            // Rebuild both queues without the removed transaction
            let regular: Vec<_> = self.queue.drain().collect();
            self.queue = regular
                .into_iter()
                .filter(|ptx| &ptx.hash != tx_hash)
                .collect();

            let express: Vec<_> = self.express_queue.drain().collect();
            self.express_queue = express
                .into_iter()
                .filter(|ptx| &ptx.hash != tx_hash)
                .collect();
        }
    }

    /// PERF-FIX 9: Bulk remove transactions from mempool after block inclusion.
    /// Rebuilds heaps only ONCE instead of per-transaction (O(n) instead of O(n*m)).
    pub fn remove_transactions_bulk(&mut self, tx_hashes: &[Hash]) {
        let hash_set: std::collections::HashSet<&Hash> = tx_hashes.iter().collect();
        let mut any_removed = false;
        for h in tx_hashes {
            if self.transactions.remove(h).is_some() {
                any_removed = true;
            }
        }
        if any_removed {
            let regular: Vec<_> = self.queue.drain().collect();
            self.queue = regular
                .into_iter()
                .filter(|ptx| !hash_set.contains(&ptx.hash))
                .collect();

            let express: Vec<_> = self.express_queue.drain().collect();
            self.express_queue = express
                .into_iter()
                .filter(|ptx| !hash_set.contains(&ptx.hash))
                .collect();
        }
    }

    /// Remove expired transactions
    pub fn cleanup_expired(&mut self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Clean regular queue
        let transactions: Vec<_> = self.queue.drain().collect();
        let (valid, expired): (Vec<_>, Vec<_>) = transactions
            .into_iter()
            .partition(|ptx| now.saturating_sub(ptx.timestamp) < self.expiration_time);
        for ptx in &expired {
            self.transactions.remove(&ptx.hash);
        }
        self.queue = valid.into_iter().collect();

        // Clean express queue
        let express: Vec<_> = self.express_queue.drain().collect();
        let (valid_express, expired_express): (Vec<_>, Vec<_>) = express
            .into_iter()
            .partition(|ptx| now.saturating_sub(ptx.timestamp) < self.expiration_time);
        for ptx in &expired_express {
            self.transactions.remove(&ptx.hash);
        }
        self.express_queue = valid_express.into_iter().collect();
    }

    /// Prune transactions whose recent_blockhash is no longer in the valid set.
    /// This prevents the death-spiral where a validator falls behind, accumulates
    /// stale-blockhash transactions, and then drops them all at block-production
    /// time (producing only empty heartbeats).
    /// Returns the number of evicted transactions.
    pub fn prune_stale_blockhashes(
        &mut self,
        valid_blockhashes: &std::collections::HashSet<Hash>,
    ) -> usize {
        let before = self.transactions.len();

        // Partition regular queue
        let regular: Vec<_> = self.queue.drain().collect();
        let (valid_regular, stale_regular): (Vec<_>, Vec<_>) = regular
            .into_iter()
            .partition(|ptx| valid_blockhashes.contains(&ptx.transaction.message.recent_blockhash));
        for ptx in &stale_regular {
            self.transactions.remove(&ptx.hash);
        }
        self.queue = valid_regular.into_iter().collect();

        // Partition express queue
        let express: Vec<_> = self.express_queue.drain().collect();
        let (valid_express, stale_express): (Vec<_>, Vec<_>) = express
            .into_iter()
            .partition(|ptx| valid_blockhashes.contains(&ptx.transaction.message.recent_blockhash));
        for ptx in &stale_express {
            self.transactions.remove(&ptx.hash);
        }
        self.express_queue = valid_express.into_iter().collect();

        before - self.transactions.len()
    }

    /// Get mempool size
    pub fn size(&self) -> usize {
        self.transactions.len()
    }

    /// Check if transaction exists in mempool
    pub fn contains(&self, tx_hash: &Hash) -> bool {
        self.transactions.contains_key(tx_hash)
    }

    /// Clear mempool
    pub fn clear(&mut self) {
        self.queue.clear();
        self.express_queue.clear();
        self.transactions.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Instruction, Message, Pubkey};

    fn create_test_transaction(nonce: u8) -> Transaction {
        let message = Message::new(
            vec![Instruction {
                program_id: Pubkey([nonce; 32]),
                accounts: vec![Pubkey([nonce + 1; 32]), Pubkey([nonce + 2; 32])],
                data: vec![nonce],
            }],
            Hash::default(),
        );

        Transaction {
            signatures: vec![[nonce; 64]],
            message,
        }
    }

    #[test]
    fn test_mempool_add() {
        let mut mempool = Mempool::new(100, 300);
        let tx = create_test_transaction(1);

        assert!(mempool.add_transaction(tx.clone(), 1000, 0).is_ok());
        assert_eq!(mempool.size(), 1);

        // Duplicate should fail
        assert!(mempool.add_transaction(tx, 1000, 0).is_err());
    }

    #[test]
    fn test_mempool_priority() {
        let mut mempool = Mempool::new(100, 300);

        let tx1 = create_test_transaction(1);
        let tx2 = create_test_transaction(2);
        let tx3 = create_test_transaction(3);

        mempool.add_transaction(tx1, 100, 0).unwrap();
        mempool.add_transaction(tx2.clone(), 1000, 0).unwrap();
        mempool.add_transaction(tx3, 500, 0).unwrap();

        let top = mempool.get_top_transactions(1);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].hash(), tx2.hash()); // Highest fee should be first
    }

    #[test]
    fn test_mempool_max_size() {
        let mut mempool = Mempool::new(2, 300);

        let tx1 = create_test_transaction(1);
        let tx2 = create_test_transaction(2);
        let tx3 = create_test_transaction(3);

        assert!(mempool.add_transaction(tx1, 100, 0).is_ok());
        assert!(mempool.add_transaction(tx2, 200, 0).is_ok());
        assert!(mempool.add_transaction(tx3, 300, 0).is_err()); // Should fail
    }

    #[test]
    fn test_mempool_reputation_priority() {
        // Two txs with same fee but different reputation — higher rep should come first
        let mut mempool = Mempool::new(100, 300);

        let tx_low_rep = create_test_transaction(1);
        let tx_high_rep = create_test_transaction(2);

        // Both have fee=1000, but tx_high_rep has reputation=1000 (Tier 3 → 1.5x)
        mempool.add_transaction(tx_low_rep, 1000, 0).unwrap();
        mempool
            .add_transaction(tx_high_rep.clone(), 1000, 1000)
            .unwrap();

        let top = mempool.get_top_transactions(1);
        assert_eq!(top.len(), 1);
        // Tier 3 tx: effective_priority = 1000 * 15000 / 10000 = 1500
        // Tier 0 tx: effective_priority = 1000 * 10000 / 10000 = 1000
        assert_eq!(top[0].hash(), tx_high_rep.hash());
    }

    #[test]
    fn test_mempool_express_lane() {
        // Tier 5 agent tx should come before higher-fee regular tx
        let mut mempool = Mempool::new(100, 300);

        let tx_regular = create_test_transaction(1);
        let tx_express = create_test_transaction(2);

        // Regular tx: very high fee, no reputation
        mempool.add_transaction(tx_regular, 100_000, 0).unwrap();
        // Express tx: low fee but Tier 5 reputation (10000+) → goes to express queue
        mempool
            .add_transaction(tx_express.clone(), 100, 10_000)
            .unwrap();

        let top = mempool.get_top_transactions(1);
        assert_eq!(top.len(), 1);
        // Express queue is drained first, so the express tx comes out first
        assert_eq!(top[0].hash(), tx_express.hash());
    }

    #[test]
    fn test_mempool_effective_priority() {
        // Verify the multiplier math for each tier
        let make_ptx = |fee: u64, reputation: u64| -> PrioritizedTransaction {
            PrioritizedTransaction {
                transaction: create_test_transaction(1),
                fee,
                reputation,
                timestamp: 0,
                hash: Hash::default(),
            }
        };

        // Tier 0 (rep 0): 1.0x
        assert_eq!(make_ptx(10_000, 0).effective_priority(), 10_000);
        // Tier 1 (rep 100): 1.1x
        assert_eq!(make_ptx(10_000, 100).effective_priority(), 11_000);
        // Tier 2 (rep 500): 1.25x
        assert_eq!(make_ptx(10_000, 500).effective_priority(), 12_500);
        // Tier 3 (rep 1000): 1.5x
        assert_eq!(make_ptx(10_000, 1_000).effective_priority(), 15_000);
        // Tier 4 (rep 5000): 2.0x
        assert_eq!(make_ptx(10_000, 5_000).effective_priority(), 20_000);
        // Tier 5 (rep 10000): 3.0x
        assert_eq!(make_ptx(10_000, 10_000).effective_priority(), 30_000);
    }
}
