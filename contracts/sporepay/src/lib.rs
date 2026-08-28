// SporePay v2 — Streaming Payment Contract for Lichen
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

use lichen_sdk::{
    balance_of_token_or_native, bytes_to_u64, call_contract, get_caller, get_contract_address,
    get_slot, log_info, receive_token_or_native, storage_get, storage_set,
    transfer_token_or_native, u64_to_bytes, Address, CrossCall,
};

// Reentrancy guard
const CP_REENTRANCY_KEY: &[u8] = b"sp_reentrancy";

fn reentrancy_enter() -> bool {
    match storage_get(CP_REENTRANCY_KEY) {
        None => {}
        Some(value) if value.as_slice() == [0] => {}
        Some(_) => return false,
    }
    storage_set(CP_REENTRANCY_KEY, &[1u8]);
    true
}

fn reentrancy_exit() {
    storage_set(CP_REENTRANCY_KEY, &[0u8]);
}

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
const ADMIN_KEY: &[u8] = b"sp_admin";
const PAUSE_KEY: &[u8] = b"sp_paused";
const CP_TOTAL_STREAMED_KEY: &[u8] = b"sp_total_streamed";
const CP_TOTAL_WITHDRAWN_KEY: &[u8] = b"sp_total_withdrawn";
const CP_CANCEL_COUNT_KEY: &[u8] = b"sp_cancel_count";
const CP_TOKEN_ADDR_KEY: &[u8] = b"sp_token_address";
const CP_SELF_ADDR_KEY: &[u8] = b"sp_self_address";
const CP_TOTAL_ESCROW_LIABILITY_KEY: &[u8] = b"sp_escrow_liability";
const CP_TOTAL_UNPAID_KEY: &[u8] = b"sp_total_unpaid";
const CP_ACCOUNTING_VERSION_KEY: &[u8] = b"sp_account_version";
const CP_MIGRATION_LOCK_KEY: &[u8] = b"sp_account_mig_lock";
const CP_MIGRATION_EXPECTED_COUNT_KEY: &[u8] = b"sp_account_mig_expected";
const CP_MIGRATION_CURSOR_KEY: &[u8] = b"sp_account_mig_cursor";
const CP_MIGRATION_LIABILITY_KEY: &[u8] = b"sp_account_mig_liability";
const CP_MIGRATION_UNPAID_KEY: &[u8] = b"sp_account_mig_unpaid";
const ACCOUNTING_VERSION: u64 = 3;

/// Load the configured payment token contract address.
fn get_token_address() -> Option<Address> {
    storage_get(CP_TOKEN_ADDR_KEY).and_then(|d| {
        if d.len() == 32 {
            let mut addr = [0u8; 32];
            addr.copy_from_slice(&d);
            Some(Address(addr))
        } else {
            None
        }
    })
}

/// Resolve the contract's deployed address from the runtime. The immutable
/// stored value is retained as a deployment assertion and must agree whenever
/// it is present. Native unit tests use the stored value when the mock runtime
/// address is zero.
fn get_self_address() -> Option<Address> {
    let configured = storage_get(CP_SELF_ADDR_KEY).and_then(|d| {
        if d.len() == 32 {
            let mut addr = [0u8; 32];
            addr.copy_from_slice(&d);
            Some(Address(addr))
        } else {
            None
        }
    });
    let runtime = get_contract_address();
    if runtime.0 == [0u8; 32] {
        return configured;
    }
    match configured {
        Some(expected) if expected != runtime => None,
        _ => Some(runtime),
    }
}

fn cliff_key(stream_id: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(6 + 20);
    key.extend_from_slice(b"cliff_");
    key.extend_from_slice(&u64_to_decimal(stream_id));
    key
}

const SENDER_INDEX_PREFIX: &[u8] = b"sp_sender_idx:";
const RECIPIENT_INDEX_PREFIX: &[u8] = b"sp_recipient_idx:";
const MAX_INDEX_PAGE: u64 = 64;

struct IndexAppend {
    count_key: Vec<u8>,
    item_key: Vec<u8>,
    next_count: u64,
}

fn address_index_count_key(prefix: &[u8], address: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(prefix.len() + 32 + 6);
    key.extend_from_slice(prefix);
    key.extend_from_slice(address);
    key.extend_from_slice(b":count");
    key
}

fn address_index_item_key(prefix: &[u8], address: &[u8; 32], index: u64) -> Vec<u8> {
    let mut key = Vec::with_capacity(prefix.len() + 32 + 1 + 20);
    key.extend_from_slice(prefix);
    key.extend_from_slice(address);
    key.push(b':');
    key.extend_from_slice(&u64_to_decimal(index));
    key
}

fn prepare_index_append(prefix: &[u8], address: &[u8; 32]) -> Option<IndexAppend> {
    let count_key = address_index_count_key(prefix, address);
    let count = checked_stored_u64(&count_key)?;
    Some(IndexAppend {
        item_key: address_index_item_key(prefix, address, count),
        count_key,
        next_count: count.checked_add(1)?,
    })
}

fn apply_index_append(append: &IndexAppend, stream_id: u64) {
    storage_set(&append.item_key, &u64_to_bytes(stream_id));
    storage_set(&append.count_key, &u64_to_bytes(append.next_count));
}

fn get_address_stream_ids(
    prefix: &[u8],
    address_ptr: *const u8,
    cursor: u64,
    limit: u64,
) -> u32 {
    let address = match read_address32(address_ptr) {
        Some(value) => value,
        None => return 40,
    };
    if limit == 0 || limit > MAX_INDEX_PAGE {
        return 3;
    }
    let count_key = address_index_count_key(prefix, &address);
    let count = match checked_stored_u64(&count_key) {
        Some(value) => value,
        None => return 4,
    };
    if cursor > count {
        return 2;
    }
    let end = core::cmp::min(count, match cursor.checked_add(limit) {
        Some(value) => value,
        None => count,
    });
    let returned = end - cursor;
    let mut result = Vec::with_capacity(24 + returned as usize * 8);
    result.extend_from_slice(&u64_to_bytes(count));
    result.extend_from_slice(&u64_to_bytes(end));
    result.extend_from_slice(&u64_to_bytes(returned));
    for index in cursor..end {
        let key = address_index_item_key(prefix, &address, index);
        let stream_id = match checked_stored_u64(&key) {
            Some(value) => value,
            None => return 4,
        };
        result.extend_from_slice(&u64_to_bytes(stream_id));
    }
    lichen_sdk::set_return_data(&result);
    0
}

fn is_paused() -> bool {
    storage_get(PAUSE_KEY)
        .map(|v| v.as_slice() == [1])
        .unwrap_or(false)
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

fn checked_add_stored(key: &[u8], amount: u64) -> Option<u64> {
    checked_stored_u64(key)?.checked_add(amount)
}

fn accounting_version() -> u64 {
    stored_u64(CP_ACCOUNTING_VERSION_KEY)
}

fn migration_locked() -> bool {
    storage_get(CP_MIGRATION_LOCK_KEY)
        .map(|value| value.as_slice() == [1])
        .unwrap_or(false)
}

fn accounting_operational() -> bool {
    accounting_version() == ACCOUNTING_VERSION && !migration_locked()
}

fn migrated_unpaid_recipient_key(recipient: &[u8; 32]) -> Vec<u8> {
    let mut key = b"sp_account_mig_recipient:".to_vec();
    key.extend_from_slice(recipient);
    key
}

fn remove_storage_key(key: &[u8]) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        extern "C" {
            fn storage_delete(key_ptr: *const u8, key_len: u32) -> u32;
        }
        unsafe { storage_delete(key.as_ptr(), key.len() as u32) == 1 }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        lichen_sdk::storage::remove(key)
    }
}

fn restore_storage_value(key: &[u8], value: &Option<Vec<u8>>) {
    match value {
        Some(bytes) => {
            storage_set(key, bytes);
        }
        None => {
            remove_storage_key(key);
        }
    }
}

fn unpaid_payout_key(token: Address, recipient: Address) -> Vec<u8> {
    let mut key = b"unpaid_payout:".to_vec();
    key.extend_from_slice(&token.0);
    key.push(b':');
    key.extend_from_slice(&recipient.0);
    key
}

fn next_unpaid_payout(token: Address, recipient: Address, amount: u64) -> Option<(Vec<u8>, u64)> {
    let key = unpaid_payout_key(token, recipient);
    let current = checked_stored_u64(&key)?;
    Some((key, current.checked_add(amount)?))
}

fn transfer_from_escrow(token: Address, self_addr: Address, to: Address, amount: u64) -> bool {
    amount == 0
        || matches!(
            transfer_token_or_native(token, self_addr, to, amount),
            Ok(true)
        )
}

fn receive_into_escrow(token: Address, from: Address, self_addr: Address, amount: u64) -> bool {
    amount > 0
        && matches!(
            receive_token_or_native(token, from, self_addr, amount),
            Ok(true)
        )
}

fn next_stream_id() -> Option<(u64, u64)> {
    let stream_id = checked_stored_u64(b"stream_count")?;
    stream_id.checked_add(1).map(|next| (stream_id, next))
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

#[derive(Clone, Copy)]
struct StreamRecord {
    sender: [u8; 32],
    recipient: [u8; 32],
    total_amount: u64,
    withdrawn: u64,
    start_slot: u64,
    end_slot: u64,
    cancelled: bool,
    created_slot: u64,
}

impl StreamRecord {
    fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < STREAM_SIZE {
            return None;
        }
        let mut sender = [0u8; 32];
        sender.copy_from_slice(&data[0..32]);
        let mut recipient = [0u8; 32];
        recipient.copy_from_slice(&data[32..64]);
        Some(Self {
            sender,
            recipient,
            total_amount: bytes_to_u64(&data[64..72]),
            withdrawn: bytes_to_u64(&data[72..80]),
            start_slot: bytes_to_u64(&data[80..88]),
            end_slot: bytes_to_u64(&data[88..96]),
            cancelled: data[96] == 1,
            created_slot: bytes_to_u64(&data[97..105]),
        })
    }

    fn encode(&self) -> Vec<u8> {
        let mut data = Vec::with_capacity(STREAM_SIZE);
        data.extend_from_slice(&self.sender);
        data.extend_from_slice(&self.recipient);
        data.extend_from_slice(&u64_to_bytes(self.total_amount));
        data.extend_from_slice(&u64_to_bytes(self.withdrawn));
        data.extend_from_slice(&u64_to_bytes(self.start_slot));
        data.extend_from_slice(&u64_to_bytes(self.end_slot));
        data.push(u8::from(self.cancelled));
        data.extend_from_slice(&u64_to_bytes(self.created_slot));
        data
    }

    fn outstanding(&self) -> Option<u64> {
        self.total_amount.checked_sub(self.withdrawn)
    }
}

/// Calculate the currently withdrawable amount for a stream.
/// v2: cliff_slot support — nothing withdrawable until cliff passes.
fn calculate_vested(
    total_amount: u64,
    start_slot: u64,
    end_slot: u64,
    current_slot: u64,
    cliff_slot: u64,
) -> u64 {
    if current_slot < start_slot {
        return 0;
    }

    // A cliff is a true vesting boundary: cancelling before it does not create
    // a payout that the recipient could not have withdrawn.
    if cliff_slot > 0 && current_slot < cliff_slot {
        return 0;
    }

    let duration = end_slot.saturating_sub(start_slot);
    if duration == 0 {
        return total_amount;
    }

    let elapsed = if current_slot >= end_slot {
        duration
    } else {
        current_slot.saturating_sub(start_slot)
    };

    // streamed = total_amount * elapsed / duration
    ((total_amount as u128) * (elapsed as u128) / (duration as u128)) as u64
}

fn calculate_withdrawable(
    total_amount: u64,
    withdrawn: u64,
    start_slot: u64,
    end_slot: u64,
    current_slot: u64,
    cancelled: bool,
    cliff_slot: u64,
) -> u64 {
    if cancelled {
        return 0;
    }
    calculate_vested(total_amount, start_slot, end_slot, current_slot, cliff_slot)
        .saturating_sub(withdrawn)
}

// ============================================================================
// CREATE STREAM
// ============================================================================

/// Create a payment stream.
///
/// Parameters:
///   - sender_ptr: 32-byte sender address
///   - recipient_ptr: 32-byte recipient address
///   - total_amount: total spores to stream
///   - start_slot: slot when streaming begins
///   - end_slot: slot when streaming ends
///
/// Returns 0 on success, stream_id in return data.
///
/// Error codes:
///   1  = zero amount
///   2  = end_slot <= start_slot
///   3  = sender == recipient
///   10 = sender lacks LichenID reputation
///   11 = recipient lacks LichenID reputation
///   20 = protocol paused / reentrancy
///   30 = token address not configured
///   31 = contract self-address not configured
///   32 = escrow transfer failed (insufficient balance/approval)
///   200 = caller spoofing
#[no_mangle]
pub extern "C" fn create_stream(
    sender_ptr: *const u8,
    recipient_ptr: *const u8,
    total_amount: u64,
    start_slot: u64,
    end_slot: u64,
) -> u32 {
    if !reentrancy_enter() {
        return 20;
    }
    if !accounting_operational() {
        reentrancy_exit();
        return 95;
    }
    log_info("Creating payment stream...");

    let sender = match read_address32(sender_ptr) {
        Some(v) => v,
        None => {
            reentrancy_exit();
            return 40;
        }
    };
    let recipient = match read_address32(recipient_ptr) {
        Some(v) => v,
        None => {
            reentrancy_exit();
            return 40;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != sender {
        reentrancy_exit();
        return 200;
    }

    if is_paused() {
        log_info("Protocol is paused");
        reentrancy_exit();
        return 20;
    }

    if total_amount == 0 {
        log_info("Amount must be > 0");
        reentrancy_exit();
        return 1;
    }

    if end_slot <= start_slot {
        log_info("End slot must be after start slot");
        reentrancy_exit();
        return 2;
    }

    if sender == recipient {
        log_info("Sender and recipient must differ");
        reentrancy_exit();
        return 3;
    }
    if sender == [0u8; 32] || recipient == [0u8; 32] {
        log_info("Sender and recipient must be nonzero addresses");
        reentrancy_exit();
        return 4;
    }

    // LichenID identity gate — both sender and recipient must have identity
    if !check_identity_gate(&sender) {
        log_info("Sender lacks required LichenID reputation");
        reentrancy_exit();
        return 10;
    }
    if !check_identity_gate(&recipient) {
        log_info("Recipient lacks required LichenID reputation");
        reentrancy_exit();
        return 11;
    }

    let (stream_id, next_stream_id) = match next_stream_id() {
        Some(ids) => ids,
        None => {
            log_info("Stream count overflow");
            reentrancy_exit();
            return 34;
        }
    };
    let next_total_streamed = match checked_add_stored(CP_TOTAL_STREAMED_KEY, total_amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    let next_liability = match checked_add_stored(CP_TOTAL_ESCROW_LIABILITY_KEY, total_amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    let sender_index = match prepare_index_append(SENDER_INDEX_PREFIX, &sender) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    let recipient_index = match prepare_index_append(RECIPIENT_INDEX_PREFIX, &recipient) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };

    // ── ESCROW: Lock tokens from sender into contract custody ───────────
    let token_addr = match get_token_address() {
        Some(addr) => addr,
        None => {
            log_info("Token address not configured — cannot escrow");
            reentrancy_exit();
            return 30;
        }
    };
    let self_addr = match get_self_address() {
        Some(addr) => addr,
        None => {
            log_info("Contract self-address not configured");
            reentrancy_exit();
            return 31;
        }
    };

    if !receive_into_escrow(token_addr, Address(sender), self_addr, total_amount) {
        log_info("Escrow transfer failed — sender lacks balance or approval");
        reentrancy_exit();
        return 32;
    }
    // ── END ESCROW ──────────────────────────────────────────────────────

    storage_set(b"stream_count", &u64_to_bytes(next_stream_id));

    let current_slot = get_slot();
    let data = StreamRecord {
        sender,
        recipient,
        total_amount,
        withdrawn: 0,
        start_slot,
        end_slot,
        cancelled: false,
        created_slot: current_slot,
    }
    .encode();

    let sk = stream_key(stream_id);
    storage_set(&sk, &data);
    apply_index_append(&sender_index, stream_id);
    apply_index_append(&recipient_index, stream_id);

    storage_set(CP_TOTAL_STREAMED_KEY, &u64_to_bytes(next_total_streamed));
    storage_set(CP_TOTAL_ESCROW_LIABILITY_KEY, &u64_to_bytes(next_liability));

    lichen_sdk::set_return_data(&u64_to_bytes(stream_id));
    log_info("Payment stream created");
    reentrancy_exit();
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
///
/// Error codes:
///   1  = zero amount
///   2  = stream not found
///   3  = bad stream data
///   4  = caller is not recipient
///   5  = stream cancelled
///   6  = amount exceeds withdrawable
///   20 = reentrancy guard
///   30 = token address missing
///   31 = self-address missing
///   32 = token transfer failed
///   200 = caller spoofing
#[no_mangle]
pub extern "C" fn withdraw_from_stream(caller_ptr: *const u8, stream_id: u64, amount: u64) -> u32 {
    if !reentrancy_enter() {
        return 20;
    }
    if !accounting_operational() {
        reentrancy_exit();
        return 95;
    }
    log_info("Withdrawing from stream...");

    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => {
            reentrancy_exit();
            return 40;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    if amount == 0 {
        log_info("Amount must be > 0");
        reentrancy_exit();
        return 1;
    }

    let sk = stream_key(stream_id);
    let mut stream_data = match storage_get(&sk) {
        Some(data) => data,
        None => {
            log_info("Stream not found");
            reentrancy_exit();
            return 2;
        }
    };

    if stream_data.len() < STREAM_SIZE {
        reentrancy_exit();
        return 3;
    }

    // Verify caller is recipient
    if stream_data[32..64] != caller[..] {
        log_info("Only recipient can withdraw");
        reentrancy_exit();
        return 4;
    }

    if stream_data[96] == 1 {
        log_info("Stream is cancelled");
        reentrancy_exit();
        return 5;
    }

    let total_amount = bytes_to_u64(&stream_data[64..72]);
    let withdrawn = bytes_to_u64(&stream_data[72..80]);
    let start_slot = bytes_to_u64(&stream_data[80..88]);
    let end_slot = bytes_to_u64(&stream_data[88..96]);
    let current_slot = get_slot();

    let cliff = get_cliff(stream_id);
    let withdrawable = calculate_withdrawable(
        total_amount,
        withdrawn,
        start_slot,
        end_slot,
        current_slot,
        false,
        cliff,
    );

    if amount > withdrawable {
        log_info("Amount exceeds withdrawable balance");
        reentrancy_exit();
        return 6;
    }

    let new_withdrawn = match withdrawn.checked_add(amount) {
        Some(next) if next <= total_amount => next,
        _ => {
            log_info("Withdrawn amount overflow");
            reentrancy_exit();
            return 7;
        }
    };
    let total_withdrawn_before = match checked_stored_u64(CP_TOTAL_WITHDRAWN_KEY) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    let total_withdrawn_after = match total_withdrawn_before.checked_add(amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    let liability_before = match checked_stored_u64(CP_TOTAL_ESCROW_LIABILITY_KEY) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    let liability_after = match liability_before.checked_sub(amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };

    // ── DISBURSE: Transfer tokens from contract to recipient ────────────
    let token_addr = match get_token_address() {
        Some(addr) => addr,
        None => {
            log_info("Token address not configured");
            reentrancy_exit();
            return 30;
        }
    };
    let self_addr = match get_self_address() {
        Some(addr) => addr,
        None => {
            log_info("Contract self-address not configured");
            reentrancy_exit();
            return 31;
        }
    };

    let mut recipient_addr = [0u8; 32];
    recipient_addr.copy_from_slice(&stream_data[32..64]);

    // Checks-effects-interactions. The explicit restoration below keeps native
    // tests and non-atomic hosts safe; the Lichen runtime also reverts nested
    // call state atomically when the outer execution fails.
    stream_data[72..80].copy_from_slice(&u64_to_bytes(new_withdrawn));
    storage_set(&sk, &stream_data);
    storage_set(CP_TOTAL_WITHDRAWN_KEY, &u64_to_bytes(total_withdrawn_after));
    storage_set(
        CP_TOTAL_ESCROW_LIABILITY_KEY,
        &u64_to_bytes(liability_after),
    );

    if !transfer_from_escrow(token_addr, self_addr, Address(recipient_addr), amount) {
        stream_data[72..80].copy_from_slice(&u64_to_bytes(withdrawn));
        storage_set(&sk, &stream_data);
        storage_set(
            CP_TOTAL_WITHDRAWN_KEY,
            &u64_to_bytes(total_withdrawn_before),
        );
        storage_set(
            CP_TOTAL_ESCROW_LIABILITY_KEY,
            &u64_to_bytes(liability_before),
        );
        log_info("Token transfer to recipient failed");
        reentrancy_exit();
        return 32;
    }
    // ── END DISBURSE ────────────────────────────────────────────────────

    lichen_sdk::set_return_data(&u64_to_bytes(amount));
    log_info("Withdrawal successful");

    reentrancy_exit();
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
///
/// Error codes:
///   1  = stream not found
///   2  = bad stream data
///   3  = caller is not sender
///   4  = already cancelled
///   20 = reentrancy guard
///   30 = token address missing
///   31 = self-address missing
///   32 = refund-to-sender transfer failed
///   33 = transfer-to-recipient failed
///   200 = caller spoofing
#[no_mangle]
pub extern "C" fn cancel_stream(caller_ptr: *const u8, stream_id: u64) -> u32 {
    if !reentrancy_enter() {
        return 20;
    }
    if !accounting_operational() {
        reentrancy_exit();
        return 95;
    }
    log_info("Cancelling payment stream...");

    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => {
            reentrancy_exit();
            return 40;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    let sk = stream_key(stream_id);
    let stream_data = match storage_get(&sk) {
        Some(data) => data,
        None => {
            log_info("Stream not found");
            reentrancy_exit();
            return 1;
        }
    };

    let mut stream = match StreamRecord::decode(&stream_data) {
        Some(stream) => stream,
        None => {
            reentrancy_exit();
            return 2;
        }
    };

    // Verify caller is sender
    if stream.sender != caller {
        log_info("Only sender can cancel");
        reentrancy_exit();
        return 3;
    }

    if stream.cancelled {
        log_info("Stream already cancelled");
        reentrancy_exit();
        return 4;
    }

    let current_slot = get_slot();
    let cliff = get_cliff(stream_id);
    let vested = calculate_vested(
        stream.total_amount,
        stream.start_slot,
        stream.end_slot,
        current_slot,
        cliff,
    );
    let refund = match stream.total_amount.checked_sub(vested) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    let recipient_due = match vested.checked_sub(stream.withdrawn) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    let outstanding = match stream.outstanding() {
        Some(value) if refund.checked_add(recipient_due) == Some(value) => value,
        _ => {
            reentrancy_exit();
            return 35;
        }
    };
    let cancel_count_after = match checked_add_stored(CP_CANCEL_COUNT_KEY, 1) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    let liability_before = match checked_stored_u64(CP_TOTAL_ESCROW_LIABILITY_KEY) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    if liability_before < outstanding {
        reentrancy_exit();
        return 35;
    }

    // ── ESCROW SETTLEMENT: Transfer refund to sender, streamed to recipient ─
    let token_addr = match get_token_address() {
        Some(addr) => addr,
        None => {
            log_info("Token address not configured — cancellation fails closed");
            reentrancy_exit();
            return 30;
        }
    };
    let self_addr = match get_self_address() {
        Some(addr) => addr,
        None => {
            log_info("Contract self-address assertion missing or mismatched");
            reentrancy_exit();
            return 31;
        }
    };

    let unpaid_before = match checked_stored_u64(CP_TOTAL_UNPAID_KEY) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    let (unpaid_key, recipient_unpaid_after) =
        match next_unpaid_payout(token_addr, Address(stream.recipient), recipient_due) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 35;
            }
        };
    let total_unpaid_after = match unpaid_before.checked_add(recipient_due) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };

    // Refund unstreamed amount to sender
    if refund > 0 && !transfer_from_escrow(token_addr, self_addr, Address(stream.sender), refund) {
        log_info("Refund to sender failed");
        reentrancy_exit();
        return 32;
    }

    // Transfer already-streamed (minus withdrawn) to recipient
    let recipient_paid = transfer_from_escrow(
        token_addr,
        self_addr,
        Address(stream.recipient),
        recipient_due,
    );
    // ── END ESCROW SETTLEMENT ───────────────────────────────────────────

    let paid_out = if recipient_paid {
        outstanding
    } else {
        if recipient_due > 0 {
            storage_set(&unpaid_key, &u64_to_bytes(recipient_unpaid_after));
            storage_set(CP_TOTAL_UNPAID_KEY, &u64_to_bytes(total_unpaid_after));
            log_info("Recipient payout deferred for explicit recovery");
        }
        refund
    };
    let liability_after = match liability_before.checked_sub(paid_out) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };

    stream.cancelled = true;
    storage_set(&sk, &stream.encode());
    storage_set(CP_CANCEL_COUNT_KEY, &u64_to_bytes(cancel_count_after));
    storage_set(
        CP_TOTAL_ESCROW_LIABILITY_KEY,
        &u64_to_bytes(liability_after),
    );

    lichen_sdk::set_return_data(&u64_to_bytes(refund));
    log_info("Stream cancelled");

    reentrancy_exit();
    0
}

// ============================================================================
// UNPAID PAYOUT RECOVERY
// ============================================================================

/// Claim a recipient payout that could not be delivered during cancel_stream.
/// This is intentionally allowed while paused so recipients can exit once their
/// account or token transfer restrictions are lifted.
///
/// Returns: 0 success, 2 nothing owed, 20 reentrancy, 30/31 escrow config missing,
///          32 transfer failed, 34 storage mutation failed, 200 caller spoofing.
#[no_mangle]
pub extern "C" fn claim_unpaid_payout(caller_ptr: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 20;
    }
    if !accounting_operational() {
        reentrancy_exit();
        return 95;
    }

    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => {
            reentrancy_exit();
            return 40;
        }
    };

    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    let token_addr = match get_token_address() {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 30;
        }
    };
    let self_addr = match get_self_address() {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 31;
        }
    };

    let recipient = Address(caller);
    let key = unpaid_payout_key(token_addr, recipient);
    let unpaid_before = storage_get(&key);
    let amount = match unpaid_before.as_ref() {
        Some(data) if data.len() == 8 => bytes_to_u64(data),
        Some(_) => {
            reentrancy_exit();
            return 35;
        }
        None => 0,
    };
    if amount == 0 {
        reentrancy_exit();
        return 2;
    }
    let liability_before = match checked_stored_u64(CP_TOTAL_ESCROW_LIABILITY_KEY) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    let liability_after = match liability_before.checked_sub(amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    let total_unpaid_before = match checked_stored_u64(CP_TOTAL_UNPAID_KEY) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    let total_unpaid_after = match total_unpaid_before.checked_sub(amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };

    if !remove_storage_key(&key) {
        reentrancy_exit();
        return 34;
    }
    storage_set(
        CP_TOTAL_ESCROW_LIABILITY_KEY,
        &u64_to_bytes(liability_after),
    );
    storage_set(CP_TOTAL_UNPAID_KEY, &u64_to_bytes(total_unpaid_after));

    if !transfer_from_escrow(token_addr, self_addr, recipient, amount) {
        restore_storage_value(&key, &unpaid_before);
        storage_set(
            CP_TOTAL_ESCROW_LIABILITY_KEY,
            &u64_to_bytes(liability_before),
        );
        storage_set(CP_TOTAL_UNPAID_KEY, &u64_to_bytes(total_unpaid_before));
        reentrancy_exit();
        return 32;
    }

    lichen_sdk::set_return_data(&u64_to_bytes(amount));
    reentrancy_exit();
    0
}

/// Query the currently recoverable unpaid payout for a recipient.
///
/// Returns: 0 success, 30 token config missing, 40 invalid pointer.
#[no_mangle]
pub extern "C" fn get_unpaid_payout(recipient_ptr: *const u8) -> u32 {
    let recipient = match read_address32(recipient_ptr) {
        Some(v) => v,
        None => return 40,
    };

    let token_addr = match get_token_address() {
        Some(addr) => addr,
        None => return 30,
    };

    let key = unpaid_payout_key(token_addr, Address(recipient));
    lichen_sdk::set_return_data(&u64_to_bytes(stored_u64(&key)));
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
            lichen_sdk::set_return_data(&data);
            0
        }
        None => {
            log_info("Stream not found");
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
            log_info("Stream not found");
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
        total_amount,
        withdrawn,
        start_slot,
        end_slot,
        current_slot,
        cancelled,
        cliff,
    );

    lichen_sdk::set_return_data(&u64_to_bytes(withdrawable));
    0
}

// ============================================================================
// V2: CLIFF STREAMS, TRANSFER, ADMIN
// ============================================================================

/// Create a stream with a cliff period.
/// No tokens vest until cliff_slot is reached; then linear vesting begins.
///
/// Returns: 0 success, 1 bad params, 2 cliff before start, 3 cliff after end,
///          10/11 identity gated, 20 paused/reentrancy, 30/31/32 escrow errors
#[no_mangle]
pub extern "C" fn create_stream_with_cliff(
    sender_ptr: *const u8,
    recipient_ptr: *const u8,
    total_amount: u64,
    start_slot: u64,
    end_slot: u64,
    cliff_slot: u64,
) -> u32 {
    if !reentrancy_enter() {
        return 20;
    }
    if !accounting_operational() {
        reentrancy_exit();
        return 95;
    }

    if is_paused() {
        reentrancy_exit();
        return 20;
    }

    let sender = match read_address32(sender_ptr) {
        Some(v) => v,
        None => {
            reentrancy_exit();
            return 40;
        }
    };
    let recipient = match read_address32(recipient_ptr) {
        Some(v) => v,
        None => {
            reentrancy_exit();
            return 40;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != sender {
        reentrancy_exit();
        return 200;
    }

    if total_amount == 0 || start_slot >= end_slot {
        reentrancy_exit();
        return 1;
    }
    if sender == recipient || sender == [0u8; 32] || recipient == [0u8; 32] {
        reentrancy_exit();
        return 4;
    }
    if cliff_slot < start_slot {
        reentrancy_exit();
        return 2;
    }
    if cliff_slot > end_slot {
        reentrancy_exit();
        return 3;
    }

    // Identity gate
    if !check_identity_gate(&sender) {
        reentrancy_exit();
        return 10;
    }
    if !check_identity_gate(&recipient) {
        reentrancy_exit();
        return 11;
    }

    let (stream_id, next_stream_id) = match next_stream_id() {
        Some(ids) => ids,
        None => {
            log_info("Stream count overflow");
            reentrancy_exit();
            return 34;
        }
    };
    let next_total_streamed = match checked_add_stored(CP_TOTAL_STREAMED_KEY, total_amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    let next_liability = match checked_add_stored(CP_TOTAL_ESCROW_LIABILITY_KEY, total_amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    let sender_index = match prepare_index_append(SENDER_INDEX_PREFIX, &sender) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };
    let recipient_index = match prepare_index_append(RECIPIENT_INDEX_PREFIX, &recipient) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };

    // ── ESCROW: Lock tokens from sender into contract custody ───────────
    let token_addr = match get_token_address() {
        Some(addr) => addr,
        None => {
            log_info("Token address not configured — cannot escrow");
            reentrancy_exit();
            return 30;
        }
    };
    let self_addr = match get_self_address() {
        Some(addr) => addr,
        None => {
            log_info("Contract self-address not configured");
            reentrancy_exit();
            return 31;
        }
    };

    if !receive_into_escrow(token_addr, Address(sender), self_addr, total_amount) {
        log_info("Escrow transfer failed — sender lacks balance or approval");
        reentrancy_exit();
        return 32;
    }
    // ── END ESCROW ──────────────────────────────────────────────────────

    storage_set(b"stream_count", &u64_to_bytes(next_stream_id));

    let current_slot = get_slot();
    let stream = StreamRecord {
        sender,
        recipient,
        total_amount,
        withdrawn: 0,
        start_slot,
        end_slot,
        cancelled: false,
        created_slot: current_slot,
    }
    .encode();

    let sk = stream_key(stream_id);
    storage_set(&sk, &stream);
    apply_index_append(&sender_index, stream_id);
    apply_index_append(&recipient_index, stream_id);

    // Store cliff
    let ck = cliff_key(stream_id);
    storage_set(&ck, &u64_to_bytes(cliff_slot));

    storage_set(CP_TOTAL_STREAMED_KEY, &u64_to_bytes(next_total_streamed));
    storage_set(CP_TOTAL_ESCROW_LIABILITY_KEY, &u64_to_bytes(next_liability));

    lichen_sdk::set_return_data(&u64_to_bytes(stream_id));
    log_info("Stream created with cliff");
    reentrancy_exit();
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
    if !reentrancy_enter() {
        return 20;
    }
    if !accounting_operational() || is_paused() {
        reentrancy_exit();
        return 20;
    }

    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => {
            reentrancy_exit();
            return 1;
        }
    };
    let new_recipient = match read_address32(new_recipient_ptr) {
        Some(v) => v,
        None => {
            reentrancy_exit();
            return 1;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }
    if new_recipient == [0u8; 32] || new_recipient == caller {
        reentrancy_exit();
        return 5;
    }
    let sk = stream_key(stream_id);
    let mut stream_data = match storage_get(&sk) {
        Some(data) => data,
        None => {
            reentrancy_exit();
            return 1;
        }
    };
    if stream_data.len() < STREAM_SIZE {
        reentrancy_exit();
        return 1;
    }

    // Only current recipient can transfer
    if caller[..] != stream_data[32..64] {
        reentrancy_exit();
        return 2;
    }

    // Cannot transfer cancelled stream
    if stream_data[96] == 1 {
        reentrancy_exit();
        return 3;
    }

    // Cannot transfer fully withdrawn stream
    let total = bytes_to_u64(&stream_data[64..72]);
    let withdrawn = bytes_to_u64(&stream_data[72..80]);
    if withdrawn >= total {
        reentrancy_exit();
        return 4;
    }
    if !check_identity_gate(&new_recipient) {
        reentrancy_exit();
        return 11;
    }
    let recipient_index = match prepare_index_append(RECIPIENT_INDEX_PREFIX, &new_recipient) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 35;
        }
    };

    // Update recipient
    stream_data[32..64].copy_from_slice(&new_recipient);
    storage_set(&sk, &stream_data);
    apply_index_append(&recipient_index, stream_id);

    log_info("Stream transferred to new recipient");
    reentrancy_exit();
    0
}

/// Initialize SporePay admin. Only callable once.
/// Returns: 0 success, 1 already set
#[no_mangle]
pub extern "C" fn initialize_cp_admin(admin_ptr: *const u8) -> u32 {
    let admin = match read_address32(admin_ptr) {
        Some(v) => v,
        None => return 40,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != admin {
        return 200;
    }
    if admin == [0u8; 32] {
        return 2;
    }

    if storage_get(ADMIN_KEY).is_some() {
        return 1;
    }
    storage_set(ADMIN_KEY, &admin);
    storage_set(IDENTITY_ADMIN_KEY, &admin);
    if checked_stored_u64(b"stream_count") == Some(0) {
        storage_set(CP_ACCOUNTING_VERSION_KEY, &u64_to_bytes(ACCOUNTING_VERSION));
        storage_set(CP_TOTAL_ESCROW_LIABILITY_KEY, &u64_to_bytes(0));
        storage_set(CP_TOTAL_UNPAID_KEY, &u64_to_bytes(0));
    }
    log_info("SporePay admin initialized");
    0
}

/// Set the payment token contract address for escrow operations.
/// Only callable by admin and only configurable once.
///
/// Returns: 0 success, 1 not admin, 2 already configured, 200 caller spoof
#[no_mangle]
pub extern "C" fn set_token_address(caller_ptr: *const u8, token_addr_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => return 40,
    };
    let token_addr = match read_address32(token_addr_ptr) {
        Some(v) => v,
        None => return 40,
    };

    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cp_admin(&caller) {
        return 1;
    }

    if load_configured_address(CP_TOKEN_ADDR_KEY).is_some() {
        return 2;
    }

    // NOTE: zero address [0;32] is allowed — it is the native LICN sentinel
    storage_set(CP_TOKEN_ADDR_KEY, &token_addr);
    log_info("Token address configured");
    0
}

/// Set the contract's own deployed address for escrow transfers.
/// Only callable by admin. Cannot be set to the zero address and is immutable once set.
///
/// Returns: 0 success, 1 not admin, 2 zero address, 3 already configured, 200 caller spoof
#[no_mangle]
pub extern "C" fn set_self_address(caller_ptr: *const u8, self_addr_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => return 40,
    };
    let self_addr = match read_address32(self_addr_ptr) {
        Some(v) => v,
        None => return 40,
    };

    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cp_admin(&caller) {
        return 1;
    }

    if self_addr == [0u8; 32] {
        return 2;
    }

    let runtime = get_contract_address();
    if runtime.0 != [0u8; 32] && runtime.0 != self_addr {
        return 4;
    }

    if load_configured_address(CP_SELF_ADDR_KEY).is_some() {
        return 3;
    }

    storage_set(CP_SELF_ADDR_KEY, &self_addr);
    log_info("Contract self-address configured");
    0
}

/// Pause the protocol. Only admin.
/// Returns: 0 success, 1 not admin, 2 already paused
#[no_mangle]
pub extern "C" fn pause(caller_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => return 40,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cp_admin(&caller) {
        return 1;
    }
    if is_paused() {
        return 2;
    }
    storage_set(PAUSE_KEY, &[1]);
    log_info("SporePay paused");
    0
}

/// Unpause the protocol. Only admin.
/// Returns: 0 success, 1 not admin, 2 not paused
#[no_mangle]
pub extern "C" fn unpause(caller_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => return 40,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_cp_admin(&caller) {
        return 1;
    }
    if !accounting_operational() {
        return 95;
    }
    if !is_paused() {
        return 2;
    }
    storage_set(PAUSE_KEY, &[0]);
    log_info("SporePay unpaused");
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
    lichen_sdk::set_return_data(&info);
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

/// Set the admin for identity/reputation configuration on a legacy deployment
/// where initialize_cp_admin predates the unified authority initialization.
/// Only the existing protocol admin may fill the value, and only once.
#[no_mangle]
pub extern "C" fn set_identity_admin(admin_ptr: *const u8) -> u32 {
    let admin = match read_address32(admin_ptr) {
        Some(v) => v,
        None => return 40,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != admin {
        return 200;
    }

    if !is_cp_admin(&admin) || admin == [0u8; 32] {
        return 2;
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
/// Only callable by the identity admin and only configurable once.
#[no_mangle]
pub extern "C" fn set_lichenid_address(caller_ptr: *const u8, lichenid_addr_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(v) => v,
        None => return 40,
    };
    let lichenid_addr = match read_address32(lichenid_addr_ptr) {
        Some(v) => v,
        None => return 40,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    let admin = match storage_get(IDENTITY_ADMIN_KEY) {
        Some(data) => data,
        None => return 1,
    };
    if caller[..] != admin[..] {
        return 2;
    }

    if lichenid_addr == [0u8; 32] {
        return 3;
    }

    if load_configured_address(LICHENID_ADDR_KEY).is_some() {
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
        None => return 40,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    let admin = match storage_get(IDENTITY_ADMIN_KEY) {
        Some(data) => data,
        None => return 1,
    };
    if caller[..] != admin[..] {
        return 2;
    }
    if min_reputation > 0 && load_configured_address(LICHENID_ADDR_KEY).is_none() {
        return 3;
    }

    storage_set(LICHENID_MIN_REP_KEY, &u64_to_bytes(min_reputation));
    log_info("Identity gate configured");
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
// ACCOUNTING V3 MIGRATION
// ============================================================================

/// Freeze a legacy deployment and start exact escrow-liability reconstruction.
/// The expected count must equal the immutable contiguous stream ID frontier.
/// Returns 0 success/idempotent resume, 1 already active, 2 count mismatch,
/// 3 conflicting migration, 40 pointer error, 200 caller spoofing.
#[no_mangle]
pub extern "C" fn begin_accounting_v3_migration(
    caller_ptr: *const u8,
    expected_stream_count: u64,
) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(value) => value,
        None => return 40,
    };
    if get_caller().0 != caller {
        return 200;
    }
    if !is_cp_admin(&caller) {
        return 4;
    }
    if accounting_version() == ACCOUNTING_VERSION {
        return 1;
    }
    if checked_stored_u64(b"stream_count") != Some(expected_stream_count) {
        return 2;
    }
    if migration_locked() {
        return if checked_stored_u64(CP_MIGRATION_EXPECTED_COUNT_KEY)
            == Some(expected_stream_count)
        {
            0
        } else {
            3
        };
    }

    storage_set(PAUSE_KEY, &[1]);
    storage_set(CP_MIGRATION_LOCK_KEY, &[1]);
    storage_set(
        CP_MIGRATION_EXPECTED_COUNT_KEY,
        &u64_to_bytes(expected_stream_count),
    );
    storage_set(CP_MIGRATION_CURSOR_KEY, &u64_to_bytes(0));
    storage_set(CP_MIGRATION_LIABILITY_KEY, &u64_to_bytes(0));
    storage_set(CP_MIGRATION_UNPAID_KEY, &u64_to_bytes(0));
    0
}

/// Migrate exactly the next contiguous stream. This operation is permissionless,
/// deterministic, idempotent at the transaction layer, and resumable by cursor.
/// Canceled-stream unpaid balances are counted once per recipient.
#[no_mangle]
pub extern "C" fn migrate_accounting_v3_stream(stream_id: u64) -> u32 {
    if !migration_locked() || accounting_version() == ACCOUNTING_VERSION {
        return 1;
    }
    let cursor = match checked_stored_u64(CP_MIGRATION_CURSOR_KEY) {
        Some(value) => value,
        None => return 7,
    };
    let expected = match checked_stored_u64(CP_MIGRATION_EXPECTED_COUNT_KEY) {
        Some(value) => value,
        None => return 7,
    };
    if stream_id != cursor || stream_id >= expected {
        return 2;
    }
    let stream = match storage_get(&stream_key(stream_id))
        .as_deref()
        .and_then(StreamRecord::decode)
    {
        Some(value) => value,
        None => return 3,
    };
    let outstanding = match stream.outstanding() {
        Some(value) => value,
        None => return 4,
    };
    let sender_index = match prepare_index_append(SENDER_INDEX_PREFIX, &stream.sender) {
        Some(value) => value,
        None => return 7,
    };
    let recipient_index = match prepare_index_append(RECIPIENT_INDEX_PREFIX, &stream.recipient) {
        Some(value) => value,
        None => return 7,
    };

    let mut liability = match checked_stored_u64(CP_MIGRATION_LIABILITY_KEY) {
        Some(value) => value,
        None => return 7,
    };
    let mut unpaid = match checked_stored_u64(CP_MIGRATION_UNPAID_KEY) {
        Some(value) => value,
        None => return 7,
    };
    if stream.cancelled {
        let marker = migrated_unpaid_recipient_key(&stream.recipient);
        let marker_value = storage_get(&marker);
        if marker_value.is_none() {
            let token = match get_token_address() {
                Some(value) => value,
                None => return 5,
            };
            let recipient_unpaid = match checked_stored_u64(&unpaid_payout_key(
                token,
                Address(stream.recipient),
            )) {
                Some(value) => value,
                None => return 7,
            };
            liability = match liability.checked_add(recipient_unpaid) {
                Some(value) => value,
                None => return 6,
            };
            unpaid = match unpaid.checked_add(recipient_unpaid) {
                Some(value) => value,
                None => return 6,
            };
            storage_set(&marker, &[1]);
        } else if marker_value.as_deref() != Some(&[1]) {
            return 7;
        }
    } else {
        liability = match liability.checked_add(outstanding) {
            Some(value) => value,
            None => return 6,
        };
    }

    let next_cursor = match cursor.checked_add(1) {
        Some(value) => value,
        None => return 6,
    };
    storage_set(CP_MIGRATION_LIABILITY_KEY, &u64_to_bytes(liability));
    storage_set(CP_MIGRATION_UNPAID_KEY, &u64_to_bytes(unpaid));
    apply_index_append(&sender_index, stream_id);
    apply_index_append(&recipient_index, stream_id);
    storage_set(CP_MIGRATION_CURSOR_KEY, &u64_to_bytes(next_cursor));
    0
}

/// Finalize accounting only after every stream has been reconstructed, the
/// operator's independently generated totals match, and contract custody covers
/// every active/deferred obligation. The protocol remains paused for a separate
/// post-migration verification and explicit unpause.
#[no_mangle]
pub extern "C" fn complete_accounting_v3_migration(
    caller_ptr: *const u8,
    expected_liability: u64,
    expected_unpaid: u64,
) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(value) => value,
        None => return 40,
    };
    if get_caller().0 != caller {
        return 200;
    }
    if !is_cp_admin(&caller) {
        return 4;
    }
    if !migration_locked() || accounting_version() == ACCOUNTING_VERSION {
        return 1;
    }
    let expected_count = match checked_stored_u64(CP_MIGRATION_EXPECTED_COUNT_KEY) {
        Some(value) => value,
        None => return 10,
    };
    if checked_stored_u64(CP_MIGRATION_CURSOR_KEY) != Some(expected_count)
        || checked_stored_u64(b"stream_count") != Some(expected_count)
    {
        return 2;
    }
    let liability = match checked_stored_u64(CP_MIGRATION_LIABILITY_KEY) {
        Some(value) => value,
        None => return 10,
    };
    let unpaid = match checked_stored_u64(CP_MIGRATION_UNPAID_KEY) {
        Some(value) => value,
        None => return 10,
    };
    if liability != expected_liability || unpaid != expected_unpaid {
        return 3;
    }
    let token = match get_token_address() {
        Some(value) => value,
        None => return 5,
    };
    let self_addr = match get_self_address() {
        Some(value) => value,
        None => return 6,
    };
    let custody = match balance_of_token_or_native(token, self_addr) {
        Ok(value) => value,
        Err(_) => return 7,
    };
    if custody < liability {
        return 8;
    }
    if !remove_storage_key(CP_MIGRATION_LOCK_KEY) {
        return 9;
    }

    storage_set(CP_TOTAL_ESCROW_LIABILITY_KEY, &u64_to_bytes(liability));
    storage_set(CP_TOTAL_UNPAID_KEY, &u64_to_bytes(unpaid));
    storage_set(CP_ACCOUNTING_VERSION_KEY, &u64_to_bytes(ACCOUNTING_VERSION));
    0
}

// ============================================================================
// TESTS
// ============================================================================

/// Get stream count
#[no_mangle]
pub extern "C" fn get_stream_count() -> u64 {
    storage_get(b"stream_count")
        .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
        .unwrap_or(0)
}

/// Return bounded sender-associated stream IDs.
/// Payload: total_count(8) + next_cursor(8) + returned_count(8) + IDs.
#[no_mangle]
pub extern "C" fn get_sender_stream_ids(
    sender_ptr: *const u8,
    cursor: u64,
    limit: u64,
) -> u32 {
    get_address_stream_ids(SENDER_INDEX_PREFIX, sender_ptr, cursor, limit)
}

/// Return bounded recipient activity stream IDs. A transferred stream remains
/// in prior recipients' activity history and is appended to the new recipient.
#[no_mangle]
pub extern "C" fn get_recipient_stream_ids(
    recipient_ptr: *const u8,
    cursor: u64,
    limit: u64,
) -> u32 {
    get_address_stream_ids(RECIPIENT_INDEX_PREFIX, recipient_ptr, cursor, limit)
}

/// Get platform stats: stream count, lifetime created/withdrawn volume, cancel
/// count, exact escrow liability, deferred payout liability, accounting version,
/// and migration lock (eight little-endian u64 values).
#[no_mangle]
pub extern "C" fn get_platform_stats() -> u32 {
    let mut buf = Vec::with_capacity(64);
    buf.extend_from_slice(&u64_to_bytes(
        storage_get(b"stream_count")
            .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
            .unwrap_or(0),
    ));
    buf.extend_from_slice(&u64_to_bytes(
        storage_get(CP_TOTAL_STREAMED_KEY)
            .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
            .unwrap_or(0),
    ));
    buf.extend_from_slice(&u64_to_bytes(
        storage_get(CP_TOTAL_WITHDRAWN_KEY)
            .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
            .unwrap_or(0),
    ));
    buf.extend_from_slice(&u64_to_bytes(
        storage_get(CP_CANCEL_COUNT_KEY)
            .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
            .unwrap_or(0),
    ));
    buf.extend_from_slice(&u64_to_bytes(stored_u64(CP_TOTAL_ESCROW_LIABILITY_KEY)));
    buf.extend_from_slice(&u64_to_bytes(stored_u64(CP_TOTAL_UNPAID_KEY)));
    buf.extend_from_slice(&u64_to_bytes(accounting_version()));
    buf.extend_from_slice(&u64_to_bytes(u64::from(migration_locked())));
    lichen_sdk::set_return_data(&buf);
    0
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use lichen_sdk::test_mock;

    fn setup() {
        test_mock::reset();
        storage_set(CP_ACCOUNTING_VERSION_KEY, &u64_to_bytes(ACCOUNTING_VERSION));
        storage_set(CP_TOTAL_ESCROW_LIABILITY_KEY, &u64_to_bytes(0));
        storage_set(CP_TOTAL_UNPAID_KEY, &u64_to_bytes(0));
    }

    /// Configure escrow addresses in storage so stream creation succeeds.
    /// Sets token address and contract self-address directly in storage.
    fn configure_escrow() {
        let token = [0xAAu8; 32];
        let self_addr = [0xBBu8; 32];
        storage_set(CP_TOKEN_ADDR_KEY, &token);
        storage_set(CP_SELF_ADDR_KEY, &self_addr);
    }

    fn unpaid_key(token: &[u8; 32], recipient: &[u8; 32]) -> Vec<u8> {
        unpaid_payout_key(Address(*token), Address(*recipient))
    }

    #[test]
    fn test_abi_includes_escrow_configuration_exports() {
        let abi = include_str!("../abi.json");
        assert!(abi.contains(r#""name": "set_token_address""#));
        assert!(abi.contains(r#""name": "token_addr_ptr""#));
        assert!(abi.contains(r#""name": "set_self_address""#));
        assert!(abi.contains(r#""name": "self_addr_ptr""#));
    }

    // ====================================================================
    // CORE STREAM TESTS (with escrow)
    // ====================================================================

    #[test]
    fn test_create_stream() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        test_mock::set_caller(sender);
        let result = create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);
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
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        test_mock::set_caller(sender);
        create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);

        // Move to halfway point (slot 600 = 500 slots elapsed out of 1000)
        test_mock::SLOT.with(|s| *s.borrow_mut() = 600);

        // Withdrawable should be 500,000 (50% of 1M)
        let result = get_withdrawable(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 500_000);

        // Withdraw 300,000 — triggers token transfer from contract to recipient
        test_mock::set_caller(recipient);
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
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        test_mock::set_caller(sender);
        create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);

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
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        test_mock::set_caller(sender);
        create_stream(sender.as_ptr(), recipient.as_ptr(), 500_000, 100, 600);

        // Move past end
        test_mock::SLOT.with(|s| *s.borrow_mut() = 700);

        let result = get_withdrawable(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 500_000);

        test_mock::set_caller(recipient);
        let result = withdraw_from_stream(recipient.as_ptr(), 0, 500_000);
        assert_eq!(result, 0);

        // Nothing left
        let result = get_withdrawable(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 0);
    }

    // ====================================================================
    // IDENTITY GATE TESTS
    // ====================================================================

    #[test]
    fn test_identity_gate_blocks_create_stream_sender() {
        setup();
        // No escrow needed — identity gate blocks before escrow check
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [5u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);
        let lichenid_addr = [0x42u8; 32];
        assert_eq!(
            set_lichenid_address(admin.as_ptr(), lichenid_addr.as_ptr()),
            0
        );
        assert_eq!(set_identity_gate(admin.as_ptr(), 1), 0);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        let result = create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);
        assert_eq!(result, 10); // sender blocked
    }

    #[test]
    fn test_identity_gate_allows_when_disabled() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        let result = create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_set_identity_gate_admin_only() {
        setup();

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);
        let lichenid_addr = [0x42u8; 32];
        assert_eq!(
            set_lichenid_address(admin.as_ptr(), lichenid_addr.as_ptr()),
            0
        );

        let other = [9u8; 32];
        test_mock::set_caller(other);
        assert_eq!(set_identity_gate(other.as_ptr(), 100), 2);
        test_mock::set_caller(admin);
        assert_eq!(set_identity_gate(admin.as_ptr(), 100), 0);
    }

    // ====================================================================
    // V2 TESTS — CLIFF STREAMS
    // ====================================================================

    #[test]
    fn test_create_stream_with_cliff() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        test_mock::set_caller(sender);
        let result = create_stream_with_cliff(
            sender.as_ptr(),
            recipient.as_ptr(),
            1_000_000,
            100,
            1100,
            500,
        );
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 0); // stream_id = 0
    }

    #[test]
    fn test_cliff_blocks_withdrawal_before_cliff() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        test_mock::set_caller(sender);
        create_stream_with_cliff(
            sender.as_ptr(),
            recipient.as_ptr(),
            1_000_000,
            100,
            1100,
            500,
        );

        // Before cliff (slot 300) — should get 0
        test_mock::SLOT.with(|s| *s.borrow_mut() = 300);
        let result = get_withdrawable(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 0);

        // Try to withdraw — should fail
        test_mock::set_caller(recipient);
        let result = withdraw_from_stream(recipient.as_ptr(), 0, 1);
        assert_eq!(result, 6); // exceeds withdrawable (0)
    }

    #[test]
    fn test_cliff_allows_withdrawal_after_cliff() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        test_mock::set_caller(sender);
        create_stream_with_cliff(
            sender.as_ptr(),
            recipient.as_ptr(),
            1_000_000,
            100,
            1100,
            500,
        );

        // After cliff (slot 600) — 500 elapsed out of 1000 = 50%
        test_mock::SLOT.with(|s| *s.borrow_mut() = 600);
        let result = get_withdrawable(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 500_000);

        // Withdraw works after cliff
        test_mock::set_caller(recipient);
        let result = withdraw_from_stream(recipient.as_ptr(), 0, 500_000);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_cliff_invalid_params() {
        setup();
        // No escrow needed — param validation fails before escrow check
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        test_mock::set_caller(sender);
        // cliff before start
        let result = create_stream_with_cliff(
            sender.as_ptr(),
            recipient.as_ptr(),
            1_000_000,
            100,
            1100,
            50,
        );
        assert_eq!(result, 2);

        // cliff after end
        let result = create_stream_with_cliff(
            sender.as_ptr(),
            recipient.as_ptr(),
            1_000_000,
            100,
            1100,
            2000,
        );
        assert_eq!(result, 3);
    }

    // ====================================================================
    // TRANSFER, PAUSE, ADMIN TESTS
    // ====================================================================

    #[test]
    fn test_transfer_stream() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        let new_recipient = [3u8; 32];

        test_mock::set_caller(sender);
        create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);

        // Non-recipient cannot transfer
        let result = transfer_stream(sender.as_ptr(), new_recipient.as_ptr(), 0);
        assert_eq!(result, 2);

        // Recipient can transfer
        test_mock::set_caller(recipient);
        let result = transfer_stream(recipient.as_ptr(), new_recipient.as_ptr(), 0);
        assert_eq!(result, 0);

        // New recipient can now withdraw
        test_mock::SLOT.with(|s| *s.borrow_mut() = 600);
        test_mock::set_caller(new_recipient);
        let result = withdraw_from_stream(new_recipient.as_ptr(), 0, 100_000);
        assert_eq!(result, 0);

        // Old recipient cannot withdraw
        test_mock::set_caller(recipient);
        let result = withdraw_from_stream(recipient.as_ptr(), 0, 100_000);
        assert_eq!(result, 4); // not recipient
    }

    #[test]
    fn test_transfer_cancelled_stream_fails() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        let new_recip = [3u8; 32];

        test_mock::set_caller(sender);
        create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);
        cancel_stream(sender.as_ptr(), 0);

        test_mock::set_caller(recipient);
        let result = transfer_stream(recipient.as_ptr(), new_recip.as_ptr(), 0);
        assert_eq!(result, 3); // cancelled
    }

    #[test]
    fn test_pause_unpause() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [10u8; 32];
        let non_admin = [11u8; 32];
        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        test_mock::set_caller(admin);
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);
        test_mock::set_caller(non_admin);
        assert_eq!(initialize_cp_admin(non_admin.as_ptr()), 1);

        // Non-admin cannot pause
        assert_eq!(pause(non_admin.as_ptr()), 1);

        // Admin pauses
        test_mock::set_caller(admin);
        assert_eq!(pause(admin.as_ptr()), 0);
        assert_eq!(pause(admin.as_ptr()), 2); // already paused

        // create_stream blocked when paused
        test_mock::set_caller(sender);
        let result = create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);
        assert_eq!(result, 20);

        // create_stream_with_cliff blocked too
        let result = create_stream_with_cliff(
            sender.as_ptr(),
            recipient.as_ptr(),
            1_000_000,
            100,
            1100,
            500,
        );
        assert_eq!(result, 20);

        // Non-admin cannot unpause
        test_mock::set_caller(non_admin);
        assert_eq!(unpause(non_admin.as_ptr()), 1);
        // Unpause
        test_mock::set_caller(admin);
        assert_eq!(unpause(admin.as_ptr()), 0);
        assert_eq!(unpause(admin.as_ptr()), 2); // already unpaused

        // Now create_stream works again (escrow configured)
        test_mock::set_caller(sender);
        let result = create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_get_stream_info_with_cliff() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        test_mock::set_caller(sender);
        create_stream_with_cliff(
            sender.as_ptr(),
            recipient.as_ptr(),
            1_000_000,
            100,
            1100,
            500,
        );

        let result = get_stream_info(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
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
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [10u8; 32];
        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        // Create before pause
        test_mock::set_caller(sender);
        create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);

        // Pause
        test_mock::set_caller(admin);
        initialize_cp_admin(admin.as_ptr());
        pause(admin.as_ptr());

        // Withdraw still works (safety valve)
        test_mock::SLOT.with(|s| *s.borrow_mut() = 600);
        test_mock::set_caller(recipient);
        let result = withdraw_from_stream(recipient.as_ptr(), 0, 100_000);
        assert_eq!(result, 0);

        // Cancel still works
        test_mock::set_caller(sender);
        let result = cancel_stream(sender.as_ptr(), 0);
        assert_eq!(result, 0);
    }

    // ====================================================================
    // ESCROW-SPECIFIC TESTS
    // ====================================================================

    #[test]
    fn test_create_stream_fails_without_token_address() {
        setup();
        // Only set self-address, NOT token address
        storage_set(CP_SELF_ADDR_KEY, &[0xBBu8; 32]);
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        let result = create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);
        assert_eq!(result, 30); // token address not configured
    }

    #[test]
    fn test_create_stream_fails_without_self_address() {
        setup();
        // Only set token address, NOT self address
        storage_set(CP_TOKEN_ADDR_KEY, &[0xAAu8; 32]);
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        let result = create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100);
        assert_eq!(result, 31); // self address not configured
    }

    #[test]
    fn test_create_stream_with_cliff_fails_without_escrow() {
        setup();
        // No escrow configured
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        let result = create_stream_with_cliff(
            sender.as_ptr(),
            recipient.as_ptr(),
            1_000_000,
            100,
            1100,
            500,
        );
        assert_eq!(result, 30); // token address not configured
    }

    #[test]
    fn test_set_token_address_admin_only() {
        setup();
        let admin = [10u8; 32];
        let non_admin = [11u8; 32];
        let token = [0xAAu8; 32];

        // Init admin
        test_mock::set_caller(admin);
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);

        // Non-admin cannot set token address
        test_mock::set_caller(non_admin);
        let result = set_token_address(non_admin.as_ptr(), token.as_ptr());
        assert_eq!(result, 1); // not admin

        // Admin can set
        test_mock::set_caller(admin);
        let result = set_token_address(admin.as_ptr(), token.as_ptr());
        assert_eq!(result, 0);

        // Verify stored correctly
        let stored = test_mock::get_storage(CP_TOKEN_ADDR_KEY).unwrap();
        assert_eq!(stored.as_slice(), &token);
    }

    #[test]
    fn test_set_token_address_accepts_zero() {
        setup();
        let admin = [10u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);

        let zero = [0u8; 32];
        let result = set_token_address(admin.as_ptr(), zero.as_ptr());
        assert_eq!(result, 0); // zero address = native LICN sentinel
    }

    #[test]
    fn test_set_token_address_cannot_reconfigure_after_zero_sentinel() {
        setup();
        let admin = [10u8; 32];
        let token = [0xAAu8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);

        assert_eq!(set_token_address(admin.as_ptr(), [0u8; 32].as_ptr()), 0);
        assert_eq!(set_token_address(admin.as_ptr(), token.as_ptr()), 2);

        let stored = test_mock::get_storage(CP_TOKEN_ADDR_KEY).unwrap();
        assert_eq!(stored.as_slice(), &[0u8; 32]);
    }

    #[test]
    fn test_set_self_address_admin_only() {
        setup();
        let admin = [10u8; 32];
        let non_admin = [11u8; 32];
        let self_addr = [0xBBu8; 32];

        test_mock::set_caller(admin);
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);

        // Non-admin cannot set
        test_mock::set_caller(non_admin);
        let result = set_self_address(non_admin.as_ptr(), self_addr.as_ptr());
        assert_eq!(result, 1);

        // Admin can set
        test_mock::set_caller(admin);
        let result = set_self_address(admin.as_ptr(), self_addr.as_ptr());
        assert_eq!(result, 0);

        let stored = test_mock::get_storage(CP_SELF_ADDR_KEY).unwrap();
        assert_eq!(stored.as_slice(), &self_addr);
    }

    #[test]
    fn test_set_self_address_rejects_zero() {
        setup();
        let admin = [10u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);

        let zero = [0u8; 32];
        let result = set_self_address(admin.as_ptr(), zero.as_ptr());
        assert_eq!(result, 2);
    }

    #[test]
    fn test_set_self_address_cannot_reconfigure() {
        setup();
        let admin = [10u8; 32];
        let first = [0xBBu8; 32];
        let second = [0xCCu8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);

        assert_eq!(set_self_address(admin.as_ptr(), first.as_ptr()), 0);
        assert_eq!(set_self_address(admin.as_ptr(), second.as_ptr()), 3);

        let stored = test_mock::get_storage(CP_SELF_ADDR_KEY).unwrap();
        assert_eq!(stored.as_slice(), &first);
    }

    #[test]
    fn test_set_lichenid_address_admin_only() {
        setup();
        let admin = [10u8; 32];
        let other = [11u8; 32];
        let lichenid = [0x42u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);

        test_mock::set_caller(other);
        assert_eq!(set_lichenid_address(other.as_ptr(), lichenid.as_ptr()), 2);

        test_mock::set_caller(admin);
        assert_eq!(set_lichenid_address(admin.as_ptr(), lichenid.as_ptr()), 0);

        let stored = test_mock::get_storage(LICHENID_ADDR_KEY).unwrap();
        assert_eq!(stored.as_slice(), &lichenid);
    }

    #[test]
    fn test_set_lichenid_address_rejects_zero_and_reconfiguration() {
        setup();
        let admin = [10u8; 32];
        let first = [0x42u8; 32];
        let second = [0x43u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);

        assert_eq!(set_lichenid_address(admin.as_ptr(), [0u8; 32].as_ptr()), 3);
        assert_eq!(set_lichenid_address(admin.as_ptr(), first.as_ptr()), 0);
        assert_eq!(set_lichenid_address(admin.as_ptr(), second.as_ptr()), 4);

        let stored = test_mock::get_storage(LICHENID_ADDR_KEY).unwrap();
        assert_eq!(stored.as_slice(), &first);
    }

    #[test]
    fn test_cancel_without_escrow_configuration_fails_closed() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        // Manually create a stream record in storage without escrow
        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        let data = StreamRecord {
            sender,
            recipient,
            total_amount: 1_000_000,
            withdrawn: 0,
            start_slot: 100,
            end_slot: 1100,
            cancelled: false,
            created_slot: 100,
        }
        .encode();
        let sk = stream_key(0);
        storage_set(&sk, &data);
        storage_set(b"stream_count", &u64_to_bytes(1));
        storage_set(CP_TOTAL_ESCROW_LIABILITY_KEY, &u64_to_bytes(1_000_000));

        test_mock::set_caller(sender);
        let result = cancel_stream(sender.as_ptr(), 0);
        assert_eq!(result, 30);

        let stream = test_mock::get_storage(&sk).unwrap();
        assert_eq!(stream[96], 0);
        assert_eq!(stored_u64(CP_TOTAL_ESCROW_LIABILITY_KEY), 1_000_000);
    }

    #[test]
    fn test_escrow_full_lifecycle_create_withdraw_complete() {
        // Full lifecycle: create → withdraw partial → withdraw rest
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100),
            0
        );

        // Withdraw 250k at 25%
        test_mock::SLOT.with(|s| *s.borrow_mut() = 350);
        test_mock::set_caller(recipient);
        assert_eq!(withdraw_from_stream(recipient.as_ptr(), 0, 250_000), 0);

        // Verify stream state
        let sk = stream_key(0);
        let stream = test_mock::get_storage(&sk).unwrap();
        assert_eq!(bytes_to_u64(&stream[72..80]), 250_000); // withdrawn = 250k

        // Withdraw remaining at end
        test_mock::SLOT.with(|s| *s.borrow_mut() = 1200);
        let result = get_withdrawable(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 750_000); // 1M - 250k already withdrawn

        assert_eq!(withdraw_from_stream(recipient.as_ptr(), 0, 750_000), 0);

        // Nothing left
        let result = get_withdrawable(0);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 0);
    }

    #[test]
    fn test_escrow_create_then_cancel_with_partial_withdrawal() {
        // Create → withdraw partial → cancel → verify settlement
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100),
            0
        );

        // Withdraw at 50%
        test_mock::SLOT.with(|s| *s.borrow_mut() = 600);
        test_mock::set_caller(recipient);
        assert_eq!(withdraw_from_stream(recipient.as_ptr(), 0, 200_000), 0);

        // Cancel at 50% — refund = 500k, recipient_due = 500k - 200k = 300k
        test_mock::set_caller(sender);
        assert_eq!(cancel_stream(sender.as_ptr(), 0), 0);

        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 500_000); // refund = 50% unstreamed

        // Verify cancelled
        let sk = stream_key(0);
        let stream = test_mock::get_storage(&sk).unwrap();
        assert_eq!(stream[96], 1);
    }

    #[test]
    fn test_cancel_already_cancelled_fails() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100),
            0
        );
        assert_eq!(cancel_stream(sender.as_ptr(), 0), 0);
        // Second cancel fails
        assert_eq!(cancel_stream(sender.as_ptr(), 0), 4);
    }

    #[test]
    fn test_get_platform_stats_with_escrow() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        // Create two streams
        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100),
            0
        );
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 500_000, 100, 600),
            0
        );

        // Withdraw from stream 0
        test_mock::SLOT.with(|s| *s.borrow_mut() = 600);
        test_mock::set_caller(recipient);
        assert_eq!(withdraw_from_stream(recipient.as_ptr(), 0, 100_000), 0);

        // Cancel stream 1
        test_mock::set_caller(sender);
        assert_eq!(cancel_stream(sender.as_ptr(), 1), 0);

        // Check platform stats
        let result = get_platform_stats();
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 64);
        assert_eq!(bytes_to_u64(&ret[0..8]), 2); // stream_count = 2
        assert_eq!(bytes_to_u64(&ret[8..16]), 1_500_000); // total_streamed = 1.5M
        assert_eq!(bytes_to_u64(&ret[16..24]), 100_000); // total_withdrawn = 100k
        assert_eq!(bytes_to_u64(&ret[24..32]), 1); // cancel_count = 1
        assert_eq!(bytes_to_u64(&ret[32..40]), 900_000); // active escrow liability
        assert_eq!(bytes_to_u64(&ret[40..48]), 0); // no deferred payout
        assert_eq!(bytes_to_u64(&ret[48..56]), ACCOUNTING_VERSION);
        assert_eq!(bytes_to_u64(&ret[56..64]), 0); // migration unlocked
    }

    #[test]
    fn test_escrow_addresses_stored_correctly() {
        setup();
        let admin = [10u8; 32];
        let token = [0xAAu8; 32];
        let self_addr = [0xBBu8; 32];

        test_mock::set_caller(admin);
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);
        assert_eq!(set_token_address(admin.as_ptr(), token.as_ptr()), 0);
        assert_eq!(set_self_address(admin.as_ptr(), self_addr.as_ptr()), 0);

        // Verify via helper functions
        let t = get_token_address().unwrap();
        assert_eq!(t.0, token);
        let s = get_self_address().unwrap();
        assert_eq!(s.0, self_addr);
    }

    #[test]
    fn test_withdraw_from_cancelled_stream_fails() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100),
            0
        );

        // Cancel
        test_mock::SLOT.with(|s| *s.borrow_mut() = 600);
        assert_eq!(cancel_stream(sender.as_ptr(), 0), 0);

        // Try to withdraw — stream is cancelled
        test_mock::set_caller(recipient);
        let result = withdraw_from_stream(recipient.as_ptr(), 0, 1);
        assert_eq!(result, 5); // cancelled
    }

    #[test]
    fn test_non_sender_cannot_cancel() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100),
            0
        );

        // Recipient cannot cancel
        test_mock::set_caller(recipient);
        let result = cancel_stream(recipient.as_ptr(), 0);
        assert_eq!(result, 3); // only sender can cancel
    }

    #[test]
    fn test_create_stream_false_escrow_status_rejected() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100),
            32
        );
        assert!(test_mock::get_storage(b"stream_count").is_none());
        assert!(test_mock::get_storage(&stream_key(0)).is_none());
    }

    #[test]
    fn test_create_stream_count_overflow_rejected_before_escrow() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        storage_set(b"stream_count", &u64_to_bytes(u64::MAX));

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100),
            34
        );
        assert!(test_mock::get_storage(&stream_key(u64::MAX)).is_none());
        assert!(test_mock::get_last_cross_call().is_none());
    }

    #[test]
    fn test_create_stream_with_cliff_false_escrow_status_rejected() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        assert_eq!(
            create_stream_with_cliff(
                sender.as_ptr(),
                recipient.as_ptr(),
                1_000_000,
                100,
                1100,
                500,
            ),
            32
        );
        assert!(test_mock::get_storage(b"stream_count").is_none());
        assert!(test_mock::get_storage(&stream_key(0)).is_none());
    }

    #[test]
    fn test_withdraw_false_transfer_preserves_withdrawn_amount() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100),
            0
        );

        test_mock::SLOT.with(|s| *s.borrow_mut() = 600);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        test_mock::set_caller(recipient);
        assert_eq!(withdraw_from_stream(recipient.as_ptr(), 0, 300_000), 32);

        let stream = test_mock::get_storage(&stream_key(0)).unwrap();
        assert_eq!(bytes_to_u64(&stream[72..80]), 0);
    }

    #[test]
    fn test_cancel_partial_recipient_failure_records_unpaid_after_refund() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100),
            0
        );

        test_mock::SLOT.with(|s| *s.borrow_mut() = 350);
        test_mock::set_cross_call_responses(alloc::vec![
            0u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(cancel_stream(sender.as_ptr(), 0), 0);

        let stream = test_mock::get_storage(&stream_key(0)).unwrap();
        assert_eq!(stream[96], 1);
        let token = [0xAAu8; 32];
        let unpaid = test_mock::get_storage(&unpaid_key(&token, &recipient)).unwrap();
        assert_eq!(bytes_to_u64(&unpaid), 250_000);
    }

    #[test]
    fn test_claim_unpaid_payout_releases_recorded_recipient_due() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100),
            0
        );

        test_mock::SLOT.with(|s| *s.borrow_mut() = 350);
        test_mock::set_cross_call_responses(alloc::vec![
            0u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(cancel_stream(sender.as_ptr(), 0), 0);

        let token = [0xAAu8; 32];
        let key = unpaid_key(&token, &recipient);
        assert_eq!(
            bytes_to_u64(&test_mock::get_storage(&key).unwrap()),
            250_000
        );

        assert_eq!(get_unpaid_payout(recipient.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 250_000);

        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        test_mock::set_caller(recipient);
        assert_eq!(claim_unpaid_payout(recipient.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 250_000);
        assert!(test_mock::get_storage(&key).is_none());

        assert_eq!(claim_unpaid_payout(recipient.as_ptr()), 2);
    }

    #[test]
    fn test_claim_unpaid_payout_failed_transfer_preserves_unpaid() {
        setup();
        configure_escrow();

        let token = [0xAAu8; 32];
        let recipient = [2u8; 32];
        let key = unpaid_key(&token, &recipient);
        storage_set(&key, &u64_to_bytes(250_000));
        storage_set(CP_TOTAL_UNPAID_KEY, &u64_to_bytes(250_000));
        storage_set(CP_TOTAL_ESCROW_LIABILITY_KEY, &u64_to_bytes(250_000));

        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        test_mock::set_caller(recipient);
        assert_eq!(claim_unpaid_payout(recipient.as_ptr()), 32);

        let unpaid = test_mock::get_storage(&key).unwrap();
        assert_eq!(bytes_to_u64(&unpaid), 250_000);
    }

    #[test]
    fn test_claim_unpaid_payout_rejects_caller_spoof() {
        setup();
        configure_escrow();

        let token = [0xAAu8; 32];
        let recipient = [2u8; 32];
        let attacker = [9u8; 32];
        let key = unpaid_key(&token, &recipient);
        storage_set(&key, &u64_to_bytes(250_000));
        storage_set(CP_TOTAL_UNPAID_KEY, &u64_to_bytes(250_000));
        storage_set(CP_TOTAL_ESCROW_LIABILITY_KEY, &u64_to_bytes(250_000));

        test_mock::set_caller(attacker);
        assert_eq!(claim_unpaid_payout(recipient.as_ptr()), 200);

        let unpaid = test_mock::get_storage(&key).unwrap();
        assert_eq!(bytes_to_u64(&unpaid), 250_000);
    }

    #[test]
    fn test_claim_unpaid_payout_works_when_paused() {
        setup();
        configure_escrow();

        let admin = [10u8; 32];
        let token = [0xAAu8; 32];
        let recipient = [2u8; 32];
        let key = unpaid_key(&token, &recipient);

        test_mock::set_caller(admin);
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);
        assert_eq!(pause(admin.as_ptr()), 0);
        storage_set(&key, &u64_to_bytes(250_000));
        storage_set(CP_TOTAL_UNPAID_KEY, &u64_to_bytes(250_000));
        storage_set(CP_TOTAL_ESCROW_LIABILITY_KEY, &u64_to_bytes(250_000));

        test_mock::set_caller(recipient);
        assert_eq!(claim_unpaid_payout(recipient.as_ptr()), 0);
        assert!(test_mock::get_storage(&key).is_none());
    }

    #[test]
    fn test_cancel_refund_failure_preserves_stream_and_unpaid_state() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100),
            0
        );

        test_mock::SLOT.with(|s| *s.borrow_mut() = 350);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(cancel_stream(sender.as_ptr(), 0), 32);

        let stream = test_mock::get_storage(&stream_key(0)).unwrap();
        assert_eq!(stream[96], 0);
        assert!(test_mock::get_storage(CP_CANCEL_COUNT_KEY).is_none());

        let token = [0xAAu8; 32];
        assert!(test_mock::get_storage(&unpaid_key(&token, &recipient)).is_none());
    }

    #[test]
    fn test_cancel_recipient_failure_without_refund_records_unpaid() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100),
            0
        );

        test_mock::SLOT.with(|s| *s.borrow_mut() = 1200);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(cancel_stream(sender.as_ptr(), 0), 0);

        let stream = test_mock::get_storage(&stream_key(0)).unwrap();
        assert_eq!(stream[96], 1);
        let token = [0xAAu8; 32];
        assert_eq!(stored_u64(&unpaid_key(&token, &recipient)), 1_000_000);
        assert_eq!(stored_u64(CP_TOTAL_UNPAID_KEY), 1_000_000);
        assert_eq!(stored_u64(CP_TOTAL_ESCROW_LIABILITY_KEY), 1_000_000);
    }

    #[test]
    fn test_cancel_count_overflow_fails_before_settlement() {
        setup();
        configure_escrow();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        storage_set(CP_CANCEL_COUNT_KEY, &u64_to_bytes(u64::MAX));

        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100),
            0
        );

        assert_eq!(cancel_stream(sender.as_ptr(), 0), 35);
        let count = test_mock::get_storage(CP_CANCEL_COUNT_KEY).unwrap();
        assert_eq!(bytes_to_u64(&count), u64::MAX);
        let stream = test_mock::get_storage(&stream_key(0)).unwrap();
        assert_eq!(stream[96], 0);
    }

    #[test]
    fn test_accounting_liability_tracks_full_lifecycle() {
        setup();
        configure_escrow();
        test_mock::set_slot(100);
        let sender = [1u8; 32];
        let recipient = [2u8; 32];

        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1_000_000, 100, 1100),
            0
        );
        assert_eq!(stored_u64(CP_TOTAL_ESCROW_LIABILITY_KEY), 1_000_000);

        test_mock::set_slot(600);
        test_mock::set_caller(recipient);
        assert_eq!(withdraw_from_stream(recipient.as_ptr(), 0, 200_000), 0);
        assert_eq!(stored_u64(CP_TOTAL_ESCROW_LIABILITY_KEY), 800_000);

        test_mock::set_caller(sender);
        assert_eq!(cancel_stream(sender.as_ptr(), 0), 0);
        assert_eq!(stored_u64(CP_TOTAL_ESCROW_LIABILITY_KEY), 0);
        assert_eq!(stored_u64(CP_TOTAL_UNPAID_KEY), 0);
    }

    #[test]
    fn test_cliff_cancel_before_boundary_refunds_everything() {
        setup();
        configure_escrow();
        test_mock::set_slot(100);
        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        assert_eq!(
            create_stream_with_cliff(
                sender.as_ptr(),
                recipient.as_ptr(),
                1_000_000,
                100,
                1100,
                500,
            ),
            0
        );

        test_mock::set_slot(400);
        assert_eq!(cancel_stream(sender.as_ptr(), 0), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 1_000_000);
        assert_eq!(stored_u64(CP_TOTAL_ESCROW_LIABILITY_KEY), 0);
        assert_eq!(stored_u64(CP_TOTAL_UNPAID_KEY), 0);
        assert_eq!(test_mock::get_last_cross_call().unwrap().1, "transfer");
    }

    #[test]
    fn test_enabled_identity_gate_fails_closed_without_lichenid() {
        setup();
        storage_set(LICHENID_MIN_REP_KEY, &u64_to_bytes(1));
        assert!(!check_identity_gate(&[7u8; 32]));

        let admin = [5u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);
        assert_eq!(set_identity_gate(admin.as_ptr(), 1), 3);
    }

    #[test]
    fn test_transfer_respects_pause_identity_and_reentrancy() {
        setup();
        configure_escrow();
        test_mock::set_slot(100);
        let admin = [9u8; 32];
        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        let replacement = [3u8; 32];
        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1000, 100, 1100),
            0
        );

        test_mock::set_caller(admin);
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);
        let lichenid = [0x42u8; 32];
        assert_eq!(set_lichenid_address(admin.as_ptr(), lichenid.as_ptr()), 0);
        assert_eq!(set_identity_gate(admin.as_ptr(), 1), 0);

        test_mock::set_caller(recipient);
        assert_eq!(
            transfer_stream(recipient.as_ptr(), replacement.as_ptr(), 0),
            11
        );

        test_mock::set_caller(admin);
        assert_eq!(set_identity_gate(admin.as_ptr(), 0), 0);
        assert_eq!(pause(admin.as_ptr()), 0);
        test_mock::set_caller(recipient);
        assert_eq!(
            transfer_stream(recipient.as_ptr(), replacement.as_ptr(), 0),
            20
        );

        storage_set(PAUSE_KEY, &[0]);
        storage_set(CP_REENTRANCY_KEY, &[1]);
        assert_eq!(
            transfer_stream(recipient.as_ptr(), replacement.as_ptr(), 0),
            20
        );
    }

    #[test]
    fn test_accounting_v3_migration_is_exact_resumable_and_solvency_gated() {
        setup();
        configure_escrow();
        let admin = [9u8; 32];
        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize_cp_admin(admin.as_ptr()), 0);

        remove_storage_key(CP_ACCOUNTING_VERSION_KEY);
        storage_set(b"stream_count", &u64_to_bytes(3));
        storage_set(
            &stream_key(0),
            &StreamRecord {
                sender,
                recipient,
                total_amount: 1000,
                withdrawn: 250,
                start_slot: 1,
                end_slot: 100,
                cancelled: false,
                created_slot: 1,
            }
            .encode(),
        );
        for stream_id in 1..=2 {
            storage_set(
                &stream_key(stream_id),
                &StreamRecord {
                    sender,
                    recipient,
                    total_amount: 500,
                    withdrawn: 0,
                    start_slot: 1,
                    end_slot: 100,
                    cancelled: true,
                    created_slot: 1,
                }
                .encode(),
            );
        }
        let token = Address([0xAAu8; 32]);
        storage_set(
            &unpaid_payout_key(token, Address(recipient)),
            &u64_to_bytes(100),
        );

        assert_eq!(begin_accounting_v3_migration(admin.as_ptr(), 3), 0);
        assert_eq!(begin_accounting_v3_migration(admin.as_ptr(), 3), 0);
        assert_eq!(migrate_accounting_v3_stream(1), 2);
        assert_eq!(migrate_accounting_v3_stream(0), 0);
        assert_eq!(migrate_accounting_v3_stream(1), 0);
        assert_eq!(migrate_accounting_v3_stream(2), 0);
        assert_eq!(stored_u64(CP_MIGRATION_LIABILITY_KEY), 850);
        assert_eq!(stored_u64(CP_MIGRATION_UNPAID_KEY), 100);
        assert_eq!(
            stored_u64(&address_index_count_key(SENDER_INDEX_PREFIX, &sender)),
            3
        );
        assert_eq!(
            stored_u64(&address_index_count_key(
                RECIPIENT_INDEX_PREFIX,
                &recipient,
            )),
            3
        );

        test_mock::set_cross_call_response(Some(849u64.to_le_bytes().to_vec()));
        assert_eq!(
            complete_accounting_v3_migration(admin.as_ptr(), 850, 100),
            8
        );
        assert!(migration_locked());

        test_mock::set_cross_call_response(Some(850u64.to_le_bytes().to_vec()));
        assert_eq!(
            complete_accounting_v3_migration(admin.as_ptr(), 850, 100),
            0
        );
        assert_eq!(accounting_version(), ACCOUNTING_VERSION);
        assert!(!migration_locked());
        assert_eq!(stored_u64(CP_TOTAL_ESCROW_LIABILITY_KEY), 850);
        assert_eq!(stored_u64(CP_TOTAL_UNPAID_KEY), 100);
        assert!(is_paused());
        assert_eq!(unpause(admin.as_ptr()), 0);
    }

    #[test]
    fn test_legacy_accounting_blocks_value_mutations_until_migrated() {
        setup();
        configure_escrow();
        remove_storage_key(CP_ACCOUNTING_VERSION_KEY);
        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1000, 1, 100),
            95
        );
        assert!(test_mock::get_last_cross_call().is_none());
    }

    #[test]
    fn test_account_stream_indexes_are_bounded_and_paginated() {
        setup();
        configure_escrow();
        test_mock::set_slot(100);
        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1000, 100, 1100),
            0
        );
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 2000, 100, 1100),
            0
        );

        assert_eq!(get_sender_stream_ids(sender.as_ptr(), 0, 1), 0);
        let first = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&first[0..8]), 2);
        assert_eq!(bytes_to_u64(&first[8..16]), 1);
        assert_eq!(bytes_to_u64(&first[16..24]), 1);
        assert_eq!(bytes_to_u64(&first[24..32]), 0);

        assert_eq!(get_sender_stream_ids(sender.as_ptr(), 1, 64), 0);
        let second = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&second[0..8]), 2);
        assert_eq!(bytes_to_u64(&second[8..16]), 2);
        assert_eq!(bytes_to_u64(&second[16..24]), 1);
        assert_eq!(bytes_to_u64(&second[24..32]), 1);

        assert_eq!(get_recipient_stream_ids(recipient.as_ptr(), 0, 64), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()[0..8]), 2);
        assert_eq!(get_sender_stream_ids(sender.as_ptr(), 3, 1), 2);
        assert_eq!(get_sender_stream_ids(sender.as_ptr(), 0, 0), 3);
        assert_eq!(get_sender_stream_ids(sender.as_ptr(), 0, 65), 3);
    }

    #[test]
    fn test_transfer_appends_new_recipient_activity_index() {
        setup();
        configure_escrow();
        test_mock::set_slot(100);
        let sender = [1u8; 32];
        let recipient = [2u8; 32];
        let replacement = [3u8; 32];
        test_mock::set_caller(sender);
        assert_eq!(
            create_stream(sender.as_ptr(), recipient.as_ptr(), 1000, 100, 1100),
            0
        );
        test_mock::set_caller(recipient);
        assert_eq!(
            transfer_stream(recipient.as_ptr(), replacement.as_ptr(), 0),
            0
        );

        assert_eq!(get_recipient_stream_ids(recipient.as_ptr(), 0, 64), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()[0..8]), 1);
        assert_eq!(
            get_recipient_stream_ids(replacement.as_ptr(), 0, 64),
            0
        );
        let replacement_page = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&replacement_page[0..8]), 1);
        assert_eq!(bytes_to_u64(&replacement_page[24..32]), 0);
    }
}
