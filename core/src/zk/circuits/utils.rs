//! Shared circuit gadgets used by shield, unshield, and transfer circuits.

use ark_bn254::Fr;
use ark_crypto_primitives::sponge::constraints::CryptographicSpongeVar;
use ark_crypto_primitives::sponge::poseidon::constraints::PoseidonSpongeVar;
use ark_crypto_primitives::sponge::poseidon::PoseidonConfig;
use ark_r1cs_std::fields::fp::FpVar;
use ark_relations::r1cs::{ConstraintSystemRef, SynthesisError};

/// Compute Poseidon(left, right) in-circuit using the given sponge config.
///
/// Mirrors the native `poseidon_hash_fr(left, right)` exactly: absorb left,
/// absorb right, squeeze 1 field element. Uses the same `poseidon_config()`
/// so that in-circuit and native hashes always match.
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
