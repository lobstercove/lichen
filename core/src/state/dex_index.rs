use super::*;
use rocksdb::IteratorMode;
use std::collections::HashMap;

const DEX_INDEX_SCHEMA_VERSION: u64 = 1;
const DEX_INDEX_SCHEMA_KEY: &[u8] = b"dex_index_schema_version";
const DEX_INDEX_ORDER_CURSOR_KEY: &[u8] = b"dex_index_order_cursor";
const DEX_INDEX_TRADE_CURSOR_KEY: &[u8] = b"dex_index_trade_cursor";
const DEX_INDEX_BACKFILL_CHUNK: u64 = 10_000;
const DEX_SIDE_BUY: u8 = 0;
const DEX_SIDE_SELL: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DexOrderbookLevel {
    pub price_raw: u64,
    pub quantity: u64,
    pub orders: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DexIndexBackfillReport {
    pub dex_program: Option<Pubkey>,
    pub schema_reset: bool,
    pub latest_order_count: u64,
    pub latest_trade_count: u64,
    pub order_cursor: u64,
    pub trade_cursor: u64,
    pub orders_indexed: u64,
    pub trades_indexed: u64,
}

impl DexIndexBackfillReport {
    fn skipped() -> Self {
        Self {
            dex_program: None,
            schema_reset: false,
            latest_order_count: 0,
            latest_trade_count: 0,
            order_cursor: 0,
            trade_cursor: 0,
            orders_indexed: 0,
            trades_indexed: 0,
        }
    }
}

fn dex_order_id_from_storage_key(storage_key: &[u8]) -> Option<u64> {
    let suffix = storage_key.strip_prefix(b"dex_order_")?;
    std::str::from_utf8(suffix).ok()?.parse().ok()
}

fn dex_trade_id_from_storage_key(storage_key: &[u8]) -> Option<u64> {
    let suffix = storage_key.strip_prefix(b"dex_trade_")?;
    std::str::from_utf8(suffix).ok()?.parse().ok()
}

fn pair_order_index_key(pair_id: u64, order_id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(16);
    key.extend_from_slice(&pair_id.to_be_bytes());
    key.extend_from_slice(&order_id.to_be_bytes());
    key
}

fn pair_trade_index_key(pair_id: u64, trade_id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(16);
    key.extend_from_slice(&pair_id.to_be_bytes());
    key.extend_from_slice(&trade_id.to_be_bytes());
    key
}

fn taker_bytes(taker_hex: &str) -> Option<[u8; 32]> {
    let decoded = hex::decode(taker_hex).ok()?;
    decoded.as_slice().try_into().ok()
}

fn taker_trade_index_key(taker: &[u8; 32], trade_id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(40);
    key.extend_from_slice(taker);
    key.extend_from_slice(&trade_id.to_be_bytes());
    key
}

fn pair_taker_trade_index_key(pair_id: u64, taker: &[u8; 32], trade_id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(48);
    key.extend_from_slice(&pair_id.to_be_bytes());
    key.extend_from_slice(taker);
    key.extend_from_slice(&trade_id.to_be_bytes());
    key
}

fn orderbook_level_key(pair_id: u64, side: u8, price_raw: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(17);
    key.extend_from_slice(&pair_id.to_be_bytes());
    key.push(side);
    key.extend_from_slice(&price_raw.to_be_bytes());
    key
}

fn encode_orderbook_level_value(quantity: u64, orders: u64) -> [u8; 16] {
    let mut value = [0u8; 16];
    value[..8].copy_from_slice(&quantity.to_le_bytes());
    value[8..].copy_from_slice(&orders.to_le_bytes());
    value
}

fn decode_orderbook_level_value(data: &[u8]) -> (u64, u64) {
    if data.len() < 16 {
        return (0, 0);
    }
    let quantity = u64::from_le_bytes(data[..8].try_into().unwrap_or([0; 8]));
    let orders = u64::from_le_bytes(data[8..16].try_into().unwrap_or([0; 8]));
    (quantity, orders)
}

fn active_level_delta(order: &crate::dex::DexOrder) -> Option<(Vec<u8>, u64, u64)> {
    if order.status != "open" && order.status != "partial" {
        return None;
    }
    if order.order_type == "market" || order.price_raw == 0 {
        return None;
    }
    let remaining = order.quantity.saturating_sub(order.filled);
    if remaining == 0 {
        return None;
    }
    let side = if order.side == "buy" {
        DEX_SIDE_BUY
    } else {
        DEX_SIDE_SELL
    };
    Some((
        orderbook_level_key(order.pair_id, side, order.price_raw),
        remaining,
        1,
    ))
}

fn add_level_delta(
    deltas: &mut HashMap<Vec<u8>, (i128, i128)>,
    key: Vec<u8>,
    quantity_delta: i128,
    orders_delta: i128,
) {
    let entry = deltas.entry(key).or_insert((0, 0));
    entry.0 = entry.0.saturating_add(quantity_delta);
    entry.1 = entry.1.saturating_add(orders_delta);
}

fn apply_signed_delta(current: u64, delta: i128) -> u64 {
    if delta >= 0 {
        current.saturating_add(delta.min(u64::MAX as i128) as u64)
    } else {
        current.saturating_sub((-delta).min(u64::MAX as i128) as u64)
    }
}

fn read_stats_u64(db: &DB, key: &[u8]) -> Result<u64, String> {
    let cf = db
        .cf_handle(CF_STATS)
        .ok_or_else(|| "Stats CF not found".to_string())?;
    match db
        .get_cf(&cf, key)
        .map_err(|e| format!("Failed to read DEX index metadata: {}", e))?
    {
        Some(data) if data.len() >= 8 => Ok(u64::from_le_bytes(data[..8].try_into().unwrap())),
        _ => Ok(0),
    }
}

fn read_contract_storage_u64(db: &DB, program: &Pubkey, storage_key: &[u8]) -> Result<u64, String> {
    let cf = db
        .cf_handle(CF_CONTRACT_STORAGE)
        .ok_or_else(|| "Contract storage CF not found".to_string())?;
    let mut full_key = Vec::with_capacity(32 + storage_key.len());
    full_key.extend_from_slice(&program.0);
    full_key.extend_from_slice(storage_key);
    match db
        .get_cf(&cf, &full_key)
        .map_err(|e| format!("Failed to read DEX contract counter: {}", e))?
    {
        Some(data) if data.len() >= 8 => Ok(u64::from_le_bytes(data[..8].try_into().unwrap())),
        _ => Ok(0),
    }
}

fn read_contract_storage_bytes(
    db: &DB,
    program: &Pubkey,
    storage_key: &[u8],
) -> Result<Option<Vec<u8>>, String> {
    let cf = db
        .cf_handle(CF_CONTRACT_STORAGE)
        .ok_or_else(|| "Contract storage CF not found".to_string())?;
    let mut full_key = Vec::with_capacity(32 + storage_key.len());
    full_key.extend_from_slice(&program.0);
    full_key.extend_from_slice(storage_key);
    db.get_cf(&cf, &full_key)
        .map(|value| value.map(|v| v.to_vec()))
        .map_err(|e| format!("Failed to read DEX contract storage: {}", e))
}

fn dex_program_from_registry(db: &DB) -> Result<Option<Pubkey>, String> {
    let cf = db
        .cf_handle(CF_SYMBOL_REGISTRY)
        .ok_or_else(|| "Symbol registry CF not found".to_string())?;
    match db
        .get_cf(&cf, b"DEX")
        .map_err(|e| format!("Failed to resolve DEX symbol: {}", e))?
    {
        Some(data) => {
            let entry: SymbolRegistryEntry = serde_json::from_slice(&data)
                .map_err(|e| format!("Failed to decode DEX symbol registry: {}", e))?;
            Ok(Some(entry.program))
        }
        None => Ok(None),
    }
}

fn is_dex_core_program(db: &DB, program: &Pubkey) -> Result<bool, String> {
    Ok(dex_program_from_registry(db)?
        .map(|dex_program| dex_program == *program)
        .unwrap_or(false))
}

fn stage_dex_order_index_mutation(
    db: &DB,
    batch: &mut WriteBatch,
    level_deltas: &mut HashMap<Vec<u8>, (i128, i128)>,
    old_data: Option<&[u8]>,
    new_data: Option<&[u8]>,
) -> Result<(), String> {
    let cf_pair = db
        .cf_handle(CF_DEX_ORDERS_BY_PAIR)
        .ok_or_else(|| "DEX orders-by-pair CF not found".to_string())?;

    if let Some(order) = old_data.and_then(crate::dex::decode_order) {
        batch.delete_cf(
            &cf_pair,
            pair_order_index_key(order.pair_id, order.order_id),
        );
        if let Some((key, quantity, orders)) = active_level_delta(&order) {
            add_level_delta(level_deltas, key, -(quantity as i128), -(orders as i128));
        }
    }

    if let Some(order) = new_data.and_then(crate::dex::decode_order) {
        batch.put_cf(
            &cf_pair,
            pair_order_index_key(order.pair_id, order.order_id),
            [],
        );
        if let Some((key, quantity, orders)) = active_level_delta(&order) {
            add_level_delta(level_deltas, key, quantity as i128, orders as i128);
        }
    }

    Ok(())
}

fn stage_dex_trade_index_mutation(
    db: &DB,
    batch: &mut WriteBatch,
    old_data: Option<&[u8]>,
    new_data: Option<&[u8]>,
) -> Result<(), String> {
    let cf_pair = db
        .cf_handle(CF_DEX_TRADES_BY_PAIR)
        .ok_or_else(|| "DEX trades-by-pair CF not found".to_string())?;
    let cf_taker = db
        .cf_handle(CF_DEX_TRADES_BY_TAKER)
        .ok_or_else(|| "DEX trades-by-taker CF not found".to_string())?;
    let cf_pair_taker = db
        .cf_handle(CF_DEX_TRADES_BY_PAIR_TAKER)
        .ok_or_else(|| "DEX trades-by-pair-taker CF not found".to_string())?;

    if let Some(trade) = old_data.and_then(crate::dex::decode_trade) {
        batch.delete_cf(
            &cf_pair,
            pair_trade_index_key(trade.pair_id, trade.trade_id),
        );
        if let Some(taker) = taker_bytes(&trade.taker) {
            batch.delete_cf(&cf_taker, taker_trade_index_key(&taker, trade.trade_id));
            batch.delete_cf(
                &cf_pair_taker,
                pair_taker_trade_index_key(trade.pair_id, &taker, trade.trade_id),
            );
        }
    }

    if let Some(trade) = new_data.and_then(crate::dex::decode_trade) {
        batch.put_cf(
            &cf_pair,
            pair_trade_index_key(trade.pair_id, trade.trade_id),
            [],
        );
        if let Some(taker) = taker_bytes(&trade.taker) {
            batch.put_cf(&cf_taker, taker_trade_index_key(&taker, trade.trade_id), []);
            batch.put_cf(
                &cf_pair_taker,
                pair_taker_trade_index_key(trade.pair_id, &taker, trade.trade_id),
                [],
            );
        }
    }

    Ok(())
}

fn stage_dex_storage_index_mutation(
    db: &DB,
    batch: &mut WriteBatch,
    level_deltas: &mut HashMap<Vec<u8>, (i128, i128)>,
    storage_key: &[u8],
    old_data: Option<&[u8]>,
    new_data: Option<&[u8]>,
) -> Result<(), String> {
    if dex_order_id_from_storage_key(storage_key).is_some() {
        stage_dex_order_index_mutation(db, batch, level_deltas, old_data, new_data)
    } else if dex_trade_id_from_storage_key(storage_key).is_some() {
        stage_dex_trade_index_mutation(db, batch, old_data, new_data)
    } else {
        Ok(())
    }
}

fn clear_cf(db: &DB, batch: &mut WriteBatch, cf_name: &str) -> Result<u64, String> {
    let cf = db
        .cf_handle(cf_name)
        .ok_or_else(|| format!("{} CF not found", cf_name))?;
    let keys: Vec<Vec<u8>> = db
        .iterator_cf(&cf, IteratorMode::Start)
        .filter_map(|item| item.ok().map(|(key, _)| key.to_vec()))
        .collect();
    let count = keys.len() as u64;
    for key in keys {
        batch.delete_cf(&cf, key);
    }
    Ok(count)
}

fn collect_index_ids(
    db: &DB,
    cf_name: &str,
    prefix: &[u8],
    start_key: &[u8],
    id_offset: usize,
    limit: usize,
    direction: Direction,
) -> Result<Vec<u64>, String> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let cf = db
        .cf_handle(cf_name)
        .ok_or_else(|| format!("{} CF not found", cf_name))?;
    let mut ids = Vec::with_capacity(limit.min(256));
    let iter = db.iterator_cf(&cf, IteratorMode::From(start_key, direction));
    for item in iter {
        let (key, _) = item.map_err(|e| format!("DEX index iterator error: {}", e))?;
        if !key.starts_with(prefix) {
            break;
        }
        if key.len() < id_offset + 8 {
            continue;
        }
        let id = u64::from_be_bytes(key[id_offset..id_offset + 8].try_into().unwrap_or([0; 8]));
        ids.push(id);
        if ids.len() >= limit {
            break;
        }
    }
    Ok(ids)
}

impl StateStore {
    pub(super) fn should_stage_dex_contract_storage_index(storage_key: &[u8]) -> bool {
        dex_order_id_from_storage_key(storage_key).is_some()
            || dex_trade_id_from_storage_key(storage_key).is_some()
    }

    pub(super) fn stage_dex_contract_storage_indexes(
        &self,
        batch: &mut WriteBatch,
        level_deltas: &mut HashMap<Vec<u8>, (i128, i128)>,
        program: &Pubkey,
        storage_key: &[u8],
        old_data: Option<&[u8]>,
        new_data: Option<&[u8]>,
    ) -> Result<(), String> {
        if !Self::should_stage_dex_contract_storage_index(storage_key) {
            return Ok(());
        }
        if !is_dex_core_program(&self.db, program)? {
            return Ok(());
        }
        stage_dex_storage_index_mutation(
            &self.db,
            batch,
            level_deltas,
            storage_key,
            old_data,
            new_data,
        )
    }

    pub(super) fn apply_dex_orderbook_level_deltas(
        &self,
        batch: &mut WriteBatch,
        deltas: &HashMap<Vec<u8>, (i128, i128)>,
    ) -> Result<(), String> {
        if deltas.is_empty() {
            return Ok(());
        }
        let cf = self
            .db
            .cf_handle(CF_DEX_ORDERBOOK_LEVELS)
            .ok_or_else(|| "DEX orderbook-levels CF not found".to_string())?;

        for (key, (quantity_delta, orders_delta)) in deltas {
            let (current_quantity, current_orders) = match self
                .db
                .get_cf(&cf, key)
                .map_err(|e| format!("Failed to read DEX orderbook level: {}", e))?
            {
                Some(data) => decode_orderbook_level_value(&data),
                None => (0, 0),
            };
            let new_quantity = apply_signed_delta(current_quantity, *quantity_delta);
            let new_orders = apply_signed_delta(current_orders, *orders_delta);
            if new_quantity == 0 || new_orders == 0 {
                batch.delete_cf(&cf, key);
            } else {
                batch.put_cf(
                    &cf,
                    key,
                    encode_orderbook_level_value(new_quantity, new_orders),
                );
            }
        }

        Ok(())
    }

    fn clear_dex_indexes(&self) -> Result<(), String> {
        let _guard = self
            .dex_index_lock
            .lock()
            .map_err(|e| format!("dex_index_lock poisoned: {}", e))?;
        let mut batch = WriteBatch::default();
        clear_cf(&self.db, &mut batch, CF_DEX_ORDERS_BY_PAIR)?;
        clear_cf(&self.db, &mut batch, CF_DEX_TRADES_BY_PAIR)?;
        clear_cf(&self.db, &mut batch, CF_DEX_TRADES_BY_TAKER)?;
        clear_cf(&self.db, &mut batch, CF_DEX_TRADES_BY_PAIR_TAKER)?;
        clear_cf(&self.db, &mut batch, CF_DEX_ORDERBOOK_LEVELS)?;
        let cf_stats = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        batch.delete_cf(&cf_stats, DEX_INDEX_SCHEMA_KEY);
        batch.put_cf(&cf_stats, DEX_INDEX_ORDER_CURSOR_KEY, 0u64.to_le_bytes());
        batch.put_cf(&cf_stats, DEX_INDEX_TRADE_CURSOR_KEY, 0u64.to_le_bytes());
        self.db
            .write(batch)
            .map_err(|e| format!("Failed to clear DEX indexes: {}", e))
    }

    pub fn sync_dex_indexes_from_contract_storage(&self) -> Result<DexIndexBackfillReport, String> {
        let Some(dex_program) = dex_program_from_registry(&self.db)? else {
            return Ok(DexIndexBackfillReport::skipped());
        };

        let latest_order_count = read_contract_storage_u64(
            &self.db,
            &dex_program,
            crate::dex::DEX_ORDER_COUNT_KEY.as_bytes(),
        )?;
        let latest_trade_count = read_contract_storage_u64(
            &self.db,
            &dex_program,
            crate::dex::DEX_TRADE_COUNT_KEY.as_bytes(),
        )?;
        let schema_version = read_stats_u64(&self.db, DEX_INDEX_SCHEMA_KEY)?;
        let mut order_cursor = read_stats_u64(&self.db, DEX_INDEX_ORDER_CURSOR_KEY)?;
        let mut trade_cursor = read_stats_u64(&self.db, DEX_INDEX_TRADE_CURSOR_KEY)?;
        let mut schema_reset = false;

        if schema_version != DEX_INDEX_SCHEMA_VERSION
            || order_cursor > latest_order_count
            || trade_cursor > latest_trade_count
        {
            self.clear_dex_indexes()?;
            order_cursor = 0;
            trade_cursor = 0;
            schema_reset = true;
        }

        let mut orders_indexed = 0u64;
        while order_cursor < latest_order_count {
            let start = order_cursor.saturating_add(1);
            let end = latest_order_count.min(
                start
                    .saturating_add(DEX_INDEX_BACKFILL_CHUNK)
                    .saturating_sub(1),
            );
            let mut batch = WriteBatch::default();
            let mut level_deltas = HashMap::new();
            for order_id in start..=end {
                let storage_key = crate::dex::order_key(order_id);
                if let Some(data) =
                    read_contract_storage_bytes(&self.db, &dex_program, storage_key.as_bytes())?
                {
                    stage_dex_storage_index_mutation(
                        &self.db,
                        &mut batch,
                        &mut level_deltas,
                        storage_key.as_bytes(),
                        None,
                        Some(&data),
                    )?;
                    orders_indexed = orders_indexed.saturating_add(1);
                }
            }
            let _guard = self
                .dex_index_lock
                .lock()
                .map_err(|e| format!("dex_index_lock poisoned: {}", e))?;
            self.apply_dex_orderbook_level_deltas(&mut batch, &level_deltas)?;
            let cf_stats = self
                .db
                .cf_handle(CF_STATS)
                .ok_or_else(|| "Stats CF not found".to_string())?;
            batch.put_cf(&cf_stats, DEX_INDEX_ORDER_CURSOR_KEY, end.to_le_bytes());
            self.db
                .write(batch)
                .map_err(|e| format!("Failed to backfill DEX order indexes: {}", e))?;
            drop(_guard);
            order_cursor = end;
        }

        let mut trades_indexed = 0u64;
        while trade_cursor < latest_trade_count {
            let start = trade_cursor.saturating_add(1);
            let end = latest_trade_count.min(
                start
                    .saturating_add(DEX_INDEX_BACKFILL_CHUNK)
                    .saturating_sub(1),
            );
            let mut batch = WriteBatch::default();
            let mut level_deltas = HashMap::new();
            for trade_id in start..=end {
                let storage_key = crate::dex::trade_key(trade_id);
                if let Some(data) =
                    read_contract_storage_bytes(&self.db, &dex_program, storage_key.as_bytes())?
                {
                    stage_dex_storage_index_mutation(
                        &self.db,
                        &mut batch,
                        &mut level_deltas,
                        storage_key.as_bytes(),
                        None,
                        Some(&data),
                    )?;
                    trades_indexed = trades_indexed.saturating_add(1);
                }
            }
            let _guard = self
                .dex_index_lock
                .lock()
                .map_err(|e| format!("dex_index_lock poisoned: {}", e))?;
            self.apply_dex_orderbook_level_deltas(&mut batch, &level_deltas)?;
            let cf_stats = self
                .db
                .cf_handle(CF_STATS)
                .ok_or_else(|| "Stats CF not found".to_string())?;
            batch.put_cf(&cf_stats, DEX_INDEX_TRADE_CURSOR_KEY, end.to_le_bytes());
            self.db
                .write(batch)
                .map_err(|e| format!("Failed to backfill DEX trade indexes: {}", e))?;
            drop(_guard);
            trade_cursor = end;
        }

        let cf_stats = self
            .db
            .cf_handle(CF_STATS)
            .ok_or_else(|| "Stats CF not found".to_string())?;
        self.db
            .put_cf(
                &cf_stats,
                DEX_INDEX_SCHEMA_KEY,
                DEX_INDEX_SCHEMA_VERSION.to_le_bytes(),
            )
            .map_err(|e| format!("Failed to store DEX index schema version: {}", e))?;

        Ok(DexIndexBackfillReport {
            dex_program: Some(dex_program),
            schema_reset,
            latest_order_count,
            latest_trade_count,
            order_cursor,
            trade_cursor,
            orders_indexed,
            trades_indexed,
        })
    }

    pub fn get_dex_orderbook_levels(
        &self,
        pair_id: u64,
        depth: usize,
    ) -> Result<(Vec<DexOrderbookLevel>, Vec<DexOrderbookLevel>), String> {
        if depth == 0 {
            return Ok((Vec::new(), Vec::new()));
        }
        let cf = self
            .db
            .cf_handle(CF_DEX_ORDERBOOK_LEVELS)
            .ok_or_else(|| "DEX orderbook-levels CF not found".to_string())?;

        let mut bid_prefix = Vec::with_capacity(9);
        bid_prefix.extend_from_slice(&pair_id.to_be_bytes());
        bid_prefix.push(DEX_SIDE_BUY);
        let mut bid_start = bid_prefix.clone();
        bid_start.extend_from_slice(&u64::MAX.to_be_bytes());

        let mut bids = Vec::with_capacity(depth.min(64));
        for item in self
            .db
            .iterator_cf(&cf, IteratorMode::From(&bid_start, Direction::Reverse))
        {
            let (key, value) = item.map_err(|e| format!("DEX orderbook iterator error: {}", e))?;
            if !key.starts_with(&bid_prefix) {
                break;
            }
            if key.len() < 17 {
                continue;
            }
            let price_raw = u64::from_be_bytes(key[9..17].try_into().unwrap_or([0; 8]));
            let (quantity, orders) = decode_orderbook_level_value(&value);
            if quantity > 0 && orders > 0 {
                bids.push(DexOrderbookLevel {
                    price_raw,
                    quantity,
                    orders,
                });
            }
            if bids.len() >= depth {
                break;
            }
        }

        let mut ask_prefix = Vec::with_capacity(9);
        ask_prefix.extend_from_slice(&pair_id.to_be_bytes());
        ask_prefix.push(DEX_SIDE_SELL);
        let mut ask_start = ask_prefix.clone();
        ask_start.extend_from_slice(&0u64.to_be_bytes());

        let mut asks = Vec::with_capacity(depth.min(64));
        for item in self
            .db
            .iterator_cf(&cf, IteratorMode::From(&ask_start, Direction::Forward))
        {
            let (key, value) = item.map_err(|e| format!("DEX orderbook iterator error: {}", e))?;
            if !key.starts_with(&ask_prefix) {
                break;
            }
            if key.len() < 17 {
                continue;
            }
            let price_raw = u64::from_be_bytes(key[9..17].try_into().unwrap_or([0; 8]));
            let (quantity, orders) = decode_orderbook_level_value(&value);
            if quantity > 0 && orders > 0 {
                asks.push(DexOrderbookLevel {
                    price_raw,
                    quantity,
                    orders,
                });
            }
            if asks.len() >= depth {
                break;
            }
        }

        Ok((bids, asks))
    }

    pub fn get_dex_pair_order_ids(&self, pair_id: u64, limit: usize) -> Result<Vec<u64>, String> {
        let mut prefix = Vec::with_capacity(16);
        prefix.extend_from_slice(&pair_id.to_be_bytes());
        collect_index_ids(
            &self.db,
            CF_DEX_ORDERS_BY_PAIR,
            &prefix,
            &prefix,
            8,
            limit,
            Direction::Forward,
        )
    }

    pub fn get_dex_pair_trade_ids(&self, pair_id: u64, limit: usize) -> Result<Vec<u64>, String> {
        let mut prefix = Vec::with_capacity(16);
        prefix.extend_from_slice(&pair_id.to_be_bytes());
        let mut start = prefix.clone();
        start.extend_from_slice(&u64::MAX.to_be_bytes());
        collect_index_ids(
            &self.db,
            CF_DEX_TRADES_BY_PAIR,
            &prefix,
            &start,
            8,
            limit,
            Direction::Reverse,
        )
    }

    pub fn get_dex_taker_trade_ids(
        &self,
        taker_hex: &str,
        limit: usize,
    ) -> Result<Vec<u64>, String> {
        let Some(taker) = taker_bytes(taker_hex) else {
            return Ok(Vec::new());
        };
        let mut start = Vec::with_capacity(40);
        start.extend_from_slice(&taker);
        start.extend_from_slice(&u64::MAX.to_be_bytes());
        collect_index_ids(
            &self.db,
            CF_DEX_TRADES_BY_TAKER,
            &taker,
            &start,
            32,
            limit,
            Direction::Reverse,
        )
    }

    pub fn get_dex_pair_taker_trade_ids(
        &self,
        pair_id: u64,
        taker_hex: &str,
        limit: usize,
    ) -> Result<Vec<u64>, String> {
        let Some(taker) = taker_bytes(taker_hex) else {
            return Ok(Vec::new());
        };
        let mut prefix = Vec::with_capacity(40);
        prefix.extend_from_slice(&pair_id.to_be_bytes());
        prefix.extend_from_slice(&taker);
        let mut start = Vec::with_capacity(48);
        start.extend_from_slice(&pair_id.to_be_bytes());
        start.extend_from_slice(&taker);
        start.extend_from_slice(&u64::MAX.to_be_bytes());
        collect_index_ids(
            &self.db,
            CF_DEX_TRADES_BY_PAIR_TAKER,
            &prefix,
            &start,
            40,
            limit,
            Direction::Reverse,
        )
    }
}

impl StateBatch {
    pub(super) fn stage_dex_contract_storage_indexes(
        &mut self,
        program: &Pubkey,
        storage_key: &[u8],
        old_data: Option<&[u8]>,
        new_data: Option<&[u8]>,
    ) -> Result<(), String> {
        if !StateStore::should_stage_dex_contract_storage_index(storage_key) {
            return Ok(());
        }
        if !is_dex_core_program(&self.db, program)? {
            return Ok(());
        }
        stage_dex_storage_index_mutation(
            &self.db,
            &mut self.batch,
            &mut self.dex_orderbook_level_deltas,
            storage_key,
            old_data,
            new_data,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[allow(clippy::too_many_arguments)]
    fn make_order_blob(
        pair_id: u64,
        side: u8,
        otype: u8,
        price: u64,
        qty: u64,
        filled: u64,
        status: u8,
        order_id: u64,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; 128];
        buf[0..32].copy_from_slice(&[0x11; 32]);
        buf[32..40].copy_from_slice(&pair_id.to_le_bytes());
        buf[40] = side;
        buf[41] = otype;
        buf[42..50].copy_from_slice(&price.to_le_bytes());
        buf[50..58].copy_from_slice(&qty.to_le_bytes());
        buf[58..66].copy_from_slice(&filled.to_le_bytes());
        buf[66] = status;
        buf[83..91].copy_from_slice(&order_id.to_le_bytes());
        buf
    }

    fn make_trade_blob(
        trade_id: u64,
        pair_id: u64,
        price: u64,
        qty: u64,
        taker: [u8; 32],
        maker_order_id: u64,
        slot: u64,
    ) -> Vec<u8> {
        let mut buf = vec![0u8; 80];
        buf[0..8].copy_from_slice(&trade_id.to_le_bytes());
        buf[8..16].copy_from_slice(&pair_id.to_le_bytes());
        buf[16..24].copy_from_slice(&price.to_le_bytes());
        buf[24..32].copy_from_slice(&qty.to_le_bytes());
        buf[32..64].copy_from_slice(&taker);
        buf[64..72].copy_from_slice(&maker_order_id.to_le_bytes());
        buf[72..80].copy_from_slice(&slot.to_le_bytes());
        buf
    }

    fn register_dex(state: &StateStore, program: Pubkey) {
        let entry = SymbolRegistryEntry {
            symbol: "DEX".to_string(),
            program,
            owner: Pubkey([0xAA; 32]),
            name: None,
            template: None,
            metadata: None,
            decimals: None,
        };
        state.register_symbol("DEX", entry).unwrap();
    }

    fn raw_put_contract_storage(
        state: &StateStore,
        program: &Pubkey,
        storage_key: &[u8],
        value: &[u8],
    ) {
        let cf = state.db.cf_handle(CF_CONTRACT_STORAGE).unwrap();
        let mut key = Vec::with_capacity(32 + storage_key.len());
        key.extend_from_slice(&program.0);
        key.extend_from_slice(storage_key);
        state.db.put_cf(&cf, key, value).unwrap();
    }

    #[test]
    fn dex_index_tracks_direct_contract_storage_writes() {
        let dir = tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();
        let dex = Pubkey([0x44; 32]);
        register_dex(&state, dex);

        let order = make_order_blob(7, DEX_SIDE_BUY, 0, 15, 1_000, 100, 1, 1);
        state
            .put_contract_storage(&dex, crate::dex::order_key(1).as_bytes(), &order)
            .unwrap();
        let trade = make_trade_blob(1, 7, 15, 20, [0x22; 32], 1, 99);
        state
            .put_contract_storage(&dex, crate::dex::trade_key(1).as_bytes(), &trade)
            .unwrap();

        assert_eq!(state.get_dex_pair_order_ids(7, 10).unwrap(), vec![1]);
        let (bids, asks) = state.get_dex_orderbook_levels(7, 10).unwrap();
        assert_eq!(asks, Vec::new());
        assert_eq!(
            bids,
            vec![DexOrderbookLevel {
                price_raw: 15,
                quantity: 900,
                orders: 1
            }]
        );
        assert_eq!(state.get_dex_pair_trade_ids(7, 10).unwrap(), vec![1]);
        assert_eq!(
            state
                .get_dex_taker_trade_ids(&hex::encode([0x22; 32]), 10)
                .unwrap(),
            vec![1]
        );
        assert_eq!(
            state
                .get_dex_pair_taker_trade_ids(7, &hex::encode([0x22; 32]), 10)
                .unwrap(),
            vec![1]
        );
    }

    #[test]
    fn dex_index_batches_multiple_updates_to_same_order() {
        let dir = tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();
        let dex = Pubkey([0x45; 32]);
        register_dex(&state, dex);

        let mut batch = state.begin_batch();
        let open = make_order_blob(9, DEX_SIDE_SELL, 0, 20, 1_000, 0, 0, 1);
        batch
            .put_contract_storage(&dex, crate::dex::order_key(1).as_bytes(), &open)
            .unwrap();
        let partial = make_order_blob(9, DEX_SIDE_SELL, 0, 20, 1_000, 400, 1, 1);
        batch
            .put_contract_storage(&dex, crate::dex::order_key(1).as_bytes(), &partial)
            .unwrap();
        state.commit_batch(batch).unwrap();

        let (bids, asks) = state.get_dex_orderbook_levels(9, 10).unwrap();
        assert!(bids.is_empty());
        assert_eq!(
            asks,
            vec![DexOrderbookLevel {
                price_raw: 20,
                quantity: 600,
                orders: 1
            }]
        );
    }

    #[test]
    fn dex_index_backfills_and_clears_on_counter_rewind() {
        let dir = tempdir().unwrap();
        let state = StateStore::open(dir.path()).unwrap();
        let dex = Pubkey([0x46; 32]);
        register_dex(&state, dex);

        raw_put_contract_storage(
            &state,
            &dex,
            crate::dex::DEX_ORDER_COUNT_KEY.as_bytes(),
            &2u64.to_le_bytes(),
        );
        raw_put_contract_storage(
            &state,
            &dex,
            crate::dex::order_key(1).as_bytes(),
            &make_order_blob(5, DEX_SIDE_BUY, 0, 10, 100, 0, 0, 1),
        );
        raw_put_contract_storage(
            &state,
            &dex,
            crate::dex::order_key(2).as_bytes(),
            &make_order_blob(5, DEX_SIDE_BUY, 0, 11, 200, 0, 0, 2),
        );
        raw_put_contract_storage(
            &state,
            &dex,
            crate::dex::DEX_TRADE_COUNT_KEY.as_bytes(),
            &2u64.to_le_bytes(),
        );
        raw_put_contract_storage(
            &state,
            &dex,
            crate::dex::trade_key(1).as_bytes(),
            &make_trade_blob(1, 5, 10, 1, [0x33; 32], 1, 10),
        );
        raw_put_contract_storage(
            &state,
            &dex,
            crate::dex::trade_key(2).as_bytes(),
            &make_trade_blob(2, 5, 11, 1, [0x33; 32], 2, 11),
        );

        let report = state.sync_dex_indexes_from_contract_storage().unwrap();
        assert!(report.schema_reset);
        assert_eq!(report.orders_indexed, 2);
        assert_eq!(report.trades_indexed, 2);
        assert_eq!(state.get_dex_pair_order_ids(5, 10).unwrap(), vec![1, 2]);
        assert_eq!(state.get_dex_pair_trade_ids(5, 10).unwrap(), vec![2, 1]);

        raw_put_contract_storage(
            &state,
            &dex,
            crate::dex::DEX_ORDER_COUNT_KEY.as_bytes(),
            &1u64.to_le_bytes(),
        );
        raw_put_contract_storage(
            &state,
            &dex,
            crate::dex::DEX_TRADE_COUNT_KEY.as_bytes(),
            &1u64.to_le_bytes(),
        );
        let report = state.sync_dex_indexes_from_contract_storage().unwrap();
        assert!(report.schema_reset);
        assert_eq!(state.get_dex_pair_order_ids(5, 10).unwrap(), vec![1]);
        assert_eq!(state.get_dex_pair_trade_ids(5, 10).unwrap(), vec![1]);
    }
}
