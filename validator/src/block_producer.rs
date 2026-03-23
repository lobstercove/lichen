// Lichen Block Producer
//
// Extracts transactions from the mempool, processes them, and constructs
// a signed Block ready for inclusion in a BFT proposal. The block is NOT
// yet stored or broadcast — that's the consensus engine's responsibility.

use lichen_core::{Block, Hash, Mempool, Pubkey, StateStore, TxProcessor};
use tracing::{debug, info};

/// Build a new block from pending mempool transactions.
///
/// `bft_timestamp`: If `Some`, use this BFT-derived timestamp (weighted
/// median of the parent block's commit vote timestamps). Falls back to
/// wall-clock time if `None` (genesis, solo validator, or no parent commit).
///
/// Returns `(block, processed_tx_hashes)`:
///   - `block` has `state_root = Hash::default()` — the caller MUST compute
///     and set it after applying block effects.
///   - `processed_tx_hashes` contains the hashes of transactions included in
///     the block, for mempool cleanup.
///
/// This function does NOT:
///   - Store the block to state
///   - Apply block effects (rewards, staking, oracle)
///   - Broadcast the block
///   - Sign the block (caller signs after setting state_root)
#[allow(clippy::too_many_arguments)]
pub fn build_block(
    _state: &StateStore,
    mempool: &mut Mempool,
    processor: &TxProcessor,
    height: u64,
    parent_hash: Hash,
    validator_pubkey: &Pubkey,
    oracle_prices: Vec<(String, u64)>,
    bft_timestamp: Option<u64>,
) -> (Block, Vec<Hash>) {
    // Collect pending transactions (up to 2000)
    let pending = mempool.get_top_transactions(2000);
    let pending_count = pending.len();

    // Process in parallel (non-conflicting TXs run simultaneously)
    let results = processor.process_transactions_parallel(&pending, validator_pubkey);

    // Keep only successful TXs; track ALL processed hashes (success + fail)
    // so we can remove failed TXs from mempool immediately.
    let mut transactions = Vec::with_capacity(pending_count);
    let mut tx_fees_paid = Vec::with_capacity(pending_count);
    let mut processed_hashes = Vec::with_capacity(pending_count);
    let mut failed_hashes = Vec::new();

    for (tx, result) in pending.into_iter().zip(results) {
        let tx_hash = tx.hash();
        if result.success {
            tx_fees_paid.push(result.fee_paid);
            transactions.push(tx);
        } else {
            failed_hashes.push(tx_hash);
        }
        processed_hashes.push(tx_hash);
    }

    // Immediately remove failed TXs from mempool so they aren't
    // reprocessed in subsequent blocks (their state effects like fee
    // charges persist from process_transactions_parallel).
    if !failed_hashes.is_empty() {
        info!(
            "🧹 Removing {} failed tx(s) from mempool at height {}",
            failed_hashes.len(),
            height
        );
        mempool.remove_transactions_bulk(&failed_hashes);
    }

    let wall_clock_timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Use BFT timestamp (weighted median of parent commit) if available,
    // falling back to wall clock for genesis or solo validator scenarios.
    let block_timestamp = bft_timestamp.unwrap_or(wall_clock_timestamp);

    let mut block = Block::new_with_timestamp(
        height,
        parent_hash,
        Hash::default(), // Placeholder — caller sets after effects
        validator_pubkey.0,
        transactions,
        block_timestamp,
    );
    block.tx_fees_paid = tx_fees_paid;
    block.oracle_prices = oracle_prices;

    if block.transactions.is_empty() {
        debug!("📦 Built empty block (heartbeat) at height {}", height);
    } else {
        info!(
            "📦 Built block at height {} with {} txs",
            height,
            block.transactions.len()
        );
    }

    (block, processed_hashes)
}
