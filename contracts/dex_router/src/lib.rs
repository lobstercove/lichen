// DEX Router — Smart Order Routing Engine (DEEP hardened)
//
// Routes trades optimally across:
//   - CLOB order book (dex_core)
//   - AMM pools (dex_amm)
//   - Legacy MoltSwap pools
//   - Multi-hop paths (A→B→C)
//   - Split routes (partial CLOB + partial AMM)
//
// DEEP features:
//   - Emergency pause, reentrancy guard
//   - Deadline enforcement on all swaps
//   - Slippage protection
//   - Admin-configured route registry

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

const MAX_ROUTES: u64 = 200;
const MAX_HOPS: u64 = 4;
const MAX_SPLIT_LEGS: u64 = 3;
const SLIPPAGE_GUARD_BPS: u64 = 500; // 5% max auto-slippage

// Route types
const ROUTE_DIRECT_CLOB: u8 = 0;
const ROUTE_DIRECT_AMM: u8 = 1;
const ROUTE_SPLIT: u8 = 2;
const ROUTE_MULTI_HOP: u8 = 3;
const ROUTE_LEGACY_SWAP: u8 = 4;

// Storage keys
const ADMIN_KEY: &[u8] = b"rtr_admin";
const PAUSED_KEY: &[u8] = b"rtr_paused";
const REENTRANCY_KEY: &[u8] = b"rtr_reentrancy";
const ROUTE_COUNT_KEY: &[u8] = b"rtr_route_count";
const SWAP_COUNT_KEY: &[u8] = b"rtr_swap_count";
const CORE_ADDRESS_KEY: &[u8] = b"rtr_core_addr";
const AMM_ADDRESS_KEY: &[u8] = b"rtr_amm_addr";
const LEGACY_SWAP_KEY: &[u8] = b"rtr_legacy_addr";

// ============================================================================
// HELPERS
// ============================================================================

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key).map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 }).unwrap_or(0)
}

fn save_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
}

fn load_addr(key: &[u8]) -> [u8; 32] {
    storage_get(key).map(|d| {
        let mut a = [0u8; 32];
        if d.len() >= 32 { a.copy_from_slice(&d[..32]); }
        a
    }).unwrap_or([0u8; 32])
}

fn is_zero(addr: &[u8; 32]) -> bool { addr.iter().all(|&b| b == 0) }

fn u64_to_decimal(mut n: u64) -> Vec<u8> {
    if n == 0 { return alloc::vec![b'0']; }
    let mut buf = Vec::new();
    while n > 0 { buf.push(b'0' + (n % 10) as u8); n /= 10; }
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

fn route_key(route_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"rtr_route_"[..]);
    k.extend_from_slice(&u64_to_decimal(route_id));
    k
}

fn pair_route_key(token_in: &[u8; 32], token_out: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"rtr_pr_"[..]);
    k.extend_from_slice(&hex_encode(token_in));
    k.push(b'_');
    k.extend_from_slice(&hex_encode(token_out));
    k
}

fn swap_record_key(swap_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"rtr_swap_"[..]);
    k.extend_from_slice(&u64_to_decimal(swap_id));
    k
}

// ============================================================================
// DEEP SECURITY
// ============================================================================

fn reentrancy_enter() -> bool {
    if storage_get(REENTRANCY_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(false) {
        return false;
    }
    storage_set(REENTRANCY_KEY, &[1u8]);
    true
}

fn reentrancy_exit() { storage_set(REENTRANCY_KEY, &[0u8]); }

fn is_paused() -> bool {
    storage_get(PAUSED_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(false)
}

fn require_not_paused() -> bool { !is_paused() }

fn require_admin(caller: &[u8; 32]) -> bool {
    let admin = load_addr(ADMIN_KEY);
    !is_zero(&admin) && *caller == admin
}

// ============================================================================
// ROUTE LAYOUT (96 bytes)
// ============================================================================
// Bytes 0..32    : token_in address
// Bytes 32..64   : token_out address
// Bytes 64..72   : route_id (u64)
// Byte  72       : route_type (u8)
// Bytes 73..81   : pool_or_pair_id (u64) — CLOB pair or AMM pool
// Bytes 81..89   : secondary_id (u64) — for split routes: second leg
// Byte  89       : split_percent (u8) — % to first leg (0-100)
// Byte  90       : enabled (u8, 0=disabled, 1=enabled)
// Bytes 91..96   : padding

const ROUTE_SIZE: usize = 96;

fn encode_route(
    token_in: &[u8; 32], token_out: &[u8; 32], route_id: u64,
    route_type: u8, pool_or_pair_id: u64, secondary_id: u64,
    split_percent: u8, enabled: u8,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(ROUTE_SIZE);
    data.extend_from_slice(token_in);
    data.extend_from_slice(token_out);
    data.extend_from_slice(&u64_to_bytes(route_id));
    data.push(route_type);
    data.extend_from_slice(&u64_to_bytes(pool_or_pair_id));
    data.extend_from_slice(&u64_to_bytes(secondary_id));
    data.push(split_percent);
    data.push(enabled);
    while data.len() < ROUTE_SIZE { data.push(0); }
    data
}

fn decode_route_type(data: &[u8]) -> u8 {
    if data.len() > 72 { data[72] } else { 0 }
}
fn decode_route_pool_id(data: &[u8]) -> u64 {
    if data.len() >= 81 { bytes_to_u64(&data[73..81]) } else { 0 }
}
fn decode_route_secondary_id(data: &[u8]) -> u64 {
    if data.len() >= 89 { bytes_to_u64(&data[81..89]) } else { 0 }
}
fn decode_route_split_percent(data: &[u8]) -> u8 {
    if data.len() > 89 { data[89] } else { 50 }
}
fn decode_route_enabled(data: &[u8]) -> bool {
    if data.len() > 90 { data[90] == 1 } else { false }
}

// ============================================================================
// SWAP RECORD LAYOUT (72 bytes)
// ============================================================================
// Bytes 0..32    : trader address
// Bytes 32..40   : amount_in (u64)
// Bytes 40..48   : amount_out (u64)
// Byte  48       : route_type used
// Bytes 49..57   : slot (u64)
// Bytes 57..65   : route_id (u64)
// Bytes 65..72   : padding

const SWAP_RECORD_SIZE: usize = 72;

fn encode_swap_record(
    trader: &[u8; 32], amount_in: u64, amount_out: u64,
    route_type: u8, slot: u64, route_id: u64,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(SWAP_RECORD_SIZE);
    data.extend_from_slice(trader);
    data.extend_from_slice(&u64_to_bytes(amount_in));
    data.extend_from_slice(&u64_to_bytes(amount_out));
    data.push(route_type);
    data.extend_from_slice(&u64_to_bytes(slot));
    data.extend_from_slice(&u64_to_bytes(route_id));
    while data.len() < SWAP_RECORD_SIZE { data.push(0); }
    data
}

// ============================================================================
// PUBLIC FUNCTIONS
// ============================================================================

/// Initialize the router
pub fn initialize(admin: *const u8) -> u32 {
    let existing = load_addr(ADMIN_KEY);
    if !is_zero(&existing) { return 1; }
    let mut addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(admin, addr.as_mut_ptr(), 32); }
    storage_set(ADMIN_KEY, &addr);
    save_u64(ROUTE_COUNT_KEY, 0);
    save_u64(SWAP_COUNT_KEY, 0);
    storage_set(PAUSED_KEY, &[0u8]);
    log_info("DEX Router initialized");
    0
}

/// Configure contract addresses (admin only)
pub fn set_addresses(
    caller: *const u8, core_addr: *const u8, amm_addr: *const u8, legacy_addr: *const u8,
) -> u32 {
    let mut c = [0u8; 32];
    let mut ca = [0u8; 32];
    let mut aa = [0u8; 32];
    let mut la = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(core_addr, ca.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(amm_addr, aa.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(legacy_addr, la.as_mut_ptr(), 32);
    }
    if !require_admin(&c) { return 1; }
    storage_set(CORE_ADDRESS_KEY, &ca);
    storage_set(AMM_ADDRESS_KEY, &aa);
    storage_set(LEGACY_SWAP_KEY, &la);
    0
}

/// Register a route (admin only)
/// Returns: 0=success, 1=not admin, 2=max routes, 3=invalid type, 4=reentrancy
pub fn register_route(
    caller: *const u8, token_in: *const u8, token_out: *const u8,
    route_type: u8, pool_or_pair_id: u64, secondary_id: u64, split_percent: u8,
) -> u32 {
    if !reentrancy_enter() { return 4; }
    let mut c = [0u8; 32];
    let mut ti = [0u8; 32];
    let mut to = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(token_in, ti.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(token_out, to.as_mut_ptr(), 32);
    }
    if !require_admin(&c) { reentrancy_exit(); return 1; }

    let count = load_u64(ROUTE_COUNT_KEY);
    if count >= MAX_ROUTES { reentrancy_exit(); return 2; }
    if route_type > ROUTE_LEGACY_SWAP { reentrancy_exit(); return 3; }
    if route_type == ROUTE_SPLIT && (split_percent == 0 || split_percent >= 100) {
        reentrancy_exit(); return 3;
    }

    let route_id = count + 1;
    let data = encode_route(&ti, &to, route_id, route_type, pool_or_pair_id, secondary_id, split_percent, 1);
    storage_set(&route_key(route_id), &data);
    save_u64(ROUTE_COUNT_KEY, route_id);

    // Index by token pair for fast lookup
    save_u64(&pair_route_key(&ti, &to), route_id);

    log_info("Route registered");
    reentrancy_exit();
    0
}

/// Enable/disable a route
pub fn set_route_enabled(caller: *const u8, route_id: u64, enabled: bool) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    if !require_admin(&c) { return 1; }
    let rk = route_key(route_id);
    let mut data = match storage_get(&rk) {
        Some(d) if d.len() >= ROUTE_SIZE => d,
        _ => return 2,
    };
    data[90] = if enabled { 1 } else { 0 };
    storage_set(&rk, &data);
    0
}

/// Execute a smart-routed swap
/// Returns: 0=success, 1=paused, 2=no route, 3=deadline, 4=slippage, 5=reentrancy, 6=zero amount
pub fn swap(
    trader: *const u8, token_in: *const u8, token_out: *const u8,
    amount_in: u64, min_amount_out: u64, deadline: u64,
) -> u32 {
    if !reentrancy_enter() { return 5; }
    if !require_not_paused() { reentrancy_exit(); return 1; }
    if amount_in == 0 { reentrancy_exit(); return 6; }

    let current_slot = get_slot();
    if deadline != 0 && current_slot > deadline { reentrancy_exit(); return 3; }

    let mut t = [0u8; 32];
    let mut ti = [0u8; 32];
    let mut to_addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(trader, t.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(token_in, ti.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(token_out, to_addr.as_mut_ptr(), 32);
    }

    // Find best route
    let route_id = load_u64(&pair_route_key(&ti, &to_addr));
    if route_id == 0 { reentrancy_exit(); return 2; }

    let rk = route_key(route_id);
    let route_data = match storage_get(&rk) {
        Some(d) if d.len() >= ROUTE_SIZE => d,
        _ => { reentrancy_exit(); return 2; }
    };

    if !decode_route_enabled(&route_data) { reentrancy_exit(); return 2; }

    let rtype = decode_route_type(&route_data);
    let pool_id = decode_route_pool_id(&route_data);
    let secondary = decode_route_secondary_id(&route_data);
    let split_pct = decode_route_split_percent(&route_data);

    // Execute based on route type
    let amount_out = match rtype {
        ROUTE_DIRECT_CLOB => {
            execute_clob_swap(amount_in, pool_id)
        }
        ROUTE_DIRECT_AMM => {
            execute_amm_swap(amount_in, pool_id)
        }
        ROUTE_SPLIT => {
            let leg1_amount = amount_in * split_pct as u64 / 100;
            let leg2_amount = amount_in - leg1_amount;
            let out1 = execute_clob_swap(leg1_amount, pool_id);
            let out2 = execute_amm_swap(leg2_amount, secondary);
            out1 + out2
        }
        ROUTE_MULTI_HOP => {
            // First hop
            let mid_amount = execute_amm_swap(amount_in, pool_id);
            // Second hop
            execute_amm_swap(mid_amount, secondary)
        }
        ROUTE_LEGACY_SWAP => {
            execute_legacy_swap(amount_in, pool_id)
        }
        _ => 0,
    };

    if amount_out < min_amount_out { reentrancy_exit(); return 4; }

    // Record swap
    let swap_count = load_u64(SWAP_COUNT_KEY);
    let swap_id = swap_count + 1;
    let record = encode_swap_record(&t, amount_in, amount_out, rtype, current_slot, route_id);
    storage_set(&swap_record_key(swap_id), &record);
    save_u64(SWAP_COUNT_KEY, swap_id);

    moltchain_sdk::set_return_data(&u64_to_bytes(amount_out));
    log_info("Router swap executed");
    reentrancy_exit();
    0
}

/// Multi-hop swap through a specified path
/// Returns: 0=success, same error codes as swap
pub fn multi_hop_swap(
    trader: *const u8, path_ptr: *const u8, path_count: u64,
    amount_in: u64, min_out: u64, deadline: u64,
) -> u32 {
    if !reentrancy_enter() { return 5; }
    if !require_not_paused() { reentrancy_exit(); return 1; }
    if amount_in == 0 { reentrancy_exit(); return 6; }
    if path_count < 2 || path_count > MAX_HOPS + 1 { reentrancy_exit(); return 2; }

    let current_slot = get_slot();
    if deadline != 0 && current_slot > deadline { reentrancy_exit(); return 3; }

    let mut t = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(trader, t.as_mut_ptr(), 32); }

    // Read path — each entry is a pool_id (u64)
    let mut current_amount = amount_in;
    for i in 0..path_count.saturating_sub(1) {
        let offset = (i * 8) as usize;
        if offset + 8 > (path_count * 8) as usize { break; }
        let mut pool_bytes = [0u8; 8];
        unsafe {
            core::ptr::copy_nonoverlapping(path_ptr.add(offset), pool_bytes.as_mut_ptr(), 8);
        }
        let pool_id = u64::from_le_bytes(pool_bytes);
        current_amount = execute_amm_swap(current_amount, pool_id);
        if current_amount == 0 { reentrancy_exit(); return 4; }
    }

    if current_amount < min_out { reentrancy_exit(); return 4; }

    // Record
    let swap_count = load_u64(SWAP_COUNT_KEY);
    let swap_id = swap_count + 1;
    let record = encode_swap_record(&t, amount_in, current_amount, ROUTE_MULTI_HOP, current_slot, 0);
    storage_set(&swap_record_key(swap_id), &record);
    save_u64(SWAP_COUNT_KEY, swap_id);

    moltchain_sdk::set_return_data(&u64_to_bytes(current_amount));
    reentrancy_exit();
    0
}

/// Get best route for a token pair
pub fn get_best_route(token_in: *const u8, token_out: *const u8, _amount: u64) -> u64 {
    let mut ti = [0u8; 32];
    let mut to = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(token_in, ti.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(token_out, to.as_mut_ptr(), 32);
    }
    let route_id = load_u64(&pair_route_key(&ti, &to));
    if route_id > 0 {
        if let Some(data) = storage_get(&route_key(route_id)) {
            if data.len() >= ROUTE_SIZE && decode_route_enabled(&data) {
                moltchain_sdk::set_return_data(&data);
                return route_id;
            }
        }
    }
    0
}

// ============================================================================
// INTERNAL EXECUTION HELPERS
// ============================================================================

/// Execute swap via CLOB (dex_core) — in production uses cross-call
fn execute_clob_swap(amount_in: u64, pair_id: u64) -> u64 {
    // In production: cross-contract call to dex_core.place_order(market)
    // For now: simulate with 99.95% fill (5 bps taker fee)
    let fee = amount_in * 5 / 10_000;
    let _ = pair_id;
    amount_in.saturating_sub(fee)
}

/// Execute swap via AMM (dex_amm) — in production uses cross-call
fn execute_amm_swap(amount_in: u64, pool_id: u64) -> u64 {
    // In production: cross-contract call to dex_amm.swap_exact_in
    // Simulate with 99.7% fill (30 bps fee)
    let fee = amount_in * 30 / 10_000;
    let _ = pool_id;
    amount_in.saturating_sub(fee)
}

/// Execute swap via legacy MoltSwap
fn execute_legacy_swap(amount_in: u64, _pool_id: u64) -> u64 {
    // In production: cross-contract call to MoltSwap
    let fee = amount_in * 30 / 10_000;
    amount_in.saturating_sub(fee)
}

/// Emergency pause
pub fn emergency_pause(caller: *const u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    if !require_admin(&c) { return 1; }
    storage_set(PAUSED_KEY, &[1u8]);
    log_info("DEX Router: EMERGENCY PAUSE");
    0
}

pub fn emergency_unpause(caller: *const u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    if !require_admin(&c) { return 1; }
    storage_set(PAUSED_KEY, &[0u8]);
    0
}

// ============================================================================
// QUERIES
// ============================================================================

pub fn get_route_count() -> u64 { load_u64(ROUTE_COUNT_KEY) }
pub fn get_swap_count() -> u64 { load_u64(SWAP_COUNT_KEY) }

pub fn get_route_info(route_id: u64) -> u64 {
    let rk = route_key(route_id);
    match storage_get(&rk) {
        Some(d) if d.len() >= ROUTE_SIZE => {
            moltchain_sdk::set_return_data(&d);
            route_id
        }
        _ => 0,
    }
}

// ============================================================================
// WASM ENTRY POINT
// ============================================================================

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
        admin
    }

    fn token_a() -> [u8; 32] { [10u8; 32] }
    fn token_b() -> [u8; 32] { [20u8; 32] }
    fn token_c() -> [u8; 32] { [30u8; 32] }

    // --- Initialization ---

    #[test]
    fn test_initialize() {
        test_mock::reset();
        let admin = [1u8; 32];
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(load_addr(ADMIN_KEY), admin);
    }

    #[test]
    fn test_initialize_twice() {
        test_mock::reset();
        let admin = [1u8; 32];
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(initialize(admin.as_ptr()), 1);
    }

    // --- Address Configuration ---

    #[test]
    fn test_set_addresses() {
        let admin = setup();
        let core = [50u8; 32];
        let amm = [51u8; 32];
        let legacy = [52u8; 32];
        assert_eq!(set_addresses(admin.as_ptr(), core.as_ptr(), amm.as_ptr(), legacy.as_ptr()), 0);
        assert_eq!(load_addr(CORE_ADDRESS_KEY), core);
        assert_eq!(load_addr(AMM_ADDRESS_KEY), amm);
    }

    #[test]
    fn test_set_addresses_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        let core = [50u8; 32];
        assert_eq!(set_addresses(rando.as_ptr(), core.as_ptr(), core.as_ptr(), core.as_ptr()), 1);
    }

    // --- Route Registration ---

    #[test]
    fn test_register_direct_clob_route() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        assert_eq!(register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_DIRECT_CLOB, 1, 0, 0), 0);
        assert_eq!(get_route_count(), 1);
    }

    #[test]
    fn test_register_direct_amm_route() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        assert_eq!(register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_DIRECT_AMM, 1, 0, 0), 0);
    }

    #[test]
    fn test_register_split_route() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        assert_eq!(register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_SPLIT, 1, 2, 60), 0);
    }

    #[test]
    fn test_register_split_invalid_percent() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        // 0% split is invalid
        assert_eq!(register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_SPLIT, 1, 2, 0), 3);
        // 100% split is invalid (should use direct route)
        assert_eq!(register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_SPLIT, 1, 2, 100), 3);
    }

    #[test]
    fn test_register_route_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        let ta = token_a();
        let tb = token_b();
        assert_eq!(register_route(rando.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_DIRECT_AMM, 1, 0, 0), 1);
    }

    #[test]
    fn test_register_invalid_route_type() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        assert_eq!(register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), 10, 1, 0, 0), 3);
    }

    // --- Route Enable/Disable ---

    #[test]
    fn test_set_route_enabled() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_DIRECT_AMM, 1, 0, 0);
        assert_eq!(set_route_enabled(admin.as_ptr(), 1, false), 0);
        let rd = storage_get(&route_key(1)).unwrap();
        assert!(!decode_route_enabled(&rd));
    }

    // --- Swap Execution ---

    #[test]
    fn test_swap_direct_clob() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_DIRECT_CLOB, 1, 0, 0);
        assert_eq!(swap(trader.as_ptr(), ta.as_ptr(), tb.as_ptr(), 1_000_000, 0, 0), 0);

        let ret = test_mock::get_return_data();
        let out = bytes_to_u64(&ret);
        // CLOB: 5 bps fee → 1_000_000 - 500 = 999_500
        assert_eq!(out, 999_500);
    }

    #[test]
    fn test_swap_direct_amm() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_DIRECT_AMM, 1, 0, 0);
        assert_eq!(swap(trader.as_ptr(), ta.as_ptr(), tb.as_ptr(), 1_000_000, 0, 0), 0);

        let ret = test_mock::get_return_data();
        let out = bytes_to_u64(&ret);
        // AMM: 30 bps fee → 1_000_000 - 3_000 = 997_000
        assert_eq!(out, 997_000);
    }

    #[test]
    fn test_swap_split_route() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_SPLIT, 1, 2, 60);
        assert_eq!(swap(trader.as_ptr(), ta.as_ptr(), tb.as_ptr(), 1_000_000, 0, 0), 0);

        let ret = test_mock::get_return_data();
        let out = bytes_to_u64(&ret);
        // 60% CLOB: 600_000 * 0.9995 = 599_700
        // 40% AMM:  400_000 * 0.9970 = 398_800
        assert_eq!(out, 599_700 + 398_800);
    }

    #[test]
    fn test_swap_multi_hop() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_MULTI_HOP, 1, 2, 0);
        assert_eq!(swap(trader.as_ptr(), ta.as_ptr(), tb.as_ptr(), 1_000_000, 0, 0), 0);

        let ret = test_mock::get_return_data();
        let out = bytes_to_u64(&ret);
        // Hop1: 1_000_000 * 0.9970 = 997_000
        // Hop2: 997_000 * 0.9970 = 994_009
        assert_eq!(out, 994_009);
    }

    #[test]
    fn test_swap_no_route() {
        let _admin = setup();
        let trader = [2u8; 32];
        let ta = token_a();
        let tb = token_b();
        test_mock::set_slot(100);
        assert_eq!(swap(trader.as_ptr(), ta.as_ptr(), tb.as_ptr(), 1_000_000, 0, 0), 2);
    }

    #[test]
    fn test_swap_paused() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        let trader = [2u8; 32];
        register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_DIRECT_AMM, 1, 0, 0);
        emergency_pause(admin.as_ptr());
        assert_eq!(swap(trader.as_ptr(), ta.as_ptr(), tb.as_ptr(), 1_000_000, 0, 0), 1);
    }

    #[test]
    fn test_swap_deadline_expired() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_DIRECT_AMM, 1, 0, 0);
        assert_eq!(swap(trader.as_ptr(), ta.as_ptr(), tb.as_ptr(), 1_000_000, 0, 50), 3);
    }

    #[test]
    fn test_swap_slippage_exceeded() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_DIRECT_AMM, 1, 0, 0);
        // min_out = input amount (impossible with fees)
        assert_eq!(swap(trader.as_ptr(), ta.as_ptr(), tb.as_ptr(), 1_000_000, 1_000_000, 0), 4);
    }

    #[test]
    fn test_swap_zero_amount() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        let trader = [2u8; 32];
        register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_DIRECT_AMM, 1, 0, 0);
        assert_eq!(swap(trader.as_ptr(), ta.as_ptr(), tb.as_ptr(), 0, 0, 0), 6);
    }

    #[test]
    fn test_swap_disabled_route() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_DIRECT_AMM, 1, 0, 0);
        set_route_enabled(admin.as_ptr(), 1, false);
        assert_eq!(swap(trader.as_ptr(), ta.as_ptr(), tb.as_ptr(), 1_000_000, 0, 0), 2);
    }

    // --- Swap Records ---

    #[test]
    fn test_swap_count_increments() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_DIRECT_AMM, 1, 0, 0);
        swap(trader.as_ptr(), ta.as_ptr(), tb.as_ptr(), 1_000_000, 0, 0);
        swap(trader.as_ptr(), ta.as_ptr(), tb.as_ptr(), 2_000_000, 0, 0);
        assert_eq!(get_swap_count(), 2);
    }

    // --- Get Route ---

    #[test]
    fn test_get_best_route() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_DIRECT_AMM, 1, 0, 0);
        assert_eq!(get_best_route(ta.as_ptr(), tb.as_ptr(), 1_000_000), 1);
    }

    #[test]
    fn test_get_best_route_none() {
        let _admin = setup();
        let ta = token_a();
        let tb = token_b();
        assert_eq!(get_best_route(ta.as_ptr(), tb.as_ptr(), 1_000_000), 0);
    }

    #[test]
    fn test_get_route_info() {
        let admin = setup();
        let ta = token_a();
        let tb = token_b();
        register_route(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), ROUTE_DIRECT_AMM, 1, 0, 0);
        assert_eq!(get_route_info(1), 1);
        assert_eq!(get_route_info(999), 0);
    }

    // --- Emergency ---

    #[test]
    fn test_emergency_pause_unpause() {
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
        assert_eq!(emergency_pause(rando.as_ptr()), 1);
    }
}
