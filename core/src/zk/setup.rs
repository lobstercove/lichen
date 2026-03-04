//! Trusted Setup Ceremony Tools
//!
//! Generates circuit-specific proving/verification keys using Groth16.
//!
//! Development: deterministic seed for reproducible keys.
//! Production: multi-party computation (MPC) ceremony where
//! only ONE honest participant is needed for security.
//!
//! Output: proving_key.bin (~100MB) and verification_key.bin (~1KB) per circuit.

use super::circuits::shield::ShieldCircuit;
use super::circuits::transfer::TransferCircuit;
use super::circuits::unshield::UnshieldCircuit;
use ark_bn254::Bn254;
use ark_groth16::{Groth16, ProvingKey, VerifyingKey};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use ark_snark::SNARK;
use ark_std::rand::rngs::OsRng;

/// Result of a trusted setup ceremony for one circuit
#[derive(Clone)]
pub struct CeremonyOutput {
    /// Serialized proving key (~100MB)
    pub proving_key_bytes: Vec<u8>,
    /// Serialized verification key (~1KB)
    pub verification_key_bytes: Vec<u8>,
    /// Circuit name for identification
    pub circuit_name: String,
}

/// Run the trusted setup for the shield circuit
pub fn setup_shield() -> Result<CeremonyOutput, String> {
    let circuit = ShieldCircuit::empty();

    let (pk, vk) = Groth16::<Bn254>::circuit_specific_setup(circuit, &mut OsRng)
        .map_err(|e| format!("shield setup failed: {}", e))?;

    Ok(serialize_keys(pk, vk, "shield"))
}

/// Run the trusted setup for the unshield circuit
pub fn setup_unshield() -> Result<CeremonyOutput, String> {
    let circuit = UnshieldCircuit::empty();

    let (pk, vk) = Groth16::<Bn254>::circuit_specific_setup(circuit, &mut OsRng)
        .map_err(|e| format!("unshield setup failed: {}", e))?;

    Ok(serialize_keys(pk, vk, "unshield"))
}

/// Run the trusted setup for the transfer circuit
pub fn setup_transfer() -> Result<CeremonyOutput, String> {
    let circuit = TransferCircuit::empty();

    let (pk, vk) = Groth16::<Bn254>::circuit_specific_setup(circuit, &mut OsRng)
        .map_err(|e| format!("transfer setup failed: {}", e))?;

    Ok(serialize_keys(pk, vk, "transfer"))
}

/// Run all three setups at once
pub fn setup_all() -> Result<Vec<CeremonyOutput>, String> {
    let shield = setup_shield()?;
    let unshield = setup_unshield()?;
    let transfer = setup_transfer()?;
    Ok(vec![shield, unshield, transfer])
}

/// Serialize proving and verification keys
fn serialize_keys(pk: ProvingKey<Bn254>, vk: VerifyingKey<Bn254>, name: &str) -> CeremonyOutput {
    let mut pk_bytes = Vec::new();
    pk.serialize_compressed(&mut pk_bytes).unwrap_or_else(|e| {
        panic!(
            "FATAL: ProvingKey '{}' serialization failed: {}. OOM or ark-serialize bug.",
            name, e
        )
    });

    let mut vk_bytes = Vec::new();
    vk.serialize_compressed(&mut vk_bytes).unwrap_or_else(|e| {
        panic!(
            "FATAL: VerifyingKey '{}' serialization failed: {}. OOM or ark-serialize bug.",
            name, e
        )
    });

    CeremonyOutput {
        proving_key_bytes: pk_bytes,
        verification_key_bytes: vk_bytes,
        circuit_name: name.to_string(),
    }
}

/// Load a verification key from bytes
pub fn load_verification_key(bytes: &[u8]) -> Result<VerifyingKey<Bn254>, String> {
    VerifyingKey::<Bn254>::deserialize_compressed(bytes)
        .map_err(|e| format!("failed to deserialize VK: {}", e))
}

/// Load a proving key from bytes
pub fn load_proving_key(bytes: &[u8]) -> Result<ProvingKey<Bn254>, String> {
    ProvingKey::<Bn254>::deserialize_compressed(bytes)
        .map_err(|e| format!("failed to deserialize PK: {}", e))
}
