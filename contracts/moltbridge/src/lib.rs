// MoltBridge — Cross-Chain Bridge Contract (v2 — Multi-Call Confirmation)
//
// Security model:
//   - Lock tokens: User-initiated, no multi-sig needed (user locks own tokens)
//   - Mint tokens: Multi-call — validator submits request, other validators confirm
//   - Unlock tokens: Multi-call — validator submits request, other validators confirm
//   - Source TX deduplication: Same external deposit can only be minted once
//   - Burn proof deduplication: Same burn proof can only unlock once
//   - Request expiry: Stale requests can be cancelled after timeout
//
// Storage layout:
//   bridge_owner                         → [u8; 32] owner address
//   bridge_validator_{hex(addr)}         → [1] (authorized validator)
//   bridge_validator_count               → u64
//   bridge_required_confirms             → u64
//   bridge_locked_amount                 → u64 (total locked tokens)
//   bridge_nonce                         → u64 (monotonic for all tx types)
//   bridge_tx_{nonce}                    → BridgeTx (115 bytes)
//   bridge_mc_{nonce}_{hex(validator)}   → [1] (mint confirmation)
//   bridge_uc_{nonce}_{hex(validator)}   → [1] (unlock confirmation)
//   bridge_st_used_{hex(source_tx)}      → u64 (nonce, source tx dedup)
//   bridge_bp_used_{hex(burn_proof)}     → u64 (nonce, burn proof dedup)
//   bridge_request_timeout               → u64 (slots until expiry)
//   moltyid_address                      → [u8; 32]
//   moltyid_min_rep                      → u64

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    log_info, storage_get, storage_set, bytes_to_u64, u64_to_bytes, get_slot, get_caller,
    Address, CrossCall, call_contract,
};

// Reentrancy guard
const MB_REENTRANCY_KEY: &[u8] = b"mb_reentrancy";

fn reentrancy_enter() -> bool {
    if storage_get(MB_REENTRANCY_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(false) {
        return false;
    }
    storage_set(MB_REENTRANCY_KEY, &[1u8]);
    true
}

fn reentrancy_exit() {
    storage_set(MB_REENTRANCY_KEY, &[0u8]);
}

// Emergency pause
const MB_PAUSE_KEY: &[u8] = b"mb_paused";

fn is_mb_paused() -> bool {
    storage_get(MB_PAUSE_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(false)
}

// ============================================================================
// CONSTANTS
// ============================================================================

const DEFAULT_REQUIRED_CONFIRMATIONS: u64 = 2;
const DEFAULT_REQUEST_TIMEOUT: u64 = 43_200; // ~12 hours at 1 slot/sec
const MAX_REQUIRED_CONFIRMATIONS: u64 = 100;
const MIN_REQUEST_TIMEOUT: u64 = 100;

// ============================================================================
// BRIDGE TX STATUS
// ============================================================================

const STATUS_PENDING: u8 = 0;
const STATUS_COMPLETED: u8 = 1;
const STATUS_CANCELLED: u8 = 2;
const STATUS_EXPIRED: u8 = 3;

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

fn validator_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(17 + 64);
    key.extend_from_slice(b"bridge_validator_");
    key.extend_from_slice(&hex_encode(addr));
    key
}

fn bridge_tx_key(nonce: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(10 + 20);
    key.extend_from_slice(b"bridge_tx_");
    key.extend_from_slice(&u64_to_decimal(nonce));
    key
}

fn mint_confirm_key(nonce: u64, validator: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(11 + 20 + 1 + 64);
    key.extend_from_slice(b"bridge_mc_");
    key.extend_from_slice(&u64_to_decimal(nonce));
    key.push(b'_');
    key.extend_from_slice(&hex_encode(validator));
    key
}

fn unlock_confirm_key(nonce: u64, validator: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(11 + 20 + 1 + 64);
    key.extend_from_slice(b"bridge_uc_");
    key.extend_from_slice(&u64_to_decimal(nonce));
    key.push(b'_');
    key.extend_from_slice(&hex_encode(validator));
    key
}

fn source_tx_used_key(tx_hash: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(15 + 64);
    key.extend_from_slice(b"bridge_st_used_");
    key.extend_from_slice(&hex_encode(tx_hash));
    key
}

fn burn_proof_used_key(proof: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(15 + 64);
    key.extend_from_slice(b"bridge_bp_used_");
    key.extend_from_slice(&hex_encode(proof));
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

// ============================================================================
// BRIDGE TX LAYOUT (115 bytes — backward compatible)
// ============================================================================
//
// Bytes 0..32   : address (sender for lock, recipient for mint/unlock)
// Bytes 32..40  : amount (u64 LE)
// Byte  40      : direction (0 = lock/out, 1 = mint/in, 2 = unlock/return)
// Byte  41      : status (0 = pending, 1 = completed, 2 = cancelled, 3 = expired)
// Bytes 42..50  : created_slot (u64 LE)
// Byte  50      : confirm_count (u8) — ON-CHAIN counted confirmations
// Bytes 51..83  : chain_hash (32 bytes) or zeros for unlock
// Bytes 83..115 : dest_address / source_tx_hash / burn_proof (32 bytes)
//
// Total: 115 bytes

const BRIDGE_TX_SIZE: usize = 115;

fn encode_bridge_tx(
    addr: &[u8; 32],
    amount: u64,
    direction: u8,
    status: u8,
    created_slot: u64,
    confirm_count: u8,
    chain_hash: &[u8; 32],
    extra_hash: &[u8; 32],
) -> Vec<u8> {
    let mut data = Vec::with_capacity(BRIDGE_TX_SIZE);
    data.extend_from_slice(addr);
    data.extend_from_slice(&u64_to_bytes(amount));
    data.push(direction);
    data.push(status);
    data.extend_from_slice(&u64_to_bytes(created_slot));
    data.push(confirm_count);
    data.extend_from_slice(chain_hash);
    data.extend_from_slice(extra_hash);
    data
}

/// Update the status and confirm_count of an existing bridge TX in-place.
fn update_bridge_tx_status(nonce: u64, status: u8, confirm_count: u8) {
    let key = bridge_tx_key(nonce);
    if let Some(mut data) = storage_get(&key) {
        if data.len() == BRIDGE_TX_SIZE {
            data[41] = status;
            data[50] = confirm_count;
            storage_set(&key, &data);
        }
    }
}

// ============================================================================
// INTERNAL HELPERS
// ============================================================================

fn get_request_timeout() -> u64 {
    storage_get(b"bridge_request_timeout")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT)
}

fn get_required_confirmations() -> u64 {
    storage_get(b"bridge_required_confirms")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_REQUIRED_CONFIRMATIONS)
}

fn is_validator(addr: &[u8; 32]) -> bool {
    match storage_get(&validator_key(addr)) {
        Some(data) => !data.is_empty() && data[0] == 1,
        None => false,
    }
}

fn require_owner(caller: &[u8]) -> Result<(), u32> {
    match storage_get(b"bridge_owner") {
        Some(data) if caller == data.as_slice() => Ok(()),
        Some(_) => Err(2),
        None => Err(1),
    }
}

fn allocate_nonce() -> u64 {
    let nonce = storage_get(b"bridge_nonce")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    storage_set(b"bridge_nonce", &u64_to_bytes(nonce + 1));
    nonce
}

fn is_zero(data: &[u8; 32]) -> bool {
    data.iter().all(|&b| b == 0)
}

// ============================================================================
// INITIALIZE
// ============================================================================

/// Initialize the bridge contract. Sets the owner.
#[no_mangle]
pub extern "C" fn initialize(owner_ptr: *const u8) -> u32 {
    log_info("Initializing MoltBridge v2 (multi-call confirmation)...");

    let mut owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != owner {
        return 200;
    }

    if storage_get(b"bridge_owner").is_some() {
        log_info("Bridge already initialized");
        return 1;
    }

    storage_set(b"bridge_owner", &owner);
    storage_set(b"bridge_validator_count", &u64_to_bytes(0));
    storage_set(b"bridge_required_confirms", &u64_to_bytes(DEFAULT_REQUIRED_CONFIRMATIONS));
    storage_set(b"bridge_locked_amount", &u64_to_bytes(0));
    storage_set(b"bridge_nonce", &u64_to_bytes(0));
    storage_set(b"bridge_request_timeout", &u64_to_bytes(DEFAULT_REQUEST_TIMEOUT));

    log_info("MoltBridge v2 initialized");
    0
}

// ============================================================================
// VALIDATOR MANAGEMENT (owner only)
// ============================================================================

/// Add an authorized bridge validator.
///
/// Parameters:
///   - caller_ptr: 32-byte caller address (must be owner)
///   - validator_ptr: 32-byte validator pubkey to add
#[no_mangle]
pub extern "C" fn add_bridge_validator(
    caller_ptr: *const u8,
    validator_ptr: *const u8,
) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let mut val_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(validator_ptr, val_arr.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if let Err(code) = require_owner(&caller) {
        return code;
    }

    if is_zero(&val_arr) {
        log_info("Cannot add zero address as validator");
        return 4;
    }

    let vk = validator_key(&val_arr);
    if is_validator(&val_arr) {
        log_info("Validator already registered");
        return 3;
    }

    storage_set(&vk, &[1]);

    let count = storage_get(b"bridge_validator_count")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    storage_set(b"bridge_validator_count", &u64_to_bytes(count + 1));

    log_info("Bridge validator added");
    0
}

/// Remove an authorized bridge validator.
///
/// Parameters:
///   - caller_ptr: 32-byte caller address (must be owner)
///   - validator_ptr: 32-byte validator pubkey to remove
#[no_mangle]
pub extern "C" fn remove_bridge_validator(
    caller_ptr: *const u8,
    validator_ptr: *const u8,
) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let mut val_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(validator_ptr, val_arr.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if let Err(code) = require_owner(&caller) {
        return code;
    }

    let vk = validator_key(&val_arr);

    if !is_validator(&val_arr) {
        log_info("Validator not registered");
        return 3;
    }

    let count = storage_get(b"bridge_validator_count")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(1);

    // SECURITY-FIX: Check threshold BEFORE removing validator to prevent
    // state mutation on error (validator was being deleted before this check)
    let required = storage_get(b"bridge_required_confirms")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_REQUIRED_CONFIRMATIONS);
    if count > 0 && (count - 1) < required {
        log_info("Cannot remove validator: would drop below confirmation threshold");
        return 4;
    }

    // Mark as removed (set to [0] so is_validator returns false)
    storage_set(&vk, &[0]);

    if count > 0 {
        storage_set(b"bridge_validator_count", &u64_to_bytes(count - 1));
    }

    log_info("Bridge validator removed");
    0
}

/// Set the required number of confirmations for mint/unlock.
///
/// Parameters:
///   - caller_ptr: 32-byte caller (must be owner)
///   - required: new threshold (1..100)
#[no_mangle]
pub extern "C" fn set_required_confirmations(
    caller_ptr: *const u8,
    required: u64,
) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if let Err(code) = require_owner(&caller) {
        return code;
    }

    if required == 0 || required > MAX_REQUIRED_CONFIRMATIONS {
        log_info("Required confirmations must be 1..100");
        return 3;
    }

    storage_set(b"bridge_required_confirms", &u64_to_bytes(required));
    log_info("Required confirmations updated");
    0
}

/// Set the request timeout in slots.
///
/// Parameters:
///   - caller_ptr: 32-byte caller (must be owner)
///   - timeout_slots: slots until request expires (min 100)
#[no_mangle]
pub extern "C" fn set_request_timeout(
    caller_ptr: *const u8,
    timeout_slots: u64,
) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if let Err(code) = require_owner(&caller) {
        return code;
    }

    if timeout_slots < MIN_REQUEST_TIMEOUT {
        log_info("Timeout must be >= 100 slots");
        return 3;
    }

    storage_set(b"bridge_request_timeout", &u64_to_bytes(timeout_slots));
    log_info("Request timeout updated");
    0
}

// ============================================================================
// LOCK TOKENS (user-initiated, no multi-sig needed)
// ============================================================================

/// Lock MOLT tokens for bridging to an external chain.
///
/// Parameters:
///   - sender_ptr: 32-byte sender address
///   - amount: number of shells to lock
///   - dest_chain_ptr: 32-byte hash of destination chain name
///   - dest_address_ptr: 32-byte destination address on target chain
///
/// Returns 0 on success. The bridge nonce is set as return data.
#[no_mangle]
pub extern "C" fn lock_tokens(
    sender_ptr: *const u8,
    amount: u64,
    dest_chain_ptr: *const u8,
    dest_address_ptr: *const u8,
) -> u32 {
    if is_mb_paused() {
        log_info("Bridge is paused");
        return 20;
    }
    if !reentrancy_enter() {
        return 21;
    }
    log_info("Locking tokens for bridge...");

    let mut sender_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(sender_ptr, sender_arr.as_mut_ptr(), 32); }
    let mut chain_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(dest_chain_ptr, chain_arr.as_mut_ptr(), 32); }
    let mut addr_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(dest_address_ptr, addr_arr.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != sender_arr {
        reentrancy_exit();
        return 200;
    }

    if amount == 0 {
        log_info("Amount must be > 0");
        reentrancy_exit();
        return 1;
    }

    // MoltyID reputation gate
    if !check_identity_gate(&sender_arr) {
        log_info("Insufficient MoltyID reputation for bridge");
        reentrancy_exit();
        return 10;
    }

    // Validate destination address is not zero
    if is_zero(&addr_arr) {
        log_info("Destination address cannot be zero");
        reentrancy_exit();
        return 5;
    }

    let nonce = allocate_nonce();

    // Update locked amount
    let locked = storage_get(b"bridge_locked_amount")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    storage_set(b"bridge_locked_amount", &u64_to_bytes(locked.saturating_add(amount)));

    // Store bridge transaction — lock is immediately complete (user-initiated)
    let current_slot = get_slot();
    let tx_data = encode_bridge_tx(
        &sender_arr,
        amount,
        0, // direction: lock/out
        STATUS_COMPLETED, // lock is immediately complete
        current_slot,
        0, // no confirmations needed for lock
        &chain_arr,
        &addr_arr,
    );
    storage_set(&bridge_tx_key(nonce), &tx_data);

    moltchain_sdk::set_return_data(&u64_to_bytes(nonce));
    log_info("Tokens locked for bridging");
    reentrancy_exit();
    0
}

// ============================================================================
// SUBMIT MINT (multi-call — replaces old mint_bridged)
// ============================================================================

/// Submit a mint request after observing a deposit on an external chain.
/// The submitting validator counts as the FIRST confirmation.
/// If required_confirmations == 1, the mint completes immediately.
///
/// Parameters:
///   - caller_ptr: 32-byte caller (must be registered validator)
///   - recipient_ptr: 32-byte recipient address for minted tokens
///   - amount: number of shells to mint
///   - source_chain_ptr: 32-byte hash of source chain
///   - source_tx_ptr: 32-byte hash of the deposit transaction on external chain
///
/// Returns 0 on success. The bridge nonce is set as return data.
#[no_mangle]
pub extern "C" fn submit_mint(
    caller_ptr: *const u8,
    recipient_ptr: *const u8,
    amount: u64,
    source_chain_ptr: *const u8,
    source_tx_ptr: *const u8,
) -> u32 {
    // AUDIT-FIX: Pause must block validator operations (emergency circuit breaker)
    if is_mb_paused() {
        log_info("Bridge is paused");
        return 20;
    }
    log_info("Submitting mint request...");

    let mut caller_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller_arr.as_mut_ptr(), 32); }
    let mut recipient_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(recipient_ptr, recipient_arr.as_mut_ptr(), 32); }
    let mut chain_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(source_chain_ptr, chain_arr.as_mut_ptr(), 32); }
    let mut tx_hash_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(source_tx_ptr, tx_hash_arr.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller_arr {
        return 200;
    }

    if amount == 0 {
        log_info("Amount must be > 0");
        return 1;
    }

    // Verify caller is a registered validator
    if !is_validator(&caller_arr) {
        log_info("Caller is not an authorized bridge validator");
        return 2;
    }

    // Source TX deduplication — prevent double-minting
    if is_zero(&tx_hash_arr) {
        log_info("Source transaction hash cannot be zero");
        return 5;
    }
    let stk = source_tx_used_key(&tx_hash_arr);
    if storage_get(&stk).is_some() {
        log_info("Source transaction already processed (duplicate)");
        return 4;
    }

    // Validate recipient
    if is_zero(&recipient_arr) {
        log_info("Recipient address cannot be zero");
        return 6;
    }

    // Allocate nonce
    let nonce = allocate_nonce();

    // Mark source TX as used (maps to nonce for traceability)
    storage_set(&stk, &u64_to_bytes(nonce));

    // Create bridge TX record as PENDING
    let current_slot = get_slot();
    let tx_data = encode_bridge_tx(
        &recipient_arr,
        amount,
        1, // direction: mint/in
        STATUS_PENDING,
        current_slot,
        1, // submitter counts as first confirmation
        &chain_arr,
        &tx_hash_arr,
    );
    storage_set(&bridge_tx_key(nonce), &tx_data);

    // Record submitter's confirmation on-chain
    storage_set(&mint_confirm_key(nonce, &caller_arr), &[1]);

    // Check if threshold already met (e.g., required_confirmations == 1)
    let required = get_required_confirmations();
    if 1 >= required {
        update_bridge_tx_status(nonce, STATUS_COMPLETED, 1);
        log_info("Mint auto-completed (threshold met with 1 confirmation)");
    } else {
        log_info("Mint request submitted, awaiting confirmations");
    }

    moltchain_sdk::set_return_data(&u64_to_bytes(nonce));
    0
}

// ============================================================================
// CONFIRM MINT (multi-call)
// ============================================================================

/// Confirm a pending mint request. Each validator calls this independently.
/// When the confirmation count reaches required_confirmations, the mint completes.
///
/// Parameters:
///   - caller_ptr: 32-byte caller (must be registered validator)
///   - nonce: the mint request nonce to confirm
///
/// Returns 0 on success.
#[no_mangle]
pub extern "C" fn confirm_mint(
    caller_ptr: *const u8,
    nonce: u64,
) -> u32 {
    // AUDIT-FIX: Pause must block validator operations (emergency circuit breaker)
    if is_mb_paused() {
        log_info("Bridge is paused");
        return 20;
    }
    log_info("Confirming mint request...");

    let mut caller_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller_arr.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller_arr {
        return 200;
    }

    // Verify caller is validator
    if !is_validator(&caller_arr) {
        log_info("Caller is not an authorized bridge validator");
        return 2;
    }

    // Load bridge TX
    let tx_key = bridge_tx_key(nonce);
    let tx_data = match storage_get(&tx_key) {
        Some(data) if data.len() == BRIDGE_TX_SIZE => data,
        _ => {
            log_info("Bridge transaction not found");
            return 1;
        }
    };

    // Verify it's a pending mint
    if tx_data[40] != 1 {
        log_info("Transaction is not a mint request");
        return 5;
    }
    if tx_data[41] != STATUS_PENDING {
        log_info("Mint request is not pending");
        return 6;
    }

    // Check expiry
    let created_slot = bytes_to_u64(&tx_data[42..50]);
    let current_slot = get_slot();
    let timeout = get_request_timeout();
    if current_slot > created_slot.saturating_add(timeout) {
        update_bridge_tx_status(nonce, STATUS_EXPIRED, tx_data[50]);
        log_info("Mint request has expired");
        return 7;
    }

    // Check duplicate confirmation
    let ck = mint_confirm_key(nonce, &caller_arr);
    if storage_get(&ck).is_some() {
        log_info("Validator already confirmed this mint");
        return 8;
    }

    // Record confirmation on-chain
    storage_set(&ck, &[1]);
    let new_count = tx_data[50].saturating_add(1);

    // Check threshold
    let required = get_required_confirmations();
    if (new_count as u64) >= required {
        update_bridge_tx_status(nonce, STATUS_COMPLETED, new_count);
        log_info("Mint confirmed and completed — threshold reached");
    } else {
        update_bridge_tx_status(nonce, STATUS_PENDING, new_count);
        log_info("Mint confirmation recorded, awaiting more");
    }

    0
}

// ============================================================================
// SUBMIT UNLOCK (multi-call — replaces old unlock_tokens)
// ============================================================================

/// Submit an unlock request after verifying burn on external chain.
/// Reserves the locked amount immediately (prevents race conditions).
/// The submitting validator counts as the FIRST confirmation.
///
/// Parameters:
///   - caller_ptr: 32-byte caller (must be registered validator)
///   - recipient_ptr: 32-byte recipient address for unlocked tokens
///   - amount: number of shells to unlock
///   - burn_proof_ptr: 32-byte burn proof hash from external chain
///
/// Returns 0 on success. The bridge nonce is set as return data.
#[no_mangle]
pub extern "C" fn submit_unlock(
    caller_ptr: *const u8,
    recipient_ptr: *const u8,
    amount: u64,
    burn_proof_ptr: *const u8,
) -> u32 {
    // AUDIT-FIX: Pause must block validator operations (emergency circuit breaker)
    if is_mb_paused() {
        log_info("Bridge is paused");
        return 20;
    }
    log_info("Submitting unlock request...");

    let mut caller_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller_arr.as_mut_ptr(), 32); }
    let mut recipient_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(recipient_ptr, recipient_arr.as_mut_ptr(), 32); }
    let mut proof_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(burn_proof_ptr, proof_arr.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller_arr {
        return 200;
    }

    if amount == 0 {
        log_info("Amount must be > 0");
        return 1;
    }

    // Verify caller is validator
    if !is_validator(&caller_arr) {
        log_info("Caller is not an authorized bridge validator");
        return 2;
    }

    // Burn proof deduplication — prevent double-unlocking
    if is_zero(&proof_arr) {
        log_info("Burn proof cannot be zero");
        return 5;
    }
    let bpk = burn_proof_used_key(&proof_arr);
    if storage_get(&bpk).is_some() {
        log_info("Burn proof already used (duplicate)");
        return 4;
    }

    // Check sufficient locked balance and reserve immediately
    let locked = storage_get(b"bridge_locked_amount")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    if amount > locked {
        log_info("Insufficient locked balance");
        return 3;
    }
    // Reserve the amount immediately to prevent race conditions
    storage_set(b"bridge_locked_amount", &u64_to_bytes(locked - amount));

    // Validate recipient
    if is_zero(&recipient_arr) {
        log_info("Recipient address cannot be zero");
        // Unreserve on validation failure
        storage_set(b"bridge_locked_amount", &u64_to_bytes(locked));
        return 6;
    }

    // Allocate nonce
    let nonce = allocate_nonce();

    // Mark burn proof as used (maps to nonce)
    storage_set(&bpk, &u64_to_bytes(nonce));

    // Create bridge TX record as PENDING
    let current_slot = get_slot();
    let tx_data = encode_bridge_tx(
        &recipient_arr,
        amount,
        2, // direction: unlock/return
        STATUS_PENDING,
        current_slot,
        1, // submitter = first confirmation
        &[0u8; 32], // no chain hash for unlock
        &proof_arr,
    );
    storage_set(&bridge_tx_key(nonce), &tx_data);

    // Record submitter's confirmation on-chain
    storage_set(&unlock_confirm_key(nonce, &caller_arr), &[1]);

    // Check if threshold already met
    let required = get_required_confirmations();
    if 1 >= required {
        update_bridge_tx_status(nonce, STATUS_COMPLETED, 1);
        log_info("Unlock auto-completed (threshold met)");
    } else {
        log_info("Unlock request submitted, awaiting confirmations");
    }

    moltchain_sdk::set_return_data(&u64_to_bytes(nonce));
    0
}

// ============================================================================
// CONFIRM UNLOCK (multi-call)
// ============================================================================

/// Confirm a pending unlock request. Each validator calls this independently.
/// When confirmation count reaches threshold, the unlock completes.
/// If the request has expired, reserved funds are returned to locked pool.
///
/// Parameters:
///   - caller_ptr: 32-byte caller (must be registered validator)
///   - nonce: the unlock request nonce to confirm
///
/// Returns 0 on success.
#[no_mangle]
pub extern "C" fn confirm_unlock(
    caller_ptr: *const u8,
    nonce: u64,
) -> u32 {
    // AUDIT-FIX: Pause must block validator operations (emergency circuit breaker)
    if is_mb_paused() {
        log_info("Bridge is paused");
        return 20;
    }
    log_info("Confirming unlock request...");

    let mut caller_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller_arr.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller_arr {
        return 200;
    }

    // Verify caller is validator
    if !is_validator(&caller_arr) {
        log_info("Caller is not an authorized bridge validator");
        return 2;
    }

    // Load bridge TX
    let tx_key = bridge_tx_key(nonce);
    let tx_data = match storage_get(&tx_key) {
        Some(data) if data.len() == BRIDGE_TX_SIZE => data,
        _ => {
            log_info("Bridge transaction not found");
            return 1;
        }
    };

    // Verify it's a pending unlock
    if tx_data[40] != 2 {
        log_info("Transaction is not an unlock request");
        return 5;
    }
    if tx_data[41] != STATUS_PENDING {
        log_info("Unlock request is not pending");
        return 6;
    }

    // Check expiry
    let created_slot = bytes_to_u64(&tx_data[42..50]);
    let current_slot = get_slot();
    let timeout = get_request_timeout();
    if current_slot > created_slot.saturating_add(timeout) {
        // Return reserved funds on expiry
        let amount = bytes_to_u64(&tx_data[32..40]);
        let locked = storage_get(b"bridge_locked_amount")
            .map(|d| bytes_to_u64(&d))
            .unwrap_or(0);
        storage_set(b"bridge_locked_amount", &u64_to_bytes(locked.saturating_add(amount)));
        update_bridge_tx_status(nonce, STATUS_EXPIRED, tx_data[50]);
        log_info("Unlock request has expired, funds returned to reserve");
        return 7;
    }

    // Check duplicate confirmation
    let ck = unlock_confirm_key(nonce, &caller_arr);
    if storage_get(&ck).is_some() {
        log_info("Validator already confirmed this unlock");
        return 8;
    }

    // Record confirmation on-chain
    storage_set(&ck, &[1]);
    let new_count = tx_data[50].saturating_add(1);

    // Check threshold
    let required = get_required_confirmations();
    if (new_count as u64) >= required {
        update_bridge_tx_status(nonce, STATUS_COMPLETED, new_count);
        log_info("Unlock confirmed and completed — threshold reached");
    } else {
        update_bridge_tx_status(nonce, STATUS_PENDING, new_count);
        log_info("Unlock confirmation recorded, awaiting more");
    }

    0
}

// ============================================================================
// CANCEL EXPIRED REQUEST (anyone can call — public cleanup)
// ============================================================================

/// Cancel an expired pending request. For unlock requests, reserved
/// funds are returned to the locked pool.
///
/// Parameters:
///   - nonce: the bridge transaction nonce
///
/// Returns 0 on success.
#[no_mangle]
pub extern "C" fn cancel_expired_request(nonce: u64) -> u32 {
    let tx_key = bridge_tx_key(nonce);
    let tx_data = match storage_get(&tx_key) {
        Some(data) if data.len() == BRIDGE_TX_SIZE => data,
        _ => {
            log_info("Bridge transaction not found");
            return 1;
        }
    };

    if tx_data[41] != STATUS_PENDING {
        log_info("Request is not pending");
        return 2;
    }

    let created_slot = bytes_to_u64(&tx_data[42..50]);
    let current_slot = get_slot();
    let timeout = get_request_timeout();

    if current_slot <= created_slot.saturating_add(timeout) {
        log_info("Request has not expired yet");
        return 3;
    }

    // If unlock, return reserved funds to locked pool
    if tx_data[40] == 2 {
        let amount = bytes_to_u64(&tx_data[32..40]);
        let locked = storage_get(b"bridge_locked_amount")
            .map(|d| bytes_to_u64(&d))
            .unwrap_or(0);
        storage_set(b"bridge_locked_amount", &u64_to_bytes(locked.saturating_add(amount)));
    }

    update_bridge_tx_status(nonce, STATUS_EXPIRED, tx_data[50]);
    log_info("Expired request cancelled");
    0
}

// ============================================================================
// QUERY FUNCTIONS
// ============================================================================

/// Query bridge transaction status by nonce.
///
/// Parameters:
///   - nonce: the bridge transaction nonce
///
/// Returns 0 on success (tx data as return data), 1 if not found.
#[no_mangle]
pub extern "C" fn get_bridge_status(nonce: u64) -> u32 {
    let tx_key = bridge_tx_key(nonce);
    match storage_get(&tx_key) {
        Some(data) => {
            moltchain_sdk::set_return_data(&data);
            0
        }
        None => {
            log_info("Bridge transaction not found");
            1
        }
    }
}

/// Check if a specific validator has confirmed a mint request.
///
/// Parameters:
///   - validator_ptr: 32-byte validator address
///   - nonce: the mint request nonce
///
/// Returns 0 always. Return data: [1] if confirmed, [0] if not.
#[no_mangle]
pub extern "C" fn has_confirmed_mint(validator_ptr: *const u8, nonce: u64) -> u32 {
    let mut val_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(validator_ptr, val_arr.as_mut_ptr(), 32); }

    if storage_get(&mint_confirm_key(nonce, &val_arr)).is_some() {
        moltchain_sdk::set_return_data(&[1]);
    } else {
        moltchain_sdk::set_return_data(&[0]);
    }
    0
}

/// Check if a specific validator has confirmed an unlock request.
///
/// Parameters:
///   - validator_ptr: 32-byte validator address
///   - nonce: the unlock request nonce
///
/// Returns 0 always. Return data: [1] if confirmed, [0] if not.
#[no_mangle]
pub extern "C" fn has_confirmed_unlock(validator_ptr: *const u8, nonce: u64) -> u32 {
    let mut val_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(validator_ptr, val_arr.as_mut_ptr(), 32); }

    if storage_get(&unlock_confirm_key(nonce, &val_arr)).is_some() {
        moltchain_sdk::set_return_data(&[1]);
    } else {
        moltchain_sdk::set_return_data(&[0]);
    }
    0
}

/// Check if a source transaction hash has already been processed.
///
/// Parameters:
///   - tx_hash_ptr: 32-byte source transaction hash
///
/// Returns 0 always. Return data: [1] if used, [0] if not.
#[no_mangle]
pub extern "C" fn is_source_tx_used(tx_hash_ptr: *const u8) -> u32 {
    let mut hash_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(tx_hash_ptr, hash_arr.as_mut_ptr(), 32); }

    if storage_get(&source_tx_used_key(&hash_arr)).is_some() {
        moltchain_sdk::set_return_data(&[1]);
    } else {
        moltchain_sdk::set_return_data(&[0]);
    }
    0
}

/// Check if a burn proof has already been processed.
///
/// Parameters:
///   - proof_ptr: 32-byte burn proof hash
///
/// Returns 0 always. Return data: [1] if used, [0] if not.
#[no_mangle]
pub extern "C" fn is_burn_proof_used(proof_ptr: *const u8) -> u32 {
    let mut proof_arr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(proof_ptr, proof_arr.as_mut_ptr(), 32); }

    if storage_get(&burn_proof_used_key(&proof_arr)).is_some() {
        moltchain_sdk::set_return_data(&[1]);
    } else {
        moltchain_sdk::set_return_data(&[0]);
    }
    0
}

// ============================================================================
// MOLTYID IDENTITY INTEGRATION
// ============================================================================

/// Storage key for minimum reputation threshold
const MOLTYID_MIN_REP_KEY: &[u8] = b"moltyid_min_rep";
/// Storage key for MoltyID contract address (32 bytes)
const MOLTYID_ADDR_KEY: &[u8] = b"moltyid_address";

/// Set MoltyID contract address for cross-contract reputation lookups.
/// Only callable by the bridge owner.
#[no_mangle]
pub extern "C" fn set_moltyid_address(caller_ptr: *const u8, moltyid_addr_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let mut moltyid_addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(moltyid_addr_ptr, moltyid_addr.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if let Err(code) = require_owner(&caller) {
        return code;
    }

    storage_set(MOLTYID_ADDR_KEY, &moltyid_addr);
    log_info("MoltyID address configured");
    0
}

/// Set minimum MoltyID reputation required for gated functions.
/// Only callable by the bridge owner.
#[no_mangle]
pub extern "C" fn set_identity_gate(caller_ptr: *const u8, min_reputation: u64) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if let Err(code) = require_owner(&caller) {
        return code;
    }

    storage_set(MOLTYID_MIN_REP_KEY, &u64_to_bytes(min_reputation));
    log_info("Identity gate configured");
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
// PAUSE ADMIN ENDPOINTS
// ============================================================================

#[no_mangle]
pub extern "C" fn mb_pause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    let owner = storage_get(b"bridge_owner").unwrap_or_default();
    if caller[..] != owner[..] {
        return 1;
    }
    storage_set(MB_PAUSE_KEY, &[1u8]);
    log_info("Bridge paused");
    0
}

#[no_mangle]
pub extern "C" fn mb_unpause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    let owner = storage_get(b"bridge_owner").unwrap_or_default();
    if caller[..] != owner[..] {
        return 1;
    }
    storage_set(MB_PAUSE_KEY, &[0u8]);
    log_info("Bridge unpaused");
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
    use moltchain_sdk::test_mock;

    fn setup() {
        test_mock::reset();
    }

    // =============================================
    // INITIALIZATION TESTS
    // =============================================

    #[test]
    fn test_initialize() {
        setup();
        let owner = [1u8; 32];
        assert_eq!(initialize(owner.as_ptr()), 0);

        let stored = test_mock::get_storage(b"bridge_owner").unwrap();
        assert_eq!(stored.as_slice(), &owner);

        let count = test_mock::get_storage(b"bridge_validator_count").unwrap();
        assert_eq!(bytes_to_u64(&count), 0);

        let timeout = test_mock::get_storage(b"bridge_request_timeout").unwrap();
        assert_eq!(bytes_to_u64(&timeout), DEFAULT_REQUEST_TIMEOUT);
    }

    #[test]
    fn test_initialize_already_initialized() {
        setup();
        let owner = [1u8; 32];
        assert_eq!(initialize(owner.as_ptr()), 0);
        assert_eq!(initialize(owner.as_ptr()), 1);
    }

    // =============================================
    // VALIDATOR MANAGEMENT TESTS
    // =============================================

    #[test]
    fn test_add_and_remove_validator() {
        setup();
        let owner = [1u8; 32];
        initialize(owner.as_ptr());

        let validator = [2u8; 32];
        assert_eq!(add_bridge_validator(owner.as_ptr(), validator.as_ptr()), 0);

        let count = test_mock::get_storage(b"bridge_validator_count").unwrap();
        assert_eq!(bytes_to_u64(&count), 1);

        // Duplicate add fails
        assert_eq!(add_bridge_validator(owner.as_ptr(), validator.as_ptr()), 3);

        // Remove fails: would drop below required_confirmations threshold (default=2)
        assert_eq!(remove_bridge_validator(owner.as_ptr(), validator.as_ptr()), 4);

        // Lower threshold to 1, add second validator, then remove first
        set_required_confirmations(owner.as_ptr(), 1);
        let validator2 = [3u8; 32];
        assert_eq!(add_bridge_validator(owner.as_ptr(), validator2.as_ptr()), 0);
        // Now count=2, required=1 → removing one leaves 1 >= 1 → allowed
        assert_eq!(remove_bridge_validator(owner.as_ptr(), validator.as_ptr()), 0);
        let count = test_mock::get_storage(b"bridge_validator_count").unwrap();
        assert_eq!(bytes_to_u64(&count), 1);

        // Remove again fails (already removed)
        assert_eq!(remove_bridge_validator(owner.as_ptr(), validator.as_ptr()), 3);
    }

    #[test]
    fn test_add_validator_unauthorized() {
        setup();
        let owner = [1u8; 32];
        initialize(owner.as_ptr());

        let other = [9u8; 32];
        let validator = [2u8; 32];
        assert_eq!(add_bridge_validator(other.as_ptr(), validator.as_ptr()), 2);
    }

    #[test]
    fn test_add_zero_address_validator_fails() {
        setup();
        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        let zero = [0u8; 32];
        assert_eq!(add_bridge_validator(owner.as_ptr(), zero.as_ptr()), 4);
    }

    #[test]
    fn test_set_required_confirmations() {
        setup();
        let owner = [1u8; 32];
        initialize(owner.as_ptr());

        assert_eq!(set_required_confirmations(owner.as_ptr(), 3), 0);
        let stored = test_mock::get_storage(b"bridge_required_confirms").unwrap();
        assert_eq!(bytes_to_u64(&stored), 3);

        // Zero fails
        assert_eq!(set_required_confirmations(owner.as_ptr(), 0), 3);
        // >100 fails
        assert_eq!(set_required_confirmations(owner.as_ptr(), 101), 3);
    }

    #[test]
    fn test_set_request_timeout() {
        setup();
        let owner = [1u8; 32];
        initialize(owner.as_ptr());

        assert_eq!(set_request_timeout(owner.as_ptr(), 10000), 0);
        let stored = test_mock::get_storage(b"bridge_request_timeout").unwrap();
        assert_eq!(bytes_to_u64(&stored), 10000);

        // <100 fails
        assert_eq!(set_request_timeout(owner.as_ptr(), 99), 3);
    }

    // =============================================
    // LOCK TOKENS TESTS
    // =============================================

    #[test]
    fn test_lock_tokens() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());

        let sender = [3u8; 32];
        let dest_chain = [0xAA; 32];
        let dest_addr = [0xBB; 32];

        let result = lock_tokens(sender.as_ptr(), 1_000_000, dest_chain.as_ptr(), dest_addr.as_ptr());
        assert_eq!(result, 0);

        // Verify locked amount
        let locked = test_mock::get_storage(b"bridge_locked_amount").unwrap();
        assert_eq!(bytes_to_u64(&locked), 1_000_000);

        // Verify nonce incremented
        let nonce = test_mock::get_storage(b"bridge_nonce").unwrap();
        assert_eq!(bytes_to_u64(&nonce), 1);

        // Verify return data has nonce 0
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 0);

        // Verify bridge TX is COMPLETED (lock is immediate)
        let tx_data = test_mock::get_storage(&bridge_tx_key(0)).unwrap();
        assert_eq!(tx_data.len(), BRIDGE_TX_SIZE);
        assert_eq!(tx_data[40], 0); // direction = lock
        assert_eq!(tx_data[41], STATUS_COMPLETED);
    }

    #[test]
    fn test_lock_zero_amount_fails() {
        setup();
        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        let sender = [3u8; 32];
        assert_eq!(lock_tokens(sender.as_ptr(), 0, [0xAA; 32].as_ptr(), [0xBB; 32].as_ptr()), 1);
    }

    #[test]
    fn test_lock_zero_dest_address_fails() {
        setup();
        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        let sender = [3u8; 32];
        assert_eq!(lock_tokens(sender.as_ptr(), 1000, [0xAA; 32].as_ptr(), [0u8; 32].as_ptr()), 5);
    }

    // =============================================
    // SUBMIT MINT + CONFIRM MINT TESTS
    // =============================================

    #[test]
    fn test_submit_mint_auto_completes_when_threshold_is_1() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 200);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        set_required_confirmations(owner.as_ptr(), 1);

        let validator = [2u8; 32];
        add_bridge_validator(owner.as_ptr(), validator.as_ptr());

        let recipient = [4u8; 32];
        let source_chain = [0xCC; 32];
        let source_tx = [0xDD; 32];

        let result = submit_mint(
            validator.as_ptr(),
            recipient.as_ptr(),
            500_000,
            source_chain.as_ptr(),
            source_tx.as_ptr(),
        );
        assert_eq!(result, 0);

        // Verify immediately completed
        let tx_data = test_mock::get_storage(&bridge_tx_key(0)).unwrap();
        assert_eq!(tx_data[40], 1); // direction = mint
        assert_eq!(tx_data[41], STATUS_COMPLETED);
        assert_eq!(tx_data[50], 1); // 1 confirmation
    }

    #[test]
    fn test_submit_mint_multi_confirm() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 200);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        // Required = 2 (default)

        let val1 = [2u8; 32];
        let val2 = [3u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());
        add_bridge_validator(owner.as_ptr(), val2.as_ptr());

        let recipient = [4u8; 32];
        let source_chain = [0xCC; 32];
        let source_tx = [0xDD; 32];

        // Validator 1 submits (counts as 1st confirmation)
        let result = submit_mint(
            val1.as_ptr(),
            recipient.as_ptr(),
            500_000,
            source_chain.as_ptr(),
            source_tx.as_ptr(),
        );
        assert_eq!(result, 0);

        // Still pending
        let tx_data = test_mock::get_storage(&bridge_tx_key(0)).unwrap();
        assert_eq!(tx_data[41], STATUS_PENDING);
        assert_eq!(tx_data[50], 1);

        // Validator 2 confirms (reaches threshold)
        let result = confirm_mint(val2.as_ptr(), 0);
        assert_eq!(result, 0);

        // Now completed
        let tx_data = test_mock::get_storage(&bridge_tx_key(0)).unwrap();
        assert_eq!(tx_data[41], STATUS_COMPLETED);
        assert_eq!(tx_data[50], 2);
    }

    #[test]
    fn test_confirm_mint_duplicate_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 200);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        set_required_confirmations(owner.as_ptr(), 3);

        let val1 = [2u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());

        let source_tx = [0xDD; 32];
        submit_mint(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, [0xCC; 32].as_ptr(), source_tx.as_ptr());

        // Same validator tries to confirm again
        assert_eq!(confirm_mint(val1.as_ptr(), 0), 8);
    }

    #[test]
    fn test_confirm_mint_non_validator_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 200);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());

        let val1 = [2u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());

        let source_tx = [0xDD; 32];
        submit_mint(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, [0xCC; 32].as_ptr(), source_tx.as_ptr());

        let non_val = [9u8; 32];
        assert_eq!(confirm_mint(non_val.as_ptr(), 0), 2);
    }

    #[test]
    fn test_submit_mint_duplicate_source_tx_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 200);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());

        let val1 = [2u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());

        let source_tx = [0xDD; 32];
        assert_eq!(submit_mint(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, [0xCC; 32].as_ptr(), source_tx.as_ptr()), 0);

        // Same source TX hash — rejected
        assert_eq!(submit_mint(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, [0xCC; 32].as_ptr(), source_tx.as_ptr()), 4);
    }

    #[test]
    fn test_submit_mint_zero_source_tx_rejected() {
        setup();
        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        let val1 = [2u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());
        assert_eq!(submit_mint(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, [0xCC; 32].as_ptr(), [0u8; 32].as_ptr()), 5);
    }

    #[test]
    fn test_submit_mint_zero_recipient_rejected() {
        setup();
        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        let val1 = [2u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());
        assert_eq!(submit_mint(val1.as_ptr(), [0u8; 32].as_ptr(), 500_000, [0xCC; 32].as_ptr(), [0xDD; 32].as_ptr()), 6);
    }

    #[test]
    fn test_submit_mint_not_validator_rejected() {
        setup();
        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        let non_val = [9u8; 32];
        assert_eq!(submit_mint(non_val.as_ptr(), [4u8; 32].as_ptr(), 500_000, [0xCC; 32].as_ptr(), [0xDD; 32].as_ptr()), 2);
    }

    // =============================================
    // SUBMIT UNLOCK + CONFIRM UNLOCK TESTS
    // =============================================

    #[test]
    fn test_submit_unlock_multi_confirm() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());

        let val1 = [2u8; 32];
        let val2 = [3u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());
        add_bridge_validator(owner.as_ptr(), val2.as_ptr());

        // Lock tokens first
        let sender = [5u8; 32];
        lock_tokens(sender.as_ptr(), 1_000_000, [0xAA; 32].as_ptr(), [0xBB; 32].as_ptr());

        // Submit unlock
        let burn_proof = [0xEE; 32];
        let result = submit_unlock(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, burn_proof.as_ptr());
        assert_eq!(result, 0);

        // Locked amount reduced immediately (reserved)
        let locked = bytes_to_u64(&test_mock::get_storage(b"bridge_locked_amount").unwrap());
        assert_eq!(locked, 500_000);

        // Nonce = 1 (0 was lock, 1 is unlock)
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 1);

        // Still pending
        let tx_data = test_mock::get_storage(&bridge_tx_key(1)).unwrap();
        assert_eq!(tx_data[41], STATUS_PENDING);
        assert_eq!(tx_data[50], 1);

        // Validator 2 confirms
        let result = confirm_unlock(val2.as_ptr(), 1);
        assert_eq!(result, 0);

        // Now completed
        let tx_data = test_mock::get_storage(&bridge_tx_key(1)).unwrap();
        assert_eq!(tx_data[41], STATUS_COMPLETED);
        assert_eq!(tx_data[50], 2);
    }

    #[test]
    fn test_submit_unlock_insufficient_locked_fails() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        let val1 = [2u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());

        // No tokens locked
        let burn_proof = [0xEE; 32];
        assert_eq!(submit_unlock(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, burn_proof.as_ptr()), 3);
    }

    #[test]
    fn test_submit_unlock_duplicate_burn_proof_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        let val1 = [2u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());

        let sender = [5u8; 32];
        lock_tokens(sender.as_ptr(), 2_000_000, [0xAA; 32].as_ptr(), [0xBB; 32].as_ptr());

        let burn_proof = [0xEE; 32];
        assert_eq!(submit_unlock(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, burn_proof.as_ptr()), 0);

        // Same burn proof — rejected
        assert_eq!(submit_unlock(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, burn_proof.as_ptr()), 4);
    }

    #[test]
    fn test_confirm_unlock_duplicate_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        set_required_confirmations(owner.as_ptr(), 3);

        let val1 = [2u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());

        let sender = [5u8; 32];
        lock_tokens(sender.as_ptr(), 1_000_000, [0xAA; 32].as_ptr(), [0xBB; 32].as_ptr());

        let burn_proof = [0xEE; 32];
        submit_unlock(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, burn_proof.as_ptr());

        // Same validator tries again
        assert_eq!(confirm_unlock(val1.as_ptr(), 1), 8);
    }

    // =============================================
    // EXPIRY TESTS
    // =============================================

    #[test]
    fn test_confirm_mint_expired_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        set_required_confirmations(owner.as_ptr(), 3);
        set_request_timeout(owner.as_ptr(), 1000);

        let val1 = [2u8; 32];
        let val2 = [3u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());
        add_bridge_validator(owner.as_ptr(), val2.as_ptr());

        submit_mint(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, [0xCC; 32].as_ptr(), [0xDD; 32].as_ptr());

        // Advance past timeout
        test_mock::SLOT.with(|s| *s.borrow_mut() = 1200);

        // Confirm after expiry — triggers expiry
        assert_eq!(confirm_mint(val2.as_ptr(), 0), 7);

        // Verify status is expired
        let tx_data = test_mock::get_storage(&bridge_tx_key(0)).unwrap();
        assert_eq!(tx_data[41], STATUS_EXPIRED);
    }

    #[test]
    fn test_cancel_expired_unlock_returns_funds() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        set_required_confirmations(owner.as_ptr(), 3);
        set_request_timeout(owner.as_ptr(), 1000);

        let val1 = [2u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());

        // Lock tokens
        let sender = [5u8; 32];
        lock_tokens(sender.as_ptr(), 1_000_000, [0xAA; 32].as_ptr(), [0xBB; 32].as_ptr());

        // Submit unlock — reserves 500K from locked
        let burn_proof = [0xEE; 32];
        submit_unlock(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, burn_proof.as_ptr());
        let locked = bytes_to_u64(&test_mock::get_storage(b"bridge_locked_amount").unwrap());
        assert_eq!(locked, 500_000);

        // Advance past timeout
        test_mock::SLOT.with(|s| *s.borrow_mut() = 1200);

        // Cancel expired — funds should return
        assert_eq!(cancel_expired_request(1), 0);

        // Locked amount restored
        let locked = bytes_to_u64(&test_mock::get_storage(b"bridge_locked_amount").unwrap());
        assert_eq!(locked, 1_000_000);

        // Status is expired
        let tx_data = test_mock::get_storage(&bridge_tx_key(1)).unwrap();
        assert_eq!(tx_data[41], STATUS_EXPIRED);
    }

    #[test]
    fn test_cancel_not_expired_fails() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        set_required_confirmations(owner.as_ptr(), 3);

        let val1 = [2u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());

        submit_mint(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, [0xCC; 32].as_ptr(), [0xDD; 32].as_ptr());

        // Try cancel while still valid
        assert_eq!(cancel_expired_request(0), 3);
    }

    #[test]
    fn test_cancel_completed_request_fails() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        set_required_confirmations(owner.as_ptr(), 1);

        let val1 = [2u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());

        // Auto-completes
        submit_mint(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, [0xCC; 32].as_ptr(), [0xDD; 32].as_ptr());

        // Can't cancel a completed request
        test_mock::SLOT.with(|s| *s.borrow_mut() = 99999);
        assert_eq!(cancel_expired_request(0), 2);
    }

    // =============================================
    // QUERY TESTS
    // =============================================

    #[test]
    fn test_get_bridge_status() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());

        let sender = [3u8; 32];
        lock_tokens(sender.as_ptr(), 100_000, [0xAA; 32].as_ptr(), [0xBB; 32].as_ptr());

        let result = get_bridge_status(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), BRIDGE_TX_SIZE);

        // Not found
        assert_eq!(get_bridge_status(999), 1);
    }

    #[test]
    fn test_has_confirmed_mint() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 200);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());

        let val1 = [2u8; 32];
        let val2 = [3u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());
        add_bridge_validator(owner.as_ptr(), val2.as_ptr());

        submit_mint(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, [0xCC; 32].as_ptr(), [0xDD; 32].as_ptr());

        // val1 confirmed (via submit)
        has_confirmed_mint(val1.as_ptr(), 0);
        assert_eq!(test_mock::get_return_data(), vec![1]);

        // val2 not confirmed yet
        has_confirmed_mint(val2.as_ptr(), 0);
        assert_eq!(test_mock::get_return_data(), vec![0]);
    }

    #[test]
    fn test_is_source_tx_used() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 200);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        let val1 = [2u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());

        let source_tx = [0xDD; 32];
        submit_mint(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, [0xCC; 32].as_ptr(), source_tx.as_ptr());

        is_source_tx_used(source_tx.as_ptr());
        assert_eq!(test_mock::get_return_data(), vec![1]);

        let unused = [0xFF; 32];
        is_source_tx_used(unused.as_ptr());
        assert_eq!(test_mock::get_return_data(), vec![0]);
    }

    #[test]
    fn test_is_burn_proof_used() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        let val1 = [2u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());

        let sender = [5u8; 32];
        lock_tokens(sender.as_ptr(), 1_000_000, [0xAA; 32].as_ptr(), [0xBB; 32].as_ptr());

        let proof = [0xEE; 32];
        submit_unlock(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, proof.as_ptr());

        is_burn_proof_used(proof.as_ptr());
        assert_eq!(test_mock::get_return_data(), vec![1]);

        let unused = [0xFF; 32];
        is_burn_proof_used(unused.as_ptr());
        assert_eq!(test_mock::get_return_data(), vec![0]);
    }

    // =============================================
    // IDENTITY GATE TESTS
    // =============================================

    #[test]
    fn test_identity_gate_blocks_lock_tokens() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());

        // Configure identity gate
        let moltyid_addr = [0x42u8; 32];
        assert_eq!(set_moltyid_address(owner.as_ptr(), moltyid_addr.as_ptr()), 0);
        assert_eq!(set_identity_gate(owner.as_ptr(), 100), 0);

        // lock_tokens should be blocked
        let sender = [3u8; 32];
        let result = lock_tokens(sender.as_ptr(), 1_000_000, [0xAA; 32].as_ptr(), [0xBB; 32].as_ptr());
        assert_eq!(result, 10);
    }

    #[test]
    fn test_identity_gate_allows_when_disabled() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());

        // No identity gate configured
        let sender = [3u8; 32];
        let result = lock_tokens(sender.as_ptr(), 1_000_000, [0xAA; 32].as_ptr(), [0xBB; 32].as_ptr());
        assert_eq!(result, 0);
    }

    #[test]
    fn test_set_identity_gate_admin_only() {
        setup();
        let owner = [1u8; 32];
        initialize(owner.as_ptr());

        let other = [9u8; 32];
        assert_eq!(set_identity_gate(other.as_ptr(), 100), 2);
        assert_eq!(set_moltyid_address(other.as_ptr(), [0x42u8; 32].as_ptr()), 2);

        assert_eq!(set_identity_gate(owner.as_ptr(), 100), 0);
        assert_eq!(set_moltyid_address(owner.as_ptr(), [0x42u8; 32].as_ptr()), 0);
    }

    // =============================================
    // ADVERSARIAL TESTS
    // =============================================

    #[test]
    fn test_adversarial_removed_validator_cannot_confirm() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        set_required_confirmations(owner.as_ptr(), 3);

        let val1 = [2u8; 32];
        let val2 = [3u8; 32];
        let val3 = [4u8; 32];
        let val4 = [6u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());
        add_bridge_validator(owner.as_ptr(), val2.as_ptr());
        add_bridge_validator(owner.as_ptr(), val3.as_ptr());
        add_bridge_validator(owner.as_ptr(), val4.as_ptr());

        // val1 submits mint
        submit_mint(val1.as_ptr(), [5u8; 32].as_ptr(), 500_000, [0xCC; 32].as_ptr(), [0xDD; 32].as_ptr());

        // Owner removes val2 (count=4→3, required=3, 3>=3 → allowed)
        assert_eq!(remove_bridge_validator(owner.as_ptr(), val2.as_ptr()), 0);

        // val2 tries to confirm — REJECTED (no longer validator)
        assert_eq!(confirm_mint(val2.as_ptr(), 0), 2);

        // val3 can still confirm
        assert_eq!(confirm_mint(val3.as_ptr(), 0), 0);
    }

    #[test]
    fn test_adversarial_double_mint_via_different_recipients_blocked() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        set_required_confirmations(owner.as_ptr(), 1);

        let val1 = [2u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());

        let source_tx = [0xDD; 32];

        // First mint succeeds
        assert_eq!(submit_mint(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, [0xCC; 32].as_ptr(), source_tx.as_ptr()), 0);

        // Try to mint same source TX to different recipient — BLOCKED
        assert_eq!(submit_mint(val1.as_ptr(), [5u8; 32].as_ptr(), 999_999, [0xCC; 32].as_ptr(), source_tx.as_ptr()), 4);
    }

    #[test]
    fn test_adversarial_confirm_wrong_type_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        set_required_confirmations(owner.as_ptr(), 3);

        let val1 = [2u8; 32];
        let val2 = [3u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());
        add_bridge_validator(owner.as_ptr(), val2.as_ptr());

        // Submit a mint (nonce 0)
        submit_mint(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, [0xCC; 32].as_ptr(), [0xDD; 32].as_ptr());

        // Try to confirm_unlock on a mint request — REJECTED
        assert_eq!(confirm_unlock(val2.as_ptr(), 0), 5);

        // Lock then submit unlock (nonce 1, 2)
        lock_tokens([5u8; 32].as_ptr(), 1_000_000, [0xAA; 32].as_ptr(), [0xBB; 32].as_ptr());
        submit_unlock(val1.as_ptr(), [4u8; 32].as_ptr(), 500_000, [0xEE; 32].as_ptr());

        // Try to confirm_mint on an unlock request — REJECTED
        assert_eq!(confirm_mint(val2.as_ptr(), 2), 5);
    }

    #[test]
    fn test_adversarial_race_condition_unlock_reserves_funds() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        set_required_confirmations(owner.as_ptr(), 3);

        let val1 = [2u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());

        // Lock 1M
        lock_tokens([5u8; 32].as_ptr(), 1_000_000, [0xAA; 32].as_ptr(), [0xBB; 32].as_ptr());

        // Submit unlock for 700K — reserves immediately
        submit_unlock(val1.as_ptr(), [4u8; 32].as_ptr(), 700_000, [0xEE; 32].as_ptr());
        let locked = bytes_to_u64(&test_mock::get_storage(b"bridge_locked_amount").unwrap());
        assert_eq!(locked, 300_000);

        // Try to submit another unlock for 400K — FAILS (only 300K available)
        let proof2 = [0xFF; 32];
        assert_eq!(submit_unlock(val1.as_ptr(), [4u8; 32].as_ptr(), 400_000, proof2.as_ptr()), 3);

        // Submit unlock for 300K — succeeds (exactly what's left)
        assert_eq!(submit_unlock(val1.as_ptr(), [4u8; 32].as_ptr(), 300_000, proof2.as_ptr()), 0);
        let locked = bytes_to_u64(&test_mock::get_storage(b"bridge_locked_amount").unwrap());
        assert_eq!(locked, 0);
    }

    #[test]
    fn test_adversarial_expired_unlock_confirm_returns_funds() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        initialize(owner.as_ptr());
        set_required_confirmations(owner.as_ptr(), 3);
        set_request_timeout(owner.as_ptr(), 500);

        let val1 = [2u8; 32];
        let val2 = [3u8; 32];
        add_bridge_validator(owner.as_ptr(), val1.as_ptr());
        add_bridge_validator(owner.as_ptr(), val2.as_ptr());

        // Lock and submit unlock
        lock_tokens([5u8; 32].as_ptr(), 1_000_000, [0xAA; 32].as_ptr(), [0xBB; 32].as_ptr());
        submit_unlock(val1.as_ptr(), [4u8; 32].as_ptr(), 600_000, [0xEE; 32].as_ptr());
        let locked = bytes_to_u64(&test_mock::get_storage(b"bridge_locked_amount").unwrap());
        assert_eq!(locked, 400_000);

        // Advance past timeout
        test_mock::SLOT.with(|s| *s.borrow_mut() = 700);

        // Confirmation triggers expiry and fund return
        assert_eq!(confirm_unlock(val2.as_ptr(), 1), 7);
        let locked = bytes_to_u64(&test_mock::get_storage(b"bridge_locked_amount").unwrap());
        assert_eq!(locked, 1_000_000);
    }

    // ====================================================================
    // PAUSE ENFORCEMENT TESTS — All 4 validator functions blocked during pause
    // ====================================================================

    /// Helper: Initialize bridge + add validators with correct set_caller calls
    fn setup_bridge_with_validators(owner: [u8; 32], validators: &[[u8; 32]]) {
        test_mock::set_caller(owner);
        assert_eq!(initialize(owner.as_ptr()), 0);
        for v in validators {
            test_mock::set_caller(owner);
            assert_eq!(add_bridge_validator(owner.as_ptr(), v.as_ptr()), 0);
        }
    }

    #[test]
    fn test_submit_mint_blocked_when_paused() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let val = [2u8; 32];
        setup_bridge_with_validators(owner, &[val]);

        // Pause bridge
        test_mock::set_caller(owner);
        assert_eq!(mb_pause(owner.as_ptr()), 0);

        // submit_mint must be blocked
        test_mock::set_caller(val);
        let result = submit_mint(
            val.as_ptr(),
            [3u8; 32].as_ptr(),
            1_000_000,
            [0xAA; 32].as_ptr(),
            [0xBB; 32].as_ptr(),
        );
        assert_eq!(result, 20); // paused
    }

    #[test]
    fn test_confirm_mint_blocked_when_paused() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let val1 = [2u8; 32];
        let val2 = [3u8; 32];
        setup_bridge_with_validators(owner, &[val1, val2]);

        // Submit a mint request before pause
        test_mock::set_caller(val1);
        assert_eq!(submit_mint(
            val1.as_ptr(), [4u8; 32].as_ptr(), 500_000,
            [0xCC; 32].as_ptr(), [0xDD; 32].as_ptr(),
        ), 0);

        // Pause bridge
        test_mock::set_caller(owner);
        assert_eq!(mb_pause(owner.as_ptr()), 0);

        // confirm_mint must be blocked
        test_mock::set_caller(val2);
        let result = confirm_mint(val2.as_ptr(), 0);
        assert_eq!(result, 20); // paused
    }

    #[test]
    fn test_submit_unlock_blocked_when_paused() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let val = [2u8; 32];
        setup_bridge_with_validators(owner, &[val]);

        // Lock some tokens first
        let locker = [5u8; 32];
        test_mock::set_caller(locker);
        lock_tokens(locker.as_ptr(), 1_000_000, [0xAA; 32].as_ptr(), [0xBB; 32].as_ptr());

        // Pause bridge
        test_mock::set_caller(owner);
        assert_eq!(mb_pause(owner.as_ptr()), 0);

        // submit_unlock must be blocked
        test_mock::set_caller(val);
        let result = submit_unlock(
            val.as_ptr(), [6u8; 32].as_ptr(), 500_000, [0xEE; 32].as_ptr(),
        );
        assert_eq!(result, 20); // paused
    }

    #[test]
    fn test_confirm_unlock_blocked_when_paused() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let val1 = [2u8; 32];
        let val2 = [3u8; 32];
        setup_bridge_with_validators(owner, &[val1, val2]);

        // Lock + submit unlock before pause
        let locker = [5u8; 32];
        test_mock::set_caller(locker);
        lock_tokens(locker.as_ptr(), 1_000_000, [0xAA; 32].as_ptr(), [0xBB; 32].as_ptr());

        test_mock::set_caller(val1);
        assert_eq!(submit_unlock(
            val1.as_ptr(), [6u8; 32].as_ptr(), 500_000, [0xEE; 32].as_ptr(),
        ), 0);

        // Pause bridge
        test_mock::set_caller(owner);
        assert_eq!(mb_pause(owner.as_ptr()), 0);

        // confirm_unlock must be blocked
        test_mock::set_caller(val2);
        let result = confirm_unlock(val2.as_ptr(), 1);
        assert_eq!(result, 20); // paused
    }

    #[test]
    fn test_validator_ops_resume_after_unpause() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let val = [2u8; 32];
        setup_bridge_with_validators(owner, &[val]);

        // Pause, then unpause
        test_mock::set_caller(owner);
        assert_eq!(mb_pause(owner.as_ptr()), 0);
        assert_eq!(mb_unpause(owner.as_ptr()), 0);

        // submit_mint should work again
        test_mock::set_caller(val);
        let result = submit_mint(
            val.as_ptr(),
            [3u8; 32].as_ptr(),
            1_000_000,
            [0xAA; 32].as_ptr(),
            [0xBB; 32].as_ptr(),
        );
        assert_eq!(result, 0);
    }
}
