use super::evm_state;
use super::*;

impl StateStore {
    // ─── Atomic Batch API (T1.4 / T3.1) ─────────────────────────────

    /// Begin an atomic write batch. All mutations go into the batch's in-memory
    /// `WriteBatch` and account overlay. Nothing touches disk until `commit_batch()`.
    pub fn begin_batch(&self) -> StateBatch {
        let archive_slot = if self.is_archive_mode() {
            self.get_last_slot().unwrap_or(0)
        } else {
            0
        };
        StateBatch {
            batch: WriteBatch::default(),
            account_overlay: std::collections::HashMap::new(),
            stake_pool_overlay: None,
            mossstake_pool_overlay: None,
            new_accounts: 0,
            active_account_delta: 0,
            burned_delta: 0,
            minted_delta: 0,
            nft_token_id_overlay: std::collections::HashSet::new(),
            symbol_overlay: std::collections::HashSet::new(),
            spent_nullifier_overlay: std::collections::HashSet::new(),
            shielded_commitment_overlay: std::collections::BTreeMap::new(),
            shielded_pool_overlay: None,
            governed_proposal_overlay: std::collections::HashMap::new(),
            governed_proposal_counter: None,
            governed_transfer_volume_overlay: std::collections::HashMap::new(),
            governance_proposal_overlay: std::collections::HashMap::new(),
            governance_proposal_counter: None,
            pending_governance_change_overlay: std::collections::HashMap::new(),
            contract_deploy_nonce_overlay: std::collections::HashMap::new(),
            new_programs: 0,
            event_seq: 0,
            dirty_contract_keys: Vec::new(),
            archive_slot,
            db: Arc::clone(&self.db),
        }
    }

    /// Commit a batch atomically. All puts in the `WriteBatch` are flushed to
    /// RocksDB in a single atomic write. Metric deltas are applied after the
    /// write succeeds.
    pub fn commit_batch(&self, batch: StateBatch) -> Result<(), String> {
        let dirty_pubkeys: Vec<Pubkey> = batch.account_overlay.keys().cloned().collect();
        let dirty_contract_keys: Vec<Vec<u8>> = batch.dirty_contract_keys.clone();

        let mut wb = batch.batch;
        let _burned_guard = if batch.burned_delta > 0 {
            let guard = self
                .burned_lock
                .lock()
                .map_err(|e| format!("burned_lock poisoned: {}", e))?;
            if let Some(cf) = self.db.cf_handle(CF_STATS) {
                let current = self.get_total_burned().unwrap_or(0);
                let new_total = current.saturating_add(batch.burned_delta);
                wb.put_cf(&cf, b"total_burned", new_total.to_le_bytes());
            }
            Some(guard)
        } else {
            None
        };

        let _minted_guard = if batch.minted_delta > 0 {
            let guard = self
                .minted_lock
                .lock()
                .map_err(|e| format!("minted_lock poisoned: {}", e))?;
            if let Some(cf) = self.db.cf_handle(CF_STATS) {
                let current = self.get_total_minted().unwrap_or(0);
                let new_total = current.saturating_add(batch.minted_delta);
                wb.put_cf(&cf, b"total_minted", new_total.to_le_bytes());
            }
            Some(guard)
        } else {
            None
        };

        self.db
            .write(wb)
            .map_err(|e| format!("Atomic batch commit failed: {}", e))?;

        if batch.new_accounts != 0 {
            for _ in 0..batch.new_accounts {
                self.metrics.increment_accounts();
            }
        }
        if batch.new_programs > 0 {
            for _ in 0..batch.new_programs {
                self.metrics.increment_programs();
            }
        }
        if batch.active_account_delta > 0 {
            for _ in 0..batch.active_account_delta {
                self.metrics.increment_active_accounts();
            }
        } else if batch.active_account_delta < 0 {
            for _ in 0..(-batch.active_account_delta) {
                self.metrics.decrement_active_accounts();
            }
        }
        self.metrics.save(&self.db)?;

        for pubkey in &dirty_pubkeys {
            self.mark_account_dirty_with_key(pubkey);
        }

        for key in &dirty_contract_keys {
            self.mark_contract_storage_dirty(key);
        }

        Ok(())
    }

    /// PERF-OPT 2: Save in-memory metrics counters to RocksDB.
    pub fn save_metrics_counters(&self) -> Result<(), String> {
        self.metrics.save(&self.db)
    }

    /// Backward-compatible alias for save_metrics_counters.
    #[deprecated(note = "Use save_metrics_counters() instead")]
    pub fn flush_metrics(&self) -> Result<(), String> {
        self.save_metrics_counters()
    }
}

impl StateBatch {
    /// B-7: Check symbol registry against both batch overlay and committed state.
    pub fn symbol_exists(&self, symbol: &str) -> Result<bool, String> {
        let normalized = StateStore::normalize_symbol(symbol)?;
        if self.symbol_overlay.contains(&normalized) {
            return Ok(true);
        }
        let cf = self
            .db
            .cf_handle(CF_SYMBOL_REGISTRY)
            .ok_or_else(|| "Symbol registry CF not found".to_string())?;
        let exists = self
            .db
            .get_cf(&cf, normalized.as_bytes())
            .map_err(|e| format!("Database error: {}", e))?
            .is_some();
        Ok(exists)
    }

    /// B-7: Get symbol registry entry from batch overlay or committed state.
    pub fn get_symbol_registry(&self, symbol: &str) -> Result<Option<SymbolRegistryEntry>, String> {
        let normalized = StateStore::normalize_symbol(symbol)?;
        let cf = self
            .db
            .cf_handle(CF_SYMBOL_REGISTRY)
            .ok_or_else(|| "Symbol registry CF not found".to_string())?;
        match self
            .db
            .get_cf(&cf, normalized.as_bytes())
            .map_err(|e| format!("Database error: {}", e))?
        {
            Some(data) => {
                let entry: SymbolRegistryEntry = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to decode symbol registry: {}", e))?;
                Ok(Some(entry))
            }
            None => Ok(None),
        }
    }

    pub fn add_burned(&mut self, amount: u64) {
        self.burned_delta = self.burned_delta.saturating_add(amount);
    }

    pub fn add_minted(&mut self, amount: u64) {
        self.minted_delta = self.minted_delta.saturating_add(amount);
    }

    /// H3 fix: Apply deferred EVM state changes atomically through this WriteBatch.
    pub fn apply_evm_changes(
        &mut self,
        changes: &[crate::evm::EvmStateChange],
    ) -> Result<(), String> {
        use rocksdb::Direction;

        let mut native_updates: Vec<(Pubkey, u64)> = Vec::new();

        {
            let cf_accounts = self
                .db
                .cf_handle(CF_EVM_ACCOUNTS)
                .ok_or_else(|| "EVM Accounts CF not found".to_string())?;
            let cf_storage = self
                .db
                .cf_handle(CF_EVM_STORAGE)
                .ok_or_else(|| "EVM Storage CF not found".to_string())?;

            for change in changes {
                if let Some(ref account) = change.account {
                    let data = bincode::serialize(account)
                        .map_err(|e| format!("Failed to serialize EVM account: {}", e))?;
                    self.batch.put_cf(&cf_accounts, change.evm_address, &data);
                } else {
                    self.batch.delete_cf(&cf_accounts, change.evm_address);

                    let prefix = &change.evm_address[..];
                    let iter = self.db.iterator_cf(
                        &cf_storage,
                        rocksdb::IteratorMode::From(prefix, Direction::Forward),
                    );
                    for item in iter.flatten() {
                        let (key, _) = item;
                        if !key.starts_with(prefix) {
                            break;
                        }
                        self.batch.delete_cf(&cf_storage, &key);
                    }
                }

                for (slot, value) in &change.storage_changes {
                    let mut key = Vec::with_capacity(52);
                    key.extend_from_slice(&change.evm_address);
                    key.extend_from_slice(slot);

                    if let Some(val) = value {
                        self.batch
                            .put_cf(&cf_storage, &key, val.to_be_bytes::<32>());
                    } else {
                        self.batch.delete_cf(&cf_storage, &key);
                    }
                }

                if let Some((pubkey, spores)) = change.native_balance_update {
                    native_updates.push((pubkey, spores));
                }
            }
        }

        for (pubkey, spores) in native_updates {
            let mut account = self
                .get_account(&pubkey)?
                .unwrap_or_else(|| Account::new(0, pubkey));
            account.spendable = spores;
            account.spores = account
                .spendable
                .saturating_add(account.staked)
                .saturating_add(account.locked);
            self.put_account(&pubkey, &account)?;
        }

        Ok(())
    }

    /// Get an account — checks the in-memory overlay first, then falls through to disk.
    pub fn get_account(&self, pubkey: &Pubkey) -> Result<Option<Account>, String> {
        if let Some(account) = self.account_overlay.get(pubkey) {
            return Ok(Some(account.clone()));
        }
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;
        match self.db.get_cf(&cf, pubkey.0) {
            Ok(Some(data)) => {
                let mut account: Account = if data.first() == Some(&0xBC) {
                    bincode::deserialize(&data[1..])
                        .map_err(|e| format!("Failed to deserialize account (bincode): {}", e))?
                } else {
                    serde_json::from_slice(&data)
                        .map_err(|e| format!("Failed to deserialize account (json): {}", e))?
                };
                account.fixup_legacy();
                Ok(Some(account))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Put an account into the batch (not written to disk until commit).
    pub fn put_account(&mut self, pubkey: &Pubkey, account: &Account) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        let old_balance = if let Some(existing) = self.account_overlay.get(pubkey) {
            Some(existing.spores)
        } else {
            match self.db.get_cf(&cf, pubkey.0) {
                Ok(Some(data)) => {
                    let acct = if data.first() == Some(&0xBC) {
                        bincode::deserialize::<Account>(&data[1..]).ok()
                    } else {
                        serde_json::from_slice::<Account>(&data).ok()
                    };
                    acct.map(|a| a.spores)
                }
                _ => None,
            }
        };

        let is_new = old_balance.is_none();
        let old_bal = old_balance.unwrap_or(0);
        let new_bal = account.spores;

        if is_new {
            self.new_accounts += 1;
        }
        if old_bal == 0 && new_bal > 0 {
            self.active_account_delta += 1;
        } else if old_bal > 0 && new_bal == 0 {
            self.active_account_delta -= 1;
        }

        let mut value = Vec::with_capacity(256);
        value.push(0xBC);
        bincode::serialize_into(&mut value, account)
            .map_err(|e| format!("Failed to serialize account: {}", e))?;

        self.batch.put_cf(&cf, pubkey.0, &value);
        self.account_overlay.insert(*pubkey, account.clone());

        if self.archive_slot > 0 {
            if let Some(snap_cf) = self.db.cf_handle(CF_ACCOUNT_SNAPSHOTS) {
                let mut snap_key = [0u8; 40];
                snap_key[..32].copy_from_slice(&pubkey.0);
                snap_key[32..].copy_from_slice(&self.archive_slot.to_be_bytes());
                self.batch.put_cf(&snap_cf, snap_key, &value);
            }
        }

        Ok(())
    }

    pub fn transfer(&mut self, from: &Pubkey, to: &Pubkey, spores: u64) -> Result<(), String> {
        if from == to {
            return Ok(());
        }

        let mut from_account = self
            .get_account(from)?
            .ok_or_else(|| "Sender account not found".to_string())?;
        from_account
            .deduct_spendable(spores)
            .map_err(|_| "Insufficient spendable balance".to_string())?;

        let mut to_account = self
            .get_account(to)?
            .unwrap_or_else(|| Account::new(0, *to));
        to_account.add_spendable(spores)?;

        if to_account.dormant {
            to_account.dormant = false;
            to_account.missed_rent_epochs = 0;
        }

        self.put_account(from, &from_account)?;
        self.put_account(to, &to_account)?;
        Ok(())
    }

    pub fn put_transaction(&mut self, tx: &Transaction) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;
        let sig = tx.signature();
        let mut value = Vec::with_capacity(512);
        value.push(0xBC);
        bincode::serialize_into(&mut value, tx)
            .map_err(|e| format!("Failed to serialize transaction: {}", e))?;
        self.batch.put_cf(&cf, sig.0, &value);
        Ok(())
    }

    pub fn put_tx_meta(&mut self, sig: &Hash, compute_units_used: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TX_META)
            .ok_or_else(|| "TX meta CF not found".to_string())?;
        self.batch
            .put_cf(&cf, sig.0, compute_units_used.to_le_bytes());
        Ok(())
    }

    pub fn put_tx_meta_full(
        &mut self,
        sig: &Hash,
        meta: &crate::processor::TxMeta,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TX_META)
            .ok_or_else(|| "TX meta CF not found".to_string())?;
        let data =
            bincode::serialize(meta).map_err(|e| format!("Failed to serialize tx meta: {}", e))?;
        self.batch.put_cf(&cf, sig.0, data);
        Ok(())
    }

    pub fn put_stake_pool(&mut self, pool: &crate::consensus::StakePool) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STAKE_POOL)
            .ok_or_else(|| "Stake pool CF not found".to_string())?;
        let data = bincode::serialize(pool)
            .map_err(|e| format!("Failed to serialize stake pool: {}", e))?;
        self.batch.put_cf(&cf, b"pool", &data);
        self.stake_pool_overlay = Some(pool.clone());
        Ok(())
    }

    pub fn get_stake_pool(&self) -> Result<crate::consensus::StakePool, String> {
        if let Some(pool) = &self.stake_pool_overlay {
            return Ok(pool.clone());
        }
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

    pub fn put_mossstake_pool(&mut self, pool: &MossStakePool) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_MOSSSTAKE)
            .ok_or_else(|| "MossStake CF not found".to_string())?;
        let data = serde_json::to_vec(pool)
            .map_err(|e| format!("Failed to serialize MossStake pool: {}", e))?;
        self.batch.put_cf(&cf, b"pool", &data);
        self.mossstake_pool_overlay = Some(pool.clone());
        Ok(())
    }

    pub fn get_mossstake_pool(&self) -> Result<MossStakePool, String> {
        if let Some(pool) = &self.mossstake_pool_overlay {
            return Ok(pool.clone());
        }
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

    pub fn set_fee_distribution_hash(&mut self, slot: u64, hash: &Hash) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("fee_dist:{}", slot);
        self.batch.put_cf(&cf, key.as_bytes(), hash.0);
        Ok(())
    }

    pub fn register_evm_address(
        &mut self,
        evm_address: &[u8; 20],
        native: &Pubkey,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_MAP)
            .ok_or_else(|| "EVM map CF not found".to_string())?;
        self.batch.put_cf(&cf, evm_address, native.0);
        let mut reverse_key = Vec::with_capacity(52);
        reverse_key.extend_from_slice(b"reverse:");
        reverse_key.extend_from_slice(&native.0);
        self.batch.put_cf(&cf, &reverse_key, evm_address);
        Ok(())
    }

    pub fn index_nft_mint(
        &mut self,
        collection: &Pubkey,
        token: &Pubkey,
        owner: &Pubkey,
    ) -> Result<(), String> {
        let cf_owner = self
            .db
            .cf_handle(CF_NFT_BY_OWNER)
            .ok_or_else(|| "NFT owner index CF not found".to_string())?;
        let mut key = Vec::with_capacity(64);
        key.extend_from_slice(&owner.0);
        key.extend_from_slice(&token.0);
        self.batch.put_cf(&cf_owner, &key, []);

        let cf_coll = self
            .db
            .cf_handle(CF_NFT_BY_COLLECTION)
            .ok_or_else(|| "NFT collection index CF not found".to_string())?;
        let mut ckey = Vec::with_capacity(64);
        ckey.extend_from_slice(&collection.0);
        ckey.extend_from_slice(&token.0);
        self.batch.put_cf(&cf_coll, &ckey, []);

        Ok(())
    }

    pub fn index_nft_token_id(
        &mut self,
        collection: &Pubkey,
        token_id: u64,
        token_account: &Pubkey,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_NFT_BY_COLLECTION)
            .ok_or_else(|| "NFT collection index CF not found".to_string())?;

        let mut key = Vec::with_capacity(44);
        key.extend_from_slice(b"tid:");
        key.extend_from_slice(&collection.0);
        key.extend_from_slice(&token_id.to_le_bytes());

        self.batch.put_cf(&cf, &key, token_account.0);
        self.nft_token_id_overlay.insert(key);
        Ok(())
    }

    pub fn nft_token_id_exists(&self, collection: &Pubkey, token_id: u64) -> Result<bool, String> {
        let mut key = Vec::with_capacity(44);
        key.extend_from_slice(b"tid:");
        key.extend_from_slice(&collection.0);
        key.extend_from_slice(&token_id.to_le_bytes());

        if self.nft_token_id_overlay.contains(&key) {
            return Ok(true);
        }

        let cf = self
            .db
            .cf_handle(CF_NFT_BY_COLLECTION)
            .ok_or_else(|| "NFT collection index CF not found".to_string())?;
        match self.db.get_cf(&cf, &key) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    pub fn index_nft_transfer(
        &mut self,
        collection: &Pubkey,
        token: &Pubkey,
        from: &Pubkey,
        to: &Pubkey,
    ) -> Result<(), String> {
        let cf_owner = self
            .db
            .cf_handle(CF_NFT_BY_OWNER)
            .ok_or_else(|| "NFT owner index CF not found".to_string())?;
        let mut old_key = Vec::with_capacity(64);
        old_key.extend_from_slice(&from.0);
        old_key.extend_from_slice(&token.0);
        self.batch.delete_cf(&cf_owner, &old_key);

        let mut new_key = Vec::with_capacity(64);
        new_key.extend_from_slice(&to.0);
        new_key.extend_from_slice(&token.0);
        self.batch.put_cf(&cf_owner, &new_key, []);

        let cf_coll = self
            .db
            .cf_handle(CF_NFT_BY_COLLECTION)
            .ok_or_else(|| "NFT collection index CF not found".to_string())?;
        let mut ckey = Vec::with_capacity(64);
        ckey.extend_from_slice(&collection.0);
        ckey.extend_from_slice(&token.0);
        self.batch.put_cf(&cf_coll, &ckey, []);

        Ok(())
    }

    pub fn put_contract_event(
        &mut self,
        program: &Pubkey,
        event: &ContractEvent,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVENTS)
            .ok_or_else(|| "Events CF not found".to_string())?;
        let seq = self.event_seq;
        self.event_seq += 1;
        let mut key = Vec::with_capacity(56);
        key.extend_from_slice(&program.0);
        key.extend_from_slice(&event.slot.to_be_bytes());
        let name_hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            event.name.hash(&mut h);
            h.finish()
        };
        key.extend_from_slice(&name_hash.to_be_bytes());
        key.extend_from_slice(&seq.to_be_bytes());
        let value =
            serde_json::to_vec(event).map_err(|e| format!("Failed to serialize event: {}", e))?;
        self.batch.put_cf(&cf, &key, &value);

        if let Some(cf_slot) = self.db.cf_handle(CF_EVENTS_BY_SLOT) {
            let mut slot_key = Vec::with_capacity(8 + 32 + 8);
            slot_key.extend_from_slice(&event.slot.to_be_bytes());
            slot_key.extend_from_slice(&program.0);
            slot_key.extend_from_slice(&seq.to_be_bytes());
            self.batch.put_cf(&cf_slot, &slot_key, &key);
        }

        Ok(())
    }

    pub fn put_contract_storage(
        &mut self,
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
        self.batch.put_cf(&cf, &key, value);
        self.dirty_contract_keys.push(key);
        Ok(())
    }

    pub fn delete_contract_storage(
        &mut self,
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
        self.batch.delete_cf(&cf, &key);
        self.dirty_contract_keys.push(key);
        Ok(())
    }

    pub fn update_token_balance(
        &mut self,
        token_program: &Pubkey,
        holder: &Pubkey,
        balance: u64,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TOKEN_BALANCES)
            .ok_or_else(|| "Token balances CF not found".to_string())?;
        let rev_cf = self
            .db
            .cf_handle(CF_HOLDER_TOKENS)
            .ok_or_else(|| "Holder tokens CF not found".to_string())?;
        let solana_cf = self
            .db
            .cf_handle(CF_SOLANA_TOKEN_ACCOUNTS)
            .ok_or_else(|| "Solana token accounts CF not found".to_string())?;
        let solana_holder_cf = self
            .db
            .cf_handle(CF_SOLANA_HOLDER_TOKEN_ACCOUNTS)
            .ok_or_else(|| "Solana holder token accounts CF not found".to_string())?;

        let mut key = Vec::with_capacity(64);
        key.extend_from_slice(&token_program.0);
        key.extend_from_slice(&holder.0);

        let mut rev_key = Vec::with_capacity(64);
        rev_key.extend_from_slice(&holder.0);
        rev_key.extend_from_slice(&token_program.0);

        let token_account = derive_solana_associated_token_address(holder, token_program)?;
        let binding = solana_token_account_binding_bytes(token_program, holder);
        let holder_key = solana_holder_token_account_key(holder, &token_account);

        if balance == 0 {
            self.batch.delete_cf(&cf, &key);
            self.batch.delete_cf(&rev_cf, &rev_key);
            self.batch.put_cf(solana_cf, token_account.0, binding);
            self.batch
                .put_cf(solana_holder_cf, holder_key, token_program.0);
        } else {
            self.batch.put_cf(&cf, &key, balance.to_le_bytes());
            self.batch.put_cf(&rev_cf, &rev_key, balance.to_le_bytes());
            self.batch.put_cf(solana_cf, token_account.0, binding);
            self.batch
                .put_cf(solana_holder_cf, holder_key, token_program.0);
        }
        Ok(())
    }

    pub fn put_evm_tx(&mut self, record: &crate::evm::EvmTxRecord) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_TXS)
            .ok_or_else(|| "EVM txs CF not found".to_string())?;
        let key = record.evm_hash.as_slice();
        let value =
            bincode::serialize(record).map_err(|e| format!("Failed to serialize EVM tx: {}", e))?;
        self.batch.put_cf(&cf, key, &value);
        Ok(())
    }

    pub fn put_evm_receipt(&mut self, receipt: &crate::evm::EvmReceipt) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_RECEIPTS)
            .ok_or_else(|| "EVM receipts CF not found".to_string())?;
        let key = receipt.evm_hash.as_slice();
        let value = evm_state::serialize_evm_receipt_for_storage(receipt)?;
        self.batch.put_cf(&cf, key, &value);
        Ok(())
    }

    pub fn put_evm_logs_for_slot(
        &mut self,
        slot: u64,
        logs: &[crate::evm::EvmLogEntry],
    ) -> Result<(), String> {
        if logs.is_empty() {
            return Ok(());
        }
        let cf = self
            .db
            .cf_handle(CF_EVM_LOGS_BY_SLOT)
            .ok_or_else(|| "EVM Logs CF not found".to_string())?;
        let key = slot.to_be_bytes();
        let mut existing: Vec<crate::evm::EvmLogEntry> = match self.db.get_cf(&cf, key) {
            Ok(Some(data)) => bincode::deserialize(&data).unwrap_or_default(),
            _ => Vec::new(),
        };
        existing.extend_from_slice(logs);
        let data = bincode::serialize(&existing)
            .map_err(|e| format!("Failed to serialize EVM logs: {}", e))?;
        self.batch.put_cf(&cf, key, &data);
        Ok(())
    }

    pub fn index_program(&mut self, program: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAMS)
            .ok_or_else(|| "Programs CF not found".to_string())?;
        let is_new = self.db.get_cf(&cf, program.0).ok().flatten().is_none();
        self.batch.put_cf(&cf, program.0, []);
        if is_new {
            self.new_programs += 1;
        }
        Ok(())
    }

    pub fn register_symbol(
        &mut self,
        symbol: &str,
        entry: &crate::state::SymbolRegistryEntry,
    ) -> Result<(), String> {
        let normalized = StateStore::normalize_symbol(symbol)?;
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
        if self.symbol_overlay.contains(&normalized) {
            return Err(format!(
                "Symbol already registered in this batch: {}",
                normalized
            ));
        }
        let mut entry_copy = entry.clone();
        entry_copy.symbol = normalized.clone();
        let data = serde_json::to_vec(&entry_copy)
            .map_err(|e| format!("Failed to encode symbol registry: {}", e))?;
        self.batch.put_cf(&cf, normalized.as_bytes(), &data);
        self.symbol_overlay.insert(normalized.clone());

        if let Some(cf_rev) = self.db.cf_handle(CF_SYMBOL_BY_PROGRAM) {
            self.batch
                .put_cf(&cf_rev, entry.program.0, normalized.as_bytes());
        }

        Ok(())
    }

    /// Allocate the next governed proposal ID through the batch.
    pub fn next_governed_proposal_id(&mut self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let current = if let Some(c) = self.governed_proposal_counter {
            c
        } else {
            match self.db.get_cf(&cf, b"governed_proposal_counter") {
                Ok(Some(data)) if data.len() == 8 => {
                    u64::from_le_bytes(data[..8].try_into().unwrap())
                }
                _ => 0,
            }
        };
        let next = current + 1;
        self.governed_proposal_counter = Some(next);
        self.batch
            .put_cf(&cf, b"governed_proposal_counter", next.to_le_bytes());
        Ok(next)
    }

    pub fn set_governed_proposal(
        &mut self,
        proposal: &crate::multisig::GovernedProposal,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("governed_proposal:{}", proposal.id);
        let data = serde_json::to_vec(proposal)
            .map_err(|e| format!("Failed to serialize governed proposal: {}", e))?;
        self.batch.put_cf(&cf, key.as_bytes(), &data);
        self.governed_proposal_overlay
            .insert(proposal.id, proposal.clone());
        Ok(())
    }

    pub fn get_governed_proposal(
        &self,
        id: u64,
    ) -> Result<Option<crate::multisig::GovernedProposal>, String> {
        if let Some(p) = self.governed_proposal_overlay.get(&id) {
            return Ok(Some(p.clone()));
        }
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("governed_proposal:{}", id);
        match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) => {
                let proposal: crate::multisig::GovernedProposal = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to deserialize proposal: {}", e))?;
                Ok(Some(proposal))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("DB error loading governed proposal: {}", e)),
        }
    }

    pub fn get_governed_transfer_day_volume(
        &self,
        wallet_pubkey: &Pubkey,
        day_start: u64,
    ) -> Result<u64, String> {
        let key = format!(
            "governed_transfer_volume:{}:{}",
            wallet_pubkey.to_base58(),
            day_start
        );
        if let Some(volume) = self.governed_transfer_volume_overlay.get(&key) {
            return Ok(*volume);
        }

        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) if data.len() == 8 => {
                Ok(u64::from_le_bytes(data[..8].try_into().unwrap()))
            }
            Ok(Some(_)) | Ok(None) => Ok(0),
            Err(e) => Err(format!("DB error loading governed transfer volume: {}", e)),
        }
    }

    pub fn set_governed_transfer_day_volume(
        &mut self,
        wallet_pubkey: &Pubkey,
        day_start: u64,
        volume: u64,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!(
            "governed_transfer_volume:{}:{}",
            wallet_pubkey.to_base58(),
            day_start
        );
        self.batch.put_cf(&cf, key.as_bytes(), volume.to_le_bytes());
        self.governed_transfer_volume_overlay.insert(key, volume);
        Ok(())
    }

    pub fn queue_governance_param_change(
        &mut self,
        param_id: u8,
        value: u64,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("pending_gov_{}", param_id);
        self.batch.put_cf(&cf, key.as_bytes(), value.to_le_bytes());
        self.pending_governance_change_overlay
            .insert(param_id, value);
        Ok(())
    }

    pub fn get_pending_governance_changes(&self) -> Result<Vec<(u8, u64)>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let mut changes = Vec::new();
        for param_id in 0..=7u8 {
            if let Some(value) = self.pending_governance_change_overlay.get(&param_id) {
                changes.push((param_id, *value));
                continue;
            }

            let key = format!("pending_gov_{}", param_id);
            if let Ok(Some(data)) = self.db.get_cf(&cf, key.as_bytes()) {
                if data.len() == 8 {
                    let bytes: [u8; 8] = data.as_slice().try_into().unwrap_or([0; 8]);
                    changes.push((param_id, u64::from_le_bytes(bytes)));
                }
            }
        }
        Ok(changes)
    }

    pub fn next_contract_deploy_nonce(&mut self, deployer: &Pubkey) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("contract_deploy_nonce:{}", deployer.to_base58());
        let current = if let Some(next_nonce) = self.contract_deploy_nonce_overlay.get(deployer) {
            *next_nonce
        } else {
            match self.db.get_cf(&cf, key.as_bytes()) {
                Ok(Some(data)) if data.len() == 8 => {
                    u64::from_le_bytes(data.as_slice().try_into().unwrap_or([0; 8]))
                }
                Ok(_) => 0,
                Err(e) => return Err(format!("Database error loading deploy nonce: {}", e)),
            }
        };

        let next = current
            .checked_add(1)
            .ok_or_else(|| "Contract deploy nonce overflow".to_string())?;
        self.contract_deploy_nonce_overlay.insert(*deployer, next);
        self.batch.put_cf(&cf, key.as_bytes(), next.to_le_bytes());
        Ok(current)
    }

    pub fn next_governance_proposal_id(&mut self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let current = if let Some(c) = self.governance_proposal_counter {
            c
        } else {
            match self.db.get_cf(&cf, b"governance_proposal_counter") {
                Ok(Some(data)) if data.len() == 8 => {
                    u64::from_le_bytes(data[..8].try_into().unwrap())
                }
                _ => 0,
            }
        };
        let next = current + 1;
        self.governance_proposal_counter = Some(next);
        self.batch
            .put_cf(&cf, b"governance_proposal_counter", next.to_le_bytes());
        Ok(next)
    }

    pub fn set_governance_proposal(
        &mut self,
        proposal: &crate::governance::GovernanceProposal,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("governance_proposal:{}", proposal.id);
        let data = serde_json::to_vec(proposal)
            .map_err(|e| format!("Failed to serialize governance proposal: {}", e))?;
        self.batch.put_cf(&cf, key.as_bytes(), &data);
        self.governance_proposal_overlay
            .insert(proposal.id, proposal.clone());
        Ok(())
    }

    pub fn get_governance_proposal(
        &self,
        id: u64,
    ) -> Result<Option<crate::governance::GovernanceProposal>, String> {
        if let Some(proposal) = self.governance_proposal_overlay.get(&id) {
            return Ok(Some(proposal.clone()));
        }
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("governance_proposal:{}", id);
        match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) => {
                let proposal: crate::governance::GovernanceProposal = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to deserialize governance proposal: {}", e))?;
                Ok(Some(proposal))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("DB error loading governance proposal: {}", e)),
        }
    }

    /// Read-only: get last slot (falls through to disk since batches don't modify this).
    pub fn get_last_slot(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        match self.db.get_cf(&cf, b"last_slot") {
            Ok(Some(data)) if data.len() == 8 => {
                Ok(u64::from_be_bytes(data.as_slice().try_into().unwrap()))
            }
            Ok(_) => Ok(0),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Insert a shielded commitment into the WriteBatch.
    pub fn insert_shielded_commitment(
        &mut self,
        index: u64,
        commitment: &[u8; 32],
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_COMMITMENTS)
            .ok_or_else(|| "Shielded commitments CF not found".to_string())?;
        self.batch.put_cf(&cf, index.to_be_bytes(), commitment);
        self.shielded_commitment_overlay.insert(index, *commitment);
        Ok(())
    }

    /// Collect all commitment leaves [0..count), including any uncommitted inserts.
    pub fn get_all_shielded_commitments(&self, count: u64) -> Result<Vec<[u8; 32]>, String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_COMMITMENTS)
            .ok_or_else(|| "Shielded commitments CF not found".to_string())?;
        let mut out = Vec::with_capacity(count as usize);

        for i in 0..count {
            if let Some(commitment) = self.shielded_commitment_overlay.get(&i) {
                out.push(*commitment);
                continue;
            }

            match self.db.get_cf(&cf, i.to_be_bytes()) {
                Ok(Some(bytes)) if bytes.len() == 32 => {
                    let mut leaf = [0u8; 32];
                    leaf.copy_from_slice(&bytes);
                    out.push(leaf);
                }
                Ok(Some(_)) => {
                    return Err(format!(
                        "Shielded commitments entry {} had invalid length",
                        i
                    ));
                }
                Ok(None) => {}
                Err(e) => {
                    return Err(format!(
                        "Database error loading shielded commitment {}: {}",
                        i, e
                    ));
                }
            }
        }

        Ok(out)
    }

    /// Check whether a nullifier has been spent.
    pub fn is_nullifier_spent(&self, nullifier: &[u8; 32]) -> Result<bool, String> {
        if self.spent_nullifier_overlay.contains(nullifier) {
            return Ok(true);
        }

        let cf = self
            .db
            .cf_handle(CF_SHIELDED_NULLIFIERS)
            .ok_or_else(|| "Shielded nullifiers CF not found".to_string())?;
        match self.db.get_cf(&cf, nullifier) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(format!("Database error checking nullifier: {}", e)),
        }
    }

    /// Mark a nullifier as spent in the WriteBatch.
    pub fn mark_nullifier_spent(&mut self, nullifier: &[u8; 32]) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_NULLIFIERS)
            .ok_or_else(|| "Shielded nullifiers CF not found".to_string())?;
        self.batch.put_cf(&cf, nullifier, [0x01]);
        self.spent_nullifier_overlay.insert(*nullifier);
        Ok(())
    }

    /// Load the singleton `ShieldedPoolState` from disk.
    #[cfg(feature = "zk")]
    pub fn get_shielded_pool_state(&self) -> Result<crate::zk::ShieldedPoolState, String> {
        if let Some(pool) = &self.shielded_pool_overlay {
            return Ok(pool.clone());
        }

        let cf = self
            .db
            .cf_handle(CF_SHIELDED_POOL)
            .ok_or_else(|| "Shielded pool CF not found".to_string())?;
        match self.db.get_cf(&cf, b"state") {
            Ok(Some(data)) => serde_json::from_slice(&data)
                .map_err(|e| format!("Failed to deserialize ShieldedPoolState: {}", e)),
            Ok(None) => Ok(crate::zk::ShieldedPoolState::default()),
            Err(e) => Err(format!("Database error reading shielded pool state: {}", e)),
        }
    }

    /// Write the singleton `ShieldedPoolState` to the WriteBatch.
    #[cfg(feature = "zk")]
    pub fn put_shielded_pool_state(
        &mut self,
        state: &crate::zk::ShieldedPoolState,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_POOL)
            .ok_or_else(|| "Shielded pool CF not found".to_string())?;
        let data = serde_json::to_vec(state)
            .map_err(|e| format!("Failed to serialize ShieldedPoolState: {}", e))?;
        self.batch.put_cf(&cf, b"state", &data);
        self.shielded_pool_overlay = Some(state.clone());
        Ok(())
    }
}
