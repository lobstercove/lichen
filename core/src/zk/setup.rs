//! ZK runtime artifact metadata.
//!
//! Lichen shielded proofs now use native Plonky3 STARK envelopes and no longer
//! require proving-key or verification-key ceremonies. This module reports the
//! active runtime scheme for each shielded circuit so callers that still expect
//! a setup step can introspect the live configuration without relying on
//! trusted-setup artifacts.

use super::{ProofType, ZkSchemeVersion};

/// Runtime artifact metadata for one shielded circuit.
#[derive(Clone)]
pub struct CeremonyOutput {
    /// Circuit name for identification
    pub circuit_name: String,
    /// Which shielded circuit this metadata belongs to
    pub proof_type: ProofType,
    /// Which proof scheme the runtime expects
    pub zk_scheme_version: ZkSchemeVersion,
    /// Human-readable description of the live runtime
    pub note: String,
}

fn runtime_artifact(proof_type: ProofType) -> CeremonyOutput {
    CeremonyOutput {
        circuit_name: proof_type.as_str().to_string(),
        proof_type,
        zk_scheme_version: ZkSchemeVersion::Plonky3FriPoseidon2,
        note: "Native Plonky3 STARK runtime; no external setup artifacts required".to_string(),
    }
}

/// Return the live runtime metadata for the shield circuit.
pub fn setup_shield() -> Result<CeremonyOutput, String> {
    Ok(runtime_artifact(ProofType::Shield))
}

/// Return the live runtime metadata for the unshield circuit.
pub fn setup_unshield() -> Result<CeremonyOutput, String> {
    Ok(runtime_artifact(ProofType::Unshield))
}

/// Return the live runtime metadata for the transfer circuit.
pub fn setup_transfer() -> Result<CeremonyOutput, String> {
    Ok(runtime_artifact(ProofType::Transfer))
}

/// Return the live runtime metadata for all shielded circuits.
pub fn setup_all() -> Result<Vec<CeremonyOutput>, String> {
    let shield = setup_shield()?;
    let unshield = setup_unshield()?;
    let transfer = setup_transfer()?;
    Ok(vec![shield, unshield, transfer])
}
