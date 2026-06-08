use rocksdb::Direction;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::*;
use crate::codec::{append_legacy_bincode, deserialize_legacy_bincode, serialize_legacy_bincode};

const STATE_ROOT_PREFIX_WITH_RESTRICTIONS: u8 = 0x03;
const STATE_ROOT_PREFIX_LEGACY: u8 = 0x02;
const STATE_ROOT_PREFIX_SPARSE_WITH_RESTRICTIONS: u8 = 0x13;
const STATE_ROOT_PREFIX_SPARSE_LEGACY: u8 = 0x12;
const STATE_ROOT_PREFIX_SPARSE_SHIELDED_WITH_RESTRICTIONS: u8 = 0x23;
const STATE_ROOT_PREFIX_SPARSE_SHIELDED_LEGACY: u8 = 0x22;
const STATE_ROOT_SCHEMA_KEY: &[u8] = b"state_root_schema";
const STATE_COMMITMENT_SCHEMA_KEY: &[u8] = b"state_commitment_schema";
const CACHED_STATE_COMMITMENT_SCHEMA_KEY: &[u8] = b"cached_state_commitment_schema";
const CACHED_STATE_ROOT_SCHEMA_KEY: &[u8] = b"cached_state_root_schema";
const CACHED_STATE_ROOT_KEY: &[u8] = b"cached_state_root";
const CACHED_ACCOUNTS_ROOT_KEY: &[u8] = b"cached_accounts_root";
const CACHED_CONTRACT_ROOT_KEY: &[u8] = b"cached_contract_root";
const SPARSE_ACCOUNTS_ROOT_KEY: &[u8] = b"sparse_accounts_root";
const SPARSE_CONTRACT_ROOT_KEY: &[u8] = b"sparse_contract_root";
const SPARSE_ACCOUNTS_LEAF_COUNT_KEY: &[u8] = b"sparse_accounts_leaf_count";
const SPARSE_CONTRACT_LEAF_COUNT_KEY: &[u8] = b"sparse_contract_leaf_count";
const SPARSE_STATE_READY_KEY: &[u8] = b"sparse_state_commitment_ready";
const SPARSE_DIRTY_MARKERS_ATOMIC_KEY: &[u8] = b"sparse_dirty_markers_atomic_v1";
const STATE_COMMITMENT_SCHEMA_ORDERED_V0: u8 = 0;
const STATE_COMMITMENT_SCHEMA_SPARSE_V1: u8 = 1;
const STATE_COMMITMENT_SCHEMA_SPARSE_SHIELDED_V2: u8 = 2;
const SPARSE_NODE_DOMAIN_LEAF: u8 = 0xA0;
const SPARSE_NODE_DOMAIN_BRANCH: u8 = 0xA1;
const SHIELDED_STATE_ROOT_PREFIX: u8 = 0x51;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SparseStateCommitmentReport {
    pub before_schema: u8,
    pub after_schema: u8,
    pub active: bool,
    pub last_slot: u64,
    pub current_state_root: Hash,
    pub latest_block_state_root: Option<Hash>,
    pub accounts_root: Hash,
    pub contract_root: Hash,
    pub shielded_root: Hash,
    pub accounts_leaf_count: u64,
    pub contract_leaf_count: u64,
    pub accounts_node_count: u64,
    pub contract_node_count: u64,
    pub activated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SparseNode {
    Leaf {
        path: [u8; 32],
        leaf_hash: Hash,
    },
    Branch {
        prefix_bits: u16,
        prefix: [u8; 32],
        left: Hash,
        right: Hash,
    },
}

#[derive(Debug, Clone)]
struct SparseLeafEntry {
    leaf_key: Vec<u8>,
    path: [u8; 32],
    leaf_hash: Hash,
}

#[derive(Debug, Clone)]
struct SparseLeafChange {
    leaf_key: Vec<u8>,
    path: [u8; 32],
    leaf_hash: Option<Hash>,
}

type SparseNodeOverlay = BTreeMap<[u8; 32], Vec<u8>>;
type SparseRootOverlay = (Hash, SparseNodeOverlay);

#[derive(Debug, Clone, Copy, Default)]
struct OptionalStateRootComponents {
    restrictions_root: Option<Hash>,
    shielded_root: Option<Hash>,
}

/// Merkle inclusion proof for an account in the state tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MerkleProof {
    pub leaf_hash: Hash,
    pub siblings: Vec<Hash>,
    pub path: Vec<bool>,
}

impl MerkleProof {
    pub fn verify(&self, expected_root: &Hash) -> bool {
        if self.siblings.len() != self.path.len() {
            return false;
        }
        let mut current = self.leaf_hash;
        let mut combined = [0u8; 64];
        for (sibling, &is_left) in self.siblings.iter().zip(self.path.iter()) {
            if is_left {
                combined[..32].copy_from_slice(&current.0);
                combined[32..].copy_from_slice(&sibling.0);
            } else {
                combined[..32].copy_from_slice(&sibling.0);
                combined[32..].copy_from_slice(&current.0);
            }
            current = Hash::hash(&combined);
        }
        current == *expected_root
    }

    pub fn verify_account(
        &self,
        expected_root: &Hash,
        pubkey: &Pubkey,
        account_data: &[u8],
    ) -> bool {
        let computed_leaf = Hash::hash_two_parts(&pubkey.0, account_data);
        if computed_leaf != self.leaf_hash {
            return false;
        }
        self.verify(expected_root)
    }
}

/// Branch step for a sparse_v1 account proof.
///
/// `target_went_right` records which child contains the proven account at this
/// branch. The sibling hash is enough to recompute the branch hash because the
/// proof also carries the compressed branch prefix.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SparseProofStep {
    pub prefix_bits: u16,
    pub prefix: [u8; 32],
    pub sibling: Hash,
    pub target_went_right: bool,
}

/// Sparse_v1 account inclusion proof.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SparseMerkleProof {
    pub leaf_hash: Hash,
    pub path: [u8; 32],
    pub steps: Vec<SparseProofStep>,
}

impl SparseMerkleProof {
    pub fn verify(&self, expected_root: &Hash) -> bool {
        for step in &self.steps {
            if step.prefix_bits >= 256 {
                return false;
            }
            if !StateStore::sparse_prefix_matches(&self.path, &step.prefix, step.prefix_bits) {
                return false;
            }
            if StateStore::sparse_bit(&self.path, step.prefix_bits) != step.target_went_right {
                return false;
            }
        }
        if self
            .steps
            .windows(2)
            .any(|pair| pair[0].prefix_bits >= pair[1].prefix_bits)
        {
            return false;
        }

        let leaf_node = SparseNode::Leaf {
            path: self.path,
            leaf_hash: self.leaf_hash,
        };
        let mut current = Hash::hash(&StateStore::sparse_encode_node(&leaf_node));
        for step in self.steps.iter().rev() {
            let (left, right) = if step.target_went_right {
                (step.sibling, current)
            } else {
                (current, step.sibling)
            };
            if left == Hash::default() || right == Hash::default() {
                return false;
            }
            let branch_node = SparseNode::Branch {
                prefix_bits: step.prefix_bits,
                prefix: StateStore::sparse_normalize_prefix(&step.prefix, step.prefix_bits),
                left,
                right,
            };
            current = Hash::hash(&StateStore::sparse_encode_node(&branch_node));
        }
        current == *expected_root
    }

    pub fn verify_account(
        &self,
        expected_root: &Hash,
        pubkey: &Pubkey,
        account_data: &[u8],
    ) -> bool {
        if self.path != StateStore::sparse_account_path(pubkey) {
            return false;
        }
        let computed_leaf = Hash::hash_two_parts(&pubkey.0, account_data);
        if computed_leaf != self.leaf_hash {
            return false;
        }
        self.verify(expected_root)
    }
}

/// Full account proof returned by `get_account_proof`, suitable for RPC responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountProof {
    pub pubkey: Pubkey,
    pub account_data: Vec<u8>,
    pub proof: MerkleProof,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sparse_proof: Option<SparseMerkleProof>,
    pub state_root: Hash,
}

pub(super) fn build_merkle_tree(leaves: &[Hash]) -> Vec<Vec<Hash>> {
    if leaves.is_empty() {
        return vec![vec![Hash::default()]];
    }
    if leaves.len() == 1 {
        return vec![leaves.to_vec()];
    }

    let mut levels: Vec<Vec<Hash>> = Vec::new();
    levels.push(leaves.to_vec());
    let mut combined = [0u8; 64];

    loop {
        let prev = levels.last().unwrap();
        if prev.len() == 1 {
            break;
        }
        let mut next = Vec::with_capacity(prev.len().div_ceil(2));
        for pair in prev.chunks(2) {
            combined[..32].copy_from_slice(&pair[0].0);
            if pair.len() == 2 {
                combined[32..].copy_from_slice(&pair[1].0);
            } else {
                combined[32..].copy_from_slice(&pair[0].0);
            }
            next.push(Hash::hash(&combined));
        }
        levels.push(next);
    }

    levels
}

pub(super) fn generate_proof(tree: &[Vec<Hash>], leaf_index: usize) -> Option<MerkleProof> {
    if tree.is_empty() || tree[0].is_empty() {
        return None;
    }
    if leaf_index >= tree[0].len() {
        return None;
    }
    if tree.len() == 1 {
        return Some(MerkleProof {
            leaf_hash: tree[0][leaf_index],
            siblings: Vec::new(),
            path: Vec::new(),
        });
    }

    let leaf_hash = tree[0][leaf_index];
    let mut siblings = Vec::with_capacity(tree.len() - 1);
    let mut path = Vec::with_capacity(tree.len() - 1);
    let mut idx = leaf_index;

    for level in tree.iter().take(tree.len() - 1) {
        let is_left = idx.is_multiple_of(2);
        let sibling_idx = if is_left { idx + 1 } else { idx - 1 };

        let sibling = if sibling_idx < level.len() {
            level[sibling_idx]
        } else {
            level[idx]
        };

        siblings.push(sibling);
        path.push(is_left);
        idx /= 2;
    }

    Some(MerkleProof {
        leaf_hash,
        siblings,
        path,
    })
}

impl StateStore {
    fn state_root_prefix_for_commitment(schema: u8, include_restrictions: bool) -> u8 {
        match (schema, include_restrictions) {
            (STATE_COMMITMENT_SCHEMA_SPARSE_SHIELDED_V2, true) => {
                STATE_ROOT_PREFIX_SPARSE_SHIELDED_WITH_RESTRICTIONS
            }
            (STATE_COMMITMENT_SCHEMA_SPARSE_SHIELDED_V2, false) => {
                STATE_ROOT_PREFIX_SPARSE_SHIELDED_LEGACY
            }
            (STATE_COMMITMENT_SCHEMA_SPARSE_V1, true) => STATE_ROOT_PREFIX_SPARSE_WITH_RESTRICTIONS,
            (STATE_COMMITMENT_SCHEMA_SPARSE_V1, false) => STATE_ROOT_PREFIX_SPARSE_LEGACY,
            (_, true) => STATE_ROOT_PREFIX_WITH_RESTRICTIONS,
            (_, false) => STATE_ROOT_PREFIX_LEGACY,
        }
    }

    fn active_state_root_prefix(&self, include_restrictions: bool) -> u8 {
        Self::state_root_prefix_for_commitment(
            self.get_state_commitment_schema(),
            include_restrictions,
        )
    }

    fn compose_state_root(
        &self,
        accounts_root: Hash,
        contract_root: Hash,
        stake_pool_hash: Hash,
        mossstake_pool_hash: Hash,
        include_restrictions: bool,
    ) -> Hash {
        let restrictions_root = if include_restrictions {
            Some(self.compute_restrictions_root())
        } else {
            None
        };
        let shielded_root = if self.uses_shielded_state_commitment() {
            Some(self.compute_shielded_state_root())
        } else {
            None
        };
        self.compose_state_root_with_restrictions_root(
            accounts_root,
            contract_root,
            stake_pool_hash,
            mossstake_pool_hash,
            include_restrictions,
            OptionalStateRootComponents {
                restrictions_root,
                shielded_root,
            },
        )
    }

    fn compose_state_root_with_restrictions_root(
        &self,
        accounts_root: Hash,
        contract_root: Hash,
        stake_pool_hash: Hash,
        mossstake_pool_hash: Hash,
        include_restrictions: bool,
        optional: OptionalStateRootComponents,
    ) -> Hash {
        let mut composite = Vec::with_capacity(
            1 + 32
                + 32
                + 32
                + 32
                + optional.restrictions_root.map_or(0, |_| 32)
                + optional.shielded_root.map_or(0, |_| 32),
        );
        composite.push(self.active_state_root_prefix(include_restrictions));
        composite.extend_from_slice(&accounts_root.0);
        composite.extend_from_slice(&contract_root.0);
        composite.extend_from_slice(&stake_pool_hash.0);
        composite.extend_from_slice(&mossstake_pool_hash.0);
        if let Some(restrictions_root) = optional.restrictions_root {
            composite.extend_from_slice(&restrictions_root.0);
        }
        if let Some(shielded_root) = optional.shielded_root {
            composite.extend_from_slice(&shielded_root.0);
        }
        Hash::hash(&composite)
    }

    pub(crate) fn clear_composite_state_root_cache(&self) {
        if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
            for key in [
                CACHED_STATE_ROOT_KEY,
                CACHED_STATE_ROOT_SCHEMA_KEY,
                CACHED_STATE_COMMITMENT_SCHEMA_KEY,
            ] {
                if let Err(e) = self.db.delete_cf(&cf_stats, key) {
                    tracing::warn!("Failed to clear cached state root: {e}");
                }
            }
        }
    }

    fn cache_state_root(&self, root: &Hash, include_restrictions: bool) {
        if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
            if let Err(e) = self.db.put_cf(&cf_stats, CACHED_STATE_ROOT_KEY, root.0) {
                tracing::error!("Failed to cache state root: {e}");
            }
            if let Err(e) = self.db.put_cf(
                &cf_stats,
                CACHED_STATE_ROOT_SCHEMA_KEY,
                [u8::from(include_restrictions)],
            ) {
                tracing::error!("Failed to cache state-root schema: {e}");
            }
            if let Err(e) = self.db.put_cf(
                &cf_stats,
                CACHED_STATE_COMMITMENT_SCHEMA_KEY,
                [self.get_state_commitment_schema()],
            ) {
                tracing::error!("Failed to cache state commitment schema: {e}");
            }
        }
    }

    pub(crate) fn clear_composite_state_root_cache_in_batch(&self, batch: &mut WriteBatch) {
        if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
            batch.delete_cf(&cf_stats, CACHED_STATE_ROOT_KEY);
            batch.delete_cf(&cf_stats, CACHED_STATE_ROOT_SCHEMA_KEY);
            batch.delete_cf(&cf_stats, CACHED_STATE_COMMITMENT_SCHEMA_KEY);
        }
    }

    fn cache_subroot(&self, key: &[u8], root: &Hash) {
        if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
            if let Err(e) = self.db.put_cf(&cf_stats, key, root.0) {
                tracing::error!("Failed to cache Merkle subroot: {e}");
            }
        }
    }

    fn read_cached_hash(&self, key: &[u8]) -> Option<Hash> {
        let cf_stats = self.db.cf_handle(CF_STATS)?;
        match self.db.get_cf(&cf_stats, key) {
            Ok(Some(data)) if data.len() == 32 => {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data);
                Some(Hash(bytes))
            }
            _ => None,
        }
    }

    fn read_dirty_count(&self, key: &[u8]) -> Option<u64> {
        let cf_stats = self.db.cf_handle(CF_STATS)?;
        match self.db.get_cf(&cf_stats, key) {
            Ok(Some(data)) if data.len() == 8 => {
                Some(u64::from_le_bytes(data.as_slice().try_into().ok()?))
            }
            _ => None,
        }
    }

    fn has_dirty_marker(&self, prefix: &[u8]) -> bool {
        let Some(cf_stats) = self.db.cf_handle(CF_STATS) else {
            return true;
        };
        let iter = self.db.iterator_cf(
            &cf_stats,
            rocksdb::IteratorMode::From(prefix, Direction::Forward),
        );
        iter.flatten()
            .next()
            .map(|(key, _)| key.starts_with(prefix))
            .unwrap_or(false)
    }

    fn can_use_cached_subroot(&self, dirty_count_key: &[u8], dirty_prefix: &[u8]) -> bool {
        self.read_dirty_count(dirty_count_key).unwrap_or(1) == 0
            && !self.has_dirty_marker(dirty_prefix)
    }

    fn cached_state_root_schema(&self) -> Option<bool> {
        let cf_stats = self.db.cf_handle(CF_STATS)?;
        match self.db.get_cf(&cf_stats, CACHED_STATE_ROOT_SCHEMA_KEY) {
            Ok(Some(data)) if data.len() == 1 => match data[0] {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            },
            _ => None,
        }
    }

    fn cached_state_commitment_schema(&self) -> Option<u8> {
        let cf_stats = self.db.cf_handle(CF_STATS)?;
        match self
            .db
            .get_cf(&cf_stats, CACHED_STATE_COMMITMENT_SCHEMA_KEY)
        {
            Ok(Some(data)) if data.len() == 1 => Some(data[0]),
            _ => None,
        }
    }

    pub fn get_state_commitment_schema(&self) -> u8 {
        let Some(cf_stats) = self.db.cf_handle(CF_STATS) else {
            return STATE_COMMITMENT_SCHEMA_ORDERED_V0;
        };
        match self.db.get_cf(&cf_stats, STATE_COMMITMENT_SCHEMA_KEY) {
            Ok(Some(data)) if data.len() == 1 => match data[0] {
                STATE_COMMITMENT_SCHEMA_SPARSE_V1 => STATE_COMMITMENT_SCHEMA_SPARSE_V1,
                STATE_COMMITMENT_SCHEMA_SPARSE_SHIELDED_V2 => {
                    STATE_COMMITMENT_SCHEMA_SPARSE_SHIELDED_V2
                }
                _ => STATE_COMMITMENT_SCHEMA_ORDERED_V0,
            },
            _ => STATE_COMMITMENT_SCHEMA_ORDERED_V0,
        }
    }

    pub fn uses_sparse_state_commitment(&self) -> bool {
        matches!(
            self.get_state_commitment_schema(),
            STATE_COMMITMENT_SCHEMA_SPARSE_V1 | STATE_COMMITMENT_SCHEMA_SPARSE_SHIELDED_V2
        )
    }

    pub fn uses_shielded_state_commitment(&self) -> bool {
        self.get_state_commitment_schema() == STATE_COMMITMENT_SCHEMA_SPARSE_SHIELDED_V2
    }

    pub fn set_state_commitment_schema(&self, schema: u8) -> Result<(), String> {
        let normalized = match schema {
            STATE_COMMITMENT_SCHEMA_SPARSE_V1 => STATE_COMMITMENT_SCHEMA_SPARSE_V1,
            STATE_COMMITMENT_SCHEMA_SPARSE_SHIELDED_V2 => {
                STATE_COMMITMENT_SCHEMA_SPARSE_SHIELDED_V2
            }
            _ => STATE_COMMITMENT_SCHEMA_ORDERED_V0,
        };
        let cf_stats = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "State stats CF is unavailable".to_string())?;

        if normalized != STATE_COMMITMENT_SCHEMA_ORDERED_V0
            && !self.is_sparse_state_commitment_ready()
        {
            return Err("Sparse state commitment must be rebuilt before activation".to_string());
        }

        self.db
            .put_cf(&cf_stats, STATE_COMMITMENT_SCHEMA_KEY, [normalized])
            .map_err(|e| e.to_string())?;
        for key in [
            CACHED_STATE_ROOT_KEY,
            CACHED_STATE_ROOT_SCHEMA_KEY,
            CACHED_STATE_COMMITMENT_SCHEMA_KEY,
        ] {
            if let Err(e) = self.db.delete_cf(&cf_stats, key) {
                tracing::warn!(
                    "Failed to clear cached state root during commitment schema switch: {e}"
                );
            }
        }
        Ok(())
    }

    pub fn state_commitment_schema_label(schema: u8) -> &'static str {
        match schema {
            STATE_COMMITMENT_SCHEMA_SPARSE_SHIELDED_V2 => "sparse_shielded_v2",
            STATE_COMMITMENT_SCHEMA_SPARSE_V1 => "sparse_v1",
            _ => "ordered_v0",
        }
    }

    pub fn is_sparse_state_commitment_ready(&self) -> bool {
        let Some(cf_stats) = self.db.cf_handle(CF_STATS) else {
            return false;
        };
        matches!(
            self.db.get_cf(&cf_stats, SPARSE_STATE_READY_KEY),
            Ok(Some(data)) if data.as_slice() == b"1"
        )
    }

    pub fn uses_trusted_sparse_dirty_markers(&self) -> bool {
        let Some(cf_stats) = self.db.cf_handle(CF_STATS) else {
            return false;
        };
        matches!(
            self.db.get_cf(&cf_stats, SPARSE_DIRTY_MARKERS_ATOMIC_KEY),
            Ok(Some(data)) if data.as_slice() == b"1"
        )
    }

    pub fn can_skip_active_sparse_startup_rebuild(&self) -> bool {
        self.uses_sparse_state_commitment()
            && self.is_sparse_state_commitment_ready()
            && self.uses_trusted_sparse_dirty_markers()
            && self.read_cached_hash(SPARSE_ACCOUNTS_ROOT_KEY).is_some()
            && self.read_cached_hash(SPARSE_CONTRACT_ROOT_KEY).is_some()
            && self.can_use_cached_subroot(b"dirty_account_count", b"dirty_acct:")
            && self.can_use_cached_subroot(b"dirty_contract_count", b"dirty_cstor:")
    }

    fn sparse_bit(path: &[u8; 32], bit_index: u16) -> bool {
        let idx = bit_index as usize;
        let byte = path[idx / 8];
        let shift = 7 - (idx % 8);
        (byte & (1u8 << shift)) != 0
    }

    fn sparse_common_prefix_bits(left: &[u8; 32], right: &[u8; 32]) -> u16 {
        for byte_index in 0..32 {
            let diff = left[byte_index] ^ right[byte_index];
            if diff != 0 {
                return (byte_index * 8 + diff.leading_zeros() as usize) as u16;
            }
        }
        256
    }

    fn sparse_normalize_prefix(path: &[u8; 32], prefix_bits: u16) -> [u8; 32] {
        if prefix_bits >= 256 {
            return *path;
        }
        let mut out = *path;
        let full_bytes = (prefix_bits / 8) as usize;
        let rem_bits = (prefix_bits % 8) as usize;
        if full_bytes < 32 {
            if rem_bits == 0 {
                out[full_bytes..].fill(0);
            } else {
                let mask = 0xffu8 << (8 - rem_bits);
                out[full_bytes] &= mask;
                out[(full_bytes + 1)..].fill(0);
            }
        }
        out
    }

    fn sparse_prefix_matches(path: &[u8; 32], prefix: &[u8; 32], prefix_bits: u16) -> bool {
        Self::sparse_normalize_prefix(path, prefix_bits)
            == Self::sparse_normalize_prefix(prefix, prefix_bits)
    }

    fn sparse_encode_node(node: &SparseNode) -> Vec<u8> {
        match node {
            SparseNode::Leaf { path, leaf_hash } => {
                let mut out = Vec::with_capacity(1 + 32 + 32);
                out.push(SPARSE_NODE_DOMAIN_LEAF);
                out.extend_from_slice(path);
                out.extend_from_slice(&leaf_hash.0);
                out
            }
            SparseNode::Branch {
                prefix_bits,
                prefix,
                left,
                right,
            } => {
                let mut out = Vec::with_capacity(1 + 2 + 32 + 32 + 32);
                out.push(SPARSE_NODE_DOMAIN_BRANCH);
                out.extend_from_slice(&prefix_bits.to_be_bytes());
                out.extend_from_slice(&Self::sparse_normalize_prefix(prefix, *prefix_bits));
                out.extend_from_slice(&left.0);
                out.extend_from_slice(&right.0);
                out
            }
        }
    }

    fn sparse_decode_node(data: &[u8]) -> Result<SparseNode, String> {
        match data.first().copied() {
            Some(SPARSE_NODE_DOMAIN_LEAF) if data.len() == 65 => {
                let mut path = [0u8; 32];
                path.copy_from_slice(&data[1..33]);
                let mut leaf = [0u8; 32];
                leaf.copy_from_slice(&data[33..65]);
                Ok(SparseNode::Leaf {
                    path,
                    leaf_hash: Hash(leaf),
                })
            }
            Some(SPARSE_NODE_DOMAIN_BRANCH) if data.len() == 99 => {
                let prefix_bits = u16::from_be_bytes([data[1], data[2]]);
                if prefix_bits >= 256 {
                    return Err(format!("Invalid sparse branch prefix length {prefix_bits}"));
                }
                let mut prefix = [0u8; 32];
                prefix.copy_from_slice(&data[3..35]);
                let mut left = [0u8; 32];
                left.copy_from_slice(&data[35..67]);
                let mut right = [0u8; 32];
                right.copy_from_slice(&data[67..99]);
                Ok(SparseNode::Branch {
                    prefix_bits,
                    prefix: Self::sparse_normalize_prefix(&prefix, prefix_bits),
                    left: Hash(left),
                    right: Hash(right),
                })
            }
            Some(tag) => Err(format!(
                "Invalid sparse node tag/length tag={tag:#x} len={}",
                data.len()
            )),
            None => Err("Empty sparse node payload".to_string()),
        }
    }

    fn sparse_put_overlay_node(
        overlay: &mut BTreeMap<[u8; 32], Vec<u8>>,
        node: SparseNode,
    ) -> Hash {
        let encoded = Self::sparse_encode_node(&node);
        let hash = Hash::hash(&encoded);
        overlay.insert(hash.0, encoded);
        hash
    }

    fn sparse_read_node(
        &self,
        cf_name: &str,
        overlay: &BTreeMap<[u8; 32], Vec<u8>>,
        node_hash: Hash,
    ) -> Result<SparseNode, String> {
        if node_hash == Hash::default() {
            return Err("Cannot read empty sparse node".to_string());
        }
        if let Some(data) = overlay.get(&node_hash.0) {
            return Self::sparse_decode_node(data);
        }
        let cf = self
            .db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("Sparse node CF '{cf_name}' not found"))?;
        match self.db.get_cf(&cf, node_hash.0) {
            Ok(Some(data)) => Self::sparse_decode_node(&data),
            Ok(None) => Err(format!("Sparse node {} missing in {}", node_hash, cf_name)),
            Err(e) => Err(format!("Failed to read sparse node {}: {}", node_hash, e)),
        }
    }

    fn sparse_make_leaf(
        overlay: &mut BTreeMap<[u8; 32], Vec<u8>>,
        path: [u8; 32],
        leaf_hash: Hash,
    ) -> Hash {
        Self::sparse_put_overlay_node(overlay, SparseNode::Leaf { path, leaf_hash })
    }

    fn sparse_make_branch(
        overlay: &mut BTreeMap<[u8; 32], Vec<u8>>,
        prefix_bits: u16,
        prefix: [u8; 32],
        left: Hash,
        right: Hash,
    ) -> Hash {
        debug_assert!(prefix_bits < 256);
        debug_assert_ne!(left, Hash::default());
        debug_assert_ne!(right, Hash::default());
        Self::sparse_put_overlay_node(
            overlay,
            SparseNode::Branch {
                prefix_bits,
                prefix: Self::sparse_normalize_prefix(&prefix, prefix_bits),
                left,
                right,
            },
        )
    }

    fn sparse_join_subtrees(
        &self,
        cf_name: &str,
        overlay: &mut BTreeMap<[u8; 32], Vec<u8>>,
        left_hash: Hash,
        left_path: [u8; 32],
        right_hash: Hash,
        right_path: [u8; 32],
    ) -> Result<Hash, String> {
        if left_hash == Hash::default() {
            return Ok(right_hash);
        }
        if right_hash == Hash::default() {
            return Ok(left_hash);
        }
        let prefix_bits = Self::sparse_common_prefix_bits(&left_path, &right_path);
        if prefix_bits >= 256 {
            return Err("Sparse tree path collision".to_string());
        }
        let prefix = Self::sparse_normalize_prefix(&left_path, prefix_bits);
        let left_bit = Self::sparse_bit(&left_path, prefix_bits);
        let right_bit = Self::sparse_bit(&right_path, prefix_bits);
        if left_bit == right_bit {
            return Err(format!(
                "Sparse tree branch construction failed at prefix length {prefix_bits}"
            ));
        }
        let (left, right) = if !left_bit {
            (left_hash, right_hash)
        } else {
            (right_hash, left_hash)
        };
        let _ = cf_name;
        Ok(Self::sparse_make_branch(
            overlay,
            prefix_bits,
            prefix,
            left,
            right,
        ))
    }

    fn sparse_update_node(
        &self,
        cf_name: &str,
        overlay: &mut BTreeMap<[u8; 32], Vec<u8>>,
        node_hash: Hash,
        path: [u8; 32],
        new_leaf_hash: Option<Hash>,
    ) -> Result<Hash, String> {
        if node_hash == Hash::default() {
            return Ok(match new_leaf_hash {
                Some(leaf_hash) => Self::sparse_make_leaf(overlay, path, leaf_hash),
                None => Hash::default(),
            });
        }

        match self.sparse_read_node(cf_name, overlay, node_hash)? {
            SparseNode::Leaf {
                path: old_path,
                leaf_hash: old_leaf_hash,
            } => {
                if old_path == path {
                    return Ok(match new_leaf_hash {
                        Some(leaf_hash) if leaf_hash == old_leaf_hash => node_hash,
                        Some(leaf_hash) => Self::sparse_make_leaf(overlay, path, leaf_hash),
                        None => Hash::default(),
                    });
                }
                let Some(leaf_hash) = new_leaf_hash else {
                    return Ok(node_hash);
                };
                let new_node_hash = Self::sparse_make_leaf(overlay, path, leaf_hash);
                self.sparse_join_subtrees(
                    cf_name,
                    overlay,
                    node_hash,
                    old_path,
                    new_node_hash,
                    path,
                )
            }
            SparseNode::Branch {
                prefix_bits,
                prefix,
                left,
                right,
            } => {
                if !Self::sparse_prefix_matches(&path, &prefix, prefix_bits) {
                    let Some(leaf_hash) = new_leaf_hash else {
                        return Ok(node_hash);
                    };
                    let new_node_hash = Self::sparse_make_leaf(overlay, path, leaf_hash);
                    return self.sparse_join_subtrees(
                        cf_name,
                        overlay,
                        node_hash,
                        prefix,
                        new_node_hash,
                        path,
                    );
                }

                let use_right = Self::sparse_bit(&path, prefix_bits);
                let old_child = if use_right { right } else { left };
                let new_child =
                    self.sparse_update_node(cf_name, overlay, old_child, path, new_leaf_hash)?;
                if new_child == old_child {
                    return Ok(node_hash);
                }

                let (new_left, new_right) = if use_right {
                    (left, new_child)
                } else {
                    (new_child, right)
                };
                match (new_left == Hash::default(), new_right == Hash::default()) {
                    (true, true) => Ok(Hash::default()),
                    (true, false) => Ok(new_right),
                    (false, true) => Ok(new_left),
                    (false, false) => Ok(Self::sparse_make_branch(
                        overlay,
                        prefix_bits,
                        prefix,
                        new_left,
                        new_right,
                    )),
                }
            }
        }
    }

    fn sparse_account_path(pubkey: &Pubkey) -> [u8; 32] {
        pubkey.0
    }

    fn sparse_contract_path(full_key: &[u8]) -> [u8; 32] {
        Hash::hash(full_key).0
    }

    fn read_stats_hash_or_default(&self, key: &[u8]) -> Hash {
        self.read_cached_hash(key).unwrap_or_default()
    }

    fn read_stats_u64(&self, key: &[u8]) -> u64 {
        self.read_dirty_count(key).unwrap_or(0)
    }

    fn sparse_compute_root_from_entries(
        &self,
        cf_name: &str,
        entries: &[SparseLeafEntry],
    ) -> Result<SparseRootOverlay, String> {
        let mut ordered = entries.to_vec();
        ordered.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.leaf_key.cmp(&right.leaf_key))
        });

        let mut overlay = BTreeMap::<[u8; 32], Vec<u8>>::new();
        let mut root = Hash::default();
        let mut last_path: Option<[u8; 32]> = None;
        for entry in ordered {
            if Some(entry.path) == last_path {
                return Err("Sparse state commitment path collision".to_string());
            }
            root = self.sparse_update_node(
                cf_name,
                &mut overlay,
                root,
                entry.path,
                Some(entry.leaf_hash),
            )?;
            last_path = Some(entry.path);
        }

        Ok((root, overlay))
    }

    fn sparse_root_with_changes(
        &self,
        cf_name: &str,
        current_root: Hash,
        changes: &[SparseLeafChange],
    ) -> Result<SparseRootOverlay, String> {
        let mut ordered = changes.to_vec();
        ordered.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.leaf_key.cmp(&right.leaf_key))
        });
        let mut overlay = BTreeMap::<[u8; 32], Vec<u8>>::new();
        let mut root = current_root;
        let mut last_path: Option<[u8; 32]> = None;
        for change in ordered {
            if Some(change.path) == last_path {
                return Err("Sparse state commitment path collision in change set".to_string());
            }
            root = self.sparse_update_node(
                cf_name,
                &mut overlay,
                root,
                change.path,
                change.leaf_hash,
            )?;
            last_path = Some(change.path);
        }
        Ok((root, overlay))
    }

    fn clear_sparse_node_cf(&self, cf_name: &str, batch: &mut WriteBatch) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("Sparse node CF '{cf_name}' not found"))?;
        for item in self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::Start)
            .flatten()
        {
            let (key, _) = item;
            batch.delete_cf(&cf, &*key);
        }
        Ok(())
    }

    fn write_sparse_overlay_nodes(
        &self,
        cf_name: &str,
        overlay: &BTreeMap<[u8; 32], Vec<u8>>,
        batch: &mut WriteBatch,
    ) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("Sparse node CF '{cf_name}' not found"))?;
        for (hash, data) in overlay {
            batch.put_cf(&cf, hash, data);
        }
        Ok(overlay.len() as u64)
    }

    fn clear_leaf_cf(&self, cf_name: &str, batch: &mut WriteBatch) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("Merkle leaf CF '{cf_name}' not found"))?;
        for item in self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::Start)
            .flatten()
        {
            let (key, _) = item;
            batch.delete_cf(&cf, &*key);
        }
        Ok(())
    }

    fn write_sparse_leaf_entries(
        &self,
        cf_name: &str,
        entries: &[SparseLeafEntry],
        batch: &mut WriteBatch,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("Merkle leaf CF '{cf_name}' not found"))?;
        for entry in entries {
            batch.put_cf(&cf, &entry.leaf_key, entry.leaf_hash.0);
        }
        Ok(())
    }

    fn collect_sparse_account_entries(&self) -> Result<Vec<SparseLeafEntry>, String> {
        let cf_accounts = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;
        let mut entries = Vec::new();
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        for item in self
            .db
            .iterator_cf_opt(&cf_accounts, read_opts, rocksdb::IteratorMode::Start)
            .flatten()
        {
            let (key, value) = item;
            if key.len() != 32 || Self::deserialize_account_check_dormant(&value) {
                continue;
            }
            let mut pubkey = [0u8; 32];
            pubkey.copy_from_slice(&key);
            entries.push(SparseLeafEntry {
                leaf_key: pubkey.to_vec(),
                path: pubkey,
                leaf_hash: Hash::hash_two_parts(&pubkey, &value),
            });
        }
        Ok(entries)
    }

    fn collect_sparse_contract_entries(&self) -> Result<Vec<SparseLeafEntry>, String> {
        let cf_storage = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;
        let mut entries = Vec::new();
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        for item in self
            .db
            .iterator_cf_opt(&cf_storage, read_opts, rocksdb::IteratorMode::Start)
            .flatten()
        {
            let (key, value) = item;
            let leaf_key = key.to_vec();
            entries.push(SparseLeafEntry {
                path: Self::sparse_contract_path(&leaf_key),
                leaf_hash: Hash::hash_two_parts(&leaf_key, &value),
                leaf_key,
            });
        }
        Ok(entries)
    }

    fn rebuild_sparse_accounts_commitment(&self) -> Result<(Hash, u64, u64), String> {
        let entries = self.collect_sparse_account_entries()?;
        let dirty_keys = self.dirty_account_keys();
        let (root, overlay) =
            self.sparse_compute_root_from_entries(CF_ACCOUNT_MERKLE_NODES, &entries)?;
        let mut batch = WriteBatch::default();
        self.clear_sparse_node_cf(CF_ACCOUNT_MERKLE_NODES, &mut batch)?;
        self.clear_leaf_cf(CF_MERKLE_LEAVES, &mut batch)?;
        self.write_sparse_leaf_entries(CF_MERKLE_LEAVES, &entries, &mut batch)?;
        let node_count =
            self.write_sparse_overlay_nodes(CF_ACCOUNT_MERKLE_NODES, &overlay, &mut batch)?;
        let cf_stats = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let leaf_count = entries.len() as u64;
        for pk in dirty_keys {
            let mut dirty_key = [0u8; 43];
            dirty_key[..11].copy_from_slice(b"dirty_acct:");
            dirty_key[11..43].copy_from_slice(&pk);
            batch.delete_cf(&cf_stats, dirty_key);
        }
        batch.put_cf(&cf_stats, SPARSE_ACCOUNTS_ROOT_KEY, root.0);
        batch.put_cf(
            &cf_stats,
            SPARSE_ACCOUNTS_LEAF_COUNT_KEY,
            leaf_count.to_le_bytes(),
        );
        batch.put_cf(&cf_stats, b"merkle_leaf_count", leaf_count.to_le_bytes());
        batch.put_cf(&cf_stats, b"dirty_account_count", 0u64.to_le_bytes());
        batch.delete_cf(&cf_stats, CACHED_ACCOUNTS_ROOT_KEY);
        batch.delete_cf(&cf_stats, CACHED_STATE_ROOT_KEY);
        batch.delete_cf(&cf_stats, CACHED_STATE_ROOT_SCHEMA_KEY);
        batch.delete_cf(&cf_stats, CACHED_STATE_COMMITMENT_SCHEMA_KEY);
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to rebuild sparse account commitment: {e}"))?;
        Ok((root, leaf_count, node_count))
    }

    fn rebuild_sparse_contract_commitment(&self) -> Result<(Hash, u64, u64), String> {
        let entries = self.collect_sparse_contract_entries()?;
        let dirty_keys = self.dirty_contract_storage_keys();
        let (root, overlay) =
            self.sparse_compute_root_from_entries(CF_CONTRACT_MERKLE_NODES, &entries)?;
        let mut batch = WriteBatch::default();
        self.clear_sparse_node_cf(CF_CONTRACT_MERKLE_NODES, &mut batch)?;
        self.clear_leaf_cf(CF_CONTRACT_MERKLE_LEAVES, &mut batch)?;
        self.write_sparse_leaf_entries(CF_CONTRACT_MERKLE_LEAVES, &entries, &mut batch)?;
        let node_count =
            self.write_sparse_overlay_nodes(CF_CONTRACT_MERKLE_NODES, &overlay, &mut batch)?;
        let cf_stats = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let leaf_count = entries.len() as u64;
        for full_key in dirty_keys {
            let mut marker_key = Vec::with_capacity(b"dirty_cstor:".len() + full_key.len());
            marker_key.extend_from_slice(b"dirty_cstor:");
            marker_key.extend_from_slice(&full_key);
            batch.delete_cf(&cf_stats, marker_key);
        }
        batch.put_cf(&cf_stats, SPARSE_CONTRACT_ROOT_KEY, root.0);
        batch.put_cf(
            &cf_stats,
            SPARSE_CONTRACT_LEAF_COUNT_KEY,
            leaf_count.to_le_bytes(),
        );
        batch.put_cf(
            &cf_stats,
            b"contract_merkle_leaf_count",
            leaf_count.to_le_bytes(),
        );
        batch.put_cf(&cf_stats, b"dirty_contract_count", 0u64.to_le_bytes());
        batch.delete_cf(&cf_stats, CACHED_CONTRACT_ROOT_KEY);
        batch.delete_cf(&cf_stats, CACHED_STATE_ROOT_KEY);
        batch.delete_cf(&cf_stats, CACHED_STATE_ROOT_SCHEMA_KEY);
        batch.delete_cf(&cf_stats, CACHED_STATE_COMMITMENT_SCHEMA_KEY);
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to rebuild sparse contract commitment: {e}"))?;
        Ok((root, leaf_count, node_count))
    }

    pub fn rebuild_sparse_state_commitment(
        &self,
        activate: bool,
    ) -> Result<SparseStateCommitmentReport, String> {
        let before_schema = self.get_state_commitment_schema();
        let cf_stats = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        self.db
            .put_cf(&cf_stats, SPARSE_STATE_READY_KEY, b"0")
            .map_err(|e| format!("Failed to mark sparse commitment rebuilding: {e}"))?;
        self.db
            .put_cf(&cf_stats, SPARSE_DIRTY_MARKERS_ATOMIC_KEY, b"0")
            .map_err(|e| format!("Failed to mark sparse dirty markers untrusted: {e}"))?;
        let (accounts_root, accounts_leaf_count, accounts_node_count) =
            self.rebuild_sparse_accounts_commitment()?;
        let (contract_root, contract_leaf_count, contract_node_count) =
            self.rebuild_sparse_contract_commitment()?;
        self.db
            .put_cf(&cf_stats, SPARSE_STATE_READY_KEY, b"1")
            .map_err(|e| format!("Failed to mark sparse commitment ready: {e}"))?;
        self.db
            .put_cf(&cf_stats, SPARSE_DIRTY_MARKERS_ATOMIC_KEY, b"1")
            .map_err(|e| format!("Failed to mark sparse dirty markers atomic: {e}"))?;
        if activate {
            self.set_state_commitment_schema(STATE_COMMITMENT_SCHEMA_SPARSE_V1)?;
        }
        let last_slot = self.get_last_slot().unwrap_or(0);
        let latest_block_state_root = self
            .get_block_by_slot(last_slot)
            .ok()
            .flatten()
            .map(|block| block.header.state_root);
        let include_restrictions = self.get_state_root_schema().unwrap_or(false);
        let shielded_root = self.compute_shielded_state_root();
        let current_state_root = self.compose_state_root(
            accounts_root,
            contract_root,
            self.compute_stake_pool_hash(),
            self.compute_mossstake_pool_hash(),
            include_restrictions,
        );
        self.cache_state_root(&current_state_root, include_restrictions);
        Ok(SparseStateCommitmentReport {
            before_schema,
            after_schema: self.get_state_commitment_schema(),
            active: self.uses_sparse_state_commitment(),
            last_slot,
            current_state_root,
            latest_block_state_root,
            accounts_root,
            contract_root,
            shielded_root,
            accounts_leaf_count,
            contract_leaf_count,
            accounts_node_count,
            contract_node_count,
            activated: self.uses_sparse_state_commitment(),
        })
    }

    pub fn activate_shielded_state_commitment(
        &self,
    ) -> Result<SparseStateCommitmentReport, String> {
        let before_schema = self.get_state_commitment_schema();
        self.rebuild_sparse_state_commitment(false)?;
        self.set_state_commitment_schema(STATE_COMMITMENT_SCHEMA_SPARSE_SHIELDED_V2)?;
        let mut report = self.verify_sparse_state_commitment()?;
        report.before_schema = before_schema;
        report.activated = true;
        Ok(report)
    }

    pub fn verify_sparse_state_commitment(&self) -> Result<SparseStateCommitmentReport, String> {
        let before_schema = self.get_state_commitment_schema();
        let account_entries = self.collect_sparse_account_entries()?;
        let contract_entries = self.collect_sparse_contract_entries()?;
        let (accounts_root, account_nodes) =
            self.sparse_compute_root_from_entries(CF_ACCOUNT_MERKLE_NODES, &account_entries)?;
        let (contract_root, contract_nodes) =
            self.sparse_compute_root_from_entries(CF_CONTRACT_MERKLE_NODES, &contract_entries)?;
        let stored_accounts_root = self.read_stats_hash_or_default(SPARSE_ACCOUNTS_ROOT_KEY);
        let stored_contract_root = self.read_stats_hash_or_default(SPARSE_CONTRACT_ROOT_KEY);
        if self.is_sparse_state_commitment_ready()
            && (accounts_root != stored_accounts_root || contract_root != stored_contract_root)
        {
            return Err(format!(
                "Sparse commitment verification failed: accounts computed={} stored={} contracts computed={} stored={}",
                accounts_root.to_hex(),
                stored_accounts_root.to_hex(),
                contract_root.to_hex(),
                stored_contract_root.to_hex()
            ));
        }
        let last_slot = self.get_last_slot().unwrap_or(0);
        let latest_block_state_root = self
            .get_block_by_slot(last_slot)
            .ok()
            .flatten()
            .map(|block| block.header.state_root);
        let include_restrictions = self.get_state_root_schema().unwrap_or(false);
        let shielded_root = self.compute_shielded_state_root();
        let current_state_root = self.compose_state_root(
            accounts_root,
            contract_root,
            self.compute_stake_pool_hash(),
            self.compute_mossstake_pool_hash(),
            include_restrictions,
        );
        Ok(SparseStateCommitmentReport {
            before_schema,
            after_schema: before_schema,
            active: self.uses_sparse_state_commitment(),
            last_slot,
            current_state_root,
            latest_block_state_root,
            accounts_root,
            contract_root,
            shielded_root,
            accounts_leaf_count: account_entries.len() as u64,
            contract_leaf_count: contract_entries.len() as u64,
            accounts_node_count: account_nodes.len() as u64,
            contract_node_count: contract_nodes.len() as u64,
            activated: self.uses_sparse_state_commitment(),
        })
    }

    pub fn get_state_root_schema(&self) -> Option<bool> {
        let cf_stats = self.db.cf_handle(CF_STATS)?;
        match self.db.get_cf(&cf_stats, STATE_ROOT_SCHEMA_KEY) {
            Ok(Some(data)) if data.len() == 1 => match data[0] {
                0 => Some(false),
                1 => Some(true),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn set_state_root_schema(&self, include_restrictions: bool) -> Result<(), String> {
        let cf_stats = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "State stats CF is unavailable".to_string())?;

        let current = self.get_state_root_schema();
        if let Some(current) = current {
            if current != include_restrictions {
                if let Err(e) = self.db.delete_cf(&cf_stats, CACHED_STATE_ROOT_KEY) {
                    tracing::warn!("Failed to clear cached state root during schema switch: {e}");
                }
                if let Err(e) = self.db.delete_cf(&cf_stats, CACHED_STATE_ROOT_SCHEMA_KEY) {
                    tracing::warn!(
                        "Failed to clear cached state-root schema during schema switch: {e}"
                    );
                }
            }
        }

        self.db
            .put_cf(
                &cf_stats,
                STATE_ROOT_SCHEMA_KEY,
                [u8::from(include_restrictions)],
            )
            .map_err(|e| e.to_string())
    }

    pub fn compute_state_root_with_restrictions(&self) -> Hash {
        let accounts_root = self.compute_accounts_root();
        let contract_root = self.compute_contract_storage_root();
        let stake_pool_hash = self.compute_stake_pool_hash();
        let mossstake_pool_hash = self.compute_mossstake_pool_hash();
        let restrictions_root = self.compute_restrictions_root();
        let root = self.compose_state_root(
            accounts_root,
            contract_root,
            stake_pool_hash,
            mossstake_pool_hash,
            true,
        );
        tracing::debug!(
            "🔍 STATE_ROOT_COMPONENTS: accts={} contracts={} stake={} moss={} restrictions={} prefix=0x{:02x} → root={}",
            hex::encode(&accounts_root.0[..8]),
            hex::encode(&contract_root.0[..8]),
            hex::encode(&stake_pool_hash.0[..8]),
            hex::encode(&mossstake_pool_hash.0[..8]),
            hex::encode(&restrictions_root.0[..8]),
            self.active_state_root_prefix(true),
            hex::encode(&root.0[..8]),
        );
        root
    }

    pub fn compute_state_root_without_restrictions(&self) -> Hash {
        let accounts_root = self.compute_accounts_root();
        let contract_root = self.compute_contract_storage_root();
        let stake_pool_hash = self.compute_stake_pool_hash();
        let mossstake_pool_hash = self.compute_mossstake_pool_hash();
        let root = self.compose_state_root(
            accounts_root,
            contract_root,
            stake_pool_hash,
            mossstake_pool_hash,
            false,
        );
        tracing::debug!(
            "🔍 STATE_ROOT_COMPONENTS: accts={} contracts={} stake={} moss={} prefix=0x{:02x} → root={}",
            hex::encode(&accounts_root.0[..8]),
            hex::encode(&contract_root.0[..8]),
            hex::encode(&stake_pool_hash.0[..8]),
            hex::encode(&mossstake_pool_hash.0[..8]),
            self.active_state_root_prefix(false),
            hex::encode(&root.0[..8]),
        );
        root
    }

    pub fn compute_state_root_with_restrictions_cold_start(&self) -> Hash {
        let accounts_root = self.compute_accounts_root_cold_start();
        let contract_root = self.compute_contract_storage_root_cold_start();
        let stake_pool_hash = self.compute_stake_pool_hash();
        let mossstake_pool_hash = self.compute_mossstake_pool_hash();
        self.compose_state_root(
            accounts_root,
            contract_root,
            stake_pool_hash,
            mossstake_pool_hash,
            true,
        )
    }

    pub fn compute_state_root_without_restrictions_cold_start(&self) -> Hash {
        let accounts_root = self.compute_accounts_root_cold_start();
        let contract_root = self.compute_contract_storage_root_cold_start();
        let stake_pool_hash = self.compute_stake_pool_hash();
        let mossstake_pool_hash = self.compute_mossstake_pool_hash();
        self.compose_state_root(
            accounts_root,
            contract_root,
            stake_pool_hash,
            mossstake_pool_hash,
            false,
        )
    }

    pub fn detect_state_root_schema_for_root(&self, expected_root: &Hash) -> Option<bool> {
        if self.compute_state_root_without_restrictions().0 == expected_root.0 {
            Some(false)
        } else if self.compute_state_root_with_restrictions().0 == expected_root.0 {
            Some(true)
        } else {
            None
        }
    }

    pub fn detect_state_root_schema_for_root_cold_start(
        &self,
        expected_root: &Hash,
    ) -> Option<bool> {
        if self.compute_state_root_without_restrictions_cold_start().0 == expected_root.0 {
            Some(false)
        } else if self.compute_state_root_with_restrictions_cold_start().0 == expected_root.0 {
            Some(true)
        } else {
            None
        }
    }

    fn build_sparse_account_proof(
        &self,
        root: Hash,
        pubkey: &Pubkey,
        leaf_hash: Hash,
    ) -> Option<SparseMerkleProof> {
        if root == Hash::default() {
            return None;
        }

        let path = Self::sparse_account_path(pubkey);
        let overlay = BTreeMap::new();
        let mut node_hash = root;
        let mut steps = Vec::new();

        loop {
            match self
                .sparse_read_node(CF_ACCOUNT_MERKLE_NODES, &overlay, node_hash)
                .ok()?
            {
                SparseNode::Leaf {
                    path: leaf_path,
                    leaf_hash: stored_leaf_hash,
                } => {
                    if leaf_path != path || stored_leaf_hash != leaf_hash {
                        return None;
                    }
                    return Some(SparseMerkleProof {
                        leaf_hash,
                        path,
                        steps,
                    });
                }
                SparseNode::Branch {
                    prefix_bits,
                    prefix,
                    left,
                    right,
                } => {
                    if !Self::sparse_prefix_matches(&path, &prefix, prefix_bits) {
                        return None;
                    }
                    let target_went_right = Self::sparse_bit(&path, prefix_bits);
                    let (child, sibling) = if target_went_right {
                        (right, left)
                    } else {
                        (left, right)
                    };
                    if child == Hash::default() || sibling == Hash::default() {
                        return None;
                    }
                    steps.push(SparseProofStep {
                        prefix_bits,
                        prefix: Self::sparse_normalize_prefix(&prefix, prefix_bits),
                        sibling,
                        target_went_right,
                    });
                    if steps.len() > 256 {
                        return None;
                    }
                    node_hash = child;
                }
            }
        }
    }

    fn get_sparse_account_proof(
        &self,
        pubkey: &Pubkey,
        account_data: Vec<u8>,
    ) -> Option<AccountProof> {
        let leaf_hash = Hash::hash_two_parts(&pubkey.0, &account_data);
        let accounts_root = match self.compute_sparse_accounts_root() {
            Ok(root) => root,
            Err(err) => {
                tracing::warn!("Sparse account proof unavailable: {err}");
                return None;
            }
        };
        let sparse_proof = self.build_sparse_account_proof(accounts_root, pubkey, leaf_hash)?;
        if !sparse_proof.verify_account(&accounts_root, pubkey, &account_data) {
            return None;
        }

        let contract_root = match self.compute_sparse_contract_storage_root() {
            Ok(root) => root,
            Err(err) => {
                tracing::warn!("Sparse account proof contract root unavailable: {err}");
                return None;
            }
        };
        let stake_pool_hash = self.compute_stake_pool_hash();
        let mossstake_pool_hash = self.compute_mossstake_pool_hash();
        let include_restrictions = self.get_state_root_schema().unwrap_or(false);
        let composite_root = self.compose_state_root(
            accounts_root,
            contract_root,
            stake_pool_hash,
            mossstake_pool_hash,
            include_restrictions,
        );

        Some(AccountProof {
            pubkey: *pubkey,
            account_data,
            proof: MerkleProof {
                leaf_hash,
                siblings: Vec::new(),
                path: Vec::new(),
            },
            sparse_proof: Some(sparse_proof),
            state_root: composite_root,
        })
    }

    /// Generate an inclusion proof for the given account.
    pub fn get_account_proof(&self, pubkey: &Pubkey) -> Option<AccountProof> {
        let cf_accounts = self.db.cf_handle(CF_ACCOUNTS)?;
        let account_data = self.db.get_cf(&cf_accounts, pubkey.0).ok()??;
        if self.uses_sparse_state_commitment() {
            return self.get_sparse_account_proof(pubkey, account_data);
        }

        let cf_leaves = self.db.cf_handle(CF_MERKLE_LEAVES)?;
        let mut leaf_hashes: Vec<Hash> = Vec::new();
        let mut leaf_keys: Vec<[u8; 32]> = Vec::new();
        let iter = self
            .db
            .iterator_cf(&cf_leaves, rocksdb::IteratorMode::Start);
        for item in iter.flatten() {
            let (key, value) = item;
            if key.len() == 32 && value.len() == 32 {
                let mut pk = [0u8; 32];
                pk.copy_from_slice(&key);
                leaf_keys.push(pk);
                let mut h = [0u8; 32];
                h.copy_from_slice(&value);
                leaf_hashes.push(Hash(h));
            }
        }

        let target_leaf = Hash::hash_two_parts(&pubkey.0, &account_data);
        let leaf_index = leaf_keys.iter().position(|k| k == &pubkey.0)?;

        if leaf_hashes[leaf_index] != target_leaf {
            let recomputed = Hash::hash_two_parts(&pubkey.0, &account_data);
            if leaf_hashes[leaf_index] != recomputed {
                return None;
            }
        }

        let tree = build_merkle_tree(&leaf_hashes);
        let root = *tree.last()?.first()?;
        let proof = generate_proof(&tree, leaf_index)?;

        let contract_root = self.compute_contract_storage_root();
        let stake_pool_hash = self.compute_stake_pool_hash();
        let mossstake_pool_hash = self.compute_mossstake_pool_hash();
        let include_restrictions = self.get_state_root_schema().unwrap_or(false);
        let restrictions_root = if include_restrictions {
            Some(self.compute_restrictions_root())
        } else {
            None
        };
        let mut composite = Vec::with_capacity(1 + 32 + 32 + 32 + 32);
        composite.push(self.active_state_root_prefix(include_restrictions));
        composite.extend_from_slice(&root.0);
        composite.extend_from_slice(&contract_root.0);
        composite.extend_from_slice(&stake_pool_hash.0);
        composite.extend_from_slice(&mossstake_pool_hash.0);
        if let Some(restrictions_root) = restrictions_root {
            composite.extend_from_slice(&restrictions_root.0);
        }
        let composite_root = Hash::hash(&composite);

        Some(AccountProof {
            pubkey: *pubkey,
            account_data,
            proof,
            sparse_proof: None,
            state_root: composite_root,
        })
    }

    /// Verify an account proof against a known state root.
    pub fn verify_account_proof(
        root: &Hash,
        pubkey: &Pubkey,
        account_data: &[u8],
        proof: &MerkleProof,
    ) -> bool {
        proof.verify_account(root, pubkey, account_data)
    }

    pub fn verify_sparse_account_proof(
        root: &Hash,
        pubkey: &Pubkey,
        account_data: &[u8],
        proof: &SparseMerkleProof,
    ) -> bool {
        proof.verify_account(root, pubkey, account_data)
    }

    /// Compute state root hash using the incremental Merkle tree.
    pub fn compute_state_root(&self) -> Hash {
        let accounts_root = self.compute_accounts_root();
        let contract_root = self.compute_contract_storage_root();
        let stake_pool_hash = self.compute_stake_pool_hash();
        let mossstake_pool_hash = self.compute_mossstake_pool_hash();
        let include_restrictions = self.get_state_root_schema().unwrap_or(false);
        let restrictions_root = if include_restrictions {
            Some(self.compute_restrictions_root())
        } else {
            None
        };
        let root = self.compose_state_root(
            accounts_root,
            contract_root,
            stake_pool_hash,
            mossstake_pool_hash,
            include_restrictions,
        );
        tracing::debug!(
            "🔍 STATE_ROOT_COMPONENTS: accts={} contracts={} stake={} moss={} restrictions={} prefix=0x{:02x} → root={}",
            hex::encode(&accounts_root.0[..8]),
            hex::encode(&contract_root.0[..8]),
            hex::encode(&stake_pool_hash.0[..8]),
            hex::encode(&mossstake_pool_hash.0[..8]),
            restrictions_root
                .map(|root| hex::encode(&root.0[..8]))
                .unwrap_or_else(|| "disabled".to_string()),
            self.active_state_root_prefix(include_restrictions),
            hex::encode(&root.0[..8]),
        );
        self.cache_state_root(&root, include_restrictions);
        root
    }

    /// Compute the post-transaction state root for an uncommitted batch.
    ///
    /// This is used by proposers to evaluate candidate blocks without cloning
    /// or mutating the live RocksDB state. It first brings the canonical
    /// incremental Merkle leaf caches up to date, then overlays the batch's
    /// account/contract/restriction changes in memory.
    pub fn compute_state_root_for_batch(&self, batch: &StateBatch) -> Hash {
        let accounts_root = if self.uses_sparse_state_commitment() {
            match self.compute_sparse_accounts_root_for_batch(batch) {
                Ok(root) => root,
                Err(err) => {
                    tracing::error!("Sparse proposal account root failed: {err}");
                    self.compute_accounts_root_for_batch(batch)
                }
            }
        } else {
            self.compute_accounts_root_for_batch(batch)
        };
        let contract_root = if self.uses_sparse_state_commitment() {
            match self.compute_sparse_contract_storage_root_for_batch(batch) {
                Ok(root) => root,
                Err(err) => {
                    tracing::error!("Sparse proposal contract root failed: {err}");
                    self.compute_contract_storage_root_for_batch(batch)
                }
            }
        } else {
            self.compute_contract_storage_root_for_batch(batch)
        };
        let stake_pool_hash = batch
            .stake_pool_overlay
            .as_ref()
            .map(|pool| pool.canonical_hash())
            .unwrap_or_else(|| self.compute_stake_pool_hash());
        let mossstake_pool_hash = batch
            .mossstake_pool_overlay
            .as_ref()
            .map(|pool| {
                if self.is_mossstake_slot_only() {
                    pool.canonical_hash()
                } else {
                    pool.legacy_canonical_hash()
                }
            })
            .unwrap_or_else(|| self.compute_mossstake_pool_hash());
        let include_restrictions = self.get_state_root_schema().unwrap_or(false);

        let restrictions_root = if include_restrictions && !batch.restriction_overlay.is_empty() {
            Some(self.compute_restrictions_root_for_batch(batch))
        } else if include_restrictions {
            Some(self.compute_restrictions_root())
        } else {
            None
        };
        let shielded_root = if self.uses_shielded_state_commitment() {
            Some(self.compute_shielded_state_root_for_batch(batch))
        } else {
            None
        };

        let mut composite = Vec::with_capacity(
            1 + 32
                + 32
                + 32
                + 32
                + restrictions_root.map_or(0, |_| 32)
                + shielded_root.map_or(0, |_| 32),
        );
        composite.push(self.active_state_root_prefix(include_restrictions));
        composite.extend_from_slice(&accounts_root.0);
        composite.extend_from_slice(&contract_root.0);
        composite.extend_from_slice(&stake_pool_hash.0);
        composite.extend_from_slice(&mossstake_pool_hash.0);
        if let Some(restrictions_root) = restrictions_root {
            composite.extend_from_slice(&restrictions_root.0);
        }
        if let Some(shielded_root) = shielded_root {
            composite.extend_from_slice(&shielded_root.0);
        }
        Hash::hash(&composite)
    }

    fn serialized_account_value(account: &Account) -> Result<Vec<u8>, String> {
        let mut value = Vec::with_capacity(256);
        value.push(0xBC);
        append_legacy_bincode(&mut value, account, "account")
            .map_err(|e| format!("Failed to serialize account: {}", e))?;
        Ok(value)
    }

    fn compute_accounts_root_for_batch(&self, batch: &StateBatch) -> Hash {
        if batch.account_overlay.is_empty() {
            return self.compute_accounts_root();
        }

        // Make sure canonical dirty markers have been folded into leaf cache.
        let _ = self.compute_accounts_root();

        let cf_leaves = match self.db.cf_handle(CF_MERKLE_LEAVES) {
            Some(handle) => handle,
            None => return self.compute_accounts_root_full_scan(),
        };

        let mut overlay =
            Vec::<(Vec<u8>, Option<Hash>)>::with_capacity(batch.account_overlay.len());
        for (pubkey, account) in &batch.account_overlay {
            if account.dormant {
                overlay.push((pubkey.0.to_vec(), None));
                continue;
            }
            match Self::serialized_account_value(account) {
                Ok(value) => {
                    overlay.push((
                        pubkey.0.to_vec(),
                        Some(Hash::hash_two_parts(&pubkey.0, &value)),
                    ));
                }
                Err(err) => tracing::warn!("Failed to overlay account in state root: {}", err),
            }
        }
        overlay.sort_by(|(left, _), (right, _)| left.cmp(right));

        let mut leaves = Vec::<Hash>::new();
        let mut overlay_iter = overlay.into_iter().peekable();
        'leaf_scan: for item in self
            .db
            .iterator_cf(&cf_leaves, rocksdb::IteratorMode::Start)
            .flatten()
        {
            let (key, value) = item;
            let key_vec = key.to_vec();
            loop {
                match overlay_iter
                    .peek()
                    .map(|(overlay_key, _)| overlay_key.as_slice().cmp(key_vec.as_slice()))
                {
                    Some(std::cmp::Ordering::Less) => {
                        let (_, overlay_leaf) = overlay_iter.next().expect("peeked overlay entry");
                        if let Some(leaf) = overlay_leaf {
                            leaves.push(leaf);
                        }
                    }
                    Some(std::cmp::Ordering::Equal) => {
                        let (_, overlay_leaf) = overlay_iter.next().expect("peeked overlay entry");
                        if let Some(leaf) = overlay_leaf {
                            leaves.push(leaf);
                        }
                        continue 'leaf_scan;
                    }
                    _ => break,
                }
            }

            if value.len() == 32 {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&value);
                leaves.push(Hash(bytes));
            }
        }

        for (_, overlay_leaf) in overlay_iter {
            if let Some(leaf) = overlay_leaf {
                leaves.push(leaf);
            }
        }

        Self::merkle_root_from_leaves(&leaves)
    }

    fn compute_contract_storage_root_for_batch(&self, batch: &StateBatch) -> Hash {
        if batch.contract_storage_overlay.is_empty() {
            return self.compute_contract_storage_root();
        }

        // Make sure canonical dirty markers have been folded into leaf cache.
        let _ = self.compute_contract_storage_root();

        let cf_leaves = match self.db.cf_handle(CF_CONTRACT_MERKLE_LEAVES) {
            Some(handle) => handle,
            None => return self.compute_contract_storage_root_full_scan(),
        };

        let mut overlay =
            Vec::<(Vec<u8>, Option<Hash>)>::with_capacity(batch.contract_storage_overlay.len());
        for (full_key, value) in &batch.contract_storage_overlay {
            match value {
                Some(value) => {
                    overlay.push((
                        full_key.clone(),
                        Some(Hash::hash_two_parts(full_key, value)),
                    ));
                }
                None => {
                    overlay.push((full_key.clone(), None));
                }
            }
        }
        overlay.sort_by(|(left, _), (right, _)| left.cmp(right));

        let mut leaves = Vec::<Hash>::new();
        let mut overlay_iter = overlay.into_iter().peekable();
        'leaf_scan: for item in self
            .db
            .iterator_cf(&cf_leaves, rocksdb::IteratorMode::Start)
            .flatten()
        {
            let (key, value) = item;
            let key_vec = key.to_vec();
            loop {
                match overlay_iter
                    .peek()
                    .map(|(overlay_key, _)| overlay_key.as_slice().cmp(key_vec.as_slice()))
                {
                    Some(std::cmp::Ordering::Less) => {
                        let (_, overlay_leaf) = overlay_iter.next().expect("peeked overlay entry");
                        if let Some(leaf) = overlay_leaf {
                            leaves.push(leaf);
                        }
                    }
                    Some(std::cmp::Ordering::Equal) => {
                        let (_, overlay_leaf) = overlay_iter.next().expect("peeked overlay entry");
                        if let Some(leaf) = overlay_leaf {
                            leaves.push(leaf);
                        }
                        continue 'leaf_scan;
                    }
                    _ => break,
                }
            }

            if value.len() == 32 {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&value);
                leaves.push(Hash(bytes));
            }
        }

        for (_, overlay_leaf) in overlay_iter {
            if let Some(leaf) = overlay_leaf {
                leaves.push(leaf);
            }
        }

        Self::merkle_root_from_leaves(&leaves)
    }

    fn compute_restrictions_root_for_batch(&self, batch: &StateBatch) -> Hash {
        let cf = match self.db.cf_handle(CF_RESTRICTIONS) {
            Some(cf) => cf,
            None => return Hash::default(),
        };
        let mut leaves = std::collections::BTreeMap::<Vec<u8>, Hash>::new();
        for item in self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::Start)
            .flatten()
        {
            let (key, value) = item;
            leaves.insert(key.to_vec(), Hash::hash_two_parts(&key, &value));
        }
        for (id, record) in &batch.restriction_overlay {
            match serialize_legacy_bincode(record, "restriction") {
                Ok(value) => {
                    let key = id.to_be_bytes().to_vec();
                    leaves.insert(key.clone(), Hash::hash_two_parts(&key, &value));
                }
                Err(err) => tracing::warn!("Failed to overlay restriction in state root: {}", err),
            }
        }
        if leaves.is_empty() {
            return Hash::default();
        }
        let ordered: Vec<Hash> = leaves.into_values().collect();
        Self::merkle_root_from_leaves(&ordered)
    }

    fn compute_cf_kv_root(&self, cf_name: &str) -> Hash {
        let Some(cf) = self.db.cf_handle(cf_name) else {
            return Hash::default();
        };
        let mut leaves = Vec::<Hash>::new();
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        for item in self
            .db
            .iterator_cf_opt(&cf, read_opts, rocksdb::IteratorMode::Start)
            .flatten()
        {
            let (key, value) = item;
            leaves.push(Hash::hash_two_parts(&key, &value));
        }
        Self::merkle_root_from_leaves(&leaves)
    }

    fn collect_cf_kv_leaf_map(&self, cf_name: &str) -> BTreeMap<Vec<u8>, Hash> {
        let Some(cf) = self.db.cf_handle(cf_name) else {
            return BTreeMap::new();
        };
        let mut leaves = BTreeMap::<Vec<u8>, Hash>::new();
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        for item in self
            .db
            .iterator_cf_opt(&cf, read_opts, rocksdb::IteratorMode::Start)
            .flatten()
        {
            let (key, value) = item;
            let key = key.to_vec();
            leaves.insert(key.clone(), Hash::hash_two_parts(&key, &value));
        }
        leaves
    }

    fn root_from_leaf_map(leaves: BTreeMap<Vec<u8>, Hash>) -> Hash {
        let ordered: Vec<Hash> = leaves.into_values().collect();
        Self::merkle_root_from_leaves(&ordered)
    }

    fn compose_shielded_state_root(
        pool_root: Hash,
        commitments_root: Hash,
        note_payloads_root: Hash,
        nullifiers_root: Hash,
    ) -> Hash {
        let mut composite = Vec::with_capacity(1 + 32 * 4);
        composite.push(SHIELDED_STATE_ROOT_PREFIX);
        composite.extend_from_slice(&pool_root.0);
        composite.extend_from_slice(&commitments_root.0);
        composite.extend_from_slice(&note_payloads_root.0);
        composite.extend_from_slice(&nullifiers_root.0);
        Hash::hash(&composite)
    }

    pub fn compute_shielded_state_root(&self) -> Hash {
        Self::compose_shielded_state_root(
            self.compute_cf_kv_root(CF_SHIELDED_POOL),
            self.compute_cf_kv_root(CF_SHIELDED_COMMITMENTS),
            self.compute_cf_kv_root(CF_SHIELDED_NOTE_PAYLOADS),
            self.compute_cf_kv_root(CF_SHIELDED_NULLIFIERS),
        )
    }

    fn compute_shielded_state_root_for_batch(&self, batch: &StateBatch) -> Hash {
        let pool_root = if let Some(pool) = &batch.shielded_pool_overlay {
            let mut leaves = self.collect_cf_kv_leaf_map(CF_SHIELDED_POOL);
            match serde_json::to_vec(pool) {
                Ok(value) => {
                    let key = b"state".to_vec();
                    leaves.insert(key.clone(), Hash::hash_two_parts(&key, &value));
                }
                Err(err) => tracing::warn!("Failed to overlay shielded pool root: {}", err),
            }
            Self::root_from_leaf_map(leaves)
        } else {
            self.compute_cf_kv_root(CF_SHIELDED_POOL)
        };

        let commitments_root = if batch.shielded_commitment_overlay.is_empty() {
            self.compute_cf_kv_root(CF_SHIELDED_COMMITMENTS)
        } else {
            let mut leaves = self.collect_cf_kv_leaf_map(CF_SHIELDED_COMMITMENTS);
            for (index, commitment) in &batch.shielded_commitment_overlay {
                let key = index.to_be_bytes().to_vec();
                leaves.insert(key.clone(), Hash::hash_two_parts(&key, commitment));
            }
            Self::root_from_leaf_map(leaves)
        };

        let note_payloads_root = if batch.shielded_note_payload_overlay.is_empty() {
            self.compute_cf_kv_root(CF_SHIELDED_NOTE_PAYLOADS)
        } else {
            let mut leaves = self.collect_cf_kv_leaf_map(CF_SHIELDED_NOTE_PAYLOADS);
            for (index, payload) in &batch.shielded_note_payload_overlay {
                let key = index.to_be_bytes().to_vec();
                leaves.insert(key.clone(), Hash::hash_two_parts(&key, payload));
            }
            Self::root_from_leaf_map(leaves)
        };

        let nullifiers_root = if batch.spent_nullifier_overlay.is_empty() {
            self.compute_cf_kv_root(CF_SHIELDED_NULLIFIERS)
        } else {
            let mut leaves = self.collect_cf_kv_leaf_map(CF_SHIELDED_NULLIFIERS);
            for nullifier in &batch.spent_nullifier_overlay {
                let key = nullifier.to_vec();
                leaves.insert(key.clone(), Hash::hash_two_parts(&key, &[0x01]));
            }
            Self::root_from_leaf_map(leaves)
        };

        Self::compose_shielded_state_root(
            pool_root,
            commitments_root,
            note_payloads_root,
            nullifiers_root,
        )
    }

    pub fn compute_stake_pool_hash(&self) -> Hash {
        match self.get_stake_pool() {
            Ok(pool) => pool.canonical_hash(),
            Err(_) => Hash::default(),
        }
    }

    pub fn compute_mossstake_pool_hash(&self) -> Hash {
        match self.get_mossstake_pool() {
            Ok(pool) => {
                if self.is_mossstake_slot_only() {
                    pool.canonical_hash()
                } else {
                    pool.legacy_canonical_hash()
                }
            }
            Err(_) => Hash::default(),
        }
    }

    fn dirty_account_keys(&self) -> Vec<[u8; 32]> {
        let Some(cf_stats) = self.db.cf_handle(CF_STATS) else {
            return Vec::new();
        };
        let dirty_prefix = b"dirty_acct:";
        let iter = self.db.iterator_cf(
            &cf_stats,
            rocksdb::IteratorMode::From(dirty_prefix, Direction::Forward),
        );
        let mut dirty_keys = Vec::new();
        for item in iter.flatten() {
            let (key, _) = item;
            if !key.starts_with(dirty_prefix) {
                break;
            }
            if key.len() == dirty_prefix.len() + 32 {
                let mut pk = [0u8; 32];
                pk.copy_from_slice(&key[dirty_prefix.len()..]);
                dirty_keys.push(pk);
            }
        }
        dirty_keys
    }

    fn dirty_contract_storage_keys(&self) -> Vec<Vec<u8>> {
        let Some(cf_stats) = self.db.cf_handle(CF_STATS) else {
            return Vec::new();
        };
        let dirty_prefix = b"dirty_cstor:";
        let iter = self.db.iterator_cf(
            &cf_stats,
            rocksdb::IteratorMode::From(dirty_prefix, Direction::Forward),
        );
        let mut dirty_keys = Vec::new();
        for item in iter.flatten() {
            let (key, _) = item;
            if !key.starts_with(dirty_prefix) {
                break;
            }
            dirty_keys.push(key[dirty_prefix.len()..].to_vec());
        }
        dirty_keys
    }

    fn compute_sparse_accounts_root(&self) -> Result<Hash, String> {
        if !self.is_sparse_state_commitment_ready() {
            let _ = self.rebuild_sparse_state_commitment(false)?;
        }

        let dirty_keys = self.dirty_account_keys();
        let current_root = self.read_stats_hash_or_default(SPARSE_ACCOUNTS_ROOT_KEY);
        if dirty_keys.is_empty()
            && self.can_use_cached_subroot(b"dirty_account_count", b"dirty_acct:")
        {
            return Ok(current_root);
        }

        let cf_accounts = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;
        let cf_leaves = self
            .db
            .cf_handle(CF_MERKLE_LEAVES)
            .ok_or_else(|| "Account Merkle leaves CF not found".to_string())?;
        let cf_stats = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut changes = Vec::<SparseLeafChange>::with_capacity(dirty_keys.len());
        let mut leaf_count = self.read_stats_u64(SPARSE_ACCOUNTS_LEAF_COUNT_KEY);
        for pk in &dirty_keys {
            let old_exists = self
                .db
                .get_cf(&cf_leaves, pk)
                .map_err(|e| format!("Failed to read account leaf cache: {e}"))?
                .is_some();
            let leaf_hash = match self.db.get_cf(&cf_accounts, pk) {
                Ok(Some(value)) if !Self::deserialize_account_check_dormant(&value) => {
                    Some(Hash::hash_two_parts(pk, &value))
                }
                Ok(Some(_)) | Ok(None) => None,
                Err(e) => return Err(format!("Failed to read dirty account: {e}")),
            };
            match (old_exists, leaf_hash.is_some()) {
                (false, true) => leaf_count = leaf_count.saturating_add(1),
                (true, false) => leaf_count = leaf_count.saturating_sub(1),
                _ => {}
            }
            changes.push(SparseLeafChange {
                leaf_key: pk.to_vec(),
                path: Self::sparse_account_path(&Pubkey(*pk)),
                leaf_hash,
            });
        }

        let (root, overlay) =
            self.sparse_root_with_changes(CF_ACCOUNT_MERKLE_NODES, current_root, &changes)?;
        let mut batch = WriteBatch::default();
        self.write_sparse_overlay_nodes(CF_ACCOUNT_MERKLE_NODES, &overlay, &mut batch)?;
        for change in &changes {
            match change.leaf_hash {
                Some(hash) => batch.put_cf(&cf_leaves, &change.leaf_key, hash.0),
                None => batch.delete_cf(&cf_leaves, &change.leaf_key),
            }
            let mut dirty_key = [0u8; 43];
            dirty_key[..11].copy_from_slice(b"dirty_acct:");
            dirty_key[11..43].copy_from_slice(&change.leaf_key);
            batch.delete_cf(&cf_stats, dirty_key);
        }
        batch.put_cf(&cf_stats, SPARSE_ACCOUNTS_ROOT_KEY, root.0);
        batch.put_cf(
            &cf_stats,
            SPARSE_ACCOUNTS_LEAF_COUNT_KEY,
            leaf_count.to_le_bytes(),
        );
        batch.put_cf(&cf_stats, b"merkle_leaf_count", leaf_count.to_le_bytes());
        batch.put_cf(&cf_stats, b"dirty_account_count", 0u64.to_le_bytes());
        batch.delete_cf(&cf_stats, CACHED_ACCOUNTS_ROOT_KEY);
        batch.delete_cf(&cf_stats, CACHED_STATE_ROOT_KEY);
        batch.delete_cf(&cf_stats, CACHED_STATE_ROOT_SCHEMA_KEY);
        batch.delete_cf(&cf_stats, CACHED_STATE_COMMITMENT_SCHEMA_KEY);
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to write sparse account root update: {e}"))?;
        Ok(root)
    }

    fn compute_sparse_contract_storage_root(&self) -> Result<Hash, String> {
        if !self.is_sparse_state_commitment_ready() {
            let _ = self.rebuild_sparse_state_commitment(false)?;
        }

        let dirty_keys = self.dirty_contract_storage_keys();
        let current_root = self.read_stats_hash_or_default(SPARSE_CONTRACT_ROOT_KEY);
        if dirty_keys.is_empty()
            && self.can_use_cached_subroot(b"dirty_contract_count", b"dirty_cstor:")
        {
            return Ok(current_root);
        }

        let cf_storage = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;
        let cf_leaves = self
            .db
            .cf_handle(CF_CONTRACT_MERKLE_LEAVES)
            .ok_or_else(|| "Contract Merkle leaves CF not found".to_string())?;
        let cf_stats = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut changes = Vec::<SparseLeafChange>::with_capacity(dirty_keys.len());
        let mut leaf_count = self.read_stats_u64(SPARSE_CONTRACT_LEAF_COUNT_KEY);
        for full_key in &dirty_keys {
            let old_exists = self
                .db
                .get_cf(&cf_leaves, full_key)
                .map_err(|e| format!("Failed to read contract leaf cache: {e}"))?
                .is_some();
            let leaf_hash = match self.db.get_cf(&cf_storage, full_key) {
                Ok(Some(value)) => Some(Hash::hash_two_parts(full_key, &value)),
                Ok(None) => None,
                Err(e) => return Err(format!("Failed to read dirty contract storage: {e}")),
            };
            match (old_exists, leaf_hash.is_some()) {
                (false, true) => leaf_count = leaf_count.saturating_add(1),
                (true, false) => leaf_count = leaf_count.saturating_sub(1),
                _ => {}
            }
            changes.push(SparseLeafChange {
                leaf_key: full_key.clone(),
                path: Self::sparse_contract_path(full_key),
                leaf_hash,
            });
        }

        let (root, overlay) =
            self.sparse_root_with_changes(CF_CONTRACT_MERKLE_NODES, current_root, &changes)?;
        let mut batch = WriteBatch::default();
        self.write_sparse_overlay_nodes(CF_CONTRACT_MERKLE_NODES, &overlay, &mut batch)?;
        for change in &changes {
            match change.leaf_hash {
                Some(hash) => batch.put_cf(&cf_leaves, &change.leaf_key, hash.0),
                None => batch.delete_cf(&cf_leaves, &change.leaf_key),
            }
            let mut marker_key = Vec::with_capacity(b"dirty_cstor:".len() + change.leaf_key.len());
            marker_key.extend_from_slice(b"dirty_cstor:");
            marker_key.extend_from_slice(&change.leaf_key);
            batch.delete_cf(&cf_stats, &marker_key);
        }
        batch.put_cf(&cf_stats, SPARSE_CONTRACT_ROOT_KEY, root.0);
        batch.put_cf(
            &cf_stats,
            SPARSE_CONTRACT_LEAF_COUNT_KEY,
            leaf_count.to_le_bytes(),
        );
        batch.put_cf(
            &cf_stats,
            b"contract_merkle_leaf_count",
            leaf_count.to_le_bytes(),
        );
        batch.put_cf(&cf_stats, b"dirty_contract_count", 0u64.to_le_bytes());
        batch.delete_cf(&cf_stats, CACHED_CONTRACT_ROOT_KEY);
        batch.delete_cf(&cf_stats, CACHED_STATE_ROOT_KEY);
        batch.delete_cf(&cf_stats, CACHED_STATE_ROOT_SCHEMA_KEY);
        batch.delete_cf(&cf_stats, CACHED_STATE_COMMITMENT_SCHEMA_KEY);
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to write sparse contract root update: {e}"))?;
        Ok(root)
    }

    fn compute_sparse_accounts_root_for_batch(&self, batch: &StateBatch) -> Result<Hash, String> {
        let canonical_root = self.compute_sparse_accounts_root()?;
        if batch.account_overlay.is_empty() {
            return Ok(canonical_root);
        }
        let mut changes = Vec::<SparseLeafChange>::with_capacity(batch.account_overlay.len());
        for (pubkey, account) in &batch.account_overlay {
            let leaf_hash = if account.dormant {
                None
            } else {
                match Self::serialized_account_value(account) {
                    Ok(value) => Some(Hash::hash_two_parts(&pubkey.0, &value)),
                    Err(err) => {
                        tracing::warn!("Failed to overlay account in sparse state root: {err}");
                        continue;
                    }
                }
            };
            changes.push(SparseLeafChange {
                leaf_key: pubkey.0.to_vec(),
                path: Self::sparse_account_path(pubkey),
                leaf_hash,
            });
        }
        let (root, _) =
            self.sparse_root_with_changes(CF_ACCOUNT_MERKLE_NODES, canonical_root, &changes)?;
        Ok(root)
    }

    fn compute_sparse_contract_storage_root_for_batch(
        &self,
        batch: &StateBatch,
    ) -> Result<Hash, String> {
        let canonical_root = self.compute_sparse_contract_storage_root()?;
        if batch.contract_storage_overlay.is_empty() {
            return Ok(canonical_root);
        }
        let mut changes =
            Vec::<SparseLeafChange>::with_capacity(batch.contract_storage_overlay.len());
        for (full_key, value) in &batch.contract_storage_overlay {
            changes.push(SparseLeafChange {
                leaf_key: full_key.clone(),
                path: Self::sparse_contract_path(full_key),
                leaf_hash: value
                    .as_ref()
                    .map(|value| Hash::hash_two_parts(full_key, value)),
            });
        }
        let (root, _) =
            self.sparse_root_with_changes(CF_CONTRACT_MERKLE_NODES, canonical_root, &changes)?;
        Ok(root)
    }

    pub fn compute_accounts_root(&self) -> Hash {
        if self.uses_sparse_state_commitment() {
            return match self.compute_sparse_accounts_root() {
                Ok(root) => root,
                Err(err) => {
                    tracing::error!("Sparse account root computation failed: {err}");
                    self.compute_accounts_root_full_scan()
                }
            };
        }
        if self.is_sparse_state_commitment_ready() {
            if let Err(err) = self.compute_sparse_accounts_root() {
                tracing::warn!("Sparse account shadow update failed: {err}");
            }
        }

        let cf_accounts = match self.db.cf_handle(CF_ACCOUNTS) {
            Some(handle) => handle,
            None => return Hash::default(),
        };
        let cf_leaves = match self.db.cf_handle(CF_MERKLE_LEAVES) {
            Some(handle) => handle,
            None => return self.compute_accounts_root_full_scan(),
        };
        let cf_stats = match self.db.cf_handle(CF_STATS) {
            Some(handle) => handle,
            None => return self.compute_accounts_root_full_scan(),
        };

        let leaf_count = match self.db.get_cf(&cf_stats, b"merkle_leaf_count") {
            Ok(Some(data)) if data.len() == 8 => {
                u64::from_le_bytes(data.as_slice().try_into().unwrap_or([0; 8]))
            }
            _ => 0,
        };

        if leaf_count == 0 {
            return self.compute_accounts_root_cold_start();
        }

        let dirty_prefix = b"dirty_acct:";
        if self.can_use_cached_subroot(b"dirty_account_count", dirty_prefix) {
            if let Some(root) = self.read_cached_hash(CACHED_ACCOUNTS_ROOT_KEY) {
                return root;
            }
        }

        let iter = self.db.iterator_cf(
            &cf_stats,
            rocksdb::IteratorMode::From(dirty_prefix, Direction::Forward),
        );

        let mut dirty_keys: Vec<[u8; 32]> = Vec::new();
        for item in iter.flatten() {
            let (key, _) = item;
            if !key.starts_with(dirty_prefix) {
                break;
            }
            if key.len() == dirty_prefix.len() + 32 {
                let mut pk = [0u8; 32];
                pk.copy_from_slice(&key[dirty_prefix.len()..]);
                dirty_keys.push(pk);
            }
        }

        let mut batch = WriteBatch::default();
        for pk in &dirty_keys {
            match self.db.get_cf(&cf_accounts, pk) {
                Ok(Some(value)) => {
                    let is_dormant = Self::deserialize_account_check_dormant(&value);
                    if is_dormant {
                        batch.delete_cf(&cf_leaves, pk);
                    } else {
                        let leaf = Hash::hash_two_parts(pk, &value);
                        batch.put_cf(&cf_leaves, pk, leaf.0);
                    }
                }
                Ok(None) => {
                    batch.delete_cf(&cf_leaves, pk);
                }
                Err(_) => continue,
            }
            let mut dirty_key = [0u8; 43];
            dirty_key[..11].copy_from_slice(dirty_prefix);
            dirty_key[11..43].copy_from_slice(pk);
            batch.delete_cf(&cf_stats, dirty_key);
        }
        batch.put_cf(&cf_stats, b"dirty_account_count", 0u64.to_le_bytes());

        if let Err(e) = self.db.write(batch) {
            tracing::error!("Failed to write Merkle leaf updates: {}", e);
            return self.compute_accounts_root_full_scan();
        }

        let mut leaves: Vec<Hash> = Vec::new();
        let iter = self
            .db
            .iterator_cf(&cf_leaves, rocksdb::IteratorMode::Start);
        for item in iter.flatten() {
            let (_, value) = item;
            if value.len() == 32 {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&value);
                leaves.push(Hash(bytes));
            }
        }

        if leaves.is_empty() {
            let root = Hash::default();
            self.cache_subroot(CACHED_ACCOUNTS_ROOT_KEY, &root);
            return root;
        }

        let root = Self::merkle_root_from_leaves(&leaves);
        self.cache_subroot(CACHED_ACCOUNTS_ROOT_KEY, &root);
        root
    }

    pub fn compute_contract_storage_root(&self) -> Hash {
        if self.uses_sparse_state_commitment() {
            return match self.compute_sparse_contract_storage_root() {
                Ok(root) => root,
                Err(err) => {
                    tracing::error!("Sparse contract root computation failed: {err}");
                    self.compute_contract_storage_root_full_scan()
                }
            };
        }
        if self.is_sparse_state_commitment_ready() {
            if let Err(err) = self.compute_sparse_contract_storage_root() {
                tracing::warn!("Sparse contract shadow update failed: {err}");
            }
        }

        let cf_storage = match self.db.cf_handle(CF_CONTRACT_STORAGE) {
            Some(h) => h,
            None => return Hash::default(),
        };
        let cf_cs_leaves = match self.db.cf_handle(CF_CONTRACT_MERKLE_LEAVES) {
            Some(h) => h,
            None => return self.compute_contract_storage_root_full_scan(),
        };
        let cf_stats = match self.db.cf_handle(CF_STATS) {
            Some(h) => h,
            None => return self.compute_contract_storage_root_full_scan(),
        };

        let leaf_count = match self.db.get_cf(&cf_stats, b"contract_merkle_leaf_count") {
            Ok(Some(data)) if data.len() == 8 => {
                u64::from_le_bytes(data.as_slice().try_into().unwrap_or([0; 8]))
            }
            _ => 0,
        };

        if leaf_count == 0 {
            return self.compute_contract_storage_root_cold_start();
        }

        let dirty_prefix = b"dirty_cstor:";
        if self.can_use_cached_subroot(b"dirty_contract_count", dirty_prefix) {
            if let Some(root) = self.read_cached_hash(CACHED_CONTRACT_ROOT_KEY) {
                return root;
            }
        }

        let iter = self.db.iterator_cf(
            &cf_stats,
            rocksdb::IteratorMode::From(dirty_prefix, Direction::Forward),
        );

        let mut dirty_keys: Vec<Vec<u8>> = Vec::new();
        for item in iter.flatten() {
            let (key, _) = item;
            if !key.starts_with(dirty_prefix) {
                break;
            }
            dirty_keys.push(key[dirty_prefix.len()..].to_vec());
        }

        let mut batch = WriteBatch::default();
        for full_key in &dirty_keys {
            match self.db.get_cf(&cf_storage, full_key) {
                Ok(Some(value)) => {
                    let leaf = Hash::hash_two_parts(full_key, &value);
                    batch.put_cf(&cf_cs_leaves, full_key, leaf.0);
                }
                Ok(None) => {
                    batch.delete_cf(&cf_cs_leaves, full_key);
                }
                Err(_) => continue,
            }
            let mut marker_key = Vec::with_capacity(dirty_prefix.len() + full_key.len());
            marker_key.extend_from_slice(dirty_prefix);
            marker_key.extend_from_slice(full_key);
            batch.delete_cf(&cf_stats, &marker_key);
        }
        batch.put_cf(&cf_stats, b"dirty_contract_count", 0u64.to_le_bytes());

        if let Err(e) = self.db.write(batch) {
            tracing::error!("Failed to write contract Merkle leaf updates: {}", e);
            return self.compute_contract_storage_root_full_scan();
        }

        let mut leaves: Vec<Hash> = Vec::new();
        let iter = self
            .db
            .iterator_cf(&cf_cs_leaves, rocksdb::IteratorMode::Start);
        for item in iter.flatten() {
            let (_, value) = item;
            if value.len() == 32 {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&value);
                leaves.push(Hash(bytes));
            }
        }

        if leaves.is_empty() {
            let root = Hash::default();
            self.cache_subroot(CACHED_CONTRACT_ROOT_KEY, &root);
            return root;
        }

        let root = Self::merkle_root_from_leaves(&leaves);
        self.cache_subroot(CACHED_CONTRACT_ROOT_KEY, &root);
        root
    }

    fn compute_contract_storage_root_cold_start(&self) -> Hash {
        if self.uses_sparse_state_commitment() {
            return match self.rebuild_sparse_contract_commitment() {
                Ok((root, _, _)) => root,
                Err(err) => {
                    tracing::error!("Sparse contract cold-start rebuild failed: {err}");
                    self.compute_contract_storage_root_full_scan()
                }
            };
        }

        let cf_storage = match self.db.cf_handle(CF_CONTRACT_STORAGE) {
            Some(h) => h,
            None => return Hash::default(),
        };
        let cf_cs_leaves = match self.db.cf_handle(CF_CONTRACT_MERKLE_LEAVES) {
            Some(h) => h,
            None => return self.compute_contract_storage_root_full_scan(),
        };

        let mut leaves: Vec<Hash> = Vec::new();
        let mut batch = WriteBatch::default();
        let mut count = 0u64;

        let clear_iter = self
            .db
            .iterator_cf(&cf_cs_leaves, rocksdb::IteratorMode::Start);
        for item in clear_iter.flatten() {
            let (key, _) = item;
            batch.delete_cf(&cf_cs_leaves, &*key);
        }

        let iter = self
            .db
            .iterator_cf(&cf_storage, rocksdb::IteratorMode::Start);
        for item in iter.flatten() {
            let (key, value) = item;
            let leaf = Hash::hash_two_parts(&key, &value);
            leaves.push(leaf);
            batch.put_cf(&cf_cs_leaves, &*key, leaf.0);
            count += 1;
        }

        let root = Self::merkle_root_from_leaves(&leaves);
        if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
            batch.put_cf(
                &cf_stats,
                b"contract_merkle_leaf_count",
                count.to_le_bytes(),
            );
            batch.put_cf(&cf_stats, b"dirty_contract_count", 0u64.to_le_bytes());
            batch.put_cf(&cf_stats, CACHED_CONTRACT_ROOT_KEY, root.0);
        }
        if let Err(e) = self.db.write(batch) {
            tracing::error!("Failed to write contract Merkle leaf cache: {e}");
        }

        root
    }

    fn compute_contract_storage_root_full_scan(&self) -> Hash {
        let cf = match self.db.cf_handle(CF_CONTRACT_STORAGE) {
            Some(h) => h,
            None => return Hash::default(),
        };

        let mut leaves: Vec<Hash> = Vec::new();
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        for (key, value) in iter.flatten() {
            leaves.push(Hash::hash_two_parts(&key, &value));
        }

        if leaves.is_empty() {
            return Hash::default();
        }

        Self::merkle_root_from_leaves(&leaves)
    }

    pub fn compute_state_root_cold_start(&self) -> Hash {
        let accounts_root = self.compute_accounts_root_cold_start();
        let contract_root = self.compute_contract_storage_root_cold_start();
        let stake_pool_hash = self.compute_stake_pool_hash();
        let mossstake_pool_hash = self.compute_mossstake_pool_hash();
        let include_restrictions = self.get_state_root_schema().unwrap_or(false);
        let root = self.compose_state_root(
            accounts_root,
            contract_root,
            stake_pool_hash,
            mossstake_pool_hash,
            include_restrictions,
        );
        self.cache_state_root(&root, include_restrictions);
        root
    }

    pub fn invalidate_merkle_cache(&self) {
        if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
            if let Err(e) = self
                .db
                .put_cf(&cf_stats, b"merkle_leaf_count", 0u64.to_le_bytes())
            {
                tracing::error!("Failed to invalidate account Merkle cache: {e}");
            }
            if let Err(e) =
                self.db
                    .put_cf(&cf_stats, b"contract_merkle_leaf_count", 0u64.to_le_bytes())
            {
                tracing::error!("Failed to invalidate contract Merkle cache: {e}");
            }
            for key in [
                CACHED_STATE_ROOT_KEY,
                CACHED_STATE_ROOT_SCHEMA_KEY,
                CACHED_STATE_COMMITMENT_SCHEMA_KEY,
                CACHED_ACCOUNTS_ROOT_KEY,
                CACHED_CONTRACT_ROOT_KEY,
            ] {
                if let Err(e) = self.db.delete_cf(&cf_stats, key) {
                    tracing::warn!("Failed to clear cached Merkle root during invalidation: {e}");
                }
            }
            tracing::info!(
                "🔄 Merkle leaf cache invalidated — cold start will run on next state root computation"
            );
        }
    }

    fn compute_accounts_root_cold_start(&self) -> Hash {
        if self.uses_sparse_state_commitment() {
            return match self.rebuild_sparse_accounts_commitment() {
                Ok((root, _, _)) => root,
                Err(err) => {
                    tracing::error!("Sparse account cold-start rebuild failed: {err}");
                    self.compute_accounts_root_full_scan()
                }
            };
        }

        let cf_accounts = match self.db.cf_handle(CF_ACCOUNTS) {
            Some(h) => h,
            None => return Hash::default(),
        };
        let cf_leaves = match self.db.cf_handle(CF_MERKLE_LEAVES) {
            Some(h) => h,
            None => return self.compute_accounts_root_full_scan(),
        };

        let mut leaves: Vec<Hash> = Vec::new();
        let mut batch = WriteBatch::default();
        let mut count = 0u64;

        let iter = self
            .db
            .iterator_cf(&cf_leaves, rocksdb::IteratorMode::Start);
        for item in iter.flatten() {
            let (key, _) = item;
            batch.delete_cf(&cf_leaves, &*key);
        }

        let iter = self
            .db
            .iterator_cf(&cf_accounts, rocksdb::IteratorMode::Start);
        for item in iter.flatten() {
            let (key, value) = item;
            if Self::deserialize_account_check_dormant(&value) {
                continue;
            }
            let leaf = Hash::hash_two_parts(&key, &value);
            leaves.push(leaf);
            batch.put_cf(&cf_leaves, &*key, leaf.0);
            count += 1;
        }

        let root = Self::merkle_root_from_leaves(&leaves);
        if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
            batch.put_cf(&cf_stats, b"merkle_leaf_count", count.to_le_bytes());
            batch.put_cf(&cf_stats, b"dirty_account_count", 0u64.to_le_bytes());
            batch.put_cf(&cf_stats, CACHED_ACCOUNTS_ROOT_KEY, root.0);
        }
        if let Err(e) = self.db.write(batch) {
            tracing::error!("Failed to write account Merkle leaf cache: {e}");
        }

        root
    }

    fn compute_accounts_root_full_scan(&self) -> Hash {
        let cf = match self.db.cf_handle(CF_ACCOUNTS) {
            Some(handle) => handle,
            None => return Hash::default(),
        };

        let mut leaves: Vec<Hash> = Vec::new();
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        for (key, value) in iter.flatten() {
            if Self::deserialize_account_check_dormant(&value) {
                continue;
            }
            leaves.push(Hash::hash_two_parts(&key, &value));
        }

        if leaves.is_empty() {
            return Hash::default();
        }

        let root = Self::merkle_root_from_leaves(&leaves);

        if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
            if let Err(e) = self
                .db
                .put_cf(&cf_stats, b"dirty_account_count", 0u64.to_le_bytes())
            {
                tracing::error!("Failed to reset dirty_account_count: {e}");
            }
        }

        root
    }

    pub(crate) fn deserialize_account_check_dormant(raw: &[u8]) -> bool {
        let data = if raw.first() == Some(&0xBC) {
            &raw[1..]
        } else {
            raw
        };
        match deserialize_legacy_bincode::<Account>(data, "account") {
            Ok(account) => account.dormant,
            Err(_) => false,
        }
    }

    pub(crate) fn merkle_root_from_leaves(leaves: &[Hash]) -> Hash {
        if leaves.is_empty() {
            return Hash::default();
        }
        if leaves.len() == 1 {
            return leaves[0];
        }

        let mut buf_a: Vec<Hash> = Vec::with_capacity(leaves.len().div_ceil(2));
        let mut buf_b: Vec<Hash> = Vec::with_capacity(leaves.len().div_ceil(4).max(1));
        let mut combined = [0u8; 64];

        for pair in leaves.chunks(2) {
            combined[..32].copy_from_slice(&pair[0].0);
            if pair.len() == 2 {
                combined[32..].copy_from_slice(&pair[1].0);
            } else {
                combined[32..].copy_from_slice(&pair[0].0);
            }
            buf_a.push(Hash::hash(&combined));
        }

        let mut use_a = true;
        while (if use_a { &buf_a } else { &buf_b }).len() > 1 {
            let (src, dst) = if use_a {
                (&buf_a as &Vec<Hash>, &mut buf_b)
            } else {
                (&buf_b as &Vec<Hash>, &mut buf_a)
            };
            dst.clear();
            for pair in src.chunks(2) {
                combined[..32].copy_from_slice(&pair[0].0);
                if pair.len() == 2 {
                    combined[32..].copy_from_slice(&pair[1].0);
                } else {
                    combined[32..].copy_from_slice(&pair[0].0);
                }
                dst.push(Hash::hash(&combined));
            }
            use_a = !use_a;
        }

        if use_a {
            buf_a[0]
        } else {
            buf_b[0]
        }
    }

    pub fn compute_state_root_cached(&self) -> Hash {
        if let Some(cf) = self.db.cf_handle(CF_STATS) {
            let include_restrictions = self.get_state_root_schema().unwrap_or(false);
            if include_restrictions {
                return self.compute_state_root();
            }

            if self.can_use_cached_subroot(b"dirty_account_count", b"dirty_acct:")
                && self.can_use_cached_subroot(b"dirty_contract_count", b"dirty_cstor:")
                && self.cached_state_root_schema() == Some(include_restrictions)
                && self.cached_state_commitment_schema() == Some(self.get_state_commitment_schema())
            {
                if let Ok(Some(data)) = self.db.get_cf(&cf, CACHED_STATE_ROOT_KEY) {
                    if data.len() == 32 {
                        let mut bytes = [0u8; 32];
                        bytes.copy_from_slice(&data);
                        return Hash(bytes);
                    }
                }
            }
        }

        self.compute_state_root()
    }

    pub(crate) fn stage_account_dirty_marker(
        &self,
        batch: &mut WriteBatch,
        pubkey: &Pubkey,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let mut key = [0u8; 43];
        key[..11].copy_from_slice(b"dirty_acct:");
        key[11..43].copy_from_slice(&pubkey.0);
        batch.put_cf(&cf, key, []);
        batch.put_cf(&cf, b"dirty_account_count", 1u64.to_le_bytes());
        Ok(())
    }

    pub(crate) fn stage_contract_storage_dirty_marker(
        &self,
        batch: &mut WriteBatch,
        full_key: &[u8],
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let prefix = b"dirty_cstor:";
        let mut dirty_key = Vec::with_capacity(prefix.len() + full_key.len());
        dirty_key.extend_from_slice(prefix);
        dirty_key.extend_from_slice(full_key);
        batch.put_cf(&cf, &dirty_key, []);
        batch.put_cf(&cf, b"dirty_contract_count", 1u64.to_le_bytes());
        Ok(())
    }

    pub fn mark_account_dirty_with_key(&self, pubkey: &Pubkey) {
        if let Some(cf) = self.db.cf_handle(CF_STATS) {
            let mut key = [0u8; 43];
            key[..11].copy_from_slice(b"dirty_acct:");
            key[11..43].copy_from_slice(&pubkey.0);
            if let Err(e) = self.db.put_cf(&cf, key, []) {
                tracing::warn!("Failed to write dirty_acct marker: {}", e);
            }

            if let Err(e) = self
                .db
                .put_cf(&cf, b"dirty_account_count", 1u64.to_le_bytes())
            {
                tracing::warn!("Failed to write dirty_account_count: {}", e);
            }

            if let Err(e) = self.db.put_cf(&cf, SPARSE_DIRTY_MARKERS_ATOMIC_KEY, b"0") {
                tracing::warn!("Failed to mark sparse dirty markers untrusted: {}", e);
            }
        }
    }

    pub fn mark_contract_storage_dirty(&self, full_key: &[u8]) {
        if let Some(cf) = self.db.cf_handle(CF_STATS) {
            let prefix = b"dirty_cstor:";
            let mut dirty_key = Vec::with_capacity(prefix.len() + full_key.len());
            dirty_key.extend_from_slice(prefix);
            dirty_key.extend_from_slice(full_key);
            if let Err(e) = self.db.put_cf(&cf, &dirty_key, []) {
                tracing::warn!("Failed to write dirty_cstor marker: {}", e);
            }
            if let Err(e) = self
                .db
                .put_cf(&cf, b"dirty_contract_count", 1u64.to_le_bytes())
            {
                tracing::warn!("Failed to write dirty_contract_count: {}", e);
            }
            if let Err(e) = self.db.put_cf(&cf, SPARSE_DIRTY_MARKERS_ATOMIC_KEY, b"0") {
                tracing::warn!("Failed to mark sparse dirty markers untrusted: {}", e);
            }
        }
    }
}
