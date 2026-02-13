// MoltChain Core - State Management with Column Families

use crate::account::{Account, Pubkey};
use crate::block::Block;
use crate::contract::ContractEvent;
use crate::evm::EvmAccount;
use crate::evm::{EvmReceipt, EvmTxRecord};
use crate::hash::Hash;
use crate::reefstake::ReefStakePool;
use crate::transaction::Transaction;
use alloy_primitives::U256;
use rocksdb::{
    BlockBasedOptions, Cache, ColumnFamilyDescriptor, Direction, Options, SliceTransform,
    WriteBatch, DB,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;

/// Detect number of CPU cores for RocksDB parallelism
fn num_cpus() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(4)
        .min(8) // Cap at 8 to avoid over-subscribing
}

/// Column family names
const CF_ACCOUNTS: &str = "accounts";
const CF_BLOCKS: &str = "blocks";
const CF_TRANSACTIONS: &str = "transactions";
const CF_ACCOUNT_TXS: &str = "account_txs";
const CF_SLOTS: &str = "slots";
const CF_VALIDATORS: &str = "validators";
const CF_STATS: &str = "stats";
const CF_EVM_MAP: &str = "evm_map"; // EVM address → Native pubkey mapping
const CF_EVM_ACCOUNTS: &str = "evm_accounts"; // EVM address → account info
const CF_EVM_STORAGE: &str = "evm_storage"; // EVM address + slot → value
const CF_EVM_TXS: &str = "evm_txs"; // EVM tx hash → metadata
const CF_EVM_RECEIPTS: &str = "evm_receipts"; // EVM tx hash → receipt
const CF_REEFSTAKE: &str = "reefstake"; // ReefStake liquid staking pool
const CF_STAKE_POOL: &str = "stake_pool"; // Validator stake pool
const CF_NFT_BY_OWNER: &str = "nft_by_owner"; // Owner pubkey + token pubkey
const CF_NFT_BY_COLLECTION: &str = "nft_by_collection"; // Collection pubkey + token pubkey
const CF_NFT_ACTIVITY: &str = "nft_activity"; // Collection pubkey + slot + seq + token
const CF_PROGRAMS: &str = "programs"; // Program pubkey
const CF_PROGRAM_CALLS: &str = "program_calls"; // Program pubkey + slot + seq + tx
const CF_MARKET_ACTIVITY: &str = "market_activity"; // Collection pubkey + slot + seq + tx
const CF_SYMBOL_REGISTRY: &str = "symbol_registry"; // Symbol -> program registry
const CF_EVENTS: &str = "events"; // Contract events (program + slot + seq)
const CF_TOKEN_BALANCES: &str = "token_balances"; // Token program + holder -> balance
const CF_TOKEN_TRANSFERS: &str = "token_transfers"; // Token program + slot + seq -> transfer
const CF_TX_BY_SLOT: &str = "tx_by_slot"; // Slot + seq -> tx hash
const CF_TX_TO_SLOT: &str = "tx_to_slot"; // tx hash -> slot (reverse index for O(1) lookup)
const CF_HOLDER_TOKENS: &str = "holder_tokens"; // Holder + token_program -> balance (reverse index)
const CF_SYMBOL_BY_PROGRAM: &str = "symbol_by_program"; // Program pubkey -> symbol (reverse index for O(1) lookup)
const CF_EVENTS_BY_SLOT: &str = "events_by_slot"; // slot(8,BE) + seq(8,BE) -> event_key (secondary index)
const CF_CONTRACT_STORAGE: &str = "contract_storage"; // Contract storage (MoltyID reputation etc.)
const CF_MERKLE_LEAVES: &str = "merkle_leaves"; // pubkey(32) -> leaf_hash(32) (incremental Merkle cache)

/// Token symbol registry entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolRegistryEntry {
    pub symbol: String,
    pub program: Pubkey,
    pub owner: Pubkey,
    pub name: Option<String>,
    pub template: Option<String>,
    pub metadata: Option<Value>,
}

/// Token transfer record stored in CF_TOKEN_TRANSFERS
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenTransfer {
    pub token_program: String,
    pub from: String,
    pub to: String,
    pub amount: u64,
    pub slot: u64,
    pub tx_hash: Option<String>,
}

/// Metrics data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metrics {
    pub tps: f64,
    pub total_transactions: u64,
    pub total_blocks: u64,
    pub average_block_time: f64,
    pub total_accounts: u64,
    pub active_accounts: u64, // Accounts with non-zero balance
    pub total_supply: u64,
    pub total_burned: u64,
    /// Transactions counted since midnight UTC (server-side, same for all)
    pub daily_transactions: u64,
}

/// Metrics tracker with rolling window for TPS
pub struct MetricsStore {
    // Rolling window: (timestamp, tx_count) for last 60 seconds
    window: Mutex<VecDeque<(u64, u64)>>,
    total_transactions: Mutex<u64>,
    total_blocks: Mutex<u64>,
    total_accounts: Mutex<u64>,  // Account counter - no iteration!
    active_accounts: Mutex<u64>, // Accounts with non-zero balance - no iteration!
    // Track block times for average calculation
    last_block_time: Mutex<u64>,
    block_times: Mutex<VecDeque<u64>>, // Last 100 block times
    /// Daily transaction counter (resets at midnight UTC)
    daily_transactions: Mutex<u64>,
    /// Date string (YYYY-MM-DD) for daily counter reset detection
    daily_date: Mutex<String>,
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
            daily_transactions: Mutex::new(0),
            daily_date: Mutex::new(today),
        }
    }

    /// Get current UTC date as YYYY-MM-DD
    fn today_utc() -> String {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let days = secs / 86400;
        // Simple date calc from days since epoch
        let (y, m, d) = Self::days_to_ymd(days);
        format!("{:04}-{:02}-{:02}", y, m, d)
    }

    /// Convert days since Unix epoch to (year, month, day)
    fn days_to_ymd(days: u64) -> (u64, u64, u64) {
        // Civil calendar from days since epoch (Gregorian)
        let z = days + 719468;
        let era = z / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        (y, m, d)
    }

    /// Track a new block
    pub fn track_block(&self, block: &Block) {
        let tx_count = block.transactions.len() as u64;
        let timestamp = block.header.timestamp;

        // Update rolling window
        {
            let mut window = self.window.lock().unwrap_or_else(|e| e.into_inner());
            window.push_back((timestamp, tx_count));

            // Remove entries older than 60 seconds
            // timestamp is in seconds (from block.header.timestamp)
            let cutoff = timestamp.saturating_sub(60);
            while let Some(&(ts, _)) = window.front() {
                if ts < cutoff {
                    window.pop_front();
                } else {
                    break;
                }
            }
        }

        // Update totals
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

        // Update daily transaction counter (reset at midnight UTC)
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

        // Track block time
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
        total_accounts: u64,
        active_accounts: u64,
    ) -> Metrics {
        // Calculate TPS from rolling window
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
            // timestamp is already in seconds, no conversion needed
            (total_txs_in_window as f64) / (time_span as f64)
        } else {
            0.0
        };

        // Get average block time
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
            total_transactions: *self
                .total_transactions
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
            total_blocks: *self.total_blocks.lock().unwrap_or_else(|e| e.into_inner()),
            average_block_time: avg_block_time,
            total_accounts,  // Use provided actual count from DB
            active_accounts, // Use provided active count from DB
            total_supply,
            total_burned,
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

        // Load total transactions
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

        // Load total blocks
        if let Ok(Some(data)) = db.get_cf(&cf, b"total_blocks") {
            if let Ok(bytes) = data.as_slice().try_into() {
                let count = u64::from_le_bytes(bytes);
                let mut total = self.total_blocks.lock().unwrap_or_else(|e| e.into_inner());
                *total = count;
            }
        }

        // Load total accounts
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

        // Load active accounts
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

        // Load daily transactions + date (reset if date changed)
        let today = Self::today_utc();
        let stored_date = db
            .get_cf(&cf, b"daily_date")
            .ok()
            .flatten()
            .and_then(|d| String::from_utf8(d).ok())
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
        // If stored_date != today, daily_transactions stays at 0 (already default)
        {
            let mut dd = self.daily_date.lock().unwrap_or_else(|e| e.into_inner());
            *dd = today;
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

    /// Decrement account counter
    pub fn decrement_accounts(&self) {
        let mut count = self
            .total_accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *count = count.saturating_sub(1);
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

    /// Save metrics to database
    pub fn save(&self, db: &Arc<DB>) -> Result<(), String> {
        let cf = db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        // Save total transactions
        let total_txs = *self
            .total_transactions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        db.put_cf(&cf, b"total_transactions", total_txs.to_le_bytes())
            .map_err(|e| format!("Failed to save total transactions: {}", e))?;

        // Save total blocks
        let total_blocks = *self.total_blocks.lock().unwrap_or_else(|e| e.into_inner());
        db.put_cf(&cf, b"total_blocks", total_blocks.to_le_bytes())
            .map_err(|e| format!("Failed to save total blocks: {}", e))?;

        // Save total accounts
        let total_accounts = *self
            .total_accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        db.put_cf(&cf, b"total_accounts", total_accounts.to_le_bytes())
            .map_err(|e| format!("Failed to save total accounts: {}", e))?;

        // Save active accounts
        let active_accounts = *self
            .active_accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        db.put_cf(&cf, b"active_accounts", active_accounts.to_le_bytes())
            .map_err(|e| format!("Failed to save active accounts: {}", e))?;

        // Save daily transactions + date
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
}

/// State store using RocksDB with column families
#[derive(Clone)]
pub struct StateStore {
    db: Arc<DB>,
    metrics: Arc<MetricsStore>,
}

/// Atomic write batch for transaction processing (T1.4/T3.1).
///
/// Accumulates all state mutations (accounts, transactions, pools, etc.) in
/// memory. Nothing is written to RocksDB until `commit()` is called, which
/// flushes everything in a single atomic `WriteBatch`. If the batch is dropped
/// without committing, all mutations are discarded (implicit rollback).
///
/// The overlay `HashMap` ensures reads-after-writes within the same transaction
/// see the updated values without hitting disk.
pub struct StateBatch {
    /// The underlying RocksDB WriteBatch (accumulates puts)
    batch: WriteBatch,
    /// In-memory overlay for accounts modified in this batch.
    /// Reads check here first, then fall through to on-disk state.
    account_overlay: std::collections::HashMap<Pubkey, Account>,
    /// In-memory overlay for stake pool (set on put, read on get)
    stake_pool_overlay: Option<crate::consensus::StakePool>,
    /// In-memory overlay for ReefStake pool (set on put, read on get)
    reefstake_pool_overlay: Option<ReefStakePool>,
    /// Metric deltas accumulated during the batch (applied on commit)
    new_accounts: i64,
    active_account_delta: i64,
    /// Accumulated burned amount delta (applied atomically on commit)
    burned_delta: u64,
    /// AUDIT-FIX 1.15: Track NFT token_ids indexed within this batch for TOCTOU-safe uniqueness
    nft_token_id_overlay: std::collections::HashSet<Vec<u8>>,
    /// Auto-incrementing sequence counter for event key uniqueness (T2.13)
    event_seq: u64,
    /// Reference to the DB (needed for cf_handle lookups during put)
    db: Arc<DB>,
}

impl StateStore {
    /// Open or create state database with production-tuned column families.
    ///
    /// Each CF gets custom Options based on its access pattern:
    /// - Point-lookup CFs (accounts, transactions, blocks): bloom filters, larger block cache
    /// - Prefix-scan CFs (account_txs, nft_*, program_calls): prefix bloom + prefix extractor
    /// - Small/singleton CFs (stats, validators, stake_pool): minimal config
    /// - Write-heavy CFs (events, token_transfers): larger write buffers
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        // ── Global DB options ────────────────────────────────────────
        let mut db_opts = Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);
        db_opts.set_max_open_files(512);
        db_opts.set_keep_log_file_num(5);
        db_opts.set_max_total_wal_size(256 * 1024 * 1024); // 256MB WAL limit
        db_opts.set_bytes_per_sync(1024 * 1024); // 1MB sync granularity
        db_opts.increase_parallelism(num_cpus());
        db_opts.set_max_background_jobs(4);

        // ── Shared block cache: 512 MB LRU ───────────────────────────
        let shared_cache = Cache::new_lru_cache(512 * 1024 * 1024);

        // ── Helper closures for CF option presets ─────────────────────

        // Point-lookup CF: bloom filter, large blocks, shared cache
        let point_lookup_opts = |prefix_len: usize| -> Options {
            let mut opts = Options::default();
            opts.set_compression_type(rocksdb::DBCompressionType::Lz4);
            let mut bbo = BlockBasedOptions::default();
            bbo.set_bloom_filter(10.0, false);
            bbo.set_block_cache(&shared_cache);
            bbo.set_block_size(16 * 1024); // 16KB blocks
            bbo.set_cache_index_and_filter_blocks(true);
            bbo.set_pin_l0_filter_and_index_blocks_in_cache(true);
            opts.set_block_based_table_factory(&bbo);
            opts.set_write_buffer_size(64 * 1024 * 1024); // 64MB write buffer
            opts.set_max_write_buffer_number(3);
            opts.set_min_write_buffer_number_to_merge(2);
            opts.set_level_compaction_dynamic_level_bytes(true);
            opts.set_target_file_size_base(64 * 1024 * 1024); // 64MB SST files
            if prefix_len > 0 {
                opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(prefix_len));
            }
            opts
        };

        // Prefix-scan CF: prefix bloom + extractor for efficient prefix iteration
        let prefix_scan_opts = |prefix_len: usize| -> Options {
            let mut opts = Options::default();
            opts.set_compression_type(rocksdb::DBCompressionType::Lz4);
            let mut bbo = BlockBasedOptions::default();
            bbo.set_bloom_filter(10.0, false);
            bbo.set_block_cache(&shared_cache);
            bbo.set_block_size(16 * 1024);
            bbo.set_cache_index_and_filter_blocks(true);
            bbo.set_pin_l0_filter_and_index_blocks_in_cache(true);
            opts.set_block_based_table_factory(&bbo);
            opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(prefix_len));
            opts.set_memtable_prefix_bloom_ratio(0.1);
            opts.set_write_buffer_size(32 * 1024 * 1024); // 32MB
            opts.set_max_write_buffer_number(3);
            opts.set_min_write_buffer_number_to_merge(2);
            opts.set_level_compaction_dynamic_level_bytes(true);
            opts.set_target_file_size_base(64 * 1024 * 1024);
            opts
        };

        // Write-heavy CF: larger write buffers, universal compaction
        let write_heavy_opts = |prefix_len: usize| -> Options {
            let mut opts = Options::default();
            opts.set_compression_type(rocksdb::DBCompressionType::Lz4);
            let mut bbo = BlockBasedOptions::default();
            bbo.set_bloom_filter(10.0, false);
            bbo.set_block_cache(&shared_cache);
            bbo.set_block_size(16 * 1024);
            bbo.set_cache_index_and_filter_blocks(true);
            opts.set_block_based_table_factory(&bbo);
            opts.set_write_buffer_size(128 * 1024 * 1024); // 128MB write buffer
            opts.set_max_write_buffer_number(4);
            opts.set_min_write_buffer_number_to_merge(2);
            opts.set_level_compaction_dynamic_level_bytes(true);
            opts.set_target_file_size_base(128 * 1024 * 1024); // 128MB SSTs
            if prefix_len > 0 {
                opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(prefix_len));
            }
            opts
        };

        // Small/singleton CF: minimal resources
        let small_cf_opts = || -> Options {
            let mut opts = Options::default();
            opts.set_compression_type(rocksdb::DBCompressionType::Lz4);
            let mut bbo = BlockBasedOptions::default();
            bbo.set_block_cache(&shared_cache);
            opts.set_block_based_table_factory(&bbo);
            opts.set_write_buffer_size(4 * 1024 * 1024); // 4MB
            opts.set_max_write_buffer_number(2);
            opts
        };

        // Cold/archival CF: Zstd compression for space efficiency
        let archival_opts = |prefix_len: usize| -> Options {
            let mut opts = Options::default();
            opts.set_compression_type(rocksdb::DBCompressionType::Zstd);
            let mut bbo = BlockBasedOptions::default();
            bbo.set_bloom_filter(10.0, false);
            bbo.set_block_cache(&shared_cache);
            bbo.set_block_size(32 * 1024); // 32KB blocks (compress better)
            bbo.set_cache_index_and_filter_blocks(true);
            opts.set_block_based_table_factory(&bbo);
            opts.set_write_buffer_size(32 * 1024 * 1024);
            opts.set_max_write_buffer_number(2);
            opts.set_level_compaction_dynamic_level_bytes(true);
            opts.set_target_file_size_base(128 * 1024 * 1024);
            if prefix_len > 0 {
                opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(prefix_len));
            }
            opts
        };

        // ── Column family definitions with tuned options ─────────────
        let cfs = vec![
            // Hot point-lookup CFs (1M+ entries expected)
            ColumnFamilyDescriptor::new(CF_ACCOUNTS, point_lookup_opts(32)), // key=pubkey(32)
            ColumnFamilyDescriptor::new(CF_TRANSACTIONS, point_lookup_opts(32)), // key=hash(32)
            ColumnFamilyDescriptor::new(CF_BLOCKS, point_lookup_opts(32)),   // key=hash(32)
            ColumnFamilyDescriptor::new(CF_TX_TO_SLOT, point_lookup_opts(32)), // key=hash(32)
            ColumnFamilyDescriptor::new(CF_SYMBOL_BY_PROGRAM, point_lookup_opts(32)), // key=pubkey(32)
            // Prefix-scan CFs (32-byte pubkey prefix)
            ColumnFamilyDescriptor::new(CF_ACCOUNT_TXS, prefix_scan_opts(32)), // key=pubkey(32)+slot+seq+hash
            ColumnFamilyDescriptor::new(CF_NFT_BY_OWNER, prefix_scan_opts(32)), // key=owner(32)+token(32)
            ColumnFamilyDescriptor::new(CF_NFT_BY_COLLECTION, prefix_scan_opts(32)), // key=collection(32)+token(32)
            ColumnFamilyDescriptor::new(CF_NFT_ACTIVITY, prefix_scan_opts(32)), // key=collection(32)+slot+seq
            ColumnFamilyDescriptor::new(CF_PROGRAM_CALLS, prefix_scan_opts(32)), // key=program(32)+slot+seq+hash
            ColumnFamilyDescriptor::new(CF_MARKET_ACTIVITY, prefix_scan_opts(32)), // key=collection(32)+slot+seq
            ColumnFamilyDescriptor::new(CF_TOKEN_BALANCES, prefix_scan_opts(32)), // key=token(32)+holder(32)
            ColumnFamilyDescriptor::new(CF_HOLDER_TOKENS, prefix_scan_opts(32)), // key=holder(32)+token(32)
            ColumnFamilyDescriptor::new(CF_TOKEN_TRANSFERS, prefix_scan_opts(32)), // key=token(32)+slot+seq
            ColumnFamilyDescriptor::new(CF_EVENTS, prefix_scan_opts(32)), // key=program(32)+slot+seq
            // Prefix-scan CFs (8-byte slot prefix)
            ColumnFamilyDescriptor::new(CF_TX_BY_SLOT, prefix_scan_opts(8)), // key=slot(8)+seq(8)
            ColumnFamilyDescriptor::new(CF_EVENTS_BY_SLOT, prefix_scan_opts(8)), // key=slot(8)+program(32)+seq(8)
            // Write-heavy archival CFs
            ColumnFamilyDescriptor::new(CF_EVM_TXS, archival_opts(32)), // key=evm_hash
            ColumnFamilyDescriptor::new(CF_EVM_RECEIPTS, archival_opts(32)), // key=evm_hash
            // EVM CFs with 20-byte address prefix
            ColumnFamilyDescriptor::new(CF_EVM_ACCOUNTS, point_lookup_opts(20)), // key=evm_addr(20)
            ColumnFamilyDescriptor::new(CF_EVM_MAP, point_lookup_opts(20)),      // key=evm_addr(20)
            ColumnFamilyDescriptor::new(CF_EVM_STORAGE, prefix_scan_opts(20)), // key=evm_addr(20)+slot(32)
            // Small/singleton CFs
            ColumnFamilyDescriptor::new(CF_SLOTS, small_cf_opts()),
            ColumnFamilyDescriptor::new(CF_STATS, write_heavy_opts(0)), // many per-slot seq counters + per-account atomic counters
            ColumnFamilyDescriptor::new(CF_VALIDATORS, small_cf_opts()),
            ColumnFamilyDescriptor::new(CF_REEFSTAKE, small_cf_opts()),
            ColumnFamilyDescriptor::new(CF_STAKE_POOL, small_cf_opts()),
            ColumnFamilyDescriptor::new(CF_PROGRAMS, point_lookup_opts(32)),
            ColumnFamilyDescriptor::new(CF_SYMBOL_REGISTRY, small_cf_opts()),
            ColumnFamilyDescriptor::new(CF_CONTRACT_STORAGE, prefix_scan_opts(32)),
            // Incremental Merkle leaf cache
            ColumnFamilyDescriptor::new(CF_MERKLE_LEAVES, point_lookup_opts(32)), // key=pubkey(32)->leaf_hash(32)
        ];

        let db = DB::open_cf_descriptors(&db_opts, path, cfs)
            .map_err(|e| format!("Failed to open database: {}", e))?;

        let db_arc = Arc::new(db);
        let metrics = Arc::new(MetricsStore::new());

        // Load existing metrics from database
        metrics.load(&db_arc)?;

        Ok(StateStore {
            db: db_arc,
            metrics,
        })
    }

    /// Get the last processed slot
    pub fn get_last_slot(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        match self.db.get_cf(&cf, b"last_slot") {
            Ok(Some(data)) => {
                let bytes: [u8; 8] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid slot data".to_string())?;
                Ok(u64::from_le_bytes(bytes))
            }
            Ok(None) => Ok(0),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Update the last processed slot
    pub fn set_last_slot(&self, slot: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        self.db
            .put_cf(&cf, b"last_slot", slot.to_le_bytes())
            .map_err(|e| format!("Failed to store slot: {}", e))
    }

    /// Get the hashes of the last N blocks for replay protection.
    /// Returns a set of block hashes from the most recent `count` slots.
    /// T1.3 fix: Hash::default() is NO LONGER accepted. Only real block hashes
    /// are valid for replay protection. Genesis block hash is included if in range.
    pub fn get_recent_blockhashes(
        &self,
        count: u64,
    ) -> Result<std::collections::HashSet<Hash>, String> {
        let mut hashes = std::collections::HashSet::new();

        let last_slot = self.get_last_slot()?;
        let start_slot = last_slot.saturating_sub(count);
        for slot in start_slot..=last_slot {
            if let Ok(Some(block)) = self.get_block_by_slot(slot) {
                hashes.insert(block.hash());
            }
        }

        Ok(hashes)
    }

    /// Store a block
    pub fn put_block(&self, block: &Block) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_BLOCKS)
            .ok_or_else(|| "Blocks CF not found".to_string())?;

        let block_hash = block.hash();
        let value =
            serde_json::to_vec(block).map_err(|e| format!("Failed to serialize block: {}", e))?;

        // Check if this is a new slot BEFORE writing the slot index
        // (otherwise the lookup finds our own write and metrics are never tracked)
        let is_new_slot = self
            .get_block_by_slot(block.header.slot)
            .unwrap_or(None)
            .is_none();

        self.db
            .put_cf(&cf, block_hash.0, &value)
            .map_err(|e| format!("Failed to store block: {}", e))?;

        // Also index by slot
        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        self.db
            .put_cf(&slot_cf, block.header.slot.to_le_bytes(), block_hash.0)
            .map_err(|e| format!("Failed to index slot: {}", e))?;

        self.index_account_transactions(block)?;

        // Store each transaction individually + index for O(1) lookup
        for tx in &block.transactions {
            let sig = tx.signature();
            // Store tx body in CF_TRANSACTIONS so getTransaction RPC can find it
            if let Err(e) = self.put_transaction(tx) {
                eprintln!("Warning: failed to store tx {}: {}", sig.to_hex(), e);
            }
            if let Err(e) = self.index_tx_to_slot(&sig, block.header.slot) {
                // Non-fatal: log but don't fail block storage
                eprintln!(
                    "Warning: failed to index tx {} to slot: {}",
                    sig.to_hex(),
                    e
                );
            }
        }

        // Track metrics for new slots (skip fork-choice replacements)
        if is_new_slot {
            self.metrics.track_block(block);
            self.metrics.save(&self.db)?;
        }

        Ok(())
    }

    /// Get block by hash
    pub fn get_block(&self, hash: &Hash) -> Result<Option<Block>, String> {
        let cf = self
            .db
            .cf_handle(CF_BLOCKS)
            .ok_or_else(|| "Blocks CF not found".to_string())?;

        match self.db.get_cf(&cf, hash.0) {
            Ok(Some(data)) => {
                let block: Block = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to deserialize block: {}", e))?;
                Ok(Some(block))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Get block by slot
    pub fn get_block_by_slot(&self, slot: u64) -> Result<Option<Block>, String> {
        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        match self.db.get_cf(&slot_cf, slot.to_le_bytes()) {
            Ok(Some(hash_bytes)) => {
                let mut hash = [0u8; 32];
                hash.copy_from_slice(&hash_bytes);
                self.get_block(&Hash(hash))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Store a transaction
    pub fn put_transaction(&self, tx: &Transaction) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;

        let sig = tx.signature();
        let value = serde_json::to_vec(tx)
            .map_err(|e| format!("Failed to serialize transaction: {}", e))?;

        self.db
            .put_cf(&cf, sig.0, &value)
            .map_err(|e| format!("Failed to store transaction: {}", e))
    }

    /// Get transaction by signature
    pub fn get_transaction(&self, sig: &Hash) -> Result<Option<Transaction>, String> {
        let cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;

        match self.db.get_cf(&cf, sig.0) {
            Ok(Some(data)) => {
                let tx: Transaction = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to deserialize transaction: {}", e))?;
                Ok(Some(tx))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Delete transaction record (used during fork choice to allow re-replay)
    pub fn delete_transaction(&self, sig: &Hash) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;

        self.db
            .delete_cf(&cf, sig.0)
            .map_err(|e| format!("Failed to delete transaction: {}", e))
    }

    /// Get account by pubkey
    pub fn get_account(&self, pubkey: &Pubkey) -> Result<Option<Account>, String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        match self.db.get_cf(&cf, pubkey.0) {
            Ok(Some(data)) => {
                let mut account: Account = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to deserialize account: {}", e))?;
                account.fixup_legacy(); // M11 fix: repair legacy accounts missing balance separation
                Ok(Some(account))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Store account
    pub fn put_account(&self, pubkey: &Pubkey, account: &Account) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        // Check existing account for counter updates
        let old_account = self
            .db
            .get_cf(&cf, pubkey.0)
            .map_err(|e| format!("Failed to check account: {}", e))?;

        let old_balance = old_account
            .as_ref()
            .and_then(|data| serde_json::from_slice::<Account>(data).ok())
            .map(|a| a.shells)
            .unwrap_or(0);
        let is_new = old_account.is_none();
        let new_balance = account.shells;

        let value = serde_json::to_vec(account)
            .map_err(|e| format!("Failed to serialize account: {}", e))?;

        self.db
            .put_cf(&cf, pubkey.0, &value)
            .map_err(|e| format!("Failed to store account: {}", e))?;

        // Update counters
        let mut needs_save = false;
        if is_new {
            self.metrics.increment_accounts();
            needs_save = true;
        }
        // Track active accounts (non-zero balance transitions)
        if old_balance == 0 && new_balance > 0 {
            self.metrics.increment_active_accounts();
            needs_save = true;
        } else if old_balance > 0 && new_balance == 0 {
            self.metrics.decrement_active_accounts();
            needs_save = true;
        }
        if needs_save {
            self.metrics.save(&self.db)?;
        }

        // Mark state root as dirty with pubkey for incremental Merkle
        self.mark_account_dirty_with_key(pubkey);

        Ok(())
    }

    /// Compute state root hash using **incremental** sparse Merkle tree.
    ///
    /// Instead of scanning ALL accounts O(N), this:
    /// 1. Reads the dirty-set from CF_STATS ("dirty_keys:..." entries)
    /// 2. Recomputes only the leaf hashes for those accounts
    /// 3. Updates the cached leaf hashes in CF_MERKLE_LEAVES
    /// 4. Rebuilds the Merkle tree from the cached leaves
    ///
    /// At 1M accounts, this turns a full O(N) scan into O(dirty_count + N_leaves_read)
    /// where dirty_count is typically tiny (transactions per block ~10-100).
    pub fn compute_state_root(&self) -> Hash {
        let cf_accounts = match self.db.cf_handle(CF_ACCOUNTS) {
            Some(handle) => handle,
            None => return Hash::default(),
        };
        let cf_leaves = match self.db.cf_handle(CF_MERKLE_LEAVES) {
            Some(handle) => handle,
            None => return self.compute_state_root_full_scan(), // fallback first time
        };
        let cf_stats = match self.db.cf_handle(CF_STATS) {
            Some(handle) => handle,
            None => return self.compute_state_root_full_scan(),
        };

        // Check if we have a populated leaf cache (merkle_leaf_count > 0)
        let leaf_count = match self.db.get_cf(&cf_stats, b"merkle_leaf_count") {
            Ok(Some(data)) if data.len() == 8 => {
                u64::from_le_bytes(data.as_slice().try_into().unwrap_or([0; 8]))
            }
            _ => 0,
        };

        if leaf_count == 0 {
            // Cold start: populate entire leaf cache
            return self.compute_state_root_cold_start();
        }

        // Read dirty account keys from CF_STATS: "dirty_acct:{pubkey}" -> []
        let dirty_prefix = b"dirty_acct:";
        let iter = self.db.iterator_cf(
            &cf_stats,
            rocksdb::IteratorMode::From(dirty_prefix, Direction::Forward),
        );

        let mut dirty_keys: Vec<[u8; 32]> = Vec::new();
        for item in iter.flatten() {
            let (key, _) = item;
            if !key.starts_with(dirty_prefix) {
                break;
            }
            if key.len() == dirty_prefix.len() + 32 {
                let mut pk = [0u8; 32];
                pk.copy_from_slice(&key[dirty_prefix.len()..]);
                dirty_keys.push(pk);
            }
        }

        // Recompute leaf hashes for dirty accounts and update leaf cache
        let mut batch = WriteBatch::default();
        for pk in &dirty_keys {
            match self.db.get_cf(&cf_accounts, pk) {
                Ok(Some(value)) => {
                    // Account exists: H(pubkey || account_bytes)
                    let mut data = Vec::with_capacity(32 + value.len());
                    data.extend_from_slice(pk);
                    data.extend_from_slice(&value);
                    let leaf = Hash::hash(&data);
                    batch.put_cf(&cf_leaves, pk, leaf.0);
                }
                Ok(None) => {
                    // Account deleted: remove from leaf cache
                    batch.delete_cf(&cf_leaves, pk);
                }
                Err(_) => continue,
            }
            // Remove dirty marker
            let mut dirty_key = Vec::with_capacity(dirty_prefix.len() + 32);
            dirty_key.extend_from_slice(dirty_prefix);
            dirty_key.extend_from_slice(pk);
            batch.delete_cf(&cf_stats, &dirty_key);
        }
        // Reset dirty count
        batch.put_cf(&cf_stats, b"dirty_account_count", 0u64.to_le_bytes());

        if let Err(e) = self.db.write(batch) {
            eprintln!("Warning: failed to write Merkle leaf updates: {}", e);
            return self.compute_state_root_full_scan();
        }

        // Rebuild Merkle tree from all cached leaves (already sorted by pubkey in RocksDB)
        let mut leaves: Vec<Hash> = Vec::new();
        let iter = self
            .db
            .iterator_cf(&cf_leaves, rocksdb::IteratorMode::Start);
        for item in iter.flatten() {
            let (_, value) = item;
            if value.len() == 32 {
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&value);
                leaves.push(Hash(bytes));
            }
        }

        if leaves.is_empty() {
            return Hash::default();
        }

        let root = Self::merkle_root_from_leaves(&leaves);

        // Cache the root
        let _ = self.db.put_cf(&cf_stats, b"cached_state_root", root.0);

        root
    }

    /// Full scan state root computation — used for cold start and fallback.
    /// Populates CF_MERKLE_LEAVES so subsequent calls are incremental.
    fn compute_state_root_cold_start(&self) -> Hash {
        let cf_accounts = match self.db.cf_handle(CF_ACCOUNTS) {
            Some(h) => h,
            None => return Hash::default(),
        };
        let cf_leaves = match self.db.cf_handle(CF_MERKLE_LEAVES) {
            Some(h) => h,
            None => return self.compute_state_root_full_scan(),
        };

        let mut leaves: Vec<Hash> = Vec::new();
        let mut batch = WriteBatch::default();
        let mut count = 0u64;

        let iter = self
            .db
            .iterator_cf(&cf_accounts, rocksdb::IteratorMode::Start);
        for item in iter.flatten() {
            let (key, value) = item;
            let mut data = Vec::with_capacity(key.len() + value.len());
            data.extend_from_slice(&key);
            data.extend_from_slice(&value);
            let leaf = Hash::hash(&data);
            leaves.push(leaf);
            batch.put_cf(&cf_leaves, &*key, leaf.0);
            count += 1;
        }

        if leaves.is_empty() {
            return Hash::default();
        }

        // Store leaf count so we know the cache is populated
        if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
            batch.put_cf(&cf_stats, b"merkle_leaf_count", count.to_le_bytes());
            batch.put_cf(&cf_stats, b"dirty_account_count", 0u64.to_le_bytes());
        }
        let _ = self.db.write(batch);

        let root = Self::merkle_root_from_leaves(&leaves);

        if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
            let _ = self.db.put_cf(&cf_stats, b"cached_state_root", root.0);
        }

        root
    }

    /// Legacy O(N) full scan — fallback only when CF_MERKLE_LEAVES is unavailable
    fn compute_state_root_full_scan(&self) -> Hash {
        let cf = match self.db.cf_handle(CF_ACCOUNTS) {
            Some(handle) => handle,
            None => return Hash::default(),
        };

        let mut leaves: Vec<Hash> = Vec::new();
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        for (key, value) in iter.flatten() {
            let mut data = Vec::with_capacity(key.len() + value.len());
            data.extend_from_slice(&key);
            data.extend_from_slice(&value);
            leaves.push(Hash::hash(&data));
        }

        if leaves.is_empty() {
            return Hash::default();
        }

        let root = Self::merkle_root_from_leaves(&leaves);

        // Cache the computed root and reset dirty counter
        if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
            let _ = self.db.put_cf(&cf_stats, b"cached_state_root", root.0);
            let _ = self
                .db
                .put_cf(&cf_stats, b"dirty_account_count", 0u64.to_le_bytes());
        }

        root
    }

    /// Build a Merkle root from a sorted list of leaf hashes
    /// Uses binary tree: pair adjacent leaves, hash pairs, repeat until single root
    fn merkle_root_from_leaves(leaves: &[Hash]) -> Hash {
        if leaves.is_empty() {
            return Hash::default();
        }
        if leaves.len() == 1 {
            return leaves[0];
        }

        let mut level = leaves.to_vec();
        while level.len() > 1 {
            let mut next_level = Vec::with_capacity(level.len().div_ceil(2));
            for pair in level.chunks(2) {
                if pair.len() == 2 {
                    let mut combined = [0u8; 64];
                    combined[..32].copy_from_slice(&pair[0].0);
                    combined[32..].copy_from_slice(&pair[1].0);
                    next_level.push(Hash::hash(&combined));
                } else {
                    // L1 fix: rehash odd leaf with itself instead of promoting verbatim
                    // Prevents CVE-2012-2459 class attacks (Merkle 2nd preimage)
                    let mut combined = [0u8; 64];
                    combined[..32].copy_from_slice(&pair[0].0);
                    combined[32..].copy_from_slice(&pair[0].0);
                    next_level.push(Hash::hash(&combined));
                }
            }
            level = next_level;
        }

        level[0]
    }

    /// Fast state root check: returns cached root if no accounts modified since last computation
    pub fn compute_state_root_cached(&self) -> Hash {
        if let Some(cf) = self.db.cf_handle(CF_STATS) {
            // Check dirty counter
            let dirty = match self.db.get_cf(&cf, b"dirty_account_count") {
                Ok(Some(data)) if data.len() == 8 => {
                    u64::from_le_bytes(data.as_slice().try_into().unwrap_or([0; 8]))
                }
                _ => 1, // Assume dirty if unknown
            };

            if dirty == 0 {
                // Return cached root
                if let Ok(Some(data)) = self.db.get_cf(&cf, b"cached_state_root") {
                    if data.len() == 32 {
                        let mut bytes = [0u8; 32];
                        bytes.copy_from_slice(&data);
                        return Hash(bytes);
                    }
                }
            }
        }

        // Dirty or no cache — full Merkle recomputation
        self.compute_state_root()
    }

    /// Mark that an account was modified (tracks dirty set for incremental Merkle).
    /// Writes "dirty_acct:{pubkey}" -> [] in CF_STATS so compute_state_root()
    /// knows which leaves need recomputation.
    pub fn mark_account_dirty_with_key(&self, pubkey: &Pubkey) {
        if let Some(cf) = self.db.cf_handle(CF_STATS) {
            // Add to dirty set: "dirty_acct:" + pubkey(32)
            let mut key = Vec::with_capacity(11 + 32);
            key.extend_from_slice(b"dirty_acct:");
            key.extend_from_slice(&pubkey.0);
            let _ = self.db.put_cf(&cf, &key, []);

            // Increment dirty counter
            let current = match self.db.get_cf(&cf, b"dirty_account_count") {
                Ok(Some(data)) if data.len() == 8 => {
                    let bytes: [u8; 8] = data.as_slice().try_into().unwrap_or([0; 8]);
                    u64::from_le_bytes(bytes)
                }
                _ => 0,
            };
            let _ = self
                .db
                .put_cf(&cf, b"dirty_account_count", (current + 1).to_le_bytes());
        }
    }

    /// Legacy mark_account_dirty (no pubkey) — increments counter only.
    /// Prefer mark_account_dirty_with_key() for incremental Merkle support.
    pub fn mark_account_dirty(&self) {
        if let Some(cf) = self.db.cf_handle(CF_STATS) {
            let current = match self.db.get_cf(&cf, b"dirty_account_count") {
                Ok(Some(data)) if data.len() == 8 => {
                    let bytes: [u8; 8] = data.as_slice().try_into().unwrap_or([0; 8]);
                    u64::from_le_bytes(bytes)
                }
                _ => 0,
            };
            let _ = self
                .db
                .put_cf(&cf, b"dirty_account_count", (current + 1).to_le_bytes());
        }
    }

    /// Get current blockchain metrics
    pub fn get_metrics(&self) -> Metrics {
        // Get total burned
        let total_burned = self.get_total_burned().unwrap_or(0);

        // Calculate total supply: initial supply (1B MOLT in shells) minus burned
        // 1 MOLT = 1_000_000_000 shells, so 1B MOLT = 1_000_000_000_000_000_000 shells
        const INITIAL_SUPPLY_SHELLS: u64 = 1_000_000_000_000_000_000; // 1B MOLT
        let total_supply = INITIAL_SUPPLY_SHELLS.saturating_sub(total_burned);

        // Use incremental counters — NO full DB scans
        let total_accounts = self.metrics.get_total_accounts();
        let active_accounts = self.metrics.get_active_accounts();

        self.metrics
            .get_metrics(total_supply, total_burned, total_accounts, active_accounts)
    }

    /// Count total number of accounts (DEPRECATED - use metrics counter instead)
    /// This method is kept for migration/verification purposes only
    #[allow(dead_code)]
    pub fn count_accounts(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        let mut count = 0u64;
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        for _ in iter {
            count += 1;
        }

        Ok(count)
    }

    /// Count accounts with non-zero balance (active accounts)
    /// Uses MetricsStore counter — O(1) via atomic counter
    /// Falls back to O(N) scan only during reconciliation
    pub fn count_active_accounts(&self) -> Result<u64, String> {
        Ok(self.metrics.get_active_accounts())
    }

    /// Full O(N) scan of active accounts — ONLY for reconciliation/verification
    #[allow(dead_code)]
    fn count_active_accounts_full_scan(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        let mut count = 0u64;
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        for (_, value) in iter.flatten() {
            if let Ok(account) = serde_json::from_slice::<Account>(&value) {
                if account.shells > 0 {
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    /// Reconcile account counter with actual database count
    /// Use this to fix discrepancies between counter and reality
    pub fn reconcile_account_count(&self) -> Result<(), String> {
        let actual_count = self.count_accounts()?;
        let mut counter = self
            .metrics
            .total_accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *counter = actual_count;
        self.metrics.save(&self.db)?;
        Ok(())
    }

    /// Get account balance in shells
    pub fn get_balance(&self, pubkey: &Pubkey) -> Result<u64, String> {
        match self.get_account(pubkey)? {
            Some(account) => Ok(account.shells),
            None => Ok(0),
        }
    }

    /// Get reputation score for an account.
    /// Reads from the MoltyID contract storage (reputation key format: "rep:" + hex(pubkey)).
    /// Returns 0 if no reputation data found.
    pub fn get_reputation(&self, pubkey: &Pubkey) -> Result<u64, String> {
        // Build the MoltyID reputation key: "rep:" + hex(pubkey)
        let hex_chars: &[u8; 16] = b"0123456789abcdef";
        let mut key = Vec::with_capacity(4 + 64);
        key.extend_from_slice(b"rep:");
        for &b in pubkey.0.iter() {
            key.push(hex_chars[(b >> 4) as usize]);
            key.push(hex_chars[(b & 0x0f) as usize]);
        }
        // Try to read from the contract storage column family
        let cf = match self.db.cf_handle(CF_CONTRACT_STORAGE) {
            Some(cf) => cf,
            None => return Ok(0), // No contract storage CF = no reputation data
        };
        match self.db.get_cf(&cf, &key) {
            Ok(Some(data)) if data.len() >= 8 => {
                let mut arr = [0u8; 8];
                arr.copy_from_slice(&data[..8]);
                Ok(u64::from_le_bytes(arr))
            }
            _ => Ok(0),
        }
    }

    /// Transfer shells between accounts
    pub fn transfer(&self, from: &Pubkey, to: &Pubkey, shells: u64) -> Result<(), String> {
        if from == to {
            return Ok(());
        }

        // Get sender account
        let mut from_account = self
            .get_account(from)?
            .ok_or_else(|| "Sender account not found".to_string())?;

        // Check and deduct spendable balance
        from_account
            .deduct_spendable(shells)
            .map_err(|_| "Insufficient spendable balance".to_string())?;

        // Get or create receiver account
        let mut to_account = self
            .get_account(to)?
            .unwrap_or_else(|| Account::new(0, *to));

        // Credit spendable balance
        to_account.add_spendable(shells)?;

        // Save both accounts atomically (H5 fix: use WriteBatch for crash safety)
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;
        let mut batch = rocksdb::WriteBatch::default();
        let from_bytes =
            serde_json::to_vec(&from_account).map_err(|e| format!("Serialize from: {}", e))?;
        let to_bytes =
            serde_json::to_vec(&to_account).map_err(|e| format!("Serialize to: {}", e))?;
        batch.put_cf(&cf, from.0, &from_bytes);
        batch.put_cf(&cf, to.0, &to_bytes);
        self.db
            .write(batch)
            .map_err(|e| format!("Atomic transfer write failed: {}", e))?;

        // Mark both accounts dirty for incremental Merkle
        self.mark_account_dirty_with_key(from);
        self.mark_account_dirty_with_key(to);

        Ok(())
    }

    fn index_account_transactions(&self, block: &Block) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNT_TXS)
            .ok_or_else(|| "Account txs CF not found".to_string())?;

        let cf_stats = self.db.cf_handle(CF_STATS);

        for (tx_index, tx) in block.transactions.iter().enumerate() {
            let mut accounts = std::collections::HashSet::new();
            for ix in &tx.message.instructions {
                for account in &ix.accounts {
                    accounts.insert(*account);
                }
            }

            let tx_hash = tx.signature();
            let seq = tx_index as u32;

            for account in accounts {
                let mut key = Vec::with_capacity(32 + 8 + 4 + 32);
                key.extend_from_slice(&account.0);
                key.extend_from_slice(&block.header.slot.to_be_bytes());
                key.extend_from_slice(&seq.to_be_bytes());
                key.extend_from_slice(&tx_hash.0);

                self.db
                    .put_cf(&cf, &key, [])
                    .map_err(|e| format!("Failed to store account tx index: {}", e))?;

                // Increment atomic counter: "atxc:{pubkey}" += 1
                if let Some(ref cf_s) = cf_stats {
                    let mut counter_key = Vec::with_capacity(5 + 32);
                    counter_key.extend_from_slice(b"atxc:");
                    counter_key.extend_from_slice(&account.0);
                    let current = match self.db.get_cf(cf_s, &counter_key) {
                        Ok(Some(data)) if data.len() == 8 => {
                            u64::from_le_bytes(data.as_slice().try_into().unwrap())
                        }
                        _ => 0,
                    };
                    let _ = self
                        .db
                        .put_cf(cf_s, &counter_key, (current + 1).to_le_bytes());
                }
            }
        }

        Ok(())
    }

    pub fn get_account_tx_signatures(
        &self,
        pubkey: &Pubkey,
        limit: usize,
    ) -> Result<Vec<(Hash, u64)>, String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNT_TXS)
            .ok_or_else(|| "Account txs CF not found".to_string())?;

        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&pubkey.0);

        // Reverse iterate from end of prefix range — O(limit) instead of O(N)
        let mut end_key = prefix.clone();
        end_key.extend_from_slice(&[0xFF; 44]); // past any valid slot(8)+seq(4)+hash(32)

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&end_key, Direction::Reverse),
        );

        let mut items = Vec::with_capacity(limit);
        for item in iter {
            let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if !key.starts_with(&prefix) {
                break;
            }

            if key.len() < 32 + 8 + 4 + 32 {
                continue;
            }

            let slot_offset = 32;
            let seq_offset = slot_offset + 8 + 4;
            let slot_bytes: [u8; 8] = key[slot_offset..slot_offset + 8]
                .try_into()
                .map_err(|_| "Invalid slot bytes in account tx index".to_string())?;
            let slot = u64::from_be_bytes(slot_bytes);

            let mut hash_bytes = [0u8; 32];
            hash_bytes.copy_from_slice(&key[seq_offset..seq_offset + 32]);
            let hash = Hash(hash_bytes);

            items.push((hash, slot));
            if items.len() >= limit {
                break;
            }
        }

        // Already in newest-first order from reverse iteration
        Ok(items)
    }

    /// Get account transaction count via O(1) atomic counter.
    /// Falls back to prefix scan if counter not yet populated.
    pub fn count_account_txs(&self, pubkey: &Pubkey) -> Result<u64, String> {
        let cf_stats = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        // O(1) counter lookup: "atxc:{pubkey}" -> u64
        let mut counter_key = Vec::with_capacity(5 + 32);
        counter_key.extend_from_slice(b"atxc:");
        counter_key.extend_from_slice(&pubkey.0);

        match self.db.get_cf(&cf_stats, &counter_key) {
            Ok(Some(data)) if data.len() == 8 => {
                Ok(u64::from_le_bytes(data.as_slice().try_into().unwrap()))
            }
            _ => {
                // Counter not populated yet — do prefix scan and populate it
                let cf = self
                    .db
                    .cf_handle(CF_ACCOUNT_TXS)
                    .ok_or_else(|| "Account txs CF not found".to_string())?;

                let mut prefix = Vec::with_capacity(32);
                prefix.extend_from_slice(&pubkey.0);

                let mut count = 0u64;
                let iter = self.db.iterator_cf(
                    &cf,
                    rocksdb::IteratorMode::From(&prefix, Direction::Forward),
                );
                for item in iter {
                    let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
                    if !key.starts_with(&prefix) {
                        break;
                    }
                    count += 1;
                }

                // Cache the count for next time
                let _ = self.db.put_cf(&cf_stats, &counter_key, count.to_le_bytes());
                Ok(count)
            }
        }
    }

    /// Paginated account transactions using reverse iteration with cursor.
    /// Returns newest-first. Pass `before_slot` to get the next page.
    pub fn get_account_tx_signatures_paginated(
        &self,
        pubkey: &Pubkey,
        limit: usize,
        before_slot: Option<u64>,
    ) -> Result<Vec<(Hash, u64)>, String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNT_TXS)
            .ok_or_else(|| "Account txs CF not found".to_string())?;

        let prefix = pubkey.0.to_vec();

        // Build seek key: pubkey + slot (BE). Use before_slot or MAX to start from end.
        let mut seek_key = Vec::with_capacity(76);
        seek_key.extend_from_slice(&pubkey.0);
        if let Some(slot) = before_slot {
            seek_key.extend_from_slice(&slot.to_be_bytes());
        } else {
            seek_key.extend_from_slice(&u64::MAX.to_be_bytes());
        }

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&seek_key, Direction::Reverse),
        );

        let mut results = Vec::new();
        for item in iter {
            let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if !key.starts_with(&prefix) {
                break;
            }
            if key.len() < 32 + 8 + 4 + 32 {
                continue;
            }

            let slot_bytes: [u8; 8] = key[32..40]
                .try_into()
                .map_err(|_| "Invalid slot bytes".to_string())?;
            let slot = u64::from_be_bytes(slot_bytes);

            // Skip entries at or after before_slot (cursor is exclusive)
            if let Some(bs) = before_slot {
                if slot >= bs {
                    continue;
                }
            }

            let mut hash_bytes = [0u8; 32];
            hash_bytes.copy_from_slice(&key[44..76]);
            results.push((Hash(hash_bytes), slot));

            if results.len() >= limit {
                break;
            }
        }

        Ok(results) // Already newest-first from reverse iteration
    }

    /// Get recent transactions across all addresses using CF_TX_BY_SLOT reverse scan.
    /// Returns (tx_hash, slot) pairs newest-first. Pass `before_slot` for next page.
    pub fn get_recent_txs(
        &self,
        limit: usize,
        before_slot: Option<u64>,
    ) -> Result<Vec<(Hash, u64)>, String> {
        let cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "TX by slot CF not found".to_string())?;

        let seek_key = if let Some(slot) = before_slot {
            slot.to_be_bytes().to_vec()
        } else {
            u64::MAX.to_be_bytes().to_vec()
        };

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&seek_key, Direction::Reverse),
        );

        let mut results = Vec::new();
        for item in iter.flatten() {
            let (key, value) = item;
            if key.len() < 16 || value.len() != 32 {
                continue;
            }

            let slot = u64::from_be_bytes(key[0..8].try_into().unwrap());

            if let Some(bs) = before_slot {
                if slot >= bs {
                    continue;
                }
            }

            let mut hash_bytes = [0u8; 32];
            hash_bytes.copy_from_slice(&value);
            results.push((Hash(hash_bytes), slot));

            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }

    /// Get all token programs a holder has balances in (reverse index scan).
    pub fn get_holder_token_balances(
        &self,
        holder: &Pubkey,
        limit: usize,
    ) -> Result<Vec<(Pubkey, u64)>, String> {
        let cf = self
            .db
            .cf_handle(CF_HOLDER_TOKENS)
            .ok_or_else(|| "Holder tokens CF not found".to_string())?;

        let prefix = holder.0.to_vec();
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&prefix, Direction::Forward),
        );

        let mut tokens = Vec::new();
        for (key, value) in iter.flatten() {
            if !key.starts_with(&prefix) {
                break;
            }
            if key.len() == 64 && value.len() == 8 {
                let mut prog_bytes = [0u8; 32];
                prog_bytes.copy_from_slice(&key[32..64]);
                let program = Pubkey(prog_bytes);
                let balance = u64::from_le_bytes((*value).try_into().unwrap());
                tokens.push((program, balance));
                if tokens.len() >= limit {
                    break;
                }
            }
        }
        Ok(tokens)
    }
}

// NFT indexing and activity methods
impl StateStore {
    pub fn index_nft_mint(
        &self,
        collection: &Pubkey,
        token: &Pubkey,
        owner: &Pubkey,
    ) -> Result<(), String> {
        self.add_nft_owner_index(owner, token)?;
        self.add_nft_collection_index(collection, token)?;
        Ok(())
    }

    pub fn index_nft_transfer(
        &self,
        collection: &Pubkey,
        token: &Pubkey,
        from: &Pubkey,
        to: &Pubkey,
    ) -> Result<(), String> {
        self.remove_nft_owner_index(from, token)?;
        self.add_nft_owner_index(to, token)?;
        self.add_nft_collection_index(collection, token)?;
        Ok(())
    }

    pub fn get_nft_tokens_by_owner(
        &self,
        owner: &Pubkey,
        limit: usize,
    ) -> Result<Vec<Pubkey>, String> {
        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&owner.0);
        self.scan_nft_index(CF_NFT_BY_OWNER, &prefix, limit)
    }

    pub fn get_nft_tokens_by_collection(
        &self,
        collection: &Pubkey,
        limit: usize,
    ) -> Result<Vec<Pubkey>, String> {
        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&collection.0);
        self.scan_nft_index(CF_NFT_BY_COLLECTION, &prefix, limit)
    }

    pub fn record_nft_activity(
        &self,
        activity: &crate::nft::NftActivity,
        sequence: u32,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_NFT_ACTIVITY)
            .ok_or_else(|| "NFT activity CF not found".to_string())?;

        let mut key = Vec::with_capacity(32 + 8 + 4 + 32);
        key.extend_from_slice(&activity.collection.0);
        key.extend_from_slice(&activity.slot.to_be_bytes());
        key.extend_from_slice(&sequence.to_be_bytes());
        key.extend_from_slice(&activity.token.0);

        let value = crate::nft::encode_nft_activity(activity)?;
        self.db
            .put_cf(&cf, key, value)
            .map_err(|e| format!("Failed to store NFT activity: {}", e))
    }

    pub fn get_nft_activity_by_collection(
        &self,
        collection: &Pubkey,
        limit: usize,
    ) -> Result<Vec<crate::nft::NftActivity>, String> {
        let cf = self
            .db
            .cf_handle(CF_NFT_ACTIVITY)
            .ok_or_else(|| "NFT activity CF not found".to_string())?;

        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&collection.0);

        // Reverse iterate from end of prefix range — O(limit) instead of O(N)
        let mut end_key = prefix.clone();
        end_key.extend_from_slice(&[0xFF; 48]);

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&end_key, Direction::Reverse),
        );

        let mut items = Vec::with_capacity(limit);
        for item in iter {
            let (key, value) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if !key.starts_with(&prefix) {
                break;
            }

            let activity = crate::nft::decode_nft_activity(&value)?;
            items.push(activity);
            if items.len() >= limit {
                break;
            }
        }

        // Already in newest-first order from reverse iteration
        Ok(items)
    }

    fn add_nft_owner_index(&self, owner: &Pubkey, token: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_NFT_BY_OWNER)
            .ok_or_else(|| "NFT owner index CF not found".to_string())?;

        let mut key = Vec::with_capacity(64);
        key.extend_from_slice(&owner.0);
        key.extend_from_slice(&token.0);

        self.db
            .put_cf(&cf, key, [])
            .map_err(|e| format!("Failed to store NFT owner index: {}", e))
    }

    fn remove_nft_owner_index(&self, owner: &Pubkey, token: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_NFT_BY_OWNER)
            .ok_or_else(|| "NFT owner index CF not found".to_string())?;

        let mut key = Vec::with_capacity(64);
        key.extend_from_slice(&owner.0);
        key.extend_from_slice(&token.0);

        self.db
            .delete_cf(&cf, key)
            .map_err(|e| format!("Failed to delete NFT owner index: {}", e))
    }

    fn add_nft_collection_index(&self, collection: &Pubkey, token: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_NFT_BY_COLLECTION)
            .ok_or_else(|| "NFT collection index CF not found".to_string())?;

        let mut key = Vec::with_capacity(64);
        key.extend_from_slice(&collection.0);
        key.extend_from_slice(&token.0);

        self.db
            .put_cf(&cf, key, [])
            .map_err(|e| format!("Failed to store NFT collection index: {}", e))
    }

    /// Index token_id within a collection for uniqueness enforcement (T2.11).
    /// Key: "tid:" + collection(32) + token_id(8)  →  token_account(32)
    pub fn index_nft_token_id(
        &self,
        collection: &Pubkey,
        token_id: u64,
        token_account: &Pubkey,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_NFT_BY_COLLECTION)
            .ok_or_else(|| "NFT collection index CF not found".to_string())?;

        let mut key = Vec::with_capacity(44); // 4 + 32 + 8
        key.extend_from_slice(b"tid:");
        key.extend_from_slice(&collection.0);
        key.extend_from_slice(&token_id.to_le_bytes());

        self.db
            .put_cf(&cf, &key, token_account.0)
            .map_err(|e| format!("Failed to index NFT token_id: {}", e))
    }

    /// Check if a token_id is already used in a collection.
    pub fn nft_token_id_exists(&self, collection: &Pubkey, token_id: u64) -> Result<bool, String> {
        let cf = self
            .db
            .cf_handle(CF_NFT_BY_COLLECTION)
            .ok_or_else(|| "NFT collection index CF not found".to_string())?;

        let mut key = Vec::with_capacity(44);
        key.extend_from_slice(b"tid:");
        key.extend_from_slice(&collection.0);
        key.extend_from_slice(&token_id.to_le_bytes());

        match self.db.get_cf(&cf, &key) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    fn scan_nft_index(
        &self,
        cf_name: &str,
        prefix: &[u8],
        limit: usize,
    ) -> Result<Vec<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{} CF not found", cf_name))?;

        let mut results = Vec::new();
        let iter = self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::From(prefix, Direction::Forward));

        for item in iter {
            let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if !key.starts_with(prefix) {
                break;
            }

            if key.len() < prefix.len() + 32 {
                continue;
            }

            let start = prefix.len();
            let end = start + 32;
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&key[start..end]);
            results.push(Pubkey(bytes));

            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }
}

// Program indexing and call activity methods
impl StateStore {
    pub fn index_program(&self, program: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAMS)
            .ok_or_else(|| "Programs CF not found".to_string())?;

        self.db
            .put_cf(&cf, program.0, [])
            .map_err(|e| format!("Failed to store program index: {}", e))
    }

    pub fn get_programs(&self, limit: usize) -> Result<Vec<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAMS)
            .ok_or_else(|| "Programs CF not found".to_string())?;

        let mut results = Vec::new();
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);

        for item in iter {
            let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if key.len() != 32 {
                continue;
            }
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(&key);
            results.push(Pubkey(bytes));
            if results.len() >= limit {
                break;
            }
        }

        Ok(results)
    }

    pub fn get_symbol_registry(&self, symbol: &str) -> Result<Option<SymbolRegistryEntry>, String> {
        let normalized = Self::normalize_symbol(symbol)?;
        let cf = self
            .db
            .cf_handle(CF_SYMBOL_REGISTRY)
            .ok_or_else(|| "Symbol registry CF not found".to_string())?;

        match self.db.get_cf(&cf, normalized.as_bytes()) {
            Ok(Some(data)) => {
                let entry: SymbolRegistryEntry = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to decode symbol registry: {}", e))?;
                Ok(Some(entry))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    pub fn get_symbol_registry_by_program(
        &self,
        program: &Pubkey,
    ) -> Result<Option<SymbolRegistryEntry>, String> {
        // O(1) lookup via reverse index: program pubkey -> symbol
        let cf_rev = self
            .db
            .cf_handle(CF_SYMBOL_BY_PROGRAM)
            .ok_or_else(|| "Symbol-by-program CF not found".to_string())?;

        match self.db.get_cf(&cf_rev, program.0) {
            Ok(Some(symbol_bytes)) => {
                let symbol = String::from_utf8(symbol_bytes.to_vec())
                    .map_err(|e| format!("Invalid UTF-8 in symbol reverse index: {}", e))?;
                self.get_symbol_registry(&symbol)
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// List all symbol registry entries (up to limit)
    pub fn get_all_symbol_registry(
        &self,
        limit: usize,
    ) -> Result<Vec<SymbolRegistryEntry>, String> {
        let cf = self
            .db
            .cf_handle(CF_SYMBOL_REGISTRY)
            .ok_or_else(|| "Symbol registry CF not found".to_string())?;

        let mut entries = Vec::new();
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        for item in iter {
            if entries.len() >= limit {
                break;
            }
            let (_, value) = item.map_err(|e| format!("Iterator error: {}", e))?;
            let entry: SymbolRegistryEntry = serde_json::from_slice(&value)
                .map_err(|e| format!("Failed to decode symbol registry: {}", e))?;
            entries.push(entry);
        }

        Ok(entries)
    }

    pub fn register_symbol(
        &self,
        symbol: &str,
        mut entry: SymbolRegistryEntry,
    ) -> Result<(), String> {
        let normalized = Self::normalize_symbol(symbol)?;
        let cf = self
            .db
            .cf_handle(CF_SYMBOL_REGISTRY)
            .ok_or_else(|| "Symbol registry CF not found".to_string())?;

        if self
            .db
            .get_cf(&cf, normalized.as_bytes())
            .map_err(|e| format!("Database error: {}", e))?
            .is_some()
        {
            return Err(format!("Symbol already registered: {}", normalized));
        }

        entry.symbol = normalized.clone();
        let data = serde_json::to_vec(&entry)
            .map_err(|e| format!("Failed to encode symbol registry: {}", e))?;

        self.db
            .put_cf(&cf, normalized.as_bytes(), &data)
            .map_err(|e| format!("Failed to store symbol registry: {}", e))?;

        // Write reverse index: program pubkey -> symbol (O(1) program→symbol lookup)
        if let Some(cf_rev) = self.db.cf_handle(CF_SYMBOL_BY_PROGRAM) {
            self.db
                .put_cf(&cf_rev, entry.program.0, normalized.as_bytes())
                .map_err(|e| format!("Failed to store symbol reverse index: {}", e))?;
        }

        Ok(())
    }

    pub fn record_program_call(
        &self,
        activity: &crate::ProgramCallActivity,
        sequence: u32,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAM_CALLS)
            .ok_or_else(|| "Program calls CF not found".to_string())?;

        let mut key = Vec::with_capacity(32 + 8 + 4 + 32);
        key.extend_from_slice(&activity.program.0);
        key.extend_from_slice(&activity.slot.to_be_bytes());
        key.extend_from_slice(&sequence.to_be_bytes());
        key.extend_from_slice(&activity.tx_signature.0);

        let value = crate::encode_program_call_activity(activity)?;
        self.db
            .put_cf(&cf, key, value)
            .map_err(|e| format!("Failed to store program call: {}", e))?;

        // Increment atomic counter: "pcall:{pubkey}" += 1
        if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
            let mut counter_key = Vec::with_capacity(6 + 32);
            counter_key.extend_from_slice(b"pcall:");
            counter_key.extend_from_slice(&activity.program.0);
            let current = match self.db.get_cf(&cf_stats, &counter_key) {
                Ok(Some(data)) if data.len() == 8 => {
                    u64::from_le_bytes(data.as_slice().try_into().unwrap())
                }
                _ => 0,
            };
            let _ = self
                .db
                .put_cf(&cf_stats, &counter_key, (current + 1).to_le_bytes());
        }

        Ok(())
    }

    pub fn get_program_calls(
        &self,
        program: &Pubkey,
        limit: usize,
    ) -> Result<Vec<crate::ProgramCallActivity>, String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAM_CALLS)
            .ok_or_else(|| "Program calls CF not found".to_string())?;

        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&program.0);

        // Reverse iterate from end of prefix range — O(limit) instead of O(N)
        let mut end_key = prefix.clone();
        end_key.extend_from_slice(&[0xFF; 44]); // past any valid slot(8)+seq(4)+hash(32)

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&end_key, Direction::Reverse),
        );

        let mut items = Vec::with_capacity(limit);
        for item in iter {
            let (key, value) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if !key.starts_with(&prefix) {
                break;
            }

            let activity = crate::decode_program_call_activity(&value)?;
            items.push(activity);
            if items.len() >= limit {
                break;
            }
        }

        // Already in newest-first order from reverse iteration
        Ok(items)
    }

    /// Get program call count via O(1) atomic counter.
    /// Falls back to prefix scan if counter not yet populated.
    pub fn count_program_calls(&self, program: &Pubkey) -> Result<u64, String> {
        let cf_stats = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        // O(1) counter lookup: "pcall:{pubkey}" -> u64
        let mut counter_key = Vec::with_capacity(6 + 32);
        counter_key.extend_from_slice(b"pcall:");
        counter_key.extend_from_slice(&program.0);

        match self.db.get_cf(&cf_stats, &counter_key) {
            Ok(Some(data)) if data.len() == 8 => {
                Ok(u64::from_le_bytes(data.as_slice().try_into().unwrap()))
            }
            _ => {
                // Counter not populated — do prefix scan and cache
                let cf = self
                    .db
                    .cf_handle(CF_PROGRAM_CALLS)
                    .ok_or_else(|| "Program calls CF not found".to_string())?;

                let mut prefix = Vec::with_capacity(32);
                prefix.extend_from_slice(&program.0);

                let mut count = 0u64;
                let iter = self.db.iterator_cf(
                    &cf,
                    rocksdb::IteratorMode::From(&prefix, Direction::Forward),
                );
                for item in iter {
                    let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
                    if !key.starts_with(&prefix) {
                        break;
                    }
                    count += 1;
                }

                let _ = self.db.put_cf(&cf_stats, &counter_key, count.to_le_bytes());
                Ok(count)
            }
        }
    }

    pub(crate) fn normalize_symbol(raw: &str) -> Result<String, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("Symbol is required".to_string());
        }
        if !trimmed.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err("Symbol must be alphanumeric".to_string());
        }
        let normalized = trimmed.to_ascii_uppercase();
        if normalized.len() > 10 {
            return Err("Symbol must be 10 characters or less".to_string());
        }
        Ok(normalized)
    }
}

// Marketplace activity methods
impl StateStore {
    pub fn record_market_activity(
        &self,
        activity: &crate::MarketActivity,
        sequence: u32,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_MARKET_ACTIVITY)
            .ok_or_else(|| "Market activity CF not found".to_string())?;

        let zero = [0u8; 32];
        let collection_bytes = activity.collection.as_ref().map(|c| &c.0).unwrap_or(&zero);

        let mut key = Vec::with_capacity(32 + 8 + 4 + 32);
        key.extend_from_slice(collection_bytes);
        key.extend_from_slice(&activity.slot.to_be_bytes());
        key.extend_from_slice(&sequence.to_be_bytes());
        key.extend_from_slice(&activity.tx_signature.0);

        let value = crate::encode_market_activity(activity)?;
        self.db
            .put_cf(&cf, key, value)
            .map_err(|e| format!("Failed to store market activity: {}", e))
    }

    pub fn get_market_activity(
        &self,
        collection: Option<&Pubkey>,
        kind: Option<crate::MarketActivityKind>,
        limit: usize,
    ) -> Result<Vec<crate::MarketActivity>, String> {
        let cf = self
            .db
            .cf_handle(CF_MARKET_ACTIVITY)
            .ok_or_else(|| "Market activity CF not found".to_string())?;

        let mut items = Vec::with_capacity(limit);

        let iter = if let Some(collection) = collection {
            let mut prefix = Vec::with_capacity(32);
            prefix.extend_from_slice(&collection.0);
            // Reverse iterate from end of prefix range — O(limit) instead of O(N)
            let mut end_key = prefix.clone();
            end_key.extend_from_slice(&[0xFF; 48]);
            self.db.iterator_cf(
                &cf,
                rocksdb::IteratorMode::From(&end_key, Direction::Reverse),
            )
        } else {
            self.db.iterator_cf(&cf, rocksdb::IteratorMode::End)
        };

        let prefix = collection.map(|c| c.0);

        for item in iter {
            let (key, value) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if let Some(prefix_bytes) = prefix.as_ref() {
                if !key.starts_with(prefix_bytes) {
                    break;
                }
            }

            let activity = crate::decode_market_activity(&value)?;
            if let Some(filter_kind) = kind.as_ref() {
                if &activity.kind != filter_kind {
                    continue;
                }
            }

            items.push(activity);
            if items.len() >= limit {
                break;
            }
        }

        // Already in newest-first order from reverse iteration
        Ok(items)
    }

    // ─── Atomic Batch API (T1.4 / T3.1) ─────────────────────────────

    /// Begin an atomic write batch. All mutations go into the batch's in-memory
    /// `WriteBatch` and account overlay. Nothing touches disk until `commit_batch()`.
    pub fn begin_batch(&self) -> StateBatch {
        StateBatch {
            batch: WriteBatch::default(),
            account_overlay: std::collections::HashMap::new(),
            stake_pool_overlay: None,
            reefstake_pool_overlay: None,
            new_accounts: 0,
            active_account_delta: 0,
            burned_delta: 0,
            nft_token_id_overlay: std::collections::HashSet::new(),
            event_seq: 0,
            db: Arc::clone(&self.db),
        }
    }

    /// Commit a batch atomically. All puts in the `WriteBatch` are flushed to
    /// RocksDB in a single atomic write. Metric deltas are applied after the
    /// write succeeds.
    pub fn commit_batch(&self, batch: StateBatch) -> Result<(), String> {
        // Collect dirty pubkeys BEFORE consuming the batch
        let dirty_pubkeys: Vec<Pubkey> = batch.account_overlay.keys().cloned().collect();

        // If burns accumulated, fold them into the WriteBatch so they
        // commit atomically with the rest of the transaction state.
        let mut wb = batch.batch;
        if batch.burned_delta > 0 {
            if let Some(cf) = self.db.cf_handle(CF_STATS) {
                let current = self.get_total_burned().unwrap_or(0);
                let new_total = current.saturating_add(batch.burned_delta);
                wb.put_cf(&cf, b"total_burned", new_total.to_le_bytes());
            }
        }

        // Atomic write — either all succeed or none.
        self.db
            .write(wb)
            .map_err(|e| format!("Atomic batch commit failed: {}", e))?;

        // Apply metric deltas (these are in-memory counters; safe post-commit)
        if batch.new_accounts != 0 {
            for _ in 0..batch.new_accounts {
                self.metrics.increment_accounts();
            }
        }
        if batch.active_account_delta > 0 {
            for _ in 0..batch.active_account_delta {
                self.metrics.increment_active_accounts();
            }
        } else if batch.active_account_delta < 0 {
            for _ in 0..(-batch.active_account_delta) {
                self.metrics.decrement_active_accounts();
            }
        }
        // Persist metrics to disk (single write of all counters)
        self.metrics.save(&self.db)?;

        // Mark each modified account dirty for incremental Merkle recomputation
        for pubkey in &dirty_pubkeys {
            self.mark_account_dirty_with_key(pubkey);
        }

        Ok(())
    }
}

// ─── StateBatch Methods ──────────────────────────────────────────────

impl StateBatch {
    /// Accumulate burned amount in this batch (committed atomically on commit_batch)
    pub fn add_burned(&mut self, amount: u64) {
        self.burned_delta = self.burned_delta.saturating_add(amount);
    }

    /// H3 fix: Apply deferred EVM state changes atomically through this WriteBatch.
    /// All EVM account, storage, and native balance writes go through the batch,
    /// guaranteeing atomicity with tx/receipt/fee writes.
    pub fn apply_evm_changes(
        &mut self,
        changes: &[crate::evm::EvmStateChange],
    ) -> Result<(), String> {
        use rocksdb::Direction;

        // Collect native balance updates to apply after EVM writes
        // (avoids borrow conflict between cf_handle refs and put_account)
        let mut native_updates: Vec<(Pubkey, u64)> = Vec::new();

        // Phase 1: EVM account + storage writes (immutable borrows only)
        {
            let cf_accounts = self
                .db
                .cf_handle(CF_EVM_ACCOUNTS)
                .ok_or_else(|| "EVM Accounts CF not found".to_string())?;
            let cf_storage = self
                .db
                .cf_handle(CF_EVM_STORAGE)
                .ok_or_else(|| "EVM Storage CF not found".to_string())?;

            for change in changes {
                if let Some(ref account) = change.account {
                    let data = bincode::serialize(account)
                        .map_err(|e| format!("Failed to serialize EVM account: {}", e))?;
                    self.batch.put_cf(&cf_accounts, change.evm_address, &data);
                } else {
                    // Clear EVM account (self-destruct)
                    self.batch.delete_cf(&cf_accounts, change.evm_address);

                    // Clear all on-disk storage slots for this address
                    let prefix = &change.evm_address[..];
                    let iter = self.db.iterator_cf(
                        &cf_storage,
                        rocksdb::IteratorMode::From(prefix, Direction::Forward),
                    );
                    for item in iter.flatten() {
                        let (key, _) = item;
                        if !key.starts_with(prefix) {
                            break;
                        }
                        self.batch.delete_cf(&cf_storage, &key);
                    }
                }

                // Apply storage changes
                for (slot, value) in &change.storage_changes {
                    let mut key = Vec::with_capacity(52);
                    key.extend_from_slice(&change.evm_address);
                    key.extend_from_slice(slot);

                    if let Some(val) = value {
                        self.batch
                            .put_cf(&cf_storage, &key, val.to_be_bytes::<32>());
                    } else {
                        self.batch.delete_cf(&cf_storage, &key);
                    }
                }

                // Collect native balance updates for phase 2
                if let Some((pubkey, shells)) = change.native_balance_update {
                    native_updates.push((pubkey, shells));
                }
            }
        }

        // Phase 2: Native account balance syncs (requires mutable self)
        for (pubkey, shells) in native_updates {
            let mut account = self
                .get_account(&pubkey)?
                .unwrap_or_else(|| Account::new(0, pubkey));
            account.spendable = shells;
            account.shells = account
                .spendable
                .saturating_add(account.staked)
                .saturating_add(account.locked);
            self.put_account(&pubkey, &account)?;
        }

        Ok(())
    }

    /// Get an account — checks the in-memory overlay first, then falls through
    /// to on-disk state.
    pub fn get_account(&self, pubkey: &Pubkey) -> Result<Option<Account>, String> {
        // Check overlay first
        if let Some(account) = self.account_overlay.get(pubkey) {
            return Ok(Some(account.clone()));
        }
        // Fall through to disk
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;
        match self.db.get_cf(&cf, pubkey.0) {
            Ok(Some(data)) => {
                let mut account: Account = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to deserialize account: {}", e))?;
                account.fixup_legacy(); // M11 fix
                Ok(Some(account))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Put an account into the batch (not written to disk until commit).
    pub fn put_account(&mut self, pubkey: &Pubkey, account: &Account) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        // Check if this is a new account
        let old_balance = if let Some(existing) = self.account_overlay.get(pubkey) {
            Some(existing.shells)
        } else {
            // Check disk
            match self.db.get_cf(&cf, pubkey.0) {
                Ok(Some(data)) => serde_json::from_slice::<Account>(&data)
                    .ok()
                    .map(|a| a.shells),
                _ => None,
            }
        };

        let is_new = old_balance.is_none();
        let old_bal = old_balance.unwrap_or(0);
        let new_bal = account.shells;

        // Track metric deltas
        if is_new {
            self.new_accounts += 1;
        }
        if old_bal == 0 && new_bal > 0 {
            self.active_account_delta += 1;
        } else if old_bal > 0 && new_bal == 0 {
            self.active_account_delta -= 1;
        }

        let value = serde_json::to_vec(account)
            .map_err(|e| format!("Failed to serialize account: {}", e))?;

        self.batch.put_cf(&cf, pubkey.0, &value);
        self.account_overlay.insert(*pubkey, account.clone());
        Ok(())
    }

    /// Transfer shells between accounts within the batch.
    pub fn transfer(&mut self, from: &Pubkey, to: &Pubkey, shells: u64) -> Result<(), String> {
        if from == to {
            return Ok(());
        }

        let mut from_account = self
            .get_account(from)?
            .ok_or_else(|| "Sender account not found".to_string())?;
        from_account
            .deduct_spendable(shells)
            .map_err(|_| "Insufficient spendable balance".to_string())?;

        let mut to_account = self
            .get_account(to)?
            .unwrap_or_else(|| Account::new(0, *to));
        to_account.add_spendable(shells)?;

        self.put_account(from, &from_account)?;
        self.put_account(to, &to_account)?;
        Ok(())
    }

    /// Put a transaction into the batch.
    pub fn put_transaction(&mut self, tx: &Transaction) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;
        let sig = tx.signature();
        let value = serde_json::to_vec(tx)
            .map_err(|e| format!("Failed to serialize transaction: {}", e))?;
        self.batch.put_cf(&cf, sig.0, &value);
        Ok(())
    }

    /// Put stake pool into the batch.
    pub fn put_stake_pool(&mut self, pool: &crate::consensus::StakePool) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STAKE_POOL)
            .ok_or_else(|| "Stake pool CF not found".to_string())?;
        let data = bincode::serialize(pool)
            .map_err(|e| format!("Failed to serialize stake pool: {}", e))?;
        self.batch.put_cf(&cf, b"pool", &data);
        self.stake_pool_overlay = Some(pool.clone());
        Ok(())
    }

    /// Get stake pool — checks overlay first, then falls through to disk.
    pub fn get_stake_pool(&self) -> Result<crate::consensus::StakePool, String> {
        if let Some(pool) = &self.stake_pool_overlay {
            return Ok(pool.clone());
        }
        // Fall through to disk
        let cf = self
            .db
            .cf_handle(CF_STAKE_POOL)
            .ok_or_else(|| "Stake pool CF not found".to_string())?;
        match self.db.get_cf(&cf, b"pool") {
            Ok(Some(data)) => bincode::deserialize(&data)
                .map_err(|e| format!("Failed to deserialize stake pool: {}", e)),
            Ok(None) => Ok(crate::consensus::StakePool::new()),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Put ReefStake pool into the batch.
    pub fn put_reefstake_pool(&mut self, pool: &ReefStakePool) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_REEFSTAKE)
            .ok_or_else(|| "ReefStake CF not found".to_string())?;
        let data = serde_json::to_vec(pool)
            .map_err(|e| format!("Failed to serialize ReefStake pool: {}", e))?;
        self.batch.put_cf(&cf, b"pool", &data);
        self.reefstake_pool_overlay = Some(pool.clone());
        Ok(())
    }

    /// Get ReefStake pool — checks overlay first, then falls through to disk.
    pub fn get_reefstake_pool(&self) -> Result<ReefStakePool, String> {
        if let Some(pool) = &self.reefstake_pool_overlay {
            return Ok(pool.clone());
        }
        let cf = self
            .db
            .cf_handle(CF_REEFSTAKE)
            .ok_or_else(|| "ReefStake CF not found".to_string())?;
        match self.db.get_cf(&cf, b"pool") {
            Ok(Some(data)) => serde_json::from_slice(&data)
                .map_err(|e| format!("Failed to deserialize ReefStake pool: {}", e)),
            Ok(None) => Ok(ReefStakePool::new()),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Register EVM address mapping in the batch.
    pub fn register_evm_address(
        &mut self,
        evm_address: &[u8; 20],
        native: &Pubkey,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_MAP)
            .ok_or_else(|| "EVM map CF not found".to_string())?;
        self.batch.put_cf(&cf, evm_address, native.0);
        // Reverse mapping
        let mut reverse_key = Vec::with_capacity(52);
        reverse_key.extend_from_slice(b"reverse:");
        reverse_key.extend_from_slice(&native.0);
        self.batch.put_cf(&cf, &reverse_key, evm_address);
        Ok(())
    }

    /// Index NFT mint in the batch.
    pub fn index_nft_mint(
        &mut self,
        collection: &Pubkey,
        token: &Pubkey,
        owner: &Pubkey,
    ) -> Result<(), String> {
        // Owner index
        let cf_owner = self
            .db
            .cf_handle(CF_NFT_BY_OWNER)
            .ok_or_else(|| "NFT owner index CF not found".to_string())?;
        let mut key = Vec::with_capacity(64);
        key.extend_from_slice(&owner.0);
        key.extend_from_slice(&token.0);
        self.batch.put_cf(&cf_owner, &key, []);

        // Collection index
        let cf_coll = self
            .db
            .cf_handle(CF_NFT_BY_COLLECTION)
            .ok_or_else(|| "NFT collection index CF not found".to_string())?;
        let mut ckey = Vec::with_capacity(64);
        ckey.extend_from_slice(&collection.0);
        ckey.extend_from_slice(&token.0);
        self.batch.put_cf(&cf_coll, &ckey, []);

        Ok(())
    }

    /// M6 fix: index NFT token_id within the batch for atomicity
    pub fn index_nft_token_id(
        &mut self,
        collection: &Pubkey,
        token_id: u64,
        token_account: &Pubkey,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_NFT_BY_COLLECTION)
            .ok_or_else(|| "NFT collection index CF not found".to_string())?;

        let mut key = Vec::with_capacity(44); // 4 + 32 + 8
        key.extend_from_slice(b"tid:");
        key.extend_from_slice(&collection.0);
        key.extend_from_slice(&token_id.to_le_bytes());

        self.batch.put_cf(&cf, &key, token_account.0);
        // AUDIT-FIX 1.15: Track in overlay for batch-aware uniqueness checks
        self.nft_token_id_overlay.insert(key);
        Ok(())
    }

    /// AUDIT-FIX 1.15: Check if a token_id exists in the batch overlay OR committed state
    pub fn nft_token_id_exists(
        &self,
        collection: &Pubkey,
        token_id: u64,
    ) -> Result<bool, String> {
        let mut key = Vec::with_capacity(44);
        key.extend_from_slice(b"tid:");
        key.extend_from_slice(&collection.0);
        key.extend_from_slice(&token_id.to_le_bytes());

        // Check batch overlay first
        if self.nft_token_id_overlay.contains(&key) {
            return Ok(true);
        }

        // Fall through to committed state
        let cf = self
            .db
            .cf_handle(CF_NFT_BY_COLLECTION)
            .ok_or_else(|| "NFT collection index CF not found".to_string())?;
        match self.db.get_cf(&cf, &key) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Index NFT transfer in the batch.
    pub fn index_nft_transfer(
        &mut self,
        collection: &Pubkey,
        token: &Pubkey,
        from: &Pubkey,
        to: &Pubkey,
    ) -> Result<(), String> {
        // Remove old owner index
        let cf_owner = self
            .db
            .cf_handle(CF_NFT_BY_OWNER)
            .ok_or_else(|| "NFT owner index CF not found".to_string())?;
        let mut old_key = Vec::with_capacity(64);
        old_key.extend_from_slice(&from.0);
        old_key.extend_from_slice(&token.0);
        self.batch.delete_cf(&cf_owner, &old_key);

        // Add new owner index
        let mut new_key = Vec::with_capacity(64);
        new_key.extend_from_slice(&to.0);
        new_key.extend_from_slice(&token.0);
        self.batch.put_cf(&cf_owner, &new_key, []);

        // Update collection index
        let cf_coll = self
            .db
            .cf_handle(CF_NFT_BY_COLLECTION)
            .ok_or_else(|| "NFT collection index CF not found".to_string())?;
        let mut ckey = Vec::with_capacity(64);
        ckey.extend_from_slice(&collection.0);
        ckey.extend_from_slice(&token.0);
        self.batch.put_cf(&cf_coll, &ckey, []);

        Ok(())
    }

    /// Put contract event in the batch.
    pub fn put_contract_event(
        &mut self,
        program: &Pubkey,
        event: &ContractEvent,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVENTS)
            .ok_or_else(|| "Events CF not found".to_string())?;
        // T2.13 fix: Key now includes an auto-incrementing sequence number
        // so that multiple events with the same name in the same slot are
        // stored separately instead of overwriting each other.
        let seq = self.event_seq;
        self.event_seq += 1;
        let mut key = Vec::with_capacity(56); // 32 + 8 + 8 + 8
        key.extend_from_slice(&program.0);
        key.extend_from_slice(&event.slot.to_be_bytes());
        let name_hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            event.name.hash(&mut h);
            h.finish()
        };
        key.extend_from_slice(&name_hash.to_be_bytes());
        key.extend_from_slice(&seq.to_be_bytes());
        let value =
            serde_json::to_vec(event).map_err(|e| format!("Failed to serialize event: {}", e))?;
        self.batch.put_cf(&cf, &key, &value);

        // Write slot secondary index: slot(8,BE) + program(32) + seq(8,BE) -> event_key
        if let Some(cf_slot) = self.db.cf_handle(CF_EVENTS_BY_SLOT) {
            let mut slot_key = Vec::with_capacity(8 + 32 + 8);
            slot_key.extend_from_slice(&event.slot.to_be_bytes());
            slot_key.extend_from_slice(&program.0);
            slot_key.extend_from_slice(&seq.to_be_bytes());
            self.batch.put_cf(&cf_slot, &slot_key, &key);
        }

        Ok(())
    }

    /// Put EVM transaction record in the batch.
    pub fn put_evm_tx(&mut self, record: &crate::evm::EvmTxRecord) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_TXS)
            .ok_or_else(|| "EVM txs CF not found".to_string())?;
        let key = record.evm_hash.as_slice();
        // Must use bincode to match StateStore::get_evm_tx reader
        let value =
            bincode::serialize(record).map_err(|e| format!("Failed to serialize EVM tx: {}", e))?;
        self.batch.put_cf(&cf, key, &value);
        Ok(())
    }

    /// Put EVM receipt in the batch.
    pub fn put_evm_receipt(&mut self, receipt: &crate::evm::EvmReceipt) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_RECEIPTS)
            .ok_or_else(|| "EVM receipts CF not found".to_string())?;
        let key = receipt.evm_hash.as_slice();
        // Must use bincode to match StateStore::get_evm_receipt reader
        let value = bincode::serialize(receipt)
            .map_err(|e| format!("Failed to serialize EVM receipt: {}", e))?;
        self.batch.put_cf(&cf, key, &value);
        Ok(())
    }

    /// Index a program in the batch.
    pub fn index_program(&mut self, program: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAMS)
            .ok_or_else(|| "Programs CF not found".to_string())?;
        self.batch.put_cf(&cf, program.0, []);
        Ok(())
    }

    /// Register a symbol in the batch.
    pub fn register_symbol(
        &mut self,
        symbol: &str,
        entry: &crate::state::SymbolRegistryEntry,
    ) -> Result<(), String> {
        // M2 fix: apply same validation as non-batch path
        let normalized = StateStore::normalize_symbol(symbol)?;
        let cf = self
            .db
            .cf_handle(CF_SYMBOL_REGISTRY)
            .ok_or_else(|| "Symbol registry CF not found".to_string())?;
        // Check if already registered (disk only - batch writes aren't visible in reads for this CF)
        if self
            .db
            .get_cf(&cf, normalized.as_bytes())
            .map_err(|e| format!("Database error: {}", e))?
            .is_some()
        {
            return Err(format!("Symbol already registered: {}", normalized));
        }
        let mut entry_copy = entry.clone();
        entry_copy.symbol = normalized.clone();
        let data = serde_json::to_vec(&entry_copy)
            .map_err(|e| format!("Failed to encode symbol registry: {}", e))?;
        self.batch.put_cf(&cf, normalized.as_bytes(), &data);

        // Write reverse index: program pubkey -> symbol (O(1) program→symbol lookup)
        if let Some(cf_rev) = self.db.cf_handle(CF_SYMBOL_BY_PROGRAM) {
            self.batch
                .put_cf(&cf_rev, entry.program.0, normalized.as_bytes());
        }

        Ok(())
    }

    /// Read-only: get last slot (falls through to disk since batches don't modify this).
    pub fn get_last_slot(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        match self.db.get_cf(&cf, b"last_slot") {
            Ok(Some(data)) if data.len() == 8 => {
                Ok(u64::from_le_bytes(data.as_slice().try_into().unwrap()))
            }
            Ok(_) => Ok(0),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }
}

// Validator management methods
impl StateStore {
    /// Store validator info
    pub fn put_validator(&self, info: &crate::consensus::ValidatorInfo) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_VALIDATORS)
            .ok_or_else(|| "Validators CF not found".to_string())?;

        let key = info.pubkey.0;
        let value = serde_json::to_vec(info)
            .map_err(|e| format!("Failed to serialize validator: {}", e))?;

        self.db
            .put_cf(&cf, key, value)
            .map_err(|e| format!("Failed to store validator: {}", e))
    }

    /// Delete validator from state
    pub fn delete_validator(&self, pubkey: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_VALIDATORS)
            .ok_or_else(|| "Validators CF not found".to_string())?;

        self.db
            .delete_cf(&cf, pubkey.0)
            .map_err(|e| format!("Failed to delete validator: {}", e))
    }

    /// Get validator info
    pub fn get_validator(
        &self,
        pubkey: &Pubkey,
    ) -> Result<Option<crate::consensus::ValidatorInfo>, String> {
        let cf = self
            .db
            .cf_handle(CF_VALIDATORS)
            .ok_or_else(|| "Validators CF not found".to_string())?;

        match self
            .db
            .get_cf(&cf, pubkey.0)
            .map_err(|e| format!("Failed to get validator: {}", e))?
        {
            Some(bytes) => {
                let info = serde_json::from_slice(&bytes)
                    .map_err(|e| format!("Failed to deserialize validator: {}", e))?;
                Ok(Some(info))
            }
            None => Ok(None),
        }
    }

    /// Get all validators
    pub fn get_all_validators(&self) -> Result<Vec<crate::consensus::ValidatorInfo>, String> {
        let cf = self
            .db
            .cf_handle(CF_VALIDATORS)
            .ok_or_else(|| "Validators CF not found".to_string())?;

        let mut validators = Vec::new();
        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);

        for item in iter {
            let (_key, value) = item.map_err(|e| format!("Iterator error: {}", e))?;
            let info: crate::consensus::ValidatorInfo = serde_json::from_slice(&value)
                .map_err(|e| format!("Failed to deserialize validator: {}", e))?;
            validators.push(info);
        }

        Ok(validators)
    }

    /// Load validator set from state
    pub fn load_validator_set(&self) -> Result<crate::consensus::ValidatorSet, String> {
        let mut set = crate::consensus::ValidatorSet::new();
        let validators = self.get_all_validators()?;

        for validator in validators {
            set.add_validator(validator);
        }

        Ok(set)
    }

    /// Save entire validator set to state (replaces all existing entries)
    pub fn save_validator_set(&self, set: &crate::consensus::ValidatorSet) -> Result<(), String> {
        // Clear stale validators first to prevent ghost entries from old keypairs
        self.clear_all_validators()?;
        for validator in set.validators() {
            self.put_validator(validator)?;
        }
        Ok(())
    }

    /// Remove ALL validators from the CF (used before full re-save)
    pub fn clear_all_validators(&self) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_VALIDATORS)
            .ok_or_else(|| "Validators CF not found".to_string())?;

        let keys: Vec<Box<[u8]>> = self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::Start)
            .filter_map(|item| item.ok().map(|(k, _)| k))
            .collect();

        for key in keys {
            self.db
                .delete_cf(&cf, &key)
                .map_err(|e| format!("Failed to delete validator: {}", e))?;
        }
        Ok(())
    }

    /// Load stake pool from state (or initialize empty)
    pub fn get_stake_pool(&self) -> Result<crate::consensus::StakePool, String> {
        let cf = self
            .db
            .cf_handle(CF_STAKE_POOL)
            .ok_or_else(|| "Stake pool CF not found".to_string())?;

        match self.db.get_cf(&cf, b"pool") {
            Ok(Some(data)) => bincode::deserialize(&data)
                .map_err(|e| format!("Failed to deserialize stake pool: {}", e)),
            Ok(None) => Ok(crate::consensus::StakePool::new()),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Store stake pool
    pub fn put_stake_pool(&self, pool: &crate::consensus::StakePool) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STAKE_POOL)
            .ok_or_else(|| "Stake pool CF not found".to_string())?;

        let data = bincode::serialize(pool)
            .map_err(|e| format!("Failed to serialize stake pool: {}", e))?;

        self.db
            .put_cf(&cf, b"pool", data)
            .map_err(|e| format!("Failed to store stake pool: {}", e))
    }

    /// Get total shells burned (fee burn)
    pub fn get_total_burned(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        match self.db.get_cf(&cf, b"total_burned") {
            Ok(Some(data)) => {
                let bytes: [u8; 8] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid burned data".to_string())?;
                Ok(u64::from_le_bytes(bytes))
            }
            Ok(None) => Ok(0),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Add to total burned amount (atomic via RocksDB merge-style read-modify-write)
    pub fn add_burned(&self, amount: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        // Use a WriteBatch to ensure read+write is at least crash-safe
        let current = self.get_total_burned()?;
        let new_total = current.saturating_add(amount);

        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(&cf, b"total_burned", new_total.to_le_bytes());
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to store burned amount: {}", e))
    }

    /// Store treasury public key
    pub fn set_treasury_pubkey(&self, pubkey: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        self.db
            .put_cf(&cf, b"treasury_pubkey", pubkey.0)
            .map_err(|e| format!("Failed to store treasury pubkey: {}", e))
    }

    /// Store genesis public key
    pub fn set_genesis_pubkey(&self, pubkey: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        self.db
            .put_cf(&cf, b"genesis_pubkey", pubkey.0)
            .map_err(|e| format!("Failed to store genesis pubkey: {}", e))
    }

    /// Store all genesis distribution accounts (role → pubkey mapping)
    /// Serialized as JSON array: [{"role":"...","pubkey":"...","amount_molt":N,"percentage":N}]
    pub fn set_genesis_accounts(
        &self,
        accounts: &[(String, Pubkey, u64, u8)],
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let entries: Vec<serde_json::Value> = accounts
            .iter()
            .map(|(role, pubkey, amount_molt, percentage)| {
                serde_json::json!({
                    "role": role,
                    "pubkey": pubkey.to_base58(),
                    "amount_molt": amount_molt,
                    "percentage": percentage,
                })
            })
            .collect();

        let json = serde_json::to_vec(&entries)
            .map_err(|e| format!("Failed to serialize genesis accounts: {}", e))?;

        self.db
            .put_cf(&cf, b"genesis_accounts", json)
            .map_err(|e| format!("Failed to store genesis accounts: {}", e))
    }

    /// Load all genesis distribution accounts
    pub fn get_genesis_accounts(&self) -> Result<Vec<(String, Pubkey, u64, u8)>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        match self.db.get_cf(&cf, b"genesis_accounts") {
            Ok(Some(data)) => {
                let entries: Vec<serde_json::Value> = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to deserialize genesis accounts: {}", e))?;
                let mut result = Vec::new();
                for entry in entries {
                    let role = entry["role"].as_str().unwrap_or("").to_string();
                    let pubkey_str = entry["pubkey"].as_str().unwrap_or("");
                    let pubkey = Pubkey::from_base58(pubkey_str)
                        .map_err(|e| format!("Invalid pubkey '{}': {}", pubkey_str, e))?;
                    let amount_molt = entry["amount_molt"].as_u64().unwrap_or(0);
                    let percentage = entry["percentage"].as_u64().unwrap_or(0) as u8;
                    result.push((role, pubkey, amount_molt, percentage));
                }
                Ok(result)
            }
            Ok(None) => Ok(Vec::new()),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Store rent parameters
    pub fn set_rent_params(
        &self,
        rate_shells_per_kb_month: u64,
        free_kb: u64,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        self.db
            .put_cf(
                &cf,
                b"rent_rate_shells_per_kb_month",
                rate_shells_per_kb_month.to_le_bytes(),
            )
            .map_err(|e| format!("Failed to store rent rate: {}", e))?;
        self.db
            .put_cf(&cf, b"rent_free_kb", free_kb.to_le_bytes())
            .map_err(|e| format!("Failed to store rent free tier: {}", e))?;

        Ok(())
    }

    /// Load rent parameters (defaults if missing)
    pub fn get_rent_params(&self) -> Result<(u64, u64), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let rate = match self.db.get_cf(&cf, b"rent_rate_shells_per_kb_month") {
            Ok(Some(data)) => {
                let bytes: [u8; 8] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid rent rate data".to_string())?;
                u64::from_le_bytes(bytes)
            }
            Ok(None) => 1_000,
            Err(e) => return Err(format!("Database error: {}", e)),
        };

        let free_kb = match self.db.get_cf(&cf, b"rent_free_kb") {
            Ok(Some(data)) => {
                let bytes: [u8; 8] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid rent free tier data".to_string())?;
                u64::from_le_bytes(bytes)
            }
            Ok(None) => 1,
            Err(e) => return Err(format!("Database error: {}", e)),
        };

        Ok((rate, free_kb))
    }

    /// Store fee configuration
    pub fn set_fee_config(
        &self,
        base_fee: u64,
        contract_deploy_fee: u64,
        contract_upgrade_fee: u64,
        nft_mint_fee: u64,
        nft_collection_fee: u64,
    ) -> Result<(), String> {
        let config = crate::FeeConfig {
            base_fee,
            contract_deploy_fee,
            contract_upgrade_fee,
            nft_mint_fee,
            nft_collection_fee,
            ..crate::FeeConfig::default_from_constants()
        };
        self.set_fee_config_full(&config)
    }

    /// Store complete fee configuration including distribution percentages
    pub fn set_fee_config_full(&self, config: &crate::FeeConfig) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        self.db
            .put_cf(&cf, b"fee_base_shells", config.base_fee.to_le_bytes())
            .map_err(|e| format!("Failed to store base fee: {}", e))?;
        self.db
            .put_cf(
                &cf,
                b"fee_contract_deploy_shells",
                config.contract_deploy_fee.to_le_bytes(),
            )
            .map_err(|e| format!("Failed to store deploy fee: {}", e))?;
        self.db
            .put_cf(
                &cf,
                b"fee_contract_upgrade_shells",
                config.contract_upgrade_fee.to_le_bytes(),
            )
            .map_err(|e| format!("Failed to store upgrade fee: {}", e))?;
        self.db
            .put_cf(
                &cf,
                b"fee_nft_mint_shells",
                config.nft_mint_fee.to_le_bytes(),
            )
            .map_err(|e| format!("Failed to store NFT mint fee: {}", e))?;
        self.db
            .put_cf(
                &cf,
                b"fee_nft_collection_shells",
                config.nft_collection_fee.to_le_bytes(),
            )
            .map_err(|e| format!("Failed to store NFT collection fee: {}", e))?;
        self.db
            .put_cf(
                &cf,
                b"fee_burn_percent",
                config.fee_burn_percent.to_le_bytes(),
            )
            .map_err(|e| format!("Failed to store burn percent: {}", e))?;
        self.db
            .put_cf(
                &cf,
                b"fee_producer_percent",
                config.fee_producer_percent.to_le_bytes(),
            )
            .map_err(|e| format!("Failed to store producer percent: {}", e))?;
        self.db
            .put_cf(
                &cf,
                b"fee_voters_percent",
                config.fee_voters_percent.to_le_bytes(),
            )
            .map_err(|e| format!("Failed to store voters percent: {}", e))?;
        self.db
            .put_cf(
                &cf,
                b"fee_treasury_percent",
                config.fee_treasury_percent.to_le_bytes(),
            )
            .map_err(|e| format!("Failed to store treasury percent: {}", e))?;

        Ok(())
    }

    /// Load fee configuration (defaults if missing)
    pub fn get_fee_config(&self) -> Result<crate::FeeConfig, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let read_u64 = |key: &[u8]| -> Result<Option<u64>, String> {
            match self.db.get_cf(&cf, key) {
                Ok(Some(data)) => {
                    let bytes: [u8; 8] = data
                        .as_slice()
                        .try_into()
                        .map_err(|_| "Invalid fee config data".to_string())?;
                    Ok(Some(u64::from_le_bytes(bytes)))
                }
                Ok(None) => Ok(None),
                Err(e) => Err(format!("Database error: {}", e)),
            }
        };

        let defaults = crate::FeeConfig::default_from_constants();

        Ok(crate::FeeConfig {
            base_fee: read_u64(b"fee_base_shells")?.unwrap_or(defaults.base_fee),
            contract_deploy_fee: read_u64(b"fee_contract_deploy_shells")?
                .unwrap_or(defaults.contract_deploy_fee),
            contract_upgrade_fee: read_u64(b"fee_contract_upgrade_shells")?
                .unwrap_or(defaults.contract_upgrade_fee),
            nft_mint_fee: read_u64(b"fee_nft_mint_shells")?.unwrap_or(defaults.nft_mint_fee),
            nft_collection_fee: read_u64(b"fee_nft_collection_shells")?
                .unwrap_or(defaults.nft_collection_fee),
            fee_burn_percent: read_u64(b"fee_burn_percent")?.unwrap_or(defaults.fee_burn_percent),
            fee_producer_percent: read_u64(b"fee_producer_percent")?
                .unwrap_or(defaults.fee_producer_percent),
            fee_voters_percent: read_u64(b"fee_voters_percent")?
                .unwrap_or(defaults.fee_voters_percent),
            fee_treasury_percent: read_u64(b"fee_treasury_percent")?
                .unwrap_or(defaults.fee_treasury_percent),
        })
    }

    /// Load treasury public key
    pub fn get_treasury_pubkey(&self) -> Result<Option<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        match self.db.get_cf(&cf, b"treasury_pubkey") {
            Ok(Some(data)) => {
                if data.len() != 32 {
                    return Err("Invalid treasury pubkey length".to_string());
                }
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data);
                Ok(Some(Pubkey(bytes)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Load genesis public key
    pub fn get_genesis_pubkey(&self) -> Result<Option<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        match self.db.get_cf(&cf, b"genesis_pubkey") {
            Ok(Some(data)) => {
                if data.len() != 32 {
                    return Err("Invalid genesis pubkey length".to_string());
                }
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data);
                Ok(Some(Pubkey(bytes)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Check if fee distribution already applied for a slot
    pub fn get_fee_distribution_hash(&self, slot: u64) -> Result<Option<Hash>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("fee_dist:{}", slot);
        match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) => {
                if data.len() != 32 {
                    return Err("Invalid fee distribution hash length".to_string());
                }
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data);
                Ok(Some(Hash(bytes)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Mark fee distribution applied for a slot
    pub fn set_fee_distribution_hash(&self, slot: u64, hash: &Hash) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("fee_dist:{}", slot);
        self.db
            .put_cf(&cf, key.as_bytes(), hash.0)
            .map_err(|e| format!("Failed to store fee distribution hash: {}", e))
    }

    /// Check if reward distribution already applied for a slot
    pub fn get_reward_distribution_hash(&self, slot: u64) -> Result<Option<Hash>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("reward_dist:{}", slot);
        match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) => {
                if data.len() != 32 {
                    return Err("Invalid reward distribution hash length".to_string());
                }
                let mut bytes = [0u8; 32];
                bytes.copy_from_slice(&data);
                Ok(Some(Hash(bytes)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Mark reward distribution applied for a slot
    pub fn set_reward_distribution_hash(&self, slot: u64, hash: &Hash) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("reward_dist:{}", slot);
        self.db
            .put_cf(&cf, key.as_bytes(), hash.0)
            .map_err(|e| format!("Failed to store reward distribution hash: {}", e))
    }

    /// Clear reward distribution record for a slot (used by fork choice).
    pub fn clear_reward_distribution_hash(&self, slot: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("reward_dist:{}", slot);
        self.db
            .delete_cf(&cf, key.as_bytes())
            .map_err(|e| format!("Failed to clear reward distribution hash: {}", e))
    }

    /// Clear fee distribution record for a slot (used by fork choice).
    pub fn clear_fee_distribution_hash(&self, slot: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("fee_dist:{}", slot);
        self.db
            .delete_cf(&cf, key.as_bytes())
            .map_err(|e| format!("Failed to clear fee distribution hash: {}", e))
    }

    // ─── Stats Pruning (Bounded Retention) ──────────────────────────────────

    /// Prune per-slot stats keys older than `retain_slots` behind `current_slot`.
    /// Removes: fee_dist:*, reward_dist:*, esq:*, tsq:*, txs:* entries for old slots.
    /// Call periodically (e.g., every 1000 slots) to bound CF_STATS growth.
    /// At 1M blocks with 10K retention, this prevents ~990K stale sequence keys.
    pub fn prune_slot_stats(&self, current_slot: u64, retain_slots: u64) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        if current_slot <= retain_slots {
            return Ok(0);
        }
        let cutoff = current_slot - retain_slots;
        let mut batch = WriteBatch::default();
        let mut deleted = 0u64;

        // 1. Prune fee_dist:{slot} (text-format slot in key)
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(b"fee_dist:", Direction::Forward),
        );
        for item in iter.flatten() {
            if !item.0.starts_with(b"fee_dist:") {
                break;
            }
            if let Ok(s) = std::str::from_utf8(&item.0[9..]) {
                if let Ok(slot) = s.parse::<u64>() {
                    if slot < cutoff {
                        batch.delete_cf(&cf, &item.0);
                        deleted += 1;
                    }
                }
            }
        }

        // 2. Prune reward_dist:{slot} (text-format slot in key)
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(b"reward_dist:", Direction::Forward),
        );
        for item in iter.flatten() {
            if !item.0.starts_with(b"reward_dist:") {
                break;
            }
            if let Ok(s) = std::str::from_utf8(&item.0[12..]) {
                if let Ok(slot) = s.parse::<u64>() {
                    if slot < cutoff {
                        batch.delete_cf(&cf, &item.0);
                        deleted += 1;
                    }
                }
            }
        }

        // 3. Prune esq:{program}{slot} (binary: 4 prefix + 32 pubkey + 8 BE slot = 44 bytes)
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(b"esq:", Direction::Forward),
        );
        for item in iter.flatten() {
            if !item.0.starts_with(b"esq:") {
                break;
            }
            if item.0.len() == 44 {
                let slot = u64::from_be_bytes(item.0[36..44].try_into().unwrap());
                if slot < cutoff {
                    batch.delete_cf(&cf, &item.0);
                    deleted += 1;
                }
            }
        }

        // 4. Prune tsq:{token}{slot} (binary: 4 prefix + 32 pubkey + 8 BE slot = 44 bytes)
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(b"tsq:", Direction::Forward),
        );
        for item in iter.flatten() {
            if !item.0.starts_with(b"tsq:") {
                break;
            }
            if item.0.len() == 44 {
                let slot = u64::from_be_bytes(item.0[36..44].try_into().unwrap());
                if slot < cutoff {
                    batch.delete_cf(&cf, &item.0);
                    deleted += 1;
                }
            }
        }

        // 5. Prune txs:{slot} (binary: 4 prefix + 8 BE slot = 12 bytes)
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(b"txs:", Direction::Forward),
        );
        for item in iter.flatten() {
            if !item.0.starts_with(b"txs:") {
                break;
            }
            if item.0.len() == 12 {
                let slot = u64::from_be_bytes(item.0[4..12].try_into().unwrap());
                if slot < cutoff {
                    batch.delete_cf(&cf, &item.0);
                    deleted += 1;
                }
            }
        }

        // 6. Prune dirty_acct:* keys (already processed by compute_state_root)
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(b"dirty_acct:", Direction::Forward),
        );
        for item in iter.flatten() {
            if !item.0.starts_with(b"dirty_acct:") {
                break;
            }
            batch.delete_cf(&cf, &item.0);
            deleted += 1;
        }

        // Apply batch delete atomically
        if deleted > 0 {
            self.db
                .write(batch)
                .map_err(|e| format!("Failed to prune stats: {}", e))?;

            // Reset dirty counter after pruning dirty_acct keys
            if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
                let _ = self
                    .db
                    .put_cf(&cf_stats, b"dirty_account_count", 0u64.to_le_bytes());
            }
        }

        Ok(deleted)
    }
}
// EVM address mapping methods
impl StateStore {
    /// Register EVM address mapping (EVM address → Native pubkey)
    /// Called on first transaction from an EVM address
    pub fn register_evm_address(
        &self,
        evm_address: &[u8; 20],
        native_pubkey: &Pubkey,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_MAP)
            .ok_or_else(|| "EVM Map CF not found".to_string())?;

        // Store: 20-byte EVM address → 32-byte native pubkey
        self.db
            .put_cf(&cf, evm_address, native_pubkey.0)
            .map_err(|e| format!("Failed to register EVM address: {}", e))?;

        // M3 fix: also write reverse mapping (native → EVM) for consistency with batch path
        let mut reverse_key = Vec::with_capacity(52);
        reverse_key.extend_from_slice(b"reverse:");
        reverse_key.extend_from_slice(&native_pubkey.0);
        self.db
            .put_cf(&cf, &reverse_key, evm_address)
            .map_err(|e| format!("Failed to register reverse EVM mapping: {}", e))?;

        Ok(())
    }

    /// Lookup native pubkey from EVM address
    /// Returns None if EVM address has never been used
    pub fn lookup_evm_address(&self, evm_address: &[u8; 20]) -> Result<Option<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_MAP)
            .ok_or_else(|| "EVM Map CF not found".to_string())?;

        match self.db.get_cf(&cf, evm_address) {
            Ok(Some(data)) => {
                if data.len() != 32 {
                    return Err("Invalid pubkey data in EVM map".to_string());
                }
                let mut pubkey_bytes = [0u8; 32];
                pubkey_bytes.copy_from_slice(&data);
                Ok(Some(Pubkey(pubkey_bytes)))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Parse EVM address from hex string (with or without 0x prefix)
    pub fn parse_evm_address(addr_str: &str) -> Result<[u8; 20], String> {
        let addr_str = addr_str.strip_prefix("0x").unwrap_or(addr_str);
        if addr_str.len() != 40 {
            return Err("Invalid EVM address length".to_string());
        }

        let mut bytes = [0u8; 20];
        for i in 0..20 {
            let byte_str = &addr_str[i * 2..i * 2 + 2];
            bytes[i] = u8::from_str_radix(byte_str, 16)
                .map_err(|_| "Invalid hex in EVM address".to_string())?;
        }
        Ok(bytes)
    }

    /// Get EVM account data by address
    pub fn get_evm_account(&self, evm_address: &[u8; 20]) -> Result<Option<EvmAccount>, String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_ACCOUNTS)
            .ok_or_else(|| "EVM Accounts CF not found".to_string())?;

        match self.db.get_cf(&cf, evm_address) {
            Ok(Some(data)) => bincode::deserialize(&data)
                .map(Some)
                .map_err(|e| format!("Failed to deserialize EVM account: {}", e)),
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Store EVM account data
    pub fn put_evm_account(
        &self,
        evm_address: &[u8; 20],
        account: &EvmAccount,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_ACCOUNTS)
            .ok_or_else(|| "EVM Accounts CF not found".to_string())?;

        let data = bincode::serialize(account)
            .map_err(|e| format!("Failed to serialize EVM account: {}", e))?;

        self.db
            .put_cf(&cf, evm_address, data)
            .map_err(|e| format!("Failed to store EVM account: {}", e))
    }

    /// Remove EVM account data
    pub fn clear_evm_account(&self, evm_address: &[u8; 20]) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_ACCOUNTS)
            .ok_or_else(|| "EVM Accounts CF not found".to_string())?;
        self.db
            .delete_cf(&cf, evm_address)
            .map_err(|e| format!("Failed to delete EVM account: {}", e))
    }

    /// Get EVM storage value (default 0)
    pub fn get_evm_storage(&self, evm_address: &[u8; 20], slot: &[u8; 32]) -> Result<U256, String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_STORAGE)
            .ok_or_else(|| "EVM Storage CF not found".to_string())?;

        let mut key = Vec::with_capacity(20 + 32);
        key.extend_from_slice(evm_address);
        key.extend_from_slice(slot);

        match self.db.get_cf(&cf, key) {
            Ok(Some(data)) => {
                let bytes: [u8; 32] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid EVM storage value length".to_string())?;
                Ok(U256::from_be_bytes(bytes))
            }
            Ok(None) => Ok(U256::ZERO),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Store EVM storage value
    pub fn put_evm_storage(
        &self,
        evm_address: &[u8; 20],
        slot: &[u8; 32],
        value: U256,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_STORAGE)
            .ok_or_else(|| "EVM Storage CF not found".to_string())?;

        let mut key = Vec::with_capacity(20 + 32);
        key.extend_from_slice(evm_address);
        key.extend_from_slice(slot);

        self.db
            .put_cf(&cf, key, value.to_be_bytes::<32>())
            .map_err(|e| format!("Failed to store EVM storage: {}", e))
    }

    /// Clear EVM storage for an account
    pub fn clear_evm_storage(&self, evm_address: &[u8; 20]) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_STORAGE)
            .ok_or_else(|| "EVM Storage CF not found".to_string())?;

        let prefix = evm_address;
        let iter = self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::From(prefix, Direction::Forward));
        for (key, _) in iter.flatten() {
            if !key.starts_with(prefix) {
                break;
            }
            self.db
                .delete_cf(&cf, key)
                .map_err(|e| format!("Failed to delete EVM storage: {}", e))?;
        }
        Ok(())
    }

    /// Clear a single EVM storage slot
    pub fn clear_evm_storage_slot(
        &self,
        evm_address: &[u8; 20],
        slot: &[u8; 32],
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_STORAGE)
            .ok_or_else(|| "EVM Storage CF not found".to_string())?;

        let mut key = Vec::with_capacity(20 + 32);
        key.extend_from_slice(evm_address);
        key.extend_from_slice(slot);

        self.db
            .delete_cf(&cf, key)
            .map_err(|e| format!("Failed to delete EVM storage: {}", e))
    }

    /// Store EVM transaction metadata
    pub fn put_evm_tx(&self, record: &EvmTxRecord) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_TXS)
            .ok_or_else(|| "EVM Txs CF not found".to_string())?;
        let data =
            bincode::serialize(record).map_err(|e| format!("Failed to serialize EVM tx: {}", e))?;
        self.db
            .put_cf(&cf, record.evm_hash, data)
            .map_err(|e| format!("Failed to store EVM tx: {}", e))
    }

    /// Get EVM transaction metadata
    pub fn get_evm_tx(&self, evm_hash: &[u8; 32]) -> Result<Option<EvmTxRecord>, String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_TXS)
            .ok_or_else(|| "EVM Txs CF not found".to_string())?;
        match self.db.get_cf(&cf, evm_hash) {
            Ok(Some(data)) => bincode::deserialize(&data)
                .map(Some)
                .map_err(|e| format!("Failed to deserialize EVM tx: {}", e)),
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Update EVM tx inclusion metadata
    pub fn mark_evm_tx_included(
        &self,
        evm_hash: &[u8; 32],
        slot: u64,
        block_hash: &Hash,
    ) -> Result<(), String> {
        let mut record = match self.get_evm_tx(evm_hash)? {
            Some(record) => record,
            None => return Ok(()),
        };
        record.block_slot = Some(slot);
        record.block_hash = Some(block_hash.0);
        self.put_evm_tx(&record)
    }

    /// Store EVM receipt
    pub fn put_evm_receipt(&self, receipt: &EvmReceipt) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_RECEIPTS)
            .ok_or_else(|| "EVM Receipts CF not found".to_string())?;
        let data = bincode::serialize(receipt)
            .map_err(|e| format!("Failed to serialize EVM receipt: {}", e))?;
        self.db
            .put_cf(&cf, receipt.evm_hash, data)
            .map_err(|e| format!("Failed to store EVM receipt: {}", e))
    }

    /// Get EVM receipt
    pub fn get_evm_receipt(&self, evm_hash: &[u8; 32]) -> Result<Option<EvmReceipt>, String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_RECEIPTS)
            .ok_or_else(|| "EVM Receipts CF not found".to_string())?;
        match self.db.get_cf(&cf, evm_hash) {
            Ok(Some(data)) => bincode::deserialize(&data)
                .map(Some)
                .map_err(|e| format!("Failed to deserialize EVM receipt: {}", e)),
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Update spendable balance for a native account
    pub fn set_spendable_balance(&self, pubkey: &Pubkey, shells: u64) -> Result<(), String> {
        let mut account = self
            .get_account(pubkey)?
            .unwrap_or_else(|| Account::new(0, *pubkey));
        account.spendable = shells;
        account.shells = account
            .spendable
            .saturating_add(account.staked)
            .saturating_add(account.locked);
        self.put_account(pubkey, &account)
    }

    /// Get ReefStake pool (creates if doesn't exist)
    pub fn get_reefstake_pool(&self) -> Result<ReefStakePool, String> {
        let cf = self
            .db
            .cf_handle(CF_REEFSTAKE)
            .ok_or_else(|| "ReefStake CF not found".to_string())?;

        match self.db.get_cf(&cf, b"pool") {
            Ok(Some(data)) => serde_json::from_slice(&data)
                .map_err(|e| format!("Failed to deserialize ReefStake pool: {}", e)),
            Ok(None) => {
                // Initialize new pool
                Ok(ReefStakePool::new())
            }
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Store ReefStake pool
    pub fn put_reefstake_pool(&self, pool: &ReefStakePool) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_REEFSTAKE)
            .ok_or_else(|| "ReefStake CF not found".to_string())?;

        let data = serde_json::to_vec(pool)
            .map_err(|e| format!("Failed to serialize ReefStake pool: {}", e))?;

        self.db
            .put_cf(&cf, b"pool", data)
            .map_err(|e| format!("Failed to store ReefStake pool: {}", e))
    }

    // ─── Contract Event Storage ──────────────────────────────────────────────

    /// Store a contract event. Key: program_pubkey + slot(BE) + seq_counter
    pub fn put_contract_event(
        &self,
        program: &Pubkey,
        event: &ContractEvent,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVENTS)
            .ok_or_else(|| "Events CF not found".to_string())?;

        // Atomic sequence counter per program+slot
        let seq = self.next_event_seq(program, event.slot)?;

        let mut key = Vec::with_capacity(32 + 8 + 8);
        key.extend_from_slice(&program.0);
        key.extend_from_slice(&event.slot.to_be_bytes());
        key.extend_from_slice(&seq.to_be_bytes());

        let data =
            serde_json::to_vec(event).map_err(|e| format!("Failed to serialize event: {}", e))?;

        self.db
            .put_cf(&cf, &key, &data)
            .map_err(|e| format!("Failed to store event: {}", e))?;

        // Write slot secondary index: slot(8,BE) + program(32) + seq(8,BE) -> event_key
        // Enables O(prefix) lookup of events by slot instead of full CF scan
        if let Some(cf_slot) = self.db.cf_handle(CF_EVENTS_BY_SLOT) {
            let mut slot_key = Vec::with_capacity(8 + 32 + 8);
            slot_key.extend_from_slice(&event.slot.to_be_bytes());
            slot_key.extend_from_slice(&program.0);
            slot_key.extend_from_slice(&seq.to_be_bytes());
            self.db
                .put_cf(&cf_slot, &slot_key, &key)
                .map_err(|e| format!("Failed to store event slot index: {}", e))?;
        }

        Ok(())
    }

    /// Get events for a specific program, newest first, with limit
    pub fn get_events_by_program(
        &self,
        program: &Pubkey,
        limit: usize,
    ) -> Result<Vec<ContractEvent>, String> {
        let cf = self
            .db
            .cf_handle(CF_EVENTS)
            .ok_or_else(|| "Events CF not found".to_string())?;

        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&program.0);

        // Create an end key that is one past the prefix for reverse iteration
        let mut end_key = prefix.clone();
        end_key.extend_from_slice(&[0xFF; 16]); // past any valid slot+seq

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&end_key, Direction::Reverse),
        );

        let mut events = Vec::new();
        for (key, value) in iter.flatten() {
            if !key.starts_with(&prefix) {
                break;
            }
            if let Ok(event) = serde_json::from_slice::<ContractEvent>(&value) {
                events.push(event);
                if events.len() >= limit {
                    break;
                }
            }
        }
        Ok(events)
    }

    /// Get all events across all programs for a given slot
    pub fn get_events_by_slot(
        &self,
        slot: u64,
        limit: usize,
    ) -> Result<Vec<ContractEvent>, String> {
        // Use slot secondary index for O(prefix) lookup instead of full CF scan
        let cf_slot = self
            .db
            .cf_handle(CF_EVENTS_BY_SLOT)
            .ok_or_else(|| "Events-by-slot CF not found".to_string())?;
        let cf_events = self
            .db
            .cf_handle(CF_EVENTS)
            .ok_or_else(|| "Events CF not found".to_string())?;

        let slot_prefix = slot.to_be_bytes();
        let iter = self.db.iterator_cf(
            &cf_slot,
            rocksdb::IteratorMode::From(&slot_prefix, Direction::Forward),
        );

        let mut events = Vec::new();
        for item in iter.flatten() {
            let (key, event_key) = item;
            // Stop when we've moved past this slot's prefix
            if key.len() < 8 || key[..8] != slot_prefix {
                break;
            }
            // Look up the actual event from CF_EVENTS using the stored event_key
            if let Ok(Some(data)) = self.db.get_cf(&cf_events, &*event_key) {
                if let Ok(event) = serde_json::from_slice::<ContractEvent>(&data) {
                    events.push(event);
                    if events.len() >= limit {
                        break;
                    }
                }
            }
        }
        Ok(events)
    }

    /// Atomic event sequence counter per program+slot
    fn next_event_seq(&self, program: &Pubkey, slot: u64) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut key = Vec::with_capacity(4 + 32 + 8);
        key.extend_from_slice(b"esq:");
        key.extend_from_slice(&program.0);
        key.extend_from_slice(&slot.to_be_bytes());

        let current = match self.db.get_cf(&cf, &key) {
            Ok(Some(data)) if data.len() == 8 => {
                u64::from_le_bytes(data.as_slice().try_into().unwrap())
            }
            _ => 0,
        };
        let next = current + 1;
        self.db
            .put_cf(&cf, &key, next.to_le_bytes())
            .map_err(|e| format!("Failed to update event seq: {}", e))?;
        Ok(current)
    }

    // ─── Token Balance Indexing ──────────────────────────────────────────────

    /// Update token balance for a holder. Key: token_program(32) + holder(32)
    pub fn update_token_balance(
        &self,
        token_program: &Pubkey,
        holder: &Pubkey,
        balance: u64,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TOKEN_BALANCES)
            .ok_or_else(|| "Token balances CF not found".to_string())?;

        let mut key = Vec::with_capacity(64);
        key.extend_from_slice(&token_program.0);
        key.extend_from_slice(&holder.0);

        // Also maintain reverse index: holder -> token_program
        let rev_cf = self
            .db
            .cf_handle(CF_HOLDER_TOKENS)
            .ok_or_else(|| "Holder tokens CF not found".to_string())?;
        let mut rev_key = Vec::with_capacity(64);
        rev_key.extend_from_slice(&holder.0);
        rev_key.extend_from_slice(&token_program.0);

        if balance == 0 {
            // Remove zero-balance entries to keep index clean
            let _ = self.db.delete_cf(&cf, &key);
            let _ = self.db.delete_cf(&rev_cf, &rev_key);
        } else {
            self.db
                .put_cf(&cf, &key, balance.to_le_bytes())
                .map_err(|e| format!("Failed to update token balance: {}", e))?;
            self.db
                .put_cf(&rev_cf, &rev_key, balance.to_le_bytes())
                .map_err(|e| format!("Failed to update holder token index: {}", e))?;
        }
        Ok(())
    }

    /// Get token balance for a specific holder
    pub fn get_token_balance(
        &self,
        token_program: &Pubkey,
        holder: &Pubkey,
    ) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_TOKEN_BALANCES)
            .ok_or_else(|| "Token balances CF not found".to_string())?;

        let mut key = Vec::with_capacity(64);
        key.extend_from_slice(&token_program.0);
        key.extend_from_slice(&holder.0);

        match self.db.get_cf(&cf, &key) {
            Ok(Some(data)) if data.len() == 8 => {
                Ok(u64::from_le_bytes(data.as_slice().try_into().unwrap()))
            }
            _ => Ok(0),
        }
    }

    /// Get all token holders for a token program with their balances
    pub fn get_token_holders(
        &self,
        token_program: &Pubkey,
        limit: usize,
    ) -> Result<Vec<(Pubkey, u64)>, String> {
        let cf = self
            .db
            .cf_handle(CF_TOKEN_BALANCES)
            .ok_or_else(|| "Token balances CF not found".to_string())?;

        let prefix = token_program.0.to_vec();
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&prefix, Direction::Forward),
        );

        let mut holders = Vec::new();
        for (key, value) in iter.flatten() {
            if !key.starts_with(&prefix) {
                break;
            }
            if key.len() == 64 && value.len() == 8 {
                let mut holder_bytes = [0u8; 32];
                holder_bytes.copy_from_slice(&key[32..64]);
                let holder = Pubkey(holder_bytes);
                let balance = u64::from_le_bytes((*value).try_into().unwrap());
                holders.push((holder, balance));
                if holders.len() >= limit {
                    break;
                }
            }
        }
        Ok(holders)
    }

    // ─── Token Transfer Indexing ─────────────────────────────────────────────

    /// Record a token transfer. Key: token_program(32) + slot(BE 8) + seq(BE 8)
    pub fn put_token_transfer(
        &self,
        token_program: &Pubkey,
        transfer: &TokenTransfer,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TOKEN_TRANSFERS)
            .ok_or_else(|| "Token transfers CF not found".to_string())?;

        let seq = self.next_transfer_seq(token_program, transfer.slot)?;

        let mut key = Vec::with_capacity(48);
        key.extend_from_slice(&token_program.0);
        key.extend_from_slice(&transfer.slot.to_be_bytes());
        key.extend_from_slice(&seq.to_be_bytes());

        let data = serde_json::to_vec(transfer)
            .map_err(|e| format!("Failed to serialize transfer: {}", e))?;

        self.db
            .put_cf(&cf, &key, data)
            .map_err(|e| format!("Failed to store token transfer: {}", e))
    }

    /// Get recent token transfers for a token program
    pub fn get_token_transfers(
        &self,
        token_program: &Pubkey,
        limit: usize,
    ) -> Result<Vec<TokenTransfer>, String> {
        let cf = self
            .db
            .cf_handle(CF_TOKEN_TRANSFERS)
            .ok_or_else(|| "Token transfers CF not found".to_string())?;

        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&token_program.0);

        let mut end_key = prefix.clone();
        end_key.extend_from_slice(&[0xFF; 16]);

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&end_key, Direction::Reverse),
        );

        let mut transfers = Vec::new();
        for (key, value) in iter.flatten() {
            if !key.starts_with(&prefix) {
                break;
            }
            if let Ok(transfer) = serde_json::from_slice::<TokenTransfer>(&value) {
                transfers.push(transfer);
                if transfers.len() >= limit {
                    break;
                }
            }
        }
        Ok(transfers)
    }

    /// Atomic transfer sequence counter per token+slot
    fn next_transfer_seq(&self, token_program: &Pubkey, slot: u64) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut key = Vec::with_capacity(4 + 32 + 8);
        key.extend_from_slice(b"tsq:");
        key.extend_from_slice(&token_program.0);
        key.extend_from_slice(&slot.to_be_bytes());

        let current = match self.db.get_cf(&cf, &key) {
            Ok(Some(data)) if data.len() == 8 => {
                u64::from_le_bytes(data.as_slice().try_into().unwrap())
            }
            _ => 0,
        };
        let next = current + 1;
        self.db
            .put_cf(&cf, &key, next.to_le_bytes())
            .map_err(|e| format!("Failed to update transfer seq: {}", e))?;
        Ok(current)
    }

    // ─── Transaction-by-Slot Index ───────────────────────────────────────────

    /// Index a transaction by slot. Key: slot(BE 8) + seq(BE 8), Value: tx hash
    pub fn index_tx_by_slot(&self, slot: u64, tx_hash: &Hash) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "TX by slot CF not found".to_string())?;

        let seq = self.next_tx_slot_seq(slot)?;

        let mut key = Vec::with_capacity(16);
        key.extend_from_slice(&slot.to_be_bytes());
        key.extend_from_slice(&seq.to_be_bytes());

        self.db
            .put_cf(&cf, &key, tx_hash.0)
            .map_err(|e| format!("Failed to index tx by slot: {}", e))
    }

    /// Get transactions for a slot
    pub fn get_txs_by_slot(&self, slot: u64, limit: usize) -> Result<Vec<Hash>, String> {
        let cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "TX by slot CF not found".to_string())?;

        let prefix = slot.to_be_bytes().to_vec();
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&prefix, Direction::Forward),
        );

        let mut hashes = Vec::new();
        for (key, value) in iter.flatten() {
            if !key.starts_with(&prefix) {
                break;
            }
            if value.len() == 32 {
                let mut hash_bytes = [0u8; 32];
                hash_bytes.copy_from_slice(&value);
                hashes.push(Hash(hash_bytes));
                if hashes.len() >= limit {
                    break;
                }
            }
        }
        Ok(hashes)
    }

    // ─── Transaction-to-Slot Reverse Index (O(1) tx→slot lookup) ────────────

    /// Look up the slot a transaction was included in, by its signature hash.
    /// Returns O(1) via the CF_TX_TO_SLOT reverse index.
    pub fn get_tx_slot(&self, sig: &Hash) -> Result<Option<u64>, String> {
        let cf = self
            .db
            .cf_handle(CF_TX_TO_SLOT)
            .ok_or_else(|| "TX to slot CF not found".to_string())?;

        match self.db.get_cf(&cf, sig.0) {
            Ok(Some(data)) if data.len() == 8 => {
                let slot = u64::from_le_bytes(data.as_slice().try_into().unwrap());
                Ok(Some(slot))
            }
            Ok(_) => Ok(None),
            Err(e) => Err(format!("Database error looking up tx slot: {}", e)),
        }
    }

    /// Index a transaction signature → slot for O(1) reverse lookup.
    pub fn index_tx_to_slot(&self, sig: &Hash, slot: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TX_TO_SLOT)
            .ok_or_else(|| "TX to slot CF not found".to_string())?;

        self.db
            .put_cf(&cf, sig.0, slot.to_le_bytes())
            .map_err(|e| format!("Failed to index tx to slot: {}", e))
    }

    fn next_tx_slot_seq(&self, slot: u64) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut key = Vec::with_capacity(12);
        key.extend_from_slice(b"txs:");
        key.extend_from_slice(&slot.to_be_bytes());

        let current = match self.db.get_cf(&cf, &key) {
            Ok(Some(data)) if data.len() == 8 => {
                u64::from_le_bytes(data.as_slice().try_into().unwrap())
            }
            _ => 0,
        };
        let next = current + 1;
        self.db
            .put_cf(&cf, &key, next.to_le_bytes())
            .map_err(|e| format!("Failed to update tx slot seq: {}", e))?;
        Ok(current)
    }

    // ─── Program Listing (for getAllContracts RPC) ──────────────────────────

    /// Get all deployed programs/contracts
    pub fn get_all_programs(&self, limit: usize) -> Result<Vec<(Pubkey, Value)>, String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAMS)
            .ok_or_else(|| "Programs CF not found".to_string())?;

        let iter = self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
        let mut programs = Vec::new();

        for (key, value) in iter.flatten() {
            if key.len() == 32 {
                let mut pk_bytes = [0u8; 32];
                pk_bytes.copy_from_slice(&key);
                let pk = Pubkey(pk_bytes);
                let metadata: Value = serde_json::from_slice(&value).unwrap_or(Value::Null);
                programs.push((pk, metadata));
                if programs.len() >= limit {
                    break;
                }
            }
        }
        Ok(programs)
    }

    /// Get contract logs (events) for a specific program
    pub fn get_contract_logs(
        &self,
        program: &Pubkey,
        limit: usize,
    ) -> Result<Vec<ContractEvent>, String> {
        self.get_events_by_program(program, limit)
    }

    /// Reconcile active account count with actual database
    pub fn reconcile_active_account_count(&self) -> Result<(), String> {
        let actual_count = self.count_active_accounts()?;
        let mut counter = self
            .metrics
            .active_accounts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *counter = actual_count;
        self.metrics.save(&self.db)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_state_store() {
        let temp_dir = tempdir().unwrap();
        let state = StateStore::open(temp_dir.path()).unwrap();

        let pubkey = Pubkey([1u8; 32]);
        let account = Account::new(100, pubkey);

        // Store
        state.put_account(&pubkey, &account).unwrap();

        // Retrieve
        let retrieved = state.get_account(&pubkey).unwrap().unwrap();
        assert_eq!(retrieved.shells, Account::molt_to_shells(100));
    }

    #[test]
    fn test_transfer() {
        let temp_dir = tempdir().unwrap();
        let state = StateStore::open(temp_dir.path()).unwrap();

        let alice = Pubkey([1u8; 32]);
        let bob = Pubkey([2u8; 32]);

        // Create Alice with 1000 MOLT
        let alice_account = Account::new(1000, alice);
        state.put_account(&alice, &alice_account).unwrap();

        // Transfer 100 MOLT to Bob
        let shells = Account::molt_to_shells(100);
        state.transfer(&alice, &bob, shells).unwrap();

        // Check balances
        assert_eq!(
            state.get_balance(&alice).unwrap(),
            Account::molt_to_shells(900)
        );
        assert_eq!(
            state.get_balance(&bob).unwrap(),
            Account::molt_to_shells(100)
        );
    }

    #[test]
    fn test_state_root_deterministic() {
        let temp1 = tempdir().unwrap();
        let state1 = StateStore::open(temp1.path()).unwrap();

        let temp2 = tempdir().unwrap();
        let state2 = StateStore::open(temp2.path()).unwrap();

        // Same accounts in both states
        let pk_a = Pubkey([1u8; 32]);
        let pk_b = Pubkey([2u8; 32]);
        state1.put_account(&pk_a, &Account::new(100, pk_a)).unwrap();
        state1.put_account(&pk_b, &Account::new(200, pk_b)).unwrap();

        state2.put_account(&pk_a, &Account::new(100, pk_a)).unwrap();
        state2.put_account(&pk_b, &Account::new(200, pk_b)).unwrap();

        let root1 = state1.compute_state_root();
        let root2 = state2.compute_state_root();
        assert_eq!(root1, root2, "Same accounts should produce same state root");
    }

    #[test]
    fn test_state_root_changes_on_mutation() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let pk = Pubkey([1u8; 32]);
        state.put_account(&pk, &Account::new(100, pk)).unwrap();
        let root1 = state.compute_state_root();

        state.put_account(&pk, &Account::new(200, pk)).unwrap();
        let root2 = state.compute_state_root();

        assert_ne!(
            root1, root2,
            "Changed balance should produce different state root"
        );
    }

    #[test]
    fn test_fee_config_roundtrip() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let config = crate::FeeConfig {
            base_fee: 5_000,
            contract_deploy_fee: 1_000_000,
            contract_upgrade_fee: 500_000,
            nft_mint_fee: 100_000,
            nft_collection_fee: 200_000,
            fee_burn_percent: 40,
            fee_producer_percent: 35,
            fee_voters_percent: 15,
            fee_treasury_percent: 10,
        };

        state.set_fee_config_full(&config).unwrap();

        let loaded = state.get_fee_config().unwrap();
        assert_eq!(loaded.base_fee, 5_000);
        assert_eq!(loaded.fee_burn_percent, 40);
        assert_eq!(loaded.fee_producer_percent, 35);
        assert_eq!(loaded.fee_voters_percent, 15);
    }

    #[test]
    fn test_recent_blockhashes() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // Store a few blocks
        let h1 = Hash::hash(b"block1");
        let _h2 = Hash::hash(b"block2");
        let block1 = crate::Block::new_with_timestamp(
            1,
            Hash::default(),
            Hash::default(),
            [0u8; 32],
            vec![],
            100,
        );
        let block2 =
            crate::Block::new_with_timestamp(2, h1, Hash::default(), [0u8; 32], vec![], 200);

        state.put_block(&block1).unwrap();
        state.put_block(&block2).unwrap();
        state.set_last_slot(2).unwrap();

        let recent = state.get_recent_blockhashes(10).unwrap();
        // Should contain the block hashes we stored (not Hash::default() anymore — T1.3)
        assert!(
            recent.len() >= 2,
            "Should contain at least the 2 stored block hashes"
        );
        assert!(
            recent.contains(&block1.hash()),
            "Should contain block1's hash"
        );
        assert!(
            recent.contains(&block2.hash()),
            "Should contain block2's hash"
        );
    }

    // ── H3 tests: StateBatch::apply_evm_changes ──

    #[test]
    fn test_apply_evm_changes_writes_account_and_storage() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();
        let changes = vec![crate::evm::EvmStateChange {
            evm_address: [0xAA; 20],
            account: Some(crate::evm::EvmAccount {
                nonce: 5,
                balance: [0u8; 32],
                code: vec![0xAB, 0xCD],
            }),
            storage_changes: vec![
                ([0x01; 32], Some(alloy_primitives::U256::from(42u64))),
                ([0x02; 32], Some(alloy_primitives::U256::from(99u64))),
            ],
            native_balance_update: None,
        }];

        let mut batch = state.begin_batch();
        batch.apply_evm_changes(&changes).unwrap();
        state.commit_batch(batch).unwrap();

        // Verify the EVM account was written
        let stored = state.get_evm_account(&[0xAA; 20]).unwrap();
        assert!(stored.is_some());
        let acct = stored.unwrap();
        assert_eq!(acct.nonce, 5);
        assert_eq!(acct.code, vec![0xABu8, 0xCD]);

        // Verify storage (returns U256::ZERO for missing, non-zero for present)
        let val1 = state.get_evm_storage(&[0xAA; 20], &[0x01; 32]).unwrap();
        assert_ne!(val1, alloy_primitives::U256::ZERO);
        let val2 = state.get_evm_storage(&[0xAA; 20], &[0x02; 32]).unwrap();
        assert_ne!(val2, alloy_primitives::U256::ZERO);
    }

    #[test]
    fn test_apply_evm_changes_clears_account() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // First write an account
        let create = vec![crate::evm::EvmStateChange {
            evm_address: [0xBB; 20],
            account: Some(crate::evm::EvmAccount {
                nonce: 1,
                balance: [0u8; 32],
                code: vec![],
            }),
            storage_changes: vec![([0x01; 32], Some(alloy_primitives::U256::from(10u64)))],
            native_balance_update: None,
        }];
        let mut batch = state.begin_batch();
        batch.apply_evm_changes(&create).unwrap();
        state.commit_batch(batch).unwrap();
        assert!(state.get_evm_account(&[0xBB; 20]).unwrap().is_some());

        // Now clear it (account = None → self-destruct)
        let clear = vec![crate::evm::EvmStateChange {
            evm_address: [0xBB; 20],
            account: None,
            storage_changes: vec![],
            native_balance_update: None,
        }];
        let mut batch2 = state.begin_batch();
        batch2.apply_evm_changes(&clear).unwrap();
        state.commit_batch(batch2).unwrap();

        // Account and storage should be gone
        assert!(state.get_evm_account(&[0xBB; 20]).unwrap().is_none());
        assert_eq!(
            state.get_evm_storage(&[0xBB; 20], &[0x01; 32]).unwrap(),
            alloy_primitives::U256::ZERO
        );
    }

    #[test]
    fn test_apply_evm_changes_native_balance_update() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let pk = Pubkey([0x77; 32]);
        state.put_account(&pk, &Account::new(100, pk)).unwrap();

        let new_spendable = 500_000_000u64; // 0.5 MOLT in shells
        let changes = vec![crate::evm::EvmStateChange {
            evm_address: [0xCC; 20],
            account: Some(crate::evm::EvmAccount {
                nonce: 0,
                balance: [0u8; 32],
                code: vec![],
            }),
            storage_changes: vec![],
            native_balance_update: Some((pk, new_spendable)),
        }];

        let mut batch = state.begin_batch();
        batch.apply_evm_changes(&changes).unwrap();
        state.commit_batch(batch).unwrap();

        let acct = state.get_account(&pk).unwrap().unwrap();
        assert_eq!(acct.spendable, new_spendable);
    }
}
