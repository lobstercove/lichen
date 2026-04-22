use std::collections::VecDeque;

use crate::block::Block;

use super::*;

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
}

/// Metrics tracker with rolling window for TPS
pub struct MetricsStore {
    // Rolling window: (timestamp, tx_count) for last 60 seconds
    window: Mutex<VecDeque<(u64, u64)>>,
    total_transactions: Mutex<u64>,
    total_blocks: Mutex<u64>,
    total_accounts: Mutex<u64>,
    active_accounts: Mutex<u64>,
    // Track block times for average calculation
    last_block_time: Mutex<u64>,
    block_times: Mutex<VecDeque<u64>>,
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

impl Default for MetricsStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MetricsStore {
    pub fn new() -> Self {
        let today = Self::today_utc();
        MetricsStore {
            window: Mutex::new(VecDeque::new()),
            total_transactions: Mutex::new(0),
            total_blocks: Mutex::new(0),
            total_accounts: Mutex::new(0),
            active_accounts: Mutex::new(0),
            last_block_time: Mutex::new(0),
            block_times: Mutex::new(VecDeque::new()),
            peak_tps: Mutex::new(0.0),
            daily_transactions: Mutex::new(0),
            daily_date: Mutex::new(today),
            program_count: Mutex::new(0),
            validator_count: Mutex::new(0),
        }
    }

    /// Get current UTC date as YYYY-MM-DD
    fn today_utc() -> String {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let days = secs / 86400;
        let (year, month, day) = Self::days_to_ymd(days);
        format!("{:04}-{:02}-{:02}", year, month, day)
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
        let tx_count = block.transactions.len() as u64;
        let timestamp = block.header.timestamp;

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
            let today = Self::today_utc();
            let mut daily_date = self.daily_date.lock().unwrap_or_else(|e| e.into_inner());
            let mut daily_txs = self
                .daily_transactions
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if *daily_date != today {
                *daily_date = today;
                *daily_txs = tx_count;
            } else {
                *daily_txs += tx_count;
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
    }

    /// Get current metrics
    pub fn get_metrics(
        &self,
        total_supply: u64,
        total_burned: u64,
        total_minted: u64,
        total_accounts: u64,
        active_accounts: u64,
    ) -> Metrics {
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
            daily_transactions: *self
                .daily_transactions
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        }
    }

    /// Load metrics from database
    pub fn load(&self, db: &Arc<DB>) -> Result<(), String> {
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

    pub(super) fn set_validator_count(&self, count: u64) {
        let mut validator_count = self
            .validator_count
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *validator_count = count;
    }

    /// Save metrics to database
    pub fn save(&self, db: &Arc<DB>) -> Result<(), String> {
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
}

impl StateStore {
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
                bincode::deserialize::<Account>(&value[1..]).ok()
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
}
