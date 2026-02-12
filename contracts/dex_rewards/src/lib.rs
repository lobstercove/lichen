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

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    storage_get, storage_set, log_info,
    bytes_to_u64, u64_to_bytes, get_slot,
};

// ============================================================================
// CONSTANTS
// ============================================================================

const REWARD_POOL_PER_MONTH: u64 = 1_000_000_000_000_000; // 1M MOLT (in shells)
const SLOTS_PER_MONTH: u64 = 2_592_000;

// Tier thresholds (cumulative volume in shells)
const TIER_BRONZE_MAX: u64 = 10_000_000_000;       // <10k MOLT
const TIER_SILVER_MAX: u64 = 100_000_000_000;      // 10k-100k MOLT
const TIER_GOLD_MAX: u64 = 1_000_000_000_000;      // 100k-1M MOLT
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

pub fn initialize(admin: *const u8) -> u32 {
    let existing = load_addr(ADMIN_KEY);
    if !is_zero(&existing) { return 1; }
    let mut addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(admin, addr.as_mut_ptr(), 32); }
    storage_set(ADMIN_KEY, &addr);
    save_u64(TOTAL_DISTRIBUTED_KEY, 0);
    save_u64(REWARD_EPOCH_KEY, 0);
    storage_set(PAUSED_KEY, &[0u8]);
    log_info("DEX Rewards initialized");
    0
}

/// Record a trade for reward calculation (called by dex_core)
/// Returns: 0=success
pub fn record_trade(trader: *const u8, fee_paid: u64, volume: u64) -> u32 {
    let mut t = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(trader, t.as_mut_ptr(), 32); }

    // Update cumulative volume
    let current_vol = load_u64(&trader_volume_key(&t));
    save_u64(&trader_volume_key(&t), current_vol + volume);

    // Calculate reward based on tier
    let tier = get_tier(current_vol + volume);
    let multiplier = get_multiplier(tier);
    let base_reward = fee_paid; // 1:1 fee mining
    let reward = base_reward * multiplier / 10_000;

    // Add to pending
    let pending = load_u64(&trader_pending_key(&t));
    save_u64(&trader_pending_key(&t), pending + reward);

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
                save_u64(&referrer_earnings_key(&referrer), ref_earnings + ref_bonus);
            }
        }
    }

    0
}

/// Claim trading rewards
/// Returns: 0=success, 1=nothing to claim, 2=paused, 3=reentrancy
pub fn claim_trading_rewards(trader: *const u8) -> u32 {
    if !reentrancy_enter() { return 3; }
    if !require_not_paused() { reentrancy_exit(); return 2; }
    let mut t = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(trader, t.as_mut_ptr(), 32); }

    let pending = load_u64(&trader_pending_key(&t));
    if pending == 0 { reentrancy_exit(); return 1; }

    // Transfer (in production: cross-call MoltCoin.transfer)
    save_u64(&trader_pending_key(&t), 0);
    let claimed = load_u64(&trader_claimed_key(&t));
    save_u64(&trader_claimed_key(&t), claimed + pending);

    let total = load_u64(TOTAL_DISTRIBUTED_KEY);
    save_u64(TOTAL_DISTRIBUTED_KEY, total + pending);

    moltchain_sdk::set_return_data(&u64_to_bytes(pending));
    log_info("Trading rewards claimed");
    reentrancy_exit();
    0
}

/// Claim LP rewards for a position
pub fn claim_lp_rewards(provider: *const u8, position_id: u64) -> u32 {
    if !reentrancy_enter() { return 3; }
    if !require_not_paused() { reentrancy_exit(); return 2; }
    let mut p = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(provider, p.as_mut_ptr(), 32); }

    let lp_k = lp_pending_key(position_id);
    let pending = load_u64(&lp_k);
    if pending == 0 { reentrancy_exit(); return 1; }

    save_u64(&lp_k, 0);
    let total = load_u64(TOTAL_DISTRIBUTED_KEY);
    save_u64(TOTAL_DISTRIBUTED_KEY, total + pending);

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

/// Set reward rate for a pair (admin only)
pub fn set_reward_rate(caller: *const u8, pair_id: u64, rate_per_slot: u64) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    if !require_admin(&c) { return 1; }
    save_u64(&pair_reward_rate_key(pair_id), rate_per_slot);
    0
}

/// Accrue LP rewards for a position (called periodically)
pub fn accrue_lp_rewards(position_id: u64, liquidity: u64, pair_id: u64) -> u32 {
    let rate = load_u64(&pair_reward_rate_key(pair_id));
    if rate == 0 { return 1; }
    let reward = liquidity * rate / 1_000_000_000;
    let lp_k = lp_pending_key(position_id);
    let current = load_u64(&lp_k);
    save_u64(&lp_k, current + reward);
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

pub fn emergency_pause(caller: *const u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32); }
    if !require_admin(&c) { return 1; }
    storage_set(PAUSED_KEY, &[1u8]);
    log_info("DEX Rewards: EMERGENCY PAUSE");
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
        assert_eq!(claim_trading_rewards(trader.as_ptr()), 0);
        assert_eq!(load_u64(&trader_pending_key(&trader)), 0);
        assert!(load_u64(&trader_claimed_key(&trader)) > 0);
        assert!(get_total_distributed() > 0);
    }

    #[test]
    fn test_claim_nothing() {
        let _admin = setup();
        let trader = [2u8; 32];
        assert_eq!(claim_trading_rewards(trader.as_ptr()), 1);
    }

    #[test]
    fn test_register_referral() {
        let _admin = setup();
        let trader = [2u8; 32];
        let referrer = [3u8; 32];
        test_mock::set_slot(100);
        assert_eq!(register_referral(trader.as_ptr(), referrer.as_ptr()), 0);
        assert_eq!(load_u64(&referrer_count_key(&referrer)), 1);
    }

    #[test]
    fn test_register_referral_self() {
        let _admin = setup();
        let trader = [2u8; 32];
        assert_eq!(register_referral(trader.as_ptr(), trader.as_ptr()), 2);
    }

    #[test]
    fn test_register_referral_duplicate() {
        let _admin = setup();
        let trader = [2u8; 32];
        let ref1 = [3u8; 32];
        let ref2 = [4u8; 32];
        test_mock::set_slot(100);
        assert_eq!(register_referral(trader.as_ptr(), ref1.as_ptr()), 0);
        assert_eq!(register_referral(trader.as_ptr(), ref2.as_ptr()), 1);
    }

    #[test]
    fn test_referral_bonus() {
        let _admin = setup();
        let trader = [2u8; 32];
        let referrer = [3u8; 32];
        test_mock::set_slot(100);
        register_referral(trader.as_ptr(), referrer.as_ptr());
        record_trade(trader.as_ptr(), 10_000, 10_000_000);
        let ref_earnings = load_u64(&referrer_earnings_key(&referrer));
        assert_eq!(ref_earnings, 1000); // 10% of 10000
    }

    #[test]
    fn test_lp_rewards() {
        let admin = setup();
        set_reward_rate(admin.as_ptr(), 1, 1_000_000);
        assert_eq!(accrue_lp_rewards(1, 100_000, 1), 0);
        let pending = load_u64(&lp_pending_key(1));
        assert!(pending > 0);
    }

    #[test]
    fn test_claim_lp_rewards() {
        let admin = setup();
        let provider = [2u8; 32];
        set_reward_rate(admin.as_ptr(), 1, 1_000_000);
        accrue_lp_rewards(1, 100_000, 1);
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
        assert_eq!(set_reward_rate(admin.as_ptr(), 1, 500_000), 0);
        assert_eq!(load_u64(&pair_reward_rate_key(1)), 500_000);
    }

    #[test]
    fn test_set_reward_rate_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        assert_eq!(set_reward_rate(rando.as_ptr(), 1, 500_000), 1);
    }

    #[test]
    fn test_emergency_pause() {
        let admin = setup();
        assert_eq!(emergency_pause(admin.as_ptr()), 0);
        assert!(is_paused());
        let trader = [2u8; 32];
        record_trade(trader.as_ptr(), 5000, 5_000_000);
        assert_eq!(claim_trading_rewards(trader.as_ptr()), 2); // paused
    }

    #[test]
    fn test_set_referral_rate() {
        let admin = setup();
        assert_eq!(get_referral_rate(), REFERRAL_RATE_BPS); // default 1000
        assert_eq!(set_referral_rate(admin.as_ptr(), 2000), 0);
        assert_eq!(get_referral_rate(), 2000);
    }

    #[test]
    fn test_set_referral_rate_cap() {
        let admin = setup();
        assert_eq!(set_referral_rate(admin.as_ptr(), 3001), 2); // over 30%
        assert_eq!(set_referral_rate(admin.as_ptr(), 3000), 0); // exactly 30%
    }

    #[test]
    fn test_set_referral_rate_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        assert_eq!(set_referral_rate(rando.as_ptr(), 500), 1);
    }

    #[test]
    fn test_dynamic_referral_rate() {
        let admin = setup();
        let trader = [2u8; 32];
        let referrer = [3u8; 32];
        test_mock::set_slot(100);
        register_referral(trader.as_ptr(), referrer.as_ptr());
        // Set rate to 20%
        set_referral_rate(admin.as_ptr(), 2000);
        record_trade(trader.as_ptr(), 10_000, 10_000_000);
        let ref_earnings = load_u64(&referrer_earnings_key(&referrer));
        assert_eq!(ref_earnings, 2000); // 20% of 10000
    }
}
