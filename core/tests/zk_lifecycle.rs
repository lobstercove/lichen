// ═══════════════════════════════════════════════════════════════════════════════
// ZK Privacy Fail-Closed Integration Tests
//
// These tests exercise the disabled shielded pool pipeline end-to-end:
//   1. Generate legacy scheme-0x01 STARK proof envelopes
//   2. Process transactions through the TxProcessor with real state
//   3. Verify the unconstrained scheme is rejected without custody mutation
//   4. Retain structural and precondition rejection coverage
//
// Each test performs full cryptographic operations so execution is slow
// (~30–60 seconds per test on commodity hardware).
// ═══════════════════════════════════════════════════════════════════════════════

use lichen_core::zk::circuits::shield::ShieldCircuit;
use lichen_core::zk::{
    commitment_hash, random_scalar_bytes, recipient_hash, recipient_preimage_from_bytes, Prover,
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
// Test 1: Legacy Scheme Is Disabled Without State Mutation
//
// Proves: even a proof envelope produced from a valid local witness is refused
// until a constrained verifier version is activated, and no custody state is
// changed by the failed transaction.
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_legacy_shield_proof_is_disabled_without_state_mutation() {
    let env = create_test_env();
    let validator = Pubkey([42u8; 32]);

    let prover = Prover::new();

    let shield_amount = 500_000_000u64; // 0.5 LICN
    let blinding = random_scalar_bytes();
    let commitment_bytes = commitment_hash(shield_amount, &blinding);

    let shield_circuit =
        ShieldCircuit::new_bytes(shield_amount, shield_amount, blinding, commitment_bytes);
    let shield_proof = prover.prove_shield(shield_circuit).expect("prove shield");

    let alice_before = env.state.get_account(&env.alice).unwrap().unwrap();
    let pool_before = env.state.get_shielded_pool_state().unwrap();
    let shield_tx = make_shield_tx(
        &env,
        shield_amount,
        &commitment_bytes,
        &shield_proof.proof_bytes,
    );
    let shield_result = env.processor.process_transaction(&shield_tx, &validator);
    assert!(!shield_result.success, "scheme 0x01 must fail closed");
    assert!(shield_result.fee_paid > 0);
    assert!(
        shield_result
            .error
            .as_deref()
            .is_some_and(|error| error.contains("disabled for shield")),
        "unexpected error: {:?}",
        shield_result.error
    );

    let mut expected_alice_after = alice_before.clone();
    expected_alice_after
        .deduct_spendable(shield_result.fee_paid)
        .expect("failed transaction fee is affordable");
    assert_eq!(
        lichen_core::codec::serialize_legacy_bincode(
            &env.state.get_account(&env.alice).unwrap().unwrap(),
            "account after disabled shield",
        )
        .unwrap(),
        lichen_core::codec::serialize_legacy_bincode(
            &expected_alice_after,
            "expected account after disabled shield fee",
        )
        .unwrap()
    );
    assert_eq!(
        env.state.get_shielded_pool_state().unwrap().merkle_root,
        pool_before.merkle_root
    );
    assert_eq!(
        env.state
            .get_shielded_pool_state()
            .unwrap()
            .commitment_count,
        0
    );
    assert_eq!(
        env.state.get_shielded_pool_state().unwrap().total_shielded,
        0
    );
    assert_eq!(env.state.get_shielded_commitment(0).unwrap(), None);
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
    let amount = 200_000_000u64;
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
// Test 4: Repeated Disabled Proofs Cannot Accumulate State
//
// Proves: retries of the disabled scheme remain deterministic and do not
// insert commitments or change the pool balance.
// ═══════════════════════════════════════════════════════════════════════════════

#[test]
fn test_repeated_disabled_shields_do_not_accumulate_state() {
    let env = create_test_env();
    let validator = Pubkey([42u8; 32]);
    let prover = Prover::new();

    let amounts = [100_000_000u64, 250_000_000u64, 150_000_000u64];
    let account_before = env.state.get_account(&env.alice).unwrap().unwrap();
    let pool_before = env.state.get_shielded_pool_state().unwrap();
    let mut total_fees = 0u64;

    for (i, &amount) in amounts.iter().enumerate() {
        let blinding = random_scalar_bytes();
        let commitment_bytes = commitment_hash(amount, &blinding);

        let circuit = ShieldCircuit::new_bytes(amount, amount, blinding, commitment_bytes);
        let proof = prover.prove_shield(circuit).unwrap();
        let tx = make_shield_tx(&env, amount, &commitment_bytes, &proof.proof_bytes);
        let result = env.processor.process_transaction(&tx, &validator);
        assert!(!result.success, "Shield {i} must fail closed");
        assert!(result.fee_paid > 0);
        total_fees = total_fees.checked_add(result.fee_paid).unwrap();
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|error| error.contains("disabled for shield")),
            "unexpected error: {:?}",
            result.error
        );

        let pool = env.state.get_shielded_pool_state().unwrap();
        assert_eq!(pool.commitment_count, 0);
        assert_eq!(pool.total_shielded, 0);
        assert_eq!(pool.merkle_root, pool_before.merkle_root);
        let stored = env.state.get_shielded_commitment(i as u64).unwrap();
        assert_eq!(stored, None);
    }

    let mut expected_account_after = account_before.clone();
    expected_account_after
        .deduct_spendable(total_fees)
        .expect("failed transaction fees are affordable");
    assert_eq!(
        lichen_core::codec::serialize_legacy_bincode(
            &env.state.get_account(&env.alice).unwrap().unwrap(),
            "account after repeated disabled shields",
        )
        .unwrap(),
        lichen_core::codec::serialize_legacy_bincode(
            &expected_account_after,
            "expected account after repeated disabled shield fees",
        )
        .unwrap()
    );
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
