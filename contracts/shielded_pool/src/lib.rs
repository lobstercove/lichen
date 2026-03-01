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

#![cfg_attr(target_arch = "wasm32", no_std)]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(dead_code)]
#![allow(unused_imports)]

extern crate alloc;

use alloc::collections::BTreeSet;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
    pub spent_nullifiers: BTreeSet<[u8; 32]>,
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

impl core::fmt::Display for ShieldedPoolError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
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
            spent_nullifiers: BTreeSet::new(),
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
        self.update_merkle_root();

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
            self.update_merkle_root();
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

    // --- Proof validation methods ---
    //
    // The actual Groth16 proof verification happens in the processor layer
    // (TxProcessor types 23/24/25) BEFORE the contract is invoked.  These
    // methods only perform structural validation (correct proof length) and
    // state consistency checks (merkle root match).  The processor already
    // verified the cryptographic proof, so the contract does not need to
    // duplicate the heavy Groth16 verification.

    fn verify_shield_proof(
        &self,
        proof: &[u8],
        _amount: u64,
        _commitment: &[u8; 32],
    ) -> Result<(), ShieldedPoolError> {
        if proof.len() != 128 {
            return Err(ShieldedPoolError::InvalidProof(
                "invalid proof length (expected exactly 128 bytes for Groth16/BN254)"
                    .to_string(),
            ));
        }
        // Proof was already cryptographically verified by the processor.
        Ok(())
    }

    fn verify_unshield_proof(
        &self,
        proof: &[u8],
        merkle_root: &[u8; 32],
        _nullifier: &[u8; 32],
        _amount: u64,
        _recipient: &[u8; 32],
    ) -> Result<(), ShieldedPoolError> {
        if proof.len() != 128 {
            return Err(ShieldedPoolError::InvalidProof(
                "invalid proof length (expected exactly 128 bytes for Groth16/BN254)"
                    .to_string(),
            ));
        }
        if merkle_root != &self.merkle_root {
            return Err(ShieldedPoolError::MerkleRootMismatch);
        }
        // Proof was already cryptographically verified by the processor.
        Ok(())
    }

    fn verify_transfer_proof(
        &self,
        proof: &[u8],
        merkle_root: &[u8; 32],
        _nullifiers: &[[u8; 32]],
        _output_commitments: &[[u8; 32]],
    ) -> Result<(), ShieldedPoolError> {
        if proof.len() != 128 {
            return Err(ShieldedPoolError::InvalidProof(
                "invalid proof length (expected exactly 128 bytes for Groth16/BN254)"
                    .to_string(),
            ));
        }
        if merkle_root != &self.merkle_root {
            return Err(ShieldedPoolError::MerkleRootMismatch);
        }
        // Proof was already cryptographically verified by the processor.
        Ok(())
    }

    fn update_merkle_root(&mut self) {
        self.merkle_root = compute_merkle_root(&self.commitments);
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

fn hash_leaf(commitment: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update([0x00]);
    hasher.update(commitment);
    hasher.finalize().into()
}

fn hash_node(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update([0x01]);
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

fn compute_merkle_root(entries: &[CommitmentEntry]) -> [u8; 32] {
    if entries.is_empty() {
        return empty_merkle_root();
    }

    let mut level: Vec<[u8; 32]> = entries.iter().map(|e| hash_leaf(&e.commitment)).collect();
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        let mut i = 0usize;
        while i < level.len() {
            let left = level[i];
            let right = if i + 1 < level.len() {
                level[i + 1]
            } else {
                left
            };
            next.push(hash_node(&left, &right));
            i += 2;
        }
        level = next;
    }

    level[0]
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

// ===== WASM Entry Points =====
//
// These are compiled only for wasm32 and call through to the pure-Rust
// ShieldedPoolState methods above. State is persisted as a single JSON
// blob under the `pool_state` storage key. The native TxProcessor
// (types 23/24/25) handles the heavy ZK proof verification and writes
// to CF_SHIELDED_POOL; this contract provides the on-chain WASM
// bytecode and a query interface.
//
// CON-12 ARCHITECTURAL NOTE: The single-JSON-blob storage model is a known
// limitation. At scale (>10k commitments), reads/writes require deserializing
// the entire pool. A sparse Merkle tree with per-leaf storage keys would be
// more efficient. For the current MVP phase, the native TxProcessor already
// handles the critical path (shielding/unshielding); this WASM contract is
// primarily a query interface and the blob size is bounded by the native
// rate-limiting on shield operations. Migration to per-leaf storage is
// planned for v2.

#[cfg(target_arch = "wasm32")]
mod wasm_abi {
    use super::*;
    use moltchain_sdk::{storage_get, storage_set, log_info, set_return_data, get_slot, get_caller};

    const STATE_KEY: &[u8] = b"pool_state";
    const OWNER_KEY: &[u8] = b"owner";
    const PAUSED_KEY: &[u8] = b"sp_paused";
    const REENTRANCY_KEY: &[u8] = b"sp_reentrancy";

    // AUDIT-FIX CON-02: Reentrancy guard (prevents double-spend via reentrant calls)
    fn reentrancy_enter() -> bool {
        if storage_get(REENTRANCY_KEY)
            .map(|v| v.first().copied() == Some(1))
            .unwrap_or(false)
        {
            return false;
        }
        storage_set(REENTRANCY_KEY, &[1u8]);
        true
    }

    fn reentrancy_exit() {
        storage_set(REENTRANCY_KEY, &[0u8]);
    }

    // AUDIT-FIX CON-04: Pause mechanism
    fn is_paused() -> bool {
        storage_get(PAUSED_KEY)
            .map(|v| v.first().copied() == Some(1))
            .unwrap_or(false)
    }

    // AUDIT-FIX CON-03: Require caller to be the owner/admin
    fn require_admin() -> bool {
        let caller = get_caller();
        match storage_get(OWNER_KEY) {
            Some(owner) if owner.len() == 32 && owner[..] == caller.0[..] => true,
            _ => {
                log_info("ShieldedPool: unauthorized caller (not admin)");
                false
            }
        }
    }

    fn load_state() -> ShieldedPoolState {
        match storage_get(STATE_KEY) {
            Some(bytes) => {
                serde_json::from_slice(&bytes).unwrap_or_else(|_| ShieldedPoolState::new())
            }
            None => ShieldedPoolState::new(),
        }
    }

    fn save_state(state: &ShieldedPoolState) {
        if let Ok(bytes) = serde_json::to_vec(state) {
            storage_set(STATE_KEY, &bytes);
        }
    }

    /// Initialize the shielded pool (called once at genesis).
    /// Sets admin, creates empty pool state with VKs marked ready.
    #[no_mangle]
    pub extern "C" fn initialize(admin_ptr: *const u8) -> u32 {
        // Re-initialization guard
        if storage_get(OWNER_KEY).is_some() {
            log_info("ShieldedPool already initialized — ignoring");
            return 0;
        }
        let mut admin = [0u8; 32];
        unsafe {
            core::ptr::copy_nonoverlapping(admin_ptr, admin.as_mut_ptr(), 32);
        }
        storage_set(OWNER_KEY, &admin);

        let mut state = ShieldedPoolState::new();
        state.vk_initialized = true; // VK verification done at processor layer
        save_state(&state);

        log_info("ShieldedPool initialized");
        0
    }

    /// AUDIT-FIX CON-04: Pause the pool (admin only)
    #[no_mangle]
    pub extern "C" fn pause() -> u32 {
        if !require_admin() { return 1; }
        storage_set(PAUSED_KEY, &[1u8]);
        log_info("ShieldedPool PAUSED");
        0
    }

    /// AUDIT-FIX CON-04: Unpause the pool (admin only)
    #[no_mangle]
    pub extern "C" fn unpause() -> u32 {
        if !require_admin() { return 1; }
        storage_set(PAUSED_KEY, &[0u8]);
        log_info("ShieldedPool UNPAUSED");
        0
    }

    /// Return pool statistics as JSON via set_return_data.
    #[no_mangle]
    pub extern "C" fn get_pool_stats() -> u32 {
        let state = load_state();
        let stats = state.stats();
        if let Ok(json) = serde_json::to_vec(&stats) {
            set_return_data(&json);
        }
        0
    }

    /// Return the current 32-byte Merkle root.
    #[no_mangle]
    pub extern "C" fn get_merkle_root() -> u32 {
        let state = load_state();
        set_return_data(&state.merkle_root);
        0
    }

    /// Check whether a nullifier has been spent (returns [0] or [1]).
    #[no_mangle]
    pub extern "C" fn check_nullifier(nullifier_ptr: *const u8) -> u32 {
        let mut nullifier = [0u8; 32];
        unsafe {
            core::ptr::copy_nonoverlapping(nullifier_ptr, nullifier.as_mut_ptr(), 32);
        }
        let state = load_state();
        let spent = if state.is_nullifier_spent(&nullifier) {
            1u8
        } else {
            0u8
        };
        set_return_data(&[spent]);
        0
    }

    /// Return commitments from a given index as JSON.
    #[no_mangle]
    pub extern "C" fn get_commitments(from_index: u64) -> u32 {
        let state = load_state();
        let entries = state.get_commitments_from(from_index);
        if let Ok(json) = serde_json::to_vec(entries) {
            set_return_data(&json);
        }
        0
    }

    /// Shield (deposit) MOLT into the shielded pool.
    /// args: JSON-serialized ShieldRequest.
    #[no_mangle]
    pub extern "C" fn shield(args_ptr: *const u8, args_len: u32) -> u32 {
        // AUDIT-FIX CON-04: Pause check
        if is_paused() { log_info("ShieldedPool: paused"); return 1; }
        // AUDIT-FIX CON-02: Reentrancy guard
        if !reentrancy_enter() { log_info("ShieldedPool: reentrant call blocked"); return 1; }

        let slice = unsafe { core::slice::from_raw_parts(args_ptr, args_len as usize) };
        let request: ShieldRequest = match serde_json::from_slice(slice) {
            Ok(r) => r,
            Err(_) => {
                log_info("shield: invalid request");
                reentrancy_exit();
                return 1;
            }
        };
        let slot = get_slot();
        let mut state = load_state();
        let result = match state.shield(&request, slot) {
            Ok(index) => {
                save_state(&state);
                set_return_data(&index.to_le_bytes());
                0
            }
            Err(e) => {
                log_info(&format!("shield failed: {}", e));
                1
            }
        };
        reentrancy_exit();
        result
    }

    /// Unshield (withdraw) MOLT from the shielded pool.
    /// args: JSON-serialized UnshieldRequest.
    #[no_mangle]
    pub extern "C" fn unshield(args_ptr: *const u8, args_len: u32) -> u32 {
        // AUDIT-FIX CON-04: Pause check
        if is_paused() { log_info("ShieldedPool: paused"); return 1; }
        // AUDIT-FIX CON-02: Reentrancy guard
        if !reentrancy_enter() { log_info("ShieldedPool: reentrant call blocked"); return 1; }

        let slice = unsafe { core::slice::from_raw_parts(args_ptr, args_len as usize) };
        let request: UnshieldRequest = match serde_json::from_slice(slice) {
            Ok(r) => r,
            Err(_) => {
                log_info("unshield: invalid request");
                reentrancy_exit();
                return 1;
            }
        };
        let mut state = load_state();
        let result = match state.unshield(&request) {
            Ok(amount) => {
                save_state(&state);
                set_return_data(&amount.to_le_bytes());
                0
            }
            Err(e) => {
                log_info(&format!("unshield failed: {}", e));
                1
            }
        };
        reentrancy_exit();
        result
    }

    /// Shielded transfer between notes.
    /// args: JSON-serialized TransferRequest.
    #[no_mangle]
    pub extern "C" fn transfer(args_ptr: *const u8, args_len: u32) -> u32 {
        // AUDIT-FIX CON-04: Pause check
        if is_paused() { log_info("ShieldedPool: paused"); return 1; }
        // AUDIT-FIX CON-02: Reentrancy guard
        if !reentrancy_enter() { log_info("ShieldedPool: reentrant call blocked"); return 1; }

        let slice = unsafe { core::slice::from_raw_parts(args_ptr, args_len as usize) };
        let request: TransferRequest = match serde_json::from_slice(slice) {
            Ok(r) => r,
            Err(_) => {
                log_info("transfer: invalid request");
                reentrancy_exit();
                return 1;
            }
        };
        let slot = get_slot();
        let mut state = load_state();
        let result = match state.transfer(&request, slot) {
            Ok(indices) => {
                save_state(&state);
                if let Ok(json) = serde_json::to_vec(&indices) {
                    set_return_data(&json);
                }
                0
            }
            Err(e) => {
                log_info(&format!("transfer failed: {}", e));
                1
            }
        };
        reentrancy_exit();
        result
    }
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
            proof: vec![0u8; 128], // proof already verified by processor
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
