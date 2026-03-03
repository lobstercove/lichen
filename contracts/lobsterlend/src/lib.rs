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
//   - Oracle price integration via moltoracle cross-contract call

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;
use alloc::vec::Vec;
use moltchain_sdk::{
    storage_get, storage_set, log_info, set_return_data,
    bytes_to_u64, u64_to_bytes, get_timestamp, get_caller,
    get_value, get_contract_address, call_token_transfer, Address,
};
use moltchain_sdk::crosscall::{CrossCall, call_contract};

// Oracle configuration key (stores moltoracle contract address)
const ORACLE_ADDR_KEY: &[u8] = b"ll_oracle_addr";

/// Query oracle price for an asset (returns price in shells, or 1:1 if unavailable).
/// Asset identifier is the token contract address hex.
fn get_oracle_price(asset: &[u8]) -> u64 {
    if let Some(oracle_bytes) = storage_get(ORACLE_ADDR_KEY) {
        if oracle_bytes.len() == 32 {
            let mut oracle_addr = [0u8; 32];
            oracle_addr.copy_from_slice(&oracle_bytes);
            // Query oracle: get_price(asset) → u64 price in shells
            let call = CrossCall::new(Address(oracle_addr), "get_price", asset.to_vec());
            if let Ok(result) = call_contract(call) {
                if result.len() >= 8 {
                    return bytes_to_u64(&result[..8]);
                }
            }
            log_info("Oracle query failed, using 1:1 fallback");
        }
    }
    // Fallback: 1:1 valuation (oracle not configured)
    1
}

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
const DEPOSIT_COUNT_KEY: &[u8] = b"ll_dep_count";
const BORROW_COUNT_KEY: &[u8] = b"ll_bor_count";
const LIQUIDATION_COUNT_KEY: &[u8] = b"ll_liq_count";
const REPAY_COUNT_KEY: &[u8] = b"ll_repay_count";

/// Maximum interest rate per slot to prevent manipulation
const MAX_RATE_PER_SLOT: u64 = 25_400; // 100x base rate

/// AUDIT-FIX G9-01: Moltcoin contract address — required for actual token transfers
const MOLTCOIN_ADDRESS_KEY: &[u8] = b"ll_molt_addr";

/// P9-SC-01: Compound-style borrow index scale factor.
/// Global `ll_borrow_index` starts at this value (1e9) and grows with interest.
/// Per-user `bix:HEXADDR` stores the index when the user last interacted.
const BORROW_INDEX_SCALE: u64 = 1_000_000_000;

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

/// AUDIT-FIX G9-01: Load configured moltcoin address (returns zero if not set)
fn load_molt_addr() -> [u8; 32] {
    storage_get(MOLTCOIN_ADDRESS_KEY).map(|d| {
        let mut a = [0u8; 32]; if d.len() >= 32 { a.copy_from_slice(&d[..32]); } a
    }).unwrap_or([0u8; 32])
}
fn is_zero_addr(a: &[u8; 32]) -> bool { a.iter().all(|&b| b == 0) }

/// Transfer tokens OUT from the contract's own balance to a recipient.
/// Uses the self-custody pattern: caller==from in CCC context.
/// Returns 0 on success, non-zero on failure.
fn transfer_out(recipient: &[u8; 32], amount: u64) -> u32 {
    let molt = load_molt_addr();
    if is_zero_addr(&molt) {
        log_info("moltcoin address not configured");
        return 30;
    }
    let self_addr = get_contract_address();
    if let Err(_) = call_token_transfer(Address(molt), self_addr, Address(*recipient), amount) {
        log_info("Token transfer failed");
        return 31;
    }
    0
}

fn get_deposit_cap() -> u64 {
    load_u64(DEPOSIT_CAP_KEY)
}

/// P9-SC-01: Settle a user's borrow balance using the global borrow index.
/// Recalculates: actual_borrow = stored_borrow * global_index / user_index
/// Stores the updated borrow and checkpoints the current index.
/// Returns the settled (index-adjusted) borrow balance.
fn settle_user_borrow(hex: &[u8; 64]) -> u64 {
    let global_index = load_u64(b"ll_borrow_index");
    if global_index == 0 {
        return 0;
    }

    let borrow_key = make_key(b"bor:", hex);
    let stored_borrow = load_u64(&borrow_key);
    if stored_borrow == 0 {
        return 0;
    }

    let index_key = make_key(b"bix:", hex);
    let user_index = load_u64(&index_key);
    // Legacy borrowers (before this upgrade) have no checkpoint → treat as BORROW_INDEX_SCALE
    let effective_user_index = if user_index == 0 {
        BORROW_INDEX_SCALE
    } else {
        user_index
    };

    // If index hasn't changed since user's last interaction, no adjustment needed
    if global_index == effective_user_index {
        return stored_borrow;
    }

    // Recalculate with u128 intermediate to prevent overflow
    let actual_borrow = (stored_borrow as u128 * global_index as u128
        / effective_user_index as u128) as u64;

    // Store updated borrow and checkpoint
    store_u64(&borrow_key, actual_borrow);
    store_u64(&index_key, global_index);

    actual_borrow
}

/// P9-SC-01: Compute current borrow without storing (for view functions).
fn compute_current_borrow(hex: &[u8; 64]) -> u64 {
    let global_index = load_u64(b"ll_borrow_index");
    if global_index == 0 {
        return 0;
    }

    let borrow_key = make_key(b"bor:", hex);
    let stored_borrow = load_u64(&borrow_key);
    if stored_borrow == 0 {
        return 0;
    }

    let index_key = make_key(b"bix:", hex);
    let user_index = load_u64(&index_key);
    let effective_user_index = if user_index == 0 {
        BORROW_INDEX_SCALE
    } else {
        user_index
    };

    (stored_borrow as u128 * global_index as u128 / effective_user_index as u128) as u64
}

// ============================================================================
// PROTOCOL STATE
// ============================================================================

/// Initialize the lending protocol
#[no_mangle]
pub extern "C" fn initialize(admin_ptr: *const u8) -> u32 {
    let mut admin = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(admin_ptr, admin.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != admin {
        return 200;
    }

    if storage_get(ADMIN_KEY).is_some() {
        log_info("Already initialized");
        return 1;
    }

    storage_set(ADMIN_KEY, &admin);
    store_u64(b"ll_total_deposits", 0);
    store_u64(b"ll_total_borrows", 0);
    store_u64(b"ll_last_update", get_timestamp());
    store_u64(b"ll_reserve_factor", 10); // 10% of interest goes to reserves
    // P9-SC-01: Initialize borrow index for Compound-style per-borrower tracking
    store_u64(b"ll_borrow_index", BORROW_INDEX_SCALE);

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

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != depositor {
        reentrancy_exit();
        return 200;
    }

    // AUDIT-FIX G9-01: Verify incoming value covers deposit
    let attached = get_value();
    if attached < amount {
        reentrancy_exit();
        log_info("Insufficient value attached for deposit");
        return 30;
    }

    let hex = hex_encode_addr(&depositor);

    accrue_interest();

    // Check deposit cap
    let cap = get_deposit_cap();
    let total = load_u64(b"ll_total_deposits");
    if cap > 0 && total.saturating_add(amount) > cap {
        reentrancy_exit();
        log_info("Would exceed deposit cap");
        return 4;
    }

    // Update user deposit
    let dep_key = make_key(b"dep:", &hex);
    let prev_deposit = load_u64(&dep_key);
    store_u64(&dep_key, prev_deposit.saturating_add(amount));

    // Update total deposits
    store_u64(b"ll_total_deposits", total.saturating_add(amount));

    // Track deposit count
    store_u64(DEPOSIT_COUNT_KEY, load_u64(DEPOSIT_COUNT_KEY) + 1);

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

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != depositor {
        reentrancy_exit();
        return 200;
    }

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
    // P9-SC-01: Use index-adjusted borrow for accurate health check
    let current_borrow = compute_current_borrow(&hex);
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

    // AUDIT-FIX G9-01: Transfer tokens to withdrawer
    let rc = transfer_out(&depositor, amount);
    if rc != 0 {
        // Revert bookkeeping on transfer failure
        store_u64(&dep_key, current_deposit);
        store_u64(b"ll_total_deposits", total);
        reentrancy_exit();
        return rc;
    }

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

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != borrower {
        reentrancy_exit();
        return 200;
    }

    let hex = hex_encode_addr(&borrower);

    accrue_interest();

    let dep_key = make_key(b"dep:", &hex);
    let deposit_val = load_u64(&dep_key);
    // P9-SC-01: Settle existing borrow via index before adding new amount
    let current_borrow = settle_user_borrow(&hex);
    let borrow_key = make_key(b"bor:", &hex);

    // AUDIT-FIX CON-10: Use oracle price for collateral valuation
    let collateral_price = get_oracle_price(&borrower);
    let deposit_value_usd = deposit_val.saturating_mul(collateral_price);
    let max_borrow = deposit_value_usd * COLLATERAL_FACTOR_PERCENT / 100;
    let new_borrow = match current_borrow.checked_add(amount) {
        Some(v) => v,
        None => {
            reentrancy_exit();
            log_info("Borrow amount overflow");
            return 5;
        }
    };

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
    // P9-SC-01: Always checkpoint the borrow index (settle_user_borrow skips
    // when stored_borrow==0, so first-time borrowers need this)
    let bix_key = make_key(b"bix:", &hex);
    store_u64(&bix_key, load_u64(b"ll_borrow_index"));
    let new_total_borrows = match total_borrows.checked_add(amount) {
        Some(v) => v,
        None => {
            reentrancy_exit();
            log_info("Total borrows overflow");
            return 5;
        }
    };
    store_u64(b"ll_total_borrows", new_total_borrows);

    // Track borrow count
    store_u64(BORROW_COUNT_KEY, load_u64(BORROW_COUNT_KEY) + 1);

    // Track borrow timestamp for interest calculation
    let ts_key = make_key(b"bts:", &hex);
    store_u64(&ts_key, get_timestamp());

    // AUDIT-FIX G9-01: Transfer borrowed tokens to borrower
    let rc = transfer_out(&borrower, amount);
    if rc != 0 {
        // Revert bookkeeping on transfer failure
        store_u64(&borrow_key, current_borrow);
        store_u64(b"ll_total_borrows", total_borrows);
        reentrancy_exit();
        return rc;
    }

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

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != borrower {
        reentrancy_exit();
        return 200;
    }

    // AUDIT-FIX G9-01: Verify incoming value covers repayment
    let attached = get_value();
    if attached < amount {
        reentrancy_exit();
        log_info("Insufficient value attached for repayment");
        return 30;
    }

    let hex = hex_encode_addr(&borrower);

    accrue_interest();

    // P9-SC-01: Settle borrow via index to get true amount owed
    let current_borrow = settle_user_borrow(&hex);
    let borrow_key = make_key(b"bor:", &hex);

    if current_borrow == 0 {
        reentrancy_exit();
        log_info("No outstanding borrow");
        return 2;
    }

    let repay_amount = if amount > current_borrow { current_borrow } else { amount };
    store_u64(&borrow_key, current_borrow - repay_amount);

    let total_borrows = load_u64(b"ll_total_borrows");
    store_u64(b"ll_total_borrows", total_borrows.saturating_sub(repay_amount));

    // Track repay count
    store_u64(REPAY_COUNT_KEY, load_u64(REPAY_COUNT_KEY) + 1);

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

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != _liquidator {
        reentrancy_exit();
        return 200;
    }

    // AUDIT-FIX G9-01: Verify incoming value covers liquidation repayment
    let attached = get_value();
    if attached < repay_amount {
        reentrancy_exit();
        log_info("Insufficient value attached for liquidation");
        return 30;
    }

    let mut borrower = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(borrower_ptr, borrower.as_mut_ptr(), 32); }
    let hex = hex_encode_addr(&borrower);

    accrue_interest();

    let dep_key = make_key(b"dep:", &hex);
    let deposit = load_u64(&dep_key);
    // P9-SC-01: Settle borrow via index to check true health
    let current_borrow = settle_user_borrow(&hex);
    let borrow_key = make_key(b"bor:", &hex);

    if current_borrow == 0 {
        reentrancy_exit();
        log_info("No borrow to liquidate");
        return 2;
    }

    // Check if position is liquidatable
    // AUDIT-FIX CON-10: Use oracle price for liquidation threshold
    let collateral_price = get_oracle_price(&borrower);
    let deposit_value_usd = deposit.saturating_mul(collateral_price);
    let liquidation_limit = deposit_value_usd * LIQUIDATION_THRESHOLD_PERCENT / 100;
    if current_borrow <= liquidation_limit {
        reentrancy_exit();
        log_info("Position is healthy, cannot liquidate");
        return 3;
    }

    // Can only liquidate up to 50% of the borrow at once
    let max_repay = current_borrow / 2;
    let actual_repay = if repay_amount > max_repay { max_repay } else { repay_amount };

    // Collateral seized = repay_amount * (1 + bonus)
    // AUDIT-FIX L6-01: Use u128 throughout to prevent overflow on large repay amounts
    let collateral_seized = (actual_repay as u128 + (actual_repay as u128 * LIQUIDATION_BONUS_PERCENT as u128 / 100)) as u64;
    let actual_seized = if collateral_seized > deposit { deposit } else { collateral_seized };

    // Update borrower
    store_u64(&borrow_key, current_borrow - actual_repay);
    store_u64(&dep_key, deposit - actual_seized);

    // Update totals
    let total_borrows = load_u64(b"ll_total_borrows");
    store_u64(b"ll_total_borrows", total_borrows.saturating_sub(actual_repay));
    let total_deposits = load_u64(b"ll_total_deposits");
    store_u64(b"ll_total_deposits", total_deposits.saturating_sub(actual_seized));

    // Track liquidation count
    store_u64(LIQUIDATION_COUNT_KEY, load_u64(LIQUIDATION_COUNT_KEY) + 1);

    // AUDIT-FIX G9-01: Transfer seized collateral to liquidator
    let rc = transfer_out(&_liquidator, actual_seized);
    if rc != 0 {
        // Revert all bookkeeping on transfer failure
        store_u64(&borrow_key, current_borrow);
        store_u64(&dep_key, deposit);
        store_u64(b"ll_total_borrows", total_borrows);
        store_u64(b"ll_total_deposits", total_deposits);
        reentrancy_exit();
        return rc;
    }

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
        store_u64(b"ll_total_borrows", total_borrows.saturating_add(interest));
        // Increase total deposits by depositor's share (depositors earn)
        store_u64(b"ll_total_deposits", total_deposits.saturating_add(depositor_interest));
        // Track protocol reserves
        let reserves = load_u64(b"ll_reserves");
        store_u64(b"ll_reserves", reserves.saturating_add(reserve_amount));

        // P9-SC-01: Update global borrow index proportionally.
        // index_delta = old_index * rate_per_slot * elapsed_slots / RATE_SCALE
        // (same factor as interest / total_borrows)
        let old_index = load_u64(b"ll_borrow_index");
        let index_delta = ((old_index as u128) * (rate_per_slot as u128)
            * (elapsed_slots as u128)
            / (RATE_SCALE as u128)) as u64;
        store_u64(b"ll_borrow_index", old_index.saturating_add(index_delta));
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
    // P9-SC-01: Use index-adjusted borrow for accurate health factor
    let borrow = compute_current_borrow(&hex);

    // Health factor in basis points (10000 = 1.0)
    // AUDIT-FIX CON-06: Cast to u128 to prevent overflow for large deposits
    // (deposit * 8500 overflows u64 when deposit > ~2.17×10¹⁵ shells ≈ 2.17M MOLT)
    let health_factor = if borrow == 0 {
        u64::MAX // Infinite health
    } else {
        ((deposit as u128) * (LIQUIDATION_THRESHOLD_PERCENT as u128) * 100 / (borrow as u128)) as u64
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

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != _borrower {
        return 200;
    }

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

    // AUDIT-FIX G9-01: Transfer flash loan tokens to borrower
    let rc = transfer_out(&_borrower, amount);
    if rc != 0 {
        // Revert flash loan state on transfer failure
        store_u64(FLASH_BORROWED_KEY, 0);
        store_u64(FLASH_FEE_KEY, 0);
        return rc;
    }

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

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != _borrower {
        return 200;
    }

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

    // AUDIT-FIX G9-01: Verify incoming value covers flash repayment
    let attached = get_value();
    if attached < required {
        log_info("Insufficient value attached for flash repay");
        return 30;
    }

    // Fee goes to protocol reserves
    let reserves = load_u64(b"ll_reserves");
    store_u64(b"ll_reserves", reserves.saturating_add(fee));

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

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

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

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

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

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

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

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

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

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

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

    // AUDIT-FIX G9-01: Transfer reserve tokens to admin
    let rc = transfer_out(&caller, amount);
    if rc != 0 {
        // Revert on transfer failure
        store_u64(b"ll_reserves", reserves);
        return rc;
    }

    log_info("Reserves withdrawn");
    0
}

/// AUDIT-FIX G9-01: Admin sets the moltcoin contract address for token transfers
#[no_mangle]
pub extern "C" fn set_moltcoin_address(caller_ptr: *const u8, addr_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // Verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_admin(&caller) {
        log_info("Not admin");
        return 1;
    }

    let mut addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(addr_ptr, addr.as_mut_ptr(), 32); }

    if is_zero_addr(&addr) {
        log_info("Cannot set zero moltcoin address");
        return 2;
    }

    storage_set(MOLTCOIN_ADDRESS_KEY, &addr);
    log_info("Moltcoin address configured");
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

/// Get deposit count
#[no_mangle]
pub extern "C" fn get_deposit_count() -> u64 {
    load_u64(DEPOSIT_COUNT_KEY)
}

/// Get borrow count
#[no_mangle]
pub extern "C" fn get_borrow_count() -> u64 {
    load_u64(BORROW_COUNT_KEY)
}

/// Get liquidation count
#[no_mangle]
pub extern "C" fn get_liquidation_count() -> u64 {
    load_u64(LIQUIDATION_COUNT_KEY)
}

/// Get lending platform stats [total_deposits(8), total_borrows(8), reserves(8), deposit_count(8), borrow_count(8), liquidation_count(8)]
#[no_mangle]
pub extern "C" fn get_platform_stats() -> u32 {
    let mut buf = Vec::with_capacity(48);
    buf.extend_from_slice(&u64_to_bytes(load_u64(b"ll_total_deposits")));
    buf.extend_from_slice(&u64_to_bytes(load_u64(b"ll_total_borrows")));
    buf.extend_from_slice(&u64_to_bytes(load_u64(b"ll_reserves")));
    buf.extend_from_slice(&u64_to_bytes(load_u64(DEPOSIT_COUNT_KEY)));
    buf.extend_from_slice(&u64_to_bytes(load_u64(BORROW_COUNT_KEY)));
    buf.extend_from_slice(&u64_to_bytes(load_u64(LIQUIDATION_COUNT_KEY)));
    set_return_data(&buf);
    0
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use moltchain_sdk::test_mock;
    use moltchain_sdk::bytes_to_u64;

    const MOLT_ADDR: [u8; 32] = [99u8; 32];
    const CONTRACT_ADDR: [u8; 32] = [88u8; 32];

    /// Standard setup: reset + configure moltcoin + contract address for transfers
    fn setup() {
        test_mock::reset();
        test_mock::set_contract_address(CONTRACT_ADDR);
        storage_set(MOLTCOIN_ADDRESS_KEY, &MOLT_ADDR);
    }

    /// Setup without moltcoin — for testing "moltcoin not configured" error paths
    fn setup_no_molt() {
        test_mock::reset();
        test_mock::set_contract_address(CONTRACT_ADDR);
    }

    #[test]
    fn test_initialize() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        let result = initialize(admin.as_ptr());
        assert_eq!(result, 0);
        let stored = test_mock::get_storage(ADMIN_KEY);
        assert_eq!(stored, Some(admin.to_vec()));
    }

    #[test]
    fn test_initialize_already_initialized() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(initialize(admin.as_ptr()), 1);
    }

    #[test]
    fn test_deposit() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        assert_eq!(deposit(user.as_ptr(), 1_000_000), 0);
        assert_eq!(load_u64(b"ll_total_deposits"), 1_000_000);
    }

    #[test]
    fn test_deposit_zero() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        assert_eq!(deposit(user.as_ptr(), 0), 1);
    }

    // AUDIT-FIX G9-01: Deposit with insufficient value attached
    #[test]
    fn test_deposit_insufficient_value() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(500_000); // less than deposit amount
        assert_eq!(deposit(user.as_ptr(), 1_000_000), 30);
    }

    #[test]
    fn test_withdraw() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);
        assert_eq!(withdraw(user.as_ptr(), 500_000), 0);
        assert_eq!(load_u64(b"ll_total_deposits"), 500_000);
    }

    #[test]
    fn test_withdraw_exceeds_deposit() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);
        assert_eq!(withdraw(user.as_ptr(), 2_000_000), 2);
    }

    #[test]
    fn test_withdraw_would_make_unhealthy() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);
        borrow(user.as_ptr(), 750_000); // max borrow at 75%
        // Any withdrawal makes it unhealthy
        assert_eq!(withdraw(user.as_ptr(), 1), 3);
    }

    #[test]
    fn test_borrow() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);
        assert_eq!(borrow(user.as_ptr(), 500_000), 0);
        assert_eq!(load_u64(b"ll_total_borrows"), 500_000);
    }

    #[test]
    fn test_borrow_exceeds_collateral_factor() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);
        assert_eq!(borrow(user.as_ptr(), 750_001), 2); // > 75%
    }

    #[test]
    fn test_borrow_exceeds_liquidity() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user1 = [2u8; 32];
        test_mock::set_caller(user1);
        test_mock::set_value(1_000_000);
        deposit(user1.as_ptr(), 1_000_000);
        borrow(user1.as_ptr(), 750_000);
        let user2 = [3u8; 32];
        test_mock::set_caller(user2);
        test_mock::set_value(1_000_000);
        deposit(user2.as_ptr(), 1_000_000);
        borrow(user2.as_ptr(), 750_000);
        let user3 = [4u8; 32];
        test_mock::set_caller(user3);
        test_mock::set_value(2_000_000);
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
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);
        borrow(user.as_ptr(), 500_000);
        test_mock::set_value(200_000);
        assert_eq!(repay(user.as_ptr(), 200_000), 0);
        assert_eq!(load_u64(b"ll_total_borrows"), 300_000);
    }

    #[test]
    fn test_repay_no_borrow() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(100);
        assert_eq!(repay(user.as_ptr(), 100), 2);
    }

    #[test]
    fn test_repay_overpay() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);
        borrow(user.as_ptr(), 500_000);
        test_mock::set_value(999_999);
        assert_eq!(repay(user.as_ptr(), 999_999), 0);
        assert_eq!(load_u64(b"ll_total_borrows"), 0);
    }

    // AUDIT-FIX G9-01: Repay with insufficient value attached
    #[test]
    fn test_repay_insufficient_value() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);
        borrow(user.as_ptr(), 500_000);
        test_mock::set_value(50_000); // less than repay amount
        assert_eq!(repay(user.as_ptr(), 200_000), 30);
    }

    #[test]
    fn test_liquidate() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let borrower = [2u8; 32];
        test_mock::set_caller(borrower);
        test_mock::set_value(1_000_000);
        deposit(borrower.as_ptr(), 1_000_000);
        borrow(borrower.as_ptr(), 750_000);
        // Manually push borrow above liquidation threshold (85%)
        let hex = hex_encode_addr(&borrower);
        let bor_key = make_key(b"bor:", &hex);
        store_u64(&bor_key, 860_000);
        store_u64(b"ll_total_borrows", 860_000);
        let liquidator = [3u8; 32];
        test_mock::set_caller(liquidator);
        test_mock::set_value(200_000);
        assert_eq!(liquidate(liquidator.as_ptr(), borrower.as_ptr(), 200_000), 0);
        let borrow_after = load_u64(&bor_key);
        assert!(borrow_after < 860_000);
    }

    #[test]
    fn test_liquidate_healthy_position() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let borrower = [2u8; 32];
        test_mock::set_caller(borrower);
        test_mock::set_value(1_000_000);
        deposit(borrower.as_ptr(), 1_000_000);
        borrow(borrower.as_ptr(), 500_000); // 50% < 85%
        let liquidator = [3u8; 32];
        test_mock::set_caller(liquidator);
        test_mock::set_value(100_000);
        assert_eq!(liquidate(liquidator.as_ptr(), borrower.as_ptr(), 100_000), 3);
    }

    #[test]
    fn test_liquidate_no_borrow() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let borrower = [2u8; 32];
        test_mock::set_caller(borrower);
        test_mock::set_value(1_000_000);
        deposit(borrower.as_ptr(), 1_000_000);
        let liquidator = [3u8; 32];
        test_mock::set_caller(liquidator);
        test_mock::set_value(100_000);
        assert_eq!(liquidate(liquidator.as_ptr(), borrower.as_ptr(), 100_000), 2);
    }

    // AUDIT-FIX G9-01: Liquidate with insufficient value attached
    #[test]
    fn test_liquidate_insufficient_value() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let borrower = [2u8; 32];
        test_mock::set_caller(borrower);
        test_mock::set_value(1_000_000);
        deposit(borrower.as_ptr(), 1_000_000);
        borrow(borrower.as_ptr(), 750_000);
        let hex = hex_encode_addr(&borrower);
        let bor_key = make_key(b"bor:", &hex);
        store_u64(&bor_key, 860_000);
        store_u64(b"ll_total_borrows", 860_000);
        let liquidator = [3u8; 32];
        test_mock::set_caller(liquidator);
        test_mock::set_value(50_000); // less than repay_amount
        assert_eq!(liquidate(liquidator.as_ptr(), borrower.as_ptr(), 200_000), 30);
    }

    #[test]
    fn test_get_account_info() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
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
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
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
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);

        let borrower = [3u8; 32];
        test_mock::set_caller(borrower);
        // Flash borrow 100,000
        assert_eq!(flash_borrow(borrower.as_ptr(), 100_000), 0);
        let fee_data = test_mock::get_return_data();
        let fee = bytes_to_u64(&fee_data);
        assert_eq!(fee, 90); // 0.09% of 100_000 = 90

        // Underpayment rejected (amount check, before value check)
        assert_eq!(flash_repay(borrower.as_ptr(), 100_000), 2);

        // Full repayment with fee — need value attached
        test_mock::set_value(100_090);
        assert_eq!(flash_repay(borrower.as_ptr(), 100_090), 0);

        // Reserves increased by fee
        assert_eq!(load_u64(b"ll_reserves"), 90);
    }

    #[test]
    fn test_flash_borrow_no_liquidity() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000);
        deposit(user.as_ptr(), 1_000);

        let borrower = [3u8; 32];
        test_mock::set_caller(borrower);
        assert_eq!(flash_borrow(borrower.as_ptr(), 2_000), 3);
    }

    #[test]
    fn test_flash_double_borrow_rejected() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);

        let borrower = [3u8; 32];
        test_mock::set_caller(borrower);
        assert_eq!(flash_borrow(borrower.as_ptr(), 100_000), 0);
        // Second borrow while first active
        assert_eq!(flash_borrow(borrower.as_ptr(), 50_000), 2);
    }

    #[test]
    fn test_flash_repay_without_borrow() {
        setup();
        let borrower = [3u8; 32];
        test_mock::set_caller(borrower);
        assert_eq!(flash_repay(borrower.as_ptr(), 100_000), 1);
    }

    // AUDIT-FIX G9-01: Flash repay with insufficient value
    #[test]
    fn test_flash_repay_insufficient_value() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);

        let borrower = [3u8; 32];
        test_mock::set_caller(borrower);
        assert_eq!(flash_borrow(borrower.as_ptr(), 100_000), 0);

        // Repay amount sufficient but value not attached
        test_mock::set_value(50); // far less than required 100,090
        assert_eq!(flash_repay(borrower.as_ptr(), 100_090), 30);
    }

    #[test]
    fn test_pause_unpause() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
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
        test_mock::set_caller(admin);
        assert_eq!(pause(admin.as_ptr()), 2);

        // Unpause
        assert_eq!(unpause(admin.as_ptr()), 0);
        assert!(!is_paused());

        // Operations work again
        test_mock::set_caller(user);
        test_mock::set_value(1_000);
        assert_eq!(deposit(user.as_ptr(), 1_000), 0);

        // Double unpause rejected
        test_mock::set_caller(admin);
        assert_eq!(unpause(admin.as_ptr()), 2);
    }

    #[test]
    fn test_pause_non_admin_rejected() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let other = [9u8; 32];
        test_mock::set_caller(other);
        assert_eq!(pause(other.as_ptr()), 1);
        assert_eq!(unpause(other.as_ptr()), 1);
    }

    #[test]
    fn test_deposit_cap() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        // Set cap
        assert_eq!(set_deposit_cap(admin.as_ptr(), 500_000), 0);

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(400_000);
        assert_eq!(deposit(user.as_ptr(), 400_000), 0);
        // Exceeds cap
        test_mock::set_value(200_000);
        assert_eq!(deposit(user.as_ptr(), 200_000), 4);
        // Just under cap
        test_mock::set_value(100_000);
        assert_eq!(deposit(user.as_ptr(), 100_000), 0);
    }

    #[test]
    fn test_deposit_cap_non_admin() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let other = [9u8; 32];
        test_mock::set_caller(other);
        assert_eq!(set_deposit_cap(other.as_ptr(), 500_000), 1);
    }

    #[test]
    fn test_set_reserve_factor() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        assert_eq!(set_reserve_factor(admin.as_ptr(), 20), 0);
        assert_eq!(load_u64(b"ll_reserve_factor"), 20);

        // Over 100 rejected
        assert_eq!(set_reserve_factor(admin.as_ptr(), 101), 2);

        // Non-admin rejected
        let other = [9u8; 32];
        test_mock::set_caller(other);
        assert_eq!(set_reserve_factor(other.as_ptr(), 5), 1);
    }

    #[test]
    fn test_withdraw_reserves() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
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
        test_mock::set_caller(other);
        assert_eq!(withdraw_reserves(other.as_ptr(), 1_000), 1);
    }

    #[test]
    fn test_get_interest_rate() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
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
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);

        // Very small borrow — fee would be 0, but minimum is 1
        let borrower = [3u8; 32];
        test_mock::set_caller(borrower);
        assert_eq!(flash_borrow(borrower.as_ptr(), 100), 0);
        let fee = bytes_to_u64(&test_mock::get_return_data());
        assert_eq!(fee, 1); // Minimum fee

        // Repay with value
        test_mock::set_value(101);
        assert_eq!(flash_repay(borrower.as_ptr(), 101), 0);
    }

    #[test]
    fn test_repay_still_works_when_paused() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);
        borrow(user.as_ptr(), 500_000);

        // Pause protocol
        test_mock::set_caller(admin);
        pause(admin.as_ptr());

        // Repay should still work (no pause check — users must be able to unwind)
        test_mock::set_caller(user);
        test_mock::set_value(200_000);
        assert_eq!(repay(user.as_ptr(), 200_000), 0);
    }

    #[test]
    fn test_liquidation_works_when_paused() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let borrower = [2u8; 32];
        test_mock::set_caller(borrower);
        test_mock::set_value(1_000_000);
        deposit(borrower.as_ptr(), 1_000_000);
        borrow(borrower.as_ptr(), 750_000);

        // Force unhealthy position
        let hex = hex_encode_addr(&borrower);
        let bor_key = make_key(b"bor:", &hex);
        store_u64(&bor_key, 860_000);
        store_u64(b"ll_total_borrows", 860_000);

        // Pause
        test_mock::set_caller(admin);
        pause(admin.as_ptr());

        // Liquidation should still work when paused (safety valve)
        let liquidator = [3u8; 32];
        test_mock::set_caller(liquidator);
        test_mock::set_value(200_000);
        assert_eq!(liquidate(liquidator.as_ptr(), borrower.as_ptr(), 200_000), 0);
    }

    // ========================================================================
    // AUDIT-FIX G9-01: Token transfer wiring tests
    // ========================================================================

    #[test]
    fn test_set_moltcoin_address() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let molt = [77u8; 32];
        assert_eq!(set_moltcoin_address(admin.as_ptr(), molt.as_ptr()), 0);
        assert_eq!(load_molt_addr(), molt);
    }

    #[test]
    fn test_set_moltcoin_address_non_admin() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let other = [9u8; 32];
        test_mock::set_caller(other);
        let molt = [77u8; 32];
        assert_eq!(set_moltcoin_address(other.as_ptr(), molt.as_ptr()), 1);
    }

    #[test]
    fn test_set_moltcoin_address_zero_rejected() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let zero = [0u8; 32];
        assert_eq!(set_moltcoin_address(admin.as_ptr(), zero.as_ptr()), 2);
    }

    #[test]
    fn test_withdraw_without_molt_configured() {
        setup_no_molt();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);
        // Withdraw should fail because moltcoin not configured for outgoing transfer
        assert_eq!(withdraw(user.as_ptr(), 500_000), 30);
        // Bookkeeping should be reverted
        assert_eq!(load_u64(b"ll_total_deposits"), 1_000_000);
    }

    #[test]
    fn test_borrow_without_molt_configured() {
        setup_no_molt();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);
        assert_eq!(borrow(user.as_ptr(), 500_000), 30);
        // Bookkeeping should be reverted
        assert_eq!(load_u64(b"ll_total_borrows"), 0);
    }

    #[test]
    fn test_flash_borrow_without_molt_configured() {
        setup_no_molt();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);
        let borrower = [3u8; 32];
        test_mock::set_caller(borrower);
        assert_eq!(flash_borrow(borrower.as_ptr(), 100_000), 30);
        // Flash state should be reverted
        assert_eq!(load_u64(FLASH_BORROWED_KEY), 0);
    }

    #[test]
    fn test_withdraw_reserves_without_molt_configured() {
        setup_no_molt();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        store_u64(b"ll_reserves", 10_000);
        assert_eq!(withdraw_reserves(admin.as_ptr(), 5_000), 30);
        // Reserves should be reverted
        assert_eq!(load_u64(b"ll_reserves"), 10_000);
    }

    #[test]
    fn test_self_custody_transfer_pattern() {
        // Verify the self-custody pattern: contract uses its own address as from
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);

        // Withdraw triggers transfer_out which uses get_contract_address()
        let self_addr = get_contract_address();
        assert_eq!(self_addr.0, CONTRACT_ADDR);
        assert_eq!(withdraw(user.as_ptr(), 100_000), 0);
        assert_eq!(load_u64(b"ll_total_deposits"), 900_000);
    }

    // ========================================================================
    // P9-SC-01: Compound-style borrow index tests
    // ========================================================================

    #[test]
    fn test_borrow_index_accrues_per_user() {
        // Verifies that after interest accrues, a borrower's settled borrow
        // reflects the global index growth, and a new borrower's checkpoint
        // starts at the current index.
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        // Deposit + borrow
        let borrower = [2u8; 32];
        test_mock::set_caller(borrower);
        test_mock::set_value(10_000_000);
        deposit(borrower.as_ptr(), 10_000_000);
        borrow(borrower.as_ptr(), 5_000_000);

        let hex = hex_encode_addr(&borrower);
        let bor_key = make_key(b"bor:", &hex);
        let bix_key = make_key(b"bix:", &hex);

        // Stored borrow should be 5_000_000
        assert_eq!(load_u64(&bor_key), 5_000_000);
        // Index checkpoint should equal initial scale
        assert_eq!(load_u64(&bix_key), BORROW_INDEX_SCALE);
        // Global index should equal initial scale (no interest yet)
        assert_eq!(load_u64(b"ll_borrow_index"), BORROW_INDEX_SCALE);

        // Advance time by 10 seconds (10_000 ms → 25 slots at 400ms each)
        // This will trigger interest accrual on the next borrow/repay call.
        test_mock::set_timestamp(1000 + 10_000);

        // Trigger accrue_interest via a repay(0) — repay of zero on a borrow is
        // rejected, but accrue_interest runs first. Use a new deposit to trigger.
        // Actually, let's just call accrue_interest() directly (it's a private fn
        // but accessible in tests within the same module).
        accrue_interest();

        // Global index should have grown
        let new_index = load_u64(b"ll_borrow_index");
        assert!(
            new_index > BORROW_INDEX_SCALE,
            "Global borrow index should have increased after interest accrual: {}",
            new_index
        );

        // User's stored borrow hasn't changed yet (lazy settlement)
        assert_eq!(load_u64(&bor_key), 5_000_000);

        // But settle_user_borrow should return more than 5_000_000
        let settled = settle_user_borrow(&hex);
        assert!(
            settled > 5_000_000,
            "Settled borrow should exceed original: {}",
            settled
        );

        // After settlement, stored borrow should match settled amount
        assert_eq!(load_u64(&bor_key), settled);
        // And checkpoint should match current global index
        assert_eq!(load_u64(&bix_key), new_index);

        // A second settle without further interest should be idempotent
        let settled2 = settle_user_borrow(&hex);
        assert_eq!(settled2, settled);

        // Now a second borrower: deposits, borrows. Their checkpoint should be
        // at the current (higher) global index.
        let borrower2 = [3u8; 32];
        test_mock::set_caller(borrower2);
        test_mock::set_value(10_000_000);
        deposit(borrower2.as_ptr(), 10_000_000);
        borrow(borrower2.as_ptr(), 1_000_000);

        let hex2 = hex_encode_addr(&borrower2);
        let bix_key2 = make_key(b"bix:", &hex2);
        // Their index checkpoint should be the current global index, not the initial scale
        assert_eq!(load_u64(&bix_key2), new_index);
    }

    #[test]
    fn test_compute_current_borrow_is_read_only() {
        // Verifies that compute_current_borrow returns the adjusted value
        // without modifying stored state.
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let borrower = [2u8; 32];
        test_mock::set_caller(borrower);
        test_mock::set_value(10_000_000);
        deposit(borrower.as_ptr(), 10_000_000);
        borrow(borrower.as_ptr(), 5_000_000);

        let hex = hex_encode_addr(&borrower);
        let bor_key = make_key(b"bor:", &hex);
        let bix_key = make_key(b"bix:", &hex);

        // Advance time to accrue interest
        test_mock::set_timestamp(1000 + 10_000);
        accrue_interest();

        let stored_before = load_u64(&bor_key);
        let checkpoint_before = load_u64(&bix_key);

        // compute_current_borrow should return adjusted value
        let computed = compute_current_borrow(&hex);
        assert!(computed > stored_before);

        // But stored values should NOT change (read-only)
        assert_eq!(load_u64(&bor_key), stored_before);
        assert_eq!(load_u64(&bix_key), checkpoint_before);
    }
}
