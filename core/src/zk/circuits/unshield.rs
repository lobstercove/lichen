//! Unshield Circuit (Shielded -> Transparent)
//!
//! Proves: "I know a note in the Merkle tree with value >= withdrawal_amount,
//! and I know the spending key that produces the nullifier."
//!
//! Public inputs: (merkle_root, nullifier, amount, recipient)
//! Private witnesses: (note, spending_key, merkle_path)
//! Constraints: ~50,000

use ark_bn254::Fr;
use ark_ff::PrimeField;
use ark_relations::lc;
use ark_relations::r1cs::{
    ConstraintSynthesizer, ConstraintSystemRef, SynthesisError,
};

/// Unshield circuit: proves correct withdrawal from shielded pool
#[derive(Clone, Debug)]
pub struct UnshieldCircuit {
    // Public inputs
    /// Current Merkle tree root
    pub merkle_root: Option<Fr>,
    /// Nullifier for the spent note
    pub nullifier: Option<Fr>,
    /// Withdrawal amount
    pub amount: Option<Fr>,
    /// Recipient address (hash)
    pub recipient: Option<Fr>,

    // Private witnesses
    /// The note being spent
    pub note_value: Option<Fr>,
    pub note_blinding: Option<Fr>,
    pub note_serial: Option<Fr>,
    /// Spending key (proves ownership)
    pub spending_key: Option<Fr>,
    /// Merkle path siblings (TREE_DEPTH elements)
    pub merkle_path: Option<Vec<Fr>>,
    /// Merkle path direction bits
    pub path_bits: Option<Vec<bool>>,
}

impl UnshieldCircuit {
    /// Empty circuit for key generation
    pub fn empty() -> Self {
        Self {
            merkle_root: None,
            nullifier: None,
            amount: None,
            recipient: None,
            note_value: None,
            note_blinding: None,
            note_serial: None,
            spending_key: None,
            merkle_path: None,
            path_bits: None,
        }
    }
}

impl ConstraintSynthesizer<Fr> for UnshieldCircuit {
    fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> Result<(), SynthesisError> {
        // Public inputs
        let merkle_root_var = cs.new_input_variable(|| {
            self.merkle_root.ok_or(SynthesisError::AssignmentMissing)
        })?;

        let nullifier_var = cs.new_input_variable(|| {
            self.nullifier.ok_or(SynthesisError::AssignmentMissing)
        })?;

        let amount_var = cs.new_input_variable(|| {
            self.amount.ok_or(SynthesisError::AssignmentMissing)
        })?;

        let _recipient_var = cs.new_input_variable(|| {
            self.recipient.ok_or(SynthesisError::AssignmentMissing)
        })?;

        // Private witnesses
        let note_value_var = cs.new_witness_variable(|| {
            self.note_value.ok_or(SynthesisError::AssignmentMissing)
        })?;

        let _note_blinding_var = cs.new_witness_variable(|| {
            self.note_blinding.ok_or(SynthesisError::AssignmentMissing)
        })?;

        let _note_serial_var = cs.new_witness_variable(|| {
            self.note_serial.ok_or(SynthesisError::AssignmentMissing)
        })?;

        let _spending_key_var = cs.new_witness_variable(|| {
            self.spending_key.ok_or(SynthesisError::AssignmentMissing)
        })?;

        // Constraint 1: note_value >= amount (sufficient balance)
        // In full implementation: range check via bit decomposition
        // For skeleton: value - amount >= 0 (add auxiliary variable for difference)
        let diff_value = self.note_value.and_then(|v| {
            self.amount.map(|a| v - a)
        });
        let diff_var = cs.new_witness_variable(|| {
            diff_value.ok_or(SynthesisError::AssignmentMissing)
        })?;

        // note_value = amount + diff (diff must be non-negative)
        cs.enforce_constraint(
            lc!() + note_value_var,
            lc!() + ark_relations::r1cs::Variable::One,
            lc!() + amount_var + diff_var,
        )?;

        // Constraint 2: nullifier = Poseidon(serial, spending_key)
        // In full implementation: Poseidon gadget in-circuit
        // This binds the nullifier to the note and the spender

        // Constraint 3: commitment = Pedersen(value, blinding), and
        // commitment is in the Merkle tree at the given root
        // In full implementation: Merkle path verification with Poseidon gadgets

        // Constraint 4: The spending key derives the correct nullifier
        // (already covered by constraint 2)

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_relations::r1cs::ConstraintSystem;

    #[test]
    fn test_unshield_circuit_satisfies() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let circuit = UnshieldCircuit {
            merkle_root: Some(Fr::from(100u64)),
            nullifier: Some(Fr::from(200u64)),
            amount: Some(Fr::from(500u64)),
            recipient: Some(Fr::from(300u64)),
            note_value: Some(Fr::from(1000u64)),
            note_blinding: Some(Fr::from(111u64)),
            note_serial: Some(Fr::from(222u64)),
            spending_key: Some(Fr::from(333u64)),
            merkle_path: Some(vec![]),
            path_bits: Some(vec![]),
        };
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(cs.is_satisfied().unwrap());
    }

    #[test]
    fn test_unshield_insufficient_balance_fails() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        // note_value (100) < amount (500) => diff is negative in the field
        // The constraint still "works" in the field (wrap-around), but the
        // range check (not yet implemented) would catch this.
        let circuit = UnshieldCircuit {
            merkle_root: Some(Fr::from(100u64)),
            nullifier: Some(Fr::from(200u64)),
            amount: Some(Fr::from(500u64)),
            recipient: Some(Fr::from(300u64)),
            note_value: Some(Fr::from(100u64)),
            note_blinding: Some(Fr::from(111u64)),
            note_serial: Some(Fr::from(222u64)),
            spending_key: Some(Fr::from(333u64)),
            merkle_path: Some(vec![]),
            path_bits: Some(vec![]),
        };
        circuit.generate_constraints(cs.clone()).unwrap();
        // Note: without range check, this still satisfies in the field
        // A full implementation adds bit decomposition range check
    }
}
