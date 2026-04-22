use std::path::Path;

use crate::block::Block;

use super::*;

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
                    bincode::deserialize(&block_data[1..]).ok()
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
}
