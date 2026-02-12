//! Keypair and public key management

pub use moltchain_core::{Keypair as CoreKeypair, Pubkey};

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
