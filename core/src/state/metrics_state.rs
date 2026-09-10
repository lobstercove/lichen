use std::collections::VecDeque;

use crate::block::Block;
use crate::codec::deserialize_legacy_bincode;

use super::*;

const OBSERVED_CADENCE_WINDOW: usize = 120;
const OBSERVED_CADENCE_MAX_SLOT_DELTA: u64 = 8;
const OBSERVED_CADENCE_MAX_LIVE_LAG_SECS: u64 = 5;

/// Metrics data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metrics {
    pub tps: f64,
    pub peak_tps: f64,
    pub total_transactions: u64,
    pub total_blocks: u64,
    pub average_block_time: f64,
    pub total_accounts: u64,
    pub active_accounts: u64,
    pub total_supply: u64,
    pub total_burned: u64,
    pub total_minted: u64,
    /// Transactions counted since midnight UTC (server-side, same for all)
    pub daily_transactions: u64,
    /// Observer-side rolling median block interval in milliseconds.
    #[serde(default)]
    pub observed_block_interval_ms: u64,
    /// Expected canonical slot cadence.
    #[serde(default)]
    pub cadence_target_ms: u64,
    /// Milliseconds since this node last observed a new canonical block.
    #[serde(default)]
    pub head_staleness_ms: u64,
    /// Number of samples currently contributing to observed cadence.
    #[serde(default)]
    pub cadence_samples: u64,
    /// Last block slot observed by this node on the canonical chain.
    #[serde(default)]
    pub last_observed_block_slot: u64,
    /// Wall-clock timestamp (ms since Unix epoch) when the last block was observed.
    #[serde(default)]
    pub last_observed_block_at_ms: u64,
}

/// Offline, evidence-bound correction of observational counters only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MetricsCounterRepair {
    pub version: u8,
    pub source_manifest_sha256: String,
    pub tip_slot: u64,
    pub tip_hash: Hash,
    pub expected_total_transactions: u64,
    pub expected_total_blocks: u64,
    pub total_transactions: u64,
    pub total_blocks: u64,
    pub daily_transactions: u64,
    pub daily_date: String,
}

/// Metrics tracker with rolling window for TPS
pub struct MetricsStore {
    // Serialize counter persistence through a block's durable write and publish.
    // Ordinary saves must not overwrite newly committed counters with old RAM.
    persistence_lock: Mutex<()>,
    // Rolling window: (timestamp, tx_count) for last 60 seconds
    window: Mutex<VecDeque<(u64, u64)>>,
    total_transactions: Mutex<u64>,
    total_blocks: Mutex<u64>,
    total_accounts: Mutex<u64>,
    active_accounts: Mutex<u64>,
    // Track block times for average calculation
    last_block_time: Mutex<u64>,
    block_times: Mutex<VecDeque<u64>>,
    /// Observer-side normalized per-slot block intervals in milliseconds.
    observed_block_intervals_ms: Mutex<VecDeque<u64>>,
    /// Last slot observed by this node while applying canonical blocks.
    last_observed_slot: Mutex<u64>,
    /// Wall-clock time of the last observed canonical block application.
    last_observed_block_at_ms: Mutex<u64>,
    /// Peak TPS observed (rolling window max)
    peak_tps: Mutex<f64>,
    /// Daily transaction counter (resets at midnight UTC)
    daily_transactions: Mutex<u64>,
    /// Date string (YYYY-MM-DD) for daily counter reset detection
    daily_date: Mutex<String>,
    /// Program (contract) count — incremented by index_program(), persisted to CF_STATS
    program_count: Mutex<u64>,
    /// Validator count — incremented/decremented by put_validator()/delete_validator()
    validator_count: Mutex<u64>,
}

pub(crate) struct PendingBlockMetrics<'a> {
    metrics: &'a MetricsStore,
    block: &'a Block,
    observed_at_ms: u64,
    date: String,
    _guard: std::sync::MutexGuard<'a, ()>,
}

impl PendingBlockMetrics<'_> {
    /// Publish only after the RocksDB batch succeeds. Dropping an uncommitted
    /// update leaves in-memory counters unchanged; the caller may safely retry.
    pub(crate) fn commit(self) {
        self.metrics
            .track_block_at(self.block, self.observed_at_ms, self.date);
    }
}

impl Default for MetricsStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsStore {
    pub fn new() -> Self {
        let today = Self::today_utc();
        MetricsStore {
            persistence_lock: Mutex::new(()),
            window: Mutex::new(VecDeque::new()),
            total_transactions: Mutex::new(0),
            total_blocks: Mutex::new(0),
            total_accounts: Mutex::new(0),
            active_accounts: Mutex::new(0),
            last_block_time: Mutex::new(0),
            block_times: Mutex::new(VecDeque::new()),
            observed_block_intervals_ms: Mutex::new(VecDeque::new()),
            last_observed_slot: Mutex::new(0),
            last_observed_block_at_ms: Mutex::new(0),
            peak_tps: Mutex::new(0.0),
            daily_transactions: Mutex::new(0),
            daily_date: Mutex::new(today),
            program_count: Mutex::new(0),
            validator_count: Mutex::new(0),
        }
    }

    /// Get current UTC date as YYYY-MM-DD
    fn today_utc() -> String {
        Self::date_utc(Self::now_unix_ms() / 1000)
    }

    fn date_utc(secs: u64) -> String {
        let days = secs / 86400;
        let (year, month, day) = Self::days_to_ymd(days);
        format!("{:04}-{:02}-{:02}", year, month, day)
    }

    fn now_unix_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

    fn median_sample(samples: &VecDeque<u64>) -> u64 {
        if samples.is_empty() {
            return 0;
        }
        let mut sorted: Vec<u64> = samples.iter().copied().collect();
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    }

    /// Convert days since Unix epoch to (year, month, day)
    fn days_to_ymd(days: u64) -> (u64, u64, u64) {
        let z = days + 719468;
        let era = z / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let year = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = if month <= 2 { year + 1 } else { year };
        (year, month, day)
    }

    /// Track a new block
    pub fn track_block(&self, block: &Block) {
        let _guard = self
            .persistence_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        self.track_block_at(block, Self::now_unix_ms(), Self::today_utc());
    }

    fn track_block_at(&self, block: &Block, observed_at_ms: u64, today: String) {
        let tx_count = block
            .transactions
            .iter()
            .filter(|transaction| !transaction.is_consensus())
            .count() as u64;
        let timestamp = block.header.timestamp;
        let daily_count = if Self::date_utc(timestamp) == today {
            tx_count
        } else {
            0
        };
        let live_observation = timestamp > 0
            && (observed_at_ms / 1000).saturating_sub(timestamp)
                <= OBSERVED_CADENCE_MAX_LIVE_LAG_SECS;

        {
            let mut window = self.window.lock().unwrap_or_else(|e| e.into_inner());
            window.push_back((timestamp, tx_count));

            let cutoff = timestamp.saturating_sub(60);
            while let Some(&(ts, _)) = window.front() {
                if ts < cutoff {
                    window.pop_front();
                } else {
                    break;
                }
            }
        }

        {
            let mut total_txs = self
                .total_transactions
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *total_txs += tx_count;
        }

        {
            let mut total_blocks = self.total_blocks.lock().unwrap_or_else(|e| e.into_inner());
            *total_blocks += 1;
        }

        {
            let mut daily_date = self.daily_date.lock().unwrap_or_else(|e| e.into_inner());
            let mut daily_txs = self
                .daily_transactions
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if *daily_date != today {
                *daily_date = today;
                *daily_txs = daily_count;
            } else {
                *daily_txs += daily_count;
            }
        }

        {
            let mut last_time = self
                .last_block_time
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if *last_time > 0 {
                let block_time = timestamp.saturating_sub(*last_time);
                let mut times = self.block_times.lock().unwrap_or_else(|e| e.into_inner());
                times.push_back(block_time);
                if times.len() > 100 {
                    times.pop_front();
                }
            }
            *last_time = timestamp;
        }

        {
            let mut last_slot = self
                .last_observed_slot
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let mut last_seen_at_ms = self
                .last_observed_block_at_ms
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let slot_delta = block.header.slot.saturating_sub(*last_slot);
            let elapsed_ms = observed_at_ms.saturating_sub(*last_seen_at_ms);
            let mut intervals = self
                .observed_block_intervals_ms
                .lock()
                .unwrap_or_else(|e| e.into_inner());

            if live_observation {
                if *last_seen_at_ms > 0 && slot_delta > 0 {
                    if slot_delta <= OBSERVED_CADENCE_MAX_SLOT_DELTA {
                        let normalized_interval_ms = elapsed_ms / slot_delta.max(1);
                        if normalized_interval_ms > 0 {
                            intervals.push_back(normalized_interval_ms);
                            if intervals.len() > OBSERVED_CADENCE_WINDOW {
                                intervals.pop_front();
                            }
                        }
                    } else {
                        intervals.clear();
                    }
                }

                *last_slot = block.header.slot;
                *last_seen_at_ms = observed_at_ms;
            } else {
                intervals.clear();
            }
        }
    }

    /// Get current metrics
    pub fn get_metrics(
        &self,
        total_supply: u64,
        total_burned: u64,
        total_minted: u64,
        total_accounts: u64,
        active_accounts: u64,
        slot_duration_ms: u64,
    ) -> Metrics {
        let _guard = self
            .persistence_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let (total_txs_in_window, time_span) = {
            let window = self.window.lock().unwrap_or_else(|e| e.into_inner());
            if window.is_empty() {
                (0, 0)
            } else {
                let total = window.iter().map(|(_, count)| count).sum::<u64>();
                let oldest = window.front().map(|(ts, _)| *ts).unwrap_or(0);
                let newest = window.back().map(|(ts, _)| *ts).unwrap_or(0);
                let span = newest.saturating_sub(oldest);
                (total, span)
            }
        };

        let tps = if time_span > 0 {
            (total_txs_in_window as f64) / (time_span as f64)
        } else {
            0.0
        };

        let peak_tps = {
            let mut peak = self.peak_tps.lock().unwrap_or_else(|e| e.into_inner());
            if tps > *peak {
                *peak = tps;
            }
            *peak
        };

        let avg_block_time = {
            let times = self.block_times.lock().unwrap_or_else(|e| e.into_inner());
            if times.is_empty() {
                0.0
            } else {
                let sum: u64 = times.iter().sum();
                (sum as f64) / (times.len() as f64)
            }
        };

        let cadence_target_ms = slot_duration_ms.max(1);

        let (observed_block_interval_ms, cadence_samples) = {
            let samples = self
                .observed_block_intervals_ms
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            (Self::median_sample(&samples), samples.len() as u64)
        };

        let last_observed_block_at_ms = *self
            .last_observed_block_at_ms
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let last_observed_block_slot = *self
            .last_observed_slot
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let head_staleness_ms = if last_observed_block_at_ms > 0 {
            Self::now_unix_ms().saturating_sub(last_observed_block_at_ms)
        } else {
            0
        };

        Metrics {
            tps,
            peak_tps,
            total_transactions: *self
                .total_transactions
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            total_blocks: *self.total_blocks.lock().unwrap_or_else(|e| e.into_inner()),
            average_block_time: avg_block_time,
            total_accounts,
            active_accounts,
            total_supply,
            total_burned,
            total_minted,
            daily_transactions: if *self.daily_date.lock().unwrap_or_else(|e| e.into_inner())
                == Self::today_utc()
            {
                *self
                    .daily_transactions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
            } else {
                0
            },
            observed_block_interval_ms,
            cadence_target_ms,
            head_staleness_ms,
            cadence_samples,
            last_observed_block_slot,
            last_observed_block_at_ms,
        }
    }

    /// Load metrics from database
    pub fn load(&self, db: &Arc<DB>) -> Result<(), String> {
        let _guard = self
            .persistence_lock
            .lock()
            .map_err(|_| "Metrics persistence lock poisoned".to_string())?;
        let cf = db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        if let Ok(Some(data)) = db.get_cf(&cf, b"total_transactions") {
            if let Ok(bytes) = data.as_slice().try_into() {
                let count = u64::from_le_bytes(bytes);
                let mut total = self
                    .total_transactions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                *total = count;
            }
        }

        if let Ok(Some(data)) = db.get_cf(&cf, b"total_blocks") {
            if let Ok(bytes) = data.as_slice().try_into() {
                let count = u64::from_le_bytes(bytes);
                let mut total = self.total_blocks.lock().unwrap_or_else(|e| e.into_inner());
                *total = count;
            }
        }

        if let Ok(Some(data)) = db.get_cf(&cf, b"total_accounts") {
            if let Ok(bytes) = data.as_slice().try_into() {
                let count = u64::from_le_bytes(bytes);
                let mut total = self
                    .total_accounts
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                *total = count;
            }
        }

        if let Ok(Some(data)) = db.get_cf(&cf, b"active_accounts") {
            if let Ok(bytes) = data.as_slice().try_into() {
                let count = u64::from_le_bytes(bytes);
                let mut total = self
                    .active_accounts
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                *total = count;
            }
        }

        if let Ok(Some(data)) = db.get_cf(&cf, b"program_count") {
            if let Ok(bytes) = data.as_slice().try_into() {
                let count = u64::from_le_bytes(bytes);
                *self.program_count.lock().unwrap_or_else(|e| e.into_inner()) = count;
            }
        }

        if let Ok(Some(data)) = db.get_cf(&cf, b"validator_count") {
            if let Ok(bytes) = data.as_slice().try_into() {
                let count = u64::from_le_bytes(bytes);
                *self
                    .validator_count
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = count;
            }
        }

        let today = Self::today_utc();
        let stored_date = db
            .get_cf(&cf, b"daily_date")
            .ok()
            .flatten()
            .and_then(|data| String::from_utf8(data).ok())
            .unwrap_or_default();
        if stored_date == today {
            if let Ok(Some(data)) = db.get_cf(&cf, b"daily_transactions") {
                if let Ok(bytes) = data.as_slice().try_into() {
                    let count = u64::from_le_bytes(bytes);
                    let mut daily = self
                        .daily_transactions
                        .lock()
                        .unwrap_or_else(|e| e.into_inner());
                    *daily = count;
                }
            }
        }

        {
            let mut daily_date = self.daily_date.lock().unwrap_or_else(|e| e.into_inner());
            *daily_date = today;
        }

        Ok(())
    }

    /// Increment account counter
    pub fn increment_accounts(&self) {
        let mut count = self
            .total_accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *count += 1;
    }

    /// Increment active accounts counter
    pub fn increment_active_accounts(&self) {
        let mut count = self
            .active_accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *count += 1;
    }

    /// Decrement active accounts counter
    pub fn decrement_active_accounts(&self) {
        let mut count = self
            .active_accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *count = count.saturating_sub(1);
    }

    /// Get total accounts count (no DB scan)
    pub fn get_total_accounts(&self) -> u64 {
        *self
            .total_accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Get active accounts count (no DB scan)
    pub fn get_active_accounts(&self) -> u64 {
        *self
            .active_accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Increment program counter
    pub fn increment_programs(&self) {
        *self.program_count.lock().unwrap_or_else(|e| e.into_inner()) += 1;
    }

    /// Get program count (no DB scan)
    pub fn get_program_count(&self) -> u64 {
        *self.program_count.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Increment validator counter
    pub fn increment_validators(&self) {
        *self
            .validator_count
            .lock()
            .unwrap_or_else(|e| e.into_inner()) += 1;
    }

    /// Decrement validator counter
    pub fn decrement_validators(&self) {
        let mut count = self
            .validator_count
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *count = count.saturating_sub(1);
    }

    /// Get validator count (no DB scan)
    pub fn get_validator_count(&self) -> u64 {
        *self
            .validator_count
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn set_total_accounts(&self, count: u64) {
        let mut total_accounts = self
            .total_accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *total_accounts = count;
    }

    pub(super) fn set_active_accounts(&self, count: u64) {
        let mut active_accounts = self
            .active_accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *active_accounts = count;
    }

    pub(super) fn set_program_count(&self, count: u64) {
        let mut program_count = self.program_count.lock().unwrap_or_else(|e| e.into_inner());
        *program_count = count;
    }

    pub(super) fn set_validator_count(&self, count: u64) {
        let mut validator_count = self
            .validator_count
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *validator_count = count;
    }

    /// Save metrics to database
    pub fn save(&self, db: &Arc<DB>) -> Result<(), String> {
        let _guard = self
            .persistence_lock
            .lock()
            .map_err(|_| "Metrics persistence lock poisoned".to_string())?;
        let cf = db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let total_txs = *self
            .total_transactions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        db.put_cf(&cf, b"total_transactions", total_txs.to_le_bytes())
            .map_err(|e| format!("Failed to save total transactions: {}", e))?;

        let total_blocks = *self.total_blocks.lock().unwrap_or_else(|e| e.into_inner());
        db.put_cf(&cf, b"total_blocks", total_blocks.to_le_bytes())
            .map_err(|e| format!("Failed to save total blocks: {}", e))?;

        let total_accounts = *self
            .total_accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        db.put_cf(&cf, b"total_accounts", total_accounts.to_le_bytes())
            .map_err(|e| format!("Failed to save total accounts: {}", e))?;

        let active_accounts = *self
            .active_accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        db.put_cf(&cf, b"active_accounts", active_accounts.to_le_bytes())
            .map_err(|e| format!("Failed to save active accounts: {}", e))?;

        let program_count = *self.program_count.lock().unwrap_or_else(|e| e.into_inner());
        db.put_cf(&cf, b"program_count", program_count.to_le_bytes())
            .map_err(|e| format!("Failed to save program count: {}", e))?;

        let validator_count = *self
            .validator_count
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        db.put_cf(&cf, b"validator_count", validator_count.to_le_bytes())
            .map_err(|e| format!("Failed to save validator count: {}", e))?;

        let daily_txs = *self
            .daily_transactions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        db.put_cf(&cf, b"daily_transactions", daily_txs.to_le_bytes())
            .map_err(|e| format!("Failed to save daily transactions: {}", e))?;
        let daily_date = self
            .daily_date
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        db.put_cf(&cf, b"daily_date", daily_date.as_bytes())
            .map_err(|e| format!("Failed to save daily date: {}", e))?;

        Ok(())
    }

    /// STOR-02: Write all metrics counters into an existing WriteBatch for atomic
    /// commit alongside block data. This eliminates the window between block commit
    /// and metrics persistence where a crash could leave counters stale.
    pub fn save_to_batch(&self, batch: &mut WriteBatch, db: &Arc<DB>) -> Result<(), String> {
        let _guard = self
            .persistence_lock
            .lock()
            .map_err(|_| "Metrics persistence lock poisoned".to_string())?;
        self.save_to_batch_locked(batch, db)
    }

    fn save_to_batch_locked(&self, batch: &mut WriteBatch, db: &Arc<DB>) -> Result<(), String> {
        let cf = db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let total_txs = *self
            .total_transactions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        batch.put_cf(&cf, b"total_transactions", total_txs.to_le_bytes());

        let total_blocks = *self.total_blocks.lock().unwrap_or_else(|e| e.into_inner());
        batch.put_cf(&cf, b"total_blocks", total_blocks.to_le_bytes());

        let total_accounts = *self
            .total_accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        batch.put_cf(&cf, b"total_accounts", total_accounts.to_le_bytes());

        let active_accounts = *self
            .active_accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        batch.put_cf(&cf, b"active_accounts", active_accounts.to_le_bytes());

        let program_count = *self.program_count.lock().unwrap_or_else(|e| e.into_inner());
        batch.put_cf(&cf, b"program_count", program_count.to_le_bytes());

        let validator_count = *self
            .validator_count
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        batch.put_cf(&cf, b"validator_count", validator_count.to_le_bytes());

        let daily_txs = *self
            .daily_transactions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        batch.put_cf(&cf, b"daily_transactions", daily_txs.to_le_bytes());

        let daily_date = self
            .daily_date
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        batch.put_cf(&cf, b"daily_date", daily_date.as_bytes());

        Ok(())
    }

    /// Stage a new canonical block's counters in the same durable batch as its
    /// anchor. The returned guard keeps saves serialized until publication.
    pub(crate) fn prepare_block<'a>(
        &'a self,
        block: &'a Block,
        batch: &mut WriteBatch,
        db: &Arc<DB>,
    ) -> Result<PendingBlockMetrics<'a>, String> {
        let guard = self
            .persistence_lock
            .lock()
            .map_err(|_| "Metrics persistence lock poisoned".to_string())?;
        let cf = db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let date = Self::today_utc();
        let observed_at_ms = Self::now_unix_ms();
        let count = block
            .transactions
            .iter()
            .filter(|transaction| !transaction.is_consensus())
            .count() as u64;
        let total_transactions = self
            .total_transactions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .checked_add(count)
            .ok_or_else(|| "Total transaction counter overflow".to_string())?;
        let total_blocks = self
            .total_blocks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .checked_add(1)
            .ok_or_else(|| "Total block counter overflow".to_string())?;
        let daily_count = if Self::date_utc(block.header.timestamp) == date {
            count
        } else {
            0
        };
        let daily_transactions =
            if *self.daily_date.lock().unwrap_or_else(|e| e.into_inner()) == date {
                *self
                    .daily_transactions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
            } else {
                0
            }
            .checked_add(daily_count)
            .ok_or_else(|| "Daily transaction counter overflow".to_string())?;
        self.save_to_batch_locked(batch, db)?;
        batch.put_cf(&cf, b"total_transactions", total_transactions.to_le_bytes());
        batch.put_cf(&cf, b"total_blocks", total_blocks.to_le_bytes());
        batch.put_cf(&cf, b"daily_transactions", daily_transactions.to_le_bytes());
        batch.put_cf(&cf, b"daily_date", date.as_bytes());
        Ok(PendingBlockMetrics {
            metrics: self,
            block,
            observed_at_ms,
            date,
            _guard: guard,
        })
    }
}

impl StateStore {
    /// Apply an independently verified source report under exclusive operator
    /// maintenance. Same evidence/plan retries are no-ops, including after restart;
    /// conflicting plans and changed frontiers/counters abort before any write.
    pub fn apply_metrics_counter_repair(
        &self,
        plan: &MetricsCounterRepair,
    ) -> Result<bool, String> {
        let source = Hash::from_hex(&plan.source_manifest_sha256)?;
        if plan.version != 1
            || source == Hash::default()
            || plan.total_transactions < plan.expected_total_transactions
            || plan.total_blocks < plan.expected_total_blocks
            || plan.tip_slot.checked_add(1) != Some(plan.total_blocks)
            || plan.daily_transactions > plan.total_transactions
        {
            return Err("invalid or non-additive metrics repair plan".to_string());
        }
        let _block_guard = self
            .block_write_lock
            .lock()
            .map_err(|_| "Block write lock poisoned".to_string())?;
        let _metrics_guard = self
            .metrics
            .persistence_lock
            .lock()
            .map_err(|_| "Metrics persistence lock poisoned".to_string())?;
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let mut record_key = b"metrics_counter_repair_v1:".to_vec();
        record_key.extend_from_slice(&source.0);
        let plan_value = serde_json::to_value(plan).map_err(|error| error.to_string())?;
        if let Some(raw) = self
            .db
            .get_cf(&cf, &record_key)
            .map_err(|error| error.to_string())?
        {
            let record: serde_json::Value =
                serde_json::from_slice(&raw).map_err(|error| error.to_string())?;
            if record["plan"] != plan_value {
                return Err("metrics repair evidence already binds a different plan".to_string());
            }
            return Ok(false);
        }
        if plan.daily_date != MetricsStore::today_utc()
            || self.get_last_slot()? != plan.tip_slot
            || self
                .get_block_by_slot(plan.tip_slot)?
                .map(|block| block.hash())
                != Some(plan.tip_hash)
        {
            return Err("metrics repair date or canonical frontier changed".to_string());
        }
        let read_counter = |key: &[u8]| -> Result<u64, String> {
            let raw = self
                .db
                .get_cf(&cf, key)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "metrics repair requires existing durable counters".to_string())?;
            Ok(u64::from_le_bytes(raw.as_slice().try_into().map_err(
                |_| "invalid durable metric counter".to_string(),
            )?))
        };
        if read_counter(b"total_transactions")? != plan.expected_total_transactions
            || read_counter(b"total_blocks")? != plan.expected_total_blocks
            || *self
                .metrics
                .total_transactions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                != plan.expected_total_transactions
            || *self
                .metrics
                .total_blocks
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                != plan.expected_total_blocks
        {
            return Err("metrics repair prior counters changed".to_string());
        }
        let record = serde_json::json!({
            "plan": plan_value,
            "previous_daily_transactions": self.db.get_cf(&cf, b"daily_transactions").map_err(|error| error.to_string())?,
            "previous_daily_date": self.db.get_cf(&cf, b"daily_date").map_err(|error| error.to_string())?,
        });
        let mut batch = WriteBatch::default();
        batch.put_cf(
            &cf,
            b"total_transactions",
            plan.total_transactions.to_le_bytes(),
        );
        batch.put_cf(&cf, b"total_blocks", plan.total_blocks.to_le_bytes());
        batch.put_cf(
            &cf,
            b"daily_transactions",
            plan.daily_transactions.to_le_bytes(),
        );
        batch.put_cf(&cf, b"daily_date", plan.daily_date.as_bytes());
        batch.put_cf(
            &cf,
            record_key,
            serde_json::to_vec(&record).map_err(|error| error.to_string())?,
        );
        let mut options = rocksdb::WriteOptions::default();
        options.set_sync(true);
        self.db
            .write_opt(batch, &options)
            .map_err(|error| error.to_string())?;
        *self
            .metrics
            .total_transactions
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = plan.total_transactions;
        *self
            .metrics
            .total_blocks
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = plan.total_blocks;
        *self
            .metrics
            .daily_transactions
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = plan.daily_transactions;
        *self
            .metrics
            .daily_date
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = plan.daily_date.clone();
        Ok(true)
    }

    /// Get current blockchain metrics
    pub fn get_metrics(&self) -> Metrics {
        let total_burned = self.get_total_burned().unwrap_or(0);
        let total_minted = self.get_total_minted().unwrap_or(0);

        use crate::consensus::GENESIS_SUPPLY_SPORES;
        let total_supply = GENESIS_SUPPLY_SPORES
            .saturating_add(total_minted)
            .saturating_sub(total_burned);

        let total_accounts = self.metrics.get_total_accounts();
        let active_accounts = self.metrics.get_active_accounts();

        self.metrics.get_metrics(
            total_supply,
            total_burned,
            total_minted,
            total_accounts,
            active_accounts,
            self.get_slot_duration_ms(),
        )
    }

    /// Count accounts with non-zero balance (active accounts)
    /// Uses MetricsStore counter — O(1) via atomic counter
    /// Falls back to O(N) scan only during reconciliation
    pub fn count_active_accounts(&self) -> Result<u64, String> {
        Ok(self.metrics.get_active_accounts())
    }

    /// Get deployed program (contract) count — O(1) via MetricsStore counter.
    /// Maintained by `index_program()`.
    pub fn get_program_count(&self) -> u64 {
        self.metrics.get_program_count()
    }

    /// Get validator count — O(1) via MetricsStore counter.
    /// Maintained by `put_validator()` / `delete_validator()`.
    pub fn get_validator_count(&self) -> u64 {
        self.metrics.get_validator_count()
    }

    /// Full O(N) scan of active accounts — ONLY for reconciliation/verification
    fn count_active_accounts_full_scan(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        let mut count = 0u64;
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        for (_, value) in iter.flatten() {
            let maybe_account = if value.first() == Some(&0xBC) {
                deserialize_legacy_bincode::<Account>(&value[1..], "account").ok()
            } else {
                serde_json::from_slice::<Account>(&value).ok()
            };
            if let Some(account) = maybe_account {
                if account.spores > 0 {
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    /// Reconcile account counter with actual database count.
    pub fn reconcile_account_count(&self) -> Result<(), String> {
        let actual_count = self.count_accounts()?;
        self.metrics.set_total_accounts(actual_count);
        self.metrics.save(&self.db)?;
        Ok(())
    }

    /// Reconcile active account count with actual database.
    pub fn reconcile_active_account_count(&self) -> Result<(), String> {
        let actual_count = self.count_active_accounts_full_scan()?;
        self.metrics.set_active_accounts(actual_count);
        self.metrics.save(&self.db)?;
        Ok(())
    }

    /// Reconcile deployed program counter with CF_PROGRAMS.
    pub fn reconcile_program_count(&self) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAMS)
            .ok_or_else(|| "Programs CF not found".to_string())?;
        let mut actual_count = 0u64;
        for _ in self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::Start)
            .flatten()
        {
            actual_count = actual_count.saturating_add(1);
        }
        self.metrics.set_program_count(actual_count);
        self.metrics.save(&self.db)?;
        Ok(())
    }

    /// Reconcile validator counter with CF_VALIDATORS.
    pub fn reconcile_validator_count(&self) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_VALIDATORS)
            .ok_or_else(|| "Validators CF not found".to_string())?;
        let mut actual_count = 0u64;
        for _ in self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::Start)
            .flatten()
        {
            actual_count = actual_count.saturating_add(1);
        }
        self.metrics.set_validator_count(actual_count);
        self.metrics.save(&self.db)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::Hash;

    fn sample_block(slot: u64, timestamp: u64) -> Block {
        Block {
            header: crate::block::BlockHeader {
                slot,
                parent_hash: Hash::default(),
                state_root: Hash::default(),
                tx_root: Hash::default(),
                timestamp,
                validators_hash: Hash::default(),
                validator: [7u8; 32],
                signature: None,
            },
            transactions: Vec::new(),
            tx_fees_paid: Vec::new(),
            oracle_prices: Vec::new(),
            commit_round: 0,
            commit_signatures: Vec::new(),
        }
    }

    fn block_with_oracle_and_consensus(slot: u64) -> Block {
        let mut block = sample_block(slot, MetricsStore::now_unix_ms() / 1000);
        let oracle = Transaction::new(crate::Message::new(
            vec![crate::Instruction {
                program_id: crate::SYSTEM_PROGRAM_ID,
                accounts: vec![Pubkey([7; 32])],
                data: vec![30, 4, b'w', b'B', b'T', b'C', 1, 0, 0, 0, 0, 0, 0, 0, 8],
            }],
            Hash::default(),
        ));
        let mut consensus = oracle.clone();
        consensus.tx_type = crate::transaction::TransactionType::Consensus;
        block.transactions = vec![oracle, consensus];
        block
    }

    #[test]
    fn prepared_block_metrics_survive_restart_before_memory_publication() {
        let temp = tempfile::tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();
        let block = block_with_oracle_and_consensus(1);
        let mut batch = WriteBatch::default();
        assert_eq!(state.get_metrics().total_transactions, 0);
        let pending = state
            .metrics
            .prepare_block(&block, &mut batch, &state.db)
            .unwrap();
        state.db.write(batch).unwrap();
        // Simulate a crash after durable write but before in-memory publication.
        drop(pending);
        drop(state);
        let reopened = StateStore::open(temp.path()).unwrap();
        assert_eq!(reopened.get_metrics().total_transactions, 1);
        assert_eq!(reopened.get_metrics().total_blocks, 1);
        assert_eq!(reopened.get_metrics().daily_transactions, 1);
    }

    #[test]
    fn failed_block_write_does_not_advance_metrics() {
        let temp = tempfile::tempdir().unwrap();
        drop(StateStore::open(temp.path()).unwrap());
        let options = rocksdb::Options::default();
        let families = DB::list_cf(&options, temp.path()).unwrap();
        let db =
            Arc::new(DB::open_cf_for_read_only(&options, temp.path(), families, false).unwrap());
        let metrics = MetricsStore::new();
        let block = block_with_oracle_and_consensus(1);
        let mut batch = WriteBatch::default();
        let pending = metrics.prepare_block(&block, &mut batch, &db).unwrap();
        assert!(db.write(batch).is_err());
        drop(pending);
        let view = metrics.get_metrics(0, 0, 0, 0, 0, 400);
        assert_eq!(view.total_transactions, 0);
        assert_eq!(view.total_blocks, 0);
        assert_eq!(view.daily_transactions, 0);
        assert_eq!(view.last_observed_block_slot, 0);
        drop(db);
        let reopened = StateStore::open(temp.path()).unwrap();
        assert_eq!(reopened.get_metrics().total_transactions, 0);
        assert_eq!(reopened.get_metrics().total_blocks, 0);
    }

    #[test]
    fn block_metrics_publish_serializes_saves_and_resets_daily_count() {
        let temp = tempfile::tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();
        *state.metrics.total_transactions.lock().unwrap() = 20;
        *state.metrics.daily_transactions.lock().unwrap() = 15;
        *state.metrics.daily_date.lock().unwrap() = "2000-01-01".to_string();
        state.metrics.save(&state.db).unwrap();
        let block = block_with_oracle_and_consensus(1);
        let mut batch = WriteBatch::default();
        let pending = state
            .metrics
            .prepare_block(&block, &mut batch, &state.db)
            .unwrap();
        state.db.write(batch).unwrap();
        std::thread::scope(|scope| {
            let (started, ready) = std::sync::mpsc::channel();
            let (finished, done) = std::sync::mpsc::channel();
            let metrics = &state.metrics;
            let db = &state.db;
            let task = scope.spawn(move || {
                started.send(()).unwrap();
                metrics.save(db).unwrap();
                finished.send(()).unwrap();
            });
            ready.recv().unwrap();
            assert!(matches!(
                done.recv_timeout(std::time::Duration::from_millis(50)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ));
            pending.commit();
            done.recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
            task.join().unwrap();
        });
        let view = state.get_metrics();
        assert_eq!(view.total_transactions, 21);
        assert_eq!(view.total_blocks, 1);
        assert_eq!(view.daily_transactions, 1);
        assert_eq!(view.last_observed_block_slot, 1);
        drop(state);
        let reopened = StateStore::open(temp.path()).unwrap();
        assert_eq!(reopened.get_metrics().total_transactions, 21);
        assert_eq!(reopened.get_metrics().daily_transactions, 1);
    }

    #[test]
    fn canonical_metrics_count_once_across_storage_orders_and_replay() {
        for anchor_first in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let state = StateStore::open(temp.path()).unwrap();
            let root_before = state.compute_state_root_with_restrictions_cold_start();
            let mut block = block_with_oracle_and_consensus(1);
            let mut evm = block.transactions[0].clone();
            evm.tx_type = crate::transaction::TransactionType::Evm;
            block.transactions.push(evm);
            state
                .put_tx_meta_full(
                    &block.transactions[2].signature(),
                    &crate::TxMeta {
                        success: Some(false),
                        error: Some("execution reverted".to_string()),
                        ..crate::TxMeta::default()
                    },
                )
                .unwrap();
            if anchor_first {
                state.put_block_atomic(&block, Some(1), Some(1)).unwrap();
            }
            for _ in 0..2 {
                let batch = state.begin_batch_at_slot(1);
                state
                    .commit_batch_with_block(batch, &block, Some(1), Some(1))
                    .unwrap();
                state.put_block_atomic(&block, Some(1), Some(1)).unwrap();
            }
            let mut empty = sample_block(2, block.header.timestamp);
            empty.transactions.clear();
            state.put_block_atomic(&empty, Some(2), Some(2)).unwrap();
            let mut consensus_only = sample_block(3, block.header.timestamp);
            consensus_only.transactions = vec![block.transactions[1].clone()];
            state
                .commit_batch_with_block(
                    state.begin_batch_at_slot(3),
                    &consensus_only,
                    Some(3),
                    Some(3),
                )
                .unwrap();
            assert_eq!(state.get_metrics().total_transactions, 2);
            assert_eq!(state.get_metrics().total_blocks, 3);
            assert_eq!(state.get_metrics().daily_transactions, 2);
            assert_eq!(
                state.compute_state_root_with_restrictions_cold_start(),
                root_before
            );
            drop(state);
            let reopened = StateStore::open(temp.path()).unwrap();
            assert_eq!(reopened.get_metrics().total_transactions, 2);
            assert_eq!(reopened.get_metrics().total_blocks, 3);
            reopened.put_block_atomic(&block, Some(1), Some(1)).unwrap();
            assert_eq!(reopened.get_metrics().total_transactions, 2);
        }
    }

    #[test]
    fn historical_replay_does_not_inflate_today_and_stale_daily_is_hidden() {
        let temp = tempfile::tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();
        let mut old = block_with_oracle_and_consensus(1);
        old.header.timestamp = 1;
        state
            .commit_batch_with_block(state.begin_batch_at_slot(1), &old, Some(1), Some(1))
            .unwrap();
        assert_eq!(state.get_metrics().total_transactions, 1);
        assert_eq!(state.get_metrics().daily_transactions, 0);
        let live = block_with_oracle_and_consensus(2);
        state.put_block_atomic(&live, Some(2), Some(2)).unwrap();
        assert_eq!(state.get_metrics().daily_transactions, 1);
        *state.metrics.daily_date.lock().unwrap() = "2000-01-01".to_string();
        assert_eq!(state.get_metrics().daily_transactions, 0);
    }

    #[test]
    fn canonical_metrics_abort_on_unreadable_existing_slot() {
        let temp = tempfile::tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();
        let cf = state.db.cf_handle(CF_SLOTS).unwrap();
        state
            .db
            .put_cf(&cf, 1u64.to_be_bytes(), b"corrupt")
            .unwrap();
        let block = block_with_oracle_and_consensus(1);
        assert!(state
            .commit_batch_with_block(state.begin_batch_at_slot(1), &block, Some(1), Some(1))
            .is_err());
        assert!(state.put_block_atomic(&block, Some(1), Some(1)).is_err());
        assert_eq!(state.get_metrics().total_transactions, 0);
        assert_eq!(state.get_metrics().total_blocks, 0);
        assert_eq!(
            state.db.get_cf(&cf, 1u64.to_be_bytes()).unwrap().unwrap(),
            b"corrupt"
        );
    }

    #[test]
    fn metrics_repair_is_atomic_additive_and_idempotent_after_restart_and_progress() {
        let temp = tempfile::tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();
        let mut genesis = sample_block(0, 1);
        genesis.transactions.clear();
        state.put_block_atomic(&genesis, Some(0), Some(0)).unwrap();
        let block = block_with_oracle_and_consensus(1);
        state.put_block_atomic(&block, Some(1), Some(1)).unwrap();
        let root = state.compute_state_root_with_restrictions_cold_start();
        *state.metrics.total_transactions.lock().unwrap() = 0;
        *state.metrics.total_blocks.lock().unwrap() = 1;
        *state.metrics.daily_transactions.lock().unwrap() = 0;
        state.metrics.save(&state.db).unwrap();
        let plan = MetricsCounterRepair {
            version: 1,
            source_manifest_sha256: Hash::hash(b"verified fixture history").to_hex(),
            tip_slot: 1,
            tip_hash: block.hash(),
            expected_total_transactions: 0,
            expected_total_blocks: 1,
            total_transactions: 1,
            total_blocks: 2,
            daily_transactions: 1,
            daily_date: MetricsStore::today_utc(),
        };
        for field in [
            "version", "source", "hash", "tip", "date", "counter", "blocks", "daily",
        ] {
            let mut bad = plan.clone();
            match field {
                "version" => bad.version = 2,
                "source" => bad.source_manifest_sha256 = Hash::default().to_hex(),
                "hash" => bad.tip_hash = Hash::default(),
                "tip" => {
                    bad.tip_slot = 2;
                    bad.total_blocks = 3;
                }
                "date" => bad.daily_date = "2000-01-01".to_string(),
                "counter" => bad.expected_total_transactions = 1,
                "blocks" => bad.total_blocks = 1,
                _ => bad.daily_transactions = 2,
            }
            assert!(state.apply_metrics_counter_repair(&bad).is_err(), "{field}");
            assert_eq!(state.get_metrics().total_transactions, 0);
        }
        drop(state);
        let read_only = StateStore::open_read_only_with_cache_mb(temp.path(), Some(8)).unwrap();
        assert!(read_only.apply_metrics_counter_repair(&plan).is_err());
        assert_eq!(read_only.get_metrics().total_transactions, 0);
        drop(read_only);
        let state = StateStore::open(temp.path()).unwrap();
        assert!(state.apply_metrics_counter_repair(&plan).unwrap());
        assert!(!state.apply_metrics_counter_repair(&plan).unwrap());
        assert_eq!(state.get_metrics().total_transactions, 1);
        assert_eq!(state.get_metrics().daily_transactions, 1);
        assert_eq!(
            state.compute_state_root_with_restrictions_cold_start(),
            root
        );
        let mut conflict = plan.clone();
        conflict.total_transactions = 2;
        assert!(state
            .apply_metrics_counter_repair(&conflict)
            .unwrap_err()
            .contains("different plan"));
        drop(state);
        let state = StateStore::open(temp.path()).unwrap();
        assert!(!state.apply_metrics_counter_repair(&plan).unwrap());
        let next = block_with_oracle_and_consensus(2);
        state.put_block_atomic(&next, Some(2), Some(2)).unwrap();
        assert_eq!(state.get_metrics().total_transactions, 2);
        assert!(!state.apply_metrics_counter_repair(&plan).unwrap());
        assert_eq!(state.get_metrics().total_transactions, 2);
    }

    #[test]
    fn observed_cadence_ignores_replay_samples_until_live_head() {
        let metrics = MetricsStore::new();
        let stale_now_secs = MetricsStore::now_unix_ms() / 1000;

        metrics.track_block(&sample_block(10, stale_now_secs.saturating_sub(30)));
        std::thread::sleep(std::time::Duration::from_millis(5));
        metrics.track_block(&sample_block(11, stale_now_secs.saturating_sub(29)));

        let replay_metrics = metrics.get_metrics(0, 0, 0, 0, 0, 400);
        assert_eq!(replay_metrics.observed_block_interval_ms, 0);
        assert_eq!(replay_metrics.head_staleness_ms, 0);

        let live_now_secs = MetricsStore::now_unix_ms() / 1000;
        metrics.track_block(&sample_block(12, live_now_secs));
        std::thread::sleep(std::time::Duration::from_millis(25));
        metrics.track_block(&sample_block(13, MetricsStore::now_unix_ms() / 1000));

        let live_metrics = metrics.get_metrics(0, 0, 0, 0, 0, 400);
        assert!(live_metrics.observed_block_interval_ms > 0);
        assert!(live_metrics.head_staleness_ms < 5_000);
        assert_eq!(live_metrics.last_observed_block_slot, 13);
        assert_eq!(live_metrics.cadence_target_ms, 400);
    }
}
