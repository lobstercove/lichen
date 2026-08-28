// ThallLend v3 - Decentralized Lending Protocol
// Deposit collateral, borrow assets, earn interest
// Per whitepaper: collateralized lending with liquidation mechanics
//
// v2/v3 additions:
//   - Flash loans with fee (0.09%)
//   - Emergency pause (admin)
//   - Reentrancy guard enforcement on all mutating functions
//   - Admin reserve withdrawal
//   - Admin reserve factor updates
//   - Protocol deposit cap
//   - Interest rate query view function
//   - Optional LichenOracle freshness circuit breaker for the LICN market

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;
use alloc::{vec, vec::Vec};
use lichen_sdk::crosscall::{call_contract, encode_layout_args, CrossCall};
use lichen_sdk::{
    balance_of_token_or_native, bytes_to_u64, get_caller, get_contract_address, get_timestamp,
    get_value, is_native_token, log_info, receive_token_or_native, set_return_data, storage_get,
    storage_set, transfer_token_or_native, u64_to_bytes, Address,
};

// Oracle configuration key (stores lichenoracle contract address)
const ORACLE_ADDR_KEY: &[u8] = b"ll_oracle_addr";
const ORACLE_ASSET_KEY: &[u8] = b"ll_oracle_asset";
const MAX_ORACLE_ASSET_KEY_LEN: u32 = 64;

/// Query the configured oracle price feed.
/// Returns 1:1 only when neither oracle configuration key exists. Any partial,
/// malformed, zero-address, empty, or oversized configuration fails closed.
fn try_get_oracle_price() -> Option<u64> {
    let oracle_bytes = storage_get(ORACLE_ADDR_KEY);
    let asset = storage_get(ORACLE_ASSET_KEY);
    if oracle_bytes.is_none() && asset.is_none() {
        return Some(1);
    }

    let (Some(oracle_bytes), Some(asset)) = (oracle_bytes, asset) else {
        log_info("Oracle configuration is incomplete");
        return None;
    };
    if oracle_bytes.len() != 32
        || asset.is_empty()
        || asset.len() > MAX_ORACLE_ASSET_KEY_LEN as usize
    {
        log_info("Oracle configuration is malformed");
        return None;
    }

    let mut oracle_addr = [0u8; 32];
    oracle_addr.copy_from_slice(&oracle_bytes);
    if is_zero_addr(&oracle_addr) {
        log_info("Oracle configuration contains the zero address");
        return None;
    }

    let mut args = Vec::with_capacity(asset.len() + 8);
    args.extend_from_slice(&asset);
    args.extend_from_slice(&(asset.len() as u64).to_le_bytes());
    let call = CrossCall::new(Address(oracle_addr), "get_price_value", args);
    if let Ok(result) = call_contract(call) {
        if result.len() >= 8 {
            let price = bytes_to_u64(&result[..8]);
            if price > 0 {
                return Some(price);
            }
        }
    }
    log_info("Configured oracle query failed");
    None
}

/// Query the configured oracle price feed for legacy view/tests.
#[cfg(test)]
fn get_oracle_price() -> u64 {
    try_get_oracle_price().unwrap_or(1)
}

// T5.12: Reentrancy guard
const REENTRANCY_KEY: &[u8] = b"_reentrancy";

struct ReentrancyGuard {
    previous: Option<Vec<u8>>,
}

impl Drop for ReentrancyGuard {
    fn drop(&mut self) {
        restore_storage_value(REENTRANCY_KEY, &self.previous);
    }
}

fn reentrancy_enter() -> Option<ReentrancyGuard> {
    let previous = storage_get(REENTRANCY_KEY);
    if previous
        .as_ref()
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
    {
        return None;
    }
    storage_set(REENTRANCY_KEY, &[1u8]);
    Some(ReentrancyGuard { previous })
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

/// Base borrow rate: approximately 2% annual at the 400ms target cadence.
/// Contract `get_timestamp()` is the canonical slot number, despite its legacy
/// name. There are 78,894,000 target slots per Julian year, and 254 / 1e12 per
/// slot annualizes to 200 basis points after deterministic integer rounding.
const BASE_RATE_SCALED: u64 = 254;
const RATE_SCALE: u64 = 1_000_000_000_000;
const TARGET_SLOT_MILLIS: u64 = 400;
const MILLIS_PER_JULIAN_YEAR: u64 = 31_557_600_000;
const SLOTS_PER_YEAR: u64 = MILLIS_PER_JULIAN_YEAR / TARGET_SLOT_MILLIS;

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
const MAX_FLASH_CALLBACK_DATA_LEN: u32 = 128;

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

/// AUDIT-FIX G9-01: LichenCoin contract address — required for actual token transfers
const LICHENCOIN_ADDRESS_KEY: &[u8] = b"ll_licn_addr";

/// Compound-style index scale factor.
///
/// Global borrow and deposit indexes start at this value (1e9) and grow with
/// accrued interest. Per-user `bix:HEXADDR` and `dix:HEXADDR` checkpoints make
/// both borrower debt and supplier claims settle lazily without iterating over
/// every account.
const BORROW_INDEX_SCALE: u64 = 1_000_000_000;
const DEPOSIT_INDEX_SCALE: u64 = 1_000_000_000;

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

fn remove_storage_key(key: &[u8]) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        extern "C" {
            fn storage_delete(key_ptr: *const u8, key_len: u32) -> u32;
        }
        unsafe { storage_delete(key.as_ptr(), key.len() as u32) == 1 }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        lichen_sdk::storage::remove(key)
    }
}

fn restore_storage_value(key: &[u8], value: &Option<Vec<u8>>) {
    match value {
        Some(bytes) => {
            storage_set(key, bytes);
        }
        None => {
            remove_storage_key(key);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LendingAccountingSnapshot {
    total_deposits: Option<Vec<u8>>,
    total_borrows: Option<Vec<u8>>,
    reserves: Option<Vec<u8>>,
    last_update: Option<Vec<u8>>,
    borrow_index: Option<Vec<u8>>,
    deposit_index: Option<Vec<u8>>,
}

fn snapshot_lending_accounting() -> LendingAccountingSnapshot {
    LendingAccountingSnapshot {
        total_deposits: storage_get(b"ll_total_deposits"),
        total_borrows: storage_get(b"ll_total_borrows"),
        reserves: storage_get(b"ll_reserves"),
        last_update: storage_get(b"ll_last_update"),
        borrow_index: storage_get(b"ll_borrow_index"),
        deposit_index: storage_get(b"ll_deposit_index"),
    }
}

fn restore_lending_accounting(snapshot: LendingAccountingSnapshot) {
    restore_storage_value(b"ll_total_deposits", &snapshot.total_deposits);
    restore_storage_value(b"ll_total_borrows", &snapshot.total_borrows);
    restore_storage_value(b"ll_reserves", &snapshot.reserves);
    restore_storage_value(b"ll_last_update", &snapshot.last_update);
    restore_storage_value(b"ll_borrow_index", &snapshot.borrow_index);
    restore_storage_value(b"ll_deposit_index", &snapshot.deposit_index);
}

fn is_paused() -> bool {
    storage_get(PAUSE_KEY)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
}

fn is_admin(caller: &[u8]) -> bool {
    match storage_get(ADMIN_KEY) {
        Some(data) => data.as_slice() == caller,
        None => false,
    }
}

/// AUDIT-FIX G9-01: Load configured lichencoin address (returns zero if not set)
fn load_licn_addr() -> [u8; 32] {
    storage_get(LICHENCOIN_ADDRESS_KEY)
        .map(|d| {
            let mut a = [0u8; 32];
            if d.len() == 32 {
                a.copy_from_slice(&d);
            }
            a
        })
        .unwrap_or([0u8; 32])
}

fn licn_address_configured() -> bool {
    storage_get(LICHENCOIN_ADDRESS_KEY)
        .map(|bytes| bytes.len() == 32)
        .unwrap_or(false)
}

fn oracle_feed_configured() -> bool {
    storage_get(ORACLE_ADDR_KEY)
        .map(|bytes| {
            let mut addr = [0u8; 32];
            if bytes.len() == 32 {
                addr.copy_from_slice(&bytes);
            }
            bytes.len() == 32 && !is_zero_addr(&addr)
        })
        .unwrap_or(false)
        && storage_get(ORACLE_ASSET_KEY)
            .map(|asset| !asset.is_empty() && asset.len() <= MAX_ORACLE_ASSET_KEY_LEN as usize)
            .unwrap_or(false)
}

fn oracle_configuration_present() -> bool {
    storage_get(ORACLE_ADDR_KEY).is_some() || storage_get(ORACLE_ASSET_KEY).is_some()
}

fn is_zero_addr(a: &[u8; 32]) -> bool {
    a.iter().all(|&b| b == 0)
}

fn u128_to_u64_saturating(value: u128) -> u64 {
    if value > u64::MAX as u128 {
        u64::MAX
    } else {
        value as u64
    }
}

fn utilization_percent(total_deposits: u64, total_borrows: u64) -> u64 {
    if total_deposits == 0 {
        return 0;
    }
    u128_to_u64_saturating((total_borrows as u128) * 100 / (total_deposits as u128))
}

fn collateral_limit(amount: u64, percent: u64) -> u64 {
    u128_to_u64_saturating((amount as u128) * (percent as u128) / 100)
}

fn liquidation_collateral(repay_amount: u64) -> Option<u64> {
    let seized =
        repay_amount as u128 + repay_amount as u128 * LIQUIDATION_BONUS_PERCENT as u128 / 100;
    u64::try_from(seized).ok()
}

/// Largest repayment whose principal plus liquidation bonus fits in the
/// borrower's remaining collateral. Binary search handles small rounding cases
/// exactly and avoids an approximation that can strand sub-bonus dust.
fn max_repay_for_collateral(collateral: u64) -> u64 {
    let mut low = 0u64;
    let mut high = collateral;
    while low < high {
        let mid = low + (high - low) / 2 + 1;
        match liquidation_collateral(mid) {
            Some(seized) if seized <= collateral => low = mid,
            _ => high = mid - 1,
        }
    }
    low
}

fn flash_loan_fee(amount: u64) -> u64 {
    let rounded = u128_to_u64_saturating(
        (amount as u128 * FLASH_LOAN_FEE_BPS as u128).div_ceil(BPS_SCALE as u128),
    );
    rounded.max(1)
}

fn current_rate_per_slot(total_deposits: u64, total_borrows: u64) -> u64 {
    let utilization = utilization_percent(total_deposits, total_borrows).min(100);

    let rate_per_slot = if utilization <= UTILIZATION_KINK_PERCENT {
        (BASE_RATE_SCALED as u128) + ((utilization as u128) * (BASE_RATE_SCALED as u128) * 2 / 100)
    } else {
        let base_at_kink = (BASE_RATE_SCALED as u128)
            + (UTILIZATION_KINK_PERCENT as u128 * BASE_RATE_SCALED as u128 * 2 / 100);
        let excess = utilization - UTILIZATION_KINK_PERCENT;
        base_at_kink + ((excess as u128) * (BASE_RATE_SCALED as u128) * 10 / 100)
    };

    if rate_per_slot > MAX_RATE_PER_SLOT as u128 {
        MAX_RATE_PER_SLOT
    } else {
        rate_per_slot as u64
    }
}

fn annual_rate_bps(rate_per_slot: u64) -> u64 {
    u128_to_u64_saturating(
        rate_per_slot as u128 * SLOTS_PER_YEAR as u128 * BPS_SCALE as u128 / RATE_SCALE as u128,
    )
}

fn quote_accrued_interest(principal: u64, elapsed_slots: u64) -> u64 {
    if principal == 0 || elapsed_slots == 0 {
        return 0;
    }

    let total_deposits = load_u64(b"ll_total_deposits");
    let total_borrows = load_u64(b"ll_total_borrows");
    if total_deposits == 0 || total_borrows == 0 {
        return 0;
    }

    let rate_per_slot = current_rate_per_slot(total_deposits, total_borrows);
    u128_to_u64_saturating(
        (principal as u128) * (rate_per_slot as u128) * (elapsed_slots as u128)
            / (RATE_SCALE as u128),
    )
}

/// Transfer tokens OUT from the contract's own balance to a recipient.
/// Uses the self-custody pattern: caller==from in CCC context.
/// Returns 0 on success, non-zero on failure.
fn transfer_out(recipient: &[u8; 32], amount: u64) -> u32 {
    let licn_addr = load_licn_addr();
    if !licn_address_configured() {
        log_info("Lichencoin address not configured");
        return 30;
    }
    let self_addr = get_contract_address();
    match transfer_token_or_native(Address(licn_addr), self_addr, Address(*recipient), amount) {
        Ok(true) => 0,
        Ok(false) => {
            log_info("Token transfer returned failure status");
            31
        }
        Err(_) => {
            log_info("Token transfer failed");
            31
        }
    }
}

fn receive_licn_in(payer: &[u8; 32], amount: u64) -> bool {
    if !licn_address_configured() {
        log_info("Lichencoin address not configured");
        return false;
    }

    let token = Address(load_licn_addr());
    if !is_native_token(&token) {
        return receive_token_or_native(token, Address(*payer), get_contract_address(), amount)
            .unwrap_or(false);
    }

    let received = get_value();
    if received < amount {
        return false;
    }
    let excess = received - amount;
    if excess == 0 {
        return true;
    }

    // Native payable value is credited before execution. Return every unused
    // spore in the same atomic call instead of silently converting a caller's
    // maximum repayment into protocol surplus.
    transfer_token_or_native(token, get_contract_address(), Address(*payer), excess)
        .unwrap_or(false)
}

fn get_deposit_cap() -> u64 {
    load_u64(DEPOSIT_CAP_KEY)
}

fn current_deposit_index() -> u64 {
    let index = load_u64(b"ll_deposit_index");
    if index == 0 {
        DEPOSIT_INDEX_SCALE
    } else {
        index
    }
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
    let actual_borrow = u128_to_u64_saturating(
        stored_borrow as u128 * global_index as u128 / effective_user_index as u128,
    );

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

    u128_to_u64_saturating(
        stored_borrow as u128 * global_index as u128 / effective_user_index as u128,
    )
}

/// Settle a supplier's balance using the global deposit index.
fn settle_user_deposit(hex: &[u8; 64]) -> u64 {
    let global_index = current_deposit_index();

    let deposit_key = make_key(b"dep:", hex);
    let stored_deposit = load_u64(&deposit_key);
    if stored_deposit == 0 {
        return 0;
    }

    let index_key = make_key(b"dix:", hex);
    let user_index = load_u64(&index_key);
    let effective_user_index = if user_index == 0 {
        DEPOSIT_INDEX_SCALE
    } else {
        user_index
    };
    if global_index == effective_user_index {
        return stored_deposit;
    }

    let actual_deposit = u128_to_u64_saturating(
        stored_deposit as u128 * global_index as u128 / effective_user_index as u128,
    );
    store_u64(&deposit_key, actual_deposit);
    store_u64(&index_key, global_index);
    actual_deposit
}

/// Compute a supplier's current balance without mutating its checkpoint.
fn compute_current_deposit(hex: &[u8; 64]) -> u64 {
    let global_index = current_deposit_index();

    let stored_deposit = load_u64(&make_key(b"dep:", hex));
    if stored_deposit == 0 {
        return 0;
    }

    let user_index = load_u64(&make_key(b"dix:", hex));
    let effective_user_index = if user_index == 0 {
        DEPOSIT_INDEX_SCALE
    } else {
        user_index
    };
    u128_to_u64_saturating(
        stored_deposit as u128 * global_index as u128 / effective_user_index as u128,
    )
}

// ============================================================================
// PROTOCOL STATE
// ============================================================================

/// Initialize the lending protocol
#[no_mangle]
pub extern "C" fn initialize(admin_ptr: *const u8) -> u32 {
    let mut admin = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(admin_ptr, admin.as_mut_ptr(), 32);
    }

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
    store_u64(b"ll_deposit_index", DEPOSIT_INDEX_SCALE);

    log_info("ThallLend initialized");
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
    let _reentrancy_guard = match reentrancy_enter() {
        Some(guard) => guard,
        None => {
            log_info("Reentrancy detected");
            return 21;
        }
    };

    let mut depositor = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(depositor_ptr, depositor.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != depositor {
        return 200;
    }

    // AUDIT-FIX G9-01: Verify incoming custody covers deposit
    if !receive_licn_in(&depositor, amount) {
        log_info("Insufficient deposit payment");
        return 30;
    }

    let hex = hex_encode_addr(&depositor);

    accrue_interest();

    // Check deposit cap
    let cap = get_deposit_cap();
    let total = load_u64(b"ll_total_deposits");
    let new_total = match total.checked_add(amount) {
        Some(v) => v,
        None => {
            log_info("Total deposits overflow");
            return 5;
        }
    };
    if cap > 0 && new_total > cap {
        log_info("Would exceed deposit cap");
        return 4;
    }

    // Update user deposit
    let dep_key = make_key(b"dep:", &hex);
    let dix_key = make_key(b"dix:", &hex);
    let prev_deposit = settle_user_deposit(&hex);
    let new_deposit = match prev_deposit.checked_add(amount) {
        Some(v) => v,
        None => {
            log_info("User deposit overflow");
            return 5;
        }
    };
    store_u64(&dep_key, new_deposit);
    store_u64(&dix_key, current_deposit_index());

    // Update total deposits
    store_u64(b"ll_total_deposits", new_total);

    // Track deposit count
    store_u64(
        DEPOSIT_COUNT_KEY,
        load_u64(DEPOSIT_COUNT_KEY).saturating_add(1),
    );
    log_info("Deposit successful");
    0
}

/// Withdraw collateral (only if health factor remains > 1)
#[no_mangle]
pub extern "C" fn withdraw(depositor_ptr: *const u8, amount: u64) -> u32 {
    if amount == 0 {
        return 1;
    }
    let _reentrancy_guard = match reentrancy_enter() {
        Some(guard) => guard,
        None => return 21,
    };

    let mut depositor = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(depositor_ptr, depositor.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != depositor {
        return 200;
    }

    let hex = hex_encode_addr(&depositor);
    let dep_key = make_key(b"dep:", &hex);
    let dix_key = make_key(b"dix:", &hex);
    let deposit_before = storage_get(&dep_key);
    let dix_before = storage_get(&dix_key);
    let accounting_before = snapshot_lending_accounting();

    accrue_interest();

    let current_deposit = settle_user_deposit(&hex);
    if amount > current_deposit {
        restore_storage_value(&dep_key, &deposit_before);
        restore_storage_value(&dix_key, &dix_before);
        restore_lending_accounting(accounting_before);
        log_info("Insufficient deposit balance");
        return 2;
    }

    let total = load_u64(b"ll_total_deposits");
    let total_borrows = load_u64(b"ll_total_borrows");
    if amount > total.saturating_sub(total_borrows) {
        restore_storage_value(&dep_key, &deposit_before);
        restore_storage_value(&dix_key, &dix_before);
        restore_lending_accounting(accounting_before);
        log_info("Insufficient available pool liquidity");
        return 4;
    }

    // Check health factor after withdrawal
    // P9-SC-01: Use index-adjusted borrow for accurate health check
    let current_borrow = compute_current_borrow(&hex);
    let new_deposit = current_deposit - amount;

    if current_borrow > 0 {
        if try_get_oracle_price().is_none() {
            restore_storage_value(&dep_key, &deposit_before);
            restore_storage_value(&dix_key, &dix_before);
            restore_lending_accounting(accounting_before);
            log_info("Oracle price unavailable");
            return 6;
        }
        let max_borrow = collateral_limit(new_deposit, COLLATERAL_FACTOR_PERCENT);
        if current_borrow > max_borrow {
            restore_storage_value(&dep_key, &deposit_before);
            restore_storage_value(&dix_key, &dix_before);
            restore_lending_accounting(accounting_before);
            log_info("Withdrawal would make position unhealthy");
            return 3;
        }
    }

    store_u64(&dep_key, new_deposit);
    store_u64(&dix_key, current_deposit_index());
    let Some(new_total_deposits) = total.checked_sub(amount) else {
        restore_storage_value(&dep_key, &deposit_before);
        restore_storage_value(&dix_key, &dix_before);
        restore_lending_accounting(accounting_before);
        log_info("Deposit liability accounting underflow");
        return 5;
    };
    store_u64(b"ll_total_deposits", new_total_deposits);

    // AUDIT-FIX G9-01: Transfer tokens to withdrawer
    let rc = transfer_out(&depositor, amount);
    if rc != 0 {
        // Revert bookkeeping on transfer failure
        restore_storage_value(&dep_key, &deposit_before);
        restore_storage_value(&dix_key, &dix_before);
        restore_lending_accounting(accounting_before);
        return rc;
    }
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
    let _reentrancy_guard = match reentrancy_enter() {
        Some(guard) => guard,
        None => return 21,
    };

    let mut borrower = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(borrower_ptr, borrower.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != borrower {
        return 200;
    }

    let hex = hex_encode_addr(&borrower);
    let borrow_key = make_key(b"bor:", &hex);
    let bix_key = make_key(b"bix:", &hex);
    let ts_key = make_key(b"bts:", &hex);
    let accounting_before = snapshot_lending_accounting();
    let borrow_before = storage_get(&borrow_key);
    let bix_before = storage_get(&bix_key);
    let ts_before = storage_get(&ts_key);
    let borrow_count_before = storage_get(BORROW_COUNT_KEY);

    accrue_interest();

    let deposit_val = compute_current_deposit(&hex);
    // P9-SC-01: Settle existing borrow via index before adding new amount
    let current_borrow = settle_user_borrow(&hex);

    // AUDIT-FIX CON-10/C-3: Use the configured oracle feed, not borrower bytes.
    if try_get_oracle_price().is_none() {
        restore_storage_value(&borrow_key, &borrow_before);
        restore_storage_value(&bix_key, &bix_before);
        restore_storage_value(&ts_key, &ts_before);
        restore_lending_accounting(accounting_before);
        log_info("Oracle price unavailable");
        return 6;
    }
    // ThallLend currently lends and escrows the same LICN asset. Its USD price
    // cancels from both sides of the health ratio; multiplying by an 8-decimal
    // quote would fabricate borrowing power. A configured oracle is therefore
    // a freshness/market-health circuit breaker, while solvency remains LICN
    // amount against LICN amount.
    let max_borrow = collateral_limit(deposit_val, COLLATERAL_FACTOR_PERCENT);
    let new_borrow = match current_borrow.checked_add(amount) {
        Some(v) => v,
        None => {
            restore_storage_value(&borrow_key, &borrow_before);
            restore_storage_value(&bix_key, &bix_before);
            restore_storage_value(&ts_key, &ts_before);
            restore_lending_accounting(accounting_before);
            log_info("Borrow amount overflow");
            return 5;
        }
    };

    if new_borrow > max_borrow {
        restore_storage_value(&borrow_key, &borrow_before);
        restore_storage_value(&bix_key, &bix_before);
        restore_storage_value(&ts_key, &ts_before);
        restore_lending_accounting(accounting_before);
        log_info("Borrow exceeds collateral factor");
        return 2;
    }

    // Check pool liquidity
    let total_deposits = load_u64(b"ll_total_deposits");
    let total_borrows = load_u64(b"ll_total_borrows");
    let available = total_deposits.saturating_sub(total_borrows);
    if amount > available {
        restore_storage_value(&borrow_key, &borrow_before);
        restore_storage_value(&bix_key, &bix_before);
        restore_storage_value(&ts_key, &ts_before);
        restore_lending_accounting(accounting_before);
        log_info("Insufficient pool liquidity");
        return 3;
    }

    store_u64(&borrow_key, new_borrow);
    // P9-SC-01: Always checkpoint the borrow index (settle_user_borrow skips
    // when stored_borrow==0, so first-time borrowers need this)
    store_u64(&bix_key, load_u64(b"ll_borrow_index"));
    let new_total_borrows = match total_borrows.checked_add(amount) {
        Some(v) => v,
        None => {
            restore_storage_value(&borrow_key, &borrow_before);
            restore_storage_value(&bix_key, &bix_before);
            restore_storage_value(&ts_key, &ts_before);
            restore_storage_value(BORROW_COUNT_KEY, &borrow_count_before);
            restore_lending_accounting(accounting_before);
            log_info("Total borrows overflow");
            return 5;
        }
    };
    store_u64(b"ll_total_borrows", new_total_borrows);

    // Track borrow count
    store_u64(
        BORROW_COUNT_KEY,
        load_u64(BORROW_COUNT_KEY).saturating_add(1),
    );

    // Track borrow timestamp for interest calculation
    store_u64(&ts_key, get_timestamp());

    // AUDIT-FIX G9-01: Transfer borrowed tokens to borrower
    let rc = transfer_out(&borrower, amount);
    if rc != 0 {
        // Revert bookkeeping on transfer failure
        restore_storage_value(&borrow_key, &borrow_before);
        restore_storage_value(&bix_key, &bix_before);
        restore_storage_value(&ts_key, &ts_before);
        restore_storage_value(BORROW_COUNT_KEY, &borrow_count_before);
        restore_lending_accounting(accounting_before);
        return rc;
    }
    log_info("Borrow successful");
    0
}

/// Repay borrowed amount
#[no_mangle]
pub extern "C" fn repay(borrower_ptr: *const u8, amount: u64) -> u32 {
    if amount == 0 {
        return 1;
    }
    let _reentrancy_guard = match reentrancy_enter() {
        Some(guard) => guard,
        None => return 21,
    };

    let mut borrower = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(borrower_ptr, borrower.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != borrower {
        return 200;
    }

    let hex = hex_encode_addr(&borrower);
    let borrow_key = make_key(b"bor:", &hex);
    let bix_key = make_key(b"bix:", &hex);
    let accounting_before = snapshot_lending_accounting();
    let borrow_before = storage_get(&borrow_key);
    let bix_before = storage_get(&bix_key);

    accrue_interest();

    // P9-SC-01: Settle borrow via index to get true amount owed
    let current_borrow = settle_user_borrow(&hex);

    if current_borrow == 0 {
        restore_storage_value(&borrow_key, &borrow_before);
        restore_storage_value(&bix_key, &bix_before);
        restore_lending_accounting(accounting_before);
        log_info("No outstanding borrow");
        return 2;
    }

    let repay_amount = if amount > current_borrow {
        current_borrow
    } else {
        amount
    };

    // Pull only the debt actually retired. In native LICN mode the payable
    // maximum is refunded down to this exact amount by receive_licn_in.
    if !receive_licn_in(&borrower, repay_amount) {
        restore_storage_value(&borrow_key, &borrow_before);
        restore_storage_value(&bix_key, &bix_before);
        restore_lending_accounting(accounting_before);
        log_info("Insufficient repayment payment or refund failed");
        return 30;
    }

    store_u64(&borrow_key, current_borrow - repay_amount);

    let total_borrows = load_u64(b"ll_total_borrows");
    let Some(new_total_borrows) = total_borrows.checked_sub(repay_amount) else {
        restore_storage_value(&borrow_key, &borrow_before);
        restore_storage_value(&bix_key, &bix_before);
        restore_lending_accounting(accounting_before);
        log_info("Borrow liability accounting underflow");
        return 5;
    };
    store_u64(b"ll_total_borrows", new_total_borrows);

    // Track repay count
    store_u64(REPAY_COUNT_KEY, load_u64(REPAY_COUNT_KEY).saturating_add(1));
    set_return_data(&u64_to_bytes(repay_amount));
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
    let _reentrancy_guard = match reentrancy_enter() {
        Some(guard) => guard,
        None => return 21,
    };

    let mut liquidator = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(liquidator_ptr, liquidator.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != liquidator {
        return 200;
    }

    let mut borrower = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(borrower_ptr, borrower.as_mut_ptr(), 32);
    }
    let hex = hex_encode_addr(&borrower);
    let dep_key = make_key(b"dep:", &hex);
    let dix_key = make_key(b"dix:", &hex);
    let borrow_key = make_key(b"bor:", &hex);
    let bix_key = make_key(b"bix:", &hex);
    let accounting_before = snapshot_lending_accounting();
    let deposit_before = storage_get(&dep_key);
    let dix_before = storage_get(&dix_key);
    let borrow_before = storage_get(&borrow_key);
    let bix_before = storage_get(&bix_key);
    let liquidation_count_before = storage_get(LIQUIDATION_COUNT_KEY);

    accrue_interest();

    let deposit = settle_user_deposit(&hex);
    // P9-SC-01: Settle borrow via index to check true health
    let current_borrow = settle_user_borrow(&hex);

    if current_borrow == 0 {
        restore_storage_value(&dep_key, &deposit_before);
        restore_storage_value(&dix_key, &dix_before);
        restore_storage_value(&borrow_key, &borrow_before);
        restore_storage_value(&bix_key, &bix_before);
        restore_lending_accounting(accounting_before);
        log_info("No borrow to liquidate");
        return 2;
    }

    // Check if position is liquidatable
    // AUDIT-FIX CON-10/C-3: Use the configured oracle feed, not borrower bytes.
    if try_get_oracle_price().is_none() {
        restore_storage_value(&dep_key, &deposit_before);
        restore_storage_value(&dix_key, &dix_before);
        restore_storage_value(&borrow_key, &borrow_before);
        restore_storage_value(&bix_key, &bix_before);
        restore_lending_accounting(accounting_before);
        log_info("Oracle price unavailable");
        return 6;
    }
    let liquidation_limit = collateral_limit(deposit, LIQUIDATION_THRESHOLD_PERCENT);
    if current_borrow <= liquidation_limit {
        restore_storage_value(&dep_key, &deposit_before);
        restore_storage_value(&dix_key, &dix_before);
        restore_storage_value(&borrow_key, &borrow_before);
        restore_storage_value(&bix_key, &bix_before);
        restore_lending_accounting(accounting_before);
        log_info("Position is healthy, cannot liquidate");
        return 3;
    }

    // Close at most 50% of debt per call, while never accepting more repayment
    // than the remaining collateral can compensate at the configured bonus.
    // Ceiling division keeps a one-spore debt liquidatable.
    let close_factor_limit = current_borrow / 2 + current_borrow % 2;
    let collateral_repay_limit = max_repay_for_collateral(deposit);
    let actual_repay = repay_amount
        .min(close_factor_limit)
        .min(collateral_repay_limit);
    if actual_repay == 0 {
        restore_storage_value(&dep_key, &deposit_before);
        restore_storage_value(&dix_key, &dix_before);
        restore_storage_value(&borrow_key, &borrow_before);
        restore_storage_value(&bix_key, &bix_before);
        restore_lending_accounting(accounting_before);
        log_info("No collateral-backed liquidation repayment available");
        return 7;
    }

    // Pull only the amount actually used. Native payable excess is refunded
    // atomically; MT-20 mode calls transfer_from for this exact amount only.
    if !receive_licn_in(&liquidator, actual_repay) {
        restore_storage_value(&dep_key, &deposit_before);
        restore_storage_value(&dix_key, &dix_before);
        restore_storage_value(&borrow_key, &borrow_before);
        restore_storage_value(&bix_key, &bix_before);
        restore_lending_accounting(accounting_before);
        log_info("Insufficient liquidation payment or refund failed");
        return 30;
    }

    // Collateral seized = repay_amount * (1 + bonus)
    let Some(actual_seized) = liquidation_collateral(actual_repay) else {
        restore_storage_value(&dep_key, &deposit_before);
        restore_storage_value(&dix_key, &dix_before);
        restore_storage_value(&borrow_key, &borrow_before);
        restore_storage_value(&bix_key, &bix_before);
        restore_lending_accounting(accounting_before);
        log_info("Liquidation collateral calculation overflow");
        return 8;
    };

    // Update borrower
    store_u64(&borrow_key, current_borrow - actual_repay);
    store_u64(&dep_key, deposit - actual_seized);
    store_u64(&dix_key, current_deposit_index());

    // Update totals
    let total_borrows = load_u64(b"ll_total_borrows");
    let total_deposits = load_u64(b"ll_total_deposits");
    let (Some(new_total_borrows), Some(new_total_deposits)) = (
        total_borrows.checked_sub(actual_repay),
        total_deposits.checked_sub(actual_seized),
    ) else {
        restore_storage_value(&dep_key, &deposit_before);
        restore_storage_value(&dix_key, &dix_before);
        restore_storage_value(&borrow_key, &borrow_before);
        restore_storage_value(&bix_key, &bix_before);
        restore_lending_accounting(accounting_before);
        log_info("Liquidation accounting underflow");
        return 5;
    };
    store_u64(b"ll_total_borrows", new_total_borrows);
    store_u64(b"ll_total_deposits", new_total_deposits);

    // Track liquidation count
    store_u64(
        LIQUIDATION_COUNT_KEY,
        load_u64(LIQUIDATION_COUNT_KEY).saturating_add(1),
    );

    // AUDIT-FIX G9-01: Transfer seized collateral to liquidator
    let rc = transfer_out(&liquidator, actual_seized);
    if rc != 0 {
        // Revert all bookkeeping on transfer failure
        restore_storage_value(&borrow_key, &borrow_before);
        restore_storage_value(&dep_key, &deposit_before);
        restore_storage_value(&dix_key, &dix_before);
        restore_storage_value(&bix_key, &bix_before);
        restore_storage_value(LIQUIDATION_COUNT_KEY, &liquidation_count_before);
        restore_lending_accounting(accounting_before);
        return rc;
    }
    log_info("Liquidation executed");

    // Preserve the legacy first field (seized collateral) and append the exact
    // repayment consumed so clients can reconcile a capped/refunded request.
    let mut result = Vec::with_capacity(16);
    result.extend_from_slice(&u64_to_bytes(actual_seized));
    result.extend_from_slice(&u64_to_bytes(actual_repay));
    set_return_data(&result);
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

    // The compatibility host call is the canonical slot number, not wall-clock
    // milliseconds. Treating it as milliseconds divided accrual by another 400.
    let elapsed_slots = now - last_update;

    let total_deposits = load_u64(b"ll_total_deposits");
    let total_borrows = load_u64(b"ll_total_borrows");

    if total_borrows == 0 || total_deposits == 0 {
        store_u64(b"ll_last_update", now);
        return;
    }

    let rate_per_slot = current_rate_per_slot(total_deposits, total_borrows);

    // Interest accrued = total_borrows * rate * elapsed_slots / SCALE
    // Use u128 intermediate to prevent overflow on large values
    let interest = u128_to_u64_saturating(
        (total_borrows as u128) * (rate_per_slot as u128) * (elapsed_slots as u128)
            / (RATE_SCALE as u128),
    );

    if interest > 0 {
        // Reserve factor: portion goes to protocol reserves
        let reserve_factor = load_u64(b"ll_reserve_factor");
        let reserve_amount =
            u128_to_u64_saturating((interest as u128) * (reserve_factor as u128) / 100)
                .min(interest);
        let depositor_interest = interest - reserve_amount;

        let new_total_borrows = match total_borrows.checked_add(interest) {
            Some(value) => value,
            None => {
                log_info("Interest accrual rejected: total borrow overflow");
                return;
            }
        };
        let new_total_deposits = match total_deposits.checked_add(depositor_interest) {
            Some(value) => value,
            None => {
                log_info("Interest accrual rejected: total deposit overflow");
                return;
            }
        };
        let reserves = load_u64(b"ll_reserves");
        let new_reserves = match reserves.checked_add(reserve_amount) {
            Some(value) => value,
            None => {
                log_info("Interest accrual rejected: reserve overflow");
                return;
            }
        };

        let old_borrow_index = load_u64(b"ll_borrow_index");
        let borrow_index_delta = u128_to_u64_saturating(
            (old_borrow_index as u128) * (rate_per_slot as u128) * (elapsed_slots as u128)
                / (RATE_SCALE as u128),
        );
        let new_borrow_index = match old_borrow_index.checked_add(borrow_index_delta) {
            Some(value) => value,
            None => {
                log_info("Interest accrual rejected: borrow index overflow");
                return;
            }
        };

        let old_deposit_index = current_deposit_index();
        let deposit_index_delta = u128_to_u64_saturating(
            old_deposit_index as u128 * depositor_interest as u128 / total_deposits as u128,
        );
        let new_deposit_index = match old_deposit_index.checked_add(deposit_index_delta) {
            Some(value) => value,
            None => {
                log_info("Interest accrual rejected: deposit index overflow");
                return;
            }
        };

        // Increase total borrows by interest (borrowers owe more)
        store_u64(b"ll_total_borrows", new_total_borrows);
        // Increase total deposits by depositor's share (depositors earn)
        store_u64(b"ll_total_deposits", new_total_deposits);
        // Track protocol reserves
        store_u64(b"ll_reserves", new_reserves);

        // P9-SC-01: Update global borrow index proportionally.
        // index_delta = old_index * rate_per_slot * elapsed_slots / RATE_SCALE
        // (same factor as interest / total_borrows)
        store_u64(b"ll_borrow_index", new_borrow_index);
        // Supplier claims grow by their exact pro-rata share of depositor
        // interest, while rounding dust remains as protocol surplus.
        store_u64(b"ll_deposit_index", new_deposit_index);
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
    unsafe {
        core::ptr::copy_nonoverlapping(user_ptr, user.as_mut_ptr(), 32);
    }
    let hex = hex_encode_addr(&user);

    let deposit = compute_current_deposit(&hex);
    // P9-SC-01: Use index-adjusted borrow for accurate health factor
    let borrow = compute_current_borrow(&hex);

    // Health factor in basis points (10000 = 1.0)
    // AUDIT-FIX CON-06: Cast to u128 to prevent overflow for large deposits
    // (deposit * 8500 overflows u64 when deposit > ~2.17×10¹⁵ spores ≈ 2.17M LICN)
    let health_factor = if borrow == 0 {
        u64::MAX // Infinite health
    } else {
        u128_to_u64_saturating(
            (deposit as u128) * (LIQUIDATION_THRESHOLD_PERCENT as u128) * 100 / (borrow as u128),
        )
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
    let utilization = utilization_percent(total_deposits, total_borrows);
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

/// Legacy two-call flash borrowing is disabled because it could commit the
/// outgoing transfer without proving repayment in the same transaction.
/// `flash_repay` remains available only to unwind a pre-upgrade active loan.
#[no_mangle]
pub extern "C" fn flash_borrow(_borrower_ptr: *const u8, _amount: u64) -> u32 {
    log_info("Legacy flash_borrow is disabled; use flash_execute");
    40
}

/// Execute a flash loan atomically through the receiver callback
/// `on_lichen_flash_loan(initiator, token, amount, fee, data, data_len)`.
/// The entire top-level call fails unless the pool's real custody balance is
/// restored with the fee before this function returns.
#[no_mangle]
pub extern "C" fn flash_execute(
    receiver_ptr: *const u8,
    amount: u64,
    data_ptr: *const u8,
    data_len: u32,
) -> u32 {
    if amount == 0 {
        return 1;
    }
    if data_len > MAX_FLASH_CALLBACK_DATA_LEN {
        log_info("Flash callback data is too large");
        return 5;
    }
    if is_paused() {
        log_info("Protocol is paused");
        return 20;
    }
    if load_u64(FLASH_BORROWED_KEY) > 0 {
        log_info("A legacy flash loan must be settled before flash execution");
        return 2;
    }
    if !licn_address_configured() {
        log_info("Lichencoin address not configured");
        return 30;
    }
    let _reentrancy_guard = match reentrancy_enter() {
        Some(guard) => guard,
        None => return 21,
    };

    let mut receiver = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(receiver_ptr, receiver.as_mut_ptr(), 32);
    }
    let self_addr = get_contract_address();
    if is_zero_addr(&receiver) || receiver == self_addr.0 {
        log_info("Flash receiver must be a separate executable contract");
        return 6;
    }

    let mut callback_data = vec![0u8; data_len as usize];
    if data_len > 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(data_ptr, callback_data.as_mut_ptr(), data_len as usize);
        }
    }

    let accounting_before = snapshot_lending_accounting();
    accrue_interest();
    let total_deposits = load_u64(b"ll_total_deposits");
    let total_borrows = load_u64(b"ll_total_borrows");
    if amount > total_deposits.saturating_sub(total_borrows) {
        restore_lending_accounting(accounting_before);
        log_info("Insufficient pool liquidity for flash loan");
        return 3;
    }

    let fee = flash_loan_fee(amount);
    let new_reserves = match load_u64(b"ll_reserves").checked_add(fee) {
        Some(value) => value,
        None => {
            restore_lending_accounting(accounting_before);
            log_info("Flash loan fee would overflow reserves");
            return 4;
        }
    };
    let token = Address(load_licn_addr());
    let starting_balance = match balance_of_token_or_native(token, self_addr) {
        Ok(balance) if balance >= amount => balance,
        _ => {
            restore_lending_accounting(accounting_before);
            log_info("Pool custody balance is unavailable or insufficient");
            return 31;
        }
    };

    if transfer_out(&receiver, amount) != 0 {
        restore_lending_accounting(accounting_before);
        log_info("Flash loan transfer failed");
        return 32;
    }

    let initiator = get_caller();
    let amount_bytes = amount.to_le_bytes();
    let fee_bytes = fee.to_le_bytes();
    let data_len_bytes = data_len.to_le_bytes();
    let callback_args = match encode_layout_args(&[
        &initiator.0,
        &token.0,
        &amount_bytes,
        &fee_bytes,
        &callback_data,
        &data_len_bytes,
    ]) {
        Ok(args) => args,
        Err(_) => {
            restore_lending_accounting(accounting_before);
            return 33;
        }
    };
    if call_contract(CrossCall::new(
        Address(receiver),
        "on_lichen_flash_loan",
        callback_args,
    ))
    .is_err()
    {
        restore_lending_accounting(accounting_before);
        log_info("Flash loan callback failed");
        return 33;
    }

    let required_balance = match starting_balance.checked_add(fee) {
        Some(value) => value,
        None => {
            restore_lending_accounting(accounting_before);
            return 4;
        }
    };
    match balance_of_token_or_native(token, self_addr) {
        Ok(ending_balance) if ending_balance >= required_balance => {}
        _ => {
            restore_lending_accounting(accounting_before);
            log_info("Flash loan callback did not restore principal and fee");
            return 34;
        }
    }

    store_u64(b"ll_reserves", new_reserves);
    set_return_data(&u64_to_bytes(fee));
    log_info("Atomic flash loan completed");
    0
}

/// Repay a flash loan with fee. Must be called after flash_borrow.
#[no_mangle]
pub extern "C" fn flash_repay(borrower_ptr: *const u8, repay_amount: u64) -> u32 {
    let mut _borrower = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(borrower_ptr, _borrower.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != _borrower {
        return 200;
    }
    let _reentrancy_guard = match reentrancy_enter() {
        Some(guard) => guard,
        None => return 21,
    };

    let borrowed = load_u64(FLASH_BORROWED_KEY);
    if borrowed == 0 {
        log_info("No active flash loan");
        return 1;
    }

    let fee = load_u64(FLASH_FEE_KEY);
    let required = match borrowed.checked_add(fee) {
        Some(v) => v,
        None => {
            log_info("Flash repayment requirement overflow");
            return 4;
        }
    };
    if repay_amount < required {
        log_info("Insufficient repayment (must include fee)");
        return 2;
    }

    // AUDIT-FIX G9-01: Verify incoming custody covers flash repayment
    if !receive_licn_in(&_borrower, required) {
        log_info("Insufficient flash repayment");
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

/// Admin pauses new deposits, borrows, and flash loans. Withdrawals,
/// repayments, and liquidations remain available so users can reduce risk.
#[no_mangle]
pub extern "C" fn pause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

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
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

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
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

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
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

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
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

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

/// AUDIT-FIX G9-01: Admin sets the lichencoin contract address for token transfers
#[no_mangle]
pub extern "C" fn set_lichencoin_address(caller_ptr: *const u8, addr_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

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
    unsafe {
        core::ptr::copy_nonoverlapping(addr_ptr, addr.as_mut_ptr(), 32);
    }

    if licn_address_configured() {
        log_info("LichenCoin address already configured");
        return 3;
    }
    storage_set(LICHENCOIN_ADDRESS_KEY, &addr);
    log_info("LichenCoin address configured");
    0
}

/// Admin configures the oracle contract address and asset feed key.
#[no_mangle]
pub extern "C" fn set_oracle_feed(
    caller_ptr: *const u8,
    oracle_addr_ptr: *const u8,
    asset_ptr: *const u8,
    asset_len: u32,
) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_admin(&caller) {
        log_info("Not admin");
        return 1;
    }

    let mut oracle_addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(oracle_addr_ptr, oracle_addr.as_mut_ptr(), 32);
    }
    if is_zero_addr(&oracle_addr) {
        log_info("Cannot set zero oracle address");
        return 2;
    }
    if asset_len == 0 {
        log_info("Cannot set empty oracle asset key");
        return 3;
    }
    if asset_len > MAX_ORACLE_ASSET_KEY_LEN {
        log_info("Oracle asset key is too long");
        return 5;
    }
    if oracle_configuration_present() {
        log_info("Oracle feed already configured");
        return 4;
    }

    let mut asset = vec![0u8; asset_len as usize];
    unsafe {
        core::ptr::copy_nonoverlapping(asset_ptr, asset.as_mut_ptr(), asset_len as usize);
    }

    storage_set(ORACLE_ADDR_KEY, &oracle_addr);
    storage_set(ORACLE_ASSET_KEY, &asset);
    log_info("Oracle feed configured");
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

    let utilization = utilization_percent(total_deposits, total_borrows);

    let rate_per_slot = current_rate_per_slot(total_deposits, total_borrows);

    let available = total_deposits.saturating_sub(total_borrows);

    let mut result = Vec::with_capacity(24);
    result.extend_from_slice(&u64_to_bytes(rate_per_slot));
    result.extend_from_slice(&u64_to_bytes(utilization));
    result.extend_from_slice(&u64_to_bytes(available));
    set_return_data(&result);
    0
}

/// Get the deterministic borrow-rate model:
/// [rate_scale, slots_per_year, base_rate_per_slot, current_rate_per_slot,
///  current_annual_bps, utilization_kink_pct, max_rate_per_slot].
#[no_mangle]
pub extern "C" fn get_rate_model() -> u32 {
    let total_deposits = load_u64(b"ll_total_deposits");
    let total_borrows = load_u64(b"ll_total_borrows");
    let current_rate = current_rate_per_slot(total_deposits, total_borrows);

    let mut result = Vec::with_capacity(56);
    for value in [
        RATE_SCALE,
        SLOTS_PER_YEAR,
        BASE_RATE_SCALED,
        current_rate,
        annual_rate_bps(current_rate),
        UTILIZATION_KINK_PERCENT,
        MAX_RATE_PER_SLOT,
    ] {
        result.extend_from_slice(&u64_to_bytes(value));
    }
    set_return_data(&result);
    0
}

/// Get operational market configuration:
/// [paused, licn_configured, native_licn, oracle_config_present,
///  oracle_config_valid, deposit_cap, reserve_factor_pct,
///  collateral_factor_pct, liquidation_threshold_pct, liquidation_bonus_pct].
#[no_mangle]
pub extern "C" fn get_market_status() -> u32 {
    let licn_configured = licn_address_configured();
    let native_licn = licn_configured && is_zero_addr(&load_licn_addr());
    let mut result = Vec::with_capacity(80);
    for value in [
        is_paused() as u64,
        licn_configured as u64,
        native_licn as u64,
        oracle_configuration_present() as u64,
        oracle_feed_configured() as u64,
        get_deposit_cap(),
        load_u64(b"ll_reserve_factor"),
        COLLATERAL_FACTOR_PERCENT,
        LIQUIDATION_THRESHOLD_PERCENT,
        LIQUIDATION_BONUS_PERCENT,
    ] {
        result.extend_from_slice(&u64_to_bytes(value));
    }
    set_return_data(&result);
    0
}

/// Quote accrued lending yield for an external vault over a slot interval.
#[no_mangle]
pub extern "C" fn get_accrued_interest(principal: u64, elapsed_slots: u64) -> u32 {
    set_return_data(&u64_to_bytes(quote_accrued_interest(
        principal,
        elapsed_slots,
    )));
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
    use lichen_sdk::bytes_to_u64;
    use lichen_sdk::test_mock;

    const LICN_ADDR: [u8; 32] = [99u8; 32];
    const CONTRACT_ADDR: [u8; 32] = [88u8; 32];

    /// Standard setup: reset + configure lichencoin + contract address for transfers
    fn setup() {
        test_mock::reset();
        test_mock::set_contract_address(CONTRACT_ADDR);
        storage_set(LICHENCOIN_ADDRESS_KEY, &LICN_ADDR);
    }

    /// Setup without lichencoin — for testing "lichencoin not configured" error paths
    fn setup_no_licn() {
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
        assert_eq!(load_u64(b"ll_deposit_index"), DEPOSIT_INDEX_SCALE);
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
    fn test_native_deposit_refunds_value_above_credited_amount() {
        setup_no_licn();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        let native_licn = [0u8; 32];
        assert_eq!(
            set_lichencoin_address(admin.as_ptr(), native_licn.as_ptr()),
            0
        );

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000);
        assert_eq!(deposit(user.as_ptr(), 600), 0);
        assert_eq!(load_u64(b"ll_total_deposits"), 600);

        let (target, function, args, _) =
            test_mock::get_last_cross_call().expect("native deposit refund");
        assert_eq!(target, [0u8; 32]);
        assert_eq!(function, "transfer");
        assert_eq!(&args[..32], &user);
        assert_eq!(bytes_to_u64(&args[32..40]), 400);
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

    // AUDIT-FIX G9-01: Deposit rejects failed incoming token custody.
    #[test]
    fn test_deposit_insufficient_value() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(deposit(user.as_ptr(), 1_000_000), 30);
    }

    #[test]
    fn test_deposit_overflow_rejected() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        let hex = hex_encode_addr(&user);
        let dep_key = make_key(b"dep:", &hex);
        store_u64(&dep_key, u64::MAX);
        store_u64(b"ll_total_deposits", u64::MAX);

        test_mock::set_caller(user);
        test_mock::set_value(1);
        assert_eq!(deposit(user.as_ptr(), 1), 5);
        assert_eq!(load_u64(&dep_key), u64::MAX);
        assert_eq!(load_u64(b"ll_total_deposits"), u64::MAX);
    }

    #[test]
    fn test_deposit_count_saturates() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        store_u64(DEPOSIT_COUNT_KEY, u64::MAX);

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1);
        assert_eq!(deposit(user.as_ptr(), 1), 0);
        assert_eq!(load_u64(DEPOSIT_COUNT_KEY), u64::MAX);
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
    fn test_withdraw_rejects_false_transfer_status_and_reverts() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        assert_eq!(deposit(user.as_ptr(), 1_000_000), 0);

        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(withdraw(user.as_ptr(), 500_000), 31);
        assert_eq!(load_u64(b"ll_total_deposits"), 1_000_000);
        let hex = hex_encode_addr(&user);
        assert_eq!(load_u64(&make_key(b"dep:", &hex)), 1_000_000);
    }

    #[test]
    fn test_withdraw_transfer_failure_restores_accrued_interest_state() {
        setup();
        test_mock::set_timestamp(1_000);
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(10_000_000);
        assert_eq!(deposit(user.as_ptr(), 10_000_000), 0);
        assert_eq!(borrow(user.as_ptr(), 5_000_000), 0);

        let hex = hex_encode_addr(&user);
        let dep_key = make_key(b"dep:", &hex);
        let dix_key = make_key(b"dix:", &hex);
        test_mock::set_timestamp(11_000);
        let accounting_before = snapshot_lending_accounting();
        let deposit_before = storage_get(&dep_key);
        let dix_before = storage_get(&dix_key);

        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(withdraw(user.as_ptr(), 1_000), 31);

        assert_eq!(snapshot_lending_accounting(), accounting_before);
        assert_eq!(storage_get(&dep_key), deposit_before);
        assert_eq!(storage_get(&dix_key), dix_before);
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
    fn test_withdraw_fails_closed_on_inconsistent_pool_liquidity() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        let user_hex = hex_encode_addr(&user);
        let dep_key = make_key(b"dep:", &user_hex);
        store_u64(&dep_key, 100);
        store_u64(b"ll_total_deposits", 50);

        test_mock::set_caller(user);
        assert_eq!(withdraw(user.as_ptr(), 100), 4);
        assert_eq!(load_u64(&dep_key), 100);
        assert_eq!(load_u64(b"ll_total_deposits"), 50);
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
    fn test_borrow_transfer_failure_restores_settlement_and_count_state() {
        setup();
        test_mock::set_timestamp(1_000);
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(10_000_000);
        assert_eq!(deposit(user.as_ptr(), 10_000_000), 0);
        assert_eq!(borrow(user.as_ptr(), 5_000_000), 0);

        let hex = hex_encode_addr(&user);
        let borrow_key = make_key(b"bor:", &hex);
        let bix_key = make_key(b"bix:", &hex);
        let ts_key = make_key(b"bts:", &hex);
        test_mock::set_timestamp(11_000);
        let accounting_before = snapshot_lending_accounting();
        let borrow_before = load_u64(&borrow_key);
        let bix_before = load_u64(&bix_key);
        let ts_before = load_u64(&ts_key);
        let borrow_count_before = load_u64(BORROW_COUNT_KEY);

        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(borrow(user.as_ptr(), 1_000), 31);

        assert_eq!(snapshot_lending_accounting(), accounting_before);
        assert_eq!(load_u64(&borrow_key), borrow_before);
        assert_eq!(load_u64(&bix_key), bix_before);
        assert_eq!(load_u64(&ts_key), ts_before);
        assert_eq!(load_u64(BORROW_COUNT_KEY), borrow_count_before);
    }

    #[test]
    fn test_first_borrow_transfer_failure_does_not_create_zero_state() {
        setup();
        test_mock::set_timestamp(1_000);
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(10_000_000);
        assert_eq!(deposit(user.as_ptr(), 10_000_000), 0);

        let hex = hex_encode_addr(&user);
        let borrow_key = make_key(b"bor:", &hex);
        let bix_key = make_key(b"bix:", &hex);
        let ts_key = make_key(b"bts:", &hex);
        let accounting_before = snapshot_lending_accounting();
        assert_eq!(test_mock::get_storage(&borrow_key), None);
        assert_eq!(test_mock::get_storage(&bix_key), None);
        assert_eq!(test_mock::get_storage(&ts_key), None);
        assert_eq!(test_mock::get_storage(BORROW_COUNT_KEY), None);
        assert_eq!(test_mock::get_storage(b"ll_reserves"), None);

        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(borrow(user.as_ptr(), 1_000), 31);

        assert_eq!(snapshot_lending_accounting(), accounting_before);
        assert_eq!(test_mock::get_storage(&borrow_key), None);
        assert_eq!(test_mock::get_storage(&bix_key), None);
        assert_eq!(test_mock::get_storage(&ts_key), None);
        assert_eq!(test_mock::get_storage(BORROW_COUNT_KEY), None);
        assert_eq!(test_mock::get_storage(b"ll_reserves"), None);
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
        let hex = hex_encode_addr(&user);
        let borrow_key = make_key(b"bor:", &hex);
        let bix_key = make_key(b"bix:", &hex);
        let ts_key = make_key(b"bts:", &hex);
        let accounting_before = snapshot_lending_accounting();
        let borrow_before = storage_get(&borrow_key);
        let bix_before = storage_get(&bix_key);
        let ts_before = storage_get(&ts_key);
        let borrow_count_before = storage_get(BORROW_COUNT_KEY);
        assert_eq!(borrow(user.as_ptr(), 750_001), 2); // > 75%
        assert_eq!(snapshot_lending_accounting(), accounting_before);
        assert_eq!(storage_get(&borrow_key), borrow_before);
        assert_eq!(storage_get(&bix_key), bix_before);
        assert_eq!(storage_get(&ts_key), ts_before);
        assert_eq!(storage_get(BORROW_COUNT_KEY), borrow_count_before);
    }

    #[test]
    fn test_borrow_rejection_does_not_create_reentrancy_state() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let user = [2u8; 32];
        test_mock::set_caller(user);
        let accounting_before = snapshot_lending_accounting();
        assert_eq!(test_mock::get_storage(REENTRANCY_KEY), None);

        assert_eq!(borrow(user.as_ptr(), 1_000), 2);

        assert_eq!(snapshot_lending_accounting(), accounting_before);
        assert_eq!(test_mock::get_storage(REENTRANCY_KEY), None);
    }

    #[test]
    fn test_borrow_configured_oracle_failure_rejected() {
        setup();
        let admin = [1u8; 32];
        let oracle = [7u8; 32];
        let asset = b"LICN/USD";
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(
            set_oracle_feed(
                admin.as_ptr(),
                oracle.as_ptr(),
                asset.as_ptr(),
                asset.len() as u32,
            ),
            0
        );

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        assert_eq!(deposit(user.as_ptr(), 1_000_000), 0);
        assert_eq!(borrow(user.as_ptr(), 500_000), 6);
        assert_eq!(load_u64(b"ll_total_borrows"), 0);
    }

    #[test]
    fn test_borrow_configured_oracle_zero_price_rejected() {
        setup();
        let admin = [1u8; 32];
        let oracle = [7u8; 32];
        let asset = b"LICN/USD";
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(
            set_oracle_feed(
                admin.as_ptr(),
                oracle.as_ptr(),
                asset.as_ptr(),
                asset.len() as u32,
            ),
            0
        );

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        assert_eq!(deposit(user.as_ptr(), 1_000_000), 0);
        test_mock::set_cross_call_response(Some(u64_to_bytes(0).to_vec()));
        assert_eq!(borrow(user.as_ptr(), 500_000), 6);
        assert_eq!(load_u64(b"ll_total_borrows"), 0);
    }

    #[test]
    fn test_oracle_quote_cannot_inflate_same_asset_borrowing_power() {
        setup();
        let admin = [1u8; 32];
        let oracle = [7u8; 32];
        let asset = b"LICN";
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(
            set_oracle_feed(
                admin.as_ptr(),
                oracle.as_ptr(),
                asset.as_ptr(),
                asset.len() as u32,
            ),
            0
        );

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        assert_eq!(deposit(user.as_ptr(), 1_000_000), 0);
        test_mock::set_cross_call_response(Some(u64_to_bytes(10_000_000).to_vec()));

        assert_eq!(
            borrow(user.as_ptr(), 750_001),
            2,
            "an 8-decimal USD quote must not multiply LICN-against-LICN collateral"
        );
        assert_eq!(load_u64(b"ll_total_borrows"), 0);
    }

    #[test]
    fn test_get_oracle_price_uses_configured_feed_surface() {
        setup();
        let admin = [1u8; 32];
        let oracle = [7u8; 32];
        let asset = b"LICN/USD";
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(
            set_oracle_feed(
                admin.as_ptr(),
                oracle.as_ptr(),
                asset.as_ptr(),
                asset.len() as u32,
            ),
            0
        );
        test_mock::set_cross_call_response(Some(u64_to_bytes(2).to_vec()));

        assert_eq!(get_oracle_price(), 2);

        let (target, function, args, value) =
            test_mock::get_last_cross_call().expect("oracle cross-call should be captured");
        assert_eq!(target, oracle);
        assert_eq!(function, "get_price_value");
        assert_eq!(value, 0);
        assert_eq!(&args[..asset.len()], asset);
        assert_eq!(
            bytes_to_u64(&args[asset.len()..asset.len() + 8]),
            asset.len() as u64
        );
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
    fn test_flash_repay_rejects_required_overflow() {
        setup();
        let borrower = [3u8; 32];
        test_mock::set_caller(borrower);
        store_u64(FLASH_BORROWED_KEY, u64::MAX);
        store_u64(FLASH_FEE_KEY, 1);

        assert_eq!(flash_repay(borrower.as_ptr(), 0), 4);
        assert_eq!(load_u64(FLASH_BORROWED_KEY), u64::MAX);
        assert_eq!(load_u64(FLASH_FEE_KEY), 1);
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
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 500_000);
        let (_, function, args, _) =
            test_mock::get_last_cross_call().expect("repayment transfer_from call");
        assert_eq!(function, "transfer_from");
        assert_eq!(bytes_to_u64(&args[args.len() - 8..]), 500_000);
    }

    #[test]
    fn test_repay_fails_closed_on_total_borrow_underflow() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let borrower = [2u8; 32];
        let borrower_hex = hex_encode_addr(&borrower);
        let borrow_key = make_key(b"bor:", &borrower_hex);
        store_u64(&borrow_key, 100);
        store_u64(&make_key(b"bix:", &borrower_hex), BORROW_INDEX_SCALE);
        store_u64(b"ll_total_deposits", 1_000);
        store_u64(b"ll_total_borrows", 50);

        test_mock::set_caller(borrower);
        assert_eq!(repay(borrower.as_ptr(), 100), 5);
        assert_eq!(load_u64(&borrow_key), 100);
        assert_eq!(load_u64(b"ll_total_borrows"), 50);
    }

    #[test]
    fn test_native_repay_overpay_refunds_every_unused_spore() {
        setup_no_licn();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        let native_licn = [0u8; 32];
        assert_eq!(
            set_lichencoin_address(admin.as_ptr(), native_licn.as_ptr()),
            0
        );

        let borrower = [2u8; 32];
        let borrower_hex = hex_encode_addr(&borrower);
        store_u64(&make_key(b"bor:", &borrower_hex), 500_000);
        store_u64(&make_key(b"bix:", &borrower_hex), BORROW_INDEX_SCALE);
        store_u64(b"ll_total_deposits", 1_000_000);
        store_u64(b"ll_total_borrows", 500_000);

        test_mock::set_caller(borrower);
        test_mock::set_value(900_000);
        assert_eq!(repay(borrower.as_ptr(), 900_000), 0);
        assert_eq!(load_u64(b"ll_total_borrows"), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 500_000);

        let (target, function, args, value) =
            test_mock::get_last_cross_call().expect("native overpayment refund");
        assert_eq!(target, [0u8; 32]);
        assert_eq!(function, "transfer");
        assert_eq!(&args[..32], &borrower);
        assert_eq!(bytes_to_u64(&args[32..40]), 400_000);
        assert_eq!(value, 0);
    }

    // AUDIT-FIX G9-01: Repay rejects failed incoming token custody.
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
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
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
        assert_eq!(
            liquidate(liquidator.as_ptr(), borrower.as_ptr(), 200_000),
            0
        );
        let borrow_after = load_u64(&bor_key);
        assert!(borrow_after < 860_000);
    }

    #[test]
    fn test_liquidation_caps_close_factor_and_reports_exact_repayment() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let borrower = [2u8; 32];
        let borrower_hex = hex_encode_addr(&borrower);
        store_u64(&make_key(b"dep:", &borrower_hex), 1_000_000);
        store_u64(&make_key(b"dix:", &borrower_hex), DEPOSIT_INDEX_SCALE);
        store_u64(&make_key(b"bor:", &borrower_hex), 900_000);
        store_u64(&make_key(b"bix:", &borrower_hex), BORROW_INDEX_SCALE);
        store_u64(b"ll_total_deposits", 1_000_000);
        store_u64(b"ll_total_borrows", 900_000);

        let liquidator = [3u8; 32];
        test_mock::set_caller(liquidator);
        assert_eq!(
            liquidate(liquidator.as_ptr(), borrower.as_ptr(), 900_000),
            0
        );
        let result = test_mock::get_return_data();
        assert_eq!(result.len(), 16);
        assert_eq!(bytes_to_u64(&result[..8]), 472_500);
        assert_eq!(bytes_to_u64(&result[8..16]), 450_000);
        assert_eq!(load_u64(b"ll_total_borrows"), 450_000);
    }

    #[test]
    fn test_liquidation_caps_repayment_to_compensating_collateral() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let borrower = [2u8; 32];
        let borrower_hex = hex_encode_addr(&borrower);
        store_u64(&make_key(b"dep:", &borrower_hex), 100);
        store_u64(&make_key(b"dix:", &borrower_hex), DEPOSIT_INDEX_SCALE);
        store_u64(&make_key(b"bor:", &borrower_hex), 900);
        store_u64(&make_key(b"bix:", &borrower_hex), BORROW_INDEX_SCALE);
        store_u64(b"ll_total_deposits", 100);
        store_u64(b"ll_total_borrows", 900);

        let liquidator = [3u8; 32];
        test_mock::set_caller(liquidator);
        assert_eq!(liquidate(liquidator.as_ptr(), borrower.as_ptr(), 450), 0);
        let result = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&result[..8]), 100);
        assert_eq!(bytes_to_u64(&result[8..16]), 96);
        assert_eq!(load_u64(b"ll_total_deposits"), 0);
        assert_eq!(load_u64(b"ll_total_borrows"), 804);
    }

    #[test]
    fn test_liquidation_fails_closed_on_total_deposit_underflow() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let borrower = [2u8; 32];
        let borrower_hex = hex_encode_addr(&borrower);
        let dep_key = make_key(b"dep:", &borrower_hex);
        let borrow_key = make_key(b"bor:", &borrower_hex);
        store_u64(&dep_key, 1_000);
        store_u64(&make_key(b"dix:", &borrower_hex), DEPOSIT_INDEX_SCALE);
        store_u64(&borrow_key, 900);
        store_u64(&make_key(b"bix:", &borrower_hex), BORROW_INDEX_SCALE);
        store_u64(b"ll_total_deposits", 100);
        store_u64(b"ll_total_borrows", 900);

        let liquidator = [3u8; 32];
        test_mock::set_caller(liquidator);
        assert_eq!(liquidate(liquidator.as_ptr(), borrower.as_ptr(), 100), 5);
        assert_eq!(load_u64(&dep_key), 1_000);
        assert_eq!(load_u64(&borrow_key), 900);
        assert_eq!(load_u64(b"ll_total_deposits"), 100);
        assert_eq!(load_u64(b"ll_total_borrows"), 900);
    }

    #[test]
    fn test_native_liquidation_refund_failure_reverts_before_charging() {
        setup_no_licn();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let native_licn = [0u8; 32];
        assert_eq!(
            set_lichencoin_address(admin.as_ptr(), native_licn.as_ptr()),
            0
        );

        let borrower = [2u8; 32];
        let borrower_hex = hex_encode_addr(&borrower);
        let dep_key = make_key(b"dep:", &borrower_hex);
        let borrow_key = make_key(b"bor:", &borrower_hex);
        store_u64(&dep_key, 1_000_000);
        store_u64(&make_key(b"dix:", &borrower_hex), DEPOSIT_INDEX_SCALE);
        store_u64(&borrow_key, 900_000);
        store_u64(&make_key(b"bix:", &borrower_hex), BORROW_INDEX_SCALE);
        store_u64(b"ll_total_deposits", 1_000_000);
        store_u64(b"ll_total_borrows", 900_000);

        let liquidator = [3u8; 32];
        test_mock::set_caller(liquidator);
        test_mock::set_value(900_000);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(
            liquidate(liquidator.as_ptr(), borrower.as_ptr(), 900_000),
            30
        );
        assert_eq!(load_u64(&dep_key), 1_000_000);
        assert_eq!(load_u64(&borrow_key), 900_000);
        assert_eq!(load_u64(b"ll_total_borrows"), 900_000);

        let (target, function, args, _) =
            test_mock::get_last_cross_call().expect("native liquidation refund");
        assert_eq!(target, [0u8; 32]);
        assert_eq!(function, "transfer");
        assert_eq!(&args[..32], &liquidator);
        assert_eq!(bytes_to_u64(&args[32..40]), 450_000);
    }

    #[test]
    fn test_liquidate_transfer_failure_restores_borrower_and_interest_state() {
        setup();
        test_mock::set_timestamp(1_000);
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let borrower = [2u8; 32];
        let hex = hex_encode_addr(&borrower);
        let dep_key = make_key(b"dep:", &hex);
        let borrow_key = make_key(b"bor:", &hex);
        let bix_key = make_key(b"bix:", &hex);
        store_u64(&dep_key, 1_000_000);
        store_u64(&borrow_key, 900_000);
        store_u64(&bix_key, BORROW_INDEX_SCALE);
        store_u64(b"ll_total_deposits", 1_000_000);
        store_u64(b"ll_total_borrows", 900_000);
        store_u64(LIQUIDATION_COUNT_KEY, 7);

        let liquidator = [3u8; 32];
        test_mock::set_caller(liquidator);
        test_mock::set_value(200_000);
        test_mock::set_timestamp(11_000);
        let accounting_before = snapshot_lending_accounting();
        let deposit_before = load_u64(&dep_key);
        let borrow_before = load_u64(&borrow_key);
        let bix_before = load_u64(&bix_key);
        let liquidation_count_before = load_u64(LIQUIDATION_COUNT_KEY);

        test_mock::set_cross_call_responses(std::vec![
            0u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(
            liquidate(liquidator.as_ptr(), borrower.as_ptr(), 200_000),
            31
        );

        assert_eq!(snapshot_lending_accounting(), accounting_before);
        assert_eq!(load_u64(&dep_key), deposit_before);
        assert_eq!(load_u64(&borrow_key), borrow_before);
        assert_eq!(load_u64(&bix_key), bix_before);
        assert_eq!(load_u64(LIQUIDATION_COUNT_KEY), liquidation_count_before);
    }

    #[test]
    fn test_liquidation_limit_uses_u128_without_wrap() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let borrower = [2u8; 32];
        let hex = hex_encode_addr(&borrower);
        let dep_key = make_key(b"dep:", &hex);
        let bor_key = make_key(b"bor:", &hex);
        store_u64(&dep_key, u64::MAX);
        store_u64(&bor_key, 500_000_000_000_000_000);
        store_u64(b"ll_total_deposits", u64::MAX);
        store_u64(b"ll_total_borrows", 500_000_000_000_000_000);

        let liquidator = [3u8; 32];
        test_mock::set_caller(liquidator);
        test_mock::set_value(1_000_000);
        assert_eq!(
            liquidate(liquidator.as_ptr(), borrower.as_ptr(), 1_000_000),
            3
        );
        assert_eq!(load_u64(&bor_key), 500_000_000_000_000_000);
        assert_eq!(load_u64(&dep_key), u64::MAX);
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
        assert_eq!(
            liquidate(liquidator.as_ptr(), borrower.as_ptr(), 100_000),
            3
        );
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
        assert_eq!(
            liquidate(liquidator.as_ptr(), borrower.as_ptr(), 100_000),
            2
        );
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
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(
            liquidate(liquidator.as_ptr(), borrower.as_ptr(), 200_000),
            30
        );
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
    fn test_get_account_info_reports_indexed_supplier_claim() {
        setup();
        test_mock::set_timestamp(1_000);
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(10_000_000);
        assert_eq!(deposit(user.as_ptr(), 10_000_000), 0);
        assert_eq!(borrow(user.as_ptr(), 5_000_000), 0);

        test_mock::set_timestamp(101_000);
        accrue_interest();
        let hex = hex_encode_addr(&user);
        let stored_principal = load_u64(&make_key(b"dep:", &hex));
        let expected_claim = compute_current_deposit(&hex);
        assert!(expected_claim > stored_principal);

        assert_eq!(get_account_info(user.as_ptr()), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret[0..8]), expected_claim);
        assert_eq!(load_u64(&make_key(b"dep:", &hex)), stored_principal);
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

    #[test]
    fn test_protocol_stats_utilization_saturates() {
        setup();
        store_u64(b"ll_total_deposits", 1);
        store_u64(b"ll_total_borrows", u64::MAX);

        assert_eq!(get_protocol_stats(), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret[16..24]), u64::MAX);
    }

    // ========================================================================
    // v2 TESTS
    // ========================================================================

    #[test]
    fn test_legacy_flash_repay_remains_available_for_upgrade_unwind() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let borrower = [3u8; 32];
        test_mock::set_caller(borrower);
        store_u64(FLASH_BORROWED_KEY, 100_000);
        store_u64(FLASH_FEE_KEY, 90);

        assert_eq!(flash_repay(borrower.as_ptr(), 100_000), 2);
        assert_eq!(flash_repay(borrower.as_ptr(), 100_090), 0);
        assert_eq!(load_u64(b"ll_reserves"), 90);
        assert_eq!(load_u64(FLASH_BORROWED_KEY), 0);
    }

    #[test]
    fn test_legacy_flash_borrow_is_disabled() {
        setup();
        let borrower = [3u8; 32];
        assert_eq!(flash_borrow(borrower.as_ptr(), 100_000), 40);
        assert_eq!(load_u64(FLASH_BORROWED_KEY), 0);
    }

    #[test]
    fn test_atomic_flash_execute_no_liquidity() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000);
        deposit(user.as_ptr(), 1_000);

        let receiver = [3u8; 32];
        assert_eq!(flash_execute(receiver.as_ptr(), 2_000, [].as_ptr(), 0), 3);
    }

    #[test]
    fn test_atomic_flash_execute_rejects_underpayment_and_restores_accounting() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        store_u64(b"ll_total_deposits", 1_000_000);
        store_u64(b"ll_total_borrows", 100_000);
        let accounting_before = snapshot_lending_accounting();

        let receiver = [3u8; 32];
        test_mock::set_cross_call_responses(std::vec![
            u64_to_bytes(1_000_000).to_vec(),
            0u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
            u64_to_bytes(1_000_089).to_vec(),
        ]);
        assert_eq!(
            flash_execute(receiver.as_ptr(), 100_000, [].as_ptr(), 0),
            34
        );
        assert_eq!(snapshot_lending_accounting(), accounting_before);
        assert_eq!(test_mock::get_storage(REENTRANCY_KEY), None);
    }

    #[test]
    fn test_atomic_flash_execute_rejects_transfer_failure() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        store_u64(b"ll_total_deposits", 1_000_000);

        let receiver = [3u8; 32];
        test_mock::set_cross_call_responses(std::vec![
            u64_to_bytes(1_000_000).to_vec(),
            2u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(
            flash_execute(receiver.as_ptr(), 100_000, [].as_ptr(), 0),
            32
        );
        assert_eq!(load_u64(b"ll_reserves"), 0);
        assert_eq!(test_mock::get_storage(REENTRANCY_KEY), None);
    }

    #[test]
    fn test_atomic_flash_execute_validates_receiver_and_callback_data() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let zero = [0u8; 32];
        assert_eq!(flash_execute(zero.as_ptr(), 1, [].as_ptr(), 0), 6);
        assert_eq!(flash_execute(CONTRACT_ADDR.as_ptr(), 1, [].as_ptr(), 0), 6);

        let receiver = [3u8; 32];
        let oversized = [0u8; MAX_FLASH_CALLBACK_DATA_LEN as usize + 1];
        assert_eq!(
            flash_execute(
                receiver.as_ptr(),
                1,
                oversized.as_ptr(),
                oversized.len() as u32,
            ),
            5
        );
    }

    #[test]
    fn test_atomic_flash_execute_rejects_active_legacy_loan() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        store_u64(FLASH_BORROWED_KEY, 100_000);

        let receiver = [3u8; 32];
        assert_eq!(flash_execute(receiver.as_ptr(), 50_000, [].as_ptr(), 0), 2);
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
        let borrower = [3u8; 32];
        test_mock::set_caller(borrower);
        store_u64(FLASH_BORROWED_KEY, 100_000);
        store_u64(FLASH_FEE_KEY, 90);

        // Repay amount sufficient but incoming token custody fails.
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
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
        assert_eq!(borrow(user.as_ptr(), 1_000), 20);
        assert_eq!(flash_execute(user.as_ptr(), 1_000, [].as_ptr(), 0), 20);

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
    fn test_withdraw_still_works_when_paused() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        assert_eq!(deposit(user.as_ptr(), 1_000_000), 0);

        test_mock::set_caller(admin);
        assert_eq!(pause(admin.as_ptr()), 0);

        test_mock::set_caller(user);
        assert_eq!(withdraw(user.as_ptr(), 100_000), 0);
        assert_eq!(load_u64(b"ll_total_deposits"), 900_000);
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
    fn test_atomic_flash_execute_collects_minimum_fee() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);

        let receiver = [3u8; 32];
        test_mock::set_cross_call_responses(std::vec![
            u64_to_bytes(1_000_000).to_vec(),
            0u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
            u64_to_bytes(1_000_001).to_vec(),
        ]);
        assert_eq!(flash_execute(receiver.as_ptr(), 100, [].as_ptr(), 0), 0);
        let fee = bytes_to_u64(&test_mock::get_return_data());
        assert_eq!(fee, 1);
        assert_eq!(load_u64(b"ll_reserves"), 1);
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
        assert_eq!(
            liquidate(liquidator.as_ptr(), borrower.as_ptr(), 200_000),
            0
        );
    }

    #[test]
    fn test_get_accrued_interest_returns_current_quote() {
        setup();
        store_u64(b"ll_total_deposits", 1_000_000);
        store_u64(b"ll_total_borrows", 800_000);

        assert_eq!(get_accrued_interest(500_000_000_000, 1_000), 0);

        let quoted = bytes_to_u64(&test_mock::get_return_data());
        assert_eq!(quoted, quote_accrued_interest(500_000_000_000, 1_000));
        assert!(quoted > 0);
    }

    #[test]
    fn test_rate_units_annualize_base_to_two_percent() {
        assert_eq!(SLOTS_PER_YEAR, 78_894_000);
        assert_eq!(annual_rate_bps(BASE_RATE_SCALED), 200);
        assert_eq!(current_rate_per_slot(1_000, 0), BASE_RATE_SCALED);
        assert_eq!(annual_rate_bps(current_rate_per_slot(1_000, 800)), 520);
        assert_eq!(annual_rate_bps(current_rate_per_slot(1_000, 1_000)), 921);
        assert_eq!(
            current_rate_per_slot(1, u64::MAX),
            current_rate_per_slot(1, 1)
        );
    }

    #[test]
    fn test_interest_elapsed_time_is_canonical_slots_without_ms_division() {
        setup();
        store_u64(b"ll_total_deposits", 1_000_000_000_000);
        store_u64(b"ll_total_borrows", 500_000_000_000);
        store_u64(b"ll_borrow_index", BORROW_INDEX_SCALE);
        store_u64(b"ll_deposit_index", DEPOSIT_INDEX_SCALE);
        store_u64(b"ll_last_update", 100);
        test_mock::set_timestamp(101);

        accrue_interest();

        // At 50% utilization the rate is 508 / 1e12 per canonical slot.
        assert_eq!(load_u64(b"ll_total_borrows"), 500_000_000_254);
        assert_eq!(load_u64(b"ll_last_update"), 101);
    }

    #[test]
    fn test_rate_and_market_views_publish_scales_and_configuration() {
        setup_no_licn();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let native_licn = [0u8; 32];
        assert_eq!(
            set_lichencoin_address(admin.as_ptr(), native_licn.as_ptr()),
            0
        );

        assert_eq!(get_rate_model(), 0);
        let rate = test_mock::get_return_data();
        assert_eq!(rate.len(), 56);
        assert_eq!(bytes_to_u64(&rate[0..8]), RATE_SCALE);
        assert_eq!(bytes_to_u64(&rate[8..16]), SLOTS_PER_YEAR);
        assert_eq!(bytes_to_u64(&rate[16..24]), BASE_RATE_SCALED);
        assert_eq!(bytes_to_u64(&rate[32..40]), 200);

        assert_eq!(get_market_status(), 0);
        let status = test_mock::get_return_data();
        assert_eq!(status.len(), 80);
        assert_eq!(bytes_to_u64(&status[0..8]), 0);
        assert_eq!(bytes_to_u64(&status[8..16]), 1);
        assert_eq!(bytes_to_u64(&status[16..24]), 1);
        assert_eq!(bytes_to_u64(&status[24..32]), 0);
        assert_eq!(bytes_to_u64(&status[32..40]), 0);
        assert_eq!(bytes_to_u64(&status[48..56]), 10);
    }

    // ========================================================================
    // AUDIT-FIX G9-01: Token transfer wiring tests
    // ========================================================================

    #[test]
    fn test_set_lichencoin_address() {
        setup_no_licn();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let licn = [77u8; 32];
        assert_eq!(set_lichencoin_address(admin.as_ptr(), licn.as_ptr()), 0);
        assert_eq!(load_licn_addr(), licn);
    }

    #[test]
    fn test_set_lichencoin_address_non_admin() {
        setup_no_licn();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let other = [9u8; 32];
        test_mock::set_caller(other);
        let licn = [77u8; 32];
        assert_eq!(set_lichencoin_address(other.as_ptr(), licn.as_ptr()), 1);
    }

    #[test]
    fn test_set_lichencoin_address_accepts_native_licn() {
        setup_no_licn();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let zero = [0u8; 32];
        assert_eq!(set_lichencoin_address(admin.as_ptr(), zero.as_ptr()), 0);
        assert_eq!(load_licn_addr(), zero);
    }

    #[test]
    fn test_set_lichencoin_address_cannot_reconfigure() {
        setup_no_licn();
        let admin = [1u8; 32];
        let first = [77u8; 32];
        let second = [88u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        assert_eq!(set_lichencoin_address(admin.as_ptr(), first.as_ptr()), 0);
        assert_eq!(set_lichencoin_address(admin.as_ptr(), second.as_ptr()), 3);
        assert_eq!(load_licn_addr(), first);
    }

    #[test]
    fn test_set_oracle_feed_cannot_reconfigure() {
        setup();
        let admin = [1u8; 32];
        let first_oracle = [7u8; 32];
        let second_oracle = [8u8; 32];
        let first_asset = b"LICN/USD";
        let second_asset = b"LICN/EUR";
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        assert_eq!(
            set_oracle_feed(
                admin.as_ptr(),
                first_oracle.as_ptr(),
                first_asset.as_ptr(),
                first_asset.len() as u32,
            ),
            0
        );
        assert_eq!(
            set_oracle_feed(
                admin.as_ptr(),
                second_oracle.as_ptr(),
                second_asset.as_ptr(),
                second_asset.len() as u32,
            ),
            4
        );
        assert_eq!(
            test_mock::get_storage(ORACLE_ADDR_KEY).unwrap().as_slice(),
            &first_oracle
        );
        assert_eq!(
            test_mock::get_storage(ORACLE_ASSET_KEY).unwrap().as_slice(),
            first_asset
        );
    }

    #[test]
    fn test_set_oracle_feed_rejects_oversized_asset_key_without_mutation() {
        setup();
        let admin = [1u8; 32];
        let oracle = [7u8; 32];
        let asset = [b'A'; MAX_ORACLE_ASSET_KEY_LEN as usize + 1];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        assert_eq!(
            set_oracle_feed(
                admin.as_ptr(),
                oracle.as_ptr(),
                asset.as_ptr(),
                asset.len() as u32,
            ),
            5
        );
        assert_eq!(test_mock::get_storage(ORACLE_ADDR_KEY), None);
        assert_eq!(test_mock::get_storage(ORACLE_ASSET_KEY), None);
    }

    #[test]
    fn test_partial_or_malformed_oracle_configuration_fails_closed() {
        setup();
        let admin = [1u8; 32];
        let oracle = [7u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        storage_set(ORACLE_ADDR_KEY, &oracle);
        assert_eq!(try_get_oracle_price(), None);
        assert!(!oracle_feed_configured());
        assert_eq!(
            set_oracle_feed(admin.as_ptr(), oracle.as_ptr(), b"LICN/USD".as_ptr(), 8),
            4
        );

        storage_set(ORACLE_ASSET_KEY, b"");
        assert_eq!(try_get_oracle_price(), None);
        storage_set(ORACLE_ADDR_KEY, &[7u8; 31]);
        storage_set(ORACLE_ASSET_KEY, b"LICN/USD");
        assert_eq!(try_get_oracle_price(), None);
    }

    #[test]
    fn test_withdraw_without_licn_configured() {
        setup_no_licn();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        assert_eq!(deposit(user.as_ptr(), 1_000_000), 30);
        let user_hex = hex_encode_addr(&user);
        store_u64(&make_key(b"dep:", &user_hex), 1_000_000);
        store_u64(b"ll_total_deposits", 1_000_000);
        // Withdraw should fail because lichencoin not configured for outgoing transfer
        assert_eq!(withdraw(user.as_ptr(), 500_000), 30);
        // Bookkeeping should be reverted
        assert_eq!(load_u64(b"ll_total_deposits"), 1_000_000);
    }

    #[test]
    fn test_borrow_without_licn_configured() {
        setup_no_licn();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        assert_eq!(deposit(user.as_ptr(), 1_000_000), 30);
        let user_hex = hex_encode_addr(&user);
        store_u64(&make_key(b"dep:", &user_hex), 1_000_000);
        store_u64(b"ll_total_deposits", 1_000_000);
        assert_eq!(borrow(user.as_ptr(), 500_000), 30);
        // Bookkeeping should be reverted
        assert_eq!(load_u64(b"ll_total_borrows"), 0);
    }

    #[test]
    fn test_flash_execute_without_licn_configured() {
        setup_no_licn();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000);
        assert_eq!(deposit(user.as_ptr(), 1_000_000), 30);
        store_u64(b"ll_total_deposits", 1_000_000);
        let receiver = [3u8; 32];
        assert_eq!(
            flash_execute(receiver.as_ptr(), 100_000, [].as_ptr(), 0),
            30
        );
    }

    #[test]
    fn test_withdraw_reserves_without_licn_configured() {
        setup_no_licn();
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

    // ========================================================================
    // Supplier deposit-index accounting tests
    // ========================================================================

    #[test]
    fn test_supplier_interest_is_settled_and_withdrawable() {
        setup();
        test_mock::set_timestamp(1_000);
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let supplier = [2u8; 32];
        test_mock::set_caller(supplier);
        test_mock::set_value(6_000_000);
        assert_eq!(deposit(supplier.as_ptr(), 6_000_000), 0);

        let borrower = [3u8; 32];
        test_mock::set_caller(borrower);
        test_mock::set_value(10_000_000);
        assert_eq!(deposit(borrower.as_ptr(), 10_000_000), 0);
        assert_eq!(borrow(borrower.as_ptr(), 5_000_000), 0);

        test_mock::set_timestamp(101_000);
        accrue_interest();
        let supplier_hex = hex_encode_addr(&supplier);
        let claim = compute_current_deposit(&supplier_hex);
        assert!(claim > 6_000_000, "supplier did not earn interest");

        test_mock::set_caller(supplier);
        assert_eq!(withdraw(supplier.as_ptr(), claim), 0);
        assert_eq!(load_u64(&make_key(b"dep:", &supplier_hex)), 0);
        assert_eq!(
            load_u64(&make_key(b"dix:", &supplier_hex)),
            current_deposit_index()
        );
    }

    #[test]
    fn test_supplier_interest_is_proportional_with_bounded_rounding_dust() {
        setup();
        test_mock::set_timestamp(1_000);
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let supplier_a = [2u8; 32];
        test_mock::set_caller(supplier_a);
        test_mock::set_value(3_000_000);
        assert_eq!(deposit(supplier_a.as_ptr(), 3_000_000), 0);

        let supplier_b = [3u8; 32];
        test_mock::set_caller(supplier_b);
        test_mock::set_value(7_000_000);
        assert_eq!(deposit(supplier_b.as_ptr(), 7_000_000), 0);
        assert_eq!(borrow(supplier_b.as_ptr(), 5_000_000), 0);

        let total_before = load_u64(b"ll_total_deposits");
        test_mock::set_timestamp(1_001_000);
        accrue_interest();
        let depositor_interest = load_u64(b"ll_total_deposits") - total_before;
        assert!(depositor_interest > 0);

        let claim_a = compute_current_deposit(&hex_encode_addr(&supplier_a));
        let claim_b = compute_current_deposit(&hex_encode_addr(&supplier_b));
        let earned_a = claim_a - 3_000_000;
        let earned_b = claim_b - 7_000_000;
        let distributed = earned_a + earned_b;

        assert!(distributed <= depositor_interest);
        assert!(depositor_interest - distributed <= 2);
        assert!((earned_a as u128 * 7).abs_diff(earned_b as u128 * 3) <= 7);
    }

    #[test]
    fn test_missing_supplier_checkpoint_uses_initial_index_for_migration() {
        setup();
        let supplier = [2u8; 32];
        let supplier_hex = hex_encode_addr(&supplier);
        let dep_key = make_key(b"dep:", &supplier_hex);
        let dix_key = make_key(b"dix:", &supplier_hex);
        store_u64(&dep_key, 1_000_000);
        store_u64(b"ll_deposit_index", 1_100_000_000);

        assert_eq!(test_mock::get_storage(&dix_key), None);
        assert_eq!(compute_current_deposit(&supplier_hex), 1_100_000);
        assert_eq!(settle_user_deposit(&supplier_hex), 1_100_000);
        assert_eq!(load_u64(&dix_key), 1_100_000_000);
    }

    #[test]
    fn test_borrow_index_settlement_saturates_on_overflow() {
        setup();
        let borrower = [2u8; 32];
        let hex = hex_encode_addr(&borrower);
        let bor_key = make_key(b"bor:", &hex);
        let bix_key = make_key(b"bix:", &hex);
        store_u64(b"ll_borrow_index", u64::MAX);
        store_u64(&bor_key, u64::MAX);
        store_u64(&bix_key, 1);

        assert_eq!(settle_user_borrow(&hex), u64::MAX);
        assert_eq!(load_u64(&bor_key), u64::MAX);
        assert_eq!(load_u64(&bix_key), u64::MAX);
    }
}
