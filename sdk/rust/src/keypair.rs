//! Keypair and public key management

pub use lichen_core::{Keypair as CoreKeypair, Pubkey};

/// Keypair wrapper with SDK convenience methods
pub struct Keypair(CoreKeypair);

impl Keypair {
    /// Generate a new random keypair
    pub fn new() -> Self {
        Self(CoreKeypair::new())
    }
    
    /// Create keypair from seed bytes
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self(CoreKeypair::from_seed(seed))
    }
    
    /// Get public key
    pub fn pubkey(&self) -> Pubkey {
        self.0.pubkey()
    }
    
    /// Get seed for saving
    pub fn to_seed(&self) -> [u8; 32] {
        self.0.to_seed()
    }
    
    /// Sign a message
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.0.sign(message)
    }
    
    /// Get reference to inner keypair
    pub fn inner(&self) -> &CoreKeypair {
        &self.0
    }
}

impl Default for Keypair {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_keypair_has_unique_pubkey() {
        let kp1 = Keypair::new();
        let kp2 = Keypair::new();
        assert_ne!(kp1.pubkey(), kp2.pubkey());
    }

    #[test]
    fn from_seed_deterministic() {
        let seed = [42u8; 32];
        let kp1 = Keypair::from_seed(&seed);
        let kp2 = Keypair::from_seed(&seed);
        assert_eq!(kp1.pubkey(), kp2.pubkey());
    }

    #[test]
    fn different_seeds_different_keys() {
        let kp1 = Keypair::from_seed(&[1u8; 32]);
        let kp2 = Keypair::from_seed(&[2u8; 32]);
        assert_ne!(kp1.pubkey(), kp2.pubkey());
    }

    #[test]
    fn to_seed_roundtrip() {
        let seed = [99u8; 32];
        let kp = Keypair::from_seed(&seed);
        assert_eq!(kp.to_seed(), seed);
    }

    #[test]
    fn sign_produces_64_bytes() {
        let kp = Keypair::new();
        let sig = kp.sign(b"hello lichen");
        assert_eq!(sig.len(), 64);
    }

    #[test]
    fn sign_deterministic() {
        let kp = Keypair::from_seed(&[7u8; 32]);
        let sig1 = kp.sign(b"msg");
        let sig2 = kp.sign(b"msg");
        assert_eq!(sig1, sig2);
    }

    #[test]
    fn sign_different_messages_differ() {
        let kp = Keypair::new();
        let sig1 = kp.sign(b"aaa");
        let sig2 = kp.sign(b"bbb");
        assert_ne!(sig1, sig2);
    }

    #[test]
    fn inner_returns_core_keypair() {
        let kp = Keypair::from_seed(&[5u8; 32]);
        assert_eq!(kp.inner().pubkey(), kp.pubkey());
    }

    #[test]
    fn default_works() {
        let kp = Keypair::default();
        // just verify it doesn't panic
        let _ = kp.pubkey();
    }

    #[test]
    fn pubkey_is_32_bytes() {
        let kp = Keypair::new();
        assert_eq!(kp.pubkey().0.len(), 32);
    }
}
