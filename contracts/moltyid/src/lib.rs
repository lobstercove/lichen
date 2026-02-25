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
    log_info, storage_get, storage_set, bytes_to_u64, u64_to_bytes, get_timestamp,
    Address, call_token_transfer, get_caller,
};

// ============================================================================
// REENTRANCY GUARD
// ============================================================================

const MOLTYID_REENTRANCY_KEY: &[u8] = b"mid_reentrancy";

fn reentrancy_enter() -> bool {
    if let Some(v) = storage_get(MOLTYID_REENTRANCY_KEY) {
        if !v.is_empty() && v[0] == 1 { return false; }
    }
    storage_set(MOLTYID_REENTRANCY_KEY, &[1u8]);
    true
}

fn reentrancy_exit() {
    storage_set(MOLTYID_REENTRANCY_KEY, &[0u8]);
}

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
/// Reputation decay period (90 days, in milliseconds)
const REPUTATION_DECAY_PERIOD_MS: u64 = 7_776_000_000;
/// Reputation decay rate per period, in basis points (5%)
const REPUTATION_DECAY_BPS: u64 = 500;
/// Maximum decay periods applied in a single call (bounds compute cost)
const MAX_DECAY_PERIODS_PER_CALL: u64 = 64;
/// Social recovery guardian count (fixed 5)
const RECOVERY_GUARDIAN_COUNT: usize = 5;
/// Social recovery threshold (3 of 5)
const RECOVERY_THRESHOLD: usize = 3;
/// Maximum delegation validity window (1 year, in ms)
const MAX_DELEGATION_TTL_MS: u64 = 31_536_000_000;

const DELEGATE_PERM_PROFILE: u8 = 0b0000_0001;
const DELEGATE_PERM_AGENT_TYPE: u8 = 0b0000_0010;
const DELEGATE_PERM_SKILLS: u8 = 0b0000_0100;
const DELEGATE_PERM_NAMING: u8 = 0b0000_1000;

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
const AGENT_TYPE_PERSONAL: u8 = 10;

fn is_valid_agent_type(agent_type: u8) -> bool {
    matches!(
        agent_type,
        AGENT_TYPE_UNKNOWN
            | AGENT_TYPE_TRADING
            | AGENT_TYPE_DEVELOPMENT
            | AGENT_TYPE_ANALYSIS
            | AGENT_TYPE_CREATIVE
            | AGENT_TYPE_INFRASTRUCTURE
            | AGENT_TYPE_GOVERNANCE
            | AGENT_TYPE_ORACLE
            | AGENT_TYPE_STORAGE
            | AGENT_TYPE_GENERAL
            | AGENT_TYPE_PERSONAL
    )
}

// ============================================================================
// .MOLT NAMING SYSTEM
// ============================================================================

/// Minimum name length for .molt domains
const MIN_MOLT_NAME_LEN: usize = 3;
/// Maximum name length for .molt domains
const MAX_MOLT_NAME_LEN: usize = 32;
/// Base registration cost (in shells) for 5+ char names
const NAME_COST_BASE: u64 = 20_000_000_000; // 20 MOLT ($2.00 at $0.10)
/// Premium cost for 4-char names
const NAME_COST_4CHAR: u64 = 100_000_000_000; // 100 MOLT ($10.00 at $0.10)
/// Premium cost for 3-char names
const NAME_COST_3CHAR: u64 = 500_000_000_000; // 500 MOLT ($50.00 at $0.10)
/// Slots per year (approx: 2.5 slots/sec * 86400 * 365, matching core/consensus.rs)
const SLOTS_PER_YEAR: u64 = 78_840_000;
/// Premium names up to this length are auction-only
const PREMIUM_AUCTION_MAX_LEN: usize = 4;
/// Auction minimum duration (1 day = 86400s / 0.4s per slot)
const NAME_AUCTION_MIN_SLOTS: u64 = 216_000;
/// Auction maximum duration (14 days = 1209600s / 0.4s per slot)
const NAME_AUCTION_MAX_SLOTS: u64 = 3_024_000;
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

fn vouch_given_key(voucher: &[u8], index: u16) -> Vec<u8> {
    let hex = hex_encode_addr(voucher);
    let mut key = Vec::with_capacity(13 + 64 + 6);
    key.extend_from_slice(b"vouch_given:");
    key.extend_from_slice(&hex);
    key.push(b':');
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

fn name_auction_key(name: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(9 + name.len());
    key.extend_from_slice(b"name_auc:");
    key.extend_from_slice(name);
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

fn delegation_key(owner: &[u8], delegate: &[u8]) -> Vec<u8> {
    let owner_hex = hex_encode_addr(owner);
    let delegate_hex = hex_encode_addr(delegate);
    let mut key = Vec::with_capacity(6 + 64 + 1 + 64);
    key.extend_from_slice(b"dleg:");
    key.extend_from_slice(&owner_hex);
    key.push(b':');
    key.extend_from_slice(&delegate_hex);
    key
}

fn has_active_permission(owner: &[u8], actor: &[u8], permission: u8, now_ms: u64) -> bool {
    if owner == actor {
        return true;
    }

    let dk = delegation_key(owner, actor);
    match storage_get(&dk) {
        Some(data) if data.len() >= 9 => {
            let perms = data[0];
            let expires_at = bytes_to_u64(&data[1..9]);
            if now_ms > expires_at {
                moltchain_sdk::storage::remove(&dk);
                return false;
            }
            (perms & permission) != 0
        }
        _ => false,
    }
}

fn is_premium_name(name: &[u8]) -> bool {
    name.len() >= MIN_MOLT_NAME_LEN && name.len() <= PREMIUM_AUCTION_MAX_LEN
}

fn recovery_guardians_key(addr: &[u8]) -> Vec<u8> {
    let hex = hex_encode_addr(addr);
    let mut key = Vec::with_capacity(6 + 64);
    key.extend_from_slice(b"recg:");
    key.extend_from_slice(&hex);
    key
}

fn recovery_nonce_key(addr: &[u8]) -> Vec<u8> {
    let hex = hex_encode_addr(addr);
    let mut key = Vec::with_capacity(6 + 64);
    key.extend_from_slice(b"recn:");
    key.extend_from_slice(&hex);
    key
}

fn recovery_candidate_key(addr: &[u8]) -> Vec<u8> {
    let hex = hex_encode_addr(addr);
    let mut key = Vec::with_capacity(6 + 64);
    key.extend_from_slice(b"recc:");
    key.extend_from_slice(&hex);
    key
}

fn recovery_approval_key(target: &[u8], nonce: u64, guardian: &[u8]) -> Vec<u8> {
    let target_hex = hex_encode_addr(target);
    let guardian_hex = hex_encode_addr(guardian);
    let mut key = Vec::with_capacity(7 + 64 + 1 + 8 + 1 + 64);
    key.extend_from_slice(b"reca:");
    key.extend_from_slice(&target_hex);
    key.push(b':');
    key.extend_from_slice(&u64_to_bytes(nonce));
    key.push(b':');
    key.extend_from_slice(&guardian_hex);
    key
}

fn is_zero_address(data: &[u8]) -> bool {
    data.len() >= 32 && data[0..32].iter().all(|&b| b == 0)
}

fn vouch_count_from_record(record: &[u8]) -> u16 {
    if record.len() >= 126 {
        (record[124] as u16) | ((record[125] as u16) << 8)
    } else {
        0
    }
}

fn has_vouched_for(vouchee: &[u8], voucher: &[u8]) -> bool {
    let id_key = identity_key(vouchee);
    let record = match storage_get(&id_key) {
        Some(data) => data,
        None => return false,
    };
    let vouch_count = vouch_count_from_record(&record);
    for i in 0..vouch_count {
        let vk = vouch_key(vouchee, i);
        if let Some(data) = storage_get(&vk) {
            if data.len() >= 32 && &data[0..32] == voucher {
                return true;
            }
        }
    }
    false
}

fn is_configured_guardian(target: &[u8], guardian: &[u8]) -> bool {
    let gk = recovery_guardians_key(target);
    let guardians = match storage_get(&gk) {
        Some(data) => data,
        None => return false,
    };
    if guardians.len() != RECOVERY_GUARDIAN_COUNT * 32 {
        return false;
    }
    for chunk in guardians.chunks(32) {
        if chunk == guardian {
            return true;
        }
    }
    false
}

fn recovery_nonce(target: &[u8]) -> u64 {
    let nk = recovery_nonce_key(target);
    storage_get(&nk).map(|d| bytes_to_u64(&d)).unwrap_or(0)
}

fn recovery_approval_count(target: &[u8], nonce: u64) -> usize {
    let gk = recovery_guardians_key(target);
    let guardians = match storage_get(&gk) {
        Some(data) if data.len() == RECOVERY_GUARDIAN_COUNT * 32 => data,
        _ => return 0,
    };

    let mut count = 0usize;
    for guardian in guardians.chunks(32) {
        let ak = recovery_approval_key(target, nonce, guardian);
        if storage_get(&ak).is_some() {
            count += 1;
        }
    }
    count
}

/// AUDIT-FIX NEW-M1: Returns (decayed_rep, periods_applied) so callers can
/// advance last_updated by exactly the applied periods, preserving remainder.
fn apply_reputation_decay(current_rep: u64, last_updated_ms: u64, now_ms: u64) -> (u64, u64) {
    if current_rep <= INITIAL_REPUTATION || now_ms <= last_updated_ms {
        return (current_rep, 0);
    }

    let elapsed = now_ms - last_updated_ms;
    let mut periods = elapsed / REPUTATION_DECAY_PERIOD_MS;
    if periods == 0 {
        return (current_rep, 0);
    }
    if periods > MAX_DECAY_PERIODS_PER_CALL {
        periods = MAX_DECAY_PERIODS_PER_CALL;
    }

    let mut decayed = current_rep;
    for _ in 0..periods {
        decayed = decayed
            .saturating_mul(10_000 - REPUTATION_DECAY_BPS)
            / 10_000;
        if decayed <= MIN_REPUTATION {
            return (MIN_REPUTATION, periods);
        }
    }

    (decayed, periods)
}

fn apply_decay_to_identity_record(
    pubkey: &[u8],
    id_key: &Vec<u8>,
    record: &mut Vec<u8>,
    now_ms: u64,
) -> u64 {
    let current_rep = if record.len() >= 107 {
        bytes_to_u64(&record[99..107])
    } else {
        INITIAL_REPUTATION
    };

    let last_updated_ms = if record.len() >= 123 {
        bytes_to_u64(&record[115..123])
    } else {
        now_ms
    };

    let (decayed, periods_applied) = apply_reputation_decay(current_rep, last_updated_ms, now_ms);
    if decayed != current_rep {
        if record.len() >= 107 {
            record[99..107].copy_from_slice(&u64_to_bytes(decayed));
        }
        // AUDIT-FIX NEW-M1: Advance last_updated by only the applied periods,
        // so remaining un-applied decay carries over to the next call.
        // (Previously set to now_ms, which forgave capped excess periods.)
        let new_last_updated = last_updated_ms + periods_applied * REPUTATION_DECAY_PERIOD_MS;
        if record.len() >= 123 {
            record[115..123].copy_from_slice(&u64_to_bytes(new_last_updated));
        }
        storage_set(id_key, record);
        storage_set(&reputation_key(pubkey), &u64_to_bytes(decayed));
    }

    decayed
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
        // System / admin
        b"admin", b"system", b"validator", b"root", b"node", b"test",
        // Chain identity
        b"moltchain", b"molt", b"moltyid", b"treasury",
        // Core token + wrapped
        b"moltcoin", b"musd", b"wsol", b"weth",
        // DEX
        b"dex", b"amm", b"router", b"margin", b"rewards", b"governance", b"analytics",
        // DeFi protocols
        b"moltswap", b"bridge", b"oracle", b"dao", b"lending",
        // Marketplaces / NFT
        b"marketplace", b"auction", b"moltpunks",
        // Infrastructure
        b"clawpay", b"clawpump", b"clawvault", b"bountyboard", b"compute", b"reefstake",
        // Prediction Markets
        b"predict", b"prediction",
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
    if !reentrancy_enter() { return 100; }
    log_info("🪪 Initializing MoltyID program...");

    let mut admin = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(admin_ptr, admin.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != admin {
        reentrancy_exit();
        return 200;
    }

    // Check not already initialized
    if storage_get(b"mid_admin").is_some() {
        log_info("MoltyID already initialized");
        reentrancy_exit();
        return 1;
    }

    storage_set(b"mid_admin", &admin);
    storage_set(b"mid_identity_count", &u64_to_bytes(0));
    storage_set(b"mid_initialized", &[1]);

    log_info("MoltyID initialized");
    reentrancy_exit();
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
    if !reentrancy_enter() { return 100; }
    log_info("🪪 Registering new MoltyID identity...");

    if is_mid_paused() {
        log_info("MoltyID is paused");
        reentrancy_exit();
        return 20;
    }

    let mut owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != owner {
        reentrancy_exit();
        return 200;
    }

    let name_len = name_len as usize;

    if name_len == 0 || name_len > MAX_NAME_LEN {
        log_info("Invalid name length");
        reentrancy_exit();
        return 1;
    }

    let mut name = alloc::vec![0u8; name_len];
    unsafe { core::ptr::copy_nonoverlapping(name_ptr, name.as_mut_ptr(), name_len); }

    // Validate agent type
    if !is_valid_agent_type(agent_type) {
        log_info("Invalid agent type");
        reentrancy_exit();
        return 2;
    }

    // Check not already registered
    let id_key = identity_key(&owner);
    if storage_get(&id_key).is_some() {
        log_info("Identity already registered for this address");
        reentrancy_exit();
        return 3;
    }

    // Hardening: registration cooldown (checked after duplicate to preserve error codes)
    let now = get_timestamp();
    let rck = register_cooldown_key(&owner);
    if let Some(last) = storage_get(&rck) {
        let last_ts = bytes_to_u64(&last);
        if now < last_ts + REGISTER_COOLDOWN_MS {
            log_info("Registration cooldown active");
            reentrancy_exit();
            return 21;
        }
    }
    storage_set(&rck, &u64_to_bytes(now));

    // Build identity record
    let mut record = [0u8; IDENTITY_SIZE];

    // Bytes 0..32: owner
    record[0..32].copy_from_slice(&owner);
    // Byte 32: agent_type
    record[32] = agent_type;
    // Bytes 33..35: name_len (u16 LE)
    record[33] = (name_len & 0xFF) as u8;
    record[34] = ((name_len >> 8) & 0xFF) as u8;
    // Bytes 35..99: name (padded with zeros)
    record[35..35 + name_len].copy_from_slice(&name);
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
    let rep_key = reputation_key(&owner);
    storage_set(&rep_key, &rep_bytes);

    // Increment global identity count
    let count = match storage_get(b"mid_identity_count") {
        Some(data) if data.len() >= 8 => bytes_to_u64(&data),
        _ => 0,
    };
    storage_set(b"mid_identity_count", &u64_to_bytes(count + 1));

    log_info("Identity registered successfully");
    reentrancy_exit();
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
    let mut pubkey = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(pubkey_ptr, pubkey.as_mut_ptr(), 32); }
    let id_key = identity_key(&pubkey);

    match storage_get(&id_key) {
        Some(data) => {
            moltchain_sdk::set_return_data(&data);
            0
        }
        None => {
            log_info("Identity not found");
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
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let mut target = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(target_ptr, target.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    // Only admin can update reputation
    let admin = match storage_get(b"mid_admin") {
        Some(data) => data,
        None => {
            log_info("MoltyID not initialized");
            reentrancy_exit();
            return 1;
        }
    };
    if caller[..] != admin[..] {
        log_info("Unauthorized: only admin can update reputation");
        reentrancy_exit();
        return 2;
    }

    let id_key = identity_key(&target);
    let mut record = match storage_get(&id_key) {
        Some(data) => data,
        None => {
            log_info("Target identity not found");
            reentrancy_exit();
            return 3;
        }
    };

    let now = get_timestamp();
    let current_rep = apply_decay_to_identity_record(&target, &id_key, &mut record, now);

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
            log_info("Invalid contribution type");
            reentrancy_exit();
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
    let now_bytes = u64_to_bytes(now);
    if record.len() >= 123 {
        record[115..123].copy_from_slice(&now_bytes);
    }

    storage_set(&id_key, &record);
    storage_set(&reputation_key(&target), &rep_bytes);

    // Track contribution counts for the formula
    let hex = hex_encode_addr(&target);
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
    check_achievements(&target, new_rep);

    log_info(&alloc::format!("Reputation updated: {} → {} (type: {}, Δ: {}{})",
        current_rep, new_rep, contribution_type,
        if is_positive { "+" } else { "-" }, delta));
    reentrancy_exit();
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
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let mut target = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(target_ptr, target.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    // Only admin can directly update reputation
    let admin = match storage_get(b"mid_admin") {
        Some(data) => data,
        None => {
            log_info("MoltyID not initialized");
            reentrancy_exit();
            return 1;
        }
    };
    if caller[..] != admin[..] {
        log_info("Unauthorized: only admin can update reputation");
        reentrancy_exit();
        return 2;
    }

    // Check identity exists
    let id_key = identity_key(&target);
    let mut record = match storage_get(&id_key) {
        Some(data) => data,
        None => {
            log_info("Target identity not found");
            reentrancy_exit();
            return 3;
        }
    };

    let now = get_timestamp();
    let current_rep = apply_decay_to_identity_record(&target, &id_key, &mut record, now);

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
    let now_bytes = u64_to_bytes(now);
    if record.len() >= 123 {
        record[115..123].copy_from_slice(&now_bytes);
    }

    storage_set(&id_key, &record);

    // Update standalone reputation key
    let rep_key = reputation_key(&target);
    storage_set(&rep_key, &rep_bytes);

    log_info("Reputation updated");
    reentrancy_exit();
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
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    let skill_name_len = skill_name_len as usize;

    if skill_name_len == 0 || skill_name_len > MAX_SKILL_LEN {
        log_info("Invalid skill name length");
        reentrancy_exit();
        return 1;
    }

    if proficiency > 100 {
        log_info("Proficiency must be 0-100");
        reentrancy_exit();
        return 2;
    }

    let mut skill_name = alloc::vec![0u8; skill_name_len];
    unsafe { core::ptr::copy_nonoverlapping(skill_name_ptr, skill_name.as_mut_ptr(), skill_name_len); }

    // Load identity
    let id_key = identity_key(&caller);
    let mut record = match storage_get(&id_key) {
        Some(data) => data,
        None => {
            log_info("Identity not found — register first");
            reentrancy_exit();
            return 3;
        }
    };

    // Verify caller owns this identity
    if record.len() < IDENTITY_SIZE || record[0..32] != caller[..] {
        log_info("Unauthorized: not identity owner");
        reentrancy_exit();
        return 4;
    }

    // Check skill count limit
    let skill_count = record[123];
    if skill_count as usize >= MAX_SKILLS {
        log_info("Maximum skills reached");
        reentrancy_exit();
        return 5;
    }

    // Store skill: [name_len(1), name(up to 32), proficiency(1), timestamp(8)]
    let mut skill_data = Vec::with_capacity(1 + skill_name_len + 1 + 8);
    skill_data.push(skill_name_len as u8);
    skill_data.extend_from_slice(&skill_name);
    skill_data.push(proficiency);
    let ts_bytes = u64_to_bytes(get_timestamp());
    skill_data.extend_from_slice(&ts_bytes);

    let sk = skill_key(&caller, skill_count);
    storage_set(&sk, &skill_data);

    // Increment skill count in identity record
    record[123] = skill_count + 1;
    // Update updated_at
    if record.len() >= 123 {
        record[115..123].copy_from_slice(&ts_bytes);
    }

    // Award reputation for adding a skill (+10 rep)
    let old_rep = bytes_to_u64(&record[99..107]);
    let new_rep = core::cmp::min(old_rep.saturating_add(10), MAX_REPUTATION);
    record[99..107].copy_from_slice(&u64_to_bytes(new_rep));
    storage_set(&id_key, &record);
    storage_set(&reputation_key(&caller), &u64_to_bytes(new_rep));

    // Check achievements based on new rep and skill count
    check_achievements_full(&caller, new_rep, record[123], vouch_count_from_record(&record));

    log_info("Skill added");
    reentrancy_exit();
    0
}

/// Add a skill to an owner's identity as a delegated actor.
/// Delegate must have DELEGATE_PERM_SKILLS.
#[no_mangle]
pub extern "C" fn add_skill_as(
    delegate_ptr: *const u8,
    owner_ptr: *const u8,
    skill_name_ptr: *const u8,
    skill_name_len: u32,
    proficiency: u8,
) -> u32 {
    if !reentrancy_enter() { return 100; }
    let mut delegate = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(delegate_ptr, delegate.as_mut_ptr(), 32); }
    let mut owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != delegate {
        reentrancy_exit();
        return 200;
    }

    let skill_name_len = skill_name_len as usize;

    if skill_name_len == 0 || skill_name_len > MAX_SKILL_LEN {
        log_info("Invalid skill name length");
        reentrancy_exit();
        return 1;
    }

    if proficiency > 100 {
        log_info("Proficiency must be 0-100");
        reentrancy_exit();
        return 2;
    }

    let now = get_timestamp();
    if !has_active_permission(&owner, &delegate, DELEGATE_PERM_SKILLS, now) {
        log_info("Unauthorized: delegate lacks skill permission");
        reentrancy_exit();
        return 3;
    }

    let mut skill_name = alloc::vec![0u8; skill_name_len];
    unsafe { core::ptr::copy_nonoverlapping(skill_name_ptr, skill_name.as_mut_ptr(), skill_name_len); }

    // Load owner's identity
    let id_key = identity_key(&owner);
    let mut record = match storage_get(&id_key) {
        Some(data) => data,
        None => {
            log_info("Identity not found — register first");
            reentrancy_exit();
            return 4;
        }
    };

    // Check skill count limit
    let skill_count = record[123];
    if skill_count as usize >= MAX_SKILLS {
        log_info("Maximum skills reached");
        reentrancy_exit();
        return 5;
    }

    // Store skill: [name_len(1), name(up to 32), proficiency(1), timestamp(8)]
    let mut skill_data = Vec::with_capacity(1 + skill_name_len + 1 + 8);
    skill_data.push(skill_name_len as u8);
    skill_data.extend_from_slice(&skill_name);
    skill_data.push(proficiency);
    let ts_bytes = u64_to_bytes(now);
    skill_data.extend_from_slice(&ts_bytes);

    let sk = skill_key(&owner, skill_count);
    storage_set(&sk, &skill_data);

    record[123] = skill_count + 1;
    if record.len() >= 123 {
        record[115..123].copy_from_slice(&ts_bytes);
    }
    storage_set(&id_key, &record);

    log_info("Delegated skill added");
    reentrancy_exit();
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
    let mut pubkey = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(pubkey_ptr, pubkey.as_mut_ptr(), 32); }

    let id_key = identity_key(&pubkey);
    let record = match storage_get(&id_key) {
        Some(data) => data,
        None => {
            log_info("Identity not found");
            return 1;
        }
    };

    let skill_count = if record.len() > 123 { record[123] } else { 0 };
    let mut all_skills = Vec::new();
    all_skills.push(skill_count);

    for i in 0..skill_count {
        let sk = skill_key(&pubkey, i);
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
    if !reentrancy_enter() { return 100; }
    let mut voucher = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(voucher_ptr, voucher.as_mut_ptr(), 32); }
    let mut vouchee = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(vouchee_ptr, vouchee.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != voucher {
        reentrancy_exit();
        return 200;
    }

    if is_mid_paused() {
        reentrancy_exit();
        return 20;
    }

    // Can't vouch for yourself
    if voucher[..] == vouchee[..] {
        log_info("Cannot vouch for yourself");
        reentrancy_exit();
        return 1;
    }

    // Both must have identities
    let voucher_id_key = identity_key(&voucher);
    let vouchee_id_key = identity_key(&vouchee);

    let mut voucher_record = match storage_get(&voucher_id_key) {
        Some(data) => data,
        None => {
            log_info("Voucher identity not found");
            reentrancy_exit();
            return 2;
        }
    };

    let mut vouchee_record = match storage_get(&vouchee_id_key) {
        Some(data) => data,
        None => {
            log_info("Vouchee identity not found");
            reentrancy_exit();
            return 3;
        }
    };

    let now = get_timestamp();
    let voucher_rep = apply_decay_to_identity_record(&voucher, &voucher_id_key, &mut voucher_record, now);
    let vouchee_rep = apply_decay_to_identity_record(&vouchee, &vouchee_id_key, &mut vouchee_record, now);

    // Check voucher has enough reputation
    if voucher_rep < VOUCH_COST {
        log_info("Insufficient reputation to vouch");
        reentrancy_exit();
        return 4;
    }

    // Check vouchee vouch count limit
    let vouchee_vouch_count = if vouchee_record.len() >= 126 {
        (vouchee_record[124] as u16) | ((vouchee_record[125] as u16) << 8)
    } else {
        0
    };

    let voucher_vouch_count = if voucher_record.len() >= 126 {
        (voucher_record[124] as u16) | ((voucher_record[125] as u16) << 8)
    } else {
        0
    };

    if vouchee_vouch_count as usize >= MAX_VOUCHES {
        log_info("Vouchee has reached maximum vouches");
        reentrancy_exit();
        return 5;
    }

    // Check voucher hasn't already vouched for this vouchee
    for i in 0..vouchee_vouch_count {
        let vk = vouch_key(&vouchee, i);
        if let Some(data) = storage_get(&vk) {
            if data.len() >= 32 && &data[0..32] == voucher {
                log_info("Already vouched for this agent");
                reentrancy_exit();
                return 6;
            }
        }
    }

    // Hardening: vouch cooldown (after all other checks to preserve error codes)
    let vck = vouch_cooldown_key(&voucher);
    if let Some(last) = storage_get(&vck) {
        let last_ts = bytes_to_u64(&last);
        if now < last_ts + VOUCH_COOLDOWN_MS {
            log_info("Vouch cooldown active");
            reentrancy_exit();
            return 21;
        }
    }
    storage_set(&vck, &u64_to_bytes(now));

    let ts_bytes = u64_to_bytes(now);

    // Store vouch record: [voucher_addr(32), timestamp(8)]
    let mut vouch_data = Vec::with_capacity(40);
    vouch_data.extend_from_slice(&voucher);
    vouch_data.extend_from_slice(&ts_bytes);

    let vk = vouch_key(&vouchee, vouchee_vouch_count);
    storage_set(&vk, &vouch_data);

    // Reverse index for O(1) "given vouches" lookup in RPC:
    // key: vouch_given:{voucher_hex}:{index} -> [vouchee(32), timestamp(8)]
    let gvk = vouch_given_key(&voucher, voucher_vouch_count);
    let mut gv_data = Vec::with_capacity(40);
    gv_data.extend_from_slice(&vouchee);
    gv_data.extend_from_slice(&ts_bytes);
    storage_set(&gvk, &gv_data);

    // Deduct reputation from voucher
    let new_voucher_rep = voucher_rep - VOUCH_COST;
    let voucher_rep_bytes = u64_to_bytes(new_voucher_rep);
    if voucher_record.len() >= 126 {
        voucher_record[99..107].copy_from_slice(&voucher_rep_bytes);
        let new_voucher_count = voucher_vouch_count + 1;
        voucher_record[124] = (new_voucher_count & 0xFF) as u8;
        voucher_record[125] = ((new_voucher_count >> 8) & 0xFF) as u8;
        voucher_record[115..123].copy_from_slice(&ts_bytes);
    }
    storage_set(&voucher_id_key, &voucher_record);
    storage_set(&reputation_key(&voucher), &voucher_rep_bytes);

    // Add reputation to vouchee
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
    storage_set(&reputation_key(&vouchee), &vouchee_rep_bytes);

    // Check achievements for vouchee (rep milestones + vouch count)
    let vouchee_vouch_new = vouchee_vouch_count + 1;
    let vouchee_skill_count = if vouchee_record.len() >= 124 { vouchee_record[123] } else { 0 };
    check_achievements_full(&vouchee, new_vouchee_rep, vouchee_skill_count, vouchee_vouch_new as u16);

    log_info("Vouch recorded successfully");
    reentrancy_exit();
    0
}

// ============================================================================
// SOCIAL RECOVERY (3-of-5 guardians)
// ============================================================================

/// Configure 5 guardians for social recovery.
/// Guardians must have already vouched for the caller.
#[no_mangle]
pub extern "C" fn set_recovery_guardians(
    caller_ptr: *const u8,
    guardian1_ptr: *const u8,
    guardian2_ptr: *const u8,
    guardian3_ptr: *const u8,
    guardian4_ptr: *const u8,
    guardian5_ptr: *const u8,
) -> u32 {
    if !reentrancy_enter() { return 100; }
    if is_mid_paused() {
        reentrancy_exit();
        return 20;
    }

    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let mut guardian1 = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(guardian1_ptr, guardian1.as_mut_ptr(), 32); }
    let mut guardian2 = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(guardian2_ptr, guardian2.as_mut_ptr(), 32); }
    let mut guardian3 = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(guardian3_ptr, guardian3.as_mut_ptr(), 32); }
    let mut guardian4 = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(guardian4_ptr, guardian4.as_mut_ptr(), 32); }
    let mut guardian5 = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(guardian5_ptr, guardian5.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    let caller_id_key = identity_key(&caller);
    if storage_get(&caller_id_key).is_none() {
        log_info("Identity not found");
        reentrancy_exit();
        return 1;
    }

    let guardians: [[u8; 32]; RECOVERY_GUARDIAN_COUNT] = [guardian1, guardian2, guardian3, guardian4, guardian5];

    for i in 0..RECOVERY_GUARDIAN_COUNT {
        if guardians[i] == caller {
            log_info("Caller cannot be a guardian");
            reentrancy_exit();
            return 2;
        }
        for j in (i + 1)..RECOVERY_GUARDIAN_COUNT {
            if guardians[i] == guardians[j] {
                log_info("Guardians must be unique");
                reentrancy_exit();
                return 3;
            }
        }
    }

    for guardian in guardians.iter() {
        if !has_vouched_for(&caller, guardian) {
            log_info("Guardian must have vouched for caller");
            reentrancy_exit();
            return 4;
        }
    }

    let mut data = Vec::with_capacity(RECOVERY_GUARDIAN_COUNT * 32);
    for guardian in guardians.iter() {
        data.extend_from_slice(guardian);
    }
    let gk = recovery_guardians_key(&caller);
    storage_set(&gk, &data);

    let next_nonce = recovery_nonce(&caller).saturating_add(1);
    let nk = recovery_nonce_key(&caller);
    storage_set(&nk, &u64_to_bytes(next_nonce));

    let ck = recovery_candidate_key(&caller);
    storage_set(&ck, &[0u8; 32]);

    log_info("Recovery guardians configured");
    reentrancy_exit();
    0
}

/// Guardian approval for recovering a target identity to a new owner key.
#[no_mangle]
pub extern "C" fn approve_recovery(
    guardian_ptr: *const u8,
    target_ptr: *const u8,
    new_owner_ptr: *const u8,
) -> u32 {
    if !reentrancy_enter() { return 100; }
    if is_mid_paused() {
        reentrancy_exit();
        return 20;
    }

    let mut guardian = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(guardian_ptr, guardian.as_mut_ptr(), 32); }
    let mut target = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(target_ptr, target.as_mut_ptr(), 32); }
    let mut new_owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(new_owner_ptr, new_owner.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != guardian {
        reentrancy_exit();
        return 200;
    }

    if target[..] == new_owner[..] {
        log_info("Target and new owner cannot be the same");
        reentrancy_exit();
        return 1;
    }

    if !is_configured_guardian(&target, &guardian) {
        log_info("Caller is not a configured guardian");
        reentrancy_exit();
        return 2;
    }

    let target_id_key = identity_key(&target);
    if storage_get(&target_id_key).is_none() {
        log_info("Target identity not found");
        reentrancy_exit();
        return 3;
    }

    let nonce = recovery_nonce(&target);
    let ck = recovery_candidate_key(&target);
    if let Some(existing) = storage_get(&ck) {
        if existing.len() >= 32 && !is_zero_address(&existing) && &existing[0..32] != new_owner {
            log_info("Recovery candidate already set to a different owner");
            reentrancy_exit();
            return 4;
        }
    }
    storage_set(&ck, &new_owner);

    let ak = recovery_approval_key(&target, nonce, &guardian);
    if storage_get(&ak).is_some() {
        log_info("Guardian already approved this recovery");
        reentrancy_exit();
        return 5;
    }
    storage_set(&ak, &[1]);

    log_info("Recovery approval recorded");
    reentrancy_exit();
    0
}

/// Execute social recovery after threshold guardian approvals.
#[no_mangle]
pub extern "C" fn execute_recovery(
    caller_ptr: *const u8,
    target_ptr: *const u8,
    new_owner_ptr: *const u8,
) -> u32 {
    if !reentrancy_enter() { return 100; }
    if is_mid_paused() {
        reentrancy_exit();
        return 20;
    }

    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let mut target = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(target_ptr, target.as_mut_ptr(), 32); }
    let mut new_owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(new_owner_ptr, new_owner.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    if target[..] == new_owner[..] {
        log_info("Target and new owner cannot be the same");
        reentrancy_exit();
        return 1;
    }

    if !is_configured_guardian(&target, &caller) {
        log_info("Caller is not a configured guardian");
        reentrancy_exit();
        return 2;
    }

    let nonce = recovery_nonce(&target);
    let ck = recovery_candidate_key(&target);
    let candidate = match storage_get(&ck) {
        Some(data) if data.len() >= 32 => data,
        _ => {
            log_info("No active recovery candidate");
            reentrancy_exit();
            return 3;
        }
    };
    if is_zero_address(&candidate) || &candidate[0..32] != new_owner {
        log_info("Candidate mismatch");
        reentrancy_exit();
        return 4;
    }

    let approvals = recovery_approval_count(&target, nonce);
    if approvals < RECOVERY_THRESHOLD {
        log_info("Insufficient guardian approvals");
        reentrancy_exit();
        return 5;
    }

    let old_id_key = identity_key(&target);
    let mut old_record = match storage_get(&old_id_key) {
        Some(data) => data,
        None => {
            log_info("Target identity not found");
            reentrancy_exit();
            return 6;
        }
    };

    let new_id_key = identity_key(&new_owner);
    if storage_get(&new_id_key).is_some() {
        log_info("New owner already has an identity");
        reentrancy_exit();
        return 7;
    }

    let now = get_timestamp();
    let now_bytes = u64_to_bytes(now);
    let old_rep = if old_record.len() >= 107 {
        bytes_to_u64(&old_record[99..107])
    } else {
        INITIAL_REPUTATION
    };

    let mut new_record = old_record.clone();
    if new_record.len() >= IDENTITY_SIZE {
        new_record[0..32].copy_from_slice(&new_owner);
        new_record[115..123].copy_from_slice(&now_bytes);
        new_record[126] = 1;
    }
    storage_set(&new_id_key, &new_record);

    if old_record.len() >= IDENTITY_SIZE {
        old_record[99..107].copy_from_slice(&u64_to_bytes(0));
        old_record[115..123].copy_from_slice(&now_bytes);
        old_record[126] = 0;
    }
    storage_set(&old_id_key, &old_record);

    storage_set(&reputation_key(&new_owner), &u64_to_bytes(old_rep));
    storage_set(&reputation_key(&target), &u64_to_bytes(0));

    for (old_key, new_key) in [
        (endpoint_key(&target), endpoint_key(&new_owner)),
        (metadata_key(&target), metadata_key(&new_owner)),
        (availability_key(&target), availability_key(&new_owner)),
        (rate_key(&target), rate_key(&new_owner)),
    ] {
        if let Some(data) = storage_get(&old_key) {
            storage_set(&new_key, &data);
            moltchain_sdk::storage::remove(&old_key);
        }
    }

    let old_rev = name_reverse_key(&target);
    if let Some(name_bytes) = storage_get(&old_rev) {
        let nk = name_key(&name_bytes);
        if let Some(mut name_record) = storage_get(&nk) {
            if name_record.len() >= 48 && name_record[0..32] == target[..] {
                name_record[0..32].copy_from_slice(&new_owner);
                storage_set(&nk, &name_record);
            }
        }
        let new_rev = name_reverse_key(&new_owner);
        storage_set(&new_rev, &name_bytes);
        moltchain_sdk::storage::remove(&old_rev);
    }

    let old_gk = recovery_guardians_key(&target);
    if let Some(guardians) = storage_get(&old_gk) {
        let new_gk = recovery_guardians_key(&new_owner);
        storage_set(&new_gk, &guardians);
        moltchain_sdk::storage::remove(&old_gk);
    }

    let old_nk = recovery_nonce_key(&target);
    let new_nk = recovery_nonce_key(&new_owner);
    storage_set(&new_nk, &u64_to_bytes(nonce.saturating_add(1)));
    moltchain_sdk::storage::remove(&old_nk);

    let old_ck = recovery_candidate_key(&target);
    let new_ck = recovery_candidate_key(&new_owner);
    storage_set(&new_ck, &[0u8; 32]);
    moltchain_sdk::storage::remove(&old_ck);

    log_info("Social recovery executed");
    reentrancy_exit();
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
    let mut pubkey = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(pubkey_ptr, pubkey.as_mut_ptr(), 32); }

    let id_key = identity_key(&pubkey);
    if let Some(mut record) = storage_get(&id_key) {
        let now = get_timestamp();
        let rep = apply_decay_to_identity_record(&pubkey, &id_key, &mut record, now);
        moltchain_sdk::set_return_data(&u64_to_bytes(rep));
        return 0;
    }

    let rep_key = reputation_key(&pubkey);
    if let Some(data) = storage_get(&rep_key) {
        if data.len() >= 8 {
            moltchain_sdk::set_return_data(&data[0..8]);
            return 0;
        }
    }

    log_info("No reputation found for address");
    1
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
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let mut target = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(target_ptr, target.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    let id_key = identity_key(&target);
    let mut record = match storage_get(&id_key) {
        Some(data) => data,
        None => {
            log_info("Identity not found");
            reentrancy_exit();
            return 1;
        }
    };

    // Must be owner or admin
    let is_owner = record.len() >= 32 && record[0..32] == caller[..];
    let is_admin = match storage_get(b"mid_admin") {
        Some(admin) => caller[..] == admin[..],
        None => false,
    };

    if !is_owner && !is_admin {
        log_info("Unauthorized: must be owner or admin");
        reentrancy_exit();
        return 2;
    }

    // Set is_active = 0
    if record.len() >= IDENTITY_SIZE {
        record[126] = 0;
        let ts_bytes = u64_to_bytes(get_timestamp());
        record[115..123].copy_from_slice(&ts_bytes);
    }

    storage_set(&id_key, &record);

    log_info("Identity deactivated");
    reentrancy_exit();
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
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    if !is_valid_agent_type(new_agent_type) {
        log_info("Invalid agent type");
        reentrancy_exit();
        return 1;
    }

    let id_key = identity_key(&caller);
    let mut record = match storage_get(&id_key) {
        Some(data) => data,
        None => {
            log_info("Identity not found");
            reentrancy_exit();
            return 2;
        }
    };

    // Verify ownership
    if record.len() < IDENTITY_SIZE || record[0..32] != caller[..] {
        log_info("Unauthorized");
        reentrancy_exit();
        return 3;
    }

    record[32] = new_agent_type;
    let ts_bytes = u64_to_bytes(get_timestamp());
    record[115..123].copy_from_slice(&ts_bytes);

    storage_set(&id_key, &record);

    log_info("Agent type updated");
    reentrancy_exit();
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
    let mut pubkey = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(pubkey_ptr, pubkey.as_mut_ptr(), 32); }

    let id_key = identity_key(&pubkey);
    let record = match storage_get(&id_key) {
        Some(data) => data,
        None => {
            log_info("Identity not found");
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
        let vk = vouch_key(&pubkey, i);
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
const ACHIEVEMENT_NAME_REGISTRAR: u8 = 9;  // Registered a .molt name
const ACHIEVEMENT_SKILL_MASTER: u8 = 10;   // Added 5+ skills
const ACHIEVEMENT_SOCIAL: u8 = 11;         // Received 3+ vouches
const ACHIEVEMENT_FIRST_NAME: u8 = 12;     // Registered first .molt name

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

/// Extended check: rep milestones + skill/vouch count achievements
fn check_achievements_full(target: &[u8], reputation: u64, skill_count: u8, vouch_count: u16) {
    check_achievements(target, reputation);
    let hex = hex_encode_addr(target);

    // Vouch count milestones
    if vouch_count >= 3 {
        award_achievement(target, &hex, ACHIEVEMENT_SOCIAL, "Social Butterfly (3+ vouches)");
    }
    if vouch_count >= 10 {
        award_achievement(target, &hex, ACHIEVEMENT_ENDORSED, "Well Endorsed (10+ vouches)");
    }

    // Skill count milestones
    if skill_count >= 5 {
        award_achievement(target, &hex, ACHIEVEMENT_SKILL_MASTER, "Skill Master (5+ skills)");
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

    log_info(&alloc::format!("Achievement unlocked: {}", name));
    let _ = target; // suppress unused warning
}

/// Award a contribution-based achievement (called externally by admin)
#[no_mangle]
pub extern "C" fn award_contribution_achievement(
    caller_ptr: *const u8,
    target_ptr: *const u8,
    achievement_id: u8,
) -> u32 {
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let mut target = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(target_ptr, target.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        reentrancy_exit();
        return 200;
    }

    let admin = match storage_get(b"mid_admin") {
        Some(data) => data,
        None => { reentrancy_exit(); return 1; },
    };
    if caller[..] != admin[..] {
        log_info("Unauthorized");
        reentrancy_exit();
        return 2;
    }

    let hex = hex_encode_addr(&target);
    let name = match achievement_id {
        ACHIEVEMENT_FIRST_TX => "First Transaction",
        ACHIEVEMENT_VOTER => "Governance Voter",
        ACHIEVEMENT_BUILDER => "Program Builder",
        ACHIEVEMENT_ENDORSED => "Well Endorsed (10+ vouches)",
        ACHIEVEMENT_GRADUATION => "Bootstrap Graduation ",
        _ => "Unknown Achievement",
    };
    award_achievement(&target, &hex, achievement_id, name);
    reentrancy_exit();
    0
}

/// Get achievements for an identity
#[no_mangle]
pub extern "C" fn get_achievements(pubkey_ptr: *const u8) -> u32 {
    let mut pubkey = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(pubkey_ptr, pubkey.as_mut_ptr(), 32); }
    let hex = hex_encode_addr(&pubkey);

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

/// FNV-1a 128-bit hash of a skill name. Produces a collision-resistant 16-byte
/// digest, unlike the old truncation approach which copied the first 16 bytes
/// verbatim and collided on shared prefixes (e.g. "smart_contracts_audit" vs
/// "smart_contracts_dev").
fn skill_name_hash(skill_name: &[u8]) -> [u8; 16] {
    // FNV-1a 128-bit constants (per the FNV spec)
    const FNV_OFFSET_BASIS: u128 = 0x6c62272e07bb0142_62b821756295c58d;
    const FNV_PRIME: u128 = 0x0000000001000000_000000000000013B;
    let mut hash: u128 = FNV_OFFSET_BASIS;
    for &byte in skill_name {
        hash ^= byte as u128;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash.to_le_bytes()
}

/// Legacy hash: copies first 16 bytes of skill name (zero-padded). Used for
/// backward-compatible lookups of attestations written before the FNV-1a upgrade.
fn skill_name_hash_legacy(skill_name: &[u8]) -> [u8; 16] {
    let mut hash = [0u8; 16];
    for (i, &b) in skill_name.iter().enumerate() {
        if i >= 16 { break; }
        hash[i] = b;
    }
    hash
}

fn hex_encode_16(bytes: &[u8; 16]) -> [u8; 32] {
    let hex_chars: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 32];
    for i in 0..16 {
        out[i * 2] = hex_chars[(bytes[i] >> 4) as usize];
        out[i * 2 + 1] = hex_chars[(bytes[i] & 0x0f) as usize];
    }
    out
}

/// Storage key for an attestation: "attest_{identity_hex}_{skill_hash_hex}_{attester_hex}"
fn attestation_key(identity: &[u8], skill_hash: &[u8; 16], attester: &[u8]) -> Vec<u8> {
    let id_hex = hex_encode_addr(identity);
    let skill_hex = hex_encode_16(skill_hash);
    let att_hex = hex_encode_addr(attester);
    let mut key = Vec::with_capacity(7 + 64 + 1 + 32 + 1 + 64);
    key.extend_from_slice(b"attest_");
    key.extend_from_slice(&id_hex);
    key.push(b'_');
    key.extend_from_slice(&skill_hex);
    key.push(b'_');
    key.extend_from_slice(&att_hex);
    key
}

/// Storage key for attestation count: "attest_count_{identity_hex}_{skill_hash_hex}"
fn attestation_count_key(identity: &[u8], skill_hash: &[u8; 16]) -> Vec<u8> {
    let id_hex = hex_encode_addr(identity);
    let skill_hex = hex_encode_16(skill_hash);
    let mut key = Vec::with_capacity(13 + 64 + 1 + 32);
    key.extend_from_slice(b"attest_count_");
    key.extend_from_slice(&id_hex);
    key.push(b'_');
    key.extend_from_slice(&skill_hex);
    key
}

// ============================================================================
// TOKEN / SELF-ADDRESS STORAGE FOR ESCROW OPERATIONS
// ============================================================================

const MID_TOKEN_ADDR_KEY: &[u8] = b"mid_token_addr";
const MID_SELF_ADDR_KEY: &[u8] = b"mid_self_addr";

fn get_mid_token_address() -> Option<Address> {
    storage_get(MID_TOKEN_ADDR_KEY).and_then(|d| {
        if d.len() == 32 {
            let mut addr = [0u8; 32];
            addr.copy_from_slice(&d);
            Some(Address(addr))
        } else {
            None
        }
    })
}

fn get_mid_self_address() -> Option<Address> {
    storage_get(MID_SELF_ADDR_KEY).and_then(|d| {
        if d.len() == 32 {
            let mut addr = [0u8; 32];
            addr.copy_from_slice(&d);
            Some(Address(addr))
        } else {
            None
        }
    })
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
    if !reentrancy_enter() { return 100; }
    log_info("Attesting skill...");

    let mut attester = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(attester_ptr, attester.as_mut_ptr(), 32); }
    let mut identity = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(identity_ptr, identity.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != attester {
        reentrancy_exit();
        return 200;
    }

    let skill_name_len = skill_name_len as usize;

    if skill_name_len == 0 || skill_name_len > MAX_SKILL_LEN {
        log_info("Invalid skill name length");
        reentrancy_exit();
        return 1;
    }

    if attestation_level == 0 || attestation_level > 5 {
        log_info("Attestation level must be 1-5");
        reentrancy_exit();
        return 2;
    }

    // Can't attest your own skills
    if attester[..] == identity[..] {
        log_info("Cannot attest your own skills");
        reentrancy_exit();
        return 3;
    }

    let mut skill_name = alloc::vec![0u8; skill_name_len];
    unsafe { core::ptr::copy_nonoverlapping(skill_name_ptr, skill_name.as_mut_ptr(), skill_name_len); }

    // Both must have identities
    let id_key = identity_key(&identity);
    if storage_get(&id_key).is_none() {
        log_info("Target identity not found");
        reentrancy_exit();
        return 4;
    }

    let attester_id_key = identity_key(&attester);
    if storage_get(&attester_id_key).is_none() {
        log_info("Attester identity not found");
        reentrancy_exit();
        return 5;
    }

    let s_hash = skill_name_hash(&skill_name);
    let s_hash_legacy = skill_name_hash_legacy(&skill_name);
    let ak = attestation_key(&identity, &s_hash, &attester);
    let ak_legacy = attestation_key(&identity, &s_hash_legacy, &attester);

    // Check both new hash and legacy hash to prevent duplicate attestation
    if storage_get(&ak).is_some() || storage_get(&ak_legacy).is_some() {
        log_info("Already attested this skill for this identity");
        reentrancy_exit();
        return 6;
    }

    // Store attestation: level (1 byte) + timestamp (8 bytes)
    let mut att_data = Vec::with_capacity(9);
    att_data.push(attestation_level);
    att_data.extend_from_slice(&u64_to_bytes(get_timestamp()));
    storage_set(&ak, &att_data);

    // Increment attestation count (new hash key).
    // Also migrate any legacy count forward on first write under new hash.
    let ck = attestation_count_key(&identity, &s_hash);
    let ck_legacy = attestation_count_key(&identity, &s_hash_legacy);
    let mut count = storage_get(&ck)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    // If this is the first attestation under the new hash and there's a legacy
    // count, absorb it so future reads see the combined total.
    if count == 0 {
        if let Some(legacy_count_data) = storage_get(&ck_legacy) {
            let legacy_count = bytes_to_u64(&legacy_count_data);
            if legacy_count > 0 {
                count = legacy_count;
            }
        }
    }
    storage_set(&ck, &u64_to_bytes(count + 1));

    log_info("Skill attestation recorded");
    reentrancy_exit();
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
    let mut identity = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(identity_ptr, identity.as_mut_ptr(), 32); }
    let skill_name_len = skill_name_len as usize;

    if skill_name_len == 0 || skill_name_len > MAX_SKILL_LEN {
        log_info("Invalid skill name length");
        return 1;
    }

    let mut skill_name = alloc::vec![0u8; skill_name_len];
    unsafe { core::ptr::copy_nonoverlapping(skill_name_ptr, skill_name.as_mut_ptr(), skill_name_len); }
    let s_hash = skill_name_hash(&skill_name);
    let ck = attestation_count_key(&identity, &s_hash);

    let mut count = storage_get(&ck)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);

    // Dual-lookup: if no count under new FNV hash, check legacy truncated hash
    if count == 0 {
        let s_hash_legacy = skill_name_hash_legacy(&skill_name);
        let ck_legacy = attestation_count_key(&identity, &s_hash_legacy);
        count = storage_get(&ck_legacy)
            .map(|d| bytes_to_u64(&d))
            .unwrap_or(0);
    }

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
    if !reentrancy_enter() { return 100; }
    log_info("Revoking attestation...");

    let mut attester = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(attester_ptr, attester.as_mut_ptr(), 32); }
    let mut identity = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(identity_ptr, identity.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify attester matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != attester {
        log_info("revoke_attestation: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    let skill_name_len = skill_name_len as usize;

    if skill_name_len == 0 || skill_name_len > MAX_SKILL_LEN {
        log_info("Invalid skill name length");
        reentrancy_exit();
        return 1;
    }

    let mut skill_name = alloc::vec![0u8; skill_name_len];
    unsafe { core::ptr::copy_nonoverlapping(skill_name_ptr, skill_name.as_mut_ptr(), skill_name_len); }

    // Dual-lookup: try new FNV hash first, then legacy truncated hash
    let s_hash = skill_name_hash(&skill_name);
    let s_hash_legacy = skill_name_hash_legacy(&skill_name);
    let ak = attestation_key(&identity, &s_hash, &attester);
    let ak_legacy = attestation_key(&identity, &s_hash_legacy, &attester);

    // Determine which key holds the attestation (prefer new, fallback legacy)
    let (active_ak, active_ck) = if storage_get(&ak).is_some() {
        (ak, attestation_count_key(&identity, &s_hash))
    } else if storage_get(&ak_legacy).is_some() {
        (ak_legacy, attestation_count_key(&identity, &s_hash_legacy))
    } else {
        log_info("No attestation found to revoke");
        reentrancy_exit();
        return 2;
    };

    // Remove attestation
    moltchain_sdk::storage::remove(&active_ak);

    // Decrement count
    let count = storage_get(&active_ck)
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    if count > 0 {
        storage_set(&active_ck, &u64_to_bytes(count - 1));
    }

    log_info("Attestation revoked");
    reentrancy_exit();
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
    if !reentrancy_enter() { return 100; }
    log_info("Registering .molt name...");

    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        log_info("register_name: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    let name_len = name_len as usize;
    let mut name = alloc::vec![0u8; name_len];
    unsafe { core::ptr::copy_nonoverlapping(name_ptr, name.as_mut_ptr(), name_len); }

    // Must have a MoltyID
    let id_key = identity_key(&caller);
    if storage_get(&id_key).is_none() {
        log_info("Must register MoltyID first");
        reentrancy_exit();
        return 1;
    }

    // Validate name
    if !validate_molt_name(&name) {
        log_info("Invalid .molt name (3-32 chars, a-z 0-9 hyphens, no leading/trailing hyphens)");
        reentrancy_exit();
        return 2;
    }

    // Check reserved
    if is_reserved_name(&name) {
        log_info("Name is reserved");
        reentrancy_exit();
        return 3;
    }

    // Premium short names must be sold via auction
    if is_premium_name(&name) {
        log_info("Premium short names are auction-only");
        reentrancy_exit();
        return 8;
    }

    // Duration: 1-10 years
    if duration_years == 0 || duration_years > 10 {
        log_info("Duration must be 1-10 years");
        reentrancy_exit();
        return 4;
    }

    // Check if name is already taken and not expired
    let nk = name_key(&name);
    if let Some(existing) = storage_get(&nk) {
        if existing.len() >= 48 {
            let expiry = bytes_to_u64(&existing[40..48]);
            let current_slot = moltchain_sdk::get_slot();
            if current_slot < expiry {
                log_info("Name already registered and not expired");
                reentrancy_exit();
                return 5;
            }
            // Name expired — can be re-registered (clear old reverse mapping)
            let old_owner = &existing[0..32];
            let old_rev = name_reverse_key(old_owner);
            moltchain_sdk::storage::remove(&old_rev);
        }
    }

    // Check caller doesn't already have a name (one name per identity)
    let rev_key = name_reverse_key(&caller);
    if let Some(existing_name) = storage_get(&rev_key) {
        // Check if the existing name is still valid
        let existing_nk = name_key(&existing_name);
        if let Some(nr) = storage_get(&existing_nk) {
            if nr.len() >= 48 {
                let expiry = bytes_to_u64(&nr[40..48]);
                if moltchain_sdk::get_slot() < expiry {
                    log_info("Already have a .molt name; release it first");
                    reentrancy_exit();
                    return 6;
                }
            }
        }
    }

    // Check payment (via get_value() — the MOLT tokens sent with this transaction)
    let required_cost = name_registration_cost(name_len) * (duration_years as u64);
    let paid = moltchain_sdk::get_value();
    if paid < required_cost {
        log_info("Insufficient payment for name registration");
        reentrancy_exit();
        return 7;
    }

    // Register the name
    let current_slot = moltchain_sdk::get_slot();
    let expiry_slot = current_slot + (SLOTS_PER_YEAR * duration_years as u64);

    let mut record = [0u8; 48];
    record[0..32].copy_from_slice(&caller);
    record[32..40].copy_from_slice(&u64_to_bytes(current_slot));
    record[40..48].copy_from_slice(&u64_to_bytes(expiry_slot));

    storage_set(&nk, &record);

    // Set reverse mapping: address → name
    storage_set(&rev_key, &name);

    // Increment name count
    let count = storage_get(b"molt_name_count")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    storage_set(b"molt_name_count", &u64_to_bytes(count + 1));

    // Award reputation for name registration (+25 rep) and check achievements
    let id_key = identity_key(&caller);
    if let Some(mut id_record) = storage_get(&id_key) {
        if id_record.len() >= IDENTITY_SIZE {
            let old_rep = bytes_to_u64(&id_record[99..107]);
            let new_rep = core::cmp::min(old_rep.saturating_add(25), MAX_REPUTATION);
            id_record[99..107].copy_from_slice(&u64_to_bytes(new_rep));
            id_record[115..123].copy_from_slice(&u64_to_bytes(get_timestamp()));
            storage_set(&id_key, &id_record);
            storage_set(&reputation_key(&caller), &u64_to_bytes(new_rep));
            // Award "First Name" and "Name Registrar" achievements
            let hex = hex_encode_addr(&caller);
            award_achievement(&caller, &hex, ACHIEVEMENT_FIRST_NAME, "First .molt Name");
            award_achievement(&caller, &hex, ACHIEVEMENT_NAME_REGISTRAR, "Name Registrar");
            check_achievements(&caller, new_rep);
        }
    }

    log_info(".molt name registered!");
    reentrancy_exit();
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
    let mut name = alloc::vec![0u8; name_len];
    unsafe { core::ptr::copy_nonoverlapping(name_ptr, name.as_mut_ptr(), name_len); }

    let nk = name_key(&name);
    match storage_get(&nk) {
        Some(data) if data.len() >= 48 => {
            let expiry = bytes_to_u64(&data[40..48]);
            let current_slot = moltchain_sdk::get_slot();
            if current_slot >= expiry {
                log_info("Name expired");
                return 1;
            }
            moltchain_sdk::set_return_data(&data);
            0
        }
        _ => {
            log_info("Name not found");
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
    let mut addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(addr_ptr, addr.as_mut_ptr(), 32); }

    let rev_key = name_reverse_key(&addr);
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
            log_info("Name expired or invalid");
            1
        }
        None => {
            log_info("No .molt name for this address");
            1
        }
    }
}

// ============================================================================
// PREMIUM NAME AUCTION
// ============================================================================

/// Create an auction for a premium short name (3-4 chars).
/// Auction record: [active(1), start_slot(8), end_slot(8), reserve_bid(8), highest_bid(8), highest_bidder(32)]
#[no_mangle]
pub extern "C" fn create_name_auction(
    caller_ptr: *const u8,
    name_ptr: *const u8,
    name_len: u32,
    reserve_bid: u64,
    end_slot: u64,
) -> u32 {
    if !reentrancy_enter() { return 100; }
    if is_mid_paused() {
        reentrancy_exit();
        return 20;
    }

    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        log_info("create_name_auction: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    if !is_mid_admin(&caller) {
        log_info("Unauthorized: only admin can create name auction");
        reentrancy_exit();
        return 1;
    }

    let name_len = name_len as usize;
    let mut name = alloc::vec![0u8; name_len];
    unsafe { core::ptr::copy_nonoverlapping(name_ptr, name.as_mut_ptr(), name_len); }

    if !validate_molt_name(&name) {
        log_info("Invalid .molt name for auction");
        reentrancy_exit();
        return 2;
    }

    if !is_premium_name(&name) {
        log_info("Only premium short names can be auctioned");
        reentrancy_exit();
        return 3;
    }

    if is_reserved_name(&name) {
        log_info("Reserved name cannot be auctioned");
        reentrancy_exit();
        return 4;
    }

    let nk = name_key(&name);
    if let Some(existing) = storage_get(&nk) {
        if existing.len() >= 48 {
            let expiry = bytes_to_u64(&existing[40..48]);
            if moltchain_sdk::get_slot() < expiry {
                log_info("Name already registered");
                reentrancy_exit();
                return 5;
            }
        }
    }

    let now_slot = moltchain_sdk::get_slot();
    if end_slot <= now_slot {
        log_info("Auction end slot must be in the future");
        reentrancy_exit();
        return 6;
    }

    let duration = end_slot - now_slot;
    if !(NAME_AUCTION_MIN_SLOTS..=NAME_AUCTION_MAX_SLOTS).contains(&duration) {
        log_info("Auction duration out of bounds");
        reentrancy_exit();
        return 7;
    }

    let ak = name_auction_key(&name);
    if let Some(existing) = storage_get(&ak) {
        if existing.len() >= 65 && existing[0] == 1 {
            let existing_end = bytes_to_u64(&existing[9..17]);
            if now_slot < existing_end {
                log_info("Auction already active for this name");
                reentrancy_exit();
                return 8;
            }
        }
    }

    let mut record = Vec::with_capacity(65);
    record.push(1); // active
    record.extend_from_slice(&u64_to_bytes(now_slot));
    record.extend_from_slice(&u64_to_bytes(end_slot));
    record.extend_from_slice(&u64_to_bytes(reserve_bid));
    record.extend_from_slice(&u64_to_bytes(0)); // highest_bid
    record.extend_from_slice(&[0u8; 32]); // highest_bidder
    storage_set(&ak, &record);

    log_info("Name auction created");
    reentrancy_exit();
    0
}

/// Place a bid on a premium-name auction.
#[no_mangle]
pub extern "C" fn bid_name_auction(
    bidder_ptr: *const u8,
    name_ptr: *const u8,
    name_len: u32,
    bid_amount: u64,
) -> u32 {
    if !reentrancy_enter() { return 100; }

    if is_mid_paused() {
        reentrancy_exit();
        return 20;
    }

    let mut bidder = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(bidder_ptr, bidder.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify bidder matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != bidder {
        log_info("bid_name_auction: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    let name_len = name_len as usize;
    let mut name = alloc::vec![0u8; name_len];
    unsafe { core::ptr::copy_nonoverlapping(name_ptr, name.as_mut_ptr(), name_len); }

    if storage_get(&identity_key(&bidder)).is_none() {
        log_info("Bidder must have a MoltyID");
        reentrancy_exit();
        return 1;
    }

    let ak = name_auction_key(&name);
    let mut record = match storage_get(&ak) {
        Some(data) if data.len() >= 65 => data,
        _ => {
            log_info("Auction not found");
            reentrancy_exit();
            return 2;
        }
    };

    if record[0] != 1 {
        log_info("Auction not active");
        reentrancy_exit();
        return 3;
    }

    let now_slot = moltchain_sdk::get_slot();
    let end_slot = bytes_to_u64(&record[9..17]);
    if now_slot >= end_slot {
        log_info("Auction ended");
        reentrancy_exit();
        return 4;
    }

    let reserve_bid = bytes_to_u64(&record[17..25]);
    let current_highest = bytes_to_u64(&record[25..33]);
    if bid_amount < reserve_bid || bid_amount <= current_highest {
        log_info("Bid too low");
        reentrancy_exit();
        return 5;
    }

    let paid = moltchain_sdk::get_value();
    if paid < bid_amount {
        log_info("Insufficient payment for bid");
        reentrancy_exit();
        return 6;
    }

    // AUDIT-FIX G18-02: Checks-Effects-Interactions pattern
    // Update state BEFORE external call to prevent reentrancy exploitation.
    let prev_bid_amount = bytes_to_u64(&record[25..33]);
    let mut prev_bidder = [0u8; 32];
    prev_bidder.copy_from_slice(&record[33..65]);

    // EFFECTS: Update auction record with new bid before any external call
    record[25..33].copy_from_slice(&u64_to_bytes(bid_amount));
    record[33..65].copy_from_slice(&bidder);
    storage_set(&ak, &record);

    // INTERACTIONS: Refund previous highest bidder after state is updated
    if prev_bid_amount > 0 && !prev_bidder.iter().all(|&b| b == 0) {
        let token_addr = match get_mid_token_address() {
            Some(a) => a,
            None => {
                log_info("bid_name_auction: token address not configured");
                reentrancy_exit();
                return 30;
            }
        };
        let self_addr = match get_mid_self_address() {
            Some(a) => a,
            None => {
                log_info("bid_name_auction: self address not configured");
                reentrancy_exit();
                return 31;
            }
        };
        match call_token_transfer(
            token_addr,
            self_addr,
            Address(prev_bidder),
            prev_bid_amount,
        ) {
            Err(_) => {
                log_info("bid_name_auction: refund transfer failed");
                reentrancy_exit();
                return 32;
            }
            Ok(_) => {
                log_info("Previous highest bidder refunded");
            }
        }
    }

    log_info("Auction bid accepted");
    reentrancy_exit();
    0
}

/// Finalize a premium-name auction and register the name to the winner.
#[no_mangle]
pub extern "C" fn finalize_name_auction(
    caller_ptr: *const u8,
    name_ptr: *const u8,
    name_len: u32,
    duration_years: u8,
) -> u32 {
    if !reentrancy_enter() { return 100; }
    let mut _caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, _caller.as_mut_ptr(), 32); }
    let name_len = name_len as usize;
    let mut name = alloc::vec![0u8; name_len];
    unsafe { core::ptr::copy_nonoverlapping(name_ptr, name.as_mut_ptr(), name_len); }

    if duration_years == 0 || duration_years > 10 {
        log_info("Duration must be 1-10 years");
        reentrancy_exit();
        return 1;
    }

    let ak = name_auction_key(&name);
    let mut auction = match storage_get(&ak) {
        Some(data) if data.len() >= 65 => data,
        _ => {
            log_info("Auction not found");
            reentrancy_exit();
            return 2;
        }
    };

    if auction[0] != 1 {
        log_info("Auction not active");
        reentrancy_exit();
        return 3;
    }

    let now_slot = moltchain_sdk::get_slot();
    let end_slot = bytes_to_u64(&auction[9..17]);
    if now_slot < end_slot {
        log_info("Auction still active");
        reentrancy_exit();
        return 4;
    }

    let highest_bid = bytes_to_u64(&auction[25..33]);
    if highest_bid == 0 {
        log_info("Auction has no bids");
        reentrancy_exit();
        return 5;
    }

    let winner = &auction[33..65];
    if storage_get(&identity_key(winner)).is_none() {
        log_info("Winner identity not found");
        reentrancy_exit();
        return 6;
    }

    // Enforce one-name-per-identity
    let winner_rev = name_reverse_key(winner);
    if let Some(existing_name) = storage_get(&winner_rev) {
        let existing_nk = name_key(&existing_name);
        if let Some(nr) = storage_get(&existing_nk) {
            if nr.len() >= 48 {
                let expiry = bytes_to_u64(&nr[40..48]);
                if now_slot < expiry {
                    log_info("Winner already has an active .molt name");
                    reentrancy_exit();
                    return 7;
                }
            }
        }
    }

    // Name must not be active now
    let nk = name_key(&name);
    if let Some(existing) = storage_get(&nk) {
        if existing.len() >= 48 {
            let expiry = bytes_to_u64(&existing[40..48]);
            if now_slot < expiry {
                log_info("Name already active");
                reentrancy_exit();
                return 8;
            }
        }
    }

    let expiry_slot = now_slot + (SLOTS_PER_YEAR * duration_years as u64);
    let mut name_record = [0u8; 48];
    name_record[0..32].copy_from_slice(winner);
    name_record[32..40].copy_from_slice(&u64_to_bytes(now_slot));
    name_record[40..48].copy_from_slice(&u64_to_bytes(expiry_slot));
    storage_set(&nk, &name_record);
    storage_set(&winner_rev, &name);

    // Increment name count
    let count = storage_get(b"molt_name_count").map(|d| bytes_to_u64(&d)).unwrap_or(0);
    storage_set(b"molt_name_count", &u64_to_bytes(count + 1));

    auction[0] = 0; // inactive
    storage_set(&ak, &auction);

    log_info("Name auction finalized");
    reentrancy_exit();
    0
}

/// Get raw auction record for a name.
#[no_mangle]
pub extern "C" fn get_name_auction(name_ptr: *const u8, name_len: u32) -> u32 {
    let name_len = name_len as usize;
    let mut name = alloc::vec![0u8; name_len];
    unsafe { core::ptr::copy_nonoverlapping(name_ptr, name.as_mut_ptr(), name_len); }
    let ak = name_auction_key(&name);
    match storage_get(&ak) {
        Some(data) if data.len() >= 65 => {
            moltchain_sdk::set_return_data(&data);
            0
        }
        _ => {
            log_info("Auction not found");
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
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        log_info("transfer_name: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    let name_len = name_len as usize;
    let mut name = alloc::vec![0u8; name_len];
    unsafe { core::ptr::copy_nonoverlapping(name_ptr, name.as_mut_ptr(), name_len); }
    let mut new_owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(new_owner_ptr, new_owner.as_mut_ptr(), 32); }

    // Look up the name record
    let nk = name_key(&name);
    let mut record = match storage_get(&nk) {
        Some(data) if data.len() >= 48 => data,
        _ => {
            log_info("Name not found");
            reentrancy_exit();
            return 1;
        }
    };

    // Verify caller is current owner
    if record[0..32] != caller[..] {
        log_info("Not the owner of this name");
        reentrancy_exit();
        return 2;
    }

    // Check name is not expired
    let expiry = bytes_to_u64(&record[40..48]);
    if moltchain_sdk::get_slot() >= expiry {
        log_info("Name has expired");
        reentrancy_exit();
        return 3;
    }

    // New owner must have a MoltyID
    let new_owner_id = identity_key(&new_owner);
    if storage_get(&new_owner_id).is_none() {
        log_info("New owner must have a MoltyID");
        reentrancy_exit();
        return 4;
    }

    // New owner must not already have a name
    let new_rev = name_reverse_key(&new_owner);
    if let Some(existing_name) = storage_get(&new_rev) {
        let existing_nk = name_key(&existing_name);
        if let Some(nr) = storage_get(&existing_nk) {
            if nr.len() >= 48 {
                let ex = bytes_to_u64(&nr[40..48]);
                if moltchain_sdk::get_slot() < ex {
                    log_info("New owner already has a .molt name");
                    reentrancy_exit();
                    return 5;
                }
            }
        }
    }

    // Update name record with new owner
    record[0..32].copy_from_slice(&new_owner);
    storage_set(&nk, &record);

    // Update reverse mappings
    let old_rev = name_reverse_key(&caller);
    moltchain_sdk::storage::remove(&old_rev);
    storage_set(&new_rev, &name);

    log_info(".molt name transferred");
    reentrancy_exit();
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
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        log_info("renew_name: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    let name_len = name_len as usize;
    let mut name = alloc::vec![0u8; name_len];
    unsafe { core::ptr::copy_nonoverlapping(name_ptr, name.as_mut_ptr(), name_len); }

    if additional_years == 0 || additional_years > 10 {
        log_info("Additional years must be 1-10");
        reentrancy_exit();
        return 1;
    }

    let nk = name_key(&name);
    let mut record = match storage_get(&nk) {
        Some(data) if data.len() >= 48 => data,
        _ => {
            log_info("Name not found");
            reentrancy_exit();
            return 2;
        }
    };

    // Must be owner
    if record[0..32] != caller[..] {
        log_info("Not the owner of this name");
        reentrancy_exit();
        return 3;
    }

    // Check payment
    let required_cost = name_registration_cost(name_len) * (additional_years as u64);
    let paid = moltchain_sdk::get_value();
    if paid < required_cost {
        log_info("Insufficient payment for renewal");
        reentrancy_exit();
        return 4;
    }

    // Extend expiry from current expiry (or from now if expired)
    let current_expiry = bytes_to_u64(&record[40..48]);
    let current_slot = moltchain_sdk::get_slot();
    let base = if current_slot > current_expiry { current_slot } else { current_expiry };
    let new_expiry = base + (SLOTS_PER_YEAR * additional_years as u64);

    record[40..48].copy_from_slice(&u64_to_bytes(new_expiry));
    storage_set(&nk, &record);

    log_info(".molt name renewed");
    reentrancy_exit();
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
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        log_info("release_name: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    let name_len = name_len as usize;
    let mut name = alloc::vec![0u8; name_len];
    unsafe { core::ptr::copy_nonoverlapping(name_ptr, name.as_mut_ptr(), name_len); }

    let nk = name_key(&name);
    let record = match storage_get(&nk) {
        Some(data) if data.len() >= 48 => data,
        _ => {
            log_info("Name not found");
            reentrancy_exit();
            return 1;
        }
    };

    // Must be owner
    if record[0..32] != caller[..] {
        log_info("Not the owner of this name");
        reentrancy_exit();
        return 2;
    }

    // Remove forward mapping
    moltchain_sdk::storage::remove(&nk);

    // Remove reverse mapping
    let rev_key = name_reverse_key(&caller);
    moltchain_sdk::storage::remove(&rev_key);

    // Decrement name count
    let count = storage_get(b"molt_name_count")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    if count > 0 {
        storage_set(b"molt_name_count", &u64_to_bytes(count - 1));
    }

    log_info(".molt name released");
    reentrancy_exit();
    0
}

/// Delegated transfer of a .molt name.
#[no_mangle]
pub extern "C" fn transfer_name_as(
    delegate_ptr: *const u8,
    owner_ptr: *const u8,
    name_ptr: *const u8,
    name_len: u32,
    new_owner_ptr: *const u8,
) -> u32 {
    if !reentrancy_enter() { return 100; }
    let mut delegate = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(delegate_ptr, delegate.as_mut_ptr(), 32); }
    let mut owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify delegate matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != delegate {
        log_info("transfer_name_as: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    let name_len = name_len as usize;
    let mut name = alloc::vec![0u8; name_len];
    unsafe { core::ptr::copy_nonoverlapping(name_ptr, name.as_mut_ptr(), name_len); }
    let mut new_owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(new_owner_ptr, new_owner.as_mut_ptr(), 32); }

    let now = get_timestamp();
    if !has_active_permission(&owner, &delegate, DELEGATE_PERM_NAMING, now) {
        log_info("Unauthorized delegate for name transfer");
        reentrancy_exit();
        return 1;
    }

    let nk = name_key(&name);
    let mut record = match storage_get(&nk) {
        Some(data) if data.len() >= 48 => data,
        _ => {
            log_info("Name not found");
            reentrancy_exit();
            return 2;
        }
    };

    if record[0..32] != owner[..] {
        log_info("Not the owner of this name");
        reentrancy_exit();
        return 3;
    }

    let expiry = bytes_to_u64(&record[40..48]);
    if moltchain_sdk::get_slot() >= expiry {
        log_info("Name has expired");
        reentrancy_exit();
        return 4;
    }

    let new_owner_id = identity_key(&new_owner);
    if storage_get(&new_owner_id).is_none() {
        log_info("New owner must have a MoltyID");
        reentrancy_exit();
        return 5;
    }

    let new_rev = name_reverse_key(&new_owner);
    if let Some(existing_name) = storage_get(&new_rev) {
        let existing_nk = name_key(&existing_name);
        if let Some(nr) = storage_get(&existing_nk) {
            if nr.len() >= 48 {
                let ex = bytes_to_u64(&nr[40..48]);
                if moltchain_sdk::get_slot() < ex {
                    log_info("New owner already has a .molt name");
                    reentrancy_exit();
                    return 6;
                }
            }
        }
    }

    record[0..32].copy_from_slice(&new_owner);
    storage_set(&nk, &record);

    let old_rev = name_reverse_key(&owner);
    moltchain_sdk::storage::remove(&old_rev);
    storage_set(&new_rev, &name);

    log_info("Delegated .molt name transferred");
    reentrancy_exit();
    0
}

/// Delegated renewal of a .molt name.
#[no_mangle]
pub extern "C" fn renew_name_as(
    delegate_ptr: *const u8,
    owner_ptr: *const u8,
    name_ptr: *const u8,
    name_len: u32,
    additional_years: u8,
) -> u32 {
    if !reentrancy_enter() { return 100; }
    let mut delegate = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(delegate_ptr, delegate.as_mut_ptr(), 32); }
    let mut owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify delegate matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != delegate {
        log_info("renew_name_as: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    let name_len = name_len as usize;
    let mut name = alloc::vec![0u8; name_len];
    unsafe { core::ptr::copy_nonoverlapping(name_ptr, name.as_mut_ptr(), name_len); }

    if additional_years == 0 || additional_years > 10 {
        log_info("Additional years must be 1-10");
        reentrancy_exit();
        return 1;
    }

    let now = get_timestamp();
    if !has_active_permission(&owner, &delegate, DELEGATE_PERM_NAMING, now) {
        log_info("Unauthorized delegate for name renewal");
        reentrancy_exit();
        return 2;
    }

    let nk = name_key(&name);
    let mut record = match storage_get(&nk) {
        Some(data) if data.len() >= 48 => data,
        _ => {
            log_info("Name not found");
            reentrancy_exit();
            return 3;
        }
    };

    if record[0..32] != owner[..] {
        log_info("Not the owner of this name");
        reentrancy_exit();
        return 4;
    }

    let required_cost = name_registration_cost(name_len) * (additional_years as u64);
    let paid = moltchain_sdk::get_value();
    if paid < required_cost {
        log_info("Insufficient payment for renewal");
        reentrancy_exit();
        return 5;
    }

    let current_expiry = bytes_to_u64(&record[40..48]);
    let current_slot = moltchain_sdk::get_slot();
    let base = if current_slot > current_expiry { current_slot } else { current_expiry };
    let new_expiry = base + (SLOTS_PER_YEAR * additional_years as u64);

    record[40..48].copy_from_slice(&u64_to_bytes(new_expiry));
    storage_set(&nk, &record);

    log_info("Delegated .molt name renewed");
    reentrancy_exit();
    0
}

/// Delegated release of a .molt name.
#[no_mangle]
pub extern "C" fn release_name_as(
    delegate_ptr: *const u8,
    owner_ptr: *const u8,
    name_ptr: *const u8,
    name_len: u32,
) -> u32 {
    if !reentrancy_enter() { return 100; }
    let mut delegate = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(delegate_ptr, delegate.as_mut_ptr(), 32); }
    let mut owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify delegate matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != delegate {
        log_info("release_name_as: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    let name_len = name_len as usize;
    let mut name = alloc::vec![0u8; name_len];
    unsafe { core::ptr::copy_nonoverlapping(name_ptr, name.as_mut_ptr(), name_len); }

    let now = get_timestamp();
    if !has_active_permission(&owner, &delegate, DELEGATE_PERM_NAMING, now) {
        log_info("Unauthorized delegate for name release");
        reentrancy_exit();
        return 1;
    }

    let nk = name_key(&name);
    let record = match storage_get(&nk) {
        Some(data) if data.len() >= 48 => data,
        _ => {
            log_info("Name not found");
            reentrancy_exit();
            return 2;
        }
    };

    if record[0..32] != owner[..] {
        log_info("Not the owner of this name");
        reentrancy_exit();
        return 3;
    }

    moltchain_sdk::storage::remove(&nk);
    let rev_key = name_reverse_key(&owner);
    moltchain_sdk::storage::remove(&rev_key);

    let count = storage_get(b"molt_name_count").map(|d| bytes_to_u64(&d)).unwrap_or(0);
    if count > 0 {
        storage_set(b"molt_name_count", &u64_to_bytes(count - 1));
    }

    log_info("Delegated .molt name released");
    reentrancy_exit();
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
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer (same as register_identity)
    let real_caller = get_caller();
    if real_caller.0 != caller {
        log_info("set_endpoint: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    let url_len = url_len as usize;

    if url_len == 0 || url_len > MAX_ENDPOINT_LEN {
        log_info("Invalid endpoint URL length");
        reentrancy_exit();
        return 1;
    }

    let mut url = alloc::vec![0u8; url_len];
    unsafe { core::ptr::copy_nonoverlapping(url_ptr, url.as_mut_ptr(), url_len); }

    // Must have identity
    let idk = identity_key(&caller);
    if storage_get(&idk).is_none() {
        log_info("Identity not found — register first");
        reentrancy_exit();
        return 2;
    }

    let ek = endpoint_key(&caller);
    storage_set(&ek, &url);

    log_info("Endpoint set");
    reentrancy_exit();
    0
}

/// Get the endpoint URL for an address.
///
/// Parameters:
///   - addr_ptr: 32-byte address
#[no_mangle]
pub extern "C" fn get_endpoint(addr_ptr: *const u8) -> u32 {
    let mut addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(addr_ptr, addr.as_mut_ptr(), 32); }

    let ek = endpoint_key(&addr);
    match storage_get(&ek) {
        Some(data) => {
            moltchain_sdk::set_return_data(&data);
            0
        }
        None => {
            log_info("No endpoint set");
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
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        log_info("set_metadata: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    let json_len = json_len as usize;

    if json_len == 0 || json_len > MAX_METADATA_LEN {
        log_info("Invalid metadata length");
        reentrancy_exit();
        return 1;
    }

    let mut json = alloc::vec![0u8; json_len];
    unsafe { core::ptr::copy_nonoverlapping(json_ptr, json.as_mut_ptr(), json_len); }

    // Must have identity
    let idk = identity_key(&caller);
    if storage_get(&idk).is_none() {
        log_info("Identity not found — register first");
        reentrancy_exit();
        return 2;
    }

    let mk = metadata_key(&caller);
    storage_set(&mk, &json);

    log_info("Metadata set");
    reentrancy_exit();
    0
}

/// Get metadata for an address.
///
/// Parameters:
///   - addr_ptr: 32-byte address
#[no_mangle]
pub extern "C" fn get_metadata(addr_ptr: *const u8) -> u32 {
    let mut addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(addr_ptr, addr.as_mut_ptr(), 32); }

    let mk = metadata_key(&addr);
    match storage_get(&mk) {
        Some(data) => {
            moltchain_sdk::set_return_data(&data);
            0
        }
        None => {
            log_info("No metadata set");
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
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        log_info("set_availability: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    if status > 2 {
        log_info("Invalid availability status (0=offline, 1=available, 2=busy)");
        reentrancy_exit();
        return 1;
    }

    // Must have identity
    let idk = identity_key(&caller);
    if storage_get(&idk).is_none() {
        log_info("Identity not found — register first");
        reentrancy_exit();
        return 2;
    }

    let ak = availability_key(&caller);
    storage_set(&ak, &[status]);

    log_info("Availability set");
    reentrancy_exit();
    0
}

/// Get availability status for an address.
///
/// Parameters:
///   - addr_ptr: 32-byte address
#[no_mangle]
pub extern "C" fn get_availability(addr_ptr: *const u8) -> u32 {
    let mut addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(addr_ptr, addr.as_mut_ptr(), 32); }

    let ak = availability_key(&addr);
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
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        log_info("set_rate: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    // Must have identity
    let idk = identity_key(&caller);
    if storage_get(&idk).is_none() {
        log_info("Identity not found — register first");
        reentrancy_exit();
        return 1;
    }

    let rk = rate_key(&caller);
    storage_set(&rk, &u64_to_bytes(molt_per_unit));

    log_info("Rate set");
    reentrancy_exit();
    0
}

/// Get rate for an address.
///
/// Parameters:
///   - addr_ptr: 32-byte address
#[no_mangle]
pub extern "C" fn get_rate(addr_ptr: *const u8) -> u32 {
    let mut addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(addr_ptr, addr.as_mut_ptr(), 32); }

    let rk = rate_key(&addr);
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
// IDENTITY DELEGATION
// ============================================================================

/// Set delegation permissions for a delegate.
/// delegation record format: [permissions(1), expires_at_ms(8), created_at_ms(8)]
#[no_mangle]
pub extern "C" fn set_delegate(
    owner_ptr: *const u8,
    delegate_ptr: *const u8,
    permissions: u8,
    expires_at_ms: u64,
) -> u32 {
    if !reentrancy_enter() { return 100; }
    if is_mid_paused() {
        reentrancy_exit();
        return 20;
    }

    let mut owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32); }
    let mut delegate = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(delegate_ptr, delegate.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify owner matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != owner {
        log_info("set_delegate: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    let id_owner = identity_key(&owner);
    if storage_get(&id_owner).is_none() {
        log_info("Owner identity not found");
        reentrancy_exit();
        return 1;
    }

    let id_delegate = identity_key(&delegate);
    if storage_get(&id_delegate).is_none() {
        log_info("Delegate identity not found");
        reentrancy_exit();
        return 2;
    }

    if owner[..] == delegate[..] {
        log_info("Owner cannot delegate to self");
        reentrancy_exit();
        return 3;
    }

    if permissions == 0 {
        log_info("Permissions must be non-zero");
        reentrancy_exit();
        return 4;
    }

    let allowed_mask = DELEGATE_PERM_PROFILE
        | DELEGATE_PERM_AGENT_TYPE
        | DELEGATE_PERM_SKILLS
        | DELEGATE_PERM_NAMING;
    if permissions & !allowed_mask != 0 {
        log_info("Invalid delegation permissions mask");
        reentrancy_exit();
        return 5;
    }

    let now = get_timestamp();
    if expires_at_ms <= now || expires_at_ms > now.saturating_add(MAX_DELEGATION_TTL_MS) {
        log_info("Invalid delegation expiry");
        reentrancy_exit();
        return 6;
    }

    let mut data = Vec::with_capacity(17);
    data.push(permissions);
    data.extend_from_slice(&u64_to_bytes(expires_at_ms));
    data.extend_from_slice(&u64_to_bytes(now));

    let dk = delegation_key(&owner, &delegate);
    storage_set(&dk, &data);

    log_info("Delegation set");
    reentrancy_exit();
    0
}

/// Revoke delegation for a delegate.
#[no_mangle]
pub extern "C" fn revoke_delegate(owner_ptr: *const u8, delegate_ptr: *const u8) -> u32 {
    if !reentrancy_enter() { return 100; }
    if is_mid_paused() {
        reentrancy_exit();
        return 20;
    }

    let mut owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32); }
    let mut delegate = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(delegate_ptr, delegate.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify owner matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != owner {
        log_info("revoke_delegate: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    let dk = delegation_key(&owner, &delegate);
    if storage_get(&dk).is_none() {
        log_info("Delegation not found");
        reentrancy_exit();
        return 1;
    }

    moltchain_sdk::storage::remove(&dk);
    log_info("Delegation revoked");
    reentrancy_exit();
    0
}

/// Get delegation record for owner -> delegate.
/// Returns [permissions(1), expires_at_ms(8), created_at_ms(8)].
#[no_mangle]
pub extern "C" fn get_delegate(owner_ptr: *const u8, delegate_ptr: *const u8) -> u32 {
    let mut owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32); }
    let mut delegate = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(delegate_ptr, delegate.as_mut_ptr(), 32); }

    let dk = delegation_key(&owner, &delegate);
    match storage_get(&dk) {
        Some(data) if data.len() >= 17 => {
            moltchain_sdk::set_return_data(&data);
            0
        }
        _ => {
            log_info("Delegation not found");
            1
        }
    }
}

/// Delegated endpoint update.
#[no_mangle]
pub extern "C" fn set_endpoint_as(
    delegate_ptr: *const u8,
    owner_ptr: *const u8,
    url_ptr: *const u8,
    url_len: u32,
) -> u32 {
    if !reentrancy_enter() { return 100; }
    let mut delegate = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(delegate_ptr, delegate.as_mut_ptr(), 32); }
    let mut owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify delegate matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != delegate {
        log_info("set_endpoint_as: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    let url_len = url_len as usize;

    if url_len == 0 || url_len > MAX_ENDPOINT_LEN {
        log_info("Invalid endpoint URL length");
        reentrancy_exit();
        return 1;
    }

    let now = get_timestamp();
    if !has_active_permission(&owner, &delegate, DELEGATE_PERM_PROFILE, now) {
        log_info("Unauthorized delegate for endpoint update");
        reentrancy_exit();
        return 2;
    }

    let idk = identity_key(&owner);
    if storage_get(&idk).is_none() {
        log_info("Identity not found — register first");
        reentrancy_exit();
        return 3;
    }

    let mut url = alloc::vec![0u8; url_len];
    unsafe { core::ptr::copy_nonoverlapping(url_ptr, url.as_mut_ptr(), url_len); }
    storage_set(&endpoint_key(&owner), &url);
    log_info("Delegated endpoint set");
    reentrancy_exit();
    0
}

/// Delegated metadata update.
#[no_mangle]
pub extern "C" fn set_metadata_as(
    delegate_ptr: *const u8,
    owner_ptr: *const u8,
    json_ptr: *const u8,
    json_len: u32,
) -> u32 {
    if !reentrancy_enter() { return 100; }
    let mut delegate = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(delegate_ptr, delegate.as_mut_ptr(), 32); }
    let mut owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify delegate matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != delegate {
        log_info("set_metadata_as: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    let json_len = json_len as usize;

    if json_len == 0 || json_len > MAX_METADATA_LEN {
        log_info("Invalid metadata length");
        reentrancy_exit();
        return 1;
    }

    let now = get_timestamp();
    if !has_active_permission(&owner, &delegate, DELEGATE_PERM_PROFILE, now) {
        log_info("Unauthorized delegate for metadata update");
        reentrancy_exit();
        return 2;
    }

    let idk = identity_key(&owner);
    if storage_get(&idk).is_none() {
        log_info("Identity not found — register first");
        reentrancy_exit();
        return 3;
    }

    let mut json = alloc::vec![0u8; json_len];
    unsafe { core::ptr::copy_nonoverlapping(json_ptr, json.as_mut_ptr(), json_len); }
    storage_set(&metadata_key(&owner), &json);
    log_info("Delegated metadata set");
    reentrancy_exit();
    0
}

/// Delegated availability update.
#[no_mangle]
pub extern "C" fn set_availability_as(
    delegate_ptr: *const u8,
    owner_ptr: *const u8,
    status: u8,
) -> u32 {
    if !reentrancy_enter() { return 100; }
    let mut delegate = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(delegate_ptr, delegate.as_mut_ptr(), 32); }
    let mut owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify delegate matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != delegate {
        log_info("set_availability_as: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    if status > 2 {
        log_info("Invalid availability status (0=offline, 1=available, 2=busy)");
        reentrancy_exit();
        return 1;
    }

    let now = get_timestamp();
    if !has_active_permission(&owner, &delegate, DELEGATE_PERM_PROFILE, now) {
        log_info("Unauthorized delegate for availability update");
        reentrancy_exit();
        return 2;
    }

    let idk = identity_key(&owner);
    if storage_get(&idk).is_none() {
        log_info("Identity not found — register first");
        reentrancy_exit();
        return 3;
    }

    storage_set(&availability_key(&owner), &[status]);
    log_info("Delegated availability set");
    reentrancy_exit();
    0
}

/// Delegated rate update.
#[no_mangle]
pub extern "C" fn set_rate_as(
    delegate_ptr: *const u8,
    owner_ptr: *const u8,
    molt_per_unit: u64,
) -> u32 {
    if !reentrancy_enter() { return 100; }
    let mut delegate = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(delegate_ptr, delegate.as_mut_ptr(), 32); }
    let mut owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify delegate matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != delegate {
        log_info("set_rate_as: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    let now = get_timestamp();
    if !has_active_permission(&owner, &delegate, DELEGATE_PERM_PROFILE, now) {
        log_info("Unauthorized delegate for rate update");
        reentrancy_exit();
        return 1;
    }

    let idk = identity_key(&owner);
    if storage_get(&idk).is_none() {
        log_info("Identity not found — register first");
        reentrancy_exit();
        return 2;
    }

    storage_set(&rate_key(&owner), &u64_to_bytes(molt_per_unit));
    log_info("Delegated rate set");
    reentrancy_exit();
    0
}

/// Delegated agent-type update.
#[no_mangle]
pub extern "C" fn update_agent_type_as(
    delegate_ptr: *const u8,
    owner_ptr: *const u8,
    new_agent_type: u8,
) -> u32 {
    if !reentrancy_enter() { return 100; }
    let mut delegate = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(delegate_ptr, delegate.as_mut_ptr(), 32); }
    let mut owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify delegate matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != delegate {
        log_info("update_agent_type_as: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    if !is_valid_agent_type(new_agent_type) {
        log_info("Invalid agent type");
        reentrancy_exit();
        return 1;
    }

    let now = get_timestamp();
    if !has_active_permission(&owner, &delegate, DELEGATE_PERM_AGENT_TYPE, now) {
        log_info("Unauthorized delegate for agent type update");
        reentrancy_exit();
        return 2;
    }

    let id_key = identity_key(&owner);
    let mut record = match storage_get(&id_key) {
        Some(data) => data,
        None => {
            log_info("Identity not found");
            reentrancy_exit();
            return 3;
        }
    };

    if record.len() < IDENTITY_SIZE {
        log_info("Identity record malformed");
        reentrancy_exit();
        return 4;
    }

    record[32] = new_agent_type;
    let ts_bytes = u64_to_bytes(now);
    record[115..123].copy_from_slice(&ts_bytes);
    storage_set(&id_key, &record);

    log_info("Delegated agent type updated");
    reentrancy_exit();
    0
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
    let mut addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(addr_ptr, addr.as_mut_ptr(), 32); }

    // Must have identity
    let idk = identity_key(&addr);
    let mut id_record = match storage_get(&idk) {
        Some(data) => data,
        None => {
            log_info("Identity not found");
            return 1;
        }
    };

    let now = get_timestamp();
    let rep = apply_decay_to_identity_record(&addr, &idk, &mut id_record, now);

    let mut result = Vec::with_capacity(512);

    // Identity record (pad/truncate to IDENTITY_SIZE)
    if id_record.len() >= IDENTITY_SIZE {
        result.extend_from_slice(&id_record[..IDENTITY_SIZE]);
    } else {
        result.extend_from_slice(&id_record);
        result.resize(IDENTITY_SIZE, 0);
    }

    // .molt name
    let rev_key = name_reverse_key(&addr);
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
    let ek = endpoint_key(&addr);
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
    let ak = availability_key(&addr);
    let avail = storage_get(&ak)
        .and_then(|d| if !d.is_empty() { Some(d[0]) } else { None })
        .unwrap_or(0);
    result.push(avail);

    // Rate
    let rk = rate_key(&addr);
    let rate = storage_get(&rk)
        .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
        .unwrap_or(0);
    result.extend_from_slice(&u64_to_bytes(rate));

    // Reputation
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
    let mut pubkey = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(pubkey_ptr, pubkey.as_mut_ptr(), 32); }
    let id_key = identity_key(&pubkey);
    let reputation = match storage_get(&id_key) {
        Some(mut record) => {
            let now = get_timestamp();
            apply_decay_to_identity_record(&pubkey, &id_key, &mut record, now)
        }
        None => {
            let rep_key = reputation_key(&pubkey);
            match storage_get(&rep_key) {
                Some(data) if data.len() >= 8 => bytes_to_u64(&data),
                _ => 0,
            }
        }
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
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        log_info("mid_pause: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    if !is_mid_admin(&caller) { reentrancy_exit(); return 1; }
    if is_mid_paused() { reentrancy_exit(); return 2; }
    storage_set(MID_PAUSE_KEY, &[1]);
    log_info("MoltyID paused");
    reentrancy_exit();
    0
}

/// Unpause MoltyID. Admin only.
/// Returns: 0 success, 1 not admin, 2 not paused
#[no_mangle]
pub extern "C" fn mid_unpause(caller_ptr: *const u8) -> u32 {
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        log_info("mid_unpause: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    if !is_mid_admin(&caller) { reentrancy_exit(); return 1; }
    if !is_mid_paused() { reentrancy_exit(); return 2; }
    storage_set(MID_PAUSE_KEY, &[0]);
    log_info("MoltyID unpaused");
    reentrancy_exit();
    0
}

/// Transfer admin key. Current admin only.
/// Returns: 0 success, 1 not admin
#[no_mangle]
pub extern "C" fn transfer_admin(caller_ptr: *const u8, new_admin_ptr: *const u8) -> u32 {
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        log_info("transfer_admin: caller does not match transaction signer");
        reentrancy_exit();
        return 200;
    }

    if !is_mid_admin(&caller) { reentrancy_exit(); return 1; }
    let mut new_admin = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(new_admin_ptr, new_admin.as_mut_ptr(), 32); }
    storage_set(b"mid_admin", &new_admin);
    log_info("Admin key transferred");
    reentrancy_exit();
    0
}

/// Set the MOLT token contract address for auction refunds. Admin only.
/// Returns: 0 success, 1 not admin, 2 zero address rejected
#[no_mangle]
pub extern "C" fn set_mid_token_address(caller_ptr: *const u8, token_addr_ptr: *const u8) -> u32 {
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let real_caller = get_caller();
    if real_caller.0 != caller {
        log_info("set_mid_token_address: caller mismatch");
        reentrancy_exit();
        return 200;
    }
    if !is_mid_admin(&caller) { reentrancy_exit(); return 1; }
    let mut token_addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(token_addr_ptr, token_addr.as_mut_ptr(), 32); }
    if token_addr.iter().all(|&b| b == 0) {
        log_info("set_mid_token_address: zero address rejected");
        reentrancy_exit();
        return 2;
    }
    storage_set(MID_TOKEN_ADDR_KEY, &token_addr);
    log_info("MoltyID token address set");
    reentrancy_exit();
    0
}

/// Set this contract's own address (needed as transfer source). Admin only.
/// Returns: 0 success, 1 not admin, 2 zero address rejected
#[no_mangle]
pub extern "C" fn set_mid_self_address(caller_ptr: *const u8, self_addr_ptr: *const u8) -> u32 {
    if !reentrancy_enter() { return 100; }
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let real_caller = get_caller();
    if real_caller.0 != caller {
        log_info("set_mid_self_address: caller mismatch");
        reentrancy_exit();
        return 200;
    }
    if !is_mid_admin(&caller) { reentrancy_exit(); return 1; }
    let mut self_addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(self_addr_ptr, self_addr.as_mut_ptr(), 32); }
    if self_addr.iter().all(|&b| b == 0) {
        log_info("set_mid_self_address: zero address rejected");
        reentrancy_exit();
        return 2;
    }
    storage_set(MID_SELF_ADDR_KEY, &self_addr);
    log_info("MoltyID self address set");
    reentrancy_exit();
    0
}

// ============================================================================
// GENESIS / ADMIN RESERVED NAME REGISTRATION
// ============================================================================

/// Admin-only: register a reserved .molt name for a system address.
/// This bypasses the reserved-name check, payment check, and identity requirement.
/// Used at genesis to assign names like moltchain.molt, treasury.molt, etc.
///
/// Parameters:
///   - admin_ptr: 32-byte admin address (must be current admin)
///   - owner_ptr: 32-byte owner address to assign the name to
///   - name_ptr: pointer to name bytes (lowercase, no .molt suffix)
///   - name_len: length of name
///   - agent_type: agent type for auto-created identity (0=human, 1=agent, etc.)
///
/// Returns: 0 success, 1 not admin, 2 invalid name, 3 malformed args, 5 already taken
///
/// Args buffer layout (read via get_args): [admin 32B][owner 32B][name bytes][name_len 4B LE][agent_type 1B]
#[no_mangle]
pub extern "C" fn admin_register_reserved_name() -> u32 {
    if !reentrancy_enter() { return 100; }
    // Read args from context buffer (avoids WASM ABI pointer-mapping issues)
    let args = moltchain_sdk::contract::args();

    // Minimum: 32 (admin) + 32 (owner) + 1 (min name) + 4 (name_len) + 1 (agent_type) = 70
    if args.len() < 70 {
        log_info("admin_register_reserved_name: args too short");
        reentrancy_exit();
        return 3;
    }

    let admin = &args[0..32];
    if !is_mid_admin(admin) {
        log_info("admin_register_reserved_name: not admin");
        reentrancy_exit();
        return 1;
    }

    let owner = &args[32..64];
    let agent_type = args[args.len() - 1];
    let name_len_offset = args.len() - 5;
    let name_len = u32::from_le_bytes([
        args[name_len_offset],
        args[name_len_offset + 1],
        args[name_len_offset + 2],
        args[name_len_offset + 3],
    ]) as usize;

    // Validate: 64 + name_len + 5 should equal total args length
    if 64 + name_len + 5 != args.len() {
        log_info("admin_register_reserved_name: malformed args");
        reentrancy_exit();
        return 3;
    }

    let name = &args[64..64 + name_len];

    // Validate name format (but NOT reserved check — that's the whole point)
    if !validate_molt_name(name) {
        log_info("admin_register_reserved_name: invalid name format");
        reentrancy_exit();
        return 2;
    }

    // Check not already taken
    let nk = name_key(&name);
    if let Some(existing) = storage_get(&nk) {
        if existing.len() >= 48 {
            let expiry = bytes_to_u64(&existing[40..48]);
            let current_slot = moltchain_sdk::get_slot();
            if current_slot < expiry {
                log_info("admin_register_reserved_name: name already taken");
                reentrancy_exit();
                return 5;
            }
        }
    }

    // Auto-create identity if owner doesn't have one yet
    let id_key = identity_key(owner);
    if storage_get(&id_key).is_none() {
        let now = get_timestamp();
        let mut record = [0u8; IDENTITY_SIZE];
        record[0..32].copy_from_slice(owner);
        record[32] = agent_type;
        // name_len
        let padded_name_len = if name_len > 64 { 64 } else { name_len };
        record[33] = (padded_name_len & 0xFF) as u8;
        record[34] = ((padded_name_len >> 8) & 0xFF) as u8;
        record[35..35 + padded_name_len].copy_from_slice(&name[..padded_name_len]);
        // reputation = 10,000 (Legendary tier for genesis reserved names)
        const GENESIS_RESERVED_REPUTATION: u64 = 10_000;
        let rep_bytes = u64_to_bytes(GENESIS_RESERVED_REPUTATION);
        record[99..107].copy_from_slice(&rep_bytes);
        // created_at, updated_at
        let ts_bytes = u64_to_bytes(now);
        record[107..115].copy_from_slice(&ts_bytes);
        record[115..123].copy_from_slice(&ts_bytes);

        // Add 3 default system skills: Infrastructure, Consensus, Security
        // Skill format: [name_len(1), name(up to 32), proficiency(1), timestamp(8)]
        let genesis_skills: &[&[u8]] = &[
            b"Infrastructure",
            b"Consensus",
            b"Security",
        ];
        let mut skill_idx: u8 = 0;
        for skill_name in genesis_skills {
            let mut skill_data = Vec::with_capacity(1 + skill_name.len() + 1 + 8);
            skill_data.push(skill_name.len() as u8);
            skill_data.extend_from_slice(skill_name);
            skill_data.push(100); // proficiency 100 (max)
            skill_data.extend_from_slice(&u64_to_bytes(now));
            let sk = skill_key(owner, skill_idx);
            storage_set(&sk, &skill_data);
            skill_idx += 1;
        }

        record[123] = skill_idx; // skill_count = 3
        record[124] = 0; // vouch_count low
        record[125] = 0; // vouch_count high
        record[126] = 1; // is_active

        storage_set(&id_key, &record);
        storage_set(&reputation_key(owner), &rep_bytes);

        // Increment identity count
        let count = storage_get(b"mid_identity_count")
            .map(|d| bytes_to_u64(&d))
            .unwrap_or(0);
        storage_set(b"mid_identity_count", &u64_to_bytes(count + 1));
    }

    // Register the name with 10-year expiry (max duration)
    let current_slot = moltchain_sdk::get_slot();
    let expiry_slot = current_slot + (SLOTS_PER_YEAR * 10);

    let mut record = [0u8; 48];
    record[0..32].copy_from_slice(owner);
    record[32..40].copy_from_slice(&u64_to_bytes(current_slot));
    record[40..48].copy_from_slice(&u64_to_bytes(expiry_slot));

    storage_set(&nk, &record);

    // Set reverse mapping: address → name
    let rev_key = name_reverse_key(owner);
    storage_set(&rev_key, name);

    // Increment name count
    let count = storage_get(b"molt_name_count")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    storage_set(b"molt_name_count", &u64_to_bytes(count + 1));

    log_info("Reserved .molt name registered by admin");
    reentrancy_exit();
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
        test_mock::set_caller(admin);
        let result = initialize(admin.as_ptr());
        assert_eq!(result, 0); // success

        assert_eq!(test_mock::get_storage(b"mid_admin"), Some(admin.to_vec()));
        assert_eq!(test_mock::get_storage(b"mid_initialized"), Some([1u8].to_vec()));
    }

    #[test]
    fn test_double_initialize_fails() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let result = initialize(admin.as_ptr());
        assert_eq!(result, 1); // already initialized
    }

    #[test]
    fn test_register_identity() {
        setup();
        test_mock::set_timestamp(5000);

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let owner = [2u8; 32];
        let name = b"TradingBot";
        test_mock::set_caller(owner);
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
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let owner = [2u8; 32];
        let name = b"Agent";
        test_mock::set_caller(owner);
        register_identity(owner.as_ptr(), AGENT_TYPE_GENERAL, name.as_ptr(), name.len() as u32);

        let result = register_identity(owner.as_ptr(), AGENT_TYPE_GENERAL, name.as_ptr(), name.len() as u32);
        assert_eq!(result, 3); // already registered
    }

    #[test]
    fn test_add_skill() {
        setup();
        test_mock::set_timestamp(5000);

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let owner = [2u8; 32];
        let name = b"SkillBot";
        test_mock::set_caller(owner);
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
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let nobody = [9u8; 32];
        let skill_name = b"Hacking";
        test_mock::set_caller(nobody);
        let result = add_skill(nobody.as_ptr(), skill_name.as_ptr(), skill_name.len() as u32, 50);
        assert_eq!(result, 3); // identity not found
    }

    #[test]
    fn test_vouch() {
        setup();
        test_mock::set_timestamp(5000);

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let agent_a = [2u8; 32];
        let agent_b = [3u8; 32];
        let name_a = b"AgentA";
        let name_b = b"AgentB";

        test_mock::set_caller(agent_a);
        register_identity(agent_a.as_ptr(), AGENT_TYPE_GENERAL, name_a.as_ptr(), name_a.len() as u32);
        test_mock::set_caller(agent_b);
        register_identity(agent_b.as_ptr(), AGENT_TYPE_GENERAL, name_b.as_ptr(), name_b.len() as u32);

        test_mock::set_caller(agent_a);
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
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let agent = [2u8; 32];
        let name = b"SelfVoucher";
        test_mock::set_caller(agent);
        register_identity(agent.as_ptr(), AGENT_TYPE_GENERAL, name.as_ptr(), name.len() as u32);

        let result = vouch(agent.as_ptr(), agent.as_ptr());
        assert_eq!(result, 1); // cannot vouch for yourself
    }

    #[test]
    fn test_double_vouch_fails() {
        setup();
        test_mock::set_timestamp(5000);

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let agent_a = [2u8; 32];
        let agent_b = [3u8; 32];
        let name_a = b"A";
        let name_b = b"B";

        test_mock::set_caller(agent_a);
        register_identity(agent_a.as_ptr(), AGENT_TYPE_GENERAL, name_a.as_ptr(), name_a.len() as u32);
        test_mock::set_caller(agent_b);
        register_identity(agent_b.as_ptr(), AGENT_TYPE_GENERAL, name_b.as_ptr(), name_b.len() as u32);

        test_mock::set_caller(agent_a);
        vouch(agent_a.as_ptr(), agent_b.as_ptr());
        let result = vouch(agent_a.as_ptr(), agent_b.as_ptr());
        assert_eq!(result, 6); // already vouched
    }

    #[test]
    fn test_social_recovery_happy_path() {
        setup();
        test_mock::set_timestamp(5_000);

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let target = [2u8; 32];
        let new_owner = [3u8; 32];
        let guardians = [[4u8; 32], [5u8; 32], [6u8; 32], [7u8; 32], [8u8; 32]];

        let target_name = b"Target";
        test_mock::set_caller(target);
        assert_eq!(register_identity(target.as_ptr(), AGENT_TYPE_GENERAL, target_name.as_ptr(), target_name.len() as u32), 0);

        for (idx, guardian) in guardians.iter().enumerate() {
            let g_name = match idx {
                0 => b"G0".as_slice(),
                1 => b"G1".as_slice(),
                2 => b"G2".as_slice(),
                3 => b"G3".as_slice(),
                _ => b"G4".as_slice(),
            };
            test_mock::set_caller(*guardian);
            assert_eq!(register_identity(guardian.as_ptr(), AGENT_TYPE_GENERAL, g_name.as_ptr(), g_name.len() as u32), 0);
            assert_eq!(vouch(guardian.as_ptr(), target.as_ptr()), 0);
        }

        test_mock::set_caller(target);
        assert_eq!(
            set_recovery_guardians(
                target.as_ptr(),
                guardians[0].as_ptr(),
                guardians[1].as_ptr(),
                guardians[2].as_ptr(),
                guardians[3].as_ptr(),
                guardians[4].as_ptr(),
            ),
            0
        );

        test_mock::set_caller(guardians[0]);
        assert_eq!(approve_recovery(guardians[0].as_ptr(), target.as_ptr(), new_owner.as_ptr()), 0);
        test_mock::set_caller(guardians[1]);
        assert_eq!(approve_recovery(guardians[1].as_ptr(), target.as_ptr(), new_owner.as_ptr()), 0);
        test_mock::set_caller(guardians[2]);
        assert_eq!(approve_recovery(guardians[2].as_ptr(), target.as_ptr(), new_owner.as_ptr()), 0);

        assert_eq!(execute_recovery(guardians[2].as_ptr(), target.as_ptr(), new_owner.as_ptr()), 0);

        let new_id = identity_key(&new_owner);
        let new_record = test_mock::get_storage(&new_id).unwrap();
        assert_eq!(&new_record[0..32], &new_owner);
        assert_eq!(new_record[126], 1);

        let old_id = identity_key(&target);
        let old_record = test_mock::get_storage(&old_id).unwrap();
        assert_eq!(old_record[126], 0);
    }

    #[test]
    fn test_social_recovery_rejects_non_guardian_and_insufficient_approvals() {
        setup();
        test_mock::set_timestamp(5_000);

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let target = [2u8; 32];
        let new_owner = [3u8; 32];
        let outsider = [9u8; 32];
        let guardians = [[4u8; 32], [5u8; 32], [6u8; 32], [7u8; 32], [8u8; 32]];

        test_mock::set_caller(target);
        assert_eq!(register_identity(target.as_ptr(), AGENT_TYPE_GENERAL, b"Target".as_ptr(), 6), 0);
        test_mock::set_caller(outsider);
        assert_eq!(register_identity(outsider.as_ptr(), AGENT_TYPE_GENERAL, b"Out".as_ptr(), 3), 0);

        for (idx, guardian) in guardians.iter().enumerate() {
            let g_name = match idx {
                0 => b"A0".as_slice(),
                1 => b"A1".as_slice(),
                2 => b"A2".as_slice(),
                3 => b"A3".as_slice(),
                _ => b"A4".as_slice(),
            };
            test_mock::set_caller(*guardian);
            assert_eq!(register_identity(guardian.as_ptr(), AGENT_TYPE_GENERAL, g_name.as_ptr(), g_name.len() as u32), 0);
            assert_eq!(vouch(guardian.as_ptr(), target.as_ptr()), 0);
        }

        test_mock::set_caller(target);
        assert_eq!(
            set_recovery_guardians(
                target.as_ptr(),
                guardians[0].as_ptr(),
                guardians[1].as_ptr(),
                guardians[2].as_ptr(),
                guardians[3].as_ptr(),
                guardians[4].as_ptr(),
            ),
            0
        );

        test_mock::set_caller(outsider);
        assert_eq!(approve_recovery(outsider.as_ptr(), target.as_ptr(), new_owner.as_ptr()), 2);
        test_mock::set_caller(guardians[0]);
        assert_eq!(approve_recovery(guardians[0].as_ptr(), target.as_ptr(), new_owner.as_ptr()), 0);
        test_mock::set_caller(guardians[1]);
        assert_eq!(approve_recovery(guardians[1].as_ptr(), target.as_ptr(), new_owner.as_ptr()), 0);

        assert_eq!(execute_recovery(guardians[1].as_ptr(), target.as_ptr(), new_owner.as_ptr()), 5);
    }

    #[test]
    fn test_identity_delegation_profile_and_type_updates() {
        setup();
        test_mock::set_timestamp(10_000);

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let owner = [2u8; 32];
        let delegate = [3u8; 32];

        test_mock::set_caller(owner);
        assert_eq!(register_identity(owner.as_ptr(), AGENT_TYPE_GENERAL, b"Owner".as_ptr(), 5), 0);
        test_mock::set_caller(delegate);
        assert_eq!(register_identity(delegate.as_ptr(), AGENT_TYPE_GENERAL, b"Agent".as_ptr(), 5), 0);

        let expires = 10_000 + 60_000;
        let perms = DELEGATE_PERM_PROFILE | DELEGATE_PERM_AGENT_TYPE;
        test_mock::set_caller(owner);
        assert_eq!(set_delegate(owner.as_ptr(), delegate.as_ptr(), perms, expires), 0);

        let endpoint = b"https://agent.example/api";
        test_mock::set_caller(delegate);
        assert_eq!(
            set_endpoint_as(delegate.as_ptr(), owner.as_ptr(), endpoint.as_ptr(), endpoint.len() as u32),
            0
        );
        let ek = endpoint_key(&owner);
        assert_eq!(test_mock::get_storage(&ek), Some(endpoint.to_vec()));

        assert_eq!(set_rate_as(delegate.as_ptr(), owner.as_ptr(), 77_000), 0);
        let rk = rate_key(&owner);
        let rb = test_mock::get_storage(&rk).unwrap();
        assert_eq!(bytes_to_u64(&rb), 77_000);

        assert_eq!(update_agent_type_as(delegate.as_ptr(), owner.as_ptr(), AGENT_TYPE_TRADING), 0);
        let idk = identity_key(&owner);
        let record = test_mock::get_storage(&idk).unwrap();
        assert_eq!(record[32], AGENT_TYPE_TRADING);
    }

    #[test]
    fn test_identity_delegation_permissions_and_expiry() {
        setup();
        test_mock::set_timestamp(20_000);

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let owner = [2u8; 32];
        let delegate = [3u8; 32];
        let outsider = [4u8; 32];

        test_mock::set_caller(owner);
        assert_eq!(register_identity(owner.as_ptr(), AGENT_TYPE_GENERAL, b"Owner".as_ptr(), 5), 0);
        test_mock::set_caller(delegate);
        assert_eq!(register_identity(delegate.as_ptr(), AGENT_TYPE_GENERAL, b"Agent".as_ptr(), 5), 0);
        test_mock::set_caller(outsider);
        assert_eq!(register_identity(outsider.as_ptr(), AGENT_TYPE_GENERAL, b"Other".as_ptr(), 5), 0);

        // only profile permission
        test_mock::set_caller(owner);
        assert_eq!(
            set_delegate(
                owner.as_ptr(),
                delegate.as_ptr(),
                DELEGATE_PERM_PROFILE,
                20_000 + 100,
            ),
            0
        );

        // Missing agent-type permission
    test_mock::set_caller(delegate);
        assert_eq!(update_agent_type_as(delegate.as_ptr(), owner.as_ptr(), AGENT_TYPE_TRADING), 2);

        // Outsider cannot act
    test_mock::set_caller(outsider);
        assert_eq!(set_rate_as(outsider.as_ptr(), owner.as_ptr(), 55_000), 1);

        // Expired delegation should fail
        test_mock::set_timestamp(20_500);
    test_mock::set_caller(delegate);
        assert_eq!(set_rate_as(delegate.as_ptr(), owner.as_ptr(), 55_000), 1);

        // Revoke idempotence path
    test_mock::set_caller(owner);
        assert_eq!(revoke_delegate(owner.as_ptr(), delegate.as_ptr()), 1);
    }

    #[test]
    fn test_identity_delegation_naming_refinement() {
        setup();
        test_mock::set_timestamp(30_000);

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let owner = [2u8; 32];
        let delegate = [3u8; 32];
        test_mock::set_caller(owner);
        assert_eq!(register_identity(owner.as_ptr(), AGENT_TYPE_GENERAL, b"Owner".as_ptr(), 5), 0);
        test_mock::set_caller(delegate);
        assert_eq!(register_identity(delegate.as_ptr(), AGENT_TYPE_GENERAL, b"Agent".as_ptr(), 5), 0);

        test_mock::set_slot(5_000);
        test_mock::set_value(50_000_000_000);
        let name = b"delegated-name";
        test_mock::set_caller(owner);
        assert_eq!(register_name(owner.as_ptr(), name.as_ptr(), name.len() as u32, 1), 0);

        // without naming permission should fail
        test_mock::set_caller(owner);
        assert_eq!(
            set_delegate(owner.as_ptr(), delegate.as_ptr(), DELEGATE_PERM_PROFILE, 31_000),
            0
        );
        test_mock::set_caller(delegate);
        test_mock::set_value(100_000_000);
        assert_eq!(renew_name_as(delegate.as_ptr(), owner.as_ptr(), name.as_ptr(), name.len() as u32, 1), 2);

        // with naming permission should succeed
        test_mock::set_caller(owner);
        assert_eq!(
            set_delegate(
                owner.as_ptr(),
                delegate.as_ptr(),
                DELEGATE_PERM_PROFILE | DELEGATE_PERM_NAMING,
                31_000,
            ),
            0
        );
        test_mock::set_caller(delegate);
        test_mock::set_value(50_000_000_000);
        assert_eq!(renew_name_as(delegate.as_ptr(), owner.as_ptr(), name.as_ptr(), name.len() as u32, 1), 0);
        assert_eq!(release_name_as(delegate.as_ptr(), owner.as_ptr(), name.as_ptr(), name.len() as u32), 0);
        assert!(test_mock::get_storage(&name_key(name)).is_none());
    }

    #[test]
    fn test_premium_name_auction_lifecycle() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let bidder1 = [2u8; 32];
        let bidder2 = [3u8; 32];
        test_mock::set_caller(bidder1);
        assert_eq!(register_identity(bidder1.as_ptr(), AGENT_TYPE_GENERAL, b"Bid1".as_ptr(), 4), 0);
        test_mock::set_caller(bidder2);
        assert_eq!(register_identity(bidder2.as_ptr(), AGENT_TYPE_GENERAL, b"Bid2".as_ptr(), 4), 0);

        test_mock::set_slot(10_000);
        let premium = b"xya";

        // Configure escrow for outbid refund path
        configure_mid_escrow(&admin);

        // Premium names are auction-only in direct registration path
        test_mock::set_caller(bidder1);
        test_mock::set_value(1_000_000_000);
        assert_eq!(register_name(bidder1.as_ptr(), premium.as_ptr(), premium.len() as u32, 1), 8);

        test_mock::set_caller(admin);
        assert_eq!(
            create_name_auction(admin.as_ptr(), premium.as_ptr(), premium.len() as u32, 500_000_000_000, 310_000),
            0
        );

        test_mock::set_caller(bidder1);
        test_mock::set_value(600_000_000_000);
        assert_eq!(bid_name_auction(bidder1.as_ptr(), premium.as_ptr(), premium.len() as u32, 600_000_000_000), 0);

        test_mock::set_caller(bidder2);
        test_mock::set_value(700_000_000_000);
        assert_eq!(bid_name_auction(bidder2.as_ptr(), premium.as_ptr(), premium.len() as u32, 700_000_000_000), 0);

        test_mock::set_slot(310_001);
        test_mock::set_caller(admin);
        assert_eq!(finalize_name_auction(admin.as_ptr(), premium.as_ptr(), premium.len() as u32, 1), 0);

        let nk = name_key(premium);
        let record = test_mock::get_storage(&nk).unwrap();
        assert_eq!(&record[0..32], &bidder2);
    }

    #[test]
    fn test_update_reputation() {
        setup();
        test_mock::set_timestamp(5000);

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let agent = [2u8; 32];
        let name = b"RepBot";
        test_mock::set_caller(agent);
        register_identity(agent.as_ptr(), AGENT_TYPE_GENERAL, name.as_ptr(), name.len() as u32);

        // Admin increases reputation
        test_mock::set_caller(admin);
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
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let agent = [2u8; 32];
        let name = b"Bot";
        test_mock::set_caller(agent);
        register_identity(agent.as_ptr(), AGENT_TYPE_GENERAL, name.as_ptr(), name.len() as u32);

        let non_admin = [9u8; 32];
        test_mock::set_caller(non_admin);
        let result = update_reputation(non_admin.as_ptr(), agent.as_ptr(), 50, 1);
        assert_eq!(result, 2); // unauthorized
    }

    #[test]
    fn test_deactivate_identity() {
        setup();
        test_mock::set_timestamp(5000);

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let agent = [2u8; 32];
        let name = b"DeactivateMe";
        test_mock::set_caller(agent);
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
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let agent = [2u8; 32];
        let name = b"RepCheck";
        test_mock::set_caller(agent);
        register_identity(agent.as_ptr(), AGENT_TYPE_GENERAL, name.as_ptr(), name.len() as u32);

        let result = get_reputation(agent.as_ptr());
        assert_eq!(result, 0); // success

        // Check return data contains the reputation
        let ret = test_mock::get_return_data();
        assert!(ret.len() >= 8);
        assert_eq!(bytes_to_u64(&ret), INITIAL_REPUTATION);
    }

    #[test]
    fn test_reputation_decay_on_lookup() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let agent = [2u8; 32];
        let name = b"DecayBot";

        test_mock::set_timestamp(1_000_000);
        test_mock::set_caller(agent);
        register_identity(agent.as_ptr(), AGENT_TYPE_GENERAL, name.as_ptr(), name.len() as u32);

        // Raise from 100 -> 1000
        test_mock::set_caller(admin);
        let result = update_reputation(admin.as_ptr(), agent.as_ptr(), 900, 1);
        assert_eq!(result, 0);

        // Move forward by ~180 days to trigger two decay periods (5% each)
        test_mock::set_timestamp(1_000_000 + (REPUTATION_DECAY_PERIOD_MS * 2) + 1);
        let result = get_reputation(agent.as_ptr());
        assert_eq!(result, 0);

        // 1000 * 0.95 * 0.95 = 902.5 -> 902 (integer math)
        let ret = test_mock::get_return_data();
        assert!(ret.len() >= 8);
        assert_eq!(bytes_to_u64(&ret), 902);

        // Ensure decayed value is persisted to storage
        let rep_key = reputation_key(&agent);
        let rep_bytes = test_mock::get_storage(&rep_key).unwrap();
        assert_eq!(bytes_to_u64(&rep_bytes), 902);
    }

    #[test]
    fn test_attest_skill() {
        setup();
        test_mock::set_timestamp(5000);

        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let agent_a = [2u8; 32];
        let agent_b = [3u8; 32];
        let name_a = b"AgentA";
        let name_b = b"AgentB";

        test_mock::set_caller(agent_a);
        register_identity(agent_a.as_ptr(), AGENT_TYPE_GENERAL, name_a.as_ptr(), name_a.len() as u32);
        test_mock::set_caller(agent_b);
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
        test_mock::set_caller(agent_a);
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
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let agent_a = [2u8; 32];
        let agent_b = [3u8; 32];
        let name_a = b"AgentA";
        let name_b = b"AgentB";

        test_mock::set_caller(agent_a);
        register_identity(agent_a.as_ptr(), AGENT_TYPE_GENERAL, name_a.as_ptr(), name_a.len() as u32);
        test_mock::set_caller(agent_b);
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
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        let name = b"TestAgent";
        test_mock::set_caller(*owner);
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
        setup_identity_with_slot(&owner, 1000, 100_000_000_000);

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
        setup_identity_with_slot(&owner, 1000, 100_000_000_000);

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
        setup_identity_with_slot(&owner, 1000, 100_000_000_000);

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
        setup_identity_with_slot(&owner, 1000, 100_000_000_000);

        let name = b"xfername";
        register_name(owner.as_ptr(), name.as_ptr(), name.len() as u32, 1);

        // Register new owner identity
        let new_owner = [3u8; 32];
        let new_name = b"NewOwner";
        test_mock::set_caller(new_owner);
        register_identity(
            new_owner.as_ptr(),
            AGENT_TYPE_GENERAL,
            new_name.as_ptr(),
            new_name.len() as u32,
        );

        test_mock::set_caller(owner);
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
        setup_identity_with_slot(&owner, 1000, 100_000_000_000);

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
        setup_identity_with_slot(&owner, 1000, 100_000_000_000);

        let name = b"renewable";
        register_name(owner.as_ptr(), name.as_ptr(), name.len() as u32, 1);

        // Get original expiry
        let nk = name_key(name);
        let record = test_mock::get_storage(&nk).unwrap();
        let original_expiry = bytes_to_u64(&record[40..48]);

        // Renew for 2 more years (need to set value again for payment)
        test_mock::set_value(40_000_000_000);
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
        setup_identity_with_slot(&owner, 1000, 100_000_000_000);

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
        setup_identity_with_slot(&owner, 1000, 100_000_000_000);

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
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let agent = [2u8; 32];
        let name = b"TierBot";
        test_mock::set_caller(agent);
        register_identity(agent.as_ptr(), AGENT_TYPE_GENERAL, name.as_ptr(), name.len() as u32);

        // Initial reputation = 100 → Tier 1
        get_trust_tier(agent.as_ptr());
        let ret = test_mock::get_return_data();
        assert_eq!(ret[0], 1); // Verified tier

        // Boost to 500 → Tier 2
        test_mock::set_caller(admin);
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
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        // Pause
        assert_eq!(mid_pause(admin.as_ptr()), 0);

        // Registration blocked
        let agent = [2u8; 32];
        let name = b"test-agent";
        test_mock::set_caller(agent);
        let result = register_identity(agent.as_ptr(), 1, name.as_ptr(), name.len() as u32);
        assert_eq!(result, 20);

        // Unpause
        test_mock::set_caller(admin);
        assert_eq!(mid_unpause(admin.as_ptr()), 0);

        // Now works
        test_mock::set_caller(agent);
        let result = register_identity(agent.as_ptr(), 1, name.as_ptr(), name.len() as u32);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_pause_admin_checks() {
        setup();
        let admin = [1u8; 32];
        let non_admin = [2u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        test_mock::set_caller(non_admin);
        assert_eq!(mid_pause(non_admin.as_ptr()), 1); // not admin
        test_mock::set_caller(admin);
        assert_eq!(mid_pause(admin.as_ptr()), 0);
        assert_eq!(mid_pause(admin.as_ptr()), 2); // already paused
        test_mock::set_caller(non_admin);
        assert_eq!(mid_unpause(non_admin.as_ptr()), 1); // not admin
        test_mock::set_caller(admin);
        assert_eq!(mid_unpause(admin.as_ptr()), 0);
        assert_eq!(mid_unpause(admin.as_ptr()), 2); // not paused
    }

    #[test]
    fn test_vouch_cooldown() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        test_mock::set_timestamp(1_000_000);

        let voucher = [2u8; 32];
        let vouchee1 = [3u8; 32];
        let vouchee2 = [4u8; 32];
        let name1 = b"voucher";
        let name2 = b"vouchee1";
        let name3 = b"vouchee2";
        test_mock::set_caller(voucher);
        register_identity(voucher.as_ptr(), 1, name1.as_ptr(), name1.len() as u32);
        test_mock::set_caller(vouchee1);
        register_identity(vouchee1.as_ptr(), 1, name2.as_ptr(), name2.len() as u32);
        test_mock::set_caller(vouchee2);
        register_identity(vouchee2.as_ptr(), 1, name3.as_ptr(), name3.len() as u32);

        // Boost voucher rep so they can vouch
        test_mock::set_caller(admin);
        update_reputation(admin.as_ptr(), voucher.as_ptr(), 100, 1);

        // First vouch works
        test_mock::set_caller(voucher);
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
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        test_mock::set_timestamp(1_000_000);

        let voucher = [2u8; 32];
        let vouchee = [3u8; 32];
        let name1 = b"voucher2";
        let name2 = b"vouchee3";
        test_mock::set_caller(voucher);
        register_identity(voucher.as_ptr(), 1, name1.as_ptr(), name1.len() as u32);
        test_mock::set_caller(vouchee);
        register_identity(vouchee.as_ptr(), 1, name2.as_ptr(), name2.len() as u32);
        test_mock::set_caller(admin);
        update_reputation(admin.as_ptr(), voucher.as_ptr(), 100, 1);

        // Pause
        test_mock::set_caller(admin);
        mid_pause(admin.as_ptr());
        test_mock::set_caller(voucher);
        let result = vouch(voucher.as_ptr(), vouchee.as_ptr());
        assert_eq!(result, 20);

        // Unpause
        test_mock::set_caller(admin);
        mid_unpause(admin.as_ptr());
        test_mock::set_caller(voucher);
        let result = vouch(voucher.as_ptr(), vouchee.as_ptr());
        assert_eq!(result, 0);
    }

    #[test]
    fn test_transfer_admin() {
        setup();
        let admin = [1u8; 32];
        let new_admin = [10u8; 32];
        let other = [11u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        // Non-admin can't transfer
        test_mock::set_caller(other);
        assert_eq!(transfer_admin(other.as_ptr(), new_admin.as_ptr()), 1);

        // Admin transfers
        test_mock::set_caller(admin);
        assert_eq!(transfer_admin(admin.as_ptr(), new_admin.as_ptr()), 0);

        // Old admin no longer works
        test_mock::set_caller(admin);
        assert_eq!(mid_pause(admin.as_ptr()), 1);
        // New admin works
        test_mock::set_caller(new_admin);
        assert_eq!(mid_pause(new_admin.as_ptr()), 0);
    }

    #[test]
    fn test_admin_register_reserved_name() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        test_mock::set_timestamp(1_000_000);

        // Owner address that will get the reserved name
        let owner = [50u8; 32];
        let name = b"moltchain";

        // Helper: build args buffer [admin 32B][owner 32B][name bytes][name_len 4B LE][agent_type 1B]
        fn build_args(admin: &[u8; 32], owner: &[u8; 32], name: &[u8], agent_type: u8) -> Vec<u8> {
            let name_len = name.len() as u32;
            let mut args = Vec::with_capacity(32 + 32 + name.len() + 4 + 1);
            args.extend_from_slice(admin);
            args.extend_from_slice(owner);
            args.extend_from_slice(name);
            args.extend_from_slice(&name_len.to_le_bytes());
            args.push(agent_type);
            args
        }

        // Non-admin cannot call
        let other = [99u8; 32];
        test_mock::ARGS.with(|a| *a.borrow_mut() = build_args(&other, &owner, name, 0));
        assert_eq!(admin_register_reserved_name(), 1);

        // Admin can register a reserved name
        test_mock::ARGS.with(|a| *a.borrow_mut() = build_args(&admin, &owner, name, 0));
        assert_eq!(admin_register_reserved_name(), 0);

        // Identity was auto-created
        let id_key = identity_key(&owner);
        assert!(storage_get(&id_key).is_some());

        // Name record exists
        let nk = name_key(name);
        let record = storage_get(&nk).unwrap();
        assert_eq!(record.len(), 48);
        assert_eq!(&record[0..32], &owner);

        // Reverse mapping exists
        let rev = name_reverse_key(&owner);
        let rev_val = storage_get(&rev).unwrap();
        assert_eq!(&rev_val, name);

        // Duplicate registration fails
        let owner2 = [51u8; 32];
        test_mock::ARGS.with(|a| *a.borrow_mut() = build_args(&admin, &owner2, name, 0));
        assert_eq!(admin_register_reserved_name(), 5);
    }

    #[test]
    fn test_admin_register_reserved_name_treasury() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());
        test_mock::set_timestamp(1_000_000);

        let treasury_addr = [60u8; 32];
        let name = b"treasury";

        // Helper
        fn build_args(admin: &[u8; 32], owner: &[u8; 32], name: &[u8], agent_type: u8) -> Vec<u8> {
            let name_len = name.len() as u32;
            let mut args = Vec::with_capacity(32 + 32 + name.len() + 4 + 1);
            args.extend_from_slice(admin);
            args.extend_from_slice(owner);
            args.extend_from_slice(name);
            args.extend_from_slice(&name_len.to_le_bytes());
            args.push(agent_type);
            args
        }

        // treasury is a reserved name — regular register fails
        // but admin_register_reserved_name succeeds
        test_mock::ARGS.with(|a| *a.borrow_mut() = build_args(&admin, &treasury_addr, name, 0));
        assert_eq!(admin_register_reserved_name(), 0);

        // Verify name count
        let count = storage_get(b"molt_name_count").map(|d| bytes_to_u64(&d)).unwrap_or(0);
        assert_eq!(count, 1);
    }

    // ========================================================================
    // FNV-1a HASH COLLISION PREVENTION TESTS
    // ========================================================================

    #[test]
    fn test_fnv1a_hash_no_collision_on_shared_prefix() {
        // Two skill names sharing a 16-byte prefix must hash to DIFFERENT values.
        // The old truncation approach would produce identical hashes.
        let skill_a = b"smart_contracts_audit";
        let skill_b = b"smart_contracts_dev";

        let hash_a = skill_name_hash(skill_a);
        let hash_b = skill_name_hash(skill_b);
        assert_ne!(hash_a, hash_b, "FNV-1a must distinguish skills with shared 16-byte prefix");

        // Verify the legacy function DOES collide (proves the bug existed)
        let legacy_a = skill_name_hash_legacy(skill_a);
        let legacy_b = skill_name_hash_legacy(skill_b);
        assert_eq!(legacy_a, legacy_b, "Legacy truncation should collide on shared prefix");
    }

    #[test]
    fn test_fnv1a_hash_deterministic() {
        let skill = b"rust_programming";
        let h1 = skill_name_hash(skill);
        let h2 = skill_name_hash(skill);
        assert_eq!(h1, h2, "Same input must produce same hash");
    }

    #[test]
    fn test_fnv1a_hash_empty_and_edge_cases() {
        // Empty input should not panic
        let h_empty = skill_name_hash(b"");
        // Single byte
        let h_a = skill_name_hash(b"a");
        let h_b = skill_name_hash(b"b");
        assert_ne!(h_a, h_b);
        // Very long input
        let long = [0x42u8; 256];
        let h_long = skill_name_hash(&long);
        assert_ne!(h_long, h_empty);
    }

    #[test]
    fn test_attest_skill_uses_fnv_hash() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let identity = [10u8; 32];
        let attester = [11u8; 32];
        test_mock::set_caller(identity);
        assert_eq!(register_identity(identity.as_ptr(), AGENT_TYPE_GENERAL, b"Ident".as_ptr(), 5), 0);
        test_mock::set_caller(attester);
        assert_eq!(register_identity(attester.as_ptr(), AGENT_TYPE_GENERAL, b"Attest".as_ptr(), 6), 0);
        test_mock::set_timestamp(1000);

        // Attest a skill — should be stored under FNV hash key
        let skill = b"smart_contracts_audit";
        assert_eq!(attest_skill(
            attester.as_ptr(), identity.as_ptr(),
            skill.as_ptr(), skill.len() as u32, 3
        ), 0);

        // Verify it's under the FNV hash
        let s_hash = skill_name_hash(skill);
        let ak = attestation_key(&identity, &s_hash, &attester);
        assert!(storage_get(&ak).is_some(), "Attestation must be stored under FNV hash");

        // Verify it's NOT under the legacy hash (they differ for this skill)
        let s_hash_legacy = skill_name_hash_legacy(skill);
        let ak_legacy = attestation_key(&identity, &s_hash_legacy, &attester);
        assert!(storage_get(&ak_legacy).is_none(), "Should not be stored under legacy hash");
    }

    #[test]
    fn test_attest_skill_no_collision_between_similar_skills() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let identity = [10u8; 32];
        let attester = [11u8; 32];
        test_mock::set_caller(identity);
        assert_eq!(register_identity(identity.as_ptr(), AGENT_TYPE_GENERAL, b"Ident".as_ptr(), 5), 0);
        test_mock::set_caller(attester);
        assert_eq!(register_identity(attester.as_ptr(), AGENT_TYPE_GENERAL, b"Attest".as_ptr(), 6), 0);
        test_mock::set_timestamp(1000);

        let skill_a = b"smart_contracts_audit";
        let skill_b = b"smart_contracts_dev";

        // Attest both — both must succeed (no collision)
        assert_eq!(attest_skill(
            attester.as_ptr(), identity.as_ptr(),
            skill_a.as_ptr(), skill_a.len() as u32, 3
        ), 0);
        assert_eq!(attest_skill(
            attester.as_ptr(), identity.as_ptr(),
            skill_b.as_ptr(), skill_b.len() as u32, 4
        ), 0);

        // Verify separate counts
        assert_eq!(get_attestations(identity.as_ptr(), skill_a.as_ptr(), skill_a.len() as u32), 0);
        let count_a = bytes_to_u64(&test_mock::get_return_data());
        assert_eq!(count_a, 1);

        assert_eq!(get_attestations(identity.as_ptr(), skill_b.as_ptr(), skill_b.len() as u32), 0);
        let count_b = bytes_to_u64(&test_mock::get_return_data());
        assert_eq!(count_b, 1);
    }

    #[test]
    fn test_duplicate_attestation_blocked_under_new_hash() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let identity = [10u8; 32];
        let attester = [11u8; 32];
        test_mock::set_caller(identity);
        assert_eq!(register_identity(identity.as_ptr(), AGENT_TYPE_GENERAL, b"Ident".as_ptr(), 5), 0);
        test_mock::set_caller(attester);
        assert_eq!(register_identity(attester.as_ptr(), AGENT_TYPE_GENERAL, b"Attest".as_ptr(), 6), 0);
        test_mock::set_timestamp(1000);

        let skill = b"blockchain";
        assert_eq!(attest_skill(
            attester.as_ptr(), identity.as_ptr(),
            skill.as_ptr(), skill.len() as u32, 2
        ), 0);
        // Second attestation must fail
        assert_eq!(attest_skill(
            attester.as_ptr(), identity.as_ptr(),
            skill.as_ptr(), skill.len() as u32, 4
        ), 6);
    }

    #[test]
    fn test_duplicate_attestation_blocked_under_legacy_hash() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let identity = [10u8; 32];
        let attester = [11u8; 32];
        test_mock::set_caller(identity);
        assert_eq!(register_identity(identity.as_ptr(), AGENT_TYPE_GENERAL, b"Ident".as_ptr(), 5), 0);
        test_mock::set_caller(attester);
        assert_eq!(register_identity(attester.as_ptr(), AGENT_TYPE_GENERAL, b"Attest".as_ptr(), 6), 0);
        test_mock::set_timestamp(1000);

        // Simulate a legacy attestation by writing directly under the old hash
        let skill = b"testing";
        let s_hash_legacy = skill_name_hash_legacy(skill);
        let ak_legacy = attestation_key(&identity, &s_hash_legacy, &attester);
        storage_set(&ak_legacy, &[2, 0, 0, 0, 0, 0, 0, 0, 1]); // level=2, timestamp=1

        // Attempting to attest the same skill under the new hash must be blocked
        assert_eq!(attest_skill(
            attester.as_ptr(), identity.as_ptr(),
            skill.as_ptr(), skill.len() as u32, 3
        ), 6, "Must detect legacy attestation and block duplicate");
    }

    #[test]
    fn test_get_attestations_falls_back_to_legacy() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let identity = [10u8; 32];
        test_mock::set_caller(identity);
        assert_eq!(register_identity(identity.as_ptr(), AGENT_TYPE_GENERAL, b"Ident".as_ptr(), 5), 0);

        // Write a count directly under legacy hash key (simulating pre-upgrade data)
        let skill = b"solidity";
        let s_hash_legacy = skill_name_hash_legacy(skill);
        let ck_legacy = attestation_count_key(&identity, &s_hash_legacy);
        storage_set(&ck_legacy, &u64_to_bytes(7));

        // get_attestations should find the legacy count
        assert_eq!(get_attestations(identity.as_ptr(), skill.as_ptr(), skill.len() as u32), 0);
        let count = bytes_to_u64(&test_mock::get_return_data());
        assert_eq!(count, 7, "Must fall back to legacy count");
    }

    #[test]
    fn test_revoke_attestation_under_new_hash() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let identity = [10u8; 32];
        let attester = [11u8; 32];
        test_mock::set_caller(identity);
        assert_eq!(register_identity(identity.as_ptr(), AGENT_TYPE_GENERAL, b"Ident".as_ptr(), 5), 0);
        test_mock::set_caller(attester);
        assert_eq!(register_identity(attester.as_ptr(), AGENT_TYPE_GENERAL, b"Attest".as_ptr(), 6), 0);
        test_mock::set_timestamp(1000);

        let skill = b"defi";
        assert_eq!(attest_skill(
            attester.as_ptr(), identity.as_ptr(),
            skill.as_ptr(), skill.len() as u32, 5
        ), 0);

        // Revoke
        assert_eq!(revoke_attestation(
            attester.as_ptr(), identity.as_ptr(),
            skill.as_ptr(), skill.len() as u32
        ), 0);

        // Attestation gone
        let s_hash = skill_name_hash(skill);
        let ak = attestation_key(&identity, &s_hash, &attester);
        assert!(storage_get(&ak).is_none());

        // Count back to 0
        assert_eq!(get_attestations(identity.as_ptr(), skill.as_ptr(), skill.len() as u32), 0);
        let count = bytes_to_u64(&test_mock::get_return_data());
        assert_eq!(count, 0);
    }

    #[test]
    fn test_revoke_attestation_under_legacy_hash() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let identity = [10u8; 32];
        let attester = [11u8; 32];
        test_mock::set_caller(identity);
        assert_eq!(register_identity(identity.as_ptr(), AGENT_TYPE_GENERAL, b"Ident".as_ptr(), 5), 0);
        test_mock::set_caller(attester);
        assert_eq!(register_identity(attester.as_ptr(), AGENT_TYPE_GENERAL, b"Attest".as_ptr(), 6), 0);

        // Write attestation directly under legacy hash (simulating pre-upgrade data)
        let skill = b"zk_proofs";
        let s_hash_legacy = skill_name_hash_legacy(skill);
        let ak_legacy = attestation_key(&identity, &s_hash_legacy, &attester);
        storage_set(&ak_legacy, &[4, 0, 0, 0, 0, 0, 0, 0, 1]);
        let ck_legacy = attestation_count_key(&identity, &s_hash_legacy);
        storage_set(&ck_legacy, &u64_to_bytes(1));

        // Revoke should find and remove the legacy attestation
        assert_eq!(revoke_attestation(
            attester.as_ptr(), identity.as_ptr(),
            skill.as_ptr(), skill.len() as u32
        ), 0);

        assert!(storage_get(&ak_legacy).is_none(), "Legacy attestation must be removed");
        let count = bytes_to_u64(&storage_get(&ck_legacy).unwrap());
        assert_eq!(count, 0, "Legacy count must be decremented");
    }

    #[test]
    fn test_attest_migrates_legacy_count_forward() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let identity = [10u8; 32];
        let attester1 = [11u8; 32];
        let attester2 = [12u8; 32];
        test_mock::set_caller(identity);
        assert_eq!(register_identity(identity.as_ptr(), AGENT_TYPE_GENERAL, b"Ident".as_ptr(), 5), 0);
        test_mock::set_caller(attester1);
        assert_eq!(register_identity(attester1.as_ptr(), AGENT_TYPE_GENERAL, b"Att1".as_ptr(), 4), 0);
        test_mock::set_caller(attester2);
        assert_eq!(register_identity(attester2.as_ptr(), AGENT_TYPE_GENERAL, b"Att2".as_ptr(), 4), 0);
        test_mock::set_timestamp(1000);

        // Seed a legacy attestation from attester1
        let skill = b"nft_minting";
        let s_hash_legacy = skill_name_hash_legacy(skill);
        let ak_legacy = attestation_key(&identity, &s_hash_legacy, &attester1);
        storage_set(&ak_legacy, &[3, 0, 0, 0, 0, 0, 0, 0, 1]);
        let ck_legacy = attestation_count_key(&identity, &s_hash_legacy);
        storage_set(&ck_legacy, &u64_to_bytes(1));

        // Now attester2 attests the same skill via the new code path
        assert_eq!(attest_skill(
            attester2.as_ptr(), identity.as_ptr(),
            skill.as_ptr(), skill.len() as u32, 4
        ), 0);

        // The new FNV count should be legacy_count + 1 = 2
        let s_hash_new = skill_name_hash(skill);
        let ck_new = attestation_count_key(&identity, &s_hash_new);
        let count = bytes_to_u64(&storage_get(&ck_new).unwrap());
        assert_eq!(count, 2, "Legacy count must be migrated forward on first new write");
    }

    // ========================================================================
    // BID REFUND FIX TESTS
    // ========================================================================

    fn configure_mid_escrow(_admin: &[u8; 32]) {
        let token_addr = [0xAA; 32];
        let self_addr = [0xBB; 32];
        storage_set(MID_TOKEN_ADDR_KEY, &token_addr);
        storage_set(MID_SELF_ADDR_KEY, &self_addr);
    }

    #[test]
    fn test_set_mid_token_address_admin_only() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let token = [0xCC; 32];

        // Non-admin rejected
        let other = [99u8; 32];
        test_mock::set_caller(other);
        assert_eq!(set_mid_token_address(other.as_ptr(), token.as_ptr()), 1);

        // Admin succeeds
        test_mock::set_caller(admin);
        assert_eq!(set_mid_token_address(admin.as_ptr(), token.as_ptr()), 0);
        assert_eq!(storage_get(MID_TOKEN_ADDR_KEY).unwrap(), token.to_vec());
    }

    #[test]
    fn test_set_mid_self_address_admin_only() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let self_addr = [0xDD; 32];

        // Non-admin rejected
        let other = [99u8; 32];
        test_mock::set_caller(other);
        assert_eq!(set_mid_self_address(other.as_ptr(), self_addr.as_ptr()), 1);

        // Admin succeeds
        test_mock::set_caller(admin);
        assert_eq!(set_mid_self_address(admin.as_ptr(), self_addr.as_ptr()), 0);
        assert_eq!(storage_get(MID_SELF_ADDR_KEY).unwrap(), self_addr.to_vec());
    }

    #[test]
    fn test_set_mid_addresses_reject_zero() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let zero = [0u8; 32];
        assert_eq!(set_mid_token_address(admin.as_ptr(), zero.as_ptr()), 2);
        assert_eq!(set_mid_self_address(admin.as_ptr(), zero.as_ptr()), 2);
    }

    #[test]
    fn test_bid_auction_refund_requires_token_config() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let bidder1 = [2u8; 32];
        let bidder2 = [3u8; 32];
        test_mock::set_caller(bidder1);
        assert_eq!(register_identity(bidder1.as_ptr(), AGENT_TYPE_GENERAL, b"Bid1".as_ptr(), 4), 0);
        test_mock::set_caller(bidder2);
        assert_eq!(register_identity(bidder2.as_ptr(), AGENT_TYPE_GENERAL, b"Bid2".as_ptr(), 4), 0);

        test_mock::set_slot(10_000);
        let name = b"abc";

        // Create auction
        test_mock::set_caller(admin);
        assert_eq!(create_name_auction(admin.as_ptr(), name.as_ptr(), name.len() as u32, 500_000_000_000, 300_500), 0);

        // First bid succeeds (no refund needed yet)
        test_mock::set_caller(bidder1);
        test_mock::set_value(600_000_000_000);
        assert_eq!(bid_name_auction(bidder1.as_ptr(), name.as_ptr(), name.len() as u32, 600_000_000_000), 0);

        // Second bid: triggers refund but token address not configured → error 30
        test_mock::set_caller(bidder2);
        test_mock::set_value(700_000_000_000);
        assert_eq!(bid_name_auction(bidder2.as_ptr(), name.as_ptr(), name.len() as u32, 700_000_000_000), 30);
    }

    #[test]
    fn test_bid_auction_refund_with_token_config() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let bidder1 = [2u8; 32];
        let bidder2 = [3u8; 32];
        test_mock::set_caller(bidder1);
        assert_eq!(register_identity(bidder1.as_ptr(), AGENT_TYPE_GENERAL, b"Bid1".as_ptr(), 4), 0);
        test_mock::set_caller(bidder2);
        assert_eq!(register_identity(bidder2.as_ptr(), AGENT_TYPE_GENERAL, b"Bid2".as_ptr(), 4), 0);

        test_mock::set_slot(10_000);
        let name = b"xyz";

        // Configure escrow
        configure_mid_escrow(&admin);

        // Create auction
        test_mock::set_caller(admin);
        assert_eq!(create_name_auction(admin.as_ptr(), name.as_ptr(), name.len() as u32, 500_000_000_000, 300_500), 0);

        // First bid
        test_mock::set_caller(bidder1);
        test_mock::set_value(600_000_000_000);
        assert_eq!(bid_name_auction(bidder1.as_ptr(), name.as_ptr(), name.len() as u32, 600_000_000_000), 0);

        // Second bid: triggers refund, should succeed with token configured
        test_mock::set_caller(bidder2);
        test_mock::set_value(700_000_000_000);
        assert_eq!(bid_name_auction(bidder2.as_ptr(), name.as_ptr(), name.len() as u32, 700_000_000_000), 0);

        // Verify the auction record was updated to bidder2
        let ak = name_auction_key(name);
        let record = storage_get(&ak).unwrap();
        assert_eq!(bytes_to_u64(&record[25..33]), 700_000_000_000);
        assert_eq!(&record[33..65], &bidder2);
    }

    #[test]
    fn test_bid_auction_refund_requires_self_address() {
        setup();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr());

        let bidder1 = [2u8; 32];
        let bidder2 = [3u8; 32];
        test_mock::set_caller(bidder1);
        assert_eq!(register_identity(bidder1.as_ptr(), AGENT_TYPE_GENERAL, b"Bid1".as_ptr(), 4), 0);
        test_mock::set_caller(bidder2);
        assert_eq!(register_identity(bidder2.as_ptr(), AGENT_TYPE_GENERAL, b"Bid2".as_ptr(), 4), 0);

        test_mock::set_slot(10_000);
        let name = b"def";

        // Only set token address, not self address
        let token_addr = [0xAA; 32];
        storage_set(MID_TOKEN_ADDR_KEY, &token_addr);

        test_mock::set_caller(admin);
        assert_eq!(create_name_auction(admin.as_ptr(), name.as_ptr(), name.len() as u32, 500_000_000_000, 300_500), 0);

        test_mock::set_caller(bidder1);
        test_mock::set_value(600_000_000_000);
        assert_eq!(bid_name_auction(bidder1.as_ptr(), name.as_ptr(), name.len() as u32, 600_000_000_000), 0);

        // Second bid: token set but self-address not → error 31
        test_mock::set_caller(bidder2);
        test_mock::set_value(700_000_000_000);
        assert_eq!(bid_name_auction(bidder2.as_ptr(), name.as_ptr(), name.len() as u32, 700_000_000_000), 31);
    }

    #[test]
    fn test_reentrancy_guard_blocks_recursive_call() {
        // G18-02: Verify reentrancy guard returns 100 when entered
        test_mock::reset();
        // Simulate reentrancy by setting the guard manually
        storage_set(MOLTYID_REENTRANCY_KEY, &[1u8]);

        // All guarded functions should return 100
        let addr = [1u8; 32];
        assert_eq!(register_identity(addr.as_ptr(), 1, addr.as_ptr(), 4), 100);
        let name = b"test";
        assert_eq!(register_name(addr.as_ptr(), name.as_ptr(), 4, 1), 100);
        assert_eq!(bid_name_auction(addr.as_ptr(), name.as_ptr(), 4, 100), 100);

        // Clear guard — should work again (will fail on other checks but not reentrancy)
        storage_set(MOLTYID_REENTRANCY_KEY, &[0u8]);
        // bid_name_auction should get past reentrancy (fail on identity check instead)
        test_mock::set_caller(addr);
        assert_eq!(bid_name_auction(addr.as_ptr(), name.as_ptr(), 4, 100), 1); // "Bidder must have MoltyID"
    }

    #[test]
    fn test_bid_auction_cei_pattern() {
        // G18-02: Verify state is updated before external call (CEI)
        // When a higher bid comes in, auction record should be updated
        // even if refund transfer fails
        test_mock::reset();

        let creator = [1u8; 32];
        let bidder1 = [2u8; 32];
        let bidder2 = [3u8; 32];

        // Setup identities
        test_mock::set_caller(creator);
        initialize(creator.as_ptr());

        test_mock::set_caller(creator);
        assert_eq!(
            register_identity(creator.as_ptr(), AGENT_TYPE_GENERAL, b"Creator".as_ptr(), 7),
            0
        );
        test_mock::set_caller(bidder1);
        assert_eq!(
            register_identity(bidder1.as_ptr(), AGENT_TYPE_GENERAL, b"Bid1".as_ptr(), 4),
            0
        );
        test_mock::set_caller(bidder2);
        assert_eq!(
            register_identity(bidder2.as_ptr(), AGENT_TYPE_GENERAL, b"Bid2".as_ptr(), 4),
            0
        );

        // Create auction
        let name = b"prm";
        test_mock::set_caller(creator);
        test_mock::set_slot(100);
        let result = create_name_auction(creator.as_ptr(), name.as_ptr(), name.len() as u32, 50_000_000_000, 300_100);
        assert_eq!(result, 0, "Auction creation should succeed");

        // First bid
        test_mock::set_caller(bidder1);
        test_mock::set_value(60_000_000_000);
        assert_eq!(bid_name_auction(bidder1.as_ptr(), name.as_ptr(), name.len() as u32, 60_000_000_000), 0);

        // After first bid, auction record should have bidder1
        let ak = name_auction_key(name);
        let record = storage_get(&ak).unwrap();
        assert_eq!(bytes_to_u64(&record[25..33]), 60_000_000_000);
        assert_eq!(&record[33..65], &bidder1);
    }
}
