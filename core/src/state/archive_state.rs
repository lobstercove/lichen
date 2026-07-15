use super::*;
use crate::codec::{append_legacy_bincode, deserialize_legacy_bincode};

impl StateStore {
    /// Enable or disable archive mode. When enabled, every `put_account` also
    /// writes a snapshot to `CF_ACCOUNT_SNAPSHOTS` keyed by `pubkey(32) + slot(8,BE)`.
    pub fn set_archive_mode(&self, enabled: bool) {
        self.archive_mode
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    /// Check if archive mode is enabled.
    pub fn is_archive_mode(&self) -> bool {
        self.archive_mode.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Write a point-in-time snapshot of an account at the given slot.
    pub fn put_account_snapshot(
        &self,
        pubkey: &Pubkey,
        account: &Account,
        slot: u64,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNT_SNAPSHOTS)
            .ok_or_else(|| "Account snapshots CF not found".to_string())?;

        let mut key = [0u8; 40];
        key[..32].copy_from_slice(&pubkey.0);
        key[32..].copy_from_slice(&slot.to_be_bytes());

        let mut value = Vec::with_capacity(256);
        value.push(0xBC);
        append_legacy_bincode(&mut value, account, "account snapshot")
            .map_err(|e| format!("Failed to serialize snapshot: {}", e))?;

        self.db
            .put_cf(&cf, key, &value)
            .map_err(|e| format!("Failed to store account snapshot: {}", e))
    }

    /// Retrieve the state of an account at (or just before) the given slot.
    ///
    /// Uses `seek_for_prev` semantics: seeks to `pubkey + target_slot` and
    /// returns the entry at or before that key if the pubkey prefix matches.
    /// O(1) via a single RocksDB seek — no scanning required.
    pub fn get_account_at_slot(
        &self,
        pubkey: &Pubkey,
        target_slot: u64,
    ) -> Result<Option<Account>, String> {
        let mut seek_key = [0u8; 40];
        seek_key[..32].copy_from_slice(&pubkey.0);
        seek_key[32..].copy_from_slice(&target_slot.to_be_bytes());

        let hot = Self::get_account_snapshot_at_or_before(
            self.db.as_ref(),
            CF_ACCOUNT_SNAPSHOTS,
            pubkey,
            target_slot,
            &seek_key,
        )?;
        let cold = match self.cold_db.as_ref() {
            Some(db) if db.cf_handle(COLD_CF_ACCOUNT_SNAPSHOTS).is_some() => {
                Self::get_account_snapshot_at_or_before(
                    db.as_ref(),
                    COLD_CF_ACCOUNT_SNAPSHOTS,
                    pubkey,
                    target_slot,
                    &seek_key,
                )?
            }
            _ => None,
        };

        Ok(match (hot, cold) {
            (Some((hot_slot, hot_account)), Some((cold_slot, cold_account))) => {
                if hot_slot >= cold_slot {
                    Some(hot_account)
                } else {
                    Some(cold_account)
                }
            }
            (Some((_, account)), None) | (None, Some((_, account))) => Some(account),
            (None, None) => None,
        })
    }

    fn get_account_snapshot_at_or_before(
        db: &DB,
        cf_name: &str,
        pubkey: &Pubkey,
        target_slot: u64,
        seek_key: &[u8; 40],
    ) -> Result<Option<(u64, Account)>, String> {
        let cf = db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{} CF not found", cf_name))?;
        let iter = db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(seek_key, Direction::Reverse),
        );

        for item in iter {
            let (key, value) = item.map_err(|e| format!("Failed reading {}: {}", cf_name, e))?;
            if key.len() != 40 || key[..32] != pubkey.0 {
                break;
            }
            let slot = u64::from_be_bytes(
                key[32..40]
                    .try_into()
                    .map_err(|_| "Invalid account snapshot slot key".to_string())?,
            );
            if slot > target_slot {
                continue;
            }
            if value.first() != Some(&0xBC) {
                return Err(format!(
                    "Unsupported account snapshot encoding in {} at slot {}",
                    cf_name, slot
                ));
            }
            let mut account: Account = deserialize_legacy_bincode(&value[1..], "account snapshot")
                .map_err(|e| format!("Failed to deserialize snapshot: {}", e))?;
            account.fixup_legacy();
            return Ok(Some((slot, account)));
        }

        Ok(None)
    }

    /// Remove all account snapshots older than `before_slot`.
    /// Returns the number of entries pruned.
    pub fn prune_account_snapshots(&self, before_slot: u64) -> Result<u64, String> {
        if self.cold_db.is_some() {
            return Err(
                "Refusing to prune account snapshots from an archive-backed store; migrate them to cold storage instead"
                    .to_string(),
            );
        }
        let cf = self
            .db
            .cf_handle(CF_ACCOUNT_SNAPSHOTS)
            .ok_or_else(|| "Account snapshots CF not found".to_string())?;

        let mut batch = WriteBatch::default();
        let mut count = 0u64;
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);

        for item in iter.flatten() {
            let (key, _) = item;
            if key.len() != 40 {
                continue;
            }
            let mut slot_bytes = [0u8; 8];
            slot_bytes.copy_from_slice(&key[32..40]);
            let slot = u64::from_be_bytes(slot_bytes);
            if slot < before_slot {
                batch.delete_cf(&cf, &key);
                count += 1;
            }
        }

        if count > 0 {
            self.db
                .write(batch)
                .map_err(|e| format!("Snapshot prune failed: {}", e))?;
        }

        Ok(count)
    }

    /// Return the oldest slot that has at least one account snapshot, or `None`
    /// if the snapshot CF is empty.
    pub fn get_oldest_snapshot_slot(&self) -> Result<Option<u64>, String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNT_SNAPSHOTS)
            .ok_or_else(|| "Account snapshots CF not found".to_string())?;

        let mut oldest = None;
        for item in self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start) {
            let (key, _) = item.map_err(|e| format!("Failed reading account snapshots: {}", e))?;
            if key.len() == 40 {
                let slot = u64::from_be_bytes(key[32..40].try_into().unwrap());
                oldest = Some(oldest.map_or(slot, |current: u64| current.min(slot)));
            }
        }
        if let Some(cold) = self.cold_db.as_ref() {
            if let Some(cold_cf) = cold.cf_handle(COLD_CF_ACCOUNT_SNAPSHOTS) {
                for item in cold.iterator_cf(&cold_cf, rocksdb::IteratorMode::Start) {
                    let (key, _) =
                        item.map_err(|e| format!("Failed reading cold account snapshots: {}", e))?;
                    if key.len() == 40 {
                        let slot = u64::from_be_bytes(key[32..40].try_into().unwrap());
                        oldest = Some(oldest.map_or(slot, |current| current.min(slot)));
                    }
                }
            }
        }
        Ok(oldest)
    }
}
