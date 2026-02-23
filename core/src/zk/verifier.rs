//! Validator-Side Proof Verification
//!
//! Takes a proof + public inputs + verification key and returns true/false.
//! Verification is ~3ms per proof (BN254 pairing check).
//! Must be deterministic across all validators.

use super::prover::deserialize_proof;
use super::{ProofType, ShieldedError, ZkProof};
use ark_bn254::{Bn254, Fr};
use ark_ff::PrimeField;
use ark_groth16::{Groth16, PreparedVerifyingKey, VerifyingKey};
use ark_serialize::CanonicalDeserialize;
use ark_snark::SNARK;

/// Validator-side proof verifier
pub struct Verifier {
    /// Prepared verification key for shield proofs
    pub pvk_shield: Option<PreparedVerifyingKey<Bn254>>,
    /// Prepared verification key for unshield proofs
    pub pvk_unshield: Option<PreparedVerifyingKey<Bn254>>,
    /// Prepared verification key for transfer proofs
    pub pvk_transfer: Option<PreparedVerifyingKey<Bn254>>,
}

impl Verifier {
    /// Create a verifier with no keys loaded
    pub fn new() -> Self {
        Self {
            pvk_shield: None,
            pvk_unshield: None,
            pvk_transfer: None,
        }
    }

    /// Create a verifier pre-loaded with the shield verification key
    pub fn from_vk_shield(vk: VerifyingKey<Bn254>) -> Self {
        Self {
            pvk_shield: Groth16::<Bn254>::process_vk(&vk).ok(),
            pvk_unshield: None,
            pvk_transfer: None,
        }
    }

    /// Create a verifier pre-loaded with the unshield verification key
    pub fn from_vk_unshield(vk: VerifyingKey<Bn254>) -> Self {
        Self {
            pvk_shield: None,
            pvk_unshield: Groth16::<Bn254>::process_vk(&vk).ok(),
            pvk_transfer: None,
        }
    }

    /// Create a verifier pre-loaded with the transfer verification key
    pub fn from_vk_transfer(vk: VerifyingKey<Bn254>) -> Self {
        Self {
            pvk_shield: None,
            pvk_unshield: None,
            pvk_transfer: Groth16::<Bn254>::process_vk(&vk).ok(),
        }
    }

    /// Load shield verification key from bytes
    pub fn load_shield_vk(&mut self, bytes: &[u8]) -> Result<(), String> {
        let vk = VerifyingKey::<Bn254>::deserialize_compressed(bytes)
            .map_err(|e| format!("failed to load shield VK: {}", e))?;
        self.pvk_shield = Some(
            Groth16::<Bn254>::process_vk(&vk)
                .map_err(|e| format!("failed to process shield VK: {}", e))?,
        );
        Ok(())
    }

    /// Load unshield verification key from bytes
    pub fn load_unshield_vk(&mut self, bytes: &[u8]) -> Result<(), String> {
        let vk = VerifyingKey::<Bn254>::deserialize_compressed(bytes)
            .map_err(|e| format!("failed to load unshield VK: {}", e))?;
        self.pvk_unshield = Some(
            Groth16::<Bn254>::process_vk(&vk)
                .map_err(|e| format!("failed to process unshield VK: {}", e))?,
        );
        Ok(())
    }

    /// Load transfer verification key from bytes
    pub fn load_transfer_vk(&mut self, bytes: &[u8]) -> Result<(), String> {
        let vk = VerifyingKey::<Bn254>::deserialize_compressed(bytes)
            .map_err(|e| format!("failed to load transfer VK: {}", e))?;
        self.pvk_transfer = Some(
            Groth16::<Bn254>::process_vk(&vk)
                .map_err(|e| format!("failed to process transfer VK: {}", e))?,
        );
        Ok(())
    }

    /// Verify a ZK proof against its public inputs
    pub fn verify(&self, proof: &ZkProof) -> Result<bool, ShieldedError> {
        let pvk = match proof.proof_type {
            ProofType::Shield => self.pvk_shield.as_ref(),
            ProofType::Unshield => self.pvk_unshield.as_ref(),
            ProofType::Transfer => self.pvk_transfer.as_ref(),
        }
        .ok_or(ShieldedError::VerificationKeyMissing(
            proof.proof_type.clone(),
        ))?;

        // Deserialize the proof
        let groth16_proof = deserialize_proof(&proof.proof_bytes)
            .map_err(|e| ShieldedError::InvalidProof(e))?;

        // Convert public inputs from bytes to field elements
        let public_inputs: Vec<Fr> = proof
            .public_inputs
            .iter()
            .map(|bytes| Fr::from_le_bytes_mod_order(bytes))
            .collect();

        // Verify the proof
        let valid = Groth16::<Bn254>::verify_with_processed_vk(pvk, &public_inputs, &groth16_proof)
            .map_err(|e| ShieldedError::InvalidProof(format!("verification error: {}", e)))?;

        Ok(valid)
    }
}

impl Default for Verifier {
    fn default() -> Self {
        Self::new()
    }
}

/// Hash a verification key to a 32-byte digest (for on-chain storage)
pub fn hash_verification_key(vk_bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(vk_bytes);
    let result = hasher.finalize();
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&result);
    hash
}
