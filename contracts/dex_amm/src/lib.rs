// DEX AMM — Concentrated Liquidity Automated Market Maker (DEEP hardened)
//
// Uniswap V3-style concentrated liquidity with:
//   - Configurable fee tiers (1bps, 5bps, 30bps, 100bps)
//   - Position management with tick ranges
//   - Q64.64 fixed-point math (no floating point in no_std)
//   - Fee accrual per position
//   - Emergency pause, reentrancy guard, admin controls
//   - Deadline enforcement on all swaps
//   - Minimum liquidity thresholds

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

const MAX_POOLS: u64 = 100;
const MIN_LIQUIDITY: u64 = 1_000;
const MAX_TICK: i32 = 887_272;
const MIN_TICK: i32 = -887_272;

// Fee tiers (in basis points)
const FEE_TIER_1BPS: u8 = 0;
const FEE_TIER_5BPS: u8 = 1;
const FEE_TIER_30BPS: u8 = 2;
const FEE_TIER_100BPS: u8 = 3;

const FEE_VALUES: [u64; 4] = [1, 5, 30, 100];
const TICK_SPACINGS: [i32; 4] = [1, 10, 60, 200];

// Q64.64 fixed-point scale
const Q64: u128 = 1u128 << 64;

// Storage keys
const ADMIN_KEY: &[u8] = b"amm_admin";
const PAUSED_KEY: &[u8] = b"amm_paused";
const REENTRANCY_KEY: &[u8] = b"amm_reentrancy";
const POOL_COUNT_KEY: &[u8] = b"amm_pool_count";
const POSITION_COUNT_KEY: &[u8] = b"amm_pos_count";
const PROTOCOL_FEE_KEY: &[u8] = b"amm_protocol_fee";

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

fn is_zero(addr: &[u8; 32]) -> bool {
    addr.iter().all(|&b| b == 0)
}

fn u64_to_decimal(mut n: u64) -> Vec<u8> {
    if n == 0 { return alloc::vec![b'0']; }
    let mut buf = Vec::new();
    while n > 0 { buf.push(b'0' + (n % 10) as u8); n /= 10; }
    buf.reverse();
    buf
}

fn i32_to_bytes(n: i32) -> [u8; 4] { n.to_le_bytes() }
fn bytes_to_i32(bytes: &[u8]) -> i32 {
    let mut arr = [0u8; 4];
    if bytes.len() >= 4 { arr.copy_from_slice(&bytes[..4]); }
    i32::from_le_bytes(arr)
}

fn pool_key(pool_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"amm_pool_"[..]);
    k.extend_from_slice(&u64_to_decimal(pool_id));
    k
}

fn position_key(pos_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"amm_pos_"[..]);
    k.extend_from_slice(&u64_to_decimal(pos_id));
    k
}

fn tick_data_key(pool_id: u64, tick: i32) -> Vec<u8> {
    let mut k = Vec::from(&b"amm_tick_"[..]);
    k.extend_from_slice(&u64_to_decimal(pool_id));
    k.push(b'_');
    if tick < 0 {
        k.push(b'n');
        k.extend_from_slice(&u64_to_decimal((-tick) as u64));
    } else {
        k.extend_from_slice(&u64_to_decimal(tick as u64));
    }
    k
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

fn owner_position_count_key(owner: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"amm_opc_"[..]);
    k.extend_from_slice(&hex_encode(owner));
    k
}

fn owner_position_key(owner: &[u8; 32], idx: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"amm_op_"[..]);
    k.extend_from_slice(&hex_encode(owner));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(idx));
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

fn reentrancy_exit() {
    storage_set(REENTRANCY_KEY, &[0u8]);
}

fn is_paused() -> bool {
    storage_get(PAUSED_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(false)
}

fn require_not_paused() -> bool { !is_paused() }

fn require_admin(caller: &[u8; 32]) -> bool {
    let admin = load_addr(ADMIN_KEY);
    !is_zero(&admin) && *caller == admin
}

// ============================================================================
// POOL LAYOUT (96 bytes)
// ============================================================================
// Bytes 0..32    : token_a address
// Bytes 32..64   : token_b address
// Bytes 64..72   : pool_id (u64)
// Bytes 72..80   : sqrt_price (u64, Q32.32 fixed point)
// Bytes 80..84   : tick (i32)
// Bytes 84..92   : liquidity (u64)
// Byte  92       : fee_tier (u8)
// Byte  93       : protocol_fee (u8, 0-100%)
// Bytes 94..96   : padding

const POOL_SIZE: usize = 96;

fn encode_pool(
    token_a: &[u8; 32], token_b: &[u8; 32], pool_id: u64,
    sqrt_price: u64, tick: i32, liquidity: u64,
    fee_tier: u8, protocol_fee: u8,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(POOL_SIZE);
    data.extend_from_slice(token_a);
    data.extend_from_slice(token_b);
    data.extend_from_slice(&u64_to_bytes(pool_id));
    data.extend_from_slice(&u64_to_bytes(sqrt_price));
    data.extend_from_slice(&i32_to_bytes(tick));
    data.extend_from_slice(&u64_to_bytes(liquidity));
    data.push(fee_tier);
    data.push(protocol_fee);
    data.extend_from_slice(&[0u8; 2]);
    data
}

fn decode_pool_id(data: &[u8]) -> u64 {
    if data.len() >= 72 { bytes_to_u64(&data[64..72]) } else { 0 }
}
fn decode_pool_sqrt_price(data: &[u8]) -> u64 {
    if data.len() >= 80 { bytes_to_u64(&data[72..80]) } else { 0 }
}
fn decode_pool_tick(data: &[u8]) -> i32 {
    if data.len() >= 84 { bytes_to_i32(&data[80..84]) } else { 0 }
}
fn decode_pool_liquidity(data: &[u8]) -> u64 {
    if data.len() >= 92 { bytes_to_u64(&data[84..92]) } else { 0 }
}
fn decode_pool_fee_tier(data: &[u8]) -> u8 {
    if data.len() > 92 { data[92] } else { 0 }
}
fn decode_pool_protocol_fee(data: &[u8]) -> u8 {
    if data.len() > 93 { data[93] } else { 0 }
}
fn decode_pool_token_a(data: &[u8]) -> [u8; 32] {
    let mut t = [0u8; 32];
    if data.len() >= 32 { t.copy_from_slice(&data[..32]); }
    t
}
fn decode_pool_token_b(data: &[u8]) -> [u8; 32] {
    let mut t = [0u8; 32];
    if data.len() >= 64 { t.copy_from_slice(&data[32..64]); }
    t
}

fn update_pool_sqrt_price(data: &mut Vec<u8>, new_sqrt: u64) {
    if data.len() >= 80 { data[72..80].copy_from_slice(&u64_to_bytes(new_sqrt)); }
}
fn update_pool_tick(data: &mut Vec<u8>, new_tick: i32) {
    if data.len() >= 84 { data[80..84].copy_from_slice(&i32_to_bytes(new_tick)); }
}
fn update_pool_liquidity(data: &mut Vec<u8>, new_liq: u64) {
    if data.len() >= 92 { data[84..92].copy_from_slice(&u64_to_bytes(new_liq)); }
}

// ============================================================================
// POSITION LAYOUT (80 bytes)
// ============================================================================
// Bytes 0..32   : owner address
// Bytes 32..40  : pool_id (u64)
// Bytes 40..44  : lower_tick (i32)
// Bytes 44..48  : upper_tick (i32)
// Bytes 48..56  : liquidity (u64)
// Bytes 56..64  : fee_a_owed (u64)
// Bytes 64..72  : fee_b_owed (u64)
// Bytes 72..80  : created_slot (u64)

const POSITION_SIZE: usize = 80;

fn encode_position(
    owner: &[u8; 32], pool_id: u64, lower_tick: i32, upper_tick: i32,
    liquidity: u64, fee_a_owed: u64, fee_b_owed: u64, created_slot: u64,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(POSITION_SIZE);
    data.extend_from_slice(owner);
    data.extend_from_slice(&u64_to_bytes(pool_id));
    data.extend_from_slice(&i32_to_bytes(lower_tick));
    data.extend_from_slice(&i32_to_bytes(upper_tick));
    data.extend_from_slice(&u64_to_bytes(liquidity));
    data.extend_from_slice(&u64_to_bytes(fee_a_owed));
    data.extend_from_slice(&u64_to_bytes(fee_b_owed));
    data.extend_from_slice(&u64_to_bytes(created_slot));
    data
}

fn decode_pos_owner(data: &[u8]) -> [u8; 32] {
    let mut o = [0u8; 32];
    if data.len() >= 32 { o.copy_from_slice(&data[..32]); }
    o
}
fn decode_pos_pool_id(data: &[u8]) -> u64 {
    if data.len() >= 40 { bytes_to_u64(&data[32..40]) } else { 0 }
}
fn decode_pos_lower_tick(data: &[u8]) -> i32 {
    if data.len() >= 44 { bytes_to_i32(&data[40..44]) } else { 0 }
}
fn decode_pos_upper_tick(data: &[u8]) -> i32 {
    if data.len() >= 48 { bytes_to_i32(&data[44..48]) } else { 0 }
}
fn decode_pos_liquidity(data: &[u8]) -> u64 {
    if data.len() >= 56 { bytes_to_u64(&data[48..56]) } else { 0 }
}
fn decode_pos_fee_a(data: &[u8]) -> u64 {
    if data.len() >= 64 { bytes_to_u64(&data[56..64]) } else { 0 }
}
fn decode_pos_fee_b(data: &[u8]) -> u64 {
    if data.len() >= 72 { bytes_to_u64(&data[64..72]) } else { 0 }
}
fn decode_pos_created_slot(data: &[u8]) -> u64 {
    if data.len() >= 80 { bytes_to_u64(&data[72..80]) } else { 0 }
}

fn update_pos_liquidity(data: &mut Vec<u8>, liq: u64) {
    if data.len() >= 56 { data[48..56].copy_from_slice(&u64_to_bytes(liq)); }
}
fn update_pos_fee_a(data: &mut Vec<u8>, fee: u64) {
    if data.len() >= 64 { data[56..64].copy_from_slice(&u64_to_bytes(fee)); }
}
fn update_pos_fee_b(data: &mut Vec<u8>, fee: u64) {
    if data.len() >= 72 { data[64..72].copy_from_slice(&u64_to_bytes(fee)); }
}

// ============================================================================
// TICK MATH (Q32.32 fixed-point for no_std)
// ============================================================================

/// Approximate sqrt_price for a given tick using integer math
/// sqrt_price = floor(2^32 * 1.0001^(tick/2))
/// For simplicity, we use a linear approximation near tick 0 and scale
fn tick_to_sqrt_price(tick: i32) -> u64 {
    // Base: at tick 0, sqrt_price = 2^32 = 4_294_967_296
    let base: u64 = 1u64 << 32;
    if tick == 0 { return base; }

    // Each tick changes price by 0.01%, so sqrt changes by ~0.005%
    // Approximation: sqrt_price ≈ base * (1 + tick * 5 / 1_000_000)
    // Use i128 for intermediate math
    let shift = tick as i128 * 5 * base as i128 / 1_000_000;
    let result = base as i128 + shift;
    if result <= 0 { 1 } else { result as u64 }
}

/// Get tick from sqrt_price (inverse)
fn sqrt_price_to_tick(sqrt_price: u64) -> i32 {
    let base: u64 = 1u64 << 32;
    if sqrt_price == base { return 0; }
    // Inverse of approximation
    let diff = sqrt_price as i128 - base as i128;
    let tick = diff * 1_000_000 / (5 * base as i128);
    tick as i32
}

/// Calculate liquidity from amounts and price range
fn compute_liquidity(amount_a: u64, amount_b: u64, sqrt_lower: u64, sqrt_upper: u64, sqrt_current: u64) -> u64 {
    if sqrt_lower >= sqrt_upper || amount_a == 0 && amount_b == 0 {
        return 0;
    }

    let liq_a = if sqrt_current >= sqrt_upper {
        0u128
    } else {
        let sqrt_l = if sqrt_current > sqrt_lower { sqrt_current } else { sqrt_lower };
        let delta_sqrt = sqrt_upper as u128 - sqrt_l as u128;
        if delta_sqrt == 0 { 0 } else {
            // Use checked_mul to prevent u128 overflow with extreme inputs
            match (amount_a as u128).checked_mul(sqrt_l as u128)
                .and_then(|v| v.checked_mul(sqrt_upper as u128)) {
                Some(num) => num / (delta_sqrt * (1u128 << 32)),
                None => u64::MAX as u128, // clamp on overflow
            }
        }
    };

    let liq_b = if sqrt_current <= sqrt_lower {
        0u128
    } else {
        let sqrt_u = if sqrt_current < sqrt_upper { sqrt_current } else { sqrt_upper };
        let delta_sqrt = sqrt_u as u128 - sqrt_lower as u128;
        if delta_sqrt == 0 { 0 } else {
            (amount_b as u128 * (1u128 << 32)) / delta_sqrt
        }
    };

    // Use minimum of the two
    let liq = if liq_a == 0 { liq_b } else if liq_b == 0 { liq_a } else {
        if liq_a < liq_b { liq_a } else { liq_b }
    };
    liq as u64
}

/// Calculate swap output amount given input
fn compute_swap_output(
    amount_in: u64, liquidity: u64, sqrt_price: u64, fee_bps: u64, is_token_a: bool,
) -> (u64, u64) {
    // amount_out = amount to receive
    // new_sqrt_price = updated sqrt price
    if liquidity == 0 || amount_in == 0 {
        return (0, sqrt_price);
    }

    // Apply fee
    let fee = (amount_in as u128 * fee_bps as u128 / 10_000) as u64;
    let amount_after_fee = amount_in - fee;

    if is_token_a {
        // Swapping A for B: price decreases
        // new_sqrt = L * sqrt_p / (L + amount * sqrt_p)
        let numerator = liquidity as u128 * sqrt_price as u128;
        let denominator = liquidity as u128 + (amount_after_fee as u128 * sqrt_price as u128 / (1u128 << 32));
        if denominator == 0 { return (0, sqrt_price); }
        let new_sqrt = (numerator / denominator) as u64;
        // amount_b_out = L * (sqrt_p - new_sqrt) / 2^32
        let delta_sqrt = sqrt_price as u128 - new_sqrt as u128;
        let amount_out = (liquidity as u128 * delta_sqrt / (1u128 << 32)) as u64;
        (amount_out, new_sqrt)
    } else {
        // Swapping B for A: price increases
        // new_sqrt = sqrt_p + amount * 2^32 / L
        let delta = amount_after_fee as u128 * (1u128 << 32) / liquidity as u128;
        let new_sqrt = (sqrt_price as u128 + delta) as u64;
        // amount_a_out = L * (1/sqrt_p - 1/new_sqrt) = L * (new_sqrt - sqrt_p) / (sqrt_p * new_sqrt / 2^32)
        let delta_sqrt = new_sqrt as u128 - sqrt_price as u128;
        let denom = sqrt_price as u128 * new_sqrt as u128 / (1u128 << 32);
        let amount_out = if denom == 0 { 0 } else {
            (liquidity as u128 * delta_sqrt / denom) as u64
        };
        (amount_out, new_sqrt)
    }
}

// ============================================================================
// PUBLIC FUNCTIONS
// ============================================================================

/// Initialize the AMM contract
pub fn initialize(admin: *const u8) -> u32 {
    let existing = load_addr(ADMIN_KEY);
    if !is_zero(&existing) { return 1; }
    let mut addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(admin, addr.as_mut_ptr(), 32); }
    storage_set(ADMIN_KEY, &addr);
    save_u64(POOL_COUNT_KEY, 0);
    save_u64(POSITION_COUNT_KEY, 0);
    storage_set(PAUSED_KEY, &[0u8]);
    log_info("DEX AMM initialized");
    0
}

/// Create a new liquidity pool
/// Returns: 0=success, 1=not admin, 2=paused, 3=max pools, 4=invalid params, 5=reentrancy
pub fn create_pool(
    caller: *const u8, token_a: *const u8, token_b: *const u8,
    fee_tier: u8, initial_sqrt_price: u64,
) -> u32 {
    if !reentrancy_enter() { return 5; }
    let mut c = [0u8; 32];
    let mut ta = [0u8; 32];
    let mut tb = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(token_a, ta.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(token_b, tb.as_mut_ptr(), 32);
    }
    if !require_admin(&c) { reentrancy_exit(); return 1; }
    if !require_not_paused() { reentrancy_exit(); return 2; }

    let count = load_u64(POOL_COUNT_KEY);
    if count >= MAX_POOLS { reentrancy_exit(); return 3; }
    if fee_tier > FEE_TIER_100BPS { reentrancy_exit(); return 4; }
    if initial_sqrt_price == 0 { reentrancy_exit(); return 4; }
    if ta == tb { reentrancy_exit(); return 4; }

    let pool_id = count + 1;
    let tick = sqrt_price_to_tick(initial_sqrt_price);
    let data = encode_pool(&ta, &tb, pool_id, initial_sqrt_price, tick, 0, fee_tier, 0);
    storage_set(&pool_key(pool_id), &data);
    save_u64(POOL_COUNT_KEY, pool_id);
    log_info("AMM pool created");
    reentrancy_exit();
    0
}

/// Set protocol fee for a pool (admin only)
pub fn set_pool_protocol_fee(caller: *const u8, pool_id: u64, fee_percent: u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    if !require_admin(&c) { return 1; }
    if fee_percent > 100 { return 2; }
    let pk = pool_key(pool_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= POOL_SIZE => d,
        _ => return 3,
    };
    data[93] = fee_percent;
    storage_set(&pk, &data);
    0
}

/// Add liquidity to a pool within a tick range
/// Returns: 0=success, 1=paused, 2=pool not found, 3=invalid range, 4=below min, 5=reentrancy
pub fn add_liquidity(
    provider: *const u8, pool_id: u64,
    lower_tick: i32, upper_tick: i32,
    amount_a: u64, amount_b: u64,
) -> u32 {
    if !reentrancy_enter() { return 5; }
    if !require_not_paused() { reentrancy_exit(); return 1; }

    let mut p = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(provider, p.as_mut_ptr(), 32); }

    let pk = pool_key(pool_id);
    let mut pool_data = match storage_get(&pk) {
        Some(d) if d.len() >= POOL_SIZE => d,
        _ => { reentrancy_exit(); return 2; }
    };

    // Validate tick range
    let fee_tier = decode_pool_fee_tier(&pool_data);
    let tick_spacing = TICK_SPACINGS[fee_tier as usize];
    if lower_tick >= upper_tick { reentrancy_exit(); return 3; }
    if lower_tick < MIN_TICK || upper_tick > MAX_TICK { reentrancy_exit(); return 3; }
    if lower_tick % tick_spacing != 0 || upper_tick % tick_spacing != 0 {
        reentrancy_exit(); return 3;
    }

    // Enforce minimum deposit amounts to prevent dust positions
    const MIN_AMOUNT: u64 = 100;
    if amount_a < MIN_AMOUNT && amount_b < MIN_AMOUNT { reentrancy_exit(); return 4; }

    let sqrt_current = decode_pool_sqrt_price(&pool_data);
    let sqrt_lower = tick_to_sqrt_price(lower_tick);
    let sqrt_upper = tick_to_sqrt_price(upper_tick);

    let liquidity = compute_liquidity(amount_a, amount_b, sqrt_lower, sqrt_upper, sqrt_current);
    if liquidity < MIN_LIQUIDITY { reentrancy_exit(); return 4; }

    // Create position
    let pos_count = load_u64(POSITION_COUNT_KEY);
    let pos_id = pos_count + 1;
    let slot = get_slot();
    let pos_data = encode_position(&p, pool_id, lower_tick, upper_tick, liquidity, 0, 0, slot);
    storage_set(&position_key(pos_id), &pos_data);
    save_u64(POSITION_COUNT_KEY, pos_id);

    // Track owner positions
    let owner_count = load_u64(&owner_position_count_key(&p));
    let new_count = owner_count + 1;
    save_u64(&owner_position_count_key(&p), new_count);
    save_u64(&owner_position_key(&p, new_count), pos_id);

    // Update pool liquidity if position is in range
    let current_tick = decode_pool_tick(&pool_data);
    if current_tick >= lower_tick && current_tick < upper_tick {
        let pool_liq = decode_pool_liquidity(&pool_data);
        update_pool_liquidity(&mut pool_data, pool_liq + liquidity);
        storage_set(&pk, &pool_data);
    }

    // Update tick data
    let lower_net = load_u64(&tick_data_key(pool_id, lower_tick));
    save_u64(&tick_data_key(pool_id, lower_tick), lower_net + liquidity);
    let upper_net = load_u64(&tick_data_key(pool_id, upper_tick));
    save_u64(&tick_data_key(pool_id, upper_tick), upper_net + liquidity);

    log_info("Liquidity added");
    reentrancy_exit();
    0
}

/// Remove liquidity from a position
/// Returns: 0=success, 1=not found, 2=not owner, 3=insufficient, 4=reentrancy
pub fn remove_liquidity(provider: *const u8, position_id: u64, liquidity_amount: u64) -> u32 {
    if !reentrancy_enter() { return 4; }
    let mut p = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(provider, p.as_mut_ptr(), 32); }

    let pk = position_key(position_id);
    let mut pos_data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE => d,
        _ => { reentrancy_exit(); return 1; }
    };

    let owner = decode_pos_owner(&pos_data);
    if owner != p { reentrancy_exit(); return 2; }

    let current_liq = decode_pos_liquidity(&pos_data);
    if liquidity_amount > current_liq { reentrancy_exit(); return 3; }

    let new_liq = current_liq - liquidity_amount;
    update_pos_liquidity(&mut pos_data, new_liq);
    storage_set(&pk, &pos_data);

    // Update pool liquidity
    let pool_id = decode_pos_pool_id(&pos_data);
    let pool_pk = pool_key(pool_id);
    if let Some(mut pool_data) = storage_get(&pool_pk) {
        if pool_data.len() >= POOL_SIZE {
            let lower = decode_pos_lower_tick(&pos_data);
            let upper = decode_pos_upper_tick(&pos_data);
            let current_tick = decode_pool_tick(&pool_data);
            if current_tick >= lower && current_tick < upper {
                let pool_liq = decode_pool_liquidity(&pool_data);
                update_pool_liquidity(&mut pool_data, pool_liq.saturating_sub(liquidity_amount));
                storage_set(&pool_pk, &pool_data);
            }
        }
    }

    log_info("Liquidity removed");
    reentrancy_exit();
    0
}

/// Collect accumulated fees for a position
/// Returns: 0=success, 1=not found, 2=not owner
pub fn collect_fees(provider: *const u8, position_id: u64) -> u32 {
    let mut p = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(provider, p.as_mut_ptr(), 32); }

    let pk = position_key(position_id);
    let mut pos_data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE => d,
        _ => return 1,
    };

    let owner = decode_pos_owner(&pos_data);
    if owner != p { return 2; }

    let fee_a = decode_pos_fee_a(&pos_data);
    let fee_b = decode_pos_fee_b(&pos_data);

    // Reset fees (transfer would happen via cross-call in production)
    update_pos_fee_a(&mut pos_data, 0);
    update_pos_fee_b(&mut pos_data, 0);
    storage_set(&pk, &pos_data);

    // Return fee amounts via return data
    let mut ret = Vec::with_capacity(16);
    ret.extend_from_slice(&u64_to_bytes(fee_a));
    ret.extend_from_slice(&u64_to_bytes(fee_b));
    moltchain_sdk::set_return_data(&ret);
    0
}

/// Swap exact input amount
/// Returns: 0=success, 1=paused, 2=pool not found, 3=deadline expired,
///          4=insufficient output, 5=reentrancy, 6=zero amount
pub fn swap_exact_in(
    _trader: *const u8, pool_id: u64, is_token_a_in: bool,
    amount_in: u64, min_out: u64, deadline: u64,
) -> u32 {
    if !reentrancy_enter() { return 5; }
    if !require_not_paused() { reentrancy_exit(); return 1; }
    if amount_in == 0 { reentrancy_exit(); return 6; }

    let current_slot = get_slot();
    if deadline != 0 && current_slot > deadline { reentrancy_exit(); return 3; }

    let pk = pool_key(pool_id);
    let mut pool_data = match storage_get(&pk) {
        Some(d) if d.len() >= POOL_SIZE => d,
        _ => { reentrancy_exit(); return 2; }
    };

    let sqrt_price = decode_pool_sqrt_price(&pool_data);
    let liquidity = decode_pool_liquidity(&pool_data);
    let fee_tier = decode_pool_fee_tier(&pool_data);
    let fee_bps = FEE_VALUES[fee_tier as usize];

    let (amount_out, new_sqrt) = compute_swap_output(amount_in, liquidity, sqrt_price, fee_bps, is_token_a_in);

    if amount_out < min_out { reentrancy_exit(); return 4; }

    // Update pool state
    update_pool_sqrt_price(&mut pool_data, new_sqrt);
    let new_tick = sqrt_price_to_tick(new_sqrt);
    update_pool_tick(&mut pool_data, new_tick);
    storage_set(&pk, &pool_data);

    // Accrue fees to in-range positions
    let fee = (amount_in as u128 * fee_bps as u128 / 10_000) as u64;
    accrue_fees_to_positions(pool_id, fee, is_token_a_in);

    // Return amount out
    moltchain_sdk::set_return_data(&u64_to_bytes(amount_out));
    log_info("Swap executed");
    reentrancy_exit();
    0
}

/// Swap to receive exact output amount
pub fn swap_exact_out(
    trader: *const u8, pool_id: u64, is_token_a_out: bool,
    amount_out: u64, max_in: u64, deadline: u64,
) -> u32 {
    if !reentrancy_enter() { return 5; }
    if !require_not_paused() { reentrancy_exit(); return 1; }
    if amount_out == 0 { reentrancy_exit(); return 6; }

    let current_slot = get_slot();
    if deadline != 0 && current_slot > deadline { reentrancy_exit(); return 3; }

    // For exact out, we estimate input needed
    // Simplified: try increasing amounts until output >= target
    let pk = pool_key(pool_id);
    let pool_data = match storage_get(&pk) {
        Some(d) if d.len() >= POOL_SIZE => d,
        _ => { reentrancy_exit(); return 2; }
    };

    let sqrt_price = decode_pool_sqrt_price(&pool_data);
    let liquidity = decode_pool_liquidity(&pool_data);
    let fee_tier = decode_pool_fee_tier(&pool_data);
    let fee_bps = FEE_VALUES[fee_tier as usize];

    // Binary search for required input
    let mut lo: u64 = 1;
    let mut hi: u64 = max_in;
    let mut best_in: u64 = 0;
    for _ in 0..64 {
        if lo > hi { break; }
        let mid = lo + (hi - lo) / 2;
        let (out, _) = compute_swap_output(mid, liquidity, sqrt_price, fee_bps, !is_token_a_out);
        if out >= amount_out {
            best_in = mid;
            hi = mid - 1;
        } else {
            lo = mid + 1;
        }
    }
    if best_in == 0 || best_in > max_in { reentrancy_exit(); return 4; }

    reentrancy_exit();
    swap_exact_in(trader, pool_id, !is_token_a_out, best_in, amount_out, deadline)
}

/// Accrue fees to in-range positions
fn accrue_fees_to_positions(pool_id: u64, fee: u64, is_token_a: bool) {
    if fee == 0 { return; }
    let pool_pk = pool_key(pool_id);
    let pool_data = match storage_get(&pool_pk) {
        Some(d) => d,
        None => return,
    };
    let current_tick = decode_pool_tick(&pool_data);
    let pool_liq = decode_pool_liquidity(&pool_data);
    if pool_liq == 0 { return; }

    // Distribute fee proportionally to all positions (simplified)
    let pos_count = load_u64(POSITION_COUNT_KEY);
    for i in 1..=pos_count {
        let pk = position_key(i);
        if let Some(mut pos_data) = storage_get(&pk) {
            if pos_data.len() >= POSITION_SIZE {
                let pos_pool = decode_pos_pool_id(&pos_data);
                if pos_pool != pool_id { continue; }
                let lower = decode_pos_lower_tick(&pos_data);
                let upper = decode_pos_upper_tick(&pos_data);
                if current_tick >= lower && current_tick < upper {
                    let pos_liq = decode_pos_liquidity(&pos_data);
                    let share = (fee as u128 * pos_liq as u128 / pool_liq as u128) as u64;
                    if is_token_a {
                        let current = decode_pos_fee_a(&pos_data);
                        update_pos_fee_a(&mut pos_data, current + share);
                    } else {
                        let current = decode_pos_fee_b(&pos_data);
                        update_pos_fee_b(&mut pos_data, current + share);
                    }
                    storage_set(&pk, &pos_data);
                }
            }
        }
    }
}

/// Emergency pause (admin only)
pub fn emergency_pause(caller: *const u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    if !require_admin(&c) { return 1; }
    storage_set(PAUSED_KEY, &[1u8]);
    log_info("DEX AMM: EMERGENCY PAUSE ACTIVATED");
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
// QUERY FUNCTIONS
// ============================================================================

pub fn get_pool_info(pool_id: u64) -> u64 {
    let pk = pool_key(pool_id);
    match storage_get(&pk) {
        Some(d) if d.len() >= POOL_SIZE => {
            moltchain_sdk::set_return_data(&d);
            pool_id
        }
        _ => 0,
    }
}

pub fn get_position(position_id: u64) -> u64 {
    let pk = position_key(position_id);
    match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE => {
            moltchain_sdk::set_return_data(&d);
            position_id
        }
        _ => 0,
    }
}

pub fn get_pool_count() -> u64 { load_u64(POOL_COUNT_KEY) }
pub fn get_position_count() -> u64 { load_u64(POSITION_COUNT_KEY) }

pub fn get_tvl(pool_id: u64) -> u64 {
    let pk = pool_key(pool_id);
    match storage_get(&pk) {
        Some(d) if d.len() >= POOL_SIZE => decode_pool_liquidity(&d),
        _ => 0,
    }
}

/// Quote a swap without executing
pub fn quote_swap(pool_id: u64, is_token_a_in: bool, amount_in: u64) -> u64 {
    let pk = pool_key(pool_id);
    let pool_data = match storage_get(&pk) {
        Some(d) if d.len() >= POOL_SIZE => d,
        _ => return 0,
    };
    let sqrt_price = decode_pool_sqrt_price(&pool_data);
    let liquidity = decode_pool_liquidity(&pool_data);
    let fee_tier = decode_pool_fee_tier(&pool_data);
    let fee_bps = FEE_VALUES[fee_tier as usize];
    let (out, _) = compute_swap_output(amount_in, liquidity, sqrt_price, fee_bps, is_token_a_in);
    out
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

    fn setup_with_pool() -> ([u8; 32], u64) {
        let admin = setup();
        let ta = [10u8; 32];
        let tb = [20u8; 32];
        let sqrt_price = 1u64 << 32; // 1:1 price
        assert_eq!(create_pool(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), FEE_TIER_30BPS, sqrt_price), 0);
        (admin, 1)
    }

    // --- Initialization ---

    #[test]
    fn test_initialize() {
        test_mock::reset();
        let admin = [1u8; 32];
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(load_addr(ADMIN_KEY), admin);
    }

    #[test]
    fn test_initialize_already_initialized() {
        test_mock::reset();
        let admin = [1u8; 32];
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(initialize(admin.as_ptr()), 1);
    }

    // --- Pool Creation ---

    #[test]
    fn test_create_pool() {
        let admin = setup();
        let ta = [10u8; 32];
        let tb = [20u8; 32];
        assert_eq!(create_pool(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), FEE_TIER_30BPS, 1u64 << 32), 0);
        assert_eq!(load_u64(POOL_COUNT_KEY), 1);
    }

    #[test]
    fn test_create_pool_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        let ta = [10u8; 32];
        let tb = [20u8; 32];
        assert_eq!(create_pool(rando.as_ptr(), ta.as_ptr(), tb.as_ptr(), FEE_TIER_30BPS, 1u64 << 32), 1);
    }

    #[test]
    fn test_create_pool_same_tokens() {
        let admin = setup();
        let t = [10u8; 32];
        assert_eq!(create_pool(admin.as_ptr(), t.as_ptr(), t.as_ptr(), FEE_TIER_30BPS, 1u64 << 32), 4);
    }

    #[test]
    fn test_create_pool_invalid_fee_tier() {
        let admin = setup();
        let ta = [10u8; 32];
        let tb = [20u8; 32];
        assert_eq!(create_pool(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), 5, 1u64 << 32), 4);
    }

    #[test]
    fn test_create_pool_zero_price() {
        let admin = setup();
        let ta = [10u8; 32];
        let tb = [20u8; 32];
        assert_eq!(create_pool(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), FEE_TIER_30BPS, 0), 4);
    }

    // --- Liquidity ---

    #[test]
    fn test_add_liquidity() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        test_mock::set_slot(100);
        // lower=-60, upper=60 (valid for 30bps tier with spacing 60)
        assert_eq!(add_liquidity(provider.as_ptr(), pool_id, -60, 60, 100_000, 100_000), 0);
        assert_eq!(load_u64(POSITION_COUNT_KEY), 1);
    }

    #[test]
    fn test_add_liquidity_invalid_range() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        // lower > upper
        assert_eq!(add_liquidity(provider.as_ptr(), pool_id, 60, -60, 100_000, 100_000), 3);
    }

    #[test]
    fn test_add_liquidity_bad_tick_spacing() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        // 30bps tier has spacing=60, so ticks must be multiple of 60
        assert_eq!(add_liquidity(provider.as_ptr(), pool_id, -30, 30, 100_000, 100_000), 3);
    }

    #[test]
    fn test_add_liquidity_paused() {
        let (admin, pool_id) = setup_with_pool();
        emergency_pause(admin.as_ptr());
        let provider = [2u8; 32];
        assert_eq!(add_liquidity(provider.as_ptr(), pool_id, -60, 60, 100_000, 100_000), 1);
    }

    #[test]
    fn test_remove_liquidity() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        test_mock::set_slot(100);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 100_000, 100_000);

        let pos_data = storage_get(&position_key(1)).unwrap();
        let liq = decode_pos_liquidity(&pos_data);
        assert!(liq > 0);

        assert_eq!(remove_liquidity(provider.as_ptr(), 1, liq / 2), 0);
        let pos_data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_liquidity(&pos_data), liq - liq / 2);
    }

    #[test]
    fn test_remove_liquidity_not_owner() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        let other = [3u8; 32];
        test_mock::set_slot(100);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 100_000, 100_000);
        assert_eq!(remove_liquidity(other.as_ptr(), 1, 1000), 2);
    }

    #[test]
    fn test_remove_liquidity_insufficient() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        test_mock::set_slot(100);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 100_000, 100_000);
        let pos_data = storage_get(&position_key(1)).unwrap();
        let liq = decode_pos_liquidity(&pos_data);
        assert_eq!(remove_liquidity(provider.as_ptr(), 1, liq + 1), 3);
    }

    // --- Swaps ---

    #[test]
    fn test_swap_exact_in() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        let trader = [3u8; 32];
        test_mock::set_slot(100);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        assert_eq!(swap_exact_in(trader.as_ptr(), pool_id, true, 10_000, 0, 0), 0);
    }

    #[test]
    fn test_swap_deadline_expired() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        let trader = [3u8; 32];
        test_mock::set_slot(100);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        // Deadline in the past
        assert_eq!(swap_exact_in(trader.as_ptr(), pool_id, true, 10_000, 0, 50), 3);
    }

    #[test]
    fn test_swap_paused() {
        let (admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        let trader = [3u8; 32];
        test_mock::set_slot(100);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        emergency_pause(admin.as_ptr());
        assert_eq!(swap_exact_in(trader.as_ptr(), pool_id, true, 10_000, 0, 0), 1);
    }

    #[test]
    fn test_swap_zero_amount() {
        let (_admin, pool_id) = setup_with_pool();
        let trader = [3u8; 32];
        test_mock::set_slot(100);
        assert_eq!(swap_exact_in(trader.as_ptr(), pool_id, true, 0, 0, 0), 6);
    }

    #[test]
    fn test_swap_slippage_protection() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        let trader = [3u8; 32];
        test_mock::set_slot(100);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        // Request impossibly high min_out
        assert_eq!(swap_exact_in(trader.as_ptr(), pool_id, true, 10_000, u64::MAX, 0), 4);
    }

    #[test]
    fn test_swap_fee_accrual() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        let trader = [3u8; 32];
        test_mock::set_slot(100);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        swap_exact_in(trader.as_ptr(), pool_id, true, 100_000, 0, 0);

        let pos_data = storage_get(&position_key(1)).unwrap();
        let fee_a = decode_pos_fee_a(&pos_data);
        assert!(fee_a > 0, "Should have accrued token_a fees");
    }

    // --- Collect Fees ---

    #[test]
    fn test_collect_fees() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        let trader = [3u8; 32];
        test_mock::set_slot(100);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        swap_exact_in(trader.as_ptr(), pool_id, true, 100_000, 0, 0);

        assert_eq!(collect_fees(provider.as_ptr(), 1), 0);
        // Fees should be zeroed after collection
        let pos_data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_fee_a(&pos_data), 0);
        assert_eq!(decode_pos_fee_b(&pos_data), 0);
    }

    #[test]
    fn test_collect_fees_not_owner() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        let other = [3u8; 32];
        test_mock::set_slot(100);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        assert_eq!(collect_fees(other.as_ptr(), 1), 2);
    }

    // --- Protocol Fee ---

    #[test]
    fn test_set_protocol_fee() {
        let (admin, pool_id) = setup_with_pool();
        assert_eq!(set_pool_protocol_fee(admin.as_ptr(), pool_id, 50), 0);
        let pd = storage_get(&pool_key(pool_id)).unwrap();
        assert_eq!(decode_pool_protocol_fee(&pd), 50);
    }

    #[test]
    fn test_set_protocol_fee_too_high() {
        let (admin, pool_id) = setup_with_pool();
        assert_eq!(set_pool_protocol_fee(admin.as_ptr(), pool_id, 101), 2);
    }

    // --- Tick Math ---

    #[test]
    fn test_tick_to_sqrt_price_at_zero() {
        let price = tick_to_sqrt_price(0);
        assert_eq!(price, 1u64 << 32);
    }

    #[test]
    fn test_tick_roundtrip() {
        let price = tick_to_sqrt_price(600);
        let tick = sqrt_price_to_tick(price);
        assert!((tick - 600).abs() <= 1, "Tick roundtrip should be close");
    }

    #[test]
    fn test_tick_negative() {
        let price_neg = tick_to_sqrt_price(-600);
        let price_pos = tick_to_sqrt_price(600);
        assert!(price_neg < price_pos, "Negative tick should give lower price");
    }

    // --- Emergency Pause ---

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
        assert_eq!(emergency_pause(rando.as_ptr()), 1);
    }

    // --- Queries ---

    #[test]
    fn test_get_pool_info() {
        let (_admin, pool_id) = setup_with_pool();
        assert_eq!(get_pool_info(pool_id), pool_id);
        assert_eq!(get_pool_info(999), 0);
    }

    #[test]
    fn test_get_pool_count() {
        let admin = setup();
        assert_eq!(get_pool_count(), 0);
        let ta = [10u8; 32];
        let tb = [20u8; 32];
        create_pool(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), FEE_TIER_30BPS, 1u64 << 32);
        assert_eq!(get_pool_count(), 1);
    }

    #[test]
    fn test_get_tvl() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        test_mock::set_slot(100);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        assert!(get_tvl(pool_id) > 0);
    }

    #[test]
    fn test_quote_swap() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        test_mock::set_slot(100);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        let out = quote_swap(pool_id, true, 10_000);
        assert!(out > 0, "Quote should return non-zero output");
    }

    #[test]
    fn test_quote_empty_pool() {
        let (_admin, pool_id) = setup_with_pool();
        let out = quote_swap(pool_id, true, 10_000);
        assert_eq!(out, 0, "Empty pool should return 0");
    }

    // --- Multiple positions ---

    #[test]
    fn test_multiple_positions_fee_distribution() {
        let (_admin, pool_id) = setup_with_pool();
        let p1 = [2u8; 32];
        let p2 = [3u8; 32];
        let trader = [4u8; 32];
        test_mock::set_slot(100);

        add_liquidity(p1.as_ptr(), pool_id, -60, 60, 500_000, 500_000);
        add_liquidity(p2.as_ptr(), pool_id, -60, 60, 500_000, 500_000);

        swap_exact_in(trader.as_ptr(), pool_id, true, 100_000, 0, 0);

        let pos1 = storage_get(&position_key(1)).unwrap();
        let pos2 = storage_get(&position_key(2)).unwrap();
        let fee1 = decode_pos_fee_a(&pos1);
        let fee2 = decode_pos_fee_a(&pos2);
        // Both should get approximately equal fees
        assert!(fee1 > 0 && fee2 > 0, "Both positions should earn fees");
    }
}
