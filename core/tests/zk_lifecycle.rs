// ═══════════════════════════════════════════════════════════════════════════════
// ZK Privacy Full Lifecycle Integration Tests
//
// These tests exercise the complete shielded pool pipeline end-to-end:
//   1. Generate native STARK proofs for shield/unshield operations
//   2. Process transactions through the TxProcessor with real state
//   3. Verify all state changes (balances, commitments, nullifiers, merkle root)
//   4. Verify security properties (double-spend rejection, invalid proofs)
//
// Each test performs full cryptographic operations so execution is slow
// (~30–60 seconds per test on commodity hardware).
// ═══════════════════════════════════════════════════════════════════════════════

use lichen_core::zk::circuits::shield::ShieldCircuit;
use lichen_core::zk::circuits::unshield::UnshieldCircuit;
use lichen_core::zk::{
    commitment_hash, nullifier_hash, random_scalar_bytes, recipient_hash,
    recipient_preimage_from_bytes, MerkleTree, Prover,
};
use lichen_core::*;

// ─────────────────────────────────────────────────────────────────────────────
// Test helpers
// ─────────────────────────────────────────────────────────────────────────────

struct TestEnv {
    processor: TxProcessor,
    state: StateStore,
    alice_kp: Keypair,
    alice: Pubkey,
    genesis_hash: Hash,
}

fn create_test_env() -> TestEnv {
    let dir = tempfile::tempdir().unwrap();
    let state = StateStore::open(dir.path()).unwrap();
    let processor = TxProcessor::new(state.clone());

    let alice_kp = Keypair::generate();
    let alice = alice_kp.pubkey();
    let treasury = Pubkey([3u8; 32]);

    state.set_treasury_pubkey(&treasury).unwrap();
    state
        .put_account(&treasury, &Account::new(0, treasury))
        .unwrap();

    // Fund alice with 10 LICN (10 billion spores)
    let alice_account = Account::new(10_000, alice);
    state.put_account(&alice, &alice_account).unwrap();

    // Store a genesis block
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

    // Leak the dir so the DB stays valid for the test duration
    let _ = Box::leak(Box::new(dir));

    TestEnv {
        processor,
        state,
        alice_kp,
        alice,
        genesis_hash,
    }
}

fn make_shield_tx(
    env: &TestEnv,
    amount: u64,
    commitment: &[u8; 32],
    proof_bytes: &[u8],
) -> Transaction {
    let mut data = vec![23u8];
    data.extend_from_slice(&amount.to_le_bytes());
    data.extend_from_slice(commitment);
    data.extend_from_slice(proof_bytes);

    let ix = Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![env.alice],
        data,
    };
    let msg = transaction::Message::new(vec![ix], env.genesis_hash);
    let mut tx = Transaction::new(msg);
    tx.signatures
        .push(env.alice_kp.sign(&tx.message.serialize()));
    tx
}

fn make_unshield_tx(
    env: &TestEnv,
    amount: u64,
    nullifier: &[u8; 32],
    merkle_root: &[u8; 32],
    recipient_public_bytes: &[u8; 32],
    proof_bytes: &[u8],
) -> Transaction {
    let mut data = vec![24u8];
    data.extend_from_slice(&amount.to_le_bytes());
    data.extend_from_slice(nullifier);
    data.extend_from_slice(merkle_root);
    data.extend_from_slice(recipient_public_bytes);
    data.extend_from_slice(proof_bytes);

    let ix = Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![env.alice],
        data,
    };
    let msg = transaction::Message::new(vec![ix], env.genesis_hash);
    let mut tx = Transaction::new(msg);
    tx.signatures
        .push(env.alice_kp.sign(&tx.message.serialize()));
    tx
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 1: Full Shield → Unshield Lifecycle
//
// Proves: shield deposits into the pool, unshield withdraws back, balances
// and pool state update correctly at every step, Merkle tree is consistent.
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_shield_then_unshield_full_lifecycle() {
    let env = create_test_env();
    let validator = Pubkey([42u8; 32]);

    let prover = Prover::new();

    // ── Step 2: Shield 0.5 LICN ─────────────────────────────────────────
    let shield_amount = 500_000_000u64; // 0.5 LICN
    let blinding = random_scalar_bytes();
    let commitment_bytes = commitment_hash(shield_amount, &blinding);

    let shield_circuit =
        ShieldCircuit::new_bytes(shield_amount, shield_amount, blinding, commitment_bytes);
    let shield_proof = prover.prove_shield(shield_circuit).expect("prove shield");

    let alice_balance_before = env.state.get_balance(&env.alice).unwrap();
    let shield_tx = make_shield_tx(
        &env,
        shield_amount,
        &commitment_bytes,
        &shield_proof.proof_bytes,
    );
    let shield_result = env.processor.process_transaction(&shield_tx, &validator);
    assert!(
        shield_result.success,
        "Shield should succeed: {:?}",
        shield_result.error
    );

    // ── Step 3: Verify state after shield ───────────────────────────────
    let alice_balance_after_shield = env.state.get_balance(&env.alice).unwrap();
    assert_eq!(
        alice_balance_before - alice_balance_after_shield - shield_result.fee_paid,
        shield_amount,
        "Balance decrease (minus fee) should equal shielded amount"
    );

    let pool_after_shield = env.state.get_shielded_pool_state().unwrap();
    assert_eq!(pool_after_shield.commitment_count, 1);
    assert_eq!(pool_after_shield.total_shielded, shield_amount);

    // Verify commitment is stored correctly
    let stored = env.state.get_shielded_commitment(0).unwrap();
    assert_eq!(stored, Some(commitment_bytes));

    // Verify Merkle root is correct (single-leaf tree)
    let mut expected_tree = MerkleTree::new();
    expected_tree.insert(commitment_bytes);
    assert_eq!(
        pool_after_shield.merkle_root,
        expected_tree.root(),
        "Merkle root should match single-leaf tree"
    );

    // ── Step 4: Unshield the same amount ────────────────────────────────
    // Derive secrets for unshield
    let serial = random_scalar_bytes();
    let spending_key = random_scalar_bytes();
    let nullifier_bytes = nullifier_hash(&serial, &spending_key);

    let recipient_preimage = recipient_preimage_from_bytes(env.alice.0);
    let recipient_public_bytes = recipient_hash(&recipient_preimage);

    // Get Merkle path for the commitment we just shielded
    let proof_path = expected_tree.proof(0).unwrap();
    let merkle_path = proof_path.siblings.clone();

    // Build and prove unshield circuit
    let unshield_circuit = UnshieldCircuit::new_bytes(
        pool_after_shield.merkle_root,
        nullifier_bytes,
        shield_amount,
        recipient_public_bytes,
        shield_amount,
        blinding,
        serial,
        spending_key,
        recipient_preimage,
        merkle_path,
        proof_path.path_bits,
    );
    let unshield_proof = prover
        .prove_unshield(unshield_circuit)
        .expect("prove unshield");

    let unshield_tx = make_unshield_tx(
        &env,
        shield_amount,
        &nullifier_bytes,
        &pool_after_shield.merkle_root,
        &recipient_public_bytes,
        &unshield_proof.proof_bytes,
    );
    let unshield_result = env.processor.process_transaction(&unshield_tx, &validator);
    assert!(
        unshield_result.success,
        "Unshield should succeed: {:?}",
        unshield_result.error
    );

    // ── Step 5: Verify state after unshield ──────────────────────────────
    let alice_balance_after_unshield = env.state.get_balance(&env.alice).unwrap();
    // After shield+unshield, alice should have: original - shield_fee - unshield_fee
    // (the shielded amount goes back to her via credit)
    let total_fees = shield_result.fee_paid + unshield_result.fee_paid;
    assert_eq!(
        alice_balance_after_unshield,
        alice_balance_before - total_fees,
        "After shield+unshield cycle, balance should only lose fees"
    );

    let pool_after_unshield = env.state.get_shielded_pool_state().unwrap();
    assert_eq!(
        pool_after_unshield.total_shielded, 0,
        "Pool should be empty after unshielding everything"
    );
    assert_eq!(
        pool_after_unshield.commitment_count, 1,
        "Commitment count should stay at 1 (commitments are never removed)"
    );

    // Nullifier should be marked as spent
    assert!(
        env.state.is_nullifier_spent(&nullifier_bytes).unwrap(),
        "Nullifier should be marked spent after unshield"
    );

    // ── Step 6: Double-spend attempt → must be rejected ─────────────────
    // Use a different recent_blockhash so the tx isn't flagged as a duplicate.
    // Create a second block to get a fresh hash.
    let block1 = Block::new_with_timestamp(
        1,
        env.genesis_hash,
        Hash::hash(b"block-1"),
        [0u8; 32],
        Vec::new(),
        1,
    );
    let block1_hash = block1.hash();
    env.state.put_block(&block1).unwrap();
    env.state.set_last_slot(1).unwrap();

    // Build a NEW unshield tx with the same nullifier but different blockhash
    let mut dupe_data = vec![24u8];
    dupe_data.extend_from_slice(&shield_amount.to_le_bytes());
    dupe_data.extend_from_slice(&nullifier_bytes);
    dupe_data.extend_from_slice(&pool_after_unshield.merkle_root);
    dupe_data.extend_from_slice(&recipient_public_bytes);
    dupe_data.extend_from_slice(&unshield_proof.proof_bytes);

    let dupe_ix = Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![env.alice],
        data: dupe_data,
    };
    let dupe_msg = transaction::Message::new(vec![dupe_ix], block1_hash);
    let mut unshield_tx2 = Transaction::new(dupe_msg);
    unshield_tx2
        .signatures
        .push(env.alice_kp.sign(&unshield_tx2.message.serialize()));

    let dupe_result = env.processor.process_transaction(&unshield_tx2, &validator);
    assert!(!dupe_result.success, "Double-spend should fail");
    // The processor may reject for "nullifier already spent" OR "insufficient
    // shielded pool balance" (since the pool is now empty) — both are correct.
    let err_msg = dupe_result.error.as_ref().unwrap();
    assert!(
        err_msg.contains("nullifier already spent")
            || err_msg.contains("insufficient")
            || err_msg.contains("merkle root"),
        "Error should reject the double-spend: {:?}",
        dupe_result.error
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 2: Invalid Proof Rejection
//
// Proves: The processor rejects transactions with tampered proof bytes.
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_invalid_proof_bytes_rejected() {
    let env = create_test_env();
    let validator = Pubkey([42u8; 32]);

    // Build a shield transaction with garbage proof bytes
    let amount = 100_000_000u64;
    let commitment_bytes = commitment_hash(amount, &random_scalar_bytes());

    let garbage_proof = vec![0xFFu8; 7];

    let tx = make_shield_tx(&env, amount, &commitment_bytes, &garbage_proof);
    let result = env.processor.process_transaction(&tx, &validator);

    assert!(!result.success, "Garbage proof should be rejected");
    // The error could be proof deserialization or verification failure
    let err = result.error.unwrap();
    assert!(
        err.contains("proof") || err.contains("Shield") || err.contains("verification"),
        "Error should relate to proof: {}",
        err
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 3: Invalid Merkle Root Rejection
//
// Proves: Unshield with a merkle root that doesn't match the pool state fails.
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_wrong_merkle_root_rejected() {
    let env = create_test_env();
    let validator = Pubkey([42u8; 32]);
    let prover = Prover::new();

    // Shield some amount
    let amount = 200_000_000u64;
    let blinding = random_scalar_bytes();
    let commitment_bytes = commitment_hash(amount, &blinding);

    let shield_circuit = ShieldCircuit::new_bytes(amount, amount, blinding, commitment_bytes);
    let shield_proof = prover.prove_shield(shield_circuit).unwrap();
    let shield_tx = make_shield_tx(&env, amount, &commitment_bytes, &shield_proof.proof_bytes);
    let shield_result = env.processor.process_transaction(&shield_tx, &validator);
    assert!(shield_result.success, "Shield should succeed");

    // Now try to unshield with a WRONG merkle root
    let wrong_root = [0xAB; 32];
    let nullifier = random_scalar_bytes();
    let recipient_public_bytes = recipient_hash(&recipient_preimage_from_bytes(env.alice.0));
    let dummy_proof = vec![0u8; 128];

    let tx = make_unshield_tx(
        &env,
        amount,
        &nullifier,
        &wrong_root,
        &recipient_public_bytes,
        &dummy_proof,
    );
    let result = env.processor.process_transaction(&tx, &validator);

    assert!(!result.success, "Wrong merkle root should be rejected");
    assert!(
        result.error.as_ref().unwrap().contains("merkle root"),
        "Error should mention merkle root: {:?}",
        result.error
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 4: Pool State Consistency Across Multiple Shields
//
// Proves: Multiple shield operations maintain correct pool state,
// monotonically increasing commitment count, and accurate Merkle root.
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_multiple_shields_maintain_consistent_pool_state() {
    let env = create_test_env();
    let validator = Pubkey([42u8; 32]);
    let prover = Prover::new();

    let amounts = [100_000_000u64, 250_000_000u64, 150_000_000u64];
    let mut expected_tree = MerkleTree::new();
    let mut total_shielded = 0u64;

    for (i, &amount) in amounts.iter().enumerate() {
        let blinding = random_scalar_bytes();
        let commitment_bytes = commitment_hash(amount, &blinding);

        let circuit = ShieldCircuit::new_bytes(amount, amount, blinding, commitment_bytes);
        let proof = prover.prove_shield(circuit).unwrap();
        let tx = make_shield_tx(&env, amount, &commitment_bytes, &proof.proof_bytes);
        let result = env.processor.process_transaction(&tx, &validator);
        assert!(
            result.success,
            "Shield {} should succeed: {:?}",
            i, result.error
        );

        expected_tree.insert(commitment_bytes);
        total_shielded += amount;

        // Verify incremental state correctness
        let pool = env.state.get_shielded_pool_state().unwrap();
        assert_eq!(pool.commitment_count, (i + 1) as u64);
        assert_eq!(pool.total_shielded, total_shielded);
        assert_eq!(
            pool.merkle_root,
            expected_tree.root(),
            "Merkle root should match after shield {}",
            i
        );

        // Verify commitment is stored at the correct index
        let stored = env.state.get_shielded_commitment(i as u64).unwrap();
        assert_eq!(stored, Some(commitment_bytes));
    }

    // Verify final state
    let final_pool = env.state.get_shielded_pool_state().unwrap();
    assert_eq!(final_pool.commitment_count, 3);
    assert_eq!(final_pool.total_shielded, 500_000_000); // 100M + 250M + 150M

    // Verify all commitments are retrievable
    let all = env.state.get_all_shielded_commitments(3).unwrap();
    assert_eq!(all.len(), 3);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 5: Shield Zero Amount Rejected
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_shield_zero_amount_rejected() {
    let env = create_test_env();
    let validator = Pubkey([42u8; 32]);

    let commitment = [0x11u8; 32];
    let proof_bytes = vec![0u8; 7];

    let tx = make_shield_tx(&env, 0, &commitment, &proof_bytes);
    let result = env.processor.process_transaction(&tx, &validator);

    assert!(!result.success, "Zero amount shield should fail");
    assert!(
        result.error.as_ref().unwrap().contains("zero")
            || result.error.as_ref().unwrap().contains("non-zero"),
        "Error should mention zero amount: {:?}",
        result.error
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 6: Insufficient Balance for Shield
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_shield_insufficient_balance_rejected() {
    let env = create_test_env();
    let validator = Pubkey([42u8; 32]);
    let prover = Prover::new();

    // Try to shield 100 LICN when alice only has 10 LICN
    let huge_amount = 100_000_000_000_000u64;
    let blinding = random_scalar_bytes();
    let commitment_bytes = commitment_hash(huge_amount, &blinding);

    let circuit = ShieldCircuit::new_bytes(huge_amount, huge_amount, blinding, commitment_bytes);
    let proof = prover.prove_shield(circuit).unwrap();

    let tx = make_shield_tx(&env, huge_amount, &commitment_bytes, &proof.proof_bytes);
    let result = env.processor.process_transaction(&tx, &validator);

    assert!(!result.success, "Shield exceeding balance should fail");
    assert!(
        result.error.as_ref().unwrap().contains("insufficient"),
        "Error should mention insufficient balance: {:?}",
        result.error
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Test 7: Shielded Transfer Data Length Rejection
//
// Verifies short instruction data for transfer (type 25) is rejected cleanly.
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_shielded_transfer_short_data_rejected() {
    let env = create_test_env();
    let validator = Pubkey([42u8; 32]);

    // Type 25 with only 101 bytes total (needs at least 162)
    let mut data = vec![25u8];
    data.extend_from_slice(&[0u8; 100]);

    let ix = Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![env.alice],
        data,
    };
    let msg = transaction::Message::new(vec![ix], env.genesis_hash);
    let mut tx = Transaction::new(msg);
    tx.signatures
        .push(env.alice_kp.sign(&tx.message.serialize()));

    let result = env.processor.process_transaction(&tx, &validator);
    assert!(!result.success);
    assert!(
        result.error.as_ref().unwrap().contains("insufficient data"),
        "Error should mention insufficient data: {:?}",
        result.error
    );
}
