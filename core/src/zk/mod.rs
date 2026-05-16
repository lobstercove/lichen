//! Lichen Zero-Knowledge Proof Module
//!
//! The live shield, unshield, and transfer proof path runs on the native
//! Plonky3 STARK backend. Any remaining witness-adapter helpers are private to
//! proof tooling and are not part of validator or RPC verification.
//!
//! Current live backend:
//! - Poseidon2/Goldilocks commitments and Merkle nodes over canonical 32-byte values
//! - Hash-derived shielded spending/viewing keys and nullifiers
//! - Plonky3 FRI proofs for shield/unshield/transfer circuits
//! - ChaCha20-Poly1305 for note encryption

pub mod air;
pub mod circuits;
pub mod keys;
pub mod merkle;
pub mod note;
pub mod pedersen;
pub mod prover;
pub(crate) mod r1cs_bn254;
pub mod setup;
pub mod verifier;

#[cfg(test)]
mod e2e_tests;

use serde::{Deserialize, Serialize};

// Re-exports
pub use air::{
    build_constant_trace, build_shield_trace, build_stark_config, bytes32_to_goldilocks_words,
    deserialize_stark_proof, goldilocks_words_to_bytes32, goldilocks_words_to_u64,
    u64_to_goldilocks_words, ConstantTraceAir, LichenStarkConfig, LichenStarkProof,
    ReserveLiabilityAirPublicValues, ShieldAir, ShieldAirPublicValues, StarkField,
    TransferAirPublicValues, UnshieldAirPublicValues, RESERVE_LIABILITY_STARK_PUBLIC_INPUT_WORDS,
    SHIELD_AIR_TRACE_WIDTH, SHIELD_STARK_PUBLIC_INPUT_WORDS, STARK_TRACE_ROWS,
    TRANSFER_STARK_PUBLIC_INPUT_WORDS, UNSHIELD_STARK_PUBLIC_INPUT_WORDS,
};
pub use keys::{ShieldedKeypair, SpendingKey, ViewingKey};
pub use merkle::{
    commitment_hash, nullifier_hash, poseidon_hash_pair, random_scalar_bytes, recipient_hash,
    recipient_preimage_from_bytes, MerklePath, MerkleTree, TREE_DEPTH,
};
pub use note::{EncryptedNote, Note, Nullifier};
pub use pedersen::{CommitmentSecret, ValueCommitment};
pub use prover::Prover;
pub use verifier::Verifier;

/// Versioned proof-scheme identifier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ZkSchemeVersion {
    /// Native Plonky3 STARK proof using FRI over Goldilocks with Poseidon2.
    #[default]
    Plonky3FriPoseidon2 = 0x01,
}

impl ZkSchemeVersion {
    pub fn fixed_proof_len(self) -> Option<usize> {
        None
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plonky3FriPoseidon2 => "plonky3-fri-poseidon2",
        }
    }
}

impl std::fmt::Display for ZkSchemeVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Proof type identifier for routing verification
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProofType {
    /// Shield: transparent -> shielded (deposit into pool)
    Shield,
    /// Unshield: shielded -> transparent (withdraw from pool)
    Unshield,
    /// Transfer: shielded -> shielded (private transfer)
    Transfer,
    /// Reserve/liability: proof-service statement over public aggregate totals.
    ReserveLiability,
}

impl ProofType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Shield => "shield",
            Self::Unshield => "unshield",
            Self::Transfer => "transfer",
            Self::ReserveLiability => "reserve_liability",
        }
    }
}

/// A scheme-versioned shielded proof envelope.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ZkProof {
    /// Raw proof bytes for the native Plonky3/FRI payload.
    pub proof_bytes: Vec<u8>,
    /// Which circuit this proof is for
    pub proof_type: ProofType,
    /// Legacy serialized public-input slot retained for backward-compatible
    /// encoding. The live Plonky3 path leaves this empty.
    pub public_inputs: Vec<[u8; 32]>,
    /// Canonical Goldilocks public-input words for the live Plonky3 path.
    #[serde(default)]
    pub stark_public_inputs: Vec<u64>,
    /// Which proof backend encoded `proof_bytes`.
    #[serde(default)]
    pub zk_scheme_version: ZkSchemeVersion,
}

impl ZkProof {
    pub fn plonky3(
        proof_type: ProofType,
        proof_bytes: Vec<u8>,
        stark_public_inputs: Vec<u64>,
    ) -> Self {
        Self {
            proof_bytes,
            proof_type,
            public_inputs: Vec::new(),
            stark_public_inputs,
            zk_scheme_version: ZkSchemeVersion::Plonky3FriPoseidon2,
        }
    }

    pub fn fixed_proof_len(&self) -> Option<usize> {
        self.zk_scheme_version.fixed_proof_len()
    }

    pub fn stark_public_inputs(&self) -> Result<&[u64], ShieldedError> {
        if self.zk_scheme_version == ZkSchemeVersion::Plonky3FriPoseidon2 {
            Ok(self.stark_public_inputs.as_slice())
        } else {
            Err(ShieldedError::UnsupportedProofScheme(
                self.zk_scheme_version,
            ))
        }
    }
}

/// The on-chain shielded pool state
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShieldedPoolState {
    /// Current Merkle tree root of all note commitments
    pub merkle_root: [u8; 32],
    /// Number of leaves (commitments) inserted
    pub commitment_count: u64,
    /// Total shielded balance in spores
    pub total_shielded: u64,
    /// Number of nullifiers marked spent
    #[serde(default)]
    pub nullifier_count: u64,
    /// Number of shield (transparent -> shielded) operations
    #[serde(default)]
    pub shield_count: u64,
    /// Number of unshield (shielded -> transparent) operations
    #[serde(default)]
    pub unshield_count: u64,
    /// Number of shielded transfer operations
    #[serde(default)]
    pub transfer_count: u64,
}

impl ShieldedPoolState {
    pub fn new() -> Self {
        Self {
            merkle_root: MerkleTree::empty_root(),
            commitment_count: 0,
            total_shielded: 0,
            nullifier_count: 0,
            shield_count: 0,
            unshield_count: 0,
            transfer_count: 0,
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
    /// Proof used a scheme that the active verifier backend cannot process yet
    UnsupportedProofScheme(ZkSchemeVersion),
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
            Self::UnsupportedProofScheme(scheme) => {
                write!(f, "unsupported proof scheme: {}", scheme)
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
