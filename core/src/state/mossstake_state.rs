use super::*;

impl StateStore {
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

        let data = serde_json::to_vec(pool)
            .map_err(|e| format!("Failed to serialize MossStake pool: {}", e))?;

        self.db
            .put_cf(&cf, b"pool", data)
            .map_err(|e| format!("Failed to store MossStake pool: {}", e))
    }
}
