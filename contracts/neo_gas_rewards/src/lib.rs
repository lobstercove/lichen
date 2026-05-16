// Neo GAS Rewards Vault
//
// The vault is an opt-in accounting layer for whole-lot wNEO positions that
// distributes reserve-backed wGAS reward imports. Reward imports are accepted
// only when an operator supplies unique route evidence and the active reward
// policy version, keeping this contract tied to the NX-850C evidence path.

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(dead_code)]

extern crate alloc;

use alloc::vec::Vec;
use lichen_sdk::{
    bytes_to_u64, call_contract, get_caller, get_contract_address, get_slot, log_info,
    set_return_data, storage_get, storage_set, u64_to_bytes, Address, CrossCall,
};

const WNEO_LOT: u64 = 1_000_000_000;
const INDEX_SCALE: u128 = 1_000_000_000_000_000_000;
const CAP_WINDOW_SLOTS: u64 = 216_000;
const MAX_EVIDENCE_AGE_SLOTS: u64 = 216_000;

const ERR_ALREADY_INITIALIZED: u32 = 1;
const ERR_UNAUTHORIZED: u32 = 2;
const ERR_INVALID_INPUT: u32 = 3;
const ERR_NOT_CONFIGURED: u32 = 4;
const ERR_PAUSED: u32 = 5;
const ERR_REENTRANCY: u32 = 6;
const ERR_OVERFLOW: u32 = 7;
const ERR_CAP_EXCEEDED: u32 = 8;
const ERR_TRANSFER_FAILED: u32 = 9;
const ERR_NOTHING_TO_CLAIM: u32 = 10;
const ERR_REPLAY: u32 = 11;
const ERR_STALE_EVIDENCE: u32 = 12;
const ERR_DISCLOSURE_REQUIRED: u32 = 13;
const ERR_POLICY_MISMATCH: u32 = 14;
const ERR_INSUFFICIENT_POSITION: u32 = 15;
const ERR_INSUFFICIENT_RESERVE: u32 = 16;
const ERR_CALLER_MISMATCH: u32 = 200;

const ADMIN_KEY: &[u8] = b"ngr_admin";
const IMPORTER_KEY: &[u8] = b"ngr_importer";
const WNEO_TOKEN_KEY: &[u8] = b"ngr_wneo";
const WGAS_TOKEN_KEY: &[u8] = b"ngr_wgas";
const PAUSED_KEY: &[u8] = b"ngr_paused";
const REENTRANCY_KEY: &[u8] = b"ngr_reent";
const CAPS_CONFIGURED_KEY: &[u8] = b"ngr_caps_set";

const ROUTE_CAP_KEY: &[u8] = b"ngr_route_cap";
const PER_USER_CAP_KEY: &[u8] = b"ngr_user_cap";
const DAILY_IMPORT_CAP_KEY: &[u8] = b"ngr_imp_cap";
const DAILY_CLAIM_CAP_KEY: &[u8] = b"ngr_clm_cap";
const CAMPAIGN_BUDGET_KEY: &[u8] = b"ngr_budget";
const IMPORT_WINDOW_START_KEY: &[u8] = b"ngr_imp_wstart";
const IMPORT_WINDOW_USED_KEY: &[u8] = b"ngr_imp_wused";
const CLAIM_WINDOW_START_KEY: &[u8] = b"ngr_clm_wstart";
const CLAIM_WINDOW_USED_KEY: &[u8] = b"ngr_clm_wused";

const DISCLOSURE_VERSION_KEY: &[u8] = b"ngr_disc_ver";
const DISCLOSURE_HASH_KEY: &[u8] = b"ngr_disc_hash";
const POLICY_VERSION_KEY: &[u8] = b"ngr_pol_ver";
const POLICY_HASH_KEY: &[u8] = b"ngr_pol_hash";

const TOTAL_PRINCIPAL_KEY: &[u8] = b"ngr_total_prn";
const TOTAL_IMPORTED_KEY: &[u8] = b"ngr_total_imp";
const TOTAL_CLAIMED_KEY: &[u8] = b"ngr_total_clm";
const REWARD_INDEX_KEY: &[u8] = b"ngr_idx";
const REWARD_DUST_KEY: &[u8] = b"ngr_dust";
const EVIDENCE_COUNT_KEY: &[u8] = b"ngr_evd_count";
const DEPOSIT_COUNT_KEY: &[u8] = b"ngr_dep_count";
const EXIT_COUNT_KEY: &[u8] = b"ngr_exit_count";
const CLAIM_COUNT_KEY: &[u8] = b"ngr_claim_count";

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key)
        .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
        .unwrap_or(0)
}

fn save_u64(key: &[u8], value: u64) {
    storage_set(key, &u64_to_bytes(value));
}

fn load_u128(key: &[u8]) -> u128 {
    storage_get(key)
        .map(|d| {
            let mut bytes = [0u8; 16];
            if d.len() >= 16 {
                bytes.copy_from_slice(&d[..16]);
            } else {
                bytes[..d.len()].copy_from_slice(&d);
            }
            u128::from_le_bytes(bytes)
        })
        .unwrap_or(0)
}

fn save_u128(key: &[u8], value: u128) {
    storage_set(key, &value.to_le_bytes());
}

fn load_addr(key: &[u8]) -> [u8; 32] {
    storage_get(key)
        .map(|d| {
            let mut out = [0u8; 32];
            if d.len() >= 32 {
                out.copy_from_slice(&d[..32]);
            }
            out
        })
        .unwrap_or([0u8; 32])
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

fn is_zero(value: &[u8; 32]) -> bool {
    value.iter().all(|b| *b == 0)
}

fn is_whole_wneo_lot(amount: u64) -> bool {
    amount > 0 && amount % WNEO_LOT == 0
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

fn key_with_addr(prefix: &[u8], addr: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::from(prefix);
    key.extend_from_slice(&hex_encode(addr));
    key
}

fn principal_key(addr: &[u8; 32]) -> Vec<u8> {
    key_with_addr(b"ngr_prn_", addr)
}

fn paid_index_key(addr: &[u8; 32]) -> Vec<u8> {
    key_with_addr(b"ngr_paid_", addr)
}

fn pending_key(addr: &[u8; 32]) -> Vec<u8> {
    key_with_addr(b"ngr_pend_", addr)
}

fn claimed_key(addr: &[u8; 32]) -> Vec<u8> {
    key_with_addr(b"ngr_uclm_", addr)
}

fn accepted_disclosure_key(addr: &[u8; 32]) -> Vec<u8> {
    key_with_addr(b"ngr_disc_acc_", addr)
}

fn evidence_key(hash: &[u8; 32]) -> Vec<u8> {
    key_with_addr(b"ngr_evd_", hash)
}

fn evidence_record_key(index: u64) -> Vec<u8> {
    let mut key = Vec::from(&b"ngr_evd_rec_"[..]);
    key.extend_from_slice(&u64_to_bytes(index));
    key
}

fn is_paused() -> bool {
    storage_get(PAUSED_KEY)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
}

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

struct ReentrancyGuard;

impl ReentrancyGuard {
    fn enter() -> Result<Self, u32> {
        if reentrancy_enter() {
            Ok(Self)
        } else {
            Err(ERR_REENTRANCY)
        }
    }
}

impl Drop for ReentrancyGuard {
    fn drop(&mut self) {
        reentrancy_exit();
    }
}

fn require_caller(addr: &[u8; 32]) -> Result<(), u32> {
    if get_caller().0 == *addr {
        Ok(())
    } else {
        Err(ERR_CALLER_MISMATCH)
    }
}

fn require_admin(caller: &[u8; 32]) -> Result<(), u32> {
    require_caller(caller)?;
    let admin = load_addr(ADMIN_KEY);
    if !is_zero(&admin) && admin == *caller {
        Ok(())
    } else {
        Err(ERR_UNAUTHORIZED)
    }
}

fn require_reward_importer(caller: &[u8; 32]) -> Result<(), u32> {
    require_caller(caller)?;
    let importer = load_addr(IMPORTER_KEY);
    if !is_zero(&importer) && importer == *caller {
        return Ok(());
    }
    let admin = load_addr(ADMIN_KEY);
    if !is_zero(&admin) && admin == *caller {
        Ok(())
    } else {
        Err(ERR_UNAUTHORIZED)
    }
}

fn caps_configured() -> bool {
    storage_get(CAPS_CONFIGURED_KEY)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
}

fn active_disclosure_version() -> u64 {
    load_u64(DISCLOSURE_VERSION_KEY)
}

fn accepted_current_disclosure(user: &[u8; 32]) -> bool {
    let current = active_disclosure_version();
    current > 0 && load_u64(&accepted_disclosure_key(user)) == current
}

fn is_configured() -> bool {
    !is_zero(&load_addr(WNEO_TOKEN_KEY))
        && !is_zero(&load_addr(WGAS_TOKEN_KEY))
        && caps_configured()
        && active_disclosure_version() > 0
        && load_u64(POLICY_VERSION_KEY) > 0
        && !is_zero(&load_addr(POLICY_HASH_KEY))
}

fn checked_add(lhs: u64, rhs: u64) -> Result<u64, u32> {
    lhs.checked_add(rhs).ok_or(ERR_OVERFLOW)
}

fn checked_sub(lhs: u64, rhs: u64) -> Result<u64, u32> {
    lhs.checked_sub(rhs).ok_or(ERR_OVERFLOW)
}

fn compute_window_update(
    start_key: &[u8],
    used_key: &[u8],
    cap: u64,
    amount: u64,
) -> Result<(u64, u64), u32> {
    let now = get_slot();
    let start = load_u64(start_key);
    let reset = start == 0 || now.saturating_sub(start) >= CAP_WINDOW_SLOTS;
    let used = if reset { 0 } else { load_u64(used_key) };
    let next_used = checked_add(used, amount)?;
    if next_used > cap {
        return Err(ERR_CAP_EXCEEDED);
    }
    Ok((if reset { now } else { start }, next_used))
}

fn apply_window_update(start_key: &[u8], used_key: &[u8], next_start: u64, next_used: u64) {
    save_u64(start_key, next_start);
    save_u64(used_key, next_used);
}

fn preview_claimable_for(user: &[u8; 32]) -> Result<(u64, u128), u32> {
    let principal = load_u64(&principal_key(user));
    let paid_index = load_u128(&paid_index_key(user));
    let index = load_u128(REWARD_INDEX_KEY);
    let pending = load_u64(&pending_key(user));

    if principal == 0 || index <= paid_index {
        return Ok((pending, index));
    }

    let delta = index.checked_sub(paid_index).ok_or(ERR_OVERFLOW)?;
    let accrued = (principal as u128)
        .checked_mul(delta)
        .ok_or(ERR_OVERFLOW)?
        .checked_div(INDEX_SCALE)
        .ok_or(ERR_OVERFLOW)?;
    if accrued > u64::MAX as u128 {
        return Err(ERR_OVERFLOW);
    }
    let next_pending = checked_add(pending, accrued as u64)?;
    Ok((next_pending, index))
}

fn call_status_success(result: &[u8]) -> bool {
    if result.len() >= 4 {
        let mut status = [0u8; 4];
        status.copy_from_slice(&result[..4]);
        let code = u32::from_le_bytes(status);
        return code == 0 || code == 1;
    }
    matches!(result.first().copied(), Some(0) | Some(1))
}

fn transfer_in_wneo(from: &[u8; 32], amount: u64) -> bool {
    let token = load_addr(WNEO_TOKEN_KEY);
    let vault = get_contract_address();
    let mut args = Vec::with_capacity(104);
    args.extend_from_slice(&vault.0);
    args.extend_from_slice(from);
    args.extend_from_slice(&vault.0);
    args.extend_from_slice(&u64_to_bytes(amount));

    let call = CrossCall::new(Address(token), "transfer_from", args).with_value(0);
    match call_contract(call) {
        Ok(result) => call_status_success(&result),
        Err(_) => false,
    }
}

fn transfer_out(token: &[u8; 32], to: &[u8; 32], amount: u64) -> bool {
    let vault = get_contract_address();
    let mut args = Vec::with_capacity(72);
    args.extend_from_slice(&vault.0);
    args.extend_from_slice(to);
    args.extend_from_slice(&u64_to_bytes(amount));

    let call = CrossCall::new(Address(*token), "transfer", args).with_value(0);
    match call_contract(call) {
        Ok(result) => call_status_success(&result),
        Err(_) => false,
    }
}

#[no_mangle]
pub extern "C" fn initialize(admin: *const u8) -> u32 {
    let admin_addr = match read_address32(admin) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    if is_zero(&admin_addr) {
        return ERR_INVALID_INPUT;
    }
    if let Err(code) = require_caller(&admin_addr) {
        return code;
    }
    if !is_zero(&load_addr(ADMIN_KEY)) {
        return ERR_ALREADY_INITIALIZED;
    }

    storage_set(ADMIN_KEY, &admin_addr);
    storage_set(IMPORTER_KEY, &admin_addr);
    storage_set(PAUSED_KEY, &[0u8]);
    save_u64(TOTAL_PRINCIPAL_KEY, 0);
    save_u64(TOTAL_IMPORTED_KEY, 0);
    save_u64(TOTAL_CLAIMED_KEY, 0);
    save_u128(REWARD_INDEX_KEY, 0);
    save_u128(REWARD_DUST_KEY, 0);
    log_info("Neo GAS rewards vault initialized");
    0
}

#[no_mangle]
pub extern "C" fn set_reward_importer(caller: *const u8, importer: *const u8) -> u32 {
    let caller_addr = match read_address32(caller) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    let importer_addr = match read_address32(importer) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    if let Err(code) = require_admin(&caller_addr) {
        return code;
    }
    if is_zero(&importer_addr) {
        return ERR_INVALID_INPUT;
    }
    if load_addr(IMPORTER_KEY) == importer_addr {
        return 0;
    }
    storage_set(IMPORTER_KEY, &importer_addr);
    0
}

#[no_mangle]
pub extern "C" fn set_token_addresses(caller: *const u8, wneo: *const u8, wgas: *const u8) -> u32 {
    let caller_addr = match read_address32(caller) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    let wneo_addr = match read_address32(wneo) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    let wgas_addr = match read_address32(wgas) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    if let Err(code) = require_admin(&caller_addr) {
        return code;
    }
    if is_zero(&wneo_addr) || is_zero(&wgas_addr) || wneo_addr == wgas_addr {
        return ERR_INVALID_INPUT;
    }
    if !is_zero(&load_addr(WNEO_TOKEN_KEY)) || !is_zero(&load_addr(WGAS_TOKEN_KEY)) {
        return ERR_ALREADY_INITIALIZED;
    }
    storage_set(WNEO_TOKEN_KEY, &wneo_addr);
    storage_set(WGAS_TOKEN_KEY, &wgas_addr);
    0
}

#[no_mangle]
pub extern "C" fn configure_caps(
    caller: *const u8,
    route_cap: u64,
    per_user_cap: u64,
    daily_import_cap: u64,
    daily_claim_cap: u64,
    campaign_budget: u64,
) -> u32 {
    let caller_addr = match read_address32(caller) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    if let Err(code) = require_admin(&caller_addr) {
        return code;
    }
    if !is_whole_wneo_lot(route_cap)
        || !is_whole_wneo_lot(per_user_cap)
        || per_user_cap > route_cap
        || daily_import_cap == 0
        || daily_claim_cap == 0
        || campaign_budget == 0
    {
        return ERR_INVALID_INPUT;
    }
    if route_cap < load_u64(TOTAL_PRINCIPAL_KEY) || campaign_budget < load_u64(TOTAL_IMPORTED_KEY) {
        return ERR_CAP_EXCEEDED;
    }

    save_u64(ROUTE_CAP_KEY, route_cap);
    save_u64(PER_USER_CAP_KEY, per_user_cap);
    save_u64(DAILY_IMPORT_CAP_KEY, daily_import_cap);
    save_u64(DAILY_CLAIM_CAP_KEY, daily_claim_cap);
    save_u64(CAMPAIGN_BUDGET_KEY, campaign_budget);
    storage_set(CAPS_CONFIGURED_KEY, &[1u8]);
    0
}

#[no_mangle]
pub extern "C" fn set_disclosure(caller: *const u8, version: u64, hash: *const u8) -> u32 {
    let caller_addr = match read_address32(caller) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    let hash_addr = match read_address32(hash) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    if let Err(code) = require_admin(&caller_addr) {
        return code;
    }
    if version == 0 || version <= load_u64(DISCLOSURE_VERSION_KEY) || is_zero(&hash_addr) {
        return ERR_INVALID_INPUT;
    }
    save_u64(DISCLOSURE_VERSION_KEY, version);
    storage_set(DISCLOSURE_HASH_KEY, &hash_addr);
    0
}

#[no_mangle]
pub extern "C" fn configure_reward_policy(
    caller: *const u8,
    version: u64,
    policy_hash: *const u8,
) -> u32 {
    let caller_addr = match read_address32(caller) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    let hash_addr = match read_address32(policy_hash) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    if let Err(code) = require_admin(&caller_addr) {
        return code;
    }
    if version == 0 || version <= load_u64(POLICY_VERSION_KEY) || is_zero(&hash_addr) {
        return ERR_INVALID_INPUT;
    }
    save_u64(POLICY_VERSION_KEY, version);
    storage_set(POLICY_HASH_KEY, &hash_addr);
    0
}

#[no_mangle]
pub extern "C" fn accept_disclosure(user: *const u8, version: u64) -> u32 {
    let user_addr = match read_address32(user) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    if let Err(code) = require_caller(&user_addr) {
        return code;
    }
    if is_zero(&user_addr) || version == 0 || version != active_disclosure_version() {
        return ERR_INVALID_INPUT;
    }
    save_u64(&accepted_disclosure_key(&user_addr), version);
    0
}

#[no_mangle]
pub extern "C" fn deposit(user: *const u8, amount: u64) -> u32 {
    let _guard = match ReentrancyGuard::enter() {
        Ok(guard) => guard,
        Err(code) => return code,
    };
    let user_addr = match read_address32(user) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    if let Err(code) = require_caller(&user_addr) {
        return code;
    }
    if is_paused() {
        return ERR_PAUSED;
    }
    if !is_configured() {
        return ERR_NOT_CONFIGURED;
    }
    if is_zero(&user_addr) || !is_whole_wneo_lot(amount) {
        return ERR_INVALID_INPUT;
    }
    if !accepted_current_disclosure(&user_addr) {
        return ERR_DISCLOSURE_REQUIRED;
    }

    let (next_pending, index) = match preview_claimable_for(&user_addr) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let current_principal = load_u64(&principal_key(&user_addr));
    let next_principal = match checked_add(current_principal, amount) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let next_total = match checked_add(load_u64(TOTAL_PRINCIPAL_KEY), amount) {
        Ok(value) => value,
        Err(code) => return code,
    };
    if next_principal > load_u64(PER_USER_CAP_KEY) || next_total > load_u64(ROUTE_CAP_KEY) {
        return ERR_CAP_EXCEEDED;
    }
    let next_deposit_count = match checked_add(load_u64(DEPOSIT_COUNT_KEY), 1) {
        Ok(value) => value,
        Err(code) => return code,
    };

    if !transfer_in_wneo(&user_addr, amount) {
        return ERR_TRANSFER_FAILED;
    }

    save_u64(&pending_key(&user_addr), next_pending);
    save_u128(&paid_index_key(&user_addr), index);
    save_u64(&principal_key(&user_addr), next_principal);
    save_u64(TOTAL_PRINCIPAL_KEY, next_total);
    save_u64(DEPOSIT_COUNT_KEY, next_deposit_count);
    0
}

#[no_mangle]
pub extern "C" fn exit(user: *const u8, amount: u64) -> u32 {
    let _guard = match ReentrancyGuard::enter() {
        Ok(guard) => guard,
        Err(code) => return code,
    };
    let user_addr = match read_address32(user) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    if let Err(code) = require_caller(&user_addr) {
        return code;
    }
    if !is_configured() {
        return ERR_NOT_CONFIGURED;
    }
    if is_zero(&user_addr) || !is_whole_wneo_lot(amount) {
        return ERR_INVALID_INPUT;
    }
    let current_principal = load_u64(&principal_key(&user_addr));
    if current_principal < amount {
        return ERR_INSUFFICIENT_POSITION;
    }

    let (next_pending, index) = match preview_claimable_for(&user_addr) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let next_principal = match checked_sub(current_principal, amount) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let next_total = match checked_sub(load_u64(TOTAL_PRINCIPAL_KEY), amount) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let next_exit_count = match checked_add(load_u64(EXIT_COUNT_KEY), 1) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let wneo = load_addr(WNEO_TOKEN_KEY);
    if !transfer_out(&wneo, &user_addr, amount) {
        return ERR_TRANSFER_FAILED;
    }

    save_u64(&pending_key(&user_addr), next_pending);
    save_u128(&paid_index_key(&user_addr), index);
    save_u64(&principal_key(&user_addr), next_principal);
    save_u64(TOTAL_PRINCIPAL_KEY, next_total);
    save_u64(EXIT_COUNT_KEY, next_exit_count);
    0
}

#[no_mangle]
pub extern "C" fn import_rewards(
    caller: *const u8,
    reward_amount: u64,
    evidence_slot: u64,
    policy_version: u64,
    evidence_hash: *const u8,
    route_evidence_hash: *const u8,
) -> u32 {
    let _guard = match ReentrancyGuard::enter() {
        Ok(guard) => guard,
        Err(code) => return code,
    };
    let caller_addr = match read_address32(caller) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    let evidence = match read_address32(evidence_hash) {
        Some(hash) => hash,
        None => return ERR_INVALID_INPUT,
    };
    let route_evidence = match read_address32(route_evidence_hash) {
        Some(hash) => hash,
        None => return ERR_INVALID_INPUT,
    };
    if let Err(code) = require_reward_importer(&caller_addr) {
        return code;
    }
    if is_paused() {
        return ERR_PAUSED;
    }
    if !is_configured() {
        return ERR_NOT_CONFIGURED;
    }
    if reward_amount == 0 || is_zero(&evidence) || is_zero(&route_evidence) {
        return ERR_INVALID_INPUT;
    }
    if policy_version == 0 || policy_version != load_u64(POLICY_VERSION_KEY) {
        return ERR_POLICY_MISMATCH;
    }
    let now = get_slot();
    if evidence_slot > now || now.saturating_sub(evidence_slot) > MAX_EVIDENCE_AGE_SLOTS {
        return ERR_STALE_EVIDENCE;
    }
    let evidence_key = evidence_key(&evidence);
    if storage_get(&evidence_key).is_some() {
        return ERR_REPLAY;
    }
    let total_principal = load_u64(TOTAL_PRINCIPAL_KEY);
    if total_principal == 0 {
        return ERR_INSUFFICIENT_POSITION;
    }
    let next_total_imported = match checked_add(load_u64(TOTAL_IMPORTED_KEY), reward_amount) {
        Ok(value) => value,
        Err(code) => return code,
    };
    if next_total_imported > load_u64(CAMPAIGN_BUDGET_KEY) {
        return ERR_CAP_EXCEEDED;
    }
    let (next_import_start, next_import_used) = match compute_window_update(
        IMPORT_WINDOW_START_KEY,
        IMPORT_WINDOW_USED_KEY,
        load_u64(DAILY_IMPORT_CAP_KEY),
        reward_amount,
    ) {
        Ok(value) => value,
        Err(code) => return code,
    };

    let dust = load_u128(REWARD_DUST_KEY);
    let scaled = match (reward_amount as u128)
        .checked_mul(INDEX_SCALE)
        .and_then(|v| v.checked_add(dust))
    {
        Some(value) => value,
        None => return ERR_OVERFLOW,
    };
    let denominator = total_principal as u128;
    let delta_index = scaled / denominator;
    let next_dust = scaled % denominator;
    let next_index = match load_u128(REWARD_INDEX_KEY).checked_add(delta_index) {
        Some(value) => value,
        None => return ERR_OVERFLOW,
    };
    let evidence_count = load_u64(EVIDENCE_COUNT_KEY);
    let next_evidence_count = match checked_add(evidence_count, 1) {
        Ok(value) => value,
        Err(code) => return code,
    };

    storage_set(&evidence_key, &[1u8]);
    let mut record = Vec::with_capacity(104);
    record.extend_from_slice(&u64_to_bytes(reward_amount));
    record.extend_from_slice(&u64_to_bytes(evidence_slot));
    record.extend_from_slice(&u64_to_bytes(policy_version));
    record.extend_from_slice(&evidence);
    record.extend_from_slice(&route_evidence);
    record.extend_from_slice(&u64_to_bytes(now));
    storage_set(&evidence_record_key(evidence_count), &record);

    save_u64(EVIDENCE_COUNT_KEY, next_evidence_count);
    save_u64(TOTAL_IMPORTED_KEY, next_total_imported);
    save_u128(REWARD_INDEX_KEY, next_index);
    save_u128(REWARD_DUST_KEY, next_dust);
    apply_window_update(
        IMPORT_WINDOW_START_KEY,
        IMPORT_WINDOW_USED_KEY,
        next_import_start,
        next_import_used,
    );
    0
}

#[no_mangle]
pub extern "C" fn claim(user: *const u8) -> u32 {
    let _guard = match ReentrancyGuard::enter() {
        Ok(guard) => guard,
        Err(code) => return code,
    };
    let user_addr = match read_address32(user) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    if let Err(code) = require_caller(&user_addr) {
        return code;
    }
    if is_paused() {
        return ERR_PAUSED;
    }
    if !is_configured() {
        return ERR_NOT_CONFIGURED;
    }
    if is_zero(&user_addr) {
        return ERR_INVALID_INPUT;
    }
    if !accepted_current_disclosure(&user_addr) {
        return ERR_DISCLOSURE_REQUIRED;
    }

    let (claimable, index) = match preview_claimable_for(&user_addr) {
        Ok(value) => value,
        Err(code) => return code,
    };
    if claimable == 0 {
        return ERR_NOTHING_TO_CLAIM;
    }

    let total_claimed = load_u64(TOTAL_CLAIMED_KEY);
    let next_total_claimed = match checked_add(total_claimed, claimable) {
        Ok(value) => value,
        Err(code) => return code,
    };
    if next_total_claimed > load_u64(TOTAL_IMPORTED_KEY) {
        return ERR_INSUFFICIENT_RESERVE;
    }
    let next_user_claimed = match checked_add(load_u64(&claimed_key(&user_addr)), claimable) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let (next_claim_start, next_claim_used) = match compute_window_update(
        CLAIM_WINDOW_START_KEY,
        CLAIM_WINDOW_USED_KEY,
        load_u64(DAILY_CLAIM_CAP_KEY),
        claimable,
    ) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let next_claim_count = match checked_add(load_u64(CLAIM_COUNT_KEY), 1) {
        Ok(value) => value,
        Err(code) => return code,
    };
    let wgas = load_addr(WGAS_TOKEN_KEY);
    if !transfer_out(&wgas, &user_addr, claimable) {
        return ERR_TRANSFER_FAILED;
    }

    save_u64(&pending_key(&user_addr), 0);
    save_u128(&paid_index_key(&user_addr), index);
    save_u64(&claimed_key(&user_addr), next_user_claimed);
    save_u64(TOTAL_CLAIMED_KEY, next_total_claimed);
    save_u64(CLAIM_COUNT_KEY, next_claim_count);
    apply_window_update(
        CLAIM_WINDOW_START_KEY,
        CLAIM_WINDOW_USED_KEY,
        next_claim_start,
        next_claim_used,
    );
    set_return_data(&u64_to_bytes(claimable));
    0
}

#[no_mangle]
pub extern "C" fn emergency_pause(caller: *const u8) -> u32 {
    let caller_addr = match read_address32(caller) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    if let Err(code) = require_admin(&caller_addr) {
        return code;
    }
    storage_set(PAUSED_KEY, &[1u8]);
    0
}

#[no_mangle]
pub extern "C" fn emergency_unpause(caller: *const u8) -> u32 {
    let caller_addr = match read_address32(caller) {
        Some(addr) => addr,
        None => return ERR_INVALID_INPUT,
    };
    if let Err(code) = require_admin(&caller_addr) {
        return code;
    }
    storage_set(PAUSED_KEY, &[0u8]);
    0
}

#[no_mangle]
pub extern "C" fn get_claimable(user: *const u8) -> u64 {
    let user_addr = match read_address32(user) {
        Some(addr) => addr,
        None => return 0,
    };
    preview_claimable_for(&user_addr)
        .map(|(claimable, _)| claimable)
        .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn get_principal(user: *const u8) -> u64 {
    let user_addr = match read_address32(user) {
        Some(addr) => addr,
        None => return 0,
    };
    load_u64(&principal_key(&user_addr))
}

#[no_mangle]
pub extern "C" fn get_claimed(user: *const u8) -> u64 {
    let user_addr = match read_address32(user) {
        Some(addr) => addr,
        None => return 0,
    };
    load_u64(&claimed_key(&user_addr))
}

#[no_mangle]
pub extern "C" fn get_vault_stats() -> u32 {
    let mut result = Vec::with_capacity(96);
    result.extend_from_slice(&u64_to_bytes(load_u64(TOTAL_PRINCIPAL_KEY)));
    result.extend_from_slice(&u64_to_bytes(load_u64(TOTAL_IMPORTED_KEY)));
    result.extend_from_slice(&u64_to_bytes(load_u64(TOTAL_CLAIMED_KEY)));
    result.extend_from_slice(&u64_to_bytes(load_u64(EVIDENCE_COUNT_KEY)));
    result.extend_from_slice(&u64_to_bytes(load_u64(DEPOSIT_COUNT_KEY)));
    result.extend_from_slice(&u64_to_bytes(load_u64(EXIT_COUNT_KEY)));
    result.extend_from_slice(&u64_to_bytes(load_u64(CLAIM_COUNT_KEY)));
    result.extend_from_slice(&u64_to_bytes(load_u64(POLICY_VERSION_KEY)));
    result.extend_from_slice(&u64_to_bytes(load_u64(DISCLOSURE_VERSION_KEY)));
    result.extend_from_slice(&u64_to_bytes(if is_paused() { 1 } else { 0 }));
    result.extend_from_slice(&u64_to_bytes(load_u64(ROUTE_CAP_KEY)));
    result.extend_from_slice(&u64_to_bytes(load_u64(CAMPAIGN_BUDGET_KEY)));
    set_return_data(&result);
    0
}

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn call() -> u32 {
    let args = lichen_sdk::get_args();
    if args.is_empty() {
        return 255;
    }
    let mut rc = 0u32;
    let mut return_rc = true;
    match args[0] {
        0 => {
            if args.len() >= 33 {
                rc = initialize(args[1..33].as_ptr());
            } else {
                rc = ERR_INVALID_INPUT;
            }
        }
        1 => {
            if args.len() >= 97 {
                rc = set_token_addresses(
                    args[1..33].as_ptr(),
                    args[33..65].as_ptr(),
                    args[65..97].as_ptr(),
                );
            } else {
                rc = ERR_INVALID_INPUT;
            }
        }
        16 => {
            if args.len() >= 65 {
                rc = set_reward_importer(args[1..33].as_ptr(), args[33..65].as_ptr());
            } else {
                rc = ERR_INVALID_INPUT;
            }
        }
        2 => {
            if args.len() >= 73 {
                rc = configure_caps(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    bytes_to_u64(&args[41..49]),
                    bytes_to_u64(&args[49..57]),
                    bytes_to_u64(&args[57..65]),
                    bytes_to_u64(&args[65..73]),
                );
            } else {
                rc = ERR_INVALID_INPUT;
            }
        }
        3 => {
            if args.len() >= 73 {
                rc = set_disclosure(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    args[41..73].as_ptr(),
                );
            } else {
                rc = ERR_INVALID_INPUT;
            }
        }
        4 => {
            if args.len() >= 73 {
                rc = configure_reward_policy(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    args[41..73].as_ptr(),
                );
            } else {
                rc = ERR_INVALID_INPUT;
            }
        }
        5 => {
            if args.len() >= 41 {
                rc = accept_disclosure(args[1..33].as_ptr(), bytes_to_u64(&args[33..41]));
            } else {
                rc = ERR_INVALID_INPUT;
            }
        }
        6 => {
            if args.len() >= 41 {
                rc = deposit(args[1..33].as_ptr(), bytes_to_u64(&args[33..41]));
            } else {
                rc = ERR_INVALID_INPUT;
            }
        }
        7 => {
            if args.len() >= 41 {
                rc = exit(args[1..33].as_ptr(), bytes_to_u64(&args[33..41]));
            } else {
                rc = ERR_INVALID_INPUT;
            }
        }
        8 => {
            if args.len() >= 121 {
                rc = import_rewards(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    bytes_to_u64(&args[41..49]),
                    bytes_to_u64(&args[49..57]),
                    args[57..89].as_ptr(),
                    args[89..121].as_ptr(),
                );
            } else {
                rc = ERR_INVALID_INPUT;
            }
        }
        9 => {
            if args.len() >= 33 {
                rc = claim(args[1..33].as_ptr());
            } else {
                rc = ERR_INVALID_INPUT;
            }
        }
        10 => {
            if args.len() >= 33 {
                rc = emergency_pause(args[1..33].as_ptr());
            } else {
                rc = ERR_INVALID_INPUT;
            }
        }
        11 => {
            if args.len() >= 33 {
                rc = emergency_unpause(args[1..33].as_ptr());
            } else {
                rc = ERR_INVALID_INPUT;
            }
        }
        12 => {
            if args.len() >= 33 {
                set_return_data(&u64_to_bytes(get_claimable(args[1..33].as_ptr())));
                return_rc = false;
            } else {
                rc = ERR_INVALID_INPUT;
            }
        }
        13 => {
            if args.len() >= 33 {
                set_return_data(&u64_to_bytes(get_principal(args[1..33].as_ptr())));
                return_rc = false;
            } else {
                rc = ERR_INVALID_INPUT;
            }
        }
        14 => {
            if args.len() >= 33 {
                set_return_data(&u64_to_bytes(get_claimed(args[1..33].as_ptr())));
                return_rc = false;
            } else {
                rc = ERR_INVALID_INPUT;
            }
        }
        15 => {
            rc = get_vault_stats();
            return_rc = false;
        }
        _ => rc = 255,
    }
    if return_rc {
        set_return_data(&u64_to_bytes(rc as u64));
    }
    rc
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use lichen_sdk::test_mock;

    fn addr(id: u8) -> [u8; 32] {
        let mut out = [0u8; 32];
        out[0] = id;
        out
    }

    fn hash(id: u8) -> [u8; 32] {
        let mut out = [0u8; 32];
        out[0] = id;
        out[31] = 0xA5;
        out
    }

    fn setup_base() -> ([u8; 32], [u8; 32], [u8; 32]) {
        test_mock::reset();
        let admin = addr(1);
        let user = addr(2);
        let vault = addr(200);
        let wneo = addr(50);
        let wgas = addr(51);
        test_mock::set_caller(admin);
        test_mock::set_contract_address(vault);
        test_mock::set_slot(1_000);
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(
            set_token_addresses(admin.as_ptr(), wneo.as_ptr(), wgas.as_ptr()),
            0
        );
        assert_eq!(
            configure_caps(
                admin.as_ptr(),
                100 * WNEO_LOT,
                20 * WNEO_LOT,
                100_000,
                100_000,
                1_000_000,
            ),
            0
        );
        assert_eq!(set_disclosure(admin.as_ptr(), 1, hash(10).as_ptr()), 0);
        assert_eq!(
            configure_reward_policy(admin.as_ptr(), 1, hash(11).as_ptr()),
            0
        );
        test_mock::set_caller(user);
        assert_eq!(accept_disclosure(user.as_ptr(), 1), 0);
        (admin, user, vault)
    }

    fn deposit_lots(user: [u8; 32], lots: u64) {
        test_mock::set_caller(user);
        assert_eq!(deposit(user.as_ptr(), lots * WNEO_LOT), 0);
    }

    fn import_reward(admin: [u8; 32], amount: u64, evidence_id: u8) {
        test_mock::set_caller(admin);
        assert_eq!(
            import_rewards(
                admin.as_ptr(),
                amount,
                get_slot(),
                1,
                hash(evidence_id).as_ptr(),
                hash(evidence_id.wrapping_add(100)).as_ptr(),
            ),
            0
        );
    }

    #[test]
    fn test_fail_closed_until_caps_policy_and_disclosure_configured() {
        test_mock::reset();
        let admin = addr(1);
        let user = addr(2);
        let vault = addr(200);
        let wneo = addr(50);
        let wgas = addr(51);
        test_mock::set_contract_address(vault);
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(
            set_token_addresses(admin.as_ptr(), wneo.as_ptr(), wgas.as_ptr()),
            0
        );

        test_mock::set_caller(user);
        assert_eq!(deposit(user.as_ptr(), WNEO_LOT), ERR_NOT_CONFIGURED);
    }

    #[test]
    fn test_whole_lot_wneo_deposit_required() {
        let (_admin, user, _vault) = setup_base();
        test_mock::set_caller(user);
        assert_eq!(deposit(user.as_ptr(), WNEO_LOT / 2), ERR_INVALID_INPUT);
        assert_eq!(deposit(user.as_ptr(), WNEO_LOT), 0);
    }

    #[test]
    fn test_deposit_uses_wneo_transfer_from() {
        let (_admin, user, vault) = setup_base();
        test_mock::set_caller(user);
        assert_eq!(deposit(user.as_ptr(), WNEO_LOT), 0);
        let call = test_mock::get_last_cross_call().expect("wNEO transfer_from call");
        assert_eq!(call.0, addr(50));
        assert_eq!(call.1, "transfer_from");
        assert_eq!(&call.2[..32], &vault);
        assert_eq!(&call.2[32..64], &user);
        assert_eq!(&call.2[64..96], &vault);
        assert_eq!(bytes_to_u64(&call.2[96..104]), WNEO_LOT);
    }

    #[test]
    fn test_disclosure_required_for_deposit_and_claim() {
        let (admin, user, _vault) = setup_base();
        let new_user = addr(3);
        test_mock::set_caller(new_user);
        assert_eq!(
            deposit(new_user.as_ptr(), WNEO_LOT),
            ERR_DISCLOSURE_REQUIRED
        );

        deposit_lots(user, 10);
        import_reward(admin, 1_000, 20);
        test_mock::set_caller(admin);
        assert_eq!(set_disclosure(admin.as_ptr(), 2, hash(12).as_ptr()), 0);
        test_mock::set_caller(user);
        assert_eq!(claim(user.as_ptr()), ERR_DISCLOSURE_REQUIRED);
        assert_eq!(accept_disclosure(user.as_ptr(), 2), 0);
        assert_eq!(claim(user.as_ptr()), 0);
        assert_eq!(get_claimed(user.as_ptr()), 1_000);
    }

    #[test]
    fn test_reward_import_requires_policy_and_unique_evidence() {
        let (admin, user, _vault) = setup_base();
        deposit_lots(user, 10);
        test_mock::set_caller(admin);
        assert_eq!(
            import_rewards(
                admin.as_ptr(),
                1_000,
                get_slot(),
                2,
                hash(21).as_ptr(),
                hash(121).as_ptr(),
            ),
            ERR_POLICY_MISMATCH
        );
        assert_eq!(
            import_rewards(
                admin.as_ptr(),
                1_000,
                get_slot(),
                1,
                hash(21).as_ptr(),
                hash(121).as_ptr(),
            ),
            0
        );
        assert_eq!(
            import_rewards(
                admin.as_ptr(),
                1_000,
                get_slot(),
                1,
                hash(21).as_ptr(),
                hash(122).as_ptr(),
            ),
            ERR_REPLAY
        );
    }

    #[test]
    fn test_admin_controls_reject_non_admin_callers() {
        let (_admin, user, _vault) = setup_base();
        let wneo = addr(52);
        let wgas = addr(53);

        test_mock::set_caller(user);
        assert_eq!(
            set_token_addresses(user.as_ptr(), wneo.as_ptr(), wgas.as_ptr()),
            ERR_UNAUTHORIZED
        );
        assert_eq!(
            configure_caps(
                user.as_ptr(),
                10 * WNEO_LOT,
                10 * WNEO_LOT,
                1_000,
                1_000,
                1_000,
            ),
            ERR_UNAUTHORIZED
        );
        assert_eq!(
            set_disclosure(user.as_ptr(), 2, hash(90).as_ptr()),
            ERR_UNAUTHORIZED
        );
        assert_eq!(
            configure_reward_policy(user.as_ptr(), 2, hash(91).as_ptr()),
            ERR_UNAUTHORIZED
        );
        assert_eq!(
            set_reward_importer(user.as_ptr(), addr(54).as_ptr()),
            ERR_UNAUTHORIZED
        );
        assert_eq!(
            import_rewards(
                user.as_ptr(),
                100,
                get_slot(),
                1,
                hash(92).as_ptr(),
                hash(192).as_ptr(),
            ),
            ERR_UNAUTHORIZED
        );
        assert_eq!(emergency_pause(user.as_ptr()), ERR_UNAUTHORIZED);
        assert_eq!(emergency_unpause(user.as_ptr()), ERR_UNAUTHORIZED);
        assert!(!is_paused());
        assert_eq!(load_u64(EVIDENCE_COUNT_KEY), 0);
    }

    #[test]
    fn test_reward_importer_role_can_import_without_admin_role() {
        let (admin, user, _vault) = setup_base();
        let importer = addr(3);

        test_mock::set_caller(admin);
        assert_eq!(set_reward_importer(admin.as_ptr(), importer.as_ptr()), 0);
        assert_eq!(load_addr(IMPORTER_KEY), importer);
        deposit_lots(user, 10);
        assert_eq!(load_addr(IMPORTER_KEY), importer);

        test_mock::set_caller(importer);
        assert_eq!(
            import_rewards(
                importer.as_ptr(),
                1_000,
                get_slot(),
                1,
                hash(93).as_ptr(),
                hash(193).as_ptr(),
            ),
            0
        );
        assert_eq!(load_u64(TOTAL_IMPORTED_KEY), 1_000);
        assert_eq!(load_u64(EVIDENCE_COUNT_KEY), 1);

        test_mock::set_caller(user);
        assert_eq!(
            import_rewards(
                user.as_ptr(),
                1_000,
                get_slot(),
                1,
                hash(94).as_ptr(),
                hash(194).as_ptr(),
            ),
            ERR_UNAUTHORIZED
        );
        assert_eq!(load_u64(TOTAL_IMPORTED_KEY), 1_000);
        assert_eq!(load_u64(EVIDENCE_COUNT_KEY), 1);
    }

    #[test]
    fn test_reward_importer_configuration_rejects_invalid_inputs() {
        let (admin, _user, _vault) = setup_base();
        let zero = [0u8; 32];
        let importer = addr(4);

        test_mock::set_caller(admin);
        assert_eq!(
            set_reward_importer(admin.as_ptr(), zero.as_ptr()),
            ERR_INVALID_INPUT
        );
        assert_eq!(set_reward_importer(admin.as_ptr(), importer.as_ptr()), 0);
        assert_eq!(load_addr(IMPORTER_KEY), importer);
        assert_eq!(set_reward_importer(admin.as_ptr(), importer.as_ptr()), 0);
        assert_eq!(load_addr(IMPORTER_KEY), importer);
    }

    #[test]
    fn test_user_paths_are_bound_to_signing_caller() {
        let (admin, user, _vault) = setup_base();
        let attacker = addr(9);
        test_mock::set_caller(attacker);
        assert_eq!(accept_disclosure(user.as_ptr(), 1), ERR_CALLER_MISMATCH);
        assert_eq!(deposit(user.as_ptr(), WNEO_LOT), ERR_CALLER_MISMATCH);

        deposit_lots(user, 2);
        import_reward(admin, 250, 93);

        test_mock::set_caller(attacker);
        assert_eq!(exit(user.as_ptr(), WNEO_LOT), ERR_CALLER_MISMATCH);
        assert_eq!(claim(user.as_ptr()), ERR_CALLER_MISMATCH);
        assert_eq!(get_principal(user.as_ptr()), 2 * WNEO_LOT);
        assert_eq!(get_claimable(user.as_ptr()), 250);
        assert_eq!(get_claimed(user.as_ptr()), 0);
    }

    #[test]
    fn test_reward_import_rejects_zero_amount_and_zero_evidence_hashes() {
        let (admin, user, _vault) = setup_base();
        deposit_lots(user, 10);
        let zero = [0u8; 32];

        test_mock::set_caller(admin);
        assert_eq!(
            import_rewards(
                admin.as_ptr(),
                0,
                get_slot(),
                1,
                hash(94).as_ptr(),
                hash(194).as_ptr(),
            ),
            ERR_INVALID_INPUT
        );
        assert_eq!(
            import_rewards(
                admin.as_ptr(),
                1_000,
                get_slot(),
                1,
                zero.as_ptr(),
                hash(195).as_ptr(),
            ),
            ERR_INVALID_INPUT
        );
        assert_eq!(
            import_rewards(
                admin.as_ptr(),
                1_000,
                get_slot(),
                1,
                hash(95).as_ptr(),
                zero.as_ptr(),
            ),
            ERR_INVALID_INPUT
        );
        assert_eq!(load_u64(TOTAL_IMPORTED_KEY), 0);
        assert_eq!(load_u64(EVIDENCE_COUNT_KEY), 0);
        assert_eq!(get_claimable(user.as_ptr()), 0);
    }

    #[test]
    fn test_reward_import_without_principal_does_not_burn_evidence() {
        let (admin, user, _vault) = setup_base();
        test_mock::set_caller(admin);
        assert_eq!(
            import_rewards(
                admin.as_ptr(),
                1_000,
                get_slot(),
                1,
                hash(96).as_ptr(),
                hash(196).as_ptr(),
            ),
            ERR_INSUFFICIENT_POSITION
        );
        assert_eq!(load_u64(EVIDENCE_COUNT_KEY), 0);
        assert!(storage_get(&evidence_key(&hash(96))).is_none());

        deposit_lots(user, 1);
        test_mock::set_caller(admin);
        assert_eq!(
            import_rewards(
                admin.as_ptr(),
                1_000,
                get_slot(),
                1,
                hash(96).as_ptr(),
                hash(196).as_ptr(),
            ),
            0
        );
        assert_eq!(load_u64(EVIDENCE_COUNT_KEY), 1);
        assert!(storage_get(&evidence_key(&hash(96))).is_some());
    }

    #[test]
    fn test_caps_cannot_be_shrunk_below_existing_principal_or_imports() {
        let (admin, user, _vault) = setup_base();
        deposit_lots(user, 10);
        import_reward(admin, 1_000, 97);

        test_mock::set_caller(admin);
        assert_eq!(
            configure_caps(
                admin.as_ptr(),
                9 * WNEO_LOT,
                9 * WNEO_LOT,
                100_000,
                100_000,
                1_000_000,
            ),
            ERR_CAP_EXCEEDED
        );
        assert_eq!(
            configure_caps(
                admin.as_ptr(),
                100 * WNEO_LOT,
                20 * WNEO_LOT,
                100_000,
                100_000,
                999,
            ),
            ERR_CAP_EXCEEDED
        );
        assert_eq!(load_u64(ROUTE_CAP_KEY), 100 * WNEO_LOT);
        assert_eq!(load_u64(CAMPAIGN_BUDGET_KEY), 1_000_000);
        assert_eq!(get_principal(user.as_ptr()), 10 * WNEO_LOT);
        assert_eq!(get_claimable(user.as_ptr()), 1_000);
    }

    #[test]
    fn test_pro_rata_distribution_excludes_late_deposit_from_prior_rewards() {
        let (admin, alice, _vault) = setup_base();
        let bob = addr(3);
        test_mock::set_caller(bob);
        assert_eq!(accept_disclosure(bob.as_ptr(), 1), 0);

        deposit_lots(alice, 10);
        import_reward(admin, 1_000, 30);
        deposit_lots(bob, 10);
        import_reward(admin, 1_000, 31);

        assert_eq!(get_claimable(alice.as_ptr()), 1_500);
        assert_eq!(get_claimable(bob.as_ptr()), 500);
    }

    #[test]
    fn test_claim_transfers_wgas_and_preserves_state_on_transfer_failure() {
        let (admin, user, vault) = setup_base();
        deposit_lots(user, 10);
        import_reward(admin, 1_000, 40);

        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        test_mock::set_caller(user);
        assert_eq!(claim(user.as_ptr()), ERR_TRANSFER_FAILED);
        assert_eq!(get_claimable(user.as_ptr()), 1_000);
        assert_eq!(get_claimed(user.as_ptr()), 0);
        assert_eq!(load_u64(TOTAL_CLAIMED_KEY), 0);

        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        assert_eq!(claim(user.as_ptr()), 0);
        assert_eq!(get_claimable(user.as_ptr()), 0);
        assert_eq!(get_claimed(user.as_ptr()), 1_000);
        assert_eq!(load_u64(TOTAL_CLAIMED_KEY), 1_000);
        let call = test_mock::get_last_cross_call().expect("wGAS transfer call");
        assert_eq!(call.0, addr(51));
        assert_eq!(call.1, "transfer");
        assert_eq!(&call.2[..32], &vault);
        assert_eq!(&call.2[32..64], &user);
        assert_eq!(bytes_to_u64(&call.2[64..72]), 1_000);
    }

    #[test]
    fn test_exit_transfer_failure_preserves_principal_and_rewards() {
        let (admin, user, _vault) = setup_base();
        deposit_lots(user, 5);
        import_reward(admin, 500, 98);

        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        test_mock::set_caller(user);
        assert_eq!(exit(user.as_ptr(), 2 * WNEO_LOT), ERR_TRANSFER_FAILED);
        assert_eq!(get_principal(user.as_ptr()), 5 * WNEO_LOT);
        assert_eq!(load_u64(TOTAL_PRINCIPAL_KEY), 5 * WNEO_LOT);
        assert_eq!(get_claimable(user.as_ptr()), 500);
        assert_eq!(load_u64(EXIT_COUNT_KEY), 0);

        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        assert_eq!(exit(user.as_ptr(), 2 * WNEO_LOT), 0);
        assert_eq!(get_principal(user.as_ptr()), 3 * WNEO_LOT);
        assert_eq!(load_u64(TOTAL_PRINCIPAL_KEY), 3 * WNEO_LOT);
        assert_eq!(get_claimable(user.as_ptr()), 500);
        assert_eq!(load_u64(EXIT_COUNT_KEY), 1);
    }

    #[test]
    fn test_pause_blocks_deposit_import_claim_but_exit_remains_open() {
        let (admin, user, _vault) = setup_base();
        deposit_lots(user, 10);
        import_reward(admin, 1_000, 50);

        test_mock::set_caller(admin);
        assert_eq!(emergency_pause(admin.as_ptr()), 0);

        test_mock::set_caller(user);
        assert_eq!(deposit(user.as_ptr(), WNEO_LOT), ERR_PAUSED);
        assert_eq!(claim(user.as_ptr()), ERR_PAUSED);

        test_mock::set_caller(admin);
        assert_eq!(
            import_rewards(
                admin.as_ptr(),
                1_000,
                get_slot(),
                1,
                hash(51).as_ptr(),
                hash(151).as_ptr(),
            ),
            ERR_PAUSED
        );

        test_mock::set_caller(user);
        assert_eq!(exit(user.as_ptr(), 10 * WNEO_LOT), 0);
        assert_eq!(get_principal(user.as_ptr()), 0);
        assert_eq!(get_claimable(user.as_ptr()), 1_000);
    }

    #[test]
    fn test_exit_preserves_pending_rewards_and_releases_wneo() {
        let (admin, user, vault) = setup_base();
        deposit_lots(user, 5);
        import_reward(admin, 500, 60);
        test_mock::set_caller(user);
        assert_eq!(exit(user.as_ptr(), 2 * WNEO_LOT), 0);
        assert_eq!(get_principal(user.as_ptr()), 3 * WNEO_LOT);
        assert_eq!(get_claimable(user.as_ptr()), 500);
        let call = test_mock::get_last_cross_call().expect("wNEO transfer call");
        assert_eq!(call.0, addr(50));
        assert_eq!(call.1, "transfer");
        assert_eq!(&call.2[..32], &vault);
        assert_eq!(&call.2[32..64], &user);
        assert_eq!(bytes_to_u64(&call.2[64..72]), 2 * WNEO_LOT);
    }

    #[test]
    fn test_stale_or_future_evidence_rejected() {
        let (admin, user, _vault) = setup_base();
        deposit_lots(user, 10);
        test_mock::set_caller(admin);
        test_mock::set_slot(1_000 + MAX_EVIDENCE_AGE_SLOTS + 1);
        assert_eq!(
            import_rewards(
                admin.as_ptr(),
                1_000,
                1_000,
                1,
                hash(70).as_ptr(),
                hash(170).as_ptr(),
            ),
            ERR_STALE_EVIDENCE
        );
        assert_eq!(
            import_rewards(
                admin.as_ptr(),
                1_000,
                get_slot() + 1,
                1,
                hash(71).as_ptr(),
                hash(171).as_ptr(),
            ),
            ERR_STALE_EVIDENCE
        );
    }

    #[test]
    fn test_claim_cannot_exceed_imported_rewards() {
        let (_admin, user, _vault) = setup_base();
        save_u64(&pending_key(&user), 100);
        save_u128(&paid_index_key(&user), load_u128(REWARD_INDEX_KEY));
        save_u64(TOTAL_IMPORTED_KEY, 50);
        test_mock::set_caller(user);
        assert_eq!(claim(user.as_ptr()), ERR_INSUFFICIENT_RESERVE);
        assert_eq!(get_claimable(user.as_ptr()), 100);
        assert_eq!(get_claimed(user.as_ptr()), 0);
    }

    #[test]
    fn test_caps_enforce_route_user_import_budget_and_claim_windows() {
        let (admin, user, _vault) = setup_base();
        test_mock::set_caller(admin);
        assert_eq!(
            configure_caps(
                admin.as_ptr(),
                2 * WNEO_LOT,
                2 * WNEO_LOT,
                1_000,
                500,
                1_000,
            ),
            0
        );
        deposit_lots(user, 2);
        test_mock::set_caller(user);
        assert_eq!(deposit(user.as_ptr(), WNEO_LOT), ERR_CAP_EXCEEDED);

        test_mock::set_caller(admin);
        assert_eq!(
            import_rewards(
                admin.as_ptr(),
                1_001,
                get_slot(),
                1,
                hash(80).as_ptr(),
                hash(180).as_ptr(),
            ),
            ERR_CAP_EXCEEDED
        );
        assert_eq!(
            import_rewards(
                admin.as_ptr(),
                600,
                get_slot(),
                1,
                hash(81).as_ptr(),
                hash(181).as_ptr(),
            ),
            0
        );
        test_mock::set_caller(user);
        assert_eq!(claim(user.as_ptr()), ERR_CAP_EXCEEDED);
        assert_eq!(get_claimable(user.as_ptr()), 600);
    }
}
