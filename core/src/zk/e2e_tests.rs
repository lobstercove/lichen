//! End-to-end prove/verify roundtrip tests.
//!
//! These tests run the native STARK pipeline:
//! 1. Build a circuit with valid witnesses
//! 2. Generate a Plonky3 proof envelope
//! 3. Verify the proof with the validator-side verifier
//! 4. Reject tampered public inputs

#[cfg(test)]
mod tests {
    use crate::zk::air::BYTES32_GOLDILOCKS_WORDS;
    use crate::zk::circuits::shield::ShieldCircuit;
    use crate::zk::circuits::transfer::TransferCircuit;
    use crate::zk::circuits::unshield::UnshieldCircuit;
    use crate::zk::{
        commitment_hash, deserialize_stark_proof, nullifier_hash, random_scalar_bytes,
        recipient_hash, recipient_preimage_from_bytes, MerkleTree, ProofType, Prover, Verifier,
        ZkSchemeVersion,
    };

    fn build_unshield_circuit(amount: u64) -> UnshieldCircuit {
        let blinding = random_scalar_bytes();
        let serial = random_scalar_bytes();
        let spending_key = random_scalar_bytes();
        let recipient_preimage = recipient_preimage_from_bytes([42u8; 32]);
        let recipient = recipient_hash(&recipient_preimage);

        let commitment = commitment_hash(amount, &blinding);
        let nullifier = nullifier_hash(&serial, &spending_key);

        let mut tree = MerkleTree::new();
        tree.insert(commitment);
        let merkle_root = tree.root();
        let proof = tree.proof(0).expect("proof for the only leaf");
        UnshieldCircuit::new_bytes(
            merkle_root,
            nullifier,
            amount,
            recipient,
            amount,
            blinding,
            serial,
            spending_key,
            recipient_preimage,
            proof.siblings,
            proof.path_bits,
        )
    }

    fn build_transfer_circuit(input_values: [u64; 2], output_values: [u64; 2]) -> TransferCircuit {
        let input_blindings = [random_scalar_bytes(), random_scalar_bytes()];
        let input_serials = [random_scalar_bytes(), random_scalar_bytes()];
        let spending_keys = [random_scalar_bytes(), random_scalar_bytes()];
        let output_blindings = [random_scalar_bytes(), random_scalar_bytes()];

        let input_commitments = [
            commitment_hash(input_values[0], &input_blindings[0]),
            commitment_hash(input_values[1], &input_blindings[1]),
        ];
        let nullifiers = [
            nullifier_hash(&input_serials[0], &spending_keys[0]),
            nullifier_hash(&input_serials[1], &spending_keys[1]),
        ];
        let output_commitments = [
            commitment_hash(output_values[0], &output_blindings[0]),
            commitment_hash(output_values[1], &output_blindings[1]),
        ];

        let mut tree = MerkleTree::new();
        tree.insert(input_commitments[0]);
        tree.insert(input_commitments[1]);
        let merkle_root = tree.root();

        let proof_zero = tree.proof(0).expect("proof for the first leaf");
        let proof_one = tree.proof(1).expect("proof for the second leaf");
        let merkle_paths = [proof_zero.siblings, proof_one.siblings];

        TransferCircuit::new_bytes(
            merkle_root,
            nullifiers,
            output_commitments,
            input_values,
            input_blindings,
            input_serials,
            spending_keys,
            merkle_paths,
            [proof_zero.path_bits, proof_one.path_bits],
            output_values,
            output_blindings,
        )
    }

    #[test]
    fn test_shield_e2e_prove_and_verify() {
        let amount = 1_000_000_000u64;
        let blinding = random_scalar_bytes();
        let commitment = commitment_hash(amount, &blinding);

        let circuit = ShieldCircuit::new_bytes(amount, amount, blinding, commitment);
        let zk_proof = Prover::new().prove_shield(circuit).expect("prove failed");

        assert_eq!(zk_proof.proof_type, ProofType::Shield);
        assert_eq!(
            zk_proof.zk_scheme_version,
            ZkSchemeVersion::Plonky3FriPoseidon2
        );
        assert!(zk_proof.public_inputs.is_empty());
        assert!(!zk_proof.proof_bytes.is_empty());

        let valid = Verifier::new()
            .verify(&zk_proof)
            .expect("verification call failed");
        assert!(valid, "valid shield proof should verify");
    }

    #[test]
    fn test_shield_e2e_wrong_public_input_fails() {
        let amount = 500u64;
        let blinding = random_scalar_bytes();
        let commitment = commitment_hash(amount, &blinding);

        let circuit = ShieldCircuit::new_bytes(amount, amount, blinding, commitment);
        let mut zk_proof = Prover::new().prove_shield(circuit).expect("prove failed");

        zk_proof.stark_public_inputs[0] = zk_proof.stark_public_inputs[0].saturating_add(1);

        let valid = Verifier::new()
            .verify(&zk_proof)
            .expect("verification call failed");
        assert!(!valid, "tampered public input should fail verification");
    }

    #[test]
    fn test_unshield_e2e_prove_and_verify() {
        let zk_proof = Prover::new()
            .prove_unshield(build_unshield_circuit(2_000_000_000u64))
            .expect("prove failed");

        assert_eq!(zk_proof.proof_type, ProofType::Unshield);
        assert_eq!(
            zk_proof.zk_scheme_version,
            ZkSchemeVersion::Plonky3FriPoseidon2
        );
        assert!(zk_proof.public_inputs.is_empty());
        assert!(!zk_proof.proof_bytes.is_empty());

        let valid = Verifier::new().verify(&zk_proof).expect("verify unshield");
        assert!(valid, "valid unshield proof should verify");
    }

    #[test]
    fn test_unshield_e2e_wrong_nullifier_fails() {
        let mut zk_proof = Prover::new()
            .prove_unshield(build_unshield_circuit(1_000u64))
            .expect("prove failed");

        zk_proof.stark_public_inputs[BYTES32_GOLDILOCKS_WORDS] =
            zk_proof.stark_public_inputs[BYTES32_GOLDILOCKS_WORDS].saturating_add(1);

        let valid = Verifier::new().verify(&zk_proof).expect("verify unshield");
        assert!(!valid, "wrong nullifier should fail verification");
    }

    #[test]
    fn test_transfer_e2e_prove_and_verify() {
        let zk_proof = Prover::new()
            .prove_transfer(build_transfer_circuit([700, 300], [600, 400]))
            .expect("prove failed");

        assert_eq!(zk_proof.proof_type, ProofType::Transfer);
        assert_eq!(
            zk_proof.zk_scheme_version,
            ZkSchemeVersion::Plonky3FriPoseidon2
        );
        assert!(zk_proof.public_inputs.is_empty());
        assert!(!zk_proof.proof_bytes.is_empty());

        let valid = Verifier::new().verify(&zk_proof).expect("verify transfer");
        assert!(valid, "valid transfer proof should verify");
    }

    #[test]
    fn test_transfer_e2e_wrong_output_commitment_fails() {
        let mut zk_proof = Prover::new()
            .prove_transfer(build_transfer_circuit([500, 500], [500, 500]))
            .expect("prove failed");

        let commitment_c_index = BYTES32_GOLDILOCKS_WORDS * 3;
        zk_proof.stark_public_inputs[commitment_c_index] =
            zk_proof.stark_public_inputs[commitment_c_index].saturating_add(1);

        let valid = Verifier::new().verify(&zk_proof).expect("verify transfer");
        assert!(!valid, "tampered output commitment should fail");
    }

    #[test]
    fn test_stark_proof_serialization_roundtrip() {
        let amount = 777u64;
        let blinding = random_scalar_bytes();
        let commitment = commitment_hash(amount, &blinding);
        let circuit = ShieldCircuit::new_bytes(amount, amount, blinding, commitment);
        let zk_proof = Prover::new().prove_shield(circuit).expect("prove failed");

        let decoded = deserialize_stark_proof(&zk_proof.proof_bytes).expect("decode stark proof");
        drop(decoded);

        let valid = Verifier::new().verify(&zk_proof).expect("verify shield");
        assert!(valid, "deserialized STARK proof should still verify");
    }
}
