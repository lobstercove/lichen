//! Native value commitment surface.
//!
//! The live runtime represents commitments as the canonical 32-byte Poseidon2
//! digest of `(value, blinding)`.

use super::merkle::{commitment_hash, random_scalar_bytes};
use serde::{Deserialize, Serialize};

/// Canonical 32-byte value commitment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ValueCommitment {
    bytes: [u8; 32],
}

/// Opening of a value commitment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommitmentSecret {
    pub value: u64,
    pub blinding: [u8; 32],
}

impl ValueCommitment {
    pub fn commit(value: u64, blinding: [u8; 32]) -> Self {
        Self {
            bytes: commitment_hash(value, &blinding),
        }
    }

    pub fn commit_random(value: u64) -> (Self, CommitmentSecret) {
        let blinding = random_scalar_bytes();
        let commitment = Self::commit(value, blinding);
        let opening = CommitmentSecret { value, blinding };
        (commitment, opening)
    }

    pub fn verify(&self, opening: &CommitmentSecret) -> bool {
        *self == Self::commit(opening.value, opening.blinding)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.bytes
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.bytes
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != 32 {
            return Err(format!(
                "invalid commitment length: expected 32 bytes, got {}",
                bytes.len()
            ));
        }

        let mut output = [0u8; 32];
        output.copy_from_slice(bytes);
        Ok(Self { bytes: output })
    }

    pub fn to_hash(&self) -> [u8; 32] {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_commit_and_verify() {
        let value = 1_000_000_000u64;
        let (commitment, opening) = ValueCommitment::commit_random(value);
        assert!(commitment.verify(&opening));
    }

    #[test]
    fn test_different_value_fails() {
        let (commitment, mut opening) = ValueCommitment::commit_random(1000);
        opening.value = 2000;
        assert!(!commitment.verify(&opening));
    }

    #[test]
    fn test_different_blinding_fails() {
        let (commitment, mut opening) = ValueCommitment::commit_random(1000);
        opening.blinding = random_scalar_bytes();
        assert!(!commitment.verify(&opening));
    }

    #[test]
    fn test_serialization_roundtrip() {
        let (commitment, _) = ValueCommitment::commit_random(42);
        let bytes = commitment.to_bytes();
        let restored = ValueCommitment::from_bytes(&bytes).unwrap();
        assert_eq!(commitment, restored);
    }

    #[test]
    fn test_deterministic_commitment() {
        let blinding = [0x39u8; 32];
        let c1 = ValueCommitment::commit(1000, blinding);
        let c2 = ValueCommitment::commit(1000, blinding);
        assert_eq!(c1, c2);
    }
}
