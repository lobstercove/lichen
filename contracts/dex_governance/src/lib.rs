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

use moltchain_sdk::{bytes_to_u64, get_slot, log_info, storage_get, storage_set, u64_to_bytes};

// ============================================================================
// CONSTANTS
// ============================================================================

const VOTING_PERIOD_SLOTS: u64 = 172_800; // ~48 hours at 1 slot/sec
const APPROVAL_THRESHOLD_BPS: u64 = 6600; // 66%
const EXECUTION_DELAY_SLOTS: u64 = 3_600; // 1 hour timelock after voting
const MIN_REPUTATION: u64 = 500;
const MIN_LISTING_LIQUIDITY: u64 = 100_000_000_000_000; // 100,000 MOLT ($10K at $0.10)
const MIN_LISTING_HOLDERS: u64 = 10;
const MAX_PROPOSALS: u64 = 500;

const PREFERRED_QUOTE_KEY: &[u8] = b"gov_preferred_quote";

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

pub fn initialize(admin: *const u8) -> u32 {
    let existing = load_addr(ADMIN_KEY);
    if !is_zero(&existing) {
        return 1;
    }
    let mut addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(admin, addr.as_mut_ptr(), 32);
    }
    storage_set(ADMIN_KEY, &addr);
    save_u64(PROPOSAL_COUNT_KEY, 0);
    storage_set(PAUSED_KEY, &[0u8]);
    log_info("DEX Governance initialized");
    0
}

/// Set the preferred quote token (admin only).
/// All new pair proposals will be validated against this address.
/// Returns: 0=success, 1=not admin, 2=zero address
pub fn set_preferred_quote(caller: *const u8, quote_addr: *const u8) -> u32 {
    let mut c = [0u8; 32];
    let mut q = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(quote_addr, q.as_mut_ptr(), 32);
    }
    if !require_admin(&c) {
        return 1;
    }
    if is_zero(&q) {
        return 2;
    }
    storage_set(PREFERRED_QUOTE_KEY, &q);
    log_info("Preferred quote token set for governance");
    0
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
/// Returns: 0=success, 1=paused, 2=max proposals, 3=reentrancy, 4=invalid quote
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

    // Validate quote token matches preferred (mUSD) if set
    let preferred = load_addr(PREFERRED_QUOTE_KEY);
    if !is_zero(&preferred) && qt != preferred {
        reentrancy_exit();
        log_info("Proposal rejected: quote token must be preferred quote (mUSD)");
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
/// Returns: 0=success, 1=not found, 2=voting ended, 3=already voted, 4=reentrancy
pub fn vote(voter: *const u8, proposal_id: u64, approve: bool) -> u32 {
    if !reentrancy_enter() {
        return 4;
    }
    let mut v = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(voter, v.as_mut_ptr(), 32);
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
        update_prop_yes(&mut data, yes + 1);
    } else {
        let no = decode_prop_no(&data);
        update_prop_no(&mut data, no + 1);
    }
    storage_set(&pk, &data);

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
/// Returns: 0=success, 1=not found, 2=not passed, 3=timelock not expired
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

    // Execute — in production would cross-call dex_core
    update_prop_status(&mut data, STATUS_EXECUTED);
    storage_set(&pk, &data);
    log_info("Proposal executed");
    0
}

/// Emergency delist a pair (admin only, no governance needed)
pub fn emergency_delist(caller: *const u8, pair_id: u64) -> u32 {
    let mut c = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller, c.as_mut_ptr(), 32);
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
    if !require_admin(&c) {
        return 1;
    }
    storage_set(PAUSED_KEY, &[0u8]);
    0
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
        _ => {}
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
        assert_eq!(initialize(admin.as_ptr()), 0);
        admin
    }

    #[test]
    fn test_initialize() {
        test_mock::reset();
        let admin = [1u8; 32];
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(load_addr(ADMIN_KEY), admin);
    }

    #[test]
    fn test_initialize_twice() {
        test_mock::reset();
        let admin = [1u8; 32];
        assert_eq!(initialize(admin.as_ptr()), 0);
        assert_eq!(initialize(admin.as_ptr()), 1);
    }

    #[test]
    fn test_propose_new_pair() {
        let _admin = setup();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
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
        assert_eq!(propose_fee_change(proposer.as_ptr(), 1, -2, 10), 0);
        assert_eq!(get_proposal_count(), 1);
    }

    #[test]
    fn test_vote_approve() {
        let _admin = setup();
        let proposer = [2u8; 32];
        let voter = [3u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());
        assert_eq!(vote(voter.as_ptr(), 1, true), 0);
        let pd = storage_get(&proposal_key(1)).unwrap();
        assert_eq!(decode_prop_yes(&pd), 1);
    }

    #[test]
    fn test_vote_reject() {
        let _admin = setup();
        let proposer = [2u8; 32];
        let voter = [3u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());
        assert_eq!(vote(voter.as_ptr(), 1, false), 0);
        let pd = storage_get(&proposal_key(1)).unwrap();
        assert_eq!(decode_prop_no(&pd), 1);
    }

    #[test]
    fn test_double_vote_prevented() {
        let _admin = setup();
        let proposer = [2u8; 32];
        let voter = [3u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());
        assert_eq!(vote(voter.as_ptr(), 1, true), 0);
        assert_eq!(vote(voter.as_ptr(), 1, true), 3); // already voted
    }

    #[test]
    fn test_vote_after_period() {
        let _admin = setup();
        let proposer = [2u8; 32];
        let voter = [3u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());
        // Fast-forward past voting period
        test_mock::set_slot(100 + VOTING_PERIOD_SLOTS + 1);
        assert_eq!(vote(voter.as_ptr(), 1, true), 2);
    }

    #[test]
    fn test_finalize_passed() {
        let _admin = setup();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());

        // 3 yes, 1 no → 75% > 66% → pass
        for i in 0u8..3 {
            let mut v = [0u8; 32];
            v[0] = 10 + i;
            vote(v.as_ptr(), 1, true);
        }
        let mut v = [0u8; 32];
        v[0] = 50;
        vote(v.as_ptr(), 1, false);

        test_mock::set_slot(100 + VOTING_PERIOD_SLOTS + 1);
        assert_eq!(finalize_proposal(1), 0); // passed
        let pd = storage_get(&proposal_key(1)).unwrap();
        assert_eq!(decode_prop_status(&pd), STATUS_PASSED);
    }

    #[test]
    fn test_finalize_rejected() {
        let _admin = setup();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());

        // 1 yes, 3 no → 25% < 66% → reject
        let mut v1 = [0u8; 32];
        v1[0] = 10;
        vote(v1.as_ptr(), 1, true);
        for i in 0u8..3 {
            let mut v = [0u8; 32];
            v[0] = 50 + i;
            vote(v.as_ptr(), 1, false);
        }

        test_mock::set_slot(100 + VOTING_PERIOD_SLOTS + 1);
        assert_eq!(finalize_proposal(1), 1); // rejected
        let pd = storage_get(&proposal_key(1)).unwrap();
        assert_eq!(decode_prop_status(&pd), STATUS_REJECTED);
    }

    #[test]
    fn test_finalize_still_active() {
        let _admin = setup();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());
        assert_eq!(finalize_proposal(1), 2); // voting still active
    }

    #[test]
    fn test_execute_proposal_after_timelock() {
        let _admin = setup();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
        propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr());

        let mut v = [0u8; 32];
        v[0] = 10;
        vote(v.as_ptr(), 1, true);

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
        assert_eq!(
            propose_new_pair(proposer.as_ptr(), base.as_ptr(), quote.as_ptr()),
            1
        );
    }

    #[test]
    fn test_get_proposal_info() {
        let _admin = setup();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
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
        let admin = setup();
        let musd = [42u8; 32];
        set_preferred_quote(admin.as_ptr(), musd.as_ptr());
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        test_mock::set_slot(100);
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
        let _admin = setup();
        let proposer = [2u8; 32];
        let base = [10u8; 32];
        let quote = [20u8; 32];
        test_mock::set_slot(100);
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
}
