// MoltChain Core - Block Structure

use crate::hash::Hash;
use crate::transaction::Transaction;
use serde::{Deserialize, Serialize};

/// Maximum block size in bytes (serialized) — 10 MB
pub const MAX_BLOCK_SIZE: usize = 10 * 1024 * 1024;

/// Maximum transactions per block
pub const MAX_TX_PER_BLOCK: usize = 10_000;

/// Maximum WASM contract code size — 2 MB
pub const MAX_CONTRACT_CODE: usize = 2 * 1024 * 1024;

/// Custom serde for [u8; 64] (ed25519 signatures)
mod sig_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(sig: &[u8; 64], s: S) -> Result<S::Ok, S::Error> {
        sig.as_slice().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 64], D::Error> {
        let v: Vec<u8> = Vec::deserialize(d)?;
        let arr: [u8; 64] = v
            .try_into()
            .map_err(|_| serde::de::Error::custom("expected 64 bytes for signature"))?;
        Ok(arr)
    }
}

/// Block header
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockHeader {
    /// Block number (slot)
    pub slot: u64,

    /// Hash of previous block
    pub parent_hash: Hash,

    /// Root hash of account state
    pub state_root: Hash,

    /// Root hash of transactions
    pub tx_root: Hash,

    /// Unix timestamp
    pub timestamp: u64,

    /// Validator that produced this block
    pub validator: [u8; 32],

    /// Ed25519 signature of the block producer over the header fields
    #[serde(with = "sig_serde", default = "zero_signature")]
    pub signature: [u8; 64],
}

fn zero_signature() -> [u8; 64] {
    [0u8; 64]
}

/// Complete block
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    /// Block header
    pub header: BlockHeader,

    /// Transactions in this block
    pub transactions: Vec<Transaction>,

    /// Exact fee charged per transaction at execution time.
    ///
    /// This is used for deterministic fee distribution and exact reorg rollback.
    /// Legacy blocks may not contain this field; in that case runtime falls back
    /// to deterministic recomputation.
    #[serde(default)]
    pub tx_fees_paid: Vec<u64>,

    /// Oracle price data included by the block producer.
    ///
    /// Each entry: (asset_symbol, price_microcents) where price_microcents =
    /// USD price × 1_000_000 (6 decimal precision).
    ///
    /// All validators apply these prices deterministically during
    /// `apply_block_effects`, ensuring oracle data is consensus-propagated
    /// rather than independently fetched. This prevents state divergence
    /// when the DEX WASM reads oracle price bands during order execution.
    ///
    /// Legacy blocks without this field deserialize to an empty vec,
    /// meaning no oracle update for that block (backward compatible).
    #[serde(default)]
    pub oracle_prices: Vec<(String, u64)>,
}

impl Block {
    /// Create genesis block (slot 0)
    pub fn genesis(state_root: Hash, timestamp: u64, transactions: Vec<Transaction>) -> Self {
        let tx_root = compute_tx_root(&transactions);
        Block {
            header: BlockHeader {
                slot: 0,
                parent_hash: Hash::default(),
                state_root,
                tx_root,
                timestamp,
                validator: [0u8; 32],
                signature: [0u8; 64],
            },
            transactions,
            tx_fees_paid: Vec::new(),
            oracle_prices: Vec::new(),
        }
    }

    /// Create new block with explicit timestamp (deterministic across validators)
    pub fn new(
        slot: u64,
        parent_hash: Hash,
        state_root: Hash,
        validator: [u8; 32],
        transactions: Vec<Transaction>,
    ) -> Self {
        let tx_root = compute_tx_root(&transactions);
        Block {
            header: BlockHeader {
                slot,
                parent_hash,
                state_root,
                tx_root,
                timestamp: current_timestamp(),
                validator,
                signature: [0u8; 64],
            },
            transactions,
            tx_fees_paid: Vec::new(),
            oracle_prices: Vec::new(),
        }
    }

    /// Create new block with explicit timestamp (preferred — deterministic)
    pub fn new_with_timestamp(
        slot: u64,
        parent_hash: Hash,
        state_root: Hash,
        validator: [u8; 32],
        transactions: Vec<Transaction>,
        timestamp: u64,
    ) -> Self {
        let tx_root = compute_tx_root(&transactions);
        Block {
            header: BlockHeader {
                slot,
                parent_hash,
                state_root,
                tx_root,
                timestamp,
                validator,
                signature: [0u8; 64],
            },
            transactions,
            tx_fees_paid: Vec::new(),
            oracle_prices: Vec::new(),
        }
    }

    /// Get the signable hash (hash of header fields excluding the signature)
    pub fn signable_hash(&self) -> Hash {
        // Serialize only the fields that are signed (everything except signature)
        let mut data = Vec::new();
        data.extend_from_slice(&self.header.slot.to_le_bytes());
        data.extend_from_slice(&self.header.parent_hash.0);
        data.extend_from_slice(&self.header.state_root.0);
        data.extend_from_slice(&self.header.tx_root.0);
        data.extend_from_slice(&self.header.timestamp.to_le_bytes());
        data.extend_from_slice(&self.header.validator);
        Hash::hash(&data)
    }

    /// Sign the block with the validator's keypair
    pub fn sign(&mut self, keypair: &crate::account::Keypair) {
        let hash = self.signable_hash();
        self.header.signature = keypair.sign(&hash.0);
    }

    /// Verify the block signature against the validator public key.
    /// T1.6 fix: Zero/unsigned signatures are now REJECTED.
    /// Only the genesis block (slot 0) may be unsigned.
    pub fn verify_signature(&self) -> bool {
        if self.header.signature.iter().all(|&b| b == 0) {
            // Only allow unsigned genesis block (slot 0)
            return self.header.slot == 0;
        }
        let validator_pubkey = crate::account::Pubkey(self.header.validator);
        let hash = self.signable_hash();
        crate::account::Keypair::verify(&validator_pubkey, &hash.0, &self.header.signature)
    }

    /// Get block hash — uses signable_hash so the hash is stable before/after signing.
    /// T3.5 fix: Block hash no longer includes the signature field.
    pub fn hash(&self) -> Hash {
        self.signable_hash()
    }
}

fn compute_tx_root(transactions: &[Transaction]) -> Hash {
    if transactions.is_empty() {
        return Hash::default();
    }

    let mut data = Vec::with_capacity(transactions.len() * 32);
    for tx in transactions {
        data.extend_from_slice(&tx.hash().0);
    }

    Hash::hash(&data)
}

/// Get current Unix timestamp (wall clock — only used as fallback)
fn current_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// AUDIT-FIX A2-01: Derive deterministic block timestamp from slot number.
/// All validators produce the same timestamp for a given slot:
///   `genesis_time_secs + (slot * slot_duration_ms / 1000)`
/// NOTE: Production now uses wall-clock timestamps; this is retained for tests.
#[allow(dead_code)]
pub fn derive_slot_timestamp(genesis_time_secs: u64, slot: u64, slot_duration_ms: u64) -> u64 {
    genesis_time_secs + (slot * slot_duration_ms / 1000)
}

/// AUDIT-FIX A2-01: Check if a block's timestamp is within the allowed window
/// of the expected slot-derived timestamp.
/// Returns Ok(()) if timestamp is within `max_drift_secs`, Err with drift otherwise.
/// NOTE: Production now uses wall-clock future-only validation; retained for tests.
#[allow(dead_code)]
pub fn validate_timestamp(
    block_timestamp: u64,
    genesis_time_secs: u64,
    slot: u64,
    slot_duration_ms: u64,
    max_drift_secs: u64,
) -> Result<(), u64> {
    let expected = derive_slot_timestamp(genesis_time_secs, slot, slot_duration_ms);
    let drift = block_timestamp.abs_diff(expected);
    if drift > max_drift_secs {
        Err(drift)
    } else {
        Ok(())
    }
}

impl Block {
    /// AUDIT-FIX A2-01: Derive deterministic block timestamp from slot number (associated fn).
    pub fn derive_slot_timestamp(genesis_time_secs: u64, slot: u64, slot_duration_ms: u64) -> u64 {
        derive_slot_timestamp(genesis_time_secs, slot, slot_duration_ms)
    }

    /// AUDIT-FIX A2-01: Validate block timestamp against expected (associated fn).
    pub fn validate_timestamp(
        block_timestamp: u64,
        genesis_time_secs: u64,
        slot: u64,
        slot_duration_ms: u64,
        max_drift_secs: u64,
    ) -> Result<(), u64> {
        validate_timestamp(
            block_timestamp,
            genesis_time_secs,
            slot,
            slot_duration_ms,
            max_drift_secs,
        )
    }

    /// Validate block structure: size limits, tx count, etc. (T1.7)
    pub fn validate_structure(&self) -> Result<(), String> {
        if self.transactions.len() > MAX_TX_PER_BLOCK {
            return Err(format!(
                "Block contains {} transactions (max {})",
                self.transactions.len(),
                MAX_TX_PER_BLOCK
            ));
        }

        // Validate each transaction's structure
        for (i, tx) in self.transactions.iter().enumerate() {
            if let Err(e) = tx.validate_structure() {
                return Err(format!("Transaction {} invalid: {}", i, e));
            }
        }

        // Check serialized size estimate (header + all txs)
        let estimated_size = self
            .transactions
            .iter()
            .map(|tx| {
                tx.message
                    .instructions
                    .iter()
                    .map(|ix| 32 + 8 + ix.data.len() + ix.accounts.len() * 32)
                    .sum::<usize>()
                    + tx.signatures.len() * 64
                    + 32
            })
            .sum::<usize>()
            + 256; // 256 bytes for header overhead

        if estimated_size > MAX_BLOCK_SIZE {
            return Err(format!(
                "Block too large: ~{} bytes (max {})",
                estimated_size, MAX_BLOCK_SIZE
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genesis_block() {
        let genesis = Block::genesis(Hash::hash(b"genesis_state"), 1, Vec::new());

        assert_eq!(genesis.header.slot, 0);
        assert_eq!(genesis.header.parent_hash, Hash::default());
        assert_eq!(genesis.transactions.len(), 0);

        println!("Genesis block hash: {}", genesis.hash());
    }

    #[test]
    fn test_block_creation() {
        let parent = Hash::hash(b"parent_block");
        let state = Hash::hash(b"current_state");
        let validator = [42u8; 32];

        let block = Block::new(1, parent, state, validator, Vec::new());

        assert_eq!(block.header.slot, 1);
        assert_eq!(block.header.parent_hash, parent);

        println!("Block 1 hash: {}", block.hash());
    }

    #[test]
    fn test_block_sign_and_verify() {
        use crate::Keypair;

        let kp = Keypair::generate();
        let validator_bytes = kp.pubkey().0;

        let mut block = Block::new_with_timestamp(
            1,
            Hash::default(),
            Hash::hash(b"state"),
            validator_bytes,
            Vec::new(),
            1000,
        );

        // Unsigned non-genesis block (slot != 0) should NOT verify (T1.6)
        assert!(
            !block.verify_signature(),
            "Unsigned non-genesis block must be rejected"
        );

        // Sign the block
        block.sign(&kp);
        assert_ne!(block.header.signature, [0u8; 64]);

        // Signed block should verify
        assert!(block.verify_signature());

        // Tamper with timestamp — verification should fail
        block.header.timestamp += 1;
        assert!(!block.verify_signature());
    }

    #[test]
    fn test_block_serde_with_signature() {
        use crate::Keypair;

        let kp = Keypair::generate();
        let mut block = Block::new_with_timestamp(
            5,
            Hash::default(),
            Hash::hash(b"state"),
            kp.pubkey().0,
            Vec::new(),
            2000,
        );
        block.sign(&kp);

        // Serialize then deserialize
        let json = serde_json::to_string(&block).unwrap();
        let deserialized: Block = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.header.signature, block.header.signature);
        assert!(deserialized.verify_signature());
    }

    #[test]
    fn test_block_serde_backward_compat() {
        // A BlockHeader without a "signature" field should deserialize with zero signature
        let header = BlockHeader {
            slot: 0,
            parent_hash: Hash::default(),
            state_root: Hash::default(),
            tx_root: Hash::default(),
            timestamp: 0,
            validator: [0u8; 32],
            signature: [0u8; 64],
        };
        // Serialize, strip signature, then deserialize
        let mut json_val: serde_json::Value = serde_json::to_value(&header).unwrap();
        json_val.as_object_mut().unwrap().remove("signature");
        let json = json_val.to_string();
        let deserialized: BlockHeader = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.signature, [0u8; 64]);
    }

    // ─── Block structure validation tests (T1.7) ─────────────────────

    #[test]
    fn test_validate_structure_empty_block_passes() {
        let block = Block::new(0, Hash::default(), Hash::default(), [0u8; 32], Vec::new());
        assert!(block.validate_structure().is_ok());
    }

    #[test]
    fn test_validate_structure_too_many_txs_rejected() {
        use crate::transaction::{Instruction, Message, Transaction};

        // Create a block with more than MAX_TX_PER_BLOCK transactions
        let mut txs = Vec::with_capacity(MAX_TX_PER_BLOCK + 1);
        for _ in 0..=MAX_TX_PER_BLOCK {
            let ix = Instruction {
                program_id: crate::Pubkey([0u8; 32]),
                accounts: vec![crate::Pubkey([1u8; 32])],
                data: vec![0u8],
            };
            let msg = Message::new(vec![ix], Hash::default());
            txs.push(Transaction::new(msg));
        }

        let block = Block::new(1, Hash::default(), Hash::default(), [0u8; 32], txs);
        let result = block.validate_structure();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("transactions"));
    }

    #[test]
    fn test_validate_structure_valid_block_passes() {
        use crate::transaction::{Instruction, Message, Transaction};

        let mut txs = Vec::new();
        for _ in 0..5 {
            let ix = Instruction {
                program_id: crate::Pubkey([0u8; 32]),
                accounts: vec![crate::Pubkey([1u8; 32])],
                data: vec![0u8; 32],
            };
            let msg = Message::new(vec![ix], Hash::default());
            txs.push(Transaction::new(msg));
        }

        let block = Block::new(1, Hash::default(), Hash::default(), [0u8; 32], txs);
        assert!(block.validate_structure().is_ok());
    }

    #[test]
    fn test_validate_structure_oversized_instruction_rejected() {
        use crate::transaction::{Instruction, Message, Transaction, MAX_INSTRUCTION_DATA};

        let ix = Instruction {
            program_id: crate::Pubkey([0u8; 32]),
            accounts: vec![crate::Pubkey([1u8; 32])],
            data: vec![0u8; MAX_INSTRUCTION_DATA + 1],
        };
        let msg = Message::new(vec![ix], Hash::default());
        let tx = Transaction::new(msg);
        let block = Block::new(1, Hash::default(), Hash::default(), [0u8; 32], vec![tx]);

        let result = block.validate_structure();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("data too large"));
    }

    // ── AUDIT-FIX A2-01: Deterministic timestamp tests ──

    #[test]
    fn test_derive_slot_timestamp_basic() {
        // genesis_time = 1700000000, slot_duration = 400ms
        let genesis = 1_700_000_000u64;
        let slot_ms = 400u64;

        assert_eq!(derive_slot_timestamp(genesis, 0, slot_ms), genesis);
        // slot 1: +0.4s → +0s (integer division)
        assert_eq!(derive_slot_timestamp(genesis, 1, slot_ms), genesis);
        // slot 2: 2*400/1000 = 0 → still genesis (sub-second)
        assert_eq!(derive_slot_timestamp(genesis, 2, slot_ms), genesis);
        // slot 3: 3*400/1000 = 1 → genesis + 1
        assert_eq!(derive_slot_timestamp(genesis, 3, slot_ms), genesis + 1);
        // slot 2500: 2500*400/1000 = 1000 → genesis + 1000
        assert_eq!(
            derive_slot_timestamp(genesis, 2500, slot_ms),
            genesis + 1000
        );
    }

    #[test]
    fn test_derive_slot_timestamp_deterministic() {
        // Two calls with same inputs produce identical results
        let genesis = 1_700_000_000u64;
        let ts1 = derive_slot_timestamp(genesis, 100, 400);
        let ts2 = derive_slot_timestamp(genesis, 100, 400);
        assert_eq!(ts1, ts2, "Must be deterministic");
    }

    #[test]
    fn test_derive_slot_timestamp_monotonic() {
        let genesis = 1_700_000_000u64;
        let slot_ms = 400u64;
        let mut prev = 0u64;
        for slot in 0..10000 {
            let ts = derive_slot_timestamp(genesis, slot, slot_ms);
            assert!(ts >= prev, "Timestamp must be monotonically non-decreasing");
            prev = ts;
        }
    }

    #[test]
    fn test_validate_timestamp_within_window() {
        let genesis = 1_700_000_000u64;
        let slot_ms = 400u64;
        let slot = 2500u64; // expected = genesis + 1000

        // Exact match
        assert!(validate_timestamp(genesis + 1000, genesis, slot, slot_ms, 60).is_ok());
        // +59 seconds (within window)
        assert!(validate_timestamp(genesis + 1059, genesis, slot, slot_ms, 60).is_ok());
        // -30 seconds (within window)
        assert!(validate_timestamp(genesis + 970, genesis, slot, slot_ms, 60).is_ok());
    }

    #[test]
    fn test_validate_timestamp_outside_window() {
        let genesis = 1_700_000_000u64;
        let slot_ms = 400u64;
        let slot = 2500u64; // expected = genesis + 1000

        // +61 seconds (outside window)
        let result = validate_timestamp(genesis + 1061, genesis, slot, slot_ms, 60);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), 61);

        // -100 seconds (outside window)
        let result = validate_timestamp(genesis + 900, genesis, slot, slot_ms, 60);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), 100);
    }

    #[test]
    fn test_new_with_timestamp_uses_provided_value() {
        let ts = 1_700_001_000u64;
        let block =
            Block::new_with_timestamp(100, Hash::default(), Hash::default(), [0u8; 32], vec![], ts);
        assert_eq!(block.header.timestamp, ts);
    }
}
