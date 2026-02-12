// MoltyID - Agent Identity & Reputation System
// Features: Register identity, skills, reputation scoring, vouching, credentials
//
// Per whitepaper:
//   - Register your agent identity
//   - Build reputation through actions
//   - Verifiable credentials
//   - Portable across platforms

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    Address, log_info, storage_get, storage_set, bytes_to_u64, u64_to_bytes, get_timestamp,
};

// ============================================================================
// IDENTITY CONFIGURATION
// ============================================================================

/// Maximum name length (UTF-8 bytes)
const MAX_NAME_LEN: usize = 64;
/// Maximum skill name length
const MAX_SKILL_LEN: usize = 32;
/// Maximum skills per identity
const MAX_SKILLS: usize = 16;
/// Maximum vouches per identity
const MAX_VOUCHES: usize = 64;
/// Initial reputation score
const INITIAL_REPUTATION: u64 = 100;
/// Minimum reputation (floor)
const MIN_REPUTATION: u64 = 0;
/// Maximum reputation (ceiling)
const MAX_REPUTATION: u64 = 100_000;
/// Reputation cost to vouch (voucher pays this from their own rep)
const VOUCH_COST: u64 = 5;
/// Reputation gained by being vouched for
const VOUCH_REWARD: u64 = 10;

// ---- Hardening constants ----
const MID_PAUSE_KEY: &[u8] = b"mid_paused";
/// Vouch cooldown: minimum time between vouches from same voucher (seconds)
const VOUCH_COOLDOWN_MS: u64 = 3_600_000; // 1 hour
/// Registration cooldown per IP/source (seconds)
const REGISTER_COOLDOWN_MS: u64 = 60_000; // 1 minute

fn is_mid_paused() -> bool {
    storage_get(MID_PAUSE_KEY).map(|d| d.first().copied() == Some(1)).unwrap_or(false)
}
fn is_mid_admin(caller: &[u8]) -> bool {
    storage_get(b"mid_admin").map(|d| d.as_slice() == caller).unwrap_or(false)
}
/// Key for tracking last vouch timestamp per voucher
fn vouch_cooldown_key(voucher: &[u8]) -> Vec<u8> {
    let hex = hex_encode_addr(voucher);
    let mut key = Vec::with_capacity(12 + 64);
    key.extend_from_slice(b"vouch_last:");
    key.extend_from_slice(&hex);
    key
}
/// Key for tracking last registration timestamp per registrant
fn register_cooldown_key(owner: &[u8]) -> Vec<u8> {
    let hex = hex_encode_addr(owner);
    let mut key = Vec::with_capacity(12 + 64);
    key.extend_from_slice(b"reg_last:");
    key.extend_from_slice(&hex);
    key
}

// ============================================================================
// AGENT TYPES
// ============================================================================

/// Agent type identifiers
const AGENT_TYPE_UNKNOWN: u8 = 0;
const AGENT_TYPE_TRADING: u8 = 1;
const AGENT_TYPE_DEVELOPMENT: u8 = 2;
const AGENT_TYPE_ANALYSIS: u8 = 3;
const AGENT_TYPE_CREATIVE: u8 = 4;
const AGENT_TYPE_INFRASTRUCTURE: u8 = 5;
const AGENT_TYPE_GOVERNANCE: u8 = 6;
const AGENT_TYPE_ORACLE: u8 = 7;
const AGENT_TYPE_STORAGE: u8 = 8;
const AGENT_TYPE_GENERAL: u8 = 9;

// ============================================================================
// .MOLT NAMING SYSTEM
// ============================================================================

/// Minimum name length for .molt domains
const MIN_MOLT_NAME_LEN: usize = 3;
/// Maximum name length for .molt domains
const MAX_MOLT_NAME_LEN: usize = 32;
/// Base registration cost (in lamports/shells) for 5+ char names
const NAME_COST_BASE: u64 = 100_000_000; // 100 MOLT (with 6 decimals = 100_000_000)
/// Premium cost for 4-char names
const NAME_COST_4CHAR: u64 = 500_000_000;
/// Premium cost for 3-char names
const NAME_COST_3CHAR: u64 = 1_000_000_000;
/// Slots per year (approx: 2 slots/sec * 86400 * 365)
const SLOTS_PER_YEAR: u64 = 63_072_000;
/// Maximum metadata length
const MAX_METADATA_LEN: usize = 1024;
/// Maximum endpoint URL length
const MAX_ENDPOINT_LEN: usize = 256;

// ============================================================================
// IDENTITY LAYOUT (stored per pubkey)
// ============================================================================
//
// Bytes 0..32   : owner pubkey (Address)
// Byte  32      : agent_type (u8)
// Bytes 33..34  : name_len (u16 LE)
// Bytes 34..98  : name (up to 64 bytes, padded)
// Bytes 98..106 : reputation (u64 LE)
// Bytes 106..114: created_at (u64 LE)
// Bytes 114..122: updated_at (u64 LE)
// Byte  122     : skill_count (u8)
// Bytes 123..126: vouch_count (u16 LE) + flags (u8)
// Byte  126     : is_active (u8, 0 or 1)
//
// Total fixed header: 127 bytes
//
// Skills stored separately: key = "skill:{hex(pubkey)}:{index}"
// Vouches stored separately: key = "vouch:{hex(pubkey)}:{index}"

const IDENTITY_SIZE: usize = 127;

// ============================================================================
// STORAGE KEY HELPERS
// ============================================================================

fn hex_encode_addr(addr: &[u8]) -> [u8; 64] {
    let hex_chars: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 64];
    for i in 0..32 {
        out[i * 2] = hex_chars[(addr[i] >> 4) as usize];
        out[i * 2 + 1] = hex_chars[(addr[i] & 0x0f) as usize];
    }
    out
}

fn identity_key(pubkey: &[u8]) -> Vec<u8> {
    let hex = hex_encode_addr(pubkey);
    let mut key = Vec::with_capacity(3 + 64);
    key.extend_from_slice(b"id:");
    key.extend_from_slice(&hex);
    key
}

fn skill_key(pubkey: &[u8], index: u8) -> Vec<u8> {
    let hex = hex_encode_addr(pubkey);
    let mut key = Vec::with_capacity(7 + 64 + 4);
    key.extend_from_slice(b"skill:");
    key.extend_from_slice(&hex);
    key.push(b':');
    // Encode index as decimal
    if index >= 100 {
        key.push(b'0' + (index / 100));
    }
    if index >= 10 {
        key.push(b'0' + ((index / 10) % 10));
    }
    key.push(b'0' + (index % 10));
    key
}

fn vouch_key(pubkey: &[u8], index: u16) -> Vec<u8> {
    let hex = hex_encode_addr(pubkey);
    let mut key = Vec::with_capacity(7 + 64 + 6);
    key.extend_from_slice(b"vouch:");
    key.extend_from_slice(&hex);
    key.push(b':');
    // Encode index as decimal
    let mut buf = [0u8; 5];
    let mut n = index;
    let mut len = 0;
    if n == 0 {
        key.push(b'0');
    } else {
        while n > 0 {
            buf[len] = b'0' + (n % 10) as u8;
            n /= 10;
            len += 1;
        }
        for i in (0..len).rev() {
            key.push(buf[i]);
        }
    }
    key
}

fn reputation_key(pubkey: &[u8]) -> Vec<u8> {
    let hex = hex_encode_addr(pubkey);
    let mut key = Vec::with_capacity(4 + 64);
    key.extend_from_slice(b"rep:");
    key.extend_from_slice(&hex);
    key
}

fn name_key(name: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(5 + name.len());
    key.extend_from_slice(b"name:");
    key.extend_from_slice(name);
    key
}

fn name_reverse_key(addr: &[u8]) -> Vec<u8> {
    let hex = hex_encode_addr(addr);
    let mut key = Vec::with_capacity(9 + 64);
    key.extend_from_slice(b"name_rev:");
    key.extend_from_slice(&hex);
    key
}

fn endpoint_key(addr: &[u8]) -> Vec<u8> {
    let hex = hex_encode_addr(addr);
    let mut key = Vec::with_capacity(9 + 64);
    key.extend_from_slice(b"endpoint:");
    key.extend_from_slice(&hex);
    key
}

fn metadata_key(addr: &[u8]) -> Vec<u8> {
    let hex = hex_encode_addr(addr);
    let mut key = Vec::with_capacity(9 + 64);
    key.extend_from_slice(b"metadata:");
    key.extend_from_slice(&hex);
    key
}

fn availability_key(addr: &[u8]) -> Vec<u8> {
    let hex = hex_encode_addr(addr);
    let mut key = Vec::with_capacity(13 + 64);
    key.extend_from_slice(b"availability:");
    key.extend_from_slice(&hex);
    key
}

fn rate_key(addr: &[u8]) -> Vec<u8> {
    let hex = hex_encode_addr(addr);
    let mut key = Vec::with_capacity(5 + 64);
    key.extend_from_slice(b"rate:");
    key.extend_from_slice(&hex);
    key
}

/// Validate a .molt name: 3-32 chars, alphanumeric + hyphens, no leading/trailing hyphens, lowercase
fn validate_molt_name(name: &[u8]) -> bool {
    if name.len() < MIN_MOLT_NAME_LEN || name.len() > MAX_MOLT_NAME_LEN {
        return false;
    }
    // Must be lowercase alphanumeric + hyphens
    for &b in name {
        if !((b >= b'a' && b <= b'z') || (b >= b'0' && b <= b'9') || b == b'-') {
            return false;
        }
    }
    // No leading or trailing hyphens
    if name[0] == b'-' || name[name.len() - 1] == b'-' {
        return false;
    }
    // No consecutive hyphens
    for i in 1..name.len() {
        if name[i] == b'-' && name[i - 1] == b'-' {
            return false;
        }
    }
    true
}

/// Check if a name is reserved
fn is_reserved_name(name: &[u8]) -> bool {
    const RESERVED: &[&[u8]] = &[
        b"admin", b"system", b"validator", b"bridge", b"oracle",
        b"moltchain", b"molt", b"moltyid", b"reefstake", b"treasury",
        b"governance", b"dao", b"root", b"node", b"test",
    ];
    for &r in RESERVED {
        if name == r {
            return true;
        }
    }
    false
}

/// Get registration cost based on name length
fn name_registration_cost(name_len: usize) -> u64 {
    match name_len {
        3 => NAME_COST_3CHAR,
        4 => NAME_COST_4CHAR,
        _ => NAME_COST_BASE,
    }
}

// ============================================================================
// INITIALIZATION
// ============================================================================

/// Initialize the MoltyID program.
/// Must be called once to set up admin and program state.
///
/// Parameters:
///   - admin_ptr: pointer to 32-byte admin address
#[no_mangle]
pub extern "C" fn initialize(admin_ptr: *const u8) -> u32 {
    log_info("🪪 Initializing MoltyID program...");

    let admin = unsafe { core::slice::from_raw_parts(admin_ptr, 32) };

    // Check not already initialized
    if storage_get(b"mid_admin").is_some() {
        log_info("❌ MoltyID already initialized");
        return 1;
    }

    storage_set(b"mid_admin", admin);
    storage_set(b"mid_identity_count", &u64_to_bytes(0));
    storage_set(b"mid_initialized", &[1]);

    log_info("✅ MoltyID initialized");
    0
}

// ============================================================================
// REGISTER IDENTITY
// ============================================================================

/// Register a new agent identity.
///
/// Parameters:
///   - owner_ptr: pointer to 32-byte owner address (the agent registering)
///   - agent_type: type of agent (see AGENT_TYPE_* constants)
///   - name_ptr: pointer to name bytes (UTF-8)
///   - name_len: length of name
///
/// Returns 0 on success, nonzero on error.
#[no_mangle]
pub extern "C" fn register_identity(
    owner_ptr: *const u8,
    agent_type: u8,
    name_ptr: *const u8,
    name_len: u32,
) -> u32 {
    log_info("🪪 Registering new MoltyID identity...");

    if is_mid_paused() {
        log_info("❌ MoltyID is paused");
        return 20;
    }

    let owner = unsafe { core::slice::from_raw_parts(owner_ptr, 32) };
    let name_len = name_len as usize;

    if name_len == 0 || name_len > MAX_NAME_LEN {
        log_info("❌ Invalid name length");
        return 1;
    }

    let name = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };

    // Validate agent type
    if agent_type > AGENT_TYPE_GENERAL {
        log_info("❌ Invalid agent type");
        return 2;
    }

    // Check not already registered
    let id_key = identity_key(owner);
    if storage_get(&id_key).is_some() {
        log_info("❌ Identity already registered for this address");
        return 3;
    }

    // Hardening: registration cooldown (checked after duplicate to preserve error codes)
    let now = get_timestamp();
    let rck = register_cooldown_key(owner);
    if let Some(last) = storage_get(&rck) {
        let last_ts = bytes_to_u64(&last);
        if now < last_ts + REGISTER_COOLDOWN_MS {
            log_info("❌ Registration cooldown active");
            return 21;
        }
    }
    storage_set(&rck, &u64_to_bytes(now));

    // Build identity record
    let mut record = [0u8; IDENTITY_SIZE];

    // Bytes 0..32: owner
    record[0..32].copy_from_slice(owner);
    // Byte 32: agent_type
    record[32] = agent_type;
    // Bytes 33..35: name_len (u16 LE)
    record[33] = (name_len & 0xFF) as u8;
    record[34] = ((name_len >> 8) & 0xFF) as u8;
    // Bytes 35..99: name (padded with zeros)
    record[35..35 + name_len].copy_from_slice(name);
    // Bytes 99..107: reputation (u64 LE) — starts at INITIAL_REPUTATION
    let rep_bytes = u64_to_bytes(INITIAL_REPUTATION);
    record[99..107].copy_from_slice(&rep_bytes);
    // Bytes 107..115: created_at
    let ts_bytes = u64_to_bytes(now);
    record[107..115].copy_from_slice(&ts_bytes);
    // Bytes 115..123: updated_at
    record[115..123].copy_from_slice(&ts_bytes);
    // Byte 123: skill_count = 0
    record[123] = 0;
    // Bytes 124..126: vouch_count (u16 LE) = 0
    record[124] = 0;
    record[125] = 0;
    // Byte 126: is_active = 1
    record[126] = 1;

    storage_set(&id_key, &record);

    // Also store reputation separately for quick lookups
    let rep_key = reputation_key(owner);
    storage_set(&rep_key, &rep_bytes);

    // Increment global identity count
    let count = match storage_get(b"mid_identity_count") {
        Some(data) if data.len() >= 8 => bytes_to_u64(&data),
        _ => 0,
    };
    storage_set(b"mid_identity_count", &u64_to_bytes(count + 1));

    log_info("✅ Identity registered successfully");
    0
}

// ============================================================================
// GET IDENTITY
// ============================================================================

/// Query identity data for a given pubkey.
/// Returns the identity record bytes via contract return data,
/// or error code if not found.
///
/// Parameters:
///   - pubkey_ptr: pointer to 32-byte address to look up
#[no_mangle]
pub extern "C" fn get_identity(pubkey_ptr: *const u8) -> u32 {
    let pubkey = unsafe { core::slice::from_raw_parts(pubkey_ptr, 32) };
    let id_key = identity_key(pubkey);

    match storage_get(&id_key) {
        Some(data) => {
            moltchain_sdk::set_return_data(&data);
            0
        }
        None => {
            log_info("❌ Identity not found");
            1
        }
    }
}

// ============================================================================
// UPDATE REPUTATION (whitepaper formula)
// ============================================================================

/// Whitepaper reputation formula:
///   reputation = (successful_txs * 10 + governance_participated * 50 +
///                 programs_deployed * 100 + uptime_hours * 1 + peer_endorsements * 25)
///                / (1 + failed_txs * 5 + slashing_events * 100)
///
/// Simplified on-chain: admin can update with delta and contribution type.
/// Contribution types:
///   0 = successful_tx (+10)
///   1 = governance_participation (+50)
///   2 = program_deployed (+100)
///   3 = uptime_hour (+1)
///   4 = peer_endorsement (+25)
///   5 = failed_tx (-5)
///   6 = slashing_event (-100)
///
/// Parameters:
///   - caller_ptr: 32-byte caller address (must be admin)
///   - target_ptr: 32-byte target agent address
///   - contribution_type: type of contribution (see above)
///   - count: number of contributions of this type
#[no_mangle]
pub extern "C" fn update_reputation_typed(
    caller_ptr: *const u8,
    target_ptr: *const u8,
    contribution_type: u8,
    count: u64,
) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let target = unsafe { core::slice::from_raw_parts(target_ptr, 32) };

    // Only admin can update reputation
    let admin = match storage_get(b"mid_admin") {
        Some(data) => data,
        None => {
            log_info("❌ MoltyID not initialized");
            return 1;
        }
    };
    if caller != admin.as_slice() {
        log_info("❌ Unauthorized: only admin can update reputation");
        return 2;
    }

    let id_key = identity_key(target);
    let mut record = match storage_get(&id_key) {
        Some(data) => data,
        None => {
            log_info("❌ Target identity not found");
            return 3;
        }
    };

    let current_rep = if record.len() >= 107 {
        bytes_to_u64(&record[99..107])
    } else {
        INITIAL_REPUTATION
    };

    // Calculate delta based on contribution type per whitepaper formula
    let (is_positive, weight) = match contribution_type {
        0 => (true, 10u64),   // successful_tx
        1 => (true, 50u64),   // governance_participation
        2 => (true, 100u64),  // program_deployed
        3 => (true, 1u64),    // uptime_hour
        4 => (true, 25u64),   // peer_endorsement
        5 => (false, 5u64),   // failed_tx
        6 => (false, 100u64), // slashing_event
        _ => {
            log_info("❌ Invalid contribution type");
            return 4;
        }
    };

    let delta = weight.saturating_mul(count);
    let new_rep = if is_positive {
        let sum = current_rep.saturating_add(delta);
        if sum > MAX_REPUTATION { MAX_REPUTATION } else { sum }
    } else {
        if delta > current_rep { MIN_REPUTATION } else { current_rep - delta }
    };

    let rep_bytes = u64_to_bytes(new_rep);
    if record.len() >= 107 {
        record[99..107].copy_from_slice(&rep_bytes);
    }
    let now_bytes = u64_to_bytes(get_timestamp());
    if record.len() >= 123 {
        record[115..123].copy_from_slice(&now_bytes);
    }

    storage_set(&id_key, &record);
    storage_set(&reputation_key(target), &rep_bytes);

    // Track contribution counts for the formula
    let hex = hex_encode_addr(target);
    let counter_key = {
        let mut k = Vec::with_capacity(6 + 64 + 2);
        k.extend_from_slice(b"cont:");
        k.extend_from_slice(&hex);
        k.push(b':');
        k.push(b'0' + contribution_type);
        k
    };
    let prev = storage_get(&counter_key)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    storage_set(&counter_key, &u64_to_bytes(prev + count));

    // Check for graduation and achievements
    check_achievements(target, new_rep);

    log_info(&alloc::format!("✅ Reputation updated: {} → {} (type: {}, Δ: {}{})",
        current_rep, new_rep, contribution_type,
        if is_positive { "+" } else { "-" }, delta));
    0
}

/// Update an agent's reputation score.
/// Only callable by the program admin.
///
/// Parameters:
///   - caller_ptr: 32-byte caller address (must be admin)
///   - target_ptr: 32-byte target agent address
///   - delta: amount to change (positive = increase)
///   - is_increase: 1 = add, 0 = subtract
#[no_mangle]
pub extern "C" fn update_reputation(
    caller_ptr: *const u8,
    target_ptr: *const u8,
    delta: u64,
    is_increase: u8,
) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let target = unsafe { core::slice::from_raw_parts(target_ptr, 32) };

    // Only admin can directly update reputation
    let admin = match storage_get(b"mid_admin") {
        Some(data) => data,
        None => {
            log_info("❌ MoltyID not initialized");
            return 1;
        }
    };
    if caller != admin.as_slice() {
        log_info("❌ Unauthorized: only admin can update reputation");
        return 2;
    }

    // Check identity exists
    let id_key = identity_key(target);
    let mut record = match storage_get(&id_key) {
        Some(data) => data,
        None => {
            log_info("❌ Target identity not found");
            return 3;
        }
    };

    // Read current reputation
    let current_rep = if record.len() >= 107 {
        bytes_to_u64(&record[99..107])
    } else {
        INITIAL_REPUTATION
    };

    // Apply delta with bounds
    let new_rep = if is_increase == 1 {
        let sum = current_rep.saturating_add(delta);
        if sum > MAX_REPUTATION { MAX_REPUTATION } else { sum }
    } else {
        if delta > current_rep {
            MIN_REPUTATION
        } else {
            current_rep - delta
        }
    };

    // Update record
    let rep_bytes = u64_to_bytes(new_rep);
    if record.len() >= 107 {
        record[99..107].copy_from_slice(&rep_bytes);
    }
    let now_bytes = u64_to_bytes(get_timestamp());
    if record.len() >= 123 {
        record[115..123].copy_from_slice(&now_bytes);
    }

    storage_set(&id_key, &record);

    // Update standalone reputation key
    let rep_key = reputation_key(target);
    storage_set(&rep_key, &rep_bytes);

    log_info("✅ Reputation updated");
    0
}

// ============================================================================
// ADD SKILL
// ============================================================================

/// Add a skill to an agent's identity.
/// Only the identity owner can add their own skills.
///
/// Parameters:
///   - caller_ptr: 32-byte caller address (must be identity owner)
///   - skill_name_ptr: pointer to skill name bytes (UTF-8)
///   - skill_name_len: length of skill name
///   - proficiency: skill proficiency level 0-100
#[no_mangle]
pub extern "C" fn add_skill(
    caller_ptr: *const u8,
    skill_name_ptr: *const u8,
    skill_name_len: u32,
    proficiency: u8,
) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let skill_name_len = skill_name_len as usize;

    if skill_name_len == 0 || skill_name_len > MAX_SKILL_LEN {
        log_info("❌ Invalid skill name length");
        return 1;
    }

    if proficiency > 100 {
        log_info("❌ Proficiency must be 0-100");
        return 2;
    }

    let skill_name = unsafe { core::slice::from_raw_parts(skill_name_ptr, skill_name_len) };

    // Load identity
    let id_key = identity_key(caller);
    let mut record = match storage_get(&id_key) {
        Some(data) => data,
        None => {
            log_info("❌ Identity not found — register first");
            return 3;
        }
    };

    // Verify caller owns this identity
    if record.len() < IDENTITY_SIZE || &record[0..32] != caller {
        log_info("❌ Unauthorized: not identity owner");
        return 4;
    }

    // Check skill count limit
    let skill_count = record[123];
    if skill_count as usize >= MAX_SKILLS {
        log_info("❌ Maximum skills reached");
        return 5;
    }

    // Store skill: [name_len(1), name(up to 32), proficiency(1), timestamp(8)]
    let mut skill_data = Vec::with_capacity(1 + skill_name_len + 1 + 8);
    skill_data.push(skill_name_len as u8);
    skill_data.extend_from_slice(skill_name);
    skill_data.push(proficiency);
    let ts_bytes = u64_to_bytes(get_timestamp());
    skill_data.extend_from_slice(&ts_bytes);

    let sk = skill_key(caller, skill_count);
    storage_set(&sk, &skill_data);

    // Increment skill count in identity record
    record[123] = skill_count + 1;
    // Update updated_at
    if record.len() >= 123 {
        record[115..123].copy_from_slice(&ts_bytes);
    }
    storage_set(&id_key, &record);

    log_info("✅ Skill added");
    0
}

// ============================================================================
// GET SKILLS
// ============================================================================

/// Get all skills for an identity.
/// Returns concatenated skill data via return data.
///
/// Parameters:
///   - pubkey_ptr: 32-byte address to look up
#[no_mangle]
pub extern "C" fn get_skills(pubkey_ptr: *const u8) -> u32 {
    let pubkey = unsafe { core::slice::from_raw_parts(pubkey_ptr, 32) };

    let id_key = identity_key(pubkey);
    let record = match storage_get(&id_key) {
        Some(data) => data,
        None => {
            log_info("❌ Identity not found");
            return 1;
        }
    };

    let skill_count = if record.len() > 123 { record[123] } else { 0 };
    let mut all_skills = Vec::new();
    all_skills.push(skill_count);

    for i in 0..skill_count {
        let sk = skill_key(pubkey, i);
        if let Some(data) = storage_get(&sk) {
            all_skills.extend_from_slice(&data);
        }
    }

    moltchain_sdk::set_return_data(&all_skills);
    0
}

// ============================================================================
// VOUCH
// ============================================================================

/// One agent vouches for another, transferring reputation.
/// The voucher pays VOUCH_COST reputation, the vouchee gains VOUCH_REWARD.
///
/// Parameters:
///   - voucher_ptr: 32-byte voucher address
///   - vouchee_ptr: 32-byte vouchee address
#[no_mangle]
pub extern "C" fn vouch(voucher_ptr: *const u8, vouchee_ptr: *const u8) -> u32 {
    let voucher = unsafe { core::slice::from_raw_parts(voucher_ptr, 32) };
    let vouchee = unsafe { core::slice::from_raw_parts(vouchee_ptr, 32) };

    if is_mid_paused() {
        return 20;
    }

    // Can't vouch for yourself
    if voucher == vouchee {
        log_info("❌ Cannot vouch for yourself");
        return 1;
    }

    // Both must have identities
    let voucher_id_key = identity_key(voucher);
    let vouchee_id_key = identity_key(vouchee);

    let mut voucher_record = match storage_get(&voucher_id_key) {
        Some(data) => data,
        None => {
            log_info("❌ Voucher identity not found");
            return 2;
        }
    };

    let mut vouchee_record = match storage_get(&vouchee_id_key) {
        Some(data) => data,
        None => {
            log_info("❌ Vouchee identity not found");
            return 3;
        }
    };

    // Check voucher has enough reputation
    let voucher_rep = if voucher_record.len() >= 107 {
        bytes_to_u64(&voucher_record[99..107])
    } else {
        0
    };

    if voucher_rep < VOUCH_COST {
        log_info("❌ Insufficient reputation to vouch");
        return 4;
    }

    // Check vouchee vouch count limit
    let vouchee_vouch_count = if vouchee_record.len() >= 126 {
        (vouchee_record[124] as u16) | ((vouchee_record[125] as u16) << 8)
    } else {
        0
    };

    if vouchee_vouch_count as usize >= MAX_VOUCHES {
        log_info("❌ Vouchee has reached maximum vouches");
        return 5;
    }

    // Check voucher hasn't already vouched for this vouchee
    for i in 0..vouchee_vouch_count {
        let vk = vouch_key(vouchee, i);
        if let Some(data) = storage_get(&vk) {
            if data.len() >= 32 && &data[0..32] == voucher {
                log_info("❌ Already vouched for this agent");
                return 6;
            }
        }
    }

    // Hardening: vouch cooldown (after all other checks to preserve error codes)
    let vck = vouch_cooldown_key(voucher);
    let now = get_timestamp();
    if let Some(last) = storage_get(&vck) {
        let last_ts = bytes_to_u64(&last);
        if now < last_ts + VOUCH_COOLDOWN_MS {
            log_info("❌ Vouch cooldown active");
            return 21;
        }
    }
    storage_set(&vck, &u64_to_bytes(now));

    let ts_bytes = u64_to_bytes(now);

    // Store vouch record: [voucher_addr(32), timestamp(8)]
    let mut vouch_data = Vec::with_capacity(40);
    vouch_data.extend_from_slice(voucher);
    vouch_data.extend_from_slice(&ts_bytes);

    let vk = vouch_key(vouchee, vouchee_vouch_count);
    storage_set(&vk, &vouch_data);

    // Deduct reputation from voucher
    let new_voucher_rep = voucher_rep - VOUCH_COST;
    let voucher_rep_bytes = u64_to_bytes(new_voucher_rep);
    if voucher_record.len() >= 107 {
        voucher_record[99..107].copy_from_slice(&voucher_rep_bytes);
        voucher_record[115..123].copy_from_slice(&ts_bytes);
    }
    storage_set(&voucher_id_key, &voucher_record);
    storage_set(&reputation_key(voucher), &voucher_rep_bytes);

    // Add reputation to vouchee
    let vouchee_rep = if vouchee_record.len() >= 107 {
        bytes_to_u64(&vouchee_record[99..107])
    } else {
        INITIAL_REPUTATION
    };
    let new_vouchee_rep = {
        let sum = vouchee_rep.saturating_add(VOUCH_REWARD);
        if sum > MAX_REPUTATION { MAX_REPUTATION } else { sum }
    };
    let vouchee_rep_bytes = u64_to_bytes(new_vouchee_rep);

    // Update vouchee record: reputation + vouch_count + updated_at
    if vouchee_record.len() >= IDENTITY_SIZE {
        vouchee_record[99..107].copy_from_slice(&vouchee_rep_bytes);
        let new_count = vouchee_vouch_count + 1;
        vouchee_record[124] = (new_count & 0xFF) as u8;
        vouchee_record[125] = ((new_count >> 8) & 0xFF) as u8;
        vouchee_record[115..123].copy_from_slice(&ts_bytes);
    }
    storage_set(&vouchee_id_key, &vouchee_record);
    storage_set(&reputation_key(vouchee), &vouchee_rep_bytes);

    log_info("✅ Vouch recorded successfully");
    0
}

// ============================================================================
// GET REPUTATION
// ============================================================================

/// Quick reputation lookup for an address.
///
/// Parameters:
///   - pubkey_ptr: 32-byte address
#[no_mangle]
pub extern "C" fn get_reputation(pubkey_ptr: *const u8) -> u32 {
    let pubkey = unsafe { core::slice::from_raw_parts(pubkey_ptr, 32) };

    let rep_key = reputation_key(pubkey);
    match storage_get(&rep_key) {
        Some(data) if data.len() >= 8 => {
            moltchain_sdk::set_return_data(&data);
            0
        }
        _ => {
            log_info("❌ No reputation found for address");
            1
        }
    }
}

// ============================================================================
// DEACTIVATE IDENTITY
// ============================================================================

/// Deactivate an identity. Only the owner or admin can do this.
///
/// Parameters:
///   - caller_ptr: 32-byte caller address
///   - target_ptr: 32-byte target identity to deactivate
#[no_mangle]
pub extern "C" fn deactivate_identity(
    caller_ptr: *const u8,
    target_ptr: *const u8,
) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let target = unsafe { core::slice::from_raw_parts(target_ptr, 32) };

    let id_key = identity_key(target);
    let mut record = match storage_get(&id_key) {
        Some(data) => data,
        None => {
            log_info("❌ Identity not found");
            return 1;
        }
    };

    // Must be owner or admin
    let is_owner = record.len() >= 32 && &record[0..32] == caller;
    let is_admin = match storage_get(b"mid_admin") {
        Some(admin) => caller == admin.as_slice(),
        None => false,
    };

    if !is_owner && !is_admin {
        log_info("❌ Unauthorized: must be owner or admin");
        return 2;
    }

    // Set is_active = 0
    if record.len() >= IDENTITY_SIZE {
        record[126] = 0;
        let ts_bytes = u64_to_bytes(get_timestamp());
        record[115..123].copy_from_slice(&ts_bytes);
    }

    storage_set(&id_key, &record);

    log_info("✅ Identity deactivated");
    0
}

// ============================================================================
// GET IDENTITY COUNT
// ============================================================================

/// Get total number of registered identities.
#[no_mangle]
pub extern "C" fn get_identity_count() -> u32 {
    match storage_get(b"mid_identity_count") {
        Some(data) if data.len() >= 8 => {
            moltchain_sdk::set_return_data(&data);
            0
        }
        _ => {
            moltchain_sdk::set_return_data(&u64_to_bytes(0));
            0
        }
    }
}

// ============================================================================
// UPDATE AGENT TYPE
// ============================================================================

/// Update the agent type for an identity. Only the owner can do this.
///
/// Parameters:
///   - caller_ptr: 32-byte caller address (must be owner)
///   - new_agent_type: new agent type value
#[no_mangle]
pub extern "C" fn update_agent_type(
    caller_ptr: *const u8,
    new_agent_type: u8,
) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };

    if new_agent_type > AGENT_TYPE_GENERAL {
        log_info("❌ Invalid agent type");
        return 1;
    }

    let id_key = identity_key(caller);
    let mut record = match storage_get(&id_key) {
        Some(data) => data,
        None => {
            log_info("❌ Identity not found");
            return 2;
        }
    };

    // Verify ownership
    if record.len() < IDENTITY_SIZE || &record[0..32] != caller {
        log_info("❌ Unauthorized");
        return 3;
    }

    record[32] = new_agent_type;
    let ts_bytes = u64_to_bytes(get_timestamp());
    record[115..123].copy_from_slice(&ts_bytes);

    storage_set(&id_key, &record);

    log_info("✅ Agent type updated");
    0
}

// ============================================================================
// GET VOUCHES
// ============================================================================

/// Get all vouches for an identity.
/// Returns concatenated vouch data via return data.
///
/// Parameters:
///   - pubkey_ptr: 32-byte address to look up
#[no_mangle]
pub extern "C" fn get_vouches(pubkey_ptr: *const u8) -> u32 {
    let pubkey = unsafe { core::slice::from_raw_parts(pubkey_ptr, 32) };

    let id_key = identity_key(pubkey);
    let record = match storage_get(&id_key) {
        Some(data) => data,
        None => {
            log_info("❌ Identity not found");
            return 1;
        }
    };

    let vouch_count = if record.len() >= 126 {
        (record[124] as u16) | ((record[125] as u16) << 8)
    } else {
        0
    };

    let mut all_vouches = Vec::new();
    // Write count as u16 LE
    all_vouches.push((vouch_count & 0xFF) as u8);
    all_vouches.push(((vouch_count >> 8) & 0xFF) as u8);

    for i in 0..vouch_count {
        let vk = vouch_key(pubkey, i);
        if let Some(data) = storage_get(&vk) {
            all_vouches.extend_from_slice(&data);
        }
    }

    moltchain_sdk::set_return_data(&all_vouches);
    0
}

// ============================================================================
// ACHIEVEMENTS & GRADUATION (per whitepaper)
// ============================================================================

/// Achievement IDs (per whitepaper):
const ACHIEVEMENT_FIRST_TX: u8 = 1;        // First successful transaction
const ACHIEVEMENT_VOTER: u8 = 2;           // Participated in governance
const ACHIEVEMENT_BUILDER: u8 = 3;         // Deployed a program
const ACHIEVEMENT_TRUSTED: u8 = 4;         // Reached reputation 500
const ACHIEVEMENT_VETERAN: u8 = 5;         // Reached reputation 1000
const ACHIEVEMENT_LEGEND: u8 = 6;          // Reached reputation 5000
const ACHIEVEMENT_ENDORSED: u8 = 7;        // Received 10+ vouches
const ACHIEVEMENT_GRADUATION: u8 = 8;      // Graduated (bootstrap debt repaid)

/// Check and award achievements based on reputation milestones
fn check_achievements(target: &[u8], reputation: u64) {
    let hex = hex_encode_addr(target);

    // Check reputation milestones
    if reputation >= 500 {
        award_achievement(target, &hex, ACHIEVEMENT_TRUSTED, "Trusted Agent (rep 500+)");
    }
    if reputation >= 1000 {
        award_achievement(target, &hex, ACHIEVEMENT_VETERAN, "Veteran Agent (rep 1000+)");
    }
    if reputation >= 5000 {
        award_achievement(target, &hex, ACHIEVEMENT_LEGEND, "Legendary Agent (rep 5000+)");
    }
}

/// Award an achievement if not already earned
fn award_achievement(target: &[u8], hex: &[u8; 64], achievement_id: u8, name: &str) {
    let mut ach_key = Vec::with_capacity(5 + 64 + 4);
    ach_key.extend_from_slice(b"ach:");
    ach_key.extend_from_slice(hex);
    ach_key.push(b':');
    ach_key.push(b'0' + (achievement_id / 10));
    ach_key.push(b'0' + (achievement_id % 10));

    if storage_get(&ach_key).is_some() {
        return; // Already earned
    }

    // Store achievement: [achievement_id(1), timestamp(8)]
    let mut ach_data = Vec::with_capacity(9);
    ach_data.push(achievement_id);
    ach_data.extend_from_slice(&u64_to_bytes(get_timestamp()));
    storage_set(&ach_key, &ach_data);

    // Increment achievement count
    let mut count_key = Vec::with_capacity(9 + 64);
    count_key.extend_from_slice(b"ach_count:");
    count_key.extend_from_slice(hex);
    let prev = storage_get(&count_key)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    storage_set(&count_key, &u64_to_bytes(prev + 1));

    log_info(&alloc::format!("🏆 Achievement unlocked: {}", name));
    let _ = target; // suppress unused warning
}

/// Award a contribution-based achievement (called externally by admin)
#[no_mangle]
pub extern "C" fn award_contribution_achievement(
    caller_ptr: *const u8,
    target_ptr: *const u8,
    achievement_id: u8,
) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let target = unsafe { core::slice::from_raw_parts(target_ptr, 32) };

    let admin = match storage_get(b"mid_admin") {
        Some(data) => data,
        None => return 1,
    };
    if caller != admin.as_slice() {
        log_info("❌ Unauthorized");
        return 2;
    }

    let hex = hex_encode_addr(target);
    let name = match achievement_id {
        ACHIEVEMENT_FIRST_TX => "First Transaction",
        ACHIEVEMENT_VOTER => "Governance Voter",
        ACHIEVEMENT_BUILDER => "Program Builder",
        ACHIEVEMENT_ENDORSED => "Well Endorsed (10+ vouches)",
        ACHIEVEMENT_GRADUATION => "Bootstrap Graduation 🎓",
        _ => "Unknown Achievement",
    };
    award_achievement(target, &hex, achievement_id, name);
    0
}

/// Get achievements for an identity
#[no_mangle]
pub extern "C" fn get_achievements(pubkey_ptr: *const u8) -> u32 {
    let pubkey = unsafe { core::slice::from_raw_parts(pubkey_ptr, 32) };
    let hex = hex_encode_addr(pubkey);

    let mut count_key = Vec::with_capacity(9 + 64);
    count_key.extend_from_slice(b"ach_count:");
    count_key.extend_from_slice(&hex);
    let count = storage_get(&count_key)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);

    let mut result = Vec::new();
    result.extend_from_slice(&u64_to_bytes(count));

    // Collect all achievements
    for id in 1..=8u8 {
        let mut ach_key = Vec::with_capacity(5 + 64 + 4);
        ach_key.extend_from_slice(b"ach:");
        ach_key.extend_from_slice(&hex);
        ach_key.push(b':');
        ach_key.push(b'0' + (id / 10));
        ach_key.push(b'0' + (id % 10));

        if let Some(data) = storage_get(&ach_key) {
            result.extend_from_slice(&data);
        }
    }

    moltchain_sdk::set_return_data(&result);
    0
}

// ============================================================================
// SKILL ATTESTATION HELPERS
// ============================================================================

/// Build a simple hash of a skill name (first 8 bytes, zero-padded)
fn skill_name_hash(skill_name: &[u8]) -> [u8; 8] {
    let mut hash = [0u8; 8];
    for (i, &b) in skill_name.iter().enumerate() {
        if i >= 8 { break; }
        hash[i] = b;
    }
    hash
}

fn hex_encode_8(bytes: &[u8; 8]) -> [u8; 16] {
    let hex_chars: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 16];
    for i in 0..8 {
        out[i * 2] = hex_chars[(bytes[i] >> 4) as usize];
        out[i * 2 + 1] = hex_chars[(bytes[i] & 0x0f) as usize];
    }
    out
}

/// Storage key for an attestation: "attest_{identity_hex}_{skill_hash_hex}_{attester_hex}"
fn attestation_key(identity: &[u8], skill_hash: &[u8; 8], attester: &[u8]) -> Vec<u8> {
    let id_hex = hex_encode_addr(identity);
    let skill_hex = hex_encode_8(skill_hash);
    let att_hex = hex_encode_addr(attester);
    let mut key = Vec::with_capacity(7 + 64 + 1 + 16 + 1 + 64);
    key.extend_from_slice(b"attest_");
    key.extend_from_slice(&id_hex);
    key.push(b'_');
    key.extend_from_slice(&skill_hex);
    key.push(b'_');
    key.extend_from_slice(&att_hex);
    key
}

/// Storage key for attestation count: "attest_count_{identity_hex}_{skill_hash_hex}"
fn attestation_count_key(identity: &[u8], skill_hash: &[u8; 8]) -> Vec<u8> {
    let id_hex = hex_encode_addr(identity);
    let skill_hex = hex_encode_8(skill_hash);
    let mut key = Vec::with_capacity(13 + 64 + 1 + 16);
    key.extend_from_slice(b"attest_count_");
    key.extend_from_slice(&id_hex);
    key.push(b'_');
    key.extend_from_slice(&skill_hex);
    key
}

// ============================================================================
// ATTEST SKILL
// ============================================================================

/// A third party attests to someone's skill proficiency.
///
/// Parameters:
///   - attester_ptr: 32-byte attester address (the one giving attestation)
///   - identity_ptr: 32-byte identity address (the one being attested)
///   - skill_name_ptr: pointer to skill name bytes (UTF-8)
///   - skill_name_len: length of skill name
///   - attestation_level: attestation level 1-5
///
/// Returns 0 on success, nonzero on error.
#[no_mangle]
pub extern "C" fn attest_skill(
    attester_ptr: *const u8,
    identity_ptr: *const u8,
    skill_name_ptr: *const u8,
    skill_name_len: u32,
    attestation_level: u8,
) -> u32 {
    log_info("🏅 Attesting skill...");

    let attester = unsafe { core::slice::from_raw_parts(attester_ptr, 32) };
    let identity = unsafe { core::slice::from_raw_parts(identity_ptr, 32) };
    let skill_name_len = skill_name_len as usize;

    if skill_name_len == 0 || skill_name_len > MAX_SKILL_LEN {
        log_info("❌ Invalid skill name length");
        return 1;
    }

    if attestation_level == 0 || attestation_level > 5 {
        log_info("❌ Attestation level must be 1-5");
        return 2;
    }

    // Can't attest your own skills
    if attester == identity {
        log_info("❌ Cannot attest your own skills");
        return 3;
    }

    let skill_name = unsafe { core::slice::from_raw_parts(skill_name_ptr, skill_name_len) };

    // Both must have identities
    let id_key = identity_key(identity);
    if storage_get(&id_key).is_none() {
        log_info("❌ Target identity not found");
        return 4;
    }

    let attester_id_key = identity_key(attester);
    if storage_get(&attester_id_key).is_none() {
        log_info("❌ Attester identity not found");
        return 5;
    }

    let s_hash = skill_name_hash(skill_name);
    let ak = attestation_key(identity, &s_hash, attester);

    if storage_get(&ak).is_some() {
        log_info("❌ Already attested this skill for this identity");
        return 6;
    }

    // Store attestation: level (1 byte) + timestamp (8 bytes)
    let mut att_data = Vec::with_capacity(9);
    att_data.push(attestation_level);
    att_data.extend_from_slice(&u64_to_bytes(get_timestamp()));
    storage_set(&ak, &att_data);

    // Increment attestation count
    let ck = attestation_count_key(identity, &s_hash);
    let count = storage_get(&ck)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    storage_set(&ck, &u64_to_bytes(count + 1));

    log_info("✅ Skill attestation recorded");
    0
}

// ============================================================================
// GET ATTESTATIONS
// ============================================================================

/// Get attestation count for a specific skill of an identity.
///
/// Parameters:
///   - identity_ptr: 32-byte identity address
///   - skill_name_ptr: pointer to skill name bytes
///   - skill_name_len: length of skill name
///
/// Returns 0 on success (attestation count as return data), 1 on error.
#[no_mangle]
pub extern "C" fn get_attestations(
    identity_ptr: *const u8,
    skill_name_ptr: *const u8,
    skill_name_len: u32,
) -> u32 {
    let identity = unsafe { core::slice::from_raw_parts(identity_ptr, 32) };
    let skill_name_len = skill_name_len as usize;

    if skill_name_len == 0 || skill_name_len > MAX_SKILL_LEN {
        log_info("❌ Invalid skill name length");
        return 1;
    }

    let skill_name = unsafe { core::slice::from_raw_parts(skill_name_ptr, skill_name_len) };
    let s_hash = skill_name_hash(skill_name);
    let ck = attestation_count_key(identity, &s_hash);

    let count = storage_get(&ck)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);

    moltchain_sdk::set_return_data(&u64_to_bytes(count));
    0
}

// ============================================================================
// REVOKE ATTESTATION
// ============================================================================

/// Revoke your attestation of someone's skill.
///
/// Parameters:
///   - attester_ptr: 32-byte attester address (the one revoking)
///   - identity_ptr: 32-byte identity address
///   - skill_name_ptr: pointer to skill name bytes
///   - skill_name_len: length of skill name
///
/// Returns 0 on success, nonzero on error.
#[no_mangle]
pub extern "C" fn revoke_attestation(
    attester_ptr: *const u8,
    identity_ptr: *const u8,
    skill_name_ptr: *const u8,
    skill_name_len: u32,
) -> u32 {
    log_info("🔄 Revoking attestation...");

    let attester = unsafe { core::slice::from_raw_parts(attester_ptr, 32) };
    let identity = unsafe { core::slice::from_raw_parts(identity_ptr, 32) };
    let skill_name_len = skill_name_len as usize;

    if skill_name_len == 0 || skill_name_len > MAX_SKILL_LEN {
        log_info("❌ Invalid skill name length");
        return 1;
    }

    let skill_name = unsafe { core::slice::from_raw_parts(skill_name_ptr, skill_name_len) };
    let s_hash = skill_name_hash(skill_name);
    let ak = attestation_key(identity, &s_hash, attester);

    if storage_get(&ak).is_none() {
        log_info("❌ No attestation found to revoke");
        return 2;
    }

    // Remove attestation
    moltchain_sdk::storage::remove(&ak);

    // Decrement count
    let ck = attestation_count_key(identity, &s_hash);
    let count = storage_get(&ck)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    if count > 0 {
        storage_set(&ck, &u64_to_bytes(count - 1));
    }

    log_info("✅ Attestation revoked");
    0
}

// ============================================================================
// REGISTER .MOLT NAME
// ============================================================================

/// Register a .molt domain name for the caller's identity.
/// The caller must have a MoltyID. Name must be valid (3-32 chars, alphanumeric + hyphens).
/// Registration requires payment (checked via get_value()).
///
/// Name record: [owner(32), registered_slot(8), expiry_slot(8)] = 48 bytes
///
/// Parameters:
///   - caller_ptr: 32-byte owner address (must have MoltyID)
///   - name_ptr: pointer to name bytes (lowercase, no .molt suffix)
///   - name_len: length of name
///   - duration_years: number of years to register (1-10)
#[no_mangle]
pub extern "C" fn register_name(
    caller_ptr: *const u8,
    name_ptr: *const u8,
    name_len: u32,
    duration_years: u8,
) -> u32 {
    log_info("🔤 Registering .molt name...");

    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let name_len = name_len as usize;
    let name = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };

    // Must have a MoltyID
    let id_key = identity_key(caller);
    if storage_get(&id_key).is_none() {
        log_info("❌ Must register MoltyID first");
        return 1;
    }

    // Validate name
    if !validate_molt_name(name) {
        log_info("❌ Invalid .molt name (3-32 chars, a-z 0-9 hyphens, no leading/trailing hyphens)");
        return 2;
    }

    // Check reserved
    if is_reserved_name(name) {
        log_info("❌ Name is reserved");
        return 3;
    }

    // Duration: 1-10 years
    if duration_years == 0 || duration_years > 10 {
        log_info("❌ Duration must be 1-10 years");
        return 4;
    }

    // Check if name is already taken and not expired
    let nk = name_key(name);
    if let Some(existing) = storage_get(&nk) {
        if existing.len() >= 48 {
            let expiry = bytes_to_u64(&existing[40..48]);
            let current_slot = moltchain_sdk::get_slot();
            if current_slot < expiry {
                log_info("❌ Name already registered and not expired");
                return 5;
            }
            // Name expired — can be re-registered (clear old reverse mapping)
            let old_owner = &existing[0..32];
            let old_rev = name_reverse_key(old_owner);
            moltchain_sdk::storage::remove(&old_rev);
        }
    }

    // Check caller doesn't already have a name (one name per identity)
    let rev_key = name_reverse_key(caller);
    if let Some(existing_name) = storage_get(&rev_key) {
        // Check if the existing name is still valid
        let existing_nk = name_key(&existing_name);
        if let Some(nr) = storage_get(&existing_nk) {
            if nr.len() >= 48 {
                let expiry = bytes_to_u64(&nr[40..48]);
                if moltchain_sdk::get_slot() < expiry {
                    log_info("❌ Already have a .molt name; release it first");
                    return 6;
                }
            }
        }
    }

    // Check payment (via get_value() — the MOLT tokens sent with this transaction)
    let required_cost = name_registration_cost(name_len) * (duration_years as u64);
    let paid = moltchain_sdk::get_value();
    if paid < required_cost {
        log_info("❌ Insufficient payment for name registration");
        return 7;
    }

    // Register the name
    let current_slot = moltchain_sdk::get_slot();
    let expiry_slot = current_slot + (SLOTS_PER_YEAR * duration_years as u64);

    let mut record = [0u8; 48];
    record[0..32].copy_from_slice(caller);
    record[32..40].copy_from_slice(&u64_to_bytes(current_slot));
    record[40..48].copy_from_slice(&u64_to_bytes(expiry_slot));

    storage_set(&nk, &record);

    // Set reverse mapping: address → name
    storage_set(&rev_key, name);

    // Increment name count
    let count = storage_get(b"molt_name_count")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    storage_set(b"molt_name_count", &u64_to_bytes(count + 1));

    log_info("✅ .molt name registered!");
    0
}

// ============================================================================
// RESOLVE .MOLT NAME
// ============================================================================

/// Resolve a .molt name to its owner address and expiry.
/// Returns owner(32) + registered_slot(8) + expiry_slot(8) as return data.
///
/// Parameters:
///   - name_ptr: pointer to name bytes
///   - name_len: length of name
#[no_mangle]
pub extern "C" fn resolve_name(name_ptr: *const u8, name_len: u32) -> u32 {
    let name_len = name_len as usize;
    let name = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };

    let nk = name_key(name);
    match storage_get(&nk) {
        Some(data) if data.len() >= 48 => {
            let expiry = bytes_to_u64(&data[40..48]);
            let current_slot = moltchain_sdk::get_slot();
            if current_slot >= expiry {
                log_info("❌ Name expired");
                return 1;
            }
            moltchain_sdk::set_return_data(&data);
            0
        }
        _ => {
            log_info("❌ Name not found");
            1
        }
    }
}

// ============================================================================
// REVERSE RESOLVE (address → .molt name)
// ============================================================================

/// Reverse resolve: given an address, return its .molt name.
/// Returns the name bytes as return data.
///
/// Parameters:
///   - addr_ptr: 32-byte address
#[no_mangle]
pub extern "C" fn reverse_resolve(addr_ptr: *const u8) -> u32 {
    let addr = unsafe { core::slice::from_raw_parts(addr_ptr, 32) };

    let rev_key = name_reverse_key(addr);
    match storage_get(&rev_key) {
        Some(name_bytes) => {
            // Verify the name is still valid
            let nk = name_key(&name_bytes);
            if let Some(record) = storage_get(&nk) {
                if record.len() >= 48 {
                    let expiry = bytes_to_u64(&record[40..48]);
                    if moltchain_sdk::get_slot() < expiry {
                        moltchain_sdk::set_return_data(&name_bytes);
                        return 0;
                    }
                }
            }
            log_info("❌ Name expired or invalid");
            1
        }
        None => {
            log_info("❌ No .molt name for this address");
            1
        }
    }
}

// ============================================================================
// TRANSFER .MOLT NAME
// ============================================================================

/// Transfer a .molt name to another address.
/// The new owner must have a MoltyID and must not already own a name.
///
/// Parameters:
///   - caller_ptr: 32-byte caller (current owner)
///   - name_ptr: pointer to name bytes
///   - name_len: length of name
///   - new_owner_ptr: 32-byte new owner address
#[no_mangle]
pub extern "C" fn transfer_name(
    caller_ptr: *const u8,
    name_ptr: *const u8,
    name_len: u32,
    new_owner_ptr: *const u8,
) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let name_len = name_len as usize;
    let name = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };
    let new_owner = unsafe { core::slice::from_raw_parts(new_owner_ptr, 32) };

    // Look up the name record
    let nk = name_key(name);
    let mut record = match storage_get(&nk) {
        Some(data) if data.len() >= 48 => data,
        _ => {
            log_info("❌ Name not found");
            return 1;
        }
    };

    // Verify caller is current owner
    if &record[0..32] != caller {
        log_info("❌ Not the owner of this name");
        return 2;
    }

    // Check name is not expired
    let expiry = bytes_to_u64(&record[40..48]);
    if moltchain_sdk::get_slot() >= expiry {
        log_info("❌ Name has expired");
        return 3;
    }

    // New owner must have a MoltyID
    let new_owner_id = identity_key(new_owner);
    if storage_get(&new_owner_id).is_none() {
        log_info("❌ New owner must have a MoltyID");
        return 4;
    }

    // New owner must not already have a name
    let new_rev = name_reverse_key(new_owner);
    if let Some(existing_name) = storage_get(&new_rev) {
        let existing_nk = name_key(&existing_name);
        if let Some(nr) = storage_get(&existing_nk) {
            if nr.len() >= 48 {
                let ex = bytes_to_u64(&nr[40..48]);
                if moltchain_sdk::get_slot() < ex {
                    log_info("❌ New owner already has a .molt name");
                    return 5;
                }
            }
        }
    }

    // Update name record with new owner
    record[0..32].copy_from_slice(new_owner);
    storage_set(&nk, &record);

    // Update reverse mappings
    let old_rev = name_reverse_key(caller);
    moltchain_sdk::storage::remove(&old_rev);
    storage_set(&new_rev, name);

    log_info("✅ .molt name transferred");
    0
}

// ============================================================================
// RENEW .MOLT NAME
// ============================================================================

/// Extend registration of a .molt name. Requires payment.
///
/// Parameters:
///   - caller_ptr: 32-byte caller (must be owner)
///   - name_ptr: pointer to name bytes
///   - name_len: length of name
///   - additional_years: years to add (1-10)
#[no_mangle]
pub extern "C" fn renew_name(
    caller_ptr: *const u8,
    name_ptr: *const u8,
    name_len: u32,
    additional_years: u8,
) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let name_len = name_len as usize;
    let name = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };

    if additional_years == 0 || additional_years > 10 {
        log_info("❌ Additional years must be 1-10");
        return 1;
    }

    let nk = name_key(name);
    let mut record = match storage_get(&nk) {
        Some(data) if data.len() >= 48 => data,
        _ => {
            log_info("❌ Name not found");
            return 2;
        }
    };

    // Must be owner
    if &record[0..32] != caller {
        log_info("❌ Not the owner of this name");
        return 3;
    }

    // Check payment
    let required_cost = name_registration_cost(name_len) * (additional_years as u64);
    let paid = moltchain_sdk::get_value();
    if paid < required_cost {
        log_info("❌ Insufficient payment for renewal");
        return 4;
    }

    // Extend expiry from current expiry (or from now if expired)
    let current_expiry = bytes_to_u64(&record[40..48]);
    let current_slot = moltchain_sdk::get_slot();
    let base = if current_slot > current_expiry { current_slot } else { current_expiry };
    let new_expiry = base + (SLOTS_PER_YEAR * additional_years as u64);

    record[40..48].copy_from_slice(&u64_to_bytes(new_expiry));
    storage_set(&nk, &record);

    log_info("✅ .molt name renewed");
    0
}

// ============================================================================
// RELEASE .MOLT NAME
// ============================================================================

/// Voluntarily release a .molt name.
///
/// Parameters:
///   - caller_ptr: 32-byte caller (must be owner)
///   - name_ptr: pointer to name bytes
///   - name_len: length of name
#[no_mangle]
pub extern "C" fn release_name(
    caller_ptr: *const u8,
    name_ptr: *const u8,
    name_len: u32,
) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let name_len = name_len as usize;
    let name = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };

    let nk = name_key(name);
    let record = match storage_get(&nk) {
        Some(data) if data.len() >= 48 => data,
        _ => {
            log_info("❌ Name not found");
            return 1;
        }
    };

    // Must be owner
    if &record[0..32] != caller {
        log_info("❌ Not the owner of this name");
        return 2;
    }

    // Remove forward mapping
    moltchain_sdk::storage::remove(&nk);

    // Remove reverse mapping
    let rev_key = name_reverse_key(caller);
    moltchain_sdk::storage::remove(&rev_key);

    // Decrement name count
    let count = storage_get(b"molt_name_count")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    if count > 0 {
        storage_set(b"molt_name_count", &u64_to_bytes(count - 1));
    }

    log_info("✅ .molt name released");
    0
}

// ============================================================================
// AGENT DISCOVERY REGISTRY
// ============================================================================

/// Set the agent's API endpoint URL.
/// Only the identity owner can set their endpoint.
///
/// Parameters:
///   - caller_ptr: 32-byte owner address
///   - url_ptr: pointer to URL bytes
///   - url_len: length of URL
#[no_mangle]
pub extern "C" fn set_endpoint(caller_ptr: *const u8, url_ptr: *const u8, url_len: u32) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let url_len = url_len as usize;

    if url_len == 0 || url_len > MAX_ENDPOINT_LEN {
        log_info("❌ Invalid endpoint URL length");
        return 1;
    }

    let url = unsafe { core::slice::from_raw_parts(url_ptr, url_len) };

    // Must have identity
    let idk = identity_key(caller);
    if storage_get(&idk).is_none() {
        log_info("❌ Identity not found — register first");
        return 2;
    }

    let ek = endpoint_key(caller);
    storage_set(&ek, url);

    log_info("✅ Endpoint set");
    0
}

/// Get the endpoint URL for an address.
///
/// Parameters:
///   - addr_ptr: 32-byte address
#[no_mangle]
pub extern "C" fn get_endpoint(addr_ptr: *const u8) -> u32 {
    let addr = unsafe { core::slice::from_raw_parts(addr_ptr, 32) };

    let ek = endpoint_key(addr);
    match storage_get(&ek) {
        Some(data) => {
            moltchain_sdk::set_return_data(&data);
            0
        }
        None => {
            log_info("❌ No endpoint set");
            1
        }
    }
}

/// Set agent metadata (up to 1KB JSON).
///
/// Parameters:
///   - caller_ptr: 32-byte owner address
///   - json_ptr: pointer to JSON metadata bytes
///   - json_len: length of metadata
#[no_mangle]
pub extern "C" fn set_metadata(caller_ptr: *const u8, json_ptr: *const u8, json_len: u32) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    let json_len = json_len as usize;

    if json_len == 0 || json_len > MAX_METADATA_LEN {
        log_info("❌ Invalid metadata length");
        return 1;
    }

    let json = unsafe { core::slice::from_raw_parts(json_ptr, json_len) };

    // Must have identity
    let idk = identity_key(caller);
    if storage_get(&idk).is_none() {
        log_info("❌ Identity not found — register first");
        return 2;
    }

    let mk = metadata_key(caller);
    storage_set(&mk, json);

    log_info("✅ Metadata set");
    0
}

/// Get metadata for an address.
///
/// Parameters:
///   - addr_ptr: 32-byte address
#[no_mangle]
pub extern "C" fn get_metadata(addr_ptr: *const u8) -> u32 {
    let addr = unsafe { core::slice::from_raw_parts(addr_ptr, 32) };

    let mk = metadata_key(addr);
    match storage_get(&mk) {
        Some(data) => {
            moltchain_sdk::set_return_data(&data);
            0
        }
        None => {
            log_info("❌ No metadata set");
            1
        }
    }
}

/// Set availability status: 0=offline, 1=available, 2=busy.
///
/// Parameters:
///   - caller_ptr: 32-byte owner address
///   - status: availability status (0-2)
#[no_mangle]
pub extern "C" fn set_availability(caller_ptr: *const u8, status: u8) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };

    if status > 2 {
        log_info("❌ Invalid availability status (0=offline, 1=available, 2=busy)");
        return 1;
    }

    // Must have identity
    let idk = identity_key(caller);
    if storage_get(&idk).is_none() {
        log_info("❌ Identity not found — register first");
        return 2;
    }

    let ak = availability_key(caller);
    storage_set(&ak, &[status]);

    log_info("✅ Availability set");
    0
}

/// Get availability status for an address.
///
/// Parameters:
///   - addr_ptr: 32-byte address
#[no_mangle]
pub extern "C" fn get_availability(addr_ptr: *const u8) -> u32 {
    let addr = unsafe { core::slice::from_raw_parts(addr_ptr, 32) };

    let ak = availability_key(addr);
    match storage_get(&ak) {
        Some(data) if !data.is_empty() => {
            moltchain_sdk::set_return_data(&data);
            0
        }
        _ => {
            // Default: offline (0)
            moltchain_sdk::set_return_data(&[0]);
            0
        }
    }
}

/// Set service rate (MOLT per unit).
///
/// Parameters:
///   - caller_ptr: 32-byte owner address
///   - molt_per_unit: rate in MOLT
#[no_mangle]
pub extern "C" fn set_rate(caller_ptr: *const u8, molt_per_unit: u64) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };

    // Must have identity
    let idk = identity_key(caller);
    if storage_get(&idk).is_none() {
        log_info("❌ Identity not found — register first");
        return 1;
    }

    let rk = rate_key(caller);
    storage_set(&rk, &u64_to_bytes(molt_per_unit));

    log_info("✅ Rate set");
    0
}

/// Get rate for an address.
///
/// Parameters:
///   - addr_ptr: 32-byte address
#[no_mangle]
pub extern "C" fn get_rate(addr_ptr: *const u8) -> u32 {
    let addr = unsafe { core::slice::from_raw_parts(addr_ptr, 32) };

    let rk = rate_key(addr);
    match storage_get(&rk) {
        Some(data) if data.len() >= 8 => {
            moltchain_sdk::set_return_data(&data);
            0
        }
        _ => {
            moltchain_sdk::set_return_data(&u64_to_bytes(0));
            0
        }
    }
}

// ============================================================================
// FULL AGENT PROFILE
// ============================================================================

/// Assemble a full agent profile as return data.
/// Format: [identity_record(127)] + [has_name(1)] + [name_len(1)] + [name(0-32)]
///       + [has_endpoint(1)] + [endpoint_len(2 LE)] + [endpoint(0-256)]
///       + [availability(1)] + [rate(8)] + [reputation(8)]
///
/// Parameters:
///   - addr_ptr: 32-byte address
#[no_mangle]
pub extern "C" fn get_agent_profile(addr_ptr: *const u8) -> u32 {
    let addr = unsafe { core::slice::from_raw_parts(addr_ptr, 32) };

    // Must have identity
    let idk = identity_key(addr);
    let id_record = match storage_get(&idk) {
        Some(data) => data,
        None => {
            log_info("❌ Identity not found");
            return 1;
        }
    };

    let mut result = Vec::with_capacity(512);

    // Identity record (pad/truncate to IDENTITY_SIZE)
    if id_record.len() >= IDENTITY_SIZE {
        result.extend_from_slice(&id_record[..IDENTITY_SIZE]);
    } else {
        result.extend_from_slice(&id_record);
        result.resize(IDENTITY_SIZE, 0);
    }

    // .molt name
    let rev_key = name_reverse_key(addr);
    match storage_get(&rev_key) {
        Some(name_bytes) => {
            // Verify not expired
            let nk = name_key(&name_bytes);
            let valid = if let Some(nr) = storage_get(&nk) {
                if nr.len() >= 48 {
                    moltchain_sdk::get_slot() < bytes_to_u64(&nr[40..48])
                } else {
                    false
                }
            } else {
                false
            };
            if valid {
                result.push(1); // has_name
                result.push(name_bytes.len() as u8); // name_len
                result.extend_from_slice(&name_bytes);
            } else {
                result.push(0); // no valid name
                result.push(0);
            }
        }
        None => {
            result.push(0); // no name
            result.push(0);
        }
    }

    // Endpoint
    let ek = endpoint_key(addr);
    match storage_get(&ek) {
        Some(ep_data) => {
            result.push(1); // has_endpoint
            let ep_len = ep_data.len() as u16;
            result.push((ep_len & 0xFF) as u8);
            result.push(((ep_len >> 8) & 0xFF) as u8);
            result.extend_from_slice(&ep_data);
        }
        None => {
            result.push(0); // no endpoint
            result.push(0);
            result.push(0);
        }
    }

    // Availability
    let ak = availability_key(addr);
    let avail = storage_get(&ak)
        .and_then(|d| if !d.is_empty() { Some(d[0]) } else { None })
        .unwrap_or(0);
    result.push(avail);

    // Rate
    let rk = rate_key(addr);
    let rate = storage_get(&rk)
        .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
        .unwrap_or(0);
    result.extend_from_slice(&u64_to_bytes(rate));

    // Reputation
    let rep_k = reputation_key(addr);
    let rep = storage_get(&rep_k)
        .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
        .unwrap_or(0);
    result.extend_from_slice(&u64_to_bytes(rep));

    moltchain_sdk::set_return_data(&result);
    0
}

// ============================================================================
// TRUST TIER
// ============================================================================

/// Calculate trust tier from reputation score.
/// Tier 0: 0-99 (Newcomer)
/// Tier 1: 100-499 (Verified)
/// Tier 2: 500-999 (Trusted)
/// Tier 3: 1000-4999 (Established)
/// Tier 4: 5000-9999 (Elite)
/// Tier 5: 10000+ (Legendary)
#[no_mangle]
pub extern "C" fn get_trust_tier(pubkey_ptr: *const u8) -> u32 {
    let pubkey = unsafe { core::slice::from_raw_parts(pubkey_ptr, 32) };
    let rep_key = reputation_key(pubkey);

    let reputation = match storage_get(&rep_key) {
        Some(data) if data.len() >= 8 => bytes_to_u64(&data),
        _ => 0,
    };

    let tier: u8 = if reputation >= 10_000 {
        5
    } else if reputation >= 5_000 {
        4
    } else if reputation >= 1_000 {
        3
    } else if reputation >= 500 {
        2
    } else if reputation >= 100 {
        1
    } else {
        0
    };

    // Return tier as first byte of return data, plus reputation
    let mut result = Vec::with_capacity(9);
    result.push(tier);
    result.extend_from_slice(&u64_to_bytes(reputation));
    moltchain_sdk::set_return_data(&result);
    0
}

// ============================================================================
// HARDENING: PAUSE, ADMIN ROTATION
// ============================================================================

/// Pause MoltyID. Admin only.
/// Blocks: register_identity, vouch
/// Returns: 0 success, 1 not admin, 2 already paused
#[no_mangle]
pub extern "C" fn mid_pause(caller_ptr: *const u8) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    if !is_mid_admin(caller) { return 1; }
    if is_mid_paused() { return 2; }
    storage_set(MID_PAUSE_KEY, &[1]);
    log_info("⏸️ MoltyID paused");
    0
}

/// Unpause MoltyID. Admin only.
/// Returns: 0 success, 1 not admin, 2 not paused
#[no_mangle]
pub extern "C" fn mid_unpause(caller_ptr: *const u8) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    if !is_mid_admin(caller) { return 1; }
    if !is_mid_paused() { return 2; }
    storage_set(MID_PAUSE_KEY, &[0]);
    log_info("▶️ MoltyID unpaused");
    0
}

/// Transfer admin key. Current admin only.
/// Returns: 0 success, 1 not admin
#[no_mangle]
pub extern "C" fn transfer_admin(caller_ptr: *const u8, new_admin_ptr: *const u8) -> u32 {
    let caller = unsafe { core::slice::from_raw_parts(caller_ptr, 32) };
    if !is_mid_admin(caller) { return 1; }
    let new_admin = unsafe { core::slice::from_raw_parts(new_admin_ptr, 32) };
    storage_set(b"mid_admin", new_admin);
    log_info("✅ Admin key transferred");
    0
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use moltchain_sdk::test_mock;
    use moltchain_sdk::bytes_to_u64;

    fn setup() {
        test_mock::reset();
    }

    #[test]
    fn test_initialize() {
        setup();
        let admin = [1u8; 32];
        let result = initialize(admin.as_ptr());
        assert_eq!(result, 0); // success

        assert_eq!(test_mock::get_storage(b"mid_admin"), Some(admin.to_vec()));
        assert_eq!(test_mock::get_storage(b"mid_initialized"), Some([1u8].to_vec()));
    }

    #[test]
    fn test_double_initialize_fails() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        let result = initialize(admin.as_ptr());
        assert_eq!(result, 1); // already initialized
    }

    #[test]
    fn test_register_identity() {
        setup();
        test_mock::set_timestamp(5000);

        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        let owner = [2u8; 32];
        let name = b"TradingBot";
        let result = register_identity(
            owner.as_ptr(),
            AGENT_TYPE_TRADING,
            name.as_ptr(),
            name.len() as u32,
        );
        assert_eq!(result, 0); // success

        // Check identity count
        let count_bytes = test_mock::get_storage(b"mid_identity_count").unwrap();
        assert_eq!(bytes_to_u64(&count_bytes), 1);

        // Check identity is stored (via key lookup)
        let id_key = identity_key(&owner);
        let record = test_mock::get_storage(&id_key).unwrap();
        assert!(record.len() >= IDENTITY_SIZE);
        assert_eq!(&record[0..32], &owner);
        assert_eq!(record[32], AGENT_TYPE_TRADING);

        // Check initial reputation
        let rep = bytes_to_u64(&record[99..107]);
        assert_eq!(rep, INITIAL_REPUTATION);

        // Check is_active = 1
        assert_eq!(record[126], 1);
    }

    #[test]
    fn test_register_duplicate_fails() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        let owner = [2u8; 32];
        let name = b"Agent";
        register_identity(owner.as_ptr(), AGENT_TYPE_GENERAL, name.as_ptr(), name.len() as u32);

        let result = register_identity(owner.as_ptr(), AGENT_TYPE_GENERAL, name.as_ptr(), name.len() as u32);
        assert_eq!(result, 3); // already registered
    }

    #[test]
    fn test_add_skill() {
        setup();
        test_mock::set_timestamp(5000);

        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        let owner = [2u8; 32];
        let name = b"SkillBot";
        register_identity(owner.as_ptr(), AGENT_TYPE_DEVELOPMENT, name.as_ptr(), name.len() as u32);

        let skill_name = b"Rust";
        let result = add_skill(
            owner.as_ptr(),
            skill_name.as_ptr(),
            skill_name.len() as u32,
            80, // proficiency
        );
        assert_eq!(result, 0); // success

        // Check skill count incremented in identity record
        let id_key = identity_key(&owner);
        let record = test_mock::get_storage(&id_key).unwrap();
        assert_eq!(record[123], 1); // 1 skill

        // Add another skill
        let skill2 = b"Solidity";
        let result2 = add_skill(
            owner.as_ptr(),
            skill2.as_ptr(),
            skill2.len() as u32,
            60,
        );
        assert_eq!(result2, 0);

        let record2 = test_mock::get_storage(&id_key).unwrap();
        assert_eq!(record2[123], 2); // 2 skills
    }

    #[test]
    fn test_add_skill_unregistered_fails() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        let nobody = [9u8; 32];
        let skill_name = b"Hacking";
        let result = add_skill(nobody.as_ptr(), skill_name.as_ptr(), skill_name.len() as u32, 50);
        assert_eq!(result, 3); // identity not found
    }

    #[test]
    fn test_vouch() {
        setup();
        test_mock::set_timestamp(5000);

        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        let agent_a = [2u8; 32];
        let agent_b = [3u8; 32];
        let name_a = b"AgentA";
        let name_b = b"AgentB";

        register_identity(agent_a.as_ptr(), AGENT_TYPE_GENERAL, name_a.as_ptr(), name_a.len() as u32);
        register_identity(agent_b.as_ptr(), AGENT_TYPE_GENERAL, name_b.as_ptr(), name_b.len() as u32);

        let result = vouch(agent_a.as_ptr(), agent_b.as_ptr());
        assert_eq!(result, 0); // success

        // Check voucher reputation decreased
        let voucher_id_key = identity_key(&agent_a);
        let voucher_record = test_mock::get_storage(&voucher_id_key).unwrap();
        let voucher_rep = bytes_to_u64(&voucher_record[99..107]);
        assert_eq!(voucher_rep, INITIAL_REPUTATION - VOUCH_COST);

        // Check vouchee reputation increased
        let vouchee_id_key = identity_key(&agent_b);
        let vouchee_record = test_mock::get_storage(&vouchee_id_key).unwrap();
        let vouchee_rep = bytes_to_u64(&vouchee_record[99..107]);
        assert_eq!(vouchee_rep, INITIAL_REPUTATION + VOUCH_REWARD);
    }

    #[test]
    fn test_vouch_self_fails() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        let agent = [2u8; 32];
        let name = b"SelfVoucher";
        register_identity(agent.as_ptr(), AGENT_TYPE_GENERAL, name.as_ptr(), name.len() as u32);

        let result = vouch(agent.as_ptr(), agent.as_ptr());
        assert_eq!(result, 1); // cannot vouch for yourself
    }

    #[test]
    fn test_double_vouch_fails() {
        setup();
        test_mock::set_timestamp(5000);

        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        let agent_a = [2u8; 32];
        let agent_b = [3u8; 32];
        let name_a = b"A";
        let name_b = b"B";

        register_identity(agent_a.as_ptr(), AGENT_TYPE_GENERAL, name_a.as_ptr(), name_a.len() as u32);
        register_identity(agent_b.as_ptr(), AGENT_TYPE_GENERAL, name_b.as_ptr(), name_b.len() as u32);

        vouch(agent_a.as_ptr(), agent_b.as_ptr());
        let result = vouch(agent_a.as_ptr(), agent_b.as_ptr());
        assert_eq!(result, 6); // already vouched
    }

    #[test]
    fn test_update_reputation() {
        setup();
        test_mock::set_timestamp(5000);

        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        let agent = [2u8; 32];
        let name = b"RepBot";
        register_identity(agent.as_ptr(), AGENT_TYPE_GENERAL, name.as_ptr(), name.len() as u32);

        // Admin increases reputation
        let result = update_reputation(admin.as_ptr(), agent.as_ptr(), 50, 1);
        assert_eq!(result, 0);

        let rep_key = reputation_key(&agent);
        let rep_bytes = test_mock::get_storage(&rep_key).unwrap();
        assert_eq!(bytes_to_u64(&rep_bytes), INITIAL_REPUTATION + 50);
    }

    #[test]
    fn test_update_reputation_unauthorized() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        let agent = [2u8; 32];
        let name = b"Bot";
        register_identity(agent.as_ptr(), AGENT_TYPE_GENERAL, name.as_ptr(), name.len() as u32);

        let non_admin = [9u8; 32];
        let result = update_reputation(non_admin.as_ptr(), agent.as_ptr(), 50, 1);
        assert_eq!(result, 2); // unauthorized
    }

    #[test]
    fn test_deactivate_identity() {
        setup();
        test_mock::set_timestamp(5000);

        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        let agent = [2u8; 32];
        let name = b"DeactivateMe";
        register_identity(agent.as_ptr(), AGENT_TYPE_GENERAL, name.as_ptr(), name.len() as u32);

        // Owner deactivates
        let result = deactivate_identity(agent.as_ptr(), agent.as_ptr());
        assert_eq!(result, 0);

        let id_key = identity_key(&agent);
        let record = test_mock::get_storage(&id_key).unwrap();
        assert_eq!(record[126], 0); // is_active = 0
    }

    #[test]
    fn test_get_reputation() {
        setup();
        test_mock::set_timestamp(5000);

        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        let agent = [2u8; 32];
        let name = b"RepCheck";
        register_identity(agent.as_ptr(), AGENT_TYPE_GENERAL, name.as_ptr(), name.len() as u32);

        let result = get_reputation(agent.as_ptr());
        assert_eq!(result, 0); // success

        // Check return data contains the reputation
        let ret = test_mock::get_return_data();
        assert!(ret.len() >= 8);
        assert_eq!(bytes_to_u64(&ret), INITIAL_REPUTATION);
    }

    #[test]
    fn test_attest_skill() {
        setup();
        test_mock::set_timestamp(5000);

        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        let agent_a = [2u8; 32];
        let agent_b = [3u8; 32];
        let name_a = b"AgentA";
        let name_b = b"AgentB";

        register_identity(agent_a.as_ptr(), AGENT_TYPE_GENERAL, name_a.as_ptr(), name_a.len() as u32);
        register_identity(agent_b.as_ptr(), AGENT_TYPE_GENERAL, name_b.as_ptr(), name_b.len() as u32);

        // Agent B attests Agent A's "Rust" skill at level 4
        let skill = b"Rust";
        let result = attest_skill(
            agent_b.as_ptr(),
            agent_a.as_ptr(),
            skill.as_ptr(),
            skill.len() as u32,
            4,
        );
        assert_eq!(result, 0);

        // Check attestation count
        let result = get_attestations(agent_a.as_ptr(), skill.as_ptr(), skill.len() as u32);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 1);

        // Duplicate attestation should fail
        let result = attest_skill(
            agent_b.as_ptr(), agent_a.as_ptr(), skill.as_ptr(), skill.len() as u32, 3,
        );
        assert_eq!(result, 6); // already attested

        // Self-attestation should fail
        let result = attest_skill(
            agent_a.as_ptr(), agent_a.as_ptr(), skill.as_ptr(), skill.len() as u32, 5,
        );
        assert_eq!(result, 3); // cannot attest own skills
    }

    #[test]
    fn test_revoke_attestation() {
        setup();
        test_mock::set_timestamp(5000);

        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        let agent_a = [2u8; 32];
        let agent_b = [3u8; 32];
        let name_a = b"AgentA";
        let name_b = b"AgentB";

        register_identity(agent_a.as_ptr(), AGENT_TYPE_GENERAL, name_a.as_ptr(), name_a.len() as u32);
        register_identity(agent_b.as_ptr(), AGENT_TYPE_GENERAL, name_b.as_ptr(), name_b.len() as u32);

        let skill = b"Solidity";
        attest_skill(agent_b.as_ptr(), agent_a.as_ptr(), skill.as_ptr(), skill.len() as u32, 3);

        // Check count is 1
        get_attestations(agent_a.as_ptr(), skill.as_ptr(), skill.len() as u32);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 1);

        // Revoke
        let result = revoke_attestation(
            agent_b.as_ptr(), agent_a.as_ptr(), skill.as_ptr(), skill.len() as u32,
        );
        assert_eq!(result, 0);

        // Count should be 0
        get_attestations(agent_a.as_ptr(), skill.as_ptr(), skill.len() as u32);
        let ret = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&ret), 0);

        // Double revoke should fail
        let result = revoke_attestation(
            agent_b.as_ptr(), agent_a.as_ptr(), skill.as_ptr(), skill.len() as u32,
        );
        assert_eq!(result, 2); // no attestation found
    }

    // ====================================================================
    // .MOLT NAMING SYSTEM TESTS
    // ====================================================================

    fn setup_identity_with_slot(owner: &[u8; 32], slot: u64, value: u64) {
        let admin = [1u8; 32];
        test_mock::set_timestamp(5000);
        test_mock::set_slot(slot);
        test_mock::set_value(value);
        initialize(admin.as_ptr());
        let name = b"TestAgent";
        register_identity(
            owner.as_ptr(),
            AGENT_TYPE_GENERAL,
            name.as_ptr(),
            name.len() as u32,
        );
    }

    #[test]
    fn test_register_name() {
        setup();
        let owner = [2u8; 32];
        setup_identity_with_slot(&owner, 1000, 100_000_000);

        let name = b"myagent";
        let result = register_name(
            owner.as_ptr(),
            name.as_ptr(),
            name.len() as u32,
            1,
        );
        assert_eq!(result, 0);

        // Verify forward mapping
        let nk = name_key(name);
        let record = test_mock::get_storage(&nk).unwrap();
        assert_eq!(record.len(), 48);
        assert_eq!(&record[0..32], &owner);
        let reg_slot = bytes_to_u64(&record[32..40]);
        assert_eq!(reg_slot, 1000);
        let expiry = bytes_to_u64(&record[40..48]);
        assert_eq!(expiry, 1000 + SLOTS_PER_YEAR);

        // Verify reverse mapping
        let rev = name_reverse_key(&owner);
        let stored_name = test_mock::get_storage(&rev).unwrap();
        assert_eq!(stored_name, name);

        // Verify count
        let count_data = test_mock::get_storage(b"molt_name_count").unwrap();
        assert_eq!(bytes_to_u64(&count_data), 1);
    }

    #[test]
    fn test_register_name_invalid() {
        setup();
        let owner = [2u8; 32];
        setup_identity_with_slot(&owner, 1000, 100_000_000);

        // Too short (2 chars)
        let short = b"ab";
        assert_eq!(register_name(owner.as_ptr(), short.as_ptr(), short.len() as u32, 1), 2);

        // Has uppercase
        let upper = b"MyAgent";
        assert_eq!(register_name(owner.as_ptr(), upper.as_ptr(), upper.len() as u32, 1), 2);

        // Leading hyphen
        let leading = b"-agent";
        assert_eq!(register_name(owner.as_ptr(), leading.as_ptr(), leading.len() as u32, 1), 2);

        // Trailing hyphen
        let trailing = b"agent-";
        assert_eq!(register_name(owner.as_ptr(), trailing.as_ptr(), trailing.len() as u32, 1), 2);

        // Consecutive hyphens
        let consec = b"my--agent";
        assert_eq!(register_name(owner.as_ptr(), consec.as_ptr(), consec.len() as u32, 1), 2);

        // Has special chars
        let special = b"my_agent";
        assert_eq!(register_name(owner.as_ptr(), special.as_ptr(), special.len() as u32, 1), 2);
    }

    #[test]
    fn test_register_name_reserved() {
        setup();
        let owner = [2u8; 32];
        setup_identity_with_slot(&owner, 1000, 1_000_000_000);

        let reserved = b"admin";
        assert_eq!(register_name(owner.as_ptr(), reserved.as_ptr(), reserved.len() as u32, 1), 3);

        let reserved2 = b"system";
        assert_eq!(register_name(owner.as_ptr(), reserved2.as_ptr(), reserved2.len() as u32, 1), 3);

        let reserved3 = b"molt";
        assert_eq!(register_name(owner.as_ptr(), reserved3.as_ptr(), reserved3.len() as u32, 1), 3);
    }

    #[test]
    fn test_resolve_name() {
        setup();
        let owner = [2u8; 32];
        setup_identity_with_slot(&owner, 1000, 100_000_000);

        let name = b"resolver";
        register_name(owner.as_ptr(), name.as_ptr(), name.len() as u32, 1);

        let result = resolve_name(name.as_ptr(), name.len() as u32);
        assert_eq!(result, 0);

        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 48);
        assert_eq!(&ret[0..32], &owner);
    }

    #[test]
    fn test_reverse_resolve() {
        setup();
        let owner = [2u8; 32];
        setup_identity_with_slot(&owner, 1000, 100_000_000);

        let name = b"reverse";
        register_name(owner.as_ptr(), name.as_ptr(), name.len() as u32, 1);

        let result = reverse_resolve(owner.as_ptr());
        assert_eq!(result, 0);

        let ret = test_mock::get_return_data();
        assert_eq!(ret.as_slice(), name);
    }

    #[test]
    fn test_transfer_name() {
        setup();
        let owner = [2u8; 32];
        setup_identity_with_slot(&owner, 1000, 100_000_000);

        let name = b"xfername";
        register_name(owner.as_ptr(), name.as_ptr(), name.len() as u32, 1);

        // Register new owner identity
        let new_owner = [3u8; 32];
        let new_name = b"NewOwner";
        register_identity(
            new_owner.as_ptr(),
            AGENT_TYPE_GENERAL,
            new_name.as_ptr(),
            new_name.len() as u32,
        );

        let result = transfer_name(
            owner.as_ptr(),
            name.as_ptr(),
            name.len() as u32,
            new_owner.as_ptr(),
        );
        assert_eq!(result, 0);

        // Verify new owner in forward mapping
        let nk = name_key(name);
        let record = test_mock::get_storage(&nk).unwrap();
        assert_eq!(&record[0..32], &new_owner);

        // Verify old reverse mapping removed
        let old_rev = name_reverse_key(&owner);
        assert!(test_mock::get_storage(&old_rev).is_none());

        // Verify new reverse mapping
        let new_rev = name_reverse_key(&new_owner);
        let stored = test_mock::get_storage(&new_rev).unwrap();
        assert_eq!(stored.as_slice(), name);
    }

    #[test]
    fn test_release_name() {
        setup();
        let owner = [2u8; 32];
        setup_identity_with_slot(&owner, 1000, 100_000_000);

        let name = b"releaseme";
        register_name(owner.as_ptr(), name.as_ptr(), name.len() as u32, 1);

        // Verify it exists
        assert_eq!(resolve_name(name.as_ptr(), name.len() as u32), 0);

        let result = release_name(owner.as_ptr(), name.as_ptr(), name.len() as u32);
        assert_eq!(result, 0);

        // Forward mapping removed
        let nk = name_key(name);
        assert!(test_mock::get_storage(&nk).is_none());

        // Reverse mapping removed
        let rev = name_reverse_key(&owner);
        assert!(test_mock::get_storage(&rev).is_none());

        // Count decremented
        let count_data = test_mock::get_storage(b"molt_name_count").unwrap();
        assert_eq!(bytes_to_u64(&count_data), 0);
    }

    #[test]
    fn test_renew_name() {
        setup();
        let owner = [2u8; 32];
        setup_identity_with_slot(&owner, 1000, 100_000_000);

        let name = b"renewable";
        register_name(owner.as_ptr(), name.as_ptr(), name.len() as u32, 1);

        // Get original expiry
        let nk = name_key(name);
        let record = test_mock::get_storage(&nk).unwrap();
        let original_expiry = bytes_to_u64(&record[40..48]);

        // Renew for 2 more years (need to set value again for payment)
        test_mock::set_value(200_000_000);
        let result = renew_name(owner.as_ptr(), name.as_ptr(), name.len() as u32, 2);
        assert_eq!(result, 0);

        // Verify extended expiry
        let record2 = test_mock::get_storage(&nk).unwrap();
        let new_expiry = bytes_to_u64(&record2[40..48]);
        assert_eq!(new_expiry, original_expiry + 2 * SLOTS_PER_YEAR);
    }

    #[test]
    fn test_name_one_per_identity() {
        setup();
        let owner = [2u8; 32];
        setup_identity_with_slot(&owner, 1000, 100_000_000);

        let name1 = b"first-name";
        assert_eq!(register_name(owner.as_ptr(), name1.as_ptr(), name1.len() as u32, 1), 0);

        // Try to register a second name — should fail with error 6
        let name2 = b"second-name";
        assert_eq!(register_name(owner.as_ptr(), name2.as_ptr(), name2.len() as u32, 1), 6);
    }

    // ====================================================================
    // AGENT DISCOVERY REGISTRY TESTS
    // ====================================================================

    #[test]
    fn test_set_endpoint() {
        setup();
        let owner = [2u8; 32];
        setup_identity_with_slot(&owner, 1000, 0);

        let url = b"https://api.myagent.molt/v1";
        let result = set_endpoint(owner.as_ptr(), url.as_ptr(), url.len() as u32);
        assert_eq!(result, 0);

        // Get it back
        let result = get_endpoint(owner.as_ptr());
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.as_slice(), url);
    }

    #[test]
    fn test_set_metadata() {
        setup();
        let owner = [2u8; 32];
        setup_identity_with_slot(&owner, 1000, 0);

        let json = b"{\"description\":\"A trading bot\",\"version\":\"1.0\"}";
        let result = set_metadata(owner.as_ptr(), json.as_ptr(), json.len() as u32);
        assert_eq!(result, 0);

        let result = get_metadata(owner.as_ptr());
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.as_slice(), json);
    }

    #[test]
    fn test_set_availability() {
        setup();
        let owner = [2u8; 32];
        setup_identity_with_slot(&owner, 1000, 0);

        // Set available
        assert_eq!(set_availability(owner.as_ptr(), 1), 0);
        get_availability(owner.as_ptr());
        let ret = test_mock::get_return_data();
        assert_eq!(ret, [1]);

        // Set busy
        assert_eq!(set_availability(owner.as_ptr(), 2), 0);
        get_availability(owner.as_ptr());
        let ret = test_mock::get_return_data();
        assert_eq!(ret, [2]);

        // Invalid status
        assert_eq!(set_availability(owner.as_ptr(), 3), 1);
    }

    #[test]
    fn test_get_agent_profile() {
        setup();
        let owner = [2u8; 32];
        setup_identity_with_slot(&owner, 1000, 100_000_000);

        // Register name
        let molt_name = b"profiled";
        register_name(owner.as_ptr(), molt_name.as_ptr(), molt_name.len() as u32, 1);

        // Set endpoint
        let url = b"https://profiled.molt/api";
        set_endpoint(owner.as_ptr(), url.as_ptr(), url.len() as u32);

        // Set availability
        set_availability(owner.as_ptr(), 1);

        // Set rate
        set_rate(owner.as_ptr(), 50_000);

        // Get full profile
        let result = get_agent_profile(owner.as_ptr());
        assert_eq!(result, 0);

        let ret = test_mock::get_return_data();
        // Minimum: 127 (identity) + 1 (has_name) + 1 (name_len) + name + 1 (has_ep) + 2 (ep_len) + ep + 1 (avail) + 8 (rate) + 8 (rep)
        assert!(ret.len() > IDENTITY_SIZE);

        // Identity record at beginning
        assert_eq!(&ret[0..32], &owner);

        // has_name = 1
        assert_eq!(ret[IDENTITY_SIZE], 1);
        let nm_len = ret[IDENTITY_SIZE + 1] as usize;
        assert_eq!(nm_len, molt_name.len());
        assert_eq!(&ret[IDENTITY_SIZE + 2..IDENTITY_SIZE + 2 + nm_len], molt_name);
    }

    #[test]
    fn test_get_trust_tier() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_timestamp(5000);
        test_mock::set_slot(1000);
        initialize(admin.as_ptr());

        let agent = [2u8; 32];
        let name = b"TierBot";
        register_identity(agent.as_ptr(), AGENT_TYPE_GENERAL, name.as_ptr(), name.len() as u32);

        // Initial reputation = 100 → Tier 1
        get_trust_tier(agent.as_ptr());
        let ret = test_mock::get_return_data();
        assert_eq!(ret[0], 1); // Verified tier

        // Boost to 500 → Tier 2
        update_reputation(admin.as_ptr(), agent.as_ptr(), 400, 1);
        get_trust_tier(agent.as_ptr());
        let ret = test_mock::get_return_data();
        assert_eq!(ret[0], 2); // Trusted

        // Boost to 1000 → Tier 3
        update_reputation(admin.as_ptr(), agent.as_ptr(), 500, 1);
        get_trust_tier(agent.as_ptr());
        let ret = test_mock::get_return_data();
        assert_eq!(ret[0], 3); // Established

        // Boost to 5000 → Tier 4
        update_reputation(admin.as_ptr(), agent.as_ptr(), 4000, 1);
        get_trust_tier(agent.as_ptr());
        let ret = test_mock::get_return_data();
        assert_eq!(ret[0], 4); // Elite

        // Boost to 10000 → Tier 5
        update_reputation(admin.as_ptr(), agent.as_ptr(), 5000, 1);
        get_trust_tier(agent.as_ptr());
        let ret = test_mock::get_return_data();
        assert_eq!(ret[0], 5); // Legendary
        let rep = bytes_to_u64(&ret[1..9]);
        assert_eq!(rep, 10000);

        // Unregistered agent → Tier 0
        let unknown = [99u8; 32];
        get_trust_tier(unknown.as_ptr());
        let ret = test_mock::get_return_data();
        assert_eq!(ret[0], 0); // Newcomer
    }

    // ====================================================================
    // HARDENING TESTS
    // ====================================================================

    #[test]
    fn test_pause_blocks_registration() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());

        // Pause
        assert_eq!(mid_pause(admin.as_ptr()), 0);

        // Registration blocked
        let agent = [2u8; 32];
        let name = b"test-agent";
        let result = register_identity(agent.as_ptr(), 1, name.as_ptr(), name.len() as u32);
        assert_eq!(result, 20);

        // Unpause
        assert_eq!(mid_unpause(admin.as_ptr()), 0);

        // Now works
        let result = register_identity(agent.as_ptr(), 1, name.as_ptr(), name.len() as u32);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_pause_admin_checks() {
        setup();
        let admin = [1u8; 32];
        let non_admin = [2u8; 32];
        initialize(admin.as_ptr());

        assert_eq!(mid_pause(non_admin.as_ptr()), 1); // not admin
        assert_eq!(mid_pause(admin.as_ptr()), 0);
        assert_eq!(mid_pause(admin.as_ptr()), 2); // already paused
        assert_eq!(mid_unpause(non_admin.as_ptr()), 1); // not admin
        assert_eq!(mid_unpause(admin.as_ptr()), 0);
        assert_eq!(mid_unpause(admin.as_ptr()), 2); // not paused
    }

    #[test]
    fn test_vouch_cooldown() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        test_mock::set_timestamp(1_000_000);

        let voucher = [2u8; 32];
        let vouchee1 = [3u8; 32];
        let vouchee2 = [4u8; 32];
        let name1 = b"voucher";
        let name2 = b"vouchee1";
        let name3 = b"vouchee2";
        register_identity(voucher.as_ptr(), 1, name1.as_ptr(), name1.len() as u32);
        register_identity(vouchee1.as_ptr(), 1, name2.as_ptr(), name2.len() as u32);
        register_identity(vouchee2.as_ptr(), 1, name3.as_ptr(), name3.len() as u32);

        // Boost voucher rep so they can vouch
        update_reputation(admin.as_ptr(), voucher.as_ptr(), 100, 1);

        // First vouch works
        let result = vouch(voucher.as_ptr(), vouchee1.as_ptr());
        assert_eq!(result, 0);

        // Second vouch too soon (within VOUCH_COOLDOWN_MS = 1 hour)
        test_mock::set_timestamp(1_000_000 + 1000); // 1 second later
        let result = vouch(voucher.as_ptr(), vouchee2.as_ptr());
        assert_eq!(result, 21); // cooldown

        // After cooldown passes
        test_mock::set_timestamp(1_000_000 + VOUCH_COOLDOWN_MS + 1);
        let result = vouch(voucher.as_ptr(), vouchee2.as_ptr());
        assert_eq!(result, 0);
    }

    #[test]
    fn test_pause_blocks_vouch() {
        setup();
        let admin = [1u8; 32];
        initialize(admin.as_ptr());
        test_mock::set_timestamp(1_000_000);

        let voucher = [2u8; 32];
        let vouchee = [3u8; 32];
        let name1 = b"voucher2";
        let name2 = b"vouchee3";
        register_identity(voucher.as_ptr(), 1, name1.as_ptr(), name1.len() as u32);
        register_identity(vouchee.as_ptr(), 1, name2.as_ptr(), name2.len() as u32);
        update_reputation(admin.as_ptr(), voucher.as_ptr(), 100, 1);

        // Pause
        mid_pause(admin.as_ptr());
        let result = vouch(voucher.as_ptr(), vouchee.as_ptr());
        assert_eq!(result, 20);

        // Unpause
        mid_unpause(admin.as_ptr());
        let result = vouch(voucher.as_ptr(), vouchee.as_ptr());
        assert_eq!(result, 0);
    }

    #[test]
    fn test_transfer_admin() {
        setup();
        let admin = [1u8; 32];
        let new_admin = [10u8; 32];
        let other = [11u8; 32];
        initialize(admin.as_ptr());

        // Non-admin can't transfer
        assert_eq!(transfer_admin(other.as_ptr(), new_admin.as_ptr()), 1);

        // Admin transfers
        assert_eq!(transfer_admin(admin.as_ptr(), new_admin.as_ptr()), 0);

        // Old admin no longer works
        assert_eq!(mid_pause(admin.as_ptr()), 1);
        // New admin works
        assert_eq!(mid_pause(new_admin.as_ptr()), 0);
    }
}
