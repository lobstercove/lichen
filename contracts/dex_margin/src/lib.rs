// DEX Margin — Margin Trading & Liquidation Engine (DEEP hardened)
//
// Features:
//   - Isolated margin positions (up to 100x leverage with tiered parameters)
//   - Tiered initial/maintenance margin and liquidation penalties
//   - Liquidation by anyone (earns 50% of penalty)
//   - Insurance fund from liquidation penalties
//   - Pool-backed funding rate (8-hour intervals, applied once to notional)
//   - Integration with ThallLend for margin funding
//   - Standard lUSD collateral custody via MT-20 approval/transfer_from
//   - Insurance fund governance withdrawal
//   - Emergency pause, reentrancy guard, admin controls
//   - Auto-deleveraging during extreme events

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(dead_code)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::manual_range_contains)]
#![allow(unused_imports)]

extern crate alloc;
use alloc::vec::Vec;

use lichen_sdk::{
    bytes_to_u64, call_contract, call_token_transfer, get_caller, get_contract_address, get_slot,
    get_timestamp, log_info, storage_get, storage_set, u64_to_bytes, Address, CrossCall,
};

// ============================================================================
// CONSTANTS
// ============================================================================

const MAX_LEVERAGE_ISOLATED: u64 = 100;
const MAX_LEVERAGE_CROSS: u64 = 3;
const MAX_CROSS_POSITIONS_PER_ACCOUNT: u64 = 32;
const LIQUIDATOR_SHARE_BPS: u64 = 5000; // 50% of penalty to liquidator
const INSURANCE_SHARE_BPS: u64 = 5000; // 50% of penalty to insurance
const FUNDING_INTERVAL_SLOTS: u64 = 72_000; // 8 hours at the canonical 400 ms slot target.
const MAX_POSITIONS: u64 = 10_000;
const MAX_FUNDING_RATE_BPS: u64 = 100; // 1% max per interval
const MAX_SKEW_FUNDING_BPS: u64 = 10; // 0.10%/interval at fully one-sided open size.
const FUNDING_INDEX_SCALE: u128 = 1_000_000_000;
const FUNDING_RATE_DENOMINATOR: u128 = 100_000; // 10_000 bps × fixed multiplier scale 10.
// Funding applies once to position notional. Leverage already changes notional
// relative to posted collateral; multiplying the rate by leverage tier again
// would double-charge risk and break equal-notional long/short symmetry.
const FUNDING_TIER_MULTIPLIERS: [u64; 7] = [10; 7];
// AUDIT-FIX H-11: Cap total open interest to prevent system insolvency
const MAX_TOTAL_OPEN_INTEREST: u64 = 100_000_000_000_000_000; // 100M LICN notional
const MIN_INSURANCE_COVERAGE_BPS: u64 = 10_000; // Require 1:1 insurance coverage for open notional.
const MARGIN_PRICE_SCALE: u64 = 1_000_000_000;
const ORACLE_PRICE_SCALE: u64 = 100_000_000;

// Match the native oracle's five-minute freshness boundary at a 400 ms slot
// target. Contract timestamps are deterministic slots, never wall-clock seconds.
const MAX_PRICE_AGE_SLOTS: u64 = 750;
const MAX_ORACLE_MARKET_BYTES: usize = 64;

// Position side
const SIDE_LONG: u8 = 0;
const SIDE_SHORT: u8 = 1;

// Margin mode
const MARGIN_MODE_ISOLATED: u8 = 0;
const MARGIN_MODE_CROSS: u8 = 1;

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
const TOTAL_COLLATERAL_ESCROWED_KEY: &[u8] = b"mrg_coll_esc";
const LAST_FUNDING_KEY: &[u8] = b"mrg_last_fund";
const COLLATERAL_TOKEN_ADDRESS_KEY: &[u8] = b"mrg_coll_addr";
const SELF_ADDRESS_KEY: &[u8] = b"mrg_self_addr";
const TOTAL_VOLUME_KEY: &[u8] = b"mrg_total_volume";
const LIQUIDATION_COUNT_KEY: &[u8] = b"mrg_liq_count";
const TOTAL_PNL_PROFIT_KEY: &[u8] = b"mrg_pnl_profit";
const TOTAL_PNL_LOSS_KEY: &[u8] = b"mrg_pnl_loss";
// Realized loss that exceeded collateral available at exit. This is an
// accounting deficit only: no corresponding lUSD may be credited to insurance.
const BAD_DEBT_KEY: &[u8] = b"mrg_bad_debt";
const FUNDING_V2_ENABLED_KEY: &[u8] = b"mrg_f2_enabled";
const FUNDING_V2_ACTIVATION_SLOT_KEY: &[u8] = b"mrg_f2_activation";
const FUNDING_POOL_KEY: &[u8] = b"mrg_f2_pool";
const FUNDING_TOTAL_CLAIMS_KEY: &[u8] = b"mrg_f2_claims";
const FUNDING_WRITEOFF_KEY: &[u8] = b"mrg_f2_writeoff";
const FUNDING_MIGRATED_OPEN_COUNT_KEY: &[u8] = b"mrg_f2_migrated";
const FUNDING_MIGRATION_FINALIZED_KEY: &[u8] = b"mrg_f2_mig_final";
const CROSS_V2_ENABLED_KEY: &[u8] = b"mrg_x2_enabled";
const CROSS_TOTAL_COLLATERAL_KEY: &[u8] = b"mrg_x2_total";
const CROSS_MIGRATED_OPEN_COUNT_KEY: &[u8] = b"mrg_x2_migrated";
const CROSS_MIGRATION_FINALIZED_KEY: &[u8] = b"mrg_x2_mig_final";
const MARGIN_V2_MIGRATION_LOCK_KEY: &[u8] = b"mrg_v2_mig_lock";
// AUDIT-FIX H-11: Track total open interest (notional) across all open positions
const TOTAL_OPEN_INTEREST_KEY: &[u8] = b"mrg_total_oi";
// AUDIT-FIX MARGIN-1: Oracle contract address for cross-contract price feeds
const ORACLE_ADDRESS_KEY: &[u8] = b"mrg_oracle_addr";

// Collateral asset mode stored in position byte 123.
const COLLATERAL_LUSD: u8 = 1;

// ============================================================================
// LEVERAGE TIER TABLE
// ============================================================================
// Returns (initial_margin_bps, maintenance_margin_bps, liquidation_penalty_bps,
// funding_rate_mult_x10). Funding is fixed at 1.0x because the rate already
// applies to leveraged position notional.
fn get_tier_params(leverage: u64) -> (u64, u64, u64, u64) {
    if leverage <= 2 {
        (5000, 2500, 300, 10) // 50% / 25% / 3% / 1.0x
    } else if leverage <= 3 {
        (3333, 1700, 300, 10) // 33% / 17% / 3% / 1.0x
    } else if leverage <= 5 {
        (2000, 1000, 500, 10) // 20% / 10% / 5% / 1.0x funding
    } else if leverage <= 10 {
        (1000, 500, 500, 10) // 10% / 5%  / 5% / 1.0x funding
    } else if leverage <= 25 {
        (400, 200, 700, 10) //  4% / 2%  / 7% / 1.0x funding
    } else if leverage <= 50 {
        (200, 100, 1000, 10) //  2% / 1%  / 10% / 1.0x funding
    } else {
        // ≤100x
        (100, 50, 1500, 10) //  1% / 0.5% / 15% / 1.0x funding
    }
}

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
fn load_i128(key: &[u8]) -> i128 {
    storage_get(key)
        .and_then(|data| data.get(..16).and_then(|bytes| bytes.try_into().ok()))
        .map(i128::from_le_bytes)
        .unwrap_or(0)
}
fn save_i128(key: &[u8], val: i128) {
    storage_set(key, &val.to_le_bytes());
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

fn has_configured_address(key: &[u8]) -> bool {
    storage_get(key).map(|d| d.len() >= 32).unwrap_or(false)
}

fn load_self_addr() -> [u8; 32] {
    load_addr(SELF_ADDRESS_KEY)
}

fn load_contract_escrow_addr() -> Option<[u8; 32]> {
    let runtime_addr = get_contract_address().0;
    if !is_zero(&runtime_addr) {
        return Some(runtime_addr);
    }

    let configured = load_self_addr();
    if is_zero(&configured) {
        None
    } else {
        Some(configured)
    }
}

fn decode_collateral_transfer_from_result(result: &[u8]) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    if result.is_empty() {
        return true;
    }

    let status = if result.len() >= 4 {
        u32::from_le_bytes([result[0], result[1], result[2], result[3]])
    } else {
        result.first().copied().unwrap_or(255) as u32
    };

    match status {
        0 => true,
        1 => {
            log_info("margin collateral transfer_from failed: token paused");
            false
        }
        5 => {
            log_info("margin collateral transfer_from failed: insufficient balance");
            false
        }
        7 => {
            log_info("margin collateral transfer_from failed: insufficient allowance");
            false
        }
        100 => {
            log_info("margin collateral transfer_from failed: reentrancy guard");
            false
        }
        200 => {
            log_info("margin collateral transfer_from failed: caller mismatch");
            false
        }
        _ => {
            log_info("margin collateral transfer_from failed: token error");
            false
        }
    }
}

fn escrow_lusd_collateral_in(payer: &[u8; 32], amount: u64) -> bool {
    if amount == 0 {
        return true;
    }

    let token_addr = load_addr(COLLATERAL_TOKEN_ADDRESS_KEY);
    if is_zero(&token_addr) {
        log_info("margin collateral token not configured");
        return false;
    }

    let contract_addr = match load_contract_escrow_addr() {
        Some(addr) => addr,
        None => {
            log_info("margin self address not configured");
            return false;
        }
    };

    let mut args = Vec::with_capacity(104);
    args.extend_from_slice(&contract_addr);
    args.extend_from_slice(payer);
    args.extend_from_slice(&contract_addr);
    args.extend_from_slice(&u64_to_bytes(amount));

    let call = CrossCall::new(Address(token_addr), "transfer_from", args);
    match call_contract(call) {
        Ok(result) => decode_collateral_transfer_from_result(&result),
        Err(_) => {
            log_info("margin collateral transfer_from failed: cross-call error");
            false
        }
    }
}

fn transfer_lusd_collateral_out(recipient: &[u8; 32], amount: u64) -> bool {
    if amount == 0 {
        return true;
    }

    let token_addr = load_addr(COLLATERAL_TOKEN_ADDRESS_KEY);
    if is_zero(&token_addr) {
        log_info("margin collateral token not configured");
        return false;
    }

    let self_addr = match load_contract_escrow_addr() {
        Some(addr) => addr,
        None => {
            log_info("margin self address not configured");
            return false;
        }
    };

    match call_token_transfer(
        Address(token_addr),
        Address(self_addr),
        Address(*recipient),
        amount,
    ) {
        Ok(true) => true,
        Ok(false) => {
            log_info("margin collateral transfer returned failure");
            false
        }
        Err(_) => {
            log_info("margin collateral transfer failed");
            false
        }
    }
}

fn collateral_in(payer: &[u8; 32], amount: u64) -> bool {
    escrow_lusd_collateral_in(payer, amount)
}

fn collateral_out(recipient: &[u8; 32], amount: u64) -> bool {
    transfer_lusd_collateral_out(recipient, amount)
}

fn pay_liquidator_reward(liquidator: &[u8; 32], amount: u64) -> bool {
    transfer_lusd_collateral_out(liquidator, amount)
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

fn oracle_lookup_args(asset: &[u8]) -> Option<Vec<u8>> {
    if asset.is_empty() || asset.len() > 64 {
        return None;
    }

    let padded_len = asset.len().div_ceil(32) * 32;
    let mut args = Vec::with_capacity(1 + 2 + padded_len + 4);
    args.push(0xAB);
    args.push(padded_len as u8);
    args.push(4);
    args.extend_from_slice(asset);
    while args.len() < 1 + 2 + padded_len {
        args.push(0);
    }
    args.extend_from_slice(&(asset.len() as u32).to_le_bytes());
    Some(args)
}

fn oracle_price_to_margin_price(price: u64) -> Option<u64> {
    price.checked_mul(MARGIN_PRICE_SCALE / ORACLE_PRICE_SCALE)
}

fn read_oracle_market(ptr: *const u8, len: u32) -> Option<Vec<u8>> {
    let len = len as usize;
    if ptr.is_null() || len == 0 || len > MAX_ORACLE_MARKET_BYTES {
        return None;
    }
    let mut market = alloc::vec![0u8; len];
    unsafe {
        core::ptr::copy_nonoverlapping(ptr, market.as_mut_ptr(), len);
    }
    let (base, quote) = match market.iter().position(|byte| *byte == b'/') {
        Some(separator) => (&market[..separator], Some(&market[separator + 1..])),
        None => (market.as_slice(), None),
    };
    let valid_asset = |asset: &[u8]| {
        !asset.is_empty()
            && asset.len() <= 16
            && asset.iter().all(|byte| byte.is_ascii_alphanumeric())
    };
    if !valid_asset(base) || quote.is_some_and(|quote| quote != b"LICN") {
        return None;
    }
    Some(market)
}

fn oracle_market_parts(market: &[u8]) -> Option<(&[u8], Option<&[u8]>)> {
    match market.iter().position(|byte| *byte == b'/') {
        Some(separator) => {
            let base = &market[..separator];
            let quote = &market[separator + 1..];
            if base.is_empty() || quote != b"LICN" {
                None
            } else {
                Some((base, Some(quote)))
            }
        }
        None if !market.is_empty() => Some((market, None)),
        None => None,
    }
}

fn call_oracle_fixed8_quote(oracle_addr: [u8; 32], asset: &[u8]) -> Result<(u64, u64), u32> {
    let oracle_args = oracle_lookup_args(asset).ok_or(3u32)?;
    let oracle_call = CrossCall::new(Address(oracle_addr), "get_price_value", oracle_args);
    let result = call_contract(oracle_call).map_err(|_| 2u32)?;
    if result.len() < 17 || result[16] != 8 {
        return Err(3);
    }
    let price = bytes_to_u64(&result[0..8]);
    let source_slot = bytes_to_u64(&result[8..16]);
    let current_slot = get_timestamp();
    if price == 0 {
        return Err(4);
    }
    if source_slot > current_slot || current_slot - source_slot > MAX_PRICE_AGE_SLOTS {
        return Err(3);
    }
    Ok((price, source_slot))
}

fn oracle_market_to_margin_price(
    oracle_addr: [u8; 32],
    market: &[u8],
) -> Result<(u64, u64), u32> {
    let (base, quote) = oracle_market_parts(market).ok_or(5u32)?;
    let (base_price, base_slot) = call_oracle_fixed8_quote(oracle_addr, base)?;
    match quote {
        None => oracle_price_to_margin_price(base_price)
            .filter(|price| *price > 0)
            .map(|price| (price, base_slot))
            .ok_or(4),
        Some(quote) if quote == b"LICN" => {
            let (licn_price, licn_slot) = call_oracle_fixed8_quote(oracle_addr, b"LICN")?;
            if licn_price == 0 {
                return Err(4);
            }
            let scaled = base_price as u128 * MARGIN_PRICE_SCALE as u128 / licn_price as u128;
            let price = u64::try_from(scaled).map_err(|_| 4u32)?;
            if price == 0 {
                return Err(4);
            }
            Ok((price, base_slot.min(licn_slot)))
        }
        Some(_) => Err(5),
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

fn position_key(pos_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_pos_"[..]);
    k.extend_from_slice(&u64_to_decimal(pos_id));
    k
}
fn max_leverage_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_maxl_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k
}
fn margin_enabled_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_ena_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k
}
fn maintenance_margin_key_fn() -> Vec<u8> {
    Vec::from(&b"mrg_maint_bps"[..])
}
fn user_position_count_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_upc_"[..]);
    k.extend_from_slice(&hex_encode(addr));
    k
}
fn user_position_key(addr: &[u8; 32], idx: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_up_"[..]);
    k.extend_from_slice(&hex_encode(addr));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(idx));
    k
}
fn mark_price_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_mark_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k
}
fn index_price_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_idx_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k
}
fn last_funding_pair_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_lfund_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k
}
fn cumulative_funding_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_cfund_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k
}
fn funding_v2_last_slot_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_f2last_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k
}
fn funding_index_key(pair_id: u64, tier: u8) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_f2idx_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(tier as u64));
    k
}
fn pair_long_size_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_lsize_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k
}
fn pair_short_size_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_ssize_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k
}
fn position_funding_index_key(position_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_pf2idx_"[..]);
    k.extend_from_slice(&u64_to_decimal(position_id));
    k
}
fn position_funding_debt_key(position_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_pf2debt_"[..]);
    k.extend_from_slice(&u64_to_decimal(position_id));
    k
}
fn user_funding_claim_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_uf2claim_"[..]);
    k.extend_from_slice(&hex_encode(addr));
    k
}
fn cross_balance_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_x2_bal_"[..]);
    k.extend_from_slice(&hex_encode(addr));
    k
}
fn cross_position_count_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_x2_count_"[..]);
    k.extend_from_slice(&hex_encode(addr));
    k
}
fn cross_position_key(addr: &[u8; 32], idx: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_x2_pos_"[..]);
    k.extend_from_slice(&hex_encode(addr));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(idx));
    k
}
fn cross_position_index_key(position_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_x2_idx_"[..]);
    k.extend_from_slice(&u64_to_decimal(position_id));
    k
}
fn cross_position_migrated_key(position_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_x2_mig_"[..]);
    k.extend_from_slice(&u64_to_decimal(position_id));
    k
}
fn oracle_market_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"mrg_market_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id));
    k
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
    let data = match storage_get(&mark_price_key(pair_id)) {
        Some(data) if data.len() >= 16 => data,
        _ => return 0,
    };
    let price = bytes_to_u64(&data[0..8]);
    let ts = bytes_to_u64(&data[8..16]);
    let now = get_timestamp();
    if price == 0 || ts > now || now - ts > MAX_PRICE_AGE_SLOTS {
        log_info("DEX Margin: Mark price stale — rejecting");
        return 0;
    }
    price
}

/// Load an index price only when it is fresh enough to trust for liquidation
/// and other safety-critical fallback paths.
fn fresh_index_price(pair_id: u64) -> u64 {
    let data = match storage_get(&index_price_key(pair_id)) {
        Some(data) if data.len() >= 16 => data,
        _ => return 0,
    };
    let price = bytes_to_u64(&data[0..8]);
    let ts = bytes_to_u64(&data[8..16]);
    let now = get_timestamp();
    if price == 0 || ts > now || now - ts > MAX_PRICE_AGE_SLOTS {
        log_info("DEX Margin: Index price stale — rejecting");
        return 0;
    }
    price
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

fn margin_v2_migration_locked() -> bool {
    storage_get(MARGIN_V2_MIGRATION_LOCK_KEY)
        .and_then(|data| data.first().copied())
        == Some(1)
}

// ============================================================================
// POSITION LAYOUT (128 bytes, V1 was 112)
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
// Bytes 106..114: sl_price (u64, stop-loss trigger price, 0 = none)
// Bytes 114..122: tp_price (u64, take-profit trigger price, 0 = none)
// Byte  122     : margin_mode (0=isolated, 1=cross)
// Byte  123     : collateral_mode (1=lUSD MT-20)
// Bytes 124..128: padding

/// V1 position records are 112 bytes — guards use this for backward compat
const POSITION_SIZE_V1: usize = 112;
const POSITION_SIZE: usize = 128;

fn encode_position(
    trader: &[u8; 32],
    pos_id: u64,
    pair_id: u64,
    side: u8,
    status: u8,
    size: u64,
    margin: u64,
    entry_price: u64,
    leverage: u64,
    created_slot: u64,
    realized_pnl: u64,
    accumulated_funding: u64,
    margin_mode: u8,
    collateral_mode: u8,
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
    // SL/TP default to 0 (no trigger)
    data.extend_from_slice(&u64_to_bytes(0)); // sl_price
    data.extend_from_slice(&u64_to_bytes(0)); // tp_price
    data.push(margin_mode);
    data.push(collateral_mode);
    while data.len() < POSITION_SIZE {
        data.push(0);
    }
    data
}

/// Decode stop-loss price from position data (0 if V1 record or not set)
fn decode_pos_sl_price(data: &[u8]) -> u64 {
    if data.len() >= 114 {
        bytes_to_u64(&data[106..114])
    } else {
        0
    }
}

/// Decode take-profit price from position data (0 if V1 record or not set)
fn decode_pos_tp_price(data: &[u8]) -> u64 {
    if data.len() >= 122 {
        bytes_to_u64(&data[114..122])
    } else {
        0
    }
}

/// Update stop-loss price on a position record. Grows V1 records to 128 bytes.
fn update_pos_sl_price(data: &mut Vec<u8>, sl: u64) {
    while data.len() < POSITION_SIZE {
        data.push(0);
    }
    data[106..114].copy_from_slice(&u64_to_bytes(sl));
}

/// Update take-profit price on a position record. Grows V1 records to 128 bytes.
fn update_pos_tp_price(data: &mut Vec<u8>, tp: u64) {
    while data.len() < POSITION_SIZE {
        data.push(0);
    }
    data[114..122].copy_from_slice(&u64_to_bytes(tp));
}

fn decode_pos_trader(data: &[u8]) -> [u8; 32] {
    let mut t = [0u8; 32];
    if data.len() >= 32 {
        t.copy_from_slice(&data[..32]);
    }
    t
}
fn decode_pos_id(data: &[u8]) -> u64 {
    if data.len() >= 40 {
        bytes_to_u64(&data[32..40])
    } else {
        0
    }
}
fn decode_pos_pair_id(data: &[u8]) -> u64 {
    if data.len() >= 48 {
        bytes_to_u64(&data[40..48])
    } else {
        0
    }
}
fn decode_pos_side(data: &[u8]) -> u8 {
    if data.len() > 48 {
        data[48]
    } else {
        0
    }
}
fn decode_pos_status(data: &[u8]) -> u8 {
    if data.len() > 49 {
        data[49]
    } else {
        0
    }
}
fn decode_pos_size(data: &[u8]) -> u64 {
    if data.len() >= 58 {
        bytes_to_u64(&data[50..58])
    } else {
        0
    }
}
fn decode_pos_margin(data: &[u8]) -> u64 {
    if data.len() >= 66 {
        bytes_to_u64(&data[58..66])
    } else {
        0
    }
}
fn decode_pos_entry_price(data: &[u8]) -> u64 {
    if data.len() >= 74 {
        bytes_to_u64(&data[66..74])
    } else {
        0
    }
}
fn decode_pos_leverage(data: &[u8]) -> u64 {
    if data.len() >= 82 {
        bytes_to_u64(&data[74..82])
    } else {
        0
    }
}
fn decode_pos_accumulated_funding(data: &[u8]) -> u64 {
    if data.len() >= 106 {
        bytes_to_u64(&data[98..106])
    } else {
        0
    }
}
fn decode_pos_margin_mode(data: &[u8]) -> u8 {
    if data.len() > 122 {
        let mode = data[122];
        if mode == MARGIN_MODE_CROSS {
            MARGIN_MODE_CROSS
        } else {
            MARGIN_MODE_ISOLATED
        }
    } else {
        MARGIN_MODE_ISOLATED
    }
}

fn update_pos_status(data: &mut Vec<u8>, s: u8) {
    if data.len() > 49 {
        data[49] = s;
    }
}
fn update_pos_size(data: &mut Vec<u8>, s: u64) {
    if data.len() >= 58 {
        data[50..58].copy_from_slice(&u64_to_bytes(s));
    }
}
fn update_pos_margin(data: &mut Vec<u8>, m: u64) {
    if data.len() >= 66 {
        data[58..66].copy_from_slice(&u64_to_bytes(m));
    }
}
fn update_pos_accumulated_funding(data: &mut Vec<u8>, f: u64) {
    while data.len() < POSITION_SIZE {
        data.push(0);
    }
    data[98..106].copy_from_slice(&u64_to_bytes(f));
}

fn funding_tier_id(leverage: u64) -> u8 {
    if leverage <= 2 {
        0
    } else if leverage <= 3 {
        1
    } else if leverage <= 5 {
        2
    } else if leverage <= 10 {
        3
    } else if leverage <= 25 {
        4
    } else if leverage <= 50 {
        5
    } else {
        6
    }
}

fn funding_v2_enabled() -> bool {
    storage_get(FUNDING_V2_ENABLED_KEY)
        .and_then(|data| data.first().copied())
        == Some(1)
}

fn cross_v2_enabled() -> bool {
    storage_get(CROSS_V2_ENABLED_KEY)
        .and_then(|data| data.first().copied())
        == Some(1)
}

fn current_funding_index(pair_id: u64, leverage: u64) -> i128 {
    load_i128(&funding_index_key(pair_id, funding_tier_id(leverage)))
}

fn funding_amount_from_index(size: u64, index_delta: i128) -> Option<u64> {
    let delta = index_delta.unsigned_abs();
    let whole = delta / FUNDING_INDEX_SCALE;
    let remainder = delta % FUNDING_INDEX_SCALE;
    let amount = whole
        .checked_mul(size as u128)?
        .checked_add(remainder.checked_mul(size as u128)? / FUNDING_INDEX_SCALE)?;
    u64::try_from(amount).ok()
}

/// Lazily settles one position against the global funding index. Payer margin
/// is moved into a real contract-held pool before receivers can be credited.
/// Unfunded receiver entitlements remain explicit user claims; they never
/// become spendable position collateral until matching payer funds exist.
fn settle_position_funding_state(
    position_id: u64,
    data: &mut Vec<u8>,
) -> Result<(u64, u64, u64, u64), u32> {
    if !funding_v2_enabled() {
        return Err(4);
    }
    if decode_pos_status(data) != POS_OPEN {
        return Err(1);
    }

    let pair_id = decode_pos_pair_id(data);
    let leverage = decode_pos_leverage(data);
    let side = decode_pos_side(data);
    let trader = decode_pos_trader(data);
    let current_index = current_funding_index(pair_id, leverage);
    let last_index_key = position_funding_index_key(position_id);
    let stored_last_index = storage_get(&last_index_key).ok_or(5u32)?;
    let index_delta = stored_last_index
        .get(..16)
        .and_then(|bytes| bytes.try_into().ok())
        .map(i128::from_le_bytes)
        .map(|last| current_index.checked_sub(last).ok_or(2u32))
        .transpose()?
        .ok_or(5u32)?;

    let funding_amount = funding_amount_from_index(decode_pos_size(data), index_delta).ok_or(2u32)?;
    let position_pays = (side == SIDE_LONG && index_delta > 0)
        || (side == SIDE_SHORT && index_delta < 0);

    let debt_key = position_funding_debt_key(position_id);
    let claim_key = user_funding_claim_key(&trader);
    let mut debt = load_u64(&debt_key);
    let mut claim = load_u64(&claim_key);
    let mut total_claims = load_u64(FUNDING_TOTAL_CLAIMS_KEY);
    let mut pool = load_u64(FUNDING_POOL_KEY);
    let is_cross = decode_pos_margin_mode(data) == MARGIN_MODE_CROSS;
    if is_cross && !cross_v2_enabled() {
        return Err(13);
    }
    if is_cross {
        let registry_index = load_u64(&cross_position_index_key(position_id));
        if registry_index == 0
            || load_u64(&cross_position_key(&trader, registry_index)) != position_id
        {
            return Err(13);
        }
    }
    let mut available_collateral = if is_cross {
        load_u64(&cross_balance_key(&trader))
    } else {
        decode_pos_margin(data)
    };

    if position_pays {
        debt = debt.checked_add(funding_amount).ok_or(2u32)?;
    } else if index_delta != 0 {
        claim = claim.checked_add(funding_amount).ok_or(2u32)?;
        total_claims = total_claims.checked_add(funding_amount).ok_or(2u32)?;
    }

    // A user's opposite-side entitlement can cancel this position's debt
    // without moving tokens. Both liabilities decrease by the exact amount.
    let netted = debt.min(claim);
    debt -= netted;
    claim -= netted;
    total_claims = total_claims.checked_sub(netted).ok_or(2u32)?;

    let collected = debt.min(available_collateral);
    debt -= collected;
    available_collateral -= collected;
    pool = pool.checked_add(collected).ok_or(2u32)?;

    let credited = claim.min(pool);
    claim -= credited;
    pool -= credited;
    total_claims = total_claims.checked_sub(credited).ok_or(2u32)?;
    available_collateral = available_collateral.checked_add(credited).ok_or(2u32)?;

    let escrowed = load_u64(TOTAL_COLLATERAL_ESCROWED_KEY);
    let next_escrowed = escrowed
        .checked_sub(collected)
        .and_then(|value| value.checked_add(credited))
        .ok_or(2u32)?;
    let next_cross_total = if is_cross {
        Some(
            load_u64(CROSS_TOTAL_COLLATERAL_KEY)
                .checked_sub(collected)
                .and_then(|value| value.checked_add(credited))
                .ok_or(2u32)?,
        )
    } else {
        None
    };

    let previous_funding_raw = decode_pos_accumulated_funding(data);
    let zero = 1u64 << 63;
    let previous_funding = if previous_funding_raw == 0 && index_delta != 0 {
        zero
    } else {
        previous_funding_raw
    };
    let next_funding = if position_pays {
        previous_funding.saturating_sub(funding_amount)
    } else if index_delta != 0 {
        previous_funding.saturating_add(funding_amount)
    } else {
        previous_funding
    };

    if is_cross {
        save_u64(&cross_balance_key(&trader), available_collateral);
        save_u64(
            CROSS_TOTAL_COLLATERAL_KEY,
            next_cross_total.ok_or(2u32)?,
        );
        update_pos_margin(data, 0);
    } else {
        update_pos_margin(data, available_collateral);
    }
    update_pos_accumulated_funding(data, next_funding);
    storage_set(&position_key(position_id), data);
    save_i128(&last_index_key, current_index);
    save_u64(&debt_key, debt);
    save_u64(&claim_key, claim);
    save_u64(FUNDING_TOTAL_CLAIMS_KEY, total_claims);
    save_u64(FUNDING_POOL_KEY, pool);
    save_u64(TOTAL_COLLATERAL_ESCROWED_KEY, next_escrowed);

    Ok((collected, credited, debt, claim))
}

struct FundingExitPlan {
    payout: u64,
    collected: u64,
    written_off: u64,
    next_pool: u64,
    next_total_writeoff: u64,
}

struct FundingPayoutPlan {
    payout: u64,
    next_debt: u64,
    next_pool: u64,
}

fn funding_payout_plan(position_id: u64, gross_payout: u64) -> Result<FundingPayoutPlan, u32> {
    let debt = load_u64(&position_funding_debt_key(position_id));
    let collected = debt.min(gross_payout);
    Ok(FundingPayoutPlan {
        payout: gross_payout - collected,
        next_debt: debt - collected,
        next_pool: load_u64(FUNDING_POOL_KEY)
            .checked_add(collected)
            .ok_or(12u32)?,
    })
}

fn commit_funding_payout(position_id: u64, plan: &FundingPayoutPlan) {
    save_u64(FUNDING_POOL_KEY, plan.next_pool);
    save_u64(&position_funding_debt_key(position_id), plan.next_debt);
}

fn funding_exit_plan(position_id: u64, gross_payout: u64) -> Result<FundingExitPlan, u32> {
    let debt = load_u64(&position_funding_debt_key(position_id));
    let collected = debt.min(gross_payout);
    let written_off = debt - collected;
    let next_pool = load_u64(FUNDING_POOL_KEY)
        .checked_add(collected)
        .ok_or(12u32)?;
    let next_total_writeoff = load_u64(FUNDING_WRITEOFF_KEY)
        .checked_add(written_off)
        .ok_or(12u32)?;
    Ok(FundingExitPlan {
        payout: gross_payout - collected,
        collected,
        written_off,
        next_pool,
        next_total_writeoff,
    })
}

fn commit_funding_exit(position_id: u64, plan: &FundingExitPlan) {
    save_u64(FUNDING_POOL_KEY, plan.next_pool);
    save_u64(&position_funding_debt_key(position_id), 0);
    save_u64(FUNDING_WRITEOFF_KEY, plan.next_total_writeoff);
}

/// Calculate margin ratio
/// margin_ratio = margin / (size * mark_price / 1e9)
fn calculate_margin_ratio(margin: u64, size: u64, mark_price: u64) -> u64 {
    let notional = match calculate_notional(size, mark_price) {
        Some(value) => value,
        None => return 0,
    };
    if notional == 0 {
        return 10_000;
    } // safe
    (margin as u128 * 10_000 / notional as u128) as u64 // in bps
}

fn calculate_notional(size: u64, price: u64) -> Option<u64> {
    let notional = size as u128 * price as u128 / 1_000_000_000;
    if notional > u64::MAX as u128 {
        None
    } else {
        Some(notional as u64)
    }
}

/// F10.2-A FIX: Calculate margin ratio accounting for unrealized PnL
/// effective_margin = margin ± unrealized PnL, then ratio = effective / notional
fn calculate_margin_ratio_with_pnl(
    margin: u64,
    size: u64,
    entry_price: u64,
    mark_price: u64,
    side: u8,
) -> u64 {
    let (is_profit, pnl) = match calculate_pnl(side, size, entry_price, mark_price) {
        Some(value) => value,
        None => return 0,
    };
    let effective = if is_profit {
        margin.saturating_add(pnl)
    } else {
        margin.saturating_sub(pnl)
    };
    let notional = match calculate_notional(size, mark_price) {
        Some(value) => value,
        None => return 0,
    };
    if notional == 0 {
        return 10_000;
    }
    (effective as u128 * 10_000 / notional as u128) as u64
}

/// Calculate unrealized PnL
fn calculate_pnl(
    side: u8,
    size: u64,
    entry_price: u64,
    mark_price: u64,
) -> Option<(bool, u64)> {
    let (is_profit, price_delta) = if side == SIDE_LONG {
        if mark_price >= entry_price {
            (true, mark_price - entry_price)
        } else {
            (false, entry_price - mark_price)
        }
    } else if mark_price <= entry_price {
        (true, entry_price - mark_price)
    } else {
        (false, mark_price - entry_price)
    };
    let pnl = size as u128 * price_delta as u128 / MARGIN_PRICE_SCALE as u128;
    Some((is_profit, u64::try_from(pnl).ok()?))
}

struct CrossPortfolioMetrics {
    balance: u64,
    position_count: u64,
    signed_equity: i128,
    equity: u64,
    total_notional: u64,
    initial_required: u64,
    maintenance_required: u64,
    funding_debt: u64,
}

/// Computes one cross account in bounded O(MAX_CROSS_POSITIONS_PER_ACCOUNT)
/// work. Every open position must be present, owned by the account, and have a
/// fresh mark; otherwise safety-sensitive operations fail closed.
fn cross_portfolio_metrics(trader: &[u8; 32]) -> Result<CrossPortfolioMetrics, u32> {
    if !cross_v2_enabled() {
        return Err(13);
    }
    let count = load_u64(&cross_position_count_key(trader));
    if count > MAX_CROSS_POSITIONS_PER_ACCOUNT {
        return Err(13);
    }

    let mut net_pnl = 0i128;
    let mut total_notional = 0u64;
    let mut initial_required = 0u64;
    let mut maintenance_required = 0u64;
    let mut funding_debt = 0u64;
    let admin_maintenance = get_maintenance_margin_override();

    for index in 1..=count {
        let position_id = load_u64(&cross_position_key(trader, index));
        let data = storage_get(&position_key(position_id)).ok_or(13u32)?;
        if position_id == 0
            || data.len() < POSITION_SIZE_V1
            || decode_pos_status(&data) != POS_OPEN
            || decode_pos_margin_mode(&data) != MARGIN_MODE_CROSS
            || decode_pos_trader(&data) != *trader
            || load_u64(&cross_position_index_key(position_id)) != index
        {
            return Err(13);
        }

        let mark_price = fresh_mark_price(decode_pos_pair_id(&data));
        if mark_price == 0 {
            return Err(6);
        }
        let size = decode_pos_size(&data);
        let notional = calculate_notional(size, mark_price).ok_or(13u32)?;
        let (is_profit, pnl) = calculate_pnl(
            decode_pos_side(&data),
            size,
            decode_pos_entry_price(&data),
            mark_price,
        )
        .ok_or(13u32)?;
        net_pnl = if is_profit {
            net_pnl.checked_add(pnl as i128)
        } else {
            net_pnl.checked_sub(pnl as i128)
        }
        .ok_or(13u32)?;

        let (initial_bps, maintenance_bps, _, _) =
            get_tier_params(decode_pos_leverage(&data));
        let effective_maintenance = maintenance_bps.max(admin_maintenance);
        let initial = u64::try_from(
            (notional as u128 * initial_bps as u128 / 10_000).max(1),
        )
        .map_err(|_| 13u32)?;
        let maintenance = u64::try_from(
            (notional as u128 * effective_maintenance as u128 / 10_000).max(1),
        )
        .map_err(|_| 13u32)?;
        total_notional = total_notional.checked_add(notional).ok_or(13u32)?;
        initial_required = initial_required.checked_add(initial).ok_or(13u32)?;
        maintenance_required = maintenance_required
            .checked_add(maintenance)
            .ok_or(13u32)?;
        funding_debt = funding_debt
            .checked_add(load_u64(&position_funding_debt_key(position_id)))
            .ok_or(13u32)?;
    }

    let balance = load_u64(&cross_balance_key(trader));
    let signed_equity = (balance as i128)
        .checked_add(net_pnl)
        .and_then(|value| value.checked_sub(funding_debt as i128))
        .ok_or(13u32)?;
    let equity = if signed_equity <= 0 {
        0
    } else {
        u64::try_from(signed_equity).map_err(|_| 13u32)?
    };
    Ok(CrossPortfolioMetrics {
        balance,
        position_count: count,
        signed_equity,
        equity,
        total_notional,
        initial_required,
        maintenance_required,
        funding_debt,
    })
}

fn settle_cross_portfolio_funding(trader: &[u8; 32]) -> Result<(), u32> {
    if !cross_v2_enabled() {
        return Err(13);
    }
    let count = load_u64(&cross_position_count_key(trader));
    if count > MAX_CROSS_POSITIONS_PER_ACCOUNT {
        return Err(13);
    }
    // Two bounded passes remove ordering effects between a user's per-position
    // debts and user-level claims. The first realizes every index delta; the
    // second nets/collects any debt that preceded a later claim or pool credit.
    for _ in 0..2 {
        for index in 1..=count {
            let position_id = load_u64(&cross_position_key(trader, index));
            let mut data = storage_get(&position_key(position_id)).ok_or(13u32)?;
            if position_id == 0
                || data.len() < POSITION_SIZE_V1
                || decode_pos_status(&data) != POS_OPEN
                || decode_pos_margin_mode(&data) != MARGIN_MODE_CROSS
                || decode_pos_trader(&data) != *trader
                || load_u64(&cross_position_index_key(position_id)) != index
            {
                return Err(13);
            }
            settle_position_funding_state(position_id, &mut data)?;
        }
    }
    Ok(())
}

struct CrossPositionRemoval {
    count: u64,
    index: u64,
    last_position_id: u64,
}

fn plan_cross_position_removal(
    trader: &[u8; 32],
    position_id: u64,
) -> Result<CrossPositionRemoval, u32> {
    let count = load_u64(&cross_position_count_key(trader));
    let index = load_u64(&cross_position_index_key(position_id));
    if count == 0 || count > MAX_CROSS_POSITIONS_PER_ACCOUNT || index == 0 || index > count {
        return Err(13);
    }
    let last_position_id = load_u64(&cross_position_key(trader, count));
    if last_position_id == 0 || load_u64(&cross_position_key(trader, index)) != position_id {
        return Err(13);
    }
    Ok(CrossPositionRemoval {
        count,
        index,
        last_position_id,
    })
}

fn commit_cross_position_removal(
    trader: &[u8; 32],
    position_id: u64,
    plan: &CrossPositionRemoval,
) {
    if plan.index != plan.count {
        save_u64(
            &cross_position_key(trader, plan.index),
            plan.last_position_id,
        );
        save_u64(
            &cross_position_index_key(plan.last_position_id),
            plan.index,
        );
    }
    save_u64(&cross_position_key(trader, plan.count), 0);
    save_u64(&cross_position_index_key(position_id), 0);
    save_u64(&cross_position_count_key(trader), plan.count - 1);
}

struct CrossRealizedPnlPlan {
    next_balance: u64,
    next_insurance: u64,
    bad_debt: u64,
}

fn plan_cross_realized_pnl(
    balance: u64,
    is_profit: bool,
    pnl: u64,
) -> Result<CrossRealizedPnlPlan, u32> {
    let insurance = load_u64(INSURANCE_FUND_KEY);
    if is_profit {
        if pnl > insurance {
            return Err(11);
        }
        Ok(CrossRealizedPnlPlan {
            next_balance: balance.checked_add(pnl).ok_or(12u32)?,
            next_insurance: insurance - pnl,
            bad_debt: 0,
        })
    } else {
        let collected = pnl.min(balance);
        Ok(CrossRealizedPnlPlan {
            next_balance: balance - collected,
            next_insurance: insurance.checked_add(collected).ok_or(12u32)?,
            bad_debt: pnl - collected,
        })
    }
}

fn cross_collateral_totals_after_balance(
    previous_balance: u64,
    next_balance: u64,
) -> Result<(u64, u64), u32> {
    let next_cross_total = load_u64(CROSS_TOTAL_COLLATERAL_KEY)
        .checked_sub(previous_balance)
        .and_then(|value| value.checked_add(next_balance))
        .ok_or(12u32)?;
    let next_escrowed = load_u64(TOTAL_COLLATERAL_ESCROWED_KEY)
        .checked_sub(previous_balance)
        .and_then(|value| value.checked_add(next_balance))
        .ok_or(12u32)?;
    Ok((next_cross_total, next_escrowed))
}

struct CrossDebtCollectionPlan {
    updates: Vec<(u64, u64)>,
    remaining_balance: u64,
    next_pool: u64,
}

fn plan_cross_debt_collection(
    trader: &[u8; 32],
    available_balance: u64,
) -> Result<CrossDebtCollectionPlan, u32> {
    let count = load_u64(&cross_position_count_key(trader));
    if count > MAX_CROSS_POSITIONS_PER_ACCOUNT {
        return Err(13);
    }
    let mut remaining_balance = available_balance;
    let mut total_collected = 0u64;
    let mut updates = Vec::with_capacity(count as usize);
    for index in 1..=count {
        let position_id = load_u64(&cross_position_key(trader, index));
        if position_id == 0 || load_u64(&cross_position_index_key(position_id)) != index {
            return Err(13);
        }
        let debt = load_u64(&position_funding_debt_key(position_id));
        let collected = debt.min(remaining_balance);
        remaining_balance -= collected;
        total_collected = total_collected.checked_add(collected).ok_or(12u32)?;
        updates.push((position_id, debt - collected));
    }
    Ok(CrossDebtCollectionPlan {
        updates,
        remaining_balance,
        next_pool: load_u64(FUNDING_POOL_KEY)
            .checked_add(total_collected)
            .ok_or(12u32)?,
    })
}

fn cross_debt_after_plan(plan: &CrossDebtCollectionPlan, position_id: u64) -> Result<u64, u32> {
    plan.updates
        .iter()
        .find_map(|(id, debt)| (*id == position_id).then_some(*debt))
        .ok_or(13)
}

fn commit_cross_debt_collection(plan: &CrossDebtCollectionPlan) {
    for (position_id, debt) in &plan.updates {
        save_u64(&position_funding_debt_key(*position_id), *debt);
    }
    save_u64(FUNDING_POOL_KEY, plan.next_pool);
}

// ============================================================================
// PUBLIC FUNCTIONS
// ============================================================================

pub fn initialize(admin: *const u8) -> u32 {
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
    if price == 0 {
        return 2;
    }
    // AUDIT-FIX M20: Store price + timestamp for freshness validation
    let mut data = Vec::with_capacity(16);
    data.extend_from_slice(&u64_to_bytes(price));
    data.extend_from_slice(&u64_to_bytes(get_timestamp()));
    storage_set(&mark_price_key(pair_id), &data);
    0
}

/// AUDIT-FIX MARGIN-1: Set oracle contract address for cross-contract price feeds.
/// Admin-only. The oracle must return the canonical fixed8 price, source slot,
/// and decimal count from `get_price_value(asset_bytes)`.
pub fn set_oracle_contract(caller: *const u8, oracle_addr: *const u8) -> u32 {
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
    let mut addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(oracle_addr, addr.as_mut_ptr(), 32);
    }
    if is_zero(&addr) {
        return 2;
    }
    if has_configured_address(ORACLE_ADDRESS_KEY) {
        return 3;
    }
    storage_set(ORACLE_ADDRESS_KEY, &addr);
    log_info("Oracle contract address set");
    0
}

/// Bind a margin pair to its canonical oracle market. Direct USD markets use
/// an asset such as `wSOL`; LICN-quoted cross markets use `wSOL/LICN`.
/// Admin/governance only.
pub fn set_oracle_market(
    caller: *const u8,
    pair_id: u64,
    market_ptr: *const u8,
    market_len: u32,
) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    if get_caller().0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    if pair_id == 0 {
        return 2;
    }
    let market = match read_oracle_market(market_ptr, market_len) {
        Some(market) => market,
        None => return 2,
    };
    storage_set(&oracle_market_key(pair_id), &market);
    0
}

/// AUDIT-FIX MARGIN-1: Update mark price by cross-calling the oracle contract.
/// Anyone can call this as a crank — the oracle validates price freshness internally.
/// Returns: 0=success, 1=no oracle set, 2=oracle call failed,
/// 3=stale/malformed quote, 4=zero/overflow price, 5=unconfigured or mismatched market.
pub fn update_mark_price_from_oracle(
    caller: *const u8,
    pair_id: u64,
    asset_ptr: *const u8,
    asset_len: u32,
) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }

    let oracle_addr = load_addr(ORACLE_ADDRESS_KEY);
    if is_zero(&oracle_addr) {
        return 1; // no oracle contract configured
    }

    let requested_market = match read_oracle_market(asset_ptr, asset_len) {
        Some(market) => market,
        None => return 5,
    };
    let configured_market = match storage_get(&oracle_market_key(pair_id)) {
        Some(market) if market == requested_market => market,
        _ => return 5,
    };
    let (price, source_slot) = match oracle_market_to_margin_price(oracle_addr, &configured_market) {
        Ok(quote) => quote,
        Err(code) => return code,
    };

    let mut data = Vec::with_capacity(16);
    data.extend_from_slice(&u64_to_bytes(price));
    data.extend_from_slice(&u64_to_bytes(source_slot));
    storage_set(&mark_price_key(pair_id), &data);
    log_info("Mark price updated from bound oracle market");
    0
}

/// Set index (spot) price for a pair (called by oracle/analytics)
/// Used together with mark price to calculate funding rates.
pub fn set_index_price(caller: *const u8, pair_id: u64, price: u64) -> u32 {
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
    if price == 0 {
        return 2;
    }
    let mut data = Vec::with_capacity(16);
    data.extend_from_slice(&u64_to_bytes(price));
    data.extend_from_slice(&u64_to_bytes(get_timestamp()));
    storage_set(&index_price_key(pair_id), &data);
    0
}

/// Load index price with timestamp. Returns (price, timestamp).
fn load_index_price(pair_id: u64) -> (u64, u64) {
    match storage_get(&index_price_key(pair_id)) {
        Some(d) if d.len() >= 16 => (bytes_to_u64(&d[..8]), bytes_to_u64(&d[8..16])),
        Some(d) if d.len() >= 8 => (bytes_to_u64(&d[..8]), 0),
        _ => (0, 0),
    }
}

/// Register one legacy open position in the V2 long/short size ledgers. This
/// is permissionless, deterministic, idempotence-protected, and available only
/// before activation. Governance still exclusively controls finalization and
/// activation.
/// Returns: 0=success, 2=missing/not open, 3=already active,
/// 4=already migrated, 5=overflow, 200=caller mismatch.
pub fn migrate_funding_v2_position(caller: *const u8, position_id: u64) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    if get_caller().0 != c {
        return 200;
    }
    if funding_v2_enabled() {
        return 3;
    }
    if load_u64(POSITION_COUNT_KEY) > 0 && !margin_v2_migration_locked() {
        return 6;
    }
    let data = match storage_get(&position_key(position_id)) {
        Some(data) if data.len() >= POSITION_SIZE_V1 && decode_pos_status(&data) == POS_OPEN => data,
        _ => return 2,
    };
    let position_index_key = position_funding_index_key(position_id);
    if storage_get(&position_index_key).is_some() {
        return 4;
    }

    let pair_id = decode_pos_pair_id(&data);
    let size = decode_pos_size(&data);
    let side_size_key = if decode_pos_side(&data) == SIDE_LONG {
        pair_long_size_key(pair_id)
    } else {
        pair_short_size_key(pair_id)
    };
    let next_side_size = match load_u64(&side_size_key).checked_add(size) {
        Some(value) => value,
        None => return 5,
    };
    let next_migrated = match load_u64(FUNDING_MIGRATED_OPEN_COUNT_KEY).checked_add(1) {
        Some(value) => value,
        None => return 5,
    };

    save_u64(&side_size_key, next_side_size);
    save_i128(&position_index_key, 0);
    save_u64(FUNDING_MIGRATED_OPEN_COUNT_KEY, next_migrated);
    0
}

/// Seal the operator-verified legacy-position manifest before activation.
pub fn finalize_funding_v2_migration(caller: *const u8, expected_open_positions: u64) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    if get_caller().0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    if funding_v2_enabled() {
        return 3;
    }
    if load_u64(FUNDING_MIGRATED_OPEN_COUNT_KEY) != expected_open_positions {
        return 2;
    }
    storage_set(FUNDING_MIGRATION_FINALIZED_KEY, &[1]);
    0
}

/// Activate the bounded, pool-backed funding engine at an explicit protocol
/// boundary. Migrated positions start at the zero V2 index, so no legacy
/// interval is charged retroactively.
/// Returns: 0=success, 1=not admin, 2=already active,
/// 5=existing network must use atomic V2 activation, 200=caller mismatch.
pub fn activate_funding_v2(caller: *const u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    if get_caller().0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    if funding_v2_enabled() {
        return 2;
    }
    // Existing networks must activate funding and Cross V2 together through
    // finalize_and_activate_margin_v2. Keeping the legacy single-feature path
    // for empty fresh deployments preserves wire compatibility without
    // allowing a partially active live margin engine.
    if load_u64(POSITION_COUNT_KEY) > 0 {
        return 5;
    }
    storage_set(FUNDING_V2_ENABLED_KEY, &[1]);
    save_u64(FUNDING_V2_ACTIVATION_SLOT_KEY, get_slot());
    save_u64(FUNDING_POOL_KEY, 0);
    save_u64(FUNDING_TOTAL_CLAIMS_KEY, 0);
    save_u64(FUNDING_WRITEOFF_KEY, 0);
    log_info("DEX Margin funding V2 activated");
    0
}

/// Register one legacy open cross position into the bounded shared-collateral
/// registry. Its previously isolated margin becomes shared account collateral;
/// total escrow is unchanged. This deterministic step is permissionless and
/// available only before Cross V2 activation; governance controls activation.
pub fn migrate_cross_v2_position(caller: *const u8, position_id: u64) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    if get_caller().0 != c {
        return 200;
    }
    if cross_v2_enabled() {
        return 3;
    }
    if load_u64(POSITION_COUNT_KEY) > 0 && !margin_v2_migration_locked() {
        return 6;
    }
    if load_u64(&cross_position_migrated_key(position_id)) != 0 {
        return 4;
    }
    let pk = position_key(position_id);
    let mut data = match storage_get(&pk) {
        Some(data)
            if data.len() >= POSITION_SIZE_V1
                && decode_pos_status(&data) == POS_OPEN
                && decode_pos_margin_mode(&data) == MARGIN_MODE_CROSS =>
        {
            data
        }
        _ => return 2,
    };
    let trader = decode_pos_trader(&data);
    let count = load_u64(&cross_position_count_key(&trader));
    if count >= MAX_CROSS_POSITIONS_PER_ACCOUNT {
        return 5;
    }
    let next_count = count + 1;
    let margin = decode_pos_margin(&data);
    let next_balance = match load_u64(&cross_balance_key(&trader)).checked_add(margin) {
        Some(value) => value,
        None => return 5,
    };
    let next_cross_total = match load_u64(CROSS_TOTAL_COLLATERAL_KEY).checked_add(margin) {
        Some(value) => value,
        None => return 5,
    };
    let next_migrated = match load_u64(CROSS_MIGRATED_OPEN_COUNT_KEY).checked_add(1) {
        Some(value) => value,
        None => return 5,
    };

    update_pos_margin(&mut data, 0);
    if bytes_to_u64(&data[90..98]) == 0 {
        data[90..98].copy_from_slice(&(1u64 << 63).to_le_bytes());
    }
    storage_set(&pk, &data);
    save_u64(&cross_balance_key(&trader), next_balance);
    save_u64(CROSS_TOTAL_COLLATERAL_KEY, next_cross_total);
    save_u64(&cross_position_count_key(&trader), next_count);
    save_u64(&cross_position_key(&trader, next_count), position_id);
    save_u64(&cross_position_index_key(position_id), next_count);
    save_u64(&cross_position_migrated_key(position_id), 1);
    save_u64(CROSS_MIGRATED_OPEN_COUNT_KEY, next_migrated);
    0
}

pub fn finalize_cross_v2_migration(caller: *const u8, expected_open_cross: u64) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    if get_caller().0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    if cross_v2_enabled() {
        return 3;
    }
    if load_u64(CROSS_MIGRATED_OPEN_COUNT_KEY) != expected_open_cross {
        return 2;
    }
    storage_set(CROSS_MIGRATION_FINALIZED_KEY, &[1]);
    0
}

/// Activates real bounded shared-collateral cross margin. Existing networks
/// must use the atomic dual-engine activation boundary instead.
pub fn activate_cross_v2(caller: *const u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    if get_caller().0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    if cross_v2_enabled() {
        return 2;
    }
    if load_u64(POSITION_COUNT_KEY) > 0 {
        return 5;
    }
    if !funding_v2_enabled() {
        return 4;
    }
    storage_set(CROSS_V2_ENABLED_KEY, &[1]);
    log_info("DEX Margin Cross V2 activated");
    0
}

/// Atomically seal both migration manifests and activate Funding V2 plus real
/// Cross Margin V2. Existing networks must be paused before this protocol
/// boundary. Every check occurs before the first write, so a count mismatch or
/// authority failure leaves both engines disabled and both manifests unsealed.
///
/// Returns: 0=success, 1=not admin, 2=already/partially active,
/// 3=funding migration count mismatch, 4=cross migration count mismatch,
/// 5=cross count exceeds total open count, 6=live network not paused,
/// 7=live-network migration lock absent, 200=caller mismatch.
pub fn finalize_and_activate_margin_v2(
    caller: *const u8,
    expected_open_positions: u64,
    expected_open_cross: u64,
) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    if get_caller().0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    if funding_v2_enabled() || cross_v2_enabled() {
        return 2;
    }
    if load_u64(FUNDING_MIGRATED_OPEN_COUNT_KEY) != expected_open_positions {
        return 3;
    }
    if load_u64(CROSS_MIGRATED_OPEN_COUNT_KEY) != expected_open_cross {
        return 4;
    }
    if expected_open_cross > expected_open_positions {
        return 5;
    }
    if load_u64(POSITION_COUNT_KEY) > 0 && !is_paused() {
        return 6;
    }
    if load_u64(POSITION_COUNT_KEY) > 0 && !margin_v2_migration_locked() {
        return 7;
    }

    storage_set(FUNDING_MIGRATION_FINALIZED_KEY, &[1]);
    storage_set(CROSS_MIGRATION_FINALIZED_KEY, &[1]);
    storage_set(FUNDING_V2_ENABLED_KEY, &[1]);
    save_u64(FUNDING_V2_ACTIVATION_SLOT_KEY, get_slot());
    save_u64(FUNDING_POOL_KEY, 0);
    save_u64(FUNDING_TOTAL_CLAIMS_KEY, 0);
    save_u64(FUNDING_WRITEOFF_KEY, 0);
    storage_set(CROSS_V2_ENABLED_KEY, &[1]);
    log_info("DEX Margin Funding V2 and Cross V2 activated atomically");
    0
}

/// Freeze all user position mutations before taking the legacy migration
/// manifest. Oracle refreshes and read-only queries remain available. The lock
/// remains through activation and is cleared only when governance completes
/// the operator-verified migration and atomically reopens trading.
pub fn begin_margin_v2_migration(caller: *const u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    if get_caller().0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    if funding_v2_enabled() || cross_v2_enabled() {
        return 2;
    }
    if margin_v2_migration_locked() {
        return 3;
    }
    storage_set(PAUSED_KEY, &[1]);
    storage_set(MARGIN_V2_MIGRATION_LOCK_KEY, &[1]);
    log_info("DEX Margin V2 migration lock enabled");
    0
}

/// Complete an operator-verified live-network migration by clearing the
/// migration lock and reopening margin in one governance action. This is the
/// only path that can clear the migration lock.
pub fn complete_margin_v2_migration(caller: *const u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    if get_caller().0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    if !margin_v2_migration_locked() {
        return 2;
    }
    if !funding_v2_enabled() || !cross_v2_enabled() {
        return 3;
    }
    if storage_get(FUNDING_MIGRATION_FINALIZED_KEY)
        .and_then(|data| data.first().copied())
        != Some(1)
        || storage_get(CROSS_MIGRATION_FINALIZED_KEY)
            .and_then(|data| data.first().copied())
            != Some(1)
    {
        return 4;
    }
    if !is_paused() {
        return 5;
    }
    storage_set(MARGIN_V2_MIGRATION_LOCK_KEY, &[0]);
    storage_set(PAUSED_KEY, &[0]);
    log_info("DEX Margin V2 migration verified and trading reopened");
    0
}

/// Advance a pair's global funding indexes in constant work. Position
/// settlement is lazy and O(1): payer funds enter FUNDING_POOL_KEY before any
/// receiver collateral or withdrawal can be credited.
///
/// Returns: 0=applied, 1=too early, 2=no fresh prices,
/// 4=funding V2 inactive, 5=invalid pair/index overflow.
pub fn apply_funding(pair_id: u64) -> u32 {
    if !funding_v2_enabled() {
        return 4;
    }
    if pair_id == 0 {
        return 5;
    }

    let current_slot = get_slot();
    let v2_last_key = funding_v2_last_slot_key(pair_id);
    let stored_last_slot = storage_get(&v2_last_key);
    let last_slot = stored_last_slot
        .as_ref()
        .filter(|data| data.len() >= 8)
        .map(|data| bytes_to_u64(data))
        .unwrap_or_else(|| load_u64(FUNDING_V2_ACTIVATION_SLOT_KEY));
    if current_slot < last_slot || current_slot - last_slot < FUNDING_INTERVAL_SLOTS {
        return 1;
    }

    let mark = fresh_mark_price(pair_id);
    let index = fresh_index_price(pair_id);
    if mark == 0 || index == 0 {
        return 2;
    }

    // Premium aligns the synthetic mark with the oracle index. A bounded skew
    // component also works when both prices are oracle-mirrored: one-sided
    // positioning pays the underrepresented side instead of producing a
    // permanently zero rate.
    let premium_positive = mark >= index;
    let premium_abs_bps = if premium_positive {
        (mark - index) as u128 * 10_000 / index as u128
    } else {
        (index - mark) as u128 * 10_000 / index as u128
    };
    let premium_abs_bps = premium_abs_bps
        .min((MAX_FUNDING_RATE_BPS + MAX_SKEW_FUNDING_BPS) as u128)
        as i128;
    let premium_bps = if premium_positive {
        premium_abs_bps
    } else {
        -premium_abs_bps
    };

    let long_size = load_u64(&pair_long_size_key(pair_id));
    let short_size = load_u64(&pair_short_size_key(pair_id));
    let total_size = match long_size.checked_add(short_size) {
        Some(value) => value,
        None => return 5,
    };
    let skew_bps = if total_size == 0 || long_size == short_size {
        0
    } else {
        let skew_abs = long_size.abs_diff(short_size) as u128 * MAX_SKEW_FUNDING_BPS as u128
            / total_size as u128;
        if long_size > short_size {
            skew_abs as i128
        } else {
            -(skew_abs as i128)
        }
    };
    let net_rate_bps = (premium_bps + skew_bps)
        .clamp(-(MAX_FUNDING_RATE_BPS as i128), MAX_FUNDING_RATE_BPS as i128);
    let rate_positive = net_rate_bps > 0;
    let clamped_bps = net_rate_bps.unsigned_abs() as u64;

    // If rate is zero, advance the interval without creating obligations.
    if clamped_bps == 0 {
        save_u64(&v2_last_key, current_slot);
        return 0;
    }

    let mut next_indexes = [0i128; FUNDING_TIER_MULTIPLIERS.len()];
    for (tier, multiplier) in FUNDING_TIER_MULTIPLIERS.iter().enumerate() {
        let unit_delta = mark as u128 * clamped_bps as u128 * *multiplier as u128
            / FUNDING_RATE_DENOMINATOR;
        let unit_delta = match i128::try_from(unit_delta) {
            Ok(value) => value,
            Err(_) => return 5,
        };
        let current = load_i128(&funding_index_key(pair_id, tier as u8));
        next_indexes[tier] = match if rate_positive {
            current.checked_add(unit_delta)
        } else {
            current.checked_sub(unit_delta)
        } {
            Some(value) => value,
            None => return 5,
        };
    }

    for (tier, index) in next_indexes.iter().enumerate() {
        save_i128(&funding_index_key(pair_id, tier as u8), *index);
    }
    save_u64(&v2_last_key, current_slot);

    let mut result = Vec::with_capacity(40);
    result.extend_from_slice(&u64_to_bytes(clamped_bps));
    result.extend_from_slice(&u64_to_bytes(rate_positive as u64));
    result.extend_from_slice(&u64_to_bytes(current_slot));
    result.extend_from_slice(&(premium_bps as i64).to_le_bytes());
    result.extend_from_slice(&(skew_bps as i64).to_le_bytes());
    lichen_sdk::set_return_data(&result);
    log_info("Funding indexes advanced");
    0
}

/// Permissionless O(1) crank for one open position.
/// Returns: 0=settled, 1=missing/not open, 2=arithmetic invariant failure,
/// 4=funding V2 inactive.
pub fn settle_position_funding(position_id: u64) -> u32 {
    let pk = position_key(position_id);
    let mut data = match storage_get(&pk) {
        Some(data) if data.len() >= POSITION_SIZE_V1 => data,
        _ => return 1,
    };
    match settle_position_funding_state(position_id, &mut data) {
        Ok((collected, credited, debt, claim)) => {
            let mut result = Vec::with_capacity(32);
            result.extend_from_slice(&u64_to_bytes(collected));
            result.extend_from_slice(&u64_to_bytes(credited));
            result.extend_from_slice(&u64_to_bytes(debt));
            result.extend_from_slice(&u64_to_bytes(claim));
            lichen_sdk::set_return_data(&result);
            0
        }
        Err(code) => code,
    }
}

/// Withdraw a funded user entitlement. Unfunded claims remain recorded until
/// payer settlement supplies the pool.
/// Returns: 0=paid, 1=no claim, 2=pool empty, 3=transfer/invariant failure,
/// 4=reentrancy or inactive funding, 200=caller mismatch.
pub fn claim_funding(caller: *const u8) -> u32 {
    if !funding_v2_enabled() || !reentrancy_enter() {
        return 4;
    }
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    if get_caller().0 != c {
        reentrancy_exit();
        return 200;
    }

    let claim_key = user_funding_claim_key(&c);
    let claim = load_u64(&claim_key);
    if claim == 0 {
        reentrancy_exit();
        return 1;
    }
    let pool = load_u64(FUNDING_POOL_KEY);
    if pool == 0 {
        reentrancy_exit();
        return 2;
    }
    let amount = claim.min(pool);
    let total_claims = load_u64(FUNDING_TOTAL_CLAIMS_KEY);
    let next_total_claims = match total_claims.checked_sub(amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 3;
        }
    };
    if !collateral_out(&c, amount) {
        reentrancy_exit();
        return 3;
    }

    save_u64(&claim_key, claim - amount);
    save_u64(FUNDING_POOL_KEY, pool - amount);
    save_u64(FUNDING_TOTAL_CLAIMS_KEY, next_total_claims);
    lichen_sdk::set_return_data(&u64_to_bytes(amount));
    reentrancy_exit();
    0
}

/// Deposit lUSD into the caller's shared cross-margin account. Existing cross
/// positions are funding-settled first so the reported balance is current.
pub fn deposit_cross_collateral(caller: *const u8, amount: u64) -> u32 {
    if !reentrancy_enter() {
        return 4;
    }
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    if get_caller().0 != c {
        reentrancy_exit();
        return 200;
    }
    if !cross_v2_enabled() {
        reentrancy_exit();
        return 13;
    }
    if amount == 0 {
        reentrancy_exit();
        return 2;
    }
    if settle_cross_portfolio_funding(&c).is_err() {
        reentrancy_exit();
        return 13;
    }
    let next_balance = match load_u64(&cross_balance_key(&c)).checked_add(amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 5;
        }
    };
    let next_cross_total = match load_u64(CROSS_TOTAL_COLLATERAL_KEY).checked_add(amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 5;
        }
    };
    let next_escrowed = match load_u64(TOTAL_COLLATERAL_ESCROWED_KEY).checked_add(amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 5;
        }
    };
    if !collateral_in(&c, amount) {
        reentrancy_exit();
        return 3;
    }
    save_u64(&cross_balance_key(&c), next_balance);
    save_u64(CROSS_TOTAL_COLLATERAL_KEY, next_cross_total);
    save_u64(TOTAL_COLLATERAL_ESCROWED_KEY, next_escrowed);
    if settle_cross_portfolio_funding(&c).is_err() {
        reentrancy_exit();
        return 13;
    }
    lichen_sdk::set_return_data(&u64_to_bytes(load_u64(&cross_balance_key(&c))));
    reentrancy_exit();
    0
}

/// Withdraw only real account collateral while retaining the weighted initial
/// margin required by every remaining cross position at fresh marks.
pub fn withdraw_cross_collateral(caller: *const u8, amount: u64) -> u32 {
    if !reentrancy_enter() {
        return 4;
    }
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    if get_caller().0 != c {
        reentrancy_exit();
        return 200;
    }
    if !cross_v2_enabled() {
        reentrancy_exit();
        return 13;
    }
    if amount == 0 {
        reentrancy_exit();
        return 2;
    }
    if settle_cross_portfolio_funding(&c).is_err() {
        reentrancy_exit();
        return 13;
    }
    let metrics = match cross_portfolio_metrics(&c) {
        Ok(value) => value,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };
    if amount > metrics.balance
        || amount > metrics.equity
        || metrics.equity - amount < metrics.initial_required
    {
        reentrancy_exit();
        return 3;
    }
    let next_balance = metrics.balance - amount;
    let next_cross_total = match load_u64(CROSS_TOTAL_COLLATERAL_KEY).checked_sub(amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 5;
        }
    };
    let next_escrowed = match load_u64(TOTAL_COLLATERAL_ESCROWED_KEY).checked_sub(amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 5;
        }
    };
    if !collateral_out(&c, amount) {
        reentrancy_exit();
        return 5;
    }
    save_u64(&cross_balance_key(&c), next_balance);
    save_u64(CROSS_TOTAL_COLLATERAL_KEY, next_cross_total);
    save_u64(TOTAL_COLLATERAL_ESCROWED_KEY, next_escrowed);
    lichen_sdk::set_return_data(&u64_to_bytes(next_balance));
    reentrancy_exit();
    0
}

/// Returns balance, active-position count, equity, notional, weighted initial
/// requirement, weighted maintenance requirement, outstanding funding debt,
/// negative-equity deficit, and status (0=valid).
pub fn get_cross_account(user: *const u8) -> u32 {
    let mut trader = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(user, trader.as_mut_ptr(), 32);
    }
    let metrics = cross_portfolio_metrics(&trader);
    let (
        balance,
        count,
        equity,
        notional,
        initial,
        maintenance,
        funding_debt,
        equity_deficit,
        status,
    ) = match metrics {
        Ok(value) => (
            value.balance,
            value.position_count,
            value.equity,
            value.total_notional,
            value.initial_required,
            value.maintenance_required,
            value.funding_debt,
            if value.signed_equity < 0 {
                u64::try_from(value.signed_equity.unsigned_abs()).unwrap_or(u64::MAX)
            } else {
                0
            },
            0,
        ),
        Err(code) => (
            load_u64(&cross_balance_key(&trader)),
            load_u64(&cross_position_count_key(&trader)),
            0,
            0,
            0,
            0,
            0,
            0,
            code as u64,
        ),
    };
    let mut result = Vec::with_capacity(72);
    for value in [
        balance,
        count,
        equity,
        notional,
        initial,
        maintenance,
        funding_debt,
        equity_deficit,
        status,
    ] {
        result.extend_from_slice(&u64_to_bytes(value));
    }
    lichen_sdk::set_return_data(&result);
    0
}

/// Enable margin trading on a pair (admin only)
/// Returns: 0=success, 1=not admin
pub fn enable_margin_pair(caller: *const u8, pair_id: u64) -> u32 {
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
    save_u64(&margin_enabled_key(pair_id), 1);
    log_info("Margin pair enabled");
    0
}

/// Disable margin trading on a pair (admin only)
/// Returns: 0=success, 1=not admin
pub fn disable_margin_pair(caller: *const u8, pair_id: u64) -> u32 {
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
    save_u64(&margin_enabled_key(pair_id), 0);
    log_info("Margin pair disabled");
    0
}

/// Check if margin is enabled for a pair
pub fn is_margin_enabled(pair_id: u64) -> u64 {
    load_u64(&margin_enabled_key(pair_id))
}

/// Open a new margin position
/// Returns: 0=success, 1=paused, 2=invalid leverage, 3=insufficient margin,
///          4=max positions, 5=reentrancy, 6=no mark price, 7=pair not margin-enabled
pub fn open_position(
    trader: *const u8,
    pair_id: u64,
    side: u8,
    size: u64,
    leverage: u64,
    margin_amount: u64,
) -> u32 {
    open_position_with_mode(
        trader,
        pair_id,
        side,
        size,
        leverage,
        margin_amount,
        MARGIN_MODE_ISOLATED,
    )
}

/// Open a new margin position with explicit margin mode.
/// Returns: 0=success, 1=paused, 2=invalid leverage, 3=insufficient margin,
///          4=max positions, 5=reentrancy, 6=no mark price, 7=pair not margin-enabled,
///          8=collateral escrow failed, 9=invalid margin mode/cap,
///          11=insufficient insurance liquidity, 12=funding V2 inactive
pub fn open_position_with_mode(
    trader: *const u8,
    pair_id: u64,
    side: u8,
    size: u64,
    leverage: u64,
    margin_amount: u64,
    margin_mode: u8,
) -> u32 {
    if !reentrancy_enter() {
        return 5;
    }
    if !require_not_paused() {
        reentrancy_exit();
        return 1;
    }
    if !funding_v2_enabled() {
        reentrancy_exit();
        return 12;
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

    // Check pair is enabled for margin
    if load_u64(&margin_enabled_key(pair_id)) != 1 {
        reentrancy_exit();
        return 7;
    }

    if margin_mode != MARGIN_MODE_ISOLATED && margin_mode != MARGIN_MODE_CROSS {
        reentrancy_exit();
        return 9;
    }
    if margin_mode == MARGIN_MODE_CROSS && !cross_v2_enabled() {
        reentrancy_exit();
        return 13;
    }

    // Validate leverage
    let max_lev = load_u64(&max_leverage_key(pair_id));
    let effective_max = if margin_mode == MARGIN_MODE_CROSS {
        let pair_cap = if max_lev > 0 {
            max_lev
        } else {
            MAX_LEVERAGE_CROSS
        };
        core::cmp::min(pair_cap, MAX_LEVERAGE_CROSS)
    } else if max_lev > 0 {
        max_lev
    } else {
        MAX_LEVERAGE_ISOLATED
    };
    if leverage == 0 || leverage > effective_max {
        reentrancy_exit();
        return 2;
    }
    if side > SIDE_SHORT {
        reentrancy_exit();
        return 2;
    }

    // AUDIT-FIX M20: Get mark price with freshness check
    let mark_price = fresh_mark_price(pair_id);
    if mark_price == 0 {
        reentrancy_exit();
        return 6;
    }

    if size == 0 {
        reentrancy_exit();
        return 2;
    }

    // Check initial margin (tiered by leverage)
    let notional = match calculate_notional(size, mark_price) {
        Some(n) if n > 0 => n,
        _ => {
            reentrancy_exit();
            return 9;
        }
    };
    let (initial_margin_bps, _maint_bps, _liq_penalty_bps, _funding_mult) =
        get_tier_params(leverage);
    // AUDIT-FIX NEW-H2: initial_margin_bps already factors in leverage via the tier table
    // (e.g. 10x → 1000 bps = 10%). Do NOT divide by leverage again — that was double-discounting.
    let required_margin = (notional as u128 * initial_margin_bps as u128 / 10_000).max(1) as u64;
    let cross_plan = if margin_mode == MARGIN_MODE_CROSS {
        if settle_cross_portfolio_funding(&t).is_err() {
            reentrancy_exit();
            return 13;
        }
        let metrics = match cross_portfolio_metrics(&t) {
            Ok(value) => value,
            Err(code) => {
                reentrancy_exit();
                return code;
            }
        };
        if metrics.position_count >= MAX_CROSS_POSITIONS_PER_ACCOUNT {
            reentrancy_exit();
            return 4;
        }
        let next_balance = match metrics.balance.checked_add(margin_amount) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 9;
            }
        };
        let projected_signed_equity = match metrics.signed_equity.checked_add(margin_amount as i128) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 9;
            }
        };
        let projected_initial = match metrics.initial_required.checked_add(required_margin) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 9;
            }
        };
        if projected_signed_equity < projected_initial as i128 {
            reentrancy_exit();
            return 3;
        }
        let next_cross_total = match load_u64(CROSS_TOTAL_COLLATERAL_KEY)
            .checked_add(margin_amount)
        {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 9;
            }
        };
        Some((
            next_balance,
            metrics.position_count + 1,
            next_cross_total,
        ))
    } else {
        if margin_amount < required_margin {
            reentrancy_exit();
            return 3;
        }
        None
    };

    // AUDIT-FIX H-11: Reject if total open interest would exceed cap
    let current_oi = load_u64(TOTAL_OPEN_INTEREST_KEY);
    let projected_oi = match current_oi.checked_add(notional) {
        Some(value) if value <= MAX_TOTAL_OPEN_INTEREST => value,
        _ => {
            log_info("Total open interest cap exceeded");
            reentrancy_exit();
            return 9;
        }
    };
    let insurance_required =
        (projected_oi as u128 * MIN_INSURANCE_COVERAGE_BPS as u128 / 10_000) as u64;
    if load_u64(INSURANCE_FUND_KEY) < insurance_required {
        log_info("Insufficient margin insurance liquidity");
        reentrancy_exit();
        return 11;
    }

    let pos_count = load_u64(POSITION_COUNT_KEY);
    if pos_count >= MAX_POSITIONS {
        reentrancy_exit();
        return 4;
    }

    let pos_id = pos_count + 1;
    let slot = get_slot();
    let user_count = load_u64(&user_position_count_key(&t));
    let next_user_count = match user_count.checked_add(1) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 9;
        }
    };
    let next_volume = match load_u64(TOTAL_VOLUME_KEY).checked_add(notional) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 9;
        }
    };
    let next_escrowed = match load_u64(TOTAL_COLLATERAL_ESCROWED_KEY).checked_add(margin_amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 9;
        }
    };
    let side_size_key = if side == SIDE_LONG {
        pair_long_size_key(pair_id)
    } else {
        pair_short_size_key(pair_id)
    };
    let next_side_size = match load_u64(&side_size_key).checked_add(size) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 9;
        }
    };

    // New margin positions use standard MT-20 lUSD custody. The frontend must
    // approve dex_margin first; this call pulls the exact margin into escrow.
    if !escrow_lusd_collateral_in(&t, margin_amount) {
        log_info("Margin collateral escrow failed");
        reentrancy_exit();
        return 8;
    }

    let data = encode_position(
        &t,
        pos_id,
        pair_id,
        side,
        POS_OPEN,
        size,
        if margin_mode == MARGIN_MODE_CROSS {
            0
        } else {
            margin_amount
        },
        mark_price,
        leverage,
        slot,
        1u64 << 63,
        0,
        margin_mode,
        COLLATERAL_LUSD,
    );
    storage_set(&position_key(pos_id), &data);
    save_i128(
        &position_funding_index_key(pos_id),
        current_funding_index(pair_id, leverage),
    );
    save_u64(POSITION_COUNT_KEY, pos_id);
    save_u64(TOTAL_COLLATERAL_ESCROWED_KEY, next_escrowed);
    if let Some((next_balance, next_cross_count, next_cross_total)) = cross_plan {
        save_u64(&cross_balance_key(&t), next_balance);
        save_u64(&cross_position_count_key(&t), next_cross_count);
        save_u64(&cross_position_key(&t, next_cross_count), pos_id);
        save_u64(&cross_position_index_key(pos_id), next_cross_count);
        save_u64(CROSS_TOTAL_COLLATERAL_KEY, next_cross_total);
    }

    // Track user positions
    save_u64(&user_position_count_key(&t), next_user_count);
    save_u64(&user_position_key(&t, next_user_count), pos_id);

    save_u64(TOTAL_VOLUME_KEY, next_volume);

    // AUDIT-FIX H-11: Track total open interest
    save_u64(TOTAL_OPEN_INTEREST_KEY, projected_oi);
    save_u64(&side_size_key, next_side_size);

    if margin_mode == MARGIN_MODE_CROSS && settle_cross_portfolio_funding(&t).is_err() {
        reentrancy_exit();
        return 13;
    }

    log_info("Margin position opened");
    reentrancy_exit();
    0
}

/// Open a margin position only if the fresh mark satisfies the requested limit.
/// Long entries execute when mark_price <= limit_price; short entries execute
/// when mark_price >= limit_price.
/// Returns open_position_with_mode codes plus 10=limit condition not met/invalid.
pub fn open_position_limit_with_mode(
    trader: *const u8,
    pair_id: u64,
    side: u8,
    size: u64,
    leverage: u64,
    margin_amount: u64,
    margin_mode: u8,
    limit_price: u64,
) -> u32 {
    if limit_price == 0 {
        return 10;
    }
    if side > SIDE_SHORT {
        return 2;
    }
    let mark_price = fresh_mark_price(pair_id);
    if mark_price == 0 {
        return 6;
    }
    let limit_ok = if side == SIDE_LONG {
        mark_price <= limit_price
    } else {
        mark_price >= limit_price
    };
    if !limit_ok {
        return 10;
    }
    open_position_with_mode(
        trader,
        pair_id,
        side,
        size,
        leverage,
        margin_amount,
        margin_mode,
    )
}

fn close_cross_position_state(
    trader: &[u8; 32],
    position_id: u64,
    mark_price: u64,
) -> u32 {
    if settle_cross_portfolio_funding(trader).is_err() {
        return 13;
    }
    let pk = position_key(position_id);
    let mut data = match storage_get(&pk) {
        Some(value) if value.len() >= POSITION_SIZE_V1 => value,
        _ => return 13,
    };
    if decode_pos_status(&data) != POS_OPEN
        || decode_pos_margin_mode(&data) != MARGIN_MODE_CROSS
        || decode_pos_trader(&data) != *trader
    {
        return 13;
    }
    let size = decode_pos_size(&data);
    let side = decode_pos_side(&data);
    let pair_id = decode_pos_pair_id(&data);
    let entry_price = decode_pos_entry_price(&data);
    let (is_profit, pnl) = match calculate_pnl(side, size, entry_price, mark_price) {
        Some(value) => value,
        None => return 12,
    };
    let previous_balance = load_u64(&cross_balance_key(trader));
    let pnl_plan = match plan_cross_realized_pnl(previous_balance, is_profit, pnl) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let debt_plan = match plan_cross_debt_collection(trader, pnl_plan.next_balance) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let selected_writeoff = match cross_debt_after_plan(&debt_plan, position_id) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let next_funding_writeoff = match load_u64(FUNDING_WRITEOFF_KEY).checked_add(selected_writeoff)
    {
        Some(value) => value,
        None => return 12,
    };
    let final_balance = debt_plan.remaining_balance;
    let (next_cross_total, next_escrowed) =
        match cross_collateral_totals_after_balance(previous_balance, final_balance) {
            Ok(value) => value,
            Err(code) => return code,
        };
    let next_pnl_total = match if is_profit {
        load_u64(TOTAL_PNL_PROFIT_KEY).checked_add(pnl)
    } else {
        load_u64(TOTAL_PNL_LOSS_KEY).checked_add(pnl)
    } {
        Some(value) => value,
        None => return 12,
    };
    let next_bad_debt = match load_u64(BAD_DEBT_KEY).checked_add(pnl_plan.bad_debt) {
        Some(value) => value,
        None => return 12,
    };
    let close_notional = match calculate_notional(size, entry_price) {
        Some(value) => value,
        None => return 12,
    };
    let next_open_interest = match load_u64(TOTAL_OPEN_INTEREST_KEY).checked_sub(close_notional) {
        Some(value) => value,
        None => return 12,
    };
    let side_size_key = if side == SIDE_LONG {
        pair_long_size_key(pair_id)
    } else {
        pair_short_size_key(pair_id)
    };
    let next_side_size = match load_u64(&side_size_key).checked_sub(size) {
        Some(value) => value,
        None => return 12,
    };
    let removal = match plan_cross_position_removal(trader, position_id) {
        Ok(value) => value,
        Err(code) => return code,
    };

    let pnl_biased = if is_profit {
        (1u64 << 63).saturating_add(pnl)
    } else {
        (1u64 << 63).saturating_sub(pnl)
    };
    data[90..98].copy_from_slice(&pnl_biased.to_le_bytes());
    update_pos_status(&mut data, POS_CLOSED);
    update_pos_margin(&mut data, 0);
    storage_set(&pk, &data);
    save_u64(&cross_balance_key(trader), final_balance);
    save_u64(CROSS_TOTAL_COLLATERAL_KEY, next_cross_total);
    save_u64(TOTAL_COLLATERAL_ESCROWED_KEY, next_escrowed);
    save_u64(INSURANCE_FUND_KEY, pnl_plan.next_insurance);
    if is_profit {
        save_u64(TOTAL_PNL_PROFIT_KEY, next_pnl_total);
    } else {
        save_u64(TOTAL_PNL_LOSS_KEY, next_pnl_total);
    }
    save_u64(BAD_DEBT_KEY, next_bad_debt);
    save_u64(TOTAL_OPEN_INTEREST_KEY, next_open_interest);
    save_u64(&side_size_key, next_side_size);
    commit_cross_debt_collection(&debt_plan);
    save_u64(&position_funding_debt_key(position_id), 0);
    save_u64(FUNDING_WRITEOFF_KEY, next_funding_writeoff);
    commit_cross_position_removal(trader, position_id, &removal);
    lichen_sdk::set_return_data(&u64_to_bytes(final_balance));
    0
}

/// Close a margin position
/// Returns: 0=success, 1=not found, 2=not owner, 3=already closed, 4=reentrancy,
///          5=oracle unavailable (price stale or missing)
pub fn close_position(caller: *const u8, position_id: u64) -> u32 {
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

    let pk = position_key(position_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE_V1 => d,
        _ => {
            reentrancy_exit();
            return 1;
        }
    };

    let trader = decode_pos_trader(&data);
    if trader != c {
        reentrancy_exit();
        return 2;
    }
    if decode_pos_status(&data) != POS_OPEN {
        reentrancy_exit();
        return 3;
    }

    let pair_id = decode_pos_pair_id(&data);
    // AUDIT-FIX M20: Use freshness-checked mark price
    let mark_price = fresh_mark_price(pair_id);

    // SECURITY FIX G6-03: Reject close when oracle price is unavailable or stale.
    // Previously returned full margin (no PnL deduction), allowing traders to
    // escape losses during oracle outages.
    if mark_price == 0 {
        log_info("Cannot close position: oracle price unavailable or stale");
        reentrancy_exit();
        return 5;
    }

    if decode_pos_margin_mode(&data) == MARGIN_MODE_CROSS {
        let result = close_cross_position_state(&c, position_id, mark_price);
        reentrancy_exit();
        return result;
    }

    if settle_position_funding_state(position_id, &mut data).is_err() {
        log_info("close_position: funding settlement failed");
        reentrancy_exit();
        return 12;
    }

    let margin = decode_pos_margin(&data);
    let size = decode_pos_size(&data);
    let side = decode_pos_side(&data);
    let entry_price = decode_pos_entry_price(&data);

    // Calculate PnL and determine unlock amount
    let (is_profit, pnl) = match calculate_pnl(side, size, entry_price, mark_price) {
        Some(value) => value,
        None => {
            log_info("close_position: PnL overflow");
            reentrancy_exit();
            return 12;
        }
    };
    // F10.2-B FIX: Write realized PnL to position data
    // Store as biased u64: value = PNL_BIAS + signed_pnl
    let pnl_biased = if is_profit {
        (1u64 << 63).saturating_add(pnl)
    } else {
        (1u64 << 63).saturating_sub(pnl)
    };
    data[90..98].copy_from_slice(&pnl_biased.to_le_bytes());
    let insurance = load_u64(INSURANCE_FUND_KEY);
    let (gross_unlock_amount, next_insurance) = if is_profit {
        if pnl > insurance {
            log_info("close_position: insufficient insurance liquidity for profit");
            reentrancy_exit();
            return 11;
        }
        let payout = match margin.checked_add(pnl) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 12;
            }
        };
        (payout, insurance - pnl)
    } else {
        let next_insurance = match insurance.checked_add(pnl.min(margin)) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 12;
            }
        };
        (margin.saturating_sub(pnl), next_insurance)
    };
    let funding_exit = match funding_exit_plan(position_id, gross_unlock_amount) {
        Ok(plan) => plan,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };
    let next_pnl_total = match if is_profit {
        load_u64(TOTAL_PNL_PROFIT_KEY).checked_add(pnl)
    } else {
        load_u64(TOTAL_PNL_LOSS_KEY).checked_add(pnl)
    } {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let bad_debt = if is_profit {
        0
    } else {
        pnl.saturating_sub(margin)
    };
    let next_bad_debt = match load_u64(BAD_DEBT_KEY).checked_add(bad_debt) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let next_escrowed = match load_u64(TOTAL_COLLATERAL_ESCROWED_KEY).checked_sub(margin) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let close_notional = match calculate_notional(size, entry_price) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let next_open_interest = match load_u64(TOTAL_OPEN_INTEREST_KEY).checked_sub(close_notional) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let side_size_key = if side == SIDE_LONG {
        pair_long_size_key(pair_id)
    } else {
        pair_short_size_key(pair_id)
    };
    let next_side_size = match load_u64(&side_size_key).checked_sub(size) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };

    // Pay collateral back before mutating position status.
    if !collateral_out(&trader, funding_exit.payout) {
        log_info("close_position: collateral release failed");
        reentrancy_exit();
        return 10;
    }

    if is_profit {
        save_u64(TOTAL_PNL_PROFIT_KEY, next_pnl_total);
    } else {
        save_u64(TOTAL_PNL_LOSS_KEY, next_pnl_total);
    }
    save_u64(INSURANCE_FUND_KEY, next_insurance);
    save_u64(BAD_DEBT_KEY, next_bad_debt);
    save_u64(TOTAL_COLLATERAL_ESCROWED_KEY, next_escrowed);
    commit_funding_exit(position_id, &funding_exit);

    update_pos_status(&mut data, POS_CLOSED);
    storage_set(&pk, &data);

    // AUDIT-FIX H-11: Decrement total open interest on close
    save_u64(TOTAL_OPEN_INTEREST_KEY, next_open_interest);
    save_u64(&side_size_key, next_side_size);

    lichen_sdk::set_return_data(&u64_to_bytes(funding_exit.payout));
    log_info("Margin position closed");
    reentrancy_exit();
    0
}

/// Close a margin position with a limit price guard.
/// Long positions can close only when mark_price >= limit_price.
/// Short positions can close only when mark_price <= limit_price.
/// Returns: 0=success, 1=not found, 2=not owner, 3=already closed, 4=reentrancy,
///          5=oracle unavailable (price stale or missing), 6=limit condition not met/invalid.
pub fn close_position_limit(caller: *const u8, position_id: u64, limit_price: u64) -> u32 {
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

    if limit_price == 0 {
        reentrancy_exit();
        return 6;
    }

    let pk = position_key(position_id);
    let data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE_V1 => d,
        _ => {
            reentrancy_exit();
            return 1;
        }
    };

    let trader = decode_pos_trader(&data);
    if trader != c {
        reentrancy_exit();
        return 2;
    }
    if decode_pos_status(&data) != POS_OPEN {
        reentrancy_exit();
        return 3;
    }

    let pair_id = decode_pos_pair_id(&data);
    let side = decode_pos_side(&data);
    let mark_price = fresh_mark_price(pair_id);
    if mark_price == 0 {
        reentrancy_exit();
        return 5;
    }

    let limit_ok = if side == SIDE_LONG {
        mark_price >= limit_price
    } else {
        mark_price <= limit_price
    };

    if !limit_ok {
        reentrancy_exit();
        return 6;
    }

    reentrancy_exit();
    close_position(caller, position_id)
}

/// Partially close a margin position with a limit price guard.
/// Long positions can close only when mark_price >= limit_price.
/// Short positions can close only when mark_price <= limit_price.
/// If close_amount >= current position size, delegates to full limit-close.
/// Returns: 0=success, 1=not found, 2=not owner, 3=already closed, 4=reentrancy,
///          5=oracle unavailable (price stale or missing), 6=invalid input/limit condition not met.
pub fn partial_close_limit(
    caller: *const u8,
    position_id: u64,
    close_amount: u64,
    limit_price: u64,
) -> u32 {
    if !reentrancy_enter() {
        return 4;
    }
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }

    let real_caller = get_caller();
    if real_caller.0 != c {
        reentrancy_exit();
        return 200;
    }

    if close_amount == 0 || limit_price == 0 {
        reentrancy_exit();
        return 6;
    }

    let pk = position_key(position_id);
    let data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE_V1 => d,
        _ => {
            reentrancy_exit();
            return 1;
        }
    };

    let trader = decode_pos_trader(&data);
    if trader != c {
        reentrancy_exit();
        return 2;
    }
    if decode_pos_status(&data) != POS_OPEN {
        reentrancy_exit();
        return 3;
    }

    let pair_id = decode_pos_pair_id(&data);
    let side = decode_pos_side(&data);
    let size = decode_pos_size(&data);
    let mark_price = fresh_mark_price(pair_id);
    if mark_price == 0 {
        reentrancy_exit();
        return 5;
    }

    let limit_ok = if side == SIDE_LONG {
        mark_price >= limit_price
    } else {
        mark_price <= limit_price
    };

    if !limit_ok {
        reentrancy_exit();
        return 6;
    }

    reentrancy_exit();
    if close_amount >= size {
        close_position(caller, position_id)
    } else {
        partial_close(caller, position_id, close_amount)
    }
}

/// Add margin to a position
pub fn add_margin(caller: *const u8, position_id: u64, amount: u64) -> u32 {
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

    let pk = position_key(position_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE_V1 => d,
        _ => {
            reentrancy_exit();
            return 1;
        }
    };
    if decode_pos_trader(&data) != c {
        reentrancy_exit();
        return 2;
    }
    if decode_pos_status(&data) != POS_OPEN {
        reentrancy_exit();
        return 3;
    }
    if amount == 0 {
        reentrancy_exit();
        return 5;
    }

    if decode_pos_margin_mode(&data) == MARGIN_MODE_CROSS {
        if !cross_v2_enabled() || settle_cross_portfolio_funding(&c).is_err() {
            reentrancy_exit();
            return 13;
        }
        let next_balance = match load_u64(&cross_balance_key(&c)).checked_add(amount) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 6;
            }
        };
        let next_cross_total = match load_u64(CROSS_TOTAL_COLLATERAL_KEY).checked_add(amount) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 8;
            }
        };
        let next_escrowed = match load_u64(TOTAL_COLLATERAL_ESCROWED_KEY).checked_add(amount) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 8;
            }
        };
        if !collateral_in(&c, amount) {
            reentrancy_exit();
            return 7;
        }
        save_u64(&cross_balance_key(&c), next_balance);
        save_u64(CROSS_TOTAL_COLLATERAL_KEY, next_cross_total);
        save_u64(TOTAL_COLLATERAL_ESCROWED_KEY, next_escrowed);
        if settle_cross_portfolio_funding(&c).is_err() {
            reentrancy_exit();
            return 13;
        }
        reentrancy_exit();
        return 0;
    }

    if settle_position_funding_state(position_id, &mut data).is_err() {
        reentrancy_exit();
        return 8;
    }

    let current_margin = decode_pos_margin(&data);
    let gross_margin = match current_margin.checked_add(amount) {
        Some(m) => m,
        None => {
            reentrancy_exit();
            return 6;
        } // overflow
    };
    let debt_key = position_funding_debt_key(position_id);
    let debt = load_u64(&debt_key);
    let debt_collected = debt.min(gross_margin);
    let new_margin = gross_margin - debt_collected;
    let next_pool = match load_u64(FUNDING_POOL_KEY).checked_add(debt_collected) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 8;
        }
    };
    let next_escrowed = match load_u64(TOTAL_COLLATERAL_ESCROWED_KEY)
        .checked_add(amount)
        .and_then(|value| value.checked_sub(debt_collected))
    {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 8;
        }
    };

    if !collateral_in(&c, amount) {
        log_info("Collateral escrow failed on add_margin");
        reentrancy_exit();
        return 7;
    }

    update_pos_margin(&mut data, new_margin);
    storage_set(&pk, &data);
    save_u64(&debt_key, debt - debt_collected);
    save_u64(FUNDING_POOL_KEY, next_pool);
    save_u64(TOTAL_COLLATERAL_ESCROWED_KEY, next_escrowed);
    reentrancy_exit();
    0
}

/// Remove margin from a position (if still healthy)
pub fn remove_margin(caller: *const u8, position_id: u64, amount: u64) -> u32 {
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

    let pk = position_key(position_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE_V1 => d,
        _ => {
            reentrancy_exit();
            return 1;
        }
    };
    if decode_pos_trader(&data) != c {
        reentrancy_exit();
        return 2;
    }
    if decode_pos_status(&data) != POS_OPEN {
        reentrancy_exit();
        return 3;
    }

    if decode_pos_margin_mode(&data) == MARGIN_MODE_CROSS {
        reentrancy_exit();
        return withdraw_cross_collateral(caller, amount);
    }

    if settle_position_funding_state(position_id, &mut data).is_err() {
        reentrancy_exit();
        return 9;
    }

    let current_margin = decode_pos_margin(&data);
    if amount > current_margin {
        reentrancy_exit();
        return 5;
    }
    let new_margin = current_margin - amount;

    // Check if still above maintenance (tiered by leverage)
    let size = decode_pos_size(&data);
    let pair_id = decode_pos_pair_id(&data);
    let leverage = decode_pos_leverage(&data);
    // AUDIT-FIX M20: Freshness-checked mark price for margin health
    let mark_price = fresh_mark_price(pair_id);
    // SECURITY FIX G6-03: Reject margin removal when oracle is stale
    if mark_price == 0 {
        log_info("Cannot remove margin: oracle price unavailable or stale");
        reentrancy_exit();
        return 7;
    }
    let side = decode_pos_side(&data);
    let entry_price = decode_pos_entry_price(&data);
    // F10.2-A FIX: Use PnL-aware margin ratio for health check
    let ratio = calculate_margin_ratio_with_pnl(new_margin, size, entry_price, mark_price, side);
    let (_init_bps, maint_bps, _liq_bps, _fund_mult) = get_tier_params(leverage);
    // Use admin-overridden maintenance if set and higher than tier
    let admin_maint = get_maintenance_margin_override();
    let effective_maint = if admin_maint > maint_bps {
        admin_maint
    } else {
        maint_bps
    };
    if ratio < effective_maint {
        reentrancy_exit();
        return 6;
    } // would be unhealthy
    let next_escrowed = match load_u64(TOTAL_COLLATERAL_ESCROWED_KEY).checked_sub(amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 9;
        }
    };

    if !collateral_out(&c, amount) {
        log_info("remove_margin: collateral release failed");
        reentrancy_exit();
        return 8;
    }

    update_pos_margin(&mut data, new_margin);
    storage_set(&pk, &data);
    save_u64(TOTAL_COLLATERAL_ESCROWED_KEY, next_escrowed);
    reentrancy_exit();
    0
}

/// Liquidate an unhealthy position
/// Returns: 0=success, 1=not found, 2=not liquidatable, 3=reentrancy
fn liquidate_cross_position_state(
    liquidator: &[u8; 32],
    position_id: u64,
    mark_price: u64,
) -> u32 {
    let initial_data = match storage_get(&position_key(position_id)) {
        Some(value) if value.len() >= POSITION_SIZE_V1 => value,
        _ => return 13,
    };
    let trader = decode_pos_trader(&initial_data);
    if settle_cross_portfolio_funding(&trader).is_err() {
        return 13;
    }
    let pk = position_key(position_id);
    let mut data = match storage_get(&pk) {
        Some(value) if value.len() >= POSITION_SIZE_V1 => value,
        _ => return 13,
    };
    if decode_pos_status(&data) != POS_OPEN
        || decode_pos_margin_mode(&data) != MARGIN_MODE_CROSS
        || decode_pos_trader(&data) != trader
    {
        return 13;
    }
    let metrics = match cross_portfolio_metrics(&trader) {
        Ok(value) => value,
        Err(code) => return code,
    };
    if metrics.equity >= metrics.maintenance_required {
        return 2;
    }

    let size = decode_pos_size(&data);
    let leverage = decode_pos_leverage(&data);
    let side = decode_pos_side(&data);
    let pair_id = decode_pos_pair_id(&data);
    let entry_price = decode_pos_entry_price(&data);
    let (is_profit, pnl) = match calculate_pnl(side, size, entry_price, mark_price) {
        Some(value) => value,
        None => return 12,
    };
    let previous_balance = metrics.balance;
    let pnl_plan = match plan_cross_realized_pnl(previous_balance, is_profit, pnl) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let debt_plan = match plan_cross_debt_collection(&trader, pnl_plan.next_balance) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let selected_writeoff = match cross_debt_after_plan(&debt_plan, position_id) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let next_funding_writeoff = match load_u64(FUNDING_WRITEOFF_KEY).checked_add(selected_writeoff)
    {
        Some(value) => value,
        None => return 12,
    };
    let notional = match calculate_notional(size, mark_price) {
        Some(value) => value,
        None => return 12,
    };
    let open_notional = match calculate_notional(size, entry_price) {
        Some(value) => value,
        None => return 12,
    };
    let (_, _, liquidation_penalty_bps, _) = get_tier_params(leverage);
    let penalty =
        u64::try_from(notional as u128 * liquidation_penalty_bps as u128 / 10_000)
            .unwrap_or(u64::MAX);
    let penalty_taken = penalty.min(debt_plan.remaining_balance);
    let liquidator_reward = u64::try_from(
        penalty_taken as u128 * LIQUIDATOR_SHARE_BPS as u128 / 10_000,
    )
    .unwrap_or(u64::MAX);
    let insurance_penalty = penalty_taken - liquidator_reward;
    let final_balance = debt_plan.remaining_balance - penalty_taken;
    let insurance_on_reward_success = match pnl_plan.next_insurance.checked_add(insurance_penalty) {
        Some(value) => value,
        None => return 12,
    };
    let insurance_on_reward_failure = match pnl_plan.next_insurance.checked_add(penalty_taken) {
        Some(value) => value,
        None => return 12,
    };
    let (next_cross_total, next_escrowed) =
        match cross_collateral_totals_after_balance(previous_balance, final_balance) {
            Ok(value) => value,
            Err(code) => return code,
        };
    let next_pnl_total = match if is_profit {
        load_u64(TOTAL_PNL_PROFIT_KEY).checked_add(pnl)
    } else {
        load_u64(TOTAL_PNL_LOSS_KEY).checked_add(pnl)
    } {
        Some(value) => value,
        None => return 12,
    };
    let next_bad_debt = match load_u64(BAD_DEBT_KEY).checked_add(pnl_plan.bad_debt) {
        Some(value) => value,
        None => return 12,
    };
    let next_open_interest = match load_u64(TOTAL_OPEN_INTEREST_KEY).checked_sub(open_notional) {
        Some(value) => value,
        None => return 12,
    };
    let side_size_key = if side == SIDE_LONG {
        pair_long_size_key(pair_id)
    } else {
        pair_short_size_key(pair_id)
    };
    let next_side_size = match load_u64(&side_size_key).checked_sub(size) {
        Some(value) => value,
        None => return 12,
    };
    let next_liquidation_count = match load_u64(LIQUIDATION_COUNT_KEY).checked_add(1) {
        Some(value) => value,
        None => return 12,
    };
    let removal = match plan_cross_position_removal(&trader, position_id) {
        Ok(value) => value,
        Err(code) => return code,
    };

    let reward_paid = liquidator_reward == 0
        || pay_liquidator_reward(liquidator, liquidator_reward);
    let pnl_biased = if is_profit {
        (1u64 << 63).saturating_add(pnl)
    } else {
        (1u64 << 63).saturating_sub(pnl)
    };
    data[90..98].copy_from_slice(&pnl_biased.to_le_bytes());
    update_pos_status(&mut data, POS_LIQUIDATED);
    update_pos_margin(&mut data, 0);
    storage_set(&pk, &data);
    save_u64(&cross_balance_key(&trader), final_balance);
    save_u64(CROSS_TOTAL_COLLATERAL_KEY, next_cross_total);
    save_u64(TOTAL_COLLATERAL_ESCROWED_KEY, next_escrowed);
    save_u64(
        INSURANCE_FUND_KEY,
        if reward_paid {
            insurance_on_reward_success
        } else {
            insurance_on_reward_failure
        },
    );
    if is_profit {
        save_u64(TOTAL_PNL_PROFIT_KEY, next_pnl_total);
    } else {
        save_u64(TOTAL_PNL_LOSS_KEY, next_pnl_total);
    }
    save_u64(BAD_DEBT_KEY, next_bad_debt);
    save_u64(TOTAL_OPEN_INTEREST_KEY, next_open_interest);
    save_u64(&side_size_key, next_side_size);
    save_u64(LIQUIDATION_COUNT_KEY, next_liquidation_count);
    commit_cross_debt_collection(&debt_plan);
    save_u64(&position_funding_debt_key(position_id), 0);
    save_u64(FUNDING_WRITEOFF_KEY, next_funding_writeoff);
    commit_cross_position_removal(&trader, position_id, &removal);
    lichen_sdk::set_return_data(&u64_to_bytes(if reward_paid {
        liquidator_reward
    } else {
        0
    }));
    0
}

pub fn liquidate(_liquidator: *const u8, position_id: u64) -> u32 {
    if !reentrancy_enter() {
        return 3;
    }

    let mut liq = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(_liquidator, liq.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != liq {
        reentrancy_exit();
        return 200;
    }

    let pk = position_key(position_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE_V1 => d,
        _ => {
            reentrancy_exit();
            return 1;
        }
    };

    if decode_pos_status(&data) != POS_OPEN {
        reentrancy_exit();
        return 2;
    }

    let pair_id = decode_pos_pair_id(&data);
    // AUDIT-FIX M20: Freshness-checked liquidation price. If the mark price is
    // stale or absent, fall back to a fresh index price so unhealthy positions
    // can still be liquidated instead of remaining stuck indefinitely.
    let mark_price = match fresh_mark_price(pair_id) {
        0 => {
            let index_price = fresh_index_price(pair_id);
            if index_price > 0 {
                log_info("DEX Margin: Falling back to index price for liquidation");
            }
            index_price
        }
        price => price,
    };
    if mark_price == 0 {
        reentrancy_exit();
        return 2;
    }

    if decode_pos_margin_mode(&data) == MARGIN_MODE_CROSS {
        let result = liquidate_cross_position_state(&liq, position_id, mark_price);
        reentrancy_exit();
        return result;
    }

    if settle_position_funding_state(position_id, &mut data).is_err() {
        reentrancy_exit();
        return 12;
    }

    let margin = decode_pos_margin(&data);
    let size = decode_pos_size(&data);
    let leverage = decode_pos_leverage(&data);
    let side = decode_pos_side(&data);
    let entry_price = decode_pos_entry_price(&data);

    // F10.2-A FIX: Use PnL-aware margin ratio for liquidation check
    let ratio = calculate_margin_ratio_with_pnl(margin, size, entry_price, mark_price, side);
    let (_init_bps, maint_bps, liq_penalty_bps, _fund_mult) = get_tier_params(leverage);
    // Use admin-overridden maintenance if set and higher than tier
    let admin_maint = get_maintenance_margin_override();
    let effective_maint = if admin_maint > maint_bps {
        admin_maint
    } else {
        maint_bps
    };
    if ratio >= effective_maint {
        reentrancy_exit();
        return 2;
    } // still healthy

    // Realize PnL before applying a liquidation penalty. The old path used PnL
    // only for the health check, then returned margin minus penalty to the
    // trader. That allowed an underwater account to recover collateral that
    // had already been lost economically.
    let (is_profit, pnl) = match calculate_pnl(side, size, entry_price, mark_price) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let pnl_biased = if is_profit {
        (1u64 << 63).saturating_add(pnl)
    } else {
        (1u64 << 63).saturating_sub(pnl)
    };
    data[90..98].copy_from_slice(&pnl_biased.to_le_bytes());

    let insurance = load_u64(INSURANCE_FUND_KEY);
    let (equity_after_pnl, insurance_after_pnl, bad_debt) = if is_profit {
        if pnl > insurance {
            log_info("liquidate: insufficient insurance liquidity for profit");
            reentrancy_exit();
            return 11;
        }
        let equity = match margin.checked_add(pnl) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 12;
            }
        };
        (equity, insurance - pnl, 0)
    } else {
        let collected_loss = pnl.min(margin);
        let next_insurance = match insurance.checked_add(collected_loss) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 12;
            }
        };
        (
            margin - collected_loss,
            next_insurance,
            pnl - collected_loss,
        )
    };
    let funding_exit = match funding_exit_plan(position_id, equity_after_pnl) {
        Ok(plan) => plan,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };

    // Calculate the tiered penalty only from funded equity left after PnL and
    // funding debt. A penalty cannot manufacture a liquidator reward from bad
    // debt or from collateral already consumed by trading losses.
    let notional = match calculate_notional(size, mark_price) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let open_notional = match calculate_notional(size, entry_price) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let penalty = (notional as u128 * liq_penalty_bps as u128 / 10_000) as u64;
    let penalty_taken = penalty.min(funding_exit.payout);
    let liquidator_reward = (penalty_taken as u128 * LIQUIDATOR_SHARE_BPS as u128 / 10_000) as u64;
    let insurance_add = penalty_taken.saturating_sub(liquidator_reward);

    let trader = decode_pos_trader(&data);
    let trader_payout = funding_exit.payout - penalty_taken;
    let insurance_on_reward_success = match insurance_after_pnl.checked_add(insurance_add) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let insurance_on_reward_failure = match insurance_after_pnl.checked_add(penalty_taken) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let next_pnl_total = match if is_profit {
        load_u64(TOTAL_PNL_PROFIT_KEY).checked_add(pnl)
    } else {
        load_u64(TOTAL_PNL_LOSS_KEY).checked_add(pnl)
    } {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let next_bad_debt = match load_u64(BAD_DEBT_KEY).checked_add(bad_debt) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let next_escrowed = match load_u64(TOTAL_COLLATERAL_ESCROWED_KEY).checked_sub(margin) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let next_open_interest = match load_u64(TOTAL_OPEN_INTEREST_KEY).checked_sub(open_notional) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let next_liquidation_count = match load_u64(LIQUIDATION_COUNT_KEY).checked_add(1) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let side_size_key = if side == SIDE_LONG {
        pair_long_size_key(pair_id)
    } else {
        pair_short_size_key(pair_id)
    };
    let next_side_size = match load_u64(&side_size_key).checked_sub(size) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    if trader_payout > 0 && !collateral_out(&trader, trader_payout) {
        log_info("liquidate: collateral release failed");
        reentrancy_exit();
        return 10;
    }

    let reward_paid = liquidator_reward == 0 || pay_liquidator_reward(&liq, liquidator_reward);
    if !reward_paid {
        log_info("liquidate: reward transfer failed, crediting to insurance");
    }
    save_u64(
        INSURANCE_FUND_KEY,
        if reward_paid {
            insurance_on_reward_success
        } else {
            insurance_on_reward_failure
        },
    );
    if is_profit {
        save_u64(TOTAL_PNL_PROFIT_KEY, next_pnl_total);
    } else {
        save_u64(TOTAL_PNL_LOSS_KEY, next_pnl_total);
    }
    save_u64(BAD_DEBT_KEY, next_bad_debt);
    save_u64(TOTAL_COLLATERAL_ESCROWED_KEY, next_escrowed);
    commit_funding_exit(position_id, &funding_exit);

    update_pos_status(&mut data, POS_LIQUIDATED);
    storage_set(&pk, &data);

    // AUDIT-FIX H-11: Decrement total open interest on liquidation
    save_u64(TOTAL_OPEN_INTEREST_KEY, next_open_interest);
    save_u64(&side_size_key, next_side_size);

    // Track liquidation count
    save_u64(LIQUIDATION_COUNT_KEY, next_liquidation_count);

    lichen_sdk::set_return_data(&u64_to_bytes(if reward_paid {
        liquidator_reward
    } else {
        0
    }));
    log_info("Position liquidated");
    reentrancy_exit();
    0
}

/// Set max leverage for a pair (admin)
pub fn set_max_leverage(caller: *const u8, pair_id: u64, max_leverage: u64) -> u32 {
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
    if max_leverage == 0 || max_leverage > 100 {
        return 2;
    }
    save_u64(&max_leverage_key(pair_id), max_leverage);
    0
}

/// Set maintenance margin in basis points (admin only)
/// Default is 1000 (10%). Min 200 (2%), Max 5000 (50%).
/// Acts as a floor override that applies when higher than tier default.
pub fn set_maintenance_margin(caller: *const u8, margin_bps: u64) -> u32 {
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
    if margin_bps < 200 || margin_bps > 5000 {
        return 2;
    }
    save_u64(&maintenance_margin_key_fn(), margin_bps);
    0
}

/// Set the standard margin collateral token address (lUSD). Admin only.
pub fn set_collateral_token_address(caller: *const u8, addr: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut a = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(addr, a.as_mut_ptr(), 32);
    }

    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    if is_zero(&a) {
        return 2;
    }
    if has_configured_address(COLLATERAL_TOKEN_ADDRESS_KEY) {
        return 3;
    }
    storage_set(COLLATERAL_TOKEN_ADDRESS_KEY, &a);
    0
}

/// Set this contract's own address for runtimes/tests that cannot provide it.
pub fn set_self_address(caller: *const u8, addr: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut a = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(addr, a.as_mut_ptr(), 32);
    }

    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    if is_zero(&a) {
        return 2;
    }
    if has_configured_address(SELF_ADDRESS_KEY) {
        return 3;
    }
    storage_set(SELF_ADDRESS_KEY, &a);
    0
}

/// Deposit lUSD into the margin insurance/settlement fund.
/// Deposits are permissionless because they only increase the settlement pool;
/// withdrawals remain admin/governance-only.
/// The caller must approve dex_margin for `amount` first.
/// Returns: 0=success, 2=zero amount, 3=transfer failed, 4=reentrancy,
///          5=insurance accounting overflow,
///          200=caller mismatch
pub fn deposit_insurance(caller: *const u8, amount: u64) -> u32 {
    if !reentrancy_enter() {
        return 4;
    }

    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }

    let real_caller = get_caller();
    if real_caller.0 != c {
        reentrancy_exit();
        return 200;
    }
    if amount == 0 {
        reentrancy_exit();
        return 2;
    }
    let next_insurance = match load_u64(INSURANCE_FUND_KEY).checked_add(amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 5;
        }
    };
    if !escrow_lusd_collateral_in(&c, amount) {
        reentrancy_exit();
        return 3;
    }

    save_u64(INSURANCE_FUND_KEY, next_insurance);
    lichen_sdk::set_return_data(&u64_to_bytes(amount));
    reentrancy_exit();
    0
}

/// Withdraw from the insurance fund (admin/governance only)
/// Returns: 0=success, 1=not admin, 2=zero amount, 3=insufficient funds,
///          4=invalid/unconfigured recipient or collateral, 5=transfer failed,
///          6=reentrancy, 200=caller mismatch
pub fn withdraw_insurance(caller: *const u8, amount: u64, recipient: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 6;
    }
    let mut c = [0u8; 32];
    let mut r = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(recipient, r.as_mut_ptr(), 32);
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
    if amount == 0 {
        reentrancy_exit();
        return 2;
    }

    let insurance = load_u64(INSURANCE_FUND_KEY);
    if amount > insurance {
        reentrancy_exit();
        return 3;
    }
    let remaining = insurance - amount;
    let required_coverage = match u64::try_from(
        load_u64(TOTAL_OPEN_INTEREST_KEY) as u128 * MIN_INSURANCE_COVERAGE_BPS as u128 / 10_000,
    ) {
        Ok(value) => value,
        Err(_) => {
            reentrancy_exit();
            return 3;
        }
    };
    if remaining < required_coverage {
        reentrancy_exit();
        return 3;
    }

    if is_zero(&r) || !has_configured_address(COLLATERAL_TOKEN_ADDRESS_KEY) {
        reentrancy_exit();
        return 4;
    }

    if transfer_lusd_collateral_out(&r, amount) {
        save_u64(INSURANCE_FUND_KEY, remaining);
        log_info("Insurance fund withdrawal");
        lichen_sdk::set_return_data(&u64_to_bytes(amount));
        reentrancy_exit();
        0
    } else {
        reentrancy_exit();
        5
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
    lichen_sdk::set_return_data(&result);
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
    if admin_override > tier_maint {
        admin_override
    } else {
        tier_maint
    }
}

/// Get margin ratio for a position (in bps)
pub fn get_margin_ratio(position_id: u64) -> u64 {
    let pk = position_key(position_id);
    let data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE_V1 => d,
        _ => return 0,
    };
    if decode_pos_margin_mode(&data) == MARGIN_MODE_CROSS {
        let trader = decode_pos_trader(&data);
        return match cross_portfolio_metrics(&trader) {
            Ok(metrics) if metrics.total_notional == 0 => 10_000,
            Ok(metrics) => {
                (metrics.equity as u128 * 10_000 / metrics.total_notional as u128) as u64
            }
            Err(_) => 0,
        };
    }
    let margin = decode_pos_margin(&data);
    let size = decode_pos_size(&data);
    let pair_id = decode_pos_pair_id(&data);
    let side = decode_pos_side(&data);
    let entry_price = decode_pos_entry_price(&data);
    // AUDIT-FIX M20: Freshness-checked mark price for ratio query
    let mark_price = fresh_mark_price(pair_id);
    if mark_price == 0 {
        return 0;
    }
    // F10.2-A FIX: Use PnL-aware ratio
    calculate_margin_ratio_with_pnl(margin, size, entry_price, mark_price, side)
}

pub fn get_position_count() -> u64 {
    load_u64(POSITION_COUNT_KEY)
}
pub fn get_insurance_fund() -> u64 {
    load_u64(INSURANCE_FUND_KEY)
}

/// Cumulative realized loss that could not be collected from position
/// collateral. This value is never included in insurance-fund assets.
pub fn get_bad_debt() -> u64 {
    load_u64(BAD_DEBT_KEY)
}

/// Return user and global funding solvency state as five u64 values:
/// enabled, user claim, pool assets, total claims, cumulative debt write-offs.
pub fn get_funding_state(user: *const u8) -> u64 {
    if user.is_null() {
        return 0;
    }
    let mut address = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(user, address.as_mut_ptr(), 32);
    }
    let claim = load_u64(&user_funding_claim_key(&address));
    let mut result = Vec::with_capacity(40);
    result.extend_from_slice(&u64_to_bytes(funding_v2_enabled() as u64));
    result.extend_from_slice(&u64_to_bytes(claim));
    result.extend_from_slice(&u64_to_bytes(load_u64(FUNDING_POOL_KEY)));
    result.extend_from_slice(&u64_to_bytes(load_u64(FUNDING_TOTAL_CLAIMS_KEY)));
    result.extend_from_slice(&u64_to_bytes(load_u64(FUNDING_WRITEOFF_KEY)));
    lichen_sdk::set_return_data(&result);
    claim
}

pub fn get_position_info(position_id: u64) -> u64 {
    let pk = position_key(position_id);
    match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE_V1 => {
            lichen_sdk::set_return_data(&d);
            position_id
        }
        _ => 0,
    }
}

/// Query a user's first open position on a given pair.
/// Returns position_id if found (with full position data in return_data),
/// or 0 if the user has no open position on that pair.
/// Used by dex_core for reduce-only order validation.
pub fn query_user_open_position(trader: *const u8, pair_id: u64) -> u64 {
    let mut addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(trader, addr.as_mut_ptr(), 32);
    }

    let count = load_u64(&user_position_count_key(&addr));
    for i in 1..=count {
        let pos_id = load_u64(&user_position_key(&addr, i));
        if pos_id == 0 {
            continue;
        }
        let pk = position_key(pos_id);
        if let Some(data) = storage_get(&pk) {
            if data.len() >= POSITION_SIZE_V1 {
                let pos_pair = decode_pos_pair_id(&data);
                let pos_status = decode_pos_status(&data);
                if pos_pair == pair_id && pos_status == 0 {
                    // Found an open position on this pair — return data
                    lichen_sdk::set_return_data(&data);
                    return pos_id;
                }
            }
        }
    }
    0
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
    log_info("DEX Margin: EMERGENCY PAUSE");
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
// STOP-LOSS / TAKE-PROFIT ON MARGIN POSITIONS
// ============================================================================

fn partial_close_cross_position_state(
    trader: &[u8; 32],
    position_id: u64,
    close_amount: u64,
    mark_price: u64,
) -> u32 {
    if settle_cross_portfolio_funding(trader).is_err() {
        return 13;
    }
    let pk = position_key(position_id);
    let mut data = match storage_get(&pk) {
        Some(value) if value.len() >= POSITION_SIZE_V1 => value,
        _ => return 13,
    };
    let size = decode_pos_size(&data);
    if decode_pos_status(&data) != POS_OPEN
        || decode_pos_margin_mode(&data) != MARGIN_MODE_CROSS
        || decode_pos_trader(&data) != *trader
        || close_amount == 0
        || close_amount >= size
    {
        return 13;
    }
    let side = decode_pos_side(&data);
    let pair_id = decode_pos_pair_id(&data);
    let entry_price = decode_pos_entry_price(&data);
    let (is_profit, pnl) = match calculate_pnl(side, close_amount, entry_price, mark_price) {
        Some(value) => value,
        None => return 12,
    };
    let previous_balance = load_u64(&cross_balance_key(trader));
    let pnl_plan = match plan_cross_realized_pnl(previous_balance, is_profit, pnl) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let debt_plan = match plan_cross_debt_collection(trader, pnl_plan.next_balance) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let final_balance = debt_plan.remaining_balance;
    let (next_cross_total, next_escrowed) =
        match cross_collateral_totals_after_balance(previous_balance, final_balance) {
            Ok(value) => value,
            Err(code) => return code,
        };
    let next_pnl_total = match if is_profit {
        load_u64(TOTAL_PNL_PROFIT_KEY).checked_add(pnl)
    } else {
        load_u64(TOTAL_PNL_LOSS_KEY).checked_add(pnl)
    } {
        Some(value) => value,
        None => return 12,
    };
    let next_bad_debt = match load_u64(BAD_DEBT_KEY).checked_add(pnl_plan.bad_debt) {
        Some(value) => value,
        None => return 12,
    };
    let closed_notional = match calculate_notional(close_amount, entry_price) {
        Some(value) => value,
        None => return 12,
    };
    let next_open_interest = match load_u64(TOTAL_OPEN_INTEREST_KEY).checked_sub(closed_notional) {
        Some(value) => value,
        None => return 12,
    };
    let side_size_key = if side == SIDE_LONG {
        pair_long_size_key(pair_id)
    } else {
        pair_short_size_key(pair_id)
    };
    let next_side_size = match load_u64(&side_size_key).checked_sub(close_amount) {
        Some(value) => value,
        None => return 12,
    };

    let stored_pnl = bytes_to_u64(&data[90..98]);
    let existing_pnl = if stored_pnl == 0 {
        1u64 << 63
    } else {
        stored_pnl
    };
    let next_realized = if is_profit {
        existing_pnl.saturating_add(pnl)
    } else {
        existing_pnl.saturating_sub(pnl)
    };
    data[90..98].copy_from_slice(&next_realized.to_le_bytes());
    update_pos_size(&mut data, size - close_amount);
    update_pos_margin(&mut data, 0);
    storage_set(&pk, &data);
    save_u64(&cross_balance_key(trader), final_balance);
    save_u64(CROSS_TOTAL_COLLATERAL_KEY, next_cross_total);
    save_u64(TOTAL_COLLATERAL_ESCROWED_KEY, next_escrowed);
    save_u64(INSURANCE_FUND_KEY, pnl_plan.next_insurance);
    if is_profit {
        save_u64(TOTAL_PNL_PROFIT_KEY, next_pnl_total);
    } else {
        save_u64(TOTAL_PNL_LOSS_KEY, next_pnl_total);
    }
    save_u64(BAD_DEBT_KEY, next_bad_debt);
    save_u64(TOTAL_OPEN_INTEREST_KEY, next_open_interest);
    save_u64(&side_size_key, next_side_size);
    commit_cross_debt_collection(&debt_plan);
    lichen_sdk::set_return_data(&u64_to_bytes(final_balance));
    0
}

/// Partially close a margin position
/// Closes `close_amount` of the position's size, settles proportional PnL,
/// reduces margin proportionally, and keeps the remainder open.
/// If close_amount >= position size, delegates to full close.
/// Returns: 0=success, 1=not found, 2=not owner, 3=not open, 4=reentrancy,
///          5=zero close amount
pub fn partial_close(caller: *const u8, position_id: u64, close_amount: u64) -> u32 {
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

    if close_amount == 0 {
        reentrancy_exit();
        return 5;
    }

    let pk = position_key(position_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE_V1 => d,
        _ => {
            reentrancy_exit();
            return 1;
        }
    };

    let trader = decode_pos_trader(&data);
    if trader != c {
        reentrancy_exit();
        return 2;
    }
    if decode_pos_status(&data) != POS_OPEN {
        reentrancy_exit();
        return 3;
    }

    let size = decode_pos_size(&data);
    let pair_id = decode_pos_pair_id(&data);
    let side = decode_pos_side(&data);
    let entry_price = decode_pos_entry_price(&data);

    // If closing the full size or more, do a full close
    if close_amount >= size {
        reentrancy_exit(); // release before calling close_position which re-enters
        return close_position(caller, position_id);
    }

    let mark_price = fresh_mark_price(pair_id);

    // SECURITY FIX G6-03: Reject partial close when oracle is stale
    if mark_price == 0 {
        log_info("Cannot partial close: oracle price unavailable or stale");
        reentrancy_exit();
        return 5;
    }

    if decode_pos_margin_mode(&data) == MARGIN_MODE_CROSS {
        let result = partial_close_cross_position_state(&c, position_id, close_amount, mark_price);
        reentrancy_exit();
        return result;
    }

    if settle_position_funding_state(position_id, &mut data).is_err() {
        reentrancy_exit();
        return 12;
    }

    let size = decode_pos_size(&data);
    let margin = decode_pos_margin(&data);

    // Calculate proportional close fraction
    // proportional_margin = margin * close_amount / size
    let proportional_margin = (margin as u128 * close_amount as u128 / size as u128) as u64;
    let remaining_margin = margin.saturating_sub(proportional_margin);
    let remaining_size = size - close_amount; // safe since close_amount < size

    // Calculate PnL on the closed portion
    let (is_profit, pnl) = match calculate_pnl(side, close_amount, entry_price, mark_price) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };

    // Write proportional realized PnL to position
    let existing_pnl_biased = if data.len() >= 98 {
        let stored = bytes_to_u64(&data[90..98]);
        if stored == 0 {
            1u64 << 63
        } else {
            stored
        }
    } else {
        1u64 << 63
    };
    // Accumulate: add the new partial PnL to existing realized PnL
    let new_pnl_biased = if is_profit {
        existing_pnl_biased.saturating_add(pnl)
    } else {
        existing_pnl_biased.saturating_sub(pnl)
    };
    while data.len() < POSITION_SIZE {
        data.push(0);
    }
    data[90..98].copy_from_slice(&new_pnl_biased.to_le_bytes());

    let insurance = load_u64(INSURANCE_FUND_KEY);
    let (gross_unlock_amount, next_insurance) = if is_profit {
        if pnl > insurance {
            log_info("partial_close: insufficient insurance liquidity for profit");
            reentrancy_exit();
            return 11;
        }
        let payout = match proportional_margin.checked_add(pnl) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 12;
            }
        };
        (payout, insurance - pnl)
    } else {
        let next_insurance = match insurance.checked_add(pnl.min(proportional_margin)) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 12;
            }
        };
        (proportional_margin.saturating_sub(pnl), next_insurance)
    };
    let funding_payout = match funding_payout_plan(position_id, gross_unlock_amount) {
        Ok(plan) => plan,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };
    let next_pnl_total = match if is_profit {
        load_u64(TOTAL_PNL_PROFIT_KEY).checked_add(pnl)
    } else {
        load_u64(TOTAL_PNL_LOSS_KEY).checked_add(pnl)
    } {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let bad_debt = if is_profit {
        0
    } else {
        pnl.saturating_sub(proportional_margin)
    };
    let next_bad_debt = match load_u64(BAD_DEBT_KEY).checked_add(bad_debt) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let next_escrowed = match load_u64(TOTAL_COLLATERAL_ESCROWED_KEY)
        .checked_sub(proportional_margin)
    {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let closed_notional = match calculate_notional(close_amount, entry_price) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let next_open_interest = match load_u64(TOTAL_OPEN_INTEREST_KEY).checked_sub(closed_notional) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let side_size_key = if side == SIDE_LONG {
        pair_long_size_key(pair_id)
    } else {
        pair_short_size_key(pair_id)
    };
    let next_side_size = match load_u64(&side_size_key).checked_sub(close_amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };

    // Release proportional collateral before mutating position.
    if !collateral_out(&trader, funding_payout.payout) {
        log_info("partial_close: collateral release failed");
        reentrancy_exit();
        return 10;
    }

    if is_profit {
        save_u64(TOTAL_PNL_PROFIT_KEY, next_pnl_total);
    } else {
        save_u64(TOTAL_PNL_LOSS_KEY, next_pnl_total);
    }
    save_u64(INSURANCE_FUND_KEY, next_insurance);
    save_u64(BAD_DEBT_KEY, next_bad_debt);
    save_u64(TOTAL_COLLATERAL_ESCROWED_KEY, next_escrowed);
    commit_funding_payout(position_id, &funding_payout);

    // Update position in-place: reduce size and margin, keep it open
    update_pos_size(&mut data, remaining_size);
    update_pos_margin(&mut data, remaining_margin);
    storage_set(&pk, &data);

    // AUDIT-FIX H-11: Decrement OI for the closed portion
    save_u64(TOTAL_OPEN_INTEREST_KEY, next_open_interest);
    save_u64(&side_size_key, next_side_size);

    lichen_sdk::set_return_data(&u64_to_bytes(funding_payout.payout));
    log_info("Margin position partially closed");
    reentrancy_exit();
    0
}

/// Set or update the stop-loss and/or take-profit prices on a margin position.
/// Pass 0 for sl_price or tp_price to clear that trigger.
/// Returns: 0=success, 1=not found, 2=not owner, 3=not open, 4=reentrancy,
///          5=invalid SL (long: sl must be < entry, short: sl must be > entry),
///          6=invalid TP (long: tp must be > entry, short: tp must be < entry)
pub fn set_position_sl_tp(
    caller: *const u8,
    position_id: u64,
    sl_price: u64,
    tp_price: u64,
) -> u32 {
    if !reentrancy_enter() {
        return 4;
    }
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    let real_caller = get_caller();
    if real_caller.0 != c {
        reentrancy_exit();
        return 200;
    }

    let pk = position_key(position_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= POSITION_SIZE_V1 => d,
        _ => {
            reentrancy_exit();
            return 1;
        }
    };

    let trader = decode_pos_trader(&data);
    if trader != c {
        reentrancy_exit();
        return 2;
    }
    if decode_pos_status(&data) != POS_OPEN {
        reentrancy_exit();
        return 3;
    }

    let side = decode_pos_side(&data);
    let entry_price = decode_pos_entry_price(&data);

    // Validate SL direction
    if sl_price > 0 {
        if side == SIDE_LONG && sl_price >= entry_price {
            reentrancy_exit();
            return 5; // Long SL must be below entry
        }
        if side == SIDE_SHORT && sl_price <= entry_price {
            reentrancy_exit();
            return 5; // Short SL must be above entry
        }
    }

    // Validate TP direction
    if tp_price > 0 {
        if side == SIDE_LONG && tp_price <= entry_price {
            reentrancy_exit();
            return 6; // Long TP must be above entry
        }
        if side == SIDE_SHORT && tp_price >= entry_price {
            reentrancy_exit();
            return 6; // Short TP must be below entry
        }
    }

    update_pos_sl_price(&mut data, sl_price);
    update_pos_tp_price(&mut data, tp_price);
    storage_set(&pk, &data);

    reentrancy_exit();
    0
}

#[cfg(target_arch = "wasm32")]
fn reject_dispatch() -> u32 {
    lichen_sdk::set_return_data(&[0xFF; 8]);
    255
}

fn dispatch_min_len(args: &[u8]) -> Option<usize> {
    let opcode = *args.first()?;
    match opcode {
        0 | 13 | 14 | 17 | 37 | 39 | 40 | 46 | 49 | 51 | 52 => Some(33),
        1 | 4 | 5 | 7 | 25 | 27 | 31 => Some(49),
        2 => Some(66),
        3 | 6 | 8 | 21 | 22 | 26 | 30 | 35 | 41 | 42 | 44 | 45 | 47 | 48 => Some(41),
        9 => Some(73),
        10 | 11 | 12 | 15 | 23 | 38 => Some(9),
        16 | 18 | 19 | 20 | 43 => Some(1),
        24 | 28 => Some(57),
        29 | 33 | 34 => Some(65),
        32 => Some(75),
        36 => Some(42),
        50 => Some(49),
        _ => None,
    }
}

fn migration_lock_allows_opcode(opcode: u8) -> bool {
    matches!(
        opcode,
        // Admin/oracle price maintenance.
        1 | 30 | 31 |
        // Read-only operations.
        10 | 11 | 12 | 16 | 17 | 18 | 19 | 20 | 23 | 26 | 40 | 43 | 49 |
        // Deterministic migration and the one atomic activation boundary.
        41 | 44 | 50 | 51 | 52
    )
}

// WASM entry
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn call() -> u32 {
    let args = lichen_sdk::get_args();
    if args.is_empty() {
        return reject_dispatch();
    }
    match dispatch_min_len(&args) {
        Some(min_len) if args.len() >= min_len => {}
        _ => return reject_dispatch(),
    }
    if margin_v2_migration_locked() && !migration_lock_allows_opcode(args[0]) {
        lichen_sdk::set_return_data(&u64_to_bytes(97));
        return 97;
    }
    let mut _rc = 0u32;
    match args[0] {
        // 0 = initialize(admin[32])
        0 => {
            if args.len() >= 33 {
                let r = initialize(args[1..33].as_ptr());
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 1 = set_mark_price(caller[32], pair_id[8], price[8])
        1 => {
            if args.len() >= 49 {
                let pair_id = bytes_to_u64(&args[33..41]);
                let price = bytes_to_u64(&args[41..49]);
                let r = set_mark_price(args[1..33].as_ptr(), pair_id, price);
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 2 = open_position(trader[32], pair_id[8], side[1], size[8], leverage[8], margin[8], margin_mode[1]?)
        2 => {
            if args.len() >= 66 {
                let pair_id = bytes_to_u64(&args[33..41]);
                let side = args[41];
                let size = bytes_to_u64(&args[42..50]);
                let leverage = bytes_to_u64(&args[50..58]);
                let margin = bytes_to_u64(&args[58..66]);
                let margin_mode = if args.len() >= 67 {
                    args[66]
                } else {
                    MARGIN_MODE_ISOLATED
                };
                let r = open_position_with_mode(
                    args[1..33].as_ptr(),
                    pair_id,
                    side,
                    size,
                    leverage,
                    margin,
                    margin_mode,
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 3 = close_position(caller[32], pos_id[8])
        3 => {
            if args.len() >= 41 {
                let pos_id = bytes_to_u64(&args[33..41]);
                let r = close_position(args[1..33].as_ptr(), pos_id);
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 4 = add_margin(caller[32], pos_id[8], amount[8])
        4 => {
            if args.len() >= 49 {
                let pos_id = bytes_to_u64(&args[33..41]);
                let amount = bytes_to_u64(&args[41..49]);
                let r = add_margin(args[1..33].as_ptr(), pos_id, amount);
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 5 = remove_margin(caller[32], pos_id[8], amount[8])
        5 => {
            if args.len() >= 49 {
                let pos_id = bytes_to_u64(&args[33..41]);
                let amount = bytes_to_u64(&args[41..49]);
                let r = remove_margin(args[1..33].as_ptr(), pos_id, amount);
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 6 = liquidate(liquidator[32], pos_id[8])
        6 => {
            if args.len() >= 41 {
                let pos_id = bytes_to_u64(&args[33..41]);
                let r = liquidate(args[1..33].as_ptr(), pos_id);
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 7 = set_max_leverage(caller[32], pair_id[8], max_lev[8])
        7 => {
            if args.len() >= 49 {
                let pair_id = bytes_to_u64(&args[33..41]);
                let max_lev = bytes_to_u64(&args[41..49]);
                let r = set_max_leverage(args[1..33].as_ptr(), pair_id, max_lev);
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 8 = set_maintenance_margin(caller[32], margin_bps[8])
        8 => {
            if args.len() >= 41 {
                let bps = bytes_to_u64(&args[33..41]);
                let r = set_maintenance_margin(args[1..33].as_ptr(), bps);
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 9 = withdraw_insurance(caller[32], amount[8], recipient[32])
        9 => {
            if args.len() >= 73 {
                let amount = bytes_to_u64(&args[33..41]);
                let r = withdraw_insurance(args[1..33].as_ptr(), amount, args[41..73].as_ptr());
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
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
                lichen_sdk::set_return_data(&u64_to_bytes(r));
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
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 14 = emergency_unpause(caller[32])
        14 => {
            if args.len() >= 33 {
                let r = emergency_unpause(args[1..33].as_ptr());
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        15 => {
            // apply_funding(pair_id[8])
            if args.len() >= 9 {
                let r = apply_funding(bytes_to_u64(&args[1..9]));
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
            }
        }
        16 => {
            // get_total_volume — cumulative notional volume of all margin positions
            lichen_sdk::set_return_data(&u64_to_bytes(load_u64(TOTAL_VOLUME_KEY)));
        }
        17 => {
            // get_user_positions — list all position IDs for a user
            if args.len() >= 33 {
                let addr: [u8; 32] = args[1..33].try_into().unwrap_or([0u8; 32]);
                let count = load_u64(&user_position_count_key(&addr));
                let mut result = Vec::with_capacity(8 + count as usize * 8);
                result.extend_from_slice(&u64_to_bytes(count));
                for i in 1..=count {
                    let pid = load_u64(&user_position_key(&addr, i));
                    result.extend_from_slice(&u64_to_bytes(pid));
                }
                lichen_sdk::set_return_data(&result);
            }
        }
        18 => {
            // get_total_pnl — returns [total_profit(8), total_loss(8)]
            let mut buf = Vec::with_capacity(16);
            buf.extend_from_slice(&u64_to_bytes(load_u64(TOTAL_PNL_PROFIT_KEY)));
            buf.extend_from_slice(&u64_to_bytes(load_u64(TOTAL_PNL_LOSS_KEY)));
            lichen_sdk::set_return_data(&buf);
        }
        19 => {
            // get_liquidation_count
            lichen_sdk::set_return_data(&u64_to_bytes(load_u64(LIQUIDATION_COUNT_KEY)));
        }
        20 => {
            // get_margin_stats — aggregated [pos_count, total_volume, liquidations, pnl_profit, pnl_loss, insurance_fund]
            let mut buf = Vec::with_capacity(48);
            buf.extend_from_slice(&u64_to_bytes(load_u64(POSITION_COUNT_KEY)));
            buf.extend_from_slice(&u64_to_bytes(load_u64(TOTAL_VOLUME_KEY)));
            buf.extend_from_slice(&u64_to_bytes(load_u64(LIQUIDATION_COUNT_KEY)));
            buf.extend_from_slice(&u64_to_bytes(load_u64(TOTAL_PNL_PROFIT_KEY)));
            buf.extend_from_slice(&u64_to_bytes(load_u64(TOTAL_PNL_LOSS_KEY)));
            buf.extend_from_slice(&u64_to_bytes(load_u64(INSURANCE_FUND_KEY)));
            lichen_sdk::set_return_data(&buf);
        }
        // 21 = enable_margin_pair(caller[32], pair_id[8])
        21 => {
            if args.len() >= 41 {
                let pair_id = bytes_to_u64(&args[33..41]);
                let r = enable_margin_pair(args[1..33].as_ptr(), pair_id);
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 22 = disable_margin_pair(caller[32], pair_id[8])
        22 => {
            if args.len() >= 41 {
                let pair_id = bytes_to_u64(&args[33..41]);
                let r = disable_margin_pair(args[1..33].as_ptr(), pair_id);
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 23 = is_margin_enabled(pair_id[8])
        23 => {
            if args.len() >= 9 {
                let pair_id = bytes_to_u64(&args[1..9]);
                lichen_sdk::set_return_data(&u64_to_bytes(is_margin_enabled(pair_id)));
            }
        }
        // 24 = set_position_sl_tp(caller[32], position_id[8], sl_price[8], tp_price[8])
        24 => {
            if args.len() >= 57 {
                let r = set_position_sl_tp(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    bytes_to_u64(&args[41..49]),
                    bytes_to_u64(&args[49..57]),
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 25 = partial_close(caller[32], position_id[8], close_amount[8])
        25 => {
            if args.len() >= 49 {
                let pos_id = bytes_to_u64(&args[33..41]);
                let close_amount = bytes_to_u64(&args[41..49]);
                let r = partial_close(args[1..33].as_ptr(), pos_id, close_amount);
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 26 = query_user_open_position(trader[32], pair_id[8])
        26 => {
            if args.len() >= 41 {
                let pair_id = bytes_to_u64(&args[33..41]);
                let r = query_user_open_position(args[1..33].as_ptr(), pair_id);
                lichen_sdk::set_return_data(&u64_to_bytes(r));
            }
        }
        // 27 = close_position_limit(caller[32], pos_id[8], limit_price[8])
        27 => {
            if args.len() >= 49 {
                let pos_id = bytes_to_u64(&args[33..41]);
                let limit_price = bytes_to_u64(&args[41..49]);
                let r = close_position_limit(args[1..33].as_ptr(), pos_id, limit_price);
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // 28 = partial_close_limit(caller[32], pos_id[8], close_amount[8], limit_price[8])
        28 => {
            if args.len() >= 57 {
                let pos_id = bytes_to_u64(&args[33..41]);
                let close_amount = bytes_to_u64(&args[41..49]);
                let limit_price = bytes_to_u64(&args[49..57]);
                let r =
                    partial_close_limit(args[1..33].as_ptr(), pos_id, close_amount, limit_price);
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
                _rc = r as u32;
            }
        }
        // AUDIT-FIX MARGIN-1: 29 = set_oracle_contract(caller[32], oracle_addr[32])
        29 => {
            if args.len() >= 65 {
                let r = set_oracle_contract(args[1..33].as_ptr(), args[33..65].as_ptr());
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
            }
        }
        // AUDIT-FIX MARGIN-1: 30 = update_mark_price_from_oracle(caller[32], pair_id[8], asset_name[N])
        30 => {
            if args.len() >= 41 {
                let pair_id = bytes_to_u64(&args[33..41]);
                let asset_len = (args.len() - 41) as u32;
                let asset_ptr = if asset_len > 0 {
                    args[41..].as_ptr()
                } else {
                    core::ptr::null()
                };
                let r = update_mark_price_from_oracle(
                    args[1..33].as_ptr(),
                    pair_id,
                    asset_ptr,
                    asset_len,
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
            }
        }
        // 31 = set_index_price(caller[32], pair_id[8], price[8])
        31 => {
            if args.len() >= 49 {
                let pair_id = bytes_to_u64(&args[33..41]);
                let price = bytes_to_u64(&args[41..49]);
                let r = set_index_price(args[1..33].as_ptr(), pair_id, price);
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
            }
        }
        // 32 = open_position_limit(trader[32], pair_id[8], side[1], size[8], leverage[8], margin[8], margin_mode[1], limit_price[8])
        32 => {
            if args.len() >= 75 {
                let pair_id = bytes_to_u64(&args[33..41]);
                let side = args[41];
                let size = bytes_to_u64(&args[42..50]);
                let leverage = bytes_to_u64(&args[50..58]);
                let margin = bytes_to_u64(&args[58..66]);
                let margin_mode = args[66];
                let limit_price = bytes_to_u64(&args[67..75]);
                let r = open_position_limit_with_mode(
                    args[1..33].as_ptr(),
                    pair_id,
                    side,
                    size,
                    leverage,
                    margin,
                    margin_mode,
                    limit_price,
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
            }
        }
        // 33 = set_collateral_token_address(caller[32], token_addr[32])
        33 => {
            if args.len() >= 65 {
                let r = set_collateral_token_address(args[1..33].as_ptr(), args[33..65].as_ptr());
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
            }
        }
        // 34 = set_self_address(caller[32], self_addr[32])
        34 => {
            if args.len() >= 65 {
                let r = set_self_address(args[1..33].as_ptr(), args[33..65].as_ptr());
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
            }
        }
        // 35 = deposit_insurance(caller[32], amount[8])
        35 => {
            if args.len() >= 41 {
                let amount = bytes_to_u64(&args[33..41]);
                let r = deposit_insurance(args[1..33].as_ptr(), amount);
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r as u32;
            }
        }
        // 36 = set_oracle_market(caller[32], pair_id[8], market[N])
        36 => {
            if args.len() > 41 {
                let pair_id = bytes_to_u64(&args[33..41]);
                let market_len = (args.len() - 41) as u32;
                let r = set_oracle_market(
                    args[1..33].as_ptr(),
                    pair_id,
                    args[41..].as_ptr(),
                    market_len,
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r;
            }
        }
        // 37 = activate_funding_v2(caller[32])
        37 => {
            if args.len() >= 33 {
                let r = activate_funding_v2(args[1..33].as_ptr());
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r;
            }
        }
        // 38 = settle_position_funding(position_id[8])
        38 => {
            if args.len() >= 9 {
                let r = settle_position_funding(bytes_to_u64(&args[1..9]));
                if r != 0 {
                    lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                }
                _rc = r;
            }
        }
        // 39 = claim_funding(caller[32])
        39 => {
            if args.len() >= 33 {
                let r = claim_funding(args[1..33].as_ptr());
                if r != 0 {
                    lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                }
                _rc = r;
            }
        }
        // 40 = get_funding_state(user[32])
        40 => {
            if args.len() >= 33 {
                get_funding_state(args[1..33].as_ptr());
            }
        }
        // 41 = migrate_funding_v2_position(caller[32], position_id[8])
        41 => {
            if args.len() >= 41 {
                let r = migrate_funding_v2_position(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r;
            }
        }
        // 42 = finalize_funding_v2_migration(caller[32], expected_open[8])
        42 => {
            if args.len() >= 41 {
                let r = finalize_funding_v2_migration(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r;
            }
        }
        // 43 = get_bad_debt
        43 => {
            lichen_sdk::set_return_data(&u64_to_bytes(get_bad_debt()));
        }
        // 44 = migrate_cross_v2_position(caller[32], position_id[8])
        44 => {
            if args.len() >= 41 {
                let r = migrate_cross_v2_position(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r;
            }
        }
        // 45 = finalize_cross_v2_migration(caller[32], expected_open_cross[8])
        45 => {
            if args.len() >= 41 {
                let r = finalize_cross_v2_migration(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r;
            }
        }
        // 46 = activate_cross_v2(caller[32])
        46 => {
            if args.len() >= 33 {
                let r = activate_cross_v2(args[1..33].as_ptr());
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r;
            }
        }
        // 47 = deposit_cross_collateral(caller[32], amount[8])
        47 => {
            if args.len() >= 41 {
                let r = deposit_cross_collateral(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                );
                if r != 0 {
                    lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                }
                _rc = r;
            }
        }
        // 48 = withdraw_cross_collateral(caller[32], amount[8])
        48 => {
            if args.len() >= 41 {
                let r = withdraw_cross_collateral(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                );
                if r != 0 {
                    lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                }
                _rc = r;
            }
        }
        // 49 = get_cross_account(user[32])
        49 => {
            if args.len() >= 33 {
                _rc = get_cross_account(args[1..33].as_ptr());
            }
        }
        // 50 = finalize_and_activate_margin_v2(caller[32], expected_open[8], expected_cross[8])
        50 => {
            if args.len() >= 49 {
                let r = finalize_and_activate_margin_v2(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    bytes_to_u64(&args[41..49]),
                );
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r;
            }
        }
        // 51 = begin_margin_v2_migration(caller[32])
        51 => {
            if args.len() >= 33 {
                let r = begin_margin_v2_migration(args[1..33].as_ptr());
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r;
            }
        }
        // 52 = complete_margin_v2_migration(caller[32])
        52 => {
            if args.len() >= 33 {
                let r = complete_margin_v2_migration(args[1..33].as_ptr());
                lichen_sdk::set_return_data(&u64_to_bytes(r as u64));
                _rc = r;
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
    use alloc::vec;
    use lichen_sdk::test_mock;

    #[test]
    fn test_wasm_dispatch_covers_every_public_opcode_and_migration_lock_is_closed() {
        for opcode in 0u8..=52 {
            assert!(
                dispatch_min_len(&[opcode]).is_some(),
                "opcode {opcode} is unreachable through the WASM dispatcher"
            );
        }
        assert_eq!(dispatch_min_len(&[53]), None);
        for blocked in [2u8, 4, 5, 6, 14, 24, 25, 27, 28, 32, 38, 39, 47, 48] {
            assert!(!migration_lock_allows_opcode(blocked));
        }
        for allowed in [1u8, 10, 30, 31, 40, 41, 43, 44, 49, 50, 51, 52] {
            assert!(migration_lock_allows_opcode(allowed));
        }
    }

    fn setup() -> [u8; 32] {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(activate_funding_v2(admin.as_ptr()), 0);
        assert_eq!(activate_cross_v2(admin.as_ptr()), 0);
        // Set mark price for pair 1: 1.0 (scaled by 1e9)
        set_mark_price(admin.as_ptr(), 1, 1_000_000_000);
        // Enable margin for pair 1
        enable_margin_pair(admin.as_ptr(), 1);
        storage_set(COLLATERAL_TOKEN_ADDRESS_KEY, &[9u8; 32]);
        storage_set(SELF_ADDRESS_KEY, &[8u8; 32]);
        save_u64(INSURANCE_FUND_KEY, 10_000_000_000_000_000);
        admin
    }

    fn oracle_quote_response(price: u64, source_slot: u64) -> Vec<u8> {
        let mut response = Vec::with_capacity(17);
        response.extend_from_slice(&price.to_le_bytes());
        response.extend_from_slice(&source_slot.to_le_bytes());
        response.push(8);
        response
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
        assert_eq!(init, 5000); // 50%
        assert_eq!(maint, 2500); // 25%
        assert_eq!(liq, 300); // 3%
        assert_eq!(fund, 10); // 1.0x
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
        assert_eq!(fund, 10);
    }

    #[test]
    fn test_tier_params_10x() {
        let (init, maint, liq, fund) = get_tier_params(10);
        assert_eq!(init, 1000);
        assert_eq!(maint, 500);
        assert_eq!(liq, 500);
        assert_eq!(fund, 10);
    }

    #[test]
    fn test_tier_params_25x() {
        let (init, maint, liq, fund) = get_tier_params(25);
        assert_eq!(init, 400);
        assert_eq!(maint, 200);
        assert_eq!(liq, 700);
        assert_eq!(fund, 10);
    }

    #[test]
    fn test_tier_params_50x() {
        let (init, maint, liq, fund) = get_tier_params(50);
        assert_eq!(init, 200);
        assert_eq!(maint, 100);
        assert_eq!(liq, 1000);
        assert_eq!(fund, 10);
    }

    #[test]
    fn test_tier_params_100x() {
        let (init, maint, liq, fund) = get_tier_params(100);
        assert_eq!(init, 100);
        assert_eq!(maint, 50);
        assert_eq!(liq, 1500);
        assert_eq!(fund, 10);
    }

    #[test]
    fn test_tier_params_7x_uses_10x_tier() {
        // 7x falls in ≤10x tier
        let (init, maint, liq, fund) = get_tier_params(7);
        assert_eq!(init, 1000);
        assert_eq!(maint, 500);
        assert_eq!(liq, 500);
        assert_eq!(fund, 10);
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
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        // AUDIT-FIX NEW-H2: corrected formula — no /leverage.
        // 2x tier: initial_margin_bps=5000 → required = 1B * 5000/10000 = 500_000_000
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );
        assert_eq!(get_position_count(), 1);
    }

    #[test]
    fn test_open_position_limit_long_respects_mark() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);

        assert_eq!(
            open_position_limit_with_mode(
                trader.as_ptr(),
                1,
                SIDE_LONG,
                1_000_000_000,
                2,
                500_000_000,
                0,
                900_000_000,
            ),
            10
        );
        assert_eq!(get_position_count(), 0);

        assert_eq!(
            open_position_limit_with_mode(
                trader.as_ptr(),
                1,
                SIDE_LONG,
                1_000_000_000,
                2,
                500_000_000,
                0,
                1_100_000_000,
            ),
            0
        );
        assert_eq!(get_position_count(), 1);
    }

    #[test]
    fn test_open_position_limit_short_respects_mark() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);

        assert_eq!(
            open_position_limit_with_mode(
                trader.as_ptr(),
                1,
                SIDE_SHORT,
                1_000_000_000,
                2,
                500_000_000,
                0,
                1_100_000_000,
            ),
            10
        );
        assert_eq!(get_position_count(), 0);

        assert_eq!(
            open_position_limit_with_mode(
                trader.as_ptr(),
                1,
                SIDE_SHORT,
                1_000_000_000,
                2,
                500_000_000,
                0,
                900_000_000,
            ),
            0
        );
        assert_eq!(get_position_count(), 1);
    }

    #[test]
    fn test_open_position_collateral_lock_failure_without_mutation() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        test_mock::set_cross_call_should_fail(true);

        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            8
        );
        assert_eq!(load_u64(POSITION_COUNT_KEY), 0);
        assert!(storage_get(&position_key(1)).is_none());
        assert_eq!(load_u64(TOTAL_OPEN_INTEREST_KEY), 0);
    }

    #[test]
    fn test_open_position_requires_insurance_liquidity() {
        let _admin = setup();
        save_u64(INSURANCE_FUND_KEY, 0);
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);

        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            11
        );
        assert_eq!(load_u64(POSITION_COUNT_KEY), 0);
        assert!(storage_get(&position_key(1)).is_none());
    }

    #[test]
    fn test_open_position_short() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        assert_eq!(
            open_position(
                trader.as_ptr(),
                1,
                SIDE_SHORT,
                1_000_000_000,
                2,
                500_000_000
            ),
            0
        );
    }

    #[test]
    fn test_open_position_5x() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        // 5x tier: initial_margin_bps=2000 → required = 1B * 2000/10000 = 200_000_000
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 5, 200_000_000),
            0
        );
    }

    #[test]
    fn test_open_position_10x() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        // 10x tier: initial_margin_bps=1000 → required = 1B * 1000/10000 = 100_000_000
        assert_eq!(
            open_position(
                trader.as_ptr(),
                1,
                SIDE_LONG,
                1_000_000_000,
                10,
                100_000_000
            ),
            0
        );
    }

    #[test]
    fn test_open_position_25x() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        // 25x tier: initial_margin_bps=400 → required = 1B * 400/10000 = 40_000_000
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 25, 40_000_000),
            0
        );
    }

    #[test]
    fn test_open_position_50x() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        // 50x tier: initial_margin_bps=200 → required = 1B * 200/10000 = 20_000_000
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 50, 20_000_000),
            0
        );
    }

    #[test]
    fn test_open_position_100x() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        // 100x tier: initial_margin_bps=100 → required = 1B * 100/10000 = 10_000_000
        assert_eq!(
            open_position(
                trader.as_ptr(),
                1,
                SIDE_LONG,
                1_000_000_000,
                100,
                10_000_000
            ),
            0
        );
    }

    #[test]
    fn test_open_position_cross_mode_enforces_3x_cap() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        assert_eq!(
            open_position_with_mode(
                trader.as_ptr(),
                1,
                SIDE_LONG,
                1_000_000_000,
                4,
                250_000_000,
                MARGIN_MODE_CROSS
            ),
            2
        );
    }

    #[test]
    fn test_open_position_cross_mode_persisted_on_chain() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        assert_eq!(
            open_position_with_mode(
                trader.as_ptr(),
                1,
                SIDE_LONG,
                1_000_000_000,
                3,
                333_300_000,
                MARGIN_MODE_CROSS
            ),
            0
        );
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin_mode(&data), MARGIN_MODE_CROSS);
        assert_eq!(decode_pos_margin(&data), 0);
        assert_eq!(load_u64(&cross_balance_key(&trader)), 333_300_000);
        assert_eq!(load_u64(&cross_position_count_key(&trader)), 1);
        assert_eq!(load_u64(&cross_position_key(&trader, 1)), 1);
        assert_eq!(load_u64(&cross_position_index_key(1)), 1);
        assert_eq!(data[123], COLLATERAL_LUSD);
    }

    #[test]
    fn test_cross_positions_share_one_bounded_collateral_account() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        assert_eq!(
            open_position_with_mode(
                trader.as_ptr(),
                1,
                SIDE_LONG,
                1_000_000_000,
                3,
                700_000_000,
                MARGIN_MODE_CROSS,
            ),
            0
        );
        // The second position uses the same account; it does not need another
        // isolated deposit because aggregate equity covers both requirements.
        assert_eq!(
            open_position_with_mode(
                trader.as_ptr(),
                1,
                SIDE_SHORT,
                1_000_000_000,
                3,
                0,
                MARGIN_MODE_CROSS,
            ),
            0
        );

        let metrics = cross_portfolio_metrics(&trader).unwrap();
        assert_eq!(metrics.balance, 700_000_000);
        assert_eq!(metrics.position_count, 2);
        assert_eq!(metrics.equity, 700_000_000);
        assert_eq!(metrics.total_notional, 2_000_000_000);
        assert_eq!(metrics.initial_required, 666_600_000);
        assert_eq!(metrics.maintenance_required, 340_000_000);
        assert_eq!(load_u64(TOTAL_COLLATERAL_ESCROWED_KEY), 700_000_000);
        assert_eq!(load_u64(CROSS_TOTAL_COLLATERAL_KEY), 700_000_000);
        assert_eq!(decode_pos_margin(&storage_get(&position_key(1)).unwrap()), 0);
        assert_eq!(decode_pos_margin(&storage_get(&position_key(2)).unwrap()), 0);

        assert_eq!(withdraw_cross_collateral(trader.as_ptr(), 33_400_000), 0);
        assert_eq!(withdraw_cross_collateral(trader.as_ptr(), 1), 3);
        assert_eq!(load_u64(&cross_balance_key(&trader)), 666_600_000);
    }

    #[test]
    fn test_cross_health_nets_portfolio_pnl_and_blocks_false_liquidation() {
        let admin = setup();
        let trader = [2u8; 32];
        let liquidator = [3u8; 32];
        test_mock::set_caller(trader);
        assert_eq!(
            open_position_with_mode(
                trader.as_ptr(),
                1,
                SIDE_LONG,
                1_000_000_000,
                3,
                700_000_000,
                MARGIN_MODE_CROSS,
            ),
            0
        );
        assert_eq!(
            open_position_with_mode(
                trader.as_ptr(),
                1,
                SIDE_SHORT,
                1_000_000_000,
                3,
                0,
                MARGIN_MODE_CROSS,
            ),
            0
        );
        test_mock::set_caller(admin);
        assert_eq!(set_mark_price(admin.as_ptr(), 1, 500_000_000), 0);

        let metrics = cross_portfolio_metrics(&trader).unwrap();
        assert_eq!(metrics.equity, 700_000_000);
        assert_eq!(metrics.maintenance_required, 170_000_000);
        test_mock::set_caller(liquidator);
        assert_eq!(liquidate(liquidator.as_ptr(), 1), 2);
        assert_eq!(liquidate(liquidator.as_ptr(), 2), 2);
    }

    #[test]
    fn test_cross_close_realizes_pnl_into_shared_balance_without_withdrawal() {
        let admin = setup();
        let trader = [2u8; 32];
        let insurance_before = get_insurance_fund();
        test_mock::set_caller(trader);
        assert_eq!(
            open_position_with_mode(
                trader.as_ptr(),
                1,
                SIDE_LONG,
                1_000_000_000,
                3,
                700_000_000,
                MARGIN_MODE_CROSS,
            ),
            0
        );
        assert_eq!(
            open_position_with_mode(
                trader.as_ptr(),
                1,
                SIDE_SHORT,
                1_000_000_000,
                3,
                0,
                MARGIN_MODE_CROSS,
            ),
            0
        );
        test_mock::set_caller(admin);
        assert_eq!(set_mark_price(admin.as_ptr(), 1, 800_000_000), 0);
        test_mock::set_caller(trader);

        assert_eq!(close_position(trader.as_ptr(), 1), 0);
        assert_eq!(load_u64(&cross_balance_key(&trader)), 500_000_000);
        assert_eq!(load_u64(&cross_position_count_key(&trader)), 1);
        assert_eq!(get_insurance_fund(), insurance_before + 200_000_000);
        assert_eq!(load_u64(TOTAL_COLLATERAL_ESCROWED_KEY), 500_000_000);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 500_000_000);

        assert_eq!(close_position(trader.as_ptr(), 2), 0);
        assert_eq!(load_u64(&cross_balance_key(&trader)), 700_000_000);
        assert_eq!(load_u64(&cross_position_count_key(&trader)), 0);
        assert_eq!(get_insurance_fund(), insurance_before);
        assert_eq!(load_u64(TOTAL_COLLATERAL_ESCROWED_KEY), 700_000_000);
        assert_eq!(withdraw_cross_collateral(trader.as_ptr(), 700_000_000), 0);
        assert_eq!(load_u64(TOTAL_COLLATERAL_ESCROWED_KEY), 0);
    }

    #[test]
    fn test_cross_liquidation_settles_loss_penalty_and_reward_exactly() {
        let admin = setup();
        let trader = [2u8; 32];
        let liquidator = [3u8; 32];
        let insurance_before = get_insurance_fund();
        test_mock::set_caller(trader);
        assert_eq!(
            open_position_with_mode(
                trader.as_ptr(),
                1,
                SIDE_LONG,
                1_000_000_000,
                3,
                333_300_000,
                MARGIN_MODE_CROSS,
            ),
            0
        );
        test_mock::set_caller(admin);
        assert_eq!(set_mark_price(admin.as_ptr(), 1, 700_000_000), 0);
        test_mock::set_caller(liquidator);
        assert_eq!(liquidate(liquidator.as_ptr(), 1), 0);

        // Loss 300M leaves 33.3M. Penalty 21M: 10.5M reward, 10.5M insurance.
        assert_eq!(load_u64(&cross_balance_key(&trader)), 12_300_000);
        assert_eq!(get_insurance_fund(), insurance_before + 310_500_000);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 10_500_000);
        assert_eq!(get_bad_debt(), 0);
        assert_eq!(load_u64(&cross_position_count_key(&trader)), 0);
        assert_eq!(load_u64(TOTAL_COLLATERAL_ESCROWED_KEY), 12_300_000);
    }

    #[test]
    fn test_cross_funding_debits_shared_balance_not_position_margin() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(admin);
        assert_eq!(set_index_price(admin.as_ptr(), 1, 1_000_000_000), 0);
        test_mock::set_caller(trader);
        assert_eq!(
            open_position_with_mode(
                trader.as_ptr(),
                1,
                SIDE_LONG,
                1_000_000_000,
                2,
                500_000_000,
                MARGIN_MODE_CROSS,
            ),
            0
        );
        test_mock::set_slot(100 + FUNDING_INTERVAL_SLOTS);
        test_mock::set_caller(admin);
        assert_eq!(set_mark_price(admin.as_ptr(), 1, 1_100_000_000), 0);
        assert_eq!(set_index_price(admin.as_ptr(), 1, 1_000_000_000), 0);
        assert_eq!(apply_funding(1), 0);
        assert_eq!(settle_position_funding(1), 0);

        assert_eq!(load_u64(&cross_balance_key(&trader)), 489_000_000);
        assert_eq!(load_u64(FUNDING_POOL_KEY), 11_000_000);
        assert_eq!(decode_pos_margin(&storage_get(&position_key(1)).unwrap()), 0);
        assert_eq!(load_u64(CROSS_TOTAL_COLLATERAL_KEY), 489_000_000);
        assert_eq!(load_u64(TOTAL_COLLATERAL_ESCROWED_KEY), 489_000_000);
    }

    #[test]
    fn test_cross_v2_legacy_migration_preserves_total_escrow() {
        test_mock::reset();
        let admin = [1u8; 32];
        let trader = [2u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        let legacy = encode_position(
            &trader,
            1,
            1,
            SIDE_LONG,
            POS_OPEN,
            1_000_000_000,
            333_300_000,
            1_000_000_000,
            3,
            10,
            0,
            0,
            MARGIN_MODE_CROSS,
            COLLATERAL_LUSD,
        );
        storage_set(&position_key(1), &legacy);
        save_u64(POSITION_COUNT_KEY, 1);
        save_u64(TOTAL_COLLATERAL_ESCROWED_KEY, 333_300_000);

        assert_eq!(begin_margin_v2_migration(admin.as_ptr()), 0);
        let migration_operator = [9u8; 32];
        test_mock::set_caller(migration_operator);
        assert_eq!(migrate_funding_v2_position(migration_operator.as_ptr(), 1), 0);
        assert_eq!(migrate_cross_v2_position(migration_operator.as_ptr(), 1), 0);
        assert_eq!(migrate_cross_v2_position(migration_operator.as_ptr(), 1), 4);
        test_mock::set_caller(admin);
        assert_eq!(finalize_and_activate_margin_v2(admin.as_ptr(), 1, 1), 0);

        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin(&data), 0);
        assert_eq!(bytes_to_u64(&data[90..98]), 1u64 << 63);
        assert_eq!(load_u64(&cross_balance_key(&trader)), 333_300_000);
        assert_eq!(load_u64(CROSS_TOTAL_COLLATERAL_KEY), 333_300_000);
        assert_eq!(load_u64(TOTAL_COLLATERAL_ESCROWED_KEY), 333_300_000);
        assert_eq!(load_u64(&cross_position_count_key(&trader)), 1);
        assert_eq!(load_u64(&cross_position_index_key(1)), 1);
    }

    #[test]
    fn test_open_position_invalid_margin_mode_rejected() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        assert_eq!(
            open_position_with_mode(
                trader.as_ptr(),
                1,
                SIDE_LONG,
                1_000_000_000,
                2,
                500_000_000,
                9
            ),
            9
        );
    }

    #[test]
    fn test_open_position_overleveraged() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        // 101x exceeds MAX_LEVERAGE_ISOLATED=100
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 101, 200),
            2
        );
    }

    #[test]
    fn test_open_position_zero_size_rejected() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 0, 2, 500_000_000),
            2
        );
    }

    #[test]
    fn test_open_position_required_margin_uses_u128_math() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        let size = 4_000_000_000_000_000u64;
        let under_margin = 1_999_999_999_999_999u64;
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, size, 2, under_margin),
            3
        );
    }

    #[test]
    fn test_open_position_insufficient_margin_5x() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        // 5x, notional=1B, required=200_000_000; give less
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 5, 199_999_999),
            3
        );
    }

    #[test]
    fn test_open_position_no_mark_price() {
        let admin = setup();
        // Enable margin for pair 2 but don't set a mark price
        enable_margin_pair(admin.as_ptr(), 2);
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        assert_eq!(
            open_position(trader.as_ptr(), 2, SIDE_LONG, 1000, 2, 200),
            6
        );
    }

    #[test]
    fn test_open_position_paused() {
        let admin = setup();
        emergency_pause(admin.as_ptr());
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1000, 2, 200),
            1
        );
    }

    #[test]
    fn test_close_position() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(close_position(trader.as_ptr(), 1), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_CLOSED);
    }

    #[test]
    fn test_close_position_still_works_when_paused() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);

        test_mock::set_caller(admin);
        assert_eq!(emergency_pause(admin.as_ptr()), 0);

        test_mock::set_caller(trader);
        assert_eq!(close_position(trader.as_ptr(), 1), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_CLOSED);
    }

    #[test]
    fn test_close_not_owner() {
        let _admin = setup();
        let trader = [2u8; 32];
        let other = [3u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        test_mock::set_caller(other);
        assert_eq!(close_position(other.as_ptr(), 1), 2);
    }

    #[test]
    fn test_close_already_closed() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        close_position(trader.as_ptr(), 1);
        assert_eq!(close_position(trader.as_ptr(), 1), 3);
    }

    #[test]
    fn test_close_position_decrements_open_interest_by_entry_notional() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );
        assert_eq!(load_u64(TOTAL_OPEN_INTEREST_KEY), 2_000_000_000);

        test_mock::set_caller(admin);
        assert_eq!(set_mark_price(admin.as_ptr(), 1, 600_000_000), 0);

        test_mock::set_caller(trader);
        assert_eq!(close_position(trader.as_ptr(), 1), 0);
        assert_eq!(load_u64(TOTAL_OPEN_INTEREST_KEY), 1_000_000_000);
    }

    #[test]
    fn test_add_margin() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(add_margin(trader.as_ptr(), 1, 100), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin(&data), 500_000_100);
    }

    #[test]
    fn test_add_margin_lock_failure_preserves_position() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );
        let before = storage_get(&position_key(1)).unwrap();

        test_mock::set_cross_call_should_fail(true);
        assert_eq!(add_margin(trader.as_ptr(), 1, 100), 7);

        let after = storage_get(&position_key(1)).unwrap();
        assert_eq!(after, before);
        assert_eq!(decode_pos_margin(&after), 500_000_000);
    }

    #[test]
    fn test_add_margin_zero() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(add_margin(trader.as_ptr(), 1, 0), 5);
    }

    #[test]
    fn test_remove_margin() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        // 2x: maint margin = 25% → need 250M for 1B notional
        // Start with 500M (50%) and remove 100M → still above 25%
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(remove_margin(trader.as_ptr(), 1, 100_000_000), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin(&data), 400_000_000);
    }

    #[test]
    fn test_remove_margin_still_works_when_paused() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);

        test_mock::set_caller(admin);
        assert_eq!(emergency_pause(admin.as_ptr()), 0);

        test_mock::set_caller(trader);
        assert_eq!(remove_margin(trader.as_ptr(), 1, 100_000_000), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin(&data), 400_000_000);
    }

    #[test]
    fn test_remove_margin_too_much() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        // 600M > 500M margin → error 5
        assert_eq!(remove_margin(trader.as_ptr(), 1, 600_000_000), 5);
    }

    #[test]
    fn test_remove_margin_would_breach_maintenance() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        // 2x: maint = 2500bps = 25%. notional=1B → need 250M maint.
        // Open with 500M (50%), remove 260M → 240M < 250M → fail
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(remove_margin(trader.as_ptr(), 1, 260_000_000), 6);
    }

    #[test]
    fn test_partial_close_decrements_open_interest_by_entry_notional() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );
        assert_eq!(load_u64(TOTAL_OPEN_INTEREST_KEY), 1_000_000_000);

        test_mock::set_caller(admin);
        assert_eq!(set_mark_price(admin.as_ptr(), 1, 500_000_000), 0);

        test_mock::set_caller(trader);
        assert_eq!(partial_close(trader.as_ptr(), 1, 400_000_000), 0);
        assert_eq!(load_u64(TOTAL_OPEN_INTEREST_KEY), 600_000_000);
    }

    #[test]
    fn test_liquidation_2x() {
        let admin = setup();
        let trader = [2u8; 32];
        let liquidator = [3u8; 32];
        test_mock::set_slot(100);
        // 2x long, margin=500M, size=1B at price 1.0
        test_mock::set_caller(trader);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        // Drop mark price to 0.6 → PnL = -400M, effective = 100M, notional = 600M
        // margin_ratio = 100M / 600M * 10000 = 1666 bps < 2500 maint → liquidatable
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 600_000_000);
        test_mock::set_caller(liquidator);
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
        test_mock::set_caller(trader);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 50, 20_000_000);
        // Drop mark price to 0.985 → PnL = -15M, effective = 5M, notional = 985M
        // ratio = 5M / 985M * 10000 ≈ 50 bps < 100 bps maint → liquidatable
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 985_000_000);
        test_mock::set_caller(liquidator);
        assert_eq!(liquidate(liquidator.as_ptr(), 1), 0);
    }

    #[test]
    fn test_liquidation_healthy_position() {
        let _admin = setup();
        let trader = [2u8; 32];
        let liquidator = [3u8; 32];
        test_mock::set_slot(100);
        // 2x with healthy margin (50%) > 25% maint
        test_mock::set_caller(trader);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        test_mock::set_caller(liquidator);
        assert_eq!(liquidate(liquidator.as_ptr(), 1), 2);
    }

    #[test]
    fn test_liquidation_falls_back_to_fresh_index_price() {
        let admin = setup_with_index();
        let trader = [2u8; 32];
        let liquidator = [3u8; 32];

        test_mock::set_timestamp(1000);
        test_mock::set_slot(100);

        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 1_000_000_000);
        set_index_price(admin.as_ptr(), 1, 1_000_000_000);

        test_mock::set_caller(trader);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 600_000_000);
        set_index_price(admin.as_ptr(), 1, 600_000_000);

        test_mock::set_timestamp(1000 + MAX_PRICE_AGE_SLOTS + 10);
        set_index_price(admin.as_ptr(), 1, 600_000_000);

        test_mock::set_caller(liquidator);
        assert_eq!(liquidate(liquidator.as_ptr(), 1), 0);

        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_LIQUIDATED);
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
        test_mock::set_caller(trader_a);
        let r1 = open_position(
            trader_a.as_ptr(),
            1,
            SIDE_LONG,
            1_000_000_000,
            5,
            200_000_000,
        );
        assert_eq!(r1, 0, "open_position 5x should succeed");

        let before = get_insurance_fund();
        // Drop mark price to 0.85 → PnL=-150M, effective=50M, notional=850M
        // ratio = 50M/850M*10000 = 588 bps < 1000 maint → liquidatable
        test_mock::set_caller(_admin);
        set_mark_price(_admin.as_ptr(), 1, 850_000_000);
        test_mock::set_caller(liquidator);
        let liq1 = liquidate(liquidator.as_ptr(), 1);
        assert_eq!(liq1, 0, "liquidate pos 1 should succeed");
        let after_a = get_insurance_fund();
        let insurance_a = after_a - before;
        let reward_a = bytes_to_u64(&test_mock::get_return_data());
        // penalty = 850M * 500/10000 = 42.5M = 42_500_000
        // realized loss = 150M; insurance also receives half the penalty.
        assert_eq!(insurance_a, 171_250_000);
        assert_eq!(reward_a, 21_250_000);
        assert_eq!(get_bad_debt(), 0);
        // 200M margin = 150M loss + 21.25M insurance penalty + 21.25M
        // liquidator reward + 7.5M trader payout.
        assert_eq!(200_000_000, 150_000_000 + insurance_a - 150_000_000 + reward_a + 7_500_000);

        // Reset price for 2nd position
        test_mock::set_caller(_admin);
        set_mark_price(_admin.as_ptr(), 1, 1_000_000_000);
        // For 2x tier: initial=5000bps, maint=2500bps=25%, penalty=300bps
        // notional=1B, required = 500M
        test_mock::set_caller(trader_b);
        let r2 = open_position(
            trader_b.as_ptr(),
            1,
            SIDE_LONG,
            1_000_000_000,
            2,
            500_000_000,
        );
        assert_eq!(r2, 0, "open_position 2x should succeed");
        // Drop mark price to 0.6 → PnL=-400M, effective=100M, notional=600M
        // ratio = 100M/600M*10000 = 1666 bps < 2500 maint → liquidatable
        test_mock::set_caller(_admin);
        set_mark_price(_admin.as_ptr(), 1, 600_000_000);
        // penalty = 600M * 300/10000 = 18M
        // insurance = 18M / 2 = 9_000_000
        test_mock::set_caller(liquidator);
        let liq2 = liquidate(liquidator.as_ptr(), 2);
        assert_eq!(liq2, 0, "liquidate pos 2 should succeed");
        let after_b = get_insurance_fund();
        let insurance_b = after_b - after_a;
        let reward_b = bytes_to_u64(&test_mock::get_return_data());
        assert_eq!(insurance_b, 409_000_000);
        assert_eq!(reward_b, 9_000_000);
        assert_eq!(get_bad_debt(), 0);
        // 500M margin = 400M loss + 9M insurance penalty + 9M reward + 82M payout.
        assert_eq!(500_000_000, 400_000_000 + 9_000_000 + reward_b + 82_000_000);
    }

    #[test]
    fn test_deeply_underwater_liquidation_records_bad_debt_without_fake_assets() {
        let admin = setup();
        let trader = [2u8; 32];
        let liquidator = [3u8; 32];
        let insurance_before = get_insurance_fund();

        test_mock::set_caller(trader);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );
        test_mock::set_caller(admin);
        assert_eq!(set_mark_price(admin.as_ptr(), 1, 400_000_000), 0);
        test_mock::set_caller(liquidator);
        assert_eq!(liquidate(liquidator.as_ptr(), 1), 0);

        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_LIQUIDATED);
        assert_eq!(get_insurance_fund(), insurance_before + 500_000_000);
        assert_eq!(get_bad_debt(), 100_000_000);
        assert_eq!(load_u64(TOTAL_PNL_LOSS_KEY), 600_000_000);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 0);
        assert_eq!(load_u64(TOTAL_COLLATERAL_ESCROWED_KEY), 0);
    }

    #[test]
    fn test_insurance_fund_accumulation() {
        let admin = setup();
        let trader = [2u8; 32];
        let liq = [3u8; 32];
        test_mock::set_slot(100);
        // 5x tier: required = 1B * 2000/10000 = 200M, maint=1000bps=10%
        test_mock::set_caller(trader);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 5, 200_000_000);
        // Drop mark price → position becomes unhealthy
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 850_000_000);
        let before = get_insurance_fund();
        test_mock::set_caller(liq);
        liquidate(liq.as_ptr(), 1);
        let after = get_insurance_fund();
        assert!(after > before);
    }

    #[test]
    fn test_set_max_leverage() {
        let admin = setup();
        assert_eq!(set_max_leverage(admin.as_ptr(), 1, 50), 0);
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 51, 200),
            2
        );
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
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        let ratio = get_margin_ratio(1);
        // margin=500M, size=1B, price=1.0 → notional=1B → ratio=500M/1B = 50% = 5000 bps
        assert_eq!(ratio, 5000);
    }

    #[test]
    fn test_pnl_calculation_long_profit() {
        let (is_profit, pnl) =
            calculate_pnl(SIDE_LONG, 1_000_000_000, 1_000_000_000, 1_500_000_000)
                .unwrap();
        assert!(is_profit);
        assert_eq!(pnl, 500_000_000);
    }

    #[test]
    fn test_pnl_calculation_long_loss() {
        let (is_profit, pnl) =
            calculate_pnl(SIDE_LONG, 1_000_000_000, 1_000_000_000, 500_000_000)
                .unwrap();
        assert!(!is_profit);
        assert_eq!(pnl, 500_000_000);
    }

    #[test]
    fn test_pnl_calculation_short_profit() {
        let (is_profit, pnl) =
            calculate_pnl(SIDE_SHORT, 1_000_000_000, 1_000_000_000, 500_000_000)
                .unwrap();
        assert!(is_profit);
        assert_eq!(pnl, 500_000_000);
    }

    #[test]
    fn test_pnl_calculation_short_loss() {
        let (is_profit, pnl) =
            calculate_pnl(SIDE_SHORT, 1_000_000_000, 1_000_000_000, 1_500_000_000)
                .unwrap();
        assert!(!is_profit);
        assert_eq!(pnl, 500_000_000);
    }

    #[test]
    fn test_pnl_overflow_fails_closed_instead_of_wrapping() {
        assert_eq!(calculate_pnl(SIDE_LONG, u64::MAX, 0, u64::MAX), None);
        assert_eq!(calculate_margin_ratio(1, u64::MAX, u64::MAX), 0);

        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 2_000_000_000, 2, 1_000_000_000),
            0
        );
        test_mock::set_caller(admin);
        assert_eq!(set_mark_price(admin.as_ptr(), 1, u64::MAX), 0);
        test_mock::set_caller(trader);
        assert_eq!(close_position(trader.as_ptr(), 1), 12);
        assert_eq!(decode_pos_status(&storage_get(&position_key(1)).unwrap()), POS_OPEN);
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
        test_mock::set_caller(trader);
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
        test_mock::set_caller(rando);
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
    fn test_withdraw_insurance_no_collateral_token_addr() {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        // Seed insurance fund
        save_u64(INSURANCE_FUND_KEY, 1_000_000);
        let recipient = [5u8; 32];
        assert_eq!(
            withdraw_insurance(admin.as_ptr(), 500_000, recipient.as_ptr()),
            4
        );
    }

    #[test]
    fn test_withdraw_insurance_success() {
        let admin = setup();
        save_u64(INSURANCE_FUND_KEY, 1_000_000);
        let recipient = [5u8; 32];
        // In test mode, cross-contract call returns Ok(Vec::new()) → success path
        assert_eq!(
            withdraw_insurance(admin.as_ptr(), 500_000, recipient.as_ptr()),
            0
        );
        assert_eq!(get_insurance_fund(), 500_000);
    }

    #[test]
    fn test_withdraw_insurance_cannot_breach_open_interest_coverage() {
        let admin = setup();
        let trader = [2u8; 32];
        let recipient = [3u8; 32];
        let insurance = get_insurance_fund();
        test_mock::set_caller(trader);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );
        test_mock::set_caller(admin);
        assert_eq!(
            withdraw_insurance(
                admin.as_ptr(),
                insurance - 999_999_999,
                recipient.as_ptr(),
            ),
            3
        );
        assert_eq!(get_insurance_fund(), insurance);
    }

    #[test]
    fn test_withdraw_insurance_rejects_zero_recipient_and_reentrancy() {
        let admin = setup();
        let zero = [0u8; 32];
        assert_eq!(withdraw_insurance(admin.as_ptr(), 1, zero.as_ptr()), 4);
        storage_set(REENTRANCY_KEY, &[1]);
        assert_eq!(withdraw_insurance(admin.as_ptr(), 1, admin.as_ptr()), 6);
    }

    #[test]
    fn test_withdraw_insurance_exceeds_balance() {
        let admin = setup();
        save_u64(INSURANCE_FUND_KEY, 100);
        let recipient = [5u8; 32];
        assert_eq!(
            withdraw_insurance(admin.as_ptr(), 200, recipient.as_ptr()),
            3
        );
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
        test_mock::set_caller(rando);
        assert_eq!(
            withdraw_insurance(rando.as_ptr(), 100, recipient.as_ptr()),
            1
        );
    }

    #[test]
    fn test_deposit_insurance_success() {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        storage_set(COLLATERAL_TOKEN_ADDRESS_KEY, &[9u8; 32]);
        storage_set(SELF_ADDRESS_KEY, &[8u8; 32]);

        assert_eq!(deposit_insurance(admin.as_ptr(), 1_000_000), 0);
        assert_eq!(get_insurance_fund(), 1_000_000);
    }

    #[test]
    fn test_deposit_insurance_overflow_rejected_before_transfer() {
        let admin = setup();
        save_u64(INSURANCE_FUND_KEY, u64::MAX - 5);
        test_mock::set_caller(admin);
        assert_eq!(deposit_insurance(admin.as_ptr(), 10), 5);
        assert_eq!(get_insurance_fund(), u64::MAX - 5);
        assert!(test_mock::get_last_cross_call().is_none());
    }

    #[test]
    fn test_deposit_insurance_is_permissionless() {
        test_mock::reset();
        let admin = [1u8; 32];
        let contributor = [2u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        storage_set(COLLATERAL_TOKEN_ADDRESS_KEY, &[9u8; 32]);
        storage_set(SELF_ADDRESS_KEY, &[8u8; 32]);

        test_mock::set_caller(contributor);
        assert_eq!(deposit_insurance(contributor.as_ptr(), 1_000_000), 0);
        assert_eq!(get_insurance_fund(), 1_000_000);
    }

    #[test]
    fn test_set_collateral_token_address() {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        let lusd = [9u8; 32];
        assert_eq!(
            set_collateral_token_address(admin.as_ptr(), lusd.as_ptr()),
            0
        );
        assert_eq!(load_addr(COLLATERAL_TOKEN_ADDRESS_KEY), lusd);
    }

    #[test]
    fn test_set_collateral_token_address_rejects_zero() {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        let zero = [0u8; 32];
        assert_eq!(
            set_collateral_token_address(admin.as_ptr(), zero.as_ptr()),
            2
        );
    }

    #[test]
    fn test_set_self_address() {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        let self_addr = [8u8; 32];
        assert_eq!(set_self_address(admin.as_ptr(), self_addr.as_ptr()), 0);
        assert_eq!(load_addr(SELF_ADDRESS_KEY), self_addr);
    }

    #[test]
    fn test_get_tier_info() {
        let _admin = setup();
        let r = get_tier_info(25);
        assert_eq!(r, 25);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 32);
        assert_eq!(bytes_to_u64(&ret[0..8]), 400); // init_margin
        assert_eq!(bytes_to_u64(&ret[8..16]), 200); // maint_margin
        assert_eq!(bytes_to_u64(&ret[16..24]), 700); // liq_penalty
        assert_eq!(bytes_to_u64(&ret[24..32]), 10); // funding applies once to notional
    }

    #[test]
    fn test_close_position_returns_unlock_amount() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        // Open with 500M margin at 2x
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(close_position(trader.as_ptr(), 1), 0);
        // Should return unlock amount (margin ± PnL at same mark price = margin)
        let ret = test_mock::get_return_data();
        let unlock = bytes_to_u64(&ret);
        assert_eq!(unlock, 500_000_000); // no price change → full margin returned
    }

    #[test]
    fn test_profitable_close_debits_insurance_and_escrow() {
        let admin = setup();
        let trader = [2u8; 32];
        let insurance_before = get_insurance_fund();
        test_mock::set_caller(trader);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        assert_eq!(load_u64(TOTAL_COLLATERAL_ESCROWED_KEY), 500_000_000);

        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 1_100_000_000);
        test_mock::set_caller(trader);
        assert_eq!(close_position(trader.as_ptr(), 1), 0);

        assert_eq!(get_insurance_fund(), insurance_before - 100_000_000);
        assert_eq!(load_u64(TOTAL_COLLATERAL_ESCROWED_KEY), 0);
    }

    #[test]
    fn test_losing_close_credits_insurance_and_escrow_clears() {
        let admin = setup();
        let trader = [2u8; 32];
        let insurance_before = get_insurance_fund();
        test_mock::set_caller(trader);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);

        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 900_000_000);
        test_mock::set_caller(trader);
        assert_eq!(close_position(trader.as_ptr(), 1), 0);

        assert_eq!(get_insurance_fund(), insurance_before + 100_000_000);
        assert_eq!(load_u64(TOTAL_COLLATERAL_ESCROWED_KEY), 0);
    }

    #[test]
    fn test_underwater_close_records_only_collectible_loss_as_insurance() {
        let admin = setup();
        let trader = [2u8; 32];
        let insurance_before = get_insurance_fund();
        test_mock::set_caller(trader);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        test_mock::set_caller(admin);
        assert_eq!(set_mark_price(admin.as_ptr(), 1, 400_000_000), 0);
        test_mock::set_caller(trader);
        assert_eq!(close_position(trader.as_ptr(), 1), 0);

        assert_eq!(get_insurance_fund(), insurance_before + 500_000_000);
        assert_eq!(get_bad_debt(), 100_000_000);
        assert_eq!(load_u64(TOTAL_PNL_LOSS_KEY), 600_000_000);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 0);
        assert_eq!(load_u64(TOTAL_COLLATERAL_ESCROWED_KEY), 0);
    }

    #[test]
    fn test_underwater_partial_close_tracks_proportional_bad_debt() {
        let admin = setup();
        let trader = [2u8; 32];
        let insurance_before = get_insurance_fund();
        test_mock::set_caller(trader);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        test_mock::set_caller(admin);
        assert_eq!(set_mark_price(admin.as_ptr(), 1, 400_000_000), 0);
        test_mock::set_caller(trader);
        assert_eq!(partial_close(trader.as_ptr(), 1, 500_000_000), 0);

        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_OPEN);
        assert_eq!(decode_pos_size(&data), 500_000_000);
        assert_eq!(decode_pos_margin(&data), 250_000_000);
        assert_eq!(get_insurance_fund(), insurance_before + 250_000_000);
        assert_eq!(get_bad_debt(), 50_000_000);
        assert_eq!(load_u64(TOTAL_PNL_LOSS_KEY), 300_000_000);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 0);
        assert_eq!(load_u64(TOTAL_COLLATERAL_ESCROWED_KEY), 250_000_000);
    }

    #[test]
    fn test_enable_margin_pair() {
        let admin = setup();
        // Pair 2 is NOT enabled
        assert_eq!(is_margin_enabled(2), 0);
        // Enable it
        assert_eq!(enable_margin_pair(admin.as_ptr(), 2), 0);
        assert_eq!(is_margin_enabled(2), 1);
    }

    #[test]
    fn test_disable_margin_pair() {
        let admin = setup();
        // Pair 1 was enabled in setup
        assert_eq!(is_margin_enabled(1), 1);
        assert_eq!(disable_margin_pair(admin.as_ptr(), 1), 0);
        assert_eq!(is_margin_enabled(1), 0);
    }

    #[test]
    fn test_enable_margin_pair_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        test_mock::set_caller(rando);
        assert_eq!(enable_margin_pair(rando.as_ptr(), 2), 1);
    }

    #[test]
    fn test_open_position_pair_not_enabled() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        // Pair 2 has no margin enabled — should return 7
        assert_eq!(
            open_position(trader.as_ptr(), 2, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            7
        );
    }

    #[test]
    fn test_disable_then_open_fails() {
        let admin = setup();
        // Disable pair 1
        assert_eq!(disable_margin_pair(admin.as_ptr(), 1), 0);
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        // Should fail with error 7 (pair not margin-enabled)
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            7
        );
    }

    // ---- COLLATERAL LOCKING TESTS (G6-01) ----

    #[test]
    fn test_collateral_lock_lifecycle() {
        // Verify collateral is tracked consistently through open → add → remove → close
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);

        // 1. Open position with 500M margin (locks 500M)
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin(&data), 500_000_000);

        // 2. Add 100M margin (locks additional 100M → total locked 600M)
        assert_eq!(add_margin(trader.as_ptr(), 1, 100_000_000), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin(&data), 600_000_000);

        // 3. Remove 50M margin (unlocks 50M → total locked 550M)
        assert_eq!(remove_margin(trader.as_ptr(), 1, 50_000_000), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin(&data), 550_000_000);

        // 4. Close position (unlocks all remaining)
        assert_eq!(close_position(trader.as_ptr(), 1), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_CLOSED);
    }

    #[test]
    fn test_add_margin_locks_collateral() {
        // Verify add_margin issues lock and updates storage correctly
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);

        // Add margin multiple times
        assert_eq!(add_margin(trader.as_ptr(), 1, 50_000_000), 0);
        assert_eq!(add_margin(trader.as_ptr(), 1, 25_000_000), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin(&data), 575_000_000); // 500M + 50M + 25M
    }

    #[test]
    fn test_remove_margin_unlocks_collateral() {
        // Verify remove_margin issues unlock and updates storage correctly
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        // 2x: maint = 25% = 250M needed for 1B notional
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);

        // Remove 100M (still above 25% maintenance)
        assert_eq!(remove_margin(trader.as_ptr(), 1, 100_000_000), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin(&data), 400_000_000);

        // Remove another 100M (400M - 100M = 300M, still > 250M)
        assert_eq!(remove_margin(trader.as_ptr(), 1, 100_000_000), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin(&data), 300_000_000);

        // Remove 60M more → 240M < 250M maintenance → should fail
        assert_eq!(remove_margin(trader.as_ptr(), 1, 60_000_000), 6);
        // Margin should remain unchanged
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin(&data), 300_000_000);
    }

    #[test]
    fn test_add_margin_to_closed_position_fails() {
        // Cannot add margin to a closed position
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        close_position(trader.as_ptr(), 1);
        // Position is now closed — add_margin should return 3 (not open)
        assert_eq!(add_margin(trader.as_ptr(), 1, 100), 3);
    }

    #[test]
    fn test_remove_margin_from_closed_position_fails() {
        // Cannot remove margin from a closed position
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);
        close_position(trader.as_ptr(), 1);
        // Position is now closed — remove_margin should return 3 (not open)
        assert_eq!(remove_margin(trader.as_ptr(), 1, 100), 3);
    }

    // ---- FUNDING RATE TESTS (G6-02) ----

    fn setup_with_index() -> [u8; 32] {
        let admin = setup();
        // Set index price for pair 1: 1.0 (same as mark initially)
        test_mock::set_caller(admin);
        assert_eq!(set_index_price(admin.as_ptr(), 1, 1_000_000_000), 0);
        admin
    }

    #[test]
    fn test_set_index_price() {
        let admin = setup();
        test_mock::set_caller(admin);
        assert_eq!(set_index_price(admin.as_ptr(), 1, 2_000_000_000), 0);
        let (price, ts) = load_index_price(1);
        assert_eq!(price, 2_000_000_000);
        assert!(ts > 0);
    }

    #[test]
    fn test_set_index_price_zero() {
        let admin = setup();
        test_mock::set_caller(admin);
        assert_eq!(set_index_price(admin.as_ptr(), 1, 0), 2);
    }

    #[test]
    fn test_set_index_price_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        test_mock::set_caller(rando);
        assert_eq!(set_index_price(rando.as_ptr(), 1, 1_000_000_000), 1);
    }

    #[test]
    fn test_apply_funding_too_early() {
        let _admin = setup_with_index();
        // apply_funding should return 1 (too early) since last_funding is 0
        // and slot is 1 (default), which is < FUNDING_INTERVAL_SLOTS
        test_mock::set_slot(100);
        assert_eq!(apply_funding(1), 1);
    }

    #[test]
    fn test_apply_funding_no_index_price() {
        let _admin = setup();
        // No index price set → return 2
        test_mock::set_slot(FUNDING_INTERVAL_SLOTS + 1);
        assert_eq!(apply_funding(1), 2);
    }

    #[test]
    fn test_apply_funding_no_positions() {
        let _admin = setup_with_index();
        // Set mark != index so there's a funding rate to compare
        test_mock::set_caller([1u8; 32]);
        set_mark_price([1u8; 32].as_ptr(), 1, 1_010_000_000);
        set_index_price([1u8; 32].as_ptr(), 1, 1_000_000_000);
        // Advancing an empty pair is still valid constant-time index work.
        test_mock::set_slot(FUNDING_INTERVAL_SLOTS + 1);
        assert_eq!(apply_funding(1), 0);
    }

    #[test]
    fn test_skew_funding_remains_live_when_oracle_mark_equals_index() {
        let admin = setup_with_index();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );
        assert_eq!(load_u64(&pair_long_size_key(1)), 1_000_000_000);
        assert_eq!(load_u64(&pair_short_size_key(1)), 0);

        test_mock::set_slot(100 + FUNDING_INTERVAL_SLOTS + 1);
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 1_000_000_000);
        set_index_price(admin.as_ptr(), 1, 1_000_000_000);
        assert_eq!(apply_funding(1), 0);
        assert_eq!(settle_position_funding(1), 0);
        assert_eq!(
            decode_pos_margin(&storage_get(&position_key(1)).unwrap()),
            499_000_000
        );
        assert_eq!(load_u64(FUNDING_POOL_KEY), 1_000_000);
    }

    #[test]
    fn test_apply_funding_mark_above_index() {
        // mark > index → longs pay, shorts receive
        let admin = setup_with_index();
        let trader = [2u8; 32];

        // Open position at mark = 1.0 (matching setup)
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        // Now shift mark above index for funding
        test_mock::set_slot(100 + FUNDING_INTERVAL_SLOTS + 1);
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 1_010_000_000); // 1.01
        set_index_price(admin.as_ptr(), 1, 1_000_000_000); // 1.0

        let result = apply_funding(1);
        // 0 = success (count in return_data)
        assert_eq!(result, 0);
        assert_eq!(settle_position_funding(1), 0);

        // Long should have paid: rate = 100 bps (1%), clamped to 100 bps
        // notional = 1B * 1.01 = 1.01B → scaled: 1_000_000_000 * 1_010_000_000 / 1e9 = 1_010_000_000
        // payment = notional * 100 * 10 / (10000*10) = notional * 100 / 10000 = notional * 1%
        // = 1_010_000_000 * 100 / 10000 = 10_100_000
        let data = storage_get(&position_key(1)).unwrap();
        let new_margin = decode_pos_margin(&data);
        // Long pays: margin decreased
        assert!(
            new_margin < 500_000_000,
            "Long margin should decrease when mark > index"
        );
        assert_eq!(new_margin, 500_000_000 - 10_100_000); // 489_900_000
    }

    #[test]
    fn test_apply_funding_mark_below_index() {
        // mark < index → shorts pay, longs receive
        let admin = setup_with_index();
        let trader = [2u8; 32];

        // Set mark to 0.99 (1% below index)
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 990_000_000); // 0.99
                                                        // Index stays at 1.0

        // Open a short position
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        assert_eq!(
            open_position(
                trader.as_ptr(),
                1,
                SIDE_SHORT,
                1_000_000_000,
                2,
                500_000_000
            ),
            0
        );

        // Advance past funding interval
        test_mock::set_slot(100 + FUNDING_INTERVAL_SLOTS + 1);
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 990_000_000);
        set_index_price(admin.as_ptr(), 1, 1_000_000_000);

        assert_eq!(apply_funding(1), 0);
        assert_eq!(settle_position_funding(1), 0);

        // Short should have paid: rate = 100 bps, mark < index so shorts pay
        // notional = 1B * 0.99 / 1e9 = 990_000_000
        // payment = 990_000_000 * 100 / 10000 = 9_900_000
        let data = storage_get(&position_key(1)).unwrap();
        let new_margin = decode_pos_margin(&data);
        assert!(
            new_margin < 500_000_000,
            "Short margin should decrease when mark < index"
        );
        assert_eq!(new_margin, 500_000_000 - 9_900_000); // 490_100_000
    }

    #[test]
    fn test_apply_funding_long_receives() {
        // mark < index → longs receive funding
        let admin = setup_with_index();
        let trader = [2u8; 32];

        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 990_000_000); // mark 0.99
                                                        // Index stays at 1.0

        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        test_mock::set_slot(100 + FUNDING_INTERVAL_SLOTS + 1);
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 990_000_000);
        set_index_price(admin.as_ptr(), 1, 1_000_000_000);

        assert_eq!(apply_funding(1), 0);

        assert_eq!(settle_position_funding(1), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_margin(&data), 500_000_000);
        assert_eq!(
            load_u64(&user_funding_claim_key(&trader)),
            8_910_000,
            "unfunded receiver entitlement must remain a non-spendable claim"
        );
        assert_eq!(load_u64(FUNDING_POOL_KEY), 0);
    }

    #[test]
    fn test_funding_v2_is_explicit_one_time_governance_activation() {
        test_mock::reset();
        let admin = [1u8; 32];
        let non_admin = [2u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(apply_funding(1), 4);

        test_mock::set_caller(non_admin);
        assert_eq!(activate_funding_v2(non_admin.as_ptr()), 1);
        test_mock::set_caller(admin);
        assert_eq!(activate_funding_v2(admin.as_ptr()), 0);
        assert_eq!(activate_funding_v2(admin.as_ptr()), 2);
    }

    #[test]
    fn test_legacy_open_positions_require_exact_migration_before_activation() {
        test_mock::reset();
        let admin = [1u8; 32];
        let trader = [2u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        let legacy = encode_position(
            &trader,
            1,
            7,
            SIDE_SHORT,
            POS_OPEN,
            123_000_000,
            50_000_000,
            1_000_000_000,
            2,
            10,
            0,
            0,
            MARGIN_MODE_ISOLATED,
            COLLATERAL_LUSD,
        );
        storage_set(&position_key(1), &legacy);
        save_u64(POSITION_COUNT_KEY, 1);

        assert_eq!(activate_funding_v2(admin.as_ptr()), 5);
        assert_eq!(finalize_and_activate_margin_v2(admin.as_ptr(), 0, 0), 6);
        assert_eq!(migrate_funding_v2_position(admin.as_ptr(), 1), 6);
        assert_eq!(begin_margin_v2_migration(admin.as_ptr()), 0);
        assert_eq!(begin_margin_v2_migration(admin.as_ptr()), 3);
        assert_eq!(migrate_funding_v2_position(admin.as_ptr(), 1), 0);
        assert_eq!(migrate_funding_v2_position(admin.as_ptr(), 1), 4);
        assert_eq!(load_u64(&pair_short_size_key(7)), 123_000_000);
        assert_eq!(finalize_funding_v2_migration(admin.as_ptr(), 0), 2);
        assert_eq!(finalize_funding_v2_migration(admin.as_ptr(), 1), 0);
        assert_eq!(finalize_and_activate_margin_v2(admin.as_ptr(), 0, 0), 3);
        assert_eq!(finalize_and_activate_margin_v2(admin.as_ptr(), 1, 1), 4);
        assert!(!funding_v2_enabled());
        assert!(!cross_v2_enabled());
        assert_eq!(finalize_and_activate_margin_v2(admin.as_ptr(), 1, 0), 0);
        assert!(funding_v2_enabled());
        assert!(cross_v2_enabled());
        assert!(margin_v2_migration_locked());
        assert!(is_paused());
        assert_eq!(complete_margin_v2_migration(admin.as_ptr()), 0);
        assert!(!margin_v2_migration_locked());
        assert!(!is_paused());
        assert_eq!(complete_margin_v2_migration(admin.as_ptr()), 2);
        assert_eq!(settle_position_funding(1), 0);
    }

    #[test]
    fn test_funding_is_zero_sum_when_receiver_settles_before_payer() {
        let admin = setup_with_index();
        let payer = [2u8; 32];
        let receiver = [3u8; 32];

        test_mock::set_caller(payer);
        test_mock::set_slot(100);
        assert_eq!(
            open_position(payer.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );
        test_mock::set_caller(receiver);
        assert_eq!(
            open_position(
                receiver.as_ptr(),
                1,
                SIDE_SHORT,
                1_000_000_000,
                2,
                500_000_000,
            ),
            0
        );
        let initial_escrow = load_u64(TOTAL_COLLATERAL_ESCROWED_KEY);

        test_mock::set_slot(100 + FUNDING_INTERVAL_SLOTS + 1);
        test_mock::set_caller(admin);
        assert_eq!(set_mark_price(admin.as_ptr(), 1, 1_010_000_000), 0);
        assert_eq!(set_index_price(admin.as_ptr(), 1, 1_000_000_000), 0);
        assert_eq!(apply_funding(1), 0);

        assert_eq!(settle_position_funding(2), 0);
        assert_eq!(decode_pos_margin(&storage_get(&position_key(2)).unwrap()), 500_000_000);
        assert_eq!(load_u64(&user_funding_claim_key(&receiver)), 10_100_000);
        assert_eq!(load_u64(FUNDING_POOL_KEY), 0);

        assert_eq!(settle_position_funding(1), 0);
        assert_eq!(load_u64(FUNDING_POOL_KEY), 10_100_000);
        assert_eq!(settle_position_funding(2), 0);

        let payer_margin = decode_pos_margin(&storage_get(&position_key(1)).unwrap());
        let receiver_margin = decode_pos_margin(&storage_get(&position_key(2)).unwrap());
        assert_eq!(payer_margin, 489_900_000);
        assert_eq!(receiver_margin, 510_100_000);
        assert_eq!(payer_margin + receiver_margin, 1_000_000_000);
        assert_eq!(load_u64(FUNDING_POOL_KEY), 0);
        assert_eq!(load_u64(FUNDING_TOTAL_CLAIMS_KEY), 0);
        assert_eq!(load_u64(TOTAL_COLLATERAL_ESCROWED_KEY), initial_escrow);
    }

    #[test]
    fn test_unmatched_funding_never_credits_more_than_collected() {
        let admin = setup_with_index();
        let payer = [2u8; 32];
        let receiver = [3u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(payer);
        assert_eq!(
            open_position(payer.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );
        test_mock::set_caller(receiver);
        assert_eq!(
            open_position(
                receiver.as_ptr(),
                1,
                SIDE_SHORT,
                2_000_000_000,
                2,
                1_000_000_000,
            ),
            0
        );

        test_mock::set_slot(100 + FUNDING_INTERVAL_SLOTS + 1);
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 1_010_000_000);
        set_index_price(admin.as_ptr(), 1, 1_000_000_000);
        assert_eq!(apply_funding(1), 0);
        assert_eq!(settle_position_funding(2), 0);
        assert_eq!(settle_position_funding(1), 0);
        assert_eq!(settle_position_funding(2), 0);

        assert_eq!(
            decode_pos_margin(&storage_get(&position_key(2)).unwrap()),
            1_009_797_000
        );
        assert_eq!(load_u64(&user_funding_claim_key(&receiver)), 9_797_000);
        assert_eq!(load_u64(FUNDING_TOTAL_CLAIMS_KEY), 9_797_000);
        assert_eq!(load_u64(FUNDING_POOL_KEY), 0);
    }

    #[test]
    fn test_closed_receiver_can_withdraw_later_pool_backed_claim() {
        let admin = setup_with_index();
        let payer = [2u8; 32];
        let receiver = [3u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(payer);
        assert_eq!(
            open_position(payer.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );
        test_mock::set_caller(receiver);
        assert_eq!(
            open_position(
                receiver.as_ptr(),
                1,
                SIDE_SHORT,
                1_000_000_000,
                2,
                500_000_000,
            ),
            0
        );

        test_mock::set_slot(100 + FUNDING_INTERVAL_SLOTS + 1);
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 1_010_000_000);
        set_index_price(admin.as_ptr(), 1, 1_000_000_000);
        assert_eq!(apply_funding(1), 0);
        assert_eq!(settle_position_funding(2), 0);

        test_mock::set_caller(receiver);
        assert_eq!(close_position(receiver.as_ptr(), 2), 0);
        assert_eq!(load_u64(&user_funding_claim_key(&receiver)), 10_100_000);

        assert_eq!(settle_position_funding(1), 0);
        assert_eq!(load_u64(FUNDING_POOL_KEY), 10_100_000);
        test_mock::set_caller(receiver);
        assert_eq!(claim_funding(receiver.as_ptr()), 0);
        assert_eq!(load_u64(&user_funding_claim_key(&receiver)), 0);
        assert_eq!(load_u64(FUNDING_POOL_KEY), 0);
        assert_eq!(load_u64(FUNDING_TOTAL_CLAIMS_KEY), 0);
    }

    #[test]
    fn test_funding_debt_is_collected_before_new_margin_becomes_spendable() {
        let admin = setup_with_index();
        let trader = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 100, 10_000_000),
            0
        );

        test_mock::set_slot(100 + FUNDING_INTERVAL_SLOTS + 1);
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 1_500_000_000);
        set_index_price(admin.as_ptr(), 1, 1_000_000_000);
        assert_eq!(apply_funding(1), 0);
        assert_eq!(settle_position_funding(1), 0);
        assert_eq!(decode_pos_margin(&storage_get(&position_key(1)).unwrap()), 0);
        assert_eq!(load_u64(&position_funding_debt_key(1)), 5_000_000);
        assert_eq!(load_u64(FUNDING_POOL_KEY), 10_000_000);

        test_mock::set_caller(trader);
        assert_eq!(add_margin(trader.as_ptr(), 1, 100_000_000), 0);
        assert_eq!(
            decode_pos_margin(&storage_get(&position_key(1)).unwrap()),
            95_000_000
        );
        assert_eq!(load_u64(&position_funding_debt_key(1)), 0);
        assert_eq!(load_u64(FUNDING_POOL_KEY), 15_000_000);
    }

    #[test]
    fn test_apply_funding_capped_at_max() {
        // Very large mark/index divergence → capped at MAX_FUNDING_RATE_BPS
        let admin = setup_with_index();
        let trader = [2u8; 32];

        // Open position at mark = 1.0 (matching setup)
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        // Set mark to 1.50 (50% above index) — would be 5000 bps, capped to 100
        test_mock::set_slot(100 + FUNDING_INTERVAL_SLOTS + 1);
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 1_500_000_000);
        set_index_price(admin.as_ptr(), 1, 1_000_000_000);

        assert_eq!(apply_funding(1), 0);
        assert_eq!(settle_position_funding(1), 0);

        // Rate = 5000bps but capped to 100bps (1%)
        // notional = 1B * 1.5 = 1_500_000_000
        // payment = 1_500_000_000 * 100 / 10000 = 15_000_000
        let data = storage_get(&position_key(1)).unwrap();
        let new_margin = decode_pos_margin(&data);
        assert_eq!(new_margin, 500_000_000 - 15_000_000); // 485_000_000
    }

    #[test]
    fn test_apply_funding_twice_blocked() {
        // Second apply within interval should fail
        let admin = setup_with_index();
        let trader = [2u8; 32];

        // Open position at mark = 1.0
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);

        let first_slot = 100 + FUNDING_INTERVAL_SLOTS + 1;
        test_mock::set_slot(first_slot);
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 1_010_000_000);
        set_index_price(admin.as_ptr(), 1, 1_000_000_000);

        apply_funding(1); // first: succeeds

        // Try again at same slot → too early
        assert_eq!(apply_funding(1), 1);

        // Advance but not enough
        test_mock::set_slot(first_slot + FUNDING_INTERVAL_SLOTS - 1);
        assert_eq!(apply_funding(1), 1);

        // Advance past next interval
        test_mock::set_slot(first_slot + FUNDING_INTERVAL_SLOTS + 1);
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 1_010_000_000);
        set_index_price(admin.as_ptr(), 1, 1_000_000_000);

        // Should succeed again (return 0 = success)
        assert_eq!(apply_funding(1), 0);
    }

    #[test]
    fn test_apply_funding_accumulated_funding_tracked() {
        // Verify accumulated_funding field is updated on positions
        let admin = setup_with_index();
        let trader = [2u8; 32];

        // Open position at mark = 1.0
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000);

        // Check initial accumulated_funding is 0
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_accumulated_funding(&data), 0);

        // Now shift mark above index
        test_mock::set_slot(100 + FUNDING_INTERVAL_SLOTS + 1);
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 1_010_000_000);
        set_index_price(admin.as_ptr(), 1, 1_000_000_000);

        assert_eq!(apply_funding(1), 0);
        assert_eq!(settle_position_funding(1), 0);

        // accumulated_funding should be updated (biased: values < 1<<63 mean paid)
        let data = storage_get(&position_key(1)).unwrap();
        let acc = decode_pos_accumulated_funding(&data);
        // Long paid 10.1M, so accumulated = (1<<63) - 10_100_000
        let zero_point = 1u64 << 63;
        assert!(
            acc < zero_point,
            "Long pays → accumulated funding below bias point"
        );
        assert_eq!(zero_point - acc, 10_100_000);
    }

    #[test]
    fn test_apply_funding_is_not_multiplied_by_leverage_twice() {
        // Funding is based on position notional, independent of margin leverage.
        let admin = setup_with_index();
        let trader = [2u8; 32];

        // Open a 10x position at mark = 1.0.
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        // 10x: init = 10%, need 100M margin for 1B notional at price 1.0
        assert_eq!(
            open_position(
                trader.as_ptr(),
                1,
                SIDE_LONG,
                1_000_000_000,
                10,
                500_000_000
            ),
            0
        );

        // Now shift mark above index
        test_mock::set_slot(100 + FUNDING_INTERVAL_SLOTS + 1);
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 1_010_000_000); // 1% above index
        set_index_price(admin.as_ptr(), 1, 1_000_000_000);

        assert_eq!(apply_funding(1), 0);
        assert_eq!(settle_position_funding(1), 0);

        // notional = 1_010_000_000, rate = 100bps, payment = 10_100_000.
        let data = storage_get(&position_key(1)).unwrap();
        let new_margin = decode_pos_margin(&data);
        assert_eq!(new_margin, 500_000_000 - 10_100_000);
    }

    // ============================================================================
    // G6-03 SECURITY TESTS: Oracle fallback handling
    // ============================================================================

    #[test]
    fn test_close_position_rejects_stale_oracle() {
        // G6-03: close_position must reject when oracle price is stale
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        test_mock::set_timestamp(1000);
        // Open position with fresh mark price
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        // Advance past the slot freshness bound without updating oracle.
        test_mock::set_timestamp(1000 + MAX_PRICE_AGE_SLOTS + 1);
        test_mock::set_caller(trader);
        // close_position should return 5 (oracle unavailable)
        assert_eq!(close_position(trader.as_ptr(), 1), 5);

        // Position should still be OPEN
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_OPEN);
    }

    #[test]
    fn test_close_position_rejects_missing_oracle() {
        // G6-03: close_position must reject when no oracle price exists for the pair
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(admin);
        // Enable pair 99 which has no mark price
        enable_margin_pair(admin.as_ptr(), 99);
        // Manually write a position for pair 99 to bypass open_position mark check
        let pos_id = 1u64;
        save_u64(POSITION_COUNT_KEY, pos_id);
        let mut pos = alloc::vec![0u8; POSITION_SIZE];
        pos[0..32].copy_from_slice(&trader); // trader
        pos[32..40].copy_from_slice(&u64_to_bytes(pos_id)); // id
        pos[40] = POS_OPEN; // status
        pos[41] = SIDE_LONG; // side
        pos[42..50].copy_from_slice(&u64_to_bytes(1_000_000_000)); // size
        pos[50..58].copy_from_slice(&u64_to_bytes(1_000_000_000)); // entry_price
        pos[58..66].copy_from_slice(&u64_to_bytes(500_000_000)); // margin
        pos[66..74].copy_from_slice(&u64_to_bytes(99)); // pair_id
        pos[74..82].copy_from_slice(&u64_to_bytes(2)); // leverage
        storage_set(&position_key(pos_id), &pos);

        test_mock::set_caller(trader);
        // No mark price for pair 99 → error 5
        assert_eq!(close_position(trader.as_ptr(), 1), 5);
    }

    #[test]
    fn test_close_position_succeeds_with_fresh_oracle() {
        // G6-03: close_position succeeds when oracle is fresh
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        test_mock::set_timestamp(1000);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        // Refresh oracle within staleness window
        test_mock::set_timestamp(1500);
        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 1_000_000_000);
        test_mock::set_caller(trader);
        assert_eq!(close_position(trader.as_ptr(), 1), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_CLOSED);
    }

    #[test]
    fn test_close_position_limit_long_success() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        test_mock::set_timestamp(1000);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 1_020_000_000);

        test_mock::set_caller(trader);
        assert_eq!(close_position_limit(trader.as_ptr(), 1, 1_010_000_000), 0);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_CLOSED);
    }

    #[test]
    fn test_close_position_limit_long_not_met() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        test_mock::set_timestamp(1000);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        // Current mark is 1.0, so requiring >= 1.1 should fail.
        assert_eq!(close_position_limit(trader.as_ptr(), 1, 1_100_000_000), 6);
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_OPEN);
    }

    #[test]
    fn test_partial_close_limit_long_success() {
        let admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        test_mock::set_timestamp(1000);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        test_mock::set_caller(admin);
        set_mark_price(admin.as_ptr(), 1, 1_020_000_000);

        test_mock::set_caller(trader);
        assert_eq!(
            partial_close_limit(trader.as_ptr(), 1, 500_000_000, 1_010_000_000),
            0
        );
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_OPEN);
        assert_eq!(decode_pos_size(&data), 500_000_000);
    }

    #[test]
    fn test_partial_close_limit_long_not_met() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        test_mock::set_timestamp(1000);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        // Current mark is 1.0, so requiring >= 1.1 should fail.
        assert_eq!(
            partial_close_limit(trader.as_ptr(), 1, 500_000_000, 1_100_000_000),
            6
        );
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_OPEN);
        assert_eq!(decode_pos_size(&data), 1_000_000_000);
    }

    #[test]
    fn test_remove_margin_rejects_stale_oracle() {
        // G6-03: remove_margin must reject when oracle is stale
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        test_mock::set_timestamp(1000);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        // Advance past staleness window
        test_mock::set_timestamp(1000 + MAX_PRICE_AGE_SLOTS + 1);
        test_mock::set_caller(trader);
        assert_eq!(remove_margin(trader.as_ptr(), 1, 1000), 7); // stale oracle
    }

    #[test]
    fn test_partial_close_rejects_stale_oracle() {
        // G6-03: partial_close_position must reject when oracle is stale
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        test_mock::set_timestamp(1000);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        test_mock::set_timestamp(1000 + MAX_PRICE_AGE_SLOTS + 1);
        test_mock::set_caller(trader);
        assert_eq!(partial_close(trader.as_ptr(), 1, 500_000_000), 5);

        // Position still OPEN
        let data = storage_get(&position_key(1)).unwrap();
        assert_eq!(decode_pos_status(&data), POS_OPEN);
    }

    // === G2-04: query_user_open_position ===

    #[test]
    fn test_query_user_open_position_found() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        test_mock::set_timestamp(1000);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        // Query should find the open position on pair 1
        let pos_id = query_user_open_position(trader.as_ptr(), 1);
        assert_eq!(pos_id, 1);
    }

    #[test]
    fn test_query_user_open_position_wrong_pair() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        test_mock::set_timestamp(1000);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        // Pair 2 doesn't exist for this trader — should return 0
        // (need to enable margin for pair 2 first, but query doesn't check that)
        let pos_id = query_user_open_position(trader.as_ptr(), 2);
        assert_eq!(pos_id, 0);
    }

    #[test]
    fn test_query_user_open_position_closed() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100);
        test_mock::set_timestamp(1000);
        assert_eq!(
            open_position(trader.as_ptr(), 1, SIDE_LONG, 1_000_000_000, 2, 500_000_000),
            0
        );

        // Close the position
        test_mock::set_caller(trader);
        test_mock::set_timestamp(1001);
        assert_eq!(close_position(trader.as_ptr(), 1), 0);

        // Query should return 0 — no open positions
        let pos_id = query_user_open_position(trader.as_ptr(), 1);
        assert_eq!(pos_id, 0);
    }

    #[test]
    fn test_query_user_open_position_no_positions() {
        let _admin = setup();
        let trader = [2u8; 32];

        // Trader has no positions at all
        let pos_id = query_user_open_position(trader.as_ptr(), 1);
        assert_eq!(pos_id, 0);
    }

    // ========================================================================
    // AUDIT-FIX MARGIN-1: Oracle integration tests
    // ========================================================================

    #[test]
    fn test_set_oracle_contract_admin_only() {
        let admin = setup();
        let oracle_addr = [0xAA; 32];
        let non_admin = [2u8; 32];

        // Non-admin should fail
        test_mock::set_caller(non_admin);
        assert_eq!(
            set_oracle_contract(non_admin.as_ptr(), oracle_addr.as_ptr()),
            1
        );

        // Admin should succeed
        test_mock::set_caller(admin);
        assert_eq!(set_oracle_contract(admin.as_ptr(), oracle_addr.as_ptr()), 0);

        // Verify stored
        let stored = load_addr(ORACLE_ADDRESS_KEY);
        assert_eq!(stored, oracle_addr);
    }

    #[test]
    fn test_set_oracle_contract_zero_and_reconfiguration_rejected() {
        let admin = setup();
        let first = [0xAA; 32];
        let second = [0xAB; 32];

        test_mock::set_caller(admin);
        assert_eq!(set_oracle_contract(admin.as_ptr(), [0u8; 32].as_ptr()), 2);

        test_mock::set_caller(admin);
        assert_eq!(set_oracle_contract(admin.as_ptr(), first.as_ptr()), 0);

        test_mock::set_caller(admin);
        assert_eq!(set_oracle_contract(admin.as_ptr(), second.as_ptr()), 3);
        assert_eq!(load_addr(ORACLE_ADDRESS_KEY), first);
    }

    #[test]
    fn test_oracle_market_binding_is_admin_only_and_bounded() {
        let admin = setup();
        let non_admin = [2u8; 32];
        let market = b"wSOL/LICN";

        test_mock::set_caller(non_admin);
        assert_eq!(
            set_oracle_market(
                non_admin.as_ptr(),
                4,
                market.as_ptr(),
                market.len() as u32,
            ),
            1
        );
        test_mock::set_caller(admin);
        assert_eq!(
            set_oracle_market(admin.as_ptr(), 4, market.as_ptr(), market.len() as u32),
            0
        );
        assert_eq!(storage_get(&oracle_market_key(4)), Some(market.to_vec()));

        let invalid = b"wSOL/USD";
        assert_eq!(
            set_oracle_market(admin.as_ptr(), 4, invalid.as_ptr(), invalid.len() as u32),
            2
        );
        let oversized = [b'A'; MAX_ORACLE_MARKET_BYTES + 1];
        assert_eq!(
            set_oracle_market(
                admin.as_ptr(),
                4,
                oversized.as_ptr(),
                oversized.len() as u32,
            ),
            2
        );
    }

    #[test]
    fn test_update_mark_price_no_oracle_configured() {
        let _admin = setup();
        let caller = [2u8; 32];
        let asset = b"LICN";

        // No oracle set — should return 1
        test_mock::set_caller(caller);
        let r =
            update_mark_price_from_oracle(caller.as_ptr(), 1, asset.as_ptr(), asset.len() as u32);
        assert_eq!(r, 1, "Should fail when oracle not configured");
    }

    #[test]
    fn test_update_mark_price_oracle_call_success() {
        let admin = setup();
        let oracle_addr = [0xBB; 32];

        // Set oracle address
        test_mock::set_caller(admin);
        assert_eq!(set_oracle_contract(admin.as_ptr(), oracle_addr.as_ptr()), 0);
        let market = b"LICN";
        assert_eq!(
            set_oracle_market(admin.as_ptr(), 1, market.as_ptr(), market.len() as u32),
            0
        );

        // Mock oracle return: $2.00 with 8 oracle decimals.
        let price: u64 = 200_000_000;
        test_mock::set_cross_call_response(Some(oracle_quote_response(price, 999)));

        // Call update
        let caller = [3u8; 32];
        test_mock::set_caller(caller);
        let asset = b"LICN";
        let r =
            update_mark_price_from_oracle(caller.as_ptr(), 1, asset.as_ptr(), asset.len() as u32);
        assert_eq!(r, 0, "Should succeed with valid oracle response");

        let (target, function, args, value) =
            test_mock::get_last_cross_call().expect("oracle cross-call captured");
        assert_eq!(target, oracle_addr);
        assert_eq!(function, "get_price_value");
        assert_eq!(value, 0);
        assert_eq!(&args[..3], &[0xAB, 32, 4]);
        assert_eq!(&args[3..3 + asset.len()], asset);
        assert_eq!(
            u32::from_le_bytes(args[35..39].try_into().unwrap()),
            asset.len() as u32
        );

        // Verify mark price was updated
        let (stored_price, source_slot) = load_mark_price(1);
        assert_eq!(stored_price, 2_000_000_000);
        assert_eq!(source_slot, 999);
    }

    #[test]
    fn test_update_mark_price_oracle_returns_zero() {
        let admin = setup();
        let oracle_addr = [0xCC; 32];

        test_mock::set_caller(admin);
        assert_eq!(set_oracle_contract(admin.as_ptr(), oracle_addr.as_ptr()), 0);
        let market = b"LICN";
        assert_eq!(
            set_oracle_market(admin.as_ptr(), 1, market.as_ptr(), market.len() as u32),
            0
        );

        // Mock oracle return: 0 price
        test_mock::set_cross_call_response(Some(oracle_quote_response(0, 999)));

        let caller = [4u8; 32];
        test_mock::set_caller(caller);
        let asset = b"LICN";
        let r =
            update_mark_price_from_oracle(caller.as_ptr(), 1, asset.as_ptr(), asset.len() as u32);
        assert_eq!(r, 4, "Should reject zero price from oracle");
    }

    #[test]
    fn test_update_mark_price_rejects_market_substitution_and_future_quote() {
        let admin = setup();
        let oracle_addr = [0xCD; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_oracle_contract(admin.as_ptr(), oracle_addr.as_ptr()), 0);
        let market = b"wSOL";
        assert_eq!(
            set_oracle_market(admin.as_ptr(), 2, market.as_ptr(), market.len() as u32),
            0
        );

        let caller = [4u8; 32];
        test_mock::set_caller(caller);
        let substituted = b"wBTC";
        assert_eq!(
            update_mark_price_from_oracle(
                caller.as_ptr(),
                2,
                substituted.as_ptr(),
                substituted.len() as u32,
            ),
            5
        );

        test_mock::set_cross_call_response(Some(oracle_quote_response(
            8_000_000_000,
            get_timestamp() + 1,
        )));
        assert_eq!(
            update_mark_price_from_oracle(
                caller.as_ptr(),
                2,
                market.as_ptr(),
                market.len() as u32,
            ),
            3
        );
    }

    #[test]
    fn test_update_cross_market_uses_licn_basis_and_oldest_source_slot() {
        let admin = setup();
        let oracle_addr = [0xCE; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_oracle_contract(admin.as_ptr(), oracle_addr.as_ptr()), 0);
        let market = b"wSOL/LICN";
        assert_eq!(
            set_oracle_market(admin.as_ptr(), 4, market.as_ptr(), market.len() as u32),
            0
        );
        test_mock::set_cross_call_responses(vec![
            oracle_quote_response(8_000_000_000, 999),
            oracle_quote_response(10_000_000, 998),
        ]);

        let caller = [5u8; 32];
        test_mock::set_caller(caller);
        assert_eq!(
            update_mark_price_from_oracle(
                caller.as_ptr(),
                4,
                market.as_ptr(),
                market.len() as u32,
            ),
            0
        );
        let (price, source_slot) = load_mark_price(4);
        assert_eq!(price, 800_000_000_000);
        assert_eq!(source_slot, 998);
    }
}
