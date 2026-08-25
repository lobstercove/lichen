use std::collections::BTreeMap;
use std::sync::Arc;

use super::{StateStore, CF_ACCOUNT_TXS, COLD_CF_ACCOUNT_TXS};
use crate::archive_v2::{
    ArchiveV2PublicRow, ArchiveV2Reader, ArchiveV2ReaderStatus, ArchiveV2Role, ArchiveV2Rows,
    ArchiveV2SegmentContents,
};
use crate::{Block, Hash, Transaction};

const ARCHIVE_V2_RAW_PUBLIC_CATEGORIES: &[&str] = &[
    "tx_meta",
    "account_txs",
    "events_by_slot",
    "events",
    "token_transfers",
    "program_calls",
    "evm_txs",
    "evm_receipts",
    "evm_logs_by_slot",
    "shielded_txs",
    "nft_activity",
    "market_activity",
    "dex_trades_by_pair",
    "dex_trades_by_taker",
    "dex_trades_by_pair_taker",
    "account_snapshots",
];
const ARCHIVE_V2_EXPORT_PAGE_ROWS: u64 = 50_000;

impl StateStore {
    pub fn attach_archive_v2_reader(&self, reader: ArchiveV2Reader) {
        *self
            .archive_v2_reader
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(reader));
    }

    pub fn detach_archive_v2_reader(&self) {
        *self
            .archive_v2_reader
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    pub fn has_archive_v2_reader(&self) -> bool {
        self.archive_v2_reader
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some()
    }

    pub fn archive_v2_status(&self) -> Option<ArchiveV2ReaderStatus> {
        self.archive_v2_reader()
            .as_ref()
            .map(|reader| reader.status())
    }

    /// Whether an admitted non-full role exclusively owns a catalog-covered
    /// public-history read. Verified-cache and consensus nodes must not expose
    /// bootstrap-only hot/cold bytes after admission. A full-archive node is
    /// different: until legacy retirement removes those canonical rows, its
    /// documented hot -> cold -> V2 order remains a local recovery path for a
    /// quarantined or temporarily unavailable V2 object.
    pub(super) fn archive_v2_exclusively_covers_slot(&self, slot: u64) -> bool {
        self.archive_v2_reader().is_some_and(|reader| {
            reader.status().admitted_after_fresh_sync
                && reader.role() != ArchiveV2Role::FullArchive
                && reader.covers_slot(slot)
        })
    }

    pub fn mark_archive_v2_admitted_after_fresh_sync(&self) -> Result<(), String> {
        let reader = self
            .archive_v2_reader()
            .ok_or_else(|| "Archive V2 reader is not attached".to_string())?;
        reader.mark_admitted_after_fresh_sync();
        Ok(())
    }

    pub fn archive_v2_local_verified_object(
        &self,
        object_hash: &Hash,
    ) -> Result<Option<Vec<u8>>, String> {
        let Some(reader) = self.archive_v2_reader() else {
            return Ok(None);
        };
        reader
            .local_verified_object(object_hash)
            .map_err(|error| error.to_string())
    }

    /// Legacy cold history is available while the validator establishes its
    /// chain identity and admits Archive V2. After admission, only a
    /// full-archive role may continue serving it. The reader lives behind a
    /// shared lock, so this policy change reaches StateStore clones that were
    /// created during startup.
    pub(super) fn legacy_cold_history_reads_enabled(&self) -> bool {
        self.archive_v2_reader()
            .is_none_or(|reader| reader.role() == ArchiveV2Role::FullArchive)
    }

    /// Collect an exact finalized-source range for offline Archive V2
    /// benchmarking and operator verification. Callers must still pin and
    /// recheck finalized boundary hashes around long-running collection.
    pub fn export_archive_v2_segment_contents(
        &self,
        start_slot: u64,
        end_slot: u64,
    ) -> Result<ArchiveV2SegmentContents, String> {
        if end_slot < start_slot {
            return Err("Archive V2 export range end precedes start".to_string());
        }
        let blocks = (start_slot..=end_slot)
            .map(|slot| {
                self.get_block_by_slot(slot)?.ok_or_else(|| {
                    format!("canonical block {slot} is missing from the legacy source")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let public_categories =
            self.archive_v2_public_categories_from_blocks(start_slot, end_slot, Some(&blocks))?;
        Ok(ArchiveV2SegmentContents {
            blocks,
            public_categories,
        })
    }

    pub(super) fn archive_v2_block_by_slot(&self, slot: u64) -> Result<Option<Block>, String> {
        let Some(reader) = self.archive_v2_reader() else {
            return Ok(None);
        };
        reader.get_block(slot).map_err(|error| error.to_string())
    }

    pub(super) fn archive_v2_block_by_hash(&self, hash: &Hash) -> Result<Option<Block>, String> {
        let Some(reader) = self.archive_v2_reader() else {
            return Ok(None);
        };
        reader
            .get_block_by_hash(hash)
            .map_err(|error| error.to_string())
    }

    pub(super) fn archive_v2_transaction(
        &self,
        signature: &Hash,
    ) -> Result<Option<Transaction>, String> {
        let Some(reader) = self.archive_v2_reader() else {
            return Ok(None);
        };
        reader
            .get_transaction(signature)
            .map_err(|error| error.to_string())
    }

    pub(super) fn archive_v2_transaction_slot(
        &self,
        signature: &Hash,
    ) -> Result<Option<u64>, String> {
        let Some(reader) = self.archive_v2_reader() else {
            return Ok(None);
        };
        reader
            .get_transaction_slot(signature)
            .map_err(|error| error.to_string())
    }

    pub(super) fn archive_v2_category_rows(
        &self,
        category: &str,
        start_slot: u64,
        end_slot: u64,
    ) -> Result<ArchiveV2Rows, String> {
        let Some(reader) = self.archive_v2_reader() else {
            return Ok(Vec::new());
        };
        if reader.role() == ArchiveV2Role::Consensus {
            // Consensus nodes serve recent rows directly from hot state. They
            // neither consult legacy cold history nor expose Archive V2 deep
            // history through aggregate public-history queries.
            return Ok(Vec::new());
        }
        reader
            .category_rows(category, start_slot, end_slot)
            .map_err(|error| error.to_string())
    }

    pub(super) fn archive_v2_category_value(
        &self,
        category: &str,
        key: &[u8],
    ) -> Result<Option<(u64, Vec<u8>)>, String> {
        let Some(reader) = self.archive_v2_reader() else {
            return Ok(None);
        };
        reader
            .category_value(category, key)
            .map_err(|error| error.to_string())
    }

    #[cfg(test)]
    pub(crate) fn archive_v2_public_categories(
        &self,
        start_slot: u64,
        end_slot: u64,
    ) -> Result<BTreeMap<String, Vec<ArchiveV2PublicRow>>, String> {
        self.archive_v2_public_categories_from_blocks(start_slot, end_slot, None)
    }

    fn archive_v2_public_categories_from_blocks(
        &self,
        start_slot: u64,
        end_slot: u64,
        blocks: Option<&[Block]>,
    ) -> Result<BTreeMap<String, Vec<ArchiveV2PublicRow>>, String> {
        if end_slot < start_slot {
            return Err("Archive V2 category range end precedes start".to_string());
        }

        let dex_trade_slots = self.archive_v2_dex_trade_slots()?;
        let mut categories = BTreeMap::new();
        for category in ARCHIVE_V2_RAW_PUBLIC_CATEGORIES {
            if *category == "account_txs" {
                if let Some(blocks) = blocks {
                    categories.insert(
                        (*category).to_string(),
                        self.archive_v2_account_tx_rows_from_blocks(start_slot, end_slot, blocks)?,
                    );
                    continue;
                }
            }
            if *category == "tx_meta" {
                let source =
                    self.archive_v2_export_bounded_category(category, start_slot, end_slot)?;
                let mut rows_by_key = BTreeMap::new();
                for (key, value) in source {
                    let signature = Hash(key.as_slice().try_into().map_err(|_| {
                        format!("Archive V2 tx_meta row has invalid {}-byte key", key.len())
                    })?);
                    let slot = self.get_tx_slot(&signature)?.ok_or_else(|| {
                        format!(
                            "Archive V2 tx_meta row {} has no canonical transaction slot",
                            signature.to_hex()
                        )
                    })?;
                    if !(start_slot..=end_slot).contains(&slot) {
                        // Canonical blocks can contain a repeated transaction
                        // signature, while the legacy tx_meta/tx_to_slot maps
                        // have one authoritative row per signature. Assign the
                        // row only to that authoritative slot's segment.
                        continue;
                    }
                    match rows_by_key.insert(key.clone(), (slot, value.clone())) {
                        Some(existing) if existing != (slot, value) => {
                            return Err(format!(
                                "Archive V2 tx_meta key {} has conflicting canonical rows",
                                hex::encode(key)
                            ));
                        }
                        _ => {}
                    }
                }
                let rows = rows_by_key
                    .into_iter()
                    .map(|(key, (slot, value))| ArchiveV2PublicRow { slot, key, value })
                    .collect();
                categories.insert((*category).to_string(), rows);
                continue;
            }

            let mut rows = Vec::new();
            let mut cursor = None;
            loop {
                let page = self.export_public_history_category_cursor_untracked(
                    category,
                    cursor.as_deref(),
                    ARCHIVE_V2_EXPORT_PAGE_ROWS,
                )?;
                for (key, value) in page.entries {
                    let slot =
                        self.archive_v2_public_row_slot(category, &key, &value, &dex_trade_slots)?;
                    if let Some(slot) = slot.filter(|slot| (start_slot..=end_slot).contains(slot)) {
                        rows.push(ArchiveV2PublicRow { slot, key, value });
                    }
                }
                if !page.has_more {
                    break;
                }
                cursor = Some(page.next_cursor.ok_or_else(|| {
                    format!("{category} export has more rows but no continuation cursor")
                })?);
            }
            if rows
                .windows(2)
                .any(|pair| pair[0].key.as_slice() >= pair[1].key.as_slice())
            {
                return Err(format!(
                    "Archive V2 {category} source rows are duplicated or out of order"
                ));
            }
            categories.insert((*category).to_string(), rows);
        }
        Ok(categories)
    }

    fn archive_v2_account_tx_rows_from_blocks(
        &self,
        start_slot: u64,
        end_slot: u64,
        blocks: &[Block],
    ) -> Result<Vec<ArchiveV2PublicRow>, String> {
        // CF_ACCOUNT_TXS is ordered by account before slot. A range export that
        // walks that CF therefore rescans and revalidates all historical rows
        // for every Archive V2 segment. Canonical blocks already determine the
        // exact index keys; point-checking each derived key in the immutable
        // hot/cold source preserves source backing and conflict detection while
        // making collection proportional to this segment's transactions.
        let expected_block_count = end_slot
            .checked_sub(start_slot)
            .and_then(|distance| distance.checked_add(1))
            .ok_or_else(|| "Archive V2 account_txs block range overflow".to_string())?;
        if blocks.len() as u64 != expected_block_count
            || blocks.first().map(|block| block.header.slot) != Some(start_slot)
            || blocks.last().map(|block| block.header.slot) != Some(end_slot)
            || blocks
                .windows(2)
                .any(|pair| pair[0].header.slot.checked_add(1) != Some(pair[1].header.slot))
        {
            return Err(
                "Archive V2 account_txs derivation requires an exact contiguous block range"
                    .to_string(),
            );
        }

        let hot_cf = self
            .db
            .cf_handle(CF_ACCOUNT_TXS)
            .ok_or_else(|| "Account txs CF not found".to_string())?;
        let cold_cf = self
            .cold_db
            .as_ref()
            .and_then(|cold| cold.cf_handle(COLD_CF_ACCOUNT_TXS));
        let mut rows = BTreeMap::<Vec<u8>, ArchiveV2PublicRow>::new();

        for block in blocks {
            for (_, key) in super::secondary_indexes::account_tx_index_entries_for_block(block) {
                let hot = self
                    .db
                    .get_cf(&hot_cf, &key)
                    .map_err(|error| format!("Archive V2 account_txs hot lookup failed: {error}"))?
                    .map(|value| {
                        self.canonical_public_history_import_value("account_txs", &key, &value)
                    })
                    .transpose()?;
                let cold = match (&self.cold_db, &cold_cf) {
                    (Some(cold), Some(cold_cf)) => cold
                        .get_cf(cold_cf, &key)
                        .map_err(|error| {
                            format!("Archive V2 account_txs cold lookup failed: {error}")
                        })?
                        .map(|value| {
                            self.canonical_public_history_import_value("account_txs", &key, &value)
                        })
                        .transpose()?,
                    _ => None,
                };
                let value = match (hot, cold) {
                    (Some(hot), Some(cold)) if hot != cold => {
                        return Err(format!(
                            "Archive V2 account_txs hot/cold conflict for key {}",
                            hex::encode(&key)
                        ));
                    }
                    (Some(value), _) | (_, Some(value)) => value,
                    (None, None) => {
                        return Err(format!(
                            "Archive V2 canonical block {} account_txs key {} is missing from the legacy source",
                            block.header.slot,
                            hex::encode(&key)
                        ));
                    }
                };
                let row = ArchiveV2PublicRow {
                    slot: block.header.slot,
                    key: key.clone(),
                    value,
                };
                match rows.insert(key.clone(), row.clone()) {
                    Some(existing) if existing != row => {
                        return Err(format!(
                            "Archive V2 account_txs key {} has conflicting canonical rows",
                            hex::encode(key)
                        ));
                    }
                    _ => {}
                }
            }
        }

        Ok(rows.into_values().collect())
    }

    fn archive_v2_export_bounded_category(
        &self,
        category: &str,
        start_slot: u64,
        end_slot: u64,
    ) -> Result<ArchiveV2Rows, String> {
        let mut rows = Vec::new();
        let mut cursor = start_slot.checked_sub(1).map(|slot| {
            let mut cursor = Vec::with_capacity(16);
            cursor.extend_from_slice(&slot.to_be_bytes());
            cursor.extend_from_slice(&u64::MAX.to_be_bytes());
            cursor
        });
        loop {
            let page = self.export_public_history_category_range_cursor_untracked(
                category,
                cursor.as_deref(),
                ARCHIVE_V2_EXPORT_PAGE_ROWS,
                Some(end_slot),
            )?;
            rows.extend(page.entries);
            if !page.has_more {
                break;
            }
            cursor = Some(page.next_cursor.ok_or_else(|| {
                format!("{category} export has more rows but no continuation cursor")
            })?);
        }
        Ok(rows)
    }

    fn archive_v2_public_row_slot(
        &self,
        category: &str,
        key: &[u8],
        value: &[u8],
        dex_trade_slots: &BTreeMap<u64, u64>,
    ) -> Result<Option<u64>, String> {
        let slot_at = |offset: usize, required_len: usize| -> Result<Option<u64>, String> {
            if key.len() < required_len {
                return Err(format!(
                    "Archive V2 {category} row has invalid {}-byte key",
                    key.len()
                ));
            }
            Ok(Some(u64::from_be_bytes(
                key[offset..offset + 8]
                    .try_into()
                    .map_err(|_| format!("Archive V2 {category} slot key is malformed"))?,
            )))
        };
        match category {
            "account_txs" => slot_at(32, 76),
            "events" => slot_at(32, 56),
            "token_transfers" => slot_at(32, 48),
            "program_calls" | "nft_activity" | "market_activity" => slot_at(32, 76),
            "account_snapshots" => slot_at(32, 40),
            "events_by_slot" => slot_at(0, 48),
            "evm_logs_by_slot" => slot_at(0, 8),
            "shielded_txs" => slot_at(0, 48),
            "evm_txs" => {
                let evm_hash: [u8; 32] = key.try_into().map_err(|_| {
                    format!("Archive V2 evm_txs row has invalid {}-byte key", key.len())
                })?;
                let record = self.get_evm_tx(&evm_hash)?.ok_or_else(|| {
                    "Archive V2 evm_txs source row disappeared during collection".to_string()
                })?;
                Ok(record.block_slot)
            }
            "evm_receipts" => {
                let evm_hash: [u8; 32] = key.try_into().map_err(|_| {
                    format!(
                        "Archive V2 evm_receipts row has invalid {}-byte key",
                        key.len()
                    )
                })?;
                let receipt = self.get_evm_receipt(&evm_hash)?.ok_or_else(|| {
                    "Archive V2 evm_receipts source row disappeared during collection".to_string()
                })?;
                Ok(receipt.block_slot)
            }
            "dex_trades_by_pair" | "dex_trades_by_taker" | "dex_trades_by_pair_taker" => {
                if !value.is_empty() {
                    return Err(format!(
                        "Archive V2 {category} row has unexpected non-empty value"
                    ));
                }
                let trade_id = u64::from_be_bytes(
                    key.get(key.len().saturating_sub(8)..)
                        .and_then(|bytes| bytes.try_into().ok())
                        .ok_or_else(|| {
                            format!(
                                "Archive V2 {category} row has invalid {}-byte key",
                                key.len()
                            )
                        })?,
                );
                dex_trade_slots
                    .get(&trade_id)
                    .copied()
                    .map(Some)
                    .ok_or_else(|| {
                        format!("Archive V2 {category} row references missing DEX trade {trade_id}")
                    })
            }
            _ => Err(format!(
                "Archive V2 has no slot decoder for public category {category}"
            )),
        }
    }

    fn archive_v2_dex_trade_slots(&self) -> Result<BTreeMap<u64, u64>, String> {
        let Some(dex_program) = super::dex_index::dex_program_from_registry(&self.db)? else {
            return Ok(BTreeMap::new());
        };
        let cf = self
            .db
            .cf_handle(super::CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;
        let mut read_options = rocksdb::ReadOptions::default();
        read_options.set_total_order_seek(true);
        let iterator = self.db.iterator_cf_opt(
            &cf,
            read_options,
            rocksdb::IteratorMode::From(&dex_program.0, rocksdb::Direction::Forward),
        );
        let mut slots = BTreeMap::new();
        for item in iterator {
            let (key, value) =
                item.map_err(|error| format!("Failed scanning DEX trade sources: {error}"))?;
            if !key.starts_with(&dex_program.0) {
                break;
            }
            let Some(storage_key) = key.get(32..) else {
                continue;
            };
            let Some(expected_trade_id) =
                super::dex_index::dex_trade_id_from_storage_key(storage_key)
            else {
                continue;
            };
            let trade = crate::dex::decode_trade(&value).ok_or_else(|| {
                format!(
                    "Archive V2 cannot decode DEX trade source key {}",
                    hex::encode(&key)
                )
            })?;
            if trade.trade_id != expected_trade_id {
                return Err(format!(
                    "Archive V2 DEX trade source key id {expected_trade_id} conflicts with encoded trade id {}",
                    trade.trade_id
                ));
            }
            match slots.insert(trade.trade_id, trade.slot) {
                Some(existing) if existing != trade.slot => {
                    return Err(format!(
                        "Archive V2 found conflicting slots for DEX trade {}",
                        trade.trade_id
                    ));
                }
                _ => {}
            }
        }
        Ok(slots)
    }

    pub(super) fn archive_v2_reader(&self) -> Option<Arc<ArchiveV2Reader>> {
        self.archive_v2_reader
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::archive_v2::{
        ArchiveV2Catalog, ArchiveV2CodecConfig, ArchiveV2DirectorySource, ArchiveV2Identity,
        ArchiveV2ReaderConfig, ArchiveV2SegmentCodec, ArchiveV2SegmentContents,
    };
    use crate::state::{
        SymbolRegistryEntry, CF_BLOCKS, CF_CONTRACT_STORAGE, CF_SLOTS, CF_TRANSACTIONS,
        CF_TX_TO_SLOT,
    };
    use crate::{Instruction, Message, Pubkey};

    #[test]
    fn state_reader_falls_through_hot_and_legacy_to_verified_archive_v2() {
        let state_root = tempdir().unwrap();
        let archive_root = tempdir().unwrap();
        let state = StateStore::open(state_root.path()).unwrap();
        let transaction = Transaction::new(Message::new(
            vec![Instruction {
                program_id: Pubkey([1; 32]),
                accounts: vec![Pubkey([2; 32])],
                data: vec![3; 64],
            }],
            Hash::hash(b"archive-v2-fallthrough-tx"),
        ));
        let signature = transaction.signature();
        let block = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"archive-v2-fallthrough-state"),
            [5; 32],
            vec![transaction],
            1,
        );
        let block_hash = block.hash();
        state.put_block(&block).unwrap();

        let identity = ArchiveV2Identity {
            network_id: "fallthrough-testnet".to_string(),
            genesis_hash: block_hash,
        };
        let (bytes, manifest) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &ArchiveV2SegmentContents::from_blocks(vec![block]),
            &ArchiveV2CodecConfig {
                target_frame_bytes: 1024 * 1024,
                ..ArchiveV2CodecConfig::default()
            },
        )
        .unwrap();
        let objects = archive_root.path().join("objects");
        fs::create_dir_all(&objects).unwrap();
        fs::write(
            objects.join(format!("{}.av2s", manifest.segment_object_hash)),
            bytes,
        )
        .unwrap();
        let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
        catalog.append(manifest).unwrap();
        let catalog_path = archive_root.path().join("catalog.av2");
        catalog.store_atomic(&catalog_path).unwrap();
        let reader = ArchiveV2Reader::open(
            identity,
            &catalog_path,
            ArchiveV2ReaderConfig {
                role: crate::archive_v2::ArchiveV2Role::FullArchive,
                root: archive_root.path().to_path_buf(),
                cache_root: None,
                cache_quota_bytes: 0,
                max_decoded_segments: 2,
                allow_remote_fetch: false,
                sources: Vec::new(),
            },
        )
        .unwrap();
        state.attach_archive_v2_reader(reader);

        state
            .db
            .delete_cf(&state.db.cf_handle(CF_BLOCKS).unwrap(), block_hash.0)
            .unwrap();
        state
            .db
            .delete_cf(&state.db.cf_handle(CF_TRANSACTIONS).unwrap(), signature.0)
            .unwrap();
        state
            .db
            .delete_cf(&state.db.cf_handle(CF_TX_TO_SLOT).unwrap(), signature.0)
            .unwrap();

        let local_hits_before = state.archive_v2_status().unwrap().local_hits;
        assert!(
            !state.has_hot_transaction(&signature).unwrap(),
            "consensus-active membership must not fall through to Archive V2"
        );
        assert_eq!(
            state.archive_v2_status().unwrap().local_hits,
            local_hits_before,
            "consensus-active membership must not read a deep-history object"
        );
        assert_eq!(
            state.get_block_by_slot(0).unwrap().unwrap().hash(),
            block_hash
        );
        assert_eq!(
            state.get_block(&block_hash).unwrap().unwrap().hash(),
            block_hash
        );
        assert_eq!(
            state
                .get_transaction(&signature)
                .unwrap()
                .unwrap()
                .signature(),
            signature
        );
        assert_eq!(state.get_tx_slot(&signature).unwrap(), Some(0));
        assert!(state.archive_v2_status().is_some());
    }

    #[test]
    fn missing_recent_block_body_does_not_scan_archive_v2_by_hash() {
        let state_root = tempdir().unwrap();
        let archive_root = tempdir().unwrap();
        let state = StateStore::open(state_root.path()).unwrap();
        let historical = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"bounded-slot-fallback-historical-state"),
            [0x51; 32],
            Vec::new(),
            1,
        );
        let identity = ArchiveV2Identity {
            network_id: "bounded-slot-fallback-testnet".to_string(),
            genesis_hash: historical.hash(),
        };
        let (bytes, manifest) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &ArchiveV2SegmentContents::from_blocks(vec![historical]),
            &ArchiveV2CodecConfig {
                target_frame_bytes: 1024 * 1024,
                ..ArchiveV2CodecConfig::default()
            },
        )
        .unwrap();
        let objects = archive_root.path().join("objects");
        fs::create_dir_all(&objects).unwrap();
        fs::write(
            objects.join(format!("{}.av2s", manifest.segment_object_hash)),
            bytes,
        )
        .unwrap();
        let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
        catalog.append(manifest).unwrap();
        let catalog_path = archive_root.path().join("catalog.av2");
        catalog.store_atomic(&catalog_path).unwrap();
        state.attach_archive_v2_reader(
            ArchiveV2Reader::open(
                identity,
                &catalog_path,
                ArchiveV2ReaderConfig {
                    role: crate::archive_v2::ArchiveV2Role::FullArchive,
                    root: archive_root.path().to_path_buf(),
                    cache_root: None,
                    cache_quota_bytes: 0,
                    max_decoded_segments: 1,
                    allow_remote_fetch: false,
                    sources: Vec::new(),
                },
            )
            .unwrap(),
        );
        state.mark_archive_v2_admitted_after_fresh_sync().unwrap();

        let recent = Block::new_with_timestamp(
            2,
            Hash::hash(b"bounded-slot-fallback-parent"),
            Hash::hash(b"bounded-slot-fallback-recent-state"),
            [0x52; 32],
            Vec::new(),
            2,
        );
        state
            .db
            .put_cf(
                &state.db.cf_handle(CF_SLOTS).unwrap(),
                recent.header.slot.to_be_bytes(),
                recent.hash().0,
            )
            .unwrap();

        let local_hits_before = state.archive_v2_status().unwrap().local_hits;
        assert!(state
            .get_block_by_slot(recent.header.slot)
            .unwrap()
            .is_none());
        assert_eq!(
            state.archive_v2_status().unwrap().local_hits,
            local_hits_before,
            "a known recent slot outside catalog coverage must not scan Archive V2 by hash"
        );
    }

    #[test]
    fn admitted_non_full_roles_hide_bootstrap_cold_history_from_existing_clones() {
        let state_root = tempdir().unwrap();
        let cold_root = tempdir().unwrap();
        let archive_root = tempdir().unwrap();
        let remote_root = tempdir().unwrap();
        let cache_root = tempdir().unwrap();
        let mut state = StateStore::open(state_root.path()).unwrap();
        state.open_cold_store(cold_root.path()).unwrap();

        let block = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"archive-v2-role-bound-state"),
            [7; 32],
            Vec::new(),
            1,
        );
        let block_hash = block.hash();
        state.put_block(&block).unwrap();
        state.migrate_to_cold(1).unwrap();
        assert_eq!(
            state.get_block_by_slot(0).unwrap().unwrap().hash(),
            block_hash,
            "legacy cold must remain available during startup"
        );
        let startup_clone = state.clone();

        let identity = ArchiveV2Identity {
            network_id: "role-bound-testnet".to_string(),
            genesis_hash: block_hash,
        };
        let (bytes, manifest) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &ArchiveV2SegmentContents::from_blocks(vec![block]),
            &ArchiveV2CodecConfig {
                target_frame_bytes: 1024 * 1024,
                ..ArchiveV2CodecConfig::default()
            },
        )
        .unwrap();
        let remote_objects = remote_root.path().join("objects");
        fs::create_dir_all(&remote_objects).unwrap();
        fs::write(
            remote_objects.join(format!("{}.av2s", manifest.segment_object_hash)),
            &bytes,
        )
        .unwrap();
        let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
        catalog.append(manifest).unwrap();
        let catalog_path = archive_root.path().join("catalog.av2");
        catalog.store_atomic(&catalog_path).unwrap();

        state.attach_archive_v2_reader(
            ArchiveV2Reader::open(
                identity.clone(),
                &catalog_path,
                ArchiveV2ReaderConfig {
                    role: ArchiveV2Role::VerifiedCache,
                    root: archive_root.path().to_path_buf(),
                    cache_root: Some(cache_root.path().to_path_buf()),
                    cache_quota_bytes: bytes.len() as u64 + 1024,
                    max_decoded_segments: 1,
                    allow_remote_fetch: true,
                    sources: vec![Arc::new(ArchiveV2DirectorySource::new(
                        "authenticated-remote",
                        remote_root.path(),
                        true,
                    ))],
                },
            )
            .unwrap(),
        );
        assert_eq!(
            startup_clone.get_block_by_slot(0).unwrap().unwrap().hash(),
            block_hash
        );
        assert_eq!(
            startup_clone.archive_v2_status().unwrap().remote_fetches,
            1,
            "verified-cache must fetch V2 instead of serving the attached legacy cold copy"
        );

        state.attach_archive_v2_reader(
            ArchiveV2Reader::open(
                identity,
                &catalog_path,
                ArchiveV2ReaderConfig {
                    role: ArchiveV2Role::Consensus,
                    root: archive_root.path().to_path_buf(),
                    cache_root: None,
                    cache_quota_bytes: 0,
                    max_decoded_segments: 1,
                    allow_remote_fetch: false,
                    sources: Vec::new(),
                },
            )
            .unwrap(),
        );
        let error = startup_clone.get_block_by_slot(0).unwrap_err();
        assert!(
            error.contains("consensus"),
            "consensus role must deny deep history instead of falling through to cold: {error}"
        );
    }

    #[test]
    fn admitted_full_archive_preserves_canonical_legacy_cold_fallback() {
        let state_root = tempdir().unwrap();
        let cold_root = tempdir().unwrap();
        let archive_root = tempdir().unwrap();
        let mut state = StateStore::open(state_root.path()).unwrap();
        state.open_cold_store(cold_root.path()).unwrap();

        let block = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"archive-v2-full-legacy-fallback-state"),
            [0x27; 32],
            Vec::new(),
            1,
        );
        let block_hash = block.hash();
        state.put_block(&block).unwrap();
        state.migrate_to_cold(1).unwrap();

        let identity = ArchiveV2Identity {
            network_id: "full-legacy-fallback-testnet".to_string(),
            genesis_hash: block_hash,
        };
        let (_, manifest) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &ArchiveV2SegmentContents::from_blocks(vec![block]),
            &ArchiveV2CodecConfig {
                target_frame_bytes: 1024 * 1024,
                ..ArchiveV2CodecConfig::default()
            },
        )
        .unwrap();
        let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
        catalog.append(manifest).unwrap();
        let catalog_path = archive_root.path().join("catalog.av2");
        catalog.store_atomic(&catalog_path).unwrap();

        state.attach_archive_v2_reader(
            ArchiveV2Reader::open(
                identity,
                &catalog_path,
                ArchiveV2ReaderConfig {
                    role: ArchiveV2Role::FullArchive,
                    root: archive_root.path().to_path_buf(),
                    cache_root: None,
                    cache_quota_bytes: 0,
                    max_decoded_segments: 1,
                    allow_remote_fetch: false,
                    sources: Vec::new(),
                },
            )
            .unwrap(),
        );
        state.mark_archive_v2_admitted_after_fresh_sync().unwrap();

        assert_eq!(
            state.get_block_by_slot(0).unwrap().unwrap().hash(),
            block_hash,
            "an admitted full archive must preserve canonical legacy cold fallback until retirement"
        );
        assert_eq!(
            state.archive_v2_status().unwrap().local_hits,
            0,
            "legacy fallback must not report a hit for the unavailable V2 object"
        );
    }

    #[test]
    fn verified_cache_source_outage_does_not_block_new_hot_block_writes() {
        let state_root = tempdir().unwrap();
        let archive_root = tempdir().unwrap();
        let cache_root = tempdir().unwrap();
        let unavailable_remote = tempdir().unwrap();
        let state = StateStore::open(state_root.path()).unwrap();

        let historical = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"archive-v2-write-outage-historical-state"),
            [0x31; 32],
            Vec::new(),
            1,
        );
        state
            .put_block_atomic(&historical, Some(0), Some(0))
            .unwrap();

        let identity = ArchiveV2Identity {
            network_id: "write-outage-testnet".to_string(),
            genesis_hash: historical.hash(),
        };
        let (_, manifest) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &ArchiveV2SegmentContents::from_blocks(vec![historical.clone()]),
            &ArchiveV2CodecConfig {
                target_frame_bytes: 1024 * 1024,
                ..ArchiveV2CodecConfig::default()
            },
        )
        .unwrap();
        let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
        catalog.append(manifest).unwrap();
        let catalog_path = archive_root.path().join("catalog.av2");
        catalog.store_atomic(&catalog_path).unwrap();
        state.attach_archive_v2_reader(
            ArchiveV2Reader::open(
                identity,
                &catalog_path,
                ArchiveV2ReaderConfig {
                    role: ArchiveV2Role::VerifiedCache,
                    root: archive_root.path().to_path_buf(),
                    cache_root: Some(cache_root.path().to_path_buf()),
                    cache_quota_bytes: 1024 * 1024,
                    max_decoded_segments: 1,
                    allow_remote_fetch: true,
                    sources: vec![Arc::new(ArchiveV2DirectorySource::new(
                        "unavailable-remote",
                        unavailable_remote.path(),
                        true,
                    ))],
                },
            )
            .unwrap(),
        );

        assert_eq!(
            state.get_block_by_slot(0).unwrap().unwrap().hash(),
            historical.hash(),
            "an established migration reader must preserve hot-first lookup order"
        );
        state.mark_archive_v2_admitted_after_fresh_sync().unwrap();
        assert!(
            state.get_block_by_slot(0).is_err(),
            "catalog-covered deep history must not fall back to stale hot bytes while the verified source is unavailable"
        );
        assert_eq!(
            state
                .get_hot_block_by_slot(0)
                .expect("read consensus-critical hot history")
                .expect("hot block exists")
                .hash(),
            historical.hash(),
            "consensus-critical recovery must remain independent of an unavailable Archive V2 source"
        );
        let transaction = Transaction::new(Message::new(
            vec![Instruction {
                program_id: Pubkey([0x41; 32]),
                accounts: vec![Pubkey([0x42; 32])],
                data: vec![0x43],
            }],
            historical.hash(),
        ));
        let signature = transaction.signature();
        assert!(
            state.get_tx_meta_full(&signature).is_err(),
            "a public historical lookup must still fail closed while its Archive V2 source is unavailable"
        );
        let source_failures_before = state.archive_v2_status().unwrap().source_failures;

        let next = Block::new_with_timestamp(
            1,
            historical.hash(),
            Hash::hash(b"archive-v2-write-outage-next-state"),
            [0x32; 32],
            vec![transaction],
            2,
        );
        state
            .put_block_atomic(&next, Some(1), Some(1))
            .expect("new hot writes must not depend on an unavailable deep-history source");

        assert_eq!(state.get_last_slot().unwrap(), 1);
        assert_eq!(
            state.archive_v2_status().unwrap().source_failures,
            source_failures_before,
            "hot block indexing must not issue a deep-history fetch"
        );
    }

    #[test]
    fn dex_trade_slot_export_ignores_metadata_and_non_dex_contracts() {
        let root = tempdir().unwrap();
        let state = StateStore::open(root.path()).unwrap();
        let dex_program = Pubkey([0x41; 32]);
        state
            .register_symbol(
                "DEX",
                SymbolRegistryEntry {
                    symbol: "DEX".to_string(),
                    program: dex_program,
                    owner: Pubkey([0x42; 32]),
                    name: None,
                    template: None,
                    metadata: None,
                    decimals: None,
                },
            )
            .unwrap();
        let contract_storage = state.db.cf_handle(CF_CONTRACT_STORAGE).unwrap();
        let put_storage = |program: Pubkey, storage_key: &[u8], value: &[u8]| {
            let mut key = Vec::with_capacity(32 + storage_key.len());
            key.extend_from_slice(&program.0);
            key.extend_from_slice(storage_key);
            state.db.put_cf(&contract_storage, key, value).unwrap();
        };

        put_storage(
            dex_program,
            crate::dex::DEX_TRADE_COUNT_KEY.as_bytes(),
            &1u64.to_le_bytes(),
        );
        put_storage(Pubkey([0x43; 32]), b"dex_trade_99", b"not-a-dex-trade");
        let mut trade = vec![0u8; 80];
        trade[0..8].copy_from_slice(&1u64.to_le_bytes());
        trade[72..80].copy_from_slice(&77u64.to_le_bytes());
        put_storage(dex_program, crate::dex::trade_key(1).as_bytes(), &trade);

        assert_eq!(
            state.archive_v2_dex_trade_slots().unwrap(),
            BTreeMap::from([(1, 77)])
        );

        put_storage(dex_program, b"dex_trade_2", b"malformed-canonical-trade");
        assert!(state
            .archive_v2_dex_trade_slots()
            .unwrap_err()
            .contains("cannot decode DEX trade"));
    }

    #[test]
    fn tx_meta_export_deduplicates_repeated_signature_at_authoritative_slot() {
        let root = tempdir().unwrap();
        let state = StateStore::open(root.path()).unwrap();
        let transaction = Transaction::new(Message::new(
            Vec::new(),
            Hash::hash(b"archive-v2-repeated-signature"),
        ));
        let signature = transaction.signature();
        let first = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"archive-v2-repeated-state-0"),
            [0x51; 32],
            vec![transaction.clone()],
            1,
        );
        let second = Block::new_with_timestamp(
            1,
            first.hash(),
            Hash::hash(b"archive-v2-repeated-state-1"),
            [0x52; 32],
            vec![transaction],
            2,
        );
        state.put_block_atomic(&first, Some(0), Some(0)).unwrap();
        state.put_block_atomic(&second, Some(1), Some(1)).unwrap();
        state.put_tx_meta(&signature, 7).unwrap();
        assert_eq!(state.get_tx_slot(&signature).unwrap(), Some(1));

        let all = state.archive_v2_public_categories(0, 1).unwrap();
        let rows = all.get("tx_meta").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].slot, 1);
        assert_eq!(rows[0].key, signature.0);
        assert!(state
            .archive_v2_public_categories(0, 0)
            .unwrap()
            .get("tx_meta")
            .unwrap()
            .is_empty());
    }

    #[test]
    fn segment_export_derives_exact_account_txs_from_canonical_blocks() {
        let root = tempdir().unwrap();
        let cold_root = tempdir().unwrap();
        let mut state = StateStore::open(root.path()).unwrap();
        state.open_cold_store(cold_root.path()).unwrap();
        let first_transaction = Transaction::new(Message::new(
            vec![Instruction {
                program_id: Pubkey([0x61; 32]),
                accounts: vec![Pubkey([0x62; 32]), Pubkey([0x63; 32])],
                data: vec![0x64],
            }],
            Hash::hash(b"archive-v2-derived-account-txs-0"),
        ));
        let second_transaction = Transaction::new(Message::new(
            vec![Instruction {
                program_id: Pubkey([0x65; 32]),
                accounts: vec![Pubkey([0x66; 32])],
                data: vec![0x67],
            }],
            Hash::hash(b"archive-v2-derived-account-txs-1"),
        ));
        let first = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"archive-v2-derived-account-state-0"),
            [0x68; 32],
            vec![first_transaction],
            1,
        );
        let second = Block::new_with_timestamp(
            1,
            first.hash(),
            Hash::hash(b"archive-v2-derived-account-state-1"),
            [0x69; 32],
            vec![second_transaction],
            2,
        );
        state.put_block_atomic(&first, Some(0), Some(0)).unwrap();
        state.put_block_atomic(&second, Some(1), Some(1)).unwrap();
        let (_, cold_only_key) =
            super::super::secondary_indexes::account_tx_index_entries_for_block(&first)
                .into_iter()
                .next()
                .unwrap();
        state
            .cold_db
            .as_ref()
            .unwrap()
            .put_cf(
                &state
                    .cold_db
                    .as_ref()
                    .unwrap()
                    .cf_handle(COLD_CF_ACCOUNT_TXS)
                    .unwrap(),
                &cold_only_key,
                [],
            )
            .unwrap();
        state
            .db
            .delete_cf(&state.db.cf_handle(CF_ACCOUNT_TXS).unwrap(), &cold_only_key)
            .unwrap();

        let scanned = state.archive_v2_public_categories(0, 1).unwrap();
        let exported = state.export_archive_v2_segment_contents(0, 1).unwrap();
        assert_eq!(
            exported.public_categories.get("account_txs"),
            scanned.get("account_txs")
        );
        assert_eq!(
            exported.public_categories["account_txs"].len(),
            super::super::secondary_indexes::account_tx_index_entries_for_block(&first).len()
                + super::super::secondary_indexes::account_tx_index_entries_for_block(&second)
                    .len()
        );
    }

    #[test]
    fn segment_export_fails_when_a_derived_account_tx_source_row_is_missing() {
        let root = tempdir().unwrap();
        let state = StateStore::open(root.path()).unwrap();
        let transaction = Transaction::new(Message::new(
            vec![Instruction {
                program_id: Pubkey([0x71; 32]),
                accounts: vec![Pubkey([0x72; 32])],
                data: vec![0x73],
            }],
            Hash::hash(b"archive-v2-missing-derived-account-tx"),
        ));
        let block = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"archive-v2-missing-derived-account-state"),
            [0x74; 32],
            vec![transaction],
            1,
        );
        state.put_block_atomic(&block, Some(0), Some(0)).unwrap();
        let (_, key) = super::super::secondary_indexes::account_tx_index_entries_for_block(&block)
            .into_iter()
            .next()
            .unwrap();
        state
            .db
            .delete_cf(&state.db.cf_handle(CF_ACCOUNT_TXS).unwrap(), key)
            .unwrap();

        let error = state.export_archive_v2_segment_contents(0, 0).unwrap_err();
        assert!(
            error.contains("account_txs key") && error.contains("missing from the legacy source"),
            "unexpected error: {error}"
        );
    }
}
