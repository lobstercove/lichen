//! MoltChain Zero-Knowledge Proof Module
//!
//! Production-grade shielded transactions using Groth16 over BN254
//! via the arkworks library suite. Replaces the placeholder privacy module.
//!
//! Architecture:
//! - Pedersen commitments for hiding values
//! - Poseidon hash for SNARK-friendly Merkle tree
//! - Groth16 proofs for shield/unshield/transfer circuits
//! - ChaCha20-Poly1305 for note encryption

pub mod circuits;
pub mod keys;
pub mod merkle;
pub mod note;
pub mod pedersen;
pub mod prover;
pub mod setup;
pub mod verifier;

#[cfg(test)]
mod e2e_tests;

use serde::{Deserialize, Serialize};

// Re-exports
pub use keys::{ShieldedKeypair, SpendingKey, ViewingKey};
pub use merkle::{fr_to_bytes, poseidon_config, poseidon_hash_fr, MerklePath, MerkleTree, TREE_DEPTH};
pub use note::{EncryptedNote, Note, Nullifier};
pub use pedersen::PedersenCommitment;
pub use prover::Prover;
pub use verifier::Verifier;

/// Proof type identifier for routing verification
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProofType {
    /// Shield: transparent -> shielded (deposit into pool)
    Shield,
    /// Unshield: shielded -> transparent (withdraw from pool)
    Unshield,
    /// Transfer: shielded -> shielded (private transfer)
    Transfer,
}

/// A serialized Groth16 proof (128 bytes: 2 G1 points + 1 G2 point on BN254)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZkProof {
    /// Raw proof bytes (128 bytes for Groth16/BN254)
    pub proof_bytes: Vec<u8>,
    /// Which circuit this proof is for
    pub proof_type: ProofType,
    /// Public inputs to the circuit (serialized field elements)
    pub public_inputs: Vec<[u8; 32]>,
}

/// The on-chain shielded pool state
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShieldedPoolState {
    /// Current Merkle tree root of all note commitments
    pub merkle_root: [u8; 32],
    /// Number of leaves (commitments) inserted
    pub commitment_count: u64,
    /// Total shielded balance in shells
    pub total_shielded: u64,
    /// Shield verification key hash (for integrity check)
    pub vk_shield_hash: [u8; 32],
    /// Unshield verification key hash
    pub vk_unshield_hash: [u8; 32],
    /// Transfer verification key hash
    pub vk_transfer_hash: [u8; 32],
}

impl ShieldedPoolState {
    pub fn new() -> Self {
        Self {
            merkle_root: MerkleTree::empty_root(),
            commitment_count: 0,
            total_shielded: 0,
            vk_shield_hash: [0u8; 32],
            vk_unshield_hash: [0u8; 32],
            vk_transfer_hash: [0u8; 32],
        }
    }
}

impl Default for ShieldedPoolState {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of a shielded operation
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ShieldedTxResult {
    /// Shield succeeded: new commitment index
    Shielded { commitment_index: u64 },
    /// Unshield succeeded: amount released
    Unshielded { amount: u64, recipient: [u8; 32] },
    /// Transfer succeeded: new commitment indices
    Transferred {
        nullifiers_spent: Vec<[u8; 32]>,
        new_commitment_indices: Vec<u64>,
    },
}

/// Error types for shielded operations
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShieldedError {
    /// ZK proof failed verification
    InvalidProof(String),
    /// Nullifier already in the spent set (double-spend)
    NullifierAlreadySpent([u8; 32]),
    /// Merkle root doesn't match current state
    InvalidMerkleRoot,
    /// Insufficient shielded balance for unshield
    InsufficientBalance { requested: u64, available: u64 },
    /// Invalid commitment (zero or malformed)
    InvalidCommitment,
    /// Verification key not initialized
    VerificationKeyMissing(ProofType),
    /// Serialization error
    SerializationError(String),
    /// Pool overflow
    PoolOverflow,
}

impl std::fmt::Display for ShieldedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidProof(msg) => write!(f, "invalid ZK proof: {}", msg),
            Self::NullifierAlreadySpent(n) => {
                write!(f, "nullifier already spent: {}", hex::encode(n))
            }
            Self::InvalidMerkleRoot => write!(f, "merkle root mismatch"),
            Self::InsufficientBalance {
                requested,
                available,
            } => write!(
                f,
                "insufficient shielded balance: requested {} but only {} available",
                requested, available
            ),
            Self::InvalidCommitment => write!(f, "invalid note commitment"),
            Self::VerificationKeyMissing(pt) => {
                write!(f, "verification key not loaded for {:?}", pt)
            }
            Self::SerializationError(msg) => write!(f, "serialization error: {}", msg),
            Self::PoolOverflow => write!(f, "shielded pool balance overflow"),
        }
    }
}

impl std::error::Error for ShieldedError {}

/// Compute units cost for ZK operations (for gas metering)
pub const SHIELD_COMPUTE_UNITS: u64 = 100_000;
pub const UNSHIELD_COMPUTE_UNITS: u64 = 150_000;
pub const TRANSFER_COMPUTE_UNITS: u64 = 200_000;
