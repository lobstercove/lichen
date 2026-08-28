//! ZK runtime artifact metadata.
//!
//! Legacy scheme 0x01 uses native Plonky3 STARK envelopes and has no external
//! proving-key or verification-key ceremony. The scheme is disabled because it
//! does not constrain private witnesses. This module retains compatibility
//! metadata only; it does not report activation.

use super::{ProofType, ZkSchemeVersion};

/// Runtime artifact metadata for one shielded circuit.
#[derive(Clone)]
pub struct CeremonyOutput {
    /// Circuit name for identification
    pub circuit_name: String,
    /// Which shielded circuit this metadata belongs to
    pub proof_type: ProofType,
    /// Which legacy proof scheme the fixture describes
    pub zk_scheme_version: ZkSchemeVersion,
    /// Human-readable compatibility status
    pub note: String,
}

fn runtime_artifact(proof_type: ProofType) -> CeremonyOutput {
    CeremonyOutput {
        circuit_name: proof_type.as_str().to_string(),
        proof_type,
        zk_scheme_version: ZkSchemeVersion::Plonky3FriPoseidon2,
        note: "Legacy scheme 0x01 fixture metadata; proof acceptance is disabled".to_string(),
    }
}

/// Return legacy compatibility metadata for the shield circuit.
pub fn setup_shield() -> Result<CeremonyOutput, String> {
    Ok(runtime_artifact(ProofType::Shield))
}

/// Return legacy compatibility metadata for the unshield circuit.
pub fn setup_unshield() -> Result<CeremonyOutput, String> {
    Ok(runtime_artifact(ProofType::Unshield))
}

/// Return legacy compatibility metadata for the transfer circuit.
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
