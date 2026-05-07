// Lichen Block Producer
//
// Extracts transactions from the mempool, processes them, and constructs
// a signed Block ready for inclusion in a BFT proposal. The block is NOT
// yet stored or broadcast — that's the consensus engine's responsibility.

use lichen_core::{Block, FeeConfig, Hash, Mempool, Pubkey, StateStore, TxProcessor};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::{debug, info, warn};

/// Compute the minimum delay (in milliseconds) the proposer should wait
/// after committing before building the next block, so that wall-clock
/// time has not fallen behind the parent timestamp.
///
/// Block timestamps are second-precision while testnet targets sub-second
/// slots. Consensus therefore requires nondecreasing timestamps
/// (`proposed_ts >= parent_ts`), not a one-second increase per block. If
/// the parent timestamp is already in the future, wait for wall clock to
/// catch up enough that the next proposal does not add more future drift.
///
/// This function returns:
///   `max(base_delay_ms, millis_until_wall_clock ≥ parent_ts) + 50`
///
/// The +50 ms pad absorbs timer jitter so we never wake up 1 ms early.
pub fn wall_clock_safe_delay(state: &StateStore, parent_hash: &Hash, base_delay_ms: u64) -> u64 {
    let parent_ts = resolve_parent_timestamp(state, parent_hash).unwrap_or(0);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let target_ms = parent_ts * 1000; // next block may reuse the parent second.
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

fn proposal_staging_dir(staging_root: &Path, height: u64) -> PathBuf {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    staging_root.join(format!(
        "lichen-proposal-stage-{}-{}-{}",
        std::process::id(),
        height,
        nonce
    ))
}

fn open_proposal_staging_state(
    state: &StateStore,
    staging_root: &Path,
    height: u64,
) -> Result<(StateStore, PathBuf), String> {
    std::fs::create_dir_all(staging_root)
        .map_err(|e| format!("failed to create proposal staging root: {e}"))?;
    let staging_dir = proposal_staging_dir(staging_root, height);
    let staging_path = staging_dir
        .to_str()
        .ok_or_else(|| "proposal staging path is not valid UTF-8".to_string())?;
    state.create_raw_checkpoint(staging_path)?;
    let staging_state = StateStore::open_checkpoint(staging_path)?;
    Ok((staging_state, staging_dir))
}

/// Build a new block from pending mempool transactions.
///
/// `bft_timestamp`: If `Some`, use this BFT-derived timestamp (weighted
/// median of the parent block's commit vote timestamps). Falls back to
/// wall-clock time if `None` (genesis, solo validator, or no parent commit).
///
/// Returns `(block, processed_tx_hashes)`:
///   - `block` has a state root computed from speculative proposal execution.
///   - `processed_tx_hashes` contains the hashes of transactions included in
///     the block, for mempool cleanup.
///
/// This function does NOT:
///   - Store the block to state
///   - Mutate canonical state while evaluating mempool transactions
///   - Apply block effects (rewards, staking, oracle)
///   - Broadcast the block
///   - Sign the block (caller signs the returned proposal block)
#[allow(clippy::too_many_arguments)]
pub fn build_block(
    state: &StateStore,
    mempool: &mut Mempool,
    _processor: &TxProcessor,
    staging_root: &Path,
    height: u64,
    parent_hash: Hash,
    validator_pubkey: &Pubkey,
    oracle_prices: Vec<(String, u64)>,
    max_transactions: usize,
    bft_timestamp: Option<u64>,
) -> (Block, Vec<Hash>) {
    let build_started = Instant::now();

    // Collect pending transactions (up to 2000).  Drop transactions that the
    // local ledger has already committed before they reach the processor; they
    // can arrive late through RPC retries or P2P relay after block inclusion.
    let collect_started = Instant::now();
    let mut stale_hashes = Vec::new();
    let tx_limit = max_transactions.min(2000);
    let mut pending: Vec<_> = if tx_limit == 0 {
        Vec::new()
    } else {
        mempool
            .get_top_transactions(tx_limit)
            .into_iter()
            .filter(|tx| {
                let tx_hash = tx.hash();
                match state.get_transaction(&tx_hash) {
                    Ok(Some(_)) => {
                        stale_hashes.push(tx_hash);
                        false
                    }
                    _ => true,
                }
            })
            .collect()
    };
    let collect_ms = collect_started.elapsed().as_millis();
    if !stale_hashes.is_empty() {
        debug!(
            "🧹 Dropping {} already-committed tx(s) from mempool before height {}",
            stale_hashes.len(),
            height
        );
        mempool.remove_transactions_bulk(&stale_hashes);
    }
    let pending_count = pending.len();

    // Proposal execution must be speculative until BFT commits the block.  Running
    // mempool transactions on the live DB lets losing proposals leave durable tx
    // effects behind, which later makes committed block replay diverge.
    let mut staging_dir: Option<PathBuf> = None;
    let mut staging_state: Option<StateStore> = None;
    let mut results = Vec::new();
    let mut staging_open_ms = 0;
    let mut execution_ms = 0;
    if !pending.is_empty() {
        let staging_started = Instant::now();
        match open_proposal_staging_state(state, staging_root, height) {
            Ok((stage, dir)) => {
                staging_open_ms = staging_started.elapsed().as_millis();
                let staging_processor = TxProcessor::new(stage.clone());
                let execution_started = Instant::now();
                results =
                    staging_processor.process_transactions_parallel(&pending, validator_pubkey);
                execution_ms = execution_started.elapsed().as_millis();
                staging_state = Some(stage);
                staging_dir = Some(dir);
            }
            Err(e) => {
                warn!(
                    "⚠️  Proposal staging unavailable at height {}: {}. Building empty liveness block.",
                    height, e
                );
                pending.clear();
            }
        }
    }

    // Fee config for computing burn/treasury split when reversing failed fees
    let execution_state = staging_state.as_ref().unwrap_or(state);
    let fee_config = execution_state
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
        let treasury_pk = execution_state.get_treasury_pubkey().ok().flatten();
        for (fee_payer, fee_paid, to_treasury) in &failed_fee_reversals {
            // Credit fee_paid back to payer
            if let Ok(Some(mut payer_account)) = execution_state.get_account(fee_payer) {
                if payer_account.add_spendable(*fee_paid).is_ok() {
                    if let Err(e) = execution_state.put_account(fee_payer, &payer_account) {
                        tracing::error!("Failed to reverse fee for payer: {e}");
                    }
                }
            }
            // Debit treasury's portion (everything except burned amount)
            if let Some(ref tpk) = treasury_pk {
                if *to_treasury > 0 {
                    if let Ok(Some(mut treasury_account)) = execution_state.get_account(tpk) {
                        if treasury_account.deduct_spendable(*to_treasury).is_ok() {
                            if let Err(e) = execution_state.put_account(tpk, &treasury_account) {
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
    let min_block_timestamp = resolve_parent_timestamp(state, &parent_hash).unwrap_or(0);

    // Use BFT timestamp (weighted median of parent commit) if available,
    // falling back to wall clock for genesis or solo validator scenarios.
    let block_timestamp = bft_timestamp
        .unwrap_or(wall_clock_timestamp)
        .max(min_block_timestamp);

    let root_started = Instant::now();
    let proposal_state_root = execution_state.compute_state_root();
    let root_ms = root_started.elapsed().as_millis();

    let mut block = Block::new_with_timestamp(
        height,
        parent_hash,
        proposal_state_root,
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
            "📦 Built block at height {} with {} txs (total_ms={} collect_ms={} staging_ms={} exec_ms={} root_ms={})",
            height,
            block.transactions.len(),
            build_started.elapsed().as_millis(),
            collect_ms,
            staging_open_ms,
            execution_ms,
            root_ms,
        );
    }

    drop(staging_state);
    if let Some(dir) = staging_dir {
        if let Err(e) = std::fs::remove_dir_all(&dir) {
            warn!("⚠️  Failed to remove proposal staging dir {:?}: {}", dir, e);
        }
    }

    (block, processed_hashes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lichen_core::{Account, Instruction, Keypair, Message, Transaction, SYSTEM_PROGRAM_ID};
    use tempfile::tempdir;

    fn signed_transfer(
        from_kp: &Keypair,
        from: Pubkey,
        to: Pubkey,
        amount_licn: u64,
        recent_blockhash: Hash,
    ) -> Transaction {
        let mut data = vec![0u8];
        data.extend_from_slice(&Account::licn_to_spores(amount_licn).to_le_bytes());
        let ix = Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![from, to],
            data,
        };
        let message = Message::new(vec![ix], recent_blockhash);
        let mut tx = Transaction::new(message);
        tx.signatures.push(from_kp.sign(&tx.message.serialize()));
        tx
    }

    #[test]
    fn build_block_fallback_timestamp_is_nondecreasing_from_parent() {
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
            temp.path(),
            1,
            parent_hash,
            &validator,
            Vec::new(),
            2000,
            None,
        );

        assert!(processed.is_empty());
        assert_eq!(block.header.timestamp, parent_timestamp);
    }

    #[test]
    fn wall_clock_safe_delay_pads_when_parent_timestamp_is_in_future() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // Create a parent block whose timestamp is ahead of wall clock.
        let wall_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let parent = Block::genesis(Hash::hash(b"safe-delay"), wall_now + 1, Vec::new());
        let parent_hash = parent.hash();
        state.put_block(&parent).unwrap();

        // With the parent one second ahead, base_delay of 400ms should be
        // overridden by the catch-up.
        let delay = wall_clock_safe_delay(&state, &parent_hash, 400);
        // Must be at least 400ms (base), and should include catch-up + 50ms pad
        assert!(delay >= 400, "delay {} should be >= base 400", delay);
        // Should not be absurdly large (parent is only one second ahead)
        assert!(delay <= 1200, "delay {} should be <= 1200ms", delay);
    }

    #[test]
    fn wall_clock_safe_delay_allows_same_second_parent_timestamp() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let wall_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let parent = Block::genesis(Hash::hash(b"same-second-parent"), wall_now, Vec::new());
        let parent_hash = parent.hash();
        state.put_block(&parent).unwrap();

        let delay = wall_clock_safe_delay(&state, &parent_hash, 400);
        assert_eq!(delay, 450);
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

        // Wall clock is already past the parent timestamp, so base_delay dominates.
        let delay = wall_clock_safe_delay(&state, &parent_hash, 800);
        // Should be base (800) + 50 pad = 850
        assert_eq!(delay, 850);
    }

    #[test]
    fn build_block_drops_already_committed_transactions_without_reprocessing() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();
        let validator = Pubkey([7u8; 32]);
        let processor = TxProcessor::new(state.clone());
        let mut mempool = Mempool::new(100, 300);
        let parent = Block::genesis(Hash::hash(b"parent-state"), 1, Vec::new());
        let parent_hash = parent.hash();
        state.put_block(&parent).unwrap();

        let tx = lichen_core::Transaction::new(lichen_core::Message::new(
            vec![lichen_core::Instruction {
                program_id: Pubkey([1u8; 32]),
                accounts: vec![Pubkey([2u8; 32])],
                data: vec![1],
            }],
            parent_hash,
        ));
        let tx_hash = tx.hash();
        state.put_transaction(&tx).unwrap();
        mempool.add_transaction(tx, 1, 0).unwrap();

        let (block, processed) = build_block(
            &state,
            &mut mempool,
            &processor,
            temp.path(),
            1,
            parent_hash,
            &validator,
            Vec::new(),
            2000,
            None,
        );

        assert!(block.transactions.is_empty());
        assert!(processed.is_empty());
        assert!(!mempool.contains(&tx_hash));
    }

    #[test]
    fn build_block_can_emit_heartbeat_without_draining_pending_transactions() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();
        let validator = Pubkey([7u8; 32]);
        let processor = TxProcessor::new(state.clone());
        let mut mempool = Mempool::new(100, 300);

        let alice_kp = Keypair::generate();
        let alice = alice_kp.pubkey();
        let bob = Pubkey([9u8; 32]);
        let treasury = Pubkey([3u8; 32]);
        state.set_treasury_pubkey(&treasury).unwrap();
        state
            .put_account(&treasury, &Account::new(0, treasury))
            .unwrap();
        state
            .put_account(&alice, &Account::new(1000, alice))
            .unwrap();

        let parent = Block::genesis(Hash::hash(b"parent-state"), 1, Vec::new());
        let parent_hash = parent.hash();
        state.put_block(&parent).unwrap();
        state.set_last_slot(0).unwrap();

        let tx = signed_transfer(&alice_kp, alice, bob, 10, parent_hash);
        let tx_hash = tx.hash();
        mempool.add_transaction(tx, 1, 0).unwrap();

        let (block, processed) = build_block(
            &state,
            &mut mempool,
            &processor,
            temp.path(),
            1,
            parent_hash,
            &validator,
            Vec::new(),
            0,
            None,
        );

        assert!(block.transactions.is_empty());
        assert!(processed.is_empty());
        assert!(mempool.contains(&tx_hash));
        assert!(state.get_transaction(&tx_hash).unwrap().is_none());
    }

    #[test]
    fn build_block_executes_transactions_on_staging_state_only() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();
        let validator = Pubkey([7u8; 32]);
        let processor = TxProcessor::new(state.clone());
        let mut mempool = Mempool::new(100, 300);

        let alice_kp = Keypair::generate();
        let alice = alice_kp.pubkey();
        let bob = Pubkey([9u8; 32]);
        let treasury = Pubkey([3u8; 32]);
        state.set_treasury_pubkey(&treasury).unwrap();
        state
            .put_account(&treasury, &Account::new(0, treasury))
            .unwrap();
        state
            .put_account(&alice, &Account::new(1000, alice))
            .unwrap();

        let parent = Block::genesis(Hash::hash(b"parent-state"), 1, Vec::new());
        let parent_hash = parent.hash();
        state.put_block(&parent).unwrap();
        state.set_last_slot(0).unwrap();

        let root_before = state.compute_state_root_cold_start();
        let tx = signed_transfer(&alice_kp, alice, bob, 10, parent_hash);
        let tx_hash = tx.hash();
        mempool.add_transaction(tx, 1, 0).unwrap();

        let (block, processed) = build_block(
            &state,
            &mut mempool,
            &processor,
            temp.path(),
            1,
            parent_hash,
            &validator,
            Vec::new(),
            2000,
            None,
        );

        assert_eq!(block.transactions.len(), 1);
        assert_eq!(processed, vec![tx_hash]);
        assert_eq!(state.compute_state_root_cold_start(), root_before);
        assert!(state.get_transaction(&tx_hash).unwrap().is_none());
        assert_eq!(state.get_balance(&bob).unwrap_or(0), 0);
    }
}
