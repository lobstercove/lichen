use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rocksdb::{
    BottommostLevelCompaction, CompactOptions, FlushOptions, LiveFile, WriteBatch, WriteOptions, DB,
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
const RETIREMENT_MULTI_GET_ROWS: usize = 4_096;
const RETIREMENT_MULTI_GET_BYTES: usize = 64 * 1024 * 1024;
const MAX_RETIREMENT_RECLAIM_RANGES: usize = 4_096;
const MAX_RETIREMENT_RECLAIM_INPUT_BYTES: u64 = 32 * 1024 * 1024 * 1024;
const MAX_RETIREMENT_RECLAIM_SPLITS_PER_PASS: u64 = 64;
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
                "Archive V2 retirement reclaim max_estimated_input_bytes must be in 1 MiB..=32 GiB"
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
    pub split_ranges: u64,
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
    pub skipped_absent_rebuildable_rows: u64,
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
        replica_evidence: Vec<ArchiveV2ReplicaEvidence>,
        required_replica_count: u16,
        required_failure_domains: u16,
        rollback_anchor: ArchiveV2RollbackAnchor,
        authorized_unix_seconds: u64,
    ) -> Result<ArchiveV2RetirementRequest, String> {
        self.prepare_archive_v2_retirement_request_for_window(
            segment_object_hash,
            None,
            replica_evidence,
            required_replica_count,
            required_failure_domains,
            rollback_anchor,
            authorized_unix_seconds,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare_archive_v2_retirement_window_request(
        &self,
        segment_object_hash: Hash,
        start_slot: u64,
        end_slot: u64,
        replica_evidence: Vec<ArchiveV2ReplicaEvidence>,
        required_replica_count: u16,
        required_failure_domains: u16,
        rollback_anchor: ArchiveV2RollbackAnchor,
        authorized_unix_seconds: u64,
    ) -> Result<ArchiveV2RetirementRequest, String> {
        self.prepare_archive_v2_retirement_request_for_window(
            segment_object_hash,
            Some((start_slot, end_slot)),
            replica_evidence,
            required_replica_count,
            required_failure_domains,
            rollback_anchor,
            authorized_unix_seconds,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_archive_v2_retirement_request_for_window(
        &self,
        segment_object_hash: Hash,
        slot_window: Option<(u64, u64)>,
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
        let segment_range = (segment_manifest.start_slot, segment_manifest.end_slot);
        let (start_slot, end_slot) = slot_window.unwrap_or(segment_range);
        if !retirement_range_within_segment(segment_range, (start_slot, end_slot)) {
            return Err(format!(
                "Archive V2 retirement window {start_slot}..{end_slot} is outside segment {}..{}",
                segment_range.0, segment_range.1
            ));
        }

        let mut category_proofs = Vec::with_capacity(PUBLIC_HISTORY_SNAPSHOT_CATEGORIES.len());
        for category in PUBLIC_HISTORY_SNAPSHOT_CATEGORIES {
            // Authorization binds the signed request to the verified Archive V2
            // rows and replica evidence. The first destructive pass separately
            // point-checks every one of these rows against hot/cold state before
            // it creates a journal or deletes anything.
            let archive_rows =
                self.archive_v2_retirement_rows(&reader, category, start_slot, end_slot)?;
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
            start_slot,
            end_slot,
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
            || !retirement_range_within_segment(
                (segment.start_slot, segment.end_slot),
                retirement.slot_range(),
            )
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
                start_slot: retirement.slot_range().0,
                end_slot: retirement.slot_range().1,
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
        let mut selected_rows = 0u64;
        let mut selected_bytes = 0u64;
        if journal.phase == ArchiveV2RetirementPhase::Complete
            || journal.phase == ArchiveV2RetirementPhase::ReclaimPending
        {
            return Ok(report);
        }
        if !journal.pending.is_empty() {
            let mut batch_report = ArchiveV2RetirementPassReport::default();
            self.apply_pending_retirement_deletions(&journal.pending, &mut batch_report, fault)?;
            let last_key = journal
                .pending
                .iter()
                .map(|pending| pending.key.as_slice())
                .max()
                .map(ToOwned::to_owned);
            journal.after_key = last_key;
            journal.deleted_hot_rows = journal
                .deleted_hot_rows
                .saturating_add(batch_report.deleted_hot_rows);
            journal.deleted_cold_rows = journal
                .deleted_cold_rows
                .saturating_add(batch_report.deleted_cold_rows);
            journal.deleted_logical_bytes = journal
                .deleted_logical_bytes
                .saturating_add(batch_report.deleted_logical_bytes);
            journal.pending.clear();
            journal.phase = ArchiveV2RetirementPhase::Tombstoning;
            store_retirement_journal(journal_path, &journal)?;
            maybe_retirement_fault(fault, ArchiveV2RetirementFaultPoint::AfterProgressJournal)?;
            report.deleted_hot_rows = batch_report.deleted_hot_rows;
            report.deleted_cold_rows = batch_report.deleted_cold_rows;
            report.deleted_logical_bytes = batch_report.deleted_logical_bytes;
            report.recovered_pending_batch = true;
            report.phase = journal.phase;
            report.elapsed_millis = started.elapsed().as_millis() as u64;
            return Ok(report);
        }

        let categories = retirement.category_proofs();
        loop {
            if journal.category_index as usize >= categories.len() {
                journal.phase = ArchiveV2RetirementPhase::ReclaimPending;
                store_retirement_journal(journal_path, &journal)?;
                report.phase = journal.phase;
                report.categories_completed = journal.category_index as u64;
                report.elapsed_millis = started.elapsed().as_millis() as u64;
                return Ok(report);
            }
            let category = &categories[journal.category_index as usize].category;
            report.category = Some(category.clone());
            let rows = reader
                .category_rows(category, journal.start_slot, journal.end_slot)
                .map_err(|error| error.to_string())?;
            let mut pending = Vec::new();
            let mut final_key = None;
            let mut exhausted = false;
            let mut rows = rows
                .into_iter()
                .filter(|(key, _)| {
                    journal
                        .after_key
                        .as_ref()
                        .is_none_or(|after| key.as_slice() > after.as_slice())
                })
                .peekable();
            loop {
                if rows.peek().is_none() {
                    exhausted = true;
                    break;
                }

                let mut chunk = Vec::with_capacity(RETIREMENT_MULTI_GET_ROWS);
                let mut chunk_bytes = 0usize;
                let mut planned_rows = selected_rows;
                let mut planned_bytes = selected_bytes;
                while chunk.len() < RETIREMENT_MULTI_GET_ROWS {
                    if planned_rows >= limits.max_rows
                        || planned_bytes >= limits.max_bytes
                        || started.elapsed() >= limits.max_wall_time
                    {
                        break;
                    }
                    let Some((key, expected_value)) = rows.peek() else {
                        break;
                    };
                    let row_bytes = key.len().saturating_add(expected_value.len());
                    if !chunk.is_empty()
                        && chunk_bytes.saturating_add(row_bytes) > RETIREMENT_MULTI_GET_BYTES
                    {
                        break;
                    }
                    let Some((key, expected_value)) = rows.next() else {
                        break;
                    };
                    chunk_bytes = chunk_bytes.saturating_add(row_bytes);
                    planned_rows = planned_rows.saturating_add(1);
                    planned_bytes =
                        planned_bytes.saturating_add((key.len() + expected_value.len()) as u64);
                    chunk.push((key, expected_value));
                }
                if chunk.is_empty() {
                    break;
                }

                let prepared = self.prepare_retirement_rows(category, &chunk)?;
                for ((key, expected_value), targets) in chunk.into_iter().zip(prepared) {
                    report.scanned_rows = report.scanned_rows.saturating_add(1);
                    if targets.is_empty() {
                        if !retirement_allows_absent_source_row(category) {
                            return Err(format!(
                                "Legacy {category} row {} disappeared before its authorized retirement batch",
                                hex::encode(&key)
                            ));
                        }
                        report.skipped_absent_rebuildable_rows =
                            report.skipped_absent_rebuildable_rows.saturating_add(1);
                    }
                    selected_rows = selected_rows.saturating_add(1);
                    selected_bytes =
                        selected_bytes.saturating_add((key.len() + expected_value.len()) as u64);
                    final_key = Some(key);
                    pending.extend(targets);
                }
            }
            if pending.len() > MAX_PENDING_DELETIONS {
                return Err("Archive V2 retirement pending batch is too large".to_string());
            }
            if pending.is_empty() {
                if !exhausted {
                    journal.after_key = final_key;
                    journal.phase = ArchiveV2RetirementPhase::Tombstoning;
                    store_retirement_journal(journal_path, &journal)?;
                    report.phase = journal.phase;
                    report.elapsed_millis = started.elapsed().as_millis() as u64;
                    return Ok(report);
                }
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
                if journal.phase == ArchiveV2RetirementPhase::ReclaimPending
                    || selected_rows >= limits.max_rows
                    || selected_bytes >= limits.max_bytes
                    || started.elapsed() >= limits.max_wall_time
                {
                    report.elapsed_millis = started.elapsed().as_millis() as u64;
                    return Ok(report);
                }
                continue;
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
            let mut batch_report = ArchiveV2RetirementPassReport::default();
            self.apply_pending_retirement_deletions(&journal.pending, &mut batch_report, fault)?;
            journal.after_key = final_key;
            journal.deleted_hot_rows = journal
                .deleted_hot_rows
                .saturating_add(batch_report.deleted_hot_rows);
            journal.deleted_cold_rows = journal
                .deleted_cold_rows
                .saturating_add(batch_report.deleted_cold_rows);
            journal.deleted_logical_bytes = journal
                .deleted_logical_bytes
                .saturating_add(batch_report.deleted_logical_bytes);
            journal.pending.clear();
            store_retirement_journal(journal_path, &journal)?;
            maybe_retirement_fault(fault, ArchiveV2RetirementFaultPoint::AfterProgressJournal)?;
            report.deleted_hot_rows = report
                .deleted_hot_rows
                .saturating_add(batch_report.deleted_hot_rows);
            report.deleted_cold_rows = report
                .deleted_cold_rows
                .saturating_add(batch_report.deleted_cold_rows);
            report.deleted_logical_bytes = report
                .deleted_logical_bytes
                .saturating_add(batch_report.deleted_logical_bytes);
            report.phase = journal.phase;

            if exhausted {
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
            }
            if report.phase == ArchiveV2RetirementPhase::ReclaimPending
                || selected_rows >= limits.max_rows
                || selected_bytes >= limits.max_bytes
                || started.elapsed() >= limits.max_wall_time
            {
                report.elapsed_millis = started.elapsed().as_millis() as u64;
                return Ok(report);
            }
        }
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
            if journal.reclaim_queue.is_empty() {
                break;
            }
            let remaining_input = limits
                .max_estimated_input_bytes
                .saturating_sub(report.estimated_input_bytes);
            let (candidate, paused_reason) = select_retirement_reclaim_candidate(
                &journal.reclaim_queue,
                remaining_input,
                hot_available_bytes,
                limits.hot_required_reserve_bytes,
                cold_available_bytes,
                limits.cold_required_reserve_bytes,
                |range| {
                    retirement_estimated_reclaim_input_bytes(
                        self.retirement_db(range.store)?,
                        range,
                    )
                },
            )?;
            let Some((range_index, range, _estimate)) = candidate else {
                // A fresh pass that cannot admit any queued range may be
                // blocked only because one SST-derived range spans too many
                // lower-level files. Split it at a real live-file boundary;
                // this preserves the exact covered keyspace and journal
                // compatibility while giving RocksDB a bounded compaction
                // interval. Never fragment a range merely because this pass
                // has already consumed its budget; the next invocation gets a
                // fresh budget and may admit it unchanged.
                if report.estimated_input_bytes == 0
                    && report.split_ranges < MAX_RETIREMENT_RECLAIM_SPLITS_PER_PASS
                    && journal.reclaim_queue.len() < MAX_RETIREMENT_RECLAIM_RANGES
                {
                    let mut split = None;
                    for (index, queued) in journal.reclaim_queue.iter().enumerate() {
                        let db = self.retirement_db(queued.store)?;
                        let live_files = db.live_files().map_err(|error| {
                            format!(
                                "Failed inspecting {:?} SSTs for retirement reclaim split: {error}",
                                queued.store
                            )
                        })?;
                        if let Some((left, right)) =
                            split_retirement_reclaim_range(queued, &live_files)
                        {
                            split = Some((index, left, right));
                            break;
                        }
                    }
                    if let Some((index, left, right)) = split {
                        journal.reclaim_queue.remove(index);
                        journal.reclaim_queue.push(left);
                        journal.reclaim_queue.push(right);
                        journal.reclaim_queue.sort();
                        journal.reclaim_queue.dedup();
                        store_retirement_journal(journal_path, &journal)?;
                        report.split_ranges = report.split_ranges.saturating_add(1);
                        report.queued_ranges_after = journal.reclaim_queue.len() as u64;
                        continue;
                    }
                }
                report.paused_reason = if report.split_ranges
                    >= MAX_RETIREMENT_RECLAIM_SPLITS_PER_PASS
                {
                    Some(format!(
                        "reclaim_split_limit:split_ranges={}:limit={MAX_RETIREMENT_RECLAIM_SPLITS_PER_PASS}",
                        report.split_ranges
                    ))
                } else {
                    paused_reason
                };
                break;
            };
            let db = self.retirement_db(range.store)?;
            let (available_bytes, required_reserve_bytes) = match range.store {
                RetirementStore::Hot => {
                    (&mut hot_available_bytes, limits.hot_required_reserve_bytes)
                }
                RetirementStore::Cold => (
                    &mut cold_available_bytes,
                    limits.cold_required_reserve_bytes,
                ),
            };

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

            journal.reclaim_queue.remove(range_index);
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
        for proof in retirement.category_proofs() {
            if !PUBLIC_HISTORY_SNAPSHOT_CATEGORIES.contains(&proof.category.as_str()) {
                return Err(format!(
                    "Retirement manifest contains unsupported category {}",
                    proof.category
                ));
            }
            let archive_rows = self.verify_archive_v2_retirement_rows(
                reader,
                &proof.category,
                start_slot,
                end_slot,
            )?;
            let actual_proof =
                ArchiveV2CategoryProof::from_rows(proof.category.clone(), &archive_rows)
                    .map_err(|error| error.to_string())?;
            if &actual_proof != proof {
                return Err(format!(
                    "Retirement proof differs from Archive V2 category {}",
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

    fn verify_archive_v2_retirement_rows(
        &self,
        reader: &crate::archive_v2::ArchiveV2Reader,
        category: &str,
        start_slot: u64,
        end_slot: u64,
    ) -> Result<ArchiveV2Rows, String> {
        let archive_rows =
            self.archive_v2_retirement_rows(reader, category, start_slot, end_slot)?;
        let mut offset = 0;
        while offset < archive_rows.len() {
            let end = retirement_multi_get_chunk_end(&archive_rows, offset, |(key, value)| {
                key.len().saturating_add(value.len())
            });
            let rows = &archive_rows[offset..end];
            // Only rows represented by this verified segment are candidates for
            // deletion. Bounded multi-gets fail on conflicting source data and on
            // missing canonical data while avoiding one remote RocksDB lookup per
            // row. A missing deterministic secondary index needs no tombstone once
            // its canonical block and transaction rows have passed this complete
            // manifest verification.
            let prepared = self.prepare_retirement_rows(category, rows)?;
            for ((key, _), targets) in rows.iter().zip(prepared) {
                if targets.is_empty() && !retirement_allows_absent_source_row(category) {
                    return Err(format!(
                        "Archive V2 retirement source row {} is absent from hot and cold {category}",
                        hex::encode(key)
                    ));
                }
            }
            offset = end;
        }
        Ok(archive_rows)
    }

    fn archive_v2_retirement_rows(
        &self,
        reader: &crate::archive_v2::ArchiveV2Reader,
        category: &str,
        start_slot: u64,
        end_slot: u64,
    ) -> Result<ArchiveV2Rows, String> {
        let archive_rows = reader
            .category_rows(category, start_slot, end_slot)
            .map_err(|error| error.to_string())?;
        self.normalize_archive_v2_retirement_rows(category, archive_rows)
    }

    fn prepare_retirement_rows(
        &self,
        category: &str,
        rows: &[(Vec<u8>, Vec<u8>)],
    ) -> Result<Vec<Vec<PendingRetirementDeletion>>, String> {
        if retirement_multi_get_chunk_end(rows, 0, |(key, value)| {
            key.len().saturating_add(value.len())
        }) != rows.len()
        {
            return Err(format!(
                "Archive V2 retirement multi-get exceeds its {RETIREMENT_MULTI_GET_ROWS}-row or {}-byte safety bound",
                RETIREMENT_MULTI_GET_BYTES
            ));
        }
        let hot_cf_name = retirement_hot_cf(category)?;
        let expected = rows
            .iter()
            .map(|(key, value)| self.canonical_archive_v2_retirement_value(category, key, value))
            .collect::<Result<Vec<_>, _>>()?;
        let expected_hashes = expected
            .iter()
            .map(|value| Hash::hash(value))
            .collect::<Vec<_>>();
        let mut targets = vec![Vec::new(); rows.len()];
        if let Some(cf) = self.db.cf_handle(hot_cf_name) {
            let values = self.db.batched_multi_get_cf(
                &cf,
                rows.iter().map(|(key, _)| key.as_slice()),
                false,
            );
            for (index, value) in values.into_iter().enumerate() {
                if let Some(value) =
                    value.map_err(|error| format!("Failed reading hot {category}: {error}"))?
                {
                    let (key, _) = &rows[index];
                    let actual =
                        self.canonical_archive_v2_retirement_value(category, key, &value)?;
                    if actual != expected[index] {
                        return Err(format!(
                            "Hot {category} row {} conflicts with Archive V2",
                            hex::encode(key)
                        ));
                    }
                    targets[index].push(PendingRetirementDeletion {
                        store: RetirementStore::Hot,
                        category: category.to_string(),
                        key: key.clone(),
                        canonical_value_hash: expected_hashes[index],
                        logical_bytes: (key.len() + value.len()) as u64,
                    });
                }
            }
        }
        if let (Some(cold), Some(cold_cf_name)) =
            (self.cold_db.as_ref(), retirement_cold_cf(category))
        {
            if let Some(cf) = cold.cf_handle(cold_cf_name) {
                let values = cold.batched_multi_get_cf(
                    &cf,
                    rows.iter().map(|(key, _)| key.as_slice()),
                    false,
                );
                for (index, value) in values.into_iter().enumerate() {
                    if let Some(value) =
                        value.map_err(|error| format!("Failed reading cold {category}: {error}"))?
                    {
                        let (key, _) = &rows[index];
                        let actual =
                            self.canonical_archive_v2_retirement_value(category, key, &value)?;
                        if actual != expected[index] {
                            return Err(format!(
                                "Cold {category} row {} conflicts with Archive V2",
                                hex::encode(key)
                            ));
                        }
                        targets[index].push(PendingRetirementDeletion {
                            store: RetirementStore::Cold,
                            category: category.to_string(),
                            key: key.clone(),
                            canonical_value_hash: expected_hashes[index],
                            logical_bytes: (key.len() + value.len()) as u64,
                        });
                    }
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
        for store in [RetirementStore::Hot, RetirementStore::Cold] {
            let deletions = pending
                .iter()
                .filter(|deletion| deletion.store == store)
                .collect::<Vec<_>>();
            if deletions.is_empty() {
                continue;
            }
            let db = self.retirement_db(store)?;
            let category = &deletions[0].category;
            if deletions
                .iter()
                .any(|deletion| deletion.category != *category)
            {
                return Err(
                    "Archive V2 retirement pending batch spans multiple categories".to_string(),
                );
            }
            let cf_name = match store {
                RetirementStore::Hot => retirement_hot_cf(category)?,
                RetirementStore::Cold => retirement_cold_cf(category)
                    .ok_or_else(|| format!("Category {category} has no cold retirement target"))?,
            };
            let cf = db
                .cf_handle(cf_name)
                .ok_or_else(|| format!("{cf_name} retirement CF is missing"))?;
            let mut offset = 0;
            while offset < deletions.len() {
                let end = retirement_multi_get_chunk_end(&deletions, offset, |deletion| {
                    usize::try_from(deletion.logical_bytes).unwrap_or(usize::MAX)
                });
                let deletions = &deletions[offset..end];
                let values = db.batched_multi_get_cf(
                    &cf,
                    deletions.iter().map(|deletion| deletion.key.as_slice()),
                    false,
                );
                for (deletion, value) in deletions.iter().zip(values) {
                    if let Some(value) = value
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
                        match store {
                            RetirementStore::Hot => hot_batch.delete_cf(&cf, &deletion.key),
                            RetirementStore::Cold => cold_batch.delete_cf(&cf, &deletion.key),
                        }
                        report.deleted_logical_bytes = report
                            .deleted_logical_bytes
                            .saturating_add(deletion.logical_bytes);
                        match store {
                            RetirementStore::Hot => {
                                report.deleted_hot_rows = report.deleted_hot_rows.saturating_add(1)
                            }
                            RetirementStore::Cold => {
                                report.deleted_cold_rows =
                                    report.deleted_cold_rows.saturating_add(1)
                            }
                        }
                    }
                }
                offset = end;
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

fn retirement_multi_get_chunk_end<T>(
    items: &[T],
    start: usize,
    logical_bytes: impl Fn(&T) -> usize,
) -> usize {
    let mut end = start;
    let mut bytes = 0usize;
    while end < items.len() && end.saturating_sub(start) < RETIREMENT_MULTI_GET_ROWS {
        let item_bytes = logical_bytes(&items[end]);
        if end > start && bytes.saturating_add(item_bytes) > RETIREMENT_MULTI_GET_BYTES {
            break;
        }
        bytes = bytes.saturating_add(item_bytes);
        end += 1;
    }
    end
}

fn retirement_range_within_segment(segment: (u64, u64), retirement: (u64, u64)) -> bool {
    retirement.0 <= retirement.1 && retirement.0 >= segment.0 && retirement.1 <= segment.1
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
    let files = db
        .live_files()
        .map_err(|error| format!("Failed inspecting retirement reclaim SSTs: {error}"))?;
    Ok(retirement_estimated_reclaim_input_bytes_from_files(
        &files, range,
    ))
}

fn retirement_live_file_overlaps_range(file: &LiveFile, range: &RetirementReclaimRange) -> bool {
    if file.column_family_name != range.cf_name {
        return false;
    }
    match (file.start_key.as_deref(), file.end_key.as_deref()) {
        (Some(file_start), Some(file_end)) => {
            file_end >= range.start_key.as_slice() && file_start < range.end_key.as_slice()
        }
        _ => true,
    }
}

fn retirement_estimated_reclaim_input_bytes_from_files(
    files: &[LiveFile],
    range: &RetirementReclaimRange,
) -> u64 {
    files
        .iter()
        .filter(|file| retirement_live_file_overlaps_range(file, range))
        .fold(0u64, |total, file| total.saturating_add(file.size as u64))
}

fn split_retirement_reclaim_range(
    range: &RetirementReclaimRange,
    files: &[LiveFile],
) -> Option<(RetirementReclaimRange, RetirementReclaimRange)> {
    let parent_estimate = retirement_estimated_reclaim_input_bytes_from_files(files, range);
    let mut boundaries = files
        .iter()
        .filter(|file| retirement_live_file_overlaps_range(file, range))
        .flat_map(|file| {
            [
                file.start_key.clone(),
                file.end_key.as_deref().map(retirement_reclaim_range_end),
            ]
        })
        .flatten()
        .filter(|boundary| {
            boundary.as_slice() > range.start_key.as_slice()
                && boundary.as_slice() < range.end_key.as_slice()
        })
        .collect::<Vec<_>>();
    boundaries.sort();
    boundaries.dedup();

    boundaries
        .into_iter()
        .filter_map(|boundary| {
            let left = RetirementReclaimRange {
                store: range.store,
                cf_name: range.cf_name.clone(),
                start_key: range.start_key.clone(),
                end_key: boundary.clone(),
            };
            let right = RetirementReclaimRange {
                store: range.store,
                cf_name: range.cf_name.clone(),
                start_key: boundary,
                end_key: range.end_key.clone(),
            };
            let left_estimate = retirement_estimated_reclaim_input_bytes_from_files(files, &left);
            let right_estimate = retirement_estimated_reclaim_input_bytes_from_files(files, &right);
            if left_estimate >= parent_estimate || right_estimate >= parent_estimate {
                return None;
            }
            Some((
                left_estimate.max(right_estimate),
                left_estimate.saturating_add(right_estimate),
                left.end_key.clone(),
                left,
                right,
            ))
        })
        .min_by(|left, right| (left.0, left.1, &left.2).cmp(&(right.0, right.1, &right.2)))
        .map(|(_, _, _, left, right)| (left, right))
}

type RetirementReclaimCandidate = (usize, RetirementReclaimRange, u64);

#[allow(clippy::too_many_arguments)]
fn select_retirement_reclaim_candidate(
    ranges: &[RetirementReclaimRange],
    remaining_input_bytes: u64,
    hot_available_bytes: u64,
    hot_required_reserve_bytes: u64,
    cold_available_bytes: u64,
    cold_required_reserve_bytes: u64,
    mut estimate_input_bytes: impl FnMut(&RetirementReclaimRange) -> Result<u64, String>,
) -> Result<(Option<RetirementReclaimCandidate>, Option<String>), String> {
    let mut first_paused_reason = None;
    for (index, range) in ranges.iter().enumerate() {
        let estimate = estimate_input_bytes(range)?;
        if estimate > remaining_input_bytes {
            first_paused_reason.get_or_insert_with(|| {
                format!(
                    "compaction_input_budget:store={:?}:family={}:estimated_bytes={estimate}:remaining_bytes={remaining_input_bytes}",
                    range.store, range.cf_name
                )
            });
            continue;
        }
        let (available_bytes, required_reserve_bytes) = match range.store {
            RetirementStore::Hot => (hot_available_bytes, hot_required_reserve_bytes),
            RetirementStore::Cold => (cold_available_bytes, cold_required_reserve_bytes),
        };
        let estimated_peak = estimate.saturating_mul(2);
        if available_bytes < required_reserve_bytes.saturating_add(estimated_peak) {
            first_paused_reason.get_or_insert_with(|| {
                format!(
                    "compaction_headroom:store={:?}:family={}:available_bytes={available_bytes}:reserve_bytes={required_reserve_bytes}:estimated_peak_bytes={estimated_peak}",
                    range.store, range.cf_name
                )
            });
            continue;
        }
        return Ok((Some((index, range.clone(), estimate)), None));
    }
    Ok((None, first_paused_reason))
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

fn retirement_allows_absent_source_row(category: &str) -> bool {
    // Archive V2 derives tx_by_slot byte-for-byte from the verified canonical
    // block transaction order. Retirement equivalence separately requires and
    // verifies both the full block and transaction categories before a journal
    // can be created. An absent tx_by_slot row is therefore an already-absent,
    // rebuildable secondary index entry, not missing canonical history. Every
    // present row must still match exactly, and every other missing category
    // continues to abort retirement.
    category == "tx_by_slot"
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
    use crate::codec::append_legacy_bincode;
    use crate::{Block, CommitSignature, Keypair, Message, PqPublicKey, PqSignature, Transaction};

    struct RetirementFixture {
        _state_root: TempDir,
        _cold_root: TempDir,
        archive_root: TempDir,
        journal_root: TempDir,
        state: StateStore,
        block_hash: Hash,
        tx_hash: Hash,
        unretired_block_hash: Option<Hash>,
        retirement: ArchiveV2RetirementManifest,
    }

    fn fixture() -> RetirementFixture {
        fixture_with_segment(0, None)
    }

    fn fixture_with_segment(
        segment_end_slot: u64,
        retirement_window: Option<(u64, u64)>,
    ) -> RetirementFixture {
        let state_root = tempdir().unwrap();
        let cold_root = tempdir().unwrap();
        let archive_root = tempdir().unwrap();
        let journal_root = tempdir().unwrap();
        let mut state = StateStore::open(state_root.path()).unwrap();
        state.open_cold_store(cold_root.path()).unwrap();
        let mut blocks = Vec::new();
        let mut tx_hash = None;
        let mut parent_hash = Hash::default();
        for slot in 0..=segment_end_slot {
            let transaction = Transaction::new(Message::new(
                Vec::new(),
                Hash::hash(&[b"archive-v2-retirement-tx".as_slice(), &slot.to_be_bytes()].concat()),
            ));
            tx_hash.get_or_insert_with(|| transaction.signature());
            let mut block = Block::new_with_timestamp(
                slot,
                parent_hash,
                Hash::hash(&slot.to_be_bytes()),
                [9; 32],
                vec![transaction],
                slot + 1,
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
                timestamp: slot + 2,
            });
            parent_hash = block.hash();
            state
                .put_block_atomic(&block, Some(slot), Some(slot))
                .unwrap();
            blocks.push(block);
        }
        let block_hash = blocks[0].hash();
        let unretired_block_hash = blocks.get(1).map(Block::hash);
        state.set_last_slot(COLD_RETENTION_SLOTS + 10).unwrap();

        let hot_blocks = state.db.cf_handle(CF_BLOCKS).unwrap();
        let cold = state.cold_db.as_ref().unwrap();
        let cold_blocks = cold.cf_handle(COLD_CF_BLOCKS).unwrap();
        for block in &blocks {
            let hash = block.hash();
            let block_bytes = state.db.get_cf(&hot_blocks, hash.0).unwrap().unwrap();
            cold.put_cf(&cold_blocks, hash.0, &block_bytes).unwrap();
        }

        let identity = ArchiveV2Identity {
            network_id: "retirement-testnet".to_string(),
            genesis_hash: block_hash,
        };
        let contents = ArchiveV2SegmentContents {
            blocks,
            public_categories: state
                .archive_v2_public_categories(0, segment_end_slot)
                .unwrap(),
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
        let rollback_anchor = ArchiveV2RollbackAnchor {
            release_tag: "v0.6.0".to_string(),
            release_commit: "b".repeat(40),
            artifact_sha256: Hash::hash(b"artifact"),
            detached_pq_checksum_signature_sha256: Hash::hash(b"pq"),
            archive_format_version: ARCHIVE_V2_FORMAT_VERSION,
            catalog_format_version: crate::archive_v2::ARCHIVE_V2_CATALOG_VERSION,
            deployed_validator_count: 4,
            activated_unix_seconds: 1,
        };
        let request = match retirement_window {
            Some((start_slot, end_slot)) => state.prepare_archive_v2_retirement_window_request(
                segment_manifest.segment_object_hash,
                start_slot,
                end_slot,
                replica_evidence,
                2,
                2,
                rollback_anchor,
                1,
            ),
            None => state.prepare_archive_v2_retirement_request(
                segment_manifest.segment_object_hash,
                replica_evidence,
                2,
                2,
                rollback_anchor,
                1,
            ),
        }
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
            tx_hash: tx_hash.expect("fixture has a transaction"),
            unretired_block_hash,
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
    fn retirement_multi_get_is_bounded_and_preserves_row_order() {
        let state_root = tempdir().unwrap();
        let state = StateStore::open(state_root.path()).unwrap();
        let slots = state.db.cf_handle(CF_SLOTS).unwrap();
        let mut batch = WriteBatch::default();
        let mut rows = Vec::with_capacity(RETIREMENT_MULTI_GET_ROWS);
        for slot in 0..RETIREMENT_MULTI_GET_ROWS as u64 {
            let key = slot.to_be_bytes().to_vec();
            let value = Hash::hash(&key).0.to_vec();
            batch.put_cf(&slots, &key, &value);
            rows.push((key, value));
        }
        state.db.write(batch).unwrap();

        let prepared = state.prepare_retirement_rows("slots", &rows).unwrap();
        assert_eq!(prepared.len(), rows.len());
        for ((key, value), targets) in rows.iter().zip(prepared) {
            assert_eq!(targets.len(), 1);
            assert_eq!(targets[0].store, RetirementStore::Hot);
            assert_eq!(targets[0].key, *key);
            assert_eq!(targets[0].canonical_value_hash, Hash::hash(value));
        }

        rows.push((vec![0; 8], vec![0; 32]));
        assert!(state
            .prepare_retirement_rows("slots", &rows)
            .unwrap_err()
            .contains("safety bound"));

        let logical_sizes = [RETIREMENT_MULTI_GET_BYTES / 2 + 1; 2];
        assert_eq!(
            retirement_multi_get_chunk_end(&logical_sizes, 0, |bytes| *bytes),
            1
        );
        assert_eq!(
            retirement_multi_get_chunk_end(&logical_sizes, 1, |bytes| *bytes),
            2
        );
    }

    #[test]
    fn retirement_window_must_be_nonempty_and_contained_by_its_segment() {
        assert!(retirement_range_within_segment(
            (250_000, 299_999),
            (250_000, 254_999)
        ));
        assert!(retirement_range_within_segment(
            (250_000, 299_999),
            (250_000, 299_999)
        ));
        assert!(!retirement_range_within_segment(
            (250_000, 299_999),
            (249_999, 254_999)
        ));
        assert!(!retirement_range_within_segment(
            (250_000, 299_999),
            (295_000, 300_000)
        ));
        assert!(!retirement_range_within_segment(
            (250_000, 299_999),
            (255_000, 254_999)
        ));
    }

    #[test]
    fn retirement_reclaim_limit_accepts_large_bounded_sst_input() {
        assert!(ArchiveV2RetirementReclaimLimits {
            max_ranges: 1,
            max_estimated_input_bytes: 32 * 1024 * 1024 * 1024,
            hot_available_bytes: u64::MAX,
            hot_required_reserve_bytes: 0,
            cold_available_bytes: u64::MAX,
            cold_required_reserve_bytes: 0,
        }
        .validate()
        .is_ok());
        assert!(ArchiveV2RetirementReclaimLimits {
            max_ranges: 1,
            max_estimated_input_bytes: 32 * 1024 * 1024 * 1024 + 1,
            hot_available_bytes: u64::MAX,
            hot_required_reserve_bytes: 0,
            cold_available_bytes: u64::MAX,
            cold_required_reserve_bytes: 0,
        }
        .validate()
        .is_err());
    }

    #[test]
    fn retirement_reclaim_skips_a_blocked_front_range() {
        let ranges = vec![
            RetirementReclaimRange {
                store: RetirementStore::Cold,
                cf_name: "blocks".to_string(),
                start_key: vec![0],
                end_key: vec![1],
            },
            RetirementReclaimRange {
                store: RetirementStore::Hot,
                cf_name: "tx_meta".to_string(),
                start_key: vec![0],
                end_key: vec![1],
            },
        ];
        let estimate = |range: &RetirementReclaimRange| match range.cf_name.as_str() {
            "blocks" => Ok(400),
            "tx_meta" => Ok(40),
            other => Err(format!("unexpected family {other}")),
        };

        let (candidate, paused_reason) =
            select_retirement_reclaim_candidate(&ranges, 1_000, 1_000, 100, 500, 100, estimate)
                .unwrap();
        assert_eq!(candidate, Some((1, ranges[1].clone(), 40)));
        assert_eq!(paused_reason, None);

        let (candidate, paused_reason) =
            select_retirement_reclaim_candidate(&ranges, 30, 1_000, 100, 500, 100, estimate)
                .unwrap();
        assert_eq!(candidate, None);
        assert_eq!(
            paused_reason.as_deref(),
            Some(
                "compaction_input_budget:store=Cold:family=blocks:estimated_bytes=400:remaining_bytes=30"
            )
        );
    }

    fn retirement_test_live_file(
        name: &str,
        size: usize,
        start: Option<u8>,
        end: Option<u8>,
    ) -> LiveFile {
        LiveFile {
            column_family_name: "blocks".to_string(),
            name: name.to_string(),
            size,
            level: 1,
            start_key: start.map(|key| vec![key]),
            end_key: end.map(|key| vec![key]),
            num_entries: 1,
            num_deletions: 1,
        }
    }

    #[test]
    fn retirement_reclaim_split_uses_a_balanced_live_file_boundary() {
        let range = RetirementReclaimRange {
            store: RetirementStore::Cold,
            cf_name: "blocks".to_string(),
            start_key: vec![0],
            end_key: vec![100],
        };
        let files = vec![
            retirement_test_live_file("one.sst", 20, Some(0), Some(30)),
            retirement_test_live_file("two.sst", 20, Some(31), Some(60)),
            retirement_test_live_file("three.sst", 20, Some(61), Some(99)),
        ];

        let (left, right) = split_retirement_reclaim_range(&range, &files).unwrap();
        assert_eq!(left.start_key, range.start_key);
        assert_eq!(left.end_key, right.start_key);
        assert_eq!(right.end_key, range.end_key);
        assert_eq!(left.end_key, vec![31]);
        assert_eq!(
            retirement_estimated_reclaim_input_bytes_from_files(&files, &left),
            20
        );
        assert_eq!(
            retirement_estimated_reclaim_input_bytes_from_files(&files, &right),
            40
        );
    }

    #[test]
    fn retirement_reclaim_split_refuses_a_non_reducing_boundary() {
        let range = RetirementReclaimRange {
            store: RetirementStore::Cold,
            cf_name: "blocks".to_string(),
            start_key: vec![0],
            end_key: vec![100],
        };
        let files = vec![
            retirement_test_live_file("spanning.sst", 100, Some(0), Some(99)),
            retirement_test_live_file("unbounded.sst", 50, None, None),
        ];

        assert_eq!(split_retirement_reclaim_range(&range, &files), None);
    }

    #[test]
    fn retirement_single_pass_advances_across_categories_within_limits() {
        let fixture = fixture();
        let journal = fixture.journal_root.path().join("single-pass.journal");
        let report = fixture
            .state
            .retire_archive_v2_segment_pass(
                &fixture.retirement,
                &journal,
                ArchiveV2RetirementLimits {
                    max_rows: 100_000,
                    max_bytes: 1024 * 1024 * 1024,
                    max_wall_time: Duration::from_secs(60),
                },
            )
            .unwrap();

        assert_eq!(report.phase, ArchiveV2RetirementPhase::ReclaimPending);
        assert_eq!(
            report.categories_completed as usize,
            fixture.retirement.category_proofs().len()
        );
        assert!(report.scanned_rows > 0);
        assert!(report.deleted_hot_rows + report.deleted_cold_rows > 0);
    }

    #[test]
    fn retirement_window_journals_and_deletes_only_its_signed_slot_range() {
        let fixture = fixture_with_segment(1, Some((0, 0)));
        let journal_path = fixture.journal_root.path().join("window.journal");
        let report = fixture
            .state
            .retire_archive_v2_segment_pass(
                &fixture.retirement,
                &journal_path,
                ArchiveV2RetirementLimits {
                    max_rows: 100_000,
                    max_bytes: 1024 * 1024 * 1024,
                    max_wall_time: Duration::from_secs(60),
                },
            )
            .unwrap();

        assert_eq!(report.phase, ArchiveV2RetirementPhase::ReclaimPending);
        assert_eq!(fixture.retirement.slot_range(), (0, 0));
        let journal = load_retirement_journal(&journal_path).unwrap();
        assert_eq!((journal.start_slot, journal.end_slot), (0, 0));

        let unretired_hash = fixture.unretired_block_hash.unwrap();
        let hot_blocks = fixture.state.db.cf_handle(CF_BLOCKS).unwrap();
        assert!(fixture
            .state
            .db
            .get_cf(&hot_blocks, fixture.block_hash.0)
            .unwrap()
            .is_none());
        assert!(fixture
            .state
            .db
            .get_cf(&hot_blocks, unretired_hash.0)
            .unwrap()
            .is_some());
        let cold = fixture.state.cold_db.as_ref().unwrap();
        let cold_blocks = cold.cf_handle(COLD_CF_BLOCKS).unwrap();
        assert!(cold
            .get_cf(&cold_blocks, fixture.block_hash.0)
            .unwrap()
            .is_none());
        assert!(cold
            .get_cf(&cold_blocks, unretired_hash.0)
            .unwrap()
            .is_some());
        assert_eq!(
            fixture
                .state
                .get_block(&fixture.block_hash)
                .unwrap()
                .unwrap()
                .hash(),
            fixture.block_hash
        );
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

    #[test]
    fn retirement_point_verification_rejects_conflicting_source_rows() {
        let fixture = fixture();
        let hot_blocks = fixture.state.db.cf_handle(CF_BLOCKS).unwrap();
        let mut conflicting_block = fixture
            .state
            .get_block(&fixture.block_hash)
            .unwrap()
            .unwrap();
        conflicting_block.header.state_root = Hash::hash(b"conflicting-state-root");
        let mut conflicting_value = vec![0xBC];
        append_legacy_bincode(&mut conflicting_value, &conflicting_block, "block").unwrap();
        fixture
            .state
            .db
            .put_cf(&hot_blocks, fixture.block_hash.0, conflicting_value)
            .unwrap();
        let journal = fixture.journal_root.path().join("conflict.journal");
        let error = fixture
            .state
            .retire_archive_v2_segment_pass(
                &fixture.retirement,
                &journal,
                ArchiveV2RetirementLimits::default(),
            )
            .unwrap_err();
        assert!(
            error.contains("Block snapshot key/hash mismatch"),
            "{error}"
        );
        assert!(!journal.exists());
    }

    #[test]
    fn retirement_point_verification_rejects_missing_source_rows() {
        let fixture = fixture();
        let hot_blocks = fixture.state.db.cf_handle(CF_BLOCKS).unwrap();
        fixture
            .state
            .db
            .delete_cf(&hot_blocks, fixture.block_hash.0)
            .unwrap();
        let cold = fixture.state.cold_db.as_ref().unwrap();
        let cold_blocks = cold.cf_handle(COLD_CF_BLOCKS).unwrap();
        cold.delete_cf(&cold_blocks, fixture.block_hash.0).unwrap();
        let journal = fixture.journal_root.path().join("missing.journal");
        let error = fixture
            .state
            .retire_archive_v2_segment_pass(
                &fixture.retirement,
                &journal,
                ArchiveV2RetirementLimits::default(),
            )
            .unwrap_err();
        assert!(error.contains("absent from hot and cold blocks"), "{error}");
        assert!(!journal.exists());
    }

    #[test]
    fn retirement_skips_absent_rebuildable_tx_by_slot_rows() {
        let fixture = fixture();
        let tx_by_slot = fixture.state.db.cf_handle(CF_TX_BY_SLOT).unwrap();
        let mut tx_by_slot_key = Vec::with_capacity(16);
        tx_by_slot_key.extend_from_slice(&0u64.to_be_bytes());
        tx_by_slot_key.extend_from_slice(&0u64.to_be_bytes());
        fixture
            .state
            .db
            .delete_cf(&tx_by_slot, &tx_by_slot_key)
            .unwrap();

        let journal = fixture
            .journal_root
            .path()
            .join("missing-rebuildable-index.journal");
        let report = fixture
            .state
            .retire_archive_v2_segment_pass(
                &fixture.retirement,
                &journal,
                ArchiveV2RetirementLimits {
                    max_rows: 100_000,
                    max_bytes: 1024 * 1024 * 1024,
                    max_wall_time: Duration::from_secs(60),
                },
            )
            .unwrap();

        assert_eq!(report.phase, ArchiveV2RetirementPhase::ReclaimPending);
        assert_eq!(report.skipped_absent_rebuildable_rows, 1);
        assert!(journal.exists());
        assert_eq!(
            fixture
                .state
                .get_block(&fixture.block_hash)
                .unwrap()
                .unwrap()
                .hash(),
            fixture.block_hash
        );
        assert_eq!(
            fixture
                .state
                .get_transaction(&fixture.tx_hash)
                .unwrap()
                .unwrap()
                .signature(),
            fixture.tx_hash
        );
    }

    #[test]
    fn retirement_rejects_conflicting_rebuildable_tx_by_slot_rows() {
        let fixture = fixture();
        let tx_by_slot = fixture.state.db.cf_handle(CF_TX_BY_SLOT).unwrap();
        let mut tx_by_slot_key = Vec::with_capacity(16);
        tx_by_slot_key.extend_from_slice(&0u64.to_be_bytes());
        tx_by_slot_key.extend_from_slice(&0u64.to_be_bytes());
        fixture
            .state
            .db
            .put_cf(
                &tx_by_slot,
                &tx_by_slot_key,
                Hash::hash(b"conflicting-tx-by-slot").0,
            )
            .unwrap();

        let journal = fixture
            .journal_root
            .path()
            .join("conflicting-rebuildable-index.journal");
        let error = fixture
            .state
            .retire_archive_v2_segment_pass(
                &fixture.retirement,
                &journal,
                ArchiveV2RetirementLimits::default(),
            )
            .unwrap_err();
        assert!(error.contains("Hot tx_by_slot row"), "{error}");
        assert!(error.contains("conflicts with Archive V2"), "{error}");
        assert!(!journal.exists());
    }
}
