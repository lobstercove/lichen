// SporePump v2 - Token Launchpad with Bonding Curves
// Per whitepaper: fair-launch bonding curves for new token creation
// Automatic DEX graduation is reserved for a future release once SporePump
// tokens have a real asset/pool migration path into SporeSwap.
//
// v2 additions:
//   - Anti-manipulation: buy cooldown, max buy per tx, sell cooldown
//   - Creator royalties on trades
//   - Admin fee withdrawal
//   - Emergency pause
//   - Token freeze (admin can freeze malicious tokens)

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;
use alloc::vec::Vec;
use lichen_sdk::{
    bytes_to_u64, call_contract, can_receive, can_send, get_caller, get_contract_address,
    get_contract_code_hash, get_slot, get_timestamp, log_info, receive_token_or_native,
    set_return_data, storage, storage_get, storage_set, transfer_token_or_native, u64_to_bytes,
    Address, CrossCall,
};

// T5.12: Reentrancy guard
const REENTRANCY_KEY: &[u8] = b"_reentrancy";
const ERROR_RETURN: u64 = u64::MAX;

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

// ============================================================================
// CONSTANTS
// ============================================================================

/// Token creation fee: 10 LICN (10,000,000,000 spores — $1.00 at $0.10/LICN)
const CREATION_FEE: u64 = 10_000_000_000;

/// Default initial supply for bonding curve tokens
const DEFAULT_MAX_SUPPLY: u64 = 1_000_000_000_000_000_000; // 1B tokens * 10^9

/// Graduation threshold: when market cap reaches this, migrate to DEX
const GRADUATION_MARKET_CAP: u64 = 100_000_000_000_000; // 100K LICN ($10K at $0.10)

/// Bonding curve slope factor (controls price steepness)
/// price = BASE_PRICE + (supply_sold * SLOPE / SLOPE_SCALE)
const BASE_PRICE: u64 = 1_000; // 0.000001 LICN per token initially
const SLOPE: u64 = 1;
const SLOPE_SCALE: u64 = 1_000_000;

/// Platform fee on buys/sells: 1%
const PLATFORM_FEE_PERCENT: u64 = 1;

/// Admin key
const ADMIN_KEY: &[u8] = b"cp_admin";

// Token counter
const TOKEN_COUNT_KEY: &[u8] = b"cp_token_count";
const MAX_TOKEN_NAME_LEN: usize = 64;
const MAX_TOKEN_SYMBOL_LEN: usize = 12;

// ============================================================================
// v2 CONSTANTS
// ============================================================================

/// Buy cooldown: minimum slots between buys per user per token.
/// Slots are the deterministic contract clock (400ms target slot time).
const DEFAULT_BUY_COOLDOWN_SLOTS: u64 = 5; // ~2 seconds
/// Maximum LICN that can be spent in a single buy
const DEFAULT_MAX_BUY_AMOUNT: u64 = 100_000_000_000_000; // 100K LICN ($10K at $0.10)
/// Sell cooldown: minimum slots after buying before selling (anti-dump).
const DEFAULT_SELL_COOLDOWN_SLOTS: u64 = 13; // ~5.2 seconds
/// Creator royalty: basis points on each trade (default 50 = 0.5%)
const DEFAULT_CREATOR_ROYALTY_BPS: u64 = 50;
const BPS_SCALE: u64 = 10_000;
/// Emergency pause key
const PAUSE_KEY: &[u8] = b"cp_paused";

// ============================================================================
// DEX MIGRATION CONSTANTS
// ============================================================================

/// DEX core contract address (for creating trading pairs on graduation)
const DEX_CORE_ADDRESS_KEY: &[u8] = b"cp_dex_core_addr";
/// DEX AMM contract address (for creating liquidity pools on graduation)
const DEX_AMM_ADDRESS_KEY: &[u8] = b"cp_dex_amm_addr";
const DEX_ROUTER_ADDRESS_KEY: &[u8] = b"cp_dex_router_addr";
const GRADUATED_TOKEN_TEMPLATE_HASH_KEY: &[u8] = b"cp_grad_template_hash";
const GRADUATION_GOVERNANCE_KEY: &[u8] = b"cp_grad_governance";
const GRADUATION_TICK_SIZE_KEY: &[u8] = b"cp_grad_tick_size";
const GRADUATION_LOT_SIZE_KEY: &[u8] = b"cp_grad_lot_size";
const GRADUATION_MIN_ORDER_KEY: &[u8] = b"cp_grad_min_order";
const GRADUATION_AMM_FEE_TIER_KEY: &[u8] = b"cp_grad_amm_fee";
/// Percentage of raised LICN seeded as liquidity on graduation (80%)
const GRADUATION_LIQUIDITY_PERCENT: u64 = 80;
/// Percentage of raised LICN retained as platform revenue on graduation (20%)
const GRADUATION_PLATFORM_PERCENT: u64 = 20;
const MIGRATION_TIMEOUT_SLOTS: u64 = 9_000;

const GRADUATION_ACTIVE: u8 = 0;
const GRADUATION_ELIGIBLE: u8 = 1;
const GRADUATION_MIGRATING: u8 = 2;
const GRADUATION_GRADUATED: u8 = 3;

/// LICN token contract address (for outgoing transfers in sell/withdraw)
const LICN_TOKEN_KEY: &[u8] = b"cp_licn_token";

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

fn u64_to_hex(val: u64) -> [u8; 16] {
    let hex_chars = b"0123456789abcdef";
    let bytes = val.to_be_bytes();
    let mut hex = [0u8; 16];
    for i in 0..8 {
        hex[i * 2] = hex_chars[(bytes[i] >> 4) as usize];
        hex[i * 2 + 1] = hex_chars[(bytes[i] & 0x0f) as usize];
    }
    hex
}

fn make_key(prefix: &[u8], id_hex: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(prefix.len() + id_hex.len());
    key.extend_from_slice(prefix);
    key.extend_from_slice(id_hex);
    key
}

fn graduation_key(prefix: &[u8], token_id: u64) -> Vec<u8> {
    make_key(prefix, &u64_to_hex(token_id))
}

fn graduation_state(token_id: u64) -> u8 {
    storage_get(&graduation_key(b"cpgs:", token_id))
        .and_then(|data| data.first().copied())
        .unwrap_or(GRADUATION_ACTIVE)
}

fn set_graduation_state(token_id: u64, state: u8) {
    storage_set(&graduation_key(b"cpgs:", token_id), &[state]);
}

fn graduation_candidate(token_id: u64) -> Option<[u8; 32]> {
    storage_get(&graduation_key(b"cpgt:", token_id)).and_then(|data| {
        if data.len() < 32 {
            return None;
        }
        let mut address = [0u8; 32];
        address.copy_from_slice(&data[..32]);
        Some(address)
    })
}

fn set_graduation_u64(prefix: &[u8], token_id: u64, value: u64) {
    store_u64(&graduation_key(prefix, token_id), value);
}

fn get_graduation_u64(prefix: &[u8], token_id: u64) -> u64 {
    load_u64(&graduation_key(prefix, token_id))
}

fn token_record(token_id: u64) -> Option<Vec<u8>> {
    storage_get(&make_key(b"cpt:", &u64_to_hex(token_id)))
        .filter(|data| data.len() >= TOKEN_DATA_SIZE)
}

fn token_name_key(token_id: u64) -> Vec<u8> {
    graduation_key(b"cpn:", token_id)
}

fn token_symbol_key(token_id: u64) -> Vec<u8> {
    graduation_key(b"cpsy:", token_id)
}

fn token_symbol_index_key(symbol: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(6 + symbol.len());
    key.extend_from_slice(b"cpsym:");
    key.extend_from_slice(symbol);
    key
}

fn valid_token_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TOKEN_NAME_LEN
        && !name.first().is_some_and(u8::is_ascii_whitespace)
        && !name.last().is_some_and(u8::is_ascii_whitespace)
        && core::str::from_utf8(name).is_ok_and(|value| !value.chars().any(char::is_control))
}

fn normalize_token_symbol(symbol: &[u8]) -> Option<Vec<u8>> {
    if symbol.len() < 2
        || symbol.len() > MAX_TOKEN_SYMBOL_LEN
        || !symbol.first().is_some_and(u8::is_ascii_alphabetic)
        || !symbol.iter().all(u8::is_ascii_alphanumeric)
    {
        return None;
    }
    Some(symbol.iter().map(u8::to_ascii_uppercase).collect())
}

fn read_metadata_bytes(ptr: *const u8, len: u32, max_len: usize) -> Option<Vec<u8>> {
    let len = len as usize;
    if ptr.is_null() || len == 0 || len > max_len {
        return None;
    }
    Some(unsafe { core::slice::from_raw_parts(ptr, len) }.to_vec())
}

fn token_metadata(token_id: u64) -> (Vec<u8>, Vec<u8>) {
    let name = storage_get(&token_name_key(token_id))
        .filter(|value| valid_token_name(value))
        .unwrap_or_else(|| alloc::format!("Spore Token {}", token_id).into_bytes());
    let symbol = storage_get(&token_symbol_key(token_id))
        .and_then(|value| normalize_token_symbol(&value))
        .unwrap_or_else(|| alloc::format!("SPT{}", token_id).into_bytes());
    (name, symbol)
}

fn token_market_cap(data: &[u8]) -> u64 {
    let supply = bytes_to_u64(&data[32..40]);
    u128_to_u64_saturating(current_price(supply) as u128 * supply as u128 / 1_000_000_000u128)
}

fn integer_sqrt_u128(value: u128) -> u64 {
    if value == 0 {
        return 0;
    }
    let mut x = 1u128 << ((128 - value.leading_zeros() as usize + 1) / 2);
    loop {
        let next = (x + value / x) / 2;
        if next >= x {
            return x.min(u64::MAX as u128) as u64;
        }
        x = next;
    }
}

fn initial_sqrt_price(price: u64) -> u64 {
    let ratio_q64 = (price as u128).saturating_mul(1u128 << 64) / 1_000_000_000u128;
    integer_sqrt_u128(ratio_q64).max(1)
}

fn cross_call_id(target: [u8; 32], function: &str, args: Vec<u8>) -> Option<u64> {
    call_contract(CrossCall::new(Address(target), function, args))
        .ok()
        .filter(|response| response.len() >= 8)
        .map(|response| bytes_to_u64(&response[..8]))
        .filter(|id| *id != 0)
}

fn cross_call_succeeded(target: [u8; 32], function: &str, args: Vec<u8>) -> bool {
    call_contract(CrossCall::new(Address(target), function, args)).is_ok()
}

fn refresh_eligibility(token_id: u64, data: &[u8]) {
    let state = graduation_state(token_id);
    let eligible = token_market_cap(data) >= GRADUATION_MARKET_CAP;
    if eligible && state == GRADUATION_ACTIVE {
        set_graduation_state(token_id, GRADUATION_ELIGIBLE);
        set_graduation_u64(b"cpge:", token_id, get_slot());
        log_info("Launchpad token became graduation eligible");
    } else if !eligible && state == GRADUATION_ELIGIBLE {
        set_graduation_state(token_id, GRADUATION_ACTIVE);
        set_graduation_u64(b"cpge:", token_id, 0);
        log_info("Launchpad token returned below graduation threshold");
    }
}

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key).map(|d| bytes_to_u64(&d)).unwrap_or(0)
}

fn store_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
}

fn u128_to_u64_saturating(value: u128) -> u64 {
    if value > u64::MAX as u128 {
        u64::MAX
    } else {
        value as u64
    }
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

fn has_configured_address(key: &[u8]) -> bool {
    storage_get(key)
        .map(|data| data.len() == 32)
        .unwrap_or(false)
}

fn configured_address(key: &[u8]) -> Option<[u8; 32]> {
    storage_get(key).and_then(|data| {
        if data.len() != 32 || data.iter().all(|byte| *byte == 0) {
            return None;
        }
        let mut address = [0u8; 32];
        address.copy_from_slice(&data);
        Some(address)
    })
}

fn read_address(ptr: *const u8) -> [u8; 32] {
    let mut address = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(ptr, address.as_mut_ptr(), 32);
    }
    address
}

fn graduation_claim_key(token_id: u64, holder: &[u8; 32]) -> Vec<u8> {
    let token_hex = u64_to_hex(token_id);
    let holder_hex = hex_encode_addr(holder);
    let mut key = Vec::with_capacity(5 + token_hex.len() + 1 + holder_hex.len());
    key.extend_from_slice(b"cpgc:");
    key.extend_from_slice(&token_hex);
    key.push(b':');
    key.extend_from_slice(&holder_hex);
    key
}

fn is_token_frozen(token_id: u64) -> bool {
    let id_hex = u64_to_hex(token_id);
    let key = make_key(b"cpf:", &id_hex);
    storage_get(&key)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
}

fn launchpad_balance_key(token_id: u64, account: &[u8; 32]) -> Vec<u8> {
    let id_hex = u64_to_hex(token_id);
    let account_hex = hex_encode_addr(account);
    let mut key = Vec::with_capacity(4 + 16 + 1 + 64);
    key.extend_from_slice(b"bal:");
    key.extend_from_slice(&id_hex);
    key.push(b':');
    key.extend_from_slice(&account_hex);
    key
}

fn account_address(account: &[u8; 32]) -> Address {
    Address::new(*account)
}

fn launchpad_can_receive(account: &[u8; 32], amount: u64, balance: u64) -> bool {
    can_receive(
        get_contract_address(),
        account_address(account),
        amount,
        balance,
    )
}

fn launchpad_can_send(account: &[u8; 32], amount: u64, balance: u64) -> bool {
    can_send(
        get_contract_address(),
        account_address(account),
        amount,
        balance,
    )
}

fn last_buy_key(token_id: u64, buyer_hex: &[u8; 64]) -> Vec<u8> {
    let id_hex = u64_to_hex(token_id);
    let mut key = Vec::with_capacity(4 + 16 + 1 + 64);
    key.extend_from_slice(b"lbk:");
    key.extend_from_slice(&id_hex);
    key.push(b':');
    key.extend_from_slice(buyer_hex);
    key
}

fn get_buy_cooldown() -> u64 {
    storage_get(b"cp_buy_cooldown")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_BUY_COOLDOWN_SLOTS)
}

fn get_sell_cooldown() -> u64 {
    storage_get(b"cp_sell_cooldown")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_SELL_COOLDOWN_SLOTS)
}

fn get_max_buy() -> u64 {
    storage_get(b"cp_max_buy")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_MAX_BUY_AMOUNT)
}

fn get_creator_royalty() -> u64 {
    storage_get(b"cp_creator_royalty")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_CREATOR_ROYALTY_BPS)
}

/// G24-01: Transfer LICN tokens from the contract to a recipient (self-custody).
/// Returns true on success, false if token address not configured or call errors.
fn transfer_licn_out(recipient: &[u8; 32], amount: u64) -> bool {
    let token_data = match storage_get(LICN_TOKEN_KEY) {
        Some(data) if data.len() == 32 => data,
        _ => {
            // AUDIT-FIX CON-05: MUST fail when LICN token address is not configured.
            // Returning true here would silently succeed without transferring funds,
            // causing sells/withdrawals to appear successful with no actual payout.
            log_info("CRITICAL: LICN token address not configured — transfer REJECTED");
            return false;
        }
    };
    let mut token = [0u8; 32];
    token.copy_from_slice(&token_data);
    let self_addr = get_contract_address();
    match transfer_token_or_native(Address(token), self_addr, Address(*recipient), amount) {
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

fn load_licn_token_or_native() -> Address {
    storage_get(LICN_TOKEN_KEY)
        .and_then(|data| {
            if data.len() != 32 {
                return None;
            }
            let mut token = [0u8; 32];
            token.copy_from_slice(&data);
            Some(Address(token))
        })
        .unwrap_or(Address([0u8; 32]))
}

// ============================================================================
// TOKEN LAUNCH LAYOUT (stored per token)
// ============================================================================
// Key: "cpt:{token_id_hex}" → [creator(32), supply_sold(8), licn_raised(8),
//                                max_supply(8), created_at(8), graduated(1)]
// Total: 65 bytes

const TOKEN_DATA_SIZE: usize = 65;

// ============================================================================
// INITIALIZATION
// ============================================================================

/// Initialize SporePump
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
    store_u64(TOKEN_COUNT_KEY, 0);
    store_u64(b"cp_fees_collected", 0);

    log_info("SporePump initialized");
    0
}

// ============================================================================
// TOKEN CREATION
// ============================================================================

/// Create a new token on the bonding curve
/// Returns token ID. Validation failures return ERROR_RETURN so the host reverts value transfers.
#[no_mangle]
pub extern "C" fn create_token(creator_ptr: *const u8, fee_paid: u64) -> u64 {
    create_token_internal(creator_ptr, fee_paid, None)
}

/// Create a named token while preserving the legacy create_token entrypoint.
/// Symbols are unique, normalized uppercase ASCII identifiers.
#[no_mangle]
pub extern "C" fn create_token_with_metadata(
    creator_ptr: *const u8,
    name_ptr: *const u8,
    name_len: u32,
    symbol_ptr: *const u8,
    symbol_len: u32,
    fee_paid: u64,
) -> u64 {
    let Some(name) = read_metadata_bytes(name_ptr, name_len, MAX_TOKEN_NAME_LEN) else {
        return ERROR_RETURN;
    };
    let Some(symbol) = read_metadata_bytes(symbol_ptr, symbol_len, MAX_TOKEN_SYMBOL_LEN) else {
        return ERROR_RETURN;
    };
    if !valid_token_name(&name) {
        return ERROR_RETURN;
    }
    let Some(symbol) = normalize_token_symbol(&symbol) else {
        return ERROR_RETURN;
    };
    create_token_internal(creator_ptr, fee_paid, Some((name, symbol)))
}

fn create_token_internal(
    creator_ptr: *const u8,
    fee_paid: u64,
    requested_metadata: Option<(Vec<u8>, Vec<u8>)>,
) -> u64 {
    if creator_ptr.is_null() {
        return ERROR_RETURN;
    }
    let mut creator = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(creator_ptr, creator.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != creator {
        return 200;
    }

    if fee_paid < CREATION_FEE {
        log_info("Insufficient creation fee (need 10 LICN)");
        return ERROR_RETURN;
    }

    let token_id = match load_u64(TOKEN_COUNT_KEY).checked_add(1) {
        Some(id) => id,
        None => {
            log_info("Token counter overflow");
            return ERROR_RETURN;
        }
    };
    let id_hex = u64_to_hex(token_id);
    let (name, symbol) = requested_metadata.unwrap_or_else(|| {
        (
            alloc::format!("Spore Token {}", token_id).into_bytes(),
            alloc::format!("SPT{}", token_id).into_bytes(),
        )
    });
    let symbol_index_key = token_symbol_index_key(&symbol);
    if storage_get(&symbol_index_key).is_some() {
        log_info("Token symbol is already registered");
        return ERROR_RETURN;
    }
    let fees = match load_u64(b"cp_fees_collected").checked_add(CREATION_FEE) {
        Some(fees) => fees,
        None => {
            log_info("Creation fee counter overflow");
            return ERROR_RETURN;
        }
    };

    // G24-01: Verify actual payment instead of trusting the parameter.
    let payment_token = load_licn_token_or_native();
    if !receive_token_or_native(
        payment_token,
        Address(creator),
        get_contract_address(),
        CREATION_FEE,
    )
    .unwrap_or(false)
    {
        log_info("Insufficient creation fee (need 10 LICN)");
        return ERROR_RETURN;
    }

    // Store token data
    let mut data = Vec::with_capacity(TOKEN_DATA_SIZE);
    data.extend_from_slice(&creator); // creator: 32 bytes
    data.extend_from_slice(&u64_to_bytes(0)); // supply_sold: 0
    data.extend_from_slice(&u64_to_bytes(0)); // licn_raised: 0
    data.extend_from_slice(&u64_to_bytes(DEFAULT_MAX_SUPPLY)); // max_supply
    data.extend_from_slice(&u64_to_bytes(get_timestamp())); // created_at
    data.push(0); // graduated: false

    let token_key = make_key(b"cpt:", &id_hex);
    storage_set(&token_key, &data);
    storage_set(&token_name_key(token_id), &name);
    storage_set(&token_symbol_key(token_id), &symbol);
    storage_set(&symbol_index_key, &u64_to_bytes(token_id));
    store_u64(TOKEN_COUNT_KEY, token_id);

    // Collect creation fee
    store_u64(b"cp_fees_collected", fees);

    log_info("🪙 New token created on bonding curve");
    token_id
}

/// Return name_len(u16) + name + symbol_len(u16) + symbol for any token ID.
#[no_mangle]
pub extern "C" fn get_token_metadata(token_id: u64) -> u32 {
    if token_record(token_id).is_none() {
        return 1;
    }
    let (name, symbol) = token_metadata(token_id);
    let mut result = Vec::with_capacity(4 + name.len() + symbol.len());
    result.extend_from_slice(&(name.len() as u16).to_le_bytes());
    result.extend_from_slice(&name);
    result.extend_from_slice(&(symbol.len() as u16).to_le_bytes());
    result.extend_from_slice(&symbol);
    set_return_data(&result);
    0
}

// ============================================================================
// BONDING CURVE MATH
// ============================================================================

/// Calculate price for buying `amount` tokens given current supply
/// Uses linear bonding curve: price = BASE_PRICE + supply * SLOPE / SLOPE_SCALE
/// Cost = integral from supply to supply+amount of price(s) ds
///      = BASE_PRICE * amount + SLOPE/(2*SLOPE_SCALE) * ((supply+amount)^2 - supply^2)
/// Using u128 intermediates to avoid overflow.
fn calculate_buy_cost(supply_sold: u64, amount: u64) -> u64 {
    let s = supply_sold as u128;
    let a = amount as u128;
    let base = BASE_PRICE as u128;
    let slope = SLOPE as u128;
    let scale = SLOPE_SCALE as u128;
    let norm = 1_000_000_000u128;

    // Integral: base*amount + slope * ((s+a)^2 - s^2) / (2 * scale)
    //         = base*amount + slope * a * (2*s + a) / (2 * scale)
    let linear_part = base.saturating_mul(a);
    let two_s_plus_a = s.saturating_mul(2).saturating_add(a);
    let quadratic_part = slope.saturating_mul(a).saturating_mul(two_s_plus_a) / (2 * scale);
    u128_to_u64_saturating(linear_part.saturating_add(quadratic_part) / norm)
}

/// Calculate refund for selling `amount` tokens given current supply
/// Same integral formula, computed from (supply-amount) to supply.
fn calculate_sell_refund(supply_sold: u64, amount: u64) -> u64 {
    if amount > supply_sold {
        return 0;
    }
    let s = supply_sold as u128;
    let a = amount as u128;
    let base = BASE_PRICE as u128;
    let slope = SLOPE as u128;
    let scale = SLOPE_SCALE as u128;
    let norm = 1_000_000_000u128;

    // Integral from (s-a) to s = base*a + slope * a * (2*s - a) / (2 * scale)
    let linear_part = base.saturating_mul(a);
    let two_s_minus_a = s.saturating_mul(2).saturating_sub(a);
    let quadratic_part = slope.saturating_mul(a).saturating_mul(two_s_minus_a) / (2 * scale);
    u128_to_u64_saturating(linear_part.saturating_add(quadratic_part) / norm)
}

/// Get current token price (spores per token)
fn current_price(supply_sold: u64) -> u64 {
    // SECURITY-FIX: Use u128 intermediate to prevent overflow
    BASE_PRICE.saturating_add(u128_to_u64_saturating(
        supply_sold as u128 * SLOPE as u128 / SLOPE_SCALE as u128,
    ))
}

// ============================================================================
// BUY / SELL
// ============================================================================

/// Buy tokens on the bonding curve
/// Returns number of tokens received. Validation failures return ERROR_RETURN so the host reverts value transfers.
#[no_mangle]
pub extern "C" fn buy(buyer_ptr: *const u8, token_id: u64, licn_amount: u64) -> u64 {
    if licn_amount == 0 {
        return ERROR_RETURN;
    }
    if is_paused() {
        log_info("Protocol is paused");
        return ERROR_RETURN;
    }
    if is_token_frozen(token_id) {
        log_info("Token is frozen");
        return ERROR_RETURN;
    }
    if !reentrancy_enter() {
        log_info("Reentrancy detected");
        return ERROR_RETURN;
    }

    // v2: Max buy per tx
    let max_buy = get_max_buy();
    if licn_amount > max_buy {
        reentrancy_exit();
        log_info("Exceeds max buy per transaction");
        return ERROR_RETURN;
    }

    let mut buyer = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(buyer_ptr, buyer.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != buyer {
        reentrancy_exit();
        return 200;
    }

    if graduation_state(token_id) != GRADUATION_ACTIVE {
        reentrancy_exit();
        log_info("Bonding-curve buys are closed for graduation");
        return ERROR_RETURN;
    }

    // G24-01: Verify actual payment instead of trusting parameter
    let payment_token = load_licn_token_or_native();
    if !receive_token_or_native(
        payment_token,
        Address(buyer),
        get_contract_address(),
        licn_amount,
    )
    .unwrap_or(false)
    {
        reentrancy_exit();
        log_info("Insufficient payment for buy");
        return ERROR_RETURN;
    }

    let buyer_hex = hex_encode_addr(&buyer);

    // v2: Buy cooldown
    let cooldown = get_buy_cooldown();
    let lbk = last_buy_key(token_id, &buyer_hex);
    let last_buy_ts = load_u64(&lbk);
    let now = get_timestamp();
    if last_buy_ts > 0 && now < last_buy_ts.saturating_add(cooldown) {
        reentrancy_exit();
        log_info("Buy cooldown not expired");
        return ERROR_RETURN;
    }

    let id_hex = u64_to_hex(token_id);
    let token_key = make_key(b"cpt:", &id_hex);

    let mut data = match storage_get(&token_key) {
        Some(d) if d.len() >= TOKEN_DATA_SIZE => d,
        _ => {
            log_info("Token not found");
            reentrancy_exit();
            return ERROR_RETURN;
        }
    };

    if data[64] != 0 {
        log_info("Token graduated to DEX, trade there");
        reentrancy_exit();
        return ERROR_RETURN;
    }

    let supply_sold = bytes_to_u64(&data[32..40]);
    let licn_raised = bytes_to_u64(&data[40..48]);
    let max_supply = bytes_to_u64(&data[48..56]);

    // Platform fee
    let maximum_fee =
        u128_to_u64_saturating(licn_amount as u128 * PLATFORM_FEE_PERCENT as u128 / 100);
    let net_amount = licn_amount - maximum_fee;

    // Binary search for how many tokens we can buy with net_amount
    let mut lo: u64 = 0;
    let mut hi: u64 = max_supply.saturating_sub(supply_sold);

    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        let cost = calculate_buy_cost(supply_sold, mid);
        if cost <= net_amount {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    let tokens_bought = lo;
    if tokens_bought == 0 {
        log_info("Amount too small to buy any tokens");
        reentrancy_exit();
        return ERROR_RETURN;
    }

    let actual_cost = calculate_buy_cost(supply_sold, tokens_bought);
    if actual_cost == 0 {
        reentrancy_exit();
        log_info("Calculated launchpad buy cost is zero");
        return ERROR_RETURN;
    }
    let fee_denominator = 100u128.saturating_sub(PLATFORM_FEE_PERCENT as u128);
    let fee = u128_to_u64_saturating(
        (actual_cost as u128 * PLATFORM_FEE_PERCENT as u128)
            .saturating_add(fee_denominator.saturating_sub(1))
            / fee_denominator,
    );
    let charged = match actual_cost.checked_add(fee) {
        Some(charged) if charged <= licn_amount => charged,
        _ => {
            reentrancy_exit();
            log_info("Launchpad buy charge overflow");
            return ERROR_RETURN;
        }
    };
    let refund = licn_amount - charged;
    if refund > 0 && !transfer_licn_out(&buyer, refund) {
        reentrancy_exit();
        log_info("Launchpad buy refund failed");
        return ERROR_RETURN;
    }
    let new_supply = supply_sold + tokens_bought;
    let new_raised = match licn_raised.checked_add(actual_cost) {
        Some(v) => v,
        None => {
            reentrancy_exit();
            log_info("Raised LICN overflow");
            return ERROR_RETURN;
        }
    };

    let bal_key = launchpad_balance_key(token_id, &buyer);
    let prev_bal = load_u64(&bal_key);
    if !launchpad_can_receive(&buyer, tokens_bought, prev_bal) {
        log_info("Buyer cannot receive launchpad token");
        reentrancy_exit();
        return ERROR_RETURN;
    }
    let new_balance = match prev_bal.checked_add(tokens_bought) {
        Some(v) => v,
        None => {
            reentrancy_exit();
            log_info("Buyer balance overflow");
            return ERROR_RETURN;
        }
    };

    // Update token data
    data[32..40].copy_from_slice(&u64_to_bytes(new_supply));
    data[40..48].copy_from_slice(&u64_to_bytes(new_raised));
    storage_set(&token_key, &data);

    // Track buyer balance
    store_u64(&bal_key, new_balance);

    // Collect platform fee
    let fees = load_u64(b"cp_fees_collected");
    store_u64(b"cp_fees_collected", fees.saturating_add(fee));

    // v2: Creator royalty
    let royalty_bps = get_creator_royalty();
    if royalty_bps > 0 {
        let royalty =
            u128_to_u64_saturating(actual_cost as u128 * royalty_bps as u128 / BPS_SCALE as u128);
        if royalty > 0 {
            let creator_hex = hex_encode_addr(&data[0..32].try_into().unwrap_or([0u8; 32]));
            let mut cr_key = Vec::with_capacity(4 + 16 + 1 + 64);
            cr_key.extend_from_slice(b"cry:");
            cr_key.extend_from_slice(&id_hex);
            cr_key.push(b':');
            cr_key.extend_from_slice(&creator_hex);
            let prev_royalty = load_u64(&cr_key);
            store_u64(&cr_key, prev_royalty.saturating_add(royalty));
        }
    }

    // v2: Record last buy timestamp for cooldown
    store_u64(&lbk, now);
    refresh_eligibility(token_id, &data);

    log_info("Buy successful");
    reentrancy_exit();
    tokens_bought
}

/// Sell tokens back to the bonding curve
/// Returns LICN refund amount. Validation failures return ERROR_RETURN so the host reverts.
#[no_mangle]
pub extern "C" fn sell(seller_ptr: *const u8, token_id: u64, token_amount: u64) -> u64 {
    if token_amount == 0 {
        return ERROR_RETURN;
    }
    if is_token_frozen(token_id) {
        log_info("Token is frozen");
        return ERROR_RETURN;
    }
    if !reentrancy_enter() {
        log_info("Reentrancy detected");
        return ERROR_RETURN;
    }

    let mut seller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(seller_ptr, seller.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != seller {
        reentrancy_exit();
        return 200;
    }

    if graduation_state(token_id) >= GRADUATION_MIGRATING {
        reentrancy_exit();
        log_info("Bonding-curve sells are closed during or after migration");
        return ERROR_RETURN;
    }

    let seller_hex = hex_encode_addr(&seller);

    // v2: Sell cooldown — check last buy timestamp
    let sell_cd = get_sell_cooldown();
    let lbk = last_buy_key(token_id, &seller_hex);
    let last_buy_ts = load_u64(&lbk);
    let now = get_timestamp();
    if last_buy_ts > 0 && now < last_buy_ts.saturating_add(sell_cd) {
        reentrancy_exit();
        log_info("Sell cooldown not expired (anti-dump)");
        return ERROR_RETURN;
    }

    let id_hex = u64_to_hex(token_id);
    let token_key = make_key(b"cpt:", &id_hex);

    let mut data = match storage_get(&token_key) {
        Some(d) if d.len() >= TOKEN_DATA_SIZE => d,
        _ => {
            log_info("Token not found");
            reentrancy_exit();
            return ERROR_RETURN;
        }
    };

    if data[64] != 0 {
        log_info("Token graduated, trade on DEX");
        reentrancy_exit();
        return ERROR_RETURN;
    }

    // Check seller balance
    let bal_key = launchpad_balance_key(token_id, &seller);
    let balance = load_u64(&bal_key);

    if token_amount > balance {
        log_info("Insufficient token balance");
        reentrancy_exit();
        return ERROR_RETURN;
    }
    if !launchpad_can_send(&seller, token_amount, balance) {
        log_info("Seller cannot send launchpad token");
        reentrancy_exit();
        return ERROR_RETURN;
    }

    let supply_sold = bytes_to_u64(&data[32..40]);
    let licn_raised = bytes_to_u64(&data[40..48]);
    if token_amount > supply_sold {
        log_info("Sell amount exceeds circulating bonding-curve supply");
        reentrancy_exit();
        return ERROR_RETURN;
    }

    let raw_refund = calculate_sell_refund(supply_sold, token_amount);
    if raw_refund == 0 {
        log_info("Sell amount too small for refund");
        reentrancy_exit();
        return ERROR_RETURN;
    }
    let fee = u128_to_u64_saturating(raw_refund as u128 * PLATFORM_FEE_PERCENT as u128 / 100);
    let net_refund = raw_refund - fee;

    // G24-01: Transfer LICN refund before mutating accounting. If payout fails,
    // no bonding-curve state is committed and the host reverts the transaction.
    if !transfer_licn_out(&seller, net_refund) {
        log_info("Sell rejected: LICN transfer failed");
        reentrancy_exit();
        return ERROR_RETURN;
    }

    // Update token data
    let new_supply = supply_sold - token_amount;
    let new_raised = licn_raised.saturating_sub(raw_refund);
    data[32..40].copy_from_slice(&u64_to_bytes(new_supply));
    data[40..48].copy_from_slice(&u64_to_bytes(new_raised));
    storage_set(&token_key, &data);
    refresh_eligibility(token_id, &data);

    // Update seller balance
    store_u64(&bal_key, balance - token_amount);

    // Collect fee
    let fees = load_u64(b"cp_fees_collected");
    store_u64(b"cp_fees_collected", fees.saturating_add(fee));

    log_info("Sell successful");
    reentrancy_exit();
    net_refund
}

// ============================================================================
// VIEW FUNCTIONS
// ============================================================================

/// Get token info: [supply_sold(8), licn_raised(8), current_price(8), market_cap(8), graduated(1)]
#[no_mangle]
pub extern "C" fn get_token_info(token_id: u64) -> u32 {
    let id_hex = u64_to_hex(token_id);
    let token_key = make_key(b"cpt:", &id_hex);

    let data = match storage_get(&token_key) {
        Some(d) if d.len() >= TOKEN_DATA_SIZE => d,
        _ => return 1,
    };

    let supply_sold = bytes_to_u64(&data[32..40]);
    let licn_raised = bytes_to_u64(&data[40..48]);
    let price = current_price(supply_sold);
    let market_cap =
        u128_to_u64_saturating(price as u128 * supply_sold as u128 / 1_000_000_000u128);

    let mut result = Vec::with_capacity(33);
    result.extend_from_slice(&u64_to_bytes(supply_sold));
    result.extend_from_slice(&u64_to_bytes(licn_raised));
    result.extend_from_slice(&u64_to_bytes(price));
    result.extend_from_slice(&u64_to_bytes(market_cap));
    result.push(data[64]); // graduated flag
    set_return_data(&result);
    0
}

/// Get buy quote: how many tokens for given LICN amount
#[no_mangle]
pub extern "C" fn get_buy_quote(token_id: u64, licn_amount: u64) -> u64 {
    if licn_amount == 0 {
        return 0;
    }
    let id_hex = u64_to_hex(token_id);
    let token_key = make_key(b"cpt:", &id_hex);

    let data = match storage_get(&token_key) {
        Some(d) if d.len() >= TOKEN_DATA_SIZE => d,
        _ => return 0,
    };

    let supply_sold = bytes_to_u64(&data[32..40]);
    let max_supply = bytes_to_u64(&data[48..56]);
    let net =
        u128_to_u64_saturating(licn_amount as u128 * (100 - PLATFORM_FEE_PERCENT) as u128 / 100);

    let mut lo: u64 = 0;
    let mut hi = max_supply.saturating_sub(supply_sold);
    while lo < hi {
        let mid = lo + (hi - lo + 1) / 2;
        if calculate_buy_cost(supply_sold, mid) <= net {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// Get total token count
#[no_mangle]
pub extern "C" fn get_token_count() -> u64 {
    load_u64(TOKEN_COUNT_KEY)
}

/// Get platform stats: [token_count(8), fees_collected(8)]
#[no_mangle]
pub extern "C" fn get_platform_stats() -> u32 {
    let count = load_u64(TOKEN_COUNT_KEY);
    let fees = load_u64(b"cp_fees_collected");

    let mut result = Vec::with_capacity(16);
    result.extend_from_slice(&u64_to_bytes(count));
    result.extend_from_slice(&u64_to_bytes(fees));
    set_return_data(&result);
    0
}

// ============================================================================
// v2: ADMIN OPERATIONS
// ============================================================================

/// Admin pauses the protocol
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
        return 1;
    }
    if is_paused() {
        return 2;
    }
    storage_set(PAUSE_KEY, &[1]);
    log_info("SporePump paused");
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
        return 1;
    }
    if !is_paused() {
        return 2;
    }
    storage_set(PAUSE_KEY, &[0]);
    log_info("SporePump unpaused");
    0
}

/// Admin freezes a specific token (blocks buy/sell)
#[no_mangle]
pub extern "C" fn freeze_token(caller_ptr: *const u8, token_id: u64) -> u32 {
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
        return 1;
    }
    let id_hex = u64_to_hex(token_id);
    let key = make_key(b"cpf:", &id_hex);
    storage_set(&key, &[1]);
    log_info("Token frozen");
    0
}

/// Admin unfreezes a token
#[no_mangle]
pub extern "C" fn unfreeze_token(caller_ptr: *const u8, token_id: u64) -> u32 {
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
        return 1;
    }
    let id_hex = u64_to_hex(token_id);
    let key = make_key(b"cpf:", &id_hex);
    storage_set(&key, &[0]);
    log_info("Token unfrozen");
    0
}

/// Admin sets buy cooldown (slots)
#[no_mangle]
pub extern "C" fn set_buy_cooldown(caller_ptr: *const u8, cooldown_slots: u64) -> u32 {
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
        return 1;
    }
    store_u64(b"cp_buy_cooldown", cooldown_slots);
    0
}

/// Admin sets sell cooldown (slots)
#[no_mangle]
pub extern "C" fn set_sell_cooldown(caller_ptr: *const u8, cooldown_slots: u64) -> u32 {
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
        return 1;
    }
    store_u64(b"cp_sell_cooldown", cooldown_slots);
    0
}

/// Admin sets max buy amount per tx
#[no_mangle]
pub extern "C" fn set_max_buy(caller_ptr: *const u8, max_amount: u64) -> u32 {
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
        return 1;
    }
    if max_amount == 0 {
        return 2;
    }
    store_u64(b"cp_max_buy", max_amount);
    0
}

/// Admin sets creator royalty in basis points
#[no_mangle]
pub extern "C" fn set_creator_royalty(caller_ptr: *const u8, bps: u64) -> u32 {
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
        return 1;
    }
    if bps > 1000 {
        return 2;
    } // Max 10%
    store_u64(b"cp_creator_royalty", bps);
    0
}

/// Admin withdraws collected platform fees
#[no_mangle]
pub extern "C" fn withdraw_fees(caller_ptr: *const u8, amount: u64) -> u32 {
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
        return 1;
    }
    if amount == 0 {
        return 2;
    }
    let fees = load_u64(b"cp_fees_collected");
    if amount > fees {
        return 3;
    }
    store_u64(b"cp_fees_collected", fees - amount);

    // G24-01: Transfer LICN to admin (self-custody)
    if !transfer_licn_out(&caller, amount) {
        // Revert on transfer failure
        store_u64(b"cp_fees_collected", fees);
        log_info("Fee withdrawal reverted: LICN transfer failed");
        return 4;
    }

    log_info("Fees withdrawn");
    0
}

/// Admin sets the LICN token contract address (for outgoing transfers)
/// Returns: 0 success, 1 not admin, 2 already configured
#[no_mangle]
pub extern "C" fn set_licn_token(caller_ptr: *const u8, token_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_admin(&caller) {
        return 1;
    }

    let mut token = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(token_ptr, token.as_mut_ptr(), 32);
    }

    if has_configured_address(LICN_TOKEN_KEY) {
        log_info("LICN token already configured");
        return 2;
    }

    // NOTE: zero address [0;32] is allowed — it is the native LICN sentinel
    storage_set(LICN_TOKEN_KEY, &token);
    log_info("LICN token address configured");
    0
}

/// Admin sets DEX contract addresses for graduation migration
/// Both addresses must be non-zero 32-byte addresses
/// Returns: 0 success, 1 not admin, 2 zero core, 3 zero amm, 4 already configured
#[no_mangle]
pub extern "C" fn set_dex_addresses(
    caller_ptr: *const u8,
    core_ptr: *const u8,
    amm_ptr: *const u8,
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

    if !is_admin(&caller) {
        return 1;
    }
    let mut core_addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(core_ptr, core_addr.as_mut_ptr(), 32);
    }
    let mut amm_addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(amm_ptr, amm_addr.as_mut_ptr(), 32);
    }

    // Validate non-zero
    if core_addr.iter().all(|&b| b == 0) {
        log_info("DEX core address cannot be zero");
        return 2;
    }
    if amm_addr.iter().all(|&b| b == 0) {
        log_info("DEX AMM address cannot be zero");
        return 3;
    }

    if has_configured_address(DEX_CORE_ADDRESS_KEY) || has_configured_address(DEX_AMM_ADDRESS_KEY) {
        log_info("DEX addresses already configured");
        return 4;
    }

    storage_set(DEX_CORE_ADDRESS_KEY, &core_addr);
    storage_set(DEX_AMM_ADDRESS_KEY, &amm_addr);
    log_info("DEX addresses recorded for future graduation migration support");
    0
}

/// Bind the governance program responsible for one-time graduation settings.
/// This is a bootstrap operation and cannot be changed after it succeeds.
#[no_mangle]
pub extern "C" fn set_graduation_governance(
    caller_ptr: *const u8,
    governance_ptr: *const u8,
) -> u32 {
    let caller = read_address(caller_ptr);
    if get_caller().0 != caller {
        return 200;
    }
    if !is_admin(&caller) {
        return 1;
    }
    let governance = read_address(governance_ptr);
    if governance.iter().all(|byte| *byte == 0) {
        return 2;
    }
    if configured_address(GRADUATION_GOVERNANCE_KEY).is_some() {
        return 3;
    }
    storage_set(GRADUATION_GOVERNANCE_KEY, &governance);
    log_info("Graduation governance configured");
    0
}

/// Configure the router and canonical graduated-token template once through
/// the bound governance program. DEX Core and AMM use their legacy one-time
/// bootstrap addresses until the deployment scripts are migrated.
#[no_mangle]
pub extern "C" fn set_graduation_config(
    caller_ptr: *const u8,
    router_ptr: *const u8,
    template_hash_ptr: *const u8,
    tick_size: u64,
    lot_size: u64,
    min_order: u64,
    amm_fee_tier: u32,
) -> u32 {
    let caller = read_address(caller_ptr);
    if get_caller().0 != caller {
        return 200;
    }
    if configured_address(GRADUATION_GOVERNANCE_KEY) != Some(caller) {
        return 1;
    }
    if configured_address(DEX_CORE_ADDRESS_KEY).is_none()
        || configured_address(DEX_AMM_ADDRESS_KEY).is_none()
    {
        return 2;
    }
    let router = read_address(router_ptr);
    let template_hash = read_address(template_hash_ptr);
    if router.iter().all(|byte| *byte == 0)
        || template_hash.iter().all(|byte| *byte == 0)
        || tick_size == 0
        || lot_size == 0
        || min_order < 1_000
        || amm_fee_tier > 3
    {
        return 3;
    }
    if configured_address(DEX_ROUTER_ADDRESS_KEY).is_some()
        || configured_address(GRADUATED_TOKEN_TEMPLATE_HASH_KEY).is_some()
    {
        return 4;
    }
    storage_set(DEX_ROUTER_ADDRESS_KEY, &router);
    storage_set(GRADUATED_TOKEN_TEMPLATE_HASH_KEY, &template_hash);
    store_u64(GRADUATION_TICK_SIZE_KEY, tick_size);
    store_u64(GRADUATION_LOT_SIZE_KEY, lot_size);
    store_u64(GRADUATION_MIN_ORDER_KEY, min_order);
    store_u64(GRADUATION_AMM_FEE_TIER_KEY, amm_fee_tier as u64);
    log_info("Graduation router and canonical token template configured");
    0
}

/// Validate and freeze a canonical token candidate for permissionless
/// migration. Candidate deployment remains separate from this transaction.
#[no_mangle]
pub extern "C" fn begin_migration(
    keeper_ptr: *const u8,
    token_id: u64,
    candidate_ptr: *const u8,
) -> u32 {
    let keeper = read_address(keeper_ptr);
    if get_caller().0 != keeper {
        return 200;
    }
    let data = match token_record(token_id) {
        Some(data) => data,
        None => return 1,
    };
    if graduation_state(token_id) != GRADUATION_ELIGIBLE
        || token_market_cap(&data) < GRADUATION_MARKET_CAP
    {
        return 2;
    }
    let candidate = read_address(candidate_ptr);
    if candidate.iter().all(|byte| *byte == 0) {
        return 3;
    }
    let expected_hash = match configured_address(GRADUATED_TOKEN_TEMPLATE_HASH_KEY) {
        Some(hash) => hash,
        None => return 4,
    };
    if get_contract_code_hash(Address(candidate)) != Some(expected_hash) {
        log_info("Graduation candidate code hash mismatch");
        return 5;
    }
    let provenance = match call_contract(CrossCall::new(
        Address(candidate),
        "get_provenance",
        Vec::new(),
    )) {
        Ok(provenance) if provenance.len() == 88 => provenance,
        _ => return 6,
    };
    let supply_sold = bytes_to_u64(&data[32..40]);
    let max_supply = bytes_to_u64(&data[48..56]);
    if provenance[0..32] != get_contract_address().0
        || bytes_to_u64(&provenance[32..40]) != token_id
        || provenance[40..72] != data[0..32]
        || bytes_to_u64(&provenance[72..80]) != max_supply
        || bytes_to_u64(&provenance[80..88]) != supply_sold
    {
        log_info("Graduation candidate provenance mismatch");
        return 7;
    }

    storage_set(&graduation_key(b"cpgt:", token_id), &candidate);
    set_graduation_u64(b"cpgb:", token_id, get_slot());
    set_graduation_state(token_id, GRADUATION_MIGRATING);
    log_info("Launchpad migration started");
    0
}

/// Release a migration freeze after the deterministic timeout. No holder or
/// economic state is modified, and eligibility is recalculated from the curve.
#[no_mangle]
pub extern "C" fn abort_migration(keeper_ptr: *const u8, token_id: u64) -> u32 {
    let keeper = read_address(keeper_ptr);
    if get_caller().0 != keeper {
        return 200;
    }
    if graduation_state(token_id) != GRADUATION_MIGRATING {
        return 1;
    }
    let boundary = get_graduation_u64(b"cpgb:", token_id);
    if get_slot() < boundary.saturating_add(MIGRATION_TIMEOUT_SLOTS) {
        return 2;
    }
    let data = match token_record(token_id) {
        Some(data) => data,
        None => return 3,
    };
    storage::remove(&graduation_key(b"cpgt:", token_id));
    storage::remove(&graduation_key(b"cpgb:", token_id));
    set_graduation_state(token_id, GRADUATION_ACTIVE);
    refresh_eligibility(token_id, &data);
    log_info("Launchpad migration timeout aborted");
    0
}

/// Atomically create the canonical DEX market and seed its initial liquidity.
/// Any non-zero return causes the host to discard every nested contract write.
#[no_mangle]
pub extern "C" fn finalize_migration(keeper_ptr: *const u8, token_id: u64) -> u32 {
    let keeper = read_address(keeper_ptr);
    if get_caller().0 != keeper {
        return 200;
    }
    if graduation_state(token_id) != GRADUATION_MIGRATING {
        return 1;
    }
    let candidate = match graduation_candidate(token_id) {
        Some(candidate) => candidate,
        None => return 2,
    };
    let dex_core = match configured_address(DEX_CORE_ADDRESS_KEY) {
        Some(address) => address,
        None => return 3,
    };
    let dex_amm = match configured_address(DEX_AMM_ADDRESS_KEY) {
        Some(address) => address,
        None => return 3,
    };
    let dex_router = match configured_address(DEX_ROUTER_ADDRESS_KEY) {
        Some(address) => address,
        None => return 3,
    };
    let mut data = match token_record(token_id) {
        Some(data) => data,
        None => return 4,
    };
    let supply_sold = bytes_to_u64(&data[32..40]);
    let licn_raised = bytes_to_u64(&data[40..48]);
    let max_supply = bytes_to_u64(&data[48..56]);
    let price = current_price(supply_sold);
    let licn_liquidity_limit =
        u128_to_u64_saturating(licn_raised as u128 * GRADUATION_LIQUIDITY_PERCENT as u128 / 100);
    let minimum_platform_revenue =
        u128_to_u64_saturating(licn_raised as u128 * GRADUATION_PLATFORM_PERCENT as u128 / 100);
    if licn_liquidity_limit == 0
        || price == 0
        || licn_liquidity_limit
            .checked_add(minimum_platform_revenue)
            .is_none_or(|total| total > licn_raised)
    {
        return 5;
    }
    let token_liquidity_limit =
        u128_to_u64_saturating(licn_liquidity_limit as u128 * 1_000_000_000u128 / price as u128);
    if token_liquidity_limit < 100
        || supply_sold
            .checked_add(token_liquidity_limit)
            .is_none_or(|total| total > max_supply)
    {
        return 6;
    }

    let sporepump = get_contract_address().0;
    let mut mint_args = Vec::with_capacity(40);
    mint_args.extend_from_slice(&sporepump);
    mint_args.extend_from_slice(&u64_to_bytes(token_liquidity_limit));
    if !cross_call_succeeded(candidate, "mint_migration_inventory", mint_args) {
        return 7;
    }

    let mut approve_args = Vec::with_capacity(72);
    approve_args.extend_from_slice(&sporepump);
    approve_args.extend_from_slice(&dex_amm);
    approve_args.extend_from_slice(&u64_to_bytes(token_liquidity_limit));
    if !cross_call_succeeded(candidate, "approve", approve_args) {
        return 8;
    }

    let tick_size = load_u64(GRADUATION_TICK_SIZE_KEY);
    let lot_size = load_u64(GRADUATION_LOT_SIZE_KEY);
    let min_order = load_u64(GRADUATION_MIN_ORDER_KEY);
    let amm_fee_tier = load_u64(GRADUATION_AMM_FEE_TIER_KEY);
    if tick_size == 0 || lot_size == 0 || min_order < 1_000 || amm_fee_tier > 3 {
        return 9;
    }
    let native_licn = [0u8; 32];
    let mut pair_args = Vec::with_capacity(120);
    pair_args.extend_from_slice(&sporepump);
    pair_args.extend_from_slice(&candidate);
    pair_args.extend_from_slice(&native_licn);
    pair_args.extend_from_slice(&u64_to_bytes(tick_size));
    pair_args.extend_from_slice(&u64_to_bytes(lot_size));
    pair_args.extend_from_slice(&u64_to_bytes(min_order));
    let pair_id = match cross_call_id(dex_core, "create_pair", pair_args) {
        Some(pair_id) => pair_id,
        None => return 10,
    };

    let mut pool_args = Vec::with_capacity(105);
    pool_args.extend_from_slice(&sporepump);
    pool_args.extend_from_slice(&candidate);
    pool_args.extend_from_slice(&native_licn);
    pool_args.push(amm_fee_tier as u8);
    pool_args.extend_from_slice(&u64_to_bytes(initial_sqrt_price(price)));
    let pool_id = match cross_call_id(dex_amm, "create_pool", pool_args) {
        Some(pool_id) => pool_id,
        None => return 11,
    };

    const FULL_RANGE_LOWER_TICK: i32 = -443_580;
    const FULL_RANGE_UPPER_TICK: i32 = 443_580;
    let mut liquidity_args = Vec::with_capacity(72);
    liquidity_args.extend_from_slice(&sporepump);
    liquidity_args.extend_from_slice(&u64_to_bytes(pool_id));
    liquidity_args.extend_from_slice(&FULL_RANGE_LOWER_TICK.to_le_bytes());
    liquidity_args.extend_from_slice(&FULL_RANGE_UPPER_TICK.to_le_bytes());
    liquidity_args.extend_from_slice(&u64_to_bytes(token_liquidity_limit));
    liquidity_args.extend_from_slice(&u64_to_bytes(licn_liquidity_limit));
    liquidity_args.extend_from_slice(&u64_to_bytes(0));
    let liquidity_response = match call_contract(
        CrossCall::new(Address(dex_amm), "add_liquidity", liquidity_args)
            .with_value(licn_liquidity_limit),
    ) {
        Ok(response) if response.len() >= 24 => response,
        _ => return 12,
    };
    let position_id = bytes_to_u64(&liquidity_response[0..8]);
    let actual_token_liquidity = bytes_to_u64(&liquidity_response[8..16]);
    let actual_licn_liquidity = bytes_to_u64(&liquidity_response[16..24]);
    if position_id == 0
        || actual_token_liquidity == 0
        || actual_licn_liquidity == 0
        || actual_token_liquidity > token_liquidity_limit
        || actual_licn_liquidity > licn_liquidity_limit
    {
        return 13;
    }

    let mut forward_route_args = Vec::with_capacity(90);
    forward_route_args.extend_from_slice(&sporepump);
    forward_route_args.extend_from_slice(&candidate);
    forward_route_args.extend_from_slice(&native_licn);
    forward_route_args.push(1);
    forward_route_args.extend_from_slice(&u64_to_bytes(pool_id));
    forward_route_args.extend_from_slice(&u64_to_bytes(0));
    forward_route_args.push(0);
    let forward_route_id = match cross_call_id(dex_router, "register_route", forward_route_args) {
        Some(route_id) => route_id,
        None => return 14,
    };

    let mut reverse_route_args = Vec::with_capacity(90);
    reverse_route_args.extend_from_slice(&sporepump);
    reverse_route_args.extend_from_slice(&native_licn);
    reverse_route_args.extend_from_slice(&candidate);
    reverse_route_args.push(1);
    reverse_route_args.extend_from_slice(&u64_to_bytes(pool_id));
    reverse_route_args.extend_from_slice(&u64_to_bytes(0));
    reverse_route_args.push(0);
    let reverse_route_id = match cross_call_id(dex_router, "register_route", reverse_route_args) {
        Some(route_id) => route_id,
        None => return 15,
    };

    set_graduation_u64(b"cpgp:", token_id, pair_id);
    set_graduation_u64(b"cpga:", token_id, pool_id);
    set_graduation_u64(b"cpgr:", token_id, forward_route_id);
    set_graduation_u64(b"cpgr2:", token_id, reverse_route_id);
    set_graduation_u64(b"cpgpos:", token_id, position_id);
    let mut liquidity = Vec::with_capacity(16);
    liquidity.extend_from_slice(&u64_to_bytes(actual_licn_liquidity));
    liquidity.extend_from_slice(&u64_to_bytes(actual_token_liquidity));
    storage_set(&graduation_key(b"cpgl:", token_id), &liquidity);
    set_graduation_u64(
        b"cpgx:",
        token_id,
        token_liquidity_limit - actual_token_liquidity,
    );
    let platform_revenue = licn_raised - actual_licn_liquidity;
    let Some(graduation_revenue) =
        load_u64(b"cp_graduation_revenue").checked_add(platform_revenue)
    else {
        return 16;
    };
    let Some(graduation_revision) = load_u64(b"cp_graduation_revision").checked_add(1) else {
        return 17;
    };
    store_u64(b"cp_graduation_revenue", graduation_revenue);
    store_u64(b"cp_graduation_revision", graduation_revision);
    data[64] = 1;
    storage_set(&graduation_key(b"cpt:", token_id), &data);
    set_graduation_state(token_id, GRADUATION_GRADUATED);

    let mut result = Vec::with_capacity(40);
    result.extend_from_slice(&u64_to_bytes(pair_id));
    result.extend_from_slice(&u64_to_bytes(pool_id));
    result.extend_from_slice(&u64_to_bytes(forward_route_id));
    result.extend_from_slice(&u64_to_bytes(reverse_route_id));
    result.extend_from_slice(&u64_to_bytes(position_id));
    set_return_data(&result);
    log_info("Launchpad token graduated atomically to DEX");
    0
}

/// Consume one frozen holder balance. The canonical graduated token is the
/// only authorized caller; its claim transaction mints the returned amount or
/// atomically rolls this write back.
#[no_mangle]
pub extern "C" fn consume_graduation_claim(token_id: u64, holder_ptr: *const u8) -> u64 {
    if graduation_state(token_id) != GRADUATION_GRADUATED {
        return 0;
    }
    let candidate = match graduation_candidate(token_id) {
        Some(candidate) => candidate,
        None => return 0,
    };
    if get_caller().0 != candidate {
        return 0;
    }
    let holder = read_address(holder_ptr);
    let claim_key = graduation_claim_key(token_id, &holder);
    if storage_get(&claim_key).is_some() {
        return 0;
    }
    let balance_key = launchpad_balance_key(token_id, &holder);
    let amount = load_u64(&balance_key);
    if amount == 0 {
        return 0;
    }
    store_u64(&balance_key, 0);
    storage_set(&claim_key, &[1]);
    set_return_data(&u64_to_bytes(amount));
    log_info("Launchpad graduation claim consumed");
    amount
}

/// Lifecycle status: state(1), eligibility slot(8), boundary slot(8),
/// candidate(32), pair id(8), pool id(8), route id(8).
#[no_mangle]
pub extern "C" fn get_graduation_status(token_id: u64) -> u32 {
    if token_record(token_id).is_none() {
        return 1;
    }
    let mut result = Vec::with_capacity(73);
    result.push(graduation_state(token_id));
    result.extend_from_slice(&u64_to_bytes(get_graduation_u64(b"cpge:", token_id)));
    result.extend_from_slice(&u64_to_bytes(get_graduation_u64(b"cpgb:", token_id)));
    result.extend_from_slice(&graduation_candidate(token_id).unwrap_or([0u8; 32]));
    result.extend_from_slice(&u64_to_bytes(get_graduation_u64(b"cpgp:", token_id)));
    result.extend_from_slice(&u64_to_bytes(get_graduation_u64(b"cpga:", token_id)));
    result.extend_from_slice(&u64_to_bytes(get_graduation_u64(b"cpgr:", token_id)));
    set_return_data(&result);
    0
}

/// Get graduation info: [graduation_revenue(8), dex_core_set(1), dex_amm_set(1)]
#[no_mangle]
pub extern "C" fn get_graduation_info() -> u32 {
    let revenue = load_u64(b"cp_graduation_revenue");
    let core_set: u8 = storage_get(DEX_CORE_ADDRESS_KEY)
        .map(|b| {
            if b.len() == 32 && b.iter().any(|&x| x != 0) {
                1
            } else {
                0
            }
        })
        .unwrap_or(0);
    let amm_set: u8 = storage_get(DEX_AMM_ADDRESS_KEY)
        .map(|b| {
            if b.len() == 32 && b.iter().any(|&x| x != 0) {
                1
            } else {
                0
            }
        })
        .unwrap_or(0);

    let mut result = Vec::with_capacity(10);
    result.extend_from_slice(&u64_to_bytes(revenue));
    result.push(core_set);
    result.push(amm_set);
    set_return_data(&result);
    0
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::vec;
    use lichen_sdk::bytes_to_u64;
    use lichen_sdk::test_mock;

    fn setup() {
        test_mock::reset();
    }

    fn create_threshold_token() -> (u64, [u8; 32], [u8; 32]) {
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(set_max_buy(admin.as_ptr(), u64::MAX), 0);

        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let token_id = create_token(creator.as_ptr(), CREATION_FEE);
        assert_ne!(token_id, ERROR_RETURN);

        let token_key = graduation_key(b"cpt:", token_id);
        let mut data = test_mock::get_storage(&token_key).unwrap();
        data[32..40].copy_from_slice(&u64_to_bytes(400_000_000_000_000));
        data[40..48].copy_from_slice(&u64_to_bytes(50_000_000_000_000_000));
        storage_set(&token_key, &data);

        let buyer = [3u8; 32];
        test_mock::set_timestamp(10_000);
        test_mock::set_slot(100);
        test_mock::set_caller(buyer);
        let buy_amount = 1_000_000_000_000;
        test_mock::set_value(buy_amount);
        let bought = buy(buyer.as_ptr(), token_id, buy_amount);
        assert_ne!(bought, ERROR_RETURN);
        assert!(bought > 0);
        assert_eq!(graduation_state(token_id), GRADUATION_ELIGIBLE);
        (token_id, creator, buyer)
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
        assert_eq!(get_token_count(), 0);
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
    fn test_create_token() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let token_id = create_token(creator.as_ptr(), CREATION_FEE);
        assert_eq!(token_id, 1);
        assert_eq!(get_token_count(), 1);
        let fees = load_u64(b"cp_fees_collected");
        assert_eq!(fees, CREATION_FEE);
        assert_eq!(storage_get(&token_name_key(token_id)).unwrap(), b"Spore Token 1");
        assert_eq!(storage_get(&token_symbol_key(token_id)).unwrap(), b"SPT1");
        assert_eq!(get_token_metadata(token_id), 0);
        let metadata = test_mock::get_return_data();
        assert_eq!(u16::from_le_bytes(metadata[0..2].try_into().unwrap()), 13);
        assert_eq!(&metadata[2..15], b"Spore Token 1");
        assert_eq!(u16::from_le_bytes(metadata[15..17].try_into().unwrap()), 4);
        assert_eq!(&metadata[17..21], b"SPT1");
    }

    #[test]
    fn test_create_token_metadata_is_normalized_unique_and_validated() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        let name = b"Forest Credit";
        let symbol = b"fern";
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let token_id = create_token_with_metadata(
            creator.as_ptr(),
            name.as_ptr(),
            name.len() as u32,
            symbol.as_ptr(),
            symbol.len() as u32,
            CREATION_FEE,
        );
        assert_eq!(token_id, 1);
        assert_eq!(storage_get(&token_name_key(token_id)).unwrap(), name);
        assert_eq!(storage_get(&token_symbol_key(token_id)).unwrap(), b"FERN");

        test_mock::set_value(CREATION_FEE);
        assert_eq!(
            create_token_with_metadata(
                creator.as_ptr(),
                b"Other".as_ptr(),
                5,
                b"FERN".as_ptr(),
                4,
                CREATION_FEE,
            ),
            ERROR_RETURN
        );
        assert_eq!(get_token_count(), 1);

        test_mock::set_value(CREATION_FEE);
        assert_eq!(
            create_token_with_metadata(
                creator.as_ptr(),
                b"Bad".as_ptr(),
                3,
                b"BAD-SYMBOL".as_ptr(),
                10,
                CREATION_FEE,
            ),
            ERROR_RETURN
        );
        assert_eq!(get_token_count(), 1);
    }

    #[test]
    fn test_create_token_records_actual_creation_fee_only() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);

        assert_eq!(create_token(creator.as_ptr(), u64::MAX), 1);
        assert_eq!(load_u64(b"cp_fees_collected"), CREATION_FEE);
    }

    #[test]
    fn test_create_token_counter_overflow_rejected() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        store_u64(TOKEN_COUNT_KEY, u64::MAX);

        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        assert_eq!(create_token(creator.as_ptr(), CREATION_FEE), ERROR_RETURN);
        assert_eq!(load_u64(TOKEN_COUNT_KEY), u64::MAX);
    }

    #[test]
    fn test_create_token_insufficient_fee() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE - 1); // insufficient
        assert_eq!(
            create_token(creator.as_ptr(), CREATION_FEE - 1),
            ERROR_RETURN
        );
        assert_eq!(get_token_count(), 0);
    }

    #[test]
    fn test_create_multiple_tokens() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        assert_eq!(create_token(creator.as_ptr(), CREATION_FEE), 1);
        assert_eq!(create_token(creator.as_ptr(), CREATION_FEE), 2);
        assert_eq!(get_token_count(), 2);
        let fees = load_u64(b"cp_fees_collected");
        assert_eq!(fees, CREATION_FEE * 2);
    }

    #[test]
    fn test_buy() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let token_id = create_token(creator.as_ptr(), CREATION_FEE);
        let buyer = [3u8; 32];
        test_mock::set_caller(buyer);
        test_mock::set_value(1_000_000_000);
        let tokens = buy(buyer.as_ptr(), token_id, 1_000_000_000);
        assert!(tokens > 0, "Should receive tokens for 1 LICN");
    }

    #[test]
    fn test_buy_uses_full_curve_range_and_matches_quote() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let token_id = create_token(creator.as_ptr(), CREATION_FEE);

        let buyer = [3u8; 32];
        let payment = 5_000_000_000;
        let quoted = get_buy_quote(token_id, payment);
        assert!(
            quoted > 1_000_000_000_000,
            "5 LICN quote must not be capped at 1,000 tokens"
        );
        test_mock::set_caller(buyer);
        test_mock::set_value(payment);
        let bought = buy(buyer.as_ptr(), token_id, payment);
        assert_eq!(bought, quoted, "quote and credited amount must match");

        let net = payment * (100 - PLATFORM_FEE_PERCENT) / 100;
        assert!(calculate_buy_cost(0, bought) <= net);
        assert!(calculate_buy_cost(0, bought + 1) > net);
    }

    #[test]
    fn test_capped_buy_refunds_every_unused_spore_and_collects_only_actual_fee() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_licn_token(admin.as_ptr(), [0u8; 32].as_ptr()), 0);
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let token_id = create_token(creator.as_ptr(), CREATION_FEE);
        let token_key = graduation_key(b"cpt:", token_id);
        let mut data = storage_get(&token_key).unwrap();
        data[48..56].copy_from_slice(&u64_to_bytes(1_000_000_000));
        storage_set(&token_key, &data);

        let buyer = [3u8; 32];
        let payment = 1_000_000_000;
        let actual_cost = calculate_buy_cost(0, 1_000_000_000);
        let actual_fee = (actual_cost * PLATFORM_FEE_PERCENT + 98) / 99;
        test_mock::set_caller(buyer);
        test_mock::set_value(payment);
        assert_eq!(buy(buyer.as_ptr(), token_id, payment), 1_000_000_000);
        assert_eq!(load_u64(b"cp_fees_collected"), CREATION_FEE + actual_fee);
        let call = test_mock::get_last_cross_call().expect("refund transfer");
        assert_eq!(call.1, "transfer");
        assert_eq!(&call.2[0..32], &buyer);
        assert_eq!(
            bytes_to_u64(&call.2[32..40]),
            payment - actual_cost - actual_fee
        );
    }

    #[test]
    fn test_zero_cost_capped_remainder_is_not_credited_for_free() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let token_id = create_token(creator.as_ptr(), CREATION_FEE);
        let token_key = graduation_key(b"cpt:", token_id);
        let mut data = storage_get(&token_key).unwrap();
        data[48..56].copy_from_slice(&u64_to_bytes(1_000));
        storage_set(&token_key, &data);
        let buyer = [3u8; 32];
        test_mock::set_caller(buyer);
        test_mock::set_value(1);
        assert_eq!(buy(buyer.as_ptr(), token_id, 1), ERROR_RETURN);
        assert_eq!(load_u64(&launchpad_balance_key(token_id, &buyer)), 0);
    }

    #[test]
    fn test_compliance_blocks_buy_without_mutation() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let token_id = create_token(creator.as_ptr(), CREATION_FEE);
        let id_hex = u64_to_hex(token_id);
        let token_key = make_key(b"cpt:", &id_hex);
        let token_before = test_mock::get_storage(&token_key).unwrap();
        let fees_before = load_u64(b"cp_fees_collected");

        let buyer = [3u8; 32];
        let bal_key = launchpad_balance_key(token_id, &buyer);
        let buyer_hex = hex_encode_addr(&buyer);
        let lbk = last_buy_key(token_id, &buyer_hex);
        test_mock::set_can_receive(false);
        test_mock::set_timestamp(10_000);
        test_mock::set_caller(buyer);
        test_mock::set_value(1_000_000_000);

        assert_eq!(buy(buyer.as_ptr(), token_id, 1_000_000_000), ERROR_RETURN);
        assert_eq!(test_mock::get_storage(&token_key).unwrap(), token_before);
        assert_eq!(load_u64(&bal_key), 0);
        assert_eq!(load_u64(&lbk), 0);
        assert_eq!(load_u64(b"cp_fees_collected"), fees_before);
    }

    #[test]
    fn test_buy_zero_amount() {
        setup();
        assert_eq!(buy([3u8; 32].as_ptr(), 1, 0), ERROR_RETURN);
    }

    #[test]
    fn test_buy_nonexistent_token() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        test_mock::set_caller([3u8; 32]);
        test_mock::set_value(1_000_000_000);
        assert_eq!(buy([3u8; 32].as_ptr(), 999, 1_000_000_000), ERROR_RETURN);
    }

    #[test]
    fn test_sell() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        // CON-05: Configure LICN token so transfer_licn_out succeeds
        let licn = [42u8; 32];
        set_licn_token(admin.as_ptr(), licn.as_ptr());
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let token_id = create_token(creator.as_ptr(), CREATION_FEE);
        let buyer = [3u8; 32];
        test_mock::set_timestamp(10_000);
        test_mock::set_caller(buyer);
        test_mock::set_value(1_000_000_000);
        let bought = buy(buyer.as_ptr(), token_id, 1_000_000_000);
        assert!(bought > 0);
        // Advance past sell cooldown (default 13 slots)
        test_mock::set_timestamp(20_000);
        // Sell half the bought tokens
        let _refund = sell(buyer.as_ptr(), token_id, bought / 2);
        // Verify buyer balance decreased
        let id_hex = u64_to_hex(token_id);
        let buyer_hex = hex_encode_addr(&buyer);
        let mut bal_key = Vec::with_capacity(4 + 16 + 1 + 64);
        bal_key.extend_from_slice(b"bal:");
        bal_key.extend_from_slice(&id_hex);
        bal_key.push(b':');
        bal_key.extend_from_slice(&buyer_hex);
        let remaining = load_u64(&bal_key);
        assert_eq!(remaining, bought - bought / 2);
    }

    #[test]
    fn test_transfer_licn_out_rejects_false_status() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let licn = [42u8; 32];
        assert_eq!(set_licn_token(admin.as_ptr(), licn.as_ptr()), 0);
        test_mock::set_cross_call_response(Some(vec![2u8]));

        let recipient = [3u8; 32];
        assert!(!transfer_licn_out(&recipient, 1_000));
    }

    #[test]
    fn test_sell_reverts_on_false_transfer_status() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let licn = [42u8; 32];
        assert_eq!(set_licn_token(admin.as_ptr(), licn.as_ptr()), 0);

        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let token_id = create_token(creator.as_ptr(), CREATION_FEE);

        let buyer = [3u8; 32];
        test_mock::set_timestamp(10_000);
        test_mock::set_caller(buyer);
        test_mock::set_value(1_000_000_000);
        let bought = buy(buyer.as_ptr(), token_id, 1_000_000_000);
        assert!(bought > 0);

        let id_hex = u64_to_hex(token_id);
        let buyer_hex = hex_encode_addr(&buyer);
        let mut bal_key = Vec::with_capacity(4 + 16 + 1 + 64);
        bal_key.extend_from_slice(b"bal:");
        bal_key.extend_from_slice(&id_hex);
        bal_key.push(b':');
        bal_key.extend_from_slice(&buyer_hex);
        let before_balance = load_u64(&bal_key);
        let token_key = make_key(b"cpt:", &id_hex);
        let before_token = storage_get(&token_key).unwrap();
        let before_fees = load_u64(b"cp_fees_collected");

        test_mock::set_timestamp(20_000);
        test_mock::set_cross_call_response(Some(vec![2u8]));
        assert_eq!(sell(buyer.as_ptr(), token_id, bought / 2), ERROR_RETURN);
        assert_eq!(load_u64(&bal_key), before_balance);
        assert_eq!(storage_get(&token_key).unwrap(), before_token);
        assert_eq!(load_u64(b"cp_fees_collected"), before_fees);
    }

    #[test]
    fn test_compliance_blocks_sell_without_mutation() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let licn = [42u8; 32];
        assert_eq!(set_licn_token(admin.as_ptr(), licn.as_ptr()), 0);
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));

        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let token_id = create_token(creator.as_ptr(), CREATION_FEE);

        let buyer = [3u8; 32];
        test_mock::set_timestamp(10_000);
        test_mock::set_caller(buyer);
        test_mock::set_value(1_000_000_000);
        let bought = buy(buyer.as_ptr(), token_id, 1_000_000_000);
        assert!(bought > 0);

        let id_hex = u64_to_hex(token_id);
        let token_key = make_key(b"cpt:", &id_hex);
        let bal_key = launchpad_balance_key(token_id, &buyer);
        let token_before = test_mock::get_storage(&token_key).unwrap();
        let balance_before = load_u64(&bal_key);
        let fees_before = load_u64(b"cp_fees_collected");
        let last_call_before = test_mock::get_last_cross_call();

        test_mock::set_can_send(false);
        test_mock::set_timestamp(20_000);
        assert_eq!(sell(buyer.as_ptr(), token_id, bought / 2), ERROR_RETURN);
        assert_eq!(test_mock::get_storage(&token_key).unwrap(), token_before);
        assert_eq!(load_u64(&bal_key), balance_before);
        assert_eq!(load_u64(b"cp_fees_collected"), fees_before);
        assert_eq!(test_mock::get_last_cross_call(), last_call_before);
    }

    #[test]
    fn test_sell_insufficient_balance() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        create_token(creator.as_ptr(), CREATION_FEE);
        test_mock::set_caller([3u8; 32]);
        assert_eq!(sell([3u8; 32].as_ptr(), 1, 1000), ERROR_RETURN);
    }

    #[test]
    fn test_sell_zero_amount() {
        setup();
        assert_eq!(sell([3u8; 32].as_ptr(), 1, 0), ERROR_RETURN);
    }

    #[test]
    fn test_get_token_info() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let tid = create_token(creator.as_ptr(), CREATION_FEE);
        assert_eq!(get_token_info(tid), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 33); // supply(8)+raised(8)+price(8)+mcap(8)+graduated(1)
    }

    #[test]
    fn test_get_token_info_nonexistent() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(get_token_info(999), 1);
    }

    #[test]
    fn test_get_buy_quote() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let tid = create_token(creator.as_ptr(), CREATION_FEE);
        assert_eq!(get_buy_quote(tid, 0), 0);
        let quote = get_buy_quote(tid, 1_000_000_000);
        assert!(quote > 0);
    }

    #[test]
    fn test_bonding_curve_math_saturates() {
        setup();
        assert_eq!(
            current_price(u64::MAX),
            BASE_PRICE + (u64::MAX as u128 / SLOPE_SCALE as u128) as u64
        );
        assert_eq!(calculate_buy_cost(u64::MAX, u64::MAX), u64::MAX);
        assert_eq!(calculate_sell_refund(u64::MAX, u64::MAX), u64::MAX);
    }

    #[test]
    fn test_get_token_count() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(get_token_count(), 0);
        let c = [2u8; 32];
        test_mock::set_caller(c);
        test_mock::set_value(CREATION_FEE);
        create_token(c.as_ptr(), CREATION_FEE);
        assert_eq!(get_token_count(), 1);
        create_token(c.as_ptr(), CREATION_FEE);
        assert_eq!(get_token_count(), 2);
    }

    #[test]
    fn test_get_platform_stats() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let c = [2u8; 32];
        test_mock::set_caller(c);
        test_mock::set_value(CREATION_FEE);
        create_token(c.as_ptr(), CREATION_FEE);
        assert_eq!(get_platform_stats(), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 16);
        assert_eq!(bytes_to_u64(&ret[0..8]), 1);
        assert_eq!(bytes_to_u64(&ret[8..16]), CREATION_FEE);
    }

    // ========================================================================
    // v2 TESTS
    // ========================================================================

    #[test]
    fn test_pause_unpause() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        create_token(creator.as_ptr(), CREATION_FEE);

        test_mock::set_caller(admin);
        assert_eq!(pause(admin.as_ptr()), 0);
        assert!(is_paused());
        // Buy blocked (paused check is before caller check)
        let buyer = [3u8; 32];
        assert_eq!(buy(buyer.as_ptr(), 1, 1_000_000_000), ERROR_RETURN);

        test_mock::set_caller(admin);
        assert_eq!(unpause(admin.as_ptr()), 0);
        assert!(!is_paused());
    }

    #[test]
    fn test_pause_allows_sell_exit() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let licn = [42u8; 32];
        assert_eq!(set_licn_token(admin.as_ptr(), licn.as_ptr()), 0);
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));

        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        create_token(creator.as_ptr(), CREATION_FEE);

        let buyer = [3u8; 32];
        test_mock::set_timestamp(10_000);
        test_mock::set_caller(buyer);
        test_mock::set_value(1_000_000_000);
        let tokens = buy(buyer.as_ptr(), 1, 1_000_000_000);
        assert!(tokens > 0);

        test_mock::set_caller(admin);
        assert_eq!(pause(admin.as_ptr()), 0);

        test_mock::set_timestamp(10_014);
        test_mock::set_caller(buyer);
        let refund = sell(buyer.as_ptr(), 1, tokens / 2);
        assert!(refund > 0, "sell should remain available while paused");
    }

    #[test]
    fn test_pause_non_admin() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let other = [9u8; 32];
        test_mock::set_caller(other);
        assert_eq!(pause(other.as_ptr()), 1);
    }

    #[test]
    fn test_freeze_token() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        create_token(creator.as_ptr(), CREATION_FEE);

        test_mock::set_caller(admin);
        assert_eq!(freeze_token(admin.as_ptr(), 1), 0);
        assert!(is_token_frozen(1));
        // Buy blocked (frozen check is before caller check)
        let buyer = [3u8; 32];
        test_mock::set_timestamp(10_000);
        assert_eq!(buy(buyer.as_ptr(), 1, 1_000_000_000), ERROR_RETURN);

        // Unfreeze
        test_mock::set_caller(admin);
        assert_eq!(unfreeze_token(admin.as_ptr(), 1), 0);
        assert!(!is_token_frozen(1));
        // Buy works
        test_mock::set_caller(buyer);
        test_mock::set_value(1_000_000_000);
        let tokens = buy(buyer.as_ptr(), 1, 1_000_000_000);
        assert!(tokens > 0);
    }

    #[test]
    fn test_freeze_token_blocks_sell_without_mutation() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let licn = [42u8; 32];
        assert_eq!(set_licn_token(admin.as_ptr(), licn.as_ptr()), 0);
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));

        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let token_id = create_token(creator.as_ptr(), CREATION_FEE);

        let buyer = [3u8; 32];
        test_mock::set_timestamp(10_000);
        test_mock::set_caller(buyer);
        test_mock::set_value(1_000_000_000);
        let bought = buy(buyer.as_ptr(), token_id, 1_000_000_000);
        assert!(bought > 0);

        test_mock::set_caller(admin);
        assert_eq!(freeze_token(admin.as_ptr(), token_id), 0);

        let id_hex = u64_to_hex(token_id);
        let token_key = make_key(b"cpt:", &id_hex);
        let bal_key = launchpad_balance_key(token_id, &buyer);
        let token_before = test_mock::get_storage(&token_key).unwrap();
        let balance_before = load_u64(&bal_key);
        let fees_before = load_u64(b"cp_fees_collected");
        let last_call_before = test_mock::get_last_cross_call();

        test_mock::set_timestamp(20_000);
        test_mock::set_caller(buyer);
        assert_eq!(sell(buyer.as_ptr(), token_id, bought / 2), ERROR_RETURN);
        assert_eq!(test_mock::get_storage(&token_key).unwrap(), token_before);
        assert_eq!(load_u64(&bal_key), balance_before);
        assert_eq!(load_u64(b"cp_fees_collected"), fees_before);
        assert_eq!(test_mock::get_last_cross_call(), last_call_before);
    }

    #[test]
    fn test_freeze_non_admin() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let other = [9u8; 32];
        test_mock::set_caller(other);
        assert_eq!(freeze_token(other.as_ptr(), 1), 1);
        assert_eq!(unfreeze_token(other.as_ptr(), 1), 1);
    }

    #[test]
    fn test_buy_cooldown() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        create_token(creator.as_ptr(), CREATION_FEE);

        let buyer = [3u8; 32];
        test_mock::set_timestamp(10_000);
        test_mock::set_caller(buyer);
        test_mock::set_value(1_000_000_000);
        let tokens = buy(buyer.as_ptr(), 1, 1_000_000_000);
        assert!(tokens > 0);

        // Second buy within cooldown (default 5 slots)
        test_mock::set_timestamp(10_003);
        assert_eq!(buy(buyer.as_ptr(), 1, 1_000_000_000), ERROR_RETURN);

        // After cooldown
        test_mock::set_timestamp(10_006);
        let tokens2 = buy(buyer.as_ptr(), 1, 1_000_000_000);
        assert!(tokens2 > 0);
    }

    #[test]
    fn test_buy_cooldown_overflow_does_not_bypass() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_buy_cooldown(admin.as_ptr(), u64::MAX), 0);

        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        create_token(creator.as_ptr(), CREATION_FEE);

        let buyer = [3u8; 32];
        test_mock::set_timestamp(10_000);
        test_mock::set_caller(buyer);
        test_mock::set_value(1_000_000_000);
        assert!(buy(buyer.as_ptr(), 1, 1_000_000_000) > 0);

        test_mock::set_timestamp(20_000);
        test_mock::set_value(1_000_000_000);
        assert_eq!(buy(buyer.as_ptr(), 1, 1_000_000_000), ERROR_RETURN);
    }

    #[test]
    fn test_sell_cooldown() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        // CON-05: Configure LICN token so transfer_licn_out succeeds
        let licn = [42u8; 32];
        set_licn_token(admin.as_ptr(), licn.as_ptr());
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        create_token(creator.as_ptr(), CREATION_FEE);

        let buyer = [3u8; 32];
        test_mock::set_timestamp(10_000);
        test_mock::set_caller(buyer);
        test_mock::set_value(1_000_000_000);
        let tokens = buy(buyer.as_ptr(), 1, 1_000_000_000);
        assert!(tokens > 0);

        // Sell within sell cooldown (default 13 slots)
        test_mock::set_timestamp(10_010);
        assert_eq!(sell(buyer.as_ptr(), 1, tokens / 2), ERROR_RETURN);

        // After sell cooldown
        test_mock::set_timestamp(10_014);
        let refund = sell(buyer.as_ptr(), 1, tokens / 2);
        assert!(refund > 0);
    }

    #[test]
    fn test_max_buy_limit() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        create_token(creator.as_ptr(), CREATION_FEE);

        // Set low max buy
        test_mock::set_caller(admin);
        assert_eq!(set_max_buy(admin.as_ptr(), 500_000_000), 0); // 0.5 LICN

        let buyer = [3u8; 32];
        test_mock::set_timestamp(10_000);
        test_mock::set_caller(buyer);
        // Over limit rejected (max buy check is before caller check)
        test_mock::set_value(1_000_000_000);
        assert_eq!(buy(buyer.as_ptr(), 1, 1_000_000_000), ERROR_RETURN);
        // Under limit works
        test_mock::set_value(400_000_000);
        let tokens = buy(buyer.as_ptr(), 1, 400_000_000);
        assert!(tokens > 0);
    }

    #[test]
    fn test_admin_set_cooldowns() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        assert_eq!(set_buy_cooldown(admin.as_ptr(), 5), 0);
        assert_eq!(get_buy_cooldown(), 5);

        assert_eq!(set_sell_cooldown(admin.as_ptr(), 25), 0);
        assert_eq!(get_sell_cooldown(), 25);

        // Non-admin rejected
        let other = [9u8; 32];
        test_mock::set_caller(other);
        assert_eq!(set_buy_cooldown(other.as_ptr(), 1), 1);
        assert_eq!(set_sell_cooldown(other.as_ptr(), 1), 1);
    }

    #[test]
    fn test_set_creator_royalty() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        assert_eq!(set_creator_royalty(admin.as_ptr(), 100), 0);
        assert_eq!(get_creator_royalty(), 100);

        // Over 10% rejected
        assert_eq!(set_creator_royalty(admin.as_ptr(), 1001), 2);

        // Non-admin rejected
        let other = [9u8; 32];
        test_mock::set_caller(other);
        assert_eq!(set_creator_royalty(other.as_ptr(), 50), 1);
    }

    #[test]
    fn test_withdraw_fees() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        // CON-05: Configure LICN token so transfer_licn_out succeeds
        let licn = [42u8; 32];
        set_licn_token(admin.as_ptr(), licn.as_ptr());
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        create_token(creator.as_ptr(), CREATION_FEE);

        let fees_before = load_u64(b"cp_fees_collected");
        assert!(fees_before > 0);

        // Withdraw some
        test_mock::set_caller(admin);
        assert_eq!(withdraw_fees(admin.as_ptr(), CREATION_FEE / 2), 0);
        assert_eq!(
            load_u64(b"cp_fees_collected"),
            fees_before - CREATION_FEE / 2
        );

        // Over-withdraw rejected
        assert_eq!(withdraw_fees(admin.as_ptr(), 999_999_999_999), 3);

        // Zero rejected
        assert_eq!(withdraw_fees(admin.as_ptr(), 0), 2);

        // Non-admin rejected
        let other = [9u8; 32];
        test_mock::set_caller(other);
        assert_eq!(withdraw_fees(other.as_ptr(), 1), 1);
    }

    #[test]
    fn test_set_max_buy_zero_rejected() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_max_buy(admin.as_ptr(), 0), 2);
    }

    #[test]
    fn test_default_values() {
        setup();
        assert_eq!(get_buy_cooldown(), DEFAULT_BUY_COOLDOWN_SLOTS);
        assert_eq!(get_sell_cooldown(), DEFAULT_SELL_COOLDOWN_SLOTS);
        assert_eq!(get_max_buy(), DEFAULT_MAX_BUY_AMOUNT);
        assert_eq!(get_creator_royalty(), DEFAULT_CREATOR_ROYALTY_BPS);
    }

    // ========================================================================
    // DEX MIGRATION TESTS (Task 2.7)
    // ========================================================================

    #[test]
    fn test_set_dex_addresses() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let core_addr = [10u8; 32];
        let amm_addr = [20u8; 32];
        let result = set_dex_addresses(admin.as_ptr(), core_addr.as_ptr(), amm_addr.as_ptr());
        assert_eq!(result, 0);

        // Verify stored
        let stored_core = test_mock::get_storage(DEX_CORE_ADDRESS_KEY);
        assert_eq!(stored_core, Some(core_addr.to_vec()));
        let stored_amm = test_mock::get_storage(DEX_AMM_ADDRESS_KEY);
        assert_eq!(stored_amm, Some(amm_addr.to_vec()));
    }

    #[test]
    fn test_set_dex_addresses_not_admin() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let other = [9u8; 32];
        let core_addr = [10u8; 32];
        let amm_addr = [20u8; 32];
        test_mock::set_caller(other);
        assert_eq!(
            set_dex_addresses(other.as_ptr(), core_addr.as_ptr(), amm_addr.as_ptr()),
            1
        );
    }

    #[test]
    fn test_set_dex_addresses_zero_core() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let zero = [0u8; 32];
        let amm_addr = [20u8; 32];
        assert_eq!(
            set_dex_addresses(admin.as_ptr(), zero.as_ptr(), amm_addr.as_ptr()),
            2
        );
    }

    #[test]
    fn test_set_dex_addresses_zero_amm() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let core_addr = [10u8; 32];
        let zero = [0u8; 32];
        assert_eq!(
            set_dex_addresses(admin.as_ptr(), core_addr.as_ptr(), zero.as_ptr()),
            3
        );
    }

    #[test]
    fn test_set_dex_addresses_cannot_reconfigure() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let core_addr = [10u8; 32];
        let amm_addr = [20u8; 32];
        assert_eq!(
            set_dex_addresses(admin.as_ptr(), core_addr.as_ptr(), amm_addr.as_ptr()),
            0
        );

        let new_core = [11u8; 32];
        let new_amm = [21u8; 32];
        assert_eq!(
            set_dex_addresses(admin.as_ptr(), new_core.as_ptr(), new_amm.as_ptr()),
            4
        );
        assert_eq!(
            test_mock::get_storage(DEX_CORE_ADDRESS_KEY),
            Some(core_addr.to_vec())
        );
        assert_eq!(
            test_mock::get_storage(DEX_AMM_ADDRESS_KEY),
            Some(amm_addr.to_vec())
        );
    }

    #[test]
    fn test_threshold_crossing_enters_eligible_and_closes_buys() {
        setup();
        let (token_id, _, buyer) = create_threshold_token();
        test_mock::set_timestamp(20_000);
        test_mock::set_value(1_000_000_000);
        assert_eq!(buy(buyer.as_ptr(), token_id, 1_000_000_000), ERROR_RETURN);
        assert_eq!(get_graduation_status(token_id), 0);
        let status = test_mock::get_return_data();
        assert_eq!(status.len(), 73);
        assert_eq!(status[0], GRADUATION_ELIGIBLE);
        assert_eq!(bytes_to_u64(&status[1..9]), 100);
    }

    #[test]
    fn test_eligible_sell_below_threshold_returns_to_active() {
        setup();
        let admin = [1u8; 32];
        let (token_id, _, buyer) = create_threshold_token();
        test_mock::set_caller(admin);
        assert_eq!(set_licn_token(admin.as_ptr(), [42u8; 32].as_ptr()), 0);
        let token_key = graduation_key(b"cpt:", token_id);
        let data = test_mock::get_storage(&token_key).unwrap();
        let supply = bytes_to_u64(&data[32..40]);
        store_u64(&launchpad_balance_key(token_id, &buyer), supply);
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        test_mock::set_timestamp(30_000);
        test_mock::set_caller(buyer);
        let refund = sell(buyer.as_ptr(), token_id, supply / 2);
        assert_ne!(refund, ERROR_RETURN);
        assert!(refund > 0);
        assert_eq!(graduation_state(token_id), GRADUATION_ACTIVE);
    }

    #[test]
    fn test_get_graduation_info_initial() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        assert_eq!(get_graduation_info(), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 10);
        // revenue=0, core_set=0, amm_set=0
        assert_eq!(bytes_to_u64(&ret[0..8]), 0);
        assert_eq!(ret[8], 0);
        assert_eq!(ret[9], 0);
    }

    #[test]
    fn test_get_graduation_info_after_address_set() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let core_addr = [10u8; 32];
        let amm_addr = [20u8; 32];
        set_dex_addresses(admin.as_ptr(), core_addr.as_ptr(), amm_addr.as_ptr());

        assert_eq!(get_graduation_info(), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 10);
        assert_eq!(bytes_to_u64(&ret[0..8]), 0); // no revenue yet
        assert_eq!(ret[8], 1); // core_set
        assert_eq!(ret[9], 1); // amm_set
    }

    #[test]
    fn test_migration_validates_hash_and_provenance_then_timeout_aborts() {
        setup();
        let admin = [1u8; 32];
        let governance = [4u8; 32];
        let candidate = [5u8; 32];
        let template_hash = [6u8; 32];
        let (token_id, creator, buyer) = create_threshold_token();
        test_mock::set_caller(admin);
        let core_addr = [10u8; 32];
        let amm_addr = [20u8; 32];
        assert_eq!(
            set_dex_addresses(admin.as_ptr(), core_addr.as_ptr(), amm_addr.as_ptr()),
            0
        );
        assert_eq!(
            set_graduation_governance(admin.as_ptr(), governance.as_ptr()),
            0
        );
        test_mock::set_caller(governance);
        assert_eq!(
            set_graduation_config(
                governance.as_ptr(),
                [30u8; 32].as_ptr(),
                template_hash.as_ptr(),
                1,
                1,
                1_000,
                2,
            ),
            0
        );

        test_mock::set_contract_address([9u8; 32]);
        test_mock::set_caller(buyer);
        assert_eq!(
            begin_migration(buyer.as_ptr(), token_id, candidate.as_ptr()),
            5
        );
        test_mock::set_contract_code_hash(candidate, template_hash);
        let data = token_record(token_id).unwrap();
        let mut provenance = Vec::with_capacity(88);
        provenance.extend_from_slice(&[9u8; 32]);
        provenance.extend_from_slice(&u64_to_bytes(token_id));
        provenance.extend_from_slice(&creator);
        provenance.extend_from_slice(&data[48..56]);
        provenance.extend_from_slice(&data[32..40]);
        test_mock::set_cross_call_response(Some(provenance));
        test_mock::set_slot(200);
        assert_eq!(
            begin_migration(buyer.as_ptr(), token_id, candidate.as_ptr()),
            0
        );
        assert_eq!(graduation_state(token_id), GRADUATION_MIGRATING);
        test_mock::set_value(1_000_000_000);
        assert_eq!(buy(buyer.as_ptr(), token_id, 1_000_000_000), ERROR_RETURN);
        assert_eq!(sell(buyer.as_ptr(), token_id, 1), ERROR_RETURN);
        assert_eq!(abort_migration(buyer.as_ptr(), token_id), 2);
        test_mock::set_slot(200 + MIGRATION_TIMEOUT_SLOTS);
        assert_eq!(abort_migration(buyer.as_ptr(), token_id), 0);
        assert_eq!(graduation_state(token_id), GRADUATION_ELIGIBLE);
        assert_eq!(graduation_candidate(token_id), None);
    }

    fn prepare_migrating_token() -> (u64, [u8; 32], [u8; 32]) {
        let admin = [1u8; 32];
        let governance = [4u8; 32];
        let candidate = [5u8; 32];
        let keeper = [7u8; 32];
        let (token_id, _, _) = create_threshold_token();
        test_mock::set_caller(admin);
        assert_eq!(
            set_dex_addresses(admin.as_ptr(), [10u8; 32].as_ptr(), [20u8; 32].as_ptr()),
            0
        );
        assert_eq!(
            set_graduation_governance(admin.as_ptr(), governance.as_ptr()),
            0
        );
        test_mock::set_caller(governance);
        assert_eq!(
            set_graduation_config(
                governance.as_ptr(),
                [30u8; 32].as_ptr(),
                [6u8; 32].as_ptr(),
                1,
                1,
                1_000,
                2,
            ),
            0
        );
        storage_set(&graduation_key(b"cpgt:", token_id), &candidate);
        set_graduation_u64(b"cpgb:", token_id, 100);
        set_graduation_state(token_id, GRADUATION_MIGRATING);
        test_mock::set_contract_address([9u8; 32]);
        test_mock::set_caller(keeper);
        (token_id, candidate, keeper)
    }

    fn finalization_responses(
        token_liquidity: u64,
        licn_liquidity: u64,
        reverse_route: Vec<u8>,
    ) -> Vec<Vec<u8>> {
        let mut deposit = Vec::with_capacity(24);
        deposit.extend_from_slice(&u64_to_bytes(13));
        deposit.extend_from_slice(&u64_to_bytes(token_liquidity));
        deposit.extend_from_slice(&u64_to_bytes(licn_liquidity));
        vec![
            0u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
            u64_to_bytes(11).to_vec(),
            u64_to_bytes(12).to_vec(),
            deposit,
            u64_to_bytes(14).to_vec(),
            reverse_route,
        ]
    }

    #[test]
    fn test_finalize_migration_commits_exact_ids_and_actual_liquidity_last() {
        setup();
        let (token_id, _, keeper) = prepare_migrating_token();
        let data = token_record(token_id).unwrap();
        let supply = bytes_to_u64(&data[32..40]);
        let raised = bytes_to_u64(&data[40..48]);
        let licn_limit = raised * GRADUATION_LIQUIDITY_PERCENT / 100;
        let token_limit = u128_to_u64_saturating(
            licn_limit as u128 * 1_000_000_000u128 / current_price(supply) as u128,
        );
        let actual_token = token_limit - 9;
        let actual_licn = licn_limit - 7;
        test_mock::set_cross_call_responses(finalization_responses(
            actual_token,
            actual_licn,
            u64_to_bytes(15).to_vec(),
        ));

        assert_eq!(finalize_migration(keeper.as_ptr(), token_id), 0);
        assert_eq!(graduation_state(token_id), GRADUATION_GRADUATED);
        assert_eq!(get_graduation_u64(b"cpgp:", token_id), 11);
        assert_eq!(get_graduation_u64(b"cpga:", token_id), 12);
        assert_eq!(get_graduation_u64(b"cpgr:", token_id), 14);
        assert_eq!(get_graduation_u64(b"cpgr2:", token_id), 15);
        assert_eq!(get_graduation_u64(b"cpgpos:", token_id), 13);
        let liquidity = storage_get(&graduation_key(b"cpgl:", token_id)).unwrap();
        assert_eq!(bytes_to_u64(&liquidity[0..8]), actual_licn);
        assert_eq!(bytes_to_u64(&liquidity[8..16]), actual_token);
        assert_eq!(load_u64(b"cp_graduation_revenue"), raised - actual_licn);
        assert_eq!(load_u64(b"cp_graduation_revision"), 1);
        assert_eq!(token_record(token_id).unwrap()[64], 1);
        assert_eq!(test_mock::get_return_data().len(), 40);
    }

    #[test]
    fn test_finalize_migration_late_failure_writes_no_local_completion_state() {
        setup();
        let (token_id, _, keeper) = prepare_migrating_token();
        let data = token_record(token_id).unwrap();
        let supply = bytes_to_u64(&data[32..40]);
        let raised = bytes_to_u64(&data[40..48]);
        let licn_limit = raised * GRADUATION_LIQUIDITY_PERCENT / 100;
        let token_limit = u128_to_u64_saturating(
            licn_limit as u128 * 1_000_000_000u128 / current_price(supply) as u128,
        );
        test_mock::set_cross_call_responses(finalization_responses(
            token_limit,
            licn_limit,
            Vec::new(),
        ));

        assert_eq!(finalize_migration(keeper.as_ptr(), token_id), 15);
        assert_eq!(graduation_state(token_id), GRADUATION_MIGRATING);
        assert_eq!(get_graduation_u64(b"cpgp:", token_id), 0);
        assert_eq!(get_graduation_u64(b"cpga:", token_id), 0);
        assert_eq!(storage_get(&graduation_key(b"cpgl:", token_id)), None);
        assert_eq!(load_u64(b"cp_graduation_revenue"), 0);
        assert_eq!(load_u64(b"cp_graduation_revision"), 0);
        assert_eq!(token_record(token_id).unwrap()[64], 0);
    }

    #[test]
    fn test_graduation_claim_is_candidate_only_and_exactly_once() {
        setup();
        let admin = [1u8; 32];
        let holder = [3u8; 32];
        let candidate = [5u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let token_id = create_token(creator.as_ptr(), CREATION_FEE);
        set_graduation_state(token_id, GRADUATION_GRADUATED);
        storage_set(&graduation_key(b"cpgt:", token_id), &candidate);
        store_u64(&launchpad_balance_key(token_id, &holder), 777);
        test_mock::set_caller([8u8; 32]);
        assert_eq!(consume_graduation_claim(token_id, holder.as_ptr()), 0);
        assert_eq!(load_u64(&launchpad_balance_key(token_id, &holder)), 777);
        test_mock::set_caller(candidate);
        assert_eq!(consume_graduation_claim(token_id, holder.as_ptr()), 777);
        assert_eq!(test_mock::get_return_data(), u64_to_bytes(777));
        assert_eq!(load_u64(&launchpad_balance_key(token_id, &holder)), 0);
        assert_eq!(consume_graduation_claim(token_id, holder.as_ptr()), 0);
    }

    // ========================================================================
    // G24-01: Financial wiring tests
    // ========================================================================

    #[test]
    fn test_g24_buy_requires_get_value() {
        // buy() must verify get_value() >= licn_amount
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        create_token(creator.as_ptr(), CREATION_FEE);

        let buyer = [3u8; 32];
        test_mock::set_caller(buyer);
        // Attempt buy with insufficient get_value
        test_mock::set_value(500_000_000); // 0.5 LICN
        assert_eq!(
            buy(buyer.as_ptr(), 1, 1_000_000_000),
            ERROR_RETURN,
            "Buy should fail: payment < amount"
        );
        // With sufficient value succeeds
        test_mock::set_value(1_000_000_000);
        let tokens = buy(buyer.as_ptr(), 1, 1_000_000_000);
        assert!(tokens > 0, "Buy should succeed with sufficient value");
    }

    #[test]
    fn test_g24_create_token_requires_get_value() {
        // create_token() must verify get_value() >= CREATION_FEE
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        // No value attached — should fail
        test_mock::set_value(0);
        assert_eq!(
            create_token(creator.as_ptr(), CREATION_FEE),
            ERROR_RETURN,
            "Create token should fail: no value"
        );
        // Exact fee attached — should succeed
        test_mock::set_value(CREATION_FEE);
        assert_eq!(create_token(creator.as_ptr(), CREATION_FEE), 1);
    }

    #[test]
    fn test_g24_sell_triggers_transfer() {
        // sell() calls transfer_licn_out to refund seller
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        // CON-05: Configure LICN token so transfer_licn_out succeeds
        let licn = [42u8; 32];
        set_licn_token(admin.as_ptr(), licn.as_ptr());
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        create_token(creator.as_ptr(), CREATION_FEE);

        let buyer = [3u8; 32];
        test_mock::set_timestamp(10_000);
        test_mock::set_caller(buyer);
        test_mock::set_value(1_000_000_000);
        let bought = buy(buyer.as_ptr(), 1, 1_000_000_000);
        assert!(bought > 0);

        // Sell after cooldown — refund should be > 0 (transfer_licn_out returns
        // true via graceful degradation when LICN token address is not configured)
        test_mock::set_timestamp(20_000);
        let refund = sell(buyer.as_ptr(), 1, bought / 2);
        assert!(refund > 0, "Sell should return refund amount");
    }

    #[test]
    fn test_g24_set_licn_token() {
        // Admin can set LICN token address for outgoing transfers
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let token = [42u8; 32];
        let other = [9u8; 32];

        test_mock::set_caller(other);
        assert_eq!(set_licn_token(other.as_ptr(), token.as_ptr()), 1);

        test_mock::set_caller(admin);
        assert_eq!(set_licn_token(admin.as_ptr(), token.as_ptr()), 0);
        let stored = test_mock::get_storage(LICN_TOKEN_KEY);
        assert_eq!(stored, Some(token.to_vec()));

        let zero = [0u8; 32];
        assert_eq!(set_licn_token(admin.as_ptr(), zero.as_ptr()), 2);
    }

    #[test]
    fn test_g24_set_licn_token_allows_native_sentinel_first() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let zero = [0u8; 32];
        assert_eq!(set_licn_token(admin.as_ptr(), zero.as_ptr()), 0);
        assert_eq!(test_mock::get_storage(LICN_TOKEN_KEY), Some(zero.to_vec()));

        let token = [42u8; 32];
        assert_eq!(set_licn_token(admin.as_ptr(), token.as_ptr()), 2);
    }

    #[test]
    fn test_g24_withdraw_fees_triggers_transfer() {
        // withdraw_fees() calls transfer_licn_out to send fees to admin
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        // CON-05: Configure LICN token so transfer_licn_out succeeds
        let licn = [42u8; 32];
        set_licn_token(admin.as_ptr(), licn.as_ptr());
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        create_token(creator.as_ptr(), CREATION_FEE);

        let fees = load_u64(b"cp_fees_collected");
        assert!(fees > 0);

        // Withdraw — should succeed (graceful degradation)
        test_mock::set_caller(admin);
        assert_eq!(withdraw_fees(admin.as_ptr(), fees / 2), 0);
        assert_eq!(load_u64(b"cp_fees_collected"), fees - fees / 2);
    }

    #[test]
    fn test_g24_threshold_without_dex_keeps_curve_active() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let token_id = create_token(creator.as_ptr(), CREATION_FEE);
        test_mock::set_caller(admin);
        set_max_buy(admin.as_ptr(), u64::MAX);

        // Set token state to above graduation threshold
        let id_hex = u64_to_hex(token_id);
        let token_key = make_key(b"cpt:", &id_hex);
        let mut data = test_mock::get_storage(&token_key).unwrap();
        let supply: u64 = 400_000_000_000_000;
        data[32..40].copy_from_slice(&u64_to_bytes(supply));
        data[40..48].copy_from_slice(&u64_to_bytes(50_000_000_000_000_000));
        storage_set(&token_key, &data);

        let buyer = [3u8; 32];
        test_mock::set_timestamp(10_000);
        test_mock::set_caller(buyer);
        test_mock::set_value(1_000_000_000_000);
        assert!(buy(buyer.as_ptr(), token_id, 1_000_000_000_000) > 0);

        let data2 = test_mock::get_storage(&token_key).unwrap();
        assert_eq!(data2[64], 0);

        test_mock::set_timestamp(15_000);
        test_mock::set_value(1_000_000_000);
        assert!(buy(buyer.as_ptr(), token_id, 1_000_000_000) > 0);
    }
}
