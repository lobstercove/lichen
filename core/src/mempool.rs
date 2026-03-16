// MoltChain Mempool - Fee-Only Transaction Priority Queue
//
// M-8 FIX: Express lane removed. All transactions ordered strictly by fee.
// Reputation may influence fee discounts but NEVER queue priority.

use crate::hash::Hash;
use crate::transaction::Transaction;
use crate::Pubkey;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

/// AUDIT-FIX H-6: Maximum pending transactions per sender
const MAX_PENDING_PER_SENDER: usize = 100;

/// Transaction with priority metadata
#[derive(Clone, Debug)]
struct PrioritizedTransaction {
    transaction: Transaction,
    fee: u64,
    timestamp: u64,
    hash: Hash,
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
        // M-8 FIX: Strict fee-only ordering. No reputation bonus.
        match self.fee.cmp(&other.fee) {
            Ordering::Equal => {
                // If fees equal, older timestamp = higher priority (FIFO)
                other.timestamp.cmp(&self.timestamp)
            }
            ord => ord,
        }
    }
}

/// Transaction mempool with fee-based priority queue
pub struct Mempool {
    /// Priority queue of transactions (ordered by fee, then FIFO)
    queue: BinaryHeap<PrioritizedTransaction>,

    /// Transaction hash -> transaction (for deduplication)
    transactions: HashMap<Hash, Transaction>,

    /// AUDIT-FIX H-6: Per-sender pending transaction count
    sender_counts: HashMap<Pubkey, usize>,

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
            transactions: HashMap::new(),
            sender_counts: HashMap::new(),
            max_size,
            expiration_time,
        }
    }

    /// Add transaction to mempool.
    /// The `_reputation` parameter is accepted for API compatibility but
    /// is intentionally ignored for ordering (M-8 fix: fee-only priority).
    pub fn add_transaction(
        &mut self,
        transaction: Transaction,
        fee: u64,
        _reputation: u64,
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

        // AUDIT-FIX H-6: Per-sender transaction limit
        let sender = transaction.sender();
        let sender_count = self.sender_counts.get(&sender).copied().unwrap_or(0);
        if sender_count >= MAX_PENDING_PER_SENDER {
            return Err(format!(
                "Sender {} has {} pending transactions (max {})",
                sender, sender_count, MAX_PENDING_PER_SENDER
            ));
        }

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let prioritized = PrioritizedTransaction {
            transaction: transaction.clone(),
            fee,
            timestamp,
            hash: tx_hash,
        };

        self.queue.push(prioritized);
        self.transactions.insert(tx_hash, transaction);
        *self.sender_counts.entry(sender).or_default() += 1;

        Ok(())
    }

    /// Get top N transactions by priority (highest fee first, then FIFO).
    pub fn get_top_transactions(&mut self, count: usize) -> Vec<Transaction> {
        let mut result = Vec::new();
        let mut temp = Vec::new();

        while result.len() < count {
            if let Some(ptx) = self.queue.pop() {
                result.push(ptx.transaction.clone());
                temp.push(ptx);
            } else {
                break;
            }
        }

        // Put them back
        for ptx in temp {
            self.queue.push(ptx);
        }

        result
    }

    /// Remove transaction from mempool (after inclusion in block)
    pub fn remove_transaction(&mut self, tx_hash: &Hash) {
        if let Some(tx) = self.transactions.remove(tx_hash) {
            // Decrement sender count
            let sender = tx.sender();
            if let Some(count) = self.sender_counts.get_mut(&sender) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.sender_counts.remove(&sender);
                }
            }
            // Rebuild queue without the removed transaction
            let items: Vec<_> = self.queue.drain().collect();
            self.queue = items
                .into_iter()
                .filter(|ptx| &ptx.hash != tx_hash)
                .collect();
        }
    }

    /// PERF-FIX 9: Bulk remove transactions from mempool after block inclusion.
    /// Rebuilds heap only ONCE instead of per-transaction (O(n) instead of O(n*m)).
    pub fn remove_transactions_bulk(&mut self, tx_hashes: &[Hash]) {
        let hash_set: std::collections::HashSet<&Hash> = tx_hashes.iter().collect();
        let mut any_removed = false;
        for h in tx_hashes {
            if let Some(tx) = self.transactions.remove(h) {
                let sender = tx.sender();
                if let Some(count) = self.sender_counts.get_mut(&sender) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        self.sender_counts.remove(&sender);
                    }
                }
                any_removed = true;
            }
        }
        if any_removed {
            let items: Vec<_> = self.queue.drain().collect();
            self.queue = items
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

        let transactions: Vec<_> = self.queue.drain().collect();
        let (valid, expired): (Vec<_>, Vec<_>) = transactions
            .into_iter()
            .partition(|ptx| now.saturating_sub(ptx.timestamp) < self.expiration_time);
        for ptx in &expired {
            if let Some(tx) = self.transactions.remove(&ptx.hash) {
                let sender = tx.sender();
                if let Some(count) = self.sender_counts.get_mut(&sender) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        self.sender_counts.remove(&sender);
                    }
                }
            }
        }
        self.queue = valid.into_iter().collect();
    }

    /// Prune transactions whose recent_blockhash is no longer in the valid set.
    /// Returns the number of evicted transactions.
    pub fn prune_stale_blockhashes(
        &mut self,
        valid_blockhashes: &std::collections::HashSet<Hash>,
    ) -> usize {
        let before = self.transactions.len();

        let items: Vec<_> = self.queue.drain().collect();
        let (valid, stale): (Vec<_>, Vec<_>) = items
            .into_iter()
            .partition(|ptx| valid_blockhashes.contains(&ptx.transaction.message.recent_blockhash));
        for ptx in &stale {
            if let Some(tx) = self.transactions.remove(&ptx.hash) {
                let sender = tx.sender();
                if let Some(count) = self.sender_counts.get_mut(&sender) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        self.sender_counts.remove(&sender);
                    }
                }
            }
        }
        self.queue = valid.into_iter().collect();

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

    /// P3-3: Retrieve a transaction by hash (for compact block reconstruction).
    pub fn get(&self, tx_hash: &Hash) -> Option<&Transaction> {
        self.transactions.get(tx_hash)
    }

    /// P3-3: Return all transactions in the mempool (for compact block reconstruction).
    pub fn all_transactions(&self) -> Vec<Transaction> {
        self.transactions.values().cloned().collect()
    }

    /// Clear mempool
    pub fn clear(&mut self) {
        self.queue.clear();
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
            tx_type: Default::default(),
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
    fn test_mempool_fee_only_ordering() {
        // M-8: Same fee but different reputation — should NOT affect order.
        // With fee-only ordering, same-fee txs are FIFO.
        let mut mempool = Mempool::new(100, 300);

        let tx_low_rep = create_test_transaction(1);
        let tx_high_rep = create_test_transaction(2);

        // Both have fee=1000 — high reputation should NOT get priority
        mempool
            .add_transaction(tx_low_rep.clone(), 1000, 0)
            .unwrap();
        mempool.add_transaction(tx_high_rep, 1000, 10_000).unwrap();

        let top = mempool.get_top_transactions(1);
        assert_eq!(top.len(), 1);
        // First submitted tx wins (FIFO tiebreak)
        assert_eq!(top[0].hash(), tx_low_rep.hash());
    }

    #[test]
    fn test_mempool_no_express_lane() {
        // M-8: High-reputation agent with low fee should NOT beat high-fee regular tx
        let mut mempool = Mempool::new(100, 300);

        let tx_high_fee = create_test_transaction(1);
        let tx_high_rep = create_test_transaction(2);

        // High fee, no reputation
        mempool
            .add_transaction(tx_high_fee.clone(), 100_000, 0)
            .unwrap();
        // Low fee, Tier 5 reputation — should NOT get express lane priority
        mempool.add_transaction(tx_high_rep, 100, 10_000).unwrap();

        let top = mempool.get_top_transactions(1);
        assert_eq!(top.len(), 1);
        // Highest fee wins, reputation irrelevant
        assert_eq!(top[0].hash(), tx_high_fee.hash());
    }

    #[test]
    fn test_mempool_strict_fee_ordering() {
        // Verify ordering is strictly by fee, regardless of reputation
        let mut mempool = Mempool::new(100, 300);

        let tx1 = create_test_transaction(1);
        let tx2 = create_test_transaction(2);
        let tx3 = create_test_transaction(3);

        // Low fee + max reputation
        mempool.add_transaction(tx1, 100, 50_000).unwrap();
        // Medium fee + zero reputation
        mempool.add_transaction(tx2, 500, 0).unwrap();
        // High fee + zero reputation
        mempool.add_transaction(tx3.clone(), 1000, 0).unwrap();

        let top = mempool.get_top_transactions(3);
        assert_eq!(top.len(), 3);
        // Strict fee order: 1000, 500, 100
        assert_eq!(top[0].hash(), tx3.hash());
    }

    // ── P3-3: Compact block helpers ──

    #[test]
    fn test_mempool_get() {
        let mut pool = Mempool::new(100, 60);
        let tx = create_test_transaction(1);
        let tx_hash = tx.hash();
        pool.add_transaction(tx.clone(), 1000, 0).unwrap();
        assert!(pool.get(&tx_hash).is_some());
        assert_eq!(pool.get(&tx_hash).unwrap().hash(), tx_hash);
        assert!(pool.get(&Hash([0xFF; 32])).is_none());
    }

    #[test]
    fn test_mempool_all_transactions() {
        let mut pool = Mempool::new(100, 60);
        assert!(pool.all_transactions().is_empty());
        let tx1 = create_test_transaction(1);
        let tx2 = create_test_transaction(2);
        pool.add_transaction(tx1, 1000, 0).unwrap();
        pool.add_transaction(tx2, 2000, 0).unwrap();
        let all = pool.all_transactions();
        assert_eq!(all.len(), 2);
    }
}
