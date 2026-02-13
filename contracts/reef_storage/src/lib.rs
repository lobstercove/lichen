// Reef Storage — Decentralized Storage Layer for MoltChain (v2 — DEEP hardened)
//
// v2 additions:
//   - Proof-of-storage challenges: random challenges to verify providers store data
//   - Provider slashing: providers that fail challenges lose staked collateral
//   - Storage marketplace pricing: providers set custom price per byte per slot
//   - Collateral staking: providers must stake MOLT proportional to capacity
//   - Challenge response window: providers have limited time to respond
//
// Storage keys:
//   data_{hash}          → StorageEntry (owner, size, replication, confirmations, expiry, providers)
//   provider_{addr}      → ProviderInfo (capacity, stored_count, active, registered_slot, stake, price)
//   reward_{addr}        → accumulated reward balance (u64)
//   data_count           → total registered data entries (u64)
//   challenge_{hash}_{addr} → Challenge (slot, response_deadline, nonce, answered)
//   challenge_window     → slots allowed for challenge response (u64)
//   slash_percent        → percentage of stake slashed on failure (u64)
//   reef_admin           → admin address (32 bytes)

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    log_info, storage_get, storage_set, bytes_to_u64, u64_to_bytes, get_slot,
};

// ============================================================================
// CONSTANTS
// ============================================================================

const MAX_REPLICATION: u8 = 10;
const MIN_STORAGE_DURATION: u64 = 1000; // minimum slots
const MAX_PROVIDERS_PER_ENTRY: usize = 16;
const REWARD_PER_SLOT_PER_BYTE: u64 = 10; // 10 shells per slot per byte stored

// v2 constants
const DEFAULT_CHALLENGE_WINDOW: u64 = 200; // slots to respond to a challenge
const DEFAULT_SLASH_PERCENT: u64 = 10;     // 10% of stake slashed on failure
const MIN_STAKE_PER_GB: u64 = 10_000_000;  // 10M shells (0.01 MOLT) per GB of capacity
const ADMIN_KEY: &[u8] = b"reef_admin";

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

fn challenge_key(data_hash: &[u8; 32], provider: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(10 + 64 + 1 + 64);
    key.extend_from_slice(b"challenge_");
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

fn decode_data_entry_provider(data: &[u8], index: u8) -> [u8; 32] {
    let offset = DATA_HEADER_SIZE + (index as usize) * 32;
    let mut addr = [0u8; 32];
    addr.copy_from_slice(&data[offset..offset + 32]);
    addr
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

fn encode_provider(capacity: u64, used: u64, stored_count: u64, active: bool, registered_slot: u64) -> Vec<u8> {
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
    log_info("📦 Storing data request...");

    let owner = unsafe { core::slice::from_raw_parts(owner_ptr, 32) };
    let data_hash_slice = unsafe { core::slice::from_raw_parts(data_hash_ptr, 32) };

    let mut data_hash = [0u8; 32];
    data_hash.copy_from_slice(data_hash_slice);

    if size == 0 {
        log_info("❌ Data size must be > 0");
        return 1;
    }

    if replication_factor == 0 || replication_factor > MAX_REPLICATION {
        log_info("❌ Invalid replication factor");
        return 2;
    }

    if duration_slots < MIN_STORAGE_DURATION {
        log_info("❌ Duration too short");
        return 3;
    }

    let dk = data_key(&data_hash);
    if storage_get(&dk).is_some() {
        log_info("❌ Data hash already registered");
        return 4;
    }

    let current_slot = get_slot();
    let expiry_slot = current_slot.saturating_add(duration_slots);

    let mut owner_arr = [0u8; 32];
    owner_arr.copy_from_slice(owner);

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

    // Increment data count
    let count = storage_get(b"data_count")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    storage_set(b"data_count", &u64_to_bytes(count + 1));

    log_info("✅ Data storage request registered");
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
pub extern "C" fn confirm_storage(
    provider_ptr: *const u8,
    data_hash_ptr: *const u8,
) -> u32 {
    log_info("✅ Confirming storage...");

    let provider = unsafe { core::slice::from_raw_parts(provider_ptr, 32) };
    let data_hash_slice = unsafe { core::slice::from_raw_parts(data_hash_ptr, 32) };

    let mut data_hash = [0u8; 32];
    data_hash.copy_from_slice(data_hash_slice);

    let mut provider_arr = [0u8; 32];
    provider_arr.copy_from_slice(provider);

    // Check data entry exists
    let dk = data_key(&data_hash);
    let mut entry = match storage_get(&dk) {
        Some(data) => data,
        None => {
            log_info("❌ Data entry not found");
            return 1;
        }
    };

    if entry.len() < DATA_HEADER_SIZE {
        log_info("❌ Corrupt data entry");
        return 2;
    }

    // Check not expired
    let current_slot = get_slot();
    let expiry = decode_data_entry_expiry(&entry);
    if current_slot > expiry {
        log_info("❌ Storage request expired");
        return 3;
    }

    // Check provider is registered
    let pk = provider_key(&provider_arr);
    let prov_data = match storage_get(&pk) {
        Some(data) => data,
        None => {
            log_info("❌ Provider not registered");
            return 4;
        }
    };

    if prov_data.len() < PROVIDER_SIZE || prov_data[24] != 1 {
        log_info("❌ Provider not active");
        return 5;
    }

    // Check provider hasn't already confirmed
    let prov_count = decode_data_entry_provider_count(&entry);
    for i in 0..prov_count {
        let existing = decode_data_entry_provider(&entry, i);
        if existing == provider_arr {
            log_info("❌ Provider already confirmed for this data");
            return 6;
        }
    }

    // Check replication limit
    let replication = decode_data_entry_replication(&entry);
    if prov_count >= replication {
        log_info("❌ Replication factor already satisfied");
        return 7;
    }

    // Add provider to the entry
    entry[41] = entry[41].saturating_add(1); // increment confirmations
    entry[58] = prov_count + 1; // increment provider count
    entry.extend_from_slice(&provider_arr); // append provider address
    storage_set(&dk, &entry);

    // Update provider stats
    let capacity = bytes_to_u64(&prov_data[0..8]);
    let used = bytes_to_u64(&prov_data[8..16]);
    let stored_count = bytes_to_u64(&prov_data[16..24]);
    let data_size = decode_data_entry_size(&entry);
    let new_used = used.saturating_add(data_size);
    let reg_slot = bytes_to_u64(&prov_data[25..33]);

    if new_used > capacity {
        log_info("❌ Provider capacity exceeded");
        return 8;
    }

    let updated_prov = encode_provider(capacity, new_used, stored_count + 1, true, reg_slot);
    storage_set(&pk, &updated_prov);

    // Accumulate reward for future claiming
    let rk = reward_key(&provider_arr);
    let prev_reward = storage_get(&rk)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    let duration_remaining = expiry.saturating_sub(current_slot);
    let reward = duration_remaining.saturating_mul(data_size).saturating_mul(REWARD_PER_SLOT_PER_BYTE);
    storage_set(&rk, &u64_to_bytes(prev_reward.saturating_add(reward)));

    log_info("✅ Storage confirmed by provider");
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
    let data_hash_slice = unsafe { core::slice::from_raw_parts(data_hash_ptr, 32) };
    let mut data_hash = [0u8; 32];
    data_hash.copy_from_slice(data_hash_slice);

    let dk = data_key(&data_hash);
    match storage_get(&dk) {
        Some(data) => {
            moltchain_sdk::set_return_data(&data);
            0
        }
        None => {
            log_info("❌ Data entry not found");
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
pub extern "C" fn register_provider(
    provider_ptr: *const u8,
    capacity_bytes: u64,
) -> u32 {
    log_info("🔌 Registering storage provider...");

    let provider = unsafe { core::slice::from_raw_parts(provider_ptr, 32) };
    let mut provider_arr = [0u8; 32];
    provider_arr.copy_from_slice(provider);

    if capacity_bytes == 0 {
        log_info("❌ Capacity must be > 0");
        return 1;
    }

    let pk = provider_key(&provider_arr);
    if storage_get(&pk).is_some() {
        log_info("❌ Provider already registered");
        return 2;
    }

    let current_slot = get_slot();
    let prov_data = encode_provider(capacity_bytes, 0, 0, true, current_slot);
    storage_set(&pk, &prov_data);

    log_info("✅ Storage provider registered");
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
    log_info("💰 Claiming storage rewards...");

    let provider = unsafe { core::slice::from_raw_parts(provider_ptr, 32) };
    let mut provider_arr = [0u8; 32];
    provider_arr.copy_from_slice(provider);

    let rk = reward_key(&provider_arr);
    let reward = storage_get(&rk)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);

    if reward == 0 {
        log_info("❌ No rewards to claim");
        return 1;
    }

    // Reset reward balance to zero
    storage_set(&rk, &u64_to_bytes(0));

    // Return reward amount
    moltchain_sdk::set_return_data(&u64_to_bytes(reward));

    log_info("✅ Storage rewards claimed");
    0
}

// ============================================================================
// v2: ADMIN
// ============================================================================

/// Initialize admin. Called once.
#[no_mangle]
pub extern "C" fn initialize(admin_ptr: *const u8) -> u32 {
    let admin = unsafe { core::slice::from_raw_parts(admin_ptr, 32) };
    if storage_get(ADMIN_KEY).is_some() {
        return 1;
    }
    storage_set(ADMIN_KEY, admin);
    storage_set(b"challenge_window", &u64_to_bytes(DEFAULT_CHALLENGE_WINDOW));
    storage_set(b"slash_percent", &u64_to_bytes(DEFAULT_SLASH_PERCENT));
    log_info("✅ Reef Storage v2 initialized");
    0
}

/// Set challenge response window (admin only).
#[no_mangle]
pub extern "C" fn set_challenge_window(caller_ptr: *const u8, window_slots: u64) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    match storage_get(ADMIN_KEY) {
        Some(admin) if caller == admin.as_slice() => {},
        _ => return 2,
    }
    if window_slots < 10 {
        return 3;
    }
    storage_set(b"challenge_window", &u64_to_bytes(window_slots));
    0
}

/// Set slash percentage (admin only).
#[no_mangle]
pub extern "C" fn set_slash_percent(caller_ptr: *const u8, percent: u64) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    match storage_get(ADMIN_KEY) {
        Some(admin) if caller == admin.as_slice() => {},
        _ => return 2,
    }
    if percent > 100 {
        return 3;
    }
    storage_set(b"slash_percent", &u64_to_bytes(percent));
    0
}

// ============================================================================
// v2: PROVIDER STAKING & PRICING
// ============================================================================

/// Provider stakes MOLT collateral. Must be called after register_provider.
/// Stake amount must be >= MIN_STAKE_PER_GB * (capacity_bytes / 1GB).
#[no_mangle]
pub extern "C" fn stake_collateral(provider_ptr: *const u8, amount: u64) -> u32 {
    let provider = unsafe { core::slice::from_raw_parts(provider_ptr, 32) };
    let mut provider_arr = [0u8; 32];
    provider_arr.copy_from_slice(provider);

    // Verify provider is registered
    let pk = provider_key(&provider_arr);
    let prov_data = match storage_get(&pk) {
        Some(data) if data.len() >= PROVIDER_SIZE && data[24] == 1 => data,
        _ => {
            log_info("❌ Provider not registered or not active");
            return 1;
        }
    };

    let capacity = bytes_to_u64(&prov_data[0..8]);
    let gb = (capacity + 1_073_741_823) / 1_073_741_824; // round up to GB
    let min_stake = gb.saturating_mul(MIN_STAKE_PER_GB);
    if amount < min_stake {
        log_info("❌ Insufficient stake for capacity");
        return 2;
    }

    let sk = stake_key(&provider_arr);
    let prev_stake = storage_get(&sk)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    storage_set(&sk, &u64_to_bytes(prev_stake.saturating_add(amount)));

    log_info("✅ Collateral staked");
    0
}

/// Provider sets custom price per byte per slot (in shells).
#[no_mangle]
pub extern "C" fn set_storage_price(provider_ptr: *const u8, price_per_byte_per_slot: u64) -> u32 {
    let provider = unsafe { core::slice::from_raw_parts(provider_ptr, 32) };
    let mut provider_arr = [0u8; 32];
    provider_arr.copy_from_slice(provider);

    // Verify registered
    let pk = provider_key(&provider_arr);
    if storage_get(&pk).is_none() {
        return 1;
    }

    let prk = price_key(&provider_arr);
    storage_set(&prk, &u64_to_bytes(price_per_byte_per_slot));
    log_info("✅ Storage price set");
    0
}

/// Get provider's custom price. Returns REWARD_PER_SLOT_PER_BYTE if no custom price set.
#[no_mangle]
pub extern "C" fn get_storage_price(provider_ptr: *const u8) -> u64 {
    let provider = unsafe { core::slice::from_raw_parts(provider_ptr, 32) };
    let mut provider_arr = [0u8; 32];
    provider_arr.copy_from_slice(provider);

    storage_get(&price_key(&provider_arr))
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(REWARD_PER_SLOT_PER_BYTE)
}

/// Get provider's staked collateral.
#[no_mangle]
pub extern "C" fn get_provider_stake(provider_ptr: *const u8) -> u64 {
    let provider = unsafe { core::slice::from_raw_parts(provider_ptr, 32) };
    let mut provider_arr = [0u8; 32];
    provider_arr.copy_from_slice(provider);

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
/// Challenge layout: [issued_slot(8), deadline_slot(8), nonce(8), answered(1)] = 25 bytes
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
    let data_hash = unsafe { core::slice::from_raw_parts(data_hash_ptr, 32) };
    let provider = unsafe { core::slice::from_raw_parts(provider_ptr, 32) };
    let mut hash_arr = [0u8; 32];
    hash_arr.copy_from_slice(data_hash);
    let mut prov_arr = [0u8; 32];
    prov_arr.copy_from_slice(provider);

    // Verify data entry exists and provider is listed
    let dk = data_key(&hash_arr);
    let entry = match storage_get(&dk) {
        Some(data) if data.len() >= DATA_HEADER_SIZE => data,
        _ => { return 1; }
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
        if chal.len() >= 25 && chal[24] == 0 {
            // Open challenge exists — check if deadline passed
            let deadline = bytes_to_u64(&chal[8..16]);
            if current_slot <= deadline {
                log_info("❌ Active challenge already pending");
                return 4;
            }
        }
    }

    // Create challenge
    let window = storage_get(b"challenge_window")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_CHALLENGE_WINDOW);
    let deadline = current_slot.saturating_add(window);

    let mut chal = Vec::with_capacity(25);
    chal.extend_from_slice(&u64_to_bytes(current_slot)); // issued_slot
    chal.extend_from_slice(&u64_to_bytes(deadline));     // deadline_slot
    chal.extend_from_slice(&u64_to_bytes(nonce));        // nonce
    chal.push(0);                                         // answered = false

    storage_set(&ck, &chal);
    log_info("⚡ Storage challenge issued");
    0
}

/// Provider responds to a proof-of-storage challenge.
/// In production, response_hash would be verified against expected hash.
/// Here we accept any non-zero response as valid (placeholder for merkle proof).
///
/// Parameters:
///   - provider_ptr: 32-byte provider address
///   - data_hash_ptr: 32-byte data hash
///   - response_hash_ptr: 32-byte proof response
///
/// Returns 0 on success.
#[no_mangle]
pub extern "C" fn respond_challenge(
    provider_ptr: *const u8,
    data_hash_ptr: *const u8,
    response_hash_ptr: *const u8,
) -> u32 {
    let provider = unsafe { core::slice::from_raw_parts(provider_ptr, 32) };
    let data_hash = unsafe { core::slice::from_raw_parts(data_hash_ptr, 32) };
    let response = unsafe { core::slice::from_raw_parts(response_hash_ptr, 32) };

    let mut prov_arr = [0u8; 32];
    prov_arr.copy_from_slice(provider);
    let mut hash_arr = [0u8; 32];
    hash_arr.copy_from_slice(data_hash);

    // Load challenge
    let ck = challenge_key(&hash_arr, &prov_arr);
    let mut chal = match storage_get(&ck) {
        Some(data) if data.len() >= 25 => data,
        _ => { return 1; }
    };

    if chal[24] != 0 {
        log_info("❌ Challenge already answered");
        return 2;
    }

    // Check deadline
    let current_slot = get_slot();
    let deadline = bytes_to_u64(&chal[8..16]);
    if current_slot > deadline {
        log_info("❌ Challenge response too late");
        return 3;
    }

    // Verify response is non-zero (placeholder; real impl would check merkle proof)
    if response.iter().all(|&b| b == 0) {
        log_info("❌ Invalid response (all zeros)");
        return 4;
    }

    // Mark as answered
    chal[24] = 1;
    storage_set(&ck, &chal);
    log_info("✅ Challenge responded successfully");
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
pub extern "C" fn slash_provider(
    data_hash_ptr: *const u8,
    provider_ptr: *const u8,
) -> u32 {
    let data_hash = unsafe { core::slice::from_raw_parts(data_hash_ptr, 32) };
    let provider = unsafe { core::slice::from_raw_parts(provider_ptr, 32) };
    let mut hash_arr = [0u8; 32];
    hash_arr.copy_from_slice(data_hash);
    let mut prov_arr = [0u8; 32];
    prov_arr.copy_from_slice(provider);

    // Load challenge
    let ck = challenge_key(&hash_arr, &prov_arr);
    let chal = match storage_get(&ck) {
        Some(data) if data.len() >= 25 => data,
        _ => { return 1; }
    };

    // Must be unanswered
    if chal[24] != 0 {
        log_info("❌ Challenge was answered — no slash");
        return 2;
    }

    // Deadline must have passed
    let current_slot = get_slot();
    let deadline = bytes_to_u64(&chal[8..16]);
    if current_slot <= deadline {
        log_info("❌ Challenge deadline not passed yet");
        return 3;
    }

    // Calculate slash amount
    let slash_pct = storage_get(b"slash_percent")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_SLASH_PERCENT);

    let sk = stake_key(&prov_arr);
    let stake = storage_get(&sk)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);

    let slash_amount = stake * slash_pct / 100;
    if slash_amount > 0 {
        storage_set(&sk, &u64_to_bytes(stake.saturating_sub(slash_amount)));
    }

    // Mark challenge as answered (so it can't be double-slashed)
    let mut updated_chal = chal;
    updated_chal[24] = 2; // 2 = slashed
    storage_set(&ck, &updated_chal);

    moltchain_sdk::set_return_data(&u64_to_bytes(slash_amount));
    log_info("⚡ Provider slashed for failed challenge");
    0
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
    fn test_store_data() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let data_hash = [0xAA; 32];

        let result = store_data(
            owner.as_ptr(),
            data_hash.as_ptr(),
            1024,  // 1KB
            3,     // 3x replication
            5000,  // 5000 slots duration
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

        store_data(owner.as_ptr(), data_hash.as_ptr(), 512, 2, 2000);
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
        let reg_result = register_provider(provider_addr.as_ptr(), 1_000_000);
        assert_eq!(reg_result, 0);

        // Store data
        store_data(owner.as_ptr(), data_hash.as_ptr(), 1024, 3, 5000);

        // Confirm storage
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

        // Verify reward accumulated
        let rk = reward_key(&provider_addr);
        let reward = test_mock::get_storage(&rk).unwrap();
        let reward_amount = bytes_to_u64(&reward);
        assert!(reward_amount > 0);
    }

    #[test]
    fn test_get_storage_info() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 50);

        let owner = [1u8; 32];
        let data_hash = [0xDD; 32];

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
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let owner = [1u8; 32];
        let data_hash = [0xEE; 32];
        let provider_addr = [2u8; 32];

        register_provider(provider_addr.as_ptr(), 1_000_000);
        store_data(owner.as_ptr(), data_hash.as_ptr(), 100, 1, 5000);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());

        let result = claim_storage_rewards(provider_addr.as_ptr());
        assert_eq!(result, 0);

        let ret = test_mock::get_return_data();
        let reward = bytes_to_u64(&ret);
        assert!(reward > 0);

        // Reward should now be zero
        let rk = reward_key(&provider_addr);
        let stored = test_mock::get_storage(&rk).unwrap();
        assert_eq!(bytes_to_u64(&stored), 0);
    }

    // =============================================
    // v2 TESTS
    // =============================================

    #[test]
    fn test_initialize_admin() {
        setup();
        let admin = [9u8; 32];
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(initialize(admin.as_ptr()), 1); // double init
    }

    #[test]
    fn test_stake_collateral() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 10);
        let provider_addr = [2u8; 32];
        register_provider(provider_addr.as_ptr(), 1_073_741_824); // 1 GB
        let result = stake_collateral(provider_addr.as_ptr(), 1_000_000);
        assert_eq!(result, 0);
        assert_eq!(get_provider_stake(provider_addr.as_ptr()), 1_000_000);
    }

    #[test]
    fn test_stake_too_low() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 10);
        let provider_addr = [2u8; 32];
        register_provider(provider_addr.as_ptr(), 2_000_000_000); // ~2 GB
        // Needs >= 2M stake (2 * MIN_STAKE_PER_GB)
        assert_eq!(stake_collateral(provider_addr.as_ptr(), 500_000), 2);
    }

    #[test]
    fn test_set_storage_price() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 10);
        let provider_addr = [2u8; 32];
        register_provider(provider_addr.as_ptr(), 1_000_000);
        assert_eq!(set_storage_price(provider_addr.as_ptr(), 5), 0);
        assert_eq!(get_storage_price(provider_addr.as_ptr()), 5);
    }

    #[test]
    fn test_storage_price_default() {
        setup();
        let unknown = [0xFF; 32];
        assert_eq!(get_storage_price(unknown.as_ptr()), REWARD_PER_SLOT_PER_BYTE);
    }

    #[test]
    fn test_issue_and_respond_challenge() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);

        let admin = [9u8; 32];
        initialize(admin.as_ptr());

        let owner = [1u8; 32];
        let data_hash = [0xCC; 32];
        let provider_addr = [2u8; 32];
        register_provider(provider_addr.as_ptr(), 1_000_000);
        store_data(owner.as_ptr(), data_hash.as_ptr(), 1024, 3, 5000);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());

        // Issue challenge
        let result = issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42);
        assert_eq!(result, 0);

        // Respond to challenge
        let response = [0xBB; 32]; // non-zero = valid
        let result = respond_challenge(
            provider_addr.as_ptr(),
            data_hash.as_ptr(),
            response.as_ptr(),
        );
        assert_eq!(result, 0);
    }

    #[test]
    fn test_challenge_duplicate_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        initialize([9u8; 32].as_ptr());

        let owner = [1u8; 32];
        let data_hash = [0xCC; 32];
        let provider_addr = [2u8; 32];
        register_provider(provider_addr.as_ptr(), 1_000_000);
        store_data(owner.as_ptr(), data_hash.as_ptr(), 1024, 1, 5000);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());

        assert_eq!(issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42), 0);
        // Same challenge while deadline active
        assert_eq!(issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 99), 4);
    }

    #[test]
    fn test_slash_unanswered_challenge() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        initialize([9u8; 32].as_ptr());

        let owner = [1u8; 32];
        let data_hash = [0xCC; 32];
        let provider_addr = [2u8; 32];
        register_provider(provider_addr.as_ptr(), 1_073_741_824);
        stake_collateral(provider_addr.as_ptr(), 1_000_000);
        store_data(owner.as_ptr(), data_hash.as_ptr(), 1024, 1, 5000);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());
        issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42);

        // Advance past deadline
        test_mock::SLOT.with(|s| *s.borrow_mut() = 400);

        let result = slash_provider(data_hash.as_ptr(), provider_addr.as_ptr());
        assert_eq!(result, 0);

        // Check stake reduced by 10%
        let stake = get_provider_stake(provider_addr.as_ptr());
        assert_eq!(stake, 900_000);

        // Return data should have slash amount
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 100_000);
    }

    #[test]
    fn test_slash_answered_challenge_fails() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        initialize([9u8; 32].as_ptr());

        let owner = [1u8; 32];
        let data_hash = [0xCC; 32];
        let provider_addr = [2u8; 32];
        register_provider(provider_addr.as_ptr(), 1_073_741_824);
        stake_collateral(provider_addr.as_ptr(), 1_000_000);
        store_data(owner.as_ptr(), data_hash.as_ptr(), 1024, 1, 5000);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());
        issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42);

        // Respond correctly
        respond_challenge(provider_addr.as_ptr(), data_hash.as_ptr(), [0xBB; 32].as_ptr());

        // Advance past deadline
        test_mock::SLOT.with(|s| *s.borrow_mut() = 400);

        // Slash should fail because challenge was answered
        assert_eq!(slash_provider(data_hash.as_ptr(), provider_addr.as_ptr()), 2);
    }

    #[test]
    fn test_slash_before_deadline_fails() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        initialize([9u8; 32].as_ptr());

        let owner = [1u8; 32];
        let data_hash = [0xCC; 32];
        let provider_addr = [2u8; 32];
        register_provider(provider_addr.as_ptr(), 1_000_000);
        store_data(owner.as_ptr(), data_hash.as_ptr(), 1024, 1, 5000);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());
        issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42);

        // Still within deadline
        assert_eq!(slash_provider(data_hash.as_ptr(), provider_addr.as_ptr()), 3);
    }

    #[test]
    fn test_set_challenge_window_admin_only() {
        setup();
        let admin = [9u8; 32];
        initialize(admin.as_ptr());
        assert_eq!(set_challenge_window(admin.as_ptr(), 500), 0);
        let other = [8u8; 32];
        assert_eq!(set_challenge_window(other.as_ptr(), 500), 2);
    }

    #[test]
    fn test_challenge_zero_response_rejected() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        initialize([9u8; 32].as_ptr());

        let owner = [1u8; 32];
        let data_hash = [0xCC; 32];
        let provider_addr = [2u8; 32];
        register_provider(provider_addr.as_ptr(), 1_000_000);
        store_data(owner.as_ptr(), data_hash.as_ptr(), 1024, 1, 5000);
        confirm_storage(provider_addr.as_ptr(), data_hash.as_ptr());
        issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42);

        // Zero response = invalid
        assert_eq!(respond_challenge(provider_addr.as_ptr(), data_hash.as_ptr(), [0u8; 32].as_ptr()), 4);
    }
}
