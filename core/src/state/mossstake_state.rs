use super::*;

impl StateStore {
    pub fn is_mossstake_slot_only(&self) -> bool {
        matches!(
            self.get_metadata(crate::mossstake::MOSSSTAKE_SLOT_ONLY_METADATA_KEY)
                .ok()
                .flatten()
                .as_deref(),
            Some(b"1")
        )
    }

    pub fn mossstake_replay_mode_for_parent_slot(
        &self,
        parent_slot: u64,
    ) -> crate::mossstake::MossStakeReplayMode {
        if self.is_mossstake_slot_only() {
            return crate::mossstake::MossStakeReplayMode::SlotOnly;
        }

        let legacy_testnet = self
            .get_metadata(crate::signing::CHAIN_ID_METADATA_KEY)
            .ok()
            .flatten()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(|chain_id| chain_id.to_ascii_lowercase().contains("testnet"))
            .unwrap_or(false);
        if !legacy_testnet {
            return crate::mossstake::MossStakeReplayMode::SlotOnly;
        }

        if (crate::mossstake::LEGACY_TESTNET_MOSSSTAKE_WALL_CLOCK_START_PARENT_SLOT
            ..crate::mossstake::LEGACY_TESTNET_MOSSSTAKE_SLOT_ONLY_ACTIVATION_PARENT_SLOT)
            .contains(&parent_slot)
        {
            crate::mossstake::MossStakeReplayMode::LegacyWallClock
        } else {
            crate::mossstake::MossStakeReplayMode::SlotOnly
        }
    }

    pub fn mossstake_replay_mode(&self) -> crate::mossstake::MossStakeReplayMode {
        self.mossstake_replay_mode_for_parent_slot(self.get_last_slot().unwrap_or(0))
    }

    pub fn should_clear_mossstake_wall_clock_times(&self) -> bool {
        self.mossstake_replay_mode() == crate::mossstake::MossStakeReplayMode::SlotOnly
    }

    /// Update spendable balance for a native account.
    pub fn set_spendable_balance(&self, pubkey: &Pubkey, spores: u64) -> Result<(), String> {
        let mut account = self
            .get_account(pubkey)?
            .unwrap_or_else(|| Account::new(0, *pubkey));
        account.spendable = spores;
        account.spores = account
            .spendable
            .saturating_add(account.staked)
            .saturating_add(account.locked);
        self.put_account(pubkey, &account)
    }

    /// Get MossStake pool (creates if doesn't exist).
    pub fn get_mossstake_pool(&self) -> Result<MossStakePool, String> {
        let cf = self
            .db
            .cf_handle(CF_MOSSSTAKE)
            .ok_or_else(|| "MossStake CF not found".to_string())?;

        match self.db.get_cf(&cf, b"pool") {
            Ok(Some(data)) => serde_json::from_slice(&data)
                .map_err(|e| format!("Failed to deserialize MossStake pool: {}", e)),
            Ok(None) => Ok(MossStakePool::new()),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Store MossStake pool.
    pub fn put_mossstake_pool(&self, pool: &MossStakePool) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_MOSSSTAKE)
            .ok_or_else(|| "MossStake CF not found".to_string())?;

        let mut normalized_pool = pool.clone();
        if self.should_clear_mossstake_wall_clock_times() {
            normalized_pool.clear_wall_clock_times();
        }
        let data = serde_json::to_vec(&normalized_pool)
            .map_err(|e| format!("Failed to serialize MossStake pool: {}", e))?;

        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(&cf, b"pool", data);
        self.clear_composite_state_root_cache_in_batch(&mut batch);

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to store MossStake pool: {}", e))
    }
}
