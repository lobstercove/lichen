use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::{ArchiveV2Error, ArchiveV2Identity};
use crate::Hash;

pub const ARCHIVE_V2_ROLE_CONFIG_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveV2Role {
    FullArchive,
    VerifiedCache,
    Consensus,
}

impl fmt::Display for ArchiveV2Role {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::FullArchive => "full_archive",
            Self::VerifiedCache => "verified_cache",
            Self::Consensus => "consensus",
        })
    }
}

impl FromStr for ArchiveV2Role {
    type Err = ArchiveV2Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "full_archive" | "full-archive" => Ok(Self::FullArchive),
            "verified_cache" | "verified-cache" => Ok(Self::VerifiedCache),
            "consensus" => Ok(Self::Consensus),
            _ => Err(ArchiveV2Error::Role(format!(
                "unknown Archive V2 role {value:?}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2RoleConfig {
    pub version: u16,
    pub role: ArchiveV2Role,
    pub recent_history_slots: u64,
    pub verified_cache_quota_bytes: u64,
    pub advertise_deep_history: bool,
}

impl Default for ArchiveV2RoleConfig {
    fn default() -> Self {
        Self {
            version: ARCHIVE_V2_ROLE_CONFIG_VERSION,
            role: ArchiveV2Role::FullArchive,
            recent_history_slots: 50_000,
            verified_cache_quota_bytes: 0,
            advertise_deep_history: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveV2RoleRequirements {
    pub independent_consensus_state: bool,
    pub consensus_wal_and_identity: bool,
    pub recovery_data_present: bool,
    pub complete_catalog_verified: bool,
    pub every_segment_local: bool,
    pub authenticated_remote_sources: u32,
    pub cache_staging_headroom_bytes: u64,
    pub network_archive_policy_satisfied: bool,
    pub no_archive_operation_in_progress: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchiveV2RoleAdmission {
    pub role: ArchiveV2Role,
    pub admitted: bool,
    pub serves_deep_history: bool,
    pub remote_outage_affects_consensus: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2CapabilityAdvertisement {
    pub version: u16,
    pub identity: ArchiveV2Identity,
    pub role: ArchiveV2Role,
    pub catalog_root: Hash,
    pub catalog_start_slot: Option<u64>,
    pub catalog_end_slot: Option<u64>,
    pub serves_deep_history: bool,
    pub remote_fetch_enabled: bool,
}

impl ArchiveV2CapabilityAdvertisement {
    pub fn validate(&self) -> Result<(), ArchiveV2Error> {
        if self.version != ARCHIVE_V2_ROLE_CONFIG_VERSION {
            return Err(ArchiveV2Error::Role(format!(
                "unsupported Archive V2 capability version {}",
                self.version
            )));
        }
        self.identity.validate()?;
        if self
            .catalog_start_slot
            .zip(self.catalog_end_slot)
            .is_some_and(|(start, end)| end < start)
            || self.catalog_start_slot.is_some() != self.catalog_end_slot.is_some()
        {
            return Err(ArchiveV2Error::Continuity(
                "Archive V2 capability has an invalid catalog range".to_string(),
            ));
        }
        match self.role {
            ArchiveV2Role::FullArchive if !self.serves_deep_history => {
                return Err(ArchiveV2Error::Role(
                    "full archive capability must serve deep history".to_string(),
                ));
            }
            ArchiveV2Role::VerifiedCache
                if !self.serves_deep_history || !self.remote_fetch_enabled =>
            {
                return Err(ArchiveV2Error::Role(
                    "verified-cache capability must advertise verified remote fetch".to_string(),
                ));
            }
            ArchiveV2Role::Consensus if self.serves_deep_history || self.remote_fetch_enabled => {
                return Err(ArchiveV2Error::Role(
                    "consensus capability must not advertise deep history".to_string(),
                ));
            }
            _ => {}
        }
        Ok(())
    }
}

impl ArchiveV2RoleConfig {
    pub fn admit(
        &self,
        requirements: &ArchiveV2RoleRequirements,
    ) -> Result<ArchiveV2RoleAdmission, ArchiveV2Error> {
        if self.version != ARCHIVE_V2_ROLE_CONFIG_VERSION {
            return Err(ArchiveV2Error::Role(format!(
                "unsupported role config version {}",
                self.version
            )));
        }
        if self.recent_history_slots < 50_000 {
            return Err(ArchiveV2Error::Role(
                "recent history retention must be at least 50000 slots".to_string(),
            ));
        }
        let mut reasons = Vec::new();
        if !requirements.independent_consensus_state {
            reasons.push("independent consensus state is missing".to_string());
        }
        if !requirements.consensus_wal_and_identity {
            reasons.push("consensus WAL or identity is missing".to_string());
        }
        if !requirements.recovery_data_present {
            reasons.push("required recovery data is missing".to_string());
        }
        if !requirements.complete_catalog_verified {
            reasons.push("complete verified archive catalog is missing".to_string());
        }

        match self.role {
            ArchiveV2Role::FullArchive => {
                if !requirements.every_segment_local {
                    reasons.push("full archive does not have every required segment".to_string());
                }
                if !self.advertise_deep_history {
                    reasons.push("full archive must advertise deep history".to_string());
                }
            }
            ArchiveV2Role::VerifiedCache => {
                if requirements.authenticated_remote_sources == 0 {
                    reasons.push(
                        "verified-cache role has no authenticated remote archive source"
                            .to_string(),
                    );
                }
                if self.verified_cache_quota_bytes == 0 {
                    reasons.push("verified-cache role has a zero cache quota".to_string());
                }
                if requirements.cache_staging_headroom_bytes == 0 {
                    reasons.push("verified-cache role has no fetch staging headroom".to_string());
                }
                if !self.advertise_deep_history {
                    reasons.push(
                        "verified-cache role must advertise typed fetchable deep history"
                            .to_string(),
                    );
                }
            }
            ArchiveV2Role::Consensus => {
                if self.advertise_deep_history {
                    reasons.push(
                        "consensus role must not advertise unavailable deep history".to_string(),
                    );
                }
                if self.verified_cache_quota_bytes != 0 {
                    reasons.push("consensus role must not configure an archive cache".to_string());
                }
            }
        }

        Ok(ArchiveV2RoleAdmission {
            role: self.role,
            admitted: reasons.is_empty(),
            serves_deep_history: matches!(
                self.role,
                ArchiveV2Role::FullArchive | ArchiveV2Role::VerifiedCache
            ),
            remote_outage_affects_consensus: false,
            reasons,
        })
    }

    pub fn admit_transition(
        &self,
        previous_role: ArchiveV2Role,
        requirements: &ArchiveV2RoleRequirements,
    ) -> Result<ArchiveV2RoleAdmission, ArchiveV2Error> {
        let mut admission = self.admit(requirements)?;
        if previous_role != self.role {
            if !requirements.no_archive_operation_in_progress {
                admission.reasons.push(
                    "an archive build, mirror, or retirement operation is active".to_string(),
                );
            }
            if matches!(previous_role, ArchiveV2Role::FullArchive)
                && !matches!(self.role, ArchiveV2Role::FullArchive)
                && !requirements.network_archive_policy_satisfied
            {
                admission.reasons.push(
                    "network archive replica policy is not proven for full-archive demotion"
                        .to_string(),
                );
            }
            admission.admitted = admission.reasons.is_empty();
        }
        Ok(admission)
    }

    pub fn capability(
        &self,
        identity: ArchiveV2Identity,
        catalog_root: Hash,
        catalog_range: Option<(u64, u64)>,
        admission: &ArchiveV2RoleAdmission,
    ) -> Result<ArchiveV2CapabilityAdvertisement, ArchiveV2Error> {
        if !admission.admitted || admission.role != self.role {
            return Err(ArchiveV2Error::Role(
                "cannot advertise an unadmitted Archive V2 role".to_string(),
            ));
        }
        let capability = ArchiveV2CapabilityAdvertisement {
            version: self.version,
            identity,
            role: self.role,
            catalog_root,
            catalog_start_slot: catalog_range.map(|range| range.0),
            catalog_end_slot: catalog_range.map(|range| range.1),
            serves_deep_history: admission.serves_deep_history,
            remote_fetch_enabled: self.role == ArchiveV2Role::VerifiedCache,
        };
        capability.validate()?;
        Ok(capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_requirements() -> ArchiveV2RoleRequirements {
        ArchiveV2RoleRequirements {
            independent_consensus_state: true,
            consensus_wal_and_identity: true,
            recovery_data_present: true,
            complete_catalog_verified: true,
            every_segment_local: true,
            authenticated_remote_sources: 2,
            cache_staging_headroom_bytes: 1024 * 1024 * 1024,
            network_archive_policy_satisfied: true,
            no_archive_operation_in_progress: true,
        }
    }

    #[test]
    fn every_role_fails_closed_when_its_requirements_are_unmet() {
        let full = ArchiveV2RoleConfig::default()
            .admit(&ArchiveV2RoleRequirements {
                every_segment_local: false,
                ..base_requirements()
            })
            .unwrap();
        assert!(!full.admitted);

        let cache = ArchiveV2RoleConfig {
            role: ArchiveV2Role::VerifiedCache,
            verified_cache_quota_bytes: 1024,
            ..ArchiveV2RoleConfig::default()
        }
        .admit(&ArchiveV2RoleRequirements {
            authenticated_remote_sources: 0,
            ..base_requirements()
        })
        .unwrap();
        assert!(!cache.admitted);

        let consensus = ArchiveV2RoleConfig {
            role: ArchiveV2Role::Consensus,
            verified_cache_quota_bytes: 0,
            advertise_deep_history: false,
            ..ArchiveV2RoleConfig::default()
        }
        .admit(&base_requirements())
        .unwrap();
        assert!(consensus.admitted);
        assert!(!consensus.serves_deep_history);
        assert!(!consensus.remote_outage_affects_consensus);
    }

    #[test]
    fn full_archive_demotion_requires_replica_policy_and_idle_operations() {
        let consensus = ArchiveV2RoleConfig {
            role: ArchiveV2Role::Consensus,
            verified_cache_quota_bytes: 0,
            advertise_deep_history: false,
            ..ArchiveV2RoleConfig::default()
        };
        let denied = consensus
            .admit_transition(
                ArchiveV2Role::FullArchive,
                &ArchiveV2RoleRequirements {
                    network_archive_policy_satisfied: false,
                    no_archive_operation_in_progress: false,
                    ..base_requirements()
                },
            )
            .unwrap();
        assert!(!denied.admitted);
        assert_eq!(denied.reasons.len(), 2);
    }
}
