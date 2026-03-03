// DEX Core — Central Limit Order Book + Matching Engine + Settlement (DEEP hardened)
//
// Features:
//   - On-chain CLOB with price-time priority matching
//   - Limit, market, stop-limit, post-only order types
//   - Reduce-only flag (0x80) — validates against margin positions
//   - Self-trade prevention (cancel-oldest)
//   - Trading pair management with configurable fees
//   - Maker rebates (-1 bps), taker fees (5 bps default)
//   - Emergency pause, reentrancy guard, admin controls
//   - Order expiry enforcement, dust order filtering
//   - MoltyID integration for identity verification
//
// Storage layout:
//   dex_admin                              → [u8; 32]
//   dex_paused                             → u8 (0=active, 1=paused)
//   dex_reentrancy                         → u8
//   dex_pair_count                         → u64
//   dex_pair_{id}                          → TradingPair (112 bytes)
//   dex_order_count                        → u64
//   dex_order_{id}                         → Order (128 bytes)
//   dex_book_bid_{pair}_{price}_{order}    → u64 (order_id)
//   dex_book_ask_{pair}_{price}_{order}    → u64 (order_id)
//   dex_best_bid_{pair}                    → u64 (price)
//   dex_best_ask_{pair}                    → u64 (price)
//   dex_user_orders_{addr}_{order}         → u64 (order_id)
//   dex_user_order_count_{addr}            → u64
//   dex_trade_count                        → u64
//   dex_trade_{id}                         → Trade (80 bytes)
//   dex_fee_treasury                       → u64 (accumulated protocol fees)
//   dex_moltyid_address                    → [u8; 32]

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]
#![allow(dead_code)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::manual_is_multiple_of)]
#![allow(unused_variables)]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{bytes_to_u64, get_caller, get_slot, log_info, storage_get, storage_set, u64_to_bytes,
    Address, CrossCall, call_contract};

// ============================================================================
// CONSTANTS
// ============================================================================

const MAX_PAIRS: u64 = 50;
const MAX_ORDER_SIZE: u64 = 10_000_000_000_000_000; // 10M MOLT max order ($1M at $0.10)
const MIN_ORDER_VALUE: u64 = 1_000; // minimum 1000 shells
const MAX_OPEN_ORDERS_PER_USER: u64 = 100;
const DEFAULT_MAKER_FEE_BPS: i64 = -1; // rebate
const DEFAULT_TAKER_FEE_BPS: u64 = 5; // 0.05%
const MAX_FEE_BPS: u64 = 100; // 1% max
const FEE_PROTOCOL_SHARE: u64 = 60; // 60% to protocol
const FEE_LP_SHARE: u64 = 20; // 20% to LPs
const FEE_STAKER_SHARE: u64 = 20; // 20% to stakers
const MIN_FEE_PER_TRADE: u64 = 1; // 1 shell minimum
const ORDER_EXPIRY_MAX: u64 = 2_592_000; // ~30 days in slots
// F18.2: Analytics cross-contract call — record trades after settlement
const ANALYTICS_ADDRESS_KEY: &str = "dex_analytics_addr";
// G2-04: Margin contract address for reduce-only cross-contract validation
const MARGIN_ADDRESS_KEY: &str = "dex_margin_addr";
// F18.7: Daily volume reset tracking (slot-based day boundary)
const SLOTS_PER_DAY: u64 = 216_000; // 24h * 3600s / 0.4s

// Order sides
const SIDE_BUY: u8 = 0;
const SIDE_SELL: u8 = 1;

// Order types
const ORDER_LIMIT: u8 = 0;
const ORDER_MARKET: u8 = 1;
const ORDER_STOP_LIMIT: u8 = 2;
const ORDER_POST_ONLY: u8 = 3;

// Reduce-only flag — OR'd with base order type (e.g. ORDER_LIMIT | REDUCE_ONLY_FLAG)
// Indicates the order should only reduce an existing margin position, not open one.
const REDUCE_ONLY_FLAG: u8 = 0x80;

// Order status
const STATUS_OPEN: u8 = 0;
const STATUS_PARTIAL: u8 = 1;
const STATUS_FILLED: u8 = 2;
const STATUS_CANCELLED: u8 = 3;
const STATUS_EXPIRED: u8 = 4;
const STATUS_DORMANT: u8 = 5; // Task 2.2: Stop-limit waiting for trigger

// Pair status
const PAIR_ACTIVE: u8 = 0;
const PAIR_PAUSED: u8 = 1;
const PAIR_DELISTED: u8 = 2;

// Storage keys
const ADMIN_KEY: &[u8] = b"dex_admin";
const PAUSED_KEY: &[u8] = b"dex_paused";
const REENTRANCY_KEY: &[u8] = b"dex_reentrancy";
const PAIR_COUNT_KEY: &[u8] = b"dex_pair_count";
const ORDER_COUNT_KEY: &[u8] = b"dex_order_count";
const TRADE_COUNT_KEY: &[u8] = b"dex_trade_count";
const FEE_TREASURY_KEY: &[u8] = b"dex_fee_treasury";
const FEE_TREASURY_ADDR_KEY: &[u8] = b"dex_fee_treasury_addr";
const PREFERRED_QUOTE_KEY: &[u8] = b"dex_preferred_quote";
const ALLOWED_QUOTE_COUNT_KEY: &[u8] = b"dex_aq_count";
const MAX_ALLOWED_QUOTES: u64 = 8;
/// AUDIT-FIX M12: Timelock for unpause — 900 slots (~6 minutes at 400ms slots).
/// Pause is instant (circuit breaker), but unpause requires a scheduling + execution
/// two-step process to prevent a compromised admin from immediately resuming trading.
pub const UNPAUSE_TIMELOCK_SLOTS: u64 = 900;
const UNPAUSE_SCHEDULED_KEY: &[u8] = b"dex_unpause_scheduled_slot";
const TOTAL_VOLUME_KEY: &[u8] = b"dex_total_volume";

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

fn fee_recipient_addr() -> [u8; 32] {
    let configured = load_addr(FEE_TREASURY_ADDR_KEY);
    if !is_zero(&configured) {
        return configured;
    }
    let admin = load_addr(ADMIN_KEY);
    if !is_zero(&admin) {
        return admin;
    }
    [0u8; 32]
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

fn pair_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"dex_pair_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k
}

fn order_key(order_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"dex_order_"[..]);
    k.extend_from_slice(&u64_to_decimal(order_id));
    k
}

fn trade_key(trade_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"dex_trade_"[..]);
    k.extend_from_slice(&u64_to_decimal(trade_id));
    k
}

fn best_bid_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"dex_best_bid_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k
}

fn best_ask_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"dex_best_ask_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k
}

fn band_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"dex_band_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k
}

fn user_order_count_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"dex_uoc_"[..]);
    k.extend_from_slice(&hex_encode(addr));
    k
}

fn user_order_key(addr: &[u8; 32], idx: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"dex_uo_"[..]);
    k.extend_from_slice(&hex_encode(addr));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(idx));
    k
}

fn bid_level_key(pair_id: u64, price: u64, order_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"dex_bid_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(price));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(order_id));
    k
}

fn ask_level_key(pair_id: u64, price: u64, order_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"dex_ask_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(price));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(order_id));
    k
}

fn bid_count_key(pair_id: u64, price: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"dex_bidc_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(price));
    k
}

fn ask_count_key(pair_id: u64, price: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"dex_askc_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(price));
    k
}

// ============================================================================
// DEEP SECURITY: Reentrancy + Pause
// ============================================================================

fn reentrancy_enter() -> bool {
    if storage_get(REENTRANCY_KEY)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
    {
        return false;
    }
    storage_set(REENTRANCY_KEY, &[1u8]);
    true
}

fn reentrancy_exit() {
    storage_set(REENTRANCY_KEY, &[0u8]);
}

fn is_paused() -> bool {
    storage_get(PAUSED_KEY)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
}

fn require_not_paused() -> bool {
    !is_paused()
}

fn require_admin(caller: &[u8; 32]) -> bool {
    let admin = load_addr(ADMIN_KEY);
    !is_zero(&admin) && *caller == admin
}

// ============================================================================
// TRADING PAIR LAYOUT (112 bytes)
// ============================================================================
// Bytes 0..32   : base_token address
// Bytes 32..64  : quote_token address
// Bytes 64..72  : pair_id (u64)
// Bytes 72..80  : tick_size (u64)
// Bytes 80..88  : lot_size (u64)
// Bytes 88..96  : min_order (u64)
// Byte  96      : status (u8)
// Bytes 97..99  : maker_fee_bps (i16 LE) — can be negative for rebate
// Bytes 99..101 : taker_fee_bps (u16 LE)
// Bytes 101..109: daily_volume (u64)
// Bytes 109..112: padding

const PAIR_SIZE: usize = 112;

fn encode_pair(
    base_token: &[u8; 32],
    quote_token: &[u8; 32],
    pair_id: u64,
    tick_size: u64,
    lot_size: u64,
    min_order: u64,
    status: u8,
    maker_fee_bps: i16,
    taker_fee_bps: u16,
    daily_volume: u64,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(PAIR_SIZE);
    data.extend_from_slice(base_token);
    data.extend_from_slice(quote_token);
    data.extend_from_slice(&u64_to_bytes(pair_id));
    data.extend_from_slice(&u64_to_bytes(tick_size));
    data.extend_from_slice(&u64_to_bytes(lot_size));
    data.extend_from_slice(&u64_to_bytes(min_order));
    data.push(status);
    data.extend_from_slice(&maker_fee_bps.to_le_bytes());
    data.extend_from_slice(&taker_fee_bps.to_le_bytes());
    data.extend_from_slice(&u64_to_bytes(daily_volume));
    data.extend_from_slice(&[0u8; 3]); // padding
    data
}

fn decode_pair_status(data: &[u8]) -> u8 {
    if data.len() > 96 {
        data[96]
    } else {
        0
    }
}

fn decode_pair_id(data: &[u8]) -> u64 {
    if data.len() >= 72 {
        bytes_to_u64(&data[64..72])
    } else {
        0
    }
}

fn decode_pair_tick_size(data: &[u8]) -> u64 {
    if data.len() >= 80 {
        bytes_to_u64(&data[72..80])
    } else {
        0
    }
}

fn decode_pair_lot_size(data: &[u8]) -> u64 {
    if data.len() >= 88 {
        bytes_to_u64(&data[80..88])
    } else {
        0
    }
}

fn decode_pair_min_order(data: &[u8]) -> u64 {
    if data.len() >= 96 {
        bytes_to_u64(&data[88..96])
    } else {
        0
    }
}

fn decode_pair_taker_fee(data: &[u8]) -> u16 {
    if data.len() >= 101 {
        u16::from_le_bytes([data[99], data[100]])
    } else {
        DEFAULT_TAKER_FEE_BPS as u16
    }
}

fn decode_pair_maker_fee(data: &[u8]) -> i16 {
    if data.len() >= 99 {
        i16::from_le_bytes([data[97], data[98]])
    } else {
        DEFAULT_MAKER_FEE_BPS as i16
    }
}

fn decode_pair_base_token(data: &[u8]) -> [u8; 32] {
    let mut t = [0u8; 32];
    if data.len() >= 32 {
        t.copy_from_slice(&data[..32]);
    }
    t
}

fn decode_pair_quote_token(data: &[u8]) -> [u8; 32] {
    let mut t = [0u8; 32];
    if data.len() >= 64 {
        t.copy_from_slice(&data[32..64]);
    }
    t
}

fn decode_pair_daily_volume(data: &[u8]) -> u64 {
    if data.len() >= 109 {
        bytes_to_u64(&data[101..109])
    } else {
        0
    }
}

// ============================================================================
// ORDER LAYOUT (128 bytes)
// ============================================================================
// Bytes 0..32   : trader address
// Bytes 32..40  : pair_id (u64)
// Byte  40      : side (0=buy, 1=sell)
// Byte  41      : order_type (0=limit, 1=market, 2=stop-limit, 3=post-only)
// Bytes 42..50  : price (u64, scaled by 10^9)
// Bytes 50..58  : quantity (u64)
// Bytes 58..66  : filled (u64)
// Byte  66      : status
// Bytes 67..75  : created_slot (u64)
// Bytes 75..83  : expiry_slot (u64, 0=GTC)
// Bytes 83..91  : order_id (u64)
// Bytes 91..99  : trigger_price (u64, for stop-limit orders)
// Bytes 99..128 : padding (29 bytes)

const ORDER_SIZE: usize = 128;

fn encode_order(
    trader: &[u8; 32],
    pair_id: u64,
    side: u8,
    order_type: u8,
    price: u64,
    quantity: u64,
    filled: u64,
    status: u8,
    created_slot: u64,
    expiry_slot: u64,
    order_id: u64,
    trigger_price: u64,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(ORDER_SIZE);
    data.extend_from_slice(trader);
    data.extend_from_slice(&u64_to_bytes(pair_id));
    data.push(side);
    data.push(order_type);
    data.extend_from_slice(&u64_to_bytes(price));
    data.extend_from_slice(&u64_to_bytes(quantity));
    data.extend_from_slice(&u64_to_bytes(filled));
    data.push(status);
    data.extend_from_slice(&u64_to_bytes(created_slot));
    data.extend_from_slice(&u64_to_bytes(expiry_slot));
    data.extend_from_slice(&u64_to_bytes(order_id));
    data.extend_from_slice(&u64_to_bytes(trigger_price));
    while data.len() < ORDER_SIZE {
        data.push(0);
    }
    data
}

fn decode_order_trader(data: &[u8]) -> [u8; 32] {
    let mut t = [0u8; 32];
    if data.len() >= 32 {
        t.copy_from_slice(&data[..32]);
    }
    t
}
fn decode_order_pair_id(data: &[u8]) -> u64 {
    if data.len() >= 40 {
        bytes_to_u64(&data[32..40])
    } else {
        0
    }
}
fn decode_order_side(data: &[u8]) -> u8 {
    if data.len() > 40 {
        data[40]
    } else {
        0
    }
}
fn decode_order_type(data: &[u8]) -> u8 {
    if data.len() > 41 {
        data[41]
    } else {
        0
    }
}
fn decode_order_price(data: &[u8]) -> u64 {
    if data.len() >= 50 {
        bytes_to_u64(&data[42..50])
    } else {
        0
    }
}
fn decode_order_quantity(data: &[u8]) -> u64 {
    if data.len() >= 58 {
        bytes_to_u64(&data[50..58])
    } else {
        0
    }
}
fn decode_order_filled(data: &[u8]) -> u64 {
    if data.len() >= 66 {
        bytes_to_u64(&data[58..66])
    } else {
        0
    }
}
fn decode_order_status(data: &[u8]) -> u8 {
    if data.len() > 66 {
        data[66]
    } else {
        0
    }
}
fn decode_order_created_slot(data: &[u8]) -> u64 {
    if data.len() >= 75 {
        bytes_to_u64(&data[67..75])
    } else {
        0
    }
}
fn decode_order_expiry_slot(data: &[u8]) -> u64 {
    if data.len() >= 83 {
        bytes_to_u64(&data[75..83])
    } else {
        0
    }
}
fn decode_order_id(data: &[u8]) -> u64 {
    if data.len() >= 91 {
        bytes_to_u64(&data[83..91])
    } else {
        0
    }
}

// Task 2.2: Decode trigger price for stop-limit orders
fn decode_order_trigger_price(data: &[u8]) -> u64 {
    if data.len() >= 99 {
        bytes_to_u64(&data[91..99])
    } else {
        0
    }
}

// Task 2.2: Update trigger price on an existing order
fn update_order_trigger_price(data: &mut Vec<u8>, trigger_price: u64) {
    while data.len() < 99 { data.push(0); }
    data[91..99].copy_from_slice(&u64_to_bytes(trigger_price));
}

fn update_order_filled(data: &mut Vec<u8>, new_filled: u64) {
    if data.len() >= 66 {
        let bytes = u64_to_bytes(new_filled);
        data[58..66].copy_from_slice(&bytes);
    }
}

fn update_order_status(data: &mut Vec<u8>, new_status: u8) {
    if data.len() > 66 {
        data[66] = new_status;
    }
}

// ============================================================================
// TRADE LAYOUT (80 bytes)
// ============================================================================
// Bytes 0..8    : trade_id (u64)
// Bytes 8..16   : pair_id (u64)
// Bytes 16..24  : price (u64)
// Bytes 24..32  : quantity (u64)
// Bytes 32..64  : taker_addr [32]
// Bytes 64..72  : maker_order_id (u64)
// Bytes 72..80  : slot (u64)

const TRADE_SIZE: usize = 80;

fn encode_trade(
    trade_id: u64,
    pair_id: u64,
    price: u64,
    quantity: u64,
    taker: &[u8; 32],
    maker_order_id: u64,
    slot: u64,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(TRADE_SIZE);
    data.extend_from_slice(&u64_to_bytes(trade_id));
    data.extend_from_slice(&u64_to_bytes(pair_id));
    data.extend_from_slice(&u64_to_bytes(price));
    data.extend_from_slice(&u64_to_bytes(quantity));
    data.extend_from_slice(taker);
    data.extend_from_slice(&u64_to_bytes(maker_order_id));
    data.extend_from_slice(&u64_to_bytes(slot));
    data
}

// ============================================================================
// FEE CALCULATION
// ============================================================================

fn calculate_taker_fee(notional: u64, fee_bps: u16) -> u64 {
    let fee = (notional as u128 * fee_bps as u128 / 10_000) as u64;
    if fee < MIN_FEE_PER_TRADE {
        MIN_FEE_PER_TRADE
    } else {
        fee
    }
}

fn calculate_maker_rebate(notional: u64, fee_bps: i16) -> u64 {
    if fee_bps >= 0 {
        return 0;
    }
    let abs_bps = (-fee_bps) as u64;
    (notional as u128 * abs_bps as u128 / 10_000) as u64
}

// ============================================================================
// PUBLIC FUNCTIONS
// ============================================================================

/// Initialize the DEX core contract
/// Returns: 0=success, 1=already initialized
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
    storage_set(FEE_TREASURY_ADDR_KEY, &addr);
    save_u64(PAIR_COUNT_KEY, 0);
    save_u64(ORDER_COUNT_KEY, 0);
    save_u64(TRADE_COUNT_KEY, 0);
    save_u64(FEE_TREASURY_KEY, 0);
    storage_set(PAUSED_KEY, &[0u8]);
    log_info("DEX Core initialized");
    0
}

fn allowed_quote_key(idx: u64) -> Vec<u8> {
    let mut k = b"dex_aq_".to_vec();
    k.extend_from_slice(&u64_to_bytes(idx));
    k
}

fn is_allowed_quote(addr: &[u8; 32]) -> bool {
    let count = load_u64(ALLOWED_QUOTE_COUNT_KEY);
    if count > 0 {
        for i in 0..count {
            if load_addr(&allowed_quote_key(i)) == *addr {
                return true;
            }
        }
        return false;
    }
    // Legacy fallback: check single preferred_quote
    let preferred = load_addr(PREFERRED_QUOTE_KEY);
    if is_zero(&preferred) {
        return true; // No restrictions
    }
    *addr == preferred
}

/// Set the preferred quote token address (admin only).
/// Legacy API — clears allowed quotes list and sets a single allowed quote.
/// Use add_allowed_quote() for multi-quote support (e.g. mUSD + MOLT).
/// Returns: 0=success, 1=not admin, 2=zero address
pub fn set_preferred_quote(caller: *const u8, quote_addr: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut q = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(quote_addr, q.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    if is_zero(&q) {
        return 2;
    }
    // Clear existing allowed quotes
    let old_count = load_u64(ALLOWED_QUOTE_COUNT_KEY);
    for i in 0..old_count {
        storage_set(&allowed_quote_key(i), &[0u8; 32]);
    }
    // Set as the sole allowed quote
    storage_set(&allowed_quote_key(0), &q);
    save_u64(ALLOWED_QUOTE_COUNT_KEY, 1);
    // Also set legacy key for get_preferred_quote compat
    storage_set(PREFERRED_QUOTE_KEY, &q);
    log_info("Preferred quote token set (single)");
    0
}

/// Add an allowed quote token (admin only).
/// Pairs can be created with any quote token in the allowed list.
/// If the list is empty, any quote token is accepted.
/// Returns: 0=success, 1=not admin, 2=zero address, 3=already in list, 4=max reached
pub fn add_allowed_quote(caller: *const u8, quote_addr: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut q = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(quote_addr, q.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) { return 1; }
    if is_zero(&q) { return 2; }
    let count = load_u64(ALLOWED_QUOTE_COUNT_KEY);
    for i in 0..count {
        if load_addr(&allowed_quote_key(i)) == q { return 3; }
    }
    if count >= MAX_ALLOWED_QUOTES { return 4; }
    storage_set(&allowed_quote_key(count), &q);
    save_u64(ALLOWED_QUOTE_COUNT_KEY, count + 1);
    log_info("Allowed quote token added");
    0
}

/// Remove an allowed quote token (admin only).
/// Returns: 0=success, 1=not admin, 2=not found
pub fn remove_allowed_quote(caller: *const u8, quote_addr: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut q = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(quote_addr, q.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) { return 1; }
    let count = load_u64(ALLOWED_QUOTE_COUNT_KEY);
    for i in 0..count {
        if load_addr(&allowed_quote_key(i)) == q {
            // Swap with last and shrink
            if i < count - 1 {
                let last = load_addr(&allowed_quote_key(count - 1));
                storage_set(&allowed_quote_key(i), &last);
            }
            storage_set(&allowed_quote_key(count - 1), &[0u8; 32]);
            save_u64(ALLOWED_QUOTE_COUNT_KEY, count - 1);
            log_info("Allowed quote token removed");
            return 0;
        }
    }
    2
}

/// Get the number of allowed quote tokens.
pub fn get_allowed_quote_count() -> u64 {
    load_u64(ALLOWED_QUOTE_COUNT_KEY)
}

/// Create a new trading pair
/// Returns: 0=success, 1=not admin, 2=paused, 3=max pairs, 4=invalid params, 5=reentrancy
pub fn create_pair(
    caller: *const u8,
    base_token: *const u8,
    quote_token: *const u8,
    tick_size: u64,
    lot_size: u64,
    min_order: u64,
) -> u32 {
    if !reentrancy_enter() {
        return 5;
    }
    let mut c = [0u8; 32];
    let mut bt = [0u8; 32];
    let mut qt = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(base_token, bt.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(quote_token, qt.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        reentrancy_exit();
        return 200;
    }
    if !require_admin(&c) {
        reentrancy_exit();
        return 1;
    }
    if !require_not_paused() {
        reentrancy_exit();
        return 2;
    }

    let count = load_u64(PAIR_COUNT_KEY);
    if count >= MAX_PAIRS {
        reentrancy_exit();
        return 3;
    }
    if tick_size == 0 || lot_size == 0 || min_order < MIN_ORDER_VALUE {
        reentrancy_exit();
        return 4;
    }
    if bt == qt {
        reentrancy_exit();
        return 4;
    }

    // AUDIT-FIX P2: Reject duplicate trading pair
    for i in 1..=count {
        let pk = pair_key(i);
        if let Some(existing) = storage_get(&pk) {
            if existing.len() >= 64 {
                let mut existing_base = [0u8; 32];
                let mut existing_quote = [0u8; 32];
                existing_base.copy_from_slice(&existing[0..32]);
                existing_quote.copy_from_slice(&existing[32..64]);
                if existing_base == bt && existing_quote == qt {
                    log_info("create_pair rejected: pair already exists");
                    reentrancy_exit();
                    return 7;
                }
            }
        }
    }

    // Enforce allowed quote tokens (supports multiple: e.g. mUSD + MOLT)
    if !is_allowed_quote(&qt) {
        reentrancy_exit();
        log_info("create_pair rejected: quote token not in allowed quotes list");
        return 6;
    }

    let pair_id = count + 1;
    let data = encode_pair(
        &bt,
        &qt,
        pair_id,
        tick_size,
        lot_size,
        min_order,
        PAIR_ACTIVE,
        DEFAULT_MAKER_FEE_BPS as i16,
        DEFAULT_TAKER_FEE_BPS as u16,
        0,
    );
    storage_set(&pair_key(pair_id), &data);
    save_u64(PAIR_COUNT_KEY, pair_id);
    // Initialize best bid/ask to sentinel values
    save_u64(&best_bid_key(pair_id), 0);
    save_u64(&best_ask_key(pair_id), u64::MAX);
    log_info("Trading pair created");
    reentrancy_exit();
    0
}

/// Update pair fees (admin only)
/// Returns: 0=success, 1=not admin, 2=pair not found, 3=fee too high
pub fn update_pair_fees(
    caller: *const u8,
    pair_id: u64,
    maker_fee_bps: i16,
    taker_fee_bps: u16,
) -> u32 {
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

    let pk = pair_key(pair_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= PAIR_SIZE => d,
        _ => return 2,
    };
    if taker_fee_bps > MAX_FEE_BPS as u16 {
        return 3;
    }
    if maker_fee_bps > MAX_FEE_BPS as i16 {
        return 3;
    }
    // Update fee fields
    data[97..99].copy_from_slice(&maker_fee_bps.to_le_bytes());
    data[99..101].copy_from_slice(&taker_fee_bps.to_le_bytes());
    storage_set(&pk, &data);
    0
}

/// Pause a trading pair
/// Returns: 0=success, 1=not admin, 2=pair not found
pub fn pause_pair(caller: *const u8, pair_id: u64) -> u32 {
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
    let pk = pair_key(pair_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= PAIR_SIZE => d,
        _ => return 2,
    };
    data[96] = PAIR_PAUSED;
    storage_set(&pk, &data);
    0
}

/// Unpause a trading pair
pub fn unpause_pair(caller: *const u8, pair_id: u64) -> u32 {
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
    let pk = pair_key(pair_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= PAIR_SIZE => d,
        _ => return 2,
    };
    data[96] = PAIR_ACTIVE;
    storage_set(&pk, &data);
    0
}

/// Place an order on the order book
/// Returns: 0=success, 1=paused, 2=pair not found, 3=pair not active,
///          4=invalid params, 5=too many orders, 6=reentrancy,
///          7=post-only would cross, 8=expired order,
///          9=market order slippage exceeded (zero fills at worst-price bound)
///
/// AUDIT-FIX M10: For market orders (order_type == ORDER_MARKET), the `price`
/// field serves as a worst-price bound:
///   - BUY market: `price` = maximum price willing to pay (0 = no limit)
///   - SELL market: `price` = minimum price willing to accept (0 = no limit)
/// The matching engine already enforces `price >= best_ask` (buy) and
/// `price <= best_bid` (sell), so passing a non-zero price activates
/// slippage protection with no engine changes.
pub fn place_order(
    trader: *const u8,
    pair_id: u64,
    side: u8,
    order_type: u8,
    price: u64,
    quantity: u64,
    expiry_slot: u64,
    trigger_price: u64,
) -> u32 {
    if !reentrancy_enter() {
        return 6;
    }
    if !require_not_paused() {
        reentrancy_exit();
        return 1;
    }

    let mut t = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(trader, t.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != t {
        reentrancy_exit();
        return 200;
    }

    // Load pair
    let pk = pair_key(pair_id);
    let pair_data = match storage_get(&pk) {
        Some(d) if d.len() >= PAIR_SIZE => d,
        _ => {
            reentrancy_exit();
            return 2;
        }
    };
    if decode_pair_status(&pair_data) != PAIR_ACTIVE {
        reentrancy_exit();
        return 3;
    }

    let tick = decode_pair_tick_size(&pair_data);
    let lot = decode_pair_lot_size(&pair_data);
    let min_ord = decode_pair_min_order(&pair_data);

    // G2-04: Extract reduce-only flag and base order type
    let is_reduce_only = (order_type & REDUCE_ONLY_FLAG) != 0;
    let base_order_type = order_type & 0x7F;

    // Validate params (using base_order_type for type range check)
    if side > SIDE_SELL || base_order_type > ORDER_POST_ONLY {
        reentrancy_exit();
        return 4;
    }
    if quantity == 0 || quantity > MAX_ORDER_SIZE {
        reentrancy_exit();
        return 4;
    }
    if base_order_type != ORDER_MARKET && price == 0 {
        reentrancy_exit();
        return 4;
    }
    if base_order_type != ORDER_MARKET && price % tick != 0 {
        reentrancy_exit();
        return 4;
    }
    if quantity % lot != 0 {
        reentrancy_exit();
        return 4;
    }

    // G2-04: mutable quantity for reduce-only capping
    let mut quantity = quantity;

    // Notional value check
    // AUDIT-FIX M10: For market orders with a worst-price bound, use
    // worst-price for notional estimation (more accurate than raw quantity).
    let notional = if base_order_type == ORDER_MARKET {
        if price > 0 {
            (price as u128 * quantity as u128 / 1_000_000_000) as u64
        } else {
            quantity
        }
    } else {
        (price as u128 * quantity as u128 / 1_000_000_000) as u64
    };
    if notional < min_ord {
        reentrancy_exit();
        return 4;
    }

    // Expiry validation
    let current_slot = get_slot();
    if expiry_slot != 0 {
        if expiry_slot <= current_slot {
            reentrancy_exit();
            return 8;
        }
        if expiry_slot.saturating_sub(current_slot) > ORDER_EXPIRY_MAX {
            reentrancy_exit();
            return 4;
        }
    }

    // Check user open order limit
    let user_count = load_u64(&user_order_count_key(&t));
    if user_count >= MAX_OPEN_ORDERS_PER_USER {
        reentrancy_exit();
        return 5;
    }

    // F19.11a: Balance validation via cross-contract call to token contract
    // For BUY orders, verify trader has sufficient quote token balance >= notional + fees
    // For SELL orders, verify trader has sufficient base token balance >= quantity
    // Uses best-effort cross-contract call (returns 0 if runtime doesn't support yet)
    {
        let pair_data_ref = storage_get(&pk).unwrap();
        let token_addr = if side == SIDE_BUY {
            // Quote token is token_b
            let mut addr = [0u8; 32];
            addr.copy_from_slice(&pair_data_ref[32..64]);
            addr
        } else {
            // Base token is token_a
            let mut addr = [0u8; 32];
            addr.copy_from_slice(&pair_data_ref[0..32]);
            addr
        };
        if !is_zero(&token_addr) {
            let mut bal_args = Vec::with_capacity(33);
            bal_args.push(5u8); // opcode: balance_of
            bal_args.extend_from_slice(&t);
            let call = CrossCall::new(Address(token_addr), "balance_of", bal_args)
                .with_value(0);
            let bal_result = call_contract(call);
            // AUDIT-FIX CON-11: Fail-closed balance validation.
            // If cross-contract call fails, reject the trade (fail-closed).
            match bal_result {
                Ok(bal_bytes) => {
                    if bal_bytes.len() >= 8 {
                        let balance = u64::from_le_bytes([
                            bal_bytes[0], bal_bytes[1], bal_bytes[2], bal_bytes[3],
                            bal_bytes[4], bal_bytes[5], bal_bytes[6], bal_bytes[7],
                        ]);
                        let required = if side == SIDE_BUY { notional } else { quantity };
                        if balance < required {
                            reentrancy_exit();
                            return 11; // Insufficient token balance
                        }
                    } else {
                        log_info("Balance query returned insufficient data — rejecting trade");
                        reentrancy_exit();
                        return 11;
                    }
                }
                Err(_) => {
                    log_info("Balance query cross-contract call failed — rejecting trade");
                    reentrancy_exit();
                    return 11;
                }
            }
        }
    }

    // ── Oracle Price Band Protection ──
    // The validator writes dex_band_{pair_id} (16 bytes: ref_price + slot)
    // for oracle-priced pairs. If present and fresh (<300 slots), enforce:
    //   Market orders: reject if worst-price bound is >5% from oracle
    //   Limit orders:  reject if limit price is >10% from oracle
    // Native-only pairs (no band record) → unrestricted.
    // Return code 10 = price outside oracle band.
    {
        let band_key_str = band_key(pair_id);
        if let Some(band_data) = storage_get(&band_key_str) {
            if band_data.len() >= 16 {
                let ref_price = u64::from_le_bytes([
                    band_data[0], band_data[1], band_data[2], band_data[3],
                    band_data[4], band_data[5], band_data[6], band_data[7],
                ]);
                let band_slot = u64::from_le_bytes([
                    band_data[8], band_data[9], band_data[10], band_data[11],
                    band_data[12], band_data[13], band_data[14], band_data[15],
                ]);

                // Only enforce if band data is fresh (within 300 slots ≈ 5 min)
                if ref_price > 0 && current_slot.saturating_sub(band_slot) < 300 {
                    let check_price = if base_order_type == ORDER_MARKET {
                        // Market order with worst-price bound
                        if price > 0 { price } else { 0 }
                    } else {
                        price
                    };

                    if check_price > 0 {
                        // Percentage thresholds (basis points): market=500 (5%), limit=1000 (10%)
                        let band_bps: u64 = if base_order_type == ORDER_MARKET { 500 } else { 1000 };

                        // Calculate allowed range: ref_price * (1 ± band_bps/10000) — use u128 to avoid overflow
                        let band = (ref_price as u128 * band_bps as u128 / 10000) as u64;
                        let lower = ref_price.saturating_sub(band);
                        let upper = ref_price.saturating_add(band);

                        if check_price < lower || check_price > upper {
                            reentrancy_exit();
                            return 10; // price outside oracle band
                        }
                    }
                }
            }
        }
    }

    // Post-only check: reject if would immediately match
    if base_order_type == ORDER_POST_ONLY {
        if side == SIDE_BUY {
            let best_ask = load_u64(&best_ask_key(pair_id));
            if best_ask != u64::MAX && price >= best_ask {
                reentrancy_exit();
                return 7;
            }
        } else {
            let best_bid = load_u64(&best_bid_key(pair_id));
            if best_bid != 0 && price <= best_bid {
                reentrancy_exit();
                return 7;
            }
        }
    }

    // G2-04: Reduce-only validation — cross-call dex_margin to verify open position
    if is_reduce_only {
        let margin_addr = load_addr(MARGIN_ADDRESS_KEY.as_bytes());
        if is_zero(&margin_addr) {
            // No margin contract configured — cannot validate reduce-only
            reentrancy_exit();
            return 12;
        }
        // Cross-call dex_margin opcode 26: query_user_open_position(trader[32], pair_id[8])
        let mut qargs = Vec::with_capacity(41);
        qargs.push(26u8); // opcode
        qargs.extend_from_slice(&t);
        qargs.extend_from_slice(&u64_to_bytes(pair_id));
        let call = CrossCall::new(Address(margin_addr), "query_user_open_position", qargs)
            .with_value(0);
        match call_contract(call) {
            Ok(ref data) if data.len() >= 58 => {
                // Parse position: byte 48 = side, byte 49 = status, bytes 50..58 = size
                let pos_side = data[48];
                let pos_status = data[49];
                let pos_size = bytes_to_u64(&data[50..58]);
                // Position must be open (status 0)
                if pos_status != 0 || pos_size == 0 {
                    reentrancy_exit();
                    return 12;
                }
                // Order must be in closing direction:
                // Long position (side=0) → must sell to reduce
                // Short position (side=1) → must buy to reduce
                if (pos_side == 0 && side != SIDE_SELL) || (pos_side == 1 && side != SIDE_BUY) {
                    reentrancy_exit();
                    return 12;
                }
                // Cap quantity at position size
                if quantity > pos_size {
                    quantity = pos_size;
                }
            }
            _ => {
                // Cross-call failed or returned insufficient data — no position found
                reentrancy_exit();
                return 12;
            }
        }
    }

    // Create order
    let order_count = load_u64(ORDER_COUNT_KEY);
    let new_order_id = order_count + 1;

    // Stop-limit orders with a trigger_price go dormant until triggered
    let initial_status = if base_order_type == ORDER_STOP_LIMIT && trigger_price > 0 {
        STATUS_DORMANT
    } else {
        STATUS_OPEN
    };

    let order_data = encode_order(
        &t,
        pair_id,
        side,
        base_order_type,
        price,
        quantity,
        0,
        initial_status,
        current_slot,
        expiry_slot,
        new_order_id,
        trigger_price,
    );
    storage_set(&order_key(new_order_id), &order_data);
    save_u64(ORDER_COUNT_KEY, new_order_id);

    // Track user orders
    let new_user_count = user_count + 1;
    save_u64(&user_order_count_key(&t), new_user_count);
    save_u64(&user_order_key(&t, new_user_count), new_order_id);

    // Dormant orders skip matching — they wait for trigger activation
    if initial_status == STATUS_DORMANT {
        reentrancy_exit();
        return 0;
    }

    // Try matching
    let remaining = match_order(new_order_id, pair_id, side, price, quantity, &t, &pair_data);

    // If not fully filled and limit-type, rest on book
    if remaining > 0 && base_order_type != ORDER_MARKET {
        add_to_book(pair_id, side, price, new_order_id);
    } else if remaining > 0 && base_order_type == ORDER_MARKET {
        // Market order: cancel unfilled remainder
        let mut od = storage_get(&order_key(new_order_id)).unwrap();
        let filled = quantity - remaining;
        update_order_filled(&mut od, filled);
        update_order_status(
            &mut od,
            if filled > 0 {
                STATUS_PARTIAL
            } else {
                STATUS_CANCELLED
            },
        );
        storage_set(&order_key(new_order_id), &od);

        // AUDIT-FIX M10: If market order got zero fills and had a worst-price
        // bound, return slippage error so caller knows the bound was hit.
        if filled == 0 && price > 0 {
            reentrancy_exit();
            return 9;
        }
    }

    reentrancy_exit();
    0
}

/// Match an incoming order against the book (internal)
fn match_order(
    taker_order_id: u64,
    pair_id: u64,
    side: u8,
    price: u64,
    mut remaining: u64,
    taker: &[u8; 32],
    pair_data: &[u8],
) -> u64 {
    let current_slot = get_slot();
    let taker_fee_bps = decode_pair_taker_fee(pair_data);
    let maker_fee_bps = decode_pair_maker_fee(pair_data);

    // For buy orders: match against asks (lowest first)
    // For sell orders: match against bids (highest first)
    if side == SIDE_BUY {
        let mut best_ask = load_u64(&best_ask_key(pair_id));
        while remaining > 0 && best_ask != u64::MAX && (price == 0 || price >= best_ask) {
            remaining = fill_at_price_level(
                taker_order_id,
                pair_id,
                SIDE_SELL,
                best_ask,
                remaining,
                taker,
                taker_fee_bps,
                maker_fee_bps,
                current_slot,
            );
            // Check if level is exhausted
            let level_count = load_u64(&ask_count_key(pair_id, best_ask));
            if level_count == 0 {
                // Move to next ask price — scan upward
                let mut next = best_ask + 1;
                let mut found = false;
                // Scan up to 1000 ticks to find next level
                for _ in 0..1000 {
                    if load_u64(&ask_count_key(pair_id, next)) > 0 {
                        best_ask = next;
                        found = true;
                        break;
                    }
                    next += 1;
                }
                if !found {
                    best_ask = u64::MAX;
                }
            }
        }
        save_u64(&best_ask_key(pair_id), best_ask);
    } else {
        let mut best_bid = load_u64(&best_bid_key(pair_id));
        while remaining > 0 && best_bid != 0 && (price == 0 || price <= best_bid) {
            remaining = fill_at_price_level(
                taker_order_id,
                pair_id,
                SIDE_BUY,
                best_bid,
                remaining,
                taker,
                taker_fee_bps,
                maker_fee_bps,
                current_slot,
            );
            let level_count = load_u64(&bid_count_key(pair_id, best_bid));
            if level_count == 0 {
                let mut next = best_bid.saturating_sub(1);
                let mut found = false;
                for _ in 0..1000 {
                    if next == 0 {
                        break;
                    }
                    if load_u64(&bid_count_key(pair_id, next)) > 0 {
                        best_bid = next;
                        found = true;
                        break;
                    }
                    next = next.saturating_sub(1);
                }
                if !found {
                    best_bid = 0;
                }
            }
        }
        save_u64(&best_bid_key(pair_id), best_bid);
    }

    // Update taker order state
    if remaining
        < decode_order_quantity(&storage_get(&order_key(taker_order_id)).unwrap_or_default())
    {
        let mut od = storage_get(&order_key(taker_order_id)).unwrap();
        let orig_qty = decode_order_quantity(&od);
        let filled = orig_qty - remaining;
        update_order_filled(&mut od, filled);
        if remaining == 0 {
            update_order_status(&mut od, STATUS_FILLED);
        } else {
            update_order_status(&mut od, STATUS_PARTIAL);
        }
        storage_set(&order_key(taker_order_id), &od);
    }

    remaining
}

/// Fill at a single price level (time priority)
fn fill_at_price_level(
    taker_order_id: u64,
    pair_id: u64,
    maker_side: u8,
    price: u64,
    mut remaining: u64,
    taker: &[u8; 32],
    taker_fee_bps: u16,
    maker_fee_bps: i16,
    current_slot: u64,
) -> u64 {
    let level_key = if maker_side == SIDE_BUY {
        bid_count_key(pair_id, price)
    } else {
        ask_count_key(pair_id, price)
    };
    let level_count = load_u64(&level_key);

    let mut new_level_count = level_count;

    for idx in 1..=level_count {
        if remaining == 0 {
            break;
        }

        let lk = if maker_side == SIDE_BUY {
            bid_level_key(pair_id, price, idx)
        } else {
            ask_level_key(pair_id, price, idx)
        };

        let maker_order_id = load_u64(&lk);
        if maker_order_id == 0 {
            continue;
        }

        let ok = order_key(maker_order_id);
        let mut maker_data = match storage_get(&ok) {
            Some(d) if d.len() >= ORDER_SIZE => d,
            _ => continue,
        };

        let maker_status = decode_order_status(&maker_data);
        if maker_status == STATUS_FILLED || maker_status == STATUS_CANCELLED {
            continue;
        }

        // Check expiry
        let expiry = decode_order_expiry_slot(&maker_data);
        if expiry != 0 && expiry <= current_slot {
            update_order_status(&mut maker_data, STATUS_EXPIRED);
            storage_set(&ok, &maker_data);
            save_u64(&lk, 0);
            new_level_count = new_level_count.saturating_sub(1);
            continue;
        }

        // Self-trade prevention: cancel maker (cancel-oldest)
        let maker_trader = decode_order_trader(&maker_data);
        if maker_trader == *taker {
            update_order_status(&mut maker_data, STATUS_CANCELLED);
            storage_set(&ok, &maker_data);
            save_u64(&lk, 0);
            new_level_count = new_level_count.saturating_sub(1);
            continue;
        }

        let maker_qty = decode_order_quantity(&maker_data);
        let maker_filled = decode_order_filled(&maker_data);
        let available = maker_qty - maker_filled;
        let fill_qty = if remaining > available {
            available
        } else {
            remaining
        };

        // Execute trade
        let notional = (price as u128 * fill_qty as u128 / 1_000_000_000) as u64;
        let taker_fee = calculate_taker_fee(notional, taker_fee_bps);
        let maker_rebate = calculate_maker_rebate(notional, maker_fee_bps);

        // Record protocol fees
        // AUDIT-FIX L6-01: u128 intermediate to prevent overflow on large trades
        let protocol_fee = (taker_fee as u128 * FEE_PROTOCOL_SHARE as u128 / 100) as u64;
        let current_treasury = load_u64(FEE_TREASURY_KEY);
        save_u64(FEE_TREASURY_KEY, current_treasury.saturating_add(protocol_fee));

        // F19.12a: Deduct taker fee from taker's quote token balance via cross-contract call
        // Uses best-effort pattern — won't fail trade if runtime doesn't support cross-contract yet
        {
            let pk_ref = pair_key(pair_id);
            if let Some(pd_ref) = storage_get(&pk_ref) {
                if pd_ref.len() >= 64 {
                    // Quote token (token_b) is where fees are denominated
                    let mut quote_addr = [0u8; 32];
                    quote_addr.copy_from_slice(&pd_ref[32..64]);
                    if !is_zero(&quote_addr) {
                        let recipient = fee_recipient_addr();
                        if is_zero(&recipient) {
                            continue;
                        }
                        // Transfer fee from taker to DEX treasury via token contract
                        let mut fee_args = Vec::with_capacity(73);
                        fee_args.push(3u8); // opcode: transfer
                        fee_args.extend_from_slice(taker); // from
                        fee_args.extend_from_slice(&recipient); // to: configured treasury
                        fee_args.extend_from_slice(&u64_to_bytes(taker_fee));
                        let call = CrossCall::new(Address(quote_addr), "transfer_fee", fee_args)
                            .with_value(0);
                        // AUDIT-FIX CON-12: Log fee transfer failures instead of silently ignoring
                        match call_contract(call) {
                            Ok(_) => {},
                            Err(_) => {
                                log_info("WARNING: Fee transfer to treasury failed — trade proceeds but fee uncollected");
                            }
                        }
                    }
                }
            }
        }

        // Update maker
        let new_maker_filled = maker_filled + fill_qty;
        update_order_filled(&mut maker_data, new_maker_filled);
        if new_maker_filled >= maker_qty {
            update_order_status(&mut maker_data, STATUS_FILLED);
            save_u64(&lk, 0);
            new_level_count = new_level_count.saturating_sub(1);
        } else {
            update_order_status(&mut maker_data, STATUS_PARTIAL);
        }
        storage_set(&ok, &maker_data);

        // Record trade
        let trade_count = load_u64(TRADE_COUNT_KEY);
        let trade_id = trade_count + 1;
        let trade_data = encode_trade(
            trade_id,
            pair_id,
            price,
            fill_qty,
            taker,
            maker_order_id,
            current_slot,
        );
        storage_set(&trade_key(trade_id), &trade_data);
        save_u64(TRADE_COUNT_KEY, trade_id);

        // Track global cumulative volume
        let total_vol = load_u64(TOTAL_VOLUME_KEY);
        save_u64(TOTAL_VOLUME_KEY, total_vol.saturating_add(notional));

        // F18.7: Update pair daily volume with slot-based daily reset
        let pk = pair_key(pair_id);
        if let Some(mut pd) = storage_get(&pk) {
            if pd.len() >= 109 {
                let mut day_key = Vec::from(&b"dex_day_slot_"[..]);
                day_key.extend_from_slice(&u64_to_decimal(pair_id));
                let current_day = current_slot / SLOTS_PER_DAY;
                let stored_day = load_u64(&day_key) / SLOTS_PER_DAY;
                if current_day != stored_day {
                    // New day — reset daily volume
                    pd[101..109].copy_from_slice(&u64_to_bytes(notional));
                    save_u64(&day_key, current_slot);
                } else {
                    let vol = decode_pair_daily_volume(&pd).saturating_add(notional);
                    pd[101..109].copy_from_slice(&u64_to_bytes(vol));
                }
                storage_set(&pk, &pd);
            }
        }

        // F18.2: Cross-contract call to analytics — record trade for candles/stats
        {
            let analytics_addr = load_addr(ANALYTICS_ADDRESS_KEY.as_bytes());
            if !is_zero(&analytics_addr) {
                let mut ana_args = Vec::with_capacity(57);
                ana_args.push(1u8); // opcode: record_trade
                ana_args.extend_from_slice(&u64_to_bytes(pair_id));
                ana_args.extend_from_slice(&u64_to_bytes(price));
                ana_args.extend_from_slice(&u64_to_bytes(notional));
                ana_args.extend_from_slice(taker);
                let call = CrossCall::new(Address(analytics_addr), "record_trade", ana_args)
                    .with_value(0);
                // Analytics recording is non-critical — log failures but don't block trade
                if call_contract(call).is_err() {
                    log_info("Analytics record_trade call failed — trade still valid");
                }
            }
        }

        remaining -= fill_qty;

        // F19.12b: Accumulate maker rebate for the maker address
        if maker_rebate > 0 {
            let mut rk = Vec::from(&b"dex_rebate_"[..]);
            rk.extend_from_slice(&maker_trader);
            let accrued = load_u64(&rk);
            save_u64(&rk, accrued.saturating_add(maker_rebate));
        }
    }

    // Update level count
    if maker_side == SIDE_BUY {
        save_u64(&bid_count_key(pair_id, price), new_level_count);
    } else {
        save_u64(&ask_count_key(pair_id, price), new_level_count);
    }

    remaining
}

/// Add resting order to book
fn add_to_book(pair_id: u64, side: u8, price: u64, order_id: u64) {
    if side == SIDE_BUY {
        let count = load_u64(&bid_count_key(pair_id, price));
        let new_count = count + 1;
        save_u64(&bid_level_key(pair_id, price, new_count), order_id);
        save_u64(&bid_count_key(pair_id, price), new_count);
        // Update best bid
        let best = load_u64(&best_bid_key(pair_id));
        if price > best {
            save_u64(&best_bid_key(pair_id), price);
        }
    } else {
        let count = load_u64(&ask_count_key(pair_id, price));
        let new_count = count + 1;
        save_u64(&ask_level_key(pair_id, price, new_count), order_id);
        save_u64(&ask_count_key(pair_id, price), new_count);
        // Update best ask
        let best = load_u64(&best_ask_key(pair_id));
        if price < best {
            save_u64(&best_ask_key(pair_id), price);
        }
    }
}

/// Cancel an order
/// Returns: 0=success, 1=not found, 2=not owner, 3=already filled/cancelled, 4=reentrancy
pub fn cancel_order(caller: *const u8, order_id: u64) -> u32 {
    if !reentrancy_enter() {
        return 4;
    }
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        reentrancy_exit();
        return 200;
    }

    let ok = order_key(order_id);
    let mut data = match storage_get(&ok) {
        Some(d) if d.len() >= ORDER_SIZE => d,
        _ => {
            reentrancy_exit();
            return 1;
        }
    };

    let trader = decode_order_trader(&data);
    if trader != c {
        reentrancy_exit();
        return 2;
    }

    let status = decode_order_status(&data);
    if status == STATUS_FILLED || status == STATUS_CANCELLED || status == STATUS_EXPIRED {
        reentrancy_exit();
        return 3;
    }

    update_order_status(&mut data, STATUS_CANCELLED);
    storage_set(&ok, &data);

    // Note: order stays in book level but will be skipped during matching (status check)
    log_info("Order cancelled");
    reentrancy_exit();
    0
}

/// Cancel all open orders for a trader on a pair
/// Returns: 0=success, 1=reentrancy
pub fn cancel_all_orders(caller: *const u8, pair_id: u64) -> u32 {
    if !reentrancy_enter() {
        return 1;
    }
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        reentrancy_exit();
        return 200;
    }

    let user_count = load_u64(&user_order_count_key(&c));
    for idx in 1..=user_count {
        let oid = load_u64(&user_order_key(&c, idx));
        if oid == 0 {
            continue;
        }
        let ok = order_key(oid);
        if let Some(mut data) = storage_get(&ok) {
            if data.len() >= ORDER_SIZE {
                let op = decode_order_pair_id(&data);
                let status = decode_order_status(&data);
                if op == pair_id && (status == STATUS_OPEN || status == STATUS_PARTIAL) {
                    update_order_status(&mut data, STATUS_CANCELLED);
                    storage_set(&ok, &data);
                }
            }
        }
    }
    reentrancy_exit();
    0
}

/// Modify an existing order (cancel + replace)
/// Returns: 0=success, 1=not found, 2=not owner, 3=not modifiable, 4=reentrancy
pub fn modify_order(caller: *const u8, order_id: u64, new_price: u64, new_quantity: u64) -> u32 {
    if !reentrancy_enter() {
        return 4;
    }
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        reentrancy_exit();
        return 200;
    }

    let ok = order_key(order_id);
    let data = match storage_get(&ok) {
        Some(d) if d.len() >= ORDER_SIZE => d,
        _ => {
            reentrancy_exit();
            return 1;
        }
    };

    let trader = decode_order_trader(&data);
    if trader != c {
        reentrancy_exit();
        return 2;
    }

    let status = decode_order_status(&data);
    if status != STATUS_OPEN && status != STATUS_PARTIAL {
        reentrancy_exit();
        return 3;
    }

    // Cancel old order
    let mut data_mut = data.clone();
    update_order_status(&mut data_mut, STATUS_CANCELLED);
    storage_set(&ok, &data_mut);

    // Place new order with same parameters but new price/quantity
    let pair_id = decode_order_pair_id(&data);
    let side = decode_order_side(&data);
    let otype = decode_order_type(&data);
    let expiry = decode_order_expiry_slot(&data);
    let trigger = decode_order_trigger_price(&data);

    reentrancy_exit();
    place_order(
        c.as_ptr(),
        pair_id,
        side,
        otype,
        new_price,
        new_quantity,
        expiry,
        trigger,
    )
}

/// F18.2: Set analytics contract address (admin only)
/// Enables cross-contract trade recording for candles/stats/leaderboard
pub fn set_analytics_address(caller: *const u8, analytics: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut a = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(analytics, a.as_mut_ptr(), 32);
    }
    let real_caller = get_caller();
    if real_caller.0 != c { return 200; }
    if !require_admin(&c) { return 1; }
    storage_set(ANALYTICS_ADDRESS_KEY.as_bytes(), &a);
    log_info("DEX Core: analytics address set");
    0
}

/// G2-04: Set the margin contract address for reduce-only validation
pub fn set_margin_address(caller: *const u8, margin: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut m = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(margin, m.as_mut_ptr(), 32);
    }
    let real_caller = get_caller();
    if real_caller.0 != c { return 200; }
    if !require_admin(&c) { return 1; }
    storage_set(MARGIN_ADDRESS_KEY.as_bytes(), &m);
    log_info("DEX Core: margin address set");
    0
}

/// Emergency pause (admin only) — instant, no timelock
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
    // AUDIT-FIX M12: Clear any pending unpause schedule when re-pausing
    storage_set(UNPAUSE_SCHEDULED_KEY, &u64_to_bytes(0));
    log_info("DEX Core: EMERGENCY PAUSE ACTIVATED");
    0
}

/// AUDIT-FIX M12: Schedule unpause (admin only) — starts the timelock countdown.
/// The actual unpause happens when `execute_unpause` is called after UNPAUSE_TIMELOCK_SLOTS.
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
    // Check that DEX is actually paused
    let is_paused = storage_get(PAUSED_KEY)
        .map(|d| !d.is_empty() && d[0] == 1)
        .unwrap_or(false);
    if !is_paused {
        log_info("DEX Core: Not paused, nothing to unpause");
        return 2;
    }
    let current_slot = get_slot();
    let execute_after = current_slot + UNPAUSE_TIMELOCK_SLOTS;
    storage_set(UNPAUSE_SCHEDULED_KEY, &u64_to_bytes(execute_after));
    log_info("DEX Core: Unpause SCHEDULED — execute after timelock elapses");
    0
}

/// AUDIT-FIX M12: Execute a previously scheduled unpause after timelock has elapsed.
pub fn execute_unpause(caller: *const u8) -> u32 {
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
    let scheduled = match storage_get(UNPAUSE_SCHEDULED_KEY) {
        Some(d) if d.len() >= 8 => bytes_to_u64(&d),
        _ => 0,
    };
    if scheduled == 0 {
        log_info("DEX Core: No unpause scheduled");
        return 3;
    }
    let current_slot = get_slot();
    if current_slot < scheduled {
        log_info("DEX Core: Timelock not yet elapsed");
        return 4;
    }
    // Timelock elapsed — execute unpause
    storage_set(PAUSED_KEY, &[0u8]);
    storage_set(UNPAUSE_SCHEDULED_KEY, &u64_to_bytes(0));
    log_info("DEX Core: Resumed after timelock");
    0
}

// ============================================================================
// QUERY FUNCTIONS
// ============================================================================

/// Get order data. Returns order_id or 0 if not found.
pub fn get_order(order_id: u64) -> u64 {
    let ok = order_key(order_id);
    match storage_get(&ok) {
        Some(d) if d.len() >= ORDER_SIZE => {
            moltchain_sdk::set_return_data(&d);
            decode_order_id(&d)
        }
        _ => 0,
    }
}

/// Get best bid price for a pair
pub fn get_best_bid(pair_id: u64) -> u64 {
    load_u64(&best_bid_key(pair_id))
}

/// Get best ask price for a pair
pub fn get_best_ask(pair_id: u64) -> u64 {
    let ask = load_u64(&best_ask_key(pair_id));
    if ask == u64::MAX {
        0
    } else {
        ask
    }
}

/// Get spread (best_ask - best_bid)
pub fn get_spread(pair_id: u64) -> u64 {
    let bid = get_best_bid(pair_id);
    let ask = load_u64(&best_ask_key(pair_id));
    if bid == 0 || ask == u64::MAX {
        return 0;
    }
    ask.saturating_sub(bid)
}

/// Get pair info
pub fn get_pair_info(pair_id: u64) -> u64 {
    let pk = pair_key(pair_id);
    match storage_get(&pk) {
        Some(d) if d.len() >= PAIR_SIZE => {
            moltchain_sdk::set_return_data(&d);
            pair_id
        }
        _ => 0,
    }
}

/// Get trade count
pub fn get_trade_count() -> u64 {
    load_u64(TRADE_COUNT_KEY)
}

/// Get total pair count
pub fn get_pair_count() -> u64 {
    load_u64(PAIR_COUNT_KEY)
}

/// Get accumulated protocol fees
pub fn get_fee_treasury() -> u64 {
    load_u64(FEE_TREASURY_KEY)
}

/// Get the preferred quote token address (returns all zeros if not set)
pub fn get_preferred_quote() -> u64 {
    let addr = load_addr(PREFERRED_QUOTE_KEY);
    moltchain_sdk::set_return_data(&addr);
    if is_zero(&addr) {
        0
    } else {
        1
    }
}

// ============================================================================
// STOP-LOSS / TAKE-PROFIT TRIGGER ENGINE
// ============================================================================

/// Check all dormant (stop-limit) orders for a pair and activate those whose
/// trigger condition is met. The validator calls this after each block with the
/// latest trade price for the pair.
///
/// Trigger conditions:
///   - Sell-stop: triggers when last_price <= trigger_price (price falling)
///   - Buy-stop:  triggers when last_price >= trigger_price (price rising)
///
/// When triggered the order is set to STATUS_OPEN and immediately sent through
/// the matching engine. Any unfilled remainder rests on the order book at the
/// order's limit price.
///
/// Returns the number of orders that were triggered.
pub fn check_triggers(pair_id: u64, last_price: u64) -> u64 {
    if last_price == 0 {
        return 0;
    }

    // Load pair data for matching
    let pk = pair_key(pair_id);
    let pair_data = match storage_get(&pk) {
        Some(d) if d.len() >= PAIR_SIZE => d,
        _ => return 0,
    };

    let order_count = load_u64(ORDER_COUNT_KEY);
    let mut triggered: u64 = 0;

    for oid in 1..=order_count {
        let ok = order_key(oid);
        let data = match storage_get(&ok) {
            Some(d) if d.len() >= ORDER_SIZE => d,
            _ => continue,
        };

        // Only process dormant orders for this pair
        if decode_order_status(&data) != STATUS_DORMANT {
            continue;
        }
        if decode_order_pair_id(&data) != pair_id {
            continue;
        }

        let trigger = decode_order_trigger_price(&data);
        if trigger == 0 {
            continue;
        }

        let side = decode_order_side(&data);

        // Check trigger condition
        let should_trigger = if side == SIDE_SELL {
            // Sell-stop: triggers when price falls to or below trigger
            last_price <= trigger
        } else {
            // Buy-stop: triggers when price rises to or above trigger
            last_price >= trigger
        };

        if !should_trigger {
            continue;
        }

        // Activate the order
        let mut od = data;
        update_order_status(&mut od, STATUS_OPEN);
        storage_set(&ok, &od);

        let price = decode_order_price(&od);
        let quantity = decode_order_quantity(&od);
        let filled = decode_order_filled(&od);
        let remaining_qty = quantity - filled;
        let trader = decode_order_trader(&od);

        // Run through matching engine
        let remaining = match_order(oid, pair_id, side, price, remaining_qty, &trader, &pair_data);

        // Rest unfilled portion on book (stop-limit acts as limit once triggered)
        if remaining > 0 {
            add_to_book(pair_id, side, price, oid);
        }

        triggered += 1;
    }

    triggered
}

// ============================================================================
// WASM ENTRY POINT
// ============================================================================

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn call() {
    let args = moltchain_sdk::get_args();
    if args.is_empty() {
        return;
    }

    match args[0] {
        0 => {
            // initialize
            if args.len() >= 33 {
                let result = initialize(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        1 => {
            // create_pair
            if args.len() >= 1 + 32 + 32 + 32 + 8 + 8 + 8 {
                let result = create_pair(
                    args[1..33].as_ptr(),
                    args[33..65].as_ptr(),
                    args[65..97].as_ptr(),
                    bytes_to_u64(&args[97..105]),
                    bytes_to_u64(&args[105..113]),
                    bytes_to_u64(&args[113..121]),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        2 => {
            // place_order (67 bytes min, 75 bytes with trigger_price)
            if args.len() >= 1 + 32 + 8 + 1 + 1 + 8 + 8 + 8 {
                let trigger_price = if args.len() >= 75 {
                    bytes_to_u64(&args[67..75])
                } else {
                    0u64
                };
                let result = place_order(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    args[41],
                    args[42],
                    bytes_to_u64(&args[43..51]),
                    bytes_to_u64(&args[51..59]),
                    bytes_to_u64(&args[59..67]),
                    trigger_price,
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        3 => {
            // cancel_order
            if args.len() >= 1 + 32 + 8 {
                let result = cancel_order(args[1..33].as_ptr(), bytes_to_u64(&args[33..41]));
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        4 => {
            // set_preferred_quote
            if args.len() >= 1 + 32 + 32 {
                let result = set_preferred_quote(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        5 => {
            // get_pair_count
            moltchain_sdk::set_return_data(&u64_to_bytes(get_pair_count()));
        }
        6 => {
            // get_preferred_quote
            get_preferred_quote();
        }
        7 => {
            // update_pair_fees
            if args.len() >= 1 + 32 + 8 + 2 + 2 {
                let maker = i16::from_le_bytes([args[41], args[42]]);
                let taker = u16::from_le_bytes([args[43], args[44]]);
                let result = update_pair_fees(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    maker,
                    taker,
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        8 => {
            // emergency_pause
            if args.len() >= 33 {
                let result = emergency_pause(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        9 => {
            // emergency_unpause
            if args.len() >= 33 {
                let result = emergency_unpause(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        10 => {
            // get_best_bid
            if args.len() >= 9 {
                moltchain_sdk::set_return_data(&u64_to_bytes(get_best_bid(bytes_to_u64(
                    &args[1..9],
                ))));
            }
        }
        11 => {
            // get_best_ask
            if args.len() >= 9 {
                moltchain_sdk::set_return_data(&u64_to_bytes(get_best_ask(bytes_to_u64(
                    &args[1..9],
                ))));
            }
        }
        12 => {
            // get_spread
            if args.len() >= 9 {
                moltchain_sdk::set_return_data(&u64_to_bytes(get_spread(bytes_to_u64(
                    &args[1..9],
                ))));
            }
        }
        13 => {
            // get_pair_info
            if args.len() >= 9 {
                get_pair_info(bytes_to_u64(&args[1..9]));
            }
        }
        14 => {
            // get_trade_count
            moltchain_sdk::set_return_data(&u64_to_bytes(get_trade_count()));
        }
        15 => {
            // get_fee_treasury
            moltchain_sdk::set_return_data(&u64_to_bytes(get_fee_treasury()));
        }
        16 => {
            // modify_order
            if args.len() >= 1 + 32 + 8 + 8 + 8 {
                let result = modify_order(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    bytes_to_u64(&args[41..49]),
                    bytes_to_u64(&args[49..57]),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        17 => {
            // cancel_all_orders
            if args.len() >= 1 + 32 + 8 {
                let result = cancel_all_orders(args[1..33].as_ptr(), bytes_to_u64(&args[33..41]));
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        18 => {
            // pause_pair
            if args.len() >= 1 + 32 + 8 {
                let result = pause_pair(args[1..33].as_ptr(), bytes_to_u64(&args[33..41]));
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        19 => {
            // unpause_pair
            if args.len() >= 1 + 32 + 8 {
                let result = unpause_pair(args[1..33].as_ptr(), bytes_to_u64(&args[33..41]));
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        20 => {
            // get_order
            if args.len() >= 9 {
                get_order(bytes_to_u64(&args[1..9]));
            }
        }
        21 => {
            // add_allowed_quote(caller[32] + quote_addr[32])
            if args.len() >= 1 + 32 + 32 {
                let result = add_allowed_quote(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        22 => {
            // remove_allowed_quote(caller[32] + quote_addr[32])
            if args.len() >= 1 + 32 + 32 {
                let result = remove_allowed_quote(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        23 => {
            // get_allowed_quote_count
            moltchain_sdk::set_return_data(&u64_to_bytes(get_allowed_quote_count()));
        }
        24 => {
            // AUDIT-FIX M12: execute_unpause — completes a previously scheduled unpause
            // after the timelock (UNPAUSE_TIMELOCK_SLOTS) has elapsed.
            if args.len() >= 33 {
                let result = execute_unpause(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        25 => {
            // get_total_volume — returns cumulative total notional volume traded
            moltchain_sdk::set_return_data(&u64_to_bytes(load_u64(TOTAL_VOLUME_KEY)));
        }
        26 => {
            // get_user_orders — returns all open order IDs for a user address
            if args.len() >= 33 {
                let addr: [u8; 32] = args[1..33].try_into().unwrap_or([0u8; 32]);
                let count = load_u64(&user_order_count_key(&addr));
                let mut result = Vec::with_capacity(8 + count as usize * 8);
                result.extend_from_slice(&u64_to_bytes(count));
                for i in 1..=count {
                    let k = user_order_key(&addr, i);
                    let oid = storage_get(&k)
                        .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
                        .unwrap_or(0);
                    result.extend_from_slice(&u64_to_bytes(oid));
                }
                moltchain_sdk::set_return_data(&result);
            }
        }
        27 => {
            // get_open_order_count — returns user's order count
            if args.len() >= 33 {
                let addr: [u8; 32] = args[1..33].try_into().unwrap_or([0u8; 32]);
                let count = load_u64(&user_order_count_key(&addr));
                moltchain_sdk::set_return_data(&u64_to_bytes(count));
            }
        }
        // 28 = F18.2: set_analytics_address(caller[32], analytics_addr[32])
        28 => {
            if args.len() >= 65 {
                let r = set_analytics_address(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        29 => {
            // check_triggers(pair_id[8], last_price[8])
            // Called by validator after each block to activate dormant stop-limit orders
            if args.len() >= 17 {
                let triggered = check_triggers(
                    bytes_to_u64(&args[1..9]),
                    bytes_to_u64(&args[9..17]),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(triggered));
            }
        }
        // 30 = G2-04: set_margin_address(caller[32], margin_addr[32])
        30 => {
            if args.len() >= 65 {
                let r = set_margin_address(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        _ => { moltchain_sdk::set_return_data(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]); }
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

    fn setup_with_pair() -> ([u8; 32], u64) {
        let admin = setup();
        let base = [10u8; 32];
        let quote = [20u8; 32];
        assert_eq!(
            create_pair(
                admin.as_ptr(),
                base.as_ptr(),
                quote.as_ptr(),
                1000,
                100,
                1000
            ),
            0
        );
        // CON-11 fix: Balance check is now fail-closed. Mock a large balance so
        // cross-contract balance queries succeed in unit tests.
        test_mock::set_cross_call_response(Some(u64::MAX.to_le_bytes().to_vec()));
        (admin, 1)
    }

    // --- Initialization ---

    #[test]
    fn test_initialize() {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(load_addr(ADMIN_KEY), admin);
        assert_eq!(fee_recipient_addr(), admin);
        assert_eq!(load_u64(PAIR_COUNT_KEY), 0);
    }

    #[test]
    fn test_fee_recipient_prefers_configured_treasury() {
        let _admin = setup();
        let treasury = [9u8; 32];
        storage_set(FEE_TREASURY_ADDR_KEY, &treasury);
        assert_eq!(fee_recipient_addr(), treasury);
    }

    #[test]
    fn test_initialize_already_initialized() {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(initialize(admin.as_ptr()), 1);
    }

    // --- Pair Management ---

    #[test]
    fn test_create_pair() {
        let admin = setup();
        let base = [10u8; 32];
        let quote = [20u8; 32];
        assert_eq!(
            create_pair(
                admin.as_ptr(),
                base.as_ptr(),
                quote.as_ptr(),
                1000,
                100,
                1000
            ),
            0
        );
        assert_eq!(load_u64(PAIR_COUNT_KEY), 1);
    }

    #[test]
    fn test_create_pair_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_caller(rando);
        assert_eq!(
            create_pair(
                rando.as_ptr(),
                base.as_ptr(),
                quote.as_ptr(),
                1000,
                100,
                1000
            ),
            1
        );
    }

    #[test]
    fn test_create_pair_same_tokens() {
        let admin = setup();
        let token = [10u8; 32];
        assert_eq!(
            create_pair(
                admin.as_ptr(),
                token.as_ptr(),
                token.as_ptr(),
                1000,
                100,
                1000
            ),
            4
        );
    }

    #[test]
    fn test_create_pair_invalid_params() {
        let admin = setup();
        let base = [10u8; 32];
        let quote = [20u8; 32];
        // tick_size = 0
        assert_eq!(
            create_pair(admin.as_ptr(), base.as_ptr(), quote.as_ptr(), 0, 100, 1000),
            4
        );
        // lot_size = 0
        assert_eq!(
            create_pair(admin.as_ptr(), base.as_ptr(), quote.as_ptr(), 1000, 0, 1000),
            4
        );
        // min_order too low
        assert_eq!(
            create_pair(
                admin.as_ptr(),
                base.as_ptr(),
                quote.as_ptr(),
                1000,
                100,
                100
            ),
            4
        );
    }

    #[test]
    fn test_pause_unpause_pair() {
        let (admin, pair_id) = setup_with_pair();
        assert_eq!(pause_pair(admin.as_ptr(), pair_id), 0);
        let pd = storage_get(&pair_key(pair_id)).unwrap();
        assert_eq!(decode_pair_status(&pd), PAIR_PAUSED);

        assert_eq!(unpause_pair(admin.as_ptr(), pair_id), 0);
        let pd = storage_get(&pair_key(pair_id)).unwrap();
        assert_eq!(decode_pair_status(&pd), PAIR_ACTIVE);
    }

    #[test]
    fn test_update_pair_fees() {
        let (admin, pair_id) = setup_with_pair();
        assert_eq!(update_pair_fees(admin.as_ptr(), pair_id, -2, 10), 0);
        let pd = storage_get(&pair_key(pair_id)).unwrap();
        assert_eq!(decode_pair_maker_fee(&pd), -2);
        assert_eq!(decode_pair_taker_fee(&pd), 10);
    }

    #[test]
    fn test_update_fees_too_high() {
        let (admin, pair_id) = setup_with_pair();
        assert_eq!(update_pair_fees(admin.as_ptr(), pair_id, 0, 200), 3); // > 100 bps
    }

    // --- Order Placement ---

    #[test]
    fn test_place_limit_buy() {
        let (_admin, pair_id) = setup_with_pair();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        // price=10000 (10000 * 100 / 1e9 = not great, let's use bigger)
        // With tick_size=1000, lot_size=100, min_order=1000
        // notional = price * quantity / 1e9 >= 1000
        // 1_000_000_000 * 1000 / 1_000_000_000 = 1000 ✓
        assert_eq!(
            place_order(
                trader.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_LIMIT,
                1_000_000_000,
                1000,
                0,
                0
            ),
            0
        );
        assert_eq!(load_u64(ORDER_COUNT_KEY), 1);
    }

    #[test]
    fn test_place_limit_sell() {
        let (_admin, pair_id) = setup_with_pair();
        let trader = [3u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        assert_eq!(
            place_order(
                trader.as_ptr(),
                pair_id,
                SIDE_SELL,
                ORDER_LIMIT,
                2_000_000_000,
                1000,
                0,
                0
            ),
            0
        );
        assert_eq!(load_u64(ORDER_COUNT_KEY), 1);
    }

    #[test]
    fn test_place_order_paused() {
        let (admin, pair_id) = setup_with_pair();
        storage_set(PAUSED_KEY, &[1u8]);
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        assert_eq!(
            place_order(
                trader.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_LIMIT,
                1_000_000_000,
                1000,
                0,
                0
            ),
            1
        );
    }

    #[test]
    fn test_place_order_pair_not_found() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        assert_eq!(
            place_order(
                trader.as_ptr(),
                999,
                SIDE_BUY,
                ORDER_LIMIT,
                1_000_000_000,
                1000,
                0,
                0
            ),
            2
        );
    }

    #[test]
    fn test_place_order_pair_paused() {
        let (admin, pair_id) = setup_with_pair();
        pause_pair(admin.as_ptr(), pair_id);
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        assert_eq!(
            place_order(
                trader.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_LIMIT,
                1_000_000_000,
                1000,
                0,
                0
            ),
            3
        );
    }

    #[test]
    fn test_place_order_zero_quantity() {
        let (_admin, pair_id) = setup_with_pair();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        assert_eq!(
            place_order(
                trader.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_LIMIT,
                1_000_000_000,
                0,
                0,
                0
            ),
            4
        );
    }

    #[test]
    fn test_place_order_bad_tick() {
        let (_admin, pair_id) = setup_with_pair();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        // tick_size = 1000, price must be multiple of 1000
        assert_eq!(
            place_order(
                trader.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_LIMIT,
                1_000_000_001,
                1000,
                0,
                0
            ),
            4
        );
    }

    #[test]
    fn test_place_order_bad_lot() {
        let (_admin, pair_id) = setup_with_pair();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        // lot_size = 100, quantity must be multiple of 100
        assert_eq!(
            place_order(
                trader.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_LIMIT,
                1_000_000_000,
                99,
                0,
                0
            ),
            4
        );
    }

    #[test]
    fn test_place_order_below_min() {
        let (_admin, pair_id) = setup_with_pair();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        // min_order = 1000 shells notional
        // notional = 1000 * 100 / 1e9 = 0 — below min
        assert_eq!(
            place_order(
                trader.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_LIMIT,
                1000,
                100,
                0,
                0
            ),
            4
        );
    }

    #[test]
    fn test_place_order_expired_expiry() {
        let (_admin, pair_id) = setup_with_pair();
        let trader = [2u8; 32];
        test_mock::set_slot(1000);
        test_mock::set_caller(trader);
        // expiry = 500 < current_slot 1000
        assert_eq!(
            place_order(
                trader.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_LIMIT,
                1_000_000_000,
                1000,
                500,
                0
            ),
            8
        );
    }

    // --- Order Matching ---

    #[test]
    fn test_limit_order_match() {
        let (_admin, pair_id) = setup_with_pair();
        let seller = [3u8; 32];
        let buyer = [4u8; 32];
        test_mock::set_slot(100);

        // Seller places ask at 1_000_000_000
        test_mock::set_caller(seller);
        assert_eq!(
            place_order(
                seller.as_ptr(),
                pair_id,
                SIDE_SELL,
                ORDER_LIMIT,
                1_000_000_000,
                1000,
                0,
                0
            ),
            0
        );

        // Buyer places bid at same price — should match
        test_mock::set_caller(buyer);
        assert_eq!(
            place_order(
                buyer.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_LIMIT,
                1_000_000_000,
                1000,
                0,
                0
            ),
            0
        );

        assert_eq!(load_u64(TRADE_COUNT_KEY), 1);
        // Both orders should be filled
        let sell_data = storage_get(&order_key(1)).unwrap();
        assert_eq!(decode_order_status(&sell_data), STATUS_FILLED);
        let buy_data = storage_get(&order_key(2)).unwrap();
        assert_eq!(decode_order_status(&buy_data), STATUS_FILLED);
    }

    #[test]
    fn test_partial_fill() {
        let (_admin, pair_id) = setup_with_pair();
        let seller = [3u8; 32];
        let buyer = [4u8; 32];
        test_mock::set_slot(100);

        // Seller places ask for 2000
        test_mock::set_caller(seller);
        assert_eq!(
            place_order(
                seller.as_ptr(),
                pair_id,
                SIDE_SELL,
                ORDER_LIMIT,
                1_000_000_000,
                2000,
                0,
                0
            ),
            0
        );

        // Buyer only wants 1000
        test_mock::set_caller(buyer);
        assert_eq!(
            place_order(
                buyer.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_LIMIT,
                1_000_000_000,
                1000,
                0,
                0
            ),
            0
        );

        let sell_data = storage_get(&order_key(1)).unwrap();
        assert_eq!(decode_order_status(&sell_data), STATUS_PARTIAL);
        assert_eq!(decode_order_filled(&sell_data), 1000);
    }

    #[test]
    fn test_self_trade_prevention() {
        let (_admin, pair_id) = setup_with_pair();
        let trader = [5u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);

        // Same trader places both sides
        assert_eq!(
            place_order(
                trader.as_ptr(),
                pair_id,
                SIDE_SELL,
                ORDER_LIMIT,
                1_000_000_000,
                1000,
                0,
                0
            ),
            0
        );
        assert_eq!(
            place_order(
                trader.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_LIMIT,
                1_000_000_000,
                1000,
                0,
                0
            ),
            0
        );

        // No trade should have occurred — self-trade prevented
        assert_eq!(load_u64(TRADE_COUNT_KEY), 0);
        // Maker order should be cancelled (cancel-oldest)
        let sell_data = storage_get(&order_key(1)).unwrap();
        assert_eq!(decode_order_status(&sell_data), STATUS_CANCELLED);
    }

    #[test]
    fn test_post_only_rejected_when_crossing() {
        let (_admin, pair_id) = setup_with_pair();
        let seller = [3u8; 32];
        let buyer = [4u8; 32];
        test_mock::set_slot(100);

        // Seller places ask at 1_000_000_000
        test_mock::set_caller(seller);
        assert_eq!(
            place_order(
                seller.as_ptr(),
                pair_id,
                SIDE_SELL,
                ORDER_LIMIT,
                1_000_000_000,
                1000,
                0,
                0
            ),
            0
        );

        // Buyer tries post-only at same price — should be rejected
        test_mock::set_caller(buyer);
        assert_eq!(
            place_order(
                buyer.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_POST_ONLY,
                1_000_000_000,
                1000,
                0,
                0
            ),
            7
        );
    }

    #[test]
    fn test_post_only_accepted_when_not_crossing() {
        let (_admin, pair_id) = setup_with_pair();
        let seller = [3u8; 32];
        let buyer = [4u8; 32];
        test_mock::set_slot(100);

        // Seller places ask at 2_000_000_000
        test_mock::set_caller(seller);
        assert_eq!(
            place_order(
                seller.as_ptr(),
                pair_id,
                SIDE_SELL,
                ORDER_LIMIT,
                2_000_000_000,
                1000,
                0,
                0
            ),
            0
        );

        // Buyer post-only at 1_000_000_000 (below ask) — should rest
        test_mock::set_caller(buyer);
        assert_eq!(
            place_order(
                buyer.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_POST_ONLY,
                1_000_000_000,
                1000,
                0,
                0
            ),
            0
        );
    }

    // --- Reduce-Only (G2-04) ---

    #[test]
    fn test_reduce_only_rejected_no_margin_address() {
        // Without configured margin address, reduce-only orders should fail
        let (_admin, pair_id) = setup_with_pair();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        assert_eq!(
            place_order(
                trader.as_ptr(),
                pair_id,
                SIDE_SELL,
                ORDER_LIMIT | REDUCE_ONLY_FLAG,
                1_000_000_000,
                1200,
                0,
                0
            ),
            12 // reduce-only rejected: no margin address
        );
    }

    #[test]
    fn test_reduce_only_rejected_no_position() {
        // With margin address set but no position (cross-call returns empty in tests),
        // reduce-only orders should fail
        let (admin, pair_id) = setup_with_pair();
        let margin_addr = [50u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_margin_address(admin.as_ptr(), margin_addr.as_ptr()), 0);

        let trader = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        assert_eq!(
            place_order(
                trader.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_MARKET | REDUCE_ONLY_FLAG,
                1_000_000_000,
                1200,
                0,
                0
            ),
            12 // reduce-only rejected: no open position
        );
    }

    #[test]
    fn test_reduce_only_flag_preserves_base_type() {
        // Verify that ORDER_LIMIT | REDUCE_ONLY_FLAG = 0x80, and
        // base type extraction yields ORDER_LIMIT(0)
        assert_eq!(ORDER_LIMIT | REDUCE_ONLY_FLAG, 0x80);
        assert_eq!((ORDER_LIMIT | REDUCE_ONLY_FLAG) & 0x7F, ORDER_LIMIT);
        assert_eq!((ORDER_MARKET | REDUCE_ONLY_FLAG) & 0x7F, ORDER_MARKET);
        assert_eq!((ORDER_STOP_LIMIT | REDUCE_ONLY_FLAG) & 0x7F, ORDER_STOP_LIMIT);
        assert_eq!((ORDER_POST_ONLY | REDUCE_ONLY_FLAG) & 0x7F, ORDER_POST_ONLY);
    }

    #[test]
    fn test_set_margin_address() {
        let admin = setup();
        let margin = [42u8; 32];
        assert_eq!(set_margin_address(admin.as_ptr(), margin.as_ptr()), 0);
        assert_eq!(load_addr(MARGIN_ADDRESS_KEY.as_bytes()), margin);
    }

    #[test]
    fn test_set_margin_address_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        let margin = [42u8; 32];
        test_mock::set_caller(rando);
        assert_eq!(set_margin_address(rando.as_ptr(), margin.as_ptr()), 1);
    }

    #[test]
    fn test_normal_order_unaffected_by_reduce_only_feature() {
        // Standard limit order (no reduce-only flag) should still work normally
        let (_admin, pair_id) = setup_with_pair();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        assert_eq!(
            place_order(
                trader.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_LIMIT,
                1_000_000_000,
                1200,
                0,
                0
            ),
            0
        );
    }

    // --- Cancel & Modify ---

    #[test]
    fn test_cancel_order() {
        let (_admin, pair_id) = setup_with_pair();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        place_order(
            trader.as_ptr(),
            pair_id,
            SIDE_BUY,
            ORDER_LIMIT,
            1_000_000_000,
            1000,
            0,
            0
        );
        assert_eq!(cancel_order(trader.as_ptr(), 1), 0);
        let data = storage_get(&order_key(1)).unwrap();
        assert_eq!(decode_order_status(&data), STATUS_CANCELLED);
    }

    #[test]
    fn test_cancel_not_owner() {
        let (_admin, pair_id) = setup_with_pair();
        let trader = [2u8; 32];
        let other = [3u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        place_order(
            trader.as_ptr(),
            pair_id,
            SIDE_BUY,
            ORDER_LIMIT,
            1_000_000_000,
            1000,
            0,
            0
        );
        test_mock::set_caller(other);
        assert_eq!(cancel_order(other.as_ptr(), 1), 2);
    }

    #[test]
    fn test_cancel_already_cancelled() {
        let (_admin, pair_id) = setup_with_pair();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        place_order(
            trader.as_ptr(),
            pair_id,
            SIDE_BUY,
            ORDER_LIMIT,
            1_000_000_000,
            1000,
            0,
            0
        );
        assert_eq!(cancel_order(trader.as_ptr(), 1), 0);
        assert_eq!(cancel_order(trader.as_ptr(), 1), 3);
    }

    #[test]
    fn test_cancel_all_orders() {
        let (_admin, pair_id) = setup_with_pair();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        place_order(
            trader.as_ptr(),
            pair_id,
            SIDE_BUY,
            ORDER_LIMIT,
            1_000_000_000,
            1000,
            0,
            0
        );
        place_order(
            trader.as_ptr(),
            pair_id,
            SIDE_BUY,
            ORDER_LIMIT,
            2_000_000_000,
            1000,
            0,
            0
        );
        assert_eq!(cancel_all_orders(trader.as_ptr(), pair_id), 0);
        let d1 = storage_get(&order_key(1)).unwrap();
        let d2 = storage_get(&order_key(2)).unwrap();
        assert_eq!(decode_order_status(&d1), STATUS_CANCELLED);
        assert_eq!(decode_order_status(&d2), STATUS_CANCELLED);
    }

    #[test]
    fn test_modify_order() {
        let (_admin, pair_id) = setup_with_pair();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        place_order(
            trader.as_ptr(),
            pair_id,
            SIDE_BUY,
            ORDER_LIMIT,
            1_000_000_000,
            1000,
            0,
            0
        );
        assert_eq!(modify_order(trader.as_ptr(), 1, 2_000_000_000, 2000), 0);
        // Old order cancelled
        let d1 = storage_get(&order_key(1)).unwrap();
        assert_eq!(decode_order_status(&d1), STATUS_CANCELLED);
        // New order placed
        let d2 = storage_get(&order_key(2)).unwrap();
        assert_eq!(decode_order_price(&d2), 2_000_000_000);
        assert_eq!(decode_order_quantity(&d2), 2000);
    }

    // --- Emergency Pause ---

    #[test]
    fn test_emergency_pause() {
        let admin = setup();
        // Pause is instant
        assert_eq!(emergency_pause(admin.as_ptr()), 0);
        assert!(is_paused());
        // AUDIT-FIX M12: Unpause is now a two-step process with timelock
        // Step 1: Schedule unpause
        assert_eq!(emergency_unpause(admin.as_ptr()), 0);
        // Still paused — timelock hasn't elapsed
        assert!(is_paused());
        // Step 2: Try executing before timelock — should fail (return code 4)
        assert_eq!(execute_unpause(admin.as_ptr()), 4);
        assert!(is_paused());
        // Step 3: Advance slot past timelock and execute
        test_mock::set_slot(1 + UNPAUSE_TIMELOCK_SLOTS);
        assert_eq!(execute_unpause(admin.as_ptr()), 0);
        assert!(!is_paused());
    }

    #[test]
    fn test_emergency_pause_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        test_mock::set_caller(rando);
        assert_eq!(emergency_pause(rando.as_ptr()), 1);
    }

    // --- Query Functions ---

    #[test]
    fn test_get_pair_info() {
        let (_admin, pair_id) = setup_with_pair();
        assert_eq!(get_pair_info(pair_id), pair_id);
        assert_eq!(get_pair_info(999), 0);
    }

    #[test]
    fn test_get_best_bid_ask() {
        let (_admin, pair_id) = setup_with_pair();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        place_order(
            trader.as_ptr(),
            pair_id,
            SIDE_BUY,
            ORDER_LIMIT,
            1_000_000_000,
            1000,
            0,
            0
        );
        assert_eq!(get_best_bid(pair_id), 1_000_000_000);

        let seller = [3u8; 32];
        test_mock::set_caller(seller);
        place_order(
            seller.as_ptr(),
            pair_id,
            SIDE_SELL,
            ORDER_LIMIT,
            2_000_000_000,
            1000,
            0,
            0
        );
        assert_eq!(get_best_ask(pair_id), 2_000_000_000);
    }

    #[test]
    fn test_get_spread() {
        let (_admin, pair_id) = setup_with_pair();
        let buyer = [2u8; 32];
        let seller = [3u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(buyer);
        place_order(
            buyer.as_ptr(),
            pair_id,
            SIDE_BUY,
            ORDER_LIMIT,
            1_000_000_000,
            1000,
            0,
            0
        );
        test_mock::set_caller(seller);
        place_order(
            seller.as_ptr(),
            pair_id,
            SIDE_SELL,
            ORDER_LIMIT,
            2_000_000_000,
            1000,
            0,
            0
        );
        assert_eq!(get_spread(pair_id), 1_000_000_000);
    }

    #[test]
    fn test_fee_calculation() {
        // 1000 shells notional at 5 bps = 0.5 → rounds to MIN_FEE_PER_TRADE = 1
        assert_eq!(calculate_taker_fee(1000, 5), 1);
        // 1_000_000 shells notional at 5 bps = 500
        assert_eq!(calculate_taker_fee(1_000_000, 5), 500);
        // Maker rebate: 1_000_000 at -1 bps = 100
        assert_eq!(calculate_maker_rebate(1_000_000, -1), 100);
    }

    #[test]
    fn test_fee_accumulation() {
        let (_admin, pair_id) = setup_with_pair();
        let seller = [3u8; 32];
        let buyer = [4u8; 32];
        test_mock::set_slot(100);

        test_mock::set_caller(seller);
        place_order(
            seller.as_ptr(),
            pair_id,
            SIDE_SELL,
            ORDER_LIMIT,
            1_000_000_000,
            1_000_000,
            0,
            0
        );
        test_mock::set_caller(buyer);
        place_order(
            buyer.as_ptr(),
            pair_id,
            SIDE_BUY,
            ORDER_LIMIT,
            1_000_000_000,
            1_000_000,
            0,
            0
        );

        let treasury = get_fee_treasury();
        assert!(treasury > 0, "Protocol fees should accumulate");
    }

    // --- Max pairs limit ---

    #[test]
    fn test_max_pairs_limit() {
        let admin = setup();
        for i in 0..MAX_PAIRS {
            let mut base = [0u8; 32];
            base[0] = (i + 1) as u8;
            let mut quote = [0u8; 32];
            quote[0] = (i + 100) as u8;
            assert_eq!(
                create_pair(
                    admin.as_ptr(),
                    base.as_ptr(),
                    quote.as_ptr(),
                    1000,
                    100,
                    1000
                ),
                0
            );
        }
        // 51st pair should fail
        let base = [200u8; 32];
        let quote = [201u8; 32];
        assert_eq!(
            create_pair(
                admin.as_ptr(),
                base.as_ptr(),
                quote.as_ptr(),
                1000,
                100,
                1000
            ),
            3
        );
    }

    // --- Price-time priority ---

    #[test]
    fn test_price_time_priority() {
        let (_admin, pair_id) = setup_with_pair();
        let seller1 = [3u8; 32];
        let seller2 = [4u8; 32];
        let buyer = [5u8; 32];
        test_mock::set_slot(100);

        // Two asks at same price — seller1 first (qty large enough for min notional)
        test_mock::set_caller(seller1);
        assert_eq!(
            place_order(
                seller1.as_ptr(),
                pair_id,
                SIDE_SELL,
                ORDER_LIMIT,
                1_000_000_000,
                10_000,
                0,
                0
            ),
            0
        );
        test_mock::set_caller(seller2);
        assert_eq!(
            place_order(
                seller2.as_ptr(),
                pair_id,
                SIDE_SELL,
                ORDER_LIMIT,
                1_000_000_000,
                10_000,
                0,
                0
            ),
            0
        );

        // Buyer takes 10_000 — should fill seller1 first (time priority)
        test_mock::set_caller(buyer);
        assert_eq!(
            place_order(
                buyer.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_LIMIT,
                1_000_000_000,
                10_000,
                0,
                0
            ),
            0
        );

        let s1 = storage_get(&order_key(1)).unwrap();
        let s2 = storage_get(&order_key(2)).unwrap();
        assert_eq!(decode_order_status(&s1), STATUS_FILLED);
        assert_eq!(decode_order_status(&s2), STATUS_OPEN); // untouched
    }

    // --- Queries ---

    #[test]
    fn test_get_order() {
        let (_admin, pair_id) = setup_with_pair();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        place_order(
            trader.as_ptr(),
            pair_id,
            SIDE_BUY,
            ORDER_LIMIT,
            1_000_000_000,
            1000,
            0,
            0
        );
        assert_eq!(get_order(1), 1);
        assert_eq!(get_order(999), 0);
    }

    #[test]
    fn test_get_trade_count() {
        let (_admin, pair_id) = setup_with_pair();
        let seller = [3u8; 32];
        let buyer = [4u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(seller);
        place_order(
            seller.as_ptr(),
            pair_id,
            SIDE_SELL,
            ORDER_LIMIT,
            1_000_000_000,
            1000,
            0,
            0
        );
        test_mock::set_caller(buyer);
        place_order(
            buyer.as_ptr(),
            pair_id,
            SIDE_BUY,
            ORDER_LIMIT,
            1_000_000_000,
            1000,
            0,
            0
        );
        assert_eq!(get_trade_count(), 1);
    }

    #[test]
    fn test_get_pair_count() {
        let admin = setup();
        assert_eq!(get_pair_count(), 0);
        let base = [10u8; 32];
        let quote = [20u8; 32];
        create_pair(
            admin.as_ptr(),
            base.as_ptr(),
            quote.as_ptr(),
            1000,
            100,
            1000,
        );
        assert_eq!(get_pair_count(), 1);
    }

    // --- Preferred quote currency (mUSD enforcement) ---

    #[test]
    fn test_set_preferred_quote() {
        let admin = setup();
        let musd = [42u8; 32];
        assert_eq!(set_preferred_quote(admin.as_ptr(), musd.as_ptr()), 0);
        assert_eq!(get_preferred_quote(), 1); // 1 = preferred is set
        assert_eq!(get_allowed_quote_count(), 1); // sets exactly one allowed
    }

    #[test]
    fn test_set_preferred_quote_not_admin() {
        let _admin = setup();
        let non_admin = [99u8; 32];
        let musd = [42u8; 32];
        test_mock::set_caller(non_admin);
        assert_eq!(set_preferred_quote(non_admin.as_ptr(), musd.as_ptr()), 1);
    }

    #[test]
    fn test_set_preferred_quote_zero_address() {
        let admin = setup();
        let zero = [0u8; 32];
        assert_eq!(set_preferred_quote(admin.as_ptr(), zero.as_ptr()), 2);
    }

    #[test]
    fn test_create_pair_enforces_preferred_quote() {
        let admin = setup();
        let musd = [42u8; 32];
        set_preferred_quote(admin.as_ptr(), musd.as_ptr());
        let base = [10u8; 32];
        // Quote matches preferred → success
        assert_eq!(
            create_pair(
                admin.as_ptr(),
                base.as_ptr(),
                musd.as_ptr(),
                1000,
                100,
                1000
            ),
            0
        );
        // Wrong quote → error 6
        let wrong_quote = [99u8; 32];
        let base2 = [11u8; 32];
        assert_eq!(
            create_pair(
                admin.as_ptr(),
                base2.as_ptr(),
                wrong_quote.as_ptr(),
                1000,
                100,
                1000
            ),
            6
        );
    }

    #[test]
    fn test_add_allowed_quote() {
        let admin = setup();
        let musd = [42u8; 32];
        let molt = [43u8; 32];
        assert_eq!(add_allowed_quote(admin.as_ptr(), musd.as_ptr()), 0);
        assert_eq!(add_allowed_quote(admin.as_ptr(), molt.as_ptr()), 0);
        assert_eq!(get_allowed_quote_count(), 2);
        // Duplicate rejected
        assert_eq!(add_allowed_quote(admin.as_ptr(), musd.as_ptr()), 3);
    }

    #[test]
    fn test_remove_allowed_quote() {
        let admin = setup();
        let musd = [42u8; 32];
        let molt = [43u8; 32];
        add_allowed_quote(admin.as_ptr(), musd.as_ptr());
        add_allowed_quote(admin.as_ptr(), molt.as_ptr());
        assert_eq!(get_allowed_quote_count(), 2);
        assert_eq!(remove_allowed_quote(admin.as_ptr(), musd.as_ptr()), 0);
        assert_eq!(get_allowed_quote_count(), 1);
        // Not found
        assert_eq!(remove_allowed_quote(admin.as_ptr(), musd.as_ptr()), 2);
    }

    #[test]
    fn test_dual_quote_enforcement() {
        let admin = setup();
        let musd = [42u8; 32];
        let molt = [43u8; 32];
        let wrong = [99u8; 32];
        // Add both mUSD and MOLT as allowed quotes
        add_allowed_quote(admin.as_ptr(), musd.as_ptr());
        add_allowed_quote(admin.as_ptr(), molt.as_ptr());
        let base1 = [10u8; 32];
        let base2 = [11u8; 32];
        let base3 = [12u8; 32];
        // TOKEN/mUSD → OK
        assert_eq!(create_pair(admin.as_ptr(), base1.as_ptr(), musd.as_ptr(), 1000, 100, 1000), 0);
        // TOKEN/MOLT → OK
        assert_eq!(create_pair(admin.as_ptr(), base2.as_ptr(), molt.as_ptr(), 1000, 100, 1000), 0);
        // TOKEN/random → rejected
        assert_eq!(create_pair(admin.as_ptr(), base3.as_ptr(), wrong.as_ptr(), 1000, 100, 1000), 6);
    }

    #[test]
    fn test_create_pair_no_preferred_quote_allows_any() {
        let admin = setup();
        let base = [10u8; 32];
        let quote = [20u8; 32];
        // No preferred set → any quote accepted
        assert_eq!(
            create_pair(
                admin.as_ptr(),
                base.as_ptr(),
                quote.as_ptr(),
                1000,
                100,
                1000
            ),
            0
        );
    }

    #[test]
    fn test_get_preferred_quote_unset() {
        let _admin = setup();
        assert_eq!(get_preferred_quote(), 0); // 0 = not set
    }

    // AUDIT-FIX P2: Security regression test
    #[test]
    fn test_duplicate_pair_rejected() {
        let admin = setup();
        let base = [10u8; 32];
        let quote = [20u8; 32];

        // First pair creation should succeed
        let result1 = create_pair(
            admin.as_ptr(),
            base.as_ptr(),
            quote.as_ptr(),
            1000,
            100,
            1000,
        );
        assert_eq!(result1, 0, "first create_pair should succeed");

        // Second creation with same base/quote should fail with error 7
        let result2 = create_pair(
            admin.as_ptr(),
            base.as_ptr(),
            quote.as_ptr(),
            1000,
            100,
            1000,
        );
        assert_ne!(result2, 0, "duplicate pair must be rejected");
        assert_eq!(result2, 7, "duplicate pair should return error code 7");
    }

    // --- K3-03: Full DEX Trading Lifecycle E2E ---

    #[test]
    fn test_full_trading_lifecycle_deposit_trade_withdraw() {
        // K3-03: Complete lifecycle: create pair → place sell → place buy (match)
        //        → verify trade → place new order → cancel → verify final state
        let (admin, pair_id) = setup_with_pair();
        let seller = [3u8; 32];
        let buyer = [4u8; 32];
        test_mock::set_slot(100);

        // --- Step 1: Seller places limit SELL at 1_000_000_000, qty 100_000 ---
        // Use large quantity so fees exceed 0 (notional = P*Q/1e9 = 100_000)
        test_mock::set_caller(seller);
        assert_eq!(
            place_order(
                seller.as_ptr(),
                pair_id,
                SIDE_SELL,
                ORDER_LIMIT,
                1_000_000_000,
                100_000,
                0,
                0
            ),
            0,
            "sell order should place successfully"
        );

        // Verify order #1 is OPEN
        let order1 = storage_get(&order_key(1)).unwrap();
        assert_eq!(decode_order_status(&order1), STATUS_OPEN);
        assert_eq!(decode_order_side(&order1), SIDE_SELL);
        assert_eq!(decode_order_price(&order1), 1_000_000_000);
        assert_eq!(decode_order_quantity(&order1), 100_000);
        assert_eq!(decode_order_filled(&order1), 0);

        // Verify seller's open order count = 1
        assert_eq!(load_u64(&user_order_count_key(&seller)), 1);

        // No trades yet
        assert_eq!(load_u64(TRADE_COUNT_KEY), 0);
        assert_eq!(load_u64(TOTAL_VOLUME_KEY), 0);
        assert_eq!(load_u64(FEE_TREASURY_KEY), 0);

        // --- Step 2: Buyer places limit BUY at same price → auto-match ---
        test_mock::set_caller(buyer);
        assert_eq!(
            place_order(
                buyer.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_LIMIT,
                1_000_000_000,
                100_000,
                0,
                0
            ),
            0,
            "buy order should place and match"
        );

        // --- Step 3: Verify trade executed ---
        assert_eq!(load_u64(TRADE_COUNT_KEY), 1, "exactly 1 trade should execute");
        assert!(load_u64(TOTAL_VOLUME_KEY) > 0, "volume must increase after trade");
        assert!(load_u64(FEE_TREASURY_KEY) > 0, "fee treasury must accumulate");

        // Both orders should be FILLED
        let sell_data = storage_get(&order_key(1)).unwrap();
        assert_eq!(decode_order_status(&sell_data), STATUS_FILLED);
        assert_eq!(decode_order_filled(&sell_data), 100_000);

        let buy_data = storage_get(&order_key(2)).unwrap();
        assert_eq!(decode_order_status(&buy_data), STATUS_FILLED);
        assert_eq!(decode_order_filled(&buy_data), 100_000);

        // --- Step 4: Buyer places another order (resting) ---
        // Price 900M, qty 1200 → notional = 1080 ≥ min_order(1000)
        test_mock::set_caller(buyer);
        assert_eq!(
            place_order(
                buyer.as_ptr(),
                pair_id,
                SIDE_BUY,
                ORDER_LIMIT,
                900_000_000,
                1200,
                0,
                0
            ),
            0,
            "second buy order should place (resting)"
        );

        let order3 = storage_get(&order_key(3)).unwrap();
        assert_eq!(decode_order_status(&order3), STATUS_OPEN);
        assert_eq!(decode_order_price(&order3), 900_000_000);

        // Buyer should have open orders
        let buyer_count = load_u64(&user_order_count_key(&buyer));
        assert!(buyer_count > 0, "buyer should have open orders");

        // --- Step 5: Cancel the resting order ("withdraw" from the orderbook) ---
        test_mock::set_caller(buyer);
        assert_eq!(cancel_order(buyer.as_ptr(), 3), 0, "cancel should succeed");

        let cancelled = storage_get(&order_key(3)).unwrap();
        assert_eq!(decode_order_status(&cancelled), STATUS_CANCELLED);

        // --- Step 6: Final state verification ---
        assert_eq!(load_u64(TRADE_COUNT_KEY), 1);
        assert_eq!(load_u64(PAIR_COUNT_KEY), 1);
    }

    #[test]
    fn test_multi_order_partial_fill_lifecycle() {
        // K3-03: Multiple participants, partial fills, then cleanup
        let (_admin, pair_id) = setup_with_pair();
        let seller = [3u8; 32];
        let buyer_a = [4u8; 32];
        let buyer_b = [5u8; 32];
        test_mock::set_slot(200);

        // Seller posts 2000 units
        test_mock::set_caller(seller);
        assert_eq!(
            place_order(seller.as_ptr(), pair_id, SIDE_SELL, ORDER_LIMIT, 1_000_000_000, 2000, 0, 0),
            0
        );

        // Buyer A takes 1000 → partial fill
        test_mock::set_caller(buyer_a);
        assert_eq!(
            place_order(buyer_a.as_ptr(), pair_id, SIDE_BUY, ORDER_LIMIT, 1_000_000_000, 1000, 0, 0),
            0
        );

        // Sell order should be partially filled
        let sell_data = storage_get(&order_key(1)).unwrap();
        assert_eq!(decode_order_status(&sell_data), STATUS_PARTIAL);
        assert_eq!(decode_order_filled(&sell_data), 1000);

        assert_eq!(load_u64(TRADE_COUNT_KEY), 1);

        // Buyer B takes remaining 1000 → fully fills seller
        test_mock::set_caller(buyer_b);
        assert_eq!(
            place_order(buyer_b.as_ptr(), pair_id, SIDE_BUY, ORDER_LIMIT, 1_000_000_000, 1000, 0, 0),
            0
        );

        let sell_final = storage_get(&order_key(1)).unwrap();
        assert_eq!(decode_order_status(&sell_final), STATUS_FILLED);
        assert_eq!(decode_order_filled(&sell_final), 2000);

        assert_eq!(load_u64(TRADE_COUNT_KEY), 2, "two trades should have executed");

        let total_vol = load_u64(TOTAL_VOLUME_KEY);
        assert!(total_vol > 0, "cumulative volume must be positive");
    }
}
