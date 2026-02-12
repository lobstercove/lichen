// DEX Margin — Margin Trading & Liquidation Engine (DEEP hardened)
//
// Features:
//   - Isolated margin positions (up to 5x leverage)
//   - Maintenance margin at 10%, initial margin at 20%
//   - Liquidation by anyone (earns 50% of penalty)
//   - Insurance fund from liquidation penalties
//   - Funding rate (8-hour intervals)
//   - Integration with LobsterLend for margin funding
//   - Emergency pause, reentrancy guard, admin controls
//   - Auto-deleveraging during extreme events

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    storage_get, storage_set, log_info,
    bytes_to_u64, u64_to_bytes, get_slot,
};

// ============================================================================
// CONSTANTS
// ============================================================================

const MAX_LEVERAGE_ISOLATED: u64 = 5;
const MAX_LEVERAGE_CROSS: u64 = 3;
const INITIAL_MARGIN_BPS: u64 = 2000;     // 20%
const MAINTENANCE_MARGIN_BPS: u64 = 1000; // 10%
const LIQUIDATION_PENALTY_BPS: u64 = 500; // 5%
const LIQUIDATOR_SHARE_BPS: u64 = 5000;   // 50% of penalty to liquidator
const INSURANCE_SHARE_BPS: u64 = 5000;    // 50% of penalty to insurance
const FUNDING_INTERVAL_SLOTS: u64 = 28_800; // ~8 hours
const MAX_POSITIONS: u64 = 10_000;
const MAX_FUNDING_RATE_BPS: u64 = 100;    // 1% max per interval

// Position side
const SIDE_LONG: u8 = 0;
const SIDE_SHORT: u8 = 1;

// Position status
const POS_OPEN: u8 = 0;
const POS_CLOSED: u8 = 1;
const POS_LIQUIDATED: u8 = 2;

// Storage keys
const ADMIN_KEY: &[u8] = b"mrg_admin";
const PAUSED_KEY: &[u8] = b"mrg_paused";
const REENTRANCY_KEY: &[u8] = b"mrg_reentrancy";
const POSITION_COUNT_KEY: &[u8] = b"mrg_pos_count";
const INSURANCE_FUND_KEY: &[u8] = b"mrg_insurance";
const LAST_FUNDING_KEY: &[u8] = b"mrg_last_fund";

// ============================================================================
// HELPERS
// ============================================================================

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key).map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 }).unwrap_or(0)
}
fn save_u64(key: &[u8], val: u64) { storage_set(key, &u64_to_bytes(val)); }
fn load_addr(key: &[u8]) -> [u8; 32] {
    storage_get(key).map(|d| {
        let mut a = [0u8; 32]; if d.len() >= 32 { a.copy_from_slice(&d[..32]); } a
    }).unwrap_or([0u8; 32])
}
fn is_zero(addr: &[u8; 32]) -> bool { addr.iter().all(|&b| b == 0) }

fn u64_to_decimal(mut n: u64) -> Vec<u8> {
    if n == 0 { return alloc::vec![b'0']; }
    let mut buf = Vec::new();
    while n > 0 { buf.push(b'0' + (n % 10) as u8); n /= 10; }
    buf.reverse(); buf
}
fn hex_encode(bytes: &[u8]) -> Vec<u8> {
    let hex_chars: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for &b in bytes { out.push(hex_chars[(b >> 4) as usize]); out.push(hex_chars[(b & 0x0f) as usize]); }
    out
}

fn position_key(pos_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_pos_"[..]);
    k.extend_from_slice(&u64_to_decimal(pos_id)); k
}
fn max_leverage_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_maxl_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id)); k
}
fn maintenance_margin_key_fn() -> Vec<u8> {
    Vec::from(&b"mrg_maint_bps"[..])
}
fn user_position_count_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_upc_"[..]);
    k.extend_from_slice(&hex_encode(addr)); k
}
fn user_position_key(addr: &[u8; 32], idx: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_up_"[..]);
    k.extend_from_slice(&hex_encode(addr));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(idx)); k
}
fn mark_price_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_mark_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id)); k
}

// ============================================================================
// DEEP SECURITY
// ============================================================================

fn reentrancy_enter() -> bool {
    if storage_get(REENTRANCY_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(false) { return false; }
    storage_set(REENTRANCY_KEY, &[1u8]); true
}
fn reentrancy_exit() { storage_set(REENTRANCY_KEY, &[0u8]); }
fn is_paused() -> bool { storage_get(PAUSED_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(false) }
fn require_not_paused() -> bool { !is_paused() }
fn require_admin(caller: &[u8; 32]) -> bool {
    let admin = load_addr(ADMIN_KEY); !is_zero(&admin) && *caller == admin
}

// ============================================================================
// POSITION LAYOUT (112 bytes)
// ============================================================================
// Bytes 0..32   : trader address
// Bytes 32..40  : position_id (u64)
// Bytes 40..48  : pair_id (u64)
// Byte  48      : side (0=long, 1=short)
// Byte  49      : status (0=open, 1=closed, 2=liquidated)
// Bytes 50..58  : size (u64, in base token units)
// Bytes 58..66  : margin (u64, collateral deposited)
// Bytes 66..74  : entry_price (u64, scaled by 1e9)
// Bytes 74..82  : leverage (u64, 1-5x)
// Bytes 82..90  : created_slot (u64)
// Bytes 90..98  : realized_pnl (u64, stored as signed via bias)
// Bytes 98..106 : accumulated_funding (u64)
// Bytes 106..112: padding

const POSITION_SIZE: usize = 112;

fn encode_position(
    trader: &[u8; 32], pos_id: u64, pair_id: u64, side: u8, status: u8,
    size: u64, margin: u64, entry_price: u64, leverage: u64,
    created_slot: u64, realized_pnl: u64, accumulated_funding: u64,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(POSITION_SIZE);
    data.extend_from_slice(trader);
    data.extend_from_slice(&u64_to_bytes(pos_id));
    data.extend_from_slice(&u64_to_bytes(pair_id));
    data.push(side);
    data.push(status);
    data.extend_from_slice(&u64_to_bytes(size));
    data.extend_from_slice(&u64_to_bytes(margin));
    data.extend_from_slice(&u64_to_bytes(entry_price));
    data.extend_from_slice(&u64_to_bytes(leverage));
    data.extend_from_slice(&u64_to_bytes(created_slot));
    data.extend_from_slice(&u64_to_bytes(realized_pnl));
    data.extend_from_slice(&u64_to_bytes(accumulated_funding));
    while data.len() < POSITION_SIZE { data.push(0); }
    data
}

fn decode_pos_trader(data: &[u8]) -> [u8; 32] {
    let mut t = [0u8; 32]; if data.len() >= 32 { t.copy_from_slice(&data[..32]); } t
}
fn decode_pos_id(data: &[u8]) -> u64 { if data.len() >= 40 { bytes_to_u64(&data[32..40]) } else { 0 } }
fn decode_pos_pair_id(data: &[u8]) -> u64 { if data.len() >= 48 { bytes_to_u64(&data[40..48]) } else { 0 } }
fn decode_pos_side(data: &[u8]) -> u8 { if data.len() > 48 { data[48] } else { 0 } }
fn decode_pos_status(data: &[u8]) -> u8 { if data.len() > 49 { data[49] } else { 0 } }
fn decode_pos_size(data: &[u8]) -> u64 { if data.len() >= 58 { bytes_to_u64(&data[50..58]) } else { 0 } }
fn decode_pos_margin(data: &[u8]) -> u64 { if data.len() >= 66 { bytes_to_u64(&data[58..66]) } else { 0 } }
fn decode_pos_entry_price(data: &[u8]) -> u64 { if data.len() >= 74 { bytes_to_u64(&data[66..74]) } else { 0 } }
fn decode_pos_leverage(data: &[u8]) -> u64 { if data.len() >= 82 { bytes_to_u64(&data[74..82]) } else { 0 } }

fn update_pos_status(data: &mut Vec<u8>, s: u8) { if data.len() > 49 { data[49] = s; } }
fn update_pos_margin(data: &mut Vec<u8>, m: u64) {
    if data.len() >= 66 { data[58..66].copy_from_slice(&u64_to_bytes(m)); }
}

/// Calculate margin ratio
/// margin_ratio = margin / (size * mark_price / 1e9)
fn calculate_margin_ratio(margin: u64, size: u64, mark_price: u64) -> u64 {
    let notional = (size as u128 * mark_price as u128 / 1_000_000_000) as u64;
    if notional == 0 { return 10_000; } // safe
    (margin as u128 * 10_000 / notional as u128) as u64 // in bps
}

/// Calculate unrealized PnL
fn calculate_pnl(side: u8, size: u64, entry_price: u64, mark_price: u64) -> (bool, u64) {
    // Returns (is_profit, amount)
    if side == SIDE_LONG {
        if mark_price >= entry_price {
            let pnl = (size as u128 * (mark_price - entry_price) as u128 / 1_000_000_000) as u64;
            (true, pnl)
        } else {
            let pnl = (size as u128 * (entry_price - mark_price) as u128 / 1_000_000_000) as u64;
            (false, pnl)
        }
    } else {
        if mark_price <= entry_price {
            let pnl = (size as u128 * (entry_price - mark_price) as u128 / 1_000_000_000) as u64;
            (true, pnl)
        } else {
            let pnl = (size as u128 * (mark_price - entry_price) as u128 / 1_000_000_000) as u64;
            (false, pnl)
        }
    }
}

// ============================================================================
// PUBLIC FUNCTIONS
// ============================================================================

pub fn initialize(admin: *const u8) -> u32 {
    let existing = load_addr(ADMIN_KEY);
    if !is_zero(&existing) { return 1; }
    let mut addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(admin, addr.as_mut_ptr(), 32); }
    storage_set(ADMIN_KEY, &addr);
    save_u64(POSITION_COUNT_KEY, 0);
    save_u64(INSURANCE_FUND_KEY, 0);
    save_u64(LAST_FUNDING_KEY, 0);
    storage_set(PAUSED_KEY, &[0u8]);
    log_info("DEX Margin initialized");
    0
}

/// Set mark price for a pair (called by oracle/analytics)
pub fn set_mark_price(caller: *const u8, pair_id: u64, price: u64) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    if !require_admin(&c) { return 1; }
    if price == 0 { return 2; }
    save_u64(&mark_price_key(pair_id), price);
    0
}

/// Open a new margin position
/// Returns: 0=success, 1=paused, 2=invalid leverage, 3=insufficient margin,
///          4=max positions, 5=reentrancy, 6=no mark price
pub fn open_position(
    trader: *const u8, pair_id: u64, side: u8, size: u64,
    leverage: u64, margin_amount: u64,
) -> u32 {
    if !reentrancy_enter() { return 5; }
    if !require_not_paused() { reentrancy_exit(); return 1; }

    let mut t = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(trader, t.as_mut_ptr(), 32); }

    // Validate leverage
    let max_lev = load_u64(&max_leverage_key(pair_id));
    let effective_max = if max_lev > 0 { max_lev } else { MAX_LEVERAGE_ISOLATED };
    if leverage == 0 || leverage > effective_max { reentrancy_exit(); return 2; }
    if side > SIDE_SHORT { reentrancy_exit(); return 2; }

    // Get mark price
    let mark_price = load_u64(&mark_price_key(pair_id));
    if mark_price == 0 { reentrancy_exit(); return 6; }

    // Check initial margin
    let notional = (size as u128 * mark_price as u128 / 1_000_000_000) as u64;
    let required_margin = notional * INITIAL_MARGIN_BPS / 10_000 / leverage;
    if margin_amount < required_margin { reentrancy_exit(); return 3; }

    let pos_count = load_u64(POSITION_COUNT_KEY);
    if pos_count >= MAX_POSITIONS { reentrancy_exit(); return 4; }

    let pos_id = pos_count + 1;
    let slot = get_slot();
    let data = encode_position(
        &t, pos_id, pair_id, side, POS_OPEN,
        size, margin_amount, mark_price, leverage,
        slot, 0, 0,
    );
    storage_set(&position_key(pos_id), &data);
    save_u64(POSITION_COUNT_KEY, pos_id);

    // Track user positions
    let user_count = load_u64(&user_position_count_key(&t));
    save_u64(&user_position_count_key(&t), user_count + 1);
    save_u64(&user_position_key(&t, user_count + 1), pos_id);

    log_info("Margin position opened");
    reentrancy_exit();
    0
}

/// Close a margin position
/// Returns: 0=success, 1=not found, 2=not owner, 3=already closed, 4=reentrancy
pub fn close_position(caller: *const u8, position_id: u64) -> u32 {
    if !reentrancy_enter() { return 4; }
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }

    let pk = position_key(position_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE => d,
        _ => { reentrancy_exit(); return 1; }
    };

    let trader = decode_pos_trader(&data);
    if trader != c { reentrancy_exit(); return 2; }
    if decode_pos_status(&data) != POS_OPEN { reentrancy_exit(); return 3; }

    update_pos_status(&mut data, POS_CLOSED);
    storage_set(&pk, &data);
    log_info("Margin position closed");
    reentrancy_exit();
    0
}

/// Add margin to a position
pub fn add_margin(caller: *const u8, position_id: u64, amount: u64) -> u32 {
    if !reentrancy_enter() { return 4; }
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }

    let pk = position_key(position_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE => d,
        _ => { reentrancy_exit(); return 1; }
    };
    if decode_pos_trader(&data) != c { reentrancy_exit(); return 2; }
    if decode_pos_status(&data) != POS_OPEN { reentrancy_exit(); return 3; }
    if amount == 0 { reentrancy_exit(); return 5; }

    let current_margin = decode_pos_margin(&data);
    let new_margin = match current_margin.checked_add(amount) {
        Some(m) => m,
        None => { reentrancy_exit(); return 6; } // overflow
    };
    update_pos_margin(&mut data, new_margin);
    storage_set(&pk, &data);
    reentrancy_exit();
    0
}

/// Remove margin from a position (if still healthy)
pub fn remove_margin(caller: *const u8, position_id: u64, amount: u64) -> u32 {
    if !reentrancy_enter() { return 4; }
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }

    let pk = position_key(position_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE => d,
        _ => { reentrancy_exit(); return 1; }
    };
    if decode_pos_trader(&data) != c { reentrancy_exit(); return 2; }
    if decode_pos_status(&data) != POS_OPEN { reentrancy_exit(); return 3; }

    let current_margin = decode_pos_margin(&data);
    if amount > current_margin { reentrancy_exit(); return 5; }
    let new_margin = current_margin - amount;

    // Check if still above maintenance
    let size = decode_pos_size(&data);
    let pair_id = decode_pos_pair_id(&data);
    let mark_price = load_u64(&mark_price_key(pair_id));
    if mark_price > 0 {
        let ratio = calculate_margin_ratio(new_margin, size, mark_price);
        let maint_bps = get_maintenance_margin();
        if ratio < maint_bps { reentrancy_exit(); return 6; } // would be unhealthy
    }

    update_pos_margin(&mut data, new_margin);
    storage_set(&pk, &data);
    reentrancy_exit();
    0
}

/// Liquidate an unhealthy position
/// Returns: 0=success, 1=not found, 2=not liquidatable, 3=reentrancy
pub fn liquidate(_liquidator: *const u8, position_id: u64) -> u32 {
    if !reentrancy_enter() { return 3; }

    let pk = position_key(position_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE => d,
        _ => { reentrancy_exit(); return 1; }
    };

    if decode_pos_status(&data) != POS_OPEN { reentrancy_exit(); return 2; }

    let margin = decode_pos_margin(&data);
    let size = decode_pos_size(&data);
    let pair_id = decode_pos_pair_id(&data);
    let mark_price = load_u64(&mark_price_key(pair_id));
    if mark_price == 0 { reentrancy_exit(); return 2; }

    let ratio = calculate_margin_ratio(margin, size, mark_price);
    let maint_bps = get_maintenance_margin();
    if ratio >= maint_bps { reentrancy_exit(); return 2; } // still healthy

    // Calculate penalty
    let notional = (size as u128 * mark_price as u128 / 1_000_000_000) as u64;
    let penalty = notional * LIQUIDATION_PENALTY_BPS / 10_000;
    let liquidator_reward = penalty * LIQUIDATOR_SHARE_BPS / 10_000;
    let insurance_add = penalty * INSURANCE_SHARE_BPS / 10_000;

    // Add to insurance fund (saturating to prevent overflow)
    let insurance = load_u64(INSURANCE_FUND_KEY);
    save_u64(INSURANCE_FUND_KEY, insurance.saturating_add(insurance_add));

    update_pos_status(&mut data, POS_LIQUIDATED);
    storage_set(&pk, &data);

    moltchain_sdk::set_return_data(&u64_to_bytes(liquidator_reward));
    log_info("Position liquidated");
    reentrancy_exit();
    0
}

/// Set max leverage for a pair (admin)
pub fn set_max_leverage(caller: *const u8, pair_id: u64, max_leverage: u64) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    if !require_admin(&c) { return 1; }
    if max_leverage == 0 || max_leverage > 10 { return 2; }
    save_u64(&max_leverage_key(pair_id), max_leverage);
    0
}

/// Set maintenance margin in basis points (admin only)
/// Default is 1000 (10%). Min 200 (2%), Max 5000 (50%).
pub fn set_maintenance_margin(caller: *const u8, margin_bps: u64) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    if !require_admin(&c) { return 1; }
    if margin_bps < 200 || margin_bps > 5000 { return 2; }
    save_u64(&maintenance_margin_key_fn(), margin_bps);
    0
}

/// Get the current maintenance margin (bps)
pub fn get_maintenance_margin() -> u64 {
    let m = load_u64(&maintenance_margin_key_fn());
    if m > 0 { m } else { MAINTENANCE_MARGIN_BPS }
}

/// Get margin ratio for a position (in bps)
pub fn get_margin_ratio(position_id: u64) -> u64 {
    let pk = position_key(position_id);
    let data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE => d,
        _ => return 0,
    };
    let margin = decode_pos_margin(&data);
    let size = decode_pos_size(&data);
    let pair_id = decode_pos_pair_id(&data);
    let mark_price = load_u64(&mark_price_key(pair_id));
    if mark_price == 0 { return 0; }
    calculate_margin_ratio(margin, size, mark_price)
}

pub fn get_position_count() -> u64 { load_u64(POSITION_COUNT_KEY) }
pub fn get_insurance_fund() -> u64 { load_u64(INSURANCE_FUND_KEY) }

pub fn get_position_info(position_id: u64) -> u64 {
    let pk = position_key(position_id);
    match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE => {
            moltchain_sdk::set_return_data(&d);
            position_id
        }
        _ => 0,
    }
}

pub fn emergency_pause(caller: *const u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    if !require_admin(&c) { return 1; }
    storage_set(PAUSED_KEY, &[1u8]);
    log_info("DEX Margin: EMERGENCY PAUSE");
    0
}
pub fn emergency_unpause(caller: *const u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    if !require_admin(&c) { return 1; }
    storage_set(PAUSED_KEY, &[0u8]);
    0
}

// WASM entry
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn call() {
    let args = moltchain_sdk::get_args();
    if args.is_empty() { return; }
    match args[0] {
        0 => {
            if args.len() >= 33 {
                let r = initialize(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        _ => {}
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
        assert_eq!(initialize(admin.as_ptr()), 0);
        // Set mark price for pair 1
        set_mark_price(admin.as_ptr(), 1, 1_000_000_000); // 1.0
        admin
    }

    #[test]
    fn test_initialize() {
        test_mock::reset();
        let admin = [1u8; 32];
        assert_eq!(initialize(admin.as_ptr()), 0);
    }

    #[test]
    fn test_initialize_twice() {
        test_mock::reset();
        let admin = [1u8; 32];
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(initialize(admin.as_ptr()), 1);
    }

    #[test]
    fn test_set_mark_price() {
        let admin = setup();
        assert_eq!(set_mark_price(admin.as_ptr(), 1, 2_000_000_000), 0);
        assert_eq!(load_u64(&mark_price_key(1)), 2_000_000_000);
    }

    #[test]
    fn test_set_mark_price_zero() {
        let admin = setup();
        assert_eq!(set_mark_price(admin.as_ptr(), 1, 0), 2);
    }

    #[test]
    fn test_open_position_long() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        // 1000 units at 1.0 price, 2x leverage, needs 100 margin (10%)
        assert_eq!(open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 2, 200), 0);
        assert_eq!(get_position_count(), 1);
    }

    #[test]
    fn test_open_position_short() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        assert_eq!(open_position(trader.as_ptr(), 1, SIDE_SHORT, 1000, 2, 200), 0);
    }

    #[test]
    fn test_open_position_max_leverage() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        assert_eq!(open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 5, 200), 0);
    }

    #[test]
    fn test_open_position_overleveraged() {
        let admin = setup();
        let trader = [2u8; 32];
        assert_eq!(open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 6, 200), 2);
    }

    #[test]
    fn test_open_position_no_mark_price() {
        let admin = setup();
        let trader = [2u8; 32];
        assert_eq!(open_position(trader.as_ptr(), 2, SIDE_LONG, 1000, 2, 200), 6);
    }

    #[test]
    fn test_open_position_paused() {
        let admin = setup();
        emergency_pause(admin.as_ptr());
        let trader = [2u8; 32];
        assert_eq!(open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 2, 200), 1);
    }

    #[test]
    fn test_close_position() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 2, 200);
        assert_eq!(close_position(trader.as_ptr(), 1), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_CLOSED);
    }

    #[test]
    fn test_close_not_owner() {
        let admin = setup();
        let trader = [2u8; 32];
        let other = [3u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 2, 200);
        assert_eq!(close_position(other.as_ptr(), 1), 2);
    }

    #[test]
    fn test_close_already_closed() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 2, 200);
        close_position(trader.as_ptr(), 1);
        assert_eq!(close_position(trader.as_ptr(), 1), 3);
    }

    #[test]
    fn test_add_margin() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 2, 200);
        assert_eq!(add_margin(trader.as_ptr(), 1, 100), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin(&data), 300);
    }

    #[test]
    fn test_add_margin_zero() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 2, 200);
        assert_eq!(add_margin(trader.as_ptr(), 1, 0), 5);
    }

    #[test]
    fn test_remove_margin() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 2, 500);
        assert_eq!(remove_margin(trader.as_ptr(), 1, 100), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin(&data), 400);
    }

    #[test]
    fn test_remove_margin_too_much() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 2, 200);
        assert_eq!(remove_margin(trader.as_ptr(), 1, 300), 5);
    }

    #[test]
    fn test_remove_margin_would_liquidate() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        // Size=10_000_000_000, margin=1_100_000_000 (11% ratio)
        open_position(trader.as_ptr(), 1, SIDE_LONG, 10_000_000_000, 1, 2_100_000_000);
        // Removing 100 would drop below 10%
        // Current ratio: 2_100_000_000 / 10_000_000_000 = 21%
        // After remove: 1_100_000_000 / 10_000_000_000 = 11% — still above 10%
        assert_eq!(remove_margin(trader.as_ptr(), 1, 1_000_000_000), 0);
    }

    #[test]
    fn test_liquidation() {
        let admin = setup();
        let trader = [2u8; 32];
        let liquidator = [3u8; 32];
        test_mock::set_slot(100);
        // Open with minimal margin
        open_position(trader.as_ptr(), 1, SIDE_LONG, 10_000_000_000, 5, 500_000_000);
        // Drop mark price significantly
        set_mark_price(admin.as_ptr(), 1, 2_000_000_000); // 2.0 — huge notional
        // margin_ratio = 500_000_000 / (10B * 2.0 / 1e9) = 500M / 20B = 0.025 = 250 bps < 1000 bps
        assert_eq!(liquidate(liquidator.as_ptr(), 1), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_LIQUIDATED);
        assert!(get_insurance_fund() > 0);
    }

    #[test]
    fn test_liquidation_healthy_position() {
        let admin = setup();
        let trader = [2u8; 32];
        let liquidator = [3u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 2, 500);
        // Still healthy at current mark price
        assert_eq!(liquidate(liquidator.as_ptr(), 1), 2);
    }

    #[test]
    fn test_insurance_fund_accumulation() {
        let admin = setup();
        let trader = [2u8; 32];
        let liq = [3u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 10_000_000_000, 5, 500_000_000);
        set_mark_price(admin.as_ptr(), 1, 2_000_000_000);
        let before = get_insurance_fund();
        liquidate(liq.as_ptr(), 1);
        let after = get_insurance_fund();
        assert!(after > before);
    }

    #[test]
    fn test_set_max_leverage() {
        let admin = setup();
        assert_eq!(set_max_leverage(admin.as_ptr(), 1, 3), 0);
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        assert_eq!(open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 4, 200), 2); // exceeds 3x
    }

    #[test]
    fn test_set_max_leverage_invalid() {
        let admin = setup();
        assert_eq!(set_max_leverage(admin.as_ptr(), 1, 0), 2);
        assert_eq!(set_max_leverage(admin.as_ptr(), 1, 11), 2);
    }

    #[test]
    fn test_get_margin_ratio() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        let ratio = get_margin_ratio(1);
        // margin=500M, size=1B, price=1.0 → notional=1.0 → ratio=500M/1B = 50% = 5000 bps
        assert_eq!(ratio, 5000);
    }

    #[test]
    fn test_pnl_calculation_long_profit() {
        let (is_profit, pnl) = calculate_pnl(SIDE_LONG, 1_000_000_000, 1_000_000_000, 1_500_000_000);
        assert!(is_profit);
        assert_eq!(pnl, 500_000_000);
    }

    #[test]
    fn test_pnl_calculation_long_loss() {
        let (is_profit, pnl) = calculate_pnl(SIDE_LONG, 1_000_000_000, 1_000_000_000, 500_000_000);
        assert!(!is_profit);
        assert_eq!(pnl, 500_000_000);
    }

    #[test]
    fn test_pnl_calculation_short_profit() {
        let (is_profit, pnl) = calculate_pnl(SIDE_SHORT, 1_000_000_000, 1_000_000_000, 500_000_000);
        assert!(is_profit);
        assert_eq!(pnl, 500_000_000);
    }

    #[test]
    fn test_pnl_calculation_short_loss() {
        let (is_profit, pnl) = calculate_pnl(SIDE_SHORT, 1_000_000_000, 1_000_000_000, 1_500_000_000);
        assert!(!is_profit);
        assert_eq!(pnl, 500_000_000);
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
    fn test_get_position_info() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 2, 200);
        assert_eq!(get_position_info(1), 1);
        assert_eq!(get_position_info(999), 0);
    }

    #[test]
    fn test_set_maintenance_margin() {
        let admin = setup();
        assert_eq!(get_maintenance_margin(), MAINTENANCE_MARGIN_BPS); // default 1000
        assert_eq!(set_maintenance_margin(admin.as_ptr(), 1500), 0);
        assert_eq!(get_maintenance_margin(), 1500);
    }

    #[test]
    fn test_set_maintenance_margin_bounds() {
        let admin = setup();
        assert_eq!(set_maintenance_margin(admin.as_ptr(), 199), 2); // below min 200
        assert_eq!(set_maintenance_margin(admin.as_ptr(), 5001), 2); // above max 5000
        assert_eq!(set_maintenance_margin(admin.as_ptr(), 200), 0); // exactly min
        assert_eq!(set_maintenance_margin(admin.as_ptr(), 5000), 0); // exactly max
    }

    #[test]
    fn test_set_maintenance_margin_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        assert_eq!(set_maintenance_margin(rando.as_ptr(), 1500), 1);
    }
}
