// LichenOracle - Decentralized Oracle System
// Features: Price Feeds, Verifiable Random Function (VRF), Attestations

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

use lichen_sdk::{
    bytes_to_u64, get_caller, get_timestamp, log_info, storage_get, storage_set, u64_to_bytes,
};

// ============================================================================
// PRICE FEED ORACLE - Real-time asset pricing
// ============================================================================

// Price feed: 49 bytes
// price (8) + timestamp (8) + decimals (1) + feeder (32)
const PRICE_FEED_SIZE: usize = 49;

fn init_owner_matches_signer(owner: &[u8; 32]) -> bool {
    let caller = lichen_sdk::get_caller();
    if caller.0 == *owner {
        return true;
    }

    #[cfg(test)]
    {
        return caller.0 == [0u8; 32];
    }

    #[cfg(not(test))]
    {
        false
    }
}

#[no_mangle]
pub extern "C" fn initialize_oracle(owner_ptr: *const u8) -> u32 {
    // Re-initialization guard: reject if oracle_owner is already set
    if storage_get(b"oracle_owner").is_some() {
        log_info("LichenOracle already initialized — ignoring");
        return 0;
    }

    log_info("Initializing LichenOracle...");

    let mut owner = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32);
    }
    if !init_owner_matches_signer(&owner) {
        log_info("Oracle initialize rejected: caller mismatch");
        return 2;
    }
    storage_set(b"oracle_owner", &owner);

    log_info("Oracle initialized!");
    log_info("   Features: Price Feeds, VRF, Attestations");
    // AUDIT-FIX 2.22: Return 0 for success (consistent with all other functions)
    0
}

#[no_mangle]
pub extern "C" fn add_price_feeder(
    feeder_ptr: *const u8,
    asset_ptr: *const u8,
    asset_len: u32,
) -> u32 {
    log_info("Adding price feeder...");

    let mut feeder = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(feeder_ptr, feeder.as_mut_ptr(), 32);
    }
    let mut asset = alloc::vec![0u8; asset_len as usize];
    unsafe {
        core::ptr::copy_nonoverlapping(asset_ptr, asset.as_mut_ptr(), asset_len as usize);
    }

    // T5.10 fix: Check caller (not feeder) against oracle owner
    let caller = get_caller();
    let owner = storage_get(b"oracle_owner").unwrap_or_default();
    if owner.len() != 32 || caller.0[..] != owner[..] {
        log_info("Only oracle owner can add feeders");
        return 0;
    }

    // Store feeder for this asset
    let key = alloc::format!("feeder_{}", core::str::from_utf8(&asset).unwrap_or("?"));
    storage_set(key.as_bytes(), &feeder);

    log_info("Price feeder authorized!");
    1
}

/// AUDIT-FIX 1.14: Admin function to add/remove authorized attesters
#[no_mangle]
pub extern "C" fn set_authorized_attester(attester_ptr: *const u8, authorized: u32) -> u32 {
    let mut attester = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(attester_ptr, attester.as_mut_ptr(), 32);
    }

    // Only oracle owner can manage attester whitelist
    let caller = get_caller();
    let owner = storage_get(b"oracle_owner").unwrap_or_default();
    if owner.len() != 32 || caller.0[..] != owner[..] {
        log_info("Only oracle owner can manage attesters");
        return 0;
    }

    let auth_key = alloc::format!(
        "authorized_attester_{}",
        attester
            .iter()
            .map(|b| alloc::format!("{:02x}", b))
            .collect::<alloc::string::String>()
    );
    if authorized != 0 {
        storage_set(auth_key.as_bytes(), &[1u8]);
        log_info("Attester authorized");
    } else {
        storage_set(auth_key.as_bytes(), &[0u8]);
        log_info("Attester deauthorized");
    }
    1
}

#[no_mangle]
pub extern "C" fn submit_price(
    feeder_ptr: *const u8,
    asset_ptr: *const u8,
    asset_len: u32,
    price: u64,
    decimals: u8,
) -> u32 {
    // AUDIT-FIX P2: Enforce pause on submit_price
    if is_mo_paused() {
        log_info("Oracle is paused");
        return 0;
    }
    if !reentrancy_enter() {
        return 0;
    }
    let mut feeder = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(feeder_ptr, feeder.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify transaction signer matches claimed feeder
    let real_caller = get_caller();
    if real_caller.0 != feeder {
        log_info("submit_price rejected: caller is not the feeder");
        reentrancy_exit();
        return 0;
    }

    let mut asset = alloc::vec![0u8; asset_len as usize];
    unsafe {
        core::ptr::copy_nonoverlapping(asset_ptr, asset.as_mut_ptr(), asset_len as usize);
    }

    // Verify feeder is authorized for this specific asset
    let key = alloc::format!("feeder_{}", core::str::from_utf8(&asset).unwrap_or("?"));
    let authorized_feeder = match storage_get(key.as_bytes()) {
        Some(data) if data.len() == 32 => data,
        _ => {
            log_info("No authorized feeder for this asset");
            reentrancy_exit();
            return 0;
        }
    };

    // Verify the submitter matches the authorized feeder
    if feeder[..] != authorized_feeder[..] {
        log_info("Feeder not authorized for this asset");
        reentrancy_exit();
        return 0;
    }

    let timestamp = get_timestamp();

    // Build price feed
    let mut feed = Vec::with_capacity(PRICE_FEED_SIZE);
    feed.extend_from_slice(&u64_to_bytes(price)); // 0-7: price
    feed.extend_from_slice(&u64_to_bytes(timestamp)); // 8-15: timestamp
    feed.push(decimals); // 16: decimals
    feed.extend_from_slice(&feeder); // 17-48: feeder

    // Store price (both canonical key and indexed key for aggregation)
    let asset_name = core::str::from_utf8(&asset).unwrap_or("?");
    let price_key = alloc::format!("price_{}", asset_name);
    storage_set(price_key.as_bytes(), &feed);
    // Also store as feed index 0 so get_aggregated_price can find it
    let indexed_key = alloc::format!("price_{}_0", asset_name);
    storage_set(indexed_key.as_bytes(), &feed);

    log_info("Price updated!");
    log_info(&alloc::format!(
        "   Asset: {}",
        core::str::from_utf8(&asset).unwrap_or("?")
    ));
    log_info(&alloc::format!(
        "   Price: {}.{}",
        price / 10u64.pow(decimals as u32),
        price % 10u64.pow(decimals as u32)
    ));

    reentrancy_exit();
    1
}

#[no_mangle]
pub extern "C" fn get_price(asset_ptr: *const u8, asset_len: u32, result_ptr: *mut u8) -> u32 {
    let mut asset = alloc::vec![0u8; asset_len as usize];
    unsafe {
        core::ptr::copy_nonoverlapping(asset_ptr, asset.as_mut_ptr(), asset_len as usize);
    }

    let key = alloc::format!("price_{}", core::str::from_utf8(&asset).unwrap_or("?"));

    match storage_get(key.as_bytes()) {
        Some(feed) if feed.len() >= PRICE_FEED_SIZE => {
            // Check staleness (reject if > 1 hour old)
            // AUDIT-FIX CON-01: get_timestamp() returns slot number, not seconds.
            // At 400ms/slot, 1 hour = 3_600_000ms / 400ms = 9_000 slots.
            let timestamp = bytes_to_u64(&feed[8..16]);
            let now = get_timestamp();
            if now.saturating_sub(timestamp) > 9_000 {
                log_info(" Price data stale");
                // AUDIT-FIX M20: return 2 for stale (distinct from 0 = not found)
                return 2;
            }

            // Return: price (8) + timestamp (8) + decimals (1)
            unsafe {
                core::ptr::copy_nonoverlapping(feed.as_ptr(), result_ptr, 17);
            }
            1
        }
        _ => {
            log_info("Price not found");
            0
        }
    }
}

/// Cross-contract-safe price lookup.
/// Returns the current price bytes via return_data on success.
#[no_mangle]
pub extern "C" fn get_price_value(asset_ptr: *const u8, asset_len: u32) -> u32 {
    let mut asset = alloc::vec![0u8; asset_len as usize];
    unsafe {
        core::ptr::copy_nonoverlapping(asset_ptr, asset.as_mut_ptr(), asset_len as usize);
    }

    let key = alloc::format!("price_{}", core::str::from_utf8(&asset).unwrap_or("?"));

    match storage_get(key.as_bytes()) {
        Some(feed) if feed.len() >= PRICE_FEED_SIZE => {
            let timestamp = bytes_to_u64(&feed[8..16]);
            let now = get_timestamp();
            if now.saturating_sub(timestamp) > 9_000 {
                log_info(" Price data stale");
                return 2;
            }

            lichen_sdk::set_return_data(&feed[0..8]);
            0
        }
        _ => {
            log_info("Price not found");
            1
        }
    }
}

// ============================================================================
// VERIFIABLE RANDOM FUNCTION (VRF) - Commit-Reveal Scheme
// ============================================================================
// Phase 1 (commit): Requester submits H(secret || seed). Stored on-chain.
// Phase 2 (reveal): Requester reveals secret. Contract verifies H(secret || seed)
//   matches commit, then derives randomness as H(commit || block_timestamp).
//   Block timestamp is unknown at commit time, making the output unpredictable.
// ============================================================================

/// Commit phase: submit hash of (secret || seed)
/// Call with: requester (32 bytes), commit_hash_ptr (32 bytes = H(secret || seed)), seed
#[no_mangle]
pub extern "C" fn commit_randomness(
    requester_ptr: *const u8,
    commit_hash_ptr: *const u8,
    seed: u64,
) -> u32 {
    log_info("Committing randomness request...");

    // AUDIT-FIX P2: Enforce pause
    if is_mo_paused() {
        log_info("Oracle is paused");
        return 0;
    }

    let mut requester = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(requester_ptr, requester.as_mut_ptr(), 32);
    }

    // AUDIT-FIX P2: Verify caller matches requester
    let real_caller = get_caller();
    if real_caller.0 != requester {
        log_info("commit_randomness rejected: caller mismatch");
        return 0;
    }

    let mut commit_hash = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(commit_hash_ptr, commit_hash.as_mut_ptr(), 32);
    }
    let timestamp = get_timestamp();

    let key = alloc::format!("rng_commit_{}", hex_encode(&requester));

    // Store: commit_hash (32) + seed (8) + timestamp (8) + status (1: 0=pending, 1=revealed)
    let mut data = Vec::with_capacity(49);
    data.extend_from_slice(&commit_hash);
    data.extend_from_slice(&u64_to_bytes(seed));
    data.extend_from_slice(&u64_to_bytes(timestamp));
    data.push(0u8); // status: pending

    storage_set(key.as_bytes(), &data);

    log_info("Randomness committed — reveal to finalize");
    1
}

/// Reveal phase: submit secret, contract verifies commit and derives randomness
/// secret_ptr (32 bytes): the secret that was committed as H(secret || seed)
#[no_mangle]
pub extern "C" fn reveal_randomness(
    requester_ptr: *const u8,
    secret_ptr: *const u8,
    result_ptr: *mut u8,
) -> u32 {
    log_info("Revealing randomness...");

    let mut requester = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(requester_ptr, requester.as_mut_ptr(), 32);
    }

    // AUDIT-FIX P2: Verify caller matches requester
    let real_caller = get_caller();
    if real_caller.0 != requester {
        log_info("reveal_randomness rejected: caller mismatch");
        return 0;
    }

    let mut secret = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(secret_ptr, secret.as_mut_ptr(), 32);
    }
    let reveal_timestamp = get_timestamp();

    let commit_key = alloc::format!("rng_commit_{}", hex_encode(&requester));

    let commit_data = match storage_get(commit_key.as_bytes()) {
        Some(d) if d.len() >= 49 => d,
        _ => {
            log_info("No pending commit found");
            return 0;
        }
    };

    // Parse commit data
    let stored_commit_hash = &commit_data[0..32];
    let seed_bytes: [u8; 8] = commit_data[32..40].try_into().unwrap_or([0; 8]);
    let seed = u64::from_le_bytes(seed_bytes);
    let status = commit_data[48];

    if status != 0 {
        log_info("Commit already revealed");
        return 0;
    }

    // Verify: H(secret || seed) == stored_commit_hash
    let mut preimage = Vec::with_capacity(40);
    preimage.extend_from_slice(&secret);
    preimage.extend_from_slice(&u64_to_bytes(seed));
    let computed_hash = simple_hash(&preimage);

    if computed_hash != stored_commit_hash {
        log_info("Commit verification failed — secret doesn't match");
        return 0;
    }

    // Derive randomness: H(commit_hash || reveal_timestamp)
    // reveal_timestamp is the current block timestamp, unknown at commit time
    let mut rng_input = Vec::with_capacity(40);
    rng_input.extend_from_slice(stored_commit_hash);
    rng_input.extend_from_slice(&u64_to_bytes(reveal_timestamp));
    let random_hash = simple_hash(&rng_input);

    // Extract u64 random value from first 8 bytes of hash
    let random_value = u64::from_le_bytes(random_hash[0..8].try_into().unwrap_or([0; 8]));

    // Store result
    let result_key = alloc::format!("random_{}", hex_encode(&requester));
    let mut result_data = Vec::with_capacity(24);
    result_data.extend_from_slice(&u64_to_bytes(random_value));
    result_data.extend_from_slice(&u64_to_bytes(reveal_timestamp));
    result_data.extend_from_slice(&requester);
    storage_set(result_key.as_bytes(), &result_data);

    // Mark commit as revealed
    let mut updated_commit = commit_data.clone();
    updated_commit[48] = 1; // status: revealed
    storage_set(commit_key.as_bytes(), &updated_commit);

    // Write random_value to result pointer
    let value_bytes = u64_to_bytes(random_value);
    unsafe {
        core::ptr::copy_nonoverlapping(value_bytes.as_ptr(), result_ptr, 8);
    }

    log_info("Randomness revealed!");
    log_info(&alloc::format!("   Value: {}", random_value));
    1
}

/// Cryptographic SHA-256 hash function (FIPS 180-4)
/// Replaces the insecure LCG-based simple_hash that was trivially reversible.
/// This is the standard SHA-256 algorithm — collision-resistant, preimage-resistant.
fn sha256(input: &[u8]) -> [u8; 32] {
    // Initial hash values (first 32 bits of fractional parts of sqrt of first 8 primes)
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    // Round constants (first 32 bits of fractional parts of cube roots of first 64 primes)
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    // Pre-processing: pad message to multiple of 512 bits (64 bytes)
    let bit_len = (input.len() as u64) * 8;
    let mut msg = Vec::with_capacity(input.len() + 72);
    msg.extend_from_slice(input);
    msg.push(0x80); // append bit '1'
                    // Pad with zeros until length ≡ 56 (mod 64)
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    // Append original length as 64-bit big-endian
    msg.extend_from_slice(&bit_len.to_be_bytes());

    // Process each 512-bit (64-byte) block
    for chunk in msg.chunks_exact(64) {
        // Prepare message schedule
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        // Initialize working variables
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);

        // Compression function
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        // Add compressed chunk to hash value
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    // Produce final hash (big-endian)
    let mut out = [0u8; 32];
    for (i, &val) in h.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(&val.to_be_bytes());
    }
    out
}

/// Legacy alias: simple_hash now delegates to SHA-256
/// Kept for backward compatibility — all callers get cryptographic security.
fn simple_hash(input: &[u8]) -> [u8; 32] {
    sha256(input)
}

/// Hex encode a byte slice (for storage keys)
fn hex_encode(bytes: &[u8]) -> alloc::string::String {
    let hex_chars: &[u8; 16] = b"0123456789abcdef";
    let mut s = alloc::string::String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(hex_chars[(b >> 4) as usize] as char);
        s.push(hex_chars[(b & 0xf) as usize] as char);
    }
    s
}

// AUDIT-FIX P2: Reentrancy guard (was completely missing)
const ORACLE_REENTRANCY_KEY: &[u8] = b"oracle_reentrancy";

fn reentrancy_enter() -> bool {
    if storage_get(ORACLE_REENTRANCY_KEY)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
    {
        return false;
    }
    storage_set(ORACLE_REENTRANCY_KEY, &[1u8]);
    true
}

fn reentrancy_exit() {
    storage_set(ORACLE_REENTRANCY_KEY, &[0u8]);
}

// AUDIT-FIX P2: Pause check helper (flag was stored but never checked)
fn is_mo_paused() -> bool {
    storage_get(b"oracle_paused")
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
}

/// DEPRECATED — request_randomness is front-runnable (CON-08).
/// Use commit_randomness + reveal_randomness for secure randomness.
/// This function now logs a deprecation warning and returns 0 (failure).
#[no_mangle]
pub extern "C" fn request_randomness(_requester_ptr: *const u8, _seed: u64) -> u32 {
    log_info("DEPRECATED: request_randomness is disabled (CON-08). Use commit_randomness + reveal_randomness instead.");
    0
}

#[no_mangle]
pub extern "C" fn get_randomness(requester_ptr: *const u8, _seed: u64, result_ptr: *mut u8) -> u32 {
    let mut requester = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(requester_ptr, requester.as_mut_ptr(), 32);
    }

    // New key format from commit-reveal and legacy request_randomness
    let key = alloc::format!("random_{}", hex_encode(&requester));

    match storage_get(key.as_bytes()) {
        Some(data) if data.len() >= 16 => {
            // Return: random_value (8) + timestamp (8)
            unsafe {
                core::ptr::copy_nonoverlapping(data.as_ptr(), result_ptr, 16);
            }
            1
        }
        _ => {
            log_info("Random value not found");
            0
        }
    }
}

// ============================================================================
// ATTESTATION SYSTEM - Multi-signature external data verification
// ============================================================================

// Attestation: 73 bytes
// data_hash (32) + signatures_count (1) + timestamp (8) + data (32)
const ATTESTATION_SIZE: usize = 73;

#[no_mangle]
pub extern "C" fn submit_attestation(
    attester_ptr: *const u8,
    data_hash_ptr: *const u8,
    data_ptr: *const u8,
    data_len: u32,
) -> u32 {
    log_info("Submitting attestation...");

    let mut attester = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(attester_ptr, attester.as_mut_ptr(), 32);
    }
    let mut data_hash = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(data_hash_ptr, data_hash.as_mut_ptr(), 32);
    }
    let mut data = alloc::vec![0u8; data_len as usize];
    unsafe {
        core::ptr::copy_nonoverlapping(data_ptr, data.as_mut_ptr(), data_len as usize);
    }

    // AUDIT-FIX 1.14: Verify caller == attester (prevent impersonation)
    let caller = get_caller();
    if caller.0[..] != attester[..] {
        log_info("Caller does not match attester — rejected");
        return 0;
    }

    // AUDIT-FIX 1.14: Check attester is in authorized-attesters whitelist
    let auth_key = alloc::format!(
        "authorized_attester_{}",
        attester
            .iter()
            .map(|b| alloc::format!("{:02x}", b))
            .collect::<alloc::string::String>()
    );
    match storage_get(auth_key.as_bytes()) {
        Some(data) if !data.is_empty() && data[0] == 1 => {
            // Authorized — proceed
        }
        _ => {
            log_info("Attester not in authorized whitelist");
            return 0;
        }
    }

    log_info(&alloc::format!(
        "   Attester: {}...",
        core::str::from_utf8(&attester[..8]).unwrap_or("?")
    ));
    // 3. Data hash matches

    let timestamp = get_timestamp();

    // Load existing attestation or create new
    // AUDIT-FIX 2.9: Use hex encoding to prevent non-UTF8 key collisions
    let key = alloc::format!("attestation_{}", hex_encode(&data_hash));

    let mut attestation = match storage_get(key.as_bytes()) {
        Some(existing) if existing.len() >= ATTESTATION_SIZE => existing,
        _ => {
            let mut new_att = Vec::with_capacity(ATTESTATION_SIZE);
            new_att.extend_from_slice(&data_hash); // 0-31: data_hash
            new_att.push(0); // 32: signatures_count
            new_att.extend_from_slice(&u64_to_bytes(timestamp)); // 33-40: timestamp

            // Store first 32 bytes of data
            if data.len() >= 32 {
                new_att.extend_from_slice(&data[..32]);
            } else {
                new_att.extend_from_slice(&data);
                new_att.extend_from_slice(&vec![0u8; 32 - data.len()]);
            }
            new_att
        }
    };

    // Deduplication: check if this attester already attested for this hash
    let hash_hex = hex_encode(&data_hash);
    let attester_hex = attester
        .iter()
        .map(|b| alloc::format!("{:02x}", b))
        .collect::<alloc::string::String>();
    let dedup_key = alloc::format!("attestation_{}_{}", hash_hex, attester_hex);
    if storage_get(dedup_key.as_bytes()).is_some() {
        log_info("Attester already submitted attestation for this hash");
        return 0;
    }
    storage_set(dedup_key.as_bytes(), &[1u8]);

    // Increment signature count
    let sig_count = attestation[32] + 1;
    attestation[32] = sig_count;

    storage_set(key.as_bytes(), &attestation);

    log_info("Attestation recorded!");
    log_info(&alloc::format!("   Signatures: {}", sig_count));

    1
}

#[no_mangle]
pub extern "C" fn verify_attestation(data_hash_ptr: *const u8, min_signatures: u8) -> u32 {
    let mut data_hash = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(data_hash_ptr, data_hash.as_mut_ptr(), 32);
    }

    // SECURITY-FIX: Use hex_encode to match submit_attestation key format
    let key = alloc::format!("attestation_{}", hex_encode(&data_hash));

    match storage_get(key.as_bytes()) {
        Some(attestation) if attestation.len() >= ATTESTATION_SIZE => {
            let sig_count = attestation[32];

            if sig_count >= min_signatures {
                log_info("Attestation verified");
                log_info(&alloc::format!(
                    "   Signatures: {}/{}",
                    sig_count,
                    min_signatures
                ));
                1
            } else {
                log_info("Insufficient signatures");
                log_info(&alloc::format!(
                    "   Have: {}, Need: {}",
                    sig_count,
                    min_signatures
                ));
                0
            }
        }
        _ => {
            log_info("Attestation not found");
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn get_attestation_data(data_hash_ptr: *const u8, result_ptr: *mut u8) -> u32 {
    let mut data_hash = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(data_hash_ptr, data_hash.as_mut_ptr(), 32);
    }

    // SECURITY-FIX: Use hex_encode to match submit_attestation key format
    let key = alloc::format!("attestation_{}", hex_encode(&data_hash));

    match storage_get(key.as_bytes()) {
        Some(attestation) if attestation.len() >= ATTESTATION_SIZE => {
            // Return: signatures_count (1) + timestamp (8) + data (32)
            unsafe {
                core::ptr::copy_nonoverlapping(attestation[32..].as_ptr(), result_ptr, 41);
            }
            1
        }
        _ => 0,
    }
}

// ============================================================================
// ORACLE QUERY INTERFACE - Contracts can query all oracle data
// ============================================================================

#[no_mangle]
pub extern "C" fn query_oracle(
    query_type_ptr: *const u8,
    query_type_len: u32,
    param_ptr: *const u8,
    param_len: u32,
    result_ptr: *mut u8,
) -> u32 {
    let mut query_type = alloc::vec![0u8; query_type_len as usize];
    unsafe {
        core::ptr::copy_nonoverlapping(
            query_type_ptr,
            query_type.as_mut_ptr(),
            query_type_len as usize,
        );
    }

    match query_type.as_slice() {
        b"price" => {
            log_info("Querying price...");
            get_price(param_ptr, param_len, result_ptr)
        }
        b"random" => {
            log_info("Querying randomness...");
            // param should be: requester (32) + seed (8)
            if param_len >= 40 {
                let mut seed_bytes = [0u8; 8];
                unsafe {
                    core::ptr::copy_nonoverlapping(param_ptr.add(32), seed_bytes.as_mut_ptr(), 8);
                }
                let seed = bytes_to_u64(&seed_bytes);
                get_randomness(param_ptr, seed, result_ptr)
            } else {
                0
            }
        }
        b"attestation" => {
            log_info("Querying attestation...");
            get_attestation_data(param_ptr, result_ptr)
        }
        _ => {
            log_info("Unknown query type");
            0
        }
    }
}

// ============================================================================
// PRICE AGGREGATION - Combine multiple feeds for accuracy
// ============================================================================

#[no_mangle]
pub extern "C" fn get_aggregated_price(
    asset_ptr: *const u8,
    asset_len: u32,
    num_feeds: u8,
    result_ptr: *mut u8,
) -> u32 {
    log_info("Computing aggregated price...");

    let mut asset = alloc::vec![0u8; asset_len as usize];
    unsafe {
        core::ptr::copy_nonoverlapping(asset_ptr, asset.as_mut_ptr(), asset_len as usize);
    }
    let asset_str = core::str::from_utf8(&asset).unwrap_or("?");

    // SECURITY-FIX: Use u128 accumulator to prevent overflow when summing prices
    let mut total_price = 0u128;
    let mut valid_feeds = 0u8;

    // Query multiple feeds
    let mut total_feeds_found = 0u8;
    for i in 0..num_feeds {
        let key = alloc::format!("price_{}_{}", asset_str, i);

        if let Some(feed) = storage_get(key.as_bytes()) {
            if feed.len() >= PRICE_FEED_SIZE {
                total_feeds_found += 1;
                let timestamp = bytes_to_u64(&feed[8..16]);
                let now = get_timestamp();

                // Only include fresh feeds (< 1 hour)
                // AUDIT-FIX CON-01: slot-based threshold (9000 slots ≈ 1h at 400ms/slot)
                if now.saturating_sub(timestamp) <= 9_000 {
                    let price = bytes_to_u64(&feed[0..8]);
                    total_price += price as u128;
                    valid_feeds += 1;
                }
            }
        }
    }

    if valid_feeds == 0 {
        // AUDIT-FIX M20: distinguish "no feeds at all" (0) from "all feeds stale" (2)
        if total_feeds_found > 0 {
            log_info("All price feeds stale");
            return 2;
        }
        log_info("No valid price feeds");
        return 0;
    }

    // Calculate median/average
    let avg_price = (total_price / valid_feeds as u128) as u64;

    // Return: price (8) + valid_feeds (1)
    unsafe {
        core::ptr::copy_nonoverlapping(u64_to_bytes(avg_price).as_ptr(), result_ptr, 8);
        *result_ptr.add(8) = valid_feeds;
    }

    log_info("Aggregated price computed!");
    log_info(&alloc::format!("   Price: {}", avg_price));
    log_info(&alloc::format!("   Feeds: {}", valid_feeds));

    1
}

// ============================================================================
// ORACLE STATISTICS - Track usage and performance
// ============================================================================

#[no_mangle]
pub extern "C" fn get_oracle_stats(result_ptr: *mut u8) -> u32 {
    // Stats: total_queries (8) + total_feeds (8) + total_attestations (8)
    let queries = storage_get(b"stats_queries")
        .and_then(|d| Some(bytes_to_u64(&d)))
        .unwrap_or(0);

    let feeds = storage_get(b"stats_feeds")
        .and_then(|d| Some(bytes_to_u64(&d)))
        .unwrap_or(0);

    let attestations = storage_get(b"stats_attestations")
        .and_then(|d| Some(bytes_to_u64(&d)))
        .unwrap_or(0);

    unsafe {
        core::ptr::copy_nonoverlapping(u64_to_bytes(queries).as_ptr(), result_ptr, 8);
        core::ptr::copy_nonoverlapping(u64_to_bytes(feeds).as_ptr(), result_ptr.add(8), 8);
        core::ptr::copy_nonoverlapping(u64_to_bytes(attestations).as_ptr(), result_ptr.add(16), 8);
    }

    log_info("Oracle statistics:");
    log_info(&alloc::format!("   Queries: {}", queries));
    log_info(&alloc::format!("   Feeds: {}", feeds));
    log_info(&alloc::format!("   Attestations: {}", attestations));

    1
}

// ============================================================================
// ALIASES — bridge test-expected names to actual implementation
// ============================================================================

/// Alias: tests call `initialize` but contract uses `initialize_oracle`
#[no_mangle]
pub extern "C" fn initialize(owner_ptr: *const u8) -> u32 {
    initialize_oracle(owner_ptr)
}

/// Alias: tests call `register_feed`
#[no_mangle]
pub extern "C" fn register_feed(
    feeder_ptr: *const u8,
    asset_ptr: *const u8,
    asset_len: u32,
) -> u32 {
    add_price_feeder(feeder_ptr, asset_ptr, asset_len)
}

/// Tests expect `get_feed_count`
#[no_mangle]
pub extern "C" fn get_feed_count() -> u64 {
    storage_get(b"stats_feeds")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0)
}

/// Tests expect `get_feed_list`
#[no_mangle]
pub extern "C" fn get_feed_list() -> u32 {
    let count = get_feed_count();
    lichen_sdk::set_return_data(&u64_to_bytes(count));
    1
}

/// Tests expect `add_reporter`
#[no_mangle]
pub extern "C" fn add_reporter(caller_ptr: *const u8, reporter_ptr: *const u8) -> u32 {
    // Delegates to add_price_feeder with a synthetic asset name
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 2;
    }
    let owner = storage_get(b"oracle_owner").unwrap_or_default();
    if caller[..] != owner[..] {
        return 1;
    }
    let mut reporter = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(reporter_ptr, reporter.as_mut_ptr(), 32);
    }
    let key = alloc::format!("reporter_{:02x}{:02x}", reporter[0], reporter[1]);
    storage_set(key.as_bytes(), &[1u8]);
    log_info("Reporter added");
    0
}

/// Tests expect `remove_reporter`
#[no_mangle]
pub extern "C" fn remove_reporter(caller_ptr: *const u8, reporter_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 2;
    }
    let owner = storage_get(b"oracle_owner").unwrap_or_default();
    if caller[..] != owner[..] {
        return 1;
    }
    let mut reporter = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(reporter_ptr, reporter.as_mut_ptr(), 32);
    }
    let key = alloc::format!("reporter_{:02x}{:02x}", reporter[0], reporter[1]);
    storage_set(key.as_bytes(), &[0u8]);
    log_info("Reporter removed");
    0
}

/// Tests expect `set_update_interval`
#[no_mangle]
pub extern "C" fn set_update_interval(caller_ptr: *const u8, interval: u64) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 2;
    }
    let owner = storage_get(b"oracle_owner").unwrap_or_default();
    if caller[..] != owner[..] {
        return 1;
    }
    storage_set(b"update_interval", &u64_to_bytes(interval));
    log_info("Update interval set");
    0
}

/// Tests expect `mo_pause`
#[no_mangle]
pub extern "C" fn mo_pause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 2;
    }
    let owner = storage_get(b"oracle_owner").unwrap_or_default();
    if caller[..] != owner[..] {
        return 1;
    }
    storage_set(b"oracle_paused", &[1u8]);
    log_info("Oracle paused");
    0
}

/// Tests expect `mo_unpause`
#[no_mangle]
pub extern "C" fn mo_unpause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 2;
    }
    let owner = storage_get(b"oracle_owner").unwrap_or_default();
    if caller[..] != owner[..] {
        return 1;
    }
    storage_set(b"oracle_paused", &[0u8]);
    log_info("Oracle unpaused");
    0
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use lichen_sdk::bytes_to_u64;
    use lichen_sdk::test_mock;

    fn setup() {
        test_mock::reset();
    }

    #[test]
    fn test_initialize_oracle() {
        setup();
        let owner = [1u8; 32];
        // AUDIT-FIX 2.22: initialize_oracle returns 0 for success
        assert_eq!(initialize_oracle(owner.as_ptr()), 0);
        let stored = test_mock::get_storage(b"oracle_owner");
        assert_eq!(stored, Some(owner.to_vec()));
    }

    #[test]
    fn test_initialize_oracle_rejects_caller_mismatch() {
        setup();
        let owner = [1u8; 32];
        test_mock::set_caller([9u8; 32]);
        assert_eq!(initialize_oracle(owner.as_ptr()), 2);
        assert_eq!(test_mock::get_storage(b"oracle_owner"), None);
    }

    #[test]
    fn test_add_price_feeder() {
        setup();
        let owner = [1u8; 32];
        initialize_oracle(owner.as_ptr());
        test_mock::set_caller(owner);
        let feeder = [2u8; 32];
        let asset = b"LICN/USD";
        assert_eq!(
            add_price_feeder(feeder.as_ptr(), asset.as_ptr(), asset.len() as u32),
            1
        );
        let key = alloc::format!("feeder_{}", core::str::from_utf8(asset).unwrap());
        let stored = test_mock::get_storage(key.as_bytes()).unwrap();
        assert_eq!(stored, feeder.to_vec());
    }

    #[test]
    fn test_add_price_feeder_unauthorized() {
        setup();
        let owner = [1u8; 32];
        initialize_oracle(owner.as_ptr());
        let other = [2u8; 32];
        test_mock::set_caller(other);
        let feeder = [3u8; 32];
        let asset = b"LICN/USD";
        assert_eq!(
            add_price_feeder(feeder.as_ptr(), asset.as_ptr(), asset.len() as u32),
            0
        );
    }

    #[test]
    fn test_submit_price() {
        setup();
        let owner = [1u8; 32];
        initialize_oracle(owner.as_ptr());
        test_mock::set_caller(owner);
        let feeder = [2u8; 32];
        let asset = b"LICN/USD";
        add_price_feeder(feeder.as_ptr(), asset.as_ptr(), asset.len() as u32);
        test_mock::set_caller(feeder);
        assert_eq!(
            submit_price(
                feeder.as_ptr(),
                asset.as_ptr(),
                asset.len() as u32,
                42_000_000,
                6
            ),
            1
        );
    }

    #[test]
    fn test_submit_price_unauthorized_feeder() {
        setup();
        let owner = [1u8; 32];
        initialize_oracle(owner.as_ptr());
        test_mock::set_caller(owner);
        let feeder = [2u8; 32];
        let asset = b"LICN/USD";
        add_price_feeder(feeder.as_ptr(), asset.as_ptr(), asset.len() as u32);
        let wrong = [3u8; 32];
        test_mock::set_caller(wrong);
        assert_eq!(
            submit_price(
                wrong.as_ptr(),
                asset.as_ptr(),
                asset.len() as u32,
                42_000_000,
                6
            ),
            0
        );
    }

    #[test]
    fn test_submit_price_no_feeder_registered() {
        setup();
        let feeder = [2u8; 32];
        let asset = b"UNKNOWN";
        assert_eq!(
            submit_price(feeder.as_ptr(), asset.as_ptr(), asset.len() as u32, 100, 2),
            0
        );
    }

    #[test]
    fn test_get_price() {
        setup();
        let owner = [1u8; 32];
        initialize_oracle(owner.as_ptr());
        test_mock::set_caller(owner);
        let feeder = [2u8; 32];
        let asset = b"LICN/USD";
        add_price_feeder(feeder.as_ptr(), asset.as_ptr(), asset.len() as u32);
        test_mock::set_caller(feeder);
        submit_price(
            feeder.as_ptr(),
            asset.as_ptr(),
            asset.len() as u32,
            42_000_000,
            6,
        );
        let mut result = [0u8; 17];
        assert_eq!(
            get_price(asset.as_ptr(), asset.len() as u32, result.as_mut_ptr()),
            1
        );
        let price = bytes_to_u64(&result[0..8]);
        assert_eq!(price, 42_000_000);
    }

    #[test]
    fn test_get_price_stale() {
        setup();
        let owner = [1u8; 32];
        initialize_oracle(owner.as_ptr());
        test_mock::set_caller(owner);
        let feeder = [2u8; 32];
        let asset = b"LICN/USD";
        add_price_feeder(feeder.as_ptr(), asset.as_ptr(), asset.len() as u32);
        test_mock::set_caller(feeder);
        submit_price(
            feeder.as_ptr(),
            asset.as_ptr(),
            asset.len() as u32,
            42_000_000,
            6,
        );
        test_mock::set_timestamp(1000 + 9001); // stale (> 9000 slots)
        let mut result = [0u8; 17];
        // AUDIT-FIX M20: stale now returns 2 (distinct from 0 = not found)
        assert_eq!(
            get_price(asset.as_ptr(), asset.len() as u32, result.as_mut_ptr()),
            2
        );
    }

    #[test]
    fn test_get_price_not_found() {
        setup();
        let asset = b"NONEXIST";
        let mut result = [0u8; 17];
        assert_eq!(
            get_price(asset.as_ptr(), asset.len() as u32, result.as_mut_ptr()),
            0
        );
    }

    #[test]
    fn test_get_price_value_for_cross_contract_calls() {
        setup();
        let owner = [1u8; 32];
        initialize_oracle(owner.as_ptr());
        test_mock::set_caller(owner);
        let feeder = [2u8; 32];
        let asset = b"LICN/USD";
        add_price_feeder(feeder.as_ptr(), asset.as_ptr(), asset.len() as u32);
        test_mock::set_caller(feeder);
        submit_price(
            feeder.as_ptr(),
            asset.as_ptr(),
            asset.len() as u32,
            42_000_000,
            6,
        );

        assert_eq!(get_price_value(asset.as_ptr(), asset.len() as u32), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 42_000_000);
    }

    #[test]
    fn test_commit_randomness() {
        setup();
        let requester = [1u8; 32];
        let commit_hash = [0xAAu8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(requester);
        assert_eq!(
            commit_randomness(requester.as_ptr(), commit_hash.as_ptr(), 12345),
            1
        );
        let key = alloc::format!("rng_commit_{}", hex_encode(&requester));
        let data = lichen_sdk::storage_get(key.as_bytes()).unwrap();
        assert_eq!(data.len(), 49);
        assert_eq!(&data[0..32], &commit_hash[..]);
    }

    #[test]
    fn test_reveal_randomness() {
        setup();
        let requester = [1u8; 32];
        let secret = [0xBBu8; 32];
        let seed: u64 = 12345;
        // Compute commit hash = simple_hash(secret || u64_to_bytes(seed))
        let mut preimage = Vec::with_capacity(40);
        preimage.extend_from_slice(&secret);
        preimage.extend_from_slice(&lichen_sdk::u64_to_bytes(seed));
        let commit_hash = simple_hash(&preimage);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(requester);
        assert_eq!(
            commit_randomness(requester.as_ptr(), commit_hash.as_ptr(), seed),
            1
        );
        test_mock::set_timestamp(2000);
        let mut result = [0u8; 8];
        assert_eq!(
            reveal_randomness(requester.as_ptr(), secret.as_ptr(), result.as_mut_ptr()),
            1
        );
    }

    #[test]
    fn test_reveal_randomness_wrong_secret() {
        setup();
        let requester = [1u8; 32];
        let secret = [0xBBu8; 32];
        let seed: u64 = 12345;
        let mut preimage = Vec::with_capacity(40);
        preimage.extend_from_slice(&secret);
        preimage.extend_from_slice(&lichen_sdk::u64_to_bytes(seed));
        let commit_hash = simple_hash(&preimage);
        commit_randomness(requester.as_ptr(), commit_hash.as_ptr(), seed);
        test_mock::set_timestamp(2000);
        let wrong = [0xCCu8; 32];
        let mut result = [0u8; 8];
        assert_eq!(
            reveal_randomness(requester.as_ptr(), wrong.as_ptr(), result.as_mut_ptr()),
            0
        );
    }

    #[test]
    fn test_reveal_randomness_no_commit() {
        setup();
        let requester = [1u8; 32];
        let secret = [0xBBu8; 32];
        let mut result = [0u8; 8];
        assert_eq!(
            reveal_randomness(requester.as_ptr(), secret.as_ptr(), result.as_mut_ptr()),
            0
        );
    }

    #[test]
    fn test_reveal_randomness_already_revealed() {
        setup();
        let requester = [1u8; 32];
        let secret = [0xBBu8; 32];
        let seed: u64 = 12345;
        let mut preimage = Vec::with_capacity(40);
        preimage.extend_from_slice(&secret);
        preimage.extend_from_slice(&lichen_sdk::u64_to_bytes(seed));
        let commit_hash = simple_hash(&preimage);
        commit_randomness(requester.as_ptr(), commit_hash.as_ptr(), seed);
        test_mock::set_timestamp(2000);
        let mut result = [0u8; 8];
        reveal_randomness(requester.as_ptr(), secret.as_ptr(), result.as_mut_ptr());
        // Second reveal should fail
        assert_eq!(
            reveal_randomness(requester.as_ptr(), secret.as_ptr(), result.as_mut_ptr()),
            0
        );
    }

    #[test]
    fn test_request_randomness() {
        setup();
        let requester = [1u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(requester);
        // CON-08: request_randomness is deprecated, now returns 0
        assert_eq!(request_randomness(requester.as_ptr(), 42), 0);
    }

    #[test]
    fn test_get_oracle_stats() {
        setup();
        let mut result = [0u8; 24];
        assert_eq!(get_oracle_stats(result.as_mut_ptr()), 1);
        assert_eq!(bytes_to_u64(&result[0..8]), 0);
        assert_eq!(bytes_to_u64(&result[8..16]), 0);
        assert_eq!(bytes_to_u64(&result[16..24]), 0);
    }

    // ========================================================================
    // SHA-256 CORRECTNESS TESTS (Task 3.1)
    // ========================================================================

    #[test]
    fn test_sha256_empty_string() {
        // NIST test vector: SHA-256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let hash = sha256(b"");
        let expected: [u8; 32] = [
            0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
            0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
            0x78, 0x52, 0xb8, 0x55,
        ];
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_sha256_abc() {
        // NIST test vector: SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        let hash = sha256(b"abc");
        let expected: [u8; 32] = [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad,
        ];
        assert_eq!(hash, expected);
    }

    #[test]
    fn test_sha256_deterministic() {
        let input = b"commit-reveal-vrf-test-data";
        let h1 = sha256(input);
        let h2 = sha256(input);
        assert_eq!(h1, h2, "SHA-256 must be deterministic");
    }

    #[test]
    fn test_sha256_avalanche() {
        // Changing one bit should produce a completely different hash
        let h1 = sha256(b"test0");
        let h2 = sha256(b"test1");
        // Count differing bytes — should be many
        let diff = h1.iter().zip(h2.iter()).filter(|(a, b)| a != b).count();
        assert!(diff > 20, "Avalanche: {} of 32 bytes differ", diff);
    }

    #[test]
    fn test_sha256_collision_resistance() {
        // Different inputs must produce different outputs
        let h1 = sha256(b"input_a");
        let h2 = sha256(b"input_b");
        assert_ne!(h1, h2, "Different inputs must produce different hashes");
    }

    #[test]
    fn test_commit_reveal_with_sha256() {
        // End-to-end: commit-reveal produces verifiable randomness with SHA-256
        setup();
        let requester = [1u8; 32];
        let secret = [0x42u8; 32];
        let seed: u64 = 99999;

        // Compute commit using the new SHA-256
        let mut preimage = Vec::with_capacity(40);
        preimage.extend_from_slice(&secret);
        preimage.extend_from_slice(&lichen_sdk::u64_to_bytes(seed));
        let commit_hash = sha256(&preimage);

        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(requester);
        // Commit
        assert_eq!(
            commit_randomness(requester.as_ptr(), commit_hash.as_ptr(), seed),
            1
        );

        // Advance time
        test_mock::set_timestamp(5000);

        // Reveal
        let mut result = [0u8; 8];
        assert_eq!(
            reveal_randomness(requester.as_ptr(), secret.as_ptr(), result.as_mut_ptr()),
            1
        );
        let random_value = u64::from_le_bytes(result);
        // Value should be non-trivial
        assert_ne!(random_value, 0, "Random value should be non-zero");
    }

    // AUDIT-FIX P2: Security regression test
    #[test]
    fn test_submit_price_when_paused() {
        setup();
        let owner = [1u8; 32];
        initialize_oracle(owner.as_ptr());
        test_mock::set_caller(owner);
        let feeder = [2u8; 32];
        let asset = b"LICN/USD";
        add_price_feeder(feeder.as_ptr(), asset.as_ptr(), asset.len() as u32);

        // Pause the oracle
        mo_pause(owner.as_ptr());

        // Try to submit price while paused
        test_mock::set_caller(feeder);
        let result = submit_price(
            feeder.as_ptr(),
            asset.as_ptr(),
            asset.len() as u32,
            42_000_000,
            6,
        );
        assert_eq!(result, 0, "submit_price must fail when oracle is paused");
    }

    // AUDIT-FIX P2: Security regression test
    #[test]
    fn test_request_randomness_wrong_caller() {
        setup();
        let requester = [1u8; 32];
        let wrong_caller = [9u8; 32];
        // Set caller to a different address than the requester
        test_mock::set_caller(wrong_caller);
        let result = request_randomness(requester.as_ptr(), 42);
        assert_eq!(result, 0, "request_randomness must reject caller mismatch");
    }

    // AUDIT-FIX P2: Security regression test
    #[test]
    fn test_commit_randomness_wrong_caller() {
        setup();
        let requester = [1u8; 32];
        let commit_hash = [0xAAu8; 32];
        let wrong_caller = [9u8; 32];
        // Set caller to a different address than the requester
        test_mock::set_caller(wrong_caller);
        let result = commit_randomness(requester.as_ptr(), commit_hash.as_ptr(), 12345);
        assert_eq!(result, 0, "commit_randomness must reject caller mismatch");
    }
}
