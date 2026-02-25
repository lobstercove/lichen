//! End-to-end prove/verify roundtrip tests.
//!
//! These tests run the full pipeline:
//! 1. Trusted setup → proving key + verification key
//! 2. Build circuit with valid witnesses
//! 3. Generate Groth16 proof (prover)
//! 4. Verify proof (verifier)
//! 5. Reject tampered proofs / wrong public inputs

#[cfg(test)]
mod tests {
    use crate::zk::circuits::shield::ShieldCircuit;
    use crate::zk::circuits::transfer::TransferCircuit;
    use crate::zk::circuits::unshield::UnshieldCircuit;
    use crate::zk::merkle::{fr_to_bytes, poseidon_hash_fr, MerkleTree};
    use crate::zk::prover::Prover;
    use crate::zk::setup;
    use crate::zk::verifier::Verifier;
    use crate::zk::ProofType;
    use ark_bn254::Fr;
    use ark_ff::{PrimeField, UniformRand};
    use ark_std::rand::rngs::OsRng;

    // ── Shield E2E ─────────────────────────────────────────────────

    #[test]
    fn test_shield_e2e_prove_and_verify() {
        // 1. Setup
        let ceremony = setup::setup_shield().expect("shield setup failed");

        // 2. Load keys
        let mut prover = Prover::new();
        prover
            .load_shield_key(&ceremony.proving_key_bytes)
            .expect("load PK");

        let mut verifier = Verifier::new();
        verifier
            .load_shield_vk(&ceremony.verification_key_bytes)
            .expect("load VK");

        // 3. Create circuit
        let amount = 1_000_000_000u64; // 1 MOLT
        let blinding = Fr::rand(&mut OsRng);
        let commitment = poseidon_hash_fr(Fr::from(amount), blinding);

        let circuit = ShieldCircuit::new(amount, amount, blinding, commitment);

        // 4. Prove
        let mut zk_proof = prover.prove_shield(circuit).expect("prove failed");
        assert_eq!(zk_proof.proof_type, ProofType::Shield);
        assert!(!zk_proof.proof_bytes.is_empty());

        // 5. Set public inputs: [amount, commitment]
        zk_proof.public_inputs = vec![fr_to_bytes(&Fr::from(amount)), fr_to_bytes(&commitment)];

        // 6. Verify
        let valid = verifier
            .verify(&zk_proof)
            .expect("verification call failed");
        assert!(valid, "valid shield proof should verify");
    }

    #[test]
    fn test_shield_e2e_wrong_public_input_fails() {
        let ceremony = setup::setup_shield().expect("shield setup failed");

        let mut prover = Prover::new();
        prover.load_shield_key(&ceremony.proving_key_bytes).unwrap();

        let mut verifier = Verifier::new();
        verifier
            .load_shield_vk(&ceremony.verification_key_bytes)
            .unwrap();

        let amount = 500u64;
        let blinding = Fr::rand(&mut OsRng);
        let commitment = poseidon_hash_fr(Fr::from(amount), blinding);

        let circuit = ShieldCircuit::new(amount, amount, blinding, commitment);
        let mut zk_proof = prover.prove_shield(circuit).unwrap();

        // Tamper with the public amount (claim 1000 instead of 500)
        zk_proof.public_inputs = vec![
            fr_to_bytes(&Fr::from(1000u64)), // wrong amount!
            fr_to_bytes(&commitment),
        ];

        let valid = verifier
            .verify(&zk_proof)
            .expect("verification call failed");
        assert!(!valid, "tampered public input should fail verification");
    }

    // ── Unshield E2E ───────────────────────────────────────────────

    #[test]
    fn test_unshield_e2e_prove_and_verify() {
        // 1. Setup
        let ceremony = setup::setup_unshield().expect("unshield setup failed");

        // 2. Load keys
        let mut prover = Prover::new();
        prover
            .load_unshield_key(&ceremony.proving_key_bytes)
            .unwrap();

        let mut verifier = Verifier::new();
        verifier
            .load_unshield_vk(&ceremony.verification_key_bytes)
            .unwrap();

        // 3. Build witnesses
        let amount = 2_000_000_000u64;
        let blinding = Fr::rand(&mut OsRng);
        let serial = Fr::rand(&mut OsRng);
        let spending_key = Fr::rand(&mut OsRng);
        let recipient_preimage = Fr::from(42u64);
        let recipient = poseidon_hash_fr(recipient_preimage, Fr::from(0u64));

        let commitment_fr = poseidon_hash_fr(Fr::from(amount), blinding);
        let nullifier_fr = poseidon_hash_fr(serial, spending_key);

        // Insert into tree
        let mut tree = MerkleTree::new();
        tree.insert(fr_to_bytes(&commitment_fr));
        let merkle_root_fr = Fr::from_le_bytes_mod_order(&tree.root());
        let proof_path = tree.proof(0).unwrap();
        let merkle_path: Vec<Fr> = proof_path
            .siblings
            .iter()
            .map(|s| Fr::from_le_bytes_mod_order(s))
            .collect();

        let circuit = UnshieldCircuit::new(
            merkle_root_fr,
            nullifier_fr,
            amount,
            recipient,
            amount,
            blinding,
            serial,
            spending_key,
            recipient_preimage,
            merkle_path,
            proof_path.path_bits,
        );

        // 4. Prove
        let mut zk_proof = prover.prove_unshield(circuit).unwrap();

        // 5. Set public inputs: [merkle_root, nullifier, amount, recipient]
        zk_proof.public_inputs = vec![
            fr_to_bytes(&merkle_root_fr),
            fr_to_bytes(&nullifier_fr),
            fr_to_bytes(&Fr::from(amount)),
            fr_to_bytes(&recipient),
        ];

        // 6. Verify
        let valid = verifier.verify(&zk_proof).unwrap();
        assert!(valid, "valid unshield proof should verify");
    }

    #[test]
    fn test_unshield_e2e_wrong_nullifier_fails() {
        let ceremony = setup::setup_unshield().unwrap();

        let mut prover = Prover::new();
        prover
            .load_unshield_key(&ceremony.proving_key_bytes)
            .unwrap();

        let mut verifier = Verifier::new();
        verifier
            .load_unshield_vk(&ceremony.verification_key_bytes)
            .unwrap();

        let amount = 1000u64;
        let blinding = Fr::rand(&mut OsRng);
        let serial = Fr::rand(&mut OsRng);
        let spending_key = Fr::rand(&mut OsRng);
        let recipient_preimage = Fr::from(42u64);
        let recipient = poseidon_hash_fr(recipient_preimage, Fr::from(0u64));

        let commitment_fr = poseidon_hash_fr(Fr::from(amount), blinding);
        let nullifier_fr = poseidon_hash_fr(serial, spending_key);

        let mut tree = MerkleTree::new();
        tree.insert(fr_to_bytes(&commitment_fr));
        let merkle_root_fr = Fr::from_le_bytes_mod_order(&tree.root());
        let proof_path = tree.proof(0).unwrap();
        let merkle_path: Vec<Fr> = proof_path
            .siblings
            .iter()
            .map(|s| Fr::from_le_bytes_mod_order(s))
            .collect();

        let circuit = UnshieldCircuit::new(
            merkle_root_fr,
            nullifier_fr,
            amount,
            recipient,
            amount,
            blinding,
            serial,
            spending_key,
            recipient_preimage,
            merkle_path,
            proof_path.path_bits,
        );

        let mut zk_proof = prover.prove_unshield(circuit).unwrap();

        // Tamper: use wrong nullifier in public inputs
        zk_proof.public_inputs = vec![
            fr_to_bytes(&merkle_root_fr),
            fr_to_bytes(&Fr::from(99999u64)), // WRONG nullifier
            fr_to_bytes(&Fr::from(amount)),
            fr_to_bytes(&recipient),
        ];

        let valid = verifier.verify(&zk_proof).unwrap();
        assert!(!valid, "wrong nullifier should fail verification");
    }

    // ── Transfer E2E ───────────────────────────────────────────────

    #[test]
    fn test_transfer_e2e_prove_and_verify() {
        // 1. Setup
        let ceremony = setup::setup_transfer().expect("transfer setup failed");

        // 2. Load keys
        let mut prover = Prover::new();
        prover
            .load_transfer_key(&ceremony.proving_key_bytes)
            .unwrap();

        let mut verifier = Verifier::new();
        verifier
            .load_transfer_vk(&ceremony.verification_key_bytes)
            .unwrap();

        // 3. Build witnesses: 2-in-2-out, 700+300 = 600+400
        let in_values = [700u64, 300u64];
        let out_values = [600u64, 400u64];

        let in_blindings = [Fr::rand(&mut OsRng), Fr::rand(&mut OsRng)];
        let in_serials = [Fr::rand(&mut OsRng), Fr::rand(&mut OsRng)];
        let sks = [Fr::rand(&mut OsRng), Fr::rand(&mut OsRng)];
        let out_blindings = [Fr::rand(&mut OsRng), Fr::rand(&mut OsRng)];

        let in_commitments = [
            poseidon_hash_fr(Fr::from(in_values[0]), in_blindings[0]),
            poseidon_hash_fr(Fr::from(in_values[1]), in_blindings[1]),
        ];
        let nullifiers = [
            poseidon_hash_fr(in_serials[0], sks[0]),
            poseidon_hash_fr(in_serials[1], sks[1]),
        ];
        let out_commitments = [
            poseidon_hash_fr(Fr::from(out_values[0]), out_blindings[0]),
            poseidon_hash_fr(Fr::from(out_values[1]), out_blindings[1]),
        ];

        // Insert input commitments into tree
        let mut tree = MerkleTree::new();
        tree.insert(fr_to_bytes(&in_commitments[0]));
        tree.insert(fr_to_bytes(&in_commitments[1]));

        let merkle_root_fr = Fr::from_le_bytes_mod_order(&tree.root());

        let proof0 = tree.proof(0).unwrap();
        let proof1 = tree.proof(1).unwrap();

        let merkle_paths = [
            proof0
                .siblings
                .iter()
                .map(|s| Fr::from_le_bytes_mod_order(s))
                .collect::<Vec<_>>(),
            proof1
                .siblings
                .iter()
                .map(|s| Fr::from_le_bytes_mod_order(s))
                .collect::<Vec<_>>(),
        ];

        let circuit = TransferCircuit::new(
            merkle_root_fr,
            nullifiers,
            out_commitments,
            in_values,
            in_blindings,
            in_serials,
            sks,
            merkle_paths,
            [proof0.path_bits, proof1.path_bits],
            out_values,
            out_blindings,
        );

        // 4. Prove
        let mut zk_proof = prover.prove_transfer(circuit).unwrap();

        // 5. Set public inputs: [merkle_root, null0, null1, out_comm0, out_comm1]
        zk_proof.public_inputs = vec![
            fr_to_bytes(&merkle_root_fr),
            fr_to_bytes(&nullifiers[0]),
            fr_to_bytes(&nullifiers[1]),
            fr_to_bytes(&out_commitments[0]),
            fr_to_bytes(&out_commitments[1]),
        ];

        // 6. Verify
        let valid = verifier.verify(&zk_proof).unwrap();
        assert!(valid, "valid transfer proof should verify");
    }

    #[test]
    fn test_transfer_e2e_wrong_output_commitment_fails() {
        let ceremony = setup::setup_transfer().unwrap();

        let mut prover = Prover::new();
        prover
            .load_transfer_key(&ceremony.proving_key_bytes)
            .unwrap();

        let mut verifier = Verifier::new();
        verifier
            .load_transfer_vk(&ceremony.verification_key_bytes)
            .unwrap();

        let in_values = [500u64, 500u64];
        let out_values = [500u64, 500u64];

        let in_blindings = [Fr::rand(&mut OsRng), Fr::rand(&mut OsRng)];
        let in_serials = [Fr::rand(&mut OsRng), Fr::rand(&mut OsRng)];
        let sks = [Fr::rand(&mut OsRng), Fr::rand(&mut OsRng)];
        let out_blindings = [Fr::rand(&mut OsRng), Fr::rand(&mut OsRng)];

        let in_commitments = [
            poseidon_hash_fr(Fr::from(in_values[0]), in_blindings[0]),
            poseidon_hash_fr(Fr::from(in_values[1]), in_blindings[1]),
        ];
        let nullifiers = [
            poseidon_hash_fr(in_serials[0], sks[0]),
            poseidon_hash_fr(in_serials[1], sks[1]),
        ];
        let out_commitments = [
            poseidon_hash_fr(Fr::from(out_values[0]), out_blindings[0]),
            poseidon_hash_fr(Fr::from(out_values[1]), out_blindings[1]),
        ];

        let mut tree = MerkleTree::new();
        tree.insert(fr_to_bytes(&in_commitments[0]));
        tree.insert(fr_to_bytes(&in_commitments[1]));
        let merkle_root_fr = Fr::from_le_bytes_mod_order(&tree.root());

        let proof0 = tree.proof(0).unwrap();
        let proof1 = tree.proof(1).unwrap();
        let merkle_paths = [
            proof0
                .siblings
                .iter()
                .map(|s| Fr::from_le_bytes_mod_order(s))
                .collect::<Vec<_>>(),
            proof1
                .siblings
                .iter()
                .map(|s| Fr::from_le_bytes_mod_order(s))
                .collect::<Vec<_>>(),
        ];

        let circuit = TransferCircuit::new(
            merkle_root_fr,
            nullifiers,
            out_commitments,
            in_values,
            in_blindings,
            in_serials,
            sks,
            merkle_paths,
            [proof0.path_bits, proof1.path_bits],
            out_values,
            out_blindings,
        );

        let mut zk_proof = prover.prove_transfer(circuit).unwrap();

        // Tamper: wrong output commitment in public inputs
        zk_proof.public_inputs = vec![
            fr_to_bytes(&merkle_root_fr),
            fr_to_bytes(&nullifiers[0]),
            fr_to_bytes(&nullifiers[1]),
            fr_to_bytes(&Fr::from(12345u64)), // WRONG
            fr_to_bytes(&out_commitments[1]),
        ];

        let valid = verifier.verify(&zk_proof).unwrap();
        assert!(!valid, "tampered output commitment should fail");
    }

    // ── Setup key serialization roundtrip ──────────────────────────

    #[test]
    fn test_setup_key_serialization_roundtrip() {
        let ceremony = setup::setup_shield().unwrap();

        // Verify keys can be re-loaded from bytes
        let pk = setup::load_proving_key(&ceremony.proving_key_bytes);
        assert!(pk.is_ok(), "proving key roundtrip failed");

        let vk = setup::load_verification_key(&ceremony.verification_key_bytes);
        assert!(vk.is_ok(), "verification key roundtrip failed");
    }
}
