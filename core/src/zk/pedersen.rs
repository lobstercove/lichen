//! Pedersen Commitment Scheme over BN254
//!
//! Commitment = value * G + blinding * H
//! where G and H are independent generators on the BN254 curve.
//!
//! Properties:
//! - Hiding: commitment reveals nothing about value (blinding is random)
//! - Binding: cannot open to a different (value, blinding) pair
//! - Homomorphic: Commit(a, r1) + Commit(b, r2) = Commit(a+b, r1+r2)

use ark_bn254::{Fr, G1Affine, G1Projective};
use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::{PrimeField, UniformRand};
use ark_serialize::{CanonicalDeserialize, CanonicalSerialize};
use ark_std::rand::rngs::OsRng;
use sha2::{Digest, Sha256};

/// Fixed generator G for value component (hash-to-curve of "MoltChain-Pedersen-G")
/// Uses deterministic try-and-increment with domain-separated hash, matching H.
fn generator_g() -> G1Affine {
    // AUDIT-FIX CORE-02: Use hash-to-curve instead of standard generator.
    // Domain separation ensures G and H are provably independent.
    let mut hasher = Sha256::new();
    hasher.update(b"MoltChain-Pedersen-G-generator-v1");
    let hash = hasher.finalize();

    let mut seed = [0u8; 32];
    seed.copy_from_slice(&hash);

    loop {
        let mut attempt_hasher = Sha256::new();
        attempt_hasher.update(seed);
        let attempt = attempt_hasher.finalize();

        if let Some(point) = try_decode_point(&attempt) {
            if !point.is_zero() {
                return point;
            }
        }
        for byte in seed.iter_mut().rev() {
            *byte = byte.wrapping_add(1);
            if *byte != 0 { break; }
        }
    }
}

/// Fixed generator H for blinding component (hash-to-curve of "MoltChain-Pedersen-H")
/// Must be independent of G (no known discrete log relationship)
fn generator_h() -> G1Affine {
    // Hash "MoltChain-Pedersen-H" to get a deterministic but independent generator
    let mut hasher = Sha256::new();
    hasher.update(b"MoltChain-Pedersen-H-generator-v1");
    let hash = hasher.finalize();

    // Use hash as x-coordinate seed, find valid point via try-and-increment
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&hash);

    // Simple deterministic point generation: hash repeatedly until valid
    loop {
        let mut attempt_hasher = Sha256::new();
        attempt_hasher.update(seed);
        let attempt = attempt_hasher.finalize();

        if let Some(point) = try_decode_point(&attempt) {
            // Ensure it's not the identity and is in the correct subgroup
            if !point.is_zero() {
                return point;
            }
        }
        // Increment seed
        for byte in seed.iter_mut().rev() {
            *byte = byte.wrapping_add(1);
            if *byte != 0 {
                break;
            }
        }
    }
}

/// Try to decode a hash as a compressed G1 point
fn try_decode_point(hash: &[u8]) -> Option<G1Affine> {
    // Attempt to deserialize as a valid curve point
    // This is a simplified version; production should use a proper hash-to-curve
    use ark_bn254::Fq;
    let x = Fq::from_le_bytes_mod_order(hash);
    G1Affine::get_point_from_x_unchecked(x, false)
}

/// A Pedersen commitment: C = value * G + blinding * H
#[derive(Clone, Debug)]
pub struct PedersenCommitment {
    /// The commitment point on BN254 G1
    pub point: G1Affine,
}

/// Opening of a Pedersen commitment (private data)
#[derive(Clone, Debug)]
pub struct CommitmentOpening {
    /// The committed value (in shells)
    pub value: u64,
    /// The blinding factor (random scalar)
    pub blinding: Fr,
}

impl PedersenCommitment {
    /// Create a new commitment: C = value * G + blinding * H
    pub fn commit(value: u64, blinding: Fr) -> Self {
        let g = generator_g();
        let h = generator_h();

        let value_scalar = Fr::from(value);
        let point =
            (G1Projective::from(g) * value_scalar + G1Projective::from(h) * blinding).into_affine();

        Self { point }
    }

    /// Create a commitment with a random blinding factor
    pub fn commit_random(value: u64) -> (Self, CommitmentOpening) {
        let blinding = Fr::rand(&mut OsRng);
        let commitment = Self::commit(value, blinding);
        let opening = CommitmentOpening { value, blinding };
        (commitment, opening)
    }

    /// Verify that an opening matches this commitment
    pub fn verify(&self, opening: &CommitmentOpening) -> bool {
        let expected = Self::commit(opening.value, opening.blinding);
        self.point == expected.point
    }

    /// Serialize commitment to 32 bytes (compressed G1 point)
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        self.point
            .serialize_compressed(&mut bytes)
            .expect("G1 serialization should not fail");
        bytes
    }

    /// Deserialize commitment from compressed bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let point = G1Affine::deserialize_compressed(bytes)
            .map_err(|e| format!("invalid commitment bytes: {}", e))?;
        Ok(Self { point })
    }

    /// Get a fixed-size 32-byte hash of the commitment (for Merkle tree leaves)
    pub fn to_hash(&self) -> [u8; 32] {
        let bytes = self.to_bytes();
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let result = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result);
        hash
    }

    /// Homomorphic addition: Commit(a, r1) + Commit(b, r2) = Commit(a+b, r1+r2)
    pub fn add(&self, other: &PedersenCommitment) -> PedersenCommitment {
        let point =
            (G1Projective::from(self.point) + G1Projective::from(other.point)).into_affine();
        PedersenCommitment { point }
    }
}

/// Add two commitment openings (for verifying homomorphic property)
impl CommitmentOpening {
    pub fn add(&self, other: &CommitmentOpening) -> CommitmentOpening {
        CommitmentOpening {
            value: self.value + other.value,
            blinding: self.blinding + other.blinding,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_std::rand::rngs::OsRng;

    #[test]
    fn test_commit_and_verify() {
        let value = 1_000_000_000u64; // 1 MOLT in shells
        let (commitment, opening) = PedersenCommitment::commit_random(value);
        assert!(commitment.verify(&opening));
    }

    #[test]
    fn test_different_value_fails() {
        let (commitment, mut opening) = PedersenCommitment::commit_random(1000);
        opening.value = 2000; // tamper with value
        assert!(!commitment.verify(&opening));
    }

    #[test]
    fn test_different_blinding_fails() {
        let (commitment, mut opening) = PedersenCommitment::commit_random(1000);
        opening.blinding = Fr::rand(&mut OsRng); // tamper with blinding
        assert!(!commitment.verify(&opening));
    }

    #[test]
    fn test_serialization_roundtrip() {
        let (commitment, _) = PedersenCommitment::commit_random(42);
        let bytes = commitment.to_bytes();
        let restored = PedersenCommitment::from_bytes(&bytes).unwrap();
        assert_eq!(commitment.point, restored.point);
    }

    #[test]
    fn test_homomorphic_addition() {
        let (c1, o1) = PedersenCommitment::commit_random(100);
        let (c2, o2) = PedersenCommitment::commit_random(200);
        let c_sum = c1.add(&c2);
        let o_sum = o1.add(&o2);
        assert!(c_sum.verify(&o_sum));
        assert_eq!(o_sum.value, 300);
    }

    #[test]
    fn test_deterministic_commitment() {
        let blinding = Fr::from(12345u64);
        let c1 = PedersenCommitment::commit(1000, blinding);
        let c2 = PedersenCommitment::commit(1000, blinding);
        assert_eq!(c1.point, c2.point);
    }
}
