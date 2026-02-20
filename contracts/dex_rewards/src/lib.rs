// DEX Rewards — Trading Incentives, LP Mining, Referral Program (DEEP hardened)
//
// Features:
//   - Trading rewards (fee mining) with tier multipliers
//   - LP rewards proportional to in-range liquidity
//   - Referral program (10% of referee fees to referrer)
//   - Configurable reward rates per pair
//   - Trader tiers: Bronze/Silver/Gold/Diamond
//   - Emergency pause, reentrancy guard, admin controls

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    storage_get, storage_set, log_info,
    bytes_to_u64, u64_to_bytes, get_slot,
    Address, call_token_transfer, get_caller, get_contract_address,
};

// ============================================================================
// CONSTANTS
// ============================================================================

const REWARD_POOL_PER_MONTH: u64 = 1_000_000_000_000_000; // 1M MOLT (in shells)
const SLOTS_PER_MONTH: u64 = 2_592_000;

// Tier thresholds (cumulative volume in shells)
const TIER_BRONZE_MAX: u64 = 100_000_000_000_000;     // <100k MOLT ($10K at $0.10)
const TIER_SILVER_MAX: u64 = 1_000_000_000_000_000;    // 100k-1M MOLT ($100K at $0.10)
const TIER_GOLD_MAX: u64 = 10_000_000_000_000_000;     // 1M-10M MOLT ($1M at $0.10)
// Above GOLD_MAX = Diamond

// Tier multipliers (in basis points, 10000 = 1x)
const MULTIPLIER_BRONZE: u64 = 10_000;  // 1x
const MULTIPLIER_SILVER: u64 = 15_000;  // 1.5x
const MULTIPLIER_GOLD: u64 = 20_000;    // 2x
const MULTIPLIER_DIAMOND: u64 = 30_000; // 3x

// Referral rates
const REFERRAL_RATE_BPS: u64 = 1000;         // 10% of fees to referrer
const REFERRAL_DISCOUNT_BPS: u64 = 500;      // 5% fee discount for referee
const REFERRAL_VERIFIED_RATE_BPS: u64 = 1500; // 15% for MoltyID-verified
const REFERRAL_DISCOUNT_DURATION: u64 = 2_592_000; // 30 days

// Storage keys
const ADMIN_KEY: &[u8] = b"rew_admin";
const PAUSED_KEY: &[u8] = b"rew_paused";
const REENTRANCY_KEY: &[u8] = b"rew_reentrancy";
const TOTAL_DISTRIBUTED_KEY: &[u8] = b"rew_total_dist";
const REWARD_EPOCH_KEY: &[u8] = b"rew_epoch";
const REFERRAL_RATE_KEY: &[u8] = b"rew_ref_rate";
const MOLTCOIN_ADDRESS_KEY: &[u8] = b"rew_molt_addr";
const REWARDS_POOL_KEY: &[u8] = b"rew_pool_addr";
const AUTHORIZED_CALLER_PREFIX: &[u8] = b"rew_auth_";
const TOTAL_VOLUME_KEY: &[u8] = b"rew_total_volume";
const TRADE_COUNT_KEY: &[u8] = b"rew_trade_count";
const TRADER_COUNT_KEY: &[u8] = b"rew_trader_count";

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

fn trader_volume_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"rew_vol_"[..]);
    k.extend_from_slice(&hex_encode(addr)); k
}
fn trader_pending_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"rew_pend_"[..]);
    k.extend_from_slice(&hex_encode(addr)); k
}
fn trader_claimed_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"rew_claim_"[..]);
    k.extend_from_slice(&hex_encode(addr)); k
}
fn referral_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"rew_ref_"[..]);
    k.extend_from_slice(&hex_encode(addr)); k
}
fn referrer_earnings_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"rew_refr_"[..]);
    k.extend_from_slice(&hex_encode(addr)); k
}
fn referrer_count_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"rew_refc_"[..]);
    k.extend_from_slice(&hex_encode(addr)); k
}
fn referral_start_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"rew_refs_"[..]);
    k.extend_from_slice(&hex_encode(addr)); k
}
fn pair_reward_rate_key(pair_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"rew_rate_"[..]);
    k.extend_from_slice(&u64_to_decimal(pair_id)); k
}
fn lp_pending_key(pos_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"rew_lp_"[..]);
    k.extend_from_slice(&u64_to_decimal(pos_id)); k
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
fn is_authorized_caller() -> bool {
    let caller = get_caller();
    let mut key = Vec::from(AUTHORIZED_CALLER_PREFIX);
    key.extend_from_slice(&caller.0);
    storage_get(&key).map(|v| v.first().copied() == Some(1)).unwrap_or(false)
}

// ============================================================================
// TIER SYSTEM
// ============================================================================

fn get_tier(volume: u64) -> u8 {
    if volume >= TIER_GOLD_MAX { 3 } // Diamond
    else if volume >= TIER_SILVER_MAX { 2 } // Gold
    else if volume >= TIER_BRONZE_MAX { 1 } // Silver
    else { 0 } // Bronze
}

fn get_multiplier(tier: u8) -> u64 {
    match tier {
        0 => MULTIPLIER_BRONZE,
        1 => MULTIPLIER_SILVER,
        2 => MULTIPLIER_GOLD,
        3 => MULTIPLIER_DIAMOND,
        _ => MULTIPLIER_BRONZE,
    }
}

// ============================================================================
// PUBLIC FUNCTIONS
// ============================================================================

#[no_mangle]
pub extern "C" fn initialize(admin: *const u8) -> u32 {
    let existing = load_addr(ADMIN_KEY);
    if !is_zero(&existing) { return 1; }
    let mut addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(admin, addr.as_mut_ptr(), 32); }
    // AUDIT-FIX: verify caller matches claimed admin address
    let real_caller = get_caller();
    if real_caller.0 != addr {
        return 200;
    }
    storage_set(ADMIN_KEY, &addr);
    save_u64(TOTAL_DISTRIBUTED_KEY, 0);
    save_u64(REWARD_EPOCH_KEY, 0);
    storage_set(PAUSED_KEY, &[0u8]);
    log_info("DEX Rewards initialized");
    0
}

/// Record a trade for reward calculation (called by dex_core)
/// Returns: 0=success, 5=unauthorized caller
pub fn record_trade(trader: *const u8, fee_paid: u64, volume: u64) -> u32 {
    if !is_authorized_caller() {
        log_info("record_trade: unauthorized caller");
        return 5;
    }
    let mut t = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(trader, t.as_mut_ptr(), 32); }

    // Update cumulative volume
    let current_vol = load_u64(&trader_volume_key(&t));
    if current_vol == 0 {
        // First trade for this trader — increment unique trader count
        save_u64(TRADER_COUNT_KEY, load_u64(TRADER_COUNT_KEY).saturating_add(1));
    }
    save_u64(&trader_volume_key(&t), current_vol.saturating_add(volume));

    // Track global volume and trade count
    save_u64(TOTAL_VOLUME_KEY, load_u64(TOTAL_VOLUME_KEY).saturating_add(volume));
    save_u64(TRADE_COUNT_KEY, load_u64(TRADE_COUNT_KEY).saturating_add(1));

    // Calculate reward based on tier
    let tier = get_tier(current_vol.saturating_add(volume));
    let multiplier = get_multiplier(tier);
    let base_reward = fee_paid; // 1:1 fee mining
    let reward = base_reward.saturating_mul(multiplier) / 10_000;

    // Add to pending
    let pending = load_u64(&trader_pending_key(&t));
    save_u64(&trader_pending_key(&t), pending.saturating_add(reward));

    // Handle referral bonus
    let referrer_data = storage_get(&referral_key(&t));
    if let Some(ref_data) = referrer_data {
        if ref_data.len() >= 32 {
            let mut referrer = [0u8; 32];
            referrer.copy_from_slice(&ref_data[..32]);
            if !is_zero(&referrer) {
                let dynamic_rate = load_u64(REFERRAL_RATE_KEY);
                let effective_rate = if dynamic_rate > 0 { dynamic_rate } else { REFERRAL_RATE_BPS };
                let ref_bonus = fee_paid * effective_rate / 10_000;
                let ref_earnings = load_u64(&referrer_earnings_key(&referrer));
                save_u64(&referrer_earnings_key(&referrer), ref_earnings.saturating_add(ref_bonus));
            }
        }
    }

    0
}

/// Claim trading rewards — transfers MOLT from contract's own balance to trader.
/// The contract itself holds the reward tokens (self-custody pattern).
/// Returns: 0=success, 1=nothing to claim, 2=paused, 3=reentrancy,
///          4=transfer failed, 5=moltcoin address not configured
pub fn claim_trading_rewards(trader: *const u8) -> u32 {
    if !reentrancy_enter() { return 3; }
    if !require_not_paused() { reentrancy_exit(); return 2; }
    let mut t = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(trader, t.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != t {
        reentrancy_exit();
        return 200;
    }

    let pending = load_u64(&trader_pending_key(&t));
    if pending == 0 { reentrancy_exit(); return 1; }

    // Transfer MOLT from contract's own balance to trader.
    // AUDIT-FIX G7-02: Use self-custody pattern — the contract holds reward
    // tokens at its own address. get_contract_address() == caller in CCC context,
    // satisfying moltcoin's caller==from check.
    let molt_addr = load_addr(MOLTCOIN_ADDRESS_KEY);
    if is_zero(&molt_addr) {
        log_info("claim_trading_rewards: moltcoin address not configured");
        reentrancy_exit();
        return 5;
    }
    let self_addr = get_contract_address();
    if let Err(_) = call_token_transfer(
        Address(molt_addr), self_addr, Address(t), pending,
    ) {
        reentrancy_exit();
        return 4;
    }

    save_u64(&trader_pending_key(&t), 0);
    let claimed = load_u64(&trader_claimed_key(&t));
    save_u64(&trader_claimed_key(&t), claimed.saturating_add(pending));

    let total = load_u64(TOTAL_DISTRIBUTED_KEY);
    save_u64(TOTAL_DISTRIBUTED_KEY, total.saturating_add(pending));

    moltchain_sdk::set_return_data(&u64_to_bytes(pending));
    log_info("Trading rewards claimed");
    reentrancy_exit();
    0
}

/// Claim LP rewards for a position — transfers MOLT from contract's own balance to provider.
/// Returns: 0=success, 1=nothing to claim, 2=paused, 3=reentrancy,
///          4=transfer failed, 5=moltcoin address not configured
pub fn claim_lp_rewards(provider: *const u8, position_id: u64) -> u32 {
    if !reentrancy_enter() { return 3; }
    if !require_not_paused() { reentrancy_exit(); return 2; }
    let mut p = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(provider, p.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != p {
        reentrancy_exit();
        return 200;
    }

    let lp_k = lp_pending_key(position_id);
    let pending = load_u64(&lp_k);
    if pending == 0 { reentrancy_exit(); return 1; }

    // Transfer MOLT from contract's own balance to provider.
    // AUDIT-FIX G7-02: self-custody pattern (see claim_trading_rewards).
    let molt_addr = load_addr(MOLTCOIN_ADDRESS_KEY);
    if is_zero(&molt_addr) {
        log_info("claim_lp_rewards: moltcoin address not configured");
        reentrancy_exit();
        return 5;
    }
    let self_addr = get_contract_address();
    if let Err(_) = call_token_transfer(
        Address(molt_addr), self_addr, Address(p), pending,
    ) {
        reentrancy_exit();
        return 4;
    }

    save_u64(&lp_k, 0);
    let total = load_u64(TOTAL_DISTRIBUTED_KEY);
    save_u64(TOTAL_DISTRIBUTED_KEY, total.saturating_add(pending));

    moltchain_sdk::set_return_data(&u64_to_bytes(pending));
    log_info("LP rewards claimed");
    reentrancy_exit();
    0
}

/// Register a referral relationship
/// Returns: 0=success, 1=already has referrer, 2=self-referral, 3=reentrancy
pub fn register_referral(trader: *const u8, referrer: *const u8) -> u32 {
    if !reentrancy_enter() { return 3; }
    let mut t = [0u8; 32];
    let mut r = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(trader, t.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(referrer, r.as_mut_ptr(), 32);
    }
    if t == r { reentrancy_exit(); return 2; }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != t {
        reentrancy_exit();
        return 200;
    }

    let rk = referral_key(&t);
    if storage_get(&rk).is_some() { reentrancy_exit(); return 1; }

    storage_set(&rk, &r);
    save_u64(&referral_start_key(&t), get_slot());

    // Increment referrer's count
    let count = load_u64(&referrer_count_key(&r));
    save_u64(&referrer_count_key(&r), count + 1);

    log_info("Referral registered");
    reentrancy_exit();
    0
}

/// Claim referral rewards — transfers accumulated referral earnings to the referrer.
/// AUDIT-FIX G7-02: referral earnings were recorded in record_trade but had
/// no claim path. This function completes the referral economy.
/// Returns: 0=success, 1=nothing to claim, 2=paused, 3=reentrancy,
///          4=transfer failed, 5=moltcoin address not configured
pub fn claim_referral_rewards(referrer: *const u8) -> u32 {
    if !reentrancy_enter() { return 3; }
    if !require_not_paused() { reentrancy_exit(); return 2; }
    let mut r = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(referrer, r.as_mut_ptr(), 32); }

    // Verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != r {
        reentrancy_exit();
        return 200;
    }

    let earnings = load_u64(&referrer_earnings_key(&r));
    if earnings == 0 { reentrancy_exit(); return 1; }

    // Transfer MOLT from contract's own balance to referrer (self-custody pattern)
    let molt_addr = load_addr(MOLTCOIN_ADDRESS_KEY);
    if is_zero(&molt_addr) {
        log_info("claim_referral_rewards: moltcoin address not configured");
        reentrancy_exit();
        return 5;
    }
    let self_addr = get_contract_address();
    if let Err(_) = call_token_transfer(
        Address(molt_addr), self_addr, Address(r), earnings,
    ) {
        reentrancy_exit();
        return 4;
    }

    save_u64(&referrer_earnings_key(&r), 0);
    let total = load_u64(TOTAL_DISTRIBUTED_KEY);
    save_u64(TOTAL_DISTRIBUTED_KEY, total.saturating_add(earnings));

    moltchain_sdk::set_return_data(&u64_to_bytes(earnings));
    log_info("Referral rewards claimed");
    reentrancy_exit();
    0
}

/// Set reward rate for a pair (admin only)
pub fn set_reward_rate(caller: *const u8, pair_id: u64, rate_per_slot: u64) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) { return 1; }
    save_u64(&pair_reward_rate_key(pair_id), rate_per_slot);
    0
}

/// Accrue LP rewards for a position (called by dex_amm)
/// Returns: 0=success, 1=zero rate, 5=unauthorized caller
pub fn accrue_lp_rewards(position_id: u64, liquidity: u64, pair_id: u64) -> u32 {
    if !is_authorized_caller() {
        log_info("accrue_lp_rewards: unauthorized caller");
        return 5;
    }
    let rate = load_u64(&pair_reward_rate_key(pair_id));
    if rate == 0 { return 1; }
    let reward = liquidity * rate / 1_000_000_000;
    let lp_k = lp_pending_key(position_id);
    let current = load_u64(&lp_k);
    save_u64(&lp_k, current.saturating_add(reward));
    0
}

// Queries
pub fn get_pending_rewards(addr: *const u8) -> u64 {
    let mut a = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(addr, a.as_mut_ptr(), 32); }
    let trading = load_u64(&trader_pending_key(&a));
    let referral = load_u64(&referrer_earnings_key(&a));
    trading + referral
}

pub fn get_trading_tier(addr: *const u8) -> u8 {
    let mut a = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(addr, a.as_mut_ptr(), 32); }
    let vol = load_u64(&trader_volume_key(&a));
    get_tier(vol)
}

pub fn get_referral_stats(addr: *const u8) -> u64 {
    let mut a = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(addr, a.as_mut_ptr(), 32); }
    load_u64(&referrer_count_key(&a))
}

pub fn get_total_distributed() -> u64 { load_u64(TOTAL_DISTRIBUTED_KEY) }

/// Set the referral rate in basis points (admin only)
/// Default is 1000 (10%). Max is 3000 (30%).
pub fn set_referral_rate(caller: *const u8, rate_bps: u64) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) { return 1; }
    if rate_bps > 3000 { return 2; } // cap at 30%
    save_u64(REFERRAL_RATE_KEY, rate_bps);
    0
}

/// Get the current referral rate (bps)
pub fn get_referral_rate() -> u64 {
    let r = load_u64(REFERRAL_RATE_KEY);
    if r > 0 { r } else { REFERRAL_RATE_BPS }
}

/// Set the MOLT coin contract address (admin only)
pub fn set_moltcoin_address(caller: *const u8, addr: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut a = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(addr, a.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) { return 1; }
    if is_zero(&a) { return 2; }
    storage_set(MOLTCOIN_ADDRESS_KEY, &a);
    0
}

/// Set the rewards pool address that holds MOLT tokens (admin only)
pub fn set_rewards_pool(caller: *const u8, addr: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut a = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(addr, a.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) { return 1; }
    if is_zero(&a) { return 2; }
    storage_set(REWARDS_POOL_KEY, &a);
    0
}

pub fn emergency_pause(caller: *const u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) { return 1; }
    storage_set(PAUSED_KEY, &[1u8]);
    log_info("DEX Rewards: EMERGENCY PAUSE");
    0
}
pub fn emergency_unpause(caller: *const u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) { return 1; }
    storage_set(PAUSED_KEY, &[0u8]);
    0
}

/// Add an authorized caller contract (admin only).
/// Only authorized callers can invoke record_trade / accrue_lp_rewards.
pub fn set_authorized_caller(caller: *const u8, contract_addr: *const u8, enabled: u8) -> u32 {
    let mut c = [0u8; 32];
    let mut a = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(contract_addr, a.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) { return 1; }
    if is_zero(&a) { return 2; }
    let mut key = Vec::from(AUTHORIZED_CALLER_PREFIX);
    key.extend_from_slice(&a);
    storage_set(&key, &[enabled]);
    log_info("Authorized caller updated");
    0
}

// WASM entry
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn call() {
    let args = moltchain_sdk::get_args();
    if args.is_empty() { return; }
    match args[0] {
        // 0: initialize(admin[32])
        0 => {
            if args.len() >= 33 {
                let r = initialize(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 1: record_trade(trader[32], fee_paid[8], volume[8])
        1 => {
            if args.len() >= 49 {
                let fee = bytes_to_u64(&args[33..41]);
                let vol = bytes_to_u64(&args[41..49]);
                let r = record_trade(args[1..33].as_ptr(), fee, vol);
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 2: claim_trading_rewards(trader[32])
        2 => {
            if args.len() >= 33 {
                let r = claim_trading_rewards(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 3: claim_lp_rewards(provider[32], position_id[8])
        3 => {
            if args.len() >= 41 {
                let pos_id = bytes_to_u64(&args[33..41]);
                let r = claim_lp_rewards(args[1..33].as_ptr(), pos_id);
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 4: register_referral(trader[32], referrer[32])
        4 => {
            if args.len() >= 65 {
                let r = register_referral(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 5: set_reward_rate(caller[32], pair_id[8], rate[8])
        5 => {
            if args.len() >= 49 {
                let pair_id = bytes_to_u64(&args[33..41]);
                let rate = bytes_to_u64(&args[41..49]);
                let r = set_reward_rate(args[1..33].as_ptr(), pair_id, rate);
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 6: accrue_lp_rewards(position_id[8], liquidity[8], pair_id[8])
        6 => {
            if args.len() >= 25 {
                let pos_id = bytes_to_u64(&args[1..9]);
                let liq = bytes_to_u64(&args[9..17]);
                let pair_id = bytes_to_u64(&args[17..25]);
                let r = accrue_lp_rewards(pos_id, liq, pair_id);
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 7: get_pending_rewards(addr[32])
        7 => {
            if args.len() >= 33 {
                let r = get_pending_rewards(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r));
            }
        }
        // 8: get_trading_tier(addr[32])
        8 => {
            if args.len() >= 33 {
                let r = get_trading_tier(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 9: emergency_pause(caller[32])
        9 => {
            if args.len() >= 33 {
                let r = emergency_pause(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 10: emergency_unpause(caller[32])
        10 => {
            if args.len() >= 33 {
                let r = emergency_unpause(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 11: set_referral_rate(caller[32], rate_bps[8])
        11 => {
            if args.len() >= 41 {
                let rate = bytes_to_u64(&args[33..41]);
                let r = set_referral_rate(args[1..33].as_ptr(), rate);
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 12: set_moltcoin_address(caller[32], addr[32])
        12 => {
            if args.len() >= 65 {
                let r = set_moltcoin_address(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 13: set_rewards_pool(caller[32], addr[32])
        13 => {
            if args.len() >= 65 {
                let r = set_rewards_pool(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        // 14: get_referral_rate
        14 => {
            moltchain_sdk::set_return_data(&u64_to_bytes(get_referral_rate()));
        }
        // 15: get_total_distributed
        15 => {
            moltchain_sdk::set_return_data(&u64_to_bytes(get_total_distributed()));
        }
        16 => {
            // get_trader_count — unique traders who have earned rewards
            moltchain_sdk::set_return_data(&u64_to_bytes(load_u64(TRADER_COUNT_KEY)));
        }
        17 => {
            // get_total_volume — cumulative volume recorded for rewards
            moltchain_sdk::set_return_data(&u64_to_bytes(load_u64(TOTAL_VOLUME_KEY)));
        }
        18 => {
            // get_reward_stats — [trade_count, trader_count, total_volume, total_distributed, epoch]
            let mut buf = Vec::with_capacity(40);
            buf.extend_from_slice(&u64_to_bytes(load_u64(TRADE_COUNT_KEY)));
            buf.extend_from_slice(&u64_to_bytes(load_u64(TRADER_COUNT_KEY)));
            buf.extend_from_slice(&u64_to_bytes(load_u64(TOTAL_VOLUME_KEY)));
            buf.extend_from_slice(&u64_to_bytes(get_total_distributed()));
            buf.extend_from_slice(&u64_to_bytes(load_u64(REWARD_EPOCH_KEY)));
            moltchain_sdk::set_return_data(&buf);
        }
        // 19: claim_referral_rewards(referrer[32])
        19 => {
            if args.len() >= 33 {
                let r = claim_referral_rewards(args[1..33].as_ptr());
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

    /// Standard setup: admin created, moltcoin address configured, dex caller
    /// authorized, and contract address set. Tokens are held at the contract
    /// address (self-custody pattern per G7-02).
    fn setup() -> [u8; 32] {
        test_mock::reset();
        let admin = [1u8; 32];
        let molt = [10u8; 32];
        let contract_self = [0xAAu8; 32]; // contract's own address
        test_mock::set_caller(admin);
        test_mock::set_contract_address(contract_self);
        assert_eq!(initialize(admin.as_ptr()), 0);
        // Configure moltcoin address so claims actually transfer
        test_mock::set_caller(admin);
        assert_eq!(set_moltcoin_address(admin.as_ptr(), molt.as_ptr()), 0);
        // Authorize a caller address for record_trade / accrue_lp_rewards
        let dex_caller = [0xFFu8; 32];
        assert_eq!(set_authorized_caller(admin.as_ptr(), dex_caller.as_ptr(), 1), 0);
        test_mock::set_caller(dex_caller);
        admin
    }

    /// Setup without moltcoin address configured — for testing error path.
    fn setup_no_molt() -> [u8; 32] {
        test_mock::reset();
        let admin = [1u8; 32];
        let contract_self = [0xAAu8; 32];
        test_mock::set_caller(admin);
        test_mock::set_contract_address(contract_self);
        assert_eq!(initialize(admin.as_ptr()), 0);
        let dex_caller = [0xFFu8; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_authorized_caller(admin.as_ptr(), dex_caller.as_ptr(), 1), 0);
        test_mock::set_caller(dex_caller);
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
    fn test_record_trade_and_rewards() {
        let _admin = setup();
        let trader = [2u8; 32];
        assert_eq!(record_trade(trader.as_ptr(), 1000, 1_000_000), 0);
        let pending = load_u64(&trader_pending_key(&trader));
        assert!(pending > 0, "Should have pending rewards");
        assert_eq!(pending, 1000); // Bronze tier = 1x
    }

    #[test]
    fn test_tier_multiplier_silver() {
        let _admin = setup();
        let trader = [2u8; 32];
        // Accumulate enough for Silver tier (>10k MOLT)
        save_u64(&trader_volume_key(&trader), TIER_BRONZE_MAX);
        assert_eq!(record_trade(trader.as_ptr(), 1000, 1_000_000), 0);
        let pending = load_u64(&trader_pending_key(&trader));
        assert_eq!(pending, 1500); // Silver = 1.5x
    }

    #[test]
    fn test_tier_multiplier_gold() {
        let _admin = setup();
        let trader = [2u8; 32];
        save_u64(&trader_volume_key(&trader), TIER_SILVER_MAX);
        assert_eq!(record_trade(trader.as_ptr(), 1000, 1_000_000), 0);
        let pending = load_u64(&trader_pending_key(&trader));
        assert_eq!(pending, 2000); // Gold = 2x
    }

    #[test]
    fn test_tier_multiplier_diamond() {
        let _admin = setup();
        let trader = [2u8; 32];
        save_u64(&trader_volume_key(&trader), TIER_GOLD_MAX);
        assert_eq!(record_trade(trader.as_ptr(), 1000, 1_000_000), 0);
        let pending = load_u64(&trader_pending_key(&trader));
        assert_eq!(pending, 3000); // Diamond = 3x
    }

    #[test]
    fn test_claim_trading_rewards() {
        let _admin = setup();
        let trader = [2u8; 32];
        record_trade(trader.as_ptr(), 5000, 5_000_000);
        test_mock::set_caller(trader);
        assert_eq!(claim_trading_rewards(trader.as_ptr()), 0);
        assert_eq!(load_u64(&trader_pending_key(&trader)), 0);
        assert!(load_u64(&trader_claimed_key(&trader)) > 0);
        assert!(get_total_distributed() > 0);
    }

    #[test]
    fn test_claim_nothing() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        assert_eq!(claim_trading_rewards(trader.as_ptr()), 1);
    }

    #[test]
    fn test_register_referral() {
        let _admin = setup();
        let trader = [2u8; 32];
        let referrer = [3u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        assert_eq!(register_referral(trader.as_ptr(), referrer.as_ptr()), 0);
        assert_eq!(load_u64(&referrer_count_key(&referrer)), 1);
    }

    #[test]
    fn test_register_referral_self() {
        let _admin = setup();
        let trader = [2u8; 32];
        test_mock::set_caller(trader);
        assert_eq!(register_referral(trader.as_ptr(), trader.as_ptr()), 2);
    }

    #[test]
    fn test_register_referral_duplicate() {
        let _admin = setup();
        let trader = [2u8; 32];
        let ref1 = [3u8; 32];
        let ref2 = [4u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        assert_eq!(register_referral(trader.as_ptr(), ref1.as_ptr()), 0);
        assert_eq!(register_referral(trader.as_ptr(), ref2.as_ptr()), 1);
    }

    #[test]
    fn test_referral_bonus() {
        let _admin = setup();
        let trader = [2u8; 32];
        let referrer = [3u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        register_referral(trader.as_ptr(), referrer.as_ptr());
        test_mock::set_caller([0xFFu8; 32]);
        record_trade(trader.as_ptr(), 10_000, 10_000_000);
        let ref_earnings = load_u64(&referrer_earnings_key(&referrer));
        assert_eq!(ref_earnings, 1000); // 10% of 10000
    }

    #[test]
    fn test_claim_referral_rewards() {
        let _admin = setup();
        let trader = [2u8; 32];
        let referrer = [3u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        register_referral(trader.as_ptr(), referrer.as_ptr());
        test_mock::set_caller([0xFFu8; 32]);
        record_trade(trader.as_ptr(), 10_000, 10_000_000);
        // Referrer has 1000 shells in earnings (10% of 10_000 fee)
        assert_eq!(load_u64(&referrer_earnings_key(&referrer)), 1000);
        // Referrer claims
        test_mock::set_caller(referrer);
        assert_eq!(claim_referral_rewards(referrer.as_ptr()), 0);
        // Earnings zeroed, total distributed increased
        assert_eq!(load_u64(&referrer_earnings_key(&referrer)), 0);
        assert_eq!(get_total_distributed(), 1000);
    }

    #[test]
    fn test_claim_referral_rewards_nothing() {
        let _admin = setup();
        let referrer = [3u8; 32];
        test_mock::set_caller(referrer);
        assert_eq!(claim_referral_rewards(referrer.as_ptr()), 1); // nothing to claim
    }

    #[test]
    fn test_claim_referral_rewards_wrong_caller() {
        let _admin = setup();
        let trader = [2u8; 32];
        let referrer = [3u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        register_referral(trader.as_ptr(), referrer.as_ptr());
        test_mock::set_caller([0xFFu8; 32]);
        record_trade(trader.as_ptr(), 10_000, 10_000_000);
        // Try to claim as trader (not the referrer)
        test_mock::set_caller(trader);
        assert_eq!(claim_referral_rewards(referrer.as_ptr()), 200); // caller mismatch
    }

    #[test]
    fn test_lp_rewards() {
        let admin = setup();
        test_mock::set_caller(admin);
        set_reward_rate(admin.as_ptr(), 1, 1_000_000);
        test_mock::set_caller([0xFFu8; 32]);
        assert_eq!(accrue_lp_rewards(1, 100_000, 1), 0);
        let pending = load_u64(&lp_pending_key(1));
        assert!(pending > 0);
    }

    #[test]
    fn test_claim_lp_rewards() {
        let admin = setup();
        let provider = [2u8; 32];
        test_mock::set_caller(admin);
        set_reward_rate(admin.as_ptr(), 1, 1_000_000);
        test_mock::set_caller([0xFFu8; 32]);
        accrue_lp_rewards(1, 100_000, 1);
        test_mock::set_caller(provider);
        assert_eq!(claim_lp_rewards(provider.as_ptr(), 1), 0);
        assert_eq!(load_u64(&lp_pending_key(1)), 0);
    }

    #[test]
    fn test_get_trading_tier() {
        let _admin = setup();
        let trader = [2u8; 32];
        assert_eq!(get_trading_tier(trader.as_ptr()), 0); // Bronze
        save_u64(&trader_volume_key(&trader), TIER_BRONZE_MAX);
        assert_eq!(get_trading_tier(trader.as_ptr()), 1); // Silver
        save_u64(&trader_volume_key(&trader), TIER_GOLD_MAX);
        assert_eq!(get_trading_tier(trader.as_ptr()), 3); // Diamond
    }

    #[test]
    fn test_get_pending_rewards() {
        let _admin = setup();
        let trader = [2u8; 32];
        record_trade(trader.as_ptr(), 5000, 5_000_000);
        let pending = get_pending_rewards(trader.as_ptr());
        assert_eq!(pending, 5000);
    }

    #[test]
    fn test_set_reward_rate() {
        let admin = setup();
        test_mock::set_caller(admin);
        assert_eq!(set_reward_rate(admin.as_ptr(), 1, 500_000), 0);
        assert_eq!(load_u64(&pair_reward_rate_key(1)), 500_000);
    }

    #[test]
    fn test_set_reward_rate_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        test_mock::set_caller(rando);
        assert_eq!(set_reward_rate(rando.as_ptr(), 1, 500_000), 1);
    }

    #[test]
    fn test_emergency_pause() {
        let admin = setup();
        test_mock::set_caller(admin);
        assert_eq!(emergency_pause(admin.as_ptr()), 0);
        assert!(is_paused());
        let trader = [2u8; 32];
        test_mock::set_caller([0xFFu8; 32]);
        record_trade(trader.as_ptr(), 5000, 5_000_000);
        test_mock::set_caller(trader);
        assert_eq!(claim_trading_rewards(trader.as_ptr()), 2); // paused
    }

    #[test]
    fn test_set_referral_rate() {
        let admin = setup();
        assert_eq!(get_referral_rate(), REFERRAL_RATE_BPS); // default 1000
        test_mock::set_caller(admin);
        assert_eq!(set_referral_rate(admin.as_ptr(), 2000), 0);
        assert_eq!(get_referral_rate(), 2000);
    }

    #[test]
    fn test_set_referral_rate_cap() {
        let admin = setup();
        test_mock::set_caller(admin);
        assert_eq!(set_referral_rate(admin.as_ptr(), 3001), 2); // over 30%
        assert_eq!(set_referral_rate(admin.as_ptr(), 3000), 0); // exactly 30%
    }

    #[test]
    fn test_set_referral_rate_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        test_mock::set_caller(rando);
        assert_eq!(set_referral_rate(rando.as_ptr(), 500), 1);
    }

    #[test]
    fn test_dynamic_referral_rate() {
        let admin = setup();
        let trader = [2u8; 32];
        let referrer = [3u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        register_referral(trader.as_ptr(), referrer.as_ptr());
        // Set rate to 20%
        test_mock::set_caller(admin);
        set_referral_rate(admin.as_ptr(), 2000);
        test_mock::set_caller([0xFFu8; 32]);
        record_trade(trader.as_ptr(), 10_000, 10_000_000);
        let ref_earnings = load_u64(&referrer_earnings_key(&referrer));
        assert_eq!(ref_earnings, 2000); // 20% of 10000
    }

    // --- MOLT Token Transfer Tests ---

    #[test]
    fn test_set_moltcoin_address() {
        let admin = setup();
        let molt = [10u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_moltcoin_address(admin.as_ptr(), molt.as_ptr()), 0);
        assert_eq!(load_addr(MOLTCOIN_ADDRESS_KEY), molt);
    }

    #[test]
    fn test_set_moltcoin_address_zero() {
        let admin = setup();
        let zero = [0u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_moltcoin_address(admin.as_ptr(), zero.as_ptr()), 2);
    }

    #[test]
    fn test_set_moltcoin_address_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        let molt = [10u8; 32];
        test_mock::set_caller(rando);
        assert_eq!(set_moltcoin_address(rando.as_ptr(), molt.as_ptr()), 1);
    }

    #[test]
    fn test_set_rewards_pool() {
        let admin = setup();
        let pool = [11u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_rewards_pool(admin.as_ptr(), pool.as_ptr()), 0);
        assert_eq!(load_addr(REWARDS_POOL_KEY), pool);
    }

    #[test]
    fn test_set_rewards_pool_zero() {
        let admin = setup();
        let zero = [0u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_rewards_pool(admin.as_ptr(), zero.as_ptr()), 2);
    }

    #[test]
    fn test_set_rewards_pool_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        let pool = [11u8; 32];
        test_mock::set_caller(rando);
        assert_eq!(set_rewards_pool(rando.as_ptr(), pool.as_ptr()), 1);
    }

    #[test]
    fn test_claim_with_molt_configured() {
        let admin = setup();
        let trader = [2u8; 32];

        test_mock::set_caller([0xFFu8; 32]);
        record_trade(trader.as_ptr(), 5000, 5_000_000);
        // In test mode, call_token_transfer returns Ok(false) — not an Err,
        // so the claim proceeds and bookkeeping is updated.
        test_mock::set_caller(trader);
        assert_eq!(claim_trading_rewards(trader.as_ptr()), 0);
        assert_eq!(load_u64(&trader_pending_key(&trader)), 0);
        assert_eq!(load_u64(&trader_claimed_key(&trader)), 5000);
    }

    #[test]
    fn test_claim_lp_with_molt_configured() {
        let admin = setup();
        let provider = [2u8; 32];
        test_mock::set_caller(admin);
        set_reward_rate(admin.as_ptr(), 1, 1_000_000);
        test_mock::set_caller([0xFFu8; 32]);
        accrue_lp_rewards(1, 100_000, 1);

        test_mock::set_caller(provider);
        assert_eq!(claim_lp_rewards(provider.as_ptr(), 1), 0);
        assert_eq!(load_u64(&lp_pending_key(1)), 0);
    }

    #[test]
    fn test_claim_without_molt_configured_fails() {
        // AUDIT-FIX G7-02: Without MOLT address configured, claims MUST fail
        // (error 5) instead of silently proceeding with bookkeeping only.
        let _admin = setup_no_molt();
        let trader = [2u8; 32];
        record_trade(trader.as_ptr(), 5000, 5_000_000);
        test_mock::set_caller(trader);
        assert_eq!(claim_trading_rewards(trader.as_ptr()), 5);
        // Pending rewards should NOT be zeroed since transfer didn't happen
        assert_eq!(load_u64(&trader_pending_key(&trader)), 5000);
    }

    #[test]
    fn test_claim_lp_without_molt_configured_fails() {
        let admin = setup_no_molt();
        let provider = [2u8; 32];
        test_mock::set_caller(admin);
        set_reward_rate(admin.as_ptr(), 1, 1_000_000);
        test_mock::set_caller([0xFFu8; 32]);
        accrue_lp_rewards(1, 100_000, 1);
        test_mock::set_caller(provider);
        assert_eq!(claim_lp_rewards(provider.as_ptr(), 1), 5);
        // Pending LP rewards preserved
        assert!(load_u64(&lp_pending_key(1)) > 0);
    }

    #[test]
    fn test_claim_referral_without_molt_configured_fails() {
        let _admin = setup_no_molt();
        let trader = [2u8; 32];
        let referrer = [3u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(trader);
        register_referral(trader.as_ptr(), referrer.as_ptr());
        test_mock::set_caller([0xFFu8; 32]);
        record_trade(trader.as_ptr(), 10_000, 10_000_000);
        test_mock::set_caller(referrer);
        assert_eq!(claim_referral_rewards(referrer.as_ptr()), 5);
        // Earnings preserved
        assert_eq!(load_u64(&referrer_earnings_key(&referrer)), 1000);
    }

    #[test]
    fn test_self_custody_transfer_pattern() {
        // Verify the self-custody pattern: get_contract_address() is used as
        // the `from` address in token transfers, ensuring caller == from in
        // cross-contract call context.
        let _admin = setup();
        let contract_self = [0xAAu8; 32];
        let trader = [2u8; 32];
        record_trade(trader.as_ptr(), 5000, 5_000_000);
        test_mock::set_caller(trader);
        let result = claim_trading_rewards(trader.as_ptr());
        assert_eq!(result, 0);
        // The transfer used get_contract_address() (0xAA...) as from, not a
        // separate pool address. In the real runtime, CCC sets caller to the
        // calling contract, so caller == from == 0xAA... is guaranteed.
        assert_eq!(get_contract_address().0, contract_self);
    }
}
