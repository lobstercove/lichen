use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use lru::LruCache;
use serde::Serialize;

use super::{
    ArchiveV2Catalog, ArchiveV2DecodedSegment, ArchiveV2Error, ArchiveV2Identity,
    ArchiveV2Manifest, ArchiveV2Role, ArchiveV2SegmentCodec,
};
use crate::codec::serialize_legacy_bincode;
use crate::{Block, Hash, Transaction};

static QUARANTINE_NONCE: AtomicU64 = AtomicU64::new(0);

mod index_cache;

pub trait ArchiveV2ObjectSource: Send + Sync {
    fn name(&self) -> &str;
    fn authenticated(&self) -> bool;
    fn fetch(&self, object_hash: &Hash) -> Result<Option<Vec<u8>>, ArchiveV2Error>;
}

#[derive(Debug, Clone)]
pub struct ArchiveV2DirectorySource {
    name: String,
    root: PathBuf,
    authenticated: bool,
}

impl ArchiveV2DirectorySource {
    pub fn new(name: impl Into<String>, root: impl Into<PathBuf>, authenticated: bool) -> Self {
        Self {
            name: name.into(),
            root: root.into(),
            authenticated,
        }
    }
}

impl ArchiveV2ObjectSource for ArchiveV2DirectorySource {
    fn name(&self) -> &str {
        &self.name
    }

    fn authenticated(&self) -> bool {
        self.authenticated
    }

    fn fetch(&self, object_hash: &Hash) -> Result<Option<Vec<u8>>, ArchiveV2Error> {
        let path = object_path(&self.root, object_hash);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(ArchiveV2Error::Io(format!(
                "source {} failed reading {}: {error}",
                self.name,
                path.display()
            ))),
        }
    }
}

pub struct ArchiveV2ReaderConfig {
    pub role: ArchiveV2Role,
    pub root: PathBuf,
    pub cache_root: Option<PathBuf>,
    pub cache_quota_bytes: u64,
    pub max_decoded_segments: usize,
    pub allow_remote_fetch: bool,
    pub sources: Vec<Arc<dyn ArchiveV2ObjectSource>>,
}

/// Exclusive key cursor for an authenticated public-index category. The row
/// and payload limits bound the merge across all segments independently of
/// the total history size; decoding one index frame has its own codec bound.
pub struct ArchiveV2CategoryCursor<'a> {
    pub category: &'a str,
    pub start_slot: u64,
    pub end_slot: u64,
    pub after_key: Option<&'a [u8]>,
    pub max_rows: usize,
    pub max_payload_bytes: usize,
}

impl ArchiveV2ReaderConfig {
    pub fn validate(&self) -> Result<(), ArchiveV2Error> {
        if self.max_decoded_segments == 0 || self.max_decoded_segments > 1024 {
            return Err(ArchiveV2Error::Bounds(
                "decoded segment cache must be in 1..=1024".to_string(),
            ));
        }
        if self.allow_remote_fetch {
            if self.cache_root.is_none() || self.cache_quota_bytes == 0 {
                return Err(ArchiveV2Error::Role(
                    "remote archive fetch requires a bounded cache".to_string(),
                ));
            }
            if !self.sources.iter().any(|source| source.authenticated()) {
                return Err(ArchiveV2Error::Role(
                    "remote archive fetch requires an authenticated source".to_string(),
                ));
            }
        }
        match self.role {
            ArchiveV2Role::VerifiedCache if !self.allow_remote_fetch => {
                return Err(ArchiveV2Error::Role(
                    "verified-cache reader must enable authenticated remote fetch".to_string(),
                ));
            }
            ArchiveV2Role::Consensus
                if self.allow_remote_fetch
                    || self.cache_quota_bytes != 0
                    || self.cache_root.is_some() =>
            {
                return Err(ArchiveV2Error::Role(
                    "consensus reader must not configure archive fetching or cache".to_string(),
                ));
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ArchiveV2ReaderStatus {
    pub role: Option<String>,
    pub serves_deep_history: bool,
    /// True when this reader was admitted only after a fresh node completed
    /// state synchronization. This durable-for-process-lifetime signal keeps
    /// admission lifecycle observability independent of log verbosity.
    pub admitted_after_fresh_sync: bool,
    pub catalog_root: Option<String>,
    pub catalog_segments: u64,
    pub local_hits: u64,
    pub cache_hits: u64,
    pub remote_fetches: u64,
    pub source_failures: u64,
    pub quarantined_objects: u64,
    pub verified_objects: u64,
    pub cache_bytes: u64,
    pub last_error: Option<String>,
}

pub struct ArchiveV2Reader {
    identity: ArchiveV2Identity,
    catalog: ArchiveV2Catalog,
    config: ArchiveV2ReaderConfig,
    decoded: Mutex<LruCache<Hash, Arc<ArchiveV2DecodedSegment>>>,
    verified_objects: Mutex<BTreeSet<Hash>>,
    cache_io_lock: Mutex<()>,
    status: Mutex<ArchiveV2ReaderStatus>,
}

impl ArchiveV2Reader {
    pub fn open(
        identity: ArchiveV2Identity,
        catalog_path: &Path,
        config: ArchiveV2ReaderConfig,
    ) -> Result<Self, ArchiveV2Error> {
        identity.validate()?;
        config.validate()?;
        let catalog = ArchiveV2Catalog::load(catalog_path)?;
        if catalog.identity.network_id != identity.network_id {
            return Err(ArchiveV2Error::WrongNetwork {
                expected: identity.network_id,
                actual: catalog.identity.network_id,
            });
        }
        if catalog.identity.genesis_hash != identity.genesis_hash {
            return Err(ArchiveV2Error::WrongGenesis);
        }
        fs::create_dir_all(config.root.join("objects"))?;
        fs::create_dir_all(config.root.join("quarantine"))?;
        if let Some(cache) = config.cache_root.as_ref() {
            fs::create_dir_all(cache.join("objects"))?;
            fs::create_dir_all(cache.join("quarantine"))?;
        }
        let status = ArchiveV2ReaderStatus {
            role: Some(config.role.to_string()),
            serves_deep_history: config.role != ArchiveV2Role::Consensus,
            catalog_root: Some(catalog.catalog_root.to_hex()),
            catalog_segments: catalog.entries.len() as u64,
            cache_bytes: config
                .cache_root
                .as_deref()
                .map(cache_size)
                .transpose()?
                .unwrap_or(0),
            ..ArchiveV2ReaderStatus::default()
        };
        Ok(Self {
            identity,
            catalog,
            decoded: Mutex::new(LruCache::new(
                NonZeroUsize::new(config.max_decoded_segments)
                    .expect("validated decoded segment cache size"),
            )),
            verified_objects: Mutex::new(BTreeSet::new()),
            cache_io_lock: Mutex::new(()),
            config,
            status: Mutex::new(status),
        })
    }

    pub fn catalog(&self) -> &ArchiveV2Catalog {
        &self.catalog
    }

    /// Bind a checkpoint to this reader's immutable, verified catalog prefix.
    /// Loading already validates every manifest, identity and catalog root.
    /// Coverage and the exact handoff boundary are checked on every call without
    /// re-encoding the entire catalog on the consensus commit path.
    pub fn checkpoint_handoff_root(&self, history_start_slot: u64) -> Result<Hash, ArchiveV2Error> {
        self.catalog
            .checkpoint_handoff_root_validated(history_start_slot)
    }

    pub fn role(&self) -> ArchiveV2Role {
        self.config.role
    }

    pub fn covers_slot(&self, slot: u64) -> bool {
        self.manifest_for_slot(slot).is_some()
    }

    pub fn status(&self) -> ArchiveV2ReaderStatus {
        self.status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn mark_admitted_after_fresh_sync(&self) {
        self.status_lock().admitted_after_fresh_sync = true;
    }

    pub fn verify_segment(&self, object_hash: &Hash) -> Result<ArchiveV2Manifest, ArchiveV2Error> {
        self.ensure_deep_history()?;
        let manifest = self
            .catalog
            .entries
            .iter()
            .filter_map(|entry| {
                self.catalog
                    .active_manifest(&entry.manifest.segment_object_hash)
                    .ok()
            })
            .find(|manifest| manifest.segment_object_hash == *object_hash)
            .ok_or_else(|| {
                ArchiveV2Error::Unavailable(format!(
                    "segment {object_hash} is not active in the catalog"
                ))
            })?;
        self.load_segment(manifest)?;
        Ok(manifest.clone())
    }

    pub fn local_verified_object(
        &self,
        object_hash: &Hash,
    ) -> Result<Option<Vec<u8>>, ArchiveV2Error> {
        if self.config.role != ArchiveV2Role::FullArchive {
            return Err(ArchiveV2Error::Role(
                "only a full-archive role may serve Archive V2 objects".to_string(),
            ));
        }
        let Some(manifest) = self
            .catalog
            .entries
            .iter()
            .filter_map(|entry| {
                self.catalog
                    .active_manifest(&entry.manifest.segment_object_hash)
                    .ok()
            })
            .find(|manifest| manifest.segment_object_hash == *object_hash)
        else {
            return Ok(None);
        };
        let path = object_path(&self.config.root, object_hash);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(ArchiveV2Error::Io(format!(
                    "failed reading {}: {error}",
                    path.display()
                )))
            }
        };
        self.verify_object_bytes(&bytes, manifest)?;
        let mut status = self.status_lock();
        status.local_hits = status.local_hits.saturating_add(1);
        Ok(Some(bytes))
    }

    pub fn get_block(&self, slot: u64) -> Result<Option<Block>, ArchiveV2Error> {
        let Some(manifest) = self.manifest_for_slot(slot) else {
            return Ok(None);
        };
        self.ensure_deep_history()?;
        self.read_authenticated_point(manifest, |path| {
            ArchiveV2SegmentCodec::decode_block_at_path(path, manifest, &self.identity, slot)
        })
    }

    pub fn get_block_by_hash(&self, hash: &Hash) -> Result<Option<Block>, ArchiveV2Error> {
        self.ensure_deep_history()?;
        for entry in self.catalog.entries.iter().rev() {
            let manifest = self
                .catalog
                .active_manifest(&entry.manifest.segment_object_hash)?;
            let segment = self.load_segment(manifest)?;
            if let Some(block) = segment.blocks.iter().find(|block| block.hash() == *hash) {
                return Ok(Some(block.clone()));
            }
        }
        Ok(None)
    }

    pub fn get_transaction(&self, signature: &Hash) -> Result<Option<Transaction>, ArchiveV2Error> {
        self.ensure_deep_history()?;
        for entry in self.catalog.entries.iter().rev() {
            let manifest = self
                .catalog
                .active_manifest(&entry.manifest.segment_object_hash)?;
            if !manifest.transaction_filter.might_contain(signature) {
                continue;
            }
            if let Some((transaction, _)) = self.read_authenticated_point(manifest, |path| {
                ArchiveV2SegmentCodec::decode_transaction_at_path(
                    path,
                    manifest,
                    &self.identity,
                    signature,
                )
            })? {
                return Ok(Some(transaction));
            }
        }
        Ok(None)
    }

    pub fn get_transaction_slot(&self, signature: &Hash) -> Result<Option<u64>, ArchiveV2Error> {
        self.ensure_deep_history()?;
        for entry in self.catalog.entries.iter().rev() {
            let manifest = self
                .catalog
                .active_manifest(&entry.manifest.segment_object_hash)?;
            if !manifest.transaction_filter.might_contain(signature) {
                continue;
            }
            if let Some((_, slot)) = self.read_authenticated_point(manifest, |path| {
                ArchiveV2SegmentCodec::decode_transaction_at_path(
                    path,
                    manifest,
                    &self.identity,
                    signature,
                )
            })? {
                return Ok(Some(slot));
            }
        }
        Ok(None)
    }

    pub fn category_rows(
        &self,
        category: &str,
        start_slot: u64,
        end_slot: u64,
    ) -> Result<super::ArchiveV2Rows, ArchiveV2Error> {
        self.category_rows_with_prefix(category, start_slot, end_slot, &[])
    }

    /// Visit authenticated segments in catalog order without filling the
    /// decoded-segment LRU. Only one new decoded segment is retained by this
    /// method at a time. The callback must filter blocks at the range edges.
    ///
    /// The limit applies to declared uncompressed frame payload, not total
    /// allocator usage. Codec allocations, encoded bytes, existing reader
    /// caches and any data retained by the callback need separate budgeting.
    /// A callback error aborts traversal; callers must discard partial digests.
    pub fn visit_segments_in_range<F>(
        &self,
        start_slot: u64,
        end_slot: u64,
        max_segment_payload_bytes: u64,
        mut visit: F,
    ) -> Result<(), ArchiveV2Error>
    where
        F: FnMut(&ArchiveV2DecodedSegment) -> Result<(), ArchiveV2Error>,
    {
        self.ensure_deep_history()?;
        if end_slot < start_slot || max_segment_payload_bytes == 0 {
            return Err(ArchiveV2Error::Bounds(
                "invalid segment visitor range or payload limit".to_string(),
            ));
        }
        for entry in &self.catalog.entries {
            let manifest = self
                .catalog
                .active_manifest(&entry.manifest.segment_object_hash)?;
            if manifest.end_slot < start_slot || manifest.start_slot > end_slot {
                continue;
            }
            let payload_bytes = manifest.frames.iter().try_fold(0u64, |total, frame| {
                total.checked_add(u64::from(frame.uncompressed_bytes))
            });
            if payload_bytes.is_none_or(|bytes| bytes > max_segment_payload_bytes) {
                return Err(ArchiveV2Error::Bounds(format!(
                    "segment {} exceeds the visitor payload limit",
                    manifest.segment_object_hash
                )));
            }
            let bytes = self.acquire_object(manifest)?;
            let decoded = ArchiveV2SegmentCodec::decode(&bytes, manifest, &self.identity)?;
            drop(bytes);
            visit(&decoded)?;
        }
        Ok(())
    }

    /// Visit one authenticated public-index category in segment order. This
    /// keeps one segment's selected index rows at a time. A consumer requiring
    /// global key order or duplicate detection must provide its own bounded
    /// merge/index; segment order is not global key order.
    pub fn visit_category_rows<F>(
        &self,
        category: &str,
        start_slot: u64,
        end_slot: u64,
        max_segment_payload_bytes: u64,
        mut visit: F,
    ) -> Result<(), ArchiveV2Error>
    where
        F: FnMut(&super::ArchiveV2PublicRow) -> Result<(), ArchiveV2Error>,
    {
        self.ensure_deep_history()?;
        if end_slot < start_slot
            || max_segment_payload_bytes == 0
            || matches!(
                category,
                "slots" | "blocks" | "transactions" | "tx_by_slot" | "tx_to_slot"
            )
        {
            return Err(ArchiveV2Error::Bounds(
                "invalid public-index visitor request".to_string(),
            ));
        }
        for entry in &self.catalog.entries {
            let manifest = self
                .catalog
                .active_manifest(&entry.manifest.segment_object_hash)?;
            if manifest.end_slot < start_slot || manifest.start_slot > end_slot {
                continue;
            }
            let payload_bytes = manifest.frames.iter().try_fold(0u64, |total, frame| {
                total.checked_add(u64::from(frame.uncompressed_bytes))
            });
            if payload_bytes.is_none_or(|bytes| bytes > max_segment_payload_bytes) {
                return Err(ArchiveV2Error::Bounds(format!(
                    "segment {} exceeds the visitor payload limit",
                    manifest.segment_object_hash
                )));
            }
            let indexes = self.public_indexes(
                manifest,
                super::codec::ArchiveV2IndexRowQuery {
                    category,
                    prefix: &[],
                    start_slot,
                    end_slot,
                },
            )?;
            if let Some(rows) = indexes.categories.get(category) {
                for row in rows {
                    if (start_slot..=end_slot).contains(&row.slot) {
                        visit(row)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Return the next globally ordered page without retaining a whole
    /// category. Equal duplicate rows collapse; conflicting rows fail in the
    /// page containing their key. The caller must consume every page for a
    /// complete-category verification.
    pub fn category_rows_page(
        &self,
        query: ArchiveV2CategoryCursor<'_>,
    ) -> Result<(super::ArchiveV2Rows, bool), ArchiveV2Error> {
        self.ensure_deep_history()?;
        if query.end_slot < query.start_slot
            || query.max_rows == 0
            || query.max_rows > 50_000
            || query.max_payload_bytes == 0
            || query.max_payload_bytes > 64 * 1024 * 1024
            || matches!(
                query.category,
                "slots" | "blocks" | "transactions" | "tx_by_slot" | "tx_to_slot"
            )
        {
            return Err(ArchiveV2Error::Bounds(
                "invalid public-index category cursor or page limits".to_string(),
            ));
        }
        let mut rows = BTreeMap::<Vec<u8>, Vec<u8>>::new();
        let mut payload_bytes = 0usize;
        let mut has_more = false;
        for entry in &self.catalog.entries {
            let manifest = self
                .catalog
                .active_manifest(&entry.manifest.segment_object_hash)?;
            if manifest.end_slot < query.start_slot || manifest.start_slot > query.end_slot {
                continue;
            }
            let indexes = self.public_indexes(
                manifest,
                super::codec::ArchiveV2IndexRowQuery {
                    category: query.category,
                    prefix: &[],
                    start_slot: query.start_slot,
                    end_slot: query.end_slot,
                },
            )?;
            if let Some(category_rows) = indexes.categories.get(query.category) {
                for row in category_rows {
                    if !(query.start_slot..=query.end_slot).contains(&row.slot)
                        || query.after_key.is_some_and(|key| row.key.as_slice() <= key)
                    {
                        continue;
                    }
                    if let Some(existing) = rows.get(&row.key) {
                        if existing != &row.value {
                            return Err(ArchiveV2Error::Ordering(format!(
                                "{} has conflicting authenticated rows at key {}",
                                query.category,
                                hex::encode(&row.key)
                            )));
                        }
                        continue;
                    }
                    // Once a page is truncated, larger keys belong to a later
                    // page. Avoid even a temporary clone of their payloads.
                    if has_more && rows.last_key_value().is_some_and(|(key, _)| row.key > *key) {
                        continue;
                    }
                    let bytes = row.key.len().saturating_add(row.value.len());
                    if bytes > query.max_payload_bytes {
                        return Err(ArchiveV2Error::Bounds(format!(
                            "{} row exceeds the cursor payload limit",
                            query.category
                        )));
                    }
                    payload_bytes += bytes;
                    rows.insert(row.key.clone(), row.value.clone());
                    while rows.len() > query.max_rows || payload_bytes > query.max_payload_bytes {
                        let (key, value) = rows.pop_last().expect("nonempty over-budget page");
                        payload_bytes -= key.len() + value.len();
                        has_more = true;
                    }
                }
            }
        }
        Ok((rows.into_iter().collect(), has_more))
    }

    /// Filter authenticated rows before accumulating results across segments.
    /// Empty prefixes preserve the complete category used by offline audits.
    pub fn category_rows_with_prefix(
        &self,
        category: &str,
        start_slot: u64,
        end_slot: u64,
        prefix: &[u8],
    ) -> Result<super::ArchiveV2Rows, ArchiveV2Error> {
        self.ensure_deep_history()?;
        if end_slot < start_slot {
            return Err(ArchiveV2Error::Bounds(
                "category range end precedes start".to_string(),
            ));
        }
        let mut rows = BTreeMap::new();
        for entry in &self.catalog.entries {
            let manifest = self
                .catalog
                .active_manifest(&entry.manifest.segment_object_hash)?;
            if manifest.end_slot < start_slot || manifest.start_slot > end_slot {
                continue;
            }
            if !matches!(
                category,
                "slots" | "blocks" | "transactions" | "tx_by_slot" | "tx_to_slot"
            ) {
                let indexes = self.public_indexes(
                    manifest,
                    super::codec::ArchiveV2IndexRowQuery {
                        category,
                        prefix,
                        start_slot,
                        end_slot,
                    },
                )?;
                if let Some(category_rows) = indexes.categories.get(category) {
                    let start = category_rows.partition_point(|row| row.key.as_slice() < prefix);
                    for row in category_rows[start..]
                        .iter()
                        .take_while(|row| row.key.starts_with(prefix))
                    {
                        if (start_slot..=end_slot).contains(&row.slot) {
                            insert_category_row(
                                &mut rows,
                                row.key.clone(),
                                row.value.clone(),
                                category,
                            )?;
                        }
                    }
                }
                continue;
            }
            let segment = self.load_segment(manifest)?;
            match category {
                "slots" => {
                    for block in &segment.blocks {
                        if (start_slot..=end_slot).contains(&block.header.slot) {
                            insert_category_row(
                                &mut rows,
                                block.header.slot.to_be_bytes().to_vec(),
                                block.hash().0.to_vec(),
                                category,
                            )?;
                        }
                    }
                }
                "blocks" => {
                    for block in &segment.blocks {
                        if (start_slot..=end_slot).contains(&block.header.slot) {
                            let mut value = vec![0xBC];
                            value.extend_from_slice(
                                &serialize_legacy_bincode(block, "archive v2 reconstructed block")
                                    .map_err(ArchiveV2Error::Codec)?,
                            );
                            insert_category_row(
                                &mut rows,
                                block.hash().0.to_vec(),
                                value,
                                category,
                            )?;
                        }
                    }
                }
                "transactions" => {
                    for transaction in &segment.transactions {
                        let Some((_, _, slot, _)) = segment
                            .indexes
                            .transactions_by_signature
                            .get(&transaction.signature())
                        else {
                            return Err(ArchiveV2Error::Ordering(
                                "transaction body has no public index location".to_string(),
                            ));
                        };
                        if (start_slot..=end_slot).contains(slot) {
                            let mut value = vec![0xBC];
                            value.extend_from_slice(
                                &serialize_legacy_bincode(
                                    transaction,
                                    "archive v2 reconstructed transaction",
                                )
                                .map_err(ArchiveV2Error::Codec)?,
                            );
                            insert_category_row(
                                &mut rows,
                                transaction.signature().0.to_vec(),
                                value,
                                category,
                            )?;
                        }
                    }
                }
                "tx_by_slot" => {
                    for block in &segment.blocks {
                        if !(start_slot..=end_slot).contains(&block.header.slot) {
                            continue;
                        }
                        for (ordinal, transaction) in block.transactions.iter().enumerate() {
                            let mut key = Vec::with_capacity(16);
                            key.extend_from_slice(&block.header.slot.to_be_bytes());
                            key.extend_from_slice(&(ordinal as u64).to_be_bytes());
                            insert_category_row(
                                &mut rows,
                                key,
                                transaction.signature().0.to_vec(),
                                category,
                            )?;
                        }
                    }
                }
                "tx_to_slot" => {
                    for (signature, (_, _, slot, _)) in &segment.indexes.transactions_by_signature {
                        if (start_slot..=end_slot).contains(slot) {
                            insert_category_row(
                                &mut rows,
                                signature.0.to_vec(),
                                slot.to_be_bytes().to_vec(),
                                category,
                            )?;
                        }
                    }
                }
                _ => unreachable!("raw categories handled by authenticated indexes above"),
            }
        }
        Ok(rows
            .into_iter()
            .filter(|(key, _)| key.starts_with(prefix))
            .collect())
    }

    pub fn category_value(
        &self,
        category: &str,
        key: &[u8],
    ) -> Result<Option<(u64, Vec<u8>)>, ArchiveV2Error> {
        self.ensure_deep_history()?;
        for entry in self.catalog.entries.iter().rev() {
            let manifest = self
                .catalog
                .active_manifest(&entry.manifest.segment_object_hash)?;
            let indexes = self.public_indexes(
                manifest,
                super::codec::ArchiveV2IndexRowQuery {
                    category,
                    prefix: key,
                    start_slot: 0,
                    end_slot: u64::MAX,
                },
            )?;
            if let Some(rows) = indexes.categories.get(category) {
                if let Ok(index) = rows.binary_search_by(|row| row.key.as_slice().cmp(key)) {
                    let row = &rows[index];
                    return Ok(Some((row.slot, row.value.clone())));
                }
            }
        }
        Ok(None)
    }

    /// Look up a public row whose canonical block slot is already known.
    /// A missing receipt must not search unrelated segments or sources.
    pub fn category_value_at_slot(
        &self,
        category: &str,
        key: &[u8],
        slot: u64,
    ) -> Result<Option<Vec<u8>>, ArchiveV2Error> {
        self.ensure_deep_history()?;
        let Some(manifest) = self.manifest_for_slot(slot) else {
            return Ok(None);
        };
        let indexes = self.public_indexes(
            manifest,
            super::codec::ArchiveV2IndexRowQuery {
                category,
                prefix: key,
                start_slot: slot,
                end_slot: slot,
            },
        )?;
        Ok(indexes.categories.get(category).and_then(|rows| {
            rows.binary_search_by(|row| row.key.as_slice().cmp(key))
                .ok()
                .map(|index| rows[index].value.clone())
        }))
    }

    fn manifest_for_slot(&self, slot: u64) -> Option<&ArchiveV2Manifest> {
        self.catalog
            .entries
            .binary_search_by(|entry| {
                if entry.manifest.end_slot < slot {
                    std::cmp::Ordering::Less
                } else if entry.manifest.start_slot > slot {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .ok()
            .and_then(|index| self.catalog.entries.get(index))
            .and_then(|entry| {
                self.catalog
                    .active_manifest(&entry.manifest.segment_object_hash)
                    .ok()
            })
    }

    fn ensure_deep_history(&self) -> Result<(), ArchiveV2Error> {
        if self.config.role == ArchiveV2Role::Consensus {
            return Err(ArchiveV2Error::Role(
                "consensus role does not serve Archive V2 deep history".to_string(),
            ));
        }
        Ok(())
    }

    fn load_segment(
        &self,
        manifest: &ArchiveV2Manifest,
    ) -> Result<Arc<ArchiveV2DecodedSegment>, ArchiveV2Error> {
        if let Some(decoded) = self
            .decoded
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&manifest.segment_object_hash)
            .cloned()
        {
            return Ok(decoded);
        }

        let bytes = self.acquire_object(manifest)?;
        if let Some(decoded) = self
            .decoded
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&manifest.segment_object_hash)
            .cloned()
        {
            return Ok(decoded);
        }
        match ArchiveV2SegmentCodec::decode(&bytes, manifest, &self.identity) {
            Ok(decoded) => self.remember(decoded),
            Err(error) => {
                self.status_lock().last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    fn read_authenticated_point<T, F>(
        &self,
        manifest: &ArchiveV2Manifest,
        decode: F,
    ) -> Result<T, ArchiveV2Error>
    where
        F: Fn(&Path) -> Result<T, ArchiveV2Error>,
    {
        let local = object_path(&self.config.root, &manifest.segment_object_hash);
        if let Some(value) =
            self.try_authenticated_point_path(&local, &manifest.segment_object_hash, true, &decode)?
        {
            return Ok(value);
        }

        if let Some(cache_root) = self.config.cache_root.as_ref() {
            let _cache_guard = self
                .cache_io_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let cached = object_path(cache_root, &manifest.segment_object_hash);
            if let Some(value) = self.try_authenticated_point_path(
                &cached,
                &manifest.segment_object_hash,
                false,
                &decode,
            )? {
                return Ok(value);
            }
        }

        if !self.config.allow_remote_fetch {
            return Err(ArchiveV2Error::Unavailable(format!(
                "segment {} is not locally readable",
                manifest.segment_object_hash
            )));
        }
        let _cache_guard = self
            .cache_io_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let cache_root = self.config.cache_root.as_ref().ok_or_else(|| {
            ArchiveV2Error::Role("remote Archive V2 point read has no configured cache".to_string())
        })?;
        let cached = object_path(cache_root, &manifest.segment_object_hash);
        // Another reader may have filled the immutable cache while this reader
        // waited. Recheck under the same lock that protects eviction and
        // persistence so one miss produces at most one remote materialization.
        if let Some(value) = self.try_authenticated_point_path(
            &cached,
            &manifest.segment_object_hash,
            false,
            &decode,
        )? {
            return Ok(value);
        }
        for source in &self.config.sources {
            if !source.authenticated() {
                continue;
            }
            let fetched = match source.fetch(&manifest.segment_object_hash) {
                Ok(Some(bytes)) => bytes,
                Ok(None) => continue,
                Err(error) => {
                    let mut status = self.status_lock();
                    status.source_failures = status.source_failures.saturating_add(1);
                    status.last_error = Some(format!("{}: {error}", source.name()));
                    continue;
                }
            };
            if let Err(error) = self.verify_object_bytes(&fetched, manifest) {
                self.quarantine_bytes(&manifest.segment_object_hash, &fetched)?;
                let mut status = self.status_lock();
                status.source_failures = status.source_failures.saturating_add(1);
                status.quarantined_objects = status.quarantined_objects.saturating_add(1);
                status.last_error = Some(format!("{}: {error}", source.name()));
                continue;
            }
            self.persist_cache_locked(&manifest.segment_object_hash, &fetched)?;
            drop(fetched);
            {
                let mut status = self.status_lock();
                status.remote_fetches = status.remote_fetches.saturating_add(1);
                status.cache_bytes = self
                    .config
                    .cache_root
                    .as_deref()
                    .map(cache_size)
                    .transpose()?
                    .unwrap_or(0);
            }
            if let Some(value) = self.try_authenticated_point_path(
                &cached,
                &manifest.segment_object_hash,
                false,
                &decode,
            )? {
                return Ok(value);
            }
        }
        Err(ArchiveV2Error::Unavailable(format!(
            "no authenticated source supplied readable segment {}",
            manifest.segment_object_hash
        )))
    }

    fn try_authenticated_point_path<T, F>(
        &self,
        path: &Path,
        expected_hash: &Hash,
        local: bool,
        decode: &F,
    ) -> Result<Option<T>, ArchiveV2Error>
    where
        F: Fn(&Path) -> Result<T, ArchiveV2Error>,
    {
        match fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => {
                return Err(ArchiveV2Error::Io(format!(
                    "Archive V2 object path {} is not a regular file",
                    path.display()
                )))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(ArchiveV2Error::Io(format!(
                    "failed reading Archive V2 object metadata {}: {error}",
                    path.display()
                )))
            }
        }
        match decode(path) {
            Ok(value) => {
                let mut status = self.status_lock();
                if local {
                    status.local_hits = status.local_hits.saturating_add(1);
                } else {
                    status.cache_hits = status.cache_hits.saturating_add(1);
                }
                Ok(Some(value))
            }
            Err(error) => {
                self.quarantine_path(path, expected_hash)?;
                let mut status = self.status_lock();
                status.quarantined_objects = status.quarantined_objects.saturating_add(1);
                status.last_error = Some(error.to_string());
                Ok(None)
            }
        }
    }

    fn acquire_object(&self, manifest: &ArchiveV2Manifest) -> Result<Vec<u8>, ArchiveV2Error> {
        let local = object_path(&self.config.root, &manifest.segment_object_hash);
        match fs::read(&local) {
            Ok(bytes) => match self.verify_object_bytes(&bytes, manifest) {
                Ok(()) => {
                    let mut status = self.status_lock();
                    status.local_hits = status.local_hits.saturating_add(1);
                    return Ok(bytes);
                }
                Err(error) => {
                    self.quarantine_path(&local, &manifest.segment_object_hash)?;
                    let mut status = self.status_lock();
                    status.quarantined_objects = status.quarantined_objects.saturating_add(1);
                    status.last_error = Some(error.to_string());
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ArchiveV2Error::Io(format!(
                    "failed reading {}: {error}",
                    local.display()
                )))
            }
        }

        // Hold the cache transaction across lookup, authenticated source fetch,
        // eviction, and immutable persistence. This bounds concurrent cache
        // misses and prevents another writer from evicting an object between
        // admission and use.
        let _cache_guard = self.config.cache_root.as_ref().map(|_| {
            self.cache_io_lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        });
        if let Some(cache_root) = self.config.cache_root.as_ref() {
            let cached = object_path(cache_root, &manifest.segment_object_hash);
            match fs::read(&cached) {
                Ok(bytes) => match self.verify_object_bytes(&bytes, manifest) {
                    Ok(()) => {
                        let mut status = self.status_lock();
                        status.cache_hits = status.cache_hits.saturating_add(1);
                        return Ok(bytes);
                    }
                    Err(error) => {
                        self.quarantine_path(&cached, &manifest.segment_object_hash)?;
                        let mut status = self.status_lock();
                        status.quarantined_objects = status.quarantined_objects.saturating_add(1);
                        status.last_error = Some(error.to_string());
                    }
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(ArchiveV2Error::Io(format!(
                        "failed reading {}: {error}",
                        cached.display()
                    )))
                }
            }
        }

        if !self.config.allow_remote_fetch {
            return Err(ArchiveV2Error::Unavailable(format!(
                "segment {} is not local",
                manifest.segment_object_hash
            )));
        }
        for source in &self.config.sources {
            if !source.authenticated() {
                continue;
            }
            let fetched = match source.fetch(&manifest.segment_object_hash) {
                Ok(Some(bytes)) => bytes,
                Ok(None) => continue,
                Err(error) => {
                    let mut status = self.status_lock();
                    status.source_failures = status.source_failures.saturating_add(1);
                    status.last_error = Some(format!("{}: {error}", source.name()));
                    continue;
                }
            };
            if let Err(error) = self.verify_object_bytes(&fetched, manifest) {
                self.quarantine_bytes(&manifest.segment_object_hash, &fetched)?;
                let mut status = self.status_lock();
                status.source_failures = status.source_failures.saturating_add(1);
                status.quarantined_objects = status.quarantined_objects.saturating_add(1);
                status.last_error = Some(format!("{}: {error}", source.name()));
                continue;
            }
            self.persist_cache_locked(&manifest.segment_object_hash, &fetched)?;
            let mut status = self.status_lock();
            status.remote_fetches = status.remote_fetches.saturating_add(1);
            status.cache_bytes = self
                .config
                .cache_root
                .as_deref()
                .map(cache_size)
                .transpose()?
                .unwrap_or(0);
            drop(status);
            return Ok(fetched);
        }
        Err(ArchiveV2Error::Unavailable(format!(
            "no authenticated source supplied valid segment {}",
            manifest.segment_object_hash
        )))
    }

    fn verify_object_bytes(
        &self,
        bytes: &[u8],
        manifest: &ArchiveV2Manifest,
    ) -> Result<(), ArchiveV2Error> {
        if Hash::hash(bytes) != manifest.segment_object_hash {
            return Err(ArchiveV2Error::WrongObjectHash);
        }
        if self
            .verified_objects
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&manifest.segment_object_hash)
        {
            return Ok(());
        }
        // Cache admission authenticates the immutable object but deliberately
        // does not inflate the whole segment into the live validator heap.
        // Point reads use seekable frame decoding; whole-segment callers reach
        // `load_segment` explicitly and remain bounded by the decoded LRU.
        ArchiveV2SegmentCodec::verify_seekable_object(bytes, manifest, &self.identity)?;
        let verified_count = {
            let mut verified = self
                .verified_objects
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            verified.insert(manifest.segment_object_hash);
            verified.len() as u64
        };
        self.status_lock().verified_objects = verified_count;
        Ok(())
    }

    fn remember(
        &self,
        decoded: ArchiveV2DecodedSegment,
    ) -> Result<Arc<ArchiveV2DecodedSegment>, ArchiveV2Error> {
        let object_hash = decoded.manifest.segment_object_hash;
        let decoded = Arc::new(decoded);
        self.decoded
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .put(object_hash, Arc::clone(&decoded));
        Ok(decoded)
    }

    fn status_lock(&self) -> std::sync::MutexGuard<'_, ArchiveV2ReaderStatus> {
        self.status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn quarantine_path(&self, path: &Path, expected_hash: &Hash) -> Result<(), ArchiveV2Error> {
        let root = path
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| ArchiveV2Error::Io("object path has no archive root".to_string()))?;
        let quarantine = root.join("quarantine").join(format!(
            "{}-{}-{}.corrupt",
            expected_hash,
            std::process::id(),
            QUARANTINE_NONCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(
            quarantine
                .parent()
                .expect("quarantine path always has a parent"),
        )?;
        match fs::rename(path, quarantine) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn quarantine_bytes(&self, expected_hash: &Hash, bytes: &[u8]) -> Result<(), ArchiveV2Error> {
        let root = self.config.cache_root.as_ref().unwrap_or(&self.config.root);
        let actual = Hash::hash(bytes);
        let path = root
            .join("quarantine")
            .join(format!("{expected_hash}-{actual}.corrupt"));
        write_new_synced(&path, bytes)
    }

    /// Persists one verified cache object while the caller holds
    /// `cache_io_lock` across the complete cache transaction.
    fn persist_cache_locked(&self, object_hash: &Hash, bytes: &[u8]) -> Result<(), ArchiveV2Error> {
        let cache =
            self.config.cache_root.as_ref().ok_or_else(|| {
                ArchiveV2Error::Role("verified cache is not configured".to_string())
            })?;
        if bytes.len() as u64 > self.config.cache_quota_bytes {
            return Err(ArchiveV2Error::Bounds(format!(
                "segment {} exceeds cache quota {}",
                bytes.len(),
                self.config.cache_quota_bytes
            )));
        }
        evict_cache_until(
            cache,
            self.config
                .cache_quota_bytes
                .saturating_sub(bytes.len() as u64),
        )?;
        let path = object_path(cache, object_hash);
        write_new_synced(&path, bytes)
    }
}

fn insert_category_row(
    rows: &mut BTreeMap<Vec<u8>, Vec<u8>>,
    key: Vec<u8>,
    value: Vec<u8>,
    category: &str,
) -> Result<(), ArchiveV2Error> {
    match rows.insert(key, value.clone()) {
        Some(existing) if existing != value => Err(ArchiveV2Error::Ordering(format!(
            "category {category} has a conflicting duplicate"
        ))),
        _ => Ok(()),
    }
}

fn object_path(root: &Path, object_hash: &Hash) -> PathBuf {
    root.join("objects")
        .join(format!("{}.av2s", object_hash.to_hex()))
}

fn write_new_synced(path: &Path, bytes: &[u8]) -> Result<(), ArchiveV2Error> {
    let parent = path
        .parent()
        .ok_or_else(|| ArchiveV2Error::Io("object path has no parent".to_string()))?;
    fs::create_dir_all(parent)?;
    match OpenOptions::new().create_new(true).write(true).open(path) {
        Ok(mut file) => {
            file.write_all(bytes)?;
            file.sync_all()?;
            OpenOptions::new().read(true).open(parent)?.sync_all()?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let existing = fs::read(path)?;
            if existing == bytes {
                Ok(())
            } else {
                Err(ArchiveV2Error::Ordering(format!(
                    "immutable object {} already exists with different bytes",
                    path.display()
                )))
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn cache_size(root: &Path) -> Result<u64, ArchiveV2Error> {
    let mut total = 0u64;
    for kind in ["objects", "indexes"] {
        let directory = root.join(kind);
        if !directory.exists() {
            continue;
        }
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                total = total.saturating_add(entry.metadata()?.len());
            }
        }
    }
    Ok(total)
}

fn evict_cache_until(root: &Path, target_bytes: u64) -> Result<(), ArchiveV2Error> {
    let mut files = Vec::new();
    let mut total = 0u64;
    // Index bundles serve many queries and are much smaller than bodies. Keep
    // them preferentially, while charging both tiers to the same hard quota.
    for (priority, kind) in ["objects", "indexes"].into_iter().enumerate() {
        let directory = root.join(kind);
        if !directory.exists() {
            continue;
        }
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let metadata = entry.metadata()?;
            total = total.saturating_add(metadata.len());
            files.push((
                priority,
                metadata
                    .modified()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                entry.path(),
                metadata.len(),
            ));
        }
    }
    files.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.cmp(&right.2))
    });
    for (_, _, path, bytes) in files {
        if total <= target_bytes {
            break;
        }
        fs::remove_file(&path)?;
        total = total.saturating_sub(bytes);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::archive_v2::{
        ArchiveV2Catalog, ArchiveV2CodecConfig, ArchiveV2PublicRow, ArchiveV2SegmentContents,
    };
    use crate::{Block, Instruction, Message, Pubkey, Transaction};

    fn fixture(
        root: &Path,
    ) -> (
        ArchiveV2Identity,
        ArchiveV2Catalog,
        Vec<u8>,
        ArchiveV2Manifest,
    ) {
        let identity = ArchiveV2Identity {
            network_id: "reader-testnet".to_string(),
            genesis_hash: Hash::hash(b"reader-genesis"),
        };
        let transaction = Transaction::new(Message::new(
            vec![Instruction {
                program_id: Pubkey([6; 32]),
                accounts: vec![Pubkey([7; 32])],
                data: vec![8; 32],
            }],
            Hash::hash(b"reader-recent-blockhash"),
        ));
        let block = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"reader-state"),
            [8; 32],
            vec![transaction],
            1,
        );
        let mut contents = ArchiveV2SegmentContents::from_blocks(vec![block]);
        contents.public_categories.insert(
            "events".to_string(),
            vec![ArchiveV2PublicRow {
                slot: 0,
                key: b"event-key".to_vec(),
                value: b"event-value".to_vec(),
            }],
        );
        let (bytes, manifest) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &contents,
            &ArchiveV2CodecConfig {
                target_frame_bytes: 1024 * 1024,
                ..ArchiveV2CodecConfig::default()
            },
        )
        .unwrap();
        let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
        catalog.append(manifest.clone()).unwrap();
        catalog.store_atomic(&root.join("catalog.av2")).unwrap();
        (identity, catalog, bytes, manifest)
    }

    #[test]
    fn checkpoint_handoff_uses_verified_immutable_catalog_and_checks_coverage() {
        let local = tempdir().unwrap();
        let (identity, catalog, _, _) = fixture(local.path());
        let catalog_path = local.path().join("catalog.av2");
        let config = || ArchiveV2ReaderConfig {
            role: ArchiveV2Role::FullArchive,
            root: local.path().to_path_buf(),
            cache_root: None,
            cache_quota_bytes: 0,
            max_decoded_segments: 1,
            allow_remote_fetch: false,
            sources: Vec::new(),
        };
        let reader = ArchiveV2Reader::open(identity.clone(), &catalog_path, config()).unwrap();
        for start in [0, 1] {
            assert_eq!(
                reader.checkpoint_handoff_root(start).unwrap(),
                catalog.checkpoint_handoff_root(start).unwrap(),
            );
        }
        for start in [2, u64::MAX] {
            assert!(reader.checkpoint_handoff_root(start).is_err());
            assert!(catalog.checkpoint_handoff_root(start).is_err());
        }
        let mut tampered = catalog.clone();
        tampered.catalog_root = Hash::default();
        assert!(tampered.checkpoint_handoff_root(0).is_err());
        assert!(tampered.checkpoint_handoff_root(1).is_err());

        // A file changing after admission cannot mutate the owned catalog.
        // Reopening it must still perform full validation and fail closed.
        fs::write(&catalog_path, b"corrupt catalog replacement").unwrap();
        assert_eq!(
            reader.checkpoint_handoff_root(1).unwrap(),
            catalog.checkpoint_handoff_root(1).unwrap(),
        );
        assert!(ArchiveV2Reader::open(identity, &catalog_path, config()).is_err());
        assert_eq!(reader.status().remote_fetches, 0);
        assert!(reader.decoded.lock().unwrap().is_empty());
    }

    fn paginated_index_fixture(root: &Path, conflict: bool) -> ArchiveV2Reader {
        let identity = ArchiveV2Identity {
            network_id: "cursor-testnet".to_string(),
            genesis_hash: Hash::hash(b"cursor-genesis"),
        };
        let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
        let mut previous_segment = None;
        let mut previous_block = Hash::default();
        fs::create_dir(root.join("objects")).unwrap();
        for (slot, keys) in [vec![b'a', b'c', b'f'], vec![b'b', b'c', b'd']]
            .into_iter()
            .enumerate()
        {
            let block = Block::new_with_timestamp(
                slot as u64,
                previous_block,
                Hash::hash(&[slot as u8]),
                [8; 32],
                Vec::new(),
                slot as u64 + 1,
            );
            let mut contents = ArchiveV2SegmentContents::from_blocks(vec![block]);
            contents.public_categories.insert(
                "events".to_string(),
                keys.into_iter()
                    .map(|key| ArchiveV2PublicRow {
                        slot: slot as u64,
                        key: vec![key],
                        value: vec![
                            if conflict && slot == 1 && key == b'c' {
                                b'x'
                            } else {
                                key
                            };
                            3
                        ],
                    })
                    .collect(),
            );
            let (bytes, manifest) = ArchiveV2SegmentCodec::encode(
                identity.clone(),
                previous_segment,
                previous_block,
                &contents,
                &ArchiveV2CodecConfig::default(),
            )
            .unwrap();
            previous_segment = Some(manifest.segment_object_hash);
            previous_block = manifest.last_block_hash;
            fs::write(object_path(root, &manifest.segment_object_hash), bytes).unwrap();
            catalog.append(manifest).unwrap();
        }
        catalog.store_atomic(&root.join("catalog.av2")).unwrap();
        ArchiveV2Reader::open(
            identity,
            &root.join("catalog.av2"),
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
        .unwrap()
    }

    #[test]
    fn known_slot_row_lookup_does_not_consult_unrelated_segments() {
        let local = tempdir().unwrap();
        let reader = paginated_index_fixture(local.path(), true);
        assert_eq!(
            reader.category_value_at_slot("events", b"c", 0).unwrap(),
            Some(b"ccc".to_vec())
        );
        assert_eq!(
            reader.category_value_at_slot("events", b"c", 1).unwrap(),
            Some(b"xxx".to_vec())
        );
        // A key from another slot is not a receipt for the requested block.
        assert_eq!(
            reader.category_value_at_slot("events", b"b", 0).unwrap(),
            None
        );
        assert_eq!(
            reader.category_value_at_slot("events", b"a", 2).unwrap(),
            None
        );

        let unrelated = reader.catalog.entries[1].manifest.segment_object_hash;
        fs::remove_file(object_path(local.path(), &unrelated)).unwrap();
        assert_eq!(
            reader
                .category_value_at_slot("events", b"missing", 0)
                .unwrap(),
            None
        );
        assert!(
            reader.category_value("events", b"missing").is_err(),
            "the old unbounded lookup reaches the unrelated unavailable segment"
        );
        assert!(reader.category_value_at_slot("events", b"b", 1).is_err());
        assert!(reader.decoded.lock().unwrap().is_empty());
    }

    #[test]
    fn index_visitor_preserves_segment_rows_and_aborts_on_source_or_callback_failure() {
        let local = tempdir().unwrap();
        let reader = paginated_index_fixture(local.path(), false);
        let mut rows = Vec::new();
        reader
            .visit_category_rows("events", 0, 1, 1024 * 1024, |row| {
                rows.push((row.slot, row.key.clone()));
                Ok(())
            })
            .unwrap();
        assert_eq!(
            rows,
            vec![
                (0, b"a".to_vec()),
                (0, b"c".to_vec()),
                (0, b"f".to_vec()),
                (1, b"b".to_vec()),
                (1, b"c".to_vec()),
                (1, b"d".to_vec())
            ]
        );
        assert!(reader.decoded.lock().unwrap().is_empty());
        let mut visited = 0;
        assert!(reader
            .visit_category_rows("events", 0, 1, 1, |_| {
                visited += 1;
                Ok(())
            })
            .is_err());
        assert_eq!(visited, 0);
        assert!(reader
            .visit_category_rows("events", 0, 1, 1024 * 1024, |_| {
                visited += 1;
                Err(ArchiveV2Error::Ordering("callback failure".to_string()))
            })
            .is_err());
        assert_eq!(visited, 1);
        let manifest = &reader.catalog.entries[1].manifest;
        fs::write(
            object_path(local.path(), &manifest.segment_object_hash),
            b"corrupt",
        )
        .unwrap();
        assert!(reader
            .visit_category_rows("events", 0, 1, 1024 * 1024, |_| Ok(()))
            .is_err());
    }

    #[test]
    fn category_cursor_merges_interleaved_segments_with_byte_and_row_bounds() {
        let local = tempdir().unwrap();
        let reader = paginated_index_fixture(local.path(), false);
        let expected = reader.category_rows("events", 0, 1).unwrap();
        for (max_rows, max_payload_bytes) in [(1, 64), (2, 64), (50, 8), (50, 64)] {
            let mut cursor = None;
            let mut actual = Vec::new();
            loop {
                let (rows, more) = reader
                    .category_rows_page(ArchiveV2CategoryCursor {
                        category: "events",
                        start_slot: 0,
                        end_slot: 1,
                        after_key: cursor.as_deref(),
                        max_rows,
                        max_payload_bytes,
                    })
                    .unwrap();
                assert!(rows.len() <= max_rows);
                assert!(
                    rows.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() <= max_payload_bytes
                );
                if more {
                    let next = rows
                        .last()
                        .expect("a continuing page must advance")
                        .0
                        .clone();
                    assert!(cursor.as_ref().is_none_or(|key| key < &next));
                    cursor = Some(next);
                }
                actual.extend(rows);
                if !more {
                    break;
                }
            }
            assert_eq!(actual, expected);
        }
        assert_eq!(expected.len(), 5);
        assert!(reader.decoded.lock().unwrap().is_empty());
    }

    #[test]
    fn segment_visitor_orders_and_filters_without_retaining_decoded_segments() {
        let local = tempdir().unwrap();
        let reader = paginated_index_fixture(local.path(), false);
        for (start, end, expected) in [(0, 1, vec![0, 1]), (1, 1, vec![1]), (2, 3, vec![])] {
            let mut slots = Vec::new();
            reader
                .visit_segments_in_range(start, end, 1024 * 1024, |segment| {
                    assert!(reader.decoded.lock().unwrap().is_empty());
                    slots.extend(segment.blocks.iter().map(|block| block.header.slot));
                    Ok(())
                })
                .unwrap();
            assert_eq!(slots, expected);
            assert!(reader.decoded.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn segment_visitor_enforces_bounds_authentication_and_callback_failure() {
        let local = tempdir().unwrap();
        let reader = paginated_index_fixture(local.path(), false);
        let mut calls = 0;
        assert!(reader
            .visit_segments_in_range(0, 1, 1, |_| {
                calls += 1;
                Ok(())
            })
            .is_err());
        assert_eq!(calls, 0);
        assert!(reader
            .visit_segments_in_range(1, 0, 1024, |_| Ok(()))
            .is_err());
        assert!(reader.visit_segments_in_range(0, 1, 0, |_| Ok(())).is_err());
        let error = reader
            .visit_segments_in_range(0, 1, 1024 * 1024, |_| {
                calls += 1;
                Err(ArchiveV2Error::Ordering("callback stopped".to_string()))
            })
            .unwrap_err();
        assert!(error.to_string().contains("callback stopped"));
        assert_eq!(calls, 1);

        let second = &reader.catalog.entries[1].manifest;
        fs::write(
            object_path(local.path(), &second.segment_object_hash),
            b"corrupt",
        )
        .unwrap();
        calls = 0;
        assert!(reader
            .visit_segments_in_range(0, 1, 1024 * 1024, |_| {
                calls += 1;
                Ok(())
            })
            .is_err());
        assert_eq!(calls, 1);
        assert!(reader.decoded.lock().unwrap().is_empty());
    }

    #[test]
    fn category_cursor_rejects_conflicts_on_later_pages_and_oversized_rows() {
        let local = tempdir().unwrap();
        let reader = paginated_index_fixture(local.path(), true);
        let query = |after_key, max_payload_bytes| ArchiveV2CategoryCursor {
            category: "events",
            start_slot: 0,
            end_slot: 1,
            after_key,
            max_rows: 1,
            max_payload_bytes,
        };
        assert!(reader.category_rows_page(query(None, 64)).is_ok());
        assert!(reader.category_rows_page(query(Some(b"b"), 64)).is_err());
        assert!(reader.category_rows_page(query(None, 3)).is_err());
        assert!(reader.category_rows_page(query(None, 0)).is_err());
    }

    #[test]
    #[ignore = "requires an explicitly supplied production catalog fixture"]
    fn checkpoint_handoff_production_catalog_parity_and_cost() {
        let path = std::env::var_os("LICHEN_TEST_HANDOFF_CATALOG")
            .map(PathBuf::from)
            .expect("set LICHEN_TEST_HANDOFF_CATALOG to a retained catalog fixture");
        let catalog = ArchiveV2Catalog::load(&path).unwrap();
        let local = tempdir().unwrap();
        let reader = ArchiveV2Reader::open(
            catalog.identity.clone(),
            &path,
            ArchiveV2ReaderConfig {
                role: ArchiveV2Role::FullArchive,
                root: local.path().to_path_buf(),
                cache_root: None,
                cache_quota_bytes: 0,
                max_decoded_segments: 1,
                allow_remote_fetch: false,
                sources: Vec::new(),
            },
        )
        .unwrap();
        let mut starts = vec![0, 1];
        for entry in &catalog.entries {
            starts.push(entry.manifest.start_slot);
            starts.push(entry.manifest.end_slot.checked_add(1).unwrap());
        }
        for gap in &catalog.legacy_loss_declarations {
            starts.extend([gap.start_slot, gap.end_slot, gap.following_slot().unwrap()]);
        }
        starts.push(u64::MAX);
        starts.sort_unstable();
        starts.dedup();
        let started = std::time::Instant::now();
        let checked: Vec<_> = starts
            .iter()
            .map(|slot| {
                catalog
                    .checkpoint_handoff_root(*slot)
                    .map_err(|e| e.to_string())
            })
            .collect();
        let checked_us = started.elapsed().as_micros();
        let started = std::time::Instant::now();
        let admitted: Vec<_> = starts
            .iter()
            .map(|slot| {
                reader
                    .checkpoint_handoff_root(*slot)
                    .map_err(|e| e.to_string())
            })
            .collect();
        let admitted_us = started.elapsed().as_micros();
        assert_eq!(checked, admitted);
        assert_eq!(reader.status().remote_fetches, 0);
        assert!(reader.decoded.lock().unwrap().is_empty());
        println!(
            "handoff parity: catalog={} segments={} boundaries={} checked_us={} admitted_us={}",
            catalog.catalog_root,
            catalog.entries.len(),
            starts.len(),
            checked_us,
            admitted_us,
        );
    }

    #[test]
    fn transaction_filter_skips_objects_for_definite_misses() {
        let local = tempdir().unwrap();
        let remote = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let (identity, _, bytes, manifest) = fixture(local.path());
        let decoded = ArchiveV2SegmentCodec::decode(&bytes, &manifest, &identity).unwrap();
        let known = decoded.transactions[0].signature();
        let missing = (0u64..)
            .map(|nonce| Hash::hash(&nonce.to_le_bytes()))
            .find(|signature| !manifest.transaction_filter.might_contain(signature))
            .unwrap();
        let reader = ArchiveV2Reader::open(
            identity,
            &local.path().join("catalog.av2"),
            ArchiveV2ReaderConfig {
                role: ArchiveV2Role::VerifiedCache,
                root: local.path().to_path_buf(),
                cache_root: Some(cache.path().to_path_buf()),
                cache_quota_bytes: bytes.len() as u64 + 1024,
                max_decoded_segments: 1,
                allow_remote_fetch: true,
                sources: vec![Arc::new(ArchiveV2DirectorySource::new(
                    "remote",
                    remote.path(),
                    true,
                ))],
            },
        )
        .unwrap();

        assert!(reader.get_transaction(&missing).unwrap().is_none());
        assert!(reader.get_transaction_slot(&missing).unwrap().is_none());
        assert_eq!(reader.status().remote_fetches, 0);
        assert_eq!(reader.status().local_hits, 0);
        assert_eq!(reader.status().cache_hits, 0);

        write_new_synced(
            &object_path(remote.path(), &manifest.segment_object_hash),
            &bytes,
        )
        .unwrap();
        assert_eq!(
            reader.get_transaction(&known).unwrap().unwrap().signature(),
            known
        );
        assert_eq!(reader.status().remote_fetches, 1);
        assert_eq!(reader.status().verified_objects, 1);
        assert!(reader.decoded.lock().unwrap().is_empty());
    }

    #[test]
    fn reader_fetches_verifies_caches_evicts_and_refetches() {
        let local = tempdir().unwrap();
        let remote = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let (identity, _, bytes, manifest) = fixture(local.path());
        write_new_synced(
            &object_path(remote.path(), &manifest.segment_object_hash),
            &bytes,
        )
        .unwrap();
        let config = ArchiveV2ReaderConfig {
            role: ArchiveV2Role::VerifiedCache,
            root: local.path().to_path_buf(),
            cache_root: Some(cache.path().to_path_buf()),
            cache_quota_bytes: bytes.len() as u64 + 1024,
            max_decoded_segments: 1,
            allow_remote_fetch: true,
            sources: vec![Arc::new(ArchiveV2DirectorySource::new(
                "remote",
                remote.path(),
                true,
            ))],
        };
        let reader =
            ArchiveV2Reader::open(identity, &local.path().join("catalog.av2"), config).unwrap();
        assert_eq!(reader.get_block(0).unwrap().unwrap().header.slot, 0);
        assert_eq!(reader.status().remote_fetches, 1);
        assert_eq!(reader.status().verified_objects, 1);
        assert!(reader.decoded.lock().unwrap().is_empty());
        assert!(object_path(cache.path(), &manifest.segment_object_hash).exists());

        fs::remove_file(object_path(cache.path(), &manifest.segment_object_hash)).unwrap();
        assert_eq!(reader.get_block(0).unwrap().unwrap().header.slot, 0);
        assert_eq!(reader.status().remote_fetches, 2);
        assert!(reader.decoded.lock().unwrap().is_empty());
    }

    #[test]
    fn corrupt_remote_object_is_quarantined_and_never_returned() {
        let local = tempdir().unwrap();
        let remote = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let (identity, _, bytes, manifest) = fixture(local.path());
        let mut corrupt = bytes;
        corrupt[32] ^= 1;
        write_new_synced(
            &object_path(remote.path(), &manifest.segment_object_hash),
            &corrupt,
        )
        .unwrap();
        let reader = ArchiveV2Reader::open(
            identity,
            &local.path().join("catalog.av2"),
            ArchiveV2ReaderConfig {
                role: ArchiveV2Role::VerifiedCache,
                root: local.path().to_path_buf(),
                cache_root: Some(cache.path().to_path_buf()),
                cache_quota_bytes: 1024 * 1024,
                max_decoded_segments: 1,
                allow_remote_fetch: true,
                sources: vec![Arc::new(ArchiveV2DirectorySource::new(
                    "remote",
                    remote.path(),
                    true,
                ))],
            },
        )
        .unwrap();
        assert!(matches!(
            reader.get_block(0),
            Err(ArchiveV2Error::Unavailable(_))
        ));
        assert_eq!(reader.status().quarantined_objects, 1);
    }

    #[test]
    fn category_reader_reconstructs_derived_rows_and_filters_bound_raw_rows() {
        let local = tempdir().unwrap();
        let (identity, _, bytes, manifest) = fixture(local.path());
        write_new_synced(
            &object_path(local.path(), &manifest.segment_object_hash),
            &bytes,
        )
        .unwrap();
        let reader = ArchiveV2Reader::open(
            identity,
            &local.path().join("catalog.av2"),
            ArchiveV2ReaderConfig {
                role: ArchiveV2Role::FullArchive,
                root: local.path().to_path_buf(),
                cache_root: None,
                cache_quota_bytes: 0,
                max_decoded_segments: 1,
                allow_remote_fetch: false,
                sources: Vec::new(),
            },
        )
        .unwrap();

        assert_eq!(
            reader.category_value("events", b"event-key").unwrap(),
            Some((0, b"event-value".to_vec()))
        );
        assert!(
            reader.decoded.lock().unwrap().is_empty(),
            "single category lookup must not inflate the whole segment"
        );
        assert_eq!(reader.category_value("events", b"missing").unwrap(), None);
        assert!(reader.decoded.lock().unwrap().is_empty());

        let slots = reader.category_rows("slots", 0, 0).unwrap();
        assert_eq!(slots.len(), 1);
        assert_eq!(slots[0].0, 0u64.to_be_bytes());
        assert_eq!(slots[0].1, manifest.first_block_hash.0);
        let blocks = reader.category_rows("blocks", 0, 0).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].0, manifest.first_block_hash.0);
        assert_eq!(
            reader.category_rows("events", 0, 0).unwrap(),
            vec![(b"event-key".to_vec(), b"event-value".to_vec())]
        );
        assert!(reader.category_rows("events", 1, 1).unwrap().is_empty());
        assert_eq!(
            reader
                .category_rows_with_prefix("events", 0, 0, b"event-")
                .unwrap(),
            reader.category_rows("events", 0, 0).unwrap()
        );
        for prefix in [b"a".as_slice(), b"event-key-extra", b"z"] {
            assert!(reader
                .category_rows_with_prefix("events", 0, 0, prefix)
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn prewarmed_indexes_survive_source_outage_and_reject_corruption() {
        let local = tempdir().unwrap();
        let source = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let (identity, _, bytes, manifest) = fixture(local.path());
        let source_path = object_path(source.path(), &manifest.segment_object_hash);
        write_new_synced(&source_path, &bytes).unwrap();
        let bundle_bytes =
            ArchiveV2SegmentCodec::public_index_cache_size(&manifest).unwrap() as u64;
        let config = |quota| ArchiveV2ReaderConfig {
            role: ArchiveV2Role::VerifiedCache,
            root: local.path().to_path_buf(),
            cache_root: Some(cache.path().to_path_buf()),
            cache_quota_bytes: quota,
            max_decoded_segments: 1,
            allow_remote_fetch: true,
            sources: vec![Arc::new(ArchiveV2DirectorySource::new(
                "source",
                source.path(),
                true,
            ))],
        };
        let too_small = ArchiveV2Reader::open(
            identity.clone(),
            &local.path().join("catalog.av2"),
            config(bundle_bytes - 1),
        )
        .unwrap();
        assert!(matches!(
            too_small.prewarm_public_indexes(source.path()),
            Err(ArchiveV2Error::Bounds(_))
        ));
        assert_eq!(cache_size(cache.path()).unwrap(), 0);
        let reader = ArchiveV2Reader::open(
            identity,
            &local.path().join("catalog.av2"),
            config(bundle_bytes),
        )
        .unwrap();
        assert_eq!(reader.prewarm_public_indexes(source.path()).unwrap(), 1);
        assert_eq!(reader.prewarm_public_indexes(source.path()).unwrap(), 1);
        assert_eq!(cache_size(cache.path()).unwrap(), bundle_bytes);
        fs::remove_file(&source_path).unwrap();
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    assert_eq!(
                        reader
                            .category_rows_with_prefix("events", 0, 0, b"event")
                            .unwrap(),
                        vec![(b"event-key".to_vec(), b"event-value".to_vec())]
                    );
                });
            }
        });
        assert_eq!(reader.status().remote_fetches, 0);
        assert_eq!(reader.status().verified_objects, 0);
        assert!(reader.decoded.lock().unwrap().is_empty());
        assert!(
            reader.get_block(0).is_err(),
            "index availability must not claim body availability"
        );
        let index_path = cache
            .path()
            .join("indexes")
            .join(format!("{}.av2i", manifest.segment_object_hash.to_hex()));
        let mut corrupted = fs::read(&index_path).unwrap();
        *corrupted.last_mut().unwrap() ^= 1;
        fs::write(&index_path, corrupted).unwrap();
        assert!(reader.category_value("events", b"event-key").is_err());
        assert_eq!(reader.status().quarantined_objects, 1);
        assert!(!index_path.exists());
        write_new_synced(&source_path, &bytes).unwrap();
        assert_eq!(reader.prewarm_public_indexes(source.path()).unwrap(), 1);
        assert_eq!(
            reader.category_value("events", b"event-key").unwrap(),
            Some((0, b"event-value".to_vec()))
        );
    }

    #[test]
    fn object_and_index_caches_share_one_quota_with_body_eviction_first() {
        let root = tempdir().unwrap();
        let body = root.path().join("objects").join("body");
        let index = root.path().join("indexes").join("index");
        write_new_synced(&index, &[1; 20]).unwrap();
        write_new_synced(&body, &[2; 80]).unwrap();
        assert_eq!(cache_size(root.path()).unwrap(), 100);
        evict_cache_until(root.path(), 50).unwrap();
        assert!(!body.exists());
        assert!(index.exists());
        assert_eq!(cache_size(root.path()).unwrap(), 20);
        evict_cache_until(root.path(), 0).unwrap();
        assert!(!index.exists());
    }

    #[test]
    fn full_archive_gateway_reads_only_active_verified_local_objects() {
        let local = tempdir().unwrap();
        let (identity, _, bytes, manifest) = fixture(local.path());
        let path = object_path(local.path(), &manifest.segment_object_hash);
        write_new_synced(&path, &bytes).unwrap();
        let reader = ArchiveV2Reader::open(
            identity,
            &local.path().join("catalog.av2"),
            ArchiveV2ReaderConfig {
                role: ArchiveV2Role::FullArchive,
                root: local.path().to_path_buf(),
                cache_root: None,
                cache_quota_bytes: 0,
                max_decoded_segments: 1,
                allow_remote_fetch: false,
                sources: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(
            reader
                .local_verified_object(&manifest.segment_object_hash)
                .unwrap(),
            Some(bytes)
        );
        assert_eq!(
            reader
                .local_verified_object(&Hash::hash(b"not-cataloged"))
                .unwrap(),
            None
        );
        fs::write(path, b"corrupt").unwrap();
        assert!(matches!(
            reader.local_verified_object(&manifest.segment_object_hash),
            Err(ArchiveV2Error::WrongObjectHash)
        ));
    }

    #[test]
    fn consensus_role_keeps_catalog_commitment_but_denies_deep_history() {
        let local = tempdir().unwrap();
        let (identity, _, bytes, manifest) = fixture(local.path());
        write_new_synced(
            &object_path(local.path(), &manifest.segment_object_hash),
            &bytes,
        )
        .unwrap();
        let reader = ArchiveV2Reader::open(
            identity,
            &local.path().join("catalog.av2"),
            ArchiveV2ReaderConfig {
                role: ArchiveV2Role::Consensus,
                root: local.path().to_path_buf(),
                cache_root: None,
                cache_quota_bytes: 0,
                max_decoded_segments: 1,
                allow_remote_fetch: false,
                sources: Vec::new(),
            },
        )
        .unwrap();
        assert_eq!(reader.status().role.as_deref(), Some("consensus"));
        assert!(!reader.status().serves_deep_history);
        assert!(!reader.status().admitted_after_fresh_sync);
        reader.mark_admitted_after_fresh_sync();
        assert!(reader.status().admitted_after_fresh_sync);
        assert!(matches!(reader.get_block(0), Err(ArchiveV2Error::Role(_))));
        assert!(reader.get_block(manifest.end_slot + 1).unwrap().is_none());
    }

    #[test]
    fn concurrent_cache_miss_readers_converge_on_one_verified_object() {
        let local = tempdir().unwrap();
        let remote = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let (identity, _, bytes, manifest) = fixture(local.path());
        write_new_synced(
            &object_path(remote.path(), &manifest.segment_object_hash),
            &bytes,
        )
        .unwrap();
        let reader = Arc::new(
            ArchiveV2Reader::open(
                identity,
                &local.path().join("catalog.av2"),
                ArchiveV2ReaderConfig {
                    role: ArchiveV2Role::VerifiedCache,
                    root: local.path().to_path_buf(),
                    cache_root: Some(cache.path().to_path_buf()),
                    cache_quota_bytes: bytes.len() as u64 + 1024,
                    max_decoded_segments: 1,
                    allow_remote_fetch: true,
                    sources: vec![Arc::new(ArchiveV2DirectorySource::new(
                        "remote",
                        remote.path(),
                        true,
                    ))],
                },
            )
            .unwrap(),
        );
        let threads = (0..16)
            .map(|_| {
                let reader = Arc::clone(&reader);
                std::thread::spawn(move || reader.get_block(0).unwrap().unwrap().hash())
            })
            .collect::<Vec<_>>();
        for thread in threads {
            assert_eq!(thread.join().unwrap(), manifest.first_block_hash);
        }
        assert_eq!(reader.status().verified_objects, 1);
        assert_eq!(reader.status().remote_fetches, 1);
        assert_eq!(
            fs::read(object_path(cache.path(), &manifest.segment_object_hash)).unwrap(),
            bytes
        );
    }
}
