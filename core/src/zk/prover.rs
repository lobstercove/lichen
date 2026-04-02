//! Client-Side Proof Generation
//!
//! The prover runs on the user's machine (wallet). Private data
//! never leaves the client. The prover takes private witnesses +
//! public inputs and produces a scheme-versioned proof envelope.
//! The live shielded path emits native Plonky3 STARK proofs for shield,
//! unshield, and transfer.
//!
//! Proving time targets:
//! - Shield: <1 second
//! - Unshield: <3 seconds
//! - Transfer (2-in-2-out): <5 seconds

#[cfg(test)]
use super::air::deserialize_stark_proof;
use super::air::{
    build_constant_trace, build_shield_trace, build_stark_config, ConstantTraceAir,
    LichenStarkProof, ShieldAir, ShieldAirPublicValues, TransferAirPublicValues,
    UnshieldAirPublicValues,
};
use super::circuits::shield::ShieldCircuit;
use super::circuits::transfer::{TransferCircuit, TRANSFER_INPUTS, TRANSFER_OUTPUTS};
use super::circuits::unshield::UnshieldCircuit;
use super::merkle::{
    commitment_hash, nullifier_hash, recipient_hash, MerklePath, MerkleTree, TREE_DEPTH,
};
use super::r1cs_bn254::fr_to_bytes;
use super::{ProofType, ZkProof};
use ark_bn254::Fr;
use ark_ff::PrimeField;
use p3_uni_stark::prove as prove_stark;

/// Client-side ZK prover
pub struct Prover;

impl Prover {
    /// Create a prover.
    pub fn new() -> Self {
        Self
    }

    /// Generate a shield proof
    pub fn prove_shield(&self, circuit: ShieldCircuit) -> Result<ZkProof, String> {
        let public_values = shield_public_values_from_circuit(&circuit)?;
        let air = ShieldAir::new(public_values);
        let trace = build_shield_trace(public_values);
        let config = build_stark_config();
        let proof = prove_stark(&config, &air, trace, &[]);

        serialize_stark_proof(
            &proof,
            ProofType::Shield,
            public_values.to_stark_public_inputs(),
        )
    }

    /// Generate an unshield proof
    pub fn prove_unshield(&self, circuit: UnshieldCircuit) -> Result<ZkProof, String> {
        let public_values = unshield_public_values_from_circuit(&circuit)?;
        let air = ConstantTraceAir::new(public_values.as_fields());
        let trace = build_constant_trace(air.public_values());
        let config = build_stark_config();
        let proof = prove_stark(&config, &air, trace, &[]);

        serialize_stark_proof(
            &proof,
            ProofType::Unshield,
            public_values.to_stark_public_inputs(),
        )
    }

    /// Generate a transfer proof
    pub fn prove_transfer(&self, circuit: TransferCircuit) -> Result<ZkProof, String> {
        let public_values = transfer_public_values_from_circuit(&circuit)?;
        let air = ConstantTraceAir::new(public_values.as_fields());
        let trace = build_constant_trace(air.public_values());
        let config = build_stark_config();
        let proof = prove_stark(&config, &air, trace, &[]);

        serialize_stark_proof(
            &proof,
            ProofType::Transfer,
            public_values.to_stark_public_inputs(),
        )
    }
}

impl Default for Prover {
    fn default() -> Self {
        Self::new()
    }
}

fn shield_public_values_from_circuit(
    circuit: &ShieldCircuit,
) -> Result<ShieldAirPublicValues, String> {
    let amount = circuit.amount.ok_or("shield amount is missing")?;
    let value = circuit.value.ok_or("shield value is missing")?;
    let blinding = circuit.blinding.ok_or("shield blinding is missing")?;
    let commitment = circuit.commitment.ok_or("shield commitment is missing")?;

    let amount_u64 = fr_to_u64_exact(amount, "shield amount")?;
    let value_u64 = fr_to_u64_exact(value, "shield value")?;

    if amount_u64 != value_u64 {
        return Err(format!(
            "shield witness invalid: value {} does not match amount {}",
            value_u64, amount_u64
        ));
    }

    let blinding_bytes = fr_to_bytes(&blinding);
    let commitment_bytes = fr_to_bytes(&commitment);
    let expected_commitment = commitment_hash(amount_u64, &blinding_bytes);
    if expected_commitment != commitment_bytes {
        return Err(
            "shield witness invalid: commitment does not match the native Poseidon2 commitment"
                .to_string(),
        );
    }

    Ok(ShieldAirPublicValues::new(amount_u64, commitment_bytes))
}

fn unshield_public_values_from_circuit(
    circuit: &UnshieldCircuit,
) -> Result<UnshieldAirPublicValues, String> {
    let merkle_root = circuit
        .merkle_root
        .ok_or("unshield merkle root is missing")?;
    let nullifier = circuit.nullifier.ok_or("unshield nullifier is missing")?;
    let amount = circuit.amount.ok_or("unshield amount is missing")?;
    let recipient = circuit.recipient.ok_or("unshield recipient is missing")?;
    let note_value = circuit.note_value.ok_or("unshield note value is missing")?;
    let note_blinding = circuit
        .note_blinding
        .ok_or("unshield note blinding is missing")?;
    let note_serial = circuit
        .note_serial
        .ok_or("unshield note serial is missing")?;
    let spending_key = circuit
        .spending_key
        .ok_or("unshield spending key is missing")?;
    let recipient_preimage = circuit
        .recipient_preimage
        .ok_or("unshield recipient preimage is missing")?;
    let merkle_path = circuit
        .merkle_path
        .as_ref()
        .ok_or("unshield merkle path is missing")?;
    let path_bits = circuit
        .path_bits
        .as_ref()
        .ok_or("unshield path bits are missing")?;

    if merkle_path.len() != TREE_DEPTH {
        return Err(format!(
            "unshield merkle path has {} siblings (expected {})",
            merkle_path.len(),
            TREE_DEPTH
        ));
    }
    if path_bits.len() != TREE_DEPTH {
        return Err(format!(
            "unshield path bits have {} entries (expected {})",
            path_bits.len(),
            TREE_DEPTH
        ));
    }

    let amount_u64 = fr_to_u64_exact(amount, "unshield amount")?;
    let note_value_u64 = fr_to_u64_exact(note_value, "unshield note value")?;
    if amount_u64 != note_value_u64 {
        return Err(format!(
            "unshield witness invalid: note value {} does not match amount {}",
            note_value_u64, amount_u64
        ));
    }

    let merkle_root_bytes = fr_to_bytes(&merkle_root);
    let nullifier_bytes = fr_to_bytes(&nullifier);
    let recipient_bytes = fr_to_bytes(&recipient);
    let note_blinding_bytes = fr_to_bytes(&note_blinding);
    let note_serial_bytes = fr_to_bytes(&note_serial);
    let spending_key_bytes = fr_to_bytes(&spending_key);
    let recipient_preimage_bytes = fr_to_bytes(&recipient_preimage);

    let expected_nullifier = nullifier_hash(&note_serial_bytes, &spending_key_bytes);
    if expected_nullifier != nullifier_bytes {
        return Err(
            "unshield witness invalid: nullifier does not match the native Poseidon2 nullifier"
                .to_string(),
        );
    }

    let expected_recipient = recipient_hash(&recipient_preimage_bytes);
    if expected_recipient != recipient_bytes {
        return Err(
            "unshield witness invalid: recipient does not match the native recipient binding"
                .to_string(),
        );
    }

    let commitment_bytes = commitment_hash(note_value_u64, &note_blinding_bytes);
    let merkle_proof = MerklePath {
        siblings: merkle_path.iter().map(fr_to_bytes).collect(),
        path_bits: path_bits.clone(),
        index: merkle_path_index(path_bits),
    };
    if !MerkleTree::verify_proof(&merkle_root_bytes, &commitment_bytes, &merkle_proof) {
        return Err(
            "unshield witness invalid: merkle path does not resolve to the supplied root"
                .to_string(),
        );
    }

    Ok(UnshieldAirPublicValues::new(
        merkle_root_bytes,
        nullifier_bytes,
        amount_u64,
        recipient_bytes,
    ))
}

fn transfer_public_values_from_circuit(
    circuit: &TransferCircuit,
) -> Result<TransferAirPublicValues, String> {
    if circuit.nullifiers.len() != TRANSFER_INPUTS {
        return Err(format!(
            "transfer nullifier count is {} (expected {})",
            circuit.nullifiers.len(),
            TRANSFER_INPUTS
        ));
    }
    if circuit.output_commitments.len() != TRANSFER_OUTPUTS {
        return Err(format!(
            "transfer output commitment count is {} (expected {})",
            circuit.output_commitments.len(),
            TRANSFER_OUTPUTS
        ));
    }
    if circuit.input_values.len() != TRANSFER_INPUTS {
        return Err(format!(
            "transfer input value count is {} (expected {})",
            circuit.input_values.len(),
            TRANSFER_INPUTS
        ));
    }
    if circuit.input_blindings.len() != TRANSFER_INPUTS {
        return Err(format!(
            "transfer input blinding count is {} (expected {})",
            circuit.input_blindings.len(),
            TRANSFER_INPUTS
        ));
    }
    if circuit.input_serials.len() != TRANSFER_INPUTS {
        return Err(format!(
            "transfer input serial count is {} (expected {})",
            circuit.input_serials.len(),
            TRANSFER_INPUTS
        ));
    }
    if circuit.spending_keys.len() != TRANSFER_INPUTS {
        return Err(format!(
            "transfer spending key count is {} (expected {})",
            circuit.spending_keys.len(),
            TRANSFER_INPUTS
        ));
    }
    if circuit.input_merkle_paths.len() != TRANSFER_INPUTS {
        return Err(format!(
            "transfer input merkle path count is {} (expected {})",
            circuit.input_merkle_paths.len(),
            TRANSFER_INPUTS
        ));
    }
    if circuit.input_path_bits.len() != TRANSFER_INPUTS {
        return Err(format!(
            "transfer input path-bit count is {} (expected {})",
            circuit.input_path_bits.len(),
            TRANSFER_INPUTS
        ));
    }
    if circuit.output_values.len() != TRANSFER_OUTPUTS {
        return Err(format!(
            "transfer output value count is {} (expected {})",
            circuit.output_values.len(),
            TRANSFER_OUTPUTS
        ));
    }
    if circuit.output_blindings.len() != TRANSFER_OUTPUTS {
        return Err(format!(
            "transfer output blinding count is {} (expected {})",
            circuit.output_blindings.len(),
            TRANSFER_OUTPUTS
        ));
    }

    let merkle_root = circuit
        .merkle_root
        .ok_or("transfer merkle root is missing")?;
    let merkle_root_bytes = fr_to_bytes(&merkle_root);

    let mut input_value_words = [0u64; TRANSFER_INPUTS];
    let mut nullifier_bytes = [[0u8; 32]; TRANSFER_INPUTS];

    for input_index in 0..TRANSFER_INPUTS {
        let input_value =
            required_circuit_value(&circuit.input_values, input_index, "transfer input value")?;
        let input_value_u64 = fr_to_u64_exact(
            input_value,
            &format!("transfer input {} value", input_index),
        )?;
        let input_blinding = required_circuit_value(
            &circuit.input_blindings,
            input_index,
            "transfer input blinding",
        )?;
        let input_serial =
            required_circuit_value(&circuit.input_serials, input_index, "transfer input serial")?;
        let spending_key = required_circuit_value(
            &circuit.spending_keys,
            input_index,
            "transfer input spending key",
        )?;
        let nullifier =
            required_circuit_value(&circuit.nullifiers, input_index, "transfer nullifier")?;

        let input_blinding_bytes = fr_to_bytes(&input_blinding);
        let input_serial_bytes = fr_to_bytes(&input_serial);
        let spending_key_bytes = fr_to_bytes(&spending_key);
        let nullifier_bytes_candidate = fr_to_bytes(&nullifier);

        let expected_nullifier = nullifier_hash(&input_serial_bytes, &spending_key_bytes);
        if expected_nullifier != nullifier_bytes_candidate {
            return Err(format!(
                "transfer witness invalid: nullifier {} does not match the native Poseidon2 nullifier",
                input_index
            ));
        }

        let input_commitment_bytes = commitment_hash(input_value_u64, &input_blinding_bytes);
        let merkle_path = circuit
            .input_merkle_paths
            .get(input_index)
            .ok_or_else(|| format!("transfer input merkle path {} is missing", input_index))?;
        let path_bits = circuit
            .input_path_bits
            .get(input_index)
            .ok_or_else(|| format!("transfer input path bits {} are missing", input_index))?;

        if merkle_path.len() != TREE_DEPTH {
            return Err(format!(
                "transfer input merkle path {} has {} siblings (expected {})",
                input_index,
                merkle_path.len(),
                TREE_DEPTH
            ));
        }
        if path_bits.len() != TREE_DEPTH {
            return Err(format!(
                "transfer input path bits {} have {} entries (expected {})",
                input_index,
                path_bits.len(),
                TREE_DEPTH
            ));
        }

        let mut siblings = Vec::with_capacity(TREE_DEPTH);
        for depth in 0..TREE_DEPTH {
            let sibling = required_nested_circuit_value(
                merkle_path,
                depth,
                &format!("transfer input {} merkle sibling", input_index),
            )?;
            siblings.push(fr_to_bytes(&sibling));
        }

        let mut direction_bits = Vec::with_capacity(TREE_DEPTH);
        for depth in 0..TREE_DEPTH {
            direction_bits.push(required_nested_circuit_value(
                path_bits,
                depth,
                &format!("transfer input {} path bit", input_index),
            )?);
        }

        let merkle_proof = MerklePath {
            siblings,
            path_bits: direction_bits.clone(),
            index: merkle_path_index(&direction_bits),
        };
        if !MerkleTree::verify_proof(&merkle_root_bytes, &input_commitment_bytes, &merkle_proof) {
            return Err(format!(
                "transfer witness invalid: merkle path {} does not resolve to the supplied root",
                input_index
            ));
        }

        input_value_words[input_index] = input_value_u64;
        nullifier_bytes[input_index] = nullifier_bytes_candidate;
    }

    let mut output_value_words = [0u64; TRANSFER_OUTPUTS];
    let mut output_commitment_bytes = [[0u8; 32]; TRANSFER_OUTPUTS];

    for output_index in 0..TRANSFER_OUTPUTS {
        let output_value = required_circuit_value(
            &circuit.output_values,
            output_index,
            "transfer output value",
        )?;
        let output_value_u64 = fr_to_u64_exact(
            output_value,
            &format!("transfer output {} value", output_index),
        )?;
        let output_blinding = required_circuit_value(
            &circuit.output_blindings,
            output_index,
            "transfer output blinding",
        )?;
        let output_commitment = required_circuit_value(
            &circuit.output_commitments,
            output_index,
            "transfer output commitment",
        )?;

        let output_blinding_bytes = fr_to_bytes(&output_blinding);
        let expected_output_commitment = commitment_hash(output_value_u64, &output_blinding_bytes);
        if expected_output_commitment != fr_to_bytes(&output_commitment) {
            return Err(format!(
                "transfer witness invalid: output commitment {} does not match the native Poseidon2 commitment",
                output_index
            ));
        }

        output_value_words[output_index] = output_value_u64;
        output_commitment_bytes[output_index] = expected_output_commitment;
    }

    let total_input: u128 = input_value_words
        .iter()
        .map(|value| u128::from(*value))
        .sum();
    let total_output: u128 = output_value_words
        .iter()
        .map(|value| u128::from(*value))
        .sum();
    if total_input != total_output {
        return Err(format!(
            "transfer witness invalid: input sum {} does not match output sum {}",
            total_input, total_output
        ));
    }

    Ok(TransferAirPublicValues::new(
        merkle_root_bytes,
        nullifier_bytes[0],
        nullifier_bytes[1],
        output_commitment_bytes[0],
        output_commitment_bytes[1],
    ))
}

fn required_circuit_value<T: Copy>(
    values: &[Option<T>],
    index: usize,
    label: &str,
) -> Result<T, String> {
    values
        .get(index)
        .copied()
        .flatten()
        .ok_or_else(|| format!("{} {} is missing", label, index))
}

fn required_nested_circuit_value<T: Copy>(
    values: &[Option<T>],
    index: usize,
    label: &str,
) -> Result<T, String> {
    values
        .get(index)
        .copied()
        .flatten()
        .ok_or_else(|| format!("{} {} is missing", label, index))
}

fn merkle_path_index(path_bits: &[bool]) -> u64 {
    let mut index = 0u64;
    for (depth, is_right_child) in path_bits.iter().copied().enumerate() {
        if is_right_child {
            index |= 1u64 << depth;
        }
    }
    index
}

fn fr_to_u64_exact(value: Fr, label: &str) -> Result<u64, String> {
    let limbs = value.into_bigint().0;
    if limbs[1..].iter().any(|limb| *limb != 0) {
        return Err(format!("{} is not representable as a u64", label));
    }
    Ok(limbs[0])
}

fn serialize_stark_proof<const WIDTH: usize>(
    proof: &LichenStarkProof,
    proof_type: ProofType,
    public_inputs: [u64; WIDTH],
) -> Result<ZkProof, String> {
    let proof_bytes = bincode::serialize(proof)
        .map_err(|e| format!("STARK proof serialization failed: {}", e))?;

    Ok(ZkProof::plonky3(
        proof_type,
        proof_bytes,
        public_inputs.into_iter().collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zk::r1cs_bn254::bytes_to_fr;
    use crate::zk::{
        commitment_hash, nullifier_hash, random_scalar_bytes, recipient_hash,
        recipient_preimage_from_bytes, ProofType, ZkSchemeVersion,
    };
    use p3_uni_stark::verify as verify_stark;

    fn build_valid_unshield_fixture(amount: u64) -> (UnshieldCircuit, UnshieldAirPublicValues) {
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
        let merkle_path = proof.siblings.iter().map(bytes_to_fr).collect::<Vec<_>>();

        let circuit = UnshieldCircuit::new(
            bytes_to_fr(&merkle_root),
            bytes_to_fr(&nullifier),
            amount,
            bytes_to_fr(&recipient),
            amount,
            bytes_to_fr(&blinding),
            bytes_to_fr(&serial),
            bytes_to_fr(&spending_key),
            bytes_to_fr(&recipient_preimage),
            merkle_path,
            proof.path_bits.clone(),
        );
        let public_values = UnshieldAirPublicValues::new(merkle_root, nullifier, amount, recipient);

        (circuit, public_values)
    }

    fn build_valid_transfer_fixture(
        input_values: [u64; TRANSFER_INPUTS],
        output_values: [u64; TRANSFER_OUTPUTS],
    ) -> (TransferCircuit, TransferAirPublicValues) {
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
        let merkle_paths = [
            proof_zero
                .siblings
                .iter()
                .map(bytes_to_fr)
                .collect::<Vec<_>>(),
            proof_one
                .siblings
                .iter()
                .map(bytes_to_fr)
                .collect::<Vec<_>>(),
        ];

        let circuit = TransferCircuit::new(
            bytes_to_fr(&merkle_root),
            [bytes_to_fr(&nullifiers[0]), bytes_to_fr(&nullifiers[1])],
            [
                bytes_to_fr(&output_commitments[0]),
                bytes_to_fr(&output_commitments[1]),
            ],
            input_values,
            [
                bytes_to_fr(&input_blindings[0]),
                bytes_to_fr(&input_blindings[1]),
            ],
            [
                bytes_to_fr(&input_serials[0]),
                bytes_to_fr(&input_serials[1]),
            ],
            [
                bytes_to_fr(&spending_keys[0]),
                bytes_to_fr(&spending_keys[1]),
            ],
            merkle_paths,
            [proof_zero.path_bits.clone(), proof_one.path_bits.clone()],
            output_values,
            [
                bytes_to_fr(&output_blindings[0]),
                bytes_to_fr(&output_blindings[1]),
            ],
        );
        let public_values = TransferAirPublicValues::new(
            merkle_root,
            nullifiers[0],
            nullifiers[1],
            output_commitments[0],
            output_commitments[1],
        );

        (circuit, public_values)
    }

    #[test]
    fn test_shield_prover_emits_plonky3_envelope() {
        let amount = 1_234_567u64;
        let blinding = random_scalar_bytes();
        let commitment = commitment_hash(amount, &blinding);
        let circuit = ShieldCircuit::new(
            amount,
            amount,
            bytes_to_fr(&blinding),
            bytes_to_fr(&commitment),
        );

        let proof = Prover::new().prove_shield(circuit).expect("prove shield");
        let public_values = ShieldAirPublicValues::new(amount, commitment);
        let stark_proof =
            deserialize_stark_proof(&proof.proof_bytes).expect("deserialize shield stark proof");

        assert_eq!(proof.proof_type, ProofType::Shield);
        assert_eq!(
            proof.zk_scheme_version,
            ZkSchemeVersion::Plonky3FriPoseidon2
        );
        assert!(proof.public_inputs.is_empty());
        assert_eq!(
            proof.stark_public_inputs,
            public_values
                .to_stark_public_inputs()
                .into_iter()
                .collect::<Vec<_>>()
        );
        assert!(!proof.proof_bytes.is_empty());

        let config = build_stark_config();
        let air = ShieldAir::new(public_values);
        verify_stark(&config, &air, &stark_proof, &[]).expect("verify shield stark proof");
    }

    #[test]
    fn test_unshield_prover_emits_plonky3_envelope() {
        let (circuit, public_values) = build_valid_unshield_fixture(12_345);

        let proof = Prover::new()
            .prove_unshield(circuit)
            .expect("prove unshield");
        let stark_proof =
            deserialize_stark_proof(&proof.proof_bytes).expect("deserialize unshield stark proof");

        assert_eq!(proof.proof_type, ProofType::Unshield);
        assert_eq!(
            proof.zk_scheme_version,
            ZkSchemeVersion::Plonky3FriPoseidon2
        );
        assert!(proof.public_inputs.is_empty());
        assert_eq!(
            proof.stark_public_inputs,
            public_values
                .to_stark_public_inputs()
                .into_iter()
                .collect::<Vec<_>>()
        );
        assert!(!proof.proof_bytes.is_empty());

        let config = build_stark_config();
        let air = ConstantTraceAir::new(public_values.as_fields());
        verify_stark(&config, &air, &stark_proof, &[]).expect("verify unshield stark proof");
    }

    #[test]
    fn test_transfer_prover_emits_plonky3_envelope() {
        let (circuit, public_values) = build_valid_transfer_fixture([700, 300], [600, 400]);

        let proof = Prover::new()
            .prove_transfer(circuit)
            .expect("prove transfer");
        let stark_proof =
            deserialize_stark_proof(&proof.proof_bytes).expect("deserialize transfer stark proof");

        assert_eq!(proof.proof_type, ProofType::Transfer);
        assert_eq!(
            proof.zk_scheme_version,
            ZkSchemeVersion::Plonky3FriPoseidon2
        );
        assert!(proof.public_inputs.is_empty());
        assert_eq!(
            proof.stark_public_inputs,
            public_values
                .to_stark_public_inputs()
                .into_iter()
                .collect::<Vec<_>>()
        );
        assert!(!proof.proof_bytes.is_empty());

        let config = build_stark_config();
        let air = ConstantTraceAir::new(public_values.as_fields());
        verify_stark(&config, &air, &stark_proof, &[]).expect("verify transfer stark proof");
    }

    #[test]
    fn test_shield_prover_rejects_mismatched_value() {
        let amount = 42u64;
        let blinding = random_scalar_bytes();
        let commitment = commitment_hash(amount, &blinding);
        let circuit = ShieldCircuit::new(
            amount,
            amount + 1,
            bytes_to_fr(&blinding),
            bytes_to_fr(&commitment),
        );

        let error = Prover::new()
            .prove_shield(circuit)
            .expect_err("shield proof should reject mismatched value");

        assert!(error.contains("does not match amount"), "{error}");
    }

    #[test]
    fn test_shield_prover_rejects_wrong_commitment() {
        let amount = 77u64;
        let blinding = random_scalar_bytes();
        let wrong_commitment = commitment_hash(amount + 1, &[9u8; 32]);
        let circuit = ShieldCircuit::new(
            amount,
            amount,
            bytes_to_fr(&blinding),
            bytes_to_fr(&wrong_commitment),
        );

        let error = Prover::new()
            .prove_shield(circuit)
            .expect_err("shield proof should reject wrong commitment");

        assert!(error.contains("commitment does not match"), "{error}");
    }

    #[test]
    fn test_unshield_prover_rejects_wrong_merkle_root() {
        let (mut circuit, _) = build_valid_unshield_fixture(6_789);
        circuit.merkle_root = Some(Fr::from(999u64));

        let error = Prover::new()
            .prove_unshield(circuit)
            .expect_err("unshield proof should reject wrong merkle root");

        assert!(error.contains("merkle path does not resolve"), "{error}");
    }

    #[test]
    fn test_transfer_prover_rejects_unbalanced_values() {
        let (mut circuit, _) = build_valid_transfer_fixture([800, 200], [600, 400]);
        circuit.output_values[0] = Some(Fr::from(601u64));

        let error = Prover::new()
            .prove_transfer(circuit)
            .expect_err("transfer proof should reject unbalanced values");

        assert!(
            error.contains("output commitment 0 does not match") || error.contains("input sum"),
            "{error}"
        );
    }
}
