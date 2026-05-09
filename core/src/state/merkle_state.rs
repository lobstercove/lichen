use rocksdb::Direction;
use serde::{Deserialize, Serialize};

use super::*;

const STATE_ROOT_PREFIX_WITH_RESTRICTIONS: u8 = 0x03;
const STATE_ROOT_PREFIX_LEGACY: u8 = 0x02;
const STATE_ROOT_SCHEMA_KEY: &[u8] = b"state_root_schema";
const CACHED_STATE_ROOT_SCHEMA_KEY: &[u8] = b"cached_state_root_schema";

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

/// Full account proof returned by `get_account_proof`, suitable for RPC responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountProof {
    pub pubkey: Pubkey,
    pub account_data: Vec<u8>,
    pub proof: MerkleProof,
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
    fn state_root_prefix(include_restrictions: bool) -> u8 {
        if include_restrictions {
            STATE_ROOT_PREFIX_WITH_RESTRICTIONS
        } else {
            STATE_ROOT_PREFIX_LEGACY
        }
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
        let mut composite =
            Vec::with_capacity(1 + 32 + 32 + 32 + 32 + restrictions_root.map_or(0, |_| 32));
        composite.push(Self::state_root_prefix(include_restrictions));
        composite.extend_from_slice(&accounts_root.0);
        composite.extend_from_slice(&contract_root.0);
        composite.extend_from_slice(&stake_pool_hash.0);
        composite.extend_from_slice(&mossstake_pool_hash.0);
        if let Some(restrictions_root) = restrictions_root {
            composite.extend_from_slice(&restrictions_root.0);
        }
        Hash::hash(&composite)
    }

    fn cache_state_root(&self, root: &Hash, include_restrictions: bool) {
        if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
            if let Err(e) = self.db.put_cf(&cf_stats, b"cached_state_root", root.0) {
                tracing::error!("Failed to cache state root: {e}");
            }
            if let Err(e) = self.db.put_cf(
                &cf_stats,
                CACHED_STATE_ROOT_SCHEMA_KEY,
                [u8::from(include_restrictions)],
            ) {
                tracing::error!("Failed to cache state-root schema: {e}");
            }
        }
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
                if let Err(e) = self.db.delete_cf(&cf_stats, b"cached_state_root") {
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
            STATE_ROOT_PREFIX_WITH_RESTRICTIONS,
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
            STATE_ROOT_PREFIX_LEGACY,
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

    /// Generate an inclusion proof for the given account.
    pub fn get_account_proof(&self, pubkey: &Pubkey) -> Option<AccountProof> {
        let cf_accounts = self.db.cf_handle(CF_ACCOUNTS)?;
        let account_data = self.db.get_cf(&cf_accounts, pubkey.0).ok()??;

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
        composite.push(Self::state_root_prefix(include_restrictions));
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
            Self::state_root_prefix(include_restrictions),
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
        let accounts_root = self.compute_accounts_root_for_batch(batch);
        let contract_root = self.compute_contract_storage_root_for_batch(batch);
        let stake_pool_hash = batch
            .stake_pool_overlay
            .as_ref()
            .map(|pool| pool.canonical_hash())
            .unwrap_or_else(|| self.compute_stake_pool_hash());
        let mossstake_pool_hash = batch
            .mossstake_pool_overlay
            .as_ref()
            .map(|pool| pool.canonical_hash())
            .unwrap_or_else(|| self.compute_mossstake_pool_hash());
        let include_restrictions = self.get_state_root_schema().unwrap_or(false);

        let restrictions_root = if include_restrictions && !batch.restriction_overlay.is_empty() {
            Some(self.compute_restrictions_root_for_batch(batch))
        } else if include_restrictions {
            Some(self.compute_restrictions_root())
        } else {
            None
        };

        let mut composite =
            Vec::with_capacity(1 + 32 + 32 + 32 + 32 + restrictions_root.map_or(0, |_| 32));
        composite.push(Self::state_root_prefix(include_restrictions));
        composite.extend_from_slice(&accounts_root.0);
        composite.extend_from_slice(&contract_root.0);
        composite.extend_from_slice(&stake_pool_hash.0);
        composite.extend_from_slice(&mossstake_pool_hash.0);
        if let Some(restrictions_root) = restrictions_root {
            composite.extend_from_slice(&restrictions_root.0);
        }
        Hash::hash(&composite)
    }

    fn serialized_account_value(account: &Account) -> Result<Vec<u8>, String> {
        let mut value = Vec::with_capacity(256);
        value.push(0xBC);
        bincode::serialize_into(&mut value, account)
            .map_err(|e| format!("Failed to serialize account: {}", e))?;
        Ok(value)
    }

    fn compute_accounts_root_for_batch(&self, batch: &StateBatch) -> Hash {
        // Make sure canonical dirty markers have been folded into leaf cache.
        let _ = self.compute_accounts_root();

        let cf_leaves = match self.db.cf_handle(CF_MERKLE_LEAVES) {
            Some(handle) => handle,
            None => return self.compute_accounts_root_full_scan(),
        };

        let mut leaves = std::collections::BTreeMap::<Vec<u8>, Hash>::new();
        for item in self
            .db
            .iterator_cf(&cf_leaves, rocksdb::IteratorMode::Start)
            .flatten()
        {
            let (key, value) = item;
            if value.len() == 32 {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&value);
                leaves.insert(key.to_vec(), Hash(bytes));
            }
        }

        for (pubkey, account) in &batch.account_overlay {
            if account.dormant {
                leaves.remove(pubkey.0.as_slice());
                continue;
            }
            match Self::serialized_account_value(account) {
                Ok(value) => {
                    leaves.insert(pubkey.0.to_vec(), Hash::hash_two_parts(&pubkey.0, &value));
                }
                Err(err) => tracing::warn!("Failed to overlay account in state root: {}", err),
            }
        }

        if leaves.is_empty() {
            return Hash::default();
        }

        let ordered: Vec<Hash> = leaves.into_values().collect();
        Self::merkle_root_from_leaves(&ordered)
    }

    fn compute_contract_storage_root_for_batch(&self, batch: &StateBatch) -> Hash {
        // Make sure canonical dirty markers have been folded into leaf cache.
        let _ = self.compute_contract_storage_root();

        let cf_leaves = match self.db.cf_handle(CF_CONTRACT_MERKLE_LEAVES) {
            Some(handle) => handle,
            None => return self.compute_contract_storage_root_full_scan(),
        };

        let mut leaves = std::collections::BTreeMap::<Vec<u8>, Hash>::new();
        for item in self
            .db
            .iterator_cf(&cf_leaves, rocksdb::IteratorMode::Start)
            .flatten()
        {
            let (key, value) = item;
            if value.len() == 32 {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&value);
                leaves.insert(key.to_vec(), Hash(bytes));
            }
        }

        for (full_key, value) in &batch.contract_storage_overlay {
            match value {
                Some(value) => {
                    leaves.insert(full_key.clone(), Hash::hash_two_parts(full_key, value));
                }
                None => {
                    leaves.remove(full_key.as_slice());
                }
            }
        }

        if leaves.is_empty() {
            return Hash::default();
        }

        let ordered: Vec<Hash> = leaves.into_values().collect();
        Self::merkle_root_from_leaves(&ordered)
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
            match bincode::serialize(record) {
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

    pub fn compute_stake_pool_hash(&self) -> Hash {
        match self.get_stake_pool() {
            Ok(pool) => pool.canonical_hash(),
            Err(_) => Hash::default(),
        }
    }

    pub fn compute_mossstake_pool_hash(&self) -> Hash {
        match self.get_mossstake_pool() {
            Ok(pool) => pool.canonical_hash(),
            Err(_) => Hash::default(),
        }
    }

    pub fn compute_accounts_root(&self) -> Hash {
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
            return Hash::default();
        }

        Self::merkle_root_from_leaves(&leaves)
    }

    pub fn compute_contract_storage_root(&self) -> Hash {
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
            return Hash::default();
        }

        Self::merkle_root_from_leaves(&leaves)
    }

    fn compute_contract_storage_root_cold_start(&self) -> Hash {
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

        if leaves.is_empty() {
            return Hash::default();
        }

        if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
            batch.put_cf(
                &cf_stats,
                b"contract_merkle_leaf_count",
                count.to_le_bytes(),
            );
            batch.put_cf(&cf_stats, b"dirty_contract_count", 0u64.to_le_bytes());
        }
        if let Err(e) = self.db.write(batch) {
            tracing::error!("Failed to write contract Merkle leaf cache: {e}");
        }

        Self::merkle_root_from_leaves(&leaves)
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
            tracing::info!(
                "🔄 Merkle leaf cache invalidated — cold start will run on next state root computation"
            );
        }
    }

    fn compute_accounts_root_cold_start(&self) -> Hash {
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

        if leaves.is_empty() {
            return Hash::default();
        }

        if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
            batch.put_cf(&cf_stats, b"merkle_leaf_count", count.to_le_bytes());
            batch.put_cf(&cf_stats, b"dirty_account_count", 0u64.to_le_bytes());
        }
        if let Err(e) = self.db.write(batch) {
            tracing::error!("Failed to write account Merkle leaf cache: {e}");
        }

        Self::merkle_root_from_leaves(&leaves)
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
        match bincode::deserialize::<Account>(data) {
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
            let accounts_dirty = match self.db.get_cf(&cf, b"dirty_account_count") {
                Ok(Some(data)) if data.len() == 8 => {
                    u64::from_le_bytes(data.as_slice().try_into().unwrap_or([0; 8]))
                }
                _ => 1,
            };
            let contract_dirty = match self.db.get_cf(&cf, b"dirty_contract_count") {
                Ok(Some(data)) if data.len() == 8 => {
                    u64::from_le_bytes(data.as_slice().try_into().unwrap_or([0; 8]))
                }
                _ => 1,
            };

            if accounts_dirty == 0
                && contract_dirty == 0
                && self.cached_state_root_schema() == Some(include_restrictions)
            {
                if let Ok(Some(data)) = self.db.get_cf(&cf, b"cached_state_root") {
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
        }
    }
}
