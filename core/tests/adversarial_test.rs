// MoltChain Adversarial & Security Tests
// Attack simulations and edge case testing

use moltchain_core::{
    Account, Block, BlockHeader, Hash, Instruction, Keypair, Message, Pubkey, SlashingEvidence,
    SlashingOffense, SlashingTracker, StateStore, Transaction, TxProcessor, ValidatorInfo,
    ValidatorSet, Vote, BASE_FEE, SYSTEM_PROGRAM_ID,
};
use std::time::SystemTime;
use tempfile::TempDir;

fn create_test_state() -> (StateStore, TempDir, Hash) {
    let temp_dir = TempDir::new().unwrap();
    let state = StateStore::open(temp_dir.path()).unwrap();
    let treasury = Keypair::new();
    let treasury_account = account_with_shells(treasury.pubkey(), 10_000_000_000_000);
    state
        .put_account(&treasury.pubkey(), &treasury_account)
        .unwrap();
    state.set_treasury_pubkey(&treasury.pubkey()).unwrap();

    // Store a genesis block so get_recent_blockhashes returns a real hash
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
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

fn make_vote(signer: &Keypair, slot: u64, block_hash: Hash) -> Vote {
    let mut message = Vec::new();
    message.extend_from_slice(&slot.to_le_bytes());
    message.extend_from_slice(&block_hash.0);
    let signature = signer.sign(&message);

    Vote {
        validator: signer.pubkey(),
        slot,
        block_hash,
        signature,
        timestamp: now_ms(),
    }
}

#[test]
fn test_double_spend_attack() {
    let (state, _temp_dir, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let attacker = Keypair::new();
    let victim1 = Keypair::new();
    let victim2 = Keypair::new();
    let validator = Keypair::new();

    // Attacker starts with 2 MOLT
    let attacker_balance = 2_000_000_000 + BASE_FEE * 2;
    state
        .put_account(
            &attacker.pubkey(),
            &account_with_shells(attacker.pubkey(), attacker_balance),
        )
        .unwrap();
    state
        .put_account(&victim1.pubkey(), &Account::new(0, victim1.pubkey()))
        .unwrap();
    state
        .put_account(&victim2.pubkey(), &Account::new(0, victim2.pubkey()))
        .unwrap();

    // Try to spend 2 MOLT twice to different victims
    let amount = 2_000_000_000u64;

    // Transaction 1: Send 2 MOLT to victim1
    let mut data1 = vec![0u8];
    data1.extend_from_slice(&amount.to_le_bytes());
    let tx1 = build_signed_tx(
        &attacker,
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![attacker.pubkey(), victim1.pubkey()],
            data: data1,
        },
        genesis_hash,
    );

    // Transaction 2: Try to send same 2 MOLT to victim2
    let mut data2 = vec![0u8];
    data2.extend_from_slice(&amount.to_le_bytes());
    let tx2 = build_signed_tx(
        &attacker,
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![attacker.pubkey(), victim2.pubkey()],
            data: data2,
        },
        genesis_hash,
    );

    // First transaction succeeds
    let result1 = processor.process_transaction(&tx1, &validator.pubkey());
    assert!(result1.success, "First transaction should succeed");

    // Second transaction MUST fail (double spend attempt)
    let result2 = processor.process_transaction(&tx2, &validator.pubkey());
    assert!(!result2.success, "Double spend must be prevented!");

    // Verify only victim1 received funds
    assert_eq!(state.get_balance(&victim1.pubkey()).unwrap(), amount);
    assert_eq!(state.get_balance(&victim2.pubkey()).unwrap(), 0);
}

#[test]
fn test_invalid_signature_attack() {
    let (state, _temp_dir, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let victim = Keypair::new();
    let attacker = Keypair::new();
    let validator = Keypair::new();

    // Victim has funds
    state
        .put_account(&victim.pubkey(), &Account::new(10, victim.pubkey()))
        .unwrap();
    state
        .put_account(&attacker.pubkey(), &Account::new(0, attacker.pubkey()))
        .unwrap();

    // Attacker tries to steal victim's funds by forging signature
    let stolen_amount = 5_000_000_000u64;
    let mut data = vec![0u8];
    data.extend_from_slice(&stolen_amount.to_le_bytes());

    let forged_tx = build_signed_tx(
        &attacker,
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![victim.pubkey(), attacker.pubkey()],
            data,
        },
        genesis_hash,
    );

    // Must fail - signature doesn't match account
    let result = processor.process_transaction(&forged_tx, &validator.pubkey());
    assert!(!result.success, "Forged signature must be rejected!");
    assert!(result.error.as_ref().unwrap().contains("signature"));

    // Victim's funds untouched
    assert_eq!(
        state.get_balance(&victim.pubkey()).unwrap(),
        Account::molt_to_shells(10)
    );
    assert_eq!(state.get_balance(&attacker.pubkey()).unwrap(), 0);
}

#[test]
fn test_overflow_attack() {
    let (state, _temp_dir, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let attacker = Keypair::new();
    let target = Keypair::new();
    let validator = Keypair::new();

    // Set up accounts
    state
        .put_account(&attacker.pubkey(), &Account::new(1, attacker.pubkey()))
        .unwrap();
    let target_account = Account {
        shells: u64::MAX - 100,
        spendable: u64::MAX - 100,
        staked: 0,
        locked: 0,
        data: Vec::new(),
        owner: target.pubkey(),
        executable: false,
        rent_epoch: 0,
    };
    state
        .put_account(&target.pubkey(), &target_account)
        .unwrap();

    // Try to overflow target's balance
    let overflow_amount = 1000u64;
    let mut data = vec![0u8];
    data.extend_from_slice(&overflow_amount.to_le_bytes());

    let tx = build_signed_tx(
        &attacker,
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![attacker.pubkey(), target.pubkey()],
            data,
        },
        genesis_hash,
    );

    let result = processor.process_transaction(&tx, &validator.pubkey());

    // Should handle overflow gracefully (either succeed with saturation or fail safely)
    if result.success {
        let balance = state.get_balance(&target.pubkey()).unwrap();
        // Balance should not overflow
        assert!(balance >= u64::MAX - 100);
    }
}

#[test]
fn test_zero_amount_transaction() {
    let (state, _temp_dir, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let sender = Keypair::new();
    let receiver = Keypair::new();
    let validator = Keypair::new();

    state
        .put_account(&sender.pubkey(), &Account::new(10, sender.pubkey()))
        .unwrap();
    state
        .put_account(&receiver.pubkey(), &Account::new(0, receiver.pubkey()))
        .unwrap();

    // Try to send 0 shells
    let mut data = vec![0u8];
    data.extend_from_slice(&0u64.to_le_bytes());

    let tx = build_signed_tx(
        &sender,
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![sender.pubkey(), receiver.pubkey()],
            data,
        },
        genesis_hash,
    );

    // Should handle gracefully (pay fee, no transfer)
    let initial_sender = state.get_balance(&sender.pubkey()).unwrap();
    let result = processor.process_transaction(&tx, &validator.pubkey());

    if result.success {
        let final_sender = state.get_balance(&sender.pubkey()).unwrap();
        // Should only deduct fee
        assert_eq!(final_sender, initial_sender - BASE_FEE);
        assert_eq!(state.get_balance(&receiver.pubkey()).unwrap(), 0);
    }
}

#[test]
fn test_self_transfer() {
    let (state, _temp_dir, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let user = Keypair::new();
    let validator = Keypair::new();

    let initial_molt = 10;
    let initial_balance = Account::molt_to_shells(initial_molt);
    state
        .put_account(&user.pubkey(), &Account::new(initial_molt, user.pubkey()))
        .unwrap();

    // Transfer to self
    let amount = 1_000_000_000u64;
    let mut data = vec![0u8];
    data.extend_from_slice(&amount.to_le_bytes());

    let tx = build_signed_tx(
        &user,
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![user.pubkey(), user.pubkey()],
            data,
        },
        genesis_hash,
    );

    let result = processor.process_transaction(&tx, &validator.pubkey());

    // Should succeed but only pay fee
    assert!(result.success);
    let final_balance = state.get_balance(&user.pubkey()).unwrap();
    assert_eq!(final_balance, initial_balance - BASE_FEE);
}

#[test]
fn test_validator_double_voting_slashing() {
    let (_state, _temp_dir, _genesis_hash) = create_test_state();

    let validator = Keypair::new();
    let validator_info = ValidatorInfo::new(validator.pubkey(), 0);

    let mut validator_set = ValidatorSet::new();
    validator_set.add_validator(validator_info);

    let mut slashing_tracker = SlashingTracker::new();

    // Validator votes for slot 100 with hash1
    let hash1 = Hash::hash(b"block1");
    let vote1 = make_vote(&validator, 100, hash1);

    // Validator votes for same slot with different hash (double vote!)
    let hash2 = Hash::hash(b"block2");
    let mut vote2 = make_vote(&validator, 100, hash2);
    vote2.timestamp = now_ms() + 1;

    // Create evidence of double voting
    let reporter = Keypair::new();
    let evidence = SlashingEvidence::new(
        SlashingOffense::DoubleVote {
            slot: 100,
            vote_1: vote1.clone(),
            vote_2: vote2.clone(),
        },
        validator.pubkey(),
        100,
        reporter.pubkey(),
    );

    // Submit evidence
    assert!(slashing_tracker.add_evidence(evidence));

    // Verify evidence was recorded correctly
    // In a real system, this would trigger stake slashing
    // The validator should lose a portion of their stake
}

#[test]
fn test_rapid_transaction_spam() {
    let (state, _temp_dir, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let spammer = Keypair::new();
    let target = Keypair::new();
    let validator = Keypair::new();

    // Fund spammer heavily
    let initial_molt = 1u64;
    state
        .put_account(
            &spammer.pubkey(),
            &Account::new(initial_molt, spammer.pubkey()),
        )
        .unwrap();
    state
        .put_account(&target.pubkey(), &Account::new(0, target.pubkey()))
        .unwrap();

    let base_amount = 100_000_000u64; // 0.1 MOLT in shells

    // Try to spam 100 transactions rapidly
    let mut successful_txs = 0;
    for _i in 0..100 {
        let amount = base_amount.saturating_add(_i as u64);
        let mut data = vec![0u8];
        data.extend_from_slice(&amount.to_le_bytes());

        let tx = build_signed_tx(
            &spammer,
            Instruction {
                program_id: SYSTEM_PROGRAM_ID,
                accounts: vec![spammer.pubkey(), target.pubkey()],
                data,
            },
            genesis_hash,
        );

        let result = processor.process_transaction(&tx, &validator.pubkey());
        if result.success {
            successful_txs += 1;
        } else {
            // Eventually should fail due to balance exhaustion
            break;
        }
    }

    // Should process some transactions but eventually run out of balance for fees
    assert!(
        successful_txs > 0,
        "Should process at least some transactions"
    );
    assert!(
        successful_txs < 100,
        "Should not process all spam transactions"
    );

    // Verify spammer cannot afford another spam transfer
    let remaining = state.get_balance(&spammer.pubkey()).unwrap();
    assert!(
        remaining < base_amount + BASE_FEE,
        "Spammer should be rate-limited by balance"
    );
}

#[test]
fn test_malformed_instruction_data() {
    let (state, _temp_dir, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let sender = Keypair::new();
    let validator = Keypair::new();

    state
        .put_account(&sender.pubkey(), &Account::new(10, sender.pubkey()))
        .unwrap();

    // Send instruction with invalid data (too short)
    let tx = build_signed_tx(
        &sender,
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![sender.pubkey(), Keypair::new().pubkey()],
            data: vec![0, 1, 2],
        },
        genesis_hash,
    );

    let result = processor.process_transaction(&tx, &validator.pubkey());
    assert!(!result.success, "Malformed instruction should be rejected");
}

#[test]
fn test_empty_transaction() {
    let (state, _temp_dir, _genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let sender = Keypair::new();
    let validator = Keypair::new();

    state
        .put_account(&sender.pubkey(), &Account::new(10, sender.pubkey()))
        .unwrap();

    // Transaction with no instructions (uses zero blockhash to trigger early rejection)
    let message = Message::new(Vec::new(), Hash::default());
    let signature = sender.sign(&message.serialize());
    let tx = Transaction {
        signatures: vec![signature],
        message,
    };

    let result = processor.process_transaction(&tx, &validator.pubkey());
    // Should fail: zero blockhash OR no instructions
    assert!(!result.success);
}

#[test]
fn test_unauthorized_program_id() {
    let (state, _temp_dir, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let sender = Keypair::new();
    let validator = Keypair::new();

    state
        .put_account(&sender.pubkey(), &Account::new(10, sender.pubkey()))
        .unwrap();

    // Try to call a non-existent program
    let fake_program = Pubkey([0xFEu8; 32]);

    let tx = build_signed_tx(
        &sender,
        Instruction {
            program_id: fake_program,
            accounts: vec![sender.pubkey()],
            data: vec![1, 2, 3],
        },
        genesis_hash,
    );

    let result = processor.process_transaction(&tx, &validator.pubkey());
    assert!(!result.success, "Unknown program should be rejected");
    assert!(result.error.as_ref().unwrap().contains("Unknown program"));
}

#[test]
fn test_replay_attack_prevention() {
    let (state, _temp_dir, genesis_hash) = create_test_state();
    let processor = TxProcessor::new(state.clone());

    let sender = Keypair::new();
    let receiver = Keypair::new();
    let validator = Keypair::new();

    state
        .put_account(&sender.pubkey(), &Account::new(10, sender.pubkey()))
        .unwrap();
    state
        .put_account(&receiver.pubkey(), &Account::new(0, receiver.pubkey()))
        .unwrap();

    // Create transaction
    let amount = 1_000_000_000u64;
    let mut data = vec![0u8];
    data.extend_from_slice(&amount.to_le_bytes());

    let tx = build_signed_tx(
        &sender,
        Instruction {
            program_id: SYSTEM_PROGRAM_ID,
            accounts: vec![sender.pubkey(), receiver.pubkey()],
            data,
        },
        genesis_hash,
    );

    // Process once - should succeed
    let result1 = processor.process_transaction(&tx, &validator.pubkey());
    assert!(result1.success);

    let receiver_balance_after_first = state.get_balance(&receiver.pubkey()).unwrap();
    assert_eq!(receiver_balance_after_first, amount);

    // Try to replay same transaction - should fail (insufficient balance)
    let _result2 = processor.process_transaction(&tx, &validator.pubkey());

    // Either fails due to balance OR transaction deduplication
    // At minimum, receiver shouldn't get paid twice
    let receiver_balance_final = state.get_balance(&receiver.pubkey()).unwrap();
    assert_eq!(receiver_balance_final, amount, "Replay attack succeeded!");
}

#[test]
fn test_byzantine_block_production() {
    let (state, _temp_dir, _genesis_hash) = create_test_state();

    let byzantine_validator = Keypair::new();

    // Byzantine validator tries to produce two blocks for same slot
    let slot = 100;

    let block1 = Block {
        header: BlockHeader {
            slot,
            parent_hash: Hash::default(),
            state_root: Hash::default(),
            tx_root: Hash::default(),
            timestamp: now_ms(),
            validator: byzantine_validator.pubkey().0,
            signature: [0u8; 64],
        },
        transactions: vec![],
    };

    let block2 = Block {
        header: BlockHeader {
            slot, // Same slot!
            parent_hash: Hash::default(),
            state_root: Hash::hash(b"different"),
            tx_root: Hash::default(),
            timestamp: now_ms() + 1,
            validator: byzantine_validator.pubkey().0,
            signature: [0u8; 64],
        },
        transactions: vec![],
    };

    // Store first block
    state.put_block(&block1).unwrap();

    // Try to store second block for same slot
    // In production, this should trigger slashing
    let hash1 = block1.hash();
    let hash2 = block2.hash();

    assert_ne!(hash1, hash2, "Byzantine blocks have different hashes");

    // Create evidence of double block production
    let reporter = Keypair::new();
    let evidence = SlashingEvidence::new(
        SlashingOffense::DoubleBlock {
            slot,
            block_hash_1: hash1,
            block_hash_2: hash2,
        },
        byzantine_validator.pubkey(),
        slot,
        reporter.pubkey(),
    );

    // This evidence should lead to slashing
    let mut tracker = SlashingTracker::new();
    assert!(tracker.add_evidence(evidence.clone()));

    // Verify evidence cannot be added twice
    assert!(!tracker.add_evidence(evidence));
}
