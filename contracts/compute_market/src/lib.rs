// Compute Marketplace v2 — Decentralized Compute for Lichen
//
// Allows compute providers to offer resources and requesters to submit jobs:
//   - Providers register with capacity and pricing
//   - Requesters submit compute jobs with hash of code
//   - Providers claim and complete jobs
//   - Escrow payment held until challenge period expires
//   - Arbitrated dispute resolution with configurable split
//   - Job cancellation with timeout enforcement
//   - Provider management (deactivate/reactivate/update)
//
// v2 additions:
//   - Escrow: payment locked on submit, released after challenge period
//   - Timeouts: claim timeout, complete timeout, challenge period
//   - Arbitrators: admin-appointed dispute resolvers
//   - cancel_job: requester cancels pending/timed-out jobs
//   - release_payment: anyone triggers after challenge period
//   - resolve_dispute: arbitrator splits payment
//
// Storage keys:
//   provider_{addr}     → ProviderInfo
//   job_{id}            → JobInfo
//   job_count           → u64
//   escrow_{id}         → u64 (escrowed amount)
//   cm_admin            → 32 bytes admin address
//   arbitrator_{addr}   → [1] if active
//   claim_timeout       → u64 (slots)
//   complete_timeout    → u64 (slots)
//   challenge_period    → u64 (slots)

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;
use alloc::vec::Vec;

use lichen_sdk::{
    bytes_to_u64, call_contract, get_caller, get_contract_address, get_slot, log_info,
    receive_token_or_native, storage_get, storage_set, transfer_token_or_native, u64_to_bytes,
    Address, CrossCall,
};

// SECURITY: Reentrancy guard
const CM_REENTRANCY_KEY: &[u8] = b"cm_reentrancy";
fn reentrancy_enter() -> bool {
    if let Some(v) = storage_get(CM_REENTRANCY_KEY) {
        if !v.is_empty() && v[0] == 1 {
            return false;
        }
    }
    storage_set(CM_REENTRANCY_KEY, &[1u8]);
    true
}
fn reentrancy_exit() {
    storage_set(CM_REENTRANCY_KEY, &[0u8]);
}

// ============================================================================
// JOB STATES
// ============================================================================

const JOB_PENDING: u8 = 0;
const JOB_CLAIMED: u8 = 1;
const JOB_COMPLETED: u8 = 2;
const JOB_DISPUTED: u8 = 3;
const JOB_CANCELLED: u8 = 4;
const JOB_RESOLVED: u8 = 5;
const JOB_RELEASED: u8 = 6;

// ============================================================================
// v2 CONSTANTS
// ============================================================================

/// Default slots a provider has to claim a pending job before requester can cancel
const DEFAULT_CLAIM_TIMEOUT: u64 = 200;
/// Default slots a provider has to complete after claiming
const DEFAULT_COMPLETE_TIMEOUT: u64 = 1000;
/// Default slots after completion before payment auto-releases
const DEFAULT_CHALLENGE_PERIOD: u64 = 100;

const ADMIN_KEY: &[u8] = b"cm_admin";
const CLAIM_TIMEOUT_KEY: &[u8] = b"claim_timeout";
const COMPLETE_TIMEOUT_KEY: &[u8] = b"complete_timeout";
const CHALLENGE_PERIOD_KEY: &[u8] = b"challenge_period";

const CM_COMPLETED_COUNT_KEY: &[u8] = b"cm_completed_count";
const CM_PAYMENT_VOLUME_KEY: &[u8] = b"cm_payment_volume";
const CM_DISPUTE_COUNT_KEY: &[u8] = b"cm_dispute_count";
const CM_TOKEN_ADDRESS_KEY: &[u8] = b"cm_token_address";

const CM_AGENT_PAYMENTS_ENABLED_KEY: &[u8] = b"cm_agent_pay_enabled";
const CM_AGENT_ROUTE_PAUSED_KEY: &[u8] = b"cm_agent_route_paused";
const CM_AGENT_MAX_DAILY_CAP_KEY: &[u8] = b"cm_agent_max_daily";
const CM_AGENT_MAX_PER_TASK_CAP_KEY: &[u8] = b"cm_agent_max_task";
const CM_AGENT_POLICY_COUNT_KEY: &[u8] = b"cm_agent_policy_count";
const CM_AGENT_PAYMENT_COUNT_KEY: &[u8] = b"cm_agent_pay_count";
const CM_AGENT_PAYMENT_VOLUME_KEY: &[u8] = b"cm_agent_pay_volume";
const CM_AGENT_BLOCKED_PAYMENT_COUNT_KEY: &[u8] = b"cm_agent_block_count";
const AGENT_SPEND_WINDOW_SLOTS: u64 = 216_000;
const AGENT_POLICY_SIZE: usize = 73;

// ============================================================================
// STORAGE KEY HELPERS
// ============================================================================

fn hex_encode(bytes: &[u8]) -> Vec<u8> {
    let hex_chars: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(hex_chars[(b >> 4) as usize]);
        out.push(hex_chars[(b & 0x0f) as usize]);
    }
    out
}

fn provider_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(9 + 64);
    key.extend_from_slice(b"provider_");
    key.extend_from_slice(&hex_encode(addr));
    key
}

fn job_key(job_id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(4 + 20);
    key.extend_from_slice(b"job_");
    let s = u64_to_decimal(job_id);
    key.extend_from_slice(&s);
    key
}

fn u64_to_decimal(mut n: u64) -> Vec<u8> {
    if n == 0 {
        return Vec::from(*b"0");
    }
    let mut buf = Vec::new();
    while n > 0 {
        buf.push(b'0' + (n % 10) as u8);
        n /= 10;
    }
    buf.reverse();
    buf
}

// v2 key helpers

fn escrow_key(job_id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(7 + 20);
    key.extend_from_slice(b"escrow_");
    key.extend_from_slice(&u64_to_decimal(job_id));
    key
}

fn arbitrator_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(12 + 64);
    key.extend_from_slice(b"arbitrator_");
    key.extend_from_slice(&hex_encode(addr));
    key
}

fn is_admin(caller: &[u8]) -> bool {
    match storage_get(ADMIN_KEY) {
        Some(data) => data.as_slice() == caller,
        None => false,
    }
}

/// Load the configured payment token address, or None if not set.
/// The zero address is a valid stored value and represents native LICN.
fn load_token_address() -> Option<[u8; 32]> {
    storage_get(CM_TOKEN_ADDRESS_KEY).and_then(|bytes| {
        if bytes.len() == 32 {
            let mut addr = [0u8; 32];
            addr.copy_from_slice(&bytes);
            Some(addr)
        } else {
            None
        }
    })
}

fn load_configured_address(key: &[u8]) -> Option<[u8; 32]> {
    if let Some(bytes) = storage_get(key) {
        if bytes.len() == 32 {
            let mut addr = [0u8; 32];
            addr.copy_from_slice(&bytes);
            if addr.iter().any(|&b| b != 0) {
                return Some(addr);
            }
        }
    }
    None
}

fn is_arbitrator(addr: &[u8; 32]) -> bool {
    let ak = arbitrator_key(addr);
    match storage_get(&ak) {
        Some(data) => !data.is_empty() && data[0] == 1,
        None => false,
    }
}

fn get_claim_timeout() -> u64 {
    storage_get(CLAIM_TIMEOUT_KEY)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_CLAIM_TIMEOUT)
}

fn get_complete_timeout() -> u64 {
    storage_get(COMPLETE_TIMEOUT_KEY)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_COMPLETE_TIMEOUT)
}

fn get_challenge_period() -> u64 {
    storage_get(CHALLENGE_PERIOD_KEY)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_CHALLENGE_PERIOD)
}

fn read_address32(ptr: *const u8) -> Option<[u8; 32]> {
    if ptr.is_null() {
        return None;
    }
    let mut out = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(ptr, out.as_mut_ptr(), 32) };
    Some(out)
}

fn stored_u64(key: &[u8]) -> u64 {
    storage_get(key)
        .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
        .unwrap_or(0)
}

fn increment_counter_saturating(key: &[u8]) {
    let current = stored_u64(key);
    storage_set(key, &u64_to_bytes(current.saturating_add(1)));
}

fn unpaid_payout_key(token: Address, recipient: Address) -> Vec<u8> {
    let mut key = b"unpaid_payout:".to_vec();
    key.extend_from_slice(&token.0);
    key.push(b':');
    key.extend_from_slice(&recipient.0);
    key
}

fn record_unpaid_payout(token: Address, recipient: Address, amount: u64) {
    if amount == 0 {
        return;
    }
    let key = unpaid_payout_key(token, recipient);
    let current = stored_u64(&key);
    storage_set(&key, &u64_to_bytes(current.saturating_add(amount)));
}

fn signer_matches(addr: &[u8; 32]) -> bool {
    get_caller().0 == *addr
}

fn stored_bool(key: &[u8]) -> bool {
    storage_get(key)
        .map(|data| data.first().copied().unwrap_or(0) != 0)
        .unwrap_or(false)
}

fn cm_paused() -> bool {
    stored_bool(b"cm_paused")
}

fn nonzero_hash(hash: &[u8; 32]) -> bool {
    hash.iter().any(|&byte| byte != 0)
}

fn increment_counter(key: &[u8]) {
    increment_counter_saturating(key);
}

fn agent_policy_key(agent: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(13 + 64);
    key.extend_from_slice(b"agent_policy:");
    key.extend_from_slice(&hex_encode(agent));
    key
}

fn agent_spend_key(agent: &[u8; 32], window: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(13 + 64 + 20);
    key.extend_from_slice(b"agent_spent:");
    key.extend_from_slice(&hex_encode(agent));
    key.push(b':');
    key.extend_from_slice(&u64_to_decimal(window));
    key
}

fn agent_job_action_key(job_id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(17 + 20);
    key.extend_from_slice(b"agent_job_action:");
    key.extend_from_slice(&u64_to_decimal(job_id));
    key
}

fn current_agent_spend_window() -> u64 {
    get_slot() / AGENT_SPEND_WINDOW_SLOTS
}

fn encode_agent_policy(
    policy_version: u64,
    daily_cap: u64,
    per_task_cap: u64,
    policy_hash: &[u8; 32],
    created_slot: u64,
    updated_slot: u64,
    active: bool,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(AGENT_POLICY_SIZE);
    data.extend_from_slice(&u64_to_bytes(policy_version));
    data.extend_from_slice(&u64_to_bytes(daily_cap));
    data.extend_from_slice(&u64_to_bytes(per_task_cap));
    data.extend_from_slice(policy_hash);
    data.extend_from_slice(&u64_to_bytes(created_slot));
    data.extend_from_slice(&u64_to_bytes(updated_slot));
    data.push(if active { 1 } else { 0 });
    data
}

fn read_agent_policy(agent: &[u8; 32]) -> Option<Vec<u8>> {
    storage_get(&agent_policy_key(agent)).filter(|data| data.len() >= AGENT_POLICY_SIZE)
}

fn create_escrowed_job(
    req_arr: &[u8; 32],
    compute_units_needed: u64,
    max_price: u64,
    hash_arr: &[u8; 32],
) -> Result<u64, u32> {
    if compute_units_needed == 0 {
        log_info("Compute units must be > 0");
        return Err(1);
    }
    if max_price == 0 {
        log_info("Max price must be > 0");
        return Err(11);
    }

    if !check_identity_gate(req_arr) {
        log_info("Insufficient LichenID reputation for job submission");
        return Err(10);
    }

    let job_id = stored_u64(b"job_count");
    let next_job_id = match job_id.checked_add(1) {
        Some(next) => next,
        None => {
            log_info("Job count overflow");
            return Err(14);
        }
    };

    let token_addr = match load_token_address() {
        Some(a) => a,
        None => {
            log_info("Payment token not configured — admin must call set_token_address");
            return Err(12);
        }
    };
    let contract_addr = get_contract_address();
    match receive_token_or_native(
        Address(token_addr),
        Address(*req_arr),
        contract_addr,
        max_price,
    ) {
        Ok(true) => {}
        Ok(false) => {
            log_info("Token transfer returned false — requester escrow not collected");
            return Err(13);
        }
        Err(_) => {
            log_info("Token transfer failed — requester has insufficient balance");
            return Err(13);
        }
    }

    storage_set(b"job_count", &u64_to_bytes(next_job_id));

    let current_slot = get_slot();
    let data = encode_job(
        req_arr,
        compute_units_needed,
        max_price,
        hash_arr,
        JOB_PENDING,
        &[0u8; 32],
        &[0u8; 32],
        current_slot,
        0,
    );

    let jk = job_key(job_id);
    storage_set(&jk, &data);

    let ek = escrow_key(job_id);
    storage_set(&ek, &u64_to_bytes(max_price));

    Ok(job_id)
}

// ============================================================================
// PROVIDER LAYOUT
// ============================================================================
//
// Bytes 0..32  : address
// Bytes 32..40 : compute_units_available (u64 LE)
// Bytes 40..48 : price_per_unit (u64 LE)
// Bytes 48..56 : jobs_completed (u64 LE)
// Byte  56     : active (u8)
// Bytes 57..65 : registered_slot (u64 LE)

const PROVIDER_SIZE: usize = 65;

fn encode_provider(
    addr: &[u8; 32],
    units: u64,
    price: u64,
    completed: u64,
    active: bool,
    reg_slot: u64,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(PROVIDER_SIZE);
    data.extend_from_slice(addr);
    data.extend_from_slice(&u64_to_bytes(units));
    data.extend_from_slice(&u64_to_bytes(price));
    data.extend_from_slice(&u64_to_bytes(completed));
    data.push(if active { 1 } else { 0 });
    data.extend_from_slice(&u64_to_bytes(reg_slot));
    data
}

// ============================================================================
// JOB LAYOUT
// ============================================================================
//
// Bytes 0..32   : requester (address)
// Bytes 32..40  : compute_units_needed (u64 LE)
// Bytes 40..48  : max_price (u64 LE)
// Bytes 48..80  : code_hash (32 bytes)
// Byte  80      : status (u8)
// Bytes 81..113 : provider (32 bytes, zero if unclaimed)
// Bytes 113..145: result_hash (32 bytes, zero if not submitted)
// Bytes 145..153: created_slot (u64 LE)
// Bytes 153..161: completed_slot (u64 LE, zero if not completed)

const JOB_SIZE: usize = 161;

fn encode_job(
    requester: &[u8; 32],
    compute_units: u64,
    max_price: u64,
    code_hash: &[u8; 32],
    status: u8,
    provider: &[u8; 32],
    result_hash: &[u8; 32],
    created_slot: u64,
    completed_slot: u64,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(JOB_SIZE);
    data.extend_from_slice(requester);
    data.extend_from_slice(&u64_to_bytes(compute_units));
    data.extend_from_slice(&u64_to_bytes(max_price));
    data.extend_from_slice(code_hash);
    data.push(status);
    data.extend_from_slice(provider);
    data.extend_from_slice(result_hash);
    data.extend_from_slice(&u64_to_bytes(created_slot));
    data.extend_from_slice(&u64_to_bytes(completed_slot));
    data
}

// ============================================================================
// REGISTER PROVIDER
// ============================================================================

/// Register as a compute provider.
///
/// Parameters:
///   - provider_ptr: 32-byte provider address
///   - compute_units_available: number of compute units offered
///   - price_per_unit: price per unit in spores
#[no_mangle]
pub extern "C" fn register_provider(
    provider_ptr: *const u8,
    compute_units_available: u64,
    price_per_unit: u64,
) -> u32 {
    log_info("Registering compute provider...");

    // SECURITY FIX: Check if contract is paused
    if cm_paused() {
        return 99;
    }

    let addr = match read_address32(provider_ptr) {
        Some(v) => v,
        None => {
            log_info("register_provider rejected: null provider_ptr");
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != addr {
        return 200;
    }

    if compute_units_available == 0 {
        log_info("Compute units must be > 0");
        return 1;
    }
    if price_per_unit == 0 {
        log_info("Price per unit must be > 0");
        return 2;
    }

    // LichenID reputation gate
    if !check_identity_gate(&addr) {
        log_info("Insufficient LichenID reputation for provider registration");
        return 10;
    }

    let pk = provider_key(&addr);
    if storage_get(&pk).is_some() {
        log_info("Provider already registered");
        return 3;
    }

    let current_slot = get_slot();
    let data = encode_provider(
        &addr,
        compute_units_available,
        price_per_unit,
        0,
        true,
        current_slot,
    );
    storage_set(&pk, &data);

    log_info("Compute provider registered");
    0
}

// ============================================================================
// SUBMIT JOB
// ============================================================================

/// Submit a compute job.
///
/// Parameters:
///   - requester_ptr: 32-byte requester address
///   - compute_units_needed: units required
///   - max_price: maximum price willing to pay (spores) — escrowed
///   - code_hash_ptr: 32-byte hash of the computation code
///
/// Returns 0 on success, job_id in return data.
#[no_mangle]
pub extern "C" fn submit_job(
    requester_ptr: *const u8,
    compute_units_needed: u64,
    max_price: u64,
    code_hash_ptr: *const u8,
) -> u32 {
    log_info("Submitting compute job...");

    // SECURITY FIX: Check if contract is paused
    let paused = storage_get(b"cm_paused").unwrap_or_default();
    if paused.len() > 0 && paused[0] == 1 {
        return 99;
    }

    let req_arr = match read_address32(requester_ptr) {
        Some(v) => v,
        None => {
            log_info("submit_job rejected: null requester_ptr");
            return 98;
        }
    };
    let hash_arr = match read_address32(code_hash_ptr) {
        Some(v) => v,
        None => {
            log_info("submit_job rejected: null code_hash_ptr");
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != req_arr {
        return 200;
    }

    match create_escrowed_job(&req_arr, compute_units_needed, max_price, &hash_arr) {
        Ok(job_id) => {
            lichen_sdk::set_return_data(&u64_to_bytes(job_id));
            log_info("Compute job submitted, payment escrowed");
            0
        }
        Err(code) => code,
    }
}

// ============================================================================
// CLAIM JOB
// ============================================================================

/// Provider claims a pending job.
///
/// Parameters:
///   - provider_ptr: 32-byte provider address
///   - job_id: the job to claim
#[no_mangle]
pub extern "C" fn claim_job(provider_ptr: *const u8, job_id: u64) -> u32 {
    log_info("Claiming compute job...");

    // SECURITY FIX: Check if contract is paused
    let paused = storage_get(b"cm_paused").unwrap_or_default();
    if paused.len() > 0 && paused[0] == 1 {
        return 99;
    }

    let prov_arr = match read_address32(provider_ptr) {
        Some(v) => v,
        None => {
            log_info("claim_job rejected: null provider_ptr");
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != prov_arr {
        return 200;
    }

    // Check provider is registered
    let pk = provider_key(&prov_arr);
    let prov_data = match storage_get(&pk) {
        Some(data) => data,
        None => {
            log_info("Provider not registered");
            return 1;
        }
    };
    if prov_data.len() < PROVIDER_SIZE {
        log_info("Corrupt provider data");
        return 5;
    }
    if prov_data[56] == 0 {
        log_info("Provider inactive");
        return 6;
    }

    // Load job
    let jk = job_key(job_id);
    let mut job_data = match storage_get(&jk) {
        Some(data) => data,
        None => {
            log_info("Job not found");
            return 2;
        }
    };

    if job_data.len() < JOB_SIZE {
        log_info("Corrupt job data");
        return 3;
    }

    if job_data[80] != JOB_PENDING {
        log_info("Job is not in pending state");
        return 4;
    }

    // Set provider and status = claimed
    job_data[80] = JOB_CLAIMED;
    job_data[81..113].copy_from_slice(&prov_arr);
    storage_set(&jk, &job_data);

    log_info("Job claimed");
    0
}

// ============================================================================
// COMPLETE JOB
// ============================================================================

/// Provider submits result for a claimed job.
///
/// Parameters:
///   - provider_ptr: 32-byte provider address
///   - job_id: the job to complete
///   - result_hash_ptr: 32-byte hash of the computation result
#[no_mangle]
pub extern "C" fn complete_job(
    provider_ptr: *const u8,
    job_id: u64,
    result_hash_ptr: *const u8,
) -> u32 {
    log_info("Completing compute job...");

    // SECURITY FIX: Check if contract is paused
    let paused = storage_get(b"cm_paused").unwrap_or_default();
    if paused.len() > 0 && paused[0] == 1 {
        return 99;
    }

    let prov_arr = match read_address32(provider_ptr) {
        Some(v) => v,
        None => {
            log_info("complete_job rejected: null provider_ptr");
            return 98;
        }
    };
    let result_hash = match read_address32(result_hash_ptr) {
        Some(v) => v,
        None => {
            log_info("complete_job rejected: null result_hash_ptr");
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != prov_arr {
        return 200;
    }

    let jk = job_key(job_id);
    let mut job_data = match storage_get(&jk) {
        Some(data) => data,
        None => {
            log_info("Job not found");
            return 1;
        }
    };

    if job_data.len() < JOB_SIZE {
        return 2;
    }

    if job_data[80] != JOB_CLAIMED {
        log_info("Job is not in claimed state");
        return 3;
    }

    // Verify provider matches
    if &job_data[81..113] != &prov_arr[..] {
        log_info("Not the assigned provider");
        return 4;
    }

    // Set result and status = completed
    job_data[80] = JOB_COMPLETED;
    job_data[113..145].copy_from_slice(&result_hash);
    let current_slot = get_slot();
    job_data[153..161].copy_from_slice(&u64_to_bytes(current_slot));
    storage_set(&jk, &job_data);

    // Update provider stats
    let pk = provider_key(&prov_arr);
    if let Some(mut prov_data) = storage_get(&pk) {
        if prov_data.len() >= PROVIDER_SIZE {
            let completed = bytes_to_u64(&prov_data[48..56]);
            prov_data[48..56].copy_from_slice(&u64_to_bytes(completed.saturating_add(1)));
            storage_set(&pk, &prov_data);
        }
    }

    log_info("Job completed");
    0
}

// ============================================================================
// DISPUTE JOB
// ============================================================================

/// Requester disputes a completed job result.
///
/// Parameters:
///   - requester_ptr: 32-byte requester address
///   - job_id: the job to dispute
#[no_mangle]
pub extern "C" fn dispute_job(requester_ptr: *const u8, job_id: u64) -> u32 {
    log_info("Disputing compute job...");

    let requester = match read_address32(requester_ptr) {
        Some(v) => v,
        None => {
            log_info("dispute_job rejected: null requester_ptr");
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != requester {
        return 200;
    }

    let jk = job_key(job_id);
    let mut job_data = match storage_get(&jk) {
        Some(data) => data,
        None => {
            log_info("Job not found");
            return 1;
        }
    };

    if job_data.len() < JOB_SIZE {
        return 2;
    }

    // Only requester can dispute
    if &job_data[0..32] != &requester[..] {
        log_info("Only requester can dispute");
        return 3;
    }

    if job_data[80] != JOB_COMPLETED {
        log_info("Job must be completed to dispute");
        return 4;
    }

    job_data[80] = JOB_DISPUTED;
    storage_set(&jk, &job_data);

    increment_counter_saturating(CM_DISPUTE_COUNT_KEY);

    log_info("Job disputed");
    0
}

// ============================================================================
// GET JOB
// ============================================================================

/// Query job information.
///
/// Parameters:
///   - job_id: the job ID to query
///
/// Returns 0 on success (job data as return data), 1 if not found.
#[no_mangle]
pub extern "C" fn get_job(job_id: u64) -> u32 {
    let jk = job_key(job_id);
    match storage_get(&jk) {
        Some(data) => {
            lichen_sdk::set_return_data(&data);
            0
        }
        None => {
            log_info("Job not found");
            1
        }
    }
}

// ============================================================================
// v2: ADMIN / ARBITRATOR MANAGEMENT
// ============================================================================

/// Initialize the compute market admin. Only callable once.
#[no_mangle]
pub extern "C" fn initialize(admin_ptr: *const u8) -> u32 {
    let admin = match read_address32(admin_ptr) {
        Some(v) => v,
        None => {
            log_info("initialize rejected: null admin_ptr");
            return 98;
        }
    };
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != admin {
        return 200;
    }
    if storage_get(ADMIN_KEY).is_some() {
        log_info("Admin already set");
        return 1;
    }
    storage_set(ADMIN_KEY, &admin);
    log_info("Compute market admin initialized");
    0
}

/// Admin sets claim timeout (slots a provider has to claim a pending job).
#[no_mangle]
pub extern "C" fn set_claim_timeout(caller_ptr: *const u8, timeout: u64) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => {
            log_info("set_claim_timeout rejected: null caller_ptr");
            return 98;
        }
    };
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }
    if !is_admin(&caller) {
        log_info("Not admin");
        return 1;
    }
    if timeout == 0 {
        return 2;
    }
    storage_set(CLAIM_TIMEOUT_KEY, &u64_to_bytes(timeout));
    log_info("Claim timeout updated");
    0
}

/// Admin sets complete timeout (slots after claiming to deliver result).
#[no_mangle]
pub extern "C" fn set_complete_timeout(caller_ptr: *const u8, timeout: u64) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => {
            log_info("set_complete_timeout rejected: null caller_ptr");
            return 98;
        }
    };
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }
    if !is_admin(&caller) {
        log_info("Not admin");
        return 1;
    }
    if timeout == 0 {
        return 2;
    }
    storage_set(COMPLETE_TIMEOUT_KEY, &u64_to_bytes(timeout));
    log_info("Complete timeout updated");
    0
}

/// Admin sets challenge period (slots after completion before payment releases).
#[no_mangle]
pub extern "C" fn set_challenge_period(caller_ptr: *const u8, period: u64) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => {
            log_info("set_challenge_period rejected: null caller_ptr");
            return 98;
        }
    };
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }
    if !is_admin(&caller) {
        log_info("Not admin");
        return 1;
    }
    if period == 0 {
        return 2;
    }
    storage_set(CHALLENGE_PERIOD_KEY, &u64_to_bytes(period));
    log_info("Challenge period updated");
    0
}

/// Admin adds an arbitrator who can resolve disputes.
#[no_mangle]
pub extern "C" fn add_arbitrator(caller_ptr: *const u8, arbitrator_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => {
            log_info("add_arbitrator rejected: null caller_ptr");
            return 98;
        }
    };
    let addr = match read_address32(arbitrator_ptr) {
        Some(v) => v,
        None => {
            log_info("add_arbitrator rejected: null arbitrator_ptr");
            return 98;
        }
    };
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }
    if !is_admin(&caller) {
        log_info("Not admin");
        return 1;
    }
    let ak = arbitrator_key(&addr);
    storage_set(&ak, &[1]);
    log_info("Arbitrator added");
    0
}

/// Admin removes an arbitrator.
#[no_mangle]
pub extern "C" fn remove_arbitrator(caller_ptr: *const u8, arbitrator_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => {
            log_info("remove_arbitrator rejected: null caller_ptr");
            return 98;
        }
    };
    let addr = match read_address32(arbitrator_ptr) {
        Some(v) => v,
        None => {
            log_info("remove_arbitrator rejected: null arbitrator_ptr");
            return 98;
        }
    };
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }
    if !is_admin(&caller) {
        log_info("Not admin");
        return 1;
    }
    let ak = arbitrator_key(&addr);
    storage_set(&ak, &[0]);
    log_info("Arbitrator removed");
    0
}

// ============================================================================
// AUDIT-FIX H-4: Admin configurable payment token address
// ============================================================================

/// Admin sets the payment token address used for escrow transfers.
/// Zero address = native LICN.
#[no_mangle]
pub extern "C" fn set_token_address(caller_ptr: *const u8, token_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => {
            log_info("set_token_address rejected: null caller_ptr");
            return 98;
        }
    };
    let token = match read_address32(token_ptr) {
        Some(v) => v,
        None => {
            log_info("set_token_address rejected: null token_ptr");
            return 98;
        }
    };
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }
    if !is_admin(&caller) {
        log_info("Not admin");
        return 1;
    }
    if load_token_address().is_some() {
        log_info("Payment token address already configured");
        return 3;
    }
    storage_set(CM_TOKEN_ADDRESS_KEY, &token);
    log_info("Payment token address set");
    0
}

// ============================================================================
// NX-980: AGENT COMPUTE SPENDING POLICY
// ============================================================================

/// Admin configures the global agent-compute payment controls.
///
/// `enabled` and `route_paused` are 0/1 flags. When disabled or paused, only
/// existing normal compute-market flows remain available; the agent-specific
/// submit path rejects new payments before escrow is collected.
#[no_mangle]
pub extern "C" fn set_agent_compute_controls(
    caller_ptr: *const u8,
    enabled: u64,
    route_paused: u64,
    max_daily_cap: u64,
    max_per_task_cap: u64,
) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => return 98,
    };
    if !signer_matches(&caller) {
        return 200;
    }
    if !is_admin(&caller) {
        return 1;
    }
    if enabled > 1 || route_paused > 1 {
        return 2;
    }
    if enabled == 1 {
        if max_daily_cap == 0 || max_per_task_cap == 0 {
            return 3;
        }
        if max_per_task_cap > max_daily_cap {
            return 4;
        }
    }

    storage_set(CM_AGENT_PAYMENTS_ENABLED_KEY, &[enabled as u8]);
    storage_set(CM_AGENT_ROUTE_PAUSED_KEY, &[route_paused as u8]);
    storage_set(CM_AGENT_MAX_DAILY_CAP_KEY, &u64_to_bytes(max_daily_cap));
    storage_set(
        CM_AGENT_MAX_PER_TASK_CAP_KEY,
        &u64_to_bytes(max_per_task_cap),
    );
    log_info("Agent compute controls configured");
    0
}

/// Agent wallet opts into a bounded spending policy.
///
/// The policy hash must be a non-zero 32-byte hash of the off-chain disclosure,
/// PQ signer set, task-accounting rules, and allowed asset/route statement.
#[no_mangle]
pub extern "C" fn set_agent_spending_policy(
    agent_ptr: *const u8,
    daily_cap: u64,
    per_task_cap: u64,
    policy_hash_ptr: *const u8,
    policy_version: u64,
) -> u32 {
    let agent = match read_address32(agent_ptr) {
        Some(v) => v,
        None => return 98,
    };
    let policy_hash = match read_address32(policy_hash_ptr) {
        Some(v) => v,
        None => return 98,
    };
    if !signer_matches(&agent) {
        return 200;
    }
    if policy_version == 0 || daily_cap == 0 || per_task_cap == 0 {
        return 1;
    }
    if per_task_cap > daily_cap {
        return 2;
    }
    if !nonzero_hash(&policy_hash) {
        return 3;
    }

    let key = agent_policy_key(&agent);
    let existed = storage_get(&key).is_some();
    let created_slot = storage_get(&key)
        .filter(|data| data.len() >= AGENT_POLICY_SIZE)
        .map(|data| bytes_to_u64(&data[56..64]))
        .unwrap_or_else(get_slot);
    let current_slot = get_slot();
    storage_set(
        &key,
        &encode_agent_policy(
            policy_version,
            daily_cap,
            per_task_cap,
            &policy_hash,
            created_slot,
            current_slot,
            true,
        ),
    );
    if !existed {
        increment_counter(CM_AGENT_POLICY_COUNT_KEY);
    }
    log_info("Agent spending policy configured");
    0
}

/// Agent wallet disables its spending policy. This remains available while the
/// market or Neo agent route is paused because it only narrows permissions.
#[no_mangle]
pub extern "C" fn disable_agent_spending_policy(agent_ptr: *const u8) -> u32 {
    let agent = match read_address32(agent_ptr) {
        Some(v) => v,
        None => return 98,
    };
    if !signer_matches(&agent) {
        return 200;
    }
    let key = agent_policy_key(&agent);
    let mut data = match storage_get(&key) {
        Some(data) if data.len() >= AGENT_POLICY_SIZE => data,
        _ => return 1,
    };
    data[72] = 0;
    data[64..72].copy_from_slice(&u64_to_bytes(get_slot()));
    storage_set(&key, &data);
    log_info("Agent spending policy disabled");
    0
}

/// Submit a compute job through the agent-specific NX-980 policy path.
///
/// This records a non-zero action hash for the PQ-attested agent action and
/// enforces per-task plus per-window spend limits before escrow collection.
#[no_mangle]
pub extern "C" fn submit_agent_job(
    agent_ptr: *const u8,
    compute_units_needed: u64,
    max_price: u64,
    code_hash_ptr: *const u8,
    action_hash_ptr: *const u8,
) -> u32 {
    log_info("Submitting agent compute job...");

    if cm_paused() {
        return 99;
    }
    if !stored_bool(CM_AGENT_PAYMENTS_ENABLED_KEY) {
        increment_counter(CM_AGENT_BLOCKED_PAYMENT_COUNT_KEY);
        return 40;
    }
    if stored_bool(CM_AGENT_ROUTE_PAUSED_KEY) {
        increment_counter(CM_AGENT_BLOCKED_PAYMENT_COUNT_KEY);
        return 41;
    }

    let agent = match read_address32(agent_ptr) {
        Some(v) => v,
        None => return 98,
    };
    let code_hash = match read_address32(code_hash_ptr) {
        Some(v) => v,
        None => return 98,
    };
    let action_hash = match read_address32(action_hash_ptr) {
        Some(v) => v,
        None => return 98,
    };
    if !signer_matches(&agent) {
        return 200;
    }
    if !nonzero_hash(&action_hash) {
        increment_counter(CM_AGENT_BLOCKED_PAYMENT_COUNT_KEY);
        return 42;
    }

    let policy = match read_agent_policy(&agent) {
        Some(data) if data[72] == 1 => data,
        _ => {
            increment_counter(CM_AGENT_BLOCKED_PAYMENT_COUNT_KEY);
            return 43;
        }
    };
    let daily_cap = bytes_to_u64(&policy[8..16]);
    let per_task_cap = bytes_to_u64(&policy[16..24]);
    if max_price > per_task_cap {
        increment_counter(CM_AGENT_BLOCKED_PAYMENT_COUNT_KEY);
        return 44;
    }

    let global_daily_cap = stored_u64(CM_AGENT_MAX_DAILY_CAP_KEY);
    let global_per_task_cap = stored_u64(CM_AGENT_MAX_PER_TASK_CAP_KEY);
    if global_per_task_cap > 0 && max_price > global_per_task_cap {
        increment_counter(CM_AGENT_BLOCKED_PAYMENT_COUNT_KEY);
        return 45;
    }

    let window = current_agent_spend_window();
    let spend_key = agent_spend_key(&agent, window);
    let spent = stored_u64(&spend_key);
    let next_spent = match spent.checked_add(max_price) {
        Some(value) => value,
        None => {
            increment_counter(CM_AGENT_BLOCKED_PAYMENT_COUNT_KEY);
            return 46;
        }
    };
    if next_spent > daily_cap {
        increment_counter(CM_AGENT_BLOCKED_PAYMENT_COUNT_KEY);
        return 47;
    }
    if global_daily_cap > 0 && next_spent > global_daily_cap {
        increment_counter(CM_AGENT_BLOCKED_PAYMENT_COUNT_KEY);
        return 48;
    }

    match create_escrowed_job(&agent, compute_units_needed, max_price, &code_hash) {
        Ok(job_id) => {
            storage_set(&spend_key, &u64_to_bytes(next_spent));
            storage_set(&agent_job_action_key(job_id), &action_hash);
            increment_counter(CM_AGENT_PAYMENT_COUNT_KEY);
            let volume = stored_u64(CM_AGENT_PAYMENT_VOLUME_KEY);
            storage_set(
                CM_AGENT_PAYMENT_VOLUME_KEY,
                &u64_to_bytes(volume.saturating_add(max_price)),
            );
            lichen_sdk::set_return_data(&u64_to_bytes(job_id));
            log_info("Agent compute job submitted, policy spend recorded");
            0
        }
        Err(code) => code,
    }
}

/// Return global agent-compute controls and counters.
///
/// Layout: enabled(1), route_paused(1), max_daily_cap(u64),
/// max_per_task_cap(u64), policy_count(u64), payment_count(u64),
/// payment_volume(u64), blocked_payment_count(u64).
#[no_mangle]
pub extern "C" fn get_agent_compute_controls() -> u32 {
    let mut buf = Vec::with_capacity(50);
    buf.push(if stored_bool(CM_AGENT_PAYMENTS_ENABLED_KEY) {
        1
    } else {
        0
    });
    buf.push(if stored_bool(CM_AGENT_ROUTE_PAUSED_KEY) {
        1
    } else {
        0
    });
    buf.extend_from_slice(&u64_to_bytes(stored_u64(CM_AGENT_MAX_DAILY_CAP_KEY)));
    buf.extend_from_slice(&u64_to_bytes(stored_u64(CM_AGENT_MAX_PER_TASK_CAP_KEY)));
    buf.extend_from_slice(&u64_to_bytes(stored_u64(CM_AGENT_POLICY_COUNT_KEY)));
    buf.extend_from_slice(&u64_to_bytes(stored_u64(CM_AGENT_PAYMENT_COUNT_KEY)));
    buf.extend_from_slice(&u64_to_bytes(stored_u64(CM_AGENT_PAYMENT_VOLUME_KEY)));
    buf.extend_from_slice(&u64_to_bytes(stored_u64(
        CM_AGENT_BLOCKED_PAYMENT_COUNT_KEY,
    )));
    lichen_sdk::set_return_data(&buf);
    0
}

#[no_mangle]
pub extern "C" fn get_agent_spending_policy(agent_ptr: *const u8) -> u32 {
    let agent = match read_address32(agent_ptr) {
        Some(v) => v,
        None => return 98,
    };
    match read_agent_policy(&agent) {
        Some(data) => {
            lichen_sdk::set_return_data(&data);
            0
        }
        None => 1,
    }
}

#[no_mangle]
pub extern "C" fn get_agent_spend_window(agent_ptr: *const u8, window: u64) -> u32 {
    let agent = match read_address32(agent_ptr) {
        Some(v) => v,
        None => return 98,
    };
    lichen_sdk::set_return_data(&u64_to_bytes(stored_u64(&agent_spend_key(&agent, window))));
    0
}

#[no_mangle]
pub extern "C" fn get_agent_job_action(job_id: u64) -> u32 {
    match storage_get(&agent_job_action_key(job_id)) {
        Some(data) => {
            lichen_sdk::set_return_data(&data);
            0
        }
        None => 1,
    }
}

// ============================================================================
// v2: JOB CANCELLATION
// ============================================================================

/// Requester cancels a job.
///
/// - Pending jobs: cancel any time after claim_timeout has passed
/// - Claimed jobs: cancel if complete_timeout has passed (provider failed to deliver)
///
/// Escrowed funds returned to requester.
#[no_mangle]
pub extern "C" fn cancel_job(requester_ptr: *const u8, job_id: u64) -> u32 {
    log_info("Cancelling compute job...");

    let requester = match read_address32(requester_ptr) {
        Some(v) => v,
        None => {
            log_info("cancel_job rejected: null requester_ptr");
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != requester {
        return 200;
    }

    let jk = job_key(job_id);
    let mut job_data = match storage_get(&jk) {
        Some(data) => data,
        None => {
            log_info("Job not found");
            return 1;
        }
    };
    if job_data.len() < JOB_SIZE {
        return 2;
    }

    // Only requester can cancel
    if &job_data[0..32] != &requester[..] {
        log_info("Only requester can cancel");
        return 3;
    }

    let status = job_data[80];
    let created_slot = bytes_to_u64(&job_data[145..153]);
    let current_slot = get_slot();

    match status {
        JOB_PENDING => {
            // Must wait for claim timeout to give providers a chance
            let timeout = get_claim_timeout();
            if current_slot < created_slot.saturating_add(timeout) {
                log_info("Claim timeout not yet expired — providers still have time");
                return 4;
            }
        }
        JOB_CLAIMED => {
            // Provider claimed but never completed — check complete timeout
            let timeout = get_complete_timeout();
            if current_slot < created_slot.saturating_add(timeout) {
                log_info("Complete timeout not yet expired");
                return 5;
            }
        }
        _ => {
            log_info("Job cannot be cancelled in current state");
            return 6;
        }
    }

    // Cancel and clear escrow. If the external refund fails, restore accounting
    // so the requester can retry without losing their escrow claim.
    let previous_status = job_data[80];
    job_data[80] = JOB_CANCELLED;
    storage_set(&jk, &job_data);
    let ek = escrow_key(job_id);
    let escrowed = storage_get(&ek).map(|d| bytes_to_u64(&d)).unwrap_or(0);
    storage_set(&ek, &u64_to_bytes(0));

    // AUDIT-FIX H-2: Return escrowed tokens to requester
    if escrowed > 0 {
        let token_addr = match load_token_address() {
            Some(addr) => addr,
            None => {
                job_data[80] = previous_status;
                storage_set(&jk, &job_data);
                storage_set(&ek, &u64_to_bytes(escrowed));
                log_info("cancel_job: payment token configuration invalid");
                return 8;
            }
        };
        let contract_addr = get_contract_address();
        match transfer_token_or_native(
            Address(token_addr),
            contract_addr,
            Address(requester),
            escrowed,
        ) {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                job_data[80] = previous_status;
                storage_set(&jk, &job_data);
                storage_set(&ek, &u64_to_bytes(escrowed));
                log_info("cancel_job: token refund transfer failed");
                return 7;
            }
        }
    }

    log_info("Job cancelled, escrow refunded");
    0
}

// ============================================================================
// v2: PAYMENT RELEASE
// ============================================================================

/// Release escrowed payment to provider after challenge period expires.
///
/// Anyone can call this (permissionless finalization).
/// Requires: job is COMPLETED and challenge_period slots have passed since completed_slot.
#[no_mangle]
pub extern "C" fn release_payment(job_id: u64) -> u32 {
    log_info("Releasing payment...");

    if !reentrancy_enter() {
        return 20;
    }

    let jk = job_key(job_id);
    let mut job_data = match storage_get(&jk) {
        Some(data) => data,
        None => {
            log_info("Job not found");
            reentrancy_exit();
            return 1;
        }
    };
    if job_data.len() < JOB_SIZE {
        reentrancy_exit();
        return 2;
    }

    if job_data[80] != JOB_COMPLETED {
        log_info("Job must be in completed state");
        reentrancy_exit();
        return 3;
    }

    let completed_slot = bytes_to_u64(&job_data[153..161]);
    if completed_slot == 0 {
        log_info("No completion recorded");
        reentrancy_exit();
        return 4;
    }

    let challenge = get_challenge_period();
    let current_slot = get_slot();
    if current_slot < completed_slot.saturating_add(challenge) {
        log_info("Challenge period not yet expired");
        reentrancy_exit();
        return 5;
    }

    // Mark as released and clear escrow (payment goes to provider). On failed
    // payout, restore the completed job and escrow so release can be retried.
    job_data[80] = JOB_RELEASED;
    storage_set(&jk, &job_data);

    let ek = escrow_key(job_id);
    let escrowed = storage_get(&ek).map(|d| bytes_to_u64(&d)).unwrap_or(0);
    storage_set(&ek, &u64_to_bytes(0));

    // AUDIT-FIX H-3: Actually transfer escrowed tokens to the provider
    if escrowed > 0 {
        let mut provider_arr = [0u8; 32];
        provider_arr.copy_from_slice(&job_data[81..113]);
        let token_addr = match load_token_address() {
            Some(addr) => addr,
            None => {
                storage_set(&ek, &u64_to_bytes(escrowed));
                job_data[80] = JOB_COMPLETED;
                storage_set(&jk, &job_data);
                log_info("release_payment: payment token configuration invalid");
                reentrancy_exit();
                return 6;
            }
        };
        let contract_addr = get_contract_address();
        match transfer_token_or_native(
            Address(token_addr),
            contract_addr,
            Address(provider_arr),
            escrowed,
        ) {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                storage_set(&ek, &u64_to_bytes(escrowed));
                job_data[80] = JOB_COMPLETED;
                storage_set(&jk, &job_data);
                log_info("release_payment: token transfer to provider failed");
                reentrancy_exit();
                return 6;
            }
        }
    }

    increment_counter_saturating(CM_COMPLETED_COUNT_KEY);
    let cmv = stored_u64(CM_PAYMENT_VOLUME_KEY);
    storage_set(
        CM_PAYMENT_VOLUME_KEY,
        &u64_to_bytes(cmv.saturating_add(escrowed)),
    );

    log_info("Payment released to provider");
    reentrancy_exit();
    0
}

// ============================================================================
// v2: DISPUTE RESOLUTION
// ============================================================================

/// Arbitrator resolves a disputed job, splitting the escrow.
///
/// Parameters:
///   - arbitrator_ptr: 32-byte arbitrator address
///   - job_id: disputed job
///   - requester_pct: percentage (0-100) of escrow returned to requester
///                    remainder goes to provider
#[no_mangle]
pub extern "C" fn resolve_dispute(
    arbitrator_ptr: *const u8,
    job_id: u64,
    requester_pct: u64,
) -> u32 {
    log_info("Resolving dispute...");

    // SECURITY FIX: Check if contract is paused
    let paused = storage_get(b"cm_paused").unwrap_or_default();
    if paused.len() > 0 && paused[0] == 1 {
        return 99;
    }

    if !reentrancy_enter() {
        return 20;
    }

    let arb_arr = match read_address32(arbitrator_ptr) {
        Some(v) => v,
        None => {
            log_info("resolve_dispute rejected: null arbitrator_ptr");
            reentrancy_exit();
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != arb_arr {
        reentrancy_exit();
        return 200;
    }

    // Must be a registered arbitrator
    if !is_arbitrator(&arb_arr) {
        log_info("Not a registered arbitrator");
        reentrancy_exit();
        return 1;
    }

    if requester_pct > 100 {
        log_info("Percentage must be 0-100");
        reentrancy_exit();
        return 2;
    }

    let jk = job_key(job_id);
    let mut job_data = match storage_get(&jk) {
        Some(data) => data,
        None => {
            log_info("Job not found");
            reentrancy_exit();
            return 3;
        }
    };
    if job_data.len() < JOB_SIZE {
        reentrancy_exit();
        return 4;
    }

    if job_data[80] != JOB_DISPUTED {
        log_info("Job must be in disputed state");
        reentrancy_exit();
        return 5;
    }

    // Calculate split
    let ek = escrow_key(job_id);
    let escrowed = storage_get(&ek).map(|d| bytes_to_u64(&d)).unwrap_or(0);

    let to_requester = (escrowed as u128 * requester_pct as u128 / 100) as u64;
    let to_provider = escrowed.saturating_sub(to_requester);

    // AUDIT-FIX: Actually transfer tokens to both parties (using shared helper)
    let mut requester_arr = [0u8; 32];
    requester_arr.copy_from_slice(&job_data[0..32]);
    let mut provider_arr = [0u8; 32];
    provider_arr.copy_from_slice(&job_data[81..113]);
    let token_addr = if escrowed > 0 {
        match load_token_address() {
            Some(addr) => Some(addr),
            None => {
                log_info("resolve_dispute: payment token configuration invalid");
                reentrancy_exit();
                return 8;
            }
        }
    } else {
        None
    };

    if let Some(token_addr) = token_addr {
        let token = Address(token_addr);
        let contract_addr = get_contract_address();
        let mut paid_any = false;
        if to_requester > 0 {
            match transfer_token_or_native(
                token,
                contract_addr,
                Address(requester_arr),
                to_requester,
            ) {
                Ok(true) => {
                    paid_any = true;
                }
                Ok(false) | Err(_) => {
                    log_info("resolve_dispute: transfer to requester failed");
                    reentrancy_exit();
                    return 6;
                }
            }
        }
        if to_provider > 0 {
            match transfer_token_or_native(token, contract_addr, Address(provider_arr), to_provider)
            {
                Ok(true) => {}
                Ok(false) | Err(_) => {
                    if paid_any {
                        record_unpaid_payout(token, Address(provider_arr), to_provider);
                    } else {
                        log_info("resolve_dispute: transfer to provider failed");
                        reentrancy_exit();
                        return 7;
                    }
                }
            }
        }
    }

    // Mark resolved and clear escrow
    job_data[80] = JOB_RESOLVED;
    storage_set(&jk, &job_data);
    storage_set(&ek, &u64_to_bytes(0));

    log_info("Dispute resolved");
    reentrancy_exit();
    0
}

// ============================================================================
// v2: PROVIDER MANAGEMENT
// ============================================================================

/// Provider deactivates themselves (stops receiving new jobs).
#[no_mangle]
pub extern "C" fn deactivate_provider(provider_ptr: *const u8) -> u32 {
    let addr = match read_address32(provider_ptr) {
        Some(v) => v,
        None => {
            log_info("deactivate_provider rejected: null provider_ptr");
            return 98;
        }
    };
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != addr {
        return 200;
    }
    let pk = provider_key(&addr);
    let mut prov_data = match storage_get(&pk) {
        Some(d) => d,
        None => {
            log_info("Provider not found");
            return 1;
        }
    };
    if prov_data.len() < PROVIDER_SIZE {
        return 2;
    }
    if prov_data[56] == 0 {
        log_info("Already inactive");
        return 3;
    }
    prov_data[56] = 0;
    storage_set(&pk, &prov_data);
    log_info("Provider deactivated");
    0
}

/// Provider reactivates themselves.
#[no_mangle]
pub extern "C" fn reactivate_provider(provider_ptr: *const u8) -> u32 {
    let addr = match read_address32(provider_ptr) {
        Some(v) => v,
        None => {
            log_info("reactivate_provider rejected: null provider_ptr");
            return 98;
        }
    };
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != addr {
        return 200;
    }
    let pk = provider_key(&addr);
    let mut prov_data = match storage_get(&pk) {
        Some(d) => d,
        None => {
            log_info("Provider not found");
            return 1;
        }
    };
    if prov_data.len() < PROVIDER_SIZE {
        return 2;
    }
    if prov_data[56] == 1 {
        log_info("Already active");
        return 3;
    }
    prov_data[56] = 1;
    storage_set(&pk, &prov_data);
    log_info("Provider reactivated");
    0
}

/// Provider updates their capacity and/or pricing.
#[no_mangle]
pub extern "C" fn update_provider(
    provider_ptr: *const u8,
    compute_units: u64,
    price_per_unit: u64,
) -> u32 {
    let addr = match read_address32(provider_ptr) {
        Some(v) => v,
        None => {
            log_info("update_provider rejected: null provider_ptr");
            return 98;
        }
    };
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != addr {
        return 200;
    }
    let pk = provider_key(&addr);
    let mut prov_data = match storage_get(&pk) {
        Some(d) => d,
        None => {
            log_info("Provider not found");
            return 1;
        }
    };
    if prov_data.len() < PROVIDER_SIZE {
        return 2;
    }
    if compute_units == 0 || price_per_unit == 0 {
        log_info("Values must be > 0");
        return 3;
    }
    prov_data[32..40].copy_from_slice(&u64_to_bytes(compute_units));
    prov_data[40..48].copy_from_slice(&u64_to_bytes(price_per_unit));
    storage_set(&pk, &prov_data);
    log_info("Provider updated");
    0
}

/// Query escrow amount for a job.
#[no_mangle]
pub extern "C" fn get_escrow(job_id: u64) -> u32 {
    let ek = escrow_key(job_id);
    match storage_get(&ek) {
        Some(data) => {
            lichen_sdk::set_return_data(&data);
            0
        }
        None => 1,
    }
}

/// Claim an unpaid payout recorded after a partial dispute split.
/// This remains available while paused so recipients can exit after restrictions lift.
///
/// Returns: 0 success, 2 nothing owed, 20 reentrancy, 32 transfer failed,
///          98 invalid pointer, 200 caller spoofing.
#[no_mangle]
pub extern "C" fn claim_unpaid_payout(caller_ptr: *const u8, token_ptr: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 20;
    }

    let caller = match read_address32(caller_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 98;
        }
    };
    let token = match read_address32(token_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 98;
        }
    };

    if !signer_matches(&caller) {
        reentrancy_exit();
        return 200;
    }

    let token = Address(token);
    let recipient = Address(caller);
    let key = unpaid_payout_key(token, recipient);
    let amount = stored_u64(&key);
    if amount == 0 {
        reentrancy_exit();
        return 2;
    }

    storage_set(&key, &u64_to_bytes(0));
    match transfer_token_or_native(token, get_contract_address(), recipient, amount) {
        Ok(true) => {
            lichen_sdk::set_return_data(&u64_to_bytes(amount));
            reentrancy_exit();
            0
        }
        Ok(false) | Err(_) => {
            storage_set(&key, &u64_to_bytes(amount));
            reentrancy_exit();
            32
        }
    }
}

/// Query an unpaid compute-market payout.
#[no_mangle]
pub extern "C" fn get_unpaid_payout(token_ptr: *const u8, recipient_ptr: *const u8) -> u32 {
    let token = match read_address32(token_ptr) {
        Some(addr) => addr,
        None => return 98,
    };
    let recipient = match read_address32(recipient_ptr) {
        Some(addr) => addr,
        None => return 98,
    };

    lichen_sdk::set_return_data(&u64_to_bytes(stored_u64(&unpaid_payout_key(
        Address(token),
        Address(recipient),
    ))));
    0
}

// ============================================================================
// LICHENID IDENTITY INTEGRATION
// ============================================================================

/// Storage key for identity admin
const IDENTITY_ADMIN_KEY: &[u8] = b"identity_admin";
/// Storage key for minimum reputation threshold
const LICHENID_MIN_REP_KEY: &[u8] = b"lichenid_min_rep";
/// Storage key for LichenID contract address (32 bytes)
const LICHENID_ADDR_KEY: &[u8] = b"lichenid_address";

/// Set the admin for identity/reputation configuration.
/// Only callable once (first caller becomes admin).
#[no_mangle]
pub extern "C" fn set_identity_admin(admin_ptr: *const u8) -> u32 {
    let admin = match read_address32(admin_ptr) {
        Some(v) => v,
        None => {
            log_info("set_identity_admin rejected: null admin_ptr");
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != admin {
        return 200;
    }

    if storage_get(IDENTITY_ADMIN_KEY).is_some() {
        log_info("Identity admin already set");
        return 1;
    }

    storage_set(IDENTITY_ADMIN_KEY, &admin);
    log_info("Identity admin set");
    0
}

/// Set LichenID contract address for cross-contract reputation lookups.
/// Only callable by the identity admin.
#[no_mangle]
pub extern "C" fn set_lichenid_address(caller_ptr: *const u8, lichenid_addr_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => {
            log_info("set_lichenid_address rejected: null caller_ptr");
            return 98;
        }
    };
    let lichenid_addr = match read_address32(lichenid_addr_ptr) {
        Some(v) => v,
        None => {
            log_info("set_lichenid_address rejected: null lichenid_addr_ptr");
            return 98;
        }
    };

    if !signer_matches(&caller) {
        return 200;
    }

    let admin = match storage_get(IDENTITY_ADMIN_KEY) {
        Some(data) => data,
        None => return 1,
    };
    if caller[..] != admin[..] {
        return 2;
    }
    if lichenid_addr.iter().all(|&b| b == 0) {
        log_info("Cannot set zero LichenID address");
        return 3;
    }
    if load_configured_address(LICHENID_ADDR_KEY).is_some() {
        log_info("LichenID address already configured");
        return 4;
    }

    storage_set(LICHENID_ADDR_KEY, &lichenid_addr);
    log_info("LichenID address configured");
    0
}

/// Set minimum LichenID reputation required for gated functions.
/// Only callable by the identity admin.
#[no_mangle]
pub extern "C" fn set_identity_gate(caller_ptr: *const u8, min_reputation: u64) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => {
            log_info("set_identity_gate rejected: null caller_ptr");
            return 98;
        }
    };

    if !signer_matches(&caller) {
        return 200;
    }

    let admin = match storage_get(IDENTITY_ADMIN_KEY) {
        Some(data) => data,
        None => return 1,
    };
    if caller[..] != admin[..] {
        return 2;
    }

    storage_set(LICHENID_MIN_REP_KEY, &u64_to_bytes(min_reputation));
    log_info("Identity gate configured");
    0
}

/// Pause the compute market. Only callable by admin.
/// While paused, new work intake and execution progression stay blocked, but
/// escrow unwind paths remain available so existing jobs can still be exited.
#[no_mangle]
pub extern "C" fn pause(caller_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => return 98,
    };
    if !signer_matches(&caller) {
        return 200;
    }
    if !is_admin(&caller) {
        return 2;
    }
    storage_set(b"cm_paused", &[1]);
    log_info("Compute market paused");
    0
}

/// Unpause the compute market. Only callable by admin.
#[no_mangle]
pub extern "C" fn unpause(caller_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => return 98,
    };
    if !signer_matches(&caller) {
        return 200;
    }
    if !is_admin(&caller) {
        return 2;
    }
    storage_set(b"cm_paused", &[]);
    log_info("Compute market unpaused");
    0
}

/// Check if caller meets the LichenID reputation threshold.
/// Returns true if no gate is set or caller meets threshold.
fn check_identity_gate(caller: &[u8]) -> bool {
    let min_rep = match storage_get(LICHENID_MIN_REP_KEY) {
        Some(data) if data.len() >= 8 => bytes_to_u64(&data),
        _ => return true,
    };
    if min_rep == 0 {
        return true;
    }

    let lichenid_addr = match storage_get(LICHENID_ADDR_KEY) {
        Some(data) if data.len() >= 32 => data,
        _ => return true,
    };

    let mut addr = [0u8; 32];
    addr.copy_from_slice(&lichenid_addr[..32]);
    let target = Address::new(addr);
    let mut args = Vec::with_capacity(32);
    args.extend_from_slice(caller);
    let call = CrossCall::new(target, "get_reputation", args);

    match call_contract(call) {
        Ok(result) if result.len() >= 8 => {
            let reputation = bytes_to_u64(&result);
            reputation >= min_rep
        }
        _ => false,
    }
}

// ============================================================================
// ALIASES — bridge test-expected names to actual implementation
// ============================================================================

/// Alias: tests call `create_job` but contract uses `submit_job`
#[no_mangle]
pub extern "C" fn create_job(
    requester_ptr: *const u8,
    compute_units_needed: u64,
    max_price: u64,
    code_hash_ptr: *const u8,
) -> u32 {
    submit_job(
        requester_ptr,
        compute_units_needed,
        max_price,
        code_hash_ptr,
    )
}

/// Alias: tests call `accept_job` but contract uses `claim_job`
#[no_mangle]
pub extern "C" fn accept_job(provider_ptr: *const u8, job_id: u64) -> u32 {
    claim_job(provider_ptr, job_id)
}

/// Alias: tests call `submit_result` but contract uses `complete_job`
#[no_mangle]
pub extern "C" fn submit_result(
    provider_ptr: *const u8,
    job_id: u64,
    result_hash_ptr: *const u8,
) -> u32 {
    complete_job(provider_ptr, job_id, result_hash_ptr)
}

/// Alias: tests call `confirm_result` but contract uses `release_payment`
#[no_mangle]
pub extern "C" fn confirm_result(job_id: u64) -> u32 {
    release_payment(job_id)
}

/// Alias: tests call `get_job_info` but contract uses `get_job`
#[no_mangle]
pub extern "C" fn get_job_info(job_id: u64) -> u32 {
    get_job(job_id)
}

/// Tests expect `get_job_count`
#[no_mangle]
pub extern "C" fn get_job_count() -> u64 {
    storage_get(b"job_count")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0)
}

/// Tests expect `get_provider_info`
#[no_mangle]
pub extern "C" fn get_provider_info(provider_ptr: *const u8) -> u32 {
    let addr = match read_address32(provider_ptr) {
        Some(v) => v,
        None => return 1,
    };
    let pk = provider_key(&addr);
    match storage_get(&pk) {
        Some(data) => {
            lichen_sdk::set_return_data(&data);
            0
        }
        None => 1,
    }
}

/// Tests expect `set_platform_fee`
#[no_mangle]
pub extern "C" fn set_platform_fee(caller_ptr: *const u8, fee_bps: u64) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => return 98,
    };
    // AUDIT-FIX: verify transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }
    if !is_admin(&caller) {
        return 1;
    }
    if fee_bps > 1000 {
        return 2;
    }
    storage_set(b"platform_fee_bps", &u64_to_bytes(fee_bps));
    log_info("Platform fee set");
    0
}

/// Tests expect `cm_pause`
#[no_mangle]
pub extern "C" fn cm_pause(caller_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => return 98,
    };
    // AUDIT-FIX: verify transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }
    if !is_admin(&caller) {
        return 1;
    }
    storage_set(b"cm_paused", &[1u8]);
    log_info("Compute market paused");
    0
}

/// Tests expect `cm_unpause`
#[no_mangle]
pub extern "C" fn cm_unpause(caller_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => return 98,
    };
    // AUDIT-FIX: verify transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }
    if !is_admin(&caller) {
        return 1;
    }
    storage_set(b"cm_paused", &[0u8]);
    log_info("Compute market unpaused");
    0
}

/// Get compute market stats [job_count(8), completed_count(8), payment_volume(8), dispute_count(8)]
#[no_mangle]
pub extern "C" fn get_platform_stats() -> u32 {
    let mut buf = Vec::with_capacity(32);
    buf.extend_from_slice(&u64_to_bytes(
        storage_get(b"job_count")
            .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
            .unwrap_or(0),
    ));
    buf.extend_from_slice(&u64_to_bytes(
        storage_get(CM_COMPLETED_COUNT_KEY)
            .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
            .unwrap_or(0),
    ));
    buf.extend_from_slice(&u64_to_bytes(
        storage_get(CM_PAYMENT_VOLUME_KEY)
            .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
            .unwrap_or(0),
    ));
    buf.extend_from_slice(&u64_to_bytes(
        storage_get(CM_DISPUTE_COUNT_KEY)
            .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
            .unwrap_or(0),
    ));
    lichen_sdk::set_return_data(&buf);
    0
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::vec;
    use lichen_sdk::test_mock;

    /// Common token address used in tests
    const TEST_TOKEN_ADDR: [u8; 32] = [0xFFu8; 32];

    fn setup() {
        test_mock::reset();
        // AUDIT-FIX H-4: Configure a mock payment token so token-flow functions work
        storage_set(CM_TOKEN_ADDRESS_KEY, &TEST_TOKEN_ADDR);
    }

    /// Helper: submit a job with caller mock set correctly
    fn submit_job_as(requester: &[u8; 32], cu: u64, price: u64, hash: &[u8; 32]) -> u32 {
        test_mock::set_caller(*requester);
        submit_job(requester.as_ptr(), cu, price, hash.as_ptr())
    }

    /// Helper: register a provider with caller mock set correctly
    fn register_as(provider: &[u8; 32], cap: u64, price: u64) -> u32 {
        test_mock::set_caller(*provider);
        register_provider(provider.as_ptr(), cap, price)
    }

    /// Helper: claim a job with caller mock set correctly
    fn claim_as(provider: &[u8; 32], job_id: u64) -> u32 {
        test_mock::set_caller(*provider);
        claim_job(provider.as_ptr(), job_id)
    }

    /// Helper: complete a job with caller mock set correctly
    fn complete_as(provider: &[u8; 32], job_id: u64, result_hash: &[u8; 32]) -> u32 {
        test_mock::set_caller(*provider);
        complete_job(provider.as_ptr(), job_id, result_hash.as_ptr())
    }

    /// Helper: dispute a job with caller mock set correctly
    fn dispute_as(requester: &[u8; 32], job_id: u64) -> u32 {
        test_mock::set_caller(*requester);
        dispute_job(requester.as_ptr(), job_id)
    }

    /// Helper: cancel a job with caller mock set correctly
    fn cancel_as(requester: &[u8; 32], job_id: u64) -> u32 {
        test_mock::set_caller(*requester);
        cancel_job(requester.as_ptr(), job_id)
    }

    /// Helper: initialize admin with caller mock set correctly
    fn initialize_as(admin: &[u8; 32]) -> u32 {
        test_mock::set_caller(*admin);
        initialize(admin.as_ptr())
    }

    /// Helper: resolve dispute with caller mock set correctly
    fn resolve_as(arb: &[u8; 32], job_id: u64, pct: u64) -> u32 {
        test_mock::set_caller(*arb);
        resolve_dispute(arb.as_ptr(), job_id, pct)
    }

    fn set_agent_controls_as(
        admin: &[u8; 32],
        enabled: u64,
        route_paused: u64,
        max_daily_cap: u64,
        max_per_task_cap: u64,
    ) -> u32 {
        test_mock::set_caller(*admin);
        set_agent_compute_controls(
            admin.as_ptr(),
            enabled,
            route_paused,
            max_daily_cap,
            max_per_task_cap,
        )
    }

    fn set_agent_policy_as(
        agent: &[u8; 32],
        daily_cap: u64,
        per_task_cap: u64,
        policy_hash: &[u8; 32],
        policy_version: u64,
    ) -> u32 {
        test_mock::set_caller(*agent);
        set_agent_spending_policy(
            agent.as_ptr(),
            daily_cap,
            per_task_cap,
            policy_hash.as_ptr(),
            policy_version,
        )
    }

    fn submit_agent_job_as(
        agent: &[u8; 32],
        cu: u64,
        price: u64,
        code_hash: &[u8; 32],
        action_hash: &[u8; 32],
    ) -> u32 {
        test_mock::set_caller(*agent);
        submit_agent_job(
            agent.as_ptr(),
            cu,
            price,
            code_hash.as_ptr(),
            action_hash.as_ptr(),
        )
    }

    fn unpaid_payout_key(token: &[u8; 32], recipient: &[u8; 32]) -> Vec<u8> {
        let mut key = b"unpaid_payout:".to_vec();
        key.extend_from_slice(token);
        key.push(b':');
        key.extend_from_slice(recipient);
        key
    }

    #[test]
    fn test_register_provider_and_submit_job() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let provider_addr = [1u8; 32];
        assert_eq!(register_as(&provider_addr, 1000, 50), 0);

        let pk = provider_key(&provider_addr);
        let prov = test_mock::get_storage(&pk).unwrap();
        assert_eq!(prov.len(), PROVIDER_SIZE);
        assert_eq!(bytes_to_u64(&prov[32..40]), 1000);
        assert_eq!(bytes_to_u64(&prov[40..48]), 50);
        assert_eq!(prov[56], 1);

        let requester = [2u8; 32];
        let code_hash = [0xAA; 32];
        assert_eq!(submit_job_as(&requester, 100, 5000, &code_hash), 0);

        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 0);

        let jk = job_key(0);
        let job = test_mock::get_storage(&jk).unwrap();
        assert_eq!(job.len(), JOB_SIZE);
        assert_eq!(&job[0..32], &requester);
        assert_eq!(job[80], JOB_PENDING);
    }

    #[test]
    fn test_claim_and_complete_job() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xAA; 32]);

        assert_eq!(claim_as(&provider_addr, 0), 0);
        let jk = job_key(0);
        let job = test_mock::get_storage(&jk).unwrap();
        assert_eq!(job[80], JOB_CLAIMED);
        assert_eq!(&job[81..113], &provider_addr);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 200);
        let result_hash = [0xBB; 32];
        assert_eq!(complete_as(&provider_addr, 0, &result_hash), 0);

        let job = test_mock::get_storage(&jk).unwrap();
        assert_eq!(job[80], JOB_COMPLETED);
        assert_eq!(&job[113..145], &result_hash);
        assert_eq!(bytes_to_u64(&job[153..161]), 200);
    }

    #[test]
    fn test_dispute_job() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xAA; 32]);
        claim_as(&provider_addr, 0);
        complete_as(&provider_addr, 0, &[0xCC; 32]);

        assert_eq!(dispute_as(&requester, 0), 0);
        let jk = job_key(0);
        let job = test_mock::get_storage(&jk).unwrap();
        assert_eq!(job[80], JOB_DISPUTED);

        // Non-requester can't dispute (caller mismatch = 200, or wrong requester = 3)
        let other = [9u8; 32];
        assert_eq!(dispute_as(&other, 0), 3);
    }

    #[test]
    fn test_get_job() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 50);

        let requester = [2u8; 32];
        submit_job_as(&requester, 200, 10000, &[0xAA; 32]);

        let result = get_job(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), JOB_SIZE);

        assert_eq!(get_job(999), 1);
    }

    #[test]
    fn test_identity_gate_blocks_submit_job() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        let lichenid_addr = [0x42u8; 32];
        assert_eq!(
            set_lichenid_address(admin.as_ptr(), lichenid_addr.as_ptr()),
            0
        );
        assert_eq!(set_identity_gate(admin.as_ptr(), 100), 0);

        let requester = [2u8; 32];
        assert_eq!(submit_job_as(&requester, 100, 5000, &[0xAA; 32]), 10);
    }

    #[test]
    fn test_identity_gate_blocks_register_provider() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        let lichenid_addr = [0x42u8; 32];
        assert_eq!(
            set_lichenid_address(admin.as_ptr(), lichenid_addr.as_ptr()),
            0
        );
        assert_eq!(set_identity_gate(admin.as_ptr(), 100), 0);

        let provider_addr = [2u8; 32];
        assert_eq!(register_as(&provider_addr, 1000, 50), 10);
    }

    #[test]
    fn test_identity_gate_allows_when_disabled() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let provider_addr = [1u8; 32];
        assert_eq!(register_as(&provider_addr, 1000, 50), 0);
        let requester = [2u8; 32];
        assert_eq!(submit_job_as(&requester, 100, 5000, &[0xAA; 32]), 0);
    }

    #[test]
    fn test_set_identity_gate_admin_only() {
        setup();

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        // Cannot set admin again
        assert_eq!(set_identity_admin(admin.as_ptr()), 1);

        let other = [9u8; 32];
        test_mock::set_caller(other);
        assert_eq!(set_identity_gate(other.as_ptr(), 100), 2);
        assert_eq!(
            set_lichenid_address(other.as_ptr(), [0x42u8; 32].as_ptr()),
            2
        );

        test_mock::set_caller(admin);
        assert_eq!(set_identity_gate(admin.as_ptr(), 100), 0);
        assert_eq!(
            set_lichenid_address(admin.as_ptr(), [0x42u8; 32].as_ptr()),
            0
        );
    }

    #[test]
    fn test_set_lichenid_address_rejects_zero_and_reconfiguration() {
        setup();

        let admin = [1u8; 32];
        let first = [0x42u8; 32];
        let second = [0x24u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);

        assert_eq!(set_lichenid_address(admin.as_ptr(), [0u8; 32].as_ptr()), 3);
        assert!(test_mock::get_storage(LICHENID_ADDR_KEY).is_none());

        assert_eq!(set_lichenid_address(admin.as_ptr(), first.as_ptr()), 0);
        assert_eq!(set_lichenid_address(admin.as_ptr(), second.as_ptr()), 4);
        assert_eq!(
            test_mock::get_storage(LICHENID_ADDR_KEY)
                .unwrap()
                .as_slice(),
            &first
        );
    }

    #[test]
    fn test_identity_admin_paths_reject_forged_caller_argument() {
        setup();

        let admin = [1u8; 32];
        let attacker = [9u8; 32];
        let lichenid_addr = [0x42u8; 32];

        test_mock::set_caller(admin);
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);

        test_mock::set_caller(attacker);
        assert_eq!(
            set_lichenid_address(admin.as_ptr(), lichenid_addr.as_ptr()),
            200
        );
        assert!(test_mock::get_storage(LICHENID_ADDR_KEY).is_none());

        assert_eq!(set_identity_gate(admin.as_ptr(), 100), 200);
        assert!(test_mock::get_storage(LICHENID_MIN_REP_KEY).is_none());

        test_mock::set_caller(admin);
        assert_eq!(
            set_lichenid_address(admin.as_ptr(), lichenid_addr.as_ptr()),
            0
        );
        assert_eq!(
            test_mock::get_storage(LICHENID_ADDR_KEY)
                .unwrap()
                .as_slice(),
            &lichenid_addr
        );

        assert_eq!(set_identity_gate(admin.as_ptr(), 100), 0);
        assert_eq!(
            bytes_to_u64(&test_mock::get_storage(LICHENID_MIN_REP_KEY).unwrap()),
            100
        );
    }

    #[test]
    fn test_pause_and_unpause_reject_forged_caller_argument() {
        setup();

        let admin = [0xAD; 32];
        let attacker = [9u8; 32];
        initialize_as(&admin);

        test_mock::set_caller(attacker);
        assert_eq!(pause(admin.as_ptr()), 200);
        assert!(test_mock::get_storage(b"cm_paused").is_none());

        test_mock::set_caller(admin);
        assert_eq!(pause(admin.as_ptr()), 0);
        assert_eq!(
            test_mock::get_storage(b"cm_paused").unwrap().as_slice(),
            &[1u8]
        );

        test_mock::set_caller(attacker);
        assert_eq!(unpause(admin.as_ptr()), 200);
        assert_eq!(
            test_mock::get_storage(b"cm_paused").unwrap().as_slice(),
            &[1u8]
        );

        test_mock::set_caller(admin);
        assert_eq!(unpause(admin.as_ptr()), 0);
        assert_eq!(
            test_mock::get_storage(b"cm_paused").unwrap().as_slice(),
            &[]
        );
    }

    // ========================================================================
    // v2 TESTS
    // ========================================================================

    #[test]
    fn test_initialize_admin() {
        setup();
        let admin = [0xAD; 32];
        assert_eq!(initialize_as(&admin), 0);
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 1);
        let stored = test_mock::get_storage(ADMIN_KEY).unwrap();
        assert_eq!(stored.as_slice(), &admin);
    }

    #[test]
    fn test_admin_set_timeouts() {
        setup();
        let admin = [0xAD; 32];
        initialize_as(&admin);

        let other = [9u8; 32];
        test_mock::set_caller(other);
        assert_eq!(set_claim_timeout(other.as_ptr(), 500), 1);
        assert_eq!(set_complete_timeout(other.as_ptr(), 2000), 1);
        assert_eq!(set_challenge_period(other.as_ptr(), 50), 1);

        test_mock::set_caller(admin);
        assert_eq!(set_claim_timeout(admin.as_ptr(), 500), 0);
        assert_eq!(set_complete_timeout(admin.as_ptr(), 2000), 0);
        assert_eq!(set_challenge_period(admin.as_ptr(), 50), 0);

        assert_eq!(set_claim_timeout(admin.as_ptr(), 0), 2);
        assert_eq!(set_complete_timeout(admin.as_ptr(), 0), 2);
        assert_eq!(set_challenge_period(admin.as_ptr(), 0), 2);

        assert_eq!(get_claim_timeout(), 500);
        assert_eq!(get_complete_timeout(), 2000);
        assert_eq!(get_challenge_period(), 50);
    }

    #[test]
    fn test_add_remove_arbitrator() {
        setup();
        let admin = [0xAD; 32];
        initialize_as(&admin);

        let arb = [0xAA; 32];
        let other = [9u8; 32];

        test_mock::set_caller(other);
        assert_eq!(add_arbitrator(other.as_ptr(), arb.as_ptr()), 1);

        test_mock::set_caller(admin);
        assert_eq!(add_arbitrator(admin.as_ptr(), arb.as_ptr()), 0);
        assert!(is_arbitrator(&arb));

        test_mock::set_caller(other);
        assert_eq!(remove_arbitrator(other.as_ptr(), arb.as_ptr()), 1);

        test_mock::set_caller(admin);
        assert_eq!(remove_arbitrator(admin.as_ptr(), arb.as_ptr()), 0);
        assert!(!is_arbitrator(&arb));
    }

    #[test]
    fn test_escrow_set_on_submit() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let requester = [2u8; 32];
        assert_eq!(submit_job_as(&requester, 100, 5000, &[0xAA; 32]), 0);

        let ek = escrow_key(0);
        let escrowed = test_mock::get_storage(&ek).unwrap();
        assert_eq!(bytes_to_u64(&escrowed), 5000);

        assert_eq!(get_escrow(0), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 5000);
    }

    #[test]
    fn test_submit_job_zero_price_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        let requester = [2u8; 32];
        assert_eq!(submit_job_as(&requester, 100, 0, &[0xAA; 32]), 11);
    }

    #[test]
    fn test_cancel_pending_job_after_timeout() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xAA; 32]);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 250);
        assert_eq!(cancel_as(&requester, 0), 4);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 301);
        assert_eq!(cancel_as(&requester, 0), 0);

        let jk = job_key(0);
        let job = test_mock::get_storage(&jk).unwrap();
        assert_eq!(job[80], JOB_CANCELLED);

        let ek = escrow_key(0);
        let escrowed = test_mock::get_storage(&ek).unwrap();
        assert_eq!(bytes_to_u64(&escrowed), 0);
    }

    #[test]
    fn test_cancel_job_still_works_when_paused() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [0xAD; 32];
        initialize_as(&admin);

        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xAA; 32]);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 301);
        test_mock::set_caller(admin);
        assert_eq!(pause(admin.as_ptr()), 0);

        assert_eq!(cancel_as(&requester, 0), 0);

        let jk = job_key(0);
        let job = test_mock::get_storage(&jk).unwrap();
        assert_eq!(job[80], JOB_CANCELLED);

        let ek = escrow_key(0);
        let escrowed = test_mock::get_storage(&ek).unwrap();
        assert_eq!(bytes_to_u64(&escrowed), 0);
    }

    #[test]
    fn test_cancel_claimed_job_after_complete_timeout() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xAA; 32]);
        claim_as(&provider_addr, 0);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 500);
        assert_eq!(cancel_as(&requester, 0), 5);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 1101);
        assert_eq!(cancel_as(&requester, 0), 0);

        let jk = job_key(0);
        let job = test_mock::get_storage(&jk).unwrap();
        assert_eq!(job[80], JOB_CANCELLED);
    }

    #[test]
    fn test_non_requester_cannot_cancel() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let requester = [2u8; 32];
        let other = [9u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xAA; 32]);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 400);
        assert_eq!(cancel_as(&other, 0), 3);
    }

    #[test]
    fn test_release_payment_after_challenge_period() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xAA; 32]);
        claim_as(&provider_addr, 0);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 200);
        complete_as(&provider_addr, 0, &[0xBB; 32]);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 250);
        assert_eq!(release_payment(0), 5);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 301);
        assert_eq!(release_payment(0), 0);

        let jk = job_key(0);
        let job = test_mock::get_storage(&jk).unwrap();
        assert_eq!(job[80], JOB_RELEASED);

        let ek = escrow_key(0);
        let escrowed = test_mock::get_storage(&ek).unwrap();
        assert_eq!(bytes_to_u64(&escrowed), 0);
    }

    #[test]
    fn test_dispute_job_still_works_when_paused() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [0xAD; 32];
        initialize_as(&admin);

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xAA; 32]);
        claim_as(&provider_addr, 0);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 200);
        complete_as(&provider_addr, 0, &[0xBB; 32]);

        test_mock::set_caller(admin);
        assert_eq!(pause(admin.as_ptr()), 0);

        assert_eq!(dispute_as(&requester, 0), 0);

        let jk = job_key(0);
        let job = test_mock::get_storage(&jk).unwrap();
        assert_eq!(job[80], JOB_DISPUTED);
    }

    #[test]
    fn test_release_payment_still_works_when_paused() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [0xAD; 32];
        initialize_as(&admin);

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xAA; 32]);
        claim_as(&provider_addr, 0);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 200);
        complete_as(&provider_addr, 0, &[0xBB; 32]);

        test_mock::set_caller(admin);
        assert_eq!(pause(admin.as_ptr()), 0);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 301);
        assert_eq!(release_payment(0), 0);

        let jk = job_key(0);
        let job = test_mock::get_storage(&jk).unwrap();
        assert_eq!(job[80], JOB_RELEASED);

        let ek = escrow_key(0);
        let escrowed = test_mock::get_storage(&ek).unwrap();
        assert_eq!(bytes_to_u64(&escrowed), 0);
    }

    #[test]
    fn test_release_rejects_non_completed() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xAA; 32]);

        assert_eq!(release_payment(0), 3);
    }

    #[test]
    fn test_resolve_dispute_full_refund() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [0xAD; 32];
        initialize_as(&admin);
        let arb = [0xAA; 32];
        test_mock::set_caller(admin);
        add_arbitrator(admin.as_ptr(), arb.as_ptr());

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xCC; 32]);
        claim_as(&provider_addr, 0);
        complete_as(&provider_addr, 0, &[0xBB; 32]);
        dispute_as(&requester, 0);

        assert_eq!(resolve_as(&arb, 0, 100), 0);

        let jk = job_key(0);
        let job = test_mock::get_storage(&jk).unwrap();
        assert_eq!(job[80], JOB_RESOLVED);

        let ek = escrow_key(0);
        let escrowed = test_mock::get_storage(&ek).unwrap();
        assert_eq!(bytes_to_u64(&escrowed), 0);
    }

    #[test]
    fn test_resolve_dispute_split() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [0xAD; 32];
        initialize_as(&admin);
        let arb = [0xAA; 32];
        test_mock::set_caller(admin);
        add_arbitrator(admin.as_ptr(), arb.as_ptr());

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 10000, &[0xCC; 32]);
        claim_as(&provider_addr, 0);
        complete_as(&provider_addr, 0, &[0xBB; 32]);
        dispute_as(&requester, 0);

        assert_eq!(resolve_as(&arb, 0, 60), 0);
        let jk = job_key(0);
        let job = test_mock::get_storage(&jk).unwrap();
        assert_eq!(job[80], JOB_RESOLVED);
    }

    #[test]
    fn test_non_arbitrator_cannot_resolve() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [0xAD; 32];
        initialize_as(&admin);

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xCC; 32]);
        claim_as(&provider_addr, 0);
        complete_as(&provider_addr, 0, &[0xBB; 32]);
        dispute_as(&requester, 0);

        let fake = [0xFE; 32]; // avoid 0xFF which is TEST_TOKEN_ADDR
        assert_eq!(resolve_as(&fake, 0, 50), 1);
    }

    #[test]
    fn test_resolve_non_disputed_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [0xAD; 32];
        initialize_as(&admin);
        let arb = [0xAA; 32];
        test_mock::set_caller(admin);
        add_arbitrator(admin.as_ptr(), arb.as_ptr());

        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xCC; 32]);

        assert_eq!(resolve_as(&arb, 0, 50), 5);
    }

    #[test]
    fn test_resolve_invalid_pct_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [0xAD; 32];
        initialize_as(&admin);
        let arb = [0xAA; 32];
        test_mock::set_caller(admin);
        add_arbitrator(admin.as_ptr(), arb.as_ptr());

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xCC; 32]);
        claim_as(&provider_addr, 0);
        complete_as(&provider_addr, 0, &[0xBB; 32]);
        dispute_as(&requester, 0);

        assert_eq!(resolve_as(&arb, 0, 101), 2);
    }

    #[test]
    fn test_deactivate_reactivate_provider() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);

        test_mock::set_caller(provider_addr);
        assert_eq!(deactivate_provider(provider_addr.as_ptr()), 0);
        let pk = provider_key(&provider_addr);
        let prov = test_mock::get_storage(&pk).unwrap();
        assert_eq!(prov[56], 0);

        assert_eq!(deactivate_provider(provider_addr.as_ptr()), 3);

        assert_eq!(reactivate_provider(provider_addr.as_ptr()), 0);
        let prov = test_mock::get_storage(&pk).unwrap();
        assert_eq!(prov[56], 1);

        assert_eq!(reactivate_provider(provider_addr.as_ptr()), 3);
    }

    #[test]
    fn test_update_provider() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);

        test_mock::set_caller(provider_addr);
        assert_eq!(update_provider(provider_addr.as_ptr(), 2000, 75), 0);
        let pk = provider_key(&provider_addr);
        let prov = test_mock::get_storage(&pk).unwrap();
        assert_eq!(bytes_to_u64(&prov[32..40]), 2000);
        assert_eq!(bytes_to_u64(&prov[40..48]), 75);

        assert_eq!(update_provider(provider_addr.as_ptr(), 0, 75), 3);
        assert_eq!(update_provider(provider_addr.as_ptr(), 2000, 0), 3);

        let fake = [0xFE; 32];
        test_mock::set_caller(fake);
        assert_eq!(update_provider(fake.as_ptr(), 100, 100), 1);
    }

    #[test]
    fn test_removed_arbitrator_cannot_resolve() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [0xAD; 32];
        initialize_as(&admin);
        let arb = [0xAA; 32];
        test_mock::set_caller(admin);
        add_arbitrator(admin.as_ptr(), arb.as_ptr());
        remove_arbitrator(admin.as_ptr(), arb.as_ptr());

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xCC; 32]);
        claim_as(&provider_addr, 0);
        complete_as(&provider_addr, 0, &[0xBB; 32]);
        dispute_as(&requester, 0);

        assert_eq!(resolve_as(&arb, 0, 50), 1);
    }

    #[test]
    fn test_cancel_completed_job_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xCC; 32]);
        claim_as(&provider_addr, 0);
        complete_as(&provider_addr, 0, &[0xBB; 32]);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 9999);
        assert_eq!(cancel_as(&requester, 0), 6);
    }

    #[test]
    fn test_default_timeouts() {
        setup();
        assert_eq!(get_claim_timeout(), DEFAULT_CLAIM_TIMEOUT);
        assert_eq!(get_complete_timeout(), DEFAULT_COMPLETE_TIMEOUT);
        assert_eq!(get_challenge_period(), DEFAULT_CHALLENGE_PERIOD);
    }

    // ========================================================================
    // AUDIT-FIX: H-1/H-2/H-3/H-4 Token flow tests
    // ========================================================================

    #[test]
    fn test_set_token_address_admin_only() {
        test_mock::reset();
        let admin = [0xAD; 32];
        initialize_as(&admin);

        let token = [0xBB; 32];
        let other = [9u8; 32];
        test_mock::set_caller(other);
        assert_eq!(set_token_address(other.as_ptr(), token.as_ptr()), 1);

        test_mock::set_caller(admin);
        assert_eq!(set_token_address(admin.as_ptr(), token.as_ptr()), 0);
        let stored = test_mock::get_storage(CM_TOKEN_ADDRESS_KEY).unwrap();
        assert_eq!(stored.as_slice(), &token);
    }

    #[test]
    fn test_set_token_address_accepts_native_licn_and_rejects_reconfiguration() {
        test_mock::reset();
        let admin = [0xAD; 32];
        initialize_as(&admin);

        let first = [0xBB; 32];
        let second = [0xCC; 32];

        test_mock::set_caller(admin);
        assert_eq!(set_token_address(admin.as_ptr(), [0u8; 32].as_ptr()), 0);
        assert_eq!(load_token_address(), Some([0u8; 32]));

        assert_eq!(set_token_address(admin.as_ptr(), first.as_ptr()), 3);
        assert_eq!(set_token_address(admin.as_ptr(), second.as_ptr()), 3);
        assert_eq!(load_token_address(), Some([0u8; 32]));
    }

    #[test]
    fn test_submit_job_requires_token_address() {
        // Reset without setting token address
        test_mock::reset();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let requester = [2u8; 32];
        // No token configured → should fail with 12
        assert_eq!(submit_job_as(&requester, 100, 5000, &[0xAA; 32]), 12);
    }

    #[test]
    fn test_submit_job_escrows_tokens() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let requester = [2u8; 32];
        // Token address configured in setup, mock call_contract returns Ok
        assert_eq!(submit_job_as(&requester, 100, 5000, &[0xAA; 32]), 0);

        // Escrow stored
        let ek = escrow_key(0);
        let escrowed = test_mock::get_storage(&ek).unwrap();
        assert_eq!(bytes_to_u64(&escrowed), 5000);
    }

    #[test]
    fn test_cancel_job_refunds_tokens() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xAA; 32]);

        // Cancel after timeout
        test_mock::SLOT.with(|s| *s.borrow_mut() = 301);
        assert_eq!(cancel_as(&requester, 0), 0);

        // Escrow cleared (tokens were refunded via call_token_transfer)
        let ek = escrow_key(0);
        let escrowed = test_mock::get_storage(&ek).unwrap();
        assert_eq!(bytes_to_u64(&escrowed), 0);
    }

    #[test]
    fn test_release_payment_transfers_to_provider() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xAA; 32]);
        claim_as(&provider_addr, 0);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 200);
        complete_as(&provider_addr, 0, &[0xBB; 32]);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 301);
        assert_eq!(release_payment(0), 0);

        // Escrow cleared (tokens were transferred to provider)
        let ek = escrow_key(0);
        let escrowed = test_mock::get_storage(&ek).unwrap();
        assert_eq!(bytes_to_u64(&escrowed), 0);

        // Completion stats tracked
        let cmc = test_mock::get_storage(CM_COMPLETED_COUNT_KEY).unwrap();
        assert_eq!(bytes_to_u64(&cmc), 1);
        let cmv = test_mock::get_storage(CM_PAYMENT_VOLUME_KEY).unwrap();
        assert_eq!(bytes_to_u64(&cmv), 5000);
    }

    #[test]
    fn test_submit_job_rejects_job_count_overflow_before_escrow() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        storage_set(b"job_count", &u64_to_bytes(u64::MAX));

        let requester = [2u8; 32];
        assert_eq!(submit_job_as(&requester, 100, 5000, &[0xAA; 32]), 14);
        assert!(test_mock::get_storage(&job_key(u64::MAX)).is_none());
        assert!(test_mock::get_last_cross_call().is_none());
    }

    #[test]
    fn test_submit_job_false_escrow_status_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));

        let requester = [2u8; 32];
        assert_eq!(submit_job_as(&requester, 100, 5000, &[0xAA; 32]), 13);
        assert!(test_mock::get_storage(b"job_count").is_none());
        assert!(test_mock::get_storage(&job_key(0)).is_none());
    }

    #[test]
    fn test_inactive_provider_cannot_claim_job() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        test_mock::set_caller(provider_addr);
        assert_eq!(deactivate_provider(provider_addr.as_ptr()), 0);

        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xAA; 32]);

        assert_eq!(claim_as(&provider_addr, 0), 6);
        let job = test_mock::get_storage(&job_key(0)).unwrap();
        assert_eq!(job[80], JOB_PENDING);
    }

    #[test]
    fn test_cancel_refund_false_status_preserves_job_and_escrow() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        test_mock::set_cross_call_responses(vec![
            0u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
        ]);

        let requester = [2u8; 32];
        assert_eq!(submit_job_as(&requester, 100, 5000, &[0xAA; 32]), 0);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 301);
        assert_eq!(cancel_as(&requester, 0), 7);

        let job = test_mock::get_storage(&job_key(0)).unwrap();
        assert_eq!(job[80], JOB_PENDING);
        let escrowed = test_mock::get_storage(&escrow_key(0)).unwrap();
        assert_eq!(bytes_to_u64(&escrowed), 5000);
    }

    #[test]
    fn test_release_false_transfer_preserves_completed_state() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        test_mock::set_cross_call_responses(vec![
            0u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
        ]);

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xAA; 32]);
        claim_as(&provider_addr, 0);
        test_mock::SLOT.with(|s| *s.borrow_mut() = 200);
        complete_as(&provider_addr, 0, &[0xBB; 32]);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 301);
        assert_eq!(release_payment(0), 6);

        let job = test_mock::get_storage(&job_key(0)).unwrap();
        assert_eq!(job[80], JOB_COMPLETED);
        let escrowed = test_mock::get_storage(&escrow_key(0)).unwrap();
        assert_eq!(bytes_to_u64(&escrowed), 5000);
    }

    #[test]
    fn test_resolve_dispute_partial_payout_failure_records_unpaid_provider() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        test_mock::set_cross_call_responses(vec![
            0u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
        ]);

        let admin = [0xAD; 32];
        initialize_as(&admin);
        let arb = [0xAA; 32];
        test_mock::set_caller(admin);
        assert_eq!(add_arbitrator(admin.as_ptr(), arb.as_ptr()), 0);

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 10000, &[0xCC; 32]);
        claim_as(&provider_addr, 0);
        complete_as(&provider_addr, 0, &[0xBB; 32]);
        dispute_as(&requester, 0);

        assert_eq!(resolve_as(&arb, 0, 60), 0);

        let job = test_mock::get_storage(&job_key(0)).unwrap();
        assert_eq!(job[80], JOB_RESOLVED);
        let escrowed = test_mock::get_storage(&escrow_key(0)).unwrap();
        assert_eq!(bytes_to_u64(&escrowed), 0);
        let unpaid =
            test_mock::get_storage(&unpaid_payout_key(&TEST_TOKEN_ADDR, &provider_addr)).unwrap();
        assert_eq!(bytes_to_u64(&unpaid), 4000);
    }

    #[test]
    fn test_claim_unpaid_payout_retries_after_failed_transfer() {
        setup();
        let provider_addr = [1u8; 32];
        let key = unpaid_payout_key(&TEST_TOKEN_ADDR, &provider_addr);
        storage_set(&key, &u64_to_bytes(4000));

        assert_eq!(
            get_unpaid_payout(TEST_TOKEN_ADDR.as_ptr(), provider_addr.as_ptr()),
            0
        );
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 4000);

        test_mock::set_caller(provider_addr);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(
            claim_unpaid_payout(provider_addr.as_ptr(), TEST_TOKEN_ADDR.as_ptr()),
            32
        );
        let unpaid = test_mock::get_storage(&key).unwrap();
        assert_eq!(bytes_to_u64(&unpaid), 4000);

        test_mock::set_cross_call_response(Some(1u32.to_le_bytes().to_vec()));
        assert_eq!(
            claim_unpaid_payout(provider_addr.as_ptr(), TEST_TOKEN_ADDR.as_ptr()),
            0
        );
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 4000);
        let unpaid = test_mock::get_storage(&key).unwrap();
        assert_eq!(bytes_to_u64(&unpaid), 0);
    }

    #[test]
    fn test_claim_unpaid_payout_rejects_caller_spoof() {
        setup();
        let provider_addr = [1u8; 32];
        let key = unpaid_payout_key(&TEST_TOKEN_ADDR, &provider_addr);
        storage_set(&key, &u64_to_bytes(4000));

        test_mock::set_caller([9u8; 32]);
        assert_eq!(
            claim_unpaid_payout(provider_addr.as_ptr(), TEST_TOKEN_ADDR.as_ptr()),
            200
        );
        let unpaid = test_mock::get_storage(&key).unwrap();
        assert_eq!(bytes_to_u64(&unpaid), 4000);
    }

    #[test]
    fn test_complete_and_dispute_counters_saturate() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let pk = provider_key(&provider_addr);
        let mut provider_data = test_mock::get_storage(&pk).unwrap();
        provider_data[48..56].copy_from_slice(&u64_to_bytes(u64::MAX));
        storage_set(&pk, &provider_data);

        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xAA; 32]);
        claim_as(&provider_addr, 0);
        complete_as(&provider_addr, 0, &[0xBB; 32]);

        let provider_data = test_mock::get_storage(&pk).unwrap();
        assert_eq!(bytes_to_u64(&provider_data[48..56]), u64::MAX);

        storage_set(CM_DISPUTE_COUNT_KEY, &u64_to_bytes(u64::MAX));
        dispute_as(&requester, 0);
        let dispute_count = test_mock::get_storage(CM_DISPUTE_COUNT_KEY).unwrap();
        assert_eq!(bytes_to_u64(&dispute_count), u64::MAX);
    }

    #[test]
    fn test_release_completed_counter_saturates() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        test_mock::set_cross_call_responses(vec![
            0u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
        ]);

        let provider_addr = [1u8; 32];
        register_as(&provider_addr, 1000, 50);
        let requester = [2u8; 32];
        submit_job_as(&requester, 100, 5000, &[0xAA; 32]);
        claim_as(&provider_addr, 0);
        complete_as(&provider_addr, 0, &[0xBB; 32]);
        storage_set(CM_COMPLETED_COUNT_KEY, &u64_to_bytes(u64::MAX));

        test_mock::SLOT.with(|s| *s.borrow_mut() = 301);
        assert_eq!(release_payment(0), 0);

        let completed_count = test_mock::get_storage(CM_COMPLETED_COUNT_KEY).unwrap();
        assert_eq!(bytes_to_u64(&completed_count), u64::MAX);
    }

    #[test]
    fn test_agent_compute_policy_enforces_task_and_daily_caps() {
        setup();
        test_mock::set_slot(100);
        let admin = [0xAD; 32];
        let agent = [0xA9; 32];
        let policy_hash = [0x44; 32];
        let code_hash = [0x55; 32];
        let action_hash = [0x66; 32];

        assert_eq!(initialize_as(&admin), 0);
        assert_eq!(set_agent_controls_as(&admin, 1, 0, 10_000, 5_000), 0);
        assert_eq!(
            set_agent_policy_as(&agent, 6_000, 4_000, &policy_hash, 1),
            0
        );

        assert_eq!(
            get_agent_spending_policy(agent.as_ptr()),
            0,
            "policy should be queryable"
        );
        let policy = test_mock::get_return_data();
        assert_eq!(policy.len(), AGENT_POLICY_SIZE);
        assert_eq!(bytes_to_u64(&policy[0..8]), 1);
        assert_eq!(bytes_to_u64(&policy[8..16]), 6_000);
        assert_eq!(bytes_to_u64(&policy[16..24]), 4_000);
        assert_eq!(&policy[24..56], &policy_hash);
        assert_eq!(policy[72], 1);

        assert_eq!(
            submit_agent_job_as(&agent, 10, 3_000, &code_hash, &action_hash),
            0
        );
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 0);
        assert_eq!(stored_u64(b"job_count"), 1);
        assert_eq!(stored_u64(CM_AGENT_PAYMENT_COUNT_KEY), 1);
        assert_eq!(stored_u64(CM_AGENT_PAYMENT_VOLUME_KEY), 3_000);

        assert_eq!(get_agent_job_action(0), 0);
        assert_eq!(test_mock::get_return_data(), action_hash.to_vec());
        assert_eq!(get_agent_spend_window(agent.as_ptr(), 0), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 3_000);

        assert_eq!(
            submit_agent_job_as(&agent, 10, 4_001, &code_hash, &action_hash),
            44,
            "per-task cap blocks before escrow"
        );
        assert_eq!(
            submit_agent_job_as(&agent, 10, 3_500, &code_hash, &action_hash),
            47,
            "daily cap blocks before escrow"
        );
        assert_eq!(stored_u64(b"job_count"), 1);
        assert_eq!(stored_u64(CM_AGENT_BLOCKED_PAYMENT_COUNT_KEY), 2);
    }

    #[test]
    fn test_agent_compute_route_pause_blocks_before_escrow() {
        setup();
        test_mock::set_slot(100);
        let admin = [0xAD; 32];
        let agent = [0xA9; 32];
        let policy_hash = [0x44; 32];
        let code_hash = [0x55; 32];
        let action_hash = [0x66; 32];

        assert_eq!(initialize_as(&admin), 0);
        assert_eq!(set_agent_controls_as(&admin, 1, 1, 10_000, 5_000), 0);
        assert_eq!(
            set_agent_policy_as(&agent, 6_000, 4_000, &policy_hash, 1),
            0
        );

        assert_eq!(
            submit_agent_job_as(&agent, 10, 3_000, &code_hash, &action_hash),
            41
        );
        assert!(test_mock::get_storage(b"job_count").is_none());
        assert!(test_mock::get_last_cross_call().is_none());
        assert_eq!(stored_u64(CM_AGENT_BLOCKED_PAYMENT_COUNT_KEY), 1);

        assert_eq!(set_agent_controls_as(&admin, 1, 0, 10_000, 5_000), 0);
        assert_eq!(
            submit_agent_job_as(&agent, 10, 3_000, &code_hash, &action_hash),
            0
        );
        assert_eq!(stored_u64(b"job_count"), 1);
    }

    #[test]
    fn test_agent_compute_requires_enabled_policy_and_pq_action_hash() {
        setup();
        test_mock::set_slot(100);
        let admin = [0xAD; 32];
        let agent = [0xA9; 32];
        let attacker = [0xFE; 32];
        let policy_hash = [0x44; 32];
        let code_hash = [0x55; 32];
        let action_hash = [0x66; 32];

        assert_eq!(initialize_as(&admin), 0);
        assert_eq!(
            set_agent_controls_as(&attacker, 1, 0, 10_000, 5_000),
            1,
            "only compute admin can enable agent payments"
        );
        assert_eq!(
            set_agent_controls_as(&admin, 1, 0, 0, 5_000),
            3,
            "enabled controls require global caps"
        );
        assert_eq!(set_agent_controls_as(&admin, 0, 0, 0, 0), 0);
        assert_eq!(
            submit_agent_job_as(&agent, 10, 1_000, &code_hash, &action_hash),
            40,
            "disabled agent payments fail closed"
        );

        assert_eq!(set_agent_controls_as(&admin, 1, 0, 10_000, 5_000), 0);
        assert_eq!(
            submit_agent_job_as(&agent, 10, 1_000, &code_hash, &[0u8; 32]),
            42,
            "zero PQ action hash is rejected"
        );
        assert_eq!(
            submit_agent_job_as(&agent, 10, 1_000, &code_hash, &action_hash),
            43,
            "agent must opt into spending policy"
        );
        assert_eq!(
            set_agent_policy_as(&agent, 6_000, 4_000, &[0u8; 32], 1),
            3,
            "policy hash must be non-zero"
        );
        assert_eq!(
            set_agent_policy_as(&agent, 6_000, 4_000, &policy_hash, 1),
            0
        );

        test_mock::set_caller(attacker);
        assert_eq!(
            set_agent_spending_policy(agent.as_ptr(), 6_000, 4_000, policy_hash.as_ptr(), 2),
            200,
            "caller cannot spoof another agent policy"
        );

        test_mock::set_caller(agent);
        assert_eq!(disable_agent_spending_policy(agent.as_ptr()), 0);
        assert_eq!(
            submit_agent_job_as(&agent, 10, 1_000, &code_hash, &action_hash),
            43,
            "disabled policy blocks new payments"
        );
    }

    #[test]
    fn test_platform_fee_is_capped() {
        setup();
        let admin = [0xAD; 32];
        initialize_as(&admin);
        test_mock::set_caller(admin);

        assert_eq!(set_platform_fee(admin.as_ptr(), 1001), 2);
        assert!(test_mock::get_storage(b"platform_fee_bps").is_none());

        assert_eq!(set_platform_fee(admin.as_ptr(), 1000), 0);
        let fee = test_mock::get_storage(b"platform_fee_bps").unwrap();
        assert_eq!(bytes_to_u64(&fee), 1000);
    }
}
