use super::*;

impl StateStore {
    /// Store validator info
    pub fn put_validator(&self, info: &crate::consensus::ValidatorInfo) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_VALIDATORS)
            .ok_or_else(|| "Validators CF not found".to_string())?;

        let key = info.pubkey.0;
        // Only increment counter for newly registered validators (not updates)
        let is_new = self.db.get_cf(&cf, key).ok().flatten().is_none();

        let value = serde_json::to_vec(info)
            .map_err(|e| format!("Failed to serialize validator: {}", e))?;

        self.db
            .put_cf(&cf, key, value)
            .map_err(|e| format!("Failed to store validator: {}", e))?;

        if is_new {
            self.metrics.increment_validators();
        }
        Ok(())
    }

    /// Delete validator from state
    pub fn delete_validator(&self, pubkey: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_VALIDATORS)
            .ok_or_else(|| "Validators CF not found".to_string())?;

        // Only decrement if the validator actually exists
        let exists = self.db.get_cf(&cf, pubkey.0).ok().flatten().is_some();

        self.db
            .delete_cf(&cf, pubkey.0)
            .map_err(|e| format!("Failed to delete validator: {}", e))?;

        if exists {
            self.metrics.decrement_validators();
        }
        Ok(())
    }

    /// Get validator info
    pub fn get_validator(
        &self,
        pubkey: &Pubkey,
    ) -> Result<Option<crate::consensus::ValidatorInfo>, String> {
        let cf = self
            .db
            .cf_handle(CF_VALIDATORS)
            .ok_or_else(|| "Validators CF not found".to_string())?;

        match self
            .db
            .get_cf(&cf, pubkey.0)
            .map_err(|e| format!("Failed to get validator: {}", e))?
        {
            Some(bytes) => {
                let info = serde_json::from_slice(&bytes)
                    .map_err(|e| format!("Failed to deserialize validator: {}", e))?;
                Ok(Some(info))
            }
            None => Ok(None),
        }
    }

    /// Get all validators
    pub fn get_all_validators(&self) -> Result<Vec<crate::consensus::ValidatorInfo>, String> {
        let cf = self
            .db
            .cf_handle(CF_VALIDATORS)
            .ok_or_else(|| "Validators CF not found".to_string())?;

        let mut validators = Vec::new();
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);

        for item in iter {
            let (_key, value) = item.map_err(|e| format!("Iterator error: {}", e))?;
            let info: crate::consensus::ValidatorInfo = serde_json::from_slice(&value)
                .map_err(|e| format!("Failed to deserialize validator: {}", e))?;
            validators.push(info);
        }

        Ok(validators)
    }

    /// Load validator set from state
    pub fn load_validator_set(&self) -> Result<crate::consensus::ValidatorSet, String> {
        let mut set = crate::consensus::ValidatorSet::new();
        let validators = self.get_all_validators()?;

        for validator in validators {
            set.add_validator(validator);
        }

        Ok(set)
    }

    /// Save entire validator set to state (replaces all existing entries)
    /// PHASE1-FIX S-4: Atomic clear-and-replace in a single WriteBatch to prevent
    /// intermediate states where validators are partially cleared.
    pub fn save_validator_set(&self, set: &crate::consensus::ValidatorSet) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_VALIDATORS)
            .ok_or_else(|| "Validators CF not found".to_string())?;

        let mut batch = rocksdb::WriteBatch::default();

        // Delete all existing validator entries
        let keys: Vec<Box<[u8]>> = self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::Start)
            .filter_map(|item| item.ok().map(|(k, _)| k))
            .collect();
        for key in &keys {
            batch.delete_cf(&cf, key);
        }

        // Insert all current validators
        for validator in set.validators() {
            let data = serde_json::to_vec(validator)
                .map_err(|e| format!("Failed to serialize validator: {}", e))?;
            batch.put_cf(&cf, validator.pubkey.0, data);
        }

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to save validator set: {}", e))?;

        // Update counter
        self.metrics
            .set_validator_count(set.validators().len() as u64);
        Ok(())
    }

    /// Remove ALL validators from the CF (used before full re-save)
    pub fn clear_all_validators(&self) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_VALIDATORS)
            .ok_or_else(|| "Validators CF not found".to_string())?;

        // Collect keys, then batch-delete in a single atomic WriteBatch
        let keys: Vec<Box<[u8]>> = self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::Start)
            .filter_map(|item| item.ok().map(|(k, _)| k))
            .collect();

        if keys.is_empty() {
            return Ok(());
        }

        let mut batch = rocksdb::WriteBatch::default();
        for key in &keys {
            batch.delete_cf(&cf, key);
        }
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to clear validators: {}", e))?;

        // Reset the validator counter
        self.metrics.set_validator_count(0);
        Ok(())
    }

    // ─── Epoch-based pending validator change queue ─────────────────────────

    /// Queue a validator set change for application at the given epoch boundary.
    ///
    /// Key format: epoch(8,BE) + queued_at_slot(8,BE) + pubkey_prefix(8)
    /// This ensures changes are ordered by epoch, slot, and validator.
    pub fn queue_pending_validator_change(
        &self,
        change: &crate::consensus::PendingValidatorChange,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_PENDING_VALIDATOR_CHANGES)
            .ok_or_else(|| "Pending validator changes CF not found".to_string())?;

        let mut key = Vec::with_capacity(24);
        key.extend_from_slice(&change.effective_epoch.to_be_bytes());
        key.extend_from_slice(&change.queued_at_slot.to_be_bytes());
        key.extend_from_slice(&change.pubkey.0[..8]);

        let value = serde_json::to_vec(change)
            .map_err(|e| format!("Failed to serialize PendingValidatorChange: {}", e))?;

        self.db
            .put_cf(&cf, &key, value)
            .map_err(|e| format!("Failed to queue pending validator change: {}", e))?;

        Ok(())
    }

    /// Get all pending validator changes for a specific epoch.
    pub fn get_pending_validator_changes(
        &self,
        epoch: u64,
    ) -> Result<Vec<crate::consensus::PendingValidatorChange>, String> {
        let cf = self
            .db
            .cf_handle(CF_PENDING_VALIDATOR_CHANGES)
            .ok_or_else(|| "Pending validator changes CF not found".to_string())?;

        let prefix = epoch.to_be_bytes();
        let iter = self.db.prefix_iterator_cf(&cf, prefix);
        let mut changes = Vec::new();

        for item in iter {
            let (key, value) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if key.len() < 8 || key[..8] != prefix {
                break;
            }
            let change: crate::consensus::PendingValidatorChange =
                serde_json::from_slice(&value)
                    .map_err(|e| format!("Failed to deserialize PendingValidatorChange: {}", e))?;
            changes.push(change);
        }

        Ok(changes)
    }

    /// Clear all pending validator changes for a specific epoch (after they've been applied).
    pub fn clear_pending_validator_changes(&self, epoch: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_PENDING_VALIDATOR_CHANGES)
            .ok_or_else(|| "Pending validator changes CF not found".to_string())?;

        let prefix = epoch.to_be_bytes();
        let keys: Vec<Box<[u8]>> = self
            .db
            .prefix_iterator_cf(&cf, prefix)
            .filter_map(|item| {
                let (key, _) = item.ok()?;
                if key.len() >= 8 && key[..8] == prefix {
                    Some(key)
                } else {
                    None
                }
            })
            .collect();

        if keys.is_empty() {
            return Ok(());
        }

        let mut batch = rocksdb::WriteBatch::default();
        for key in &keys {
            batch.delete_cf(&cf, key);
        }

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to clear pending validator changes: {}", e))?;

        Ok(())
    }

    /// Load stake pool from state (or initialize empty)
    pub fn get_stake_pool(&self) -> Result<crate::consensus::StakePool, String> {
        let cf = self
            .db
            .cf_handle(CF_STAKE_POOL)
            .ok_or_else(|| "Stake pool CF not found".to_string())?;

        match self.db.get_cf(&cf, b"pool") {
            Ok(Some(data)) => bincode::deserialize(&data)
                .map_err(|e| format!("Failed to deserialize stake pool: {}", e)),
            Ok(None) => Ok(crate::consensus::StakePool::new()),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Store stake pool
    pub fn put_stake_pool(&self, pool: &crate::consensus::StakePool) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STAKE_POOL)
            .ok_or_else(|| "Stake pool CF not found".to_string())?;

        let data = bincode::serialize(pool)
            .map_err(|e| format!("Failed to serialize stake pool: {}", e))?;

        self.db
            .put_cf(&cf, b"pool", data)
            .map_err(|e| format!("Failed to store stake pool: {}", e))
    }
}
