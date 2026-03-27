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
#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use lichen_sdk::{
    bytes_to_u64, call_contract, call_token_transfer, get_caller, get_contract_address, get_slot,
    get_value, is_native_token, log_info, storage_get, storage_set, transfer_native, u64_to_bytes,
    Address, CrossCall,
};

// ============================================================================
// CONSTANTS
// ============================================================================

const MAX_POOLS: u64 = 100;
const MIN_LIQUIDITY: u64 = 1_000;
// AUDIT-FIX G3-01: MAX_TICK/MIN_TICK adjusted for u64 Q32.32 range.
// With sqrt_price stored as u64 Q32.32, the representable ratio range is
// [1/2^32, 2^32), corresponding to tick ≈ ±443,636. The original ±887,272
// (from Uniswap V3's uint160 Q64.96) would overflow u64.
const MAX_TICK: i32 = 443_636;
const MIN_TICK: i32 = -443_636;

// Fee tiers (in basis points)
const FEE_TIER_1BPS: u8 = 0;
const FEE_TIER_5BPS: u8 = 1;
const FEE_TIER_30BPS: u8 = 2;
const FEE_TIER_100BPS: u8 = 3;

const FEE_VALUES: [u64; 4] = [1, 5, 30, 100];
const TICK_SPACINGS: [i32; 4] = [1, 10, 60, 200];

// Q64.64 fixed-point scale
const Q64: u128 = 1u128 << 64;

// AUDIT-FIX AMM-5: Maximum tick boundaries a single swap can cross
const MAX_TICK_CROSSES: u32 = 100;
// AUDIT-FIX AMM-7: Maximum protocol fee percentage (protects LP incentives)
const MAX_PROTOCOL_FEE_PCT: u8 = 50;

// Storage keys
const ADMIN_KEY: &[u8] = b"amm_admin";
const PAUSED_KEY: &[u8] = b"amm_paused";
const REENTRANCY_KEY: &[u8] = b"amm_reentrancy";
const POOL_COUNT_KEY: &[u8] = b"amm_pool_count";
const POSITION_COUNT_KEY: &[u8] = b"amm_pos_count";
const PROTOCOL_FEE_KEY: &[u8] = b"amm_protocol_fee";
const PROTOCOL_FEE_ACCRUED_A_KEY: &[u8] = b"amm_proto_fee_a";
const PROTOCOL_FEE_ACCRUED_B_KEY: &[u8] = b"amm_proto_fee_b";
const FEE_TREASURY_ADDR_KEY: &[u8] = b"amm_fee_treasury_addr";
const SWAP_COUNT_KEY: &[u8] = b"amm_swap_count";
const TOTAL_VOLUME_KEY: &[u8] = b"amm_total_volume";
const TOTAL_FEES_KEY: &[u8] = b"amm_total_fees";
const POOL_PAIR_INDEX_PREFIX: &[u8] = b"amm_pair_idx_";

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

fn i32_to_bytes(n: i32) -> [u8; 4] {
    n.to_le_bytes()
}
fn bytes_to_i32(bytes: &[u8]) -> i32 {
    let mut arr = [0u8; 4];
    if bytes.len() >= 4 {
        arr.copy_from_slice(&bytes[..4]);
    }
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

fn pool_pair_index_key(token_a: &[u8; 32], token_b: &[u8; 32]) -> Vec<u8> {
    let (left, right) = if token_a <= token_b {
        (token_a, token_b)
    } else {
        (token_b, token_a)
    };
    let mut k = Vec::from(POOL_PAIR_INDEX_PREFIX);
    k.extend_from_slice(left);
    k.extend_from_slice(right);
    k
}

// AUDIT-FIX AMM-5: Initialized ticks list per pool for cross-tick swaps
fn init_ticks_key(pool_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"amm_iticks_"[..]);
    k.extend_from_slice(&u64_to_decimal(pool_id));
    k
}

fn load_initialized_ticks(pool_id: u64) -> Vec<i32> {
    match storage_get(&init_ticks_key(pool_id)) {
        Some(d) => {
            let mut ticks = Vec::new();
            for chunk in d.chunks_exact(4) {
                ticks.push(i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
            }
            ticks
        }
        None => Vec::new(),
    }
}

fn save_initialized_ticks(pool_id: u64, ticks: &[i32]) {
    let mut data = Vec::with_capacity(ticks.len() * 4);
    for &t in ticks {
        data.extend_from_slice(&t.to_le_bytes());
    }
    storage_set(&init_ticks_key(pool_id), &data);
}

fn ensure_tick_initialized(pool_id: u64, tick: i32) {
    let mut ticks = load_initialized_ticks(pool_id);
    match ticks.binary_search(&tick) {
        Ok(_) => {} // already present
        Err(pos) => {
            ticks.insert(pos, tick);
            save_initialized_ticks(pool_id, &ticks);
        }
    }
}

// AUDIT-FIX AMM-5: Signed liquidityNet for cross-tick mechanics
fn load_tick_net(key: &[u8]) -> i64 {
    storage_get(key)
        .map(|d| {
            if d.len() >= 8 {
                i64::from_le_bytes([d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]])
            } else {
                0
            }
        })
        .unwrap_or(0)
}

fn save_tick_net(key: &[u8], val: i64) {
    storage_set(key, &val.to_le_bytes());
}

// AUDIT-FIX AMM-1/2: Compute actual token amounts from liquidity and price range
fn compute_amounts_from_liquidity(
    liquidity: u64,
    sqrt_lower: u64,
    sqrt_upper: u64,
    sqrt_current: u64,
) -> (u64, u64) {
    if liquidity == 0 || sqrt_lower >= sqrt_upper {
        return (0, 0);
    }
    let amount_a = if sqrt_current >= sqrt_upper {
        0
    } else {
        let eff = if sqrt_current > sqrt_lower {
            sqrt_current
        } else {
            sqrt_lower
        };
        let delta = sqrt_upper as u128 - eff as u128;
        let denom = eff as u128 * sqrt_upper as u128 / (1u128 << 32);
        if denom == 0 {
            0
        } else {
            (liquidity as u128 * delta / denom) as u64
        }
    };
    let amount_b = if sqrt_current <= sqrt_lower {
        0
    } else {
        let eff = if sqrt_current < sqrt_upper {
            sqrt_current
        } else {
            sqrt_upper
        };
        let delta = eff as u128 - sqrt_lower as u128;
        (liquidity as u128 * delta / (1u128 << 32)) as u64
    };
    (amount_a, amount_b)
}

// AUDIT-FIX AMM-1/3: Pull tokens from user into the AMM contract.
// For native LICN: checks get_value() >= amount.
// For tokens: cross-contract call to transfer_from (requires prior approval).
fn pull_tokens(token: &[u8; 32], from: &[u8; 32], amount: u64) -> bool {
    if amount == 0 {
        return true;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (token, from, amount);
        return true;
    }
    #[cfg(target_arch = "wasm32")]
    {
        if is_native_token(&Address(*token)) {
            let received = get_value();
            return received >= amount;
        }
        let contract = get_contract_address();
        let mut args = Vec::with_capacity(104);
        args.extend_from_slice(&contract.0);
        args.extend_from_slice(from);
        args.extend_from_slice(&contract.0);
        args.extend_from_slice(&u64_to_bytes(amount));
        let call = CrossCall::new(Address(*token), "transfer_from", args).with_value(0);
        match call_contract(call) {
            Ok(_) => true,
            Err(_) => false,
        }
    }
}

// AUDIT-FIX AMM-2/3: Send tokens from the AMM contract to a user.
fn send_tokens(token: &[u8; 32], to: &[u8; 32], amount: u64) -> bool {
    if amount == 0 {
        return true;
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (token, to, amount);
        return true;
    }
    #[cfg(target_arch = "wasm32")]
    {
        if is_native_token(&Address(*token)) {
            return transfer_native(Address(*to), amount).unwrap_or(false);
        }
        let contract = get_contract_address();
        call_token_transfer(Address(*token), contract, Address(*to), amount).unwrap_or(false)
    }
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
    token_a: &[u8; 32],
    token_b: &[u8; 32],
    pool_id: u64,
    sqrt_price: u64,
    tick: i32,
    liquidity: u64,
    fee_tier: u8,
    protocol_fee: u8,
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
    if data.len() >= 72 {
        bytes_to_u64(&data[64..72])
    } else {
        0
    }
}
fn decode_pool_sqrt_price(data: &[u8]) -> u64 {
    if data.len() >= 80 {
        bytes_to_u64(&data[72..80])
    } else {
        0
    }
}
fn decode_pool_tick(data: &[u8]) -> i32 {
    if data.len() >= 84 {
        bytes_to_i32(&data[80..84])
    } else {
        0
    }
}
fn decode_pool_liquidity(data: &[u8]) -> u64 {
    if data.len() >= 92 {
        bytes_to_u64(&data[84..92])
    } else {
        0
    }
}
fn decode_pool_fee_tier(data: &[u8]) -> u8 {
    if data.len() > 92 {
        data[92]
    } else {
        0
    }
}
fn decode_pool_protocol_fee(data: &[u8]) -> u8 {
    if data.len() > 93 {
        data[93]
    } else {
        0
    }
}
fn decode_pool_token_a(data: &[u8]) -> [u8; 32] {
    let mut t = [0u8; 32];
    if data.len() >= 32 {
        t.copy_from_slice(&data[..32]);
    }
    t
}
fn decode_pool_token_b(data: &[u8]) -> [u8; 32] {
    let mut t = [0u8; 32];
    if data.len() >= 64 {
        t.copy_from_slice(&data[32..64]);
    }
    t
}

fn update_pool_sqrt_price(data: &mut Vec<u8>, new_sqrt: u64) {
    if data.len() >= 80 {
        data[72..80].copy_from_slice(&u64_to_bytes(new_sqrt));
    }
}
fn update_pool_tick(data: &mut Vec<u8>, new_tick: i32) {
    if data.len() >= 84 {
        data[80..84].copy_from_slice(&i32_to_bytes(new_tick));
    }
}
fn update_pool_liquidity(data: &mut Vec<u8>, new_liq: u64) {
    if data.len() >= 92 {
        data[84..92].copy_from_slice(&u64_to_bytes(new_liq));
    }
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
    owner: &[u8; 32],
    pool_id: u64,
    lower_tick: i32,
    upper_tick: i32,
    liquidity: u64,
    fee_a_owed: u64,
    fee_b_owed: u64,
    created_slot: u64,
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
    if data.len() >= 32 {
        o.copy_from_slice(&data[..32]);
    }
    o
}
fn decode_pos_pool_id(data: &[u8]) -> u64 {
    if data.len() >= 40 {
        bytes_to_u64(&data[32..40])
    } else {
        0
    }
}
fn decode_pos_lower_tick(data: &[u8]) -> i32 {
    if data.len() >= 44 {
        bytes_to_i32(&data[40..44])
    } else {
        0
    }
}
fn decode_pos_upper_tick(data: &[u8]) -> i32 {
    if data.len() >= 48 {
        bytes_to_i32(&data[44..48])
    } else {
        0
    }
}
fn decode_pos_liquidity(data: &[u8]) -> u64 {
    if data.len() >= 56 {
        bytes_to_u64(&data[48..56])
    } else {
        0
    }
}
fn decode_pos_fee_a(data: &[u8]) -> u64 {
    if data.len() >= 64 {
        bytes_to_u64(&data[56..64])
    } else {
        0
    }
}
fn decode_pos_fee_b(data: &[u8]) -> u64 {
    if data.len() >= 72 {
        bytes_to_u64(&data[64..72])
    } else {
        0
    }
}
fn decode_pos_created_slot(data: &[u8]) -> u64 {
    if data.len() >= 80 {
        bytes_to_u64(&data[72..80])
    } else {
        0
    }
}

fn update_pos_liquidity(data: &mut Vec<u8>, liq: u64) {
    if data.len() >= 56 {
        data[48..56].copy_from_slice(&u64_to_bytes(liq));
    }
}
fn update_pos_fee_a(data: &mut Vec<u8>, fee: u64) {
    if data.len() >= 64 {
        data[56..64].copy_from_slice(&u64_to_bytes(fee));
    }
}
fn update_pos_fee_b(data: &mut Vec<u8>, fee: u64) {
    if data.len() >= 72 {
        data[64..72].copy_from_slice(&u64_to_bytes(fee));
    }
}

// ============================================================================
// TICK MATH (Q32.32 fixed-point for no_std)
// AUDIT-FIX G3-01: Replaced linear approximation with correct exponential
// formula: sqrt_price = 1.0001^(tick/2) = 1.00005^tick
// Uses bit-decomposition of |tick| with precomputed Q64.64 constants for
// 1.00005^(2^k), then narrows to Q32.32 for storage. Matches Uniswap V3
// TickMath.sol methodology adapted for integer-only no_std WASM.
// ============================================================================

/// Precomputed Q64.64 constants for 1.00005^(2^k).
/// Each entry = floor(1.00005^(2^k) * 2^64).
/// Generated with 80-decimal-digit precision arithmetic.
/// We need bits 0..18 to cover |tick| up to 443,636.
const TICK_RATIOS: [u128; 19] = [
    18447666410913237093,      // k=0:  1.00005^1
    18448588794233782755,      // k=1:  1.00005^2
    18450433699234678119,      // k=2:  1.00005^4
    18454124062740255875,      // k=3:  1.00005^8
    18461507004283223312,      // k=4:  1.00005^16
    18476281749631266690,      // k=5:  1.00005^32
    18505866722479494652,      // k=6:  1.00005^64
    18565178862011796984,      // k=7:  1.00005^128
    18684374044615830753,      // k=8:  1.00005^256
    18925065152102488741,      // k=9:  1.00005^512
    19415789018386924678,      // k=10: 1.00005^1024
    20435739862829184269,      // k=11: 1.00005^2048
    22639196493023416200,      // k=12: 1.00005^4096
    27784481413182840296,      // k=13: 1.00005^8192
    41848979110613408870,      // k=14: 1.00005^16384
    94940171859194227663,      // k=15: 1.00005^32768
    488630199271840203185,     // k=16: 1.00005^65536
    12943176892702717671113,   // k=17: 1.00005^131072
    9081593337360425506718466, // k=18: 1.00005^262144
];

/// Multiply two Q64.64 numbers and return Q64.64 result.
/// Computes floor((a * b) / 2^64) without overflow.
/// Both a and b are u128 representing Q64.64 fixed-point numbers.
fn mul_q64(a: u128, b: u128) -> u128 {
    // Split a and b into high (integer) and low (fractional) 64-bit parts.
    let a_hi = a >> 64;
    let a_lo = a & 0xFFFFFFFFFFFFFFFF_u128;
    let b_hi = b >> 64;
    let b_lo = b & 0xFFFFFFFFFFFFFFFF_u128;

    // 256-bit product P = a*b, decomposed:
    //   P = (a_hi*b_hi)<<128 + (a_hi*b_lo + a_lo*b_hi)<<64 + a_lo*b_lo
    //
    // We want floor(P / 2^64) = floor(P >> 64).
    // P >> 64 = (a_hi*b_hi)<<64 + (a_hi*b_lo + a_lo*b_hi) + (a_lo*b_lo)>>64
    //
    // Each partial product fits in u128 since all halves are at most 64 bits.
    // The sum can exceed u128 so we track carries.

    let ll = a_lo * b_lo; // u128, holds full product of two u64 values
    let hl = a_hi * b_lo; // u128
    let lh = a_lo * b_hi; // u128
    let hh = a_hi * b_hi; // u128

    // Start with the fractional contribution: ll >> 64
    let ll_hi = ll >> 64;

    // Sum the middle terms and ll_hi
    let (sum1, c1) = hl.overflowing_add(lh);
    let (sum2, c2) = sum1.overflowing_add(ll_hi);

    // Carry into the high part: each overflow = 2^128, which shifts to 2^64 after >>64
    let carry: u128 = (c1 as u128) + (c2 as u128);

    // Result = hh<<64 + carry<<64 + sum2
    // Use wrapping arithmetic to avoid debug-mode overflow panics.
    // For our bounded inputs (price ratios ~1.0 to ~2^32) the result always fits u128.
    (hh.wrapping_add(carry)).wrapping_shl(64).wrapping_add(sum2)
}

/// Compute sqrt_price (Q32.32) for a given tick using exponential formula.
/// sqrt_price = floor(2^32 * 1.00005^tick)
/// Returns: Q32.32 fixed-point sqrt price as u64
fn tick_to_sqrt_price(tick: i32) -> u64 {
    let abs_tick = if tick < 0 {
        (-tick) as u32
    } else {
        tick as u32
    };

    // Accumulator in Q64.64
    let mut acc: u128 = 1u128 << 64;

    // Multiply by precomputed 1.00005^(2^k) for each set bit
    for k in 0..19u32 {
        if abs_tick & (1u32 << k) != 0 {
            acc = mul_q64(acc, TICK_RATIOS[k as usize]);
        }
    }

    // For negative ticks: take reciprocal = Q64^2 / acc = (2^128) / acc
    if tick < 0 {
        // acc represents 1.00005^|tick| in Q64.64
        // reciprocal in Q64.64 = 2^128 / acc
        if acc == 0 {
            return 1;
        }
        // Use u128::MAX / acc as approximation (loses 1 ULP at most)
        acc = (u128::MAX / acc) + 1; // ceil division approximation
    }

    // Convert Q64.64 → Q32.32: right-shift by 32
    let result = acc >> 32;

    // Clamp to u64
    if result == 0 {
        1
    } else if result > u64::MAX as u128 {
        u64::MAX
    } else {
        result as u64
    }
}

/// Get tick from sqrt_price (inverse of tick_to_sqrt_price).
/// Uses binary search over the tick range for exact inversion.
fn sqrt_price_to_tick(sqrt_price: u64) -> i32 {
    if sqrt_price == (1u64 << 32) {
        return 0;
    }

    // Binary search: find the largest tick where tick_to_sqrt_price(tick) <= sqrt_price
    let mut lo: i32 = MIN_TICK;
    let mut hi: i32 = MAX_TICK;

    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        let price_at_mid = tick_to_sqrt_price(mid);
        if price_at_mid <= sqrt_price {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    lo
}

/// Calculate liquidity from amounts and price range
fn compute_liquidity(
    amount_a: u64,
    amount_b: u64,
    sqrt_lower: u64,
    sqrt_upper: u64,
    sqrt_current: u64,
) -> u64 {
    if sqrt_lower >= sqrt_upper || amount_a == 0 && amount_b == 0 {
        return 0;
    }

    let liq_a = if sqrt_current >= sqrt_upper {
        0u128
    } else {
        let sqrt_l = if sqrt_current > sqrt_lower {
            sqrt_current
        } else {
            sqrt_lower
        };
        let delta_sqrt = sqrt_upper as u128 - sqrt_l as u128;
        if delta_sqrt == 0 {
            0
        } else {
            // Use checked_mul to prevent u128 overflow with extreme inputs
            match (amount_a as u128)
                .checked_mul(sqrt_l as u128)
                .and_then(|v| v.checked_mul(sqrt_upper as u128))
            {
                Some(num) => num / (delta_sqrt * (1u128 << 32)),
                None => u64::MAX as u128, // clamp on overflow
            }
        }
    };

    let liq_b = if sqrt_current <= sqrt_lower {
        0u128
    } else {
        let sqrt_u = if sqrt_current < sqrt_upper {
            sqrt_current
        } else {
            sqrt_upper
        };
        let delta_sqrt = sqrt_u as u128 - sqrt_lower as u128;
        if delta_sqrt == 0 {
            0
        } else {
            (amount_b as u128 * (1u128 << 32)) / delta_sqrt
        }
    };

    // Use minimum of the two
    let liq = if liq_a == 0 {
        liq_b
    } else if liq_b == 0 {
        liq_a
    } else {
        if liq_a < liq_b {
            liq_a
        } else {
            liq_b
        }
    };
    liq as u64
}

/// AUDIT-FIX AMM-5/6: Raw swap math without fee deduction (for cross-tick loop).
/// Fee is deducted once at the swap entry point.
fn compute_swap_output_raw(
    amount_in: u64,
    liquidity: u64,
    sqrt_price: u64,
    is_token_a: bool,
) -> (u64, u64) {
    if liquidity == 0 || amount_in == 0 {
        return (0, sqrt_price);
    }

    if is_token_a {
        // Swapping A for B: price decreases
        let numerator = liquidity as u128 * sqrt_price as u128;
        let denominator =
            liquidity as u128 + (amount_in as u128 * sqrt_price as u128 / (1u128 << 32));
        if denominator == 0 {
            return (0, sqrt_price);
        }
        let new_sqrt = (numerator / denominator) as u64;
        let delta_sqrt = sqrt_price as u128 - new_sqrt as u128;
        let amount_out = (liquidity as u128 * delta_sqrt / (1u128 << 32)) as u64;
        (amount_out, new_sqrt)
    } else {
        // Swapping B for A: price increases
        let delta = amount_in as u128 * (1u128 << 32) / liquidity as u128;
        let new_sqrt = (sqrt_price as u128 + delta) as u64;
        let delta_sqrt = new_sqrt as u128 - sqrt_price as u128;
        let denom = sqrt_price as u128 * new_sqrt as u128 / (1u128 << 32);
        let amount_out = if denom == 0 {
            0
        } else {
            (liquidity as u128 * delta_sqrt / denom) as u64
        };
        (amount_out, new_sqrt)
    }
}

/// Backward-compatible wrapper: deducts fee then calls raw computation.
fn compute_swap_output(
    amount_in: u64,
    liquidity: u64,
    sqrt_price: u64,
    fee_bps: u64,
    is_token_a: bool,
) -> (u64, u64) {
    if liquidity == 0 || amount_in == 0 {
        return (0, sqrt_price);
    }
    let fee = (amount_in as u128 * fee_bps as u128 / 10_000) as u64;
    let amount_after_fee = amount_in - fee;
    compute_swap_output_raw(amount_after_fee, liquidity, sqrt_price, is_token_a)
}

/// AUDIT-FIX AMM-5: Compute input amount needed to move price to target.
fn compute_input_to_target(
    liquidity: u64,
    sqrt_price: u64,
    target_sqrt: u64,
    is_token_a_in: bool,
) -> u64 {
    if liquidity == 0 {
        return u64::MAX;
    }
    if is_token_a_in {
        // Price going down
        if sqrt_price <= target_sqrt {
            return 0;
        }
        let delta = sqrt_price as u128 - target_sqrt as u128;
        let denom = (sqrt_price as u128)
            .checked_mul(target_sqrt as u128)
            .map(|v| v / (1u128 << 32))
            .unwrap_or(0);
        if denom == 0 {
            return u64::MAX;
        }
        (liquidity as u128 * delta / denom).saturating_add(1) as u64
    } else {
        // Price going up
        if target_sqrt <= sqrt_price {
            return 0;
        }
        let delta = target_sqrt as u128 - sqrt_price as u128;
        (liquidity as u128 * delta / (1u128 << 32)).saturating_add(1) as u64
    }
}

/// AUDIT-FIX AMM-5: Execute swap with cross-tick liquidity transitions.
/// Returns (total_amount_out, new_sqrt_price, new_tick).
fn compute_swap_with_ticks(
    pool_id: u64,
    amount_in_after_fee: u64,
    liquidity: u64,
    sqrt_price: u64,
    current_tick: i32,
    is_token_a_in: bool,
) -> (u64, u64, i32) {
    let mut remaining = amount_in_after_fee;
    let mut total_out: u64 = 0;
    let mut liq = liquidity;
    let mut sqp = sqrt_price;
    let mut ct = current_tick;

    let init_ticks = load_initialized_ticks(pool_id);

    for _ in 0..MAX_TICK_CROSSES {
        if remaining == 0 || liq == 0 {
            break;
        }

        let going_up = !is_token_a_in;
        let next_tick = if going_up {
            init_ticks.iter().find(|&&t| t > ct).copied()
        } else {
            init_ticks.iter().rev().find(|&&t| t <= ct).copied()
        };

        match next_tick {
            None => {
                // No more initialized ticks — use remaining with current liquidity
                let (out, new_sqp) = compute_swap_output_raw(remaining, liq, sqp, is_token_a_in);
                total_out = total_out.saturating_add(out);
                sqp = new_sqp;
                ct = sqrt_price_to_tick(new_sqp);
                remaining = 0;
            }
            Some(tick_bound) => {
                let target_sqp = tick_to_sqrt_price(tick_bound);
                let input_needed = compute_input_to_target(liq, sqp, target_sqp, is_token_a_in);

                if remaining < input_needed {
                    // Won't reach the next tick boundary
                    let (out, new_sqp) =
                        compute_swap_output_raw(remaining, liq, sqp, is_token_a_in);
                    total_out = total_out.saturating_add(out);
                    sqp = new_sqp;
                    ct = sqrt_price_to_tick(new_sqp);
                    remaining = 0;
                } else {
                    // Reach the tick boundary
                    let (out, _) =
                        compute_swap_output_raw(input_needed, liq, sqp, is_token_a_in);
                    total_out = total_out.saturating_add(out);
                    remaining = remaining.saturating_sub(input_needed);
                    sqp = target_sqp;

                    // Cross the tick: apply liquidityNet
                    let net = load_tick_net(&tick_data_key(pool_id, tick_bound));
                    if going_up {
                        liq = ((liq as i64).saturating_add(net)) as u64;
                        ct = tick_bound;
                    } else {
                        liq = ((liq as i64).saturating_sub(net)) as u64;
                        ct = tick_bound - 1;
                    }
                }
            }
        }
    }

    (total_out, sqp, ct)
}

// ============================================================================
// PUBLIC FUNCTIONS
// ============================================================================

/// Initialize the AMM contract
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
    save_u64(POOL_COUNT_KEY, 0);
    save_u64(POSITION_COUNT_KEY, 0);
    storage_set(PAUSED_KEY, &[0u8]);
    log_info("DEX AMM initialized");
    0
}

/// Create a new liquidity pool
/// Returns: 0=success, 1=not admin, 2=paused, 3=max pools, 4=invalid params, 5=reentrancy, 6=duplicate pair
pub fn create_pool(
    caller: *const u8,
    token_a: *const u8,
    token_b: *const u8,
    fee_tier: u8,
    initial_sqrt_price: u64,
) -> u32 {
    if !reentrancy_enter() {
        return 5;
    }
    let mut c = [0u8; 32];
    let mut ta = [0u8; 32];
    let mut tb = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(token_a, ta.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(token_b, tb.as_mut_ptr(), 32);
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

    let count = load_u64(POOL_COUNT_KEY);
    if count >= MAX_POOLS {
        reentrancy_exit();
        return 3;
    }
    if fee_tier > FEE_TIER_100BPS {
        reentrancy_exit();
        return 4;
    }
    if initial_sqrt_price == 0 {
        reentrancy_exit();
        return 4;
    }
    if ta == tb {
        reentrancy_exit();
        return 4;
    }

    let pair_key = pool_pair_index_key(&ta, &tb);
    if storage_get(&pair_key).is_some() {
        reentrancy_exit();
        return 6;
    }

    let pool_id = count + 1;
    let tick = sqrt_price_to_tick(initial_sqrt_price);
    let data = encode_pool(&ta, &tb, pool_id, initial_sqrt_price, tick, 0, fee_tier, 0);
    storage_set(&pool_key(pool_id), &data);
    storage_set(&pair_key, &u64_to_bytes(pool_id));
    save_u64(POOL_COUNT_KEY, pool_id);
    log_info("AMM pool created");
    reentrancy_exit();
    0
}

/// Set protocol fee for a pool (admin only)
pub fn set_pool_protocol_fee(caller: *const u8, pool_id: u64, fee_percent: u8) -> u32 {
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
    // AUDIT-FIX AMM-7: Cap protocol fee at 50% to protect LP incentives
    if fee_percent > MAX_PROTOCOL_FEE_PCT {
        return 2;
    }
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
    provider: *const u8,
    pool_id: u64,
    lower_tick: i32,
    upper_tick: i32,
    amount_a: u64,
    amount_b: u64,
) -> u32 {
    if !reentrancy_enter() {
        return 5;
    }
    if !require_not_paused() {
        reentrancy_exit();
        return 1;
    }

    let mut p = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(provider, p.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != p {
        reentrancy_exit();
        return 200;
    }

    let pk = pool_key(pool_id);
    let mut pool_data = match storage_get(&pk) {
        Some(d) if d.len() >= POOL_SIZE => d,
        _ => {
            reentrancy_exit();
            return 2;
        }
    };

    // Validate tick range
    let fee_tier = decode_pool_fee_tier(&pool_data);
    let tick_spacing = TICK_SPACINGS[fee_tier as usize];
    if lower_tick >= upper_tick {
        reentrancy_exit();
        return 3;
    }
    if lower_tick < MIN_TICK || upper_tick > MAX_TICK {
        reentrancy_exit();
        return 3;
    }
    if lower_tick % tick_spacing != 0 || upper_tick % tick_spacing != 0 {
        reentrancy_exit();
        return 3;
    }

    // Enforce minimum deposit amounts to prevent dust positions
    const MIN_AMOUNT: u64 = 100;
    if amount_a < MIN_AMOUNT && amount_b < MIN_AMOUNT {
        reentrancy_exit();
        return 4;
    }

    let sqrt_current = decode_pool_sqrt_price(&pool_data);
    let sqrt_lower = tick_to_sqrt_price(lower_tick);
    let sqrt_upper = tick_to_sqrt_price(upper_tick);

    let liquidity = compute_liquidity(amount_a, amount_b, sqrt_lower, sqrt_upper, sqrt_current);
    if liquidity < MIN_LIQUIDITY {
        reentrancy_exit();
        return 4;
    }

    // AUDIT-FIX AMM-1: Compute actual token amounts from the liquidity
    let (actual_a, actual_b) =
        compute_amounts_from_liquidity(liquidity, sqrt_lower, sqrt_upper, sqrt_current);

    // AUDIT-FIX AMM-1: Pull tokens from provider to the AMM contract
    let token_a = decode_pool_token_a(&pool_data);
    let token_b = decode_pool_token_b(&pool_data);
    if actual_a > 0 && !pull_tokens(&token_a, &p, actual_a) {
        reentrancy_exit();
        return 6; // token transfer failed
    }
    if actual_b > 0 && !pull_tokens(&token_b, &p, actual_b) {
        reentrancy_exit();
        return 6;
    }

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
        update_pool_liquidity(&mut pool_data, pool_liq.saturating_add(liquidity));
        storage_set(&pk, &pool_data);
    }

    // AUDIT-FIX AMM-5: Use signed liquidityNet for cross-tick mechanics
    // lower tick: +L (entering range going up), upper tick: -L (exiting range going up)
    let tk_lower = tick_data_key(pool_id, lower_tick);
    let net_lower = load_tick_net(&tk_lower);
    save_tick_net(&tk_lower, net_lower.saturating_add(liquidity as i64));
    ensure_tick_initialized(pool_id, lower_tick);

    let tk_upper = tick_data_key(pool_id, upper_tick);
    let net_upper = load_tick_net(&tk_upper);
    save_tick_net(&tk_upper, net_upper.saturating_sub(liquidity as i64));
    ensure_tick_initialized(pool_id, upper_tick);

    log_info("Liquidity added");
    reentrancy_exit();
    0
}

/// Remove liquidity from a position
/// Returns: 0=success, 1=not found, 2=not owner, 3=insufficient, 4=reentrancy
pub fn remove_liquidity(provider: *const u8, position_id: u64, liquidity_amount: u64) -> u32 {
    if !reentrancy_enter() {
        return 4;
    }
    let mut p = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(provider, p.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != p {
        reentrancy_exit();
        return 200;
    }

    let pk = position_key(position_id);
    let mut pos_data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE => d,
        _ => {
            reentrancy_exit();
            return 1;
        }
    };

    let owner = decode_pos_owner(&pos_data);
    if owner != p {
        reentrancy_exit();
        return 2;
    }

    let current_liq = decode_pos_liquidity(&pos_data);
    if liquidity_amount > current_liq {
        reentrancy_exit();
        return 3;
    }

    let new_liq = current_liq - liquidity_amount;
    update_pos_liquidity(&mut pos_data, new_liq);
    storage_set(&pk, &pos_data);

    // Update pool liquidity
    let pool_id = decode_pos_pool_id(&pos_data);
    let pool_pk = pool_key(pool_id);
    let lower = decode_pos_lower_tick(&pos_data);
    let upper = decode_pos_upper_tick(&pos_data);

    if let Some(mut pool_data) = storage_get(&pool_pk) {
        if pool_data.len() >= POOL_SIZE {
            let current_tick = decode_pool_tick(&pool_data);
            if current_tick >= lower && current_tick < upper {
                let pool_liq = decode_pool_liquidity(&pool_data);
                update_pool_liquidity(&mut pool_data, pool_liq.saturating_sub(liquidity_amount));
                storage_set(&pool_pk, &pool_data);
            }

            // AUDIT-FIX AMM-2: Compute and return withdrawn token amounts
            let sqrt_current = decode_pool_sqrt_price(&pool_data);
            let sqrt_lower = tick_to_sqrt_price(lower);
            let sqrt_upper = tick_to_sqrt_price(upper);
            let (return_a, return_b) =
                compute_amounts_from_liquidity(liquidity_amount, sqrt_lower, sqrt_upper, sqrt_current);

            let token_a = decode_pool_token_a(&pool_data);
            let token_b = decode_pool_token_b(&pool_data);
            if return_a > 0 {
                send_tokens(&token_a, &p, return_a);
            }
            if return_b > 0 {
                send_tokens(&token_b, &p, return_b);
            }
        }
    }

    // AUDIT-FIX AMM-5: Update signed tick liquidityNet
    let tk_lower = tick_data_key(pool_id, lower);
    save_tick_net(&tk_lower, load_tick_net(&tk_lower).saturating_sub(liquidity_amount as i64));
    let tk_upper = tick_data_key(pool_id, upper);
    save_tick_net(&tk_upper, load_tick_net(&tk_upper).saturating_add(liquidity_amount as i64));

    log_info("Liquidity removed");
    reentrancy_exit();
    0
}

/// Collect accumulated fees for a position
/// Returns: 0=success, 1=not found, 2=not owner
pub fn collect_fees(provider: *const u8, position_id: u64) -> u32 {
    let mut p = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(provider, p.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != p {
        return 200;
    }

    let pk = position_key(position_id);
    let mut pos_data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE => d,
        _ => return 1,
    };

    let owner = decode_pos_owner(&pos_data);
    if owner != p {
        return 2;
    }

    let fee_a = decode_pos_fee_a(&pos_data);
    let fee_b = decode_pos_fee_b(&pos_data);

    // P9-SC-02: Transfer fee tokens to LP before resetting counters
    let pool_id = decode_pos_pool_id(&pos_data);
    let pool_pk = pool_key(pool_id);
    let pool_data = match storage_get(&pool_pk) {
        Some(d) if d.len() >= POOL_SIZE => d,
        _ => return 3,
    };
    let token_a_addr = decode_pool_token_a(&pool_data);
    let token_b_addr = decode_pool_token_b(&pool_data);
    let contract_addr = get_contract_address();

    if fee_a > 0 {
        if is_native_token(&Address(token_a_addr)) {
            if transfer_native(Address(p), fee_a).is_err() {
                log_info("Fee A native transfer failed");
                return 4;
            }
        } else if call_token_transfer(Address(token_a_addr), contract_addr, Address(p), fee_a)
            .is_err()
        {
            log_info("Fee A transfer failed");
            return 4;
        }
    }
    if fee_b > 0 {
        if is_native_token(&Address(token_b_addr)) {
            if transfer_native(Address(p), fee_b).is_err() {
                log_info("Fee B native transfer failed");
                return 4;
            }
        } else if call_token_transfer(Address(token_b_addr), contract_addr, Address(p), fee_b)
            .is_err()
        {
            log_info("Fee B transfer failed");
            return 4;
        }
    }

    // Reset fees after successful transfer
    update_pos_fee_a(&mut pos_data, 0);
    update_pos_fee_b(&mut pos_data, 0);
    storage_set(&pk, &pos_data);

    // Return fee amounts via return data
    let mut ret = Vec::with_capacity(16);
    ret.extend_from_slice(&u64_to_bytes(fee_a));
    ret.extend_from_slice(&u64_to_bytes(fee_b));
    lichen_sdk::set_return_data(&ret);
    0
}

/// Swap exact input amount
/// Returns: 0=success, 1=paused, 2=pool not found, 3=deadline expired,
///          4=insufficient output, 5=reentrancy, 6=zero amount
pub fn swap_exact_in(
    trader: *const u8,
    pool_id: u64,
    is_token_a_in: bool,
    amount_in: u64,
    min_out: u64,
    deadline: u64,
) -> u32 {
    if !reentrancy_enter() {
        return 5;
    }
    if !require_not_paused() {
        reentrancy_exit();
        return 1;
    }
    if amount_in == 0 {
        reentrancy_exit();
        return 6;
    }

    let mut tr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(trader, tr.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != tr {
        reentrancy_exit();
        return 200;
    }

    let current_slot = get_slot();
    if deadline != 0 && current_slot > deadline {
        reentrancy_exit();
        return 3;
    }

    let pk = pool_key(pool_id);
    let mut pool_data = match storage_get(&pk) {
        Some(d) if d.len() >= POOL_SIZE => d,
        _ => {
            reentrancy_exit();
            return 2;
        }
    };

    let sqrt_price = decode_pool_sqrt_price(&pool_data);
    let liquidity = decode_pool_liquidity(&pool_data);
    let fee_tier = decode_pool_fee_tier(&pool_data);
    let fee_bps = FEE_VALUES[fee_tier as usize];
    let current_tick = decode_pool_tick(&pool_data);
    let token_a = decode_pool_token_a(&pool_data);
    let token_b = decode_pool_token_b(&pool_data);

    // AUDIT-FIX AMM-6: Fee deduction once at entry, then cross-tick computation
    let fee = (amount_in as u128 * fee_bps as u128 / 10_000) as u64;
    let amount_after_fee = amount_in - fee;

    // AUDIT-FIX AMM-5: Cross-tick swap with proper liquidity transitions
    let (amount_out, new_sqrt, new_tick) = compute_swap_with_ticks(
        pool_id,
        amount_after_fee,
        liquidity,
        sqrt_price,
        current_tick,
        is_token_a_in,
    );

    if amount_out < min_out {
        reentrancy_exit();
        return 4;
    }

    // AUDIT-FIX AMM-3: Pull input tokens from trader
    let input_token = if is_token_a_in { &token_a } else { &token_b };
    if !pull_tokens(input_token, &tr, amount_in) {
        reentrancy_exit();
        return 7; // input token transfer failed
    }

    // AUDIT-FIX AMM-3: Send output tokens to trader
    let output_token = if is_token_a_in { &token_b } else { &token_a };
    if !send_tokens(output_token, &tr, amount_out) {
        reentrancy_exit();
        return 8; // output token transfer failed
    }

    // Update pool state
    update_pool_sqrt_price(&mut pool_data, new_sqrt);
    update_pool_tick(&mut pool_data, new_tick);
    // Update pool active liquidity to reflect post-cross state
    let final_liq = {
        let mut liq = 0u64;
        // Sum liquidity of all positions whose range contains new_tick
        // (This equals the pool's active liquidity at the new tick)
        let pos_count = load_u64(POSITION_COUNT_KEY);
        for i in 1..=pos_count {
            if let Some(pd) = storage_get(&position_key(i)) {
                if pd.len() >= POSITION_SIZE && decode_pos_pool_id(&pd) == pool_id {
                    let lower = decode_pos_lower_tick(&pd);
                    let upper = decode_pos_upper_tick(&pd);
                    if new_tick >= lower && new_tick < upper {
                        liq = liq.saturating_add(decode_pos_liquidity(&pd));
                    }
                }
            }
        }
        liq
    };
    update_pool_liquidity(&mut pool_data, final_liq);
    storage_set(&pk, &pool_data);

    // Accrue fees to in-range positions
    accrue_fees_to_positions(pool_id, fee, is_token_a_in);

    // Track global swap count, volume, and fees
    save_u64(SWAP_COUNT_KEY, load_u64(SWAP_COUNT_KEY) + 1);
    save_u64(
        TOTAL_VOLUME_KEY,
        load_u64(TOTAL_VOLUME_KEY).saturating_add(amount_in),
    );
    save_u64(TOTAL_FEES_KEY, load_u64(TOTAL_FEES_KEY).saturating_add(fee));

    // Return amount out
    lichen_sdk::set_return_data(&u64_to_bytes(amount_out));
    log_info("Swap executed");
    reentrancy_exit();
    0
}

/// Swap to receive exact output amount
pub fn swap_exact_out(
    trader: *const u8,
    pool_id: u64,
    is_token_a_out: bool,
    amount_out: u64,
    max_in: u64,
    deadline: u64,
) -> u32 {
    if !reentrancy_enter() {
        return 5;
    }
    if !require_not_paused() {
        reentrancy_exit();
        return 1;
    }
    if amount_out == 0 {
        reentrancy_exit();
        return 6;
    }

    let mut tr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(trader, tr.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != tr {
        reentrancy_exit();
        return 200;
    }

    let current_slot = get_slot();
    if deadline != 0 && current_slot > deadline {
        reentrancy_exit();
        return 3;
    }

    // For exact out, we estimate input needed
    // Simplified: try increasing amounts until output >= target
    let pk = pool_key(pool_id);
    let pool_data = match storage_get(&pk) {
        Some(d) if d.len() >= POOL_SIZE => d,
        _ => {
            reentrancy_exit();
            return 2;
        }
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
        if lo > hi {
            break;
        }
        let mid = lo + (hi - lo) / 2;
        let (out, _) = compute_swap_output(mid, liquidity, sqrt_price, fee_bps, !is_token_a_out);
        if out >= amount_out {
            best_in = mid;
            hi = mid - 1;
        } else {
            lo = mid + 1;
        }
    }
    if best_in == 0 || best_in > max_in {
        reentrancy_exit();
        return 4;
    }

    reentrancy_exit();
    swap_exact_in(
        trader,
        pool_id,
        !is_token_a_out,
        best_in,
        amount_out,
        deadline,
    )
}

/// Accrue fees to in-range positions, splitting out protocol fee first.
/// Protocol fee is a percentage (0-100) of the total swap fee, stored per pool.
/// Matches Uniswap V3 protocol fee switch design.
fn accrue_fees_to_positions(pool_id: u64, fee: u64, is_token_a: bool) {
    if fee == 0 {
        return;
    }
    let pool_pk = pool_key(pool_id);
    let pool_data = match storage_get(&pool_pk) {
        Some(d) => d,
        None => return,
    };

    // Split: protocol_fee_pct% → protocol treasury, rest → LPs
    let proto_pct = decode_pool_protocol_fee(&pool_data) as u64;
    let protocol_share = if proto_pct > 0 {
        (fee as u128 * proto_pct as u128 / 100) as u64
    } else {
        0
    };
    let lp_fee = fee.saturating_sub(protocol_share);

    // Accumulate protocol share
    if protocol_share > 0 {
        let key = if is_token_a {
            PROTOCOL_FEE_ACCRUED_A_KEY
        } else {
            PROTOCOL_FEE_ACCRUED_B_KEY
        };
        let mut pk_key = Vec::from(key);
        pk_key.extend_from_slice(&u64_to_bytes(pool_id));
        save_u64(&pk_key, load_u64(&pk_key).saturating_add(protocol_share));
    }

    if lp_fee == 0 {
        return;
    }

    let current_tick = decode_pool_tick(&pool_data);
    let pool_liq = decode_pool_liquidity(&pool_data);
    if pool_liq == 0 {
        return;
    }

    // Distribute LP fee proportionally to all in-range positions
    let pos_count = load_u64(POSITION_COUNT_KEY);
    for i in 1..=pos_count {
        let pk = position_key(i);
        if let Some(mut pos_data) = storage_get(&pk) {
            if pos_data.len() >= POSITION_SIZE {
                let pos_pool = decode_pos_pool_id(&pos_data);
                if pos_pool != pool_id {
                    continue;
                }
                let lower = decode_pos_lower_tick(&pos_data);
                let upper = decode_pos_upper_tick(&pos_data);
                if current_tick >= lower && current_tick < upper {
                    let pos_liq = decode_pos_liquidity(&pos_data);
                    let share = (lp_fee as u128 * pos_liq as u128 / pool_liq as u128) as u64;
                    if is_token_a {
                        let current = decode_pos_fee_a(&pos_data);
                        update_pos_fee_a(&mut pos_data, current.saturating_add(share));
                    } else {
                        let current = decode_pos_fee_b(&pos_data);
                        update_pos_fee_b(&mut pos_data, current.saturating_add(share));
                    }
                    storage_set(&pk, &pos_data);
                }
            }
        }
    }
}

/// Set the fee treasury address (admin only).
/// Returns: 0=success, 1=not admin, 200=caller mismatch
pub fn set_fee_treasury_address(caller: *const u8, treasury: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut t = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(treasury, t.as_mut_ptr(), 32);
    }
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    // AUDIT-FIX: Reject zero address
    if is_zero(&t) {
        return 3;
    }
    storage_set(FEE_TREASURY_ADDR_KEY, &t);
    0
}

/// Collect accrued protocol fees for a pool (admin only).
/// Transfers accumulated protocol fees to the treasury address.
/// Returns: 0=success, 1=not admin, 2=no treasury set, 3=nothing to collect
pub fn collect_protocol_fees(caller: *const u8, pool_id: u64) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    let treasury = load_addr(FEE_TREASURY_ADDR_KEY);
    if is_zero(&treasury) {
        return 2;
    }
    let pk = pool_key(pool_id);
    let pool_data = match storage_get(&pk) {
        Some(d) if d.len() >= POOL_SIZE => d,
        _ => return 4,
    };
    let token_a = decode_pool_token_a(&pool_data);
    let token_b = decode_pool_token_b(&pool_data);
    let contract_addr = get_contract_address();

    let mut key_a = Vec::from(PROTOCOL_FEE_ACCRUED_A_KEY);
    key_a.extend_from_slice(&u64_to_bytes(pool_id));
    let mut key_b = Vec::from(PROTOCOL_FEE_ACCRUED_B_KEY);
    key_b.extend_from_slice(&u64_to_bytes(pool_id));

    let fee_a = load_u64(&key_a);
    let fee_b = load_u64(&key_b);
    if fee_a == 0 && fee_b == 0 {
        return 3;
    }
    if fee_a > 0 {
        if is_native_token(&Address(token_a)) {
            let _ = transfer_native(Address(treasury), fee_a);
        } else {
            let _ = call_token_transfer(Address(token_a), contract_addr, Address(treasury), fee_a);
        }
        save_u64(&key_a, 0);
    }
    if fee_b > 0 {
        if is_native_token(&Address(token_b)) {
            let _ = transfer_native(Address(treasury), fee_b);
        } else {
            let _ = call_token_transfer(Address(token_b), contract_addr, Address(treasury), fee_b);
        }
        save_u64(&key_b, 0);
    }
    0
}

/// Emergency pause (admin only)
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
    log_info("DEX AMM: EMERGENCY PAUSE ACTIVATED");
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

// ============================================================================
// QUERY FUNCTIONS
// ============================================================================

pub fn get_pool_info(pool_id: u64) -> u64 {
    let pk = pool_key(pool_id);
    match storage_get(&pk) {
        Some(d) if d.len() >= POOL_SIZE => {
            lichen_sdk::set_return_data(&d);
            pool_id
        }
        _ => 0,
    }
}

pub fn get_position(position_id: u64) -> u64 {
    let pk = position_key(position_id);
    match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE => {
            lichen_sdk::set_return_data(&d);
            position_id
        }
        _ => 0,
    }
}

pub fn get_pool_count() -> u64 {
    load_u64(POOL_COUNT_KEY)
}
pub fn get_position_count() -> u64 {
    load_u64(POSITION_COUNT_KEY)
}

pub fn get_tvl(pool_id: u64) -> u64 {
    let pk = pool_key(pool_id);
    match storage_get(&pk) {
        Some(d) if d.len() >= POOL_SIZE => decode_pool_liquidity(&d),
        _ => 0,
    }
}

/// Quote a swap without executing (uses cross-tick logic for accuracy)
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
    let current_tick = decode_pool_tick(&pool_data);
    let fee = (amount_in as u128 * fee_bps as u128 / 10_000) as u64;
    let amount_after_fee = amount_in - fee;
    let (out, _, _) = compute_swap_with_ticks(
        pool_id,
        amount_after_fee,
        liquidity,
        sqrt_price,
        current_tick,
        is_token_a_in,
    );
    out
}

// ============================================================================
// WASM ENTRY POINT
// ============================================================================

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn call() -> u32 {
    let args = lichen_sdk::get_args();
    if args.is_empty() {
        return 255;
    }
    let mut _rc = 0u32;
    match args[0] {
        // 0: initialize(admin)
        0 => {
            if args.len() >= 33 {
                let r = initialize(args[1..33].as_ptr());
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 1: create_pool(caller, token_a, token_b, fee_tier, initial_sqrt_price)
        1 => {
            // caller(32) + token_a(32) + token_b(32) + fee_tier(1) + initial_sqrt_price(8) = 105
            if args.len() >= 1 + 32 + 32 + 32 + 1 + 8 {
                let r = create_pool(
                    args[1..33].as_ptr(),
                    args[33..65].as_ptr(),
                    args[65..97].as_ptr(),
                    args[97],
                    bytes_to_u64(&args[98..106]),
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 2: set_pool_protocol_fee(caller, pool_id, fee_percent)
        2 => {
            // caller(32) + pool_id(8) + fee_percent(1) = 41
            if args.len() >= 1 + 32 + 8 + 1 {
                let r = set_pool_protocol_fee(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    args[41],
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 3: add_liquidity(provider, pool_id, lower_tick, upper_tick, amount_a, amount_b)
        3 => {
            // provider(32) + pool_id(8) + lower_tick(4) + upper_tick(4) + amount_a(8) + amount_b(8) = 64
            if args.len() >= 1 + 32 + 8 + 4 + 4 + 8 + 8 {
                let r = add_liquidity(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    bytes_to_i32(&args[41..45]),
                    bytes_to_i32(&args[45..49]),
                    bytes_to_u64(&args[49..57]),
                    bytes_to_u64(&args[57..65]),
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 4: remove_liquidity(provider, position_id, liquidity_amount)
        4 => {
            // provider(32) + position_id(8) + liquidity_amount(8) = 48
            if args.len() >= 1 + 32 + 8 + 8 {
                let r = remove_liquidity(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    bytes_to_u64(&args[41..49]),
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 5: collect_fees(provider, position_id)
        5 => {
            // provider(32) + position_id(8) = 40
            if args.len() >= 1 + 32 + 8 {
                let r = collect_fees(args[1..33].as_ptr(), bytes_to_u64(&args[33..41]));
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 6: swap_exact_in(trader, pool_id, is_token_a_in, amount_in, min_out, deadline)
        6 => {
            // trader(32) + pool_id(8) + is_token_a_in(1) + amount_in(8) + min_out(8) + deadline(8) = 65
            if args.len() >= 1 + 32 + 8 + 1 + 8 + 8 + 8 {
                let r = swap_exact_in(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    args[41] != 0,
                    bytes_to_u64(&args[42..50]),
                    bytes_to_u64(&args[50..58]),
                    bytes_to_u64(&args[58..66]),
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 7: swap_exact_out(trader, pool_id, is_token_a_out, amount_out, max_in, deadline)
        7 => {
            // trader(32) + pool_id(8) + is_token_a_out(1) + amount_out(8) + max_in(8) + deadline(8) = 65
            if args.len() >= 1 + 32 + 8 + 1 + 8 + 8 + 8 {
                let r = swap_exact_out(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    args[41] != 0,
                    bytes_to_u64(&args[42..50]),
                    bytes_to_u64(&args[50..58]),
                    bytes_to_u64(&args[58..66]),
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 8: emergency_pause(caller)
        8 => {
            if args.len() >= 1 + 32 {
                let r = emergency_pause(args[1..33].as_ptr());
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 9: emergency_unpause(caller)
        9 => {
            if args.len() >= 1 + 32 {
                let r = emergency_unpause(args[1..33].as_ptr());
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 10: get_pool_info(pool_id)
        10 => {
            if args.len() >= 1 + 8 {
                let r = get_pool_info(bytes_to_u64(&args[1..9]));
                lichen_sdk::set_return_data(&u64_to_bytes(r));
            }
        }
        // 11: get_position(position_id)
        11 => {
            if args.len() >= 1 + 8 {
                let r = get_position(bytes_to_u64(&args[1..9]));
                lichen_sdk::set_return_data(&u64_to_bytes(r));
            }
        }
        // 12: get_pool_count()
        12 => {
            let r = get_pool_count();
            lichen_sdk::set_return_data(&u64_to_bytes(r));
        }
        // 13: get_position_count()
        13 => {
            let r = get_position_count();
            lichen_sdk::set_return_data(&u64_to_bytes(r));
        }
        // 14: get_tvl(pool_id)
        14 => {
            if args.len() >= 1 + 8 {
                let r = get_tvl(bytes_to_u64(&args[1..9]));
                lichen_sdk::set_return_data(&u64_to_bytes(r));
            }
        }
        // 15: quote_swap(pool_id, is_token_a_in, amount_in)
        15 => {
            // pool_id(8) + is_token_a_in(1) + amount_in(8) = 17
            if args.len() >= 1 + 8 + 1 + 8 {
                let r = quote_swap(
                    bytes_to_u64(&args[1..9]),
                    args[9] != 0,
                    bytes_to_u64(&args[10..18]),
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r));
            }
        }
        16 => {
            // get_total_volume — returns cumulative swap volume
            lichen_sdk::set_return_data(&u64_to_bytes(load_u64(TOTAL_VOLUME_KEY)));
        }
        17 => {
            // get_swap_count — returns total number of swaps
            lichen_sdk::set_return_data(&u64_to_bytes(load_u64(SWAP_COUNT_KEY)));
        }
        18 => {
            // get_total_fees_collected — returns cumulative fees
            lichen_sdk::set_return_data(&u64_to_bytes(load_u64(TOTAL_FEES_KEY)));
        }
        19 => {
            // get_amm_stats — returns aggregated stats [pool_count, position_count, swap_count, total_volume, total_fees]
            let mut buf = Vec::with_capacity(40);
            buf.extend_from_slice(&u64_to_bytes(load_u64(POOL_COUNT_KEY)));
            buf.extend_from_slice(&u64_to_bytes(load_u64(POSITION_COUNT_KEY)));
            buf.extend_from_slice(&u64_to_bytes(load_u64(SWAP_COUNT_KEY)));
            buf.extend_from_slice(&u64_to_bytes(load_u64(TOTAL_VOLUME_KEY)));
            buf.extend_from_slice(&u64_to_bytes(load_u64(TOTAL_FEES_KEY)));
            lichen_sdk::set_return_data(&buf);
        }
        20 => {
            // set_fee_treasury_address(caller[32], treasury[32])
            if args.len() >= 65 {
                let r = set_fee_treasury_address(args[1..33].as_ptr(), args[33..65].as_ptr());
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
            }
        }
        21 => {
            // collect_protocol_fees(caller[32], pool_id[8])
            if args.len() >= 41 {
                let r = collect_protocol_fees(args[1..33].as_ptr(), bytes_to_u64(&args[33..41]));
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
            }
        }
        _ => {
            lichen_sdk::set_return_data(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
            _rc = 255;
        }
    }
    _rc
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use lichen_sdk::test_mock;

    fn setup() -> [u8; 32] {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        admin
    }

    fn setup_with_pool() -> ([u8; 32], u64) {
        let admin = setup();
        let ta = [10u8; 32];
        let tb = [20u8; 32];
        let sqrt_price = 1u64 << 32; // 1:1 price
        assert_eq!(
            create_pool(
                admin.as_ptr(),
                ta.as_ptr(),
                tb.as_ptr(),
                FEE_TIER_30BPS,
                sqrt_price
            ),
            0
        );
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
    }

    #[test]
    fn test_initialize_already_initialized() {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(initialize(admin.as_ptr()), 1);
    }

    // --- Pool Creation ---

    #[test]
    fn test_create_pool() {
        let admin = setup();
        let ta = [10u8; 32];
        let tb = [20u8; 32];
        assert_eq!(
            create_pool(
                admin.as_ptr(),
                ta.as_ptr(),
                tb.as_ptr(),
                FEE_TIER_30BPS,
                1u64 << 32
            ),
            0
        );
        assert_eq!(load_u64(POOL_COUNT_KEY), 1);
    }

    #[test]
    fn test_create_pool_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        let ta = [10u8; 32];
        let tb = [20u8; 32];
        test_mock::set_caller(rando);
        assert_eq!(
            create_pool(
                rando.as_ptr(),
                ta.as_ptr(),
                tb.as_ptr(),
                FEE_TIER_30BPS,
                1u64 << 32
            ),
            1
        );
    }

    #[test]
    fn test_create_pool_same_tokens() {
        let admin = setup();
        let t = [10u8; 32];
        assert_eq!(
            create_pool(
                admin.as_ptr(),
                t.as_ptr(),
                t.as_ptr(),
                FEE_TIER_30BPS,
                1u64 << 32
            ),
            4
        );
    }

    #[test]
    fn test_create_pool_invalid_fee_tier() {
        let admin = setup();
        let ta = [10u8; 32];
        let tb = [20u8; 32];
        assert_eq!(
            create_pool(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), 5, 1u64 << 32),
            4
        );
    }

    #[test]
    fn test_create_pool_zero_price() {
        let admin = setup();
        let ta = [10u8; 32];
        let tb = [20u8; 32];
        assert_eq!(
            create_pool(admin.as_ptr(), ta.as_ptr(), tb.as_ptr(), FEE_TIER_30BPS, 0),
            4
        );
    }

    // --- Liquidity ---

    #[test]
    fn test_add_liquidity() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(provider);
        // lower=-60, upper=60 (valid for 30bps tier with spacing 60)
        assert_eq!(
            add_liquidity(provider.as_ptr(), pool_id, -60, 60, 100_000, 100_000),
            0
        );
        assert_eq!(load_u64(POSITION_COUNT_KEY), 1);
    }

    #[test]
    fn test_add_liquidity_invalid_range() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        test_mock::set_caller(provider);
        // lower > upper
        assert_eq!(
            add_liquidity(provider.as_ptr(), pool_id, 60, -60, 100_000, 100_000),
            3
        );
    }

    #[test]
    fn test_add_liquidity_bad_tick_spacing() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        test_mock::set_caller(provider);
        // 30bps tier has spacing=60, so ticks must be multiple of 60
        assert_eq!(
            add_liquidity(provider.as_ptr(), pool_id, -30, 30, 100_000, 100_000),
            3
        );
    }

    #[test]
    fn test_add_liquidity_paused() {
        let (admin, pool_id) = setup_with_pool();
        emergency_pause(admin.as_ptr());
        let provider = [2u8; 32];
        test_mock::set_caller(provider);
        assert_eq!(
            add_liquidity(provider.as_ptr(), pool_id, -60, 60, 100_000, 100_000),
            1
        );
    }

    #[test]
    fn test_remove_liquidity() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(provider);
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
        test_mock::set_caller(provider);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 100_000, 100_000);
        test_mock::set_caller(other);
        assert_eq!(remove_liquidity(other.as_ptr(), 1, 1000), 2);
    }

    #[test]
    fn test_remove_liquidity_insufficient() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(provider);
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
        test_mock::set_caller(provider);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        test_mock::set_caller(trader);
        assert_eq!(
            swap_exact_in(trader.as_ptr(), pool_id, true, 10_000, 0, 0),
            0
        );
    }

    #[test]
    fn test_swap_deadline_expired() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        let trader = [3u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(provider);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        test_mock::set_caller(trader);
        // Deadline in the past
        assert_eq!(
            swap_exact_in(trader.as_ptr(), pool_id, true, 10_000, 0, 50),
            3
        );
    }

    #[test]
    fn test_swap_paused() {
        let (admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        let trader = [3u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(provider);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        test_mock::set_caller(admin);
        emergency_pause(admin.as_ptr());
        test_mock::set_caller(trader);
        assert_eq!(
            swap_exact_in(trader.as_ptr(), pool_id, true, 10_000, 0, 0),
            1
        );
    }

    #[test]
    fn test_swap_zero_amount() {
        let (_admin, pool_id) = setup_with_pool();
        let trader = [3u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        assert_eq!(swap_exact_in(trader.as_ptr(), pool_id, true, 0, 0, 0), 6);
    }

    #[test]
    fn test_swap_slippage_protection() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        let trader = [3u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(provider);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        test_mock::set_caller(trader);
        // Request impossibly high min_out
        assert_eq!(
            swap_exact_in(trader.as_ptr(), pool_id, true, 10_000, u64::MAX, 0),
            4
        );
    }

    #[test]
    fn test_swap_fee_accrual() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        let trader = [3u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(provider);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        test_mock::set_caller(trader);
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
        test_mock::set_caller(provider);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        test_mock::set_caller(trader);
        swap_exact_in(trader.as_ptr(), pool_id, true, 100_000, 0, 0);

        test_mock::set_caller(provider);
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
        test_mock::set_caller(provider);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        test_mock::set_caller(other);
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
        // AUDIT-FIX AMM-7: Cap at 50%, not 100%
        assert_eq!(set_pool_protocol_fee(admin.as_ptr(), pool_id, 51), 2);
        // 50% should succeed
        assert_eq!(set_pool_protocol_fee(admin.as_ptr(), pool_id, 50), 0);
    }

    #[test]
    fn test_set_fee_treasury_address() {
        let (admin, _pool_id) = setup_with_pool();
        let treasury = [42u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(
            set_fee_treasury_address(admin.as_ptr(), treasury.as_ptr()),
            0
        );
        assert_eq!(load_addr(FEE_TREASURY_ADDR_KEY), treasury);
    }

    #[test]
    fn test_set_fee_treasury_address_not_admin() {
        let (_admin, _pool_id) = setup_with_pool();
        let rando = [99u8; 32];
        test_mock::set_caller(rando);
        assert_eq!(set_fee_treasury_address(rando.as_ptr(), rando.as_ptr()), 1);
    }

    #[test]
    fn test_protocol_fee_split_in_accrue() {
        let (admin, pool_id) = setup_with_pool();
        // Set 10% protocol fee
        assert_eq!(set_pool_protocol_fee(admin.as_ptr(), pool_id, 10), 0);
        // Accrue 1000 fee for token A
        accrue_fees_to_positions(pool_id, 1000, true);
        // Check protocol fee accrued (10% of 1000 = 100)
        let mut key_a = Vec::from(PROTOCOL_FEE_ACCRUED_A_KEY);
        key_a.extend_from_slice(&u64_to_bytes(pool_id));
        assert_eq!(load_u64(&key_a), 100);
    }

    #[test]
    fn test_collect_protocol_fees_no_treasury() {
        let (admin, pool_id) = setup_with_pool();
        test_mock::set_caller(admin);
        assert_eq!(collect_protocol_fees(admin.as_ptr(), pool_id), 2);
    }

    // --- Tick Math (AUDIT-FIX G3-01: exponential accuracy tests) ---

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
        assert!(
            price_neg < price_pos,
            "Negative tick should give lower price"
        );
    }

    #[test]
    fn test_tick_exponential_accuracy() {
        // Verify against precomputed reference values (80-digit precision):
        // sqrt_price_Q32 = floor(1.00005^tick * 2^32)
        let test_vectors: &[(i32, u64)] = &[
            (0, 4_294_967_296),
            (1, 4_295_182_044),      // 1.00005
            (-1, 4_294_752_558),     // 0.999950002...
            (100, 4_316_495_369),    // 1.00501239...
            (-100, 4_273_546_591),   // 0.99501260...
            (600, 4_425_765_204),    // 1.03045376...
            (-600, 4_168_034_955),   // 0.97044626...
            (10000, 7_081_115_426),  // 1.64870066...
            (-10000, 2_605_061_909), // 0.60653824...
        ];

        for &(tick, expected) in test_vectors {
            let computed = tick_to_sqrt_price(tick);
            // Allow ±1 ULP tolerance for fixed-point rounding
            let diff = if computed > expected {
                computed - expected
            } else {
                expected - computed
            };
            assert!(
                diff <= 1,
                "tick_to_sqrt_price({}) = {} but expected {} (diff={})",
                tick,
                computed,
                expected,
                diff
            );
        }
    }

    #[test]
    fn test_tick_large_values() {
        // At tick 100000: sqrt_price = 1.00005^100000 ≈ 148.39
        // Q32.32 = floor(148.39 * 2^32) = 637,349,993,568
        let price_100k = tick_to_sqrt_price(100_000);
        let expected_100k: u64 = 637_349_993_568;
        let diff = if price_100k > expected_100k {
            price_100k - expected_100k
        } else {
            expected_100k - price_100k
        };
        assert!(diff <= 200, "tick 100000 off by {} (expected ~637B)", diff);

        // Negative large tick
        let price_neg100k = tick_to_sqrt_price(-100_000);
        let expected_neg100k: u64 = 28_942_879;
        let diff2 = if price_neg100k > expected_neg100k {
            price_neg100k - expected_neg100k
        } else {
            expected_neg100k - price_neg100k
        };
        assert!(
            diff2 <= 2,
            "tick -100000 off by {} (expected ~28.9M)",
            diff2
        );
    }

    #[test]
    fn test_tick_monotonicity() {
        // Price must strictly increase with tick
        let mut prev_price = tick_to_sqrt_price(-1000);
        for tick in (-999..=1000).step_by(1) {
            let price = tick_to_sqrt_price(tick);
            assert!(
                price > prev_price,
                "tick_to_sqrt_price not monotonic at tick {}: {} <= {}",
                tick,
                price,
                prev_price
            );
            prev_price = price;
        }
    }

    #[test]
    fn test_tick_roundtrip_range() {
        // Roundtrip accuracy across a range of ticks
        for tick in &[
            0, 1, -1, 10, -10, 100, -100, 600, -600, 1000, -1000, 10000, -10000, 100000, -100000,
        ] {
            let price = tick_to_sqrt_price(*tick);
            let recovered = sqrt_price_to_tick(price);
            assert!(
                (recovered - tick).abs() <= 1,
                "Roundtrip failed for tick {}: got {}",
                tick,
                recovered
            );
        }
    }

    #[test]
    fn test_mul_q64_basic() {
        // 1.0 * 1.0 = 1.0
        let one = 1u128 << 64;
        assert_eq!(mul_q64(one, one), one);

        // 1.0 * ratio[0] = ratio[0]
        let r0 = TICK_RATIOS[0];
        assert_eq!(mul_q64(one, r0), r0);

        // ratio[0] * ratio[0] should equal ratio[1] (since 1.00005^1 * 1.00005^1 = 1.00005^2)
        let r1 = TICK_RATIOS[1];
        let product = mul_q64(r0, r0);
        let diff = if product > r1 {
            product - r1
        } else {
            r1 - product
        };
        assert!(diff <= 1, "R0*R0 should equal R1, diff={}", diff);
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
        test_mock::set_caller(rando);
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
        create_pool(
            admin.as_ptr(),
            ta.as_ptr(),
            tb.as_ptr(),
            FEE_TIER_30BPS,
            1u64 << 32,
        );
        assert_eq!(get_pool_count(), 1);
    }

    #[test]
    fn test_get_tvl() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(provider);
        add_liquidity(provider.as_ptr(), pool_id, -60, 60, 1_000_000, 1_000_000);
        assert!(get_tvl(pool_id) > 0);
    }

    #[test]
    fn test_quote_swap() {
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(provider);
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

        test_mock::set_caller(p1);
        add_liquidity(p1.as_ptr(), pool_id, -60, 60, 500_000, 500_000);
        test_mock::set_caller(p2);
        add_liquidity(p2.as_ptr(), pool_id, -60, 60, 500_000, 500_000);

        test_mock::set_caller(trader);
        swap_exact_in(trader.as_ptr(), pool_id, true, 100_000, 0, 0);

        let pos1 = storage_get(&position_key(1)).unwrap();
        let pos2 = storage_get(&position_key(2)).unwrap();
        let fee1 = decode_pos_fee_a(&pos1);
        let fee2 = decode_pos_fee_a(&pos2);
        // Both should get approximately equal fees
        assert!(fee1 > 0 && fee2 > 0, "Both positions should earn fees");
    }

    // --- K3-03: Full AMM Lifecycle E2E ---

    #[test]
    fn test_full_amm_lifecycle_deposit_swap_withdraw() {
        // K3-03: Complete lifecycle: create pool → add liquidity (deposit)
        //        → swap (trade) → collect fees → remove liquidity (withdraw)
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        let trader = [4u8; 32];
        test_mock::set_slot(100);

        // --- Step 1: Provider adds liquidity ("deposit") ---
        test_mock::set_caller(provider);
        let result = add_liquidity(
            provider.as_ptr(),
            pool_id,
            -60, // lower tick
            60,  // upper tick
            1_000_000,
            1_000_000,
        );
        assert_eq!(result, 0, "add_liquidity should succeed");

        // Verify position created
        let pos_data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_owner(&pos_data), provider);
        assert_eq!(decode_pos_pool_id(&pos_data), pool_id);
        assert_eq!(decode_pos_lower_tick(&pos_data), -60);
        assert_eq!(decode_pos_upper_tick(&pos_data), 60);
        let initial_liquidity = decode_pos_liquidity(&pos_data);
        assert!(initial_liquidity > 0, "position must have liquidity");

        // No swaps yet
        assert_eq!(load_u64(SWAP_COUNT_KEY), 0);

        // --- Step 2: Trader swaps token_a → token_b ("trade") ---
        test_mock::set_caller(trader);
        let swap_result = swap_exact_in(
            trader.as_ptr(),
            pool_id,
            true,   // is_token_a_in
            10_000, // amount in
            0,      // min out (no slippage protection for test)
            0,      // deadline (no deadline)
        );
        assert_eq!(swap_result, 0, "swap should succeed");

        // Verify swap counted
        assert_eq!(load_u64(SWAP_COUNT_KEY), 1, "swap count should be 1");

        // --- Step 3: Provider collects fees ---
        test_mock::set_caller(provider);
        let collect_result = collect_fees(provider.as_ptr(), 1); // position_id = 1
        assert_eq!(collect_result, 0, "collect_fees should succeed");

        // After collection, accumulated fees should be zeroed
        let pos_after_collect = storage_get(&position_key(1)).unwrap();
        assert_eq!(
            decode_pos_fee_a(&pos_after_collect),
            0,
            "fee_a should be 0 after collection"
        );
        assert_eq!(
            decode_pos_fee_b(&pos_after_collect),
            0,
            "fee_b should be 0 after collection"
        );

        // --- Step 4: Provider removes all liquidity ("withdraw") ---
        test_mock::set_caller(provider);
        let remove_result = remove_liquidity(
            provider.as_ptr(),
            1, // position_id
            initial_liquidity,
        );
        assert_eq!(remove_result, 0, "remove_liquidity should succeed");

        // Verify position liquidity is now 0
        let pos_final = storage_get(&position_key(1)).unwrap();
        assert_eq!(
            decode_pos_liquidity(&pos_final),
            0,
            "liquidity should be 0 after full removal"
        );

        // --- Step 5: Pool still exists with correct state ---
        assert_eq!(load_u64(POOL_COUNT_KEY), 1, "pool count should still be 1");
        assert_eq!(load_u64(SWAP_COUNT_KEY), 1, "swap count should still be 1");
    }

    #[test]
    fn test_amm_multi_swap_volume_accumulation() {
        // K3-03: Multiple swaps accumulate volume correctly
        let (_admin, pool_id) = setup_with_pool();
        let provider = [2u8; 32];
        let trader = [4u8; 32];
        test_mock::set_slot(100);

        // Add deep liquidity
        test_mock::set_caller(provider);
        add_liquidity(provider.as_ptr(), pool_id, -120, 120, 5_000_000, 5_000_000);

        // Execute 3 swaps
        test_mock::set_caller(trader);
        for _ in 0..3 {
            assert_eq!(
                swap_exact_in(trader.as_ptr(), pool_id, true, 10_000, 0, 0),
                0
            );
        }

        assert_eq!(load_u64(SWAP_COUNT_KEY), 3, "3 swaps should be counted");

        // Fees should have accumulated on the position
        let pos_data = storage_get(&position_key(1)).unwrap();
        let fees = decode_pos_fee_a(&pos_data);
        assert!(fees > 0, "LP should have earned fees from 3 swaps");
    }
}
