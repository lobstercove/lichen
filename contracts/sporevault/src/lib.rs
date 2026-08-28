// SporeVault v3 - Custody-backed yield aggregator
// Per whitepaper: auto-compounding vault that optimizes yield across DeFi protocols
// v2: Emergency pause, deposit/withdrawal fees, risk tiers, deposit cap, strategy management

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(dead_code)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;
use alloc::vec::Vec;
use lichen_sdk::{
    balance_of_token_or_native, bytes_to_u64, call_contract, encode_layout_args, get_caller,
    get_contract_address, get_timestamp, get_value, is_native_token, log_info,
    receive_token_or_native, set_return_data, storage_get, storage_set,
    transfer_token_or_native, u64_to_bytes, Address, CrossCall,
};

// ============================================================================
// CONSTANTS
// ============================================================================

/// Performance fee: 10% of yield goes to protocol
const PERFORMANCE_FEE_PERCENT: u64 = 10;
/// Management fee: 2% annualized at the protocol's 400 ms target cadence.
const MANAGEMENT_FEE_BPS: u64 = 200;
const TARGET_SLOT_MILLIS: u64 = 400;
const SECONDS_PER_YEAR: u64 = 365 * 24 * 60 * 60;
const SLOTS_PER_YEAR: u64 = SECONDS_PER_YEAR * 1_000 / TARGET_SLOT_MILLIS;

/// Maximum strategies per vault
const MAX_STRATEGIES: usize = 5;

/// Admin key
const ADMIN_KEY: &[u8] = b"cv_admin";

/// Minimum shares locked permanently on first deposit to prevent
/// ERC-4626 inflation / donation attack (T5.9)
const MIN_LOCKED_SHARES: u64 = 1_000;

/// Storage key for ThallLend protocol address (lending yield source)
const THALLLEND_ADDRESS_KEY: &[u8] = b"cv_thalllend_addr";
/// Storage key for LichenSwap protocol address (LP yield source)
const LICHENSWAP_ADDRESS_KEY: &[u8] = b"cv_lichenswap_addr";
const ACCOUNTING_VERSION_KEY: &[u8] = b"cv_accounting_version";
const ACCOUNTING_VERSION_V2: u64 = 2;
const IDLE_ASSETS_KEY: &[u8] = b"cv_idle_assets";
const LENDING_ASSETS_KEY: &[u8] = b"cv_lending_assets";
const LAST_MANAGEMENT_FEE_SLOT_KEY: &[u8] = b"cv_last_management_fee_slot";
const MANAGEMENT_FEE_REMAINDER_KEY: &[u8] = b"cv_management_fee_remainder";

// ---- V2 constants ----
const CV_PAUSE_KEY: &[u8] = b"cv_paused";
/// Deposit fee in basis points (default: 10 = 0.1%)
const DEFAULT_DEPOSIT_FEE_BPS: u64 = 10;
/// Withdrawal fee in basis points (default: 30 = 0.3%)
const DEFAULT_WITHDRAWAL_FEE_BPS: u64 = 30;
/// Maximum deposit fee (5%)
const MAX_DEPOSIT_FEE_BPS: u64 = 500;
/// Maximum withdrawal fee (5%)
const MAX_WITHDRAWAL_FEE_BPS: u64 = 500;
/// Default deposit cap (0 = unlimited)
const DEFAULT_DEPOSIT_CAP: u64 = 0;
/// Risk tier constants
const RISK_CONSERVATIVE: u8 = 1; // lending-only, ≤33% alloc
const RISK_MODERATE: u8 = 2; // mixed, ≤66% alloc
const RISK_AGGRESSIVE: u8 = 3; // high yield, up to 100%

/// Storage key for LICN token address (used in call_token_transfer)
const LICN_TOKEN_KEY: &[u8] = b"cv_licn_token";

fn is_cv_paused() -> bool {
    storage_get(CV_PAUSE_KEY)
        .map(|d| d.first().copied() == Some(1))
        .unwrap_or(false)
}
fn is_cv_admin(caller: &[u8]) -> bool {
    storage_get(ADMIN_KEY)
        .map(|d| d.as_slice() == caller)
        .unwrap_or(false)
}
fn has_cv_config_entry(key: &[u8]) -> bool {
    storage_get(key).is_some()
}
fn get_deposit_fee_bps() -> u64 {
    storage_get(b"cv_dep_fee")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_DEPOSIT_FEE_BPS)
}
fn get_withdrawal_fee_bps() -> u64 {
    storage_get(b"cv_wd_fee")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_WITHDRAWAL_FEE_BPS)
}
fn get_deposit_cap() -> u64 {
    storage_get(b"cv_dep_cap")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_DEPOSIT_CAP)
}

// Reentrancy guard
const CV_REENTRANCY_KEY: &[u8] = b"cv_reentrancy";

fn reentrancy_enter() -> bool {
    if storage_get(CV_REENTRANCY_KEY)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
    {
        return false;
    }
    storage_set(CV_REENTRANCY_KEY, &[1u8]);
    true
}

fn reentrancy_exit() {
    storage_set(CV_REENTRANCY_KEY, &[0u8]);
}

struct ReentrancyGuard;

impl ReentrancyGuard {
    fn enter() -> Option<Self> {
        if reentrancy_enter() {
            Some(Self)
        } else {
            None
        }
    }
}

impl Drop for ReentrancyGuard {
    fn drop(&mut self) {
        reentrancy_exit();
    }
}

// ============================================================================
// STRATEGY TYPES
// ============================================================================

/// Strategy type identifiers
const STRATEGY_LENDING: u8 = 1; // Deposit into ThallLend
const STRATEGY_LP: u8 = 2; // Provide liquidity on SporeSwap
const STRATEGY_STAKING: u8 = 3; // Stake LICN for validator rewards

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

fn make_key(prefix: &[u8], suffix: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(prefix.len() + suffix.len());
    key.extend_from_slice(prefix);
    key.extend_from_slice(suffix);
    key
}

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key).map(|d| bytes_to_u64(&d)).unwrap_or(0)
}

fn store_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
}

fn read_address32(ptr: *const u8) -> Option<[u8; 32]> {
    if ptr.is_null() {
        return None;
    }
    let mut out = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(ptr, out.as_mut_ptr(), 32);
    }
    Some(out)
}

fn load_licn_token() -> Option<Address> {
    storage_get(LICN_TOKEN_KEY).and_then(|data| {
        if data.len() != 32 {
            return None;
        }
        let mut token = [0u8; 32];
        token.copy_from_slice(&data);
        Some(Address(token))
    })
}

fn load_protocol_address(key: &[u8]) -> Option<Address> {
    storage_get(key).and_then(|data| {
        if data.len() != 32 || data.iter().all(|byte| *byte == 0) {
            return None;
        }
        let mut address = [0u8; 32];
        address.copy_from_slice(&data);
        Some(Address(address))
    })
}

fn accounting_v2_ready() -> bool {
    load_u64(ACCOUNTING_VERSION_KEY) == ACCOUNTING_VERSION_V2
}

fn decode_zero_status(result: &[u8]) -> bool {
    result.len() >= 4 && bytes_to_u64(&result[..4]) == 0
}

fn lending_strategy() -> Result<Option<(usize, u64)>, u32> {
    let count_u64 = load_u64(b"cv_strategy_count");
    if count_u64 > MAX_STRATEGIES as u64 {
        return Err(96);
    }
    let count = count_u64 as usize;
    let mut found = None;
    for index in 0..count {
        let type_key = alloc::format!("cv_strat_type:{}", index);
        let allocation_key = alloc::format!("cv_strat_alloc:{}", index);
        let deployed_key = alloc::format!("cv_strat_deployed:{}", index);
        let strategy_type = load_u64(type_key.as_bytes());
        let allocation = load_u64(allocation_key.as_bytes());
        if strategy_type == STRATEGY_LENDING as u64 {
            if allocation > 100 || found.is_some() {
                return Err(96);
            }
            found = Some((index, allocation));
        } else if strategy_type != 0
            || allocation != 0
            || load_u64(deployed_key.as_bytes()) != 0
        {
            return Err(96);
        }
    }
    Ok(found)
}

fn thalllend_account_claim() -> Option<u64> {
    let thalllend = load_protocol_address(THALLLEND_ADDRESS_KEY)?;
    let vault = get_contract_address();
    let args = encode_layout_args(&[&vault.0]).ok()?;
    match call_contract(CrossCall::new(thalllend, "get_account_info", args)) {
        Ok(result) if result.len() >= 8 => Some(bytes_to_u64(&result[..8])),
        _ => None,
    }
}

fn thalllend_deposit(amount: u64) -> bool {
    if amount == 0 {
        return true;
    }
    let thalllend = match load_protocol_address(THALLLEND_ADDRESS_KEY) {
        Some(address) => address,
        None => return false,
    };
    let token = match load_licn_token() {
        Some(token) => token,
        None => return false,
    };
    let vault = get_contract_address();
    if !is_native_token(&token) {
        let amount_bytes = amount.to_le_bytes();
        let approve_args = match encode_layout_args(&[&vault.0, &thalllend.0, &amount_bytes]) {
            Ok(args) => args,
            Err(_) => return false,
        };
        match call_contract(CrossCall::new(token, "approve", approve_args)) {
            Ok(result) if decode_zero_status(&result) => {}
            _ => return false,
        }
    }

    let amount_bytes = amount.to_le_bytes();
    let args = match encode_layout_args(&[&vault.0, &amount_bytes]) {
        Ok(args) => args,
        Err(_) => return false,
    };
    let call = CrossCall::new(thalllend, "deposit", args);
    let call = if is_native_token(&token) {
        call.with_value(amount)
    } else {
        call
    };
    matches!(call_contract(call), Ok(result) if decode_zero_status(&result))
}

fn thalllend_withdraw(amount: u64) -> bool {
    if amount == 0 {
        return true;
    }
    let thalllend = match load_protocol_address(THALLLEND_ADDRESS_KEY) {
        Some(address) => address,
        None => return false,
    };
    let vault = get_contract_address();
    let amount_bytes = amount.to_le_bytes();
    let args = match encode_layout_args(&[&vault.0, &amount_bytes]) {
        Ok(args) => args,
        Err(_) => return false,
    };
    matches!(
        call_contract(CrossCall::new(thalllend, "withdraw", args)),
        Ok(result) if decode_zero_status(&result)
    )
}

fn set_lending_strategy_deployed(amount: u64) -> Result<(), u32> {
    if let Some((index, _)) = lending_strategy()? {
        let deployed_key = alloc::format!("cv_strat_deployed:{}", index);
        store_u64(deployed_key.as_bytes(), amount);
    }
    Ok(())
}

fn store_vault_assets_from_components() -> Result<u64, u32> {
    let total = load_u64(IDLE_ASSETS_KEY)
        .checked_add(load_u64(LENDING_ASSETS_KEY))
        .ok_or(91u32)?;
    store_u64(b"cv_total_assets", total);
    Ok(total)
}

/// Accrue the published 2% annual management fee against real vault assets.
/// The runtime timestamp is the deterministic slot, so the denominator uses
/// the protocol's 400 ms target cadence. A carried numerator remainder
/// preserves sub-spore accrual so frequent keeper calls cannot round the fee
/// permanently to zero. Fees are moved out of depositor accounting into liquid
/// protocol custody; deployed ThallLend assets are recalled only when idle
/// assets are insufficient.
fn accrue_management_fee() -> Result<u64, u32> {
    if !accounting_v2_ready() {
        return Err(90);
    }
    let now = get_timestamp();
    let last = load_u64(LAST_MANAGEMENT_FEE_SLOT_KEY);
    if last == 0 {
        store_u64(LAST_MANAGEMENT_FEE_SLOT_KEY, now);
        store_u64(MANAGEMENT_FEE_REMAINDER_KEY, 0);
        return Ok(0);
    }
    if now <= last {
        return Ok(0);
    }

    let total_assets = store_vault_assets_from_components()?;
    if total_assets == 0 {
        store_u64(LAST_MANAGEMENT_FEE_SLOT_KEY, now);
        store_u64(MANAGEMENT_FEE_REMAINDER_KEY, 0);
        return Ok(0);
    }
    let denominator = (10_000u128)
        .checked_mul(SLOTS_PER_YEAR as u128)
        .ok_or(91u32)?;
    let numerator = (total_assets as u128)
        .checked_mul(MANAGEMENT_FEE_BPS as u128)
        .and_then(|value| value.checked_mul(now.saturating_sub(last) as u128))
        .and_then(|value| {
            value.checked_add(load_u64(MANAGEMENT_FEE_REMAINDER_KEY) as u128)
        })
        .ok_or(91u32)?;
    let fee_wide = numerator / denominator;
    let remainder = numerator % denominator;
    let fee = u64::try_from(fee_wide).map_err(|_| 91u32)?;
    let remainder = u64::try_from(remainder).map_err(|_| 91u32)?;
    if fee == 0 {
        store_u64(LAST_MANAGEMENT_FEE_SLOT_KEY, now);
        store_u64(MANAGEMENT_FEE_REMAINDER_KEY, remainder);
        return Ok(0);
    }
    if fee > total_assets {
        return Err(91);
    }

    let idle_before = load_u64(IDLE_ASSETS_KEY);
    let lending_before = load_u64(LENDING_ASSETS_KEY);
    let idle_fee = idle_before.min(fee);
    let lending_fee = fee - idle_fee;
    let protocol_fees = load_u64(b"cv_protocol_fees")
        .checked_add(fee)
        .ok_or(91u32)?;
    let fees_earned = load_u64(b"cv_fees_earned")
        .checked_add(fee)
        .ok_or(91u32)?;
    let lending_after = if lending_fee > 0 {
        if lending_fee > lending_before || !thalllend_withdraw(lending_fee) {
            return Err(94);
        }
        match thalllend_account_claim() {
            Some(actual) if actual == lending_before - lending_fee => actual,
            _ => return Err(95),
        }
    } else {
        lending_before
    };

    store_u64(IDLE_ASSETS_KEY, idle_before - idle_fee);
    store_u64(LENDING_ASSETS_KEY, lending_after);
    set_lending_strategy_deployed(lending_after)?;
    store_u64(b"cv_total_assets", total_assets - fee);
    store_u64(b"cv_protocol_fees", protocol_fees);
    store_u64(b"cv_fees_earned", fees_earned);
    store_u64(LAST_MANAGEMENT_FEE_SLOT_KEY, now);
    store_u64(MANAGEMENT_FEE_REMAINDER_KEY, remainder);
    Ok(fee)
}

/// Synchronize the real ThallLend claim and realize the performance fee into
/// liquid custody. No formula quote is ever booked as an asset.
fn collect_lending_yield() -> Result<u64, u32> {
    if !accounting_v2_ready() {
        log_info("Vault accounting migration is required");
        return Err(90);
    }
    let Some((_index, _allocation)) = lending_strategy()? else {
        if load_u64(LENDING_ASSETS_KEY) != 0 {
            log_info("Lending assets exist without a configured strategy");
            return Err(92);
        }
        store_vault_assets_from_components()?;
        accrue_management_fee()?;
        return Ok(0);
    };

    let actual_claim = thalllend_account_claim().ok_or(93u32)?;
    let cached_claim = load_u64(LENDING_ASSETS_KEY);
    let mut final_claim = actual_claim;
    let mut net_yield = 0u64;

    if actual_claim > cached_claim {
        let gross_yield = actual_claim - cached_claim;
        let performance_fee = ((gross_yield as u128) * PERFORMANCE_FEE_PERCENT as u128 / 100)
            .min(u64::MAX as u128) as u64;
        net_yield = gross_yield - performance_fee;
        let protocol_fees = load_u64(b"cv_protocol_fees")
            .checked_add(performance_fee)
            .ok_or(91u32)?;
        let fees_earned = load_u64(b"cv_fees_earned")
            .checked_add(performance_fee)
            .ok_or(91u32)?;
        let total_earned = load_u64(b"cv_total_earned")
            .checked_add(net_yield)
            .ok_or(91u32)?;

        if performance_fee > 0 {
            if !thalllend_withdraw(performance_fee) {
                return Err(94);
            }
            final_claim = thalllend_account_claim().ok_or(93u32)?;
            if final_claim != actual_claim - performance_fee {
                log_info("ThallLend performance-fee withdrawal did not settle exactly");
                return Err(95);
            }
            store_u64(b"cv_protocol_fees", protocol_fees);
            store_u64(b"cv_fees_earned", fees_earned);
        }

        store_u64(b"cv_total_earned", total_earned);
    }

    store_u64(LENDING_ASSETS_KEY, final_claim);
    set_lending_strategy_deployed(final_claim)?;
    store_vault_assets_from_components()?;
    accrue_management_fee()?;
    Ok(net_yield)
}

fn rebalance_internal() -> Result<(), u32> {
    collect_lending_yield()?;
    let Some((_index, allocation)) = lending_strategy()? else {
        return Ok(());
    };
    if allocation > 100 {
        return Err(96);
    }

    let total_assets = load_u64(b"cv_total_assets");
    let target = ((total_assets as u128) * allocation as u128 / 100) as u64;
    let current = load_u64(LENDING_ASSETS_KEY);
    if target > current {
        let amount = target - current;
        let idle = load_u64(IDLE_ASSETS_KEY);
        if amount > idle || !thalllend_deposit(amount) {
            return Err(97);
        }
        let actual = thalllend_account_claim().ok_or(93u32)?;
        if actual != current.checked_add(amount).ok_or(91u32)? {
            log_info("ThallLend deposit did not settle exactly");
            return Err(95);
        }
        store_u64(IDLE_ASSETS_KEY, idle - amount);
        store_u64(LENDING_ASSETS_KEY, actual);
    } else if current > target {
        let amount = current - target;
        let idle = load_u64(IDLE_ASSETS_KEY).checked_add(amount).ok_or(91u32)?;
        if !thalllend_withdraw(amount) {
            return Err(94);
        }
        let actual = thalllend_account_claim().ok_or(93u32)?;
        if actual != target {
            log_info("ThallLend withdrawal did not settle exactly");
            return Err(95);
        }
        store_u64(IDLE_ASSETS_KEY, idle);
        store_u64(LENDING_ASSETS_KEY, actual);
    }
    set_lending_strategy_deployed(load_u64(LENDING_ASSETS_KEY))?;
    store_vault_assets_from_components()?;
    Ok(())
}

// ============================================================================
// VAULT STATE
// ============================================================================

/// Initialize the vault
#[no_mangle]
pub extern "C" fn initialize(admin_ptr: *const u8) -> u32 {
    let admin = match read_address32(admin_ptr) {
        Some(addr) => addr,
        None => return 98,
    };

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
    store_u64(b"cv_total_shares", 0);
    store_u64(b"cv_total_assets", 0);
    store_u64(b"cv_strategy_count", 0);
    store_u64(b"cv_last_harvest", get_timestamp());
    store_u64(b"cv_total_earned", 0);
    store_u64(IDLE_ASSETS_KEY, 0);
    store_u64(LENDING_ASSETS_KEY, 0);
    store_u64(ACCOUNTING_VERSION_KEY, ACCOUNTING_VERSION_V2);
    store_u64(LAST_MANAGEMENT_FEE_SLOT_KEY, get_timestamp());
    store_u64(MANAGEMENT_FEE_REMAINDER_KEY, 0);
    store_u64(b"cv_risk_tier", RISK_CONSERVATIVE as u64);
    store_u64(b"cv_dep_fee", DEFAULT_DEPOSIT_FEE_BPS);
    store_u64(b"cv_wd_fee", DEFAULT_WITHDRAWAL_FEE_BPS);
    store_u64(b"cv_dep_cap", DEFAULT_DEPOSIT_CAP);
    store_u64(b"cv_protocol_fees", 0);
    store_u64(b"cv_fees_earned", 0);
    storage_set(CV_PAUSE_KEY, &[0]);
    // Native LICN is the safe default. A fresh empty vault may switch once to
    // an MT-20 LICN representation before accepting any assets.
    storage_set(LICN_TOKEN_KEY, &[0u8; 32]);

    log_info("SporeVault initialized");
    0
}

// ============================================================================
// STRATEGY MANAGEMENT (admin only)
// ============================================================================

/// Add a yield strategy
/// strategy_type: 1=lending, 2=lp, 3=staking
/// allocation_percent: portion of vault funds allocated (0-100)
#[no_mangle]
pub extern "C" fn add_strategy(
    caller_ptr: *const u8,
    strategy_type: u8,
    allocation_percent: u64,
) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(addr) => addr,
        None => return 98,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    let admin = match storage_get(ADMIN_KEY) {
        Some(a) => a,
        None => return 1,
    };
    if caller[..] != admin[..] {
        log_info("Unauthorized");
        return 2;
    }

    if strategy_type != STRATEGY_LENDING {
        log_info("Strategy adapter is not implemented; only ThallLend is supported");
        return 6;
    }
    if allocation_percent == 0 {
        log_info("Strategy allocation must be non-zero");
        return 3;
    }
    let risk_tier = load_u64(b"cv_risk_tier") as u8;
    let max_allocation = match risk_tier {
        RISK_CONSERVATIVE => 33,
        RISK_MODERATE => 66,
        RISK_AGGRESSIVE => 100,
        _ => return 7,
    };
    if allocation_percent > max_allocation {
        log_info("Strategy allocation exceeds the active risk tier");
        return 7;
    }

    let count_u64 = load_u64(b"cv_strategy_count");
    if count_u64 > MAX_STRATEGIES as u64 {
        log_info("Invalid strategy registry state");
        return 9;
    }
    if lending_strategy().is_err() {
        log_info("Invalid strategy registry state");
        return 9;
    }
    let count = count_u64 as usize;
    let target_index = (0..count)
        .find(|index| {
            let type_key = alloc::format!("cv_strat_type:{}", index);
            load_u64(type_key.as_bytes()) == 0
        })
        .unwrap_or(count);
    if target_index == count && count >= MAX_STRATEGIES {
        log_info("Max strategies reached");
        return 4;
    }
    for index in 0..count {
        let type_key = alloc::format!("cv_strat_type:{}", index);
        if load_u64(type_key.as_bytes()) as u8 == STRATEGY_LENDING {
            log_info("Only one active ThallLend strategy is supported");
            return 8;
        }
    }

    // Check total allocation doesn't exceed 100%
    let mut total_alloc = allocation_percent;
    for i in 0..count {
        let alloc_key = alloc::format!("cv_strat_alloc:{}", i);
        total_alloc = match total_alloc.checked_add(load_u64(alloc_key.as_bytes())) {
            Some(total) => total,
            None => return 5,
        };
    }
    if total_alloc > 100 {
        log_info("Total allocation exceeds 100%");
        return 5;
    }

    // Store strategy
    let type_key = alloc::format!("cv_strat_type:{}", target_index);
    let alloc_key = alloc::format!("cv_strat_alloc:{}", target_index);
    let deployed_key = alloc::format!("cv_strat_deployed:{}", target_index);

    store_u64(type_key.as_bytes(), strategy_type as u64);
    store_u64(alloc_key.as_bytes(), allocation_percent);
    store_u64(deployed_key.as_bytes(), 0);
    if target_index == count {
        store_u64(b"cv_strategy_count", (count + 1) as u64);
    }

    log_info("Strategy added");
    0
}

// ============================================================================
// DEPOSIT / WITHDRAW (ERC-4626 style vault shares)
// ============================================================================

/// Deposit LICN into the vault, receive shares
/// Returns shares minted (0 on failure)
#[no_mangle]
pub extern "C" fn deposit(depositor_ptr: *const u8, amount: u64) -> u64 {
    if amount == 0 {
        return 0;
    }
    if is_cv_paused() {
        log_info("Vault is paused");
        return 0;
    }
    if !accounting_v2_ready() {
        log_info("Vault accounting migration is required");
        return 0;
    }
    let _guard = match ReentrancyGuard::enter() {
        Some(guard) => guard,
        None => return 0,
    };

    let depositor = match read_address32(depositor_ptr) {
        Some(addr) => addr,
        None => return 0,
    };

    let real_caller = get_caller();
    if real_caller.0 != depositor {
        return 200;
    }

    if collect_lending_yield().is_err() {
        return 0;
    }

    let payment_token = match load_licn_token() {
        Some(token) => token,
        None => {
            log_info("LICN asset is not configured");
            return 0;
        }
    };
    let attached_value = get_value();
    if (is_native_token(&payment_token) && attached_value != amount)
        || (!is_native_token(&payment_token) && attached_value != 0)
    {
        log_info("Deposit attached value does not exactly match the configured asset");
        return 0;
    }
    // V2: Deposit cap check
    let cap = get_deposit_cap();
    if cap > 0 {
        let total_assets = load_u64(b"cv_total_assets");
        // AUDIT-FIX L6-01: Overflow-safe cap check
        if amount > cap.saturating_sub(total_assets) {
            log_info("Deposit cap exceeded");
            return 0;
        }
    }

    // V2: Deposit fee
    let fee_bps = get_deposit_fee_bps();
    if fee_bps > MAX_DEPOSIT_FEE_BPS {
        return 0;
    }
    let fee = ((amount as u128) * (fee_bps as u128) / 10_000) as u64;
    let net_amount = match amount.checked_sub(fee) {
        Some(net) => net,
        None => return 0,
    };
    if net_amount == 0 {
        return 0;
    }
    let prev_protocol_fees = load_u64(b"cv_protocol_fees");
    let new_protocol_fees = match prev_protocol_fees.checked_add(fee) {
        Some(total) => total,
        None => return 0,
    };
    let prev_fees_earned = load_u64(b"cv_fees_earned");
    let new_fees_earned = match prev_fees_earned.checked_add(fee) {
        Some(total) => total,
        None => return 0,
    };

    let hex = hex_encode_addr(&depositor);

    let total_shares = load_u64(b"cv_total_shares");
    let total_assets = load_u64(b"cv_total_assets");
    if (total_shares == 0) != (total_assets == 0) {
        log_info("Vault share and asset bootstrap state is inconsistent");
        return 0;
    }
    let is_first_deposit = total_shares == 0;

    // Calculate shares to mint (first depositor gets 1:1)
    let shares = if is_first_deposit {
        // T5.9: On first deposit, lock MIN_LOCKED_SHARES to a dead address
        if net_amount <= MIN_LOCKED_SHARES {
            log_info("First deposit must exceed minimum locked shares");
            return 0;
        }
        net_amount - MIN_LOCKED_SHARES
    } else {
        // Use u128 to prevent overflow on large values
        ((net_amount as u128) * (total_shares as u128) / (total_assets as u128)) as u64
    };

    if shares == 0 {
        log_info("Deposit too small");
        return 0;
    }

    // Update user shares
    let share_key = make_key(b"cv_shares:", &hex);
    let prev_shares = load_u64(&share_key);
    let new_user_shares = match prev_shares.checked_add(shares) {
        Some(total) => total,
        None => return 0,
    };

    let base_total_shares = if is_first_deposit {
        MIN_LOCKED_SHARES
    } else {
        total_shares
    };
    let base_total_assets = if is_first_deposit {
        MIN_LOCKED_SHARES
    } else {
        total_assets
    };
    let new_total_shares = match base_total_shares.checked_add(shares) {
        Some(total) => total,
        None => return 0,
    };
    let additional_assets = if is_first_deposit {
        net_amount - MIN_LOCKED_SHARES
    } else {
        net_amount
    };
    let new_total_assets = match base_total_assets.checked_add(additional_assets) {
        Some(total) => total,
        None => return 0,
    };
    let new_idle_assets = match load_u64(IDLE_ASSETS_KEY).checked_add(net_amount) {
        Some(total) => total,
        None => return 0,
    };

    // Execute custody movement only after every deterministic rejection and
    // arithmetic check has passed. The transaction processor is atomic, but
    // this ordering also keeps nested-call behavior and standalone execution
    // fail-closed without relying on a late rollback.
    if !receive_token_or_native(
        payment_token,
        Address(depositor),
        get_contract_address(),
        amount,
    )
    .unwrap_or(false)
    {
        return 0;
    }

    if is_first_deposit {
        let dead_hex = [b'0'; 64];
        let dead_key = make_key(b"cv_shares:", &dead_hex);
        store_u64(&dead_key, MIN_LOCKED_SHARES);
    }
    store_u64(&share_key, new_user_shares);
    store_u64(b"cv_total_shares", new_total_shares);
    store_u64(b"cv_total_assets", new_total_assets);
    store_u64(IDLE_ASSETS_KEY, new_idle_assets);
    if fee > 0 {
        store_u64(b"cv_protocol_fees", new_protocol_fees);
        store_u64(b"cv_fees_earned", new_fees_earned);
    }

    log_info("Vault deposit successful");
    shares
}

/// Withdraw from vault by burning shares
/// Returns LICN amount withdrawn (0 on failure)
#[no_mangle]
pub extern "C" fn withdraw(depositor_ptr: *const u8, shares_to_burn: u64) -> u64 {
    if shares_to_burn == 0 {
        return 0;
    }
    if !accounting_v2_ready() {
        log_info("Vault accounting migration is required");
        return 0;
    }
    let _guard = match ReentrancyGuard::enter() {
        Some(guard) => guard,
        None => return 0,
    };

    let depositor = match read_address32(depositor_ptr) {
        Some(addr) => addr,
        None => return 0,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != depositor {
        return 200;
    }

    if collect_lending_yield().is_err() {
        return 0;
    }

    let hex = hex_encode_addr(&depositor);

    let share_key = make_key(b"cv_shares:", &hex);
    let user_shares = load_u64(&share_key);
    if shares_to_burn > user_shares {
        log_info("Insufficient shares");
        return 0;
    }

    let total_shares = load_u64(b"cv_total_shares");
    let total_assets = load_u64(b"cv_total_assets");
    if total_shares == 0 || shares_to_burn > total_shares {
        log_info("Invalid total share accounting");
        return 0;
    }

    // Calculate LICN to return
    // Use u128 to prevent overflow on large values
    let gross_amount =
        ((shares_to_burn as u128) * (total_assets as u128) / (total_shares as u128)) as u64;
    if gross_amount == 0 {
        return 0;
    }

    // V2: Withdrawal fee
    let fee_bps = get_withdrawal_fee_bps();
    if fee_bps > MAX_WITHDRAWAL_FEE_BPS {
        return 0;
    }
    let fee = ((gross_amount as u128) * (fee_bps as u128) / 10_000) as u64;
    let amount = match gross_amount.checked_sub(fee) {
        Some(amount) => amount,
        None => return 0,
    };
    let total_assets_after = match total_assets.checked_sub(gross_amount) {
        Some(total) => total,
        None => return 0,
    };
    let prev_protocol_fees = load_u64(b"cv_protocol_fees");
    let prev_fees_earned = load_u64(b"cv_fees_earned");
    let new_protocol_fees = match prev_protocol_fees.checked_add(fee) {
        Some(total) => total,
        None => return 0,
    };
    let new_fees_earned = match prev_fees_earned.checked_add(fee) {
        Some(total) => total,
        None => return 0,
    };

    let idle_before = load_u64(IDLE_ASSETS_KEY);
    let lending_before = load_u64(LENDING_ASSETS_KEY);
    let mut idle_after_recall = idle_before;
    let mut lending_after_recall = lending_before;
    if idle_before < gross_amount {
        let shortfall = gross_amount - idle_before;
        if shortfall > lending_before || !thalllend_withdraw(shortfall) {
            log_info("Unable to recall enough deployed liquidity");
            return 0;
        }
        let actual_lending = match thalllend_account_claim() {
            Some(claim) if claim == lending_before - shortfall => claim,
            _ => {
                log_info("ThallLend withdrawal did not settle exactly");
                return 0;
            }
        };
        idle_after_recall = match idle_before.checked_add(shortfall) {
            Some(idle) => idle,
            None => return 0,
        };
        lending_after_recall = actual_lending;
        store_u64(IDLE_ASSETS_KEY, idle_after_recall);
        store_u64(LENDING_ASSETS_KEY, lending_after_recall);
        if set_lending_strategy_deployed(actual_lending).is_err() {
            return 0;
        }
    }

    let available_idle = idle_after_recall;
    if available_idle < gross_amount {
        return 0;
    }

    if fee > 0 {
        store_u64(b"cv_protocol_fees", new_protocol_fees);
        store_u64(b"cv_fees_earned", new_fees_earned);
    }

    // Update user shares
    store_u64(&share_key, user_shares - shares_to_burn);

    // Update totals
    store_u64(b"cv_total_shares", total_shares - shares_to_burn);
    store_u64(b"cv_total_assets", total_assets_after);
    store_u64(IDLE_ASSETS_KEY, available_idle - gross_amount);

    // G25-02: Transfer LICN to depositor
    if !transfer_licn_out(&depositor, amount) {
        // Revert all state changes
        store_u64(&share_key, user_shares);
        store_u64(b"cv_total_shares", total_shares);
        store_u64(b"cv_total_assets", total_assets);
        // A strategy recall may already have moved custody before the outbound
        // transfer failed. Keep the exact post-recall components instead of
        // pretending the funds remain deployed. Top-level ABI failure is also
        // transaction-atomic, making this safe for both execution modes.
        store_u64(IDLE_ASSETS_KEY, idle_after_recall);
        store_u64(LENDING_ASSETS_KEY, lending_after_recall);
        let _ = set_lending_strategy_deployed(lending_after_recall);
        if fee > 0 {
            store_u64(b"cv_protocol_fees", prev_protocol_fees);
            store_u64(b"cv_fees_earned", prev_fees_earned);
        }
        log_info("Withdrawal transfer failed");
        return 0;
    }

    log_info("Vault withdrawal successful");
    amount
}

/// G25-02: Transfer LICN tokens out of the vault to a recipient.
/// Uses self-custody pattern: vault holds tokens at its own contract address.
/// Returns true on success, false if token not configured or transfer fails.
fn transfer_licn_out(to: &[u8; 32], amount: u64) -> bool {
    if amount == 0 {
        return true;
    }
    let token = match load_licn_token() {
        Some(token) => token,
        None => {
            log_info("LICN token not configured - transfer rejected");
            return false;
        }
    };
    let contract_addr = get_contract_address();
    match transfer_token_or_native(token, Address(contract_addr.0), Address(*to), amount) {
        Ok(true) => true,
        Ok(false) => {
            log_info("LICN transfer returned failure status");
            false
        }
        Err(_) => {
            log_info("LICN transfer failed");
            false
        }
    }
}

/// Set protocol addresses for real yield sources. Admin only.
/// Both addresses optional (pass zero to skip). Non-zero addresses are stored.
///
/// Returns: 0 success, 1 not admin, 2 ThallLend already configured, 3 LichenSwap already configured
#[no_mangle]
pub extern "C" fn set_protocol_addresses(
    caller_ptr: *const u8,
    thalllend_ptr: *const u8,
    lichenswap_ptr: *const u8,
) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(addr) => addr,
        None => return 98,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cv_admin(&caller) {
        return 1;
    }

    let thalllend = match read_address32(thalllend_ptr) {
        Some(addr) => addr,
        None => return 98,
    };
    let lichenswap = match read_address32(lichenswap_ptr) {
        Some(addr) => addr,
        None => return 98,
    };

    let set_thalllend = thalllend.iter().any(|&b| b != 0);
    let set_lichenswap = lichenswap.iter().any(|&b| b != 0);

    if set_lichenswap {
        log_info("LichenSwap LP adapter is not available for this single-asset vault");
        return 4;
    }

    if set_thalllend && has_cv_config_entry(THALLLEND_ADDRESS_KEY) {
        log_info("ThallLend address already configured");
        return 2;
    }

    if set_thalllend {
        storage_set(THALLLEND_ADDRESS_KEY, &thalllend);
        log_info("ThallLend address configured");
    }
    0
}

/// G25-02: Set LICN token address for self-custody transfers. Admin only.
/// Returns: 0 success, 1 not admin, 2 already configured
#[no_mangle]
pub extern "C" fn set_licn_token(caller_ptr: *const u8, token_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(addr) => addr,
        None => return 98,
    };
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }
    if !is_cv_admin(&caller) {
        return 1;
    }
    let token = match read_address32(token_ptr) {
        Some(addr) => addr,
        None => return 98,
    };
    if let Some(current_data) = storage_get(LICN_TOKEN_KEY) {
        if current_data.len() != 32 {
            log_info("Existing LICN token configuration is malformed");
            return 3;
        }
        let mut current_bytes = [0u8; 32];
        current_bytes.copy_from_slice(&current_data);
        let current = Address(current_bytes);
        let may_replace_fresh_native = is_native_token(&current)
            && load_u64(b"cv_total_assets") == 0
            && load_u64(b"cv_total_shares") == 0
            && load_u64(b"cv_protocol_fees") == 0
            && load_u64(b"cv_strategy_count") == 0
            && token.iter().any(|byte| *byte != 0);
        if !may_replace_fresh_native {
            log_info("LICN token already configured");
            return 2;
        }
    }
    storage_set(LICN_TOKEN_KEY, &token);
    log_info("LICN token address configured");
    0
}

/// Retire one exact legacy strategy row before activating accounting v2.
/// This is intentionally unavailable after migration and requires the vault to
/// be paused. Expected values bind the write to an independently captured
/// source row so a stale or edited migration plan aborts without mutation.
#[no_mangle]
pub extern "C" fn retire_legacy_strategy(
    caller_ptr: *const u8,
    index: u64,
    expected_type: u8,
    expected_allocation: u64,
    expected_deployed: u64,
) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(address) => address,
        None => return 98,
    };
    if get_caller().0 != caller {
        return 200;
    }
    if !is_cv_admin(&caller) {
        return 1;
    }
    if accounting_v2_ready() {
        return 2;
    }
    if !is_cv_paused() {
        return 3;
    }
    let count = load_u64(b"cv_strategy_count");
    if count > MAX_STRATEGIES as u64 || index >= count {
        return 4;
    }
    let type_key = alloc::format!("cv_strat_type:{}", index);
    let allocation_key = alloc::format!("cv_strat_alloc:{}", index);
    let deployed_key = alloc::format!("cv_strat_deployed:{}", index);
        let actual_type = load_u64(type_key.as_bytes());
    let actual_allocation = load_u64(allocation_key.as_bytes());
    let actual_deployed = load_u64(deployed_key.as_bytes());
    if actual_type != expected_type as u64
        || actual_allocation != expected_allocation
        || actual_deployed != expected_deployed
    {
        return 5;
    }
    if actual_type == 0 {
        return 6;
    }
    if actual_type == STRATEGY_LENDING as u64 {
        let lending_count = (0..count)
            .filter(|candidate| {
                let key = alloc::format!("cv_strat_type:{}", candidate);
                load_u64(key.as_bytes()) == STRATEGY_LENDING as u64
            })
            .count();
        if lending_count <= 1 {
            log_info("The final lending strategy cannot be retired before custody migration");
            return 7;
        }
    }

    store_u64(type_key.as_bytes(), 0);
    store_u64(allocation_key.as_bytes(), 0);
    store_u64(deployed_key.as_bytes(), 0);
    log_info("Exact legacy strategy row retired for accounting v2 migration");
    0
}

/// One-time migration for pre-v2 vaults. The caller supplies independently
/// audited expected balances; the contract verifies them against real token
/// custody and the real ThallLend account claim before activating v2.
#[no_mangle]
pub extern "C" fn migrate_accounting_v2(
    caller_ptr: *const u8,
    expected_idle_assets: u64,
    expected_lending_assets: u64,
) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(address) => address,
        None => return 98,
    };
    if get_caller().0 != caller {
        return 200;
    }
    if !is_cv_admin(&caller) {
        return 1;
    }
    if accounting_v2_ready() {
        return 2;
    }
    if !is_cv_paused() {
        log_info("Vault must be paused before accounting migration");
        return 10;
    }

    let count_u64 = load_u64(b"cv_strategy_count");
    if count_u64 > MAX_STRATEGIES as u64 {
        return 3;
    }
    let count = count_u64 as usize;
    let mut lending_count = 0usize;
    for index in 0..count {
        let type_key = alloc::format!("cv_strat_type:{}", index);
        let allocation_key = alloc::format!("cv_strat_alloc:{}", index);
        let deployed_key = alloc::format!("cv_strat_deployed:{}", index);
        let strategy_type = load_u64(type_key.as_bytes());
        let allocation = load_u64(allocation_key.as_bytes());
        let deployed = load_u64(deployed_key.as_bytes());
        if strategy_type == STRATEGY_LENDING as u64 {
            if allocation > 100 {
                return 3;
            }
            lending_count += 1;
        } else if strategy_type != 0 || allocation > 0 || deployed > 0 {
            log_info("Legacy strategy row must be retired exactly before migration");
            return 3;
        }
    }
    if lending_count > 1 {
        return 3;
    }

    let token = match load_licn_token() {
        Some(token) => token,
        None => return 4,
    };
    let custody = match balance_of_token_or_native(token, get_contract_address()) {
        Ok(balance) => balance,
        Err(_) => return 5,
    };
    let protocol_fees = load_u64(b"cv_protocol_fees");
    let actual_idle = match custody.checked_sub(protocol_fees) {
        Some(balance) => balance,
        None => return 6,
    };
    let actual_lending = if lending_count == 1 {
        match thalllend_account_claim() {
            Some(claim) => claim,
            None => return 7,
        }
    } else {
        0
    };
    if actual_idle != expected_idle_assets || actual_lending != expected_lending_assets {
        log_info("Vault migration balances do not match audited expectations");
        return 8;
    }
    let total_assets = match actual_idle.checked_add(actual_lending) {
        Some(total) => total,
        None => return 9,
    };
    let total_shares = load_u64(b"cv_total_shares");
    if (total_assets == 0) != (total_shares == 0) {
        log_info("Real migrated assets and legacy shares are inconsistent");
        return 11;
    }

    store_u64(IDLE_ASSETS_KEY, actual_idle);
    store_u64(LENDING_ASSETS_KEY, actual_lending);
    store_u64(b"cv_total_assets", total_assets);
    if set_lending_strategy_deployed(actual_lending).is_err() {
        return 3;
    }
    store_u64(ACCOUNTING_VERSION_KEY, ACCOUNTING_VERSION_V2);
    store_u64(b"cv_last_harvest", get_timestamp());
    store_u64(LAST_MANAGEMENT_FEE_SLOT_KEY, get_timestamp());
    store_u64(MANAGEMENT_FEE_REMAINDER_KEY, 0);
    log_info("SporeVault accounting v2 migration activated");
    0
}

// ============================================================================
// HARVEST & AUTO-COMPOUND
// ============================================================================

/// Harvest yield from all strategies and auto-compound
/// Can be called by anyone (typically a cron job or keeper)
#[no_mangle]
pub extern "C" fn harvest() -> u32 {
    if is_cv_paused() {
        log_info("Vault is paused");
        return 2;
    }
    let _guard = match ReentrancyGuard::enter() {
        Some(guard) => guard,
        None => return 1,
    };
    let last_harvest = load_u64(b"cv_last_harvest");
    let now = get_timestamp();
    if now <= last_harvest {
        return 0; // Nothing to harvest
    }

    if let Err(code) = rebalance_internal() {
        log_info("Harvest failed to synchronize real strategy custody");
        return code;
    }

    store_u64(b"cv_last_harvest", now);
    log_info("Harvest synchronized and rebalanced real vault assets");
    0
}

/// Permissionless deterministic rebalance to the configured ThallLend target.
#[no_mangle]
pub extern "C" fn rebalance() -> u32 {
    if is_cv_paused() {
        log_info("Vault is paused");
        return 2;
    }
    let _guard = match ReentrancyGuard::enter() {
        Some(guard) => guard,
        None => return 1,
    };
    match rebalance_internal() {
        Ok(()) => 0,
        Err(code) => code,
    }
}

// ============================================================================
// VIEW FUNCTIONS
// ============================================================================

/// Get vault stats: [total_assets(8), total_shares(8), share_price(8),
///                    strategy_count(8), total_earned(8), fees_earned(8)]
#[no_mangle]
pub extern "C" fn get_vault_stats() -> u32 {
    let total_assets = load_u64(b"cv_total_assets");
    let total_shares = load_u64(b"cv_total_shares");
    let share_price = if total_shares > 0 {
        // Use u128 to prevent overflow
        ((total_assets as u128) * 1_000_000_000 / (total_shares as u128)) as u64
    } else {
        1_000_000_000 // 1:1 initially
    };
    let strategy_count = load_u64(b"cv_strategy_count");
    let total_earned = load_u64(b"cv_total_earned");
    let fees_earned = load_u64(b"cv_fees_earned");

    let mut result = Vec::with_capacity(48);
    result.extend_from_slice(&u64_to_bytes(total_assets));
    result.extend_from_slice(&u64_to_bytes(total_shares));
    result.extend_from_slice(&u64_to_bytes(share_price));
    result.extend_from_slice(&u64_to_bytes(strategy_count));
    result.extend_from_slice(&u64_to_bytes(total_earned));
    result.extend_from_slice(&u64_to_bytes(fees_earned));
    set_return_data(&result);
    0
}

/// Get user position: [shares(8), estimated_value(8)]
#[no_mangle]
pub extern "C" fn get_user_position(user_ptr: *const u8) -> u32 {
    let user = match read_address32(user_ptr) {
        Some(addr) => addr,
        None => return 98,
    };
    let hex = hex_encode_addr(&user);

    let share_key = make_key(b"cv_shares:", &hex);
    let shares = load_u64(&share_key);

    let total_shares = load_u64(b"cv_total_shares");
    let total_assets = load_u64(b"cv_total_assets");

    let estimated_value = if total_shares > 0 {
        // AUDIT-FIX: Use u128 to prevent overflow
        ((shares as u128) * (total_assets as u128) / (total_shares as u128)) as u64
    } else {
        0
    };

    let mut result = Vec::with_capacity(16);
    result.extend_from_slice(&u64_to_bytes(shares));
    result.extend_from_slice(&u64_to_bytes(estimated_value));
    set_return_data(&result);
    0
}

/// Get strategy info: [type(8), allocation_percent(8), deployed_amount(8)]
#[no_mangle]
pub extern "C" fn get_strategy_info(index: u64) -> u32 {
    let count = load_u64(b"cv_strategy_count");
    if count > MAX_STRATEGIES as u64 || index >= count {
        return 1;
    }

    let i = index as usize;
    let type_key = alloc::format!("cv_strat_type:{}", i);
    let alloc_key = alloc::format!("cv_strat_alloc:{}", i);
    let deployed_key = alloc::format!("cv_strat_deployed:{}", i);

    let strategy_type = load_u64(type_key.as_bytes());
    let allocation = load_u64(alloc_key.as_bytes());
    let deployed = load_u64(deployed_key.as_bytes());

    let mut result = Vec::with_capacity(24);
    result.extend_from_slice(&u64_to_bytes(strategy_type));
    result.extend_from_slice(&u64_to_bytes(allocation));
    result.extend_from_slice(&u64_to_bytes(deployed));
    set_return_data(&result);
    0
}

/// Get the complete operational status as 23 little-endian u64 values:
/// [accounting_version, paused, LICN entry present, LICN config valid,
///  LICN is native, ThallLend entry present, ThallLend config valid,
///  strategy registry valid, idle assets, lending assets, total assets,
///  total shares, protocol fees, real liquid custody, custody query ok,
///  liquid custody covers accounting, deposit fee bps, withdrawal fee bps,
///  deposit cap, risk tier, performance fee percent, management fee bps,
///  target slots per year].
#[no_mangle]
pub extern "C" fn get_vault_status() -> u32 {
    let licn_entry_present = has_cv_config_entry(LICN_TOKEN_KEY);
    let licn_token = load_licn_token();
    let licn_config_valid = licn_token.is_some();
    let licn_is_native = licn_token
        .as_ref()
        .map(is_native_token)
        .unwrap_or(false);
    let thalllend_entry_present = has_cv_config_entry(THALLLEND_ADDRESS_KEY);
    let thalllend_config_valid = load_protocol_address(THALLLEND_ADDRESS_KEY).is_some();
    let strategy_registry_valid = lending_strategy().is_ok();
    let idle_assets = load_u64(IDLE_ASSETS_KEY);
    let lending_assets = load_u64(LENDING_ASSETS_KEY);
    let total_assets = load_u64(b"cv_total_assets");
    let total_shares = load_u64(b"cv_total_shares");
    let protocol_fees = load_u64(b"cv_protocol_fees");
    let expected_liquid_custody = idle_assets.checked_add(protocol_fees);
    let custody_result = licn_token
        .map(|token| balance_of_token_or_native(token, get_contract_address()));
    let (real_liquid_custody, custody_query_ok) = match custody_result {
        Some(Ok(custody)) => (custody, true),
        _ => (0, false),
    };
    let custody_covers_accounting = custody_query_ok
        && expected_liquid_custody
            .map(|expected| real_liquid_custody >= expected)
            .unwrap_or(false);

    let values = [
        load_u64(ACCOUNTING_VERSION_KEY),
        is_cv_paused() as u64,
        licn_entry_present as u64,
        licn_config_valid as u64,
        licn_is_native as u64,
        thalllend_entry_present as u64,
        thalllend_config_valid as u64,
        strategy_registry_valid as u64,
        idle_assets,
        lending_assets,
        total_assets,
        total_shares,
        protocol_fees,
        real_liquid_custody,
        custody_query_ok as u64,
        custody_covers_accounting as u64,
        get_deposit_fee_bps(),
        get_withdrawal_fee_bps(),
        get_deposit_cap(),
        load_u64(b"cv_risk_tier"),
        PERFORMANCE_FEE_PERCENT,
        MANAGEMENT_FEE_BPS,
        SLOTS_PER_YEAR,
    ];
    let mut result = Vec::with_capacity(values.len() * 8);
    for value in values {
        result.extend_from_slice(&u64_to_bytes(value));
    }
    set_return_data(&result);
    0
}

// ============================================================================
// V2: PAUSE, FEE CONFIG, DEPOSIT CAP, RISK TIERS, STRATEGY REMOVAL
// ============================================================================

/// Pause vault. Admin only. Blocks deposits; withdrawals still work (safety valve).
/// Returns: 0 success, 1 not admin, 2 already paused
#[no_mangle]
pub extern "C" fn cv_pause(caller_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(addr) => addr,
        None => return 98,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cv_admin(&caller) {
        return 1;
    }
    if is_cv_paused() {
        return 2;
    }
    storage_set(CV_PAUSE_KEY, &[1]);
    log_info("SporeVault paused");
    0
}

/// Unpause vault. Admin only.
/// Returns: 0 success, 1 not admin, 2 not paused
#[no_mangle]
pub extern "C" fn cv_unpause(caller_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(addr) => addr,
        None => return 98,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cv_admin(&caller) {
        return 1;
    }
    if !is_cv_paused() {
        return 2;
    }
    storage_set(CV_PAUSE_KEY, &[0]);
    log_info("SporeVault unpaused");
    0
}

/// Set deposit fee (in BPS). Admin only.
/// Returns: 0 success, 1 not admin, 2 too high
#[no_mangle]
pub extern "C" fn set_deposit_fee(caller_ptr: *const u8, fee_bps: u64) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(addr) => addr,
        None => return 98,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cv_admin(&caller) {
        return 1;
    }
    if fee_bps > MAX_DEPOSIT_FEE_BPS {
        return 2;
    }
    store_u64(b"cv_dep_fee", fee_bps);
    0
}

/// Set withdrawal fee (in BPS). Admin only.
/// Returns: 0 success, 1 not admin, 2 too high
#[no_mangle]
pub extern "C" fn set_withdrawal_fee(caller_ptr: *const u8, fee_bps: u64) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(addr) => addr,
        None => return 98,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cv_admin(&caller) {
        return 1;
    }
    if fee_bps > MAX_WITHDRAWAL_FEE_BPS {
        return 2;
    }
    store_u64(b"cv_wd_fee", fee_bps);
    0
}

/// Set deposit cap (0 = unlimited). Admin only.
/// Returns: 0 success, 1 not admin
#[no_mangle]
pub extern "C" fn set_deposit_cap(caller_ptr: *const u8, cap: u64) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(addr) => addr,
        None => return 98,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cv_admin(&caller) {
        return 1;
    }
    store_u64(b"cv_dep_cap", cap);
    0
}

/// Set vault risk tier. Admin only.
/// Until additional real adapters are implemented, every tier remains
/// ThallLend-only and controls its maximum allocation:
///   1 (conservative) = max 33%
///   2 (moderate) = max 66%
///   3 (aggressive) = max 100%
/// Returns: 0 success, 1 not admin, 2 invalid tier
#[no_mangle]
pub extern "C" fn set_risk_tier(caller_ptr: *const u8, tier: u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(addr) => addr,
        None => return 98,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cv_admin(&caller) {
        return 1;
    }
    if !(RISK_CONSERVATIVE..=RISK_AGGRESSIVE).contains(&tier) {
        return 2;
    }
    let max_allocation = match tier {
        RISK_CONSERVATIVE => 33,
        RISK_MODERATE => 66,
        RISK_AGGRESSIVE => 100,
        _ => return 2,
    };
    let count_u64 = load_u64(b"cv_strategy_count");
    if count_u64 > MAX_STRATEGIES as u64 || lending_strategy().is_err() {
        return 4;
    }
    let count = count_u64 as usize;
    for index in 0..count {
        let allocation_key = alloc::format!("cv_strat_alloc:{}", index);
        if load_u64(allocation_key.as_bytes()) > max_allocation {
            log_info("Existing strategy allocation exceeds requested risk tier");
            return 3;
        }
    }
    store_u64(b"cv_risk_tier", tier as u64);
    0
}

/// Remove a strategy (zero out its allocation). Admin only.
/// Returns: 0 success, 1 not admin, 2 out of bounds
#[no_mangle]
pub extern "C" fn remove_strategy(caller_ptr: *const u8, index: u64) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(addr) => addr,
        None => return 98,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cv_admin(&caller) {
        return 1;
    }
    let count = load_u64(b"cv_strategy_count");
    if count > MAX_STRATEGIES as u64 || lending_strategy().is_err() {
        return 4;
    }
    if index >= count {
        return 2;
    }
    let i = index as usize;
    let type_key = alloc::format!("cv_strat_type:{}", i);
    let alloc_key = alloc::format!("cv_strat_alloc:{}", i);
    let deployed_key = alloc::format!("cv_strat_deployed:{}", i);
    if load_u64(deployed_key.as_bytes()) > 0 {
        log_info("Rebalance deployed assets to zero before removing strategy");
        return 3;
    }
    store_u64(type_key.as_bytes(), 0);
    store_u64(alloc_key.as_bytes(), 0);
    store_u64(deployed_key.as_bytes(), 0);
    log_info("Strategy removed (allocation zeroed)");
    0
}

/// Withdraw accumulated protocol fees. Admin only.
/// Returns fee amount withdrawn (0 if none or not admin).
#[no_mangle]
pub extern "C" fn withdraw_protocol_fees(caller_ptr: *const u8) -> u64 {
    let caller = match read_address32(caller_ptr) {
        Some(addr) => addr,
        None => return 0,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cv_admin(&caller) {
        return 0;
    }
    let fees = load_u64(b"cv_protocol_fees");
    if fees == 0 {
        return 0;
    }
    store_u64(b"cv_protocol_fees", 0);

    // G25-02: Transfer fees to admin
    if !transfer_licn_out(&caller, fees) {
        store_u64(b"cv_protocol_fees", fees); // revert
        log_info("Fee transfer failed");
        return 0;
    }

    log_info("Protocol fees withdrawn");
    fees
}

/// Update strategy allocation. Admin only.
/// Returns: 0 success, 1 not admin, 2 out of bounds, 3 total > 100%
#[no_mangle]
pub extern "C" fn update_strategy_allocation(
    caller_ptr: *const u8,
    index: u64,
    new_alloc: u64,
) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(addr) => addr,
        None => return 98,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cv_admin(&caller) {
        return 1;
    }
    let count = load_u64(b"cv_strategy_count");
    if count > MAX_STRATEGIES as u64 || lending_strategy().is_err() {
        return 6;
    }
    if index >= count {
        return 2;
    }
    let type_key = alloc::format!("cv_strat_type:{}", index);
    if load_u64(type_key.as_bytes()) != STRATEGY_LENDING as u64 {
        return 4;
    }
    let risk_tier = load_u64(b"cv_risk_tier") as u8;
    let max_allocation = match risk_tier {
        RISK_CONSERVATIVE => 33,
        RISK_MODERATE => 66,
        RISK_AGGRESSIVE => 100,
        _ => return 5,
    };
    if new_alloc > max_allocation {
        return 5;
    }

    // Check total allocation with new value
    let mut total: u64 = new_alloc;
    for i in 0..count as usize {
        if i == index as usize {
            continue;
        }
        let alloc_key = alloc::format!("cv_strat_alloc:{}", i);
        total = match total.checked_add(load_u64(alloc_key.as_bytes())) {
            Some(total) => total,
            None => return 3,
        };
    }
    if total > 100 {
        return 3;
    }

    let alloc_key = alloc::format!("cv_strat_alloc:{}", index);
    store_u64(alloc_key.as_bytes(), new_alloc);
    0
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use lichen_sdk::bytes_to_u64;
    use lichen_sdk::test_mock;

    fn setup() {
        test_mock::reset();
    }

    /// G25-02: Configure LICN token and mock cross-contract transfers so
    /// withdraw / withdraw_protocol_fees can succeed in unit tests.
    fn enable_token_transfers() {
        let admin = [1u8; 32];
        let prev_caller = lichen_sdk::get_caller();
        test_mock::set_caller(admin);
        let licn_token = [0xCC; 32];
        set_licn_token(admin.as_ptr(), licn_token.as_ptr());
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        test_mock::set_caller(prev_caller.0);
    }

    fn reentrancy_engaged() -> bool {
        storage_get(CV_REENTRANCY_KEY)
            .map(|data| data.first().copied() == Some(1))
            .unwrap_or(false)
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
        assert_eq!(load_u64(b"cv_total_shares"), 0);
        assert_eq!(load_u64(b"cv_total_assets"), 0);
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
    fn test_add_strategy() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_risk_tier(admin.as_ptr(), RISK_MODERATE), 0);
        let result = add_strategy(admin.as_ptr(), STRATEGY_LENDING, 50);
        assert_eq!(result, 0);
        assert_eq!(load_u64(b"cv_strategy_count"), 1);
    }

    #[test]
    fn test_add_strategy_unauthorized() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let other = [2u8; 32];
        test_mock::set_caller(other);
        assert_eq!(add_strategy(other.as_ptr(), STRATEGY_LENDING, 50), 2);
    }

    #[test]
    fn test_add_strategy_invalid_type() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(add_strategy(admin.as_ptr(), 0, 50), 6);
    }

    #[test]
    fn test_add_strategy_rejects_duplicate_lending_adapter() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_risk_tier(admin.as_ptr(), RISK_AGGRESSIVE), 0);
        assert_eq!(add_strategy(admin.as_ptr(), STRATEGY_LENDING, 60), 0);
        assert_eq!(add_strategy(admin.as_ptr(), STRATEGY_LENDING, 40), 8);
        assert_eq!(update_strategy_allocation(admin.as_ptr(), 0, 0), 0);
        assert_eq!(add_strategy(admin.as_ptr(), STRATEGY_LENDING, 40), 8);
    }

    #[test]
    fn test_deposit() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        let amount = 100_000u64;
        test_mock::set_value(amount);
        let shares = deposit(user.as_ptr(), amount);
        // V2: deposit fee = 100_000 * 10 / 10_000 = 100; net = 99_900
        // First deposit: shares = net - MIN_LOCKED_SHARES = 99_900 - 1_000 = 98_900
        assert_eq!(shares, 98_900);
    }

    #[test]
    fn test_deposit_zero() {
        setup();
        let user = [2u8; 32];
        assert_eq!(deposit(user.as_ptr(), 0), 0);
    }

    #[test]
    fn test_deposit_too_small_first() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(MIN_LOCKED_SHARES);
        assert_eq!(deposit(user.as_ptr(), MIN_LOCKED_SHARES), 0);
    }

    #[test]
    fn test_deposit_second() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user1 = [2u8; 32];
        test_mock::set_caller(user1);
        test_mock::set_value(100_000);
        deposit(user1.as_ptr(), 100_000);
        // After first deposit: total_shares = 1000 + 98_900 = 99_900, total_assets = 99_900
        let user2 = [3u8; 32];
        test_mock::set_caller(user2);
        test_mock::set_value(50_000);
        let shares2 = deposit(user2.as_ptr(), 50_000);
        // fee = 50_000 * 10 / 10_000 = 50, net = 49_950
        // shares = 49_950 * 99_900 / 99_900 = 49_950
        assert_eq!(shares2, 49_950);
    }

    #[test]
    fn test_withdraw() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        enable_token_transfers();
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(0);
        let shares = deposit(user.as_ptr(), 100_000);
        let amount = withdraw(user.as_ptr(), shares);
        assert!(amount > 0);
    }

    #[test]
    fn test_withdraw_zero() {
        setup();
        let user = [2u8; 32];
        assert_eq!(withdraw(user.as_ptr(), 0), 0);
    }

    #[test]
    fn test_withdraw_insufficient_shares() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(100_000);
        deposit(user.as_ptr(), 100_000);
        // User has 98_900 shares, try withdrawing 100_000
        assert_eq!(withdraw(user.as_ptr(), 100_000), 0);
    }

    #[test]
    fn test_deposit_reentrancy_guard_blocks_nested_entry() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let user = [2u8; 32];
        storage_set(CV_REENTRANCY_KEY, &[1u8]);
        test_mock::set_caller(user);
        test_mock::set_value(100_000);

        assert_eq!(deposit(user.as_ptr(), 100_000), 0);
        assert!(reentrancy_engaged());
        assert_eq!(load_u64(b"cv_total_assets"), 0);
    }

    #[test]
    fn test_deposit_failed_first_deposit_clears_reentrancy_guard() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(MIN_LOCKED_SHARES);
        assert_eq!(deposit(user.as_ptr(), MIN_LOCKED_SHARES), 0);
        assert!(!reentrancy_engaged());

        test_mock::set_value(100_000);
        assert!(deposit(user.as_ptr(), 100_000) > 0);
    }

    #[test]
    fn test_withdraw_reentrancy_guard_blocks_nested_entry() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        enable_token_transfers();

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(0);
        let shares = deposit(user.as_ptr(), 100_000);

        storage_set(CV_REENTRANCY_KEY, &[1u8]);
        assert_eq!(withdraw(user.as_ptr(), shares), 0);
        assert!(reentrancy_engaged());
    }

    #[test]
    fn test_withdraw_failed_burn_clears_reentrancy_guard() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        enable_token_transfers();

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(0);
        let shares = deposit(user.as_ptr(), 100_000);

        assert_eq!(withdraw(user.as_ptr(), shares + 1), 0);
        assert!(!reentrancy_engaged());
        assert!(withdraw(user.as_ptr(), shares) > 0);
    }

    #[test]
    fn test_harvest_without_strategy_keeps_idle_assets() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        // Set deposit fee to 0 for clean math
        set_deposit_fee(admin.as_ptr(), 0);
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000_000_000);
        deposit(user.as_ptr(), 1_000_000_000_000);
        // Advance 400,000 deterministic slots.
        test_mock::set_timestamp(401_000);
        let result = harvest();
        assert_eq!(result, 0);
        // No simulated yield is booked; only the published 2% annualized
        // management fee moves from depositor assets to protocol custody.
        let total_assets = load_u64(b"cv_total_assets");
        assert_eq!(total_assets, 999_898_528_666);
        assert_eq!(load_u64(b"cv_protocol_fees"), 101_471_334);
        assert_eq!(load_u64(b"cv_fees_earned"), 101_471_334);
    }

    #[test]
    fn test_management_fee_is_exactly_two_percent_per_target_slot_year() {
        setup();
        let admin = [1u8; 32];
        let user = [2u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_deposit_fee(admin.as_ptr(), 0), 0);

        test_mock::set_caller(user);
        test_mock::set_value(1_000_000_000);
        assert_eq!(deposit(user.as_ptr(), 1_000_000_000), 999_999_000);

        test_mock::set_timestamp(1_000 + SLOTS_PER_YEAR);
        assert_eq!(harvest(), 0);
        assert_eq!(load_u64(b"cv_total_assets"), 980_000_000);
        assert_eq!(load_u64(IDLE_ASSETS_KEY), 980_000_000);
        assert_eq!(load_u64(b"cv_protocol_fees"), 20_000_000);
        assert_eq!(load_u64(b"cv_fees_earned"), 20_000_000);
        assert_eq!(load_u64(MANAGEMENT_FEE_REMAINDER_KEY), 0);
    }

    #[test]
    fn test_harvest_no_assets() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        test_mock::set_timestamp(2000);
        assert_eq!(harvest(), 0);
    }

    #[test]
    fn test_harvest_reentrancy_guard_blocks_nested_entry() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        storage_set(CV_REENTRANCY_KEY, &[1u8]);
        assert_eq!(harvest(), 1);
        assert!(reentrancy_engaged());
    }

    #[test]
    fn test_harvest_no_assets_clears_reentrancy_guard() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        test_mock::set_timestamp(2000);
        assert_eq!(harvest(), 0);
        assert!(!reentrancy_engaged());

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(100_000);
        assert!(deposit(user.as_ptr(), 100_000) > 0);
    }

    #[test]
    fn test_get_vault_stats() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(get_vault_stats(), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 48);
    }

    #[test]
    fn test_get_user_position() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(100_000);
        deposit(user.as_ptr(), 100_000);
        assert_eq!(get_user_position(user.as_ptr()), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 16);
        let shares = bytes_to_u64(&ret[0..8]);
        assert_eq!(shares, 98_900); // 100k - 100 fee - 1k locked
    }

    #[test]
    fn test_get_strategy_info() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_risk_tier(admin.as_ptr(), RISK_MODERATE), 0);
        assert_eq!(add_strategy(admin.as_ptr(), STRATEGY_LENDING, 50), 0);
        assert_eq!(get_strategy_info(0), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 24);
        assert_eq!(bytes_to_u64(&ret[0..8]), STRATEGY_LENDING as u64);
        assert_eq!(bytes_to_u64(&ret[8..16]), 50);
    }

    #[test]
    fn test_get_strategy_info_out_of_bounds() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(get_strategy_info(0), 1);
    }

    // ====================================================================
    // V2 TESTS
    // ====================================================================

    #[test]
    fn test_pause_unpause() {
        setup();
        let admin = [1u8; 32];
        let non_admin = [2u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        enable_token_transfers();

        test_mock::set_caller(non_admin);
        assert_eq!(cv_pause(non_admin.as_ptr()), 1); // not admin
        test_mock::set_caller(admin);
        assert_eq!(cv_pause(admin.as_ptr()), 0);
        assert_eq!(cv_pause(admin.as_ptr()), 2); // already paused

        // Deposit blocked when paused
        let user = [3u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(100_000);
        assert_eq!(deposit(user.as_ptr(), 100_000), 0);

        // Withdraw still works (safety valve) — need prior deposit
        // Unpause first to deposit, then re-pause
        test_mock::set_caller(admin);
        assert_eq!(cv_unpause(admin.as_ptr()), 0);
        test_mock::set_caller(user);
        test_mock::set_value(0);
        let shares = deposit(user.as_ptr(), 100_000);
        assert!(shares > 0);
        test_mock::set_caller(admin);
        assert_eq!(cv_pause(admin.as_ptr()), 0);

        // Withdraw works even when paused
        test_mock::set_caller(user);
        let amount = withdraw(user.as_ptr(), shares);
        assert!(amount > 0);

        test_mock::set_caller(non_admin);
        assert_eq!(cv_unpause(non_admin.as_ptr()), 1); // not admin
        test_mock::set_caller(admin);
        assert_eq!(cv_unpause(admin.as_ptr()), 0);
        assert_eq!(cv_unpause(admin.as_ptr()), 2); // not paused
    }

    #[test]
    fn test_deposit_fee_configuration() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        // Set deposit fee to 0
        assert_eq!(set_deposit_fee(admin.as_ptr(), 0), 0);
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(100_000);
        let shares = deposit(user.as_ptr(), 100_000);
        // No fee: shares = 100_000 - 1_000 = 99_000
        assert_eq!(shares, 99_000);
    }

    #[test]
    fn test_deposit_fee_too_high() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_deposit_fee(admin.as_ptr(), 501), 2); // > 500 BPS
    }

    #[test]
    fn test_withdrawal_fee_configuration() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        enable_token_transfers();

        // Set withdrawal fee to 0
        assert_eq!(set_withdrawal_fee(admin.as_ptr(), 0), 0);
        let user = [2u8; 32];
        // Also set deposit fee to 0 for simpler math
        set_deposit_fee(admin.as_ptr(), 0);
        test_mock::set_caller(user);
        test_mock::set_value(0);
        let shares = deposit(user.as_ptr(), 100_000);
        assert_eq!(shares, 99_000); // 100k - 1k locked

        // Withdraw all shares — no fee
        let amount = withdraw(user.as_ptr(), shares);
        // total_assets = 100_000 (1k locked + 99k user), shares = 99_000
        // gross = 99_000 * 100_000 / 100_000 = 99_000, fee = 0, net = 99_000
        assert_eq!(amount, 99_000);
    }

    #[test]
    fn test_withdrawal_fee_too_high() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_withdrawal_fee(admin.as_ptr(), 501), 2);
    }

    #[test]
    fn test_deposit_cap() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        // Set cap at 200_000
        assert_eq!(set_deposit_cap(admin.as_ptr(), 200_000), 0);

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(150_000);
        let shares1 = deposit(user.as_ptr(), 150_000);
        assert!(shares1 > 0);

        // Second deposit would exceed cap (total_assets ~149_850 + 100_000 > 200_000)
        test_mock::set_value(100_000);
        let shares2 = deposit(user.as_ptr(), 100_000);
        assert_eq!(shares2, 0); // rejected
    }

    #[test]
    fn test_risk_tier() {
        setup();
        let admin = [1u8; 32];
        let non_admin = [2u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        test_mock::set_caller(non_admin);
        assert_eq!(set_risk_tier(non_admin.as_ptr(), RISK_CONSERVATIVE), 1);
        test_mock::set_caller(admin);
        assert_eq!(set_risk_tier(admin.as_ptr(), 0), 2); // invalid
        assert_eq!(set_risk_tier(admin.as_ptr(), 4), 2); // invalid
        assert_eq!(set_risk_tier(admin.as_ptr(), RISK_CONSERVATIVE), 0);
        assert_eq!(load_u64(b"cv_risk_tier"), RISK_CONSERVATIVE as u64);
        assert_eq!(set_risk_tier(admin.as_ptr(), RISK_AGGRESSIVE), 0);
    }

    #[test]
    fn test_remove_strategy() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_risk_tier(admin.as_ptr(), RISK_MODERATE), 0);
        add_strategy(admin.as_ptr(), STRATEGY_LENDING, 50);

        // Non-admin fails
        let other = [2u8; 32];
        test_mock::set_caller(other);
        assert_eq!(remove_strategy(other.as_ptr(), 0), 1);

        // Out of bounds fails
        test_mock::set_caller(admin);
        assert_eq!(remove_strategy(admin.as_ptr(), 5), 2);

        // Remove strategy 0
        assert_eq!(remove_strategy(admin.as_ptr(), 0), 0);

        // Verify allocation zeroed
        let alloc_key = alloc::format!("cv_strat_alloc:{}", 0);
        assert_eq!(load_u64(alloc_key.as_bytes()), 0);
    }

    #[test]
    fn test_withdraw_protocol_fees() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        enable_token_transfers();

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(0);
        deposit(user.as_ptr(), 1_000_000); // fee = 1_000_000 * 10 / 10_000 = 100

        test_mock::set_caller(admin);
        let fees = withdraw_protocol_fees(admin.as_ptr());
        assert_eq!(fees, 1000); // 1_000_000 * 10 / 10_000 = 1000

        // Second call returns 0
        assert_eq!(withdraw_protocol_fees(admin.as_ptr()), 0);

        // Non-admin returns 0
        let other = [3u8; 32];
        test_mock::set_caller(other);
        assert_eq!(withdraw_protocol_fees(other.as_ptr()), 0);
    }

    #[test]
    fn test_update_strategy_allocation() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_risk_tier(admin.as_ptr(), RISK_MODERATE), 0);
        add_strategy(admin.as_ptr(), STRATEGY_LENDING, 50);

        // Update strategy 0 from 50 to 40
        assert_eq!(update_strategy_allocation(admin.as_ptr(), 0, 40), 0);
        let alloc_key = alloc::format!("cv_strat_alloc:{}", 0);
        assert_eq!(load_u64(alloc_key.as_bytes()), 40);

        // Requested allocation exceeds the moderate risk-tier limit.
        assert_eq!(update_strategy_allocation(admin.as_ptr(), 0, 80), 5);

        // Non-admin fails
        let other = [2u8; 32];
        test_mock::set_caller(other);
        assert_eq!(update_strategy_allocation(other.as_ptr(), 0, 10), 1);
    }

    // ====================================================================
    // PROTOCOL YIELD INTEGRATION TESTS
    // ====================================================================

    #[test]
    fn test_set_protocol_addresses_rejects_unimplemented_lp_adapter() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let thalllend = [0xAA; 32];
        let lichenswap = [0xBB; 32];
        assert_eq!(
            set_protocol_addresses(admin.as_ptr(), thalllend.as_ptr(), lichenswap.as_ptr()),
            4
        );
        assert!(test_mock::get_storage(THALLLEND_ADDRESS_KEY).is_none());
        assert!(test_mock::get_storage(LICHENSWAP_ADDRESS_KEY).is_none());
    }

    #[test]
    fn test_set_protocol_addresses_not_admin() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let other = [99u8; 32];
        test_mock::set_caller(other);
        let addr = [0xAA; 32];
        assert_eq!(
            set_protocol_addresses(other.as_ptr(), addr.as_ptr(), addr.as_ptr()),
            1
        );
    }

    #[test]
    fn test_set_protocol_addresses_partial() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        // Only set thalllend (lichenswap = zero → skipped)
        let thalllend = [0xAA; 32];
        let zero = [0u8; 32];
        assert_eq!(
            set_protocol_addresses(admin.as_ptr(), thalllend.as_ptr(), zero.as_ptr()),
            0
        );
        assert!(test_mock::get_storage(THALLLEND_ADDRESS_KEY).is_some());
        assert!(test_mock::get_storage(LICHENSWAP_ADDRESS_KEY).is_none());
    }

    #[test]
    fn test_set_protocol_addresses_cannot_reconfigure() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let zero = [0u8; 32];
        let thalllend = [0xAA; 32];
        assert_eq!(
            set_protocol_addresses(admin.as_ptr(), thalllend.as_ptr(), zero.as_ptr()),
            0
        );

        let new_thalllend = [0xAB; 32];
        assert_eq!(
            set_protocol_addresses(admin.as_ptr(), new_thalllend.as_ptr(), zero.as_ptr()),
            2
        );
        assert_eq!(
            test_mock::get_storage(THALLLEND_ADDRESS_KEY)
                .unwrap()
                .as_slice(),
            &thalllend
        );

        let new_lichenswap = [0xBC; 32];
        assert_eq!(
            set_protocol_addresses(admin.as_ptr(), zero.as_ptr(), new_lichenswap.as_ptr()),
            4
        );
        assert!(test_mock::get_storage(LICHENSWAP_ADDRESS_KEY).is_none());
    }

    #[test]
    fn test_unsupported_strategy_adapters_fail_closed() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        assert_eq!(add_strategy(admin.as_ptr(), STRATEGY_LP, 10), 6);
        assert_eq!(add_strategy(admin.as_ptr(), STRATEGY_STAKING, 10), 6);
        assert_eq!(load_u64(b"cv_strategy_count"), 0);
    }

    #[test]
    fn test_accounting_migration_verifies_real_custody_and_expectations() {
        setup();
        let admin = [1u8; 32];
        let user = [2u8; 32];
        let token = [0xCC; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_licn_token(admin.as_ptr(), token.as_ptr()), 0);
        lichen_sdk::storage::remove(ACCOUNTING_VERSION_KEY);
        lichen_sdk::storage::remove(IDLE_ASSETS_KEY);
        lichen_sdk::storage::remove(LENDING_ASSETS_KEY);
        store_u64(b"cv_protocol_fees", 100);
        store_u64(b"cv_total_shares", 1_000);
        assert_eq!(cv_pause(admin.as_ptr()), 0);

        test_mock::set_caller(user);
        assert_eq!(deposit(user.as_ptr(), 10_000), 0);

        test_mock::set_caller(admin);
        test_mock::set_cross_call_response(Some(u64_to_bytes(1_100).to_vec()));
        assert_eq!(migrate_accounting_v2(admin.as_ptr(), 999, 0), 8);
        assert!(!accounting_v2_ready());
        assert_eq!(migrate_accounting_v2(admin.as_ptr(), 1_000, 0), 0);
        assert!(accounting_v2_ready());
        assert_eq!(load_u64(IDLE_ASSETS_KEY), 1_000);
        assert_eq!(load_u64(LENDING_ASSETS_KEY), 0);
        assert_eq!(load_u64(b"cv_total_assets"), 1_000);
        assert_eq!(migrate_accounting_v2(admin.as_ptr(), 1_000, 0), 2);
    }

    #[test]
    fn test_legacy_strategy_retirement_is_paused_and_source_bound() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        lichen_sdk::storage::remove(ACCOUNTING_VERSION_KEY);
        store_u64(b"cv_strategy_count", 2);
        store_u64(b"cv_strat_type:0", STRATEGY_LP as u64);
        store_u64(b"cv_strat_alloc:0", 40);
        store_u64(b"cv_strat_deployed:0", 12_345);
        store_u64(b"cv_strat_type:1", STRATEGY_LENDING as u64);
        store_u64(b"cv_strat_alloc:1", 30);
        store_u64(b"cv_strat_deployed:1", 6_789);

        assert_eq!(retire_legacy_strategy(admin.as_ptr(), 0, 2, 40, 12_345), 3);
        assert_eq!(cv_pause(admin.as_ptr()), 0);
        assert_eq!(retire_legacy_strategy(admin.as_ptr(), 0, 2, 41, 12_345), 5);
        assert_eq!(retire_legacy_strategy(admin.as_ptr(), 0, 2, 40, 12_345), 0);
        assert_eq!(load_u64(b"cv_strat_type:0"), 0);
        assert_eq!(load_u64(b"cv_strat_alloc:0"), 0);
        assert_eq!(load_u64(b"cv_strat_deployed:0"), 0);
        assert_eq!(retire_legacy_strategy(admin.as_ptr(), 1, 1, 30, 6_789), 7);
    }

    #[test]
    fn test_rebalance_deploys_real_lending_assets() {
        setup();
        let admin = [1u8; 32];
        let user = [2u8; 32];
        let token = [0xCC; 32];
        let thalllend = [0xAA; 32];
        let zero = [0u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_licn_token(admin.as_ptr(), token.as_ptr()), 0);
        assert_eq!(set_deposit_fee(admin.as_ptr(), 0), 0);
        assert_eq!(
            set_protocol_addresses(admin.as_ptr(), thalllend.as_ptr(), zero.as_ptr()),
            0
        );

        test_mock::set_caller(user);
        assert_eq!(deposit(user.as_ptr(), 1_000_000), 999_000);
        test_mock::set_caller(admin);
        assert_eq!(set_risk_tier(admin.as_ptr(), RISK_MODERATE), 0);
        assert_eq!(add_strategy(admin.as_ptr(), STRATEGY_LENDING, 50), 0);

        test_mock::set_cross_call_responses(std::vec![
            u64_to_bytes(0).to_vec(),
            0u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
            u64_to_bytes(500_000).to_vec(),
        ]);
        assert_eq!(rebalance(), 0);
        assert_eq!(load_u64(IDLE_ASSETS_KEY), 500_000);
        assert_eq!(load_u64(LENDING_ASSETS_KEY), 500_000);
        assert_eq!(load_u64(b"cv_total_assets"), 1_000_000);
        assert_eq!(load_u64(b"cv_strat_deployed:0"), 500_000);
    }

    #[test]
    fn test_withdraw_recalls_deployed_lending_liquidity() {
        setup();
        let admin = [1u8; 32];
        let user = [2u8; 32];
        let token = [0xCC; 32];
        let thalllend = [0xAA; 32];
        let zero = [0u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_licn_token(admin.as_ptr(), token.as_ptr()), 0);
        assert_eq!(set_withdrawal_fee(admin.as_ptr(), 0), 0);
        assert_eq!(
            set_protocol_addresses(admin.as_ptr(), thalllend.as_ptr(), zero.as_ptr()),
            0
        );
        assert_eq!(add_strategy(admin.as_ptr(), STRATEGY_LENDING, 30), 0);

        let share_key = make_key(b"cv_shares:", &hex_encode_addr(&user));
        store_u64(&share_key, 1_000);
        store_u64(b"cv_total_shares", 1_000);
        store_u64(IDLE_ASSETS_KEY, 100);
        store_u64(LENDING_ASSETS_KEY, 900);
        store_u64(b"cv_total_assets", 1_000);
        store_u64(b"cv_strat_deployed:0", 900);

        test_mock::set_caller(user);
        test_mock::set_cross_call_responses(std::vec![
            u64_to_bytes(900).to_vec(),
            0u32.to_le_bytes().to_vec(),
            u64_to_bytes(500).to_vec(),
            0u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(withdraw(user.as_ptr(), 500), 500);
        assert_eq!(load_u64(IDLE_ASSETS_KEY), 0);
        assert_eq!(load_u64(LENDING_ASSETS_KEY), 500);
        assert_eq!(load_u64(b"cv_total_assets"), 500);
        assert_eq!(load_u64(&share_key), 500);
    }

    #[test]
    fn test_harvest_realizes_only_real_lending_yield() {
        setup();
        let admin = [1u8; 32];
        let token = [0xCC; 32];
        let thalllend = [0xAA; 32];
        let zero = [0u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_licn_token(admin.as_ptr(), token.as_ptr()), 0);
        assert_eq!(
            set_protocol_addresses(admin.as_ptr(), thalllend.as_ptr(), zero.as_ptr()),
            0
        );
        assert_eq!(set_risk_tier(admin.as_ptr(), RISK_MODERATE), 0);
        assert_eq!(add_strategy(admin.as_ptr(), STRATEGY_LENDING, 40), 0);
        store_u64(IDLE_ASSETS_KEY, 600_000);
        store_u64(LENDING_ASSETS_KEY, 400_000);
        store_u64(b"cv_total_assets", 1_000_000);
        store_u64(b"cv_strat_deployed:0", 400_000);

        test_mock::set_cross_call_responses(std::vec![
            u64_to_bytes(410_000).to_vec(),
            0u32.to_le_bytes().to_vec(),
            u64_to_bytes(409_000).to_vec(),
            0u32.to_le_bytes().to_vec(),
            u64_to_bytes(403_559).to_vec(),
        ]);
        test_mock::set_timestamp(401_000);
        assert_eq!(harvest(), 0);
        assert_eq!(load_u64(IDLE_ASSETS_KEY), 605_339);
        assert_eq!(load_u64(LENDING_ASSETS_KEY), 403_559);
        assert_eq!(load_u64(b"cv_total_assets"), 1_008_898);
        assert_eq!(load_u64(b"cv_protocol_fees"), 1_102);
        assert_eq!(load_u64(b"cv_fees_earned"), 1_102);
        assert_eq!(load_u64(b"cv_total_earned"), 9_000);
    }

    #[test]
    fn test_harvest_fails_closed_when_real_lending_claim_is_unavailable() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(add_strategy(admin.as_ptr(), STRATEGY_LENDING, 30), 0);
        store_u64(IDLE_ASSETS_KEY, 1_000_000);
        store_u64(b"cv_total_assets", 1_000_000);
        test_mock::set_timestamp(401_000);

        assert_eq!(harvest(), 93);
        assert_eq!(load_u64(b"cv_total_assets"), 1_000_000);
        assert_eq!(load_u64(b"cv_total_earned"), 0);
    }

    // ====================================================================
    // G25-02 TESTS: Financial wiring & real yield
    // ====================================================================

    #[test]
    fn test_g25_deposit_requires_get_value() {
        // Deposit must fail when get_value() < amount (no LICN attached)
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        // No set_value → get_value() returns 0
        assert_eq!(deposit(user.as_ptr(), 100_000), 0);
    }

    #[test]
    fn test_g25_withdraw_triggers_transfer() {
        // Withdraw must attempt token transfer via cross-contract call
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        enable_token_transfers();
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(0);
        let shares = deposit(user.as_ptr(), 100_000);
        assert!(shares > 0);
        let amount = withdraw(user.as_ptr(), shares);
        assert!(amount > 0);
    }

    #[test]
    fn test_g25_withdraw_fees_triggers_transfer() {
        // withdraw_protocol_fees must attempt transfer via cross-contract call
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        enable_token_transfers();
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(0);
        deposit(user.as_ptr(), 1_000_000);
        test_mock::set_caller(admin);
        let fees = withdraw_protocol_fees(admin.as_ptr());
        // Fees collected from deposit (1000 = 1M * 10bps)
        assert_eq!(fees, 1000);
    }

    #[test]
    fn test_g25_set_licn_token() {
        // Admin can set LICN token address
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let token = [0xCC; 32];
        assert_eq!(set_licn_token(admin.as_ptr(), token.as_ptr()), 0);
        let stored = test_mock::get_storage(LICN_TOKEN_KEY).unwrap();
        assert_eq!(stored.as_slice(), &token);

        // Non-admin fails
        let other = [2u8; 32];
        test_mock::set_caller(other);
        assert_eq!(set_licn_token(other.as_ptr(), token.as_ptr()), 1);
    }

    #[test]
    fn test_g25_set_licn_token_cannot_reconfigure() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let token = [0xCC; 32];
        let new_token = [0xCD; 32];
        assert_eq!(set_licn_token(admin.as_ptr(), token.as_ptr()), 0);
        assert_eq!(set_licn_token(admin.as_ptr(), new_token.as_ptr()), 2);
        assert_eq!(
            test_mock::get_storage(LICN_TOKEN_KEY).unwrap().as_slice(),
            &token
        );
    }

    #[test]
    fn test_g25_no_phantom_inflation() {
        // Harvest with strategies but no real protocol → total_assets stays unchanged
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        set_deposit_fee(admin.as_ptr(), 0);

        add_strategy(admin.as_ptr(), STRATEGY_LENDING, 40);
        add_strategy(admin.as_ptr(), STRATEGY_LP, 30);
        add_strategy(admin.as_ptr(), STRATEGY_STAKING, 30);

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000_000);
        deposit(user.as_ptr(), 1_000_000_000);

        let assets_before = load_u64(b"cv_total_assets");

        // Harvest multiple times with advancing timestamps
        test_mock::set_timestamp(100_000);
        harvest();
        test_mock::set_timestamp(200_000);
        harvest();
        test_mock::set_timestamp(500_000);
        harvest();

        let assets_after = load_u64(b"cv_total_assets");
        // No phantom yield: only deterministic management fees reduce NAV.
        assert_eq!(assets_before, 1_000_000_000);
        assert_eq!(assets_after, 999_873_419);
        assert_eq!(load_u64(b"cv_total_earned"), 0);
        assert_eq!(load_u64(b"cv_fees_earned"), 126_581);
        assert_eq!(load_u64(b"cv_protocol_fees"), 126_581);
    }

    #[test]
    fn test_g25_deposit_requires_exact_native_value() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(100_000); // exact match
        let shares = deposit(user.as_ptr(), 100_000);
        assert!(shares > 0);

        let second_user = [3u8; 32];
        test_mock::set_caller(second_user);
        test_mock::set_value(100_001);
        assert_eq!(deposit(second_user.as_ptr(), 100_000), 0);
        assert_eq!(load_u64(b"cv_total_assets"), 99_900);
    }

    #[test]
    fn test_mt20_deposit_rejects_attached_native_value() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        enable_token_transfers();

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(100_000);
        assert_eq!(deposit(user.as_ptr(), 100_000), 0);
        assert_eq!(load_u64(b"cv_total_assets"), 0);
    }

    #[test]
    fn test_malformed_immutable_configuration_fails_closed() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        storage_set(LICN_TOKEN_KEY, &[7u8; 31]);
        let replacement = [8u8; 32];
        assert_eq!(set_licn_token(admin.as_ptr(), replacement.as_ptr()), 3);

        storage_set(THALLLEND_ADDRESS_KEY, &[9u8; 31]);
        let thalllend = [10u8; 32];
        let zero = [0u8; 32];
        assert_eq!(
            set_protocol_addresses(admin.as_ptr(), thalllend.as_ptr(), zero.as_ptr()),
            2
        );

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(100_000);
        assert_eq!(deposit(user.as_ptr(), 100_000), 0);
    }

    #[test]
    fn test_inconsistent_share_bootstrap_and_strategy_registry_fail_closed() {
        setup();
        let admin = [1u8; 32];
        let user = [2u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        store_u64(b"cv_total_shares", 1);
        test_mock::set_caller(user);
        test_mock::set_value(100_000);
        assert_eq!(deposit(user.as_ptr(), 100_000), 0);

        store_u64(b"cv_total_shares", 0);
        store_u64(b"cv_strategy_count", MAX_STRATEGIES as u64 + 1);
        test_mock::set_timestamp(get_timestamp() + 1);
        assert_eq!(harvest(), 96);
        assert!(!reentrancy_engaged());
    }

    #[test]
    fn test_get_vault_status_reports_real_configuration_and_custody() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        test_mock::set_cross_call_response(Some(0u64.to_le_bytes().to_vec()));

        assert_eq!(get_vault_status(), 0);
        let result = test_mock::get_return_data();
        assert_eq!(result.len(), 23 * 8);
        let values: Vec<u64> = result
            .as_chunks::<8>()
            .0
            .iter()
            .map(|value| bytes_to_u64(value))
            .collect();
        assert_eq!(values[0], ACCOUNTING_VERSION_V2);
        assert_eq!(values[2], 1);
        assert_eq!(values[3], 1);
        assert_eq!(values[4], 1);
        assert_eq!(values[5], 0);
        assert_eq!(values[14], 1);
        assert_eq!(values[15], 1);
        assert_eq!(values[16], DEFAULT_DEPOSIT_FEE_BPS);
        assert_eq!(values[17], DEFAULT_WITHDRAWAL_FEE_BPS);
        assert_eq!(values[20], PERFORMANCE_FEE_PERCENT);
        assert_eq!(values[21], MANAGEMENT_FEE_BPS);
        assert_eq!(values[22], SLOTS_PER_YEAR);
    }

    #[test]
    fn test_deposit_spoofed_caller_does_not_accrue_fee() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let user = [2u8; 32];
        let attacker = [9u8; 32];
        test_mock::set_caller(attacker);
        test_mock::set_value(100_000);

        assert_eq!(deposit(user.as_ptr(), 100_000), 200);
        assert_eq!(load_u64(b"cv_protocol_fees"), 0);
        assert_eq!(load_u64(b"cv_total_assets"), 0);
        assert_eq!(load_u64(b"cv_total_shares"), 0);
    }

    #[test]
    fn test_failed_first_deposit_does_not_accrue_fee() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(MIN_LOCKED_SHARES);

        assert_eq!(deposit(user.as_ptr(), MIN_LOCKED_SHARES), 0);
        assert_eq!(load_u64(b"cv_protocol_fees"), 0);
        assert_eq!(load_u64(b"cv_total_assets"), 0);
        assert_eq!(load_u64(b"cv_total_shares"), 0);
    }

    #[test]
    fn test_withdraw_false_transfer_restores_exact_fee_state() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        enable_token_transfers();

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(100_000);
        let shares = deposit(user.as_ptr(), 100_000);
        let share_key = make_key(b"cv_shares:", &hex_encode_addr(&user));
        let prev_assets = load_u64(b"cv_total_assets");
        let prev_shares = load_u64(b"cv_total_shares");
        store_u64(b"cv_protocol_fees", u64::MAX - 1);

        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));

        assert_eq!(withdraw(user.as_ptr(), shares), 0);
        assert_eq!(load_u64(&share_key), shares);
        assert_eq!(load_u64(b"cv_total_assets"), prev_assets);
        assert_eq!(load_u64(b"cv_total_shares"), prev_shares);
        assert_eq!(load_u64(b"cv_protocol_fees"), u64::MAX - 1);
    }

    #[test]
    fn test_withdraw_protocol_fees_false_transfer_preserves_fees() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        enable_token_transfers();
        store_u64(b"cv_protocol_fees", 777);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));

        assert_eq!(withdraw_protocol_fees(admin.as_ptr()), 0);
        assert_eq!(load_u64(b"cv_protocol_fees"), 777);
    }

    #[test]
    fn test_add_strategy_malformed_registry_rejected() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        store_u64(b"cv_strategy_count", 1);
        store_u64(b"cv_strat_alloc:0", u64::MAX);

        assert_eq!(add_strategy(admin.as_ptr(), STRATEGY_LENDING, 1), 9);
        assert_eq!(load_u64(b"cv_strategy_count"), 1);
    }

    #[test]
    fn test_update_strategy_malformed_registry_rejected() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        store_u64(b"cv_strategy_count", 2);
        store_u64(b"cv_strat_type:0", STRATEGY_LENDING as u64);
        store_u64(b"cv_strat_type:1", STRATEGY_LENDING as u64);
        store_u64(b"cv_strat_alloc:0", u64::MAX);
        store_u64(b"cv_strat_alloc:1", 0);
        store_u64(b"cv_risk_tier", RISK_AGGRESSIVE as u64);

        assert_eq!(update_strategy_allocation(admin.as_ptr(), 1, 1), 6);
        assert_eq!(load_u64(b"cv_strat_alloc:1"), 0);
    }

    #[test]
    fn test_deposit_rejection_precedes_mt20_custody_call() {
        setup();
        let admin = [1u8; 32];
        let user = [2u8; 32];
        let token = [0xCC; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_licn_token(admin.as_ptr(), token.as_ptr()), 0);
        assert_eq!(set_deposit_cap(admin.as_ptr(), 50_000), 0);

        test_mock::set_caller(user);
        assert_eq!(deposit(user.as_ptr(), 100_000), 0);
        assert!(test_mock::get_last_cross_call().is_none());
        assert_eq!(load_u64(b"cv_total_assets"), 0);
        assert_eq!(load_u64(b"cv_protocol_fees"), 0);
    }

    #[test]
    fn test_failed_transfer_preserves_exact_recalled_components() {
        setup();
        let admin = [1u8; 32];
        let user = [2u8; 32];
        let token = [0xCC; 32];
        let thalllend = [0xAA; 32];
        let zero = [0u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_licn_token(admin.as_ptr(), token.as_ptr()), 0);
        assert_eq!(set_withdrawal_fee(admin.as_ptr(), 0), 0);
        assert_eq!(
            set_protocol_addresses(admin.as_ptr(), thalllend.as_ptr(), zero.as_ptr()),
            0
        );
        assert_eq!(add_strategy(admin.as_ptr(), STRATEGY_LENDING, 30), 0);

        let share_key = make_key(b"cv_shares:", &hex_encode_addr(&user));
        store_u64(&share_key, 1_000);
        store_u64(b"cv_total_shares", 1_000);
        store_u64(IDLE_ASSETS_KEY, 100);
        store_u64(LENDING_ASSETS_KEY, 900);
        store_u64(b"cv_total_assets", 1_000);
        store_u64(b"cv_strat_deployed:0", 900);

        test_mock::set_caller(user);
        test_mock::set_cross_call_responses(std::vec![
            u64_to_bytes(900).to_vec(),
            0u32.to_le_bytes().to_vec(),
            u64_to_bytes(500).to_vec(),
            2u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(withdraw(user.as_ptr(), 500), 0);
        assert_eq!(load_u64(&share_key), 1_000);
        assert_eq!(load_u64(b"cv_total_shares"), 1_000);
        assert_eq!(load_u64(b"cv_total_assets"), 1_000);
        assert_eq!(load_u64(IDLE_ASSETS_KEY), 500);
        assert_eq!(load_u64(LENDING_ASSETS_KEY), 500);
        assert_eq!(load_u64(b"cv_strat_deployed:0"), 500);
    }

    #[test]
    fn test_pause_blocks_permissionless_strategy_movement() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(cv_pause(admin.as_ptr()), 0);

        assert_eq!(harvest(), 2);
        assert_eq!(rebalance(), 2);
        assert!(!reentrancy_engaged());
        assert!(test_mock::get_last_cross_call().is_none());
    }

    #[test]
    fn test_noncanonical_strategy_type_and_oversized_count_fail_closed() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        store_u64(b"cv_strategy_count", 1);
        store_u64(b"cv_strat_type:0", 257);
        store_u64(b"cv_strat_alloc:0", 10);

        assert_eq!(lending_strategy(), Err(96));
        assert_eq!(set_risk_tier(admin.as_ptr(), RISK_AGGRESSIVE), 4);
        assert_eq!(remove_strategy(admin.as_ptr(), 0), 4);
        assert_eq!(update_strategy_allocation(admin.as_ptr(), 0, 10), 6);

        store_u64(b"cv_strategy_count", MAX_STRATEGIES as u64 + 1);
        assert_eq!(get_strategy_info(0), 1);
    }

    #[test]
    fn test_get_user_position_null_pointer_rejected() {
        setup();
        assert_eq!(get_user_position(core::ptr::null()), 98);
    }

    #[test]
    fn test_component_sum_overflow_is_rejected() {
        setup();
        store_u64(IDLE_ASSETS_KEY, u64::MAX);
        store_u64(LENDING_ASSETS_KEY, 1);
        assert_eq!(store_vault_assets_from_components(), Err(91));
    }
}
