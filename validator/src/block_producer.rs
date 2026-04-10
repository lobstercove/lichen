// Lichen Block Producer
//
// Extracts transactions from the mempool, processes them, and constructs
// a signed Block ready for inclusion in a BFT proposal. The block is NOT
// yet stored or broadcast — that's the consensus engine's responsibility.

use lichen_core::{Block, FeeConfig, Hash, Mempool, Pubkey, StateStore, TxProcessor};
use tracing::{debug, info};

/// Compute the minimum delay (in milliseconds) the proposer should wait
/// after committing before building the next block, so that wall-clock
/// time has advanced past `parent_timestamp + 1`.
///
/// Block timestamps are second-precision and must be strictly increasing
/// (`proposed_ts > parent_ts`). If a proposer completes a round faster
/// than one second (e.g. solo BFT or low-latency 3-of-3 commit), the
/// `parent_timestamp + 1` floor in `build_block` would push the next
/// block's timestamp ahead of wall clock, causing cumulative drift.
/// Joining validators reject blocks whose timestamps exceed wall clock
/// by more than 120 s.
///
/// This function returns:
///   `max(base_delay_ms, millis_until_wall_clock ≥ parent_ts + 1) + 50`
///
/// The +50 ms pad absorbs timer jitter so we never wake up 1 ms early.
pub fn wall_clock_safe_delay(state: &StateStore, parent_hash: &Hash, base_delay_ms: u64) -> u64 {
    let parent_ts = resolve_parent_timestamp(state, parent_hash).unwrap_or(0);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let target_ms = (parent_ts + 1) * 1000; // next block needs ts ≥ parent+1
    let catch_up_ms = target_ms.saturating_sub(now_ms);
    base_delay_ms.max(catch_up_ms).saturating_add(50)
}

fn resolve_parent_timestamp(state: &StateStore, parent_hash: &Hash) -> Option<u64> {
    let parent_slot = state.get_last_slot().ok()?;
    let parent_block = if parent_slot == 0 {
        state.get_block_by_slot(0).ok().flatten()
    } else {
        match state.get_block_by_slot(parent_slot).ok().flatten() {
            Some(block) if block.hash() == *parent_hash => Some(block),
            _ => None,
        }
    }?;

    Some(parent_block.header.timestamp)
}

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
    state: &StateStore,
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

    // Fee config for computing burn/treasury split when reversing failed fees
    let fee_config = state
        .get_fee_config()
        .unwrap_or_else(|_| FeeConfig::default_from_constants());

    // Keep only successful TXs; track ALL processed hashes (success + fail)
    // so we can remove failed TXs from mempool immediately.
    let mut transactions = Vec::with_capacity(pending_count);
    let mut tx_fees_paid = Vec::with_capacity(pending_count);
    let mut processed_hashes = Vec::with_capacity(pending_count);
    let mut failed_hashes = Vec::new();
    // RC8: Collect info needed to reverse fee charges for failed TXs
    let mut failed_fee_reversals: Vec<(Pubkey, u64, u64)> = Vec::new(); // (payer, fee_paid, to_treasury)

    for (tx, result) in pending.into_iter().zip(results) {
        let tx_hash = tx.hash();
        if result.success {
            tx_fees_paid.push(result.fee_paid);
            transactions.push(tx);
        } else {
            // RC8: If a fee was charged, record the info needed to reverse it.
            // charge_fee_with_priority persists outside the WriteBatch (M4 anti-DoS),
            // but failed TXs aren't in the block so verifiers never charge these fees.
            // Without reversal, the proposer's accounts_root diverges from verifiers'.
            if result.fee_paid > 0 {
                if let Some(ix) = tx.message.instructions.first() {
                    if let Some(&fee_payer) = ix.accounts.first() {
                        let priority_fee = TxProcessor::compute_priority_fee(&tx);
                        let total_fee = TxProcessor::compute_base_fee(&tx, &fee_config)
                            .saturating_add(priority_fee);
                        let base_portion = total_fee.saturating_sub(priority_fee);
                        let base_burn = (base_portion as u128 * fee_config.fee_burn_percent as u128
                            / 100) as u64;
                        let priority_burn = priority_fee / 2;
                        let burn_amount = base_burn.saturating_add(priority_burn);
                        let to_treasury = total_fee.saturating_sub(burn_amount);
                        failed_fee_reversals.push((fee_payer, result.fee_paid, to_treasury));
                    }
                }
            }
            let error_detail = result.error.as_deref().unwrap_or("unknown error");
            if result.contract_logs.is_empty() {
                info!(
                    "❌ TX {} failed at height {}: error={}, return_code={:?}",
                    tx_hash.to_hex(),
                    height,
                    error_detail,
                    result.return_code
                );
            } else {
                info!(
                    "❌ TX {} failed at height {}: error={}, return_code={:?}, logs={:?}",
                    tx_hash.to_hex(),
                    height,
                    error_detail,
                    result.return_code,
                    result.contract_logs
                );
            }
            failed_hashes.push(tx_hash);
        }
        processed_hashes.push(tx_hash);
    }

    // RC8 FIX: Reverse fee charges for failed TXs that won't be in the block.
    //
    // process_transaction_inner charges fees BEFORE begin_batch (M4 anti-DoS):
    //   charge_fee_with_priority → atomic_put_accounts (payer debit + treasury credit + burn)
    // For failed TXs, rollback_batch only undoes instruction effects — the fee
    // persists.  Since failed TXs are excluded from the block, verifiers never
    // replay them and never charge those fees.  This causes accounts_root
    // divergence between proposer and all verifiers.
    //
    // Fix: credit fee_paid back to payer, debit treasury portion from treasury.
    // The burn counter (CF_STATS) is NOT part of the state root so we skip it.
    if !failed_fee_reversals.is_empty() {
        info!(
            "🔄 RC8: Reversing {} failed-tx fee charge(s) at height {} to prevent state root divergence",
            failed_fee_reversals.len(),
            height,
        );
        let treasury_pk = state.get_treasury_pubkey().ok().flatten();
        for (fee_payer, fee_paid, to_treasury) in &failed_fee_reversals {
            // Credit fee_paid back to payer
            if let Ok(Some(mut payer_account)) = state.get_account(fee_payer) {
                if payer_account.add_spendable(*fee_paid).is_ok() {
                    if let Err(e) = state.put_account(fee_payer, &payer_account) {
                        tracing::error!("Failed to reverse fee for payer: {e}");
                    }
                }
            }
            // Debit treasury's portion (everything except burned amount)
            if let Some(ref tpk) = treasury_pk {
                if *to_treasury > 0 {
                    if let Ok(Some(mut treasury_account)) = state.get_account(tpk) {
                        if treasury_account.deduct_spendable(*to_treasury).is_ok() {
                            if let Err(e) = state.put_account(tpk, &treasury_account) {
                                tracing::error!("Failed to debit treasury fee reversal: {e}");
                            }
                        }
                    }
                }
            }
        }
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
    let min_block_timestamp = resolve_parent_timestamp(state, &parent_hash)
        .map(|timestamp| timestamp.saturating_add(1))
        .unwrap_or(0);

    // Use BFT timestamp (weighted median of parent commit) if available,
    // falling back to wall clock for genesis or solo validator scenarios.
    let block_timestamp = bft_timestamp
        .unwrap_or(wall_clock_timestamp)
        .max(min_block_timestamp);

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn build_block_fallback_timestamp_advances_past_parent() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();
        let validator = Pubkey([7u8; 32]);
        let processor = TxProcessor::new(state.clone());
        let mut mempool = Mempool::new(100, 300);

        let wall_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let parent_timestamp = wall_now.saturating_add(10);
        let parent = Block::genesis(Hash::hash(b"parent-state"), parent_timestamp, Vec::new());
        let parent_hash = parent.hash();
        state.put_block(&parent).unwrap();

        let (block, processed) = build_block(
            &state,
            &mut mempool,
            &processor,
            1,
            parent_hash,
            &validator,
            Vec::new(),
            None,
        );

        assert!(processed.is_empty());
        assert_eq!(block.header.timestamp, parent_timestamp.saturating_add(1));
    }

    #[test]
    fn wall_clock_safe_delay_pads_when_parent_timestamp_ahead() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // Create a parent block whose timestamp is at wall clock (i.e. now)
        let wall_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let parent = Block::genesis(Hash::hash(b"safe-delay"), wall_now, Vec::new());
        let parent_hash = parent.hash();
        state.put_block(&parent).unwrap();

        // With parent at wall_now, next block needs ts >= wall_now+1,
        // so we need to wait ~1000ms from now. base_delay of 400ms
        // should be overridden by the catch-up.
        let delay = wall_clock_safe_delay(&state, &parent_hash, 400);
        // Must be at least 400ms (base), and should include catch-up + 50ms pad
        assert!(delay >= 400, "delay {} should be >= base 400", delay);
        // Should not be absurdly large (parent is at now, not far in the future)
        assert!(delay <= 1200, "delay {} should be <= 1200ms", delay);
    }

    #[test]
    fn wall_clock_safe_delay_returns_base_when_clock_already_ahead() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // Create a parent block whose timestamp is 5s in the past
        let wall_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let parent = Block::genesis(Hash::hash(b"old-parent"), wall_now - 5, Vec::new());
        let parent_hash = parent.hash();
        state.put_block(&parent).unwrap();

        // Wall clock is already past parent+1, so base_delay dominates
        let delay = wall_clock_safe_delay(&state, &parent_hash, 800);
        // Should be base (800) + 50 pad = 850
        assert_eq!(delay, 850);
    }
}
