use crate::block::Block;
use crate::codec::{append_legacy_bincode, deserialize_legacy_bincode, serialize_legacy_bincode};

use super::*;

const POST_STATE_COMMITMENT_ANCHOR_PREFIX: &[u8] = b"post_state_commitment_anchor:";
const ARCHIVE_CONTIGUOUS_TIP_KEY: &[u8] = b"archive_contiguous_tip_v1";

#[derive(serde::Deserialize)]
struct LegacyTxMetaV1 {
    compute_units_used: u64,
    return_code: Option<i64>,
    return_data: Vec<u8>,
    logs: Vec<String>,
}

fn decode_tx_meta(data: &[u8]) -> Result<crate::processor::TxMeta, String> {
    if let Ok(meta) =
        deserialize_legacy_bincode::<crate::processor::TxMeta>(data, "transaction receipt")
    {
        return Ok(meta);
    }

    let legacy = deserialize_legacy_bincode::<LegacyTxMetaV1>(data, "legacy tx meta")?;
    Ok(crate::processor::TxMeta {
        compute_units_used: legacy.compute_units_used,
        return_code: legacy.return_code,
        return_data: legacy.return_data,
        logs: legacy.logs,
        ..Default::default()
    })
}

fn post_state_commitment_anchor_key(slot: u64) -> Vec<u8> {
    let mut key =
        Vec::with_capacity(POST_STATE_COMMITMENT_ANCHOR_PREFIX.len() + std::mem::size_of::<u64>());
    key.extend_from_slice(POST_STATE_COMMITMENT_ANCHOR_PREFIX);
    key.extend_from_slice(&slot.to_be_bytes());
    key
}

impl StateStore {
    /// Highest canonical slot proven contiguous from genesis, paired with its
    /// block hash. The marker advances in the same WriteBatch as block storage.
    pub fn get_archive_contiguous_tip(&self) -> Result<Option<(u64, Hash)>, String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let Some(value) = self
            .db
            .get_cf(&cf, ARCHIVE_CONTIGUOUS_TIP_KEY)
            .map_err(|e| format!("Database error: {}", e))?
        else {
            return Ok(None);
        };
        if value.len() != 40 {
            return Err(format!(
                "Invalid archive contiguous-tip marker length: {}",
                value.len()
            ));
        }
        let mut slot_bytes = [0u8; 8];
        slot_bytes.copy_from_slice(&value[..8]);
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(&value[8..]);
        Ok(Some((u64::from_be_bytes(slot_bytes), Hash(hash_bytes))))
    }

    /// Record a contiguity proof produced by a complete verified snapshot.
    pub fn set_archive_contiguous_tip(&self, slot: u64, hash: Hash) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let mut value = Vec::with_capacity(40);
        value.extend_from_slice(&slot.to_be_bytes());
        value.extend_from_slice(&hash.0);
        self.db
            .put_cf(&cf, ARCHIVE_CONTIGUOUS_TIP_KEY, value)
            .map_err(|e| format!("Failed to persist archive contiguous tip: {}", e))
    }

    /// Get the last processed slot
    pub fn get_last_slot(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        match self.db.get_cf(&cf, b"last_slot") {
            Ok(Some(data)) => {
                let bytes: [u8; 8] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid slot data".to_string())?;
                Ok(u64::from_be_bytes(bytes))
            }
            Ok(None) => Ok(0),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Update the last processed slot
    pub fn set_last_slot(&self, slot: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        self.db
            .put_cf(&cf, b"last_slot", slot.to_be_bytes())
            .map_err(|e| format!("Failed to store slot: {}", e))
    }

    /// Get the last confirmed slot (2/3 supermajority reached)
    pub fn get_last_confirmed_slot(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        match self.db.get_cf(&cf, b"confirmed_slot") {
            Ok(Some(data)) => {
                let bytes: [u8; 8] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid confirmed slot data".to_string())?;
                Ok(u64::from_be_bytes(bytes))
            }
            Ok(None) => Ok(0),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Update the last confirmed slot
    pub fn set_last_confirmed_slot(&self, slot: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        self.db
            .put_cf(&cf, b"confirmed_slot", slot.to_be_bytes())
            .map_err(|e| format!("Failed to store confirmed slot: {}", e))
    }

    /// Get the last finalized slot under the active BFT commitment policy.
    pub fn get_last_finalized_slot(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        match self.db.get_cf(&cf, b"finalized_slot") {
            Ok(Some(data)) => {
                let bytes: [u8; 8] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid finalized slot data".to_string())?;
                Ok(u64::from_be_bytes(bytes))
            }
            Ok(None) => Ok(0),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Update the last finalized slot
    pub fn set_last_finalized_slot(&self, slot: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        self.db
            .put_cf(&cf, b"finalized_slot", slot.to_be_bytes())
            .map_err(|e| format!("Failed to store finalized slot: {}", e))
    }

    /// Get the hashes of the last N blocks for replay protection.
    /// Returns a set of block hashes from the most recent `count` slots.
    /// T1.3 fix: Hash::default() is NO LONGER accepted. Only real block hashes
    /// are valid for replay protection. Genesis block hash is included if in range.
    ///
    /// PERF-OPT 3: Uses an in-memory cache that is populated on block commit
    /// and avoids reading up to 300 blocks from RocksDB on every call.
    pub fn get_recent_blockhashes(
        &self,
        count: u64,
    ) -> Result<std::collections::HashSet<Hash>, String> {
        let last_slot = self.get_last_slot()?;
        let start_slot = last_slot.saturating_sub(count);
        {
            let cache = self
                .blockhash_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(ref inner) = *cache {
                if inner
                    .covered_range
                    .is_some_and(|(covered_start, covered_end)| {
                        covered_start <= start_slot && covered_end >= last_slot
                    })
                {
                    return Ok(inner
                        .entries
                        .iter()
                        .filter(|(slot, _)| *slot >= start_slot && *slot <= last_slot)
                        .map(|(_, hash)| *hash)
                        .collect());
                }
            }
        }

        let mut hashes = std::collections::HashSet::new();
        let mut entries: Vec<(u64, Hash)> = Vec::new();
        for slot in start_slot..=last_slot {
            if let Ok(Some(block)) = self.get_block_by_slot(slot) {
                let hash = block.hash();
                hashes.insert(hash);
                entries.push((slot, hash));
            }
        }

        {
            let mut cache = self
                .blockhash_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *cache = Some(BlockhashCache {
                entries,
                covered_range: Some((start_slot, last_slot)),
            });
        }

        Ok(hashes)
    }

    /// PERF-OPT 3: Push a new blockhash into the in-memory cache after committing a block.
    /// Evicts entries older than 300 slots.
    pub(crate) fn push_blockhash_cache(&self, hash: Hash, slot: u64) {
        let mut cache = self
            .blockhash_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let inner = cache.get_or_insert_with(|| BlockhashCache {
            entries: Vec::new(),
            covered_range: None,
        });
        if let Some(existing) = inner
            .entries
            .iter_mut()
            .find(|(entry_slot, _)| *entry_slot == slot)
        {
            existing.1 = hash;
        } else {
            inner.entries.push((slot, hash));
            inner
                .entries
                .sort_unstable_by_key(|(entry_slot, _)| *entry_slot);
        }
        inner.covered_range = match inner.covered_range {
            Some((start, end)) if slot <= end => Some((start, end)),
            Some((start, end)) if slot == end.saturating_add(1) => Some((start, slot)),
            Some(_) => None,
            None => None,
        };
        let cutoff = slot.saturating_sub(300);
        inner
            .entries
            .retain(|(entry_slot, _)| *entry_slot >= cutoff);
        if let Some((start, end)) = inner.covered_range.as_mut() {
            *start = (*start).max(cutoff);
            *end = (*end).max(slot);
        }
    }

    fn batch_index_block_activity(
        &self,
        block: &Block,
        batch: &mut WriteBatch,
    ) -> Result<(), String> {
        let nft_activity_cf = self
            .db
            .cf_handle(CF_NFT_ACTIVITY)
            .ok_or_else(|| "NFT activity CF not found".to_string())?;
        let program_calls_cf = self
            .db
            .cf_handle(CF_PROGRAM_CALLS)
            .ok_or_else(|| "Program calls CF not found".to_string())?;
        let market_activity_cf = self
            .db
            .cf_handle(CF_MARKET_ACTIVITY)
            .ok_or_else(|| "Market activity CF not found".to_string())?;

        let mut activity_seq: u32 = 0;

        for tx in block
            .transactions
            .iter()
            .filter(|transaction| !transaction.is_consensus())
        {
            let tx_signature = tx.signature();
            if self
                .get_tx_meta_full(&tx_signature)?
                .is_some_and(|meta| !meta.succeeded())
            {
                continue;
            }
            for ix in &tx.message.instructions {
                if ix.program_id == crate::processor::SYSTEM_PROGRAM_ID {
                    match ix.data.first() {
                        Some(7) => {
                            if ix.accounts.len() < 4 {
                                continue;
                            }

                            let activity = crate::NftActivity {
                                slot: block.header.slot,
                                timestamp: block.header.timestamp,
                                kind: crate::NftActivityKind::Mint,
                                collection: ix.accounts[1],
                                token: ix.accounts[2],
                                from: None,
                                to: ix.accounts[3],
                                tx_signature,
                            };

                            let mut key = Vec::with_capacity(32 + 8 + 4 + 32);
                            key.extend_from_slice(&activity.collection.0);
                            key.extend_from_slice(&activity.slot.to_be_bytes());
                            key.extend_from_slice(&activity_seq.to_be_bytes());
                            key.extend_from_slice(&activity.token.0);
                            let value = crate::nft::encode_nft_activity(&activity)?;
                            batch.put_cf(&nft_activity_cf, key, value);
                            activity_seq = activity_seq.saturating_add(1);
                        }
                        Some(8) => {
                            if ix.accounts.len() < 3 {
                                continue;
                            }

                            let token = ix.accounts[1];
                            let token_account = match self.get_account(&token) {
                                Ok(Some(account)) => account,
                                _ => continue,
                            };
                            let token_state =
                                match crate::nft::decode_token_state(&token_account.data) {
                                    Ok(state) => state,
                                    Err(_) => continue,
                                };

                            let activity = crate::NftActivity {
                                slot: block.header.slot,
                                timestamp: block.header.timestamp,
                                kind: crate::NftActivityKind::Transfer,
                                collection: token_state.collection,
                                token,
                                from: Some(ix.accounts[0]),
                                to: ix.accounts[2],
                                tx_signature,
                            };

                            let mut key = Vec::with_capacity(32 + 8 + 4 + 32);
                            key.extend_from_slice(&activity.collection.0);
                            key.extend_from_slice(&activity.slot.to_be_bytes());
                            key.extend_from_slice(&activity_seq.to_be_bytes());
                            key.extend_from_slice(&activity.token.0);
                            let value = crate::nft::encode_nft_activity(&activity)?;
                            batch.put_cf(&nft_activity_cf, key, value);
                            activity_seq = activity_seq.saturating_add(1);
                        }
                        _ => {}
                    }
                } else if ix.program_id == crate::processor::CONTRACT_PROGRAM_ID {
                    let Ok(crate::ContractInstruction::Call {
                        function,
                        args,
                        value,
                    }) = crate::ContractInstruction::deserialize(&ix.data)
                    else {
                        continue;
                    };
                    if ix.accounts.len() < 2 {
                        continue;
                    }

                    let caller = ix.accounts[0];
                    let program = ix.accounts[1];
                    let activity = crate::ProgramCallActivity {
                        slot: block.header.slot,
                        timestamp: block.header.timestamp,
                        program,
                        caller,
                        function: function.clone(),
                        value,
                        tx_signature,
                    };

                    let mut key = Vec::with_capacity(32 + 8 + 4 + 32);
                    key.extend_from_slice(&activity.program.0);
                    key.extend_from_slice(&activity.slot.to_be_bytes());
                    key.extend_from_slice(&activity_seq.to_be_bytes());
                    key.extend_from_slice(&activity.tx_signature.0);
                    let encoded = crate::encode_program_call_activity(&activity)?;
                    batch.put_cf(&program_calls_cf, key, encoded);
                    activity_seq = activity_seq.saturating_add(1);

                    if let Some(kind) =
                        crate::market_activity_kind_for_contract_function(function.as_str())
                    {
                        let market_activity = crate::build_market_activity_from_contract_call(
                            kind,
                            function,
                            program,
                            caller,
                            &args,
                            value,
                            block.header.slot,
                            block.header.timestamp,
                            tx_signature,
                        );
                        let zero = [0u8; 32];
                        let collection_bytes = market_activity
                            .collection
                            .as_ref()
                            .map(|collection| &collection.0)
                            .unwrap_or(&zero);

                        let mut key = Vec::with_capacity(32 + 8 + 4 + 32);
                        key.extend_from_slice(collection_bytes);
                        key.extend_from_slice(&market_activity.slot.to_be_bytes());
                        key.extend_from_slice(&activity_seq.to_be_bytes());
                        key.extend_from_slice(&market_activity.tx_signature.0);
                        let encoded = crate::encode_market_activity(&market_activity)?;
                        batch.put_cf(&market_activity_cf, key, encoded);
                        activity_seq = activity_seq.saturating_add(1);
                    } else {
                        activity_seq = activity_seq.saturating_add(1);
                    }
                }
            }
        }

        Ok(())
    }

    pub(crate) fn stage_canonical_block_anchor(
        &self,
        block: &Block,
        last_slot: Option<u64>,
        confirmed_slot: Option<u64>,
        finalized_slot: Option<u64>,
        batch: &mut WriteBatch,
    ) -> Result<bool, String> {
        let cf = self
            .db
            .cf_handle(CF_BLOCKS)
            .ok_or_else(|| "Blocks CF not found".to_string())?;
        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let tx_cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;
        let tx_to_slot_cf = self
            .db
            .cf_handle(CF_TX_TO_SLOT)
            .ok_or_else(|| "TX to slot CF not found".to_string())?;
        let tx_by_slot_cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "TX by slot CF not found".to_string())?;

        let block_hash = block.hash();
        let mut value = Vec::with_capacity(4096);
        value.push(0xBC);
        append_legacy_bincode(&mut value, block, "block")
            .map_err(|e| format!("Failed to serialize block: {}", e))?;

        let is_new_slot = self
            .get_block_by_slot(block.header.slot)
            .unwrap_or(None)
            .is_none();
        let current_last_slot = self.get_last_slot().unwrap_or(0);
        let current_confirmed_slot = self.get_last_confirmed_slot().unwrap_or(0);
        let current_finalized_slot = self.get_last_finalized_slot().unwrap_or(0);

        let archive_tip_before = self.get_archive_contiguous_tip()?;
        let mut archive_tip_after = archive_tip_before;
        if block.header.slot == 0 && archive_tip_before.is_none() {
            archive_tip_after = Some((0, block_hash));
        } else if let Some((contiguous_slot, contiguous_hash)) = archive_tip_before {
            if block.header.slot == contiguous_slot.saturating_add(1)
                && block.header.parent_hash == contiguous_hash
            {
                let mut next_slot = block.header.slot;
                let mut next_hash = block_hash;
                while let Some(candidate_slot) = next_slot.checked_add(1) {
                    let Some(candidate) = self.get_block_by_slot(candidate_slot)? else {
                        break;
                    };
                    if candidate.header.slot != candidate_slot
                        || candidate.header.parent_hash != next_hash
                    {
                        break;
                    }
                    next_slot = candidate_slot;
                    next_hash = candidate.hash();
                }
                archive_tip_after = Some((next_slot, next_hash));
            }
        }

        batch.put_cf(&cf, block_hash.0, &value);
        batch.put_cf(&slot_cf, block.header.slot.to_be_bytes(), block_hash.0);
        if let Some(slot) = last_slot {
            batch.put_cf(
                &slot_cf,
                b"last_slot",
                slot.max(current_last_slot).to_be_bytes(),
            );
        }
        if let Some(slot) = confirmed_slot {
            batch.put_cf(
                &slot_cf,
                b"confirmed_slot",
                slot.max(current_confirmed_slot).to_be_bytes(),
            );
        }
        if let Some(slot) = finalized_slot {
            batch.put_cf(
                &slot_cf,
                b"finalized_slot",
                slot.max(current_finalized_slot).to_be_bytes(),
            );
        }
        if archive_tip_after != archive_tip_before {
            let (slot, hash) = archive_tip_after.expect("changed archive tip exists");
            let mut marker = Vec::with_capacity(40);
            marker.extend_from_slice(&slot.to_be_bytes());
            marker.extend_from_slice(&hash.0);
            batch.put_cf(&slot_cf, ARCHIVE_CONTIGUOUS_TIP_KEY, marker);
        }

        for (tx_index, tx) in block.transactions.iter().enumerate() {
            let sig = tx.signature();
            let mut tx_value = Vec::with_capacity(512);
            tx_value.push(0xBC);
            append_legacy_bincode(&mut tx_value, tx, "transaction")
                .map_err(|e| format!("Failed to serialize tx {}: {}", sig.to_hex(), e))?;
            batch.put_cf(&tx_cf, sig.0, &tx_value);
            batch.put_cf(&tx_to_slot_cf, sig.0, block.header.slot.to_be_bytes());

            let mut key = Vec::with_capacity(16);
            key.extend_from_slice(&block.header.slot.to_be_bytes());
            key.extend_from_slice(&(tx_index as u64).to_be_bytes());
            batch.put_cf(&tx_by_slot_cf, &key, sig.0);
        }

        Ok(is_new_slot)
    }

    /// Store a block and all related per-transaction indexes atomically.
    fn write_block_batch(
        &self,
        block: &Block,
        last_slot: Option<u64>,
        confirmed_slot: Option<u64>,
        finalized_slot: Option<u64>,
    ) -> Result<(), String> {
        let _block_write_guard = self
            .block_write_lock
            .lock()
            .map_err(|_| "Block write lock poisoned".to_string())?;

        let shielded_txs_cf = self.db.cf_handle(CF_SHIELDED_TXS);

        let block_hash = block.hash();
        let mut batch = WriteBatch::default();
        let is_new_slot = self.stage_canonical_block_anchor(
            block,
            last_slot,
            confirmed_slot,
            finalized_slot,
            &mut batch,
        )?;

        for (tx_index, tx) in block.transactions.iter().enumerate() {
            let sig = tx.signature();
            let succeeded = self
                .get_tx_meta_full(&sig)?
                .map(|meta| meta.succeeded())
                .unwrap_or(true);
            if succeeded && is_shielded_transaction(tx) {
                if let Some(ref cf) = shielded_txs_cf {
                    let mut shielded_key = Vec::with_capacity(48);
                    shielded_key.extend_from_slice(&block.header.slot.to_be_bytes());
                    shielded_key.extend_from_slice(&(tx_index as u64).to_be_bytes());
                    shielded_key.extend_from_slice(&sig.0);
                    batch.put_cf(cf, &shielded_key, []);
                }
            }
        }

        self.batch_index_account_transactions(block, &mut batch)?;
        self.batch_index_block_activity(block, &mut batch)?;

        if is_new_slot {
            self.metrics.track_block(block);
            self.metrics.save_to_batch(&mut batch, &self.db)?;
        }

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to write block batch: {}", e))?;

        self.push_blockhash_cache(block_hash, block.header.slot);

        Ok(())
    }

    pub fn put_block(&self, block: &Block) -> Result<(), String> {
        self.write_block_batch(block, None, None, None)
    }

    /// Use `put_block_atomic` for canonical block application so block storage,
    /// tip advance, and commitment metadata land in the same durable WriteBatch.
    pub fn put_block_atomic(
        &self,
        block: &Block,
        confirmed_slot: Option<u64>,
        finalized_slot: Option<u64>,
    ) -> Result<(), String> {
        self.write_block_batch(
            block,
            Some(block.header.slot),
            confirmed_slot,
            finalized_slot,
        )
    }

    /// Store only the canonical block header for historical replay diagnostics.
    ///
    /// This preserves `get_block_by_slot(...).hash()` and timestamp lookups
    /// without duplicating the full block transaction/index archive into a
    /// scratch replay DB. Transaction effects are committed separately through
    /// the normal transaction replay batch.
    pub fn put_replay_block_header_atomic(
        &self,
        block: &Block,
        confirmed_slot: Option<u64>,
        finalized_slot: Option<u64>,
    ) -> Result<(), String> {
        let _block_write_guard = self
            .block_write_lock
            .lock()
            .map_err(|_| "Block write lock poisoned".to_string())?;

        let cf = self
            .db
            .cf_handle(CF_BLOCKS)
            .ok_or_else(|| "Blocks CF not found".to_string())?;
        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        let block_hash = block.hash();
        let mut header_only = block.clone();
        header_only.transactions.clear();
        header_only.tx_fees_paid.clear();

        let mut value = Vec::with_capacity(512);
        value.push(0xBC);
        append_legacy_bincode(&mut value, &header_only, "block header")
            .map_err(|e| format!("Failed to serialize replay block header: {}", e))?;

        let mut batch = WriteBatch::default();
        let current_last_slot = self.get_last_slot().unwrap_or(0);
        let current_confirmed_slot = self.get_last_confirmed_slot().unwrap_or(0);
        let current_finalized_slot = self.get_last_finalized_slot().unwrap_or(0);

        batch.put_cf(&cf, block_hash.0, &value);
        batch.put_cf(&slot_cf, block.header.slot.to_be_bytes(), block_hash.0);
        batch.put_cf(
            &slot_cf,
            b"last_slot",
            block.header.slot.max(current_last_slot).to_be_bytes(),
        );
        if let Some(slot) = confirmed_slot {
            batch.put_cf(
                &slot_cf,
                b"confirmed_slot",
                slot.max(current_confirmed_slot).to_be_bytes(),
            );
        }
        if let Some(slot) = finalized_slot {
            batch.put_cf(
                &slot_cf,
                b"finalized_slot",
                slot.max(current_finalized_slot).to_be_bytes(),
            );
        }

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to write replay block header: {}", e))?;
        self.push_blockhash_cache(block_hash, block.header.slot);

        Ok(())
    }

    pub fn get_block(&self, hash: &Hash) -> Result<Option<Block>, String> {
        let cf = self
            .db
            .cf_handle(CF_BLOCKS)
            .ok_or_else(|| "Blocks CF not found".to_string())?;

        match self.db.get_cf(&cf, hash.0) {
            Ok(Some(data)) => {
                let block: Block = if data.first() == Some(&0xBC) {
                    deserialize_legacy_bincode(&data[1..], "block")
                        .map_err(|e| format!("Failed to deserialize block (bincode): {}", e))?
                } else {
                    serde_json::from_slice(&data)
                        .map_err(|e| format!("Failed to deserialize block (json): {}", e))?
                };
                Ok(Some(block))
            }
            Ok(None) => {
                if let Some(ref cold) = self.cold_db {
                    if let Some(cold_cf) = cold.cf_handle(COLD_CF_BLOCKS) {
                        if let Ok(Some(data)) = cold.get_cf(&cold_cf, hash.0) {
                            let block: Block = if data.first() == Some(&0xBC) {
                                deserialize_legacy_bincode(&data[1..], "cold block").map_err(
                                    |e| {
                                        format!("Failed to deserialize cold block (bincode): {}", e)
                                    },
                                )?
                            } else {
                                serde_json::from_slice(&data).map_err(|e| {
                                    format!("Failed to deserialize cold block (json): {}", e)
                                })?
                            };
                            return Ok(Some(block));
                        }
                    }
                }
                Ok(None)
            }
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Get block by slot
    pub fn get_block_by_slot(&self, slot: u64) -> Result<Option<Block>, String> {
        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        match self.db.get_cf(&slot_cf, slot.to_be_bytes()) {
            Ok(Some(hash_bytes)) => {
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&hash_bytes);
                self.get_block(&Hash(hash))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Iterate canonical blocks from the slot index in ascending slot order.
    ///
    /// CF_SLOTS also contains metadata entries, so only exact slot->hash rows
    /// (`8-byte key`, `32-byte value`) are visited. The block value is resolved
    /// through `get_block`, preserving the existing hot/cold storage fallback.
    pub fn for_each_canonical_block_in_range<F>(
        &self,
        start_slot: u64,
        end_slot: u64,
        mut visit: F,
    ) -> Result<u64, String>
    where
        F: FnMut(u64, &Block) -> Result<(), String>,
    {
        if end_slot < start_slot {
            return Ok(0);
        }

        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        let seek_key = start_slot.to_be_bytes();
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);

        let iter = self.db.iterator_cf_opt(
            &slot_cf,
            read_opts,
            rocksdb::IteratorMode::From(&seek_key, Direction::Forward),
        );

        let mut visited = 0u64;
        for item in iter {
            let (key, value) = item.map_err(|e| format!("Slot index iterator error: {}", e))?;

            if key.len() != 8 || value.len() != 32 {
                continue;
            }

            let slot = u64::from_be_bytes(
                key.as_ref()
                    .try_into()
                    .map_err(|_| "Invalid slot index key".to_string())?,
            );
            if slot < start_slot {
                continue;
            }
            if slot > end_slot {
                break;
            }

            let mut hash_bytes = [0u8; 32];
            hash_bytes.copy_from_slice(value.as_ref());
            if let Some(block) = self.get_block(&Hash(hash_bytes))? {
                visit(slot, &block)?;
                visited = visited.saturating_add(1);
            }
        }

        Ok(visited)
    }

    /// Store the post-block state root for a canonical block after all
    /// deterministic post-block hooks have run. This is a sidecar commitment:
    /// it does not mutate consensus state and is not included in the state root.
    pub fn put_post_state_commitment_anchor(
        &self,
        slot: u64,
        block_hash: &Hash,
        state_root: &Hash,
    ) -> Result<(), String> {
        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        let mut value = Vec::with_capacity(64);
        value.extend_from_slice(&block_hash.0);
        value.extend_from_slice(&state_root.0);

        self.db
            .put_cf(&slot_cf, post_state_commitment_anchor_key(slot), value)
            .map_err(|e| format!("Failed to store post-state commitment anchor: {}", e))
    }

    pub fn get_post_state_commitment_anchor(
        &self,
        slot: u64,
    ) -> Result<Option<PostStateCommitmentAnchor>, String> {
        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        match self
            .db
            .get_cf(&slot_cf, post_state_commitment_anchor_key(slot))
        {
            Ok(Some(data)) => {
                if data.len() != 64 {
                    return Err(format!(
                        "Invalid post-state commitment anchor length for slot {}: {}",
                        slot,
                        data.len()
                    ));
                }
                let mut block_hash = [0u8; 32];
                block_hash.copy_from_slice(&data[..32]);
                let mut state_root = [0u8; 32];
                state_root.copy_from_slice(&data[32..64]);
                Ok(Some(PostStateCommitmentAnchor {
                    slot,
                    block_hash: Hash(block_hash),
                    state_root: Hash(state_root),
                }))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!(
                "Failed to read post-state commitment anchor for slot {}: {}",
                slot, e
            )),
        }
    }

    /// Get recent blocks via the slot index, newest first.
    /// Pass `before_slot` to fetch the next page strictly before that slot.
    pub fn get_recent_blocks(
        &self,
        limit: usize,
        before_slot: Option<u64>,
    ) -> Result<Vec<Block>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }

        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        let seek_key = before_slot.unwrap_or(u64::MAX).to_be_bytes().to_vec();
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);

        let iter = self.db.iterator_cf_opt(
            &slot_cf,
            read_opts,
            rocksdb::IteratorMode::From(&seek_key, Direction::Reverse),
        );

        let mut blocks = Vec::with_capacity(limit);
        for item in iter {
            let (key, value) = item.map_err(|e| format!("Slot index iterator error: {}", e))?;

            // CF_SLOTS also stores metadata such as last_slot/confirmed_slot.
            if key.len() != 8 || value.len() != 32 {
                continue;
            }

            let slot = u64::from_be_bytes(
                key.as_ref()
                    .try_into()
                    .map_err(|_| "Invalid slot index key".to_string())?,
            );

            if let Some(before) = before_slot {
                if slot >= before {
                    continue;
                }
            }

            let mut hash_bytes = [0u8; 32];
            hash_bytes.copy_from_slice(value.as_ref());
            if let Some(block) = self.get_block(&Hash(hash_bytes))? {
                blocks.push(block);
            }

            if blocks.len() >= limit {
                break;
            }
        }

        Ok(blocks)
    }

    /// Store a transaction
    pub fn put_transaction(&self, tx: &Transaction) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;

        let sig = tx.signature();
        let mut value = Vec::with_capacity(512);
        value.push(0xBC);
        append_legacy_bincode(&mut value, tx, "transaction")
            .map_err(|e| format!("Failed to serialize transaction: {}", e))?;

        self.db
            .put_cf(&cf, sig.0, &value)
            .map_err(|e| format!("Failed to store transaction: {}", e))
    }

    /// Get transaction by signature
    pub fn get_transaction(&self, sig: &Hash) -> Result<Option<Transaction>, String> {
        let cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;

        match self.db.get_cf(&cf, sig.0) {
            Ok(Some(data)) => {
                let tx: Transaction = if data.first() == Some(&0xBC) {
                    deserialize_legacy_bincode(&data[1..], "transaction").map_err(|e| {
                        format!("Failed to deserialize transaction (bincode): {}", e)
                    })?
                } else {
                    serde_json::from_slice(&data)
                        .map_err(|e| format!("Failed to deserialize transaction (json): {}", e))?
                };
                Ok(Some(tx))
            }
            Ok(None) => {
                if let Some(ref cold) = self.cold_db {
                    if let Some(cold_cf) = cold.cf_handle(COLD_CF_TRANSACTIONS) {
                        if let Ok(Some(data)) = cold.get_cf(&cold_cf, sig.0) {
                            let tx: Transaction = if data.first() == Some(&0xBC) {
                                deserialize_legacy_bincode(&data[1..], "cold transaction").map_err(
                                    |e| {
                                        format!(
                                            "Failed to deserialize cold transaction (bincode): {}",
                                            e
                                        )
                                    },
                                )?
                            } else {
                                serde_json::from_slice(&data).map_err(|e| {
                                    format!("Failed to deserialize cold transaction (json): {}", e)
                                })?
                            };
                            return Ok(Some(tx));
                        }
                    }
                }
                Ok(None)
            }
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Delete transaction record (used during fork choice to allow re-replay)
    pub fn delete_transaction(&self, sig: &Hash) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;

        self.db
            .delete_cf(&cf, sig.0)
            .map_err(|e| format!("Failed to delete transaction: {}", e))
    }

    /// Store transaction execution metadata (compute_units_used).
    pub fn put_tx_meta(&self, sig: &Hash, compute_units_used: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TX_META)
            .ok_or_else(|| "TX meta CF not found".to_string())?;
        self.db
            .put_cf(&cf, sig.0, compute_units_used.to_le_bytes())
            .map_err(|e| format!("Failed to store tx meta: {}", e))
    }

    /// Store full transaction execution metadata (CU, return_code, return_data, logs).
    pub fn put_tx_meta_full(
        &self,
        sig: &Hash,
        meta: &crate::processor::TxMeta,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TX_META)
            .ok_or_else(|| "TX meta CF not found".to_string())?;
        let data = serialize_legacy_bincode(meta, "tx meta")?;
        self.db
            .put_cf(&cf, sig.0, data)
            .map_err(|e| format!("Failed to store tx meta: {}", e))
    }

    /// Get stored compute_units_used for a transaction.
    /// Handles both old 8-byte format and new bincode TxMeta format.
    pub fn get_tx_meta_cu(&self, sig: &Hash) -> Result<Option<u64>, String> {
        let cf = self
            .db
            .cf_handle(CF_TX_META)
            .ok_or_else(|| "TX meta CF not found".to_string())?;
        match self.db.get_cf(&cf, sig.0) {
            Ok(Some(data)) if data.len() == 8 => {
                Ok(Some(u64::from_le_bytes(data.try_into().unwrap())))
            }
            Ok(Some(data)) => {
                if let Ok(meta) = decode_tx_meta(&data) {
                    Ok(Some(meta.compute_units_used))
                } else {
                    Ok(None)
                }
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Get full transaction execution metadata.
    /// Returns None for transactions stored in the old 8-byte CU-only format
    /// (those are handled transparently with default return_code/return_data/logs).
    pub fn get_tx_meta_full(&self, sig: &Hash) -> Result<Option<crate::processor::TxMeta>, String> {
        let cf = self
            .db
            .cf_handle(CF_TX_META)
            .ok_or_else(|| "TX meta CF not found".to_string())?;
        match self.db.get_cf(&cf, sig.0) {
            Ok(Some(data)) if data.len() == 8 => Ok(Some(crate::processor::TxMeta {
                compute_units_used: u64::from_le_bytes(data.try_into().unwrap()),
                ..Default::default()
            })),
            Ok(Some(data)) => decode_tx_meta(&data)
                .map(Some)
                .map_err(|e| format!("Failed to deserialize tx meta: {}", e)),
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }
}
