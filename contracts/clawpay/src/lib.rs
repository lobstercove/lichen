// ClawPay v2 — Streaming Payment Contract for MoltChain
//
// Sablier-style streaming payments:
//   - Sender creates a payment stream with total amount and time window
//   - Recipient can withdraw proportionally as time passes
//   - Sender can cancel stream (remaining unstreamed returned)
//
// v2 additions:
//   - Cliff periods (no withdrawal until cliff_slot)
//   - Stream transfer (recipient can reassign)
//   - Admin pause
//   - Enhanced stream queries
//
// Storage keys:
//   stream_{id}     → StreamInfo
//   stream_count    → u64
//   cliff_{id}      → u64 (cliff slot, 0 = no cliff)
//   cp_admin        → 32 bytes
//   cp_paused       → u8

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    log_info, storage_get, storage_set, bytes_to_u64, u64_to_bytes, get_slot,
    Address, CrossCall, call_contract,
};

// ============================================================================
// STORAGE KEY HELPERS
// ============================================================================

fn stream_key(stream_id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(7 + 20);
    key.extend_from_slice(b"stream_");
    let s = u64_to_decimal(stream_id);
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

// v2 constants
const ADMIN_KEY: &[u8] = b"cp_admin";
const PAUSE_KEY: &[u8] = b"cp_paused";

fn cliff_key(stream_id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(6 + 20);
    key.extend_from_slice(b"cliff_");
    key.extend_from_slice(&u64_to_decimal(stream_id));
    key
}

fn is_paused() -> bool {
    storage_get(PAUSE_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(false)
}

fn is_cp_admin(caller: &[u8]) -> bool {
    match storage_get(ADMIN_KEY) {
        Some(data) => data.as_slice() == caller,
        None => false,
    }
}

fn get_cliff(stream_id: u64) -> u64 {
    let ck = cliff_key(stream_id);
    storage_get(&ck).map(|d| bytes_to_u64(&d)).unwrap_or(0)
}

// ============================================================================
// STREAM LAYOUT
// ============================================================================
//
// Bytes 0..32   : sender (address)
// Bytes 32..64  : recipient (address)
// Bytes 64..72  : total_amount (u64 LE)
// Bytes 72..80  : withdrawn (u64 LE)
// Bytes 80..88  : start_slot (u64 LE)
// Bytes 88..96  : end_slot (u64 LE)
// Byte  96      : cancelled (u8, 0 or 1)
// Bytes 97..105 : created_slot (u64 LE)

const STREAM_SIZE: usize = 105;

fn encode_stream(
    sender: &[u8; 32],
    recipient: &[u8; 32],
    total_amount: u64,
    withdrawn: u64,
    start_slot: u64,
    end_slot: u64,
    cancelled: bool,
    created_slot: u64,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(STREAM_SIZE);
    data.extend_from_slice(sender);
    data.extend_from_slice(recipient);
    data.extend_from_slice(&u64_to_bytes(total_amount));
    data.extend_from_slice(&u64_to_bytes(withdrawn));
    data.extend_from_slice(&u64_to_bytes(start_slot));
    data.extend_from_slice(&u64_to_bytes(end_slot));
    data.push(if cancelled { 1 } else { 0 });
    data.extend_from_slice(&u64_to_bytes(created_slot));
    data
}

/// Calculate the currently withdrawable amount for a stream.
/// v2: cliff_slot support — nothing withdrawable until cliff passes.
fn calculate_withdrawable(
    total_amount: u64,
    withdrawn: u64,
    start_slot: u64,
    end_slot: u64,
    current_slot: u64,
    cancelled: bool,
    cliff_slot: u64,
) -> u64 {
    if cancelled || current_slot < start_slot {
        return 0;
    }

    // v2: cliff check — if cliff is set and not yet passed, nothing withdrawable
    if cliff_slot > 0 && current_slot < cliff_slot {
        return 0;
    }

    let duration = end_slot.saturating_sub(start_slot);
    if duration == 0 {
        return total_amount.saturating_sub(withdrawn);
    }

    let elapsed = if current_slot >= end_slot {
        duration
    } else {
        current_slot.saturating_sub(start_slot)
    };

    // streamed = total_amount * elapsed / duration
    let streamed = (total_amount as u128)
        .saturating_mul(elapsed as u128)
        / (duration as u128);
    let streamed = streamed as u64;

    streamed.saturating_sub(withdrawn)
}

// ============================================================================
// CREATE STREAM
// ============================================================================

/// Create a payment stream.
///
/// Parameters:
///   - sender_ptr: 32-byte sender address
///   - recipient_ptr: 32-byte recipient address
///   - total_amount: total shells to stream
///   - start_slot: slot when streaming begins
///   - end_slot: slot when streaming ends
///
/// Returns 0 on success, stream_id in return data.
#[no_mangle]
pub extern "C" fn create_stream(
    sender_ptr: *const u8,
    recipient_ptr: *const u8,
    total_amount: u64,
    start_slot: u64,
    end_slot: u64,
) -> u32 {
    log_info("💸 Creating payment stream...");

    let sender = unsafe { core::slice::from_raw_parts(sender_ptr, 32) };
    let recipient = unsafe { core::slice::from_raw_parts(recipient_ptr, 32) };

    if is_paused() {
        log_info("❌ Protocol is paused");
        return 20;
    }

    if total_amount == 0 {
        log_info("❌ Amount must be > 0");
        return 1;
    }

    if end_slot <= start_slot {
        log_info("❌ End slot must be after start slot");
        return 2;
    }

    if sender == recipient {
        log_info("❌ Sender and recipient must differ");
        return 3;
    }

    // MoltyID identity gate — both sender and recipient must have identity
    if !check_identity_gate(sender) {
        log_info("❌ Sender lacks required MoltyID reputation");
        return 10;
    }
    if !check_identity_gate(recipient) {
        log_info("❌ Recipient lacks required MoltyID reputation");
        return 11;
    }

    let mut sender_arr = [0u8; 32];
    sender_arr.copy_from_slice(sender);
    let mut recipient_arr = [0u8; 32];
    recipient_arr.copy_from_slice(recipient);

    let stream_id = storage_get(b"stream_count")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    storage_set(b"stream_count", &u64_to_bytes(stream_id + 1));

    let current_slot = get_slot();
    let data = encode_stream(
        &sender_arr,
        &recipient_arr,
        total_amount,
        0, // nothing withdrawn yet
        start_slot,
        end_slot,
        false,
        current_slot,
    );

    let sk = stream_key(stream_id);
    storage_set(&sk, &data);

    moltchain_sdk::set_return_data(&u64_to_bytes(stream_id));
    log_info("✅ Payment stream created");
    0
}

// ============================================================================
// WITHDRAW FROM STREAM
// ============================================================================

/// Recipient withdraws available funds from a stream.
///
/// Parameters:
///   - caller_ptr: 32-byte caller address (must be recipient)
///   - stream_id: the stream to withdraw from
///   - amount: amount to withdraw (must be <= withdrawable)
///
/// Returns 0 on success.
#[no_mangle]
pub extern "C" fn withdraw_from_stream(
    caller_ptr: *const u8,
    stream_id: u64,
    amount: u64,
) -> u32 {
    log_info("💰 Withdrawing from stream...");

    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };

    if amount == 0 {
        log_info("❌ Amount must be > 0");
        return 1;
    }

    let sk = stream_key(stream_id);
    let mut stream_data = match storage_get(&sk) {
        Some(data) => data,
        None => {
            log_info("❌ Stream not found");
            return 2;
        }
    };

    if stream_data.len() < STREAM_SIZE {
        return 3;
    }

    // Verify caller is recipient
    if &stream_data[32..64] != caller {
        log_info("❌ Only recipient can withdraw");
        return 4;
    }

    if stream_data[96] == 1 {
        log_info("❌ Stream is cancelled");
        return 5;
    }

    let total_amount = bytes_to_u64(&stream_data[64..72]);
    let withdrawn = bytes_to_u64(&stream_data[72..80]);
    let start_slot = bytes_to_u64(&stream_data[80..88]);
    let end_slot = bytes_to_u64(&stream_data[88..96]);
    let current_slot = get_slot();

    let cliff = get_cliff(stream_id);
    let withdrawable = calculate_withdrawable(
        total_amount, withdrawn, start_slot, end_slot, current_slot, false, cliff,
    );

    if amount > withdrawable {
        log_info("❌ Amount exceeds withdrawable balance");
        return 6;
    }

    // Update withdrawn
    let new_withdrawn = withdrawn.saturating_add(amount);
    stream_data[72..80].copy_from_slice(&u64_to_bytes(new_withdrawn));
    storage_set(&sk, &stream_data);

    moltchain_sdk::set_return_data(&u64_to_bytes(amount));
    log_info("✅ Withdrawal successful");
    0
}

// ============================================================================
// CANCEL STREAM
// ============================================================================

/// Sender cancels a stream. Remaining unstreamed amount is returned.
///
/// Parameters:
///   - caller_ptr: 32-byte caller address (must be sender)
///   - stream_id: the stream to cancel
///
/// Returns 0 on success. Unstreamed amount in return data.
#[no_mangle]
pub extern "C" fn cancel_stream(
    caller_ptr: *const u8,
    stream_id: u64,
) -> u32 {
    log_info("❌ Cancelling payment stream...");

    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };

    let sk = stream_key(stream_id);
    let mut stream_data = match storage_get(&sk) {
        Some(data) => data,
        None => {
            log_info("❌ Stream not found");
            return 1;
        }
    };

    if stream_data.len() < STREAM_SIZE {
        return 2;
    }

    // Verify caller is sender
    if &stream_data[0..32] != caller {
        log_info("❌ Only sender can cancel");
        return 3;
    }

    if stream_data[96] == 1 {
        log_info("❌ Stream already cancelled");
        return 4;
    }

    let total_amount = bytes_to_u64(&stream_data[64..72]);
    let withdrawn = bytes_to_u64(&stream_data[72..80]);
    let start_slot = bytes_to_u64(&stream_data[80..88]);
    let end_slot = bytes_to_u64(&stream_data[88..96]);
    let current_slot = get_slot();

    // Calculate how much has been streamed (not yet withdrawn) + already withdrawn
    let duration = end_slot.saturating_sub(start_slot);
    let elapsed = if current_slot >= end_slot {
        duration
    } else if current_slot < start_slot {
        0
    } else {
        current_slot.saturating_sub(start_slot)
    };

    let streamed = if duration > 0 {
        ((total_amount as u128).saturating_mul(elapsed as u128) / (duration as u128)) as u64
    } else {
        total_amount
    };

    let refund = total_amount.saturating_sub(streamed);

    // Mark as cancelled
    stream_data[96] = 1;
    storage_set(&sk, &stream_data);

    moltchain_sdk::set_return_data(&u64_to_bytes(refund));
    log_info("✅ Stream cancelled");
    0
}

// ============================================================================
// GET STREAM
// ============================================================================

/// Query stream info.
///
/// Parameters:
///   - stream_id: the stream to query
///
/// Returns 0 on success (stream data as return data), 1 if not found.
#[no_mangle]
pub extern "C" fn get_stream(stream_id: u64) -> u32 {
    let sk = stream_key(stream_id);
    match storage_get(&sk) {
        Some(data) => {
            moltchain_sdk::set_return_data(&data);
            0
        }
        None => {
            log_info("❌ Stream not found");
            1
        }
    }
}

// ============================================================================
// GET WITHDRAWABLE
// ============================================================================

/// Query the currently withdrawable amount for a stream.
///
/// Parameters:
///   - stream_id: the stream to check
///
/// Returns 0 on success (withdrawable amount as return data), 1 if not found.
#[no_mangle]
pub extern "C" fn get_withdrawable(stream_id: u64) -> u32 {
    let sk = stream_key(stream_id);
    let stream_data = match storage_get(&sk) {
        Some(data) => data,
        None => {
            log_info("❌ Stream not found");
            return 1;
        }
    };

    if stream_data.len() < STREAM_SIZE {
        return 2;
    }

    let total_amount = bytes_to_u64(&stream_data[64..72]);
    let withdrawn = bytes_to_u64(&stream_data[72..80]);
    let start_slot = bytes_to_u64(&stream_data[80..88]);
    let end_slot = bytes_to_u64(&stream_data[88..96]);
    let cancelled = stream_data[96] == 1;
    let current_slot = get_slot();

    let cliff = get_cliff(stream_id);
    let withdrawable = calculate_withdrawable(
        total_amount, withdrawn, start_slot, end_slot, current_slot, cancelled, cliff,
    );

    moltchain_sdk::set_return_data(&u64_to_bytes(withdrawable));
    0
}

// ============================================================================
// V2: CLIFF STREAMS, TRANSFER, ADMIN
// ============================================================================

/// Create a stream with a cliff period.
/// No tokens vest until cliff_slot is reached; then linear vesting begins.
///
/// Returns: 0 success, 1 bad params, 2 cliff before start, 3 cliff after end,
///          10/11 identity gated, 20 paused
#[no_mangle]
pub extern "C" fn create_stream_with_cliff(
    sender_ptr: *const u8,
    recipient_ptr: *const u8,
    total_amount: u64,
    start_slot: u64,
    end_slot: u64,
    cliff_slot: u64,
) -> u32 {
    if is_paused() {
        return 20;
    }

    let sender = unsafe { core::slice::from_raw_parts(sender_ptr, 32) };
    let recipient = unsafe { core::slice::from_raw_parts(recipient_ptr, 32) };

    if total_amount == 0 || start_slot >= end_slot {
        return 1;
    }
    if cliff_slot < start_slot {
        return 2;
    }
    if cliff_slot > end_slot {
        return 3;
    }

    // Identity gate
    if !check_identity_gate(sender) {
        return 10;
    }
    if !check_identity_gate(recipient) {
        return 11;
    }

    // Allocate stream ID
    let stream_id = storage_get(b"stream_count")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    storage_set(b"stream_count", &u64_to_bytes(stream_id + 1));

    // Build stream data
    let current_slot = get_slot();
    let mut stream = alloc::vec![0u8; STREAM_SIZE];
    stream[0..32].copy_from_slice(sender);
    stream[32..64].copy_from_slice(recipient);
    stream[64..72].copy_from_slice(&u64_to_bytes(total_amount));
    // withdrawn = 0
    stream[80..88].copy_from_slice(&u64_to_bytes(start_slot));
    stream[88..96].copy_from_slice(&u64_to_bytes(end_slot));
    // cancelled = 0
    stream[97..105].copy_from_slice(&u64_to_bytes(current_slot));

    let sk = stream_key(stream_id);
    storage_set(&sk, &stream);

    // Store cliff
    let ck = cliff_key(stream_id);
    storage_set(&ck, &u64_to_bytes(cliff_slot));

    moltchain_sdk::set_return_data(&u64_to_bytes(stream_id));
    log_info("✅ Stream created with cliff");
    0
}

/// Transfer a stream to a new recipient.
/// Only the current recipient can transfer.
///
/// Returns: 0 success, 1 not found, 2 not recipient, 3 cancelled, 4 fully withdrawn
#[no_mangle]
pub extern "C" fn transfer_stream(
    caller_ptr: *const u8,
    new_recipient_ptr: *const u8,
    stream_id: u64,
) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let new_recipient = unsafe { core::slice::from_raw_parts(new_recipient_ptr, 32) };

    let sk = stream_key(stream_id);
    let mut stream_data = match storage_get(&sk) {
        Some(data) => data,
        None => return 1,
    };
    if stream_data.len() < STREAM_SIZE {
        return 1;
    }

    // Only current recipient can transfer
    if caller != &stream_data[32..64] {
        return 2;
    }

    // Cannot transfer cancelled stream
    if stream_data[96] == 1 {
        return 3;
    }

    // Cannot transfer fully withdrawn stream
    let total = bytes_to_u64(&stream_data[64..72]);
    let withdrawn = bytes_to_u64(&stream_data[72..80]);
    if withdrawn >= total {
        return 4;
    }

    // Update recipient
    stream_data[32..64].copy_from_slice(new_recipient);
    storage_set(&sk, &stream_data);

    log_info("✅ Stream transferred to new recipient");
    0
}

/// Initialize ClawPay admin. Only callable once.
/// Returns: 0 success, 1 already set
#[no_mangle]
pub extern "C" fn initialize_cp_admin(admin_ptr: *const u8) -> u32 {
    let admin = unsafe { core::slice::from_raw_parts(admin_ptr, 32) };
    if storage_get(ADMIN_KEY).is_some() {
        return 1;
    }
    storage_set(ADMIN_KEY, admin);
    log_info("✅ ClawPay admin initialized");
    0
}

/// Pause the protocol. Only admin.
/// Returns: 0 success, 1 not admin, 2 already paused
#[no_mangle]
pub extern "C" fn pause(caller_ptr: *const u8) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    if !is_cp_admin(caller) {
        return 1;
    }
    if is_paused() {
        return 2;
    }
    storage_set(PAUSE_KEY, &[1]);
    log_info("⏸️ ClawPay paused");
    0
}

/// Unpause the protocol. Only admin.
/// Returns: 0 success, 1 not admin, 2 not paused
#[no_mangle]
pub extern "C" fn unpause(caller_ptr: *const u8) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    if !is_cp_admin(caller) {
        return 1;
    }
    if !is_paused() {
        return 2;
    }
    storage_set(PAUSE_KEY, &[0]);
    log_info("▶️ ClawPay unpaused");
    0
}

/// Get stream info. Returns stream data as return data.
/// Layout: sender(32) + recipient(32) + total(8) + withdrawn(8) + start(8) + end(8) + cancelled(1) + created(8) + cliff(8)
/// Returns: 0 success, 1 not found
#[no_mangle]
pub extern "C" fn get_stream_info(stream_id: u64) -> u32 {
    let sk = stream_key(stream_id);
    let stream_data = match storage_get(&sk) {
        Some(data) => data,
        None => return 1,
    };
    if stream_data.len() < STREAM_SIZE {
        return 1;
    }

    let cliff = get_cliff(stream_id);
    let mut info = Vec::with_capacity(STREAM_SIZE + 8);
    info.extend_from_slice(&stream_data[..STREAM_SIZE]);
    info.extend_from_slice(&u64_to_bytes(cliff));
    moltchain_sdk::set_return_data(&info);
    0
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
    fn test_create_stream() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        let result = create_stream(
            sender.as_ptr(),
            recipient.as_ptr(),
            1_000_000, // 1M shells
            100,       // start now
            1100,      // end at slot 1100 (1000 slot duration)
        );
        assert_eq!(result, 0);

        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 0); // stream_id = 0

        let sk = stream_key(0);
        let stream = test_mock::get_storage(&sk).unwrap();
        assert_eq!(stream.len(), STREAM_SIZE);
        assert_eq!(&stream[0..32], &sender);
        assert_eq!(&stream[32..64], &recipient);
        assert_eq!(bytes_to_u64(&stream[64..72]), 1_000_000);
        assert_eq!(bytes_to_u64(&stream[72..80]), 0); // nothing withdrawn
        assert_eq!(stream[96], 0); // not cancelled
    }

    #[test]
    fn test_withdraw_from_stream() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        create_stream(
            sender.as_ptr(),
            recipient.as_ptr(),
            1_000_000,
            100,  // start
            1100, // end (1000 slot duration)
        );

        // Move to halfway point (slot 600 = 500 slots elapsed out of 1000)
        test_mock::SLOT.with(|s| *s.borrow_mut() = 600);

        // Withdrawable should be 500,000 (50% of 1M)
        let result = get_withdrawable(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 500_000);

        // Withdraw 300,000
        let result = withdraw_from_stream(recipient.as_ptr(), 0, 300_000);
        assert_eq!(result, 0);

        // Now withdrawable should be 200,000
        let result = get_withdrawable(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 200_000);

        // Try to withdraw too much
        let result = withdraw_from_stream(recipient.as_ptr(), 0, 300_000);
        assert_eq!(result, 6); // exceeds withdrawable
    }

    #[test]
    fn test_cancel_stream() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        create_stream(
            sender.as_ptr(),
            recipient.as_ptr(),
            1_000_000,
            100,
            1100,
        );

        // Move to 25% (slot 350 = 250 slots of 1000)
        test_mock::SLOT.with(|s| *s.borrow_mut() = 350);

        let result = cancel_stream(sender.as_ptr(), 0);
        assert_eq!(result, 0);

        // Refund should be 75% = 750,000
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 750_000);

        // Stream should be marked cancelled
        let sk = stream_key(0);
        let stream = test_mock::get_storage(&sk).unwrap();
        assert_eq!(stream[96], 1);

        // Withdrawable should now be 0
        let result = get_withdrawable(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 0);
    }

    #[test]
    fn test_full_stream_withdrawal() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        create_stream(sender.as_ptr(), recipient.as_ptr(), 500_000, 100, 600);

        // Move past end
        test_mock::SLOT.with(|s| *s.borrow_mut() = 700);

        let result = get_withdrawable(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 500_000); // full amount

        let result = withdraw_from_stream(recipient.as_ptr(), 0, 500_000);
        assert_eq!(result, 0);

        // Nothing left
        let result = get_withdrawable(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 0);
    }

    #[test]
    fn test_identity_gate_blocks_create_stream_sender() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [5u8; 32];
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);
        let moltyid_addr = [0x42u8; 32];
        assert_eq!(set_moltyid_address(admin.as_ptr(), moltyid_addr.as_ptr()), 0);
        assert_eq!(set_identity_gate(admin.as_ptr(), 1), 0);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        let result = create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);
        assert_eq!(result, 10); // sender blocked
    }

    #[test]
    fn test_identity_gate_allows_when_disabled() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        let result = create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_set_identity_gate_admin_only() {
        setup();

        let admin = [1u8; 32];
        assert_eq!(set_identity_admin(admin.as_ptr()), 0);

        let other = [9u8; 32];
        assert_eq!(set_identity_gate(other.as_ptr(), 100), 2);
        assert_eq!(set_identity_gate(admin.as_ptr(), 100), 0);
    }

    // ====================================================================
    // V2 TESTS
    // ====================================================================

    #[test]
    fn test_create_stream_with_cliff() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        // cliff at slot 500 (400 slots into 1000-slot stream)
        let result = create_stream_with_cliff(
            sender.as_ptr(),
            recipient.as_ptr(),
            1_000_000,
            100,  // start
            1100, // end
            500,  // cliff
        );
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 0); // stream_id = 0
    }

    #[test]
    fn test_cliff_blocks_withdrawal_before_cliff() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        create_stream_with_cliff(
            sender.as_ptr(),
            recipient.as_ptr(),
            1_000_000,
            100,
            1100,
            500, // cliff at 500
        );

        // Before cliff (slot 300) — should get 0
        test_mock::SLOT.with(|s| *s.borrow_mut() = 300);
        let result = get_withdrawable(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 0);

        // Try to withdraw — should fail
        let result = withdraw_from_stream(recipient.as_ptr(), 0, 1);
        assert_eq!(result, 6); // exceeds withdrawable (0)
    }

    #[test]
    fn test_cliff_allows_withdrawal_after_cliff() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        create_stream_with_cliff(
            sender.as_ptr(),
            recipient.as_ptr(),
            1_000_000,
            100,
            1100,
            500, // cliff at 500
        );

        // After cliff (slot 600) — 500 elapsed out of 1000 = 50%
        test_mock::SLOT.with(|s| *s.borrow_mut() = 600);
        let result = get_withdrawable(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 500_000);

        // Withdraw works after cliff
        let result = withdraw_from_stream(recipient.as_ptr(), 0, 500_000);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_cliff_invalid_params() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        // cliff before start
        let result = create_stream_with_cliff(
            sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100, 50,
        );
        assert_eq!(result, 2);

        // cliff after end
        let result = create_stream_with_cliff(
            sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100, 2000,
        );
        assert_eq!(result, 3);
    }

    #[test]
    fn test_transfer_stream() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        let new_recipient = [3u8; 32];

        create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);

        // Non-recipient cannot transfer
        let result = transfer_stream(sender.as_ptr(), new_recipient.as_ptr(), 0);
        assert_eq!(result, 2);

        // Recipient can transfer
        let result = transfer_stream(recipient.as_ptr(), new_recipient.as_ptr(), 0);
        assert_eq!(result, 0);

        // New recipient can now withdraw
        test_mock::SLOT.with(|s| *s.borrow_mut() = 600);
        let result = withdraw_from_stream(new_recipient.as_ptr(), 0, 100_000);
        assert_eq!(result, 0);

        // Old recipient cannot withdraw
        let result = withdraw_from_stream(recipient.as_ptr(), 0, 100_000);
        assert_eq!(result, 4); // not recipient
    }

    #[test]
    fn test_transfer_cancelled_stream_fails() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        let new_recip = [3u8; 32];

        create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);
        cancel_stream(sender.as_ptr(), 0);

        let result = transfer_stream(recipient.as_ptr(), new_recip.as_ptr(), 0);
        assert_eq!(result, 3); // cancelled
    }

    #[test]
    fn test_pause_unpause() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [10u8; 32];
        let non_admin = [11u8; 32];
        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        // Init admin
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);
        // Cannot init twice
        assert_eq!(initialize_cp_admin(non_admin.as_ptr()), 1);

        // Non-admin cannot pause
        assert_eq!(pause(non_admin.as_ptr()), 1);

        // Admin pauses
        assert_eq!(pause(admin.as_ptr()), 0);
        // Cannot pause again
        assert_eq!(pause(admin.as_ptr()), 2);

        // create_stream blocked when paused
        let result = create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);
        assert_eq!(result, 20);

        // create_stream_with_cliff blocked too
        let result = create_stream_with_cliff(
            sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100, 500,
        );
        assert_eq!(result, 20);

        // Non-admin cannot unpause
        assert_eq!(unpause(non_admin.as_ptr()), 1);
        // Unpause
        assert_eq!(unpause(admin.as_ptr()), 0);
        // Cannot unpause again
        assert_eq!(unpause(admin.as_ptr()), 2);

        // Now create_stream works again
        let result = create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_get_stream_info_with_cliff() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        create_stream_with_cliff(
            sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100, 500,
        );

        let result = get_stream_info(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        // STREAM_SIZE (105) + cliff (8) = 113
        assert_eq!(ret.len(), STREAM_SIZE + 8);
        assert_eq!(bytes_to_u64(&ret[STREAM_SIZE..STREAM_SIZE + 8]), 500);
    }

    #[test]
    fn test_get_stream_info_not_found() {
        setup();
        let result = get_stream_info(999);
        assert_eq!(result, 1);
    }

    #[test]
    fn test_withdraw_blocked_when_paused_still_works() {
        // Withdrawal/cancel should NOT be blocked by pause (safety valve)
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [10u8; 32];
        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        // Create before pause
        create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);

        // Pause
        initialize_cp_admin(admin.as_ptr());
        pause(admin.as_ptr());

        // Withdraw still works (safety valve)
        test_mock::SLOT.with(|s| *s.borrow_mut() = 600);
        let result = withdraw_from_stream(recipient.as_ptr(), 0, 100_000);
        assert_eq!(result, 0);

        // Cancel still works
        let result = cancel_stream(sender.as_ptr(), 0);
        assert_eq!(result, 0);
    }
}
