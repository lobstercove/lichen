//! Unshield Circuit (Shielded -> Transparent)
//!
//! Proves: "I own a note in the Merkle tree and I'm withdrawing `amount` from it."
//!
//! Public inputs:
//!   1. `merkle_root` — current Merkle tree root (anchors the proof to a state)
//!   2. `nullifier`   — Poseidon(serial, spending_key), marks note as spent
//!   3. `amount`      — withdrawal amount (visible on-chain)
//!   4. `recipient`   — hash identifying the transparent recipient
//!
//! Private witnesses:
//!   - `note_value`    — the note's value
//!   - `note_blinding` — the note's blinding factor
//!   - `note_serial`   — the note's serial number
//!   - `spending_key`  — proves ownership (derives the nullifier)
//!   - `merkle_path`   — TREE_DEPTH sibling hashes from leaf to root
//!   - `path_bits`     — TREE_DEPTH direction bits (true = leaf is right child)
//!
//! Constraints:
//!   1. note_value == amount  (exact withdrawal, no change)
//!   2. nullifier == Poseidon(serial, spending_key)
//!   3. commitment == Poseidon(value, blinding)
//!   4. Merkle path from commitment to merkle_root is valid (32 Poseidon hashes)
//!   5. 64-bit range check on value
//!
//! Design note: we enforce value == amount (exact match) rather than value >= amount
//! to avoid needing a change output. If policy requires partial withdrawal, use
//! the Transfer circuit to split the note first.

use ark_bn254::Fr;
use ark_crypto_primitives::sponge::constraints::CryptographicSpongeVar;
use ark_crypto_primitives::sponge::poseidon::constraints::PoseidonSpongeVar;
use ark_crypto_primitives::sponge::poseidon::PoseidonConfig;
use ark_r1cs_std::fields::fp::FpVar;
use ark_r1cs_std::prelude::*;
use ark_relations::r1cs::{ConstraintSynthesizer, ConstraintSystemRef, SynthesisError};

use crate::zk::merkle::{poseidon_config, TREE_DEPTH};

/// Compute Poseidon(left, right) in-circuit using the given sponge config.
/// Mirrors the native `poseidon_hash_fr(left, right)`.
fn poseidon_hash_var(
    cs: ConstraintSystemRef<Fr>,
    config: &PoseidonConfig<Fr>,
    left: &FpVar<Fr>,
    right: &FpVar<Fr>,
) -> Result<FpVar<Fr>, SynthesisError> {
    let mut sponge = PoseidonSpongeVar::new(cs, config);
    sponge.absorb(left)?;
    sponge.absorb(right)?;
    let out = sponge.squeeze_field_elements(1)?;
    Ok(out[0].clone())
}

/// Unshield circuit: proves correct withdrawal from shielded pool
#[derive(Clone, Debug)]
pub struct UnshieldCircuit {
    /// Poseidon config (must match native computation)
    pub poseidon_config: PoseidonConfig<Fr>,

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
    /// Merkle path direction bits (true = leaf is right child at that level)
    pub path_bits: Option<Vec<bool>>,
}

impl UnshieldCircuit {
    /// Create a new unshield circuit with concrete witness values.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        merkle_root: Fr,
        nullifier: Fr,
        amount: u64,
        recipient: Fr,
        note_value: u64,
        note_blinding: Fr,
        note_serial: Fr,
        spending_key: Fr,
        merkle_path: Vec<Fr>,
        path_bits: Vec<bool>,
    ) -> Self {
        assert_eq!(merkle_path.len(), TREE_DEPTH);
        assert_eq!(path_bits.len(), TREE_DEPTH);
        Self {
            poseidon_config: poseidon_config(),
            merkle_root: Some(merkle_root),
            nullifier: Some(nullifier),
            amount: Some(Fr::from(amount)),
            recipient: Some(recipient),
            note_value: Some(Fr::from(note_value)),
            note_blinding: Some(note_blinding),
            note_serial: Some(note_serial),
            spending_key: Some(spending_key),
            merkle_path: Some(merkle_path),
            path_bits: Some(path_bits),
        }
    }

    /// Empty circuit for key generation (setup/ceremony phase).
    pub fn empty() -> Self {
        Self {
            poseidon_config: poseidon_config(),
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
        let config = &self.poseidon_config;

        // ── Public inputs ──────────────────────────────────────────────
        let merkle_root_var = FpVar::new_input(cs.clone(), || {
            self.merkle_root.ok_or(SynthesisError::AssignmentMissing)
        })?;

        let nullifier_var = FpVar::new_input(cs.clone(), || {
            self.nullifier.ok_or(SynthesisError::AssignmentMissing)
        })?;

        let amount_var = FpVar::new_input(cs.clone(), || {
            self.amount.ok_or(SynthesisError::AssignmentMissing)
        })?;

        let _recipient_var = FpVar::new_input(cs.clone(), || {
            self.recipient.ok_or(SynthesisError::AssignmentMissing)
        })?;

        // ── Private witnesses ──────────────────────────────────────────
        let value_var = FpVar::new_witness(cs.clone(), || {
            self.note_value.ok_or(SynthesisError::AssignmentMissing)
        })?;

        let blinding_var = FpVar::new_witness(cs.clone(), || {
            self.note_blinding.ok_or(SynthesisError::AssignmentMissing)
        })?;

        let serial_var = FpVar::new_witness(cs.clone(), || {
            self.note_serial.ok_or(SynthesisError::AssignmentMissing)
        })?;

        let sk_var = FpVar::new_witness(cs.clone(), || {
            self.spending_key.ok_or(SynthesisError::AssignmentMissing)
        })?;

        // Merkle path siblings
        let path = self.merkle_path.unwrap_or_else(|| vec![Fr::from(0u64); TREE_DEPTH]);
        let sibling_vars: Vec<FpVar<Fr>> = path
            .iter()
            .map(|s| FpVar::new_witness(cs.clone(), || Ok(*s)))
            .collect::<Result<_, _>>()?;

        // Merkle path direction bits
        let bits = self.path_bits.unwrap_or_else(|| vec![false; TREE_DEPTH]);
        let path_bit_vars: Vec<Boolean<Fr>> = bits
            .iter()
            .map(|b| Boolean::new_witness(cs.clone(), || Ok(*b)))
            .collect::<Result<_, _>>()?;

        // ── Constraint 1: value == amount ──────────────────────────────
        value_var.enforce_equal(&amount_var)?;

        // ── Constraint 2: nullifier == Poseidon(serial, spending_key) ──
        let computed_nullifier = poseidon_hash_var(cs.clone(), config, &serial_var, &sk_var)?;
        computed_nullifier.enforce_equal(&nullifier_var)?;

        // ── Constraint 3: commitment == Poseidon(value, blinding) ──────
        let commitment_var = poseidon_hash_var(cs.clone(), config, &value_var, &blinding_var)?;

        // ── Constraint 4: Merkle path verification ─────────────────────
        // Start from the commitment leaf, hash up TREE_DEPTH levels.
        // At each level, the path_bit determines if our node is left or right:
        //   path_bit = false → (current, sibling)
        //   path_bit = true  → (sibling, current)
        let mut current = commitment_var;
        for i in 0..TREE_DEPTH {
            // Conditional select: if path_bit is true, left = sibling, right = current
            // Otherwise, left = current, right = sibling
            let left =
                FpVar::conditionally_select(&path_bit_vars[i], &sibling_vars[i], &current)?;
            let right =
                FpVar::conditionally_select(&path_bit_vars[i], &current, &sibling_vars[i])?;

            current = poseidon_hash_var(cs.clone(), config, &left, &right)?;
        }
        // The final hash must equal the public Merkle root
        current.enforce_equal(&merkle_root_var)?;

        // ── Constraint 5: 64-bit range check on value ─────────────────
        let value_bits = value_var.to_bits_le()?;
        for bit in value_bits.iter().skip(64) {
            bit.enforce_equal(&Boolean::FALSE)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zk::merkle::{fr_to_bytes, poseidon_hash_fr, MerkleTree};
    use ark_ff::{PrimeField, UniformRand};
    use ark_relations::r1cs::ConstraintSystem;
    use ark_std::rand::rngs::OsRng;

    /// Build a valid unshield circuit: insert a note into a Merkle tree,
    /// generate a proof, and construct the circuit with matching witnesses.
    fn valid_unshield(amount: u64) -> UnshieldCircuit {
        let blinding = Fr::rand(&mut OsRng);
        let serial = Fr::rand(&mut OsRng);
        let spending_key = Fr::rand(&mut OsRng);

        // Compute the commitment leaf and nullifier (native)
        let commitment_fr = poseidon_hash_fr(Fr::from(amount), blinding);
        let nullifier_fr = poseidon_hash_fr(serial, spending_key);

        let commitment_bytes = fr_to_bytes(&commitment_fr);

        // Insert into Merkle tree and get proof
        let mut tree = MerkleTree::new();
        tree.insert(commitment_bytes);
        let merkle_root = tree.root();
        let proof = tree.proof(0).expect("proof for index 0");

        // Convert proof to Fr
        let merkle_root_fr = Fr::from_le_bytes_mod_order(&merkle_root);
        let merkle_path: Vec<Fr> = proof
            .siblings
            .iter()
            .map(|s| Fr::from_le_bytes_mod_order(s))
            .collect();

        UnshieldCircuit::new(
            merkle_root_fr,
            nullifier_fr,
            amount,
            Fr::from(999u64), // recipient hash
            amount,
            blinding,
            serial,
            spending_key,
            merkle_path,
            proof.path_bits,
        )
    }

    #[test]
    fn test_unshield_circuit_satisfies() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let circuit = valid_unshield(1_000_000_000);
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(
            cs.is_satisfied().unwrap(),
            "valid unshield circuit not satisfied"
        );
        let num = cs.num_constraints();
        println!("Unshield circuit constraints: {}", num);
        assert!(num > 1000, "expected >1000 constraints, got {}", num);
    }

    #[test]
    fn test_unshield_wrong_amount_fails() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let blinding = Fr::rand(&mut OsRng);
        let serial = Fr::rand(&mut OsRng);
        let spending_key = Fr::rand(&mut OsRng);

        let real_value = 1000u64;
        let commitment_fr = poseidon_hash_fr(Fr::from(real_value), blinding);
        let nullifier_fr = poseidon_hash_fr(serial, spending_key);

        let mut tree = MerkleTree::new();
        tree.insert(fr_to_bytes(&commitment_fr));
        let merkle_root_fr = Fr::from_le_bytes_mod_order(&tree.root());
        let proof = tree.proof(0).unwrap();
        let merkle_path: Vec<Fr> = proof.siblings.iter().map(|s| Fr::from_le_bytes_mod_order(s)).collect();

        // Claim amount=2000 but note value=1000 → fails value==amount
        let circuit = UnshieldCircuit::new(
            merkle_root_fr,
            nullifier_fr,
            2000, // wrong amount
            Fr::from(999u64),
            real_value,
            blinding,
            serial,
            spending_key,
            merkle_path,
            proof.path_bits,
        );
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(!cs.is_satisfied().unwrap());
    }

    #[test]
    fn test_unshield_wrong_nullifier_fails() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let blinding = Fr::rand(&mut OsRng);
        let serial = Fr::rand(&mut OsRng);
        let spending_key = Fr::rand(&mut OsRng);
        let amount = 1000u64;

        let commitment_fr = poseidon_hash_fr(Fr::from(amount), blinding);
        let wrong_nullifier = Fr::from(12345u64); // doesn't match Poseidon(serial, sk)

        let mut tree = MerkleTree::new();
        tree.insert(fr_to_bytes(&commitment_fr));
        let merkle_root_fr = Fr::from_le_bytes_mod_order(&tree.root());
        let proof = tree.proof(0).unwrap();
        let merkle_path: Vec<Fr> = proof.siblings.iter().map(|s| Fr::from_le_bytes_mod_order(s)).collect();

        let circuit = UnshieldCircuit::new(
            merkle_root_fr,
            wrong_nullifier,
            amount,
            Fr::from(999u64),
            amount,
            blinding,
            serial,
            spending_key,
            merkle_path,
            proof.path_bits,
        );
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(!cs.is_satisfied().unwrap());
    }

    #[test]
    fn test_unshield_wrong_merkle_root_fails() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let blinding = Fr::rand(&mut OsRng);
        let serial = Fr::rand(&mut OsRng);
        let spending_key = Fr::rand(&mut OsRng);
        let amount = 1000u64;

        let commitment_fr = poseidon_hash_fr(Fr::from(amount), blinding);
        let nullifier_fr = poseidon_hash_fr(serial, spending_key);

        let mut tree = MerkleTree::new();
        tree.insert(fr_to_bytes(&commitment_fr));
        let proof = tree.proof(0).unwrap();
        let merkle_path: Vec<Fr> = proof.siblings.iter().map(|s| Fr::from_le_bytes_mod_order(s)).collect();

        let wrong_root = Fr::from(99999u64); // wrong root

        let circuit = UnshieldCircuit::new(
            wrong_root,
            nullifier_fr,
            amount,
            Fr::from(999u64),
            amount,
            blinding,
            serial,
            spending_key,
            merkle_path,
            proof.path_bits,
        );
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(!cs.is_satisfied().unwrap());
    }

    #[test]
    fn test_unshield_wrong_spending_key_fails() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let blinding = Fr::rand(&mut OsRng);
        let serial = Fr::rand(&mut OsRng);
        let spending_key = Fr::rand(&mut OsRng);
        let wrong_sk = Fr::rand(&mut OsRng);
        let amount = 1000u64;

        let commitment_fr = poseidon_hash_fr(Fr::from(amount), blinding);
        // Nullifier was computed with the real spending_key
        let nullifier_fr = poseidon_hash_fr(serial, spending_key);

        let mut tree = MerkleTree::new();
        tree.insert(fr_to_bytes(&commitment_fr));
        let merkle_root_fr = Fr::from_le_bytes_mod_order(&tree.root());
        let proof = tree.proof(0).unwrap();
        let merkle_path: Vec<Fr> = proof.siblings.iter().map(|s| Fr::from_le_bytes_mod_order(s)).collect();

        // Use wrong_sk as witness → nullifier constraint will fail
        let circuit = UnshieldCircuit::new(
            merkle_root_fr,
            nullifier_fr,
            amount,
            Fr::from(999u64),
            amount,
            blinding,
            serial,
            wrong_sk,
            merkle_path,
            proof.path_bits,
        );
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(!cs.is_satisfied().unwrap());
    }

    #[test]
    fn test_unshield_empty_for_setup() {
        use ark_relations::r1cs::{ConstraintSystem, OptimizationGoal};
        let cs = ConstraintSystem::<Fr>::new_ref();
        cs.set_optimization_goal(OptimizationGoal::Constraints);
        cs.set_mode(ark_relations::r1cs::SynthesisMode::Setup);
        let circuit = UnshieldCircuit::empty();
        let result = circuit.generate_constraints(cs.clone());
        assert!(result.is_ok(), "empty unshield circuit failed in setup: {:?}", result.err());
    }

    #[test]
    fn test_unshield_multiple_leaves() {
        // Insert multiple notes, prove for the 3rd one (index 2)
        let cs = ConstraintSystem::<Fr>::new_ref();
        let blinding = Fr::rand(&mut OsRng);
        let serial = Fr::rand(&mut OsRng);
        let spending_key = Fr::rand(&mut OsRng);
        let amount = 500u64;

        let commitment_fr = poseidon_hash_fr(Fr::from(amount), blinding);
        let nullifier_fr = poseidon_hash_fr(serial, spending_key);

        let mut tree = MerkleTree::new();
        // Insert 2 dummy leaves before ours
        tree.insert(fr_to_bytes(&Fr::from(111u64)));
        tree.insert(fr_to_bytes(&Fr::from(222u64)));
        // Our leaf at index 2
        tree.insert(fr_to_bytes(&commitment_fr));

        let merkle_root_fr = Fr::from_le_bytes_mod_order(&tree.root());
        let proof = tree.proof(2).unwrap();
        let merkle_path: Vec<Fr> = proof.siblings.iter().map(|s| Fr::from_le_bytes_mod_order(s)).collect();

        let circuit = UnshieldCircuit::new(
            merkle_root_fr,
            nullifier_fr,
            amount,
            Fr::from(999u64),
            amount,
            blinding,
            serial,
            spending_key,
            merkle_path,
            proof.path_bits,
        );
        circuit.generate_constraints(cs.clone()).unwrap();
        assert!(cs.is_satisfied().unwrap(), "unshield with multiple leaves failed");
    }

    #[test]
    fn test_unshield_constraint_count() {
        let cs = ConstraintSystem::<Fr>::new_ref();
        let circuit = valid_unshield(1000);
        circuit.generate_constraints(cs.clone()).unwrap();
        let num = cs.num_constraints();
        // 32 Poseidon hashes (Merkle) + 2 Poseidon (nullifier, commitment)
        // + range check + equality constraints
        // Each Poseidon ≈ 300 constraints, 34 × 300 = ~10200
        // + 32 conditional selects ≈ 64 constraints
        // + bit decomposition ≈ 254
        // Total ≈ 10500-12000
        assert!(
            num >= 5000 && num <= 20000,
            "constraint count {} outside expected range [5000, 20000]",
            num
        );
        println!("Unshield circuit constraint count: {}", num);
    }
}
