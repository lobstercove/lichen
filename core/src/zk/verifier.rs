//! Validator-Side Proof Verification
//!
//! Takes a proof + canonical public inputs and returns true/false.
//! Must be deterministic across all validators. Proof scheme `0x01` remains
//! decodable for compatibility, but is deliberately not accepted: its AIRs do
//! not constrain the private witnesses required for custody operations.

use super::{ShieldedError, ZkProof, ZkSchemeVersion};

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

        Err(ShieldedError::DisabledProofScheme {
            scheme: proof.zk_scheme_version,
            proof_type: proof.proof_type.clone(),
        })
    }
}

impl Default for Verifier {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::serialize_legacy_bincode;
    use crate::zk::{
        build_constant_trace, build_stark_config, ConstantTraceAir, ProofType,
        UnshieldAirPublicValues, ZkProof,
    };
    use p3_uni_stark::{prove as prove_stark, verify as verify_stark};

    #[test]
    fn public_input_only_unshield_proof_reproduces_old_acceptance_but_is_now_disabled() {
        // This is intentionally a local defensive regression. It constructs
        // only public values and the old constant trace; no note, Merkle path,
        // serial, spending key, value, or blinding witness is supplied.
        let public_values =
            UnshieldAirPublicValues::new([0x11; 32], [0x22; 32], 1_000_000_000, [0x33; 32]);
        let air = ConstantTraceAir::new(public_values.as_fields());
        let trace = build_constant_trace(air.public_values());
        let config = build_stark_config();
        let legacy_stark_proof = prove_stark(&config, &air, trace, &[]);

        verify_stark(&config, &air, &legacy_stark_proof, &[])
            .expect("the old public-input-only verifier path accepted this proof");

        let proof = ZkProof::plonky3(
            ProofType::Unshield,
            serialize_legacy_bincode(&legacy_stark_proof, "defensive regression proof")
                .expect("serialize defensive regression proof"),
            public_values.to_stark_public_inputs().into_iter().collect(),
        );

        assert_eq!(
            Verifier::new().verify(&proof),
            Err(ShieldedError::DisabledProofScheme {
                scheme: ZkSchemeVersion::Plonky3FriPoseidon2,
                proof_type: ProofType::Unshield,
            })
        );
    }
}
