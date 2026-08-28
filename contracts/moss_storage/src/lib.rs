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
    bytes_to_u64, get_caller, get_contract_address, get_slot, log_info, receive_token_or_native,
    storage_get, storage_set, transfer_token_or_native, u64_to_bytes, Address,
};
use sha2::{Digest, Sha256};

// ============================================================================
// CONSTANTS
// ============================================================================

const MAX_REPLICATION: u8 = 10;
const MIN_STORAGE_DURATION: u64 = 1000; // minimum slots
const MAX_PROVIDERS_PER_ENTRY: usize = 16;
const REWARD_PER_SLOT_PER_BYTE: u64 = 10; // 10 spores per slot per byte stored
const STORAGE_PRICING_V2_SCALE: u128 = 100_000_000;
const STORAGE_PRICING_V2: u64 = 2;

// v2 constants
const DEFAULT_CHALLENGE_WINDOW: u64 = 200; // slots to respond to a challenge
const DEFAULT_SLASH_PERCENT: u64 = 10; // 10% of stake slashed on failure
const MIN_STAKE_PER_GB: u64 = 10_000_000; // 10M spores (0.01 LICN) per GB of capacity
const OBLIGATION_COLLATERAL_BPS: u64 = 10_000; // 100% of remaining paid obligations
const ADMIN_KEY: &[u8] = b"moss_admin";

/// Storage key for LICN token address (used in call_token_transfer)
const LICN_TOKEN_KEY: &[u8] = b"moss_licn_token";

const MOSS_TOTAL_BYTES_KEY: &[u8] = b"moss_total_bytes";
const MOSS_CHALLENGE_COUNT_KEY: &[u8] = b"moss_challenge_count";
const CHALLENGE_RECORD_SIZE: usize = 25;
const CHALLENGE_RECORD_V2_SIZE: usize = 26;
const CHALLENGE_RECORD_V2_VERSION: u8 = 1;
const CHALLENGE_MIN_INTERVAL_SLOTS: u64 = 50;
const CHALLENGE_STATUS_OPEN: u8 = 0;
const CHALLENGE_STATUS_RESPONDED: u8 = 1;
const CHALLENGE_STATUS_SLASHED: u8 = 2;
const STORAGE_CHUNK_BYTES: usize = 65_536;
const MAX_MERKLE_PROOF_DEPTH: usize = 63;
const MAX_REWARD_CLAIM_ENTRIES: u32 = 64;

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
    let token_data = storage_get(LICN_TOKEN_KEY).filter(|data| data.len() >= 32)?;
    let mut token = [0u8; 32];
    token.copy_from_slice(&token_data[..32]);
    Some(Address(token))
}

fn unpaid_payout_key(token: Address, recipient: &[u8; 32]) -> Vec<u8> {
    let mut key = b"unpaid_payout:".to_vec();
    key.extend_from_slice(&token.0);
    key.push(b':');
    key.extend_from_slice(recipient);
    key
}

fn record_unpaid_licn_payout(token: Address, recipient: &[u8; 32], amount: u64) -> bool {
    if amount == 0 {
        return true;
    }
    let key = unpaid_payout_key(token, recipient);
    let current = stored_u64(&key);
    match current.checked_add(amount) {
        Some(total) => {
            storage_set(&key, &u64_to_bytes(total));
            true
        }
        None => false,
    }
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

fn required_provider_stake(capacity_bytes: u64) -> Option<u64> {
    let gib = capacity_bytes
        .checked_add(1_073_741_823)?
        .checked_div(1_073_741_824)?;
    gib.checked_mul(MIN_STAKE_PER_GB)
}

fn provider_obligation_key(provider: &[u8; 32]) -> Vec<u8> {
    let mut key = b"provider_obligation:".to_vec();
    key.extend_from_slice(provider);
    key
}

fn assignment_obligation_key(provider: &[u8; 32], data_hash: &[u8; 32]) -> Vec<u8> {
    let mut key = b"assignment_obligation:".to_vec();
    key.extend_from_slice(provider);
    key.push(b':');
    key.extend_from_slice(data_hash);
    key
}

fn assignment_failed_key(provider: &[u8; 32], data_hash: &[u8; 32]) -> Vec<u8> {
    let mut key = b"assignment_failed:".to_vec();
    key.extend_from_slice(provider);
    key.push(b':');
    key.extend_from_slice(data_hash);
    key
}

fn storage_failed_earned_key(data_hash: &[u8; 32]) -> Vec<u8> {
    let mut key = b"storage_failed_earned:".to_vec();
    key.extend_from_slice(data_hash);
    key
}

fn required_provider_collateral(
    provider: &[u8; 32],
    capacity_bytes: u64,
    additional_obligation: u64,
) -> Option<u64> {
    let base = required_provider_stake(capacity_bytes)?;
    let obligation = stored_u64(&provider_obligation_key(provider))
        .checked_add(additional_obligation)?;
    let obligation_collateral = (obligation as u128)
        .checked_mul(OBLIGATION_COLLATERAL_BPS as u128)?
        .checked_add(9_999)?
        .checked_div(10_000)?;
    let obligation_collateral = u64::try_from(obligation_collateral).ok()?;
    base.checked_add(obligation_collateral)
}

fn provider_is_sufficiently_collateralized(provider: &[u8; 32], capacity_bytes: u64) -> bool {
    required_provider_collateral(provider, capacity_bytes, 0)
        .map(|required| stored_u64(&stake_key(provider)) >= required)
        .unwrap_or(false)
}

// ============================================================================
// STORAGE KEY HELPERS
// ============================================================================

fn sha256_hash(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

fn sha256_pair(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(left);
    hasher.update(right);
    hasher.finalize().into()
}

fn storage_chunk_count(data_size: u64) -> Option<u64> {
    data_size
        .checked_add(STORAGE_CHUNK_BYTES as u64 - 1)
        .map(|rounded| rounded / STORAGE_CHUNK_BYTES as u64)
        .filter(|count| *count > 0)
}

fn merkle_proof_depth(chunk_count: u64) -> Option<usize> {
    if chunk_count == 0 {
        return None;
    }
    let mut width = chunk_count;
    let mut depth = 0usize;
    while width > 1 {
        width = width.checked_add(1)? / 2;
        depth = depth.checked_add(1)?;
        if depth > MAX_MERKLE_PROOF_DEPTH {
            return None;
        }
    }
    Some(depth)
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

fn reward_index_count_key(addr: &[u8; 32]) -> Vec<u8> {
    let mut key = b"reward_idx_count:".to_vec();
    key.extend_from_slice(addr);
    key
}

fn reward_index_entry_key(addr: &[u8; 32], index: u64) -> Vec<u8> {
    let mut key = b"reward_idx_entry:".to_vec();
    key.extend_from_slice(addr);
    key.push(b':');
    key.extend_from_slice(&u64_to_bytes(index));
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

fn storage_closed_key(data_hash: &[u8; 32]) -> Vec<u8> {
    let mut key = b"storage_closed:".to_vec();
    key.extend_from_slice(data_hash);
    key
}

fn storage_max_price_key(data_hash: &[u8; 32]) -> Vec<u8> {
    let mut key = b"storage_max_price:".to_vec();
    key.extend_from_slice(data_hash);
    key
}

fn storage_provider_price_key(data_hash: &[u8; 32], provider: &[u8; 32]) -> Vec<u8> {
    let mut key = b"storage_provider_price:".to_vec();
    key.extend_from_slice(data_hash);
    key.push(b':');
    key.extend_from_slice(provider);
    key
}

fn storage_pricing_version_key(data_hash: &[u8; 32]) -> Vec<u8> {
    let mut key = b"storage_pricing_version:".to_vec();
    key.extend_from_slice(data_hash);
    key
}

fn storage_prepaid_key(data_hash: &[u8; 32]) -> Vec<u8> {
    let mut key = b"storage_prepaid:".to_vec();
    key.extend_from_slice(data_hash);
    key
}

fn reward_start_key(provider: &[u8; 32], data_hash: &[u8; 32]) -> Vec<u8> {
    let mut key = b"reward_start:".to_vec();
    key.extend_from_slice(provider);
    key.push(b':');
    key.extend_from_slice(data_hash);
    key
}

fn reward_remainder_key(provider: &[u8; 32], data_hash: &[u8; 32]) -> Vec<u8> {
    let mut key = b"reward_remainder:".to_vec();
    key.extend_from_slice(provider);
    key.push(b':');
    key.extend_from_slice(data_hash);
    key
}

fn uses_pricing_v2(data_hash: &[u8; 32]) -> bool {
    stored_u64(&storage_pricing_version_key(data_hash)) == STORAGE_PRICING_V2
}

fn storage_pricing_v2_charge(
    size: u64,
    replicas: u64,
    duration: u64,
    price: u64,
) -> Option<u64> {
    let numerator = (size as u128)
        .checked_mul(duration as u128)?
        .checked_mul(price as u128)?;
    let per_replica = numerator
        .checked_add(STORAGE_PRICING_V2_SCALE - 1)?
        .checked_div(STORAGE_PRICING_V2_SCALE)?;
    let total = per_replica.checked_mul(replicas as u128)?;
    (total <= u64::MAX as u128).then_some(total as u64)
}

fn storage_pricing_v2_obligation(
    size: u64,
    duration: u64,
    price: u64,
) -> Option<u64> {
    let total = (size as u128)
        .checked_mul(duration as u128)?
        .checked_mul(price as u128)?
        .checked_div(STORAGE_PRICING_V2_SCALE)?;
    (total <= u64::MAX as u128).then_some(total as u64)
}

fn storage_max_price(data_hash: &[u8; 32]) -> u64 {
    storage_get(&storage_max_price_key(data_hash))
        .filter(|data| data.len() >= 8)
        .map(|data| bytes_to_u64(&data))
        .unwrap_or(REWARD_PER_SLOT_PER_BYTE)
}

fn provider_price(provider: &[u8; 32]) -> u64 {
    storage_get(&price_key(provider))
        .filter(|data| data.len() >= 8)
        .map(|data| bytes_to_u64(&data))
        .unwrap_or(REWARD_PER_SLOT_PER_BYTE)
}

fn storage_provider_price(data_hash: &[u8; 32], provider: &[u8; 32]) -> u64 {
    storage_get(&storage_provider_price_key(data_hash, provider))
        .filter(|data| data.len() >= 8)
        .map(|data| bytes_to_u64(&data))
        .unwrap_or(REWARD_PER_SLOT_PER_BYTE)
}

fn challenge_effective_nonce(
    data_hash: &[u8; 32],
    provider: &[u8; 32],
    challenge: &[u8],
) -> Result<u64, u32> {
    if challenge.len() < CHALLENGE_RECORD_SIZE {
        return Err(1);
    }
    let submitted_nonce = bytes_to_u64(&challenge[16..24]);
    if challenge.len() < CHALLENGE_RECORD_V2_SIZE
        || challenge[25] != CHALLENGE_RECORD_V2_VERSION
    {
        return Ok(submitted_nonce);
    }
    let entropy_slot = bytes_to_u64(&challenge[0..8])
        .checked_add(1)
        .ok_or(7u32)?;
    let entropy = lichen_sdk::get_block_entropy(entropy_slot).ok_or(7u32)?;
    let challenger =
        storage_get(&challenge_challenger_key(data_hash, provider)).ok_or(1u32)?;
    if challenger.len() != 32 {
        return Err(1);
    }
    let mut input = Vec::with_capacity(32 + 32 + 32 + 8 + 8 + 32);
    input.extend_from_slice(data_hash);
    input.extend_from_slice(provider);
    input.extend_from_slice(&challenger);
    input.extend_from_slice(&u64_to_bytes(submitted_nonce));
    input.extend_from_slice(&u64_to_bytes(entropy_slot));
    input.extend_from_slice(&entropy);
    Ok(bytes_to_u64(&sha256_hash(&input)[..8]))
}

fn provider_deactivated_slot_key(provider: &[u8; 32]) -> Vec<u8> {
    let mut key = b"provider_deactivated_slot:".to_vec();
    key.extend_from_slice(provider);
    key
}

fn provider_collateral_unlock_slot_key(provider: &[u8; 32]) -> Vec<u8> {
    let mut key = b"provider_collateral_unlock_slot:".to_vec();
    key.extend_from_slice(provider);
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

fn data_entry_without_provider(data: &[u8], provider: &[u8; 32]) -> Option<Vec<u8>> {
    if !data_entry_provider_bytes_valid(data) {
        return None;
    }
    let mut providers = Vec::new();
    let mut found = false;
    for index in 0..decode_data_entry_provider_count(data) {
        let current = decode_data_entry_provider(data, index);
        if current == *provider {
            found = true;
        } else {
            providers.push(current);
        }
    }
    found.then(|| {
        encode_data_entry(
            &decode_data_entry_owner(data),
            decode_data_entry_size(data),
            decode_data_entry_replication(data),
            decode_data_entry_confirmations(data).saturating_sub(1),
            decode_data_entry_expiry(data),
            decode_data_entry_created(data),
            &providers,
        )
    })
}

fn legacy_reward_index_count(addr: &[u8; 32]) -> Option<u64> {
    let data = storage_get(&reward_index_key(addr)).unwrap_or_default();
    if !data.len().is_multiple_of(32) {
        return None;
    }
    u64::try_from(data.len() / 32).ok()
}

fn provider_reward_entry_count(provider: &[u8; 32]) -> Option<u64> {
    let legacy_count = legacy_reward_index_count(provider)?;
    match storage_get(&reward_index_count_key(provider)) {
        Some(data) if data.len() >= 8 => {
            let count = bytes_to_u64(&data);
            (count >= legacy_count).then_some(count)
        }
        Some(_) => None,
        None => Some(legacy_count),
    }
}

fn provider_reward_entry(provider: &[u8; 32], index: u64) -> Option<[u8; 32]> {
    let legacy_data = storage_get(&reward_index_key(provider)).unwrap_or_default();
    if !legacy_data.len().is_multiple_of(32) {
        return None;
    }
    let legacy_count = u64::try_from(legacy_data.len() / 32).ok()?;
    let data = if index < legacy_count {
        let start = usize::try_from(index).ok()?.checked_mul(32)?;
        legacy_data.get(start..start.checked_add(32)?)?.to_vec()
    } else {
        storage_get(&reward_index_entry_key(provider, index))?
    };
    if data.len() != 32 {
        return None;
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&data);
    Some(hash)
}

fn compute_vested_reward(
    last_reward_slot: u64,
    reward_until_slot: u64,
    data_size: u64,
    reward_per_slot_per_byte: u64,
) -> Option<(u64, u64)> {
    let reward = (reward_until_slot.saturating_sub(last_reward_slot) as u128)
        .checked_mul(data_size as u128)?
        .checked_mul(reward_per_slot_per_byte as u128)?;
    (reward <= u64::MAX as u128).then_some((reward as u64, 0))
}

fn compute_vested_reward_v2(
    last_reward_slot: u64,
    reward_until_slot: u64,
    data_size: u64,
    reward_per_slot_per_byte: u64,
    previous_remainder: u64,
) -> Option<(u64, u64)> {
    if previous_remainder as u128 >= STORAGE_PRICING_V2_SCALE {
        return None;
    }
    let numerator = (reward_until_slot.saturating_sub(last_reward_slot) as u128)
        .checked_mul(data_size as u128)?
        .checked_mul(reward_per_slot_per_byte as u128)?
        .checked_add(previous_remainder as u128)?;
    let reward = numerator.checked_div(STORAGE_PRICING_V2_SCALE)?;
    let remainder = numerator.checked_rem(STORAGE_PRICING_V2_SCALE)?;
    if reward > u64::MAX as u128 || remainder > u64::MAX as u128 {
        return None;
    }
    Some((reward as u64, remainder as u64))
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
    store_data_at_price(
        owner_ptr,
        data_hash_ptr,
        size,
        replication_factor,
        duration_slots,
        REWARD_PER_SLOT_PER_BYTE,
        false,
    )
}

/// Register storage with an explicit maximum provider price. Providers whose
/// current price exceeds this immutable ceiling cannot confirm. Any spread
/// between the ceiling and confirmed provider prices is refunded at expiry.
#[no_mangle]
pub extern "C" fn store_data_v2(
    owner_ptr: *const u8,
    data_hash_ptr: *const u8,
    size: u64,
    replication_factor: u8,
    duration_slots: u64,
    max_price_per_byte_per_slot: u64,
) -> u32 {
    store_data_at_price(
        owner_ptr,
        data_hash_ptr,
        size,
        replication_factor,
        duration_slots,
        max_price_per_byte_per_slot,
        true,
    )
}

#[no_mangle]
pub extern "C" fn quote_storage_v2(
    size: u64,
    replication_factor: u8,
    duration_slots: u64,
    max_price_per_byte_per_slot: u64,
) -> u64 {
    if size == 0
        || replication_factor == 0
        || replication_factor > MAX_REPLICATION
        || duration_slots < MIN_STORAGE_DURATION
        || max_price_per_byte_per_slot == 0
    {
        return 0;
    }
    storage_pricing_v2_charge(
        size,
        u64::from(replication_factor),
        duration_slots,
        max_price_per_byte_per_slot,
    )
    .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn get_storage_pricing_v2_config() -> u32 {
    let mut result = Vec::with_capacity(16);
    result.extend_from_slice(&u64_to_bytes(STORAGE_PRICING_V2_SCALE as u64));
    result.extend_from_slice(&u64_to_bytes(REWARD_PER_SLOT_PER_BYTE));
    lichen_sdk::set_return_data(&result);
    0
}

fn store_data_at_price(
    owner_ptr: *const u8,
    data_hash_ptr: *const u8,
    size: u64,
    replication_factor: u8,
    duration_slots: u64,
    max_price_per_byte_per_slot: u64,
    pricing_v2: bool,
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
    if max_price_per_byte_per_slot == 0 {
        log_info("Maximum storage price must be nonzero");
        reentrancy_exit();
        return 8;
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
    let cost = if pricing_v2 {
        storage_pricing_v2_charge(
            size,
            u64::from(replication_factor),
            duration_slots,
            max_price_per_byte_per_slot,
        )
    } else {
        (size as u128)
            .checked_mul(replication_factor as u128)
            .and_then(|value| value.checked_mul(duration_slots as u128))
            .and_then(|value| value.checked_mul(max_price_per_byte_per_slot as u128))
            .filter(|value| *value <= u64::MAX as u128)
            .map(|value| value as u64)
    };
    let cost = match cost {
        Some(cost) => cost,
        None => {
            log_info("Storage cost overflow");
            reentrancy_exit();
            return 6;
        }
    };
    let payment_token = load_licn_token().unwrap_or(Address([0u8; 32]));
    if !receive_token_or_native(
        payment_token,
        Address(owner_arr),
        get_contract_address(),
        cost,
    )
    .unwrap_or(false)
    {
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
    storage_set(
        &storage_max_price_key(&data_hash),
        &u64_to_bytes(max_price_per_byte_per_slot),
    );
    if pricing_v2 {
        storage_set(
            &storage_pricing_version_key(&data_hash),
            &u64_to_bytes(STORAGE_PRICING_V2),
        );
        storage_set(&storage_prepaid_key(&data_hash), &u64_to_bytes(cost));
    }

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

    let capacity = bytes_to_u64(&prov_data[0..8]);
    let confirmed_price = provider_price(&provider_arr);
    if confirmed_price == 0 || confirmed_price > storage_max_price(&data_hash) {
        log_info("Provider price exceeds the storage request ceiling");
        reentrancy_exit();
        return 11;
    }

    // Check provider hasn't already confirmed
    let prov_count = decode_data_entry_provider_count(&entry);
    if data_entry_has_provider(&entry, &provider_arr) {
        log_info("Provider already confirmed for this data");
        reentrancy_exit();
        return 6;
    }
    if storage_get(&assignment_failed_key(&provider_arr, &data_hash)).is_some() {
        log_info("Provider previously failed this storage assignment");
        reentrancy_exit();
        return 12;
    }

    // Check replication limit
    let replication = decode_data_entry_replication(&entry);
    if prov_count >= replication || prov_count as usize >= MAX_PROVIDERS_PER_ENTRY {
        log_info("Replication factor already satisfied");
        reentrancy_exit();
        return 7;
    }

    let used = bytes_to_u64(&prov_data[8..16]);
    let stored_count = bytes_to_u64(&prov_data[16..24]);
    let data_size = decode_data_entry_size(&entry);
    let assignment_obligation = if uses_pricing_v2(&data_hash) {
        match storage_pricing_v2_obligation(
            data_size,
            expiry.saturating_sub(current_slot),
            confirmed_price,
        ) {
            Some(obligation) => obligation,
            None => {
                reentrancy_exit();
                return 9;
            }
        }
    } else {
        0
    };
    if required_provider_collateral(&provider_arr, capacity, assignment_obligation)
        .map(|required| stored_u64(&stake_key(&provider_arr)) < required)
        .unwrap_or(true)
    {
        log_info("Provider collateral is below capacity plus obligation coverage");
        reentrancy_exit();
        return 9;
    }
    let next_provider_obligation = match stored_u64(&provider_obligation_key(&provider_arr))
        .checked_add(assignment_obligation)
    {
        Some(obligation) => obligation,
        None => {
            reentrancy_exit();
            return 9;
        }
    };
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
    let reward_index_count = match provider_reward_entry_count(&provider_arr) {
        Some(count) => count,
        None => {
            log_info("Corrupt provider reward index");
            reentrancy_exit();
            return 10;
        }
    };
    let next_reward_index_count = match reward_index_count.checked_add(1) {
        Some(count) => count,
        None => {
            log_info("Provider reward index overflow");
            reentrancy_exit();
            return 10;
        }
    };

    storage_set(&dk, &updated_entry);
    storage_set(&pk, &updated_prov);
    storage_set(&reward_pos_key, &u64_to_bytes(current_slot));
    storage_set(
        &reward_start_key(&provider_arr, &data_hash),
        &u64_to_bytes(current_slot),
    );
    storage_set(
        &reward_remainder_key(&provider_arr, &data_hash),
        &u64_to_bytes(0),
    );
    storage_set(
        &reward_index_entry_key(&provider_arr, reward_index_count),
        &data_hash,
    );
    storage_set(
        &reward_index_count_key(&provider_arr),
        &u64_to_bytes(next_reward_index_count),
    );
    storage_set(
        &storage_provider_price_key(&data_hash, &provider_arr),
        &u64_to_bytes(confirmed_price),
    );
    storage_set(
        &assignment_obligation_key(&provider_arr, &data_hash),
        &u64_to_bytes(assignment_obligation),
    );
    storage_set(
        &provider_obligation_key(&provider_arr),
        &u64_to_bytes(next_provider_obligation),
    );

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
    if required_provider_stake(capacity_bytes).is_none() {
        log_info("Capacity cannot be collateralized within protocol limits");
        reentrancy_exit();
        return 3;
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
    let provider = match read_address32(provider_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let count = match provider_reward_entry_count(&provider) {
        Some(count) if count <= u64::from(MAX_REWARD_CLAIM_ENTRIES) => count,
        Some(_) => return 3,
        None => return 4,
    };
    claim_storage_rewards_page(provider_ptr, 0, core::cmp::max(count as u32, 1))
}

/// Claim a bounded reward-index page. Return data is
/// [reward(8), next_cursor(8), total_entries(8), complete(1)]. A zero-reward
/// page succeeds so agents can always advance deterministically.
#[no_mangle]
pub extern "C" fn claim_storage_rewards_page(
    provider_ptr: *const u8,
    cursor: u64,
    max_entries: u32,
) -> u32 {
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

    if max_entries == 0 || max_entries > MAX_REWARD_CLAIM_ENTRIES {
        reentrancy_exit();
        return 3;
    }
    let total_entries = match provider_reward_entry_count(&provider_arr) {
        Some(count) if cursor <= count => count,
        _ => {
            reentrancy_exit();
            return 4;
        }
    };
    let end = cursor
        .saturating_add(u64::from(max_entries))
        .min(total_entries);
    let current_slot = get_slot();
    let rk = reward_key(&provider_arr);
    let legacy_reward = if cursor == 0 { stored_u64(&rk) } else { 0 };
    let mut reward = legacy_reward;
    let mut reward_updates = Vec::new();
    let mut obligation_updates = Vec::new();
    let mut provider_obligation_after = stored_u64(&provider_obligation_key(&provider_arr));

    for index in cursor..end {
        let data_hash = match provider_reward_entry(&provider_arr, index) {
            Some(hash) => hash,
            None => {
                reentrancy_exit();
                return 4;
            }
        };

        let entry = match storage_get(&data_key(&data_hash)) {
            Some(data) if data_entry_provider_bytes_valid(&data) => data,
            _ => {
                reentrancy_exit();
                return 4;
            }
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
        let mut reward_until_slot = decode_data_entry_expiry(&entry).min(current_slot);
        if let Some(challenge) = storage_get(&challenge_key(&data_hash, &provider_arr)) {
            if challenge.len() >= CHALLENGE_RECORD_SIZE
                && challenge[24] == CHALLENGE_STATUS_OPEN
            {
                // An unresolved proof caps vesting at challenge issuance. A
                // successful response reopens normal vesting; a failed proof
                // cannot earn through its response window.
                reward_until_slot = reward_until_slot.min(bytes_to_u64(&challenge[0..8]));
            }
        }
        if reward_until_slot <= last_reward_slot {
            continue;
        }

        let remainder_key = reward_remainder_key(&provider_arr, &data_hash);
        let pricing_v2 = uses_pricing_v2(&data_hash);
        let vested = if pricing_v2 {
            compute_vested_reward_v2(
                last_reward_slot,
                reward_until_slot,
                decode_data_entry_size(&entry),
                storage_provider_price(&data_hash, &provider_arr),
                stored_u64(&remainder_key),
            )
        } else {
            compute_vested_reward(
                last_reward_slot,
                reward_until_slot,
                decode_data_entry_size(&entry),
                storage_provider_price(&data_hash, &provider_arr),
            )
        };
        let (vested, next_remainder) = match vested {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 4;
            }
        };
        reward = match reward.checked_add(vested) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 4;
            }
        };
        reward_updates.push((
            reward_pos_key,
            reward_until_slot,
            remainder_key,
            next_remainder,
        ));
        if pricing_v2 && storage_get(&storage_closed_key(&data_hash)).is_none() {
            let assignment_key = assignment_obligation_key(&provider_arr, &data_hash);
            let assignment_before = stored_u64(&assignment_key);
            let assignment_after = match assignment_before.checked_sub(vested) {
                Some(value) => value,
                None => {
                    reentrancy_exit();
                    return 4;
                }
            };
            provider_obligation_after = match provider_obligation_after.checked_sub(vested) {
                Some(value) => value,
                None => {
                    reentrancy_exit();
                    return 4;
                }
            };
            obligation_updates.push((assignment_key, assignment_after));
        }
    }

    // G27-02: Transfer reward tokens to provider
    if reward > 0 && !transfer_licn_out(&provider_arr, reward) {
        log_info("Reward transfer failed");
        reentrancy_exit();
        return 2;
    }

    if cursor == 0 && legacy_reward > 0 {
        storage_set(&rk, &u64_to_bytes(0));
    }
    for (reward_pos_key, reward_until_slot, remainder_key, next_remainder) in reward_updates {
        storage_set(&reward_pos_key, &u64_to_bytes(reward_until_slot));
        storage_set(&remainder_key, &u64_to_bytes(next_remainder));
    }
    for (assignment_key, assignment_after) in obligation_updates {
        storage_set(&assignment_key, &u64_to_bytes(assignment_after));
    }
    storage_set(
        &provider_obligation_key(&provider_arr),
        &u64_to_bytes(provider_obligation_after),
    );

    let mut result = Vec::with_capacity(25);
    result.extend_from_slice(&u64_to_bytes(reward));
    result.extend_from_slice(&u64_to_bytes(end));
    result.extend_from_slice(&u64_to_bytes(total_entries));
    result.push(u8::from(end == total_entries));
    lichen_sdk::set_return_data(&result);

    log_info("Storage rewards claimed");
    reentrancy_exit();
    0
}

#[no_mangle]
pub extern "C" fn get_provider_reward_entry_count(provider_ptr: *const u8) -> u64 {
    read_address32(provider_ptr)
        .and_then(|provider| provider_reward_entry_count(&provider))
        .unwrap_or(0)
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
    let current_window = stored_u64(b"challenge_window");
    if current_window != 0 && window_slots < current_window {
        // Outstanding challenges were created with the previous window. The
        // global window is therefore monotonic so deactivation can snapshot a
        // delay that covers every challenge which may still be slashable.
        reentrancy_exit();
        return 4;
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
    let min_stake = match required_provider_stake(capacity) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 2;
        }
    };
    let sk = stake_key(&provider_arr);
    let prev_stake = stored_u64(&sk);
    let next_stake = match prev_stake.checked_add(amount) {
        Some(value) if amount > 0 => value,
        _ => {
            reentrancy_exit();
            return 2;
        }
    };
    if prev_stake == 0 && next_stake < min_stake {
        log_info("Insufficient initial stake for capacity");
        reentrancy_exit();
        return 2;
    }

    // G27-02: Verify provider paid sufficient LICN collateral
    let payment_token = load_licn_token().unwrap_or(Address([0u8; 32]));
    if !receive_token_or_native(
        payment_token,
        Address(provider_arr),
        get_contract_address(),
        amount,
    )
    .unwrap_or(false)
    {
        log_info("Insufficient LICN attached for staking");
        reentrancy_exit();
        return 3;
    }

    storage_set(&sk, &u64_to_bytes(next_stake));

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
    if price_per_byte_per_slot == 0 {
        reentrancy_exit();
        return 2;
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

    provider_price(&provider_arr)
}

/// Get the immutable maximum price prepaid by a storage request.
#[no_mangle]
pub extern "C" fn get_storage_max_price(data_hash_ptr: *const u8) -> u64 {
    read_address32(data_hash_ptr)
        .map(|hash| storage_max_price(&hash))
        .unwrap_or(0)
}

/// Get a confirmed provider's immutable price snapshot for a request.
#[no_mangle]
pub extern "C" fn get_confirmed_storage_price(
    data_hash_ptr: *const u8,
    provider_ptr: *const u8,
) -> u64 {
    let data_hash = match read_address32(data_hash_ptr) {
        Some(hash) => hash,
        None => return 0,
    };
    let provider = match read_address32(provider_ptr) {
        Some(address) => address,
        None => return 0,
    };
    storage_get(&data_key(&data_hash))
        .filter(|entry| data_entry_has_provider(entry, &provider))
        .map(|_| storage_provider_price(&data_hash, &provider))
        .unwrap_or(0)
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

/// Return provider state followed by collateral and current price:
/// [capacity(8), used(8), stored_count(8), active(1), registered_slot(8),
/// collateral(8), price(8), remaining_obligations(8), required_collateral(8)].
#[no_mangle]
pub extern "C" fn get_provider_info(provider_ptr: *const u8) -> u32 {
    let provider = match read_address32(provider_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let data = match storage_get(&provider_key(&provider)) {
        Some(data) if data.len() >= PROVIDER_SIZE => data,
        _ => return 1,
    };
    let capacity = bytes_to_u64(&data[0..8]);
    let required_collateral = match required_provider_collateral(&provider, capacity, 0) {
        Some(required) => required,
        None => return 2,
    };
    let mut result = Vec::with_capacity(PROVIDER_SIZE + 32);
    result.extend_from_slice(&data[..PROVIDER_SIZE]);
    result.extend_from_slice(&u64_to_bytes(stored_u64(&stake_key(&provider))));
    result.extend_from_slice(&u64_to_bytes(provider_price(&provider)));
    result.extend_from_slice(&u64_to_bytes(stored_u64(&provider_obligation_key(
        &provider,
    ))));
    result.extend_from_slice(&u64_to_bytes(required_collateral));
    lichen_sdk::set_return_data(&result);
    0
}

/// Close an expired storage request, refund prepaid unfilled replicas, and
/// release every confirmed provider's reserved capacity. Anyone may finalize
/// expiry so providers are not locked by an unavailable owner; the supplied
/// owner must still exactly match the immutable entry and receives the refund.
/// Matured provider rewards remain claimable from the retained entry.
#[no_mangle]
pub extern "C" fn close_storage(owner_ptr: *const u8, data_hash_ptr: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let owner = match read_address32(owner_ptr) {
        Some(address) => address,
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
    let closed_key = storage_closed_key(&data_hash);
    if storage_get(&closed_key).is_some() {
        reentrancy_exit();
        return 2;
    }
    let entry = match storage_get(&data_key(&data_hash)) {
        Some(data) if data_entry_provider_bytes_valid(&data) => data,
        _ => {
            reentrancy_exit();
            return 1;
        }
    };
    if decode_data_entry_owner(&entry) != owner {
        reentrancy_exit();
        return 200;
    }
    let current_slot = get_slot();
    let expiry_slot = decode_data_entry_expiry(&entry);
    if current_slot <= expiry_slot {
        reentrancy_exit();
        return 3;
    }

    let size = decode_data_entry_size(&entry);
    let provider_count = decode_data_entry_provider_count(&entry);
    let replication = decode_data_entry_replication(&entry);
    let unfilled = match replication.checked_sub(provider_count) {
        Some(count) => u64::from(count),
        None => {
            reentrancy_exit();
            return 4;
        }
    };
    let duration = match expiry_slot.checked_sub(decode_data_entry_created(&entry)) {
        Some(slots) => slots,
        None => {
            reentrancy_exit();
            return 4;
        }
    };
    for index in 0..provider_count {
        let provider = decode_data_entry_provider(&entry, index);
        if storage_get(&challenge_key(&data_hash, &provider))
            .filter(|challenge| challenge.len() >= CHALLENGE_RECORD_SIZE)
            .is_some_and(|challenge| challenge[24] == CHALLENGE_STATUS_OPEN)
        {
            // Retain provider capacity and content until the open challenge is
            // answered or slashed. Finalization must never erase the only
            // operational incentive to serve an already-issued proof.
            reentrancy_exit();
            return 6;
        }
    }
    let max_price = storage_max_price(&data_hash);
    if max_price == 0 {
        reentrancy_exit();
        return 4;
    }
    let pricing_v2 = uses_pricing_v2(&data_hash);
    let mut refund_wide = if pricing_v2 {
        storage_get(&storage_prepaid_key(&data_hash))
            .filter(|data| data.len() >= 8)
            .map(|data| bytes_to_u64(&data) as u128)
    } else {
        (size as u128)
            .checked_mul(unfilled as u128)
            .and_then(|value| value.checked_mul(duration as u128))
            .and_then(|value| value.checked_mul(max_price as u128))
    };
    let mut refund_wide = match refund_wide.take() {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 4;
        }
    };
    if pricing_v2 {
        refund_wide = match refund_wide
            .checked_sub(stored_u64(&storage_failed_earned_key(&data_hash)) as u128)
        {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 4;
            }
        };
    }

    let mut provider_updates = Vec::with_capacity(provider_count as usize);
    let mut obligation_updates = Vec::with_capacity(provider_count as usize);
    for index in 0..provider_count {
        let provider = decode_data_entry_provider(&entry, index);
        let confirmed_price = storage_provider_price(&data_hash, &provider);
        refund_wide = if pricing_v2 {
            let reward_start = stored_u64(&reward_start_key(&provider, &data_hash));
            let reward_duration = match expiry_slot.checked_sub(reward_start) {
                Some(value) if reward_start >= decode_data_entry_created(&entry) => value,
                _ => {
                    reentrancy_exit();
                    return 4;
                }
            };
            let obligation = match storage_pricing_v2_obligation(
                size,
                reward_duration,
                confirmed_price,
            ) {
                Some(value) => value as u128,
                None => {
                    reentrancy_exit();
                    return 4;
                }
            };
            match refund_wide.checked_sub(obligation) {
                Some(value) => value,
                None => {
                    reentrancy_exit();
                    return 4;
                }
            }
        } else {
            let price_spread = match max_price.checked_sub(confirmed_price) {
                Some(spread) => spread,
                None => {
                    reentrancy_exit();
                    return 4;
                }
            };
            let spread_refund = match (size as u128)
                .checked_mul(duration as u128)
                .and_then(|value| value.checked_mul(price_spread as u128))
            {
                Some(value) => value,
                None => {
                    reentrancy_exit();
                    return 4;
                }
            };
            match refund_wide.checked_add(spread_refund) {
                Some(value) => value,
                None => {
                    reentrancy_exit();
                    return 4;
                }
            }
        };
        let key = provider_key(&provider);
        let data = match storage_get(&key) {
            Some(data) if data.len() >= PROVIDER_SIZE => data,
            _ => {
                reentrancy_exit();
                return 4;
            }
        };
        let capacity = bytes_to_u64(&data[0..8]);
        let used = match bytes_to_u64(&data[8..16]).checked_sub(size) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 4;
            }
        };
        let stored_count = match bytes_to_u64(&data[16..24]).checked_sub(1) {
            Some(value) => value,
            None => {
                reentrancy_exit();
                return 4;
            }
        };
        let active = data[24] == 1;
        let registered_slot = bytes_to_u64(&data[25..33]);
        provider_updates.push((
            key,
            encode_provider(capacity, used, stored_count, active, registered_slot),
        ));
        if pricing_v2 {
            let assignment_key = assignment_obligation_key(&provider, &data_hash);
            let assignment_remaining = stored_u64(&assignment_key);
            let provider_obligation_key = provider_obligation_key(&provider);
            let provider_obligation = match stored_u64(&provider_obligation_key)
                .checked_sub(assignment_remaining)
            {
                Some(value) => value,
                None => {
                    reentrancy_exit();
                    return 4;
                }
            };
            obligation_updates.push((
                assignment_key,
                provider_obligation_key,
                provider_obligation,
            ));
        }
    }
    let refund = match u64::try_from(refund_wide) {
        Ok(value) => value,
        Err(_) => {
            reentrancy_exit();
            return 4;
        }
    };
    let total_bytes = match stored_u64(MOSS_TOTAL_BYTES_KEY).checked_sub(size) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 4;
        }
    };

    if refund > 0 && !transfer_licn_out(&owner, refund) {
        reentrancy_exit();
        return 5;
    }
    for (key, data) in provider_updates {
        storage_set(&key, &data);
    }
    for (assignment_key, provider_key, provider_obligation) in obligation_updates {
        storage_set(&assignment_key, &u64_to_bytes(0));
        storage_set(&provider_key, &u64_to_bytes(provider_obligation));
    }
    storage_set(MOSS_TOTAL_BYTES_KEY, &u64_to_bytes(total_bytes));
    storage_set(&closed_key, &[1]);
    lichen_sdk::set_return_data(&u64_to_bytes(refund));
    reentrancy_exit();
    0
}

#[no_mangle]
pub extern "C" fn is_storage_closed(data_hash_ptr: *const u8) -> u64 {
    read_address32(data_hash_ptr)
        .map(|hash| u64::from(storage_get(&storage_closed_key(&hash)).is_some()))
        .unwrap_or(0)
}

/// Stop accepting new storage. Capacity must already be fully released.
#[no_mangle]
pub extern "C" fn deactivate_provider(provider_ptr: *const u8) -> u32 {
    let provider = match read_address32(provider_ptr) {
        Some(address) => address,
        None => return 98,
    };
    if get_caller().0 != provider {
        return 200;
    }
    let key = provider_key(&provider);
    let mut data = match storage_get(&key) {
        Some(data) if data.len() >= PROVIDER_SIZE => data,
        _ => return 1,
    };
    if bytes_to_u64(&data[8..16]) != 0 || bytes_to_u64(&data[16..24]) != 0 {
        return 2;
    }
    let deactivated_slot = get_slot();
    let challenge_window = storage_get(b"challenge_window")
        .filter(|value| value.len() >= 8)
        .map(|value| bytes_to_u64(&value))
        .unwrap_or(DEFAULT_CHALLENGE_WINDOW);
    let unlock_slot = match deactivated_slot.checked_add(challenge_window) {
        Some(slot) => slot,
        None => return 3,
    };
    data[24] = 0;
    storage_set(&key, &data);
    storage_set(
        &provider_deactivated_slot_key(&provider),
        &u64_to_bytes(deactivated_slot),
    );
    storage_set(
        &provider_collateral_unlock_slot_key(&provider),
        &u64_to_bytes(unlock_slot),
    );
    0
}

/// Withdraw collateral after deactivation and a full challenge-window delay.
#[no_mangle]
pub extern "C" fn withdraw_collateral(provider_ptr: *const u8, amount: u64) -> u32 {
    if !reentrancy_enter() {
        return 100;
    }
    let provider = match read_address32(provider_ptr) {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 98;
        }
    };
    if get_caller().0 != provider {
        reentrancy_exit();
        return 200;
    }
    let provider_data = match storage_get(&provider_key(&provider)) {
        Some(data) if data.len() >= PROVIDER_SIZE => data,
        _ => {
            reentrancy_exit();
            return 1;
        }
    };
    if provider_data[24] != 0
        || bytes_to_u64(&provider_data[8..16]) != 0
        || bytes_to_u64(&provider_data[16..24]) != 0
        || amount == 0
    {
        reentrancy_exit();
        return 2;
    }
    let deactivated_slot = stored_u64(&provider_deactivated_slot_key(&provider));
    let withdrawal_slot = stored_u64(&provider_collateral_unlock_slot_key(&provider));
    if deactivated_slot == 0 || withdrawal_slot == 0 || get_slot() <= withdrawal_slot {
        reentrancy_exit();
        return 3;
    }
    let key = stake_key(&provider);
    let current = stored_u64(&key);
    let remaining = match current.checked_sub(amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 4;
        }
    };
    storage_set(&key, &u64_to_bytes(remaining));
    if !transfer_licn_out(&provider, amount) {
        storage_set(&key, &u64_to_bytes(current));
        reentrancy_exit();
        return 5;
    }
    lichen_sdk::set_return_data(&u64_to_bytes(amount));
    reentrancy_exit();
    0
}

// ============================================================================
// v2: PROOF-OF-STORAGE CHALLENGES
// ============================================================================

/// Issue a proof-of-storage challenge to a provider for specific data.
/// Anyone can issue challenges (permissionless — keeps providers honest).
///
/// Challenge layout v2:
/// [issued_slot(8), deadline_slot(8), submitted_nonce(8), status(1), version(1)].
/// The challenged chunk is derived from committed entropy at issued_slot + 1,
/// so neither challenger nor provider can choose it when the challenge opens.
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

    // An unanswered challenge must be resolved by response or slash before it
    // can be replaced. Otherwise a failing provider could overwrite an expired
    // challenge and evade the penalty indefinitely.
    let ck = challenge_key(&hash_arr, &prov_arr);
    if let Some(chal) = storage_get(&ck) {
        if chal.len() >= CHALLENGE_RECORD_SIZE && chal[24] == CHALLENGE_STATUS_OPEN {
            log_info("Active challenge already pending");
            return 4;
        }
        if chal.len() >= CHALLENGE_RECORD_SIZE {
            let next_challenge_slot = match bytes_to_u64(&chal[0..8])
                .checked_add(CHALLENGE_MIN_INTERVAL_SLOTS)
            {
                Some(slot) => slot,
                None => return 5,
            };
            if current_slot < next_challenge_slot {
                return 5;
            }
        }
    }

    // Create challenge
    let window = storage_get(b"challenge_window")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(DEFAULT_CHALLENGE_WINDOW);
    let entropy_slot = match current_slot.checked_add(1) {
        Some(slot) => slot,
        None => return 6,
    };
    let deadline = match entropy_slot.checked_add(window) {
        Some(slot) => slot,
        None => return 6,
    };

    let mut chal = Vec::with_capacity(CHALLENGE_RECORD_V2_SIZE);
    chal.extend_from_slice(&u64_to_bytes(current_slot)); // issued_slot
    chal.extend_from_slice(&u64_to_bytes(deadline)); // deadline_slot
    chal.extend_from_slice(&u64_to_bytes(nonce)); // nonce
    chal.push(CHALLENGE_STATUS_OPEN);
    chal.push(CHALLENGE_RECORD_V2_VERSION);

    storage_set(&ck, &chal);
    storage_set(
        &challenge_challenger_key(&hash_arr, &prov_arr),
        &get_caller().0,
    );

    increment_counter_saturating(MOSS_CHALLENGE_COUNT_KEY);

    log_info("Storage challenge issued");
    0
}

/// Query a challenge and its entropy-derived effective nonce. Return data is
/// the stored 25/26-byte record followed by effective_nonce(8). Returns 7 while
/// a v2 challenge is waiting for its committed entropy slot.
#[no_mangle]
pub extern "C" fn get_challenge(
    data_hash_ptr: *const u8,
    provider_ptr: *const u8,
) -> u32 {
    let data_hash = match read_address32(data_hash_ptr) {
        Some(hash) => hash,
        None => return 98,
    };
    let provider = match read_address32(provider_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let challenge = match storage_get(&challenge_key(&data_hash, &provider)) {
        Some(data) if data.len() >= CHALLENGE_RECORD_SIZE => data,
        _ => return 1,
    };
    let effective_nonce = match challenge_effective_nonce(&data_hash, &provider, &challenge) {
        Ok(nonce) => nonce,
        Err(code) => return code,
    };
    let mut result = Vec::with_capacity(challenge.len() + 8);
    result.extend_from_slice(&challenge);
    result.extend_from_slice(&u64_to_bytes(effective_nonce));
    lichen_sdk::set_return_data(&result);
    0
}

/// Provider responds to a single-chunk proof-of-storage challenge.
/// The response pointer must reference the full committed data bytes, limited
/// to one 64 KiB chunk. Larger commitments use `respond_challenge_merkle`.
///
/// Parameters:
///   - provider_ptr: 32-byte provider address
///   - data_hash_ptr: 32-byte data hash
///   - response_ptr: full challenged data bytes; expected length is the stored data size
///
/// Returns 0 on success.
#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)] // WASM ABI validates pointer and bounded length above.
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
    if let Err(code) = challenge_effective_nonce(&hash_arr, &prov_arr, &chal) {
        return code;
    }

    let entry = match storage_get(&data_key(&hash_arr)) {
        Some(data) if data.len() >= DATA_HEADER_SIZE => data,
        _ => {
            log_info("Challenge data entry missing");
            return 1;
        }
    };

    let data_size_u64 = decode_data_entry_size(&entry);
    if data_size_u64 == 0 || data_size_u64 > STORAGE_CHUNK_BYTES as u64 {
        log_info("Invalid committed data size");
        return 4;
    }
    if response_ptr.is_null() {
        log_info("Null challenge response");
        return 6;
    }
    let data_size = data_size_u64 as usize;

    let response = unsafe { core::slice::from_raw_parts(response_ptr, data_size) };
    if sha256_hash(response) != hash_arr {
        log_info("Invalid proof-of-retrievability: commitment mismatch");
        return 4;
    }

    // Mark as answered
    chal[24] = CHALLENGE_STATUS_RESPONDED;
    storage_set(&ck, &chal);
    log_info("Challenge responded successfully");
    0
}

/// Respond to a challenge for an arbitrary-size Merkle commitment.
///
/// Data is split into 64 KiB chunks. Leaves are SHA-256(chunk), parent nodes
/// are SHA-256(left || right), and an odd final node is duplicated at each
/// level. The challenged chunk index is `challenge_nonce % chunk_count`.
#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)] // WASM ABI validates pointer/length pairs.
pub extern "C" fn respond_challenge_merkle(
    provider_ptr: *const u8,
    data_hash_ptr: *const u8,
    chunk_ptr: *const u8,
    chunk_len: u32,
    proof_ptr: *const u8,
    proof_len: u32,
) -> u32 {
    let prov_arr = match read_address32(provider_ptr) {
        Some(addr) => addr,
        None => return 98,
    };
    let hash_arr = match read_address32(data_hash_ptr) {
        Some(hash) => hash,
        None => return 98,
    };
    if get_caller().0 != prov_arr {
        return 5;
    }

    let ck = challenge_key(&hash_arr, &prov_arr);
    let mut challenge = match storage_get(&ck) {
        Some(data) if data.len() >= CHALLENGE_RECORD_SIZE => data,
        _ => return 1,
    };
    if challenge[24] != CHALLENGE_STATUS_OPEN {
        return 2;
    }
    if get_slot() > bytes_to_u64(&challenge[8..16]) {
        return 3;
    }

    let entry = match storage_get(&data_key(&hash_arr)) {
        Some(data) if data_entry_provider_bytes_valid(&data) => data,
        _ => return 1,
    };
    let data_size = decode_data_entry_size(&entry);
    let chunk_count = match storage_chunk_count(data_size) {
        Some(count) => count,
        None => return 4,
    };
    let nonce = match challenge_effective_nonce(&hash_arr, &prov_arr, &challenge) {
        Ok(nonce) => nonce,
        Err(code) => return code,
    };
    let chunk_index = nonce % chunk_count;
    let chunk_start = match chunk_index.checked_mul(STORAGE_CHUNK_BYTES as u64) {
        Some(offset) => offset,
        None => return 4,
    };
    let expected_chunk_len = core::cmp::min(
        STORAGE_CHUNK_BYTES as u64,
        match data_size.checked_sub(chunk_start) {
            Some(remaining) => remaining,
            None => return 4,
        },
    );
    if expected_chunk_len == 0 || u64::from(chunk_len) != expected_chunk_len {
        return 4;
    }
    let proof_depth = match merkle_proof_depth(chunk_count) {
        Some(depth) => depth,
        None => return 4,
    };
    let expected_proof_len = match proof_depth.checked_mul(32) {
        Some(length) => length,
        None => return 4,
    };
    if proof_len as usize != expected_proof_len
        || chunk_ptr.is_null()
        || (expected_proof_len > 0 && proof_ptr.is_null())
    {
        return 4;
    }

    let chunk = unsafe { core::slice::from_raw_parts(chunk_ptr, chunk_len as usize) };
    let proof = if expected_proof_len == 0 {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(proof_ptr, expected_proof_len) }
    };
    let mut node = sha256_hash(chunk);
    let mut node_index = chunk_index;
    for sibling_bytes in proof.as_chunks::<32>().0 {
        let sibling = *sibling_bytes;
        node = if node_index.is_multiple_of(2) {
            sha256_pair(&node, &sibling)
        } else {
            sha256_pair(&sibling, &node)
        };
        node_index /= 2;
    }
    if node != hash_arr {
        return 4;
    }

    challenge[24] = CHALLENGE_STATUS_RESPONDED;
    storage_set(&ck, &challenge);
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
    if !reentrancy_enter() {
        return 100;
    }
    let result = slash_provider_inner(data_hash_ptr, provider_ptr);
    reentrancy_exit();
    result
}

fn slash_provider_inner(data_hash_ptr: *const u8, provider_ptr: *const u8) -> u32 {
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

    let entry_key = data_key(&hash_arr);
    let entry = match storage_get(&entry_key) {
        Some(entry)
            if data_entry_provider_bytes_valid(&entry)
                && data_entry_has_provider(&entry, &prov_arr) =>
        {
            entry
        }
        _ => return 4,
    };
    let owner = decode_data_entry_owner(&entry);

    let token = match load_licn_token() {
        Some(token) => token,
        None => return 7,
    };
    let treasury = match storage_get(ADMIN_KEY) {
        Some(admin) if admin.len() == 32 => {
            let mut treasury = [0u8; 32];
            treasury.copy_from_slice(&admin);
            treasury
        }
        _ => return 7,
    };

    let mut assignment_updates = None;
    if uses_pricing_v2(&hash_arr) {
        let updated_entry = match data_entry_without_provider(&entry, &prov_arr) {
            Some(entry) => entry,
            None => return 4,
        };
        let provider_key = provider_key(&prov_arr);
        let provider_data = match storage_get(&provider_key) {
            Some(data) if data.len() >= PROVIDER_SIZE => data,
            _ => return 4,
        };
        let size = decode_data_entry_size(&entry);
        let capacity = bytes_to_u64(&provider_data[0..8]);
        let used = match bytes_to_u64(&provider_data[8..16]).checked_sub(size) {
            Some(value) => value,
            None => return 4,
        };
        let stored_count = match bytes_to_u64(&provider_data[16..24]).checked_sub(1) {
            Some(value) => value,
            None => return 4,
        };
        let updated_provider = encode_provider(
            capacity,
            used,
            stored_count,
            provider_data[24] == 1,
            bytes_to_u64(&provider_data[25..33]),
        );

        let failure_slot = bytes_to_u64(&chal[0..8]).min(decode_data_entry_expiry(&entry));
        let reward_position_key = reward_position_key(&prov_arr, &hash_arr);
        let last_reward_slot = stored_u64(&reward_position_key);
        if last_reward_slot > failure_slot {
            return 4;
        }
        let reward_remainder_key = reward_remainder_key(&prov_arr, &hash_arr);
        let (newly_vested, next_remainder) = match compute_vested_reward_v2(
            last_reward_slot,
            failure_slot,
            size,
            storage_provider_price(&hash_arr, &prov_arr),
            stored_u64(&reward_remainder_key),
        ) {
            Some(value) => value,
            None => return 4,
        };
        let reward_key = reward_key(&prov_arr);
        let matured_reward = match stored_u64(&reward_key).checked_add(newly_vested) {
            Some(value) => value,
            None => return 4,
        };
        let assignment_key = assignment_obligation_key(&prov_arr, &hash_arr);
        let assignment_before = stored_u64(&assignment_key);
        let assignment_after_vesting = match assignment_before.checked_sub(newly_vested) {
            Some(value) => value,
            None => return 4,
        };
        let provider_obligation_key = provider_obligation_key(&prov_arr);
        let provider_obligation = match stored_u64(&provider_obligation_key)
            .checked_sub(assignment_before)
        {
            Some(value) => value,
            None => return 4,
        };
        let reward_start = stored_u64(&reward_start_key(&prov_arr, &hash_arr));
        let reward_duration = match decode_data_entry_expiry(&entry).checked_sub(reward_start) {
            Some(duration) if reward_start >= decode_data_entry_created(&entry) => duration,
            _ => return 4,
        };
        let full_obligation = match storage_pricing_v2_obligation(
            size,
            reward_duration,
            storage_provider_price(&hash_arr, &prov_arr),
        ) {
            Some(value) => value,
            None => return 4,
        };
        let earned = match full_obligation.checked_sub(assignment_after_vesting) {
            Some(value) => value,
            None => return 4,
        };
        let failed_earned_key = storage_failed_earned_key(&hash_arr);
        let failed_earned = match stored_u64(&failed_earned_key).checked_add(earned) {
            Some(value) => value,
            None => return 4,
        };
        assignment_updates = Some((
            updated_entry,
            provider_key,
            updated_provider,
            reward_position_key,
            failure_slot,
            reward_remainder_key,
            next_remainder,
            reward_key,
            matured_reward,
            assignment_key,
            provider_obligation_key,
            provider_obligation,
            failed_earned_key,
            failed_earned,
        ));
    }

    // A slash may make external token calls. Commit the one-shot marker before
    // those calls so a callback cannot slash the same challenge twice. Any
    // non-zero return still rolls this transaction back atomically.
    let mut updated_chal = chal.clone();
    updated_chal[24] = CHALLENGE_STATUS_SLASHED;
    storage_set(&ck, &updated_chal);
    if let Some((
        updated_entry,
        provider_key,
        updated_provider,
        reward_position_key,
        failure_slot,
        reward_remainder_key,
        next_remainder,
        reward_key,
        matured_reward,
        assignment_key,
        provider_obligation_key,
        provider_obligation,
        failed_earned_key,
        failed_earned,
    )) = assignment_updates
    {
        storage_set(&entry_key, &updated_entry);
        storage_set(&provider_key, &updated_provider);
        storage_set(&reward_position_key, &u64_to_bytes(failure_slot));
        storage_set(&reward_remainder_key, &u64_to_bytes(next_remainder));
        storage_set(&reward_key, &u64_to_bytes(matured_reward));
        storage_set(&assignment_key, &u64_to_bytes(0));
        storage_set(
            &provider_obligation_key,
            &u64_to_bytes(provider_obligation),
        );
        storage_set(&failed_earned_key, &u64_to_bytes(failed_earned));
        storage_set(&assignment_failed_key(&prov_arr, &hash_arr), &[1]);
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

        // Reliability failures compensate the data owner first while retaining
        // a permissionless challenger incentive and protocol insurance share.
        let owner_amount = slash_amount / 2;
        let challenger_amount = slash_amount / 4;
        let mut treasury_amount = slash_amount
            .saturating_sub(owner_amount)
            .saturating_sub(challenger_amount);
        if owner_amount > 0
            && !transfer_licn_out(&owner, owner_amount)
            && !record_unpaid_licn_payout(token, &owner, owner_amount)
        {
            return 8;
        }
        if let Some(challenger_data) = storage_get(&challenge_challenger_key(&hash_arr, &prov_arr))
        {
            if challenger_data.len() >= 32 && challenger_amount > 0 {
                let mut challenger = [0u8; 32];
                challenger.copy_from_slice(&challenger_data[..32]);
                if !transfer_licn_out(&challenger, challenger_amount)
                    && !record_unpaid_licn_payout(token, &challenger, challenger_amount)
                {
                    return 8;
                }
            } else {
                treasury_amount = treasury_amount.saturating_add(challenger_amount);
            }
        } else {
            treasury_amount = treasury_amount.saturating_add(challenger_amount);
        }

        if treasury_amount > 0
            && !transfer_licn_out(&treasury, treasury_amount)
            && !record_unpaid_licn_payout(token, &treasury, treasury_amount)
        {
            return 8;
        }
    }

    lichen_sdk::set_return_data(&u64_to_bytes(slash_amount));
    log_info("Provider slashed for failed challenge");
    0
}

/// Claim a failed slash-distribution payout. The liability is cleared before
/// the external transfer and restored exactly when the transfer fails.
#[no_mangle]
pub extern "C" fn claim_unpaid_payout(caller_ptr: *const u8) -> u32 {
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
    if get_caller().0 != caller {
        reentrancy_exit();
        return 200;
    }
    let token = match load_licn_token() {
        Some(token) => token,
        None => {
            reentrancy_exit();
            return 7;
        }
    };
    let key = unpaid_payout_key(token, &caller);
    let amount = stored_u64(&key);
    if amount == 0 {
        reentrancy_exit();
        return 2;
    }
    storage_set(&key, &u64_to_bytes(0));
    if !transfer_licn_out(&caller, amount) {
        storage_set(&key, &u64_to_bytes(amount));
        reentrancy_exit();
        return 32;
    }
    lichen_sdk::set_return_data(&u64_to_bytes(amount));
    reentrancy_exit();
    0
}

/// Query the recoverable failed slash payout for a recipient.
#[no_mangle]
pub extern "C" fn get_unpaid_payout(recipient_ptr: *const u8) -> u32 {
    let recipient = match read_address32(recipient_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let token = match load_licn_token() {
        Some(token) => token,
        None => return 7,
    };
    lichen_sdk::set_return_data(&u64_to_bytes(stored_u64(&unpaid_payout_key(
        token, &recipient,
    ))));
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
    use alloc::vec;
    use lichen_sdk::test_mock;

    fn setup() {
        test_mock::reset();
    }

    /// Most storage-flow tests need a provider that has already completed the
    /// collateral step. Staking-specific tests call `super::register_provider`
    /// directly to preserve their zero-stake precondition.
    fn register_provider(provider_ptr: *const u8, capacity_bytes: u64) -> u32 {
        let result = super::register_provider(provider_ptr, capacity_bytes);
        if result == 0 {
            let provider = read_address32(provider_ptr).expect("test provider address");
            let required = required_provider_stake(capacity_bytes).expect("test capacity");
            storage_set(&stake_key(&provider), &u64_to_bytes(required));
        }
        result
    }

    fn issue_challenge(
        data_hash_ptr: *const u8,
        provider_ptr: *const u8,
        nonce: u64,
    ) -> u32 {
        let entropy_slot = get_slot().checked_add(1).expect("test challenge slot");
        test_mock::set_block_entropy(entropy_slot, [0x5C; 32]);
        super::issue_challenge(data_hash_ptr, provider_ptr, nonce)
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
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
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

        assert_eq!(get_provider_reward_entry_count(provider_addr.as_ptr()), 1);
        assert_eq!(provider_reward_entry(&provider_addr, 0), Some(data_hash));
        assert!(test_mock::get_storage(&reward_index_key(&provider_addr)).is_none());
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
    fn test_confirm_storage_requires_capacity_collateral() {
        setup();
        test_mock::set_slot(100);
        let owner = [1u8; 32];
        let provider = [2u8; 32];
        let data_hash = [0xCF; 32];

        test_mock::set_caller(provider);
        assert_eq!(super::register_provider(provider.as_ptr(), 1_000_000), 0);
        assert_eq!(get_provider_stake(provider.as_ptr()), 0);

        test_mock::set_caller(owner);
        test_mock::set_value(10_000_000);
        assert_eq!(
            store_data(owner.as_ptr(), data_hash.as_ptr(), 1_000, 1, 1_000),
            0
        );

        test_mock::set_caller(provider);
        assert_eq!(confirm_storage(provider.as_ptr(), data_hash.as_ptr()), 9);
        let entry = test_mock::get_storage(&data_key(&data_hash)).unwrap();
        assert_eq!(decode_data_entry_provider_count(&entry), 0);
        let provider_data = test_mock::get_storage(&provider_key(&provider)).unwrap();
        assert_eq!(bytes_to_u64(&provider_data[8..16]), 0);
    }

    #[test]
    fn test_close_storage_refunds_unfilled_and_releases_capacity() {
        setup();
        configure_licn_transfers([9u8; 32]);
        test_mock::set_slot(100);
        let owner = [1u8; 32];
        let provider = [2u8; 32];
        let finalizer = [3u8; 32];
        let data_hash = [0xD0; 32];
        let size = 1_000u64;
        let duration = 1_000u64;

        test_mock::set_caller(provider);
        assert_eq!(register_provider(provider.as_ptr(), 1_000_000), 0);
        test_mock::set_caller(owner);
        assert_eq!(
            store_data(owner.as_ptr(), data_hash.as_ptr(), size, 2, duration),
            0
        );
        test_mock::set_caller(provider);
        assert_eq!(confirm_storage(provider.as_ptr(), data_hash.as_ptr()), 0);

        test_mock::set_slot(1_101);
        test_mock::set_caller(finalizer);
        assert_eq!(close_storage(owner.as_ptr(), data_hash.as_ptr()), 0);
        assert_eq!(
            bytes_to_u64(&test_mock::get_return_data()),
            size * duration * REWARD_PER_SLOT_PER_BYTE
        );
        assert_eq!(is_storage_closed(data_hash.as_ptr()), 1);
        assert_eq!(stored_u64(MOSS_TOTAL_BYTES_KEY), 0);
        let provider_data = test_mock::get_storage(&provider_key(&provider)).unwrap();
        assert_eq!(bytes_to_u64(&provider_data[8..16]), 0);
        assert_eq!(bytes_to_u64(&provider_data[16..24]), 0);
        assert_eq!(close_storage(owner.as_ptr(), data_hash.as_ptr()), 2);

        // Closing retains the entry and provider reward position so the exact
        // matured reward remains claimable after capacity is released.
        test_mock::set_caller(provider);
        assert_eq!(claim_storage_rewards(provider.as_ptr()), 0);
        assert_eq!(
            bytes_to_u64(&test_mock::get_return_data()),
            size * duration * REWARD_PER_SLOT_PER_BYTE
        );
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
        assert_eq!(get_provider_info(provider_addr.as_ptr()), 0);
        let info = test_mock::get_return_data();
        assert_eq!(info.len(), PROVIDER_SIZE + 32);
        assert_eq!(bytes_to_u64(&info[PROVIDER_SIZE..PROVIDER_SIZE + 8]), 10_000_000);
        assert_eq!(bytes_to_u64(&info[PROVIDER_SIZE + 8..PROVIDER_SIZE + 16]), 10);
        assert_eq!(bytes_to_u64(&info[PROVIDER_SIZE + 16..PROVIDER_SIZE + 24]), 0);
        assert_eq!(bytes_to_u64(&info[PROVIDER_SIZE + 24..]), 10_000_000);
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

        assert_eq!(claim_storage_rewards(provider_addr.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 0);

        test_mock::SLOT.with(|s| *s.borrow_mut() = 150);

        let result = claim_storage_rewards(provider_addr.as_ptr());
        assert_eq!(result, 0);

        let ret = test_mock::get_return_data();
        let reward = bytes_to_u64(&ret);
        assert_eq!(reward, 50_000);

        // Reward should now be zero
        let rk = reward_key(&provider_addr);
        assert_eq!(stored_u64(&rk), 0);

        let reward_pos =
            test_mock::get_storage(&reward_position_key(&provider_addr, &data_hash)).unwrap();
        assert_eq!(bytes_to_u64(&reward_pos), 150);
    }

    #[test]
    fn test_paginated_rewards_are_bounded_and_legacy_compatible() {
        setup();
        enable_reward_transfers();
        test_mock::set_slot(110);
        let provider = [2u8; 32];
        let owner = [1u8; 32];
        let total = 70u64;
        let mut legacy_index = Vec::new();

        for index in 0..total {
            let mut hash = [0u8; 32];
            hash[..8].copy_from_slice(&u64_to_bytes(index));
            storage_set(
                &data_key(&hash),
                &encode_data_entry(&owner, 1, 1, 1, 2_000, 100, &[provider]),
            );
            storage_set(
                &reward_position_key(&provider, &hash),
                &u64_to_bytes(100),
            );
            if index < 2 {
                legacy_index.extend_from_slice(&hash);
            } else {
                storage_set(&reward_index_entry_key(&provider, index), &hash);
            }
        }
        storage_set(&reward_index_key(&provider), &legacy_index);
        storage_set(&reward_index_count_key(&provider), &u64_to_bytes(total));
        test_mock::set_caller(provider);

        assert_eq!(get_provider_reward_entry_count(provider.as_ptr()), total);
        assert_eq!(claim_storage_rewards(provider.as_ptr()), 3);
        assert_eq!(claim_storage_rewards_page(provider.as_ptr(), 0, 65), 3);

        assert_eq!(
            claim_storage_rewards_page(provider.as_ptr(), 0, MAX_REWARD_CLAIM_ENTRIES),
            0
        );
        let first = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&first[0..8]), 6_400);
        assert_eq!(bytes_to_u64(&first[8..16]), 64);
        assert_eq!(bytes_to_u64(&first[16..24]), total);
        assert_eq!(first[24], 0);

        assert_eq!(
            claim_storage_rewards_page(provider.as_ptr(), 64, MAX_REWARD_CLAIM_ENTRIES),
            0
        );
        let second = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&second[0..8]), 600);
        assert_eq!(bytes_to_u64(&second[8..16]), total);
        assert_eq!(bytes_to_u64(&second[16..24]), total);
        assert_eq!(second[24], 1);
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
        super::register_provider(provider_addr.as_ptr(), 1_073_741_824); // 1 GB
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
        super::register_provider(provider_addr.as_ptr(), 2_000_000_000); // ~2 GB
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
    fn test_provider_price_is_snapshotted_and_accounted_exactly() {
        setup();
        configure_licn_transfers([9u8; 32]);
        test_mock::set_slot(100);
        let owner = [1u8; 32];
        let provider = [2u8; 32];
        let finalizer = [3u8; 32];
        let data_hash = [0xA7; 32];
        let size = 100_000_000u64;
        let duration = 1_000u64;

        test_mock::set_caller(provider);
        assert_eq!(register_provider(provider.as_ptr(), size), 0);
        assert_eq!(set_storage_price(provider.as_ptr(), 0), 2);
        assert_eq!(set_storage_price(provider.as_ptr(), 9), 0);

        test_mock::set_caller(owner);
        assert_eq!(
            store_data_v2(
                owner.as_ptr(),
                data_hash.as_ptr(),
                size,
                2,
                duration,
                8,
            ),
            0
        );
        assert_eq!(get_storage_max_price(data_hash.as_ptr()), 8);

        test_mock::set_caller(provider);
        assert_eq!(confirm_storage(provider.as_ptr(), data_hash.as_ptr()), 11);
        assert_eq!(set_storage_price(provider.as_ptr(), 6), 0);
        assert_eq!(confirm_storage(provider.as_ptr(), data_hash.as_ptr()), 9);
        assert_eq!(stored_u64(&provider_obligation_key(&provider)), 0);
        let entry = storage_get(&data_key(&data_hash)).expect("storage request");
        assert_eq!(decode_data_entry_provider_count(&entry), 0);
        test_mock::set_value(6_000);
        assert_eq!(stake_collateral(provider.as_ptr(), 6_000), 0);
        assert_eq!(confirm_storage(provider.as_ptr(), data_hash.as_ptr()), 0);
        assert_eq!(get_confirmed_storage_price(data_hash.as_ptr(), provider.as_ptr()), 6);
        assert_eq!(set_storage_price(provider.as_ptr(), 4), 0);
        assert_eq!(get_confirmed_storage_price(data_hash.as_ptr(), provider.as_ptr()), 6);

        test_mock::set_slot(1_101);
        test_mock::set_caller(finalizer);
        assert_eq!(close_storage(owner.as_ptr(), data_hash.as_ptr()), 0);
        // Exact prepaid 16,000 minus the confirmed provider's 6,000 obligation.
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 10_000);

        test_mock::set_caller(provider);
        assert_eq!(claim_storage_rewards(provider.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 6_000);
    }

    #[test]
    fn test_pricing_v2_fractional_rewards_are_cumulative_not_rounding_exploitable() {
        setup();
        configure_licn_transfers([9u8; 32]);
        test_mock::set_slot(100);
        let owner = [1u8; 32];
        let provider = [2u8; 32];
        let data_hash = [0xA8; 32];
        let duration = 10_000_000u64;
        assert_eq!(quote_storage_v2(1, 1, duration, 10), 1);
        assert_eq!(quote_storage_v2(0, 1, duration, 10), 0);
        assert_eq!(get_storage_pricing_v2_config(), 0);
        let pricing = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&pricing[0..8]), 100_000_000);
        assert_eq!(bytes_to_u64(&pricing[8..16]), 10);

        test_mock::set_caller(provider);
        assert_eq!(register_provider(provider.as_ptr(), 1), 0);
        test_mock::set_caller(owner);
        assert_eq!(
            store_data_v2(
                owner.as_ptr(),
                data_hash.as_ptr(),
                1,
                1,
                duration,
                10,
            ),
            0
        );
        assert_eq!(stored_u64(&storage_prepaid_key(&data_hash)), 1);
        test_mock::set_caller(provider);
        test_mock::set_value(1);
        assert_eq!(stake_collateral(provider.as_ptr(), 1), 0);
        assert_eq!(confirm_storage(provider.as_ptr(), data_hash.as_ptr()), 0);

        test_mock::set_slot(5_000_100);
        assert_eq!(claim_storage_rewards(provider.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 0);
        assert_eq!(
            stored_u64(&reward_remainder_key(&provider, &data_hash)),
            50_000_000
        );

        test_mock::set_slot(10_000_100);
        assert_eq!(claim_storage_rewards(provider.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 1);
        assert_eq!(
            stored_u64(&reward_remainder_key(&provider, &data_hash)),
            0
        );
    }

    #[test]
    fn test_pricing_v2_failed_provider_is_terminated_compensated_and_replaceable() {
        setup();
        let admin = [9u8; 32];
        let owner = [1u8; 32];
        let failed_provider = [2u8; 32];
        let replacement = [3u8; 32];
        let challenger = [7u8; 32];
        let data_hash = [0xA9; 32];
        let size = 100_000_000u64;
        let duration = 1_000u64;
        let price = 6u64;
        configure_licn_transfers(admin);
        test_mock::set_slot(100);

        test_mock::set_caller(failed_provider);
        assert_eq!(register_provider(failed_provider.as_ptr(), size), 0);
        assert_eq!(set_storage_price(failed_provider.as_ptr(), price), 0);
        test_mock::set_value(6_000);
        assert_eq!(stake_collateral(failed_provider.as_ptr(), 6_000), 0);

        test_mock::set_caller(owner);
        assert_eq!(
            store_data_v2(owner.as_ptr(), data_hash.as_ptr(), size, 1, duration, 8),
            0
        );
        test_mock::set_caller(failed_provider);
        assert_eq!(confirm_storage(failed_provider.as_ptr(), data_hash.as_ptr()), 0);
        assert_eq!(stored_u64(&provider_obligation_key(&failed_provider)), 6_000);
        assert_eq!(get_provider_info(failed_provider.as_ptr()), 0);
        let info = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&info[PROVIDER_SIZE + 16..PROVIDER_SIZE + 24]), 6_000);
        assert_eq!(bytes_to_u64(&info[PROVIDER_SIZE + 24..]), 10_006_000);

        test_mock::set_slot(300);
        test_mock::set_caller(challenger);
        assert_eq!(issue_challenge(data_hash.as_ptr(), failed_provider.as_ptr(), 42), 0);

        // An open proof challenge freezes accrual at its issue slot. The
        // provider can claim the 200 slots already served, never the response
        // window during which availability is unproven.
        test_mock::set_slot(350);
        test_mock::set_caller(failed_provider);
        assert_eq!(claim_storage_rewards(failed_provider.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 1_200);
        assert_eq!(stored_u64(&provider_obligation_key(&failed_provider)), 4_800);

        test_mock::set_slot(502);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(slash_provider(data_hash.as_ptr(), failed_provider.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 1_000_600);
        assert_eq!(get_provider_stake(failed_provider.as_ptr()), 9_005_400);
        assert_eq!(stored_u64(&provider_obligation_key(&failed_provider)), 0);
        assert_eq!(stored_u64(&assignment_obligation_key(&failed_provider, &data_hash)), 0);
        assert_eq!(stored_u64(&storage_failed_earned_key(&data_hash)), 1_200);
        assert!(storage_get(&assignment_failed_key(&failed_provider, &data_hash)).is_some());

        let entry = storage_get(&data_key(&data_hash)).expect("storage request remains open");
        assert_eq!(decode_data_entry_confirmations(&entry), 0);
        assert_eq!(decode_data_entry_provider_count(&entry), 0);
        let provider = storage_get(&provider_key(&failed_provider)).expect("failed provider");
        assert_eq!(bytes_to_u64(&provider[8..16]), 0);
        assert_eq!(bytes_to_u64(&provider[16..24]), 0);

        let token = [0xDD; 32];
        assert_eq!(
            bytes_to_u64(
                &test_mock::get_storage(&unpaid_key(&token, &owner)).expect("owner compensation")
            ),
            500_300
        );
        assert_eq!(
            bytes_to_u64(
                &test_mock::get_storage(&unpaid_key(&token, &challenger))
                    .expect("challenger incentive")
            ),
            250_150
        );
        assert_eq!(
            bytes_to_u64(
                &test_mock::get_storage(&unpaid_key(&token, &admin)).expect("treasury insurance")
            ),
            250_150
        );

        // The failed assignment cannot be reclaimed by the same provider, but
        // its vacant replica slot can be filled by a properly collateralized
        // replacement for the exact remaining obligation.
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        test_mock::set_caller(failed_provider);
        assert_eq!(confirm_storage(failed_provider.as_ptr(), data_hash.as_ptr()), 12);

        test_mock::set_caller(replacement);
        assert_eq!(register_provider(replacement.as_ptr(), size), 0);
        assert_eq!(set_storage_price(replacement.as_ptr(), price), 0);
        let replacement_obligation =
            storage_pricing_v2_obligation(size, 1_100 - 502, price).expect("replacement obligation");
        assert_eq!(replacement_obligation, 3_588);
        test_mock::set_value(replacement_obligation);
        assert_eq!(stake_collateral(replacement.as_ptr(), replacement_obligation), 0);
        assert_eq!(confirm_storage(replacement.as_ptr(), data_hash.as_ptr()), 0);

        test_mock::set_slot(1_101);
        test_mock::set_caller([8u8; 32]);
        assert_eq!(close_storage(owner.as_ptr(), data_hash.as_ptr()), 0);
        // 8,000 prepaid - 1,200 earned before failure - 3,588 owed to the
        // replacement. No unserved or failed-provider future reward leaks.
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 3_212);
        assert_eq!(stored_u64(&provider_obligation_key(&replacement)), 0);

        test_mock::set_caller(replacement);
        assert_eq!(claim_storage_rewards(replacement.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 3_588);
        test_mock::set_caller(failed_provider);
        assert_eq!(claim_storage_rewards(failed_provider.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 0);
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
        let data_hash = sha256_hash(&payload);
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
    fn test_challenge_waits_for_future_entropy_and_cannot_be_overwritten() {
        setup();
        test_mock::set_slot(100);
        let owner = [1u8; 32];
        let provider = [2u8; 32];
        let challenger = [3u8; 32];
        let data_hash = [0xC8; 32];
        storage_set(
            &data_key(&data_hash),
            &encode_data_entry(&owner, 1, 1, 1, 1_000, 0, &[provider]),
        );
        test_mock::set_caller(challenger);

        assert_eq!(
            super::issue_challenge(data_hash.as_ptr(), provider.as_ptr(), 42),
            0
        );
        assert_eq!(get_challenge(data_hash.as_ptr(), provider.as_ptr()), 7);

        test_mock::set_block_entropy(101, [0xA5; 32]);
        assert_eq!(get_challenge(data_hash.as_ptr(), provider.as_ptr()), 0);
        let result = test_mock::get_return_data();
        assert_eq!(result.len(), CHALLENGE_RECORD_V2_SIZE + 8);
        assert_ne!(bytes_to_u64(&result[result.len() - 8..]), 42);

        // The deadline has passed, but the unresolved challenge remains the
        // slashable record and cannot be replaced by a fresh easy challenge.
        test_mock::set_slot(302);
        assert_eq!(
            super::issue_challenge(data_hash.as_ptr(), provider.as_ptr(), 99),
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
        super::register_provider(provider_addr.as_ptr(), 1_073_741_824);
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
        assert_eq!(args.len(), 76);
        assert_eq!(
            &args[..4],
            &[lichen_sdk::crosscall::ABI_LAYOUT_MARKER, 32, 32, 8]
        );
        let mut recipient = [0u8; 32];
        recipient.copy_from_slice(&args[36..68]);
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
        let data_hash = sha256_hash(&payload);
        let provider_addr = [2u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_073_741_824);
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
        assert_eq!(set_challenge_window(admin.as_ptr(), 499), 4);
        let other = [8u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(other);
        assert_eq!(set_challenge_window(other.as_ptr(), 500), 2);
    }

    #[test]
    fn test_collateral_exit_delay_and_failed_transfer_are_retry_safe() {
        setup();
        let admin = [9u8; 32];
        let provider = [2u8; 32];
        configure_licn_transfers(admin);
        test_mock::set_slot(100);

        test_mock::set_caller(provider);
        assert_eq!(
            super::register_provider(provider.as_ptr(), 1_073_741_824),
            0
        );
        assert_eq!(stake_collateral(provider.as_ptr(), 10_000_000), 0);
        assert_eq!(deactivate_provider(provider.as_ptr()), 0);
        assert_eq!(
            stored_u64(&provider_collateral_unlock_slot_key(&provider)),
            300
        );

        test_mock::set_slot(300);
        assert_eq!(withdraw_collateral(provider.as_ptr(), 4_000_000), 3);
        assert_eq!(get_provider_stake(provider.as_ptr()), 10_000_000);

        test_mock::set_slot(301);
        test_mock::set_cross_call_should_fail(true);
        assert_eq!(withdraw_collateral(provider.as_ptr(), 4_000_000), 5);
        assert_eq!(get_provider_stake(provider.as_ptr()), 10_000_000);

        test_mock::set_cross_call_should_fail(false);
        assert_eq!(withdraw_collateral(provider.as_ptr(), 4_000_000), 0);
        assert_eq!(get_provider_stake(provider.as_ptr()), 6_000_000);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 4_000_000);
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
        let data_hash = sha256_hash(&payload);
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
        super::register_provider(provider.as_ptr(), 1_073_741_824); // 1 GB
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
        let capacity = u64::MAX - 1_073_741_823;
        assert_eq!(register_provider(provider_addr.as_ptr(), capacity), 0);
        let pk = provider_key(&provider_addr);
        let provider_data = encode_provider(capacity, capacity - 5, 0, true, 100);
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
        assert_eq!(bytes_to_u64(&provider_data[8..16]), capacity - 5);
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

        let owner = [1u8; 32];
        let data_hash = [0xA7; 32];
        let provider_addr = [2u8; 32];
        let challenger = [7u8; 32];
        test_mock::set_caller(provider_addr);
        register_provider(provider_addr.as_ptr(), 1_000_000);
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
        test_mock::set_caller(challenger);
        assert_eq!(
            issue_challenge(data_hash.as_ptr(), provider_addr.as_ptr(), 42),
            0
        );

        test_mock::SLOT.with(|s| *s.borrow_mut() = 400);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(
            slash_provider(data_hash.as_ptr(), provider_addr.as_ptr()),
            0
        );

        let token = [0xDD; 32];
        let owner_unpaid = test_mock::get_storage(&unpaid_key(&token, &owner)).unwrap();
        assert_eq!(bytes_to_u64(&owner_unpaid), 500_000);
        let challenger_unpaid = test_mock::get_storage(&unpaid_key(&token, &challenger)).unwrap();
        assert_eq!(bytes_to_u64(&challenger_unpaid), 250_000);
        let admin_unpaid = test_mock::get_storage(&unpaid_key(&token, &admin)).unwrap();
        assert_eq!(bytes_to_u64(&admin_unpaid), 250_000);
    }

    #[test]
    fn test_unpaid_slash_payout_is_queryable_and_retry_safe() {
        setup();
        let recipient = [7u8; 32];
        let admin = [9u8; 32];
        configure_licn_transfers(admin);
        let token = Address([0xDD; 32]);
        let key = unpaid_payout_key(token, &recipient);
        storage_set(&key, &u64_to_bytes(50));

        assert_eq!(get_unpaid_payout(recipient.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 50);

        test_mock::set_caller(recipient);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(claim_unpaid_payout(recipient.as_ptr()), 32);
        assert_eq!(stored_u64(&key), 50);

        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        assert_eq!(claim_unpaid_payout(recipient.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 50);
        assert_eq!(stored_u64(&key), 0);
        assert_eq!(claim_unpaid_payout(recipient.as_ptr()), 2);
    }

    #[test]
    fn test_unpaid_slash_payout_rejects_caller_spoof() {
        setup();
        let recipient = [7u8; 32];
        configure_licn_transfers([9u8; 32]);
        let key = unpaid_payout_key(Address([0xDD; 32]), &recipient);
        storage_set(&key, &u64_to_bytes(50));
        test_mock::set_caller([8u8; 32]);
        assert_eq!(claim_unpaid_payout(recipient.as_ptr()), 200);
        assert_eq!(stored_u64(&key), 50);
    }

    fn merkle_root_and_proof(chunks: &[Vec<u8>], target_index: usize) -> ([u8; 32], Vec<u8>) {
        let mut nodes = chunks
            .iter()
            .map(|chunk| sha256_hash(chunk))
            .collect::<Vec<_>>();
        let mut index = target_index;
        let mut proof = Vec::new();
        while nodes.len() > 1 {
            let sibling_index = if index.is_multiple_of(2) {
                core::cmp::min(index + 1, nodes.len() - 1)
            } else {
                index - 1
            };
            proof.extend_from_slice(&nodes[sibling_index]);

            let mut next = Vec::with_capacity(nodes.len().div_ceil(2));
            for pair in nodes.chunks(2) {
                let right = if pair.len() == 2 { &pair[1] } else { &pair[0] };
                next.push(sha256_pair(&pair[0], right));
            }
            nodes = next;
            index /= 2;
        }
        (nodes[0], proof)
    }

    #[test]
    fn test_merkle_challenge_verifies_large_sha256_commitment() {
        setup();
        test_mock::set_slot(100);
        let owner = [1u8; 32];
        let provider = [2u8; 32];
        let challenger = [3u8; 32];
        let chunks = vec![
            vec![0x11; STORAGE_CHUNK_BYTES],
            vec![0x22; STORAGE_CHUNK_BYTES],
            vec![0x33; 123],
        ];
        let (root, _) = merkle_root_and_proof(&chunks, 0);
        let size = (STORAGE_CHUNK_BYTES * 2 + 123) as u64;
        let cost = size * 1_000 * REWARD_PER_SLOT_PER_BYTE;

        test_mock::set_caller(provider);
        assert_eq!(register_provider(provider.as_ptr(), size), 0);
        test_mock::set_caller(owner);
        test_mock::set_value(cost);
        assert_eq!(store_data(owner.as_ptr(), root.as_ptr(), size, 1, 1_000), 0);
        test_mock::set_caller(provider);
        assert_eq!(confirm_storage(provider.as_ptr(), root.as_ptr()), 0);
        test_mock::set_caller(challenger);
        assert_eq!(issue_challenge(root.as_ptr(), provider.as_ptr(), 2), 0);
        assert_eq!(get_challenge(root.as_ptr(), provider.as_ptr()), 0);
        let challenge = test_mock::get_return_data();
        let effective_nonce = bytes_to_u64(&challenge[challenge.len() - 8..]);
        let target_index = (effective_nonce % chunks.len() as u64) as usize;
        let (_, proof) = merkle_root_and_proof(&chunks, target_index);

        let mut bad_proof = proof.clone();
        bad_proof[0] ^= 1;
        test_mock::set_caller(provider);
        assert_eq!(
            respond_challenge_merkle(
                provider.as_ptr(),
                root.as_ptr(),
                chunks[target_index].as_ptr(),
                chunks[target_index].len() as u32,
                bad_proof.as_ptr(),
                bad_proof.len() as u32,
            ),
            4
        );
        assert_eq!(
            respond_challenge_merkle(
                provider.as_ptr(),
                root.as_ptr(),
                chunks[target_index].as_ptr(),
                chunks[target_index].len() as u32,
                proof.as_ptr(),
                proof.len() as u32,
            ),
            0
        );
    }

    #[test]
    fn test_respond_challenge_rejects_null_response_pointer() {
        setup();
        test_mock::SLOT.with(|s| *s.borrow_mut() = 100);
        test_mock::set_caller([9u8; 32]);
        initialize([9u8; 32].as_ptr());

        let owner = [1u8; 32];
        let payload = [0xAB; 16];
        let data_hash = sha256_hash(&payload);
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
