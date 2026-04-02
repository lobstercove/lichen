//! Shared gadgets used by the private shielded witness-adapter circuits.

use ark_bn254::Fr;
use ark_crypto_primitives::sponge::constraints::CryptographicSpongeVar;
use ark_crypto_primitives::sponge::poseidon::constraints::PoseidonSpongeVar;
use ark_crypto_primitives::sponge::poseidon::PoseidonConfig;
use ark_r1cs_std::fields::fp::FpVar;
use ark_relations::r1cs::{ConstraintSystemRef, SynthesisError};

/// Compute Poseidon(left, right) in-circuit using the given sponge config.
///
/// This mirrors the private two-input Poseidon helper used by the internal
/// witness-adapter circuits.
pub fn poseidon_hash_var(
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
