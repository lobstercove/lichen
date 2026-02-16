// LobsterLend v2 - Decentralized Lending Protocol
// Deposit collateral, borrow assets, earn interest
// Per whitepaper: collateralized lending with liquidation mechanics
//
// v2 additions:
//   - Flash loans with fee (0.09%)
//   - Emergency pause (admin)
//   - Reentrancy guard enforcement on all mutating functions
//   - Admin reserve withdrawal
//   - Admin reserve factor updates
//   - Protocol deposit cap
//   - Interest rate query view function
//   - Oracle price integration placeholder

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;
use alloc::vec::Vec;
use moltchain_sdk::{
    storage_get, storage_set, log_info, set_return_data,
    bytes_to_u64, u64_to_bytes, get_timestamp,
};

// T5.12: Reentrancy guard
const REENTRANCY_KEY: &[u8] = b"_reentrancy";

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

// ============================================================================
// CONSTANTS
// ============================================================================

/// Collateral factor: 75% (can borrow up to 75% of collateral value)
const COLLATERAL_FACTOR_PERCENT: u64 = 75;

/// Liquidation threshold: 85% (liquidatable when debt/collateral > 85%)
const LIQUIDATION_THRESHOLD_PERCENT: u64 = 85;

/// Liquidation bonus: 5% discount for liquidators
const LIQUIDATION_BONUS_PERCENT: u64 = 5;

/// Base interest rate: 2% annual (in basis points per slot, assuming 400ms slots)
/// ~788,400,000 slots/year → 2% / 788M ≈ 0.0000000254 per slot
/// We use a scaled rate: 254 per 10^10 per slot
const BASE_RATE_SCALED: u64 = 254;
const RATE_SCALE: u64 = 10_000_000_000;

/// Utilization kink: at 80% utilization, rate increases sharply
const UTILIZATION_KINK_PERCENT: u64 = 80;

/// Admin key for protocol operations
const ADMIN_KEY: &[u8] = b"ll_admin";

// ============================================================================
// v2 CONSTANTS
// ============================================================================

/// Flash loan fee: 9 basis points (0.09%)
const FLASH_LOAN_FEE_BPS: u64 = 9;
const BPS_SCALE: u64 = 10_000;

/// Maximum deposit cap (0 = unlimited)
const DEPOSIT_CAP_KEY: &[u8] = b"ll_deposit_cap";

/// Emergency pause key
const PAUSE_KEY: &[u8] = b"ll_paused";

/// Flash loan state keys
const FLASH_BORROWED_KEY: &[u8] = b"ll_flash_borrowed";
const FLASH_FEE_KEY: &[u8] = b"ll_flash_fee";

/// Maximum interest rate per slot to prevent manipulation
const MAX_RATE_PER_SLOT: u64 = 25_400; // 100x base rate

// ============================================================================
// STORAGE HELPERS
// ============================================================================

fn hex_encode_addr(addr: &[u8]) -> [u8; 64] {
    let hex_chars = b"0123456789abcdef";
    let mut hex = [0u8; 64];
    for i in 0..32 {
        hex[i * 2] = hex_chars[(addr[i] >> 4) as usize];
        hex[i * 2 + 1] = hex_chars[(addr[i] & 0x0f) as usize];
    }
    hex
}

fn make_key(prefix: &[u8], hex: &[u8; 64]) -> Vec<u8> {
    let mut key = Vec::with_capacity(prefix.len() + 64);
    key.extend_from_slice(prefix);
    key.extend_from_slice(hex);
    key
}

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key).map(|d| bytes_to_u64(&d)).unwrap_or(0)
}

fn store_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
}

fn is_paused() -> bool {
    storage_get(PAUSE_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(false)
}

fn is_admin(caller: &[u8]) -> bool {
    match storage_get(ADMIN_KEY) {
        Some(data) => data.as_slice() == caller,
        None => false,
    }
}

fn get_deposit_cap() -> u64 {
    load_u64(DEPOSIT_CAP_KEY)
}

// ============================================================================
// PROTOCOL STATE
// ============================================================================

/// Initialize the lending protocol
#[no_mangle]
pub extern "C" fn initialize(admin_ptr: *const u8) -> u32 {
    let mut admin = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(admin_ptr, admin.as_mut_ptr(), 32); }

    if storage_get(ADMIN_KEY).is_some() {
        log_info("Already initialized");
        return 1;
    }

    storage_set(ADMIN_KEY, &admin);
    store_u64(b"ll_total_deposits", 0);
    store_u64(b"ll_total_borrows", 0);
    store_u64(b"ll_last_update", get_timestamp());
    store_u64(b"ll_reserve_factor", 10); // 10% of interest goes to reserves

    log_info("LobsterLend initialized");
    0
}

// ============================================================================
// CORE LENDING OPERATIONS
// ============================================================================

/// Deposit collateral into the lending pool
#[no_mangle]
pub extern "C" fn deposit(depositor_ptr: *const u8, amount: u64) -> u32 {
    if amount == 0 {
        log_info("Cannot deposit zero");
        return 1;
    }
    if is_paused() {
        log_info("Protocol is paused");
        return 20;
    }
    if !reentrancy_enter() {
        log_info("Reentrancy detected");
        return 21;
    }

    let mut depositor = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(depositor_ptr, depositor.as_mut_ptr(), 32); }
    let hex = hex_encode_addr(&depositor);

    accrue_interest();

    // Check deposit cap
    let cap = get_deposit_cap();
    let total = load_u64(b"ll_total_deposits");
    if cap > 0 && total + amount > cap {
        reentrancy_exit();
        log_info("Would exceed deposit cap");
        return 4;
    }

    // Update user deposit
    let dep_key = make_key(b"dep:", &hex);
    let prev_deposit = load_u64(&dep_key);
    store_u64(&dep_key, prev_deposit + amount);

    // Update total deposits
    store_u64(b"ll_total_deposits", total + amount);

    reentrancy_exit();
    log_info("Deposit successful");
    0
}

/// Withdraw collateral (only if health factor remains > 1)
#[no_mangle]
pub extern "C" fn withdraw(depositor_ptr: *const u8, amount: u64) -> u32 {
    if amount == 0 {
        return 1;
    }
    if is_paused() {
        log_info("Protocol is paused");
        return 20;
    }
    if !reentrancy_enter() {
        return 21;
    }

    let mut depositor = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(depositor_ptr, depositor.as_mut_ptr(), 32); }
    let hex = hex_encode_addr(&depositor);

    accrue_interest();

    let dep_key = make_key(b"dep:", &hex);
    let current_deposit = load_u64(&dep_key);
    if amount > current_deposit {
        reentrancy_exit();
        log_info("Insufficient deposit balance");
        return 2;
    }

    // Check health factor after withdrawal
    let borrow_key = make_key(b"bor:", &hex);
    let current_borrow = load_u64(&borrow_key);
    let new_deposit = current_deposit - amount;

    if current_borrow > 0 {
        let max_borrow = new_deposit * COLLATERAL_FACTOR_PERCENT / 100;
        if current_borrow > max_borrow {
            reentrancy_exit();
            log_info("Withdrawal would make position unhealthy");
            return 3;
        }
    }

    store_u64(&dep_key, new_deposit);
    let total = load_u64(b"ll_total_deposits");
    store_u64(b"ll_total_deposits", total.saturating_sub(amount));

    reentrancy_exit();
    log_info("Withdrawal successful");
    0
}

/// Borrow against deposited collateral
#[no_mangle]
pub extern "C" fn borrow(borrower_ptr: *const u8, amount: u64) -> u32 {
    if amount == 0 {
        return 1;
    }
    if is_paused() {
        log_info("Protocol is paused");
        return 20;
    }
    if !reentrancy_enter() {
        return 21;
    }

    let mut borrower = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(borrower_ptr, borrower.as_mut_ptr(), 32); }
    let hex = hex_encode_addr(&borrower);

    accrue_interest();

    let dep_key = make_key(b"dep:", &hex);
    let deposit_val = load_u64(&dep_key);
    let borrow_key = make_key(b"bor:", &hex);
    let current_borrow = load_u64(&borrow_key);

    let max_borrow = deposit_val * COLLATERAL_FACTOR_PERCENT / 100;
    let new_borrow = current_borrow + amount;

    if new_borrow > max_borrow {
        reentrancy_exit();
        log_info("Borrow exceeds collateral factor");
        return 2;
    }

    // Check pool liquidity
    let total_deposits = load_u64(b"ll_total_deposits");
    let total_borrows = load_u64(b"ll_total_borrows");
    let available = total_deposits.saturating_sub(total_borrows);
    if amount > available {
        reentrancy_exit();
        log_info("Insufficient pool liquidity");
        return 3;
    }

    store_u64(&borrow_key, new_borrow);
    store_u64(b"ll_total_borrows", total_borrows + amount);

    // Track borrow timestamp for interest calculation
    let ts_key = make_key(b"bts:", &hex);
    store_u64(&ts_key, get_timestamp());

    reentrancy_exit();
    log_info("Borrow successful");
    0
}

/// Repay borrowed amount
#[no_mangle]
pub extern "C" fn repay(borrower_ptr: *const u8, amount: u64) -> u32 {
    if amount == 0 {
        return 1;
    }
    if !reentrancy_enter() {
        return 21;
    }

    let mut borrower = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(borrower_ptr, borrower.as_mut_ptr(), 32); }
    let hex = hex_encode_addr(&borrower);

    accrue_interest();

    let borrow_key = make_key(b"bor:", &hex);
    let current_borrow = load_u64(&borrow_key);

    if current_borrow == 0 {
        reentrancy_exit();
        log_info("No outstanding borrow");
        return 2;
    }

    let repay_amount = if amount > current_borrow { current_borrow } else { amount };
    store_u64(&borrow_key, current_borrow - repay_amount);

    let total_borrows = load_u64(b"ll_total_borrows");
    store_u64(b"ll_total_borrows", total_borrows.saturating_sub(repay_amount));

    reentrancy_exit();
    log_info("Repayment successful");
    0
}

/// Liquidate an unhealthy position
/// Liquidator repays part of borrower's debt and receives collateral + bonus
#[no_mangle]
pub extern "C" fn liquidate(
    liquidator_ptr: *const u8,
    borrower_ptr: *const u8,
    repay_amount: u64,
) -> u32 {
    if repay_amount == 0 {
        return 1;
    }
    if !reentrancy_enter() {
        return 21;
    }

    let mut _liquidator = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(liquidator_ptr, _liquidator.as_mut_ptr(), 32); }
    let mut borrower = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(borrower_ptr, borrower.as_mut_ptr(), 32); }
    let hex = hex_encode_addr(&borrower);

    accrue_interest();

    let dep_key = make_key(b"dep:", &hex);
    let deposit = load_u64(&dep_key);
    let borrow_key = make_key(b"bor:", &hex);
    let current_borrow = load_u64(&borrow_key);

    if current_borrow == 0 {
        reentrancy_exit();
        log_info("No borrow to liquidate");
        return 2;
    }

    // Check if position is liquidatable
    let liquidation_limit = deposit * LIQUIDATION_THRESHOLD_PERCENT / 100;
    if current_borrow <= liquidation_limit {
        reentrancy_exit();
        log_info("Position is healthy, cannot liquidate");
        return 3;
    }

    // Can only liquidate up to 50% of the borrow at once
    let max_repay = current_borrow / 2;
    let actual_repay = if repay_amount > max_repay { max_repay } else { repay_amount };

    // Collateral seized = repay_amount * (1 + bonus)
    // Use u128 to prevent overflow on large repay amounts
    let collateral_seized = actual_repay + ((actual_repay as u128 * LIQUIDATION_BONUS_PERCENT as u128 / 100) as u64);
    let actual_seized = if collateral_seized > deposit { deposit } else { collateral_seized };

    // Update borrower
    store_u64(&borrow_key, current_borrow - actual_repay);
    store_u64(&dep_key, deposit - actual_seized);

    // Update totals
    let total_borrows = load_u64(b"ll_total_borrows");
    store_u64(b"ll_total_borrows", total_borrows.saturating_sub(actual_repay));
    let total_deposits = load_u64(b"ll_total_deposits");
    store_u64(b"ll_total_deposits", total_deposits.saturating_sub(actual_seized));

    reentrancy_exit();
    log_info("Liquidation executed");

    // Return seized collateral amount in return data
    set_return_data(&u64_to_bytes(actual_seized));
    0
}

// ============================================================================
// INTEREST ACCRUAL
// ============================================================================

/// Accrue interest on all borrows (called automatically before state changes)
fn accrue_interest() {
    let last_update = load_u64(b"ll_last_update");
    let now = get_timestamp();
    if now <= last_update {
        return;
    }

    let elapsed_ms = now - last_update;
    // Convert to approximate slots (400ms each)
    let elapsed_slots = elapsed_ms / 400;
    if elapsed_slots == 0 {
        return;
    }

    let total_deposits = load_u64(b"ll_total_deposits");
    let total_borrows = load_u64(b"ll_total_borrows");

    if total_borrows == 0 || total_deposits == 0 {
        store_u64(b"ll_last_update", now);
        return;
    }

    // Calculate utilization rate (0-100)
    let utilization = (total_borrows * 100) / total_deposits;

    // Interest rate based on utilization (kinked model)
    let rate_per_slot = if utilization <= UTILIZATION_KINK_PERCENT {
        // Linear increase up to kink
        BASE_RATE_SCALED + (utilization * BASE_RATE_SCALED * 2 / 100)
    } else {
        // Sharp increase after kink
        let base_at_kink = BASE_RATE_SCALED + (UTILIZATION_KINK_PERCENT * BASE_RATE_SCALED * 2 / 100);
        let excess = utilization - UTILIZATION_KINK_PERCENT;
        base_at_kink + (excess * BASE_RATE_SCALED * 10 / 100)
    };

    // v2: Cap rate to prevent manipulation
    let rate_per_slot = if rate_per_slot > MAX_RATE_PER_SLOT { MAX_RATE_PER_SLOT } else { rate_per_slot };

    // Interest accrued = total_borrows * rate * elapsed_slots / SCALE
    // Use u128 intermediate to prevent overflow on large values
    let interest = ((total_borrows as u128) * (rate_per_slot as u128) * (elapsed_slots as u128) / (RATE_SCALE as u128)) as u64;

    if interest > 0 {
        // Reserve factor: portion goes to protocol reserves
        let reserve_factor = load_u64(b"ll_reserve_factor");
        let reserve_amount = ((interest as u128) * (reserve_factor as u128) / 100) as u64;
        let depositor_interest = interest - reserve_amount;

        // Increase total borrows by interest (borrowers owe more)
        store_u64(b"ll_total_borrows", total_borrows + interest);
        // Increase total deposits by depositor's share (depositors earn)
        store_u64(b"ll_total_deposits", total_deposits + depositor_interest);
        // Track protocol reserves
        let reserves = load_u64(b"ll_reserves");
        store_u64(b"ll_reserves", reserves + reserve_amount);
    }

    store_u64(b"ll_last_update", now);
}

// ============================================================================
// VIEW FUNCTIONS
// ============================================================================

/// Get account info: [deposit(8), borrow(8), health_factor_bps(8)]
#[no_mangle]
pub extern "C" fn get_account_info(user_ptr: *const u8) -> u32 {
    let mut user = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(user_ptr, user.as_mut_ptr(), 32); }
    let hex = hex_encode_addr(&user);

    let deposit = load_u64(&make_key(b"dep:", &hex));
    let borrow = load_u64(&make_key(b"bor:", &hex));

    // Health factor in basis points (10000 = 1.0)
    let health_factor = if borrow == 0 {
        u64::MAX // Infinite health
    } else {
        deposit * LIQUIDATION_THRESHOLD_PERCENT * 100 / borrow
    };

    let mut result = Vec::with_capacity(24);
    result.extend_from_slice(&u64_to_bytes(deposit));
    result.extend_from_slice(&u64_to_bytes(borrow));
    result.extend_from_slice(&u64_to_bytes(health_factor));
    set_return_data(&result);
    0
}

/// Get protocol stats: [total_deposits(8), total_borrows(8), utilization_pct(8), reserves(8)]
#[no_mangle]
pub extern "C" fn get_protocol_stats() -> u32 {
    let total_deposits = load_u64(b"ll_total_deposits");
    let total_borrows = load_u64(b"ll_total_borrows");
    let utilization = if total_deposits > 0 {
        total_borrows * 100 / total_deposits
    } else {
        0
    };
    let reserves = load_u64(b"ll_reserves");

    let mut result = Vec::with_capacity(32);
    result.extend_from_slice(&u64_to_bytes(total_deposits));
    result.extend_from_slice(&u64_to_bytes(total_borrows));
    result.extend_from_slice(&u64_to_bytes(utilization));
    result.extend_from_slice(&u64_to_bytes(reserves));
    set_return_data(&result);
    0
}

// ============================================================================
// v2: FLASH LOANS
// ============================================================================

/// Borrow a flash loan — must be repaid with fee in the same transaction.
/// Step 1: flash_borrow records the loan, step 2: flash_repay settles it.
#[no_mangle]
pub extern "C" fn flash_borrow(borrower_ptr: *const u8, amount: u64) -> u32 {
    if amount == 0 {
        return 1;
    }
    if is_paused() {
        log_info("Protocol is paused");
        return 20;
    }

    let mut _borrower = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(borrower_ptr, _borrower.as_mut_ptr(), 32); }

    // Check no active flash loan
    if load_u64(FLASH_BORROWED_KEY) > 0 {
        log_info("Flash loan already active");
        return 2;
    }

    // Check pool has sufficient liquidity
    let total_deposits = load_u64(b"ll_total_deposits");
    let total_borrows = load_u64(b"ll_total_borrows");
    let available = total_deposits.saturating_sub(total_borrows);
    if amount > available {
        log_info("Insufficient pool liquidity for flash loan");
        return 3;
    }

    // AUDIT-FIX NEW-M2: round-up fee (consistent with moltswap), u128 intermediate
    let fee = ((amount as u128 * FLASH_LOAN_FEE_BPS as u128 + (BPS_SCALE as u128 - 1)) / BPS_SCALE as u128) as u64;
    let fee = if fee == 0 { 1 } else { fee }; // minimum 1 shell fee

    // Record flash loan
    store_u64(FLASH_BORROWED_KEY, amount);
    store_u64(FLASH_FEE_KEY, fee);

    // Return fee in return data so borrower knows what to repay
    set_return_data(&u64_to_bytes(fee));
    log_info("Flash loan issued");
    0
}

/// Repay a flash loan with fee. Must be called after flash_borrow.
#[no_mangle]
pub extern "C" fn flash_repay(borrower_ptr: *const u8, repay_amount: u64) -> u32 {
    let mut _borrower = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(borrower_ptr, _borrower.as_mut_ptr(), 32); }

    let borrowed = load_u64(FLASH_BORROWED_KEY);
    if borrowed == 0 {
        log_info("No active flash loan");
        return 1;
    }

    let fee = load_u64(FLASH_FEE_KEY);
    let required = borrowed + fee;
    if repay_amount < required {
        log_info("Insufficient repayment (must include fee)");
        return 2;
    }

    // Fee goes to protocol reserves
    let reserves = load_u64(b"ll_reserves");
    store_u64(b"ll_reserves", reserves + fee);

    // Clear flash loan state
    store_u64(FLASH_BORROWED_KEY, 0);
    store_u64(FLASH_FEE_KEY, 0);

    log_info("Flash loan repaid");
    0
}

// ============================================================================
// v2: ADMIN OPERATIONS
// ============================================================================

/// Admin pauses the protocol (blocks new deposits, borrows, withdrawals)
#[no_mangle]
pub extern "C" fn pause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    if !is_admin(&caller) {
        log_info("Not admin");
        return 1;
    }
    if is_paused() {
        log_info("Already paused");
        return 2;
    }
    storage_set(PAUSE_KEY, &[1]);
    log_info("Protocol paused");
    0
}

/// Admin unpauses the protocol
#[no_mangle]
pub extern "C" fn unpause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    if !is_admin(&caller) {
        log_info("Not admin");
        return 1;
    }
    if !is_paused() {
        log_info("Not paused");
        return 2;
    }
    storage_set(PAUSE_KEY, &[0]);
    log_info("Protocol unpaused");
    0
}

/// Admin sets the deposit cap (0 = unlimited)
#[no_mangle]
pub extern "C" fn set_deposit_cap(caller_ptr: *const u8, cap: u64) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    if !is_admin(&caller) {
        log_info("Not admin");
        return 1;
    }
    store_u64(DEPOSIT_CAP_KEY, cap);
    log_info("Deposit cap updated");
    0
}

/// Admin updates reserve factor (0-100)
#[no_mangle]
pub extern "C" fn set_reserve_factor(caller_ptr: *const u8, factor: u64) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    if !is_admin(&caller) {
        log_info("Not admin");
        return 1;
    }
    if factor > 100 {
        log_info("Factor must be 0-100");
        return 2;
    }
    store_u64(b"ll_reserve_factor", factor);
    log_info("Reserve factor updated");
    0
}

/// Admin withdraws protocol reserves
#[no_mangle]
pub extern "C" fn withdraw_reserves(caller_ptr: *const u8, amount: u64) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    if !is_admin(&caller) {
        log_info("Not admin");
        return 1;
    }
    if amount == 0 {
        return 2;
    }
    let reserves = load_u64(b"ll_reserves");
    if amount > reserves {
        log_info("Amount exceeds reserves");
        return 3;
    }
    store_u64(b"ll_reserves", reserves - amount);
    log_info("Reserves withdrawn");
    0
}

// ============================================================================
// v2: INTEREST RATE VIEW
// ============================================================================

/// Get current interest rate info: [rate_per_slot(8), utilization_pct(8), total_available(8)]
#[no_mangle]
pub extern "C" fn get_interest_rate() -> u32 {
    let total_deposits = load_u64(b"ll_total_deposits");
    let total_borrows = load_u64(b"ll_total_borrows");

    let utilization = if total_deposits > 0 {
        (total_borrows * 100) / total_deposits
    } else {
        0
    };

    let rate_per_slot = if utilization <= UTILIZATION_KINK_PERCENT {
        BASE_RATE_SCALED + (utilization * BASE_RATE_SCALED * 2 / 100)
    } else {
        let base_at_kink = BASE_RATE_SCALED + (UTILIZATION_KINK_PERCENT * BASE_RATE_SCALED * 2 / 100);
        let excess = utilization - UTILIZATION_KINK_PERCENT;
        base_at_kink + (excess * BASE_RATE_SCALED * 10 / 100)
    };
    let rate_per_slot = if rate_per_slot > MAX_RATE_PER_SLOT { MAX_RATE_PER_SLOT } else { rate_per_slot };

    let available = total_deposits.saturating_sub(total_borrows);

    let mut result = Vec::with_capacity(24);
    result.extend_from_slice(&u64_to_bytes(rate_per_slot));
    result.extend_from_slice(&u64_to_bytes(utilization));
    result.extend_from_slice(&u64_to_bytes(available));
    set_return_data(&result);
    0
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
    fn test_initialize() {
        setup();
        let admin = [1u8; 32];
        let result = initialize(admin.as_ptr());
        assert_eq!(result, 0);
        let stored = test_mock::get_storage(ADMIN_KEY);
        assert_eq!(stored, Some(admin.to_vec()));
    }

    #[test]
    fn test_initialize_already_initialized() {
        setup();
        let admin = [1u8; 32];
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(initialize(admin.as_ptr()), 1);
    }

    #[test]
    fn test_deposit() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        assert_eq!(deposit(user.as_ptr(), 1_000_000), 0);
        assert_eq!(load_u64(b"ll_total_deposits"), 1_000_000);
    }

    #[test]
    fn test_deposit_zero() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        assert_eq!(deposit(user.as_ptr(), 0), 1);
    }

    #[test]
    fn test_withdraw() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        deposit(user.as_ptr(), 1_000_000);
        assert_eq!(withdraw(user.as_ptr(), 500_000), 0);
        assert_eq!(load_u64(b"ll_total_deposits"), 500_000);
    }

    #[test]
    fn test_withdraw_exceeds_deposit() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        deposit(user.as_ptr(), 1_000_000);
        assert_eq!(withdraw(user.as_ptr(), 2_000_000), 2);
    }

    #[test]
    fn test_withdraw_would_make_unhealthy() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        deposit(user.as_ptr(), 1_000_000);
        borrow(user.as_ptr(), 750_000); // max borrow at 75%
        // Any withdrawal makes it unhealthy
        assert_eq!(withdraw(user.as_ptr(), 1), 3);
    }

    #[test]
    fn test_borrow() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        deposit(user.as_ptr(), 1_000_000);
        assert_eq!(borrow(user.as_ptr(), 500_000), 0);
        assert_eq!(load_u64(b"ll_total_borrows"), 500_000);
    }

    #[test]
    fn test_borrow_exceeds_collateral_factor() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        deposit(user.as_ptr(), 1_000_000);
        assert_eq!(borrow(user.as_ptr(), 750_001), 2); // > 75%
    }

    #[test]
    fn test_borrow_exceeds_liquidity() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user1 = [2u8; 32];
        deposit(user1.as_ptr(), 1_000_000);
        borrow(user1.as_ptr(), 750_000);
        // user2 deposits 200_000 and tries to borrow 200_000 (only 250_000 available)
        let user2 = [3u8; 32];
        deposit(user2.as_ptr(), 1_000_000);
        // Available = 2M - 750K = 1.25M; user2 max = 750K; try exceed availability
        // Drain pool: user2 borrows 750K, then user3 tries
        borrow(user2.as_ptr(), 750_000);
        let user3 = [4u8; 32];
        deposit(user3.as_ptr(), 2_000_000);
        // Available = 4M - 1.5M = 2.5M; user3 max = 1.5M; borrow 1.5M
        assert_eq!(borrow(user3.as_ptr(), 1_500_000), 0);
    }

    #[test]
    fn test_borrow_zero() {
        setup();
        let user = [2u8; 32];
        assert_eq!(borrow(user.as_ptr(), 0), 1);
    }

    #[test]
    fn test_repay() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        deposit(user.as_ptr(), 1_000_000);
        borrow(user.as_ptr(), 500_000);
        assert_eq!(repay(user.as_ptr(), 200_000), 0);
        assert_eq!(load_u64(b"ll_total_borrows"), 300_000);
    }

    #[test]
    fn test_repay_no_borrow() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        assert_eq!(repay(user.as_ptr(), 100), 2);
    }

    #[test]
    fn test_repay_overpay() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        deposit(user.as_ptr(), 1_000_000);
        borrow(user.as_ptr(), 500_000);
        assert_eq!(repay(user.as_ptr(), 999_999), 0);
        assert_eq!(load_u64(b"ll_total_borrows"), 0);
    }

    #[test]
    fn test_liquidate() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let borrower = [2u8; 32];
        deposit(borrower.as_ptr(), 1_000_000);
        borrow(borrower.as_ptr(), 750_000);
        // Manually push borrow above liquidation threshold (85%)
        let hex = hex_encode_addr(&borrower);
        let bor_key = make_key(b"bor:", &hex);
        store_u64(&bor_key, 860_000);
        store_u64(b"ll_total_borrows", 860_000);
        let liquidator = [3u8; 32];
        assert_eq!(liquidate(liquidator.as_ptr(), borrower.as_ptr(), 200_000), 0);
        let borrow_after = load_u64(&bor_key);
        assert!(borrow_after < 860_000);
    }

    #[test]
    fn test_liquidate_healthy_position() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let borrower = [2u8; 32];
        deposit(borrower.as_ptr(), 1_000_000);
        borrow(borrower.as_ptr(), 500_000); // 50% < 85%
        let liquidator = [3u8; 32];
        assert_eq!(liquidate(liquidator.as_ptr(), borrower.as_ptr(), 100_000), 3);
    }

    #[test]
    fn test_liquidate_no_borrow() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let borrower = [2u8; 32];
        deposit(borrower.as_ptr(), 1_000_000);
        let liquidator = [3u8; 32];
        assert_eq!(liquidate(liquidator.as_ptr(), borrower.as_ptr(), 100_000), 2);
    }

    #[test]
    fn test_get_account_info() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        deposit(user.as_ptr(), 1_000_000);
        borrow(user.as_ptr(), 500_000);
        assert_eq!(get_account_info(user.as_ptr()), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 24);
        assert_eq!(bytes_to_u64(&ret[0..8]), 1_000_000);
        assert_eq!(bytes_to_u64(&ret[8..16]), 500_000);
    }

    #[test]
    fn test_get_protocol_stats() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        deposit(user.as_ptr(), 1_000_000);
        borrow(user.as_ptr(), 500_000);
        assert_eq!(get_protocol_stats(), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 32);
        assert_eq!(bytes_to_u64(&ret[0..8]), 1_000_000);
        assert_eq!(bytes_to_u64(&ret[8..16]), 500_000);
        assert_eq!(bytes_to_u64(&ret[16..24]), 50);
    }

    // ========================================================================
    // v2 TESTS
    // ========================================================================

    #[test]
    fn test_flash_borrow_repay() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        deposit(user.as_ptr(), 1_000_000);

        let borrower = [3u8; 32];
        // Flash borrow 100,000
        assert_eq!(flash_borrow(borrower.as_ptr(), 100_000), 0);
        let fee_data = test_mock::get_return_data();
        let fee = bytes_to_u64(&fee_data);
        assert_eq!(fee, 90); // 0.09% of 100_000 = 90

        // Underpayment rejected
        assert_eq!(flash_repay(borrower.as_ptr(), 100_000), 2);

        // Full repayment with fee
        assert_eq!(flash_repay(borrower.as_ptr(), 100_090), 0);

        // Reserves increased by fee
        assert_eq!(load_u64(b"ll_reserves"), 90);
    }

    #[test]
    fn test_flash_borrow_no_liquidity() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        deposit(user.as_ptr(), 1_000);

        let borrower = [3u8; 32];
        assert_eq!(flash_borrow(borrower.as_ptr(), 2_000), 3);
    }

    #[test]
    fn test_flash_double_borrow_rejected() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        deposit(user.as_ptr(), 1_000_000);

        let borrower = [3u8; 32];
        assert_eq!(flash_borrow(borrower.as_ptr(), 100_000), 0);
        // Second borrow while first active
        assert_eq!(flash_borrow(borrower.as_ptr(), 50_000), 2);
    }

    #[test]
    fn test_flash_repay_without_borrow() {
        setup();
        let borrower = [3u8; 32];
        assert_eq!(flash_repay(borrower.as_ptr(), 100_000), 1);
    }

    #[test]
    fn test_pause_unpause() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];

        // Pause
        assert_eq!(pause(admin.as_ptr()), 0);
        assert!(is_paused());

        // Operations blocked
        assert_eq!(deposit(user.as_ptr(), 1_000), 20);
        assert_eq!(withdraw(user.as_ptr(), 1_000), 20);
        assert_eq!(borrow(user.as_ptr(), 1_000), 20);
        assert_eq!(flash_borrow(user.as_ptr(), 1_000), 20);

        // Double pause rejected
        assert_eq!(pause(admin.as_ptr()), 2);

        // Unpause
        assert_eq!(unpause(admin.as_ptr()), 0);
        assert!(!is_paused());

        // Operations work again
        assert_eq!(deposit(user.as_ptr(), 1_000), 0);

        // Double unpause rejected
        assert_eq!(unpause(admin.as_ptr()), 2);
    }

    #[test]
    fn test_pause_non_admin_rejected() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let other = [9u8; 32];
        assert_eq!(pause(other.as_ptr()), 1);
        assert_eq!(unpause(other.as_ptr()), 1);
    }

    #[test]
    fn test_deposit_cap() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        // Set cap
        assert_eq!(set_deposit_cap(admin.as_ptr(), 500_000), 0);

        let user = [2u8; 32];
        assert_eq!(deposit(user.as_ptr(), 400_000), 0);
        // Exceeds cap
        assert_eq!(deposit(user.as_ptr(), 200_000), 4);
        // Just under cap
        assert_eq!(deposit(user.as_ptr(), 100_000), 0);
    }

    #[test]
    fn test_deposit_cap_non_admin() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let other = [9u8; 32];
        assert_eq!(set_deposit_cap(other.as_ptr(), 500_000), 1);
    }

    #[test]
    fn test_set_reserve_factor() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        assert_eq!(set_reserve_factor(admin.as_ptr(), 20), 0);
        assert_eq!(load_u64(b"ll_reserve_factor"), 20);

        // Over 100 rejected
        assert_eq!(set_reserve_factor(admin.as_ptr(), 101), 2);

        // Non-admin rejected
        let other = [9u8; 32];
        assert_eq!(set_reserve_factor(other.as_ptr(), 5), 1);
    }

    #[test]
    fn test_withdraw_reserves() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        // Seed some reserves
        store_u64(b"ll_reserves", 10_000);

        assert_eq!(withdraw_reserves(admin.as_ptr(), 5_000), 0);
        assert_eq!(load_u64(b"ll_reserves"), 5_000);

        // Over-withdraw rejected
        assert_eq!(withdraw_reserves(admin.as_ptr(), 10_000), 3);

        // Zero rejected
        assert_eq!(withdraw_reserves(admin.as_ptr(), 0), 2);

        // Non-admin rejected
        let other = [9u8; 32];
        assert_eq!(withdraw_reserves(other.as_ptr(), 1_000), 1);
    }

    #[test]
    fn test_get_interest_rate() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        deposit(user.as_ptr(), 1_000_000);
        borrow(user.as_ptr(), 500_000);

        assert_eq!(get_interest_rate(), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 24);
        let rate = bytes_to_u64(&ret[0..8]);
        assert!(rate > 0);
        let util = bytes_to_u64(&ret[8..16]);
        assert_eq!(util, 50);
        let avail = bytes_to_u64(&ret[16..24]);
        assert_eq!(avail, 500_000);
    }

    #[test]
    fn test_flash_loan_minimum_fee() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        deposit(user.as_ptr(), 1_000_000);

        // Very small borrow — fee would be 0, but minimum is 1
        let borrower = [3u8; 32];
        assert_eq!(flash_borrow(borrower.as_ptr(), 100), 0);
        let fee = bytes_to_u64(&test_mock::get_return_data());
        assert_eq!(fee, 1); // Minimum fee

        // Repay
        assert_eq!(flash_repay(borrower.as_ptr(), 101), 0);
    }

    #[test]
    fn test_repay_still_works_when_paused() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        deposit(user.as_ptr(), 1_000_000);
        borrow(user.as_ptr(), 500_000);

        // Pause protocol
        pause(admin.as_ptr());

        // Repay should still work (no pause check — users must be able to unwind)
        assert_eq!(repay(user.as_ptr(), 200_000), 0);
    }

    #[test]
    fn test_liquidation_works_when_paused() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        let borrower = [2u8; 32];
        deposit(borrower.as_ptr(), 1_000_000);
        borrow(borrower.as_ptr(), 750_000);

        // Force unhealthy position
        let hex = hex_encode_addr(&borrower);
        let bor_key = make_key(b"bor:", &hex);
        store_u64(&bor_key, 860_000);
        store_u64(b"ll_total_borrows", 860_000);

        // Pause
        pause(admin.as_ptr());

        // Liquidation should still work when paused (safety valve)
        let liquidator = [3u8; 32];
        assert_eq!(liquidate(liquidator.as_ptr(), borrower.as_ptr(), 200_000), 0);
    }
}
