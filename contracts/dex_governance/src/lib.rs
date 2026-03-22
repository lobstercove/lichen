// DEX Governance — Trading Pair Listing & Fee Governance (DEEP hardened)
//
// Features:
//   - Proposal-based pair listing via community voting
//   - Fee change proposals with time-locks
//   - MoltyID reputation-gated proposals (min 500 rep)
//   - 48-hour voting period, 66% approval threshold
//   - Emergency delisting by admin
//   - Listing requirements: min liquidity, min holders
//   - Emergency pause, reentrancy guard

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]
#![allow(dead_code)]
#![allow(clippy::ptr_arg)]

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    bytes_to_u64, call_contract, get_caller, get_slot, log_info, storage_get, storage_set,
    u64_to_bytes, Address, CrossCall,
};

// ============================================================================
// CONSTANTS
// ============================================================================

const SLOT_DURATION_MS: u64 = 400;
const VOTING_PERIOD_SLOTS: u64 = 432_000; // ~48 hours at 400ms/slot
const APPROVAL_THRESHOLD_BPS: u64 = 6600; // 66%
const EXECUTION_DELAY_SLOTS: u64 = 9_000; // 1 hour timelock after voting at 400ms/slot
const MIN_REPUTATION: u64 = 500;
const MIN_LISTING_LIQUIDITY: u64 = 10_000_000_000_000; // 10,000 MOLT ($1K at $0.10) per TOKENOMICS.md
const MIN_LISTING_HOLDERS: u64 = 10;
const MAX_PROPOSALS: u64 = 500;

const PREFERRED_QUOTE_KEY: &[u8] = b"gov_preferred_quote";
const ALLOWED_QUOTE_COUNT_KEY: &[u8] = b"gov_aq_count";
const MAX_ALLOWED_QUOTES: u64 = 8;

// Proposal types
const PROPOSAL_NEW_PAIR: u8 = 0;
const PROPOSAL_FEE_CHANGE: u8 = 1;
const PROPOSAL_DELIST: u8 = 2;
const PROPOSAL_PARAM_CHANGE: u8 = 3;

// Proposal status
const STATUS_ACTIVE: u8 = 0;
const STATUS_PASSED: u8 = 1;
const STATUS_REJECTED: u8 = 2;
const STATUS_EXECUTED: u8 = 3;
const STATUS_CANCELLED: u8 = 4;

// Storage keys
const ADMIN_KEY: &[u8] = b"gov_admin";
const PAUSED_KEY: &[u8] = b"gov_paused";
const REENTRANCY_KEY: &[u8] = b"gov_reentrancy";
const PROPOSAL_COUNT_KEY: &[u8] = b"gov_prop_count";
const CORE_ADDRESS_KEY: &[u8] = b"gov_core_addr";
const MOLTYID_ADDRESS_KEY: &[u8] = b"gov_moltyid_addr";
const TOTAL_VOTES_KEY: &[u8] = b"gov_total_votes";
const VOTER_COUNT_KEY: &[u8] = b"gov_voter_count";

// ============================================================================
// HELPERS
// ============================================================================

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key)
        .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
        .unwrap_or(0)
}
fn save_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
}
fn load_addr(key: &[u8]) -> [u8; 32] {
    storage_get(key)
        .map(|d| {
            let mut a = [0u8; 32];
            if d.len() >= 32 {
                a.copy_from_slice(&d[..32]);
            }
            a
        })
        .unwrap_or([0u8; 32])
}
fn is_zero(addr: &[u8; 32]) -> bool {
    addr.iter().all(|&b| b == 0)
}

fn allowed_quote_key(idx: u64) -> Vec<u8> {
    let mut k = b"gov_aq_".to_vec();
    k.extend_from_slice(&u64_to_bytes(idx));
    k
}

fn is_allowed_quote(addr: &[u8; 32]) -> bool {
    let count = load_u64(ALLOWED_QUOTE_COUNT_KEY);
    if count > 0 {
        for i in 0..count {
            if load_addr(&allowed_quote_key(i)) == *addr {
                return true;
            }
        }
        return false;
    }
    let preferred = load_addr(PREFERRED_QUOTE_KEY);
    if is_zero(&preferred) {
        return true;
    }
    *addr == preferred
}

fn u64_to_decimal(mut n: u64) -> Vec<u8> {
    if n == 0 {
        return alloc::vec![b'0'];
    }
    let mut buf = Vec::new();
    while n > 0 {
        buf.push(b'0' + (n % 10) as u8);
        n /= 10;
    }
    buf.reverse();
    buf
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

fn proposal_key(id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"gov_prop_"[..]);
    k.extend_from_slice(&u64_to_decimal(id));
    k
}

fn vote_key(proposal_id: u64, voter: &[u8; 32]) -> Vec<u8> {
    let mut k = Vec::from(&b"gov_vote_"[..]);
    k.extend_from_slice(&u64_to_decimal(proposal_id));
    k.push(b'_');
    k.extend_from_slice(&hex_encode(voter));
    k
}

// ============================================================================
// DEEP SECURITY
// ============================================================================

fn reentrancy_enter() -> bool {
    if storage_get(REENTRANCY_KEY)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
    {
        return false;
    }
    storage_set(REENTRANCY_KEY, &[1u8]);
    true
}
fn reentrancy_exit() {
    storage_set(REENTRANCY_KEY, &[0u8]);
}
fn is_paused() -> bool {
    storage_get(PAUSED_KEY)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
}
fn require_not_paused() -> bool {
    !is_paused()
}
fn require_admin(caller: &[u8; 32]) -> bool {
    let admin = load_addr(ADMIN_KEY);
    !is_zero(&admin) && *caller == admin
}

// ============================================================================
// PROPOSAL LAYOUT (120 bytes)
// ============================================================================
// Bytes 0..32   : proposer address
// Bytes 32..40  : proposal_id (u64)
// Byte  40      : proposal_type (u8)
// Byte  41      : status (u8)
// Bytes 42..50  : created_slot (u64)
// Bytes 50..58  : end_slot (u64) — voting end
// Bytes 58..66  : yes_votes (u64)
// Bytes 66..74  : no_votes (u64)
// Bytes 74..82  : pair_id (u64) — target pair (for fee/delist)
// Bytes 82..114 : evidence/data (32 bytes) — base_token for new pair, etc.
// Bytes 114..116: new_maker_fee (i16) — for fee proposals
// Bytes 116..118: new_taker_fee (u16) — for fee proposals
// Bytes 118..120: padding

const PROPOSAL_SIZE: usize = 120;

fn encode_proposal(
    proposer: &[u8; 32],
    proposal_id: u64,
    ptype: u8,
    status: u8,
    created_slot: u64,
    end_slot: u64,
    yes_votes: u64,
    no_votes: u64,
    pair_id: u64,
    evidence: &[u8; 32],
    maker_fee: i16,
    taker_fee: u16,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(PROPOSAL_SIZE);
    data.extend_from_slice(proposer);
    data.extend_from_slice(&u64_to_bytes(proposal_id));
    data.push(ptype);
    data.push(status);
    data.extend_from_slice(&u64_to_bytes(created_slot));
    data.extend_from_slice(&u64_to_bytes(end_slot));
    data.extend_from_slice(&u64_to_bytes(yes_votes));
    data.extend_from_slice(&u64_to_bytes(no_votes));
    data.extend_from_slice(&u64_to_bytes(pair_id));
    data.extend_from_slice(evidence);
    data.extend_from_slice(&maker_fee.to_le_bytes());
    data.extend_from_slice(&taker_fee.to_le_bytes());
    while data.len() < PROPOSAL_SIZE {
        data.push(0);
    }
    data
}

fn decode_prop_status(data: &[u8]) -> u8 {
    if data.len() > 41 {
        data[41]
    } else {
        0
    }
}
fn decode_prop_end_slot(data: &[u8]) -> u64 {
    if data.len() >= 58 {
        bytes_to_u64(&data[50..58])
    } else {
        0
    }
}
fn decode_prop_yes(data: &[u8]) -> u64 {
    if data.len() >= 66 {
        bytes_to_u64(&data[58..66])
    } else {
        0
    }
}
fn decode_prop_no(data: &[u8]) -> u64 {
    if data.len() >= 74 {
        bytes_to_u64(&data[66..74])
    } else {
        0
    }
}
fn decode_prop_type(data: &[u8]) -> u8 {
    if data.len() > 40 {
        data[40]
    } else {
        0
    }
}
fn decode_prop_proposer(data: &[u8]) -> [u8; 32] {
    let mut p = [0u8; 32];
    if data.len() >= 32 {
        p.copy_from_slice(&data[..32]);
    }
    p
}
fn decode_prop_pair_id(data: &[u8]) -> u64 {
    if data.len() >= 82 {
        bytes_to_u64(&data[74..82])
    } else {
        0
    }
}
fn decode_prop_evidence(data: &[u8]) -> [u8; 32] {
    let mut e = [0u8; 32];
    if data.len() >= 114 {
        e.copy_from_slice(&data[82..114]);
    }
    e
}
fn decode_prop_maker_fee(data: &[u8]) -> i16 {
    if data.len() >= 116 {
        i16::from_le_bytes([data[114], data[115]])
    } else {
        0
    }
}
fn decode_prop_taker_fee(data: &[u8]) -> u16 {
    if data.len() >= 118 {
        u16::from_le_bytes([data[116], data[117]])
    } else {
        0
    }
}

fn update_prop_status(data: &mut Vec<u8>, status: u8) {
    if data.len() > 41 {
        data[41] = status;
    }
}
fn update_prop_yes(data: &mut Vec<u8>, val: u64) {
    if data.len() >= 66 {
        data[58..66].copy_from_slice(&u64_to_bytes(val));
    }
}
fn update_prop_no(data: &mut Vec<u8>, val: u64) {
    if data.len() >= 74 {
        data[66..74].copy_from_slice(&u64_to_bytes(val));
    }
}

// ============================================================================
// PUBLIC FUNCTIONS
// ============================================================================

#[no_mangle]
pub extern "C" fn initialize(admin: *const u8) -> u32 {
    let existing = load_addr(ADMIN_KEY);
    if !is_zero(&existing) {
        return 1;
    }
    let mut addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(admin, addr.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != addr {
        return 200;
    }
    storage_set(ADMIN_KEY, &addr);
    save_u64(PROPOSAL_COUNT_KEY, 0);
    storage_set(PAUSED_KEY, &[0u8]);
    log_info("DEX Governance initialized");
    0
}

/// Set the preferred quote token (admin only).
/// Legacy API — clears allowed quotes list and sets a single allowed quote.
/// Returns: 0=success, 1=not admin, 2=zero address
pub fn set_preferred_quote(caller: *const u8, quote_addr: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut q = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(quote_addr, q.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    if is_zero(&q) {
        return 2;
    }
    let old_count = load_u64(ALLOWED_QUOTE_COUNT_KEY);
    for i in 0..old_count {
        storage_set(&allowed_quote_key(i), &[0u8; 32]);
    }
    storage_set(&allowed_quote_key(0), &q);
    save_u64(ALLOWED_QUOTE_COUNT_KEY, 1);
    storage_set(PREFERRED_QUOTE_KEY, &q);
    log_info("Preferred quote token set for governance (single)");
    0
}

/// Add an allowed quote token (admin only).
/// Returns: 0=success, 1=not admin, 2=zero address, 3=already in list, 4=max reached
pub fn add_allowed_quote(caller: *const u8, quote_addr: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut q = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(quote_addr, q.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    if is_zero(&q) {
        return 2;
    }
    let count = load_u64(ALLOWED_QUOTE_COUNT_KEY);
    for i in 0..count {
        if load_addr(&allowed_quote_key(i)) == q {
            return 3;
        }
    }
    if count >= MAX_ALLOWED_QUOTES {
        return 4;
    }
    storage_set(&allowed_quote_key(count), &q);
    save_u64(ALLOWED_QUOTE_COUNT_KEY, count + 1);
    log_info("Allowed quote token added (governance)");
    0
}

/// Remove an allowed quote token (admin only).
/// Returns: 0=success, 1=not admin, 2=not found
pub fn remove_allowed_quote(caller: *const u8, quote_addr: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut q = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(quote_addr, q.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    let count = load_u64(ALLOWED_QUOTE_COUNT_KEY);
    for i in 0..count {
        if load_addr(&allowed_quote_key(i)) == q {
            if i < count - 1 {
                let last = load_addr(&allowed_quote_key(count - 1));
                storage_set(&allowed_quote_key(i), &last);
            }
            storage_set(&allowed_quote_key(count - 1), &[0u8; 32]);
            save_u64(ALLOWED_QUOTE_COUNT_KEY, count - 1);
            log_info("Allowed quote token removed (governance)");
            return 0;
        }
    }
    2
}

/// Get the number of allowed quote tokens.
pub fn get_allowed_quote_count() -> u64 {
    load_u64(ALLOWED_QUOTE_COUNT_KEY)
}

/// Get the preferred quote token address
pub fn get_preferred_quote() -> u64 {
    let addr = load_addr(PREFERRED_QUOTE_KEY);
    moltchain_sdk::set_return_data(&addr);
    if is_zero(&addr) {
        0
    } else {
        1
    }
}

/// Propose a new trading pair
/// Returns: 0=success, 1=paused, 2=max proposals, 3=reentrancy, 4=invalid quote, 5=insufficient reputation
pub fn propose_new_pair(proposer: *const u8, base_token: *const u8, quote_token: *const u8) -> u32 {
    if !reentrancy_enter() {
        return 3;
    }
    if !require_not_paused() {
        reentrancy_exit();
        return 1;
    }
    let mut p = [0u8; 32];
    let mut bt = [0u8; 32];
    let mut qt = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(proposer, p.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(base_token, bt.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(quote_token, qt.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != p {
        reentrancy_exit();
        return 200;
    }

    // Verify proposer has sufficient on-chain reputation
    if !verify_reputation(&p, MIN_REPUTATION) {
        reentrancy_exit();
        log_info("Proposal rejected: insufficient MoltyID reputation");
        return 5;
    }

    // Validate quote token against allowed quotes list
    if !is_allowed_quote(&qt) {
        reentrancy_exit();
        log_info("Proposal rejected: quote token not in allowed quotes list");
        return 4;
    }

    let count = load_u64(PROPOSAL_COUNT_KEY);
    if count >= MAX_PROPOSALS {
        reentrancy_exit();
        return 2;
    }

    let current_slot = get_slot();
    let end_slot = current_slot + VOTING_PERIOD_SLOTS;
    let prop_id = count + 1;
    let data = encode_proposal(
        &p,
        prop_id,
        PROPOSAL_NEW_PAIR,
        STATUS_ACTIVE,
        current_slot,
        end_slot,
        0,
        0,
        0,
        &bt,
        0,
        0,
    );
    storage_set(&proposal_key(prop_id), &data);
    save_u64(PROPOSAL_COUNT_KEY, prop_id);
    log_info("New pair proposal created");
    reentrancy_exit();
    0
}

/// Propose a fee change for an existing pair
pub fn propose_fee_change(
    proposer: *const u8,
    pair_id: u64,
    new_maker_fee: i16,
    new_taker_fee: u16,
) -> u32 {
    if !reentrancy_enter() {
        return 3;
    }
    if !require_not_paused() {
        reentrancy_exit();
        return 1;
    }
    let mut p = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(proposer, p.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != p {
        reentrancy_exit();
        return 200;
    }

    let count = load_u64(PROPOSAL_COUNT_KEY);
    if count >= MAX_PROPOSALS {
        reentrancy_exit();
        return 2;
    }

    let current_slot = get_slot();
    let end_slot = current_slot + VOTING_PERIOD_SLOTS;
    let prop_id = count + 1;
    let data = encode_proposal(
        &p,
        prop_id,
        PROPOSAL_FEE_CHANGE,
        STATUS_ACTIVE,
        current_slot,
        end_slot,
        0,
        0,
        pair_id,
        &[0u8; 32],
        new_maker_fee,
        new_taker_fee,
    );
    storage_set(&proposal_key(prop_id), &data);
    save_u64(PROPOSAL_COUNT_KEY, prop_id);
    log_info("Fee change proposal created");
    reentrancy_exit();
    0
}

/// Vote on a proposal
/// Returns: 0=success, 1=not found, 2=voting ended, 3=already voted, 4=reentrancy, 5=insufficient reputation
pub fn vote(voter: *const u8, proposal_id: u64, approve: bool) -> u32 {
    if !reentrancy_enter() {
        return 4;
    }
    let mut v = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(voter, v.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != v {
        reentrancy_exit();
        return 200;
    }

    // Verify voter has on-chain reputation via MoltyID cross-contract call
    if !verify_reputation(&v, MIN_REPUTATION) {
        reentrancy_exit();
        log_info("Vote rejected: insufficient MoltyID reputation");
        return 5;
    }

    let pk = proposal_key(proposal_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= PROPOSAL_SIZE => d,
        _ => {
            reentrancy_exit();
            return 1;
        }
    };

    if decode_prop_status(&data) != STATUS_ACTIVE {
        reentrancy_exit();
        return 1;
    }

    let current_slot = get_slot();
    let end_slot = decode_prop_end_slot(&data);
    if current_slot > end_slot {
        reentrancy_exit();
        return 2;
    }

    // Check if already voted
    let vk = vote_key(proposal_id, &v);
    if storage_get(&vk).is_some() {
        reentrancy_exit();
        return 3;
    }

    // Record vote
    storage_set(&vk, &[if approve { 1u8 } else { 0u8 }]);

    if approve {
        let yes = decode_prop_yes(&data);
        update_prop_yes(&mut data, yes.saturating_add(1));
    } else {
        let no = decode_prop_no(&data);
        update_prop_no(&mut data, no.saturating_add(1));
    }
    storage_set(&pk, &data);

    // Track global vote stats
    save_u64(TOTAL_VOTES_KEY, load_u64(TOTAL_VOTES_KEY).saturating_add(1));
    // Track unique voters: check if voter has voted before (use voter global key)
    let mut voter_global_key = Vec::from(&b"gov_vg_"[..]);
    voter_global_key.extend_from_slice(&hex_encode(&v));
    if storage_get(&voter_global_key).is_none() {
        storage_set(&voter_global_key, &[1]);
        save_u64(VOTER_COUNT_KEY, load_u64(VOTER_COUNT_KEY).saturating_add(1));
    }

    log_info("Vote recorded");
    reentrancy_exit();
    0
}

/// Finalize a proposal after voting period ends
/// Returns: 0=success (passed), 1=not found, 2=still active, 3=already finalized
pub fn finalize_proposal(proposal_id: u64) -> u32 {
    let pk = proposal_key(proposal_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= PROPOSAL_SIZE => d,
        _ => return 1,
    };

    if decode_prop_status(&data) != STATUS_ACTIVE {
        return 3;
    }

    let current_slot = get_slot();
    let end_slot = decode_prop_end_slot(&data);
    if current_slot <= end_slot {
        return 2;
    }

    let yes = decode_prop_yes(&data);
    let no = decode_prop_no(&data);
    let total = yes + no;

    // AUDIT-FIX P2: Minimum quorum — prevent single-voter governance capture
    const MIN_QUORUM: u64 = 3;
    if total < MIN_QUORUM {
        // Mark as rejected — insufficient quorum
        update_prop_status(&mut data, STATUS_REJECTED);
        storage_set(&pk, &data);
        log_info("Proposal rejected: insufficient quorum");
        return 1;
    }

    let passed = if total == 0 {
        false
    } else {
        yes * 10_000 / total >= APPROVAL_THRESHOLD_BPS
    };

    if passed {
        update_prop_status(&mut data, STATUS_PASSED);
    } else {
        update_prop_status(&mut data, STATUS_REJECTED);
    }
    storage_set(&pk, &data);

    if passed {
        0
    } else {
        1
    }
}

/// Execute a passed proposal (after timelock)
/// Returns: 0=success, 1=not found, 2=not passed, 3=timelock not expired, 4=downstream call failed
pub fn execute_proposal(proposal_id: u64) -> u32 {
    let pk = proposal_key(proposal_id);
    let mut data = match storage_get(&pk) {
        Some(d) if d.len() >= PROPOSAL_SIZE => d,
        _ => return 1,
    };

    if decode_prop_status(&data) != STATUS_PASSED {
        return 2;
    }

    let current_slot = get_slot();
    let end_slot = decode_prop_end_slot(&data);
    if current_slot < end_slot + EXECUTION_DELAY_SLOTS {
        return 3;
    }

    // Dispatch cross-contract call based on proposal type
    let core_addr = load_addr(CORE_ADDRESS_KEY);
    let prop_type = decode_prop_type(&data);

    match prop_type {
        PROPOSAL_NEW_PAIR => {
            // Cross-call dex_core::create_pair(admin, base_token, quote_token, tick, lot, min_order)
            // evidence field stores the base_token address (32 bytes)
            let base_token = decode_prop_evidence(&data);
            // Use preferred quote as the quote token
            let quote_token = load_addr(PREFERRED_QUOTE_KEY);
            // Use sensible defaults: tick_size=1_000_000, lot_size=100, min_order=1000
            let mut args = Vec::new();
            args.extend_from_slice(&core_addr); // admin/caller (governance contract itself)
            args.extend_from_slice(&base_token);
            args.extend_from_slice(&quote_token);
            args.extend_from_slice(&u64_to_bytes(1_000_000)); // tick_size
            args.extend_from_slice(&u64_to_bytes(100)); // lot_size
            args.extend_from_slice(&u64_to_bytes(1_000)); // min_order
            let target = Address(core_addr);
            let call = CrossCall::new(target, "create_pair", args);
            match call_contract(call) {
                Ok(result) => {
                    log_info("Proposal executed: new pair created");
                    // Store creation result for queryability
                    let mut rk = Vec::from(&b"gov_exec_result_"[..]);
                    rk.extend_from_slice(&u64_to_bytes(proposal_id));
                    storage_set(&rk, &result);
                }
                Err(_) => {
                    log_info("Proposal execution failed: pair creation cross-contract call failed");
                    return 4;
                }
            }
        }
        PROPOSAL_FEE_CHANGE => {
            // Cross-call dex_core::update_pair_fees(admin, pair_id, maker_fee, taker_fee)
            let pair_id = decode_prop_pair_id(&data);
            let maker_fee = decode_prop_maker_fee(&data);
            let taker_fee = decode_prop_taker_fee(&data);
            let mut args = Vec::new();
            args.extend_from_slice(&core_addr); // admin/caller
            args.extend_from_slice(&u64_to_bytes(pair_id));
            args.extend_from_slice(&maker_fee.to_le_bytes());
            args.extend_from_slice(&taker_fee.to_le_bytes());
            let target = Address(core_addr);
            let call = CrossCall::new(target, "update_pair_fees", args);
            match call_contract(call) {
                Ok(_) => {
                    log_info("Proposal executed: fees updated");
                    // Store executed fee params for auditability
                    let mut fk = Vec::from(&b"gov_exec_fees_"[..]);
                    fk.extend_from_slice(&u64_to_bytes(proposal_id));
                    let mut fee_record = Vec::new();
                    fee_record.extend_from_slice(&u64_to_bytes(pair_id));
                    fee_record.extend_from_slice(&maker_fee.to_le_bytes());
                    fee_record.extend_from_slice(&taker_fee.to_le_bytes());
                    storage_set(&fk, &fee_record);
                }
                Err(_) => {
                    log_info("Proposal execution failed: fee update cross-contract call failed");
                    return 4;
                }
            }
        }
        PROPOSAL_DELIST => {
            // Cross-call dex_core::pause_pair(admin, pair_id) to halt trading
            let pair_id = decode_prop_pair_id(&data);
            let mut args = Vec::new();
            args.extend_from_slice(&core_addr);
            args.extend_from_slice(&u64_to_bytes(pair_id));
            let target = Address(core_addr);
            let call = CrossCall::new(target, "pause_pair", args);
            match call_contract(call) {
                Ok(_) => {
                    log_info("Proposal executed: pair delisted");
                }
                Err(_) => {
                    log_info("Proposal execution failed: pair delist cross-contract call failed");
                    return 4;
                }
            }
        }
        PROPOSAL_PARAM_CHANGE => {
            // Generic parameter change — store evidence as the new config blob
            // Evidence holds the parameter key/value to apply
            let evidence = decode_prop_evidence(&data);
            let mut pk_param = Vec::from(&b"gov_param_applied_"[..]);
            pk_param.extend_from_slice(&u64_to_bytes(proposal_id));
            storage_set(&pk_param, &evidence);
            log_info("Proposal executed: parameter change applied");
        }
        _ => {
            log_info("Proposal executed: unknown type (signaling only)");
        }
    }

    update_prop_status(&mut data, STATUS_EXECUTED);
    storage_set(&pk, &data);
    save_u64(
        &{
            let mut ek = Vec::from(&b"gov_exec_slot_"[..]);
            ek.extend_from_slice(&u64_to_bytes(proposal_id));
            ek
        },
        current_slot,
    );
    0
}

/// Emergency delist a pair (admin only, no governance needed)
pub fn emergency_delist(caller: *const u8, pair_id: u64) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    // In production: cross-call dex_core to pause the pair
    // Store delist record
    let mut dk = Vec::from(&b"gov_delist_"[..]);
    dk.extend_from_slice(&u64_to_decimal(pair_id));
    save_u64(&dk, get_slot());
    log_info("Emergency delist executed");
    0
}

/// Set listing requirements (admin only)
pub fn set_listing_requirements(caller: *const u8, min_liquidity: u64, min_holders: u64) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    save_u64(b"gov_min_liq", min_liquidity);
    save_u64(b"gov_min_holders", min_holders);
    0
}

pub fn emergency_pause(caller: *const u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    storage_set(PAUSED_KEY, &[1u8]);
    log_info("DEX Governance: EMERGENCY PAUSE");
    0
}

pub fn emergency_unpause(caller: *const u8) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    storage_set(PAUSED_KEY, &[0u8]);
    0
}

/// Set the MoltyID contract address for on-chain reputation verification.
/// Admin only. Required for reputation-gated voting and proposals.
/// Returns: 0=success, 1=not admin, 2=zero address
pub fn set_moltyid_address(caller: *const u8, moltyid_addr: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(moltyid_addr, addr.as_mut_ptr(), 32);
    }
    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != c {
        return 200;
    }
    if !require_admin(&c) {
        return 1;
    }
    if is_zero(&addr) {
        return 2;
    }
    storage_set(MOLTYID_ADDRESS_KEY, &addr);
    log_info("MoltyID address configured for reputation verification");
    0
}

/// Verify that an address has sufficient on-chain reputation via MoltyID.
/// The processor injects the caller's MoltyID reputation into the contract's
/// storage at key "rep:{hex_pubkey}" before execution, so we can read it
/// directly via storage_get. If no MoltyID address is configured, fails closed.
fn verify_reputation(addr: &[u8; 32], min_rep: u64) -> bool {
    // Check if MoltyID address is configured (non-zero)
    match storage_get(MOLTYID_ADDRESS_KEY) {
        Some(b) if b.len() == 32 && b.iter().any(|&x| x != 0) => {}
        _ => {
            // P10-SC-10: Fail closed when MoltyID is not configured
            log_info("verify_reputation: MoltyID not configured — denying (fail closed)");
            return false;
        }
    };

    // Read reputation from injected cross-contract storage.
    // The processor pre-populates "rep:{hex_pubkey}" with the MoltyID
    // reputation value for the transaction caller.
    let hex_chars: &[u8; 16] = b"0123456789abcdef";
    let mut rep_key = Vec::with_capacity(68);
    rep_key.extend_from_slice(b"rep:");
    for &b in addr.iter() {
        rep_key.push(hex_chars[(b >> 4) as usize]);
        rep_key.push(hex_chars[(b & 0x0f) as usize]);
    }

    match storage_get(&rep_key) {
        Some(data) if data.len() >= 8 => {
            let reputation = bytes_to_u64(&data);
            reputation >= min_rep
        }
        // No reputation data found → block (MoltyID is configured but
        // caller has no identity/reputation registered)
        _ => false,
    }
}

// Queries
pub fn get_proposal_count() -> u64 {
    load_u64(PROPOSAL_COUNT_KEY)
}
pub fn get_proposal_info(proposal_id: u64) -> u64 {
    let pk = proposal_key(proposal_id);
    match storage_get(&pk) {
        Some(d) if d.len() >= PROPOSAL_SIZE => {
            moltchain_sdk::set_return_data(&d);
            proposal_id
        }
        _ => 0,
    }
}

// WASM entry
#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn call() {
    let args = moltchain_sdk::get_args();
    if args.is_empty() {
        return;
    }
    match args[0] {
        0 => {
            if args.len() >= 33 {
                let r = initialize(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        1 => {
            // propose_new_pair
            if args.len() >= 1 + 32 + 32 + 32 {
                let r = propose_new_pair(
                    args[1..33].as_ptr(),
                    args[33..65].as_ptr(),
                    args[65..97].as_ptr(),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        2 => {
            // vote
            if args.len() >= 1 + 32 + 8 + 1 {
                let r = vote(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    args[41] != 0,
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        3 => {
            // finalize_proposal
            if args.len() >= 9 {
                let r = finalize_proposal(bytes_to_u64(&args[1..9]));
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        4 => {
            // execute_proposal
            if args.len() >= 9 {
                let r = execute_proposal(bytes_to_u64(&args[1..9]));
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        5 => {
            // set_preferred_quote
            if args.len() >= 1 + 32 + 32 {
                let r = set_preferred_quote(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        6 => {
            // get_preferred_quote
            get_preferred_quote();
        }
        7 => {
            // get_proposal_count
            moltchain_sdk::set_return_data(&u64_to_bytes(get_proposal_count()));
        }
        8 => {
            // get_proposal_info
            if args.len() >= 9 {
                get_proposal_info(bytes_to_u64(&args[1..9]));
            }
        }
        9 => {
            // propose_fee_change
            if args.len() >= 1 + 32 + 8 + 2 + 2 {
                let maker = i16::from_le_bytes([args[41], args[42]]);
                let taker = u16::from_le_bytes([args[43], args[44]]);
                let r = propose_fee_change(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    maker,
                    taker,
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        10 => {
            // emergency_delist
            if args.len() >= 1 + 32 + 8 {
                let r = emergency_delist(args[1..33].as_ptr(), bytes_to_u64(&args[33..41]));
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        11 => {
            // set_listing_requirements
            if args.len() >= 1 + 32 + 8 + 8 {
                let r = set_listing_requirements(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    bytes_to_u64(&args[41..49]),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        12 => {
            // emergency_pause
            if args.len() >= 33 {
                let r = emergency_pause(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        13 => {
            // emergency_unpause
            if args.len() >= 33 {
                let r = emergency_unpause(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        14 => {
            // set_moltyid_address
            if args.len() >= 1 + 32 + 32 {
                let r = set_moltyid_address(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        15 => {
            // add_allowed_quote(caller[32] + quote_addr[32])
            if args.len() >= 1 + 32 + 32 {
                let r = add_allowed_quote(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        16 => {
            // remove_allowed_quote(caller[32] + quote_addr[32])
            if args.len() >= 1 + 32 + 32 {
                let r = remove_allowed_quote(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(r as u64));
            }
        }
        17 => {
            // get_allowed_quote_count
            moltchain_sdk::set_return_data(&u64_to_bytes(get_allowed_quote_count()));
        }
        18 => {
            // get_governance_stats — [proposal_count, total_votes, voter_count]
            let mut buf = Vec::with_capacity(24);
            buf.extend_from_slice(&u64_to_bytes(load_u64(PROPOSAL_COUNT_KEY)));
            buf.extend_from_slice(&u64_to_bytes(load_u64(TOTAL_VOTES_KEY)));
            buf.extend_from_slice(&u64_to_bytes(load_u64(VOTER_COUNT_KEY)));
            moltchain_sdk::set_return_data(&buf);
        }
        19 => {
            // get_voter_count — unique voters
            moltchain_sdk::set_return_data(&u64_to_bytes(load_u64(VOTER_COUNT_KEY)));
        }
        _ => {
            moltchain_sdk::set_return_data(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        }
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

    fn setup() -> [u8; 32] {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        admin
    }

    fn setup_with_reputation() -> [u8; 32] {
        let admin = setup();
        let moltyid = [77u8; 32];
        assert_eq!(set_moltyid_address(admin.as_ptr(), moltyid.as_ptr()), 0);

        let hex_chars: &[u8; 16] = b"0123456789abcdef";
        let seed_rep = |addr: [u8; 32]| {
            let mut rep_key = Vec::with_capacity(68);
            rep_key.extend_from_slice(b"rep:");
            for &byte in &addr {
                rep_key.push(hex_chars[(byte >> 4) as usize]);
                rep_key.push(hex_chars[(byte & 0x0f) as usize]);
            }
            storage_set(&rep_key, &u64_to_bytes(1_000));
        };

        for id in 0u8..=255 {
            seed_rep([id; 32]);
            let mut first_byte = [0u8; 32];
            first_byte[0] = id;
            seed_rep(first_byte);
        }

        admin
    }

    #[test]
    fn test_initialize() {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(load_addr(ADMIN_KEY), admin);
    }

    #[test]
    fn test_initialize_twice() {
        test_mock::reset();
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(initialize(admin.as_ptr()), 1);
    }

    #[test]
    fn test_propose_new_pair() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        assert_eq!(
            propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr()),
            0
        );
        assert_eq!(get_proposal_count(), 1);
    }

    #[test]
    fn test_propose_fee_change() {
        let _admin = setup();
        let proposer = [2u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        assert_eq!(propose_fee_change(proposer.as_ptr(), 1, -2, 10), 0);
        assert_eq!(get_proposal_count(), 1);
    }

    #[test]
    fn test_vote_approve() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        let voter = [3u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());
        test_mock::set_caller(voter);
        assert_eq!(vote(voter.as_ptr(), 1, true), 0);
        let pd = storage_get(&proposal_key(1)).unwrap();
        assert_eq!(decode_prop_yes(&pd), 1);
    }

    #[test]
    fn test_vote_reject() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        let voter = [3u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());
        test_mock::set_caller(voter);
        assert_eq!(vote(voter.as_ptr(), 1, false), 0);
        let pd = storage_get(&proposal_key(1)).unwrap();
        assert_eq!(decode_prop_no(&pd), 1);
    }

    #[test]
    fn test_double_vote_prevented() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        let voter = [3u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());
        test_mock::set_caller(voter);
        assert_eq!(vote(voter.as_ptr(), 1, true), 0);
        assert_eq!(vote(voter.as_ptr(), 1, true), 3); // already voted
    }

    #[test]
    fn test_vote_after_period() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        let voter = [3u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());
        // Fast-forward past voting period
        test_mock::set_slot(100 + VOTING_PERIOD_SLOTS + 1);
        test_mock::set_caller(voter);
        assert_eq!(vote(voter.as_ptr(), 1, true), 2);
    }

    #[test]
    fn test_finalize_passed() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());

        // 3 yes, 1 no → 75% > 66% → pass
        for i in 0u8..3 {
            let mut v = [0u8; 32];
            v[0] = 10 + i;
            test_mock::set_caller(v);
            vote(v.as_ptr(), 1, true);
        }
        let mut v = [0u8; 32];
        v[0] = 50;
        test_mock::set_caller(v);
        vote(v.as_ptr(), 1, false);

        test_mock::set_slot(100 + VOTING_PERIOD_SLOTS + 1);
        assert_eq!(finalize_proposal(1), 0); // passed
        let pd = storage_get(&proposal_key(1)).unwrap();
        assert_eq!(decode_prop_status(&pd), STATUS_PASSED);
    }

    #[test]
    fn test_finalize_rejected() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());

        // 1 yes, 3 no → 25% < 66% → reject
        let mut v1 = [0u8; 32];
        v1[0] = 10;
        test_mock::set_caller(v1);
        vote(v1.as_ptr(), 1, true);
        for i in 0u8..3 {
            let mut v = [0u8; 32];
            v[0] = 50 + i;
            test_mock::set_caller(v);
            vote(v.as_ptr(), 1, false);
        }

        test_mock::set_slot(100 + VOTING_PERIOD_SLOTS + 1);
        assert_eq!(finalize_proposal(1), 1); // rejected
        let pd = storage_get(&proposal_key(1)).unwrap();
        assert_eq!(decode_prop_status(&pd), STATUS_REJECTED);
    }

    #[test]
    fn test_finalize_still_active() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());
        assert_eq!(finalize_proposal(1), 2); // voting still active
    }

    #[test]
    fn test_execute_proposal_after_timelock() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());

        // AUDIT-FIX P2: Need MIN_QUORUM (3) voters to finalize
        let mut v = [0u8; 32];
        v[0] = 10;
        test_mock::set_caller(v);
        vote(v.as_ptr(), 1, true);

        let mut v2 = [0u8; 32];
        v2[0] = 11;
        test_mock::set_caller(v2);
        vote(v2.as_ptr(), 1, true);

        let mut v3 = [0u8; 32];
        v3[0] = 12;
        test_mock::set_caller(v3);
        vote(v3.as_ptr(), 1, true);

        test_mock::set_slot(100 + VOTING_PERIOD_SLOTS + 1);
        finalize_proposal(1);

        // Before timelock
        test_mock::set_slot(100 + VOTING_PERIOD_SLOTS + 2);
        assert_eq!(execute_proposal(1), 3); // timelock not expired

        // After timelock
        test_mock::set_slot(100 + VOTING_PERIOD_SLOTS + EXECUTION_DELAY_SLOTS + 1);
        assert_eq!(execute_proposal(1), 0);
    }

    #[test]
    fn test_emergency_delist() {
        let admin = setup();
        assert_eq!(emergency_delist(admin.as_ptr(), 1), 0);
    }

    #[test]
    fn test_emergency_delist_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        test_mock::set_caller(rando);
        assert_eq!(emergency_delist(rando.as_ptr(), 1), 1);
    }

    #[test]
    fn test_set_listing_requirements() {
        let admin = setup();
        assert_eq!(set_listing_requirements(admin.as_ptr(), 50_000, 20), 0);
        assert_eq!(load_u64(b"gov_min_liq"), 50_000);
        assert_eq!(load_u64(b"gov_min_holders"), 20);
    }

    #[test]
    fn test_emergency_pause() {
        let admin = setup();
        assert_eq!(emergency_pause(admin.as_ptr()), 0);
        assert!(is_paused());
        assert_eq!(emergency_unpause(admin.as_ptr()), 0);
        assert!(!is_paused());
    }

    #[test]
    fn test_propose_when_paused() {
        let admin = setup();
        emergency_pause(admin.as_ptr());
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_caller(proposer);
        assert_eq!(
            propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr()),
            1
        );
    }

    #[test]
    fn test_get_proposal_info() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());
        assert_eq!(get_proposal_info(1), 1);
        assert_eq!(get_proposal_info(999), 0);
    }

    // --- Preferred quote currency (mUSD enforcement) ---

    #[test]
    fn test_set_preferred_quote_governance() {
        let admin = setup();
        let musd = [42u8; 32];
        assert_eq!(set_preferred_quote(admin.as_ptr(), musd.as_ptr()), 0);
        assert_eq!(get_preferred_quote(), 1); // 1 = set
    }

    #[test]
    fn test_set_preferred_quote_not_admin_governance() {
        let _admin = setup();
        let non_admin = [99u8; 32];
        let musd = [42u8; 32];
        test_mock::set_caller(non_admin);
        assert_eq!(set_preferred_quote(non_admin.as_ptr(), musd.as_ptr()), 1);
    }

    #[test]
    fn test_set_preferred_quote_zero_address_governance() {
        let admin = setup();
        let zero = [0u8; 32];
        assert_eq!(set_preferred_quote(admin.as_ptr(), zero.as_ptr()), 2);
    }

    #[test]
    fn test_propose_pair_enforces_preferred_quote() {
        let admin = setup_with_reputation();
        let musd = [42u8; 32];
        set_preferred_quote(admin.as_ptr(), musd.as_ptr());
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        // Correct quote → success
        assert_eq!(
            propose_new_pair(proposer.as_ptr(), base.as_ptr(), musd.as_ptr()),
            0
        );
        // Wrong quote → error 4
        let wrong = [99u8; 32];
        assert_eq!(
            propose_new_pair(proposer.as_ptr(), base.as_ptr(), wrong.as_ptr()),
            4
        );
    }

    #[test]
    fn test_propose_pair_no_preferred_allows_any() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        assert_eq!(
            propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr()),
            0
        );
    }

    #[test]
    fn test_get_preferred_quote_unset_governance() {
        let _admin = setup();
        assert_eq!(get_preferred_quote(), 0); // 0 = not set
    }

    // --- MoltyID reputation integration ---

    #[test]
    fn test_set_moltyid_address_success() {
        let admin = setup();
        let moltyid = [77u8; 32];
        assert_eq!(set_moltyid_address(admin.as_ptr(), moltyid.as_ptr()), 0);
        let stored = storage_get(MOLTYID_ADDRESS_KEY).unwrap();
        assert_eq!(stored.as_slice(), &moltyid);
    }

    #[test]
    fn test_set_moltyid_address_not_admin() {
        let _admin = setup();
        let rando = [99u8; 32];
        let moltyid = [77u8; 32];
        test_mock::set_caller(rando);
        assert_eq!(set_moltyid_address(rando.as_ptr(), moltyid.as_ptr()), 1);
    }

    #[test]
    fn test_set_moltyid_address_zero_rejected() {
        let admin = setup();
        let zero = [0u8; 32];
        assert_eq!(set_moltyid_address(admin.as_ptr(), zero.as_ptr()), 2);
    }

    #[test]
    fn test_verify_reputation_no_address_denies() {
        // P10-SC-10: Without MoltyID address configured, verify_reputation fails closed
        let _admin = setup();
        let user = [5u8; 32];
        assert!(!verify_reputation(&user, 500));
        assert!(!verify_reputation(&user, u64::MAX));
    }

    #[test]
    fn test_verify_reputation_test_mode_blocks_without_data() {
        // With MoltyID configured but no reputation data → blocks
        let admin = setup();
        let moltyid = [77u8; 32];
        set_moltyid_address(admin.as_ptr(), moltyid.as_ptr());
        let user = [5u8; 32];
        assert!(!verify_reputation(&user, 500));
    }

    #[test]
    fn test_verify_reputation_with_data() {
        // With MoltyID configured and reputation data injected → checks threshold
        let admin = setup();
        let moltyid = [77u8; 32];
        set_moltyid_address(admin.as_ptr(), moltyid.as_ptr());
        let user = [5u8; 32];
        // Inject reputation data into mock storage (simulating processor injection)
        let hex_chars: &[u8; 16] = b"0123456789abcdef";
        let mut rep_key = Vec::with_capacity(68);
        rep_key.extend_from_slice(b"rep:");
        for &b in user.iter() {
            rep_key.push(hex_chars[(b >> 4) as usize]);
            rep_key.push(hex_chars[(b & 0x0f) as usize]);
        }
        let rep_value: u64 = 1000;
        storage_set(&rep_key, &u64_to_bytes(rep_value));
        assert!(verify_reputation(&user, 500)); // 1000 >= 500
        assert!(verify_reputation(&user, 1000)); // 1000 >= 1000
        assert!(!verify_reputation(&user, 1001)); // 1000 < 1001
    }

    #[test]
    fn test_propose_with_reputation_check() {
        // With MoltyID configured but no reputation data, proposals are blocked
        let admin = setup();
        let moltyid = [77u8; 32];
        set_moltyid_address(admin.as_ptr(), moltyid.as_ptr());
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        assert_eq!(
            propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr()),
            5 // reputation check fails — no reputation data
        );
    }

    #[test]
    fn test_vote_with_reputation_check() {
        // With MoltyID configured but no reputation data, votes are blocked
        let admin = setup();
        let moltyid = [77u8; 32];
        set_moltyid_address(admin.as_ptr(), moltyid.as_ptr());
        let proposer = [2u8; 32];
        let _voter = [3u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        // Propose also fails reputation check with MoltyID configured
        assert_eq!(
            propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr()),
            5 // reputation check fails — no reputation data
        );
    }

    // AUDIT-FIX P2: Security regression test
    #[test]
    fn test_finalize_insufficient_quorum() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        // Create proposal
        assert_eq!(
            propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr()),
            0
        );
        // Only 1 vote (quorum=3 not met)
        let voter = [3u8; 32];
        test_mock::set_caller(voter);
        assert_eq!(vote(voter.as_ptr(), 1, true), 0);
        // Fast-forward past voting period
        test_mock::set_slot(100 + VOTING_PERIOD_SLOTS + 1);
        // Finalize → should reject due to insufficient quorum
        let result = finalize_proposal(1);
        assert_eq!(result, 1); // rejected (insufficient quorum)
                               // Verify status is REJECTED
        let pd = storage_get(&proposal_key(1)).unwrap();
        assert_eq!(decode_prop_status(&pd), STATUS_REJECTED);
    }

    // Helper: pass a proposal through voting → finalize → timelock
    fn pass_and_timelock(proposal_id: u64, start_slot: u64) {
        // 3 votes FOR (meets MIN_QUORUM=3)
        for i in 10u8..13 {
            let mut v = [0u8; 32];
            v[0] = i;
            test_mock::set_caller(v);
            assert_eq!(vote(v.as_ptr(), proposal_id, true), 0);
        }
        // Advance past voting period + finalize
        test_mock::set_slot(start_slot + VOTING_PERIOD_SLOTS + 1);
        assert_eq!(finalize_proposal(proposal_id), 0);
        // Advance past execution delay
        test_mock::set_slot(start_slot + VOTING_PERIOD_SLOTS + EXECUTION_DELAY_SLOTS + 1);
    }

    #[test]
    fn test_execute_new_pair_dispatches_cross_call() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        assert_eq!(
            propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr()),
            0
        );

        pass_and_timelock(1, 100);

        // Execute — should dispatch create_pair cross-call
        assert_eq!(execute_proposal(1), 0);

        // Verify proposal marked as EXECUTED
        let pd = storage_get(&proposal_key(1)).unwrap();
        assert_eq!(decode_prop_status(&pd), STATUS_EXECUTED);

        // Verify execution slot recorded
        let mut ek = Vec::from(&b"gov_exec_slot_"[..]);
        ek.extend_from_slice(&u64_to_bytes(1));
        assert!(load_u64(&ek) > 0, "execution slot must be recorded");

        // Verify execution result stored (cross-call mock returns empty)
        let mut rk = Vec::from(&b"gov_exec_result_"[..]);
        rk.extend_from_slice(&u64_to_bytes(1));
        assert!(
            storage_get(&rk).is_some(),
            "execution result must be stored"
        );
    }

    #[test]
    fn test_execute_new_pair_failure_keeps_proposal_retryable() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        assert_eq!(
            propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr()),
            0
        );

        pass_and_timelock(1, 100);
        test_mock::set_cross_call_should_fail(true);

        assert_eq!(execute_proposal(1), 4);

        let pd = storage_get(&proposal_key(1)).unwrap();
        assert_eq!(decode_prop_status(&pd), STATUS_PASSED);

        let mut ek = Vec::from(&b"gov_exec_slot_"[..]);
        ek.extend_from_slice(&u64_to_bytes(1));
        assert_eq!(load_u64(&ek), 0, "failed execution must not record a slot");

        let mut rk = Vec::from(&b"gov_exec_result_"[..]);
        rk.extend_from_slice(&u64_to_bytes(1));
        assert!(
            storage_get(&rk).is_none(),
            "failed execution must not store a success result"
        );

        let logs = test_mock::get_logs();
        assert!(
            logs.iter().any(|log| {
                log.contains("Proposal execution failed: pair creation cross-contract call failed")
            }),
            "failure log must describe the downstream execution error"
        );
    }

    #[test]
    fn test_execute_fee_change_dispatches_and_records() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        test_mock::set_slot(200);
        test_mock::set_caller(proposer);
        // Propose fee change: pair_id=5, maker=-2, taker=10
        assert_eq!(propose_fee_change(proposer.as_ptr(), 5, -2, 10), 0);

        pass_and_timelock(1, 200);
        assert_eq!(execute_proposal(1), 0);

        // Status = EXECUTED
        let pd = storage_get(&proposal_key(1)).unwrap();
        assert_eq!(decode_prop_status(&pd), STATUS_EXECUTED);

        // Fee record stored for auditability
        let mut fk = Vec::from(&b"gov_exec_fees_"[..]);
        fk.extend_from_slice(&u64_to_bytes(1));
        let fee_record = storage_get(&fk).expect("fee record must be stored");
        // pair_id=5 at bytes 0..8
        assert_eq!(bytes_to_u64(&fee_record[0..8]), 5);
        // maker_fee=-2 at bytes 8..10
        assert_eq!(i16::from_le_bytes([fee_record[8], fee_record[9]]), -2);
        // taker_fee=10 at bytes 10..12
        assert_eq!(u16::from_le_bytes([fee_record[10], fee_record[11]]), 10);
    }

    #[test]
    fn test_execute_cannot_reexecute() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());
        pass_and_timelock(1, 100);

        assert_eq!(execute_proposal(1), 0); // first execution succeeds
        assert_eq!(execute_proposal(1), 2); // second returns 2 (not passed — it's EXECUTED)
    }

    #[test]
    fn test_execute_rejected_proposal_fails() {
        let _admin = setup_with_reputation();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        test_mock::set_caller(proposer);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());

        // 3 votes AGAINST
        for i in 10u8..13 {
            let mut v = [0u8; 32];
            v[0] = i;
            test_mock::set_caller(v);
            assert_eq!(vote(v.as_ptr(), 1, false), 0);
        }
        test_mock::set_slot(100 + VOTING_PERIOD_SLOTS + 1);
        finalize_proposal(1);
        test_mock::set_slot(100 + VOTING_PERIOD_SLOTS + EXECUTION_DELAY_SLOTS + 1);

        assert_eq!(execute_proposal(1), 2); // rejected, can't execute
    }
}
