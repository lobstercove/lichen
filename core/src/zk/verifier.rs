//! Validator-Side Proof Verification
//!
//! Takes a proof + canonical public inputs and returns true/false.
//! Must be deterministic across all validators. The proof envelope is
//! scheme-versioned; the live verifier backend consumes the native Plonky3
//! STARK envelopes emitted by the prover.

use super::{
    build_stark_config, deserialize_stark_proof, ConstantTraceAir, ProofType, ShieldAir,
    ShieldAirPublicValues, ShieldedError, TransferAirPublicValues, UnshieldAirPublicValues,
    ZkProof, ZkSchemeVersion,
};
use p3_uni_stark::verify as verify_stark;

/// Validator-side proof verifier
pub struct Verifier;

impl Verifier {
    /// Create a verifier with no keys loaded
    pub fn new() -> Self {
        Self
    }

    /// Verify a ZK proof against its public inputs
    pub fn verify(&self, proof: &ZkProof) -> Result<bool, ShieldedError> {
        if proof.zk_scheme_version != ZkSchemeVersion::Plonky3FriPoseidon2 {
            return Err(ShieldedError::UnsupportedProofScheme(
                proof.zk_scheme_version,
            ));
        }

        self.verify_plonky3(proof)
    }

    fn verify_plonky3(&self, proof: &ZkProof) -> Result<bool, ShieldedError> {
        let stark_proof =
            deserialize_stark_proof(&proof.proof_bytes).map_err(ShieldedError::InvalidProof)?;
        let stark_public_inputs = proof.stark_public_inputs()?;
        let config = build_stark_config();

        let verification = match proof.proof_type {
            ProofType::Shield => {
                let public_values =
                    ShieldAirPublicValues::from_stark_public_inputs(stark_public_inputs)
                        .map_err(ShieldedError::InvalidProof)?;
                let air = ShieldAir::new(public_values);
                verify_stark(&config, &air, &stark_proof, &[])
            }
            ProofType::Unshield => {
                let public_values =
                    UnshieldAirPublicValues::from_stark_public_inputs(stark_public_inputs)
                        .map_err(ShieldedError::InvalidProof)?;
                let air = ConstantTraceAir::new(public_values.as_fields());
                verify_stark(&config, &air, &stark_proof, &[])
            }
            ProofType::Transfer => {
                let public_values =
                    TransferAirPublicValues::from_stark_public_inputs(stark_public_inputs)
                        .map_err(ShieldedError::InvalidProof)?;
                let air = ConstantTraceAir::new(public_values.as_fields());
                verify_stark(&config, &air, &stark_proof, &[])
            }
        };

        Ok(verification.is_ok())
    }
}

impl Default for Verifier {
    fn default() -> Self {
        Self::new()
    }
}
