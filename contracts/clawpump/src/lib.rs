// ClawPump v2 - Token Launchpad with Bonding Curves
// Per whitepaper: fair-launch bonding curves for new token creation
// Automatic DEX graduation is reserved for a future release once ClawPump
// tokens have a real asset/pool migration path into ClawSwap.
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
use moltchain_sdk::{
    bytes_to_u64, call_token_transfer, get_caller, get_contract_address, get_timestamp, get_value,
    log_info, set_return_data, storage_get, storage_set, u64_to_bytes, Address,
};

// T5.12: Reentrancy guard
const REENTRANCY_KEY: &[u8] = b"_reentrancy";

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

/// Token creation fee: 10 MOLT (10,000,000,000 shells — $1.00 at $0.10/MOLT)
const CREATION_FEE: u64 = 10_000_000_000;

/// Default initial supply for bonding curve tokens
const DEFAULT_MAX_SUPPLY: u64 = 1_000_000_000_000_000_000; // 1B tokens * 10^9

/// Graduation threshold: when market cap reaches this, migrate to DEX
const GRADUATION_MARKET_CAP: u64 = 100_000_000_000_000; // 100K MOLT ($10K at $0.10)

/// Bonding curve slope factor (controls price steepness)
/// price = BASE_PRICE + (supply_sold * SLOPE / SLOPE_SCALE)
const BASE_PRICE: u64 = 1_000; // 0.000001 MOLT per token initially
const SLOPE: u64 = 1;
const SLOPE_SCALE: u64 = 1_000_000;

/// Platform fee on buys/sells: 1%
const PLATFORM_FEE_PERCENT: u64 = 1;

/// Admin key
const ADMIN_KEY: &[u8] = b"cp_admin";

// Token counter
const TOKEN_COUNT_KEY: &[u8] = b"cp_token_count";

// ============================================================================
// v2 CONSTANTS
// ============================================================================

/// Buy cooldown: minimum milliseconds between buys per user per token
const DEFAULT_BUY_COOLDOWN_MS: u64 = 2_000; // 2 seconds
/// Maximum MOLT that can be spent in a single buy
const DEFAULT_MAX_BUY_AMOUNT: u64 = 100_000_000_000_000; // 100K MOLT ($10K at $0.10)
/// Sell cooldown: minimum ms after buying before selling (anti-dump)
const DEFAULT_SELL_COOLDOWN_MS: u64 = 5_000; // 5 seconds
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
/// Percentage of raised MOLT seeded as liquidity on graduation (80%)
const GRADUATION_LIQUIDITY_PERCENT: u64 = 80;
/// Percentage of raised MOLT retained as platform revenue on graduation (20%)
const GRADUATION_PLATFORM_PERCENT: u64 = 20;

/// MOLT token contract address (for outgoing transfers in sell/withdraw)
const MOLT_TOKEN_KEY: &[u8] = b"cp_molt_token";

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

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key).map(|d| bytes_to_u64(&d)).unwrap_or(0)
}

fn store_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
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

fn is_token_frozen(token_id: u64) -> bool {
    let id_hex = u64_to_hex(token_id);
    let key = make_key(b"cpf:", &id_hex);
    storage_get(&key)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
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
        .unwrap_or(DEFAULT_BUY_COOLDOWN_MS)
}

fn get_sell_cooldown() -> u64 {
    storage_get(b"cp_sell_cooldown")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_SELL_COOLDOWN_MS)
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

/// G24-01: Transfer MOLT tokens from the contract to a recipient (self-custody).
/// Returns true on success, false if token address not configured or call errors.
fn transfer_molt_out(recipient: &[u8; 32], amount: u64) -> bool {
    let token_data = match storage_get(MOLT_TOKEN_KEY) {
        Some(data) if data.len() == 32 && data.iter().any(|&x| x != 0) => data,
        _ => {
            // AUDIT-FIX CON-05: MUST fail when MOLT token address is not configured.
            // Returning true here would silently succeed without transferring funds,
            // causing sells/withdrawals to appear successful with no actual payout.
            log_info("CRITICAL: MOLT token address not configured — transfer REJECTED");
            return false;
        }
    };
    let mut token = [0u8; 32];
    token.copy_from_slice(&token_data);
    let self_addr = get_contract_address();
    match call_token_transfer(Address(token), self_addr, Address(*recipient), amount) {
        Err(_) => {
            log_info("MOLT transfer failed");
            false
        }
        Ok(_) => true,
    }
}

// ============================================================================
// TOKEN LAUNCH LAYOUT (stored per token)
// ============================================================================
// Key: "cpt:{token_id_hex}" → [creator(32), supply_sold(8), molt_raised(8),
//                                max_supply(8), created_at(8), graduated(1)]
// Total: 65 bytes

const TOKEN_DATA_SIZE: usize = 65;

// ============================================================================
// INITIALIZATION
// ============================================================================

/// Initialize ClawPump
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

    log_info("ClawPump initialized");
    0
}

// ============================================================================
// TOKEN CREATION
// ============================================================================

/// Create a new token on the bonding curve
/// Returns token ID (0 on failure)
#[no_mangle]
pub extern "C" fn create_token(creator_ptr: *const u8, fee_paid: u64) -> u64 {
    let mut creator = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(creator_ptr, creator.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != creator {
        return 200;
    }

    // G24-01: Verify actual payment via get_value() instead of trusting parameter
    if get_value() < CREATION_FEE {
        log_info("Insufficient creation fee (need 10 MOLT)");
        return 0;
    }

    if fee_paid < CREATION_FEE {
        log_info("Insufficient creation fee (need 0.1 MOLT)");
        return 0;
    }

    let token_id = load_u64(TOKEN_COUNT_KEY) + 1;
    let id_hex = u64_to_hex(token_id);

    // Store token data
    let mut data = Vec::with_capacity(TOKEN_DATA_SIZE);
    data.extend_from_slice(&creator); // creator: 32 bytes
    data.extend_from_slice(&u64_to_bytes(0)); // supply_sold: 0
    data.extend_from_slice(&u64_to_bytes(0)); // molt_raised: 0
    data.extend_from_slice(&u64_to_bytes(DEFAULT_MAX_SUPPLY)); // max_supply
    data.extend_from_slice(&u64_to_bytes(get_timestamp())); // created_at
    data.push(0); // graduated: false

    let token_key = make_key(b"cpt:", &id_hex);
    storage_set(&token_key, &data);
    store_u64(TOKEN_COUNT_KEY, token_id);

    // Collect creation fee
    let fees = load_u64(b"cp_fees_collected");
    store_u64(b"cp_fees_collected", fees.saturating_add(fee_paid));

    log_info("🪙 New token created on bonding curve");
    token_id
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
    let linear_part = base * a;
    let quadratic_part = slope * a * (2 * s + a) / (2 * scale);
    ((linear_part + quadratic_part) / norm) as u64
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
    let linear_part = base * a;
    let quadratic_part = slope * a * (2 * s - a) / (2 * scale);
    ((linear_part + quadratic_part) / norm) as u64
}

/// Get current token price (shells per token)
fn current_price(supply_sold: u64) -> u64 {
    // SECURITY-FIX: Use u128 intermediate to prevent overflow
    BASE_PRICE + ((supply_sold as u128 * SLOPE as u128 / SLOPE_SCALE as u128) as u64)
}

// ============================================================================
// BUY / SELL
// ============================================================================

/// Buy tokens on the bonding curve
/// Returns number of tokens received (0 on failure)
#[no_mangle]
pub extern "C" fn buy(buyer_ptr: *const u8, token_id: u64, molt_amount: u64) -> u64 {
    if molt_amount == 0 {
        return 0;
    }
    if is_paused() {
        log_info("Protocol is paused");
        return 0;
    }
    if is_token_frozen(token_id) {
        log_info("Token is frozen");
        return 0;
    }
    if !reentrancy_enter() {
        log_info("Reentrancy detected");
        return 0;
    }

    // v2: Max buy per tx
    let max_buy = get_max_buy();
    if molt_amount > max_buy {
        reentrancy_exit();
        log_info("Exceeds max buy per transaction");
        return 0;
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

    // G24-01: Verify actual payment via get_value() instead of trusting parameter
    if get_value() < molt_amount {
        reentrancy_exit();
        log_info("Insufficient payment for buy");
        return 0;
    }

    let buyer_hex = hex_encode_addr(&buyer);

    // v2: Buy cooldown
    let cooldown = get_buy_cooldown();
    let lbk = last_buy_key(token_id, &buyer_hex);
    let last_buy_ts = load_u64(&lbk);
    let now = get_timestamp();
    if last_buy_ts > 0 && now < last_buy_ts + cooldown {
        reentrancy_exit();
        log_info("Buy cooldown not expired");
        return 0;
    }

    let id_hex = u64_to_hex(token_id);
    let token_key = make_key(b"cpt:", &id_hex);

    let mut data = match storage_get(&token_key) {
        Some(d) if d.len() >= TOKEN_DATA_SIZE => d,
        _ => {
            log_info("Token not found");
            return 0;
        }
    };

    // Check not graduated
    if data[64] != 0 {
        log_info("Token graduated to DEX, trade there");
        return 0;
    }

    let supply_sold = bytes_to_u64(&data[32..40]);
    let molt_raised = bytes_to_u64(&data[40..48]);
    let max_supply = bytes_to_u64(&data[48..56]);

    // Platform fee
    let fee = molt_amount * PLATFORM_FEE_PERCENT / 100;
    let net_amount = molt_amount - fee;

    // Binary search for how many tokens we can buy with net_amount
    let mut lo: u64 = 0;
    let mut hi: u64 = max_supply.saturating_sub(supply_sold);
    if hi > 1_000_000_000_000 {
        hi = 1_000_000_000_000; // Cap search
    }

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
        return 0;
    }

    let actual_cost = calculate_buy_cost(supply_sold, tokens_bought);
    let new_supply = supply_sold + tokens_bought;
    let new_raised = molt_raised + actual_cost;

    // Update token data
    data[32..40].copy_from_slice(&u64_to_bytes(new_supply));
    data[40..48].copy_from_slice(&u64_to_bytes(new_raised));
    storage_set(&token_key, &data);

    // Track buyer balance
    let buyer_hex = hex_encode_addr(&buyer);
    let mut bal_key = Vec::with_capacity(4 + 16 + 1 + 64);
    bal_key.extend_from_slice(b"bal:");
    bal_key.extend_from_slice(&id_hex);
    bal_key.push(b':');
    bal_key.extend_from_slice(&buyer_hex);
    let prev_bal = load_u64(&bal_key);
    store_u64(&bal_key, prev_bal.saturating_add(tokens_bought));

    // Collect platform fee
    let fees = load_u64(b"cp_fees_collected");
    store_u64(b"cp_fees_collected", fees.saturating_add(fee));

    // v2: Creator royalty
    let royalty_bps = get_creator_royalty();
    if royalty_bps > 0 {
        let royalty = (actual_cost as u128 * royalty_bps as u128 / BPS_SCALE as u128) as u64;
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

    // Check graduation (use u128 to prevent overflow with large supplies)
    let market_cap =
        (current_price(new_supply) as u128 * new_supply as u128 / 1_000_000_000u128) as u64;
    if market_cap >= GRADUATION_MARKET_CAP {
        let dex_core_bytes = storage_get(DEX_CORE_ADDRESS_KEY);
        let dex_amm_bytes = storage_get(DEX_AMM_ADDRESS_KEY);

        let has_core = dex_core_bytes
            .as_ref()
            .map(|b| b.len() == 32 && b.iter().any(|&x| x != 0))
            .unwrap_or(false);
        let has_amm = dex_amm_bytes
            .as_ref()
            .map(|b| b.len() == 32 && b.iter().any(|&x| x != 0))
            .unwrap_or(false);

        if has_core && has_amm {
            let _ = (GRADUATION_LIQUIDITY_PERCENT, GRADUATION_PLATFORM_PERCENT);
            log_info(
                "Graduation threshold reached, but automatic DEX migration is disabled until ClawPump exposes a real ABI-compatible asset and pool migration path",
            );
        } else {
            log_info(
                "Graduation threshold reached, but no automatic DEX migration path is configured — token remains on bonding curve",
            );
        }
    }

    log_info("Buy successful");
    reentrancy_exit();
    tokens_bought
}

/// Sell tokens back to the bonding curve
/// Returns MOLT refund amount (0 on failure)
#[no_mangle]
pub extern "C" fn sell(seller_ptr: *const u8, token_id: u64, token_amount: u64) -> u64 {
    if token_amount == 0 {
        return 0;
    }
    if is_paused() {
        log_info("Protocol is paused");
        return 0;
    }
    if is_token_frozen(token_id) {
        log_info("Token is frozen");
        return 0;
    }
    if !reentrancy_enter() {
        log_info("Reentrancy detected");
        return 0;
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

    let seller_hex = hex_encode_addr(&seller);

    // v2: Sell cooldown — check last buy timestamp
    let sell_cd = get_sell_cooldown();
    let lbk = last_buy_key(token_id, &seller_hex);
    let last_buy_ts = load_u64(&lbk);
    let now = get_timestamp();
    if last_buy_ts > 0 && now < last_buy_ts + sell_cd {
        reentrancy_exit();
        log_info("Sell cooldown not expired (anti-dump)");
        return 0;
    }

    let id_hex = u64_to_hex(token_id);
    let token_key = make_key(b"cpt:", &id_hex);

    let mut data = match storage_get(&token_key) {
        Some(d) if d.len() >= TOKEN_DATA_SIZE => d,
        _ => {
            log_info("Token not found");
            return 0;
        }
    };

    if data[64] != 0 {
        log_info("Token graduated, trade on DEX");
        return 0;
    }

    // Check seller balance
    let mut bal_key = Vec::with_capacity(4 + 16 + 1 + 64);
    bal_key.extend_from_slice(b"bal:");
    bal_key.extend_from_slice(&id_hex);
    bal_key.push(b':');
    bal_key.extend_from_slice(&seller_hex);
    let balance = load_u64(&bal_key);

    if token_amount > balance {
        log_info("Insufficient token balance");
        return 0;
    }

    let supply_sold = bytes_to_u64(&data[32..40]);
    let molt_raised = bytes_to_u64(&data[40..48]);

    let raw_refund = calculate_sell_refund(supply_sold, token_amount);
    let fee = raw_refund * PLATFORM_FEE_PERCENT / 100;
    let net_refund = raw_refund - fee;

    // Update token data
    let new_supply = supply_sold - token_amount;
    let new_raised = molt_raised.saturating_sub(raw_refund);
    data[32..40].copy_from_slice(&u64_to_bytes(new_supply));
    data[40..48].copy_from_slice(&u64_to_bytes(new_raised));
    storage_set(&token_key, &data);

    // Update seller balance
    store_u64(&bal_key, balance - token_amount);

    // Collect fee
    let fees = load_u64(b"cp_fees_collected");
    store_u64(b"cp_fees_collected", fees.saturating_add(fee));

    // G24-01: Transfer MOLT refund to seller (self-custody)
    if !transfer_molt_out(&seller, net_refund) {
        // Revert state changes on transfer failure
        data[32..40].copy_from_slice(&u64_to_bytes(supply_sold));
        data[40..48].copy_from_slice(&u64_to_bytes(molt_raised));
        storage_set(&token_key, &data);
        store_u64(&bal_key, balance);
        store_u64(b"cp_fees_collected", fees);
        log_info("Sell reverted: MOLT transfer failed");
        reentrancy_exit();
        return 0;
    }

    log_info("Sell successful");
    reentrancy_exit();
    net_refund
}

// ============================================================================
// VIEW FUNCTIONS
// ============================================================================

/// Get token info: [supply_sold(8), molt_raised(8), current_price(8), market_cap(8), graduated(1)]
#[no_mangle]
pub extern "C" fn get_token_info(token_id: u64) -> u32 {
    let id_hex = u64_to_hex(token_id);
    let token_key = make_key(b"cpt:", &id_hex);

    let data = match storage_get(&token_key) {
        Some(d) if d.len() >= TOKEN_DATA_SIZE => d,
        _ => return 1,
    };

    let supply_sold = bytes_to_u64(&data[32..40]);
    let molt_raised = bytes_to_u64(&data[40..48]);
    let price = current_price(supply_sold);
    let market_cap = (price as u128 * supply_sold as u128 / 1_000_000_000u128) as u64;

    let mut result = Vec::with_capacity(33);
    result.extend_from_slice(&u64_to_bytes(supply_sold));
    result.extend_from_slice(&u64_to_bytes(molt_raised));
    result.extend_from_slice(&u64_to_bytes(price));
    result.extend_from_slice(&u64_to_bytes(market_cap));
    result.push(data[64]); // graduated flag
    set_return_data(&result);
    0
}

/// Get buy quote: how many tokens for given MOLT amount
#[no_mangle]
pub extern "C" fn get_buy_quote(token_id: u64, molt_amount: u64) -> u64 {
    let id_hex = u64_to_hex(token_id);
    let token_key = make_key(b"cpt:", &id_hex);

    let data = match storage_get(&token_key) {
        Some(d) if d.len() >= TOKEN_DATA_SIZE => d,
        _ => return 0,
    };

    let supply_sold = bytes_to_u64(&data[32..40]);
    let max_supply = bytes_to_u64(&data[48..56]);
    let net = molt_amount * (100 - PLATFORM_FEE_PERCENT) / 100;

    let mut lo: u64 = 0;
    let mut hi = max_supply
        .saturating_sub(supply_sold)
        .min(1_000_000_000_000);
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
    log_info("ClawPump paused");
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
    log_info("ClawPump unpaused");
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

/// Admin sets buy cooldown (ms)
#[no_mangle]
pub extern "C" fn set_buy_cooldown(caller_ptr: *const u8, cooldown_ms: u64) -> u32 {
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
    store_u64(b"cp_buy_cooldown", cooldown_ms);
    0
}

/// Admin sets sell cooldown (ms)
#[no_mangle]
pub extern "C" fn set_sell_cooldown(caller_ptr: *const u8, cooldown_ms: u64) -> u32 {
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
    store_u64(b"cp_sell_cooldown", cooldown_ms);
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

    // G24-01: Transfer MOLT to admin (self-custody)
    if !transfer_molt_out(&caller, amount) {
        // Revert on transfer failure
        store_u64(b"cp_fees_collected", fees);
        log_info("Fee withdrawal reverted: MOLT transfer failed");
        return 4;
    }

    log_info("Fees withdrawn");
    0
}

/// Admin sets the MOLT token contract address (for outgoing transfers)
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

    if !is_admin(&caller) {
        return 1;
    }

    let mut token = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(token_ptr, token.as_mut_ptr(), 32);
    }

    if token.iter().all(|&b| b == 0) {
        log_info("MOLT token address cannot be zero");
        return 2;
    }

    storage_set(MOLT_TOKEN_KEY, &token);
    log_info("MOLT token address configured");
    0
}

/// Admin sets DEX contract addresses for graduation migration
/// Both addresses must be non-zero 32-byte addresses
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

    storage_set(DEX_CORE_ADDRESS_KEY, &core_addr);
    storage_set(DEX_AMM_ADDRESS_KEY, &amm_addr);
    log_info("DEX addresses recorded for future graduation migration support");
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
    use moltchain_sdk::bytes_to_u64;
    use moltchain_sdk::test_mock;

    fn setup() {
        test_mock::reset();
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
        assert_eq!(create_token(creator.as_ptr(), CREATION_FEE - 1), 0);
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
        assert!(tokens > 0, "Should receive tokens for 1 MOLT");
    }

    #[test]
    fn test_buy_zero_amount() {
        setup();
        assert_eq!(buy([3u8; 32].as_ptr(), 1, 0), 0);
    }

    #[test]
    fn test_buy_nonexistent_token() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        test_mock::set_caller([3u8; 32]);
        test_mock::set_value(1_000_000_000);
        assert_eq!(buy([3u8; 32].as_ptr(), 999, 1_000_000_000), 0);
    }

    #[test]
    fn test_sell() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        // CON-05: Configure MOLT token so transfer_molt_out succeeds
        let molt = [42u8; 32];
        set_molt_token(admin.as_ptr(), molt.as_ptr());
        test_mock::set_cross_call_response(Some(vec![1u8]));
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
        // Advance past sell cooldown (default 5000ms)
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
        assert_eq!(sell([3u8; 32].as_ptr(), 1, 1000), 0);
    }

    #[test]
    fn test_sell_zero_amount() {
        setup();
        assert_eq!(sell([3u8; 32].as_ptr(), 1, 0), 0);
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
        let quote = get_buy_quote(tid, 1_000_000_000);
        assert!(quote > 0);
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
        assert_eq!(buy(buyer.as_ptr(), 1, 1_000_000_000), 0);
        // Sell blocked
        assert_eq!(sell(buyer.as_ptr(), 1, 100), 0);

        test_mock::set_caller(admin);
        assert_eq!(unpause(admin.as_ptr()), 0);
        assert!(!is_paused());
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
        assert_eq!(buy(buyer.as_ptr(), 1, 1_000_000_000), 0);

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

        // Second buy within cooldown (default 2000ms)
        test_mock::set_timestamp(11_000);
        assert_eq!(buy(buyer.as_ptr(), 1, 1_000_000_000), 0);

        // After cooldown
        test_mock::set_timestamp(13_000);
        let tokens2 = buy(buyer.as_ptr(), 1, 1_000_000_000);
        assert!(tokens2 > 0);
    }

    #[test]
    fn test_sell_cooldown() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        // CON-05: Configure MOLT token so transfer_molt_out succeeds
        let molt = [42u8; 32];
        set_molt_token(admin.as_ptr(), molt.as_ptr());
        test_mock::set_cross_call_response(Some(vec![1u8]));
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

        // Sell within sell cooldown (default 5000ms)
        test_mock::set_timestamp(12_000);
        assert_eq!(sell(buyer.as_ptr(), 1, tokens / 2), 0);

        // After sell cooldown
        test_mock::set_timestamp(16_000);
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
        assert_eq!(set_max_buy(admin.as_ptr(), 500_000_000), 0); // 0.5 MOLT

        let buyer = [3u8; 32];
        test_mock::set_timestamp(10_000);
        test_mock::set_caller(buyer);
        // Over limit rejected (max buy check is before caller check)
        test_mock::set_value(1_000_000_000);
        assert_eq!(buy(buyer.as_ptr(), 1, 1_000_000_000), 0);
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

        assert_eq!(set_buy_cooldown(admin.as_ptr(), 5000), 0);
        assert_eq!(get_buy_cooldown(), 5000);

        assert_eq!(set_sell_cooldown(admin.as_ptr(), 10000), 0);
        assert_eq!(get_sell_cooldown(), 10000);

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
        // CON-05: Configure MOLT token so transfer_molt_out succeeds
        let molt = [42u8; 32];
        set_molt_token(admin.as_ptr(), molt.as_ptr());
        test_mock::set_cross_call_response(Some(vec![1u8]));
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
        assert_eq!(get_buy_cooldown(), DEFAULT_BUY_COOLDOWN_MS);
        assert_eq!(get_sell_cooldown(), DEFAULT_SELL_COOLDOWN_MS);
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
    fn test_threshold_crossing_with_dex_addresses_keeps_token_on_curve() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        // Configure DEX addresses
        let core_addr = [10u8; 32];
        let amm_addr = [20u8; 32];
        set_dex_addresses(admin.as_ptr(), core_addr.as_ptr(), amm_addr.as_ptr());

        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let token_id = create_token(creator.as_ptr(), CREATION_FEE);
        let id_hex = u64_to_hex(token_id);
        let token_key = make_key(b"cpt:", &id_hex);
        let mut data = test_mock::get_storage(&token_key).unwrap();
        let near_supply: u64 = 400_000_000_000_000;
        data[32..40].copy_from_slice(&u64_to_bytes(near_supply));
        data[40..48].copy_from_slice(&u64_to_bytes(50_000_000_000_000_000));
        storage_set(&token_key, &data);

        let buyer = [3u8; 32];
        test_mock::set_timestamp(10_000);
        test_mock::set_caller(buyer);
        let buy_amt: u64 = 1_000_000_000_000;
        test_mock::set_value(buy_amt);
        assert!(buy(buyer.as_ptr(), token_id, buy_amt) > 0);

        let data2 = test_mock::get_storage(&token_key).unwrap();
        assert_eq!(data2[64], 0, "token must not be marked graduated");
        assert_eq!(
            load_u64(b"cp_graduation_revenue"),
            0,
            "no graduation revenue should be tracked while auto migration is disabled"
        );
    }

    #[test]
    fn test_threshold_crossing_without_dex_addresses_keeps_token_on_curve() {
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

        let id_hex = u64_to_hex(token_id);
        let token_key = make_key(b"cpt:", &id_hex);
        let mut data = test_mock::get_storage(&token_key).unwrap();
        let near_supply: u64 = 400_000_000_000_000;
        data[32..40].copy_from_slice(&u64_to_bytes(near_supply));
        data[40..48].copy_from_slice(&u64_to_bytes(50_000_000_000_000_000));
        storage_set(&token_key, &data);

        let buyer = [3u8; 32];
        test_mock::set_timestamp(10_000);
        test_mock::set_caller(buyer);
        let buy_amt: u64 = 1_000_000_000_000;
        test_mock::set_value(buy_amt);
        assert!(buy(buyer.as_ptr(), token_id, buy_amt) > 0);

        let revenue = load_u64(b"cp_graduation_revenue");
        assert_eq!(revenue, 0, "No graduation revenue without DEX addresses");

        let data2 = test_mock::get_storage(&token_key).unwrap();
        assert_eq!(
            data2[64], 0,
            "Token should stay on the bonding curve without a real migration path"
        );

        test_mock::set_timestamp(15_000);
        test_mock::set_value(1_000_000_000);
        assert!(
            buy(buyer.as_ptr(), token_id, 1_000_000_000) > 0,
            "further buys should remain possible"
        );
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
    fn test_threshold_crossing_does_not_block_buys() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let core_addr = [10u8; 32];
        let amm_addr = [20u8; 32];
        set_dex_addresses(admin.as_ptr(), core_addr.as_ptr(), amm_addr.as_ptr());
        set_max_buy(admin.as_ptr(), u64::MAX);

        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(CREATION_FEE);
        let token_id = create_token(creator.as_ptr(), CREATION_FEE);
        let id_hex = u64_to_hex(token_id);
        let token_key = make_key(b"cpt:", &id_hex);
        let mut data = test_mock::get_storage(&token_key).unwrap();
        data[32..40].copy_from_slice(&u64_to_bytes(400_000_000_000_000));
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

    #[test]
    fn test_threshold_crossing_does_not_block_sells() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let molt = [42u8; 32];
        set_molt_token(admin.as_ptr(), molt.as_ptr());
        test_mock::set_cross_call_response(Some(vec![1u8]));
        set_max_buy(admin.as_ptr(), u64::MAX);

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
        let mut data = test_mock::get_storage(&token_key).unwrap();
        data[32..40].copy_from_slice(&u64_to_bytes(400_000_000_000_000));
        data[40..48].copy_from_slice(&u64_to_bytes(50_000_000_000_000_000));
        storage_set(&token_key, &data);

        test_mock::set_timestamp(15_000);
        test_mock::set_value(1_000_000_000_000);
        assert!(buy(buyer.as_ptr(), token_id, 1_000_000_000_000) > 0);

        let data2 = test_mock::get_storage(&token_key).unwrap();
        assert_eq!(data2[64], 0);

        test_mock::set_timestamp(25_000);
        assert!(sell(buyer.as_ptr(), token_id, bought / 2) > 0);
    }

    // ========================================================================
    // G24-01: Financial wiring tests
    // ========================================================================

    #[test]
    fn test_g24_buy_requires_get_value() {
        // buy() must verify get_value() >= molt_amount
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
        test_mock::set_value(500_000_000); // 0.5 MOLT
        assert_eq!(
            buy(buyer.as_ptr(), 1, 1_000_000_000),
            0,
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
            0,
            "Create token should fail: no value"
        );
        // Exact fee attached — should succeed
        test_mock::set_value(CREATION_FEE);
        assert_eq!(create_token(creator.as_ptr(), CREATION_FEE), 1);
    }

    #[test]
    fn test_g24_sell_triggers_transfer() {
        // sell() calls transfer_molt_out to refund seller
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        // CON-05: Configure MOLT token so transfer_molt_out succeeds
        let molt = [42u8; 32];
        set_molt_token(admin.as_ptr(), molt.as_ptr());
        test_mock::set_cross_call_response(Some(vec![1u8]));
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

        // Sell after cooldown — refund should be > 0 (transfer_molt_out returns
        // true via graceful degradation when MOLT token address is not configured)
        test_mock::set_timestamp(20_000);
        let refund = sell(buyer.as_ptr(), 1, bought / 2);
        assert!(refund > 0, "Sell should return refund amount");
    }

    #[test]
    fn test_g24_set_molt_token() {
        // Admin can set MOLT token address for outgoing transfers
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let token = [42u8; 32];
        assert_eq!(set_molt_token(admin.as_ptr(), token.as_ptr()), 0);
        let stored = test_mock::get_storage(MOLT_TOKEN_KEY);
        assert_eq!(stored, Some(token.to_vec()));

        // Zero address rejected
        let zero = [0u8; 32];
        assert_eq!(set_molt_token(admin.as_ptr(), zero.as_ptr()), 2);

        // Non-admin rejected
        let other = [9u8; 32];
        test_mock::set_caller(other);
        assert_eq!(set_molt_token(other.as_ptr(), token.as_ptr()), 1);
    }

    #[test]
    fn test_g24_withdraw_fees_triggers_transfer() {
        // withdraw_fees() calls transfer_molt_out to send fees to admin
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        // CON-05: Configure MOLT token so transfer_molt_out succeeds
        let molt = [42u8; 32];
        set_molt_token(admin.as_ptr(), molt.as_ptr());
        test_mock::set_cross_call_response(Some(vec![1u8]));
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
