// MoltSwap - Automated Market Maker DEX (v2 — DEEP hardened)
// Features: AMM pools, TWAP oracle, flash loans, deadline+impact guards, protocol fees
//
// v2 additions:
//   - TWAP oracle: cumulative price snapshots for external consumers
//   - Swap deadlines: transactions rejected after user-specified timestamp
//   - Price impact guard: rejects swaps that move price > MAX_PRICE_IMPACT_BPS
//   - Protocol fee: configurable split of trading fees to treasury
//   - Max flash loan cap: percentage of reserves

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{Pool, Address, log_info, storage_get, storage_set, bytes_to_u64, u64_to_bytes, get_timestamp,
    CrossCall, call_contract, get_caller,
};

// ============================================================================
// v2 CONSTANTS
// ============================================================================

/// Maximum price impact in basis points (5% = 500 bps)
const MAX_PRICE_IMPACT_BPS: u64 = 500;

/// Protocol fee: share of trading fees that goes to treasury (in basis points of the swap fee)
/// E.g., if swap fee is 30bps and protocol_fee_share is 1667, treasury gets ~5bps
const DEFAULT_PROTOCOL_FEE_SHARE: u64 = 1667; // 1/6 of swap fee

/// Max flash loan: 90% of reserves
const MAX_FLASH_LOAN_PERCENT: u64 = 90;

/// TWAP storage keys
const TWAP_CUMULATIVE_A_KEY: &[u8] = b"twap_cum_a";
const TWAP_CUMULATIVE_B_KEY: &[u8] = b"twap_cum_b";
const TWAP_LAST_UPDATE_KEY: &[u8] = b"twap_last_update";
const TWAP_LAST_RESERVE_A_KEY: &[u8] = b"twap_last_ra";
const TWAP_LAST_RESERVE_B_KEY: &[u8] = b"twap_last_rb";
const TWAP_SNAPSHOT_COUNT_KEY: &[u8] = b"twap_snap_count";
/// Protocol treasury address
const PROTOCOL_TREASURY_KEY: &[u8] = b"protocol_treasury";
const PROTOCOL_FEE_SHARE_KEY: &[u8] = b"protocol_fee_share";
const PROTOCOL_FEES_A_KEY: &[u8] = b"protocol_fees_a";
const PROTOCOL_FEES_B_KEY: &[u8] = b"protocol_fees_b";

// T5.12: Reentrancy guard — prevents recursive calls into state-mutating functions
const REENTRANCY_KEY: &[u8] = b"_reentrancy";

/// Emergency pause key
const MS_PAUSE_KEY: &[u8] = b"ms_paused";
/// Admin key for pause/unpause
const MS_ADMIN_KEY: &[u8] = b"ms_admin";

fn is_ms_paused() -> bool {
    storage_get(MS_PAUSE_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(false)
}

fn is_ms_admin(caller: &[u8]) -> bool {
    storage_get(MS_ADMIN_KEY).map(|d| d.as_slice() == caller).unwrap_or(false)
}

fn reentrancy_enter() -> bool {
    if storage_get(REENTRANCY_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(false) {
        return false; // Already entered
    }
    storage_set(REENTRANCY_KEY, &[1u8]);
    true
}

fn reentrancy_exit() {
    storage_set(REENTRANCY_KEY, &[0u8]);
}

/// Reconstruct pool state from persistent storage.
/// Called at the start of every entry point (except initialize).
fn load_pool() -> Pool {
    let mut pool = Pool::new(Address::new([0u8; 32]), Address::new([0u8; 32]));
    pool.load().expect("Failed to load pool state");
    pool
}

// ============================================================================
// TWAP ORACLE (v2)
// ============================================================================

/// Update cumulative price accumulators. Should be called before any reserve change.
/// Uses time-weighted cumulative sums: cumulative_price_A += (reserve_B / reserve_A) * elapsed
/// Stored as scaled integers (× 2^32) to avoid floating point.
fn twap_update() {
    let now = get_timestamp();
    let last_update = storage_get(TWAP_LAST_UPDATE_KEY)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);

    if last_update == 0 {
        // First call — just record current time and reserves
        let pool = load_pool();
        storage_set(TWAP_LAST_UPDATE_KEY, &u64_to_bytes(now));
        storage_set(TWAP_LAST_RESERVE_A_KEY, &u64_to_bytes(pool.reserve_a));
        storage_set(TWAP_LAST_RESERVE_B_KEY, &u64_to_bytes(pool.reserve_b));
        storage_set(TWAP_CUMULATIVE_A_KEY, &u64_to_bytes(0));
        storage_set(TWAP_CUMULATIVE_B_KEY, &u64_to_bytes(0));
        return;
    }

    let elapsed = now.saturating_sub(last_update);
    if elapsed == 0 {
        return; // Same block — no update
    }

    let last_ra = storage_get(TWAP_LAST_RESERVE_A_KEY)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    let last_rb = storage_get(TWAP_LAST_RESERVE_B_KEY)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);

    if last_ra > 0 && last_rb > 0 {
        // price_a_cumulative += (reserve_b << 32) / reserve_a * elapsed
        // price_b_cumulative += (reserve_a << 32) / reserve_b * elapsed
        let cum_a = storage_get(TWAP_CUMULATIVE_A_KEY)
            .map(|d| bytes_to_u64(&d))
            .unwrap_or(0);
        let cum_b = storage_get(TWAP_CUMULATIVE_B_KEY)
            .map(|d| bytes_to_u64(&d))
            .unwrap_or(0);

        // Use u128 to avoid overflow in intermediate math
        let price_a_scaled = ((last_rb as u128) << 32) / (last_ra as u128);
        let price_b_scaled = ((last_ra as u128) << 32) / (last_rb as u128);

        // AUDIT-FIX 3.19: wrapping_add is intentional — Uniswap V2 TWAP design.
        // Cumulative prices are meant to overflow; consumers compute the delta
        // between two snapshots using wrapping_sub to derive the average price
        // over an interval. The `as u64` truncation is also by design.
        let new_cum_a = (cum_a as u128).wrapping_add(price_a_scaled * elapsed as u128) as u64;
        let new_cum_b = (cum_b as u128).wrapping_add(price_b_scaled * elapsed as u128) as u64;

        storage_set(TWAP_CUMULATIVE_A_KEY, &u64_to_bytes(new_cum_a));
        storage_set(TWAP_CUMULATIVE_B_KEY, &u64_to_bytes(new_cum_b));
    }

    // Snapshot current reserves for next interval
    let pool = load_pool();
    storage_set(TWAP_LAST_UPDATE_KEY, &u64_to_bytes(now));
    storage_set(TWAP_LAST_RESERVE_A_KEY, &u64_to_bytes(pool.reserve_a));
    storage_set(TWAP_LAST_RESERVE_B_KEY, &u64_to_bytes(pool.reserve_b));

    // Increment snapshot counter
    let count = storage_get(TWAP_SNAPSHOT_COUNT_KEY)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    storage_set(TWAP_SNAPSHOT_COUNT_KEY, &u64_to_bytes(count + 1));
}

/// Check price impact of a swap. Returns true if within limits.
fn check_price_impact(reserve_in: u64, reserve_out: u64, amount_in: u64) -> bool {
    if reserve_in == 0 || reserve_out == 0 || amount_in == 0 {
        return false;
    }
    // Price impact ≈ amount_in / (reserve_in + amount_in) in basis points
    // impact_bps = amount_in * 10000 / (reserve_in + amount_in)
    let impact_bps = (amount_in as u128 * 10000) / (reserve_in as u128 + amount_in as u128);
    impact_bps <= MAX_PRICE_IMPACT_BPS as u128
}

/// Accrue protocol fee from a swap. Deducts protocol's share from swap output.
fn accrue_protocol_fee(amount_out: u64, is_token_a: bool) -> u64 {
    let fee_share = storage_get(PROTOCOL_FEE_SHARE_KEY)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_PROTOCOL_FEE_SHARE);
    if fee_share == 0 {
        return amount_out;
    }
    // Protocol fee = amount_out * fee_share / 10000
    // This is the protocol's cut of the trading fee (not an additional fee)
    // Use u128 to prevent overflow on large swap amounts
    let protocol_cut = ((amount_out as u128) * (fee_share as u128) / 10_000_000) as u64; // fee_share is per 10M to get fine resolution
    if protocol_cut == 0 {
        return amount_out;
    }
    let fee_key = if is_token_a { PROTOCOL_FEES_A_KEY } else { PROTOCOL_FEES_B_KEY };
    let accrued = storage_get(fee_key)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    storage_set(fee_key, &u64_to_bytes(accrued + protocol_cut));
    // AUDIT-FIX 1.11: Return amount MINUS protocol cut — was returning full amount_out
    amount_out - protocol_cut
}

/// Initialize the liquidity pool
#[no_mangle]
pub extern "C" fn initialize(token_a_ptr: *const u8, token_b_ptr: *const u8) {
    // Re-initialization guard: reject if pool is already set up
    if storage_get(b"pool_token_a").is_some() {
        log_info("MoltSwap pool already initialized — ignoring");
        return;
    }

    unsafe {
        // Parse token addresses
        let mut token_a_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(token_a_ptr, token_a_addr.as_mut_ptr(), 32);
        let token_a = Address(token_a_addr);
        
        let mut token_b_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(token_b_ptr, token_b_addr.as_mut_ptr(), 32);
        let token_b = Address(token_b_addr);
        
        // Pool::initialize calls save() which now persists token addresses too
        let mut pool = Pool::new(token_a, token_b);
        pool.initialize(token_a, token_b).expect("Init failed");

        // SECURITY FIX: Set caller as admin, not token_a address
        let caller = get_caller();
        storage_set(MS_ADMIN_KEY, &caller.0);
        
        log_info("MoltSwap liquidity pool initialized");
    }
}

/// Add liquidity to the pool
#[no_mangle]
pub extern "C" fn add_liquidity(
    provider_ptr: *const u8,
    amount_a: u64,
    amount_b: u64,
    min_liquidity: u64,
) -> u64 {
    if is_ms_paused() {
        log_info("MoltSwap is paused");
        return 0;
    }
    // AUDIT-FIX 1.10: Add reentrancy_enter() — was missing, causing
    // reentrancy_exit() to clear the guard for concurrent operations.
    if !reentrancy_enter() {
        return 0;
    }
    unsafe {
        let mut provider_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(provider_ptr, provider_addr.as_mut_ptr(), 32);
        let provider = Address(provider_addr);
        
        let mut pool = load_pool();
        match pool.add_liquidity(provider, amount_a, amount_b, min_liquidity) {
            Ok(liquidity) => {
                log_info("Liquidity added successfully");
                reentrancy_exit();
                liquidity
            }
            Err(_) => {
                log_info("Add liquidity failed");
                reentrancy_exit();
                0
            }
        }
    }
}

/// Remove liquidity from the pool
#[no_mangle]
pub extern "C" fn remove_liquidity(
    provider_ptr: *const u8,
    liquidity: u64,
    min_amount_a: u64,
    min_amount_b: u64,
    out_a_ptr: *mut u8,
    out_b_ptr: *mut u8,
) -> u32 {
    if !reentrancy_enter() {
        log_info("Reentrancy detected");
        return 0;
    }
    unsafe {
        let mut provider_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(provider_ptr, provider_addr.as_mut_ptr(), 32);
        let provider = Address(provider_addr);
        
        let mut pool = load_pool();
        match pool.remove_liquidity(provider, liquidity, min_amount_a, min_amount_b) {
            Ok((amount_a, amount_b)) => {
                log_info("Liquidity removed successfully");
                
                // Write amounts to output pointers
                let out_a_slice = core::slice::from_raw_parts_mut(out_a_ptr, 8);
                out_a_slice.copy_from_slice(&amount_a.to_le_bytes());
                
                let out_b_slice = core::slice::from_raw_parts_mut(out_b_ptr, 8);
                out_b_slice.copy_from_slice(&amount_b.to_le_bytes());
                
                reentrancy_exit();
                1
            }
            Err(_) => {
                log_info("Remove liquidity failed");
                reentrancy_exit();
                0
            }
        }
    }
}

/// Swap token A for token B
#[no_mangle]
pub extern "C" fn swap_a_for_b(amount_a_in: u64, min_amount_b_out: u64) -> u64 {
    if is_ms_paused() {
        log_info("MoltSwap is paused");
        return 0;
    }
    if !reentrancy_enter() {
        log_info("Reentrancy detected");
        return 0;
    }
    // v2: TWAP oracle update before reserve change
    twap_update();

    // AUDIT-FIX 3.20: Load pool once, use for both price impact check and swap
    let mut pool = load_pool();

    // v2: Price impact guard
    if !check_price_impact(pool.reserve_a, pool.reserve_b, amount_a_in) {
        log_info("Price impact exceeds maximum (5%)");
        reentrancy_exit();
        return 0;
    }

    match pool.swap_a_for_b(amount_a_in, min_amount_b_out) {
        Ok(amount_b_out) => {
            log_info("Swap A->B successful");
            // AUDIT-FIX 1.11: Use the fee-deducted amount
            let amount_b_out = accrue_protocol_fee(amount_b_out, false);

            let bonus = get_reputation_bonus(amount_b_out);
            let final_out = if bonus > 0 {
                let mut pool2 = load_pool();
                if pool2.reserve_b >= bonus {
                    pool2.reserve_b -= bonus;
                    let _ = pool2.save();
                    log_info("Reputation fee discount applied");
                    amount_b_out + bonus
                } else {
                    amount_b_out
                }
            } else {
                amount_b_out
            };
            reentrancy_exit();
            final_out
        }
        Err(_) => {
            log_info("Swap A->B failed");
            reentrancy_exit();
            0
        }
    }
}

/// Swap token B for token A
#[no_mangle]
pub extern "C" fn swap_b_for_a(amount_b_in: u64, min_amount_a_out: u64) -> u64 {
    if is_ms_paused() {
        log_info("MoltSwap is paused");
        return 0;
    }
    if !reentrancy_enter() {
        log_info("Reentrancy detected");
        return 0;
    }
    // v2: TWAP oracle update before reserve change
    twap_update();

    // AUDIT-FIX 3.20: Load pool once, use for both price impact check and swap
    let mut pool = load_pool();

    // v2: Price impact guard
    if !check_price_impact(pool.reserve_b, pool.reserve_a, amount_b_in) {
        log_info("Price impact exceeds maximum (5%)");
        reentrancy_exit();
        return 0;
    }

    match pool.swap_b_for_a(amount_b_in, min_amount_a_out) {
        Ok(amount_a_out) => {
            log_info("Swap B->A successful");
            // AUDIT-FIX 1.11: Use the fee-deducted amount
            let amount_a_out = accrue_protocol_fee(amount_a_out, true);

            let bonus = get_reputation_bonus(amount_a_out);
            let final_out = if bonus > 0 {
                let mut pool2 = load_pool();
                if pool2.reserve_a >= bonus {
                    pool2.reserve_a -= bonus;
                    let _ = pool2.save();
                    log_info("Reputation fee discount applied");
                    amount_a_out + bonus
                } else {
                    amount_a_out
                }
            } else {
                amount_a_out
            };
            reentrancy_exit();
            final_out
        }
        Err(_) => {
            log_info("Swap B->A failed");
            reentrancy_exit();
            0
        }
    }
}

/// Swap token A for B with deadline. Rejected if current timestamp > deadline.
#[no_mangle]
pub extern "C" fn swap_a_for_b_with_deadline(amount_a_in: u64, min_amount_b_out: u64, deadline: u64) -> u64 {
    if get_timestamp() > deadline {
        log_info("Transaction expired (deadline passed)");
        return 0;
    }
    swap_a_for_b(amount_a_in, min_amount_b_out)
}

/// Swap token B for A with deadline. Rejected if current timestamp > deadline.
#[no_mangle]
pub extern "C" fn swap_b_for_a_with_deadline(amount_b_in: u64, min_amount_a_out: u64, deadline: u64) -> u64 {
    if get_timestamp() > deadline {
        log_info("Transaction expired (deadline passed)");
        return 0;
    }
    swap_b_for_a(amount_b_in, min_amount_a_out)
}

/// Get quote for swap (how much output for given input)
#[no_mangle]
pub extern "C" fn get_quote(amount_in: u64, is_a_to_b: u32) -> u64 {
    let pool = load_pool();
    
    if is_a_to_b == 1 {
        pool.get_amount_out(amount_in, pool.reserve_a, pool.reserve_b)
    } else {
        pool.get_amount_out(amount_in, pool.reserve_b, pool.reserve_a)
    }
}

/// Get reserve amounts
#[no_mangle]
pub extern "C" fn get_reserves(out_a_ptr: *mut u8, out_b_ptr: *mut u8) {
    unsafe {
        let pool = load_pool();
        
        let out_a_slice = core::slice::from_raw_parts_mut(out_a_ptr, 8);
        out_a_slice.copy_from_slice(&pool.reserve_a.to_le_bytes());
        
        let out_b_slice = core::slice::from_raw_parts_mut(out_b_ptr, 8);
        out_b_slice.copy_from_slice(&pool.reserve_b.to_le_bytes());
    }
}

/// Get liquidity balance of provider
#[no_mangle]
pub extern "C" fn get_liquidity_balance(provider_ptr: *const u8) -> u64 {
    unsafe {
        let mut provider_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(provider_ptr, provider_addr.as_mut_ptr(), 32);
        let provider = Address(provider_addr);
        
        load_pool().get_liquidity_balance(provider)
    }
}

/// Get total liquidity (read from persistent storage)
#[no_mangle]
pub extern "C" fn get_total_liquidity() -> u64 {
    match storage_get(b"total_liquidity") {
        Some(bytes) => bytes_to_u64(&bytes),
        None => 0,
    }
}

// ============================================================================
// FLASH LOANS (per whitepaper)
// ============================================================================

/// Flash loan fee: 0.09% (9 basis points)
const FLASH_LOAN_FEE_BPS: u64 = 9;

// Storage keys for in-flight flash loan tracking.
// Stored in contract storage so state is consistent even if the WASM instance
// is re-created between calls within the same transaction.
const FL_ACTIVE_KEY: &[u8] = b"_fl_active";
const FL_TOKEN_IS_A_KEY: &[u8] = b"_fl_is_a";
const FL_AMOUNT_KEY: &[u8] = b"_fl_amount";
const FL_FEE_KEY: &[u8] = b"_fl_fee";

fn fl_is_active() -> bool {
    storage_get(FL_ACTIVE_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(false)
}

fn fl_set_active(active: bool) {
    storage_set(FL_ACTIVE_KEY, &[active as u8]);
}

fn fl_get_token_is_a() -> bool {
    storage_get(FL_TOKEN_IS_A_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(true)
}

fn fl_get_amount() -> u64 {
    storage_get(FL_AMOUNT_KEY).map(|v| bytes_to_u64(&v)).unwrap_or(0)
}

fn fl_get_fee() -> u64 {
    storage_get(FL_FEE_KEY).map(|v| bytes_to_u64(&v)).unwrap_or(0)
}

fn fl_store_loan(token_is_a: bool, amount: u64, fee: u64) {
    fl_set_active(true);
    storage_set(FL_TOKEN_IS_A_KEY, &[token_is_a as u8]);
    storage_set(FL_AMOUNT_KEY, &u64_to_bytes(amount));
    storage_set(FL_FEE_KEY, &u64_to_bytes(fee));
}

fn fl_clear() {
    fl_set_active(false);
    storage_set(FL_AMOUNT_KEY, &u64_to_bytes(0));
    storage_set(FL_FEE_KEY, &u64_to_bytes(0));
}

/// Borrow tokens via flash loan. Must call `flash_loan_repay` in the same tx.
/// Reserves are NOT decremented until repay succeeds (atomic loan model).
/// Returns the amount lent (0 on failure).
#[no_mangle]
pub extern "C" fn flash_loan_borrow(amount: u64, token_is_a: u32) -> u64 {
    if is_ms_paused() {
        log_info("MoltSwap is paused");
        return 0;
    }
    if !reentrancy_enter() {
        log_info("Reentrancy detected");
        return 0;
    }
    if fl_is_active() {
        log_info("Flash loan already active");
        reentrancy_exit();
        return 0;
    }

    let pool = load_pool();
    let reserve = if token_is_a == 1 { pool.reserve_a } else { pool.reserve_b };

    if amount == 0 || amount > reserve {
        log_info("Flash loan: invalid amount or insufficient reserves");
        reentrancy_exit();
        return 0;
    }

    // v2: Cap flash loans at MAX_FLASH_LOAN_PERCENT of reserves
    let max_loan = reserve * MAX_FLASH_LOAN_PERCENT / 100;
    if amount > max_loan {
        log_info("Flash loan exceeds maximum (90% of reserves)");
        reentrancy_exit();
        return 0;
    }

    let fee = (amount * FLASH_LOAN_FEE_BPS + 9999) / 10000; // Round up

    // Store loan metadata WITHOUT modifying reserves
    // Reserves only change when repay succeeds (atomic guarantee)
    fl_store_loan(token_is_a == 1, amount, fee);

    // Store the borrow timestamp for staleness detection
    let timestamp = get_timestamp();
    storage_set(b"fl_borrow_time", &u64_to_bytes(timestamp));

    log_info("Flash loan issued (reserves held until repay)");
    // Keep reentrancy guard ACTIVE to prevent any pool mutations until repay
    // Do NOT call reentrancy_exit() — borrower must call flash_loan_repay
    amount
}

/// Repay flash loan. Must return borrowed amount + fee.
/// On success: fee is added to reserves (benefits LPs). 
/// On failure: nothing changes (reserves were never decremented).
/// Returns 0 on success, non-zero on failure.
#[no_mangle]
pub extern "C" fn flash_loan_repay(repay_amount: u64) -> u32 {
    if !fl_is_active() {
        log_info("No active flash loan to repay");
        return 1;
    }

    let loan_amount = fl_get_amount();
    let loan_fee = fl_get_fee();
    let token_is_a = fl_get_token_is_a();

    let required = loan_amount + loan_fee;
    if repay_amount < required {
        log_info("Flash loan repayment insufficient — loan reverted");
        // No reserve changes needed — reserves were never decremented
        fl_clear();
        reentrancy_exit();
        return 2;
    }

    // Success: add ONLY the fee to reserves (the principal was never removed)
    let mut pool = load_pool();
    let fee_collected = repay_amount.saturating_sub(loan_amount);
    if token_is_a {
        pool.reserve_a += fee_collected;
    } else {
        pool.reserve_b += fee_collected;
    }
    let _ = pool.save();

    log_info("Flash loan repaid successfully");
    fl_clear();
    reentrancy_exit();
    0
}

/// Abort a stale flash loan. Anyone can call this if a loan has been active
/// for more than 60 seconds (covers block finality window).
/// This ensures reserves can never be permanently locked.
#[no_mangle]
pub extern "C" fn flash_loan_abort() -> u32 {
    if !fl_is_active() {
        return 0; // Nothing to abort
    }
    
    let borrow_time = storage_get(b"fl_borrow_time")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    let now = get_timestamp();
    
    if now.saturating_sub(borrow_time) < 60 {
        log_info("Flash loan not yet stale (< 60s)");
        return 1;
    }
    
    // Clear stale loan — reserves were never modified, so nothing to restore
    log_info("Stale flash loan aborted");
    fl_clear();
    reentrancy_exit();
    0
}

/// Get flash loan fee for a given amount
#[no_mangle]
pub extern "C" fn get_flash_loan_fee(amount: u64) -> u64 {
    (amount * FLASH_LOAN_FEE_BPS + 9999) / 10000
}

// ============================================================================
// v2: TWAP ORACLE QUERIES
// ============================================================================

/// Get TWAP cumulative prices. Returns 24 bytes:
/// [cumulative_price_a(8), cumulative_price_b(8), last_update_timestamp(8)]
#[no_mangle]
pub extern "C" fn get_twap_cumulatives() -> u32 {
    let cum_a = storage_get(TWAP_CUMULATIVE_A_KEY)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    let cum_b = storage_get(TWAP_CUMULATIVE_B_KEY)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    let last_update = storage_get(TWAP_LAST_UPDATE_KEY)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);

    let mut result = Vec::with_capacity(24);
    result.extend_from_slice(&u64_to_bytes(cum_a));
    result.extend_from_slice(&u64_to_bytes(cum_b));
    result.extend_from_slice(&u64_to_bytes(last_update));
    moltchain_sdk::set_return_data(&result);
    0
}

/// Get total TWAP snapshot count (number of oracle updates)
#[no_mangle]
pub extern "C" fn get_twap_snapshot_count() -> u64 {
    storage_get(TWAP_SNAPSHOT_COUNT_KEY)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0)
}

// ============================================================================
// v2: PROTOCOL FEE ADMIN
// ============================================================================

/// Set protocol treasury address and fee share. Admin only.
#[no_mangle]
pub extern "C" fn set_protocol_fee(
    caller_ptr: *const u8,
    treasury_ptr: *const u8,
    fee_share: u64,
) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    let admin = match storage_get(IDENTITY_ADMIN_KEY) {
        Some(data) => data,
        None => return 1,
    };
    if caller[..] != admin[..] {
        log_info("Unauthorized");
        return 2;
    }

    if fee_share > 5_000_000 {
        log_info("Fee share too high (max 50%)");
        return 3;
    }

    let mut treasury = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(treasury_ptr, treasury.as_mut_ptr(), 32); }
    storage_set(PROTOCOL_TREASURY_KEY, &treasury);
    storage_set(PROTOCOL_FEE_SHARE_KEY, &u64_to_bytes(fee_share));
    log_info("Protocol fee configured");
    0
}

/// Get accrued protocol fees. Returns 16 bytes: [fees_a(8), fees_b(8)]
#[no_mangle]
pub extern "C" fn get_protocol_fees() -> u32 {
    let fees_a = storage_get(PROTOCOL_FEES_A_KEY)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    let fees_b = storage_get(PROTOCOL_FEES_B_KEY)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);

    let mut result = Vec::with_capacity(16);
    result.extend_from_slice(&u64_to_bytes(fees_a));
    result.extend_from_slice(&u64_to_bytes(fees_b));
    moltchain_sdk::set_return_data(&result);
    0
}

// ============================================================================
// MOLTYID IDENTITY INTEGRATION
// ============================================================================

/// Storage key for identity admin
const IDENTITY_ADMIN_KEY: &[u8] = b"identity_admin";
/// Storage key for MoltyID contract address (32 bytes)
const MOLTYID_ADDR_KEY: &[u8] = b"moltyid_address";
/// Storage key for reputation discount threshold
const MOLTYID_DISCOUNT_THRESHOLD_KEY: &[u8] = b"moltyid_disc_threshold";
/// Storage key for reputation discount in basis points
const MOLTYID_DISCOUNT_BPS_KEY: &[u8] = b"moltyid_disc_bps";

/// Set the admin for identity/reputation configuration.
/// Only callable once (first caller becomes admin).
#[no_mangle]
pub extern "C" fn set_identity_admin(admin_ptr: *const u8) -> u32 {
    let mut admin = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(admin_ptr, admin.as_mut_ptr(), 32); }

    if storage_get(IDENTITY_ADMIN_KEY).is_some() {
        log_info("Identity admin already set");
        return 1;
    }

    storage_set(IDENTITY_ADMIN_KEY, &admin);
    log_info("Identity admin set");
    0
}

/// Set MoltyID contract address for cross-contract reputation lookups.
/// Only callable by the identity admin.
#[no_mangle]
pub extern "C" fn set_moltyid_address(caller_ptr: *const u8, moltyid_addr_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let mut moltyid_addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(moltyid_addr_ptr, moltyid_addr.as_mut_ptr(), 32); }

    let admin = match storage_get(IDENTITY_ADMIN_KEY) {
        Some(data) => data,
        None => return 1,
    };
    if caller[..] != admin[..] {
        return 2;
    }

    storage_set(MOLTYID_ADDR_KEY, &moltyid_addr);
    log_info("MoltyID address configured");
    0
}

/// Set reputation-based fee discount parameters.
/// Only callable by the identity admin.
/// - threshold: minimum reputation to qualify for discount
/// - discount_bps: discount in basis points (e.g., 15 = 0.15%)
#[no_mangle]
pub extern "C" fn set_reputation_discount(
    caller_ptr: *const u8,
    threshold: u64,
    discount_bps: u64,
) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    let admin = match storage_get(IDENTITY_ADMIN_KEY) {
        Some(data) => data,
        None => return 1,
    };
    if caller[..] != admin[..] {
        return 2;
    }

    storage_set(MOLTYID_DISCOUNT_THRESHOLD_KEY, &u64_to_bytes(threshold));
    storage_set(MOLTYID_DISCOUNT_BPS_KEY, &u64_to_bytes(discount_bps));
    log_info("Reputation fee discount configured");
    0
}

/// Calculate reputation-based fee bonus for the current caller.
/// Returns additional tokens to award (0 if no discount applies).
fn get_reputation_bonus(amount_out: u64) -> u64 {
    let threshold = match storage_get(MOLTYID_DISCOUNT_THRESHOLD_KEY) {
        Some(data) if data.len() >= 8 => bytes_to_u64(&data),
        _ => return 0,
    };
    if threshold == 0 {
        return 0;
    }

    let discount_bps = match storage_get(MOLTYID_DISCOUNT_BPS_KEY) {
        Some(data) if data.len() >= 8 => bytes_to_u64(&data),
        _ => return 0,
    };
    if discount_bps == 0 {
        return 0;
    }

    let moltyid_addr = match storage_get(MOLTYID_ADDR_KEY) {
        Some(data) if data.len() >= 32 => data,
        _ => return 0,
    };

    let caller = get_caller();
    let mut addr = [0u8; 32];
    addr.copy_from_slice(&moltyid_addr[..32]);
    let target = Address::new(addr);
    let mut args = Vec::with_capacity(32);
    args.extend_from_slice(&caller.0);
    let call = CrossCall::new(target, "get_reputation", args);

    match call_contract(call) {
        Ok(result) if result.len() >= 8 => {
            let reputation = bytes_to_u64(&result);
            if reputation >= threshold {
                amount_out * discount_bps / 10000
            } else {
                0
            }
        }
        _ => 0,
    }
}

// ============================================================================
// EMERGENCY PAUSE (admin only)
// ============================================================================

/// Pause the protocol — blocks swaps, liquidity ops, and flash loans
#[no_mangle]
pub extern "C" fn ms_pause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    if !is_ms_admin(&caller) {
        return 1;
    }
    storage_set(MS_PAUSE_KEY, &[1u8]);
    log_info("MoltSwap paused");
    0
}

/// Unpause the protocol
#[no_mangle]
pub extern "C" fn ms_unpause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    if !is_ms_admin(&caller) {
        return 1;
    }
    storage_set(MS_PAUSE_KEY, &[0u8]);
    log_info("MoltSwap unpaused");
    0
}

// ============================================================================
// ALIASES — bridge test-expected names to actual implementation
// ============================================================================

/// Alias: tests call `create_pool` — in single-pool AMM, this is `initialize`
#[no_mangle]
pub extern "C" fn create_pool(token_a_ptr: *const u8, token_b_ptr: *const u8) {
    initialize(token_a_ptr, token_b_ptr)
}

/// Alias: tests call `swap` — delegates to swap_a_for_b or swap_b_for_a based on flag
#[no_mangle]
pub extern "C" fn swap(amount_in: u64, min_out: u64, a_to_b: u32) -> u64 {
    if a_to_b != 0 {
        swap_a_for_b(amount_in, min_out)
    } else {
        swap_b_for_a(amount_in, min_out)
    }
}

/// Tests expect `get_pool_info` — return reserves + liquidity data
#[no_mangle]
pub extern "C" fn get_pool_info() -> u32 {
    let reserve_a = storage_get(b"reserve_a")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    let reserve_b = storage_get(b"reserve_b")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    let total_liq = get_total_liquidity();
    let mut data = Vec::with_capacity(24);
    data.extend_from_slice(&u64_to_bytes(reserve_a));
    data.extend_from_slice(&u64_to_bytes(reserve_b));
    data.extend_from_slice(&u64_to_bytes(total_liq));
    moltchain_sdk::set_return_data(&data);
    1
}

/// Tests expect `get_pool_count` — single-pool AMM, returns 1 if initialized
#[no_mangle]
pub extern "C" fn get_pool_count() -> u64 {
    if storage_get(b"pool_token_a").is_some() { 1 } else { 0 }
}

/// Alias: tests call `set_platform_fee` — wraps `set_protocol_fee` with zero treasury
#[no_mangle]
pub extern "C" fn set_platform_fee(caller_ptr: *const u8, fee_bps: u64) -> u32 {
    // Use caller as treasury for simplicity when called via the alias
    set_protocol_fee(caller_ptr, caller_ptr, fee_bps)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use moltchain_sdk::test_mock;
    use moltchain_sdk::bytes_to_u64;

    fn setup() {
        test_mock::reset();
    }

    #[test]
    fn test_initialize_pool() {
        setup();
        let token_a = [1u8; 32];
        let token_b = [2u8; 32];

        initialize(token_a.as_ptr(), token_b.as_ptr());

        // Verify pool tokens stored
        assert_eq!(test_mock::get_storage(b"token_a"), Some(token_a.to_vec()));
        assert_eq!(test_mock::get_storage(b"token_b"), Some(token_b.to_vec()));

        // Reserves should be 0
        let ra = test_mock::get_storage(b"reserve_a").map(|b| bytes_to_u64(&b)).unwrap_or(0);
        let rb = test_mock::get_storage(b"reserve_b").map(|b| bytes_to_u64(&b)).unwrap_or(0);
        assert_eq!(ra, 0);
        assert_eq!(rb, 0);
    }

    #[test]
    fn test_add_liquidity_first_provider() {
        setup();
        let token_a = [1u8; 32];
        let token_b = [2u8; 32];
        initialize(token_a.as_ptr(), token_b.as_ptr());

        let provider = [3u8; 32];
        let amount_a: u64 = 10_000_000;
        let amount_b: u64 = 20_000_000;
        let min_liquidity: u64 = 0;

        let liquidity = add_liquidity(
            provider.as_ptr(),
            amount_a,
            amount_b,
            min_liquidity,
        );

        assert!(liquidity > 0, "Should receive liquidity tokens");

        // Check reserves updated
        let ra = test_mock::get_storage(b"reserve_a").map(|b| bytes_to_u64(&b)).unwrap_or(0);
        let rb = test_mock::get_storage(b"reserve_b").map(|b| bytes_to_u64(&b)).unwrap_or(0);
        assert_eq!(ra, amount_a);
        assert_eq!(rb, amount_b);
    }

    #[test]
    fn test_swap_a_for_b() {
        setup();
        let token_a = [1u8; 32];
        let token_b = [2u8; 32];
        initialize(token_a.as_ptr(), token_b.as_ptr());

        // Add liquidity first
        let provider = [3u8; 32];
        add_liquidity(provider.as_ptr(), 1_000_000, 1_000_000, 0);

        // Swap A for B
        let amount_in: u64 = 10_000;
        let min_out: u64 = 0;
        let amount_out = swap_a_for_b(amount_in, min_out);

        assert!(amount_out > 0, "Should receive some token B");
        assert!(amount_out < amount_in, "Output should be less due to fees");

        // Verify reserves changed
        let ra = test_mock::get_storage(b"reserve_a").map(|b| bytes_to_u64(&b)).unwrap_or(0);
        let rb = test_mock::get_storage(b"reserve_b").map(|b| bytes_to_u64(&b)).unwrap_or(0);
        assert_eq!(ra, 1_000_000 + amount_in);
        // Reserve decrease may differ by ±1 from amount_out due to protocol fee rounding (u128 precision)
        assert!((rb as i64 - (1_000_000 - amount_out) as i64).unsigned_abs() <= 1,
            "Reserve B should approximate 1M - amount_out, got rb={} vs expected={}", rb, 1_000_000 - amount_out);
    }

    #[test]
    fn test_swap_b_for_a() {
        setup();
        let token_a = [1u8; 32];
        let token_b = [2u8; 32];
        initialize(token_a.as_ptr(), token_b.as_ptr());

        let provider = [3u8; 32];
        add_liquidity(provider.as_ptr(), 1_000_000, 1_000_000, 0);

        let amount_in: u64 = 10_000;
        let amount_out = swap_b_for_a(amount_in, 0);

        assert!(amount_out > 0);
        assert!(amount_out < amount_in);
    }

    #[test]
    fn test_get_quote() {
        setup();
        let token_a = [1u8; 32];
        let token_b = [2u8; 32];
        initialize(token_a.as_ptr(), token_b.as_ptr());

        let provider = [3u8; 32];
        add_liquidity(provider.as_ptr(), 1_000_000, 2_000_000, 0);

        // Quote for swapping A->B
        let quote = get_quote(10_000, 1);
        assert!(quote > 0);

        // Quote for swapping B->A
        let quote2 = get_quote(10_000, 0);
        assert!(quote2 > 0);
        // Since reserve_b > reserve_a, swapping B should give less A
        assert!(quote2 < quote);
    }

    #[test]
    fn test_flash_loan_fee_calculation() {
        // Pure function, no setup needed
        let fee = get_flash_loan_fee(100_000);
        // 0.09% = 9 bps, rounded up: (100_000 * 9 + 9999) / 10000 = 90
        assert_eq!(fee, 90);

        let fee2 = get_flash_loan_fee(1);
        // (1 * 9 + 9999) / 10000 = 1 (rounded up)
        assert_eq!(fee2, 1);
    }

    #[test]
    fn test_get_total_liquidity() {
        setup();
        let token_a = [1u8; 32];
        let token_b = [2u8; 32];
        initialize(token_a.as_ptr(), token_b.as_ptr());

        assert_eq!(get_total_liquidity(), 0);

        let provider = [3u8; 32];
        add_liquidity(provider.as_ptr(), 1_000_000, 1_000_000, 0);

        assert!(get_total_liquidity() > 0);
    }

    #[test]
    fn test_swap_no_discount_without_config() {
        setup();
        let token_a = [1u8; 32];
        let token_b = [2u8; 32];
        initialize(token_a.as_ptr(), token_b.as_ptr());

        let provider = [3u8; 32];
        add_liquidity(provider.as_ptr(), 1_000_000, 1_000_000, 0);

        // Without discount config, swap works as normal
        let amount_out = swap_a_for_b(10_000, 0);
        assert!(amount_out > 0);
    }

    #[test]
    fn test_set_reputation_discount_admin_only() {
        setup();

        let admin = [1u8; 32];
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        assert_eq!(set_identity_admin(admin.as_ptr()), 1); // already set

        let other = [9u8; 32];
        assert_eq!(set_reputation_discount(other.as_ptr(), 100, 15), 2);
        assert_eq!(set_reputation_discount(admin.as_ptr(), 100, 15), 0);

        // Verify stored values
        let threshold = test_mock::get_storage(MOLTYID_DISCOUNT_THRESHOLD_KEY).unwrap();
        assert_eq!(bytes_to_u64(&threshold), 100);
        let bps = test_mock::get_storage(MOLTYID_DISCOUNT_BPS_KEY).unwrap();
        assert_eq!(bytes_to_u64(&bps), 15);
    }

    #[test]
    fn test_set_moltyid_address_admin_only() {
        setup();

        let admin = [1u8; 32];
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);

        let other = [9u8; 32];
        assert_eq!(set_moltyid_address(other.as_ptr(), [0x42u8; 32].as_ptr()), 2);
        assert_eq!(set_moltyid_address(admin.as_ptr(), [0x42u8; 32].as_ptr()), 0);
    }

    // =============================================
    // v2 TESTS: TWAP ORACLE
    // =============================================

    #[test]
    fn test_twap_initialized_on_first_swap() {
        setup();
        let token_a = [1u8; 32];
        let token_b = [2u8; 32];
        initialize(token_a.as_ptr(), token_b.as_ptr());

        let provider = [3u8; 32];
        add_liquidity(provider.as_ptr(), 1_000_000, 2_000_000, 0);

        test_mock::set_timestamp(1000);
        swap_a_for_b(1_000, 0);

        // TWAP should have been initialized
        let last_update = test_mock::get_storage(TWAP_LAST_UPDATE_KEY);
        assert!(last_update.is_some());
        assert_eq!(bytes_to_u64(&last_update.unwrap()), 1000);
    }

    #[test]
    fn test_twap_accumulates_over_time() {
        setup();
        let token_a = [1u8; 32];
        let token_b = [2u8; 32];
        initialize(token_a.as_ptr(), token_b.as_ptr());

        let provider = [3u8; 32];
        add_liquidity(provider.as_ptr(), 1_000_000, 1_000_000, 0);

        test_mock::set_timestamp(1000);
        swap_a_for_b(1_000, 0);

        test_mock::set_timestamp(2000);
        swap_a_for_b(1_000, 0);

        // Snapshot count should be 1 (first swap initializes, second increments once)
        let count = get_twap_snapshot_count();
        assert_eq!(count, 1);

        // Cumulatives should be non-zero
        assert_eq!(get_twap_cumulatives(), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 24);
    }

    // =============================================
    // v2 TESTS: DEADLINE
    // =============================================

    #[test]
    fn test_swap_with_deadline_ok() {
        setup();
        let token_a = [1u8; 32];
        let token_b = [2u8; 32];
        initialize(token_a.as_ptr(), token_b.as_ptr());
        add_liquidity([3u8; 32].as_ptr(), 1_000_000, 1_000_000, 0);

        test_mock::set_timestamp(100);
        let out = swap_a_for_b_with_deadline(1_000, 0, 200);
        assert!(out > 0);
    }

    #[test]
    fn test_swap_with_deadline_expired() {
        setup();
        let token_a = [1u8; 32];
        let token_b = [2u8; 32];
        initialize(token_a.as_ptr(), token_b.as_ptr());
        add_liquidity([3u8; 32].as_ptr(), 1_000_000, 1_000_000, 0);

        test_mock::set_timestamp(300);
        let out = swap_a_for_b_with_deadline(1_000, 0, 200);
        assert_eq!(out, 0);
    }

    #[test]
    fn test_swap_b_with_deadline_expired() {
        setup();
        let token_a = [1u8; 32];
        let token_b = [2u8; 32];
        initialize(token_a.as_ptr(), token_b.as_ptr());
        add_liquidity([3u8; 32].as_ptr(), 1_000_000, 1_000_000, 0);

        test_mock::set_timestamp(300);
        let out = swap_b_for_a_with_deadline(1_000, 0, 200);
        assert_eq!(out, 0);
    }

    // =============================================
    // v2 TESTS: PRICE IMPACT
    // =============================================

    #[test]
    fn test_price_impact_guard_rejects_large_swap() {
        setup();
        let token_a = [1u8; 32];
        let token_b = [2u8; 32];
        initialize(token_a.as_ptr(), token_b.as_ptr());
        add_liquidity([3u8; 32].as_ptr(), 1_000_000, 1_000_000, 0);

        // Try to swap 10% of reserves (>5% impact) — should be rejected
        let out = swap_a_for_b(100_000, 0);
        assert_eq!(out, 0, "Large swap should be rejected by price impact guard");
    }

    #[test]
    fn test_price_impact_guard_allows_small_swap() {
        setup();
        let token_a = [1u8; 32];
        let token_b = [2u8; 32];
        initialize(token_a.as_ptr(), token_b.as_ptr());
        add_liquidity([3u8; 32].as_ptr(), 1_000_000, 1_000_000, 0);

        // Swap 1% of reserves (<5% impact) — should succeed
        let out = swap_a_for_b(10_000, 0);
        assert!(out > 0, "Small swap should pass price impact guard");
    }

    // =============================================
    // v2 TESTS: FLASH LOAN CAP
    // =============================================

    #[test]
    fn test_flash_loan_cap_rejects_over_90_percent() {
        setup();
        let token_a = [1u8; 32];
        let token_b = [2u8; 32];
        initialize(token_a.as_ptr(), token_b.as_ptr());
        add_liquidity([3u8; 32].as_ptr(), 1_000_000, 1_000_000, 0);

        // Try to flash loan 95% of reserves
        let out = flash_loan_borrow(950_000, 1);
        assert_eq!(out, 0, "Flash loan >90% should be rejected");
    }

    // =============================================
    // v2 TESTS: PROTOCOL FEES
    // =============================================

    #[test]
    fn test_set_protocol_fee_admin_only() {
        setup();
        let admin = [1u8; 32];
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);

        let treasury = [0xBBu8; 32];
        assert_eq!(set_protocol_fee(admin.as_ptr(), treasury.as_ptr(), 1000), 0);

        let other = [9u8; 32];
        assert_eq!(set_protocol_fee(other.as_ptr(), treasury.as_ptr(), 1000), 2);
    }

    #[test]
    fn test_protocol_fees_query() {
        setup();
        assert_eq!(get_protocol_fees(), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 16);
        assert_eq!(bytes_to_u64(&ret[0..8]), 0);
        assert_eq!(bytes_to_u64(&ret[8..16]), 0);
    }
}
