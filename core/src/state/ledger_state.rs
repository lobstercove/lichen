use crate::block::Block;

use super::*;

impl StateStore {
    /// Get the last processed slot
    pub fn get_last_slot(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        match self.db.get_cf(&cf, b"last_slot") {
            Ok(Some(data)) => {
                let bytes: [u8; 8] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid slot data".to_string())?;
                Ok(u64::from_be_bytes(bytes))
            }
            Ok(None) => Ok(0),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Update the last processed slot
    pub fn set_last_slot(&self, slot: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        self.db
            .put_cf(&cf, b"last_slot", slot.to_be_bytes())
            .map_err(|e| format!("Failed to store slot: {}", e))
    }

    /// Get the last confirmed slot (2/3 supermajority reached)
    pub fn get_last_confirmed_slot(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        match self.db.get_cf(&cf, b"confirmed_slot") {
            Ok(Some(data)) => {
                let bytes: [u8; 8] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid confirmed slot data".to_string())?;
                Ok(u64::from_be_bytes(bytes))
            }
            Ok(None) => Ok(0),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Update the last confirmed slot
    pub fn set_last_confirmed_slot(&self, slot: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        self.db
            .put_cf(&cf, b"confirmed_slot", slot.to_be_bytes())
            .map_err(|e| format!("Failed to store confirmed slot: {}", e))
    }

    /// Get the last finalized slot under the active BFT commitment policy.
    pub fn get_last_finalized_slot(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        match self.db.get_cf(&cf, b"finalized_slot") {
            Ok(Some(data)) => {
                let bytes: [u8; 8] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid finalized slot data".to_string())?;
                Ok(u64::from_be_bytes(bytes))
            }
            Ok(None) => Ok(0),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Update the last finalized slot
    pub fn set_last_finalized_slot(&self, slot: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        self.db
            .put_cf(&cf, b"finalized_slot", slot.to_be_bytes())
            .map_err(|e| format!("Failed to store finalized slot: {}", e))
    }

    /// Get the hashes of the last N blocks for replay protection.
    /// Returns a set of block hashes from the most recent `count` slots.
    /// T1.3 fix: Hash::default() is NO LONGER accepted. Only real block hashes
    /// are valid for replay protection. Genesis block hash is included if in range.
    ///
    /// PERF-OPT 3: Uses an in-memory cache that is populated on block commit
    /// and avoids reading up to 300 blocks from RocksDB on every call.
    pub fn get_recent_blockhashes(
        &self,
        count: u64,
    ) -> Result<std::collections::HashSet<Hash>, String> {
        {
            let cache = self
                .blockhash_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(ref inner) = *cache {
                let last_slot = self.get_last_slot()?;
                let start_slot = last_slot.saturating_sub(count);
                let hashes: std::collections::HashSet<Hash> = inner
                    .entries
                    .iter()
                    .filter(|(slot, _)| *slot >= start_slot)
                    .map(|(_, hash)| *hash)
                    .collect();
                if !hashes.is_empty() {
                    return Ok(hashes);
                }
            }
        }

        let mut hashes = std::collections::HashSet::new();
        let last_slot = self.get_last_slot()?;
        let start_slot = last_slot.saturating_sub(count);
        let mut entries: Vec<(u64, Hash)> = Vec::new();
        for slot in start_slot..=last_slot {
            if let Ok(Some(block)) = self.get_block_by_slot(slot) {
                let hash = block.hash();
                hashes.insert(hash);
                entries.push((slot, hash));
            }
        }

        {
            let mut cache = self
                .blockhash_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *cache = Some(BlockhashCache { entries });
        }

        Ok(hashes)
    }

    /// PERF-OPT 3: Push a new blockhash into the in-memory cache after committing a block.
    /// Evicts entries older than 300 slots.
    fn push_blockhash_cache(&self, hash: Hash, slot: u64) {
        let mut cache = self
            .blockhash_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let inner = cache.get_or_insert_with(|| BlockhashCache {
            entries: Vec::new(),
        });
        inner.entries.push((slot, hash));
        let cutoff = slot.saturating_sub(300);
        inner
            .entries
            .retain(|(entry_slot, _)| *entry_slot >= cutoff);
    }

    /// Store a block and all related per-transaction indexes atomically.
    fn write_block_batch(
        &self,
        block: &Block,
        last_slot: Option<u64>,
        confirmed_slot: Option<u64>,
        finalized_slot: Option<u64>,
    ) -> Result<(), String> {
        let _block_write_guard = self
            .block_write_lock
            .lock()
            .map_err(|_| "Block write lock poisoned".to_string())?;

        let cf = self
            .db
            .cf_handle(CF_BLOCKS)
            .ok_or_else(|| "Blocks CF not found".to_string())?;
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
            .ok_or_else(|| "TX to slot CF not found".to_string())?;
        let tx_by_slot_cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "TX by slot CF not found".to_string())?;

        let block_hash = block.hash();
        let mut value = Vec::with_capacity(4096);
        value.push(0xBC);
        bincode::serialize_into(&mut value, block)
            .map_err(|e| format!("Failed to serialize block: {}", e))?;

        let is_new_slot = self
            .get_block_by_slot(block.header.slot)
            .unwrap_or(None)
            .is_none();

        let mut batch = WriteBatch::default();
        let current_last_slot = self.get_last_slot().unwrap_or(0);
        let current_confirmed_slot = self.get_last_confirmed_slot().unwrap_or(0);
        let current_finalized_slot = self.get_last_finalized_slot().unwrap_or(0);

        batch.put_cf(&cf, block_hash.0, &value);
        batch.put_cf(&slot_cf, block.header.slot.to_be_bytes(), block_hash.0);
        if let Some(slot) = last_slot {
            batch.put_cf(
                &slot_cf,
                b"last_slot",
                slot.max(current_last_slot).to_be_bytes(),
            );
        }
        if let Some(slot) = confirmed_slot {
            batch.put_cf(
                &slot_cf,
                b"confirmed_slot",
                slot.max(current_confirmed_slot).to_be_bytes(),
            );
        }
        if let Some(slot) = finalized_slot {
            batch.put_cf(
                &slot_cf,
                b"finalized_slot",
                slot.max(current_finalized_slot).to_be_bytes(),
            );
        }

        for (tx_index, tx) in block.transactions.iter().enumerate() {
            let sig = tx.signature();

            let mut tx_value = Vec::with_capacity(512);
            tx_value.push(0xBC);
            match bincode::serialize_into(&mut tx_value, tx) {
                Ok(()) => {
                    batch.put_cf(&tx_cf, sig.0, &tx_value);
                }
                Err(e) => tracing::warn!("Failed to serialize tx {}: {}", sig.to_hex(), e),
            }

            batch.put_cf(&tx_to_slot_cf, sig.0, block.header.slot.to_be_bytes());

            let mut key = Vec::with_capacity(16);
            key.extend_from_slice(&block.header.slot.to_be_bytes());
            key.extend_from_slice(&(tx_index as u64).to_be_bytes());
            batch.put_cf(&tx_by_slot_cf, &key, sig.0);
        }

        self.batch_index_account_transactions(block, &mut batch)?;

        if is_new_slot {
            self.metrics.track_block(block);
            self.metrics.save_to_batch(&mut batch, &self.db)?;
        }

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to write block batch: {}", e))?;

        self.push_blockhash_cache(block_hash, block.header.slot);

        Ok(())
    }

    pub fn put_block(&self, block: &Block) -> Result<(), String> {
        self.write_block_batch(block, None, None, None)
    }

    /// Use `put_block_atomic` for canonical block application so block storage,
    /// tip advance, and commitment metadata land in the same durable WriteBatch.
    pub fn put_block_atomic(
        &self,
        block: &Block,
        confirmed_slot: Option<u64>,
        finalized_slot: Option<u64>,
    ) -> Result<(), String> {
        self.write_block_batch(
            block,
            Some(block.header.slot),
            confirmed_slot,
            finalized_slot,
        )
    }

    pub fn get_block(&self, hash: &Hash) -> Result<Option<Block>, String> {
        let cf = self
            .db
            .cf_handle(CF_BLOCKS)
            .ok_or_else(|| "Blocks CF not found".to_string())?;

        match self.db.get_cf(&cf, hash.0) {
            Ok(Some(data)) => {
                let block: Block = if data.first() == Some(&0xBC) {
                    bincode::deserialize(&data[1..])
                        .map_err(|e| format!("Failed to deserialize block (bincode): {}", e))?
                } else {
                    serde_json::from_slice(&data)
                        .map_err(|e| format!("Failed to deserialize block (json): {}", e))?
                };
                Ok(Some(block))
            }
            Ok(None) => {
                if let Some(ref cold) = self.cold_db {
                    if let Some(cold_cf) = cold.cf_handle(COLD_CF_BLOCKS) {
                        if let Ok(Some(data)) = cold.get_cf(&cold_cf, hash.0) {
                            let block: Block = if data.first() == Some(&0xBC) {
                                bincode::deserialize(&data[1..]).map_err(|e| {
                                    format!("Failed to deserialize cold block (bincode): {}", e)
                                })?
                            } else {
                                serde_json::from_slice(&data).map_err(|e| {
                                    format!("Failed to deserialize cold block (json): {}", e)
                                })?
                            };
                            return Ok(Some(block));
                        }
                    }
                }
                Ok(None)
            }
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Get block by slot
    pub fn get_block_by_slot(&self, slot: u64) -> Result<Option<Block>, String> {
        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        match self.db.get_cf(&slot_cf, slot.to_be_bytes()) {
            Ok(Some(hash_bytes)) => {
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&hash_bytes);
                self.get_block(&Hash(hash))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Get recent blocks via the slot index, newest first.
    /// Pass `before_slot` to fetch the next page strictly before that slot.
    pub fn get_recent_blocks(
        &self,
        limit: usize,
        before_slot: Option<u64>,
    ) -> Result<Vec<Block>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        let seek_key = before_slot.unwrap_or(u64::MAX).to_be_bytes().to_vec();
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);

        let iter = self.db.iterator_cf_opt(
            &slot_cf,
            read_opts,
            rocksdb::IteratorMode::From(&seek_key, Direction::Reverse),
        );

        let mut blocks = Vec::with_capacity(limit);
        for item in iter {
            let (key, value) = item.map_err(|e| format!("Slot index iterator error: {}", e))?;

            // CF_SLOTS also stores metadata such as last_slot/confirmed_slot.
            if key.len() != 8 || value.len() != 32 {
                continue;
            }

            let slot = u64::from_be_bytes(
                key.as_ref()
                    .try_into()
                    .map_err(|_| "Invalid slot index key".to_string())?,
            );

            if let Some(before) = before_slot {
                if slot >= before {
                    continue;
                }
            }

            let mut hash_bytes = [0u8; 32];
            hash_bytes.copy_from_slice(value.as_ref());
            if let Some(block) = self.get_block(&Hash(hash_bytes))? {
                blocks.push(block);
            }

            if blocks.len() >= limit {
                break;
            }
        }

        Ok(blocks)
    }

    /// Store a transaction
    pub fn put_transaction(&self, tx: &Transaction) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;

        let sig = tx.signature();
        let mut value = Vec::with_capacity(512);
        value.push(0xBC);
        bincode::serialize_into(&mut value, tx)
            .map_err(|e| format!("Failed to serialize transaction: {}", e))?;

        self.db
            .put_cf(&cf, sig.0, &value)
            .map_err(|e| format!("Failed to store transaction: {}", e))
    }

    /// Get transaction by signature
    pub fn get_transaction(&self, sig: &Hash) -> Result<Option<Transaction>, String> {
        let cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;

        match self.db.get_cf(&cf, sig.0) {
            Ok(Some(data)) => {
                let tx: Transaction = if data.first() == Some(&0xBC) {
                    bincode::deserialize(&data[1..]).map_err(|e| {
                        format!("Failed to deserialize transaction (bincode): {}", e)
                    })?
                } else {
                    serde_json::from_slice(&data)
                        .map_err(|e| format!("Failed to deserialize transaction (json): {}", e))?
                };
                Ok(Some(tx))
            }
            Ok(None) => {
                if let Some(ref cold) = self.cold_db {
                    if let Some(cold_cf) = cold.cf_handle(COLD_CF_TRANSACTIONS) {
                        if let Ok(Some(data)) = cold.get_cf(&cold_cf, sig.0) {
                            let tx: Transaction = if data.first() == Some(&0xBC) {
                                bincode::deserialize(&data[1..]).map_err(|e| {
                                    format!(
                                        "Failed to deserialize cold transaction (bincode): {}",
                                        e
                                    )
                                })?
                            } else {
                                serde_json::from_slice(&data).map_err(|e| {
                                    format!("Failed to deserialize cold transaction (json): {}", e)
                                })?
                            };
                            return Ok(Some(tx));
                        }
                    }
                }
                Ok(None)
            }
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Delete transaction record (used during fork choice to allow re-replay)
    pub fn delete_transaction(&self, sig: &Hash) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;

        self.db
            .delete_cf(&cf, sig.0)
            .map_err(|e| format!("Failed to delete transaction: {}", e))
    }

    /// Store transaction execution metadata (compute_units_used).
    pub fn put_tx_meta(&self, sig: &Hash, compute_units_used: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TX_META)
            .ok_or_else(|| "TX meta CF not found".to_string())?;
        self.db
            .put_cf(&cf, sig.0, compute_units_used.to_le_bytes())
            .map_err(|e| format!("Failed to store tx meta: {}", e))
    }

    /// Store full transaction execution metadata (CU, return_code, return_data, logs).
    pub fn put_tx_meta_full(
        &self,
        sig: &Hash,
        meta: &crate::processor::TxMeta,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TX_META)
            .ok_or_else(|| "TX meta CF not found".to_string())?;
        let data =
            bincode::serialize(meta).map_err(|e| format!("Failed to serialize tx meta: {}", e))?;
        self.db
            .put_cf(&cf, sig.0, data)
            .map_err(|e| format!("Failed to store tx meta: {}", e))
    }

    /// Get stored compute_units_used for a transaction.
    /// Handles both old 8-byte format and new bincode TxMeta format.
    pub fn get_tx_meta_cu(&self, sig: &Hash) -> Result<Option<u64>, String> {
        let cf = self
            .db
            .cf_handle(CF_TX_META)
            .ok_or_else(|| "TX meta CF not found".to_string())?;
        match self.db.get_cf(&cf, sig.0) {
            Ok(Some(data)) if data.len() == 8 => {
                Ok(Some(u64::from_le_bytes(data.try_into().unwrap())))
            }
            Ok(Some(data)) => {
                if let Ok(meta) = bincode::deserialize::<crate::processor::TxMeta>(&data) {
                    Ok(Some(meta.compute_units_used))
                } else {
                    Ok(None)
                }
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Get full transaction execution metadata.
    /// Returns None for transactions stored in the old 8-byte CU-only format
    /// (those are handled transparently with default return_code/return_data/logs).
    pub fn get_tx_meta_full(&self, sig: &Hash) -> Result<Option<crate::processor::TxMeta>, String> {
        let cf = self
            .db
            .cf_handle(CF_TX_META)
            .ok_or_else(|| "TX meta CF not found".to_string())?;
        match self.db.get_cf(&cf, sig.0) {
            Ok(Some(data)) if data.len() == 8 => Ok(Some(crate::processor::TxMeta {
                compute_units_used: u64::from_le_bytes(data.try_into().unwrap()),
                ..Default::default()
            })),
            Ok(Some(data)) => bincode::deserialize(&data)
                .map(Some)
                .map_err(|e| format!("Failed to deserialize tx meta: {}", e)),
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }
}
