use std::path::Path;
use std::sync::{Arc, Mutex};

use rocksdb::{BlockBasedOptions, Cache, ColumnFamilyDescriptor, Options, SliceTransform, DB};

use super::{
    MetricsStore, StateStore, CF_ACCOUNTS, CF_ACCOUNT_SNAPSHOTS, CF_ACCOUNT_TXS, CF_BLOCKS,
    CF_CONTRACT_MERKLE_LEAVES, CF_CONTRACT_STORAGE, CF_EVENTS, CF_EVENTS_BY_SLOT, CF_EVM_ACCOUNTS,
    CF_EVM_LOGS_BY_SLOT, CF_EVM_MAP, CF_EVM_RECEIPTS, CF_EVM_STORAGE, CF_EVM_TXS, CF_HOLDER_TOKENS,
    CF_MARKET_ACTIVITY, CF_MERKLE_LEAVES, CF_MOSSSTAKE, CF_NFT_ACTIVITY, CF_NFT_BY_COLLECTION,
    CF_NFT_BY_OWNER, CF_PENDING_VALIDATOR_CHANGES, CF_PROGRAMS, CF_PROGRAM_CALLS, CF_RESTRICTIONS,
    CF_RESTRICTION_INDEX_CODE_HASH, CF_RESTRICTION_INDEX_TARGET, CF_SHIELDED_COMMITMENTS,
    CF_SHIELDED_NULLIFIERS, CF_SHIELDED_POOL, CF_SLOTS, CF_SOLANA_HOLDER_TOKEN_ACCOUNTS,
    CF_SOLANA_TOKEN_ACCOUNTS, CF_STAKE_POOL, CF_STATS, CF_SYMBOL_BY_PROGRAM, CF_SYMBOL_REGISTRY,
    CF_TOKEN_BALANCES, CF_TOKEN_TRANSFERS, CF_TRANSACTIONS, CF_TX_BY_SLOT, CF_TX_META,
    CF_TX_TO_SLOT, CF_VALIDATORS, COLD_CF_ACCOUNT_TXS, COLD_CF_BLOCKS, COLD_CF_EVENTS,
    COLD_CF_PROGRAM_CALLS, COLD_CF_TOKEN_TRANSFERS, COLD_CF_TRANSACTIONS, COLD_CF_TX_TO_SLOT,
};

impl StateStore {
    /// Open or create state database with production-tuned column families.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        Self::open_with_cache_mb(path, None)
    }

    /// Open the state store with a configurable LRU cache size.
    pub fn open_with_cache_mb<P: AsRef<Path>>(
        path: P,
        cache_mb: Option<usize>,
    ) -> Result<Self, String> {
        let db_arc = Arc::new(open_hot_db(path, cache_mb)?);
        let metrics = Arc::new(MetricsStore::new());

        metrics.load(&db_arc)?;

        Ok(StateStore {
            db: db_arc,
            cold_db: None,
            metrics,
            event_seq_lock: Arc::new(std::sync::Mutex::new(())),
            transfer_seq_lock: Arc::new(std::sync::Mutex::new(())),
            tx_slot_seq_lock: Arc::new(std::sync::Mutex::new(())),
            block_write_lock: Arc::new(std::sync::Mutex::new(())),
            burned_lock: Arc::new(std::sync::Mutex::new(())),
            minted_lock: Arc::new(std::sync::Mutex::new(())),
            treasury_lock: Arc::new(std::sync::Mutex::new(())),
            blockhash_cache: Arc::new(Mutex::new(None)),
            archive_mode: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RocksDbCompressionProfile {
    Lz4,
    Zstd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RocksDbGlobalTuningProfile {
    pub(super) max_open_files: i32,
    pub(super) keep_log_file_num: usize,
    pub(super) max_total_wal_size: u64,
    pub(super) wal_bytes_per_sync: u64,
    pub(super) bytes_per_sync: u64,
    pub(super) max_background_jobs: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RocksDbCfTuningProfile {
    pub(super) compression: RocksDbCompressionProfile,
    pub(super) block_size: usize,
    pub(super) write_buffer_size: usize,
    pub(super) max_write_buffer_number: i32,
    pub(super) min_write_buffer_number_to_merge: i32,
    pub(super) dynamic_level_bytes: bool,
    pub(super) target_file_size_base: u64,
    pub(super) prefix_len: usize,
    pub(super) enable_bloom_filter: bool,
    pub(super) cache_index_and_filter_blocks: bool,
    pub(super) pin_l0_filter_and_index_blocks_in_cache: bool,
    pub(super) memtable_prefix_bloom_ratio_per_mille: u16,
}

/// Detect number of CPU cores for RocksDB parallelism.
fn num_cpus() -> i32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as i32)
        .unwrap_or(4)
        .min(8)
}

pub(super) fn hot_db_tuning_profile() -> RocksDbGlobalTuningProfile {
    RocksDbGlobalTuningProfile {
        max_open_files: 4096,
        keep_log_file_num: 5,
        max_total_wal_size: 256 * 1024 * 1024,
        wal_bytes_per_sync: 1024 * 1024,
        bytes_per_sync: 1024 * 1024,
        max_background_jobs: 4,
    }
}

pub(super) fn point_lookup_tuning_profile(prefix_len: usize) -> RocksDbCfTuningProfile {
    RocksDbCfTuningProfile {
        compression: RocksDbCompressionProfile::Lz4,
        block_size: 16 * 1024,
        write_buffer_size: 64 * 1024 * 1024,
        max_write_buffer_number: 3,
        min_write_buffer_number_to_merge: 2,
        dynamic_level_bytes: true,
        target_file_size_base: 64 * 1024 * 1024,
        prefix_len,
        enable_bloom_filter: true,
        cache_index_and_filter_blocks: true,
        pin_l0_filter_and_index_blocks_in_cache: true,
        memtable_prefix_bloom_ratio_per_mille: 0,
    }
}

pub(super) fn prefix_scan_tuning_profile(prefix_len: usize) -> RocksDbCfTuningProfile {
    RocksDbCfTuningProfile {
        compression: RocksDbCompressionProfile::Lz4,
        block_size: 16 * 1024,
        write_buffer_size: 32 * 1024 * 1024,
        max_write_buffer_number: 3,
        min_write_buffer_number_to_merge: 2,
        dynamic_level_bytes: true,
        target_file_size_base: 64 * 1024 * 1024,
        prefix_len,
        enable_bloom_filter: true,
        cache_index_and_filter_blocks: true,
        pin_l0_filter_and_index_blocks_in_cache: true,
        memtable_prefix_bloom_ratio_per_mille: 100,
    }
}

pub(super) fn write_heavy_tuning_profile(prefix_len: usize) -> RocksDbCfTuningProfile {
    RocksDbCfTuningProfile {
        compression: RocksDbCompressionProfile::Lz4,
        block_size: 16 * 1024,
        write_buffer_size: 128 * 1024 * 1024,
        max_write_buffer_number: 4,
        min_write_buffer_number_to_merge: 2,
        dynamic_level_bytes: true,
        target_file_size_base: 128 * 1024 * 1024,
        prefix_len,
        enable_bloom_filter: true,
        cache_index_and_filter_blocks: true,
        pin_l0_filter_and_index_blocks_in_cache: false,
        memtable_prefix_bloom_ratio_per_mille: 0,
    }
}

pub(super) fn small_cf_tuning_profile() -> RocksDbCfTuningProfile {
    RocksDbCfTuningProfile {
        compression: RocksDbCompressionProfile::Lz4,
        block_size: 0,
        write_buffer_size: 4 * 1024 * 1024,
        max_write_buffer_number: 2,
        min_write_buffer_number_to_merge: 0,
        dynamic_level_bytes: false,
        target_file_size_base: 0,
        prefix_len: 0,
        enable_bloom_filter: false,
        cache_index_and_filter_blocks: false,
        pin_l0_filter_and_index_blocks_in_cache: false,
        memtable_prefix_bloom_ratio_per_mille: 0,
    }
}

pub(super) fn archival_tuning_profile(prefix_len: usize) -> RocksDbCfTuningProfile {
    RocksDbCfTuningProfile {
        compression: RocksDbCompressionProfile::Zstd,
        block_size: 32 * 1024,
        write_buffer_size: 32 * 1024 * 1024,
        max_write_buffer_number: 2,
        min_write_buffer_number_to_merge: 0,
        dynamic_level_bytes: true,
        target_file_size_base: 128 * 1024 * 1024,
        prefix_len,
        enable_bloom_filter: true,
        cache_index_and_filter_blocks: true,
        pin_l0_filter_and_index_blocks_in_cache: false,
        memtable_prefix_bloom_ratio_per_mille: 0,
    }
}

fn apply_global_tuning(opts: &mut Options, tuning: RocksDbGlobalTuningProfile) {
    opts.set_max_open_files(tuning.max_open_files);
    opts.set_keep_log_file_num(tuning.keep_log_file_num);
    opts.set_max_total_wal_size(tuning.max_total_wal_size);
    opts.set_wal_recovery_mode(rocksdb::DBRecoveryMode::PointInTime);
    opts.set_wal_bytes_per_sync(tuning.wal_bytes_per_sync);
    opts.set_bytes_per_sync(tuning.bytes_per_sync);
    opts.set_max_background_jobs(tuning.max_background_jobs);
}

fn apply_cf_tuning(opts: &mut Options, shared_cache: &Cache, tuning: RocksDbCfTuningProfile) {
    match tuning.compression {
        RocksDbCompressionProfile::Lz4 => {
            opts.set_compression_type(rocksdb::DBCompressionType::Lz4)
        }
        RocksDbCompressionProfile::Zstd => {
            opts.set_compression_type(rocksdb::DBCompressionType::Zstd)
        }
    }

    let mut bbo = BlockBasedOptions::default();
    if tuning.enable_bloom_filter {
        bbo.set_bloom_filter(10.0, false);
    }
    bbo.set_block_cache(shared_cache);
    if tuning.block_size > 0 {
        bbo.set_block_size(tuning.block_size);
    }
    if tuning.cache_index_and_filter_blocks {
        bbo.set_cache_index_and_filter_blocks(true);
    }
    if tuning.pin_l0_filter_and_index_blocks_in_cache {
        bbo.set_pin_l0_filter_and_index_blocks_in_cache(true);
    }
    opts.set_block_based_table_factory(&bbo);

    if tuning.prefix_len > 0 {
        opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(tuning.prefix_len));
    }
    if tuning.memtable_prefix_bloom_ratio_per_mille > 0 {
        opts.set_memtable_prefix_bloom_ratio(
            tuning.memtable_prefix_bloom_ratio_per_mille as f64 / 1000.0,
        );
    }

    opts.set_write_buffer_size(tuning.write_buffer_size);
    opts.set_max_write_buffer_number(tuning.max_write_buffer_number);
    if tuning.min_write_buffer_number_to_merge > 0 {
        opts.set_min_write_buffer_number_to_merge(tuning.min_write_buffer_number_to_merge);
    }
    if tuning.dynamic_level_bytes {
        opts.set_level_compaction_dynamic_level_bytes(true);
    }
    if tuning.target_file_size_base > 0 {
        opts.set_target_file_size_base(tuning.target_file_size_base);
    }
}

fn detect_cache_size_mb(cache_mb: Option<usize>) -> usize {
    cache_mb.unwrap_or_else(|| {
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
            512
        }
        #[cfg(target_os = "macos")]
        {
            use std::process::Command;

            if let Ok(output) = Command::new("sysctl").arg("-n").arg("hw.memsize").output() {
                if let Ok(s) = String::from_utf8(output.stdout) {
                    if let Ok(bytes) = s.trim().parse::<usize>() {
                        let total_mb = bytes / (1024 * 1024);
                        return (total_mb / 4).clamp(256, 4096);
                    }
                }
            }
            512
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            512
        }
    })
}

fn point_lookup_options(shared_cache: &Cache, prefix_len: usize) -> Options {
    let mut opts = Options::default();
    apply_cf_tuning(
        &mut opts,
        shared_cache,
        point_lookup_tuning_profile(prefix_len),
    );
    opts
}

fn prefix_scan_options(shared_cache: &Cache, prefix_len: usize) -> Options {
    let mut opts = Options::default();
    apply_cf_tuning(
        &mut opts,
        shared_cache,
        prefix_scan_tuning_profile(prefix_len),
    );
    opts
}

fn write_heavy_options(shared_cache: &Cache, prefix_len: usize) -> Options {
    let mut opts = Options::default();
    apply_cf_tuning(
        &mut opts,
        shared_cache,
        write_heavy_tuning_profile(prefix_len),
    );
    opts
}

fn small_cf_options(shared_cache: &Cache) -> Options {
    let mut opts = Options::default();
    apply_cf_tuning(&mut opts, shared_cache, small_cf_tuning_profile());
    opts
}

fn archival_options(shared_cache: &Cache, prefix_len: usize) -> Options {
    let mut opts = Options::default();
    apply_cf_tuning(&mut opts, shared_cache, archival_tuning_profile(prefix_len));
    opts
}

fn build_hot_cf_descriptors(shared_cache: &Cache) -> Vec<ColumnFamilyDescriptor> {
    vec![
        ColumnFamilyDescriptor::new(CF_ACCOUNTS, point_lookup_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_TRANSACTIONS, point_lookup_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_BLOCKS, point_lookup_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_TX_TO_SLOT, point_lookup_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_SYMBOL_BY_PROGRAM, point_lookup_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_ACCOUNT_TXS, prefix_scan_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_NFT_BY_OWNER, prefix_scan_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_NFT_BY_COLLECTION, prefix_scan_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_NFT_ACTIVITY, prefix_scan_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_PROGRAM_CALLS, prefix_scan_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_MARKET_ACTIVITY, prefix_scan_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_TOKEN_BALANCES, prefix_scan_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_HOLDER_TOKENS, prefix_scan_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(
            CF_SOLANA_TOKEN_ACCOUNTS,
            point_lookup_options(shared_cache, 32),
        ),
        ColumnFamilyDescriptor::new(
            CF_SOLANA_HOLDER_TOKEN_ACCOUNTS,
            prefix_scan_options(shared_cache, 32),
        ),
        ColumnFamilyDescriptor::new(CF_TOKEN_TRANSFERS, prefix_scan_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_EVENTS, prefix_scan_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_TX_BY_SLOT, prefix_scan_options(shared_cache, 8)),
        ColumnFamilyDescriptor::new(CF_EVENTS_BY_SLOT, prefix_scan_options(shared_cache, 8)),
        ColumnFamilyDescriptor::new(CF_EVM_TXS, archival_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_EVM_RECEIPTS, archival_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_EVM_ACCOUNTS, point_lookup_options(shared_cache, 20)),
        ColumnFamilyDescriptor::new(CF_EVM_MAP, point_lookup_options(shared_cache, 20)),
        ColumnFamilyDescriptor::new(CF_EVM_STORAGE, prefix_scan_options(shared_cache, 20)),
        ColumnFamilyDescriptor::new(CF_SLOTS, point_lookup_options(shared_cache, 8)),
        ColumnFamilyDescriptor::new(CF_STATS, write_heavy_options(shared_cache, 0)),
        ColumnFamilyDescriptor::new(CF_VALIDATORS, small_cf_options(shared_cache)),
        ColumnFamilyDescriptor::new(CF_MOSSSTAKE, small_cf_options(shared_cache)),
        ColumnFamilyDescriptor::new(CF_STAKE_POOL, small_cf_options(shared_cache)),
        ColumnFamilyDescriptor::new(CF_PROGRAMS, point_lookup_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_SYMBOL_REGISTRY, small_cf_options(shared_cache)),
        ColumnFamilyDescriptor::new(CF_CONTRACT_STORAGE, prefix_scan_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_MERKLE_LEAVES, point_lookup_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(
            CF_CONTRACT_MERKLE_LEAVES,
            prefix_scan_options(shared_cache, 32),
        ),
        ColumnFamilyDescriptor::new(
            CF_SHIELDED_COMMITMENTS,
            point_lookup_options(shared_cache, 8),
        ),
        ColumnFamilyDescriptor::new(
            CF_SHIELDED_NULLIFIERS,
            point_lookup_options(shared_cache, 32),
        ),
        ColumnFamilyDescriptor::new(CF_SHIELDED_POOL, small_cf_options(shared_cache)),
        ColumnFamilyDescriptor::new(CF_EVM_LOGS_BY_SLOT, prefix_scan_options(shared_cache, 8)),
        ColumnFamilyDescriptor::new(CF_ACCOUNT_SNAPSHOTS, archival_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(
            CF_PENDING_VALIDATOR_CHANGES,
            prefix_scan_options(shared_cache, 8),
        ),
        ColumnFamilyDescriptor::new(CF_TX_META, point_lookup_options(shared_cache, 32)),
        ColumnFamilyDescriptor::new(CF_RESTRICTIONS, point_lookup_options(shared_cache, 8)),
        ColumnFamilyDescriptor::new(
            CF_RESTRICTION_INDEX_TARGET,
            prefix_scan_options(shared_cache, 1),
        ),
        ColumnFamilyDescriptor::new(
            CF_RESTRICTION_INDEX_CODE_HASH,
            prefix_scan_options(shared_cache, 32),
        ),
    ]
}

fn cold_archival_cf_options() -> Options {
    let mut opts = Options::default();
    opts.set_compression_type(rocksdb::DBCompressionType::Zstd);
    let mut bbo = BlockBasedOptions::default();
    bbo.set_bloom_filter(10.0, false);
    bbo.set_block_size(32 * 1024);
    opts.set_block_based_table_factory(&bbo);
    opts.set_write_buffer_size(32 * 1024 * 1024);
    opts
}

fn build_cold_cf_descriptors() -> Vec<ColumnFamilyDescriptor> {
    vec![
        ColumnFamilyDescriptor::new(COLD_CF_BLOCKS, cold_archival_cf_options()),
        ColumnFamilyDescriptor::new(COLD_CF_TRANSACTIONS, cold_archival_cf_options()),
        ColumnFamilyDescriptor::new(COLD_CF_TX_TO_SLOT, cold_archival_cf_options()),
        ColumnFamilyDescriptor::new(COLD_CF_ACCOUNT_TXS, cold_archival_cf_options()),
        ColumnFamilyDescriptor::new(COLD_CF_EVENTS, cold_archival_cf_options()),
        ColumnFamilyDescriptor::new(COLD_CF_TOKEN_TRANSFERS, cold_archival_cf_options()),
        ColumnFamilyDescriptor::new(COLD_CF_PROGRAM_CALLS, cold_archival_cf_options()),
    ]
}

pub(super) fn open_hot_db<P: AsRef<Path>>(path: P, cache_mb: Option<usize>) -> Result<DB, String> {
    let mut db_opts = Options::default();
    db_opts.create_if_missing(true);
    db_opts.create_missing_column_families(true);
    apply_global_tuning(&mut db_opts, hot_db_tuning_profile());
    db_opts.increase_parallelism(num_cpus());

    let cache_size_mb = detect_cache_size_mb(cache_mb);
    tracing::info!("🗄️  RocksDB shared cache: {} MB", cache_size_mb);
    let shared_cache = Cache::new_lru_cache(cache_size_mb * 1024 * 1024);

    DB::open_cf_descriptors(&db_opts, path, build_hot_cf_descriptors(&shared_cache))
        .map_err(|e| format!("Failed to open database: {}", e))
}

pub(super) fn open_cold_db<P: AsRef<Path>>(cold_path: P) -> Result<DB, String> {
    let mut db_opts = Options::default();
    db_opts.create_if_missing(true);
    db_opts.create_missing_column_families(true);
    db_opts.set_max_open_files(256);
    db_opts.set_keep_log_file_num(3);
    db_opts.set_max_total_wal_size(64 * 1024 * 1024);
    db_opts.increase_parallelism(2);
    db_opts.set_max_background_jobs(2);

    DB::open_cf_descriptors(&db_opts, cold_path.as_ref(), build_cold_cf_descriptors())
        .map_err(|e| format!("Failed to open cold DB: {}", e))
}
