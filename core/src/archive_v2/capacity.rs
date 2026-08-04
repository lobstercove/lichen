use serde::{Deserialize, Serialize};

use super::ArchiveV2Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveV2PressureAction {
    Normal,
    StopSegmentBuilding,
    EvictVerifiedCache,
    StopCheckpointWork,
    PreserveArchiveObjects,
    StopValidator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveV2CapacityComponent {
    None,
    HotConsensus,
    ArchiveBuild,
    VerifiedCache,
    Checkpoint,
    ArchivePreservation,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2CapacityInputs {
    pub segment_build_enabled: bool,
    pub verified_cache_enabled: bool,
    pub checkpoint_enabled: bool,
    pub hot_available_bytes: u64,
    pub archive_available_bytes: u64,
    pub cache_available_bytes: u64,
    pub mutable_state_write_peak_bytes: u64,
    pub wal_peak_bytes: u64,
    pub bounded_compaction_peak_bytes: u64,
    pub checkpoint_peak_bytes: u64,
    pub segment_staging_peak_bytes: u64,
    pub verification_copy_bytes: u64,
    pub replication_retry_bytes: u64,
    pub filesystem_reserve_bytes: u64,
    pub cache_fetch_staging_bytes: u64,
    pub cache_eviction_margin_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2CapacityThresholds {
    pub hot_warning_bytes: u64,
    pub hot_fatal_bytes: u64,
    pub archive_warning_bytes: u64,
    pub cache_warning_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2CapacityTotals {
    pub hot_total_bytes: u64,
    pub archive_total_bytes: u64,
    pub cache_total_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2AdaptiveReservePolicy {
    pub reserve_basis_points: u16,
    pub hot_growth_reserve_bytes: u64,
    pub archive_growth_reserve_bytes: u64,
    pub cache_growth_reserve_bytes: u64,
    pub emergency_evidence_reserve_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2CapacityDecision {
    pub action: ArchiveV2PressureAction,
    pub limiting_component: ArchiveV2CapacityComponent,
    pub available_bytes: u64,
    pub required_bytes: u64,
    pub absolute_reserve_bytes: u64,
    pub percentage_reserve_bytes: u64,
    pub growth_reserve_bytes: u64,
    pub staging_reserve_bytes: u64,
    pub compaction_reserve_bytes: u64,
    pub hot_available_bytes: u64,
    pub archive_available_bytes: u64,
    pub cache_available_bytes: u64,
    pub hot_consensus_required_bytes: u64,
    pub hot_required_bytes: u64,
    pub archive_required_bytes: u64,
    pub cache_required_bytes: u64,
    pub warning: bool,
    pub fatal: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ArchiveV2CapacityGuard;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2CapacitySample {
    pub unix_seconds: u64,
    pub hot_used_bytes: u64,
    pub archive_used_bytes: u64,
    pub cache_used_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveV2CapacityForecast {
    pub sample_window_seconds: u64,
    pub hot_growth_bytes_per_hour: f64,
    pub archive_growth_bytes_per_hour: f64,
    pub cache_growth_bytes_per_hour: f64,
    pub hot_hours_until_reserve: Option<f64>,
    pub archive_hours_until_reserve: Option<f64>,
    pub cache_hours_until_reserve: Option<f64>,
    pub planning_horizon_hours: u64,
    pub planning_horizon_satisfied: bool,
}

impl ArchiveV2CapacityGuard {
    pub fn evaluate(
        inputs: ArchiveV2CapacityInputs,
        thresholds: ArchiveV2CapacityThresholds,
    ) -> Result<ArchiveV2CapacityDecision, ArchiveV2Error> {
        Self::evaluate_adaptive(
            inputs,
            thresholds,
            ArchiveV2CapacityTotals::default(),
            ArchiveV2AdaptiveReservePolicy::default(),
        )
    }

    pub fn evaluate_adaptive(
        inputs: ArchiveV2CapacityInputs,
        thresholds: ArchiveV2CapacityThresholds,
        totals: ArchiveV2CapacityTotals,
        policy: ArchiveV2AdaptiveReservePolicy,
    ) -> Result<ArchiveV2CapacityDecision, ArchiveV2Error> {
        if thresholds.hot_fatal_bytes == 0
            || thresholds.hot_warning_bytes < thresholds.hot_fatal_bytes
        {
            return Err(ArchiveV2Error::Bounds(
                "hot capacity thresholds are invalid".to_string(),
            ));
        }
        if policy.reserve_basis_points > 5_000 {
            return Err(ArchiveV2Error::Bounds(
                "capacity percentage reserve must be in 0..=5000 basis points".to_string(),
            ));
        }
        let percentage = |total: u64| {
            total
                .saturating_mul(u64::from(policy.reserve_basis_points))
                .saturating_add(9_999)
                / 10_000
        };
        let hot_percentage_reserve = percentage(totals.hot_total_bytes);
        let archive_percentage_reserve = percentage(totals.archive_total_bytes);
        let cache_percentage_reserve = percentage(totals.cache_total_bytes);
        let hot_base_reserve = inputs
            .filesystem_reserve_bytes
            .max(hot_percentage_reserve)
            .max(policy.hot_growth_reserve_bytes)
            .max(policy.emergency_evidence_reserve_bytes)
            .max(thresholds.hot_fatal_bytes);
        let archive_base_reserve = inputs
            .filesystem_reserve_bytes
            .max(archive_percentage_reserve)
            .max(policy.archive_growth_reserve_bytes)
            .max(policy.emergency_evidence_reserve_bytes);
        let cache_base_reserve = inputs
            .filesystem_reserve_bytes
            .max(cache_percentage_reserve)
            .max(policy.cache_growth_reserve_bytes)
            .max(policy.emergency_evidence_reserve_bytes);
        let hot_consensus_required_bytes = inputs
            .mutable_state_write_peak_bytes
            .saturating_add(inputs.wal_peak_bytes)
            .saturating_add(inputs.bounded_compaction_peak_bytes)
            .saturating_add(hot_base_reserve);
        let checkpoint_reserve = if inputs.checkpoint_enabled {
            inputs.checkpoint_peak_bytes
        } else {
            0
        };
        let hot_required_bytes = hot_consensus_required_bytes.saturating_add(checkpoint_reserve);
        let archive_operation_reserve = if inputs.segment_build_enabled {
            inputs
                .segment_staging_peak_bytes
                .saturating_add(inputs.verification_copy_bytes)
                .saturating_add(inputs.replication_retry_bytes)
        } else {
            0
        };
        let archive_required_bytes = archive_operation_reserve.saturating_add(archive_base_reserve);
        let cache_operation_reserve = if inputs.verified_cache_enabled {
            inputs
                .cache_fetch_staging_bytes
                .saturating_add(inputs.cache_eviction_margin_bytes)
        } else {
            0
        };
        let cache_required_bytes = cache_operation_reserve.saturating_add(cache_base_reserve);
        let mut reasons = Vec::new();
        let fatal = inputs.hot_available_bytes < hot_consensus_required_bytes;
        let (action, limiting_component) = if fatal {
            reasons.push(format!(
                "hot mutable storage has {} bytes, below consensus reserve {}",
                inputs.hot_available_bytes, hot_consensus_required_bytes
            ));
            (
                ArchiveV2PressureAction::StopValidator,
                ArchiveV2CapacityComponent::HotConsensus,
            )
        } else if inputs.segment_build_enabled
            && inputs.archive_available_bytes < archive_required_bytes
        {
            reasons.push(format!(
                "archive storage cannot cover build/verification reserve {archive_required_bytes}"
            ));
            (
                ArchiveV2PressureAction::StopSegmentBuilding,
                ArchiveV2CapacityComponent::ArchiveBuild,
            )
        } else if inputs.verified_cache_enabled
            && inputs.cache_available_bytes < cache_required_bytes
        {
            reasons.push(format!(
                "cache storage cannot cover fetch/eviction reserve {cache_required_bytes}"
            ));
            (
                ArchiveV2PressureAction::EvictVerifiedCache,
                ArchiveV2CapacityComponent::VerifiedCache,
            )
        } else if inputs.checkpoint_enabled && inputs.hot_available_bytes < hot_required_bytes {
            reasons.push(format!(
                "hot storage cannot cover calculated reserve {hot_required_bytes}"
            ));
            (
                ArchiveV2PressureAction::StopCheckpointWork,
                ArchiveV2CapacityComponent::Checkpoint,
            )
        } else if inputs.archive_available_bytes < thresholds.archive_warning_bytes {
            reasons.push("archive storage is below its warning floor".to_string());
            (
                ArchiveV2PressureAction::PreserveArchiveObjects,
                ArchiveV2CapacityComponent::ArchivePreservation,
            )
        } else {
            (
                ArchiveV2PressureAction::Normal,
                ArchiveV2CapacityComponent::None,
            )
        };
        let warning = action != ArchiveV2PressureAction::Normal
            || inputs.hot_available_bytes < thresholds.hot_warning_bytes
            || inputs.cache_available_bytes < thresholds.cache_warning_bytes;
        let (
            available_bytes,
            required_bytes,
            percentage_reserve_bytes,
            growth_reserve_bytes,
            staging_reserve_bytes,
        ) = match limiting_component {
            ArchiveV2CapacityComponent::HotConsensus => (
                inputs.hot_available_bytes,
                hot_consensus_required_bytes,
                hot_percentage_reserve,
                policy.hot_growth_reserve_bytes,
                0,
            ),
            ArchiveV2CapacityComponent::ArchiveBuild => (
                inputs.archive_available_bytes,
                archive_required_bytes,
                archive_percentage_reserve,
                policy.archive_growth_reserve_bytes,
                archive_operation_reserve,
            ),
            ArchiveV2CapacityComponent::VerifiedCache => (
                inputs.cache_available_bytes,
                cache_required_bytes,
                cache_percentage_reserve,
                policy.cache_growth_reserve_bytes,
                cache_operation_reserve,
            ),
            ArchiveV2CapacityComponent::Checkpoint => (
                inputs.hot_available_bytes,
                hot_required_bytes,
                hot_percentage_reserve,
                policy.hot_growth_reserve_bytes,
                checkpoint_reserve,
            ),
            ArchiveV2CapacityComponent::ArchivePreservation => (
                inputs.archive_available_bytes,
                thresholds.archive_warning_bytes,
                archive_percentage_reserve,
                policy.archive_growth_reserve_bytes,
                0,
            ),
            ArchiveV2CapacityComponent::None => (
                inputs.hot_available_bytes,
                hot_required_bytes,
                hot_percentage_reserve,
                policy.hot_growth_reserve_bytes,
                checkpoint_reserve,
            ),
        };
        Ok(ArchiveV2CapacityDecision {
            action,
            limiting_component,
            available_bytes,
            required_bytes,
            absolute_reserve_bytes: inputs.filesystem_reserve_bytes,
            percentage_reserve_bytes,
            growth_reserve_bytes,
            staging_reserve_bytes,
            compaction_reserve_bytes: inputs.bounded_compaction_peak_bytes,
            hot_available_bytes: inputs.hot_available_bytes,
            archive_available_bytes: inputs.archive_available_bytes,
            cache_available_bytes: inputs.cache_available_bytes,
            hot_consensus_required_bytes,
            hot_required_bytes,
            archive_required_bytes,
            cache_required_bytes,
            warning,
            fatal,
            reasons,
        })
    }

    pub fn forecast(
        samples: &[ArchiveV2CapacitySample],
        current: ArchiveV2CapacityInputs,
        decision: &ArchiveV2CapacityDecision,
        planning_horizon_hours: u64,
    ) -> Result<ArchiveV2CapacityForecast, ArchiveV2Error> {
        if samples.len() < 2 || samples.len() > 100_000 {
            return Err(ArchiveV2Error::Bounds(
                "capacity forecast requires 2..=100000 samples".to_string(),
            ));
        }
        if planning_horizon_hours == 0 || planning_horizon_hours > 24 * 365 * 10 {
            return Err(ArchiveV2Error::Bounds(
                "capacity planning horizon must be in 1 hour..=10 years".to_string(),
            ));
        }
        if samples
            .windows(2)
            .any(|pair| pair[0].unix_seconds >= pair[1].unix_seconds)
        {
            return Err(ArchiveV2Error::Ordering(
                "capacity samples are duplicated or out of order".to_string(),
            ));
        }
        let first = samples.first().expect("sample length checked");
        let last = samples.last().expect("sample length checked");
        let window = last.unix_seconds - first.unix_seconds;
        let growth_per_hour =
            |start: u64, end: u64| end.saturating_sub(start) as f64 * 3600.0 / window as f64;
        let hot_growth = growth_per_hour(first.hot_used_bytes, last.hot_used_bytes);
        let archive_growth = growth_per_hour(first.archive_used_bytes, last.archive_used_bytes);
        let cache_growth = growth_per_hour(first.cache_used_bytes, last.cache_used_bytes);
        let hours_until = |available: u64, required: u64, growth: f64| {
            if growth <= 0.0 {
                None
            } else {
                Some(available.saturating_sub(required) as f64 / growth)
            }
        };
        let hot_hours = hours_until(
            current.hot_available_bytes,
            decision.hot_required_bytes,
            hot_growth,
        );
        let archive_hours = hours_until(
            current.archive_available_bytes,
            decision.archive_required_bytes,
            archive_growth,
        );
        let cache_hours = hours_until(
            current.cache_available_bytes,
            decision.cache_required_bytes,
            cache_growth,
        );
        let horizon = planning_horizon_hours as f64;
        let planning_horizon_satisfied = [hot_hours, archive_hours, cache_hours]
            .into_iter()
            .flatten()
            .all(|hours| hours >= horizon);
        Ok(ArchiveV2CapacityForecast {
            sample_window_seconds: window,
            hot_growth_bytes_per_hour: hot_growth,
            archive_growth_bytes_per_hour: archive_growth,
            cache_growth_bytes_per_hour: cache_growth,
            hot_hours_until_reserve: hot_hours,
            archive_hours_until_reserve: archive_hours,
            cache_hours_until_reserve: cache_hours,
            planning_horizon_hours,
            planning_horizon_satisfied,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thresholds() -> ArchiveV2CapacityThresholds {
        ArchiveV2CapacityThresholds {
            hot_warning_bytes: 10,
            hot_fatal_bytes: 5,
            archive_warning_bytes: 10,
            cache_warning_bytes: 10,
        }
    }

    #[test]
    fn capacity_priority_preserves_consensus_before_archive_and_cache() {
        let base = ArchiveV2CapacityInputs {
            segment_build_enabled: true,
            verified_cache_enabled: true,
            checkpoint_enabled: true,
            hot_available_bytes: 100,
            archive_available_bytes: 100,
            cache_available_bytes: 100,
            mutable_state_write_peak_bytes: 2,
            wal_peak_bytes: 2,
            bounded_compaction_peak_bytes: 2,
            checkpoint_peak_bytes: 2,
            segment_staging_peak_bytes: 10,
            verification_copy_bytes: 10,
            replication_retry_bytes: 10,
            filesystem_reserve_bytes: 2,
            cache_fetch_staging_bytes: 10,
            cache_eviction_margin_bytes: 10,
        };
        assert_eq!(
            ArchiveV2CapacityGuard::evaluate(base, thresholds())
                .unwrap()
                .action,
            ArchiveV2PressureAction::Normal
        );
        assert_eq!(
            ArchiveV2CapacityGuard::evaluate(
                ArchiveV2CapacityInputs {
                    archive_available_bytes: 1,
                    ..base
                },
                thresholds(),
            )
            .unwrap()
            .action,
            ArchiveV2PressureAction::StopSegmentBuilding
        );
        let fatal = ArchiveV2CapacityGuard::evaluate(
            ArchiveV2CapacityInputs {
                hot_available_bytes: 4,
                archive_available_bytes: 0,
                cache_available_bytes: 0,
                ..base
            },
            thresholds(),
        )
        .unwrap();
        assert_eq!(fatal.action, ArchiveV2PressureAction::StopValidator);
        assert!(fatal.fatal);
    }

    #[test]
    fn measured_growth_forecast_fails_an_insufficient_planning_horizon() {
        let inputs = ArchiveV2CapacityInputs {
            segment_build_enabled: true,
            verified_cache_enabled: false,
            checkpoint_enabled: true,
            hot_available_bytes: 100,
            archive_available_bytes: 100,
            cache_available_bytes: 0,
            mutable_state_write_peak_bytes: 2,
            wal_peak_bytes: 2,
            bounded_compaction_peak_bytes: 2,
            checkpoint_peak_bytes: 2,
            segment_staging_peak_bytes: 2,
            verification_copy_bytes: 2,
            replication_retry_bytes: 2,
            filesystem_reserve_bytes: 2,
            cache_fetch_staging_bytes: 0,
            cache_eviction_margin_bytes: 0,
        };
        let decision = ArchiveV2CapacityGuard::evaluate(inputs, thresholds()).unwrap();
        let forecast = ArchiveV2CapacityGuard::forecast(
            &[
                ArchiveV2CapacitySample {
                    unix_seconds: 1,
                    hot_used_bytes: 10,
                    archive_used_bytes: 10,
                    cache_used_bytes: 0,
                },
                ArchiveV2CapacitySample {
                    unix_seconds: 3_601,
                    hot_used_bytes: 20,
                    archive_used_bytes: 30,
                    cache_used_bytes: 0,
                },
            ],
            inputs,
            &decision,
            10,
        )
        .unwrap();
        assert!(!forecast.planning_horizon_satisfied);
        assert_eq!(forecast.archive_growth_bytes_per_hour, 20.0);
    }

    #[test]
    fn adaptive_decision_reports_shared_reserve_components_and_limiter() {
        let inputs = ArchiveV2CapacityInputs {
            segment_build_enabled: true,
            verified_cache_enabled: true,
            checkpoint_enabled: true,
            hot_available_bytes: 1_000,
            archive_available_bytes: 299,
            cache_available_bytes: 1_000,
            mutable_state_write_peak_bytes: 10,
            wal_peak_bytes: 10,
            bounded_compaction_peak_bytes: 20,
            checkpoint_peak_bytes: 30,
            segment_staging_peak_bytes: 40,
            verification_copy_bytes: 40,
            replication_retry_bytes: 40,
            filesystem_reserve_bytes: 100,
            cache_fetch_staging_bytes: 20,
            cache_eviction_margin_bytes: 20,
        };
        let decision = ArchiveV2CapacityGuard::evaluate_adaptive(
            inputs,
            ArchiveV2CapacityThresholds {
                hot_warning_bytes: 100,
                hot_fatal_bytes: 50,
                archive_warning_bytes: 100,
                cache_warning_bytes: 100,
            },
            ArchiveV2CapacityTotals {
                hot_total_bytes: 10_000,
                archive_total_bytes: 20_000,
                cache_total_bytes: 10_000,
            },
            ArchiveV2AdaptiveReservePolicy {
                reserve_basis_points: 100,
                archive_growth_reserve_bytes: 150,
                emergency_evidence_reserve_bytes: 75,
                ..ArchiveV2AdaptiveReservePolicy::default()
            },
        )
        .unwrap();
        assert_eq!(
            decision.action,
            ArchiveV2PressureAction::StopSegmentBuilding
        );
        assert_eq!(
            decision.limiting_component,
            ArchiveV2CapacityComponent::ArchiveBuild
        );
        assert_eq!(decision.available_bytes, 299);
        assert_eq!(decision.required_bytes, 320);
        assert_eq!(decision.absolute_reserve_bytes, 100);
        assert_eq!(decision.percentage_reserve_bytes, 200);
        assert_eq!(decision.growth_reserve_bytes, 150);
        assert_eq!(decision.staging_reserve_bytes, 120);
        assert_eq!(decision.compaction_reserve_bytes, 20);
    }
}
