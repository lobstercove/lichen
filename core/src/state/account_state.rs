use super::*;

impl StateStore {
    /// Get account by pubkey.
    pub fn get_account(&self, pubkey: &Pubkey) -> Result<Option<Account>, String> {
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

    /// Batch account lookup (single RocksDB multi_get call).
    /// Returns only accounts that exist and decode successfully.
    pub fn get_accounts_batch(
        &self,
        pubkeys: &[Pubkey],
    ) -> Result<std::collections::HashMap<Pubkey, Account>, String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        let raw = self
            .db
            .multi_get_cf(pubkeys.iter().map(|pk| (&cf, pk.0.as_ref())));

        let mut out = std::collections::HashMap::with_capacity(pubkeys.len());
        for (pk, item) in pubkeys.iter().zip(raw.into_iter()) {
            let maybe_data = item.map_err(|e| format!("Database error: {}", e))?;
            let Some(data) = maybe_data else {
                continue;
            };

            let mut account: Account = if data.first() == Some(&0xBC) {
                bincode::deserialize(&data[1..])
                    .map_err(|e| format!("Failed to deserialize account (bincode): {}", e))?
            } else {
                serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to deserialize account (json): {}", e))?
            };
            account.fixup_legacy();
            out.insert(*pk, account);
        }

        Ok(out)
    }

    /// Store account.
    pub fn put_account(&self, pubkey: &Pubkey, account: &Account) -> Result<(), String> {
        self.put_account_with_hint(pubkey, account, None, None)
    }

    /// Store account with optional hints to skip the extra read.
    pub fn put_account_with_hint(
        &self,
        pubkey: &Pubkey,
        account: &Account,
        is_new_hint: Option<bool>,
        old_balance_hint: Option<u64>,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        let (is_new, old_balance) = match (is_new_hint, old_balance_hint) {
            (Some(new), Some(bal)) => (new, bal),
            _ => {
                let old_account = self
                    .db
                    .get_cf(&cf, pubkey.0)
                    .map_err(|e| format!("Failed to check account: {}", e))?;
                let old_bal = old_account
                    .as_ref()
                    .and_then(|data| {
                        if data.first() == Some(&0xBC) {
                            bincode::deserialize::<Account>(&data[1..]).ok()
                        } else {
                            serde_json::from_slice::<Account>(data).ok()
                        }
                    })
                    .map(|a| a.spores)
                    .unwrap_or(0);
                let new_flag = old_account.is_none();
                (
                    is_new_hint.unwrap_or(new_flag),
                    old_balance_hint.unwrap_or(old_bal),
                )
            }
        };
        let new_balance = account.spores;

        let mut value = Vec::with_capacity(256);
        value.push(0xBC);
        bincode::serialize_into(&mut value, account)
            .map_err(|e| format!("Failed to serialize account: {}", e))?;

        self.db
            .put_cf(&cf, pubkey.0, &value)
            .map_err(|e| format!("Failed to store account: {}", e))?;

        if self.is_archive_mode() {
            let slot = self.get_last_slot().unwrap_or(0);
            if slot > 0 {
                if let Some(snap_cf) = self.db.cf_handle(CF_ACCOUNT_SNAPSHOTS) {
                    let mut snap_key = [0u8; 40];
                    snap_key[..32].copy_from_slice(&pubkey.0);
                    snap_key[32..].copy_from_slice(&slot.to_be_bytes());
                    if let Err(e) = self.db.put_cf(&snap_cf, snap_key, &value) {
                        tracing::warn!("Failed to write archive snapshot: {}", e);
                    }
                }
            }
        }

        if is_new {
            self.metrics.increment_accounts();
        }
        if old_balance == 0 && new_balance > 0 {
            self.metrics.increment_active_accounts();
        } else if old_balance > 0 && new_balance == 0 {
            self.metrics.decrement_active_accounts();
        }

        self.mark_account_dirty_with_key(pubkey);

        Ok(())
    }

    /// Count total number of accounts (DEPRECATED - use metrics counter instead).
    pub fn count_accounts(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        let mut count = 0u64;
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        for _ in iter {
            count += 1;
        }

        Ok(count)
    }

    /// Get account balance in spores.
    pub fn get_balance(&self, pubkey: &Pubkey) -> Result<u64, String> {
        match self.get_account(pubkey)? {
            Some(account) => Ok(account.spores),
            None => Ok(0),
        }
    }

    /// Get reputation score for an account.
    pub fn get_reputation(&self, pubkey: &Pubkey) -> Result<u64, String> {
        let hex_chars: &[u8; 16] = b"0123456789abcdef";
        let mut rep_key = Vec::with_capacity(4 + 64);
        rep_key.extend_from_slice(b"rep:");
        for &b in pubkey.0.iter() {
            rep_key.push(hex_chars[(b >> 4) as usize]);
            rep_key.push(hex_chars[(b & 0x0f) as usize]);
        }
        Ok(self.get_program_storage_u64("lichenid", &rep_key))
    }

    /// Transfer spores between accounts.
    pub fn transfer(&self, from: &Pubkey, to: &Pubkey, spores: u64) -> Result<(), String> {
        if from == to {
            return Ok(());
        }

        let mut from_account = self
            .get_account(from)?
            .ok_or_else(|| "Sender account not found".to_string())?;

        from_account
            .deduct_spendable(spores)
            .map_err(|_| "Insufficient spendable balance".to_string())?;

        let existing = self.get_account(to)?;
        let to_existed = existing.is_some();
        let mut to_account = existing.unwrap_or_else(|| Account::new(0, *to));

        to_account.add_spendable(spores)?;

        if to_account.dormant {
            to_account.dormant = false;
            to_account.missed_rent_epochs = 0;
        }

        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;
        let mut batch = rocksdb::WriteBatch::default();
        let mut from_bytes = Vec::with_capacity(256);
        from_bytes.push(0xBC);
        bincode::serialize_into(&mut from_bytes, &from_account)
            .map_err(|e| format!("Serialize from: {}", e))?;
        let mut to_bytes = Vec::with_capacity(256);
        to_bytes.push(0xBC);
        bincode::serialize_into(&mut to_bytes, &to_account)
            .map_err(|e| format!("Serialize to: {}", e))?;
        batch.put_cf(&cf, from.0, &from_bytes);
        batch.put_cf(&cf, to.0, &to_bytes);
        self.db
            .write(batch)
            .map_err(|e| format!("Atomic transfer write failed: {}", e))?;

        self.mark_account_dirty_with_key(from);
        self.mark_account_dirty_with_key(to);

        if !to_existed {
            self.metrics.increment_accounts();
            self.metrics.increment_active_accounts();
        }

        Ok(())
    }

    /// Atomically persist multiple account mutations and an optional burn-counter increment.
    pub fn atomic_put_accounts(
        &self,
        accounts: &[(&Pubkey, &Account)],
        burn_delta: u64,
    ) -> Result<(), String> {
        if accounts.is_empty() && burn_delta == 0 {
            return Ok(());
        }

        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        let mut batch = WriteBatch::default();
        let mut meta: Vec<(&Pubkey, bool, u64, u64)> = Vec::with_capacity(accounts.len());

        for (pubkey, account) in accounts {
            let (is_new, old_balance) = {
                let old = self
                    .db
                    .get_cf(&cf, pubkey.0)
                    .map_err(|e| format!("Failed to read account: {}", e))?;
                let old_bal = old
                    .as_ref()
                    .and_then(|data| {
                        if data.first() == Some(&0xBC) {
                            bincode::deserialize::<Account>(&data[1..]).ok()
                        } else {
                            serde_json::from_slice::<Account>(data).ok()
                        }
                    })
                    .map(|a| a.spores)
                    .unwrap_or(0);
                (old.is_none(), old_bal)
            };

            let mut value = Vec::with_capacity(256);
            value.push(0xBC);
            bincode::serialize_into(&mut value, account)
                .map_err(|e| format!("Failed to serialize account: {}", e))?;
            batch.put_cf(&cf, pubkey.0, &value);
            meta.push((pubkey, is_new, old_balance, account.spores));
        }

        let _burned_guard = if burn_delta > 0 {
            let guard = self
                .burned_lock
                .lock()
                .map_err(|e| format!("burned_lock poisoned: {}", e))?;
            let cf_stats = self
                .db
                .cf_handle(CF_STATS)
                .ok_or_else(|| "Stats CF not found".to_string())?;
            let current_burned = self.get_total_burned()?;
            let new_total = current_burned.saturating_add(burn_delta);
            batch.put_cf(&cf_stats, b"total_burned", new_total.to_le_bytes());
            Some(guard)
        } else {
            None
        };

        self.db
            .write(batch)
            .map_err(|e| format!("Atomic account write failed: {}", e))?;

        for (pubkey, is_new, old_balance, new_balance) in meta {
            if is_new {
                self.metrics.increment_accounts();
            }
            if old_balance == 0 && new_balance > 0 {
                self.metrics.increment_active_accounts();
            } else if old_balance > 0 && new_balance == 0 {
                self.metrics.decrement_active_accounts();
            }
            self.mark_account_dirty_with_key(pubkey);
        }

        Ok(())
    }

    /// Atomically persist multiple account mutations and a mint-counter increment.
    pub fn atomic_mint_accounts(
        &self,
        accounts: &[(&Pubkey, &Account)],
        mint_delta: u64,
    ) -> Result<(), String> {
        if accounts.is_empty() && mint_delta == 0 {
            return Ok(());
        }

        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        let mut batch = WriteBatch::default();
        let mut meta: Vec<(&Pubkey, bool, u64, u64)> = Vec::with_capacity(accounts.len());

        for (pubkey, account) in accounts {
            let (is_new, old_balance) = {
                let old = self
                    .db
                    .get_cf(&cf, pubkey.0)
                    .map_err(|e| format!("Failed to read account: {}", e))?;
                let old_bal = old
                    .as_ref()
                    .and_then(|data| {
                        if data.first() == Some(&0xBC) {
                            bincode::deserialize::<Account>(&data[1..]).ok()
                        } else {
                            serde_json::from_slice::<Account>(data).ok()
                        }
                    })
                    .map(|a| a.spores)
                    .unwrap_or(0);
                (old.is_none(), old_bal)
            };

            let mut value = Vec::with_capacity(256);
            value.push(0xBC);
            bincode::serialize_into(&mut value, account)
                .map_err(|e| format!("Failed to serialize account: {}", e))?;
            batch.put_cf(&cf, pubkey.0, &value);
            meta.push((pubkey, is_new, old_balance, account.spores));
        }

        let _minted_guard = if mint_delta > 0 {
            let guard = self
                .minted_lock
                .lock()
                .map_err(|e| format!("minted_lock poisoned: {}", e))?;
            let cf_stats = self
                .db
                .cf_handle(CF_STATS)
                .ok_or_else(|| "Stats CF not found".to_string())?;
            let current_minted = self.get_total_minted()?;
            let new_total = current_minted.saturating_add(mint_delta);
            batch.put_cf(&cf_stats, b"total_minted", new_total.to_le_bytes());
            Some(guard)
        } else {
            None
        };

        self.db
            .write(batch)
            .map_err(|e| format!("Atomic mint account write failed: {}", e))?;

        for (pubkey, is_new, old_balance, new_balance) in meta {
            if is_new {
                self.metrics.increment_accounts();
            }
            if old_balance == 0 && new_balance > 0 {
                self.metrics.increment_active_accounts();
            } else if old_balance > 0 && new_balance == 0 {
                self.metrics.decrement_active_accounts();
            }
            self.mark_account_dirty_with_key(pubkey);
        }

        Ok(())
    }

    /// Atomically persist an account mutation together with a MossStake pool update.
    pub fn atomic_put_account_with_mossstake(
        &self,
        acct_key: &Pubkey,
        acct: &Account,
        pool: &MossStakePool,
    ) -> Result<(), String> {
        let cf_accounts = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;
        let cf_moss = self
            .db
            .cf_handle(CF_MOSSSTAKE)
            .ok_or_else(|| "MossStake CF not found".to_string())?;

        let (is_new, old_balance) = {
            let old = self
                .db
                .get_cf(&cf_accounts, acct_key.0)
                .map_err(|e| format!("Failed to read account: {}", e))?;
            let old_bal = old
                .as_ref()
                .and_then(|data| {
                    if data.first() == Some(&0xBC) {
                        bincode::deserialize::<Account>(&data[1..]).ok()
                    } else {
                        serde_json::from_slice::<Account>(data).ok()
                    }
                })
                .map(|a| a.spores)
                .unwrap_or(0);
            (old.is_none(), old_bal)
        };

        let mut batch = WriteBatch::default();

        let mut acct_bytes = Vec::with_capacity(256);
        acct_bytes.push(0xBC);
        bincode::serialize_into(&mut acct_bytes, acct)
            .map_err(|e| format!("Failed to serialize account: {}", e))?;
        batch.put_cf(&cf_accounts, acct_key.0, &acct_bytes);

        let pool_bytes = serde_json::to_vec(pool)
            .map_err(|e| format!("Failed to serialize MossStake pool: {}", e))?;
        batch.put_cf(&cf_moss, b"pool", &pool_bytes);

        self.db
            .write(batch)
            .map_err(|e| format!("Atomic account+mossstake write failed: {}", e))?;

        if is_new {
            self.metrics.increment_accounts();
        }
        let new_balance = acct.spores;
        if old_balance == 0 && new_balance > 0 {
            self.metrics.increment_active_accounts();
        } else if old_balance > 0 && new_balance == 0 {
            self.metrics.decrement_active_accounts();
        }
        self.mark_account_dirty_with_key(acct_key);

        Ok(())
    }

    /// Atomically update a MossStake pool and increment the mint counter.
    pub fn atomic_mint_mossstake(
        &self,
        pool: &MossStakePool,
        mint_delta: u64,
    ) -> Result<(), String> {
        let cf_moss = self
            .db
            .cf_handle(CF_MOSSSTAKE)
            .ok_or_else(|| "MossStake CF not found".to_string())?;

        let mut batch = WriteBatch::default();

        let pool_bytes = serde_json::to_vec(pool)
            .map_err(|e| format!("Failed to serialize MossStake pool: {}", e))?;
        batch.put_cf(&cf_moss, b"pool", &pool_bytes);

        let _minted_guard = if mint_delta > 0 {
            let guard = self
                .minted_lock
                .lock()
                .map_err(|e| format!("minted_lock poisoned: {}", e))?;
            let cf_stats = self
                .db
                .cf_handle(CF_STATS)
                .ok_or_else(|| "Stats CF not found".to_string())?;
            let current_minted = self.get_total_minted()?;
            let new_total = current_minted.saturating_add(mint_delta);
            batch.put_cf(&cf_stats, b"total_minted", new_total.to_le_bytes());
            Some(guard)
        } else {
            None
        };

        self.db
            .write(batch)
            .map_err(|e| format!("Atomic mint+mossstake write failed: {}", e))?;

        Ok(())
    }
}
