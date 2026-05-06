// wSOL Token — Wrapped SOL on Lichen
//
// Architecture:
//   wSOL is a 1:1 receipt token backed by native SOL reserves held in the
//   Lichen treasury (Solana wallet). Users deposit SOL on Solana,
//   custody service sweeps to treasury, then mints wSOL on Lichen.
//
// Identical security model to lusd_token:
//   - Treasury multisig (3-of-5) is the sole minting authority
//   - Reserve attestation with proof hashes
//   - Circuit breaker: no minting beyond attested reserves
//   - Epoch rate limiting, reentrancy guard, emergency pause
//
// DEX Integration:
//   wSOL/lUSD — SOL priced in USD
//   wSOL/LICN — SOL priced in LICN (direct, no stablecoin needed)

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]
#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use lichen_sdk::{
    bytes_to_u64, can_receive, can_send, can_transfer, get_caller, get_contract_address, get_slot,
    log_info, storage_get, storage_set, u64_to_bytes, Address,
};

// ============================================================================
// CONSTANTS
// ============================================================================

#[allow(dead_code)]
const TOKEN_NAME: &[u8] = b"Wrapped SOL";
#[allow(dead_code)]
const TOKEN_SYMBOL: &[u8] = b"wSOL";
#[allow(dead_code)]
const DECIMALS: u8 = 9; // Same as native SOL (9 decimals / lamports)

// Minting controls
const MINT_CAP_PER_EPOCH: u64 = 500_000_000_000_000; // 500K wSOL per epoch — circuit breaker, not growth limiter
const EPOCH_SLOTS: u64 = 86_400;
const MAX_ATTESTATION_AGE_SLOTS: u64 = EPOCH_SLOTS;
#[allow(dead_code)]
const RESERVE_FLOOR_BPS: u64 = 10_000;
#[allow(dead_code)]
const RESERVE_WARNING_BPS: u64 = 10_200;
const ERR_CIRCUIT_BREAKER: u32 = 10;
const ERR_EPOCH_CAP: u32 = 11;
const ERR_ARITHMETIC_OVERFLOW: u32 = 12;
const ERR_COMPLIANCE_RESTRICTED: u32 = 13;

// Storage keys — prefixed "wsol_" to avoid collision with musd/weth
const ADMIN_KEY: &[u8] = b"wsol_admin";
const PENDING_ADMIN_KEY: &[u8] = b"wsol_pending_admin";
const ATTESTER_KEY: &[u8] = b"wsol_attester";
const MINTER_KEY: &[u8] = b"wsol_minter";
const BOOTSTRAP_COMPLETE_KEY: &[u8] = b"wsol_bootstrap_complete";
const PAUSED_KEY: &[u8] = b"wsol_paused";
const REENTRANCY_KEY: &[u8] = b"wsol_reentrancy";
const TOTAL_SUPPLY_KEY: &[u8] = b"wsol_supply";
const TOTAL_MINTED_KEY: &[u8] = b"wsol_minted";
const TOTAL_BURNED_KEY: &[u8] = b"wsol_burned";

const RESERVE_ATTESTED_KEY: &[u8] = b"wsol_reserve_att";
const RESERVE_SLOT_KEY: &[u8] = b"wsol_reserve_slot";
const RESERVE_HASH_KEY: &[u8] = b"wsol_reserve_hash";
const ATTESTATION_COUNT_KEY: &[u8] = b"wsol_att_count";

const EPOCH_START_KEY: &[u8] = b"wsol_epoch_start";
const EPOCH_MINTED_KEY: &[u8] = b"wsol_epoch_mint";

const MINT_EVENT_COUNT_KEY: &[u8] = b"wsol_mint_evt";
const BURN_EVENT_COUNT_KEY: &[u8] = b"wsol_burn_evt";
const TRANSFER_COUNT_KEY: &[u8] = b"wsol_xfer_cnt";

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

fn checked_add_u64(lhs: u64, rhs: u64) -> Result<u64, u32> {
    lhs.checked_add(rhs).ok_or(ERR_ARITHMETIC_OVERFLOW)
}

fn checked_sub_u64(lhs: u64, rhs: u64) -> Result<u64, u32> {
    lhs.checked_sub(rhs).ok_or(ERR_ARITHMETIC_OVERFLOW)
}

fn address_from_bytes(bytes: &[u8; 32]) -> Address {
    Address::new(*bytes)
}

fn compliance_can_receive(to: &[u8; 32], amount: u64, balance: u64) -> bool {
    can_receive(
        get_contract_address(),
        address_from_bytes(to),
        amount,
        balance,
    )
}

fn compliance_can_send(from: &[u8; 32], amount: u64, balance: u64) -> bool {
    can_send(
        get_contract_address(),
        address_from_bytes(from),
        amount,
        balance,
    )
}

fn compliance_can_transfer(
    from: &[u8; 32],
    to: &[u8; 32],
    amount: u64,
    from_balance: u64,
    to_balance: u64,
) -> bool {
    can_transfer(
        get_contract_address(),
        address_from_bytes(from),
        address_from_bytes(to),
        amount,
        from_balance,
        to_balance,
    )
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

fn balance_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"wsol_bal_"[..]);
    k.extend_from_slice(&hex_encode(addr));
    k
}

fn allowance_key(owner: &[u8; 32], spender: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"wsol_alw_"[..]);
    k.extend_from_slice(&hex_encode(owner));
    k.push(b'_');
    k.extend_from_slice(&hex_encode(spender));
    k
}

fn attestation_key(index: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"wsol_att_"[..]);
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

fn require_minter(caller: &[u8; 32]) -> bool {
    let minter = load_addr(MINTER_KEY);
    !is_zero(&minter) && *caller == minter
}

fn require_attester(caller: &[u8; 32]) -> bool {
    let attester = load_addr(ATTESTER_KEY);
    !is_zero(&attester) && *caller == attester
}

fn is_bootstrap_complete() -> bool {
    storage_get(BOOTSTRAP_COMPLETE_KEY)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
}

fn check_reserve_circuit_breaker(additional_mint: u64) -> Result<(), u32> {
    let attested = load_u64(RESERVE_ATTESTED_KEY);
    if attested == 0 {
        return if is_bootstrap_complete() {
            Err(ERR_CIRCUIT_BREAKER)
        } else {
            Ok(())
        };
    }
    if is_bootstrap_complete() {
        let last_attestation_slot = load_u64(RESERVE_SLOT_KEY);
        let current_slot = get_slot();
        if last_attestation_slot == 0
            || current_slot > last_attestation_slot.saturating_add(MAX_ATTESTATION_AGE_SLOTS)
        {
            return Err(ERR_CIRCUIT_BREAKER);
        }
    }
    let supply = load_u64(TOTAL_SUPPLY_KEY);
    let new_supply = checked_add_u64(supply, additional_mint)?;
    if new_supply > attested {
        return Err(ERR_CIRCUIT_BREAKER);
    }

    Ok(())
}

fn next_epoch_state(amount: u64) -> Result<(u64, u64), u32> {
    let current_slot = get_slot();
    let epoch_start = load_u64(EPOCH_START_KEY);
    let epoch_minted = load_u64(EPOCH_MINTED_KEY);

    if current_slot >= epoch_start.saturating_add(EPOCH_SLOTS) {
        if amount > MINT_CAP_PER_EPOCH {
            return Err(ERR_EPOCH_CAP);
        }
        return Ok((current_slot, amount));
    }

    let next_epoch_minted = checked_add_u64(epoch_minted, amount)?;
    if next_epoch_minted > MINT_CAP_PER_EPOCH {
        return Err(ERR_EPOCH_CAP);
    }

    Ok((epoch_start, next_epoch_minted))
}

// ============================================================================
// PUBLIC FUNCTIONS — TOKEN OPERATIONS
// ============================================================================

fn init_signer_matches(admin: &[u8; 32]) -> bool {
    let caller = lichen_sdk::get_caller();
    if caller.0 == *admin {
        return true;
    }

    #[cfg(test)]
    {
        return caller.0 == [0u8; 32];
    }

    #[cfg(not(test))]
    {
        false
    }
}

#[no_mangle]
pub extern "C" fn initialize(admin: *const u8) -> u32 {
    let existing = load_addr(ADMIN_KEY);
    if !is_zero(&existing) {
        return 1;
    }

    let mut addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(admin, addr.as_mut_ptr(), 32);
    }
    if is_zero(&addr) {
        return 2;
    }
    if !init_signer_matches(&addr) {
        return 200;
    }

    storage_set(ADMIN_KEY, &addr);
    storage_set(ATTESTER_KEY, &addr);
    storage_set(MINTER_KEY, &addr);
    storage_set(BOOTSTRAP_COMPLETE_KEY, &[0u8]);
    save_u64(TOTAL_SUPPLY_KEY, 0);
    save_u64(TOTAL_MINTED_KEY, 0);
    save_u64(TOTAL_BURNED_KEY, 0);
    save_u64(EPOCH_START_KEY, get_slot());
    save_u64(EPOCH_MINTED_KEY, 0);

    log_info("wSOL token initialized");
    0
}

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
    if !require_minter(&caller_addr) {
        reentrancy_exit();
        return 2;
    }
    if is_zero(&to_addr) {
        reentrancy_exit();
        return 3;
    }
    if amount == 0 {
        reentrancy_exit();
        return 4;
    }

    if let Err(code) = check_reserve_circuit_breaker(amount) {
        reentrancy_exit();
        if code == ERR_CIRCUIT_BREAKER {
            log_info("CIRCUIT BREAKER: wSOL mint blocked - exceeds attested reserves");
        }
        return code;
    }

    let (next_epoch_start, next_epoch_minted) = match next_epoch_state(amount) {
        Ok(values) => values,
        Err(code) => {
            reentrancy_exit();
            if code == ERR_EPOCH_CAP {
                log_info("RATE LIMIT: wSOL epoch mint cap reached");
            }
            return code;
        }
    };

    let bk = balance_key(&to_addr);
    let bal = load_u64(&bk);
    if !compliance_can_receive(&to_addr, amount, bal) {
        reentrancy_exit();
        return ERR_COMPLIANCE_RESTRICTED;
    }
    let next_balance = match checked_add_u64(bal, amount) {
        Ok(value) => value,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };
    let next_total_supply = match checked_add_u64(load_u64(TOTAL_SUPPLY_KEY), amount) {
        Ok(value) => value,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };
    let next_total_minted = match checked_add_u64(load_u64(TOTAL_MINTED_KEY), amount) {
        Ok(value) => value,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };
    let next_evt_count = match checked_add_u64(load_u64(MINT_EVENT_COUNT_KEY), 1) {
        Ok(value) => value,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };

    save_u64(EPOCH_START_KEY, next_epoch_start);
    save_u64(EPOCH_MINTED_KEY, next_epoch_minted);
    save_u64(&bk, next_balance);
    save_u64(TOTAL_SUPPLY_KEY, next_total_supply);
    save_u64(TOTAL_MINTED_KEY, next_total_minted);
    save_u64(MINT_EVENT_COUNT_KEY, next_evt_count);

    let mut msg = Vec::from(&b"MINT wSOL #"[..]);
    msg.extend_from_slice(&u64_to_decimal(next_evt_count));
    msg.extend_from_slice(b": ");
    msg.extend_from_slice(&u64_to_decimal(amount));
    msg.extend_from_slice(b" lamports to 0x");
    msg.extend_from_slice(&hex_encode(&to_addr[..4]));
    log_info(core::str::from_utf8(&msg).unwrap_or("event"));

    reentrancy_exit();
    0
}

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
    if amount == 0 {
        reentrancy_exit();
        return 4;
    }

    let bk = balance_key(&caller_addr);
    let bal = load_u64(&bk);
    if bal < amount {
        reentrancy_exit();
        return 5;
    }
    if !compliance_can_send(&caller_addr, amount, bal) {
        reentrancy_exit();
        return ERR_COMPLIANCE_RESTRICTED;
    }

    let next_balance = match checked_sub_u64(bal, amount) {
        Ok(value) => value,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };
    let next_total_supply = match checked_sub_u64(load_u64(TOTAL_SUPPLY_KEY), amount) {
        Ok(value) => value,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };
    let next_total_burned = match checked_add_u64(load_u64(TOTAL_BURNED_KEY), amount) {
        Ok(value) => value,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };
    let next_evt_count = match checked_add_u64(load_u64(BURN_EVENT_COUNT_KEY), 1) {
        Ok(value) => value,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };

    save_u64(&bk, next_balance);
    save_u64(TOTAL_SUPPLY_KEY, next_total_supply);
    save_u64(TOTAL_BURNED_KEY, next_total_burned);
    save_u64(BURN_EVENT_COUNT_KEY, next_evt_count);

    let mut msg = Vec::from(&b"BURN wSOL #"[..]);
    msg.extend_from_slice(&u64_to_decimal(next_evt_count));
    msg.extend_from_slice(b": ");
    msg.extend_from_slice(&u64_to_decimal(amount));
    msg.extend_from_slice(b" lamports from 0x");
    msg.extend_from_slice(&hex_encode(&caller_addr[..4]));
    log_info(core::str::from_utf8(&msg).unwrap_or("event"));

    reentrancy_exit();
    0
}

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
    }

    let from_bk = balance_key(&from_addr);
    let from_bal = load_u64(&from_bk);
    if from_bal < amount {
        reentrancy_exit();
        return 5;
    }

    let to_bk = balance_key(&to_addr);
    let to_bal = load_u64(&to_bk);
    if !compliance_can_transfer(&from_addr, &to_addr, amount, from_bal, to_bal) {
        reentrancy_exit();
        return ERR_COMPLIANCE_RESTRICTED;
    }

    let next_from_balance = match checked_sub_u64(from_bal, amount) {
        Ok(value) => value,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };
    let next_to_balance = match checked_add_u64(to_bal, amount) {
        Ok(value) => value,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };
    let next_transfer_count = match checked_add_u64(load_u64(TRANSFER_COUNT_KEY), 1) {
        Ok(value) => value,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };

    save_u64(&from_bk, next_from_balance);
    save_u64(&to_bk, next_to_balance);
    save_u64(TRANSFER_COUNT_KEY, next_transfer_count);

    reentrancy_exit();
    0
}

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

    let ak = allowance_key(&from_addr, &caller_addr);
    let allowed = load_u64(&ak);
    if allowed < amount {
        reentrancy_exit();
        return 7;
    }

    let from_bk = balance_key(&from_addr);
    let from_bal = load_u64(&from_bk);
    if from_bal < amount {
        reentrancy_exit();
        return 5;
    }

    let to_bk = balance_key(&to_addr);
    let to_bal = load_u64(&to_bk);
    if !compliance_can_transfer(&from_addr, &to_addr, amount, from_bal, to_bal) {
        reentrancy_exit();
        return ERR_COMPLIANCE_RESTRICTED;
    }

    let next_from_balance = match checked_sub_u64(from_bal, amount) {
        Ok(value) => value,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };
    let next_to_balance = match checked_add_u64(to_bal, amount) {
        Ok(value) => value,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };
    let next_allowance = match checked_sub_u64(allowed, amount) {
        Ok(value) => value,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };
    let next_transfer_count = match checked_add_u64(load_u64(TRANSFER_COUNT_KEY), 1) {
        Ok(value) => value,
        Err(code) => {
            reentrancy_exit();
            return code;
        }
    };

    save_u64(&from_bk, next_from_balance);
    save_u64(&to_bk, next_to_balance);
    save_u64(&ak, next_allowance);
    save_u64(TRANSFER_COUNT_KEY, next_transfer_count);

    reentrancy_exit();
    0
}

// ============================================================================
// PUBLIC FUNCTIONS — RESERVE ATTESTATION
// ============================================================================

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

    if !require_attester(&caller_addr) {
        return 2;
    }

    let count = load_u64(ATTESTATION_COUNT_KEY);
    let next_count = match checked_add_u64(count, 1) {
        Ok(value) => value,
        Err(code) => return code,
    };

    save_u64(RESERVE_ATTESTED_KEY, reserve_amount);
    save_u64(RESERVE_SLOT_KEY, get_slot());
    storage_set(RESERVE_HASH_KEY, &hash);

    let ak = attestation_key(count);
    let mut record = Vec::with_capacity(48);
    record.extend_from_slice(&u64_to_bytes(reserve_amount));
    record.extend_from_slice(&u64_to_bytes(get_slot()));
    record.extend_from_slice(&hash);
    storage_set(&ak, &record);
    save_u64(ATTESTATION_COUNT_KEY, next_count);

    let mut msg = Vec::from(&b"wSOL RESERVE ATTESTATION #"[..]);
    msg.extend_from_slice(&u64_to_decimal(next_count));
    msg.extend_from_slice(b": ");
    msg.extend_from_slice(&u64_to_decimal(reserve_amount));
    msg.extend_from_slice(b" lamports backing declared");
    log_info(core::str::from_utf8(&msg).unwrap_or("event"));

    0
}

// ============================================================================
// QUERIES
// ============================================================================

#[no_mangle]
pub extern "C" fn balance_of(addr: *const u8) -> u64 {
    let mut address = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(addr, address.as_mut_ptr(), 32);
    }
    load_u64(&balance_key(&address))
}

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

#[no_mangle]
pub extern "C" fn total_supply() -> u64 {
    load_u64(TOTAL_SUPPLY_KEY)
}
#[no_mangle]
pub extern "C" fn total_minted() -> u64 {
    load_u64(TOTAL_MINTED_KEY)
}
#[no_mangle]
pub extern "C" fn total_burned() -> u64 {
    load_u64(TOTAL_BURNED_KEY)
}

#[no_mangle]
pub extern "C" fn get_reserve_ratio() -> u64 {
    let attested = load_u64(RESERVE_ATTESTED_KEY);
    let supply = load_u64(TOTAL_SUPPLY_KEY);
    if supply == 0 {
        return 10_000;
    }
    if attested == 0 {
        return 0;
    }
    ((attested as u128) * 10_000 / (supply as u128)) as u64
}

#[no_mangle]
pub extern "C" fn get_last_attestation_slot() -> u64 {
    load_u64(RESERVE_SLOT_KEY)
}
#[no_mangle]
pub extern "C" fn get_attestation_count() -> u64 {
    load_u64(ATTESTATION_COUNT_KEY)
}

#[no_mangle]
pub extern "C" fn get_epoch_remaining() -> u64 {
    let current_slot = get_slot();
    let epoch_start = load_u64(EPOCH_START_KEY);
    if current_slot >= epoch_start.saturating_add(EPOCH_SLOTS) {
        return MINT_CAP_PER_EPOCH;
    }
    let minted = load_u64(EPOCH_MINTED_KEY);
    if minted >= MINT_CAP_PER_EPOCH {
        0
    } else {
        MINT_CAP_PER_EPOCH - minted
    }
}

#[no_mangle]
pub extern "C" fn get_transfer_count() -> u64 {
    load_u64(TRANSFER_COUNT_KEY)
}

// ============================================================================
// ADMIN
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
    log_info("wSOL: EMERGENCY PAUSE");
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
    log_info("wSOL: RESUMED");
    0
}

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
    if is_bootstrap_complete()
        && (new_addr == load_addr(ATTESTER_KEY) || new_addr == load_addr(MINTER_KEY))
    {
        return 4;
    }
    storage_set(PENDING_ADMIN_KEY, &new_addr);
    log_info("wSOL: pending admin set");
    0
}

#[no_mangle]
pub extern "C" fn accept_admin(caller: *const u8) -> u32 {
    let mut caller_addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, caller_addr.as_mut_ptr(), 32);
    }
    let real_caller = get_caller();
    if real_caller.0 != caller_addr {
        return 200;
    }

    let pending_admin = load_addr(PENDING_ADMIN_KEY);
    if is_zero(&pending_admin) {
        return 1;
    }
    if pending_admin != caller_addr {
        return 2;
    }
    if is_bootstrap_complete()
        && (caller_addr == load_addr(ATTESTER_KEY) || caller_addr == load_addr(MINTER_KEY))
    {
        return 3;
    }

    storage_set(ADMIN_KEY, &caller_addr);
    storage_set(PENDING_ADMIN_KEY, &[0u8; 32]);
    log_info("wSOL: admin accepted");
    0
}

#[no_mangle]
pub extern "C" fn set_minter(caller: *const u8, new_minter: *const u8) -> u32 {
    let mut caller_addr = [0u8; 32];
    let mut new_addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, caller_addr.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(new_minter, new_addr.as_mut_ptr(), 32);
    }
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
    if is_bootstrap_complete()
        && (new_addr == load_addr(ADMIN_KEY) || new_addr == load_addr(ATTESTER_KEY))
    {
        return 4;
    }
    if load_addr(MINTER_KEY) == new_addr {
        return 0;
    }

    storage_set(MINTER_KEY, &new_addr);
    log_info("wSOL: mint authority updated");
    0
}

#[no_mangle]
pub extern "C" fn set_attester(caller: *const u8, new_attester: *const u8) -> u32 {
    let mut caller_addr = [0u8; 32];
    let mut new_addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, caller_addr.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(new_attester, new_addr.as_mut_ptr(), 32);
    }
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
    if is_bootstrap_complete()
        && (new_addr == load_addr(ADMIN_KEY) || new_addr == load_addr(MINTER_KEY))
    {
        return 4;
    }
    if load_addr(ATTESTER_KEY) == new_addr {
        return 0;
    }

    storage_set(ATTESTER_KEY, &new_addr);
    save_u64(RESERVE_ATTESTED_KEY, 0);
    save_u64(RESERVE_SLOT_KEY, 0);
    storage_set(RESERVE_HASH_KEY, &[0u8; 32]);
    log_info("wSOL: reserve attester updated");
    0
}

#[no_mangle]
pub extern "C" fn complete_bootstrap(caller: *const u8) -> u32 {
    let mut caller_addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, caller_addr.as_mut_ptr(), 32);
    }
    let real_caller = get_caller();
    if real_caller.0 != caller_addr {
        return 200;
    }
    if !require_admin(&caller_addr) {
        return 2;
    }
    if is_bootstrap_complete() {
        return 1;
    }

    let admin = load_addr(ADMIN_KEY);
    let attester = load_addr(ATTESTER_KEY);
    let minter = load_addr(MINTER_KEY);
    if is_zero(&attester)
        || is_zero(&minter)
        || attester == admin
        || attester == minter
        || admin == minter
    {
        return 3;
    }
    if load_u64(RESERVE_ATTESTED_KEY) == 0 || load_u64(RESERVE_SLOT_KEY) == 0 {
        return 4;
    }

    storage_set(BOOTSTRAP_COMPLETE_KEY, &[1u8]);
    log_info("wSOL: bootstrap completed");
    0
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use lichen_sdk::test_mock;

    fn set_slot(v: u64) {
        test_mock::SLOT.with(|s| *s.borrow_mut() = v);
    }

    fn addr(id: u8) -> [u8; 32] {
        let mut a = [0u8; 32];
        a[0] = id;
        a
    }

    #[test]
    fn test_initialize() {
        test_mock::reset();
        let admin = addr(1);
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(total_supply(), 0);
    }

    #[test]
    fn test_initialize_caller_mismatch_fails() {
        test_mock::reset();
        let admin = addr(1);
        test_mock::set_caller(addr(9));
        assert_eq!(initialize(admin.as_ptr()), 200);
        assert_eq!(load_addr(ADMIN_KEY), [0u8; 32]);
    }

    #[test]
    fn test_mint_and_burn() {
        test_mock::reset();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        assert_eq!(mint(admin.as_ptr(), user.as_ptr(), 1_500_000_000), 0); // 1.5 SOL
        assert_eq!(balance_of(user.as_ptr()), 1_500_000_000);
        assert_eq!(total_supply(), 1_500_000_000);

        test_mock::set_caller(user);
        assert_eq!(burn(user.as_ptr(), 500_000_000), 0); // burn 0.5 SOL
        assert_eq!(balance_of(user.as_ptr()), 1_000_000_000);
        assert_eq!(total_supply(), 1_000_000_000);
        assert_eq!(total_burned(), 500_000_000);
    }

    #[test]
    fn test_transfer() {
        test_mock::reset();
        let admin = addr(1);
        let alice = addr(2);
        let bob = addr(3);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        mint(admin.as_ptr(), alice.as_ptr(), 5_000_000_000);
        test_mock::set_caller(alice);
        assert_eq!(transfer(alice.as_ptr(), bob.as_ptr(), 2_000_000_000), 0);
        assert_eq!(balance_of(alice.as_ptr()), 3_000_000_000);
        assert_eq!(balance_of(bob.as_ptr()), 2_000_000_000);
    }

    #[test]
    fn test_approve_transfer_from() {
        test_mock::reset();
        let admin = addr(1);
        let alice = addr(2);
        let bob = addr(3);
        let dex = addr(4);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        mint(admin.as_ptr(), alice.as_ptr(), 10_000_000_000);
        test_mock::set_caller(alice);
        assert_eq!(approve(alice.as_ptr(), dex.as_ptr(), 5_000_000_000), 0);
        test_mock::set_caller(dex);
        assert_eq!(
            transfer_from(dex.as_ptr(), alice.as_ptr(), bob.as_ptr(), 3_000_000_000),
            0
        );
        assert_eq!(balance_of(bob.as_ptr()), 3_000_000_000);
        assert_eq!(allowance(alice.as_ptr(), dex.as_ptr()), 2_000_000_000);
    }

    #[test]
    fn test_reserve_circuit_breaker() {
        test_mock::reset();
        let admin = addr(1);
        let user = addr(2);
        let proof = [0xABu8; 32];
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        attest_reserves(admin.as_ptr(), 5_000_000_000, proof.as_ptr());
        assert_eq!(mint(admin.as_ptr(), user.as_ptr(), 5_000_000_000), 0);
        assert_eq!(mint(admin.as_ptr(), user.as_ptr(), 1), 10); // blocked
    }

    #[test]
    fn test_non_admin_cannot_mint() {
        test_mock::reset();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());
        test_mock::set_caller(user);
        assert_eq!(mint(user.as_ptr(), user.as_ptr(), 1_000_000_000), 2);
    }

    #[test]
    fn test_mint_overflow_preserves_state() {
        test_mock::reset();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());
        save_u64(&balance_key(&user), u64::MAX);
        save_u64(TOTAL_SUPPLY_KEY, 41);
        save_u64(TOTAL_MINTED_KEY, 7);
        save_u64(EPOCH_START_KEY, get_slot());
        save_u64(EPOCH_MINTED_KEY, 0);

        test_mock::set_caller(admin);
        assert_eq!(
            mint(admin.as_ptr(), user.as_ptr(), 1),
            ERR_ARITHMETIC_OVERFLOW
        );
        assert_eq!(balance_of(user.as_ptr()), u64::MAX);
        assert_eq!(total_supply(), 41);
        assert_eq!(total_minted(), 7);
        assert_eq!(load_u64(EPOCH_MINTED_KEY), 0);
        assert_eq!(load_u64(MINT_EVENT_COUNT_KEY), 0);
    }

    #[test]
    fn test_compliance_blocks_mint_without_mutation() {
        test_mock::reset();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());

        test_mock::set_can_receive(false);
        test_mock::set_caller(admin);
        assert_eq!(
            mint(admin.as_ptr(), user.as_ptr(), 1_000_000_000),
            ERR_COMPLIANCE_RESTRICTED
        );
        assert_eq!(balance_of(user.as_ptr()), 0);
        assert_eq!(total_supply(), 0);
        assert_eq!(total_minted(), 0);
        assert_eq!(load_u64(EPOCH_MINTED_KEY), 0);
        assert_eq!(load_u64(MINT_EVENT_COUNT_KEY), 0);
    }

    #[test]
    fn test_burn_underflow_preserves_state() {
        test_mock::reset();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());
        save_u64(&balance_key(&user), 1);
        save_u64(TOTAL_SUPPLY_KEY, 0);
        save_u64(TOTAL_BURNED_KEY, 9);

        test_mock::set_caller(user);
        assert_eq!(burn(user.as_ptr(), 1), ERR_ARITHMETIC_OVERFLOW);
        assert_eq!(balance_of(user.as_ptr()), 1);
        assert_eq!(total_supply(), 0);
        assert_eq!(total_burned(), 9);
        assert_eq!(load_u64(BURN_EVENT_COUNT_KEY), 0);
    }

    #[test]
    fn test_compliance_blocks_burn_without_mutation() {
        test_mock::reset();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        assert_eq!(mint(admin.as_ptr(), user.as_ptr(), 5_000_000_000), 0);

        test_mock::set_can_send(false);
        test_mock::set_caller(user);
        assert_eq!(
            burn(user.as_ptr(), 2_000_000_000),
            ERR_COMPLIANCE_RESTRICTED
        );
        assert_eq!(balance_of(user.as_ptr()), 5_000_000_000);
        assert_eq!(total_supply(), 5_000_000_000);
        assert_eq!(total_burned(), 0);
        assert_eq!(load_u64(BURN_EVENT_COUNT_KEY), 0);
    }

    #[test]
    fn test_transfer_overflow_preserves_state() {
        test_mock::reset();
        let admin = addr(1);
        let alice = addr(2);
        let bob = addr(3);
        initialize(admin.as_ptr());
        save_u64(&balance_key(&alice), 5);
        save_u64(&balance_key(&bob), u64::MAX);

        test_mock::set_caller(alice);
        assert_eq!(
            transfer(alice.as_ptr(), bob.as_ptr(), 1),
            ERR_ARITHMETIC_OVERFLOW
        );
        assert_eq!(balance_of(alice.as_ptr()), 5);
        assert_eq!(balance_of(bob.as_ptr()), u64::MAX);
        assert_eq!(load_u64(TRANSFER_COUNT_KEY), 0);
    }

    #[test]
    fn test_compliance_blocks_transfer_without_mutation() {
        test_mock::reset();
        let admin = addr(1);
        let alice = addr(2);
        let bob = addr(3);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        assert_eq!(mint(admin.as_ptr(), alice.as_ptr(), 10_000_000_000), 0);

        test_mock::set_can_transfer(false);
        test_mock::set_caller(alice);
        assert_eq!(
            transfer(alice.as_ptr(), bob.as_ptr(), 3_000_000_000),
            ERR_COMPLIANCE_RESTRICTED
        );
        assert_eq!(balance_of(alice.as_ptr()), 10_000_000_000);
        assert_eq!(balance_of(bob.as_ptr()), 0);
        assert_eq!(load_u64(TRANSFER_COUNT_KEY), 0);
    }

    #[test]
    fn test_compliance_blocks_transfer_from_without_mutation() {
        test_mock::reset();
        let admin = addr(1);
        let alice = addr(2);
        let bob = addr(3);
        let dex = addr(4);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        assert_eq!(mint(admin.as_ptr(), alice.as_ptr(), 10_000_000_000), 0);
        test_mock::set_caller(alice);
        assert_eq!(approve(alice.as_ptr(), dex.as_ptr(), 5_000_000_000), 0);

        test_mock::set_can_transfer(false);
        test_mock::set_caller(dex);
        assert_eq!(
            transfer_from(dex.as_ptr(), alice.as_ptr(), bob.as_ptr(), 3_000_000_000),
            ERR_COMPLIANCE_RESTRICTED
        );
        assert_eq!(balance_of(alice.as_ptr()), 10_000_000_000);
        assert_eq!(balance_of(bob.as_ptr()), 0);
        assert_eq!(allowance(alice.as_ptr(), dex.as_ptr()), 5_000_000_000);
        assert_eq!(load_u64(TRANSFER_COUNT_KEY), 0);
    }

    #[test]
    fn test_pause_blocks_operations() {
        test_mock::reset();
        let admin = addr(1);
        let user = addr(2);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        mint(admin.as_ptr(), user.as_ptr(), 1_000_000_000);
        emergency_pause(admin.as_ptr());
        test_mock::set_caller(user);
        assert_eq!(transfer(user.as_ptr(), admin.as_ptr(), 100), 1);
        assert_eq!(burn(user.as_ptr(), 100), 0);
        test_mock::set_caller(admin);
        emergency_unpause(admin.as_ptr());
        test_mock::set_caller(user);
        assert_eq!(transfer(user.as_ptr(), admin.as_ptr(), 100), 0);
    }

    #[test]
    fn test_admin_transfer_requires_acceptance() {
        test_mock::reset();
        let admin = addr(1);
        let new_admin = addr(5);
        let user = addr(2);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        assert_eq!(transfer_admin(admin.as_ptr(), new_admin.as_ptr()), 0);
        assert_eq!(load_addr(PENDING_ADMIN_KEY), new_admin);
        assert_eq!(mint(admin.as_ptr(), user.as_ptr(), 1_000_000_000), 0); // old admin still active
        test_mock::set_caller(new_admin);
        assert_eq!(mint(new_admin.as_ptr(), user.as_ptr(), 1_000_000_000), 2); // new admin not active yet
        assert_eq!(accept_admin(new_admin.as_ptr()), 0);
        assert_eq!(mint(new_admin.as_ptr(), user.as_ptr(), 1_000_000_000), 2);
        test_mock::set_caller(admin);
        assert_eq!(mint(admin.as_ptr(), user.as_ptr(), 1_000_000_000), 0);
        test_mock::set_caller(new_admin);
        assert_eq!(set_minter(new_admin.as_ptr(), new_admin.as_ptr()), 0);
        assert_eq!(mint(new_admin.as_ptr(), user.as_ptr(), 1_000_000_000), 0);
        assert_eq!(load_addr(PENDING_ADMIN_KEY), [0u8; 32]);
    }

    #[test]
    fn test_accept_admin_rejects_non_pending_admin() {
        test_mock::reset();
        let admin = addr(1);
        let new_admin = addr(5);
        let attacker = addr(9);
        initialize(admin.as_ptr());
        test_mock::set_caller(admin);
        assert_eq!(transfer_admin(admin.as_ptr(), new_admin.as_ptr()), 0);

        test_mock::set_caller(attacker);
        assert_eq!(accept_admin(attacker.as_ptr()), 2);
        assert_eq!(load_addr(ADMIN_KEY), admin);
        assert_eq!(load_addr(PENDING_ADMIN_KEY), new_admin);
    }

    #[test]
    fn test_attester_bootstrap_requires_split_and_fresh_attestation() {
        test_mock::reset();
        let admin = addr(1);
        let attester = addr(7);
        let minter = addr(8);
        let user = addr(2);
        let proof = addr(9);
        initialize(admin.as_ptr());

        test_mock::set_caller(admin);
        assert_eq!(complete_bootstrap(admin.as_ptr()), 3);
        assert_eq!(set_attester(admin.as_ptr(), attester.as_ptr()), 0);
        assert_eq!(set_minter(admin.as_ptr(), minter.as_ptr()), 0);
        assert_eq!(
            attest_reserves(admin.as_ptr(), 5_000_000_000, proof.as_ptr()),
            2
        );
        assert_eq!(complete_bootstrap(admin.as_ptr()), 4);

        set_slot(100);
        test_mock::set_caller(attester);
        assert_eq!(
            attest_reserves(attester.as_ptr(), 5_000_000_000, proof.as_ptr()),
            0
        );

        test_mock::set_caller(admin);
        assert_eq!(complete_bootstrap(admin.as_ptr()), 0);
        assert_eq!(transfer_admin(admin.as_ptr(), attester.as_ptr()), 4);
        assert_eq!(set_attester(admin.as_ptr(), admin.as_ptr()), 4);
        assert_eq!(set_attester(admin.as_ptr(), minter.as_ptr()), 4);
        assert_eq!(set_minter(admin.as_ptr(), attester.as_ptr()), 4);

        test_mock::set_caller(minter);
        assert_eq!(mint(minter.as_ptr(), user.as_ptr(), 1_000_000_000), 0);

        set_slot(100 + MAX_ATTESTATION_AGE_SLOTS + 1);
        assert_eq!(mint(minter.as_ptr(), user.as_ptr(), 1_000_000_000), 10);

        test_mock::set_caller(attester);
        assert_eq!(
            attest_reserves(attester.as_ptr(), 10_000_000_000, proof.as_ptr()),
            0
        );

        test_mock::set_caller(minter);
        assert_eq!(mint(minter.as_ptr(), user.as_ptr(), 1_000_000_000), 0);
    }
}
