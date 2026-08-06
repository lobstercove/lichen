use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rocksdb::{
    BottommostLevelCompaction, CompactOptions, FlushOptions, WriteBatch, WriteOptions, DB,
};
use serde::{Deserialize, Serialize};

use super::*;
use crate::archive_v2::{
    ArchiveV2CategoryProof, ArchiveV2ReplicaEvidence, ArchiveV2RetirementManifest,
    ArchiveV2RetirementRequest, ArchiveV2RollbackAnchor, ArchiveV2Rows,
};
use crate::codec::{deserialize_legacy_bincode_strict, serialize_legacy_bincode};

const RETIREMENT_JOURNAL_MAGIC: &[u8] = b"LICHEN-AV2-RETIRE-JOURNAL\0";
const RETIREMENT_JOURNAL_VERSION: u16 = 2;
const MAX_RETIREMENT_JOURNAL_BYTES: usize = 16 * 1024 * 1024;
const MAX_PENDING_DELETIONS: usize = 100_000;
const MAX_RETIREMENT_RECLAIM_RANGES: usize = 4_096;
const MAX_RETIREMENT_RECLAIM_INPUT_BYTES: u64 = 8 * 1024 * 1024 * 1024;
static RETIREMENT_TEMPORARY_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveV2RetirementLimits {
    pub max_rows: u64,
    pub max_bytes: u64,
    pub max_wall_time: Duration,
}

impl Default for ArchiveV2RetirementLimits {
    fn default() -> Self {
        Self {
            max_rows: 2_000,
            max_bytes: 64 * 1024 * 1024,
            max_wall_time: Duration::from_secs(2),
        }
    }
}

impl ArchiveV2RetirementLimits {
    pub fn validate(self) -> Result<Self, String> {
        if self.max_rows == 0 || self.max_rows > MAX_PENDING_DELETIONS as u64 {
            return Err("Archive V2 retirement max_rows must be in 1..=100000".to_string());
        }
        if self.max_bytes < 1024 * 1024 || self.max_bytes > 1024 * 1024 * 1024 {
            return Err("Archive V2 retirement max_bytes must be in 1 MiB..=1 GiB".to_string());
        }
        if self.max_wall_time < Duration::from_millis(10)
            || self.max_wall_time > Duration::from_secs(60)
        {
            return Err("Archive V2 retirement max_wall_time must be in 10ms..=60s".to_string());
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArchiveV2RetirementReclaimLimits {
    pub max_ranges: u64,
    pub max_estimated_input_bytes: u64,
    pub hot_available_bytes: u64,
    pub hot_required_reserve_bytes: u64,
    pub cold_available_bytes: u64,
    pub cold_required_reserve_bytes: u64,
}

impl ArchiveV2RetirementReclaimLimits {
    pub fn validate(self) -> Result<Self, String> {
        if self.max_ranges == 0 || self.max_ranges > 16 {
            return Err("Archive V2 retirement reclaim max_ranges must be in 1..=16".to_string());
        }
        if self.max_estimated_input_bytes < 1024 * 1024
            || self.max_estimated_input_bytes > MAX_RETIREMENT_RECLAIM_INPUT_BYTES
        {
            return Err(
                "Archive V2 retirement reclaim max_estimated_input_bytes must be in 1 MiB..=8 GiB"
                    .to_string(),
            );
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveV2RetirementReclaimReport {
    pub phase: ArchiveV2RetirementPhase,
    pub queued_ranges_before: u64,
    pub queued_ranges_after: u64,
    pub compacted_ranges: u64,
    pub estimated_input_bytes: u64,
    pub reclaimed_physical_bytes: u64,
    pub total_reclaimed_physical_bytes: u64,
    pub compaction_duration_millis: u64,
    pub paused_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArchiveV2RetirementPhase {
    #[default]
    Authorized,
    Tombstoning,
    ReclaimPending,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveV2RetirementFaultPoint {
    AfterPendingJournal,
    AfterHotDeletion,
    AfterColdDeletion,
    AfterProgressJournal,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchiveV2RetirementPassReport {
    pub phase: ArchiveV2RetirementPhase,
    pub category: Option<String>,
    pub scanned_rows: u64,
    pub deleted_hot_rows: u64,
    pub deleted_cold_rows: u64,
    pub deleted_logical_bytes: u64,
    pub recovered_pending_batch: bool,
    pub categories_completed: u64,
    pub elapsed_millis: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RetirementStore {
    Hot,
    Cold,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetirementFamily {
    store: RetirementStore,
    category: String,
    cf_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetirementReclaimRange {
    store: RetirementStore,
    cf_name: String,
    start_key: Vec<u8>,
    end_key: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingRetirementDeletion {
    store: RetirementStore,
    category: String,
    key: Vec<u8>,
    canonical_value_hash: Hash,
    logical_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveV2RetirementJournal {
    journal_version: u16,
    retirement_manifest_hash: Hash,
    segment_object_hash: Hash,
    start_slot: u64,
    end_slot: u64,
    phase: ArchiveV2RetirementPhase,
    category_index: u16,
    after_key: Option<Vec<u8>>,
    pending: Vec<PendingRetirementDeletion>,
    affected_families: Vec<RetirementFamily>,
    reclaim_initialized: bool,
    reclaim_queue: Vec<RetirementReclaimRange>,
    deleted_hot_rows: u64,
    deleted_cold_rows: u64,
    deleted_logical_bytes: u64,
    reclaimed_physical_bytes: u64,
}

impl StateStore {
    pub fn prepare_archive_v2_retirement_request(
        &self,
        segment_object_hash: Hash,
        mut replica_evidence: Vec<ArchiveV2ReplicaEvidence>,
        required_replica_count: u16,
        required_failure_domains: u16,
        rollback_anchor: ArchiveV2RollbackAnchor,
        authorized_unix_seconds: u64,
    ) -> Result<ArchiveV2RetirementRequest, String> {
        let reader = self
            .archive_v2_reader()
            .ok_or_else(|| "Archive V2 reader is not attached".to_string())?;
        let segment_manifest = reader
            .verify_segment(&segment_object_hash)
            .map_err(|error| error.to_string())?;
        let (start_slot, end_slot) = (segment_manifest.start_slot, segment_manifest.end_slot);

        let raw_categories = self.archive_v2_public_categories(start_slot, end_slot)?;
        let mut category_proofs = Vec::with_capacity(PUBLIC_HISTORY_SNAPSHOT_CATEGORIES.len());
        for category in PUBLIC_HISTORY_SNAPSHOT_CATEGORIES {
            let legacy_rows = if let Some(rows) = raw_categories.get(*category) {
                rows.iter()
                    .map(|row| (row.key.clone(), row.value.clone()))
                    .collect::<Vec<_>>()
            } else {
                self.archive_v2_legacy_category_rows(category, start_slot, end_slot)?
            };
            let legacy_rows = self.normalize_archive_v2_retirement_rows(category, legacy_rows)?;
            let archive_rows = reader
                .category_rows(category, start_slot, end_slot)
                .map_err(|error| error.to_string())?;
            let archive_rows = self.normalize_archive_v2_retirement_rows(category, archive_rows)?;
            if legacy_rows != archive_rows {
                return Err(format!(
                    "Archive V2 retirement equivalence failed for category {category}"
                ));
            }
            category_proofs.push(
                ArchiveV2CategoryProof::from_rows(*category, &archive_rows)
                    .map_err(|error| error.to_string())?,
            );
        }
        category_proofs.sort_by(|left, right| left.category.cmp(&right.category));
        replica_evidence.sort_by(|left, right| {
            left.failure_domain
                .cmp(&right.failure_domain)
                .then_with(|| left.destination.cmp(&right.destination))
        });

        Ok(ArchiveV2RetirementRequest {
            identity: reader.catalog().identity.clone(),
            catalog_root: reader.catalog().catalog_root,
            segment_manifest,
            category_proofs,
            replica_evidence,
            required_replica_count,
            required_failure_domains,
            rollback_anchor,
            authorized_unix_seconds,
        })
    }

    pub fn retire_archive_v2_segment_pass(
        &self,
        retirement: &ArchiveV2RetirementManifest,
        journal_path: &Path,
        limits: ArchiveV2RetirementLimits,
    ) -> Result<ArchiveV2RetirementPassReport, String> {
        self.retire_archive_v2_segment_pass_with_fault(retirement, journal_path, limits, None)
    }

    #[cfg(test)]
    fn retire_archive_v2_segment_pass_faulted(
        &self,
        retirement: &ArchiveV2RetirementManifest,
        journal_path: &Path,
        limits: ArchiveV2RetirementLimits,
        fault: ArchiveV2RetirementFaultPoint,
    ) -> Result<ArchiveV2RetirementPassReport, String> {
        self.retire_archive_v2_segment_pass_with_fault(
            retirement,
            journal_path,
            limits,
            Some(fault),
        )
    }

    fn retire_archive_v2_segment_pass_with_fault(
        &self,
        retirement: &ArchiveV2RetirementManifest,
        journal_path: &Path,
        limits: ArchiveV2RetirementLimits,
        fault: Option<ArchiveV2RetirementFaultPoint>,
    ) -> Result<ArchiveV2RetirementPassReport, String> {
        let limits = limits.validate()?;
        let _guard = self
            .cold_migration_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        retirement.validate().map_err(|error| error.to_string())?;
        let reader = self
            .archive_v2_reader()
            .ok_or_else(|| "Archive V2 reader is not attached".to_string())?;
        if retirement.identity() != &reader.catalog().identity
            || retirement.catalog_root() != reader.catalog().catalog_root
        {
            return Err("Retirement manifest does not match the attached catalog".to_string());
        }
        let segment = reader
            .verify_segment(&retirement.segment_object_hash())
            .map_err(|error| error.to_string())?;
        if segment.segment_content_root != retirement.segment_content_root()
            || (segment.start_slot, segment.end_slot) != retirement.slot_range()
        {
            return Err("Retirement manifest does not match its verified segment".to_string());
        }
        let current_slot = self.get_last_slot()?;
        if segment.end_slot >= current_slot.saturating_sub(COLD_RETENTION_SLOTS) {
            return Err(format!(
                "Refusing Archive V2 retirement inside the {COLD_RETENTION_SLOTS}-slot hot retention window"
            ));
        }

        let encoded_manifest = retirement
            .encode_canonical()
            .map_err(|error| error.to_string())?;
        let manifest_hash = Hash::hash(&encoded_manifest);
        let mut journal = if journal_path.exists() {
            let journal = load_retirement_journal(journal_path)?;
            validate_retirement_journal(&journal, retirement, manifest_hash)?;
            journal
        } else {
            self.verify_retirement_equivalence(retirement, &reader)?;
            let journal = ArchiveV2RetirementJournal {
                journal_version: RETIREMENT_JOURNAL_VERSION,
                retirement_manifest_hash: manifest_hash,
                segment_object_hash: retirement.segment_object_hash(),
                start_slot: segment.start_slot,
                end_slot: segment.end_slot,
                phase: ArchiveV2RetirementPhase::Authorized,
                category_index: 0,
                after_key: None,
                pending: Vec::new(),
                affected_families: Vec::new(),
                reclaim_initialized: false,
                reclaim_queue: Vec::new(),
                deleted_hot_rows: 0,
                deleted_cold_rows: 0,
                deleted_logical_bytes: 0,
                reclaimed_physical_bytes: 0,
            };
            store_retirement_journal(journal_path, &journal)?;
            journal
        };

        let started = Instant::now();
        let mut report = ArchiveV2RetirementPassReport {
            phase: journal.phase,
            categories_completed: journal.category_index as u64,
            ..ArchiveV2RetirementPassReport::default()
        };
        if journal.phase == ArchiveV2RetirementPhase::Complete
            || journal.phase == ArchiveV2RetirementPhase::ReclaimPending
        {
            return Ok(report);
        }
        if !journal.pending.is_empty() {
            self.apply_pending_retirement_deletions(&journal.pending, &mut report, fault)?;
            let last_key = journal
                .pending
                .iter()
                .map(|pending| pending.key.as_slice())
                .max()
                .map(ToOwned::to_owned);
            journal.after_key = last_key;
            journal.deleted_hot_rows = journal
                .deleted_hot_rows
                .saturating_add(report.deleted_hot_rows);
            journal.deleted_cold_rows = journal
                .deleted_cold_rows
                .saturating_add(report.deleted_cold_rows);
            journal.deleted_logical_bytes = journal
                .deleted_logical_bytes
                .saturating_add(report.deleted_logical_bytes);
            journal.pending.clear();
            journal.phase = ArchiveV2RetirementPhase::Tombstoning;
            store_retirement_journal(journal_path, &journal)?;
            maybe_retirement_fault(fault, ArchiveV2RetirementFaultPoint::AfterProgressJournal)?;
            report.recovered_pending_batch = true;
            report.phase = journal.phase;
            report.elapsed_millis = started.elapsed().as_millis() as u64;
            return Ok(report);
        }

        let categories = retirement.category_proofs();
        if journal.category_index as usize >= categories.len() {
            journal.phase = ArchiveV2RetirementPhase::ReclaimPending;
            store_retirement_journal(journal_path, &journal)?;
            report.phase = journal.phase;
            report.elapsed_millis = started.elapsed().as_millis() as u64;
            return Ok(report);
        }
        let category = &categories[journal.category_index as usize].category;
        report.category = Some(category.clone());
        let rows = reader
            .category_rows(category, segment.start_slot, segment.end_slot)
            .map_err(|error| error.to_string())?;
        let mut pending = Vec::new();
        let mut selected_rows = 0u64;
        let mut selected_bytes = 0u64;
        let mut final_key = None;
        for (key, expected_value) in rows {
            if journal
                .after_key
                .as_ref()
                .is_some_and(|after| key.as_slice() <= after.as_slice())
            {
                continue;
            }
            if selected_rows >= limits.max_rows
                || selected_bytes >= limits.max_bytes
                || started.elapsed() >= limits.max_wall_time
            {
                break;
            }
            report.scanned_rows = report.scanned_rows.saturating_add(1);
            let targets = self.prepare_retirement_row(category, &key, &expected_value)?;
            if targets.is_empty() {
                return Err(format!(
                    "Legacy {category} row {} disappeared before its authorized retirement batch",
                    hex::encode(&key)
                ));
            }
            selected_rows = selected_rows.saturating_add(1);
            selected_bytes =
                selected_bytes.saturating_add((key.len() + expected_value.len()) as u64);
            final_key = Some(key);
            pending.extend(targets);
        }
        if pending.len() > MAX_PENDING_DELETIONS {
            return Err("Archive V2 retirement pending batch is too large".to_string());
        }
        if pending.is_empty() {
            journal.category_index = journal.category_index.saturating_add(1);
            journal.after_key = None;
            journal.phase = if journal.category_index as usize >= categories.len() {
                ArchiveV2RetirementPhase::ReclaimPending
            } else {
                ArchiveV2RetirementPhase::Tombstoning
            };
            store_retirement_journal(journal_path, &journal)?;
            report.phase = journal.phase;
            report.categories_completed = journal.category_index as u64;
            report.elapsed_millis = started.elapsed().as_millis() as u64;
            return Ok(report);
        }

        journal.affected_families.extend(
            pending
                .iter()
                .map(retirement_family_for_deletion)
                .collect::<Result<Vec<_>, _>>()?,
        );
        journal.affected_families.sort();
        journal.affected_families.dedup();
        journal.pending = pending;
        journal.phase = ArchiveV2RetirementPhase::Tombstoning;
        store_retirement_journal(journal_path, &journal)?;
        maybe_retirement_fault(fault, ArchiveV2RetirementFaultPoint::AfterPendingJournal)?;
        self.apply_pending_retirement_deletions(&journal.pending, &mut report, fault)?;
        journal.after_key = final_key;
        journal.deleted_hot_rows = journal
            .deleted_hot_rows
            .saturating_add(report.deleted_hot_rows);
        journal.deleted_cold_rows = journal
            .deleted_cold_rows
            .saturating_add(report.deleted_cold_rows);
        journal.deleted_logical_bytes = journal
            .deleted_logical_bytes
            .saturating_add(report.deleted_logical_bytes);
        journal.pending.clear();
        store_retirement_journal(journal_path, &journal)?;
        maybe_retirement_fault(fault, ArchiveV2RetirementFaultPoint::AfterProgressJournal)?;
        report.phase = journal.phase;
        report.elapsed_millis = started.elapsed().as_millis() as u64;
        Ok(report)
    }

    pub fn reclaim_archive_v2_retirement_pass(
        &self,
        retirement: &ArchiveV2RetirementManifest,
        journal_path: &Path,
        limits: ArchiveV2RetirementReclaimLimits,
    ) -> Result<ArchiveV2RetirementReclaimReport, String> {
        let limits = limits.validate()?;
        let _guard = self
            .cold_migration_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        retirement.validate().map_err(|error| error.to_string())?;
        let encoded_manifest = retirement
            .encode_canonical()
            .map_err(|error| error.to_string())?;
        let manifest_hash = Hash::hash(&encoded_manifest);
        let mut journal = load_retirement_journal(journal_path)?;
        validate_retirement_journal(&journal, retirement, manifest_hash)?;
        if journal.phase != ArchiveV2RetirementPhase::ReclaimPending {
            if journal.phase == ArchiveV2RetirementPhase::Complete {
                return Ok(ArchiveV2RetirementReclaimReport {
                    phase: journal.phase,
                    total_reclaimed_physical_bytes: journal.reclaimed_physical_bytes,
                    ..ArchiveV2RetirementReclaimReport::default()
                });
            }
            return Err("Archive V2 retirement is not awaiting physical reclaim".to_string());
        }
        let reader = self
            .archive_v2_reader()
            .ok_or_else(|| "Archive V2 reader is not attached".to_string())?;
        if retirement.identity() != &reader.catalog().identity
            || retirement.catalog_root() != reader.catalog().catalog_root
        {
            return Err("Retirement manifest does not match the attached catalog".to_string());
        }
        reader
            .verify_segment(&retirement.segment_object_hash())
            .map_err(|error| error.to_string())?;

        if !journal.reclaim_initialized {
            journal.reclaim_queue = self.initialize_retirement_reclaim_queue(&journal, &reader)?;
            journal.reclaim_initialized = true;
            store_retirement_journal(journal_path, &journal)?;
        }

        let started = Instant::now();
        let mut report = ArchiveV2RetirementReclaimReport {
            phase: journal.phase,
            queued_ranges_before: journal.reclaim_queue.len() as u64,
            queued_ranges_after: journal.reclaim_queue.len() as u64,
            total_reclaimed_physical_bytes: journal.reclaimed_physical_bytes,
            ..ArchiveV2RetirementReclaimReport::default()
        };
        let mut hot_available_bytes = limits.hot_available_bytes;
        let mut cold_available_bytes = limits.cold_available_bytes;
        while report.compacted_ranges < limits.max_ranges {
            let Some(range) = journal.reclaim_queue.first().cloned() else {
                break;
            };
            let db = self.retirement_db(range.store)?;
            let estimate = retirement_estimated_reclaim_input_bytes(db, &range)?;
            let remaining_input = limits
                .max_estimated_input_bytes
                .saturating_sub(report.estimated_input_bytes);
            if estimate > remaining_input {
                report.paused_reason = Some(format!(
                    "compaction_input_budget:store={:?}:family={}:estimated_bytes={estimate}:remaining_bytes={remaining_input}",
                    range.store, range.cf_name
                ));
                break;
            }
            let (available_bytes, required_reserve_bytes) = match range.store {
                RetirementStore::Hot => {
                    (&mut hot_available_bytes, limits.hot_required_reserve_bytes)
                }
                RetirementStore::Cold => (
                    &mut cold_available_bytes,
                    limits.cold_required_reserve_bytes,
                ),
            };
            let estimated_peak = estimate.saturating_mul(2);
            if *available_bytes < required_reserve_bytes.saturating_add(estimated_peak) {
                report.paused_reason = Some(format!(
                    "compaction_headroom:store={:?}:family={}:available_bytes={}:reserve_bytes={required_reserve_bytes}:estimated_peak_bytes={estimated_peak}",
                    range.store, range.cf_name, *available_bytes
                ));
                break;
            }

            let cf = db
                .cf_handle(&range.cf_name)
                .ok_or_else(|| format!("{} retirement CF is missing", range.cf_name))?;
            let mut flush_options = FlushOptions::default();
            flush_options.set_wait(true);
            db.flush_cf_opt(&cf, &flush_options).map_err(|error| {
                format!(
                    "Failed flushing {:?} {} before retirement reclaim: {error}",
                    range.store, range.cf_name
                )
            })?;
            let refreshed_estimate = retirement_estimated_reclaim_input_bytes(db, &range)?;
            let refreshed_peak = refreshed_estimate.saturating_mul(2);
            let remaining_input = limits
                .max_estimated_input_bytes
                .saturating_sub(report.estimated_input_bytes);
            if refreshed_estimate > remaining_input
                || *available_bytes < required_reserve_bytes.saturating_add(refreshed_peak)
            {
                report.paused_reason = Some(format!(
                    "compaction_budget_after_flush:store={:?}:family={}:estimated_bytes={refreshed_estimate}:remaining_bytes={remaining_input}",
                    range.store, range.cf_name
                ));
                break;
            }

            let before = retirement_family_physical_bytes(db, &range.cf_name)?;
            let range_started = Instant::now();
            let mut options = CompactOptions::default();
            options.set_exclusive_manual_compaction(false);
            options.set_bottommost_level_compaction(BottommostLevelCompaction::ForceOptimized);
            db.compact_range_cf_opt(
                &cf,
                Some(range.start_key.as_slice()),
                Some(range.end_key.as_slice()),
                &options,
            );
            let after = retirement_family_physical_bytes(db, &range.cf_name)?;
            let reclaimed = before.saturating_sub(after);

            journal.reclaim_queue.remove(0);
            journal.reclaimed_physical_bytes =
                journal.reclaimed_physical_bytes.saturating_add(reclaimed);
            store_retirement_journal(journal_path, &journal)?;
            report.compacted_ranges = report.compacted_ranges.saturating_add(1);
            report.estimated_input_bytes = report
                .estimated_input_bytes
                .saturating_add(refreshed_estimate);
            report.reclaimed_physical_bytes =
                report.reclaimed_physical_bytes.saturating_add(reclaimed);
            report.compaction_duration_millis = report
                .compaction_duration_millis
                .saturating_add(range_started.elapsed().as_millis() as u64);
            *available_bytes = (*available_bytes).saturating_add(reclaimed);
        }
        if journal.reclaim_queue.is_empty() {
            journal.phase = ArchiveV2RetirementPhase::Complete;
            store_retirement_journal(journal_path, &journal)?;
        }
        report.phase = journal.phase;
        report.queued_ranges_after = journal.reclaim_queue.len() as u64;
        report.total_reclaimed_physical_bytes = journal.reclaimed_physical_bytes;
        if report.compaction_duration_millis == 0 {
            report.compaction_duration_millis = started.elapsed().as_millis() as u64;
        }
        Ok(report)
    }

    fn initialize_retirement_reclaim_queue(
        &self,
        journal: &ArchiveV2RetirementJournal,
        reader: &crate::archive_v2::ArchiveV2Reader,
    ) -> Result<Vec<RetirementReclaimRange>, String> {
        let mut ranges = std::collections::BTreeSet::new();
        for family in &journal.affected_families {
            let db = self.retirement_db(family.store)?;
            let cf = db
                .cf_handle(&family.cf_name)
                .ok_or_else(|| format!("{} retirement CF is missing", family.cf_name))?;
            let rows = reader
                .category_rows(&family.category, journal.start_slot, journal.end_slot)
                .map_err(|error| error.to_string())?;
            let keys = rows.into_iter().map(|(key, _)| key).collect::<Vec<_>>();
            for key in &keys {
                if db
                    .get_cf(&cf, key)
                    .map_err(|error| {
                        format!(
                            "Failed checking {:?} {} retirement tombstone: {error}",
                            family.store, family.cf_name
                        )
                    })?
                    .is_some()
                {
                    return Err(format!(
                        "Retired {:?} {} key {} reappeared before physical reclaim",
                        family.store,
                        family.cf_name,
                        hex::encode(key)
                    ));
                }
            }
            let mut flush_options = FlushOptions::default();
            flush_options.set_wait(true);
            db.flush_cf_opt(&cf, &flush_options).map_err(|error| {
                format!(
                    "Failed flushing {:?} {} retirement tombstones: {error}",
                    family.store, family.cf_name
                )
            })?;
            for file in db.live_files().map_err(|error| {
                format!(
                    "Failed inspecting {:?} SSTs for retirement reclaim: {error}",
                    family.store
                )
            })? {
                if file.column_family_name != family.cf_name || file.num_deletions == 0 {
                    continue;
                }
                let (Some(start_key), Some(end_key)) = (file.start_key, file.end_key) else {
                    return Err(format!(
                        "Refusing unbounded reclaim for {:?} {} SST {} without key bounds",
                        family.store, family.cf_name, file.name
                    ));
                };
                if sorted_keys_overlap_range(&keys, &start_key, &end_key) {
                    ranges.insert(RetirementReclaimRange {
                        store: family.store,
                        cf_name: family.cf_name.clone(),
                        start_key,
                        end_key: retirement_reclaim_range_end(&end_key),
                    });
                }
            }
        }
        if ranges.len() > MAX_RETIREMENT_RECLAIM_RANGES {
            return Err(format!(
                "Archive V2 retirement would queue {} SST ranges, exceeding the {}-range safety limit",
                ranges.len(),
                MAX_RETIREMENT_RECLAIM_RANGES
            ));
        }
        Ok(ranges.into_iter().collect())
    }

    fn retirement_db(&self, store: RetirementStore) -> Result<&DB, String> {
        match store {
            RetirementStore::Hot => Ok(self.db.as_ref()),
            RetirementStore::Cold => self
                .cold_db
                .as_deref()
                .ok_or_else(|| "Cold retirement target is not attached".to_string()),
        }
    }

    fn normalize_archive_v2_retirement_rows(
        &self,
        category: &str,
        rows: ArchiveV2Rows,
    ) -> Result<ArchiveV2Rows, String> {
        if category != "blocks" {
            return Ok(rows);
        }
        rows.into_iter()
            .map(|(key, value)| {
                self.canonical_archive_v2_retirement_value(category, &key, &value)
                    .map(|value| (key, value))
            })
            .collect()
    }

    fn verify_retirement_equivalence(
        &self,
        retirement: &ArchiveV2RetirementManifest,
        reader: &crate::archive_v2::ArchiveV2Reader,
    ) -> Result<(), String> {
        let (start_slot, end_slot) = retirement.slot_range();
        let raw_categories = self.archive_v2_public_categories(start_slot, end_slot)?;
        for proof in retirement.category_proofs() {
            if !PUBLIC_HISTORY_SNAPSHOT_CATEGORIES.contains(&proof.category.as_str()) {
                return Err(format!(
                    "Retirement manifest contains unsupported category {}",
                    proof.category
                ));
            }
            let archive_rows = reader
                .category_rows(&proof.category, start_slot, end_slot)
                .map_err(|error| error.to_string())?;
            let archive_rows =
                self.normalize_archive_v2_retirement_rows(&proof.category, archive_rows)?;
            let actual_proof =
                ArchiveV2CategoryProof::from_rows(proof.category.clone(), &archive_rows)
                    .map_err(|error| error.to_string())?;
            if &actual_proof != proof {
                return Err(format!(
                    "Retirement proof differs from Archive V2 category {}",
                    proof.category
                ));
            }
            let legacy_rows = if let Some(rows) = raw_categories.get(&proof.category) {
                rows.iter()
                    .map(|row| (row.key.clone(), row.value.clone()))
                    .collect()
            } else {
                self.archive_v2_legacy_category_rows(&proof.category, start_slot, end_slot)?
            };
            let legacy_rows =
                self.normalize_archive_v2_retirement_rows(&proof.category, legacy_rows)?;
            if archive_rows != legacy_rows {
                return Err(format!(
                    "Legacy/V2 category {} equivalence changed before retirement",
                    proof.category
                ));
            }
        }
        let proved = retirement
            .category_proofs()
            .iter()
            .map(|proof| proof.category.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let required = PUBLIC_HISTORY_SNAPSHOT_CATEGORIES
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        if proved != required {
            return Err("Retirement manifest does not prove every public-history category".into());
        }
        Ok(())
    }

    fn prepare_retirement_row(
        &self,
        category: &str,
        key: &[u8],
        expected_value: &[u8],
    ) -> Result<Vec<PendingRetirementDeletion>, String> {
        let hot_cf_name = retirement_hot_cf(category)?;
        let expected = self.canonical_archive_v2_retirement_value(category, key, expected_value)?;
        let expected_hash = Hash::hash(&expected);
        let mut targets = Vec::new();
        if let Some(cf) = self.db.cf_handle(hot_cf_name) {
            if let Some(value) = self
                .db
                .get_cf(&cf, key)
                .map_err(|error| format!("Failed reading hot {category}: {error}"))?
            {
                let actual = self.canonical_archive_v2_retirement_value(category, key, &value)?;
                if actual != expected {
                    return Err(format!(
                        "Hot {category} row {} conflicts with Archive V2",
                        hex::encode(key)
                    ));
                }
                targets.push(PendingRetirementDeletion {
                    store: RetirementStore::Hot,
                    category: category.to_string(),
                    key: key.to_vec(),
                    canonical_value_hash: expected_hash,
                    logical_bytes: (key.len() + value.len()) as u64,
                });
            }
        }
        if let (Some(cold), Some(cold_cf_name)) =
            (self.cold_db.as_ref(), retirement_cold_cf(category))
        {
            if let Some(cf) = cold.cf_handle(cold_cf_name) {
                if let Some(value) = cold
                    .get_cf(&cf, key)
                    .map_err(|error| format!("Failed reading cold {category}: {error}"))?
                {
                    let actual =
                        self.canonical_archive_v2_retirement_value(category, key, &value)?;
                    if actual != expected {
                        return Err(format!(
                            "Cold {category} row {} conflicts with Archive V2",
                            hex::encode(key)
                        ));
                    }
                    targets.push(PendingRetirementDeletion {
                        store: RetirementStore::Cold,
                        category: category.to_string(),
                        key: key.to_vec(),
                        canonical_value_hash: expected_hash,
                        logical_bytes: (key.len() + value.len()) as u64,
                    });
                }
            }
        }
        Ok(targets)
    }

    fn apply_pending_retirement_deletions(
        &self,
        pending: &[PendingRetirementDeletion],
        report: &mut ArchiveV2RetirementPassReport,
        fault: Option<ArchiveV2RetirementFaultPoint>,
    ) -> Result<(), String> {
        let mut hot_batch = WriteBatch::default();
        let mut cold_batch = WriteBatch::default();
        for deletion in pending {
            let (db, cf_name) = match deletion.store {
                RetirementStore::Hot => (self.db.as_ref(), retirement_hot_cf(&deletion.category)?),
                RetirementStore::Cold => {
                    let cold = self
                        .cold_db
                        .as_deref()
                        .ok_or_else(|| "Cold retirement target is not attached".to_string())?;
                    let cf_name = retirement_cold_cf(&deletion.category).ok_or_else(|| {
                        format!(
                            "Category {} has no cold retirement target",
                            deletion.category
                        )
                    })?;
                    (cold, cf_name)
                }
            };
            let cf = db
                .cf_handle(cf_name)
                .ok_or_else(|| format!("{cf_name} retirement CF is missing"))?;
            if let Some(value) = db
                .get_cf(&cf, &deletion.key)
                .map_err(|error| format!("Failed validating pending retirement: {error}"))?
            {
                let canonical = self.canonical_archive_v2_retirement_value(
                    &deletion.category,
                    &deletion.key,
                    &value,
                )?;
                if Hash::hash(&canonical) != deletion.canonical_value_hash {
                    return Err(format!(
                        "Pending retirement row {} changed after authorization",
                        hex::encode(&deletion.key)
                    ));
                }
                match deletion.store {
                    RetirementStore::Hot => hot_batch.delete_cf(&cf, &deletion.key),
                    RetirementStore::Cold => cold_batch.delete_cf(&cf, &deletion.key),
                }
                report.deleted_logical_bytes = report
                    .deleted_logical_bytes
                    .saturating_add(deletion.logical_bytes);
                match deletion.store {
                    RetirementStore::Hot => {
                        report.deleted_hot_rows = report.deleted_hot_rows.saturating_add(1)
                    }
                    RetirementStore::Cold => {
                        report.deleted_cold_rows = report.deleted_cold_rows.saturating_add(1)
                    }
                }
            }
        }
        let mut write_options = WriteOptions::default();
        write_options.set_sync(true);
        if report.deleted_hot_rows > 0 {
            self.db
                .write_opt(hot_batch, &write_options)
                .map_err(|error| format!("Failed deleting authorized hot history: {error}"))?;
        }
        maybe_retirement_fault(fault, ArchiveV2RetirementFaultPoint::AfterHotDeletion)?;
        if report.deleted_cold_rows > 0 {
            self.cold_db
                .as_ref()
                .ok_or_else(|| "Cold retirement target is not attached".to_string())?
                .write_opt(cold_batch, &write_options)
                .map_err(|error| format!("Failed deleting authorized cold history: {error}"))?;
        }
        maybe_retirement_fault(fault, ArchiveV2RetirementFaultPoint::AfterColdDeletion)?;
        Ok(())
    }
}

fn retirement_family_for_deletion(
    deletion: &PendingRetirementDeletion,
) -> Result<RetirementFamily, String> {
    let cf_name = match deletion.store {
        RetirementStore::Hot => retirement_hot_cf(&deletion.category)?,
        RetirementStore::Cold => retirement_cold_cf(&deletion.category).ok_or_else(|| {
            format!(
                "Category {} has no cold retirement target",
                deletion.category
            )
        })?,
    };
    Ok(RetirementFamily {
        store: deletion.store,
        category: deletion.category.clone(),
        cf_name: cf_name.to_string(),
    })
}

fn retirement_reclaim_range_end(key: &[u8]) -> Vec<u8> {
    let mut end = key.to_vec();
    for index in (0..end.len()).rev() {
        if end[index] != u8::MAX {
            end[index] += 1;
            end.truncate(index + 1);
            return end;
        }
    }
    end.push(0);
    end
}

fn sorted_keys_overlap_range(keys: &[Vec<u8>], start_key: &[u8], end_key: &[u8]) -> bool {
    let index = keys.partition_point(|key| key.as_slice() < start_key);
    keys.get(index).is_some_and(|key| key.as_slice() <= end_key)
}

fn retirement_estimated_reclaim_input_bytes(
    db: &DB,
    range: &RetirementReclaimRange,
) -> Result<u64, String> {
    let mut total = 0u64;
    for file in db
        .live_files()
        .map_err(|error| format!("Failed inspecting retirement reclaim SSTs: {error}"))?
    {
        if file.column_family_name != range.cf_name {
            continue;
        }
        let overlaps = match (file.start_key.as_deref(), file.end_key.as_deref()) {
            (Some(file_start), Some(file_end)) => {
                file_end >= range.start_key.as_slice() && file_start < range.end_key.as_slice()
            }
            _ => true,
        };
        if overlaps {
            total = total.saturating_add(file.size as u64);
        }
    }
    Ok(total)
}

fn retirement_family_physical_bytes(db: &DB, cf_name: &str) -> Result<u64, String> {
    let cf = db
        .cf_handle(cf_name)
        .ok_or_else(|| format!("{cf_name} retirement CF is missing"))?;
    let metadata = db.get_column_family_metadata_cf(&cf);
    let memtable = db
        .property_int_value_cf(&cf, rocksdb::properties::CUR_SIZE_ALL_MEM_TABLES)
        .map_err(|error| format!("Failed reading {cf_name} memtable size: {error}"))?
        .unwrap_or(0);
    Ok(metadata.size.saturating_add(memtable))
}

fn retirement_hot_cf(category: &str) -> Result<&'static str, String> {
    match category {
        "slots" => Ok(CF_SLOTS),
        "blocks" => Ok(CF_BLOCKS),
        "transactions" => Ok(CF_TRANSACTIONS),
        "tx_by_slot" => Ok(CF_TX_BY_SLOT),
        "tx_to_slot" => Ok(CF_TX_TO_SLOT),
        "tx_meta" => Ok(CF_TX_META),
        "account_txs" => Ok(CF_ACCOUNT_TXS),
        "events_by_slot" => Ok(CF_EVENTS_BY_SLOT),
        "events" => Ok(CF_EVENTS),
        "token_transfers" => Ok(CF_TOKEN_TRANSFERS),
        "program_calls" => Ok(CF_PROGRAM_CALLS),
        "evm_txs" => Ok(CF_EVM_TXS),
        "evm_receipts" => Ok(CF_EVM_RECEIPTS),
        "evm_logs_by_slot" => Ok(CF_EVM_LOGS_BY_SLOT),
        "shielded_txs" => Ok(CF_SHIELDED_TXS),
        "nft_activity" => Ok(CF_NFT_ACTIVITY),
        "market_activity" => Ok(CF_MARKET_ACTIVITY),
        "dex_trades_by_pair" => Ok(CF_DEX_TRADES_BY_PAIR),
        "dex_trades_by_taker" => Ok(CF_DEX_TRADES_BY_TAKER),
        "dex_trades_by_pair_taker" => Ok(CF_DEX_TRADES_BY_PAIR_TAKER),
        "account_snapshots" => Ok(CF_ACCOUNT_SNAPSHOTS),
        _ => Err(format!("Unsupported retirement category {category}")),
    }
}

fn retirement_cold_cf(category: &str) -> Option<&'static str> {
    match category {
        "blocks" => Some(COLD_CF_BLOCKS),
        "transactions" => Some(COLD_CF_TRANSACTIONS),
        "tx_to_slot" => Some(COLD_CF_TX_TO_SLOT),
        "account_txs" => Some(COLD_CF_ACCOUNT_TXS),
        "account_snapshots" => Some(COLD_CF_ACCOUNT_SNAPSHOTS),
        "events" => Some(COLD_CF_EVENTS),
        "token_transfers" => Some(COLD_CF_TOKEN_TRANSFERS),
        "program_calls" => Some(COLD_CF_PROGRAM_CALLS),
        _ => None,
    }
}

fn maybe_retirement_fault(
    requested: Option<ArchiveV2RetirementFaultPoint>,
    point: ArchiveV2RetirementFaultPoint,
) -> Result<(), String> {
    if requested == Some(point) {
        Err(format!("injected Archive V2 retirement fault at {point:?}"))
    } else {
        Ok(())
    }
}

fn validate_retirement_journal(
    journal: &ArchiveV2RetirementJournal,
    retirement: &ArchiveV2RetirementManifest,
    manifest_hash: Hash,
) -> Result<(), String> {
    if journal.journal_version != RETIREMENT_JOURNAL_VERSION
        || journal.retirement_manifest_hash != manifest_hash
        || journal.segment_object_hash != retirement.segment_object_hash()
        || (journal.start_slot, journal.end_slot) != retirement.slot_range()
        || journal.category_index as usize > retirement.category_proofs().len()
        || journal.pending.len() > MAX_PENDING_DELETIONS
        || journal.reclaim_queue.len() > MAX_RETIREMENT_RECLAIM_RANGES
        || (!journal.reclaim_initialized && !journal.reclaim_queue.is_empty())
        || (journal.phase == ArchiveV2RetirementPhase::Complete
            && (!journal.reclaim_initialized || !journal.reclaim_queue.is_empty()))
    {
        return Err("Archive V2 retirement journal is malformed or mismatched".to_string());
    }
    if !journal
        .affected_families
        .windows(2)
        .all(|window| window[0] < window[1])
        || !journal
            .reclaim_queue
            .windows(2)
            .all(|window| window[0] < window[1])
    {
        return Err("Archive V2 retirement journal ranges are not canonical".to_string());
    }
    for family in &journal.affected_families {
        let expected = match family.store {
            RetirementStore::Hot => retirement_hot_cf(&family.category)?,
            RetirementStore::Cold => retirement_cold_cf(&family.category).ok_or_else(|| {
                format!(
                    "Archive V2 retirement journal has unsupported cold category {}",
                    family.category
                )
            })?,
        };
        if family.cf_name != expected {
            return Err("Archive V2 retirement journal family mapping changed".to_string());
        }
    }
    let families = journal
        .affected_families
        .iter()
        .map(|family| (family.store, family.cf_name.as_str()))
        .collect::<std::collections::BTreeSet<_>>();
    if journal.reclaim_queue.iter().any(|range| {
        range.start_key >= range.end_key
            || !families.contains(&(range.store, range.cf_name.as_str()))
    }) {
        return Err("Archive V2 retirement journal contains an invalid reclaim range".to_string());
    }
    Ok(())
}

fn encode_retirement_journal(journal: &ArchiveV2RetirementJournal) -> Result<Vec<u8>, String> {
    let payload = serialize_legacy_bincode(journal, "Archive V2 retirement journal")?;
    if payload.len() > MAX_RETIREMENT_JOURNAL_BYTES {
        return Err("Archive V2 retirement journal is too large".to_string());
    }
    let mut encoded = Vec::with_capacity(RETIREMENT_JOURNAL_MAGIC.len() + 4 + payload.len() + 32);
    encoded.extend_from_slice(RETIREMENT_JOURNAL_MAGIC);
    encoded.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    encoded.extend_from_slice(&payload);
    encoded.extend_from_slice(&Hash::hash(&payload).0);
    Ok(encoded)
}

fn load_retirement_journal(path: &Path) -> Result<ArchiveV2RetirementJournal, String> {
    let encoded =
        fs::read(path).map_err(|error| format!("Failed reading {}: {error}", path.display()))?;
    let minimum = RETIREMENT_JOURNAL_MAGIC.len() + 4 + 32;
    if encoded.len() < minimum || !encoded.starts_with(RETIREMENT_JOURNAL_MAGIC) {
        return Err("Archive V2 retirement journal is truncated".to_string());
    }
    let offset = RETIREMENT_JOURNAL_MAGIC.len();
    let payload_len = u32::from_le_bytes(encoded[offset..offset + 4].try_into().unwrap()) as usize;
    if payload_len > MAX_RETIREMENT_JOURNAL_BYTES {
        return Err("Archive V2 retirement journal is too large".to_string());
    }
    let start = offset + 4;
    let end = start
        .checked_add(payload_len)
        .ok_or_else(|| "Archive V2 retirement journal length overflow".to_string())?;
    if end.checked_add(32) != Some(encoded.len())
        || Hash::hash(&encoded[start..end]).0 != encoded[end..]
    {
        return Err("Archive V2 retirement journal checksum mismatch".to_string());
    }
    deserialize_legacy_bincode_strict(
        &encoded[start..end],
        MAX_RETIREMENT_JOURNAL_BYTES as u64,
        "Archive V2 retirement journal",
    )
}

fn store_retirement_journal(
    path: &Path,
    journal: &ArchiveV2RetirementJournal,
) -> Result<(), String> {
    let encoded = encode_retirement_journal(journal)?;
    let parent = path
        .parent()
        .ok_or_else(|| "Archive V2 retirement journal has no parent".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Failed creating {}: {error}", parent.display()))?;
    let temporary = parent.join(format!(
        ".retirement-journal.{}.{}.tmp",
        std::process::id(),
        RETIREMENT_TEMPORARY_NONCE.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| format!("Failed creating {}: {error}", temporary.display()))?;
    let result = (|| {
        file.write_all(&encoded)
            .map_err(|error| format!("Failed writing retirement journal: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Failed syncing retirement journal: {error}"))?;
        drop(file);
        fs::rename(&temporary, path)
            .map_err(|error| format!("Failed publishing retirement journal: {error}"))?;
        OpenOptions::new()
            .read(true)
            .open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("Failed syncing retirement journal directory: {error}"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::{tempdir, TempDir};

    use super::*;
    use crate::archive_v2::{
        ArchiveV2Catalog, ArchiveV2CodecConfig, ArchiveV2DirectorySource, ArchiveV2Identity,
        ArchiveV2Reader, ArchiveV2ReaderConfig, ArchiveV2SegmentCodec, ArchiveV2SegmentContents,
        ARCHIVE_V2_FORMAT_VERSION,
    };
    use crate::{Block, CommitSignature, Keypair, PqPublicKey, PqSignature};

    struct RetirementFixture {
        _state_root: TempDir,
        _cold_root: TempDir,
        archive_root: TempDir,
        journal_root: TempDir,
        state: StateStore,
        block_hash: Hash,
        retirement: ArchiveV2RetirementManifest,
    }

    fn fixture() -> RetirementFixture {
        let state_root = tempdir().unwrap();
        let cold_root = tempdir().unwrap();
        let archive_root = tempdir().unwrap();
        let journal_root = tempdir().unwrap();
        let mut state = StateStore::open(state_root.path()).unwrap();
        state.open_cold_store(cold_root.path()).unwrap();
        let mut block = Block::new_with_timestamp(
            0,
            Hash::default(),
            Hash::hash(b"retirement-state"),
            [9; 32],
            Vec::new(),
            1,
        );
        block.commit_round = 7;
        block.commit_signatures.push(CommitSignature {
            validator: [6; 32],
            signature: PqSignature {
                scheme_version: 1,
                public_key: PqPublicKey {
                    scheme_version: 1,
                    bytes: vec![7; 32],
                },
                sig: vec![8; 64],
            },
            timestamp: 2,
        });
        let block_hash = block.hash();
        state.put_block_atomic(&block, Some(0), Some(0)).unwrap();
        state.set_last_slot(COLD_RETENTION_SLOTS + 10).unwrap();

        let hot_blocks = state.db.cf_handle(CF_BLOCKS).unwrap();
        let block_bytes = state.db.get_cf(&hot_blocks, block_hash.0).unwrap().unwrap();
        let cold = state.cold_db.as_ref().unwrap();
        let cold_blocks = cold.cf_handle(COLD_CF_BLOCKS).unwrap();
        cold.put_cf(&cold_blocks, block_hash.0, &block_bytes)
            .unwrap();

        let identity = ArchiveV2Identity {
            network_id: "retirement-testnet".to_string(),
            genesis_hash: block_hash,
        };
        let contents = ArchiveV2SegmentContents {
            blocks: vec![block],
            public_categories: state.archive_v2_public_categories(0, 0).unwrap(),
        };
        let (segment_bytes, segment_manifest) = ArchiveV2SegmentCodec::encode(
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
        let objects = archive_root.path().join("objects");
        fs::create_dir_all(&objects).unwrap();
        fs::write(
            objects.join(format!("{}.av2s", segment_manifest.segment_object_hash)),
            segment_bytes,
        )
        .unwrap();
        let mut catalog = ArchiveV2Catalog::empty(identity.clone()).unwrap();
        catalog.append(segment_manifest.clone()).unwrap();
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
                    max_decoded_segments: 2,
                    allow_remote_fetch: false,
                    sources: vec![std::sync::Arc::new(ArchiveV2DirectorySource::new(
                        "unused",
                        archive_root.path(),
                        true,
                    ))],
                },
            )
            .unwrap(),
        );
        let replica_evidence = vec![
            ArchiveV2ReplicaEvidence {
                destination: "provider-a".to_string(),
                failure_domain: "region-a".to_string(),
                segment_object_hash: segment_manifest.segment_object_hash,
                verified_unix_seconds: 1,
            },
            ArchiveV2ReplicaEvidence {
                destination: "provider-b".to_string(),
                failure_domain: "region-b".to_string(),
                segment_object_hash: segment_manifest.segment_object_hash,
                verified_unix_seconds: 1,
            },
        ];
        let request = state
            .prepare_archive_v2_retirement_request(
                segment_manifest.segment_object_hash,
                replica_evidence,
                2,
                2,
                ArchiveV2RollbackAnchor {
                    release_tag: "v0.6.0".to_string(),
                    release_commit: "b".repeat(40),
                    artifact_sha256: Hash::hash(b"artifact"),
                    detached_pq_checksum_signature_sha256: Hash::hash(b"pq"),
                    archive_format_version: ARCHIVE_V2_FORMAT_VERSION,
                    catalog_format_version: crate::archive_v2::ARCHIVE_V2_CATALOG_VERSION,
                    deployed_validator_count: 4,
                    activated_unix_seconds: 1,
                },
                1,
            )
            .unwrap();
        let retirement =
            ArchiveV2RetirementManifest::sign(request, &Keypair::from_seed(&[3; 32])).unwrap();

        RetirementFixture {
            _state_root: state_root,
            _cold_root: cold_root,
            archive_root,
            journal_root,
            state,
            block_hash,
            retirement,
        }
    }

    fn reclaim_limits() -> ArchiveV2RetirementReclaimLimits {
        ArchiveV2RetirementReclaimLimits {
            max_ranges: 16,
            max_estimated_input_bytes: 4 * 1024 * 1024 * 1024,
            hot_available_bytes: u64::MAX,
            hot_required_reserve_bytes: 0,
            cold_available_bytes: u64::MAX,
            cold_required_reserve_bytes: 0,
        }
    }

    #[test]
    fn retirement_reclaim_limit_accepts_large_bounded_sst_input() {
        assert!(ArchiveV2RetirementReclaimLimits {
            max_ranges: 1,
            max_estimated_input_bytes: 8 * 1024 * 1024 * 1024,
            hot_available_bytes: u64::MAX,
            hot_required_reserve_bytes: 0,
            cold_available_bytes: u64::MAX,
            cold_required_reserve_bytes: 0,
        }
        .validate()
        .is_ok());
        assert!(ArchiveV2RetirementReclaimLimits {
            max_ranges: 1,
            max_estimated_input_bytes: 8 * 1024 * 1024 * 1024 + 1,
            hot_available_bytes: u64::MAX,
            hot_required_reserve_bytes: 0,
            cold_available_bytes: u64::MAX,
            cold_required_reserve_bytes: 0,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn retirement_resumes_every_delete_boundary_without_losing_v2_reads() {
        for fault in [
            ArchiveV2RetirementFaultPoint::AfterPendingJournal,
            ArchiveV2RetirementFaultPoint::AfterHotDeletion,
            ArchiveV2RetirementFaultPoint::AfterColdDeletion,
            ArchiveV2RetirementFaultPoint::AfterProgressJournal,
        ] {
            let fixture = fixture();
            let journal = fixture
                .journal_root
                .path()
                .join(format!("retirement-{fault:?}.journal"));
            let mut injected = false;
            for _ in 0..64 {
                match fixture.state.retire_archive_v2_segment_pass_faulted(
                    &fixture.retirement,
                    &journal,
                    ArchiveV2RetirementLimits::default(),
                    fault,
                ) {
                    Ok(report) => {
                        if report.phase == ArchiveV2RetirementPhase::ReclaimPending {
                            break;
                        }
                    }
                    Err(error) if error.contains("injected Archive V2 retirement fault") => {
                        injected = true;
                        break;
                    }
                    Err(error) => panic!("unexpected retirement error: {error}"),
                }
            }
            assert!(injected);

            let mut final_report = ArchiveV2RetirementPassReport::default();
            for _ in 0..128 {
                final_report = fixture
                    .state
                    .retire_archive_v2_segment_pass(
                        &fixture.retirement,
                        &journal,
                        ArchiveV2RetirementLimits::default(),
                    )
                    .unwrap();
                if final_report.phase == ArchiveV2RetirementPhase::ReclaimPending {
                    break;
                }
            }
            assert_eq!(final_report.phase, ArchiveV2RetirementPhase::ReclaimPending);
            assert_eq!(
                fixture
                    .state
                    .get_block(&fixture.block_hash)
                    .unwrap()
                    .unwrap()
                    .hash(),
                fixture.block_hash
            );
            assert!(!fixture
                .archive_root
                .path()
                .join("quarantine")
                .read_dir()
                .unwrap()
                .any(|_| true));
            let mut reclaim = ArchiveV2RetirementReclaimReport::default();
            for _ in 0..128 {
                reclaim = fixture
                    .state
                    .reclaim_archive_v2_retirement_pass(
                        &fixture.retirement,
                        &journal,
                        reclaim_limits(),
                    )
                    .unwrap();
                if reclaim.phase == ArchiveV2RetirementPhase::Complete {
                    break;
                }
            }
            assert_eq!(reclaim.phase, ArchiveV2RetirementPhase::Complete);
            assert_eq!(
                fixture
                    .state
                    .reclaim_archive_v2_retirement_pass(
                        &fixture.retirement,
                        &journal,
                        reclaim_limits(),
                    )
                    .unwrap()
                    .total_reclaimed_physical_bytes,
                reclaim.total_reclaimed_physical_bytes
            );
        }
    }

    #[test]
    fn retirement_journal_corruption_fails_closed() {
        let fixture = fixture();
        let journal = fixture.journal_root.path().join("corrupt.journal");
        for _ in 0..64 {
            if fixture
                .state
                .retire_archive_v2_segment_pass_faulted(
                    &fixture.retirement,
                    &journal,
                    ArchiveV2RetirementLimits::default(),
                    ArchiveV2RetirementFaultPoint::AfterPendingJournal,
                )
                .is_err()
            {
                break;
            }
        }
        let mut bytes = fs::read(&journal).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        fs::write(&journal, bytes).unwrap();
        assert!(fixture
            .state
            .retire_archive_v2_segment_pass(
                &fixture.retirement,
                &journal,
                ArchiveV2RetirementLimits::default(),
            )
            .unwrap_err()
            .contains("checksum"));
    }
}
