// Moss Storage — Decentralized Storage Layer for Lichen (v2 — DEEP hardened)
//
// v2 additions:
//   - Proof-of-storage challenges: random challenges to verify providers store data
//   - Provider slashing: providers that fail challenges lose staked collateral
//   - Storage marketplace pricing: providers set custom price per byte per slot
//   - Collateral staking: providers must stake LICN proportional to capacity
//   - Challenge response window: providers have limited time to respond
//
// Storage keys:
//   data_{hash}          → StorageEntry (owner, size, replication, confirmations, expiry, providers)
//   provider_{addr}      → ProviderInfo (capacity, stored_count, active, registered_slot, stake, price)
//   reward_{addr}        → matured reward balance / legacy pending reward balance (u64)
//   reward_idx_{addr}    → concatenated 32-byte data hashes confirmed by provider
//   reward_pos_{addr}_{hash} → last rewarded slot for that provider/data confirmation (u64)
//   data_count           → total registered data entries (u64)
//   challenge_{hash}_{addr} → Challenge (slot, response_deadline, nonce, answered)
//   challenge_challenger_{hash}_{addr} → challenger address bound to the open challenge
//   challenge_window     → slots allowed for challenge response (u64)
//   slash_percent        → percentage of stake slashed on failure (u64)
//   moss_admin           → admin address (32 bytes)

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use lichen_sdk::{
    bytes_to_u64, get_caller, get_contract_address, get_slot, get_value, log_info, storage_get,
    storage_set, transfer_token_or_native, u64_to_bytes, Address,
};

// ============================================================================
// CONSTANTS
// ============================================================================

const MAX_REPLICATION: u8 = 10;
const MIN_STORAGE_DURATION: u64 = 1000; // minimum slots
const MAX_PROVIDERS_PER_ENTRY: usize = 16;
const REWARD_PER_SLOT_PER_BYTE: u64 = 10; // 10 spores per slot per byte stored

// v2 constants
const DEFAULT_CHALLENGE_WINDOW: u64 = 200; // slots to respond to a challenge
const DEFAULT_SLASH_PERCENT: u64 = 10; // 10% of stake slashed on failure
const MIN_STAKE_PER_GB: u64 = 10_000_000; // 10M spores (0.01 LICN) per GB of capacity
const ADMIN_KEY: &[u8] = b"moss_admin";

/// Storage key for LICN token address (used in call_token_transfer)
const LICN_TOKEN_KEY: &[u8] = b"moss_licn_token";

const MOSS_TOTAL_BYTES_KEY: &[u8] = b"moss_total_bytes";
const MOSS_CHALLENGE_COUNT_KEY: &[u8] = b"moss_challenge_count";
const CHALLENGE_RECORD_SIZE: usize = 25;
const CHALLENGE_STATUS_OPEN: u8 = 0;
const CHALLENGE_STATUS_RESPONDED: u8 = 1;
const CHALLENGE_STATUS_SLASHED: u8 = 2;
const MAX_CHALLENGE_RESPONSE_BYTES: usize = 1_048_576;

// ============================================================================
// REENTRANCY GUARD
// ============================================================================

const RS_REENTRANCY_KEY: &[u8] = b"rs_reentrancy";

fn reentrancy_enter() -> bool {
    if let Some(v) = storage_get(RS_REENTRANCY_KEY) {
        if !v.is_empty() && v[0] == 1 {
            return false;
        }
    }
    storage_set(RS_REENTRANCY_KEY, &[1u8]);
    true
}

fn reentrancy_exit() {
    storage_set(RS_REENTRANCY_KEY, &[0u8]);
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

fn stored_u64(key: &[u8]) -> u64 {
    storage_get(key)
        .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
        .unwrap_or(0)
}

fn increment_counter_saturating(key: &[u8]) {
    let current = stored_u64(key);
    storage_set(key, &u64_to_bytes(current.saturating_add(1)));
}

fn load_licn_token() -> Option<Address> {
    let token_data = storage_get(LICN_TOKEN_KEY);
    if token_data.is_none() || token_data.as_ref().unwrap().len() < 32 {
        return None;
    }
    let mut token = [0u8; 32];
    token.copy_from_slice(&token_data.unwrap()[..32]);
    Some(Address(token))
}

fn unpaid_payout_key(token: Address, recipient: &[u8; 32]) -> Vec<u8> {
    let mut key = b"unpaid_payout:".to_vec();
    key.extend_from_slice(&token.0);
    key.push(b':');
    key.extend_from_slice(recipient);
    key
}

fn record_unpaid_licn_payout(recipient: &[u8; 32], amount: u64) {
    if amount == 0 {
        return;
    }
    let token = load_licn_token().unwrap_or(Address([0u8; 32]));
    let key = unpaid_payout_key(token, recipient);
    let current = stored_u64(&key);
    storage_set(&key, &u64_to_bytes(current.saturating_add(amount)));
}

/// G27-02: Transfer LICN tokens out of the contract to a recipient.
/// Uses self-custody pattern: contract holds tokens at its own address.
/// Returns true on explicit success, false if token not configured or transfer fails.
fn transfer_licn_out(to: &[u8; 32], amount: u64) -> bool {
    if amount == 0 {
        return true;
    }
    let token = match load_licn_token() {
        Some(token) => token,
        None => {
            log_info("LICN token not configured — transfer rejected");
            return false;
        }
    };
    let contract_addr = get_contract_address();
    matches!(
        transfer_token_or_native(token, Address(contract_addr.0), Address(*to), amount),
        Ok(true)
    )
}

// ============================================================================
// STORAGE KEY HELPERS
// ============================================================================

/// Simple hash function for proof-of-retrievability verification.
/// Uses a Merkle-Damgård-style construction with XOR mixing.
fn simple_hash(data: &[u8]) -> [u8; 32] {
    let mut state = [0u8; 32];
    // Initialize with a domain separator
    for (i, b) in b"MossStoragePoR__".iter().enumerate() {
        state[i] = *b;
    }
    // Process input in 32-byte blocks with XOR + rotation mixing
    for chunk in data.chunks(32) {
        for (i, &b) in chunk.iter().enumerate() {
            state[i] ^= b;
        }
        // Mix: rotate state bytes and XOR with index-dependent constant
        let prev = state;
        for i in 0..32 {
            state[i] = prev[(i + 7) % 32]
                .wrapping_add(prev[(i + 13) % 32])
                .wrapping_mul(0x9E)
                .wrapping_add(i as u8);
            state[i] ^= prev[i];
        }
    }
    // Final mixing rounds
    for _ in 0..4 {
        let prev = state;
        for i in 0..32 {
            state[i] = prev[(i + 11) % 32]
                .wrapping_add(prev[(i + 23) % 32])
                .wrapping_mul(0x6D)
                .wrapping_add(prev[i]);
        }
    }
    state
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

fn data_key(hash: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(5 + 64);
    key.extend_from_slice(b"data_");
    key.extend_from_slice(&hex_encode(hash));
    key
}

fn provider_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(9 + 64);
    key.extend_from_slice(b"provider_");
    key.extend_from_slice(&hex_encode(addr));
    key
}

fn reward_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(7 + 64);
    key.extend_from_slice(b"reward_");
    key.extend_from_slice(&hex_encode(addr));
    key
}

fn reward_index_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(11 + 64);
    key.extend_from_slice(b"reward_idx_");
    key.extend_from_slice(&hex_encode(addr));
    key
}

fn reward_position_key(addr: &[u8; 32], data_hash: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(11 + 64 + 1 + 64);
    key.extend_from_slice(b"reward_pos_");
    key.extend_from_slice(&hex_encode(addr));
    key.push(b'_');
    key.extend_from_slice(&hex_encode(data_hash));
    key
}

fn challenge_key(data_hash: &[u8; 32], provider: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(10 + 64 + 1 + 64);
    key.extend_from_slice(b"challenge_");
    key.extend_from_slice(&hex_encode(data_hash));
    key.push(b'_');
    key.extend_from_slice(&hex_encode(provider));
    key
}

fn challenge_challenger_key(data_hash: &[u8; 32], provider: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(21 + 64 + 1 + 64);
    key.extend_from_slice(b"challenge_challenger_");
    key.extend_from_slice(&hex_encode(data_hash));
    key.push(b'_');
    key.extend_from_slice(&hex_encode(provider));
    key
}

fn stake_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(6 + 64);
    key.extend_from_slice(b"stake_");
    key.extend_from_slice(&hex_encode(addr));
    key
}

fn price_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(6 + 64);
    key.extend_from_slice(b"price_");
    key.extend_from_slice(&hex_encode(addr));
    key
}

// ============================================================================
// DATA ENTRY LAYOUT (variable length)
// ============================================================================
//
// Bytes 0..32   : owner (address)
// Bytes 32..40  : size (u64 LE)
// Byte  40      : replication_factor (u8)
// Byte  41      : confirmations_count (u8)
// Bytes 42..50  : expiry_slot (u64 LE)
// Bytes 50..58  : created_slot (u64 LE)
// Byte  58      : provider_count (u8)
// Bytes 59..    : provider addresses (32 bytes each)
//
// Fixed header: 59 bytes + (provider_count * 32)

const DATA_HEADER_SIZE: usize = 59;

fn encode_data_entry(
    owner: &[u8; 32],
    size: u64,
    replication_factor: u8,
    confirmations: u8,
    expiry_slot: u64,
    created_slot: u64,
    providers: &[[u8; 32]],
) -> Vec<u8> {
    let mut data = Vec::with_capacity(DATA_HEADER_SIZE + providers.len() * 32);
    data.extend_from_slice(owner);
    data.extend_from_slice(&u64_to_bytes(size));
    data.push(replication_factor);
    data.push(confirmations);
    data.extend_from_slice(&u64_to_bytes(expiry_slot));
    data.extend_from_slice(&u64_to_bytes(created_slot));
    data.push(providers.len() as u8);
    for p in providers {
        data.extend_from_slice(p);
    }
    data
}

fn decode_data_entry_owner(data: &[u8]) -> [u8; 32] {
    let mut owner = [0u8; 32];
    owner.copy_from_slice(&data[0..32]);
    owner
}

fn decode_data_entry_size(data: &[u8]) -> u64 {
    bytes_to_u64(&data[32..40])
}

fn decode_data_entry_replication(data: &[u8]) -> u8 {
    data[40]
}

fn decode_data_entry_confirmations(data: &[u8]) -> u8 {
    data[41]
}

fn decode_data_entry_expiry(data: &[u8]) -> u64 {
    bytes_to_u64(&data[42..50])
}

fn decode_data_entry_created(data: &[u8]) -> u64 {
    bytes_to_u64(&data[50..58])
}

fn decode_data_entry_provider_count(data: &[u8]) -> u8 {
    data[58]
}

fn data_entry_provider_bytes_valid(data: &[u8]) -> bool {
    if data.len() < DATA_HEADER_SIZE {
        return false;
    }
    let provider_count = decode_data_entry_provider_count(data) as usize;
    provider_count <= MAX_PROVIDERS_PER_ENTRY
        && data.len() >= DATA_HEADER_SIZE + provider_count.saturating_mul(32)
}

fn decode_data_entry_provider(data: &[u8], index: u8) -> [u8; 32] {
    let offset = DATA_HEADER_SIZE + (index as usize) * 32;
    let mut addr = [0u8; 32];
    addr.copy_from_slice(&data[offset..offset + 32]);
    addr
}

fn data_entry_has_provider(data: &[u8], provider: &[u8; 32]) -> bool {
    if !data_entry_provider_bytes_valid(data) {
        return false;
    }
    let prov_count = decode_data_entry_provider_count(data);
    for i in 0..prov_count {
        if decode_data_entry_provider(data, i) == *provider {
            return true;
        }
    }
    false
}

fn reward_index_contains(index_data: &[u8], data_hash: &[u8; 32]) -> bool {
    index_data
        .chunks_exact(32)
        .any(|chunk| chunk == data_hash.as_slice())
}

fn compute_vested_reward(last_reward_slot: u64, reward_until_slot: u64, data_size: u64) -> u64 {
    reward_until_slot
        .saturating_sub(last_reward_slot)
        .saturating_mul(data_size)
        .saturating_mul(REWARD_PER_SLOT_PER_BYTE)
}

// ============================================================================
// PROVIDER INFO LAYOUT
// ============================================================================
//
// Bytes 0..8    : capacity_bytes (u64 LE)
// Bytes 8..16   : used_bytes (u64 LE)
// Bytes 16..24  : stored_count (u64 LE) — number of data entries stored
// Byte  24      : active (u8, 0 or 1)
// Bytes 25..33  : registered_slot (u64 LE)

const PROVIDER_SIZE: usize = 33;

fn encode_provider(
    capacity: u64,
    used: u64,
    stored_count: u64,
    active: bool,
    registered_slot: u64,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(PROVIDER_SIZE);
    data.extend_from_slice(&u64_to_bytes(capacity));
    data.extend_from_slice(&u64_to_bytes(used));
    data.extend_from_slice(&u64_to_bytes(stored_count));
    data.push(if active { 1 } else { 0 });
    data.extend_from_slice(&u64_to_bytes(registered_slot));
    data
}

// ============================================================================
// STORE DATA
// ============================================================================

/// Register a storage request for data.
///
/// Parameters:
///   - owner_ptr: 32-byte owner address
///   - data_hash_ptr: 32-byte hash of the data to store
///   - size: size of data in bytes
///   - replication_factor: desired number of storage providers (1-10)
///   - duration_slots: how many slots the data should be stored
///
/// Returns 0 on success, nonzero on error.
#[no_mangle]
pub extern "C" fn store_data(
    owner_ptr: *const u8,
    data_hash_ptr: *const u8,
    size: u64,
    replication_factor: u8,
    duration_slots: u64,
) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    log_info("Storing data request...");

    let owner_arr = match read_address32(owner_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 98;
        }
    };
    let data_hash = match read_address32(data_hash_ptr) {
        Some(hash) => hash,
        None => {
            reentrancy_exit();
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != owner_arr {
        reentrancy_exit();
        return 200;
    }

    if size == 0 {
        log_info("Data size must be > 0");
        reentrancy_exit();
        return 1;
    }

    if replication_factor == 0 || replication_factor > MAX_REPLICATION {
        log_info("Invalid replication factor");
        reentrancy_exit();
        return 2;
    }

    if duration_slots < MIN_STORAGE_DURATION {
        log_info("Duration too short");
        reentrancy_exit();
        return 3;
    }

    let dk = data_key(&data_hash);
    if storage_get(&dk).is_some() {
        log_info("Data hash already registered");
        reentrancy_exit();
        return 4;
    }

    let count = stored_u64(b"data_count");
    let next_count = match count.checked_add(1) {
        Some(next) => next,
        None => {
            log_info("Data count overflow");
            reentrancy_exit();
            return 7;
        }
    };

    // G27-02: Verify payment for storage cost
    let cost = match (size as u128)
        .saturating_mul(replication_factor as u128)
        .saturating_mul(duration_slots as u128)
        .checked_mul(REWARD_PER_SLOT_PER_BYTE as u128)
    {
        Some(cost) if cost <= u64::MAX as u128 => cost as u64,
        _ => {
            log_info("Storage cost overflow");
            reentrancy_exit();
            return 6;
        }
    };
    if get_value() < cost {
        log_info("Insufficient payment for storage");
        reentrancy_exit();
        return 5;
    }

    let current_slot = get_slot();
    let expiry_slot = match current_slot.checked_add(duration_slots) {
        Some(slot) => slot,
        None => {
            log_info("Expiry slot overflow");
            reentrancy_exit();
            return 6;
        }
    };

    storage_set(b"data_count", &u64_to_bytes(next_count));

    let entry = encode_data_entry(
        &owner_arr,
        size,
        replication_factor,
        0, // no confirmations yet
        expiry_slot,
        current_slot,
        &[], // no providers yet
    );
    storage_set(&dk, &entry);

    // Track total bytes stored
    let tb = stored_u64(MOSS_TOTAL_BYTES_KEY);
    storage_set(MOSS_TOTAL_BYTES_KEY, &u64_to_bytes(tb.saturating_add(size)));

    log_info("Data storage request registered");
    reentrancy_exit();
    0
}

// ============================================================================
// CONFIRM STORAGE
// ============================================================================

/// Provider confirms they are storing the data.
///
/// Parameters:
///   - provider_ptr: 32-byte provider address
///   - data_hash_ptr: 32-byte hash of the data
///
/// Returns 0 on success, nonzero on error.
#[no_mangle]
pub extern "C" fn confirm_storage(provider_ptr: *const u8, data_hash_ptr: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    log_info("Confirming storage...");

    let data_hash = match read_address32(data_hash_ptr) {
        Some(hash) => hash,
        None => {
            reentrancy_exit();
            return 98;
        }
    };
    let provider_arr = match read_address32(provider_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != provider_arr {
        reentrancy_exit();
        return 200;
    }

    // Check data entry exists
    let dk = data_key(&data_hash);
    let entry = match storage_get(&dk) {
        Some(data) => data,
        None => {
            log_info("Data entry not found");
            reentrancy_exit();
            return 1;
        }
    };

    if !data_entry_provider_bytes_valid(&entry) {
        log_info("Corrupt data entry");
        reentrancy_exit();
        return 2;
    }

    // Check not expired
    let current_slot = get_slot();
    let expiry = decode_data_entry_expiry(&entry);
    if current_slot > expiry {
        log_info("Storage request expired");
        reentrancy_exit();
        return 3;
    }

    // Check provider is registered
    let pk = provider_key(&provider_arr);
    let prov_data = match storage_get(&pk) {
        Some(data) => data,
        None => {
            log_info("Provider not registered");
            reentrancy_exit();
            return 4;
        }
    };

    if prov_data.len() < PROVIDER_SIZE || prov_data[24] != 1 {
        log_info("Provider not active");
        reentrancy_exit();
        return 5;
    }

    // Check provider hasn't already confirmed
    let prov_count = decode_data_entry_provider_count(&entry);
    if data_entry_has_provider(&entry, &provider_arr) {
        log_info("Provider already confirmed for this data");
        reentrancy_exit();
        return 6;
    }

    // Check replication limit
    let replication = decode_data_entry_replication(&entry);
    if prov_count >= replication || prov_count as usize >= MAX_PROVIDERS_PER_ENTRY {
        log_info("Replication factor already satisfied");
        reentrancy_exit();
        return 7;
    }

    let capacity = bytes_to_u64(&prov_data[0..8]);
    let used = bytes_to_u64(&prov_data[8..16]);
    let stored_count = bytes_to_u64(&prov_data[16..24]);
    let data_size = decode_data_entry_size(&entry);
    let new_used = match used.checked_add(data_size) {
        Some(next) if next <= capacity => next,
        _ => {
            log_info("Provider capacity exceeded");
            reentrancy_exit();
            return 8;
        }
    };
    let reg_slot = bytes_to_u64(&prov_data[25..33]);

    let mut updated_entry = entry;
    updated_entry[41] = updated_entry[41].saturating_add(1);
    updated_entry[58] = prov_count.saturating_add(1);
    updated_entry.extend_from_slice(&provider_arr);

    let updated_prov = encode_provider(
        capacity,
        new_used,
        stored_count.saturating_add(1),
        true,
        reg_slot,
    );
    let reward_pos_key = reward_position_key(&provider_arr, &data_hash);
    let reward_idx_key = reward_index_key(&provider_arr);
    let mut reward_index = storage_get(&reward_idx_key).unwrap_or_default();
    if !reward_index_contains(&reward_index, &data_hash) {
        reward_index.extend_from_slice(&data_hash);
    }

    storage_set(&dk, &updated_entry);
    storage_set(&pk, &updated_prov);
    storage_set(&reward_pos_key, &u64_to_bytes(current_slot));
    storage_set(&reward_idx_key, &reward_index);

    log_info("Storage confirmed by provider");
    reentrancy_exit();
    0
}

// ============================================================================
// GET STORAGE INFO
// ============================================================================

/// Query storage metadata for a given data hash.
///
/// Parameters:
///   - data_hash_ptr: 32-byte hash of the data
///
/// Returns 0 on success (data set as return data), 1 if not found.
#[no_mangle]
pub extern "C" fn get_storage_info(data_hash_ptr: *const u8) -> u32 {
    let data_hash = match read_address32(data_hash_ptr) {
        Some(hash) => hash,
        None => return 98,
    };

    let dk = data_key(&data_hash);
    match storage_get(&dk) {
        Some(data) => {
            lichen_sdk::set_return_data(&data);
            0
        }
        None => {
            log_info("Data entry not found");
            1
        }
    }
}

// ============================================================================
// REGISTER PROVIDER
// ============================================================================

/// Register as a storage provider.
///
/// Parameters:
///   - provider_ptr: 32-byte provider address
///   - capacity_bytes: total storage capacity in bytes
///
/// Returns 0 on success, nonzero on error.
#[no_mangle]
pub extern "C" fn register_provider(provider_ptr: *const u8, capacity_bytes: u64) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    log_info("Registering storage provider...");

    let provider_arr = match read_address32(provider_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != provider_arr {
        reentrancy_exit();
        return 200;
    }

    if capacity_bytes == 0 {
        log_info("Capacity must be > 0");
        reentrancy_exit();
        return 1;
    }

    let pk = provider_key(&provider_arr);
    if storage_get(&pk).is_some() {
        log_info("Provider already registered");
        reentrancy_exit();
        return 2;
    }

    let current_slot = get_slot();
    let prov_data = encode_provider(capacity_bytes, 0, 0, true, current_slot);
    storage_set(&pk, &prov_data);

    log_info("Storage provider registered");
    reentrancy_exit();
    0
}

// ============================================================================
// CLAIM STORAGE REWARDS
// ============================================================================

/// Provider claims accumulated storage rewards.
///
/// Parameters:
///   - provider_ptr: 32-byte provider address
///
/// Returns 0 on success (reward amount set as return data), nonzero on error.
#[no_mangle]
pub extern "C" fn claim_storage_rewards(provider_ptr: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    log_info("Claiming storage rewards...");

    let provider_arr = match read_address32(provider_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != provider_arr {
        reentrancy_exit();
        return 200;
    }

    let current_slot = get_slot();
    let rk = reward_key(&provider_arr);
    let mut reward = storage_get(&rk).map(|d| bytes_to_u64(&d)).unwrap_or(0);
    let reward_idx_key = reward_index_key(&provider_arr);
    let reward_index = storage_get(&reward_idx_key).unwrap_or_default();
    let mut reward_updates = Vec::new();

    for hash_chunk in reward_index.chunks_exact(32) {
        let mut data_hash = [0u8; 32];
        data_hash.copy_from_slice(hash_chunk);

        let entry = match storage_get(&data_key(&data_hash)) {
            Some(data) if data.len() >= DATA_HEADER_SIZE => data,
            _ => continue,
        };

        if !data_entry_has_provider(&entry, &provider_arr) {
            continue;
        }

        let reward_pos_key = reward_position_key(&provider_arr, &data_hash);
        let last_reward_slot = storage_get(&reward_pos_key)
            .map(|d| {
                if d.len() >= 8 {
                    bytes_to_u64(&d)
                } else {
                    current_slot
                }
            })
            .unwrap_or(current_slot);
        let reward_until_slot = decode_data_entry_expiry(&entry).min(current_slot);
        if reward_until_slot <= last_reward_slot {
            continue;
        }

        reward = reward.saturating_add(compute_vested_reward(
            last_reward_slot,
            reward_until_slot,
            decode_data_entry_size(&entry),
        ));
        reward_updates.push((reward_pos_key, reward_until_slot));
    }

    if reward == 0 {
        log_info("No rewards to claim");
        reentrancy_exit();
        return 1;
    }

    // G27-02: Transfer reward tokens to provider
    if !transfer_licn_out(&provider_arr, reward) {
        log_info("Reward transfer failed");
        reentrancy_exit();
        return 2;
    }

    storage_set(&rk, &u64_to_bytes(0));
    for (reward_pos_key, reward_until_slot) in reward_updates {
        storage_set(&reward_pos_key, &u64_to_bytes(reward_until_slot));
    }

    // Return reward amount
    lichen_sdk::set_return_data(&u64_to_bytes(reward));

    log_info("Storage rewards claimed");
    reentrancy_exit();
    0
}

// ============================================================================
// v2: ADMIN
// ============================================================================

/// Initialize admin. Called once.
#[no_mangle]
pub extern "C" fn initialize(admin_ptr: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let admin = match read_address32(admin_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != admin {
        reentrancy_exit();
        return 200;
    }

    if storage_get(ADMIN_KEY).is_some() {
        reentrancy_exit();
        return 1;
    }
    storage_set(ADMIN_KEY, &admin);
    storage_set(b"challenge_window", &u64_to_bytes(DEFAULT_CHALLENGE_WINDOW));
    storage_set(b"slash_percent", &u64_to_bytes(DEFAULT_SLASH_PERCENT));
    log_info("Moss Storage v2 initialized");
    reentrancy_exit();
    0
}

/// G27-02: Set LICN token address for self-custody transfers. Admin only.
/// Returns: 0 success, 1 not admin, 2 already configured
#[no_mangle]
pub extern "C" fn set_licn_token(caller_ptr: *const u8, token_ptr: *const u8) -> u32 {
    let caller = match read_address32(caller_ptr) {
        Some(addr) => addr,
        None => return 98,
    };
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }
    match storage_get(ADMIN_KEY) {
        Some(admin) if caller[..] == admin[..] => {}
        _ => {
            return 1;
        }
    }
    let token = match read_address32(token_ptr) {
        Some(addr) => addr,
        None => return 98,
    };
    if storage_get(LICN_TOKEN_KEY)
        .map(|data| data.len() == 32)
        .unwrap_or(false)
    {
        log_info("LICN token already configured");
        return 2;
    }
    storage_set(LICN_TOKEN_KEY, &token);
    log_info("LICN token address configured");
    0
}

/// Set challenge response window (admin only).
#[no_mangle]
pub extern "C" fn set_challenge_window(caller_ptr: *const u8, window_slots: u64) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let caller = match read_address32(caller_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    match storage_get(ADMIN_KEY) {
        Some(admin) if caller[..] == admin[..] => {}
        _ => {
            reentrancy_exit();
            return 2;
        }
    }
    if window_slots < 10 {
        reentrancy_exit();
        return 3;
    }
    storage_set(b"challenge_window", &u64_to_bytes(window_slots));
    reentrancy_exit();
    0
}

/// Set slash percentage (admin only).
#[no_mangle]
pub extern "C" fn set_slash_percent(caller_ptr: *const u8, percent: u64) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let caller = match read_address32(caller_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    match storage_get(ADMIN_KEY) {
        Some(admin) if caller[..] == admin[..] => {}
        _ => {
            reentrancy_exit();
            return 2;
        }
    }
    if percent > 100 {
        reentrancy_exit();
        return 3;
    }
    storage_set(b"slash_percent", &u64_to_bytes(percent));
    reentrancy_exit();
    0
}

// ============================================================================
// v2: PROVIDER STAKING & PRICING
// ============================================================================

/// Provider stakes LICN collateral. Must be called after register_provider.
/// Stake amount must be >= MIN_STAKE_PER_GB * (capacity_bytes / 1GB).
#[no_mangle]
pub extern "C" fn stake_collateral(provider_ptr: *const u8, amount: u64) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let provider_arr = match read_address32(provider_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != provider_arr {
        reentrancy_exit();
        return 200;
    }

    // Verify provider is registered
    let pk = provider_key(&provider_arr);
    let prov_data = match storage_get(&pk) {
        Some(data) if data.len() >= PROVIDER_SIZE && data[24] == 1 => data,
        _ => {
            log_info("Provider not registered or not active");
            reentrancy_exit();
            return 1;
        }
    };

    let capacity = bytes_to_u64(&prov_data[0..8]);
    let gb = capacity.saturating_add(1_073_741_823) / 1_073_741_824; // round up to GB
    let min_stake = gb.saturating_mul(MIN_STAKE_PER_GB);
    if amount < min_stake {
        log_info("Insufficient stake for capacity");
        reentrancy_exit();
        return 2;
    }

    // G27-02: Verify provider attached sufficient LICN
    if get_value() < amount {
        log_info("Insufficient LICN attached for staking");
        reentrancy_exit();
        return 3;
    }

    let sk = stake_key(&provider_arr);
    let prev_stake = storage_get(&sk).map(|d| bytes_to_u64(&d)).unwrap_or(0);
    storage_set(&sk, &u64_to_bytes(prev_stake.saturating_add(amount)));

    log_info("Collateral staked");
    reentrancy_exit();
    0
}

/// Provider sets custom price per byte per slot (in spores).
#[no_mangle]
pub extern "C" fn set_storage_price(provider_ptr: *const u8, price_per_byte_per_slot: u64) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let provider_arr = match read_address32(provider_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 98;
        }
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != provider_arr {
        reentrancy_exit();
        return 200;
    }

    // Verify registered
    let pk = provider_key(&provider_arr);
    if storage_get(&pk).is_none() {
        reentrancy_exit();
        return 1;
    }

    let prk = price_key(&provider_arr);
    storage_set(&prk, &u64_to_bytes(price_per_byte_per_slot));
    log_info("Storage price set");
    reentrancy_exit();
    0
}

/// Get provider's custom price. Returns REWARD_PER_SLOT_PER_BYTE if no custom price set.
#[no_mangle]
pub extern "C" fn get_storage_price(provider_ptr: *const u8) -> u64 {
    let provider_arr = match read_address32(provider_ptr) {
        Some(addr) => addr,
        None => return REWARD_PER_SLOT_PER_BYTE,
    };

    storage_get(&price_key(&provider_arr))
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(REWARD_PER_SLOT_PER_BYTE)
}

/// Get provider's staked collateral.
#[no_mangle]
pub extern "C" fn get_provider_stake(provider_ptr: *const u8) -> u64 {
    let provider_arr = match read_address32(provider_ptr) {
        Some(addr) => addr,
        None => return 0,
    };

    storage_get(&stake_key(&provider_arr))
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0)
}

// ============================================================================
// v2: PROOF-OF-STORAGE CHALLENGES
// ============================================================================

/// Issue a proof-of-storage challenge to a provider for specific data.
/// Anyone can issue challenges (permissionless — keeps providers honest).
///
/// Challenge layout: [issued_slot(8), deadline_slot(8), nonce(8), answered(1)] = 25 bytes.
/// Challenger identity is stored separately under `challenge_challenger_{hash}_{provider}`.
///
/// Parameters:
///   - data_hash_ptr: 32-byte hash of data to challenge
///   - provider_ptr: 32-byte provider address
///   - nonce: random nonce for the challenge
///
/// Returns 0 on success.
#[no_mangle]
pub extern "C" fn issue_challenge(
    data_hash_ptr: *const u8,
    provider_ptr: *const u8,
    nonce: u64,
) -> u32 {
    let hash_arr = match read_address32(data_hash_ptr) {
        Some(hash) => hash,
        None => return 98,
    };
    let prov_arr = match read_address32(provider_ptr) {
        Some(addr) => addr,
        None => return 98,
    };

    // Verify data entry exists and provider is listed
    let dk = data_key(&hash_arr);
    let entry = match storage_get(&dk) {
        Some(data) if data_entry_provider_bytes_valid(&data) => data,
        _ => {
            return 1;
        }
    };

    // Check data not expired
    let current_slot = get_slot();
    let expiry = decode_data_entry_expiry(&entry);
    if current_slot > expiry {
        return 2;
    }

    // Verify provider is listed in this data entry
    let prov_count = decode_data_entry_provider_count(&entry);
    let mut found = false;
    for i in 0..prov_count {
        if decode_data_entry_provider(&entry, i) == prov_arr {
            found = true;
            break;
        }
    }
    if !found {
        return 3;
    }

    // Check no active challenge already
    let ck = challenge_key(&hash_arr, &prov_arr);
    if let Some(chal) = storage_get(&ck) {
        if chal.len() >= CHALLENGE_RECORD_SIZE && chal[24] == CHALLENGE_STATUS_OPEN {
            // Open challenge exists — check if deadline passed
            let deadline = bytes_to_u64(&chal[8..16]);
            if current_slot <= deadline {
                log_info("Active challenge already pending");
                return 4;
            }
        }
    }

    // Create challenge
    let window = storage_get(b"challenge_window")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_CHALLENGE_WINDOW);
    let deadline = current_slot.saturating_add(window);

    let mut chal = Vec::with_capacity(CHALLENGE_RECORD_SIZE);
    chal.extend_from_slice(&u64_to_bytes(current_slot)); // issued_slot
    chal.extend_from_slice(&u64_to_bytes(deadline)); // deadline_slot
    chal.extend_from_slice(&u64_to_bytes(nonce)); // nonce
    chal.push(CHALLENGE_STATUS_OPEN);

    storage_set(&ck, &chal);
    storage_set(
        &challenge_challenger_key(&hash_arr, &prov_arr),
        &get_caller().0,
    );

    increment_counter_saturating(MOSS_CHALLENGE_COUNT_KEY);

    log_info("Storage challenge issued");
    0
}

/// Provider responds to a proof-of-storage challenge.
/// The response pointer must reference the full committed data bytes.
/// A response is valid only when `simple_hash(response_bytes) == data_hash`.
///
/// Parameters:
///   - provider_ptr: 32-byte provider address
///   - data_hash_ptr: 32-byte data hash
///   - response_ptr: full challenged data bytes; expected length is the stored data size
///
/// Returns 0 on success.
#[no_mangle]
pub extern "C" fn respond_challenge(
    provider_ptr: *const u8,
    data_hash_ptr: *const u8,
    response_ptr: *const u8,
) -> u32 {
    let prov_arr = match read_address32(provider_ptr) {
        Some(addr) => addr,
        None => return 98,
    };
    let hash_arr = match read_address32(data_hash_ptr) {
        Some(hash) => hash,
        None => return 98,
    };
    // Verify caller matches provider
    let real_caller = get_caller();
    if real_caller.0 != prov_arr {
        log_info("respond_challenge rejected: caller mismatch");
        return 5;
    }

    // Load challenge
    let ck = challenge_key(&hash_arr, &prov_arr);
    let mut chal = match storage_get(&ck) {
        Some(data) if data.len() >= CHALLENGE_RECORD_SIZE => data,
        _ => {
            return 1;
        }
    };

    if chal[24] != CHALLENGE_STATUS_OPEN {
        log_info("Challenge already answered");
        return 2;
    }

    // Check deadline
    let current_slot = get_slot();
    let deadline = bytes_to_u64(&chal[8..16]);
    if current_slot > deadline {
        log_info("Challenge response too late");
        return 3;
    }

    let entry = match storage_get(&data_key(&hash_arr)) {
        Some(data) if data.len() >= DATA_HEADER_SIZE => data,
        _ => {
            log_info("Challenge data entry missing");
            return 1;
        }
    };

    let data_size_u64 = decode_data_entry_size(&entry);
    if data_size_u64 == 0 || data_size_u64 > MAX_CHALLENGE_RESPONSE_BYTES as u64 {
        log_info("Invalid committed data size");
        return 4;
    }
    if response_ptr.is_null() {
        log_info("Null challenge response");
        return 6;
    }
    let data_size = data_size_u64 as usize;

    let response = unsafe { core::slice::from_raw_parts(response_ptr, data_size) };
    if simple_hash(response) != hash_arr {
        log_info("Invalid proof-of-retrievability: commitment mismatch");
        return 4;
    }

    // Mark as answered
    chal[24] = CHALLENGE_STATUS_RESPONDED;
    storage_set(&ck, &chal);
    log_info("Challenge responded successfully");
    0
}

/// Slash a provider that failed to respond to a challenge.
/// Anyone can call after the challenge deadline has passed.
///
/// Parameters:
///   - data_hash_ptr: 32-byte data hash
///   - provider_ptr: 32-byte provider address
///
/// Returns 0 on success (slashed amount set as return data).
#[no_mangle]
pub extern "C" fn slash_provider(data_hash_ptr: *const u8, provider_ptr: *const u8) -> u32 {
    let hash_arr = match read_address32(data_hash_ptr) {
        Some(hash) => hash,
        None => return 98,
    };
    let prov_arr = match read_address32(provider_ptr) {
        Some(addr) => addr,
        None => return 98,
    };

    // Load challenge
    let ck = challenge_key(&hash_arr, &prov_arr);
    let chal = match storage_get(&ck) {
        Some(data) if data.len() >= CHALLENGE_RECORD_SIZE => data,
        _ => {
            return 1;
        }
    };

    // Must be unanswered
    if chal[24] != CHALLENGE_STATUS_OPEN {
        log_info("Challenge was answered — no slash");
        return 2;
    }

    // Deadline must have passed
    let current_slot = get_slot();
    let deadline = bytes_to_u64(&chal[8..16]);
    if current_slot <= deadline {
        log_info("Challenge deadline not passed yet");
        return 3;
    }

    // Calculate slash amount
    let slash_pct = storage_get(b"slash_percent")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_SLASH_PERCENT)
        .min(100);

    let sk = stake_key(&prov_arr);
    let stake = storage_get(&sk).map(|d| bytes_to_u64(&d)).unwrap_or(0);

    let slash_amount =
        ((stake as u128).saturating_mul(slash_pct as u128) / 100).min(u64::MAX as u128) as u64;
    if slash_amount > 0 {
        storage_set(&sk, &u64_to_bytes(stake.saturating_sub(slash_amount)));

        // Redistribute slashed tokens — 50% to the recorded challenger, 50% to treasury.
        let half = slash_amount / 2;
        let mut treasury_amount = slash_amount;
        if let Some(challenger_data) = storage_get(&challenge_challenger_key(&hash_arr, &prov_arr))
        {
            if challenger_data.len() >= 32 && half > 0 {
                let mut challenger = [0u8; 32];
                challenger.copy_from_slice(&challenger_data[..32]);
                if !transfer_licn_out(&challenger, half) {
                    record_unpaid_licn_payout(&challenger, half);
                }
                treasury_amount = slash_amount - half;
            }
        }

        if treasury_amount > 0 {
            if let Some(admin_data) = storage_get(ADMIN_KEY) {
                if admin_data.len() >= 32 {
                    let mut treasury = [0u8; 32];
                    treasury.copy_from_slice(&admin_data[..32]);
                    if !transfer_licn_out(&treasury, treasury_amount) {
                        record_unpaid_licn_payout(&treasury, treasury_amount);
                    }
                }
            }
        }
    }

    // Mark challenge as answered (so it can't be double-slashed)
    let mut updated_chal = chal;
    updated_chal[24] = CHALLENGE_STATUS_SLASHED;
    storage_set(&ck, &updated_chal);

    lichen_sdk::set_return_data(&u64_to_bytes(slash_amount));
    log_info("Provider slashed for failed challenge");
    0
}

/// Get moss storage stats [data_count(8), total_bytes(8), challenge_count(8)]
#[no_mangle]
pub extern "C" fn get_platform_stats() -> u32 {
    let mut buf = Vec::with_capacity(24);
    buf.extend_from_slice(&u64_to_bytes(
        storage_get(b"data_count")
            .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
            .unwrap_or(0),
    ));
    buf.extend_from_slice(&u64_to_bytes(
        storage_get(MOSS_TOTAL_BYTES_KEY)
            .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
            .unwrap_or(0),
    ));
    buf.extend_from_slice(&u64_to_bytes(
        storage_get(MOSS_CHALLENGE_COUNT_KEY)
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
    use lichen_sdk::test_mock;

    fn setup() {
        test_mock::reset();
    }

    fn unpaid_key(token: &[u8; 32], recipient: &[u8; 32]) -> Vec<u8> {
        let mut key = b"unpaid_payout:".to_vec();
        key.extend_from_slice(token);
        key.push(b':');
        key.extend_from_slice(recipient);
        key
    }

    fn configure_licn_transfers(admin: [u8; 32]) {
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let licn_token = [0xDD; 32];
        set_licn_token(admin.as_ptr(), licn_token.as_ptr());
        test_mock::set_cross_call_response(Some(alloc::vec![1u8]));
    }

    /// G27-02: Configure admin + LICN token + mock cross-contract transfers
    /// so claim_storage_rewards can succeed in unit tests.
    fn enable_reward_transfers() {
        configure_licn_transfers([9u8; 32]);
    }

    #[test]
    fn test_store_data() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let data_hash = [0xAA; 32];

        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        test_mock::set_value(153_600_000); // cost = 1024 * 3 * 5000 * 10
        let result = store_data(
            owner.as_ptr(),
            data_hash.as_ptr(),
            1024, // 1KB
            3,    // 3x replication
            5000, // 5000 slots duration
        );
        assert_eq!(result, 0);

        // Verify data entry exists
        let dk = data_key(&data_hash);
        let entry = test_mock::get_storage(&dk).unwrap();
        assert!(entry.len() >= DATA_HEADER_SIZE);
        assert_eq!(decode_data_entry_owner(&entry), owner);
        assert_eq!(decode_data_entry_size(&entry), 1024);
        assert_eq!(decode_data_entry_replication(&entry), 3);
        assert_eq!(decode_data_entry_confirmations(&entry), 0);
        assert_eq!(decode_data_entry_expiry(&entry), 5100); // 100 + 5000
        assert_eq!(decode_data_entry_provider_count(&entry), 0);

        // Verify data count incremented
        let count = test_mock::get_storage(b"data_count").unwrap();
        assert_eq!(bytes_to_u64(&count), 1);
    }

    #[test]
    fn test_store_data_duplicate_fails() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let data_hash = [0xBB; 32];

        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        test_mock::set_value(20_480_000); // cost = 512 * 2 * 2000 * 10
        store_data(owner.as_ptr(), data_hash.as_ptr(), 512, 2, 2000);
        test_mock::set_value(2_560_000); // cost = 256 * 1 * 1000 * 10
        let result = store_data(owner.as_ptr(), data_hash.as_ptr(), 256, 1, 1000);
        assert_eq!(result, 4); // already registered
    }

    #[test]
    fn test_confirm_storage() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let data_hash = [0xCC; 32];
        let provider_addr = [2u8; 32];

        // Register provider first
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        let reg_result = register_provider(provider_addr.as_ptr(), 1_000_000);
        assert_eq!(reg_result, 0);

        // Store data
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        test_mock::set_value(153_600_000); // cost = 1024 * 3 * 5000 * 10
        store_data(owner.as_ptr(), data_hash.as_ptr(), 1024, 3, 5000);

        // Confirm storage
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        let result = confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());
        assert_eq!(result, 0);

        // Verify confirmation recorded
        let dk = data_key(&data_hash);
        let entry = test_mock::get_storage(&dk).unwrap();
        assert_eq!(decode_data_entry_confirmations(&entry), 1);
        assert_eq!(decode_data_entry_provider_count(&entry), 1);

        // Verify provider stats updated
        let pk = provider_key(&provider_addr);
        let prov = test_mock::get_storage(&pk).unwrap();
        let used = bytes_to_u64(&prov[8..16]);
        assert_eq!(used, 1024);
        let stored = bytes_to_u64(&prov[16..24]);
        assert_eq!(stored, 1);

        // Verify reward vesting starts at confirmation time rather than front-loading.
        let rk = reward_key(&provider_addr);
        assert!(test_mock::get_storage(&rk).is_none());

        let reward_pos =
            test_mock::get_storage(&reward_position_key(&provider_addr, &data_hash)).unwrap();
        assert_eq!(bytes_to_u64(&reward_pos), 100);

        let reward_index = test_mock::get_storage(&reward_index_key(&provider_addr)).unwrap();
        assert_eq!(reward_index.as_slice(), &data_hash);
    }

    #[test]
    fn test_confirm_storage_capacity_failure_is_atomic() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let data_hash = [0xCE; 32];
        let provider_addr = [2u8; 32];

        test_mock::set_caller(provider_addr);
        assert_eq!(register_provider(provider_addr.as_ptr(), 1_000), 0);

        test_mock::set_caller(owner);
        test_mock::set_value(51_200_000);
        assert_eq!(
            store_data(owner.as_ptr(), data_hash.as_ptr(), 1024, 1, 5000),
            0
        );

        test_mock::set_caller(provider_addr);
        assert_eq!(
            confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr()),
            8
        );

        let entry = test_mock::get_storage(&data_key(&data_hash)).unwrap();
        assert_eq!(decode_data_entry_confirmations(&entry), 0);
        assert_eq!(decode_data_entry_provider_count(&entry), 0);

        let prov = test_mock::get_storage(&provider_key(&provider_addr)).unwrap();
        assert_eq!(bytes_to_u64(&prov[8..16]), 0);
        assert_eq!(bytes_to_u64(&prov[16..24]), 0);
        assert!(test_mock::get_storage(&reward_index_key(&provider_addr)).is_none());
        assert!(test_mock::get_storage(&reward_position_key(&provider_addr, &data_hash)).is_none());
    }

    #[test]
    fn test_get_storage_info() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 50);

        let owner = [1u8; 32];
        let data_hash = [0xDD; 32];

        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        test_mock::set_value(122_880_000); // cost = 2048 * 2 * 3000 * 10
        store_data(owner.as_ptr(), data_hash.as_ptr(), 2048, 2, 3000);

        let result = get_storage_info(data_hash.as_ptr());
        assert_eq!(result, 0);

        let ret = test_mock::get_return_data();
        assert!(ret.len() >= DATA_HEADER_SIZE);
        assert_eq!(decode_data_entry_size(&ret), 2048);
    }

    #[test]
    fn test_get_storage_info_not_found() {
        setup();
        let unknown_hash = [0xFF; 32];
        let result = get_storage_info(unknown_hash.as_ptr());
        assert_eq!(result, 1);
    }

    #[test]
    fn test_register_provider() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 10);

        let provider_addr = [5u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        let result = register_provider(provider_addr.as_ptr(), 500_000);
        assert_eq!(result, 0);

        let pk = provider_key(&provider_addr);
        let prov = test_mock::get_storage(&pk).unwrap();
        assert_eq!(prov.len(), PROVIDER_SIZE);
        let capacity = bytes_to_u64(&prov[0..8]);
        assert_eq!(capacity, 500_000);
        assert_eq!(prov[24], 1); // active
    }

    #[test]
    fn test_claim_storage_rewards() {
        setup();
        enable_reward_transfers();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let data_hash = [0xEE; 32];
        let provider_addr = [2u8; 32];

        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_000_000);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        test_mock::set_value(5_000_000); // cost = 100 * 1 * 5000 * 10
        store_data(owner.as_ptr(), data_hash.as_ptr(), 100, 1, 5000);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());

        assert_eq!(claim_storage_rewards(provider_addr.as_ptr()), 1);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 150);

        let result = claim_storage_rewards(provider_addr.as_ptr());
        assert_eq!(result, 0);

        let ret = test_mock::get_return_data();
        let reward = bytes_to_u64(&ret);
        assert_eq!(reward, 50_000);

        // Reward should now be zero
        let rk = reward_key(&provider_addr);
        let stored = test_mock::get_storage(&rk).unwrap();
        assert_eq!(bytes_to_u64(&stored), 0);

        let reward_pos =
            test_mock::get_storage(&reward_position_key(&provider_addr, &data_hash)).unwrap();
        assert_eq!(bytes_to_u64(&reward_pos), 150);
    }

    #[test]
    fn test_claim_storage_rewards_preserves_vesting_when_transfer_fails() {
        setup();
        enable_reward_transfers();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let data_hash = [0xEF; 32];
        let provider_addr = [2u8; 32];

        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_000_000);
        test_mock::set_caller(owner);
        test_mock::set_value(5_000_000);
        store_data(owner.as_ptr(), data_hash.as_ptr(), 100, 1, 5000);
        test_mock::set_caller(provider_addr);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());

        test_mock::SLOT.with(|s| *s.borrow_mut() = 150);
        test_mock::set_cross_call_should_fail(true);
        assert_eq!(claim_storage_rewards(provider_addr.as_ptr()), 2);

        let reward_pos =
            test_mock::get_storage(&reward_position_key(&provider_addr, &data_hash)).unwrap();
        assert_eq!(bytes_to_u64(&reward_pos), 100);

        test_mock::set_cross_call_should_fail(false);
        assert_eq!(claim_storage_rewards(provider_addr.as_ptr()), 0);
        let reward = bytes_to_u64(&test_mock::get_return_data());
        assert_eq!(reward, 50_000);

        let reward_pos =
            test_mock::get_storage(&reward_position_key(&provider_addr, &data_hash)).unwrap();
        assert_eq!(bytes_to_u64(&reward_pos), 150);
    }

    #[test]
    fn test_claim_storage_rewards_preserves_vesting_on_false_transfer_status() {
        setup();
        enable_reward_transfers();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let data_hash = [0xE1; 32];
        let provider_addr = [2u8; 32];

        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_000_000);
        test_mock::set_caller(owner);
        test_mock::set_value(5_000_000);
        store_data(owner.as_ptr(), data_hash.as_ptr(), 100, 1, 5000);
        test_mock::set_caller(provider_addr);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());

        test_mock::SLOT.with(|s| *s.borrow_mut() = 150);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(claim_storage_rewards(provider_addr.as_ptr()), 2);

        let reward_pos =
            test_mock::get_storage(&reward_position_key(&provider_addr, &data_hash)).unwrap();
        assert_eq!(bytes_to_u64(&reward_pos), 100);
    }

    // =============================================
    // v2 TESTS
    // =============================================

    #[test]
    fn test_initialize_admin() {
        setup();
        let admin = [9u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(initialize(admin.as_ptr()), 1); // double init
    }

    #[test]
    fn test_stake_collateral() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 10);
        let provider_addr = [2u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_073_741_824); // 1 GB
        test_mock::set_value(10_000_000);
        let result = stake_collateral(provider_addr.as_ptr(), 10_000_000);
        assert_eq!(result, 0);
        assert_eq!(get_provider_stake(provider_addr.as_ptr()), 10_000_000);
    }

    #[test]
    fn test_stake_too_low() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 10);
        let provider_addr = [2u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 2_000_000_000); // ~2 GB
                                                                  // Needs >= 2M stake (2 * MIN_STAKE_PER_GB)
        assert_eq!(stake_collateral(provider_addr.as_ptr(), 500_000), 2);
    }

    #[test]
    fn test_set_storage_price() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 10);
        let provider_addr = [2u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_000_000);
        assert_eq!(set_storage_price(provider_addr.as_ptr(), 5), 0);
        assert_eq!(get_storage_price(provider_addr.as_ptr()), 5);
    }

    #[test]
    fn test_storage_price_default() {
        setup();
        let unknown = [0xFF; 32];
        assert_eq!(
            get_storage_price(unknown.as_ptr()),
            REWARD_PER_SLOT_PER_BYTE
        );
    }

    #[test]
    fn test_issue_and_respond_challenge() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [9u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let owner = [1u8; 32];
        let payload = [0xAC; 64];
        let data_hash = simple_hash(&payload);
        let provider_addr = [2u8; 32];
        let challenger = [7u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_000_000);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        test_mock::set_value(9_600_000); // cost = 64 * 3 * 5000 * 10
        store_data(
            owner.as_ptr(),
            data_hash.as_ptr(),
            payload.len() as u64,
            3,
            5000,
        );
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());

        // Issue challenge
        test_mock::set_caller(challenger);
        let result = issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42);
        assert_eq!(result, 0);
        assert_eq!(
            test_mock::get_storage(&challenge_challenger_key(&data_hash, &provider_addr)).unwrap(),
            challenger.to_vec()
        );

        // Respond to challenge
        test_mock::set_caller(provider_addr);
        let result =
            respond_challenge(provider_addr.as_ptr(), data_hash.as_ptr(), payload.as_ptr());
        assert_eq!(result, 0);
    }

    #[test]
    fn test_challenge_duplicate_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller([9u8; 32]);
        initialize([9u8; 32].as_ptr());

        let owner = [1u8; 32];
        let data_hash = [0xCC; 32];
        let provider_addr = [2u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_000_000);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        test_mock::set_value(51_200_000); // cost = 1024 * 1 * 5000 * 10
        store_data(owner.as_ptr(), data_hash.as_ptr(), 1024, 1, 5000);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());

        assert_eq!(
            issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42),
            0
        );
        // Same challenge while deadline active
        assert_eq!(
            issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 99),
            4
        );
    }

    #[test]
    fn test_slash_unanswered_challenge() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        let challenger = [9u8; 32];
        configure_licn_transfers(challenger);

        let owner = [1u8; 32];
        let data_hash = [0xCC; 32];
        let provider_addr = [2u8; 32];
        let slash_caller = [8u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_073_741_824);
        test_mock::set_value(51_200_000); // covers stake(10M) and store_data cost(51.2M)
        stake_collateral(provider_addr.as_ptr(), 10_000_000);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        store_data(owner.as_ptr(), data_hash.as_ptr(), 1024, 1, 5000);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());
        test_mock::set_caller(challenger);
        issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42);

        // Advance past deadline
        test_mock::SLOT.with(|s| *s.borrow_mut() = 400);

        test_mock::set_caller(slash_caller);
        let result = slash_provider(data_hash.as_ptr(), provider_addr.as_ptr());
        assert_eq!(result, 0);

        // Check stake reduced by 10%
        let stake = get_provider_stake(provider_addr.as_ptr());
        assert_eq!(stake, 9_000_000);

        // Return data should have slash amount
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 1_000_000);

        let (_, function, args, value) = test_mock::get_last_cross_call()
            .expect("slash should perform recorded challenger payout");
        assert_eq!(function, "transfer");
        assert_eq!(value, 0);
        let mut recipient = [0u8; 32];
        recipient.copy_from_slice(&args[32..64]);
        assert_eq!(recipient, challenger);
        assert_ne!(recipient, slash_caller);
    }

    #[test]
    fn test_slash_answered_challenge_fails() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller([9u8; 32]);
        initialize([9u8; 32].as_ptr());

        let owner = [1u8; 32];
        let payload = [0xBD; 64];
        let data_hash = simple_hash(&payload);
        let provider_addr = [2u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_073_741_824);
        stake_collateral(provider_addr.as_ptr(), 1_000_000);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        test_mock::set_value(3_200_000); // cost = 64 * 1 * 5000 * 10
        store_data(
            owner.as_ptr(),
            data_hash.as_ptr(),
            payload.len() as u64,
            1,
            5000,
        );
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());
        issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42);

        // Respond correctly
        respond_challenge(provider_addr.as_ptr(), data_hash.as_ptr(), payload.as_ptr());

        // Advance past deadline
        test_mock::SLOT.with(|s| *s.borrow_mut() = 400);

        // Slash should fail because challenge was answered
        assert_eq!(
            slash_provider(data_hash.as_ptr(), provider_addr.as_ptr()),
            2
        );
    }

    #[test]
    fn test_slash_before_deadline_fails() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller([9u8; 32]);
        initialize([9u8; 32].as_ptr());

        let owner = [1u8; 32];
        let data_hash = [0xCC; 32];
        let provider_addr = [2u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_000_000);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        test_mock::set_value(51_200_000); // cost = 1024 * 1 * 5000 * 10
        store_data(owner.as_ptr(), data_hash.as_ptr(), 1024, 1, 5000);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());
        issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42);

        // Still within deadline
        assert_eq!(
            slash_provider(data_hash.as_ptr(), provider_addr.as_ptr()),
            3
        );
    }

    #[test]
    fn test_set_challenge_window_admin_only() {
        setup();
        let admin = [9u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        assert_eq!(set_challenge_window(admin.as_ptr(), 500), 0);
        let other = [8u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(other);
        assert_eq!(set_challenge_window(other.as_ptr(), 500), 2);
    }

    #[test]
    fn test_challenge_wrong_preimage_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller([9u8; 32]);
        initialize([9u8; 32].as_ptr());

        let owner = [1u8; 32];
        let payload = [0xC1; 64];
        let data_hash = simple_hash(&payload);
        let wrong_payload = [0u8; 64];
        let provider_addr = [2u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_000_000);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        test_mock::set_value(3_200_000); // cost = 64 * 1 * 5000 * 10
        store_data(
            owner.as_ptr(),
            data_hash.as_ptr(),
            payload.len() as u64,
            1,
            5000,
        );
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());
        issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42);

        // Wrong preimage = invalid
        assert_eq!(
            respond_challenge(
                provider_addr.as_ptr(),
                data_hash.as_ptr(),
                wrong_payload.as_ptr()
            ),
            4
        );
    }

    // ====================================================================
    // G27-02 TESTS: Financial wiring
    // ====================================================================

    #[test]
    fn test_g27_store_data_requires_payment() {
        // store_data must fail when get_value() < cost (no LICN attached)
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        let owner = [1u8; 32];
        let data_hash = [0xF1; 32];
        test_mock::set_caller(owner);
        // No set_value → get_value() returns 0
        let result = store_data(owner.as_ptr(), data_hash.as_ptr(), 1024, 1, 5000);
        assert_eq!(result, 5); // insufficient payment
    }

    #[test]
    fn test_g27_stake_requires_get_value() {
        // stake_collateral must fail when get_value() < amount
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 10);
        let provider = [2u8; 32];
        test_mock::set_caller(provider);
        register_provider(provider.as_ptr(), 1_073_741_824); // 1 GB
                                                             // No set_value → get_value() returns 0
        let result = stake_collateral(provider.as_ptr(), 10_000_000);
        assert_eq!(result, 3); // insufficient LICN
    }

    #[test]
    fn test_g27_claim_rewards_triggers_transfer() {
        // claim_storage_rewards must attempt token transfer via cross-contract call
        setup();
        enable_reward_transfers();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        let owner = [1u8; 32];
        let data_hash = [0xF2; 32];
        let provider = [2u8; 32];
        test_mock::set_caller(provider);
        register_provider(provider.as_ptr(), 1_000_000);
        test_mock::set_caller(owner);
        test_mock::set_value(5_000_000);
        store_data(owner.as_ptr(), data_hash.as_ptr(), 100, 1, 5000);
        test_mock::set_caller(provider);
        confirm_storage(provider.as_ptr(), data_hash.as_ptr());
        test_mock::SLOT.with(|s| *s.borrow_mut() = 125);
        let result = claim_storage_rewards(provider.as_ptr());
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        let reward = bytes_to_u64(&ret);
        assert!(reward > 0);
    }

    #[test]
    fn test_g27_set_licn_token() {
        // Admin can set LICN token address
        setup();
        let admin = [9u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let token = [0xDD; 32];
        assert_eq!(set_licn_token(admin.as_ptr(), token.as_ptr()), 0);
        let stored = test_mock::get_storage(LICN_TOKEN_KEY).unwrap();
        assert_eq!(stored.as_slice(), &token);
        // Non-admin fails
        let other = [5u8; 32];
        test_mock::set_caller(other);
        assert_eq!(set_licn_token(other.as_ptr(), token.as_ptr()), 1);
    }

    #[test]
    fn test_g27_set_licn_token_cannot_reconfigure() {
        setup();
        let admin = [9u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let token = [0xDD; 32];
        let new_token = [0xDE; 32];
        assert_eq!(set_licn_token(admin.as_ptr(), token.as_ptr()), 0);
        assert_eq!(set_licn_token(admin.as_ptr(), new_token.as_ptr()), 2);
        assert_eq!(
            test_mock::get_storage(LICN_TOKEN_KEY).unwrap().as_slice(),
            &token
        );
    }

    #[test]
    fn test_g27_store_data_exact_payment() {
        // Exact payment should succeed
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        let owner = [1u8; 32];
        let data_hash = [0xF3; 32];
        test_mock::set_caller(owner);
        // cost = 512 * 2 * 1000 * 10 = 10_240_000
        test_mock::set_value(10_240_000);
        let result = store_data(owner.as_ptr(), data_hash.as_ptr(), 512, 2, 1000);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_store_data_rejects_data_count_overflow_before_state_write() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        storage_set(b"data_count", &u64_to_bytes(u64::MAX));

        let owner = [1u8; 32];
        let data_hash = [0xA1; 32];
        test_mock::set_caller(owner);
        test_mock::set_value(10_000);
        assert_eq!(
            store_data(owner.as_ptr(), data_hash.as_ptr(), 1, 1, 1000),
            7
        );
        assert!(test_mock::get_storage(&data_key(&data_hash)).is_none());
        assert_eq!(
            bytes_to_u64(&test_mock::get_storage(b"data_count").unwrap()),
            u64::MAX
        );
    }

    #[test]
    fn test_store_data_rejects_cost_overflow() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let data_hash = [0xA2; 32];
        test_mock::set_caller(owner);
        test_mock::set_value(u64::MAX);
        assert_eq!(
            store_data(owner.as_ptr(), data_hash.as_ptr(), u64::MAX, 10, u64::MAX),
            6
        );
        assert!(test_mock::get_storage(&data_key(&data_hash)).is_none());
    }

    #[test]
    fn test_confirm_storage_rejects_capacity_overflow_atomically() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let data_hash = [0xA3; 32];
        let provider_addr = [2u8; 32];

        test_mock::set_caller(provider_addr);
        assert_eq!(register_provider(provider_addr.as_ptr(), u64::MAX), 0);
        let pk = provider_key(&provider_addr);
        let provider_data = encode_provider(u64::MAX, u64::MAX - 5, 0, true, 100);
        storage_set(&pk, &provider_data);

        test_mock::set_caller(owner);
        test_mock::set_value(100_000);
        assert_eq!(
            store_data(owner.as_ptr(), data_hash.as_ptr(), 10, 1, 1000),
            0
        );

        test_mock::set_caller(provider_addr);
        assert_eq!(
            confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr()),
            8
        );

        let entry = test_mock::get_storage(&data_key(&data_hash)).unwrap();
        assert_eq!(decode_data_entry_provider_count(&entry), 0);
        let provider_data = test_mock::get_storage(&pk).unwrap();
        assert_eq!(bytes_to_u64(&provider_data[8..16]), u64::MAX - 5);
    }

    #[test]
    fn test_confirm_storage_stored_count_saturates() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let data_hash = [0xA4; 32];
        let provider_addr = [2u8; 32];

        test_mock::set_caller(provider_addr);
        assert_eq!(register_provider(provider_addr.as_ptr(), 1_000_000), 0);
        let pk = provider_key(&provider_addr);
        let provider_data = encode_provider(1_000_000, 0, u64::MAX, true, 100);
        storage_set(&pk, &provider_data);

        test_mock::set_caller(owner);
        test_mock::set_value(100_000);
        assert_eq!(
            store_data(owner.as_ptr(), data_hash.as_ptr(), 10, 1, 1000),
            0
        );

        test_mock::set_caller(provider_addr);
        assert_eq!(
            confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr()),
            0
        );

        let provider_data = test_mock::get_storage(&pk).unwrap();
        assert_eq!(bytes_to_u64(&provider_data[16..24]), u64::MAX);
    }

    #[test]
    fn test_issue_challenge_counter_saturates() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        test_mock::set_caller([9u8; 32]);
        initialize([9u8; 32].as_ptr());

        let owner = [1u8; 32];
        let data_hash = [0xA5; 32];
        let provider_addr = [2u8; 32];
        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_000_000);
        test_mock::set_caller(owner);
        test_mock::set_value(100_000);
        store_data(owner.as_ptr(), data_hash.as_ptr(), 10, 1, 1000);
        test_mock::set_caller(provider_addr);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());

        storage_set(MOSS_CHALLENGE_COUNT_KEY, &u64_to_bytes(u64::MAX));
        test_mock::set_caller([7u8; 32]);
        assert_eq!(
            issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42),
            0
        );
        let count = test_mock::get_storage(MOSS_CHALLENGE_COUNT_KEY).unwrap();
        assert_eq!(bytes_to_u64(&count), u64::MAX);
    }

    #[test]
    fn test_slash_amount_uses_wide_arithmetic() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        let challenger = [9u8; 32];
        configure_licn_transfers(challenger);

        let owner = [1u8; 32];
        let data_hash = [0xA6; 32];
        let provider_addr = [2u8; 32];
        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_000_000);
        storage_set(&stake_key(&provider_addr), &u64_to_bytes(u64::MAX));
        storage_set(b"slash_percent", &u64_to_bytes(100));
        test_mock::set_caller(owner);
        test_mock::set_value(100_000);
        store_data(owner.as_ptr(), data_hash.as_ptr(), 10, 1, 1000);
        test_mock::set_caller(provider_addr);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());
        test_mock::set_caller(challenger);
        issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 400);
        assert_eq!(
            slash_provider(data_hash.as_ptr(), provider_addr.as_ptr()),
            0
        );
        assert_eq!(get_provider_stake(provider_addr.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), u64::MAX);
    }

    #[test]
    fn test_slash_failed_payouts_record_unpaid_amounts() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        let admin = [9u8; 32];
        configure_licn_transfers(admin);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));

        let owner = [1u8; 32];
        let data_hash = [0xA7; 32];
        let provider_addr = [2u8; 32];
        let challenger = [7u8; 32];
        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_000_000);
        storage_set(&stake_key(&provider_addr), &u64_to_bytes(1000));
        test_mock::set_caller(owner);
        test_mock::set_value(100_000);
        store_data(owner.as_ptr(), data_hash.as_ptr(), 10, 1, 1000);
        test_mock::set_caller(provider_addr);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());
        test_mock::set_caller(challenger);
        issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 400);
        assert_eq!(
            slash_provider(data_hash.as_ptr(), provider_addr.as_ptr()),
            0
        );

        let token = [0xDD; 32];
        let challenger_unpaid = test_mock::get_storage(&unpaid_key(&token, &challenger)).unwrap();
        assert_eq!(bytes_to_u64(&challenger_unpaid), 50);
        let admin_unpaid = test_mock::get_storage(&unpaid_key(&token, &admin)).unwrap();
        assert_eq!(bytes_to_u64(&admin_unpaid), 50);
    }

    #[test]
    fn test_respond_challenge_rejects_null_response_pointer() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        test_mock::set_caller([9u8; 32]);
        initialize([9u8; 32].as_ptr());

        let owner = [1u8; 32];
        let payload = [0xAB; 16];
        let data_hash = simple_hash(&payload);
        let provider_addr = [2u8; 32];
        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_000_000);
        test_mock::set_caller(owner);
        test_mock::set_value(160_000);
        store_data(
            owner.as_ptr(),
            data_hash.as_ptr(),
            payload.len() as u64,
            1,
            1000,
        );
        test_mock::set_caller(provider_addr);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());
        issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42);

        assert_eq!(
            respond_challenge(
                provider_addr.as_ptr(),
                data_hash.as_ptr(),
                core::ptr::null()
            ),
            6
        );
    }
}
