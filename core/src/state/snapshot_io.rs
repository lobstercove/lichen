use serde::{Deserialize, Serialize};

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

impl StateStore {
    pub(crate) fn snapshot_category_cf(category: &str) -> Option<(&'static str, &'static str)> {
        match category {
            "accounts" => Some((CF_ACCOUNTS, "Accounts")),
            "contract_storage" => Some((CF_CONTRACT_STORAGE, "Contract storage")),
            "programs" => Some((CF_PROGRAMS, "Programs")),
            "symbol_registry" => Some((CF_SYMBOL_REGISTRY, "Symbol registry")),
            "symbol_by_program" => Some((CF_SYMBOL_BY_PROGRAM, "Symbol reverse registry")),
            "evm_map" => Some((CF_EVM_MAP, "EVM address map")),
            "evm_accounts" => Some((CF_EVM_ACCOUNTS, "EVM accounts")),
            "evm_storage" => Some((CF_EVM_STORAGE, "EVM storage")),
            "nft_by_owner" => Some((CF_NFT_BY_OWNER, "NFT owner index")),
            "nft_by_collection" => Some((CF_NFT_BY_COLLECTION, "NFT collection index")),
            "token_balances" => Some((CF_TOKEN_BALANCES, "Token balances")),
            "holder_tokens" => Some((CF_HOLDER_TOKENS, "Holder token index")),
            "solana_token_accounts" => {
                Some((CF_SOLANA_TOKEN_ACCOUNTS, "Solana token-account bindings"))
            }
            "solana_holder_token_accounts" => Some((
                CF_SOLANA_HOLDER_TOKEN_ACCOUNTS,
                "Solana holder token-account index",
            )),
            "dex_orders_by_pair" => Some((CF_DEX_ORDERS_BY_PAIR, "DEX orders-by-pair index")),
            "dex_trades_by_pair" => Some((CF_DEX_TRADES_BY_PAIR, "DEX trades-by-pair index")),
            "dex_trades_by_taker" => Some((CF_DEX_TRADES_BY_TAKER, "DEX trades-by-taker index")),
            "dex_trades_by_pair_taker" => Some((
                CF_DEX_TRADES_BY_PAIR_TAKER,
                "DEX trades-by-pair-taker index",
            )),
            "dex_orderbook_levels" => Some((CF_DEX_ORDERBOOK_LEVELS, "DEX orderbook levels")),
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
        let state_root = checkpoint_store.compute_state_root_cached();
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
        Self::open(checkpoint_dir)
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
        let checkpoints = Self::list_checkpoints(data_dir);
        if checkpoints.len() <= keep_count {
            return Ok(0);
        }
        let to_remove = checkpoints.len() - keep_count;
        let mut removed = 0;
        for (_, path) in checkpoints.iter().take(to_remove) {
            if std::fs::remove_dir_all(path).is_ok() {
                removed += 1;
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
        let (cf_name, display_name) = Self::snapshot_category_cf(category)
            .ok_or_else(|| format!("Unsupported snapshot category: {}", category))?;
        self.export_cf_page_cursor_uncounted(cf_name, display_name, after_key, limit)
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
