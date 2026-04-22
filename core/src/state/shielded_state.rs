use super::*;

impl StateStore {
    /// Insert a note commitment into the shielded commitments column family.
    pub fn insert_shielded_commitment(
        &self,
        index: u64,
        commitment: &[u8; 32],
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_COMMITMENTS)
            .ok_or_else(|| "Shielded commitments CF not found".to_string())?;

        self.db
            .put_cf(&cf, index.to_be_bytes(), commitment)
            .map_err(|e| format!("Failed to insert shielded commitment: {}", e))
    }

    /// Retrieve a commitment leaf by its insertion index.
    pub fn get_shielded_commitment(&self, index: u64) -> Result<Option<[u8; 32]>, String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_COMMITMENTS)
            .ok_or_else(|| "Shielded commitments CF not found".to_string())?;

        match self.db.get_cf(&cf, index.to_be_bytes()) {
            Ok(Some(data)) => {
                if data.len() != 32 {
                    return Err(format!(
                        "Invalid commitment length {} at index {}",
                        data.len(),
                        index
                    ));
                }
                let mut out = [0u8; 32];
                out.copy_from_slice(&data);
                Ok(Some(out))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error reading commitment: {}", e)),
        }
    }

    /// Check whether a nullifier has been spent.
    pub fn is_nullifier_spent(&self, nullifier: &[u8; 32]) -> Result<bool, String> {
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

    /// Mark a nullifier as spent.
    pub fn mark_nullifier_spent(&self, nullifier: &[u8; 32]) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_NULLIFIERS)
            .ok_or_else(|| "Shielded nullifiers CF not found".to_string())?;

        self.db
            .put_cf(&cf, nullifier, [0x01])
            .map_err(|e| format!("Failed to mark nullifier spent: {}", e))
    }

    /// Load the singleton `ShieldedPoolState` from CF_SHIELDED_POOL.
    #[cfg(feature = "zk")]
    pub fn get_shielded_pool_state(&self) -> Result<crate::zk::ShieldedPoolState, String> {
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

    /// Persist the singleton `ShieldedPoolState` to CF_SHIELDED_POOL.
    #[cfg(feature = "zk")]
    pub fn put_shielded_pool_state(
        &self,
        state: &crate::zk::ShieldedPoolState,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_POOL)
            .ok_or_else(|| "Shielded pool CF not found".to_string())?;

        let data = serde_json::to_vec(state)
            .map_err(|e| format!("Failed to serialize ShieldedPoolState: {}", e))?;

        self.db
            .put_cf(&cf, b"state", &data)
            .map_err(|e| format!("Failed to store ShieldedPoolState: {}", e))
    }

    /// Collect all commitment leaves [0..count) from CF_SHIELDED_COMMITMENTS.
    pub fn get_all_shielded_commitments(&self, count: u64) -> Result<Vec<[u8; 32]>, String> {
        let mut leaves = Vec::with_capacity(count as usize);
        for index in 0..count {
            match self.get_shielded_commitment(index)? {
                Some(commitment) => leaves.push(commitment),
                None => {
                    return Err(format!(
                        "Missing shielded commitment at index {} (expected {})",
                        index, count
                    ))
                }
            }
        }
        Ok(leaves)
    }
}
