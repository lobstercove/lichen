use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rocksdb::{
    BottommostLevelCompaction, CompactOptions, Direction, FlushOptions, LiveFile, ReadOptions,
    WriteBatch, WriteOptions,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::*;

pub const COLD_MIGRATION_CURSOR_FORMAT_VERSION: u16 = 1;
pub const COLD_MIGRATION_STORAGE_FORMAT_VERSION: u16 = 1;

const COLD_MIGRATION_CURSOR_KEY: &[u8] = b"cold_migration_cursor_v1";
const COLD_MIGRATION_CURSOR_MAGIC: &[u8] = b"lichen-cold-migration-cursor-v1\0";
const COLD_MIGRATION_CURSOR_HASH_DOMAIN: &[u8] = b"lichen:cold-migration-cursor:v1";
const MAX_CURSOR_RECORD_BYTES: usize = 8 * 1024 * 1024;
const MAX_PENDING_ROWS: usize = 100_000;
const MAX_PENDING_KEY_BYTES: usize = 4 * 1024;
const MAX_RECLAIM_RANGES: usize = 4_096;
const MAX_RECLAIM_SPLITS_PER_PASS: u64 = 64;
type MigrationRows = BTreeMap<(String, Vec<u8>), Vec<u8>>;

const BLOCK_CATEGORY: &str = "canonical_blocks";
const INDEX_CATEGORIES: [(&str, &str, &str); 5] = [
    ("account_txs", CF_ACCOUNT_TXS, COLD_CF_ACCOUNT_TXS),
    (
        "account_snapshots",
        CF_ACCOUNT_SNAPSHOTS,
        COLD_CF_ACCOUNT_SNAPSHOTS,
    ),
    ("events", CF_EVENTS, COLD_CF_EVENTS),
    (
        "token_transfers",
        CF_TOKEN_TRANSFERS,
        COLD_CF_TOKEN_TRANSFERS,
    ),
    ("program_calls", CF_PROGRAM_CALLS, COLD_CF_PROGRAM_CALLS),
];

fn required_category_names() -> impl Iterator<Item = &'static str> {
    std::iter::once(BLOCK_CATEGORY).chain(INDEX_CATEGORIES.iter().map(|entry| entry.0))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColdMigrationPhase {
    #[default]
    Idle,
    Migrating,
    ColdDurable,
    Paused,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColdMigrationCategoryProgress {
    pub category: String,
    pub completed_through_slot: Option<u64>,
    pub scan_after_key: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingRow {
    cf_name: String,
    key: Vec<u8>,
    value_hash: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReclaimRange {
    cf_name: String,
    start_key: Vec<u8>,
    end_key: Vec<u8>,
}

fn reclaim_live_file_overlaps_range(file: &LiveFile, range: &ReclaimRange) -> bool {
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

fn estimated_reclaim_input_bytes_from_files(files: &[LiveFile], range: &ReclaimRange) -> u64 {
    files
        .iter()
        .filter(|file| reclaim_live_file_overlaps_range(file, range))
        .fold(0u64, |total, file| total.saturating_add(file.size as u64))
}

fn split_reclaim_range(
    range: &ReclaimRange,
    files: &[LiveFile],
) -> Option<(ReclaimRange, ReclaimRange)> {
    let parent_estimate = estimated_reclaim_input_bytes_from_files(files, range);
    let mut boundaries = files
        .iter()
        .filter(|file| reclaim_live_file_overlaps_range(file, range))
        .flat_map(|file| {
            [
                file.start_key.clone(),
                file.end_key.as_deref().map(StateStore::reclaim_range_end),
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
            let left = ReclaimRange {
                cf_name: range.cf_name.clone(),
                start_key: range.start_key.clone(),
                end_key: boundary.clone(),
            };
            let right = ReclaimRange {
                cf_name: range.cf_name.clone(),
                start_key: boundary,
                end_key: range.end_key.clone(),
            };
            let left_estimate = estimated_reclaim_input_bytes_from_files(files, &left);
            let right_estimate = estimated_reclaim_input_bytes_from_files(files, &right);
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingBatch {
    category: String,
    cold_rows: Vec<PendingRow>,
    hot_deletes: Vec<PendingRow>,
    progress_after: ColdMigrationCategoryProgress,
    #[serde(default)]
    reclaim_ranges: Vec<ReclaimRange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColdMigrationCursor {
    pub cursor_format_version: u16,
    pub storage_format_version: u16,
    pub network_id: String,
    pub genesis_hash: [u8; 32],
    pub highest_fully_migrated_slot: Option<u64>,
    pub last_fully_migrated_block_hash: Option<[u8; 32]>,
    pub active_target_slot: Option<u64>,
    pub categories: Vec<ColdMigrationCategoryProgress>,
    pub phase: ColdMigrationPhase,
    #[serde(default)]
    reclaim_queue: Vec<ReclaimRange>,
    pending: Option<PendingBatch>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColdMigrationLimits {
    pub max_rows: u64,
    pub max_bytes: u64,
    pub max_wall_time: Duration,
    pub max_slots_per_target: u64,
}

impl Default for ColdMigrationLimits {
    fn default() -> Self {
        Self {
            max_rows: 2_000,
            max_bytes: 64 * 1024 * 1024,
            max_wall_time: Duration::from_secs(2),
            max_slots_per_target: 50_000,
        }
    }
}

impl ColdMigrationLimits {
    pub fn validate(self) -> Result<Self, String> {
        if self.max_rows == 0 || self.max_rows > 100_000 {
            return Err("cold migration max_rows must be in 1..=100000".to_string());
        }
        if self.max_bytes < 1024 * 1024 || self.max_bytes > 1024 * 1024 * 1024 {
            return Err("cold migration max_bytes must be in 1048576..=1073741824".to_string());
        }
        if self.max_wall_time < Duration::from_millis(10)
            || self.max_wall_time > Duration::from_secs(60)
        {
            return Err("cold migration max_wall_time must be in 10ms..=60s".to_string());
        }
        if self.max_slots_per_target == 0 || self.max_slots_per_target > 1_000_000 {
            return Err("cold migration max_slots_per_target must be in 1..=1000000".to_string());
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColdReclaimLimits {
    pub max_ranges: u64,
    pub max_estimated_input_bytes: u64,
    pub available_bytes: u64,
    pub required_reserve_bytes: u64,
}

impl ColdReclaimLimits {
    pub fn validate(self) -> Result<Self, String> {
        if self.max_ranges == 0 || self.max_ranges > 16 {
            return Err("cold reclaim max_ranges must be in 1..=16".to_string());
        }
        if self.max_estimated_input_bytes < 1024 * 1024
            || self.max_estimated_input_bytes > 4 * 1024 * 1024 * 1024
        {
            return Err(
                "cold reclaim max_estimated_input_bytes must be in 1048576..=4294967296"
                    .to_string(),
            );
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColdReclaimReport {
    pub queued_ranges_before: u64,
    pub queued_ranges_after: u64,
    pub compacted_ranges: u64,
    pub split_ranges: u64,
    pub estimated_input_bytes: u64,
    pub reclaimed_physical_bytes: u64,
    pub compaction_duration_millis: u64,
    pub paused_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColdMigrationFaultPoint {
    BeforeColdBatch,
    AfterColdWriteBeforeWalSync,
    AfterWalSyncBeforeHotDeletion,
    AfterHotDeletionBeforeCursorUpdate,
    AfterCursorUpdate,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ColdMigrationPassReport {
    pub cutoff_slot: u64,
    pub cursor_slot_before: Option<u64>,
    pub cursor_slot_after: Option<u64>,
    pub cursor_hash_after: Option<[u8; 32]>,
    pub backlog_slots: u64,
    pub category: Option<String>,
    pub scanned_rows: u64,
    pub migrated_rows: u64,
    pub scanned_bytes: u64,
    pub migrated_logical_bytes: u64,
    pub migrated_physical_bytes: u64,
    pub identical_cold_rows: u64,
    pub missing_cold_rows: u64,
    pub conflicting_cold_rows: u64,
    pub recovered_pending_batch: bool,
    pub elapsed_millis: u64,
    pub phase: ColdMigrationPhase,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ColdStorageFamilyMetrics {
    pub store: String,
    pub family: String,
    pub estimated_live_bytes: u64,
    pub sst_bytes: u64,
    pub memtable_bytes: u64,
    pub estimated_rows: u64,
    pub file_count: u64,
    pub growth_bytes_per_hour: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ColdMigrationReserveStatus {
    pub runtime_floor_bytes: u64,
    pub scheduler_headroom_bytes: u64,
    pub cold_batch_staging_bytes: u64,
    pub bounded_compaction_peak_bytes: u64,
    pub calculated_reserve_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ColdMigrationStatus {
    pub cursor_format_version: u16,
    pub storage_format_version: u16,
    pub cursor_slot: Option<u64>,
    pub cursor_hash: Option<String>,
    pub backlog_slots: u64,
    pub scanned_rows: u64,
    pub migrated_rows: u64,
    pub scanned_bytes: u64,
    pub migrated_logical_bytes: u64,
    pub migrated_physical_bytes: u64,
    pub identical_cold_rows: u64,
    pub conflicting_cold_rows: u64,
    pub missing_cold_rows: u64,
    pub scan_duration_millis: u64,
    pub cold_write_duration_millis: u64,
    pub cold_flush_duration_millis: u64,
    pub hot_delete_duration_millis: u64,
    pub cursor_write_duration_millis: u64,
    pub compaction_duration_millis: u64,
    pub reclaimed_physical_bytes: u64,
    pub reclaim_queue_ranges: u64,
    pub reclaim_paused_reason: Option<String>,
    pub last_success_unix_millis: Option<u64>,
    pub last_error: Option<String>,
    pub phase: ColdMigrationPhase,
    pub paused: bool,
    pub pause_reason: Option<String>,
    pub storage_sample_unix_millis: Option<u64>,
    pub storage_metrics_error: Option<String>,
    pub storage_families: Vec<ColdStorageFamilyMetrics>,
    pub reserves: ColdMigrationReserveStatus,
    #[serde(skip)]
    previous_storage_sample_unix_millis: Option<u64>,
    #[serde(skip)]
    previous_storage_bytes: BTreeMap<String, u64>,
}

#[derive(Debug, Clone)]
struct MigrationIdentity {
    network_id: String,
    genesis_hash: [u8; 32],
    public_network: bool,
}

#[derive(Default)]
struct PreparedBatch {
    category: String,
    cold_verified: MigrationRows,
    cold_rows: MigrationRows,
    hot_deletes: MigrationRows,
    progress_after: Option<ColdMigrationCategoryProgress>,
    report: ColdMigrationPassReport,
}

impl PreparedBatch {
    fn add_cold_row(
        &mut self,
        cold: &DB,
        cf_name: &str,
        key: &[u8],
        value: &[u8],
    ) -> Result<(), String> {
        let cf = cold
            .cf_handle(cf_name)
            .ok_or_else(|| format!("Cold {cf_name} CF not found"))?;
        match cold
            .get_cf(&cf, key)
            .map_err(|err| format!("Cold read error ({cf_name}): {err}"))?
        {
            Some(existing) if existing.as_slice() == value => {
                self.report.identical_cold_rows = self.report.identical_cold_rows.saturating_add(1);
                self.cold_verified
                    .insert((cf_name.to_string(), key.to_vec()), value.to_vec());
            }
            Some(_) => {
                self.report.conflicting_cold_rows =
                    self.report.conflicting_cold_rows.saturating_add(1);
                return Err(format!(
                    "Refusing cold migration: {cf_name} key {} conflicts with the hot value",
                    hex::encode(key)
                ));
            }
            None => {
                self.report.missing_cold_rows = self.report.missing_cold_rows.saturating_add(1);
                self.cold_verified
                    .insert((cf_name.to_string(), key.to_vec()), value.to_vec());
                self.cold_rows
                    .insert((cf_name.to_string(), key.to_vec()), value.to_vec());
            }
        }
        Ok(())
    }

    fn add_hot_delete(&mut self, cf_name: &str, key: &[u8], value: &[u8]) {
        self.hot_deletes
            .insert((cf_name.to_string(), key.to_vec()), value.to_vec());
    }
}

impl StateStore {
    fn archival_family_names() -> [&'static str; 8] {
        [
            CF_BLOCKS,
            CF_TRANSACTIONS,
            CF_TX_TO_SLOT,
            CF_ACCOUNT_TXS,
            CF_ACCOUNT_SNAPSHOTS,
            CF_EVENTS,
            CF_TOKEN_TRANSFERS,
            CF_PROGRAM_CALLS,
        ]
    }

    fn storage_family_metrics(
        db: &DB,
        store: &str,
        family: &str,
    ) -> Result<ColdStorageFamilyMetrics, String> {
        let cf = db
            .cf_handle(family)
            .ok_or_else(|| format!("{store} {family} CF not found"))?;
        let property = |name| {
            db.property_int_value_cf(&cf, name)
                .map_err(|err| format!("failed reading {store} {family} metrics: {err}"))
                .map(|value| value.unwrap_or(0))
        };
        let metadata = db.get_column_family_metadata_cf(&cf);
        Ok(ColdStorageFamilyMetrics {
            store: store.to_string(),
            family: family.to_string(),
            estimated_live_bytes: property(rocksdb::properties::ESTIMATE_LIVE_DATA_SIZE)?,
            sst_bytes: metadata.size,
            memtable_bytes: property(rocksdb::properties::CUR_SIZE_ALL_MEM_TABLES)?,
            estimated_rows: property(rocksdb::properties::ESTIMATE_NUM_KEYS)?,
            file_count: metadata.file_count as u64,
            growth_bytes_per_hour: 0,
        })
    }

    fn current_archival_storage_metrics(&self) -> Result<Vec<ColdStorageFamilyMetrics>, String> {
        let mut metrics = Vec::with_capacity(16);
        for family in Self::archival_family_names() {
            metrics.push(Self::storage_family_metrics(&self.db, "hot", family)?);
        }
        if let Some(cold) = self.cold_db.as_ref() {
            for family in Self::archival_family_names() {
                metrics.push(Self::storage_family_metrics(cold, "cold", family)?);
            }
        }
        Ok(metrics)
    }

    fn refresh_cold_migration_storage_metrics_inner(&self) -> Result<(), String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        {
            let status = self
                .cold_migration_status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if status
                .storage_sample_unix_millis
                .is_some_and(|sample| now.saturating_sub(sample) < 10_000)
            {
                return Ok(());
            }
        }

        let mut families = self.current_archival_storage_metrics()?;
        let mut status = self
            .cold_migration_status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let elapsed = status
            .previous_storage_sample_unix_millis
            .map(|previous| now.saturating_sub(previous))
            .unwrap_or(0);
        for family in &mut families {
            let key = format!("{}:{}", family.store, family.family);
            let current = family.sst_bytes.saturating_add(family.memtable_bytes);
            if elapsed > 0 {
                if let Some(previous) = status.previous_storage_bytes.get(&key) {
                    let delta = i128::from(current) - i128::from(*previous);
                    let hourly = delta.saturating_mul(3_600_000) / i128::from(elapsed);
                    family.growth_bytes_per_hour =
                        hourly.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
                }
            }
            status.previous_storage_bytes.insert(key, current);
        }
        status.previous_storage_sample_unix_millis = Some(now);
        status.storage_sample_unix_millis = Some(now);
        status.storage_metrics_error = None;
        status.storage_families = families;
        Ok(())
    }

    /// Refresh the archival storage sample from a caller that is already
    /// isolated from latency-sensitive work.
    ///
    /// RocksDB column-family metadata can open or inspect every SST. On a
    /// transition fleet where immutable cold SSTs are read through FUSE, this
    /// operation can block on remote storage. It must therefore run only from
    /// the bounded maintenance blocking pool, never from an RPC handler or a
    /// consensus executor task.
    pub fn refresh_cold_migration_storage_metrics(&self) {
        if let Err(error) = self.refresh_cold_migration_storage_metrics_inner() {
            let mut status = self
                .cold_migration_status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            status.storage_metrics_error = Some(error);
        }
    }

    fn cold_physical_bytes_for_rows(&self, rows: &MigrationRows) -> Result<u64, String> {
        let cold = self
            .cold_db
            .as_ref()
            .ok_or_else(|| "Cold storage not attached".to_string())?;
        let names = rows
            .keys()
            .map(|(name, _)| name.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let mut total = 0u64;
        for name in names {
            let metrics = Self::storage_family_metrics(cold, "cold", name)?;
            total = total
                .saturating_add(metrics.sst_bytes)
                .saturating_add(metrics.memtable_bytes);
        }
        Ok(total)
    }

    fn estimated_reclaim_input_bytes(&self, range: &ReclaimRange) -> Result<u64, String> {
        let files = self
            .db
            .live_files()
            .map_err(|err| format!("failed inspecting hot SSTs for bounded reclaim: {err}"))?;
        Ok(estimated_reclaim_input_bytes_from_files(&files, range))
    }

    fn hot_family_physical_bytes(&self, cf_name: &str) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("Hot {cf_name} CF not found"))?;
        let metadata = self.db.get_column_family_metadata_cf(&cf);
        let memtable = self
            .db
            .property_int_value_cf(&cf, rocksdb::properties::CUR_SIZE_ALL_MEM_TABLES)
            .map_err(|err| format!("failed reading hot {cf_name} memtable size: {err}"))?
            .unwrap_or(0);
        Ok(metadata.size.saturating_add(memtable))
    }

    fn cold_migration_identity(&self) -> Result<MigrationIdentity, String> {
        let network_id = match self.get_metadata(crate::signing::CHAIN_ID_METADATA_KEY)? {
            Some(encoded) => String::from_utf8(encoded)
                .map_err(|_| "cold migration chain identity is not valid UTF-8".to_string())?,
            None => "development-unbound".to_string(),
        };
        if network_id.is_empty() {
            return Err("cold migration chain identity must not be empty".to_string());
        }
        let public_network = {
            let lowered = network_id.to_ascii_lowercase();
            lowered.contains("testnet") || lowered.contains("mainnet")
        };
        let genesis_hash = match self.get_block_by_slot(0)? {
            Some(block) => block.hash().0,
            None if public_network => {
                return Err(
                    "cold migration requires a canonical genesis block on a public network"
                        .to_string(),
                );
            }
            None => [0u8; 32],
        };
        Ok(MigrationIdentity {
            network_id,
            genesis_hash,
            public_network,
        })
    }

    fn initial_cold_migration_cursor(identity: &MigrationIdentity) -> ColdMigrationCursor {
        ColdMigrationCursor {
            cursor_format_version: COLD_MIGRATION_CURSOR_FORMAT_VERSION,
            storage_format_version: COLD_MIGRATION_STORAGE_FORMAT_VERSION,
            network_id: identity.network_id.clone(),
            genesis_hash: identity.genesis_hash,
            highest_fully_migrated_slot: None,
            last_fully_migrated_block_hash: None,
            active_target_slot: None,
            categories: required_category_names()
                .map(|category| ColdMigrationCategoryProgress {
                    category: category.to_string(),
                    completed_through_slot: None,
                    scan_after_key: None,
                })
                .collect(),
            phase: ColdMigrationPhase::Idle,
            reclaim_queue: Vec::new(),
            pending: None,
        }
    }

    fn cursor_payload_hash(payload: &[u8]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(COLD_MIGRATION_CURSOR_HASH_DOMAIN);
        hasher.update((payload.len() as u64).to_be_bytes());
        hasher.update(payload);
        hasher.finalize().into()
    }

    fn encode_cold_migration_cursor(cursor: &ColdMigrationCursor) -> Result<Vec<u8>, String> {
        let payload = serde_json::to_vec(cursor)
            .map_err(|err| format!("failed to encode cold migration cursor: {err}"))?;
        if payload.len() > MAX_CURSOR_RECORD_BYTES {
            return Err(format!(
                "cold migration cursor payload is too large: {} bytes",
                payload.len()
            ));
        }
        let payload_len = u32::try_from(payload.len())
            .map_err(|_| "cold migration cursor payload length overflow".to_string())?;
        let mut encoded =
            Vec::with_capacity(COLD_MIGRATION_CURSOR_MAGIC.len() + 4 + payload.len() + 32);
        encoded.extend_from_slice(COLD_MIGRATION_CURSOR_MAGIC);
        encoded.extend_from_slice(&payload_len.to_be_bytes());
        encoded.extend_from_slice(&payload);
        encoded.extend_from_slice(&Self::cursor_payload_hash(&payload));
        Ok(encoded)
    }

    fn decode_cold_migration_cursor(encoded: &[u8]) -> Result<ColdMigrationCursor, String> {
        let minimum = COLD_MIGRATION_CURSOR_MAGIC.len() + 4 + 32;
        if encoded.len() < minimum || encoded.len() > MAX_CURSOR_RECORD_BYTES + minimum {
            return Err(format!(
                "malformed cold migration cursor length: {}",
                encoded.len()
            ));
        }
        if !encoded.starts_with(COLD_MIGRATION_CURSOR_MAGIC) {
            return Err("malformed cold migration cursor magic".to_string());
        }
        let length_offset = COLD_MIGRATION_CURSOR_MAGIC.len();
        let payload_len = u32::from_be_bytes(
            encoded[length_offset..length_offset + 4]
                .try_into()
                .expect("cursor length slice"),
        ) as usize;
        let payload_offset = length_offset + 4;
        let checksum_offset = payload_offset.saturating_add(payload_len);
        if checksum_offset.saturating_add(32) != encoded.len() {
            return Err("malformed cold migration cursor payload length".to_string());
        }
        let payload = &encoded[payload_offset..checksum_offset];
        let expected = Self::cursor_payload_hash(payload);
        if encoded[checksum_offset..] != expected {
            return Err("cold migration cursor checksum mismatch".to_string());
        }
        serde_json::from_slice(payload)
            .map_err(|err| format!("malformed cold migration cursor payload: {err}"))
    }

    fn cold_migration_stats_cf(&self) -> Result<impl rocksdb::AsColumnFamilyRef + '_, String> {
        self.db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())
    }

    fn load_cold_migration_cursor_raw(&self) -> Result<Option<ColdMigrationCursor>, String> {
        let cf = self.cold_migration_stats_cf()?;
        let encoded = self
            .db
            .get_cf(&cf, COLD_MIGRATION_CURSOR_KEY)
            .map_err(|err| format!("failed reading cold migration cursor: {err}"))?;
        encoded
            .as_deref()
            .map(Self::decode_cold_migration_cursor)
            .transpose()
    }

    fn persist_cold_migration_cursor(&self, cursor: &ColdMigrationCursor) -> Result<(), String> {
        let encoded = Self::encode_cold_migration_cursor(cursor)?;
        let cf = self.cold_migration_stats_cf()?;
        let mut batch = WriteBatch::default();
        batch.put_cf(&cf, COLD_MIGRATION_CURSOR_KEY, encoded);
        let mut options = WriteOptions::default();
        options.set_sync(true);
        self.db
            .write_opt(batch, &options)
            .map_err(|err| format!("failed to durably write cold migration cursor: {err}"))
    }

    fn cold_store_has_blocks(&self) -> Result<bool, String> {
        let cold = self
            .cold_db
            .as_ref()
            .ok_or_else(|| "Cold storage not attached".to_string())?;
        let cf = cold
            .cf_handle(COLD_CF_BLOCKS)
            .ok_or_else(|| "Cold blocks CF not found".to_string())?;
        let mut read_options = ReadOptions::default();
        read_options.set_total_order_seek(true);
        let mut iter = cold.iterator_cf_opt(&cf, read_options, rocksdb::IteratorMode::Start);
        match iter.next() {
            Some(Ok(_)) => Ok(true),
            Some(Err(err)) => Err(format!("failed probing cold block storage: {err}")),
            None => Ok(false),
        }
    }

    fn validate_cold_migration_cursor(
        &self,
        cursor: &ColdMigrationCursor,
        identity: &MigrationIdentity,
    ) -> Result<(), String> {
        if cursor.cursor_format_version != COLD_MIGRATION_CURSOR_FORMAT_VERSION {
            return Err(format!(
                "unsupported cold migration cursor version {}",
                cursor.cursor_format_version
            ));
        }
        if cursor.storage_format_version != COLD_MIGRATION_STORAGE_FORMAT_VERSION {
            return Err(format!(
                "cold migration cursor storage format {} does not match {}",
                cursor.storage_format_version, COLD_MIGRATION_STORAGE_FORMAT_VERSION
            ));
        }
        if cursor.network_id != identity.network_id || cursor.genesis_hash != identity.genesis_hash
        {
            return Err(format!(
                "cold migration cursor belongs to a different network or genesis (cursor network={}, runtime network={})",
                cursor.network_id, identity.network_id
            ));
        }
        if cursor.highest_fully_migrated_slot.is_some()
            != cursor.last_fully_migrated_block_hash.is_some()
        {
            return Err("cold migration cursor slot/hash presence is inconsistent".to_string());
        }

        let expected_categories = required_category_names().collect::<Vec<_>>();
        let actual_categories = cursor
            .categories
            .iter()
            .map(|progress| progress.category.as_str())
            .collect::<Vec<_>>();
        if actual_categories != expected_categories {
            return Err("cold migration cursor category set/order is invalid".to_string());
        }
        for progress in &cursor.categories {
            if progress
                .scan_after_key
                .as_ref()
                .is_some_and(|key| key.len() > MAX_PENDING_KEY_BYTES)
            {
                return Err(format!(
                    "cold migration cursor {} scan key is too large",
                    progress.category
                ));
            }
            if let (Some(high_water), Some(completed)) = (
                cursor.highest_fully_migrated_slot,
                progress.completed_through_slot,
            ) {
                if completed < high_water {
                    return Err(format!(
                        "cold migration cursor {} progress {} is behind global high-water {}",
                        progress.category, completed, high_water
                    ));
                }
            }
        }
        if cursor.active_target_slot.is_none()
            && cursor
                .categories
                .iter()
                .any(|progress| progress.scan_after_key.is_some())
        {
            return Err(
                "cold migration cursor has scan progress without an active target".to_string(),
            );
        }
        if let (Some(high_water), Some(target)) = (
            cursor.highest_fully_migrated_slot,
            cursor.active_target_slot,
        ) {
            if target <= high_water {
                return Err(
                    "cold migration cursor active target is not above its high-water".to_string(),
                );
            }
        }
        if cursor.phase == ColdMigrationPhase::ColdDurable && cursor.pending.is_none() {
            return Err(
                "cold migration cursor is cold_durable without a pending batch".to_string(),
            );
        }
        if cursor.pending.is_some() && cursor.phase != ColdMigrationPhase::ColdDurable {
            return Err(
                "cold migration cursor has a pending batch outside cold_durable phase".to_string(),
            );
        }
        if cursor.reclaim_queue.len() > MAX_RECLAIM_RANGES {
            return Err("cold migration cursor reclaim queue is too large".to_string());
        }
        let hot_archival_families = Self::archival_family_names();
        for range in &cursor.reclaim_queue {
            if !hot_archival_families.contains(&range.cf_name.as_str())
                || range.start_key.is_empty()
                || range.end_key.is_empty()
                || range.start_key >= range.end_key
                || range.start_key.len() > MAX_PENDING_KEY_BYTES
                || range.end_key.len() > MAX_PENDING_KEY_BYTES
            {
                return Err("cold migration cursor reclaim range is invalid".to_string());
            }
        }
        if let Some(pending) = cursor.pending.as_ref() {
            if pending
                .cold_rows
                .len()
                .saturating_add(pending.hot_deletes.len())
                > MAX_PENDING_ROWS
            {
                return Err("cold migration cursor pending batch is too large".to_string());
            }
            if pending.category != pending.progress_after.category
                || !expected_categories.contains(&pending.category.as_str())
            {
                return Err("cold migration cursor pending category is invalid".to_string());
            }
            for row in pending.cold_rows.iter().chain(&pending.hot_deletes) {
                if row.key.len() > MAX_PENDING_KEY_BYTES {
                    return Err("cold migration cursor pending key is too large".to_string());
                }
            }
            for range in &pending.reclaim_ranges {
                if !hot_archival_families.contains(&range.cf_name.as_str())
                    || range.start_key.is_empty()
                    || range.end_key.is_empty()
                    || range.start_key >= range.end_key
                    || range.start_key.len() > MAX_PENDING_KEY_BYTES
                    || range.end_key.len() > MAX_PENDING_KEY_BYTES
                {
                    return Err(
                        "cold migration cursor pending reclaim range is invalid".to_string()
                    );
                }
            }
        }

        if let (Some(slot), Some(expected_hash)) = (
            cursor.highest_fully_migrated_slot,
            cursor.last_fully_migrated_block_hash,
        ) {
            let block = self.get_block_by_slot(slot)?.ok_or_else(|| {
                format!("cold migration cursor high-water slot {slot} has no canonical block")
            })?;
            if block.hash().0 != expected_hash {
                return Err(format!(
                    "cold migration cursor high-water slot {slot} hash conflicts with canonical storage"
                ));
            }
            if let Some(next) = self.get_block_by_slot(slot.saturating_add(1))? {
                if next.header.parent_hash.0 != expected_hash {
                    return Err(format!(
                        "cold migration cursor high-water slot {slot} is not continuous with slot {}",
                        slot.saturating_add(1)
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn audit_cold_migration_cursor(&self) -> Result<Option<ColdMigrationCursor>, String> {
        let identity = self.cold_migration_identity()?;
        let cursor = self.load_cold_migration_cursor_raw()?;
        if let Some(cursor) = cursor.as_ref() {
            self.validate_cold_migration_cursor(cursor, &identity)?;
        }
        Ok(cursor)
    }

    pub fn rebuild_cold_migration_cursor(
        &self,
        highest_fully_migrated_slot: u64,
        expected_hash: [u8; 32],
        execute: bool,
    ) -> Result<ColdMigrationCursor, String> {
        let _guard = self
            .cold_migration_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let identity = self.cold_migration_identity()?;
        let block = self
            .get_block_by_slot(highest_fully_migrated_slot)?
            .ok_or_else(|| {
                format!(
                    "cannot rebuild cold migration cursor: slot {highest_fully_migrated_slot} is missing"
                )
            })?;
        if block.hash().0 != expected_hash {
            return Err(format!(
                "cannot rebuild cold migration cursor: slot {highest_fully_migrated_slot} canonical hash {} does not match {}",
                block.hash().to_hex(),
                hex::encode(expected_hash)
            ));
        }
        let cold = self
            .cold_db
            .as_ref()
            .ok_or_else(|| "Cold storage not attached".to_string())?;
        let hot_blocks = self
            .db
            .cf_handle(CF_BLOCKS)
            .ok_or_else(|| "Blocks CF not found".to_string())?;
        if self
            .db
            .get_cf(&hot_blocks, expected_hash)
            .map_err(|err| format!("failed reading hot rebuild boundary block: {err}"))?
            .is_some()
        {
            return Err(format!(
                "cannot rebuild cold migration cursor: slot {highest_fully_migrated_slot} still has a hot block row"
            ));
        }
        let cold_blocks = cold
            .cf_handle(COLD_CF_BLOCKS)
            .ok_or_else(|| "Cold blocks CF not found".to_string())?;
        let cold_block_data = cold
            .get_cf(&cold_blocks, expected_hash)
            .map_err(|err| format!("failed reading cold rebuild boundary block: {err}"))?
            .ok_or_else(|| {
                format!(
                    "cannot rebuild cold migration cursor: slot {highest_fully_migrated_slot} is not present in cold block storage"
                )
            })?;
        let cold_block =
            Self::decode_cold_migration_block(&cold_block_data, "cursor rebuild boundary block")?;
        if cold_block.header.slot != highest_fully_migrated_slot
            || cold_block.hash().0 != expected_hash
        {
            return Err(
                "cannot rebuild cold migration cursor: cold boundary block is conflicting"
                    .to_string(),
            );
        }
        let cold_transactions = cold
            .cf_handle(COLD_CF_TRANSACTIONS)
            .ok_or_else(|| "Cold transactions CF not found".to_string())?;
        let cold_tx_to_slot = cold
            .cf_handle(COLD_CF_TX_TO_SLOT)
            .ok_or_else(|| "Cold tx_to_slot CF not found".to_string())?;
        for transaction in &cold_block.transactions {
            let signature = transaction.signature();
            let transaction_data = cold
                .get_cf(&cold_transactions, signature.0)
                .map_err(|err| format!("failed reading cold rebuild transaction: {err}"))?
                .ok_or_else(|| {
                    format!(
                        "cannot rebuild cold migration cursor: boundary transaction {} is missing from cold storage",
                        signature.to_hex()
                    )
                })?;
            Self::validate_cold_migration_transaction(
                &transaction_data,
                transaction,
                "cursor rebuild boundary transaction",
            )?;
            let slot_data = cold
                .get_cf(&cold_tx_to_slot, signature.0)
                .map_err(|err| format!("failed reading cold rebuild tx_to_slot: {err}"))?
                .ok_or_else(|| {
                    format!(
                        "cannot rebuild cold migration cursor: boundary tx_to_slot {} is missing from cold storage",
                        signature.to_hex()
                    )
                })?;
            Self::validate_cold_migration_tx_slot(
                &slot_data,
                highest_fully_migrated_slot,
                "cursor rebuild boundary tx_to_slot",
            )?;
        }
        if let Some(next) = self.get_block_by_slot(highest_fully_migrated_slot.saturating_add(1))? {
            if next.header.parent_hash != block.hash() {
                return Err(format!(
                    "cannot rebuild cold migration cursor: slot {} is not parent-linked to slot {highest_fully_migrated_slot}",
                    highest_fully_migrated_slot.saturating_add(1)
                ));
            }
        }
        let finalized = self.get_last_finalized_slot().unwrap_or(0);
        if finalized > 0 && highest_fully_migrated_slot > finalized {
            return Err(format!(
                "cannot rebuild cold migration cursor beyond finalized slot {finalized}"
            ));
        }
        let mut cursor = Self::initial_cold_migration_cursor(&identity);
        cursor.highest_fully_migrated_slot = Some(highest_fully_migrated_slot);
        cursor.last_fully_migrated_block_hash = Some(expected_hash);
        for progress in &mut cursor.categories {
            progress.completed_through_slot = Some(highest_fully_migrated_slot);
        }
        self.validate_cold_migration_cursor(&cursor, &identity)?;
        if execute {
            self.persist_cold_migration_cursor(&cursor)?;
        }
        Ok(cursor)
    }

    fn row_hash(value: &[u8]) -> [u8; 32] {
        Sha256::digest(value).into()
    }

    fn pending_rows(rows: &MigrationRows) -> Vec<PendingRow> {
        rows.iter()
            .map(|((cf_name, key), value)| PendingRow {
                cf_name: cf_name.clone(),
                key: key.clone(),
                value_hash: Self::row_hash(value),
            })
            .collect()
    }

    fn reclaim_range_end(key: &[u8]) -> Vec<u8> {
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

    fn reclaim_ranges_for_hot_deletes(
        &self,
        hot_deletes: &MigrationRows,
    ) -> Result<Vec<ReclaimRange>, String> {
        if hot_deletes.is_empty() {
            return Ok(Vec::new());
        }
        let mut keys_by_cf = BTreeMap::<&str, Vec<&[u8]>>::new();
        for (cf_name, key) in hot_deletes.keys() {
            keys_by_cf
                .entry(cf_name.as_str())
                .or_default()
                .push(key.as_slice());
        }
        let files = self
            .db
            .live_files()
            .map_err(|err| format!("failed inspecting hot SSTs for bounded reclaim: {err}"))?;
        let mut ranges = std::collections::BTreeSet::new();
        for file in files {
            let Some(keys) = keys_by_cf.get(file.column_family_name.as_str()) else {
                continue;
            };
            let (Some(start_key), Some(last_key)) =
                (file.start_key.as_deref(), file.end_key.as_deref())
            else {
                continue;
            };
            if keys.iter().any(|key| *key >= start_key && *key <= last_key) {
                ranges.insert(ReclaimRange {
                    cf_name: file.column_family_name,
                    start_key: start_key.to_vec(),
                    end_key: Self::reclaim_range_end(last_key),
                });
            }
        }
        if ranges.len() > MAX_RECLAIM_RANGES {
            return Err(format!(
                "bounded reclaim would queue {} SST ranges, exceeding the {}-range safety limit",
                ranges.len(),
                MAX_RECLAIM_RANGES
            ));
        }
        Ok(ranges.into_iter().collect())
    }

    fn write_cold_batch(&self, cold_rows: &MigrationRows) -> Result<(), String> {
        if cold_rows.is_empty() {
            return Ok(());
        }
        let cold = self
            .cold_db
            .as_ref()
            .ok_or_else(|| "Cold storage not attached".to_string())?;
        let mut batch = WriteBatch::default();
        for ((cf_name, key), value) in cold_rows {
            let cf = cold
                .cf_handle(cf_name)
                .ok_or_else(|| format!("Cold {cf_name} CF not found"))?;
            batch.put_cf(&cf, key, value);
        }
        cold.write(batch)
            .map_err(|err| format!("failed writing cold migration batch: {err}"))
    }

    fn write_hot_deletes(&self, hot_deletes: &MigrationRows) -> Result<(), String> {
        if hot_deletes.is_empty() {
            return Ok(());
        }
        let mut batch = WriteBatch::default();
        for ((cf_name, key), expected_value) in hot_deletes {
            let cf = self
                .db
                .cf_handle(cf_name)
                .ok_or_else(|| format!("Hot {cf_name} CF not found"))?;
            match self
                .db
                .get_cf(&cf, key)
                .map_err(|err| format!("failed reading hot {cf_name} before deletion: {err}"))?
            {
                Some(actual) if actual.as_slice() == expected_value => {
                    batch.delete_cf(&cf, key);
                }
                Some(_) => {
                    return Err(format!(
                        "refusing hot deletion: {cf_name} key {} changed after cold validation",
                        hex::encode(key)
                    ));
                }
                None => {}
            }
        }
        let mut options = WriteOptions::default();
        options.set_sync(true);
        self.db
            .write_opt(batch, &options)
            .map_err(|err| format!("failed durably deleting migrated hot rows: {err}"))
    }

    fn validate_pending_cold_rows(&self, pending: &PendingBatch) -> Result<(), String> {
        let cold = self
            .cold_db
            .as_ref()
            .ok_or_else(|| "Cold storage not attached".to_string())?;
        for row in &pending.cold_rows {
            let cf = cold
                .cf_handle(&row.cf_name)
                .ok_or_else(|| format!("Cold {} CF not found", row.cf_name))?;
            let value = cold
                .get_cf(&cf, &row.key)
                .map_err(|err| format!("failed reading pending cold row: {err}"))?
                .ok_or_else(|| {
                    format!(
                        "pending cold migration row {}:{} is missing",
                        row.cf_name,
                        hex::encode(&row.key)
                    )
                })?;
            if Self::row_hash(&value) != row.value_hash {
                return Err(format!(
                    "pending cold migration row {}:{} has a conflicting value",
                    row.cf_name,
                    hex::encode(&row.key)
                ));
            }
        }
        Ok(())
    }

    fn pending_hot_delete_values(&self, pending: &PendingBatch) -> Result<MigrationRows, String> {
        let mut rows = BTreeMap::new();
        for row in &pending.hot_deletes {
            let cf = self
                .db
                .cf_handle(&row.cf_name)
                .ok_or_else(|| format!("Hot {} CF not found", row.cf_name))?;
            if let Some(value) = self
                .db
                .get_cf(&cf, &row.key)
                .map_err(|err| format!("failed reading pending hot row: {err}"))?
            {
                if Self::row_hash(&value) != row.value_hash {
                    return Err(format!(
                        "pending hot migration row {}:{} has changed",
                        row.cf_name,
                        hex::encode(&row.key)
                    ));
                }
                rows.insert((row.cf_name.clone(), row.key.clone()), value);
            }
        }
        Ok(rows)
    }

    fn apply_progress_after_pending(
        &self,
        cursor: &mut ColdMigrationCursor,
        progress_after: ColdMigrationCategoryProgress,
        reclaim_ranges: Vec<ReclaimRange>,
    ) -> Result<(), String> {
        let mut queued = cursor
            .reclaim_queue
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        queued.extend(reclaim_ranges);
        if queued.len() > MAX_RECLAIM_RANGES {
            return Err(format!(
                "cold migration reclaim queue would exceed {MAX_RECLAIM_RANGES} ranges"
            ));
        }
        let progress = cursor
            .categories
            .iter_mut()
            .find(|progress| progress.category == progress_after.category)
            .ok_or_else(|| "pending cold migration category disappeared".to_string())?;
        *progress = progress_after;
        cursor.reclaim_queue = queued.into_iter().collect();
        cursor.pending = None;
        cursor.phase = ColdMigrationPhase::Migrating;
        self.finish_active_target_if_complete(cursor)
    }

    fn recover_pending_cold_migration(
        &self,
        cursor: &mut ColdMigrationCursor,
    ) -> Result<bool, String> {
        let Some(pending) = cursor.pending.clone() else {
            return Ok(false);
        };
        self.validate_pending_cold_rows(&pending)?;
        let deletes = self.pending_hot_delete_values(&pending)?;
        self.write_hot_deletes(&deletes)?;
        self.apply_progress_after_pending(cursor, pending.progress_after, pending.reclaim_ranges)?;
        self.persist_cold_migration_cursor(cursor)?;
        Ok(true)
    }

    fn category_progress(
        cursor: &ColdMigrationCursor,
        category: &str,
    ) -> Result<ColdMigrationCategoryProgress, String> {
        cursor
            .categories
            .iter()
            .find(|progress| progress.category == category)
            .cloned()
            .ok_or_else(|| format!("cold migration category {category} is missing"))
    }

    fn active_category(cursor: &ColdMigrationCursor) -> Result<Option<String>, String> {
        let Some(target) = cursor.active_target_slot else {
            return Ok(None);
        };
        Ok(cursor
            .categories
            .iter()
            .find(|progress| {
                progress
                    .completed_through_slot
                    .is_none_or(|slot| slot < target)
            })
            .map(|progress| progress.category.clone()))
    }

    fn ensure_active_target(
        &self,
        cursor: &mut ColdMigrationCursor,
        cutoff_slot: u64,
        limits: ColdMigrationLimits,
    ) -> Result<(), String> {
        if cursor.active_target_slot.is_some() || cutoff_slot == 0 {
            return Ok(());
        }
        let next = cursor
            .highest_fully_migrated_slot
            .map(|slot| slot.saturating_add(1))
            .unwrap_or(0);
        if next >= cutoff_slot {
            cursor.phase = ColdMigrationPhase::Idle;
            return Ok(());
        }
        let target = next
            .saturating_add(limits.max_slots_per_target.saturating_sub(1))
            .min(cutoff_slot.saturating_sub(1));
        cursor.active_target_slot = Some(target);
        cursor.phase = ColdMigrationPhase::Migrating;
        self.persist_cold_migration_cursor(cursor)
    }

    fn finish_active_target_if_complete(
        &self,
        cursor: &mut ColdMigrationCursor,
    ) -> Result<(), String> {
        let Some(target) = cursor.active_target_slot else {
            return Ok(());
        };
        if cursor.categories.iter().any(|progress| {
            progress
                .completed_through_slot
                .is_none_or(|slot| slot < target)
        }) {
            cursor.phase = ColdMigrationPhase::Migrating;
            return Ok(());
        }
        let block = self.get_block_by_slot(target)?.ok_or_else(|| {
            format!("cannot advance cold migration high-water: target slot {target} is missing")
        })?;
        cursor.highest_fully_migrated_slot = Some(target);
        cursor.last_fully_migrated_block_hash = Some(block.hash().0);
        cursor.active_target_slot = None;
        cursor.phase = ColdMigrationPhase::Idle;
        Ok(())
    }

    fn prepare_block_batch(
        &self,
        cursor: &ColdMigrationCursor,
        limits: ColdMigrationLimits,
        started: Instant,
    ) -> Result<PreparedBatch, String> {
        let cold = self
            .cold_db
            .as_ref()
            .ok_or_else(|| "Cold storage not attached".to_string())?;
        let target = cursor
            .active_target_slot
            .ok_or_else(|| "cold migration has no active target".to_string())?;
        let progress = Self::category_progress(cursor, BLOCK_CATEGORY)?;
        let start_slot = progress
            .completed_through_slot
            .or(cursor.highest_fully_migrated_slot)
            .map(|slot| slot.saturating_add(1))
            .unwrap_or(0);

        let hot_slots = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let hot_blocks = self
            .db
            .cf_handle(CF_BLOCKS)
            .ok_or_else(|| "Blocks CF not found".to_string())?;
        let hot_transactions = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;
        let hot_tx_to_slot = self
            .db
            .cf_handle(CF_TX_TO_SLOT)
            .ok_or_else(|| "tx_to_slot CF not found".to_string())?;
        let cold_blocks = cold
            .cf_handle(COLD_CF_BLOCKS)
            .ok_or_else(|| "Cold blocks CF not found".to_string())?;
        let cold_transactions = cold
            .cf_handle(COLD_CF_TRANSACTIONS)
            .ok_or_else(|| "Cold transactions CF not found".to_string())?;
        let cold_tx_to_slot = cold
            .cf_handle(COLD_CF_TX_TO_SLOT)
            .ok_or_else(|| "Cold tx_to_slot CF not found".to_string())?;

        let mut prepared = PreparedBatch {
            category: BLOCK_CATEGORY.to_string(),
            ..PreparedBatch::default()
        };
        prepared.report.category = Some(BLOCK_CATEGORY.to_string());
        let mut last_slot = None;
        for slot in start_slot..=target {
            if prepared.report.scanned_rows > 0
                && (prepared.report.scanned_rows >= limits.max_rows
                    || prepared.report.scanned_bytes >= limits.max_bytes
                    || started.elapsed() >= limits.max_wall_time)
            {
                break;
            }
            let block_hash = self
                .db
                .get_cf(&hot_slots, slot.to_be_bytes())
                .map_err(|err| format!("failed reading canonical slot {slot}: {err}"))?
                .ok_or_else(|| {
                    format!(
                        "refusing cold migration: canonical slot {slot} is missing; a gap cannot be skipped"
                    )
                })?;
            if block_hash.len() != 32 {
                return Err(format!(
                    "refusing cold migration: canonical slot {slot} has an invalid {}-byte hash",
                    block_hash.len()
                ));
            }

            let hot_block_data = self
                .db
                .get_cf(&hot_blocks, &block_hash)
                .map_err(|err| format!("failed reading hot block at slot {slot}: {err}"))?;
            let cold_block_data = cold
                .get_cf(&cold_blocks, &block_hash)
                .map_err(|err| format!("failed reading cold block at slot {slot}: {err}"))?;
            let block_data = hot_block_data
                .as_deref()
                .or(cold_block_data.as_deref())
                .ok_or_else(|| {
                    format!(
                        "refusing cold migration: slot {slot} block {} is missing from hot and cold storage",
                        hex::encode(&block_hash)
                    )
                })?;
            let block =
                Self::decode_cold_migration_block(block_data, "bounded cursor block migration")?;
            if block.header.slot != slot || block.hash().0.as_slice() != block_hash.as_slice() {
                return Err(format!(
                    "refusing cold migration: slot {slot} cursor resolves to a conflicting block"
                ));
            }
            if let (Some(hot), Some(cold_value)) =
                (hot_block_data.as_deref(), cold_block_data.as_deref())
            {
                if hot != cold_value {
                    prepared.report.conflicting_cold_rows =
                        prepared.report.conflicting_cold_rows.saturating_add(1);
                    return Err(format!(
                        "refusing cold migration: cold block {} conflicts with hot storage",
                        hex::encode(&block_hash)
                    ));
                }
            }
            if let Some(hot) = hot_block_data.as_deref() {
                prepared.add_cold_row(cold, COLD_CF_BLOCKS, &block_hash, hot)?;
                prepared.add_hot_delete(CF_BLOCKS, &block_hash, hot);
                prepared.report.migrated_rows = prepared.report.migrated_rows.saturating_add(1);
                prepared.report.migrated_logical_bytes = prepared
                    .report
                    .migrated_logical_bytes
                    .saturating_add(hot.len() as u64);
            }
            prepared.report.scanned_rows = prepared.report.scanned_rows.saturating_add(1);
            prepared.report.scanned_bytes = prepared
                .report
                .scanned_bytes
                .saturating_add(block_hash.len() as u64)
                .saturating_add(block_data.len() as u64);

            for transaction in &block.transactions {
                let signature = transaction.signature();
                let hot_tx = self
                    .db
                    .get_cf(&hot_transactions, signature.0)
                    .map_err(|err| format!("failed reading hot transaction: {err}"))?;
                let cold_tx = cold
                    .get_cf(&cold_transactions, signature.0)
                    .map_err(|err| format!("failed reading cold transaction: {err}"))?;
                let tx_data = hot_tx.as_deref().or(cold_tx.as_deref()).ok_or_else(|| {
                    format!(
                        "refusing cold migration: block slot {slot} transaction {} is missing from hot and cold storage",
                        signature.to_hex()
                    )
                })?;
                Self::validate_cold_migration_transaction(
                    tx_data,
                    transaction,
                    "bounded cursor transaction migration",
                )?;
                if let (Some(hot), Some(cold_value)) = (hot_tx.as_deref(), cold_tx.as_deref()) {
                    if hot != cold_value {
                        return Err(format!(
                            "refusing cold migration: transaction {} conflicts with cold storage",
                            signature.to_hex()
                        ));
                    }
                }
                if let Some(hot) = hot_tx.as_deref() {
                    prepared.add_cold_row(cold, COLD_CF_TRANSACTIONS, &signature.0, hot)?;
                    prepared.add_hot_delete(CF_TRANSACTIONS, &signature.0, hot);
                    prepared.report.migrated_rows = prepared.report.migrated_rows.saturating_add(1);
                    prepared.report.migrated_logical_bytes = prepared
                        .report
                        .migrated_logical_bytes
                        .saturating_add(hot.len() as u64);
                }
                prepared.report.scanned_rows = prepared.report.scanned_rows.saturating_add(1);
                prepared.report.scanned_bytes = prepared
                    .report
                    .scanned_bytes
                    .saturating_add(signature.0.len() as u64)
                    .saturating_add(tx_data.len() as u64);

                let hot_slot = self
                    .db
                    .get_cf(&hot_tx_to_slot, signature.0)
                    .map_err(|err| format!("failed reading hot tx_to_slot: {err}"))?;
                let cold_slot = cold
                    .get_cf(&cold_tx_to_slot, signature.0)
                    .map_err(|err| format!("failed reading cold tx_to_slot: {err}"))?;
                let slot_data = hot_slot
                    .as_deref()
                    .or(cold_slot.as_deref())
                    .ok_or_else(|| {
                        format!(
                            "refusing cold migration: block slot {slot} transaction {} has no hot or cold tx_to_slot row",
                            signature.to_hex()
                        )
                    })?;
                Self::validate_cold_migration_tx_slot(
                    slot_data,
                    slot,
                    "bounded cursor tx_to_slot migration",
                )?;
                if let (Some(hot), Some(cold_value)) = (hot_slot.as_deref(), cold_slot.as_deref()) {
                    if hot != cold_value {
                        return Err(format!(
                            "refusing cold migration: tx_to_slot {} conflicts with cold storage",
                            signature.to_hex()
                        ));
                    }
                }
                if let Some(hot) = hot_slot.as_deref() {
                    prepared.add_cold_row(cold, COLD_CF_TX_TO_SLOT, &signature.0, hot)?;
                    prepared.add_hot_delete(CF_TX_TO_SLOT, &signature.0, hot);
                    prepared.report.migrated_rows = prepared.report.migrated_rows.saturating_add(1);
                    prepared.report.migrated_logical_bytes = prepared
                        .report
                        .migrated_logical_bytes
                        .saturating_add(hot.len() as u64);
                }
                prepared.report.scanned_rows = prepared.report.scanned_rows.saturating_add(1);
                prepared.report.scanned_bytes = prepared
                    .report
                    .scanned_bytes
                    .saturating_add(signature.0.len() as u64)
                    .saturating_add(slot_data.len() as u64);
            }
            last_slot = Some(slot);
        }

        if let Some(last_slot) = last_slot {
            prepared.progress_after = Some(ColdMigrationCategoryProgress {
                category: BLOCK_CATEGORY.to_string(),
                completed_through_slot: Some(last_slot),
                scan_after_key: None,
            });
        }
        Ok(prepared)
    }

    fn prepare_index_batch(
        &self,
        cursor: &ColdMigrationCursor,
        category: &str,
        hot_name: &str,
        cold_name: &str,
        limits: ColdMigrationLimits,
        started: Instant,
    ) -> Result<PreparedBatch, String> {
        let cold = self
            .cold_db
            .as_ref()
            .ok_or_else(|| "Cold storage not attached".to_string())?;
        let target = cursor
            .active_target_slot
            .ok_or_else(|| "cold migration has no active target".to_string())?;
        let progress = Self::category_progress(cursor, category)?;
        let hot_cf = self
            .db
            .cf_handle(hot_name)
            .ok_or_else(|| format!("Hot {hot_name} CF not found"))?;
        let mut read_options = ReadOptions::default();
        read_options.set_total_order_seek(true);
        let mode = progress
            .scan_after_key
            .as_deref()
            .map(|key| rocksdb::IteratorMode::From(key, Direction::Forward))
            .unwrap_or(rocksdb::IteratorMode::Start);
        let iter = self.db.iterator_cf_opt(&hot_cf, read_options, mode);
        let mut prepared = PreparedBatch {
            category: category.to_string(),
            ..PreparedBatch::default()
        };
        prepared.report.category = Some(category.to_string());
        let mut last_seen = None;
        let mut reached_end = true;

        for item in iter {
            let (key, value) =
                item.map_err(|err| format!("failed iterating hot {hot_name}: {err}"))?;
            if progress.scan_after_key.as_deref() == Some(key.as_ref()) {
                continue;
            }
            if prepared.report.scanned_rows > 0
                && (prepared.report.scanned_rows >= limits.max_rows
                    || prepared.report.scanned_bytes >= limits.max_bytes
                    || started.elapsed() >= limits.max_wall_time)
            {
                reached_end = false;
                break;
            }
            if key.len() < 40 {
                return Err(format!(
                    "refusing cold migration: {hot_name} key {} is too short to contain a slot",
                    hex::encode(&key)
                ));
            }
            let slot = u64::from_be_bytes(
                key[32..40]
                    .try_into()
                    .expect("validated archival index slot key"),
            );
            prepared.report.scanned_rows = prepared.report.scanned_rows.saturating_add(1);
            prepared.report.scanned_bytes = prepared
                .report
                .scanned_bytes
                .saturating_add(key.len() as u64)
                .saturating_add(value.len() as u64);
            if slot <= target {
                prepared.add_cold_row(cold, cold_name, &key, &value)?;
                prepared.add_hot_delete(hot_name, &key, &value);
                prepared.report.migrated_rows = prepared.report.migrated_rows.saturating_add(1);
                prepared.report.migrated_logical_bytes = prepared
                    .report
                    .migrated_logical_bytes
                    .saturating_add(value.len() as u64);
            }
            last_seen = Some(key.to_vec());
        }

        prepared.progress_after = Some(if reached_end {
            ColdMigrationCategoryProgress {
                category: category.to_string(),
                completed_through_slot: Some(target),
                scan_after_key: None,
            }
        } else {
            ColdMigrationCategoryProgress {
                category: category.to_string(),
                completed_through_slot: progress.completed_through_slot,
                scan_after_key: last_seen.or(progress.scan_after_key),
            }
        });
        Ok(prepared)
    }

    fn maybe_inject_fault(
        requested: Option<ColdMigrationFaultPoint>,
        point: ColdMigrationFaultPoint,
    ) -> Result<(), String> {
        if requested == Some(point) {
            Err(format!("injected cold migration fault at {point:?}"))
        } else {
            Ok(())
        }
    }

    fn execute_prepared_cold_batch(
        &self,
        cursor: &mut ColdMigrationCursor,
        prepared: &mut PreparedBatch,
        fault: Option<ColdMigrationFaultPoint>,
        status: &mut ColdMigrationStatus,
    ) -> Result<(), String> {
        let progress_after = prepared
            .progress_after
            .clone()
            .ok_or_else(|| "cold migration batch made no bounded progress".to_string())?;
        if prepared.hot_deletes.is_empty() {
            self.apply_progress_after_pending(cursor, progress_after, Vec::new())?;
            let cursor_started = Instant::now();
            self.persist_cold_migration_cursor(cursor)?;
            status.cursor_write_duration_millis = cursor_started.elapsed().as_millis() as u64;
            Self::maybe_inject_fault(fault, ColdMigrationFaultPoint::AfterCursorUpdate)?;
            return Ok(());
        }

        Self::maybe_inject_fault(fault, ColdMigrationFaultPoint::BeforeColdBatch)?;
        let cold_physical_before = self.cold_physical_bytes_for_rows(&prepared.cold_rows)?;
        let cold_write_started = Instant::now();
        self.write_cold_batch(&prepared.cold_rows)?;
        status.cold_write_duration_millis = cold_write_started.elapsed().as_millis() as u64;
        Self::maybe_inject_fault(fault, ColdMigrationFaultPoint::AfterColdWriteBeforeWalSync)?;

        let cold = self
            .cold_db
            .as_ref()
            .ok_or_else(|| "Cold storage not attached".to_string())?;
        let cold_flush_started = Instant::now();
        cold.flush_wal(true)
            .map_err(|err| format!("failed syncing cold migration WAL: {err}"))?;
        status.cold_flush_duration_millis = cold_flush_started.elapsed().as_millis() as u64;
        let cold_physical_after = self.cold_physical_bytes_for_rows(&prepared.cold_rows)?;
        prepared.report.migrated_physical_bytes =
            cold_physical_after.saturating_sub(cold_physical_before);
        Self::maybe_inject_fault(
            fault,
            ColdMigrationFaultPoint::AfterWalSyncBeforeHotDeletion,
        )?;

        cursor.phase = ColdMigrationPhase::ColdDurable;
        let reclaim_ranges = self.reclaim_ranges_for_hot_deletes(&prepared.hot_deletes)?;
        let queued_range_count = cursor
            .reclaim_queue
            .iter()
            .chain(&reclaim_ranges)
            .cloned()
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        if queued_range_count > MAX_RECLAIM_RANGES {
            return Err(format!(
                "cold migration reclaim queue would exceed {MAX_RECLAIM_RANGES} ranges"
            ));
        }
        cursor.pending = Some(PendingBatch {
            category: prepared.category.clone(),
            cold_rows: Self::pending_rows(&prepared.cold_verified),
            hot_deletes: Self::pending_rows(&prepared.hot_deletes),
            progress_after: progress_after.clone(),
            reclaim_ranges: reclaim_ranges.clone(),
        });
        self.persist_cold_migration_cursor(cursor)?;

        let hot_delete_started = Instant::now();
        self.write_hot_deletes(&prepared.hot_deletes)?;
        status.hot_delete_duration_millis = hot_delete_started.elapsed().as_millis() as u64;
        Self::maybe_inject_fault(
            fault,
            ColdMigrationFaultPoint::AfterHotDeletionBeforeCursorUpdate,
        )?;

        self.apply_progress_after_pending(cursor, progress_after, reclaim_ranges)?;
        let cursor_started = Instant::now();
        self.persist_cold_migration_cursor(cursor)?;
        status.cursor_write_duration_millis = cursor_started.elapsed().as_millis() as u64;
        Self::maybe_inject_fault(fault, ColdMigrationFaultPoint::AfterCursorUpdate)
    }

    fn migrate_cold_pass_inner(
        &self,
        cutoff_slot: u64,
        limits: ColdMigrationLimits,
        fault: Option<ColdMigrationFaultPoint>,
    ) -> Result<ColdMigrationPassReport, String> {
        let limits = limits.validate()?;
        let started = Instant::now();
        let _guard = self
            .cold_migration_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let identity = self.cold_migration_identity()?;

        let mut cursor = match self.load_cold_migration_cursor_raw()? {
            Some(cursor) => cursor,
            None if identity.public_network && self.cold_store_has_blocks()? => {
                return Err(
                    "mature public archive has no cold migration cursor; run the bounded cursor audit/rebuild command with a source-backed slot/hash proof"
                        .to_string(),
                );
            }
            None => {
                let cursor = Self::initial_cold_migration_cursor(&identity);
                self.persist_cold_migration_cursor(&cursor)?;
                cursor
            }
        };
        self.validate_cold_migration_cursor(&cursor, &identity)?;
        let cursor_before = cursor.highest_fully_migrated_slot;

        let mut report = ColdMigrationPassReport {
            cutoff_slot,
            cursor_slot_before: cursor_before,
            ..ColdMigrationPassReport::default()
        };
        if self.recover_pending_cold_migration(&mut cursor)? {
            report.recovered_pending_batch = true;
            report.cursor_slot_after = cursor.highest_fully_migrated_slot;
            report.cursor_hash_after = cursor.last_fully_migrated_block_hash;
            report.backlog_slots = cutoff_slot.saturating_sub(
                cursor
                    .highest_fully_migrated_slot
                    .map(|slot| slot.saturating_add(1))
                    .unwrap_or(0),
            );
            report.elapsed_millis = started.elapsed().as_millis() as u64;
            report.phase = cursor.phase;
            return Ok(report);
        }

        self.ensure_active_target(&mut cursor, cutoff_slot, limits)?;
        let Some(category) = Self::active_category(&cursor)? else {
            report.cursor_slot_after = cursor.highest_fully_migrated_slot;
            report.cursor_hash_after = cursor.last_fully_migrated_block_hash;
            report.backlog_slots = 0;
            report.elapsed_millis = started.elapsed().as_millis() as u64;
            report.phase = cursor.phase;
            return Ok(report);
        };

        let mut prepared = if category == BLOCK_CATEGORY {
            self.prepare_block_batch(&cursor, limits, started)?
        } else {
            let (_, hot_name, cold_name) = INDEX_CATEGORIES
                .iter()
                .find(|entry| entry.0 == category)
                .copied()
                .ok_or_else(|| format!("unknown cold migration category {category}"))?;
            self.prepare_index_batch(&cursor, &category, hot_name, cold_name, limits, started)?
        };

        let mut status = self
            .cold_migration_status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        status.scan_duration_millis = started.elapsed().as_millis() as u64;
        self.execute_prepared_cold_batch(&mut cursor, &mut prepared, fault, &mut status)?;

        report = prepared.report.clone();
        report.cutoff_slot = cutoff_slot;
        report.cursor_slot_before = cursor_before;
        report.cursor_slot_after = cursor.highest_fully_migrated_slot;
        report.cursor_hash_after = cursor.last_fully_migrated_block_hash;
        report.backlog_slots = cutoff_slot.saturating_sub(
            cursor
                .highest_fully_migrated_slot
                .map(|slot| slot.saturating_add(1))
                .unwrap_or(0),
        );
        report.elapsed_millis = started.elapsed().as_millis() as u64;
        report.phase = cursor.phase;

        status.cursor_format_version = cursor.cursor_format_version;
        status.storage_format_version = cursor.storage_format_version;
        status.cursor_slot = cursor.highest_fully_migrated_slot;
        status.cursor_hash = cursor.last_fully_migrated_block_hash.map(hex::encode);
        status.backlog_slots = report.backlog_slots;
        status.reclaim_queue_ranges = cursor.reclaim_queue.len() as u64;
        status.scanned_rows = status.scanned_rows.saturating_add(report.scanned_rows);
        status.migrated_rows = status.migrated_rows.saturating_add(report.migrated_rows);
        status.scanned_bytes = status.scanned_bytes.saturating_add(report.scanned_bytes);
        status.migrated_logical_bytes = status
            .migrated_logical_bytes
            .saturating_add(report.migrated_logical_bytes);
        status.migrated_physical_bytes = status
            .migrated_physical_bytes
            .saturating_add(report.migrated_physical_bytes);
        status.identical_cold_rows = status
            .identical_cold_rows
            .saturating_add(report.identical_cold_rows);
        status.missing_cold_rows = status
            .missing_cold_rows
            .saturating_add(report.missing_cold_rows);
        status.conflicting_cold_rows = status
            .conflicting_cold_rows
            .saturating_add(report.conflicting_cold_rows);
        status.phase = report.phase;
        status.last_success_unix_millis = Some(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        );
        status.last_error = None;
        status.paused = false;
        status.pause_reason = None;
        Ok(report)
    }

    pub fn migrate_cold_pass(
        &self,
        cutoff_slot: u64,
        limits: ColdMigrationLimits,
    ) -> Result<ColdMigrationPassReport, String> {
        let result = self.migrate_cold_pass_inner(cutoff_slot, limits, None);
        if let Err(error) = result.as_ref() {
            let mut status = self
                .cold_migration_status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            status.phase = ColdMigrationPhase::Failed;
            status.last_error = Some(error.clone());
        }
        result
    }

    pub fn reclaim_migrated_hot_ranges(
        &self,
        limits: ColdReclaimLimits,
    ) -> Result<ColdReclaimReport, String> {
        let limits = limits.validate()?;
        let _guard = self
            .cold_migration_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let identity = self.cold_migration_identity()?;
        let Some(mut cursor) = self.load_cold_migration_cursor_raw()? else {
            return Ok(ColdReclaimReport::default());
        };
        self.validate_cold_migration_cursor(&cursor, &identity)?;

        let started = Instant::now();
        let mut report = ColdReclaimReport {
            queued_ranges_before: cursor.reclaim_queue.len() as u64,
            queued_ranges_after: cursor.reclaim_queue.len() as u64,
            ..ColdReclaimReport::default()
        };
        let mut available_bytes = limits.available_bytes;
        while report.compacted_ranges < limits.max_ranges {
            let Some(range) = cursor.reclaim_queue.first().cloned() else {
                break;
            };
            let live_files = self
                .db
                .live_files()
                .map_err(|err| format!("failed inspecting hot SSTs for bounded reclaim: {err}"))?;
            let initial_estimate = estimated_reclaim_input_bytes_from_files(&live_files, &range);
            let initial_peak = initial_estimate.saturating_mul(2);
            if initial_estimate > limits.max_estimated_input_bytes {
                // An SST-derived range can later overlap additional lower-level
                // files and grow beyond the configured compaction envelope.
                // Persistently split it at a real live-file key boundary so a
                // safe child can be admitted on this or a later pass. Keeping
                // the oversized parent at queue index zero would otherwise
                // block physical reclaim forever.
                if report.split_ranges < MAX_RECLAIM_SPLITS_PER_PASS
                    && cursor.reclaim_queue.len() < MAX_RECLAIM_RANGES
                {
                    if let Some((left, right)) = split_reclaim_range(&range, &live_files) {
                        cursor.reclaim_queue.remove(0);
                        cursor.reclaim_queue.push(left);
                        cursor.reclaim_queue.push(right);
                        cursor.reclaim_queue.sort();
                        cursor.reclaim_queue.dedup();
                        self.persist_cold_migration_cursor(&cursor)?;
                        report.split_ranges = report.split_ranges.saturating_add(1);
                        report.queued_ranges_after = cursor.reclaim_queue.len() as u64;
                        continue;
                    }
                }
                report.paused_reason = Some(
                    if report.split_ranges >= MAX_RECLAIM_SPLITS_PER_PASS {
                        format!(
                        "reclaim_split_limit:split_ranges={}:limit={MAX_RECLAIM_SPLITS_PER_PASS}",
                        report.split_ranges
                    )
                    } else if cursor.reclaim_queue.len() >= MAX_RECLAIM_RANGES {
                        format!(
                            "reclaim_split_queue_capacity:ranges={}:limit={MAX_RECLAIM_RANGES}",
                            cursor.reclaim_queue.len()
                        )
                    } else {
                        format!(
                        "compaction_input_too_large_unsplittable:family={}:estimated_bytes={}:limit_bytes={}",
                        range.cf_name, initial_estimate, limits.max_estimated_input_bytes
                    )
                    },
                );
                break;
            }
            if available_bytes < limits.required_reserve_bytes.saturating_add(initial_peak) {
                report.paused_reason = Some(format!(
                    "compaction_headroom:available_bytes={available_bytes}:reserve_bytes={}:estimated_peak_bytes={initial_peak}",
                    limits.required_reserve_bytes
                ));
                break;
            }

            let cf = self
                .db
                .cf_handle(&range.cf_name)
                .ok_or_else(|| format!("Hot {} CF not found", range.cf_name))?;
            let mut flush_options = FlushOptions::default();
            flush_options.set_wait(true);
            self.db.flush_cf_opt(&cf, &flush_options).map_err(|err| {
                format!(
                    "failed flushing hot {} before bounded reclaim: {err}",
                    range.cf_name
                )
            })?;

            let estimate = self.estimated_reclaim_input_bytes(&range)?;
            let remaining_input = limits
                .max_estimated_input_bytes
                .saturating_sub(report.estimated_input_bytes);
            let estimated_peak = estimate.saturating_mul(2);
            if estimate > remaining_input
                || available_bytes < limits.required_reserve_bytes.saturating_add(estimated_peak)
            {
                report.paused_reason = Some(format!(
                    "compaction_budget_after_flush:family={}:estimated_bytes={estimate}:remaining_bytes={remaining_input}",
                    range.cf_name
                ));
                break;
            }

            let before = self.hot_family_physical_bytes(&range.cf_name)?;
            let range_started = Instant::now();
            let mut options = CompactOptions::default();
            options.set_exclusive_manual_compaction(false);
            options.set_bottommost_level_compaction(BottommostLevelCompaction::ForceOptimized);
            self.db.compact_range_cf_opt(
                &cf,
                Some(range.start_key.as_slice()),
                Some(range.end_key.as_slice()),
                &options,
            );
            let range_elapsed = range_started.elapsed().as_millis() as u64;
            let after = self.hot_family_physical_bytes(&range.cf_name)?;
            let reclaimed = before.saturating_sub(after);

            cursor.reclaim_queue.remove(0);
            self.persist_cold_migration_cursor(&cursor)?;
            report.compacted_ranges = report.compacted_ranges.saturating_add(1);
            report.estimated_input_bytes = report.estimated_input_bytes.saturating_add(estimate);
            report.reclaimed_physical_bytes =
                report.reclaimed_physical_bytes.saturating_add(reclaimed);
            report.compaction_duration_millis = report
                .compaction_duration_millis
                .saturating_add(range_elapsed);
            available_bytes = available_bytes.saturating_add(reclaimed);
        }
        report.queued_ranges_after = cursor.reclaim_queue.len() as u64;
        if report.compaction_duration_millis == 0 {
            report.compaction_duration_millis = started.elapsed().as_millis() as u64;
        }

        let mut status = self
            .cold_migration_status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        status.compaction_duration_millis = report.compaction_duration_millis;
        status.reclaimed_physical_bytes = status
            .reclaimed_physical_bytes
            .saturating_add(report.reclaimed_physical_bytes);
        status.reclaim_queue_ranges = report.queued_ranges_after;
        status.reclaim_paused_reason = report.paused_reason.clone();
        Ok(report)
    }

    #[cfg(test)]
    pub(crate) fn migrate_cold_pass_with_fault(
        &self,
        cutoff_slot: u64,
        limits: ColdMigrationLimits,
        fault: ColdMigrationFaultPoint,
    ) -> Result<ColdMigrationPassReport, String> {
        self.migrate_cold_pass_inner(cutoff_slot, limits, Some(fault))
    }

    pub fn cold_migration_status(&self) -> ColdMigrationStatus {
        // Status retrieval is intentionally cache-only. Public getHealth and
        // getMetrics requests share the validator's Tokio runtime with BFT;
        // synchronously refreshing RocksDB metadata here previously allowed a
        // monitoring request to block consensus on remote/FUSE cold SST I/O.
        self.cold_migration_status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn set_cold_migration_paused(&self, reason: impl Into<String>) {
        let mut status = self
            .cold_migration_status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        status.phase = ColdMigrationPhase::Paused;
        status.paused = true;
        status.pause_reason = Some(reason.into());
    }

    pub fn set_cold_migration_reserves(&self, reserves: ColdMigrationReserveStatus) {
        self.cold_migration_status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .reserves = reserves;
    }

    pub fn hot_storage_path(&self) -> std::path::PathBuf {
        self.db.path().to_path_buf()
    }

    pub fn cold_storage_path(&self) -> Option<std::path::PathBuf> {
        self.cold_db.as_ref().map(|cold| cold.path().to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::tempdir;

    use super::*;
    use crate::{Block, Instruction, Message, Pubkey, Transaction};

    fn linked_blocks(count: u64) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut parent = Hash::default();
        for slot in 0..count {
            let block = Block::new_with_timestamp(
                slot,
                parent,
                Hash::hash(b"cold-migration-v1-state"),
                [0xA5; 32],
                vec![],
                1_700_000_000 + slot,
            );
            parent = block.hash();
            blocks.push(block);
        }
        blocks
    }

    fn linked_blocks_with_transactions(count: u64) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut parent = Hash::default();
        for slot in 0..count {
            let marker = u8::try_from(slot).unwrap_or(u8::MAX);
            let transaction = Transaction::new(Message::new(
                vec![Instruction {
                    program_id: Pubkey([marker.wrapping_add(1); 32]),
                    accounts: vec![Pubkey([marker.wrapping_add(2); 32])],
                    data: vec![marker; 64],
                }],
                Hash::hash(&slot.to_be_bytes()),
            ));
            let block = Block::new_with_timestamp(
                slot,
                parent,
                Hash::hash(b"cold-migration-v1-transaction-state"),
                [0x5A; 32],
                vec![transaction],
                1_700_100_000 + slot,
            );
            parent = block.hash();
            blocks.push(block);
        }
        blocks
    }

    fn open_store(root: &Path) -> StateStore {
        let hot = root.join("hot");
        let cold = root.join("cold");
        let mut state = StateStore::open(&hot).expect("open hot");
        state.open_cold_store(&cold).expect("open cold");
        state
    }

    #[test]
    fn status_reads_are_cache_only_and_maintenance_refresh_is_explicit() {
        let root = tempdir().unwrap();
        let state = open_store(root.path());

        let initial = state.cold_migration_status();
        assert!(initial.storage_sample_unix_millis.is_none());
        assert!(initial.storage_families.is_empty());

        state.refresh_cold_migration_storage_metrics();
        let sampled = state.cold_migration_status();
        assert!(sampled.storage_sample_unix_millis.is_some());
        assert!(!sampled.storage_families.is_empty());

        let cached = state.cold_migration_status();
        assert_eq!(
            cached.storage_sample_unix_millis,
            sampled.storage_sample_unix_millis
        );
        assert_eq!(cached.storage_families, sampled.storage_families);
    }

    fn run_to_cursor(state: &StateStore, cutoff: u64) {
        for _ in 0..100 {
            let report = state
                .migrate_cold_pass(
                    cutoff,
                    ColdMigrationLimits {
                        max_rows: 2,
                        max_bytes: 1024 * 1024,
                        max_wall_time: Duration::from_secs(1),
                        max_slots_per_target: 32,
                    },
                )
                .expect("bounded migration pass");
            if report.cursor_slot_after == cutoff.checked_sub(1) {
                return;
            }
        }
        panic!("cold migration did not reach cutoff {cutoff}");
    }

    #[test]
    fn cursor_checksum_and_network_binding_fail_closed() {
        let root = tempdir().unwrap();
        let state = open_store(root.path());
        let blocks = linked_blocks(3);
        for block in &blocks {
            state.put_block(block).unwrap();
        }
        run_to_cursor(&state, 2);

        let stats = state.db.cf_handle(CF_STATS).unwrap();
        let mut encoded = state
            .db
            .get_cf(&stats, COLD_MIGRATION_CURSOR_KEY)
            .unwrap()
            .unwrap();
        let last = encoded.len() - 1;
        encoded[last] ^= 0x80;
        state
            .db
            .put_cf(&stats, COLD_MIGRATION_CURSOR_KEY, encoded)
            .unwrap();
        assert!(state
            .audit_cold_migration_cursor()
            .unwrap_err()
            .contains("checksum mismatch"));

        let other_root = tempdir().unwrap();
        let other = open_store(other_root.path());
        let other_blocks = linked_blocks(3);
        for block in &other_blocks {
            other.put_block(block).unwrap();
        }
        other
            .put_metadata(crate::signing::CHAIN_ID_METADATA_KEY, b"other-testnet")
            .unwrap();
        let cursor = StateStore::initial_cold_migration_cursor(&MigrationIdentity {
            network_id: "first-testnet".to_string(),
            genesis_hash: other_blocks[0].hash().0,
            public_network: true,
        });
        other
            .persist_cold_migration_cursor(&cursor)
            .expect("persist conflicting cursor");
        assert!(other
            .audit_cold_migration_cursor()
            .unwrap_err()
            .contains("different network"));
    }

    #[test]
    fn bounded_cursor_avoids_rescanning_genesis() {
        let root = tempdir().unwrap();
        let state = open_store(root.path());
        let blocks = linked_blocks(8);
        for block in &blocks {
            state.put_block(block).unwrap();
        }
        run_to_cursor(&state, 5);
        let cursor = state
            .audit_cold_migration_cursor()
            .unwrap()
            .expect("cursor");
        assert_eq!(cursor.highest_fully_migrated_slot, Some(4));

        let report = state
            .migrate_cold_pass(
                7,
                ColdMigrationLimits {
                    max_rows: 1,
                    max_bytes: 1024 * 1024,
                    max_wall_time: Duration::from_secs(1),
                    max_slots_per_target: 32,
                },
            )
            .unwrap();
        assert_eq!(report.category.as_deref(), Some(BLOCK_CATEGORY));
        assert_eq!(report.scanned_rows, 1);
        assert!(state.get_block_by_slot(0).unwrap().is_some());
    }

    #[test]
    fn every_durability_fault_converges_after_reopen() {
        for fault in [
            ColdMigrationFaultPoint::BeforeColdBatch,
            ColdMigrationFaultPoint::AfterColdWriteBeforeWalSync,
            ColdMigrationFaultPoint::AfterWalSyncBeforeHotDeletion,
            ColdMigrationFaultPoint::AfterHotDeletionBeforeCursorUpdate,
            ColdMigrationFaultPoint::AfterCursorUpdate,
        ] {
            let root = tempdir().unwrap();
            {
                let state = open_store(root.path());
                let blocks = linked_blocks_with_transactions(4);
                for block in &blocks {
                    state.put_block(block).unwrap();
                }
                let error = state
                    .migrate_cold_pass_with_fault(
                        3,
                        ColdMigrationLimits {
                            max_rows: 1,
                            max_bytes: 1024 * 1024,
                            max_wall_time: Duration::from_secs(1),
                            max_slots_per_target: 32,
                        },
                        fault,
                    )
                    .unwrap_err();
                assert!(error.contains("injected cold migration fault"), "{error}");
            }

            let state = open_store(root.path());
            run_to_cursor(&state, 3);
            for slot in 0..4 {
                assert_eq!(
                    state
                        .get_block_by_slot(slot)
                        .unwrap()
                        .expect("block after recovery")
                        .header
                        .slot,
                    slot
                );
            }
            assert_eq!(
                state
                    .audit_cold_migration_cursor()
                    .unwrap()
                    .unwrap()
                    .highest_fully_migrated_slot,
                Some(2),
                "fault {fault:?}"
            );
        }
    }

    #[test]
    fn rebuild_requires_exact_canonical_slot_hash() {
        let root = tempdir().unwrap();
        let state = open_store(root.path());
        let blocks = linked_blocks(3);
        for block in &blocks {
            state.put_block(block).unwrap();
        }
        assert!(state
            .rebuild_cold_migration_cursor(1, [0xFF; 32], false)
            .unwrap_err()
            .contains("does not match"));
        let hot_blocks = state.db.cf_handle(CF_BLOCKS).unwrap();
        let cold = state.cold_db.as_ref().unwrap();
        let cold_blocks = cold.cf_handle(COLD_CF_BLOCKS).unwrap();
        let encoded = state
            .db
            .get_cf(&hot_blocks, blocks[1].hash().0)
            .unwrap()
            .unwrap();
        cold.put_cf(&cold_blocks, blocks[1].hash().0, encoded)
            .unwrap();
        cold.flush_wal(true).unwrap();
        state.db.delete_cf(&hot_blocks, blocks[1].hash().0).unwrap();
        let rebuilt = state
            .rebuild_cold_migration_cursor(1, blocks[1].hash().0, true)
            .unwrap();
        assert_eq!(rebuilt.highest_fully_migrated_slot, Some(1));
        assert_eq!(
            state
                .audit_cold_migration_cursor()
                .unwrap()
                .unwrap()
                .last_fully_migrated_block_hash,
            Some(blocks[1].hash().0)
        );
    }

    #[test]
    fn block_transaction_and_slot_rows_remain_readable_from_cold() {
        let root = tempdir().unwrap();
        let state = open_store(root.path());
        let blocks = linked_blocks_with_transactions(3);
        for block in &blocks {
            state.put_block(block).unwrap();
        }
        let signatures = blocks
            .iter()
            .map(|block| block.transactions[0].signature())
            .collect::<Vec<_>>();

        run_to_cursor(&state, 3);

        let hot_transactions = state.db.cf_handle(CF_TRANSACTIONS).unwrap();
        let hot_tx_to_slot = state.db.cf_handle(CF_TX_TO_SLOT).unwrap();
        let cold = state.cold_db.as_ref().unwrap();
        let cold_transactions = cold.cf_handle(COLD_CF_TRANSACTIONS).unwrap();
        let cold_tx_to_slot = cold.cf_handle(COLD_CF_TX_TO_SLOT).unwrap();
        for (slot, signature) in signatures.iter().enumerate() {
            assert!(state
                .db
                .get_cf(&hot_transactions, signature.0)
                .unwrap()
                .is_none());
            assert!(state
                .db
                .get_cf(&hot_tx_to_slot, signature.0)
                .unwrap()
                .is_none());
            assert!(cold
                .get_cf(&cold_transactions, signature.0)
                .unwrap()
                .is_some());
            assert!(cold
                .get_cf(&cold_tx_to_slot, signature.0)
                .unwrap()
                .is_some());
            assert_eq!(
                state.get_transaction(signature).unwrap().unwrap().hash(),
                *signature
            );
            assert_eq!(state.get_tx_slot(signature).unwrap(), Some(slot as u64));
        }
    }

    #[test]
    fn every_index_category_resumes_and_preserves_cold_rows() {
        let root = tempdir().unwrap();
        let state = open_store(root.path());
        state.put_block(&linked_blocks(1)[0]).unwrap();

        for (index, (_, hot_name, _)) in INDEX_CATEGORIES.iter().enumerate() {
            let hot_cf = state.db.cf_handle(hot_name).unwrap();
            let mut key = vec![u8::try_from(index + 1).unwrap(); 40];
            key[32..40].copy_from_slice(&0u64.to_be_bytes());
            state
                .db
                .put_cf(&hot_cf, &key, [u8::try_from(index).unwrap(); 16])
                .unwrap();
        }

        run_to_cursor(&state, 1);

        let cold = state.cold_db.as_ref().unwrap();
        for (index, (_, hot_name, cold_name)) in INDEX_CATEGORIES.iter().enumerate() {
            let hot_cf = state.db.cf_handle(hot_name).unwrap();
            let cold_cf = cold.cf_handle(cold_name).unwrap();
            let mut key = vec![u8::try_from(index + 1).unwrap(); 40];
            key[32..40].copy_from_slice(&0u64.to_be_bytes());
            assert!(state.db.get_cf(&hot_cf, &key).unwrap().is_none());
            assert_eq!(
                cold.get_cf(&cold_cf, &key).unwrap().as_deref(),
                Some([u8::try_from(index).unwrap(); 16].as_slice())
            );
        }
        let cursor = state.audit_cold_migration_cursor().unwrap().unwrap();
        assert_eq!(cursor.highest_fully_migrated_slot, Some(0));
        assert!(cursor
            .categories
            .iter()
            .all(|progress| progress.completed_through_slot == Some(0)));
    }

    #[test]
    fn conflicting_cold_row_aborts_before_hot_deletion_or_cursor_advance() {
        let root = tempdir().unwrap();
        let state = open_store(root.path());
        let blocks = linked_blocks(2);
        for block in &blocks {
            state.put_block(block).unwrap();
        }
        let hash = blocks[0].hash();
        let hot_blocks = state.db.cf_handle(CF_BLOCKS).unwrap();
        let cold = state.cold_db.as_ref().unwrap();
        let cold_blocks = cold.cf_handle(COLD_CF_BLOCKS).unwrap();
        cold.put_cf(&cold_blocks, hash.0, b"conflicting-block")
            .unwrap();
        cold.flush_wal(true).unwrap();

        let error = state
            .migrate_cold_pass(2, ColdMigrationLimits::default())
            .unwrap_err();
        assert!(error.contains("conflicts with hot storage"), "{error}");
        assert!(state.db.get_cf(&hot_blocks, hash.0).unwrap().is_some());
        let cursor = state.audit_cold_migration_cursor().unwrap().unwrap();
        assert_eq!(cursor.highest_fully_migrated_slot, None);
        assert_eq!(
            StateStore::category_progress(&cursor, BLOCK_CATEGORY)
                .unwrap()
                .completed_through_slot,
            None
        );
    }

    #[test]
    fn canonical_gap_aborts_the_whole_prepared_batch_without_deletion() {
        let root = tempdir().unwrap();
        let state = open_store(root.path());
        let blocks = linked_blocks(3);
        for block in &blocks {
            state.put_block(block).unwrap();
        }
        let slots = state.db.cf_handle(CF_SLOTS).unwrap();
        state.db.delete_cf(&slots, 1u64.to_be_bytes()).unwrap();
        let hot_blocks = state.db.cf_handle(CF_BLOCKS).unwrap();

        let error = state
            .migrate_cold_pass(
                3,
                ColdMigrationLimits {
                    max_rows: 100,
                    ..ColdMigrationLimits::default()
                },
            )
            .unwrap_err();
        assert!(error.contains("gap cannot be skipped"), "{error}");
        for block in &blocks {
            assert!(state
                .db
                .get_cf(&hot_blocks, block.hash().0)
                .unwrap()
                .is_some());
        }
        assert_eq!(
            state
                .audit_cold_migration_cursor()
                .unwrap()
                .unwrap()
                .highest_fully_migrated_slot,
            None
        );
    }

    #[test]
    fn identical_write_first_row_is_idempotent_and_deleted_only_after_sync() {
        let root = tempdir().unwrap();
        let state = open_store(root.path());
        let block = linked_blocks(1).remove(0);
        let hash = block.hash();
        state.put_block(&block).unwrap();
        let hot_blocks = state.db.cf_handle(CF_BLOCKS).unwrap();
        let encoded = state.db.get_cf(&hot_blocks, hash.0).unwrap().unwrap();
        let cold = state.cold_db.as_ref().unwrap();
        let cold_blocks = cold.cf_handle(COLD_CF_BLOCKS).unwrap();
        cold.put_cf(&cold_blocks, hash.0, &encoded).unwrap();
        cold.flush_wal(true).unwrap();

        let report = state
            .migrate_cold_pass(1, ColdMigrationLimits::default())
            .unwrap();
        assert_eq!(report.identical_cold_rows, 1);
        assert!(state.db.get_cf(&hot_blocks, hash.0).unwrap().is_none());
        let recovered = state.get_block_by_slot(0).unwrap().unwrap();
        assert_eq!(recovered.header.slot, block.header.slot);
        assert_eq!(recovered.hash(), block.hash());
        run_to_cursor(&state, 1);
        assert_eq!(
            state
                .audit_cold_migration_cursor()
                .unwrap()
                .unwrap()
                .highest_fully_migrated_slot,
            Some(0)
        );
    }

    fn reclaim_test_live_file(
        name: &str,
        size: usize,
        start: Option<u8>,
        end: Option<u8>,
    ) -> LiveFile {
        LiveFile {
            column_family_name: CF_BLOCKS.to_string(),
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
    fn oversized_reclaim_range_splits_at_balanced_live_file_boundary() {
        let range = ReclaimRange {
            cf_name: CF_BLOCKS.to_string(),
            start_key: vec![0],
            end_key: vec![100],
        };
        let files = vec![
            reclaim_test_live_file("one.sst", 200, Some(0), Some(30)),
            reclaim_test_live_file("two.sst", 200, Some(31), Some(60)),
            reclaim_test_live_file("three.sst", 200, Some(61), Some(99)),
        ];

        let (left, right) = split_reclaim_range(&range, &files).unwrap();
        assert_eq!(left.start_key, range.start_key);
        assert_eq!(left.end_key, right.start_key);
        assert_eq!(right.end_key, range.end_key);
        assert_eq!(left.end_key, vec![31]);
        assert_eq!(estimated_reclaim_input_bytes_from_files(&files, &left), 200);
        assert_eq!(
            estimated_reclaim_input_bytes_from_files(&files, &right),
            400
        );
    }

    #[test]
    fn oversized_reclaim_range_refuses_non_reducing_split() {
        let range = ReclaimRange {
            cf_name: CF_BLOCKS.to_string(),
            start_key: vec![0],
            end_key: vec![100],
        };
        let files = vec![
            reclaim_test_live_file("spanning.sst", 100, Some(0), Some(99)),
            reclaim_test_live_file("unbounded.sst", 50, None, None),
        ];

        assert_eq!(split_reclaim_range(&range, &files), None);
    }

    #[test]
    fn bounded_reclaim_queue_survives_restart_and_honors_headroom() {
        let root = tempdir().unwrap();
        let queued_before;
        {
            let state = open_store(root.path());
            let blocks = linked_blocks_with_transactions(4);
            for block in &blocks {
                state.put_block(block).unwrap();
            }
            for cf_name in [CF_BLOCKS, CF_TRANSACTIONS, CF_TX_TO_SLOT] {
                let cf = state.db.cf_handle(cf_name).unwrap();
                state.db.flush_cf(&cf).unwrap();
            }
            state
                .migrate_cold_pass(4, ColdMigrationLimits::default())
                .unwrap();
            queued_before = state
                .audit_cold_migration_cursor()
                .unwrap()
                .unwrap()
                .reclaim_queue
                .len();
            assert!(queued_before > 0);
        }

        let state = open_store(root.path());
        assert_eq!(
            state
                .audit_cold_migration_cursor()
                .unwrap()
                .unwrap()
                .reclaim_queue
                .len(),
            queued_before
        );
        let paused = state
            .reclaim_migrated_hot_ranges(ColdReclaimLimits {
                max_ranges: 1,
                max_estimated_input_bytes: 4 * 1024 * 1024 * 1024,
                available_bytes: 1024 * 1024 * 1024,
                required_reserve_bytes: 1024 * 1024 * 1024,
            })
            .unwrap();
        assert_eq!(paused.compacted_ranges, 0);
        assert!(paused.paused_reason.is_some());
        assert_eq!(paused.queued_ranges_after, queued_before as u64);
        let status = state.cold_migration_status();
        assert!(!status.paused);
        assert_eq!(status.reclaim_paused_reason, paused.paused_reason);

        let reclaimed = state
            .reclaim_migrated_hot_ranges(ColdReclaimLimits {
                max_ranges: 16,
                max_estimated_input_bytes: 4 * 1024 * 1024 * 1024,
                available_bytes: 32 * 1024 * 1024 * 1024,
                required_reserve_bytes: 1024 * 1024 * 1024,
            })
            .unwrap();
        assert!(reclaimed.compacted_ranges > 0);
        assert!(reclaimed.queued_ranges_after < queued_before as u64);
        assert_eq!(
            state.cold_migration_status().reclaim_paused_reason,
            reclaimed.paused_reason
        );
        for slot in 0..4 {
            assert!(state.get_block_by_slot(slot).unwrap().is_some());
        }
    }
}
