// Lichen Block Producer
//
// Extracts transactions from the mempool, processes them, and constructs
// a signed Block ready for inclusion in a BFT proposal. The block is NOT
// yet stored or broadcast — that's the consensus engine's responsibility.

use lichen_core::{
    Block, CanonicalCommitCertificate, Hash, Mempool, Pubkey, StateBatch, StateStore, TxProcessor,
};
use std::path::Path;
use std::time::Instant;
use tracing::{debug, info};

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
///   `max(base_delay_ms, millis_until_wall_clock >= parent_ts)`
pub fn wall_clock_safe_delay(state: &StateStore, parent_hash: &Hash, base_delay_ms: u64) -> u64 {
    let parent_ts = resolve_parent_timestamp(state, parent_hash).unwrap_or(0);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let target_ms = parent_ts * 1000; // next block may reuse the parent second.
    let catch_up_ms = target_ms.saturating_sub(now_ms);
    base_delay_ms.max(catch_up_ms)
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
    _staging_root: &Path,
    height: u64,
    parent_hash: Hash,
    validator_pubkey: &Pubkey,
    oracle_prices: Vec<(String, u64)>,
    max_transactions: usize,
    bft_timestamp: Option<u64>,
) -> Result<(Block, Vec<Hash>), String> {
    let build_started = Instant::now();

    // Collect pending transactions (up to 2000).  Drop transactions that the
    // local ledger has already committed before they reach the processor; they
    // can arrive late through RPC retries or P2P relay after block inclusion.
    let collect_started = Instant::now();
    let mut stale_hashes = Vec::new();
    let tx_limit = max_transactions.min(2000);
    let pending: Vec<_> = if tx_limit == 0 {
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

    // Proposal execution must be speculative until BFT commits the block.
    // Execute against an in-memory StateBatch overlay; canonical RocksDB is
    // replayed exactly once at the commit boundary.
    let mut results = Vec::new();
    let mut proposal_batch: Option<StateBatch> = None;
    let mut spec_ms = 0;
    let mut execution_ms = 0;
    if !pending.is_empty() {
        let spec_started = Instant::now();
        let staging_processor = TxProcessor::new_speculative(state.clone());
        let execution_started = Instant::now();
        let speculative = staging_processor.process_transactions_speculative_at_slot(
            &pending,
            validator_pubkey,
            height,
        );
        execution_ms = execution_started.elapsed().as_millis();
        results = speculative.results;
        proposal_batch = Some(speculative.batch);
        spec_ms = spec_started.elapsed().as_millis();
    }

    // Deterministic execution failures are canonical transactions with durable
    // receipts. Only structurally/cryptographically invalid transactions are
    // excluded and removed before proposal construction.
    let mut transactions = Vec::with_capacity(pending_count);
    let mut tx_fees_paid = Vec::with_capacity(pending_count);
    let mut processed_hashes = Vec::with_capacity(pending_count);
    let mut invalid_hashes = Vec::new();

    for (tx, result) in pending.into_iter().zip(results) {
        let tx_hash = tx.hash();
        if result.receipt_eligible {
            tx_fees_paid.push(result.fee_paid);
            transactions.push(tx);
        }
        if !result.success {
            let error_detail = result.error.as_deref().unwrap_or("unknown error");
            if result.contract_logs.is_empty() {
                info!(
                    "❌ TX {} {} at height {}: error={}, return_code={:?}",
                    tx_hash.to_hex(),
                    if result.receipt_eligible {
                        "recorded a failed receipt"
                    } else {
                        "was rejected"
                    },
                    height,
                    error_detail,
                    result.return_code
                );
            } else {
                info!(
                    "❌ TX {} {} at height {}: error={}, return_code={:?}, logs={:?}",
                    tx_hash.to_hex(),
                    if result.receipt_eligible {
                        "recorded a failed receipt"
                    } else {
                        "was rejected"
                    },
                    height,
                    error_detail,
                    result.return_code,
                    result.contract_logs
                );
            }
            if !result.receipt_eligible {
                invalid_hashes.push(tx_hash);
            }
        }
        processed_hashes.push(tx_hash);
    }

    if !invalid_hashes.is_empty() {
        info!(
            "🧹 Removing {} invalid tx(s) from mempool at height {}",
            invalid_hashes.len(),
            height
        );
        mempool.remove_transactions_bulk(&invalid_hashes);
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
    let parent_post_state_root = state.compute_state_root();
    let proposal_state_root = match proposal_batch.as_ref() {
        Some(batch) if !transactions.is_empty() => state.compute_state_root_for_batch(batch),
        _ => state.compute_state_root(),
    };
    let root_ms = root_started.elapsed().as_millis();

    let user_transaction_count = transactions.len();
    if height > 1 {
        let parent = state
            .get_block(&parent_hash)?
            .ok_or_else(|| format!("parent block {} is unavailable", parent_hash))?;
        if parent.header.slot + 1 != height {
            return Err(format!(
                "parent slot {} does not precede proposal height {}",
                parent.header.slot, height
            ));
        }
        tx_fees_paid.insert(0, 0);
        let parent_powers = state
            .get_validator_power_snapshot(&parent.header.validators_hash)?
            .ok_or_else(|| {
                format!(
                    "validator power snapshot {} for parent block {} is unavailable",
                    parent.header.validators_hash, parent.header.slot
                )
            })?;
        let canonical_commit = CanonicalCommitCertificate::from_committed_block(
            &parent,
            parent_powers,
            parent_post_state_root,
        )?
        .bind_child_metadata(&tx_fees_paid, &oracle_prices)?;
        transactions.insert(0, canonical_commit.to_transaction()?);
    }

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

    if user_transaction_count == 0 {
        debug!("📦 Built empty liveness block at height {}", height);
    } else {
        info!(
            "📦 Built block at height {} with {} txs (total_ms={} collect_ms={} spec_ms={} exec_ms={} root_ms={})",
            height,
            user_transaction_count,
            build_started.elapsed().as_millis(),
            collect_ms,
            spec_ms,
            execution_ms,
            root_ms,
        );
    }

    Ok((block, processed_hashes))
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
        )
        .unwrap();

        assert!(processed.is_empty());
        assert_eq!(block.header.timestamp, parent_timestamp);
    }

    #[test]
    fn wall_clock_safe_delay_waits_when_parent_timestamp_is_in_future() {
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
        // Must be at least 400ms (base), and should include catch-up time.
        assert!(delay >= 400, "delay {} should be >= base 400", delay);
        // Should not be absurdly large (parent is only one second ahead)
        assert!(delay <= 1000, "delay {} should be <= 1000ms", delay);
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
        assert_eq!(delay, 400);
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
        assert_eq!(delay, 800);
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
        )
        .unwrap();

        assert!(block.transactions.is_empty());
        assert!(processed.is_empty());
        assert!(!mempool.contains(&tx_hash));
    }

    #[test]
    fn build_block_can_emit_liveness_block_without_draining_pending_transactions() {
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
        )
        .unwrap();

        assert!(block.transactions.is_empty());
        assert!(processed.is_empty());
        assert!(mempool.contains(&tx_hash));
        assert!(state.get_transaction(&tx_hash).unwrap().is_none());
    }

    #[test]
    fn build_block_executes_transactions_speculatively_only() {
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
        )
        .unwrap();

        assert_eq!(block.transactions.len(), 1);
        assert_eq!(processed, vec![tx_hash]);
        assert_eq!(state.compute_state_root_cold_start(), root_before);
        assert!(state.get_transaction(&tx_hash).unwrap().is_none());
        assert_eq!(state.get_balance(&bob).unwrap_or(0), 0);

        let replay_results =
            processor.process_transactions_parallel(&block.transactions, &validator);
        assert!(replay_results.iter().all(|result| result.success));
        assert_eq!(state.compute_state_root(), block.header.state_root);
    }

    #[test]
    fn build_block_speculative_batch_sees_prior_tx_writes() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();
        let validator = Pubkey([7u8; 32]);
        let processor = TxProcessor::new(state.clone());
        let mut mempool = Mempool::new(100, 300);

        let alice_kp = Keypair::generate();
        let alice = alice_kp.pubkey();
        let bob_kp = Keypair::generate();
        let bob = bob_kp.pubkey();
        let carol = Pubkey([10u8; 32]);
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

        let tx1 = signed_transfer(&alice_kp, alice, bob, 20, parent_hash);
        let tx2 = signed_transfer(&bob_kp, bob, carol, 5, parent_hash);
        mempool.add_transaction(tx1.clone(), 2, 0).unwrap();
        mempool.add_transaction(tx2.clone(), 1, 0).unwrap();

        let root_before = state.compute_state_root_cold_start();
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
        )
        .unwrap();

        assert_eq!(block.transactions.len(), 2);
        assert_eq!(processed, vec![tx1.hash(), tx2.hash()]);
        assert_eq!(state.compute_state_root_cold_start(), root_before);
        assert_eq!(state.get_balance(&bob).unwrap_or(0), 0);

        let replay_results =
            processor.process_transactions_parallel(&block.transactions, &validator);
        assert!(replay_results.iter().all(|result| result.success));
        assert_eq!(state.compute_state_root(), block.header.state_root);
        assert!(state.get_balance(&carol).unwrap_or(0) > 0);
    }

    #[test]
    fn build_block_includes_failed_execution_with_replayable_receipt() {
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
            .put_account(&alice, &Account::new(1_000, alice))
            .unwrap();

        let parent = Block::genesis(state.compute_state_root_cold_start(), 1, Vec::new());
        let parent_hash = parent.hash();
        state.put_block(&parent).unwrap();
        state.set_last_slot(0).unwrap();

        let tx = signed_transfer(&alice_kp, alice, bob, 2_000, parent_hash);
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
        )
        .unwrap();

        assert_eq!(block.transactions.len(), 1);
        assert_eq!(block.transactions[0].hash(), tx_hash);
        assert_eq!(block.tx_fees_paid.len(), 1);
        assert_eq!(processed, vec![tx_hash]);
        assert!(mempool.contains(&tx_hash));
        assert!(state.get_transaction(&tx_hash).unwrap().is_none());

        let replay = TxProcessor::new_speculative(state.clone())
            .process_transactions_speculative(&block.transactions, &validator);
        assert!(!replay.results[0].success);
        assert!(replay.results[0].receipt_eligible);
        assert_eq!(replay.results[0].fee_paid, block.tx_fees_paid[0]);
        assert_eq!(
            state.compute_state_root_for_batch(&replay.batch),
            block.header.state_root
        );
        state.commit_batch(replay.batch).unwrap();

        let receipt = state.get_tx_meta_full(&tx_hash).unwrap().unwrap();
        assert_eq!(receipt.success, Some(false));
        assert_eq!(receipt.fee_paid, Some(block.tx_fees_paid[0]));
        assert_eq!(state.get_balance(&bob).unwrap_or(0), 0);
    }

    #[test]
    fn build_block_commits_parent_certificate_at_height_two() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();
        let validator = Keypair::generate();
        let processor = TxProcessor::new(state.clone());
        let mut mempool = Mempool::new(100, 300);

        let genesis = Block::genesis(Hash::hash(b"genesis-state"), 1, Vec::new());
        state.put_block(&genesis).unwrap();
        let mut parent = Block::new_with_timestamp(
            1,
            genesis.hash(),
            Hash::hash(b"parent-state"),
            validator.pubkey().0,
            Vec::new(),
            2,
        );
        let powers = vec![lichen_core::CanonicalValidatorPower {
            validator: validator.pubkey(),
            power: lichen_core::MIN_VALIDATOR_STAKE,
        }];
        parent.header.validators_hash = lichen_core::canonical_validator_powers_hash(&powers);
        parent.sign(&validator);
        let parent_hash = parent.hash();
        let precommit = lichen_core::Precommit::signable_bytes(
            1,
            0,
            &Some(parent_hash),
            parent.header.timestamp,
        );
        parent.commit_signatures = vec![lichen_core::CommitSignature {
            validator: validator.pubkey().0,
            signature: validator.sign(&precommit),
            timestamp: parent.header.timestamp,
        }];
        state.put_block(&parent).unwrap();
        state.set_last_slot(1).unwrap();
        state
            .put_validator_power_snapshot(&parent.header.validators_hash, &powers)
            .unwrap();

        let (block, processed) = build_block(
            &state,
            &mut mempool,
            &processor,
            temp.path(),
            2,
            parent_hash,
            &validator.pubkey(),
            Vec::new(),
            2000,
            None,
        )
        .unwrap();

        assert!(processed.is_empty());
        assert_eq!(block.transactions.len(), 1);
        let certificate = CanonicalCommitCertificate::from_transaction(&block.transactions[0])
            .unwrap()
            .unwrap();
        assert_eq!(certificate.height, 1);
        assert_eq!(certificate.block_hash, parent_hash);
        assert_eq!(block.tx_fees_paid, vec![0]);
    }
}
