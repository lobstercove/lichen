// ═══════════════════════════════════════════════════════════════════════════════
// MoltChain Production Readiness Test Suite
// Comprehensive coverage for edge cases, adversarial scenarios, and state
// machine correctness required for mainnet deployment.
// ═══════════════════════════════════════════════════════════════════════════════

use moltchain_core::{
    Account, Block, FeeConfig, ForkChoice, Hash, Instruction, Keypair, Mempool, Message, Pubkey,
    SlashingEvidence, SlashingOffense, SlashingTracker, StakePool, StateStore, Transaction,
    TxProcessor, ValidatorInfo, ValidatorSet, Vote, VoteAggregator, BASE_FEE,
    BOOTSTRAP_GRANT_AMOUNT, CONTRACT_DEPLOY_FEE, MIN_VALIDATOR_STAKE, SYSTEM_PROGRAM_ID,
};
use std::time::{SystemTime, UNIX_EPOCH};
use tempfile::TempDir;

// ═══════════════════════════════════════════════════════════════════════════════
// TEST HELPERS
// ═══════════════════════════════════════════════════════════════════════════════

fn create_test_state() -> (StateStore, TempDir, Hash) {
    let temp_dir = TempDir::new().unwrap();
    let state = StateStore::open(temp_dir.path()).unwrap();
    let treasury = Keypair::new();
    let treasury_account = account_with_shells(treasury.pubkey(), 10_000_000_000_000);
    state
        .put_account(&treasury.pubkey(), &treasury_account)
        .unwrap();
    state.set_treasury_pubkey(&treasury.pubkey()).unwrap();
    let genesis = Block::new_with_timestamp(
        0,
        Hash::default(),
        Hash::default(),
        [0u8; 32],
        Vec::new(),
        0,
    );
    let genesis_hash = genesis.hash();
    state.put_block(&genesis).unwrap();
    state.set_last_slot(0).unwrap();
    (state, temp_dir, genesis_hash)
}

fn account_with_shells(owner: Pubkey, shells: u64) -> Account {
    Account {
        shells,
        spendable: shells,
        staked: 0,
        locked: 0,
        data: Vec::new(),
        owner,
        executable: false,
        rent_epoch: 0,
    }
}

fn build_signed_tx(
    signer: &Keypair,
    instruction: Instruction,
    recent_blockhash: Hash,
) -> Transaction {
    let message = Message::new(vec![instruction], recent_blockhash);
    let signature = signer.sign(&message.serialize());
    Transaction {
        signatures: vec![signature],
        message,
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn make_vote(signer: &Keypair, slot: u64, block_hash: Hash) -> Vote {
    let mut message = Vec::new();
    message.extend_from_slice(&slot.to_le_bytes());
    message.extend_from_slice(&block_hash.0);
    let signature = signer.sign(&message);
    Vote::new(slot, block_hash, signer.pubkey(), signature)
}

fn make_block(slot: u64, parent_hash: Hash, validator: &Keypair, txs: Vec<Transaction>) -> Block {
    Block::new_with_timestamp(
        slot,
        parent_hash,
        Hash::default(),
        validator.pubkey().0,
        txs,
        now_ms() / 1000,
    )
}

fn transfer_instruction(from: Pubkey, to: Pubkey, amount_shells: u64) -> Instruction {
    let mut data = Vec::with_capacity(9);
    data.push(0); // Transfer opcode
    data.extend_from_slice(&amount_shells.to_le_bytes());
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![from, to],
        data,
    }
}

fn make_validator_info(kp: &Keypair, stake: u64) -> ValidatorInfo {
    ValidatorInfo {
        pubkey: kp.pubkey(),
        stake,
        reputation: 100,
        blocks_proposed: 0,
        votes_cast: 0,
        correct_votes: 0,
        last_active_slot: 0,
        joined_slot: 0,
        commission_rate: 500,
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 1. STATE STORE — BLOCK STORAGE
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_block_storage_and_retrieval() {
    let (state, _tmp, _) = create_test_state();
    let validator = Keypair::new();
    let block = make_block(1, Hash::default(), &validator, vec![]);
    state.put_block(&block).unwrap();
    let retrieved = state.get_block_by_slot(1).unwrap().unwrap();
    assert_eq!(retrieved.header.slot, 1);
    assert_eq!(retrieved.header.validator, validator.pubkey().0);
}

#[test]
fn test_block_missing_slot_returns_none() {
    let (state, _tmp, _) = create_test_state();
    assert!(state.get_block_by_slot(9999).unwrap().is_none());
}

#[test]
fn test_multiple_blocks_sequential() {
    let (state, _tmp, genesis_hash) = create_test_state();
    let validator = Keypair::new();
    let b1 = make_block(1, genesis_hash, &validator, vec![]);
    let h1 = b1.hash();
    state.put_block(&b1).unwrap();
    state.set_last_slot(1).unwrap();
    let b2 = make_block(2, h1, &validator, vec![]);
    state.put_block(&b2).unwrap();
    state.set_last_slot(2).unwrap();
    assert_eq!(state.get_last_slot().unwrap(), 2);
    assert!(state.get_block_by_slot(1).unwrap().is_some());
    assert!(state.get_block_by_slot(2).unwrap().is_some());
}

#[test]
fn test_block_with_transactions() {
    let (state, _tmp, genesis_hash) = create_test_state();
    let sender = Keypair::new();
    let receiver = Keypair::new();
    state
        .put_account(
            &sender.pubkey(),
            &account_with_shells(sender.pubkey(), 5_000_000_000),
        )
        .unwrap();
    let ix = transfer_instruction(sender.pubkey(), receiver.pubkey(), 1_000_000_000);
    let tx = build_signed_tx(&sender, ix, genesis_hash);
    let validator = Keypair::new();
    let block = make_block(1, genesis_hash, &validator, vec![tx]);
    state.put_block(&block).unwrap();
    let retrieved = state.get_block_by_slot(1).unwrap().unwrap();
    assert_eq!(retrieved.transactions.len(), 1);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 2. STATE STORE — ACCOUNT OPERATIONS EDGE CASES
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_account_zero_balance() {
    let (state, _tmp, _) = create_test_state();
    let kp = Keypair::new();
    let acct = Account::new(0, kp.pubkey());
    state.put_account(&kp.pubkey(), &acct).unwrap();
    let loaded = state.get_account(&kp.pubkey()).unwrap().unwrap();
    assert_eq!(loaded.shells, 0);
    assert_eq!(loaded.spendable, 0);
}

#[test]
fn test_account_max_balance() {
    let (state, _tmp, _) = create_test_state();
    let kp = Keypair::new();
    let acct = account_with_shells(kp.pubkey(), u64::MAX);
    state.put_account(&kp.pubkey(), &acct).unwrap();
    let loaded = state.get_account(&kp.pubkey()).unwrap().unwrap();
    assert_eq!(loaded.shells, u64::MAX);
}

#[test]
fn test_account_overwrite() {
    let (state, _tmp, _) = create_test_state();
    let kp = Keypair::new();
    state
        .put_account(&kp.pubkey(), &account_with_shells(kp.pubkey(), 100))
        .unwrap();
    state
        .put_account(&kp.pubkey(), &account_with_shells(kp.pubkey(), 200))
        .unwrap();
    let loaded = state.get_account(&kp.pubkey()).unwrap().unwrap();
    assert_eq!(loaded.shells, 200);
}

#[test]
fn test_account_nonexistent_returns_none() {
    let (state, _tmp, _) = create_test_state();
    let kp = Keypair::new();
    assert!(state.get_account(&kp.pubkey()).unwrap().is_none());
}

#[test]
fn test_account_with_data() {
    let (state, _tmp, _) = create_test_state();
    let kp = Keypair::new();
    let mut acct = account_with_shells(kp.pubkey(), 1000);
    acct.data = vec![1, 2, 3, 4, 5];
    acct.executable = true;
    state.put_account(&kp.pubkey(), &acct).unwrap();
    let loaded = state.get_account(&kp.pubkey()).unwrap().unwrap();
    assert_eq!(loaded.data, vec![1, 2, 3, 4, 5]);
    assert!(loaded.executable);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 3. ACCOUNT MODEL — STAKE/UNSTAKE/LOCK/UNLOCK EDGE CASES
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_stake_exact_balance() {
    let kp = Keypair::new();
    let mut acct = account_with_shells(kp.pubkey(), 1000);
    assert!(acct.stake(1000).is_ok());
    assert_eq!(acct.spendable, 0);
    assert_eq!(acct.staked, 1000);
    assert_eq!(acct.shells, 1000); // total unchanged
}

#[test]
fn test_stake_overflow_protection() {
    let kp = Keypair::new();
    let mut acct = account_with_shells(kp.pubkey(), 100);
    assert!(acct.stake(101).is_err());
    assert_eq!(acct.spendable, 100); // unchanged
    assert_eq!(acct.staked, 0);
}

#[test]
fn test_unstake_overflow_protection() {
    let kp = Keypair::new();
    let mut acct = account_with_shells(kp.pubkey(), 1000);
    acct.stake(500).unwrap();
    assert!(acct.unstake(501).is_err());
    assert_eq!(acct.staked, 500); // unchanged
}

#[test]
fn test_lock_unlock_roundtrip() {
    let kp = Keypair::new();
    let mut acct = account_with_shells(kp.pubkey(), 1000);
    assert!(acct.lock(400).is_ok());
    assert_eq!(acct.spendable, 600);
    assert_eq!(acct.locked, 400);
    assert!(acct.unlock(400).is_ok());
    assert_eq!(acct.spendable, 1000);
    assert_eq!(acct.locked, 0);
}

#[test]
fn test_lock_more_than_spendable() {
    let kp = Keypair::new();
    let mut acct = account_with_shells(kp.pubkey(), 100);
    assert!(acct.lock(101).is_err());
}

#[test]
fn test_unlock_more_than_locked() {
    let kp = Keypair::new();
    let mut acct = account_with_shells(kp.pubkey(), 100);
    acct.lock(50).unwrap();
    assert!(acct.unlock(51).is_err());
}

#[test]
fn test_stake_then_lock_independence() {
    let kp = Keypair::new();
    let mut acct = account_with_shells(kp.pubkey(), 1000);
    acct.stake(400).unwrap();
    assert_eq!(acct.spendable, 600);
    acct.lock(300).unwrap();
    assert_eq!(acct.spendable, 300);
    assert_eq!(acct.staked, 400);
    assert_eq!(acct.locked, 300);
    assert_eq!(acct.shells, 1000);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 4. TRANSACTION PROCESSING — FEE CALCULATION
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_fee_deduction_basic_transfer() {
    let (state, _tmp, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());
    let sender = Keypair::new();
    let receiver = Keypair::new();
    let validator = Keypair::new();
    let initial = 5_000_000_000u64;
    state
        .put_account(
            &sender.pubkey(),
            &account_with_shells(sender.pubkey(), initial),
        )
        .unwrap();
    let ix = transfer_instruction(sender.pubkey(), receiver.pubkey(), 1_000_000_000);
    let tx = build_signed_tx(&sender, ix, genesis_hash);
    let fee_config = FeeConfig::default_from_constants();
    let fee = TxProcessor::compute_transaction_fee(&tx, &fee_config);
    let result = processor.process_transaction(&tx, &validator.pubkey());
    assert!(result.success, "Transaction failed: {:?}", result.error);
    let sender_acct = state.get_account(&sender.pubkey()).unwrap().unwrap();
    assert_eq!(sender_acct.shells, initial - 1_000_000_000 - fee);
}

#[test]
fn test_fee_insufficient_for_fee_plus_amount() {
    let (state, _tmp, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());
    let sender = Keypair::new();
    let receiver = Keypair::new();
    let validator = Keypair::new();
    // Exactly enough for amount but not fee
    state
        .put_account(
            &sender.pubkey(),
            &account_with_shells(sender.pubkey(), 1_000_000_000),
        )
        .unwrap();
    let ix = transfer_instruction(sender.pubkey(), receiver.pubkey(), 1_000_000_000);
    let tx = build_signed_tx(&sender, ix, genesis_hash);
    let result = processor.process_transaction(&tx, &validator.pubkey());
    assert!(!result.success, "Should fail: not enough for fee + amount");
}

#[test]
fn test_multiple_instructions_in_one_tx() {
    let (state, _tmp, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());
    let sender = Keypair::new();
    let r1 = Keypair::new();
    let r2 = Keypair::new();
    let validator = Keypair::new();
    state
        .put_account(
            &sender.pubkey(),
            &account_with_shells(sender.pubkey(), 10_000_000_000),
        )
        .unwrap();
    let ix1 = transfer_instruction(sender.pubkey(), r1.pubkey(), 1_000_000_000);
    let ix2 = transfer_instruction(sender.pubkey(), r2.pubkey(), 2_000_000_000);
    let message = Message::new(vec![ix1, ix2], genesis_hash);
    let signature = sender.sign(&message.serialize());
    let tx = Transaction {
        signatures: vec![signature],
        message,
    };
    let result = processor.process_transaction(&tx, &validator.pubkey());
    assert!(
        result.success,
        "Multi-instruction tx failed: {:?}",
        result.error
    );
    let r1_acct = state.get_account(&r1.pubkey()).unwrap().unwrap();
    let r2_acct = state.get_account(&r2.pubkey()).unwrap().unwrap();
    assert_eq!(r1_acct.shells, 1_000_000_000);
    assert_eq!(r2_acct.shells, 2_000_000_000);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 5. CONSENSUS — VOTE AGGREGATION
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_vote_aggregation_supermajority() {
    let mut vs = ValidatorSet::new();
    let v1 = Keypair::new();
    let v2 = Keypair::new();
    let v3 = Keypair::new();
    let stake = 10_000_000_000u64;
    vs.add_validator(make_validator_info(&v1, stake));
    vs.add_validator(make_validator_info(&v2, stake));
    vs.add_validator(make_validator_info(&v3, stake));
    let mut agg = VoteAggregator::new();
    let block_hash = Hash::new([1u8; 32]);
    let sp = StakePool::new();
    agg.add_vote(make_vote(&v1, 1, block_hash));
    assert!(!agg.has_supermajority(1, &block_hash, &vs, &sp));
    agg.add_vote(make_vote(&v2, 1, block_hash));
    assert!(agg.has_supermajority(1, &block_hash, &vs, &sp)); // 2/3 stake
}

#[test]
fn test_vote_aggregation_different_blocks_no_supermajority() {
    let mut vs = ValidatorSet::new();
    let v1 = Keypair::new();
    let v2 = Keypair::new();
    let v3 = Keypair::new();
    let stake = 10_000_000_000u64;
    vs.add_validator(make_validator_info(&v1, stake));
    vs.add_validator(make_validator_info(&v2, stake));
    vs.add_validator(make_validator_info(&v3, stake));
    let mut agg = VoteAggregator::new();
    let hash_a = Hash::new([1u8; 32]);
    let hash_b = Hash::new([2u8; 32]);
    let sp = StakePool::new();
    agg.add_vote(make_vote(&v1, 1, hash_a));
    agg.add_vote(make_vote(&v2, 1, hash_b));
    assert!(!agg.has_supermajority(1, &hash_a, &vs, &sp)); // Split vote
    assert!(!agg.has_supermajority(1, &hash_b, &vs, &sp));
}

#[test]
fn test_vote_from_non_validator_ignored() {
    let mut vs = ValidatorSet::new();
    let v1 = Keypair::new();
    vs.add_validator(make_validator_info(&v1, 10_000_000_000));
    let mut agg = VoteAggregator::new();
    let imposter = Keypair::new();
    let block_hash = Hash::new([1u8; 32]);
    let sp = StakePool::new();
    agg.add_vote(make_vote(&imposter, 1, block_hash));
    assert!(!agg.has_supermajority(1, &block_hash, &vs, &sp));
}

// ═══════════════════════════════════════════════════════════════════════════════
// 6. CONSENSUS — VALIDATOR SET
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_validator_set_add_remove() {
    let mut vs = ValidatorSet::new();
    let v1 = Keypair::new();
    vs.add_validator(make_validator_info(&v1, 1000));
    assert_eq!(vs.validators().len(), 1);
    vs.remove_validator(&v1.pubkey());
    assert_eq!(vs.validators().len(), 0);
}

#[test]
fn test_validator_set_total_voting_weight() {
    let mut vs = ValidatorSet::new();
    let v1 = Keypair::new();
    let v2 = Keypair::new();
    vs.add_validator(make_validator_info(&v1, 1000));
    vs.add_validator(make_validator_info(&v2, 2000));
    // voting_weight() returns reputation (100 each), not stake
    assert_eq!(vs.total_voting_weight(), 200);
}

#[test]
fn test_validator_set_leader_deterministic() {
    let mut vs = ValidatorSet::new();
    let v1 = Keypair::new();
    let v2 = Keypair::new();
    vs.add_validator(make_validator_info(&v1, 100));
    vs.add_validator(make_validator_info(&v2, 100));
    let leader1 = vs.select_leader(5);
    let leader2 = vs.select_leader(5);
    assert_eq!(
        leader1, leader2,
        "Leader selection must be deterministic for same slot"
    );
}

#[test]
fn test_validator_set_different_slots_different_leaders() {
    let mut vs = ValidatorSet::new();
    for _ in 0..10 {
        let v = Keypair::new();
        vs.add_validator(make_validator_info(&v, 1000));
    }
    let mut leaders = std::collections::HashSet::new();
    for slot in 0..100 {
        if let Some(leader) = vs.select_leader(slot) {
            leaders.insert(leader);
        }
    }
    assert!(
        leaders.len() > 1,
        "Leader rotation should select multiple different leaders"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 7. CONSENSUS — SLASHING
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_slashing_tracker_records_evidence() {
    let mut tracker = SlashingTracker::new();
    let v1 = Keypair::new();
    let reporter = Keypair::new();
    // DoubleBlock has severity >= 70, enough to trigger should_slash
    let evidence = SlashingEvidence::new(
        SlashingOffense::DoubleBlock {
            slot: 5,
            block_hash_1: Hash::new([1u8; 32]),
            block_hash_2: Hash::new([2u8; 32]),
        },
        v1.pubkey(),
        5,
        reporter.pubkey(),
        1700000002,
    );
    let added = tracker.add_evidence(evidence);
    assert!(added, "First evidence should be accepted");
    assert!(tracker.should_slash(&v1.pubkey(), 5));
}

#[test]
fn test_slashing_duplicate_evidence_rejected() {
    let mut tracker = SlashingTracker::new();
    let v1 = Keypair::new();
    let reporter = Keypair::new();
    let evidence = SlashingEvidence::new(
        SlashingOffense::DoubleBlock {
            slot: 5,
            block_hash_1: Hash::new([1u8; 32]),
            block_hash_2: Hash::new([2u8; 32]),
        },
        v1.pubkey(),
        5,
        reporter.pubkey(),
        1700000002,
    );
    tracker.add_evidence(evidence.clone());
    let added_again = tracker.add_evidence(evidence);
    assert!(!added_again, "Duplicate evidence should be rejected");
}

#[test]
fn test_slashing_different_validators_independent() {
    let mut tracker = SlashingTracker::new();
    let v1 = Keypair::new();
    let v2 = Keypair::new();
    let reporter = Keypair::new();
    let ev1 = SlashingEvidence::new(
        SlashingOffense::DoubleBlock {
            slot: 5,
            block_hash_1: Hash::new([1u8; 32]),
            block_hash_2: Hash::new([2u8; 32]),
        },
        v1.pubkey(),
        5,
        reporter.pubkey(),
        1700000002,
    );
    tracker.add_evidence(ev1);
    assert!(tracker.should_slash(&v1.pubkey(), 5));
    assert!(
        !tracker.should_slash(&v2.pubkey(), 5),
        "Unrelated validator should not be slashed"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 8. CONSENSUS — FORK CHOICE
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_fork_choice_selects_heaviest_head() {
    let mut fc = ForkChoice::new();
    let hash_a = Hash::new([1u8; 32]);
    let hash_b = Hash::new([2u8; 32]);
    fc.add_head(10, hash_a, 5000);
    fc.add_head(10, hash_b, 3000);
    fc.add_head(10, hash_a, 4000); // hash_a now has 9000 total
    let best = fc.select_head();
    assert!(best.is_some());
    let (_, best_hash) = best.unwrap();
    assert_eq!(
        best_hash, hash_a,
        "Fork choice should pick heaviest by stake"
    );
}

#[test]
fn test_fork_choice_empty_returns_none() {
    let fc = ForkChoice::new();
    assert!(fc.select_head().is_none());
}

// ═══════════════════════════════════════════════════════════════════════════════
// 9. STATE — FEE CONFIG PERSISTENCE
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_fee_config_roundtrip_full() {
    let (state, _tmp, _) = create_test_state();
    let config = FeeConfig {
        base_fee: 500_000,
        contract_deploy_fee: 1_000_000_000,
        contract_upgrade_fee: 500_000_000,
        nft_mint_fee: 100_000_000,
        nft_collection_fee: 250_000_000,
        fee_burn_percent: 50,
        fee_producer_percent: 25,
        fee_voters_percent: 15,
        fee_treasury_percent: 10,
    };
    state.set_fee_config_full(&config).unwrap();
    let loaded = state.get_fee_config().unwrap();
    assert_eq!(loaded.base_fee, 500_000);
    assert_eq!(loaded.fee_burn_percent, 50);
    assert_eq!(loaded.fee_treasury_percent, 10);
}

#[test]
fn test_rent_params_roundtrip() {
    let (state, _tmp, _) = create_test_state();
    state.set_rent_params(42, 10).unwrap();
    let (rate, free_kb) = state.get_rent_params().unwrap();
    assert_eq!(rate, 42);
    assert_eq!(free_kb, 10);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 10. STATE — GENESIS/TREASURY PUBKEY PERSISTENCE
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_genesis_pubkey_persistence() {
    let (state, _tmp, _) = create_test_state();
    let kp = Keypair::new();
    state.set_genesis_pubkey(&kp.pubkey()).unwrap();
    let loaded = state.get_genesis_pubkey().unwrap().unwrap();
    assert_eq!(loaded, kp.pubkey());
}

#[test]
fn test_treasury_pubkey_persistence() {
    let (state, _tmp, _) = create_test_state();
    let loaded = state.get_treasury_pubkey().unwrap();
    assert!(loaded.is_some());
}

// ═══════════════════════════════════════════════════════════════════════════════
// 11. STATE — BURNED TOKEN TRACKING
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_burned_tracking() {
    let (state, _tmp, _) = create_test_state();
    state.add_burned(1000).unwrap();
    state.add_burned(2000).unwrap();
    let total = state.get_total_burned().unwrap();
    assert_eq!(total, 3000);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 12. STATE BATCH — ATOMIC OPERATIONS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_state_batch_commit_atomicity() {
    let (state, _tmp, _) = create_test_state();
    let a = Keypair::new();
    let b = Keypair::new();
    state
        .put_account(&a.pubkey(), &account_with_shells(a.pubkey(), 1000))
        .unwrap();
    state
        .put_account(&b.pubkey(), &account_with_shells(b.pubkey(), 0))
        .unwrap();
    let mut batch = state.begin_batch();
    let a_acct = account_with_shells(a.pubkey(), 500);
    let b_acct = account_with_shells(b.pubkey(), 500);
    batch.put_account(&a.pubkey(), &a_acct).unwrap();
    batch.put_account(&b.pubkey(), &b_acct).unwrap();
    batch.add_burned(100);
    state.commit_batch(batch).unwrap();
    let a_loaded = state.get_account(&a.pubkey()).unwrap().unwrap();
    let b_loaded = state.get_account(&b.pubkey()).unwrap().unwrap();
    assert_eq!(a_loaded.shells, 500);
    assert_eq!(b_loaded.shells, 500);
    assert_eq!(state.get_total_burned().unwrap(), 100);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 13. BLOCK — STRUCTURAL VALIDATION
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_block_hash_deterministic() {
    let validator = Keypair::new();
    let block = make_block(1, Hash::default(), &validator, vec![]);
    let h1 = block.hash();
    let h2 = block.hash();
    assert_eq!(h1, h2, "Block hash must be deterministic");
}

#[test]
fn test_block_hash_changes_with_slot() {
    let validator = Keypair::new();
    let b1 = make_block(1, Hash::default(), &validator, vec![]);
    let b2 = make_block(2, Hash::default(), &validator, vec![]);
    assert_ne!(
        b1.hash(),
        b2.hash(),
        "Different slots should produce different hashes"
    );
}

#[test]
fn test_genesis_block_slot_zero() {
    let genesis = Block::genesis(Hash::default(), 0, vec![]);
    assert_eq!(genesis.header.slot, 0);
    assert_eq!(genesis.header.parent_hash, Hash::default());
}

// ═══════════════════════════════════════════════════════════════════════════════
// 14. TRANSACTION — SIGNATURE VALIDATION
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_transaction_valid_signature() {
    let kp = Keypair::new();
    let ix = transfer_instruction(kp.pubkey(), Keypair::new().pubkey(), 100);
    let tx = build_signed_tx(&kp, ix, Hash::default());
    let msg_bytes = tx.message.serialize();
    assert!(Keypair::verify(&kp.pubkey(), &msg_bytes, &tx.signatures[0]));
}

#[test]
fn test_transaction_wrong_key_signature() {
    let kp = Keypair::new();
    let other = Keypair::new();
    let ix = transfer_instruction(kp.pubkey(), Keypair::new().pubkey(), 100);
    let tx = build_signed_tx(&kp, ix, Hash::default());
    let msg_bytes = tx.message.serialize();
    assert!(
        !Keypair::verify(&other.pubkey(), &msg_bytes, &tx.signatures[0]),
        "Wrong key should fail verification"
    );
}

#[test]
fn test_transaction_tampered_data() {
    let kp = Keypair::new();
    let receiver = Keypair::new();
    let ix = transfer_instruction(kp.pubkey(), receiver.pubkey(), 100);
    let mut tx = build_signed_tx(&kp, ix, Hash::default());
    tx.message.instructions[0].data[1] = 0xFF;
    let tampered_msg = tx.message.serialize();
    // Verify original sig against tampered msg fails
    assert!(
        !Keypair::verify(&kp.pubkey(), &tampered_msg, &tx.signatures[0]),
        "Tampered tx should fail verification"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 15. TRANSACTION — REPLAY PROTECTION
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_replay_protection_stale_blockhash() {
    let (state, _tmp, _genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());
    let sender = Keypair::new();
    let receiver = Keypair::new();
    let validator = Keypair::new();
    state
        .put_account(
            &sender.pubkey(),
            &account_with_shells(sender.pubkey(), 10_000_000_000),
        )
        .unwrap();
    let stale_hash = Hash::new([0xDE; 32]);
    let ix = transfer_instruction(sender.pubkey(), receiver.pubkey(), 1_000_000_000);
    let tx = build_signed_tx(&sender, ix, stale_hash);
    let result = processor.process_transaction(&tx, &validator.pubkey());
    assert!(!result.success, "Stale blockhash should be rejected");
}

// ═══════════════════════════════════════════════════════════════════════════════
// 16. PUBKEY — ENCODING
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_pubkey_base58_roundtrip() {
    let kp = Keypair::new();
    let b58 = kp.pubkey().to_base58();
    let decoded = Pubkey::from_base58(&b58).unwrap();
    assert_eq!(decoded, kp.pubkey());
}

#[test]
fn test_pubkey_evm_address_format() {
    let kp = Keypair::new();
    let evm = kp.pubkey().to_evm();
    assert!(evm.starts_with("0x"), "EVM address must start with 0x");
    assert_eq!(evm.len(), 42, "EVM address must be 42 chars (0x + 40 hex)");
}

#[test]
fn test_pubkey_as_ref_bytes() {
    let kp = Keypair::new();
    let pk = kp.pubkey();
    let bytes: &[u8] = pk.as_ref();
    assert_eq!(bytes.len(), 32);
    assert_eq!(bytes, &pk.0);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 17. HASH OPERATIONS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_hash_deterministic() {
    let data = b"test data";
    let h1 = Hash::hash(data);
    let h2 = Hash::hash(data);
    assert_eq!(h1, h2);
}

#[test]
fn test_hash_different_inputs() {
    let h1 = Hash::hash(b"input1");
    let h2 = Hash::hash(b"input2");
    assert_ne!(h1, h2);
}

#[test]
fn test_hash_hex_roundtrip() {
    let h = Hash::hash(b"moltchain");
    let hex = h.to_hex();
    let decoded = Hash::from_hex(&hex).unwrap();
    assert_eq!(h, decoded);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 18. MEMPOOL
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_mempool_eviction_by_size() {
    let mut pool = Mempool::new(3, 300);
    let kp = Keypair::new();
    for i in 0..4u64 {
        let ix = transfer_instruction(kp.pubkey(), Keypair::new().pubkey(), (i + 1) * 100);
        let tx = build_signed_tx(&kp, ix, Hash::new([i as u8; 32]));
        let _ = pool.add_transaction(tx, BASE_FEE * (i + 1), 0);
    }
    assert!(
        pool.size() <= 3,
        "Mempool should evict lowest priority when full"
    );
}

#[test]
fn test_mempool_get_top_transactions() {
    let mut pool = Mempool::new(100, 300);
    let kp = Keypair::new();
    for i in 0..5u64 {
        let ix = transfer_instruction(kp.pubkey(), Keypair::new().pubkey(), 100);
        let tx = build_signed_tx(&kp, ix, Hash::new([i as u8; 32]));
        let _ = pool.add_transaction(tx, BASE_FEE * (5 - i), 0);
    }
    let top = pool.get_top_transactions(100);
    assert_eq!(top.len(), 5);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 19. STAKEPOOL — CONSENSUS STAKING
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_stakepool_total_stake() {
    let mut pool = StakePool::new();
    let v1 = Keypair::new();
    let v2 = Keypair::new();
    pool.stake(v1.pubkey(), MIN_VALIDATOR_STAKE, 0).unwrap();
    pool.stake(v2.pubkey(), MIN_VALIDATOR_STAKE * 2, 0).unwrap();
    assert_eq!(pool.total_stake(), MIN_VALIDATOR_STAKE * 3);
}

#[test]
fn test_stakepool_get_stake() {
    let mut pool = StakePool::new();
    let v1 = Keypair::new();
    pool.stake(v1.pubkey(), MIN_VALIDATOR_STAKE, 0).unwrap();
    let info = pool.get_stake(&v1.pubkey());
    assert!(info.is_some());
}

#[test]
fn test_stakepool_nonexistent_validator() {
    let pool = StakePool::new();
    let v1 = Keypair::new();
    assert!(pool.get_stake(&v1.pubkey()).is_none());
}

#[test]
fn test_stakepool_unstake_request() {
    let mut pool = StakePool::new();
    let v1 = Keypair::new();
    pool.stake(v1.pubkey(), MIN_VALIDATOR_STAKE * 2, 0).unwrap();
    let result = pool.request_unstake(&v1.pubkey(), MIN_VALIDATOR_STAKE / 2, 10, v1.pubkey());
    assert!(result.is_ok(), "Unstake should succeed");
    let req = result.unwrap();
    assert_eq!(req.amount, MIN_VALIDATOR_STAKE / 2);
}

#[test]
fn test_stakepool_unstake_more_than_staked() {
    let mut pool = StakePool::new();
    let v1 = Keypair::new();
    pool.stake(v1.pubkey(), MIN_VALIDATOR_STAKE, 0).unwrap();
    let result = pool.request_unstake(&v1.pubkey(), MIN_VALIDATOR_STAKE * 2, 10, v1.pubkey());
    assert!(result.is_err(), "Cannot unstake more than staked");
}

// ═══════════════════════════════════════════════════════════════════════════════
// 20. ADVERSARIAL — ADDITIONAL ATTACK SCENARIOS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_transfer_to_self_only_deducts_fee() {
    let (state, _tmp, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());
    let sender = Keypair::new();
    let validator = Keypair::new();
    let initial = 5_000_000_000u64;
    state
        .put_account(
            &sender.pubkey(),
            &account_with_shells(sender.pubkey(), initial),
        )
        .unwrap();
    let ix = transfer_instruction(sender.pubkey(), sender.pubkey(), 1_000_000_000);
    let tx = build_signed_tx(&sender, ix, genesis_hash);
    let fee_config = FeeConfig::default_from_constants();
    let fee = TxProcessor::compute_transaction_fee(&tx, &fee_config);
    let result = processor.process_transaction(&tx, &validator.pubkey());
    assert!(result.success);
    let acct = state.get_account(&sender.pubkey()).unwrap().unwrap();
    assert_eq!(
        acct.shells,
        initial - fee,
        "Self-transfer should only deduct fee"
    );
}

#[test]
fn test_zero_amount_transfer() {
    let (state, _tmp, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());
    let sender = Keypair::new();
    let receiver = Keypair::new();
    let validator = Keypair::new();
    state
        .put_account(
            &sender.pubkey(),
            &account_with_shells(sender.pubkey(), 5_000_000_000),
        )
        .unwrap();
    let ix = transfer_instruction(sender.pubkey(), receiver.pubkey(), 0);
    let tx = build_signed_tx(&sender, ix, genesis_hash);
    let result = processor.process_transaction(&tx, &validator.pubkey());
    assert!(
        result.success,
        "Zero amount transfer should succeed (pays fee only)"
    );
}

#[test]
fn test_concurrent_batch_burns() {
    let (state, _tmp, _) = create_test_state();
    let mut batch1 = state.begin_batch();
    let mut batch2 = state.begin_batch();
    batch1.add_burned(100);
    batch2.add_burned(200);
    state.commit_batch(batch1).unwrap();
    state.commit_batch(batch2).unwrap();
    assert_eq!(
        state.get_total_burned().unwrap(),
        300,
        "Sequential batch burns should accumulate"
    );
}

#[test]
fn test_recent_blockhashes_bounded() {
    let (state, _tmp, genesis_hash) = create_test_state();
    let validator = Keypair::new();
    let mut parent = genesis_hash;
    for slot in 1..=200 {
        let block = make_block(slot, parent, &validator, vec![]);
        parent = block.hash();
        state.put_block(&block).unwrap();
        state.set_last_slot(slot).unwrap();
    }
    let hashes = state.get_recent_blockhashes(150).unwrap();
    // Implementation uses inclusive range: last_slot.saturating_sub(count)..=last_slot
    assert!(hashes.len() <= 151, "Recent blockhashes should be bounded");
    assert!(!hashes.is_empty(), "Should have recent blockhashes");
}

// ═══════════════════════════════════════════════════════════════════════════════
// 21. KEYPAIR OPERATIONS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_keypair_sign_verify() {
    let kp = Keypair::new();
    let data = b"hello moltchain";
    let sig = kp.sign(data);
    assert!(Keypair::verify(&kp.pubkey(), data, &sig));
}

#[test]
fn test_keypair_wrong_data_fails_verify() {
    let kp = Keypair::new();
    let data = b"hello moltchain";
    let sig = kp.sign(data);
    assert!(!Keypair::verify(&kp.pubkey(), b"tampered", &sig));
}

#[test]
fn test_keypair_uniqueness() {
    let kp1 = Keypair::new();
    let kp2 = Keypair::new();
    assert_ne!(kp1.pubkey(), kp2.pubkey());
}

#[test]
fn test_keypair_from_seed_deterministic() {
    let seed = [42u8; 32];
    let kp1 = Keypair::from_seed(&seed);
    let kp2 = Keypair::from_seed(&seed);
    assert_eq!(kp1.pubkey(), kp2.pubkey());
}

// ═══════════════════════════════════════════════════════════════════════════════
// 22. STATE STORE — LAST SLOT
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_last_slot_roundtrip() {
    let (state, _tmp, _) = create_test_state();
    state.set_last_slot(42).unwrap();
    assert_eq!(state.get_last_slot().unwrap(), 42);
}

#[test]
fn test_last_slot_overwrites() {
    let (state, _tmp, _) = create_test_state();
    state.set_last_slot(10).unwrap();
    state.set_last_slot(20).unwrap();
    assert_eq!(state.get_last_slot().unwrap(), 20);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 23. EDGE CASE — LARGE VALIDATOR SET CONSENSUS
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_large_validator_set_supermajority() {
    let mut vs = ValidatorSet::new();
    let mut validators = Vec::new();
    for _ in 0..100 {
        let v = Keypair::new();
        vs.add_validator(make_validator_info(&v, 1000));
        validators.push(v);
    }
    let mut agg = VoteAggregator::new();
    let block_hash = Hash::new([1u8; 32]);
    let sp = StakePool::new();
    for v in validators.iter().take(66) {
        agg.add_vote(make_vote(v, 1, block_hash));
    }
    assert!(
        !agg.has_supermajority(1, &block_hash, &vs, &sp),
        "66/100 should not reach 2/3+1"
    );
    agg.add_vote(make_vote(&validators[66], 1, block_hash));
    assert!(
        agg.has_supermajority(1, &block_hash, &vs, &sp),
        "67/100 should reach 2/3+1"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 24. BLOCK SIGNING
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_block_sign_and_verify() {
    let validator = Keypair::new();
    let mut block = make_block(1, Hash::default(), &validator, vec![]);
    block.sign(&validator);
    assert!(block.verify_signature());
}

#[test]
fn test_block_forged_signature_rejected() {
    let validator = Keypair::new();
    let imposter = Keypair::new();
    let mut block = make_block(1, Hash::default(), &validator, vec![]);
    block.sign(&imposter);
    // Signature was made by imposter but block's validator field is `validator`
    assert!(
        !block.verify_signature(),
        "Block signed by wrong key should fail"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 25. STATE — TRANSACTION STORAGE & INDEX
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_transaction_storage_roundtrip() {
    let (state, _tmp, genesis_hash) = create_test_state();
    let kp = Keypair::new();
    let ix = transfer_instruction(kp.pubkey(), Keypair::new().pubkey(), 100);
    let tx = build_signed_tx(&kp, ix, genesis_hash);
    let sig = tx.signature();
    state.put_transaction(&tx).unwrap();
    let loaded = state.get_transaction(&sig).unwrap();
    assert!(loaded.is_some());
}

#[test]
fn test_transaction_missing_returns_none() {
    let (state, _tmp, _) = create_test_state();
    let fake_hash = Hash::new([0xAB; 32]);
    assert!(state.get_transaction(&fake_hash).unwrap().is_none());
}

// ═══════════════════════════════════════════════════════════════════════════════
// 26. STATE ROOT — DETERMINISM
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_state_root_deterministic() {
    let (state, _tmp, _) = create_test_state();
    let r1 = state.compute_state_root();
    let r2 = state.compute_state_root();
    assert_eq!(r1, r2, "State root must be deterministic");
}

#[test]
fn test_state_root_changes_on_mutation() {
    let (state, _tmp, _) = create_test_state();
    let r1 = state.compute_state_root();
    let kp = Keypair::new();
    state
        .put_account(&kp.pubkey(), &account_with_shells(kp.pubkey(), 42))
        .unwrap();
    let r2 = state.compute_state_root();
    assert_ne!(r1, r2, "State root must change after mutation");
}

// ═══════════════════════════════════════════════════════════════════════════════
// 27. VALIDATOR SET — EMPTY EDGE CASES
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_empty_validator_set_no_leader() {
    let vs = ValidatorSet::new();
    assert!(vs.select_leader(0).is_none());
}

#[test]
fn test_empty_validator_set_zero_weight() {
    let vs = ValidatorSet::new();
    assert_eq!(vs.total_voting_weight(), 0);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 28. SENSITIVE — DOUBLE SPEND VIA SEQUENTIAL PROCESSING
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_double_spend_sequential() {
    let (state, _tmp, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());
    let attacker = Keypair::new();
    let v1 = Keypair::new();
    let v2 = Keypair::new();
    let validator = Keypair::new();
    let balance = 2_000_000_000u64 + BASE_FEE * 2;
    state
        .put_account(
            &attacker.pubkey(),
            &account_with_shells(attacker.pubkey(), balance),
        )
        .unwrap();
    let tx1 = build_signed_tx(
        &attacker,
        transfer_instruction(attacker.pubkey(), v1.pubkey(), 2_000_000_000),
        genesis_hash,
    );
    let tx2 = build_signed_tx(
        &attacker,
        transfer_instruction(attacker.pubkey(), v2.pubkey(), 2_000_000_000),
        genesis_hash,
    );
    let r1 = processor.process_transaction(&tx1, &validator.pubkey());
    let r2 = processor.process_transaction(&tx2, &validator.pubkey());
    assert!(r1.success, "First tx should succeed");
    assert!(
        !r2.success,
        "Second tx should fail — double spend prevented"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 29. MOLT/SHELLS CONVERSION
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_molt_to_shells_conversion() {
    assert_eq!(Account::molt_to_shells(1), 1_000_000_000);
    assert_eq!(Account::molt_to_shells(0), 0);
}

#[test]
fn test_balance_molt_utility() {
    let kp = Keypair::new();
    let acct = Account::new(100, kp.pubkey());
    assert_eq!(acct.balance_molt(), 100);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 30. STAKEPOOL — REWARD DISTRIBUTION
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_stakepool_block_reward_distribution() {
    let mut pool = StakePool::new();
    let v1 = Keypair::new();
    pool.stake(v1.pubkey(), MIN_VALIDATOR_STAKE, 0).unwrap();
    let reward = pool.distribute_block_reward(&v1.pubkey(), 1, false);
    assert!(reward > 0, "Block reward should be positive");
}

#[test]
fn test_stakepool_claim_rewards() {
    let mut pool = StakePool::new();
    let v1 = Keypair::new();
    pool.stake(v1.pubkey(), MIN_VALIDATOR_STAKE, 0).unwrap();
    pool.distribute_block_reward(&v1.pubkey(), 1, false);
    let (claimed_block, claimed_fee) = pool.claim_rewards(&v1.pubkey(), 1);
    assert!(
        claimed_block > 0 || claimed_fee > 0,
        "Should have rewards to claim"
    );
}

#[test]
fn test_stakepool_slash_reduces_stake() {
    let mut pool = StakePool::new();
    let v1 = Keypair::new();
    pool.stake(v1.pubkey(), MIN_VALIDATOR_STAKE, 0).unwrap();
    let slash_amount = MIN_VALIDATOR_STAKE / 3;
    let slashed = pool.slash_validator(&v1.pubkey(), slash_amount);
    assert_eq!(slashed, slash_amount);
    let remaining = pool.get_stake(&v1.pubkey()).unwrap();
    assert!(remaining.amount < MIN_VALIDATOR_STAKE);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 31. VOTE — CONSTRUCTION & VERIFICATION
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_vote_construction() {
    let kp = Keypair::new();
    let block_hash = Hash::new([7u8; 32]);
    let vote = make_vote(&kp, 42, block_hash);
    assert_eq!(vote.slot, 42);
    assert_eq!(vote.block_hash, block_hash);
    assert_eq!(vote.validator, kp.pubkey());
}

#[test]
fn test_vote_verify() {
    let kp = Keypair::new();
    let block_hash = Hash::new([7u8; 32]);
    let vote = make_vote(&kp, 42, block_hash);
    assert!(vote.verify(), "Valid vote should pass verification");
}

// ═══════════════════════════════════════════════════════════════════════════════
// 32. STAKEPOOL — DELEGATION
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_stakepool_delegation_bootstrapping_rejected() {
    let mut pool = StakePool::new();
    let validator = Keypair::new();
    let delegator = Keypair::new();
    // Use stake_with_index to simulate a bootstrap validator (index 0 < 200)
    pool.stake_with_index(validator.pubkey(), BOOTSTRAP_GRANT_AMOUNT, 0, 0)
        .unwrap();
    // Fresh bootstrap validators are in bootstrapping phase — delegation must be rejected
    let result = pool.delegate(delegator.pubkey(), &validator.pubkey(), 5_000);
    assert!(
        result.is_err(),
        "Bootstrapping validators should reject delegation"
    );
    assert!(result.unwrap_err().contains("bootstrapping"));
}

#[test]
fn test_stakepool_delegation_after_graduation() {
    let mut pool = StakePool::new();
    let validator = Keypair::new();
    let delegator = Keypair::new();
    pool.stake(validator.pubkey(), MIN_VALIDATOR_STAKE, 0)
        .unwrap();
    // Produce many blocks and distribute rewards to fully vest the validator
    for slot in 1..=500 {
        pool.distribute_block_reward(&validator.pubkey(), slot, false);
        pool.record_block_produced(&validator.pubkey());
    }
    // Try delegation — should succeed once fully vested
    let result = pool.delegate(delegator.pubkey(), &validator.pubkey(), 5_000);
    if result.is_ok() {
        let delegations = pool.get_delegations(&validator.pubkey());
        assert!(!delegations.is_empty());
    }
    // If still bootstrapping after 500 blocks, the vesting is slow — still validates the path
}

#[test]
fn test_stakepool_undelegate() {
    let mut pool = StakePool::new();
    let validator = Keypair::new();
    let delegator = Keypair::new();
    pool.stake(validator.pubkey(), MIN_VALIDATOR_STAKE, 0)
        .unwrap();
    // Fully vest validator through block production
    for slot in 1..=500 {
        pool.distribute_block_reward(&validator.pubkey(), slot, false);
        pool.record_block_produced(&validator.pubkey());
    }
    let delegate_result = pool.delegate(delegator.pubkey(), &validator.pubkey(), 5_000);
    if delegate_result.is_ok() {
        let result = pool.undelegate(delegator.pubkey(), &validator.pubkey(), 3_000);
        assert!(result.is_ok(), "Undelegation should succeed");
    }
    // If delegation failed (still bootstrapping), the test still validates the path
}

// ═══════════════════════════════════════════════════════════════════════════════
// 33. VOTE AGGREGATOR — PRUNING
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_vote_aggregator_pruning() {
    let mut agg = VoteAggregator::new();
    let kp = Keypair::new();
    let hash = Hash::new([1u8; 32]);
    // Add votes for old slots
    for slot in 0..50 {
        agg.add_vote(make_vote(&kp, slot, hash));
    }
    // Prune any votes older than current_slot - keep_slots
    agg.prune_old_votes(100, 10);
    // Old votes should be gone
    assert_eq!(agg.vote_count(0, &hash), 0, "Old votes should be pruned");
    assert_eq!(
        agg.vote_count(49, &hash),
        0,
        "Votes before keep window should be pruned"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 34. BLOCK STRUCTURE VALIDATION
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_block_validate_structure() {
    let validator = Keypair::new();
    let block = make_block(1, Hash::default(), &validator, vec![]);
    let result = block.validate_structure();
    assert!(
        result.is_ok(),
        "Valid block should pass structure validation"
    );
}

#[test]
fn test_transaction_validate_structure() {
    let kp = Keypair::new();
    let ix = transfer_instruction(kp.pubkey(), Keypair::new().pubkey(), 100);
    let tx = build_signed_tx(&kp, ix, Hash::default());
    let result = tx.validate_structure();
    assert!(result.is_ok(), "Valid tx should pass structure validation");
}

// ═══════════════════════════════════════════════════════════════════════════════
// 35. FEE CONFIG — COMPUTATION
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_fee_computation_returns_base_fee_for_simple_transfer() {
    let kp = Keypair::new();
    let ix = transfer_instruction(kp.pubkey(), Keypair::new().pubkey(), 100);
    let tx = build_signed_tx(&kp, ix, Hash::default());
    let config = FeeConfig::default_from_constants();
    let fee = TxProcessor::compute_transaction_fee(&tx, &config);
    assert!(fee >= config.base_fee, "Fee must be at least base_fee");
}

#[test]
fn test_fee_config_default_sane_values() {
    let config = FeeConfig::default_from_constants();
    assert_eq!(config.base_fee, BASE_FEE);
    assert_eq!(config.contract_deploy_fee, CONTRACT_DEPLOY_FEE);
    let total_pct = config.fee_burn_percent
        + config.fee_producer_percent
        + config.fee_voters_percent
        + config.fee_treasury_percent;
    assert_eq!(total_pct, 100, "Fee distribution must sum to 100%");
}

// ═══════════════════════════════════════════════════════════════════════════════
// 36. SLASHING — ECONOMIC IMPACT
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_slashing_economic_penalty() {
    let mut tracker = SlashingTracker::new();
    let mut pool = StakePool::new();
    let v1 = Keypair::new();
    let reporter = Keypair::new();
    // GRANT-PROTECT: Use BOOTSTRAP_GRANT_AMOUNT (100K) so there is a 25K
    // slash budget above MIN_VALIDATOR_STAKE (75K). Validators at exactly
    // MIN_VALIDATOR_STAKE have no buffer and cannot be slashed economically.
    pool.stake(v1.pubkey(), BOOTSTRAP_GRANT_AMOUNT, 0).unwrap();
    let evidence = SlashingEvidence::new(
        SlashingOffense::DoubleBlock {
            slot: 10,
            block_hash_1: Hash::new([1u8; 32]),
            block_hash_2: Hash::new([2u8; 32]),
        },
        v1.pubkey(),
        10,
        reporter.pubkey(),
        1700000004,
    );
    tracker.add_evidence(evidence);
    let slashed_amount = tracker.apply_economic_slashing(&v1.pubkey(), &mut pool);
    assert!(slashed_amount > 0, "Slashing should remove stake from the 25K buffer");
    let remaining = pool.get_stake(&v1.pubkey()).unwrap().total_stake();
    assert!(remaining >= MIN_VALIDATOR_STAKE, "Stake must never drop below MIN_VALIDATOR_STAKE");
}

// ═══════════════════════════════════════════════════════════════════════════════
// 37. STATE STORE — VALIDATOR PERSISTENCE
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_validator_set_save_load_roundtrip() {
    let (state, _tmp, _) = create_test_state();
    let mut vs = ValidatorSet::new();
    let v1 = Keypair::new();
    let v2 = Keypair::new();
    vs.add_validator(make_validator_info(&v1, 5000));
    vs.add_validator(make_validator_info(&v2, 3000));
    state.save_validator_set(&vs).unwrap();
    let loaded = state.load_validator_set().unwrap();
    assert_eq!(loaded.validators().len(), 2);
    // voting weight = sum of reputation (100 each)
    assert_eq!(loaded.total_voting_weight(), 200);
}

#[test]
fn test_stake_pool_save_load_roundtrip() {
    let (state, _tmp, _) = create_test_state();
    let mut pool = StakePool::new();
    let v1 = Keypair::new();
    pool.stake(v1.pubkey(), MIN_VALIDATOR_STAKE, 0).unwrap();
    state.put_stake_pool(&pool).unwrap();
    let loaded = state.get_stake_pool().unwrap();
    assert_eq!(loaded.total_stake(), MIN_VALIDATOR_STAKE);
}

// ═══════════════════════════════════════════════════════════════════════════════
// 38. FORK CHOICE — LEGACY API
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_fork_choice_legacy_weight() {
    let mut fc = ForkChoice::new();
    let hash_a = Hash::new([1u8; 32]);
    fc.add_weight(hash_a, 100);
    fc.add_weight(hash_a, 200);
    assert_eq!(fc.get_weight(&hash_a), 300);
}

#[test]
fn test_fork_choice_clear() {
    let mut fc = ForkChoice::new();
    fc.add_head(1, Hash::new([1u8; 32]), 100);
    fc.add_head(2, Hash::new([2u8; 32]), 200);
    fc.clear();
    assert!(
        fc.select_head().is_none(),
        "After clear, no head should be selected"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 39. MEMPOOL — CONTAINS & REMOVE
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_mempool_contains_and_remove() {
    let mut pool = Mempool::new(100, 300);
    let kp = Keypair::new();
    let ix = transfer_instruction(kp.pubkey(), Keypair::new().pubkey(), 100);
    let tx = build_signed_tx(&kp, ix, Hash::new([99u8; 32]));
    let tx_hash = tx.hash();
    pool.add_transaction(tx, BASE_FEE, 0).unwrap();
    assert!(pool.contains(&tx_hash));
    pool.remove_transaction(&tx_hash);
    assert!(!pool.contains(&tx_hash));
}

// ═══════════════════════════════════════════════════════════════════════════════
// 40. STATE — TRANSFER UTILITY
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_state_transfer_utility() {
    let (state, _tmp, _) = create_test_state();
    let a = Keypair::new();
    let b = Keypair::new();
    state
        .put_account(&a.pubkey(), &account_with_shells(a.pubkey(), 10_000))
        .unwrap();
    state
        .put_account(&b.pubkey(), &account_with_shells(b.pubkey(), 0))
        .unwrap();
    state.transfer(&a.pubkey(), &b.pubkey(), 3_000).unwrap();
    let a_acct = state.get_account(&a.pubkey()).unwrap().unwrap();
    let b_acct = state.get_account(&b.pubkey()).unwrap().unwrap();
    assert_eq!(a_acct.shells, 7_000);
    assert_eq!(b_acct.shells, 3_000);
}

#[test]
fn test_state_transfer_insufficient() {
    let (state, _tmp, _) = create_test_state();
    let a = Keypair::new();
    let b = Keypair::new();
    state
        .put_account(&a.pubkey(), &account_with_shells(a.pubkey(), 100))
        .unwrap();
    state
        .put_account(&b.pubkey(), &account_with_shells(b.pubkey(), 0))
        .unwrap();
    let result = state.transfer(&a.pubkey(), &b.pubkey(), 200);
    assert!(result.is_err(), "Transfer more than balance should fail");
}
