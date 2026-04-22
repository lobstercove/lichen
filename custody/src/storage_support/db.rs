use super::*;

pub(crate) fn open_db<P: AsRef<Path>>(path: P) -> Result<DB, String> {
    let mut opts = Options::default();
    opts.create_if_missing(true);
    opts.create_missing_column_families(true);
    opts.set_max_open_files(2048);
    opts.set_keep_log_file_num(5);
    opts.set_max_total_wal_size(128 * 1024 * 1024);

    let cache_size_mb: usize = {
        #[cfg(target_os = "linux")]
        {
            std::fs::read_to_string("/proc/meminfo")
                .ok()
                .and_then(|meminfo| {
                    meminfo
                        .lines()
                        .find(|line| line.starts_with("MemTotal:"))
                        .and_then(|line| line.split_whitespace().nth(1))
                        .and_then(|kb| kb.parse::<usize>().ok())
                        .map(|total_kb| (total_kb / 1024 / 4).clamp(128, 2048))
                })
                .unwrap_or(256)
        }
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("sysctl")
                .arg("-n")
                .arg("hw.memsize")
                .output()
                .ok()
                .and_then(|output| String::from_utf8(output.stdout).ok())
                .and_then(|text| text.trim().parse::<usize>().ok())
                .map(|bytes| (bytes / (1024 * 1024) / 4).clamp(128, 2048))
                .unwrap_or(256)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            256
        }
    };
    info!("Custody DB cache: {} MB", cache_size_mb);
    let shared_cache = Cache::new_lru_cache(cache_size_mb * 1024 * 1024);

    let point_lookup_opts = || -> Options {
        let mut cf_opts = Options::default();
        let mut block_options = BlockBasedOptions::default();
        block_options.set_bloom_filter(10.0, false);
        block_options.set_block_cache(&shared_cache);
        block_options.set_cache_index_and_filter_blocks(true);
        block_options.set_pin_l0_filter_and_index_blocks_in_cache(true);
        cf_opts.set_block_based_table_factory(&block_options);
        cf_opts.set_write_buffer_size(32 * 1024 * 1024);
        cf_opts.set_max_write_buffer_number(3);
        cf_opts.set_level_compaction_dynamic_level_bytes(true);
        cf_opts
    };

    let prefix_scan_opts = |prefix_len: usize| -> Options {
        let mut cf_opts = Options::default();
        let mut block_options = BlockBasedOptions::default();
        block_options.set_bloom_filter(10.0, false);
        block_options.set_block_cache(&shared_cache);
        block_options.set_cache_index_and_filter_blocks(true);
        block_options.set_pin_l0_filter_and_index_blocks_in_cache(true);
        cf_opts.set_block_based_table_factory(&block_options);
        cf_opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(prefix_len));
        cf_opts.set_memtable_prefix_bloom_ratio(0.1);
        cf_opts.set_write_buffer_size(32 * 1024 * 1024);
        cf_opts.set_max_write_buffer_number(3);
        cf_opts.set_level_compaction_dynamic_level_bytes(true);
        cf_opts
    };

    let write_heavy_opts = || -> Options {
        let mut cf_opts = Options::default();
        let mut block_options = BlockBasedOptions::default();
        block_options.set_bloom_filter(10.0, false);
        block_options.set_block_cache(&shared_cache);
        block_options.set_cache_index_and_filter_blocks(true);
        cf_opts.set_block_based_table_factory(&block_options);
        cf_opts.set_write_buffer_size(64 * 1024 * 1024);
        cf_opts.set_max_write_buffer_number(4);
        cf_opts.set_level_compaction_dynamic_level_bytes(true);
        cf_opts
    };

    let small_cf_opts = || -> Options {
        let mut cf_opts = Options::default();
        let mut block_options = BlockBasedOptions::default();
        block_options.set_block_cache(&shared_cache);
        cf_opts.set_block_based_table_factory(&block_options);
        cf_opts.set_write_buffer_size(4 * 1024 * 1024);
        cf_opts.set_max_write_buffer_number(2);
        cf_opts
    };

    let cfs = vec![
        ColumnFamilyDescriptor::new(CF_DEPOSITS, point_lookup_opts()),
        ColumnFamilyDescriptor::new(CF_INDEXES, point_lookup_opts()),
        ColumnFamilyDescriptor::new(CF_ADDRESS_INDEX, prefix_scan_opts(8)),
        ColumnFamilyDescriptor::new(CF_DEPOSIT_EVENTS, write_heavy_opts()),
        ColumnFamilyDescriptor::new(CF_SWEEP_JOBS, point_lookup_opts()),
        ColumnFamilyDescriptor::new(CF_ADDRESS_BALANCES, point_lookup_opts()),
        ColumnFamilyDescriptor::new(CF_TOKEN_BALANCES, prefix_scan_opts(7)),
        ColumnFamilyDescriptor::new(CF_CREDIT_JOBS, point_lookup_opts()),
        ColumnFamilyDescriptor::new(CF_WITHDRAWAL_JOBS, point_lookup_opts()),
        ColumnFamilyDescriptor::new(CF_AUDIT_EVENTS, write_heavy_opts()),
        ColumnFamilyDescriptor::new(CF_AUDIT_EVENTS_BY_TIME, write_heavy_opts()),
        ColumnFamilyDescriptor::new(CF_AUDIT_EVENTS_BY_TYPE_TIME, prefix_scan_opts(12)),
        ColumnFamilyDescriptor::new(CF_AUDIT_EVENTS_BY_ENTITY_TIME, prefix_scan_opts(12)),
        ColumnFamilyDescriptor::new(CF_AUDIT_EVENTS_BY_TX_TIME, prefix_scan_opts(12)),
        ColumnFamilyDescriptor::new(CF_CURSORS, small_cf_opts()),
        ColumnFamilyDescriptor::new(CF_RESERVE_LEDGER, write_heavy_opts()),
        ColumnFamilyDescriptor::new(CF_REBALANCE_JOBS, point_lookup_opts()),
        ColumnFamilyDescriptor::new(CF_BRIDGE_AUTH_REPLAY, point_lookup_opts()),
        ColumnFamilyDescriptor::new(CF_STATUS_INDEX, prefix_scan_opts(7)),
        ColumnFamilyDescriptor::new(CF_TX_INTENTS, prefix_scan_opts(7)),
        ColumnFamilyDescriptor::new(CF_WEBHOOKS, point_lookup_opts()),
    ];

    DB::open_cf_descriptors(&opts, path, cfs).map_err(|e| format!("db open: {}", e))
}
