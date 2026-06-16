use serde::{Deserialize, Serialize};

use crate::block::Block;
use crate::codec::{append_legacy_bincode, deserialize_legacy_bincode};

use super::*;

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
    let mut block = decode_snapshot_block_value(value)?;
    let block_hash = block.hash();
    if key != block_hash.0 {
        return Err(format!(
            "Block snapshot key/hash mismatch: key={} block_hash={}",
            hex::encode(key),
            block_hash.to_hex()
        ));
    }
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

    let mut canonical = Vec::with_capacity(value.len().max(1));
    canonical.push(0xBC);
    append_legacy_bincode(&mut canonical, &block, "block").map_err(|err| {
        format!(
            "Failed to serialize canonical block snapshot value: {}",
            err
        )
    })?;
    Ok(canonical)
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

    /// Create a point-in-time checkpoint without snapshot metadata.
    ///
    /// This is used by short-lived staging databases on hot paths. Persistent
    /// sync checkpoints should use `create_checkpoint`, which writes metadata
    /// and records the already-committed checkpoint state root.
    pub fn create_raw_checkpoint(&self, checkpoint_dir: &str) -> Result<(), String> {
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
            .map_err(|e| format!("Failed to create checkpoint: {}", e))
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
        Self::open_read_only_with_cache_mb(checkpoint_dir, None)
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
            return self.export_blocks_cursor_canonical(after_key, limit);
        }
        if category == "tx_by_slot" {
            return self.export_tx_by_slot_from_blocks_cursor(after_key, limit);
        }
        if category == "stats" {
            return self.export_stats_cursor_for_snapshot(after_key, limit);
        }

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

    fn export_blocks_cursor_canonical(
        &self,
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

        let cf = self
            .db
            .cf_handle(CF_BLOCKS)
            .ok_or_else(|| "Blocks CF not found".to_string())?;

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
            let (key, value) = item.map_err(|err| format!("Failed iterating Blocks: {}", err))?;
            if let Some(after) = after_key {
                if key.as_ref() == after {
                    continue;
                }
            }

            let canonical = canonical_block_snapshot_value(&key, &value)?;
            entries.push((key.to_vec(), canonical));
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

    fn export_tx_by_slot_from_blocks_cursor(
        &self,
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

        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        let (start_slot, after_index) = match after_key {
            Some(key) if key.len() == 16 => {
                let mut slot_bytes = [0u8; 8];
                slot_bytes.copy_from_slice(&key[..8]);
                let mut index_bytes = [0u8; 8];
                index_bytes.copy_from_slice(&key[8..16]);
                (
                    u64::from_be_bytes(slot_bytes),
                    Some(u64::from_be_bytes(index_bytes)),
                )
            }
            Some(key) if key.len() >= 8 => {
                let mut slot_bytes = [0u8; 8];
                slot_bytes.copy_from_slice(&key[..8]);
                (u64::from_be_bytes(slot_bytes), None)
            }
            _ => (0, None),
        };

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
        let limit = limit as usize;

        'slots: for item in iter {
            let (slot_key, _) = item
                .map_err(|err| format!("Failed iterating Slots for tx_by_slot export: {}", err))?;
            if slot_key.len() != 8 {
                continue;
            }

            let mut slot_bytes = [0u8; 8];
            slot_bytes.copy_from_slice(&slot_key);
            let slot = u64::from_be_bytes(slot_bytes);
            if slot < start_slot {
                continue;
            }

            let first_tx_index = if Some(slot) == Some(start_slot) {
                after_index
                    .map(|index| index.saturating_add(1))
                    .unwrap_or(0)
            } else {
                0
            };

            let Some(block) = self.get_block_by_slot(slot)? else {
                continue;
            };

            for (tx_index, tx) in block.transactions.iter().enumerate() {
                let tx_index = tx_index as u64;
                if tx_index < first_tx_index {
                    continue;
                }

                let mut key = Vec::with_capacity(16);
                key.extend_from_slice(&slot.to_be_bytes());
                key.extend_from_slice(&tx_index.to_be_bytes());
                entries.push((key, tx.signature().0.to_vec()));

                if entries.len() > limit {
                    entries.pop();
                    has_more = true;
                    break 'slots;
                }
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
        let mut keys = Vec::new();
        for item in iter {
            let (key, _) = item.map_err(|e| format!("{} iterator error: {}", display_name, e))?;
            keys.push(key.to_vec());
        }

        let mut deleted = 0u64;
        for chunk in keys.chunks(DELETE_BATCH_SIZE) {
            let mut batch = WriteBatch::default();
            for key in chunk {
                batch.delete_cf(&cf, key);
            }
            self.db
                .write(batch)
                .map_err(|e| format!("Failed to clear {}: {}", category, e))?;
            deleted = deleted.saturating_add(chunk.len() as u64);
        }

        Ok(deleted)
    }
}
