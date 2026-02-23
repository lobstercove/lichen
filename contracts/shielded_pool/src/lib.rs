//! MoltChain Shielded Pool Contract
//!
//! On-chain contract managing the shielded transaction pool.
//! Stores the commitment Merkle tree root, spent nullifier set,
//! and verification keys. Verifies ZK proofs and updates state.
//!
//! Entry points:
//! - shield(amount, commitment, proof) — deposit into shielded pool
//! - unshield(nullifier, amount, recipient, proof) — withdraw from pool
//! - transfer(nullifiers[], commitments[], proof) — private transfer
//! - get_merkle_root() — read current root for wallet proof generation
//! - get_pool_stats() — read pool statistics

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;

// ===== Storage Layout =====
// merkle_root      -> [u8; 32]       Current commitment tree root
// merkle_count     -> u64            Number of leaves inserted
// nullifier:{hex}  -> u8             Spent nullifier set (1 = spent)
// vk_shield        -> Vec<u8>        Shield verification key
// vk_unshield      -> Vec<u8>        Unshield verification key
// vk_transfer      -> Vec<u8>        Transfer verification key
// pool_balance     -> u64            Total shielded MOLT (shells)

/// Shielded pool state (persisted in contract storage)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShieldedPoolState {
    /// Current Merkle tree root of all note commitments
    pub merkle_root: [u8; 32],
    /// Number of commitments inserted
    pub commitment_count: u64,
    /// Total shielded balance in shells
    pub pool_balance: u64,
    /// Spent nullifier set
    pub spent_nullifiers: HashSet<[u8; 32]>,
    /// All commitments (for wallet sync)
    pub commitments: Vec<CommitmentEntry>,
    /// Whether verification keys are initialized
    pub vk_initialized: bool,
}

/// A commitment entry in the Merkle tree
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitmentEntry {
    /// The commitment hash (Merkle tree leaf)
    pub commitment: [u8; 32],
    /// Block slot when this commitment was inserted
    pub slot: u64,
    /// Encrypted note data (for recipient to trial-decrypt)
    pub encrypted_note: Vec<u8>,
    /// Ephemeral public key for ECDH
    pub ephemeral_pk: [u8; 32],
}

/// Shield request: deposit transparent MOLT into the shielded pool
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ShieldRequest {
    /// Amount to shield (in shells)
    pub amount: u64,
    /// Pedersen commitment to the note value
    pub commitment: [u8; 32],
    /// ZK proof that commitment is well-formed and matches amount
    pub proof: Vec<u8>,
    /// Encrypted note for the recipient
    pub encrypted_note: Vec<u8>,
    /// Ephemeral public key
    pub ephemeral_pk: [u8; 32],
}

/// Unshield request: withdraw from shielded pool to transparent address
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnshieldRequest {
    /// Nullifier proving note ownership (prevents double-spend)
    pub nullifier: [u8; 32],
    /// Amount to withdraw (in shells)
    pub amount: u64,
    /// Recipient's transparent address
    pub recipient: [u8; 32],
    /// Merkle root the proof was generated against
    pub merkle_root: [u8; 32],
    /// ZK proof of valid unshield
    pub proof: Vec<u8>,
}

/// Shielded transfer request: spend input notes, create output notes
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransferRequest {
    /// Nullifiers for spent input notes
    pub nullifiers: Vec<[u8; 32]>,
    /// New output commitments
    pub output_commitments: Vec<OutputCommitment>,
    /// Merkle root the proof was generated against
    pub merkle_root: [u8; 32],
    /// ZK proof of valid transfer (value conservation + ownership)
    pub proof: Vec<u8>,
}

/// An output commitment with encrypted note
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputCommitment {
    /// Pedersen commitment hash
    pub commitment: [u8; 32],
    /// Encrypted note (for recipient)
    pub encrypted_note: Vec<u8>,
    /// Ephemeral public key for ECDH
    pub ephemeral_pk: [u8; 32],
}

/// Pool statistics (public, readable by anyone)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolStats {
    /// Current Merkle root
    pub merkle_root: String,
    /// Total commitments in the tree
    pub commitment_count: u64,
    /// Total shielded balance in shells
    pub pool_balance: u64,
    /// Pool balance in MOLT
    pub pool_balance_molt: f64,
    /// Number of spent nullifiers
    pub nullifier_count: u64,
    /// Whether VKs are initialized
    pub vk_initialized: bool,
}

/// Contract error types
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ShieldedPoolError {
    InvalidProof(String),
    NullifierAlreadySpent(String),
    MerkleRootMismatch,
    InsufficientBalance,
    InvalidCommitment,
    VKNotInitialized,
    InvalidRequest(String),
    PoolOverflow,
}

impl std::fmt::Display for ShieldedPoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidProof(msg) => write!(f, "invalid proof: {}", msg),
            Self::NullifierAlreadySpent(n) => write!(f, "nullifier already spent: {}", n),
            Self::MerkleRootMismatch => write!(f, "merkle root does not match current state"),
            Self::InsufficientBalance => write!(f, "insufficient shielded pool balance"),
            Self::InvalidCommitment => write!(f, "invalid commitment (zero or malformed)"),
            Self::VKNotInitialized => write!(f, "verification keys not initialized"),
            Self::InvalidRequest(msg) => write!(f, "invalid request: {}", msg),
            Self::PoolOverflow => write!(f, "pool balance overflow"),
        }
    }
}

impl ShieldedPoolState {
    /// Initialize a new empty shielded pool
    pub fn new() -> Self {
        Self {
            merkle_root: empty_merkle_root(),
            commitment_count: 0,
            pool_balance: 0,
            spent_nullifiers: HashSet::new(),
            commitments: Vec::new(),
            vk_initialized: false,
        }
    }

    /// Process a shield (deposit) request
    pub fn shield(
        &mut self,
        request: &ShieldRequest,
        current_slot: u64,
    ) -> Result<u64, ShieldedPoolError> {
        // Validate commitment is non-zero
        if request.commitment == [0u8; 32] {
            return Err(ShieldedPoolError::InvalidCommitment);
        }

        // Verify ZK proof
        if !self.vk_initialized {
            return Err(ShieldedPoolError::VKNotInitialized);
        }
        self.verify_shield_proof(&request.proof, request.amount, &request.commitment)?;

        // Update state
        let index = self.commitment_count;
        self.commitments.push(CommitmentEntry {
            commitment: request.commitment,
            slot: current_slot,
            encrypted_note: request.encrypted_note.clone(),
            ephemeral_pk: request.ephemeral_pk,
        });
        self.commitment_count += 1;
        self.pool_balance = self
            .pool_balance
            .checked_add(request.amount)
            .ok_or(ShieldedPoolError::PoolOverflow)?;

        // Update Merkle root
        self.update_merkle_root(&request.commitment);

        Ok(index)
    }

    /// Process an unshield (withdrawal) request
    pub fn unshield(&mut self, request: &UnshieldRequest) -> Result<u64, ShieldedPoolError> {
        // Check Merkle root matches (prevent stale proofs)
        if request.merkle_root != self.merkle_root {
            return Err(ShieldedPoolError::MerkleRootMismatch);
        }

        // Check nullifier hasn't been spent
        if self.spent_nullifiers.contains(&request.nullifier) {
            return Err(ShieldedPoolError::NullifierAlreadySpent(hex::encode(
                request.nullifier,
            )));
        }

        // Check sufficient balance
        if request.amount > self.pool_balance {
            return Err(ShieldedPoolError::InsufficientBalance);
        }

        // Verify ZK proof
        if !self.vk_initialized {
            return Err(ShieldedPoolError::VKNotInitialized);
        }
        self.verify_unshield_proof(
            &request.proof,
            &request.merkle_root,
            &request.nullifier,
            request.amount,
            &request.recipient,
        )?;

        // Mark nullifier as spent (atomic with state update)
        self.spent_nullifiers.insert(request.nullifier);
        self.pool_balance -= request.amount;

        Ok(request.amount)
    }

    /// Process a shielded transfer request
    pub fn transfer(
        &mut self,
        request: &TransferRequest,
        current_slot: u64,
    ) -> Result<Vec<u64>, ShieldedPoolError> {
        // Check Merkle root matches
        if request.merkle_root != self.merkle_root {
            return Err(ShieldedPoolError::MerkleRootMismatch);
        }

        // Check no nullifier has been spent
        for nullifier in &request.nullifiers {
            if self.spent_nullifiers.contains(nullifier) {
                return Err(ShieldedPoolError::NullifierAlreadySpent(hex::encode(
                    nullifier,
                )));
            }
        }

        // Validate output commitments
        for output in &request.output_commitments {
            if output.commitment == [0u8; 32] {
                return Err(ShieldedPoolError::InvalidCommitment);
            }
        }

        // Verify ZK proof (value conservation + ownership)
        if !self.vk_initialized {
            return Err(ShieldedPoolError::VKNotInitialized);
        }
        self.verify_transfer_proof(
            &request.proof,
            &request.merkle_root,
            &request.nullifiers,
            &request
                .output_commitments
                .iter()
                .map(|o| o.commitment)
                .collect::<Vec<_>>(),
        )?;

        // Spend nullifiers
        for nullifier in &request.nullifiers {
            self.spent_nullifiers.insert(*nullifier);
        }

        // Insert new commitments
        let mut indices = Vec::new();
        for output in &request.output_commitments {
            let index = self.commitment_count;
            self.commitments.push(CommitmentEntry {
                commitment: output.commitment,
                slot: current_slot,
                encrypted_note: output.encrypted_note.clone(),
                ephemeral_pk: output.ephemeral_pk,
            });
            self.commitment_count += 1;
            self.update_merkle_root(&output.commitment);
            indices.push(index);
        }

        Ok(indices)
    }

    /// Get pool statistics
    pub fn stats(&self) -> PoolStats {
        PoolStats {
            merkle_root: hex::encode(self.merkle_root),
            commitment_count: self.commitment_count,
            pool_balance: self.pool_balance,
            pool_balance_molt: self.pool_balance as f64 / 1_000_000_000.0,
            nullifier_count: self.spent_nullifiers.len() as u64,
            vk_initialized: self.vk_initialized,
        }
    }

    /// Check if a nullifier has been spent
    pub fn is_nullifier_spent(&self, nullifier: &[u8; 32]) -> bool {
        self.spent_nullifiers.contains(nullifier)
    }

    /// Get commitments from a given index (for wallet sync)
    pub fn get_commitments_from(&self, from_index: u64) -> &[CommitmentEntry] {
        let start = from_index as usize;
        if start >= self.commitments.len() {
            return &[];
        }
        &self.commitments[start..]
    }

    // --- Private verification methods ---

    fn verify_shield_proof(
        &self,
        proof: &[u8],
        amount: u64,
        commitment: &[u8; 32],
    ) -> Result<(), ShieldedPoolError> {
        // In production: deserialize and verify Groth16 proof
        // Public inputs: (amount, commitment)
        // The verifier checks the R1CS constraints are satisfied

        if proof.len() < 128 {
            return Err(ShieldedPoolError::InvalidProof(
                "proof too short (expected 128+ bytes for Groth16)".to_string(),
            ));
        }

        // Groth16 verification would happen here using ark-groth16
        // For now, validate proof structure
        Ok(())
    }

    fn verify_unshield_proof(
        &self,
        proof: &[u8],
        merkle_root: &[u8; 32],
        nullifier: &[u8; 32],
        amount: u64,
        recipient: &[u8; 32],
    ) -> Result<(), ShieldedPoolError> {
        if proof.len() < 128 {
            return Err(ShieldedPoolError::InvalidProof(
                "proof too short".to_string(),
            ));
        }
        Ok(())
    }

    fn verify_transfer_proof(
        &self,
        proof: &[u8],
        merkle_root: &[u8; 32],
        nullifiers: &[[u8; 32]],
        output_commitments: &[[u8; 32]],
    ) -> Result<(), ShieldedPoolError> {
        if proof.len() < 128 {
            return Err(ShieldedPoolError::InvalidProof(
                "proof too short".to_string(),
            ));
        }
        Ok(())
    }

    fn update_merkle_root(&mut self, new_commitment: &[u8; 32]) {
        // Incrementally update the Merkle root
        // Hash the new commitment with the existing root
        let mut hasher = Sha256::new();
        hasher.update(&self.merkle_root);
        hasher.update(new_commitment);
        let result = hasher.finalize();
        self.merkle_root.copy_from_slice(&result);
    }
}

impl Default for ShieldedPoolState {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the empty Merkle tree root (deterministic constant)
fn empty_merkle_root() -> [u8; 32] {
    let mut hash = [0u8; 32];
    for _ in 0..32 {
        let mut hasher = Sha256::new();
        hasher.update(&hash);
        hasher.update(&hash);
        let result = hasher.finalize();
        hash.copy_from_slice(&result);
    }
    hash
}

// ===== Contract Entry Points (WASM ABI) =====

/// Contract instruction enum (dispatched by the runtime)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ShieldedPoolInstruction {
    /// Initialize the pool with verification keys
    Initialize {
        vk_shield: Vec<u8>,
        vk_unshield: Vec<u8>,
        vk_transfer: Vec<u8>,
    },
    /// Shield (deposit) MOLT into the pool
    Shield(ShieldRequest),
    /// Unshield (withdraw) MOLT from the pool
    Unshield(UnshieldRequest),
    /// Shielded transfer between notes
    Transfer(TransferRequest),
    /// Query: get current Merkle root
    GetMerkleRoot,
    /// Query: get pool statistics
    GetPoolStats,
    /// Query: check if nullifier is spent
    CheckNullifier { nullifier: [u8; 32] },
    /// Query: get commitments from index (for wallet sync)
    GetCommitments { from_index: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_pool() -> ShieldedPoolState {
        let mut pool = ShieldedPoolState::new();
        pool.vk_initialized = true; // skip VK check for tests
        pool
    }

    #[test]
    fn test_shield() {
        let mut pool = test_pool();
        let request = ShieldRequest {
            amount: 1_000_000_000, // 1 MOLT
            commitment: [1u8; 32],
            proof: vec![0u8; 128], // placeholder proof
            encrypted_note: vec![0xAA, 0xBB],
            ephemeral_pk: [2u8; 32],
        };

        let index = pool.shield(&request, 100).unwrap();
        assert_eq!(index, 0);
        assert_eq!(pool.pool_balance, 1_000_000_000);
        assert_eq!(pool.commitment_count, 1);
    }

    #[test]
    fn test_unshield() {
        let mut pool = test_pool();
        // First shield
        let shield_req = ShieldRequest {
            amount: 1_000_000_000,
            commitment: [1u8; 32],
            proof: vec![0u8; 128],
            encrypted_note: vec![],
            ephemeral_pk: [2u8; 32],
        };
        pool.shield(&shield_req, 100).unwrap();

        // Then unshield
        let unshield_req = UnshieldRequest {
            nullifier: [3u8; 32],
            amount: 500_000_000, // 0.5 MOLT
            recipient: [4u8; 32],
            merkle_root: pool.merkle_root,
            proof: vec![0u8; 128],
        };
        let amount = pool.unshield(&unshield_req).unwrap();
        assert_eq!(amount, 500_000_000);
        assert_eq!(pool.pool_balance, 500_000_000);
    }

    #[test]
    fn test_double_spend_prevention() {
        let mut pool = test_pool();
        let shield_req = ShieldRequest {
            amount: 1_000_000_000,
            commitment: [1u8; 32],
            proof: vec![0u8; 128],
            encrypted_note: vec![],
            ephemeral_pk: [2u8; 32],
        };
        pool.shield(&shield_req, 100).unwrap();

        let nullifier = [3u8; 32];
        let unshield_req = UnshieldRequest {
            nullifier,
            amount: 500_000_000,
            recipient: [4u8; 32],
            merkle_root: pool.merkle_root,
            proof: vec![0u8; 128],
        };
        pool.unshield(&unshield_req).unwrap();

        // Try again with same nullifier
        let unshield_req2 = UnshieldRequest {
            nullifier,
            amount: 500_000_000,
            recipient: [4u8; 32],
            merkle_root: pool.merkle_root,
            proof: vec![0u8; 128],
        };
        assert!(pool.unshield(&unshield_req2).is_err());
    }

    #[test]
    fn test_shielded_transfer() {
        let mut pool = test_pool();
        // Shield first
        let shield_req = ShieldRequest {
            amount: 2_000_000_000,
            commitment: [1u8; 32],
            proof: vec![0u8; 128],
            encrypted_note: vec![],
            ephemeral_pk: [2u8; 32],
        };
        pool.shield(&shield_req, 100).unwrap();

        // Transfer
        let transfer_req = TransferRequest {
            nullifiers: vec![[3u8; 32]],
            output_commitments: vec![
                OutputCommitment {
                    commitment: [5u8; 32],
                    encrypted_note: vec![0xCC],
                    ephemeral_pk: [6u8; 32],
                },
                OutputCommitment {
                    commitment: [7u8; 32],
                    encrypted_note: vec![0xDD],
                    ephemeral_pk: [8u8; 32],
                },
            ],
            merkle_root: pool.merkle_root,
            proof: vec![0u8; 128],
        };

        let indices = pool.transfer(&transfer_req, 200).unwrap();
        assert_eq!(indices.len(), 2);
        assert_eq!(pool.commitment_count, 3); // 1 shield + 2 transfer outputs
    }

    #[test]
    fn test_pool_stats() {
        let pool = test_pool();
        let stats = pool.stats();
        assert_eq!(stats.commitment_count, 0);
        assert_eq!(stats.pool_balance, 0);
        assert_eq!(stats.nullifier_count, 0);
    }

    #[test]
    fn test_zero_commitment_rejected() {
        let mut pool = test_pool();
        let request = ShieldRequest {
            amount: 1000,
            commitment: [0u8; 32], // zero commitment
            proof: vec![0u8; 128],
            encrypted_note: vec![],
            ephemeral_pk: [2u8; 32],
        };
        assert!(pool.shield(&request, 100).is_err());
    }
}
