// Lichen Core - Cryptographic Hash

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// 32-byte hash (SHA-256)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct Hash(pub [u8; 32]);

impl Hash {
    /// Create hash from bytes
    pub const fn new(bytes: [u8; 32]) -> Self {
        Hash(bytes)
    }

    /// Hash arbitrary data
    #[allow(clippy::self_named_constructors)]
    pub fn hash(data: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result);
        Hash(hash)
    }

    /// PERF-OPT 7: Hash two byte slices without concatenating them first.
    /// Avoids a heap allocation (Vec::with_capacity + extend_from_slice)
    /// on every call in hot paths like Merkle leaf computation where
    /// we hash(pubkey || account_bytes). With 100+ dirty accounts per
    /// block, this eliminates 100+ allocations per state root computation.
    pub fn hash_two_parts(part_a: &[u8], part_b: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(part_a);
        hasher.update(part_b);
        let result = hasher.finalize();
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&result);
        Hash(hash)
    }

    /// Convert to hex string
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parse from hex string
    pub fn from_hex(s: &str) -> Result<Self, String> {
        let bytes = hex::decode(s).map_err(|e| format!("Invalid hex: {}", e))?;
        if bytes.len() != 32 {
            return Err(format!("Invalid length: {} (expected 32)", bytes.len()));
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&bytes);
        Ok(Hash(hash))
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash() {
        let data = b"The moss is ready!";
        let hash = Hash::hash(data);
        println!("Hash: {}", hash);
        assert_ne!(hash.0, [0u8; 32]);
    }

    #[test]
    fn test_hex_roundtrip() {
        let original = Hash::hash(b"Lichen");
        let hex = original.to_hex();
        let parsed = Hash::from_hex(&hex).unwrap();
        assert_eq!(original, parsed);
    }
}
