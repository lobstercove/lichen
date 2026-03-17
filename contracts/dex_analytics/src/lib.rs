// DEX Analytics — On-Chain OHLCV, Volume Tracking, Leaderboards (DEEP hardened)
//
// All trading pairs are denominated in mUSD (preferred quote currency).
// Prices and volumes are therefore expressed in mUSD units (6 decimals).
//
// Features:
//   - OHLCV candle aggregation (1m, 5m, 15m, 1h, 4h, 1d, 3d, 1w, 1y)
//   - 24h rolling stats per pair (volume, high, low, price change)
//   - Trader stats: volume, trade count, PnL
//   - Leaderboard tracking (top traders by volume/PnL)
//   - Price feed publication to MoltOracle
//   - Emergency pause, admin controls

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]
#![allow(dead_code)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::manual_is_multiple_of)]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    bytes_to_u64, get_caller, get_slot, log_info, storage_get, storage_set, u64_to_bytes,
};

// ============================================================================
// CONSTANTS
// ============================================================================

// Candle intervals in slots (~1 slot/sec)
const INTERVAL_1M: u64 = 60;
const INTERVAL_5M: u64 = 300;
const INTERVAL_15M: u64 = 900;
const INTERVAL_1H: u64 = 3_600;
const INTERVAL_4H: u64 = 14_400;
const INTERVAL_1D: u64 = 86_400;
const INTERVAL_3D: u64 = 259_200;
const INTERVAL_1W: u64 = 604_800;
const INTERVAL_1Y: u64 = 31_536_000;

// Max candles to keep per interval per pair (retention policy per milestone spec)
const MAX_CANDLES_1M: u64 = 1_440; // 24 hours
const MAX_CANDLES_5M: u64 = 2_016; // 7 days
const MAX_CANDLES_15M: u64 = 2_880; // 30 days
const MAX_CANDLES_1H: u64 = 2_160; // 90 days
const MAX_CANDLES_4H: u64 = 2_190; // 365 days
const MAX_CANDLES_1D: u64 = 1_095; // 3 years
const MAX_CANDLES_3D: u64 = 243; // 2 years
const MAX_CANDLES_1W: u64 = 260; // 5 years
const MAX_CANDLES_1Y: u64 = u64::MAX; // unlimited (forever)

const MAX_LEADERBOARD: u64 = 100;
const INTERVALS: [u64; 9] = [
    INTERVAL_1M,
    INTERVAL_5M,
    INTERVAL_15M,
    INTERVAL_1H,
    INTERVAL_4H,
    INTERVAL_1D,
    INTERVAL_3D,
    INTERVAL_1W,
    INTERVAL_1Y,
];

// Storage keys
const ADMIN_KEY: &[u8] = b"ana_admin";
const PAUSED_KEY: &[u8] = b"ana_paused";
const TRADE_RECORD_COUNT_KEY: &[u8] = b"ana_rec_count";
const TRADER_COUNT_KEY: &[u8] = b"ana_trader_count";
const TOTAL_VOLUME_KEY: &[u8] = b"ana_total_volume";
// F18.2: Authorized caller key — allows dex_core to call record_trade on behalf of traders
const AUTHORIZED_CALLER_KEY: &[u8] = b"ana_auth_caller";
// F18.9: Minimum volume for leaderboard entry (updated when board is full)
const LEADERBOARD_MIN_VOL_KEY: &[u8] = b"ana_lb_min_vol";
const LEADERBOARD_COUNT_KEY: &[u8] = b"ana_lb_count";

// ============================================================================
// HELPERS
// ============================================================================

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key)
        .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
        .unwrap_or(0)
}
fn save_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
}
fn load_addr(key: &[u8]) -> [u8; 32] {
    storage_get(key)
        .map(|d| {
            let mut a = [0u8; 32];
            if d.len() >= 32 {
                a.copy_from_slice(&d[..32]);
            }
            a
        })
        .unwrap_or([0u8; 32])
}
fn is_zero(addr: &[u8; 32]) -> bool {
    addr.iter().all(|&b| b == 0)
}

fn u64_to_decimal(mut n: u64) -> Vec<u8> {
    if n == 0 {
        return alloc::vec![b'0'];
    }
    let mut buf = Vec::new();
    while n > 0 {
        buf.push(b'0' + (n % 10) as u8);
        n /= 10;
    }
    buf.reverse();
    buf
}
fn hex_encode(bytes: &[u8]) -> Vec<u8> {
    let hex_chars: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(hex_chars[(b >> 4) as usize]);
        out.push(hex_chars[(b & 0x0f) as usize]);
    }
    out
}

fn require_admin(caller: &[u8; 32]) -> bool {
    let admin = load_addr(ADMIN_KEY);
    !is_zero(&admin) && *caller == admin
}
fn is_paused() -> bool {
    storage_get(PAUSED_KEY)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
}

// SECURITY: Reentrancy guard
const ANA_REENTRANCY_KEY: &[u8] = b"ana_reentrancy";
fn reentrancy_enter() -> bool {
    if let Some(v) = storage_get(ANA_REENTRANCY_KEY) {
        if !v.is_empty() && v[0] == 1 {
            return false;
        }
    }
    storage_set(ANA_REENTRANCY_KEY, &[1u8]);
    true
}
fn reentrancy_exit() {
    storage_set(ANA_REENTRANCY_KEY, &[0u8]);
}

/// Return the maximum number of candles retained for a given interval.
fn get_retention(interval: u64) -> u64 {
    match interval {
        INTERVAL_1M => MAX_CANDLES_1M,
        INTERVAL_5M => MAX_CANDLES_5M,
        INTERVAL_15M => MAX_CANDLES_15M,
        INTERVAL_1H => MAX_CANDLES_1H,
        INTERVAL_4H => MAX_CANDLES_4H,
        INTERVAL_1D => MAX_CANDLES_1D,
        INTERVAL_3D => MAX_CANDLES_3D,
        INTERVAL_1W => MAX_CANDLES_1W,
        INTERVAL_1Y => MAX_CANDLES_1Y,
        _ => 365, // safe default
    }
}

// Key helpers
fn candle_key(pair_id: u64, interval: u64, index: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"ana_c_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(interval));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(index));
    k
}
fn candle_count_key(pair_id: u64, interval: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"ana_cc_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(interval));
    k
}
fn candle_current_key(pair_id: u64, interval: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"ana_cur_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(interval));
    k
}
fn stats_24h_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"ana_24h_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k
}
fn trader_stats_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"ana_ts_"[..]);
    k.extend_from_slice(&hex_encode(addr));
    k
}
fn leaderboard_key(rank: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"ana_lb_"[..]);
    k.extend_from_slice(&u64_to_decimal(rank));
    k
}
fn last_price_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"ana_lp_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k
}

// ============================================================================
// CANDLE LAYOUT (48 bytes)
// ============================================================================
// Bytes 0..8    : open (u64)
// Bytes 8..16   : high (u64)
// Bytes 16..24  : low (u64)
// Bytes 24..32  : close (u64)
// Bytes 32..40  : volume (u64)
// Bytes 40..48  : timestamp/slot (u64)

const CANDLE_SIZE: usize = 48;

fn encode_candle(open: u64, high: u64, low: u64, close: u64, volume: u64, slot: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(CANDLE_SIZE);
    data.extend_from_slice(&u64_to_bytes(open));
    data.extend_from_slice(&u64_to_bytes(high));
    data.extend_from_slice(&u64_to_bytes(low));
    data.extend_from_slice(&u64_to_bytes(close));
    data.extend_from_slice(&u64_to_bytes(volume));
    data.extend_from_slice(&u64_to_bytes(slot));
    data
}

fn decode_candle_open(data: &[u8]) -> u64 {
    if data.len() >= 8 {
        bytes_to_u64(&data[0..8])
    } else {
        0
    }
}
fn decode_candle_high(data: &[u8]) -> u64 {
    if data.len() >= 16 {
        bytes_to_u64(&data[8..16])
    } else {
        0
    }
}
fn decode_candle_low(data: &[u8]) -> u64 {
    if data.len() >= 24 {
        bytes_to_u64(&data[16..24])
    } else {
        0
    }
}
fn decode_candle_close(data: &[u8]) -> u64 {
    if data.len() >= 32 {
        bytes_to_u64(&data[24..32])
    } else {
        0
    }
}
fn decode_candle_volume(data: &[u8]) -> u64 {
    if data.len() >= 40 {
        bytes_to_u64(&data[32..40])
    } else {
        0
    }
}
fn decode_candle_slot(data: &[u8]) -> u64 {
    if data.len() >= 48 {
        bytes_to_u64(&data[40..48])
    } else {
        0
    }
}

// ============================================================================
// 24H STATS LAYOUT (48 bytes)
// ============================================================================
// Bytes 0..8    : volume_24h (u64)
// Bytes 8..16   : high_24h (u64)
// Bytes 16..24  : low_24h (u64)
// Bytes 24..32  : open_24h (u64)  — price 24h ago
// Bytes 32..40  : close_24h (u64) — current price
// Bytes 40..48  : trade_count_24h (u64)

const STATS_SIZE: usize = 48;

fn encode_stats(volume: u64, high: u64, low: u64, open: u64, close: u64, trades: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(STATS_SIZE);
    data.extend_from_slice(&u64_to_bytes(volume));
    data.extend_from_slice(&u64_to_bytes(high));
    data.extend_from_slice(&u64_to_bytes(low));
    data.extend_from_slice(&u64_to_bytes(open));
    data.extend_from_slice(&u64_to_bytes(close));
    data.extend_from_slice(&u64_to_bytes(trades));
    data
}

fn decode_stats_volume(data: &[u8]) -> u64 {
    if data.len() >= 8 {
        bytes_to_u64(&data[0..8])
    } else {
        0
    }
}
fn decode_stats_high(data: &[u8]) -> u64 {
    if data.len() >= 16 {
        bytes_to_u64(&data[8..16])
    } else {
        0
    }
}
fn decode_stats_low(data: &[u8]) -> u64 {
    if data.len() >= 24 {
        bytes_to_u64(&data[16..24])
    } else {
        0
    }
}
fn decode_stats_trades(data: &[u8]) -> u64 {
    if data.len() >= 48 {
        bytes_to_u64(&data[40..48])
    } else {
        0
    }
}

// ============================================================================
// TRADER STATS LAYOUT (32 bytes)
// ============================================================================
// Bytes 0..8    : total_volume (u64)
// Bytes 8..16   : trade_count (u64)
// Bytes 16..24  : total_pnl (u64, biased — 2^63 = zero)
// Bytes 24..32  : last_trade_slot (u64)

const TRADER_STATS_SIZE: usize = 32;
const PNL_BIAS: u64 = 1u64 << 63;

fn encode_trader_stats(volume: u64, trades: u64, pnl: u64, last_slot: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(TRADER_STATS_SIZE);
    data.extend_from_slice(&u64_to_bytes(volume));
    data.extend_from_slice(&u64_to_bytes(trades));
    data.extend_from_slice(&u64_to_bytes(pnl));
    data.extend_from_slice(&u64_to_bytes(last_slot));
    data
}

fn decode_ts_volume(data: &[u8]) -> u64 {
    if data.len() >= 8 {
        bytes_to_u64(&data[0..8])
    } else {
        0
    }
}
fn decode_ts_trades(data: &[u8]) -> u64 {
    if data.len() >= 16 {
        bytes_to_u64(&data[8..16])
    } else {
        0
    }
}
fn decode_ts_pnl(data: &[u8]) -> u64 {
    if data.len() >= 24 {
        bytes_to_u64(&data[16..24])
    } else {
        PNL_BIAS
    }
}

// ============================================================================
// PUBLIC FUNCTIONS
// ============================================================================

#[no_mangle]
pub extern "C" fn initialize(admin: *const u8) -> u32 {
    let existing = load_addr(ADMIN_KEY);
    if !is_zero(&existing) {
        return 1;
    }
    let mut addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(admin, addr.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != addr {
        return 200;
    }
    storage_set(ADMIN_KEY, &addr);
    save_u64(TRADE_RECORD_COUNT_KEY, 0);
    storage_set(PAUSED_KEY, &[0u8]);
    log_info("DEX Analytics initialized");
    0
}

/// Record a trade (called by dex_core after settlement, or directly by trader)
/// Returns: 0=success
pub fn record_trade(pair_id: u64, price: u64, volume: u64, trader: *const u8) -> u32 {
    // SECURITY-FIX: Check pause state before recording
    if is_paused() {
        return 2;
    }
    if !reentrancy_enter() {
        return 3;
    }
    if price == 0 || volume == 0 {
        reentrancy_exit();
        return 1;
    }
    let current_slot = get_slot();

    let mut t = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(trader, t.as_mut_ptr(), 32);
    }
    // F18.2: Accept calls from either the trader themselves OR an authorized contract (dex_core)
    let real_caller = get_caller();
    let authorized = load_addr(AUTHORIZED_CALLER_KEY);
    if real_caller.0 != t && (is_zero(&authorized) || real_caller.0 != authorized) {
        reentrancy_exit();
        return 200;
    }

    // Update last price
    save_u64(&last_price_key(pair_id), price);

    // Update 24h stats
    update_24h_stats(pair_id, price, volume);

    // Update candles for all intervals
    for &interval in &INTERVALS {
        update_candle(pair_id, interval, price, volume, current_slot);
    }

    // Update trader stats
    update_trader_stats(&t, volume, current_slot);

    // Increment record count
    let count = load_u64(TRADE_RECORD_COUNT_KEY);
    save_u64(TRADE_RECORD_COUNT_KEY, count + 1);

    reentrancy_exit();
    0
}

/// F18.10: Record realized PnL for a trader (called after margin position close/liquidation)
/// pnl_biased: PNL_BIAS + signed_pnl. Value > PNL_BIAS means profit, < PNL_BIAS means loss.
pub fn record_pnl(trader: *const u8, pnl_biased: u64) -> u32 {
    if is_paused() {
        return 2;
    }
    if !reentrancy_enter() {
        return 3;
    }

    let mut t = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(trader, t.as_mut_ptr(), 32);
    }
    // Accept calls from trader themselves or authorized caller (dex_margin)
    let real_caller = get_caller();
    let authorized = load_addr(AUTHORIZED_CALLER_KEY);
    if real_caller.0 != t && (is_zero(&authorized) || real_caller.0 != authorized) {
        reentrancy_exit();
        return 200;
    }

    let tk = trader_stats_key(&t);
    let (vol, trades, current_pnl, last_slot) = match storage_get(&tk) {
        Some(d) if d.len() >= TRADER_STATS_SIZE => (
            decode_ts_volume(&d),
            decode_ts_trades(&d),
            decode_ts_pnl(&d),
            if d.len() >= 32 {
                bytes_to_u64(&d[24..32])
            } else {
                0
            },
        ),
        _ => (0, 0, PNL_BIAS, 0),
    };

    // Apply PnL delta: both are biased values, so subtract one PNL_BIAS to stay in biased space
    // new_pnl = current_pnl + (pnl_biased - PNL_BIAS)
    let pnl_delta_signed = (pnl_biased as i128) - (PNL_BIAS as i128);
    let new_pnl = ((current_pnl as i128) + pnl_delta_signed) as u64;

    let stats = encode_trader_stats(vol, trades, new_pnl, last_slot);
    storage_set(&tk, &stats);

    reentrancy_exit();
    0
}

fn update_24h_stats(pair_id: u64, price: u64, volume: u64) {
    let sk = stats_24h_key(pair_id);
    let (mut vol, mut high, mut low, open, _close, mut trades) = match storage_get(&sk) {
        Some(d) if d.len() >= STATS_SIZE => (
            decode_stats_volume(&d),
            decode_stats_high(&d),
            decode_stats_low(&d),
            bytes_to_u64(&d[24..32]),
            bytes_to_u64(&d[32..40]),
            decode_stats_trades(&d),
        ),
        _ => (0, 0, u64::MAX, price, price, 0), // first trade
    };

    vol += volume;
    if price > high {
        high = price;
    }
    if price < low {
        low = price;
    }
    trades += 1;

    let stats = encode_stats(vol, high, low, open, price, trades);
    storage_set(&sk, &stats);
}

fn update_candle(pair_id: u64, interval: u64, price: u64, volume: u64, current_slot: u64) {
    let cur_key = candle_current_key(pair_id, interval);
    let candle_start_slot = (current_slot / interval) * interval;

    // Check if we're in a new candle period
    let stored_start = load_u64(&cur_key);
    if stored_start == candle_start_slot {
        // Update existing candle
        let count = load_u64(&candle_count_key(pair_id, interval));
        if count == 0 {
            return;
        }
        let ck = candle_key(pair_id, interval, count);
        if let Some(mut data) = storage_get(&ck) {
            if data.len() >= CANDLE_SIZE {
                let high = decode_candle_high(&data);
                let low = decode_candle_low(&data);
                let vol = decode_candle_volume(&data);

                if price > high {
                    data[8..16].copy_from_slice(&u64_to_bytes(price));
                }
                if price < low {
                    data[16..24].copy_from_slice(&u64_to_bytes(price));
                }
                data[24..32].copy_from_slice(&u64_to_bytes(price)); // close
                data[32..40].copy_from_slice(&u64_to_bytes(vol + volume));
                storage_set(&ck, &data);
            }
        }
    } else {
        // New candle
        save_u64(&cur_key, candle_start_slot);
        let count = load_u64(&candle_count_key(pair_id, interval));
        let new_count = count + 1;
        // F18.3: Enforce candle retention via modular indexing
        let max_candles = get_retention(interval);
        let write_idx = if max_candles > 0 && max_candles != u64::MAX {
            ((new_count - 1) % max_candles) + 1
        } else {
            new_count
        };
        let candle = encode_candle(price, price, price, price, volume, current_slot);
        storage_set(&candle_key(pair_id, interval, write_idx), &candle);
        save_u64(&candle_count_key(pair_id, interval), new_count);
    }
}

fn update_trader_stats(trader: &[u8; 32], volume: u64, slot: u64) {
    let tk = trader_stats_key(trader);
    let (vol, trades, pnl) = match storage_get(&tk) {
        Some(d) if d.len() >= TRADER_STATS_SIZE => (
            decode_ts_volume(&d),
            decode_ts_trades(&d),
            decode_ts_pnl(&d),
        ),
        _ => {
            // First trade for this trader — increment unique trader count
            save_u64(TRADER_COUNT_KEY, load_u64(TRADER_COUNT_KEY) + 1);
            (0, 0, PNL_BIAS)
        }
    };
    let new_volume = vol + volume;
    // Track global cumulative volume
    save_u64(
        TOTAL_VOLUME_KEY,
        load_u64(TOTAL_VOLUME_KEY).saturating_add(volume),
    );
    let stats = encode_trader_stats(new_volume, trades + 1, pnl, slot);
    storage_set(&tk, &stats);

    // F18.9: Leaderboard maintenance — insert/promote trader if volume qualifies
    update_leaderboard(trader, new_volume);
}

/// F18.9: Leaderboard insertion logic
/// Maintains a sorted top-100 leaderboard by trading volume.
/// Each entry is a 32-byte trader address stored at `ana_lb_{rank}`.
fn update_leaderboard(trader: &[u8; 32], new_volume: u64) {
    let lb_count = load_u64(LEADERBOARD_COUNT_KEY);
    let min_vol = load_u64(LEADERBOARD_MIN_VOL_KEY);

    // Skip expensive scan if leaderboard is full and trader volume is below minimum
    if lb_count >= MAX_LEADERBOARD && new_volume <= min_vol {
        return;
    }

    // Scan leaderboard to find trader's current position (if any) and insertion point
    let mut existing_rank: Option<u64> = None;
    let mut insert_rank: u64 = lb_count; // default: end of list

    for rank in 0..lb_count {
        let key = leaderboard_key(rank);
        if let Some(addr_data) = storage_get(&key) {
            if addr_data.len() >= 32 {
                let mut addr = [0u8; 32];
                addr.copy_from_slice(&addr_data[..32]);
                if addr == *trader {
                    existing_rank = Some(rank);
                    continue;
                }
                // Look up this entry's volume from trader stats
                let entry_vol = match storage_get(&trader_stats_key(&addr)) {
                    Some(d) if d.len() >= 8 => decode_ts_volume(&d),
                    _ => 0,
                };
                if insert_rank == lb_count && new_volume > entry_vol {
                    insert_rank = rank;
                }
            }
        }
    }

    // If trader already exists on the leaderboard at a better position, nothing to do
    if let Some(er) = existing_rank {
        if er <= insert_rank {
            return; // already at or above the insertion point
        }
        // Remove from old position by shifting entries up
        for r in er..lb_count.saturating_sub(1) {
            let next_data = storage_get(&leaderboard_key(r + 1)).unwrap_or_default();
            storage_set(&leaderboard_key(r), &next_data);
        }
        let new_count = lb_count - 1;
        save_u64(LEADERBOARD_COUNT_KEY, new_count);
        // Adjust insert rank if it was after the removed position
        if insert_rank > er {
            insert_rank -= 1;
        }
        // Re-insert below
        let effective_count = new_count;
        // Shift entries down from insert_rank
        let final_count = if effective_count < MAX_LEADERBOARD {
            effective_count + 1
        } else {
            MAX_LEADERBOARD
        };
        let mut r = final_count.saturating_sub(1);
        while r > insert_rank {
            let prev_data = storage_get(&leaderboard_key(r - 1)).unwrap_or_default();
            storage_set(&leaderboard_key(r), &prev_data);
            r -= 1;
        }
        storage_set(&leaderboard_key(insert_rank), trader);
        save_u64(LEADERBOARD_COUNT_KEY, final_count);
    } else if lb_count < MAX_LEADERBOARD || insert_rank < lb_count {
        // New entry: shift down and insert
        let final_count = if lb_count < MAX_LEADERBOARD {
            lb_count + 1
        } else {
            MAX_LEADERBOARD
        };
        let mut r = final_count.saturating_sub(1);
        while r > insert_rank {
            let prev_data = storage_get(&leaderboard_key(r - 1)).unwrap_or_default();
            storage_set(&leaderboard_key(r), &prev_data);
            r -= 1;
        }
        storage_set(&leaderboard_key(insert_rank), trader);
        save_u64(LEADERBOARD_COUNT_KEY, final_count);
    }

    // Update minimum volume (entry at last rank)
    let final_count = load_u64(LEADERBOARD_COUNT_KEY);
    if final_count > 0 {
        if let Some(last_addr) = storage_get(&leaderboard_key(final_count - 1)) {
            if last_addr.len() >= 32 {
                let mut addr = [0u8; 32];
                addr.copy_from_slice(&last_addr[..32]);
                let last_vol = match storage_get(&trader_stats_key(&addr)) {
                    Some(d) if d.len() >= 8 => decode_ts_volume(&d),
                    _ => 0,
                };
                save_u64(LEADERBOARD_MIN_VOL_KEY, last_vol);
            }
        }
    }
}

// ============================================================================
// QUERY FUNCTIONS
// ============================================================================

/// Get OHLCV candles for a pair
pub fn get_ohlcv(pair_id: u64, interval: u64, count: u64) -> u64 {
    let total = load_u64(&candle_count_key(pair_id, interval));
    if total == 0 {
        return 0;
    }

    let start = if count >= total { 1 } else { total - count + 1 };
    let mut result = Vec::new();
    for i in start..=total {
        let ck = candle_key(pair_id, interval, i);
        if let Some(d) = storage_get(&ck) {
            result.extend_from_slice(&d);
        }
    }
    if !result.is_empty() {
        moltchain_sdk::set_return_data(&result);
    }
    total.min(count)
}

/// Get 24h stats for a pair
pub fn get_24h_stats(pair_id: u64) -> u64 {
    let sk = stats_24h_key(pair_id);
    match storage_get(&sk) {
        Some(d) if d.len() >= STATS_SIZE => {
            moltchain_sdk::set_return_data(&d);
            1
        }
        _ => 0,
    }
}

/// Get trader stats
pub fn get_trader_stats(trader: *const u8) -> u64 {
    let mut t = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(trader, t.as_mut_ptr(), 32);
    }
    let tk = trader_stats_key(&t);
    match storage_get(&tk) {
        Some(d) if d.len() >= TRADER_STATS_SIZE => {
            moltchain_sdk::set_return_data(&d);
            1
        }
        _ => 0,
    }
}

/// Get last price for a pair
pub fn get_last_price(pair_id: u64) -> u64 {
    load_u64(&last_price_key(pair_id))
}

/// Get total recorded trades
pub fn get_record_count() -> u64 {
    load_u64(TRADE_RECORD_COUNT_KEY)
}

pub fn emergency_pause(caller: *const u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    storage_set(PAUSED_KEY, &[1u8]);
    log_info("DEX Analytics: EMERGENCY PAUSE");
    0
}
pub fn emergency_unpause(caller: *const u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    storage_set(PAUSED_KEY, &[0u8]);
    0
}

/// F18.2: Set authorized caller (dex_core contract address) — admin only
/// Allows dex_core to call record_trade on behalf of traders
pub fn set_authorized_caller(caller: *const u8, authorized: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut a = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(authorized, a.as_mut_ptr(), 32);
    }
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    storage_set(AUTHORIZED_CALLER_KEY, &a);
    log_info("DEX Analytics: authorized caller set");
    0
}

// WASM entry
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn call() {
    let args = moltchain_sdk::get_args();
    if args.is_empty() {
        return;
    }
    match args[0] {
        // 0 = initialize(admin[32])
        0 => {
            if args.len() >= 33 {
                let r = initialize(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 1 = record_trade(pair_id[8], price[8], volume[8], trader[32])
        1 => {
            if args.len() >= 57 {
                let pair_id = bytes_to_u64(&args[1..9]);
                let price = bytes_to_u64(&args[9..17]);
                let volume = bytes_to_u64(&args[17..25]);
                let r = record_trade(pair_id, price, volume, args[25..57].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 2 = get_ohlcv(pair_id[8], interval[8], count[8])
        2 => {
            if args.len() >= 25 {
                let pair_id = bytes_to_u64(&args[1..9]);
                let interval = bytes_to_u64(&args[9..17]);
                let count = bytes_to_u64(&args[17..25]);
                let n = get_ohlcv(pair_id, interval, count);
                moltchain_sdk::set_return_data(&u64_to_bytes(n));
            }
        }
        // 3 = get_24h_stats(pair_id[8])
        3 => {
            if args.len() >= 9 {
                let pair_id = bytes_to_u64(&args[1..9]);
                let r = get_24h_stats(pair_id);
                if r == 0 {
                    moltchain_sdk::set_return_data(&u64_to_bytes(0));
                }
            }
        }
        // 4 = get_trader_stats(addr[32])
        4 => {
            if args.len() >= 33 {
                let r = get_trader_stats(args[1..33].as_ptr());
                if r == 0 {
                    moltchain_sdk::set_return_data(&u64_to_bytes(0));
                }
            }
        }
        // 5 = get_last_price(pair_id[8])
        5 => {
            if args.len() >= 9 {
                let pair_id = bytes_to_u64(&args[1..9]);
                let p = get_last_price(pair_id);
                moltchain_sdk::set_return_data(&u64_to_bytes(p));
            }
        }
        // 6 = get_record_count()
        6 => {
            let c = get_record_count();
            moltchain_sdk::set_return_data(&u64_to_bytes(c));
        }
        // 7 = emergency_pause(caller[32])
        7 => {
            if args.len() >= 33 {
                let r = emergency_pause(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 8 = emergency_unpause(caller[32])
        8 => {
            if args.len() >= 33 {
                let r = emergency_unpause(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        9 => {
            // get_trader_count — unique traders seen by analytics
            moltchain_sdk::set_return_data(&u64_to_bytes(load_u64(TRADER_COUNT_KEY)));
        }
        10 => {
            // get_global_stats — [record_count, trader_count, total_volume]
            let mut buf = Vec::with_capacity(24);
            buf.extend_from_slice(&u64_to_bytes(load_u64(TRADE_RECORD_COUNT_KEY)));
            buf.extend_from_slice(&u64_to_bytes(load_u64(TRADER_COUNT_KEY)));
            buf.extend_from_slice(&u64_to_bytes(load_u64(TOTAL_VOLUME_KEY)));
            moltchain_sdk::set_return_data(&buf);
        }
        // 11 = F18.2: set_authorized_caller(caller[32], authorized[32])
        11 => {
            if args.len() >= 65 {
                let r = set_authorized_caller(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 12 = F18.10: record_pnl(trader[32], pnl_biased[8])
        12 => {
            if args.len() >= 41 {
                let pnl_biased = bytes_to_u64(&args[33..41]);
                let r = record_pnl(args[1..33].as_ptr(), pnl_biased);
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        _ => {
            moltchain_sdk::set_return_data(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        }
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use moltchain_sdk::test_mock;

    fn setup() -> [u8; 32] {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        admin
    }

    #[test]
    fn test_initialize() {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
    }

    #[test]
    fn test_initialize_twice() {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(initialize(admin.as_ptr()), 1);
    }

    #[test]
    fn test_record_trade() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(1000);
        assert_eq!(record_trade(1, 1_000_000_000, 5_000, trader.as_ptr()), 0);
        assert_eq!(get_record_count(), 1);
    }

    #[test]
    fn test_record_trade_zero_values() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        assert_eq!(record_trade(1, 0, 5_000, trader.as_ptr()), 1);
        assert_eq!(record_trade(1, 1_000, 0, trader.as_ptr()), 1);
    }

    #[test]
    fn test_last_price_updated() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(1000);
        record_trade(1, 1_500_000_000, 5_000, trader.as_ptr());
        assert_eq!(get_last_price(1), 1_500_000_000);
    }

    #[test]
    fn test_24h_stats_single_trade() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(1000);
        record_trade(1, 1_000_000_000, 5_000, trader.as_ptr());

        let sk = stats_24h_key(1);
        let data = storage_get(&sk).unwrap();
        assert_eq!(decode_stats_volume(&data), 5_000);
        assert_eq!(decode_stats_high(&data), 1_000_000_000);
        assert_eq!(decode_stats_low(&data), 1_000_000_000);
        assert_eq!(decode_stats_trades(&data), 1);
    }

    #[test]
    fn test_24h_stats_multiple_trades() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(1000);
        record_trade(1, 1_000_000_000, 3_000, trader.as_ptr());
        record_trade(1, 1_500_000_000, 2_000, trader.as_ptr());
        record_trade(1, 800_000_000, 5_000, trader.as_ptr());

        let sk = stats_24h_key(1);
        let data = storage_get(&sk).unwrap();
        assert_eq!(decode_stats_volume(&data), 10_000);
        assert_eq!(decode_stats_high(&data), 1_500_000_000);
        assert_eq!(decode_stats_low(&data), 800_000_000);
        assert_eq!(decode_stats_trades(&data), 3);
    }

    #[test]
    fn test_candle_creation() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(60); // Start of a 1-min candle
        record_trade(1, 1_000_000_000, 5_000, trader.as_ptr());

        let count = load_u64(&candle_count_key(1, INTERVAL_1M));
        assert_eq!(count, 1);

        let ck = candle_key(1, INTERVAL_1M, 1);
        let data = storage_get(&ck).unwrap();
        assert_eq!(decode_candle_open(&data), 1_000_000_000);
        assert_eq!(decode_candle_close(&data), 1_000_000_000);
        assert_eq!(decode_candle_volume(&data), 5_000);
    }

    #[test]
    fn test_candle_update_same_period() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(60);
        record_trade(1, 1_000_000_000, 3_000, trader.as_ptr());
        test_mock::set_slot(90); // Same 1-min candle
        record_trade(1, 1_200_000_000, 2_000, trader.as_ptr());

        let count = load_u64(&candle_count_key(1, INTERVAL_1M));
        assert_eq!(count, 1); // Still one candle

        let ck = candle_key(1, INTERVAL_1M, 1);
        let data = storage_get(&ck).unwrap();
        assert_eq!(decode_candle_open(&data), 1_000_000_000);
        assert_eq!(decode_candle_high(&data), 1_200_000_000);
        assert_eq!(decode_candle_close(&data), 1_200_000_000);
        assert_eq!(decode_candle_volume(&data), 5_000);
    }

    #[test]
    fn test_candle_new_period() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(60);
        record_trade(1, 1_000_000_000, 3_000, trader.as_ptr());
        test_mock::set_slot(120); // New 1-min candle
        record_trade(1, 1_100_000_000, 4_000, trader.as_ptr());

        let count = load_u64(&candle_count_key(1, INTERVAL_1M));
        assert_eq!(count, 2); // Two candles now
    }

    #[test]
    fn test_trader_stats() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(1000);
        record_trade(1, 1_000_000_000, 5_000, trader.as_ptr());
        record_trade(1, 1_100_000_000, 3_000, trader.as_ptr());

        let tk = trader_stats_key(&trader);
        let data = storage_get(&tk).unwrap();
        assert_eq!(decode_ts_volume(&data), 8_000);
        assert_eq!(decode_ts_trades(&data), 2);
    }

    #[test]
    fn test_get_ohlcv() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(60);
        record_trade(1, 1_000_000_000, 5_000, trader.as_ptr());
        test_mock::set_slot(120);
        record_trade(1, 1_100_000_000, 3_000, trader.as_ptr());

        let count = get_ohlcv(1, INTERVAL_1M, 10);
        assert_eq!(count, 2);
    }

    #[test]
    fn test_get_24h_stats() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(1000);
        record_trade(1, 1_000_000_000, 5_000, trader.as_ptr());
        assert_eq!(get_24h_stats(1), 1);
        assert_eq!(get_24h_stats(999), 0); // no data
    }

    #[test]
    fn test_get_trader_stats() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(1000);
        record_trade(1, 1_000_000_000, 5_000, trader.as_ptr());
        assert_eq!(get_trader_stats(trader.as_ptr()), 1);

        let other = [3u8; 32];
        assert_eq!(get_trader_stats(other.as_ptr()), 0);
    }

    #[test]
    fn test_emergency_pause() {
        let admin = setup();
        assert_eq!(emergency_pause(admin.as_ptr()), 0);
        assert!(is_paused());
        assert_eq!(emergency_unpause(admin.as_ptr()), 0);
        assert!(!is_paused());
    }

    #[test]
    fn test_emergency_pause_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        test_mock::set_caller(rando);
        assert_eq!(emergency_pause(rando.as_ptr()), 1);
    }

    #[test]
    fn test_intervals_count() {
        // Verify we have exactly 9 intervals
        assert_eq!(INTERVALS.len(), 9);
        assert_eq!(INTERVALS[0], INTERVAL_1M);
        assert_eq!(INTERVALS[6], INTERVAL_3D);
        assert_eq!(INTERVALS[7], INTERVAL_1W);
        assert_eq!(INTERVALS[8], INTERVAL_1Y);
    }

    #[test]
    fn test_get_retention() {
        assert_eq!(get_retention(INTERVAL_1M), 1_440);
        assert_eq!(get_retention(INTERVAL_5M), 2_016);
        assert_eq!(get_retention(INTERVAL_15M), 2_880);
        assert_eq!(get_retention(INTERVAL_1H), 2_160);
        assert_eq!(get_retention(INTERVAL_4H), 2_190);
        assert_eq!(get_retention(INTERVAL_1D), 1_095);
        assert_eq!(get_retention(INTERVAL_3D), 243);
        assert_eq!(get_retention(INTERVAL_1W), 260);
        assert_eq!(get_retention(INTERVAL_1Y), u64::MAX);
        // Unknown interval defaults to 365
        assert_eq!(get_retention(999), 365);
    }

    #[test]
    fn test_candle_3d_creation() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        // 3d = 259_200 slots; place trade at start of a 3d bucket
        test_mock::set_slot(259_200);
        record_trade(1, 2_000_000_000, 10_000, trader.as_ptr());

        let count = load_u64(&candle_count_key(1, INTERVAL_3D));
        assert_eq!(count, 1);

        let ck = candle_key(1, INTERVAL_3D, 1);
        let data = storage_get(&ck).unwrap();
        assert_eq!(decode_candle_open(&data), 2_000_000_000);
        assert_eq!(decode_candle_close(&data), 2_000_000_000);
        assert_eq!(decode_candle_volume(&data), 10_000);
    }

    #[test]
    fn test_candle_1w_creation() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        // 1w = 604_800 slots
        test_mock::set_slot(604_800);
        record_trade(1, 3_000_000_000, 7_500, trader.as_ptr());

        let count = load_u64(&candle_count_key(1, INTERVAL_1W));
        assert_eq!(count, 1);

        let ck = candle_key(1, INTERVAL_1W, 1);
        let data = storage_get(&ck).unwrap();
        assert_eq!(decode_candle_open(&data), 3_000_000_000);
        assert_eq!(decode_candle_volume(&data), 7_500);
    }

    #[test]
    fn test_candle_1y_creation() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        // 1y = 31_536_000 slots
        test_mock::set_slot(31_536_000);
        record_trade(1, 5_000_000_000, 50_000, trader.as_ptr());

        let count = load_u64(&candle_count_key(1, INTERVAL_1Y));
        assert_eq!(count, 1);

        let ck = candle_key(1, INTERVAL_1Y, 1);
        let data = storage_get(&ck).unwrap();
        assert_eq!(decode_candle_open(&data), 5_000_000_000);
        assert_eq!(decode_candle_volume(&data), 50_000);
    }

    #[test]
    fn test_candle_3d_update_same_period() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(259_200);
        record_trade(1, 1_000_000_000, 5_000, trader.as_ptr());
        test_mock::set_slot(259_200 + 100_000); // still in same 3d bucket
        record_trade(1, 1_500_000_000, 3_000, trader.as_ptr());

        let count = load_u64(&candle_count_key(1, INTERVAL_3D));
        assert_eq!(count, 1); // still one candle

        let ck = candle_key(1, INTERVAL_3D, 1);
        let data = storage_get(&ck).unwrap();
        assert_eq!(decode_candle_open(&data), 1_000_000_000);
        assert_eq!(decode_candle_high(&data), 1_500_000_000);
        assert_eq!(decode_candle_close(&data), 1_500_000_000);
        assert_eq!(decode_candle_volume(&data), 8_000);
    }

    #[test]
    fn test_candle_1w_new_period() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(604_800);
        record_trade(1, 1_000_000_000, 5_000, trader.as_ptr());
        test_mock::set_slot(604_800 * 2); // next week
        record_trade(1, 1_100_000_000, 4_000, trader.as_ptr());

        let count = load_u64(&candle_count_key(1, INTERVAL_1W));
        assert_eq!(count, 2); // two candles
    }

    #[test]
    fn test_get_ohlcv_3d() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(259_200);
        record_trade(1, 1_000_000_000, 5_000, trader.as_ptr());
        test_mock::set_slot(259_200 * 2);
        record_trade(1, 1_100_000_000, 3_000, trader.as_ptr());

        let count = get_ohlcv(1, INTERVAL_3D, 10);
        assert_eq!(count, 2);
    }

    #[test]
    fn test_all_intervals_get_candles_on_trade() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        // Use slot that's a multiple of all intervals (lcm-like)
        test_mock::set_slot(31_536_000); // multiple of all intervals
        record_trade(1, 1_000_000_000, 5_000, trader.as_ptr());

        // Every interval should have at least 1 candle
        for &interval in &INTERVALS {
            let count = load_u64(&candle_count_key(1, interval));
            assert!(count >= 1, "Interval {} should have candles", interval);
        }
    }
}
