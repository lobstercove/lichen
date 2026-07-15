use std::path::Path;

use crate::block::Block;
use crate::codec::deserialize_legacy_bincode;

use super::*;

const PUBLIC_HISTORY_WRITE_BATCH_SIZE: usize = 10_000;
const COLD_BLOCK_MIGRATION_BATCH_SIZE: u64 = 1_000;
pub const COLD_BLOCK_MIGRATION_COMPACTION_BATCH_SIZE: u64 = 10_000;

fn is_public_history_merge_row(cf_name: &str, key: &[u8], value: &[u8]) -> bool {
    match cf_name {
        // CF_SLOTS also stores live cursor metadata such as `last_slot` and
        // `confirmed_slot`. Public-history merge may import canonical
        // slot->block hash rows, but it must never downgrade those cursors
        // from an older archive or backup source.
        CF_SLOTS => key.len() == 8 && value.len() == 32,
        _ => true,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublicHistoryMergeCfReport {
    pub source_cf: &'static str,
    pub source_cold: bool,
    pub target_cf: &'static str,
    pub target_cold: bool,
    pub source_rows: u64,
    pub inserted_rows: u64,
    pub identical_rows: u64,
    pub conflict_rows: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublicHistoryMergeReport {
    pub dry_run: bool,
    pub used_cold_store: bool,
    pub cleared_account_tx_counters: u64,
    pub source_rows: u64,
    pub inserted_rows: u64,
    pub identical_rows: u64,
    pub conflict_rows: u64,
    pub cf_reports: Vec<PublicHistoryMergeCfReport>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColdMigrationAuditRows {
    pub hot_rows: u64,
    pub hot_bytes: u64,
    pub identical_cold_rows: u64,
    pub missing_cold_rows: u64,
    pub conflicting_cold_rows: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColdMigrationAuditReport {
    pub cutoff_slot: u64,
    pub scanned_slots: u64,
    pub decode_errors: u64,
    pub hash_mismatch_rows: u64,
    pub missing_slot_cursors: u64,
    pub conflicting_slot_cursors: u64,
    pub missing_transaction_rows: u64,
    pub mismatched_transaction_rows: u64,
    pub missing_tx_to_slot_rows: u64,
    pub invalid_tx_to_slot_rows: u64,
    pub blocks: ColdMigrationAuditRows,
    pub transactions: ColdMigrationAuditRows,
    pub tx_to_slot: ColdMigrationAuditRows,
}

impl ColdMigrationAuditReport {
    pub fn integrity_errors(&self) -> u64 {
        self.decode_errors
            .saturating_add(self.hash_mismatch_rows)
            .saturating_add(self.missing_slot_cursors)
            .saturating_add(self.conflicting_slot_cursors)
            .saturating_add(self.missing_transaction_rows)
            .saturating_add(self.mismatched_transaction_rows)
            .saturating_add(self.missing_tx_to_slot_rows)
            .saturating_add(self.invalid_tx_to_slot_rows)
    }

    pub fn conflict_rows(&self) -> u64 {
        self.integrity_errors().saturating_add(
            self.blocks
                .conflicting_cold_rows
                .saturating_add(self.transactions.conflicting_cold_rows)
                .saturating_add(self.tx_to_slot.conflicting_cold_rows),
        )
    }
}

struct PublicHistoryMergeCfSource<'a> {
    db: &'a DB,
    source_cf_name: &'static str,
    public_cf_name: &'static str,
    source_cold: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicHistoryMergeMode {
    Full,
    IndexesOnly,
}

impl PublicHistoryMergeReport {
    pub fn has_conflicts(&self) -> bool {
        self.conflict_rows > 0
    }
}

impl StateStore {
    fn lexicographic_successor(key: &[u8]) -> Option<Vec<u8>> {
        let mut successor = key.to_vec();
        for index in (0..successor.len()).rev() {
            if successor[index] != u8::MAX {
                successor[index] += 1;
                successor.truncate(index + 1);
                return Some(successor);
            }
        }
        None
    }

    fn decode_cold_migration_block(value: &[u8], context: &str) -> Result<Block, String> {
        if value.first() == Some(&0xBC) {
            deserialize_legacy_bincode(&value[1..], context)
                .map_err(|err| format!("Failed decoding {context}: {err}"))
        } else {
            serde_json::from_slice(value).map_err(|err| format!("Failed decoding {context}: {err}"))
        }
    }

    fn decode_cold_migration_transaction(
        value: &[u8],
        context: &str,
    ) -> Result<Transaction, String> {
        if value.first() == Some(&0xBC) {
            deserialize_legacy_bincode(&value[1..], context)
                .map_err(|err| format!("Failed decoding {context}: {err}"))
        } else {
            serde_json::from_slice(value).map_err(|err| format!("Failed decoding {context}: {err}"))
        }
    }

    fn cold_migration_transaction_matches(
        value: &[u8],
        expected: &Transaction,
        context: &str,
    ) -> Result<bool, String> {
        let decoded = Self::decode_cold_migration_transaction(value, context)?;
        let expected_hash = expected.hash();
        let decoded_hash = decoded.hash();
        Ok(decoded_hash == expected_hash && decoded.signature() == expected.signature())
    }

    fn validate_cold_migration_transaction(
        value: &[u8],
        expected: &Transaction,
        context: &str,
    ) -> Result<(), String> {
        if !Self::cold_migration_transaction_matches(value, expected, context)? {
            return Err(format!(
                "{context} does not match block transaction {}",
                expected.hash().to_hex()
            ));
        }
        Ok(())
    }

    fn validate_cold_migration_tx_slot(
        value: &[u8],
        expected_slot: u64,
        context: &str,
    ) -> Result<(), String> {
        if value != expected_slot.to_be_bytes() {
            return Err(format!(
                "{context} does not match block slot {expected_slot}: value={}",
                hex::encode(value)
            ));
        }
        Ok(())
    }

    fn audit_cold_row(
        cold: &DB,
        cold_cf: &impl rocksdb::AsColumnFamilyRef,
        cold_name: &str,
        key: &[u8],
        value: &[u8],
        rows: &mut ColdMigrationAuditRows,
    ) -> Result<(), String> {
        rows.hot_rows = rows.hot_rows.saturating_add(1);
        rows.hot_bytes = rows.hot_bytes.saturating_add(value.len() as u64);
        match cold
            .get_cf(cold_cf, key)
            .map_err(|e| format!("Cold read error ({}): {}", cold_name, e))?
        {
            Some(existing) if existing.as_slice() == value => {
                rows.identical_cold_rows = rows.identical_cold_rows.saturating_add(1);
            }
            Some(_) => {
                rows.conflicting_cold_rows = rows.conflicting_cold_rows.saturating_add(1);
            }
            None => {
                rows.missing_cold_rows = rows.missing_cold_rows.saturating_add(1);
            }
        }
        Ok(())
    }

    fn copy_cold_row_checked(
        cold: &DB,
        cold_cf: &impl rocksdb::AsColumnFamilyRef,
        cold_name: &str,
        key: &[u8],
        value: &[u8],
    ) -> Result<(), String> {
        match cold
            .get_cf(cold_cf, key)
            .map_err(|e| format!("Cold read error ({}): {}", cold_name, e))?
        {
            Some(existing) if existing.as_slice() == value => Ok(()),
            Some(_) => Err(format!(
                "Refusing cold migration: {} key {} conflicts with the hot value",
                cold_name,
                hex::encode(key)
            )),
            None => cold
                .put_cf(cold_cf, key, value)
                .map_err(|e| format!("Cold write error ({}): {}", cold_name, e)),
        }
    }

    /// Open (or create) the cold archival DB at `cold_path` and attach it to
    /// this store. Once attached, `get_block` and `get_transaction` will
    /// fall through to the cold DB if the key is missing from hot storage.
    pub fn open_cold_store<P: AsRef<Path>>(&mut self, cold_path: P) -> Result<(), String> {
        self.cold_db = Some(Arc::new(storage_bootstrap::open_cold_db(
            cold_path.as_ref(),
        )?));
        tracing::info!(
            "🗄️  Cold storage opened at {}",
            cold_path.as_ref().display()
        );
        Ok(())
    }

    /// Attach an existing cold archival DB read-only. This is used by
    /// offline repair/inspection tools when an archive backup is the source.
    pub fn open_cold_store_read_only<P: AsRef<Path>>(
        &mut self,
        cold_path: P,
    ) -> Result<(), String> {
        self.cold_db = Some(Arc::new(storage_bootstrap::open_cold_db_read_only(
            cold_path.as_ref(),
        )?));
        tracing::info!(
            "🗄️  Cold storage opened read-only at {}",
            cold_path.as_ref().display()
        );
        Ok(())
    }

    /// Audit every hot row that `migrate_to_cold` would move without mutating
    /// either database. Operators use this before stopped-node maintenance so
    /// a conflicting archive copy is detected before any hot deletion begins.
    pub fn audit_cold_migration(
        &self,
        cutoff_slot: u64,
    ) -> Result<ColdMigrationAuditReport, String> {
        let cold = self
            .cold_db
            .as_ref()
            .ok_or_else(|| "Cold storage not attached".to_string())?;
        let hot_blocks_cf = self
            .db
            .cf_handle(CF_BLOCKS)
            .ok_or_else(|| "Blocks CF not found".to_string())?;
        let hot_slots_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let hot_txs_cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;
        let hot_tx_to_slot_cf = self
            .db
            .cf_handle(CF_TX_TO_SLOT)
            .ok_or_else(|| "tx_to_slot CF not found".to_string())?;
        let cold_blocks_cf = cold
            .cf_handle(COLD_CF_BLOCKS)
            .ok_or_else(|| "Cold blocks CF not found".to_string())?;
        let cold_txs_cf = cold
            .cf_handle(COLD_CF_TRANSACTIONS)
            .ok_or_else(|| "Cold transactions CF not found".to_string())?;
        let cold_tx_to_slot_cf = cold
            .cf_handle(COLD_CF_TX_TO_SLOT)
            .ok_or_else(|| "Cold tx_to_slot CF not found".to_string())?;

        let mut report = ColdMigrationAuditReport {
            cutoff_slot,
            ..ColdMigrationAuditReport::default()
        };
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = self
            .db
            .iterator_cf_opt(&hot_blocks_cf, read_opts, rocksdb::IteratorMode::Start);
        for item in iter {
            let (block_hash, block_data) =
                item.map_err(|e| format!("Failed iterating raw hot blocks: {e}"))?;
            let block = match Self::decode_cold_migration_block(
                &block_data,
                "hot block for cold migration audit",
            ) {
                Ok(block) => block,
                Err(_) => {
                    report.decode_errors = report.decode_errors.saturating_add(1);
                    continue;
                }
            };
            if block.header.slot >= cutoff_slot {
                continue;
            }
            report.scanned_slots = report.scanned_slots.saturating_add(1);

            let expected_hash = block.hash();
            if block_hash.as_ref() != expected_hash.0 {
                report.hash_mismatch_rows = report.hash_mismatch_rows.saturating_add(1);
                continue;
            }
            let slot_key = block.header.slot.to_be_bytes();
            match self
                .db
                .get_cf(&hot_slots_cf, slot_key)
                .map_err(|e| format!("Failed reading hot slot cursor: {e}"))?
            {
                Some(cursor) if cursor.as_slice() == expected_hash.0 => {}
                Some(_) => {
                    report.conflicting_slot_cursors =
                        report.conflicting_slot_cursors.saturating_add(1);
                    continue;
                }
                None => {
                    report.missing_slot_cursors = report.missing_slot_cursors.saturating_add(1);
                    continue;
                }
            }

            Self::audit_cold_row(
                cold,
                &cold_blocks_cf,
                COLD_CF_BLOCKS,
                &block_hash,
                &block_data,
                &mut report.blocks,
            )?;

            for tx in &block.transactions {
                let signature = tx.signature();
                let hot_tx_data = self
                    .db
                    .get_cf(&hot_txs_cf, signature.0)
                    .map_err(|e| format!("Failed reading hot transaction: {e}"))?;
                let cold_tx_data = cold
                    .get_cf(&cold_txs_cf, signature.0)
                    .map_err(|e| format!("Failed reading cold transaction: {e}"))?;
                if let Some(tx_data) = hot_tx_data.as_deref() {
                    Self::audit_cold_row(
                        cold,
                        &cold_txs_cf,
                        COLD_CF_TRANSACTIONS,
                        &signature.0,
                        tx_data,
                        &mut report.transactions,
                    )?;
                    match Self::cold_migration_transaction_matches(
                        tx_data,
                        tx,
                        "hot transaction for cold migration audit",
                    ) {
                        Ok(true) => {}
                        Ok(false) => {
                            report.mismatched_transaction_rows =
                                report.mismatched_transaction_rows.saturating_add(1);
                        }
                        Err(_) => {
                            report.decode_errors = report.decode_errors.saturating_add(1);
                        }
                    }
                } else if let Some(tx_data) = cold_tx_data.as_deref() {
                    match Self::cold_migration_transaction_matches(
                        tx_data,
                        tx,
                        "cold-only transaction for cold migration audit",
                    ) {
                        Ok(true) => {}
                        Ok(false) => {
                            report.mismatched_transaction_rows =
                                report.mismatched_transaction_rows.saturating_add(1);
                        }
                        Err(_) => {
                            report.decode_errors = report.decode_errors.saturating_add(1);
                        }
                    }
                } else {
                    report.missing_transaction_rows =
                        report.missing_transaction_rows.saturating_add(1);
                }

                let hot_slot_data = self
                    .db
                    .get_cf(&hot_tx_to_slot_cf, signature.0)
                    .map_err(|e| format!("Failed reading hot tx_to_slot: {e}"))?;
                let cold_slot_data = cold
                    .get_cf(&cold_tx_to_slot_cf, signature.0)
                    .map_err(|e| format!("Failed reading cold tx_to_slot: {e}"))?;
                if let Some(slot_data) = hot_slot_data.as_deref() {
                    Self::audit_cold_row(
                        cold,
                        &cold_tx_to_slot_cf,
                        COLD_CF_TX_TO_SLOT,
                        &signature.0,
                        slot_data,
                        &mut report.tx_to_slot,
                    )?;
                    if Self::validate_cold_migration_tx_slot(
                        slot_data,
                        block.header.slot,
                        "hot tx_to_slot for cold migration audit",
                    )
                    .is_err()
                    {
                        report.invalid_tx_to_slot_rows =
                            report.invalid_tx_to_slot_rows.saturating_add(1);
                    }
                } else if let Some(slot_data) = cold_slot_data.as_deref() {
                    if Self::validate_cold_migration_tx_slot(
                        slot_data,
                        block.header.slot,
                        "cold-only tx_to_slot for cold migration audit",
                    )
                    .is_err()
                    {
                        report.invalid_tx_to_slot_rows =
                            report.invalid_tx_to_slot_rows.saturating_add(1);
                    }
                } else {
                    report.missing_tx_to_slot_rows =
                        report.missing_tx_to_slot_rows.saturating_add(1);
                }
            }
        }
        Ok(report)
    }

    /// Migrate old blocks and transactions from the hot DB to the cold DB.
    ///
    /// Moves all blocks with slot < `cutoff_slot` and their associated
    /// transactions. Data is written to cold first, then deleted from hot
    /// in a single atomic batch to avoid data loss.
    ///
    /// Returns the number of blocks migrated.
    pub fn migrate_to_cold(&self, cutoff_slot: u64) -> Result<u64, String> {
        let cold = self
            .cold_db
            .as_ref()
            .ok_or_else(|| "Cold storage not attached".to_string())?;

        let hot_blocks_cf = self
            .db
            .cf_handle(CF_BLOCKS)
            .ok_or_else(|| "Blocks CF not found".to_string())?;
        let hot_slots_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let hot_txs_cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;
        let hot_tx_to_slot_cf = self
            .db
            .cf_handle(CF_TX_TO_SLOT)
            .ok_or_else(|| "tx_to_slot CF not found".to_string())?;

        let cold_blocks_cf = cold
            .cf_handle(COLD_CF_BLOCKS)
            .ok_or_else(|| "Cold blocks CF not found".to_string())?;
        let cold_txs_cf = cold
            .cf_handle(COLD_CF_TRANSACTIONS)
            .ok_or_else(|| "Cold transactions CF not found".to_string())?;
        let cold_tx_to_slot_cf = cold
            .cf_handle(COLD_CF_TX_TO_SLOT)
            .ok_or_else(|| "Cold tx_to_slot CF not found".to_string())?;

        let mut migrated: u64 = 0;
        let mut pending_migrated: u64 = 0;
        let mut hot_delete_batch = WriteBatch::default();

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = self.db.iterator_cf_opt(
            &hot_slots_cf,
            read_opts,
            rocksdb::IteratorMode::From(&0u64.to_be_bytes(), Direction::Forward),
        );

        for item in iter {
            let (slot_key, block_hash) =
                item.map_err(|e| format!("Failed iterating hot slots: {e}"))?;
            if slot_key.len() != 8 {
                continue;
            }
            let slot = u64::from_be_bytes(slot_key[..8].try_into().unwrap());
            if slot >= cutoff_slot {
                break;
            }

            if block_hash.len() != 32 {
                return Err(format!(
                    "Refusing cold migration: slot {slot} has an invalid {}-byte cursor",
                    block_hash.len()
                ));
            }
            let Some(block_data) = self
                .db
                .get_cf(&hot_blocks_cf, &block_hash)
                .map_err(|e| format!("Failed reading hot block: {e}"))?
            else {
                continue;
            };
            let block =
                Self::decode_cold_migration_block(&block_data, "hot block for cold migration")?;
            let expected_hash = block.hash();
            if block.header.slot != slot || block_hash.as_ref() != expected_hash.0 {
                return Err(format!(
                    "Refusing cold migration: slot {slot} cursor {} resolves to block slot {} hash {}",
                    hex::encode(&block_hash),
                    block.header.slot,
                    expected_hash.to_hex()
                ));
            }

            Self::copy_cold_row_checked(
                cold,
                &cold_blocks_cf,
                COLD_CF_BLOCKS,
                &block_hash,
                &block_data,
            )?;

            for tx in &block.transactions {
                let sig = tx.signature();
                let hot_tx_data = self
                    .db
                    .get_cf(&hot_txs_cf, sig.0)
                    .map_err(|e| format!("Failed reading hot transaction: {e}"))?;
                if let Some(tx_data) = hot_tx_data.as_deref() {
                    Self::validate_cold_migration_transaction(
                        tx_data,
                        tx,
                        "hot transaction for cold migration",
                    )?;
                    Self::copy_cold_row_checked(
                        cold,
                        &cold_txs_cf,
                        COLD_CF_TRANSACTIONS,
                        &sig.0,
                        tx_data,
                    )?;
                    hot_delete_batch.delete_cf(&hot_txs_cf, sig.0);
                } else {
                    let cold_tx_data = cold
                        .get_cf(&cold_txs_cf, sig.0)
                        .map_err(|e| format!("Failed reading cold transaction: {e}"))?
                        .ok_or_else(|| {
                            format!(
                                "Refusing cold migration: block slot {} transaction {} is missing from hot and cold storage",
                                block.header.slot,
                                sig.to_hex()
                            )
                        })?;
                    Self::validate_cold_migration_transaction(
                        &cold_tx_data,
                        tx,
                        "cold-only transaction for cold migration",
                    )?;
                }

                let hot_slot_data = self
                    .db
                    .get_cf(&hot_tx_to_slot_cf, sig.0)
                    .map_err(|e| format!("Failed reading hot tx_to_slot: {e}"))?;
                if let Some(slot_data) = hot_slot_data.as_deref() {
                    Self::validate_cold_migration_tx_slot(
                        slot_data,
                        block.header.slot,
                        "hot tx_to_slot for cold migration",
                    )?;
                    Self::copy_cold_row_checked(
                        cold,
                        &cold_tx_to_slot_cf,
                        COLD_CF_TX_TO_SLOT,
                        &sig.0,
                        slot_data,
                    )?;
                    hot_delete_batch.delete_cf(&hot_tx_to_slot_cf, sig.0);
                } else {
                    let cold_slot_data = cold
                        .get_cf(&cold_tx_to_slot_cf, sig.0)
                        .map_err(|e| format!("Failed reading cold tx_to_slot: {e}"))?
                        .ok_or_else(|| {
                            format!(
                                "Refusing cold migration: block slot {} transaction {} has no hot or cold tx_to_slot row",
                                block.header.slot,
                                sig.to_hex()
                            )
                        })?;
                    Self::validate_cold_migration_tx_slot(
                        &cold_slot_data,
                        block.header.slot,
                        "cold-only tx_to_slot for cold migration",
                    )?;
                }
            }

            hot_delete_batch.delete_cf(&hot_blocks_cf, &block_hash);
            migrated += 1;
            pending_migrated += 1;

            if pending_migrated >= COLD_BLOCK_MIGRATION_BATCH_SIZE {
                cold.flush_wal(true).map_err(|e| {
                    format!("Failed to sync cold WAL before hot block deletion: {}", e)
                })?;
                self.db
                    .write(std::mem::take(&mut hot_delete_batch))
                    .map_err(|e| format!("Failed to delete migrated data from hot DB: {}", e))?;
                pending_migrated = 0;
            }
        }

        if pending_migrated > 0 {
            cold.flush_wal(true)
                .map_err(|e| format!("Failed to sync cold WAL before hot block deletion: {}", e))?;
            self.db
                .write(hot_delete_batch)
                .map_err(|e| format!("Failed to delete migrated data from hot DB: {}", e))?;
        }

        if migrated > 0 {
            tracing::info!(
                "🗄️  Migrated {} blocks (slots < {}) to cold storage in durable batches of at most {}",
                migrated,
                cutoff_slot,
                COLD_BLOCK_MIGRATION_BATCH_SIZE
            );
        }

        Ok(migrated)
    }

    /// Stopped-node migration with bounded transient disk use.
    ///
    /// Raw hot block keys are processed in hash order. After each bounded
    /// write-first batch, the iterator is dropped and only the traversed hash
    /// range is compacted. This lets RocksDB reclaim hot SST space before the
    /// next cold batch is written instead of retaining all tombstoned blocks
    /// until one final full-column-family compaction.
    pub fn migrate_to_cold_with_bounded_compaction(
        &self,
        cutoff_slot: u64,
        compaction_batch_size: u64,
    ) -> Result<(u64, u64), String> {
        if compaction_batch_size == 0 {
            return Err("Cold migration compaction batch size must be positive".to_string());
        }

        let cold = self
            .cold_db
            .as_ref()
            .ok_or_else(|| "Cold storage not attached".to_string())?;
        let hot_blocks_cf = self
            .db
            .cf_handle(CF_BLOCKS)
            .ok_or_else(|| "Blocks CF not found".to_string())?;
        let hot_slots_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let hot_txs_cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;
        let hot_tx_to_slot_cf = self
            .db
            .cf_handle(CF_TX_TO_SLOT)
            .ok_or_else(|| "tx_to_slot CF not found".to_string())?;
        let cold_blocks_cf = cold
            .cf_handle(COLD_CF_BLOCKS)
            .ok_or_else(|| "Cold blocks CF not found".to_string())?;
        let cold_txs_cf = cold
            .cf_handle(COLD_CF_TRANSACTIONS)
            .ok_or_else(|| "Cold transactions CF not found".to_string())?;
        let cold_tx_to_slot_cf = cold
            .cf_handle(COLD_CF_TX_TO_SLOT)
            .ok_or_else(|| "Cold tx_to_slot CF not found".to_string())?;

        let mut total_migrated = 0u64;
        let mut compaction_batches = 0u64;
        let mut after_hash: Option<Vec<u8>> = None;

        loop {
            let mut read_opts = rocksdb::ReadOptions::default();
            read_opts.set_total_order_seek(true);
            let mode = match after_hash.as_deref() {
                Some(hash) => rocksdb::IteratorMode::From(hash, Direction::Forward),
                None => rocksdb::IteratorMode::Start,
            };
            let iter = self.db.iterator_cf_opt(&hot_blocks_cf, read_opts, mode);
            let mut first_visited_hash: Option<Vec<u8>> = None;
            let mut last_visited_hash: Option<Vec<u8>> = None;
            let mut migrated_in_batch = 0u64;
            let mut pending_migrated = 0u64;
            let mut hot_delete_batch = WriteBatch::default();
            let mut reached_end = true;

            for item in iter {
                let (block_hash, block_data) =
                    item.map_err(|e| format!("Failed iterating raw hot blocks: {e}"))?;
                if after_hash.as_deref() == Some(block_hash.as_ref()) {
                    continue;
                }
                first_visited_hash.get_or_insert_with(|| block_hash.to_vec());
                last_visited_hash = Some(block_hash.to_vec());

                let block = Self::decode_cold_migration_block(
                    &block_data,
                    "hot block for bounded cold migration",
                )?;
                if block.header.slot >= cutoff_slot {
                    continue;
                }
                let expected_hash = block.hash();
                if block_hash.as_ref() != expected_hash.0 {
                    return Err(format!(
                        "Refusing bounded cold migration: hot block key {} does not match decoded hash {} at slot {}",
                        hex::encode(&block_hash),
                        expected_hash.to_hex(),
                        block.header.slot
                    ));
                }
                match self
                    .db
                    .get_cf(&hot_slots_cf, block.header.slot.to_be_bytes())
                    .map_err(|e| format!("Failed reading hot slot cursor: {e}"))?
                {
                    Some(cursor) if cursor.as_slice() == expected_hash.0 => {}
                    Some(cursor) => {
                        return Err(format!(
                            "Refusing bounded cold migration: slot {} cursor {} conflicts with hot block {}",
                            block.header.slot,
                            hex::encode(cursor),
                            expected_hash.to_hex()
                        ));
                    }
                    None => {
                        return Err(format!(
                            "Refusing bounded cold migration: hot block {} at slot {} has no canonical slot cursor",
                            expected_hash.to_hex(),
                            block.header.slot
                        ));
                    }
                }

                Self::copy_cold_row_checked(
                    cold,
                    &cold_blocks_cf,
                    COLD_CF_BLOCKS,
                    &block_hash,
                    &block_data,
                )?;
                for tx in &block.transactions {
                    let signature = tx.signature();
                    let hot_tx_data = self
                        .db
                        .get_cf(&hot_txs_cf, signature.0)
                        .map_err(|e| format!("Failed reading hot transaction: {e}"))?;
                    if let Some(tx_data) = hot_tx_data.as_deref() {
                        Self::validate_cold_migration_transaction(
                            tx_data,
                            tx,
                            "hot transaction for bounded cold migration",
                        )?;
                        Self::copy_cold_row_checked(
                            cold,
                            &cold_txs_cf,
                            COLD_CF_TRANSACTIONS,
                            &signature.0,
                            tx_data,
                        )?;
                        hot_delete_batch.delete_cf(&hot_txs_cf, signature.0);
                    } else {
                        let cold_tx_data = cold
                            .get_cf(&cold_txs_cf, signature.0)
                            .map_err(|e| format!("Failed reading cold transaction: {e}"))?
                            .ok_or_else(|| {
                                format!(
                                    "Refusing bounded cold migration: block slot {} transaction {} is missing from hot and cold storage",
                                    block.header.slot,
                                    signature.to_hex()
                                )
                            })?;
                        Self::validate_cold_migration_transaction(
                            &cold_tx_data,
                            tx,
                            "cold-only transaction for bounded cold migration",
                        )?;
                    }

                    let hot_slot_data = self
                        .db
                        .get_cf(&hot_tx_to_slot_cf, signature.0)
                        .map_err(|e| format!("Failed reading hot tx_to_slot: {e}"))?;
                    if let Some(slot_data) = hot_slot_data.as_deref() {
                        Self::validate_cold_migration_tx_slot(
                            slot_data,
                            block.header.slot,
                            "hot tx_to_slot for bounded cold migration",
                        )?;
                        Self::copy_cold_row_checked(
                            cold,
                            &cold_tx_to_slot_cf,
                            COLD_CF_TX_TO_SLOT,
                            &signature.0,
                            slot_data,
                        )?;
                        hot_delete_batch.delete_cf(&hot_tx_to_slot_cf, signature.0);
                    } else {
                        let cold_slot_data = cold
                            .get_cf(&cold_tx_to_slot_cf, signature.0)
                            .map_err(|e| format!("Failed reading cold tx_to_slot: {e}"))?
                            .ok_or_else(|| {
                                format!(
                                    "Refusing bounded cold migration: block slot {} transaction {} has no hot or cold tx_to_slot row",
                                    block.header.slot,
                                    signature.to_hex()
                                )
                            })?;
                        Self::validate_cold_migration_tx_slot(
                            &cold_slot_data,
                            block.header.slot,
                            "cold-only tx_to_slot for bounded cold migration",
                        )?;
                    }
                }
                hot_delete_batch.delete_cf(&hot_blocks_cf, &block_hash);
                migrated_in_batch = migrated_in_batch.saturating_add(1);
                pending_migrated = pending_migrated.saturating_add(1);

                if pending_migrated >= COLD_BLOCK_MIGRATION_BATCH_SIZE {
                    cold.flush_wal(true).map_err(|e| {
                        format!("Failed to sync cold WAL before hot block deletion: {e}")
                    })?;
                    self.db
                        .write(std::mem::take(&mut hot_delete_batch))
                        .map_err(|e| format!("Failed to delete migrated hot rows: {e}"))?;
                    pending_migrated = 0;
                }
                if migrated_in_batch >= compaction_batch_size {
                    reached_end = false;
                    break;
                }
            }

            if pending_migrated > 0 {
                cold.flush_wal(true).map_err(|e| {
                    format!("Failed to sync cold WAL before hot block deletion: {e}")
                })?;
                self.db
                    .write(hot_delete_batch)
                    .map_err(|e| format!("Failed to delete migrated hot rows: {e}"))?;
            }

            if migrated_in_batch > 0 {
                for (name, cf) in [
                    (COLD_CF_BLOCKS, &cold_blocks_cf),
                    (COLD_CF_TRANSACTIONS, &cold_txs_cf),
                    (COLD_CF_TX_TO_SLOT, &cold_tx_to_slot_cf),
                ] {
                    cold.flush_cf(cf)
                        .map_err(|e| format!("Failed flushing cold {name}: {e}"))?;
                }
                self.db
                    .flush_cf(&hot_blocks_cf)
                    .map_err(|e| format!("Failed flushing migrated hot blocks: {e}"))?;
                let compact_start = first_visited_hash.as_deref();
                let compact_end = last_visited_hash
                    .as_deref()
                    .and_then(Self::lexicographic_successor);
                self.db
                    .compact_range_cf(&hot_blocks_cf, compact_start, compact_end.as_deref());
                total_migrated = total_migrated.saturating_add(migrated_in_batch);
                compaction_batches = compaction_batches.saturating_add(1);
                tracing::info!(
                    migrated_in_batch,
                    total_migrated,
                    compaction_batches,
                    "Completed bounded hot-to-cold block migration batch"
                );
            }

            if reached_end {
                break;
            }
            after_hash = last_visited_hash;
        }

        Ok((total_migrated, compaction_batches))
    }

    pub fn compact_migrated_hot_transaction_indexes(&self) -> Result<(), String> {
        for cf_name in [CF_TRANSACTIONS, CF_TX_TO_SLOT] {
            let cf = self
                .db
                .cf_handle(cf_name)
                .ok_or_else(|| format!("{} CF not found", cf_name))?;
            self.db
                .flush_cf(&cf)
                .map_err(|e| format!("Failed flushing {} before compaction: {}", cf_name, e))?;
            self.db.compact_range_cf(&cf, None::<&[u8]>, None::<&[u8]>);
        }
        Ok(())
    }

    /// Flush and compact only the hot column families whose rows are moved by
    /// `migrate_to_cold`. This makes stopped-node maintenance reclaim SST space
    /// before normal validator startup resumes.
    pub fn compact_migrated_hot_history(&self) -> Result<(), String> {
        for cf_name in [CF_BLOCKS, CF_TRANSACTIONS, CF_TX_TO_SLOT] {
            let cf = self
                .db
                .cf_handle(cf_name)
                .ok_or_else(|| format!("{} CF not found", cf_name))?;
            self.db
                .flush_cf(&cf)
                .map_err(|e| format!("Failed flushing {} before compaction: {}", cf_name, e))?;
            self.db.compact_range_cf(&cf, None::<&[u8]>, None::<&[u8]>);
        }
        Ok(())
    }

    /// Migrate per-slot public-history CFs (account snapshots, account_txs,
    /// events, token_transfers, program_calls) to cold storage. Keys are
    /// pubkey(32) + slot(8,BE) + …
    /// so we extract the slot at bytes 32..40 and migrate entries below cutoff.
    pub fn migrate_indexes_to_cold(&self, cutoff_slot: u64) -> Result<u64, String> {
        let cold = self
            .cold_db
            .as_ref()
            .ok_or_else(|| "Cold storage not attached".to_string())?;

        let cf_pairs: &[(&str, &str)] = &[
            (CF_ACCOUNT_TXS, COLD_CF_ACCOUNT_TXS),
            (CF_ACCOUNT_SNAPSHOTS, COLD_CF_ACCOUNT_SNAPSHOTS),
            (CF_EVENTS, COLD_CF_EVENTS),
            (CF_TOKEN_TRANSFERS, COLD_CF_TOKEN_TRANSFERS),
            (CF_PROGRAM_CALLS, COLD_CF_PROGRAM_CALLS),
        ];

        let mut total_migrated: u64 = 0;

        for &(hot_name, cold_name) in cf_pairs {
            let hot_cf = match self.db.cf_handle(hot_name) {
                Some(cf) => cf,
                None => continue,
            };
            let cold_cf = match cold.cf_handle(cold_name) {
                Some(cf) => cf,
                None => continue,
            };

            let mut batch = WriteBatch::default();
            let mut count: u64 = 0;
            let mut read_opts = rocksdb::ReadOptions::default();
            read_opts.set_total_order_seek(true);
            let iter = self
                .db
                .iterator_cf_opt(&hot_cf, read_opts, rocksdb::IteratorMode::Start);

            for item in iter {
                let item = item.map_err(|e| format!("Failed iterating {}: {}", hot_name, e))?;
                if item.0.len() < 40 {
                    continue;
                }
                let slot = u64::from_be_bytes(item.0[32..40].try_into().unwrap());
                if slot >= cutoff_slot {
                    continue;
                }
                Self::copy_cold_row_checked(cold, &cold_cf, cold_name, &item.0, &item.1)?;
                batch.delete_cf(&hot_cf, &item.0);
                count += 1;

                if count.is_multiple_of(10_000) {
                    cold.flush_wal(true).map_err(|e| {
                        format!(
                            "Failed to sync cold WAL before deleting {}: {}",
                            hot_name, e
                        )
                    })?;
                    self.db
                        .write(std::mem::take(&mut batch))
                        .map_err(|e| format!("Failed to delete {} from hot: {}", hot_name, e))?;
                }
            }

            if count > 0 {
                cold.flush_wal(true).map_err(|e| {
                    format!(
                        "Failed to sync cold WAL before deleting {}: {}",
                        hot_name, e
                    )
                })?;
                self.db
                    .write(batch)
                    .map_err(|e| format!("Failed to delete {} from hot: {}", hot_name, e))?;
                tracing::info!(
                    "🗄️  Cold-migrated {} entries from {} (slots < {})",
                    count,
                    hot_name,
                    cutoff_slot
                );
            }
            total_migrated += count;
        }

        Ok(total_migrated)
    }

    /// Returns true if a cold DB is attached.
    pub fn has_cold_storage(&self) -> bool {
        self.cold_db.is_some()
    }

    fn merge_public_history_cf_from_db(
        &self,
        source: PublicHistoryMergeCfSource<'_>,
        target_cf_name: &'static str,
        target_cold: bool,
        dry_run: bool,
    ) -> Result<PublicHistoryMergeCfReport, String> {
        let source_cf = source
            .db
            .cf_handle(source.source_cf_name)
            .ok_or_else(|| format!("Source CF {} not found", source.source_cf_name))?;
        let target_db = if target_cold {
            self.cold_db
                .as_ref()
                .ok_or_else(|| "Cold storage must be attached for cold history merge".to_string())?
                .as_ref()
        } else {
            self.db.as_ref()
        };
        let target_cf = target_db
            .cf_handle(target_cf_name)
            .ok_or_else(|| format!("Target CF {target_cf_name} not found"))?;

        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);
        let iter = source
            .db
            .iterator_cf_opt(&source_cf, read_opts, rocksdb::IteratorMode::Start);

        let mut report = PublicHistoryMergeCfReport {
            source_cf: source.source_cf_name,
            source_cold: source.source_cold,
            target_cf: target_cf_name,
            target_cold,
            ..PublicHistoryMergeCfReport::default()
        };
        let mut batch = WriteBatch::default();
        let mut pending = 0usize;

        for item in iter {
            let (key, value) =
                item.map_err(|err| format!("Failed iterating {}: {err}", source.source_cf_name))?;
            if !is_public_history_merge_row(source.public_cf_name, &key, &value) {
                continue;
            }
            report.source_rows = report.source_rows.saturating_add(1);

            if target_cold {
                if let Some(hot_cf) = self.db.cf_handle(source.public_cf_name) {
                    match self.db.get_cf(&hot_cf, &key).map_err(|err| {
                        format!("Failed reading hot {}: {err}", source.public_cf_name)
                    })? {
                        Some(existing) if existing.as_slice() == value.as_ref() => {
                            report.identical_rows = report.identical_rows.saturating_add(1);
                            continue;
                        }
                        Some(_) => {
                            report.conflict_rows = report.conflict_rows.saturating_add(1);
                            if !dry_run {
                                return Err(format!(
                                    "Refusing public history merge: hot {} key {} differs between source and target",
                                    source.public_cf_name,
                                    hex::encode(&key)
                                ));
                            }
                            continue;
                        }
                        None => {}
                    }
                }
            }

            match target_db
                .get_cf(&target_cf, &key)
                .map_err(|err| format!("Failed reading {target_cf_name}: {err}"))?
            {
                Some(existing) if existing.as_slice() == value.as_ref() => {
                    report.identical_rows = report.identical_rows.saturating_add(1);
                }
                Some(_) => {
                    report.conflict_rows = report.conflict_rows.saturating_add(1);
                    if !dry_run {
                        return Err(format!(
                            "Refusing public history merge: {target_cf_name} key {} differs between source and target",
                            hex::encode(&key)
                        ));
                    }
                }
                None => {
                    report.inserted_rows = report.inserted_rows.saturating_add(1);
                    if !dry_run {
                        batch.put_cf(&target_cf, &key, &value);
                        pending += 1;
                        if pending >= PUBLIC_HISTORY_WRITE_BATCH_SIZE {
                            target_db.write(std::mem::take(&mut batch)).map_err(|err| {
                                format!("Failed writing {target_cf_name} merge batch: {err}")
                            })?;
                            pending = 0;
                        }
                    }
                }
            }
        }

        if !dry_run && pending > 0 {
            target_db
                .write(batch)
                .map_err(|err| format!("Failed writing {target_cf_name} merge batch: {err}"))?;
        }

        Ok(report)
    }

    fn merge_public_history_cf(
        &self,
        source: &StateStore,
        source_cf_name: &'static str,
        target_cf_name: &'static str,
        target_cold: bool,
        dry_run: bool,
    ) -> Result<PublicHistoryMergeCfReport, String> {
        self.merge_public_history_cf_from_db(
            PublicHistoryMergeCfSource {
                db: source.db.as_ref(),
                source_cf_name,
                public_cf_name: source_cf_name,
                source_cold: false,
            },
            target_cf_name,
            target_cold,
            dry_run,
        )
    }

    pub fn merge_public_history_from_source(
        &self,
        source: &StateStore,
        dry_run: bool,
    ) -> Result<PublicHistoryMergeReport, String> {
        self.merge_public_history_from_source_with_mode(
            source,
            dry_run,
            PublicHistoryMergeMode::Full,
        )
    }

    pub fn merge_public_history_indexes_from_source(
        &self,
        source: &StateStore,
        dry_run: bool,
    ) -> Result<PublicHistoryMergeReport, String> {
        self.merge_public_history_from_source_with_mode(
            source,
            dry_run,
            PublicHistoryMergeMode::IndexesOnly,
        )
    }

    fn merge_public_history_from_source_with_mode(
        &self,
        source: &StateStore,
        dry_run: bool,
        mode: PublicHistoryMergeMode,
    ) -> Result<PublicHistoryMergeReport, String> {
        let mut report = PublicHistoryMergeReport {
            dry_run,
            used_cold_store: self.cold_db.is_some(),
            ..PublicHistoryMergeReport::default()
        };

        let full_cold_cf_pairs: &[(&'static str, &'static str)] = &[
            (CF_BLOCKS, COLD_CF_BLOCKS),
            (CF_TRANSACTIONS, COLD_CF_TRANSACTIONS),
            (CF_TX_TO_SLOT, COLD_CF_TX_TO_SLOT),
            (CF_ACCOUNT_TXS, COLD_CF_ACCOUNT_TXS),
            (CF_ACCOUNT_SNAPSHOTS, COLD_CF_ACCOUNT_SNAPSHOTS),
            (CF_EVENTS, COLD_CF_EVENTS),
            (CF_TOKEN_TRANSFERS, COLD_CF_TOKEN_TRANSFERS),
            (CF_PROGRAM_CALLS, COLD_CF_PROGRAM_CALLS),
        ];
        let index_cold_cf_pairs: &[(&'static str, &'static str)] = &[
            (CF_TRANSACTIONS, COLD_CF_TRANSACTIONS),
            (CF_TX_TO_SLOT, COLD_CF_TX_TO_SLOT),
            (CF_ACCOUNT_TXS, COLD_CF_ACCOUNT_TXS),
            (CF_ACCOUNT_SNAPSHOTS, COLD_CF_ACCOUNT_SNAPSHOTS),
            (CF_EVENTS, COLD_CF_EVENTS),
            (CF_TOKEN_TRANSFERS, COLD_CF_TOKEN_TRANSFERS),
            (CF_PROGRAM_CALLS, COLD_CF_PROGRAM_CALLS),
        ];
        let full_hot_cf_names: &[&'static str] = &[
            CF_SLOTS,
            CF_TX_BY_SLOT,
            CF_EVENTS_BY_SLOT,
            CF_EVM_TXS,
            CF_EVM_RECEIPTS,
            CF_EVM_LOGS_BY_SLOT,
            CF_SHIELDED_TXS,
            CF_TX_META,
            CF_NFT_ACTIVITY,
            CF_MARKET_ACTIVITY,
        ];
        let index_hot_cf_names: &[&'static str] = &[
            CF_TX_BY_SLOT,
            CF_EVENTS_BY_SLOT,
            CF_EVM_TXS,
            CF_EVM_RECEIPTS,
            CF_EVM_LOGS_BY_SLOT,
            CF_SHIELDED_TXS,
            CF_TX_META,
            CF_NFT_ACTIVITY,
            CF_MARKET_ACTIVITY,
        ];
        let cold_cf_pairs = match mode {
            PublicHistoryMergeMode::Full => full_cold_cf_pairs,
            PublicHistoryMergeMode::IndexesOnly => index_cold_cf_pairs,
        };
        let hot_cf_names = match mode {
            PublicHistoryMergeMode::Full => full_hot_cf_names,
            PublicHistoryMergeMode::IndexesOnly => index_hot_cf_names,
        };

        if self.cold_db.is_none() && !dry_run {
            return Err("Refusing public history merge without an attached cold store".to_string());
        }

        for &(source_cf, cold_cf) in cold_cf_pairs {
            let cf_report = self.merge_public_history_cf(
                source,
                source_cf,
                if self.cold_db.is_some() {
                    cold_cf
                } else {
                    source_cf
                },
                self.cold_db.is_some(),
                dry_run,
            )?;
            report.source_rows = report.source_rows.saturating_add(cf_report.source_rows);
            report.inserted_rows = report.inserted_rows.saturating_add(cf_report.inserted_rows);
            report.identical_rows = report
                .identical_rows
                .saturating_add(cf_report.identical_rows);
            report.conflict_rows = report.conflict_rows.saturating_add(cf_report.conflict_rows);
            report.cf_reports.push(cf_report);

            if let Some(source_cold) = source.cold_db.as_ref() {
                if source_cold.cf_handle(cold_cf).is_none() {
                    continue;
                }
                let target_cf = if self.cold_db.is_some() {
                    cold_cf
                } else {
                    source_cf
                };
                let cf_report = self.merge_public_history_cf_from_db(
                    PublicHistoryMergeCfSource {
                        db: source_cold.as_ref(),
                        source_cf_name: cold_cf,
                        public_cf_name: source_cf,
                        source_cold: true,
                    },
                    target_cf,
                    self.cold_db.is_some(),
                    dry_run,
                )?;
                report.source_rows = report.source_rows.saturating_add(cf_report.source_rows);
                report.inserted_rows = report.inserted_rows.saturating_add(cf_report.inserted_rows);
                report.identical_rows = report
                    .identical_rows
                    .saturating_add(cf_report.identical_rows);
                report.conflict_rows = report.conflict_rows.saturating_add(cf_report.conflict_rows);
                report.cf_reports.push(cf_report);
            }
        }

        for &cf_name in hot_cf_names {
            let cf_report =
                self.merge_public_history_cf(source, cf_name, cf_name, false, dry_run)?;
            report.source_rows = report.source_rows.saturating_add(cf_report.source_rows);
            report.inserted_rows = report.inserted_rows.saturating_add(cf_report.inserted_rows);
            report.identical_rows = report
                .identical_rows
                .saturating_add(cf_report.identical_rows);
            report.conflict_rows = report.conflict_rows.saturating_add(cf_report.conflict_rows);
            report.cf_reports.push(cf_report);
        }

        if !dry_run {
            report.cleared_account_tx_counters = self.clear_account_tx_counters()?;
        }

        Ok(report)
    }
}
