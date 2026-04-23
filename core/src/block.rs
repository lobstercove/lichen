// Lichen Core - Block Structure

use crate::hash::Hash;
use crate::transaction::Transaction;
use crate::PqSignature;
use serde::{Deserialize, Serialize};

/// Maximum block size in bytes (serialized) — 10 MB
pub const MAX_BLOCK_SIZE: usize = 10 * 1024 * 1024;

/// Maximum transactions per block
pub const MAX_TX_PER_BLOCK: usize = 10_000;

/// Maximum deployable WASM contract code size — 512 KB.
pub const MAX_CONTRACT_CODE: usize = 512 * 1024;

/// Custom serde for [u8; 32] (validator pubkeys in commit signatures)
mod pubkey_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(key: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        key.as_slice().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 32], D::Error> {
        let v: Vec<u8> = Vec::deserialize(d)?;
        let arr: [u8; 32] = v
            .try_into()
            .map_err(|_| serde::de::Error::custom("expected 32 bytes for pubkey"))?;
        Ok(arr)
    }
}

/// A validator's precommit signature included in the block's commit certificate.
///
/// After 2/3+ stake-weighted precommits are collected for a block, their
/// signatures are bundled into the block so any node (including light clients)
/// can verify finality without replaying consensus.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommitSignature {
    /// Validator address recorded in the commit certificate.
    #[serde(with = "pubkey_serde")]
    pub validator: [u8; 32],
    /// Self-contained PQ signature over `(0x02 || height || round || block_hash || timestamp)`.
    pub signature: PqSignature,
    /// Validator's wall-clock timestamp when casting the precommit vote.
    /// Used to compute BFT Time (weighted median) for deterministic block timestamps.
    #[serde(default)]
    pub timestamp: u64,
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

    /// Hash of the active validator set for this block's height.
    /// Enables light clients to verify which validator set signed the block
    /// without replaying full state. Computed as SHA-256 of the sorted
    /// validator pubkeys and their stakes.
    ///
    /// Legacy blocks (before this field was added) will have the default
    /// zero hash via `#[serde(default)]`.
    #[serde(default)]
    pub validators_hash: Hash,

    /// Validator that produced this block
    pub validator: [u8; 32],

    /// Optional PQ signature of the block producer over the header fields.
    #[serde(default)]
    pub signature: Option<PqSignature>,
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
    /// This is retained as producer-supplied metadata for backward compatibility
    /// and legacy/header-only sync paths. Canonical fee accounting should prefer
    /// locally derived execution receipts when available.
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

    /// Consensus round that produced the commit certificate for this block.
    /// This makes commit verification self-contained: verifiers no longer
    /// need to guess which round the 2/3 precommit quorum used.
    pub commit_round: u32,

    /// Commit certificate: precommit signatures from 2/3+ of stake that
    /// finalized this block. Each entry contains the validator pubkey and
    /// their PQ signature over `(0x02 || height || round || block_hash || timestamp)`.
    ///
    /// Light clients verify finality by checking these signatures sum to
    /// ≥2/3 of the total stake. Genesis block (slot 0) has no commit.
    ///
    /// Legacy blocks without this field deserialize to an empty vec.
    #[serde(default)]
    pub commit_signatures: Vec<CommitSignature>,
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
                validators_hash: Hash::default(),
                validator: [0u8; 32],
                signature: None,
            },
            transactions,
            tx_fees_paid: Vec::new(),
            oracle_prices: Vec::new(),
            commit_round: 0,
            commit_signatures: Vec::new(),
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
                validators_hash: Hash::default(),
                validator,
                signature: None,
            },
            transactions,
            tx_fees_paid: Vec::new(),
            oracle_prices: Vec::new(),
            commit_round: 0,
            commit_signatures: Vec::new(),
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
                validators_hash: Hash::default(),
                validator,
                signature: None,
            },
            transactions,
            tx_fees_paid: Vec::new(),
            oracle_prices: Vec::new(),
            commit_round: 0,
            commit_signatures: Vec::new(),
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
        data.extend_from_slice(&self.header.validators_hash.0);
        data.extend_from_slice(&self.header.validator);
        Hash::hash(&data)
    }

    /// Sign the block with the validator's keypair
    pub fn sign(&mut self, keypair: &crate::account::Keypair) {
        let hash = self.signable_hash();
        self.header.signature = Some(keypair.sign(&hash.0));
    }

    /// Verify the block signature against the validator public key.
    /// Only the genesis block (slot 0) may be unsigned.
    pub fn verify_signature(&self) -> bool {
        let validator_pubkey = crate::account::Pubkey(self.header.validator);
        let hash = self.signable_hash();
        match &self.header.signature {
            Some(signature) => {
                crate::account::Keypair::verify(&validator_pubkey, &hash.0, signature)
            }
            None => self.header.slot == 0,
        }
    }

    /// Get block hash — uses signable_hash so the hash is stable before/after signing.
    /// T3.5 fix: Block hash no longer includes the signature field.
    pub fn hash(&self) -> Hash {
        self.signable_hash()
    }

    /// Verify the block's commit certificate against a validator set and stake pool.
    ///
    /// Returns `Ok(())` if the commit signatures represent ≥2/3 of the total
    /// eligible stake. Genesis block (slot 0) always passes (no commit required).
    ///
    /// Each signature is verified as a self-contained PQ signature over `Precommit::signable_bytes`
    /// (tag 0x02 || height || round || block_hash). Duplicate validators or
    /// validators not in the set are silently skipped.
    pub fn verify_commit(
        &self,
        validator_set: &crate::consensus::ValidatorSet,
        stake_pool: &crate::consensus::StakePool,
    ) -> Result<(), String> {
        // Genesis block has no commit
        if self.header.slot == 0 {
            return Ok(());
        }

        if self.commit_signatures.is_empty() {
            return Err("Block has no commit signatures".to_string());
        }

        let block_hash = self.hash();
        // NOTE: Each CommitSignature carries its own timestamp, so signable_bytes
        // must be computed per-signature (not once for the whole block).

        let mut committed_stake: u128 = 0;
        let mut total_stake: u128 = 0;
        let mut seen = std::collections::HashSet::new();

        for vi in validator_set.validators() {
            // Only count active (non-pending) validators toward the 2/3
            // supermajority denominator.  Pending validators have been
            // discovered on-chain but are not yet participating in BFT
            // consensus, so their stake must not inflate total_stake.
            if vi.pending_activation {
                continue;
            }
            let pubkey = vi.pubkey;
            let stake = stake_pool.get_stake(&pubkey).map(|s| s.amount).unwrap_or(0);
            total_stake += stake as u128;
        }

        if total_stake == 0 {
            return Err("No staked validators in set".to_string());
        }

        for cs in &self.commit_signatures {
            let pubkey = crate::Pubkey(cs.validator);

            // Skip duplicates
            if !seen.insert(cs.validator) {
                continue;
            }

            // Skip validators not in the set or still pending activation
            match validator_set.get_validator(&pubkey) {
                Some(vi) if !vi.pending_activation => {}
                _ => continue,
            }

            // Verify signature — each precommit includes its own timestamp
            let signable = crate::consensus::Precommit::signable_bytes(
                self.header.slot,
                self.commit_round,
                &Some(block_hash),
                cs.timestamp,
            );
            if !crate::Keypair::verify(&pubkey, &signable, &cs.signature) {
                continue;
            }

            let stake = stake_pool.get_stake(&pubkey).map(|s| s.amount).unwrap_or(0);
            committed_stake += stake as u128;
        }

        // Check 2/3+ supermajority: committed_stake * 3 >= total_stake * 2
        if committed_stake * 3 >= total_stake * 2 {
            Ok(())
        } else {
            Err(format!(
                "Insufficient commit stake: {} / {} (need 2/3+)",
                committed_stake, total_stake
            ))
        }
    }
}

/// Compute BFT Time: stake-weighted median of precommit timestamps.
///
/// Matches CometBFT behavior: the block timestamp is the weighted median of
/// the commit vote timestamps, where each vote is weighted by the validator's
/// stake. This ensures that no single validator (even the block proposer) can
/// manipulate the block timestamp unilaterally.
///
/// If `min_timestamp` is provided (typically parent block's timestamp),
/// the result is clamped to be at least `min_timestamp + 1` to guarantee
/// strict monotonic increase.
///
/// Returns `None` if there are no commit signatures (genesis block).
pub fn compute_bft_timestamp(
    commit_signatures: &[CommitSignature],
    validator_set: &crate::consensus::ValidatorSet,
    stake_pool: &crate::consensus::StakePool,
    min_timestamp: Option<u64>,
) -> Option<u64> {
    if commit_signatures.is_empty() {
        return None;
    }

    // Collect (timestamp, stake) pairs for valid commit voters
    let mut weighted: Vec<(u64, u64)> = commit_signatures
        .iter()
        .filter(|cs| {
            let pubkey = crate::Pubkey(cs.validator);
            validator_set.get_validator(&pubkey).is_some()
        })
        .map(|cs| {
            let pubkey = crate::Pubkey(cs.validator);
            let stake = stake_pool.get_stake(&pubkey).map(|s| s.amount).unwrap_or(0);
            (cs.timestamp, stake)
        })
        .filter(|(_, stake)| *stake > 0)
        .collect();

    if weighted.is_empty() {
        return None;
    }

    // Sort by timestamp ascending
    weighted.sort_by_key(|(ts, _)| *ts);

    // Find the weighted median: the timestamp where cumulative stake reaches 50%+
    let total_stake: u128 = weighted.iter().map(|(_, s)| *s as u128).sum();
    let half = total_stake / 2;
    let mut cumulative: u128 = 0;
    let mut median_ts = weighted[0].0;

    for (ts, stake) in &weighted {
        cumulative += *stake as u128;
        if cumulative > half {
            median_ts = *ts;
            break;
        }
    }

    // Enforce monotonicity: BFT time must be > parent time
    if let Some(min_ts) = min_timestamp {
        if median_ts <= min_ts {
            median_ts = min_ts + 1;
        }
    }

    Some(median_ts)
}

/// Compute a deterministic hash of the validator set and their stakes.
///
/// The hash is SHA-256 over the sorted (pubkey, stake) pairs:
///   SHA256(pubkey_1 || stake_1_le64 || pubkey_2 || stake_2_le64 || ...)
///
/// This is included in the block header so light clients can verify which
/// validator set was active when the block was committed.
pub fn compute_validators_hash(
    validator_set: &crate::consensus::ValidatorSet,
    stake_pool: &crate::consensus::StakePool,
) -> Hash {
    let mut sorted: Vec<_> = validator_set
        .sorted_validators()
        .iter()
        .map(|vi| {
            let stake = stake_pool
                .get_stake(&vi.pubkey)
                .map(|s| s.total_stake())
                .unwrap_or(0);
            (vi.pubkey, stake)
        })
        .collect();
    // Sort by pubkey bytes for determinism
    sorted.sort_by_key(|a| a.0 .0);

    let mut data = Vec::with_capacity(sorted.len() * 40);
    for (pk, stake) in &sorted {
        data.extend_from_slice(&pk.0);
        data.extend_from_slice(&stake.to_le_bytes());
    }
    Hash::hash(&data)
}

/// Domain-separated leaf hash: SHA256(0x00 || tx_hash)
/// Prefix 0x00 prevents second-preimage attacks against the Merkle tree.
fn merkle_leaf(tx_hash: &Hash) -> Hash {
    let mut data = [0u8; 33];
    data[0] = 0x00;
    data[1..].copy_from_slice(&tx_hash.0);
    Hash::hash(&data)
}

/// Domain-separated internal node: SHA256(0x01 || left || right)
fn merkle_node(left: &Hash, right: &Hash) -> Hash {
    let mut data = [0u8; 65];
    data[0] = 0x01;
    data[1..33].copy_from_slice(&left.0);
    data[33..].copy_from_slice(&right.0);
    Hash::hash(&data)
}

/// Compute binary Merkle root of the block's transactions.
///
/// - Empty block → `Hash::default()` (32 zero bytes)
/// - Leaf = `SHA256(0x00 || tx_hash)` (domain-separated)
/// - Internal = `SHA256(0x01 || left || right)`
/// - Odd leaf count: duplicate the last leaf (standard Bitcoin-style padding)
fn compute_tx_root(transactions: &[Transaction]) -> Hash {
    if transactions.is_empty() {
        return Hash::default();
    }
    let mut level: Vec<Hash> = transactions
        .iter()
        .map(|tx| merkle_leaf(&tx.hash()))
        .collect();
    while level.len() > 1 {
        if level.len() % 2 == 1 {
            let last = *level.last().unwrap();
            level.push(last);
        }
        level = level
            .chunks(2)
            .map(|pair| merkle_node(&pair[0], &pair[1]))
            .collect();
    }
    level[0]
}

/// Build binary Merkle root from pre-computed transaction hashes.
/// Used by compact block sync where we already have tx hashes.
pub fn merkle_tx_root_from_hashes(tx_hashes: &[Hash]) -> Hash {
    if tx_hashes.is_empty() {
        return Hash::default();
    }
    let mut level: Vec<Hash> = tx_hashes.iter().map(merkle_leaf).collect();
    while level.len() > 1 {
        if level.len() % 2 == 1 {
            let last = *level.last().unwrap();
            level.push(last);
        }
        level = level
            .chunks(2)
            .map(|pair| merkle_node(&pair[0], &pair[1]))
            .collect();
    }
    level[0]
}

/// Generate a Merkle inclusion proof for the transaction at `index`.
///
/// Returns a list of `(sibling_hash, is_left)` pairs from leaf to root.
/// `is_left == true` means the sibling is on the LEFT side (i.e., the
/// current node is on the right).
///
/// Returns `None` if `index >= transactions.len()` or the block is empty.
pub fn merkle_tx_proof(transactions: &[Transaction], index: usize) -> Option<Vec<(Hash, bool)>> {
    if transactions.is_empty() || index >= transactions.len() {
        return None;
    }
    let mut level: Vec<Hash> = transactions
        .iter()
        .map(|tx| merkle_leaf(&tx.hash()))
        .collect();
    let mut proof = Vec::new();
    let mut idx = index;

    while level.len() > 1 {
        if level.len() % 2 == 1 {
            let last = *level.last().unwrap();
            level.push(last);
        }
        let sibling_idx = idx ^ 1; // flip last bit to get sibling
        let is_left = sibling_idx < idx; // sibling is on the left if its index is smaller
        proof.push((level[sibling_idx], is_left));
        idx /= 2;
        level = level
            .chunks(2)
            .map(|pair| merkle_node(&pair[0], &pair[1]))
            .collect();
    }
    Some(proof)
}

/// Verify a Merkle inclusion proof for a transaction hash against a root.
pub fn verify_merkle_tx_proof(root: &Hash, tx_hash: &Hash, proof: &[(Hash, bool)]) -> bool {
    let mut current = merkle_leaf(tx_hash);
    for (sibling, is_left) in proof {
        current = if *is_left {
            merkle_node(sibling, &current)
        } else {
            merkle_node(&current, sibling)
        };
    }
    current == *root
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
pub fn derive_slot_timestamp(genesis_time_secs: u64, slot: u64, slot_duration_ms: u64) -> u64 {
    genesis_time_secs + (slot * slot_duration_ms / 1000)
}

/// AUDIT-FIX A2-01: Check if a block's timestamp is within the allowed window
/// of the expected slot-derived timestamp.
/// Returns Ok(()) if timestamp is within `max_drift_secs`, Err with drift otherwise.
/// NOTE: Production now uses wall-clock future-only validation; retained for tests.
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

    fn test_signature(fill: u8) -> crate::PqSignature {
        crate::PqSignature::test_fixture(fill)
    }

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
        assert!(block.header.signature.is_some());

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
    fn test_block_signature_covers_validators_hash() {
        use crate::Keypair;

        let kp = Keypair::generate();
        let mut block = Block::new_with_timestamp(
            7,
            Hash::default(),
            Hash::hash(b"state"),
            kp.pubkey().0,
            Vec::new(),
            3000,
        );
        block.header.validators_hash = Hash::hash(b"validator-set-a");
        block.sign(&kp);

        assert!(block.verify_signature());

        block.header.validators_hash = Hash::hash(b"validator-set-b");
        assert!(
            !block.verify_signature(),
            "tampering validators_hash must invalidate the block signature"
        );
    }

    #[test]
    fn test_compute_validators_hash_ignores_cached_stake_without_pool_entry() {
        let mut cached_set = crate::consensus::ValidatorSet::new();
        let mut zero_set = crate::consensus::ValidatorSet::new();
        let pool = crate::consensus::StakePool::new();
        let pk = crate::Pubkey::new([7u8; 32]);

        cached_set.add_validator(crate::consensus::ValidatorInfo::new(
            pk,
            crate::consensus::MIN_VALIDATOR_STAKE,
        ));
        zero_set.add_validator(crate::consensus::ValidatorInfo::new(pk, 0));

        assert_eq!(
            compute_validators_hash(&cached_set, &pool),
            compute_validators_hash(&zero_set, &pool),
            "missing stake-pool entries must hash as zero effective stake"
        );
    }

    #[test]
    fn test_block_serde_backward_compat() {
        // A BlockHeader without a "signature" field should deserialize as unsigned.
        let header = BlockHeader {
            slot: 0,
            parent_hash: Hash::default(),
            state_root: Hash::default(),
            tx_root: Hash::default(),
            timestamp: 0,
            validators_hash: Hash::default(),
            validator: [0u8; 32],
            signature: None,
        };
        // Serialize, strip signature, then deserialize
        let mut json_val: serde_json::Value = serde_json::to_value(&header).unwrap();
        json_val.as_object_mut().unwrap().remove("signature");
        let json = json_val.to_string();
        let deserialized: BlockHeader = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.signature, None);
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

    // ─── Commit certificate tests (Task 1.2) ────────────────────────

    #[test]
    fn test_commit_signature_serde_roundtrip() {
        let cs = CommitSignature {
            validator: [42u8; 32],
            signature: test_signature(17),
            timestamp: 1000,
        };
        let json = serde_json::to_string(&cs).unwrap();
        let deserialized: CommitSignature = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.validator, cs.validator);
        assert_eq!(deserialized.signature, cs.signature);
        assert_eq!(deserialized.timestamp, cs.timestamp);
    }

    #[test]
    fn test_block_commit_signatures_default_empty() {
        // Legacy blocks without commit_signatures should deserialize with empty vec
        let block = Block::genesis(Hash::hash(b"state"), 1, Vec::new());
        let mut json_val: serde_json::Value = serde_json::to_value(&block).unwrap();
        json_val
            .as_object_mut()
            .unwrap()
            .remove("commit_signatures");
        let json = json_val.to_string();
        let deserialized: Block = serde_json::from_str(&json).unwrap();
        assert!(deserialized.commit_signatures.is_empty());
    }

    #[test]
    fn test_block_with_commit_signatures_serde() {
        let kp = crate::Keypair::generate();
        let mut block = Block::new_with_timestamp(
            5,
            Hash::default(),
            Hash::hash(b"state"),
            kp.pubkey().0,
            Vec::new(),
            2000,
        );
        block.sign(&kp);

        // Add fake commit signatures
        block.commit_signatures = vec![
            CommitSignature {
                validator: [1u8; 32],
                signature: test_signature(2),
                timestamp: 2000,
            },
            CommitSignature {
                validator: [3u8; 32],
                signature: test_signature(4),
                timestamp: 2001,
            },
        ];

        let json = serde_json::to_string(&block).unwrap();
        let deserialized: Block = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.commit_signatures.len(), 2);
        assert_eq!(deserialized.commit_signatures[0].validator, [1u8; 32]);
        assert_eq!(deserialized.commit_signatures[1].validator, [3u8; 32]);
    }

    #[test]
    fn test_genesis_block_verify_commit_passes() {
        let block = Block::genesis(Hash::hash(b"state"), 1, Vec::new());
        let vs = crate::consensus::ValidatorSet::new();
        let sp = crate::consensus::StakePool::new();
        assert!(block.verify_commit(&vs, &sp).is_ok());
    }

    #[test]
    fn test_verify_commit_empty_signatures_fails() {
        let block = Block::new_with_timestamp(
            5,
            Hash::default(),
            Hash::hash(b"state"),
            [1u8; 32],
            Vec::new(),
            2000,
        );
        let vs = crate::consensus::ValidatorSet::new();
        let sp = crate::consensus::StakePool::new();
        let result = block.verify_commit(&vs, &sp);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no commit signatures"));
    }

    #[test]
    fn test_verify_commit_valid_supermajority() {
        use crate::consensus::{StakePool, ValidatorInfo, ValidatorSet};

        // Create 3 validators with equal stake
        let kp1 = crate::Keypair::generate();
        let kp2 = crate::Keypair::generate();
        let kp3 = crate::Keypair::generate();

        let mut vs = ValidatorSet::new();
        for kp in [&kp1, &kp2, &kp3] {
            let vi = ValidatorInfo {
                pubkey: kp.pubkey(),
                reputation: 100,
                blocks_proposed: 0,
                votes_cast: 0,
                correct_votes: 0,
                stake: 100_000_000_000_000,
                joined_slot: 0,
                last_active_slot: 0,
                commission_rate: 500,
                transactions_processed: 0,
                pending_activation: false,
            };
            vs.add_validator(vi);
        }

        let mut sp = StakePool::new();
        for kp in [&kp1, &kp2, &kp3] {
            sp.stake(kp.pubkey(), 100_000_000_000_000, 0).ok();
        }

        // Create a block at slot 5
        let mut block = Block::new_with_timestamp(
            5,
            Hash::default(),
            Hash::hash(b"state"),
            kp1.pubkey().0,
            Vec::new(),
            2000,
        );
        block.sign(&kp1);

        let block_hash = block.hash();
        let round = 0u32;
        block.commit_round = round;

        // Sign precommits from 2 of 3 validators (2/3+)
        let ts = 2000u64;
        let signable = crate::consensus::Precommit::signable_bytes(5, round, &Some(block_hash), ts);
        let sig1 = kp1.sign(&signable);
        let sig2 = kp2.sign(&signable);

        block.commit_signatures = vec![
            CommitSignature {
                validator: kp1.pubkey().0,
                signature: sig1,
                timestamp: ts,
            },
            CommitSignature {
                validator: kp2.pubkey().0,
                signature: sig2,
                timestamp: ts,
            },
        ];

        assert!(block.verify_commit(&vs, &sp).is_ok());
    }

    #[test]
    fn test_verify_commit_insufficient_stake_fails() {
        use crate::consensus::{StakePool, ValidatorInfo, ValidatorSet};

        let kp1 = crate::Keypair::generate();
        let kp2 = crate::Keypair::generate();
        let kp3 = crate::Keypair::generate();

        let mut vs = ValidatorSet::new();
        for kp in [&kp1, &kp2, &kp3] {
            let vi = ValidatorInfo {
                pubkey: kp.pubkey(),
                reputation: 100,
                blocks_proposed: 0,
                votes_cast: 0,
                correct_votes: 0,
                stake: 100_000_000_000_000,
                joined_slot: 0,
                last_active_slot: 0,
                commission_rate: 500,
                transactions_processed: 0,
                pending_activation: false,
            };
            vs.add_validator(vi);
        }

        let mut sp = StakePool::new();
        for kp in [&kp1, &kp2, &kp3] {
            sp.stake(kp.pubkey(), 100_000_000_000_000, 0).ok();
        }

        let mut block = Block::new_with_timestamp(
            5,
            Hash::default(),
            Hash::hash(b"state"),
            kp1.pubkey().0,
            Vec::new(),
            2000,
        );
        block.sign(&kp1);

        let block_hash = block.hash();
        let round = 0u32;
        block.commit_round = round;

        // Only 1 of 3 validators signed (1/3, need 2/3+)
        let ts = 2000u64;
        let signable = crate::consensus::Precommit::signable_bytes(5, round, &Some(block_hash), ts);
        let sig1 = kp1.sign(&signable);

        block.commit_signatures = vec![CommitSignature {
            validator: kp1.pubkey().0,
            signature: sig1,
            timestamp: ts,
        }];

        let result = block.verify_commit(&vs, &sp);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Insufficient commit stake"));
    }

    #[test]
    fn test_verify_commit_bad_signature_skipped() {
        use crate::consensus::{StakePool, ValidatorInfo, ValidatorSet};

        let kp1 = crate::Keypair::generate();
        let kp2 = crate::Keypair::generate();

        let mut vs = ValidatorSet::new();
        for kp in [&kp1, &kp2] {
            let vi = ValidatorInfo {
                pubkey: kp.pubkey(),
                reputation: 100,
                blocks_proposed: 0,
                votes_cast: 0,
                correct_votes: 0,
                stake: 100_000_000_000_000,
                joined_slot: 0,
                last_active_slot: 0,
                commission_rate: 500,
                transactions_processed: 0,
                pending_activation: false,
            };
            vs.add_validator(vi);
        }

        let mut sp = StakePool::new();
        for kp in [&kp1, &kp2] {
            sp.stake(kp.pubkey(), 100_000_000_000_000, 0).ok();
        }

        let mut block = Block::new_with_timestamp(
            5,
            Hash::default(),
            Hash::hash(b"state"),
            kp1.pubkey().0,
            Vec::new(),
            2000,
        );
        block.sign(&kp1);

        let block_hash = block.hash();
        let round = 0u32;
        block.commit_round = round;

        // kp1 signed correctly, kp2 has garbage signature
        let ts = 2000u64;
        let signable = crate::consensus::Precommit::signable_bytes(5, round, &Some(block_hash), ts);
        let sig1 = kp1.sign(&signable);

        block.commit_signatures = vec![
            CommitSignature {
                validator: kp1.pubkey().0,
                signature: sig1,
                timestamp: ts,
            },
            CommitSignature {
                validator: kp2.pubkey().0,
                signature: test_signature(0xAA),
                timestamp: ts,
            },
        ];

        // Only 1 valid out of 2 = 50%, need 2/3+ → should fail
        let result = block.verify_commit(&vs, &sp);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_commit_wrong_round_fails() {
        use crate::consensus::{StakePool, ValidatorInfo, ValidatorSet};

        let kp1 = crate::Keypair::generate();
        let kp2 = crate::Keypair::generate();

        let mut vs = ValidatorSet::new();
        for kp in [&kp1, &kp2] {
            let vi = ValidatorInfo {
                pubkey: kp.pubkey(),
                reputation: 100,
                blocks_proposed: 0,
                votes_cast: 0,
                correct_votes: 0,
                stake: 100_000_000_000_000,
                joined_slot: 0,
                last_active_slot: 0,
                commission_rate: 500,
                transactions_processed: 0,
                pending_activation: false,
            };
            vs.add_validator(vi);
        }

        let mut sp = StakePool::new();
        for kp in [&kp1, &kp2] {
            sp.stake(kp.pubkey(), 100_000_000_000_000, 0).ok();
        }

        let mut block = Block::new_with_timestamp(
            5,
            Hash::default(),
            Hash::hash(b"state"),
            kp1.pubkey().0,
            Vec::new(),
            2000,
        );
        block.sign(&kp1);
        block.commit_round = 1;

        let ts = 2000u64;
        let signable = crate::consensus::Precommit::signable_bytes(5, 0, &Some(block.hash()), ts);
        block.commit_signatures = vec![
            CommitSignature {
                validator: kp1.pubkey().0,
                signature: kp1.sign(&signable),
                timestamp: ts,
            },
            CommitSignature {
                validator: kp2.pubkey().0,
                signature: kp2.sign(&signable),
                timestamp: ts,
            },
        ];

        let result = block.verify_commit(&vs, &sp);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Insufficient commit stake"));
    }

    // ─── BFT timestamp tests (Task 3.2) ─────────────────────────────

    #[test]
    fn test_bft_timestamp_weighted_median_equal_stake() {
        use crate::consensus::{StakePool, ValidatorInfo, ValidatorSet};

        let mut vs = ValidatorSet::new();
        let mut sp = StakePool::new();
        let keys: Vec<[u8; 32]> = (1..=3u8)
            .map(|i| {
                let mut k = [0u8; 32];
                k[0] = i;
                k
            })
            .collect();

        for k in &keys {
            vs.add_validator(ValidatorInfo {
                pubkey: crate::Pubkey(*k),
                reputation: 100,
                blocks_proposed: 0,
                votes_cast: 0,
                correct_votes: 0,
                stake: 100_000_000_000_000,
                joined_slot: 0,
                last_active_slot: 0,
                commission_rate: 500,
                transactions_processed: 0,
                pending_activation: false,
            });
            sp.stake(crate::Pubkey(*k), 100_000_000_000_000, 0).ok();
        }

        // Timestamps: [1000, 1002, 1004] — median with equal stake = 1002
        let sigs = vec![
            CommitSignature {
                validator: keys[0],
                signature: test_signature(0),
                timestamp: 1000,
            },
            CommitSignature {
                validator: keys[1],
                signature: test_signature(0),
                timestamp: 1002,
            },
            CommitSignature {
                validator: keys[2],
                signature: test_signature(0),
                timestamp: 1004,
            },
        ];

        let result = compute_bft_timestamp(&sigs, &vs, &sp, None);
        assert_eq!(result, Some(1002));
    }

    #[test]
    fn test_bft_timestamp_weighted_median_unequal_stake() {
        use crate::consensus::{StakePool, ValidatorInfo, ValidatorSet};

        let mut vs = ValidatorSet::new();
        let mut sp = StakePool::new();

        // Validator A: 60% stake, ts=1000 | B: 25%, ts=1005 | C: 15%, ts=1010
        let ka = {
            let mut k = [0u8; 32];
            k[0] = 1;
            k
        };
        let kb = {
            let mut k = [0u8; 32];
            k[0] = 2;
            k
        };
        let kc = {
            let mut k = [0u8; 32];
            k[0] = 3;
            k
        };

        // Stakes proportional: 60%, 25%, 15% above MIN_VALIDATOR_STAKE
        let base = 100_000_000_000_000u64; // 100K LICN
        for (k, stake) in [(ka, base * 6), (kb, base * 25 / 10), (kc, base * 15 / 10)] {
            vs.add_validator(ValidatorInfo {
                pubkey: crate::Pubkey(k),
                reputation: 100,
                blocks_proposed: 0,
                votes_cast: 0,
                correct_votes: 0,
                stake,
                joined_slot: 0,
                last_active_slot: 0,
                commission_rate: 500,
                transactions_processed: 0,
                pending_activation: false,
            });
            sp.stake(crate::Pubkey(k), stake, 0).ok();
        }

        let sigs = vec![
            CommitSignature {
                validator: ka,
                signature: test_signature(0),
                timestamp: 1000,
            },
            CommitSignature {
                validator: kb,
                signature: test_signature(0),
                timestamp: 1005,
            },
            CommitSignature {
                validator: kc,
                signature: test_signature(0),
                timestamp: 1010,
            },
        ];

        // Sorted: (1000, 60%), (1005, 25%), (1010, 15%)
        // Cumulative at 1000: 600K/1000K = 60% > 50% → median = 1000
        let result = compute_bft_timestamp(&sigs, &vs, &sp, None);
        assert_eq!(result, Some(1000));
    }

    #[test]
    fn test_bft_timestamp_monotonicity_enforcement() {
        use crate::consensus::{StakePool, ValidatorInfo, ValidatorSet};

        let mut vs = ValidatorSet::new();
        let mut sp = StakePool::new();
        let k = {
            let mut k = [0u8; 32];
            k[0] = 1;
            k
        };

        let stake = 100_000_000_000_000u64; // 100K LICN
        vs.add_validator(ValidatorInfo {
            pubkey: crate::Pubkey(k),
            reputation: 100,
            blocks_proposed: 0,
            votes_cast: 0,
            correct_votes: 0,
            stake,
            joined_slot: 0,
            last_active_slot: 0,
            commission_rate: 500,
            transactions_processed: 0,
            pending_activation: false,
        });
        sp.stake(crate::Pubkey(k), stake, 0).ok();

        let sigs = vec![CommitSignature {
            validator: k,
            signature: test_signature(0),
            timestamp: 500,
        }];

        // Parent timestamp is 1000, BFT median is 500 → clamps to 1001
        let result = compute_bft_timestamp(&sigs, &vs, &sp, Some(1000));
        assert_eq!(result, Some(1001));
    }

    #[test]
    fn test_bft_timestamp_empty_commit_returns_none() {
        let vs = crate::consensus::ValidatorSet::new();
        let sp = crate::consensus::StakePool::new();
        let result = compute_bft_timestamp(&[], &vs, &sp, None);
        assert!(result.is_none());
    }

    #[test]
    fn test_commit_signature_timestamp_serde_default() {
        // Legacy CommitSignature without timestamp field should default to 0.
        let cs = CommitSignature {
            validator: [0u8; 32],
            signature: test_signature(0),
            timestamp: 42,
        };
        let json = serde_json::to_string(&cs).unwrap();
        // Re-parse *without* the timestamp key → should default to 0
        let without_ts = json.replace(",\"timestamp\":42", "");
        let cs2: CommitSignature = serde_json::from_str(&without_ts).unwrap();
        assert_eq!(cs2.timestamp, 0);
    }

    // ─── Merkle tx_root tests (Plan D: PR-02/BS-01) ─────────────────

    fn make_test_txs(count: usize) -> Vec<Transaction> {
        use crate::transaction::{Instruction, Message};
        (0..count)
            .map(|i| {
                let ix = Instruction {
                    program_id: crate::Pubkey([0u8; 32]),
                    accounts: vec![crate::Pubkey([1u8; 32])],
                    data: vec![i as u8],
                };
                Transaction::new(Message::new(vec![ix], Hash::default()))
            })
            .collect()
    }

    #[test]
    fn test_merkle_root_empty_block() {
        let root = compute_tx_root(&[]);
        assert_eq!(root, Hash::default());
    }

    #[test]
    fn test_merkle_root_single_tx() {
        let txs = make_test_txs(1);
        let root = compute_tx_root(&txs);
        // Single tx: root = merkle_node(leaf, leaf) because we duplicate for odd count
        // Actually with 1 element, the loop body doesn't execute since len() == 1
        // so root == merkle_leaf(tx_hash)
        let expected = merkle_leaf(&txs[0].hash());
        assert_eq!(root, expected);
    }

    #[test]
    fn test_merkle_root_two_txs() {
        let txs = make_test_txs(2);
        let root = compute_tx_root(&txs);
        let l0 = merkle_leaf(&txs[0].hash());
        let l1 = merkle_leaf(&txs[1].hash());
        let expected = merkle_node(&l0, &l1);
        assert_eq!(root, expected);
    }

    #[test]
    fn test_merkle_root_three_txs() {
        let txs = make_test_txs(3);
        let root = compute_tx_root(&txs);
        let l0 = merkle_leaf(&txs[0].hash());
        let l1 = merkle_leaf(&txs[1].hash());
        let l2 = merkle_leaf(&txs[2].hash());
        // Odd count: duplicate last → [l0, l1, l2, l2]
        let n01 = merkle_node(&l0, &l1);
        let n22 = merkle_node(&l2, &l2);
        let expected = merkle_node(&n01, &n22);
        assert_eq!(root, expected);
    }

    #[test]
    fn test_merkle_root_seven_txs() {
        let txs = make_test_txs(7);
        let root = compute_tx_root(&txs);
        assert_ne!(root, Hash::default());
        // Verify determinism
        assert_eq!(root, compute_tx_root(&txs));
    }

    #[test]
    fn test_merkle_root_128_txs() {
        let txs = make_test_txs(128);
        let root = compute_tx_root(&txs);
        assert_ne!(root, Hash::default());
        // Power-of-two: no odd-padding needed at any level
        assert_eq!(root, compute_tx_root(&txs));
    }

    #[test]
    fn test_merkle_root_different_tx_sets() {
        let txs_a = make_test_txs(3);
        let txs_b = make_test_txs(4);
        assert_ne!(compute_tx_root(&txs_a), compute_tx_root(&txs_b));
    }

    #[test]
    fn test_merkle_root_from_hashes_matches() {
        let txs = make_test_txs(5);
        let root_direct = compute_tx_root(&txs);
        let hashes: Vec<Hash> = txs.iter().map(|tx| tx.hash()).collect();
        let root_from_hashes = merkle_tx_root_from_hashes(&hashes);
        assert_eq!(root_direct, root_from_hashes);
    }

    #[test]
    fn test_merkle_proof_empty_returns_none() {
        assert!(merkle_tx_proof(&[], 0).is_none());
    }

    #[test]
    fn test_merkle_proof_out_of_bounds_returns_none() {
        let txs = make_test_txs(3);
        assert!(merkle_tx_proof(&txs, 3).is_none());
        assert!(merkle_tx_proof(&txs, 100).is_none());
    }

    #[test]
    fn test_merkle_proof_single_tx_roundtrip() {
        let txs = make_test_txs(1);
        let root = compute_tx_root(&txs);
        let proof = merkle_tx_proof(&txs, 0).unwrap();
        assert!(verify_merkle_tx_proof(&root, &txs[0].hash(), &proof));
    }

    #[test]
    fn test_merkle_proof_two_txs_roundtrip() {
        let txs = make_test_txs(2);
        let root = compute_tx_root(&txs);
        for i in 0..2 {
            let proof = merkle_tx_proof(&txs, i).unwrap();
            assert!(
                verify_merkle_tx_proof(&root, &txs[i].hash(), &proof),
                "Proof verification failed for tx index {i}"
            );
        }
    }

    #[test]
    fn test_merkle_proof_three_txs_roundtrip() {
        let txs = make_test_txs(3);
        let root = compute_tx_root(&txs);
        for i in 0..3 {
            let proof = merkle_tx_proof(&txs, i).unwrap();
            assert!(
                verify_merkle_tx_proof(&root, &txs[i].hash(), &proof),
                "Proof verification failed for tx index {i}"
            );
        }
    }

    #[test]
    fn test_merkle_proof_seven_txs_roundtrip() {
        let txs = make_test_txs(7);
        let root = compute_tx_root(&txs);
        for i in 0..7 {
            let proof = merkle_tx_proof(&txs, i).unwrap();
            assert!(
                verify_merkle_tx_proof(&root, &txs[i].hash(), &proof),
                "Proof verification failed for tx index {i}"
            );
        }
    }

    #[test]
    fn test_merkle_proof_128_txs_roundtrip() {
        let txs = make_test_txs(128);
        let root = compute_tx_root(&txs);
        for i in 0..128 {
            let proof = merkle_tx_proof(&txs, i).unwrap();
            assert!(
                verify_merkle_tx_proof(&root, &txs[i].hash(), &proof),
                "Proof verification failed for tx index {i}"
            );
        }
    }

    #[test]
    fn test_merkle_proof_wrong_hash_fails() {
        let txs = make_test_txs(4);
        let root = compute_tx_root(&txs);
        let proof = merkle_tx_proof(&txs, 0).unwrap();
        let wrong_hash = Hash::hash(b"wrong");
        assert!(!verify_merkle_tx_proof(&root, &wrong_hash, &proof));
    }

    #[test]
    fn test_merkle_proof_wrong_root_fails() {
        let txs = make_test_txs(4);
        let proof = merkle_tx_proof(&txs, 0).unwrap();
        let wrong_root = Hash::hash(b"wrong_root");
        assert!(!verify_merkle_tx_proof(&wrong_root, &txs[0].hash(), &proof));
    }

    #[test]
    fn test_merkle_proof_tampered_sibling_fails() {
        let txs = make_test_txs(4);
        let root = compute_tx_root(&txs);
        let mut proof = merkle_tx_proof(&txs, 0).unwrap();
        if !proof.is_empty() {
            proof[0].0 = Hash::hash(b"tampered");
        }
        assert!(!verify_merkle_tx_proof(&root, &txs[0].hash(), &proof));
    }

    #[test]
    fn test_merkle_domain_separation() {
        // Verify that leaf and internal nodes use different domain tags
        let h = Hash::hash(b"test");
        let leaf = merkle_leaf(&h);
        // An internal node with same data should produce a different hash
        let internal = merkle_node(&h, &h);
        assert_ne!(leaf, internal);
    }
}
