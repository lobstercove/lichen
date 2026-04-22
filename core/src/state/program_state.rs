use super::*;

impl StateStore {
    pub fn index_program(&self, program: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAMS)
            .ok_or_else(|| "Programs CF not found".to_string())?;

        let is_new = self.db.get_cf(&cf, program.0).ok().flatten().is_none();

        self.db
            .put_cf(&cf, program.0, [])
            .map_err(|e| format!("Failed to store program index: {}", e))?;

        if is_new {
            self.metrics.increment_programs();
        }
        Ok(())
    }

    pub fn get_programs(&self, limit: usize) -> Result<Vec<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAMS)
            .ok_or_else(|| "Programs CF not found".to_string())?;

        let mut results = Vec::new();
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);

        for item in iter {
            let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if key.len() != 32 {
                continue;
            }
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&key);
            results.push(Pubkey(bytes));
            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }

    pub fn get_programs_paginated(
        &self,
        limit: usize,
        after: Option<&Pubkey>,
    ) -> Result<Vec<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAMS)
            .ok_or_else(|| "Programs CF not found".to_string())?;

        let mut results = Vec::new();
        let iter = if let Some(after_pk) = after {
            self.db.iterator_cf(
                &cf,
                rocksdb::IteratorMode::From(&after_pk.0, rocksdb::Direction::Forward),
            )
        } else {
            self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start)
        };

        for item in iter {
            let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if key.len() != 32 {
                continue;
            }
            if let Some(after_pk) = after {
                if key.as_ref() == &after_pk.0[..] {
                    continue;
                }
            }

            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&key);
            results.push(Pubkey(bytes));
            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }

    /// Get all deployed programs/contracts with their stored metadata payloads.
    pub fn get_all_programs(&self, limit: usize) -> Result<Vec<(Pubkey, Value)>, String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAMS)
            .ok_or_else(|| "Programs CF not found".to_string())?;

        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        let mut programs = Vec::new();

        for (key, value) in iter.flatten() {
            if key.len() == 32 {
                let mut pk_bytes = [0u8; 32];
                pk_bytes.copy_from_slice(&key);
                let pk = Pubkey(pk_bytes);
                let metadata: Value = serde_json::from_slice(&value).unwrap_or(Value::Null);
                programs.push((pk, metadata));
                if programs.len() >= limit {
                    break;
                }
            }
        }
        Ok(programs)
    }

    pub fn get_all_programs_paginated(
        &self,
        limit: usize,
        after: Option<&Pubkey>,
    ) -> Result<Vec<(Pubkey, Value)>, String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAMS)
            .ok_or_else(|| "Programs CF not found".to_string())?;

        let iter = if let Some(after_pk) = after {
            self.db.iterator_cf(
                &cf,
                rocksdb::IteratorMode::From(&after_pk.0, rocksdb::Direction::Forward),
            )
        } else {
            self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start)
        };

        let mut programs = Vec::new();
        for (key, value) in iter.flatten() {
            if key.len() != 32 {
                continue;
            }
            if let Some(after_pk) = after {
                if key.as_ref() == &after_pk.0[..] {
                    continue;
                }
            }

            let mut pk_bytes = [0u8; 32];
            pk_bytes.copy_from_slice(&key);
            let pk = Pubkey(pk_bytes);
            let metadata: Value = serde_json::from_slice(&value).unwrap_or(Value::Null);
            programs.push((pk, metadata));
            if programs.len() >= limit {
                break;
            }
        }

        Ok(programs)
    }

    /// Get contract logs (events) for a specific program.
    pub fn get_contract_logs(
        &self,
        program: &Pubkey,
        limit: usize,
        before_slot: Option<u64>,
    ) -> Result<Vec<ContractEvent>, String> {
        self.get_events_by_program(program, limit, before_slot)
    }

    pub fn get_symbol_registry(&self, symbol: &str) -> Result<Option<SymbolRegistryEntry>, String> {
        let normalized = Self::normalize_symbol(symbol)?;
        let cf = self
            .db
            .cf_handle(CF_SYMBOL_REGISTRY)
            .ok_or_else(|| "Symbol registry CF not found".to_string())?;

        match self.db.get_cf(&cf, normalized.as_bytes()) {
            Ok(Some(data)) => {
                let entry: SymbolRegistryEntry = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to decode symbol registry: {}", e))?;
                Ok(Some(entry))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    pub fn get_symbol_registry_by_program(
        &self,
        program: &Pubkey,
    ) -> Result<Option<SymbolRegistryEntry>, String> {
        let cf_rev = self
            .db
            .cf_handle(CF_SYMBOL_BY_PROGRAM)
            .ok_or_else(|| "Symbol-by-program CF not found".to_string())?;

        match self.db.get_cf(&cf_rev, program.0) {
            Ok(Some(symbol_bytes)) => {
                let symbol = String::from_utf8(symbol_bytes.to_vec())
                    .map_err(|e| format!("Invalid UTF-8 in symbol reverse index: {}", e))?;
                self.get_symbol_registry(&symbol)
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    pub fn get_all_symbol_registry(
        &self,
        limit: usize,
    ) -> Result<Vec<SymbolRegistryEntry>, String> {
        let cf = self
            .db
            .cf_handle(CF_SYMBOL_REGISTRY)
            .ok_or_else(|| "Symbol registry CF not found".to_string())?;

        let mut entries = Vec::new();
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        for item in iter {
            if entries.len() >= limit {
                break;
            }
            let (_, value) = item.map_err(|e| format!("Iterator error: {}", e))?;
            let entry: SymbolRegistryEntry = serde_json::from_slice(&value)
                .map_err(|e| format!("Failed to decode symbol registry: {}", e))?;
            entries.push(entry);
        }

        Ok(entries)
    }

    pub fn get_all_symbol_registry_paginated(
        &self,
        limit: usize,
        after_symbol: Option<&str>,
    ) -> Result<Vec<SymbolRegistryEntry>, String> {
        let cf = self
            .db
            .cf_handle(CF_SYMBOL_REGISTRY)
            .ok_or_else(|| "Symbol registry CF not found".to_string())?;

        let normalized_after = if let Some(symbol) = after_symbol {
            Some(Self::normalize_symbol(symbol)?)
        } else {
            None
        };

        let iter = if let Some(after) = normalized_after.as_ref() {
            self.db.iterator_cf(
                &cf,
                rocksdb::IteratorMode::From(after.as_bytes(), rocksdb::Direction::Forward),
            )
        } else {
            self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start)
        };

        let mut entries = Vec::new();
        for item in iter {
            if entries.len() >= limit {
                break;
            }
            let (key, value) = item.map_err(|e| format!("Iterator error: {}", e))?;

            if let Some(after) = normalized_after.as_ref() {
                if key.as_ref() == after.as_bytes() {
                    continue;
                }
            }

            let entry: SymbolRegistryEntry = serde_json::from_slice(&value)
                .map_err(|e| format!("Failed to decode symbol registry: {}", e))?;
            entries.push(entry);
        }

        Ok(entries)
    }

    pub fn register_symbol(
        &self,
        symbol: &str,
        mut entry: SymbolRegistryEntry,
    ) -> Result<(), String> {
        let normalized = Self::normalize_symbol(symbol)?;
        let cf = self
            .db
            .cf_handle(CF_SYMBOL_REGISTRY)
            .ok_or_else(|| "Symbol registry CF not found".to_string())?;

        if self
            .db
            .get_cf(&cf, normalized.as_bytes())
            .map_err(|e| format!("Database error: {}", e))?
            .is_some()
        {
            return Err(format!("Symbol already registered: {}", normalized));
        }

        entry.symbol = normalized.clone();
        let data = serde_json::to_vec(&entry)
            .map_err(|e| format!("Failed to encode symbol registry: {}", e))?;

        self.db
            .put_cf(&cf, normalized.as_bytes(), &data)
            .map_err(|e| format!("Failed to store symbol registry: {}", e))?;

        if let Some(cf_rev) = self.db.cf_handle(CF_SYMBOL_BY_PROGRAM) {
            self.db
                .put_cf(&cf_rev, entry.program.0, normalized.as_bytes())
                .map_err(|e| format!("Failed to store symbol reverse index: {}", e))?;
        }

        Ok(())
    }

    pub fn record_program_call(
        &self,
        activity: &crate::ProgramCallActivity,
        sequence: u32,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAM_CALLS)
            .ok_or_else(|| "Program calls CF not found".to_string())?;

        let mut key = Vec::with_capacity(32 + 8 + 4 + 32);
        key.extend_from_slice(&activity.program.0);
        key.extend_from_slice(&activity.slot.to_be_bytes());
        key.extend_from_slice(&sequence.to_be_bytes());
        key.extend_from_slice(&activity.tx_signature.0);

        let value = crate::encode_program_call_activity(activity)?;
        self.db
            .put_cf(&cf, key, value)
            .map_err(|e| format!("Failed to store program call: {}", e))?;

        if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
            let mut counter_key = Vec::with_capacity(6 + 32);
            counter_key.extend_from_slice(b"pcall:");
            counter_key.extend_from_slice(&activity.program.0);
            let current = match self.db.get_cf(&cf_stats, &counter_key) {
                Ok(Some(data)) if data.len() == 8 => {
                    u64::from_le_bytes(data.as_slice().try_into().unwrap())
                }
                _ => 0,
            };
            if let Err(e) = self
                .db
                .put_cf(&cf_stats, &counter_key, (current + 1).to_le_bytes())
            {
                tracing::warn!("Failed to increment program call counter: {e}");
            }
        }

        Ok(())
    }

    pub fn get_program_calls(
        &self,
        program: &Pubkey,
        limit: usize,
        before_slot: Option<u64>,
    ) -> Result<Vec<crate::ProgramCallActivity>, String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAM_CALLS)
            .ok_or_else(|| "Program calls CF not found".to_string())?;

        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&program.0);

        let mut end_key = prefix.clone();
        if let Some(bs) = before_slot {
            end_key.extend_from_slice(&bs.to_be_bytes());
        } else {
            end_key.extend_from_slice(&[0xFF; 44]);
        }

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&end_key, Direction::Reverse),
        );

        let mut items = Vec::with_capacity(limit);
        for item in iter {
            let (key, value) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if !key.starts_with(&prefix) {
                break;
            }

            if let Some(bs) = before_slot {
                if key.len() >= 40 {
                    let slot_bytes: [u8; 8] = key[32..40].try_into().unwrap_or([0xFF; 8]);
                    let slot = u64::from_be_bytes(slot_bytes);
                    if slot >= bs {
                        continue;
                    }
                }
            }

            let activity = crate::decode_program_call_activity(&value)?;
            items.push(activity);
            if items.len() >= limit {
                break;
            }
        }

        Ok(items)
    }

    pub fn count_program_calls(&self, program: &Pubkey) -> Result<u64, String> {
        let cf_stats = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut counter_key = Vec::with_capacity(6 + 32);
        counter_key.extend_from_slice(b"pcall:");
        counter_key.extend_from_slice(&program.0);

        match self.db.get_cf(&cf_stats, &counter_key) {
            Ok(Some(data)) if data.len() == 8 => {
                Ok(u64::from_le_bytes(data.as_slice().try_into().unwrap()))
            }
            _ => {
                let cf = self
                    .db
                    .cf_handle(CF_PROGRAM_CALLS)
                    .ok_or_else(|| "Program calls CF not found".to_string())?;

                let mut prefix = Vec::with_capacity(32);
                prefix.extend_from_slice(&program.0);

                let mut count = 0u64;
                let iter = self.db.iterator_cf(
                    &cf,
                    rocksdb::IteratorMode::From(&prefix, Direction::Forward),
                );
                for item in iter {
                    let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
                    if !key.starts_with(&prefix) {
                        break;
                    }
                    count += 1;
                }

                if let Err(e) = self.db.put_cf(&cf_stats, &counter_key, count.to_le_bytes()) {
                    tracing::warn!("Failed to cache program TX count: {e}");
                }
                Ok(count)
            }
        }
    }

    pub(crate) fn normalize_symbol(raw: &str) -> Result<String, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("Symbol is required".to_string());
        }
        if !trimmed.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err("Symbol must be alphanumeric".to_string());
        }
        let normalized = trimmed.to_ascii_uppercase();
        if normalized.len() > 10 {
            return Err("Symbol must be 10 characters or less".to_string());
        }
        Ok(normalized)
    }

    pub fn record_market_activity(
        &self,
        activity: &crate::MarketActivity,
        sequence: u32,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_MARKET_ACTIVITY)
            .ok_or_else(|| "Market activity CF not found".to_string())?;

        let zero = [0u8; 32];
        let collection_bytes = activity.collection.as_ref().map(|c| &c.0).unwrap_or(&zero);

        let mut key = Vec::with_capacity(32 + 8 + 4 + 32);
        key.extend_from_slice(collection_bytes);
        key.extend_from_slice(&activity.slot.to_be_bytes());
        key.extend_from_slice(&sequence.to_be_bytes());
        key.extend_from_slice(&activity.tx_signature.0);

        let value = crate::encode_market_activity(activity)?;
        self.db
            .put_cf(&cf, key, value)
            .map_err(|e| format!("Failed to store market activity: {}", e))
    }

    pub fn get_market_activity(
        &self,
        collection: Option<&Pubkey>,
        kind: Option<crate::MarketActivityKind>,
        limit: usize,
    ) -> Result<Vec<crate::MarketActivity>, String> {
        let cf = self
            .db
            .cf_handle(CF_MARKET_ACTIVITY)
            .ok_or_else(|| "Market activity CF not found".to_string())?;

        let mut items = Vec::with_capacity(limit);

        let iter = if let Some(collection) = collection {
            let mut prefix = Vec::with_capacity(32);
            prefix.extend_from_slice(&collection.0);
            let mut end_key = prefix.clone();
            end_key.extend_from_slice(&[0xFF; 48]);
            self.db.iterator_cf(
                &cf,
                rocksdb::IteratorMode::From(&end_key, Direction::Reverse),
            )
        } else {
            self.db.iterator_cf(&cf, rocksdb::IteratorMode::End)
        };

        let prefix = collection.map(|c| c.0);

        for item in iter {
            let (key, value) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if let Some(prefix_bytes) = prefix.as_ref() {
                if !key.starts_with(prefix_bytes) {
                    break;
                }
            }

            let activity = crate::decode_market_activity(&value)?;
            if let Some(filter_kind) = kind.as_ref() {
                if &activity.kind != filter_kind {
                    continue;
                }
            }

            items.push(activity);
            if items.len() >= limit {
                break;
            }
        }

        Ok(items)
    }
}
