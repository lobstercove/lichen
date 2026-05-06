use std::collections::BTreeSet;

use rocksdb::{Direction, IteratorMode, DB};

use super::*;
use crate::restrictions::{
    restriction_mode_blocks_transfer, ContractRestrictionAccess, EffectiveRestrictionRecord,
    ProtocolModuleId, RestrictionMode, RestrictionRecord, RestrictionTarget,
    RestrictionTransferDirection,
};

const RESTRICTION_COUNTER_KEY: &[u8] = b"restriction_counter";
const CACHED_STATE_ROOT_KEY: &[u8] = b"cached_state_root";
const CACHED_STATE_ROOT_SCHEMA_KEY: &[u8] = b"cached_state_root_schema";

fn restriction_id_key(id: u64) -> [u8; 8] {
    id.to_be_bytes()
}

fn restriction_id_from_key(key: &[u8]) -> Option<u64> {
    if key.len() != 8 {
        return None;
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(key);
    Some(u64::from_be_bytes(bytes))
}

fn encode_restriction(record: &RestrictionRecord) -> Result<Vec<u8>, String> {
    bincode::serialize(record).map_err(|e| format!("Failed to serialize restriction: {}", e))
}

fn decode_restriction(data: &[u8]) -> Result<RestrictionRecord, String> {
    bincode::deserialize(data).map_err(|e| format!("Failed to deserialize restriction: {}", e))
}

fn get_counter(db: &DB) -> Result<u64, String> {
    let cf = db
        .cf_handle(CF_STATS)
        .ok_or_else(|| "Stats CF not found".to_string())?;
    match db.get_cf(&cf, RESTRICTION_COUNTER_KEY) {
        Ok(Some(data)) if data.len() == 8 => Ok(u64::from_le_bytes(
            data.as_slice().try_into().unwrap_or([0; 8]),
        )),
        Ok(_) => Ok(0),
        Err(e) => Err(format!("Database error loading restriction counter: {}", e)),
    }
}

fn get_restriction_from_db(db: &DB, id: u64) -> Result<Option<RestrictionRecord>, String> {
    let cf = db
        .cf_handle(CF_RESTRICTIONS)
        .ok_or_else(|| "Restrictions CF not found".to_string())?;
    match db.get_cf(&cf, restriction_id_key(id)) {
        Ok(Some(data)) => Ok(Some(decode_restriction(&data)?)),
        Ok(None) => Ok(None),
        Err(e) => Err(format!("Database error loading restriction {}: {}", id, e)),
    }
}

fn index_key(prefix: &[u8], id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(prefix.len() + 8);
    key.extend_from_slice(prefix);
    key.extend_from_slice(&restriction_id_key(id));
    key
}

fn ids_from_index(db: &DB, cf_name: &str, prefix: &[u8]) -> Result<BTreeSet<u64>, String> {
    let cf = db
        .cf_handle(cf_name)
        .ok_or_else(|| format!("{} CF not found", cf_name))?;
    let iter = db.iterator_cf(&cf, IteratorMode::From(prefix, Direction::Forward));
    let mut ids = BTreeSet::new();
    for item in iter.flatten() {
        let (key, _) = item;
        if !key.starts_with(prefix) {
            break;
        }
        if key.len() == prefix.len() + 8 {
            if let Some(id) = restriction_id_from_key(&key[prefix.len()..]) {
                ids.insert(id);
            }
        }
    }
    Ok(ids)
}

fn all_restrictions_from_db(db: &DB) -> Result<Vec<RestrictionRecord>, String> {
    let cf = db
        .cf_handle(CF_RESTRICTIONS)
        .ok_or_else(|| "Restrictions CF not found".to_string())?;
    let iter = db.iterator_cf(&cf, IteratorMode::Start);
    let mut records = Vec::new();
    for item in iter.flatten() {
        let (_, value) = item;
        records.push(decode_restriction(&value)?);
    }
    records.sort_by_key(|record| record.id);
    Ok(records)
}

fn target_ids_from_db(db: &DB, target: &RestrictionTarget) -> Result<BTreeSet<u64>, String> {
    ids_from_index(db, CF_RESTRICTION_INDEX_TARGET, &target.canonical_key())
}

fn code_hash_ids_from_db(db: &DB, code_hash: &Hash) -> Result<BTreeSet<u64>, String> {
    ids_from_index(db, CF_RESTRICTION_INDEX_CODE_HASH, &code_hash.0)
}

fn validate_same_target_on_update(
    existing: Option<RestrictionRecord>,
    record: &RestrictionRecord,
) -> Result<(), String> {
    if let Some(existing) = existing {
        if existing.target != record.target {
            return Err(format!(
                "Restriction {} target cannot change after creation",
                record.id
            ));
        }
    }
    Ok(())
}

fn invalidate_cached_state_root(db: &DB, batch: &mut WriteBatch) {
    if let Some(cf_stats) = db.cf_handle(CF_STATS) {
        batch.delete_cf(&cf_stats, CACHED_STATE_ROOT_KEY);
        batch.delete_cf(&cf_stats, CACHED_STATE_ROOT_SCHEMA_KEY);
    }
}

fn active_effective_records_for_ids<F>(
    ids: BTreeSet<u64>,
    mut get_record: F,
    slot: u64,
    limit: usize,
) -> Result<Vec<RestrictionRecord>, String>
where
    F: FnMut(u64) -> Result<Option<RestrictionRecord>, String>,
{
    let mut records = Vec::new();
    for id in ids {
        if let Some(record) = get_record(id)? {
            if record.is_effectively_active(slot) {
                records.push(record);
                if limit > 0 && records.len() >= limit {
                    break;
                }
            }
        }
    }
    Ok(records)
}

fn account_record_blocks(
    record: &RestrictionRecord,
    direction: RestrictionTransferDirection,
    amount: u64,
    spendable: u64,
) -> bool {
    restriction_mode_blocks_transfer(&record.mode, direction, amount, spendable)
}

fn contract_record_blocks(record: &RestrictionRecord, access: ContractRestrictionAccess) -> bool {
    match record.mode {
        RestrictionMode::ExecuteBlocked
        | RestrictionMode::Quarantined
        | RestrictionMode::Terminated => true,
        RestrictionMode::StateChangingBlocked => access == ContractRestrictionAccess::StateChanging,
        _ => false,
    }
}

impl StateStore {
    pub fn next_restriction_id(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let current = get_counter(&self.db)?;
        let next = current
            .checked_add(1)
            .ok_or_else(|| "Restriction ID counter overflow".to_string())?;
        self.db
            .put_cf(&cf, RESTRICTION_COUNTER_KEY, next.to_le_bytes())
            .map_err(|e| format!("Failed to update restriction counter: {}", e))?;
        Ok(next)
    }

    pub fn put_restriction(&self, record: &RestrictionRecord) -> Result<(), String> {
        record.validate()?;
        validate_same_target_on_update(self.get_restriction(record.id)?, record)?;

        let cf_records = self
            .db
            .cf_handle(CF_RESTRICTIONS)
            .ok_or_else(|| "Restrictions CF not found".to_string())?;
        let cf_target = self
            .db
            .cf_handle(CF_RESTRICTION_INDEX_TARGET)
            .ok_or_else(|| "Restriction target index CF not found".to_string())?;

        let mut batch = WriteBatch::default();
        let data = encode_restriction(record)?;
        batch.put_cf(&cf_records, restriction_id_key(record.id), &data);
        invalidate_cached_state_root(&self.db, &mut batch);
        batch.put_cf(
            &cf_target,
            index_key(&record.target.canonical_key(), record.id),
            [],
        );

        if let Some(code_hash) = record.target.code_hash() {
            let cf_code = self
                .db
                .cf_handle(CF_RESTRICTION_INDEX_CODE_HASH)
                .ok_or_else(|| "Restriction code-hash index CF not found".to_string())?;
            batch.put_cf(&cf_code, index_key(&code_hash.0, record.id), []);
        }

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to store restriction {}: {}", record.id, e))
    }

    pub fn get_restriction(&self, id: u64) -> Result<Option<RestrictionRecord>, String> {
        get_restriction_from_db(&self.db, id)
    }

    pub fn get_effective_restriction_record(
        &self,
        id: u64,
        slot: u64,
    ) -> Result<Option<EffectiveRestrictionRecord>, String> {
        Ok(self
            .get_restriction(id)?
            .map(|record| EffectiveRestrictionRecord::new(record, slot)))
    }

    pub fn list_effective_restrictions_by_target(
        &self,
        target: &RestrictionTarget,
        slot: u64,
        limit: usize,
    ) -> Result<Vec<EffectiveRestrictionRecord>, String> {
        let mut records = Vec::new();
        for id in target_ids_from_db(&self.db, target)? {
            if let Some(record) = self.get_restriction(id)? {
                records.push(EffectiveRestrictionRecord::new(record, slot));
                if limit > 0 && records.len() >= limit {
                    break;
                }
            }
        }
        Ok(records)
    }

    pub fn get_active_restrictions_for_target(
        &self,
        target: &RestrictionTarget,
        slot: u64,
        limit: usize,
    ) -> Result<Vec<RestrictionRecord>, String> {
        let ids = target_ids_from_db(&self.db, target)?;
        active_effective_records_for_ids(ids, |id| self.get_restriction(id), slot, limit)
    }

    pub fn list_active_restrictions(
        &self,
        slot: u64,
        limit: usize,
    ) -> Result<Vec<RestrictionRecord>, String> {
        let mut records = Vec::new();
        for record in all_restrictions_from_db(&self.db)? {
            if record.is_effectively_active(slot) {
                records.push(record);
                if limit > 0 && records.len() >= limit {
                    break;
                }
            }
        }
        Ok(records)
    }

    pub fn is_account_restricted(
        &self,
        account: &Pubkey,
        direction: RestrictionTransferDirection,
        asset: Option<&Pubkey>,
        amount: u64,
        spendable: u64,
        slot: u64,
    ) -> Result<bool, String> {
        let account_target = RestrictionTarget::Account(*account);
        for record in self.get_active_restrictions_for_target(&account_target, slot, 0)? {
            if account_record_blocks(&record, direction, amount, spendable) {
                return Ok(true);
            }
        }

        if let Some(asset) = asset {
            let account_asset_target = RestrictionTarget::AccountAsset {
                account: *account,
                asset: *asset,
            };
            for record in self.get_active_restrictions_for_target(&account_asset_target, slot, 0)? {
                if account_record_blocks(&record, direction, amount, spendable) {
                    return Ok(true);
                }
            }
            if self.is_asset_restricted(asset, slot)? {
                return Ok(true);
            }
        }

        Ok(false)
    }

    pub fn is_asset_restricted(&self, asset: &Pubkey, slot: u64) -> Result<bool, String> {
        let target = RestrictionTarget::Asset(*asset);
        Ok(self
            .get_active_restrictions_for_target(&target, slot, 1)?
            .iter()
            .any(|record| record.mode == RestrictionMode::AssetPaused))
    }

    pub fn is_contract_restricted(
        &self,
        contract: &Pubkey,
        access: ContractRestrictionAccess,
        slot: u64,
    ) -> Result<bool, String> {
        let target = RestrictionTarget::Contract(*contract);
        Ok(self
            .get_active_restrictions_for_target(&target, slot, 0)?
            .iter()
            .any(|record| contract_record_blocks(record, access)))
    }

    pub fn is_code_hash_blocked(&self, code_hash: &Hash, slot: u64) -> Result<bool, String> {
        let ids = code_hash_ids_from_db(&self.db, code_hash)?;
        Ok(
            !active_effective_records_for_ids(ids, |id| self.get_restriction(id), slot, 1)?
                .is_empty(),
        )
    }

    pub fn is_bridge_route_paused(
        &self,
        chain_id: &str,
        asset: &str,
        slot: u64,
    ) -> Result<bool, String> {
        let target = RestrictionTarget::BridgeRoute {
            chain_id: chain_id.to_string(),
            asset: asset.to_string(),
        };
        Ok(self
            .get_active_restrictions_for_target(&target, slot, 1)?
            .iter()
            .any(|record| record.mode == RestrictionMode::RoutePaused))
    }

    pub fn is_protocol_module_paused(
        &self,
        module: ProtocolModuleId,
        slot: u64,
    ) -> Result<bool, String> {
        let target = RestrictionTarget::ProtocolModule(module);
        Ok(self
            .get_active_restrictions_for_target(&target, slot, 1)?
            .iter()
            .any(|record| record.mode == RestrictionMode::ProtocolPaused))
    }

    pub fn compute_restrictions_root(&self) -> Hash {
        let cf = match self.db.cf_handle(CF_RESTRICTIONS) {
            Some(cf) => cf,
            None => return Hash::default(),
        };
        let mut leaves = Vec::new();
        for item in self.db.iterator_cf(&cf, IteratorMode::Start).flatten() {
            let (key, value) = item;
            leaves.push(Hash::hash_two_parts(&key, &value));
        }
        if leaves.is_empty() {
            return Hash::default();
        }
        Self::merkle_root_from_leaves(&leaves)
    }
}

impl StateBatch {
    pub fn next_restriction_id(&mut self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let current = if let Some(counter) = self.restriction_counter {
            counter
        } else {
            get_counter(&self.db)?
        };
        let next = current
            .checked_add(1)
            .ok_or_else(|| "Restriction ID counter overflow".to_string())?;
        self.restriction_counter = Some(next);
        self.batch
            .put_cf(&cf, RESTRICTION_COUNTER_KEY, next.to_le_bytes());
        Ok(next)
    }

    pub fn put_restriction(&mut self, record: &RestrictionRecord) -> Result<(), String> {
        record.validate()?;
        validate_same_target_on_update(self.get_restriction(record.id)?, record)?;

        let cf_records = self
            .db
            .cf_handle(CF_RESTRICTIONS)
            .ok_or_else(|| "Restrictions CF not found".to_string())?;
        let cf_target = self
            .db
            .cf_handle(CF_RESTRICTION_INDEX_TARGET)
            .ok_or_else(|| "Restriction target index CF not found".to_string())?;
        let data = encode_restriction(record)?;
        self.batch
            .put_cf(&cf_records, restriction_id_key(record.id), &data);
        invalidate_cached_state_root(&self.db, &mut self.batch);

        let target_key = record.target.canonical_key();
        self.batch
            .put_cf(&cf_target, index_key(&target_key, record.id), []);
        self.restriction_target_index_overlay
            .entry(target_key)
            .or_default()
            .insert(record.id);

        if let Some(code_hash) = record.target.code_hash() {
            let cf_code = self
                .db
                .cf_handle(CF_RESTRICTION_INDEX_CODE_HASH)
                .ok_or_else(|| "Restriction code-hash index CF not found".to_string())?;
            self.batch
                .put_cf(&cf_code, index_key(&code_hash.0, record.id), []);
            self.restriction_code_hash_index_overlay
                .entry(code_hash.0)
                .or_default()
                .insert(record.id);
        }

        self.restriction_overlay.insert(record.id, record.clone());
        Ok(())
    }

    pub fn get_restriction(&self, id: u64) -> Result<Option<RestrictionRecord>, String> {
        if let Some(record) = self.restriction_overlay.get(&id) {
            return Ok(Some(record.clone()));
        }
        get_restriction_from_db(&self.db, id)
    }

    pub fn get_effective_restriction_record(
        &self,
        id: u64,
        slot: u64,
    ) -> Result<Option<EffectiveRestrictionRecord>, String> {
        Ok(self
            .get_restriction(id)?
            .map(|record| EffectiveRestrictionRecord::new(record, slot)))
    }

    pub fn list_effective_restrictions_by_target(
        &self,
        target: &RestrictionTarget,
        slot: u64,
        limit: usize,
    ) -> Result<Vec<EffectiveRestrictionRecord>, String> {
        let target_key = target.canonical_key();
        let mut ids = target_ids_from_db(&self.db, target)?;
        if let Some(overlay_ids) = self.restriction_target_index_overlay.get(&target_key) {
            ids.extend(overlay_ids.iter().copied());
        }

        let mut records = Vec::new();
        for id in ids {
            if let Some(record) = self.get_restriction(id)? {
                records.push(EffectiveRestrictionRecord::new(record, slot));
                if limit > 0 && records.len() >= limit {
                    break;
                }
            }
        }
        Ok(records)
    }

    pub fn get_active_restrictions_for_target(
        &self,
        target: &RestrictionTarget,
        slot: u64,
        limit: usize,
    ) -> Result<Vec<RestrictionRecord>, String> {
        let target_key = target.canonical_key();
        let mut ids = target_ids_from_db(&self.db, target)?;
        if let Some(overlay_ids) = self.restriction_target_index_overlay.get(&target_key) {
            ids.extend(overlay_ids.iter().copied());
        }
        active_effective_records_for_ids(ids, |id| self.get_restriction(id), slot, limit)
    }

    pub fn list_active_restrictions(
        &self,
        slot: u64,
        limit: usize,
    ) -> Result<Vec<RestrictionRecord>, String> {
        let mut by_id = std::collections::BTreeMap::new();
        for record in all_restrictions_from_db(&self.db)? {
            by_id.insert(record.id, record);
        }
        for record in self.restriction_overlay.values() {
            by_id.insert(record.id, record.clone());
        }

        let mut records = Vec::new();
        for record in by_id.into_values() {
            if record.is_effectively_active(slot) {
                records.push(record);
                if limit > 0 && records.len() >= limit {
                    break;
                }
            }
        }
        Ok(records)
    }

    pub fn is_account_restricted(
        &self,
        account: &Pubkey,
        direction: RestrictionTransferDirection,
        asset: Option<&Pubkey>,
        amount: u64,
        spendable: u64,
        slot: u64,
    ) -> Result<bool, String> {
        let account_target = RestrictionTarget::Account(*account);
        for record in self.get_active_restrictions_for_target(&account_target, slot, 0)? {
            if account_record_blocks(&record, direction, amount, spendable) {
                return Ok(true);
            }
        }

        if let Some(asset) = asset {
            let account_asset_target = RestrictionTarget::AccountAsset {
                account: *account,
                asset: *asset,
            };
            for record in self.get_active_restrictions_for_target(&account_asset_target, slot, 0)? {
                if account_record_blocks(&record, direction, amount, spendable) {
                    return Ok(true);
                }
            }
            if self.is_asset_restricted(asset, slot)? {
                return Ok(true);
            }
        }

        Ok(false)
    }

    pub fn is_asset_restricted(&self, asset: &Pubkey, slot: u64) -> Result<bool, String> {
        let target = RestrictionTarget::Asset(*asset);
        Ok(self
            .get_active_restrictions_for_target(&target, slot, 1)?
            .iter()
            .any(|record| record.mode == RestrictionMode::AssetPaused))
    }

    pub fn is_contract_restricted(
        &self,
        contract: &Pubkey,
        access: ContractRestrictionAccess,
        slot: u64,
    ) -> Result<bool, String> {
        let target = RestrictionTarget::Contract(*contract);
        Ok(self
            .get_active_restrictions_for_target(&target, slot, 0)?
            .iter()
            .any(|record| contract_record_blocks(record, access)))
    }

    pub fn is_code_hash_blocked(&self, code_hash: &Hash, slot: u64) -> Result<bool, String> {
        let mut ids = code_hash_ids_from_db(&self.db, code_hash)?;
        if let Some(overlay_ids) = self.restriction_code_hash_index_overlay.get(&code_hash.0) {
            ids.extend(overlay_ids.iter().copied());
        }
        Ok(
            !active_effective_records_for_ids(ids, |id| self.get_restriction(id), slot, 1)?
                .is_empty(),
        )
    }

    pub fn is_bridge_route_paused(
        &self,
        chain_id: &str,
        asset: &str,
        slot: u64,
    ) -> Result<bool, String> {
        let target = RestrictionTarget::BridgeRoute {
            chain_id: chain_id.to_string(),
            asset: asset.to_string(),
        };
        Ok(self
            .get_active_restrictions_for_target(&target, slot, 1)?
            .iter()
            .any(|record| record.mode == RestrictionMode::RoutePaused))
    }

    pub fn is_protocol_module_paused(
        &self,
        module: ProtocolModuleId,
        slot: u64,
    ) -> Result<bool, String> {
        let target = RestrictionTarget::ProtocolModule(module);
        Ok(self
            .get_active_restrictions_for_target(&target, slot, 1)?
            .iter()
            .any(|record| record.mode == RestrictionMode::ProtocolPaused))
    }
}
