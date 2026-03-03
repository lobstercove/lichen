//! Shielded Key Derivation
//!
//! Generates shielded keypairs for privacy transactions.
//!
//! spending_key: random scalar (Fr), used to compute nullifiers
//! viewing_key:  spending_key * G (public point), used for note encryption/decryption
//!
//! The spending key is the master secret. The viewing key can be shared
//! with auditors for compliance (selective disclosure).

use super::merkle::fr_to_bytes;
use ark_bn254::{Fr, G1Affine, G1Projective};
use ark_ec::{AffineRepr, CurveGroup};
use ark_ff::{PrimeField, UniformRand};
use ark_serialize::CanonicalSerialize;
use ark_std::rand::rngs::OsRng;
use sha2::{Digest, Sha256};

/// Spending key: secret scalar used to derive nullifiers
#[derive(Clone, Debug)]
pub struct SpendingKey(pub Fr);

/// Viewing key: public point used for note encryption
#[derive(Clone, Debug)]
pub struct ViewingKey(pub G1Affine);

/// Complete shielded keypair
#[derive(Clone, Debug)]
pub struct ShieldedKeypair {
    /// Secret spending key (never shared)
    pub spending_key: SpendingKey,
    /// Public viewing key (can be shared with auditors)
    pub viewing_key: ViewingKey,
}

impl ShieldedKeypair {
    /// Generate a new random shielded keypair
    pub fn generate() -> Self {
        let spending_key = SpendingKey(Fr::rand(&mut OsRng));
        let viewing_key = spending_key.derive_viewing_key();
        Self {
            spending_key,
            viewing_key,
        }
    }

    /// Derive a shielded keypair from an existing seed (e.g., wallet seed phrase)
    /// Uses HKDF-like derivation: spending_key = SHA256(seed || "moltchain-shielded-key")
    pub fn from_seed(seed: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(seed);
        hasher.update(b"moltchain-shielded-spending-key-v1");
        let hash = hasher.finalize();

        let spending_key = SpendingKey(Fr::from_le_bytes_mod_order(&hash));
        let viewing_key = spending_key.derive_viewing_key();
        Self {
            spending_key,
            viewing_key,
        }
    }

    /// Get the viewing key as 32 bytes (for note encryption)
    pub fn viewing_key_bytes(&self) -> [u8; 32] {
        self.viewing_key.to_bytes()
    }

    /// Get the spending key as 32 bytes
    pub fn spending_key_bytes(&self) -> [u8; 32] {
        fr_to_bytes(&self.spending_key.0)
    }
}

impl SpendingKey {
    /// Derive the corresponding viewing key: V = sk * G
    pub fn derive_viewing_key(&self) -> ViewingKey {
        let g = G1Affine::generator();
        let point = (G1Projective::from(g) * self.0).into_affine();
        ViewingKey(point)
    }

    /// Restore spending key from 32 bytes
    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        Self(Fr::from_le_bytes_mod_order(bytes))
    }
}

impl ViewingKey {
    /// Serialize viewing key to 32 bytes (hash of compressed point)
    pub fn to_bytes(&self) -> [u8; 32] {
        let mut serialized = Vec::new();
        self.0
            .serialize_compressed(&mut serialized)
            .unwrap_or_else(|e| panic!("FATAL: G1 point serialization failed: {}. OOM or ark-serialize bug.", e));

        let mut hasher = Sha256::new();
        hasher.update(&serialized);
        let hash = hasher.finalize();
        let mut output = [0u8; 32];
        output.copy_from_slice(&hash);
        output
    }

    /// Get the full compressed point bytes
    pub fn to_compressed_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        self.0
            .serialize_compressed(&mut bytes)
            .unwrap_or_else(|e| panic!("FATAL: G1 point serialization failed: {}. OOM or ark-serialize bug.", e));
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ark_std::rand::rngs::OsRng;

    #[test]
    fn test_keypair_generation() {
        let kp1 = ShieldedKeypair::generate();
        let kp2 = ShieldedKeypair::generate();
        // Different keypairs each time
        assert_ne!(kp1.spending_key_bytes(), kp2.spending_key_bytes());
        assert_ne!(kp1.viewing_key_bytes(), kp2.viewing_key_bytes());
    }

    #[test]
    fn test_keypair_from_seed_deterministic() {
        let seed = b"test-wallet-seed-phrase-12-words";
        let kp1 = ShieldedKeypair::from_seed(seed);
        let kp2 = ShieldedKeypair::from_seed(seed);
        assert_eq!(kp1.spending_key_bytes(), kp2.spending_key_bytes());
        assert_eq!(kp1.viewing_key_bytes(), kp2.viewing_key_bytes());
    }

    #[test]
    fn test_different_seeds_different_keys() {
        let kp1 = ShieldedKeypair::from_seed(b"seed-a");
        let kp2 = ShieldedKeypair::from_seed(b"seed-b");
        assert_ne!(kp1.spending_key_bytes(), kp2.spending_key_bytes());
    }

    #[test]
    fn test_viewing_key_derivation() {
        let sk = SpendingKey(Fr::from(42u64));
        let vk1 = sk.derive_viewing_key();
        let vk2 = sk.derive_viewing_key();
        assert_eq!(vk1.to_bytes(), vk2.to_bytes());
    }

    #[test]
    fn test_spending_key_roundtrip() {
        let kp = ShieldedKeypair::generate();
        let bytes = kp.spending_key_bytes();
        let restored = SpendingKey::from_bytes(&bytes);
        assert_eq!(
            restored.derive_viewing_key().to_bytes(),
            kp.viewing_key_bytes()
        );
    }
}
