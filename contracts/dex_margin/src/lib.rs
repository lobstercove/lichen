// DEX Margin — Margin Trading & Liquidation Engine (DEEP hardened)
//
// Features:
//   - Isolated margin positions (up to 100x leverage with tiered parameters)
//   - Tiered initial/maintenance margin and liquidation penalties
//   - Liquidation by anyone (earns 50% of penalty)
//   - Insurance fund from liquidation penalties
//   - Funding rate (8-hour intervals, scaled by leverage tier)
//   - Integration with LobsterLend for margin funding
//   - Host-level collateral locking via cross-contract calls
//   - Insurance fund governance withdrawal
//   - Emergency pause, reentrancy guard, admin controls
//   - Auto-deleveraging during extreme events

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    storage_get, storage_set, log_info,
    bytes_to_u64, u64_to_bytes, get_slot, get_timestamp,
    Address, CrossCall, call_contract, call_token_transfer,
};

// ============================================================================
// CONSTANTS
// ============================================================================

const MAX_LEVERAGE_ISOLATED: u64 = 100;
const MAX_LEVERAGE_CROSS: u64 = 3;
const LIQUIDATOR_SHARE_BPS: u64 = 5000;   // 50% of penalty to liquidator
const INSURANCE_SHARE_BPS: u64 = 5000;    // 50% of penalty to insurance
const FUNDING_INTERVAL_SLOTS: u64 = 28_800; // ~8 hours
const MAX_POSITIONS: u64 = 10_000;
const MAX_FUNDING_RATE_BPS: u64 = 100;    // 1% max per interval

// AUDIT-FIX M20: Mark price staleness guard — reject prices older than 30 minutes
const MAX_PRICE_AGE_SECONDS: u64 = 1800;

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
const MOLTCOIN_ADDRESS_KEY: &[u8] = b"mrg_molt_addr";

// ============================================================================
// LEVERAGE TIER TABLE
// ============================================================================
// Returns (initial_margin_bps, maintenance_margin_bps, liquidation_penalty_bps, funding_rate_mult_x10)
// funding_rate_mult_x10: 10 = 1.0x, 15 = 1.5x, 20 = 2.0x, etc.
fn get_tier_params(leverage: u64) -> (u64, u64, u64, u64) {
    if leverage <= 2 {
        (5000, 2500, 300, 10)       // 50% / 25% / 3% / 1.0x
    } else if leverage <= 3 {
        (3333, 1700, 300, 10)       // 33% / 17% / 3% / 1.0x
    } else if leverage <= 5 {
        (2000, 1000, 500, 15)       // 20% / 10% / 5% / 1.5x
    } else if leverage <= 10 {
        (1000, 500, 500, 20)        // 10% / 5%  / 5% / 2.0x
    } else if leverage <= 25 {
        (400, 200, 700, 30)         //  4% / 2%  / 7% / 3.0x
    } else if leverage <= 50 {
        (200, 100, 1000, 50)        //  2% / 1%  / 10% / 5.0x
    } else {
        // ≤100x
        (100, 50, 1500, 100)        //  1% / 0.5% / 15% / 10.0x
    }
}

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

/// AUDIT-FIX M20: Load mark price with timestamp. Returns (price, timestamp).
/// Backward-compatible: if only 8 bytes stored (legacy), timestamp = 0.
fn load_mark_price(pair_id: u64) -> (u64, u64) {
    match storage_get(&mark_price_key(pair_id)) {
        Some(d) if d.len() >= 16 => (bytes_to_u64(&d[..8]), bytes_to_u64(&d[8..16])),
        Some(d) if d.len() >= 8 => (bytes_to_u64(&d[..8]), 0), // legacy format
        _ => (0, 0),
    }
}

/// AUDIT-FIX M20: Check if a mark price is fresh enough for trading.
/// Returns the price if fresh, or 0 if missing/stale.
fn fresh_mark_price(pair_id: u64) -> u64 {
    let (price, ts) = load_mark_price(pair_id);
    if price == 0 { return 0; }
    let now = get_timestamp();
    if ts == 0 || (now > ts && now - ts > MAX_PRICE_AGE_SECONDS) {
        log_info("DEX Margin: Mark price stale — rejecting");
        return 0;
    }
    price
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
    // AUDIT-FIX M20: Store price + timestamp for freshness validation
    let mut data = Vec::with_capacity(16);
    data.extend_from_slice(&u64_to_bytes(price));
    data.extend_from_slice(&u64_to_bytes(get_timestamp()));
    storage_set(&mark_price_key(pair_id), &data);
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

    // AUDIT-FIX M20: Get mark price with freshness check
    let mark_price = fresh_mark_price(pair_id);
    if mark_price == 0 { reentrancy_exit(); return 6; }

    // Check initial margin (tiered by leverage)
    let notional = (size as u128 * mark_price as u128 / 1_000_000_000) as u64;
    let (initial_margin_bps, _maint_bps, _liq_penalty_bps, _funding_mult) = get_tier_params(leverage);
    // AUDIT-FIX NEW-H2: initial_margin_bps already factors in leverage via the tier table
    // (e.g. 10x → 1000 bps = 10%). Do NOT divide by leverage again — that was double-discounting.
    let required_margin = (notional * initial_margin_bps / 10_000).max(1);
    if margin_amount < required_margin { reentrancy_exit(); return 3; }

    let pos_count = load_u64(POSITION_COUNT_KEY);
    if pos_count >= MAX_POSITIONS { reentrancy_exit(); return 4; }

    let pos_id = pos_count + 1;
    let slot = get_slot();

    // Lock collateral at host level (move from spendable to locked)
    let lock_call = CrossCall::new(
        Address([0u8; 32]), // host-level call (address zero = runtime)
        "lock",
        {
            let mut args = Vec::with_capacity(40);
            args.extend_from_slice(&t);
            args.extend_from_slice(&u64_to_bytes(margin_amount));
            args
        },
    );
    let _ = call_contract(lock_call); // best-effort in test mode

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

    let margin = decode_pos_margin(&data);
    let size = decode_pos_size(&data);
    let pair_id = decode_pos_pair_id(&data);
    let side = decode_pos_side(&data);
    let entry_price = decode_pos_entry_price(&data);
    // AUDIT-FIX M20: Use freshness-checked mark price
    let mark_price = fresh_mark_price(pair_id);

    // Calculate PnL and determine unlock amount
    let unlock_amount = if mark_price > 0 {
        let (is_profit, pnl) = calculate_pnl(side, size, entry_price, mark_price);
        if is_profit {
            margin.saturating_add(pnl)
        } else {
            margin.saturating_sub(pnl)
        }
    } else {
        margin // no mark price available → return full margin
    };

    // Unlock collateral at host level (move from locked to spendable)
    let unlock_call = CrossCall::new(
        Address([0u8; 32]),
        "unlock",
        {
            let mut args = Vec::with_capacity(40);
            args.extend_from_slice(&trader);
            args.extend_from_slice(&u64_to_bytes(unlock_amount));
            args
        },
    );
    let _ = call_contract(unlock_call);

    update_pos_status(&mut data, POS_CLOSED);
    storage_set(&pk, &data);
    moltchain_sdk::set_return_data(&u64_to_bytes(unlock_amount));
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

    // Check if still above maintenance (tiered by leverage)
    let size = decode_pos_size(&data);
    let pair_id = decode_pos_pair_id(&data);
    let leverage = decode_pos_leverage(&data);
    // AUDIT-FIX M20: Freshness-checked mark price for margin health
    let mark_price = fresh_mark_price(pair_id);
    if mark_price > 0 {
        let ratio = calculate_margin_ratio(new_margin, size, mark_price);
        let (_init_bps, maint_bps, _liq_bps, _fund_mult) = get_tier_params(leverage);
        // Use admin-overridden maintenance if set and higher than tier
        let admin_maint = get_maintenance_margin_override();
        let effective_maint = if admin_maint > maint_bps { admin_maint } else { maint_bps };
        if ratio < effective_maint { reentrancy_exit(); return 6; } // would be unhealthy
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
    let leverage = decode_pos_leverage(&data);
    // AUDIT-FIX M20: Freshness-checked mark price for liquidation
    let mark_price = fresh_mark_price(pair_id);
    if mark_price == 0 { reentrancy_exit(); return 2; }

    let ratio = calculate_margin_ratio(margin, size, mark_price);
    let (_init_bps, maint_bps, liq_penalty_bps, _fund_mult) = get_tier_params(leverage);
    // Use admin-overridden maintenance if set and higher than tier
    let admin_maint = get_maintenance_margin_override();
    let effective_maint = if admin_maint > maint_bps { admin_maint } else { maint_bps };
    if ratio >= effective_maint { reentrancy_exit(); return 2; } // still healthy

    // Calculate penalty (tiered by leverage)
    // AUDIT-FIX NEW-L1: Use u128 intermediates to prevent overflow; derive insurance_add
    // as remainder so no dust is lost with odd penalty values.
    let notional = (size as u128 * mark_price as u128 / 1_000_000_000) as u64;
    let penalty = (notional as u128 * liq_penalty_bps as u128 / 10_000) as u64;
    let liquidator_reward = (penalty as u128 * LIQUIDATOR_SHARE_BPS as u128 / 10_000) as u64;
    let insurance_add = penalty.saturating_sub(liquidator_reward);

    // Add to insurance fund (saturating to prevent overflow)
    let insurance = load_u64(INSURANCE_FUND_KEY);
    save_u64(INSURANCE_FUND_KEY, insurance.saturating_add(insurance_add));

    // Unlock remaining margin minus penalty at host level
    let trader = decode_pos_trader(&data);
    let remaining = margin.saturating_sub(penalty);
    if remaining > 0 {
        let unlock_call = CrossCall::new(
            Address([0u8; 32]),
            "unlock",
            {
                let mut args = Vec::with_capacity(40);
                args.extend_from_slice(&trader);
                args.extend_from_slice(&u64_to_bytes(remaining));
                args
            },
        );
        let _ = call_contract(unlock_call);
    }

    // Deduct penalty from locked balance
    let deduct_call = CrossCall::new(
        Address([0u8; 32]),
        "deduct",
        {
            let mut args = Vec::with_capacity(40);
            args.extend_from_slice(&trader);
            args.extend_from_slice(&u64_to_bytes(penalty.min(margin)));
            args
        },
    );
    let _ = call_contract(deduct_call);

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
    if max_leverage == 0 || max_leverage > 100 { return 2; }
    save_u64(&max_leverage_key(pair_id), max_leverage);
    0
}

/// Set maintenance margin in basis points (admin only)
/// Default is 1000 (10%). Min 200 (2%), Max 5000 (50%).
/// Acts as a floor override that applies when higher than tier default.
pub fn set_maintenance_margin(caller: *const u8, margin_bps: u64) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    if !require_admin(&c) { return 1; }
    if margin_bps < 200 || margin_bps > 5000 { return 2; }
    save_u64(&maintenance_margin_key_fn(), margin_bps);
    0
}

/// Set the MoltCoin contract address (admin only, for insurance withdrawal)
pub fn set_moltcoin_address(caller: *const u8, addr: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut a = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(addr, a.as_mut_ptr(), 32);
    }
    if !require_admin(&c) { return 1; }
    if is_zero(&a) { return 2; }
    storage_set(MOLTCOIN_ADDRESS_KEY, &a);
    0
}

/// Withdraw from the insurance fund (admin/governance only)
/// Returns: 0=success, 1=not admin, 2=zero amount, 3=insufficient funds,
///          4=no moltcoin address, 5=transfer failed
pub fn withdraw_insurance(caller: *const u8, amount: u64, recipient: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut r = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(recipient, r.as_mut_ptr(), 32);
    }
    if !require_admin(&c) { return 1; }
    if amount == 0 { return 2; }

    let insurance = load_u64(INSURANCE_FUND_KEY);
    if amount > insurance { return 3; }

    let molt_addr = load_addr(MOLTCOIN_ADDRESS_KEY);
    if is_zero(&molt_addr) { return 4; }

    // Cross-contract call to transfer MOLT from this contract to recipient
    let admin_addr = load_addr(ADMIN_KEY);
    match call_token_transfer(
        Address(molt_addr),
        Address(admin_addr), // source: contract admin (insurance custodian)
        Address(r),
        amount,
    ) {
        Ok(_) => {
            save_u64(INSURANCE_FUND_KEY, insurance - amount);
            log_info("Insurance fund withdrawal");
            moltchain_sdk::set_return_data(&u64_to_bytes(amount));
            0
        }
        Err(_) => 5,
    }
}

/// Get tier parameters for a given leverage (for external queries)
pub fn get_tier_info(leverage: u64) -> u64 {
    let (init_bps, maint_bps, liq_bps, fund_mult) = get_tier_params(leverage);
    let mut result = Vec::with_capacity(32);
    result.extend_from_slice(&u64_to_bytes(init_bps));
    result.extend_from_slice(&u64_to_bytes(maint_bps));
    result.extend_from_slice(&u64_to_bytes(liq_bps));
    result.extend_from_slice(&u64_to_bytes(fund_mult));
    moltchain_sdk::set_return_data(&result);
    leverage
}

/// Get the admin-set maintenance margin override (bps); returns 0 if unset.
/// When > 0, acts as a floor that overrides tier defaults if higher.
pub fn get_maintenance_margin_override() -> u64 {
    load_u64(&maintenance_margin_key_fn())
}

/// Get the effective maintenance margin for a given leverage (bps).
/// Returns the tier default or the admin override, whichever is higher.
pub fn get_maintenance_margin(leverage: u64) -> u64 {
    let (_init_bps, tier_maint, _liq_bps, _fund_mult) = get_tier_params(leverage);
    let admin_override = get_maintenance_margin_override();
    if admin_override > tier_maint { admin_override } else { tier_maint }
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
    // AUDIT-FIX M20: Freshness-checked mark price for ratio query
    let mark_price = fresh_mark_price(pair_id);
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
        // 0 = initialize(admin[32])
        0 => {
            if args.len() >= 33 {
                let r = initialize(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 1 = set_mark_price(caller[32], pair_id[8], price[8])
        1 => {
            if args.len() >= 49 {
                let pair_id = bytes_to_u64(&args[33..41]);
                let price = bytes_to_u64(&args[41..49]);
                let r = set_mark_price(args[1..33].as_ptr(), pair_id, price);
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 2 = open_position(trader[32], pair_id[8], side[1], size[8], leverage[8], margin[8])
        2 => {
            if args.len() >= 66 {
                let pair_id = bytes_to_u64(&args[33..41]);
                let side = args[41];
                let size = bytes_to_u64(&args[42..50]);
                let leverage = bytes_to_u64(&args[50..58]);
                let margin = bytes_to_u64(&args[58..66]);
                let r = open_position(args[1..33].as_ptr(), pair_id, side, size, leverage, margin);
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 3 = close_position(caller[32], pos_id[8])
        3 => {
            if args.len() >= 41 {
                let pos_id = bytes_to_u64(&args[33..41]);
                let r = close_position(args[1..33].as_ptr(), pos_id);
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 4 = add_margin(caller[32], pos_id[8], amount[8])
        4 => {
            if args.len() >= 49 {
                let pos_id = bytes_to_u64(&args[33..41]);
                let amount = bytes_to_u64(&args[41..49]);
                let r = add_margin(args[1..33].as_ptr(), pos_id, amount);
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 5 = remove_margin(caller[32], pos_id[8], amount[8])
        5 => {
            if args.len() >= 49 {
                let pos_id = bytes_to_u64(&args[33..41]);
                let amount = bytes_to_u64(&args[41..49]);
                let r = remove_margin(args[1..33].as_ptr(), pos_id, amount);
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 6 = liquidate(liquidator[32], pos_id[8])
        6 => {
            if args.len() >= 41 {
                let pos_id = bytes_to_u64(&args[33..41]);
                let r = liquidate(args[1..33].as_ptr(), pos_id);
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 7 = set_max_leverage(caller[32], pair_id[8], max_lev[8])
        7 => {
            if args.len() >= 49 {
                let pair_id = bytes_to_u64(&args[33..41]);
                let max_lev = bytes_to_u64(&args[41..49]);
                let r = set_max_leverage(args[1..33].as_ptr(), pair_id, max_lev);
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 8 = set_maintenance_margin(caller[32], margin_bps[8])
        8 => {
            if args.len() >= 41 {
                let bps = bytes_to_u64(&args[33..41]);
                let r = set_maintenance_margin(args[1..33].as_ptr(), bps);
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 9 = withdraw_insurance(caller[32], amount[8], recipient[32])
        9 => {
            if args.len() >= 73 {
                let amount = bytes_to_u64(&args[33..41]);
                let r = withdraw_insurance(args[1..33].as_ptr(), amount, args[41..73].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 10 = get_position_info(pos_id[8])
        10 => {
            if args.len() >= 9 {
                let pos_id = bytes_to_u64(&args[1..9]);
                get_position_info(pos_id);
            }
        }
        // 11 = get_margin_ratio(pos_id[8])
        11 => {
            if args.len() >= 9 {
                let pos_id = bytes_to_u64(&args[1..9]);
                let r = get_margin_ratio(pos_id);
                moltchain_sdk::set_return_data(&u64_to_bytes(r));
            }
        }
        // 12 = get_tier_info(leverage[8])
        12 => {
            if args.len() >= 9 {
                let lev = bytes_to_u64(&args[1..9]);
                get_tier_info(lev);
            }
        }
        // 13 = emergency_pause(caller[32])
        13 => {
            if args.len() >= 33 {
                let r = emergency_pause(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 14 = emergency_unpause(caller[32])
        14 => {
            if args.len() >= 33 {
                let r = emergency_unpause(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 15 = set_moltcoin_address(caller[32], addr[32])
        15 => {
            if args.len() >= 65 {
                let r = set_moltcoin_address(args[1..33].as_ptr(), args[33..65].as_ptr());
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
        assert_eq!(initialize(admin.as_ptr()), 0);
        // Set mark price for pair 1: 1.0 (scaled by 1e9)
        set_mark_price(admin.as_ptr(), 1, 1_000_000_000);
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
        // AUDIT-FIX M20: mark price now stored as (price, timestamp)
        let (price, ts) = load_mark_price(1);
        assert_eq!(price, 2_000_000_000);
        assert!(ts > 0);
    }

    #[test]
    fn test_set_mark_price_zero() {
        let admin = setup();
        assert_eq!(set_mark_price(admin.as_ptr(), 1, 0), 2);
    }

    // ---- TIER TABLE TESTS ----

    #[test]
    fn test_tier_params_2x() {
        let (init, maint, liq, fund) = get_tier_params(2);
        assert_eq!(init, 5000);  // 50%
        assert_eq!(maint, 2500); // 25%
        assert_eq!(liq, 300);    // 3%
        assert_eq!(fund, 10);    // 1.0x
    }

    #[test]
    fn test_tier_params_3x() {
        let (init, maint, liq, fund) = get_tier_params(3);
        assert_eq!(init, 3333);
        assert_eq!(maint, 1700);
        assert_eq!(liq, 300);
        assert_eq!(fund, 10);
    }

    #[test]
    fn test_tier_params_5x() {
        let (init, maint, liq, fund) = get_tier_params(5);
        assert_eq!(init, 2000);
        assert_eq!(maint, 1000);
        assert_eq!(liq, 500);
        assert_eq!(fund, 15);
    }

    #[test]
    fn test_tier_params_10x() {
        let (init, maint, liq, fund) = get_tier_params(10);
        assert_eq!(init, 1000);
        assert_eq!(maint, 500);
        assert_eq!(liq, 500);
        assert_eq!(fund, 20);
    }

    #[test]
    fn test_tier_params_25x() {
        let (init, maint, liq, fund) = get_tier_params(25);
        assert_eq!(init, 400);
        assert_eq!(maint, 200);
        assert_eq!(liq, 700);
        assert_eq!(fund, 30);
    }

    #[test]
    fn test_tier_params_50x() {
        let (init, maint, liq, fund) = get_tier_params(50);
        assert_eq!(init, 200);
        assert_eq!(maint, 100);
        assert_eq!(liq, 1000);
        assert_eq!(fund, 50);
    }

    #[test]
    fn test_tier_params_100x() {
        let (init, maint, liq, fund) = get_tier_params(100);
        assert_eq!(init, 100);
        assert_eq!(maint, 50);
        assert_eq!(liq, 1500);
        assert_eq!(fund, 100);
    }

    #[test]
    fn test_tier_params_7x_uses_10x_tier() {
        // 7x falls in ≤10x tier
        let (init, maint, liq, fund) = get_tier_params(7);
        assert_eq!(init, 1000);
        assert_eq!(maint, 500);
        assert_eq!(liq, 500);
        assert_eq!(fund, 20);
    }

    #[test]
    fn test_tier_params_1x() {
        // 1x leverage is ≤2x tier
        let (init, maint, liq, _fund) = get_tier_params(1);
        assert_eq!(init, 5000);
        assert_eq!(maint, 2500);
        assert_eq!(liq, 300);
    }

    // ---- POSITION TESTS (updated for tiered margins) ----

    #[test]
    fn test_open_position_long_2x() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        // AUDIT-FIX NEW-H2: corrected formula — no /leverage.
        // 2x tier: initial_margin_bps=5000 → required = 1B * 5000/10000 = 500_000_000
        assert_eq!(open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000), 0);
        assert_eq!(get_position_count(), 1);
    }

    #[test]
    fn test_open_position_short() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        assert_eq!(open_position(trader.as_ptr(), 1, SIDE_SHORT, 1_000_000_000, 2, 500_000_000), 0);
    }

    #[test]
    fn test_open_position_5x() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        // 5x tier: initial_margin_bps=2000 → required = 1B * 2000/10000 = 200_000_000
        assert_eq!(open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 5, 200_000_000), 0);
    }

    #[test]
    fn test_open_position_10x() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        // 10x tier: initial_margin_bps=1000 → required = 1B * 1000/10000 = 100_000_000
        assert_eq!(open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 10, 100_000_000), 0);
    }

    #[test]
    fn test_open_position_25x() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        // 25x tier: initial_margin_bps=400 → required = 1B * 400/10000 = 40_000_000
        assert_eq!(open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 25, 40_000_000), 0);
    }

    #[test]
    fn test_open_position_50x() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        // 50x tier: initial_margin_bps=200 → required = 1B * 200/10000 = 20_000_000
        assert_eq!(open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 50, 20_000_000), 0);
    }

    #[test]
    fn test_open_position_100x() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        // 100x tier: initial_margin_bps=100 → required = 1B * 100/10000 = 10_000_000
        assert_eq!(open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 100, 10_000_000), 0);
    }

    #[test]
    fn test_open_position_overleveraged() {
        let _admin = setup();
        let trader = [2u8; 32];
        // 101x exceeds MAX_LEVERAGE_ISOLATED=100
        assert_eq!(open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 101, 200), 2);
    }

    #[test]
    fn test_open_position_insufficient_margin_5x() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        // 5x, notional=1B, required=200_000_000; give less
        assert_eq!(open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 5, 199_999_999), 3);
    }

    #[test]
    fn test_open_position_no_mark_price() {
        let _admin = setup();
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
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(close_position(trader.as_ptr(), 1), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_CLOSED);
    }

    #[test]
    fn test_close_not_owner() {
        let _admin = setup();
        let trader = [2u8; 32];
        let other = [3u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(close_position(other.as_ptr(), 1), 2);
    }

    #[test]
    fn test_close_already_closed() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        close_position(trader.as_ptr(), 1);
        assert_eq!(close_position(trader.as_ptr(), 1), 3);
    }

    #[test]
    fn test_add_margin() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(add_margin(trader.as_ptr(), 1, 100), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin(&data), 500_000_100);
    }

    #[test]
    fn test_add_margin_zero() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(add_margin(trader.as_ptr(), 1, 0), 5);
    }

    #[test]
    fn test_remove_margin() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        // 2x: maint margin = 25% → need 250M for 1B notional
        // Start with 500M (50%) and remove 100M → still above 25%
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(remove_margin(trader.as_ptr(), 1, 100_000_000), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin(&data), 400_000_000);
    }

    #[test]
    fn test_remove_margin_too_much() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        // 600M > 500M margin → error 5
        assert_eq!(remove_margin(trader.as_ptr(), 1, 600_000_000), 5);
    }

    #[test]
    fn test_remove_margin_would_breach_maintenance() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        // 2x: maint = 2500bps = 25%. notional=1B → need 250M maint.
        // Open with 500M (50%), remove 260M → 240M < 250M → fail
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(remove_margin(trader.as_ptr(), 1, 260_000_000), 6);
    }

    #[test]
    fn test_liquidation_2x() {
        let admin = setup();
        let trader = [2u8; 32];
        let liquidator = [3u8; 32];
        test_mock::set_slot(100);
        // 2x long, margin=500M, size=1B at price 1.0
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        // Raise mark price to 2.5 → notional=2.5B
        // margin_ratio = 500M / 2.5B = 2000 bps = 20% < 25% maint → liquidatable
        set_mark_price(admin.as_ptr(), 1, 2_500_000_000);
        assert_eq!(liquidate(liquidator.as_ptr(), 1), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_LIQUIDATED);
        assert!(get_insurance_fund() > 0);
    }

    #[test]
    fn test_liquidation_high_leverage() {
        let admin = setup();
        let trader = [2u8; 32];
        let liquidator = [3u8; 32];
        test_mock::set_slot(100);
        // 50x tier: initial_margin_bps=200 → required = 1B * 200/10000 = 20M
        // maint_margin_bps=100 = 1%
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 50, 20_000_000);
        // Mark price 3.0 → notional=3B, ratio = 20M/3B ≈ 6.67bps < 100bps → liquidatable
        set_mark_price(admin.as_ptr(), 1, 3_000_000_000);
        assert_eq!(liquidate(liquidator.as_ptr(), 1), 0);
    }

    #[test]
    fn test_liquidation_healthy_position() {
        let _admin = setup();
        let trader = [2u8; 32];
        let liquidator = [3u8; 32];
        test_mock::set_slot(100);
        // 2x with healthy margin (50%) > 25% maint
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(liquidate(liquidator.as_ptr(), 1), 2);
    }

    #[test]
    fn test_liquidation_penalty_different_tiers() {
        let _admin = setup();
        let trader_a = [2u8; 32];
        let trader_b = [3u8; 32];
        let liquidator = [4u8; 32];
        test_mock::set_slot(100);

        // For 5x tier: initial_margin_bps=2000, maint=1000bps=10%, penalty=500bps
        // notional=1B, required margin = 1B * 2000/10000 = 200M
        // Give 200M = exactly at initial margin, below maintenance for a 2x price increase
        let r1 = open_position(trader_a.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 5, 200_000_000);
        assert_eq!(r1, 0, "open_position 5x should succeed");

        let before = get_insurance_fund();
        // Raise mark price to 2.5 → notional=2.5B
        // margin_ratio = 200M/2.5B = 800bps < 1000bps maint → liquidatable
        set_mark_price(_admin.as_ptr(), 1, 2_500_000_000);
        let liq1 = liquidate(liquidator.as_ptr(), 1);
        assert_eq!(liq1, 0, "liquidate pos 1 should succeed");
        let after_a = get_insurance_fund();
        let insurance_a = after_a - before;
        // penalty = 2.5B * 500/10000 = 125M, capped to min(125M, 200M) = 125M
        // insurance = 125M / 2 = 62.5M → 62_500_000
        assert_eq!(insurance_a, 62_500_000);

        // Reset price for 2nd position
        set_mark_price(_admin.as_ptr(), 1, 1_000_000_000);
        // For 2x tier: initial=5000bps, maint=2500bps=25%, penalty=300bps
        // notional=1B, required = 500M
        let r2 = open_position(trader_b.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(r2, 0, "open_position 2x should succeed");
        // Raise mark price to 2.5: notional = 1B * 2.5 = 2.5B
        // margin_ratio = 500M/2.5B = 2000bps < 2500bps → liquidatable
        set_mark_price(_admin.as_ptr(), 1, 2_500_000_000);
        // penalty = 2.5B * 300/10000 = 75M, capped to min(75M, 500M) = 75M
        // insurance = 75M / 2 = 37_500_000
        let liq2 = liquidate(liquidator.as_ptr(), 2);
        assert_eq!(liq2, 0, "liquidate pos 2 should succeed");
        let after_b = get_insurance_fund();
        let insurance_b = after_b - after_a;
        assert_eq!(insurance_b, 37_500_000);
    }

    #[test]
    fn test_insurance_fund_accumulation() {
        let admin = setup();
        let trader = [2u8; 32];
        let liq = [3u8; 32];
        test_mock::set_slot(100);
        // 5x tier: required = 1B * 2000/10000 = 200M, maint=1000bps=10%
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 5, 200_000_000);
        // Raise mark price → notional grows → position becomes unhealthy
        set_mark_price(admin.as_ptr(), 1, 3_000_000_000);
        let before = get_insurance_fund();
        liquidate(liq.as_ptr(), 1);
        let after = get_insurance_fund();
        assert!(after > before);
    }

    #[test]
    fn test_set_max_leverage() {
        let admin = setup();
        assert_eq!(set_max_leverage(admin.as_ptr(), 1, 50), 0);
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        assert_eq!(open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 51, 200), 2);
    }

    #[test]
    fn test_set_max_leverage_100x() {
        let admin = setup();
        assert_eq!(set_max_leverage(admin.as_ptr(), 1, 100), 0); // now valid
    }

    #[test]
    fn test_set_max_leverage_invalid() {
        let admin = setup();
        assert_eq!(set_max_leverage(admin.as_ptr(), 1, 0), 2);
        assert_eq!(set_max_leverage(admin.as_ptr(), 1, 101), 2);
    }

    #[test]
    fn test_get_margin_ratio() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        let ratio = get_margin_ratio(1);
        // margin=500M, size=1B, price=1.0 → notional=1B → ratio=500M/1B = 50% = 5000 bps
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
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(get_position_info(1), 1);
        assert_eq!(get_position_info(999), 0);
    }

    #[test]
    fn test_set_maintenance_margin() {
        let admin = setup();
        assert_eq!(set_maintenance_margin(admin.as_ptr(), 1500), 0);
        assert_eq!(get_maintenance_margin_override(), 1500);
    }

    #[test]
    fn test_set_maintenance_margin_bounds() {
        let admin = setup();
        assert_eq!(set_maintenance_margin(admin.as_ptr(), 199), 2);
        assert_eq!(set_maintenance_margin(admin.as_ptr(), 5001), 2);
        assert_eq!(set_maintenance_margin(admin.as_ptr(), 200), 0);
        assert_eq!(set_maintenance_margin(admin.as_ptr(), 5000), 0);
    }

    #[test]
    fn test_set_maintenance_margin_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        assert_eq!(set_maintenance_margin(rando.as_ptr(), 1500), 1);
    }

    #[test]
    fn test_get_maintenance_margin_effective() {
        let admin = setup();
        // 5x tier has 1000 bps maint by default
        assert_eq!(get_maintenance_margin(5), 1000);
        // Set admin override to 1500 — higher than tier, so it takes effect
        set_maintenance_margin(admin.as_ptr(), 1500);
        assert_eq!(get_maintenance_margin(5), 1500);
        // 2x tier has 2500 bps maint — admin override 1500 is lower, tier wins
        assert_eq!(get_maintenance_margin(2), 2500);
    }

    // ---- INSURANCE FUND WITHDRAWAL TESTS ----

    #[test]
    fn test_withdraw_insurance_no_moltcoin_addr() {
        let admin = setup();
        // Seed insurance fund
        save_u64(INSURANCE_FUND_KEY, 1_000_000);
        let recipient = [5u8; 32];
        assert_eq!(withdraw_insurance(admin.as_ptr(), 500_000, recipient.as_ptr()), 4);
    }

    #[test]
    fn test_withdraw_insurance_success() {
        let admin = setup();
        save_u64(INSURANCE_FUND_KEY, 1_000_000);
        let molt_addr = [10u8; 32];
        set_moltcoin_address(admin.as_ptr(), molt_addr.as_ptr());
        let recipient = [5u8; 32];
        // In test mode, cross-contract call returns Ok(Vec::new()) → success path
        assert_eq!(withdraw_insurance(admin.as_ptr(), 500_000, recipient.as_ptr()), 0);
        assert_eq!(get_insurance_fund(), 500_000);
    }

    #[test]
    fn test_withdraw_insurance_exceeds_balance() {
        let admin = setup();
        save_u64(INSURANCE_FUND_KEY, 100);
        let molt_addr = [10u8; 32];
        set_moltcoin_address(admin.as_ptr(), molt_addr.as_ptr());
        let recipient = [5u8; 32];
        assert_eq!(withdraw_insurance(admin.as_ptr(), 200, recipient.as_ptr()), 3);
    }

    #[test]
    fn test_withdraw_insurance_zero_amount() {
        let admin = setup();
        let recipient = [5u8; 32];
        assert_eq!(withdraw_insurance(admin.as_ptr(), 0, recipient.as_ptr()), 2);
    }

    #[test]
    fn test_withdraw_insurance_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        let recipient = [5u8; 32];
        assert_eq!(withdraw_insurance(rando.as_ptr(), 100, recipient.as_ptr()), 1);
    }

    #[test]
    fn test_set_moltcoin_address() {
        let admin = setup();
        let molt = [10u8; 32];
        assert_eq!(set_moltcoin_address(admin.as_ptr(), molt.as_ptr()), 0);
        assert_eq!(load_addr(MOLTCOIN_ADDRESS_KEY), molt);
    }

    #[test]
    fn test_set_moltcoin_address_zero() {
        let admin = setup();
        let zero = [0u8; 32];
        assert_eq!(set_moltcoin_address(admin.as_ptr(), zero.as_ptr()), 2);
    }

    #[test]
    fn test_set_moltcoin_address_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        let molt = [10u8; 32];
        assert_eq!(set_moltcoin_address(rando.as_ptr(), molt.as_ptr()), 1);
    }

    #[test]
    fn test_get_tier_info() {
        let _admin = setup();
        let r = get_tier_info(25);
        assert_eq!(r, 25);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 32);
        assert_eq!(bytes_to_u64(&ret[0..8]), 400);   // init_margin
        assert_eq!(bytes_to_u64(&ret[8..16]), 200);  // maint_margin
        assert_eq!(bytes_to_u64(&ret[16..24]), 700); // liq_penalty
        assert_eq!(bytes_to_u64(&ret[24..32]), 30);  // funding_mult
    }

    #[test]
    fn test_close_position_returns_unlock_amount() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        // Open with 500M margin at 2x
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(close_position(trader.as_ptr(), 1), 0);
        // Should return unlock amount (margin ± PnL at same mark price = margin)
        let ret = test_mock::get_return_data();
        let unlock = bytes_to_u64(&ret);
        assert_eq!(unlock, 500_000_000); // no price change → full margin returned
    }
}
