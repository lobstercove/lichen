//! Client-Side Proof Generation
//!
//! The prover runs on the user's machine (wallet). Private data
//! never leaves the client. The prover takes private witnesses +
//! public inputs and produces a 128-byte Groth16 proof.
//!
//! Proving time targets:
//! - Shield: <1 second
//! - Unshield: <3 seconds
//! - Transfer (2-in-2-out): <5 seconds

use super::circuits::shield::ShieldCircuit;
use super::circuits::transfer::TransferCircuit;
use super::circuits::unshield::UnshieldCircuit;
use super::{ProofType, ZkProof};
use ark_bn254::Bn254;
use ark_groth16::{Groth16, Proof, ProvingKey};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use ark_snark::SNARK;
use ark_std::rand::rngs::OsRng;

/// Client-side ZK prover
pub struct Prover {
    /// Proving key for shield circuit
    pub pk_shield: Option<ProvingKey<Bn254>>,
    /// Proving key for unshield circuit
    pub pk_unshield: Option<ProvingKey<Bn254>>,
    /// Proving key for transfer circuit
    pub pk_transfer: Option<ProvingKey<Bn254>>,
}

impl Prover {
    /// Create a prover with no keys loaded (call load_* to add keys)
    pub fn new() -> Self {
        Self {
            pk_shield: None,
            pk_unshield: None,
            pk_transfer: None,
        }
    }

    /// Load proving key from bytes
    pub fn load_shield_key(&mut self, bytes: &[u8]) -> Result<(), String> {
        let pk = ProvingKey::<Bn254>::deserialize_compressed(bytes)
            .map_err(|e| format!("failed to load shield proving key: {}", e))?;
        self.pk_shield = Some(pk);
        Ok(())
    }

    /// Load unshield proving key from bytes
    pub fn load_unshield_key(&mut self, bytes: &[u8]) -> Result<(), String> {
        let pk = ProvingKey::<Bn254>::deserialize_compressed(bytes)
            .map_err(|e| format!("failed to load unshield proving key: {}", e))?;
        self.pk_unshield = Some(pk);
        Ok(())
    }

    /// Load transfer proving key from bytes
    pub fn load_transfer_key(&mut self, bytes: &[u8]) -> Result<(), String> {
        let pk = ProvingKey::<Bn254>::deserialize_compressed(bytes)
            .map_err(|e| format!("failed to load transfer proving key: {}", e))?;
        self.pk_transfer = Some(pk);
        Ok(())
    }

    /// Generate a shield proof
    pub fn prove_shield(&self, circuit: ShieldCircuit) -> Result<ZkProof, String> {
        let pk = self
            .pk_shield
            .as_ref()
            .ok_or("shield proving key not loaded")?;

        let proof = Groth16::<Bn254>::prove(pk, circuit, &mut OsRng)
            .map_err(|e| format!("shield proof generation failed: {}", e))?;

        Ok(serialize_proof(proof, ProofType::Shield))
    }

    /// Generate an unshield proof
    pub fn prove_unshield(&self, circuit: UnshieldCircuit) -> Result<ZkProof, String> {
        let pk = self
            .pk_unshield
            .as_ref()
            .ok_or("unshield proving key not loaded")?;

        let proof = Groth16::<Bn254>::prove(pk, circuit, &mut OsRng)
            .map_err(|e| format!("unshield proof generation failed: {}", e))?;

        Ok(serialize_proof(proof, ProofType::Unshield))
    }

    /// Generate a transfer proof
    pub fn prove_transfer(&self, circuit: TransferCircuit) -> Result<ZkProof, String> {
        let pk = self
            .pk_transfer
            .as_ref()
            .ok_or("transfer proving key not loaded")?;

        let proof = Groth16::<Bn254>::prove(pk, circuit, &mut OsRng)
            .map_err(|e| format!("transfer proof generation failed: {}", e))?;

        Ok(serialize_proof(proof, ProofType::Transfer))
    }
}

impl Default for Prover {
    fn default() -> Self {
        Self::new()
    }
}

/// Serialize a Groth16 proof into our ZkProof format
fn serialize_proof(proof: Proof<Bn254>, proof_type: ProofType) -> ZkProof {
    let mut proof_bytes = Vec::new();
    proof
        .serialize_compressed(&mut proof_bytes)
        .expect("proof serialization should not fail");

    ZkProof {
        proof_bytes,
        proof_type,
        public_inputs: Vec::new(), // caller fills these in
    }
}

/// Deserialize proof bytes back to a Groth16 proof
pub fn deserialize_proof(bytes: &[u8]) -> Result<Proof<Bn254>, String> {
    Proof::<Bn254>::deserialize_compressed(bytes).map_err(|e| format!("invalid proof bytes: {}", e))
}
