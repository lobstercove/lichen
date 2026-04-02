//! Shielded key derivation.
//!
//! The live runtime now derives both spending and viewing keys as canonical
//! 32-byte byte strings.

use super::merkle::{random_scalar_bytes, scalar_bytes_from_seed};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const DOMAIN_SPENDING_KEY: u64 = 0x4c49434e534b5631;
const DOMAIN_VIEWING_KEY: u64 = 0x4c49434e564b5931;

/// Spending key: secret bytes used to derive nullifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SpendingKey(pub [u8; 32]);

/// Viewing key: public 32-byte encryption identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ViewingKey(pub [u8; 32]);

/// Complete shielded keypair.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShieldedKeypair {
    pub spending_key: SpendingKey,
    pub viewing_key: ViewingKey,
}

impl ShieldedKeypair {
    pub fn generate() -> Self {
        let spending_key = SpendingKey(random_scalar_bytes());
        let viewing_key = spending_key.derive_viewing_key();
        Self {
            spending_key,
            viewing_key,
        }
    }

    pub fn from_seed(seed: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(seed);
        hasher.update(b"lichen-shielded-spending-key-v1");
        let hash: [u8; 32] = hasher.finalize().into();

        let spending_key = SpendingKey(scalar_bytes_from_seed(DOMAIN_SPENDING_KEY, hash));
        let viewing_key = spending_key.derive_viewing_key();
        Self {
            spending_key,
            viewing_key,
        }
    }

    pub fn viewing_key_bytes(&self) -> [u8; 32] {
        self.viewing_key.to_bytes()
    }

    pub fn spending_key_bytes(&self) -> [u8; 32] {
        self.spending_key.to_bytes()
    }
}

impl SpendingKey {
    pub fn derive_viewing_key(&self) -> ViewingKey {
        ViewingKey(scalar_bytes_from_seed(DOMAIN_VIEWING_KEY, self.0))
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        Self(*bytes)
    }
}

impl ViewingKey {
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    pub fn to_compressed_bytes(&self) -> Vec<u8> {
        self.0.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keypair_generation() {
        let kp1 = ShieldedKeypair::generate();
        let kp2 = ShieldedKeypair::generate();
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
        let sk = SpendingKey([42u8; 32]);
        let vk1 = sk.derive_viewing_key();
        let vk2 = sk.derive_viewing_key();
        assert_eq!(vk1.to_bytes(), vk2.to_bytes());
    }

    #[test]
    fn test_spending_key_roundtrip() {
        let kp = ShieldedKeypair::generate();
        let bytes = kp.spending_key_bytes();
        let restored = SpendingKey::from_bytes(&bytes);
        assert_eq!(restored.to_bytes(), kp.spending_key_bytes());
    }
}
