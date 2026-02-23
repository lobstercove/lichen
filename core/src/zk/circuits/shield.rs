//! Shield Circuit (Transparent -> Shielded)
//!
//! Proves: "I deposited `amount` into the shielded pool and know the blinding
//! factor such that `commitment = Poseidon(value, blinding)` where `value == amount`."
//!
//! Public inputs:
//!   1. `amount`     — the deposit amount (visible on-chain)
//!   2. `commitment` — Poseidon(value, blinding), the Merkle-tree leaf
//!
//! Private witnesses:
//!   - `value`    — the actual note value (must equal `amount`)
//!   - `blinding` — the random blinding factor
//!
//! Constraints:
//!   1. value == amount                           (1 R1CS constraint)
//!   2. Poseidon(value, blinding) == commitment   (~300 Poseidon constraints)
//!   3. value fits in 64 bits (range check)       (64 bit-decomposition constraints)
//!
//! Total: ~370 constraints

use ark_bn254::Fr;
use ark_crypto_primitives::sponge::constraints::CryptographicSpongeVar;
use ark_crypto_primitives::sponge::poseidon::constraints::PoseidonSpongeVar;
use ark_crypto_primitives::sponge::poseidon::PoseidonConfig;
use ark_r1cs_std::fields::fp::FpVar;
use ark_r1cs_std::prelude::*;
use ark_relations::r1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError};

use crate::zk::merkle::poseidon_config;

/// Shield circuit: proves correct deposit into shielded pool
#[derive(Clone, Debug)]
pub struct ShieldCircuit {
    /// Poseidon config (must match the config used in native commitment computation)
    pub poseidon_config: PoseidonConfig<Fr>,

    // Public inputs
    /// The deposit amount (public, visible on-chain)
    pub amount: Option<Fr>,
    /// The Poseidon commitment to the note (public, stored in Merkle tree)
    pub commitment: Option<Fr>,

    // Private witnesses
    /// The actual value (must equal amount)
    pub value: Option<Fr>,
    /// The blinding factor
    pub blinding: Option<Fr>,
}

impl ShieldCircuit {
    /// Create a new shield circuit with concrete witness values.
    ///
    /// `amount` and `value` should be equal for a valid proof.
    /// `commitment` should equal `poseidon_hash_fr(Fr::from(value), blinding)`.
    pub fn new(amount: u64, value: u64, blinding: Fr, commitment: Fr) -> Self {
        Self {
            poseidon_config: poseidon_config(),
            amount: Some(Fr::from(amount)),
            commitment: Some(commitment),
            value: Some(Fr::from(value)),
            blinding: Some(blinding),
        }
    }

    /// Empty circuit for key generation (setup/ceremony phase).
    ///
    /// All witness and public input slots are `None`; the circuit structure
    /// (number and shape of constraints) is fully determined without values.
    pub fn empty() -> Self {
        Self {
            poseidon_config: poseidon_config(),
            amount: None,
            commitment: None,
            value: None,
            blinding: None,
        }
    }
}

impl ConstraintSynthesizer<Fr> for ShieldCircuit {
    fn generate_constraints(self, cs: ConstraintSystemRef<Fr>) -> Result<(), SynthesisError> {
        // ── Public inputs ──────────────────────────────────────────────
        let amount_var =
            FpVar::new_input(cs.clone(), || self.amount.ok_or(SynthesisError::AssignmentMissing))?;

        let commitment_var = FpVar::new_input(cs.clone(), || {
            self.commitment.ok_or(SynthesisError::AssignmentMissing)
        })?;

        // ── Private witnesses ──────────────────────────────────────────
        let value_var = FpVar::new_witness(cs.clone(), || {
            self.value.ok_or(SynthesisError::AssignmentMissing)
        })?;

        let blinding_var = FpVar::new_witness(cs.clone(), || {
            self.blinding.ok_or(SynthesisError::AssignmentMissing)
        })?;

        // ── Constraint 1: value == amount ──────────────────────────────
        // The private witness `value` must exactly equal the public `amount`.
        value_var.enforce_equal(&amount_var)?;

        // ── Constraint 2: Poseidon(value, blinding) == commitment ──────
        // Compute the Poseidon hash in-circuit using the same config and
        // absorb order as the native `poseidon_hash_fr(value, blinding)`.
        let mut sponge = PoseidonSpongeVar::new(cs.clone(), &self.poseidon_config);
        sponge.absorb(&value_var)?;
        sponge.absorb(&blinding_var)?;
        let computed_commitment = sponge.squeeze_field_elements(1)?;

        // The in-circuit Poseidon output must equal the public commitment.
        computed_commitment[0].enforce_equal(&commitment_var)?;

        // ── Constraint 3: 64-bit range check on value ──────────────────
        // Decompose `value` into 64 bits and enforce the decomposition is
        // valid. This proves value ∈ [0, 2^64) and prevents overflow /
        // negative-value attacks. Each bit b_i satisfies b_i * (1 - b_i) = 0
        // (boolean constraint) and Σ b_i * 2^i == value.
        let value_bits = value_var.to_bits_le()?;
        // Enforce bits [64..] are all zero → value < 2^64.
        // BN254 scalar field is ~254 bits, so to_bits_le() returns ~254 bits.
        for bit in value_bits.iter().skip(64) {
            bit.enforce_equal(&Boolean::FALSE)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zk::merkle::poseidon_hash_fr;
    use ark_ff::UniformRand;
    use ark_relations::r1cs::ConstraintSystem;
    use ark_std::rand::rngs::OsRng;

    /// Helper: build a valid shield circuit for the given amount.
    fn valid_circuit(amount: u64) -> ShieldCircuit {
        let blinding = Fr::rand(&mut OsRng);
        let commitment = poseidon_hash_fr(Fr::from(amount), blinding);
        ShieldCircuit::new(amount, amount, blinding, commitment)
    }

    #[test]
    fn test_shield_circuit_satisfies() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let circuit = valid_circuit(1_000_000_000); // 1 MOLT
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(cs.is_satisfied().unwrap());
        // Expect non-trivial constraint count (Poseidon + range check + equality)
        let num = cs.num_constraints();
        assert!(num > 100, "expected >100 constraints, got {}", num);
    }

    #[test]
    fn test_shield_circuit_wrong_amount_fails() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let blinding = Fr::rand(&mut OsRng);
        let commitment = poseidon_hash_fr(Fr::from(1000u64), blinding);

        // value (2000) != amount (1000) → must fail
        let circuit = ShieldCircuit::new(1000, 2000, blinding, commitment);
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(!cs.is_satisfied().unwrap());
    }

    #[test]
    fn test_shield_circuit_wrong_commitment_fails() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let blinding = Fr::rand(&mut OsRng);
        let wrong_commitment = Fr::from(42u64); // does not match Poseidon(value, blinding)

        let circuit = ShieldCircuit::new(1000, 1000, blinding, wrong_commitment);
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(!cs.is_satisfied().unwrap());
    }

    #[test]
    fn test_shield_circuit_value_range_check() {
        // Value within 64-bit range should pass
        let cs = ConstraintSystem::<Fr>::new_ref();
        let circuit = valid_circuit(u64::MAX);
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(cs.is_satisfied().unwrap());
    }

    #[test]
    fn test_shield_circuit_empty_for_setup() {
        // Empty circuit (for trusted setup) must define the same constraint
        // structure without panicking. arkworks setup mode sets the optimization
        // goal which suppresses AssignmentMissing during key generation.
        use ark_relations::r1cs::{ConstraintSystem, OptimizationGoal};

        let cs = ConstraintSystem::<Fr>::new_ref();
        cs.set_optimization_goal(OptimizationGoal::Constraints);
        cs.set_mode(ark_relations::r1cs::SynthesisMode::Setup);
        let circuit = ShieldCircuit::empty();
        // generate_constraints should not panic in setup mode
        let result = circuit.generate_constraints(cs.clone());
        assert!(result.is_ok(), "empty circuit failed in setup mode: {:?}", result.err());
    }

    #[test]
    fn test_shield_circuit_different_blindings_different_commitments() {
        let b1 = Fr::rand(&mut OsRng);
        let b2 = Fr::rand(&mut OsRng);
        let c1 = poseidon_hash_fr(Fr::from(1000u64), b1);
        let c2 = poseidon_hash_fr(Fr::from(1000u64), b2);
        assert_ne!(c1, c2, "different blindings should give different commitments");

        // Both should produce satisfied circuits with their own commitments
        for (blinding, commitment) in [(b1, c1), (b2, c2)] {
            let cs = ConstraintSystem::<Fr>::new_ref();
            let circuit = ShieldCircuit::new(1000, 1000, blinding, commitment);
            circuit.generate_constraints(cs.clone()).unwrap();
            assert!(cs.is_satisfied().unwrap());
        }
    }

    #[test]
    fn test_shield_circuit_zero_amount() {
        // Zero deposits should be valid (the pool allows them; policy
        // enforcement happens at the transaction level, not circuit level)
        let cs = ConstraintSystem::<Fr>::new_ref();
        let circuit = valid_circuit(0);
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(cs.is_satisfied().unwrap());
    }

    #[test]
    fn test_shield_circuit_constraint_count() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let circuit = valid_circuit(500);
        circuit.generate_constraints(cs.clone()).unwrap();
        let num = cs.num_constraints();
        // Poseidon with our config: ~300 constraints
        // 64-bit range check: ~254 boolean + 190 high-bit zero + 1 recomposition ≈ 445
        // value == amount: 1
        // commitment ==: 1
        // Total should be in the range [300, 1500]
        assert!(
            num >= 200 && num <= 2000,
            "constraint count {} outside expected range [200, 2000]",
            num
        );
        println!("Shield circuit constraint count: {}", num);
    }
}
