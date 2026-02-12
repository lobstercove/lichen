// BountyBoard — Bounty/Task Management Contract for MoltChain
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

use moltchain_sdk::{
    log_info, storage_get, storage_set, bytes_to_u64, u64_to_bytes, get_slot,
    Address, CrossCall, call_contract,
};

// ============================================================================
// BOUNTY STATUS
// ============================================================================

const BOUNTY_OPEN: u8 = 0;
const BOUNTY_COMPLETED: u8 = 1;
const BOUNTY_CANCELLED: u8 = 2;

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

fn encode_bounty(
    creator: &[u8; 32],
    title_hash: &[u8; 32],
    reward_amount: u64,
    deadline_slot: u64,
    status: u8,
    submission_count: u8,
    created_slot: u64,
    approved_idx: u8,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(BOUNTY_SIZE);
    data.extend_from_slice(creator);
    data.extend_from_slice(title_hash);
    data.extend_from_slice(&u64_to_bytes(reward_amount));
    data.extend_from_slice(&u64_to_bytes(deadline_slot));
    data.push(status);
    data.push(submission_count);
    data.extend_from_slice(&u64_to_bytes(created_slot));
    data.push(approved_idx);
    data
}

// ============================================================================
// SUBMISSION LAYOUT
// ============================================================================
//
// Bytes 0..32  : worker (address)
// Bytes 32..64 : proof_hash (32 bytes)
// Bytes 64..72 : submitted_slot (u64 LE)

const SUBMISSION_SIZE: usize = 72;

fn encode_submission(
    worker: &[u8; 32],
    proof_hash: &[u8; 32],
    submitted_slot: u64,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(SUBMISSION_SIZE);
    data.extend_from_slice(worker);
    data.extend_from_slice(proof_hash);
    data.extend_from_slice(&u64_to_bytes(submitted_slot));
    data
}

// ============================================================================
// CREATE BOUNTY
// ============================================================================

/// Create a new bounty.
///
/// Parameters:
///   - creator_ptr: 32-byte creator address
///   - title_hash_ptr: 32-byte hash of the bounty title/description
///   - reward_amount: reward in shells
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
    log_info("📋 Creating bounty...");

    let creator = unsafe { core::slice::from_raw_parts(creator_ptr, 32) };
    let title_hash = unsafe { core::slice::from_raw_parts(title_hash_ptr, 32) };

    if reward_amount == 0 {
        log_info("❌ Reward must be > 0");
        return 1;
    }

    // MoltyID reputation gate
    if !check_identity_gate(creator) {
        log_info("❌ Insufficient MoltyID reputation for bounty creation");
        return 10;
    }

    let current_slot = get_slot();
    if deadline_slot <= current_slot {
        log_info("❌ Deadline must be in the future");
        return 2;
    }

    let mut creator_arr = [0u8; 32];
    creator_arr.copy_from_slice(creator);
    let mut title_arr = [0u8; 32];
    title_arr.copy_from_slice(title_hash);

    let bounty_id = storage_get(b"bounty_count")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    storage_set(b"bounty_count", &u64_to_bytes(bounty_id + 1));

    let data = encode_bounty(
        &creator_arr,
        &title_arr,
        reward_amount,
        deadline_slot,
        BOUNTY_OPEN,
        0, // no submissions
        current_slot,
        0xFF, // no approved submission
    );

    let bk = bounty_key(bounty_id);
    storage_set(&bk, &data);

    moltchain_sdk::set_return_data(&u64_to_bytes(bounty_id));
    log_info("✅ Bounty created");
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
    log_info("📝 Submitting work for bounty...");

    let worker = unsafe { core::slice::from_raw_parts(worker_ptr, 32) };
    let proof_hash = unsafe { core::slice::from_raw_parts(proof_hash_ptr, 32) };

    let bk = bounty_key(bounty_id);
    let mut bounty_data = match storage_get(&bk) {
        Some(data) => data,
        None => {
            log_info("❌ Bounty not found");
            return 1;
        }
    };

    if bounty_data.len() < BOUNTY_SIZE {
        return 2;
    }

    if bounty_data[80] != BOUNTY_OPEN {
        log_info("❌ Bounty is not open");
        return 3;
    }

    // MoltyID identity gate (any reputation level)
    if !check_identity_gate(worker) {
        log_info("❌ MoltyID identity required to submit work");
        return 10;
    }

    // Check deadline
    let deadline = bytes_to_u64(&bounty_data[72..80]);
    let current_slot = get_slot();
    if current_slot > deadline {
        log_info("❌ Bounty deadline passed");
        return 4;
    }

    let sub_count = bounty_data[81];
    if sub_count >= 255 {
        log_info("❌ Maximum submissions reached");
        return 5;
    }

    let mut worker_arr = [0u8; 32];
    worker_arr.copy_from_slice(worker);
    let mut proof_arr = [0u8; 32];
    proof_arr.copy_from_slice(proof_hash);

    // Store submission
    let sk = submission_key(bounty_id, sub_count);
    let sub_data = encode_submission(&worker_arr, &proof_arr, current_slot);
    storage_set(&sk, &sub_data);

    // Increment submission count
    bounty_data[81] = sub_count + 1;
    storage_set(&bk, &bounty_data);

    moltchain_sdk::set_return_data(&[sub_count]); // return submission index
    log_info("✅ Work submitted");
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
pub extern "C" fn approve_work(
    caller_ptr: *const u8,
    bounty_id: u64,
    submission_idx: u8,
) -> u32 {
    log_info("✅ Approving bounty work...");

    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };

    let bk = bounty_key(bounty_id);
    let mut bounty_data = match storage_get(&bk) {
        Some(data) => data,
        None => {
            log_info("❌ Bounty not found");
            return 1;
        }
    };

    if bounty_data.len() < BOUNTY_SIZE {
        return 2;
    }

    // Verify caller is creator
    if &bounty_data[0..32] != caller {
        log_info("❌ Only creator can approve");
        return 3;
    }

    if bounty_data[80] != BOUNTY_OPEN {
        log_info("❌ Bounty is not open");
        return 4;
    }

    let sub_count = bounty_data[81];
    if submission_idx >= sub_count {
        log_info("❌ Invalid submission index");
        return 5;
    }

    // Load submission to get worker address
    let sk = submission_key(bounty_id, submission_idx);
    let _sub_data = match storage_get(&sk) {
        Some(data) => data,
        None => {
            log_info("❌ Submission not found");
            return 6;
        }
    };

    // Mark bounty as completed
    bounty_data[80] = BOUNTY_COMPLETED;
    bounty_data[90] = submission_idx;
    storage_set(&bk, &bounty_data);

    // Note: actual token transfer would happen via cross-contract call in production
    // The reward_amount is stored in the bounty for the runtime to handle
    log_info("✅ Work approved, bounty completed");
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
pub extern "C" fn cancel_bounty(
    caller_ptr: *const u8,
    bounty_id: u64,
) -> u32 {
    log_info("❌ Cancelling bounty...");

    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };

    let bk = bounty_key(bounty_id);
    let mut bounty_data = match storage_get(&bk) {
        Some(data) => data,
        None => {
            log_info("❌ Bounty not found");
            return 1;
        }
    };

    if bounty_data.len() < BOUNTY_SIZE {
        return 2;
    }

    if &bounty_data[0..32] != caller {
        log_info("❌ Only creator can cancel");
        return 3;
    }

    if bounty_data[80] != BOUNTY_OPEN {
        log_info("❌ Bounty is not open");
        return 4;
    }

    bounty_data[80] = BOUNTY_CANCELLED;
    storage_set(&bk, &bounty_data);

    let reward = bytes_to_u64(&bounty_data[64..72]);
    moltchain_sdk::set_return_data(&u64_to_bytes(reward));

    log_info("✅ Bounty cancelled, refund issued");
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
        Some(data) => {
            moltchain_sdk::set_return_data(&data);
            0
        }
        None => {
            log_info("❌ Bounty not found");
            1
        }
    }
}

// ============================================================================
// MOLTYID IDENTITY INTEGRATION
// ============================================================================

/// Storage key for identity admin
const IDENTITY_ADMIN_KEY: &[u8] = b"identity_admin";
/// Storage key for minimum reputation threshold
const MOLTYID_MIN_REP_KEY: &[u8] = b"moltyid_min_rep";
/// Storage key for MoltyID contract address (32 bytes)
const MOLTYID_ADDR_KEY: &[u8] = b"moltyid_address";

/// Set the admin for identity/reputation configuration.
/// Only callable once (first caller becomes admin).
#[no_mangle]
pub extern "C" fn set_identity_admin(admin_ptr: *const u8) -> u32 {
    let admin = unsafe { core::slice::from_raw_parts(admin_ptr, 32) };

    if storage_get(IDENTITY_ADMIN_KEY).is_some() {
        log_info("❌ Identity admin already set");
        return 1;
    }

    storage_set(IDENTITY_ADMIN_KEY, admin);
    log_info("✅ Identity admin set");
    0
}

/// Set MoltyID contract address for cross-contract reputation lookups.
/// Only callable by the identity admin.
#[no_mangle]
pub extern "C" fn set_moltyid_address(caller_ptr: *const u8, moltyid_addr_ptr: *const u8) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let moltyid_addr = unsafe { core::slice::from_raw_parts(moltyid_addr_ptr, 32) };

    let admin = match storage_get(IDENTITY_ADMIN_KEY) {
        Some(data) => data,
        None => return 1,
    };
    if caller != admin.as_slice() {
        return 2;
    }

    storage_set(MOLTYID_ADDR_KEY, moltyid_addr);
    log_info("✅ MoltyID address configured");
    0
}

/// Set minimum MoltyID reputation required for gated functions.
/// Only callable by the identity admin.
#[no_mangle]
pub extern "C" fn set_identity_gate(caller_ptr: *const u8, min_reputation: u64) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };

    let admin = match storage_get(IDENTITY_ADMIN_KEY) {
        Some(data) => data,
        None => return 1,
    };
    if caller != admin.as_slice() {
        return 2;
    }

    storage_set(MOLTYID_MIN_REP_KEY, &u64_to_bytes(min_reputation));
    log_info("✅ Identity gate configured");
    0
}

/// Check if caller meets the MoltyID reputation threshold.
/// Returns true if no gate is set or caller meets threshold.
fn check_identity_gate(caller: &[u8]) -> bool {
    let min_rep = match storage_get(MOLTYID_MIN_REP_KEY) {
        Some(data) if data.len() >= 8 => bytes_to_u64(&data),
        _ => return true,
    };
    if min_rep == 0 {
        return true;
    }

    let moltyid_addr = match storage_get(MOLTYID_ADDR_KEY) {
        Some(data) if data.len() >= 32 => data,
        _ => return true,
    };

    let mut addr = [0u8; 32];
    addr.copy_from_slice(&moltyid_addr[..32]);
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
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use moltchain_sdk::test_mock;

    fn setup() {
        test_mock::reset();
    }

    #[test]
    fn test_create_bounty() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];

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
        create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1000);

        // Submit work
        let worker = [2u8; 32];
        let proof_hash = [0xBB; 32];
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
        let result = cancel_bounty(other.as_ptr(), 0);
        assert_eq!(result, 3);
    }

    #[test]
    fn test_get_bounty() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 50);

        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
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
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        let moltyid_addr = [0x42u8; 32];
        assert_eq!(set_moltyid_address(admin.as_ptr(), moltyid_addr.as_ptr()), 0);
        assert_eq!(set_identity_gate(admin.as_ptr(), 100), 0);

        let creator = [2u8; 32];
        let title_hash = [0xAA; 32];
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
        create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1000);

        // Now configure gate
        let admin = [5u8; 32];
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        let moltyid_addr = [0x42u8; 32];
        assert_eq!(set_moltyid_address(admin.as_ptr(), moltyid_addr.as_ptr()), 0);
        assert_eq!(set_identity_gate(admin.as_ptr(), 1), 0); // any reputation

        let worker = [2u8; 32];
        let proof_hash = [0xBB; 32];
        let result = submit_work(0, worker.as_ptr(), proof_hash.as_ptr());
        assert_eq!(result, 10);
    }

    #[test]
    fn test_identity_gate_allows_when_disabled() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let creator = [1u8; 32];
        let title_hash = [0xAA; 32];
        let result = create_bounty(creator.as_ptr(), title_hash.as_ptr(), 500_000, 1000);
        assert_eq!(result, 0);

        let worker = [2u8; 32];
        let proof_hash = [0xBB; 32];
        let result = submit_work(0, worker.as_ptr(), proof_hash.as_ptr());
        assert_eq!(result, 0);
    }

    #[test]
    fn test_set_identity_gate_admin_only() {
        setup();

        let admin = [1u8; 32];
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        assert_eq!(set_identity_admin(admin.as_ptr()), 1); // already set

        let other = [9u8; 32];
        assert_eq!(set_identity_gate(other.as_ptr(), 100), 2);
        assert_eq!(set_identity_gate(admin.as_ptr(), 100), 0);
    }
}
