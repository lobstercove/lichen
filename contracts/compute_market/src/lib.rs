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
    balance_of_token_or_native, bytes_to_u64, call_contract, get_caller, get_contract_address,
    get_slot, log_info, receive_token_or_native, storage_get, storage_set,
    transfer_token_or_native, u64_to_bytes, Address, CrossCall,
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
const CM_PLATFORM_FEE_BPS_KEY: &[u8] = b"platform_fee_bps";
const CM_FEE_TREASURY_KEY: &[u8] = b"cm_fee_treasury";
const CM_ACCOUNTING_VERSION_KEY: &[u8] = b"cm_account_version";
const CM_ESCROW_LIABILITY_KEY: &[u8] = b"cm_escrow_liability";
const CM_TOTAL_UNPAID_KEY: &[u8] = b"cm_total_unpaid";
const CM_MIGRATION_LOCK_KEY: &[u8] = b"cm_account_mig_lock";
const CM_MIGRATION_EXPECTED_COUNT_KEY: &[u8] = b"cm_account_mig_expected";
const CM_MIGRATION_CURSOR_KEY: &[u8] = b"cm_account_mig_cursor";
const CM_MIGRATION_ESCROW_KEY: &[u8] = b"cm_account_mig_escrow";
const CM_MIGRATION_UNPAID_KEY: &[u8] = b"cm_account_mig_unpaid";
const ACCOUNTING_VERSION_V3: u64 = 3;

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

fn job_metadata_key(prefix: &[u8], job_id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(prefix.len() + 20);
    key.extend_from_slice(prefix);
    key.extend_from_slice(&u64_to_decimal(job_id));
    key
}

fn job_token_key(job_id: u64) -> Vec<u8> {
    job_metadata_key(b"job_token_", job_id)
}

fn job_fee_bps_key(job_id: u64) -> Vec<u8> {
    job_metadata_key(b"job_fee_bps_", job_id)
}

fn job_claim_deadline_key(job_id: u64) -> Vec<u8> {
    job_metadata_key(b"job_claim_deadline_", job_id)
}

fn job_claimed_slot_key(job_id: u64) -> Vec<u8> {
    job_metadata_key(b"job_claimed_slot_", job_id)
}

fn job_complete_deadline_key(job_id: u64) -> Vec<u8> {
    job_metadata_key(b"job_complete_deadline_", job_id)
}

fn job_complete_timeout_key(job_id: u64) -> Vec<u8> {
    job_metadata_key(b"job_complete_timeout_", job_id)
}

fn job_challenge_deadline_key(job_id: u64) -> Vec<u8> {
    job_metadata_key(b"job_challenge_deadline_", job_id)
}

fn job_challenge_period_key(job_id: u64) -> Vec<u8> {
    job_metadata_key(b"job_challenge_period_", job_id)
}

fn job_payment_due_key(job_id: u64) -> Vec<u8> {
    job_metadata_key(b"job_payment_due_", job_id)
}

fn job_reserved_units_key(job_id: u64) -> Vec<u8> {
    job_metadata_key(b"job_reserved_units_", job_id)
}

fn job_capacity_released_key(job_id: u64) -> Vec<u8> {
    job_metadata_key(b"job_capacity_released_", job_id)
}

fn provider_reserved_key(provider: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(18 + 64);
    key.extend_from_slice(b"provider_reserved_");
    key.extend_from_slice(&hex_encode(provider));
    key
}

fn platform_fee_key(token: Address) -> Vec<u8> {
    let mut key = b"platform_fee:".to_vec();
    key.extend_from_slice(&token.0);
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

fn is_arbitrator(addr: &[u8; 32]) -> bool {
    let ak = arbitrator_key(addr);
    match storage_get(&ak) {
        Some(data) => !data.is_empty() && data[0] == 1,
        None => false,
    }
}

fn exact_u64_or_default(key: &[u8], default: u64) -> Option<u64> {
    match storage_get(key) {
        None => Some(default),
        Some(data) if data.len() == 8 => Some(bytes_to_u64(&data)),
        Some(_) => None,
    }
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

fn checked_stored_u64(key: &[u8]) -> Option<u64> {
    match storage_get(key) {
        None => Some(0),
        Some(data) if data.len() == 8 => Some(bytes_to_u64(&data)),
        Some(_) => None,
    }
}

fn accounting_version() -> u64 {
    checked_stored_u64(CM_ACCOUNTING_VERSION_KEY).unwrap_or(0)
}

fn migration_lock_valid() -> bool {
    matches!(storage_get(CM_MIGRATION_LOCK_KEY).as_deref(), None | Some([0]) | Some([1]))
}

fn migration_locked() -> bool {
    storage_get(CM_MIGRATION_LOCK_KEY).as_deref() == Some(&[1])
}

fn accounting_operational() -> bool {
    migration_lock_valid()
        && !migration_locked()
        && accounting_version() == ACCOUNTING_VERSION_V3
        && checked_stored_u64(CM_ESCROW_LIABILITY_KEY).is_some()
        && checked_stored_u64(CM_TOTAL_UNPAID_KEY).is_some()
}

fn stored_address(key: &[u8]) -> Option<[u8; 32]> {
    storage_get(key).and_then(|bytes| {
        if bytes.len() != 32 {
            return None;
        }
        let mut address = [0u8; 32];
        address.copy_from_slice(&bytes);
        Some(address)
    })
}

fn job_payment_token(job_id: u64) -> Option<[u8; 32]> {
    // New jobs snapshot their payment asset so a later configuration change
    // cannot redirect or strand existing escrow. Legacy jobs retain the old
    // global-token behavior.
    stored_address(&job_token_key(job_id)).or_else(load_token_address)
}

fn job_platform_fee_bps(job_id: u64) -> Option<u64> {
    // A missing snapshot identifies a legacy job. Charging the current fee
    // retroactively would violate its funded terms, so legacy jobs pay zero.
    exact_u64_or_default(&job_fee_bps_key(job_id), 0)
}

fn split_provider_payment(gross: u64, fee_bps: u64) -> Option<(u64, u64)> {
    if fee_bps > 10_000 {
        return None;
    }
    let fee = ((gross as u128).checked_mul(fee_bps as u128)? / 10_000) as u64;
    Some((gross.checked_sub(fee)?, fee))
}

fn job_payment_due(job_id: u64, escrowed: u64) -> Option<u64> {
    match storage_get(&job_payment_due_key(job_id)) {
        Some(bytes) if bytes.len() == 8 => {
            let due = bytes_to_u64(&bytes);
            (due <= escrowed).then_some(due)
        }
        Some(_) => None,
        // Legacy jobs paid their entire escrow. Preserve that settlement.
        None => Some(escrowed),
    }
}

/// Release capacity reserved by a v3 claim. Returns the provider reservation
/// key and its previous value so callers with a later fallible custody action
/// can restore the exact prior state.
fn release_job_capacity(job_id: u64, job_data: &[u8]) -> Result<Option<(Vec<u8>, u64)>, u32> {
    match exact_bool_or_default(&job_capacity_released_key(job_id), false) {
        Some(true) => return Ok(None),
        Some(false) => {}
        None => return Err(15),
    }
    let reserved_units = match storage_get(&job_reserved_units_key(job_id)) {
        Some(bytes) if bytes.len() == 8 => bytes_to_u64(&bytes),
        Some(_) => return Err(15),
        // Legacy claims did not reserve provider capacity.
        None => return Ok(None),
    };
    if job_data.len() != JOB_SIZE {
        return Err(2);
    }
    let mut provider = [0u8; 32];
    provider.copy_from_slice(&job_data[81..113]);
    let key = provider_reserved_key(&provider);
    let previous = checked_stored_u64(&key).ok_or(15u32)?;
    let remaining = previous.checked_sub(reserved_units).ok_or(15u32)?;
    storage_set(&key, &u64_to_bytes(remaining));
    storage_set(&job_capacity_released_key(job_id), &[1]);
    Ok(Some((key, previous)))
}

fn restore_job_capacity(job_id: u64, snapshot: Option<(Vec<u8>, u64)>) {
    if let Some((key, previous)) = snapshot {
        storage_set(&key, &u64_to_bytes(previous));
        storage_set(&job_capacity_released_key(job_id), &[0]);
    }
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

fn migrated_unpaid_recipient_key(recipient: &[u8; 32]) -> Vec<u8> {
    let mut key = b"cm_account_mig_recipient:".to_vec();
    key.extend_from_slice(recipient);
    key
}

fn signer_matches(addr: &[u8; 32]) -> bool {
    get_caller().0 == *addr
}

fn exact_bool_or_default(key: &[u8], default: bool) -> Option<bool> {
    match storage_get(key).as_deref() {
        None => Some(default),
        Some([0]) => Some(false),
        Some([1]) => Some(true),
        Some(_) => None,
    }
}

fn cm_paused() -> bool {
    match storage_get(b"cm_paused").as_deref() {
        None | Some([]) | Some([0]) => false,
        Some([1]) => true,
        Some(_) => true,
    }
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

fn agent_action_used_key(action_hash: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(18 + 64);
    key.extend_from_slice(b"agent_action_used:");
    key.extend_from_slice(&hex_encode(action_hash));
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
    storage_get(&agent_policy_key(agent)).filter(|data| data.len() == AGENT_POLICY_SIZE)
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
    if !nonzero_hash(hash_arr) {
        log_info("Code hash must be non-zero");
        return Err(15);
    }

    if !check_identity_gate(req_arr) {
        log_info("Insufficient LichenID reputation for job submission");
        return Err(10);
    }

    let job_id = match checked_stored_u64(b"job_count") {
        Some(value) => value,
        None => return Err(17),
    };
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
    if !accounting_operational() {
        log_info("Compute market accounting is not active");
        return Err(16);
    }
    let escrow_liability = match checked_stored_u64(CM_ESCROW_LIABILITY_KEY) {
        Some(value) => value,
        None => return Err(17),
    };
    let next_escrow_liability = match escrow_liability.checked_add(max_price) {
        Some(value) => value,
        None => return Err(17),
    };
    let fee_bps = match exact_u64_or_default(CM_PLATFORM_FEE_BPS_KEY, 0) {
        Some(value) if value <= 1_000 => value,
        _ => return Err(18),
    };
    let claim_timeout = match exact_u64_or_default(CLAIM_TIMEOUT_KEY, DEFAULT_CLAIM_TIMEOUT) {
        Some(value) => value,
        None => return Err(18),
    };
    let complete_timeout =
        match exact_u64_or_default(COMPLETE_TIMEOUT_KEY, DEFAULT_COMPLETE_TIMEOUT) {
            Some(value) => value,
            None => return Err(18),
        };
    let challenge_period =
        match exact_u64_or_default(CHALLENGE_PERIOD_KEY, DEFAULT_CHALLENGE_PERIOD) {
            Some(value) => value,
            None => return Err(18),
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
    storage_set(
        CM_ESCROW_LIABILITY_KEY,
        &u64_to_bytes(next_escrow_liability),
    );

    let current_slot = get_slot();
    let empty_address = [0u8; 32];
    let data = encode_job(JobEncoding {
        requester: req_arr,
        compute_units: compute_units_needed,
        max_price,
        code_hash: hash_arr,
        status: JOB_PENDING,
        provider: &empty_address,
        result_hash: &empty_address,
        created_slot: current_slot,
        completed_slot: 0,
    });

    let jk = job_key(job_id);
    storage_set(&jk, &data);

    let ek = escrow_key(job_id);
    storage_set(&ek, &u64_to_bytes(max_price));

    // Snapshot all funded-job terms. Administrative configuration changes are
    // prospective only and cannot alter an escrow that already exists.
    storage_set(&job_token_key(job_id), &token_addr);
    storage_set(
        &job_fee_bps_key(job_id),
        &u64_to_bytes(fee_bps),
    );
    storage_set(
        &job_claim_deadline_key(job_id),
        &u64_to_bytes(current_slot.saturating_add(claim_timeout)),
    );
    storage_set(
        &job_complete_timeout_key(job_id),
        &u64_to_bytes(complete_timeout),
    );
    storage_set(
        &job_challenge_period_key(job_id),
        &u64_to_bytes(challenge_period),
    );

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

struct JobEncoding<'a> {
    requester: &'a [u8; 32],
    compute_units: u64,
    max_price: u64,
    code_hash: &'a [u8; 32],
    status: u8,
    provider: &'a [u8; 32],
    result_hash: &'a [u8; 32],
    created_slot: u64,
    completed_slot: u64,
}

fn encode_job(job: JobEncoding<'_>) -> Vec<u8> {
    let mut data = Vec::with_capacity(JOB_SIZE);
    data.extend_from_slice(job.requester);
    data.extend_from_slice(&u64_to_bytes(job.compute_units));
    data.extend_from_slice(&u64_to_bytes(job.max_price));
    data.extend_from_slice(job.code_hash);
    data.push(job.status);
    data.extend_from_slice(job.provider);
    data.extend_from_slice(job.result_hash);
    data.extend_from_slice(&u64_to_bytes(job.created_slot));
    data.extend_from_slice(&u64_to_bytes(job.completed_slot));
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
    if cm_paused() {
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
    if cm_paused() {
        return 99;
    }
    if !accounting_operational() {
        return 90;
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
    if prov_data.len() != PROVIDER_SIZE {
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

    if job_data.len() != JOB_SIZE {
        log_info("Corrupt job data");
        return 3;
    }

    if job_data[80] != JOB_PENDING {
        log_info("Job is not in pending state");
        return 4;
    }

    let current_slot = get_slot();
    let created_slot = bytes_to_u64(&job_data[145..153]);
    let claim_timeout = match exact_u64_or_default(CLAIM_TIMEOUT_KEY, DEFAULT_CLAIM_TIMEOUT) {
        Some(value) => value,
        None => return 9,
    };
    let claim_deadline = match exact_u64_or_default(
        &job_claim_deadline_key(job_id),
        created_slot.saturating_add(claim_timeout),
    ) {
        Some(value) => value,
        None => return 9,
    };
    if current_slot > claim_deadline {
        log_info("Job claim deadline has expired");
        return 7;
    }

    let capacity = bytes_to_u64(&prov_data[32..40]);
    let reserved_key = provider_reserved_key(&prov_arr);
    let reserved = match checked_stored_u64(&reserved_key) {
        Some(value) => value,
        None => return 8,
    };
    let available = match capacity.checked_sub(reserved) {
        Some(value) => value,
        None => {
            log_info("Provider capacity accounting is inconsistent");
            return 8;
        }
    };
    let units_needed = bytes_to_u64(&job_data[32..40]);
    if units_needed > available {
        log_info("Provider has insufficient unreserved compute capacity");
        return 8;
    }
    let unit_price = bytes_to_u64(&prov_data[40..48]);
    let payment_due = match units_needed.checked_mul(unit_price) {
        Some(value) => value,
        None => {
            log_info("Provider quote overflow");
            return 9;
        }
    };
    let max_price = bytes_to_u64(&job_data[40..48]);
    if payment_due > max_price {
        log_info("Provider quote exceeds the requester maximum price");
        return 10;
    }
    let next_reserved = match reserved.checked_add(units_needed) {
        Some(value) if value <= capacity => value,
        _ => {
            log_info("Provider capacity reservation overflow");
            return 8;
        }
    };

    // Set provider and status = claimed
    job_data[80] = JOB_CLAIMED;
    job_data[81..113].copy_from_slice(&prov_arr);
    storage_set(&jk, &job_data);
    storage_set(&reserved_key, &u64_to_bytes(next_reserved));
    storage_set(&job_reserved_units_key(job_id), &u64_to_bytes(units_needed));
    storage_set(&job_capacity_released_key(job_id), &[0]);
    storage_set(&job_payment_due_key(job_id), &u64_to_bytes(payment_due));
    storage_set(
        &job_claimed_slot_key(job_id),
        &u64_to_bytes(current_slot),
    );
    let complete_timeout = match exact_u64_or_default(
        &job_complete_timeout_key(job_id),
        match exact_u64_or_default(COMPLETE_TIMEOUT_KEY, DEFAULT_COMPLETE_TIMEOUT) {
            Some(value) => value,
            None => return 9,
        },
    ) {
        Some(value) => value,
        None => return 9,
    };
    storage_set(
        &job_complete_deadline_key(job_id),
        &u64_to_bytes(current_slot.saturating_add(complete_timeout)),
    );

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
    if cm_paused() {
        return 99;
    }
    if !accounting_operational() {
        return 90;
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

    if job_data.len() != JOB_SIZE {
        return 2;
    }

    if job_data[80] != JOB_CLAIMED {
        log_info("Job is not in claimed state");
        return 3;
    }

    // Verify provider matches
    if job_data[81..113] != prov_arr[..] {
        log_info("Not the assigned provider");
        return 4;
    }

    if !nonzero_hash(&result_hash) {
        log_info("Result hash must be non-zero");
        return 6;
    }


    let current_slot = get_slot();
    let created_slot = bytes_to_u64(&job_data[145..153]);
    let complete_timeout =
        match exact_u64_or_default(COMPLETE_TIMEOUT_KEY, DEFAULT_COMPLETE_TIMEOUT) {
            Some(value) => value,
            None => return 7,
        };
    let complete_deadline = match exact_u64_or_default(
        &job_complete_deadline_key(job_id),
        created_slot.saturating_add(complete_timeout),
    ) {
        Some(value) => value,
        None => return 7,
    };
    if current_slot > complete_deadline {
        log_info("Job completion deadline has expired");
        return 5;
    }
    let challenge_period = match exact_u64_or_default(
        &job_challenge_period_key(job_id),
        match exact_u64_or_default(CHALLENGE_PERIOD_KEY, DEFAULT_CHALLENGE_PERIOD) {
            Some(value) => value,
            None => return 7,
        },
    ) {
        Some(value) => value,
        None => return 7,
    };

    if let Err(code) = release_job_capacity(job_id, &job_data) {
        log_info("Provider capacity release failed");
        return code;
    }

    // Set result and status = completed
    job_data[80] = JOB_COMPLETED;
    job_data[113..145].copy_from_slice(&result_hash);
    job_data[153..161].copy_from_slice(&u64_to_bytes(current_slot));
    storage_set(&jk, &job_data);
    storage_set(
        &job_challenge_deadline_key(job_id),
        &u64_to_bytes(current_slot.saturating_add(challenge_period)),
    );

    // Update provider stats
    let pk = provider_key(&prov_arr);
    if let Some(mut prov_data) = storage_get(&pk) {
        if prov_data.len() == PROVIDER_SIZE {
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

    if !accounting_operational() {
        return 90;
    }

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

    if job_data.len() != JOB_SIZE {
        return 2;
    }

    // Only requester can dispute
    if job_data[0..32] != requester[..] {
        log_info("Only requester can dispute");
        return 3;
    }

    if job_data[80] != JOB_COMPLETED {
        log_info("Job must be completed to dispute");
        return 4;
    }


    let completed_slot = bytes_to_u64(&job_data[153..161]);
    let challenge_period =
        match exact_u64_or_default(CHALLENGE_PERIOD_KEY, DEFAULT_CHALLENGE_PERIOD) {
            Some(value) => value,
            None => return 6,
        };
    let challenge_deadline = match exact_u64_or_default(
        &job_challenge_deadline_key(job_id),
        completed_slot.saturating_add(challenge_period),
    ) {
        Some(value) => value,
        None => return 6,
    };
    if get_slot() > challenge_deadline {
        log_info("Job challenge deadline has expired");
        return 5;
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
        Some(data) if data.len() == JOB_SIZE => {
            lichen_sdk::set_return_data(&data);
            0
        }
        None => {
            log_info("Job not found");
            1
        }
        Some(_) => 2,
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
    if admin.iter().all(|byte| *byte == 0) {
        log_info("Admin cannot be the zero address");
        return 2;
    }
    storage_set(ADMIN_KEY, &admin);
    storage_set(CM_FEE_TREASURY_KEY, &admin);
    // Identity configuration is governed by the protocol admin from genesis.
    // Legacy deployments can bind this key through `set_identity_admin`, which
    // requires the same already-initialized protocol admin.
    storage_set(IDENTITY_ADMIN_KEY, &admin);
    storage_set(CM_ESCROW_LIABILITY_KEY, &u64_to_bytes(0));
    storage_set(CM_TOTAL_UNPAID_KEY, &u64_to_bytes(0));
    storage_set(CM_MIGRATION_LOCK_KEY, &[0]);
    storage_set(
        CM_ACCOUNTING_VERSION_KEY,
        &u64_to_bytes(ACCOUNTING_VERSION_V3),
    );
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
    if addr.iter().all(|byte| *byte == 0) {
        log_info("Arbitrator cannot be the zero address");
        return 2;
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
    // Presence, not successful decoding, makes this immutable. A malformed
    // legacy value must fail closed and be repaired through a migration.
    if storage_get(CM_TOKEN_ADDRESS_KEY).is_some() {
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
    let existing = storage_get(&key);
    let existed = existing.is_some();
    let created_slot = match existing {
        Some(data) if data.len() == AGENT_POLICY_SIZE => {
            let current_version = bytes_to_u64(&data[0..8]);
            if policy_version <= current_version {
                return 4;
            }
            bytes_to_u64(&data[56..64])
        }
        Some(_) => return 5,
        None => get_slot(),
    };
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
        Some(data) if data.len() == AGENT_POLICY_SIZE => data,
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
    if exact_bool_or_default(CM_AGENT_PAYMENTS_ENABLED_KEY, false) != Some(true) {
        return 40;
    }
    if exact_bool_or_default(CM_AGENT_ROUTE_PAUSED_KEY, false) != Some(false) {
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
        return 42;
    }
    let policy = match read_agent_policy(&agent) {
        Some(data) if data[72] == 1 => data,
        _ => return 43,
    };
    let daily_cap = bytes_to_u64(&policy[8..16]);
    let per_task_cap = bytes_to_u64(&policy[16..24]);
    if max_price > per_task_cap {
        return 44;
    }

    let global_daily_cap = match checked_stored_u64(CM_AGENT_MAX_DAILY_CAP_KEY) {
        Some(value) => value,
        None => return 50,
    };
    let global_per_task_cap = match checked_stored_u64(CM_AGENT_MAX_PER_TASK_CAP_KEY) {
        Some(value) => value,
        None => return 50,
    };
    if global_per_task_cap > 0 && max_price > global_per_task_cap {
        return 45;
    }

    let window = current_agent_spend_window();
    let spend_key = agent_spend_key(&agent, window);
    let spent = match checked_stored_u64(&spend_key) {
        Some(value) => value,
        None => return 50,
    };
    let next_spent = match spent.checked_add(max_price) {
        Some(value) => value,
        None => return 46,
    };
    if next_spent > daily_cap {
        return 47;
    }
    if global_daily_cap > 0 && next_spent > global_daily_cap {
        return 48;
    }

    let action_used_key = agent_action_used_key(&action_hash);
    if storage_get(&action_used_key).is_some() {
        return 49;
    }
    let next_payment_count = match checked_stored_u64(CM_AGENT_PAYMENT_COUNT_KEY)
        .and_then(|value| value.checked_add(1))
    {
        Some(value) => value,
        None => return 50,
    };
    let next_payment_volume = match checked_stored_u64(CM_AGENT_PAYMENT_VOLUME_KEY)
        .and_then(|value| value.checked_add(max_price))
    {
        Some(value) => value,
        None => return 50,
    };

    match create_escrowed_job(&agent, compute_units_needed, max_price, &code_hash) {
        Ok(job_id) => {
            storage_set(&spend_key, &u64_to_bytes(next_spent));
            storage_set(&agent_job_action_key(job_id), &action_hash);
            // Mark the attested action only after escrow collection succeeds.
            // A failed submission therefore remains exactly retryable.
            storage_set(&action_used_key, &u64_to_bytes(job_id));
            storage_set(
                CM_AGENT_PAYMENT_COUNT_KEY,
                &u64_to_bytes(next_payment_count),
            );
            storage_set(
                CM_AGENT_PAYMENT_VOLUME_KEY,
                &u64_to_bytes(next_payment_volume),
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
    let enabled = match exact_bool_or_default(CM_AGENT_PAYMENTS_ENABLED_KEY, false) {
        Some(value) => value,
        None => return 2,
    };
    let route_paused = match exact_bool_or_default(CM_AGENT_ROUTE_PAUSED_KEY, false) {
        Some(value) => value,
        None => return 2,
    };
    let mut values = [0u64; 6];
    for (index, key) in [
        CM_AGENT_MAX_DAILY_CAP_KEY,
        CM_AGENT_MAX_PER_TASK_CAP_KEY,
        CM_AGENT_POLICY_COUNT_KEY,
        CM_AGENT_PAYMENT_COUNT_KEY,
        CM_AGENT_PAYMENT_VOLUME_KEY,
        CM_AGENT_BLOCKED_PAYMENT_COUNT_KEY,
    ]
    .into_iter()
    .enumerate()
    {
        values[index] = match checked_stored_u64(key) {
            Some(value) => value,
            None => return 2,
        };
    }
    let mut buf = Vec::with_capacity(50);
    buf.push(u8::from(enabled));
    buf.push(u8::from(route_paused));
    for value in values {
        buf.extend_from_slice(&u64_to_bytes(value));
    }
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
    match checked_stored_u64(&agent_spend_key(&agent, window)) {
        Some(value) => {
            lichen_sdk::set_return_data(&u64_to_bytes(value));
            0
        }
        None => 2,
    }
}

#[no_mangle]
pub extern "C" fn get_agent_job_action(job_id: u64) -> u32 {
    match storage_get(&agent_job_action_key(job_id)) {
        Some(data) if data.len() == 32 => {
            lichen_sdk::set_return_data(&data);
            0
        }
        None => 1,
        Some(_) => 2,
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

    if !accounting_operational() {
        return 90;
    }

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
    if job_data.len() != JOB_SIZE {
        return 2;
    }

    // Only requester can cancel
    if job_data[0..32] != requester[..] {
        log_info("Only requester can cancel");
        return 3;
    }

    let status = job_data[80];
    let created_slot = bytes_to_u64(&job_data[145..153]);
    let current_slot = get_slot();

    match status {
        JOB_PENDING => {
            // Must wait for claim timeout to give providers a chance
            let timeout = match exact_u64_or_default(CLAIM_TIMEOUT_KEY, DEFAULT_CLAIM_TIMEOUT) {
                Some(value) => value,
                None => return 10,
            };
            let deadline = match exact_u64_or_default(
                &job_claim_deadline_key(job_id),
                created_slot.saturating_add(timeout),
            ) {
                Some(value) => value,
                None => return 10,
            };
            if current_slot <= deadline {
                log_info("Claim timeout not yet expired — providers still have time");
                return 4;
            }
        }
        JOB_CLAIMED => {
            // Provider claimed but never completed — check complete timeout
            let timeout =
                match exact_u64_or_default(COMPLETE_TIMEOUT_KEY, DEFAULT_COMPLETE_TIMEOUT) {
                    Some(value) => value,
                    None => return 10,
                };
            let deadline = match exact_u64_or_default(
                &job_complete_deadline_key(job_id),
                created_slot.saturating_add(timeout),
            ) {
                Some(value) => value,
                None => return 10,
            };
            if current_slot <= deadline {
                log_info("Complete timeout not yet expired");
                return 5;
            }
        }
        _ => {
            log_info("Job cannot be cancelled in current state");
            return 6;
        }
    }

    let ek = escrow_key(job_id);
    let escrowed = match storage_get(&ek) {
        Some(bytes) if bytes.len() == 8 => bytes_to_u64(&bytes),
        _ => 0,
    };
    if escrowed == 0 {
        log_info("cancel_job: funded job has no escrow");
        return 9;
    }
    let escrow_liability_before = match checked_stored_u64(CM_ESCROW_LIABILITY_KEY) {
        Some(value) => value,
        None => return 10,
    };
    let escrow_liability_after = match escrow_liability_before.checked_sub(escrowed) {
        Some(value) => value,
        None => return 10,
    };

    let capacity_snapshot = match release_job_capacity(job_id, &job_data) {
        Ok(snapshot) => snapshot,
        Err(code) => {
            log_info("cancel_job: provider capacity release failed");
            return code;
        }
    };

    // Cancel and clear escrow. If the external refund fails, restore accounting
    // so the requester can retry without losing their escrow claim.
    let previous_status = job_data[80];
    job_data[80] = JOB_CANCELLED;
    storage_set(&jk, &job_data);
    storage_set(&ek, &u64_to_bytes(0));
    storage_set(
        CM_ESCROW_LIABILITY_KEY,
        &u64_to_bytes(escrow_liability_after),
    );

    // AUDIT-FIX H-2: Return escrowed tokens to requester
    if escrowed > 0 {
        let token_addr = match job_payment_token(job_id) {
            Some(addr) => addr,
            None => {
                job_data[80] = previous_status;
                storage_set(&jk, &job_data);
                storage_set(&ek, &u64_to_bytes(escrowed));
                storage_set(
                    CM_ESCROW_LIABILITY_KEY,
                    &u64_to_bytes(escrow_liability_before),
                );
                restore_job_capacity(job_id, capacity_snapshot);
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
                storage_set(
                    CM_ESCROW_LIABILITY_KEY,
                    &u64_to_bytes(escrow_liability_before),
                );
                restore_job_capacity(job_id, capacity_snapshot);
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

    if !accounting_operational() {
        return 90;
    }

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
    if job_data.len() != JOB_SIZE {
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

    let current_slot = get_slot();
    let challenge_period =
        match exact_u64_or_default(CHALLENGE_PERIOD_KEY, DEFAULT_CHALLENGE_PERIOD) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 12;
            }
        };
    let challenge_deadline = match exact_u64_or_default(
        &job_challenge_deadline_key(job_id),
        completed_slot.saturating_add(challenge_period),
    ) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    if current_slot <= challenge_deadline {
        log_info("Challenge period not yet expired");
        reentrancy_exit();
        return 5;
    }

    let ek = escrow_key(job_id);
    let escrowed = match storage_get(&ek) {
        Some(bytes) if bytes.len() == 8 => bytes_to_u64(&bytes),
        _ => 0,
    };
    if escrowed == 0 {
        log_info("release_payment: funded job has no escrow");
        reentrancy_exit();
        return 9;
    }
    let escrow_liability_before = match checked_stored_u64(CM_ESCROW_LIABILITY_KEY) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 10;
        }
    };
    let escrow_liability_after = match escrow_liability_before.checked_sub(escrowed) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 10;
        }
    };
    let token_addr = if escrowed > 0 {
        match job_payment_token(job_id) {
            Some(addr) => Some(addr),
            None => {
                log_info("release_payment: payment token configuration invalid");
                reentrancy_exit();
                return 6;
            }
        }
    } else {
        None
    };
    let payment_due = match job_payment_due(job_id, escrowed) {
        Some(amount) => amount,
        None => {
            log_info("release_payment: agreed payment exceeds escrow");
            reentrancy_exit();
            return 8;
        }
    };
    let requester_refund = escrowed - payment_due;
    let (provider_payment, platform_fee) =
        match job_platform_fee_bps(job_id).and_then(|fee| split_provider_payment(payment_due, fee)) {
            Some(split) => split,
            None => {
                log_info("release_payment: invalid snapshotted platform fee");
                reentrancy_exit();
                return 7;
            }
        };
    let next_platform_fee = token_addr.and_then(|addr| {
        checked_stored_u64(&platform_fee_key(Address(addr)))?.checked_add(platform_fee)
    });
    if token_addr.is_some() && next_platform_fee.is_none() {
        log_info("release_payment: platform fee accounting overflow");
        reentrancy_exit();
        return 7;
    }
    let mut requester_arr = [0u8; 32];
    requester_arr.copy_from_slice(&job_data[0..32]);
    let next_unpaid_refund = token_addr.and_then(|addr| {
        checked_stored_u64(&unpaid_payout_key(Address(addr), Address(requester_arr)))?
            .checked_add(requester_refund)
    });
    if token_addr.is_some() && next_unpaid_refund.is_none() {
        log_info("release_payment: requester refund accounting overflow");
        reentrancy_exit();
        return 8;
    }
    let next_total_unpaid = match checked_stored_u64(CM_TOTAL_UNPAID_KEY)
        .and_then(|total| total.checked_add(requester_refund))
    {
        Some(value) => value,
        None => {
            log_info("release_payment: total unpaid accounting overflow");
            reentrancy_exit();
            return 8;
        }
    };

    // Mark as released and clear escrow. On failed payout, restore the
    // completed job and escrow so release can be retried exactly.
    job_data[80] = JOB_RELEASED;
    storage_set(&jk, &job_data);
    storage_set(&ek, &u64_to_bytes(0));
    storage_set(
        CM_ESCROW_LIABILITY_KEY,
        &u64_to_bytes(escrow_liability_after),
    );

    // Pay the provider net of the fee snapshotted when the job was funded.
    let mut provider_paid = false;
    if provider_payment > 0 {
        let mut provider_arr = [0u8; 32];
        provider_arr.copy_from_slice(&job_data[81..113]);
        let token_addr = match token_addr {
            Some(address) => address,
            None => {
                storage_set(&ek, &u64_to_bytes(escrowed));
                storage_set(
                    CM_ESCROW_LIABILITY_KEY,
                    &u64_to_bytes(escrow_liability_before),
                );
                job_data[80] = JOB_COMPLETED;
                storage_set(&jk, &job_data);
                log_info("release_payment: funded job has no payment token");
                reentrancy_exit();
                return 6;
            }
        };
        let contract_addr = get_contract_address();
        match transfer_token_or_native(
            Address(token_addr),
            contract_addr,
            Address(provider_arr),
            provider_payment,
        ) {
            Ok(true) => {
                provider_paid = true;
            }
            Ok(false) | Err(_) => {
                storage_set(&ek, &u64_to_bytes(escrowed));
                storage_set(
                    CM_ESCROW_LIABILITY_KEY,
                    &u64_to_bytes(escrow_liability_before),
                );
                job_data[80] = JOB_COMPLETED;
                storage_set(&jk, &job_data);
                log_info("release_payment: token transfer to provider failed");
                reentrancy_exit();
                return 6;
            }
        }
    }
    if requester_refund > 0 {
        let token_addr = match token_addr {
            Some(address) => address,
            None => {
                storage_set(&ek, &u64_to_bytes(escrowed));
                storage_set(
                    CM_ESCROW_LIABILITY_KEY,
                    &u64_to_bytes(escrow_liability_before),
                );
                job_data[80] = JOB_COMPLETED;
                storage_set(&jk, &job_data);
                log_info("release_payment: funded job has no refund token");
                reentrancy_exit();
                return 6;
            }
        };
        match transfer_token_or_native(
            Address(token_addr),
            get_contract_address(),
            Address(requester_arr),
            requester_refund,
        ) {
            Ok(true) => {}
            Ok(false) | Err(_) if provider_paid => {
                let unpaid_key = unpaid_payout_key(Address(token_addr), Address(requester_arr));
                storage_set(
                    &unpaid_key,
                    &u64_to_bytes(next_unpaid_refund.unwrap_or(requester_refund)),
                );
                storage_set(CM_TOTAL_UNPAID_KEY, &u64_to_bytes(next_total_unpaid));
            }
            Ok(false) | Err(_) => {
                storage_set(&ek, &u64_to_bytes(escrowed));
                storage_set(
                    CM_ESCROW_LIABILITY_KEY,
                    &u64_to_bytes(escrow_liability_before),
                );
                job_data[80] = JOB_COMPLETED;
                storage_set(&jk, &job_data);
                log_info("release_payment: token refund to requester failed");
                reentrancy_exit();
                return 6;
            }
        }
    }
    if let (Some(token_addr), Some(next_fee)) = (token_addr, next_platform_fee) {
        storage_set(&platform_fee_key(Address(token_addr)), &u64_to_bytes(next_fee));
    }

    increment_counter_saturating(CM_COMPLETED_COUNT_KEY);
    let cmv = stored_u64(CM_PAYMENT_VOLUME_KEY);
    storage_set(
        CM_PAYMENT_VOLUME_KEY,
        &u64_to_bytes(cmv.saturating_add(payment_due)),
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
/// - requester_pct: percentage (0-100) of the agreed provider payment awarded
///   to requester. Unused max-price escrow is always refunded to requester.
#[no_mangle]
pub extern "C" fn resolve_dispute(
    arbitrator_ptr: *const u8,
    job_id: u64,
    requester_pct: u64,
) -> u32 {
    log_info("Resolving dispute...");

    if !accounting_operational() {
        return 90;
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
    if job_data.len() != JOB_SIZE {
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
    let escrowed = match storage_get(&ek) {
        Some(bytes) if bytes.len() == 8 => bytes_to_u64(&bytes),
        _ => 0,
    };
    if escrowed == 0 {
        log_info("resolve_dispute: funded job has no escrow");
        reentrancy_exit();
        return 10;
    }
    let escrow_liability_before = match checked_stored_u64(CM_ESCROW_LIABILITY_KEY) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 11;
        }
    };
    let escrow_liability_after = match escrow_liability_before.checked_sub(escrowed) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 11;
        }
    };

    let payment_due = match job_payment_due(job_id, escrowed) {
        Some(amount) => amount,
        None => {
            log_info("resolve_dispute: agreed payment exceeds escrow");
            reentrancy_exit();
            return 9;
        }
    };
    let unused_budget = escrowed - payment_due;
    let requester_award = (payment_due as u128 * requester_pct as u128 / 100) as u64;
    let to_requester = match unused_budget.checked_add(requester_award) {
        Some(amount) => amount,
        None => {
            reentrancy_exit();
            return 9;
        }
    };
    let provider_gross = payment_due.saturating_sub(requester_award);
    let (to_provider, platform_fee) = match job_platform_fee_bps(job_id)
        .and_then(|fee| split_provider_payment(provider_gross, fee))
    {
            Some(split) => split,
            None => {
                log_info("resolve_dispute: invalid snapshotted platform fee");
                reentrancy_exit();
                return 9;
            }
    };

    // AUDIT-FIX: Actually transfer tokens to both parties (using shared helper)
    let mut requester_arr = [0u8; 32];
    requester_arr.copy_from_slice(&job_data[0..32]);
    let mut provider_arr = [0u8; 32];
    provider_arr.copy_from_slice(&job_data[81..113]);
    let token_addr = if escrowed > 0 {
        match job_payment_token(job_id) {
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

    let mut provider_deferred = false;
    if let Some(token_addr) = token_addr {
        let token = Address(token_addr);
        let contract_addr = get_contract_address();
        let fee_key = platform_fee_key(token);
        let next_platform_fee = match checked_stored_u64(&fee_key)
            .and_then(|current| current.checked_add(platform_fee))
        {
            Some(next) => next,
            None => {
                log_info("resolve_dispute: platform fee accounting overflow");
                reentrancy_exit();
                return 9;
            }
        };
        let provider_unpaid_key = unpaid_payout_key(token, Address(provider_arr));
        let next_provider_unpaid = match checked_stored_u64(&provider_unpaid_key)
            .and_then(|current| current.checked_add(to_provider))
        {
            Some(next) => next,
            None => {
                log_info("resolve_dispute: unpaid provider accounting overflow");
                reentrancy_exit();
                return 9;
            }
        };
        let next_total_unpaid = match checked_stored_u64(CM_TOTAL_UNPAID_KEY)
            .and_then(|current| current.checked_add(to_provider))
        {
            Some(next) => next,
            None => {
                log_info("resolve_dispute: total unpaid accounting overflow");
                reentrancy_exit();
                return 9;
            }
        };
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
                        storage_set(&provider_unpaid_key, &u64_to_bytes(next_provider_unpaid));
                        storage_set(CM_TOTAL_UNPAID_KEY, &u64_to_bytes(next_total_unpaid));
                        provider_deferred = true;
                    } else {
                        log_info("resolve_dispute: transfer to provider failed");
                        reentrancy_exit();
                        return 7;
                    }
                }
            }
        }
        storage_set(&fee_key, &u64_to_bytes(next_platform_fee));
    }

    // Mark resolved and clear escrow
    job_data[80] = JOB_RESOLVED;
    storage_set(&jk, &job_data);
    storage_set(&ek, &u64_to_bytes(0));
    storage_set(
        CM_ESCROW_LIABILITY_KEY,
        &u64_to_bytes(escrow_liability_after),
    );

    increment_counter_saturating(CM_COMPLETED_COUNT_KEY);
    storage_set(
        CM_PAYMENT_VOLUME_KEY,
        &u64_to_bytes(stored_u64(CM_PAYMENT_VOLUME_KEY).saturating_add(payment_due)),
    );

    if provider_deferred {
        log_info("Dispute resolved with deferred provider payout");
    }

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
    if prov_data.len() != PROVIDER_SIZE {
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
    if prov_data.len() != PROVIDER_SIZE {
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
    if prov_data.len() != PROVIDER_SIZE {
        return 2;
    }
    if compute_units == 0 || price_per_unit == 0 {
        log_info("Values must be > 0");
        return 3;
    }
    let reserved = match checked_stored_u64(&provider_reserved_key(&addr)) {
        Some(value) => value,
        None => return 5,
    };
    if compute_units < reserved {
        log_info("Capacity cannot be reduced below active reservations");
        return 4;
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
        Some(data) if data.len() == 8 => {
            lichen_sdk::set_return_data(&data);
            0
        }
        None => 1,
        Some(_) => 2,
    }
}

/// Claim an unpaid payout recorded after a partial dispute split.
/// This remains available while paused so recipients can exit after restrictions lift.
///
/// Returns: 0 success, 2 nothing owed, 20 reentrancy, 32 transfer failed,
///          98 invalid pointer, 200 caller spoofing.
#[no_mangle]
pub extern "C" fn claim_unpaid_payout(caller_ptr: *const u8, token_ptr: *const u8) -> u32 {
    if !accounting_operational() {
        return 90;
    }
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
    if load_token_address() != Some(token.0) {
        reentrancy_exit();
        return 3;
    }
    let recipient = Address(caller);
    let key = unpaid_payout_key(token, recipient);
    let amount = match checked_stored_u64(&key) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 4;
        }
    };
    if amount == 0 {
        reentrancy_exit();
        return 2;
    }
    let total_unpaid_before = match checked_stored_u64(CM_TOTAL_UNPAID_KEY) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 4;
        }
    };
    let total_unpaid_after = match total_unpaid_before.checked_sub(amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 4;
        }
    };

    storage_set(&key, &u64_to_bytes(0));
    storage_set(CM_TOTAL_UNPAID_KEY, &u64_to_bytes(total_unpaid_after));
    match transfer_token_or_native(token, get_contract_address(), recipient, amount) {
        Ok(true) => {
            lichen_sdk::set_return_data(&u64_to_bytes(amount));
            reentrancy_exit();
            0
        }
        Ok(false) | Err(_) => {
            storage_set(&key, &u64_to_bytes(amount));
            storage_set(CM_TOTAL_UNPAID_KEY, &u64_to_bytes(total_unpaid_before));
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

/// Bind identity/reputation configuration to the existing protocol admin on a
/// legacy deployment. Fresh deployments do this atomically in `initialize`.
#[no_mangle]
pub extern "C" fn set_identity_admin(admin_ptr: *const u8) -> u32 {
    let admin = match read_address32(admin_ptr) {
        Some(v) => v,
        None => {
            log_info("set_identity_admin rejected: null admin_ptr");
            return 98;
        }
    };

    if !signer_matches(&admin) {
        return 200;
    }
    if storage_get(IDENTITY_ADMIN_KEY).is_some() {
        log_info("Identity admin already set");
        return 1;
    }
    if admin.iter().all(|byte| *byte == 0) {
        return 2;
    }
    let protocol_admin = match stored_address(ADMIN_KEY) {
        Some(address) => address,
        None => return 3,
    };
    if admin != protocol_admin {
        log_info("Identity admin must equal the initialized protocol admin");
        return 4;
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

    let admin = match stored_address(IDENTITY_ADMIN_KEY) {
        Some(data) => data,
        None => return 1,
    };
    if caller != admin {
        return 2;
    }
    if lichenid_addr.iter().all(|&b| b == 0) {
        log_info("Cannot set zero LichenID address");
        return 3;
    }
    // Presence makes this dependency immutable. Malformed legacy state must be
    // repaired explicitly instead of silently replacing a security boundary.
    if storage_get(LICHENID_ADDR_KEY).is_some() {
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

    let admin = match stored_address(IDENTITY_ADMIN_KEY) {
        Some(data) => data,
        None => return 1,
    };
    if caller != admin {
        return 2;
    }
    if min_reputation > 0 {
        let configured = match stored_address(LICHENID_ADDR_KEY) {
            Some(address) => address,
            None => return 3,
        };
        if configured.iter().all(|byte| *byte == 0) {
            return 3;
        }
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
    if !accounting_operational() {
        return 3;
    }
    storage_set(b"cm_paused", &[]);
    log_info("Compute market unpaused");
    0
}

/// Check if caller meets the LichenID reputation threshold.
/// Returns true if no gate is set or caller meets threshold.
fn check_identity_gate(caller: &[u8]) -> bool {
    let min_rep = match storage_get(LICHENID_MIN_REP_KEY) {
        None => return true,
        Some(data) if data.len() == 8 => bytes_to_u64(&data),
        Some(_) => return false,
    };
    if min_rep == 0 {
        return true;
    }

    let addr = match stored_address(LICHENID_ADDR_KEY) {
        Some(address) if address.iter().any(|byte| *byte != 0) => address,
        _ => return false,
    };
    let target = Address::new(addr);
    let mut args = Vec::with_capacity(32);
    args.extend_from_slice(caller);
    let call = CrossCall::new(target, "get_reputation", args);

    match call_contract(call) {
        Ok(result) if result.len() == 8 => {
            let reputation = bytes_to_u64(&result);
            reputation >= min_rep
        }
        _ => false,
    }
}

// ============================================================================
// ACCOUNTING V3 MIGRATION AND SOLVENCY
// ============================================================================

fn migration_recipient_unpaid(
    token: Address,
    recipient: &[u8; 32],
) -> Result<(u64, bool), u32> {
    if recipient.iter().all(|byte| *byte == 0) {
        return Ok((0, false));
    }
    let marker = migrated_unpaid_recipient_key(recipient);
    match storage_get(&marker) {
        None => checked_stored_u64(&unpaid_payout_key(token, Address(*recipient)))
            .map(|amount| (amount, true))
            .ok_or(8),
        Some(value) if value.as_slice() == [1] => Ok((0, false)),
        Some(_) => Err(8),
    }
}

/// Freeze a legacy deployment and bind migration to the immutable contiguous
/// job frontier. The market remains paused after completion for independent
/// verification and an explicit unpause.
#[no_mangle]
pub extern "C" fn begin_accounting_v3_migration(
    caller_ptr: *const u8,
    expected_job_count: u64,
) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(value) => value,
        None => return 98,
    };
    if !signer_matches(&caller) {
        return 200;
    }
    if !is_admin(&caller) {
        return 1;
    }
    if accounting_version() == ACCOUNTING_VERSION_V3 {
        return 2;
    }
    if !migration_lock_valid() {
        return 8;
    }
    if checked_stored_u64(b"job_count") != Some(expected_job_count) {
        return 3;
    }
    if load_token_address().is_none() {
        return 4;
    }
    if migration_locked() {
        return if checked_stored_u64(CM_MIGRATION_EXPECTED_COUNT_KEY)
            == Some(expected_job_count)
        {
            0
        } else {
            5
        };
    }

    storage_set(b"cm_paused", &[1]);
    storage_set(CM_MIGRATION_LOCK_KEY, &[1]);
    storage_set(
        CM_MIGRATION_EXPECTED_COUNT_KEY,
        &u64_to_bytes(expected_job_count),
    );
    storage_set(CM_MIGRATION_CURSOR_KEY, &u64_to_bytes(0));
    storage_set(CM_MIGRATION_ESCROW_KEY, &u64_to_bytes(0));
    storage_set(CM_MIGRATION_UNPAID_KEY, &u64_to_bytes(0));
    0
}

/// Reconstruct the next exact legacy job. The cursor makes this permissionless,
/// deterministic, resumable, and conflict-aborting.
#[no_mangle]
pub extern "C" fn migrate_accounting_v3_job(job_id: u64) -> u32 {
    if !migration_locked() || accounting_version() == ACCOUNTING_VERSION_V3 {
        return 1;
    }
    let cursor = match checked_stored_u64(CM_MIGRATION_CURSOR_KEY) {
        Some(value) => value,
        None => return 8,
    };
    let expected = match checked_stored_u64(CM_MIGRATION_EXPECTED_COUNT_KEY) {
        Some(value) => value,
        None => return 8,
    };
    if job_id != cursor || job_id >= expected {
        return 2;
    }
    let job = match storage_get(&job_key(job_id)) {
        Some(value) if value.len() == JOB_SIZE => value,
        _ => return 3,
    };
    let status = job[80];
    if status > JOB_RELEASED {
        return 4;
    }
    let token = match load_token_address() {
        Some(value) => value,
        None => return 5,
    };
    let job_token = match storage_get(&job_token_key(job_id)) {
        None => token,
        Some(value) if value.len() == 32 => {
            let mut address = [0u8; 32];
            address.copy_from_slice(&value);
            address
        }
        Some(_) => return 5,
    };
    if job_token != token {
        return 5;
    }
    let escrow = match storage_get(&escrow_key(job_id)) {
        Some(value) if value.len() == 8 => bytes_to_u64(&value),
        _ => return 6,
    };
    let active = matches!(status, JOB_PENDING | JOB_CLAIMED | JOB_COMPLETED | JOB_DISPUTED);
    if (active && escrow == 0) || (!active && escrow != 0) {
        return 6;
    }
    let current_escrow = match checked_stored_u64(CM_MIGRATION_ESCROW_KEY) {
        Some(value) => value,
        None => return 8,
    };
    let next_escrow = match current_escrow.checked_add(escrow) {
        Some(value) => value,
        None => return 7,
    };

    let mut requester = [0u8; 32];
    requester.copy_from_slice(&job[0..32]);
    let mut provider = [0u8; 32];
    provider.copy_from_slice(&job[81..113]);
    let token = Address(token);
    let (requester_unpaid, mark_requester) =
        match migration_recipient_unpaid(token, &requester) {
            Ok(value) => value,
            Err(code) => return code,
        };
    let (provider_unpaid, mark_provider) = if provider == requester {
        (0, false)
    } else {
        match migration_recipient_unpaid(token, &provider) {
            Ok(value) => value,
            Err(code) => return code,
        }
    };
    let current_unpaid = match checked_stored_u64(CM_MIGRATION_UNPAID_KEY) {
        Some(value) => value,
        None => return 8,
    };
    let next_unpaid = match current_unpaid
        .checked_add(requester_unpaid)
        .and_then(|value| value.checked_add(provider_unpaid))
    {
        Some(value) => value,
        None => return 7,
    };
    let next_cursor = match cursor.checked_add(1) {
        Some(value) => value,
        None => return 7,
    };

    if mark_requester {
        storage_set(&migrated_unpaid_recipient_key(&requester), &[1]);
    }
    if mark_provider {
        storage_set(&migrated_unpaid_recipient_key(&provider), &[1]);
    }
    storage_set(CM_MIGRATION_ESCROW_KEY, &u64_to_bytes(next_escrow));
    storage_set(CM_MIGRATION_UNPAID_KEY, &u64_to_bytes(next_unpaid));
    storage_set(CM_MIGRATION_CURSOR_KEY, &u64_to_bytes(next_cursor));
    0
}

/// Activate V3 only when the full job frontier was reconstructed, independent
/// operator totals match, and custody covers escrow, fees, and deferred payouts.
#[no_mangle]
pub extern "C" fn complete_accounting_v3_migration(
    caller_ptr: *const u8,
    expected_escrow: u64,
    expected_unpaid: u64,
    expected_platform_fees: u64,
    expected_total_liability: u64,
) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(value) => value,
        None => return 98,
    };
    if !signer_matches(&caller) {
        return 200;
    }
    if !is_admin(&caller) {
        return 1;
    }
    if !migration_locked() || accounting_version() == ACCOUNTING_VERSION_V3 {
        return 2;
    }
    let expected_count = match checked_stored_u64(CM_MIGRATION_EXPECTED_COUNT_KEY) {
        Some(value) => value,
        None => return 8,
    };
    if checked_stored_u64(CM_MIGRATION_CURSOR_KEY) != Some(expected_count)
        || checked_stored_u64(b"job_count") != Some(expected_count)
    {
        return 3;
    }
    let escrow = match checked_stored_u64(CM_MIGRATION_ESCROW_KEY) {
        Some(value) => value,
        None => return 8,
    };
    let unpaid = match checked_stored_u64(CM_MIGRATION_UNPAID_KEY) {
        Some(value) => value,
        None => return 8,
    };
    let token = match load_token_address() {
        Some(value) => Address(value),
        None => return 4,
    };
    let platform_fees = match checked_stored_u64(&platform_fee_key(token)) {
        Some(value) => value,
        None => return 8,
    };
    let total = match escrow
        .checked_add(unpaid)
        .and_then(|value| value.checked_add(platform_fees))
    {
        Some(value) => value,
        None => return 7,
    };
    if escrow != expected_escrow
        || unpaid != expected_unpaid
        || platform_fees != expected_platform_fees
        || total != expected_total_liability
    {
        return 5;
    }
    let custody = match balance_of_token_or_native(token, get_contract_address()) {
        Ok(value) => value,
        Err(_) => return 6,
    };
    if custody < total {
        return 9;
    }

    storage_set(CM_ESCROW_LIABILITY_KEY, &u64_to_bytes(escrow));
    storage_set(CM_TOTAL_UNPAID_KEY, &u64_to_bytes(unpaid));
    storage_set(
        CM_ACCOUNTING_VERSION_KEY,
        &u64_to_bytes(ACCOUNTING_VERSION_V3),
    );
    storage_set(CM_MIGRATION_LOCK_KEY, &[0]);
    0
}

/// Return expected job count, cursor, reconstructed escrow, reconstructed
/// unpaid total, accounting version, and migration lock as six u64 values.
#[no_mangle]
pub extern "C" fn get_accounting_migration_status() -> u32 {
    let expected = match checked_stored_u64(CM_MIGRATION_EXPECTED_COUNT_KEY) {
        Some(value) => value,
        None => return 2,
    };
    let cursor = match checked_stored_u64(CM_MIGRATION_CURSOR_KEY) {
        Some(value) => value,
        None => return 2,
    };
    let escrow = match checked_stored_u64(CM_MIGRATION_ESCROW_KEY) {
        Some(value) => value,
        None => return 2,
    };
    let unpaid = match checked_stored_u64(CM_MIGRATION_UNPAID_KEY) {
        Some(value) => value,
        None => return 2,
    };
    let version = match checked_stored_u64(CM_ACCOUNTING_VERSION_KEY) {
        Some(value) => value,
        None => return 2,
    };
    if !migration_lock_valid() {
        return 2;
    }
    let mut result = Vec::with_capacity(48);
    for value in [
        expected,
        cursor,
        escrow,
        unpaid,
        version,
        u64::from(migration_locked()),
    ] {
        result.extend_from_slice(&u64_to_bytes(value));
    }
    lichen_sdk::set_return_data(&result);
    0
}

/// Return accounting version, migration lock, active escrow, deferred payouts,
/// platform fees, total liability, custody, and solvent flag as eight u64 values.
#[no_mangle]
pub extern "C" fn get_accounting_health() -> u32 {
    let token = match load_token_address() {
        Some(value) => Address(value),
        None => return 1,
    };
    let escrow = match checked_stored_u64(CM_ESCROW_LIABILITY_KEY) {
        Some(value) => value,
        None => return 2,
    };
    let unpaid = match checked_stored_u64(CM_TOTAL_UNPAID_KEY) {
        Some(value) => value,
        None => return 2,
    };
    let fees = match checked_stored_u64(&platform_fee_key(token)) {
        Some(value) => value,
        None => return 2,
    };
    let total = match escrow
        .checked_add(unpaid)
        .and_then(|value| value.checked_add(fees))
    {
        Some(value) => value,
        None => return 3,
    };
    let custody = match balance_of_token_or_native(token, get_contract_address()) {
        Ok(value) => value,
        Err(_) => return 4,
    };
    let mut result = Vec::with_capacity(64);
    for value in [
        accounting_version(),
        u64::from(migration_locked()),
        escrow,
        unpaid,
        fees,
        total,
        custody,
        u64::from(accounting_operational() && custody >= total),
    ] {
        result.extend_from_slice(&u64_to_bytes(value));
    }
    lichen_sdk::set_return_data(&result);
    0
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
    checked_stored_u64(b"job_count").unwrap_or(0)
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
        Some(data) if data.len() == PROVIDER_SIZE => {
            lichen_sdk::set_return_data(&data);
            0
        }
        None => 1,
        Some(_) => 2,
    }
}

/// Query total, reserved, and currently available provider capacity.
#[no_mangle]
pub extern "C" fn get_provider_capacity(provider_ptr: *const u8) -> u32 {
    let provider = match read_address32(provider_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let provider_data = match storage_get(&provider_key(&provider)) {
        Some(data) if data.len() == PROVIDER_SIZE => data,
        _ => return 1,
    };
    let total = bytes_to_u64(&provider_data[32..40]);
    let reserved = match checked_stored_u64(&provider_reserved_key(&provider)) {
        Some(value) => value,
        None => return 2,
    };
    let available = match total.checked_sub(reserved) {
        Some(value) => value,
        None => return 2,
    };
    let mut capacity = Vec::with_capacity(24);
    capacity.extend_from_slice(&u64_to_bytes(total));
    capacity.extend_from_slice(&u64_to_bytes(reserved));
    capacity.extend_from_slice(&u64_to_bytes(available));
    lichen_sdk::set_return_data(&capacity);
    0
}

/// Tests expect `set_platform_fee`
#[no_mangle]
pub extern "C" fn set_platform_fee(caller_ptr: *const u8, fee_bps: u64) -> u32 {
    if migration_locked() {
        return 90;
    }
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
    storage_set(CM_PLATFORM_FEE_BPS_KEY, &u64_to_bytes(fee_bps));
    log_info("Platform fee set");
    0
}

/// Admin sets the recipient for accrued platform-fee withdrawals.
#[no_mangle]
pub extern "C" fn set_fee_treasury(caller_ptr: *const u8, treasury_ptr: *const u8) -> u32 {
    if migration_locked() {
        return 90;
    }
    let caller = match read_address32(caller_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let treasury = match read_address32(treasury_ptr) {
        Some(address) => address,
        None => return 98,
    };
    if !signer_matches(&caller) {
        return 200;
    }
    if !is_admin(&caller) {
        return 1;
    }
    if treasury.iter().all(|byte| *byte == 0) {
        return 2;
    }
    storage_set(CM_FEE_TREASURY_KEY, &treasury);
    0
}

/// Withdraw an exact amount of realized platform fees to the configured treasury.
/// The balance is restored if the custody transfer fails, making retries exact.
#[no_mangle]
pub extern "C" fn withdraw_platform_fees(
    caller_ptr: *const u8,
    token_ptr: *const u8,
    amount: u64,
) -> u32 {
    if !accounting_operational() {
        return 90;
    }
    if !reentrancy_enter() {
        return 20;
    }
    let caller = match read_address32(caller_ptr) {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 98;
        }
    };
    let token = match read_address32(token_ptr) {
        Some(address) => Address(address),
        None => {
            reentrancy_exit();
            return 98;
        }
    };
    if load_token_address() != Some(token.0) {
        reentrancy_exit();
        return 6;
    }
    if !signer_matches(&caller) {
        reentrancy_exit();
        return 200;
    }
    if !is_admin(&caller) {
        reentrancy_exit();
        return 1;
    }
    if amount == 0 {
        reentrancy_exit();
        return 2;
    }
    let treasury = match stored_address(CM_FEE_TREASURY_KEY) {
        Some(address) if address.iter().any(|byte| *byte != 0) => Address(address),
        _ => {
            reentrancy_exit();
            return 3;
        }
    };
    let key = platform_fee_key(token);
    let accrued = match checked_stored_u64(&key) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 7;
        }
    };
    let remaining = match accrued.checked_sub(amount) {
        Some(balance) => balance,
        None => {
            reentrancy_exit();
            return 4;
        }
    };

    storage_set(&key, &u64_to_bytes(remaining));
    match transfer_token_or_native(token, get_contract_address(), treasury, amount) {
        Ok(true) => {
            lichen_sdk::set_return_data(&u64_to_bytes(amount));
            reentrancy_exit();
            0
        }
        Ok(false) | Err(_) => {
            storage_set(&key, &u64_to_bytes(accrued));
            reentrancy_exit();
            5
        }
    }
}

/// Query realized, custody-backed platform fees for a payment asset.
#[no_mangle]
pub extern "C" fn get_platform_fees(token_ptr: *const u8) -> u32 {
    let token = match read_address32(token_ptr) {
        Some(address) => Address(address),
        None => return 98,
    };
    if load_token_address() != Some(token.0) {
        return 6;
    }
    match checked_stored_u64(&platform_fee_key(token)) {
        Some(amount) => {
            lichen_sdk::set_return_data(&u64_to_bytes(amount));
            0
        }
        None => 7,
    }
}

/// Query snapshotted timing terms for a job.
/// Returns created, claim deadline, claimed, completion deadline, completed,
/// and challenge deadline as six little-endian u64 values.
#[no_mangle]
pub extern "C" fn get_job_timing(job_id: u64) -> u32 {
    let job = match storage_get(&job_key(job_id)) {
        Some(data) if data.len() == JOB_SIZE => data,
        _ => return 1,
    };
    let created = bytes_to_u64(&job[145..153]);
    let completed = bytes_to_u64(&job[153..161]);
    let claim_timeout = match exact_u64_or_default(CLAIM_TIMEOUT_KEY, DEFAULT_CLAIM_TIMEOUT) {
        Some(value) => value,
        None => return 2,
    };
    let complete_timeout =
        match exact_u64_or_default(COMPLETE_TIMEOUT_KEY, DEFAULT_COMPLETE_TIMEOUT) {
            Some(value) => value,
            None => return 2,
        };
    let challenge_period =
        match exact_u64_or_default(CHALLENGE_PERIOD_KEY, DEFAULT_CHALLENGE_PERIOD) {
            Some(value) => value,
            None => return 2,
        };
    let claim_deadline = match exact_u64_or_default(
        &job_claim_deadline_key(job_id),
        created.saturating_add(claim_timeout),
    ) {
        Some(value) => value,
        None => return 2,
    };
    let claimed = match exact_u64_or_default(&job_claimed_slot_key(job_id), 0) {
        Some(value) => value,
        None => return 2,
    };
    let complete_deadline = match exact_u64_or_default(
        &job_complete_deadline_key(job_id),
        created.saturating_add(complete_timeout),
    ) {
        Some(value) => value,
        None => return 2,
    };
    let challenge_deadline = match exact_u64_or_default(
        &job_challenge_deadline_key(job_id),
        completed.saturating_add(challenge_period),
    ) {
        Some(value) => value,
        None => return 2,
    };
    let mut timing = Vec::with_capacity(48);
    for value in [
        created,
        claim_deadline,
        claimed,
        complete_deadline,
        completed,
        challenge_deadline,
    ] {
        timing.extend_from_slice(&u64_to_bytes(value));
    }
    lichen_sdk::set_return_data(&timing);
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
    if !accounting_operational() {
        return 3;
    }
    storage_set(b"cm_paused", &[0u8]);
    log_info("Compute market unpaused");
    0
}

/// Get compute market stats [job_count(8), completed_count(8), payment_volume(8), dispute_count(8)]
#[no_mangle]
pub extern "C" fn get_platform_stats() -> u32 {
    let mut values = [0u64; 4];
    for (index, key) in [
        b"job_count".as_slice(),
        CM_COMPLETED_COUNT_KEY,
        CM_PAYMENT_VOLUME_KEY,
        CM_DISPUTE_COUNT_KEY,
    ]
    .into_iter()
    .enumerate()
    {
        values[index] = match checked_stored_u64(key) {
            Some(value) => value,
            None => return 2,
        };
    }
    let mut buf = Vec::with_capacity(32);
    for value in values {
        buf.extend_from_slice(&u64_to_bytes(value));
    }
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
        storage_set(
            CM_ACCOUNTING_VERSION_KEY,
            &u64_to_bytes(ACCOUNTING_VERSION_V3),
        );
        storage_set(CM_ESCROW_LIABILITY_KEY, &u64_to_bytes(0));
        storage_set(CM_TOTAL_UNPAID_KEY, &u64_to_bytes(0));
        storage_set(CM_MIGRATION_LOCK_KEY, &[0]);
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
        assert_eq!(initialize_as(&admin), 0);
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
        assert_eq!(initialize_as(&admin), 0);
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
        assert_eq!(initialize_as(&admin), 0);
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
        assert_eq!(set_identity_gate(admin.as_ptr(), 100), 3);
        assert_eq!(
            set_lichenid_address(admin.as_ptr(), [0x42u8; 32].as_ptr()),
            0
        );
        assert_eq!(set_identity_gate(admin.as_ptr(), 100), 0);
    }

    #[test]
    fn test_set_lichenid_address_rejects_zero_and_reconfiguration() {
        setup();

        let admin = [1u8; 32];
        let first = [0x42u8; 32];
        let second = [0x24u8; 32];
        assert_eq!(initialize_as(&admin), 0);

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

        assert_eq!(initialize_as(&admin), 0);

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

        assert_eq!(exact_u64_or_default(CLAIM_TIMEOUT_KEY, 0), Some(500));
        assert_eq!(exact_u64_or_default(COMPLETE_TIMEOUT_KEY, 0), Some(2000));
        assert_eq!(exact_u64_or_default(CHALLENGE_PERIOD_KEY, 0), Some(50));
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
        assert_eq!(stored_u64(CM_ESCROW_LIABILITY_KEY), 5000);

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
    fn test_late_claim_receives_full_snapshotted_completion_window() {
        setup();
        test_mock::set_slot(100);

        let provider = [1u8; 32];
        let requester = [2u8; 32];
        register_as(&provider, 1_000, 50);
        assert_eq!(submit_job_as(&requester, 100, 5_000, &[0xAA; 32]), 0);

        // The claim deadline is slot 300. A provider claiming at that boundary
        // gets the full 1,000-slot completion window from the claim itself.
        test_mock::set_slot(300);
        assert_eq!(claim_as(&provider, 0), 0);
        assert_eq!(stored_u64(&job_claimed_slot_key(0)), 300);
        assert_eq!(stored_u64(&job_complete_deadline_key(0)), 1_300);

        test_mock::set_slot(1_299);
        assert_eq!(cancel_as(&requester, 0), 5);
        test_mock::set_slot(1_301);
        assert_eq!(cancel_as(&requester, 0), 0);
    }

    #[test]
    fn test_claim_and_completion_deadlines_fail_closed_after_expiry() {
        setup();
        test_mock::set_slot(100);
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        register_as(&provider, 1_000, 50);
        assert_eq!(submit_job_as(&requester, 100, 5_000, &[0xAA; 32]), 0);

        test_mock::set_slot(301);
        assert_eq!(claim_as(&provider, 0), 7);
        assert_eq!(test_mock::get_storage(&job_key(0)).unwrap()[80], JOB_PENDING);

        test_mock::set_slot(200);
        assert_eq!(claim_as(&provider, 0), 0);
        test_mock::set_slot(1_201);
        assert_eq!(complete_as(&provider, 0, &[0xBB; 32]), 5);
        assert_eq!(test_mock::get_storage(&job_key(0)).unwrap()[80], JOB_CLAIMED);
    }

    #[test]
    fn test_job_deadlines_are_not_changed_retroactively() {
        setup();
        test_mock::set_slot(100);
        let admin = [0xAD; 32];
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        initialize_as(&admin);
        register_as(&provider, 1_000, 50);
        assert_eq!(submit_job_as(&requester, 100, 5_000, &[0xAA; 32]), 0);
        assert_eq!(stored_u64(&job_claim_deadline_key(0)), 300);

        test_mock::set_caller(admin);
        assert_eq!(set_claim_timeout(admin.as_ptr(), 1), 0);
        assert_eq!(set_complete_timeout(admin.as_ptr(), 2), 0);
        assert_eq!(set_challenge_period(admin.as_ptr(), 3), 0);

        test_mock::set_slot(250);
        assert_eq!(claim_as(&provider, 0), 0);
        assert_eq!(stored_u64(&job_complete_deadline_key(0)), 1_250);
        test_mock::set_slot(1_250);
        assert_eq!(complete_as(&provider, 0, &[0xBB; 32]), 0);
        assert_eq!(stored_u64(&job_challenge_deadline_key(0)), 1_350);

        test_mock::set_caller(admin);
        assert_eq!(set_challenge_period(admin.as_ptr(), 10_000), 0);
        test_mock::set_slot(1_351);
        assert_eq!(release_payment(0), 0);
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
        assert_eq!(
            exact_u64_or_default(CLAIM_TIMEOUT_KEY, DEFAULT_CLAIM_TIMEOUT),
            Some(DEFAULT_CLAIM_TIMEOUT)
        );
        assert_eq!(
            exact_u64_or_default(COMPLETE_TIMEOUT_KEY, DEFAULT_COMPLETE_TIMEOUT),
            Some(DEFAULT_COMPLETE_TIMEOUT)
        );
        assert_eq!(
            exact_u64_or_default(CHALLENGE_PERIOD_KEY, DEFAULT_CHALLENGE_PERIOD),
            Some(DEFAULT_CHALLENGE_PERIOD)
        );
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
    fn test_release_pays_agreed_quote_and_refunds_unused_budget() {
        setup();
        test_mock::set_slot(100);
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        register_as(&provider, 1_000, 50);
        submit_job_as(&requester, 100, 10_000, &[0xAA; 32]);
        claim_as(&provider, 0);
        assert_eq!(stored_u64(&job_payment_due_key(0)), 5_000);
        test_mock::set_slot(200);
        complete_as(&provider, 0, &[0xBB; 32]);
        test_mock::set_slot(301);
        assert_eq!(release_payment(0), 0);

        // The final transfer is the unused 5,000 max-price budget back to the
        // requester; volume records the agreed provider payment, not the cap.
        let (_, _, args, _) = test_mock::get_last_cross_call().unwrap();
        assert_eq!(bytes_to_u64(&args[args.len() - 8..]), 5_000);
        assert_eq!(stored_u64(CM_PAYMENT_VOLUME_KEY), 5_000);
    }

    #[test]
    fn test_failed_refund_after_provider_payment_becomes_exact_unpaid_claim() {
        setup();
        test_mock::set_slot(100);
        test_mock::set_cross_call_responses(vec![
            0u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
        ]);
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        register_as(&provider, 1_000, 50);
        submit_job_as(&requester, 100, 10_000, &[0xAA; 32]);
        claim_as(&provider, 0);
        test_mock::set_slot(200);
        complete_as(&provider, 0, &[0xBB; 32]);
        test_mock::set_slot(301);
        assert_eq!(release_payment(0), 0);

        assert_eq!(test_mock::get_storage(&job_key(0)).unwrap()[80], JOB_RELEASED);
        assert_eq!(stored_u64(&escrow_key(0)), 0);
        assert_eq!(
            stored_u64(&unpaid_payout_key(
                &TEST_TOKEN_ADDR,
                &requester
            )),
            5_000
        );
        assert_eq!(stored_u64(CM_TOTAL_UNPAID_KEY), 5_000);
        assert_eq!(stored_u64(CM_ESCROW_LIABILITY_KEY), 0);
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
    fn test_claim_enforces_provider_capacity_and_quote() {
        setup();
        test_mock::set_slot(100);
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        register_as(&provider, 100, 50);

        submit_job_as(&requester, 101, 10_000, &[0xAA; 32]);
        assert_eq!(claim_as(&provider, 0), 8);
        assert_eq!(stored_u64(&provider_reserved_key(&provider)), 0);

        submit_job_as(&requester, 100, 4_999, &[0xAB; 32]);
        assert_eq!(claim_as(&provider, 1), 10);
        assert_eq!(stored_u64(&provider_reserved_key(&provider)), 0);
    }

    #[test]
    fn test_provider_capacity_is_reserved_and_released_exactly() {
        setup();
        test_mock::set_slot(100);
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        register_as(&provider, 100, 10);
        submit_job_as(&requester, 60, 600, &[0xAA; 32]);
        submit_job_as(&requester, 60, 600, &[0xAB; 32]);

        assert_eq!(claim_as(&provider, 0), 0);
        assert_eq!(stored_u64(&provider_reserved_key(&provider)), 60);
        assert_eq!(claim_as(&provider, 1), 8);
        test_mock::set_caller(provider);
        assert_eq!(update_provider(provider.as_ptr(), 59, 10), 4);

        assert_eq!(complete_as(&provider, 0, &[0xCC; 32]), 0);
        assert_eq!(stored_u64(&provider_reserved_key(&provider)), 0);
        assert_eq!(claim_as(&provider, 1), 0);
        assert_eq!(get_provider_capacity(provider.as_ptr()), 0);
        let capacity = test_mock::get_return_data();
        let values: Vec<u64> = capacity
            .as_chunks::<8>()
            .0
            .iter()
            .map(|value| bytes_to_u64(value))
            .collect();
        assert_eq!(values, vec![100, 60, 40]);
    }

    #[test]
    fn test_failed_claim_timeout_refund_restores_capacity_reservation() {
        setup();
        test_mock::set_slot(100);
        test_mock::set_cross_call_responses(vec![
            0u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
        ]);
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        register_as(&provider, 100, 10);
        submit_job_as(&requester, 60, 600, &[0xAA; 32]);
        claim_as(&provider, 0);
        assert_eq!(stored_u64(&provider_reserved_key(&provider)), 60);

        test_mock::set_slot(1_101);
        assert_eq!(cancel_as(&requester, 0), 7);
        assert_eq!(test_mock::get_storage(&job_key(0)).unwrap()[80], JOB_CLAIMED);
        assert_eq!(stored_u64(&provider_reserved_key(&provider)), 60);
        assert_eq!(
            exact_bool_or_default(&job_capacity_released_key(0), false),
            Some(false)
        );
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
        assert_eq!(bytes_to_u64(&unpaid), 2000);
        assert_eq!(stored_u64(CM_TOTAL_UNPAID_KEY), 2000);
        assert_eq!(stored_u64(CM_ESCROW_LIABILITY_KEY), 0);
    }

    #[test]
    fn test_claim_unpaid_payout_retries_after_failed_transfer() {
        setup();
        let provider_addr = [1u8; 32];
        let key = unpaid_payout_key(&TEST_TOKEN_ADDR, &provider_addr);
        storage_set(&key, &u64_to_bytes(4000));
        storage_set(CM_TOTAL_UNPAID_KEY, &u64_to_bytes(4000));

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
        assert_eq!(stored_u64(CM_TOTAL_UNPAID_KEY), 4000);

        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        assert_eq!(
            claim_unpaid_payout(provider_addr.as_ptr(), TEST_TOKEN_ADDR.as_ptr()),
            0
        );
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 4000);
        let unpaid = test_mock::get_storage(&key).unwrap();
        assert_eq!(bytes_to_u64(&unpaid), 0);
        assert_eq!(stored_u64(CM_TOTAL_UNPAID_KEY), 0);
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
        // Rejected transactions roll back contract storage, so failures are
        // counted by the external transaction index rather than on-chain.
        assert_eq!(stored_u64(CM_AGENT_BLOCKED_PAYMENT_COUNT_KEY), 0);
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
        assert_eq!(stored_u64(CM_AGENT_BLOCKED_PAYMENT_COUNT_KEY), 0);

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
    fn test_release_realizes_snapshotted_fee_and_pays_provider_net() {
        setup();
        test_mock::set_slot(100);
        let admin = [0xAD; 32];
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        initialize_as(&admin);
        test_mock::set_caller(admin);
        assert_eq!(set_platform_fee(admin.as_ptr(), 500), 0);
        register_as(&provider, 1_000, 100);
        assert_eq!(submit_job_as(&requester, 100, 10_000, &[0xAA; 32]), 0);
        assert_eq!(stored_u64(&job_fee_bps_key(0)), 500);
        claim_as(&provider, 0);
        test_mock::set_slot(200);
        complete_as(&provider, 0, &[0xBB; 32]);
        test_mock::set_slot(301);
        assert_eq!(release_payment(0), 0);

        let fee_key = platform_fee_key(Address(TEST_TOKEN_ADDR));
        assert_eq!(stored_u64(&fee_key), 500);
        assert_eq!(get_platform_fees(TEST_TOKEN_ADDR.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 500);
        let (target, function, args, _) = test_mock::get_last_cross_call().unwrap();
        assert_eq!(target, TEST_TOKEN_ADDR);
        assert_eq!(function, "transfer");
        assert_eq!(bytes_to_u64(&args[args.len() - 8..]), 9_500);
    }

    #[test]
    fn test_platform_fee_changes_are_prospective_only() {
        setup();
        test_mock::set_slot(100);
        let admin = [0xAD; 32];
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        initialize_as(&admin);
        register_as(&provider, 1_000, 100);
        assert_eq!(submit_job_as(&requester, 100, 10_000, &[0xAA; 32]), 0);

        test_mock::set_caller(admin);
        assert_eq!(set_platform_fee(admin.as_ptr(), 1_000), 0);
        claim_as(&provider, 0);
        test_mock::set_slot(200);
        complete_as(&provider, 0, &[0xBB; 32]);
        test_mock::set_slot(301);
        assert_eq!(release_payment(0), 0);
        assert_eq!(stored_u64(&platform_fee_key(Address(TEST_TOKEN_ADDR))), 0);
        let (_, _, args, _) = test_mock::get_last_cross_call().unwrap();
        assert_eq!(bytes_to_u64(&args[args.len() - 8..]), 10_000);
    }

    #[test]
    fn test_failed_provider_payment_does_not_realize_platform_fee() {
        setup();
        test_mock::set_slot(100);
        test_mock::set_cross_call_responses(vec![
            0u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
        ]);
        let admin = [0xAD; 32];
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        initialize_as(&admin);
        test_mock::set_caller(admin);
        assert_eq!(set_platform_fee(admin.as_ptr(), 500), 0);
        register_as(&provider, 1_000, 50);
        assert_eq!(submit_job_as(&requester, 100, 10_000, &[0xAA; 32]), 0);
        claim_as(&provider, 0);
        test_mock::set_slot(200);
        complete_as(&provider, 0, &[0xBB; 32]);
        test_mock::set_slot(301);
        assert_eq!(release_payment(0), 6);

        assert_eq!(test_mock::get_storage(&job_key(0)).unwrap()[80], JOB_COMPLETED);
        assert_eq!(stored_u64(&escrow_key(0)), 10_000);
        assert_eq!(stored_u64(&platform_fee_key(Address(TEST_TOKEN_ADDR))), 0);
    }

    #[test]
    fn test_dispute_fee_applies_only_to_provider_compensation() {
        setup();
        test_mock::set_slot(100);
        let admin = [0xAD; 32];
        let arbitrator = [0xAA; 32];
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        initialize_as(&admin);
        test_mock::set_caller(admin);
        assert_eq!(set_platform_fee(admin.as_ptr(), 500), 0);
        assert_eq!(add_arbitrator(admin.as_ptr(), arbitrator.as_ptr()), 0);
        register_as(&provider, 1_000, 100);
        submit_job_as(&requester, 100, 10_000, &[0xAA; 32]);
        claim_as(&provider, 0);
        complete_as(&provider, 0, &[0xBB; 32]);
        dispute_as(&requester, 0);
        assert_eq!(resolve_as(&arbitrator, 0, 60), 0);

        // Requester refund is the full 6,000. Fee is 5% of the provider's
        // 4,000 compensation, leaving a 3,800 provider transfer.
        assert_eq!(stored_u64(&platform_fee_key(Address(TEST_TOKEN_ADDR))), 200);
        let (_, function, args, _) = test_mock::get_last_cross_call().unwrap();
        assert_eq!(function, "transfer");
        assert_eq!(bytes_to_u64(&args[args.len() - 8..]), 3_800);
    }

    #[test]
    fn test_fee_withdrawal_is_treasury_bound_and_retry_safe() {
        setup();
        let admin = [0xAD; 32];
        let treasury = [0x77; 32];
        initialize_as(&admin);
        let key = platform_fee_key(Address(TEST_TOKEN_ADDR));
        storage_set(&key, &u64_to_bytes(500));
        test_mock::set_caller(admin);
        assert_eq!(set_fee_treasury(admin.as_ptr(), [0u8; 32].as_ptr()), 2);
        assert_eq!(set_fee_treasury(admin.as_ptr(), treasury.as_ptr()), 0);

        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(withdraw_platform_fees(admin.as_ptr(), TEST_TOKEN_ADDR.as_ptr(), 300), 5);
        assert_eq!(stored_u64(&key), 500);

        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        assert_eq!(withdraw_platform_fees(admin.as_ptr(), TEST_TOKEN_ADDR.as_ptr(), 300), 0);
        assert_eq!(stored_u64(&key), 200);
        let (_, _, args, _) = test_mock::get_last_cross_call().unwrap();
        assert_eq!(&args[36..68], &treasury);
        assert_eq!(bytes_to_u64(&args[args.len() - 8..]), 300);
    }

    #[test]
    fn test_job_payment_asset_is_snapshotted() {
        setup();
        test_mock::set_slot(100);
        let replacement_token = [0xEE; 32];
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        register_as(&provider, 1_000, 50);
        submit_job_as(&requester, 100, 5_000, &[0xAA; 32]);
        storage_set(CM_TOKEN_ADDRESS_KEY, &replacement_token);
        claim_as(&provider, 0);
        test_mock::set_slot(200);
        complete_as(&provider, 0, &[0xBB; 32]);
        test_mock::set_slot(301);
        assert_eq!(release_payment(0), 0);
        let (target, _, _, _) = test_mock::get_last_cross_call().unwrap();
        assert_eq!(target, TEST_TOKEN_ADDR);
    }

    #[test]
    fn test_job_timing_query_returns_snapshotted_lifecycle() {
        setup();
        test_mock::set_slot(100);
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        register_as(&provider, 1_000, 50);
        submit_job_as(&requester, 100, 5_000, &[0xAA; 32]);
        test_mock::set_slot(250);
        claim_as(&provider, 0);
        test_mock::set_slot(300);
        complete_as(&provider, 0, &[0xBB; 32]);

        assert_eq!(get_job_timing(0), 0);
        let timing = test_mock::get_return_data();
        let values: Vec<u64> = timing
            .as_chunks::<8>()
            .0
            .iter()
            .map(|value| bytes_to_u64(value))
            .collect();
        assert_eq!(values, vec![100, 300, 250, 1_250, 300, 400]);
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

    #[test]
    fn test_identity_admin_is_protocol_bound_for_fresh_and_legacy_state() {
        setup();
        let admin = [0xAD; 32];
        let attacker = [0xEE; 32];

        assert_eq!(initialize_as(&admin), 0);
        assert_eq!(stored_address(IDENTITY_ADMIN_KEY), Some(admin));
        test_mock::set_caller(attacker);
        assert_eq!(set_identity_admin(attacker.as_ptr()), 1);

        test_mock::reset();
        storage_set(ADMIN_KEY, &admin);
        test_mock::set_caller(attacker);
        assert_eq!(set_identity_admin(attacker.as_ptr()), 4);
        assert!(storage_get(IDENTITY_ADMIN_KEY).is_none());
        test_mock::set_caller(admin);
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        assert_eq!(stored_address(IDENTITY_ADMIN_KEY), Some(admin));
    }

    #[test]
    fn test_identity_gate_fails_closed_without_exact_dependency_and_response() {
        setup();
        let admin = [0xAD; 32];
        let provider = [1u8; 32];
        assert_eq!(initialize_as(&admin), 0);
        test_mock::set_caller(admin);
        assert_eq!(set_identity_gate(admin.as_ptr(), 100), 3);

        storage_set(LICHENID_ADDR_KEY, &[0x42]);
        storage_set(LICHENID_MIN_REP_KEY, &u64_to_bytes(100));
        assert_eq!(register_as(&provider, 1_000, 10), 10);

        storage_set(LICHENID_ADDR_KEY, &[0x42; 32]);
        test_mock::set_cross_call_response(Some(vec![100; 9]));
        assert_eq!(register_as(&provider, 1_000, 10), 10);

        test_mock::set_cross_call_response(Some(u64_to_bytes(100).to_vec()));
        assert_eq!(register_as(&provider, 1_000, 10), 0);
    }

    #[test]
    fn test_hashes_and_arbitrators_must_be_nonzero() {
        setup();
        test_mock::set_slot(100);
        let admin = [0xAD; 32];
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        assert_eq!(initialize_as(&admin), 0);
        test_mock::set_caller(admin);
        assert_eq!(add_arbitrator(admin.as_ptr(), [0u8; 32].as_ptr()), 2);
        assert_eq!(submit_job_as(&requester, 10, 100, &[0u8; 32]), 15);
        assert!(test_mock::get_last_cross_call().is_none());

        assert_eq!(register_as(&provider, 100, 10), 0);
        assert_eq!(submit_job_as(&requester, 10, 100, &[0xAA; 32]), 0);
        assert_eq!(claim_as(&provider, 0), 0);
        assert_eq!(complete_as(&provider, 0, &[0u8; 32]), 6);
        assert_eq!(test_mock::get_storage(&job_key(0)).unwrap()[80], JOB_CLAIMED);
        assert_eq!(stored_u64(&provider_reserved_key(&provider)), 10);
    }

    #[test]
    fn test_deadline_boundaries_have_no_overlapping_transitions() {
        setup();
        test_mock::set_slot(100);
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        register_as(&provider, 100, 10);
        submit_job_as(&requester, 10, 100, &[0xAA; 32]);

        test_mock::set_slot(300);
        assert_eq!(cancel_as(&requester, 0), 4);
        assert_eq!(claim_as(&provider, 0), 0);
        test_mock::set_slot(1_300);
        assert_eq!(cancel_as(&requester, 0), 5);
        assert_eq!(complete_as(&provider, 0, &[0xBB; 32]), 0);
        test_mock::set_slot(1_400);
        assert_eq!(release_payment(0), 5);
        assert_eq!(dispute_as(&requester, 0), 0);
    }

    #[test]
    fn test_disputed_escrow_can_be_resolved_while_paused_and_updates_stats() {
        setup();
        test_mock::set_slot(100);
        let admin = [0xAD; 32];
        let arbitrator = [0xA1; 32];
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        initialize_as(&admin);
        test_mock::set_caller(admin);
        assert_eq!(add_arbitrator(admin.as_ptr(), arbitrator.as_ptr()), 0);
        register_as(&provider, 100, 10);
        submit_job_as(&requester, 10, 100, &[0xAA; 32]);
        claim_as(&provider, 0);
        complete_as(&provider, 0, &[0xBB; 32]);
        dispute_as(&requester, 0);
        test_mock::set_caller(admin);
        assert_eq!(pause(admin.as_ptr()), 0);

        assert_eq!(resolve_as(&arbitrator, 0, 50), 0);
        assert_eq!(stored_u64(CM_COMPLETED_COUNT_KEY), 1);
        assert_eq!(stored_u64(CM_PAYMENT_VOLUME_KEY), 100);
    }

    #[test]
    fn test_agent_policy_versions_and_action_hashes_are_replay_safe() {
        setup();
        test_mock::set_slot(100);
        let admin = [0xAD; 32];
        let agent = [0xA9; 32];
        let policy_hash = [0x44; 32];
        let code_hash = [0x55; 32];
        let action_hash = [0x66; 32];
        initialize_as(&admin);
        assert_eq!(set_agent_controls_as(&admin, 1, 0, 10_000, 5_000), 0);
        assert_eq!(set_agent_policy_as(&agent, 6_000, 4_000, &policy_hash, 2), 0);
        assert_eq!(set_agent_policy_as(&agent, 6_000, 4_000, &policy_hash, 2), 4);
        assert_eq!(set_agent_policy_as(&agent, 6_000, 4_000, &policy_hash, 1), 4);

        assert_eq!(submit_agent_job_as(&agent, 10, 1_000, &code_hash, &action_hash), 0);
        assert_eq!(submit_agent_job_as(&agent, 10, 1_000, &code_hash, &action_hash), 49);
        assert_eq!(stored_u64(b"job_count"), 1);

        let unused_action = [0x67; 32];
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(submit_agent_job_as(&agent, 10, 1_000, &code_hash, &unused_action), 13);
        assert!(storage_get(&agent_action_used_key(&unused_action)).is_none());
    }

    #[test]
    fn test_malformed_immutable_token_cannot_be_overwritten() {
        setup();
        let admin = [0xAD; 32];
        initialize_as(&admin);
        storage_set(CM_TOKEN_ADDRESS_KEY, &[0x01]);
        test_mock::set_caller(admin);
        assert_eq!(set_token_address(admin.as_ptr(), TEST_TOKEN_ADDR.as_ptr()), 3);
        assert_eq!(storage_get(CM_TOKEN_ADDRESS_KEY), Some(vec![0x01]));
    }

    #[test]
    fn test_settlement_and_cancel_fail_closed_on_missing_escrow() {
        setup();
        test_mock::set_slot(100);
        let admin = [0xAD; 32];
        let arbitrator = [0xA1; 32];
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        initialize_as(&admin);
        test_mock::set_caller(admin);
        add_arbitrator(admin.as_ptr(), arbitrator.as_ptr());
        register_as(&provider, 100, 10);

        submit_job_as(&requester, 10, 100, &[0xAA; 32]);
        storage_set(&escrow_key(0), &u64_to_bytes(0));
        test_mock::set_slot(301);
        assert_eq!(cancel_as(&requester, 0), 9);

        test_mock::set_slot(100);
        submit_job_as(&requester, 10, 100, &[0xAB; 32]);
        claim_as(&provider, 1);
        complete_as(&provider, 1, &[0xBB; 32]);
        storage_set(&escrow_key(1), &u64_to_bytes(0));
        test_mock::set_slot(201);
        assert_eq!(release_payment(1), 9);
        test_mock::set_slot(200);
        assert_eq!(dispute_as(&requester, 1), 0);
        assert_eq!(resolve_as(&arbitrator, 1, 50), 10);
    }

    #[test]
    fn test_malformed_control_and_snapshot_state_fails_closed_before_value_mutation() {
        setup();
        test_mock::set_slot(100);
        let provider = [1u8; 32];
        let requester = [2u8; 32];
        register_as(&provider, 100, 10);

        storage_set(CLAIM_TIMEOUT_KEY, &[1]);
        assert_eq!(submit_job_as(&requester, 10, 100, &[0xAA; 32]), 18);
        assert_eq!(checked_stored_u64(b"job_count"), Some(0));
        assert_eq!(checked_stored_u64(CM_ESCROW_LIABILITY_KEY), Some(0));
        lichen_sdk::storage::remove(CLAIM_TIMEOUT_KEY);

        assert_eq!(submit_job_as(&requester, 10, 100, &[0xAA; 32]), 0);
        storage_set(&job_claim_deadline_key(0), &[1]);
        assert_eq!(claim_as(&provider, 0), 9);
        assert_eq!(storage_get(&job_key(0)).unwrap()[80], JOB_PENDING);
        assert_eq!(checked_stored_u64(&provider_reserved_key(&provider)), Some(0));

        storage_set(b"cm_paused", &[2]);
        assert_eq!(submit_job_as(&requester, 10, 100, &[0xAB; 32]), 99);
        storage_set(b"cm_paused", &[0]);
        storage_set(CM_AGENT_PAYMENTS_ENABLED_KEY, &[2]);
        assert_eq!(
            submit_agent_job_as(&requester, 10, 100, &[0xAC; 32], &[0xAD; 32]),
            40
        );

        storage_set(CM_PAYMENT_VOLUME_KEY, &[1]);
        assert_eq!(get_platform_stats(), 2);
        storage_set(CM_AGENT_PAYMENT_VOLUME_KEY, &[1]);
        assert_eq!(get_agent_compute_controls(), 2);
    }

    #[test]
    fn test_accounting_v3_migration_is_source_bound_resumable_and_solvent() {
        setup();
        test_mock::set_slot(100);
        let admin = [0xAD; 32];
        let requester = [2u8; 32];
        initialize_as(&admin);
        submit_job_as(&requester, 10, 100, &[0xAA; 32]);
        submit_job_as(&requester, 10, 200, &[0xAB; 32]);

        let mut terminal = storage_get(&job_key(1)).unwrap();
        terminal[80] = JOB_RELEASED;
        storage_set(&job_key(1), &terminal);
        storage_set(&escrow_key(1), &u64_to_bytes(0));
        storage_set(
            &unpaid_payout_key(&TEST_TOKEN_ADDR, &requester),
            &u64_to_bytes(20),
        );
        storage_set(
            &platform_fee_key(Address(TEST_TOKEN_ADDR)),
            &u64_to_bytes(10),
        );
        lichen_sdk::storage::remove(CM_ACCOUNTING_VERSION_KEY);
        lichen_sdk::storage::remove(CM_ESCROW_LIABILITY_KEY);
        lichen_sdk::storage::remove(CM_TOTAL_UNPAID_KEY);

        test_mock::set_caller(admin);
        assert_eq!(begin_accounting_v3_migration(admin.as_ptr(), 1), 3);
        assert_eq!(begin_accounting_v3_migration(admin.as_ptr(), 2), 0);
        assert!(cm_paused());
        assert_eq!(begin_accounting_v3_migration(admin.as_ptr(), 2), 0);
        assert_eq!(submit_job_as(&requester, 1, 1, &[0xAC; 32]), 99);
        assert_eq!(set_platform_fee(admin.as_ptr(), 500), 90);
        assert_eq!(set_fee_treasury(admin.as_ptr(), [0x55; 32].as_ptr()), 90);

        assert_eq!(migrate_accounting_v3_job(1), 2);
        assert_eq!(migrate_accounting_v3_job(0), 0);
        assert_eq!(migrate_accounting_v3_job(0), 2);
        assert_eq!(migrate_accounting_v3_job(1), 0);
        assert_eq!(get_accounting_migration_status(), 0);
        let status: Vec<u64> = test_mock::get_return_data()
            .as_chunks::<8>()
            .0
            .iter()
            .map(|value| bytes_to_u64(value))
            .collect();
        assert_eq!(status, vec![2, 2, 100, 20, 0, 1]);
        storage_set(CM_MIGRATION_CURSOR_KEY, &[1]);
        assert_eq!(get_accounting_migration_status(), 2);
        storage_set(CM_MIGRATION_CURSOR_KEY, &u64_to_bytes(2));

        test_mock::set_caller(admin);
        assert_eq!(
            complete_accounting_v3_migration(admin.as_ptr(), 100, 20, 10, 131),
            5
        );
        test_mock::set_cross_call_response(Some(u64_to_bytes(129).to_vec()));
        assert_eq!(
            complete_accounting_v3_migration(admin.as_ptr(), 100, 20, 10, 130),
            9
        );
        assert!(migration_locked());
        test_mock::set_cross_call_response(Some(u64_to_bytes(130).to_vec()));
        assert_eq!(
            complete_accounting_v3_migration(admin.as_ptr(), 100, 20, 10, 130),
            0
        );
        assert!(accounting_operational());
        assert!(cm_paused());

        test_mock::set_cross_call_response(Some(u64_to_bytes(130).to_vec()));
        assert_eq!(get_accounting_health(), 0);
        let health: Vec<u64> = test_mock::get_return_data()
            .as_chunks::<8>()
            .0
            .iter()
            .map(|value| bytes_to_u64(value))
            .collect();
        assert_eq!(health, vec![3, 0, 100, 20, 10, 130, 130, 1]);
        test_mock::set_caller(admin);
        assert_eq!(unpause(admin.as_ptr()), 0);
    }
}
