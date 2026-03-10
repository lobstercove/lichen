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

/// Type alias for bulk key-value export results to satisfy clippy::type_complexity.
pub type KvEntries = Vec<(Vec<u8>, Vec<u8>)>;

/// Page of key-value entries returned by paginated export functions.
pub struct KvPage {
    /// The entries in this page.
    pub entries: KvEntries,
    /// Total number of entries in the column family (across all pages).
    pub total: u64,
    /// Cursor key for the next page (exclusive). None when there are no more pages.
    pub next_cursor: Option<Vec<u8>>,
    /// Whether more entries are available after this page.
    pub has_more: bool,
}

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
                                                // Shielded pool (ZK privacy layer)
const CF_SHIELDED_COMMITMENTS: &str = "shielded_commitments"; // index(8,LE) -> commitment_leaf(32)
const CF_SHIELDED_NULLIFIERS: &str = "shielded_nullifiers"; // nullifier(32) -> 0x01 (spent flag)
const CF_SHIELDED_POOL: &str = "shielded_pool"; // singleton key "state" -> ShieldedPoolState (JSON)

// ─── P2-3: Cold storage column family names ─────────────────────────────────
// Cold DB mirrors a subset of hot CFs for archival data (old blocks, txns).
const COLD_CF_BLOCKS: &str = "blocks";
const COLD_CF_TRANSACTIONS: &str = "transactions";
const COLD_CF_TX_TO_SLOT: &str = "tx_to_slot";

/// Default number of slots to retain in the hot DB before migration-eligible.
/// Blocks older than `current_slot - COLD_RETENTION_SLOTS` can be moved.
pub const COLD_RETENTION_SLOTS: u64 = 100_000;

// ─── PERF-OPT 3: In-process blockhash cache ─────────────────────────────────

/// Cached (slot, hash) pairs for the recent 300 slots.
struct BlockhashCache {
    /// Sorted by slot (oldest first). Capped to ~300 entries.
    entries: Vec<(u64, Hash)>,
}

// AUDIT-FIX C-7: Blockhash cache moved from static global to StateStore instance field
// so that each store instance has its own cache (avoids cross-instance pollution in tests).

/// Token symbol registry entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolRegistryEntry {
    pub symbol: String,
    pub program: Pubkey,
    pub owner: Pubkey,
    pub name: Option<String>,
    pub template: Option<String>,
    pub metadata: Option<Value>,
    #[serde(default)]
    pub decimals: Option<u8>,
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
    pub peak_tps: f64,
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

        // Update peak TPS
        let peak_tps = {
            let mut peak = self.peak_tps.lock().unwrap_or_else(|e| e.into_inner());
            if tps > *peak {
                *peak = tps;
            }
            *peak
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
            peak_tps,
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

        // Load program count
        if let Ok(Some(data)) = db.get_cf(&cf, b"program_count") {
            if let Ok(bytes) = data.as_slice().try_into() {
                let count = u64::from_le_bytes(bytes);
                *self.program_count.lock().unwrap_or_else(|e| e.into_inner()) = count;
            }
        }

        // Load validator count
        if let Ok(Some(data)) = db.get_cf(&cf, b"validator_count") {
            if let Ok(bytes) = data.as_slice().try_into() {
                let count = u64::from_le_bytes(bytes);
                *self
                    .validator_count
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = count;
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
    #[allow(dead_code)]
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
        let mut c = self
            .validator_count
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        *c = c.saturating_sub(1);
    }

    /// Get validator count (no DB scan)
    pub fn get_validator_count(&self) -> u64 {
        *self
            .validator_count
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

        // Save program count
        let pc = *self.program_count.lock().unwrap_or_else(|e| e.into_inner());
        db.put_cf(&cf, b"program_count", pc.to_le_bytes())
            .map_err(|e| format!("Failed to save program count: {}", e))?;

        // Save validator count
        let vc = *self
            .validator_count
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        db.put_cf(&cf, b"validator_count", vc.to_le_bytes())
            .map_err(|e| format!("Failed to save validator count: {}", e))?;

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
    /// Optional cold/archival DB for historical blocks and transactions.
    /// When present, `get_block_by_slot` and `get_transaction` fall through
    /// to cold storage if the key is missing from the hot DB. Populated by
    /// `migrate_to_cold()` which moves old data out of the hot DB.
    cold_db: Option<Arc<DB>>,
    metrics: Arc<MetricsStore>,
    /// AUDIT-FIX H6: Mutex to serialize next_event_seq read-modify-write operations,
    /// preventing duplicate sequence numbers under concurrent access.
    event_seq_lock: Arc<std::sync::Mutex<()>>,
    /// AUDIT-FIX CP-8: Mutex to serialize next_transfer_seq read-modify-write operations,
    /// preventing duplicate transfer sequence numbers under concurrent access.
    transfer_seq_lock: Arc<std::sync::Mutex<()>>,
    /// PHASE1-FIX S-2: Mutex to serialize next_tx_slot_seq read-modify-write operations,
    /// preventing duplicate tx sequence numbers under concurrent block processing.
    tx_slot_seq_lock: Arc<std::sync::Mutex<()>>,
    /// P10-CORE-01: Mutex to serialize add_burned read-modify-write operations,
    /// preventing lost updates under concurrent access.
    burned_lock: Arc<std::sync::Mutex<()>>,
    /// AUDIT-FIX B-1: Mutex to serialize treasury read-modify-write in charge_fee_direct,
    /// preventing lost-update race when parallel TX groups credit fees concurrently.
    treasury_lock: Arc<std::sync::Mutex<()>>,
    /// AUDIT-FIX C-7: Per-instance blockhash cache (was previously a static global).
    /// Populated lazily on first `get_recent_blockhashes`, kept warm by `push_blockhash_cache`.
    blockhash_cache: Arc<Mutex<Option<BlockhashCache>>>,
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
    /// AUDIT-FIX CP-7: Track symbols registered within this batch to catch duplicates
    symbol_overlay: std::collections::HashSet<String>,
    /// Track nullifiers marked spent inside this batch so reads are batch-consistent.
    spent_nullifier_overlay: std::collections::HashSet<[u8; 32]>,
    /// AUDIT-FIX H-1: Governed proposal overlay so proposals participate in batch atomicity.
    governed_proposal_overlay: std::collections::HashMap<u64, crate::multisig::GovernedProposal>,
    /// AUDIT-FIX H-1: Governed proposal counter override (set on first alloc in this batch).
    governed_proposal_counter: Option<u64>,
    /// Track newly indexed programs in this batch (applied on commit)
    new_programs: i64,
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
        Self::open_with_cache_mb(path, None)
    }

    /// Open the state store with a configurable LRU cache size.
    ///
    /// `cache_mb`: If `Some(n)`, use `n` MB for the shared block cache.
    ///             If `None`, auto-detect: use 25% of total RAM, capped at 4096 MB, floor 256 MB.
    ///
    /// Each CF gets custom Options based on its access pattern:
    /// - Point-lookup CFs (accounts, transactions, blocks): bloom filters, larger block cache
    /// - Prefix-scan CFs (account_txs, nft_*, program_calls): prefix bloom + prefix extractor
    /// - Small/singleton CFs (stats, validators, stake_pool): minimal config
    /// - Write-heavy CFs (events, token_transfers): larger write buffers
    pub fn open_with_cache_mb<P: AsRef<Path>>(
        path: P,
        cache_mb: Option<usize>,
    ) -> Result<Self, String> {
        // ── Global DB options ────────────────────────────────────────
        let mut db_opts = Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);
        db_opts.set_max_open_files(4096);
        db_opts.set_keep_log_file_num(5);
        db_opts.set_max_total_wal_size(256 * 1024 * 1024); // 256MB WAL limit
        db_opts.set_wal_recovery_mode(rocksdb::DBRecoveryMode::PointInTime);
        db_opts.set_wal_bytes_per_sync(1024 * 1024);
        db_opts.set_bytes_per_sync(1024 * 1024); // 1MB sync granularity
        db_opts.increase_parallelism(num_cpus());
        db_opts.set_max_background_jobs(4);

        // ── Shared block cache: configurable LRU ─────────────────────
        let cache_size_mb = cache_mb.unwrap_or_else(|| {
            // Auto-detect: 25% of total RAM, capped at 4GB, floor 256MB
            #[cfg(target_os = "linux")]
            {
                if let Ok(meminfo) = std::fs::read_to_string("/proc/meminfo") {
                    if let Some(line) = meminfo.lines().find(|l| l.starts_with("MemTotal:")) {
                        if let Some(kb_str) = line.split_whitespace().nth(1) {
                            if let Ok(total_kb) = kb_str.parse::<usize>() {
                                let total_mb = total_kb / 1024;
                                return (total_mb / 4).clamp(256, 4096);
                            }
                        }
                    }
                }
                512 // fallback
            }
            #[cfg(target_os = "macos")]
            {
                // sysctl hw.memsize returns bytes
                use std::process::Command;
                if let Ok(output) = Command::new("sysctl").arg("-n").arg("hw.memsize").output() {
                    if let Ok(s) = String::from_utf8(output.stdout) {
                        if let Ok(bytes) = s.trim().parse::<usize>() {
                            let total_mb = bytes / (1024 * 1024);
                            return (total_mb / 4).clamp(256, 4096);
                        }
                    }
                }
                512 // fallback
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                512 // default fallback
            }
        });
        tracing::info!("🗄️  RocksDB shared cache: {} MB", cache_size_mb);
        let shared_cache = Cache::new_lru_cache(cache_size_mb * 1024 * 1024);

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
            // Shielded pool (ZK privacy layer)
            ColumnFamilyDescriptor::new(CF_SHIELDED_COMMITMENTS, point_lookup_opts(8)), // key=index(8,LE)->commitment(32)
            ColumnFamilyDescriptor::new(CF_SHIELDED_NULLIFIERS, point_lookup_opts(32)), // key=nullifier(32)->0x01
            ColumnFamilyDescriptor::new(CF_SHIELDED_POOL, small_cf_opts()), // singleton pool state
        ];

        let db = DB::open_cf_descriptors(&db_opts, path, cfs)
            .map_err(|e| format!("Failed to open database: {}", e))?;

        let db_arc = Arc::new(db);
        let metrics = Arc::new(MetricsStore::new());

        // Load existing metrics from database
        metrics.load(&db_arc)?;

        Ok(StateStore {
            db: db_arc,
            cold_db: None,
            metrics,
            event_seq_lock: Arc::new(std::sync::Mutex::new(())),
            transfer_seq_lock: Arc::new(std::sync::Mutex::new(())),
            tx_slot_seq_lock: Arc::new(std::sync::Mutex::new(())),
            burned_lock: Arc::new(std::sync::Mutex::new(())),
            treasury_lock: Arc::new(std::sync::Mutex::new(())),
            blockhash_cache: Arc::new(Mutex::new(None)),
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

    /// Get the last confirmed slot (2/3 supermajority reached)
    pub fn get_last_confirmed_slot(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        match self.db.get_cf(&cf, b"confirmed_slot") {
            Ok(Some(data)) => {
                let bytes: [u8; 8] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid confirmed slot data".to_string())?;
                Ok(u64::from_le_bytes(bytes))
            }
            Ok(None) => Ok(0),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Update the last confirmed slot
    pub fn set_last_confirmed_slot(&self, slot: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        self.db
            .put_cf(&cf, b"confirmed_slot", slot.to_le_bytes())
            .map_err(|e| format!("Failed to store confirmed slot: {}", e))
    }

    /// Get the last finalized slot (confirmed + 32 slots deep)
    pub fn get_last_finalized_slot(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        match self.db.get_cf(&cf, b"finalized_slot") {
            Ok(Some(data)) => {
                let bytes: [u8; 8] = data
                    .as_slice()
                    .try_into()
                    .map_err(|_| "Invalid finalized slot data".to_string())?;
                Ok(u64::from_le_bytes(bytes))
            }
            Ok(None) => Ok(0),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Update the last finalized slot
    pub fn set_last_finalized_slot(&self, slot: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;

        self.db
            .put_cf(&cf, b"finalized_slot", slot.to_le_bytes())
            .map_err(|e| format!("Failed to store finalized slot: {}", e))
    }

    /// Get the hashes of the last N blocks for replay protection.
    /// Returns a set of block hashes from the most recent `count` slots.
    /// T1.3 fix: Hash::default() is NO LONGER accepted. Only real block hashes
    /// are valid for replay protection. Genesis block hash is included if in range.
    ///
    /// PERF-OPT 3: Uses an in-memory cache that is populated on block commit
    /// and avoids reading up to 300 blocks from RocksDB on every call.
    pub fn get_recent_blockhashes(
        &self,
        count: u64,
    ) -> Result<std::collections::HashSet<Hash>, String> {
        // Fast path: check the in-process cache
        {
            let cache = self
                .blockhash_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(ref inner) = *cache {
                // Cache is valid — return all hashes within the requested window
                let last_slot = self.get_last_slot()?;
                let start_slot = last_slot.saturating_sub(count);
                let hashes: std::collections::HashSet<Hash> = inner
                    .entries
                    .iter()
                    .filter(|(slot, _)| *slot >= start_slot)
                    .map(|(_, hash)| *hash)
                    .collect();
                if !hashes.is_empty() {
                    return Ok(hashes);
                }
                // Cache is populated but empty for this range — fall through to cold path
            }
        }

        // Cold path: read from RocksDB and populate cache
        let mut hashes = std::collections::HashSet::new();
        let last_slot = self.get_last_slot()?;
        let start_slot = last_slot.saturating_sub(count);
        let mut entries: Vec<(u64, Hash)> = Vec::new();
        for slot in start_slot..=last_slot {
            if let Ok(Some(block)) = self.get_block_by_slot(slot) {
                let h = block.hash();
                hashes.insert(h);
                entries.push((slot, h));
            }
        }

        // Warm the cache
        {
            let mut cache = self
                .blockhash_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *cache = Some(BlockhashCache { entries });
        }

        Ok(hashes)
    }

    /// PERF-OPT 3: Push a new blockhash into the in-memory cache after committing a block.
    /// Evicts entries older than 300 slots.
    fn push_blockhash_cache(&self, hash: Hash, slot: u64) {
        let mut cache = self
            .blockhash_cache
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let inner = cache.get_or_insert_with(|| BlockhashCache {
            entries: Vec::new(),
        });
        inner.entries.push((slot, hash));
        // Evict anything older than 300 slots from the newest slot
        let cutoff = slot.saturating_sub(300);
        inner.entries.retain(|(s, _)| *s >= cutoff);
    }

    /// Store a block
    ///
    /// PERF-OPT 1: All block-level, transaction, and index writes are collected
    /// into a single `WriteBatch` and committed with one WAL sync. This reduces
    /// ~1500 individual RocksDB puts (for a 500-TX block) to 1 atomic write.
    pub fn put_block(&self, block: &Block) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_BLOCKS)
            .ok_or_else(|| "Blocks CF not found".to_string())?;
        let slot_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let tx_cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;
        let tx_to_slot_cf = self
            .db
            .cf_handle(CF_TX_TO_SLOT)
            .ok_or_else(|| "TX to slot CF not found".to_string())?;
        let tx_by_slot_cf = self
            .db
            .cf_handle(CF_TX_BY_SLOT)
            .ok_or_else(|| "TX by slot CF not found".to_string())?;

        let block_hash = block.hash();
        let mut value = Vec::with_capacity(4096);
        value.push(0xBC);
        bincode::serialize_into(&mut value, block)
            .map_err(|e| format!("Failed to serialize block: {}", e))?;

        // Check if this is a new slot BEFORE writing the slot index
        // (otherwise the lookup finds our own write and metrics are never tracked)
        let is_new_slot = self
            .get_block_by_slot(block.header.slot)
            .unwrap_or(None)
            .is_none();

        let mut batch = WriteBatch::default();

        // Block data + slot index
        batch.put_cf(&cf, block_hash.0, &value);
        batch.put_cf(&slot_cf, block.header.slot.to_le_bytes(), block_hash.0);

        // Per-transaction writes: tx body + tx→slot + slot→tx indexes
        for tx in &block.transactions {
            let sig = tx.signature();

            // Serialize tx body into batch
            {
                let mut tx_value = Vec::with_capacity(512);
                tx_value.push(0xBC);
                match bincode::serialize_into(&mut tx_value, tx) {
                    Ok(()) => {
                        batch.put_cf(&tx_cf, sig.0, &tx_value);
                    }
                    Err(e) => eprintln!("Warning: failed to serialize tx {}: {}", sig.to_hex(), e),
                }
            }

            // tx hash → slot (reverse index)
            batch.put_cf(&tx_to_slot_cf, sig.0, block.header.slot.to_le_bytes());

            // slot+seq → tx hash (forward index)
            // next_tx_slot_seq still does a read+write, but the seq counter
            // must be sequential so we keep it outside the batch for correctness.
            match self.next_tx_slot_seq(block.header.slot) {
                Ok(seq) => {
                    let mut key = Vec::with_capacity(16);
                    key.extend_from_slice(&block.header.slot.to_be_bytes());
                    key.extend_from_slice(&seq.to_be_bytes());
                    batch.put_cf(&tx_by_slot_cf, &key, sig.0);
                }
                Err(e) => eprintln!(
                    "Warning: failed to get seq for tx {} by slot: {}",
                    sig.to_hex(),
                    e
                ),
            }
        }

        // AUDIT-FIX M7: Fold account-transaction indexes into the same atomic
        // WriteBatch so a crash between block commit and index write cannot
        // leave transaction history in an inconsistent state.
        self.batch_index_account_transactions(block, &mut batch)?;

        // Commit all block + tx + account-index writes atomically
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to write block batch: {}", e))?;

        // Track metrics for new slots (skip fork-choice replacements)
        if is_new_slot {
            self.metrics.track_block(block);
            self.metrics.save(&self.db)?;
        }

        // PERF-OPT 3: Update blockhash cache with newly committed block
        self.push_blockhash_cache(block_hash, block.header.slot);

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
                let block: Block = if data.first() == Some(&0xBC) {
                    bincode::deserialize(&data[1..])
                        .map_err(|e| format!("Failed to deserialize block (bincode): {}", e))?
                } else {
                    serde_json::from_slice(&data)
                        .map_err(|e| format!("Failed to deserialize block (json): {}", e))?
                };
                Ok(Some(block))
            }
            Ok(None) => {
                // P2-3: Fall through to cold storage for historical blocks
                if let Some(ref cold) = self.cold_db {
                    if let Some(cold_cf) = cold.cf_handle(COLD_CF_BLOCKS) {
                        if let Ok(Some(data)) = cold.get_cf(&cold_cf, hash.0) {
                            let block: Block = if data.first() == Some(&0xBC) {
                                bincode::deserialize(&data[1..]).map_err(|e| {
                                    format!("Failed to deserialize cold block (bincode): {}", e)
                                })?
                            } else {
                                serde_json::from_slice(&data).map_err(|e| {
                                    format!("Failed to deserialize cold block (json): {}", e)
                                })?
                            };
                            return Ok(Some(block));
                        }
                    }
                }
                Ok(None)
            }
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
        let mut value = Vec::with_capacity(512);
        value.push(0xBC);
        bincode::serialize_into(&mut value, tx)
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
                let tx: Transaction = if data.first() == Some(&0xBC) {
                    bincode::deserialize(&data[1..]).map_err(|e| {
                        format!("Failed to deserialize transaction (bincode): {}", e)
                    })?
                } else {
                    serde_json::from_slice(&data)
                        .map_err(|e| format!("Failed to deserialize transaction (json): {}", e))?
                };
                Ok(Some(tx))
            }
            Ok(None) => {
                // P2-3: Fall through to cold storage for historical transactions
                if let Some(ref cold) = self.cold_db {
                    if let Some(cold_cf) = cold.cf_handle(COLD_CF_TRANSACTIONS) {
                        if let Ok(Some(data)) = cold.get_cf(&cold_cf, sig.0) {
                            let tx: Transaction = if data.first() == Some(&0xBC) {
                                bincode::deserialize(&data[1..]).map_err(|e| {
                                    format!(
                                        "Failed to deserialize cold transaction (bincode): {}",
                                        e
                                    )
                                })?
                            } else {
                                serde_json::from_slice(&data).map_err(|e| {
                                    format!("Failed to deserialize cold transaction (json): {}", e)
                                })?
                            };
                            return Ok(Some(tx));
                        }
                    }
                }
                Ok(None)
            }
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
                let mut account: Account = if data.first() == Some(&0xBC) {
                    bincode::deserialize(&data[1..])
                        .map_err(|e| format!("Failed to deserialize account (bincode): {}", e))?
                } else {
                    serde_json::from_slice(&data)
                        .map_err(|e| format!("Failed to deserialize account (json): {}", e))?
                };
                account.fixup_legacy(); // M11 fix: repair legacy accounts missing balance separation
                Ok(Some(account))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error: {}", e)),
        }
    }

    /// Batch account lookup (single RocksDB multi_get call).
    /// Returns only accounts that exist and decode successfully.
    pub fn get_accounts_batch(
        &self,
        pubkeys: &[Pubkey],
    ) -> Result<std::collections::HashMap<Pubkey, Account>, String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        let raw = self
            .db
            .multi_get_cf(pubkeys.iter().map(|pk| (&cf, pk.0.as_ref())));

        let mut out = std::collections::HashMap::with_capacity(pubkeys.len());
        for (pk, item) in pubkeys.iter().zip(raw.into_iter()) {
            let maybe_data = item.map_err(|e| format!("Database error: {}", e))?;
            let Some(data) = maybe_data else {
                continue;
            };

            let mut account: Account = if data.first() == Some(&0xBC) {
                bincode::deserialize(&data[1..])
                    .map_err(|e| format!("Failed to deserialize account (bincode): {}", e))?
            } else {
                serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to deserialize account (json): {}", e))?
            };
            account.fixup_legacy();
            out.insert(*pk, account);
        }

        Ok(out)
    }

    /// Store account
    pub fn put_account(&self, pubkey: &Pubkey, account: &Account) -> Result<(), String> {
        // Delegate to the hint variant, which will do the extra read itself
        self.put_account_with_hint(pubkey, account, None, None)
    }

    /// PERF-OPT 5: Store account with optional hints to skip the extra read.
    ///
    /// When the caller already knows whether the account is new and/or what
    /// the old balance was (e.g. during parallel batch processing), pass those
    /// hints to avoid a redundant RocksDB get + deserialize on every put.
    ///
    /// `is_new_hint`:     Some(true/false) → skip the existence check
    /// `old_balance_hint`: Some(balance)    → skip the old-account read
    pub fn put_account_with_hint(
        &self,
        pubkey: &Pubkey,
        account: &Account,
        is_new_hint: Option<bool>,
        old_balance_hint: Option<u64>,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        // Only read the old account when we don't have hints
        let (is_new, old_balance) = match (is_new_hint, old_balance_hint) {
            (Some(new), Some(bal)) => (new, bal),
            _ => {
                // Fallback: read existing account for counter updates
                let old_account = self
                    .db
                    .get_cf(&cf, pubkey.0)
                    .map_err(|e| format!("Failed to check account: {}", e))?;
                let old_bal = old_account
                    .as_ref()
                    .and_then(|data| {
                        if data.first() == Some(&0xBC) {
                            bincode::deserialize::<Account>(&data[1..]).ok()
                        } else {
                            serde_json::from_slice::<Account>(data).ok()
                        }
                    })
                    .map(|a| a.shells)
                    .unwrap_or(0);
                let new_flag = old_account.is_none();
                (
                    is_new_hint.unwrap_or(new_flag),
                    old_balance_hint.unwrap_or(old_bal),
                )
            }
        };
        let new_balance = account.shells;

        let mut value = Vec::with_capacity(256);
        value.push(0xBC);
        bincode::serialize_into(&mut value, account)
            .map_err(|e| format!("Failed to serialize account: {}", e))?;

        self.db
            .put_cf(&cf, pubkey.0, &value)
            .map_err(|e| format!("Failed to store account: {}", e))?;

        // PERF-OPT 2: Update in-memory counters only — do NOT persist metrics
        // here. The caller (block processor / commit_batch) is responsible for
        // calling flush_metrics() once after the full block is processed.
        if is_new {
            self.metrics.increment_accounts();
        }
        // Track active accounts (non-zero balance transitions)
        if old_balance == 0 && new_balance > 0 {
            self.metrics.increment_active_accounts();
        } else if old_balance > 0 && new_balance == 0 {
            self.metrics.decrement_active_accounts();
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
                    // PERF-OPT 7: Use hash_two_parts to avoid heap allocation
                    let leaf = Hash::hash_two_parts(pk, &value);
                    batch.put_cf(&cf_leaves, pk, leaf.0);
                }
                Ok(None) => {
                    // Account deleted: remove from leaf cache
                    batch.delete_cf(&cf_leaves, pk);
                }
                Err(_) => continue,
            }
            // Remove dirty marker
            // PERF-OPT 8: Stack-allocated [u8; 43] instead of Vec for fixed-size key
            let mut dirty_key = [0u8; 43]; // 11 ("dirty_acct:") + 32 (pubkey)
            dirty_key[..11].copy_from_slice(dirty_prefix);
            dirty_key[11..43].copy_from_slice(pk);
            batch.delete_cf(&cf_stats, dirty_key);
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
    pub fn compute_state_root_cold_start(&self) -> Hash {
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
            // PERF-OPT 7: hash_two_parts avoids alloc per leaf
            let leaf = Hash::hash_two_parts(&key, &value);
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
            // PERF-OPT 7: hash_two_parts avoids alloc per leaf
            leaves.push(Hash::hash_two_parts(&key, &value));
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
    /// PERF-OPT 6: Double-buffer approach — alternates between two pre-allocated Vecs
    /// instead of allocating a new Vec per tree level. Eliminates ~log2(N) allocations.
    fn merkle_root_from_leaves(leaves: &[Hash]) -> Hash {
        if leaves.is_empty() {
            return Hash::default();
        }
        if leaves.len() == 1 {
            return leaves[0];
        }

        // Two alternating buffers to avoid per-level allocation
        let mut buf_a: Vec<Hash> = Vec::with_capacity(leaves.len().div_ceil(2));
        let mut buf_b: Vec<Hash> = Vec::with_capacity(leaves.len().div_ceil(4).max(1));
        let mut combined = [0u8; 64];

        // First level: consume input slice
        for pair in leaves.chunks(2) {
            combined[..32].copy_from_slice(&pair[0].0);
            if pair.len() == 2 {
                combined[32..].copy_from_slice(&pair[1].0);
            } else {
                // L1 fix: rehash odd leaf with itself (CVE-2012-2459 mitigation)
                combined[32..].copy_from_slice(&pair[0].0);
            }
            buf_a.push(Hash::hash(&combined));
        }

        // Subsequent levels: alternate between buf_a and buf_b
        let mut use_a = true;
        while (if use_a { &buf_a } else { &buf_b }).len() > 1 {
            let (src, dst) = if use_a {
                (&buf_a as &Vec<Hash>, &mut buf_b)
            } else {
                (&buf_b as &Vec<Hash>, &mut buf_a)
            };
            dst.clear();
            for pair in src.chunks(2) {
                combined[..32].copy_from_slice(&pair[0].0);
                if pair.len() == 2 {
                    combined[32..].copy_from_slice(&pair[1].0);
                } else {
                    combined[32..].copy_from_slice(&pair[0].0);
                }
                dst.push(Hash::hash(&combined));
            }
            use_a = !use_a;
        }

        if use_a {
            buf_a[0]
        } else {
            buf_b[0]
        }
    }

    /// Fast state root check: returns cached root if no accounts modified since last computation
    #[allow(dead_code)]
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
            // PERF-OPT 8: Stack-allocated [u8; 43] instead of heap Vec
            let mut key = [0u8; 43]; // 11 ("dirty_acct:") + 32 (pubkey)
            key[..11].copy_from_slice(b"dirty_acct:");
            key[11..43].copy_from_slice(&pubkey.0);
            let _ = self.db.put_cf(&cf, key, []);

            // PERF-OPT 9: Write a non-zero marker instead of read-modify-write.
            // compute_state_root_cached() only checks dirty == 0 vs non-zero,
            // so incrementing is unnecessary. This eliminates a RocksDB GET on
            // every account write (hot path during block processing).
            let _ = self
                .db
                .put_cf(&cf, b"dirty_account_count", 1u64.to_le_bytes());
        }
    }

    /// Legacy mark_account_dirty (no pubkey) — sets dirty flag only.
    /// Prefer mark_account_dirty_with_key() for incremental Merkle support.
    #[allow(dead_code)]
    pub fn mark_account_dirty(&self) {
        if let Some(cf) = self.db.cf_handle(CF_STATS) {
            // PERF-OPT 9: Just write non-zero instead of read-modify-write
            let _ = self
                .db
                .put_cf(&cf, b"dirty_account_count", 1u64.to_le_bytes());
        }
    }

    // ─── Shielded pool (ZK privacy layer) ───────────────────────────────

    /// Insert a note commitment into the shielded commitments column family.
    /// Key = index as u64 LE (8 bytes), value = commitment leaf (32 bytes).
    pub fn insert_shielded_commitment(
        &self,
        index: u64,
        commitment: &[u8; 32],
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_COMMITMENTS)
            .ok_or_else(|| "Shielded commitments CF not found".to_string())?;

        self.db
            .put_cf(&cf, index.to_le_bytes(), commitment)
            .map_err(|e| format!("Failed to insert shielded commitment: {}", e))
    }

    /// Retrieve a commitment leaf by its insertion index.
    pub fn get_shielded_commitment(&self, index: u64) -> Result<Option<[u8; 32]>, String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_COMMITMENTS)
            .ok_or_else(|| "Shielded commitments CF not found".to_string())?;

        match self.db.get_cf(&cf, index.to_le_bytes()) {
            Ok(Some(data)) => {
                if data.len() != 32 {
                    return Err(format!(
                        "Invalid commitment length {} at index {}",
                        data.len(),
                        index
                    ));
                }
                let mut out = [0u8; 32];
                out.copy_from_slice(&data);
                Ok(Some(out))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("Database error reading commitment: {}", e)),
        }
    }

    /// Check whether a nullifier has been spent (exists in CF_SHIELDED_NULLIFIERS).
    pub fn is_nullifier_spent(&self, nullifier: &[u8; 32]) -> Result<bool, String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_NULLIFIERS)
            .ok_or_else(|| "Shielded nullifiers CF not found".to_string())?;

        match self.db.get_cf(&cf, nullifier) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(format!("Database error checking nullifier: {}", e)),
        }
    }

    /// Mark a nullifier as spent.  Value is a single 0x01 byte (tombstone).
    pub fn mark_nullifier_spent(&self, nullifier: &[u8; 32]) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_NULLIFIERS)
            .ok_or_else(|| "Shielded nullifiers CF not found".to_string())?;

        self.db
            .put_cf(&cf, nullifier, [0x01])
            .map_err(|e| format!("Failed to mark nullifier spent: {}", e))
    }

    /// Load the singleton `ShieldedPoolState` from CF_SHIELDED_POOL.
    /// Returns `Default` (empty tree, zero balance) if not yet initialised.
    #[cfg(feature = "zk")]
    pub fn get_shielded_pool_state(&self) -> Result<crate::zk::ShieldedPoolState, String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_POOL)
            .ok_or_else(|| "Shielded pool CF not found".to_string())?;

        match self.db.get_cf(&cf, b"state") {
            Ok(Some(data)) => serde_json::from_slice(&data)
                .map_err(|e| format!("Failed to deserialize ShieldedPoolState: {}", e)),
            Ok(None) => Ok(crate::zk::ShieldedPoolState::default()),
            Err(e) => Err(format!("Database error reading shielded pool state: {}", e)),
        }
    }

    /// Persist the singleton `ShieldedPoolState` to CF_SHIELDED_POOL.
    #[cfg(feature = "zk")]
    pub fn put_shielded_pool_state(
        &self,
        state: &crate::zk::ShieldedPoolState,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_POOL)
            .ok_or_else(|| "Shielded pool CF not found".to_string())?;

        let data = serde_json::to_vec(state)
            .map_err(|e| format!("Failed to serialize ShieldedPoolState: {}", e))?;

        self.db
            .put_cf(&cf, b"state", &data)
            .map_err(|e| format!("Failed to store ShieldedPoolState: {}", e))
    }

    /// Collect all commitment leaves [0..count) from CF_SHIELDED_COMMITMENTS.
    /// Used to rebuild the in-memory Merkle tree for proof verification.
    pub fn get_all_shielded_commitments(&self, count: u64) -> Result<Vec<[u8; 32]>, String> {
        let mut leaves = Vec::with_capacity(count as usize);
        for i in 0..count {
            match self.get_shielded_commitment(i)? {
                Some(c) => leaves.push(c),
                None => {
                    return Err(format!(
                        "Missing shielded commitment at index {} (expected {})",
                        i, count
                    ))
                }
            }
        }
        Ok(leaves)
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
    #[allow(dead_code)]
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
                if account.shells > 0 {
                    count += 1;
                }
            }
        }

        Ok(count)
    }

    /// Reconcile account counter with actual database count
    /// Use this to fix discrepancies between counter and reality
    #[allow(dead_code)]
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
    /// Reads from the MoltyID contract storage via the symbol registry.
    /// Key format in CF_CONTRACT_STORAGE: program(32) + "rep:" + hex(pubkey).
    /// Returns 0 if no reputation data found.
    pub fn get_reputation(&self, pubkey: &Pubkey) -> Result<u64, String> {
        // Build the MoltyID reputation storage key: "rep:" + hex(pubkey)
        let hex_chars: &[u8; 16] = b"0123456789abcdef";
        let mut rep_key = Vec::with_capacity(4 + 64);
        rep_key.extend_from_slice(b"rep:");
        for &b in pubkey.0.iter() {
            rep_key.push(hex_chars[(b >> 4) as usize]);
            rep_key.push(hex_chars[(b & 0x0f) as usize]);
        }
        // Use get_program_storage_u64 which resolves "moltyid" → program Pubkey
        // via the symbol registry, then reads program(32) + storage_key from
        // CF_CONTRACT_STORAGE. This is the correct key format.
        Ok(self.get_program_storage_u64("moltyid", &rep_key))
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
        // AUDIT-FIX C-5: Track whether this is a new account for counter increment
        let existing = self.get_account(to)?;
        let to_existed = existing.is_some();
        let mut to_account = existing.unwrap_or_else(|| Account::new(0, *to));

        // Credit spendable balance
        to_account.add_spendable(shells)?;

        // Save both accounts atomically (H5 fix: use WriteBatch for crash safety)
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;
        let mut batch = rocksdb::WriteBatch::default();
        let mut from_bytes = Vec::with_capacity(256);
        from_bytes.push(0xBC);
        bincode::serialize_into(&mut from_bytes, &from_account)
            .map_err(|e| format!("Serialize from: {}", e))?;
        let mut to_bytes = Vec::with_capacity(256);
        to_bytes.push(0xBC);
        bincode::serialize_into(&mut to_bytes, &to_account)
            .map_err(|e| format!("Serialize to: {}", e))?;
        batch.put_cf(&cf, from.0, &from_bytes);
        batch.put_cf(&cf, to.0, &to_bytes);
        self.db
            .write(batch)
            .map_err(|e| format!("Atomic transfer write failed: {}", e))?;

        // Mark both accounts dirty for incremental Merkle
        self.mark_account_dirty_with_key(from);
        self.mark_account_dirty_with_key(to);

        // AUDIT-FIX C-5: Increment account counters when transfer creates a new account
        if !to_existed {
            self.metrics.increment_accounts();
            self.metrics.increment_active_accounts();
        }

        Ok(())
    }

    /// L4-01 fix: Atomically persist multiple account mutations and an optional
    /// burn-counter increment in a single RocksDB WriteBatch.
    ///
    /// This prevents partially-committed state when a crash occurs between
    /// sequential `put_account` calls (e.g., fee charging, reward distribution,
    /// transaction reversal). Pass `burn_delta: 0` when no burn is needed.
    pub fn atomic_put_accounts(
        &self,
        accounts: &[(&Pubkey, &Account)],
        burn_delta: u64,
    ) -> Result<(), String> {
        if accounts.is_empty() && burn_delta == 0 {
            return Ok(());
        }

        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        let mut batch = WriteBatch::default();

        // Track per-account metadata for post-commit metrics & dirty markers
        let mut meta: Vec<(&Pubkey, bool, u64, u64)> = Vec::with_capacity(accounts.len());

        for (pubkey, account) in accounts {
            // Read old state for metrics (is_new, old_balance)
            let (is_new, old_balance) = {
                let old = self
                    .db
                    .get_cf(&cf, pubkey.0)
                    .map_err(|e| format!("Failed to read account: {}", e))?;
                let old_bal = old
                    .as_ref()
                    .and_then(|data| {
                        if data.first() == Some(&0xBC) {
                            bincode::deserialize::<Account>(&data[1..]).ok()
                        } else {
                            serde_json::from_slice::<Account>(data).ok()
                        }
                    })
                    .map(|a| a.shells)
                    .unwrap_or(0);
                (old.is_none(), old_bal)
            };

            let mut value = Vec::with_capacity(256);
            value.push(0xBC);
            bincode::serialize_into(&mut value, account)
                .map_err(|e| format!("Failed to serialize account: {}", e))?;
            batch.put_cf(&cf, pubkey.0, &value);
            meta.push((pubkey, is_new, old_balance, account.shells));
        }

        // Optionally fold burn counter into the same WriteBatch
        // C-4 FIX: acquire burned_lock to prevent lost-update races.
        let _burned_guard = if burn_delta > 0 {
            let guard = self
                .burned_lock
                .lock()
                .map_err(|e| format!("burned_lock poisoned: {}", e))?;
            let cf_stats = self
                .db
                .cf_handle(CF_STATS)
                .ok_or_else(|| "Stats CF not found".to_string())?;
            let current_burned = self.get_total_burned()?;
            let new_total = current_burned.saturating_add(burn_delta);
            batch.put_cf(&cf_stats, b"total_burned", new_total.to_le_bytes());
            Some(guard)
        } else {
            None
        };

        // Commit everything in one WAL sync
        self.db
            .write(batch)
            .map_err(|e| format!("Atomic account write failed: {}", e))?;

        // Post-commit side effects: metrics + dirty markers (crash-safe because
        // they are rebuilt on startup from persisted state)
        for (pubkey, is_new, old_balance, new_balance) in meta {
            if is_new {
                self.metrics.increment_accounts();
            }
            if old_balance == 0 && new_balance > 0 {
                self.metrics.increment_active_accounts();
            } else if old_balance > 0 && new_balance == 0 {
                self.metrics.decrement_active_accounts();
            }
            self.mark_account_dirty_with_key(pubkey);
        }

        Ok(())
    }

    /// L4-01 fix: Atomically persist an account mutation together with a
    /// ReefStake pool update. The treasury debit and pool reward distribution
    /// land in a single WriteBatch to prevent partial updates on crash.
    pub fn atomic_put_account_with_reefstake(
        &self,
        acct_key: &Pubkey,
        acct: &Account,
        pool: &ReefStakePool,
    ) -> Result<(), String> {
        let cf_accounts = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;
        let cf_reef = self
            .db
            .cf_handle(CF_REEFSTAKE)
            .ok_or_else(|| "ReefStake CF not found".to_string())?;

        // Read old account state for metrics
        let (is_new, old_balance) = {
            let old = self
                .db
                .get_cf(&cf_accounts, acct_key.0)
                .map_err(|e| format!("Failed to read account: {}", e))?;
            let old_bal = old
                .as_ref()
                .and_then(|data| {
                    if data.first() == Some(&0xBC) {
                        bincode::deserialize::<Account>(&data[1..]).ok()
                    } else {
                        serde_json::from_slice::<Account>(data).ok()
                    }
                })
                .map(|a| a.shells)
                .unwrap_or(0);
            (old.is_none(), old_bal)
        };

        let mut batch = WriteBatch::default();

        // Account serialization
        let mut acct_bytes = Vec::with_capacity(256);
        acct_bytes.push(0xBC);
        bincode::serialize_into(&mut acct_bytes, acct)
            .map_err(|e| format!("Failed to serialize account: {}", e))?;
        batch.put_cf(&cf_accounts, acct_key.0, &acct_bytes);

        // ReefStake pool serialization
        let pool_bytes = serde_json::to_vec(pool)
            .map_err(|e| format!("Failed to serialize ReefStake pool: {}", e))?;
        batch.put_cf(&cf_reef, b"pool", &pool_bytes);

        self.db
            .write(batch)
            .map_err(|e| format!("Atomic account+reefstake write failed: {}", e))?;

        // Post-commit metrics
        if is_new {
            self.metrics.increment_accounts();
        }
        let new_balance = acct.shells;
        if old_balance == 0 && new_balance > 0 {
            self.metrics.increment_active_accounts();
        } else if old_balance > 0 && new_balance == 0 {
            self.metrics.decrement_active_accounts();
        }
        self.mark_account_dirty_with_key(acct_key);

        Ok(())
    }
}

/// Extracts the token recipient pubkey from a contract call instruction's args.
/// For mint(caller[32]+to[32]+amount[8]) and transfer(from[32]+to[32]+amount[8]),
/// the recipient is at args[32..64].  Returns None for non-token ops.
fn extract_token_recipient_from_ix(ix: &crate::transaction::Instruction) -> Option<Pubkey> {
    let json_str = std::str::from_utf8(&ix.data).ok()?;
    let val: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let call = val.get("Call")?;
    let function = call.get("function")?.as_str()?;
    match function {
        "mint" | "transfer" | "transfer_from" => {
            let args = call.get("args")?.as_array()?;
            if args.len() < 64 {
                return None;
            }
            let mut to_bytes = [0u8; 32];
            for (i, item) in args[32..64].iter().enumerate() {
                to_bytes[i] = item.as_u64()? as u8;
            }
            Some(Pubkey::new(to_bytes))
        }
        _ => None,
    }
}

impl StateStore {
    /// AUDIT-FIX M7: Write account-transaction indexes into the provided WriteBatch
    /// so they are committed atomically with the block data.
    fn batch_index_account_transactions(
        &self,
        block: &Block,
        batch: &mut WriteBatch,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNT_TXS)
            .ok_or_else(|| "Account txs CF not found".to_string())?;

        let cf_stats = self.db.cf_handle(CF_STATS);

        let contract_program_id = crate::processor::CONTRACT_PROGRAM_ID;

        // Track counter deltas in-memory so multiple txs touching the same
        // account within one block produce correct sequential counts.
        let mut counter_deltas: std::collections::HashMap<Pubkey, u64> =
            std::collections::HashMap::new();

        for (tx_index, tx) in block.transactions.iter().enumerate() {
            let mut accounts = std::collections::HashSet::new();
            for ix in &tx.message.instructions {
                for account in &ix.accounts {
                    accounts.insert(*account);
                }
                if ix.program_id == contract_program_id {
                    if let Some(recipient) = extract_token_recipient_from_ix(ix) {
                        accounts.insert(recipient);
                    }
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

                batch.put_cf(&cf, &key, []);

                // Increment counter using in-memory delta tracking
                if let Some(ref cf_s) = cf_stats {
                    let delta = counter_deltas.entry(account).or_insert_with(|| {
                        let mut counter_key = Vec::with_capacity(5 + 32);
                        counter_key.extend_from_slice(b"atxc:");
                        counter_key.extend_from_slice(&account.0);
                        match self.db.get_cf(cf_s, &counter_key) {
                            Ok(Some(data)) if data.len() == 8 => {
                                u64::from_le_bytes(data.as_slice().try_into().unwrap())
                            }
                            _ => 0,
                        }
                    });
                    *delta += 1;
                }
            }
        }

        // Write final counter values into the batch
        if let Some(ref cf_s) = cf_stats {
            for (account, count) in &counter_deltas {
                let mut counter_key = Vec::with_capacity(5 + 32);
                counter_key.extend_from_slice(b"atxc:");
                counter_key.extend_from_slice(&account.0);
                batch.put_cf(cf_s, &counter_key, count.to_le_bytes());
            }
        }

        Ok(())
    }

    /// Legacy non-batched version — kept for backwards compatibility.
    /// Prefer `batch_index_account_transactions` for atomic block commits.
    #[allow(dead_code)]
    fn index_account_transactions(&self, block: &Block) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNT_TXS)
            .ok_or_else(|| "Account txs CF not found".to_string())?;

        let cf_stats = self.db.cf_handle(CF_STATS);

        let contract_program_id = crate::processor::CONTRACT_PROGRAM_ID;

        for (tx_index, tx) in block.transactions.iter().enumerate() {
            let mut accounts = std::collections::HashSet::new();
            for ix in &tx.message.instructions {
                for account in &ix.accounts {
                    accounts.insert(*account);
                }
                // For contract calls (mint/transfer), also index the token
                // recipient whose pubkey is embedded in the args data, not in
                // ix.accounts.  Without this, token recipients never see the
                // mint/transfer transaction in their history.
                if ix.program_id == contract_program_id {
                    if let Some(recipient) = extract_token_recipient_from_ix(ix) {
                        accounts.insert(recipient);
                    }
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

        // Use total_order_seek to bypass prefix bloom filter and scan across slots
        let mut read_opts = rocksdb::ReadOptions::default();
        read_opts.set_total_order_seek(true);

        let iter = self.db.iterator_cf_opt(
            &cf,
            read_opts,
            rocksdb::IteratorMode::From(&seek_key, Direction::Reverse),
        );

        let mut results = Vec::new();
        for item in iter.flatten() {
            let (key, value) = item;
            if key.len() < 16 || value.len() != 32 {
                continue;
            }

            let slot = u64::from_be_bytes(
                key[0..8]
                    .try_into()
                    .map_err(|_| "Corrupt slot key in block hashes".to_string())?,
            );

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
                // AUDIT-FIX CP-13: safe conversion with length guard above
                let balance = u64::from_le_bytes(match (*value).try_into() {
                    Ok(b) => b,
                    Err(_) => continue,
                });
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

        // Only increment if this is a newly indexed program (not an update)
        let is_new = self.db.get_cf(&cf, program.0).ok().flatten().is_none();

        self.db
            .put_cf(&cf, program.0, [])
            .map_err(|e| format!("Failed to store program index: {}", e))?;

        if is_new {
            self.metrics.increment_programs();
        }
        Ok(())
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

    pub fn get_programs_paginated(
        &self,
        limit: usize,
        after: Option<&Pubkey>,
    ) -> Result<Vec<Pubkey>, String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAMS)
            .ok_or_else(|| "Programs CF not found".to_string())?;

        let mut results = Vec::new();
        let iter = if let Some(after_pk) = after {
            self.db.iterator_cf(
                &cf,
                rocksdb::IteratorMode::From(&after_pk.0, rocksdb::Direction::Forward),
            )
        } else {
            self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start)
        };

        for item in iter {
            let (key, _) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if key.len() != 32 {
                continue;
            }
            if let Some(after_pk) = after {
                if key.as_ref() == &after_pk.0[..] {
                    continue;
                }
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

    /// List symbol registry entries with cursor pagination.
    /// `after_symbol` is exclusive: results start strictly after this symbol key.
    pub fn get_all_symbol_registry_paginated(
        &self,
        limit: usize,
        after_symbol: Option<&str>,
    ) -> Result<Vec<SymbolRegistryEntry>, String> {
        let cf = self
            .db
            .cf_handle(CF_SYMBOL_REGISTRY)
            .ok_or_else(|| "Symbol registry CF not found".to_string())?;

        let normalized_after = if let Some(symbol) = after_symbol {
            Some(Self::normalize_symbol(symbol)?)
        } else {
            None
        };

        let iter = if let Some(after) = normalized_after.as_ref() {
            self.db.iterator_cf(
                &cf,
                rocksdb::IteratorMode::From(after.as_bytes(), rocksdb::Direction::Forward),
            )
        } else {
            self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start)
        };

        let mut entries = Vec::new();
        for item in iter {
            if entries.len() >= limit {
                break;
            }
            let (key, value) = item.map_err(|e| format!("Iterator error: {}", e))?;

            if let Some(after) = normalized_after.as_ref() {
                if key.as_ref() == after.as_bytes() {
                    continue;
                }
            }

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
        before_slot: Option<u64>,
    ) -> Result<Vec<crate::ProgramCallActivity>, String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAM_CALLS)
            .ok_or_else(|| "Program calls CF not found".to_string())?;

        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&program.0);

        // Build seek key: use before_slot as upper bound, or 0xFF..FF to start from newest
        let mut end_key = prefix.clone();
        if let Some(bs) = before_slot {
            end_key.extend_from_slice(&bs.to_be_bytes());
        } else {
            end_key.extend_from_slice(&[0xFF; 44]); // past any valid slot(8)+seq(4)+hash(32)
        }

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

            // When paginating, skip entries at or after before_slot (cursor is exclusive)
            if let Some(bs) = before_slot {
                if key.len() >= 40 {
                    let slot_bytes: [u8; 8] = key[32..40].try_into().unwrap_or([0xFF; 8]);
                    let slot = u64::from_be_bytes(slot_bytes);
                    if slot >= bs {
                        continue;
                    }
                }
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
            symbol_overlay: std::collections::HashSet::new(),
            spent_nullifier_overlay: std::collections::HashSet::new(),
            governed_proposal_overlay: std::collections::HashMap::new(),
            governed_proposal_counter: None,
            new_programs: 0,
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
        // C-3 FIX: acquire burned_lock to prevent lost-update races with
        // concurrent add_burned() or other commit_batch() calls.
        let mut wb = batch.batch;
        let _burned_guard = if batch.burned_delta > 0 {
            let guard = self
                .burned_lock
                .lock()
                .map_err(|e| format!("burned_lock poisoned: {}", e))?;
            if let Some(cf) = self.db.cf_handle(CF_STATS) {
                let current = self.get_total_burned().unwrap_or(0);
                let new_total = current.saturating_add(batch.burned_delta);
                wb.put_cf(&cf, b"total_burned", new_total.to_le_bytes());
            }
            Some(guard)
        } else {
            None
        };

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
        if batch.new_programs > 0 {
            for _ in 0..batch.new_programs {
                self.metrics.increment_programs();
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
        // PERF-OPT 2: Persist metrics once after the full batch commit,
        // not on every individual put_account call.
        self.metrics.save(&self.db)?;

        // Mark each modified account dirty for incremental Merkle recomputation
        for pubkey in &dirty_pubkeys {
            self.mark_account_dirty_with_key(pubkey);
        }

        Ok(())
    }

    /// PERF-OPT 2: Flush in-memory metrics counters to RocksDB.
    ///
    /// Call this once after processing a full block instead of on every
    /// `put_account`. Reduces per-block metrics I/O from O(num_accounts)
    /// to O(1) — saving ~6 RocksDB puts per account touched.
    pub fn flush_metrics(&self) -> Result<(), String> {
        self.metrics.save(&self.db)
    }
}

// ─── StateBatch Methods ──────────────────────────────────────────────

impl StateBatch {
    /// B-7: Check symbol registry against both batch overlay and committed state.
    /// Returns true if the symbol exists in either the batch overlay or committed DB.
    pub fn symbol_exists(&self, symbol: &str) -> Result<bool, String> {
        let normalized = StateStore::normalize_symbol(symbol)?;
        // Check batch overlay first
        if self.symbol_overlay.contains(&normalized) {
            return Ok(true);
        }
        // Fall back to committed state
        let cf = self
            .db
            .cf_handle(CF_SYMBOL_REGISTRY)
            .ok_or_else(|| "Symbol registry CF not found".to_string())?;
        let exists = self
            .db
            .get_cf(&cf, normalized.as_bytes())
            .map_err(|e| format!("Database error: {}", e))?
            .is_some();
        Ok(exists)
    }

    /// B-7: Get symbol registry entry from batch overlay or committed state.
    pub fn get_symbol_registry(&self, symbol: &str) -> Result<Option<SymbolRegistryEntry>, String> {
        let normalized = StateStore::normalize_symbol(symbol)?;
        let cf = self
            .db
            .cf_handle(CF_SYMBOL_REGISTRY)
            .ok_or_else(|| "Symbol registry CF not found".to_string())?;
        match self
            .db
            .get_cf(&cf, normalized.as_bytes())
            .map_err(|e| format!("Database error: {}", e))?
        {
            Some(data) => {
                let entry: SymbolRegistryEntry = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to decode symbol registry: {}", e))?;
                Ok(Some(entry))
            }
            None => {
                // If in batch overlay but not in DB yet, it exists but we can't read the entry
                // (the overlay only stores the name, not the full entry).
                // This is sufficient for the B-4 uniqueness check.
                Ok(None)
            }
        }
    }

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
                let mut account: Account = if data.first() == Some(&0xBC) {
                    bincode::deserialize(&data[1..])
                        .map_err(|e| format!("Failed to deserialize account (bincode): {}", e))?
                } else {
                    serde_json::from_slice(&data)
                        .map_err(|e| format!("Failed to deserialize account (json): {}", e))?
                };
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
                Ok(Some(data)) => {
                    let acct = if data.first() == Some(&0xBC) {
                        bincode::deserialize::<Account>(&data[1..]).ok()
                    } else {
                        serde_json::from_slice::<Account>(&data).ok()
                    };
                    acct.map(|a| a.shells)
                }
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

        let mut value = Vec::with_capacity(256);
        value.push(0xBC);
        bincode::serialize_into(&mut value, account)
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
        let mut value = Vec::with_capacity(512);
        value.push(0xBC);
        bincode::serialize_into(&mut value, tx)
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

    /// Record fee distribution hash in the batch (atomic with account writes).
    pub fn set_fee_distribution_hash(&mut self, slot: u64, hash: &Hash) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("fee_dist:{}", slot);
        self.batch.put_cf(&cf, key.as_bytes(), hash.0);
        Ok(())
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
    pub fn nft_token_id_exists(&self, collection: &Pubkey, token_id: u64) -> Result<bool, String> {
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

    /// Write contract storage key/value to CF_CONTRACT_STORAGE in the batch.
    /// Key format: program(32) + storage_key_bytes  → value_bytes
    /// Enables fast-path reads via prefix scan without deserializing the whole account.
    pub fn put_contract_storage(
        &mut self,
        program: &Pubkey,
        storage_key: &[u8],
        value: &[u8],
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;
        let mut key = Vec::with_capacity(32 + storage_key.len());
        key.extend_from_slice(&program.0);
        key.extend_from_slice(storage_key);
        self.batch.put_cf(&cf, &key, value);
        Ok(())
    }

    /// Delete a contract storage key from CF_CONTRACT_STORAGE in the batch.
    pub fn delete_contract_storage(
        &mut self,
        program: &Pubkey,
        storage_key: &[u8],
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;
        let mut key = Vec::with_capacity(32 + storage_key.len());
        key.extend_from_slice(&program.0);
        key.extend_from_slice(storage_key);
        self.batch.delete_cf(&cf, &key);
        Ok(())
    }

    /// Update token balance indexes within the batch.
    /// Mirrors StateStore::update_token_balance but writes to the batch.
    pub fn update_token_balance(
        &mut self,
        token_program: &Pubkey,
        holder: &Pubkey,
        balance: u64,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TOKEN_BALANCES)
            .ok_or_else(|| "Token balances CF not found".to_string())?;
        let rev_cf = self
            .db
            .cf_handle(CF_HOLDER_TOKENS)
            .ok_or_else(|| "Holder tokens CF not found".to_string())?;

        let mut key = Vec::with_capacity(64);
        key.extend_from_slice(&token_program.0);
        key.extend_from_slice(&holder.0);

        let mut rev_key = Vec::with_capacity(64);
        rev_key.extend_from_slice(&holder.0);
        rev_key.extend_from_slice(&token_program.0);

        if balance == 0 {
            self.batch.delete_cf(&cf, &key);
            self.batch.delete_cf(&rev_cf, &rev_key);
        } else {
            self.batch.put_cf(&cf, &key, balance.to_le_bytes());
            self.batch.put_cf(&rev_cf, &rev_key, balance.to_le_bytes());
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
        // Only count as new if not already indexed on disk
        let is_new = self.db.get_cf(&cf, program.0).ok().flatten().is_none();
        self.batch.put_cf(&cf, program.0, []);
        if is_new {
            self.new_programs += 1;
        }
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
        // AUDIT-FIX CP-7: Also check the in-batch overlay for duplicate symbols
        if self.symbol_overlay.contains(&normalized) {
            return Err(format!(
                "Symbol already registered in this batch: {}",
                normalized
            ));
        }
        let mut entry_copy = entry.clone();
        entry_copy.symbol = normalized.clone();
        let data = serde_json::to_vec(&entry_copy)
            .map_err(|e| format!("Failed to encode symbol registry: {}", e))?;
        self.batch.put_cf(&cf, normalized.as_bytes(), &data);
        // AUDIT-FIX CP-7: Track this symbol in the batch overlay
        self.symbol_overlay.insert(normalized.clone());

        // Write reverse index: program pubkey -> symbol (O(1) program→symbol lookup)
        if let Some(cf_rev) = self.db.cf_handle(CF_SYMBOL_BY_PROGRAM) {
            self.batch
                .put_cf(&cf_rev, entry.program.0, normalized.as_bytes());
        }

        Ok(())
    }

    // ─── AUDIT-FIX H-1: Governed proposal batch support ────────────

    /// Allocate the next governed proposal ID through the batch.
    /// Reads the current counter from disk (or batch override), increments it,
    /// and writes the new value into the WriteBatch so it commits atomically.
    pub fn next_governed_proposal_id(&mut self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let current = if let Some(c) = self.governed_proposal_counter {
            c
        } else {
            match self.db.get_cf(&cf, b"governed_proposal_counter") {
                Ok(Some(data)) if data.len() == 8 => {
                    u64::from_le_bytes(data[..8].try_into().unwrap())
                }
                _ => 0,
            }
        };
        let next = current + 1;
        self.governed_proposal_counter = Some(next);
        self.batch
            .put_cf(&cf, b"governed_proposal_counter", next.to_le_bytes());
        Ok(next)
    }

    /// Store a governed proposal into the batch overlay + WriteBatch.
    pub fn set_governed_proposal(
        &mut self,
        proposal: &crate::multisig::GovernedProposal,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("governed_proposal:{}", proposal.id);
        let data = serde_json::to_vec(proposal)
            .map_err(|e| format!("Failed to serialize governed proposal: {}", e))?;
        self.batch.put_cf(&cf, key.as_bytes(), &data);
        self.governed_proposal_overlay
            .insert(proposal.id, proposal.clone());
        Ok(())
    }

    /// Read a governed proposal, checking batch overlay first then disk.
    pub fn get_governed_proposal(
        &self,
        id: u64,
    ) -> Result<Option<crate::multisig::GovernedProposal>, String> {
        if let Some(p) = self.governed_proposal_overlay.get(&id) {
            return Ok(Some(p.clone()));
        }
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("governed_proposal:{}", id);
        match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) => {
                let proposal: crate::multisig::GovernedProposal = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to deserialize proposal: {}", e))?;
                Ok(Some(proposal))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("DB error loading governed proposal: {}", e)),
        }
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

    // ─── Shielded pool (ZK privacy layer) ───────────────────────────────

    /// Insert a shielded commitment into the WriteBatch.
    pub fn insert_shielded_commitment(
        &mut self,
        index: u64,
        commitment: &[u8; 32],
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_COMMITMENTS)
            .ok_or_else(|| "Shielded commitments CF not found".to_string())?;
        self.batch.put_cf(&cf, index.to_le_bytes(), commitment);
        Ok(())
    }

    /// Check whether a nullifier has been spent (checks disk only — batch
    /// nullifiers are tracked in-memory until committed).
    pub fn is_nullifier_spent(&self, nullifier: &[u8; 32]) -> Result<bool, String> {
        if self.spent_nullifier_overlay.contains(nullifier) {
            return Ok(true);
        }

        let cf = self
            .db
            .cf_handle(CF_SHIELDED_NULLIFIERS)
            .ok_or_else(|| "Shielded nullifiers CF not found".to_string())?;
        match self.db.get_cf(&cf, nullifier) {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => Err(format!("Database error checking nullifier: {}", e)),
        }
    }

    /// Mark a nullifier as spent in the WriteBatch.
    pub fn mark_nullifier_spent(&mut self, nullifier: &[u8; 32]) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_NULLIFIERS)
            .ok_or_else(|| "Shielded nullifiers CF not found".to_string())?;
        self.batch.put_cf(&cf, nullifier, [0x01]);
        self.spent_nullifier_overlay.insert(*nullifier);
        Ok(())
    }

    /// Load the singleton `ShieldedPoolState` from disk.
    #[cfg(feature = "zk")]
    pub fn get_shielded_pool_state(&self) -> Result<crate::zk::ShieldedPoolState, String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_POOL)
            .ok_or_else(|| "Shielded pool CF not found".to_string())?;
        match self.db.get_cf(&cf, b"state") {
            Ok(Some(data)) => serde_json::from_slice(&data)
                .map_err(|e| format!("Failed to deserialize ShieldedPoolState: {}", e)),
            Ok(None) => Ok(crate::zk::ShieldedPoolState::default()),
            Err(e) => Err(format!("Database error reading shielded pool state: {}", e)),
        }
    }

    /// Write the singleton `ShieldedPoolState` to the WriteBatch.
    #[cfg(feature = "zk")]
    pub fn put_shielded_pool_state(
        &mut self,
        state: &crate::zk::ShieldedPoolState,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_SHIELDED_POOL)
            .ok_or_else(|| "Shielded pool CF not found".to_string())?;
        let data = serde_json::to_vec(state)
            .map_err(|e| format!("Failed to serialize ShieldedPoolState: {}", e))?;
        self.batch.put_cf(&cf, b"state", &data);
        Ok(())
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
        // Only increment counter for newly registered validators (not updates)
        let is_new = self.db.get_cf(&cf, key).ok().flatten().is_none();

        let value = serde_json::to_vec(info)
            .map_err(|e| format!("Failed to serialize validator: {}", e))?;

        self.db
            .put_cf(&cf, key, value)
            .map_err(|e| format!("Failed to store validator: {}", e))?;

        if is_new {
            self.metrics.increment_validators();
        }
        Ok(())
    }

    /// Delete validator from state
    pub fn delete_validator(&self, pubkey: &Pubkey) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_VALIDATORS)
            .ok_or_else(|| "Validators CF not found".to_string())?;

        // Only decrement if the validator actually exists
        let exists = self.db.get_cf(&cf, pubkey.0).ok().flatten().is_some();

        self.db
            .delete_cf(&cf, pubkey.0)
            .map_err(|e| format!("Failed to delete validator: {}", e))?;

        if exists {
            self.metrics.decrement_validators();
        }
        Ok(())
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
    /// PHASE1-FIX S-4: Atomic clear-and-replace in a single WriteBatch to prevent
    /// intermediate states where validators are partially cleared.
    pub fn save_validator_set(&self, set: &crate::consensus::ValidatorSet) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_VALIDATORS)
            .ok_or_else(|| "Validators CF not found".to_string())?;

        let mut batch = rocksdb::WriteBatch::default();

        // Delete all existing validator entries
        let keys: Vec<Box<[u8]>> = self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::Start)
            .filter_map(|item| item.ok().map(|(k, _)| k))
            .collect();
        for key in &keys {
            batch.delete_cf(&cf, key);
        }

        // Insert all current validators
        for validator in set.validators() {
            let data = serde_json::to_vec(validator)
                .map_err(|e| format!("Failed to serialize validator: {}", e))?;
            batch.put_cf(&cf, validator.pubkey.0, data);
        }

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to save validator set: {}", e))?;

        // Update counter
        *self
            .metrics
            .validator_count
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = set.validators().len() as u64;
        Ok(())
    }

    /// Remove ALL validators from the CF (used before full re-save)
    pub fn clear_all_validators(&self) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_VALIDATORS)
            .ok_or_else(|| "Validators CF not found".to_string())?;

        // Collect keys, then batch-delete in a single atomic WriteBatch
        let keys: Vec<Box<[u8]>> = self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::Start)
            .filter_map(|item| item.ok().map(|(k, _)| k))
            .collect();

        if keys.is_empty() {
            return Ok(());
        }

        let mut batch = rocksdb::WriteBatch::default();
        for key in &keys {
            batch.delete_cf(&cf, key);
        }
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to clear validators: {}", e))?;

        // Reset the validator counter
        *self
            .metrics
            .validator_count
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = 0;
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

    /// Add to total burned amount.
    ///
    /// P10-CORE-01 FIX: The read-modify-write is protected by `burned_lock` to
    /// prevent lost updates when called concurrently.  The primary burn path
    /// goes through `StateBatch::add_burned()` (which accumulates a delta and
    /// commits atomically), but this direct method is also used in tests and
    /// non-batch code paths.
    pub fn add_burned(&self, amount: u64) -> Result<(), String> {
        let _guard = self
            .burned_lock
            .lock()
            .map_err(|e| format!("burned_lock poisoned: {}", e))?;

        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

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

    /// Look up a specific genesis wallet pubkey by role name.
    ///
    /// Valid roles: "validator_rewards", "community_treasury", "builder_grants",
    /// "founding_moltys", "ecosystem_partnerships", "reserve_pool".
    pub fn get_wallet_pubkey(&self, role: &str) -> Result<Option<Pubkey>, String> {
        let accounts = self.get_genesis_accounts()?;
        Ok(accounts
            .into_iter()
            .find(|(r, _, _, _)| r == role)
            .map(|(_, pk, _, _)| pk))
    }

    /// Get community treasury wallet pubkey.
    pub fn get_community_treasury_pubkey(&self) -> Result<Option<Pubkey>, String> {
        self.get_wallet_pubkey("community_treasury")
    }

    /// Get builder grants wallet pubkey.
    pub fn get_builder_grants_pubkey(&self) -> Result<Option<Pubkey>, String> {
        self.get_wallet_pubkey("builder_grants")
    }

    /// Get founding moltys wallet pubkey.
    pub fn get_founding_moltys_pubkey(&self) -> Result<Option<Pubkey>, String> {
        self.get_wallet_pubkey("founding_moltys")
    }

    /// Get ecosystem partnerships wallet pubkey.
    pub fn get_ecosystem_partnerships_pubkey(&self) -> Result<Option<Pubkey>, String> {
        self.get_wallet_pubkey("ecosystem_partnerships")
    }

    /// Get reserve pool wallet pubkey.
    pub fn get_reserve_pool_pubkey(&self) -> Result<Option<Pubkey>, String> {
        self.get_wallet_pubkey("reserve_pool")
    }

    /// Store founding moltys vesting parameters (absolute Unix timestamps + total amount).
    ///
    /// `cliff_end`: Unix timestamp when the 6-month cliff ends (first unlock).
    /// `vest_end`: Unix timestamp when vesting is fully complete (month 24).
    /// `total_amount_shells`: Total founding moltys allocation in shells.
    pub fn set_founding_vesting_params(
        &self,
        cliff_end: u64,
        vest_end: u64,
        total_amount_shells: u64,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(&cf, b"founding_vest_cliff_end", cliff_end.to_le_bytes());
        batch.put_cf(&cf, b"founding_vest_end", vest_end.to_le_bytes());
        batch.put_cf(
            &cf,
            b"founding_vest_total_amount",
            total_amount_shells.to_le_bytes(),
        );

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to store founding vesting params: {}", e))
    }

    /// Load founding moltys vesting parameters.
    /// Returns `Ok(Some((cliff_end, vest_end, total_amount_shells)))` if set.
    pub fn get_founding_vesting_params(&self) -> Result<Option<(u64, u64, u64)>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let cliff_end = match self.db.get_cf(&cf, b"founding_vest_cliff_end") {
            Ok(Some(data)) if data.len() == 8 => u64::from_le_bytes(data[..8].try_into().unwrap()),
            _ => return Ok(None),
        };
        let vest_end = match self.db.get_cf(&cf, b"founding_vest_end") {
            Ok(Some(data)) if data.len() == 8 => u64::from_le_bytes(data[..8].try_into().unwrap()),
            _ => return Ok(None),
        };
        let total_amount = match self.db.get_cf(&cf, b"founding_vest_total_amount") {
            Ok(Some(data)) if data.len() == 8 => u64::from_le_bytes(data[..8].try_into().unwrap()),
            _ => return Ok(None),
        };

        Ok(Some((cliff_end, vest_end, total_amount)))
    }

    // ========================================================================
    // GOVERNED WALLET MULTI-SIG SYSTEM
    // ========================================================================

    /// Store a governed wallet configuration (multi-sig config for distribution wallets).
    /// Key: `governed_wallet:<base58_pubkey>` in CF_STATS.
    pub fn set_governed_wallet_config(
        &self,
        wallet_pubkey: &Pubkey,
        config: &crate::multisig::GovernedWalletConfig,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("governed_wallet:{}", wallet_pubkey.to_base58());
        let data = serde_json::to_vec(config)
            .map_err(|e| format!("Failed to serialize governed wallet config: {}", e))?;
        self.db
            .put_cf(&cf, key.as_bytes(), data)
            .map_err(|e| format!("Failed to store governed wallet config: {}", e))
    }

    /// Load governed wallet configuration. Returns None if wallet is not governed.
    pub fn get_governed_wallet_config(
        &self,
        wallet_pubkey: &Pubkey,
    ) -> Result<Option<crate::multisig::GovernedWalletConfig>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("governed_wallet:{}", wallet_pubkey.to_base58());
        match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) => {
                let config: crate::multisig::GovernedWalletConfig =
                    serde_json::from_slice(&data)
                        .map_err(|e| format!("Failed to deserialize governed config: {}", e))?;
                Ok(Some(config))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("DB error loading governed wallet config: {}", e)),
        }
    }

    /// Get the next governed proposal ID (auto-incrementing counter).
    pub fn next_governed_proposal_id(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let current = match self.db.get_cf(&cf, b"governed_proposal_counter") {
            Ok(Some(data)) if data.len() == 8 => u64::from_le_bytes(data[..8].try_into().unwrap()),
            _ => 0,
        };
        let next = current + 1;
        self.db
            .put_cf(&cf, b"governed_proposal_counter", next.to_le_bytes())
            .map_err(|e| format!("Failed to update proposal counter: {}", e))?;
        Ok(next)
    }

    /// Store a governed transfer proposal.
    pub fn set_governed_proposal(
        &self,
        proposal: &crate::multisig::GovernedProposal,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("governed_proposal:{}", proposal.id);
        let data = serde_json::to_vec(proposal)
            .map_err(|e| format!("Failed to serialize governed proposal: {}", e))?;
        self.db
            .put_cf(&cf, key.as_bytes(), data)
            .map_err(|e| format!("Failed to store governed proposal: {}", e))
    }

    /// Load a governed transfer proposal by ID.
    pub fn get_governed_proposal(
        &self,
        id: u64,
    ) -> Result<Option<crate::multisig::GovernedProposal>, String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let key = format!("governed_proposal:{}", id);
        match self.db.get_cf(&cf, key.as_bytes()) {
            Ok(Some(data)) => {
                let proposal: crate::multisig::GovernedProposal = serde_json::from_slice(&data)
                    .map_err(|e| format!("Failed to deserialize proposal: {}", e))?;
                Ok(Some(proposal))
            }
            Ok(None) => Ok(None),
            Err(e) => Err(format!("DB error loading governed proposal: {}", e)),
        }
    }

    /// Store rent parameters
    /// PHASE1-FIX S-6: Atomic WriteBatch for both rent parameters.
    pub fn set_rent_params(
        &self,
        rate_shells_per_kb_month: u64,
        free_kb: u64,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(
            &cf,
            b"rent_rate_shells_per_kb_month",
            rate_shells_per_kb_month.to_le_bytes(),
        );
        batch.put_cf(&cf, b"rent_free_kb", free_kb.to_le_bytes());

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to store rent params: {}", e))
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
    /// PHASE1-FIX S-5: Single atomic WriteBatch for all 9 fee config keys.
    pub fn set_fee_config_full(&self, config: &crate::FeeConfig) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;

        let mut batch = rocksdb::WriteBatch::default();
        batch.put_cf(&cf, b"fee_base_shells", config.base_fee.to_le_bytes());
        batch.put_cf(
            &cf,
            b"fee_contract_deploy_shells",
            config.contract_deploy_fee.to_le_bytes(),
        );
        batch.put_cf(
            &cf,
            b"fee_contract_upgrade_shells",
            config.contract_upgrade_fee.to_le_bytes(),
        );
        batch.put_cf(
            &cf,
            b"fee_nft_mint_shells",
            config.nft_mint_fee.to_le_bytes(),
        );
        batch.put_cf(
            &cf,
            b"fee_nft_collection_shells",
            config.nft_collection_fee.to_le_bytes(),
        );
        batch.put_cf(
            &cf,
            b"fee_burn_percent",
            config.fee_burn_percent.to_le_bytes(),
        );
        batch.put_cf(
            &cf,
            b"fee_producer_percent",
            config.fee_producer_percent.to_le_bytes(),
        );
        batch.put_cf(
            &cf,
            b"fee_voters_percent",
            config.fee_voters_percent.to_le_bytes(),
        );
        batch.put_cf(
            &cf,
            b"fee_treasury_percent",
            config.fee_treasury_percent.to_le_bytes(),
        );
        batch.put_cf(
            &cf,
            b"fee_community_percent",
            config.fee_community_percent.to_le_bytes(),
        );

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to store fee config: {}", e))
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
            fee_community_percent: read_u64(b"fee_community_percent")?
                .unwrap_or(defaults.fee_community_percent),
        })
    }

    /// Store slot_duration_ms in CF_STATS at genesis boot.
    pub fn set_slot_duration_ms(&self, ms: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        self.db
            .put_cf(&cf, b"slot_duration_ms", ms.to_le_bytes())
            .map_err(|e| format!("Failed to store slot_duration_ms: {}", e))
    }

    /// Read slot_duration_ms from CF_STATS (defaults to 400 if not set).
    pub fn get_slot_duration_ms(&self) -> u64 {
        let cf = match self.db.cf_handle(CF_STATS) {
            Some(cf) => cf,
            None => return 400,
        };
        match self.db.get_cf(&cf, b"slot_duration_ms") {
            Ok(Some(data)) if data.len() == 8 => {
                let bytes: [u8; 8] = data.as_slice().try_into().unwrap_or([0; 8]);
                u64::from_le_bytes(bytes)
            }
            _ => 400,
        }
    }

    /// AUDIT-FIX M7: Persist slashing tracker to RocksDB for restart-proof evidence.
    pub fn put_slashing_tracker(
        &self,
        tracker: &crate::consensus::SlashingTracker,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        let data = bincode::serialize(tracker)
            .map_err(|e| format!("Failed to serialize slashing tracker: {}", e))?;
        self.db
            .put_cf(&cf, b"slashing_tracker", &data)
            .map_err(|e| format!("Failed to persist slashing tracker: {}", e))
    }

    /// AUDIT-FIX M7: Load slashing tracker from RocksDB.
    /// Returns default empty tracker if not found or on deserialization error.
    pub fn get_slashing_tracker(&self) -> crate::consensus::SlashingTracker {
        let cf = match self.db.cf_handle(CF_STATS) {
            Some(cf) => cf,
            None => return crate::consensus::SlashingTracker::new(),
        };
        match self.db.get_cf(&cf, b"slashing_tracker") {
            Ok(Some(data)) => bincode::deserialize(&data).unwrap_or_else(|e| {
                eprintln!(
                    "⚠️  Failed to deserialize slashing tracker, starting fresh: {}",
                    e
                );
                crate::consensus::SlashingTracker::new()
            }),
            _ => crate::consensus::SlashingTracker::new(),
        }
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

    /// AUDIT-FIX B-1: Acquire treasury lock to serialize concurrent treasury
    /// read-modify-write operations during parallel fee charging.
    /// Returns a MutexGuard that must be held for the entire treasury RMW cycle.
    pub fn lock_treasury(&self) -> Result<std::sync::MutexGuard<'_, ()>, String> {
        self.treasury_lock
            .lock()
            .map_err(|e| format!("treasury_lock poisoned: {}", e))
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
        // AUDIT-FIX C-1: dirty_acct keys have format "dirty_acct:{pubkey}" (43 bytes total)
        // with NO slot component. We prune ALL dirty_acct keys since they are only
        // relevant for the state root computation of the current/recent block, which
        // has already been computed by the time pruning runs.
        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(b"dirty_acct:", Direction::Forward),
        );
        let mut dirty_deleted = 0u64;
        for item in iter.flatten() {
            if !item.0.starts_with(b"dirty_acct:") {
                break;
            }
            // Only prune if key length matches expected format (11 prefix + 32 pubkey)
            // to avoid accidentally deleting unrelated keys
            if item.0.len() == 43 {
                batch.delete_cf(&cf, &item.0);
                dirty_deleted += 1;
                deleted += 1;
            }
        }

        // Apply batch delete atomically
        if deleted > 0 {
            self.db
                .write(batch)
                .map_err(|e| format!("Failed to prune stats: {}", e))?;

            // AUDIT-FIX C-2: Only reset dirty counter if we actually pruned dirty
            // keys, and only to 0 (meaning "no outstanding dirty markers"). The
            // mark_account_dirty_with_key() function uses a non-zero marker (1)
            // so any concurrent writes will re-set it to 1 after this reset.
            // This is safe because the dirty flag is a simple "has any dirty"
            // indicator, not a count.
            if dirty_deleted > 0 {
                if let Some(cf_stats) = self.db.cf_handle(CF_STATS) {
                    let _ = self
                        .db
                        .put_cf(&cf_stats, b"dirty_account_count", 0u64.to_le_bytes());
                }
            }
        }

        Ok(deleted)
    }
}
// EVM address mapping methods
impl StateStore {
    /// Register EVM address mapping (EVM address → Native pubkey)
    /// Called on first transaction from an EVM address
    /// PHASE1-FIX S-8: Atomic WriteBatch for forward + reverse EVM address mapping.
    pub fn register_evm_address(
        &self,
        evm_address: &[u8; 20],
        native_pubkey: &Pubkey,
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_MAP)
            .ok_or_else(|| "EVM Map CF not found".to_string())?;

        let mut batch = rocksdb::WriteBatch::default();

        // Forward: 20-byte EVM address → 32-byte native pubkey
        batch.put_cf(&cf, evm_address, native_pubkey.0);

        // Reverse: native → EVM (M3 fix preserved)
        let mut reverse_key = Vec::with_capacity(52);
        reverse_key.extend_from_slice(b"reverse:");
        reverse_key.extend_from_slice(&native_pubkey.0);
        batch.put_cf(&cf, &reverse_key, evm_address);

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to register EVM address: {}", e))
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

    /// Reverse lookup: native pubkey → EVM address
    /// Uses the "reverse:" prefix key stored in CF_EVM_MAP
    pub fn lookup_native_to_evm(&self, native_pubkey: &Pubkey) -> Result<Option<[u8; 20]>, String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_MAP)
            .ok_or_else(|| "EVM Map CF not found".to_string())?;

        let mut reverse_key = Vec::with_capacity(40);
        reverse_key.extend_from_slice(b"reverse:");
        reverse_key.extend_from_slice(&native_pubkey.0);

        match self.db.get_cf(&cf, &reverse_key) {
            Ok(Some(data)) => {
                if data.len() != 20 {
                    return Err("Invalid EVM address data in reverse map".to_string());
                }
                let mut evm_bytes = [0u8; 20];
                evm_bytes.copy_from_slice(&data);
                Ok(Some(evm_bytes))
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
    /// PHASE1-FIX S-3: Use WriteBatch for atomic bulk delete instead of one-by-one.
    pub fn clear_evm_storage(&self, evm_address: &[u8; 20]) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_EVM_STORAGE)
            .ok_or_else(|| "EVM Storage CF not found".to_string())?;

        let prefix = evm_address;
        let keys: Vec<Box<[u8]>> = self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::From(prefix, Direction::Forward))
            .filter_map(|item| item.ok())
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, _)| k)
            .collect();

        if keys.is_empty() {
            return Ok(());
        }

        let mut batch = rocksdb::WriteBatch::default();
        for key in &keys {
            batch.delete_cf(&cf, key);
        }
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to batch-delete EVM storage: {}", e))
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

    /// Store a contract event. Key: program_pubkey + slot(BE) + name_hash(BE) + seq_counter
    /// (Matches batch writer key format for consistency)
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

        let name_hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut h = DefaultHasher::new();
            event.name.hash(&mut h);
            h.finish()
        };

        let mut key = Vec::with_capacity(32 + 8 + 8 + 8);
        key.extend_from_slice(&program.0);
        key.extend_from_slice(&event.slot.to_be_bytes());
        key.extend_from_slice(&name_hash.to_be_bytes());
        key.extend_from_slice(&seq.to_be_bytes());

        let data =
            serde_json::to_vec(event).map_err(|e| format!("Failed to serialize event: {}", e))?;

        // P10-CORE-05: Atomic WriteBatch for event data + slot secondary index
        let mut batch = WriteBatch::default();
        batch.put_cf(&cf, &key, &data);

        // Write slot secondary index: slot(8,BE) + program(32) + seq(8,BE) -> event_key
        // Enables O(prefix) lookup of events by slot instead of full CF scan
        if let Some(cf_slot) = self.db.cf_handle(CF_EVENTS_BY_SLOT) {
            let mut slot_key = Vec::with_capacity(8 + 32 + 8);
            slot_key.extend_from_slice(&event.slot.to_be_bytes());
            slot_key.extend_from_slice(&program.0);
            slot_key.extend_from_slice(&seq.to_be_bytes());
            batch.put_cf(&cf_slot, &slot_key, &key);
        }

        self.db
            .write(batch)
            .map_err(|e| format!("Failed to atomically store event + index: {}", e))?;
        Ok(())
    }

    /// Write contract storage key/value to CF_CONTRACT_STORAGE (non-batch).
    /// Key format: program(32) + storage_key_bytes → value_bytes
    pub fn put_contract_storage(
        &self,
        program: &Pubkey,
        storage_key: &[u8],
        value: &[u8],
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;
        let mut key = Vec::with_capacity(32 + storage_key.len());
        key.extend_from_slice(&program.0);
        key.extend_from_slice(storage_key);
        self.db
            .put_cf(&cf, &key, value)
            .map_err(|e| format!("Failed to store contract storage: {}", e))
    }

    /// Delete contract storage from CF_CONTRACT_STORAGE (non-batch).
    pub fn delete_contract_storage(
        &self,
        program: &Pubkey,
        storage_key: &[u8],
    ) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;
        let mut key = Vec::with_capacity(32 + storage_key.len());
        key.extend_from_slice(&program.0);
        key.extend_from_slice(storage_key);
        self.db
            .delete_cf(&cf, &key)
            .map_err(|e| format!("Failed to delete contract storage: {}", e))
    }

    /// O(1) point-read of a single contract storage key from CF_CONTRACT_STORAGE.
    /// Avoids deserializing the entire ContractAccount (which includes WASM bytecode).
    /// Key format: program(32) + storage_key → value.
    pub fn get_contract_storage(
        &self,
        program: &Pubkey,
        storage_key: &[u8],
    ) -> Result<Option<Vec<u8>>, String> {
        let cf = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;
        let mut key = Vec::with_capacity(32 + storage_key.len());
        key.extend_from_slice(&program.0);
        key.extend_from_slice(storage_key);
        self.db
            .get_cf(&cf, &key)
            .map(|opt| opt.map(|v| v.to_vec()))
            .map_err(|e| format!("Failed to read contract storage: {}", e))
    }

    /// O(1) point-read of a u64 from contract storage.
    pub fn get_contract_storage_u64(&self, program: &Pubkey, storage_key: &[u8]) -> u64 {
        match self.get_contract_storage(program, storage_key) {
            Ok(Some(data)) if data.len() >= 8 => {
                u64::from_le_bytes(data[..8].try_into().unwrap_or([0; 8]))
            }
            _ => 0,
        }
    }

    /// Resolve a symbol name → program Pubkey via the symbol registry, then
    /// read a single storage key from CF_CONTRACT_STORAGE. This is the fast
    /// path that avoids deserializing the ContractAccount (no WASM bytecode).
    pub fn get_program_storage(&self, symbol: &str, storage_key: &[u8]) -> Option<Vec<u8>> {
        let entry = self.get_symbol_registry(symbol).ok()??;
        self.get_contract_storage(&entry.program, storage_key)
            .ok()?
    }

    /// Resolve symbol → program Pubkey, then read a u64 storage value.
    pub fn get_program_storage_u64(&self, symbol: &str, storage_key: &[u8]) -> u64 {
        match self.get_symbol_registry(symbol) {
            Ok(Some(entry)) => self.get_contract_storage_u64(&entry.program, storage_key),
            _ => 0,
        }
    }

    /// Iterate contract storage entries from CF_CONTRACT_STORAGE using prefix scan.
    /// Key format: program(32) + storage_key(var) → value(var).
    /// Uses `after_key` cursor for pagination (entries strictly after the given key).
    pub fn get_contract_storage_entries(
        &self,
        program: &Pubkey,
        limit: usize,
        after_key: Option<Vec<u8>>,
    ) -> Result<KvEntries, String> {
        let cf = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;

        let prefix = program.0.to_vec();
        let start = if let Some(ak) = after_key {
            let mut k = prefix.clone();
            k.extend_from_slice(&ak);
            k.push(0); // position just past the after_key
            k
        } else {
            prefix.clone()
        };

        let iter = self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::From(&start, Direction::Forward));

        let mut results = Vec::new();
        for item in iter {
            let (k, v) = item.map_err(|e| format!("Iterator error: {}", e))?;
            if !k.starts_with(&prefix) {
                break;
            }
            let storage_key = k[32..].to_vec();
            results.push((storage_key, v.to_vec()));
            if results.len() >= limit {
                break;
            }
        }
        Ok(results)
    }

    /// Get events for a specific program, newest first, with limit
    pub fn get_events_by_program(
        &self,
        program: &Pubkey,
        limit: usize,
        before_slot: Option<u64>,
    ) -> Result<Vec<ContractEvent>, String> {
        let cf = self
            .db
            .cf_handle(CF_EVENTS)
            .ok_or_else(|| "Events CF not found".to_string())?;

        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&program.0);

        // Build seek key: use before_slot as upper bound, or 0xFF..FF to start from newest
        let mut end_key = prefix.clone();
        if let Some(bs) = before_slot {
            end_key.extend_from_slice(&bs.to_be_bytes());
        } else {
            end_key.extend_from_slice(&[0xFF; 16]); // past any valid slot+seq
        }

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&end_key, Direction::Reverse),
        );

        let mut events = Vec::new();
        for (key, value) in iter.flatten() {
            if !key.starts_with(&prefix) {
                break;
            }
            // When paginating, skip entries at or after before_slot (cursor is exclusive)
            if let Some(bs) = before_slot {
                if key.len() >= 40 {
                    let slot_bytes: [u8; 8] = key[32..40].try_into().unwrap_or([0xFF; 8]);
                    let slot = u64::from_be_bytes(slot_bytes);
                    if slot >= bs {
                        continue;
                    }
                }
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
    /// AUDIT-FIX H6: Protected by event_seq_lock to prevent duplicate sequence
    /// numbers when called concurrently (e.g., parallel contract execution).
    fn next_event_seq(&self, program: &Pubkey, slot: u64) -> Result<u64, String> {
        let _guard = self
            .event_seq_lock
            .lock()
            .map_err(|e| format!("Event seq lock poisoned: {}", e))?;

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

        // P10-CORE-04: Atomic WriteBatch for forward + reverse indexes
        let mut batch = WriteBatch::default();
        if balance == 0 {
            // Remove zero-balance entries to keep index clean
            batch.delete_cf(&cf, &key);
            batch.delete_cf(&rev_cf, &rev_key);
        } else {
            batch.put_cf(&cf, &key, balance.to_le_bytes());
            batch.put_cf(&rev_cf, &rev_key, balance.to_le_bytes());
        }
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to atomically update token balance indexes: {}", e))?;
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
        after_holder: Option<&Pubkey>,
    ) -> Result<Vec<(Pubkey, u64)>, String> {
        let cf = self
            .db
            .cf_handle(CF_TOKEN_BALANCES)
            .ok_or_else(|| "Token balances CF not found".to_string())?;

        let prefix = token_program.0.to_vec();

        // Build start key: if after_holder is provided, start just past it
        let start_key = if let Some(ah) = after_holder {
            let mut k = prefix.clone();
            k.extend_from_slice(&ah.0);
            // Add a zero byte to position just past this key
            k.push(0);
            k
        } else {
            prefix.clone()
        };

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&start_key, Direction::Forward),
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
        before_slot: Option<u64>,
    ) -> Result<Vec<TokenTransfer>, String> {
        let cf = self
            .db
            .cf_handle(CF_TOKEN_TRANSFERS)
            .ok_or_else(|| "Token transfers CF not found".to_string())?;

        let mut prefix = Vec::with_capacity(32);
        prefix.extend_from_slice(&token_program.0);

        // Build seek key: use before_slot as upper bound, or 0xFF..FF to start from newest
        let mut end_key = prefix.clone();
        if let Some(bs) = before_slot {
            end_key.extend_from_slice(&bs.to_be_bytes());
        } else {
            end_key.extend_from_slice(&[0xFF; 16]);
        }

        let iter = self.db.iterator_cf(
            &cf,
            rocksdb::IteratorMode::From(&end_key, Direction::Reverse),
        );

        let mut transfers = Vec::new();
        for (key, value) in iter.flatten() {
            if !key.starts_with(&prefix) {
                break;
            }
            // When paginating, skip entries at or after before_slot (cursor is exclusive)
            if let Some(bs) = before_slot {
                if key.len() >= 40 {
                    let slot_bytes: [u8; 8] = key[32..40].try_into().unwrap_or([0xFF; 8]);
                    let slot = u64::from_be_bytes(slot_bytes);
                    if slot >= bs {
                        continue;
                    }
                }
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
    /// AUDIT-FIX CP-8: Protected by Mutex to prevent read-modify-write race conditions
    fn next_transfer_seq(&self, token_program: &Pubkey, slot: u64) -> Result<u64, String> {
        let _lock = self
            .transfer_seq_lock
            .lock()
            .unwrap_or_else(|e| e.into_inner());

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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
    pub fn index_tx_to_slot(&self, sig: &Hash, slot: u64) -> Result<(), String> {
        let cf = self
            .db
            .cf_handle(CF_TX_TO_SLOT)
            .ok_or_else(|| "TX to slot CF not found".to_string())?;

        self.db
            .put_cf(&cf, sig.0, slot.to_le_bytes())
            .map_err(|e| format!("Failed to index tx to slot: {}", e))
    }

    /// PHASE1-FIX S-2: Protected by tx_slot_seq_lock to prevent duplicate
    /// sequence numbers under concurrent access (mirrors event_seq_lock pattern).
    fn next_tx_slot_seq(&self, slot: u64) -> Result<u64, String> {
        let _guard = self
            .tx_slot_seq_lock
            .lock()
            .map_err(|e| format!("TX slot seq lock poisoned: {}", e))?;

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

    pub fn get_all_programs_paginated(
        &self,
        limit: usize,
        after: Option<&Pubkey>,
    ) -> Result<Vec<(Pubkey, Value)>, String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAMS)
            .ok_or_else(|| "Programs CF not found".to_string())?;

        let iter = if let Some(after_pk) = after {
            self.db.iterator_cf(
                &cf,
                rocksdb::IteratorMode::From(&after_pk.0, rocksdb::Direction::Forward),
            )
        } else {
            self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start)
        };

        let mut programs = Vec::new();
        for (key, value) in iter.flatten() {
            if key.len() != 32 {
                continue;
            }
            if let Some(after_pk) = after {
                if key.as_ref() == &after_pk.0[..] {
                    continue;
                }
            }

            let mut pk_bytes = [0u8; 32];
            pk_bytes.copy_from_slice(&key);
            let pk = Pubkey(pk_bytes);
            let metadata: Value = serde_json::from_slice(&value).unwrap_or(Value::Null);
            programs.push((pk, metadata));
            if programs.len() >= limit {
                break;
            }
        }

        Ok(programs)
    }

    /// Get contract logs (events) for a specific program
    pub fn get_contract_logs(
        &self,
        program: &Pubkey,
        limit: usize,
        before_slot: Option<u64>,
    ) -> Result<Vec<ContractEvent>, String> {
        self.get_events_by_program(program, limit, before_slot)
    }

    /// Reconcile active account count with actual database
    #[allow(dead_code)]
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

// ════════════════════════════════════════════════════════════════════════════
// State Snapshot / Checkpoint System
// ════════════════════════════════════════════════════════════════════════════

/// Metadata stored alongside each checkpoint (serialized as JSON in the
/// checkpoint directory).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointMeta {
    /// Finalized slot at which the checkpoint was taken
    pub slot: u64,
    /// State root hash at the checkpoint slot
    pub state_root: [u8; 32],
    /// Timestamp (unix seconds) when the checkpoint was created
    pub created_at: u64,
    /// Total accounts at checkpoint time
    pub total_accounts: u64,
}

impl StateStore {
    // ── P2-3: Cold/archival storage ──────────────────────────────────

    /// Open (or create) the cold archival DB at `cold_path` and attach it to
    /// this store. Once attached, `get_block` and `get_transaction` will
    /// fall through to the cold DB if the key is missing from hot storage.
    pub fn open_cold_store<P: AsRef<Path>>(&mut self, cold_path: P) -> Result<(), String> {
        let mut db_opts = Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);
        db_opts.set_max_open_files(256);
        db_opts.set_keep_log_file_num(3);
        db_opts.set_max_total_wal_size(64 * 1024 * 1024);
        db_opts.increase_parallelism(2);
        db_opts.set_max_background_jobs(2);

        // Archival tuning: Zstd compression, large block sizes
        let archival_cf_opts = || {
            let mut opts = Options::default();
            opts.set_compression_type(rocksdb::DBCompressionType::Zstd);
            let mut bbo = BlockBasedOptions::default();
            bbo.set_bloom_filter(10.0, false);
            bbo.set_block_size(32 * 1024); // 32KB blocks for better compression
            opts.set_block_based_table_factory(&bbo);
            opts.set_write_buffer_size(32 * 1024 * 1024);
            opts
        };

        let cf_descs = vec![
            ColumnFamilyDescriptor::new(COLD_CF_BLOCKS, archival_cf_opts()),
            ColumnFamilyDescriptor::new(COLD_CF_TRANSACTIONS, archival_cf_opts()),
            ColumnFamilyDescriptor::new(COLD_CF_TX_TO_SLOT, archival_cf_opts()),
        ];

        let cold = DB::open_cf_descriptors(&db_opts, cold_path.as_ref(), cf_descs)
            .map_err(|e| format!("Failed to open cold DB: {}", e))?;

        self.cold_db = Some(Arc::new(cold));
        tracing::info!(
            "🗄️  Cold storage opened at {}",
            cold_path.as_ref().display()
        );
        Ok(())
    }

    /// Migrate old blocks and transactions from the hot DB to the cold DB.
    ///
    /// Moves all blocks with slot < `cutoff_slot` and their associated
    /// transactions. Data is written to cold first, then deleted from hot
    /// in a single atomic batch to avoid data loss.
    ///
    /// Returns the number of blocks migrated.
    pub fn migrate_to_cold(&self, cutoff_slot: u64) -> Result<u64, String> {
        let cold = self
            .cold_db
            .as_ref()
            .ok_or_else(|| "Cold storage not attached".to_string())?;

        let hot_blocks_cf = self
            .db
            .cf_handle(CF_BLOCKS)
            .ok_or_else(|| "Blocks CF not found".to_string())?;
        let hot_slots_cf = self
            .db
            .cf_handle(CF_SLOTS)
            .ok_or_else(|| "Slots CF not found".to_string())?;
        let hot_txs_cf = self
            .db
            .cf_handle(CF_TRANSACTIONS)
            .ok_or_else(|| "Transactions CF not found".to_string())?;
        let hot_tx_to_slot_cf = self
            .db
            .cf_handle(CF_TX_TO_SLOT)
            .ok_or_else(|| "tx_to_slot CF not found".to_string())?;

        let cold_blocks_cf = cold
            .cf_handle(COLD_CF_BLOCKS)
            .ok_or_else(|| "Cold blocks CF not found".to_string())?;
        let cold_txs_cf = cold
            .cf_handle(COLD_CF_TRANSACTIONS)
            .ok_or_else(|| "Cold transactions CF not found".to_string())?;
        let cold_tx_to_slot_cf = cold
            .cf_handle(COLD_CF_TX_TO_SLOT)
            .ok_or_else(|| "Cold tx_to_slot CF not found".to_string())?;

        let mut migrated: u64 = 0;
        let mut hot_delete_batch = WriteBatch::default();

        // Scan slots 0..cutoff_slot in the hot DB
        let iter = self.db.iterator_cf(
            &hot_slots_cf,
            rocksdb::IteratorMode::From(&0u64.to_le_bytes(), Direction::Forward),
        );

        for item in iter.flatten() {
            // Slot keys are 8-byte LE u64 except "last_slot" which is a string key
            if item.0.len() != 8 {
                continue;
            }
            let slot = u64::from_le_bytes(item.0[..8].try_into().unwrap());
            if slot >= cutoff_slot {
                break;
            }

            // item.1 is the block hash (32 bytes)
            if item.1.len() != 32 {
                continue;
            }
            let block_hash: [u8; 32] = item.1[..32].try_into().unwrap();

            // Read the block from hot
            if let Ok(Some(block_data)) = self.db.get_cf(&hot_blocks_cf, block_hash) {
                // Write to cold
                cold.put_cf(&cold_blocks_cf, block_hash, &block_data)
                    .map_err(|e| format!("Cold write error (block): {}", e))?;

                // Deserialize block to get transaction signatures
                let block: Option<Block> = if block_data.first() == Some(&0xBC) {
                    bincode::deserialize(&block_data[1..]).ok()
                } else {
                    serde_json::from_slice(&block_data).ok()
                };

                if let Some(block) = block {
                    for tx in &block.transactions {
                        let sig = tx.signature();
                        // Migrate transaction data
                        if let Ok(Some(tx_data)) = self.db.get_cf(&hot_txs_cf, sig.0) {
                            cold.put_cf(&cold_txs_cf, sig.0, &tx_data)
                                .map_err(|e| format!("Cold write error (tx): {}", e))?;
                            hot_delete_batch.delete_cf(&hot_txs_cf, sig.0);
                        }
                        // Migrate tx_to_slot mapping
                        if let Ok(Some(slot_data)) = self.db.get_cf(&hot_tx_to_slot_cf, sig.0) {
                            cold.put_cf(&cold_tx_to_slot_cf, sig.0, &slot_data)
                                .map_err(|e| format!("Cold write error (tx_to_slot): {}", e))?;
                            hot_delete_batch.delete_cf(&hot_tx_to_slot_cf, sig.0);
                        }
                    }
                }

                // Delete block from hot
                hot_delete_batch.delete_cf(&hot_blocks_cf, block_hash);
                migrated += 1;
            }

            // Note: we do NOT delete the slot→hash mapping from hot_slots_cf,
            // so `get_block_by_slot` can still resolve the hash and then fall
            // through to cold storage via `get_block`.
        }

        // Atomically remove migrated data from hot DB
        if migrated > 0 {
            self.db
                .write(hot_delete_batch)
                .map_err(|e| format!("Failed to delete migrated data from hot DB: {}", e))?;
            tracing::info!(
                "🗄️  Migrated {} blocks (slots < {}) to cold storage",
                migrated,
                cutoff_slot
            );
        }

        Ok(migrated)
    }

    /// Returns true if a cold DB is attached.
    pub fn has_cold_storage(&self) -> bool {
        self.cold_db.is_some()
    }
}

impl StateStore {
    // ── Checkpoint creation (RocksDB native hardlink snapshot) ────────────

    /// Create a point-in-time checkpoint of the entire database.
    ///
    /// Uses RocksDB's native `Checkpoint` API which creates hardlinks to SST
    /// files — effectively O(1) in time and zero additional disk space until
    /// compaction replaces the SST files.
    ///
    /// `checkpoint_dir` is the directory where the checkpoint will be stored,
    /// e.g. `data/state-8000/checkpoints/slot-10000`.
    ///
    /// Returns the `CheckpointMeta` for the created checkpoint.
    pub fn create_checkpoint(
        &self,
        checkpoint_dir: &str,
        slot: u64,
    ) -> Result<CheckpointMeta, String> {
        use rocksdb::checkpoint::Checkpoint;

        // Ensure parent directory exists
        let parent = std::path::Path::new(checkpoint_dir)
            .parent()
            .ok_or_else(|| "Invalid checkpoint path".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create checkpoint parent dir: {}", e))?;

        // Remove stale checkpoint at the same path if it exists
        if std::path::Path::new(checkpoint_dir).exists() {
            std::fs::remove_dir_all(checkpoint_dir)
                .map_err(|e| format!("Failed to remove old checkpoint: {}", e))?;
        }

        // P10-CORE-02 FIX: Compute state root and account count BEFORE taking
        // the snapshot so the metadata matches the checkpoint contents exactly.
        // Previously these were computed after the snapshot, allowing concurrent
        // writes to make the recorded state_root diverge from the snapshot data.
        let state_root = self.compute_state_root();
        let total_accounts = self.count_accounts().unwrap_or(0);

        // Create RocksDB checkpoint (hardlink-based, near-instant)
        let cp = Checkpoint::new(&self.db)
            .map_err(|e| format!("Failed to create checkpoint object: {}", e))?;
        cp.create_checkpoint(checkpoint_dir)
            .map_err(|e| format!("Failed to create checkpoint: {}", e))?;
        let meta = CheckpointMeta {
            slot,
            state_root: state_root.0,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            total_accounts,
        };

        // Write metadata file inside the checkpoint directory
        let meta_path = std::path::Path::new(checkpoint_dir).join("checkpoint_meta.json");
        let meta_json = serde_json::to_string_pretty(&meta)
            .map_err(|e| format!("Failed to serialize checkpoint meta: {}", e))?;
        std::fs::write(&meta_path, meta_json)
            .map_err(|e| format!("Failed to write checkpoint meta: {}", e))?;

        Ok(meta)
    }

    /// Open a checkpoint as a read-only StateStore for serving snapshot data.
    pub fn open_checkpoint(checkpoint_dir: &str) -> Result<Self, String> {
        Self::open(checkpoint_dir)
    }

    /// List available checkpoints in the data directory.
    /// Returns sorted (oldest first) list of `(slot, checkpoint_dir_path)`.
    pub fn list_checkpoints(data_dir: &str) -> Vec<(u64, String)> {
        let cp_root = std::path::Path::new(data_dir).join("checkpoints");
        let mut result = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&cp_root) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let meta_path = path.join("checkpoint_meta.json");
                    if meta_path.exists() {
                        if let Ok(data) = std::fs::read_to_string(&meta_path) {
                            if let Ok(meta) = serde_json::from_str::<CheckpointMeta>(&data) {
                                result.push((meta.slot, path.to_string_lossy().to_string()));
                            }
                        }
                    }
                }
            }
        }
        result.sort_by_key(|(slot, _)| *slot);
        result
    }

    /// Get the latest checkpoint metadata from the data directory.
    pub fn latest_checkpoint(data_dir: &str) -> Option<(CheckpointMeta, String)> {
        let checkpoints = Self::list_checkpoints(data_dir);
        checkpoints.last().and_then(|(_, path)| {
            let meta_path = std::path::Path::new(path).join("checkpoint_meta.json");
            let data = std::fs::read_to_string(&meta_path).ok()?;
            let meta: CheckpointMeta = serde_json::from_str(&data).ok()?;
            Some((meta, path.clone()))
        })
    }

    /// Prune old checkpoints, keeping only the most recent `keep_count`.
    pub fn prune_checkpoints(data_dir: &str, keep_count: usize) -> Result<usize, String> {
        let checkpoints = Self::list_checkpoints(data_dir);
        if checkpoints.len() <= keep_count {
            return Ok(0);
        }
        let to_remove = checkpoints.len() - keep_count;
        let mut removed = 0;
        for (_, path) in checkpoints.iter().take(to_remove) {
            if std::fs::remove_dir_all(path).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    // ── Snapshot export / import (for P2P state transfer) ────────────────

    /// Export a page of accounts as (pubkey_bytes, account_bytes).
    ///
    /// P10-CORE-03 FIX: Uses RocksDB iterator with skip/take so only the
    /// requested page is materialised in memory, avoiding OOM on large state.
    pub fn export_accounts_iter(&self, offset: u64, limit: u64) -> Result<KvPage, String> {
        self.export_cf_page(CF_ACCOUNTS, "Accounts", offset, limit)
    }

    /// Export a cursor-paginated page of accounts.
    pub fn export_accounts_cursor(
        &self,
        after_key: Option<&[u8]>,
        limit: u64,
    ) -> Result<KvPage, String> {
        self.export_cf_page_cursor(
            CF_ACCOUNTS,
            "Accounts",
            after_key,
            limit,
            Some(self.metrics.get_total_accounts()),
        )
    }

    /// Export a page of contract storage entries as (key_bytes, value_bytes).
    pub fn export_contract_storage_iter(&self, offset: u64, limit: u64) -> Result<KvPage, String> {
        self.export_cf_page(CF_CONTRACT_STORAGE, "Contract storage", offset, limit)
    }

    /// Export a cursor-paginated page of contract storage entries.
    pub fn export_contract_storage_cursor(
        &self,
        after_key: Option<&[u8]>,
        limit: u64,
    ) -> Result<KvPage, String> {
        self.export_cf_page_cursor(
            CF_CONTRACT_STORAGE,
            "Contract storage",
            after_key,
            limit,
            None,
        )
    }

    /// Count total number of contract storage entries.
    pub fn count_contract_storage_entries(&self) -> Result<u64, String> {
        let cf = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;
        let mut count = 0u64;
        for _ in self
            .db
            .iterator_cf(&cf, rocksdb::IteratorMode::Start)
            .flatten()
        {
            count = count.saturating_add(1);
        }
        Ok(count)
    }

    /// Export a page of programs (WASM bytecode) as (pubkey_bytes, program_bytes).
    pub fn export_programs_iter(&self, offset: u64, limit: u64) -> Result<KvPage, String> {
        self.export_cf_page(CF_PROGRAMS, "Programs", offset, limit)
    }

    /// Export a cursor-paginated page of programs.
    pub fn export_programs_cursor(
        &self,
        after_key: Option<&[u8]>,
        limit: u64,
    ) -> Result<KvPage, String> {
        self.export_cf_page_cursor(
            CF_PROGRAMS,
            "Programs",
            after_key,
            limit,
            Some(self.get_program_count()),
        )
    }

    /// Generic helper: read a page of (key, value) pairs from a column family.
    fn export_cf_page(
        &self,
        cf_name: &str,
        display_name: &str,
        offset: u64,
        limit: u64,
    ) -> Result<KvPage, String> {
        if limit == 0 {
            return Ok(KvPage {
                entries: Vec::new(),
                total: 0,
                next_cursor: None,
                has_more: false,
            });
        }

        let pages_to_advance = offset / limit;
        let intra_page_skip = (offset % limit) as usize;
        let mut cursor: Option<Vec<u8>> = None;
        let mut advanced = 0u64;

        while advanced < pages_to_advance {
            let page =
                self.export_cf_page_cursor(cf_name, display_name, cursor.as_deref(), limit, None)?;

            if !page.has_more && page.entries.is_empty() {
                return Ok(KvPage {
                    entries: Vec::new(),
                    total: page.total,
                    next_cursor: None,
                    has_more: false,
                });
            }

            cursor = page.next_cursor;
            advanced = advanced.saturating_add(1);

            if !page.has_more {
                break;
            }
        }

        let mut page = self.export_cf_page_cursor(
            cf_name,
            display_name,
            cursor.as_deref(),
            limit.saturating_add(intra_page_skip as u64),
            None,
        )?;

        if intra_page_skip > 0 {
            if intra_page_skip >= page.entries.len() {
                page.entries.clear();
                page.has_more = false;
                page.next_cursor = None;
            } else {
                page.entries.drain(0..intra_page_skip);
                if page.entries.len() > limit as usize {
                    page.entries.truncate(limit as usize);
                    page.has_more = true;
                    page.next_cursor = page.entries.last().map(|(key, _)| key.clone());
                }
            }
        }

        if page.entries.len() > limit as usize {
            page.entries.truncate(limit as usize);
            page.has_more = true;
            page.next_cursor = page.entries.last().map(|(key, _)| key.clone());
        }

        Ok(page)
    }

    fn export_cf_page_cursor(
        &self,
        cf_name: &str,
        display_name: &str,
        after_key: Option<&[u8]>,
        limit: u64,
        total_hint: Option<u64>,
    ) -> Result<KvPage, String> {
        let cf = self
            .db
            .cf_handle(cf_name)
            .ok_or_else(|| format!("{} CF not found", display_name))?;

        let total = match total_hint {
            Some(value) => value,
            None => {
                let mut count = 0u64;
                for _ in self
                    .db
                    .iterator_cf(&cf, rocksdb::IteratorMode::Start)
                    .flatten()
                {
                    count = count.saturating_add(1);
                }
                count
            }
        };

        let iter = if let Some(after) = after_key {
            self.db.iterator_cf(
                &cf,
                rocksdb::IteratorMode::From(after, rocksdb::Direction::Forward),
            )
        } else {
            self.db.iterator_cf(&cf, rocksdb::IteratorMode::Start)
        };

        let mut entries = Vec::with_capacity(limit.min(10_000) as usize);
        let mut has_more = false;

        for (key, value) in iter.flatten() {
            if let Some(after) = after_key {
                if key.as_ref() == after {
                    continue;
                }
            }

            entries.push((key.to_vec(), value.to_vec()));
            if entries.len() > limit as usize {
                has_more = true;
                entries.pop();
                break;
            }
        }

        let next_cursor = if has_more {
            entries.last().map(|(key, _)| key.clone())
        } else {
            None
        };

        Ok(KvPage {
            entries,
            total,
            next_cursor,
            has_more,
        })
    }

    /// Import a batch of accounts into the store (used by joining validators).
    /// Returns the number of accounts imported.
    pub fn import_accounts(&self, entries: &[(Vec<u8>, Vec<u8>)]) -> Result<usize, String> {
        let cf = self
            .db
            .cf_handle(CF_ACCOUNTS)
            .ok_or_else(|| "Accounts CF not found".to_string())?;

        let mut batch = WriteBatch::default();
        for (key, value) in entries {
            batch.put_cf(&cf, key, value);
        }
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to import accounts: {}", e))?;

        Ok(entries.len())
    }

    /// Import a batch of contract storage entries.
    pub fn import_contract_storage(&self, entries: &[(Vec<u8>, Vec<u8>)]) -> Result<usize, String> {
        let cf = self
            .db
            .cf_handle(CF_CONTRACT_STORAGE)
            .ok_or_else(|| "Contract storage CF not found".to_string())?;

        let mut batch = WriteBatch::default();
        for (key, value) in entries {
            batch.put_cf(&cf, key, value);
        }
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to import contract storage: {}", e))?;

        Ok(entries.len())
    }

    /// Import a batch of programs (WASM bytecode).
    pub fn import_programs(&self, entries: &[(Vec<u8>, Vec<u8>)]) -> Result<usize, String> {
        let cf = self
            .db
            .cf_handle(CF_PROGRAMS)
            .ok_or_else(|| "Programs CF not found".to_string())?;

        let mut batch = WriteBatch::default();
        for (key, value) in entries {
            batch.put_cf(&cf, key, value);
        }
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to import programs: {}", e))?;

        Ok(entries.len())
    }

    /// Get a reference to the underlying DB Arc for direct access when needed.
    pub fn db_ref(&self) -> &Arc<DB> {
        &self.db
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
            fee_producer_percent: 30,
            fee_voters_percent: 10,
            fee_treasury_percent: 10,
            fee_community_percent: 10,
        };

        state.set_fee_config_full(&config).unwrap();

        let loaded = state.get_fee_config().unwrap();
        assert_eq!(loaded.base_fee, 5_000);
        assert_eq!(loaded.fee_burn_percent, 40);
        assert_eq!(loaded.fee_producer_percent, 30);
        assert_eq!(loaded.fee_voters_percent, 10);
        assert_eq!(loaded.fee_treasury_percent, 10);
        assert_eq!(loaded.fee_community_percent, 10);
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

    /// AUDIT-FIX C-1: prune_slot_stats correctly handles dirty_acct keys
    /// whose format is "dirty_acct:{pubkey}" (43 bytes, no slot).
    /// Pruning must not corrupt state by misinterpreting pubkey bytes as slots.
    #[test]
    fn test_prune_dirty_acct_correct_format() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // Write some dirty_acct markers
        let pk1 = Pubkey([0xAA; 32]);
        let pk2 = Pubkey([0xBB; 32]);
        state.mark_account_dirty_with_key(&pk1);
        state.mark_account_dirty_with_key(&pk2);

        // Verify they exist
        let cf = state.db.cf_handle(CF_STATS).unwrap();
        let mut key1 = [0u8; 43];
        key1[..11].copy_from_slice(b"dirty_acct:");
        key1[11..43].copy_from_slice(&pk1.0);
        assert!(state.db.get_cf(&cf, key1).unwrap().is_some());

        // Prune with a high current_slot (should clean all dirty markers)
        let deleted = state.prune_slot_stats(10000, 100).unwrap();
        assert!(
            deleted >= 2,
            "Should have pruned at least 2 dirty_acct keys, got {}",
            deleted
        );

        // Dirty markers should be gone
        assert!(state.db.get_cf(&cf, key1).unwrap().is_none());
    }

    /// AUDIT-FIX C-2: dirty_account_count is only reset when dirty keys
    /// were actually pruned, and new writes after pruning re-set the flag.
    #[test]
    fn test_prune_dirty_count_not_unconditional_reset() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // Create a fee_dist entry so pruning has something to delete even
        // without dirty_acct keys
        let cf = state.db.cf_handle(CF_STATS).unwrap();
        let _ = state.db.put_cf(&cf, b"fee_dist:1", b"data");

        // Set dirty_account_count to 1 (simulating a concurrent write)
        let _ = state
            .db
            .put_cf(&cf, b"dirty_account_count", 1u64.to_le_bytes());

        // Prune — should delete fee_dist:1 but NOT reset dirty_account_count
        // because no dirty_acct keys were pruned
        let _ = state.prune_slot_stats(10000, 100).unwrap();

        // dirty_account_count should still be 1 (not reset to 0)
        let val = state
            .db
            .get_cf(&cf, b"dirty_account_count")
            .unwrap()
            .map(|v| u64::from_le_bytes(v.try_into().unwrap_or([0; 8])))
            .unwrap_or(0);
        assert_eq!(
            val, 1,
            "dirty_account_count must not be reset when no dirty_acct keys were pruned"
        );
    }

    /// AUDIT-FIX C-3: commit_batch holds burned_lock during RMW to prevent
    /// concurrent add_burned() from losing updates.
    #[test]
    fn test_commit_batch_burned_lock_serializes() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // Direct add_burned to set baseline
        state.add_burned(100).unwrap();
        assert_eq!(state.get_total_burned().unwrap(), 100);

        // Now commit a batch with burned_delta = 50
        let mut batch = state.begin_batch();
        batch.add_burned(50);
        state.commit_batch(batch).unwrap();

        // Total should be 150, not 50 (which would happen if lock was missing
        // and the batch read a stale value overwriting the direct add)
        assert_eq!(state.get_total_burned().unwrap(), 150);

        // And another direct add should also serialize
        state.add_burned(25).unwrap();
        assert_eq!(state.get_total_burned().unwrap(), 175);
    }

    /// AUDIT-FIX C-4: atomic_put_accounts holds burned_lock during RMW to
    /// prevent lost updates to the burned counter.
    #[test]
    fn test_atomic_put_accounts_burned_lock_serializes() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // Set baseline
        state.add_burned(200).unwrap();

        // Put accounts with a burn_delta
        let pk = Pubkey([0xCC; 32]);
        let acct = Account::new(10, pk); // 10 MOLT
        state.atomic_put_accounts(&[(&pk, &acct)], 80).unwrap();

        // Total burned should be 280, not 80
        assert_eq!(state.get_total_burned().unwrap(), 280);

        // Verify account was also written
        let loaded = state.get_account(&pk).unwrap().unwrap();
        assert_eq!(loaded.shells, 10_000_000_000);
    }

    // ═══════════════════════════════════════════════════════════════════════
    // TOKENOMICS OVERHAUL: All 6 wallet pubkey accessors
    // ═══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_all_wallet_pubkeys_stored_and_retrievable() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        // Simulate genesis: store all 6 wallet entries
        let wallets: Vec<(String, Pubkey, u64, u8)> = vec![
            (
                "validator_rewards".into(),
                Pubkey([0x01; 32]),
                100_000_000,
                10,
            ),
            (
                "community_treasury".into(),
                Pubkey([0x02; 32]),
                250_000_000,
                25,
            ),
            ("builder_grants".into(), Pubkey([0x03; 32]), 350_000_000, 35),
            (
                "founding_moltys".into(),
                Pubkey([0x04; 32]),
                100_000_000,
                10,
            ),
            (
                "ecosystem_partnerships".into(),
                Pubkey([0x05; 32]),
                100_000_000,
                10,
            ),
            ("reserve_pool".into(), Pubkey([0x06; 32]), 100_000_000, 10),
        ];
        state.set_genesis_accounts(&wallets).unwrap();

        // Also set treasury_pubkey (legacy path)
        state.set_treasury_pubkey(&Pubkey([0x01; 32])).unwrap();

        // Verify treasury (legacy path)
        let treasury = state.get_treasury_pubkey().unwrap();
        assert_eq!(treasury, Some(Pubkey([0x01; 32])));

        // Verify all 6 wallet role-based accessors
        assert_eq!(
            state.get_wallet_pubkey("validator_rewards").unwrap(),
            Some(Pubkey([0x01; 32]))
        );
        assert_eq!(
            state.get_community_treasury_pubkey().unwrap(),
            Some(Pubkey([0x02; 32]))
        );
        assert_eq!(
            state.get_builder_grants_pubkey().unwrap(),
            Some(Pubkey([0x03; 32]))
        );
        assert_eq!(
            state.get_founding_moltys_pubkey().unwrap(),
            Some(Pubkey([0x04; 32]))
        );
        assert_eq!(
            state.get_ecosystem_partnerships_pubkey().unwrap(),
            Some(Pubkey([0x05; 32]))
        );
        assert_eq!(
            state.get_reserve_pool_pubkey().unwrap(),
            Some(Pubkey([0x06; 32]))
        );

        // Unknown role returns None
        assert_eq!(state.get_wallet_pubkey("nonexistent").unwrap(), None);

        // Verify count and ordering via get_genesis_accounts
        let loaded = state.get_genesis_accounts().unwrap();
        assert_eq!(loaded.len(), 6);
        let total: u64 = loaded.iter().map(|(_, _, amt, _)| amt).sum();
        assert_eq!(total, 1_000_000_000, "All 6 wallets must sum to 1B MOLT");
    }

    #[test]
    fn test_dao_treasury_wired_to_community_treasury() {
        // Verify that community_treasury pubkey is fetchable and distinct,
        // confirming it can be used as the DAO treasury address at genesis.
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let community_pk = Pubkey([0xCC; 32]);
        let validator_rewards_pk = Pubkey([0xAA; 32]);

        // Store genesis accounts with community_treasury (4th element is percentage u8)
        let accounts: Vec<(String, Pubkey, u64, u8)> = vec![
            (
                "validator_rewards".to_string(),
                validator_rewards_pk,
                100_000_000,
                10,
            ),
            (
                "community_treasury".to_string(),
                community_pk,
                250_000_000,
                25,
            ),
        ];
        state.set_genesis_accounts(&accounts).unwrap();
        state.set_treasury_pubkey(&validator_rewards_pk).unwrap();

        // DAO should use community_treasury, NOT validator_rewards
        let dao_treasury = state
            .get_community_treasury_pubkey()
            .unwrap()
            .expect("community_treasury must be set");
        assert_eq!(
            dao_treasury, community_pk,
            "DAO treasury must be community_treasury wallet"
        );
        assert_ne!(
            dao_treasury, validator_rewards_pk,
            "DAO treasury must NOT be validator_rewards"
        );
    }

    // ─── Shielded pool state tests ──────────────────────────────────

    #[test]
    fn test_shielded_commitment_insert_and_get() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let commitment = [0xABu8; 32];
        state.insert_shielded_commitment(0, &commitment).unwrap();

        let retrieved = state.get_shielded_commitment(0).unwrap();
        assert_eq!(retrieved, Some(commitment));

        // Non-existent index
        assert_eq!(state.get_shielded_commitment(1).unwrap(), None);
    }

    #[test]
    fn test_shielded_commitment_multiple() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        for i in 0u64..5 {
            let mut c = [0u8; 32];
            c[0] = i as u8;
            state.insert_shielded_commitment(i, &c).unwrap();
        }

        let all = state.get_all_shielded_commitments(5).unwrap();
        assert_eq!(all.len(), 5);
        for (i, entry) in all.iter().enumerate() {
            assert_eq!(entry[0], i as u8);
        }
    }

    #[test]
    fn test_nullifier_spent_tracking() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let nullifier = [0xFFu8; 32];

        assert!(!state.is_nullifier_spent(&nullifier).unwrap());
        state.mark_nullifier_spent(&nullifier).unwrap();
        assert!(state.is_nullifier_spent(&nullifier).unwrap());

        // Different nullifier is not spent
        let other = [0x01u8; 32];
        assert!(!state.is_nullifier_spent(&other).unwrap());
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shielded_pool_state_default() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let pool = state.get_shielded_pool_state().unwrap();
        assert_eq!(pool.commitment_count, 0);
        assert_eq!(pool.total_shielded, 0);
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shielded_pool_state_roundtrip() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let mut pool = crate::zk::ShieldedPoolState::new();
        pool.commitment_count = 42;
        pool.total_shielded = 1_000_000;
        pool.merkle_root = [0xEE; 32];

        state.put_shielded_pool_state(&pool).unwrap();
        let loaded = state.get_shielded_pool_state().unwrap();

        assert_eq!(loaded.commitment_count, 42);
        assert_eq!(loaded.total_shielded, 1_000_000);
        assert_eq!(loaded.merkle_root, [0xEE; 32]);
    }

    #[test]
    fn test_shielded_batch_commitment_and_nullifier() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let mut batch = state.begin_batch();

        // Insert commitment via batch
        let commitment = [0xBBu8; 32];
        batch.insert_shielded_commitment(0, &commitment).unwrap();

        // Mark nullifier via batch
        let nullifier = [0xCCu8; 32];
        batch.mark_nullifier_spent(&nullifier).unwrap();

        // Batch view must see in-flight nullifier spend immediately
        assert!(batch.is_nullifier_spent(&nullifier).unwrap());

        // Before commit, disk has nothing
        assert_eq!(state.get_shielded_commitment(0).unwrap(), None);
        assert!(!state.is_nullifier_spent(&nullifier).unwrap());

        // Commit the batch
        state.commit_batch(batch).unwrap();

        // Now disk has the data
        assert_eq!(state.get_shielded_commitment(0).unwrap(), Some(commitment));
        assert!(state.is_nullifier_spent(&nullifier).unwrap());
    }

    #[cfg(feature = "zk")]
    #[test]
    fn test_shielded_batch_pool_state() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();

        let mut batch = state.begin_batch();

        let mut pool = batch.get_shielded_pool_state().unwrap();
        pool.commitment_count = 10;
        pool.total_shielded = 5_000;
        batch.put_shielded_pool_state(&pool).unwrap();

        // Commit
        state.commit_batch(batch).unwrap();

        let loaded = state.get_shielded_pool_state().unwrap();
        assert_eq!(loaded.commitment_count, 10);
        assert_eq!(loaded.total_shielded, 5_000);
    }

    // ── P2-3: Cold storage tests ──

    fn make_test_block(slot: u64) -> Block {
        Block::new(
            slot,
            Hash::default(),
            Hash::default(),
            [0u8; 32],
            Vec::new(),
        )
    }

    #[test]
    fn test_cold_storage_open_and_attach() {
        let hot_dir = tempdir().unwrap();
        let cold_dir = tempdir().unwrap();
        let mut state = StateStore::open(hot_dir.path()).unwrap();
        assert!(!state.has_cold_storage());

        state.open_cold_store(cold_dir.path()).unwrap();
        assert!(state.has_cold_storage());
    }

    #[test]
    fn test_cold_storage_migrate_and_fallthrough() {
        let hot_dir = tempdir().unwrap();
        let cold_dir = tempdir().unwrap();
        let mut state = StateStore::open(hot_dir.path()).unwrap();
        state.open_cold_store(cold_dir.path()).unwrap();

        // Store blocks at slots 0..10
        for slot in 0..10u64 {
            let block = make_test_block(slot);
            state.put_block(&block).unwrap();
        }

        // All blocks readable from hot
        for slot in 0..10u64 {
            assert!(state.get_block_by_slot(slot).unwrap().is_some());
        }

        // Migrate blocks older than slot 5
        let migrated = state.migrate_to_cold(5).unwrap();
        assert_eq!(migrated, 5);

        // Slots 0..5 are now only in cold (fall-through read)
        for slot in 0..5u64 {
            let block = state.get_block_by_slot(slot).unwrap();
            assert!(block.is_some(), "slot {} should fall through to cold", slot);
            assert_eq!(block.unwrap().header.slot, slot);
        }

        // Slots 5..10 remain in hot
        for slot in 5..10u64 {
            let block = state.get_block_by_slot(slot).unwrap();
            assert!(block.is_some(), "slot {} should still be in hot", slot);
        }
    }

    #[test]
    fn test_cold_migration_without_cold_db_errors() {
        let hot_dir = tempdir().unwrap();
        let state = StateStore::open(hot_dir.path()).unwrap();
        let result = state.migrate_to_cold(100);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not attached"));
    }

    #[test]
    fn test_cold_migration_nothing_to_migrate() {
        let hot_dir = tempdir().unwrap();
        let cold_dir = tempdir().unwrap();
        let mut state = StateStore::open(hot_dir.path()).unwrap();
        state.open_cold_store(cold_dir.path()).unwrap();

        // No blocks stored — nothing to migrate
        let migrated = state.migrate_to_cold(100).unwrap();
        assert_eq!(migrated, 0);
    }

    #[test]
    fn test_cold_migration_idempotent() {
        let hot_dir = tempdir().unwrap();
        let cold_dir = tempdir().unwrap();
        let mut state = StateStore::open(hot_dir.path()).unwrap();
        state.open_cold_store(cold_dir.path()).unwrap();

        for slot in 0..5u64 {
            state.put_block(&make_test_block(slot)).unwrap();
        }

        // First migration moves 3 blocks
        let migrated1 = state.migrate_to_cold(3).unwrap();
        assert_eq!(migrated1, 3);

        // Second migration with same cutoff: nothing to move (already in cold)
        let migrated2 = state.migrate_to_cold(3).unwrap();
        assert_eq!(migrated2, 0);

        // All blocks still readable
        for slot in 0..5u64 {
            assert!(state.get_block_by_slot(slot).unwrap().is_some());
        }
    }

    #[test]
    fn test_cold_clone_shares_cold_db() {
        let hot_dir = tempdir().unwrap();
        let cold_dir = tempdir().unwrap();
        let mut state = StateStore::open(hot_dir.path()).unwrap();
        state.open_cold_store(cold_dir.path()).unwrap();

        // Store and migrate a block
        state.put_block(&make_test_block(0)).unwrap();
        state.migrate_to_cold(1).unwrap();

        // Clone should share the same cold DB
        let cloned = state.clone();
        assert!(cloned.has_cold_storage());
        let block = cloned.get_block_by_slot(0).unwrap();
        assert!(block.is_some(), "clone should read from shared cold DB");
    }
}
