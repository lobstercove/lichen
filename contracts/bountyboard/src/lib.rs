// BountyBoard — Bounty/Task Management Contract for Lichen
//
// On-chain bounty system for task management:
//   - Creators post bounties with rewards and deadlines
//   - Workers submit proof of work
//   - Creators approve submissions and pay rewards
//   - Creators can cancel and get refunds
//
// Storage keys:
//   bounty_{id}       → BountyInfo
//   bounty_count      → u64
//   submission_{id}_{idx} → SubmissionInfo

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;
use alloc::vec::Vec;

use lichen_sdk::{
    balance_of_token_or_native, bytes_to_u64, call_contract, get_caller, get_contract_address,
    get_slot, get_value, log_info, receive_token_or_native, storage_get, storage_set,
    transfer_token_or_native, u64_to_bytes, Address, CrossCall,
};

// ============================================================================
// BOUNTY STATUS
// ============================================================================

const BOUNTY_OPEN: u8 = 0;
const BOUNTY_COMPLETED: u8 = 1;
const BOUNTY_CANCELLED: u8 = 2;

const ERR_PAUSED: u32 = 13;

const BB_COMPLETED_COUNT_KEY: &[u8] = b"bb_completed_count";
const BB_REWARD_VOLUME_KEY: &[u8] = b"bb_reward_volume";
const BB_CANCEL_COUNT_KEY: &[u8] = b"bb_cancel_count";
const BB_PLATFORM_FEE_BPS_KEY: &[u8] = b"platform_fee_bps";
const BB_FEE_TREASURY_KEY: &[u8] = b"bb_fee_treasury";
const BB_PENDING_ADMIN_KEY: &[u8] = b"bb_pending_admin";
const BB_ACCOUNTING_VERSION_KEY: &[u8] = b"bb_account_version";
const BB_ESCROW_LIABILITY_KEY: &[u8] = b"bb_escrow_liability";
const BB_MIGRATION_LOCK_KEY: &[u8] = b"bb_account_mig_lock";
const BB_MIGRATION_EXPECTED_COUNT_KEY: &[u8] = b"bb_account_mig_expected";
const BB_MIGRATION_CURSOR_KEY: &[u8] = b"bb_account_mig_cursor";
const BB_MIGRATION_ESCROW_KEY: &[u8] = b"bb_account_mig_escrow";
const ACCOUNTING_VERSION_V2: u64 = 2;

// ============================================================================
// STORAGE KEY HELPERS
// ============================================================================

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

fn bounty_key(bounty_id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(7 + 20);
    key.extend_from_slice(b"bounty_");
    key.extend_from_slice(&u64_to_decimal(bounty_id));
    key
}

fn submission_key(bounty_id: u64, idx: u8) -> Vec<u8> {
    let mut key = Vec::with_capacity(12 + 20 + 4);
    key.extend_from_slice(b"submission_");
    key.extend_from_slice(&u64_to_decimal(bounty_id));
    key.push(b'_');
    key.extend_from_slice(&u64_to_decimal(idx as u64));
    key
}

fn bounty_metadata_key(prefix: &[u8], bounty_id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(prefix.len() + 20);
    key.extend_from_slice(prefix);
    key.extend_from_slice(&u64_to_decimal(bounty_id));
    key
}

fn bounty_token_key(bounty_id: u64) -> Vec<u8> {
    bounty_metadata_key(b"bounty_token_", bounty_id)
}

fn bounty_fee_bps_key(bounty_id: u64) -> Vec<u8> {
    bounty_metadata_key(b"bounty_fee_bps_", bounty_id)
}

fn worker_submission_key(bounty_id: u64, worker: &[u8; 32]) -> Vec<u8> {
    let mut key = bounty_metadata_key(b"bounty_worker_", bounty_id);
    key.push(b'_');
    key.extend_from_slice(worker);
    key
}

fn platform_fee_key(token: Address) -> Vec<u8> {
    let mut key = b"bb_platform_fee:".to_vec();
    key.extend_from_slice(&token.0);
    key
}

// ============================================================================
// REENTRANCY GUARD
// ============================================================================

const BB_REENTRANCY_KEY: &[u8] = b"bb_reentrancy";

fn reentrancy_enter() -> bool {
    match storage_get(BB_REENTRANCY_KEY).as_deref() {
        None | Some([0]) => {}
        Some([1]) | Some(_) => return false,
    }
    storage_set(BB_REENTRANCY_KEY, &[1u8]);
    true
}

fn reentrancy_exit() {
    storage_set(BB_REENTRANCY_KEY, &[0u8]);
}

fn is_bb_paused() -> bool {
    match storage_get(b"bb_paused").as_deref() {
        None | Some([0]) => false,
        Some([1]) | Some(_) => true,
    }
}

fn load_configured_address(key: &[u8]) -> Option<[u8; 32]> {
    storage_get(key).and_then(|bytes| {
        if bytes.len() != 32 {
            return None;
        }

        let mut addr = [0u8; 32];
        addr.copy_from_slice(&bytes[..32]);
        Some(addr)
    })
}

fn read_address(ptr: *const u8) -> Option<[u8; 32]> {
    if ptr.is_null() {
        return None;
    }
    let mut addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(ptr, addr.as_mut_ptr(), 32);
    }
    Some(addr)
}

fn checked_stored_u64(key: &[u8]) -> Option<u64> {
    match storage_get(key) {
        None => Some(0),
        Some(data) if data.len() == 8 => Some(bytes_to_u64(&data)),
        Some(_) => None,
    }
}

#[cfg(test)]
fn stored_u64(key: &[u8]) -> u64 {
    checked_stored_u64(key).unwrap_or(0)
}

fn checked_increment(key: &[u8]) -> Option<u64> {
    checked_stored_u64(key)?.checked_add(1)
}

fn accounting_version() -> Option<u64> {
    checked_stored_u64(BB_ACCOUNTING_VERSION_KEY)
}

fn migration_lock_valid() -> bool {
    matches!(
        storage_get(BB_MIGRATION_LOCK_KEY).as_deref(),
        None | Some([0]) | Some([1])
    )
}

fn migration_locked() -> bool {
    storage_get(BB_MIGRATION_LOCK_KEY).as_deref() == Some(&[1])
}

fn accounting_operational() -> bool {
    accounting_version() == Some(ACCOUNTING_VERSION_V2)
        && migration_lock_valid()
        && !migration_locked()
        && checked_stored_u64(BB_ESCROW_LIABILITY_KEY).is_some()
}

fn runtime_configuration_valid() -> bool {
    if reward_token_or_native().is_none()
        || checked_stored_u64(BB_PLATFORM_FEE_BPS_KEY).is_none_or(|fee| fee > 1_000)
        || !load_configured_address(BB_FEE_TREASURY_KEY)
            .is_some_and(|treasury| treasury.iter().any(|byte| *byte != 0))
    {
        return false;
    }

    match checked_stored_u64(LICHENID_MIN_REP_KEY) {
        Some(0) => true,
        Some(_) => load_configured_address(LICHENID_ADDR_KEY)
            .is_some_and(|address| address.iter().any(|byte| *byte != 0)),
        None => false,
    }
}

fn effectively_paused() -> bool {
    is_bb_paused() || !accounting_operational() || !runtime_configuration_valid()
}

fn reward_token_or_native() -> Option<Address> {
    match storage_get(TOKEN_ADDRESS_KEY) {
        Some(bytes) if bytes.len() == 32 => {
            let mut addr = [0u8; 32];
            addr.copy_from_slice(&bytes);
            Some(Address(addr))
        }
        None | Some(_) => None,
    }
}

fn bounty_reward_token(bounty_id: u64) -> Option<Address> {
    match storage_get(&bounty_token_key(bounty_id)) {
        Some(bytes) if bytes.len() == 32 => {
            let mut address = [0u8; 32];
            address.copy_from_slice(&bytes);
            Some(Address(address))
        }
        None | Some(_) => None,
    }
}

fn bounty_platform_fee_bps(bounty_id: u64) -> Option<u64> {
    match storage_get(&bounty_fee_bps_key(bounty_id)) {
        Some(bytes) if bytes.len() == 8 => Some(bytes_to_u64(&bytes)),
        None | Some(_) => None,
    }
}

fn split_reward(reward: u64, fee_bps: u64) -> Option<(u64, u64)> {
    if fee_bps > 10_000 {
        return None;
    }
    let fee = ((reward as u128).checked_mul(fee_bps as u128)? / 10_000) as u64;
    Some((reward.checked_sub(fee)?, fee))
}

fn require_admin(caller: &[u8; 32]) -> bool {
    storage_get(IDENTITY_ADMIN_KEY)
        .map(|admin| admin.len() == 32 && admin.as_slice() == caller)
        .unwrap_or(false)
}

// ============================================================================
// BOUNTY LAYOUT
// ============================================================================
//
// Bytes 0..32   : creator (address)
// Bytes 32..64  : title_hash (32 bytes)
// Bytes 64..72  : reward_amount (u64 LE)
// Bytes 72..80  : deadline_slot (u64 LE)
// Byte  80      : status (u8)
// Byte  81      : submission_count (u8)
// Bytes 82..90  : created_slot (u64 LE)
// Byte  90      : approved_idx (u8, 0xFF if none)

const BOUNTY_SIZE: usize = 91;

struct BountyEncoding<'a> {
    creator: &'a [u8; 32],
    title_hash: &'a [u8; 32],
    reward_amount: u64,
    deadline_slot: u64,
    status: u8,
    submission_count: u8,
    created_slot: u64,
    approved_idx: u8,
}

fn encode_bounty(bounty: BountyEncoding<'_>) -> Vec<u8> {
    let mut data = Vec::with_capacity(BOUNTY_SIZE);
    data.extend_from_slice(bounty.creator);
    data.extend_from_slice(bounty.title_hash);
    data.extend_from_slice(&u64_to_bytes(bounty.reward_amount));
    data.extend_from_slice(&u64_to_bytes(bounty.deadline_slot));
    data.push(bounty.status);
    data.push(bounty.submission_count);
    data.extend_from_slice(&u64_to_bytes(bounty.created_slot));
    data.push(bounty.approved_idx);
    data
}

fn bounty_row_valid(data: &[u8]) -> bool {
    if data.len() != BOUNTY_SIZE
        || data[..32].iter().all(|byte| *byte == 0)
        || data[32..64].iter().all(|byte| *byte == 0)
        || bytes_to_u64(&data[64..72]) == 0
        || bytes_to_u64(&data[72..80]) <= bytes_to_u64(&data[82..90])
    {
        return false;
    }

    match data[80] {
        BOUNTY_OPEN | BOUNTY_CANCELLED => data[90] == u8::MAX,
        BOUNTY_COMPLETED => data[81] > 0 && data[90] < data[81],
        _ => false,
    }
}

// ============================================================================
// SUBMISSION LAYOUT
// ============================================================================
//
// Bytes 0..32  : worker (address)
// Bytes 32..64 : proof_hash (32 bytes)
// Bytes 64..72 : submitted_slot (u64 LE)

const SUBMISSION_SIZE: usize = 72;

fn encode_submission(worker: &[u8; 32], proof_hash: &[u8; 32], submitted_slot: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(SUBMISSION_SIZE);
    data.extend_from_slice(worker);
    data.extend_from_slice(proof_hash);
    data.extend_from_slice(&u64_to_bytes(submitted_slot));
    data
}

fn submission_row_valid(data: &[u8]) -> bool {
    data.len() == SUBMISSION_SIZE
        && data[..32].iter().any(|byte| *byte != 0)
        && data[32..64].iter().any(|byte| *byte != 0)
}

fn submission_matches_bounty(submission: &[u8], bounty: &[u8]) -> bool {
    if !submission_row_valid(submission) || !bounty_row_valid(bounty) {
        return false;
    }
    let submitted_slot = bytes_to_u64(&submission[64..72]);
    submitted_slot >= bytes_to_u64(&bounty[82..90])
        && submitted_slot <= bytes_to_u64(&bounty[72..80])
}

// ============================================================================
// CREATE BOUNTY
// ============================================================================

/// Create a new bounty.
///
/// Parameters:
///   - creator_ptr: 32-byte creator address
///   - title_hash_ptr: 32-byte hash of the bounty title/description
///   - reward_amount: reward in spores
///   - deadline_slot: deadline for submissions
///
/// Returns 0 on success, bounty_id in return data.
#[no_mangle]
pub extern "C" fn create_bounty(
    creator_ptr: *const u8,
    title_hash_ptr: *const u8,
    reward_amount: u64,
    deadline_slot: u64,
) -> u32 {
    log_info("Creating bounty...");
    // AUDIT-FIX P2: Enforce pause
    if effectively_paused() {
        log_info("BountyBoard is paused or accounting is unavailable");
        return ERR_PAUSED;
    }
    if !reentrancy_enter() {
        return 100;
    }

    let creator_arr = match read_address(creator_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 200;
        }
    };
    let title_arr = match read_address(title_hash_ptr) {
        Some(hash) => hash,
        None => {
            reentrancy_exit();
            return 2;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != creator_arr {
        reentrancy_exit();
        return 200;
    }

    if reward_amount == 0 {
        log_info("Reward must be > 0");
        reentrancy_exit();
        return 1;
    }
    if title_arr.iter().all(|byte| *byte == 0) {
        log_info("Title hash must be non-zero");
        reentrancy_exit();
        return 3;
    }

    // LichenID reputation gate
    if !check_identity_gate(&creator_arr) {
        log_info("Insufficient LichenID reputation for bounty creation");
        reentrancy_exit();
        return 10;
    }

    let current_slot = get_slot();
    if deadline_slot <= current_slot {
        log_info("Deadline must be in the future");
        reentrancy_exit();
        return 2;
    }

    let bounty_id = match checked_stored_u64(b"bounty_count") {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 12;
        }
    };
    let next_bounty_id = match bounty_id.checked_add(1) {
        Some(next) => next,
        None => {
            log_info("Bounty count overflow");
            reentrancy_exit();
            return 12;
        }
    };

    let reward_token = match reward_token_or_native() {
        Some(token) => token,
        None => {
            log_info("Invalid reward token configuration");
            reentrancy_exit();
            return 14;
        }
    };
    let fee_bps = match checked_stored_u64(BB_PLATFORM_FEE_BPS_KEY) {
        Some(value) if value <= 1_000 => value,
        _ => {
            reentrancy_exit();
            return 15;
        }
    };
    let escrow_liability = match checked_stored_u64(BB_ESCROW_LIABILITY_KEY) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 15;
        }
    };
    let next_escrow_liability = match escrow_liability.checked_add(reward_amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 15;
        }
    };
    let attached_value = get_value();
    let payment_value_is_exact = if reward_token.0 == [0u8; 32] {
        attached_value == reward_amount
    } else {
        attached_value == 0
    };
    if !payment_value_is_exact {
        log_info("Native payment value does not match the configured reward asset");
        reentrancy_exit();
        return 11;
    }
    if !receive_token_or_native(
        reward_token,
        Address(creator_arr),
        get_contract_address(),
        reward_amount,
    )
    .unwrap_or(false)
    {
        log_info("Insufficient reward escrow payment");
        reentrancy_exit();
        return 11;
    }
    storage_set(b"bounty_count", &u64_to_bytes(next_bounty_id));
    storage_set(
        BB_ESCROW_LIABILITY_KEY,
        &u64_to_bytes(next_escrow_liability),
    );

    let data = encode_bounty(BountyEncoding {
        creator: &creator_arr,
        title_hash: &title_arr,
        reward_amount,
        deadline_slot,
        status: BOUNTY_OPEN,
        submission_count: 0,
        created_slot: current_slot,
        approved_idx: 0xFF,
    });

    let bk = bounty_key(bounty_id);
    storage_set(&bk, &data);
    storage_set(&bounty_token_key(bounty_id), &reward_token.0);
    storage_set(&bounty_fee_bps_key(bounty_id), &u64_to_bytes(fee_bps));

    lichen_sdk::set_return_data(&u64_to_bytes(bounty_id));
    log_info("Bounty created");
    reentrancy_exit();
    0
}

// ============================================================================
// SUBMIT WORK
// ============================================================================

/// Submit work for a bounty.
///
/// Parameters:
///   - bounty_id: the bounty to submit work for
///   - worker_ptr: 32-byte worker address
///   - proof_hash_ptr: 32-byte hash of the proof of work
///
/// Returns 0 on success.
#[no_mangle]
pub extern "C" fn submit_work(
    bounty_id: u64,
    worker_ptr: *const u8,
    proof_hash_ptr: *const u8,
) -> u32 {
    log_info("Submitting work for bounty...");
    // AUDIT-FIX P2: Enforce pause
    if effectively_paused() {
        log_info("BountyBoard is paused or accounting is unavailable");
        return ERR_PAUSED;
    }
    if !reentrancy_enter() {
        return 100;
    }

    let worker_arr = match read_address(worker_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 200;
        }
    };
    let proof_arr = match read_address(proof_hash_ptr) {
        Some(hash) => hash,
        None => {
            reentrancy_exit();
            return 2;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != worker_arr {
        reentrancy_exit();
        return 200;
    }

    let bk = bounty_key(bounty_id);
    let mut bounty_data = match storage_get(&bk) {
        Some(data) => data,
        None => {
            log_info("Bounty not found");
            reentrancy_exit();
            return 1;
        }
    };

    if !bounty_row_valid(&bounty_data) {
        reentrancy_exit();
        return 2;
    }

    if bounty_data[80] != BOUNTY_OPEN {
        log_info("Bounty is not open");
        reentrancy_exit();
        return 3;
    }
    if proof_arr.iter().all(|byte| *byte == 0) {
        log_info("Proof hash must be non-zero");
        reentrancy_exit();
        return 6;
    }
    if bounty_data[0..32] == worker_arr[..] {
        log_info("Bounty creator cannot submit to their own bounty");
        reentrancy_exit();
        return 7;
    }

    // LichenID identity gate (any reputation level)
    if !check_identity_gate(&worker_arr) {
        log_info("LichenID identity required to submit work");
        reentrancy_exit();
        return 10;
    }

    // Check deadline
    let deadline = bytes_to_u64(&bounty_data[72..80]);
    let current_slot = get_slot();
    if current_slot > deadline {
        log_info("Bounty deadline passed");
        reentrancy_exit();
        return 4;
    }

    let sub_count = bounty_data[81];
    if sub_count == u8::MAX {
        log_info("Maximum submissions reached");
        reentrancy_exit();
        return 5;
    }
    let worker_key = worker_submission_key(bounty_id, &worker_arr);
    if storage_get(&worker_key).is_some() {
        log_info("Worker already submitted to this bounty");
        reentrancy_exit();
        return 8;
    }

    // Store submission
    let sk = submission_key(bounty_id, sub_count);
    let sub_data = encode_submission(&worker_arr, &proof_arr, current_slot);
    storage_set(&sk, &sub_data);
    storage_set(&worker_key, &[sub_count]);

    // Increment submission count
    bounty_data[81] = sub_count + 1;
    storage_set(&bk, &bounty_data);

    lichen_sdk::set_return_data(&[sub_count]); // return submission index
    log_info("Work submitted");
    reentrancy_exit();
    0
}

// ============================================================================
// APPROVE WORK
// ============================================================================

/// Creator approves a submission and pays the reward.
///
/// Parameters:
///   - caller_ptr: 32-byte caller address (must be creator)
///   - bounty_id: the bounty
///   - submission_idx: index of submission to approve
///
/// Returns 0 on success.
#[no_mangle]
pub extern "C" fn approve_work(caller_ptr: *const u8, bounty_id: u64, submission_idx: u8) -> u32 {
    log_info("Approving bounty work...");
    // AUDIT-FIX P2: Enforce pause
    if effectively_paused() {
        log_info("BountyBoard is paused or accounting is unavailable");
        return ERR_PAUSED;
    }
    if !reentrancy_enter() {
        return 100;
    }

    let caller = match read_address(caller_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 200;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    let bk = bounty_key(bounty_id);
    let mut bounty_data = match storage_get(&bk) {
        Some(data) => data,
        None => {
            log_info("Bounty not found");
            reentrancy_exit();
            return 1;
        }
    };

    if !bounty_row_valid(&bounty_data) {
        reentrancy_exit();
        return 2;
    }

    // Verify caller is creator
    if bounty_data[0..32] != caller[..] {
        log_info("Only creator can approve");
        reentrancy_exit();
        return 3;
    }

    if bounty_data[80] != BOUNTY_OPEN {
        log_info("Bounty is not open");
        reentrancy_exit();
        return 4;
    }

    let sub_count = bounty_data[81];
    if submission_idx >= sub_count {
        log_info("Invalid submission index");
        reentrancy_exit();
        return 5;
    }

    // Load submission to get worker address
    let sk = submission_key(bounty_id, submission_idx);
    let sub_data = match storage_get(&sk) {
        Some(data) => data,
        None => {
            log_info("Submission not found");
            reentrancy_exit();
            return 6;
        }
    };
    if !submission_matches_bounty(&sub_data, &bounty_data) {
        log_info("Invalid submission data");
        reentrancy_exit();
        return 6;
    }

    // Transfer reward tokens from contract to worker via self-custody
    // AUDIT-FIX G22-01: Use contract's own address as source (self-custody pattern)
    let reward_amount = bytes_to_u64(&bounty_data[64..72]);
    let reward_token = match bounty_reward_token(bounty_id) {
        Some(token) => token,
        None => {
            log_info("Invalid reward token configuration");
            reentrancy_exit();
            return 9;
        }
    };
    let self_addr = get_contract_address();
    let mut worker_addr = [0u8; 32];
    worker_addr.copy_from_slice(&sub_data[0..32]);
    let fee_bps = match bounty_platform_fee_bps(bounty_id) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 15;
        }
    };
    let (worker_payment, platform_fee) = match split_reward(reward_amount, fee_bps) {
        Some(split) => split,
        None => {
            log_info("Invalid snapshotted platform fee");
            reentrancy_exit();
            return 15;
        }
    };
    let fee_key = platform_fee_key(reward_token);
    let accrued_platform_fee = match checked_stored_u64(&fee_key) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 15;
        }
    };
    let next_platform_fee = match accrued_platform_fee.checked_add(platform_fee) {
        Some(next) => next,
        None => {
            log_info("Platform fee accounting overflow");
            reentrancy_exit();
            return 15;
        }
    };
    let escrow_liability = match checked_stored_u64(BB_ESCROW_LIABILITY_KEY) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 15;
        }
    };
    let next_escrow_liability = match escrow_liability.checked_sub(reward_amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 15;
        }
    };
    let next_completed_count = match checked_increment(BB_COMPLETED_COUNT_KEY) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 15;
        }
    };
    let reward_volume = match checked_stored_u64(BB_REWARD_VOLUME_KEY) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 15;
        }
    };
    let next_reward_volume = match reward_volume.checked_add(reward_amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 15;
        }
    };

    // Mark bounty as completed, then revert this effect if payout fails.
    bounty_data[80] = BOUNTY_COMPLETED;
    bounty_data[90] = submission_idx;
    storage_set(&bk, &bounty_data);

    match transfer_token_or_native(
        reward_token,
        self_addr,
        Address(worker_addr),
        worker_payment,
    ) {
        Ok(true) => {
            log_info("Reward transferred successfully");
        }
        Ok(false) => {
            bounty_data[80] = BOUNTY_OPEN;
            bounty_data[90] = 0xFF;
            storage_set(&bk, &bounty_data);
            log_info("Reward transfer returned false, bounty reverted to open");
            reentrancy_exit();
            return 8;
        }
        Err(_) => {
            bounty_data[80] = BOUNTY_OPEN;
            bounty_data[90] = 0xFF;
            storage_set(&bk, &bounty_data);
            log_info("Reward transfer failed, bounty reverted to open");
            reentrancy_exit();
            return 7;
        }
    }
    storage_set(&fee_key, &u64_to_bytes(next_platform_fee));
    storage_set(
        BB_ESCROW_LIABILITY_KEY,
        &u64_to_bytes(next_escrow_liability),
    );
    let mut settlement = Vec::with_capacity(16);
    settlement.extend_from_slice(&u64_to_bytes(worker_payment));
    settlement.extend_from_slice(&u64_to_bytes(platform_fee));
    lichen_sdk::set_return_data(&settlement);

    // Track completion stats
    storage_set(BB_COMPLETED_COUNT_KEY, &u64_to_bytes(next_completed_count));
    storage_set(BB_REWARD_VOLUME_KEY, &u64_to_bytes(next_reward_volume));

    log_info("Work approved, bounty completed");
    reentrancy_exit();
    0
}

// ============================================================================
// CANCEL BOUNTY
// ============================================================================

/// Creator cancels a bounty (refund).
///
/// Parameters:
///   - caller_ptr: 32-byte caller address (must be creator)
///   - bounty_id: the bounty to cancel
///
/// Returns 0 on success.
#[no_mangle]
pub extern "C" fn cancel_bounty(caller_ptr: *const u8, bounty_id: u64) -> u32 {
    log_info("Cancelling bounty...");
    if !reentrancy_enter() {
        return 100;
    }
    if !accounting_operational() {
        reentrancy_exit();
        return ERR_PAUSED;
    }

    let caller = match read_address(caller_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 200;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    let bk = bounty_key(bounty_id);
    let mut bounty_data = match storage_get(&bk) {
        Some(data) => data,
        None => {
            log_info("Bounty not found");
            reentrancy_exit();
            return 1;
        }
    };

    if !bounty_row_valid(&bounty_data) {
        reentrancy_exit();
        return 2;
    }

    if bounty_data[0..32] != caller[..] {
        log_info("Only creator can cancel");
        reentrancy_exit();
        return 3;
    }

    if bounty_data[80] != BOUNTY_OPEN {
        log_info("Bounty is not open");
        reentrancy_exit();
        return 4;
    }

    // Once a worker has submitted, preserve the advertised review window.
    // The creator may still approve at any time, or reclaim the escrow after
    // the deadline. This prevents cancellation from rugging in-window work.
    if bounty_data[81] > 0 && get_slot() <= bytes_to_u64(&bounty_data[72..80]) {
        log_info("Cannot cancel a submitted bounty before its deadline");
        reentrancy_exit();
        return 11;
    }

    let reward = bytes_to_u64(&bounty_data[64..72]);

    // AUDIT-FIX G22-01: Transfer refund from contract to creator (self-custody)
    let mut creator_addr = [0u8; 32];
    creator_addr.copy_from_slice(&bounty_data[0..32]);
    let reward_token = match bounty_reward_token(bounty_id) {
        Some(token) => token,
        None => {
            log_info("Invalid reward token configuration");
            reentrancy_exit();
            return 9;
        }
    };
    if bounty_platform_fee_bps(bounty_id).is_none() {
        reentrancy_exit();
        return 9;
    }
    let escrow_liability = match checked_stored_u64(BB_ESCROW_LIABILITY_KEY) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 10;
        }
    };
    let next_escrow_liability = match escrow_liability.checked_sub(reward) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 10;
        }
    };
    let next_cancel_count = match checked_increment(BB_CANCEL_COUNT_KEY) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 10;
        }
    };

    bounty_data[80] = BOUNTY_CANCELLED;
    storage_set(&bk, &bounty_data);

    if reward > 0 {
        let self_addr = get_contract_address();
        match transfer_token_or_native(reward_token, self_addr, Address(creator_addr), reward) {
            Ok(true) => {
                log_info("Refund transferred successfully");
            }
            Ok(false) | Err(_) => {
                // Revert cancellation on transfer failure
                bounty_data[80] = BOUNTY_OPEN;
                storage_set(&bk, &bounty_data);
                log_info("Refund transfer failed, cancellation reverted");
                reentrancy_exit();
                return 8;
            }
        }
    }

    lichen_sdk::set_return_data(&u64_to_bytes(reward));

    storage_set(
        BB_ESCROW_LIABILITY_KEY,
        &u64_to_bytes(next_escrow_liability),
    );
    storage_set(BB_CANCEL_COUNT_KEY, &u64_to_bytes(next_cancel_count));

    log_info("Bounty cancelled, refund issued");
    reentrancy_exit();
    0
}

// ============================================================================
// GET BOUNTY
// ============================================================================

/// Query bounty information.
///
/// Parameters:
///   - bounty_id: the bounty to query
///
/// Returns 0 on success (bounty data as return data), 1 if not found.
#[no_mangle]
pub extern "C" fn get_bounty(bounty_id: u64) -> u32 {
    let bk = bounty_key(bounty_id);
    match storage_get(&bk) {
        Some(data) if bounty_row_valid(&data) => {
            lichen_sdk::set_return_data(&data);
            0
        }
        None => {
            log_info("Bounty not found");
            1
        }
        Some(_) => 2,
    }
}

/// Query one submission. Returns worker, proof hash, and submitted/updated slot.
#[no_mangle]
pub extern "C" fn get_submission(bounty_id: u64, submission_idx: u8) -> u32 {
    match storage_get(&submission_key(bounty_id, submission_idx)) {
        Some(data) if submission_row_valid(&data) => {
            lichen_sdk::set_return_data(&data);
            0
        }
        None => 1,
        Some(_) => 2,
    }
}

/// Replace a worker's proof while the bounty is open and accepting work.
#[no_mangle]
pub extern "C" fn update_work(
    bounty_id: u64,
    submission_idx: u8,
    worker_ptr: *const u8,
    proof_hash_ptr: *const u8,
) -> u32 {
    if effectively_paused() {
        return ERR_PAUSED;
    }
    if !reentrancy_enter() {
        return 100;
    }
    let worker = match read_address(worker_ptr) {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 200;
        }
    };
    let proof_hash = match read_address(proof_hash_ptr) {
        Some(hash) => hash,
        None => {
            reentrancy_exit();
            return 2;
        }
    };
    if get_caller().0 != worker {
        reentrancy_exit();
        return 200;
    }
    if proof_hash.iter().all(|byte| *byte == 0) {
        reentrancy_exit();
        return 2;
    }
    let bounty = match storage_get(&bounty_key(bounty_id)) {
        Some(data) if bounty_row_valid(&data) => data,
        _ => {
            reentrancy_exit();
            return 1;
        }
    };
    if bounty[80] != BOUNTY_OPEN {
        reentrancy_exit();
        return 3;
    }
    if get_slot() > bytes_to_u64(&bounty[72..80]) {
        reentrancy_exit();
        return 4;
    }
    let key = submission_key(bounty_id, submission_idx);
    let mut submission = match storage_get(&key) {
        Some(data) if submission_matches_bounty(&data, &bounty) => data,
        _ => {
            reentrancy_exit();
            return 5;
        }
    };
    if submission[0..32] != worker[..] {
        reentrancy_exit();
        return 6;
    }
    submission[32..64].copy_from_slice(&proof_hash);
    submission[64..72].copy_from_slice(&u64_to_bytes(get_slot()));
    storage_set(&key, &submission);
    reentrancy_exit();
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
/// Storage key for the reward token contract address (32 bytes)
const TOKEN_ADDRESS_KEY: &[u8] = b"bounty_token_addr";

/// Initialize protocol administration and fresh Accounting V2 state.
/// Deployment must invoke this atomically before making the program public.
#[no_mangle]
pub extern "C" fn set_identity_admin(admin_ptr: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let admin = match read_address(admin_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 200;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != admin {
        reentrancy_exit();
        return 200;
    }

    if storage_get(IDENTITY_ADMIN_KEY).is_some() {
        log_info("Identity admin already set");
        reentrancy_exit();
        return 1;
    }
    if admin.iter().all(|byte| *byte == 0) {
        reentrancy_exit();
        return 2;
    }

    storage_set(IDENTITY_ADMIN_KEY, &admin);
    storage_set(BB_FEE_TREASURY_KEY, &admin);
    storage_set(BB_PLATFORM_FEE_BPS_KEY, &u64_to_bytes(0));
    storage_set(
        BB_ACCOUNTING_VERSION_KEY,
        &u64_to_bytes(ACCOUNTING_VERSION_V2),
    );
    storage_set(BB_ESCROW_LIABILITY_KEY, &u64_to_bytes(0));
    storage_set(BB_MIGRATION_LOCK_KEY, &[0]);
    storage_set(BB_MIGRATION_EXPECTED_COUNT_KEY, &u64_to_bytes(0));
    storage_set(BB_MIGRATION_CURSOR_KEY, &u64_to_bytes(0));
    storage_set(BB_MIGRATION_ESCROW_KEY, &u64_to_bytes(0));
    storage_set(b"bb_paused", &[0]);
    log_info("Identity admin set");
    reentrancy_exit();
    0
}

/// Set LichenID contract address for cross-contract reputation lookups.
/// Only callable by the identity admin.
#[no_mangle]
pub extern "C" fn set_lichenid_address(caller_ptr: *const u8, lichenid_addr_ptr: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let caller = match read_address(caller_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 200;
        }
    };
    let lichenid_addr = match read_address(lichenid_addr_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 3;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    let admin = match storage_get(IDENTITY_ADMIN_KEY) {
        Some(data) => data,
        None => {
            reentrancy_exit();
            return 1;
        }
    };
    if caller[..] != admin[..] {
        reentrancy_exit();
        return 2;
    }

    if lichenid_addr.iter().all(|&b| b == 0) {
        reentrancy_exit();
        return 3;
    }

    if storage_get(LICHENID_ADDR_KEY).is_some() {
        reentrancy_exit();
        return 4;
    }

    storage_set(LICHENID_ADDR_KEY, &lichenid_addr);
    log_info("LichenID address configured");
    reentrancy_exit();
    0
}

/// Set minimum LichenID reputation required for gated functions.
/// Only callable by the identity admin.
#[no_mangle]
pub extern "C" fn set_identity_gate(caller_ptr: *const u8, min_reputation: u64) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let caller = match read_address(caller_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 200;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    let admin = match storage_get(IDENTITY_ADMIN_KEY) {
        Some(data) => data,
        None => {
            reentrancy_exit();
            return 1;
        }
    };
    if caller[..] != admin[..] {
        reentrancy_exit();
        return 2;
    }

    if min_reputation > 0 {
        match load_configured_address(LICHENID_ADDR_KEY) {
            Some(address) if address.iter().any(|byte| *byte != 0) => {}
            _ => {
                reentrancy_exit();
                return 3;
            }
        }
    }

    storage_set(LICHENID_MIN_REP_KEY, &u64_to_bytes(min_reputation));
    log_info("Identity gate configured");
    reentrancy_exit();
    0
}

/// Set the reward token contract address.
/// Only callable by the identity admin.
#[no_mangle]
pub extern "C" fn set_token_address(caller_ptr: *const u8, token_addr_ptr: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let caller = match read_address(caller_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 200;
        }
    };
    let token_addr = match read_address(token_addr_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 3;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    let admin = match storage_get(IDENTITY_ADMIN_KEY) {
        Some(data) => data,
        None => {
            reentrancy_exit();
            return 1;
        } // no admin set
    };
    if caller[..] != admin[..] {
        reentrancy_exit();
        return 2; // not admin
    }
    // The all-zero address is an explicit native LICN binding. A fresh,
    // unused zero binding may be replaced during deployment staging; once any
    // bounty exists the payment asset is immutable.
    if let Some(existing) = storage_get(TOKEN_ADDRESS_KEY) {
        if existing.len() != 32 {
            reentrancy_exit();
            return 4;
        }
        if existing.as_slice() == token_addr {
            reentrancy_exit();
            return 0;
        }
        if existing.iter().any(|byte| *byte != 0)
            || checked_stored_u64(b"bounty_count") != Some(0)
            || checked_stored_u64(BB_ESCROW_LIABILITY_KEY) != Some(0)
        {
            reentrancy_exit();
            return 4;
        }
    }

    storage_set(TOKEN_ADDRESS_KEY, &token_addr);
    log_info("Reward token address configured");
    reentrancy_exit();
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

    let lichenid_addr = match storage_get(LICHENID_ADDR_KEY) {
        Some(data) if data.len() == 32 => data,
        _ => return false,
    };

    let mut addr = [0u8; 32];
    addr.copy_from_slice(&lichenid_addr[..32]);
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
// ALIASES — bridge test-expected names to actual implementation
// ============================================================================

/// Tests expect `initialize` — admin setup
#[no_mangle]
pub extern "C" fn initialize(admin_ptr: *const u8) -> u32 {
    set_identity_admin(admin_ptr)
}

/// Propose a new protocol administrator. The proposed address must accept in a
/// separate transaction, so a typo cannot immediately orphan the contract.
#[no_mangle]
pub extern "C" fn propose_admin(caller_ptr: *const u8, new_admin_ptr: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 200;
        }
    };
    let new_admin = match read_address(new_admin_ptr) {
        Some(address) if address.iter().any(|byte| *byte != 0) => address,
        Some(_) | None => {
            reentrancy_exit();
            return 2;
        }
    };
    if get_caller().0 != caller {
        reentrancy_exit();
        return 200;
    }
    if !require_admin(&caller) {
        reentrancy_exit();
        return 1;
    }
    if caller == new_admin {
        reentrancy_exit();
        return 2;
    }
    match storage_get(BB_PENDING_ADMIN_KEY) {
        None => {}
        Some(data) if data.len() == 32 && data.as_slice() == new_admin => {
            reentrancy_exit();
            return 0;
        }
        Some(data) if data.len() == 32 => {}
        Some(_) => {
            reentrancy_exit();
            return 3;
        }
    }
    storage_set(BB_PENDING_ADMIN_KEY, &new_admin);
    log_info("BountyBoard administrator proposed");
    reentrancy_exit();
    0
}

/// Accept a pending administrator role using the proposed key itself.
#[no_mangle]
pub extern "C" fn accept_admin(caller_ptr: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let caller = match read_address(caller_ptr) {
        Some(address) if address.iter().any(|byte| *byte != 0) => address,
        Some(_) | None => {
            reentrancy_exit();
            return 200;
        }
    };
    if get_caller().0 != caller {
        reentrancy_exit();
        return 200;
    }
    match storage_get(BB_PENDING_ADMIN_KEY) {
        Some(data) if data.len() == 32 && data.as_slice() == caller => {}
        Some(data) if data.len() == 32 => {
            reentrancy_exit();
            return 1;
        }
        None => {
            reentrancy_exit();
            return 2;
        }
        Some(_) => {
            reentrancy_exit();
            return 3;
        }
    }
    storage_set(IDENTITY_ADMIN_KEY, &caller);
    lichen_sdk::storage::remove(BB_PENDING_ADMIN_KEY);
    log_info("BountyBoard administrator accepted");
    reentrancy_exit();
    0
}

/// Return current and pending administrator addresses as exactly 64 bytes.
/// An all-zero pending address means that no transition is active.
#[no_mangle]
pub extern "C" fn get_admin_transition() -> u32 {
    let current = match storage_get(IDENTITY_ADMIN_KEY) {
        Some(data) if data.len() == 32 && data.iter().any(|byte| *byte != 0) => data,
        _ => return 1,
    };
    let pending = match storage_get(BB_PENDING_ADMIN_KEY) {
        None => [0u8; 32],
        Some(data) if data.len() == 32 && data.iter().any(|byte| *byte != 0) => {
            let mut address = [0u8; 32];
            address.copy_from_slice(&data);
            address
        }
        Some(_) => return 2,
    };
    let mut result = Vec::with_capacity(64);
    result.extend_from_slice(&current);
    result.extend_from_slice(&pending);
    lichen_sdk::set_return_data(&result);
    0
}

/// Revoke a pending administrator handoff before it is accepted.
#[no_mangle]
pub extern "C" fn cancel_admin_proposal(caller_ptr: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 200;
        }
    };
    if get_caller().0 != caller {
        reentrancy_exit();
        return 200;
    }
    if !require_admin(&caller) {
        reentrancy_exit();
        return 1;
    }
    match storage_get(BB_PENDING_ADMIN_KEY) {
        None => {}
        Some(data) if data.len() == 32 && data.iter().any(|byte| *byte != 0) => {
            lichen_sdk::storage::remove(BB_PENDING_ADMIN_KEY);
        }
        Some(_) => {
            reentrancy_exit();
            return 2;
        }
    }
    log_info("BountyBoard administrator proposal cancelled");
    reentrancy_exit();
    0
}

/// Alias: tests call `approve_submission` but contract uses `approve_work`
#[no_mangle]
pub extern "C" fn approve_submission(
    caller_ptr: *const u8,
    bounty_id: u64,
    submission_idx: u8,
) -> u32 {
    approve_work(caller_ptr, bounty_id, submission_idx)
}

/// Tests expect `get_bounty_count`
#[no_mangle]
pub extern "C" fn get_bounty_count() -> u64 {
    checked_stored_u64(b"bounty_count").unwrap_or(0)
}

/// Return the bounty count through return data so malformed state is
/// distinguishable from a legitimate zero-bounty board. The legacy
/// `get_bounty_count` return-value view remains exported for compatibility.
#[no_mangle]
pub extern "C" fn get_bounty_count_exact() -> u32 {
    let count = match checked_stored_u64(b"bounty_count") {
        Some(value) => value,
        None => return 2,
    };
    lichen_sdk::set_return_data(&u64_to_bytes(count));
    0
}

/// Tests expect `set_platform_fee`
#[no_mangle]
pub extern "C" fn set_platform_fee(caller_ptr: *const u8, fee_bps: u64) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let caller = match read_address(caller_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 200;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    if !require_admin(&caller) {
        reentrancy_exit();
        return 1;
    }
    if !accounting_operational() {
        reentrancy_exit();
        return 6;
    }
    if fee_bps > 1000 {
        reentrancy_exit();
        return 2;
    }
    if !migration_lock_valid() || migration_locked() {
        reentrancy_exit();
        return 3;
    }
    storage_set(BB_PLATFORM_FEE_BPS_KEY, &u64_to_bytes(fee_bps));
    log_info("Platform fee set");
    reentrancy_exit();
    0
}

/// Set the recipient for realized platform-fee withdrawals.
#[no_mangle]
pub extern "C" fn set_fee_treasury(caller_ptr: *const u8, treasury_ptr: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 200;
        }
    };
    let treasury = match read_address(treasury_ptr) {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 3;
        }
    };
    if get_caller().0 != caller {
        reentrancy_exit();
        return 200;
    }
    if !require_admin(&caller) {
        reentrancy_exit();
        return 1;
    }
    if treasury.iter().all(|byte| *byte == 0) {
        reentrancy_exit();
        return 2;
    }
    storage_set(BB_FEE_TREASURY_KEY, &treasury);
    reentrancy_exit();
    0
}

/// Withdraw an exact amount of realized fees to the configured treasury.
#[no_mangle]
pub extern "C" fn withdraw_platform_fees(
    caller_ptr: *const u8,
    token_ptr: *const u8,
    amount: u64,
) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 200;
        }
    };
    let token = match read_address(token_ptr) {
        Some(address) => Address(address),
        None => {
            reentrancy_exit();
            return 3;
        }
    };
    if get_caller().0 != caller {
        reentrancy_exit();
        return 200;
    }
    if !require_admin(&caller) {
        reentrancy_exit();
        return 1;
    }
    if !accounting_operational() {
        reentrancy_exit();
        return 6;
    }
    if reward_token_or_native() != Some(token) {
        reentrancy_exit();
        return 3;
    }
    if amount == 0 {
        reentrancy_exit();
        return 2;
    }
    let treasury = match load_configured_address(BB_FEE_TREASURY_KEY) {
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
            return 6;
        }
    };
    let remaining = match accrued.checked_sub(amount) {
        Some(value) => value,
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

/// Query realized platform fees for a reward asset.
#[no_mangle]
pub extern "C" fn get_platform_fees(token_ptr: *const u8) -> u32 {
    let token = match read_address(token_ptr) {
        Some(address) => Address(address),
        None => return 3,
    };
    if reward_token_or_native() != Some(token) {
        return 4;
    }
    let fees = match checked_stored_u64(&platform_fee_key(token)) {
        Some(value) => value,
        None => return 5,
    };
    lichen_sdk::set_return_data(&u64_to_bytes(fees));
    0
}

/// Compatibility helper for the current Accounting V2 cursor. It can only bind
/// the already configured canonical asset while migration is locked; the
/// regular migration step can perform the same deterministic snapshot itself.
#[no_mangle]
pub extern "C" fn migrate_bounty_token(
    caller_ptr: *const u8,
    bounty_id: u64,
    token_ptr: *const u8,
) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 200;
        }
    };
    let token = match read_address(token_ptr) {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 3;
        }
    };
    if get_caller().0 != caller {
        reentrancy_exit();
        return 200;
    }
    if !require_admin(&caller) {
        reentrancy_exit();
        return 1;
    }
    if !migration_locked() || checked_stored_u64(BB_MIGRATION_CURSOR_KEY) != Some(bounty_id) {
        reentrancy_exit();
        return 6;
    }
    if reward_token_or_native() != Some(Address(token)) {
        reentrancy_exit();
        return 7;
    }
    let bounty = match storage_get(&bounty_key(bounty_id)) {
        Some(data) if bounty_row_valid(&data) => data,
        _ => {
            reentrancy_exit();
            return 2;
        }
    };
    if bounty[80] != BOUNTY_OPEN {
        reentrancy_exit();
        return 4;
    }
    if storage_get(&bounty_token_key(bounty_id)).is_some() {
        reentrancy_exit();
        return 5;
    }
    storage_set(&bounty_token_key(bounty_id), &token);
    storage_set(&bounty_fee_bps_key(bounty_id), &u64_to_bytes(0));
    reentrancy_exit();
    0
}

/// Query a bounty's snapshotted payment token, fee, gross reward, worker net,
/// and realized fee. Returns 64 bytes.
#[no_mangle]
pub extern "C" fn get_bounty_terms(bounty_id: u64) -> u32 {
    let bounty = match storage_get(&bounty_key(bounty_id)) {
        Some(data) if bounty_row_valid(&data) => data,
        None => return 1,
        Some(_) => return 4,
    };
    let token = match bounty_reward_token(bounty_id) {
        Some(value) => value,
        None => return 2,
    };
    let reward = bytes_to_u64(&bounty[64..72]);
    let fee_bps = match bounty_platform_fee_bps(bounty_id) {
        Some(value) => value,
        None => return 3,
    };
    let (worker_net, fee) = match split_reward(reward, fee_bps) {
        Some(split) => split,
        None => return 3,
    };
    let mut terms = Vec::with_capacity(64);
    terms.extend_from_slice(&token.0);
    terms.extend_from_slice(&u64_to_bytes(fee_bps));
    terms.extend_from_slice(&u64_to_bytes(reward));
    terms.extend_from_slice(&u64_to_bytes(worker_net));
    terms.extend_from_slice(&u64_to_bytes(fee));
    lichen_sdk::set_return_data(&terms);
    0
}

/// Return the exact immutable bounty row plus the presence and value of its
/// token and fee snapshots. This makes legacy migration manifests source-bound
/// even when one or both snapshots do not exist yet. Returns exactly 147 bytes:
/// bounty (91), token-present u64, token (32), fee-present u64, fee-bps u64.
#[no_mangle]
pub extern "C" fn get_bounty_migration_record(bounty_id: u64) -> u32 {
    let bounty = match storage_get(&bounty_key(bounty_id)) {
        Some(data) if bounty_row_valid(&data) => data,
        None => return 1,
        Some(_) => return 4,
    };
    let (token_present, token) = match storage_get(&bounty_token_key(bounty_id)) {
        None => (false, [0u8; 32]),
        Some(data) if data.len() == 32 => {
            let mut token = [0u8; 32];
            token.copy_from_slice(&data);
            (true, token)
        }
        Some(_) => return 2,
    };
    let (fee_present, fee_bps) = match storage_get(&bounty_fee_bps_key(bounty_id)) {
        None => (false, 0),
        Some(data) if data.len() == 8 => {
            let fee_bps = bytes_to_u64(&data);
            if fee_bps > 1_000 {
                return 3;
            }
            (true, fee_bps)
        }
        Some(_) => return 3,
    };

    let mut record = Vec::with_capacity(147);
    record.extend_from_slice(&bounty);
    record.extend_from_slice(&u64_to_bytes(u64::from(token_present)));
    record.extend_from_slice(&token);
    record.extend_from_slice(&u64_to_bytes(u64::from(fee_present)));
    record.extend_from_slice(&u64_to_bytes(fee_bps));
    lichen_sdk::set_return_data(&record);
    0
}

// ============================================================================
// ACCOUNTING V2 MIGRATION AND SOLVENCY
// ============================================================================

/// Freeze a legacy deployment and bind migration to the immutable contiguous
/// bounty frontier. The board remains paused after completion until operators
/// independently verify the reconstructed liabilities and explicitly unpause.
#[no_mangle]
pub extern "C" fn begin_accounting_v2_migration(
    caller_ptr: *const u8,
    expected_bounty_count: u64,
) -> u32 {
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => return 200,
    };
    if get_caller().0 != caller {
        return 200;
    }
    if !require_admin(&caller) {
        return 1;
    }
    match accounting_version() {
        Some(ACCOUNTING_VERSION_V2) => return 2,
        Some(_) => {}
        None => return 8,
    }
    if !migration_lock_valid() {
        return 8;
    }
    if checked_stored_u64(b"bounty_count") != Some(expected_bounty_count) {
        return 3;
    }
    if reward_token_or_native().is_none() {
        return 4;
    }
    if migration_locked() {
        return if checked_stored_u64(BB_MIGRATION_EXPECTED_COUNT_KEY) == Some(expected_bounty_count)
        {
            0
        } else {
            5
        };
    }

    storage_set(b"bb_paused", &[1]);
    storage_set(BB_MIGRATION_LOCK_KEY, &[1]);
    storage_set(
        BB_MIGRATION_EXPECTED_COUNT_KEY,
        &u64_to_bytes(expected_bounty_count),
    );
    storage_set(BB_MIGRATION_CURSOR_KEY, &u64_to_bytes(0));
    storage_set(BB_MIGRATION_ESCROW_KEY, &u64_to_bytes(0));
    0
}

/// Reconstruct one exact bounty in ascending ID order. Missing legacy token
/// and fee snapshots are deterministically bound to the canonical configured
/// asset and a zero retroactive fee.
#[no_mangle]
pub extern "C" fn migrate_accounting_v2_bounty(bounty_id: u64) -> u32 {
    if !migration_locked() || accounting_version() == Some(ACCOUNTING_VERSION_V2) {
        return 1;
    }
    let cursor = match checked_stored_u64(BB_MIGRATION_CURSOR_KEY) {
        Some(value) => value,
        None => return 8,
    };
    let expected = match checked_stored_u64(BB_MIGRATION_EXPECTED_COUNT_KEY) {
        Some(value) => value,
        None => return 8,
    };
    if bounty_id != cursor || bounty_id >= expected {
        return 2;
    }
    let bounty = match storage_get(&bounty_key(bounty_id)) {
        Some(data) if bounty_row_valid(&data) => data,
        _ => return 3,
    };
    let status = bounty[80];
    let submission_count = bounty[81];
    let approved_idx = bounty[90];
    match status {
        BOUNTY_OPEN if approved_idx == u8::MAX => {}
        BOUNTY_COMPLETED if approved_idx < submission_count => {}
        BOUNTY_CANCELLED if approved_idx == u8::MAX => {}
        _ => return 4,
    }
    if status == BOUNTY_COMPLETED {
        let approved_submission = match storage_get(&submission_key(bounty_id, approved_idx)) {
            Some(data) => data,
            None => return 4,
        };
        if !submission_matches_bounty(&approved_submission, &bounty) {
            return 4;
        }
    }
    let canonical_token = match reward_token_or_native() {
        Some(token) => token,
        None => return 5,
    };
    let token_key = bounty_token_key(bounty_id);
    let write_token = match storage_get(&token_key) {
        None => true,
        Some(data) if data.len() == 32 && data.as_slice() == canonical_token.0 => false,
        Some(_) => return 5,
    };
    let fee_key = bounty_fee_bps_key(bounty_id);
    let (fee_bps, write_fee) = match storage_get(&fee_key) {
        None => (0, true),
        Some(data) if data.len() == 8 => (bytes_to_u64(&data), false),
        Some(_) => return 6,
    };
    if fee_bps > 1_000 {
        return 6;
    }
    let reward = bytes_to_u64(&bounty[64..72]);
    if reward == 0 {
        return 4;
    }
    let current_escrow = match checked_stored_u64(BB_MIGRATION_ESCROW_KEY) {
        Some(value) => value,
        None => return 8,
    };
    let next_escrow = if status == BOUNTY_OPEN {
        match current_escrow.checked_add(reward) {
            Some(value) => value,
            None => return 7,
        }
    } else {
        current_escrow
    };
    let next_cursor = match cursor.checked_add(1) {
        Some(value) => value,
        None => return 7,
    };

    if write_token {
        storage_set(&token_key, &canonical_token.0);
    }
    if write_fee {
        storage_set(&fee_key, &u64_to_bytes(0));
    }
    storage_set(BB_MIGRATION_ESCROW_KEY, &u64_to_bytes(next_escrow));
    storage_set(BB_MIGRATION_CURSOR_KEY, &u64_to_bytes(next_cursor));
    0
}

/// Activate Accounting V2 only after full reconstruction, independent expected
/// totals, and real custody all agree. Explicit pause remains set.
#[no_mangle]
pub extern "C" fn complete_accounting_v2_migration(
    caller_ptr: *const u8,
    expected_escrow: u64,
    expected_platform_fees: u64,
    expected_total_liability: u64,
) -> u32 {
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => return 200,
    };
    if get_caller().0 != caller {
        return 200;
    }
    if !require_admin(&caller) {
        return 1;
    }
    if !migration_locked() || accounting_version() == Some(ACCOUNTING_VERSION_V2) {
        return 2;
    }
    let expected_count = match checked_stored_u64(BB_MIGRATION_EXPECTED_COUNT_KEY) {
        Some(value) => value,
        None => return 8,
    };
    if checked_stored_u64(BB_MIGRATION_CURSOR_KEY) != Some(expected_count)
        || checked_stored_u64(b"bounty_count") != Some(expected_count)
    {
        return 3;
    }
    let escrow = match checked_stored_u64(BB_MIGRATION_ESCROW_KEY) {
        Some(value) => value,
        None => return 8,
    };
    let token = match reward_token_or_native() {
        Some(value) => value,
        None => return 4,
    };
    let fees = match checked_stored_u64(&platform_fee_key(token)) {
        Some(value) => value,
        None => return 8,
    };
    let total = match escrow.checked_add(fees) {
        Some(value) => value,
        None => return 7,
    };
    if escrow != expected_escrow
        || fees != expected_platform_fees
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

    storage_set(BB_ESCROW_LIABILITY_KEY, &u64_to_bytes(escrow));
    storage_set(
        BB_ACCOUNTING_VERSION_KEY,
        &u64_to_bytes(ACCOUNTING_VERSION_V2),
    );
    storage_set(BB_MIGRATION_LOCK_KEY, &[0]);
    0
}

/// Return expected bounty count, cursor, reconstructed escrow, accounting
/// version, and migration lock as five little-endian u64 values.
#[no_mangle]
pub extern "C" fn get_accounting_migration_status() -> u32 {
    let values = [
        checked_stored_u64(BB_MIGRATION_EXPECTED_COUNT_KEY),
        checked_stored_u64(BB_MIGRATION_CURSOR_KEY),
        checked_stored_u64(BB_MIGRATION_ESCROW_KEY),
        accounting_version(),
    ];
    if values.iter().any(Option::is_none) || !migration_lock_valid() {
        return 2;
    }
    let mut result = Vec::with_capacity(40);
    for value in values.into_iter().flatten() {
        result.extend_from_slice(&u64_to_bytes(value));
    }
    result.extend_from_slice(&u64_to_bytes(u64::from(migration_locked())));
    lichen_sdk::set_return_data(&result);
    0
}

/// Return version, migration lock, active escrow, platform fees, total
/// liability, real custody, and solvent/operational flag as seven u64 values.
#[no_mangle]
pub extern "C" fn get_accounting_health() -> u32 {
    let token = match reward_token_or_native() {
        Some(value) => value,
        None => return 1,
    };
    let version = match accounting_version() {
        Some(value) => value,
        None => return 2,
    };
    let escrow = match checked_stored_u64(BB_ESCROW_LIABILITY_KEY) {
        Some(value) => value,
        None => return 2,
    };
    let fees = match checked_stored_u64(&platform_fee_key(token)) {
        Some(value) => value,
        None => return 2,
    };
    let total = match escrow.checked_add(fees) {
        Some(value) => value,
        None => return 3,
    };
    let custody = match balance_of_token_or_native(token, get_contract_address()) {
        Ok(value) => value,
        Err(_) => return 4,
    };
    let mut result = Vec::with_capacity(56);
    for value in [
        version,
        u64::from(migration_locked()),
        escrow,
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

/// Tests expect `bb_pause`
#[no_mangle]
pub extern "C" fn bb_pause(caller_ptr: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let caller = match read_address(caller_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 200;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    if !require_admin(&caller) {
        reentrancy_exit();
        return 1;
    }
    storage_set(b"bb_paused", &[1u8]);
    log_info("BountyBoard paused");
    reentrancy_exit();
    0
}

/// Tests expect `bb_unpause`
#[no_mangle]
pub extern "C" fn bb_unpause(caller_ptr: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let caller = match read_address(caller_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 200;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    if !require_admin(&caller) {
        reentrancy_exit();
        return 1;
    }
    if !accounting_operational() || !runtime_configuration_valid() {
        reentrancy_exit();
        return 2;
    }
    let token = match reward_token_or_native() {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 2;
        }
    };
    let escrow = match checked_stored_u64(BB_ESCROW_LIABILITY_KEY) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 2;
        }
    };
    let fees = match checked_stored_u64(&platform_fee_key(token)) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 2;
        }
    };
    let total = match escrow.checked_add(fees) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 2;
        }
    };
    match balance_of_token_or_native(token, get_contract_address()) {
        Ok(custody) if custody >= total => {}
        Ok(_) | Err(_) => {
            reentrancy_exit();
            return 3;
        }
    }
    storage_set(b"bb_paused", &[0u8]);
    log_info("BountyBoard unpaused");
    reentrancy_exit();
    0
}

/// Get bounty platform stats [bounty_count(8), completed_count(8), reward_volume(8), cancel_count(8)]
#[no_mangle]
pub extern "C" fn get_platform_stats() -> u32 {
    let values = [
        checked_stored_u64(b"bounty_count"),
        checked_stored_u64(BB_COMPLETED_COUNT_KEY),
        checked_stored_u64(BB_REWARD_VOLUME_KEY),
        checked_stored_u64(BB_CANCEL_COUNT_KEY),
    ];
    if values.iter().any(Option::is_none) {
        return 2;
    }
    let mut buf = Vec::with_capacity(32);
    for value in values.into_iter().flatten() {
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
    use lichen_sdk::test_mock;

    fn setup() {
        test_mock::reset();
        storage_set(
            BB_ACCOUNTING_VERSION_KEY,
            &u64_to_bytes(ACCOUNTING_VERSION_V2),
        );
        storage_set(BB_ESCROW_LIABILITY_KEY, &u64_to_bytes(0));
        storage_set(BB_MIGRATION_LOCK_KEY, &[0]);
        storage_set(BB_PLATFORM_FEE_BPS_KEY, &u64_to_bytes(0));
        storage_set(BB_FEE_TREASURY_KEY, &[0xAD; 32]);
        storage_set(TOKEN_ADDRESS_KEY, &[0u8; 32]);
    }

    fn create_basic_bounty(creator: &[u8; 32], reward: u64) {
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        let title_hash = [0xAA; 32];
        test_mock::set_caller(*creator);
        let payment_value = if reward_token_or_native() == Some(Address([0u8; 32])) {
            reward
        } else {
            0
        };
        test_mock::set_value(payment_value);
        assert_eq!(
            create_bounty(creator.as_ptr(), title_hash.as_ptr(), reward, 1000),
            0
        );
    }

    fn submit_basic_work(worker: &[u8; 32]) {
        let proof_hash = [0xBB; 32];
        test_mock::set_caller(*worker);
        assert_eq!(submit_work(0, worker.as_ptr(), proof_hash.as_ptr()), 0);
    }

    fn submit_basic_work_result(worker: &[u8; 32], proof_hash: &[u8; 32]) -> u32 {
        test_mock::set_caller(*worker);
        submit_work(0, worker.as_ptr(), proof_hash.as_ptr())
    }

    fn assert_token_transfer_args(
        args: &[u8],
        expected_from: &[u8; 32],
        expected_to: &[u8; 32],
        expected_amount: u64,
    ) {
        assert_eq!(args.len(), 76);
        assert_eq!(
            &args[..4],
            &[lichen_sdk::crosscall::ABI_LAYOUT_MARKER, 32, 32, 8]
        );
        assert_eq!(&args[4..36], expected_from);
        assert_eq!(&args[36..68], expected_to);
        assert_eq!(bytes_to_u64(&args[68..76]), expected_amount);
    }

    #[test]
    fn test_create_bounty() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];

        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(creator);
        test_mock::set_value(500_000);
        let result = create_bounty(
            creator.as_ptr(),
            title_hash.as_ptr(),
            500_000, // reward
            1000,    // deadline at slot 1000
        );
        assert_eq!(result, 0);

        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 0); // bounty_id = 0

        let bk = bounty_key(0);
        let bounty = test_mock::get_storage(&bk).unwrap();
        assert_eq!(bounty.len(), BOUNTY_SIZE);
        assert_eq!(&bounty[0..32], &creator);
        assert_eq!(bytes_to_u64(&bounty[64..72]), 500_000);
        assert_eq!(bounty[80], BOUNTY_OPEN);
        assert_eq!(bounty[81], 0); // no submissions
    }

    #[test]
    fn test_submit_and_approve_work() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(creator);
        test_mock::set_value(500_000);
        create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1000);

        // Submit work
        let worker = [2u8; 32];
        let proof_hash = [0xBB; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(worker);
        let result = submit_work(0, worker.as_ptr(), proof_hash.as_ptr());
        assert_eq!(result, 0);

        // Check submission count
        let bk = bounty_key(0);
        let bounty = test_mock::get_storage(&bk).unwrap();
        assert_eq!(bounty[81], 1); // 1 submission

        // Verify submission stored
        let sk = submission_key(0, 0);
        let sub = test_mock::get_storage(&sk).unwrap();
        assert_eq!(sub.len(), SUBMISSION_SIZE);
        assert_eq!(&sub[0..32], &worker);

        // Approve
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(creator);
        let result = approve_work(creator.as_ptr(), 0, 0);
        assert_eq!(result, 0);

        let bounty = test_mock::get_storage(&bk).unwrap();
        assert_eq!(bounty[80], BOUNTY_COMPLETED);
        assert_eq!(bounty[90], 0); // approved submission idx
    }

    #[test]
    fn test_cancel_bounty() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(creator);
        test_mock::set_value(300_000);
        create_bounty(creator.as_ptr(), title_hash.as_ptr(), 300_000, 1000);

        let result = cancel_bounty(creator.as_ptr(), 0);
        assert_eq!(result, 0);

        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 300_000); // refund amount

        let bk = bounty_key(0);
        let bounty = test_mock::get_storage(&bk).unwrap();
        assert_eq!(bounty[80], BOUNTY_CANCELLED);

        // Non-creator can't cancel (creator check fires before status check)
        let other = [9u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(other);
        let result = cancel_bounty(other.as_ptr(), 0);
        assert_eq!(result, 3);
    }

    #[test]
    fn test_cancel_preserves_submitted_work_until_deadline() {
        setup();
        let creator = [1u8; 32];
        let worker = [2u8; 32];
        create_basic_bounty(&creator, 300_000);
        submit_basic_work(&worker);

        test_mock::set_caller(creator);
        assert_eq!(cancel_bounty(creator.as_ptr(), 0), 11);
        assert_eq!(
            test_mock::get_storage(&bounty_key(0)).unwrap()[80],
            BOUNTY_OPEN
        );
        assert_eq!(stored_u64(BB_ESCROW_LIABILITY_KEY), 300_000);

        test_mock::set_slot(1_001);
        assert_eq!(cancel_bounty(creator.as_ptr(), 0), 0);
        assert_eq!(
            test_mock::get_storage(&bounty_key(0)).unwrap()[80],
            BOUNTY_CANCELLED
        );
        assert_eq!(stored_u64(BB_ESCROW_LIABILITY_KEY), 0);
    }

    #[test]
    fn test_get_bounty() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 50);

        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(creator);
        test_mock::set_value(100_000);
        create_bounty(creator.as_ptr(), title_hash.as_ptr(), 100_000, 500);

        let result = get_bounty(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), BOUNTY_SIZE);

        // Not found
        let result = get_bounty(999);
        assert_eq!(result, 1);
    }

    #[test]
    fn test_identity_gate_blocks_create_bounty() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [1u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(admin);
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        let lichenid_addr = [0x42u8; 32];
        assert_eq!(
            set_lichenid_address(admin.as_ptr(), lichenid_addr.as_ptr()),
            0
        );
        assert_eq!(set_identity_gate(admin.as_ptr(), 100), 0);

        let creator = [2u8; 32];
        let title_hash = [0xAA; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(creator);
        test_mock::set_value(500_000);
        let result = create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1000);
        assert_eq!(result, 10);
    }

    #[test]
    fn test_identity_gate_blocks_submit_work() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        // Create a bounty first (no gate yet)
        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(creator);
        test_mock::set_value(500_000);
        create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1000);

        // Now configure gate
        let admin = [5u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(admin);
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        let lichenid_addr = [0x42u8; 32];
        assert_eq!(
            set_lichenid_address(admin.as_ptr(), lichenid_addr.as_ptr()),
            0
        );
        assert_eq!(set_identity_gate(admin.as_ptr(), 1), 0); // any reputation

        let worker = [2u8; 32];
        let proof_hash = [0xBB; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(worker);
        let result = submit_work(0, worker.as_ptr(), proof_hash.as_ptr());
        assert_eq!(result, 10);
    }

    #[test]
    fn test_identity_gate_allows_when_disabled() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(creator);
        test_mock::set_value(500_000);
        let result = create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1000);
        assert_eq!(result, 0);

        let worker = [2u8; 32];
        let proof_hash = [0xBB; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(worker);
        let result = submit_work(0, worker.as_ptr(), proof_hash.as_ptr());
        assert_eq!(result, 0);
    }

    #[test]
    fn test_set_identity_gate_admin_only() {
        setup();

        let admin = [1u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(admin);
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        assert_eq!(set_identity_admin(admin.as_ptr()), 1); // already set

        let other = [9u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(other);
        assert_eq!(set_identity_gate(other.as_ptr(), 100), 2);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(admin);
        assert_eq!(set_identity_gate(admin.as_ptr(), 100), 3);
        let lichenid = [0x42; 32];
        assert_eq!(set_lichenid_address(admin.as_ptr(), lichenid.as_ptr()), 0);
        assert_eq!(set_identity_gate(admin.as_ptr(), 100), 0);
    }

    // --- Token transfer integration ---

    #[test]
    fn test_set_token_address_success() {
        setup();
        let admin = [1u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());
        let token = [0xDD; 32];
        assert_eq!(set_token_address(admin.as_ptr(), token.as_ptr()), 0);
        let stored = test_mock::get_storage(TOKEN_ADDRESS_KEY).unwrap();
        assert_eq!(stored.as_slice(), &token);
    }

    #[test]
    fn test_set_token_address_not_admin() {
        setup();
        let admin = [1u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());
        let rando = [99u8; 32];
        let token = [0xDD; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(rando);
        assert_eq!(set_token_address(rando.as_ptr(), token.as_ptr()), 2);
    }

    #[test]
    fn test_set_token_address_no_admin_set() {
        setup();
        let caller = [1u8; 32];
        let token = [0xDD; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(caller);
        assert_eq!(set_token_address(caller.as_ptr(), token.as_ptr()), 1);
    }

    #[test]
    fn test_set_token_address_accepts_explicit_native_licn() {
        setup();
        let admin = [1u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());
        let zero = [0u8; 32];
        assert_eq!(set_token_address(admin.as_ptr(), zero.as_ptr()), 0);
        assert_eq!(reward_token_or_native(), Some(Address(zero)));
    }

    #[test]
    fn test_set_token_address_cannot_reconfigure() {
        setup();
        let admin = [1u8; 32];
        let first = [0xDD; 32];
        let second = [0xEE; 32];
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());

        assert_eq!(set_token_address(admin.as_ptr(), first.as_ptr()), 0);
        assert_eq!(set_token_address(admin.as_ptr(), second.as_ptr()), 4);

        let stored = test_mock::get_storage(TOKEN_ADDRESS_KEY).unwrap();
        assert_eq!(stored.as_slice(), &first);
    }

    #[test]
    fn test_set_lichenid_address_rejects_zero_and_reconfiguration() {
        setup();
        let admin = [1u8; 32];
        let other = [9u8; 32];
        let first = [0x42u8; 32];
        let second = [0x43u8; 32];
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());

        test_mock::set_caller(other);
        assert_eq!(set_lichenid_address(other.as_ptr(), first.as_ptr()), 2);

        test_mock::set_caller(admin);
        assert_eq!(set_lichenid_address(admin.as_ptr(), [0u8; 32].as_ptr()), 3);
        assert_eq!(set_lichenid_address(admin.as_ptr(), first.as_ptr()), 0);
        assert_eq!(set_lichenid_address(admin.as_ptr(), second.as_ptr()), 4);

        let stored = test_mock::get_storage(LICHENID_ADDR_KEY).unwrap();
        assert_eq!(stored.as_slice(), &first);
    }

    #[test]
    fn test_approve_work_with_token_transfer() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        let contract_addr = [0xCC; 32];
        test_mock::set_contract_address(contract_addr);

        // Configure token address
        let admin = [5u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());
        let token = [0xDD; 32];
        set_token_address(admin.as_ptr(), token.as_ptr());

        // Create bounty and submit work
        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(creator);
        test_mock::set_value(0);
        create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1000);

        let worker = [2u8; 32];
        let proof_hash = [0xBB; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(worker);
        submit_work(0, worker.as_ptr(), proof_hash.as_ptr());

        // Approve — default mock transfer succeeds and should use contract
        // self-custody as the source account.
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(creator);
        let result = approve_work(creator.as_ptr(), 0, 0);
        assert_eq!(result, 0);

        let bk = bounty_key(0);
        let bounty = test_mock::get_storage(&bk).unwrap();
        assert_eq!(bounty[80], BOUNTY_COMPLETED);

        let (target, function, args, value) =
            test_mock::get_last_cross_call().expect("approve_work should perform a token transfer");
        assert_eq!(target, token);
        assert_eq!(function, "transfer");
        assert_eq!(value, 0);
        assert_token_transfer_args(&args, &contract_addr, &worker, 500_000);
    }

    #[test]
    fn test_approve_work_without_token_configured() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        // No token address configured — approve still works (no transfer attempted)
        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(creator);
        test_mock::set_value(500_000);
        create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1000);

        let worker = [2u8; 32];
        let proof_hash = [0xBB; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(worker);
        submit_work(0, worker.as_ptr(), proof_hash.as_ptr());

        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(creator);
        let result = approve_work(creator.as_ptr(), 0, 0);
        assert_eq!(result, 0);

        let bk = bounty_key(0);
        let bounty = test_mock::get_storage(&bk).unwrap();
        assert_eq!(bounty[80], BOUNTY_COMPLETED);
    }

    // AUDIT-FIX P2: Security regression test
    #[test]
    fn test_create_bounty_when_paused() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        // Set up admin and pause the contract
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());
        bb_pause(admin.as_ptr());

        // Attempt to create bounty while paused → should fail
        let creator = [2u8; 32];
        let title_hash = [0xAA; 32];
        test_mock::set_caller(creator);
        let result = create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1000);
        assert_eq!(result, ERR_PAUSED);
    }

    // AUDIT-FIX P2: Security regression test
    #[test]
    fn test_submit_work_when_paused() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        // Create a bounty first (before pause)
        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(500_000);
        create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1000);

        // Set up admin and pause the contract
        let admin = [5u8; 32];
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());
        bb_pause(admin.as_ptr());

        // Attempt to submit work while paused → should fail
        let worker = [2u8; 32];
        let proof_hash = [0xBB; 32];
        test_mock::set_caller(worker);
        let result = submit_work(0, worker.as_ptr(), proof_hash.as_ptr());
        assert_eq!(result, ERR_PAUSED);
    }

    #[test]
    fn test_cancel_bounty_still_works_when_paused() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        let contract_addr = [0xCC; 32];
        test_mock::set_contract_address(contract_addr);

        let admin = [5u8; 32];
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());
        let token = [0xDD; 32];
        assert_eq!(set_token_address(admin.as_ptr(), token.as_ptr()), 0);

        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(0);
        assert_eq!(
            create_bounty(creator.as_ptr(), title_hash.as_ptr(), 300_000, 1000),
            0
        );

        test_mock::set_caller(admin);
        assert_eq!(bb_pause(admin.as_ptr()), 0);

        test_mock::set_caller(creator);
        assert_eq!(cancel_bounty(creator.as_ptr(), 0), 0);

        let bounty = test_mock::get_storage(&bounty_key(0)).unwrap();
        assert_eq!(bounty[80], BOUNTY_CANCELLED);

        let (target, function, args, value) = test_mock::get_last_cross_call()
            .expect("cancel_bounty should refund even while paused");
        assert_eq!(target, token);
        assert_eq!(function, "transfer");
        assert_eq!(value, 0);
        assert_token_transfer_args(&args, &contract_addr, &creator, 300_000);
    }

    // ========================================================================
    // G22-01 FINANCIAL WIRING TESTS
    // ========================================================================

    #[test]
    fn test_create_bounty_insufficient_value() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(499_999); // 1 short of 500_000
        let result = create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1000);
        assert_eq!(
            result, 11,
            "Should reject insufficient value for bounty reward"
        );
    }

    #[test]
    fn test_create_bounty_exact_value() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(500_000); // exact amount
        let result = create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1000);
        assert_eq!(result, 0, "Exact value should be accepted");
    }

    #[test]
    fn test_create_bounty_rejects_native_overpayment_and_token_attached_value() {
        setup();
        test_mock::set_slot(100);
        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(500_001);
        assert_eq!(
            create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1_000),
            11
        );
        assert_eq!(checked_stored_u64(b"bounty_count"), Some(0));
        assert_eq!(checked_stored_u64(BB_ESCROW_LIABILITY_KEY), Some(0));

        let admin = [5u8; 32];
        let token = [0xDD; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        assert_eq!(set_token_address(admin.as_ptr(), token.as_ptr()), 0);
        test_mock::set_caller(creator);
        test_mock::set_value(1);
        assert_eq!(
            create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1_000),
            11
        );
        assert_eq!(checked_stored_u64(b"bounty_count"), Some(0));
        assert_eq!(checked_stored_u64(BB_ESCROW_LIABILITY_KEY), Some(0));
    }

    #[test]
    fn test_cancel_bounty_uses_correct_token_key() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        let contract_addr = [0xCC; 32];
        test_mock::set_contract_address(contract_addr);

        // Configure token address via set_token_address (writes TOKEN_ADDRESS_KEY)
        let admin = [5u8; 32];
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());
        let token = [0xDD; 32];
        set_token_address(admin.as_ptr(), token.as_ptr());

        // Create bounty
        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(0);
        create_bounty(creator.as_ptr(), title_hash.as_ptr(), 300_000, 1000);

        // Cancel — should use TOKEN_ADDRESS_KEY (not the old wrong key)
        test_mock::set_caller(creator);
        let result = cancel_bounty(creator.as_ptr(), 0);
        assert_eq!(result, 0, "Cancel should refund successfully");

        let bounty = test_mock::get_storage(&bounty_key(0)).unwrap();
        assert_eq!(bounty[80], BOUNTY_CANCELLED);

        let (target, function, args, value) = test_mock::get_last_cross_call()
            .expect("cancel_bounty should perform a token transfer");
        assert_eq!(target, token);
        assert_eq!(function, "transfer");
        assert_eq!(value, 0);
        assert_token_transfer_args(&args, &contract_addr, &creator, 300_000);
    }

    #[test]
    fn test_approve_uses_self_custody() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        let contract_addr = [0xCC; 32];
        test_mock::set_contract_address(contract_addr);

        let admin = [5u8; 32];
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());
        let token = [0xDD; 32];
        set_token_address(admin.as_ptr(), token.as_ptr());

        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(0);
        create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1000);

        let worker = [2u8; 32];
        let proof_hash = [0xBB; 32];
        test_mock::set_caller(worker);
        submit_work(0, worker.as_ptr(), proof_hash.as_ptr());

        // Approve — verify self-custody transfer uses the contract address as
        // the source account.
        test_mock::set_caller(creator);
        let result = approve_work(creator.as_ptr(), 0, 0);
        assert_eq!(result, 0, "Approve should succeed with token transfer");

        let (_, function, args, value) =
            test_mock::get_last_cross_call().expect("approve_work should perform a token transfer");
        assert_eq!(function, "transfer");
        assert_eq!(value, 0);
        assert_token_transfer_args(&args, &contract_addr, &worker, 500_000);
    }

    #[test]
    fn test_cancel_without_token_succeeds() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        // No token configured — cancel should still succeed (no transfer attempted)
        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(300_000);
        create_bounty(creator.as_ptr(), title_hash.as_ptr(), 300_000, 1000);

        test_mock::set_caller(creator);
        let result = cancel_bounty(creator.as_ptr(), 0);
        assert_eq!(result, 0, "Cancel without token should succeed");

        let bk = bounty_key(0);
        let bounty = test_mock::get_storage(&bk).unwrap();
        assert_eq!(bounty[80], BOUNTY_CANCELLED);
    }

    #[test]
    fn test_create_bounty_count_overflow_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        storage_set(b"bounty_count", &u64_to_bytes(u64::MAX));

        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(500_000);

        assert_eq!(
            create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1000),
            12
        );
        assert_eq!(stored_u64(b"bounty_count"), u64::MAX);
    }

    #[test]
    fn test_approve_without_token_uses_native_transfer() {
        setup();
        let contract_addr = [0xCC; 32];
        test_mock::set_contract_address(contract_addr);
        let creator = [1u8; 32];
        let worker = [2u8; 32];

        create_basic_bounty(&creator, 500_000);
        submit_basic_work(&worker);

        test_mock::set_caller(creator);
        assert_eq!(approve_work(creator.as_ptr(), 0, 0), 0);

        let (target, function, args, value) =
            test_mock::get_last_cross_call().expect("native payout should transfer");
        assert_eq!(target, [0u8; 32]);
        assert_eq!(function, "transfer");
        assert_eq!(value, 0);
        assert_eq!(&args[0..32], &worker);
        assert_eq!(bytes_to_u64(&args[32..40]), 500_000);
    }

    #[test]
    fn test_cancel_without_token_uses_native_refund() {
        setup();
        let creator = [1u8; 32];
        create_basic_bounty(&creator, 300_000);

        test_mock::set_caller(creator);
        assert_eq!(cancel_bounty(creator.as_ptr(), 0), 0);

        let (target, function, args, value) =
            test_mock::get_last_cross_call().expect("native refund should transfer");
        assert_eq!(target, [0u8; 32]);
        assert_eq!(function, "transfer");
        assert_eq!(value, 0);
        assert_eq!(&args[0..32], &creator);
        assert_eq!(bytes_to_u64(&args[32..40]), 300_000);
    }

    #[test]
    fn test_malformed_token_config_fails_closed() {
        setup();
        let creator = [1u8; 32];
        let worker = [2u8; 32];
        create_basic_bounty(&creator, 500_000);
        submit_basic_work(&worker);
        storage_set(&bounty_token_key(0), &[1u8, 2u8]);

        test_mock::set_caller(creator);
        assert_eq!(approve_work(creator.as_ptr(), 0, 0), 9);
        let bounty = test_mock::get_storage(&bounty_key(0)).unwrap();
        assert_eq!(bounty[80], BOUNTY_OPEN);

        test_mock::set_slot(1_001);
        assert_eq!(cancel_bounty(creator.as_ptr(), 0), 9);
        let bounty = test_mock::get_storage(&bounty_key(0)).unwrap();
        assert_eq!(bounty[80], BOUNTY_OPEN);
    }

    #[test]
    fn test_platform_fee_cap_and_admin_required() {
        setup();
        let zero = [0u8; 32];
        test_mock::set_caller(zero);
        assert_eq!(set_platform_fee(zero.as_ptr(), 1), 1);

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        assert_eq!(set_platform_fee(admin.as_ptr(), 1001), 2);
        assert_eq!(set_platform_fee(admin.as_ptr(), 1000), 0);
    }

    #[test]
    fn test_two_step_admin_rotation_requires_pending_authority_acceptance() {
        setup();
        let admin = [0xAD; 32];
        let next_admin = [0xBC; 32];
        let outsider = [0xEF; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        assert_eq!(propose_admin(admin.as_ptr(), [0u8; 32].as_ptr()), 2);
        assert_eq!(propose_admin(admin.as_ptr(), next_admin.as_ptr()), 0);
        assert_eq!(propose_admin(admin.as_ptr(), next_admin.as_ptr()), 0);

        assert_eq!(cancel_admin_proposal(admin.as_ptr()), 0);
        test_mock::set_caller(next_admin);
        assert_eq!(accept_admin(next_admin.as_ptr()), 2);
        test_mock::set_caller(admin);
        assert_eq!(cancel_admin_proposal(admin.as_ptr()), 0);
        assert_eq!(propose_admin(admin.as_ptr(), next_admin.as_ptr()), 0);

        assert_eq!(get_admin_transition(), 0);
        let transition = test_mock::get_return_data();
        assert_eq!(transition.len(), 64);
        assert_eq!(&transition[..32], &admin);
        assert_eq!(&transition[32..], &next_admin);

        test_mock::set_caller(outsider);
        assert_eq!(accept_admin(outsider.as_ptr()), 1);
        test_mock::set_caller(next_admin);
        assert_eq!(accept_admin(next_admin.as_ptr()), 0);
        assert_eq!(accept_admin(next_admin.as_ptr()), 2);
        assert_eq!(get_admin_transition(), 0);
        let transition = test_mock::get_return_data();
        assert_eq!(&transition[..32], &next_admin);
        assert_eq!(&transition[32..], &[0u8; 32]);

        test_mock::set_caller(admin);
        assert_eq!(set_platform_fee(admin.as_ptr(), 100), 1);
        test_mock::set_caller(next_admin);
        assert_eq!(set_platform_fee(next_admin.as_ptr(), 100), 0);
    }

    #[test]
    fn test_admin_transition_malformed_state_fails_closed() {
        setup();
        let admin = [0xAD; 32];
        let next_admin = [0xBC; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        storage_set(BB_PENDING_ADMIN_KEY, &[1]);
        assert_eq!(get_admin_transition(), 2);
        assert_eq!(propose_admin(admin.as_ptr(), next_admin.as_ptr()), 3);
        assert_eq!(cancel_admin_proposal(admin.as_ptr()), 2);
        test_mock::set_caller(next_admin);
        assert_eq!(accept_admin(next_admin.as_ptr()), 3);
        assert!(require_admin(&admin));
    }

    #[test]
    fn test_read_views_distinguish_missing_from_malformed_rows() {
        setup();
        assert_eq!(get_submission(9, 0), 1);
        assert_eq!(get_bounty_terms(9), 1);
        assert_eq!(get_bounty_migration_record(9), 1);

        storage_set(&submission_key(9, 0), &[1]);
        storage_set(&bounty_key(9), &[1]);
        assert_eq!(get_submission(9, 0), 2);
        assert_eq!(get_bounty_terms(9), 4);
        assert_eq!(get_bounty_migration_record(9), 4);

        create_basic_bounty(&[1u8; 32], 100);
        let mut inconsistent_bounty = storage_get(&bounty_key(0)).unwrap();
        inconsistent_bounty[80] = BOUNTY_COMPLETED;
        storage_set(&bounty_key(0), &inconsistent_bounty);
        assert_eq!(get_bounty(0), 2);
        assert_eq!(get_bounty_terms(0), 4);
        assert_eq!(get_bounty_migration_record(0), 4);

        storage_set(&submission_key(0, 0), &[0u8; SUBMISSION_SIZE]);
        assert_eq!(get_submission(0, 0), 2);
    }

    #[test]
    fn test_approve_work_false_transfer_preserves_bounty_and_stats() {
        setup();
        let creator = [1u8; 32];
        let worker = [2u8; 32];
        create_basic_bounty(&creator, 500_000);
        submit_basic_work(&worker);

        let before = test_mock::get_storage(&bounty_key(0)).unwrap();
        test_mock::set_caller(creator);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));

        assert_eq!(approve_work(creator.as_ptr(), 0, 0), 8);

        let after = test_mock::get_storage(&bounty_key(0)).unwrap();
        assert_eq!(after, before);
        assert_eq!(stored_u64(BB_COMPLETED_COUNT_KEY), 0);
        assert_eq!(stored_u64(BB_REWARD_VOLUME_KEY), 0);
    }

    #[test]
    fn test_cancel_bounty_false_refund_preserves_bounty_and_stats() {
        setup();
        let creator = [1u8; 32];
        create_basic_bounty(&creator, 300_000);

        let before = test_mock::get_storage(&bounty_key(0)).unwrap();
        test_mock::set_caller(creator);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));

        assert_eq!(cancel_bounty(creator.as_ptr(), 0), 8);

        let after = test_mock::get_storage(&bounty_key(0)).unwrap();
        assert_eq!(after, before);
        assert_eq!(stored_u64(BB_CANCEL_COUNT_KEY), 0);
    }

    #[test]
    fn test_platform_fee_is_snapshotted_realized_and_worker_gets_net() {
        setup();
        let admin = [0xAD; 32];
        let token = [0xDD; 32];
        let creator = [1u8; 32];
        let worker = [2u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        assert_eq!(set_token_address(admin.as_ptr(), token.as_ptr()), 0);
        assert_eq!(set_platform_fee(admin.as_ptr(), 500), 0);
        create_basic_bounty(&creator, 10_000);
        submit_basic_work(&worker);

        test_mock::set_caller(creator);
        assert_eq!(approve_work(creator.as_ptr(), 0, 0), 0);
        assert_eq!(stored_u64(&platform_fee_key(Address(token))), 500);
        let (target, function, args, _) = test_mock::get_last_cross_call().unwrap();
        assert_eq!(target, token);
        assert_eq!(function, "transfer");
        assert_token_transfer_args(&args, &[0u8; 32], &worker, 9_500);
        let settlement = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&settlement[..8]), 9_500);
        assert_eq!(bytes_to_u64(&settlement[8..]), 500);
    }

    #[test]
    fn test_fee_change_is_prospective_and_cancel_refunds_gross() {
        setup();
        let admin = [0xAD; 32];
        let creator = [1u8; 32];
        let worker = [2u8; 32];
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());
        create_basic_bounty(&creator, 10_000);
        test_mock::set_caller(admin);
        assert_eq!(set_platform_fee(admin.as_ptr(), 1_000), 0);
        submit_basic_work(&worker);
        test_mock::set_caller(creator);
        assert_eq!(approve_work(creator.as_ptr(), 0, 0), 0);
        assert_eq!(stored_u64(&platform_fee_key(Address([0u8; 32]))), 0);

        create_basic_bounty(&creator, 10_000);
        test_mock::set_caller(creator);
        assert_eq!(cancel_bounty(creator.as_ptr(), 1), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 10_000);
        assert_eq!(stored_u64(&platform_fee_key(Address([0u8; 32]))), 0);
    }

    #[test]
    fn test_failed_worker_payment_does_not_realize_fee() {
        setup();
        let admin = [0xAD; 32];
        let creator = [1u8; 32];
        let worker = [2u8; 32];
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());
        set_platform_fee(admin.as_ptr(), 500);
        create_basic_bounty(&creator, 10_000);
        submit_basic_work(&worker);
        test_mock::set_caller(creator);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(approve_work(creator.as_ptr(), 0, 0), 8);
        assert_eq!(stored_u64(&platform_fee_key(Address([0u8; 32]))), 0);
        assert_eq!(
            test_mock::get_storage(&bounty_key(0)).unwrap()[80],
            BOUNTY_OPEN
        );
    }

    #[test]
    fn test_fee_withdrawal_is_treasury_bound_and_retry_safe() {
        setup();
        let admin = [0xAD; 32];
        let treasury = [0x77; 32];
        let token = [0xDD; 32];
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());
        assert_eq!(set_token_address(admin.as_ptr(), token.as_ptr()), 0);
        assert_eq!(set_fee_treasury(admin.as_ptr(), treasury.as_ptr()), 0);
        let key = platform_fee_key(Address(token));
        storage_set(&key, &u64_to_bytes(500));

        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(
            withdraw_platform_fees(admin.as_ptr(), token.as_ptr(), 300),
            5
        );
        assert_eq!(stored_u64(&key), 500);
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        assert_eq!(
            withdraw_platform_fees(admin.as_ptr(), token.as_ptr(), 300),
            0
        );
        assert_eq!(stored_u64(&key), 200);
        let (_, _, args, _) = test_mock::get_last_cross_call().unwrap();
        assert_eq!(&args[36..68], &treasury);
        assert_eq!(bytes_to_u64(&args[68..]), 300);
    }

    #[test]
    fn test_reward_asset_is_snapshotted_per_bounty() {
        setup();
        let admin = [0xAD; 32];
        let token = [0xDD; 32];
        let replacement = [0xEE; 32];
        let creator = [1u8; 32];
        let worker = [2u8; 32];
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());
        set_token_address(admin.as_ptr(), token.as_ptr());
        create_basic_bounty(&creator, 10_000);
        storage_set(TOKEN_ADDRESS_KEY, &replacement);
        submit_basic_work(&worker);
        test_mock::set_caller(creator);
        assert_eq!(approve_work(creator.as_ptr(), 0, 0), 0);
        assert_eq!(test_mock::get_last_cross_call().unwrap().0, token);
    }

    #[test]
    fn test_legacy_token_migration_is_one_time_and_fee_free() {
        setup();
        let admin = [0xAD; 32];
        let creator = [1u8; 32];
        let token = [0xDD; 32];
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());
        assert_eq!(set_token_address(admin.as_ptr(), token.as_ptr()), 0);
        storage_set(BB_ACCOUNTING_VERSION_KEY, &u64_to_bytes(0));
        storage_set(BB_MIGRATION_LOCK_KEY, &[1]);
        storage_set(BB_MIGRATION_CURSOR_KEY, &u64_to_bytes(7));
        let bounty = encode_bounty(BountyEncoding {
            creator: &creator,
            title_hash: &[0xAA; 32],
            reward_amount: 10_000,
            deadline_slot: 1_000,
            status: BOUNTY_OPEN,
            submission_count: 0,
            created_slot: 100,
            approved_idx: 0xFF,
        });
        storage_set(&bounty_key(7), &bounty);
        assert_eq!(migrate_bounty_token(admin.as_ptr(), 7, token.as_ptr()), 0);
        assert_eq!(migrate_bounty_token(admin.as_ptr(), 7, token.as_ptr()), 5);
        assert_eq!(bounty_reward_token(7), Some(Address(token)));
        assert_eq!(bounty_platform_fee_bps(7), Some(0));
    }

    #[test]
    fn test_submission_is_unique_queryable_and_updatable() {
        setup();
        let creator = [1u8; 32];
        let worker = [2u8; 32];
        create_basic_bounty(&creator, 10_000);
        submit_basic_work(&worker);
        assert_eq!(submit_basic_work_result(&worker, &[0xCC; 32]), 8);
        assert_eq!(get_submission(0, 0), 0);
        assert_eq!(&test_mock::get_return_data()[0..32], &worker);

        test_mock::set_slot(200);
        test_mock::set_caller(worker);
        assert_eq!(update_work(0, 0, worker.as_ptr(), [0xCC; 32].as_ptr()), 0);
        assert_eq!(get_submission(0, 0), 0);
        let submission = test_mock::get_return_data();
        assert_eq!(&submission[32..64], &[0xCC; 32]);
        assert_eq!(bytes_to_u64(&submission[64..72]), 200);
    }

    #[test]
    fn test_submission_rejects_zero_proof_and_creator_self_award() {
        setup();
        let creator = [1u8; 32];
        let worker = [2u8; 32];
        create_basic_bounty(&creator, 10_000);
        assert_eq!(submit_basic_work_result(&worker, &[0u8; 32]), 6);
        assert_eq!(submit_basic_work_result(&creator, &[0xBB; 32]), 7);
    }

    #[test]
    fn test_identity_gate_without_contract_configuration_fails_closed() {
        setup();
        let admin = [0xAD; 32];
        let creator = [1u8; 32];
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());
        assert_eq!(set_identity_gate(admin.as_ptr(), 1), 3);
        storage_set(LICHENID_MIN_REP_KEY, &u64_to_bytes(1));
        test_mock::set_slot(100);
        test_mock::set_caller(creator);
        assert_eq!(
            create_bounty(creator.as_ptr(), [0xAA; 32].as_ptr(), 10_000, 1_000),
            ERR_PAUSED
        );
    }

    #[test]
    fn test_bounty_terms_expose_net_reward_and_fee() {
        setup();
        let admin = [0xAD; 32];
        let creator = [1u8; 32];
        test_mock::set_caller(admin);
        set_identity_admin(admin.as_ptr());
        set_platform_fee(admin.as_ptr(), 500);
        create_basic_bounty(&creator, 10_000);
        assert_eq!(get_bounty_terms(0), 0);
        let terms = test_mock::get_return_data();
        assert_eq!(terms.len(), 64);
        assert_eq!(&terms[..32], &[0u8; 32]);
        assert_eq!(bytes_to_u64(&terms[32..40]), 500);
        assert_eq!(bytes_to_u64(&terms[40..48]), 10_000);
        assert_eq!(bytes_to_u64(&terms[48..56]), 9_500);
        assert_eq!(bytes_to_u64(&terms[56..64]), 500);
    }

    #[test]
    fn test_migration_record_seals_legacy_snapshot_presence_exactly() {
        setup();
        let creator = [1u8; 32];
        create_basic_bounty(&creator, 10_000);
        lichen_sdk::storage::remove(&bounty_token_key(0));
        lichen_sdk::storage::remove(&bounty_fee_bps_key(0));

        assert_eq!(get_bounty_migration_record(0), 0);
        let legacy = test_mock::get_return_data();
        assert_eq!(legacy.len(), 147);
        assert_eq!(bytes_to_u64(&legacy[91..99]), 0);
        assert_eq!(&legacy[99..131], &[0u8; 32]);
        assert_eq!(bytes_to_u64(&legacy[131..139]), 0);
        assert_eq!(bytes_to_u64(&legacy[139..147]), 0);

        storage_set(&bounty_token_key(0), &[7u8; 32]);
        storage_set(&bounty_fee_bps_key(0), &u64_to_bytes(250));
        assert_eq!(get_bounty_migration_record(0), 0);
        let snapshotted = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&snapshotted[91..99]), 1);
        assert_eq!(&snapshotted[99..131], &[7u8; 32]);
        assert_eq!(bytes_to_u64(&snapshotted[131..139]), 1);
        assert_eq!(bytes_to_u64(&snapshotted[139..147]), 250);

        storage_set(&bounty_fee_bps_key(0), &[1]);
        assert_eq!(get_bounty_migration_record(0), 3);
    }

    #[test]
    fn test_stats_counter_overflow_fails_before_settlement() {
        setup();
        let creator = [1u8; 32];
        let worker = [2u8; 32];
        create_basic_bounty(&creator, 500_000);
        submit_basic_work(&worker);
        storage_set(BB_COMPLETED_COUNT_KEY, &u64_to_bytes(u64::MAX));
        storage_set(BB_REWARD_VOLUME_KEY, &u64_to_bytes(u64::MAX - 1));

        test_mock::set_caller(creator);
        assert_eq!(approve_work(creator.as_ptr(), 0, 0), 15);

        assert_eq!(stored_u64(BB_COMPLETED_COUNT_KEY), u64::MAX);
        assert_eq!(stored_u64(BB_REWARD_VOLUME_KEY), u64::MAX - 1);
        assert_eq!(
            test_mock::get_storage(&bounty_key(0)).unwrap()[80],
            BOUNTY_OPEN
        );
    }

    #[test]
    fn test_accounting_v2_migration_is_source_bound_resumable_and_solvent() {
        setup();
        let admin = [0xAD; 32];
        let creator = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        create_basic_bounty(&creator, 100);
        create_basic_bounty(&creator, 200);

        let mut completed = storage_get(&bounty_key(1)).unwrap();
        completed[80] = BOUNTY_COMPLETED;
        completed[81] = 1;
        completed[90] = 0;
        storage_set(&bounty_key(1), &completed);
        storage_set(
            &submission_key(1, 0),
            &encode_submission(&[2u8; 32], &[0xBB; 32], 100),
        );
        storage_set(&platform_fee_key(Address([0u8; 32])), &u64_to_bytes(10));
        lichen_sdk::storage::remove(BB_ACCOUNTING_VERSION_KEY);
        lichen_sdk::storage::remove(BB_ESCROW_LIABILITY_KEY);

        test_mock::set_caller(admin);
        assert_eq!(begin_accounting_v2_migration(admin.as_ptr(), 1), 3);
        assert_eq!(begin_accounting_v2_migration(admin.as_ptr(), 2), 0);
        assert!(is_bb_paused());
        assert_eq!(begin_accounting_v2_migration(admin.as_ptr(), 2), 0);
        test_mock::set_caller(creator);
        assert_eq!(
            create_bounty(creator.as_ptr(), [1u8; 32].as_ptr(), 1, 200),
            ERR_PAUSED
        );
        assert_eq!(migrate_accounting_v2_bounty(1), 2);
        assert_eq!(migrate_accounting_v2_bounty(0), 0);
        assert_eq!(migrate_accounting_v2_bounty(0), 2);
        assert_eq!(migrate_accounting_v2_bounty(1), 0);

        assert_eq!(get_accounting_migration_status(), 0);
        let status: Vec<u64> = test_mock::get_return_data()
            .chunks_exact(8)
            .map(bytes_to_u64)
            .collect();
        assert_eq!(status.as_slice(), &[2, 2, 100, 0, 1]);
        storage_set(BB_MIGRATION_CURSOR_KEY, &[1]);
        assert_eq!(get_accounting_migration_status(), 2);
        storage_set(BB_MIGRATION_CURSOR_KEY, &u64_to_bytes(2));

        test_mock::set_caller(admin);
        assert_eq!(
            complete_accounting_v2_migration(admin.as_ptr(), 100, 10, 111),
            5
        );
        test_mock::set_cross_call_response(Some(u64_to_bytes(109).to_vec()));
        assert_eq!(
            complete_accounting_v2_migration(admin.as_ptr(), 100, 10, 110),
            9
        );
        assert!(migration_locked());
        test_mock::set_cross_call_response(Some(u64_to_bytes(110).to_vec()));
        assert_eq!(
            complete_accounting_v2_migration(admin.as_ptr(), 100, 10, 110),
            0
        );
        assert!(accounting_operational());
        assert!(is_bb_paused());

        test_mock::set_cross_call_response(Some(u64_to_bytes(110).to_vec()));
        assert_eq!(get_accounting_health(), 0);
        let health: Vec<u64> = test_mock::get_return_data()
            .chunks_exact(8)
            .map(bytes_to_u64)
            .collect();
        assert_eq!(health.as_slice(), &[2, 0, 100, 10, 110, 110, 1]);
        test_mock::set_caller(admin);
        assert_eq!(bb_unpause(admin.as_ptr()), 0);
    }

    #[test]
    fn test_accounting_v2_zero_frontier_is_explicit_and_solvency_gated() {
        setup();
        let admin = [0xAD; 32];
        test_mock::set_caller(admin);
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        lichen_sdk::storage::remove(BB_ACCOUNTING_VERSION_KEY);
        lichen_sdk::storage::remove(BB_ESCROW_LIABILITY_KEY);
        assert_eq!(begin_accounting_v2_migration(admin.as_ptr(), 0), 0);
        test_mock::set_cross_call_response(Some(u64_to_bytes(0).to_vec()));
        assert_eq!(complete_accounting_v2_migration(admin.as_ptr(), 0, 0, 0), 0);
        assert!(accounting_operational());
        assert!(is_bb_paused());
    }

    #[test]
    fn test_malformed_control_and_accounting_state_fails_closed_before_value() {
        setup();
        let creator = [1u8; 32];
        let title = [0xAA; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(creator);
        test_mock::set_value(100);

        storage_set(b"bounty_count", &[1]);
        assert_eq!(get_bounty_count_exact(), 2);
        assert_eq!(
            create_bounty(creator.as_ptr(), title.as_ptr(), 100, 200),
            12
        );
        assert!(test_mock::get_storage(&bounty_key(0)).is_none());
        lichen_sdk::storage::remove(b"bounty_count");

        storage_set(b"bb_paused", &[2]);
        assert_eq!(
            create_bounty(creator.as_ptr(), title.as_ptr(), 100, 200),
            ERR_PAUSED
        );
        storage_set(b"bb_paused", &[0]);
        storage_set(BB_PLATFORM_FEE_BPS_KEY, &[1]);
        assert_eq!(
            create_bounty(creator.as_ptr(), title.as_ptr(), 100, 200),
            ERR_PAUSED
        );
        storage_set(BB_PLATFORM_FEE_BPS_KEY, &u64_to_bytes(0));
        storage_set(TOKEN_ADDRESS_KEY, &[1]);
        assert_eq!(
            create_bounty(creator.as_ptr(), title.as_ptr(), 100, 200),
            ERR_PAUSED
        );
        storage_set(TOKEN_ADDRESS_KEY, &[0u8; 32]);
        storage_set(LICHENID_MIN_REP_KEY, &[1]);
        assert_eq!(
            create_bounty(creator.as_ptr(), title.as_ptr(), 100, 200),
            ERR_PAUSED
        );
        storage_set(LICHENID_MIN_REP_KEY, &u64_to_bytes(0));
        lichen_sdk::storage::remove(BB_FEE_TREASURY_KEY);
        assert_eq!(
            create_bounty(creator.as_ptr(), title.as_ptr(), 100, 200),
            ERR_PAUSED
        );
        storage_set(BB_FEE_TREASURY_KEY, &[0xAD; 32]);
        storage_set(BB_ESCROW_LIABILITY_KEY, &[1]);
        assert_eq!(
            create_bounty(creator.as_ptr(), title.as_ptr(), 100, 200),
            ERR_PAUSED
        );
        assert!(test_mock::get_storage(&bounty_key(0)).is_none());
    }

    #[test]
    fn test_exact_bounty_count_distinguishes_zero_from_malformed_state() {
        setup();
        assert_eq!(get_bounty_count_exact(), 0);
        assert_eq!(test_mock::get_return_data(), u64_to_bytes(0));

        storage_set(b"bounty_count", &u64_to_bytes(7));
        assert_eq!(get_bounty_count_exact(), 0);
        assert_eq!(test_mock::get_return_data(), u64_to_bytes(7));

        storage_set(b"bounty_count", &[7]);
        assert_eq!(get_bounty_count_exact(), 2);
    }

    #[test]
    fn test_cancel_counter_overflow_preserves_open_bounty_and_escrow() {
        setup();
        let creator = [1u8; 32];
        create_basic_bounty(&creator, 100);
        storage_set(BB_CANCEL_COUNT_KEY, &u64_to_bytes(u64::MAX));
        test_mock::set_caller(creator);
        assert_eq!(cancel_bounty(creator.as_ptr(), 0), 10);
        assert_eq!(
            test_mock::get_storage(&bounty_key(0)).unwrap()[80],
            BOUNTY_OPEN
        );
        assert_eq!(checked_stored_u64(BB_ESCROW_LIABILITY_KEY), Some(100));
    }
}
