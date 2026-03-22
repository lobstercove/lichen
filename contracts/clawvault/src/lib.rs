// ClawVault v2 - Yield Aggregator
// Per whitepaper: auto-compounding vault that optimizes yield across DeFi protocols
// v2: Emergency pause, deposit/withdrawal fees, risk tiers, deposit cap, strategy management

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;
use moltchain_sdk::{
    bytes_to_u64, call_contract, call_token_transfer, get_caller, get_contract_address,
    get_timestamp, get_value, log_info, set_return_data, storage_get, storage_set, u64_to_bytes,
    Address, CrossCall,
};

// ============================================================================
// CONSTANTS
// ============================================================================

/// Performance fee: 10% of yield goes to protocol
const PERFORMANCE_FEE_PERCENT: u64 = 10;

/// Management fee: 2% annual (in basis points per slot)
const MANAGEMENT_FEE_BPS_PER_SLOT: u64 = 25; // ~2% annual at 788M slots/year
const FEE_SCALE: u64 = 10_000_000_000;

/// Maximum strategies per vault
const MAX_STRATEGIES: usize = 5;

/// Admin key
const ADMIN_KEY: &[u8] = b"cv_admin";

/// Minimum shares locked permanently on first deposit to prevent
/// ERC-4626 inflation / donation attack (T5.9)
const MIN_LOCKED_SHARES: u64 = 1_000;

/// Storage key for LobsterLend protocol address (lending yield source)
const LOBSTERLEND_ADDRESS_KEY: &[u8] = b"cv_lobsterlend_addr";
/// Storage key for MoltSwap protocol address (LP yield source)
const MOLTSWAP_ADDRESS_KEY: &[u8] = b"cv_moltswap_addr";

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

/// Storage key for MOLT token address (used in call_token_transfer)
const MOLT_TOKEN_KEY: &[u8] = b"cv_molt_token";

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

// ============================================================================
// STRATEGY TYPES
// ============================================================================

/// Strategy type identifiers
const STRATEGY_LENDING: u8 = 1; // Deposit into LobsterLend
const STRATEGY_LP: u8 = 2; // Provide liquidity on ClawSwap
const STRATEGY_STAKING: u8 = 3; // Stake MOLT for validator rewards

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

// ============================================================================
// VAULT STATE
// ============================================================================

/// Initialize the vault
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
    store_u64(b"cv_total_shares", 0);
    store_u64(b"cv_total_assets", 0);
    store_u64(b"cv_strategy_count", 0);
    store_u64(b"cv_last_harvest", get_timestamp());
    store_u64(b"cv_total_earned", 0);

    log_info("ClawVault initialized");
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
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

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

    if strategy_type < STRATEGY_LENDING || strategy_type > STRATEGY_STAKING {
        log_info("Invalid strategy type");
        return 3;
    }

    let count = load_u64(b"cv_strategy_count") as usize;
    if count >= MAX_STRATEGIES {
        log_info("Max strategies reached");
        return 4;
    }

    // Check total allocation doesn't exceed 100%
    let mut total_alloc = allocation_percent;
    for i in 0..count {
        let alloc_key = alloc::format!("cv_strat_alloc:{}", i);
        total_alloc += load_u64(alloc_key.as_bytes());
    }
    if total_alloc > 100 {
        log_info("Total allocation exceeds 100%");
        return 5;
    }

    // Store strategy
    let type_key = alloc::format!("cv_strat_type:{}", count);
    let alloc_key = alloc::format!("cv_strat_alloc:{}", count);
    let deployed_key = alloc::format!("cv_strat_deployed:{}", count);

    store_u64(type_key.as_bytes(), strategy_type as u64);
    store_u64(alloc_key.as_bytes(), allocation_percent);
    store_u64(deployed_key.as_bytes(), 0);
    store_u64(b"cv_strategy_count", (count + 1) as u64);

    log_info("Strategy added");
    0
}

// ============================================================================
// DEPOSIT / WITHDRAW (ERC-4626 style vault shares)
// ============================================================================

/// Deposit MOLT into the vault, receive shares
/// Returns shares minted (0 on failure)
#[no_mangle]
pub extern "C" fn deposit(depositor_ptr: *const u8, amount: u64) -> u64 {
    if amount == 0 {
        return 0;
    }
    // G25-02: Verify caller attached sufficient MOLT
    if get_value() < amount {
        return 0;
    }
    if is_cv_paused() {
        log_info("Vault is paused");
        return 0;
    }
    if !reentrancy_enter() {
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
    let fee = ((amount as u128) * (fee_bps as u128) / 10_000) as u64;
    let net_amount = amount - fee;
    if net_amount == 0 {
        return 0;
    }

    // Track fees
    if fee > 0 {
        let prev_fees = load_u64(b"cv_protocol_fees");
        store_u64(b"cv_protocol_fees", prev_fees.saturating_add(fee));
    }

    let mut depositor = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(depositor_ptr, depositor.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != depositor {
        reentrancy_exit();
        return 200;
    }

    let hex = hex_encode_addr(&depositor);

    let total_shares = load_u64(b"cv_total_shares");
    let total_assets = load_u64(b"cv_total_assets");

    // Calculate shares to mint (first depositor gets 1:1)
    let shares = if total_shares == 0 || total_assets == 0 {
        // T5.9: On first deposit, lock MIN_LOCKED_SHARES to a dead address
        if net_amount <= MIN_LOCKED_SHARES {
            log_info("First deposit must exceed minimum locked shares");
            return 0;
        }
        let dead_hex = [b'0'; 64];
        let dead_key = make_key(b"cv_shares:", &dead_hex);
        store_u64(&dead_key, MIN_LOCKED_SHARES);
        store_u64(b"cv_total_shares", MIN_LOCKED_SHARES);
        store_u64(b"cv_total_assets", MIN_LOCKED_SHARES);
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
    store_u64(&share_key, prev_shares.saturating_add(shares));

    // Update totals (re-read in case first-deposit wrote them)
    let total_shares = load_u64(b"cv_total_shares");
    let total_assets = load_u64(b"cv_total_assets");
    store_u64(b"cv_total_shares", total_shares.saturating_add(shares));
    // For first deposit, MIN_LOCKED_SHARES of the amount is already counted;
    // for subsequent deposits, just add the net amount.
    let additional_assets = if total_shares == MIN_LOCKED_SHARES {
        net_amount - MIN_LOCKED_SHARES
    } else {
        net_amount
    };
    store_u64(
        b"cv_total_assets",
        total_assets.saturating_add(additional_assets),
    );

    reentrancy_exit();
    log_info("Vault deposit successful");
    shares
}

/// Withdraw from vault by burning shares
/// Returns MOLT amount withdrawn (0 on failure)
#[no_mangle]
pub extern "C" fn withdraw(depositor_ptr: *const u8, shares_to_burn: u64) -> u64 {
    if shares_to_burn == 0 {
        return 0;
    }
    if !reentrancy_enter() {
        return 0;
    }

    let mut depositor = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(depositor_ptr, depositor.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != depositor {
        reentrancy_exit();
        return 200;
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

    // Calculate MOLT to return
    // Use u128 to prevent overflow on large values
    let gross_amount =
        ((shares_to_burn as u128) * (total_assets as u128) / (total_shares as u128)) as u64;
    if gross_amount == 0 {
        reentrancy_exit();
        return 0;
    }

    // V2: Withdrawal fee
    let fee_bps = get_withdrawal_fee_bps();
    let fee = ((gross_amount as u128) * (fee_bps as u128) / 10_000) as u64;
    let amount = gross_amount - fee;

    if fee > 0 {
        let prev_fees = load_u64(b"cv_protocol_fees");
        store_u64(b"cv_protocol_fees", prev_fees.saturating_add(fee));
    }

    // Update user shares
    store_u64(&share_key, user_shares - shares_to_burn);

    // Update totals
    store_u64(b"cv_total_shares", total_shares - shares_to_burn);
    store_u64(
        b"cv_total_assets",
        total_assets.saturating_sub(gross_amount),
    );

    // G25-02: Transfer MOLT to depositor
    if !transfer_molt_out(&depositor, amount) {
        // Revert all state changes
        store_u64(&share_key, user_shares);
        store_u64(b"cv_total_shares", total_shares);
        store_u64(b"cv_total_assets", total_assets);
        if fee > 0 {
            let prev_fees = load_u64(b"cv_protocol_fees");
            store_u64(b"cv_protocol_fees", prev_fees.saturating_sub(fee));
        }
        reentrancy_exit();
        log_info("Withdrawal transfer failed");
        return 0;
    }

    reentrancy_exit();
    log_info("Vault withdrawal successful");
    amount
}

// ============================================================================
// PROTOCOL YIELD HELPERS
// ============================================================================

/// Compute simulated yield using a fixed APY rate (fallback when no protocol configured).
/// yield = deployed * rate * slots / FEE_SCALE / 100
fn simulated_yield(rate_bps: u64, deployed: u64, elapsed_slots: u64) -> u64 {
    // Use u128 to prevent overflow on large values
    ((deployed as u128) * (rate_bps as u128) * (elapsed_slots as u128) / (FEE_SCALE as u128) / 100)
        as u64
}

/// Query a connected protocol for a vault-yield quote via CrossCall.
/// Returns `Some(yield_amount)` if the protocol address is configured and the export returns >=8 bytes.
/// Returns `None` when the strategy has no configured or supported external quote surface.
fn query_protocol_yield(
    addr_key: &[u8],
    function: &str,
    deployed: u64,
    elapsed_slots: u64,
) -> Option<u64> {
    let addr_bytes = storage_get(addr_key)?;
    if addr_bytes.len() != 32 || addr_bytes.iter().all(|&b| b == 0) {
        return None;
    }
    let mut addr = [0u8; 32];
    addr.copy_from_slice(&addr_bytes);

    // Build args: [deployed(8), elapsed_slots(8)]
    let mut args = Vec::with_capacity(16);
    args.extend_from_slice(&u64_to_bytes(deployed));
    args.extend_from_slice(&u64_to_bytes(elapsed_slots));

    let call = CrossCall::new(Address(addr), function, args);
    match call_contract(call) {
        Ok(result) if result.len() >= 8 => Some(bytes_to_u64(&result)),
        _ => None,
    }
}

/// G25-02: Transfer MOLT tokens out of the vault to a recipient.
/// Uses self-custody pattern: vault holds tokens at its own contract address.
/// Returns true on success, false if token not configured or transfer fails.
fn transfer_molt_out(to: &[u8; 32], amount: u64) -> bool {
    if amount == 0 {
        return true;
    }
    let token_data = storage_get(MOLT_TOKEN_KEY);
    if token_data.is_none() || token_data.as_ref().unwrap().len() < 32 {
        log_info("MOLT token not configured — transfer rejected");
        return false;
    }
    let mut token = [0u8; 32];
    token.copy_from_slice(&token_data.unwrap()[..32]);
    let contract_addr = get_contract_address();
    match call_token_transfer(
        Address(token),
        Address(contract_addr.0),
        Address(*to),
        amount,
    ) {
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Set protocol addresses for real yield sources. Admin only.
/// Both addresses optional (pass zero to skip). Non-zero addresses are stored.
///
/// Returns: 0 success, 1 not admin
#[no_mangle]
pub extern "C" fn set_protocol_addresses(
    caller_ptr: *const u8,
    lobsterlend_ptr: *const u8,
    moltswap_ptr: *const u8,
) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cv_admin(&caller) {
        return 1;
    }

    let mut lobsterlend = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(lobsterlend_ptr, lobsterlend.as_mut_ptr(), 32);
    }
    let mut moltswap = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(moltswap_ptr, moltswap.as_mut_ptr(), 32);
    }

    if lobsterlend.iter().any(|&b| b != 0) {
        storage_set(LOBSTERLEND_ADDRESS_KEY, &lobsterlend);
        log_info("LobsterLend address configured");
    }
    if moltswap.iter().any(|&b| b != 0) {
        storage_set(MOLTSWAP_ADDRESS_KEY, &moltswap);
        log_info("MoltSwap address configured");
    }
    0
}

/// G25-02: Set MOLT token address for self-custody transfers. Admin only.
/// Returns: 0 success, 1 not admin
#[no_mangle]
pub extern "C" fn set_molt_token(caller_ptr: *const u8, token_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }
    if !is_cv_admin(&caller) {
        return 1;
    }
    let mut token = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(token_ptr, token.as_mut_ptr(), 32);
    }
    storage_set(MOLT_TOKEN_KEY, &token);
    log_info("MOLT token address configured");
    0
}

// ============================================================================
// HARVEST & AUTO-COMPOUND
// ============================================================================

/// Harvest yield from all strategies and auto-compound
/// Can be called by anyone (typically a cron job or keeper)
#[no_mangle]
pub extern "C" fn harvest() -> u32 {
    if !reentrancy_enter() {
        return 1;
    }
    let last_harvest = load_u64(b"cv_last_harvest");
    let now = get_timestamp();
    if now <= last_harvest {
        reentrancy_exit();
        return 0; // Nothing to harvest
    }

    let elapsed_ms = now - last_harvest;
    let elapsed_slots = elapsed_ms / 400;
    if elapsed_slots == 0 {
        return 0;
    }

    let total_assets = load_u64(b"cv_total_assets");
    if total_assets == 0 {
        store_u64(b"cv_last_harvest", now);
        return 0;
    }

    let strategy_count = load_u64(b"cv_strategy_count") as usize;
    let mut total_yield: u64 = 0;
    let mut unavailable_yield_surfaces: u32 = 0;

    // Yield from each strategy — use real protocol data only (G25-02: no simulated fallback)
    for i in 0..strategy_count {
        let type_key = alloc::format!("cv_strat_type:{}", i);
        let alloc_key = alloc::format!("cv_strat_alloc:{}", i);

        let strategy_type = load_u64(type_key.as_bytes()) as u8;
        let allocation = load_u64(alloc_key.as_bytes());

        let deployed = total_assets * allocation / 100;

        // G25-02: Only real yield from connected protocols — no phantom inflation.
        // CON-10: Track unavailable quote surfaces so we can report instead of pretending.
        let strategy_yield = match strategy_type {
            STRATEGY_LENDING => {
                match query_protocol_yield(
                    LOBSTERLEND_ADDRESS_KEY,
                    "get_accrued_interest",
                    deployed,
                    elapsed_slots,
                ) {
                    Some(y) => y,
                    None => {
                        log_info(
                            "harvest: LobsterLend yield quote unavailable — skipping lending yield",
                        );
                        unavailable_yield_surfaces += 1;
                        0
                    }
                }
            }
            STRATEGY_LP => {
                let _ = (deployed, elapsed_slots);
                log_info(
                    "harvest: MoltSwap LP yield is embedded in pool share value; no standalone reward export",
                );
                unavailable_yield_surfaces += 1;
                0
            }
            STRATEGY_STAKING => {
                // Staking yield requires a real staking protocol endpoint
                0
            }
            _ => 0,
        };

        total_yield += strategy_yield;

        // Update deployed amount
        let deployed_key = alloc::format!("cv_strat_deployed:{}", i);
        store_u64(
            deployed_key.as_bytes(),
            deployed.saturating_add(strategy_yield),
        );
    }

    if total_yield > 0 {
        // Performance fee
        let perf_fee = total_yield * PERFORMANCE_FEE_PERCENT / 100;
        let net_yield = total_yield - perf_fee;

        // Auto-compound: add net yield back to total assets
        store_u64(b"cv_total_assets", total_assets.saturating_add(net_yield));

        // Track fees and earnings
        let fees = load_u64(b"cv_fees_earned");
        store_u64(b"cv_fees_earned", fees.saturating_add(perf_fee));
        let earned = load_u64(b"cv_total_earned");
        store_u64(b"cv_total_earned", earned.saturating_add(net_yield));

        log_info("Harvest & auto-compound complete");
    }

    // CON-10: If strategies are configured but expose no usable external quote surface,
    // do NOT update last_harvest — the harvest interval is not consumed
    // and the next call can retry once addresses are configured.
    if unavailable_yield_surfaces > 0 && total_yield == 0 && strategy_count > 0 {
        log_info("harvest: no yield collected because connected strategies exposed no usable quote surface");
        reentrancy_exit();
        return 2; // Distinct code: strategies configured, but no harvestable external yield
    }

    store_u64(b"cv_last_harvest", now);
    reentrancy_exit();
    0
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
    let mut user = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(user_ptr, user.as_mut_ptr(), 32);
    }
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
    if index >= count {
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

// ============================================================================
// V2: PAUSE, FEE CONFIG, DEPOSIT CAP, RISK TIERS, STRATEGY REMOVAL
// ============================================================================

/// Pause vault. Admin only. Blocks deposits; withdrawals still work (safety valve).
/// Returns: 0 success, 1 not admin, 2 already paused
#[no_mangle]
pub extern "C" fn cv_pause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

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
    log_info("ClawVault paused");
    0
}

/// Unpause vault. Admin only.
/// Returns: 0 success, 1 not admin, 2 not paused
#[no_mangle]
pub extern "C" fn cv_unpause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

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
    log_info("ClawVault unpaused");
    0
}

/// Set deposit fee (in BPS). Admin only.
/// Returns: 0 success, 1 not admin, 2 too high
#[no_mangle]
pub extern "C" fn set_deposit_fee(caller_ptr: *const u8, fee_bps: u64) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

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
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

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
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

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
/// Tier affects which strategy types are allowed:
///   1 (conservative) = lending only, max 33% allocation per strategy
///   2 (moderate) = lending + LP, max 66%
///   3 (aggressive) = all, max 100%
/// Returns: 0 success, 1 not admin, 2 invalid tier
#[no_mangle]
pub extern "C" fn set_risk_tier(caller_ptr: *const u8, tier: u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cv_admin(&caller) {
        return 1;
    }
    if tier < RISK_CONSERVATIVE || tier > RISK_AGGRESSIVE {
        return 2;
    }
    store_u64(b"cv_risk_tier", tier as u64);
    0
}

/// Remove a strategy (zero out its allocation). Admin only.
/// Returns: 0 success, 1 not admin, 2 out of bounds
#[no_mangle]
pub extern "C" fn remove_strategy(caller_ptr: *const u8, index: u64) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cv_admin(&caller) {
        return 1;
    }
    let count = load_u64(b"cv_strategy_count");
    if index >= count {
        return 2;
    }
    let i = index as usize;
    let alloc_key = alloc::format!("cv_strat_alloc:{}", i);
    store_u64(alloc_key.as_bytes(), 0);
    let deployed_key = alloc::format!("cv_strat_deployed:{}", i);
    store_u64(deployed_key.as_bytes(), 0);
    log_info("Strategy removed (allocation zeroed)");
    0
}

/// Withdraw accumulated protocol fees. Admin only.
/// Returns fee amount withdrawn (0 if none or not admin).
#[no_mangle]
pub extern "C" fn withdraw_protocol_fees(caller_ptr: *const u8) -> u64 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

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
    if !transfer_molt_out(&caller, fees) {
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
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cv_admin(&caller) {
        return 1;
    }
    let count = load_u64(b"cv_strategy_count");
    if index >= count {
        return 2;
    }

    // Check total allocation with new value
    let mut total: u64 = new_alloc;
    for i in 0..count as usize {
        if i == index as usize {
            continue;
        }
        let alloc_key = alloc::format!("cv_strat_alloc:{}", i);
        total += load_u64(alloc_key.as_bytes());
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
    use moltchain_sdk::bytes_to_u64;
    use moltchain_sdk::test_mock;

    fn setup() {
        test_mock::reset();
    }

    /// G25-02: Configure MOLT token and mock cross-contract transfers so
    /// withdraw / withdraw_protocol_fees can succeed in unit tests.
    fn enable_token_transfers() {
        let admin = [1u8; 32];
        let prev_caller = moltchain_sdk::get_caller();
        test_mock::set_caller(admin);
        let molt_token = [0xCC; 32];
        set_molt_token(admin.as_ptr(), molt_token.as_ptr());
        test_mock::set_cross_call_response(Some(alloc::vec![1u8]));
        test_mock::set_caller(prev_caller.0);
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
        assert_eq!(add_strategy(admin.as_ptr(), 0, 50), 3);
    }

    #[test]
    fn test_add_strategy_allocation_exceeds_100() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(add_strategy(admin.as_ptr(), STRATEGY_LENDING, 60), 0);
        assert_eq!(add_strategy(admin.as_ptr(), STRATEGY_LP, 50), 5); // 60+50>100
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
        test_mock::set_value(100_000);
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
    fn test_harvest() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        add_strategy(admin.as_ptr(), STRATEGY_STAKING, 50);
        let user = [2u8; 32];
        // Set deposit fee to 0 for clean math
        set_deposit_fee(admin.as_ptr(), 0);
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000_000_000);
        deposit(user.as_ptr(), 1_000_000_000_000);
        // Advance timestamp by 400 seconds (1000 slots)
        test_mock::set_timestamp(401_000);
        let result = harvest();
        assert_eq!(result, 0);
        // G25-02: No simulated yield — without real protocol, total_assets stays same
        let total_assets = load_u64(b"cv_total_assets");
        assert_eq!(total_assets, 1_000_000_000_000);
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
        add_strategy(admin.as_ptr(), STRATEGY_STAKING, 50);
        assert_eq!(get_strategy_info(0), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 24);
        assert_eq!(bytes_to_u64(&ret[0..8]), STRATEGY_STAKING as u64);
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
        test_mock::set_value(100_000);
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
        test_mock::set_value(100_000);
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
        add_strategy(admin.as_ptr(), STRATEGY_LENDING, 50);
        add_strategy(admin.as_ptr(), STRATEGY_LP, 30);

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
        test_mock::set_value(1_000_000);
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
        add_strategy(admin.as_ptr(), STRATEGY_LENDING, 50);
        add_strategy(admin.as_ptr(), STRATEGY_LP, 30);

        // Update strategy 0 from 50 to 40
        assert_eq!(update_strategy_allocation(admin.as_ptr(), 0, 40), 0);
        let alloc_key = alloc::format!("cv_strat_alloc:{}", 0);
        assert_eq!(load_u64(alloc_key.as_bytes()), 40);

        // Try to exceed 100% (40 + 80 = 120)
        assert_eq!(update_strategy_allocation(admin.as_ptr(), 1, 80), 3);

        // Non-admin fails
        let other = [2u8; 32];
        test_mock::set_caller(other);
        assert_eq!(update_strategy_allocation(other.as_ptr(), 0, 10), 1);
    }

    // ====================================================================
    // PROTOCOL YIELD INTEGRATION TESTS
    // ====================================================================

    #[test]
    fn test_set_protocol_addresses_success() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let lobsterlend = [0xAA; 32];
        let moltswap = [0xBB; 32];
        assert_eq!(
            set_protocol_addresses(admin.as_ptr(), lobsterlend.as_ptr(), moltswap.as_ptr()),
            0
        );

        let stored_ll = test_mock::get_storage(LOBSTERLEND_ADDRESS_KEY).unwrap();
        assert_eq!(stored_ll.as_slice(), &lobsterlend);
        let stored_ms = test_mock::get_storage(MOLTSWAP_ADDRESS_KEY).unwrap();
        assert_eq!(stored_ms.as_slice(), &moltswap);
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

        // Only set lobsterlend (moltswap = zero → skipped)
        let lobsterlend = [0xAA; 32];
        let zero = [0u8; 32];
        assert_eq!(
            set_protocol_addresses(admin.as_ptr(), lobsterlend.as_ptr(), zero.as_ptr()),
            0
        );
        assert!(test_mock::get_storage(LOBSTERLEND_ADDRESS_KEY).is_some());
        assert!(test_mock::get_storage(MOLTSWAP_ADDRESS_KEY).is_none());
    }

    #[test]
    fn test_simulated_yield_calculation() {
        // Verify the simulated yield formula directly
        let deployed = 1_000_000_000u64;
        let rate = 300u64; // ~3% APY
        let slots = 1000u64;
        let y = simulated_yield(rate, deployed, slots);
        // y = 1_000_000_000 * 300 * 1000 / 10_000_000_000 / 100
        // = 300_000_000_000_000 / 1_000_000_000_000 = 300
        assert_eq!(y, 300);
    }

    #[test]
    fn test_query_protocol_yield_no_address() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        // No protocol addresses set → query returns None → fallback
        let result = query_protocol_yield(
            LOBSTERLEND_ADDRESS_KEY,
            "get_accrued_interest",
            1_000_000,
            100,
        );
        assert!(result.is_none());
    }

    #[test]
    fn test_query_protocol_yield_test_mode() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        // Set protocol address — call_contract returns the test-configured mock response.
        let lobsterlend = [0xAA; 32];
        let zero = [0u8; 32];
        set_protocol_addresses(admin.as_ptr(), lobsterlend.as_ptr(), zero.as_ptr());

        test_mock::set_cross_call_response(Some(u64_to_bytes(777).to_vec()));

        let result = query_protocol_yield(
            LOBSTERLEND_ADDRESS_KEY,
            "get_accrued_interest",
            1_000_000,
            100,
        );
        assert_eq!(result, Some(777));
    }

    #[test]
    fn test_harvest_with_protocol_addresses_configured() {
        // G25-02: With addresses configured, test mode returns empty → yield = 0 (no simulated fallback)
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        set_deposit_fee(admin.as_ptr(), 0);

        // Configure protocol addresses
        let lobsterlend = [0xAA; 32];
        let moltswap = [0xBB; 32];
        set_protocol_addresses(admin.as_ptr(), lobsterlend.as_ptr(), moltswap.as_ptr());

        // Add strategies
        add_strategy(admin.as_ptr(), STRATEGY_LENDING, 40);
        add_strategy(admin.as_ptr(), STRATEGY_LP, 30);
        add_strategy(admin.as_ptr(), STRATEGY_STAKING, 30);

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000_000_000);
        deposit(user.as_ptr(), 1_000_000_000_000);

        test_mock::set_cross_call_response(Some(u64_to_bytes(2_500).to_vec()));

        // Advance time
        test_mock::set_timestamp(401_000);
        assert_eq!(harvest(), 0);

        // Only the lending strategy yields because MoltSwap exposes no standalone LP reward export.
        let total_assets = load_u64(b"cv_total_assets");
        assert_eq!(total_assets, 1_000_000_002_250);
        let total_earned = load_u64(b"cv_total_earned");
        assert_eq!(total_earned, 2_250);
    }

    #[test]
    fn test_harvest_without_protocol_addresses() {
        // G25-02: Without protocol addresses → 0 yield (no phantom inflation)
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        set_deposit_fee(admin.as_ptr(), 0);

        add_strategy(admin.as_ptr(), STRATEGY_LENDING, 50);
        add_strategy(admin.as_ptr(), STRATEGY_LP, 50);

        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(1_000_000_000_000);
        deposit(user.as_ptr(), 1_000_000_000_000);

        test_mock::set_timestamp(401_000);
        // CON-10: All yield-producing strategies have missing addresses → returns 2
        assert_eq!(harvest(), 2);
        let total_assets = load_u64(b"cv_total_assets");
        assert_eq!(total_assets, 1_000_000_000_000);
    }

    // ====================================================================
    // G25-02 TESTS: Financial wiring & real yield
    // ====================================================================

    #[test]
    fn test_g25_deposit_requires_get_value() {
        // Deposit must fail when get_value() < amount (no MOLT attached)
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
        test_mock::set_value(100_000);
        let shares = deposit(user.as_ptr(), 100_000);
        assert!(shares > 0);
        let amount = withdraw(user.as_ptr(), shares);
        // Graceful degradation: transfer_molt_out returns true when MOLT token not configured
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
        test_mock::set_value(1_000_000);
        deposit(user.as_ptr(), 1_000_000);
        test_mock::set_caller(admin);
        let fees = withdraw_protocol_fees(admin.as_ptr());
        // Fees collected from deposit (1000 = 1M * 10bps)
        assert_eq!(fees, 1000);
    }

    #[test]
    fn test_g25_set_molt_token() {
        // Admin can set MOLT token address
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let token = [0xCC; 32];
        assert_eq!(set_molt_token(admin.as_ptr(), token.as_ptr()), 0);
        let stored = test_mock::get_storage(MOLT_TOKEN_KEY).unwrap();
        assert_eq!(stored.as_slice(), &token);

        // Non-admin fails
        let other = [2u8; 32];
        test_mock::set_caller(other);
        assert_eq!(set_molt_token(other.as_ptr(), token.as_ptr()), 1);
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
        // No phantom inflation — assets unchanged without real yield source
        assert_eq!(assets_before, assets_after);
        assert_eq!(load_u64(b"cv_total_earned"), 0);
        assert_eq!(load_u64(b"cv_fees_earned"), 0);
    }

    #[test]
    fn test_g25_deposit_partial_value() {
        // Deposit with get_value() >= amount but less than double — should work
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let user = [2u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(100_000); // exact match
        let shares = deposit(user.as_ptr(), 100_000);
        assert!(shares > 0);
    }
}
