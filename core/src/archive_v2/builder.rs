use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use super::{
    ArchiveV2Catalog, ArchiveV2CodecConfig, ArchiveV2Error, ArchiveV2Identity, ArchiveV2Manifest,
    ArchiveV2SegmentCodec,
};
use crate::codec::{deserialize_legacy_bincode_strict, serialize_legacy_bincode};
use crate::{Hash, StateStore};

const BUILD_JOURNAL_MAGIC: &[u8] = b"LICHEN-AV2-BUILD\0";
const MAX_BUILD_JOURNAL_BYTES: usize = 1024 * 1024;
static TEMPORARY_FILE_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct ArchiveV2BuildOptions {
    pub root: PathBuf,
    pub start_slot: u64,
    pub end_slot: u64,
    /// Required distance behind the finalized tip. Production archival builds
    /// use at least the configured hot-history retention window.
    pub required_finality_depth_slots: u64,
    pub codec: ArchiveV2CodecConfig,
    pub replica_roots: Vec<PathBuf>,
    pub required_replica_count: usize,
}

impl ArchiveV2BuildOptions {
    pub fn validate(&self) -> Result<(), ArchiveV2Error> {
        self.codec.validate()?;
        if self.end_slot < self.start_slot {
            return Err(ArchiveV2Error::Bounds(
                "build range end precedes start".to_string(),
            ));
        }
        if self.required_replica_count == 0
            || self.replica_roots.len() < self.required_replica_count
        {
            return Err(ArchiveV2Error::Bounds(
                "required replica count must be non-zero and not exceed configured destinations"
                    .to_string(),
            ));
        }
        let destinations = self
            .replica_roots
            .iter()
            .map(|path| path.display().to_string())
            .collect::<std::collections::BTreeSet<_>>();
        if destinations.len() != self.replica_roots.len() {
            return Err(ArchiveV2Error::Bounds(
                "replica destinations must be unique".to_string(),
            ));
        }
        if destinations.contains(&self.root.display().to_string()) {
            return Err(ArchiveV2Error::Bounds(
                "primary Archive V2 root cannot also be a replica destination".to_string(),
            ));
        }
        if self.required_replica_count > u32::MAX as usize {
            return Err(ArchiveV2Error::Bounds(
                "required replica count exceeds u32".to_string(),
            ));
        }
        Ok(())
    }

    pub fn catalog_path(&self) -> PathBuf {
        self.root.join("catalog.av2")
    }

    pub fn staging_root(&self) -> PathBuf {
        self.root.join("staging")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveV2FaultPoint {
    AfterStageWrite,
    AfterLocalVerification,
    AfterReplication,
    AfterPromotion,
    AfterCatalogUpdate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ArchiveV2BuildPhase {
    Collecting,
    Staged,
    LocallyVerified,
    Replicated,
    Promoted,
    Cataloged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2BuildJournal {
    pub format_version: u16,
    pub network_id: String,
    pub genesis_hash: Hash,
    pub start_slot: u64,
    pub end_slot: u64,
    pub required_finality_depth_slots: u64,
    pub codec_config_hash: Hash,
    pub replica_destinations: Vec<String>,
    pub required_replica_count: u32,
    phase: ArchiveV2BuildPhase,
    pub segment_object_hash: Option<Hash>,
    pub replica_acknowledgements: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveV2BuildReport {
    pub start_slot: u64,
    pub end_slot: u64,
    pub segment_object_hash: Option<Hash>,
    pub segment_content_root: Option<Hash>,
    pub block_count: u64,
    pub transaction_count: u64,
    pub public_index_rows: u64,
    pub segment_bytes: u64,
    pub replica_acknowledgements: u64,
    pub resumed: bool,
    pub promoted: bool,
    pub catalog_root: Option<Hash>,
}

pub struct ArchiveV2Builder<'a> {
    state: &'a StateStore,
    identity: ArchiveV2Identity,
    options: ArchiveV2BuildOptions,
}

impl<'a> ArchiveV2Builder<'a> {
    pub fn new(
        state: &'a StateStore,
        identity: ArchiveV2Identity,
        options: ArchiveV2BuildOptions,
    ) -> Result<Self, ArchiveV2Error> {
        identity.validate()?;
        options.validate()?;
        let state_network = state
            .get_metadata(crate::CHAIN_ID_METADATA_KEY)
            .map_err(ArchiveV2Error::Io)?
            .ok_or_else(|| {
                ArchiveV2Error::Continuity(
                    "authoritative source has no persisted chain identity".to_string(),
                )
            })?;
        if state_network != identity.network_id.as_bytes() {
            return Err(ArchiveV2Error::WrongNetwork {
                expected: identity.network_id.clone(),
                actual: String::from_utf8_lossy(&state_network).into_owned(),
            });
        }
        let genesis_hash = state
            .get_block_by_slot(0)
            .map_err(ArchiveV2Error::Io)?
            .map(|block| block.hash())
            .ok_or_else(|| {
                ArchiveV2Error::Continuity(
                    "authoritative source has no canonical genesis block".to_string(),
                )
            })?;
        if genesis_hash != identity.genesis_hash {
            return Err(ArchiveV2Error::WrongGenesis);
        }
        Ok(Self {
            state,
            identity,
            options,
        })
    }

    pub fn build(&self) -> Result<ArchiveV2BuildReport, ArchiveV2Error> {
        self.build_with_fault(None)
    }

    #[cfg(test)]
    pub(crate) fn build_faulted(
        &self,
        fault: ArchiveV2FaultPoint,
    ) -> Result<ArchiveV2BuildReport, ArchiveV2Error> {
        self.build_with_fault(Some(fault))
    }

    fn build_with_fault(
        &self,
        fault: Option<ArchiveV2FaultPoint>,
    ) -> Result<ArchiveV2BuildReport, ArchiveV2Error> {
        let _maintenance = self.state.lock_archive_maintenance();
        fs::create_dir_all(self.options.staging_root())?;
        fs::create_dir_all(self.options.root.join("objects"))?;
        fs::create_dir_all(self.options.root.join("manifests"))?;
        let journal_path = self.journal_path();
        let mut journal = if journal_path.exists() {
            let journal = load_journal(&journal_path)?;
            self.validate_journal(&journal)?;
            journal
        } else {
            let journal = ArchiveV2BuildJournal {
                format_version: super::ARCHIVE_V2_FORMAT_VERSION,
                network_id: self.identity.network_id.clone(),
                genesis_hash: self.identity.genesis_hash,
                start_slot: self.options.start_slot,
                end_slot: self.options.end_slot,
                required_finality_depth_slots: self.options.required_finality_depth_slots,
                codec_config_hash: self.codec_config_hash()?,
                replica_destinations: self.replica_destinations(),
                required_replica_count: self.options.required_replica_count as u32,
                phase: ArchiveV2BuildPhase::Collecting,
                segment_object_hash: None,
                replica_acknowledgements: Vec::new(),
            };
            store_journal(&journal_path, &journal)?;
            journal
        };
        let resumed = journal.phase != ArchiveV2BuildPhase::Collecting;
        let mut catalog = if self.options.catalog_path().exists() {
            ArchiveV2Catalog::load(&self.options.catalog_path())?
        } else {
            ArchiveV2Catalog::empty(self.identity.clone())?
        };
        catalog.validate()?;
        if journal.phase == ArchiveV2BuildPhase::Cataloged {
            let manifest = catalog
                .entries
                .iter()
                .filter_map(|entry| {
                    catalog
                        .active_manifest(&entry.manifest.segment_object_hash)
                        .ok()
                })
                .find(|manifest| {
                    Some(manifest.segment_object_hash) == journal.segment_object_hash
                        && manifest.start_slot == self.options.start_slot
                        && manifest.end_slot == self.options.end_slot
                })
                .ok_or_else(|| {
                    ArchiveV2Error::Continuity(
                        "cataloged build journal has no matching active catalog entry".to_string(),
                    )
                })?;
            let object = object_path(&self.options.root, &manifest.segment_object_hash);
            let object_bytes = fs::read(&object)?;
            let promoted_manifest = ArchiveV2Manifest::decode_canonical(&fs::read(
                manifest_path(&self.options.root, &manifest.segment_object_hash),
            )?)?;
            if promoted_manifest != *manifest {
                return Err(ArchiveV2Error::WrongRoot);
            }
            ArchiveV2SegmentCodec::decode(&object_bytes, manifest, &self.identity)?;
            self.replicate_catalog_and_verify(&catalog, &journal.replica_acknowledgements)?;
            return Ok(ArchiveV2BuildReport {
                start_slot: manifest.start_slot,
                end_slot: manifest.end_slot,
                segment_object_hash: Some(manifest.segment_object_hash),
                segment_content_root: Some(manifest.segment_content_root),
                block_count: manifest.block_count,
                transaction_count: manifest.transaction_count,
                public_index_rows: manifest.public_index_rows,
                segment_bytes: object_bytes.len() as u64,
                replica_acknowledgements: journal.replica_acknowledgements.len() as u64,
                resumed: true,
                promoted: true,
                catalog_root: Some(catalog.catalog_root),
            });
        }

        let (segment_bytes, manifest) = if journal.phase == ArchiveV2BuildPhase::Collecting {
            let finalized = self
                .state
                .get_last_finalized_slot()
                .map_err(ArchiveV2Error::Io)?;
            let eligible_end = finalized
                .checked_sub(self.options.required_finality_depth_slots)
                .ok_or_else(|| {
                    ArchiveV2Error::Continuity(format!(
                        "finalized slot {finalized} is below required Archive V2 depth {}",
                        self.options.required_finality_depth_slots
                    ))
                })?;
            if self.options.end_slot > eligible_end {
                return Err(ArchiveV2Error::Continuity(format!(
                    "build end {} is newer than eligible finalized boundary {eligible_end}",
                    self.options.end_slot,
                )));
            }
            self.verify_archive_watermark(self.options.end_slot)?;
            let (previous_segment_hash, previous_block_hash) = if let Some(previous) =
                catalog.entries.last()
            {
                let previous = catalog.active_manifest(&previous.manifest.segment_object_hash)?;
                if self.options.start_slot != previous.end_slot.saturating_add(1) {
                    return Err(ArchiveV2Error::Continuity(format!(
                        "build starts at {}, catalog expects {}",
                        self.options.start_slot,
                        previous.end_slot.saturating_add(1)
                    )));
                }
                (Some(previous.segment_object_hash), previous.last_block_hash)
            } else {
                if self.options.start_slot != 0 {
                    return Err(ArchiveV2Error::Continuity(
                        "a new catalog must begin at genesis".to_string(),
                    ));
                }
                (None, Hash::default())
            };

            let contents = self
                .state
                .export_archive_v2_segment_contents(self.options.start_slot, self.options.end_slot)
                .map_err(ArchiveV2Error::Io)?;
            let first_source_hash = contents
                .blocks
                .first()
                .map(crate::Block::hash)
                .ok_or_else(|| ArchiveV2Error::Unavailable("build range is empty".to_string()))?;
            let last_source_hash = contents
                .blocks
                .last()
                .map(crate::Block::hash)
                .ok_or_else(|| ArchiveV2Error::Unavailable("build range is empty".to_string()))?;
            let finalized_after_collection = self
                .state
                .get_last_finalized_slot()
                .map_err(ArchiveV2Error::Io)?;
            let first_after_collection = self
                .state
                .get_block_by_slot(self.options.start_slot)
                .map_err(ArchiveV2Error::Io)?
                .map(|block| block.hash());
            let last_after_collection = self
                .state
                .get_block_by_slot(self.options.end_slot)
                .map_err(ArchiveV2Error::Io)?
                .map(|block| block.hash());
            if finalized_after_collection
                .checked_sub(self.options.required_finality_depth_slots)
                .is_none_or(|eligible_end| eligible_end < self.options.end_slot)
                || first_after_collection != Some(first_source_hash)
                || last_after_collection != Some(last_source_hash)
            {
                return Err(ArchiveV2Error::Continuity(
                    "authoritative finalized source changed during segment collection".to_string(),
                ));
            }
            self.verify_archive_watermark(self.options.end_slot)?;
            let (segment_bytes, manifest) = ArchiveV2SegmentCodec::encode(
                self.identity.clone(),
                previous_segment_hash,
                previous_block_hash,
                &contents,
                &self.options.codec,
            )?;
            let (independent_bytes, independent_manifest) = ArchiveV2SegmentCodec::encode(
                self.identity.clone(),
                previous_segment_hash,
                previous_block_hash,
                &contents,
                &self.options.codec,
            )?;
            if segment_bytes != independent_bytes || manifest != independent_manifest {
                return Err(ArchiveV2Error::WrongRoot);
            }
            write_atomic_identical(
                &self.staged_segment_path(&manifest.segment_object_hash),
                &segment_bytes,
            )?;
            write_atomic_identical(
                &self.staged_manifest_path(&manifest.segment_object_hash),
                &manifest.encode_canonical()?,
            )?;
            journal.phase = ArchiveV2BuildPhase::Staged;
            journal.segment_object_hash = Some(manifest.segment_object_hash);
            store_journal(&journal_path, &journal)?;
            maybe_fault(fault, ArchiveV2FaultPoint::AfterStageWrite)?;
            (segment_bytes, manifest)
        } else {
            let object_hash = journal.segment_object_hash.ok_or_else(|| {
                ArchiveV2Error::Continuity(
                    "resumed build journal has no staged object hash".to_string(),
                )
            })?;
            let segment_bytes = fs::read(self.staged_segment_path(&object_hash))?;
            let manifest = ArchiveV2Manifest::decode_canonical(&fs::read(
                self.staged_manifest_path(&object_hash),
            )?)?;
            if manifest.segment_object_hash != object_hash
                || manifest.identity != self.identity
                || manifest.start_slot != self.options.start_slot
                || manifest.end_slot != self.options.end_slot
            {
                return Err(ArchiveV2Error::Continuity(
                    "staged object does not match its resumed build journal".to_string(),
                ));
            }
            (segment_bytes, manifest)
        };

        // Every resume re-verifies the staged bytes. It never trusts a journal
        // phase as proof that the immutable staging object is still intact.
        ArchiveV2SegmentCodec::decode(&segment_bytes, &manifest, &self.identity)?;
        if journal.phase == ArchiveV2BuildPhase::Staged {
            journal.phase = ArchiveV2BuildPhase::LocallyVerified;
            store_journal(&journal_path, &journal)?;
            maybe_fault(fault, ArchiveV2FaultPoint::AfterLocalVerification)?;
        }

        if journal.phase == ArchiveV2BuildPhase::LocallyVerified {
            journal.replica_acknowledgements =
                self.replicate_and_verify(&manifest, &segment_bytes)?;
            journal.phase = ArchiveV2BuildPhase::Replicated;
            store_journal(&journal_path, &journal)?;
            maybe_fault(fault, ArchiveV2FaultPoint::AfterReplication)?;
        } else if journal.phase >= ArchiveV2BuildPhase::Replicated {
            let verified = self.replicate_and_verify(&manifest, &segment_bytes)?;
            if verified != journal.replica_acknowledgements {
                return Err(ArchiveV2Error::Continuity(
                    "replica acknowledgement set changed after durable replication".to_string(),
                ));
            }
        }

        if journal.phase == ArchiveV2BuildPhase::Replicated {
            let promoted = object_path(&self.options.root, &manifest.segment_object_hash);
            let promoted_manifest =
                manifest_path(&self.options.root, &manifest.segment_object_hash);
            write_atomic_identical(&promoted, &segment_bytes)?;
            write_atomic_identical(&promoted_manifest, &manifest.encode_canonical()?)?;
            let promoted_bytes = fs::read(&promoted)?;
            let decoded_manifest =
                ArchiveV2Manifest::decode_canonical(&fs::read(&promoted_manifest)?)?;
            if decoded_manifest != manifest {
                return Err(ArchiveV2Error::WrongRoot);
            }
            ArchiveV2SegmentCodec::decode(&promoted_bytes, &decoded_manifest, &self.identity)?;
            journal.phase = ArchiveV2BuildPhase::Promoted;
            store_journal(&journal_path, &journal)?;
            maybe_fault(fault, ArchiveV2FaultPoint::AfterPromotion)?;
        } else if journal.phase >= ArchiveV2BuildPhase::Promoted {
            let promoted = fs::read(object_path(
                &self.options.root,
                &manifest.segment_object_hash,
            ))?;
            let promoted_manifest = ArchiveV2Manifest::decode_canonical(&fs::read(
                manifest_path(&self.options.root, &manifest.segment_object_hash),
            )?)?;
            if promoted_manifest != manifest {
                return Err(ArchiveV2Error::WrongRoot);
            }
            ArchiveV2SegmentCodec::decode(&promoted, &promoted_manifest, &self.identity)?;
        }

        if journal.phase == ArchiveV2BuildPhase::Promoted {
            if catalog
                .manifest_by_object_hash(&manifest.segment_object_hash)
                .is_none()
            {
                catalog.append(manifest.clone())?;
                catalog.store_atomic(&self.options.catalog_path())?;
            } else {
                let cataloged = catalog
                    .manifest_by_object_hash(&manifest.segment_object_hash)
                    .ok_or(ArchiveV2Error::WrongRoot)?;
                if cataloged != &manifest {
                    return Err(ArchiveV2Error::Continuity(
                        "catalog object hash refers to a different manifest".to_string(),
                    ));
                }
            }
            // Keep the journal at Promoted until every object replica has the
            // exact append-only catalog extension. A crash after the local
            // update therefore resumes here and completes remote publication
            // instead of treating an undiscoverable replica as durable.
            maybe_fault(fault, ArchiveV2FaultPoint::AfterCatalogUpdate)?;
            self.replicate_catalog_and_verify(&catalog, &journal.replica_acknowledgements)?;
            journal.phase = ArchiveV2BuildPhase::Cataloged;
            store_journal(&journal_path, &journal)?;
        }

        Ok(ArchiveV2BuildReport {
            start_slot: manifest.start_slot,
            end_slot: manifest.end_slot,
            segment_object_hash: Some(manifest.segment_object_hash),
            segment_content_root: Some(manifest.segment_content_root),
            block_count: manifest.block_count,
            transaction_count: manifest.transaction_count,
            public_index_rows: manifest.public_index_rows,
            segment_bytes: segment_bytes.len() as u64,
            replica_acknowledgements: journal.replica_acknowledgements.len() as u64,
            resumed,
            promoted: true,
            catalog_root: Some(catalog.catalog_root),
        })
    }

    fn codec_config_hash(&self) -> Result<Hash, ArchiveV2Error> {
        let encoded = serialize_legacy_bincode(&self.options.codec, "Archive V2 codec config")
            .map_err(ArchiveV2Error::Codec)?;
        Ok(Hash::hash(&encoded))
    }

    fn verify_archive_watermark(&self, required_end: u64) -> Result<(), ArchiveV2Error> {
        let (watermark_slot, watermark_hash) = self
            .state
            .get_archive_contiguous_tip()
            .map_err(ArchiveV2Error::Io)?
            .ok_or_else(|| {
                ArchiveV2Error::Continuity(
                    "authoritative source has no genesis-to-tip archive watermark".to_string(),
                )
            })?;
        if watermark_slot < required_end {
            return Err(ArchiveV2Error::Continuity(format!(
                "archive watermark {watermark_slot} does not cover build end {required_end}"
            )));
        }
        let canonical_hash = self
            .state
            .get_block_by_slot(watermark_slot)
            .map_err(ArchiveV2Error::Io)?
            .map(|block| block.hash());
        if canonical_hash != Some(watermark_hash) {
            return Err(ArchiveV2Error::Continuity(
                "archive watermark hash does not match its canonical block".to_string(),
            ));
        }
        Ok(())
    }

    fn replica_destinations(&self) -> Vec<String> {
        self.options
            .replica_roots
            .iter()
            .map(|path| path.display().to_string())
            .collect()
    }

    fn replicate_and_verify(
        &self,
        manifest: &ArchiveV2Manifest,
        segment_bytes: &[u8],
    ) -> Result<Vec<String>, ArchiveV2Error> {
        let mut acknowledgements = Vec::new();
        for replica in &self.options.replica_roots {
            let replica_path = object_path(replica, &manifest.segment_object_hash);
            let replica_manifest_path = manifest_path(replica, &manifest.segment_object_hash);
            write_atomic_identical(&replica_path, segment_bytes)?;
            write_atomic_identical(&replica_manifest_path, &manifest.encode_canonical()?)?;
            let replica_bytes = fs::read(&replica_path)?;
            let replica_manifest =
                ArchiveV2Manifest::decode_canonical(&fs::read(&replica_manifest_path)?)?;
            if replica_manifest != *manifest {
                return Err(ArchiveV2Error::WrongRoot);
            }
            ArchiveV2SegmentCodec::decode(&replica_bytes, &replica_manifest, &self.identity)?;
            acknowledgements.push(replica.display().to_string());
        }
        if acknowledgements.len() < self.options.required_replica_count {
            return Err(ArchiveV2Error::Unavailable(format!(
                "only {} replicas acknowledged, {} required",
                acknowledgements.len(),
                self.options.required_replica_count
            )));
        }
        Ok(acknowledgements)
    }

    fn replicate_catalog_and_verify(
        &self,
        catalog: &ArchiveV2Catalog,
        object_acknowledgements: &[String],
    ) -> Result<(), ArchiveV2Error> {
        let encoded = catalog.encode_canonical()?;
        let mut acknowledgements = Vec::new();
        for replica in &self.options.replica_roots {
            let destination = replica.display().to_string();
            if !object_acknowledgements.contains(&destination) {
                continue;
            }
            let replica_catalog = ArchiveV2Catalog::import_extension_atomic(
                &replica.join("catalog.av2"),
                &encoded,
                &self.identity,
                Some(catalog.catalog_root),
            )?;
            if replica_catalog != *catalog {
                return Err(ArchiveV2Error::WrongRoot);
            }
            acknowledgements.push(destination);
        }
        if acknowledgements.len() < self.options.required_replica_count {
            return Err(ArchiveV2Error::Unavailable(format!(
                "only {} replicas acknowledged catalog {}, {} required",
                acknowledgements.len(),
                catalog.catalog_root,
                self.options.required_replica_count
            )));
        }
        Ok(())
    }

    fn journal_path(&self) -> PathBuf {
        self.options.staging_root().join(format!(
            "build-{}-{}.journal",
            self.options.start_slot, self.options.end_slot
        ))
    }

    fn staged_segment_path(&self, hash: &Hash) -> PathBuf {
        self.options
            .staging_root()
            .join(format!("{}.av2s", hash.to_hex()))
    }

    fn staged_manifest_path(&self, hash: &Hash) -> PathBuf {
        self.options
            .staging_root()
            .join(format!("{}.manifest", hash.to_hex()))
    }

    fn validate_journal(&self, journal: &ArchiveV2BuildJournal) -> Result<(), ArchiveV2Error> {
        if journal.format_version != super::ARCHIVE_V2_FORMAT_VERSION
            || journal.network_id != self.identity.network_id
            || journal.genesis_hash != self.identity.genesis_hash
            || journal.start_slot != self.options.start_slot
            || journal.end_slot != self.options.end_slot
            || journal.required_finality_depth_slots != self.options.required_finality_depth_slots
            || journal.codec_config_hash != self.codec_config_hash()?
            || journal.replica_destinations != self.replica_destinations()
            || journal.required_replica_count != self.options.required_replica_count as u32
            || (journal.phase == ArchiveV2BuildPhase::Collecting
                && (journal.segment_object_hash.is_some()
                    || !journal.replica_acknowledgements.is_empty()))
            || (journal.phase > ArchiveV2BuildPhase::Collecting
                && journal.segment_object_hash.is_none())
            || (journal.phase < ArchiveV2BuildPhase::Replicated
                && !journal.replica_acknowledgements.is_empty())
            || (journal.phase >= ArchiveV2BuildPhase::Replicated
                && journal.replica_acknowledgements != self.replica_destinations())
        {
            return Err(ArchiveV2Error::WrongRoot);
        }
        Ok(())
    }
}

fn maybe_fault(
    requested: Option<ArchiveV2FaultPoint>,
    point: ArchiveV2FaultPoint,
) -> Result<(), ArchiveV2Error> {
    if requested == Some(point) {
        Err(ArchiveV2Error::Io(format!(
            "injected archive v2 build fault at {point:?}"
        )))
    } else {
        Ok(())
    }
}

fn object_path(root: &Path, hash: &Hash) -> PathBuf {
    root.join("objects").join(format!("{}.av2s", hash.to_hex()))
}

fn manifest_path(root: &Path, hash: &Hash) -> PathBuf {
    root.join("manifests")
        .join(format!("{}.av2m", hash.to_hex()))
}

fn write_atomic_identical(path: &Path, bytes: &[u8]) -> Result<(), ArchiveV2Error> {
    let parent = path
        .parent()
        .ok_or_else(|| ArchiveV2Error::Io("archive path has no parent".to_string()))?;
    fs::create_dir_all(parent)?;
    if path.exists() {
        if fs::read(path)? == bytes {
            return Ok(());
        }
        return Err(ArchiveV2Error::Ordering(format!(
            "immutable archive path {} contains conflicting bytes",
            path.display()
        )));
    }
    let temporary = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| ArchiveV2Error::Io("archive filename is invalid".to_string()))?,
        std::process::id(),
        TEMPORARY_FILE_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        OpenOptions::new()
            .read(true)
            .open(parent)?
            .sync_all()
            .map_err(ArchiveV2Error::from)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn encode_journal(journal: &ArchiveV2BuildJournal) -> Result<Vec<u8>, ArchiveV2Error> {
    let payload = serialize_legacy_bincode(journal, "archive v2 build journal")
        .map_err(ArchiveV2Error::Codec)?;
    if payload.len() > MAX_BUILD_JOURNAL_BYTES {
        return Err(ArchiveV2Error::Bounds(
            "build journal is too large".to_string(),
        ));
    }
    let mut encoded = Vec::with_capacity(BUILD_JOURNAL_MAGIC.len() + 4 + payload.len() + 32);
    encoded.extend_from_slice(BUILD_JOURNAL_MAGIC);
    encoded.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    encoded.extend_from_slice(&payload);
    encoded.extend_from_slice(&Hash::hash(&payload).0);
    Ok(encoded)
}

fn load_journal(path: &Path) -> Result<ArchiveV2BuildJournal, ArchiveV2Error> {
    let encoded = fs::read(path)?;
    let minimum = BUILD_JOURNAL_MAGIC.len() + 4 + 32;
    if encoded.len() < minimum || !encoded.starts_with(BUILD_JOURNAL_MAGIC) {
        return Err(ArchiveV2Error::Truncated("build journal"));
    }
    let offset = BUILD_JOURNAL_MAGIC.len();
    let payload_len = u32::from_le_bytes(
        encoded[offset..offset + 4]
            .try_into()
            .map_err(|_| ArchiveV2Error::Truncated("build journal length"))?,
    ) as usize;
    if payload_len > MAX_BUILD_JOURNAL_BYTES {
        return Err(ArchiveV2Error::Bounds(
            "build journal is too large".to_string(),
        ));
    }
    let start = offset + 4;
    let end = start
        .checked_add(payload_len)
        .ok_or_else(|| ArchiveV2Error::Bounds("journal length overflow".to_string()))?;
    if end.checked_add(32) != Some(encoded.len())
        || Hash::hash(&encoded[start..end]).0 != encoded[end..]
    {
        return Err(ArchiveV2Error::WrongRoot);
    }
    deserialize_legacy_bincode_strict(
        &encoded[start..end],
        MAX_BUILD_JOURNAL_BYTES as u64,
        "archive v2 build journal",
    )
    .map_err(ArchiveV2Error::Codec)
}

fn store_journal(path: &Path, journal: &ArchiveV2BuildJournal) -> Result<(), ArchiveV2Error> {
    let encoded = encode_journal(journal)?;
    let parent = path
        .parent()
        .ok_or_else(|| ArchiveV2Error::Io("journal path has no parent".to_string()))?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(
        ".journal.{}.{}.tmp",
        std::process::id(),
        TEMPORARY_FILE_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(&encoded)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        OpenOptions::new()
            .read(true)
            .open(parent)?
            .sync_all()
            .map_err(ArchiveV2Error::from)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::Block;

    #[test]
    fn builder_resumes_every_promotion_boundary_with_identical_output() {
        for fault in [
            ArchiveV2FaultPoint::AfterStageWrite,
            ArchiveV2FaultPoint::AfterLocalVerification,
            ArchiveV2FaultPoint::AfterReplication,
            ArchiveV2FaultPoint::AfterPromotion,
            ArchiveV2FaultPoint::AfterCatalogUpdate,
        ] {
            let root = tempdir().unwrap();
            let replica = tempdir().unwrap();
            let state_root = tempdir().unwrap();
            let state = StateStore::open(state_root.path()).unwrap();
            let block = Block::new_with_timestamp(
                0,
                Hash::default(),
                Hash::hash(b"builder-state"),
                [4; 32],
                Vec::new(),
                1,
            );
            state.put_block_atomic(&block, Some(0), Some(0)).unwrap();
            state
                .put_metadata(crate::CHAIN_ID_METADATA_KEY, b"builder-testnet")
                .unwrap();
            let identity = ArchiveV2Identity {
                network_id: "builder-testnet".to_string(),
                genesis_hash: block.hash(),
            };
            let options = ArchiveV2BuildOptions {
                root: root.path().to_path_buf(),
                start_slot: 0,
                end_slot: 0,
                required_finality_depth_slots: 0,
                codec: ArchiveV2CodecConfig {
                    target_frame_bytes: 1024 * 1024,
                    ..ArchiveV2CodecConfig::default()
                },
                replica_roots: vec![replica.path().to_path_buf()],
                required_replica_count: 1,
            };
            let builder = ArchiveV2Builder::new(&state, identity, options).unwrap();
            assert!(builder.build_faulted(fault).is_err());
            let report = builder.build().unwrap();
            assert!(report.promoted);
            assert!(report.resumed);
            assert_eq!(report.replica_acknowledgements, 1);
            assert!(builder.options.catalog_path().exists());
            let primary_catalog = ArchiveV2Catalog::load(&builder.options.catalog_path()).unwrap();
            let replica_catalog =
                ArchiveV2Catalog::load(&replica.path().join("catalog.av2")).unwrap();
            assert_eq!(replica_catalog, primary_catalog);
        }
    }

    #[test]
    fn staged_resume_does_not_rescan_or_reselect_the_source_range() {
        let root = tempdir().unwrap();
        let replica = tempdir().unwrap();
        let state_root = tempdir().unwrap();
        let state = StateStore::open(state_root.path()).unwrap();
        let first = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"resume-state-0"),
            [4; 32],
            Vec::new(),
            1,
        );
        let second = Block::new_with_timestamp(
            1,
            first.hash(),
            Hash::hash(b"resume-state-1"),
            [4; 32],
            Vec::new(),
            2,
        );
        state.put_block_atomic(&first, Some(0), Some(0)).unwrap();
        state.put_block_atomic(&second, Some(1), Some(1)).unwrap();
        state
            .put_metadata(crate::CHAIN_ID_METADATA_KEY, b"builder-resume-testnet")
            .unwrap();
        let identity = ArchiveV2Identity {
            network_id: "builder-resume-testnet".to_string(),
            genesis_hash: first.hash(),
        };
        let builder = ArchiveV2Builder::new(
            &state,
            identity,
            ArchiveV2BuildOptions {
                root: root.path().to_path_buf(),
                start_slot: 0,
                end_slot: 1,
                required_finality_depth_slots: 0,
                codec: ArchiveV2CodecConfig {
                    target_frame_bytes: 1024 * 1024,
                    ..ArchiveV2CodecConfig::default()
                },
                replica_roots: vec![replica.path().to_path_buf()],
                required_replica_count: 1,
            },
        )
        .unwrap();
        assert!(builder
            .build_faulted(ArchiveV2FaultPoint::AfterStageWrite)
            .is_err());
        // A collecting pass would now reject end slot 1. A staged resume must
        // trust only the re-verified immutable staging object and continue.
        state.set_last_finalized_slot(0).unwrap();
        assert!(builder.build().unwrap().promoted);
    }
}
