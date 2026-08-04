use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::{
    ArchiveV2Catalog, ArchiveV2Error, ArchiveV2Identity, ArchiveV2ReplicaTransport, ArchiveV2Role,
    ArchiveV2RoleConfig,
};
use crate::{Hash, STATE_SNAPSHOT_CATEGORIES};

pub const ARCHIVE_V2_JOIN_PLAN_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveV2JoinArchiveAction {
    MirrorEveryActiveSegment,
    FetchOnVerifiedDemand,
    CatalogCommitmentsOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveV2MutableStateJoinMethod {
    VerifiedCategoryChunkSync,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2JoinPlan {
    pub version: u16,
    pub identity: ArchiveV2Identity,
    pub role: ArchiveV2Role,
    pub catalog_root: Hash,
    pub catalog_start_slot: Option<u64>,
    pub catalog_end_slot: Option<u64>,
    pub archive_action: ArchiveV2JoinArchiveAction,
    pub mutable_state_method: ArchiveV2MutableStateJoinMethod,
    pub checkpoint_categories: Vec<String>,
    pub recent_history_slots: u64,
    pub preserve_local_validator_identity: bool,
    pub preserve_local_consensus_wal: bool,
    pub permits_raw_database_copy: bool,
}

impl ArchiveV2JoinPlan {
    pub fn new(
        config: &ArchiveV2RoleConfig,
        catalog: &ArchiveV2Catalog,
    ) -> Result<Self, ArchiveV2Error> {
        catalog.validate()?;
        let admission = config.admit(&super::ArchiveV2RoleRequirements {
            independent_consensus_state: true,
            consensus_wal_and_identity: true,
            recovery_data_present: true,
            complete_catalog_verified: true,
            every_segment_local: true,
            authenticated_remote_sources: (config.role == ArchiveV2Role::VerifiedCache) as u32,
            cache_staging_headroom_bytes: if config.role == ArchiveV2Role::VerifiedCache {
                config.verified_cache_quota_bytes.max(1)
            } else {
                0
            },
            network_archive_policy_satisfied: true,
            no_archive_operation_in_progress: true,
        })?;
        if !admission.admitted {
            return Err(ArchiveV2Error::Role(admission.reasons.join("; ")));
        }
        let range = catalog
            .entries
            .first()
            .zip(catalog.entries.last())
            .map(|(first, last)| (first.manifest.start_slot, last.manifest.end_slot));
        Ok(Self {
            version: ARCHIVE_V2_JOIN_PLAN_VERSION,
            identity: catalog.identity.clone(),
            role: config.role,
            catalog_root: catalog.catalog_root,
            catalog_start_slot: range.map(|range| range.0),
            catalog_end_slot: range.map(|range| range.1),
            archive_action: match config.role {
                ArchiveV2Role::FullArchive => ArchiveV2JoinArchiveAction::MirrorEveryActiveSegment,
                ArchiveV2Role::VerifiedCache => ArchiveV2JoinArchiveAction::FetchOnVerifiedDemand,
                ArchiveV2Role::Consensus => ArchiveV2JoinArchiveAction::CatalogCommitmentsOnly,
            },
            mutable_state_method: ArchiveV2MutableStateJoinMethod::VerifiedCategoryChunkSync,
            checkpoint_categories: STATE_SNAPSHOT_CATEGORIES
                .iter()
                .map(|category| (*category).to_string())
                .collect(),
            recent_history_slots: config.recent_history_slots,
            preserve_local_validator_identity: true,
            preserve_local_consensus_wal: true,
            permits_raw_database_copy: false,
        })
    }

    pub fn recent_history_start_slot(&self, checkpoint_slot: u64) -> u64 {
        checkpoint_slot.saturating_sub(self.recent_history_slots.saturating_sub(1))
    }

    pub fn validate(&self) -> Result<(), ArchiveV2Error> {
        if self.version != ARCHIVE_V2_JOIN_PLAN_VERSION
            || self.recent_history_slots < 50_000
            || !self.preserve_local_validator_identity
            || !self.preserve_local_consensus_wal
            || self.permits_raw_database_copy
            || self.mutable_state_method
                != ArchiveV2MutableStateJoinMethod::VerifiedCategoryChunkSync
        {
            return Err(ArchiveV2Error::Role(
                "Archive V2 join plan weakens independent-state or identity requirements"
                    .to_string(),
            ));
        }
        self.identity.validate()?;
        if self.catalog_start_slot.is_some() != self.catalog_end_slot.is_some()
            || self
                .catalog_start_slot
                .zip(self.catalog_end_slot)
                .is_some_and(|(start, end)| end < start)
        {
            return Err(ArchiveV2Error::Continuity(
                "Archive V2 join plan has an invalid catalog range".to_string(),
            ));
        }
        let expected_action = match self.role {
            ArchiveV2Role::FullArchive => ArchiveV2JoinArchiveAction::MirrorEveryActiveSegment,
            ArchiveV2Role::VerifiedCache => ArchiveV2JoinArchiveAction::FetchOnVerifiedDemand,
            ArchiveV2Role::Consensus => ArchiveV2JoinArchiveAction::CatalogCommitmentsOnly,
        };
        if self.archive_action != expected_action {
            return Err(ArchiveV2Error::Role(
                "Archive V2 join archive action does not match its role".to_string(),
            ));
        }
        let required = STATE_SNAPSHOT_CATEGORIES
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        let actual = self
            .checkpoint_categories
            .iter()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        if actual != required || actual.len() != self.checkpoint_categories.len() {
            return Err(ArchiveV2Error::Ordering(
                "Archive V2 join checkpoint category set is incomplete or duplicated".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveV2CatalogDiscoveryReport {
    pub catalog: Option<ArchiveV2Catalog>,
    pub accepted_sources: Vec<String>,
    pub unavailable_sources: Vec<String>,
}

pub fn discover_archive_v2_catalog(
    expected_identity: &ArchiveV2Identity,
    sources: &[Arc<dyn ArchiveV2ReplicaTransport>],
) -> Result<ArchiveV2CatalogDiscoveryReport, ArchiveV2Error> {
    expected_identity.validate()?;
    if sources.is_empty() {
        return Err(ArchiveV2Error::Unavailable(
            "Archive V2 join has no catalog source".to_string(),
        ));
    }
    let mut report = ArchiveV2CatalogDiscoveryReport::default();
    for source in sources {
        if !source.authenticated() {
            report
                .unavailable_sources
                .push(format!("{} is not authenticated", source.name()));
            continue;
        }
        let candidate = match source.fetch_catalog() {
            Ok(Some(bytes)) => ArchiveV2Catalog::decode_canonical(&bytes),
            Ok(None) => {
                report
                    .unavailable_sources
                    .push(format!("{} has no catalog", source.name()));
                continue;
            }
            Err(error) => {
                report
                    .unavailable_sources
                    .push(format!("{}: {error}", source.name()));
                continue;
            }
        }?;
        if &candidate.identity != expected_identity {
            return Err(
                if candidate.identity.network_id != expected_identity.network_id {
                    ArchiveV2Error::WrongNetwork {
                        expected: expected_identity.network_id.clone(),
                        actual: candidate.identity.network_id,
                    }
                } else {
                    ArchiveV2Error::WrongGenesis
                },
            );
        }
        if let Some(current) = report.catalog.as_mut() {
            let mut candidate_prefix = candidate.clone();
            if current.merge_verified_extension(&candidate).is_err()
                && candidate_prefix.merge_verified_extension(current).is_err()
            {
                return Err(ArchiveV2Error::Continuity(format!(
                    "authenticated source {} supplied a conflicting catalog",
                    source.name()
                )));
            }
        } else {
            report.catalog = Some(candidate);
        }
        report.accepted_sources.push(source.name().to_string());
    }
    if report.catalog.is_none() {
        return Err(ArchiveV2Error::Unavailable(format!(
            "no authenticated source supplied a valid catalog: {}",
            report.unavailable_sources.join("; ")
        )));
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::archive_v2::{
        ArchiveV2CodecConfig, ArchiveV2DirectoryReplica, ArchiveV2ReplicaTransport,
        ArchiveV2SegmentCodec, ArchiveV2SegmentContents,
    };
    use crate::Block;

    fn catalog_with_blocks(count: u64) -> ArchiveV2Catalog {
        let identity = ArchiveV2Identity {
            network_id: "join-testnet".to_string(),
            genesis_hash: Hash::hash(b"join-genesis"),
        };
        let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
        let mut parent = Hash::default();
        let mut previous = None;
        for slot in 0..count {
            let block = Block::new_with_timestamp(
                slot,
                parent,
                Hash::hash(&slot.to_le_bytes()),
                [8; 32],
                Vec::new(),
                slot + 1,
            );
            let (_, manifest) = ArchiveV2SegmentCodec::encode(
                identity.clone(),
                previous,
                parent,
                &ArchiveV2SegmentContents::from_blocks(vec![block]),
                &ArchiveV2CodecConfig {
                    target_frame_bytes: 1024 * 1024,
                    ..ArchiveV2CodecConfig::default()
                },
            )
            .unwrap();
            parent = manifest.last_block_hash;
            previous = Some(manifest.segment_object_hash);
            catalog.append(manifest).unwrap();
        }
        catalog
    }

    #[test]
    fn discovery_selects_an_exact_authenticated_extension() {
        let short = catalog_with_blocks(1);
        let long = catalog_with_blocks(2);
        let short_root = tempdir().unwrap();
        let long_root = tempdir().unwrap();
        let short_source = Arc::new(
            ArchiveV2DirectoryReplica::new("short", "region-a", short_root.path(), true).unwrap(),
        );
        let long_source = Arc::new(
            ArchiveV2DirectoryReplica::new("long", "region-b", long_root.path(), true).unwrap(),
        );
        short_source.put_catalog_extension(&short).unwrap();
        long_source.put_catalog_extension(&long).unwrap();
        let sources: Vec<Arc<dyn ArchiveV2ReplicaTransport>> = vec![short_source, long_source];
        let discovered = discover_archive_v2_catalog(&long.identity, &sources).unwrap();
        assert_eq!(discovered.catalog.unwrap(), long);
        assert_eq!(discovered.accepted_sources.len(), 2);
    }

    #[test]
    fn every_join_role_preserves_identity_wal_and_raw_database_isolation() {
        let catalog = catalog_with_blocks(1);
        for role in [
            ArchiveV2Role::FullArchive,
            ArchiveV2Role::VerifiedCache,
            ArchiveV2Role::Consensus,
        ] {
            let config = ArchiveV2RoleConfig {
                role,
                verified_cache_quota_bytes: if role == ArchiveV2Role::VerifiedCache {
                    1024
                } else {
                    0
                },
                advertise_deep_history: role != ArchiveV2Role::Consensus,
                ..ArchiveV2RoleConfig::default()
            };
            let plan = ArchiveV2JoinPlan::new(&config, &catalog).unwrap();
            plan.validate().unwrap();
            assert!(plan.preserve_local_validator_identity);
            assert!(plan.preserve_local_consensus_wal);
            assert!(!plan.permits_raw_database_copy);
            assert_eq!(plan.recent_history_start_slot(100_000), 50_001);
        }
    }
}
