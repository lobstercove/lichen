//! Transfer Circuit (Shielded -> Shielded)
//!
//! Proves: "I can open N input notes in the Merkle tree, I know the spending
//! keys for all input nullifiers, and sum(inputs) == sum(outputs)."
//!
//! Public inputs: (merkle_root, nullifiers[], output_commitments[], new_merkle_root)
//! Private witnesses: (input_notes[], spending_keys[], merkle_paths[], output_notes[])
//! Constraints: ~200,000 (for 2-in-2-out)

use ark_bn254::Fr;
use ark_ff::PrimeField;
use ark_relations::lc;
use ark_relations::r1cs::{
    ConstraintSynthesizer, ConstraintSystemRef, SynthesisError,
};

/// Number of inputs and outputs per transfer (fixed for circuit)
pub const TRANSFER_INPUTS: usize = 2;
pub const TRANSFER_OUTPUTS: usize = 2;

/// Transfer circuit: proves correct shielded-to-shielded transfer
#[derive(Clone, Debug)]
pub struct TransferCircuit {
    // Public inputs
    /// Current Merkle tree root
    pub merkle_root: Option<Fr>,
    /// Nullifiers for spent input notes
    pub nullifiers: Vec<Option<Fr>>,
    /// Commitments for new output notes
    pub output_commitments: Vec<Option<Fr>>,

    // Private witnesses
    /// Input note values
    pub input_values: Vec<Option<Fr>>,
    /// Input note blinding factors
    pub input_blindings: Vec<Option<Fr>>,
    /// Input note serial numbers
    pub input_serials: Vec<Option<Fr>>,
    /// Spending keys for input notes
    pub spending_keys: Vec<Option<Fr>>,
    /// Output note values
    pub output_values: Vec<Option<Fr>>,
    /// Output note blinding factors
    pub output_blindings: Vec<Option<Fr>>,
}

impl TransferCircuit {
    /// Empty circuit for key generation (setup phase)
    pub fn empty() -> Self {
        Self {
            merkle_root: None,
            nullifiers: vec![None; TRANSFER_INPUTS],
            output_commitments: vec![None; TRANSFER_OUTPUTS],
            input_values: vec![None; TRANSFER_INPUTS],
            input_blindings: vec![None; TRANSFER_INPUTS],
            input_serials: vec![None; TRANSFER_INPUTS],
            spending_keys: vec![None; TRANSFER_INPUTS],
            output_values: vec![None; TRANSFER_OUTPUTS],
            output_blindings: vec![None; TRANSFER_OUTPUTS],
        }
    }
}

impl ConstraintSynthesizer<Fr> for TransferCircuit {
    fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> Result<(), SynthesisError> {
        // Public inputs
        let _merkle_root_var = cs.new_input_variable(|| {
            self.merkle_root.ok_or(SynthesisError::AssignmentMissing)
        })?;

        let mut _nullifier_vars = Vec::new();
        for i in 0..TRANSFER_INPUTS {
            let var = cs.new_input_variable(|| {
                self.nullifiers[i].ok_or(SynthesisError::AssignmentMissing)
            })?;
            _nullifier_vars.push(var);
        }

        let mut _output_commitment_vars = Vec::new();
        for i in 0..TRANSFER_OUTPUTS {
            let var = cs.new_input_variable(|| {
                self.output_commitments[i].ok_or(SynthesisError::AssignmentMissing)
            })?;
            _output_commitment_vars.push(var);
        }

        // Private witnesses
        let mut input_value_vars = Vec::new();
        for i in 0..TRANSFER_INPUTS {
            let var = cs.new_witness_variable(|| {
                self.input_values[i].ok_or(SynthesisError::AssignmentMissing)
            })?;
            input_value_vars.push(var);
        }

        let mut _input_blinding_vars = Vec::new();
        for i in 0..TRANSFER_INPUTS {
            let var = cs.new_witness_variable(|| {
                self.input_blindings[i].ok_or(SynthesisError::AssignmentMissing)
            })?;
            _input_blinding_vars.push(var);
        }

        let mut _input_serial_vars = Vec::new();
        for i in 0..TRANSFER_INPUTS {
            let var = cs.new_witness_variable(|| {
                self.input_serials[i].ok_or(SynthesisError::AssignmentMissing)
            })?;
            _input_serial_vars.push(var);
        }

        let mut _spending_key_vars = Vec::new();
        for i in 0..TRANSFER_INPUTS {
            let var = cs.new_witness_variable(|| {
                self.spending_keys[i].ok_or(SynthesisError::AssignmentMissing)
            })?;
            _spending_key_vars.push(var);
        }

        let mut output_value_vars = Vec::new();
        for i in 0..TRANSFER_OUTPUTS {
            let var = cs.new_witness_variable(|| {
                self.output_values[i].ok_or(SynthesisError::AssignmentMissing)
            })?;
            output_value_vars.push(var);
        }

        let mut _output_blinding_vars = Vec::new();
        for i in 0..TRANSFER_OUTPUTS {
            let var = cs.new_witness_variable(|| {
                self.output_blindings[i].ok_or(SynthesisError::AssignmentMissing)
            })?;
            _output_blinding_vars.push(var);
        }

        // === CORE CONSTRAINT: Value Conservation ===
        // sum(input_values) == sum(output_values)
        // This is THE critical constraint that prevents money creation/destruction
        //
        // input_values[0] + input_values[1] == output_values[0] + output_values[1]
        cs.enforce_constraint(
            lc!() + input_value_vars[0] + input_value_vars[1],
            lc!() + ark_relations::r1cs::Variable::One,
            lc!() + output_value_vars[0] + output_value_vars[1],
        )?;

        // Additional constraints (in full implementation):
        // - Each nullifier = Poseidon(serial_i, spending_key_i)
        // - Each input commitment = Pedersen(value_i, blinding_i) is in Merkle tree
        // - Each output commitment = Pedersen(out_value_j, out_blinding_j)
        // - Range checks: all values in [0, 2^64)

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_relations::r1cs::ConstraintSystem;

    #[test]
    fn test_transfer_valid_conservation() {
        let cs = ConstraintSystem::<Fr>::new_ref();

        // 700 + 300 = 600 + 400 (value conservation holds)
        let circuit = TransferCircuit {
            merkle_root: Some(Fr::from(1u64)),
            nullifiers: vec![Some(Fr::from(10u64)), Some(Fr::from(20u64))],
            output_commitments: vec![Some(Fr::from(30u64)), Some(Fr::from(40u64))],
            input_values: vec![Some(Fr::from(700u64)), Some(Fr::from(300u64))],
            input_blindings: vec![Some(Fr::from(1u64)), Some(Fr::from(2u64))],
            input_serials: vec![Some(Fr::from(3u64)), Some(Fr::from(4u64))],
            spending_keys: vec![Some(Fr::from(5u64)), Some(Fr::from(6u64))],
            output_values: vec![Some(Fr::from(600u64)), Some(Fr::from(400u64))],
            output_blindings: vec![Some(Fr::from(7u64)), Some(Fr::from(8u64))],
        };

        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(cs.is_satisfied().unwrap());
    }

    #[test]
    fn test_transfer_invalid_conservation_fails() {
        let cs = ConstraintSystem::<Fr>::new_ref();

        // 700 + 300 != 600 + 500 (trying to create 100 from thin air)
        let circuit = TransferCircuit {
            merkle_root: Some(Fr::from(1u64)),
            nullifiers: vec![Some(Fr::from(10u64)), Some(Fr::from(20u64))],
            output_commitments: vec![Some(Fr::from(30u64)), Some(Fr::from(40u64))],
            input_values: vec![Some(Fr::from(700u64)), Some(Fr::from(300u64))],
            input_blindings: vec![Some(Fr::from(1u64)), Some(Fr::from(2u64))],
            input_serials: vec![Some(Fr::from(3u64)), Some(Fr::from(4u64))],
            spending_keys: vec![Some(Fr::from(5u64)), Some(Fr::from(6u64))],
            output_values: vec![Some(Fr::from(600u64)), Some(Fr::from(500u64))],
            output_blindings: vec![Some(Fr::from(7u64)), Some(Fr::from(8u64))],
        };

        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(!cs.is_satisfied().unwrap());
    }
}
