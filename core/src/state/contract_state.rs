use super::*;

impl StateStore {
    /// Store a contract event. Key: program_pubkey + slot(BE) + name_hash(BE) + seq_counter.
    /// Matches the batch writer key format for consistency.
    pub fn put_contract_event(
        &self,
        program: &Pubkey,
        event: &ContractEvent,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVENTS)
            .ok_or_else(|| "Events CF not found".to_string())?;

        let seq = self.next_event_seq(program, event.slot)?;

        let name_hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};

            let mut hasher = DefaultHasher::new();
            event.name.hash(&mut hasher);
            hasher.finish()
        };

        let mut key = Vec::with_capacity(32 + 8 + 8 + 8);
        key.extend_from_slice(&program.0);
        key.extend_from_slice(&event.slot.to_be_bytes());
        key.extend_from_slice(&name_hash.to_be_bytes());
        key.extend_from_slice(&seq.to_be_bytes());

        let data =
            serde_json::to_vec(event).map_err(|e| format!("Failed to serialize event: {}", e))?;

        let mut batch = WriteBatch::default();
        batch.put_cf(&cf, &key, &data);

        if let Some(cf_slot) = self.db.cf_handle(CF_EVENTS_BY_SLOT) {
            let mut slot_key = Vec::with_capacity(8 + 32 + 8);
            slot_key.extend_from_slice(&event.slot.to_be_bytes());
            slot_key.extend_from_slice(&program.0);
            slot_key.extend_from_slice(&seq.to_be_bytes());
            batch.put_cf(&cf_slot, &slot_key, &key);
        }

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to atomically store event + index: {}", e))?;
        Ok(())
    }

    /// Write contract storage key/value to CF_CONTRACT_STORAGE.
    pub fn put_contract_storage(
        &self,
        program: &Pubkey,
        storage_key: &[u8],
        value: &[u8],
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;
        let mut key = Vec::with_capacity(32 + storage_key.len());
        key.extend_from_slice(&program.0);
        key.extend_from_slice(storage_key);
        self.db
            .put_cf(&cf, &key, value)
            .map_err(|e| format!("Failed to store contract storage: {}", e))?;
        self.mark_contract_storage_dirty(&key);
        Ok(())
    }

    /// Delete contract storage from CF_CONTRACT_STORAGE.
    pub fn delete_contract_storage(
        &self,
        program: &Pubkey,
        storage_key: &[u8],
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;
        let mut key = Vec::with_capacity(32 + storage_key.len());
        key.extend_from_slice(&program.0);
        key.extend_from_slice(storage_key);
        self.db
            .delete_cf(&cf, &key)
            .map_err(|e| format!("Failed to delete contract storage: {}", e))?;
        self.mark_contract_storage_dirty(&key);
        Ok(())
    }

    /// Point-read a single contract storage key from CF_CONTRACT_STORAGE.
    pub fn get_contract_storage(
        &self,
        program: &Pubkey,
        storage_key: &[u8],
    ) -> Result<Option<Vec<u8>>, String> {
        let cf = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;
        let mut key = Vec::with_capacity(32 + storage_key.len());
        key.extend_from_slice(&program.0);
        key.extend_from_slice(storage_key);
        self.db
            .get_cf(&cf, &key)
            .map(|opt| opt.map(|value| value.to_vec()))
            .map_err(|e| format!("Failed to read contract storage: {}", e))
    }

    pub fn get_contract_storage_u64(&self, program: &Pubkey, storage_key: &[u8]) -> u64 {
        match self.get_contract_storage(program, storage_key) {
            Ok(Some(data)) if data.len() >= 8 => {
                u64::from_le_bytes(data[..8].try_into().unwrap_or([0; 8]))
            }
            _ => 0,
        }
    }

    /// Resolve symbol name -> program and read a single storage key.
    pub fn get_program_storage(&self, symbol: &str, storage_key: &[u8]) -> Option<Vec<u8>> {
        let entry = self.get_symbol_registry(symbol).ok()??;
        self.get_contract_storage(&entry.program, storage_key)
            .ok()?
    }

    /// Resolve symbol -> program and read a u64 storage value.
    pub fn get_program_storage_u64(&self, symbol: &str, storage_key: &[u8]) -> u64 {
        match self.get_symbol_registry(symbol) {
            Ok(Some(entry)) => self.get_contract_storage_u64(&entry.program, storage_key),
            _ => 0,
        }
    }

    /// Iterate contract storage entries using prefix scan pagination.
    pub fn get_contract_storage_entries(
        &self,
        program: &Pubkey,
        limit: usize,
        after_key: Option<Vec<u8>>,
    ) -> Result<KvEntries, String> {
        let cf = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;

        let prefix = program.0.to_vec();
        let start = if let Some(after_key) = after_key {
            let mut key = prefix.clone();
            key.extend_from_slice(&after_key);
            key.push(0);
            key
        } else {
            prefix.clone()
        };

        let iter = self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::From(&start, Direction::Forward));

        let mut results = Vec::new();
        for item in iter {
            let (key, value) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if !key.starts_with(&prefix) {
                break;
            }
            let storage_key = key[32..].to_vec();
            results.push((storage_key, value.to_vec()));
            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }

    /// Count canonical storage entries and aggregate value bytes for a contract.
    pub fn get_contract_storage_stats(
        &self,
        program: &Pubkey,
    ) -> Result<ContractStorageStats, String> {
        let cf = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;

        let prefix = program.0.to_vec();
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&prefix, Direction::Forward),
        );

        let mut entry_count = 0usize;
        let mut total_value_size = 0usize;
        for item in iter {
            let (key, value) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if !key.starts_with(&prefix) {
                break;
            }
            entry_count += 1;
            total_value_size += value.len();
        }

        Ok(ContractStorageStats {
            entry_count,
            total_value_size,
        })
    }

    /// Load the full live storage map for a contract from CF_CONTRACT_STORAGE.
    pub fn load_contract_storage_map(&self, program: &Pubkey) -> Result<KvEntries, String> {
        self.get_contract_storage_entries(program, usize::MAX, None)
    }

    /// Get events for a specific program, newest first, with limit.
    pub fn get_events_by_program(
        &self,
        program: &Pubkey,
        limit: usize,
        before_slot: Option<u64>,
    ) -> Result<Vec<ContractEvent>, String> {
        let cf = self
            .db
            .cf_handle(CF_EVENTS)
            .ok_or_else(|| "Events CF not found".to_string())?;

        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&program.0);

        let mut end_key = prefix.clone();
        if let Some(before_slot) = before_slot {
            end_key.extend_from_slice(&before_slot.to_be_bytes());
        } else {
            end_key.extend_from_slice(&[0xFF; 16]);
        }

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&end_key, Direction::Reverse),
        );

        let mut events = Vec::new();
        for (key, value) in iter.flatten() {
            if !key.starts_with(&prefix) {
                break;
            }
            if let Some(before_slot) = before_slot {
                if key.len() >= 40 {
                    let slot_bytes: [u8; 8] = key[32..40].try_into().unwrap_or([0xFF; 8]);
                    let slot = u64::from_be_bytes(slot_bytes);
                    if slot >= before_slot {
                        continue;
                    }
                }
            }
            if let Ok(event) = serde_json::from_slice::<ContractEvent>(&value) {
                events.push(event);
                if events.len() >= limit {
                    break;
                }
            }
        }

        Ok(events)
    }

    /// Get all events across all programs for a given slot.
    pub fn get_events_by_slot(
        &self,
        slot: u64,
        limit: usize,
    ) -> Result<Vec<ContractEvent>, String> {
        let cf_slot = self
            .db
            .cf_handle(CF_EVENTS_BY_SLOT)
            .ok_or_else(|| "Events-by-slot CF not found".to_string())?;
        let cf_events = self
            .db
            .cf_handle(CF_EVENTS)
            .ok_or_else(|| "Events CF not found".to_string())?;

        let slot_prefix = slot.to_be_bytes();
        let iter = self.db.iterator_cf(
            &cf_slot,
            rocksdb::IteratorMode::From(&slot_prefix, Direction::Forward),
        );

        let mut events = Vec::new();
        for item in iter.flatten() {
            let (key, event_key) = item;
            if key.len() < 8 || key[..8] != slot_prefix {
                break;
            }
            if let Ok(Some(data)) = self.db.get_cf(&cf_events, &*event_key) {
                if let Ok(event) = serde_json::from_slice::<ContractEvent>(&data) {
                    events.push(event);
                    if events.len() >= limit {
                        break;
                    }
                }
            }
        }

        Ok(events)
    }

    /// Atomic event sequence counter per program+slot.
    fn next_event_seq(&self, program: &Pubkey, slot: u64) -> Result<u64, String> {
        let _guard = self
            .event_seq_lock
            .lock()
            .map_err(|e| format!("Event seq lock poisoned: {}", e))?;

        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut key = Vec::with_capacity(4 + 32 + 8);
        key.extend_from_slice(b"esq:");
        key.extend_from_slice(&program.0);
        key.extend_from_slice(&slot.to_be_bytes());

        let current = match self.db.get_cf(&cf, &key) {
            Ok(Some(data)) if data.len() == 8 => {
                u64::from_le_bytes(data.as_slice().try_into().unwrap())
            }
            _ => 0,
        };
        let next = current + 1;
        self.db
            .put_cf(&cf, &key, next.to_le_bytes())
            .map_err(|e| format!("Failed to update event seq: {}", e))?;
        Ok(current)
    }
}
