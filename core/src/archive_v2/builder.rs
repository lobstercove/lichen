use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use super::{
    ArchiveV2Catalog, ArchiveV2CodecConfig, ArchiveV2Error, ArchiveV2Identity,
    ArchiveV2LegacyLossDeclaration, ArchiveV2Manifest, ArchiveV2SegmentCodec,
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
    /// The existing lichen-testnet-1 database predates the atomic archive
    /// watermark. This acknowledgement permits bounded catalog construction
    /// only for that exact network/genesis while every source range remains
    /// subject to the builder's canonical block and parent-link verification.
    pub acknowledge_exact_testnet_missing_watermark: bool,
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
        let genesis_hash = match state
            .get_block_by_slot(0)
            .map_err(ArchiveV2Error::Io)?
        {
            Some(block) => block.hash(),
            None => verified_catalog_block_at(&options.root, &identity, 0)?
                .map(|block| block.hash())
                .ok_or_else(|| {
                    ArchiveV2Error::Continuity(
                        "authoritative source and Archive V2 catalog have no canonical genesis block"
                            .to_string(),
                    )
                })?,
        };
        if genesis_hash != identity.genesis_hash {
            return Err(ArchiveV2Error::WrongGenesis);
        }
        if options.acknowledge_exact_testnet_missing_watermark {
            ArchiveV2LegacyLossDeclaration::lichen_testnet_1().validate_for_identity(&identity)?;
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
            self.verify_archive_watermark(self.options.end_slot, &catalog)?;
            let (previous_segment_hash, previous_block_hash) = if let Some(previous) =
                catalog.entries.last()
            {
                let previous = catalog.active_manifest(&previous.manifest.segment_object_hash)?;
                let previous_block_hash =
                    if self.options.start_slot == previous.end_slot.saturating_add(1) {
                        previous.last_block_hash
                    } else if let Some(declaration) = catalog.trailing_loss_declaration()? {
                        if declaration.following_slot()? != self.options.start_slot {
                            return Err(ArchiveV2Error::Continuity(format!(
                            "build starts at {}, catalog expects {} or declared-loss successor {}",
                            self.options.start_slot,
                            previous.end_slot.saturating_add(1),
                            declaration.following_slot()?
                        )));
                        }
                        declaration.missing_tip_block_hash
                    } else {
                        return Err(ArchiveV2Error::Continuity(format!(
                            "build starts at {}, catalog expects {}",
                            self.options.start_slot,
                            previous.end_slot.saturating_add(1)
                        )));
                    };
                (Some(previous.segment_object_hash), previous_block_hash)
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
            self.verify_archive_watermark(self.options.end_slot, &catalog)?;
            let (segment_bytes, manifest) = ArchiveV2SegmentCodec::encode(
                self.identity.clone(),
                previous_segment_hash,
                previous_block_hash,
                &contents,
                &self.options.codec,
            )?;
            let segment_object_hash = Hash::hash(&segment_bytes);
            if segment_object_hash != manifest.segment_object_hash {
                return Err(ArchiveV2Error::WrongRoot);
            }
            let staged_segment = self.staged_segment_path(&manifest.segment_object_hash);
            let staged_manifest = self.staged_manifest_path(&manifest.segment_object_hash);
            write_atomic_identical(&staged_segment, &segment_bytes)?;
            write_atomic_identical(&staged_manifest, &manifest.encode_canonical()?)?;
            // The deterministic rebuild is intentionally independent, but the
            // two complete encoded objects must never be resident together.
            // Persist the first immutable encoding, release it, then compare
            // the second encoding by hash before reopening the staged object.
            drop(segment_bytes);
            let (independent_bytes, independent_manifest) = ArchiveV2SegmentCodec::encode(
                self.identity.clone(),
                previous_segment_hash,
                previous_block_hash,
                &contents,
                &self.options.codec,
            )?;
            if Hash::hash(&independent_bytes) != segment_object_hash
                || manifest != independent_manifest
            {
                return Err(ArchiveV2Error::WrongRoot);
            }
            drop(independent_bytes);
            drop(contents);
            let segment_bytes = fs::read(&staged_segment)?;
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

    fn verify_archive_watermark(
        &self,
        required_end: u64,
        catalog: &ArchiveV2Catalog,
    ) -> Result<(), ArchiveV2Error> {
        let watermark = self
            .state
            .get_archive_contiguous_tip()
            .map_err(ArchiveV2Error::Io)?;
        let Some((watermark_slot, watermark_hash)) = watermark else {
            if !self.options.acknowledge_exact_testnet_missing_watermark {
                return Err(ArchiveV2Error::Continuity(
                    "authoritative source has no genesis-to-tip archive watermark; the exact existing testnet requires an explicit signed-release acknowledgement"
                        .to_string(),
                ));
            }
            ArchiveV2LegacyLossDeclaration::lichen_testnet_1()
                .validate_for_identity(&self.identity)?;
            return Ok(());
        };
        let canonical_hash = self
            .canonical_or_catalog_block(catalog, watermark_slot)?
            .map(|block| block.hash());
        if canonical_hash != Some(watermark_hash) {
            return Err(ArchiveV2Error::Continuity(
                "archive watermark hash does not match its canonical block".to_string(),
            ));
        }
        if watermark_slot >= required_end {
            return Ok(());
        }

        // The existing lichen-testnet-1 history cannot advance its legacy
        // genesis-contiguous watermark across the one approved missing-body
        // interval. A root-committed exact waiver may bridge only that known
        // boundary; every requested source block remains mandatory.
        let declaration = catalog.legacy_loss_declarations.first().ok_or_else(|| {
            ArchiveV2Error::Continuity(format!(
                "archive watermark {watermark_slot} does not cover build end {required_end}"
            ))
        })?;
        let following_slot = declaration.following_slot()?;
        if self.options.start_slot < following_slot {
            return Err(ArchiveV2Error::Continuity(format!(
                "archive watermark {watermark_slot} does not cover pre-waiver build end {required_end}"
            )));
        }
        let following = self
            .canonical_or_catalog_block(catalog, following_slot)?
            .ok_or_else(|| {
                ArchiveV2Error::Unavailable(format!(
                    "declared-loss successor block {following_slot} is unavailable"
                ))
            })?;
        if following.hash() != declaration.following_block_hash
            || following.header.parent_hash != declaration.missing_tip_block_hash
        {
            return Err(ArchiveV2Error::Continuity(
                "declared-loss successor boundary conflicts with canonical history".to_string(),
            ));
        }
        if self.options.start_slot > following_slot {
            let previous = catalog.entries.last().ok_or_else(|| {
                ArchiveV2Error::Continuity(
                    "post-waiver build has no cataloged predecessor".to_string(),
                )
            })?;
            let previous = catalog.active_manifest(&previous.manifest.segment_object_hash)?;
            if previous.start_slot < following_slot
                || previous.end_slot.checked_add(1) != Some(self.options.start_slot)
                || self
                    .canonical_or_catalog_block(catalog, previous.end_slot)?
                    .map(|block| block.hash())
                    != Some(previous.last_block_hash)
            {
                return Err(ArchiveV2Error::Continuity(
                    "post-waiver catalog predecessor conflicts with canonical history".to_string(),
                ));
            }
        }
        if self
            .state
            .get_block_by_slot(required_end)
            .map_err(ArchiveV2Error::Io)?
            .is_none()
        {
            return Err(ArchiveV2Error::Unavailable(format!(
                "canonical build-end block {required_end} is unavailable"
            )));
        }
        Ok(())
    }

    fn canonical_or_catalog_block(
        &self,
        catalog: &ArchiveV2Catalog,
        slot: u64,
    ) -> Result<Option<crate::Block>, ArchiveV2Error> {
        match self
            .state
            .get_block_by_slot(slot)
            .map_err(ArchiveV2Error::Io)?
        {
            Some(block) => Ok(Some(block)),
            None => {
                verified_catalog_block_at_loaded(&self.options.root, &self.identity, catalog, slot)
            }
        }
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

fn verified_catalog_block_at(
    root: &Path,
    identity: &ArchiveV2Identity,
    slot: u64,
) -> Result<Option<crate::Block>, ArchiveV2Error> {
    let catalog_path = root.join("catalog.av2");
    if !catalog_path.is_file() {
        return Ok(None);
    }
    let catalog = ArchiveV2Catalog::load(&catalog_path)?;
    verified_catalog_block_at_loaded(root, identity, &catalog, slot)
}

fn verified_catalog_block_at_loaded(
    root: &Path,
    identity: &ArchiveV2Identity,
    catalog: &ArchiveV2Catalog,
    slot: u64,
) -> Result<Option<crate::Block>, ArchiveV2Error> {
    if &catalog.identity != identity {
        return Err(if catalog.identity.network_id != identity.network_id {
            ArchiveV2Error::WrongNetwork {
                expected: identity.network_id.clone(),
                actual: catalog.identity.network_id.clone(),
            }
        } else {
            ArchiveV2Error::WrongGenesis
        });
    }
    let Some(entry) = catalog
        .entries
        .iter()
        .find(|entry| entry.manifest.start_slot <= slot && slot <= entry.manifest.end_slot)
    else {
        return Ok(None);
    };
    let manifest = catalog.active_manifest(&entry.manifest.segment_object_hash)?;
    let object = fs::read(object_path(root, &manifest.segment_object_hash)).map_err(|error| {
        ArchiveV2Error::Io(format!(
            "failed reading catalog fallback object {}: {error}",
            manifest.segment_object_hash
        ))
    })?;
    ArchiveV2SegmentCodec::decode_block_at(&object, manifest, identity, slot)
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
    fn verified_catalog_fallback_authenticates_retired_genesis() {
        let root = tempdir().unwrap();
        let block = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"retired-genesis-state"),
            [4; 32],
            Vec::new(),
            1,
        );
        let identity = ArchiveV2Identity {
            network_id: "retired-genesis-testnet".to_string(),
            genesis_hash: block.hash(),
        };
        let (object, manifest) = ArchiveV2SegmentCodec::encode(
            identity.clone(),
            None,
            Hash::default(),
            &super::super::ArchiveV2SegmentContents::from_blocks(vec![block.clone()]),
            &ArchiveV2CodecConfig {
                target_frame_bytes: 1024 * 1024,
                ..ArchiveV2CodecConfig::default()
            },
        )
        .unwrap();
        let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
        catalog.append(manifest.clone()).unwrap();
        catalog
            .store_atomic(&root.path().join("catalog.av2"))
            .unwrap();
        write_atomic_identical(
            &object_path(root.path(), &manifest.segment_object_hash),
            &object,
        )
        .unwrap();

        let restored = verified_catalog_block_at(root.path(), &identity, 0)
            .unwrap()
            .unwrap();
        assert_eq!(restored.header.slot, 0);
        assert_eq!(restored.hash(), block.hash());
        assert!(verified_catalog_block_at(root.path(), &identity, 1)
            .unwrap()
            .is_none());
    }

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
                acknowledge_exact_testnet_missing_watermark: false,
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
                acknowledge_exact_testnet_missing_watermark: false,
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

    #[test]
    fn exact_testnet_missing_watermark_requires_explicit_acknowledgement() {
        let state_root = tempdir().unwrap();
        let state = StateStore::open(state_root.path()).unwrap();
        let exact_identity = ArchiveV2Identity {
            network_id: "lichen-testnet-1".to_string(),
            genesis_hash: Hash::from_hex(
                "f08308ef2520af0967120f3314fa95b14d8239a898d34a6993981cb93f740884",
            )
            .unwrap(),
        };
        let catalog = ArchiveV2Catalog::empty(exact_identity.clone()).unwrap();
        let root = tempdir().unwrap();
        let replica = tempdir().unwrap();
        let options = ArchiveV2BuildOptions {
            root: root.path().to_path_buf(),
            start_slot: 0,
            end_slot: 0,
            required_finality_depth_slots: 0,
            codec: ArchiveV2CodecConfig::default(),
            replica_roots: vec![replica.path().to_path_buf()],
            required_replica_count: 1,
            acknowledge_exact_testnet_missing_watermark: true,
        };
        let builder = ArchiveV2Builder {
            state: &state,
            identity: exact_identity,
            options: options.clone(),
        };
        builder.verify_archive_watermark(0, &catalog).unwrap();

        let unacknowledged = ArchiveV2Builder {
            state: &state,
            identity: builder.identity.clone(),
            options: ArchiveV2BuildOptions {
                acknowledge_exact_testnet_missing_watermark: false,
                ..options
            },
        };
        let error = unacknowledged
            .verify_archive_watermark(0, &catalog)
            .unwrap_err();
        assert!(error
            .to_string()
            .contains("explicit signed-release acknowledgement"));
    }

    #[test]
    fn missing_watermark_acknowledgement_is_rejected_for_other_networks() {
        let state_root = tempdir().unwrap();
        let state = StateStore::open(state_root.path()).unwrap();
        let block = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"other-network-state"),
            [7; 32],
            Vec::new(),
            1,
        );
        state.put_block_atomic(&block, Some(0), Some(0)).unwrap();
        state
            .put_metadata(crate::CHAIN_ID_METADATA_KEY, b"other-network")
            .unwrap();
        let root = tempdir().unwrap();
        let replica = tempdir().unwrap();
        let error = ArchiveV2Builder::new(
            &state,
            ArchiveV2Identity {
                network_id: "other-network".to_string(),
                genesis_hash: block.hash(),
            },
            ArchiveV2BuildOptions {
                root: root.path().to_path_buf(),
                start_slot: 0,
                end_slot: 0,
                required_finality_depth_slots: 0,
                codec: ArchiveV2CodecConfig::default(),
                replica_roots: vec![replica.path().to_path_buf()],
                required_replica_count: 1,
                acknowledge_exact_testnet_missing_watermark: true,
            },
        )
        .err()
        .expect("other networks must reject the exact-testnet acknowledgement");
        assert!(error
            .to_string()
            .contains("not the exact lichen-testnet-1 waiver"));
    }
}
