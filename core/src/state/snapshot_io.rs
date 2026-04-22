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
    /// Get a reference to the underlying DB Arc for direct access when needed.
    pub fn db_ref(&self) -> &Arc<DB> {
        &self.db
    }

    // ── Checkpoint creation (RocksDB native hardlink snapshot) ────────────

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
        use rocksdb::checkpoint::Checkpoint;

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
        for _ in self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::Start)
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
                    for _ in self
                        .db
                        .iterator_cf(&cf, rocksdb::IteratorMode::Start)
                        .flatten()
                    {
                        count = count.saturating_add(1);
                    }
                    count
                }
            }
        } else {
            0
        };

        let iter = if let Some(after) = after_key {
            self.db.iterator_cf(
                &cf,
                rocksdb::IteratorMode::From(after, rocksdb::Direction::Forward),
            )
        } else {
            self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start)
        };

        let mut entries = Vec::with_capacity(limit.min(10_000) as usize);
        let mut has_more = false;

        for (key, value) in iter.flatten() {
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
}
