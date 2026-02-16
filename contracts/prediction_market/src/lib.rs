// PredictionReef — Prediction Markets on MoltChain
//
// Single contract handling:
//   - Market creation and configuration
//   - Share minting and redemption (complete sets)
//   - Integrated CPMM AMM for each market (binary & multi-outcome)
//   - Resolution and settlement via MoltOracle attestation
//   - Dispute handling with bond escrow + DAO escalation
//   - LP incentives and fee distribution
//
// Storage layout (prefixed pm_):
//   pm_admin                             → [u8; 32]  Admin pubkey
//   pm_paused                            → u8        Emergency pause flag
//   pm_reentrancy                        → u8        Reentrancy guard
//   pm_market_count                      → u64       Global market counter
//   pm_open_markets                      → u64       Currently active markets
//   pm_total_volume                      → u64       Platform lifetime volume
//   pm_total_collateral                  → u64       Current total collateral locked
//   pm_fees_collected                    → u64       Platform fees accumulated
//   pm_moltyid_addr                      → [u8; 32]  MoltyID contract address
//   pm_oracle_addr                       → [u8; 32]  MoltOracle contract address
//   pm_musd_addr                         → [u8; 32]  mUSD token contract address
//   pm_dex_gov_addr                      → [u8; 32]  DEX governance address (disputes)
//   pm_m_{id}                            → [u8; 192] Market record
//   pm_q_{id}                            → Vec<u8>   Question text (UTF-8, up to 512)
//   pm_o_{id}_{outcome}                  → [u8; 64]  Outcome pool data
//   pm_on_{id}_{outcome}                 → Vec<u8>   Outcome name/label (up to 64)
//   pm_p_{id}_{addr_hex}_{outcome}       → [u8; 16]  Position (shares + cost_basis)
//   pm_lp_{id}_{addr_hex}                → u64       LP shares for market
//   pm_cat_{category}_{idx}              → u64       Market ID by category index
//   pm_catc_{category}                   → u64       Count per category
//   pm_active_{idx}                      → u64       Active market IDs (frontpage)
//   pm_user_{addr_hex}_{idx}             → u64       Market IDs user participated in
//   pm_userc_{addr_hex}                  → u64       User's market participation count

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]
#![allow(dead_code)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::manual_is_multiple_of)]
#![allow(unused_variables)]

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

use moltchain_sdk::{bytes_to_u64, get_slot, log_info, storage_get, storage_set, u64_to_bytes};

// ============================================================================
// CONSTANTS
// ============================================================================

// --- Market limits ---
const MAX_OUTCOMES: u8 = 8;
const MAX_MARKETS: u64 = 100_000;
const MAX_OPEN_MARKETS: u64 = 10_000;
const MIN_COLLATERAL: u64 = 1_000_000;       // 1 mUSD (6 decimals)
const MAX_COLLATERAL: u64 = 100_000_000_000;  // 100K mUSD

// --- Timing (in slots; 1 slot ≈ 0.5s) ---
const MIN_DURATION: u64 = 7_200;             // 1 hour minimum
const MAX_DURATION: u64 = 63_072_000;        // 1 year maximum
const RESOLUTION_TIMEOUT: u64 = 604_800;     // 7 days to resolve after close
const DISPUTE_PERIOD: u64 = 172_800;         // 48 hours to challenge resolution
const EMERGENCY_TIMEOUT: u64 = 2_592_000;    // 30 days — auto-void if unresolved

// --- Fees (basis points / flat) ---
const MARKET_CREATION_FEE: u64 = 10_000_000; // 10 mUSD (anti-spam)
const TRADING_FEE_BPS: u64 = 200;            // 2% on AMM swaps
const RESOLUTION_REWARD_BPS: u64 = 50;       // 0.5% of pool to resolver
const LP_FEE_BPS: u64 = 100;                 // 1% to liquidity providers

// Fee distribution (% of trading fee)
const FEE_LP_SHARE: u64 = 50;               // 50% to LPs
const FEE_PROTOCOL_SHARE: u64 = 30;         // 30% to protocol
const FEE_STAKER_SHARE: u64 = 20;           // 20% to stakers

// --- AMM ---
const INITIAL_LIQUIDITY_MIN: u64 = 1_000;   // Min initial liquidity per outcome (shares)
const MIN_SHARE_PRICE: u64 = 10_000;        // $0.01 minimum (6 decimal mUSD)
const MAX_SHARE_PRICE: u64 = 990_000;       // $0.99 maximum
const MUSD_UNIT: u64 = 1_000_000;           // 1 mUSD = 10^6

// --- Reputation ---
const MIN_REPUTATION_CREATE: u64 = 500;
const MIN_REPUTATION_RESOLVE: u64 = 1000;
const DISPUTE_BOND: u64 = 100_000_000;      // 100 mUSD bond to dispute

// --- Resolution ---
const RESOLUTION_THRESHOLD: u8 = 3;         // Min oracle attestations

// --- Circuit breakers ---
const CIRCUIT_BREAKER_COLLATERAL: u64 = 50_000_000_000; // 50K mUSD per market
const CIRCUIT_BREAKER_PLATFORM: u64 = 1_000_000_000_000; // 1M mUSD total
const PRICE_MOVE_PAUSE_BPS: u64 = 5_000;    // 50% in single slot → pause
const PRICE_MOVE_PAUSE_SLOTS: u64 = 120;    // 60 seconds (at 0.5s/slot)

// --- Market statuses ---
const STATUS_PENDING: u8 = 0;
const STATUS_ACTIVE: u8 = 1;
const STATUS_CLOSED: u8 = 2;
const STATUS_RESOLVING: u8 = 3;
const STATUS_RESOLVED: u8 = 4;
const STATUS_DISPUTED: u8 = 5;
const STATUS_VOIDED: u8 = 6;

// --- Categories ---
const CATEGORY_POLITICS: u8 = 0;
const CATEGORY_SPORTS: u8 = 1;
const CATEGORY_CRYPTO: u8 = 2;
const CATEGORY_SCIENCE: u8 = 3;
const CATEGORY_ENTERTAINMENT: u8 = 4;
const CATEGORY_ECONOMICS: u8 = 5;
const CATEGORY_TECH: u8 = 6;
const CATEGORY_CUSTOM: u8 = 7;
const MAX_CATEGORY: u8 = 7;

// --- Outcome constants ---
const UNRESOLVED: u8 = 0xFF;

// --- Sizes ---
const MARKET_RECORD_SIZE: usize = 192;
const OUTCOME_POOL_SIZE: usize = 64;
const POSITION_SIZE: usize = 16;
const MAX_QUESTION_LEN: usize = 512;
const MAX_OUTCOME_NAME_LEN: usize = 64;

// --- Storage key prefixes ---
const ADMIN_KEY: &[u8] = b"pm_admin";
const PAUSED_KEY: &[u8] = b"pm_paused";
const REENTRANCY_KEY: &[u8] = b"pm_reentrancy";
const MARKET_COUNT_KEY: &[u8] = b"pm_market_count";
const OPEN_MARKETS_KEY: &[u8] = b"pm_open_markets";
const TOTAL_VOLUME_KEY: &[u8] = b"pm_total_volume";
const TOTAL_COLLATERAL_KEY: &[u8] = b"pm_total_collateral";
const FEES_COLLECTED_KEY: &[u8] = b"pm_fees_collected";
const MOLTYID_ADDR_KEY: &[u8] = b"pm_moltyid_addr";
const ORACLE_ADDR_KEY: &[u8] = b"pm_oracle_addr";
const MUSD_ADDR_KEY: &[u8] = b"pm_musd_addr";
const DEX_GOV_ADDR_KEY: &[u8] = b"pm_dex_gov_addr";

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key)
        .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
        .unwrap_or(0)
}

fn save_u64(key: &[u8], val: u64) {
    storage_set(key, &u64_to_bytes(val));
}

fn load_u8(key: &[u8]) -> u8 {
    storage_get(key)
        .map(|d| if !d.is_empty() { d[0] } else { 0 })
        .unwrap_or(0)
}

fn save_u8(key: &[u8], val: u8) {
    storage_set(key, &[val]);
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

fn addrs_equal(a: &[u8], b: &[u8]) -> bool {
    a.len() == 32 && b.len() == 32 && a == b
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

// --- Storage key builders ---

fn market_key(market_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_m_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k
}

fn question_key(market_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_q_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k
}

fn outcome_pool_key(market_id: u64, outcome: u8) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_o_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(outcome as u64));
    k
}

fn outcome_name_key(market_id: u64, outcome: u8) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_on_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(outcome as u64));
    k
}

fn position_key(market_id: u64, addr: &[u8], outcome: u8) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_p_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k.push(b'_');
    k.extend_from_slice(&hex_encode(addr));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(outcome as u64));
    k
}

fn lp_key(market_id: u64, addr: &[u8]) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_lp_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k.push(b'_');
    k.extend_from_slice(&hex_encode(addr));
    k
}

fn category_index_key(category: u8, idx: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_cat_"[..]);
    k.extend_from_slice(&u64_to_decimal(category as u64));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(idx));
    k
}

fn category_count_key(category: u8) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_catc_"[..]);
    k.extend_from_slice(&u64_to_decimal(category as u64));
    k
}

fn active_market_key(idx: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_active_"[..]);
    k.extend_from_slice(&u64_to_decimal(idx));
    k
}

fn user_market_key(addr: &[u8], idx: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_user_"[..]);
    k.extend_from_slice(&hex_encode(addr));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(idx));
    k
}

fn user_market_count_key(addr: &[u8]) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_userc_"[..]);
    k.extend_from_slice(&hex_encode(addr));
    k
}

/// Key for storing the question_hash → market_id mapping to prevent duplicates.
fn question_hash_key(hash: &[u8]) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_qh_"[..]);
    k.extend_from_slice(&hex_encode(hash));
    k
}

/// Key for per-market trading pause (circuit breaker).
fn market_pause_key(market_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_mpause_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k
}

/// Key for storing the slot of the last trade (circuit breaker price move check).
fn market_last_trade_slot_key(market_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_mlt_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k
}

/// Key for dispute count per market.
fn dispute_count_key(market_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_dc_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k
}

/// Key for challenger bond escrow.
fn challenger_key(market_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_chal_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k
}

// ============================================================================
// SECURITY: REENTRANCY + PAUSE
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
// MARKET RECORD ENCODING/DECODING (192 bytes)
// ============================================================================
//
// Bytes 0..8     : market_id (u64)
// Bytes 8..40    : creator (Pubkey)
// Bytes 40..48   : created_slot (u64)
// Bytes 48..56   : close_slot (u64)
// Bytes 56..64   : resolve_slot (u64) — when resolution submitted
// Byte  64       : status (u8)
// Byte  65       : outcome_count (u8)
// Byte  66       : winning_outcome (u8) — 0xFF = unresolved
// Byte  67       : category (u8)
// Bytes 68..76   : total_collateral (u64)
// Bytes 76..84   : total_volume (u64)
// Bytes 84..92   : resolution_bond (u64)
// Bytes 92..124  : resolver (Pubkey)
// Bytes 124..156 : question_hash (32 bytes)
// Bytes 156..164 : dispute_end_slot (u64)
// Bytes 164..172 : fees_collected (u64)
// Bytes 172..180 : lp_total_shares (u64)
// Bytes 180..188 : oracle_attestation_hash (first 8 bytes)
// Bytes 188..192 : pad (4 bytes)

fn encode_market(
    market_id: u64,
    creator: &[u8],
    created_slot: u64,
    close_slot: u64,
    resolve_slot: u64,
    status: u8,
    outcome_count: u8,
    winning_outcome: u8,
    category: u8,
    total_collateral: u64,
    total_volume: u64,
    resolution_bond: u64,
    resolver: &[u8],
    question_hash: &[u8],
    dispute_end_slot: u64,
    fees_collected: u64,
    lp_total_shares: u64,
    oracle_attestation_hash: &[u8],
) -> Vec<u8> {
    let mut data = Vec::with_capacity(MARKET_RECORD_SIZE);
    data.extend_from_slice(&u64_to_bytes(market_id));           // 0..8
    // creator: 32 bytes
    if creator.len() >= 32 {
        data.extend_from_slice(&creator[..32]);
    } else {
        data.extend_from_slice(creator);
        data.resize(data.len() + 32 - creator.len(), 0);
    }                                                            // 8..40
    data.extend_from_slice(&u64_to_bytes(created_slot));         // 40..48
    data.extend_from_slice(&u64_to_bytes(close_slot));           // 48..56
    data.extend_from_slice(&u64_to_bytes(resolve_slot));         // 56..64
    data.push(status);                                           // 64
    data.push(outcome_count);                                    // 65
    data.push(winning_outcome);                                  // 66
    data.push(category);                                         // 67
    data.extend_from_slice(&u64_to_bytes(total_collateral));     // 68..76
    data.extend_from_slice(&u64_to_bytes(total_volume));         // 76..84
    data.extend_from_slice(&u64_to_bytes(resolution_bond));      // 84..92
    // resolver: 32 bytes
    if resolver.len() >= 32 {
        data.extend_from_slice(&resolver[..32]);
    } else {
        data.extend_from_slice(resolver);
        data.resize(data.len() + 32 - resolver.len(), 0);
    }                                                            // 92..124
    // question_hash: 32 bytes
    if question_hash.len() >= 32 {
        data.extend_from_slice(&question_hash[..32]);
    } else {
        data.extend_from_slice(question_hash);
        data.resize(data.len() + 32 - question_hash.len(), 0);
    }                                                            // 124..156
    data.extend_from_slice(&u64_to_bytes(dispute_end_slot));     // 156..164
    data.extend_from_slice(&u64_to_bytes(fees_collected));       // 164..172
    data.extend_from_slice(&u64_to_bytes(lp_total_shares));      // 172..180
    // oracle_attestation_hash: first 8 bytes
    if oracle_attestation_hash.len() >= 8 {
        data.extend_from_slice(&oracle_attestation_hash[..8]);
    } else {
        data.extend_from_slice(oracle_attestation_hash);
        data.resize(data.len() + 8 - oracle_attestation_hash.len(), 0);
    }                                                            // 180..188
    data.extend_from_slice(&[0u8; 4]);                           // 188..192 pad
    debug_assert_eq!(data.len(), MARKET_RECORD_SIZE);
    data
}

// Market record decoders — individual field extractors
fn market_id(data: &[u8]) -> u64 { bytes_to_u64(&data[0..8]) }
fn market_creator(data: &[u8]) -> [u8; 32] {
    let mut a = [0u8; 32]; a.copy_from_slice(&data[8..40]); a
}
fn market_created_slot(data: &[u8]) -> u64 { bytes_to_u64(&data[40..48]) }
fn market_close_slot(data: &[u8]) -> u64 { bytes_to_u64(&data[48..56]) }
fn market_resolve_slot(data: &[u8]) -> u64 { bytes_to_u64(&data[56..64]) }
fn market_status(data: &[u8]) -> u8 { data[64] }
fn market_outcome_count(data: &[u8]) -> u8 { data[65] }
fn market_winning_outcome(data: &[u8]) -> u8 { data[66] }
fn market_category(data: &[u8]) -> u8 { data[67] }
fn market_total_collateral(data: &[u8]) -> u64 { bytes_to_u64(&data[68..76]) }
fn market_total_volume(data: &[u8]) -> u64 { bytes_to_u64(&data[76..84]) }
fn market_resolution_bond(data: &[u8]) -> u64 { bytes_to_u64(&data[84..92]) }
fn market_resolver(data: &[u8]) -> [u8; 32] {
    let mut a = [0u8; 32]; a.copy_from_slice(&data[92..124]); a
}
fn market_question_hash(data: &[u8]) -> [u8; 32] {
    let mut a = [0u8; 32]; a.copy_from_slice(&data[124..156]); a
}
fn market_dispute_end_slot(data: &[u8]) -> u64 { bytes_to_u64(&data[156..164]) }
fn market_fees_collected(data: &[u8]) -> u64 { bytes_to_u64(&data[164..172]) }
fn market_lp_total_shares(data: &[u8]) -> u64 { bytes_to_u64(&data[172..180]) }

// Market record mutators — set individual fields in-place
fn set_market_status(data: &mut [u8], status: u8) { data[64] = status; }
fn set_market_winning_outcome(data: &mut [u8], outcome: u8) { data[66] = outcome; }
fn set_market_total_collateral(data: &mut [u8], val: u64) {
    data[68..76].copy_from_slice(&u64_to_bytes(val));
}
fn set_market_total_volume(data: &mut [u8], val: u64) {
    data[76..84].copy_from_slice(&u64_to_bytes(val));
}
fn set_market_resolution_bond(data: &mut [u8], val: u64) {
    data[84..92].copy_from_slice(&u64_to_bytes(val));
}
fn set_market_resolver(data: &mut [u8], resolver: &[u8]) {
    if resolver.len() >= 32 {
        data[92..124].copy_from_slice(&resolver[..32]);
    }
}
fn set_market_resolve_slot(data: &mut [u8], slot: u64) {
    data[56..64].copy_from_slice(&u64_to_bytes(slot));
}
fn set_market_dispute_end_slot(data: &mut [u8], slot: u64) {
    data[156..164].copy_from_slice(&u64_to_bytes(slot));
}
fn set_market_fees_collected(data: &mut [u8], val: u64) {
    data[164..172].copy_from_slice(&u64_to_bytes(val));
}
fn set_market_lp_total_shares(data: &mut [u8], val: u64) {
    data[172..180].copy_from_slice(&u64_to_bytes(val));
}

/// Load a market record from storage. Returns None if not found or invalid size.
fn load_market(market_id: u64) -> Option<Vec<u8>> {
    let data = storage_get(&market_key(market_id))?;
    if data.len() >= MARKET_RECORD_SIZE {
        Some(data)
    } else {
        None
    }
}

/// Save a market record to storage.
fn save_market(market_id: u64, data: &[u8]) {
    storage_set(&market_key(market_id), data);
}

// ============================================================================
// OUTCOME POOL ENCODING/DECODING (64 bytes per outcome)
// ============================================================================
//
// Bytes 0..8     : reserve (u64) — AMM virtual reserve
// Bytes 8..16    : total_shares (u64) — total shares minted
// Bytes 16..24   : total_redeemed (u64) — shares redeemed after resolution
// Bytes 24..32   : price_last (u64) — last traded price (6 decimal mUSD basis)
// Bytes 32..40   : volume (u64) — outcome-specific volume
// Bytes 40..48   : open_interest (u64) — outstanding unredeemed shares
// Bytes 48..56   : pad
// Bytes 56..64   : pad

fn encode_outcome_pool(
    reserve: u64,
    total_shares: u64,
    total_redeemed: u64,
    price_last: u64,
    volume: u64,
    open_interest: u64,
) -> Vec<u8> {
    let mut data = Vec::with_capacity(OUTCOME_POOL_SIZE);
    data.extend_from_slice(&u64_to_bytes(reserve));
    data.extend_from_slice(&u64_to_bytes(total_shares));
    data.extend_from_slice(&u64_to_bytes(total_redeemed));
    data.extend_from_slice(&u64_to_bytes(price_last));
    data.extend_from_slice(&u64_to_bytes(volume));
    data.extend_from_slice(&u64_to_bytes(open_interest));
    data.extend_from_slice(&[0u8; 8]); // pad
    data.extend_from_slice(&[0u8; 8]); // pad
    debug_assert_eq!(data.len(), OUTCOME_POOL_SIZE);
    data
}

fn pool_reserve(data: &[u8]) -> u64 { bytes_to_u64(&data[0..8]) }
fn pool_total_shares(data: &[u8]) -> u64 { bytes_to_u64(&data[8..16]) }
fn pool_total_redeemed(data: &[u8]) -> u64 { bytes_to_u64(&data[16..24]) }
fn pool_price_last(data: &[u8]) -> u64 { bytes_to_u64(&data[24..32]) }
fn pool_volume(data: &[u8]) -> u64 { bytes_to_u64(&data[32..40]) }
fn pool_open_interest(data: &[u8]) -> u64 { bytes_to_u64(&data[40..48]) }

fn set_pool_reserve(data: &mut [u8], val: u64) {
    data[0..8].copy_from_slice(&u64_to_bytes(val));
}
fn set_pool_total_shares(data: &mut [u8], val: u64) {
    data[8..16].copy_from_slice(&u64_to_bytes(val));
}
fn set_pool_total_redeemed(data: &mut [u8], val: u64) {
    data[16..24].copy_from_slice(&u64_to_bytes(val));
}
fn set_pool_price_last(data: &mut [u8], val: u64) {
    data[24..32].copy_from_slice(&u64_to_bytes(val));
}
fn set_pool_volume(data: &mut [u8], val: u64) {
    data[32..40].copy_from_slice(&u64_to_bytes(val));
}
fn set_pool_open_interest(data: &mut [u8], val: u64) {
    data[40..48].copy_from_slice(&u64_to_bytes(val));
}

fn load_outcome_pool(market_id: u64, outcome: u8) -> Option<Vec<u8>> {
    let data = storage_get(&outcome_pool_key(market_id, outcome))?;
    if data.len() >= OUTCOME_POOL_SIZE {
        Some(data)
    } else {
        None
    }
}

fn save_outcome_pool(market_id: u64, outcome: u8, data: &[u8]) {
    storage_set(&outcome_pool_key(market_id, outcome), data);
}

// ============================================================================
// POSITION ENCODING/DECODING (16 bytes per user per outcome per market)
// ============================================================================
//
// Bytes 0..8  : shares (u64)
// Bytes 8..16 : cost_basis (u64) — total mUSD spent acquiring

fn encode_position(shares: u64, cost_basis: u64) -> Vec<u8> {
    let mut data = Vec::with_capacity(POSITION_SIZE);
    data.extend_from_slice(&u64_to_bytes(shares));
    data.extend_from_slice(&u64_to_bytes(cost_basis));
    data
}

fn position_shares(data: &[u8]) -> u64 {
    if data.len() >= 8 { bytes_to_u64(&data[0..8]) } else { 0 }
}
fn position_cost_basis(data: &[u8]) -> u64 {
    if data.len() >= 16 { bytes_to_u64(&data[8..16]) } else { 0 }
}

fn load_position(market_id: u64, addr: &[u8], outcome: u8) -> (u64, u64) {
    match storage_get(&position_key(market_id, addr, outcome)) {
        Some(data) if data.len() >= POSITION_SIZE => {
            (position_shares(&data), position_cost_basis(&data))
        }
        _ => (0, 0),
    }
}

fn save_position(market_id: u64, addr: &[u8], outcome: u8, shares: u64, cost_basis: u64) {
    storage_set(
        &position_key(market_id, addr, outcome),
        &encode_position(shares, cost_basis),
    );
}

// ============================================================================
// INDEX MANAGEMENT
// ============================================================================

/// Track user participation in a market (idempotent — checks before adding).
fn track_user_market(addr: &[u8], market_id: u64) {
    let count_key = user_market_count_key(addr);
    let count = load_u64(&count_key);
    // Deduplicate: check existing entries (scan is bounded by user's participation count)
    for i in 0..count {
        let existing = load_u64(&user_market_key(addr, i));
        if existing == market_id {
            return; // Already tracked
        }
    }
    save_u64(&user_market_key(addr, count), market_id);
    save_u64(&count_key, count + 1);
}

/// Add market to category index.
fn index_category(category: u8, market_id: u64) {
    let cc_key = category_count_key(category);
    let count = load_u64(&cc_key);
    save_u64(&category_index_key(category, count), market_id);
    save_u64(&cc_key, count + 1);
}

/// Add market to the active markets list.
fn add_active_market(market_id: u64) {
    let idx = load_u64(OPEN_MARKETS_KEY);
    save_u64(&active_market_key(idx), market_id);
    save_u64(OPEN_MARKETS_KEY, idx + 1);
}

/// Remove market from the active markets list (swap-remove).
fn remove_active_market(market_id: u64) {
    let count = load_u64(OPEN_MARKETS_KEY);
    if count == 0 {
        return;
    }
    for i in 0..count {
        let mid = load_u64(&active_market_key(i));
        if mid == market_id {
            // Swap with last
            if i < count - 1 {
                let last = load_u64(&active_market_key(count - 1));
                save_u64(&active_market_key(i), last);
            }
            save_u64(OPEN_MARKETS_KEY, count - 1);
            return;
        }
    }
}

// ============================================================================
// AMM MATH — Binary CPMM (x·y=k)
// ============================================================================

/// Calculate the price of an outcome given all reserves.
/// price_i = (product of all OTHER reserves) / (sum of all such products)
/// For binary: price_YES = reserve_NO / (reserve_YES + reserve_NO)
/// Returns price in mUSD units (6 decimals), so 500_000 = $0.50
fn calculate_price(reserves: &[u64], outcome: u8) -> u64 {
    let n = reserves.len();
    if n == 0 || outcome as usize >= n {
        return 0;
    }

    if n == 2 {
        // Binary shortcut: price_i = other / (self + other)
        let self_r = reserves[outcome as usize] as u128;
        let other_r = reserves[1 - outcome as usize] as u128;
        let sum = self_r + other_r;
        if sum == 0 {
            return 0;
        }
        // price = other / sum * MUSD_UNIT
        ((other_r * MUSD_UNIT as u128) / sum) as u64
    } else {
        // Multi-outcome: price_i = (1/reserve_i) / sum(1/reserve_j)
        // Use inverse approach to avoid products that overflow:
        // price_i = (1/r_i) / sum(1/r_j for all j)
        // Multiply through by product of all reserves:
        // price_i = (prod / r_i) / sum(prod / r_j)
        // = recip_i / sum(recip_j)
        //
        // To avoid overflow, use u128 and the reciprocal formula with a scale factor.
        let scale: u128 = 1_000_000_000_000; // 10^12 precision
        let mut recip_sum: u128 = 0;
        for &r in reserves {
            if r == 0 {
                return 0;
            }
            recip_sum += scale / (r as u128);
        }
        if recip_sum == 0 {
            return 0;
        }
        let recip_i = scale / (reserves[outcome as usize] as u128);
        ((recip_i * MUSD_UNIT as u128) / recip_sum) as u64
    }
}

/// Calculate shares received when buying outcome `outcome` with `amount_musd`.
/// Uses the complete-set-mint + swap model from the plan.
///
/// Returns (shares_received_after_fee, fee_shares)
fn calculate_buy(
    reserves: &[u64],
    outcome: u8,
    amount_musd: u64,
) -> (u64, u64) {
    let n = reserves.len();
    if n < 2 || outcome as usize >= n || amount_musd == 0 {
        return (0, 0);
    }

    // Step 1: Mint complete sets — each mUSD mints 1 share of each outcome
    // We work in "shares" (1 share = 1 MUSD_UNIT of collateral backing)
    let shares_per_set = amount_musd; // 1:1 ratio (shares denominated in mUSD micro-units)

    if n == 2 {
        // Binary CPMM: x·y=k
        // User mints shares_per_set of YES + NO.
        // User wants outcome A. Sells shares_per_set of B into pool.
        let a = outcome as usize;
        let b = 1 - a;
        let x = reserves[a] as u128; // reserve of desired outcome
        let y = reserves[b] as u128; // reserve of other outcome

        // Selling B shares into pool:
        // a_received = x * b_sold / (y + b_sold)
        let b_sold = shares_per_set as u128;
        let a_received_from_swap = (x * b_sold) / (y + b_sold);

        // Total shares = mint shares + swap shares
        let total_shares = shares_per_set as u128 + a_received_from_swap;

        // Fee deduction from the swap portion only (not the mint)
        let fee_shares = (a_received_from_swap * TRADING_FEE_BPS as u128) / 10_000;
        let actual_shares = total_shares - fee_shares;

        (actual_shares as u64, fee_shares as u64)
    } else {
        // Multi-outcome: mint complete sets, then sell all non-desired outcomes into pool
        let a = outcome as usize;
        let sps = shares_per_set as u128;

        // For each non-desired outcome j, sell sps shares of j into pool.
        // This adds sps to reserve[j] and extracts shares of outcome a.
        // We do this iteratively, updating reserves as we go.
        let mut temp_reserves: Vec<u128> = reserves.iter().map(|&r| r as u128).collect();
        let mut total_a_from_swaps: u128 = 0;

        for j in 0..n {
            if j == a {
                continue;
            }
            // Sell sps shares of outcome j. Receive shares of outcome a.
            // CPMM product invariant for the pair (a, j):
            // a_received = reserve_a * sps / (reserve_j + sps)
            let a_received = (temp_reserves[a] * sps) / (temp_reserves[j] + sps);
            temp_reserves[a] -= a_received;
            temp_reserves[j] += sps;
            total_a_from_swaps += a_received;
        }

        let total_shares = sps + total_a_from_swaps;
        let fee_shares = (total_a_from_swaps * TRADING_FEE_BPS as u128) / 10_000;
        let actual_shares = total_shares - fee_shares;

        (actual_shares as u64, fee_shares as u64)
    }
}

/// Calculate mUSD returned when selling `shares_amount` of outcome `outcome`.
/// Uses the swap + burn-complete-set model from the plan.
///
/// Returns (musd_returned_after_fee, fee_musd)
fn calculate_sell(
    reserves: &[u64],
    outcome: u8,
    shares_amount: u64,
) -> (u64, u64) {
    let n = reserves.len();
    if n < 2 || outcome as usize >= n || shares_amount == 0 {
        return (0, 0);
    }

    if n == 2 {
        // Binary CPMM
        let a = outcome as usize;
        let b = 1 - a;
        let x = reserves[a] as u128;
        let y = reserves[b] as u128;
        let sell = shares_amount as u128;

        // Selling A shares into pool for B shares:
        // b_received = y * sell / (x + sell)
        let b_received = (y * sell) / (x + sell);

        // Burn complete sets: min(remaining_a_after_swap, b_received)
        // After swap, user has 0 A shares (all sold) but had shares_amount - 0 = 0 from swap
        // Wait — user starts with shares_amount of A. Sells them ALL into pool for B.
        // User ends with b_received B shares.
        // To burn a complete set, need 1 A + 1 B. User has 0 A, so can't burn... 
        // Actually the plan says: "Sell outcome shares into pool for opposite outcome shares, 
        // then burn complete sets". The user doesn't sell ALL shares, they sell to swap for the 
        // other side, then burn matched pairs.
        //
        // Correct model: user has `shares_amount` of A.
        // They sell `s` shares of A to get `b` shares of B, where
        // b = y * s / (x + s).
        // Then they have (shares_amount - s) of A and b of B.
        // They burn min(shares_amount - s, b) complete sets.
        // To maximize mUSD, they want to sell all A: s = shares_amount.
        // Then have 0 A and b_received B. Can't burn any sets.
        //
        // Alternative approach (correct per the plan): swap SOME A for B, burn matched sets.
        // Optimal: sell exactly half (approximate for best result).
        // Actually, the correct Polymarket/prediction-market model:
        // User sells shares → pool pays mUSD out of collateral.
        // Let's use the direct "sell to pool" approach.
        //
        // Direct sell approach: pool receives A shares, user gets mUSD proportional.
        // The AMM "buys" the A shares from the user by releasing collateral.
        // mUSD received = amount of collateral that gets freed.
        //
        // Cleaner: just reverse the buy math.
        // If buying A costs X mUSD, selling A returns roughly X mUSD (minus fees, plus slippage).
        // sell_musd = b_received * MUSD_UNIT / MUSD_UNIT = b_received (since shares = mUSD micro-units)
        //
        // Actually the simplest correct model: selling A shares is equivalent to "un-buying".
        // The user swaps A shares into the pool and gets back mUSD equal to the number of 
        // complete sets they can form plus the residual.
        //
        // Let me recompute using the standard approach:
        // 1. User has shares_amount of A
        // 2. Swap s_a shares of A into pool → get s_b shares of B
        //    s_b = y * s_a / (x + s_a)
        // 3. After swap: user has (shares_amount - s_a) of A, s_b of B
        // 4. Burn min(shares_amount - s_a, s_b) complete sets → get that many mUSD units
        //
        // Optimal s_a: maximize min(shares_amount - s_a, s_b)
        // This is maximized when shares_amount - s_a = s_b = y * s_a / (x + s_a)
        // shares_amount - s_a = y * s_a / (x + s_a)
        // (shares_amount - s_a)(x + s_a) = y * s_a
        // shares_amount * x + shares_amount * s_a - s_a * x - s_a² = y * s_a
        // -s_a² + (shares_amount - x - y) * s_a + shares_amount * x = 0
        // s_a² - (shares_amount - x - y) * s_a - shares_amount * x = 0
        //
        // This is a quadratic. Solving with integer sqrt.
        // s_a = [(A - x - y) + sqrt((A - x - y)² + 4 * A * x)] / 2
        // where A = shares_amount
        
        let a_val = sell;
        let sum = x + y;
        
        // Discriminant: (A + x + y)² - 4·A·y  [rearranging for positive root]
        // Actually let's re-derive:
        // s_a² + (x + y - A)*s_a - A*x = 0
        // disc = (x + y - A)² + 4*A*x
        let coeff = sum.abs_diff(a_val);
        let disc = coeff * coeff + 4 * a_val * x;
        let sqrt_disc = isqrt_u128(disc);
        
        // s_a = (-coeff + sqrt_disc) / 2 when A > x + y
        // s_a = (coeff - sqrt_disc + sqrt_disc) ... need to handle signs
        // Going back to: s_a² + (x + y - A)*s_a - A*x = 0
        // s_a = [-(x + y - A) + sqrt((x+y-A)² + 4Ax)] / 2
        // = [A - x - y + sqrt((x+y-A)² + 4Ax)] / 2
        let s_a = if a_val + sqrt_disc >= sum {
            (a_val + sqrt_disc - sum) / 2
        } else {
            0
        };

        if s_a == 0 || s_a > a_val {
            return (0, 0);
        }

        // B received from swap
        let b_got = (y * s_a) / (x + s_a);
        // Complete sets = min(A - s_a, b_got)
        let remaining_a = a_val - s_a;
        let sets = if remaining_a < b_got { remaining_a } else { b_got };
        
        // mUSD returned = sets (since 1 share of each outcome = 1 mUSD_unit backing)
        let musd_raw = sets;
        let fee = (musd_raw * TRADING_FEE_BPS as u128) / 10_000;
        let net = musd_raw - fee;

        (net as u64, fee as u64)
    } else {
        // Multi-outcome sell is more complex.
        // For simplicity and correctness: swap A shares for each other outcome,
        // then burn the minimum complete sets.
        let a = outcome as usize;
        let sell = shares_amount as u128;
        let mut temp_reserves: Vec<u128> = reserves.iter().map(|&r| r as u128).collect();
        
        // Distribute: sell `sell / (n-1)` shares of A to get shares of each other outcome j.
        // Then burn min across all outcomes.
        let parts = (n - 1) as u128;
        let sell_per = sell / parts;
        let sell_remainder = sell - sell_per * parts;
        
        let mut min_other: u128 = u128::MAX;
        let mut j_idx = 0u8;
        
        for j in 0..n {
            if j == a {
                continue;
            }
            let s_a = if j_idx < sell_remainder as u8 { sell_per + 1 } else { sell_per };
            j_idx += 1;
            // Swap s_a shares of A -> shares of j
            let j_received = (temp_reserves[j] * s_a) / (temp_reserves[a] + s_a);
            temp_reserves[a] += s_a;
            temp_reserves[j] -= j_received;
            if j_received < min_other {
                min_other = j_received;
            }
        }

        // Also check the remaining A shares
        let remaining_a = sell - (sell_per * parts + sell_remainder); // should be 0 due to above
        // Actually the user keeps no A shares (sold them all), but we need A to burn sets.
        // Rethink: user has `sell` of A. They want to convert some to each other outcome.
        // They keep some A and swap rest to get other outcomes. Then burn min across all.
        //
        // Better approach: sell fraction of A into each non-A pool.
        // User starts with `sell` shares of A.
        // For each outcome j ≠ A, sell s_j shares of A to get some j shares.
        // sum(s_j) ≤ sell.
        // Remaining A = sell - sum(s_j).
        // Complete sets = min(remaining_A, min_j(j_shares_received)).
        // This is a multi-dimensional optimization. For now, use equal partition.
        // User sells (sell * n/(n+1)) total, keeps (sell / (n+1)) of A.
        // Nah, let's just use the approach of selling A for equal value in each other.
        
        // Restart with cleaner approach:
        // Sell (sell * (n-1)/n) of A total, keeping sell/n of A
        // Each non-A gets sell/n swapped
        let temp_reserves2: Vec<u128> = reserves.iter().map(|&r| r as u128).collect();
        let keep_a = sell / (n as u128); // keep this many A shares
        let total_sell_a = sell - keep_a;
        let sell_each = total_sell_a / parts;
        
        let mut min_j: u128 = u128::MAX;
        let mut temp2 = temp_reserves2.clone();
        for j in 0..n {
            if j == a { continue; }
            let j_got = (temp2[j] * sell_each) / (temp2[a] + sell_each);
            temp2[a] += sell_each;
            temp2[j] -= j_got;
            if j_got < min_j { min_j = j_got; }
        }
        
        let sets = if keep_a < min_j { keep_a } else { min_j };
        let fee = (sets * TRADING_FEE_BPS as u128) / 10_000;
        let net = sets - fee;

        (net as u64, fee as u64)
    }
}

/// Integer square root for u128 (Newton's method).
fn isqrt_u128(n: u128) -> u128 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = x.div_ceil(2);
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// Apply new reserves after a buy operation.
/// Returns the new reserves after the buy.
fn apply_buy_reserves(
    reserves: &[u64],
    outcome: u8,
    amount_musd: u64,
    fee_shares: u64,
) -> Vec<u64> {
    let n = reserves.len();
    let a = outcome as usize;
    let sps = amount_musd;

    if n == 2 {
        let b = 1 - a;
        let x = reserves[a] as u128;
        let y = reserves[b] as u128;
        let b_sold = sps as u128;

        // Swap: sell B into pool
        let a_received = (x * b_sold) / (y + b_sold);
        
        // New reserves:
        // reserve_a = x - a_received + fee_shares (fees go to LP/pool)
        // reserve_b = y + b_sold
        let new_x = x - a_received + fee_shares as u128;
        let new_y = y + b_sold;

        let mut new_reserves = alloc::vec![0u64; n];
        new_reserves[a] = new_x as u64;
        new_reserves[b] = new_y as u64;
        new_reserves
    } else {
        let mut temp: Vec<u128> = reserves.iter().map(|&r| r as u128).collect();
        let sps128 = sps as u128;

        for j in 0..n {
            if j == a { continue; }
            let a_received = (temp[a] * sps128) / (temp[j] + sps128);
            temp[a] -= a_received;
            temp[j] += sps128;
        }
        // Add fee shares back to outcome a reserve (LP accumulation)
        temp[a] += fee_shares as u128;

        temp.iter().map(|&r| r as u64).collect()
    }
}

/// Apply new reserves after a sell operation.
fn apply_sell_reserves(
    reserves: &[u64],
    outcome: u8,
    shares_amount: u64,
) -> Vec<u64> {
    let n = reserves.len();
    let a = outcome as usize;

    if n == 2 {
        let b = 1 - a;
        let x = reserves[a] as u128;
        let y = reserves[b] as u128;
        let sell = shares_amount as u128;

        // Re-calculate the optimal s_a using same quadratic as calculate_sell
        let sum = x + y;
        let coeff = sum.abs_diff(sell);
        let disc = coeff * coeff + 4 * sell * x;
        let sqrt_disc = isqrt_u128(disc);
        
        let s_a = if sell + sqrt_disc >= sum {
            (sell + sqrt_disc - sum) / 2
        } else {
            0
        };

        if s_a == 0 || s_a > sell {
            return reserves.to_vec();
        }

        let b_got = (y * s_a) / (x + s_a);
        let remaining_a_after_swap = sell - s_a;
        let sets = if remaining_a_after_swap < b_got { remaining_a_after_swap } else { b_got };

        // After swap: pool absorbed s_a of A, gave out b_got of B.
        // After burn: remove `sets` from each outcome's total supply.
        // Pool reserves:
        // reserve_a = x + s_a (pool absorbed A)
        // reserve_b = y - b_got (pool gave out B)
        let new_x = x + s_a;
        let new_y = y - b_got;

        let mut new_reserves = alloc::vec![0u64; n];
        new_reserves[a] = new_x as u64;
        new_reserves[b] = new_y as u64;
        new_reserves
    } else {
        // Multi: use same approach as calculate_sell
        let sell = shares_amount as u128;
        let parts = (n - 1) as u128;
        let keep_a = sell / (n as u128);
        let total_sell_a = sell - keep_a;
        let sell_each = total_sell_a / parts;

        let mut temp: Vec<u128> = reserves.iter().map(|&r| r as u128).collect();
        for j in 0..n {
            if j == a { continue; }
            let j_got = (temp[j] * sell_each) / (temp[a] + sell_each);
            temp[a] += sell_each;
            temp[j] -= j_got;
        }
        temp.iter().map(|&r| r as u64).collect()
    }
}

// ============================================================================
// CORE OPERATIONS
// ============================================================================

/// Initialize the prediction market contract.
/// Returns 0 on success, non-zero on error.
#[no_mangle]
pub extern "C" fn initialize(admin_ptr: *const u8) -> u32 {
    // Re-initialization guard
    if storage_get(ADMIN_KEY).is_some() {
        let existing = load_addr(ADMIN_KEY);
        if !is_zero(&existing) {
            return 1;
        }
    }

    let mut admin = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(admin_ptr, admin.as_mut_ptr(), 32); }
    let admin = &admin[..];
    storage_set(ADMIN_KEY, admin);
    save_u64(MARKET_COUNT_KEY, 0);
    save_u64(OPEN_MARKETS_KEY, 0);
    save_u64(TOTAL_VOLUME_KEY, 0);
    save_u64(TOTAL_COLLATERAL_KEY, 0);
    save_u64(FEES_COLLECTED_KEY, 0);
    save_u8(PAUSED_KEY, 0);
    save_u8(REENTRANCY_KEY, 0);

    log_info("PredictionReef initialized!");
    0
}

/// Create a new prediction market.
/// Returns market_id on success, 0 on failure.
pub fn create_market(
    creator_ptr: *const u8,
    category: u8,
    close_slot: u64,
    outcome_count: u8,
    question_hash_ptr: *const u8,
    question_ptr: *const u8,
    question_len: u32,
) -> u32 {
    if !reentrancy_enter() {
        return 0;
    }
    if !require_not_paused() {
        reentrancy_exit();
        return 0;
    }

    let mut creator = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(creator_ptr, creator.as_mut_ptr(), 32); }
    let creator = &creator[..];
    let mut question_hash = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(question_hash_ptr, question_hash.as_mut_ptr(), 32); }
    let question_hash = &question_hash[..];
    let mut question = vec![0u8; question_len as usize];
    unsafe { core::ptr::copy_nonoverlapping(question_ptr, question.as_mut_ptr(), question_len as usize); }

    // Validate caller == creator
    let caller = moltchain_sdk::get_caller();
    if caller.0[..] != creator[..] {
        reentrancy_exit();
        return 0;
    }

    // Validate outcome_count
    if !(2..=MAX_OUTCOMES).contains(&outcome_count) {
        reentrancy_exit();
        return 0;
    }

    // Validate category
    if category > MAX_CATEGORY {
        reentrancy_exit();
        return 0;
    }

    // Validate duration
    let current_slot = get_slot();
    if close_slot <= current_slot {
        reentrancy_exit();
        return 0;
    }
    let duration = close_slot - current_slot;
    if duration < MIN_DURATION {
        reentrancy_exit();
        return 0;
    }
    if duration > MAX_DURATION {
        reentrancy_exit();
        return 0;
    }

    // Validate question
    if question_len == 0 || question_len > MAX_QUESTION_LEN as u32 {
        reentrancy_exit();
        return 0;
    }

    // Check for duplicate question hash
    let qh_key = question_hash_key(question_hash);
    if let Some(existing) = storage_get(&qh_key) {
        if existing.len() >= 8 && bytes_to_u64(&existing) != 0 {
            reentrancy_exit();
            return 0;
        }
    }

    // Check market limits
    let market_count = load_u64(MARKET_COUNT_KEY);
    if market_count >= MAX_MARKETS {
        reentrancy_exit();
        return 0;
    }
    let open_count = load_u64(OPEN_MARKETS_KEY);
    if open_count >= MAX_OPEN_MARKETS {
        reentrancy_exit();
        return 0;
    }

    // MoltyID reputation check (reads pm_moltyid_addr for the contract address)
    // Since cross-contract calls are stubs on MoltChain, we read MoltyID's storage
    // directly using the known key format: "rep:{hex_encoded_pubkey}"
    let moltyid_addr = load_addr(MOLTYID_ADDR_KEY);
    if !is_zero(&moltyid_addr) {
        let rep_key_for_creator = {
            let mut k = Vec::from(&b"rep:"[..]);
            k.extend_from_slice(&hex_encode(creator));
            k
        };
        let reputation = load_u64(&rep_key_for_creator);
        if reputation < MIN_REPUTATION_CREATE {
            reentrancy_exit();
            return 0;
        }
    }

    // Create market record
    let new_id = market_count + 1;
    let record = encode_market(
        new_id,
        creator,
        current_slot,
        close_slot,
        0, // resolve_slot
        STATUS_PENDING,
        outcome_count,
        UNRESOLVED,
        category,
        0, // total_collateral
        0, // total_volume
        0, // resolution_bond
        &[0u8; 32], // resolver
        question_hash,
        0, // dispute_end_slot
        0, // fees_collected
        0, // lp_total_shares
        &[0u8; 8], // oracle_attestation_hash
    );
    save_market(new_id, &record);

    // Store question text
    storage_set(&question_key(new_id), &question);

    // Initialize outcome pools (all reserves start at 0 until liquidity is added)
    for i in 0..outcome_count {
        let pool = encode_outcome_pool(0, 0, 0, 0, 0, 0);
        save_outcome_pool(new_id, i, &pool);
    }

    // Store question hash → market_id mapping
    save_u64(&qh_key, new_id);

    // Update global counters
    save_u64(MARKET_COUNT_KEY, new_id);

    // Index by category
    index_category(category, new_id);

    log_info("Market created!");

    reentrancy_exit();
    new_id as u32
}

/// Add initial liquidity to a PENDING market, transitioning it to ACTIVE.
/// `odds_bps_ptr` points to an array of outcome_count u16 values (basis points each).
/// If the first u16 is 0, defaults to equal odds.
/// Returns 1 on success, 0 on failure.
pub fn add_initial_liquidity(
    provider_ptr: *const u8,
    market_id: u64,
    amount_musd: u64,
    odds_bps_ptr: *const u8,
    odds_bps_len: u32,
) -> u32 {
    if !reentrancy_enter() { return 0; }
    if !require_not_paused() { reentrancy_exit(); return 0; }

    let mut provider = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(provider_ptr, provider.as_mut_ptr(), 32); }
    let provider = &provider[..];

    // Validate caller
    let caller = moltchain_sdk::get_caller();
    if caller.0[..] != provider[..] {
        reentrancy_exit();
        return 0;
    }

    // Load market
    let mut record = match load_market(market_id) {
        Some(r) => r,
        None => { log_info("Market not found"); reentrancy_exit(); return 0; }
    };

    // Only PENDING markets accept initial liquidity
    if market_status(&record) != STATUS_PENDING {
        reentrancy_exit();
        return 0;
    }

    // Only creator can add initial liquidity
    let creator = market_creator(&record);
    if !addrs_equal(provider, &creator) {
        reentrancy_exit();
        return 0;
    }

    // Validate amount
    if amount_musd < MIN_COLLATERAL {
        reentrancy_exit();
        return 0;
    }
    if amount_musd > MAX_COLLATERAL {
        reentrancy_exit();
        return 0;
    }

    let outcome_count = market_outcome_count(&record);

    // Parse odds or use equal distribution
    let mut reserves = Vec::with_capacity(outcome_count as usize);
    let total_shares = amount_musd; // 1 mUSD = 1 share unit in each outcome

    if odds_bps_len >= (outcome_count as u32) * 2 {
        let mut odds_data = vec![0u8; odds_bps_len as usize];
        unsafe { core::ptr::copy_nonoverlapping(odds_bps_ptr, odds_data.as_mut_ptr(), odds_bps_len as usize); }
        // Check if first value is 0 (use default equal odds)
        let first_odds = u16::from_le_bytes([odds_data[0], odds_data[1]]);
        if first_odds == 0 {
            // Equal odds
            let per_outcome = total_shares / (outcome_count as u64);
            for _ in 0..outcome_count {
                reserves.push(per_outcome);
            }
        } else {
            // Custom odds (in basis points, must sum to 10000)
            let mut bps_sum: u64 = 0;
            let mut bps_values = Vec::with_capacity(outcome_count as usize);
            for i in 0..outcome_count as usize {
                let offset = i * 2;
                let bps = u16::from_le_bytes([odds_data[offset], odds_data[offset + 1]]) as u64;
                bps_values.push(bps);
                bps_sum += bps;
            }
            if bps_sum != 10_000 {
                reentrancy_exit();
                return 0;
            }
            // Convert odds to reserves. Higher probability → lower reserve (inverse).
            // price_i = reserve_other / total → we want price_i = bps_i / 10000
            // For binary: price_YES = y/(x+y), price_NO = x/(x+y)
            // If bps_YES = 7000, then price_YES = 0.70
            // y/(x+y) = 0.70 → y = 0.70*(x+y) → y = 0.70x + 0.70y → 0.30y = 0.70x → x/y = 3/7
            // So reserve_YES = total * (1 - p_YES) = total * bps_NO / 10000
            // Generalizing: reserve_i is proportional to (1/bps_i) normalized.
            // Actually for CPMM: price_i = (1/r_i) / sum(1/r_j)
            // We want (1/r_i) / sum(1/r_j) = p_i (= bps_i / 10000)
            // So 1/r_i = C * p_i for some constant C
            // r_i = 1 / (C * p_i) = K / p_i where K is normalization constant
            // sum(r_i) = K * sum(1/p_i)
            // We want total liquidity to be total_shares * outcome_count (one set per mUSD).
            // Actually just set: r_i = total_shares * (10000 - bps_i) / 10000 for binary, 
            // or more generally, r_i = total_shares * C / bps_i
            //
            // Simplest correct approach for binary: 
            //   reserve_YES = total_shares * bps_NO / 10000
            //   reserve_NO = total_shares * bps_YES / 10000
            // These give: price_YES = r_NO / (r_YES + r_NO)
            //           = (T * bps_YES/10000) / (T * bps_NO/10000 + T * bps_YES/10000)
            //           = bps_YES / (bps_NO + bps_YES) = bps_YES / 10000 //
            // For multi-outcome with CPMM product model:
            //   r_i = K / bps_i (where K is chosen so that all reserves are reasonable)
            
            if outcome_count == 2 {
                // Binary: swap the bps values for reserves
                reserves.push((total_shares * bps_values[1]) / 10_000);
                reserves.push((total_shares * bps_values[0]) / 10_000);
            } else {
                // Multi: r_i = total_shares * C / bps_i
                // C chosen so that the minimum reserve is at least total_shares / outcome_count
                // Use: r_i = (total_shares * 10000) / (bps_i * outcome_count)
                // This ensures sum is close to total_shares
                for &bps in &bps_values {
                    if bps == 0 {
                        reentrancy_exit();
                        return 0;
                    }
                    let r = (total_shares as u128 * 10_000) / (bps as u128 * outcome_count as u128);
                    reserves.push(r as u64);
                }
            }
        }
    } else {
        // Default: equal odds
        let per_outcome = total_shares / (outcome_count as u64);
        for _ in 0..outcome_count {
            reserves.push(per_outcome);
        }
    }

    // Ensure all reserves are above minimum
    for &r in &reserves {
        if r < INITIAL_LIQUIDITY_MIN {
            reentrancy_exit();
            return 0;
        }
    }

    // Write outcome pools with initial reserves
    for i in 0..outcome_count {
        let price = calculate_price(&reserves, i);
        let pool = encode_outcome_pool(
            reserves[i as usize],
            total_shares, // total shares minted for this outcome (one set per mUSD)
            0,
            price,
            0,
            total_shares, // all shares are outstanding
        );
        save_outcome_pool(market_id, i, &pool);
    }

    // Creator gets LP tokens proportional to their deposit
    save_u64(&lp_key(market_id, provider), amount_musd);
    set_market_lp_total_shares(&mut record, amount_musd);

    // Update market: status → ACTIVE, collateral
    set_market_status(&mut record, STATUS_ACTIVE);
    set_market_total_collateral(&mut record, amount_musd);
    save_market(market_id, &record);

    // Update global state
    let total_coll = load_u64(TOTAL_COLLATERAL_KEY);
    save_u64(TOTAL_COLLATERAL_KEY, total_coll + amount_musd);
    add_active_market(market_id);

    // Track user
    track_user_market(provider, market_id);

    log_info("Initial liquidity added — market is now ACTIVE!");
    reentrancy_exit();
    1
}

/// Add liquidity to an ACTIVE market.
/// Returns LP shares minted, or 0 on failure.
pub fn add_liquidity(
    provider_ptr: *const u8,
    market_id: u64,
    amount_musd: u64,
) -> u32 {
    if !reentrancy_enter() { return 0; }
    if !require_not_paused() { reentrancy_exit(); return 0; }

    let mut provider = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(provider_ptr, provider.as_mut_ptr(), 32); }
    let provider = &provider[..];

    let caller = moltchain_sdk::get_caller();
    if caller.0[..] != provider[..] {
        reentrancy_exit();
        return 0;
    }

    let mut record = match load_market(market_id) {
        Some(r) => r,
        None => { log_info("Market not found"); reentrancy_exit(); return 0; }
    };

    if market_status(&record) != STATUS_ACTIVE {
        reentrancy_exit();
        return 0;
    }

    if amount_musd < MIN_COLLATERAL {
        reentrancy_exit();
        return 0;
    }

    let outcome_count = market_outcome_count(&record);
    let existing_collateral = market_total_collateral(&record);
    let existing_lp_total = market_lp_total_shares(&record);

    // Circuit breaker: per-market collateral cap
    if existing_collateral + amount_musd > CIRCUIT_BREAKER_COLLATERAL {
        reentrancy_exit();
        return 0;
    }

    // Platform-wide circuit breaker
    let platform_coll = load_u64(TOTAL_COLLATERAL_KEY);
    if platform_coll + amount_musd > CIRCUIT_BREAKER_PLATFORM {
        reentrancy_exit();
        return 0;
    }

    // LP shares proportional to existing pool
    // new_lp = existing_lp_total * amount_musd / existing_collateral
    let new_lp = if existing_collateral == 0 || existing_lp_total == 0 {
        amount_musd
    } else {
        (existing_lp_total as u128 * amount_musd as u128 / existing_collateral as u128) as u64
    };

    // Add to each outcome pool's reserve proportionally
    for i in 0..outcome_count {
        let mut pool = match load_outcome_pool(market_id, i) {
            Some(p) => p,
            None => { reentrancy_exit(); return 0; }
        };
        let old_reserve = pool_reserve(&pool);
        // Add proportional to current reserve
        let add = if existing_collateral > 0 {
            (old_reserve as u128 * amount_musd as u128 / existing_collateral as u128) as u64
        } else {
            amount_musd / (outcome_count as u64)
        };
        set_pool_reserve(&mut pool, old_reserve + add);
        let ts = pool_total_shares(&pool);
        set_pool_total_shares(&mut pool, ts + amount_musd);
        let oi = pool_open_interest(&pool);
        set_pool_open_interest(&mut pool, oi + amount_musd);
        save_outcome_pool(market_id, i, &pool);
    }

    // Update LP token balance
    let existing_lp = load_u64(&lp_key(market_id, provider));
    save_u64(&lp_key(market_id, provider), existing_lp + new_lp);

    // Update market totals
    set_market_total_collateral(&mut record, existing_collateral + amount_musd);
    set_market_lp_total_shares(&mut record, existing_lp_total + new_lp);
    save_market(market_id, &record);

    // Update global
    save_u64(TOTAL_COLLATERAL_KEY, platform_coll + amount_musd);
    track_user_market(provider, market_id);

    log_info("Liquidity added!");
    reentrancy_exit();
    new_lp as u32
}

/// Buy shares of a specific outcome.
/// Returns shares received (as u32 — truncated for return code; real value in return_data).
pub fn buy_shares(
    trader_ptr: *const u8,
    market_id: u64,
    outcome: u8,
    amount_musd: u64,
) -> u32 {
    if !reentrancy_enter() { return 0; }
    if !require_not_paused() { reentrancy_exit(); return 0; }

    let mut trader = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(trader_ptr, trader.as_mut_ptr(), 32); }
    let trader = &trader[..];

    let caller = moltchain_sdk::get_caller();
    if caller.0[..] != trader[..] {
        reentrancy_exit();
        return 0;
    }

    let mut record = match load_market(market_id) {
        Some(r) => r,
        None => { log_info("Market not found"); reentrancy_exit(); return 0; }
    };

    // Must be ACTIVE and not past close_slot
    if market_status(&record) != STATUS_ACTIVE {
        reentrancy_exit();
        return 0;
    }
    let current_slot = get_slot();
    if current_slot >= market_close_slot(&record) {
        log_info("Market closed for trading");
        reentrancy_exit();
        return 0;
    }

    let outcome_count = market_outcome_count(&record);
    if outcome >= outcome_count {
        reentrancy_exit();
        return 0;
    }

    if amount_musd == 0 {
        reentrancy_exit();
        return 0;
    }

    // Check per-market circuit breaker on collateral
    let existing_coll = market_total_collateral(&record);
    if existing_coll + amount_musd > CIRCUIT_BREAKER_COLLATERAL {
        reentrancy_exit();
        return 0;
    }

    // Load all reserves
    let mut reserves = Vec::with_capacity(outcome_count as usize);
    for i in 0..outcome_count {
        let pool = match load_outcome_pool(market_id, i) {
            Some(p) => p,
            None => { reentrancy_exit(); return 0; }
        };
        reserves.push(pool_reserve(&pool));
    }

    // Check price circuit breaker (no >50% move in single slot)
    let prev_price = {
        let pool = load_outcome_pool(market_id, outcome).unwrap();
        pool_price_last(&pool)
    };

    // Check temporary market pause (from a previous trade's circuit breaker)
    let pause_until = load_u64(&market_pause_key(market_id));
    if pause_until > 0 && current_slot < pause_until {
        reentrancy_exit();
        return 0;
    }

    // Calculate buy
    let (shares_received, fee_shares) = calculate_buy(&reserves, outcome, amount_musd);
    if shares_received == 0 {
        reentrancy_exit();
        return 0;
    }

    // Apply new reserves
    let new_reserves = apply_buy_reserves(&reserves, outcome, amount_musd, fee_shares);

    // Calculate new price for circuit breaker check
    let new_price = calculate_price(&new_reserves, outcome);
    if prev_price > 0 && new_price > 0 {
        let price_diff = new_price.abs_diff(prev_price);
        // Price move > 50% of previous price → arm circuit breaker for NEXT trade
        if (price_diff * 10_000) / prev_price > PRICE_MOVE_PAUSE_BPS {
            save_u64(&market_pause_key(market_id), current_slot + PRICE_MOVE_PAUSE_SLOTS);
        }
    }

    // Update outcome pools
    for i in 0..outcome_count {
        let mut pool = load_outcome_pool(market_id, i).unwrap();
        set_pool_reserve(&mut pool, new_reserves[i as usize]);

        if i == outcome {
            let vol = pool_volume(&pool);
            set_pool_volume(&mut pool, vol + amount_musd);
            set_pool_price_last(&mut pool, new_price);
            // total_shares increases (new shares minted)
            let ts = pool_total_shares(&pool);
            set_pool_total_shares(&mut pool, ts + shares_received);
            let oi = pool_open_interest(&pool);
            set_pool_open_interest(&mut pool, oi + shares_received);
        } else {
            // Update price for this outcome too
            let p = calculate_price(&new_reserves, i);
            set_pool_price_last(&mut pool, p);
        }

        save_outcome_pool(market_id, i, &pool);
    }

    // Update user position
    let (existing_shares, existing_cost) = load_position(market_id, trader, outcome);
    save_position(
        market_id,
        trader,
        outcome,
        existing_shares + shares_received,
        existing_cost + amount_musd,
    );

    // Update market totals
    set_market_total_collateral(&mut record, existing_coll + amount_musd);
    let vol = market_total_volume(&record);
    set_market_total_volume(&mut record, vol + amount_musd);
    // Distribute fee: LP portion stays in pool (already via fee_shares), protocol portion:
    let fee_musd = (amount_musd as u128 * TRADING_FEE_BPS as u128 / 10_000) as u64;
    let protocol_fee = (fee_musd * FEE_PROTOCOL_SHARE) / 100;
    let fees = market_fees_collected(&record);
    set_market_fees_collected(&mut record, fees + protocol_fee);
    save_market(market_id, &record);

    // Update global counters
    let total_vol = load_u64(TOTAL_VOLUME_KEY);
    save_u64(TOTAL_VOLUME_KEY, total_vol + amount_musd);
    let total_coll = load_u64(TOTAL_COLLATERAL_KEY);
    save_u64(TOTAL_COLLATERAL_KEY, total_coll + amount_musd);
    let total_fees = load_u64(FEES_COLLECTED_KEY);
    save_u64(FEES_COLLECTED_KEY, total_fees + protocol_fee);

    track_user_market(trader, market_id);

    log_info("Shares purchased!");
    reentrancy_exit();
    shares_received as u32
}

/// Sell shares of a specific outcome.
/// Returns mUSD received (truncated to u32 for return code).
pub fn sell_shares(
    trader_ptr: *const u8,
    market_id: u64,
    outcome: u8,
    shares_amount: u64,
) -> u32 {
    if !reentrancy_enter() { return 0; }
    if !require_not_paused() { reentrancy_exit(); return 0; }

    let mut trader = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(trader_ptr, trader.as_mut_ptr(), 32); }
    let trader = &trader[..];

    let caller = moltchain_sdk::get_caller();
    if caller.0[..] != trader[..] {
        reentrancy_exit();
        return 0;
    }

    let mut record = match load_market(market_id) {
        Some(r) => r,
        None => { log_info("Market not found"); reentrancy_exit(); return 0; }
    };

    if market_status(&record) != STATUS_ACTIVE {
        reentrancy_exit();
        return 0;
    }
    let current_slot = get_slot();
    if current_slot >= market_close_slot(&record) {
        log_info("Market closed for trading");
        reentrancy_exit();
        return 0;
    }

    let outcome_count = market_outcome_count(&record);
    if outcome >= outcome_count {
        reentrancy_exit();
        return 0;
    }

    if shares_amount == 0 {
        reentrancy_exit();
        return 0;
    }

    // Check user has enough shares
    let (user_shares, user_cost) = load_position(market_id, trader, outcome);
    if user_shares < shares_amount {
        reentrancy_exit();
        return 0;
    }

    // Check temporary market pause
    let pause_until = load_u64(&market_pause_key(market_id));
    if pause_until > 0 && current_slot < pause_until {
        reentrancy_exit();
        return 0;
    }

    // Load all reserves
    let mut reserves = Vec::with_capacity(outcome_count as usize);
    for i in 0..outcome_count {
        let pool = load_outcome_pool(market_id, i).unwrap();
        reserves.push(pool_reserve(&pool));
    }

    // Calculate sell
    let (musd_returned, fee_musd) = calculate_sell(&reserves, outcome, shares_amount);
    if musd_returned == 0 {
        reentrancy_exit();
        return 0;
    }

    // Apply new reserves
    let new_reserves = apply_sell_reserves(&reserves, outcome, shares_amount);

    // Update outcome pools
    for i in 0..outcome_count {
        let mut pool = load_outcome_pool(market_id, i).unwrap();
        set_pool_reserve(&mut pool, new_reserves[i as usize]);

        let new_p = calculate_price(&new_reserves, i);
        set_pool_price_last(&mut pool, new_p);

        if i == outcome {
            let vol = pool_volume(&pool);
            set_pool_volume(&mut pool, vol + musd_returned + fee_musd);
            let ts = pool_total_shares(&pool);
            // Decrease total shares (shares burned as part of complete set redemption)
            if ts >= shares_amount {
                set_pool_total_shares(&mut pool, ts - shares_amount);
            }
            let oi = pool_open_interest(&pool);
            if oi >= shares_amount {
                set_pool_open_interest(&mut pool, oi - shares_amount);
            }
        }

        save_outcome_pool(market_id, i, &pool);
    }

    // Update user position
    let cost_basis_reduction = if user_shares > 0 {
        (user_cost as u128 * shares_amount as u128 / user_shares as u128) as u64
    } else {
        0
    };
    save_position(
        market_id,
        trader,
        outcome,
        user_shares - shares_amount,
        user_cost.saturating_sub(cost_basis_reduction),
    );

    // Update market totals
    let existing_coll = market_total_collateral(&record);
    let total_out = musd_returned + fee_musd;
    set_market_total_collateral(
        &mut record,
        existing_coll.saturating_sub(total_out),
    );
    let vol = market_total_volume(&record);
    set_market_total_volume(&mut record, vol + total_out);
    let protocol_fee = (fee_musd * FEE_PROTOCOL_SHARE) / 100;
    let fees = market_fees_collected(&record);
    set_market_fees_collected(&mut record, fees + protocol_fee);
    save_market(market_id, &record);

    // Update global counters
    let total_vol = load_u64(TOTAL_VOLUME_KEY);
    save_u64(TOTAL_VOLUME_KEY, total_vol + total_out);
    let total_coll = load_u64(TOTAL_COLLATERAL_KEY);
    save_u64(TOTAL_COLLATERAL_KEY, total_coll.saturating_sub(total_out));
    let total_fees = load_u64(FEES_COLLECTED_KEY);
    save_u64(FEES_COLLECTED_KEY, total_fees + protocol_fee);

    log_info("Shares sold!");
    reentrancy_exit();
    musd_returned as u32
}

/// Mint a complete set (1 share of every outcome for amount_musd collateral).
/// Returns 1 on success, 0 on failure.
pub fn mint_complete_set(
    user_ptr: *const u8,
    market_id: u64,
    amount_musd: u64,
) -> u32 {
    if !reentrancy_enter() { return 0; }
    if !require_not_paused() { reentrancy_exit(); return 0; }

    let mut user = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(user_ptr, user.as_mut_ptr(), 32); }
    let user = &user[..];
    let caller = moltchain_sdk::get_caller();
    if caller.0[..] != user[..] {
        reentrancy_exit();
        return 0;
    }

    let mut record = match load_market(market_id) {
        Some(r) => r,
        None => { reentrancy_exit(); return 0; }
    };

    if market_status(&record) != STATUS_ACTIVE {
        reentrancy_exit();
        return 0;
    }

    // Check close slot — no minting after market closes
    let current_slot = moltchain_sdk::get_slot();
    let close_slot = market_close_slot(&record);
    if current_slot > close_slot {
        log_info("Market closed for minting");
        reentrancy_exit();
        return 0;
    }

    if amount_musd == 0 {
        reentrancy_exit();
        return 0;
    }

    let outcome_count = market_outcome_count(&record);

    // Collateral checks
    let existing_coll = market_total_collateral(&record);
    if existing_coll + amount_musd > CIRCUIT_BREAKER_COLLATERAL {
        reentrancy_exit();
        return 0;
    }

    // Mint: for each outcome, give user `amount_musd` shares
    for i in 0..outcome_count {
        let (existing_shares, existing_cost) = load_position(market_id, user, i);
        save_position(
            market_id, user, i,
            existing_shares + amount_musd,
            existing_cost + amount_musd,
        );

        // Update pool: increase total shares and open interest
        let mut pool = match load_outcome_pool(market_id, i) {
            Some(p) => p,
            None => { reentrancy_exit(); return 0; }
        };
        let ts = pool_total_shares(&pool);
        set_pool_total_shares(&mut pool, ts + amount_musd);
        let oi = pool_open_interest(&pool);
        set_pool_open_interest(&mut pool, oi + amount_musd);
        save_outcome_pool(market_id, i, &pool);
    }

    // Lock collateral
    set_market_total_collateral(&mut record, existing_coll + amount_musd);
    save_market(market_id, &record);

    let total_coll = load_u64(TOTAL_COLLATERAL_KEY);
    save_u64(TOTAL_COLLATERAL_KEY, total_coll + amount_musd);

    track_user_market(user, market_id);

    reentrancy_exit();
    1
}

/// Redeem a complete set (burn 1 share of every outcome for collateral return).
/// Returns mUSD amount returned, 0 on failure.
pub fn redeem_complete_set(
    user_ptr: *const u8,
    market_id: u64,
    amount: u64,
) -> u32 {
    if !reentrancy_enter() { return 0; }
    if !require_not_paused() { reentrancy_exit(); return 0; }

    let mut user = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(user_ptr, user.as_mut_ptr(), 32); }
    let user = &user[..];
    let caller = moltchain_sdk::get_caller();
    if caller.0[..] != user[..] {
        reentrancy_exit();
        return 0;
    }

    let mut record = match load_market(market_id) {
        Some(r) => r,
        None => { reentrancy_exit(); return 0; }
    };

    let status = market_status(&record);
    if status != STATUS_ACTIVE && status != STATUS_CLOSED {
        reentrancy_exit();
        return 0;
    }

    if amount == 0 {
        reentrancy_exit();
        return 0;
    }

    let outcome_count = market_outcome_count(&record);

    // Verify user has at least `amount` of every outcome
    for i in 0..outcome_count {
        let (shares, _) = load_position(market_id, user, i);
        if shares < amount {
            reentrancy_exit();
            return 0;
        }
    }

    // Burn shares from each outcome
    for i in 0..outcome_count {
        let (shares, cost) = load_position(market_id, user, i);
        let cost_reduction = if shares > 0 {
            (cost as u128 * amount as u128 / shares as u128) as u64
        } else { 0 };
        save_position(
            market_id, user, i,
            shares - amount,
            cost.saturating_sub(cost_reduction),
        );

        // Update pool
        let mut pool = match load_outcome_pool(market_id, i) {
            Some(p) => p,
            None => { reentrancy_exit(); return 0; }
        };
        let ts = pool_total_shares(&pool);
        set_pool_total_shares(&mut pool, ts.saturating_sub(amount));
        let oi = pool_open_interest(&pool);
        set_pool_open_interest(&mut pool, oi.saturating_sub(amount));
        save_outcome_pool(market_id, i, &pool);
    }

    // Return collateral (no fee on redemption per plan)
    let musd_returned = amount;
    let existing_coll = market_total_collateral(&record);
    set_market_total_collateral(
        &mut record,
        existing_coll.saturating_sub(musd_returned),
    );
    save_market(market_id, &record);

    let total_coll = load_u64(TOTAL_COLLATERAL_KEY);
    save_u64(TOTAL_COLLATERAL_KEY, total_coll.saturating_sub(musd_returned));

    reentrancy_exit();
    musd_returned as u32
}

/// Close a market after its close_slot has passed.
/// Anyone can call this to transition ACTIVE → CLOSED.
/// Returns 1 on success, 0 on failure.
pub fn close_market(
    caller_ptr: *const u8,
    market_id: u64,
) -> u32 {
    if !reentrancy_enter() { return 0; }

    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let caller = &caller[..];
    let actual_caller = moltchain_sdk::get_caller();
    if actual_caller.0[..] != caller[..] {
        reentrancy_exit();
        return 0;
    }

    let mut record = match load_market(market_id) {
        Some(r) => r,
        None => { reentrancy_exit(); return 0; }
    };

    if market_status(&record) != STATUS_ACTIVE {
        reentrancy_exit();
        return 0;
    }

    let current_slot = get_slot();
    let close_slot = market_close_slot(&record);
    if current_slot <= close_slot {
        reentrancy_exit();
        return 0;
    }

    set_market_status(&mut record, STATUS_CLOSED);
    save_market(market_id, &record);

    reentrancy_exit();
    1
}

/// Submit a resolution for a CLOSED market.
/// Returns 1 on success, 0 on failure.
pub fn submit_resolution(
    resolver_ptr: *const u8,
    market_id: u64,
    winning_outcome: u8,
    attestation_hash_ptr: *const u8,
    bond: u64,
) -> u32 {
    if !reentrancy_enter() { return 0; }
    if !require_not_paused() { reentrancy_exit(); return 0; }

    let mut resolver = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(resolver_ptr, resolver.as_mut_ptr(), 32); }
    let resolver = &resolver[..];
    let mut attestation_hash = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(attestation_hash_ptr, attestation_hash.as_mut_ptr(), 32); }
    let attestation_hash = &attestation_hash[..];

    let caller = moltchain_sdk::get_caller();
    if caller.0[..] != resolver[..] {
        reentrancy_exit();
        return 0;
    }

    let mut record = match load_market(market_id) {
        Some(r) => r,
        None => { log_info("Market not found"); reentrancy_exit(); return 0; }
    };

    // Market must be CLOSED
    if market_status(&record) != STATUS_CLOSED {
        reentrancy_exit();
        return 0;
    }

    // Validate outcome
    if winning_outcome >= market_outcome_count(&record) {
        reentrancy_exit();
        return 0;
    }

    // Validate bond
    if bond < DISPUTE_BOND {
        reentrancy_exit();
        return 0;
    }

    // MoltyID reputation check for resolver (1000+ required)
    let moltyid_addr = load_addr(MOLTYID_ADDR_KEY);
    if !is_zero(&moltyid_addr) {
        let rep_key_for_resolver = {
            let mut k = Vec::from(&b"rep:"[..]);
            k.extend_from_slice(&hex_encode(resolver));
            k
        };
        let reputation = load_u64(&rep_key_for_resolver);
        if reputation < MIN_REPUTATION_RESOLVE {
            reentrancy_exit();
            return 0;
        }
    }

    // Verify MoltOracle attestation
    // The attestation key in MoltOracle: "attestation_{hex_of_data_hash}"
    // We check that attestation exists and has >= RESOLUTION_THRESHOLD signatures.
    // Since we can't do cross-contract calls, we read MoltOracle's storage directly.
    let oracle_addr = load_addr(ORACLE_ADDR_KEY);
    if !is_zero(&oracle_addr) {
        let att_key = {
            let mut k = Vec::from(&b"attestation_"[..]);
            k.extend_from_slice(&hex_encode(attestation_hash));
            k
        };
        match storage_get(&att_key) {
            Some(att_data) if att_data.len() >= 33 => {
                let sig_count = att_data[32];
                if sig_count < RESOLUTION_THRESHOLD {
                    reentrancy_exit();
                    return 0;
                }
            }
            _ => {
                reentrancy_exit();
                return 0;
            }
        }
    }

    // Set market to RESOLVING
    let current_slot = get_slot();
    set_market_status(&mut record, STATUS_RESOLVING);
    set_market_winning_outcome(&mut record, winning_outcome);
    set_market_resolve_slot(&mut record, current_slot);
    set_market_resolution_bond(&mut record, bond);
    set_market_resolver(&mut record, resolver);
    set_market_dispute_end_slot(&mut record, current_slot + DISPUTE_PERIOD);
    // Store attestation hash (first 8 bytes) in the oracle_attestation_hash field
    record[180..188].copy_from_slice(&attestation_hash[..8]);
    save_market(market_id, &record);

    log_info("Resolution submitted — dispute period started!");
    reentrancy_exit();
    1
}

/// Challenge a resolution during the dispute period.
/// Returns 1 on success, 0 on failure.
pub fn challenge_resolution(
    challenger_ptr: *const u8,
    market_id: u64,
    evidence_hash_ptr: *const u8,
    bond: u64,
) -> u32 {
    if !reentrancy_enter() { return 0; }
    if !require_not_paused() { reentrancy_exit(); return 0; }

    let mut challenger = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(challenger_ptr, challenger.as_mut_ptr(), 32); }
    let challenger = &challenger[..];
    let mut evidence_hash = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(evidence_hash_ptr, evidence_hash.as_mut_ptr(), 32); }
    let evidence_hash = &evidence_hash[..];

    let caller = moltchain_sdk::get_caller();
    if caller.0[..] != challenger[..] {
        reentrancy_exit();
        return 0;
    }

    let mut record = match load_market(market_id) {
        Some(r) => r,
        None => { reentrancy_exit(); return 0; }
    };

    if market_status(&record) != STATUS_RESOLVING {
        reentrancy_exit();
        return 0;
    }

    // Must be within dispute period
    let current_slot = get_slot();
    if current_slot > market_dispute_end_slot(&record) {
        reentrancy_exit();
        return 0;
    }

    // Bond must be sufficient
    if bond < DISPUTE_BOND {
        reentrancy_exit();
        return 0;
    }

    // Cannot dispute own resolution
    let resolver = market_resolver(&record);
    if addrs_equal(challenger, &resolver) {
        reentrancy_exit();
        return 0;
    }

    // Set market to DISPUTED
    set_market_status(&mut record, STATUS_DISPUTED);
    save_market(market_id, &record);

    // Store challenger info for bond distribution later
    let chal_key = challenger_key(market_id);
    let mut chal_data = Vec::with_capacity(40);
    chal_data.extend_from_slice(challenger);
    chal_data.extend_from_slice(&u64_to_bytes(bond));
    storage_set(&chal_key, &chal_data);

    // Increment dispute count
    let dc_key = dispute_count_key(market_id);
    let dc = load_u64(&dc_key);
    save_u64(&dc_key, dc + 1);

    // Note: auto-escalation to DAO happens when dispute_count >= 3
    // The DAO can call dao_resolve() to settle at any time

    reentrancy_exit();
    1
}

/// Finalize a resolution after the dispute period passes without challenge.
/// This transitions RESOLVING → RESOLVED.
/// Returns 1 on success, 0 on failure.
pub fn finalize_resolution(
    caller_ptr: *const u8,
    market_id: u64,
) -> u32 {
    if !reentrancy_enter() { return 0; }

    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let caller = &caller[..];

    let mut record = match load_market(market_id) {
        Some(r) => r,
        None => { reentrancy_exit(); return 0; }
    };

    if market_status(&record) != STATUS_RESOLVING {
        reentrancy_exit();
        return 0;
    }

    // Dispute period must have passed
    let current_slot = get_slot();
    if current_slot <= market_dispute_end_slot(&record) {
        reentrancy_exit();
        return 0;
    }

    // Finalize
    set_market_status(&mut record, STATUS_RESOLVED);
    save_market(market_id, &record);

    // Remove from active markets
    remove_active_market(market_id);
    let open = load_u64(OPEN_MARKETS_KEY);
    // The remove_active_market already decremented, no need to double-decrement

    // Pay resolver reward: 0.5% of total collateral
    let total_coll = market_total_collateral(&record);
    let reward = (total_coll as u128 * RESOLUTION_REWARD_BPS as u128 / 10_000) as u64;
    // Reward would be credited to resolver's mUSD balance in a production system.
    // For now, record it in fees_collected and the resolver can claim it.
    let resolver = market_resolver(&record);
    let (res_shares, res_cost) = load_position(market_id, &resolver, 0);
    // Note: we don't actually give them prediction shares. In production, we'd credit mUSD.
    // Store resolver reward in a dedicated key
    let reward_key = {
        let mut k = Vec::from(&b"pm_rw_"[..]);
        k.extend_from_slice(&u64_to_decimal(market_id));
        k
    };
    save_u64(&reward_key, reward);

    reentrancy_exit();
    1
}

/// DAO resolve: admin/DAO can override resolution for DISPUTED markets.
/// Returns 1 on success, 0 on failure.
pub fn dao_resolve(
    caller_ptr: *const u8,
    market_id: u64,
    winning_outcome: u8,
) -> u32 {
    if !reentrancy_enter() { return 0; }

    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let caller = &caller[..];
    let mut caller_arr = [0u8; 32];
    caller_arr.copy_from_slice(caller);

    // Admin or DAO governance only
    let actual_caller = moltchain_sdk::get_caller();
    if actual_caller.0[..] != caller[..] {
        reentrancy_exit();
        return 0;
    }

    if !require_admin(&caller_arr) {
        // Also check DAO governance address
        let dao_addr = load_addr(DEX_GOV_ADDR_KEY);
        if is_zero(&dao_addr) || !addrs_equal(caller, &dao_addr) {
            reentrancy_exit();
            return 0;
        }
    }

    let mut record = match load_market(market_id) {
        Some(r) => r,
        None => { reentrancy_exit(); return 0; }
    };

    if market_status(&record) != STATUS_DISPUTED {
        reentrancy_exit();
        return 0;
    }

    if winning_outcome >= market_outcome_count(&record) {
        reentrancy_exit();
        return 0;
    }

    set_market_status(&mut record, STATUS_RESOLVED);
    set_market_winning_outcome(&mut record, winning_outcome);
    save_market(market_id, &record);

    remove_active_market(market_id);

    // Bond distribution: loser's bond goes 50% winner, 50% DAO treasury
    // (handled via separate claim mechanism)

    log_info("DAO resolution applied — market RESOLVED!");
    reentrancy_exit();
    1
}

/// DAO void: admin/DAO can void a market (refund all collateral).
/// Returns 1 on success, 0 on failure.
pub fn dao_void(
    caller_ptr: *const u8,
    market_id: u64,
) -> u32 {
    if !reentrancy_enter() { return 0; }

    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let caller = &caller[..];
    let mut caller_arr = [0u8; 32];
    caller_arr.copy_from_slice(caller);

    let actual_caller = moltchain_sdk::get_caller();
    if actual_caller.0[..] != caller[..] {
        reentrancy_exit();
        return 0;
    }

    if !require_admin(&caller_arr) {
        let dao_addr = load_addr(DEX_GOV_ADDR_KEY);
        if is_zero(&dao_addr) || !addrs_equal(caller, &dao_addr) {
            reentrancy_exit();
            return 0;
        }
    }

    let mut record = match load_market(market_id) {
        Some(r) => r,
        None => { reentrancy_exit(); return 0; }
    };

    let status = market_status(&record);
    // Can void from ACTIVE, CLOSED, RESOLVING, DISPUTED
    if status == STATUS_RESOLVED || status == STATUS_VOIDED || status == STATUS_PENDING {
        reentrancy_exit();
        return 0;
    }

    set_market_status(&mut record, STATUS_VOIDED);
    save_market(market_id, &record);

    remove_active_market(market_id);

    reentrancy_exit();
    1
}

/// Redeem winning shares after market is RESOLVED.
/// Returns mUSD payout.
pub fn redeem_shares(
    user_ptr: *const u8,
    market_id: u64,
    outcome: u8,
) -> u32 {
    if !reentrancy_enter() { return 0; }

    let mut user = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(user_ptr, user.as_mut_ptr(), 32); }
    let user = &user[..];
    let caller = moltchain_sdk::get_caller();
    if caller.0[..] != user[..] {
        reentrancy_exit();
        return 0;
    }

    let mut record = match load_market(market_id) {
        Some(r) => r,
        None => { reentrancy_exit(); return 0; }
    };

    if market_status(&record) != STATUS_RESOLVED {
        reentrancy_exit();
        return 0;
    }

    let winning = market_winning_outcome(&record);
    let (user_shares, _user_cost) = load_position(market_id, user, outcome);

    if user_shares == 0 {
        reentrancy_exit();
        return 0;
    }

    let payout = if outcome == winning {
        // Each winning share redeems at 1 mUSD (= MUSD_UNIT micro-units)
        // But shares are already denominated in micro-units, so payout = shares * 1
        // Actually, 1 share = backed by 1 mUSD_UNIT of collateral.
        // Since shares are in the same unit as mUSD micro-units,
        // payout = user_shares (in mUSD micro-units)
        user_shares
    } else {
        0 // Losing shares are worthless
    };

    // Clear position (prevent double redemption)
    save_position(market_id, user, outcome, 0, 0);

    // Update pool redeemed count
    if let Some(mut pool) = load_outcome_pool(market_id, outcome) {
        let redeemed = pool_total_redeemed(&pool);
        set_pool_total_redeemed(&mut pool, redeemed + user_shares);
        let oi = pool_open_interest(&pool);
        set_pool_open_interest(&mut pool, oi.saturating_sub(user_shares));
        save_outcome_pool(market_id, outcome, &pool);
    }

    // Reduce total collateral
    if payout > 0 {
        let coll = market_total_collateral(&record);
        set_market_total_collateral(&mut record, coll.saturating_sub(payout));
        save_market(market_id, &record);

        let total_coll = load_u64(TOTAL_COLLATERAL_KEY);
        save_u64(TOTAL_COLLATERAL_KEY, total_coll.saturating_sub(payout));
    }

    reentrancy_exit();
    payout as u32
}

/// Reclaim collateral from a VOIDED market.
/// Returns mUSD refund amount.
pub fn reclaim_collateral(
    user_ptr: *const u8,
    market_id: u64,
) -> u32 {
    if !reentrancy_enter() { return 0; }

    let mut user = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(user_ptr, user.as_mut_ptr(), 32); }
    let user = &user[..];
    let caller = moltchain_sdk::get_caller();
    if caller.0[..] != user[..] {
        reentrancy_exit();
        return 0;
    }

    let mut record = match load_market(market_id) {
        Some(r) => r,
        None => { reentrancy_exit(); return 0; }
    };

    if market_status(&record) != STATUS_VOIDED {
        reentrancy_exit();
        return 0;
    }

    let outcome_count = market_outcome_count(&record);
    let total_coll_market = market_total_collateral(&record);

    // Calculate user's total cost basis across all outcomes
    let mut user_total_cost: u64 = 0;
    for i in 0..outcome_count {
        let (_, cost) = load_position(market_id, user, i);
        user_total_cost += cost;
    }

    // Also check LP position
    let user_lp = load_u64(&lp_key(market_id, user));
    let total_lp = market_lp_total_shares(&record);

    // Refund = user's proportional share
    // For traders: pro-rata based on cost_basis / total_collateral
    // For LPs: pro-rata based on lp_shares / lp_total_shares
    let mut refund: u64 = 0;

    if total_coll_market > 0 && user_total_cost > 0 {
        // Pro-rata refund: user's cost basis as proportion of total collateral.
        // If total collateral is >= sum of all cost bases, users get full refund.
        // Otherwise, proportional reduction.
        refund = user_total_cost;
    }

    if total_lp > 0 && user_lp > 0 {
        // LP's share of remaining collateral
        let lp_share = (total_coll_market as u128 * user_lp as u128 / total_lp as u128) as u64;
        // Don't double-count with cost basis
        if lp_share > refund {
            refund = lp_share;
        }
    }

    if refund == 0 {
        reentrancy_exit();
        return 0;
    }

    // Cap refund to available collateral
    if refund > total_coll_market {
        refund = total_coll_market;
    }

    // Clear all positions
    for i in 0..outcome_count {
        save_position(market_id, user, i, 0, 0);
    }
    // Clear LP
    save_u64(&lp_key(market_id, user), 0);

    // Update collateral
    set_market_total_collateral(&mut record, total_coll_market - refund);
    save_market(market_id, &record);

    let total_coll = load_u64(TOTAL_COLLATERAL_KEY);
    save_u64(TOTAL_COLLATERAL_KEY, total_coll.saturating_sub(refund));

    reentrancy_exit();
    refund as u32
}

/// Withdraw liquidity from an ACTIVE market.
/// Returns mUSD returned.
pub fn withdraw_liquidity(
    provider_ptr: *const u8,
    market_id: u64,
    lp_shares_amount: u64,
) -> u32 {
    if !reentrancy_enter() { return 0; }
    if !require_not_paused() { reentrancy_exit(); return 0; }

    let mut provider = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(provider_ptr, provider.as_mut_ptr(), 32); }
    let provider = &provider[..];
    let caller = moltchain_sdk::get_caller();
    if caller.0[..] != provider[..] {
        reentrancy_exit();
        return 0;
    }

    let mut record = match load_market(market_id) {
        Some(r) => r,
        None => { reentrancy_exit(); return 0; }
    };

    if market_status(&record) != STATUS_ACTIVE {
        reentrancy_exit();
        return 0;
    }

    let user_lp = load_u64(&lp_key(market_id, provider));
    if lp_shares_amount == 0 || lp_shares_amount > user_lp {
        reentrancy_exit();
        return 0;
    }

    let total_lp = market_lp_total_shares(&record);
    let total_coll = market_total_collateral(&record);

    if total_lp == 0 {
        reentrancy_exit();
        return 0;
    }

    // mUSD returned = proportional share of collateral
    let musd_returned = (total_coll as u128 * lp_shares_amount as u128 / total_lp as u128) as u64;

    if musd_returned == 0 {
        reentrancy_exit();
        return 0;
    }

    // Reduce reserves proportionally
    let outcome_count = market_outcome_count(&record);
    for i in 0..outcome_count {
        let mut pool = match load_outcome_pool(market_id, i) {
            Some(p) => p,
            None => { reentrancy_exit(); return 0; }
        };
        let r = pool_reserve(&pool);
        let remove = (r as u128 * lp_shares_amount as u128 / total_lp as u128) as u64;
        set_pool_reserve(&mut pool, r.saturating_sub(remove));
        save_outcome_pool(market_id, i, &pool);
    }

    // Update LP
    save_u64(&lp_key(market_id, provider), user_lp - lp_shares_amount);
    set_market_lp_total_shares(&mut record, total_lp - lp_shares_amount);

    // Update collateral
    set_market_total_collateral(&mut record, total_coll - musd_returned);
    save_market(market_id, &record);

    let platform_coll = load_u64(TOTAL_COLLATERAL_KEY);
    save_u64(TOTAL_COLLATERAL_KEY, platform_coll.saturating_sub(musd_returned));

    reentrancy_exit();
    musd_returned as u32
}

// ============================================================================
// ADMIN FUNCTIONS
// ============================================================================

/// Emergency pause the entire contract.
pub fn emergency_pause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let caller = &caller[..];
    let mut caller_arr = [0u8; 32];
    caller_arr.copy_from_slice(caller);

    let actual_caller = moltchain_sdk::get_caller();
    if actual_caller.0[..] != caller[..] {
        return 0;
    }

    if !require_admin(&caller_arr) {
        return 0;
    }
    save_u8(PAUSED_KEY, 1);
    1
}

/// Unpause the contract.
pub fn emergency_unpause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let caller = &caller[..];
    let mut caller_arr = [0u8; 32];
    caller_arr.copy_from_slice(caller);

    let actual_caller = moltchain_sdk::get_caller();
    if actual_caller.0[..] != caller[..] {
        return 0;
    }

    if !require_admin(&caller_arr) {
        return 0;
    }
    save_u8(PAUSED_KEY, 0);
    1
}

/// Set MoltyID contract address.
pub fn set_moltyid_address(caller_ptr: *const u8, address_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let caller = &caller[..];
    let mut caller_arr = [0u8; 32];
    caller_arr.copy_from_slice(caller);

    let actual_caller = moltchain_sdk::get_caller();
    if actual_caller.0[..] != caller[..] {
        return 0;
    }

    if !require_admin(&caller_arr) {
        return 0;
    }
    let mut address = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(address_ptr, address.as_mut_ptr(), 32); }
    let address = &address[..];
    storage_set(MOLTYID_ADDR_KEY, address);
    1
}

/// Set MoltOracle contract address.
pub fn set_oracle_address(caller_ptr: *const u8, address_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let caller = &caller[..];
    let mut caller_arr = [0u8; 32];
    caller_arr.copy_from_slice(caller);

    let actual_caller = moltchain_sdk::get_caller();
    if actual_caller.0[..] != caller[..] {
        return 0;
    }

    if !require_admin(&caller_arr) {
        return 0;
    }
    let mut address = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(address_ptr, address.as_mut_ptr(), 32); }
    let address = &address[..];
    storage_set(ORACLE_ADDR_KEY, address);
    1
}

/// Set mUSD token contract address.
pub fn set_musd_address(caller_ptr: *const u8, address_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let caller = &caller[..];
    let mut caller_arr = [0u8; 32];
    caller_arr.copy_from_slice(caller);

    let actual_caller = moltchain_sdk::get_caller();
    if actual_caller.0[..] != caller[..] { return 0; }
    if !require_admin(&caller_arr) { return 0; }
    let mut address = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(address_ptr, address.as_mut_ptr(), 32); }
    let address = &address[..];
    storage_set(MUSD_ADDR_KEY, address);
    1
}

/// Set DEX governance address (for dispute resolution).
pub fn set_dex_gov_address(caller_ptr: *const u8, address_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    let caller = &caller[..];
    let mut caller_arr = [0u8; 32];
    caller_arr.copy_from_slice(caller);

    let actual_caller = moltchain_sdk::get_caller();
    if actual_caller.0[..] != caller[..] { return 0; }
    if !require_admin(&caller_arr) { return 0; }
    let mut address = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(address_ptr, address.as_mut_ptr(), 32); }
    let address = &address[..];
    storage_set(DEX_GOV_ADDR_KEY, address);
    1
}

// ============================================================================
// QUERY FUNCTIONS (read-only)
// ============================================================================

/// Get the number of markets created.
pub fn get_market_count() -> u64 {
    load_u64(MARKET_COUNT_KEY)
}

/// Get the market record (returns to return_data as 192-byte buffer).
pub fn get_market(market_id: u64) -> u32 {
    match load_market(market_id) {
        Some(data) => {
            moltchain_sdk::set_return_data(&data);
            1
        }
        None => 0,
    }
}

/// Get the outcome pool data for a specific market + outcome.
pub fn get_outcome_pool(market_id: u64, outcome: u8) -> u32 {
    match load_outcome_pool(market_id, outcome) {
        Some(data) => {
            moltchain_sdk::set_return_data(&data);
            1
        }
        None => 0,
    }
}

/// Get the current price of an outcome.
/// Returns price via return_data as u64 LE (6 decimals, mUSD basis).
pub fn get_price(market_id: u64, outcome: u8) -> u32 {
    let record = match load_market(market_id) {
        Some(r) => r,
        None => return 0,
    };
    let outcome_count = market_outcome_count(&record);
    if outcome >= outcome_count {
        return 0;
    }

    let mut reserves = Vec::with_capacity(outcome_count as usize);
    for i in 0..outcome_count {
        match load_outcome_pool(market_id, i) {
            Some(pool) => reserves.push(pool_reserve(&pool)),
            None => return 0,
        }
    }

    let price = calculate_price(&reserves, outcome);
    moltchain_sdk::set_return_data(&u64_to_bytes(price));
    1
}

/// Get a user's position for a market + outcome.
/// Returns 16-byte position data via return_data.
pub fn get_position(market_id: u64, user_ptr: *const u8, outcome: u8) -> u32 {
    let mut user = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(user_ptr, user.as_mut_ptr(), 32); }
    let user = &user[..];
    let (shares, cost) = load_position(market_id, user, outcome);
    moltchain_sdk::set_return_data(&encode_position(shares, cost));
    1
}

/// Get user's market participation count.
pub fn get_user_markets(user_ptr: *const u8) -> u64 {
    let mut user = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(user_ptr, user.as_mut_ptr(), 32); }
    let user = &user[..];
    load_u64(&user_market_count_key(user))
}

/// Quote a buy — returns estimated shares for a given mUSD input.
pub fn quote_buy(market_id: u64, outcome: u8, amount_musd: u64) -> u32 {
    let record = match load_market(market_id) {
        Some(r) => r,
        None => return 0,
    };
    let outcome_count = market_outcome_count(&record);
    if outcome >= outcome_count { return 0; }

    let mut reserves = Vec::with_capacity(outcome_count as usize);
    for i in 0..outcome_count {
        match load_outcome_pool(market_id, i) {
            Some(pool) => reserves.push(pool_reserve(&pool)),
            None => return 0,
        }
    }

    let (shares, _) = calculate_buy(&reserves, outcome, amount_musd);
    moltchain_sdk::set_return_data(&u64_to_bytes(shares));
    1
}

/// Quote a sell — returns estimated mUSD for a given shares input.
pub fn quote_sell(market_id: u64, outcome: u8, shares_amount: u64) -> u32 {
    let record = match load_market(market_id) {
        Some(r) => r,
        None => return 0,
    };
    let outcome_count = market_outcome_count(&record);
    if outcome >= outcome_count { return 0; }

    let mut reserves = Vec::with_capacity(outcome_count as usize);
    for i in 0..outcome_count {
        match load_outcome_pool(market_id, i) {
            Some(pool) => reserves.push(pool_reserve(&pool)),
            None => return 0,
        }
    }

    let (musd, _) = calculate_sell(&reserves, outcome, shares_amount);
    moltchain_sdk::set_return_data(&u64_to_bytes(musd));
    1
}

/// Get all pool reserves for a market.
/// Returns array of u64 LE reserves via return_data.
pub fn get_pool_reserves(market_id: u64) -> u32 {
    let record = match load_market(market_id) {
        Some(r) => r,
        None => return 0,
    };
    let outcome_count = market_outcome_count(&record);
    let mut data = Vec::with_capacity(outcome_count as usize * 8);
    for i in 0..outcome_count {
        match load_outcome_pool(market_id, i) {
            Some(pool) => data.extend_from_slice(&u64_to_bytes(pool_reserve(&pool))),
            None => return 0,
        }
    }
    moltchain_sdk::set_return_data(&data);
    1
}

/// Get global platform statistics.
/// Returns: market_count(8) + open_markets(8) + total_volume(8) + total_collateral(8) + fees(8) = 40 bytes
pub fn get_platform_stats() -> u32 {
    let mut data = Vec::with_capacity(40);
    data.extend_from_slice(&u64_to_bytes(load_u64(MARKET_COUNT_KEY)));
    data.extend_from_slice(&u64_to_bytes(load_u64(OPEN_MARKETS_KEY)));
    data.extend_from_slice(&u64_to_bytes(load_u64(TOTAL_VOLUME_KEY)));
    data.extend_from_slice(&u64_to_bytes(load_u64(TOTAL_COLLATERAL_KEY)));
    data.extend_from_slice(&u64_to_bytes(load_u64(FEES_COLLECTED_KEY)));
    moltchain_sdk::set_return_data(&data);
    1
}

/// Get LP balance for a user in a market.
pub fn get_lp_balance(market_id: u64, user_ptr: *const u8) -> u32 {
    let mut user = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(user_ptr, user.as_mut_ptr(), 32); }
    let user = &user[..];
    let balance = load_u64(&lp_key(market_id, user));
    moltchain_sdk::set_return_data(&u64_to_bytes(balance));
    1
}

/// Get the fee treasury (protocol fees).
pub fn get_fee_treasury() -> u64 {
    load_u64(FEES_COLLECTED_KEY)
}

// ============================================================================
// WASM ENTRY POINT — Opcode Dispatch
// ============================================================================

#[cfg(target_arch = "wasm32")]
#[no_mangle]
pub extern "C" fn call() {
    let args = moltchain_sdk::get_args();
    if args.is_empty() {
        return;
    }

    match args[0] {
        // 0: initialize
        0 => {
            if args.len() >= 33 {
                let result = initialize(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 1: create_market
        1 => {
            // [creator 32B][category 1B][close_slot 8B][outcome_count 1B][question_hash 32B][question_len 4B][question...]
            if args.len() >= 1 + 32 + 1 + 8 + 1 + 32 + 4 {
                let creator_ptr = args[1..33].as_ptr();
                let category = args[33];
                let close_slot = bytes_to_u64(&args[34..42]);
                let outcome_count = args[42];
                let qh_ptr = args[43..75].as_ptr();
                let q_len = u32::from_le_bytes([args[75], args[76], args[77], args[78]]);
                if args.len() >= 79 + q_len as usize {
                    let q_ptr = args[79..].as_ptr();
                    let result = create_market(
                        creator_ptr, category, close_slot, outcome_count,
                        qh_ptr, q_ptr, q_len,
                    );
                    moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
                }
            }
        }
        // 2: add_initial_liquidity
        2 => {
            // [provider 32B][market_id 8B][amount_musd 8B][odds_bps array (2B × outcomes)]
            if args.len() >= 1 + 32 + 8 + 8 {
                let provider_ptr = args[1..33].as_ptr();
                let mid = bytes_to_u64(&args[33..41]);
                let amount = bytes_to_u64(&args[41..49]);
                let odds_ptr = if args.len() > 49 { args[49..].as_ptr() } else { core::ptr::null() };
                let odds_len = if args.len() > 49 { (args.len() - 49) as u32 } else { 0 };
                let result = add_initial_liquidity(provider_ptr, mid, amount, odds_ptr, odds_len);
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 3: add_liquidity
        3 => {
            if args.len() >= 1 + 32 + 8 + 8 {
                let result = add_liquidity(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    bytes_to_u64(&args[41..49]),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 4: buy_shares
        4 => {
            if args.len() >= 1 + 32 + 8 + 1 + 8 {
                let result = buy_shares(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    args[41],
                    bytes_to_u64(&args[42..50]),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 5: sell_shares
        5 => {
            if args.len() >= 1 + 32 + 8 + 1 + 8 {
                let result = sell_shares(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    args[41],
                    bytes_to_u64(&args[42..50]),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 6: mint_complete_set
        6 => {
            if args.len() >= 1 + 32 + 8 + 8 {
                let result = mint_complete_set(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    bytes_to_u64(&args[41..49]),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 7: redeem_complete_set
        7 => {
            if args.len() >= 1 + 32 + 8 + 8 {
                let result = redeem_complete_set(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    bytes_to_u64(&args[41..49]),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 8: submit_resolution
        8 => {
            if args.len() >= 1 + 32 + 8 + 1 + 32 + 8 {
                let result = submit_resolution(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    args[41],
                    args[42..74].as_ptr(),
                    bytes_to_u64(&args[74..82]),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 9: challenge_resolution
        9 => {
            if args.len() >= 1 + 32 + 8 + 32 + 8 {
                let result = challenge_resolution(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    args[41..73].as_ptr(),
                    bytes_to_u64(&args[73..81]),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 10: finalize_resolution
        10 => {
            if args.len() >= 1 + 32 + 8 {
                let result = finalize_resolution(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 11: dao_resolve
        11 => {
            if args.len() >= 1 + 32 + 8 + 1 {
                let result = dao_resolve(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    args[41],
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 12: dao_void
        12 => {
            if args.len() >= 1 + 32 + 8 {
                let result = dao_void(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 13: redeem_shares
        13 => {
            if args.len() >= 1 + 32 + 8 + 1 {
                let result = redeem_shares(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    args[41],
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 14: reclaim_collateral
        14 => {
            if args.len() >= 1 + 32 + 8 {
                let result = reclaim_collateral(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 15: withdraw_liquidity
        15 => {
            if args.len() >= 1 + 32 + 8 + 8 {
                let result = withdraw_liquidity(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                    bytes_to_u64(&args[41..49]),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 16: emergency_pause
        16 => {
            if args.len() >= 33 {
                let result = emergency_pause(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 17: emergency_unpause
        17 => {
            if args.len() >= 33 {
                let result = emergency_unpause(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 18: set_moltyid_address
        18 => {
            if args.len() >= 65 {
                let result = set_moltyid_address(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 19: set_oracle_address
        19 => {
            if args.len() >= 65 {
                let result = set_oracle_address(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 20: set_musd_address
        20 => {
            if args.len() >= 65 {
                let result = set_musd_address(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 21: set_dex_gov_address
        21 => {
            if args.len() >= 65 {
                let result = set_dex_gov_address(args[1..33].as_ptr(), args[33..65].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }
        // 22: close_market
        22 => {
            if args.len() >= 1 + 32 + 8 {
                let result = close_market(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                );
                moltchain_sdk::set_return_data(&u64_to_bytes(result as u64));
            }
        }

        // QUERIES (0x20+)
        23 => {
            // get_market
            if args.len() >= 9 {
                get_market(bytes_to_u64(&args[1..9]));
            }
        }
        24 => {
            // get_outcome_pool
            if args.len() >= 10 {
                get_outcome_pool(bytes_to_u64(&args[1..9]), args[9]);
            }
        }
        25 => {
            // get_price
            if args.len() >= 10 {
                get_price(bytes_to_u64(&args[1..9]), args[9]);
            }
        }
        26 => {
            // get_position
            if args.len() >= 42 {
                get_position(bytes_to_u64(&args[1..9]), args[9..41].as_ptr(), args[41]);
            }
        }
        27 => {
            // get_market_count
            let count = get_market_count();
            moltchain_sdk::set_return_data(&u64_to_bytes(count));
        }
        28 => {
            // get_user_markets
            if args.len() >= 33 {
                let count = get_user_markets(args[1..33].as_ptr());
                moltchain_sdk::set_return_data(&u64_to_bytes(count));
            }
        }
        29 => {
            // quote_buy
            if args.len() >= 18 {
                quote_buy(bytes_to_u64(&args[1..9]), args[9], bytes_to_u64(&args[10..18]));
            }
        }
        30 => {
            // quote_sell
            if args.len() >= 18 {
                quote_sell(bytes_to_u64(&args[1..9]), args[9], bytes_to_u64(&args[10..18]));
            }
        }
        31 => {
            // get_pool_reserves
            if args.len() >= 9 {
                get_pool_reserves(bytes_to_u64(&args[1..9]));
            }
        }
        32 => {
            // get_platform_stats
            get_platform_stats();
        }
        33 => {
            // get_lp_balance
            if args.len() >= 41 {
                get_lp_balance(bytes_to_u64(&args[1..9]), args[9..41].as_ptr());
            }
        }
        _ => { moltchain_sdk::set_return_data(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]); }
    }
}
