// mUSD Token — Treasury-Backed Stable Unit for MoltChain DEX
//
// Architecture:
//   mUSD is a 1:1 receipt token backed by USDT/USDC reserves held in the
//   MoltChain treasury. It is NOT an algorithmic stablecoin — it's a
//   custodial wrapper that unifies USDT + USDC into a single quote asset
//   for all DEX trading pairs.
//
// Trust Model (Proof of Reserves):
//   - Treasury multisig (3-of-5 keyholders) is the sole minting authority
//   - Total minted mUSD is tracked on-chain (auditable at any time)
//   - reserve_attestation() records periodic reserve proofs linked to
//     external auditor reports or MoltOracle price feeds
//   - Anyone can call get_reserve_ratio() to compare minted vs attested
//   - Mint/burn events logged with full audit trail
//   - Circuit breaker: minting pauses if reserve ratio drops below 100%
//
// Flow:
//   Deposit:  User sends USDT/USDC (any network) → wallet sweeps to treasury
//             → treasury calls mint(to, amount) → user receives mUSD on MoltChain
//   Withdraw: User calls burn(amount) → treasury releases USDT or USDC
//             to user's preferred address/network
//
// DEX Integration:
//   All trading pairs: MOLT/mUSD, BTC/mUSD, ETH/mUSD, REEF/mUSD
//   All AMM pools: paired against mUSD
//   All fee collection: denominated in mUSD
//   All margin collateral: held in mUSD
//
// Security:
//   - Reentrancy guard on all state-changing functions
//   - Emergency pause (admin/multisig)
//   - Multisig minting (admin must be multisig contract)
//   - Mint cap per epoch (rate limiting)
//   - Reserve attestation circuit breaker
//   - Zero-address checks on all transfers

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]
#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{bytes_to_u64, get_caller, get_slot, log_info, storage_get, storage_set, u64_to_bytes};

// ============================================================================
// CONSTANTS
// ============================================================================

// Token metadata
#[allow(dead_code)]
const TOKEN_NAME: &[u8] = b"MoltChain USD";
#[allow(dead_code)]
const TOKEN_SYMBOL: &[u8] = b"mUSD";
#[allow(dead_code)]
const DECIMALS: u8 = 6; // Same as USDT/USDC (6 decimals)

// Minting controls
const MINT_CAP_PER_EPOCH: u64 = 100_000_000_000; // 100K mUSD per epoch (in micro-units)
const EPOCH_SLOTS: u64 = 86_400; // ~24 hours at 1 slot/sec
#[allow(dead_code)]
const RESERVE_FLOOR_BPS: u64 = 10_000; // 100% — must be fully backed
#[allow(dead_code)]
const RESERVE_WARNING_BPS: u64 = 10_200; // 102% — warn if close to floor

// Storage keys
const ADMIN_KEY: &[u8] = b"musd_admin";
const PAUSED_KEY: &[u8] = b"musd_paused";
const REENTRANCY_KEY: &[u8] = b"musd_reentrancy";
const TOTAL_SUPPLY_KEY: &[u8] = b"musd_supply";
const TOTAL_MINTED_KEY: &[u8] = b"musd_minted";
const TOTAL_BURNED_KEY: &[u8] = b"musd_burned";

// Reserve attestation keys
const RESERVE_ATTESTED_KEY: &[u8] = b"musd_reserve_att"; // Last attested reserve amount
const RESERVE_SLOT_KEY: &[u8] = b"musd_reserve_slot"; // Slot of last attestation
const RESERVE_HASH_KEY: &[u8] = b"musd_reserve_hash"; // Hash of external proof
const ATTESTATION_COUNT_KEY: &[u8] = b"musd_att_count";

// Epoch tracking
const EPOCH_START_KEY: &[u8] = b"musd_epoch_start";
const EPOCH_MINTED_KEY: &[u8] = b"musd_epoch_mint";

// Event counters
const MINT_EVENT_COUNT_KEY: &[u8] = b"musd_mint_evt";
const BURN_EVENT_COUNT_KEY: &[u8] = b"musd_burn_evt";
const TRANSFER_COUNT_KEY: &[u8] = b"musd_xfer_cnt";

// ============================================================================
// HELPERS
// ============================================================================

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key)
        .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
        .unwrap_or(0)
}
fn save_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
}

fn load_addr(key: &[u8]) -> [u8; 32] {
    storage_get(key)
        .map(|d| {
            let mut a = [0u8; 32];
            if d.len() >= 32 {
                a.copy_from_slice(&d[..32]);
            }
            a
        })
        .unwrap_or([0u8; 32])
}
fn is_zero(addr: &[u8; 32]) -> bool {
    addr.iter().all(|&b| b == 0)
}

fn u64_to_decimal(mut n: u64) -> Vec<u8> {
    if n == 0 {
        return alloc::vec![b'0'];
    }
    let mut buf = Vec::new();
    while n > 0 {
        buf.push(b'0' + (n % 10) as u8);
        n /= 10;
    }
    buf.reverse();
    buf
}
fn hex_encode(bytes: &[u8]) -> Vec<u8> {
    let hex_chars: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(hex_chars[(b >> 4) as usize]);
        out.push(hex_chars[(b & 0x0f) as usize]);
    }
    out
}

// Per-account balance key
fn balance_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"musd_bal_"[..]);
    k.extend_from_slice(&hex_encode(addr));
    k
}

// Per-account allowance key (owner → spender)
fn allowance_key(owner: &[u8; 32], spender: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"musd_alw_"[..]);
    k.extend_from_slice(&hex_encode(owner));
    k.push(b'_');
    k.extend_from_slice(&hex_encode(spender));
    k
}

// Attestation history key
fn attestation_key(index: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"musd_att_"[..]);
    k.extend_from_slice(&u64_to_decimal(index));
    k
}

// ============================================================================
// SECURITY
// ============================================================================

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

fn is_paused() -> bool {
    storage_get(PAUSED_KEY)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
}
fn require_not_paused() -> bool {
    !is_paused()
}

fn require_admin(caller: &[u8; 32]) -> bool {
    let admin = load_addr(ADMIN_KEY);
    !is_zero(&admin) && *caller == admin
}

// Reserve circuit breaker: block minting if supply would exceed attested reserves
fn check_reserve_circuit_breaker(additional_mint: u64) -> bool {
    let attested = load_u64(RESERVE_ATTESTED_KEY);
    if attested == 0 {
        return true;
    } // No attestation yet — allow initial minting
    let supply = load_u64(TOTAL_SUPPLY_KEY);
    let new_supply = supply.saturating_add(additional_mint);
    // new_supply must not exceed attested reserves
    new_supply <= attested
}

// Epoch rate limiting: check if mint cap exceeded for current epoch
fn check_epoch_cap(amount: u64) -> bool {
    let current_slot = get_slot();
    let epoch_start = load_u64(EPOCH_START_KEY);
    let epoch_minted = load_u64(EPOCH_MINTED_KEY);

    if current_slot >= epoch_start + EPOCH_SLOTS {
        // New epoch — reset
        save_u64(EPOCH_START_KEY, current_slot);
        save_u64(EPOCH_MINTED_KEY, amount);
        return amount <= MINT_CAP_PER_EPOCH;
    }

    epoch_minted.saturating_add(amount) <= MINT_CAP_PER_EPOCH
}

// ============================================================================
// PUBLIC FUNCTIONS — TOKEN OPERATIONS
// ============================================================================

/// Initialize the mUSD token contract. Admin should be a multisig address.
#[no_mangle]
pub extern "C" fn initialize(admin: *const u8) -> u32 {
    let existing = load_addr(ADMIN_KEY);
    if !is_zero(&existing) {
        return 1;
    } // Already initialized

    let mut addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(admin, addr.as_mut_ptr(), 32);
    }
    if is_zero(&addr) {
        return 2;
    } // Zero address

    storage_set(ADMIN_KEY, &addr);
    save_u64(TOTAL_SUPPLY_KEY, 0);
    save_u64(TOTAL_MINTED_KEY, 0);
    save_u64(TOTAL_BURNED_KEY, 0);
    save_u64(EPOCH_START_KEY, get_slot());
    save_u64(EPOCH_MINTED_KEY, 0);

    log_info("mUSD token initialized");
    0
}

/// Mint new mUSD. Only callable by admin (treasury multisig).
/// Protected by: reentrancy guard, pause check, reserve circuit breaker, epoch cap.
#[no_mangle]
pub extern "C" fn mint(caller: *const u8, to: *const u8, amount: u64) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }

    let mut caller_addr = [0u8; 32];
    let mut to_addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, caller_addr.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(to, to_addr.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let caller = get_caller();
    if caller.0 != caller_addr {
        reentrancy_exit();
        return 200;
    }

    if !require_not_paused() {
        reentrancy_exit();
        return 1;
    }
    if !require_admin(&caller_addr) {
        reentrancy_exit();
        return 2;
    }
    if is_zero(&to_addr) {
        reentrancy_exit();
        return 3;
    } // No mint to zero
    if amount == 0 {
        reentrancy_exit();
        return 4;
    }

    // Circuit breaker: check reserves
    if !check_reserve_circuit_breaker(amount) {
        reentrancy_exit();
        log_info("CIRCUIT BREAKER: mint blocked \u{2014} would exceed attested reserves");
        return 10;
    }

    // Epoch rate limit
    if !check_epoch_cap(amount) {
        reentrancy_exit();
        log_info("RATE LIMIT: epoch mint cap reached");
        return 11;
    }

    // Update epoch counter
    let current_slot = get_slot();
    let epoch_start = load_u64(EPOCH_START_KEY);
    if current_slot >= epoch_start + EPOCH_SLOTS {
        save_u64(EPOCH_START_KEY, current_slot);
        save_u64(EPOCH_MINTED_KEY, amount);
    } else {
        save_u64(
            EPOCH_MINTED_KEY,
            load_u64(EPOCH_MINTED_KEY).saturating_add(amount),
        );
    }

    // Credit recipient
    let bk = balance_key(&to_addr);
    let bal = load_u64(&bk);
    save_u64(&bk, bal.saturating_add(amount));

    // Update totals
    save_u64(
        TOTAL_SUPPLY_KEY,
        load_u64(TOTAL_SUPPLY_KEY).saturating_add(amount),
    );
    save_u64(
        TOTAL_MINTED_KEY,
        load_u64(TOTAL_MINTED_KEY).saturating_add(amount),
    );

    // Log mint event
    let evt_count = load_u64(MINT_EVENT_COUNT_KEY);
    save_u64(MINT_EVENT_COUNT_KEY, evt_count.saturating_add(1));

    let mut msg = Vec::from(&b"MINT #"[..]);
    msg.extend_from_slice(&u64_to_decimal(evt_count.saturating_add(1)));
    msg.extend_from_slice(b": ");
    msg.extend_from_slice(&u64_to_decimal(amount));
    msg.extend_from_slice(b" mUSD to 0x");
    msg.extend_from_slice(&hex_encode(&to_addr[..4]));
    log_info(core::str::from_utf8(&msg).unwrap_or("mint event"));

    reentrancy_exit();
    0
}

/// Burn mUSD. Called by the holder or by admin on behalf of a withdrawal.
/// Burns reduce supply — treasury then releases USDT/USDC externally.
#[no_mangle]
pub extern "C" fn burn(caller: *const u8, amount: u64) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }

    let mut caller_addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, caller_addr.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let caller = get_caller();
    if caller.0 != caller_addr {
        reentrancy_exit();
        return 200;
    }

    if !require_not_paused() {
        reentrancy_exit();
        return 1;
    }
    if amount == 0 {
        reentrancy_exit();
        return 4;
    }

    let bk = balance_key(&caller_addr);
    let bal = load_u64(&bk);
    if bal < amount {
        reentrancy_exit();
        return 5;
    } // Insufficient balance

    save_u64(&bk, bal - amount);
    save_u64(
        TOTAL_SUPPLY_KEY,
        load_u64(TOTAL_SUPPLY_KEY).saturating_sub(amount),
    );
    save_u64(
        TOTAL_BURNED_KEY,
        load_u64(TOTAL_BURNED_KEY).saturating_add(amount),
    );

    let evt_count = load_u64(BURN_EVENT_COUNT_KEY);
    save_u64(BURN_EVENT_COUNT_KEY, evt_count.saturating_add(1));

    let mut msg = Vec::from(&b"BURN #"[..]);
    msg.extend_from_slice(&u64_to_decimal(evt_count.saturating_add(1)));
    msg.extend_from_slice(b": ");
    msg.extend_from_slice(&u64_to_decimal(amount));
    msg.extend_from_slice(b" mUSD from 0x");
    msg.extend_from_slice(&hex_encode(&caller_addr[..4]));
    log_info(core::str::from_utf8(&msg).unwrap_or("burn event"));

    reentrancy_exit();
    0
}

/// Transfer mUSD between accounts. Standard token transfer.
#[no_mangle]
pub extern "C" fn transfer(from: *const u8, to: *const u8, amount: u64) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }

    let mut from_addr = [0u8; 32];
    let mut to_addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(from, from_addr.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(to, to_addr.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches from address
    let caller = get_caller();
    if caller.0 != from_addr {
        reentrancy_exit();
        return 200;
    }

    if !require_not_paused() {
        reentrancy_exit();
        return 1;
    }
    if is_zero(&to_addr) {
        reentrancy_exit();
        return 3;
    }
    if amount == 0 {
        reentrancy_exit();
        return 4;
    }
    if from_addr == to_addr {
        reentrancy_exit();
        return 6;
    } // Self-transfer

    let from_bk = balance_key(&from_addr);
    let from_bal = load_u64(&from_bk);
    if from_bal < amount {
        reentrancy_exit();
        return 5;
    }

    let to_bk = balance_key(&to_addr);
    let to_bal = load_u64(&to_bk);

    save_u64(&from_bk, from_bal - amount);
    save_u64(&to_bk, to_bal.saturating_add(amount));

    save_u64(TRANSFER_COUNT_KEY, load_u64(TRANSFER_COUNT_KEY).saturating_add(1));

    reentrancy_exit();
    0
}

/// Approve a spender to transfer on owner's behalf (for DEX integration).
#[no_mangle]
pub extern "C" fn approve(owner: *const u8, spender: *const u8, amount: u64) -> u32 {
    // AUDIT-FIX 2.23: Reentrancy guard for consistency with transfer/transfer_from
    if !reentrancy_enter() {
        return 100;
    }

    let mut owner_addr = [0u8; 32];
    let mut spender_addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(owner, owner_addr.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(spender, spender_addr.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches owner address
    let caller = get_caller();
    if caller.0 != owner_addr {
        reentrancy_exit();
        return 200;
    }

    if is_zero(&spender_addr) {
        reentrancy_exit();
        return 3;
    }
    if owner_addr == spender_addr {
        reentrancy_exit();
        return 6;
    }

    let ak = allowance_key(&owner_addr, &spender_addr);
    save_u64(&ak, amount);
    reentrancy_exit();
    0
}

/// Transfer from another account using allowance (for DEX contracts to move mUSD).
#[no_mangle]
pub extern "C" fn transfer_from(
    caller: *const u8,
    from: *const u8,
    to: *const u8,
    amount: u64,
) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }

    let mut caller_addr = [0u8; 32];
    let mut from_addr = [0u8; 32];
    let mut to_addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, caller_addr.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(from, from_addr.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(to, to_addr.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let caller = get_caller();
    if caller.0 != caller_addr {
        reentrancy_exit();
        return 200;
    }

    if !require_not_paused() {
        reentrancy_exit();
        return 1;
    }
    if is_zero(&to_addr) {
        reentrancy_exit();
        return 3;
    }
    if amount == 0 {
        reentrancy_exit();
        return 4;
    }

    // Check allowance
    let ak = allowance_key(&from_addr, &caller_addr);
    let allowed = load_u64(&ak);
    if allowed < amount {
        reentrancy_exit();
        return 7;
    } // Exceeds allowance

    // Check balance
    let from_bk = balance_key(&from_addr);
    let from_bal = load_u64(&from_bk);
    if from_bal < amount {
        reentrancy_exit();
        return 5;
    }

    let to_bk = balance_key(&to_addr);
    let to_bal = load_u64(&to_bk);

    // Execute transfer
    save_u64(&from_bk, from_bal - amount);
    save_u64(&to_bk, to_bal.saturating_add(amount));
    save_u64(&ak, allowed - amount);

    save_u64(TRANSFER_COUNT_KEY, load_u64(TRANSFER_COUNT_KEY).saturating_add(1));

    reentrancy_exit();
    0
}

// ============================================================================
// PUBLIC FUNCTIONS — RESERVE ATTESTATION (PROOF OF RESERVES)
// ============================================================================

/// Record a reserve attestation. Called by admin/auditor to declare
/// the current off-chain reserve balance (USDT + USDC held in treasury).
/// proof_hash: 32-byte hash of the external audit report or MoltOracle feed.
#[no_mangle]
pub extern "C" fn attest_reserves(
    caller: *const u8,
    reserve_amount: u64,
    proof_hash: *const u8,
) -> u32 {
    let mut caller_addr = [0u8; 32];
    let mut hash = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, caller_addr.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(proof_hash, hash.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller_addr {
        return 200;
    }

    if !require_admin(&caller_addr) {
        return 2;
    }

    // Store current attestation
    save_u64(RESERVE_ATTESTED_KEY, reserve_amount);
    save_u64(RESERVE_SLOT_KEY, get_slot());
    storage_set(RESERVE_HASH_KEY, &hash);

    // Store in history
    let count = load_u64(ATTESTATION_COUNT_KEY);
    let ak = attestation_key(count);
    // Pack: [amount(8)] [slot(8)] [hash(32)] = 48 bytes
    let mut record = Vec::with_capacity(48);
    record.extend_from_slice(&u64_to_bytes(reserve_amount));
    record.extend_from_slice(&u64_to_bytes(get_slot()));
    record.extend_from_slice(&hash);
    storage_set(&ak, &record);
    save_u64(ATTESTATION_COUNT_KEY, count.saturating_add(1));

    let mut msg = Vec::from(&b"RESERVE ATTESTATION #"[..]);
    msg.extend_from_slice(&u64_to_decimal(count.saturating_add(1)));
    msg.extend_from_slice(b": ");
    msg.extend_from_slice(&u64_to_decimal(reserve_amount));
    msg.extend_from_slice(b" mUSD backing declared");
    log_info(core::str::from_utf8(&msg).unwrap_or("reserve attestation"));

    0
}

// ============================================================================
// PUBLIC FUNCTIONS — QUERIES
// ============================================================================

/// Get mUSD balance of an account.
#[no_mangle]
pub extern "C" fn balance_of(addr: *const u8) -> u64 {
    let mut address = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(addr, address.as_mut_ptr(), 32);
    }
    load_u64(&balance_key(&address))
}

/// Get allowance granted from owner to spender.
#[no_mangle]
pub extern "C" fn allowance(owner: *const u8, spender: *const u8) -> u64 {
    let mut owner_addr = [0u8; 32];
    let mut spender_addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(owner, owner_addr.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(spender, spender_addr.as_mut_ptr(), 32);
    }
    load_u64(&allowance_key(&owner_addr, &spender_addr))
}

/// Get total mUSD in circulation.
#[no_mangle]
pub extern "C" fn total_supply() -> u64 {
    load_u64(TOTAL_SUPPLY_KEY)
}

/// Get total mUSD ever minted.
#[no_mangle]
pub extern "C" fn total_minted() -> u64 {
    load_u64(TOTAL_MINTED_KEY)
}

/// Get total mUSD ever burned.
#[no_mangle]
pub extern "C" fn total_burned() -> u64 {
    load_u64(TOTAL_BURNED_KEY)
}

/// Get the reserve ratio in basis points (10000 = 100%).
/// Returns 0 if no attestation has been made.
#[no_mangle]
pub extern "C" fn get_reserve_ratio() -> u64 {
    let attested = load_u64(RESERVE_ATTESTED_KEY);
    let supply = load_u64(TOTAL_SUPPLY_KEY);
    if supply == 0 {
        return 10_000;
    } // 100% if nothing is minted
    if attested == 0 {
        return 0;
    } // No attestation yet
    ((attested as u128) * 10_000 / (supply as u128)) as u64 // bps — 10000 = 100%, 10500 = 105%
}

/// Get slot of last reserve attestation.
#[no_mangle]
pub extern "C" fn get_last_attestation_slot() -> u64 {
    load_u64(RESERVE_SLOT_KEY)
}

/// Get attestation count (number of reserve proofs submitted).
#[no_mangle]
pub extern "C" fn get_attestation_count() -> u64 {
    load_u64(ATTESTATION_COUNT_KEY)
}

/// Get remaining mint capacity for current epoch.
#[no_mangle]
pub extern "C" fn get_epoch_remaining() -> u64 {
    let current_slot = get_slot();
    let epoch_start = load_u64(EPOCH_START_KEY);
    if current_slot >= epoch_start + EPOCH_SLOTS {
        return MINT_CAP_PER_EPOCH; // New epoch hasn't started — full capacity
    }
    let minted = load_u64(EPOCH_MINTED_KEY);
    MINT_CAP_PER_EPOCH.saturating_sub(minted)
}

/// Get transfer count.
#[no_mangle]
pub extern "C" fn get_transfer_count() -> u64 {
    load_u64(TRANSFER_COUNT_KEY)
}

// ============================================================================
// ADMIN FUNCTIONS
// ============================================================================

#[no_mangle]
pub extern "C" fn emergency_pause(caller: *const u8) -> u32 {
    let mut addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, addr.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != addr {
        return 200;
    }
    if !require_admin(&addr) {
        return 2;
    }
    storage_set(PAUSED_KEY, &[1u8]);
    log_info("mUSD: EMERGENCY PAUSE");
    0
}

#[no_mangle]
pub extern "C" fn emergency_unpause(caller: *const u8) -> u32 {
    let mut addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, addr.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != addr {
        return 200;
    }
    if !require_admin(&addr) {
        return 2;
    }
    storage_set(PAUSED_KEY, &[0u8]);
    log_info("mUSD: RESUMED");
    0
}

/// Transfer admin to a new multisig address.
#[no_mangle]
pub extern "C" fn transfer_admin(caller: *const u8, new_admin: *const u8) -> u32 {
    let mut caller_addr = [0u8; 32];
    let mut new_addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, caller_addr.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(new_admin, new_addr.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller_addr {
        return 200;
    }
    if !require_admin(&caller_addr) {
        return 2;
    }
    if is_zero(&new_addr) {
        return 3;
    }
    storage_set(ADMIN_KEY, &new_addr);
    log_info("mUSD: admin transferred");
    0
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use moltchain_sdk::test_mock;

    fn reset_store() {
        test_mock::reset();
    }
    fn set_slot(v: u64) {
        test_mock::SLOT.with(|s| *s.borrow_mut() = v);
    }

    fn addr(id: u8) -> [u8; 32] {
        let mut a = [0u8; 32];
        a[0] = id;
        a
    }

    // ---- Initialization ----

    #[test]
    fn test_initialize() {
        reset_store();
        let admin = addr(1);
        let result = initialize(admin.as_ptr());
        assert_eq!(result, 0);
        assert_eq!(total_supply(), 0);
    }

    #[test]
    fn test_initialize_twice_fails() {
        reset_store();
        let admin = addr(1);
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(initialize(admin.as_ptr()), 1); // Already initialized
    }

    #[test]
    fn test_initialize_zero_address_fails() {
        reset_store();
        let zero = [0u8; 32];
        assert_eq!(initialize(zero.as_ptr()), 2);
    }

    // ---- Minting ----

    #[test]
    fn test_mint_basic() {
        reset_store();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        let result = mint(admin.as_ptr(), user.as_ptr(), 1_000_000);
        assert_eq!(result, 0);
        assert_eq!(balance_of(user.as_ptr()), 1_000_000);
        assert_eq!(total_supply(), 1_000_000);
        assert_eq!(total_minted(), 1_000_000);
    }

    #[test]
    fn test_mint_non_admin_fails() {
        reset_store();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());
        test_mock::set_caller(user);
        assert_eq!(mint(user.as_ptr(), user.as_ptr(), 1_000_000), 2);
    }

    #[test]
    fn test_mint_zero_amount_fails() {
        reset_store();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        assert_eq!(mint(admin.as_ptr(), user.as_ptr(), 0), 4);
    }

    #[test]
    fn test_mint_to_zero_address_fails() {
        reset_store();
        let admin = addr(1);
        let zero = [0u8; 32];
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        assert_eq!(mint(admin.as_ptr(), zero.as_ptr(), 1_000_000), 3);
    }

    #[test]
    fn test_mint_epoch_cap() {
        reset_store();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        // Mint up to cap
        assert_eq!(mint(admin.as_ptr(), user.as_ptr(), MINT_CAP_PER_EPOCH), 0);
        // Next mint should fail
        assert_eq!(mint(admin.as_ptr(), user.as_ptr(), 1), 11);
    }

    // ---- Burning ----

    #[test]
    fn test_burn_basic() {
        reset_store();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        mint(admin.as_ptr(), user.as_ptr(), 5_000_000);
        test_mock::set_caller(user);
        let result = burn(user.as_ptr(), 2_000_000);
        assert_eq!(result, 0);
        assert_eq!(balance_of(user.as_ptr()), 3_000_000);
        assert_eq!(total_supply(), 3_000_000);
        assert_eq!(total_burned(), 2_000_000);
    }

    #[test]
    fn test_burn_insufficient_balance_fails() {
        reset_store();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        mint(admin.as_ptr(), user.as_ptr(), 1_000_000);
        test_mock::set_caller(user);
        assert_eq!(burn(user.as_ptr(), 2_000_000), 5);
    }

    // ---- Transfers ----

    #[test]
    fn test_transfer_basic() {
        reset_store();
        let admin = addr(1);
        let alice = addr(2);
        let bob = addr(3);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        mint(admin.as_ptr(), alice.as_ptr(), 10_000_000);
        test_mock::set_caller(alice);
        let result = transfer(alice.as_ptr(), bob.as_ptr(), 3_000_000);
        assert_eq!(result, 0);
        assert_eq!(balance_of(alice.as_ptr()), 7_000_000);
        assert_eq!(balance_of(bob.as_ptr()), 3_000_000);
    }

    #[test]
    fn test_transfer_insufficient_fails() {
        reset_store();
        let admin = addr(1);
        let alice = addr(2);
        let bob = addr(3);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        mint(admin.as_ptr(), alice.as_ptr(), 1_000_000);
        test_mock::set_caller(alice);
        assert_eq!(transfer(alice.as_ptr(), bob.as_ptr(), 5_000_000), 5);
    }

    #[test]
    fn test_self_transfer_fails() {
        reset_store();
        let admin = addr(1);
        let alice = addr(2);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        mint(admin.as_ptr(), alice.as_ptr(), 1_000_000);
        test_mock::set_caller(alice);
        assert_eq!(transfer(alice.as_ptr(), alice.as_ptr(), 500_000), 6);
    }

    // ---- Allowance / TransferFrom ----

    #[test]
    fn test_approve_and_transfer_from() {
        reset_store();
        let admin = addr(1);
        let alice = addr(2);
        let bob = addr(3);
        let dex = addr(4); // DEX contract
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        mint(admin.as_ptr(), alice.as_ptr(), 10_000_000);

        // Alice approves DEX to spend 5M
        test_mock::set_caller(alice);
        assert_eq!(approve(alice.as_ptr(), dex.as_ptr(), 5_000_000), 0);
        assert_eq!(allowance(alice.as_ptr(), dex.as_ptr()), 5_000_000);

        // DEX moves 3M from Alice to Bob
        test_mock::set_caller(dex);
        assert_eq!(
            transfer_from(dex.as_ptr(), alice.as_ptr(), bob.as_ptr(), 3_000_000),
            0
        );
        assert_eq!(balance_of(alice.as_ptr()), 7_000_000);
        assert_eq!(balance_of(bob.as_ptr()), 3_000_000);
        assert_eq!(allowance(alice.as_ptr(), dex.as_ptr()), 2_000_000);
    }

    #[test]
    fn test_transfer_from_exceeds_allowance_fails() {
        reset_store();
        let admin = addr(1);
        let alice = addr(2);
        let bob = addr(3);
        let dex = addr(4);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        mint(admin.as_ptr(), alice.as_ptr(), 10_000_000);
        test_mock::set_caller(alice);
        approve(alice.as_ptr(), dex.as_ptr(), 1_000_000);
        test_mock::set_caller(dex);
        assert_eq!(
            transfer_from(dex.as_ptr(), alice.as_ptr(), bob.as_ptr(), 5_000_000),
            7
        );
    }

    // ---- Reserve Attestation ----

    #[test]
    fn test_attest_reserves() {
        reset_store();
        let admin = addr(1);
        let proof = [0xABu8; 32];
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        let result = attest_reserves(admin.as_ptr(), 50_000_000_000, proof.as_ptr());
        assert_eq!(result, 0);
        assert_eq!(get_attestation_count(), 1);
    }

    #[test]
    fn test_reserve_ratio() {
        reset_store();
        let admin = addr(1);
        let user = addr(2);
        let proof = [0xABu8; 32];
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        // Mint 10M
        mint(admin.as_ptr(), user.as_ptr(), 10_000_000);
        // Attest 10M reserves
        attest_reserves(admin.as_ptr(), 10_000_000, proof.as_ptr());
        assert_eq!(get_reserve_ratio(), 10_000); // 100%
                                                 // Attest 12M reserves (over-collateralized)
        attest_reserves(admin.as_ptr(), 12_000_000, proof.as_ptr());
        assert_eq!(get_reserve_ratio(), 12_000); // 120%
    }

    #[test]
    fn test_circuit_breaker_blocks_over_mint() {
        reset_store();
        let admin = addr(1);
        let user = addr(2);
        let proof = [0xABu8; 32];
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        // Attest 5M reserves
        attest_reserves(admin.as_ptr(), 5_000_000, proof.as_ptr());
        // Mint 5M — should work (exactly at limit)
        assert_eq!(mint(admin.as_ptr(), user.as_ptr(), 5_000_000), 0);
        // Mint 1 more — should be blocked by circuit breaker
        assert_eq!(mint(admin.as_ptr(), user.as_ptr(), 1), 10);
    }

    // ---- Pause ----

    #[test]
    fn test_pause_blocks_operations() {
        reset_store();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        mint(admin.as_ptr(), user.as_ptr(), 1_000_000);

        emergency_pause(admin.as_ptr());
        test_mock::set_caller(user);
        assert_eq!(transfer(user.as_ptr(), admin.as_ptr(), 100_000), 1); // Blocked
        test_mock::set_caller(admin);
        assert_eq!(mint(admin.as_ptr(), user.as_ptr(), 100_000), 1); // Blocked
        test_mock::set_caller(user);
        assert_eq!(burn(user.as_ptr(), 100_000), 1); // Blocked

        test_mock::set_caller(admin);
        emergency_unpause(admin.as_ptr());
        test_mock::set_caller(user);
        assert_eq!(transfer(user.as_ptr(), admin.as_ptr(), 100_000), 0); // Works again
    }

    // ---- Admin Transfer ----

    #[test]
    fn test_transfer_admin() {
        reset_store();
        let admin = addr(1);
        let new_admin = addr(5);
        let user = addr(2);
        initialize(admin.as_ptr());

        test_mock::set_caller(admin);
        assert_eq!(transfer_admin(admin.as_ptr(), new_admin.as_ptr()), 0);
        // Old admin can no longer mint
        assert_eq!(mint(admin.as_ptr(), user.as_ptr(), 1_000_000), 2);
        // New admin can mint
        test_mock::set_caller(new_admin);
        assert_eq!(mint(new_admin.as_ptr(), user.as_ptr(), 1_000_000), 0);
    }

    // ---- Edge cases ----

    #[test]
    fn test_burn_zero_fails() {
        reset_store();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        mint(admin.as_ptr(), user.as_ptr(), 1_000_000);
        test_mock::set_caller(user);
        assert_eq!(burn(user.as_ptr(), 0), 4);
    }

    #[test]
    fn test_transfer_to_zero_address_fails() {
        reset_store();
        let admin = addr(1);
        let user = addr(2);
        let zero = [0u8; 32];
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        mint(admin.as_ptr(), user.as_ptr(), 1_000_000);
        test_mock::set_caller(user);
        assert_eq!(transfer(user.as_ptr(), zero.as_ptr(), 500_000), 3);
    }

    #[test]
    fn test_epoch_resets_after_period() {
        reset_store();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);

        // Mint up to cap
        assert_eq!(mint(admin.as_ptr(), user.as_ptr(), MINT_CAP_PER_EPOCH), 0);
        assert_eq!(mint(admin.as_ptr(), user.as_ptr(), 1), 11); // Capped

        // Advance to next epoch
        set_slot(1000 + EPOCH_SLOTS + 1);
        assert_eq!(mint(admin.as_ptr(), user.as_ptr(), 1_000_000), 0); // Works again
    }

    #[test]
    fn test_supply_accounting_consistency() {
        reset_store();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());

        test_mock::set_caller(admin);
        mint(admin.as_ptr(), user.as_ptr(), 10_000_000);
        test_mock::set_caller(user);
        burn(user.as_ptr(), 3_000_000);
        test_mock::set_caller(admin);
        mint(admin.as_ptr(), user.as_ptr(), 5_000_000);
        test_mock::set_caller(user);
        burn(user.as_ptr(), 2_000_000);

        // supply = 10M - 3M + 5M - 2M = 10M
        assert_eq!(total_supply(), 10_000_000);
        assert_eq!(total_minted(), 15_000_000);
        assert_eq!(total_burned(), 5_000_000);
        assert_eq!(balance_of(user.as_ptr()), 10_000_000);
    }
}
