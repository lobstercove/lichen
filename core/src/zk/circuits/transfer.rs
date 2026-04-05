//! Transfer Circuit (Shielded -> Shielded)
//!
//! Proves: "I own 2 input notes in the Merkle tree, and I'm transferring their
//! combined value into 2 new output notes such that value is conserved."
//!
//! Public inputs:
//!   1. `merkle_root`          — current Merkle tree root
//!   2. `nullifiers[0..2]`     — Poseidon(serial_i, sk_i), marks inputs as spent
//!   3. `output_commitments[0..2]` — Poseidon(out_value_j, out_blinding_j)
//!
//! Private witnesses (per input):
//!   - `input_values[i]`, `input_blindings[i]`, `input_serials[i]`
//!   - `spending_keys[i]`
//!   - `merkle_paths[i]` (TREE_DEPTH siblings), `path_bits[i]` (TREE_DEPTH bools)
//!
//! Private witnesses (per output):
//!   - `output_values[j]`, `output_blindings[j]`
//!
//! Constraints:
//!   Per input (i):
//!     1. nullifier_i == Poseidon(serial_i, spending_key_i)
//!     2. commitment_i == Poseidon(value_i, blinding_i)
//!     3. Merkle path from commitment_i to merkle_root is valid
//!     4. 64-bit range check on value_i
//!   Per output (j):
//!     5. output_commitment_j == Poseidon(out_value_j, out_blinding_j)
//!     6. 64-bit range check on out_value_j
//!   Global:
//!     7. sum(input_values) == sum(output_values)

use ark_bn254::Fr;
use ark_crypto_primitives::sponge::poseidon::PoseidonConfig;
use ark_r1cs_std::fields::fp::FpVar;
use ark_r1cs_std::prelude::*;
use ark_relations::r1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError};

use crate::zk::circuits::utils::poseidon_hash_var;
use crate::zk::merkle::TREE_DEPTH;
use crate::zk::r1cs_bn254::{bytes_to_fr, poseidon_config};

/// Number of inputs and outputs per transfer (fixed for circuit structure)
pub const TRANSFER_INPUTS: usize = 2;
pub const TRANSFER_OUTPUTS: usize = 2;

/// Transfer circuit: proves correct shielded-to-shielded transfer
#[derive(Clone, Debug)]
pub struct TransferCircuit {
    /// Poseidon config (must match native computation)
    pub poseidon_config: PoseidonConfig<Fr>,

    // Public inputs
    /// Current Merkle tree root
    pub merkle_root: Option<Fr>,
    /// Nullifiers for spent input notes
    pub nullifiers: Vec<Option<Fr>>,
    /// Commitments for new output notes
    pub output_commitments: Vec<Option<Fr>>,

    // Private witnesses — inputs
    /// Input note values
    pub input_values: Vec<Option<Fr>>,
    /// Input note blinding factors
    pub input_blindings: Vec<Option<Fr>>,
    /// Input note serial numbers
    pub input_serials: Vec<Option<Fr>>,
    /// Spending keys for input notes
    pub spending_keys: Vec<Option<Fr>>,
    /// Merkle path siblings per input (TRANSFER_INPUTS × TREE_DEPTH)
    pub input_merkle_paths: Vec<Vec<Option<Fr>>>,
    /// Merkle path direction bits per input (TRANSFER_INPUTS × TREE_DEPTH)
    pub input_path_bits: Vec<Vec<Option<bool>>>,

    // Private witnesses — outputs
    /// Output note values
    pub output_values: Vec<Option<Fr>>,
    /// Output note blinding factors
    pub output_blindings: Vec<Option<Fr>>,
}

impl TransferCircuit {
    /// Create a new transfer circuit with concrete witness values.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        merkle_root: Fr,
        nullifiers: [Fr; TRANSFER_INPUTS],
        output_commitments: [Fr; TRANSFER_OUTPUTS],
        input_values: [u64; TRANSFER_INPUTS],
        input_blindings: [Fr; TRANSFER_INPUTS],
        input_serials: [Fr; TRANSFER_INPUTS],
        spending_keys: [Fr; TRANSFER_INPUTS],
        input_merkle_paths: [Vec<Fr>; TRANSFER_INPUTS],
        input_path_bits: [Vec<bool>; TRANSFER_INPUTS],
        output_values: [u64; TRANSFER_OUTPUTS],
        output_blindings: [Fr; TRANSFER_OUTPUTS],
    ) -> Self {
        for path in &input_merkle_paths {
            assert_eq!(path.len(), TREE_DEPTH);
        }
        for bits in &input_path_bits {
            assert_eq!(bits.len(), TREE_DEPTH);
        }
        Self {
            poseidon_config: poseidon_config(),
            merkle_root: Some(merkle_root),
            nullifiers: nullifiers.iter().map(|n| Some(*n)).collect(),
            output_commitments: output_commitments.iter().map(|c| Some(*c)).collect(),
            input_values: input_values.iter().map(|v| Some(Fr::from(*v))).collect(),
            input_blindings: input_blindings.iter().map(|b| Some(*b)).collect(),
            input_serials: input_serials.iter().map(|s| Some(*s)).collect(),
            spending_keys: spending_keys.iter().map(|k| Some(*k)).collect(),
            input_merkle_paths: input_merkle_paths
                .iter()
                .map(|p| p.iter().map(|s| Some(*s)).collect())
                .collect(),
            input_path_bits: input_path_bits
                .iter()
                .map(|p| p.iter().map(|b| Some(*b)).collect())
                .collect(),
            output_values: output_values.iter().map(|v| Some(Fr::from(*v))).collect(),
            output_blindings: output_blindings.iter().map(|b| Some(*b)).collect(),
        }
    }

    /// Create a new transfer circuit from canonical 32-byte witness values.
    #[allow(clippy::too_many_arguments)]
    pub fn new_bytes(
        merkle_root: [u8; 32],
        nullifiers: [[u8; 32]; TRANSFER_INPUTS],
        output_commitments: [[u8; 32]; TRANSFER_OUTPUTS],
        input_values: [u64; TRANSFER_INPUTS],
        input_blindings: [[u8; 32]; TRANSFER_INPUTS],
        input_serials: [[u8; 32]; TRANSFER_INPUTS],
        spending_keys: [[u8; 32]; TRANSFER_INPUTS],
        input_merkle_paths: [Vec<[u8; 32]>; TRANSFER_INPUTS],
        input_path_bits: [Vec<bool>; TRANSFER_INPUTS],
        output_values: [u64; TRANSFER_OUTPUTS],
        output_blindings: [[u8; 32]; TRANSFER_OUTPUTS],
    ) -> Self {
        Self::new(
            bytes_to_fr(&merkle_root),
            nullifiers.map(|value| bytes_to_fr(&value)),
            output_commitments.map(|value| bytes_to_fr(&value)),
            input_values,
            input_blindings.map(|value| bytes_to_fr(&value)),
            input_serials.map(|value| bytes_to_fr(&value)),
            spending_keys.map(|value| bytes_to_fr(&value)),
            input_merkle_paths.map(|path| {
                path.into_iter()
                    .map(|sibling| bytes_to_fr(&sibling))
                    .collect()
            }),
            input_path_bits,
            output_values,
            output_blindings.map(|value| bytes_to_fr(&value)),
        )
    }

    /// Empty circuit for key generation (setup/ceremony phase).
    pub fn empty() -> Self {
        Self {
            poseidon_config: poseidon_config(),
            merkle_root: None,
            nullifiers: vec![None; TRANSFER_INPUTS],
            output_commitments: vec![None; TRANSFER_OUTPUTS],
            input_values: vec![None; TRANSFER_INPUTS],
            input_blindings: vec![None; TRANSFER_INPUTS],
            input_serials: vec![None; TRANSFER_INPUTS],
            spending_keys: vec![None; TRANSFER_INPUTS],
            input_merkle_paths: vec![vec![None; TREE_DEPTH]; TRANSFER_INPUTS],
            input_path_bits: vec![vec![None; TREE_DEPTH]; TRANSFER_INPUTS],
            output_values: vec![None; TRANSFER_OUTPUTS],
            output_blindings: vec![None; TRANSFER_OUTPUTS],
        }
    }
}

impl ConstraintSynthesizer<Fr> for TransferCircuit {
    fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> Result<(), SynthesisError> {
        let config = &self.poseidon_config;

        // ── Public inputs ──────────────────────────────────────────────
        let merkle_root_var = FpVar::new_input(cs.clone(), || {
            self.merkle_root.ok_or(SynthesisError::AssignmentMissing)
        })?;

        let mut nullifier_vars = Vec::with_capacity(TRANSFER_INPUTS);
        for i in 0..TRANSFER_INPUTS {
            let var = FpVar::new_input(cs.clone(), || {
                self.nullifiers[i].ok_or(SynthesisError::AssignmentMissing)
            })?;
            nullifier_vars.push(var);
        }

        let mut output_commitment_vars = Vec::with_capacity(TRANSFER_OUTPUTS);
        for i in 0..TRANSFER_OUTPUTS {
            let var = FpVar::new_input(cs.clone(), || {
                self.output_commitments[i].ok_or(SynthesisError::AssignmentMissing)
            })?;
            output_commitment_vars.push(var);
        }

        // ── Private witnesses — inputs ─────────────────────────────────
        let mut input_value_vars = Vec::with_capacity(TRANSFER_INPUTS);
        let mut input_blinding_vars = Vec::with_capacity(TRANSFER_INPUTS);
        let mut input_serial_vars = Vec::with_capacity(TRANSFER_INPUTS);
        let mut sk_vars = Vec::with_capacity(TRANSFER_INPUTS);
        let mut sibling_vars_all = Vec::with_capacity(TRANSFER_INPUTS);
        let mut path_bit_vars_all = Vec::with_capacity(TRANSFER_INPUTS);

        for i in 0..TRANSFER_INPUTS {
            input_value_vars.push(FpVar::new_witness(cs.clone(), || {
                self.input_values[i].ok_or(SynthesisError::AssignmentMissing)
            })?);

            input_blinding_vars.push(FpVar::new_witness(cs.clone(), || {
                self.input_blindings[i].ok_or(SynthesisError::AssignmentMissing)
            })?);

            input_serial_vars.push(FpVar::new_witness(cs.clone(), || {
                self.input_serials[i].ok_or(SynthesisError::AssignmentMissing)
            })?);

            sk_vars.push(FpVar::new_witness(cs.clone(), || {
                self.spending_keys[i].ok_or(SynthesisError::AssignmentMissing)
            })?);

            // Merkle path for this input
            let path_ref = &self.input_merkle_paths[i];
            let siblings: Vec<FpVar<Fr>> = (0..TREE_DEPTH)
                .map(|j| {
                    FpVar::new_witness(cs.clone(), || {
                        path_ref[j].ok_or(SynthesisError::AssignmentMissing)
                    })
                })
                .collect::<Result<_, _>>()?;
            sibling_vars_all.push(siblings);

            let bits_ref = &self.input_path_bits[i];
            let path_bits: Vec<Boolean<Fr>> = (0..TREE_DEPTH)
                .map(|j| {
                    Boolean::new_witness(cs.clone(), || {
                        bits_ref[j].ok_or(SynthesisError::AssignmentMissing)
                    })
                })
                .collect::<Result<_, _>>()?;
            path_bit_vars_all.push(path_bits);
        }

        // ── Private witnesses — outputs ─────────────────────────────────
        let mut output_value_vars = Vec::with_capacity(TRANSFER_OUTPUTS);
        let mut output_blinding_vars = Vec::with_capacity(TRANSFER_OUTPUTS);

        for i in 0..TRANSFER_OUTPUTS {
            output_value_vars.push(FpVar::new_witness(cs.clone(), || {
                self.output_values[i].ok_or(SynthesisError::AssignmentMissing)
            })?);

            output_blinding_vars.push(FpVar::new_witness(cs.clone(), || {
                self.output_blindings[i].ok_or(SynthesisError::AssignmentMissing)
            })?);
        }

        // ── Per-input constraints ──────────────────────────────────────
        for i in 0..TRANSFER_INPUTS {
            // 1. nullifier_i == Poseidon(serial_i, spending_key_i)
            let computed_nullifier =
                poseidon_hash_var(cs.clone(), config, &input_serial_vars[i], &sk_vars[i])?;
            computed_nullifier.enforce_equal(&nullifier_vars[i])?;

            // 2. commitment_i == Poseidon(value_i, blinding_i)
            let commitment_var = poseidon_hash_var(
                cs.clone(),
                config,
                &input_value_vars[i],
                &input_blinding_vars[i],
            )?;

            // 3. Merkle path from commitment_i to merkle_root
            let mut current = commitment_var;
            for j in 0..TREE_DEPTH {
                let left = FpVar::conditionally_select(
                    &path_bit_vars_all[i][j],
                    &sibling_vars_all[i][j],
                    &current,
                )?;
                let right = FpVar::conditionally_select(
                    &path_bit_vars_all[i][j],
                    &current,
                    &sibling_vars_all[i][j],
                )?;
                current = poseidon_hash_var(cs.clone(), config, &left, &right)?;
            }
            current.enforce_equal(&merkle_root_var)?;

            // 4. 64-bit range check on input value
            let bits = input_value_vars[i].to_bits_le()?;
            for bit in bits.iter().skip(64) {
                bit.enforce_equal(&Boolean::FALSE)?;
            }
        }

        // ── Per-output constraints ─────────────────────────────────────
        for i in 0..TRANSFER_OUTPUTS {
            // 5. output_commitment_i == Poseidon(out_value_i, out_blinding_i)
            let computed_commitment = poseidon_hash_var(
                cs.clone(),
                config,
                &output_value_vars[i],
                &output_blinding_vars[i],
            )?;
            computed_commitment.enforce_equal(&output_commitment_vars[i])?;

            // 6. 64-bit range check on output value
            let bits = output_value_vars[i].to_bits_le()?;
            for bit in bits.iter().skip(64) {
                bit.enforce_equal(&Boolean::FALSE)?;
            }
        }

        // ── Global: value conservation ─────────────────────────────────
        // 7. sum(input_values) == sum(output_values)
        let input_sum = &input_value_vars[0] + &input_value_vars[1];
        let output_sum = &output_value_vars[0] + &output_value_vars[1];
        input_sum.enforce_equal(&output_sum)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zk::r1cs_bn254::{Bn254MerkleTree, poseidon_hash_fr};
    use ark_ff::UniformRand;
    use ark_relations::r1cs::ConstraintSystem;
    use ark_std::rand::rngs::OsRng;

    /// Helper: insert two notes into a tree, return (tree, proofs, note data)
    #[allow(dead_code)]
    struct TestInput {
        value: u64,
        blinding: Fr,
        serial: Fr,
        sk: Fr,
        commitment_fr: Fr,
        nullifier_fr: Fr,
        merkle_path: Vec<Fr>,
        path_bits: Vec<bool>,
    }

    fn build_inputs_and_tree(values: [u64; 2]) -> (Fr, [TestInput; 2]) {
        let mut tree = Bn254MerkleTree::new();

        let inputs: Vec<TestInput> = values
            .iter()
            .map(|&val| {
                let blinding = Fr::rand(&mut OsRng);
                let serial = Fr::rand(&mut OsRng);
                let sk = Fr::rand(&mut OsRng);
                let commitment_fr = poseidon_hash_fr(Fr::from(val), blinding);
                let nullifier_fr = poseidon_hash_fr(serial, sk);
                tree.insert(commitment_fr);
                TestInput {
                    value: val,
                    blinding,
                    serial,
                    sk,
                    commitment_fr,
                    nullifier_fr,
                    merkle_path: vec![], // filled after tree complete
                    path_bits: vec![],
                }
            })
            .collect();

        let merkle_root_fr = tree.root();

        let mut result: Vec<TestInput> = Vec::with_capacity(2);
        for (idx, input) in inputs.into_iter().enumerate() {
            let proof = tree.proof(idx as u64).unwrap();
            result.push(TestInput {
                merkle_path: proof.siblings,
                path_bits: proof.path_bits,
                ..input
            });
        }

        let [r0, r1] = <[TestInput; 2]>::try_from(result).ok().unwrap();
        (merkle_root_fr, [r0, r1])
    }

    fn valid_transfer(in_values: [u64; 2], out_values: [u64; 2]) -> TransferCircuit {
        let (merkle_root, inputs) = build_inputs_and_tree(in_values);

        let out_blindings = [Fr::rand(&mut OsRng), Fr::rand(&mut OsRng)];
        let out_commitments = [
            poseidon_hash_fr(Fr::from(out_values[0]), out_blindings[0]),
            poseidon_hash_fr(Fr::from(out_values[1]), out_blindings[1]),
        ];

        TransferCircuit::new(
            merkle_root,
            [inputs[0].nullifier_fr, inputs[1].nullifier_fr],
            out_commitments,
            in_values,
            [inputs[0].blinding, inputs[1].blinding],
            [inputs[0].serial, inputs[1].serial],
            [inputs[0].sk, inputs[1].sk],
            [inputs[0].merkle_path.clone(), inputs[1].merkle_path.clone()],
            [inputs[0].path_bits.clone(), inputs[1].path_bits.clone()],
            out_values,
            out_blindings,
        )
    }

    #[test]
    fn test_transfer_valid_conservation() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        // 700 + 300 = 600 + 400
        let circuit = valid_transfer([700, 300], [600, 400]);
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(cs.is_satisfied().unwrap(), "valid transfer not satisfied");
    }

    #[test]
    fn test_transfer_valid_equal_split() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        // 500 + 500 = 500 + 500
        let circuit = valid_transfer([500, 500], [500, 500]);
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(cs.is_satisfied().unwrap());
    }

    #[test]
    fn test_transfer_invalid_conservation_fails() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let (merkle_root, inputs) = build_inputs_and_tree([700, 300]);

        let out_blindings = [Fr::rand(&mut OsRng), Fr::rand(&mut OsRng)];
        // Claim output 600 + 500 = 1100, but input is 1000 → fails conservation
        let out_commitments = [
            poseidon_hash_fr(Fr::from(600u64), out_blindings[0]),
            poseidon_hash_fr(Fr::from(500u64), out_blindings[1]),
        ];

        let circuit = TransferCircuit::new(
            merkle_root,
            [inputs[0].nullifier_fr, inputs[1].nullifier_fr],
            out_commitments,
            [700, 300],
            [inputs[0].blinding, inputs[1].blinding],
            [inputs[0].serial, inputs[1].serial],
            [inputs[0].sk, inputs[1].sk],
            [inputs[0].merkle_path.clone(), inputs[1].merkle_path.clone()],
            [inputs[0].path_bits.clone(), inputs[1].path_bits.clone()],
            [600, 500],
            out_blindings,
        );
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(!cs.is_satisfied().unwrap());
    }

    #[test]
    fn test_transfer_wrong_nullifier_fails() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let (merkle_root, inputs) = build_inputs_and_tree([700, 300]);

        let out_blindings = [Fr::rand(&mut OsRng), Fr::rand(&mut OsRng)];
        let out_commitments = [
            poseidon_hash_fr(Fr::from(600u64), out_blindings[0]),
            poseidon_hash_fr(Fr::from(400u64), out_blindings[1]),
        ];

        // Use a wrong nullifier for input 0
        let circuit = TransferCircuit::new(
            merkle_root,
            [Fr::from(99999u64), inputs[1].nullifier_fr], // wrong!
            out_commitments,
            [700, 300],
            [inputs[0].blinding, inputs[1].blinding],
            [inputs[0].serial, inputs[1].serial],
            [inputs[0].sk, inputs[1].sk],
            [inputs[0].merkle_path.clone(), inputs[1].merkle_path.clone()],
            [inputs[0].path_bits.clone(), inputs[1].path_bits.clone()],
            [600, 400],
            out_blindings,
        );
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(!cs.is_satisfied().unwrap());
    }

    #[test]
    fn test_transfer_wrong_output_commitment_fails() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let (merkle_root, inputs) = build_inputs_and_tree([700, 300]);

        let out_blindings = [Fr::rand(&mut OsRng), Fr::rand(&mut OsRng)];

        // Correct output commitment for [0], wrong for [1]
        let out_commitments = [
            poseidon_hash_fr(Fr::from(600u64), out_blindings[0]),
            Fr::from(12345u64), // wrong!
        ];

        let circuit = TransferCircuit::new(
            merkle_root,
            [inputs[0].nullifier_fr, inputs[1].nullifier_fr],
            out_commitments,
            [700, 300],
            [inputs[0].blinding, inputs[1].blinding],
            [inputs[0].serial, inputs[1].serial],
            [inputs[0].sk, inputs[1].sk],
            [inputs[0].merkle_path.clone(), inputs[1].merkle_path.clone()],
            [inputs[0].path_bits.clone(), inputs[1].path_bits.clone()],
            [600, 400],
            out_blindings,
        );
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(!cs.is_satisfied().unwrap());
    }

    #[test]
    fn test_transfer_empty_for_setup() {
        use ark_relations::r1cs::{ConstraintSystem, OptimizationGoal};
        let cs = ConstraintSystem::<Fr>::new_ref();
        cs.set_optimization_goal(OptimizationGoal::Constraints);
        cs.set_mode(ark_relations::r1cs::SynthesisMode::Setup);
        let circuit = TransferCircuit::empty();
        let result = circuit.generate_constraints(cs.clone());
        assert!(
            result.is_ok(),
            "empty transfer circuit failed in setup: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_transfer_constraint_count() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let circuit = valid_transfer([700, 300], [600, 400]);
        circuit.generate_constraints(cs.clone()).unwrap();
        let num = cs.num_constraints();
        // 2 inputs × (nullifier Poseidon + commitment Poseidon + 32 Merkle Poseidon + range check)
        // + 2 outputs × (commitment Poseidon + range check) + value conservation
        // ≈ 2 × (300 + 300 + 32×300 + 254) + 2 × (300 + 254) + 1 ≈ 21,000
        println!("Transfer circuit constraint count: {}", num);
        assert!(
            (10000..=50000).contains(&num),
            "constraint count {} outside expected range [10000, 50000]",
            num
        );
    }
}
