use std::path::Path;

use crate::block::Block;
use crate::codec::deserialize_legacy_bincode;

use super::*;

const PUBLIC_HISTORY_WRITE_BATCH_SIZE: usize = 10_000;

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

impl PublicHistoryMergeReport {
    pub fn has_conflicts(&self) -> bool {
        self.conflict_rows > 0
    }
}

impl StateStore {
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
        let mut hot_delete_batch = WriteBatch::default();

        let iter = self.db.iterator_cf(
            &hot_slots_cf,
            rocksdb::IteratorMode::From(&0u64.to_be_bytes(), Direction::Forward),
        );

        for item in iter.flatten() {
            if item.0.len() != 8 {
                continue;
            }
            let slot = u64::from_be_bytes(item.0[..8].try_into().unwrap());
            if slot >= cutoff_slot {
                break;
            }

            if item.1.len() != 32 {
                continue;
            }
            let block_hash: [u8; 32] = item.1[..32].try_into().unwrap();

            if let Ok(Some(block_data)) = self.db.get_cf(&hot_blocks_cf, block_hash) {
                cold.put_cf(&cold_blocks_cf, block_hash, &block_data)
                    .map_err(|e| format!("Cold write error (block): {}", e))?;

                let block: Option<Block> = if block_data.first() == Some(&0xBC) {
                    deserialize_legacy_bincode(&block_data[1..], "cold block").ok()
                } else {
                    serde_json::from_slice(&block_data).ok()
                };

                if let Some(block) = block {
                    for tx in &block.transactions {
                        let sig = tx.signature();
                        if let Ok(Some(tx_data)) = self.db.get_cf(&hot_txs_cf, sig.0) {
                            cold.put_cf(&cold_txs_cf, sig.0, &tx_data)
                                .map_err(|e| format!("Cold write error (tx): {}", e))?;
                            hot_delete_batch.delete_cf(&hot_txs_cf, sig.0);
                        }
                        if let Ok(Some(slot_data)) = self.db.get_cf(&hot_tx_to_slot_cf, sig.0) {
                            cold.put_cf(&cold_tx_to_slot_cf, sig.0, &slot_data)
                                .map_err(|e| format!("Cold write error (tx_to_slot): {}", e))?;
                            hot_delete_batch.delete_cf(&hot_tx_to_slot_cf, sig.0);
                        }
                    }
                }

                hot_delete_batch.delete_cf(&hot_blocks_cf, block_hash);
                migrated += 1;
            }
        }

        if migrated > 0 {
            self.db
                .write(hot_delete_batch)
                .map_err(|e| format!("Failed to delete migrated data from hot DB: {}", e))?;
            tracing::info!(
                "🗄️  Migrated {} blocks (slots < {}) to cold storage",
                migrated,
                cutoff_slot
            );
        }

        Ok(migrated)
    }

    /// Migrate per-slot index CFs (account_txs, events, token_transfers,
    /// program_calls) to cold storage. Keys are pubkey(32) + slot(8,BE) + …
    /// so we extract the slot at bytes 32..40 and migrate entries below cutoff.
    pub fn migrate_indexes_to_cold(&self, cutoff_slot: u64) -> Result<u64, String> {
        let cold = self
            .cold_db
            .as_ref()
            .ok_or_else(|| "Cold storage not attached".to_string())?;

        let cf_pairs: &[(&str, &str)] = &[
            (CF_ACCOUNT_TXS, COLD_CF_ACCOUNT_TXS),
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
            let iter = self.db.iterator_cf(&hot_cf, rocksdb::IteratorMode::Start);

            for item in iter.flatten() {
                if item.0.len() < 40 {
                    continue;
                }
                let slot = u64::from_be_bytes(item.0[32..40].try_into().unwrap());
                if slot >= cutoff_slot {
                    continue;
                }
                cold.put_cf(&cold_cf, &item.0, &item.1)
                    .map_err(|e| format!("Cold write error ({}): {}", cold_name, e))?;
                batch.delete_cf(&hot_cf, &item.0);
                count += 1;

                if count.is_multiple_of(10_000) {
                    self.db
                        .write(std::mem::take(&mut batch))
                        .map_err(|e| format!("Failed to delete {} from hot: {}", hot_name, e))?;
                }
            }

            if count > 0 {
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

    /// Prune archive snapshots older than `keep_recent_slots` slots from the
    /// current slot. Returns the number of entries removed.
    pub fn prune_archive_snapshots(&self, current_slot: u64, keep_recent_slots: u64) -> u64 {
        if !self.is_archive_mode() {
            return 0;
        }
        let snap_cf = match self.db.cf_handle(CF_ACCOUNT_SNAPSHOTS) {
            Some(cf) => cf,
            None => return 0,
        };
        let cutoff = current_slot.saturating_sub(keep_recent_slots);
        if cutoff == 0 {
            return 0;
        }

        let mut batch = WriteBatch::default();
        let mut pruned: u64 = 0;
        let iter = self.db.iterator_cf(&snap_cf, rocksdb::IteratorMode::Start);

        for item in iter.flatten() {
            if item.0.len() != 40 {
                continue;
            }
            let slot = u64::from_be_bytes(item.0[32..40].try_into().unwrap());
            if slot < cutoff {
                batch.delete_cf(&snap_cf, &item.0);
                pruned += 1;
            }
        }

        if pruned > 0 {
            if let Err(e) = self.db.write(batch) {
                tracing::warn!("Failed to prune archive snapshots: {}", e);
                return 0;
            }
            tracing::info!(
                "🗂️  Pruned {} archive snapshots older than slot {}",
                pruned,
                cutoff
            );
        }
        pruned
    }

    /// Returns true if a cold DB is attached.
    pub fn has_cold_storage(&self) -> bool {
        self.cold_db.is_some()
    }

    fn merge_public_history_cf_from_db(
        &self,
        source_db: &DB,
        source_cf_name: &'static str,
        public_cf_name: &'static str,
        target_cf_name: &'static str,
        source_cold: bool,
        target_cold: bool,
        dry_run: bool,
    ) -> Result<PublicHistoryMergeCfReport, String> {
        let source_cf = source_db
            .cf_handle(source_cf_name)
            .ok_or_else(|| format!("Source CF {source_cf_name} not found"))?;
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
        let iter = source_db.iterator_cf_opt(&source_cf, read_opts, rocksdb::IteratorMode::Start);

        let mut report = PublicHistoryMergeCfReport {
            source_cf: source_cf_name,
            source_cold,
            target_cf: target_cf_name,
            target_cold,
            ..PublicHistoryMergeCfReport::default()
        };
        let mut batch = WriteBatch::default();
        let mut pending = 0usize;

        for item in iter {
            let (key, value) =
                item.map_err(|err| format!("Failed iterating {source_cf_name}: {err}"))?;
            if !is_public_history_merge_row(public_cf_name, &key, &value) {
                continue;
            }
            report.source_rows = report.source_rows.saturating_add(1);

            if target_cold {
                if let Some(hot_cf) = self.db.cf_handle(public_cf_name) {
                    match self
                        .db
                        .get_cf(&hot_cf, &key)
                        .map_err(|err| format!("Failed reading hot {public_cf_name}: {err}"))?
                    {
                        Some(existing) if existing.as_slice() == value.as_ref() => {
                            report.identical_rows = report.identical_rows.saturating_add(1);
                            continue;
                        }
                        Some(_) => {
                            report.conflict_rows = report.conflict_rows.saturating_add(1);
                            if !dry_run {
                                return Err(format!(
                                    "Refusing public history merge: hot {public_cf_name} key {} differs between source and target",
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
            source.db.as_ref(),
            source_cf_name,
            source_cf_name,
            target_cf_name,
            false,
            target_cold,
            dry_run,
        )
    }

    pub fn merge_public_history_from_source(
        &self,
        source: &StateStore,
        dry_run: bool,
    ) -> Result<PublicHistoryMergeReport, String> {
        let mut report = PublicHistoryMergeReport {
            dry_run,
            used_cold_store: self.cold_db.is_some(),
            ..PublicHistoryMergeReport::default()
        };

        let cold_cf_pairs: &[(&'static str, &'static str)] = &[
            (CF_BLOCKS, COLD_CF_BLOCKS),
            (CF_TRANSACTIONS, COLD_CF_TRANSACTIONS),
            (CF_TX_TO_SLOT, COLD_CF_TX_TO_SLOT),
            (CF_ACCOUNT_TXS, COLD_CF_ACCOUNT_TXS),
            (CF_EVENTS, COLD_CF_EVENTS),
            (CF_TOKEN_TRANSFERS, COLD_CF_TOKEN_TRANSFERS),
            (CF_PROGRAM_CALLS, COLD_CF_PROGRAM_CALLS),
        ];
        let hot_cf_names: &[&'static str] = &[
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
                let target_cf = if self.cold_db.is_some() {
                    cold_cf
                } else {
                    source_cf
                };
                let cf_report = self.merge_public_history_cf_from_db(
                    source_cold.as_ref(),
                    cold_cf,
                    source_cf,
                    target_cf,
                    true,
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
