use serde::{Deserialize, Serialize};
use std::time::Duration;

use crate::block::Block;
use crate::codec::{append_legacy_bincode, deserialize_legacy_bincode};

use super::*;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AccountTxsRebuildSource {
    #[default]
    Blocks,
    ParentChain,
    TxIndex,
}

impl AccountTxsRebuildSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::ParentChain => "parent-chain",
            Self::TxIndex => "tx-index",
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum CanonicalTxSnapshotCategory {
    Transactions,
    TxBySlot,
    TxToSlot,
    TxMeta,
}

type SnapshotEntry = (Vec<u8>, Vec<u8>);

enum PublicHistoryExistingRow {
    Missing,
    Identical(Vec<u8>),
    UpgradeIncompleteBlock(Vec<u8>),
    Conflict,
}

const CANONICAL_LEDGER_MANIFEST_CATEGORIES: &[&str] = &[
    "slots",
    "blocks",
    "transactions",
    "tx_by_slot",
    "tx_to_slot",
    "tx_meta",
];

struct PublicHistoryDigestAccumulator {
    category: String,
    hasher: Sha256,
    entry_count: u64,
    first_key_hex: Option<String>,
    last_key_hex: Option<String>,
}

impl PublicHistoryDigestAccumulator {
    fn new(category: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"lichen-public-history-category-v1");
        update_public_history_digest_bytes(&mut hasher, category.as_bytes());
        Self {
            category: category.to_string(),
            hasher,
            entry_count: 0,
            first_key_hex: None,
            last_key_hex: None,
        }
    }

    fn push(&mut self, key: &[u8], value: &[u8]) {
        if self.first_key_hex.is_none() {
            self.first_key_hex = Some(hex::encode(key));
        }
        self.last_key_hex = Some(hex::encode(key));
        update_public_history_digest_entry(&mut self.hasher, key, value);
        self.entry_count = self.entry_count.saturating_add(1);
    }

    fn finish(mut self) -> PublicHistoryCategoryDigest {
        self.hasher.update(self.entry_count.to_le_bytes());
        let digest = self.hasher.finalize();
        let mut sha256 = [0u8; 32];
        sha256.copy_from_slice(&digest[..32]);
        PublicHistoryCategoryDigest {
            category: self.category,
            entry_count: self.entry_count,
            sha256,
            first_key_hex: self.first_key_hex,
            last_key_hex: self.last_key_hex,
        }
    }
}

#[derive(Default)]
struct RawBlockHistoryScan {
    seen_body_slots: std::collections::BTreeMap<u64, [u8; 32]>,
    repairable: std::collections::BTreeMap<u64, [u8; 32]>,
    report: RawBlockHistoryRepairReport,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountTxsRebuildReport {
    pub source: AccountTxsRebuildSource,
    pub dry_run: bool,
    pub last_slot: u64,
    pub canonical_slots: u64,
    pub available_blocks: u64,
    pub missing_block_bodies: u64,
    pub first_missing_block_slot: Option<u64>,
    pub header_only_blocks: u64,
    pub first_header_only_slot: Option<u64>,
    pub reached_genesis: bool,
    pub tx_by_slot_rows: u64,
    pub missing_transactions: u64,
    pub first_missing_transaction_slot: Option<u64>,
    pub oldest_tx_slot: Option<u64>,
    pub newest_tx_slot: Option<u64>,
    pub transactions_seen: u64,
    pub expected_account_tx_rows: u64,
    pub existing_hot_rows: u64,
    pub existing_cold_rows: u64,
    pub existing_counter_keys: u64,
    pub rebuilt_rows: u64,
    pub after_hot_rows: u64,
    pub after_counter_keys: u64,
}

impl AccountTxsRebuildReport {
    pub fn source_complete(&self) -> bool {
        match self.source {
            AccountTxsRebuildSource::Blocks | AccountTxsRebuildSource::ParentChain => {
                self.reached_genesis
                    && self.missing_block_bodies == 0
                    && self.header_only_blocks == 0
            }
            AccountTxsRebuildSource::TxIndex => self.missing_transactions == 0,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountTxsSlotInspection {
    pub slot: u64,
    pub block_present: bool,
    pub block_tx_count: u64,
    pub block_matching_account_rows: u64,
    pub tx_by_slot_rows: u64,
    pub tx_by_slot_tx_bodies_present: u64,
    pub tx_by_slot_matching_account_rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountTxsSourceInspection {
    pub account: Pubkey,
    pub cached_account_tx_count: u64,
    pub hot_account_tx_rows: u64,
    pub cold_account_tx_rows: u64,
    pub indexed_signatures: Vec<(Hash, u64)>,
    pub tx_by_slot_rows: u64,
    pub tx_by_slot_missing_transactions: u64,
    pub tx_by_slot_oldest_slot: Option<u64>,
    pub tx_by_slot_newest_slot: Option<u64>,
    pub tx_by_slot_matching_account_rows: u64,
    pub tx_by_slot_matching_signatures: Vec<(Hash, u64)>,
    pub slots: Vec<AccountTxsSlotInspection>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GovernedProposalTxBackfillReport {
    pub dry_run: bool,
    pub tx_by_slot_rows: u64,
    pub proposal_txs: u64,
    pub missing_transactions: u64,
    pub existing_links: u64,
    pub linked: u64,
    pub unresolved: u64,
    pub first_unresolved_tx: Option<String>,
}

/// Metadata stored alongside each checkpoint (serialized as JSON in the
/// checkpoint directory).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointMeta {
    /// Finalized slot at which the checkpoint was taken.
    pub slot: u64,
    /// State root hash of the checkpoint contents.
    pub state_root: [u8; 32],
    /// Timestamp (unix seconds) when the checkpoint was created.
    pub created_at: u64,
    /// Total accounts at checkpoint time.
    pub total_accounts: u64,
}

fn decode_snapshot_block_value(value: &[u8]) -> Result<Block, String> {
    if value.first() == Some(&0xBC) {
        deserialize_legacy_bincode(&value[1..], "block")
            .map_err(|err| format!("Failed to deserialize block snapshot value: {}", err))
    } else {
        serde_json::from_slice(value).map_err(|err| {
            format!(
                "Failed to deserialize legacy JSON block snapshot value: {}",
                err
            )
        })
    }
}

fn canonical_block_snapshot_value(key: &[u8], value: &[u8]) -> Result<Vec<u8>, String> {
    let block = decode_snapshot_block_value(value)?;
    let block_hash = block.hash();
    if key != block_hash.0 {
        return Err(format!(
            "Block snapshot key/hash mismatch: key={} block_hash={}",
            hex::encode(key),
            block_hash.to_hex()
        ));
    }
    canonical_block_snapshot_value_from_block(block)
}

fn validate_complete_public_history_block(block: &Block) -> Result<(), String> {
    if block.transactions.is_empty() && block.header.tx_root != Hash::default() {
        return Err(format!(
            "Refusing header-only public-history block at slot {}",
            block.header.slot
        ));
    }
    let transaction_hashes: Vec<Hash> = block.transactions.iter().map(|tx| tx.hash()).collect();
    let computed_root = crate::block::merkle_tx_root_from_hashes(&transaction_hashes);
    if computed_root != block.header.tx_root {
        return Err(format!(
            "Public-history block transaction root mismatch at slot {}: header={} computed={}",
            block.header.slot,
            block.header.tx_root.to_hex(),
            computed_root.to_hex()
        ));
    }
    Ok(())
}

fn canonical_public_history_block_import_value(
    key: &[u8],
    value: &[u8],
) -> Result<Vec<u8>, String> {
    let block = decode_snapshot_block_value(value)?;
    let block_hash = block.hash();
    if key != block_hash.0 {
        return Err(format!(
            "Block snapshot key/hash mismatch: key={} block_hash={}",
            hex::encode(key),
            block_hash.to_hex()
        ));
    }
    validate_complete_public_history_block(&block)?;
    canonical_block_snapshot_value_from_block(block)
}

fn canonical_block_header_value(block: &Block) -> Result<Vec<u8>, String> {
    let mut value = Vec::new();
    append_legacy_bincode(&mut value, &block.header, "block header")
        .map_err(|err| format!("Failed to serialize canonical block header: {err}"))?;
    Ok(value)
}

fn incomplete_public_history_block_upgrade(
    key: &[u8],
    existing: &[u8],
    incoming: &[u8],
) -> Result<Option<Vec<u8>>, String> {
    let existing = decode_snapshot_block_value(existing)?;
    if !existing.transactions.is_empty()
        || existing.header.tx_root == Hash::default()
        || !existing.tx_fees_paid.is_empty()
    {
        return Ok(None);
    }

    let mut incoming = decode_snapshot_block_value(incoming)?;
    validate_complete_public_history_block(&incoming)?;
    if incoming.transactions.is_empty()
        || canonical_block_header_value(&existing)? != canonical_block_header_value(&incoming)?
        || existing.oracle_prices != incoming.oracle_prices
        || existing.hash().0.as_slice() != key
        || incoming.hash().0.as_slice() != key
    {
        return Ok(None);
    }

    // A replay placeholder retained the target's local finality proof. Restore
    // only the missing body fields and keep that proof instead of replacing it
    // with a source validator's potentially different valid quorum subset.
    incoming.commit_round = existing.commit_round;
    incoming.commit_signatures = existing.commit_signatures;
    canonical_block_snapshot_value_from_block(incoming).map(Some)
}

fn canonical_block_snapshot_value_from_block(mut block: Block) -> Result<Vec<u8>, String> {
    // Commit certificates are semantically a set; collection order can differ
    // across validators that finalized the same block.
    block.commit_signatures.sort_by(|a, b| {
        a.validator
            .cmp(&b.validator)
            .then(a.timestamp.cmp(&b.timestamp))
            .then(a.signature.scheme_version.cmp(&b.signature.scheme_version))
            .then(
                a.signature
                    .public_key
                    .scheme_version
                    .cmp(&b.signature.public_key.scheme_version),
            )
            .then(
                a.signature
                    .public_key
                    .bytes
                    .cmp(&b.signature.public_key.bytes),
            )
            .then(a.signature.sig.cmp(&b.signature.sig))
    });

    let mut canonical = Vec::new();
    canonical.push(0xBC);
    append_legacy_bincode(&mut canonical, &block, "block").map_err(|err| {
        format!(
            "Failed to serialize canonical block snapshot value: {}",
            err
        )
    })?;
    Ok(canonical)
}

fn public_history_manifest_block_value(key: &[u8], value: &[u8]) -> Result<Vec<u8>, String> {
    let mut block = decode_snapshot_block_value(value)?;
    let block_hash = block.hash();
    if key != block_hash.0 {
        return Err(format!(
            "Block snapshot key/hash mismatch: key={} block_hash={}",
            hex::encode(key),
            block_hash.to_hex()
        ));
    }

    // Public-history availability is defined by the canonical block body and
    // transaction payloads. Commit certificates are local finality proofs and
    // can legitimately contain different quorum subsets for the same block hash.
    // Keep those certificates in RocksDB export/import; only normalize them out
    // of manifest digests so archive parity catches missing history, not local
    // consensus-proof collection timing.
    block.commit_round = 0;
    block.commit_signatures.clear();
    canonical_block_snapshot_value_from_block(block)
}

fn canonical_transaction_snapshot_value(
    tx: &crate::transaction::Transaction,
) -> Result<Vec<u8>, String> {
    let mut canonical = Vec::new();
    canonical.push(0xBC);
    append_legacy_bincode(&mut canonical, tx, "transaction").map_err(|err| {
        format!(
            "Failed to serialize canonical transaction snapshot value: {}",
            err
        )
    })?;
    Ok(canonical)
}

fn decode_snapshot_transaction_value(
    value: &[u8],
) -> Result<crate::transaction::Transaction, String> {
    if value.first() == Some(&0xBC) {
        deserialize_legacy_bincode(&value[1..], "transaction")
            .map_err(|err| format!("Failed to deserialize transaction snapshot value: {}", err))
    } else {
        serde_json::from_slice(value).map_err(|err| {
            format!(
                "Failed to deserialize legacy JSON transaction snapshot value: {}",
                err
            )
        })
    }
}

fn canonical_transaction_snapshot_value_from_entry(
    key: &[u8],
    value: &[u8],
) -> Result<Vec<u8>, String> {
    if key.len() != 32 {
        return Err(format!(
            "Transaction snapshot key length mismatch: expected 32 bytes, got {}",
            key.len()
        ));
    }
    let tx = decode_snapshot_transaction_value(value)?;
    let tx_hash = tx.signature();
    if key != tx_hash.0 {
        return Err(format!(
            "Transaction snapshot key/signature mismatch: key={} signature={}",
            hex::encode(key),
            tx_hash.to_hex()
        ));
    }
    canonical_transaction_snapshot_value(&tx)
}

fn update_public_history_digest_bytes(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn update_public_history_digest_entry(hasher: &mut Sha256, key: &[u8], value: &[u8]) {
    update_public_history_digest_bytes(hasher, key);
    update_public_history_digest_bytes(hasher, value);
}

fn append_canonical_tx_manifest_entries(
    accumulators: &mut std::collections::BTreeMap<String, PublicHistoryDigestAccumulator>,
    slot: u64,
    tx_index: u64,
    tx_hash: Hash,
    transaction: &crate::transaction::Transaction,
    tx_meta: Option<&[u8]>,
) -> Result<(), String> {
    if let Some(accumulator) = accumulators.get_mut("transactions") {
        let value = canonical_transaction_snapshot_value(transaction)?;
        accumulator.push(&tx_hash.0, &value);
    }
    if let Some(accumulator) = accumulators.get_mut("tx_by_slot") {
        accumulator.push(&encode_tx_snapshot_cursor(slot, tx_index), &tx_hash.0);
    }
    if let Some(accumulator) = accumulators.get_mut("tx_to_slot") {
        accumulator.push(&tx_hash.0, &slot.to_be_bytes());
    }
    if let (Some(accumulator), Some(value)) = (accumulators.get_mut("tx_meta"), tx_meta) {
        accumulator.push(&tx_hash.0, value);
    }
    Ok(())
}

fn public_history_manifest_root(categories: &[PublicHistoryCategoryDigest]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"lichen-public-history-manifest-v1");
    for digest in categories {
        update_public_history_digest_bytes(&mut hasher, digest.category.as_bytes());
        hasher.update(digest.entry_count.to_le_bytes());
        hasher.update(digest.sha256);
        if let Some(first) = &digest.first_key_hex {
            update_public_history_digest_bytes(&mut hasher, first.as_bytes());
        } else {
            update_public_history_digest_bytes(&mut hasher, b"");
        }
        if let Some(last) = &digest.last_key_hex {
            update_public_history_digest_bytes(&mut hasher, last.as_bytes());
        } else {
            update_public_history_digest_bytes(&mut hasher, b"");
        }
    }
    let digest = hasher.finalize();
    let mut root = [0u8; 32];
    root.copy_from_slice(&digest[..32]);
    root
}

fn parse_slot_snapshot_cursor(after_key: Option<&[u8]>, category: &str) -> Result<u64, String> {
    match after_key {
        None => Ok(0),
        Some(cursor) if cursor.len() == 8 => {
            let mut slot_bytes = [0u8; 8];
            slot_bytes.copy_from_slice(cursor);
            Ok(u64::from_be_bytes(slot_bytes).saturating_add(1))
        }
        Some(cursor) => Err(format!(
            "Invalid {} snapshot cursor length: expected 8 bytes, got {}",
            category,
            cursor.len()
        )),
    }
}

fn encode_tx_snapshot_cursor(slot: u64, tx_index: u64) -> Vec<u8> {
    let mut cursor = Vec::with_capacity(16);
    cursor.extend_from_slice(&slot.to_be_bytes());
    cursor.extend_from_slice(&tx_index.to_be_bytes());
    cursor
}

fn parse_tx_snapshot_cursor(
    after_key: Option<&[u8]>,
    category: &str,
) -> Result<(u64, Option<u64>), String> {
    match after_key {
        None => Ok((0, None)),
        Some(cursor) if cursor.len() == 16 => {
            let mut slot_bytes = [0u8; 8];
            slot_bytes.copy_from_slice(&cursor[..8]);
            let mut index_bytes = [0u8; 8];
            index_bytes.copy_from_slice(&cursor[8..16]);
            Ok((
                u64::from_be_bytes(slot_bytes),
                Some(u64::from_be_bytes(index_bytes)),
            ))
        }
        Some(cursor) => Err(format!(
            "Invalid {} snapshot cursor length: expected 16 bytes, got {}",
            category,
            cursor.len()
        )),
    }
}

fn parse_account_tx_snapshot_key(key: &[u8]) -> Result<Option<(u64, u32, Hash)>, String> {
    if key.len() < 32 + 8 + 4 + 32 {
        return Ok(None);
    }

    let slot_bytes: [u8; 8] = key[32..40]
        .try_into()
        .map_err(|_| "Invalid slot bytes in account tx snapshot key".to_string())?;
    let seq_bytes: [u8; 4] = key[40..44]
        .try_into()
        .map_err(|_| "Invalid sequence bytes in account tx snapshot key".to_string())?;
    let mut hash_bytes = [0u8; 32];
    hash_bytes.copy_from_slice(&key[44..76]);

    Ok(Some((
        u64::from_be_bytes(slot_bytes),
        u32::from_be_bytes(seq_bytes),
        Hash(hash_bytes),
    )))
}

fn parse_tx_by_slot_snapshot_row(
    key: &[u8],
    value: &[u8],
) -> Result<Option<(u64, u64, Hash)>, String> {
    if key.len() != 16 || value.len() != 32 {
        return Ok(None);
    }

    let slot_bytes: [u8; 8] = key[0..8]
        .try_into()
        .map_err(|_| "Invalid slot bytes in tx_by_slot key".to_string())?;
    let seq_bytes: [u8; 8] = key[8..16]
        .try_into()
        .map_err(|_| "Invalid sequence bytes in tx_by_slot key".to_string())?;
    let mut hash_bytes = [0u8; 32];
    hash_bytes.copy_from_slice(value);

    Ok(Some((
        u64::from_be_bytes(slot_bytes),
        u64::from_be_bytes(seq_bytes),
        Hash(hash_bytes),
    )))
}

fn governed_transfer_proposal_instruction(
    tx: &Transaction,
) -> Option<(Pubkey, Pubkey, Pubkey, u64)> {
    let ix = tx.message.instructions.first()?;
    if ix.program_id != crate::SYSTEM_PROGRAM_ID
        || ix.data.first().copied() != Some(21)
        || ix.data.len() < 9
        || ix.accounts.len() < 3
    {
        return None;
    }

    let amount = u64::from_le_bytes(ix.data[1..9].try_into().ok()?);
    Some((ix.accounts[0], ix.accounts[1], ix.accounts[2], amount))
}

fn directory_logical_size(path: &std::path::Path) -> Result<u64, String> {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(current) = stack.pop() {
        let metadata = std::fs::symlink_metadata(&current)
            .map_err(|err| format!("failed to stat {}: {}", current.display(), err))?;
        if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        } else if metadata.is_dir() {
            for entry in std::fs::read_dir(&current)
                .map_err(|err| format!("failed to read {}: {}", current.display(), err))?
            {
                let entry = entry.map_err(|err| {
                    format!("failed to read entry in {}: {}", current.display(), err)
                })?;
                stack.push(entry.path());
            }
        }
    }
    Ok(total)
}

fn checkpoint_paths_total_size(checkpoints: &[(u64, String)]) -> Result<u64, String> {
    checkpoints.iter().try_fold(0u64, |total, (_, path)| {
        let size = directory_logical_size(std::path::Path::new(path))?;
        Ok(total.saturating_add(size))
    })
}

fn checkpoint_directory_paths(data_dir: &str) -> Result<Vec<std::path::PathBuf>, String> {
    let checkpoint_root = std::path::Path::new(data_dir).join("checkpoints");
    let entries = match std::fs::read_dir(&checkpoint_root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => {
            return Err(format!(
                "failed to read checkpoint directory {}: {}",
                checkpoint_root.display(),
                err
            ));
        }
    };

    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|err| {
            format!(
                "failed to read entry in checkpoint directory {}: {}",
                checkpoint_root.display(),
                err
            )
        })?;
        let file_type = entry
            .file_type()
            .map_err(|err| format!("failed to inspect {}: {}", entry.path().display(), err))?;
        if file_type.is_dir() {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

#[cfg(unix)]
fn checkpoint_paths_reclaimable_size(checkpoints: &[std::path::PathBuf]) -> Result<u64, String> {
    use std::os::unix::fs::MetadataExt;

    // Count every link that lives inside the checkpoint set. An inode is
    // reclaimable only when deleting all checkpoints would remove all of its
    // links; files still referenced by the active hot/cold stores do not count.
    let mut files = std::collections::HashMap::<(u64, u64), (u64, u64, u64)>::new();
    for checkpoint in checkpoints {
        let mut stack = vec![checkpoint.clone()];
        while let Some(current) = stack.pop() {
            let metadata = std::fs::symlink_metadata(&current)
                .map_err(|err| format!("failed to stat {}: {}", current.display(), err))?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                for entry in std::fs::read_dir(&current)
                    .map_err(|err| format!("failed to read {}: {}", current.display(), err))?
                {
                    let entry = entry.map_err(|err| {
                        format!("failed to read entry in {}: {}", current.display(), err)
                    })?;
                    stack.push(entry.path());
                }
                continue;
            }
            if !metadata.is_file() {
                continue;
            }

            let entry = files.entry((metadata.dev(), metadata.ino())).or_insert((
                metadata.blocks().saturating_mul(512),
                metadata.nlink(),
                0,
            ));
            entry.2 = entry.2.saturating_add(1);
        }
    }

    Ok(files
        .values()
        .filter(|(_, total_links, checkpoint_links)| checkpoint_links >= total_links)
        .fold(0u64, |total, (bytes, _, _)| total.saturating_add(*bytes)))
}

#[cfg(not(unix))]
fn checkpoint_paths_reclaimable_size(checkpoints: &[std::path::PathBuf]) -> Result<u64, String> {
    // RocksDB checkpoint files are copies on platforms without Unix hard-link
    // metadata, so their allocated size is reclaimable with the directory.
    checkpoints.iter().try_fold(0u64, |total, checkpoint| {
        directory_total_size(checkpoint).map(|size| total.saturating_add(size))
    })
}

impl StateStore {
    pub(crate) fn snapshot_category_cf(category: &str) -> Option<(&'static str, &'static str)> {
        match category {
            "accounts" => Some((CF_ACCOUNTS, "Accounts")),
            "blocks" => Some((CF_BLOCKS, "Blocks")),
            "transactions" => Some((CF_TRANSACTIONS, "Transactions")),
            "account_txs" => Some((CF_ACCOUNT_TXS, "Account transaction index")),
            "slots" => Some((CF_SLOTS, "Slots")),
            "contract_storage" => Some((CF_CONTRACT_STORAGE, "Contract storage")),
            "programs" => Some((CF_PROGRAMS, "Programs")),
            "program_calls" => Some((CF_PROGRAM_CALLS, "Program call index")),
            "market_activity" => Some((CF_MARKET_ACTIVITY, "Market activity index")),
            "symbol_registry" => Some((CF_SYMBOL_REGISTRY, "Symbol registry")),
            "symbol_by_program" => Some((CF_SYMBOL_BY_PROGRAM, "Symbol reverse registry")),
            "evm_map" => Some((CF_EVM_MAP, "EVM address map")),
            "evm_accounts" => Some((CF_EVM_ACCOUNTS, "EVM accounts")),
            "evm_storage" => Some((CF_EVM_STORAGE, "EVM storage")),
            "evm_txs" => Some((CF_EVM_TXS, "EVM transaction metadata")),
            "evm_receipts" => Some((CF_EVM_RECEIPTS, "EVM receipts")),
            "evm_logs_by_slot" => Some((CF_EVM_LOGS_BY_SLOT, "EVM logs by slot")),
            "nft_by_owner" => Some((CF_NFT_BY_OWNER, "NFT owner index")),
            "nft_by_collection" => Some((CF_NFT_BY_COLLECTION, "NFT collection index")),
            "nft_activity" => Some((CF_NFT_ACTIVITY, "NFT activity index")),
            "token_balances" => Some((CF_TOKEN_BALANCES, "Token balances")),
            "token_transfers" => Some((CF_TOKEN_TRANSFERS, "Token transfer index")),
            "holder_tokens" => Some((CF_HOLDER_TOKENS, "Holder token index")),
            "solana_token_accounts" => {
                Some((CF_SOLANA_TOKEN_ACCOUNTS, "Solana token-account bindings"))
            }
            "solana_holder_token_accounts" => Some((
                CF_SOLANA_HOLDER_TOKEN_ACCOUNTS,
                "Solana holder token-account index",
            )),
            "events" => Some((CF_EVENTS, "Contract events")),
            "events_by_slot" => Some((CF_EVENTS_BY_SLOT, "Contract events by slot")),
            "dex_orders_by_pair" => Some((CF_DEX_ORDERS_BY_PAIR, "DEX orders-by-pair index")),
            "dex_trades_by_pair" => Some((CF_DEX_TRADES_BY_PAIR, "DEX trades-by-pair index")),
            "dex_trades_by_taker" => Some((CF_DEX_TRADES_BY_TAKER, "DEX trades-by-taker index")),
            "dex_trades_by_pair_taker" => Some((
                CF_DEX_TRADES_BY_PAIR_TAKER,
                "DEX trades-by-pair-taker index",
            )),
            "dex_orderbook_levels" => Some((CF_DEX_ORDERBOOK_LEVELS, "DEX orderbook levels")),
            "tx_by_slot" => Some((CF_TX_BY_SLOT, "Transaction by slot index")),
            "tx_to_slot" => Some((CF_TX_TO_SLOT, "Transaction slot index")),
            "tx_meta" => Some((CF_TX_META, "Transaction metadata")),
            "account_snapshots" => Some((CF_ACCOUNT_SNAPSHOTS, "Account snapshots")),
            "pending_validator_changes" => {
                Some((CF_PENDING_VALIDATOR_CHANGES, "Pending validator changes"))
            }
            "restrictions" => Some((CF_RESTRICTIONS, "Restrictions")),
            "restriction_index_target" => {
                Some((CF_RESTRICTION_INDEX_TARGET, "Restriction target index"))
            }
            "restriction_index_code_hash" => Some((
                CF_RESTRICTION_INDEX_CODE_HASH,
                "Restriction code-hash index",
            )),
            "shielded_commitments" => Some((CF_SHIELDED_COMMITMENTS, "Shielded commitments")),
            "shielded_note_payloads" => Some((CF_SHIELDED_NOTE_PAYLOADS, "Shielded note payloads")),
            "shielded_nullifiers" => Some((CF_SHIELDED_NULLIFIERS, "Shielded nullifiers")),
            "shielded_pool" => Some((CF_SHIELDED_POOL, "Shielded pool")),
            "shielded_txs" => Some((CF_SHIELDED_TXS, "Shielded transaction index")),
            "stats" => Some((CF_STATS, "Stats")),
            _ => None,
        }
    }

    pub fn snapshot_category_names() -> &'static [&'static str] {
        STATE_SNAPSHOT_CATEGORIES
    }

    /// Get a reference to the underlying DB Arc for direct access when needed.
    pub fn db_ref(&self) -> &Arc<DB> {
        &self.db
    }

    // ── Checkpoint creation (RocksDB native hardlink snapshot) ────────────

    /// Create a point-in-time checkpoint of only the hot database.
    ///
    /// Snapshot live-apply rollback uses this because the apply path never
    /// mutates the independently managed cold archive. Pinning cold SSTs in
    /// that rollback checkpoint would waste capacity and block maintenance.
    pub fn create_hot_raw_checkpoint(&self, checkpoint_dir: &str) -> Result<(), String> {
        use rocksdb::checkpoint::Checkpoint;

        // Persist in-memory counters first so the checkpoint sees a coherent
        // DB view, matching regular snapshot checkpoint behavior.
        self.save_metrics_counters()?;

        let parent = std::path::Path::new(checkpoint_dir)
            .parent()
            .ok_or_else(|| "Invalid checkpoint path".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create checkpoint parent dir: {}", e))?;

        if std::path::Path::new(checkpoint_dir).exists() {
            std::fs::remove_dir_all(checkpoint_dir)
                .map_err(|e| format!("Failed to remove old checkpoint: {}", e))?;
        }

        let cp = Checkpoint::new(&self.db)
            .map_err(|e| format!("Failed to create checkpoint object: {}", e))?;
        cp.create_checkpoint(checkpoint_dir)
            .map_err(|e| format!("Failed to create checkpoint: {}", e))?;

        Ok(())
    }

    /// Create a point-in-time checkpoint without snapshot metadata.
    ///
    /// This is used by persistent checkpoint/snapshot paths that must carry
    /// both hot state and the attached cold archive. Snapshot live-apply
    /// rollback deliberately calls `create_hot_raw_checkpoint` instead.
    pub fn create_raw_checkpoint(&self, checkpoint_dir: &str) -> Result<(), String> {
        use rocksdb::checkpoint::Checkpoint;

        self.create_hot_raw_checkpoint(checkpoint_dir)?;

        if let Some(cold) = self.cold_db.as_ref() {
            let cold_checkpoint_dir = std::path::Path::new(checkpoint_dir).join("cold");
            let cold_cp = Checkpoint::new(cold)
                .map_err(|e| format!("Failed to create cold checkpoint object: {}", e))?;
            cold_cp
                .create_checkpoint(&cold_checkpoint_dir)
                .map_err(|e| format!("Failed to create cold checkpoint: {}", e))?;
        }

        Ok(())
    }

    /// Create a point-in-time checkpoint of the entire database.
    ///
    /// Uses RocksDB's native `Checkpoint` API which creates hardlinks to SST
    /// files — effectively O(1) in time and zero additional disk space until
    /// compaction replaces the SST files.
    ///
    /// `checkpoint_dir` is the directory where the checkpoint will be stored,
    /// e.g. `data/state-8000/checkpoints/slot-10000`.
    ///
    /// Returns the `CheckpointMeta` for the created checkpoint.
    pub fn create_checkpoint(
        &self,
        checkpoint_dir: &str,
        slot: u64,
    ) -> Result<CheckpointMeta, String> {
        self.create_raw_checkpoint(checkpoint_dir)?;
        let checkpoint_store = Self::open_checkpoint(checkpoint_dir)
            .map_err(|e| format!("Failed to open created checkpoint: {}", e))?;
        let state_root = checkpoint_store
            .compute_state_root_cached_read_only()
            .unwrap_or_else(|| checkpoint_store.compute_state_root_read_only());
        let total_accounts = checkpoint_store.metrics.get_total_accounts();
        let meta = CheckpointMeta {
            slot,
            state_root: state_root.0,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            total_accounts,
        };

        let meta_path = std::path::Path::new(checkpoint_dir).join("checkpoint_meta.json");
        let meta_json = serde_json::to_string_pretty(&meta)
            .map_err(|e| format!("Failed to serialize checkpoint meta: {}", e))?;
        std::fs::write(&meta_path, meta_json)
            .map_err(|e| format!("Failed to write checkpoint meta: {}", e))?;

        Ok(meta)
    }

    /// Open a checkpoint as a read-only StateStore for serving snapshot data.
    pub fn open_checkpoint(checkpoint_dir: &str) -> Result<Self, String> {
        let mut store = Self::open_read_only_with_cache_mb(checkpoint_dir, None)?;
        let cold_checkpoint_dir = std::path::Path::new(checkpoint_dir).join("cold");
        if cold_checkpoint_dir.is_dir() {
            store.open_cold_store_read_only(&cold_checkpoint_dir)?;
        }
        Ok(store)
    }

    /// List available checkpoints in the data directory.
    /// Returns sorted (oldest first) list of `(slot, checkpoint_dir_path)`.
    pub fn list_checkpoints(data_dir: &str) -> Vec<(u64, String)> {
        let cp_root = std::path::Path::new(data_dir).join("checkpoints");
        let mut result = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&cp_root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let meta_path = path.join("checkpoint_meta.json");
                    if meta_path.exists() {
                        if let Ok(data) = std::fs::read_to_string(&meta_path) {
                            if let Ok(meta) = serde_json::from_str::<CheckpointMeta>(&data) {
                                result.push((meta.slot, path.to_string_lossy().to_string()));
                            }
                        }
                    }
                }
            }
        }
        result.sort_by_key(|(slot, _)| *slot);
        result
    }

    /// Get the latest checkpoint metadata from the data directory.
    pub fn latest_checkpoint(data_dir: &str) -> Option<(CheckpointMeta, String)> {
        let checkpoints = Self::list_checkpoints(data_dir);
        checkpoints.last().and_then(|(_, path)| {
            let meta_path = std::path::Path::new(path).join("checkpoint_meta.json");
            let data = std::fs::read_to_string(&meta_path).ok()?;
            let meta: CheckpointMeta = serde_json::from_str(&data).ok()?;
            Some((meta, path.clone()))
        })
    }

    /// Return bytes that would actually be released by deleting every listed
    /// checkpoint. Hard-linked SSTs still referenced by active hot/cold stores
    /// are excluded.
    pub fn checkpoint_reclaimable_bytes(data_dir: &str) -> Result<u64, String> {
        checkpoint_paths_reclaimable_size(&checkpoint_directory_paths(data_dir)?)
    }

    /// Remove every derived checkpoint directory, including incomplete
    /// checkpoints left by interrupted creation. Active hot/cold stores live
    /// outside this directory and hard-linked SSTs remain available there.
    pub fn prune_all_checkpoints(data_dir: &str) -> Result<usize, String> {
        let checkpoints = checkpoint_directory_paths(data_dir)?;
        for checkpoint in &checkpoints {
            std::fs::remove_dir_all(checkpoint).map_err(|err| {
                format!(
                    "failed to remove checkpoint {}: {}",
                    checkpoint.display(),
                    err
                )
            })?;
        }
        Ok(checkpoints.len())
    }

    /// Prune old checkpoints, keeping only the most recent `keep_count`.
    pub fn prune_checkpoints(data_dir: &str, keep_count: usize) -> Result<usize, String> {
        Self::prune_checkpoints_with_size_limit(data_dir, keep_count, None)
    }

    /// Prune old checkpoints by count and, optionally, by total logical size.
    ///
    /// RocksDB checkpoints are hardlink snapshots. A checkpoint can initially be
    /// cheap, then pin a large set of obsolete SSTs after compaction. Count-only
    /// retention is therefore not enough for long-running validators.
    pub fn prune_checkpoints_with_size_limit(
        data_dir: &str,
        keep_count: usize,
        max_total_bytes: Option<u64>,
    ) -> Result<usize, String> {
        let checkpoints = Self::list_checkpoints(data_dir);
        let mut remaining = checkpoints;
        let mut removed = 0;

        while remaining.len() > keep_count {
            let (_, path) = remaining.remove(0);
            if std::fs::remove_dir_all(path).is_ok() {
                removed += 1;
            }
        }

        if let Some(max_bytes) = max_total_bytes.filter(|value| *value > 0) {
            while remaining.len() > 1 && checkpoint_paths_total_size(&remaining)? > max_bytes {
                let (_, path) = remaining.remove(0);
                if std::fs::remove_dir_all(path).is_ok() {
                    removed += 1;
                }
            }
        }

        Ok(removed)
    }

    // ── Snapshot export / import (for P2P state transfer) ────────────────

    /// Export a page of accounts as (pubkey_bytes, account_bytes).
    pub fn export_accounts_iter(&self, offset: u64, limit: u64) -> Result<KvPage, String> {
        self.export_cf_page(CF_ACCOUNTS, "Accounts", offset, limit)
    }

    /// Export a cursor-paginated page of accounts.
    pub fn export_accounts_cursor(
        &self,
        after_key: Option<&[u8]>,
        limit: u64,
    ) -> Result<KvPage, String> {
        self.export_cf_page_cursor_counted(
            CF_ACCOUNTS,
            "Accounts",
            after_key,
            limit,
            Some(self.metrics.get_total_accounts()),
        )
    }

    /// Export a cursor-paginated page of accounts without computing totals.
    pub fn export_accounts_cursor_untracked(
        &self,
        after_key: Option<&[u8]>,
        limit: u64,
    ) -> Result<KvPage, String> {
        self.export_cf_page_cursor_uncounted(CF_ACCOUNTS, "Accounts", after_key, limit)
    }

    /// Export a page of contract storage entries as (key_bytes, value_bytes).
    pub fn export_contract_storage_iter(&self, offset: u64, limit: u64) -> Result<KvPage, String> {
        self.export_cf_page(CF_CONTRACT_STORAGE, "Contract storage", offset, limit)
    }

    /// Export a cursor-paginated page of contract storage entries.
    pub fn export_contract_storage_cursor(
        &self,
        after_key: Option<&[u8]>,
        limit: u64,
    ) -> Result<KvPage, String> {
        self.export_cf_page_cursor_counted(
            CF_CONTRACT_STORAGE,
            "Contract storage",
            after_key,
            limit,
            None,
        )
    }

    /// Export a cursor-paginated page of contract storage without computing totals.
    pub fn export_contract_storage_cursor_untracked(
        &self,
        after_key: Option<&[u8]>,
        limit: u64,
    ) -> Result<KvPage, String> {
        self.export_cf_page_cursor_uncounted(
            CF_CONTRACT_STORAGE,
            "Contract storage",
            after_key,
            limit,
        )
    }

    /// Count total number of contract storage entries.
    pub fn count_contract_storage_entries(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;
        let mut count = 0u64;
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        for _ in self
            .db
            .iterator_cf_opt(&cf, read_opts, rocksdb::IteratorMode::Start)
            .flatten()
        {
            count = count.saturating_add(1);
        }
        Ok(count)
    }

    /// Export a page of programs (WASM bytecode) as (pubkey_bytes, program_bytes).
    pub fn export_programs_iter(&self, offset: u64, limit: u64) -> Result<KvPage, String> {
        self.export_cf_page(CF_PROGRAMS, "Programs", offset, limit)
    }

    /// Export a cursor-paginated page of programs.
    pub fn export_programs_cursor(
        &self,
        after_key: Option<&[u8]>,
        limit: u64,
    ) -> Result<KvPage, String> {
        self.export_cf_page_cursor_counted(
            CF_PROGRAMS,
            "Programs",
            after_key,
            limit,
            Some(self.get_program_count()),
        )
    }

    /// Export a cursor-paginated page of programs without computing totals.
    pub fn export_programs_cursor_untracked(
        &self,
        after_key: Option<&[u8]>,
        limit: u64,
    ) -> Result<KvPage, String> {
        self.export_cf_page_cursor_uncounted(CF_PROGRAMS, "Programs", after_key, limit)
    }

    /// Export a cursor-paginated page for a public-history category.
    ///
    /// This is the archive/parity surface, not the full state snapshot surface.
    /// In particular, `slots` exports only canonical slot->block-hash rows and
    /// excludes live cursor metadata such as `last_slot`.
    pub fn export_public_history_category_cursor_untracked(
        &self,
        category: &str,
        after_key: Option<&[u8]>,
        limit: u64,
    ) -> Result<KvPage, String> {
        self.export_public_history_category_range_cursor_untracked(category, after_key, limit, None)
    }

    /// Export an inclusive slot-bounded page for range repair.
    ///
    /// Only canonical slot-driven categories support an upper bound. Keeping
    /// the bound inside each iterator prevents a page from leaking later rows
    /// or truncating transactions when the final slot spans multiple pages.
    pub fn export_public_history_category_range_cursor_untracked(
        &self,
        category: &str,
        after_key: Option<&[u8]>,
        limit: u64,
        to_slot: Option<u64>,
    ) -> Result<KvPage, String> {
        if !PUBLIC_HISTORY_SNAPSHOT_CATEGORIES.contains(&category) {
            return Err(format!("Unsupported public-history category: {}", category));
        }
        if category == "slots" {
            return self.export_public_slots_cursor(after_key, limit, to_slot);
        }
        if category == "blocks" {
            return self.export_blocks_cursor_canonical(after_key, limit, to_slot);
        }
        let tx_category = match category {
            "transactions" => Some(CanonicalTxSnapshotCategory::Transactions),
            "tx_by_slot" => Some(CanonicalTxSnapshotCategory::TxBySlot),
            "tx_to_slot" => Some(CanonicalTxSnapshotCategory::TxToSlot),
            "tx_meta" => Some(CanonicalTxSnapshotCategory::TxMeta),
            _ => None,
        };
        if let Some(tx_category) = tx_category {
            return self.export_canonical_tx_snapshot_cursor(
                tx_category,
                after_key,
                limit,
                to_slot,
            );
        }
        if to_slot.is_some() {
            return Err(format!(
                "Slot-bounded public-history export is not supported for {category}"
            ));
        }
        self.export_snapshot_category_cursor_untracked(category, after_key, limit)
    }

    fn export_public_slots_cursor(
        &self,
        after_key: Option<&[u8]>,
        limit: u64,
        to_slot: Option<u64>,
    ) -> Result<KvPage, String> {
        if limit == 0 {
            return Ok(KvPage {
                entries: Vec::new(),
                total: 0,
                next_cursor: None,
                has_more: false,
            });
        }

        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = if let Some(after) = after_key {
            self.db.iterator_cf_opt(
                &cf,
                read_opts,
                rocksdb::IteratorMode::From(after, rocksdb::Direction::Forward),
            )
        } else {
            self.db
                .iterator_cf_opt(&cf, read_opts, rocksdb::IteratorMode::Start)
        };

        let mut entries = Vec::with_capacity(limit.min(10_000) as usize);
        let mut has_more = false;
        for item in iter {
            let (key, value) =
                item.map_err(|err| format!("Failed iterating public slots: {}", err))?;
            if let Some(after) = after_key {
                if key.as_ref() == after {
                    continue;
                }
            }
            if key.len() != 8 || value.len() != 32 {
                continue;
            }

            let mut slot_bytes = [0u8; 8];
            slot_bytes.copy_from_slice(&key);
            if to_slot.is_some_and(|upper| u64::from_be_bytes(slot_bytes) > upper) {
                break;
            }

            if entries.len() == limit as usize {
                has_more = true;
                break;
            }
            entries.push((key.to_vec(), value.to_vec()));
        }

        let next_cursor = if has_more {
            entries.last().map(|(key, _)| key.clone())
        } else {
            None
        };

        Ok(KvPage {
            entries,
            total: 0,
            next_cursor,
            has_more,
        })
    }

    pub fn compute_public_history_manifest(
        &self,
        categories: &[&str],
        chunk_size: u64,
    ) -> Result<PublicHistoryManifest, String> {
        let chunk_size = chunk_size.max(1);
        let canonical_categories: Vec<_> = categories
            .iter()
            .copied()
            .filter(|category| CANONICAL_LEDGER_MANIFEST_CATEGORIES.contains(category))
            .collect();
        let share_canonical_walk = canonical_categories.len() > 1
            && canonical_categories.iter().any(|category| {
                matches!(
                    *category,
                    "transactions" | "tx_by_slot" | "tx_to_slot" | "tx_meta"
                )
            });
        let canonical_digests = if share_canonical_walk {
            self.compute_canonical_ledger_manifest_digests(&canonical_categories)?
        } else {
            std::collections::BTreeMap::new()
        };
        let mut category_digests = Vec::with_capacity(categories.len());
        for category in categories {
            if let Some(digest) = canonical_digests.get(*category) {
                category_digests.push(digest.clone());
            } else {
                category_digests
                    .push(self.compute_public_history_category_digest_paged(category, chunk_size)?);
            }
        }
        let root = public_history_manifest_root(&category_digests);
        Ok(PublicHistoryManifest {
            schema_version: 1,
            categories: category_digests,
            root,
        })
    }

    fn compute_public_history_category_digest_paged(
        &self,
        category: &str,
        chunk_size: u64,
    ) -> Result<PublicHistoryCategoryDigest, String> {
        let mut hasher = Sha256::new();
        hasher.update(b"lichen-public-history-category-v1");
        update_public_history_digest_bytes(&mut hasher, category.as_bytes());

        let mut entry_count = 0u64;
        let mut first_key_hex = None;
        let mut last_key_hex = None;
        let mut cursor: Option<Vec<u8>> = None;
        loop {
            let page = self.export_public_history_category_cursor_untracked(
                category,
                cursor.as_deref(),
                chunk_size,
            )?;
            for (key, value) in page.entries {
                if first_key_hex.is_none() {
                    first_key_hex = Some(hex::encode(&key));
                }
                last_key_hex = Some(hex::encode(&key));
                let digest_value;
                let value_for_digest = if category == "blocks" {
                    digest_value = public_history_manifest_block_value(&key, &value)?;
                    digest_value.as_slice()
                } else {
                    value.as_slice()
                };
                update_public_history_digest_entry(&mut hasher, &key, value_for_digest);
                entry_count = entry_count.saturating_add(1);
            }
            if !page.has_more {
                break;
            }
            let Some(next_cursor) = page.next_cursor else {
                return Err(format!(
                    "{} public-history export had more entries but no cursor",
                    category
                ));
            };
            cursor = Some(next_cursor);
        }

        hasher.update(entry_count.to_le_bytes());
        let digest = hasher.finalize();
        let mut sha256 = [0u8; 32];
        sha256.copy_from_slice(&digest[..32]);
        Ok(PublicHistoryCategoryDigest {
            category: category.to_string(),
            entry_count,
            sha256,
            first_key_hex,
            last_key_hex,
        })
    }

    fn compute_canonical_ledger_manifest_digests(
        &self,
        categories: &[&str],
    ) -> Result<std::collections::BTreeMap<String, PublicHistoryCategoryDigest>, String> {
        if categories.is_empty() {
            return Ok(std::collections::BTreeMap::new());
        }

        let mut accumulators = std::collections::BTreeMap::new();
        for category in categories {
            accumulators
                .entry((*category).to_string())
                .or_insert_with(|| PublicHistoryDigestAccumulator::new(category));
        }

        let include_blocks = accumulators.contains_key("blocks");
        let include_tx_meta = accumulators.contains_key("tx_meta");
        let tx_meta_cf = if include_tx_meta {
            Some(
                self.db
                    .cf_handle(CF_TX_META)
                    .ok_or_else(|| "Transaction metadata CF not found".to_string())?,
            )
        } else {
            None
        };

        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let tx_by_slot_cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "TX by slot CF not found".to_string())?;

        let mut slot_read_opts = rocksdb::ReadOptions::default();
        slot_read_opts.set_total_order_seek(true);
        let mut slot_iter =
            self.db
                .iterator_cf_opt(&slot_cf, slot_read_opts, rocksdb::IteratorMode::Start);
        let mut next_slot_row = || -> Result<Option<(u64, [u8; 32])>, String> {
            loop {
                let Some(item) = slot_iter.next() else {
                    return Ok(None);
                };
                let (key, value) =
                    item.map_err(|err| format!("Failed iterating Slots for manifest: {err}"))?;
                if key.len() != 8 || value.len() != 32 {
                    continue;
                }
                let mut slot_bytes = [0u8; 8];
                slot_bytes.copy_from_slice(&key);
                let mut block_hash = [0u8; 32];
                block_hash.copy_from_slice(&value);
                return Ok(Some((u64::from_be_bytes(slot_bytes), block_hash)));
            }
        };

        let mut tx_read_opts = rocksdb::ReadOptions::default();
        tx_read_opts.set_total_order_seek(true);
        let mut tx_iter =
            self.db
                .iterator_cf_opt(&tx_by_slot_cf, tx_read_opts, rocksdb::IteratorMode::Start);
        let mut next_tx_row = || -> Result<Option<(u64, u64, Hash)>, String> {
            loop {
                let Some(item) = tx_iter.next() else {
                    return Ok(None);
                };
                let (key, value) =
                    item.map_err(|err| format!("Failed iterating tx_by_slot for manifest: {err}"))?;
                if let Some(row) = parse_tx_by_slot_snapshot_row(&key, &value)? {
                    return Ok(Some(row));
                }
            }
        };

        let mut slot_row = next_slot_row()?;
        let mut tx_row = next_tx_row()?;
        while slot_row.is_some() || tx_row.is_some() {
            let current_slot = match (slot_row.as_ref(), tx_row.as_ref()) {
                (Some((slot, _)), Some((tx_slot, _, _))) => (*slot).min(*tx_slot),
                (Some((slot, _)), None) => *slot,
                (None, Some((slot, _, _))) => *slot,
                (None, None) => break,
            };

            if slot_row
                .as_ref()
                .is_some_and(|(slot, _)| *slot == current_slot)
            {
                let (_, block_hash) = slot_row.take().expect("current slot row exists");
                if let Some(accumulator) = accumulators.get_mut("slots") {
                    accumulator.push(&current_slot.to_be_bytes(), &block_hash);
                }

                let block = self.get_block(&Hash(block_hash))?;
                match block {
                    Some(block) => {
                        if let Some(accumulator) = accumulators.get_mut("blocks") {
                            let canonical =
                                canonical_block_snapshot_value_from_block(block.clone())?;
                            let digest_value =
                                public_history_manifest_block_value(&block_hash, &canonical)?;
                            accumulator.push(&block_hash, &digest_value);
                        }

                        for (tx_index, transaction) in block.transactions.iter().enumerate() {
                            let tx_hash = transaction.signature();
                            let tx_meta = if let Some(cf) = tx_meta_cf.as_ref() {
                                self.db
                                    .get_cf(cf, tx_hash.0)
                                    .map_err(|err| format!("Failed reading tx metadata: {err}"))?
                            } else {
                                None
                            };
                            append_canonical_tx_manifest_entries(
                                &mut accumulators,
                                current_slot,
                                tx_index as u64,
                                tx_hash,
                                transaction,
                                tx_meta.as_deref(),
                            )?;
                        }

                        while tx_row
                            .as_ref()
                            .is_some_and(|(slot, _, _)| *slot == current_slot)
                        {
                            tx_row = next_tx_row()?;
                        }
                    }
                    None if include_blocks => {
                        return Err(format!(
                            "Canonical block {current_slot} missing from hot/cold storage"
                        ));
                    }
                    None => {}
                }
                slot_row = next_slot_row()?;
            }

            while tx_row
                .as_ref()
                .is_some_and(|(slot, _, _)| *slot == current_slot)
            {
                let (slot, tx_index, tx_hash) =
                    tx_row.take().expect("current transaction row exists");
                if let Some(transaction) = self.get_transaction(&tx_hash)? {
                    if transaction.signature() == tx_hash {
                        let tx_meta = if let Some(cf) = tx_meta_cf.as_ref() {
                            self.db
                                .get_cf(cf, tx_hash.0)
                                .map_err(|err| format!("Failed reading tx metadata: {err}"))?
                        } else {
                            None
                        };
                        append_canonical_tx_manifest_entries(
                            &mut accumulators,
                            slot,
                            tx_index,
                            tx_hash,
                            &transaction,
                            tx_meta.as_deref(),
                        )?;
                    }
                }
                tx_row = next_tx_row()?;
            }
        }

        Ok(accumulators
            .into_iter()
            .map(|(category, accumulator)| (category, accumulator.finish()))
            .collect())
    }

    /// Export a cursor-paginated page for a whitelisted snapshot category.
    ///
    /// This is intentionally not an arbitrary column-family escape hatch. It is
    /// used by genesis/state-sync code for categories that are either committed
    /// by the state root or required to execute the chain after import.
    pub fn export_snapshot_category_cursor_untracked(
        &self,
        category: &str,
        after_key: Option<&[u8]>,
        limit: u64,
    ) -> Result<KvPage, String> {
        if category == "blocks" {
            return self.export_blocks_cursor_canonical(after_key, limit, None);
        }
        if category == "transactions" {
            return self.export_canonical_tx_snapshot_cursor(
                CanonicalTxSnapshotCategory::Transactions,
                after_key,
                limit,
                None,
            );
        }
        if category == "account_txs" {
            return self.export_public_history_index_cursor(
                category,
                CF_ACCOUNT_TXS,
                Some(COLD_CF_ACCOUNT_TXS),
                after_key,
                limit,
            );
        }
        if category == "account_snapshots" {
            return self.export_public_history_index_cursor(
                category,
                CF_ACCOUNT_SNAPSHOTS,
                Some(COLD_CF_ACCOUNT_SNAPSHOTS),
                after_key,
                limit,
            );
        }
        if category == "tx_by_slot" {
            return self.export_canonical_tx_snapshot_cursor(
                CanonicalTxSnapshotCategory::TxBySlot,
                after_key,
                limit,
                None,
            );
        }
        if category == "tx_to_slot" {
            return self.export_canonical_tx_snapshot_cursor(
                CanonicalTxSnapshotCategory::TxToSlot,
                after_key,
                limit,
                None,
            );
        }
        if category == "tx_meta" {
            return self.export_canonical_tx_snapshot_cursor(
                CanonicalTxSnapshotCategory::TxMeta,
                after_key,
                limit,
                None,
            );
        }
        if category == "events" {
            return self.export_public_history_index_cursor(
                category,
                CF_EVENTS,
                Some(COLD_CF_EVENTS),
                after_key,
                limit,
            );
        }
        if category == "token_transfers" {
            return self.export_public_history_index_cursor(
                category,
                CF_TOKEN_TRANSFERS,
                Some(COLD_CF_TOKEN_TRANSFERS),
                after_key,
                limit,
            );
        }
        if category == "program_calls" {
            return self.export_public_history_index_cursor(
                category,
                CF_PROGRAM_CALLS,
                Some(COLD_CF_PROGRAM_CALLS),
                after_key,
                limit,
            );
        }
        if category == "stats" {
            return self.export_stats_cursor_for_snapshot(after_key, limit);
        }

        let (cf_name, display_name) = Self::snapshot_category_cf(category)
            .ok_or_else(|| format!("Unsupported snapshot category: {}", category))?;
        self.export_cf_page_cursor_uncounted(cf_name, display_name, after_key, limit)
    }

    /// Export one whitelisted category directly from the hot database.
    ///
    /// Local live-apply rollback uses this to restore the pre-apply hot layout
    /// without falling through to, filtering, or duplicating the independent
    /// cold archive. Network snapshots must use the canonical exporter above.
    pub fn export_hot_snapshot_category_cursor_untracked(
        &self,
        category: &str,
        after_key: Option<&[u8]>,
        limit: u64,
    ) -> Result<KvPage, String> {
        let (cf_name, display_name) = Self::snapshot_category_cf(category)
            .ok_or_else(|| format!("Unsupported snapshot category: {}", category))?;
        self.export_cf_page_cursor_uncounted(cf_name, display_name, after_key, limit)
    }

    fn export_stats_cursor_for_snapshot(
        &self,
        after_key: Option<&[u8]>,
        limit: u64,
    ) -> Result<KvPage, String> {
        const VOLATILE_MERKLE_STATS_KEYS: &[&[u8]] = &[
            b"cached_state_root",
            b"cached_state_root_schema",
            b"cached_state_commitment_schema",
            b"cached_accounts_root",
            b"cached_contract_root",
            b"merkle_leaf_count",
            b"contract_merkle_leaf_count",
        ];

        if limit == 0 {
            return Ok(KvPage {
                entries: Vec::new(),
                total: 0,
                next_cursor: None,
                has_more: false,
            });
        }

        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = if let Some(after) = after_key {
            self.db.iterator_cf_opt(
                &cf,
                read_opts,
                rocksdb::IteratorMode::From(after, rocksdb::Direction::Forward),
            )
        } else {
            self.db
                .iterator_cf_opt(&cf, read_opts, rocksdb::IteratorMode::Start)
        };

        let mut entries = Vec::with_capacity(limit.min(10_000) as usize);
        let mut has_more = false;
        for item in iter {
            let (key, value) = item.map_err(|err| format!("Failed iterating Stats: {}", err))?;
            if let Some(after) = after_key {
                if key.as_ref() == after {
                    continue;
                }
            }
            if VOLATILE_MERKLE_STATS_KEYS
                .iter()
                .any(|volatile| key.as_ref() == *volatile)
            {
                continue;
            }

            entries.push((key.to_vec(), value.to_vec()));
            if entries.len() > limit as usize {
                has_more = true;
                entries.pop();
                break;
            }
        }

        let next_cursor = if has_more {
            entries.last().map(|(key, _)| key.clone())
        } else {
            None
        };

        Ok(KvPage {
            entries,
            total: 0,
            next_cursor,
            has_more,
        })
    }

    fn account_tx_snapshot_entry_is_canonical_or_unverifiable(
        &self,
        key: &[u8],
    ) -> Result<bool, String> {
        let Some((slot, seq, tx_hash)) = parse_account_tx_snapshot_key(key)? else {
            return Ok(true);
        };

        match self.get_block_by_slot(slot)? {
            Some(block) => Ok(block
                .transactions
                .iter()
                .any(|tx| tx.signature() == tx_hash)),
            None => self.account_txs_row_backed_by_tx_index(slot, seq, &tx_hash),
        }
    }

    fn export_public_history_index_cursor(
        &self,
        category: &str,
        hot_cf_name: &str,
        cold_cf_name: Option<&str>,
        after_key: Option<&[u8]>,
        limit: u64,
    ) -> Result<KvPage, String> {
        if limit == 0 {
            return Ok(KvPage {
                entries: Vec::new(),
                total: 0,
                next_cursor: None,
                has_more: false,
            });
        }

        let mut merged = std::collections::BTreeMap::<Vec<u8>, Vec<u8>>::new();
        self.collect_public_history_index_snapshot_entries(
            category,
            &self.db,
            hot_cf_name,
            after_key,
            limit,
            &mut merged,
        )?;
        if let (Some(ref cold), Some(cold_cf_name)) = (&self.cold_db, cold_cf_name) {
            if cold.cf_handle(cold_cf_name).is_some() {
                self.collect_public_history_index_snapshot_entries(
                    category,
                    cold,
                    cold_cf_name,
                    after_key,
                    limit,
                    &mut merged,
                )?;
            }
        }

        let mut entries: Vec<_> = merged.into_iter().collect();
        let has_more = entries.len() > limit as usize;
        if has_more {
            entries.truncate(limit as usize);
        }
        let next_cursor = if has_more {
            entries.last().map(|(key, _)| key.clone())
        } else {
            None
        };

        Ok(KvPage {
            entries,
            total: 0,
            next_cursor,
            has_more,
        })
    }

    fn collect_public_history_index_snapshot_entries(
        &self,
        category: &str,
        db: &DB,
        cf_name: &str,
        after_key: Option<&[u8]>,
        limit: u64,
        entries: &mut std::collections::BTreeMap<Vec<u8>, Vec<u8>>,
    ) -> Result<(), String> {
        let cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{} CF not found", cf_name))?;
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = if let Some(after) = after_key {
            db.iterator_cf_opt(
                &cf,
                read_opts,
                rocksdb::IteratorMode::From(after, rocksdb::Direction::Forward),
            )
        } else {
            db.iterator_cf_opt(&cf, read_opts, rocksdb::IteratorMode::Start)
        };

        let mut collected = 0usize;
        for item in iter {
            let (key, value) =
                item.map_err(|err| format!("Failed iterating {}: {}", cf_name, err))?;
            if let Some(after) = after_key {
                if key.as_ref() == after {
                    continue;
                }
            }

            if category == "account_txs"
                && !self.account_tx_snapshot_entry_is_canonical_or_unverifiable(&key)?
            {
                continue;
            }

            match entries.entry(key.to_vec()) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(value.to_vec());
                }
                std::collections::btree_map::Entry::Occupied(existing)
                    if existing.get().as_slice() == value.as_ref() => {}
                std::collections::btree_map::Entry::Occupied(_) => {
                    return Err(format!(
                        "Conflicting hot/cold {} snapshot row for key {}",
                        category,
                        hex::encode(&key)
                    ));
                }
            }
            collected += 1;
            if collected > limit as usize {
                break;
            }
        }

        Ok(())
    }

    fn export_blocks_cursor_canonical(
        &self,
        after_key: Option<&[u8]>,
        limit: u64,
        to_slot: Option<u64>,
    ) -> Result<KvPage, String> {
        if limit == 0 {
            return Ok(KvPage {
                entries: Vec::new(),
                total: 0,
                next_cursor: None,
                has_more: false,
            });
        }

        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let start_slot = parse_slot_snapshot_cursor(after_key, "blocks")?;
        let start_key = start_slot.to_be_bytes();
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = self.db.iterator_cf_opt(
            &slot_cf,
            read_opts,
            rocksdb::IteratorMode::From(&start_key, rocksdb::Direction::Forward),
        );

        let mut entries = Vec::with_capacity(limit.min(10_000) as usize);
        let mut has_more = false;
        let mut next_cursor = None;

        for item in iter {
            let (slot_key, hash_value) =
                item.map_err(|err| format!("Failed iterating Slots for block export: {}", err))?;
            if slot_key.len() != 8 || hash_value.len() != 32 {
                continue;
            }

            let mut slot_bytes = [0u8; 8];
            slot_bytes.copy_from_slice(&slot_key);
            let slot = u64::from_be_bytes(slot_bytes);
            if slot < start_slot {
                continue;
            }
            if to_slot.is_some_and(|upper| slot > upper) {
                break;
            }

            if entries.len() == limit as usize {
                has_more = true;
                break;
            }

            let mut block_hash = [0u8; 32];
            block_hash.copy_from_slice(&hash_value);
            let block = self
                .get_block(&Hash(block_hash))
                .map_err(|err| format!("Failed reading canonical block {}: {}", slot, err))?
                .ok_or_else(|| format!("Canonical block {} missing from hot/cold storage", slot))?;
            let canonical = canonical_block_snapshot_value_from_block(block)?;
            entries.push((block_hash.to_vec(), canonical));
            next_cursor = Some(slot.to_be_bytes().to_vec());
        }

        Ok(KvPage {
            entries,
            total: 0,
            next_cursor: if has_more { next_cursor } else { None },
            has_more,
        })
    }

    fn export_canonical_tx_snapshot_cursor(
        &self,
        category: CanonicalTxSnapshotCategory,
        after_key: Option<&[u8]>,
        limit: u64,
        to_slot: Option<u64>,
    ) -> Result<KvPage, String> {
        if limit == 0 {
            return Ok(KvPage {
                entries: Vec::new(),
                total: 0,
                next_cursor: None,
                has_more: false,
            });
        }

        let category_name = match category {
            CanonicalTxSnapshotCategory::Transactions => "transactions",
            CanonicalTxSnapshotCategory::TxBySlot => "tx_by_slot",
            CanonicalTxSnapshotCategory::TxToSlot => "tx_to_slot",
            CanonicalTxSnapshotCategory::TxMeta => "tx_meta",
        };
        let tx_meta_cf = if matches!(category, CanonicalTxSnapshotCategory::TxMeta) {
            Some(
                self.db
                    .cf_handle(CF_TX_META)
                    .ok_or_else(|| "Transaction metadata CF not found".to_string())?,
            )
        } else {
            None
        };

        let (start_slot, after_index) = parse_tx_snapshot_cursor(after_key, category_name)?;

        let make_entry = |slot: u64,
                          tx_index: u64,
                          tx_hash: Hash,
                          tx: &crate::transaction::Transaction|
         -> Result<Option<SnapshotEntry>, String> {
            match category {
                CanonicalTxSnapshotCategory::Transactions => Ok(Some((
                    tx_hash.0.to_vec(),
                    canonical_transaction_snapshot_value(tx)?,
                ))),
                CanonicalTxSnapshotCategory::TxBySlot => Ok(Some((
                    encode_tx_snapshot_cursor(slot, tx_index),
                    tx_hash.0.to_vec(),
                ))),
                CanonicalTxSnapshotCategory::TxToSlot => {
                    Ok(Some((tx_hash.0.to_vec(), slot.to_be_bytes().to_vec())))
                }
                CanonicalTxSnapshotCategory::TxMeta => {
                    let cf = tx_meta_cf
                        .as_ref()
                        .ok_or_else(|| "Transaction metadata CF not found".to_string())?;
                    let meta = self
                        .db
                        .get_cf(cf, tx_hash.0)
                        .map_err(|err| format!("Failed reading tx metadata: {}", err))?;
                    Ok(meta.map(|value| (tx_hash.0.to_vec(), value.to_vec())))
                }
            }
        };

        let limit = limit as usize;
        let mut merged = std::collections::BTreeMap::<Vec<u8>, SnapshotEntry>::new();

        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let start_key = start_slot.to_be_bytes();
        let mut slot_read_opts = rocksdb::ReadOptions::default();
        slot_read_opts.set_total_order_seek(true);
        let slot_iter = self.db.iterator_cf_opt(
            &slot_cf,
            slot_read_opts,
            rocksdb::IteratorMode::From(&start_key, rocksdb::Direction::Forward),
        );
        let mut canonical_has_more = false;

        'canonical: for item in slot_iter {
            let (slot_key, _) = item.map_err(|err| {
                format!(
                    "Failed iterating Slots for {} export: {}",
                    category_name, err
                )
            })?;
            if slot_key.len() != 8 {
                continue;
            }
            let mut slot_bytes = [0u8; 8];
            slot_bytes.copy_from_slice(&slot_key);
            let slot = u64::from_be_bytes(slot_bytes);
            if slot < start_slot {
                continue;
            }
            if to_slot.is_some_and(|upper| slot > upper) {
                break;
            }
            let Some(block) = self.get_block_by_slot(slot)? else {
                continue;
            };
            for (tx_index, tx) in block.transactions.iter().enumerate() {
                let tx_index = tx_index as u64;
                if slot == start_slot && after_index.is_some_and(|index| tx_index <= index) {
                    continue;
                }
                let tx_hash = tx.signature();
                let Some(entry) = make_entry(slot, tx_index, tx_hash, tx)? else {
                    continue;
                };
                merged.insert(encode_tx_snapshot_cursor(slot, tx_index), entry);
                if merged.len() > limit {
                    canonical_has_more = true;
                    break 'canonical;
                }
            }
        }

        let tx_by_slot_cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "TX by slot CF not found".to_string())?;
        let start_key = encode_tx_snapshot_cursor(start_slot, 0);
        let mut tx_read_opts = rocksdb::ReadOptions::default();
        tx_read_opts.set_total_order_seek(true);
        let tx_iter = self.db.iterator_cf_opt(
            &tx_by_slot_cf,
            tx_read_opts,
            rocksdb::IteratorMode::From(&start_key, rocksdb::Direction::Forward),
        );
        let canonical_frontier = canonical_has_more
            .then(|| merged.last_key_value().map(|(cursor, _)| cursor.clone()))
            .flatten();
        let mut tx_index_has_more = false;

        for item in tx_iter {
            let (key, value) = item.map_err(|err| {
                format!(
                    "Failed iterating tx_by_slot for {} export: {}",
                    category_name, err
                )
            })?;
            if canonical_frontier
                .as_ref()
                .is_some_and(|frontier| key.as_ref() > frontier.as_slice())
            {
                break;
            }
            let Some((slot, tx_index, tx_hash)) = parse_tx_by_slot_snapshot_row(&key, &value)?
            else {
                continue;
            };
            if slot < start_slot {
                continue;
            }
            if to_slot.is_some_and(|upper| slot > upper) {
                break;
            }
            if slot == start_slot && after_index.is_some_and(|index| tx_index <= index) {
                continue;
            }
            if merged.contains_key(key.as_ref()) {
                continue;
            }
            if self.get_block_by_slot(slot)?.is_some() {
                continue;
            }

            let Some(tx) = self.get_transaction(&tx_hash)? else {
                continue;
            };
            if tx.signature() != tx_hash {
                continue;
            }

            let Some(entry) = make_entry(slot, tx_index, tx_hash, &tx)? else {
                continue;
            };
            merged.insert(key.to_vec(), entry);
            if merged.len() > limit {
                tx_index_has_more = true;
                break;
            }
        }

        let mut ordered: Vec<_> = merged.into_iter().collect();
        let has_more = canonical_has_more || tx_index_has_more || ordered.len() > limit;
        if ordered.len() > limit {
            ordered.truncate(limit);
        }
        let next_cursor = ordered.last().map(|(cursor, _)| cursor.clone());
        let entries = ordered.into_iter().map(|(_, entry)| entry).collect();

        Ok(KvPage {
            entries,
            total: 0,
            next_cursor: if has_more { next_cursor } else { None },
            has_more,
        })
    }

    pub fn rebuild_tx_by_slot_index_from_blocks(&self) -> Result<u64, String> {
        const WRITE_BATCH_SIZE: usize = 10_000;

        self.clear_snapshot_category("tx_by_slot")?;

        let tx_by_slot_cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "TX by slot CF not found".to_string())?;
        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = self
            .db
            .iterator_cf_opt(&slot_cf, read_opts, rocksdb::IteratorMode::Start);

        let mut batch = WriteBatch::default();
        let mut pending = 0usize;
        let mut indexed = 0u64;

        for item in iter {
            let (slot_key, _) = item
                .map_err(|err| format!("Failed iterating Slots for tx_by_slot rebuild: {}", err))?;
            if slot_key.len() != 8 {
                continue;
            }

            let mut slot_bytes = [0u8; 8];
            slot_bytes.copy_from_slice(&slot_key);
            let slot = u64::from_be_bytes(slot_bytes);
            let Some(block) = self.get_block_by_slot(slot)? else {
                continue;
            };

            for (tx_index, tx) in block.transactions.iter().enumerate() {
                let mut key = Vec::with_capacity(16);
                key.extend_from_slice(&slot.to_be_bytes());
                key.extend_from_slice(&(tx_index as u64).to_be_bytes());
                batch.put_cf(&tx_by_slot_cf, &key, tx.signature().0);
                pending += 1;
                indexed = indexed.saturating_add(1);

                if pending >= WRITE_BATCH_SIZE {
                    self.db
                        .write(batch)
                        .map_err(|err| format!("Failed rebuilding tx_by_slot index: {}", err))?;
                    batch = WriteBatch::default();
                    pending = 0;
                }
            }
        }

        if pending > 0 {
            self.db
                .write(batch)
                .map_err(|err| format!("Failed rebuilding tx_by_slot index: {}", err))?;
        }

        Ok(indexed)
    }

    fn raw_block_slot_in_range(slot: u64, from_slot: Option<u64>, to_slot: Option<u64>) -> bool {
        if from_slot.is_some_and(|from| slot < from) {
            return false;
        }
        if to_slot.is_some_and(|to| slot > to) {
            return false;
        }
        true
    }

    fn inspect_raw_block_body_cf(
        &self,
        db: &DB,
        cf_name: &str,
        source_cold: bool,
        from_slot: Option<u64>,
        to_slot: Option<u64>,
        scan: &mut RawBlockHistoryScan,
    ) -> Result<(), String> {
        let block_cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{cf_name} CF not found"))?;
        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = db.iterator_cf_opt(&block_cf, read_opts, rocksdb::IteratorMode::Start);

        for item in iter {
            let (key, value) =
                item.map_err(|err| format!("Failed iterating raw block CF {cf_name}: {err}"))?;
            if source_cold {
                scan.report.cold_rows_scanned = scan.report.cold_rows_scanned.saturating_add(1);
            } else {
                scan.report.hot_rows_scanned = scan.report.hot_rows_scanned.saturating_add(1);
            }

            let block = match decode_snapshot_block_value(&value) {
                Ok(block) => block,
                Err(err) => {
                    scan.report.decode_errors = scan.report.decode_errors.saturating_add(1);
                    if scan.report.first_decode_error.is_none() {
                        scan.report.first_decode_error =
                            Some(format!("{cf_name}:{}:{err}", hex::encode(&key)));
                    }
                    continue;
                }
            };
            scan.report.decoded_blocks = scan.report.decoded_blocks.saturating_add(1);

            let slot = block.header.slot;
            if !Self::raw_block_slot_in_range(slot, from_slot, to_slot) {
                continue;
            }
            scan.report.body_slots_in_range = scan.report.body_slots_in_range.saturating_add(1);
            scan.report.min_body_slot = Some(
                scan.report
                    .min_body_slot
                    .map_or(slot, |current| current.min(slot)),
            );
            scan.report.max_body_slot = Some(
                scan.report
                    .max_body_slot
                    .map_or(slot, |current| current.max(slot)),
            );

            let block_hash = block.hash();
            if key.as_ref() != block_hash.0 {
                scan.report.hash_mismatch_rows = scan.report.hash_mismatch_rows.saturating_add(1);
                if scan.report.first_hash_mismatch.is_none() {
                    scan.report.first_hash_mismatch = Some(format!(
                        "{cf_name}:key={} block_hash={} slot={slot}",
                        hex::encode(&key),
                        block_hash.to_hex()
                    ));
                }
                continue;
            }

            match scan.seen_body_slots.entry(slot) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(block_hash.0);
                }
                std::collections::btree_map::Entry::Occupied(existing)
                    if existing.get() == &block_hash.0 =>
                {
                    scan.report.duplicate_identical_body_slots =
                        scan.report.duplicate_identical_body_slots.saturating_add(1);
                    continue;
                }
                std::collections::btree_map::Entry::Occupied(_) => {
                    scan.report.duplicate_conflicting_body_slots = scan
                        .report
                        .duplicate_conflicting_body_slots
                        .saturating_add(1);
                    if scan.report.first_duplicate_conflicting_body_slot.is_none() {
                        scan.report.first_duplicate_conflicting_body_slot = Some(slot);
                    }
                    scan.repairable.remove(&slot);
                    continue;
                }
            }

            let slot_key = slot.to_be_bytes();
            match self
                .db
                .get_cf(&slot_cf, slot_key)
                .map_err(|err| format!("Failed reading slot cursor {slot}: {err}"))?
            {
                Some(existing) if existing.as_slice() == block_hash.0.as_slice() => {
                    scan.report.existing_slot_cursors =
                        scan.report.existing_slot_cursors.saturating_add(1);
                }
                Some(_) => {
                    scan.report.conflicting_slot_cursors =
                        scan.report.conflicting_slot_cursors.saturating_add(1);
                    if scan.report.first_conflicting_slot_cursor.is_none() {
                        scan.report.first_conflicting_slot_cursor = Some(slot);
                    }
                    scan.repairable.remove(&slot);
                }
                None => {
                    scan.report.missing_slot_cursors =
                        scan.report.missing_slot_cursors.saturating_add(1);
                    if scan.report.first_missing_slot_cursor.is_none() {
                        scan.report.first_missing_slot_cursor = Some(slot);
                    }
                    scan.repairable.insert(slot, block_hash.0);
                }
            }
        }

        Ok(())
    }

    fn inspect_orphan_slot_cursors(
        &self,
        from_slot: Option<u64>,
        to_slot: Option<u64>,
        body_hashes_by_slot: &std::collections::BTreeMap<u64, [u8; 32]>,
        report: &mut RawBlockHistoryRepairReport,
    ) -> Result<(), String> {
        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let start = from_slot.unwrap_or(0).to_be_bytes();
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = self.db.iterator_cf_opt(
            &slot_cf,
            read_opts,
            rocksdb::IteratorMode::From(&start, rocksdb::Direction::Forward),
        );

        for item in iter {
            let (key, value) =
                item.map_err(|err| format!("Failed iterating slot cursors: {err}"))?;
            if key.len() != 8 {
                continue;
            }
            let mut slot_bytes = [0u8; 8];
            slot_bytes.copy_from_slice(&key);
            let slot = u64::from_be_bytes(slot_bytes);
            if !Self::raw_block_slot_in_range(slot, from_slot, to_slot) {
                if to_slot.is_some_and(|to| slot > to) {
                    break;
                }
                continue;
            }
            report.slot_cursors_scanned = report.slot_cursors_scanned.saturating_add(1);

            if value.len() != 32 {
                report.invalid_slot_cursors = report.invalid_slot_cursors.saturating_add(1);
                if report.first_invalid_slot_cursor.is_none() {
                    report.first_invalid_slot_cursor = Some(slot);
                }
                continue;
            }
            match body_hashes_by_slot.get(&slot) {
                Some(block_hash) if block_hash.as_slice() == value.as_ref() => {}
                _ => {
                    report.orphan_slot_cursors = report.orphan_slot_cursors.saturating_add(1);
                    if report.first_orphan_slot_cursor.is_none() {
                        report.first_orphan_slot_cursor = Some(slot);
                    }
                }
            }
        }

        Ok(())
    }

    pub fn repair_missing_slot_cursors_from_raw_blocks(
        &self,
        from_slot: Option<u64>,
        to_slot: Option<u64>,
        dry_run: bool,
    ) -> Result<RawBlockHistoryRepairReport, String> {
        if let (Some(from), Some(to)) = (from_slot, to_slot) {
            if to < from {
                return Err("--to-slot must be >= --from-slot".to_string());
            }
        }

        let mut scan = RawBlockHistoryScan {
            report: RawBlockHistoryRepairReport {
                dry_run,
                from_slot,
                to_slot,
                ..RawBlockHistoryRepairReport::default()
            },
            ..RawBlockHistoryScan::default()
        };

        self.inspect_raw_block_body_cf(&self.db, CF_BLOCKS, false, from_slot, to_slot, &mut scan)?;
        if let Some(cold) = self.cold_db.as_ref() {
            self.inspect_raw_block_body_cf(
                cold.as_ref(),
                COLD_CF_BLOCKS,
                true,
                from_slot,
                to_slot,
                &mut scan,
            )?;
        }
        self.inspect_orphan_slot_cursors(
            from_slot,
            to_slot,
            &scan.seen_body_slots,
            &mut scan.report,
        )?;

        scan.report.repairable_slot_cursors = scan.repairable.len() as u64;
        scan.report.first_repairable_slot_cursor = scan.repairable.keys().next().copied();

        let RawBlockHistoryScan {
            repairable,
            mut report,
            ..
        } = scan;

        if !dry_run {
            if report.has_conflicts() {
                return Err(format!(
                    "Refusing raw block slot-cursor repair with conflicts: decode_errors={} hash_mismatch_rows={} duplicate_conflicting_body_slots={} conflicting_slot_cursors={} orphan_slot_cursors={} invalid_slot_cursors={}",
                    report.decode_errors,
                    report.hash_mismatch_rows,
                    report.duplicate_conflicting_body_slots,
                    report.conflicting_slot_cursors,
                    report.orphan_slot_cursors,
                    report.invalid_slot_cursors
                ));
            }

            let slot_cf = self
                .db
                .cf_handle(CF_SLOTS)
                .ok_or_else(|| "Slots CF not found".to_string())?;
            let mut batch = WriteBatch::default();
            let mut pending = 0usize;
            for (slot, hash) in repairable {
                batch.put_cf(&slot_cf, slot.to_be_bytes(), hash);
                pending += 1;
                report.repaired_slot_cursors = report.repaired_slot_cursors.saturating_add(1);
                if report.first_repaired_slot_cursor.is_none() {
                    report.first_repaired_slot_cursor = Some(slot);
                }
                if pending >= 10_000 {
                    self.db.write(batch).map_err(|err| {
                        format!("Failed writing repaired slot cursor batch: {err}")
                    })?;
                    batch = WriteBatch::default();
                    pending = 0;
                }
            }
            if pending > 0 {
                self.db
                    .write(batch)
                    .map_err(|err| format!("Failed writing repaired slot cursor batch: {err}"))?;
            }
        }

        Ok(report)
    }

    pub fn clear_account_tx_counters(&self) -> Result<u64, String> {
        const DELETE_BATCH_SIZE: usize = 10_000;
        let stats_cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let prefix = b"atxc:";
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = self.db.iterator_cf_opt(
            &stats_cf,
            read_opts,
            rocksdb::IteratorMode::From(prefix, rocksdb::Direction::Forward),
        );

        let mut batch = WriteBatch::default();
        let mut pending = 0usize;
        let mut deleted = 0u64;
        for item in iter {
            let (key, _) =
                item.map_err(|err| format!("Failed iterating account tx counters: {}", err))?;
            if !key.starts_with(prefix) {
                break;
            }
            batch.delete_cf(&stats_cf, key);
            pending += 1;

            if pending >= DELETE_BATCH_SIZE {
                self.db
                    .write(batch)
                    .map_err(|err| format!("Failed clearing account tx counters: {}", err))?;
                deleted = deleted.saturating_add(pending as u64);
                batch = WriteBatch::default();
                pending = 0;
            }
        }

        if pending > 0 {
            self.db
                .write(batch)
                .map_err(|err| format!("Failed clearing account tx counters: {}", err))?;
            deleted = deleted.saturating_add(pending as u64);
        }

        Ok(deleted)
    }

    fn count_rows_in_db(db: &DB, cf_name: &str) -> Result<u64, String> {
        let cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{} CF not found", cf_name))?;
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = db.iterator_cf_opt(&cf, read_opts, rocksdb::IteratorMode::Start);
        let mut rows = 0u64;
        for item in iter {
            item.map_err(|err| format!("Failed iterating {}: {}", cf_name, err))?;
            rows = rows.saturating_add(1);
        }
        Ok(rows)
    }

    fn count_stats_prefix(&self, prefix: &[u8]) -> Result<u64, String> {
        let stats_cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = self.db.iterator_cf_opt(
            &stats_cf,
            read_opts,
            rocksdb::IteratorMode::From(prefix, rocksdb::Direction::Forward),
        );
        let mut rows = 0u64;
        for item in iter {
            let (key, _) = item.map_err(|err| format!("Failed iterating stats prefix: {}", err))?;
            if !key.starts_with(prefix) {
                break;
            }
            rows = rows.saturating_add(1);
        }
        Ok(rows)
    }

    pub fn account_txs_rebuild_report_from_blocks(
        &self,
    ) -> Result<AccountTxsRebuildReport, String> {
        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        let mut report = AccountTxsRebuildReport {
            source: AccountTxsRebuildSource::Blocks,
            last_slot: self.get_last_slot().unwrap_or(0),
            existing_hot_rows: Self::count_rows_in_db(&self.db, CF_ACCOUNT_TXS)?,
            existing_cold_rows: if let Some(ref cold) = self.cold_db {
                Self::count_rows_in_db(cold, COLD_CF_ACCOUNT_TXS)?
            } else {
                0
            },
            existing_counter_keys: self.count_stats_prefix(b"atxc:")?,
            ..AccountTxsRebuildReport::default()
        };

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = self
            .db
            .iterator_cf_opt(&slot_cf, read_opts, rocksdb::IteratorMode::Start);

        for item in iter {
            let (slot_key, hash_value) =
                item.map_err(|err| format!("Failed iterating Slots for rebuild report: {}", err))?;
            if slot_key.len() != 8 || hash_value.len() != 32 {
                continue;
            }

            let mut slot_bytes = [0u8; 8];
            slot_bytes.copy_from_slice(&slot_key);
            let slot = u64::from_be_bytes(slot_bytes);
            let mut hash = [0u8; 32];
            hash.copy_from_slice(&hash_value);
            report.canonical_slots = report.canonical_slots.saturating_add(1);
            if slot == 0 {
                report.reached_genesis = true;
            }

            let Some(block) = self.get_block(&Hash(hash))? else {
                report.missing_block_bodies = report.missing_block_bodies.saturating_add(1);
                report.first_missing_block_slot.get_or_insert(slot);
                continue;
            };

            report.available_blocks = report.available_blocks.saturating_add(1);
            if block.transactions.is_empty() && block.header.tx_root != Hash::default() {
                report.header_only_blocks = report.header_only_blocks.saturating_add(1);
                report.first_header_only_slot.get_or_insert(slot);
                continue;
            }

            report.transactions_seen = report
                .transactions_seen
                .saturating_add(block.transactions.len() as u64);
            report.expected_account_tx_rows = report.expected_account_tx_rows.saturating_add(
                super::secondary_indexes::account_tx_index_entries_for_block(&block).len() as u64,
            );
        }

        Ok(report)
    }

    pub fn account_txs_rebuild_report_from_parent_chain(
        &self,
    ) -> Result<AccountTxsRebuildReport, String> {
        let last_slot = self.get_last_slot().unwrap_or(0);
        let mut report = AccountTxsRebuildReport {
            source: AccountTxsRebuildSource::ParentChain,
            last_slot,
            existing_hot_rows: Self::count_rows_in_db(&self.db, CF_ACCOUNT_TXS)?,
            existing_cold_rows: if let Some(ref cold) = self.cold_db {
                Self::count_rows_in_db(cold, COLD_CF_ACCOUNT_TXS)?
            } else {
                0
            },
            existing_counter_keys: self.count_stats_prefix(b"atxc:")?,
            ..AccountTxsRebuildReport::default()
        };

        let Some(tip) = self.get_block_by_slot(last_slot)? else {
            report.missing_block_bodies = 1;
            report.first_missing_block_slot = Some(last_slot);
            return Ok(report);
        };

        let mut current = tip;
        let mut seen = std::collections::HashSet::<Hash>::new();
        loop {
            let block_hash = current.hash();
            if !seen.insert(block_hash) {
                return Err(format!(
                    "canonical parent chain cycle detected at slot {} hash {}",
                    current.header.slot,
                    block_hash.to_hex()
                ));
            }

            report.canonical_slots = report.canonical_slots.saturating_add(1);
            report.available_blocks = report.available_blocks.saturating_add(1);
            if current.transactions.is_empty() && current.header.tx_root != Hash::default() {
                report.header_only_blocks = report.header_only_blocks.saturating_add(1);
                report
                    .first_header_only_slot
                    .get_or_insert(current.header.slot);
            } else {
                report.transactions_seen = report
                    .transactions_seen
                    .saturating_add(current.transactions.len() as u64);
                report.expected_account_tx_rows = report.expected_account_tx_rows.saturating_add(
                    super::secondary_indexes::account_tx_index_entries_for_block(&current).len()
                        as u64,
                );
            }

            if current.header.slot == 0 {
                report.reached_genesis = true;
                break;
            }

            if current.header.parent_hash == Hash::default() {
                report.missing_block_bodies = report.missing_block_bodies.saturating_add(1);
                report.first_missing_block_slot.get_or_insert(0);
                break;
            }

            let parent_hash = current.header.parent_hash;
            match self.get_block(&parent_hash)? {
                Some(parent) => current = parent,
                None => {
                    report.missing_block_bodies = report.missing_block_bodies.saturating_add(1);
                    report
                        .first_missing_block_slot
                        .get_or_insert(current.header.slot.saturating_sub(1));
                    break;
                }
            }
        }

        Ok(report)
    }

    pub fn account_txs_rebuild_report_from_tx_index(
        &self,
    ) -> Result<AccountTxsRebuildReport, String> {
        let tx_by_slot_cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "tx_by_slot CF not found".to_string())?;

        let mut report = AccountTxsRebuildReport {
            source: AccountTxsRebuildSource::TxIndex,
            last_slot: self.get_last_slot().unwrap_or(0),
            existing_hot_rows: Self::count_rows_in_db(&self.db, CF_ACCOUNT_TXS)?,
            existing_cold_rows: if let Some(ref cold) = self.cold_db {
                Self::count_rows_in_db(cold, COLD_CF_ACCOUNT_TXS)?
            } else {
                0
            },
            existing_counter_keys: self.count_stats_prefix(b"atxc:")?,
            ..AccountTxsRebuildReport::default()
        };

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = self
            .db
            .iterator_cf_opt(&tx_by_slot_cf, read_opts, rocksdb::IteratorMode::Start);

        for item in iter {
            let (key, value) =
                item.map_err(|err| format!("Failed iterating tx_by_slot for report: {}", err))?;
            let Some((slot, tx_index, tx_hash)) = parse_tx_by_slot_snapshot_row(&key, &value)?
            else {
                continue;
            };

            let tx_index_usize = usize::try_from(tx_index).map_err(|_| {
                format!(
                    "tx_by_slot sequence {} at slot {} does not fit this platform",
                    tx_index, slot
                )
            })?;
            report.tx_by_slot_rows = report.tx_by_slot_rows.saturating_add(1);
            report.oldest_tx_slot = Some(
                report
                    .oldest_tx_slot
                    .map_or(slot, |oldest| oldest.min(slot)),
            );
            report.newest_tx_slot = Some(
                report
                    .newest_tx_slot
                    .map_or(slot, |newest| newest.max(slot)),
            );

            match self.get_transaction(&tx_hash)? {
                Some(tx) => {
                    report.transactions_seen = report.transactions_seen.saturating_add(1);
                    report.expected_account_tx_rows =
                        report.expected_account_tx_rows.saturating_add(
                            super::secondary_indexes::account_tx_index_entries_for_transaction(
                                slot,
                                tx_index_usize,
                                &tx,
                            )
                            .len() as u64,
                        );
                }
                None => {
                    report.missing_transactions = report.missing_transactions.saturating_add(1);
                    report.first_missing_transaction_slot.get_or_insert(slot);
                }
            }
        }

        Ok(report)
    }

    fn count_account_txs_rows_for_account_in_db(
        db: &DB,
        cf_name: &str,
        account: &Pubkey,
    ) -> Result<u64, String> {
        let cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{} CF not found", cf_name))?;
        let prefix = account.0.to_vec();
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = db.iterator_cf_opt(
            &cf,
            read_opts,
            rocksdb::IteratorMode::From(&prefix, rocksdb::Direction::Forward),
        );

        let mut count = 0u64;
        for item in iter {
            let (key, _) = item.map_err(|err| format!("Failed iterating {}: {}", cf_name, err))?;
            if !key.starts_with(&prefix) {
                break;
            }
            if key.len() >= 32 + 8 + 4 + 32 {
                count = count.saturating_add(1);
            }
        }
        Ok(count)
    }

    fn transaction_indexes_account(
        slot: u64,
        tx_index: usize,
        tx: &crate::transaction::Transaction,
        account: &Pubkey,
    ) -> bool {
        super::secondary_indexes::account_tx_index_entries_for_transaction(slot, tx_index, tx)
            .into_iter()
            .any(|(indexed_account, _)| indexed_account == *account)
    }

    pub fn inspect_account_txs_sources(
        &self,
        account: &Pubkey,
        slots: &[u64],
        max_matches: usize,
    ) -> Result<AccountTxsSourceInspection, String> {
        let tx_by_slot_cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "tx_by_slot CF not found".to_string())?;

        let mut slot_reports = std::collections::BTreeMap::<u64, AccountTxsSlotInspection>::new();
        for slot in slots {
            slot_reports
                .entry(*slot)
                .or_insert_with(|| AccountTxsSlotInspection {
                    slot: *slot,
                    ..AccountTxsSlotInspection::default()
                });
        }

        for (slot, report) in slot_reports.iter_mut() {
            if let Some(block) = self.get_block_by_slot(*slot)? {
                report.block_present = true;
                report.block_tx_count = block.transactions.len() as u64;
                report.block_matching_account_rows = block
                    .transactions
                    .iter()
                    .enumerate()
                    .filter(|(tx_index, tx)| {
                        Self::transaction_indexes_account(*slot, *tx_index, tx, account)
                    })
                    .count() as u64;
            }
        }

        let mut inspection = AccountTxsSourceInspection {
            account: *account,
            cached_account_tx_count: self.count_account_txs(account)?,
            hot_account_tx_rows: Self::count_account_txs_rows_for_account_in_db(
                &self.db,
                CF_ACCOUNT_TXS,
                account,
            )?,
            cold_account_tx_rows: if let Some(ref cold) = self.cold_db {
                Self::count_account_txs_rows_for_account_in_db(cold, COLD_CF_ACCOUNT_TXS, account)?
            } else {
                0
            },
            indexed_signatures: self.get_account_tx_signatures_paginated(
                account,
                max_matches,
                None,
            )?,
            tx_by_slot_rows: 0,
            tx_by_slot_missing_transactions: 0,
            tx_by_slot_oldest_slot: None,
            tx_by_slot_newest_slot: None,
            tx_by_slot_matching_account_rows: 0,
            tx_by_slot_matching_signatures: Vec::new(),
            slots: Vec::new(),
        };

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = self
            .db
            .iterator_cf_opt(&tx_by_slot_cf, read_opts, rocksdb::IteratorMode::Start);

        for item in iter {
            let (key, value) =
                item.map_err(|err| format!("Failed iterating tx_by_slot: {}", err))?;
            let Some((slot, tx_index, tx_hash)) = parse_tx_by_slot_snapshot_row(&key, &value)?
            else {
                continue;
            };

            inspection.tx_by_slot_rows = inspection.tx_by_slot_rows.saturating_add(1);
            inspection.tx_by_slot_oldest_slot = Some(
                inspection
                    .tx_by_slot_oldest_slot
                    .map_or(slot, |oldest| oldest.min(slot)),
            );
            inspection.tx_by_slot_newest_slot = Some(
                inspection
                    .tx_by_slot_newest_slot
                    .map_or(slot, |newest| newest.max(slot)),
            );

            let tx = self.get_transaction(&tx_hash)?;
            if tx.is_none() {
                inspection.tx_by_slot_missing_transactions =
                    inspection.tx_by_slot_missing_transactions.saturating_add(1);
            }

            if let Some(slot_report) = slot_reports.get_mut(&slot) {
                slot_report.tx_by_slot_rows = slot_report.tx_by_slot_rows.saturating_add(1);
                if tx.is_some() {
                    slot_report.tx_by_slot_tx_bodies_present =
                        slot_report.tx_by_slot_tx_bodies_present.saturating_add(1);
                }
            }

            let Some(tx) = tx else {
                continue;
            };
            let tx_index_usize = usize::try_from(tx_index).map_err(|_| {
                format!(
                    "tx_by_slot sequence {} at slot {} does not fit this platform",
                    tx_index, slot
                )
            })?;

            if Self::transaction_indexes_account(slot, tx_index_usize, &tx, account) {
                inspection.tx_by_slot_matching_account_rows = inspection
                    .tx_by_slot_matching_account_rows
                    .saturating_add(1);
                if inspection.tx_by_slot_matching_signatures.len() < max_matches {
                    inspection
                        .tx_by_slot_matching_signatures
                        .push((tx_hash, slot));
                }
                if let Some(slot_report) = slot_reports.get_mut(&slot) {
                    slot_report.tx_by_slot_matching_account_rows = slot_report
                        .tx_by_slot_matching_account_rows
                        .saturating_add(1);
                }
            }
        }

        inspection.slots = slot_reports.into_values().collect();
        Ok(inspection)
    }

    fn ensure_account_txs_rebuild_source_complete(&self) -> Result<(), String> {
        self.ensure_account_txs_rows_rebuildable_in_db(&self.db, CF_ACCOUNT_TXS)?;
        if let Some(ref cold) = self.cold_db {
            self.ensure_account_txs_rows_rebuildable_in_db(cold, COLD_CF_ACCOUNT_TXS)?;
        }

        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = self
            .db
            .iterator_cf_opt(&slot_cf, read_opts, rocksdb::IteratorMode::Start);
        for item in iter {
            let (slot_key, hash_value) =
                item.map_err(|err| format!("Failed iterating Slots for rebuild guard: {}", err))?;
            if slot_key.len() != 8 || hash_value.len() != 32 {
                continue;
            }
            let mut slot_bytes = [0u8; 8];
            slot_bytes.copy_from_slice(&slot_key);
            let slot = u64::from_be_bytes(slot_bytes);
            let mut hash = [0u8; 32];
            hash.copy_from_slice(&hash_value);
            if self.get_block(&Hash(hash))?.is_none() {
                return Err(format!(
                    "Refusing destructive account_txs rebuild: canonical slot {} has no source block body",
                    slot
                ));
            }
        }

        Ok(())
    }

    fn account_txs_row_backed_by_tx_index(
        &self,
        slot: u64,
        seq: u32,
        tx_hash: &Hash,
    ) -> Result<bool, String> {
        let tx_by_slot_cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "tx_by_slot CF not found".to_string())?;

        let mut key = Vec::with_capacity(16);
        key.extend_from_slice(&slot.to_be_bytes());
        key.extend_from_slice(&(seq as u64).to_be_bytes());
        let Some(value) = self
            .db
            .get_cf(&tx_by_slot_cf, &key)
            .map_err(|err| format!("Failed reading tx_by_slot row: {}", err))?
        else {
            return Ok(false);
        };
        if value.len() != 32 || value.as_slice() != tx_hash.0.as_slice() {
            return Ok(false);
        }

        self.get_transaction(tx_hash).map(|tx| tx.is_some())
    }

    fn ensure_account_txs_rows_rebuildable_from_tx_index_in_db(
        &self,
        db: &DB,
        cf_name: &str,
    ) -> Result<(), String> {
        let cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{} CF not found", cf_name))?;
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = db.iterator_cf_opt(&cf, read_opts, rocksdb::IteratorMode::Start);
        for item in iter {
            let (key, _) = item.map_err(|err| {
                format!(
                    "Failed iterating {} for tx-index rebuild guard: {}",
                    cf_name, err
                )
            })?;
            let Some((slot, seq, tx_hash)) = parse_account_tx_snapshot_key(&key)? else {
                continue;
            };
            if !self.account_txs_row_backed_by_tx_index(slot, seq, &tx_hash)? {
                return Err(format!(
                    "Refusing destructive account_txs rebuild: indexed tx {} at slot {} seq {} is not backed by tx_by_slot + transaction body",
                    tx_hash.to_hex(),
                    slot,
                    seq
                ));
            }
        }

        Ok(())
    }

    fn ensure_account_txs_rows_rebuildable_in_db(
        &self,
        db: &DB,
        cf_name: &str,
    ) -> Result<(), String> {
        let cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{} CF not found", cf_name))?;
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = db.iterator_cf_opt(&cf, read_opts, rocksdb::IteratorMode::Start);
        for item in iter {
            let (key, _) = item.map_err(|err| {
                format!("Failed iterating {} for rebuild guard: {}", cf_name, err)
            })?;
            let Some((slot, _, tx_hash)) = parse_account_tx_snapshot_key(&key)? else {
                continue;
            };
            let Some(block) = self.get_block_by_slot(slot)? else {
                return Err(format!(
                    "Refusing destructive account_txs rebuild: indexed slot {} has no source block body",
                    slot
                ));
            };
            if !block
                .transactions
                .iter()
                .any(|tx| tx.signature() == tx_hash)
            {
                return Err(format!(
                    "Refusing destructive account_txs rebuild: indexed tx {} is not in source block {}",
                    tx_hash.to_hex(),
                    slot
                ));
            }
        }

        Ok(())
    }

    pub fn rebuild_account_txs_index_from_blocks(&self) -> Result<u64, String> {
        const WRITE_BATCH_SIZE: usize = 10_000;

        self.ensure_account_txs_rebuild_source_complete()?;
        self.clear_snapshot_category("account_txs")?;

        let account_txs_cf = self
            .db
            .cf_handle(CF_ACCOUNT_TXS)
            .ok_or_else(|| "Account txs CF not found".to_string())?;
        let stats_cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = self
            .db
            .iterator_cf_opt(&slot_cf, read_opts, rocksdb::IteratorMode::Start);

        let mut batch = WriteBatch::default();
        let mut pending = 0usize;
        let mut indexed = 0u64;
        let mut counters = std::collections::BTreeMap::<Pubkey, u64>::new();

        for item in iter {
            let (slot_key, _) = item.map_err(|err| {
                format!("Failed iterating Slots for account_txs rebuild: {}", err)
            })?;
            if slot_key.len() != 8 {
                continue;
            }

            let mut slot_bytes = [0u8; 8];
            slot_bytes.copy_from_slice(&slot_key);
            let slot = u64::from_be_bytes(slot_bytes);
            let Some(block) = self.get_block_by_slot(slot)? else {
                continue;
            };

            for (account, key) in
                super::secondary_indexes::account_tx_index_entries_for_block(&block)
            {
                batch.put_cf(&account_txs_cf, &key, []);
                *counters.entry(account).or_default() += 1;
                pending += 1;
                indexed = indexed.saturating_add(1);

                if pending >= WRITE_BATCH_SIZE {
                    self.db
                        .write(batch)
                        .map_err(|err| format!("Failed rebuilding account_txs index: {}", err))?;
                    batch = WriteBatch::default();
                    pending = 0;
                }
            }
        }

        for (account, count) in counters {
            let mut counter_key = Vec::with_capacity(5 + 32);
            counter_key.extend_from_slice(b"atxc:");
            counter_key.extend_from_slice(&account.0);
            batch.put_cf(&stats_cf, &counter_key, count.to_le_bytes());
            pending += 1;

            if pending >= WRITE_BATCH_SIZE {
                self.db
                    .write(batch)
                    .map_err(|err| format!("Failed rebuilding account tx counters: {}", err))?;
                batch = WriteBatch::default();
                pending = 0;
            }
        }

        if pending > 0 {
            self.db
                .write(batch)
                .map_err(|err| format!("Failed finalizing account_txs index rebuild: {}", err))?;
        }

        Ok(indexed)
    }

    pub fn rebuild_account_txs_index_from_blocks_with_report(
        &self,
        dry_run: bool,
    ) -> Result<AccountTxsRebuildReport, String> {
        let mut report = self.account_txs_rebuild_report_from_blocks()?;
        report.dry_run = dry_run;
        if dry_run {
            return Ok(report);
        }

        report.rebuilt_rows = self.rebuild_account_txs_index_from_blocks()?;
        report.after_hot_rows = Self::count_rows_in_db(&self.db, CF_ACCOUNT_TXS)?;
        report.after_counter_keys = self.count_stats_prefix(b"atxc:")?;
        Ok(report)
    }

    pub fn rebuild_account_txs_index_from_parent_chain_with_report(
        &self,
        dry_run: bool,
    ) -> Result<AccountTxsRebuildReport, String> {
        const WRITE_BATCH_SIZE: usize = 10_000;

        let mut report = self.account_txs_rebuild_report_from_parent_chain()?;
        report.dry_run = dry_run;
        if dry_run {
            return Ok(report);
        }
        if !report.source_complete() {
            return Err(format!(
                "Refusing account_txs parent-chain rebuild: source incomplete (reached_genesis={}, missing_block_bodies={}, header_only_blocks={})",
                report.reached_genesis, report.missing_block_bodies, report.header_only_blocks
            ));
        }

        if report.existing_hot_rows > 0 || report.existing_cold_rows > 0 {
            self.ensure_account_txs_rebuild_source_complete()?;
        }

        self.clear_snapshot_category("account_txs")?;

        let account_txs_cf = self
            .db
            .cf_handle(CF_ACCOUNT_TXS)
            .ok_or_else(|| "Account txs CF not found".to_string())?;
        let stats_cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let tx_cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;
        let tx_to_slot_cf = self
            .db
            .cf_handle(CF_TX_TO_SLOT)
            .ok_or_else(|| "tx_to_slot CF not found".to_string())?;
        let tx_by_slot_cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "tx_by_slot CF not found".to_string())?;
        let shielded_txs_cf = self.db.cf_handle(CF_SHIELDED_TXS);

        let tip = self
            .get_block_by_slot(report.last_slot)?
            .ok_or_else(|| format!("canonical tip block {} is missing", report.last_slot))?;
        let mut current = tip;
        let mut seen = std::collections::HashSet::<Hash>::new();
        let mut batch = WriteBatch::default();
        let mut pending = 0usize;
        let mut indexed = 0u64;
        let mut counters = std::collections::BTreeMap::<Pubkey, u64>::new();

        loop {
            let block_hash = current.hash();
            if !seen.insert(block_hash) {
                return Err(format!(
                    "canonical parent chain cycle detected at slot {} hash {}",
                    current.header.slot,
                    block_hash.to_hex()
                ));
            }

            batch.put_cf(&slot_cf, current.header.slot.to_be_bytes(), block_hash.0);

            for (tx_index, tx) in current.transactions.iter().enumerate() {
                let sig = tx.signature();

                let mut tx_value = Vec::with_capacity(512);
                tx_value.push(0xBC);
                append_legacy_bincode(&mut tx_value, tx, "transaction")
                    .map_err(|err| format!("Failed to serialize tx {}: {}", sig.to_hex(), err))?;
                batch.put_cf(&tx_cf, sig.0, &tx_value);
                batch.put_cf(&tx_to_slot_cf, sig.0, current.header.slot.to_be_bytes());

                let mut by_slot_key = Vec::with_capacity(16);
                by_slot_key.extend_from_slice(&current.header.slot.to_be_bytes());
                by_slot_key.extend_from_slice(&(tx_index as u64).to_be_bytes());
                batch.put_cf(&tx_by_slot_cf, &by_slot_key, sig.0);

                if super::is_shielded_transaction(tx) {
                    if let Some(ref cf) = shielded_txs_cf {
                        let mut shielded_key = Vec::with_capacity(48);
                        shielded_key.extend_from_slice(&current.header.slot.to_be_bytes());
                        shielded_key.extend_from_slice(&(tx_index as u64).to_be_bytes());
                        shielded_key.extend_from_slice(&sig.0);
                        batch.put_cf(cf, &shielded_key, []);
                    }
                }
            }

            let account_entries =
                super::secondary_indexes::account_tx_index_entries_for_block(&current);
            let account_entry_count = account_entries.len();
            for (account, key) in account_entries {
                batch.put_cf(&account_txs_cf, &key, []);
                *counters.entry(account).or_default() += 1;
                indexed = indexed.saturating_add(1);
            }

            pending = pending
                .saturating_add(1)
                .saturating_add(current.transactions.len())
                .saturating_add(account_entry_count);
            if pending >= WRITE_BATCH_SIZE {
                self.db
                    .write(batch)
                    .map_err(|err| format!("Failed rebuilding history indexes: {}", err))?;
                batch = WriteBatch::default();
                pending = 0;
            }

            if current.header.slot == 0 {
                break;
            }
            if current.header.parent_hash == Hash::default() {
                return Err(format!(
                    "canonical parent chain ended at slot {} before genesis",
                    current.header.slot
                ));
            }
            current = self
                .get_block(&current.header.parent_hash)?
                .ok_or_else(|| {
                    "canonical parent block is missing below rebuilt chain".to_string()
                })?;
        }

        for (account, count) in counters {
            let mut counter_key = Vec::with_capacity(5 + 32);
            counter_key.extend_from_slice(b"atxc:");
            counter_key.extend_from_slice(&account.0);
            batch.put_cf(&stats_cf, &counter_key, count.to_le_bytes());
            pending += 1;

            if pending >= WRITE_BATCH_SIZE {
                self.db
                    .write(batch)
                    .map_err(|err| format!("Failed rebuilding account tx counters: {}", err))?;
                batch = WriteBatch::default();
                pending = 0;
            }
        }

        if pending > 0 {
            self.db
                .write(batch)
                .map_err(|err| format!("Failed finalizing history index rebuild: {}", err))?;
        }

        report.rebuilt_rows = indexed;
        report.after_hot_rows = Self::count_rows_in_db(&self.db, CF_ACCOUNT_TXS)?;
        report.after_counter_keys = self.count_stats_prefix(b"atxc:")?;
        Ok(report)
    }

    pub fn rebuild_account_txs_index_from_tx_index_with_report(
        &self,
        dry_run: bool,
    ) -> Result<AccountTxsRebuildReport, String> {
        const WRITE_BATCH_SIZE: usize = 10_000;

        let mut report = self.account_txs_rebuild_report_from_tx_index()?;
        report.dry_run = dry_run;
        if dry_run {
            return Ok(report);
        }
        if !report.source_complete() {
            return Err(format!(
                "Refusing account_txs tx-index rebuild: source incomplete (tx_by_slot_rows={}, missing_transactions={})",
                report.tx_by_slot_rows, report.missing_transactions
            ));
        }

        if report.existing_hot_rows > 0 || report.existing_cold_rows > 0 {
            self.ensure_account_txs_rows_rebuildable_from_tx_index_in_db(&self.db, CF_ACCOUNT_TXS)?;
            if let Some(ref cold) = self.cold_db {
                self.ensure_account_txs_rows_rebuildable_from_tx_index_in_db(
                    cold,
                    COLD_CF_ACCOUNT_TXS,
                )?;
            }
        }

        self.clear_snapshot_category("account_txs")?;

        let account_txs_cf = self
            .db
            .cf_handle(CF_ACCOUNT_TXS)
            .ok_or_else(|| "Account txs CF not found".to_string())?;
        let stats_cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let tx_by_slot_cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "tx_by_slot CF not found".to_string())?;
        let tx_to_slot_cf = self
            .db
            .cf_handle(CF_TX_TO_SLOT)
            .ok_or_else(|| "tx_to_slot CF not found".to_string())?;
        let shielded_txs_cf = self.db.cf_handle(CF_SHIELDED_TXS);

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = self
            .db
            .iterator_cf_opt(&tx_by_slot_cf, read_opts, rocksdb::IteratorMode::Start);

        let mut batch = WriteBatch::default();
        let mut pending = 0usize;
        let mut indexed = 0u64;
        let mut counters = std::collections::BTreeMap::<Pubkey, u64>::new();

        for item in iter {
            let (key, value) =
                item.map_err(|err| format!("Failed iterating tx_by_slot for rebuild: {}", err))?;
            let Some((slot, tx_index, tx_hash)) = parse_tx_by_slot_snapshot_row(&key, &value)?
            else {
                continue;
            };
            let tx_index_usize = usize::try_from(tx_index).map_err(|_| {
                format!(
                    "tx_by_slot sequence {} at slot {} does not fit this platform",
                    tx_index, slot
                )
            })?;
            let tx = self.get_transaction(&tx_hash)?.ok_or_else(|| {
                format!(
                    "tx_by_slot source became incomplete at slot {} tx {}",
                    slot,
                    tx_hash.to_hex()
                )
            })?;

            batch.put_cf(&tx_to_slot_cf, tx_hash.0, slot.to_be_bytes());
            if super::is_shielded_transaction(&tx) {
                if let Some(ref cf) = shielded_txs_cf {
                    let mut shielded_key = Vec::with_capacity(48);
                    shielded_key.extend_from_slice(&slot.to_be_bytes());
                    shielded_key.extend_from_slice(&tx_index.to_be_bytes());
                    shielded_key.extend_from_slice(&tx_hash.0);
                    batch.put_cf(cf, &shielded_key, []);
                }
            }

            let account_entries =
                super::secondary_indexes::account_tx_index_entries_for_transaction(
                    slot,
                    tx_index_usize,
                    &tx,
                );
            let account_entry_count = account_entries.len();
            for (account, key) in account_entries {
                batch.put_cf(&account_txs_cf, &key, []);
                *counters.entry(account).or_default() += 1;
                indexed = indexed.saturating_add(1);
            }

            pending = pending
                .saturating_add(2)
                .saturating_add(account_entry_count);
            if pending >= WRITE_BATCH_SIZE {
                self.db.write(batch).map_err(|err| {
                    format!("Failed rebuilding account_txs from tx-index: {}", err)
                })?;
                batch = WriteBatch::default();
                pending = 0;
            }
        }

        for (account, count) in counters {
            let mut counter_key = Vec::with_capacity(5 + 32);
            counter_key.extend_from_slice(b"atxc:");
            counter_key.extend_from_slice(&account.0);
            batch.put_cf(&stats_cf, &counter_key, count.to_le_bytes());
            pending += 1;

            if pending >= WRITE_BATCH_SIZE {
                self.db
                    .write(batch)
                    .map_err(|err| format!("Failed rebuilding account tx counters: {}", err))?;
                batch = WriteBatch::default();
                pending = 0;
            }
        }

        if pending > 0 {
            self.db.write(batch).map_err(|err| {
                format!("Failed finalizing tx-index account_txs rebuild: {}", err)
            })?;
        }

        report.rebuilt_rows = indexed;
        report.after_hot_rows = Self::count_rows_in_db(&self.db, CF_ACCOUNT_TXS)?;
        report.after_counter_keys = self.count_stats_prefix(b"atxc:")?;
        Ok(report)
    }

    /// Backfill the derived governed proposal tx->proposal-id index from
    /// canonical transaction history. This never changes account balances,
    /// proposal state, or state roots.
    pub fn backfill_governed_proposal_tx_index(
        &self,
        dry_run: bool,
    ) -> Result<GovernedProposalTxBackfillReport, String> {
        self.backfill_governed_proposal_tx_index_throttled(dry_run, 0, Duration::ZERO)
    }

    /// Backfill governed proposal tx links while optionally yielding between
    /// scan batches. The throttle is for live validator maintenance: it keeps
    /// derived-index repair off the consensus hot path without changing what
    /// gets linked.
    pub fn backfill_governed_proposal_tx_index_throttled(
        &self,
        dry_run: bool,
        throttle_every_rows: u64,
        throttle_sleep: Duration,
    ) -> Result<GovernedProposalTxBackfillReport, String> {
        const WRITE_BATCH_SIZE: usize = 10_000;

        let tx_by_slot_cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "tx_by_slot CF not found".to_string())?;
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = self
            .db
            .iterator_cf_opt(&tx_by_slot_cf, read_opts, rocksdb::IteratorMode::Start);

        let mut report = GovernedProposalTxBackfillReport {
            dry_run,
            ..GovernedProposalTxBackfillReport::default()
        };
        let mut batch = WriteBatch::default();
        let mut pending = 0usize;

        for item in iter {
            let (key, value) = item.map_err(|err| {
                format!("Failed iterating tx_by_slot for proposal backfill: {}", err)
            })?;
            let Some((_slot, _tx_index, tx_hash)) = parse_tx_by_slot_snapshot_row(&key, &value)?
            else {
                continue;
            };
            report.tx_by_slot_rows = report.tx_by_slot_rows.saturating_add(1);
            if throttle_every_rows > 0
                && !throttle_sleep.is_zero()
                && report.tx_by_slot_rows.is_multiple_of(throttle_every_rows)
            {
                std::thread::sleep(throttle_sleep);
            }

            let Some(tx) = self.get_transaction(&tx_hash)? else {
                report.missing_transactions = report.missing_transactions.saturating_add(1);
                continue;
            };

            let Some((proposer, source, recipient, amount)) =
                governed_transfer_proposal_instruction(&tx)
            else {
                continue;
            };
            report.proposal_txs = report.proposal_txs.saturating_add(1);

            if let Some(existing_id) = self.get_governed_proposal_id_for_tx(&tx_hash)? {
                let proposal = self.get_governed_proposal(existing_id)?.ok_or_else(|| {
                    format!(
                        "Governed proposal tx link {} points to missing proposal {}",
                        tx_hash.to_hex(),
                        existing_id
                    )
                })?;
                if proposal.source != source
                    || proposal.recipient != recipient
                    || proposal.amount != amount
                {
                    return Err(format!(
                        "Governed proposal tx link {} points to mismatched proposal {}",
                        tx_hash.to_hex(),
                        existing_id
                    ));
                }
                report.existing_links = report.existing_links.saturating_add(1);
                continue;
            }

            let Some(proposal) =
                self.find_governed_transfer_proposal(&source, &recipient, amount)?
            else {
                report.unresolved = report.unresolved.saturating_add(1);
                if report.first_unresolved_tx.is_none() {
                    report.first_unresolved_tx = Some(tx_hash.to_hex());
                }
                continue;
            };

            if !proposal.approvals.contains(&proposer) {
                report.unresolved = report.unresolved.saturating_add(1);
                if report.first_unresolved_tx.is_none() {
                    report.first_unresolved_tx = Some(tx_hash.to_hex());
                }
                continue;
            }

            report.linked = report.linked.saturating_add(1);
            if dry_run {
                continue;
            }

            self.batch_link_governed_proposal_tx(&mut batch, &tx_hash, proposal.id)?;
            pending = pending.saturating_add(1);
            if pending >= WRITE_BATCH_SIZE {
                self.db.write(batch).map_err(|err| {
                    format!(
                        "Failed writing governed proposal tx index backfill: {}",
                        err
                    )
                })?;
                batch = WriteBatch::default();
                pending = 0;
            }
        }

        if !dry_run && pending > 0 {
            self.db.write(batch).map_err(|err| {
                format!(
                    "Failed finalizing governed proposal tx index backfill: {}",
                    err
                )
            })?;
        }

        Ok(report)
    }

    /// Generic helper: read a page of (key, value) pairs from a column family.
    fn export_cf_page(
        &self,
        cf_name: &str,
        display_name: &str,
        offset: u64,
        limit: u64,
    ) -> Result<KvPage, String> {
        if limit == 0 {
            return Ok(KvPage {
                entries: Vec::new(),
                total: 0,
                next_cursor: None,
                has_more: false,
            });
        }

        let pages_to_advance = offset / limit;
        let intra_page_skip = (offset % limit) as usize;
        let mut cursor: Option<Vec<u8>> = None;
        let mut advanced = 0u64;

        while advanced < pages_to_advance {
            let page = self.export_cf_page_cursor_counted(
                cf_name,
                display_name,
                cursor.as_deref(),
                limit,
                None,
            )?;

            if !page.has_more && page.entries.is_empty() {
                return Ok(KvPage {
                    entries: Vec::new(),
                    total: page.total,
                    next_cursor: None,
                    has_more: false,
                });
            }

            cursor = page.next_cursor;
            advanced = advanced.saturating_add(1);

            if !page.has_more {
                break;
            }
        }

        let mut page = self.export_cf_page_cursor_counted(
            cf_name,
            display_name,
            cursor.as_deref(),
            limit.saturating_add(intra_page_skip as u64),
            None,
        )?;

        if intra_page_skip > 0 {
            if intra_page_skip >= page.entries.len() {
                page.entries.clear();
                page.has_more = false;
                page.next_cursor = None;
            } else {
                page.entries.drain(0..intra_page_skip);
                if page.entries.len() > limit as usize {
                    page.entries.truncate(limit as usize);
                    page.has_more = true;
                    page.next_cursor = page.entries.last().map(|(key, _)| key.clone());
                }
            }
        }

        if page.entries.len() > limit as usize {
            page.entries.truncate(limit as usize);
            page.has_more = true;
            page.next_cursor = page.entries.last().map(|(key, _)| key.clone());
        }

        Ok(page)
    }

    fn export_cf_page_cursor_counted(
        &self,
        cf_name: &str,
        display_name: &str,
        after_key: Option<&[u8]>,
        limit: u64,
        total_hint: Option<u64>,
    ) -> Result<KvPage, String> {
        self.export_cf_page_cursor_impl(cf_name, display_name, after_key, limit, total_hint, true)
    }

    fn export_cf_page_cursor_uncounted(
        &self,
        cf_name: &str,
        display_name: &str,
        after_key: Option<&[u8]>,
        limit: u64,
    ) -> Result<KvPage, String> {
        self.export_cf_page_cursor_impl(cf_name, display_name, after_key, limit, None, false)
    }

    fn export_cf_page_cursor_impl(
        &self,
        cf_name: &str,
        display_name: &str,
        after_key: Option<&[u8]>,
        limit: u64,
        total_hint: Option<u64>,
        include_total: bool,
    ) -> Result<KvPage, String> {
        let cf = self
            .db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{} CF not found", display_name))?;

        let total = if include_total {
            match total_hint {
                Some(value) => value,
                None => {
                    let mut count = 0u64;
                    let mut read_opts = rocksdb::ReadOptions::default();
                    read_opts.set_total_order_seek(true);
                    for item in
                        self.db
                            .iterator_cf_opt(&cf, read_opts, rocksdb::IteratorMode::Start)
                    {
                        item.map_err(|err| {
                            format!("Failed counting {} entries: {}", display_name, err)
                        })?;
                        count = count.saturating_add(1);
                    }
                    count
                }
            }
        } else {
            0
        };

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = if let Some(after) = after_key {
            self.db.iterator_cf_opt(
                &cf,
                read_opts,
                rocksdb::IteratorMode::From(after, rocksdb::Direction::Forward),
            )
        } else {
            self.db
                .iterator_cf_opt(&cf, read_opts, rocksdb::IteratorMode::Start)
        };

        let mut entries = Vec::with_capacity(limit.min(10_000) as usize);
        let mut has_more = false;

        for item in iter {
            let (key, value) =
                item.map_err(|err| format!("Failed iterating {}: {}", display_name, err))?;
            if let Some(after) = after_key {
                if key.as_ref() == after {
                    continue;
                }
            }

            entries.push((key.to_vec(), value.to_vec()));
            if entries.len() > limit as usize {
                has_more = true;
                entries.pop();
                break;
            }
        }

        let next_cursor = if has_more {
            entries.last().map(|(key, _)| key.clone())
        } else {
            None
        };

        Ok(KvPage {
            entries,
            total,
            next_cursor,
            has_more,
        })
    }

    /// Import a batch of accounts into the store (used by joining validators).
    /// Returns the number of accounts imported.
    pub fn import_accounts(&self, entries: &[(Vec<u8>, Vec<u8>)]) -> Result<usize, String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        let mut batch = WriteBatch::default();
        for (key, value) in entries {
            batch.put_cf(&cf, key, value);
        }
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to import accounts: {}", e))?;

        Ok(entries.len())
    }

    /// Import a batch of contract storage entries.
    pub fn import_contract_storage(&self, entries: &[(Vec<u8>, Vec<u8>)]) -> Result<usize, String> {
        let cf = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;

        let mut batch = WriteBatch::default();
        for (key, value) in entries {
            batch.put_cf(&cf, key, value);
        }
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to import contract storage: {}", e))?;

        Ok(entries.len())
    }

    /// Import a batch of programs (WASM bytecode).
    pub fn import_programs(&self, entries: &[(Vec<u8>, Vec<u8>)]) -> Result<usize, String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAMS)
            .ok_or_else(|| "Programs CF not found".to_string())?;

        let mut batch = WriteBatch::default();
        for (key, value) in entries {
            batch.put_cf(&cf, key, value);
        }
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to import programs: {}", e))?;

        Ok(entries.len())
    }

    fn public_history_cold_target_cf(category: &str) -> Option<&'static str> {
        match category {
            "blocks" => Some(COLD_CF_BLOCKS),
            "transactions" => Some(COLD_CF_TRANSACTIONS),
            "tx_to_slot" => Some(COLD_CF_TX_TO_SLOT),
            "account_txs" => Some(COLD_CF_ACCOUNT_TXS),
            "account_snapshots" => Some(COLD_CF_ACCOUNT_SNAPSHOTS),
            "events" => Some(COLD_CF_EVENTS),
            "token_transfers" => Some(COLD_CF_TOKEN_TRANSFERS),
            "program_calls" => Some(COLD_CF_PROGRAM_CALLS),
            _ => None,
        }
    }

    fn public_history_hot_cf(category: &str) -> Result<&'static str, String> {
        match category {
            "slots" => Ok(CF_SLOTS),
            "blocks" => Ok(CF_BLOCKS),
            "transactions" => Ok(CF_TRANSACTIONS),
            "tx_by_slot" => Ok(CF_TX_BY_SLOT),
            "tx_to_slot" => Ok(CF_TX_TO_SLOT),
            "tx_meta" => Ok(CF_TX_META),
            "account_txs" => Ok(CF_ACCOUNT_TXS),
            "events_by_slot" => Ok(CF_EVENTS_BY_SLOT),
            "events" => Ok(CF_EVENTS),
            "token_transfers" => Ok(CF_TOKEN_TRANSFERS),
            "program_calls" => Ok(CF_PROGRAM_CALLS),
            "evm_txs" => Ok(CF_EVM_TXS),
            "evm_receipts" => Ok(CF_EVM_RECEIPTS),
            "evm_logs_by_slot" => Ok(CF_EVM_LOGS_BY_SLOT),
            "shielded_txs" => Ok(CF_SHIELDED_TXS),
            "nft_activity" => Ok(CF_NFT_ACTIVITY),
            "market_activity" => Ok(CF_MARKET_ACTIVITY),
            "dex_trades_by_pair" => Ok(CF_DEX_TRADES_BY_PAIR),
            "dex_trades_by_taker" => Ok(CF_DEX_TRADES_BY_TAKER),
            "dex_trades_by_pair_taker" => Ok(CF_DEX_TRADES_BY_PAIR_TAKER),
            "account_snapshots" => Ok(CF_ACCOUNT_SNAPSHOTS),
            _ => Err(format!("Unsupported public-history category: {}", category)),
        }
    }

    fn canonical_public_history_import_value(
        category: &str,
        key: &[u8],
        value: &[u8],
    ) -> Result<Vec<u8>, String> {
        match category {
            "blocks" => canonical_public_history_block_import_value(key, value),
            "transactions" => canonical_transaction_snapshot_value_from_entry(key, value),
            "slots" => {
                if key.len() != 8 || value.len() != 32 {
                    return Err(format!(
                        "Invalid public slot row: key_len={} value_len={}",
                        key.len(),
                        value.len()
                    ));
                }
                Ok(value.to_vec())
            }
            "tx_by_slot" => {
                if key.len() != 16 || value.len() != 32 {
                    return Err(format!(
                        "Invalid tx_by_slot row: key_len={} value_len={}",
                        key.len(),
                        value.len()
                    ));
                }
                Ok(value.to_vec())
            }
            "tx_to_slot" => {
                if key.len() != 32 || value.len() != 8 {
                    return Err(format!(
                        "Invalid tx_to_slot row: key_len={} value_len={}",
                        key.len(),
                        value.len()
                    ));
                }
                Ok(value.to_vec())
            }
            _ => Ok(value.to_vec()),
        }
    }

    fn public_history_values_match(
        category: &str,
        key: &[u8],
        existing: &[u8],
        incoming: &[u8],
    ) -> Result<bool, String> {
        if category == "blocks" {
            return Ok(public_history_manifest_block_value(key, existing)?
                == public_history_manifest_block_value(key, incoming)?);
        }
        let existing = Self::canonical_public_history_import_value(category, key, existing)?;
        Ok(existing == incoming)
    }

    fn classify_public_history_existing_row(
        category: &str,
        key: &[u8],
        existing: Option<&[u8]>,
        incoming: &[u8],
    ) -> Result<PublicHistoryExistingRow, String> {
        let Some(existing) = existing else {
            return Ok(PublicHistoryExistingRow::Missing);
        };
        if Self::public_history_values_match(category, key, existing, incoming)? {
            return Ok(PublicHistoryExistingRow::Identical(
                Self::canonical_public_history_import_value(category, key, existing)?,
            ));
        }
        if category == "blocks" {
            if let Some(upgraded) =
                incomplete_public_history_block_upgrade(key, existing, incoming)?
            {
                return Ok(PublicHistoryExistingRow::UpgradeIncompleteBlock(upgraded));
            }
        }
        Ok(PublicHistoryExistingRow::Conflict)
    }

    /// Additively import public-history snapshot entries.
    ///
    /// Existing identical rows are skipped. A matching header-only block may
    /// receive its source-backed body; every other same-key mismatch is a
    /// conflict, and execute mode aborts on the first one. Cold-capable
    /// categories are written into the attached cold store when present.
    pub fn import_public_history_category_entries(
        &self,
        category: &str,
        entries: &[(Vec<u8>, Vec<u8>)],
        dry_run: bool,
    ) -> Result<PublicHistoryImportReport, String> {
        if !PUBLIC_HISTORY_SNAPSHOT_CATEGORIES.contains(&category) {
            return Err(format!("Unsupported public-history category: {}", category));
        }

        let hot_cf_name = Self::public_history_hot_cf(category)?;
        let cold_cf_name = Self::public_history_cold_target_cf(category);
        if cold_cf_name.is_some() && self.cold_db.is_none() && !dry_run {
            return Err(format!(
                "Refusing public-history import for {} without an attached cold store",
                category
            ));
        }

        let target_cold = cold_cf_name.is_some() && self.cold_db.is_some();
        let target_cf_name = if target_cold {
            cold_cf_name.expect("checked cold target")
        } else {
            hot_cf_name
        };
        let target_db = if target_cold {
            self.cold_db
                .as_ref()
                .ok_or_else(|| "Cold storage must be attached".to_string())?
                .as_ref()
        } else {
            self.db.as_ref()
        };
        let target_cf = target_db
            .cf_handle(target_cf_name)
            .ok_or_else(|| format!("Target CF {} not found", target_cf_name))?;

        let hot_cf = if target_cold {
            self.db.cf_handle(hot_cf_name)
        } else {
            None
        };

        let mut report = PublicHistoryImportReport {
            category: category.to_string(),
            target_cf: target_cf_name.to_string(),
            target_cold,
            ..PublicHistoryImportReport::default()
        };
        let mut batch = WriteBatch::default();
        let mut pending = 0usize;

        for (key, value) in entries {
            let canonical = Self::canonical_public_history_import_value(category, key, value)?;
            report.source_rows = report.source_rows.saturating_add(1);
            let row_bytes = (key.len() as u64).saturating_add(canonical.len() as u64);
            report.source_bytes = report.source_bytes.saturating_add(row_bytes);

            let hot_existing = if let Some(hot_cf) = hot_cf.as_ref() {
                self.db
                    .get_cf(hot_cf, key)
                    .map_err(|err| format!("Failed reading hot {}: {}", hot_cf_name, err))?
            } else {
                None
            };
            let target_existing = target_db
                .get_cf(&target_cf, key)
                .map_err(|err| format!("Failed reading {}: {}", target_cf_name, err))?;
            let hot_action = Self::classify_public_history_existing_row(
                category,
                key,
                hot_existing.as_deref(),
                &canonical,
            )?;
            let target_action = Self::classify_public_history_existing_row(
                category,
                key,
                target_existing.as_deref(),
                &canonical,
            )?;

            if matches!(&hot_action, PublicHistoryExistingRow::Conflict)
                || matches!(&target_action, PublicHistoryExistingRow::Conflict)
            {
                report.conflict_rows = report.conflict_rows.saturating_add(1);
                if !dry_run {
                    return Err(format!(
                        "Refusing public-history import: hot/cold {} key {} differs from source",
                        hot_cf_name,
                        hex::encode(key)
                    ));
                }
                continue;
            }

            let upgrades_incomplete = matches!(
                &hot_action,
                PublicHistoryExistingRow::UpgradeIncompleteBlock(_)
            ) || matches!(
                &target_action,
                PublicHistoryExistingRow::UpgradeIncompleteBlock(_)
            );
            if upgrades_incomplete {
                report.inserted_rows = report.inserted_rows.saturating_add(1);
                report.inserted_bytes = report.inserted_bytes.saturating_add(row_bytes);
                report.upgraded_incomplete_rows = report.upgraded_incomplete_rows.saturating_add(1);
                if !dry_run {
                    let upgraded = match (&hot_action, &target_action) {
                        (PublicHistoryExistingRow::Identical(value), _) => value,
                        (_, PublicHistoryExistingRow::Identical(value)) => value,
                        (PublicHistoryExistingRow::UpgradeIncompleteBlock(value), _) => value,
                        (_, PublicHistoryExistingRow::UpgradeIncompleteBlock(value)) => value,
                        _ => unreachable!("incomplete upgrade must provide a full block"),
                    };
                    target_db.put_cf(&target_cf, key, upgraded).map_err(|err| {
                        format!(
                            "Failed upgrading incomplete {} key {}: {}",
                            target_cf_name,
                            hex::encode(key),
                            err
                        )
                    })?;
                    target_db.flush_wal(true).map_err(|err| {
                        format!("Failed syncing {} upgrade WAL: {}", target_cf_name, err)
                    })?;
                    if let (Some(hot_cf), Some(_)) = (hot_cf.as_ref(), hot_existing.as_ref()) {
                        self.db.delete_cf(hot_cf, key).map_err(|err| {
                            format!(
                                "Failed retiring incomplete hot {} key {}: {}",
                                hot_cf_name,
                                hex::encode(key),
                                err
                            )
                        })?;
                        self.db.flush_wal(true).map_err(|err| {
                            format!("Failed syncing hot {} upgrade WAL: {}", hot_cf_name, err)
                        })?;
                    }
                }
                continue;
            }

            if matches!(&hot_action, PublicHistoryExistingRow::Identical(_))
                || matches!(&target_action, PublicHistoryExistingRow::Identical(_))
            {
                report.identical_rows = report.identical_rows.saturating_add(1);
                continue;
            }

            report.inserted_rows = report.inserted_rows.saturating_add(1);
            report.inserted_bytes = report.inserted_bytes.saturating_add(row_bytes);
            if !dry_run {
                batch.put_cf(&target_cf, key, canonical);
                pending += 1;
                if pending >= 10_000 {
                    target_db.write(std::mem::take(&mut batch)).map_err(|err| {
                        format!("Failed writing {} import batch: {}", target_cf_name, err)
                    })?;
                    pending = 0;
                }
            }
        }

        if !dry_run && pending > 0 {
            target_db.write(batch).map_err(|err| {
                format!("Failed writing {} import batch: {}", target_cf_name, err)
            })?;
        }
        if !dry_run && report.inserted_rows > 0 {
            target_db
                .flush_wal(true)
                .map_err(|err| format!("Failed syncing {} import WAL: {}", target_cf_name, err))?;
        }

        Ok(report)
    }

    /// Import a whitelisted snapshot category.
    pub fn import_snapshot_category(
        &self,
        category: &str,
        entries: &[(Vec<u8>, Vec<u8>)],
    ) -> Result<usize, String> {
        match category {
            "accounts" => return self.import_accounts(entries),
            "blocks" => return self.import_blocks_canonical(entries),
            "contract_storage" => return self.import_contract_storage(entries),
            "programs" => return self.import_programs(entries),
            _ => {}
        }

        let (cf_name, display_name) = Self::snapshot_category_cf(category)
            .ok_or_else(|| format!("Unsupported snapshot category: {}", category))?;
        let cf = self
            .db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{} CF not found", display_name))?;

        let mut batch = WriteBatch::default();
        for (key, value) in entries {
            batch.put_cf(&cf, key, value);
        }
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to import {}: {}", category, e))?;

        if category == "stats" {
            self.reload_metrics_from_stats()?;
        }

        Ok(entries.len())
    }

    fn import_blocks_canonical(&self, entries: &[(Vec<u8>, Vec<u8>)]) -> Result<usize, String> {
        let cf = self
            .db
            .cf_handle(CF_BLOCKS)
            .ok_or_else(|| "Blocks CF not found".to_string())?;

        let mut batch = WriteBatch::default();
        for (key, value) in entries {
            let canonical = canonical_block_snapshot_value(key, value)?;
            batch.put_cf(&cf, key, canonical);
        }
        self.db
            .write(batch)
            .map_err(|err| format!("Failed to import blocks: {}", err))?;

        Ok(entries.len())
    }

    /// Remove all entries from a whitelisted snapshot category before applying
    /// a verified full-category snapshot.
    pub fn clear_snapshot_category(&self, category: &str) -> Result<u64, String> {
        const DELETE_BATCH_SIZE: usize = 10_000;

        let (cf_name, display_name) = Self::snapshot_category_cf(category)
            .ok_or_else(|| format!("Unsupported snapshot category: {}", category))?;
        let cf = self
            .db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{} CF not found", display_name))?;

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = self
            .db
            .iterator_cf_opt(&cf, read_opts, rocksdb::IteratorMode::Start);
        let mut batch = WriteBatch::default();
        let mut batch_count = 0usize;
        let mut deleted = 0u64;
        for item in iter {
            let (key, _) = item.map_err(|e| format!("{} iterator error: {}", display_name, e))?;
            batch.delete_cf(&cf, key);
            batch_count += 1;
            if batch_count >= DELETE_BATCH_SIZE {
                self.db
                    .write(batch)
                    .map_err(|e| format!("Failed to clear {}: {}", category, e))?;
                deleted = deleted.saturating_add(batch_count as u64);
                batch = WriteBatch::default();
                batch_count = 0;
            }
        }

        if batch_count > 0 {
            self.db
                .write(batch)
                .map_err(|e| format!("Failed to clear {}: {}", category, e))?;
            deleted = deleted.saturating_add(batch_count as u64);
        }

        if category == "account_txs" {
            deleted = deleted.saturating_add(self.clear_account_tx_counters()?);
        }

        Ok(deleted)
    }
}

#[cfg(test)]
mod manifest_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn shared_canonical_ledger_walk_matches_paged_category_digests() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let tx_a = crate::transaction::Transaction::new(crate::transaction::Message::new(
            Vec::new(),
            Hash::hash(b"manifest-shared-walk-a"),
        ));
        let tx_b = crate::transaction::Transaction::new(crate::transaction::Message::new(
            Vec::new(),
            Hash::hash(b"manifest-shared-walk-b"),
        ));
        let block = Block::new(
            10,
            Hash::default(),
            Hash::hash(b"manifest-shared-state"),
            [0x44; 32],
            vec![tx_a.clone(), tx_b.clone()],
        );
        state.put_block_atomic(&block, Some(10), Some(10)).unwrap();
        state.put_tx_meta(&tx_a.signature(), 123).unwrap();

        let stale_hash = Hash::hash(b"manifest-stale-index");
        state.index_tx_by_slot(10, &stale_hash).unwrap();

        let indexed_only = crate::transaction::Transaction::new(crate::transaction::Message::new(
            Vec::new(),
            Hash::hash(b"manifest-indexed-only"),
        ));
        state.put_transaction(&indexed_only).unwrap();
        state
            .index_tx_by_slot(11, &indexed_only.signature())
            .unwrap();

        let manifest = state
            .compute_public_history_manifest(CANONICAL_LEDGER_MANIFEST_CATEGORIES, 1)
            .unwrap();
        for actual in manifest.categories {
            let expected = state
                .compute_public_history_category_digest_paged(&actual.category, 1)
                .unwrap();
            assert_eq!(actual, expected, "{} digest changed", expected.category);
        }
    }
}
