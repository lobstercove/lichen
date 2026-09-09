use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;

use super::snapshot_io::{
    append_canonical_tx_manifest_entries, canonical_block_snapshot_value_from_block,
    public_history_manifest_root, PublicHistoryDigestAccumulator,
    CANONICAL_LEDGER_MANIFEST_CATEGORIES,
};
use super::{
    CheckpointSnapshotProfile, KvEntries, KvPage, PublicHistoryCategoryDigest,
    PublicHistoryManifest, StateStore, CF_ACCOUNT_TXS, COLD_CF_ACCOUNT_TXS,
    PUBLIC_HISTORY_SNAPSHOT_CATEGORIES,
};
use crate::archive_v2::{
    ArchiveV2Catalog, ArchiveV2PublicRow, ArchiveV2Reader, ArchiveV2ReaderStatus, ArchiveV2Role,
    ArchiveV2Rows, ArchiveV2SegmentContents,
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
const CHECKPOINT_HISTORY_FILTER_SCAN_PAGE_ROWS: u64 = 10_000;
const MANIFEST_RAW_PAGE_BYTES: usize = 64 * 1024 * 1024;
const MANIFEST_SEGMENT_PAYLOAD_BYTES: u64 = 1024 * 1024 * 1024;
type ManifestRow = (Vec<u8>, Vec<u8>);

/// Ephemeral lookup for global rows. Its memtables and block cache are
/// bounded independently of history length; the operator must provision the
/// temporary filesystem for the index and compaction output. No source state
/// is changed. Keep the DB before the directory so it closes before cleanup.
#[derive(Debug)]
struct ManifestDiskLookup {
    db: rocksdb::DB,
    _directory: tempfile::TempDir,
    category: String,
}

impl ManifestDiskLookup {
    fn new(category: &str) -> Result<Self, String> {
        let directory = tempfile::Builder::new()
            .prefix("lichen-manifest-")
            .tempdir()
            .map_err(|error| format!("create manifest scratch: {error}"))?;
        let mut options = rocksdb::Options::default();
        options.create_if_missing(true);
        options.set_write_buffer_size(16 * 1024 * 1024);
        options.set_max_write_buffer_number(2);
        options.set_max_background_jobs(1);
        options.set_max_open_files(32);
        let mut table = rocksdb::BlockBasedOptions::default();
        table.disable_cache();
        options.set_block_based_table_factory(&table);
        let db = rocksdb::DB::open(&options, directory.path())
            .map_err(|error| format!("open manifest scratch: {error}"))?;
        Ok(Self {
            db,
            _directory: directory,
            category: category.to_string(),
        })
    }

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, String> {
        self.db
            .get(key)
            .map_err(|error| format!("read manifest scratch: {error}"))
    }

    fn insert(&self, key: &[u8], value: &[u8]) -> Result<(), String> {
        if key.len().saturating_add(value.len()) > MANIFEST_RAW_PAGE_BYTES {
            return Err("manifest scratch row exceeds payload limit".to_string());
        }
        if let Some(existing) = self.get(key)? {
            if existing != value {
                return Err(format!(
                    "{} has conflicting rows at key {}",
                    self.category,
                    hex::encode(key)
                ));
            }
            return Ok(());
        }
        let mut options = rocksdb::WriteOptions::default();
        // This index is rebuilt on every invocation and is never recovery data.
        options.disable_wal(true);
        self.db
            .put_opt(key, value, &options)
            .map_err(|error| format!("write manifest scratch: {error}"))
    }

    fn archive_prefix(
        reader: &ArchiveV2Reader,
        category: &str,
        end_slot: Option<u64>,
    ) -> Result<Self, String> {
        let lookup = Self::new(category)?;
        if let Some(end_slot) = end_slot {
            reader
                .visit_category_rows(
                    category,
                    0,
                    end_slot,
                    MANIFEST_SEGMENT_PAYLOAD_BYTES,
                    |row| {
                        lookup
                            .insert(&row.key, &row.value)
                            .map_err(crate::archive_v2::ArchiveV2Error::Ordering)
                    },
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(lookup)
    }

    fn page(&self, after_key: Option<&[u8]>, max_rows: u64) -> Result<KvPage, String> {
        let limit = max_rows.clamp(1, ARCHIVE_V2_EXPORT_PAGE_ROWS) as usize;
        let mode = after_key.map_or(rocksdb::IteratorMode::Start, |key| {
            rocksdb::IteratorMode::From(key, rocksdb::Direction::Forward)
        });
        let mut entries = Vec::new();
        let mut bytes = 0usize;
        let mut has_more = false;
        for row in self.db.iterator(mode) {
            let (key, value) = row.map_err(|error| format!("iterate manifest scratch: {error}"))?;
            if after_key.is_some_and(|after| key.as_ref() <= after) {
                continue;
            }
            let row_bytes = key.len().saturating_add(value.len());
            if row_bytes > MANIFEST_RAW_PAGE_BYTES {
                return Err("manifest raw row exceeds payload limit".to_string());
            }
            if entries.len() == limit || bytes.saturating_add(row_bytes) > MANIFEST_RAW_PAGE_BYTES {
                has_more = true;
                break;
            }
            bytes += row_bytes;
            entries.push((key.into_vec(), value.into_vec()));
        }
        Ok(KvPage {
            next_cursor: if has_more {
                entries.last().map(|(key, _)| key.clone())
            } else {
                None
            },
            entries,
            total: 0,
            has_more,
        })
    }
}

fn validate_manifest_page_size(entries: &KvEntries) -> Result<(), String> {
    if entries.len() > ARCHIVE_V2_EXPORT_PAGE_ROWS as usize {
        return Err("manifest raw page exceeds row limit".to_string());
    }
    let bytes = entries
        .iter()
        .try_fold(0usize, |total, (key, value)| {
            total
                .checked_add(key.len())
                .and_then(|n| n.checked_add(value.len()))
        })
        .ok_or_else(|| "manifest raw page byte count overflow".to_string())?;
    if bytes > MANIFEST_RAW_PAGE_BYTES {
        return Err("manifest raw page exceeds payload limit".to_string());
    }
    Ok(())
}

/// Two independently ordered partitions retain at most one page each. Validate
/// exclusive continuation before folding any page into the existing digest.
/// Producer decoding and canonical-ledger traversal have separate memory bounds.
struct ManifestRowPages<F> {
    fetch: F,
    rows: VecDeque<ManifestRow>,
    cursor: Option<Vec<u8>>,
    finished: bool,
}

impl<F> ManifestRowPages<F>
where
    F: FnMut(Option<&[u8]>) -> Result<KvPage, String>,
{
    fn new(fetch: F) -> Self {
        Self {
            fetch,
            rows: VecDeque::new(),
            cursor: None,
            finished: false,
        }
    }

    fn next(&mut self) -> Result<Option<ManifestRow>, String> {
        if self.rows.is_empty() && !self.finished {
            let page = (self.fetch)(self.cursor.as_deref())?;
            let mut previous = self.cursor.as_deref();
            validate_manifest_page_size(&page.entries)?;
            for (key, _) in &page.entries {
                if previous.is_some_and(|prior| key.as_slice() <= prior) {
                    return Err(
                        "manifest raw page does not advance in exclusive key order".to_string()
                    );
                }
                previous = Some(key);
            }
            let last = page.entries.last().map(|(key, _)| key.clone());
            if page.has_more && (last.is_none() || page.next_cursor != last) {
                return Err("manifest raw page has invalid continuation".to_string());
            }
            self.cursor = last;
            self.finished = !page.has_more;
            self.rows = page.entries.into();
        }
        Ok(self.rows.pop_front())
    }
}

fn merge_manifest_row_pages<P, S>(
    category: &str,
    prefix: P,
    suffix: S,
) -> Result<PublicHistoryCategoryDigest, String>
where
    P: FnMut(Option<&[u8]>) -> Result<KvPage, String>,
    S: FnMut(Option<&[u8]>) -> Result<KvPage, String>,
{
    let mut prefix = ManifestRowPages::new(prefix);
    let mut suffix = ManifestRowPages::new(suffix);
    let mut left = prefix.next()?;
    let mut right = suffix.next()?;
    let mut accumulator = PublicHistoryDigestAccumulator::new(category);
    while left.is_some() || right.is_some() {
        let ordering = match (&left, &right) {
            (Some((a, _)), Some((b, _))) => a.cmp(b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => unreachable!(),
        };
        match ordering {
            std::cmp::Ordering::Less => {
                let (key, value) = left.take().expect("prefix row exists");
                accumulator.push(&key, &value);
                left = prefix.next()?;
            }
            std::cmp::Ordering::Greater => {
                let (key, value) = right.take().expect("suffix row exists");
                accumulator.push(&key, &value);
                right = suffix.next()?;
            }
            std::cmp::Ordering::Equal => {
                let (key, value) = left.take().expect("prefix row exists");
                let (_, other) = right.take().expect("suffix row exists");
                if value != other {
                    return Err(format!(
                        "{category} has conflicting prefix/suffix rows at key {}",
                        hex::encode(key)
                    ));
                }
                accumulator.push(&key, &value);
                left = prefix.next()?;
                right = suffix.next()?;
            }
        }
    }
    Ok(accumulator.finish())
}

fn archive_v2_declared_gap_covers(
    catalog: &ArchiveV2Catalog,
    start_slot: u64,
    end_slot: u64,
) -> bool {
    if end_slot < start_slot {
        return true;
    }
    let mut cursor = start_slot;
    for declaration in &catalog.legacy_loss_declarations {
        if declaration.end_slot < cursor {
            continue;
        }
        if declaration.start_slot > cursor {
            return false;
        }
        if declaration.end_slot >= end_slot {
            return true;
        }
        cursor = declaration.end_slot.saturating_add(1);
    }
    false
}

impl StateStore {
    /// Compute one partition-independent public-history manifest for a
    /// catalog-bound hot checkpoint. The immutable Archive V2 prefix and the
    /// checkpoint's recent hot suffix are authenticated independently, then
    /// folded into the same logical category order used by the legacy
    /// genesis-to-tip manifest.
    ///
    /// This deliberately accepts the reader separately instead of attaching
    /// it to the checkpoint state. Offline verification must not change the
    /// checkpoint's runtime role or let ordinary hot iterators silently fall
    /// through to a different physical history tier.
    ///
    /// Global row ordering and metadata lookup use ephemeral disk indexes in
    /// the standard temporary directory. Operators must provision that
    /// filesystem for scratch/compaction output and allow writes there (for
    /// restricted units, set TMPDIR to a private writable scratch directory).
    /// Source databases remain read-only. Segment payload limits and bounded
    /// scratch buffers are not a substitute for whole-process memory admission.
    pub fn compute_archive_v2_checkpoint_public_history_manifest(
        &self,
        reader: &ArchiveV2Reader,
        snapshot_slot: u64,
        profile: CheckpointSnapshotProfile,
        categories: &[&str],
        page_size: u64,
    ) -> Result<PublicHistoryManifest, String> {
        let CheckpointSnapshotProfile::HotRepairV1 {
            history_start_slot,
            archive_v2_catalog_root: Some(bound_handoff_root),
        } = profile
        else {
            return Err(
                "Archive V2 logical manifest requires a catalog-bound hot_repair_v1 checkpoint"
                    .to_string(),
            );
        };
        if history_start_slot > snapshot_slot {
            return Err(format!(
                "checkpoint history start {history_start_slot} exceeds snapshot slot {snapshot_slot}"
            ));
        }
        if categories.is_empty() {
            return Err("Archive V2 logical manifest requires at least one category".to_string());
        }
        let mut unique_categories = std::collections::BTreeSet::new();
        for category in categories {
            if !PUBLIC_HISTORY_SNAPSHOT_CATEGORIES.contains(category) {
                return Err(format!(
                    "Unsupported Archive V2 logical public-history category: {category}"
                ));
            }
            if !unique_categories.insert(*category) {
                return Err(format!(
                    "Archive V2 logical public-history category is duplicated: {category}"
                ));
            }
        }

        let catalog = reader.catalog();
        catalog.validate().map_err(|error| error.to_string())?;
        let actual_handoff_root = catalog
            .checkpoint_handoff_root(history_start_slot)
            .map_err(|error| error.to_string())?;
        if actual_handoff_root.0 != bound_handoff_root {
            return Err(format!(
                "checkpoint Archive V2 handoff {} differs from supplied catalog handoff {}",
                hex::encode(bound_handoff_root),
                actual_handoff_root
            ));
        }

        let prefix_end = history_start_slot.checked_sub(1);
        let page_size = page_size.max(1);
        let hot_profile = CheckpointSnapshotProfile::HotRepairV1 {
            history_start_slot,
            archive_v2_catalog_root: Some(bound_handoff_root),
        };

        let expected_hot_slots = snapshot_slot
            .checked_sub(history_start_slot)
            .and_then(|span| span.checked_add(1))
            .ok_or_else(|| "hot checkpoint slot span overflow".to_string())?;
        let tx_meta = if categories.contains(&"tx_meta") {
            // Preserve global conflict semantics for repeated signatures
            // without retaining every historical row in process memory.
            let merged = ManifestDiskLookup::archive_prefix(reader, "tx_meta", prefix_end)?;
            // Canonical metadata exports use a slot/index continuation, not
            // the signature key of the returned row. Preserve that cursor.
            let mut cursor: Option<Vec<u8>> = None;
            loop {
                let page = self.export_checkpoint_snapshot_category_cursor_untracked(
                    "tx_meta",
                    cursor.as_deref(),
                    1,
                    snapshot_slot,
                    hot_profile,
                )?;
                validate_manifest_page_size(&page.entries)?;
                if page.has_more
                    && (page.entries.is_empty()
                        || page.next_cursor.as_ref().is_none_or(|next| {
                            cursor.as_ref().is_some_and(|previous| next <= previous)
                        }))
                {
                    return Err("tx_meta hot-checkpoint continuation does not advance".to_string());
                }
                for (key, value) in page.entries {
                    merged.insert(&key, &value)?;
                }
                if !page.has_more {
                    break;
                }
                cursor = page.next_cursor;
            }
            Some(merged)
        } else {
            None
        };
        let mut canonical_accumulators = BTreeMap::new();
        for category in categories
            .iter()
            .copied()
            .filter(|category| CANONICAL_LEDGER_MANIFEST_CATEGORIES.contains(category))
        {
            canonical_accumulators.insert(
                category.to_string(),
                PublicHistoryDigestAccumulator::new(category),
            );
        }

        let mut previous_present_block: Option<(u64, crate::Hash)> = None;
        let mut expected_slot = 0u64;
        let mut fold_block = |block: &Block| -> Result<(), String> {
            let slot = block.header.slot;
            let block_hash = block.hash();
            if slot < expected_slot || slot > snapshot_slot {
                return Err(format!(
                    "composed canonical block {slot} is out of order or beyond checkpoint"
                ));
            }
            if slot > expected_slot
                && (slot > history_start_slot
                    || !archive_v2_declared_gap_covers(catalog, expected_slot, slot - 1))
            {
                return Err(format!("Archive V2 logical manifest has an undeclared canonical gap {expected_slot}..={} before checkpoint handoff", slot - 1));
            }
            if let Some((previous_slot, previous_hash)) = previous_present_block {
                if slot == previous_slot.saturating_add(1)
                    && block.header.parent_hash != previous_hash
                {
                    return Err(format!(
                        "composed canonical parent mismatch between slots {previous_slot} and {slot}"
                    ));
                }
            }
            previous_present_block = Some((slot, block_hash));
            expected_slot = slot.saturating_add(1);

            if let Some(accumulator) = canonical_accumulators.get_mut("slots") {
                accumulator.push(&slot.to_be_bytes(), &block_hash.0);
            }
            if let Some(accumulator) = canonical_accumulators.get_mut("blocks") {
                let mut manifest_block = block.clone();
                manifest_block.commit_round = 0;
                manifest_block.commit_signatures.clear();
                let value = canonical_block_snapshot_value_from_block(manifest_block)?;
                accumulator.push(&block_hash.0, &value);
            }
            for (tx_index, transaction) in block.transactions.iter().enumerate() {
                let tx_hash = transaction.signature();
                let metadata = tx_meta
                    .as_ref()
                    .map(|rows| rows.get(tx_hash.0.as_slice()))
                    .transpose()?
                    .flatten();
                append_canonical_tx_manifest_entries(
                    &mut canonical_accumulators,
                    slot,
                    tx_index as u64,
                    tx_hash,
                    transaction,
                    metadata.as_deref(),
                )?;
            }
            Ok(())
        };
        if let Some(end_slot) = prefix_end {
            reader
                .visit_segments_in_range(0, end_slot, MANIFEST_SEGMENT_PAYLOAD_BYTES, |segment| {
                    for block in &segment.blocks {
                        if block.header.slot <= end_slot {
                            fold_block(block)
                                .map_err(crate::archive_v2::ArchiveV2Error::Ordering)?;
                        }
                    }
                    Ok(())
                })
                .map_err(|error| error.to_string())?;
        }
        let mut hot_slots = ManifestRowPages::new(|after_key| {
            self.export_checkpoint_snapshot_category_cursor_untracked(
                "slots",
                after_key,
                page_size.min(ARCHIVE_V2_EXPORT_PAGE_ROWS),
                snapshot_slot,
                hot_profile,
            )
        });
        let mut hot_slot_count = 0u64;
        while let Some((slot_key, hash_value)) = hot_slots.next()? {
            if slot_key.len() != 8 || hash_value.len() != 32 {
                return Err(format!(
                    "composed canonical slot row has invalid key/value lengths {}/{}",
                    slot_key.len(),
                    hash_value.len()
                ));
            }
            let slot =
                u64::from_be_bytes(slot_key.as_slice().try_into().expect("validated slot key"));
            if hot_slot_count >= expected_hot_slots || slot != history_start_slot + hot_slot_count {
                return Err(format!("hot checkpoint canonical slots are not contiguous at offset {hot_slot_count}: slot {slot}"));
            }
            let block = self
                .get_block_by_slot(slot)?
                .ok_or_else(|| format!("composed canonical block {slot} is unavailable"))?;
            if block.header.slot != slot || block.hash().0.as_slice() != hash_value.as_slice() {
                return Err(format!(
                    "composed canonical slot {slot} differs from its block"
                ));
            }
            fold_block(&block)?;
            hot_slot_count += 1;
        }
        if hot_slot_count != expected_hot_slots {
            return Err(format!("hot checkpoint has {hot_slot_count} canonical slots, expected {expected_hot_slots}"));
        }
        if previous_present_block.is_none_or(|(slot, _)| slot != snapshot_slot) {
            return Err(format!(
                "composed canonical slots do not end at checkpoint slot {snapshot_slot}"
            ));
        }

        // Metadata is no longer needed once the canonical walk is complete.
        // Release its scratch files before indexing the remaining categories.
        drop(tx_meta);
        let mut canonical_digests = canonical_accumulators
            .into_iter()
            .map(|(category, accumulator)| (category, accumulator.finish()))
            .collect::<BTreeMap<_, _>>();
        let mut digests = Vec::with_capacity(categories.len());
        let dex_slots = if categories.iter().any(|category| {
            matches!(
                *category,
                "dex_trades_by_pair" | "dex_trades_by_taker" | "dex_trades_by_pair_taker"
            )
        }) {
            self.manifest_dex_trade_slots()?
        } else {
            None
        };
        for category in categories {
            if let Some(digest) = canonical_digests.remove(*category) {
                digests.push(digest);
                continue;
            }
            let prefix = ManifestDiskLookup::archive_prefix(reader, category, prefix_end)?;
            digests.push(merge_manifest_row_pages(
                category,
                |after_key| prefix.page(after_key, page_size),
                |after_key| {
                    self.manifest_hot_raw_page(
                        category,
                        after_key,
                        page_size.min(ARCHIVE_V2_EXPORT_PAGE_ROWS),
                        snapshot_slot,
                        hot_profile,
                        &dex_slots,
                    )
                },
            )?);
        }
        let root = public_history_manifest_root(&digests);
        Ok(PublicHistoryManifest {
            schema_version: 1,
            categories: digests,
            root,
        })
    }

    /// Read one source row at a time before adding it to a byte-bounded page.
    /// The checkpoint contains only its own hot DB; archive-prefix rows are
    /// merged separately. Reuse the DEX lookup across every category/page.
    fn manifest_hot_raw_page(
        &self,
        category: &str,
        after_key: Option<&[u8]>,
        limit: u64,
        snapshot_slot: u64,
        profile: CheckpointSnapshotProfile,
        dex_slots: &Option<ManifestDiskLookup>,
    ) -> Result<KvPage, String> {
        let CheckpointSnapshotProfile::HotRepairV1 {
            history_start_slot, ..
        } = profile
        else {
            return Err("manifest hot page requires hot_repair_v1".to_string());
        };
        let mut cursor = after_key.map(ToOwned::to_owned);
        let mut entries: KvEntries = Vec::new();
        let mut bytes = 0usize;
        loop {
            let page = self.export_snapshot_category_cursor_untracked_with_source(
                category,
                cursor.as_deref(),
                1,
                true,
            )?;
            if page.has_more
                && (page.entries.is_empty()
                    || page.next_cursor.as_ref().is_none_or(|next| {
                        cursor.as_ref().is_some_and(|previous| next <= previous)
                    }))
            {
                return Err(format!("{category} hot manifest cursor does not advance"));
            }
            for (key, value) in page.entries {
                let slot = self
                    .archive_v2_public_row_slot(category, &key, &value, &|id: u64| {
                        dex_slots
                            .as_ref()
                            .map(|slots| slots.get(&id.to_be_bytes()))
                            .transpose()?
                            .flatten()
                            .map(|bytes| {
                                bytes
                                    .as_slice()
                                    .try_into()
                                    .map(u64::from_be_bytes)
                                    .map_err(|_| "invalid DEX trade slot scratch value".to_string())
                            })
                            .transpose()
                    })?
                    .ok_or_else(|| format!("hot manifest cannot determine {category} row slot"))?;
                if !(history_start_slot..=snapshot_slot).contains(&slot) {
                    continue;
                }
                let row_bytes = key.len().saturating_add(value.len());
                if row_bytes > MANIFEST_RAW_PAGE_BYTES {
                    return Err("hot manifest row exceeds payload limit".to_string());
                }
                if entries.len() as u64 == limit
                    || bytes.saturating_add(row_bytes) > MANIFEST_RAW_PAGE_BYTES
                {
                    return Ok(KvPage {
                        next_cursor: entries.last().map(|(key, _)| key.clone()),
                        entries,
                        total: 0,
                        has_more: true,
                    });
                }
                bytes += row_bytes;
                entries.push((key, value));
            }
            if !page.has_more {
                return Ok(KvPage {
                    entries,
                    total: 0,
                    next_cursor: None,
                    has_more: false,
                });
            }
            cursor = page.next_cursor;
        }
    }

    /// Export one checkpoint snapshot page under the checkpoint's advertised
    /// recovery profile.
    ///
    /// Full-archive checkpoints preserve the legacy export byte-for-byte. A
    /// hot-repair checkpoint carries all current state, but only public-history
    /// rows in its inclusive recent-history window. Keeping this filter in the
    /// shared StateStore surface makes manifest hashing and P2P chunk serving
    /// use exactly the same cursor and row-selection semantics.
    pub fn export_checkpoint_snapshot_category_cursor_untracked(
        &self,
        category: &str,
        after_key: Option<&[u8]>,
        limit: u64,
        snapshot_slot: u64,
        profile: CheckpointSnapshotProfile,
    ) -> Result<KvPage, String> {
        let CheckpointSnapshotProfile::HotRepairV1 {
            history_start_slot, ..
        } = profile
        else {
            return self.export_snapshot_category_cursor_untracked(category, after_key, limit);
        };

        if history_start_slot > snapshot_slot {
            return Err(format!(
                "hot-repair checkpoint history start {} exceeds snapshot slot {}",
                history_start_slot, snapshot_slot
            ));
        }
        if !PUBLIC_HISTORY_SNAPSHOT_CATEGORIES.contains(&category) {
            return self.export_snapshot_category_cursor_untracked(category, after_key, limit);
        }
        if limit == 0 {
            return Ok(KvPage {
                entries: Vec::new(),
                total: 0,
                next_cursor: None,
                has_more: false,
            });
        }

        if matches!(
            category,
            "slots" | "blocks" | "transactions" | "tx_by_slot" | "tx_to_slot" | "tx_meta"
        ) {
            let initial_cursor = if after_key.is_none() && history_start_slot > 0 {
                if matches!(category, "slots" | "blocks") {
                    Some(history_start_slot.saturating_sub(1).to_be_bytes().to_vec())
                } else {
                    let mut cursor = history_start_slot.saturating_sub(1).to_be_bytes().to_vec();
                    cursor.extend_from_slice(&u64::MAX.to_be_bytes());
                    Some(cursor)
                }
            } else {
                None
            };
            let mut page = self.export_public_history_category_range_cursor_untracked_with_source(
                category,
                after_key.or(initial_cursor.as_deref()),
                limit,
                Some(snapshot_slot),
                true,
            )?;
            if category == "blocks" {
                // A hot-repair checkpoint is anchored by its own certified
                // snapshot. Per-block commit certificates are node-local
                // finality-proof subsets and are not part of a block hash, so
                // exclude them from the portable checkpoint bytes as well as
                // from public-history manifests.
                for (key, value) in &mut page.entries {
                    *value = super::snapshot_io::public_history_manifest_block_value(key, value)?;
                }
            }
            return Ok(page);
        }

        if !ARCHIVE_V2_RAW_PUBLIC_CATEGORIES.contains(&category) {
            return Err(format!(
                "hot-repair checkpoint has no bounded exporter for public-history category {category}"
            ));
        }

        let dex_trade_slots = if matches!(
            category,
            "dex_trades_by_pair" | "dex_trades_by_taker" | "dex_trades_by_pair_taker"
        ) {
            self.archive_v2_dex_trade_slots()?
        } else {
            BTreeMap::new()
        };
        let mut scan_cursor = after_key.map(ToOwned::to_owned);
        let mut entries: Vec<(Vec<u8>, Vec<u8>)> = Vec::with_capacity(limit.min(10_000) as usize);
        loop {
            let page = self.export_snapshot_category_cursor_untracked_with_source(
                category,
                scan_cursor.as_deref(),
                CHECKPOINT_HISTORY_FILTER_SCAN_PAGE_ROWS.max(limit),
                true,
            )?;
            for (key, value) in page.entries {
                let slot = self
                    .archive_v2_public_row_slot(category, &key, &value, &|id| {
                        Ok(dex_trade_slots.get(&id).copied())
                    })?
                    .ok_or_else(|| {
                        format!(
                            "hot-repair checkpoint cannot determine the slot for {category} key {}",
                            hex::encode(&key)
                        )
                    })?;
                if !(history_start_slot..=snapshot_slot).contains(&slot) {
                    continue;
                }
                if entries.len() == limit as usize {
                    return Ok(KvPage {
                        next_cursor: entries.last().map(|(key, _)| key.clone()),
                        entries,
                        total: 0,
                        has_more: true,
                    });
                }
                entries.push((key, value));
            }
            if !page.has_more {
                return Ok(KvPage {
                    entries,
                    total: 0,
                    next_cursor: None,
                    has_more: false,
                });
            }
            scan_cursor = Some(page.next_cursor.ok_or_else(|| {
                format!("{category} checkpoint filter has more rows but no cursor")
            })?);
        }
    }

    pub fn attach_archive_v2_reader(&self, reader: ArchiveV2Reader) {
        *self
            .archive_v2_reader
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(reader));
        *self
            .archive_v2_deferred_checkpoint_catalog
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
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

    /// Install an already validated configured catalog as a checkpoint-only
    /// fresh-join trust input. This does not attach a reader, enable public
    /// history, or mark the Archive V2 role admitted.
    pub fn attach_archive_v2_deferred_checkpoint_catalog(&self, catalog: ArchiveV2Catalog) {
        *self
            .archive_v2_deferred_checkpoint_catalog
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(catalog));
    }

    /// Return the genesis hash committed by the validated catalog retained for
    /// a fresh Archive V2 join. This exposes only the chain-identity anchor,
    /// not an Archive V2 reader or any unadmitted public-history capability.
    pub fn archive_v2_deferred_genesis_hash(&self) -> Option<Hash> {
        self.archive_v2_deferred_checkpoint_catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|catalog| catalog.identity.genesis_hash)
    }

    /// Return the append-stable configured catalog handoff root only when it
    /// covers the full checkpoint predecessor range. This is the narrow fresh-
    /// join analogue of `archive_v2_checkpoint_catalog_root`; it never exposes
    /// a reader.
    pub fn archive_v2_deferred_checkpoint_catalog_root(
        &self,
        history_start_slot: u64,
    ) -> Result<Option<Hash>, String> {
        let catalog = self
            .archive_v2_deferred_checkpoint_catalog
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(catalog) = catalog else {
            return Ok(None);
        };
        if let Some(required_end) = history_start_slot.checked_sub(1) {
            if !catalog
                .covers_genesis_through(required_end)
                .map_err(|error| error.to_string())?
            {
                return Err(format!(
                    "Deferred Archive V2 catalog {} does not cover genesis through hot checkpoint predecessor slot {}",
                    catalog.catalog_root, required_end
                ));
            }
        } else {
            catalog.validate().map_err(|error| error.to_string())?;
        }
        catalog
            .checkpoint_handoff_root(history_start_slot)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    /// Return the append-stable verified Archive V2 handoff root that
    /// authenticates history older than a hot-repair checkpoint's retained
    /// window.
    ///
    /// Reader attachment has already verified network/genesis identity,
    /// signatures, continuity, and the testnet-only loss declaration.
    /// Rechecking coverage here prevents a checkpoint from claiming an Archive
    /// V2 handoff across a gap that its attached catalog does not cover.
    pub fn archive_v2_checkpoint_catalog_root(
        &self,
        history_start_slot: u64,
    ) -> Result<Option<Hash>, String> {
        let Some(reader) = self.archive_v2_reader() else {
            return Ok(None);
        };
        reader
            .checkpoint_handoff_root(history_start_slot)
            .map(Some)
            .map_err(|error| error.to_string())
    }

    /// Select the earliest public-history slot a catalog-bound hot checkpoint
    /// must retain. The configured hot window is a minimum: when publication
    /// trails the live chain, the checkpoint also carries the unpublished tail
    /// immediately after the admitted catalog instead of creating a recovery
    /// gap. The caller-supplied bound keeps a stale catalog from silently
    /// turning a hot checkpoint into an indefinitely growing full archive.
    pub fn archive_v2_checkpoint_handoff(
        &self,
        nominal_history_start_slot: u64,
        max_unpublished_extension_slots: u64,
    ) -> Result<Option<(u64, Hash)>, String> {
        let Some(reader) = self.archive_v2_reader() else {
            return Ok(None);
        };
        if !reader.status().admitted_after_fresh_sync {
            return Err(
                "Archive V2 checkpoint handoff requires a catalog admitted after fresh sync"
                    .to_string(),
            );
        }
        let catalog = reader.catalog();
        let catalog_coverage_end = match catalog.trailing_loss_declaration() {
            Ok(Some(declaration)) => Some(declaration.end_slot),
            Ok(None) => catalog.entries.last().map(|entry| entry.manifest.end_slot),
            Err(error) => return Err(error.to_string()),
        };
        let first_unpublished_slot = catalog_coverage_end
            .map(|slot| {
                slot.checked_add(1).ok_or_else(|| {
                    "Archive V2 catalog coverage end cannot advance to an unpublished slot"
                        .to_string()
                })
            })
            .transpose()?
            .unwrap_or(0);
        let history_start_slot = nominal_history_start_slot.min(first_unpublished_slot);
        let unpublished_extension_slots = nominal_history_start_slot
            .checked_sub(history_start_slot)
            .ok_or_else(|| {
                "Archive V2 checkpoint unpublished-tail arithmetic failed".to_string()
            })?;
        if unpublished_extension_slots > max_unpublished_extension_slots {
            return Err(format!(
                "Archive V2 catalog {} trails the configured hot checkpoint window by {} slots, above the {}-slot unpublished-tail bound",
                catalog.catalog_root,
                unpublished_extension_slots,
                max_unpublished_extension_slots
            ));
        }
        let catalog_root = reader
            .checkpoint_handoff_root(history_start_slot)
            .map_err(|error| error.to_string())?;
        Ok(Some((history_start_slot, catalog_root)))
    }

    /// Whether an admitted Archive V2 role owns a catalog-covered public-
    /// history read. Once admitted, every role must use the authenticated V2
    /// path for covered slots: verified-cache cannot expose bootstrap bytes,
    /// consensus must deny deep history, and full-archive must fail closed on
    /// a missing or corrupt local object instead of hiding the fault behind a
    /// legacy-cold fallback. Legacy history remains available for uncovered
    /// migration and recovery ranges until authorized retirement.
    pub(super) fn archive_v2_admitted_covers_slot(&self, slot: u64) -> bool {
        self.archive_v2_reader().is_some_and(|reader| {
            reader.status().admitted_after_fresh_sync && reader.covers_slot(slot)
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
        self.archive_v2_category_rows_with_prefix(category, start_slot, end_slot, &[])
    }

    pub(super) fn archive_v2_category_rows_with_prefix(
        &self,
        category: &str,
        start_slot: u64,
        end_slot: u64,
        prefix: &[u8],
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
            .category_rows_with_prefix(category, start_slot, end_slot, prefix)
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

    pub(super) fn archive_v2_category_value_at_slot(
        &self,
        category: &str,
        key: &[u8],
        slot: u64,
    ) -> Result<Option<Vec<u8>>, String> {
        let Some(reader) = self.archive_v2_reader() else {
            return Ok(None);
        };
        reader
            .category_value_at_slot(category, key, slot)
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
                    let slot = self.archive_v2_public_row_slot(category, &key, &value, &|id| {
                        Ok(dex_trade_slots.get(&id).copied())
                    })?;
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

    pub(super) fn archive_v2_account_tx_rows_from_blocks(
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
        dex_trade_slot: &impl Fn(u64) -> Result<Option<u64>, String>,
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
                let slot = dex_trade_slot(trade_id)?.ok_or_else(|| {
                    format!("Archive V2 {category} row references missing DEX trade {trade_id}")
                })?;
                Ok(Some(slot))
            }
            _ => Err(format!(
                "Archive V2 has no slot decoder for public category {category}"
            )),
        }
    }

    fn archive_v2_dex_trade_slots(&self) -> Result<BTreeMap<u64, u64>, String> {
        let mut slots = BTreeMap::new();
        self.visit_archive_v2_dex_trade_slots(|id, slot| {
            if slots
                .insert(id, slot)
                .is_some_and(|previous| previous != slot)
            {
                return Err(format!(
                    "Archive V2 found conflicting slots for DEX trade {id}"
                ));
            }
            Ok(())
        })?;
        Ok(slots)
    }

    fn manifest_dex_trade_slots(&self) -> Result<Option<ManifestDiskLookup>, String> {
        let mut slots: Option<ManifestDiskLookup> = None;
        self.visit_archive_v2_dex_trade_slots(|id, slot| {
            if slots.is_none() {
                slots = Some(ManifestDiskLookup::new("DEX trade slots")?);
            }
            slots
                .as_ref()
                .expect("created lookup")
                .insert(&id.to_be_bytes(), &slot.to_be_bytes())
        })?;
        Ok(slots)
    }

    fn visit_archive_v2_dex_trade_slots(
        &self,
        mut visit: impl FnMut(u64, u64) -> Result<(), String>,
    ) -> Result<(), String> {
        let Some(dex_program) = super::dex_index::dex_program_from_registry(&self.db)? else {
            return Ok(());
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
            visit(trade.trade_id, trade.slot)?;
        }
        Ok(())
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
    use std::path::Path;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn manifest_scratch_preserves_rows_conflicts_and_cleanup_after_flush() {
        let scratch = ManifestDiskLookup::new("events").unwrap();
        let path = scratch._directory.path().to_path_buf();
        // Cross multiple 16 MiB memtables; verify against independently
        // generated rows after flush, including duplicate/conflict handling.
        for n in (0u64..100_000).rev() {
            let mut value = vec![42; 512];
            value[..8].copy_from_slice(&n.to_le_bytes());
            scratch.insert(&n.to_be_bytes(), &value).unwrap();
        }
        scratch.db.flush().unwrap();
        let mut value = vec![42; 512];
        value[..8].copy_from_slice(&123u64.to_le_bytes());
        scratch.insert(&123u64.to_be_bytes(), &value).unwrap();
        assert!(scratch
            .insert(&123u64.to_be_bytes(), b"conflict")
            .unwrap_err()
            .contains("conflicting"));
        assert_eq!(scratch.get(&123u64.to_be_bytes()).unwrap(), Some(value));
        assert_eq!(scratch.get(&100_000u64.to_be_bytes()).unwrap(), None);
        let mut pages = ManifestRowPages::new(|after| scratch.page(after, 997));
        for n in 0u64..100_000 {
            let (key, value) = pages.next().unwrap().unwrap();
            assert_eq!(key, n.to_be_bytes());
            assert_eq!(&value[..8], &n.to_le_bytes());
            assert_eq!(&value[8..], &[42; 504]);
        }
        assert!(pages.next().unwrap().is_none());
        drop(pages);
        drop(scratch);
        assert!(
            !path.exists(),
            "ephemeral lookup must be removed after closing the DB"
        );
    }

    fn manifest_test_pages(
        rows: KvEntries,
        width: usize,
    ) -> impl FnMut(Option<&[u8]>) -> Result<KvPage, String> {
        move |after| {
            let remaining = rows
                .iter()
                .filter(|(key, _)| after.is_none_or(|a| key.as_slice() > a));
            let mut entries = remaining.take(width + 1).cloned().collect::<Vec<_>>();
            let has_more = entries.len() > width;
            entries.truncate(width);
            Ok(KvPage {
                next_cursor: if has_more {
                    entries.last().map(|(key, _)| key.clone())
                } else {
                    None
                },
                entries,
                has_more,
                total: 0,
            })
        }
    }

    #[test]
    fn manifest_raw_merge_preserves_global_digest_across_page_boundaries() {
        let rows: KvEntries = (0u8..20).map(|n| (vec![n], vec![n, 42])).collect();
        let mut expected = PublicHistoryDigestAccumulator::new("events");
        for (key, value) in &rows {
            expected.push(key, value);
        }
        let expected = expected.finish();
        for left_width in [1, 3, 20] {
            for right_width in [1, 7, 20] {
                // Multiples of six occur in both partitions, including page edges.
                let left = rows
                    .iter()
                    .filter(|(key, _)| key[0] % 2 == 0)
                    .cloned()
                    .collect();
                let right = rows
                    .iter()
                    .filter(|(key, _)| key[0] % 2 != 0 || key[0] % 3 == 0)
                    .cloned()
                    .collect();
                assert_eq!(
                    merge_manifest_row_pages(
                        "events",
                        manifest_test_pages(left, left_width),
                        manifest_test_pages(right, right_width)
                    )
                    .unwrap(),
                    expected
                );
            }
        }
        assert_eq!(
            merge_manifest_row_pages(
                "events",
                manifest_test_pages(rows, 1),
                manifest_test_pages(Vec::new(), 1)
            )
            .unwrap(),
            expected
        );
    }

    #[test]
    fn manifest_raw_merge_rejects_late_conflict_and_source_failure() {
        let left = vec![(vec![1], vec![1]), (vec![9], vec![9])];
        let right = vec![(vec![2], vec![2]), (vec![9], vec![8])];
        assert!(merge_manifest_row_pages(
            "events",
            manifest_test_pages(left.clone(), 1),
            manifest_test_pages(right, 1)
        )
        .unwrap_err()
        .contains("conflicting"));
        let mut calls = 0;
        let mut source = manifest_test_pages(left, 1);
        assert!(merge_manifest_row_pages(
            "events",
            move |after| {
                calls += 1;
                if calls == 2 {
                    Err("authenticated source failure".to_string())
                } else {
                    source(after)
                }
            },
            manifest_test_pages(Vec::new(), 1)
        )
        .unwrap_err()
        .contains("authenticated source failure"));
    }

    #[test]
    fn manifest_raw_pages_reject_invalid_order_and_continuation() {
        for page in [
            KvPage {
                entries: vec![(vec![2], vec![]), (vec![1], vec![])],
                total: 0,
                next_cursor: None,
                has_more: false,
            },
            KvPage {
                entries: vec![(vec![1], vec![]), (vec![1], vec![])],
                total: 0,
                next_cursor: None,
                has_more: false,
            },
            KvPage {
                entries: vec![],
                total: 0,
                next_cursor: Some(vec![1]),
                has_more: true,
            },
            KvPage {
                entries: vec![(vec![1], vec![])],
                total: 0,
                next_cursor: Some(vec![2]),
                has_more: true,
            },
        ] {
            let mut page = Some(page);
            assert!(ManifestRowPages::new(move |_| Ok(page.take().unwrap()))
                .next()
                .is_err());
        }
        let mut pages = ManifestRowPages::new(|_| {
            Ok(KvPage {
                entries: vec![(vec![1], vec![])],
                total: 0,
                next_cursor: Some(vec![1]),
                has_more: true,
            })
        });
        assert!(pages.next().unwrap().is_some());
        assert!(pages.next().unwrap_err().contains("exclusive key order"));
    }

    #[test]
    fn manifest_raw_pages_enforce_payload_and_row_envelopes() {
        let mut oversized = Some(vec![(vec![1], vec![0; MANIFEST_RAW_PAGE_BYTES])]);
        let mut pages = ManifestRowPages::new(move |_| {
            Ok(KvPage {
                entries: oversized.take().unwrap(),
                total: 0,
                next_cursor: None,
                has_more: false,
            })
        });
        assert!(pages.next().unwrap_err().contains("payload limit"));
        let mut pages = ManifestRowPages::new(|_| {
            Ok(KvPage {
                entries: vec![(vec![], vec![]); ARCHIVE_V2_EXPORT_PAGE_ROWS as usize + 1],
                total: 0,
                next_cursor: None,
                has_more: false,
            })
        });
        assert!(pages.next().unwrap_err().contains("row limit"));
    }

    #[test]
    fn recent_hot_page_does_not_fetch_older_archive_segments() {
        let state_root = tempdir().unwrap();
        let archive_root = tempdir().unwrap();
        let remote_root = tempdir().unwrap();
        let cache_root = tempdir().unwrap();
        let state = StateStore::open(state_root.path()).unwrap();
        let archived_transaction = Transaction::new(crate::Message::new(
            vec![crate::Instruction {
                program_id: crate::Pubkey([1; 32]),
                accounts: vec![crate::Pubkey([2; 32])],
                data: vec![4; 64],
            }],
            Hash::default(),
        ));
        let archived_signature = archived_transaction.signature();
        let genesis = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::default(),
            [5; 32],
            vec![archived_transaction],
            1,
        );
        let identity = ArchiveV2Identity {
            network_id: "recent-page-regression".to_string(),
            genesis_hash: genesis.hash(),
        };
        let (bytes, manifest) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &ArchiveV2SegmentContents::from_blocks(vec![genesis.clone()]),
            &ArchiveV2CodecConfig::default(),
        )
        .unwrap();
        fs::create_dir_all(remote_root.path().join("objects")).unwrap();
        fs::write(
            remote_root
                .path()
                .join("objects")
                .join(format!("{}.av2s", manifest.segment_object_hash)),
            &bytes,
        )
        .unwrap();
        let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
        catalog.append(manifest).unwrap();
        let catalog_path = archive_root.path().join("catalog.av2");
        catalog.store_atomic(&catalog_path).unwrap();
        let transaction = Transaction::new(crate::Message::new(
            vec![crate::Instruction {
                program_id: crate::Pubkey([1; 32]),
                accounts: vec![crate::Pubkey([2; 32])],
                data: vec![3; 64],
            }],
            genesis.hash(),
        ));
        let signature = transaction.signature();
        let recent = Block::new_with_timestamp(
            1,
            genesis.hash(),
            Hash::default(),
            [5; 32],
            vec![transaction],
            2,
        );
        state.put_block_atomic(&recent, None, None).unwrap();
        state.attach_archive_v2_reader(
            ArchiveV2Reader::open(
                identity,
                &catalog_path,
                ArchiveV2ReaderConfig {
                    role: ArchiveV2Role::VerifiedCache,
                    root: archive_root.path().to_path_buf(),
                    cache_root: Some(cache_root.path().to_path_buf()),
                    cache_quota_bytes: bytes.len() as u64 + 1024,
                    max_decoded_segments: 1,
                    allow_remote_fetch: true,
                    sources: vec![Arc::new(ArchiveV2DirectorySource::new(
                        "test-source",
                        remote_root.path(),
                        true,
                    ))],
                },
            )
            .unwrap(),
        );
        state.mark_archive_v2_admitted_after_fresh_sync().unwrap();
        assert_eq!(
            state.get_recent_txs_paginated_exact(1, None).unwrap(),
            vec![(signature, 1, 0)]
        );
        assert_eq!(
            state.archive_v2_status().unwrap().remote_fetches,
            0,
            "a complete hot page must not fetch strictly older archive data"
        );
        assert_eq!(
            state.get_recent_user_txs_paginated_exact(1, None).unwrap(),
            vec![(signature, 1, 0)]
        );
        assert_eq!(
            state.archive_v2_status().unwrap().remote_fetches,
            0,
            "the user-only RPC page must also stay inside the hot suffix"
        );
        assert_eq!(
            state.get_recent_txs_paginated_exact(2, None).unwrap(),
            vec![(signature, 1, 0), (archived_signature, 0, 0)],
            "a partial hot page must still include authenticated history"
        );
        assert_eq!(state.archive_v2_status().unwrap().remote_fetches, 1);
        assert_eq!(
            state
                .get_recent_txs_paginated_exact(1, Some((1, 0)))
                .unwrap(),
            vec![(archived_signature, 0, 0)],
            "pagination must continue across the hot/archive boundary"
        );
        // An overlapping hot row at the catalog boundary cannot exclude other
        // canonical ordinals in that same slot, and must deduplicate exactly.
        state.put_block_atomic(&genesis, None, None).unwrap();
        assert_eq!(
            state.get_recent_txs_paginated_exact(2, None).unwrap(),
            vec![(signature, 1, 0), (archived_signature, 0, 0)]
        );
    }
    use crate::archive_v2::{
        ArchiveV2Catalog, ArchiveV2CodecConfig, ArchiveV2DirectorySource, ArchiveV2Identity,
        ArchiveV2ReaderConfig, ArchiveV2SegmentCodec, ArchiveV2SegmentContents,
    };
    use crate::state::{
        SymbolRegistryEntry, CF_BLOCKS, CF_CONTRACT_STORAGE, CF_EVENTS, CF_SLOTS, CF_TRANSACTIONS,
        CF_TX_TO_SLOT,
    };
    use crate::{Instruction, Message, Pubkey};

    #[test]
    fn logical_checkpoint_manifest_is_independent_of_archive_handoff() {
        fn archive_reader(
            root: &Path,
            identity: ArchiveV2Identity,
            parts: Vec<ArchiveV2SegmentContents>,
            history_start_slot: u64,
        ) -> (ArchiveV2Reader, Hash) {
            let objects = root.join("objects");
            fs::create_dir_all(&objects).unwrap();
            let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
            let mut previous_segment = None;
            let mut previous_block = Hash::default();
            for contents in parts {
                let (bytes, manifest) = ArchiveV2SegmentCodec::encode(
                    identity.clone(),
                    previous_segment,
                    previous_block,
                    &contents,
                    &ArchiveV2CodecConfig {
                        target_frame_bytes: 1024 * 1024,
                        ..ArchiveV2CodecConfig::default()
                    },
                )
                .unwrap();
                previous_segment = Some(manifest.segment_object_hash);
                previous_block = manifest.last_block_hash;
                fs::write(
                    objects.join(format!("{}.av2s", manifest.segment_object_hash)),
                    bytes,
                )
                .unwrap();
                catalog.append(manifest).unwrap();
            }
            let handoff_root = catalog.checkpoint_handoff_root(history_start_slot).unwrap();
            let catalog_path = root.join("catalog.av2");
            catalog.store_atomic(&catalog_path).unwrap();
            let reader = ArchiveV2Reader::open(
                identity,
                &catalog_path,
                ArchiveV2ReaderConfig {
                    role: ArchiveV2Role::FullArchive,
                    root: root.to_path_buf(),
                    cache_root: None,
                    cache_quota_bytes: 0,
                    max_decoded_segments: 1,
                    allow_remote_fetch: false,
                    sources: Vec::new(),
                },
            )
            .unwrap();
            (reader, handoff_root)
        }

        let state_root = tempdir().unwrap();
        let archive_a_root = tempdir().unwrap();
        let archive_b_root = tempdir().unwrap();
        let state = StateStore::open(state_root.path()).unwrap();
        let mut blocks: Vec<Block> = Vec::new();
        let mut parent_hash = Hash::default();
        for slot in 0u64..4 {
            let transaction = if slot == 3 {
                blocks[0].transactions[0].clone()
            } else {
                Transaction::new(Message::new(
                    vec![Instruction {
                        program_id: Pubkey([slot as u8 + 1; 32]),
                        accounts: vec![Pubkey([slot as u8 + 9; 32])],
                        data: vec![slot as u8; 8],
                    }],
                    Hash::hash(&slot.to_le_bytes()),
                ))
            };
            let block = Block::new_with_timestamp(
                slot,
                parent_hash,
                Hash::hash(format!("logical-manifest-state-{slot}").as_bytes()),
                [slot as u8 + 17; 32],
                vec![transaction],
                slot + 1,
            );
            parent_hash = block.hash();
            state
                .put_block_atomic(&block, Some(slot), Some(slot))
                .unwrap();
            let signature = block.transactions[0].signature();
            if slot % 2 == 0 {
                state.put_tx_meta(&signature, slot + 10).unwrap();
            } else {
                state
                    .put_tx_meta_full(
                        &signature,
                        &crate::processor::TxMeta {
                            compute_units_used: slot + 10,
                            return_code: Some(0),
                            return_data: vec![slot as u8],
                            logs: vec![format!("receipt-{slot}")],
                            success: Some(true),
                            fee_paid: Some(7),
                            ..Default::default()
                        },
                    )
                    .unwrap();
            }
            blocks.push(block);
        }
        let identity = ArchiveV2Identity {
            network_id: "logical-manifest-testnet".to_string(),
            genesis_hash: blocks[0].hash(),
        };
        let (reader_a, handoff_a) = archive_reader(
            archive_a_root.path(),
            identity.clone(),
            vec![state.export_archive_v2_segment_contents(0, 1).unwrap()],
            2,
        );
        let (reader_b, handoff_b) = archive_reader(
            archive_b_root.path(),
            identity.clone(),
            vec![state.export_archive_v2_segment_contents(0, 2).unwrap()],
            3,
        );

        let manifest_a = state
            .compute_archive_v2_checkpoint_public_history_manifest(
                &reader_a,
                3,
                CheckpointSnapshotProfile::HotRepairV1 {
                    history_start_slot: 2,
                    archive_v2_catalog_root: Some(handoff_a.0),
                },
                PUBLIC_HISTORY_SNAPSHOT_CATEGORIES,
                1,
            )
            .unwrap();
        let manifest_b = state
            .compute_archive_v2_checkpoint_public_history_manifest(
                &reader_b,
                3,
                CheckpointSnapshotProfile::HotRepairV1 {
                    history_start_slot: 3,
                    archive_v2_catalog_root: Some(handoff_b.0),
                },
                PUBLIC_HISTORY_SNAPSHOT_CATEGORIES,
                1,
            )
            .unwrap();
        let full_manifest = state
            .compute_public_history_manifest(PUBLIC_HISTORY_SNAPSHOT_CATEGORIES, 1)
            .unwrap();
        assert_eq!(
            full_manifest
                .categories
                .iter()
                .find(|digest| digest.category == "tx_meta")
                .unwrap()
                .entry_count,
            4
        );

        assert_eq!(manifest_a, manifest_b);
        assert_eq!(manifest_a, full_manifest);
        let conflicting_root = tempdir().unwrap();
        let mut conflicting_contents = state.export_archive_v2_segment_contents(0, 1).unwrap();
        let metadata_rows = conflicting_contents
            .public_categories
            .entry("tx_meta".to_string())
            .or_default();
        metadata_rows.push(ArchiveV2PublicRow {
            slot: 0,
            key: blocks[0].transactions[0].signature().0.to_vec(),
            value: 999u64.to_le_bytes().to_vec(),
        });
        metadata_rows.sort_by(|a, b| a.key.cmp(&b.key));
        let (conflicting_reader, conflicting_handoff) = archive_reader(
            conflicting_root.path(),
            identity.clone(),
            vec![conflicting_contents],
            2,
        );
        assert!(state
            .compute_archive_v2_checkpoint_public_history_manifest(
                &conflicting_reader,
                3,
                CheckpointSnapshotProfile::HotRepairV1 {
                    history_start_slot: 2,
                    archive_v2_catalog_root: Some(conflicting_handoff.0)
                },
                PUBLIC_HISTORY_SNAPSHOT_CATEGORIES,
                1,
            )
            .unwrap_err()
            .contains("tx_meta has conflicting"));
        assert!(state
            .compute_archive_v2_checkpoint_public_history_manifest(
                &reader_a,
                3,
                CheckpointSnapshotProfile::HotRepairV1 {
                    history_start_slot: 2,
                    archive_v2_catalog_root: Some([0xff; 32])
                },
                PUBLIC_HISTORY_SNAPSHOT_CATEGORIES,
                1,
            )
            .unwrap_err()
            .contains("differs from supplied catalog handoff"));

        let mut broken_parent = blocks[3].clone();
        broken_parent.header.parent_hash = Hash::hash(b"incorrect-hot-parent");
        state
            .put_block_atomic(&broken_parent, Some(3), Some(3))
            .unwrap();
        assert!(state
            .compute_archive_v2_checkpoint_public_history_manifest(
                &reader_a,
                3,
                CheckpointSnapshotProfile::HotRepairV1 {
                    history_start_slot: 2,
                    archive_v2_catalog_root: Some(handoff_a.0)
                },
                PUBLIC_HISTORY_SNAPSHOT_CATEGORIES,
                1,
            )
            .unwrap_err()
            .contains("parent mismatch"));
        state
            .put_block_atomic(&blocks[3], Some(3), Some(3))
            .unwrap();

        // Segment boundaries and a handoff inside the final segment must not
        // change the digest or fold the overlapped hot suffix twice.
        for (ranges, history_start_slot) in [
            (vec![(0, 0), (1, 1), (2, 2)], 3),
            (vec![(0, 0), (1, 3)], 2),
            (vec![(0, 0), (1, 3)], 0),
        ] {
            let archive_root = tempdir().unwrap();
            let (reader, handoff) = archive_reader(
                archive_root.path(),
                identity.clone(),
                ranges
                    .into_iter()
                    .map(|(start, end)| {
                        state
                            .export_archive_v2_segment_contents(start, end)
                            .unwrap()
                    })
                    .collect(),
                history_start_slot,
            );
            for page_size in [1, 2, 50_000] {
                assert_eq!(
                    state
                        .compute_archive_v2_checkpoint_public_history_manifest(
                            &reader,
                            3,
                            CheckpointSnapshotProfile::HotRepairV1 {
                                history_start_slot,
                                archive_v2_catalog_root: Some(handoff.0)
                            },
                            PUBLIC_HISTORY_SNAPSHOT_CATEGORIES,
                            page_size,
                        )
                        .unwrap(),
                    full_manifest
                );
            }
        }
    }

    #[test]
    fn known_slot_receipts_preserve_formats_and_bound_absent_metadata() {
        let state_root = tempdir().unwrap();
        let archive_root = tempdir().unwrap();
        let state = StateStore::open(state_root.path()).unwrap();
        let transactions: Vec<_> = (1..=4)
            .map(|byte| {
                Transaction::new(Message::new(
                    vec![Instruction {
                        program_id: Pubkey([1; 32]),
                        accounts: vec![Pubkey([2; 32])],
                        data: vec![byte; 64],
                    }],
                    Hash::default(),
                ))
            })
            .collect();
        let signatures: Vec<_> = transactions.iter().map(Transaction::signature).collect();
        let block = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::default(),
            [5; 32],
            transactions,
            1,
        );
        let child =
            Block::new_with_timestamp(1, block.hash(), Hash::default(), [5; 32], Vec::new(), 2);
        let identity = ArchiveV2Identity {
            network_id: "known-slot-receipts".to_string(),
            genesis_hash: block.hash(),
        };
        let full = crate::processor::TxMeta {
            compute_units_used: 23,
            return_code: Some(7),
            fee_paid: Some(11),
            logs: vec!["retained receipt".to_string()],
            ..Default::default()
        };
        let mut contents = ArchiveV2SegmentContents::from_blocks(vec![block]);
        contents.public_categories.insert(
            "tx_meta".to_string(),
            [
                (signatures[0], 17u64.to_le_bytes().to_vec()),
                (
                    signatures[1],
                    crate::codec::serialize_legacy_bincode(&full, "test receipt").unwrap(),
                ),
                (signatures[2], vec![255]),
            ]
            .into_iter()
            .map(|(signature, value)| ArchiveV2PublicRow {
                slot: 0,
                key: signature.0.to_vec(),
                value,
            })
            .collect(),
        );
        contents
            .public_categories
            .get_mut("tx_meta")
            .unwrap()
            .sort_by(|a, b| a.key.cmp(&b.key));
        let (bytes, manifest) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &contents,
            &ArchiveV2CodecConfig::default(),
        )
        .unwrap();
        fs::create_dir_all(archive_root.path().join("objects")).unwrap();
        fs::write(
            archive_root
                .path()
                .join("objects")
                .join(format!("{}.av2s", manifest.segment_object_hash)),
            bytes,
        )
        .unwrap();
        let (_, unavailable) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            Some(manifest.segment_object_hash),
            manifest.last_block_hash,
            &ArchiveV2SegmentContents::from_blocks(vec![child]),
            &ArchiveV2CodecConfig::default(),
        )
        .unwrap();
        let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
        catalog.append(manifest).unwrap();
        catalog.append(unavailable).unwrap();
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
        assert_eq!(
            state
                .get_tx_meta_full_at_slot(&signatures[0], 0)
                .unwrap()
                .unwrap()
                .compute_units_used,
            17
        );
        let actual = state
            .get_tx_meta_full_at_slot(&signatures[1], 0)
            .unwrap()
            .unwrap();
        assert_eq!(actual.compute_units_used, full.compute_units_used);
        assert_eq!(actual.return_code, full.return_code);
        assert_eq!(actual.fee_paid, full.fee_paid);
        assert_eq!(actual.logs, full.logs);
        assert!(state
            .get_tx_meta_full_at_slot(&signatures[2], 0)
            .unwrap_err()
            .contains("deserialize"));
        assert!(state
            .get_tx_meta_full_at_slot(&signatures[3], 0)
            .unwrap()
            .is_none());
        assert!(
            state.get_tx_meta_full(&signatures[3]).is_err(),
            "unbounded fallback reaches unrelated unavailable history"
        );
        let hot = Hash::hash(b"hot-only-receipt");
        state.put_tx_meta(&hot, 42).unwrap();
        assert_eq!(
            state
                .get_tx_meta_full_at_slot(&hot, 100)
                .unwrap()
                .unwrap()
                .compute_units_used,
            42
        );
    }

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
    fn checkpoint_handoff_retains_a_bounded_unpublished_catalog_tail() {
        let state_root = tempdir().unwrap();
        let archive_root = tempdir().unwrap();
        let state = StateStore::open(state_root.path()).unwrap();
        let genesis = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"archive-v2-checkpoint-handoff-state"),
            [0x81; 32],
            Vec::new(),
            1,
        );
        let identity = ArchiveV2Identity {
            network_id: "checkpoint-handoff-testnet".to_string(),
            genesis_hash: genesis.hash(),
        };
        let (bytes, manifest) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &ArchiveV2SegmentContents::from_blocks(vec![genesis]),
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
        let expected_genesis_handoff = catalog.checkpoint_handoff_root(1).unwrap();
        let expected_empty_handoff = catalog.checkpoint_handoff_root(0).unwrap();
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

        let error = state.archive_v2_checkpoint_handoff(5, 4).unwrap_err();
        assert!(
            error.contains("catalog admitted after fresh sync"),
            "unexpected pre-admission checkpoint error: {error}"
        );
        state.mark_archive_v2_admitted_after_fresh_sync().unwrap();

        assert_eq!(
            state.archive_v2_checkpoint_handoff(5, 4).unwrap(),
            Some((1, expected_genesis_handoff)),
            "checkpoint must retain slots 1..tip after a catalog ending at genesis"
        );
        let error = state.archive_v2_checkpoint_handoff(5, 3).unwrap_err();
        assert!(
            error.contains("above the 3-slot unpublished-tail bound"),
            "unexpected stale-catalog error: {error}"
        );
        assert_eq!(
            state.archive_v2_checkpoint_handoff(0, 0).unwrap(),
            Some((0, expected_empty_handoff)),
            "a checkpoint retaining genesis needs no catalog predecessor"
        );
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
    fn admitted_full_archive_owns_catalog_covered_history_and_fails_closed() {
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

        let object_dir = archive_root.path().join("objects");
        let object_path = object_dir.join(format!(
            "{}.av2s",
            catalog.entries[0].manifest.segment_object_hash
        ));
        fs::write(&object_path, b"truncated archive v2 object").unwrap();

        let error = state.get_block_by_slot(0).unwrap_err();
        assert!(
            error.contains("not locally readable"),
            "an admitted full archive must fail closed instead of serving legacy cold: {error}"
        );
        assert_eq!(
            state.archive_v2_status().unwrap().local_hits,
            0,
            "an unavailable V2 object must not report a local hit"
        );
        assert_eq!(
            state.archive_v2_status().unwrap().quarantined_objects,
            1,
            "an admitted full archive must quarantine a corrupt local object"
        );
        assert!(
            !object_path.exists(),
            "a corrupt Archive V2 object must leave the active object path"
        );

        fs::write(object_path, bytes).unwrap();
        assert_eq!(
            state.get_block_by_slot(0).unwrap().unwrap().hash(),
            block_hash,
            "an admitted full archive must serve the authenticated local V2 object"
        );
        assert_eq!(state.archive_v2_status().unwrap().local_hits, 1);
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
    fn hot_repair_checkpoint_export_bounds_canonical_and_key_ordered_history() {
        let root = tempdir().unwrap();
        let state = StateStore::open(root.path()).unwrap();
        let genesis = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"checkpoint-bounded-state-0"),
            [0x71; 32],
            Vec::new(),
            1,
        );
        let recent = Block::new_with_timestamp(
            1,
            genesis.hash(),
            Hash::hash(b"checkpoint-bounded-state-1"),
            [0x72; 32],
            Vec::new(),
            2,
        );
        let future = Block::new_with_timestamp(
            2,
            recent.hash(),
            Hash::hash(b"checkpoint-bounded-state-2"),
            [0x73; 32],
            Vec::new(),
            3,
        );
        for block in [&genesis, &recent, &future] {
            state
                .put_block_atomic(block, Some(block.header.slot), Some(block.header.slot))
                .unwrap();
        }

        let events_cf = state.db.cf_handle(CF_EVENTS).unwrap();
        let event_key = |account: u8, slot: u64| {
            let mut key = vec![account; 32];
            key.extend_from_slice(&slot.to_be_bytes());
            key.extend_from_slice(&0u64.to_be_bytes());
            key.extend_from_slice(&0u64.to_be_bytes());
            key
        };
        for (account, slot) in [(0x10, 0), (0x10, 1), (0x10, 2), (0x20, 0), (0x20, 1)] {
            state
                .db
                .put_cf(&events_cf, event_key(account, slot), [account, slot as u8])
                .unwrap();
        }

        let profile = CheckpointSnapshotProfile::HotRepairV1 {
            history_start_slot: 1,
            archive_v2_catalog_root: Some([0x74; 32]),
        };
        for category in ["slots", "blocks", "events"] {
            let mut cursor = None;
            let mut pages = 0usize;
            let mut rows = Vec::new();
            loop {
                let page = state
                    .export_checkpoint_snapshot_category_cursor_untracked(
                        category,
                        cursor.as_deref(),
                        1,
                        1,
                        profile,
                    )
                    .unwrap();
                pages += 1;
                rows.extend(page.entries);
                if !page.has_more {
                    break;
                }
                cursor = page.next_cursor;
                assert!(cursor.is_some(), "{category} must advance bounded pages");
            }

            let expected = if category == "events" { 2 } else { 1 };
            assert_eq!(rows.len(), expected, "bounded {category} row count");
            assert_eq!(pages, expected, "bounded {category} page count");
            if category == "events" {
                assert!(rows
                    .iter()
                    .all(|(key, _)| { u64::from_be_bytes(key[32..40].try_into().unwrap()) == 1 }));
            }
        }

        let full = state
            .export_checkpoint_snapshot_category_cursor_untracked(
                "slots",
                None,
                10,
                1,
                CheckpointSnapshotProfile::FullArchiveV1,
            )
            .unwrap();
        assert_eq!(
            full.entries
                .iter()
                .filter(|(key, value)| key.len() == 8 && value.len() == 32)
                .count(),
            3,
            "full profile must retain canonical rows outside the hot window"
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
            state
                .manifest_dex_trade_slots()
                .unwrap()
                .unwrap()
                .get(&1u64.to_be_bytes())
                .unwrap(),
            Some(77u64.to_be_bytes().to_vec())
        );

        // Historical keys can encode the same parsed id with leading zeros.
        // Preserve equality/conflict checks across those distinct source keys.
        put_storage(dex_program, b"dex_trade_01", &trade);
        assert_eq!(
            state.archive_v2_dex_trade_slots().unwrap(),
            BTreeMap::from([(1, 77)])
        );
        let slots = state.manifest_dex_trade_slots().unwrap();
        for (category, prefix_bytes) in [
            ("dex_trades_by_pair", 8),
            ("dex_trades_by_taker", 32),
            ("dex_trades_by_pair_taker", 40),
        ] {
            let mut key = vec![0; prefix_bytes];
            key.extend_from_slice(&1u64.to_be_bytes());
            let cf = state.db.cf_handle(category).unwrap();
            state.db.put_cf(&cf, &key, []).unwrap();
            let profile = CheckpointSnapshotProfile::HotRepairV1 {
                history_start_slot: 77,
                archive_v2_catalog_root: Some([1; 32]),
            };
            let page = state
                .manifest_hot_raw_page(category, None, 1, 77, profile, &slots)
                .unwrap();
            assert_eq!(page.entries, vec![(key.clone(), Vec::new())]);
            assert!(!page.has_more);
            let empty = state
                .manifest_hot_raw_page(category, Some(&key), 1, 77, profile, &slots)
                .unwrap();
            assert!(empty.entries.is_empty());
            assert!(state
                .manifest_hot_raw_page(
                    category,
                    None,
                    1,
                    76,
                    CheckpointSnapshotProfile::HotRepairV1 {
                        history_start_slot: 76,
                        archive_v2_catalog_root: Some([1; 32])
                    },
                    &slots
                )
                .unwrap()
                .entries
                .is_empty());
        }
        trade[72..80].copy_from_slice(&78u64.to_le_bytes());
        put_storage(dex_program, b"dex_trade_01", &trade);
        assert!(state
            .archive_v2_dex_trade_slots()
            .unwrap_err()
            .contains("conflicting"));
        assert!(state
            .manifest_dex_trade_slots()
            .unwrap_err()
            .contains("conflicting"));
        trade[72..80].copy_from_slice(&77u64.to_le_bytes());
        put_storage(dex_program, b"dex_trade_01", &trade);

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
