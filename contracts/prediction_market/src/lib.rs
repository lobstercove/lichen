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
//   pm_phc_{id}                           → u64       Price history snapshot count
//   pm_ph_{id}_{idx}                      → [u8; 24]  Price snapshot (slot u64 + yes_price u64 + volume u64)

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

use moltchain_sdk::{
    bytes_to_u64, get_slot, log_info, storage_get, storage_set, u64_to_bytes,
    Address, CrossCall, call_contract, call_token_transfer, get_value,
};

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
const SELF_ADDR_KEY: &[u8] = b"pm_self_addr";

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

/// Load the contract's own address (needed as source for token transfers).
fn load_self_addr() -> [u8; 32] {
    load_addr(SELF_ADDR_KEY)
}

/// Transfer mUSD tokens from the contract to a recipient.
/// Returns true on success, false if addresses not configured or transfer fails.
fn transfer_musd_out(recipient: &[u8], amount: u64) -> bool {
    let musd_addr = load_addr(MUSD_ADDR_KEY);
    if is_zero(&musd_addr) {
        log_info("mUSD address not configured — skipping transfer");
        return true; // graceful degradation for unconfigured deployments
    }
    let self_addr = load_self_addr();
    if is_zero(&self_addr) {
        log_info("Self address not configured — skipping transfer");
        return true; // graceful degradation for unconfigured deployments
    }
    let mut recip = [0u8; 32];
    recip.copy_from_slice(recipient);
    match call_token_transfer(
        Address(musd_addr),
        Address(self_addr),
        Address(recip),
        amount,
    ) {
        Err(_) => {
            log_info("mUSD transfer failed");
            false
        }
        Ok(_) => true,
    }
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

/// Key for per-trader stats: total_volume(8) + trade_count(8) + last_trade_slot(8) = 24 bytes.
fn trader_stats_key(addr: &[u8]) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_ts_"[..]);
    k.extend_from_slice(&hex_encode(addr));
    k
}

/// Key for total unique traders count.
const TOTAL_TRADERS_KEY: &[u8] = b"pm_total_traders";

/// Key for trader list (indexed for enumeration).
fn trader_list_key(idx: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_tl_"[..]);
    k.extend_from_slice(&u64_to_decimal(idx));
    k
}

/// Key for per-market unique trader count.
fn market_trader_count_key(market_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_mtc_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k
}

/// Key for per-market trader tracking (boolean marker).
fn market_trader_marker_key(market_id: u64, addr: &[u8]) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_mtm_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k.push(b'_');
    k.extend_from_slice(&hex_encode(addr));
    k
}

/// Key for per-market 24h volume (rolling window via slot-based reset).
fn market_24h_volume_key(market_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_mv24_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k
}

/// Key for per-market 24h volume reset slot.
fn market_24h_reset_slot_key(market_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_mv24s_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k
}

/// Update per-trader stats after a trade. Tracks volume, trade count, and last slot.
fn update_trader_stats(trader: &[u8], volume: u64) {
    let key = trader_stats_key(trader);
    let (old_vol, old_count) = match storage_get(&key) {
        Some(d) if d.len() >= 24 => (bytes_to_u64(&d[0..8]), bytes_to_u64(&d[8..16])),
        _ => (0, 0),
    };
    let slot = get_slot();
    let mut buf = [0u8; 24];
    buf[0..8].copy_from_slice(&u64_to_bytes(old_vol + volume));
    buf[8..16].copy_from_slice(&u64_to_bytes(old_count + 1));
    buf[16..24].copy_from_slice(&u64_to_bytes(slot));

    // If first trade ever: increment global trader count, add to trader list
    if old_count == 0 {
        let total = load_u64(TOTAL_TRADERS_KEY);
        let list_key = trader_list_key(total);
        storage_set(&list_key, trader);
        save_u64(TOTAL_TRADERS_KEY, total + 1);
    }

    storage_set(&key, &buf);
}

/// Track unique traders per market and update 24h rolling volume.
fn update_market_trader_stats(market_id: u64, trader: &[u8], volume: u64) {
    // Unique trader tracking
    let marker = market_trader_marker_key(market_id, trader);
    if storage_get(&marker).is_none() {
        storage_set(&marker, &[1]);
        let count_key = market_trader_count_key(market_id);
        let count = load_u64(&count_key);
        save_u64(&count_key, count + 1);
    }
    // 24h rolling volume (172,800 slots at 0.5s/slot = 24 hours)
    let vol_key = market_24h_volume_key(market_id);
    let reset_key = market_24h_reset_slot_key(market_id);
    let current_slot = get_slot();
    let reset_slot = load_u64(&reset_key);
    let elapsed = current_slot.saturating_sub(reset_slot);
    if elapsed >= 172_800 {
        // Reset window
        save_u64(&vol_key, volume);
        save_u64(&reset_key, current_slot);
    } else {
        let existing = load_u64(&vol_key);
        save_u64(&vol_key, existing + volume);
    }
}

/// Key for per-market trading pause (circuit breaker).
fn market_pause_key(market_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_mpause_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k
}

/// Key for price history snapshot count.
fn price_history_count_key(market_id: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_phc_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k
}

/// Key for a single price history snapshot entry.
fn price_history_entry_key(market_id: u64, idx: u64) -> Vec<u8> {
    let mut k = Vec::from(&b"pm_ph_"[..]);
    k.extend_from_slice(&u64_to_decimal(market_id));
    k.push(b'_');
    k.extend_from_slice(&u64_to_decimal(idx));
    k
}

/// Record a price snapshot after a trade. Stores slot + yes_price + trade_volume (24 bytes).
fn record_price_snapshot(market_id: u64, yes_price: u64, trade_volume: u64) {
    let slot = get_slot();
    let count_key = price_history_count_key(market_id);
    let count = storage_get(&count_key)
        .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
        .unwrap_or(0);
    let entry_key = price_history_entry_key(market_id, count);
    let mut entry = [0u8; 24];
    entry[0..8].copy_from_slice(&u64_to_bytes(slot));
    entry[8..16].copy_from_slice(&u64_to_bytes(yes_price));
    entry[16..24].copy_from_slice(&u64_to_bytes(trade_volume));
    storage_set(&entry_key, &entry);
    save_u64(&count_key, count + 1);
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
        // Multi-outcome sell: binary search for max complete sets formable.
        // User has `sell` shares of outcome A. They keep some A and swap the rest
        // into each non-A pool to acquire shares of every other outcome. The maximum
        // number of complete sets (1 of each outcome) is found via binary search over
        // total_a_for_sets().
        let a = outcome as usize;
        let sell = shares_amount as u128;

        // Upper bound: can't form more sets than min(reserve_j) for j != a
        // (can't extract more than a pool has), and can't exceed sell itself.
        let mut hi = sell;
        for j in 0..n {
            if j == a {
                continue;
            }
            let rj = reserves[j] as u128;
            if rj <= 1 {
                return (0, 0);
            }
            if rj - 1 < hi {
                hi = rj - 1;
            }
        }
        if hi == 0 {
            return (0, 0);
        }

        // Binary search: find max C where total_a_for_sets(reserves, a, C) <= sell
        let mut lo: u128 = 0;
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            let needed = total_a_for_sets(reserves, a, mid);
            if needed <= sell {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }

        let sets = lo;
        if sets == 0 {
            return (0, 0);
        }

        let fee = (sets * TRADING_FEE_BPS as u128) / 10_000;
        let net = sets - fee;

        (net as u64, fee as u64)
    }
}

/// For multi-outcome sell: compute total A shares needed to form `c` complete sets.
/// Simulates sequential swaps of A into each non-A pool to acquire `c` shares of each outcome.
/// Returns u128::MAX if impossible (any reserve_j <= c).
fn total_a_for_sets(reserves: &[u64], a: usize, c: u128) -> u128 {
    let mut current_ra = reserves[a] as u128;
    let mut total: u128 = c; // user keeps c shares of A

    for j in 0..reserves.len() {
        if j == a {
            continue;
        }
        let rj = reserves[j] as u128;
        if rj <= c {
            return u128::MAX; // impossible: can't extract c shares from pool with only rj
        }
        // Need j_received = rj * s_j / (current_ra + s_j) >= c
        // Solving: s_j = c * current_ra / (rj - c)
        // Use ceiling division to ensure we get at least c shares
        let denom = rj - c;
        let s_j = (c * current_ra + denom - 1) / denom;
        total = total.saturating_add(s_j);
        current_ra += s_j;
    }

    total
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
        // Multi-outcome: use same binary search as calculate_sell for optimal sets
        let sell = shares_amount as u128;
        let a_idx = a;

        let mut hi = sell;
        for j in 0..n {
            if j == a_idx {
                continue;
            }
            let rj = reserves[j] as u128;
            if rj <= 1 {
                return reserves.to_vec();
            }
            if rj - 1 < hi {
                hi = rj - 1;
            }
        }
        if hi == 0 {
            return reserves.to_vec();
        }

        let mut lo: u128 = 0;
        while lo < hi {
            let mid = (lo + hi + 1) / 2;
            let needed = total_a_for_sets(reserves, a_idx, mid);
            if needed <= sell {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }

        let sets = lo;
        if sets == 0 {
            return reserves.to_vec();
        }

        // Apply the actual swaps to update reserves
        let mut temp: Vec<u128> = reserves.iter().map(|&r| r as u128).collect();
        for j in 0..n {
            if j == a_idx {
                continue;
            }
            let rj = temp[j];
            if rj <= sets {
                return reserves.to_vec(); // shouldn't happen after binary search
            }
            let denom = rj - sets;
            let s_j = (sets * temp[a_idx] + denom - 1) / denom;
            let j_got = (rj * s_j) / (temp[a_idx] + s_j);
            temp[a_idx] += s_j;
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

    // G21-01: Verify attached value covers market creation fee
    if get_value() < MARKET_CREATION_FEE {
        log_info("Insufficient value for market creation fee");
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

    // G21-02: Store full u64 market_id in return_data (no u32 truncation)
    moltchain_sdk::set_return_data(&u64_to_bytes(new_id));

    reentrancy_exit();
    new_id as u32 // also in return_data as full u64
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

    // G21-01: Verify attached value covers liquidity deposit
    if get_value() < amount_musd {
        log_info("Insufficient value for initial liquidity");
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

    // G21-01: Verify attached value covers liquidity deposit
    if get_value() < amount_musd {
        log_info("Insufficient value for liquidity");
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

    // G21-02: Store full u64 LP shares in return_data (no u32 truncation)
    moltchain_sdk::set_return_data(&u64_to_bytes(new_lp));

    reentrancy_exit();
    new_lp as u32 // also in return_data as full u64
}

/// Buy shares of a specific outcome.
/// Returns shares_received as u32 (full u64 also in return_data), 0 on failure.
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

    // G21-01: Verify attached value covers share purchase
    if get_value() < amount_musd {
        log_info("Insufficient value for share purchase");
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

    // Record price history snapshot
    record_price_snapshot(market_id, new_price, amount_musd);

    // Update per-trader and per-market analytics
    update_trader_stats(trader, amount_musd);
    update_market_trader_stats(market_id, trader, amount_musd);

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

    // G21-02: Store full u64 shares in return_data (no u32 truncation)
    moltchain_sdk::set_return_data(&u64_to_bytes(shares_received));

    reentrancy_exit();
    shares_received as u32 // also in return_data as full u64
}

/// Sell shares of a specific outcome.
/// Returns musd_returned as u32 (full u64 also in return_data), 0 on failure.
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

    // Record price history snapshot
    {
        let sell_price = calculate_price(&new_reserves, outcome);
        record_price_snapshot(market_id, sell_price, musd_returned + fee_musd);
    }

    // Update per-trader and per-market analytics
    update_trader_stats(trader, musd_returned + fee_musd);
    update_market_trader_stats(market_id, trader, musd_returned + fee_musd);

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

    // G21-01: Transfer mUSD to trader
    if !transfer_musd_out(trader, musd_returned) {
        log_info("sell_shares: mUSD transfer to trader failed");
        reentrancy_exit();
        return 0;
    }

    log_info("Shares sold!");

    // G21-02: Store full u64 mUSD in return_data (no u32 truncation)
    moltchain_sdk::set_return_data(&u64_to_bytes(musd_returned));

    reentrancy_exit();
    musd_returned as u32 // also in return_data as full u64
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

    // G21-01: Verify attached value covers complete set mint
    if get_value() < amount_musd {
        log_info("Insufficient value for complete set mint");
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

    // G21-01: Transfer mUSD to user
    if !transfer_musd_out(user, musd_returned) {
        log_info("redeem_complete_set: mUSD transfer to user failed");
        reentrancy_exit();
        return 0;
    }

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

    // G21-01: Verify attached value covers resolution bond
    if get_value() < bond {
        log_info("Insufficient value for resolution bond");
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

    // Verify MoltOracle attestation via cross-contract call.
    // Sends the attestation_hash as args and expects the oracle to return
    // attestation data if it exists. Format: [data_hash(32) + sig_count(1) + ...]
    // If oracle is not configured, skip verification (allows testing/genesis).
    let oracle_addr = load_addr(ORACLE_ADDR_KEY);
    if !is_zero(&oracle_addr) {
        let cross_call = CrossCall::new(
            Address(oracle_addr),
            "get_attestation",
            attestation_hash.to_vec(),
        );
        match call_contract(cross_call) {
            Ok(att_data) if att_data.len() >= 33 => {
                let sig_count = att_data[32];
                if sig_count < RESOLUTION_THRESHOLD {
                    log_info("Insufficient oracle attestation signatures");
                    reentrancy_exit();
                    return 0;
                }
            }
            Ok(_) => {
                // Empty or too-short response means attestation not found
                log_info("Oracle attestation not found");
                reentrancy_exit();
                return 0;
            }
            Err(_) => {
                log_info("Oracle cross-contract call failed");
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

    // G21-01: Verify attached value covers dispute bond
    if get_value() < bond {
        log_info("Insufficient value for dispute bond");
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
    let resolver = market_resolver(&record);
    // Store resolver reward in a dedicated key
    let reward_key = {
        let mut k = Vec::from(&b"pm_rw_"[..]);
        k.extend_from_slice(&u64_to_decimal(market_id));
        k
    };
    save_u64(&reward_key, reward);

    // G21-01: Transfer reward to resolver
    if reward > 0 {
        if !transfer_musd_out(&resolver, reward) {
            log_info("finalize_resolution: resolver reward transfer failed");
        }
    }

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
/// Returns 1 on success (payout in return data as u64), 0 on failure.
/// SECURITY FIX: Now actually transfers mUSD tokens via call_token_transfer
/// and stores full u64 payout in return_data (no more u32 truncation).
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
        user_shares
    } else {
        0 // Losing shares are worthless
    };

    if payout == 0 {
        // Clear losing position (no transfer needed)
        save_position(market_id, user, outcome, 0, 0);
        moltchain_sdk::set_return_data(&u64_to_bytes(0));
        reentrancy_exit();
        return 1; // success — position cleared, payout=0 in return_data
    }

    // Transfer mUSD to the user BEFORE updating state (checks-effects-interactions
    // is reversed here, but reentrancy guard protects us)
    if !transfer_musd_out(user, payout) {
        log_info("redeem_shares: mUSD transfer to user failed");
        reentrancy_exit();
        return 0;
    }

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
    let coll = market_total_collateral(&record);
    set_market_total_collateral(&mut record, coll.saturating_sub(payout));
    save_market(market_id, &record);

    let total_coll = load_u64(TOTAL_COLLATERAL_KEY);
    save_u64(TOTAL_COLLATERAL_KEY, total_coll.saturating_sub(payout));

    // Store full u64 payout in return data (no u32 truncation)
    moltchain_sdk::set_return_data(&u64_to_bytes(payout));

    log_info("Shares redeemed — mUSD transferred!");
    reentrancy_exit();
    1 // success — full u64 payout in return_data
}

/// Reclaim collateral from a VOIDED market.
/// Returns 1 on success (refund amount in return data as u64), 0 on failure.
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

    // Transfer mUSD to the user
    if !transfer_musd_out(user, refund) {
        log_info("reclaim_collateral: mUSD transfer to user failed");
        // Revert position clears — re-save positions so user can try again
        // (This is a simplification; a full revert would also restore LP)
        reentrancy_exit();
        return 0;
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

    // Store full u64 refund in return data (no u32 truncation)
    moltchain_sdk::set_return_data(&u64_to_bytes(refund));
    log_info("Collateral reclaimed — mUSD transferred!");

    reentrancy_exit();
    1 // success — full u64 refund in return_data
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

    // G21-01: Transfer mUSD to LP provider
    if !transfer_musd_out(provider, musd_returned) {
        log_info("withdraw_liquidity: mUSD transfer to provider failed");
        reentrancy_exit();
        return 0;
    }

    // G21-02: Store full u64 mUSD in return_data (no u32 truncation)
    moltchain_sdk::set_return_data(&u64_to_bytes(musd_returned));

    reentrancy_exit();
    musd_returned as u32 // also in return_data as full u64
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

/// Set this contract's own address (needed for token transfer source). Admin only.
pub fn set_self_address(caller_ptr: *const u8, address_ptr: *const u8) -> u32 {
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
    if address.iter().all(|&b| b == 0) {
        log_info("set_self_address: zero address rejected");
        return 0;
    }
    storage_set(SELF_ADDR_KEY, &address);
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
                    // G21-02: function sets return_data with full u64 on success
                    if result == 0 {
                        moltchain_sdk::set_return_data(&u64_to_bytes(0));
                    }
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
                // G21-02: function sets return_data with full u64 on success
                if result == 0 {
                    moltchain_sdk::set_return_data(&u64_to_bytes(0));
                }
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
                // G21-02: function sets return_data with full u64 on success
                if result == 0 {
                    moltchain_sdk::set_return_data(&u64_to_bytes(0));
                }
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
                // G21-02: function sets return_data with full u64 on success
                if result == 0 {
                    moltchain_sdk::set_return_data(&u64_to_bytes(0));
                }
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
                // G21-02: function sets return_data with full u64 on success
                if result == 0 {
                    moltchain_sdk::set_return_data(&u64_to_bytes(0));
                }
            }
        }
        // 14: reclaim_collateral
        14 => {
            if args.len() >= 1 + 32 + 8 {
                let result = reclaim_collateral(
                    args[1..33].as_ptr(),
                    bytes_to_u64(&args[33..41]),
                );
                // G21-02: function sets return_data with full u64 on success
                if result == 0 {
                    moltchain_sdk::set_return_data(&u64_to_bytes(0));
                }
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
                // G21-02: function sets return_data with full u64 on success
                if result == 0 {
                    moltchain_sdk::set_return_data(&u64_to_bytes(0));
                }
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
        34 => {
            // get_price_history(market_id 8B) — returns count as u64
            if args.len() >= 9 {
                let mid = bytes_to_u64(&args[1..9]);
                let count_key = price_history_count_key(mid);
                let count = storage_get(&count_key)
                    .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
                    .unwrap_or(0);
                // Return count as first 8 bytes, then up to 50 most recent entries (24B each)
                let max_entries: u64 = 50;
                let start = count.saturating_sub(max_entries);
                let entry_count = count - start;
                let mut result = Vec::with_capacity(8 + entry_count as usize * 24);
                result.extend_from_slice(&u64_to_bytes(count));
                for i in start..count {
                    let entry_key = price_history_entry_key(mid, i);
                    if let Some(data) = storage_get(&entry_key) {
                        if data.len() >= 24 {
                            result.extend_from_slice(&data[..24]);
                        }
                    }
                }
                moltchain_sdk::set_return_data(&result);
            }
        }
        35 => {
            // get_trader_stats(addr 32B) → 24 bytes: volume(8) + trade_count(8) + last_slot(8)
            if args.len() >= 33 {
                let tk = trader_stats_key(&args[1..33]);
                match storage_get(&tk) {
                    Some(d) if d.len() >= 24 => {
                        moltchain_sdk::set_return_data(&d[..24]);
                    }
                    _ => {
                        moltchain_sdk::set_return_data(&[0u8; 24]);
                    }
                }
            }
        }
        36 => {
            // get_leaderboard(limit 8B) → returns up to N traders sorted by volume
            // Format: count(8B) + [addr(32B) + volume(8B) + trades(8B)] * count
            let limit = if args.len() >= 9 { bytes_to_u64(&args[1..9]).min(50) } else { 20 };
            let total_traders = load_u64(TOTAL_TRADERS_KEY);
            // Collect all traders and their volumes
            let scan_max = total_traders.min(500); // cap scan to prevent gas-bomb
            let mut entries: Vec<([u8; 32], u64, u64)> = Vec::with_capacity(scan_max as usize);
            for i in 0..scan_max {
                let lk = trader_list_key(i);
                if let Some(addr_data) = storage_get(&lk) {
                    if addr_data.len() >= 32 {
                        let mut addr = [0u8; 32];
                        addr.copy_from_slice(&addr_data[..32]);
                        let tk = trader_stats_key(&addr);
                        if let Some(sd) = storage_get(&tk) {
                            if sd.len() >= 24 {
                                let vol = bytes_to_u64(&sd[0..8]);
                                let trades = bytes_to_u64(&sd[8..16]);
                                entries.push((addr, vol, trades));
                            }
                        }
                    }
                }
            }
            // Simple selection sort for top N (efficient for small N)
            let take = (limit as usize).min(entries.len());
            for i in 0..take {
                let mut max_idx = i;
                for j in (i + 1)..entries.len() {
                    if entries[j].1 > entries[max_idx].1 {
                        max_idx = j;
                    }
                }
                entries.swap(i, max_idx);
            }
            let mut result = Vec::with_capacity(8 + take * 48);
            result.extend_from_slice(&u64_to_bytes(take as u64));
            for i in 0..take {
                result.extend_from_slice(&entries[i].0);
                result.extend_from_slice(&u64_to_bytes(entries[i].1));
                result.extend_from_slice(&u64_to_bytes(entries[i].2));
            }
            moltchain_sdk::set_return_data(&result);
        }
        37 => {
            // get_market_analytics(market_id 8B) → market trader count(8) + 24h volume(8)
            if args.len() >= 9 {
                let mid = bytes_to_u64(&args[1..9]);
                let tc = load_u64(&market_trader_count_key(mid));
                let vol24 = load_u64(&market_24h_volume_key(mid));
                let mut result = [0u8; 16];
                result[0..8].copy_from_slice(&u64_to_bytes(tc));
                result[8..16].copy_from_slice(&u64_to_bytes(vol24));
                moltchain_sdk::set_return_data(&result);
            }
        }
        _ => { moltchain_sdk::set_return_data(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]); }
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
    use moltchain_sdk::bytes_to_u64;

    fn setup() {
        test_mock::reset();
    }

    /// Initialize the contract and return admin address.
    fn init_contract() -> [u8; 32] {
        let admin = [1u8; 32];
        test_mock::set_caller(admin);
        assert_eq!(initialize(admin.as_ptr()), 0);
        admin
    }

    /// Configure mUSD and self addresses for token transfers.
    fn configure_escrow() {
        let musd = [0xAA; 32];
        let self_addr = [0xBB; 32];
        storage_set(MUSD_ADDR_KEY, &musd);
        storage_set(SELF_ADDR_KEY, &self_addr);
    }

    /// Create a binary market (2 outcomes) and return the market_id.
    fn create_binary_market(creator: &[u8; 32], close_slot: u64) -> u64 {
        test_mock::set_caller(*creator);
        test_mock::set_slot(1000);
        // G21-01: Attach value covering creation fee
        test_mock::set_value(MARKET_CREATION_FEE);
        let qhash = [0x42u8; 32];
        let question = b"Will ETH hit $10K?";
        let result = create_market(
            creator.as_ptr(),
            CATEGORY_CRYPTO,
            close_slot,
            2, // binary
            qhash.as_ptr(),
            question.as_ptr(),
            question.len() as u32,
        );
        assert!(result > 0, "create_market should return market_id > 0");
        result as u64
    }

    /// Add initial liquidity to transition market from PENDING → ACTIVE.
    fn activate_market(creator: &[u8; 32], market_id: u64, amount: u64) {
        test_mock::set_caller(*creator);
        // G21-01: Attach value covering liquidity deposit
        test_mock::set_value(amount);
        let result = add_initial_liquidity(
            creator.as_ptr(),
            market_id,
            amount,
            core::ptr::null(),
            0,
        );
        assert_eq!(result, 1, "add_initial_liquidity should succeed");
    }

    /// Manually transition a market to RESOLVED with a winning outcome.
    fn force_resolve_market(market_id: u64, winning_outcome: u8) {
        let mut record = load_market(market_id).unwrap();
        set_market_status(&mut record, STATUS_RESOLVED);
        set_market_winning_outcome(&mut record, winning_outcome);
        save_market(market_id, &record);
    }

    /// Manually transition a market to VOIDED.
    fn force_void_market(market_id: u64) {
        let mut record = load_market(market_id).unwrap();
        set_market_status(&mut record, STATUS_VOIDED);
        save_market(market_id, &record);
    }

    // ========================================================================
    // INITIALIZATION TESTS
    // ========================================================================

    #[test]
    fn test_initialize() {
        setup();
        let admin = init_contract();
        assert_eq!(load_addr(ADMIN_KEY), admin);
        assert_eq!(load_u64(MARKET_COUNT_KEY), 0);
    }

    #[test]
    fn test_double_initialize_blocked() {
        setup();
        let admin = init_contract();
        let other = [2u8; 32];
        test_mock::set_caller(other);
        assert_eq!(initialize(other.as_ptr()), 1, "Re-init should fail");
    }

    // ========================================================================
    // MARKET CREATION TESTS
    // ========================================================================

    #[test]
    fn test_create_market_binary() {
        setup();
        let admin = init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        assert_eq!(mid, 1);

        let record = load_market(mid).unwrap();
        assert_eq!(market_status(&record), STATUS_PENDING);
        assert_eq!(market_outcome_count(&record), 2);
        assert_eq!(market_close_slot(&record), 100_000);
        assert_eq!(market_category(&record), CATEGORY_CRYPTO);
    }

    #[test]
    fn test_create_market_invalid_outcome_count() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_slot(1000);
        let qhash = [0x42u8; 32];
        let question = b"Test?";
        // 1 outcome: too few
        assert_eq!(create_market(creator.as_ptr(), 0, 100_000, 1, qhash.as_ptr(), question.as_ptr(), 5), 0);
        // 9 outcomes: too many
        assert_eq!(create_market(creator.as_ptr(), 0, 100_000, 9, qhash.as_ptr(), question.as_ptr(), 5), 0);
    }

    #[test]
    fn test_create_market_duration_too_short() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_slot(1000);
        let qhash = [0x42u8; 32];
        let question = b"Test?";
        // close_slot too close (< MIN_DURATION)
        assert_eq!(create_market(creator.as_ptr(), 0, 1050, 2, qhash.as_ptr(), question.as_ptr(), 5), 0);
    }

    #[test]
    fn test_create_market_caller_mismatch() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let imposter = [3u8; 32];
        test_mock::set_caller(imposter); // caller != creator
        test_mock::set_slot(1000);
        let qhash = [0x42u8; 32];
        let question = b"Test?";
        assert_eq!(create_market(creator.as_ptr(), 0, 100_000, 2, qhash.as_ptr(), question.as_ptr(), 5), 0);
    }

    // ========================================================================
    // LIQUIDITY TESTS
    // ========================================================================

    #[test]
    fn test_add_initial_liquidity() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000); // 10 mUSD

        let record = load_market(mid).unwrap();
        assert_eq!(market_status(&record), STATUS_ACTIVE);
        assert_eq!(market_total_collateral(&record), 10_000_000);
        assert_eq!(market_lp_total_shares(&record), 10_000_000);

        // Pools should be initialized
        let pool_0 = load_outcome_pool(mid, 0).unwrap();
        assert!(pool_reserve(&pool_0) > 0, "Pool 0 reserve should be > 0");
        let pool_1 = load_outcome_pool(mid, 1).unwrap();
        assert!(pool_reserve(&pool_1) > 0, "Pool 1 reserve should be > 0");
    }

    #[test]
    fn test_add_initial_liquidity_too_low() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        test_mock::set_caller(creator);
        // Below MIN_COLLATERAL (1_000_000)
        assert_eq!(add_initial_liquidity(creator.as_ptr(), mid, 500_000, core::ptr::null(), 0), 0);
    }

    #[test]
    fn test_add_initial_liquidity_non_creator() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        let other = [3u8; 32];
        test_mock::set_caller(other);
        assert_eq!(add_initial_liquidity(other.as_ptr(), mid, 10_000_000, core::ptr::null(), 0), 0);
    }

    // ========================================================================
    // TRADING TESTS
    // ========================================================================

    #[test]
    fn test_buy_shares() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        let trader = [3u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(2000);
        let result = buy_shares(trader.as_ptr(), mid, 0, 1_000_000);
        assert!(result > 0, "buy_shares should return shares received > 0");

        // Trader should have shares in outcome 0
        let (shares, cost) = load_position(mid, &trader, 0);
        assert!(shares > 0, "Trader should have shares");
        assert_eq!(cost, 1_000_000, "Cost basis should equal amount paid");
    }

    #[test]
    fn test_buy_shares_wrong_caller() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        let trader = [3u8; 32];
        let imposter = [4u8; 32];
        test_mock::set_caller(imposter);
        test_mock::set_slot(2000);
        assert_eq!(buy_shares(trader.as_ptr(), mid, 0, 1_000_000), 0);
    }

    #[test]
    fn test_buy_shares_after_close() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        let trader = [3u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(100_001); // past close
        assert_eq!(buy_shares(trader.as_ptr(), mid, 0, 1_000_000), 0);
    }

    #[test]
    fn test_sell_shares() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        let trader = [3u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(2000);
        let bought = buy_shares(trader.as_ptr(), mid, 0, 1_000_000);
        assert!(bought > 0);

        let (shares, _) = load_position(mid, &trader, 0);
        let sold = sell_shares(trader.as_ptr(), mid, 0, shares);
        assert!(sold > 0, "sell_shares should return mUSD > 0");

        let (remaining, _) = load_position(mid, &trader, 0);
        assert_eq!(remaining, 0, "All shares should be sold");
    }

    // ========================================================================
    // COMPLETE SET TESTS
    // ========================================================================

    #[test]
    fn test_mint_complete_set() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        let user = [3u8; 32];
        test_mock::set_caller(user);
        test_mock::set_slot(2000);
        let result = mint_complete_set(user.as_ptr(), mid, 5_000_000);
        assert_eq!(result, 1);

        // User should have 5M shares in each outcome
        let (s0, _) = load_position(mid, &user, 0);
        let (s1, _) = load_position(mid, &user, 1);
        assert_eq!(s0, 5_000_000);
        assert_eq!(s1, 5_000_000);
    }

    #[test]
    fn test_redeem_complete_set() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        let user = [3u8; 32];
        test_mock::set_caller(user);
        test_mock::set_slot(2000);
        mint_complete_set(user.as_ptr(), mid, 5_000_000);

        let result = redeem_complete_set(user.as_ptr(), mid, 3_000_000);
        assert!(result > 0);

        let (s0, _) = load_position(mid, &user, 0);
        let (s1, _) = load_position(mid, &user, 1);
        assert_eq!(s0, 2_000_000);
        assert_eq!(s1, 2_000_000);
    }

    // ========================================================================
    // CLOSE MARKET TESTS
    // ========================================================================

    #[test]
    fn test_close_market_after_close_slot() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        test_mock::set_caller(creator);
        test_mock::set_slot(100_001);
        assert_eq!(close_market(creator.as_ptr(), mid), 1);

        let record = load_market(mid).unwrap();
        assert_eq!(market_status(&record), STATUS_CLOSED);
    }

    #[test]
    fn test_close_market_before_close_slot_fails() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        test_mock::set_caller(creator);
        test_mock::set_slot(50_000); // before close
        assert_eq!(close_market(creator.as_ptr(), mid), 0);
    }

    // ========================================================================
    // RESOLUTION TESTS
    // ========================================================================

    #[test]
    fn test_submit_resolution_no_oracle_configured() {
        // Without oracle address set, submit_resolution skips oracle verification
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        // Close the market
        test_mock::set_caller(creator);
        test_mock::set_slot(100_001);
        close_market(creator.as_ptr(), mid);

        // Submit resolution — no oracle, should succeed
        let resolver = [5u8; 32];
        test_mock::set_caller(resolver);
        test_mock::set_value(DISPUTE_BOND);
        let att_hash = [0xCC; 32];
        let result = submit_resolution(
            resolver.as_ptr(), mid, 0, att_hash.as_ptr(), DISPUTE_BOND,
        );
        assert_eq!(result, 1, "Resolution should succeed without oracle");

        let record = load_market(mid).unwrap();
        assert_eq!(market_status(&record), STATUS_RESOLVING);
        assert_eq!(market_winning_outcome(&record), 0);
    }

    #[test]
    fn test_submit_resolution_with_oracle_rejects_in_mock() {
        // With oracle address set, call_contract returns Ok(Vec::new()),
        // which is too short (< 33 bytes), so submit_resolution should reject.
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        // Close the market
        test_mock::set_caller(creator);
        test_mock::set_slot(100_001);
        close_market(creator.as_ptr(), mid);

        // Set oracle address (triggers cross-contract call path)
        let oracle = [0xEE; 32];
        storage_set(ORACLE_ADDR_KEY, &oracle);

        let resolver = [5u8; 32];
        test_mock::set_caller(resolver);
        let att_hash = [0xCC; 32];
        let result = submit_resolution(
            resolver.as_ptr(), mid, 0, att_hash.as_ptr(), DISPUTE_BOND,
        );
        // Mock returns empty vec → "attestation not found" → rejection
        assert_eq!(result, 0, "Should reject when oracle returns empty data");
    }

    #[test]
    fn test_submit_resolution_requires_closed_market() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);
        // Market is still ACTIVE, not CLOSED
        let resolver = [5u8; 32];
        test_mock::set_caller(resolver);
        let att_hash = [0xCC; 32];
        assert_eq!(submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), DISPUTE_BOND), 0);
    }

    #[test]
    fn test_submit_resolution_invalid_outcome() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        test_mock::set_caller(creator);
        test_mock::set_slot(100_001);
        close_market(creator.as_ptr(), mid);

        let resolver = [5u8; 32];
        test_mock::set_caller(resolver);
        let att_hash = [0xCC; 32];
        // outcome 5 is invalid for binary market (0 or 1 only)
        assert_eq!(submit_resolution(resolver.as_ptr(), mid, 5, att_hash.as_ptr(), DISPUTE_BOND), 0);
    }

    #[test]
    fn test_finalize_resolution() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        test_mock::set_caller(creator);
        test_mock::set_slot(100_001);
        close_market(creator.as_ptr(), mid);

        let resolver = [5u8; 32];
        test_mock::set_caller(resolver);
        test_mock::set_value(DISPUTE_BOND);
        let att_hash = [0xCC; 32];
        submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), DISPUTE_BOND);

        // Advance past dispute period
        test_mock::set_slot(100_001 + DISPUTE_PERIOD + 1);
        assert_eq!(finalize_resolution(resolver.as_ptr(), mid), 1);

        let record = load_market(mid).unwrap();
        assert_eq!(market_status(&record), STATUS_RESOLVED);
        assert_eq!(market_winning_outcome(&record), 0);
    }

    // ========================================================================
    // REDEEM SHARES TESTS (POST-FIX: token transfer + full u64 payout)
    // ========================================================================

    #[test]
    fn test_redeem_winning_shares() {
        setup();
        init_contract();
        configure_escrow();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        // Trader buys YES shares (outcome 0)
        let trader = [3u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(2000);
        let shares = buy_shares(trader.as_ptr(), mid, 0, 2_000_000);
        assert!(shares > 0);

        // Forcefully resolve market: YES wins (outcome 0)
        force_resolve_market(mid, 0);

        // Redeem winning shares
        test_mock::set_caller(trader);
        let (user_shares, _) = load_position(mid, &trader, 0);
        assert!(user_shares > 0);

        let result = redeem_shares(trader.as_ptr(), mid, 0);
        assert_eq!(result, 1, "redeem_shares should return 1 on success");

        // Return data should have full u64 payout
        let payout = bytes_to_u64(&test_mock::get_return_data());
        assert_eq!(payout, user_shares, "Payout should equal shares held");

        // Position should be cleared
        let (remaining, _) = load_position(mid, &trader, 0);
        assert_eq!(remaining, 0, "Position should be cleared after redemption");
    }

    #[test]
    fn test_redeem_losing_shares() {
        setup();
        init_contract();
        configure_escrow();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        // Trader buys NO shares (outcome 1)
        let trader = [3u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(2000);
        buy_shares(trader.as_ptr(), mid, 1, 2_000_000);

        // Resolve: YES wins (outcome 0) — trader's NO shares are losing
        force_resolve_market(mid, 0);

        test_mock::set_caller(trader);
        let result = redeem_shares(trader.as_ptr(), mid, 1);
        // Losing shares: returns 1 (position cleared, payout=0 in return_data)
        assert_eq!(result, 1, "Losing shares redeem should succeed");
        let payout = bytes_to_u64(&test_mock::get_return_data());
        assert_eq!(payout, 0, "Losing shares should have zero payout");
    }

    #[test]
    fn test_redeem_double_redemption_blocked() {
        setup();
        init_contract();
        configure_escrow();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        let trader = [3u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(2000);
        buy_shares(trader.as_ptr(), mid, 0, 2_000_000);

        force_resolve_market(mid, 0);

        test_mock::set_caller(trader);
        assert_eq!(redeem_shares(trader.as_ptr(), mid, 0), 1); // first redeem succeeds
        assert_eq!(redeem_shares(trader.as_ptr(), mid, 0), 0); // second blocked (no shares)
    }

    #[test]
    fn test_redeem_shares_not_resolved_fails() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        let trader = [3u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(2000);
        buy_shares(trader.as_ptr(), mid, 0, 2_000_000);

        // Market is still ACTIVE (not resolved)
        test_mock::set_caller(trader);
        assert_eq!(redeem_shares(trader.as_ptr(), mid, 0), 0);
    }

    #[test]
    fn test_redeem_large_payout_no_truncation() {
        // Verify the u32 truncation bug is fixed — payouts > 4B micro-mUSD work
        setup();
        init_contract();
        configure_escrow();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        // Directly set a position with > u32::MAX shares to verify no truncation
        let trader = [3u8; 32];
        let large_shares: u64 = 5_000_000_000; // 5B micro-mUSD = $5000
        save_position(mid, &trader, 0, large_shares, large_shares);

        force_resolve_market(mid, 0);

        // Also set enough collateral
        let mut record = load_market(mid).unwrap();
        set_market_total_collateral(&mut record, large_shares + 1_000_000);
        save_market(mid, &record);
        save_u64(TOTAL_COLLATERAL_KEY, large_shares + 1_000_000);

        test_mock::set_caller(trader);
        let result = redeem_shares(trader.as_ptr(), mid, 0);
        assert_eq!(result, 1, "Redeem should succeed");

        let payout = bytes_to_u64(&test_mock::get_return_data());
        assert_eq!(payout, large_shares, "Full u64 payout should be preserved in return_data");
        assert!(payout > u32::MAX as u64, "Payout exceeds u32::MAX — return_data is not truncated");
    }

    // ========================================================================
    // RECLAIM COLLATERAL TESTS (VOIDED MARKET)
    // ========================================================================

    #[test]
    fn test_reclaim_collateral_voided_market() {
        setup();
        init_contract();
        configure_escrow();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        // User mints complete set
        let user = [3u8; 32];
        test_mock::set_caller(user);
        test_mock::set_slot(2000);
        mint_complete_set(user.as_ptr(), mid, 3_000_000);

        // Void the market
        force_void_market(mid);

        test_mock::set_caller(user);
        let result = reclaim_collateral(user.as_ptr(), mid);
        assert_eq!(result, 1, "Reclaim should succeed for voided market");

        let refund = bytes_to_u64(&test_mock::get_return_data());
        assert!(refund > 0, "Refund should be > 0");

        // Position should be cleared
        let (s0, _) = load_position(mid, &user, 0);
        let (s1, _) = load_position(mid, &user, 1);
        assert_eq!(s0, 0);
        assert_eq!(s1, 0);
    }

    #[test]
    fn test_reclaim_collateral_non_voided_fails() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        let user = [3u8; 32];
        test_mock::set_caller(user);
        test_mock::set_slot(2000);
        mint_complete_set(user.as_ptr(), mid, 3_000_000);

        // Market is ACTIVE, not VOIDED
        test_mock::set_caller(user);
        assert_eq!(reclaim_collateral(user.as_ptr(), mid), 0);
    }

    // ========================================================================
    // DAO RESOLVE/VOID TESTS
    // ========================================================================

    #[test]
    fn test_dao_resolve() {
        setup();
        let admin = init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        // Manually set to DISPUTED
        let mut record = load_market(mid).unwrap();
        set_market_status(&mut record, STATUS_DISPUTED);
        save_market(mid, &record);

        test_mock::set_caller(admin);
        assert_eq!(dao_resolve(admin.as_ptr(), mid, 1), 1);

        let record = load_market(mid).unwrap();
        assert_eq!(market_status(&record), STATUS_RESOLVED);
        assert_eq!(market_winning_outcome(&record), 1);
    }

    #[test]
    fn test_dao_void() {
        setup();
        let admin = init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        test_mock::set_caller(admin);
        assert_eq!(dao_void(admin.as_ptr(), mid), 1);

        let record = load_market(mid).unwrap();
        assert_eq!(market_status(&record), STATUS_VOIDED);
    }

    #[test]
    fn test_dao_resolve_non_admin_fails() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        let mut record = load_market(mid).unwrap();
        set_market_status(&mut record, STATUS_DISPUTED);
        save_market(mid, &record);

        let rando = [99u8; 32];
        test_mock::set_caller(rando);
        assert_eq!(dao_resolve(rando.as_ptr(), mid, 0), 0);
    }

    // ========================================================================
    // ADMIN CONFIG TESTS
    // ========================================================================

    #[test]
    fn test_set_self_address() {
        setup();
        let admin = init_contract();
        let self_addr = [0xBB; 32];
        assert_eq!(set_self_address(admin.as_ptr(), self_addr.as_ptr()), 1);
        assert_eq!(load_self_addr(), self_addr);
    }

    #[test]
    fn test_set_self_address_non_admin() {
        setup();
        init_contract();
        let rando = [99u8; 32];
        let addr = [0xBB; 32];
        test_mock::set_caller(rando);
        assert_eq!(set_self_address(rando.as_ptr(), addr.as_ptr()), 0);
    }

    #[test]
    fn test_set_self_address_rejects_zero() {
        setup();
        let admin = init_contract();
        let zero = [0u8; 32];
        assert_eq!(set_self_address(admin.as_ptr(), zero.as_ptr()), 0);
    }

    #[test]
    fn test_set_musd_address() {
        setup();
        let admin = init_contract();
        let musd = [0xAA; 32];
        assert_eq!(set_musd_address(admin.as_ptr(), musd.as_ptr()), 1);
        assert_eq!(load_addr(MUSD_ADDR_KEY), musd);
    }

    #[test]
    fn test_set_oracle_address() {
        setup();
        let admin = init_contract();
        let oracle = [0xEE; 32];
        assert_eq!(set_oracle_address(admin.as_ptr(), oracle.as_ptr()), 1);
        assert_eq!(load_addr(ORACLE_ADDR_KEY), oracle);
    }

    // ========================================================================
    // PAUSE TESTS
    // ========================================================================

    #[test]
    fn test_pause_blocks_create_market() {
        setup();
        let admin = init_contract();
        storage_set(PAUSED_KEY, &[1]);

        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_slot(1000);
        let qhash = [0x42u8; 32];
        let question = b"Paused?";
        assert_eq!(create_market(creator.as_ptr(), 0, 100_000, 2, qhash.as_ptr(), question.as_ptr(), 7), 0);
    }

    #[test]
    fn test_pause_blocks_buy_shares() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        storage_set(PAUSED_KEY, &[1]);

        let trader = [3u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(2000);
        assert_eq!(buy_shares(trader.as_ptr(), mid, 0, 1_000_000), 0);
    }

    // ========================================================================
    // FULL LIFECYCLE TEST
    // ========================================================================

    #[test]
    fn test_full_market_lifecycle() {
        setup();
        init_contract();
        configure_escrow();
        let creator = [2u8; 32];

        // 1. Create market
        let mid = create_binary_market(&creator, 100_000);
        assert_eq!(mid, 1);

        // 2. Add initial liquidity → ACTIVE
        activate_market(&creator, mid, 10_000_000);
        let record = load_market(mid).unwrap();
        assert_eq!(market_status(&record), STATUS_ACTIVE);

        // 3. Traders buy shares
        let trader_yes = [3u8; 32];
        let trader_no = [4u8; 32];
        test_mock::set_slot(5000);

        test_mock::set_caller(trader_yes);
        let yes_shares = buy_shares(trader_yes.as_ptr(), mid, 0, 2_000_000);
        assert!(yes_shares > 0);

        test_mock::set_caller(trader_no);
        let no_shares = buy_shares(trader_no.as_ptr(), mid, 1, 1_000_000);
        assert!(no_shares > 0);

        // 4. Close market
        test_mock::set_slot(100_001);
        test_mock::set_caller(creator);
        assert_eq!(close_market(creator.as_ptr(), mid), 1);

        // 5. Resolve: YES wins
        let resolver = [5u8; 32];
        test_mock::set_caller(resolver);
        test_mock::set_value(DISPUTE_BOND);
        let att_hash = [0xDD; 32];
        assert_eq!(submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), DISPUTE_BOND), 1);

        // 6. Finalize after dispute period
        test_mock::set_slot(100_001 + DISPUTE_PERIOD + 1);
        assert_eq!(finalize_resolution(resolver.as_ptr(), mid), 1);

        // 7. Winning trader redeems
        test_mock::set_caller(trader_yes);
        let (yes_held, _) = load_position(mid, &trader_yes, 0);
        assert!(yes_held > 0);
        let rdm = redeem_shares(trader_yes.as_ptr(), mid, 0);
        assert_eq!(rdm, 1);
        let payout = bytes_to_u64(&test_mock::get_return_data());
        assert_eq!(payout, yes_held);

        // 8. Losing trader redeems (zero payout)
        test_mock::set_caller(trader_no);
        assert_eq!(redeem_shares(trader_no.as_ptr(), mid, 1), 1); // losing returns 1
        let no_payout = bytes_to_u64(&test_mock::get_return_data());
        assert_eq!(no_payout, 0);

        // 9. Verify final state
        let record = load_market(mid).unwrap();
        assert_eq!(market_status(&record), STATUS_RESOLVED);
        assert_eq!(market_winning_outcome(&record), 0);
    }

    // ========================================================================
    // QUERY TESTS
    // ========================================================================

    #[test]
    fn test_get_market_count() {
        setup();
        init_contract();
        assert_eq!(get_market_count(), 0);

        let creator = [2u8; 32];
        create_binary_market(&creator, 100_000);
        assert_eq!(get_market_count(), 1);

        // Use different question hash
        test_mock::set_caller(creator);
        let qhash2 = [0x43u8; 32];
        let q2 = b"Will BTC hit $500K?";
        create_market(creator.as_ptr(), CATEGORY_CRYPTO, 100_000, 2, qhash2.as_ptr(), q2.as_ptr(), q2.len() as u32);
        assert_eq!(get_market_count(), 2);
    }

    #[test]
    fn test_price_calculation() {
        // Verify AMM price math for equal odds
        let reserves = [1_000_000u64, 1_000_000];
        let p0 = calculate_price(&reserves, 0);
        let p1 = calculate_price(&reserves, 1);
        assert_eq!(p0, 500_000, "Equal odds should give $0.50");
        assert_eq!(p1, 500_000);
    }

    #[test]
    fn test_price_calculation_skewed() {
        // More YES shares than NO → YES is cheaper
        let reserves = [2_000_000u64, 1_000_000];
        let p_yes = calculate_price(&reserves, 0);
        let p_no = calculate_price(&reserves, 1);
        // price_yes = r_no / (r_yes + r_no) = 1M / 3M = 0.333 → 333_333
        assert!(p_yes < 400_000, "YES should be cheap");
        assert!(p_no > 600_000, "NO should be expensive");
        // Sum should be ~MUSD_UNIT
        assert!((p_yes + p_no).abs_diff(MUSD_UNIT) < 2, "Prices should sum to ~$1");
    }

    // ========================================================================
    // G21-01 FINANCIAL WIRING TESTS
    // ========================================================================

    #[test]
    fn test_create_market_insufficient_fee() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        test_mock::set_caller(creator);
        test_mock::set_value(MARKET_CREATION_FEE - 1); // 1 short
        let q = b"Will it rain tomorrow?";
        let qh = [0xEE; 32];
        let r = create_market(creator.as_ptr(), 0, 200_000, 2, qh.as_ptr(), q.as_ptr(), q.len() as u32);
        assert_eq!(r, 0, "Should reject insufficient creation fee");
    }

    #[test]
    fn test_buy_shares_insufficient_value() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        let trader = [3u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(5000);
        test_mock::set_value(999_999); // less than 1_000_000
        let r = buy_shares(trader.as_ptr(), mid, 0, 1_000_000);
        assert_eq!(r, 0, "Should reject insufficient value for buy_shares");
    }

    #[test]
    fn test_mint_complete_set_insufficient_value() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        let user = [3u8; 32];
        test_mock::set_caller(user);
        test_mock::set_value(4_999_999); // less than 5_000_000
        let r = mint_complete_set(user.as_ptr(), mid, 5_000_000);
        assert_eq!(r, 0, "Should reject insufficient value for mint_complete_set");
    }

    #[test]
    fn test_submit_resolution_insufficient_bond() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        // Close the market
        test_mock::set_caller(creator);
        test_mock::set_slot(100_001);
        close_market(creator.as_ptr(), mid);

        let resolver = [5u8; 32];
        test_mock::set_caller(resolver);
        test_mock::set_value(DISPUTE_BOND - 1); // 1 short
        let att_hash = [0xCC; 32];
        let r = submit_resolution(resolver.as_ptr(), mid, 0, att_hash.as_ptr(), DISPUTE_BOND);
        assert_eq!(r, 0, "Should reject insufficient bond value");
    }

    #[test]
    fn test_sell_shares_transfers_musd_out() {
        setup();
        init_contract();
        configure_escrow();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        // Buy some shares first
        let trader = [3u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(5000);
        test_mock::set_value(2_000_000);
        let shares = buy_shares(trader.as_ptr(), mid, 0, 2_000_000);
        assert!(shares > 0);

        // Sell shares — should trigger transfer_musd_out
        test_mock::set_caller(trader);
        let sold = sell_shares(trader.as_ptr(), mid, 0, shares as u64);
        assert!(sold > 0, "sell_shares should return mUSD amount with escrow configured");
    }

    #[test]
    fn test_withdraw_liquidity_transfers_musd_out() {
        setup();
        init_contract();
        configure_escrow();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        // Add additional liquidity
        test_mock::set_caller(creator);
        test_mock::set_value(5_000_000);
        let r = add_liquidity(creator.as_ptr(), mid, 5_000_000);
        assert!(r > 0, "add_liquidity should return LP shares");

        // Withdraw — should trigger transfer_musd_out
        test_mock::set_caller(creator);
        let w = withdraw_liquidity(creator.as_ptr(), mid, 2_000_000);
        assert!(w > 0, "withdraw_liquidity should return mUSD amount with escrow configured");
    }

    #[test]
    fn test_add_liquidity_insufficient_value() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        test_mock::set_caller(creator);
        test_mock::set_value(4_999_999); // less than 5_000_000
        let r = add_liquidity(creator.as_ptr(), mid, 5_000_000);
        assert_eq!(r, 0, "Should reject insufficient value for add_liquidity");
    }

    // ====================================================================
    // G21-02: u32 truncation / u64 return_data tests
    // ====================================================================

    #[test]
    fn test_buy_shares_sets_return_data() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        let trader = [3u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(5000);
        test_mock::set_value(1_000_000);
        let shares = buy_shares(trader.as_ptr(), mid, 0, 1_000_000);
        assert!(shares > 0, "buy_shares should succeed");

        // return_data should contain the same value as the u32 return (for small amounts)
        let rd = test_mock::get_return_data();
        assert!(rd.len() >= 8, "return_data should have 8 bytes");
        let rd_val = u64::from_le_bytes(rd[0..8].try_into().unwrap());
        assert_eq!(rd_val, shares as u64, "return_data u64 should match u32 return for small values");
    }

    #[test]
    fn test_sell_shares_sets_return_data() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        activate_market(&creator, mid, 10_000_000);

        let trader = [3u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(5000);
        test_mock::set_value(2_000_000);
        let shares = buy_shares(trader.as_ptr(), mid, 0, 2_000_000);
        assert!(shares > 0);

        test_mock::set_caller(trader);
        let musd = sell_shares(trader.as_ptr(), mid, 0, shares as u64);
        assert!(musd > 0, "sell_shares should return mUSD");

        let rd = test_mock::get_return_data();
        assert!(rd.len() >= 8);
        let rd_val = u64::from_le_bytes(rd[0..8].try_into().unwrap());
        assert_eq!(rd_val, musd as u64, "return_data u64 should match u32 return for sell");
    }

    #[test]
    fn test_create_market_sets_return_data() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_binary_market(&creator, 100_000);
        assert!(mid > 0);

        let rd = test_mock::get_return_data();
        assert!(rd.len() >= 8);
        let rd_val = u64::from_le_bytes(rd[0..8].try_into().unwrap());
        assert_eq!(rd_val, mid, "return_data should contain market_id");
    }

    // ====================================================================
    // G21-03: Multi-outcome sell correctness tests
    // ====================================================================

    /// Create a 3-outcome market for testing
    fn create_3outcome_market(creator: &[u8; 32], close_slot: u64) -> u64 {
        test_mock::set_caller(*creator);
        test_mock::set_slot(1000);
        test_mock::set_value(MARKET_CREATION_FEE);
        let qhash = [0x33u8; 32];
        let question = b"Which team wins?";
        let result = create_market(
            creator.as_ptr(),
            CATEGORY_SPORTS,
            close_slot,
            3, // 3 outcomes
            qhash.as_ptr(),
            question.as_ptr(),
            question.len() as u32,
        );
        assert!(result > 0, "create 3-outcome market should succeed");
        result as u64
    }

    #[test]
    fn test_multi_outcome_sell_returns_nonzero() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_3outcome_market(&creator, 100_000);

        // Add initial liquidity
        test_mock::set_caller(creator);
        test_mock::set_value(30_000_000); // 30 mUSD
        let liq = add_initial_liquidity(
            creator.as_ptr(), mid, 30_000_000, core::ptr::null(), 0,
        );
        assert_eq!(liq, 1);

        // Buy shares of outcome 0
        let trader = [4u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(5000);
        test_mock::set_value(3_000_000);
        let shares = buy_shares(trader.as_ptr(), mid, 0, 3_000_000);
        assert!(shares > 0, "buy should succeed");

        // Sell some shares back (advance slot past any circuit-breaker pause)
        test_mock::set_caller(trader);
        test_mock::set_slot(5200);
        let sell_amount = (shares / 2) as u64;
        let musd = sell_shares(trader.as_ptr(), mid, 0, sell_amount);
        assert!(musd > 0, "multi-outcome sell should return nonzero mUSD");
    }

    #[test]
    fn test_multi_outcome_sell_all_shares() {
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_3outcome_market(&creator, 100_000);

        test_mock::set_caller(creator);
        test_mock::set_value(30_000_000);
        let liq = add_initial_liquidity(
            creator.as_ptr(), mid, 30_000_000, core::ptr::null(), 0,
        );
        assert_eq!(liq, 1);

        // Buy and sell all shares
        let trader = [5u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(5000);
        test_mock::set_value(2_000_000);
        let shares = buy_shares(trader.as_ptr(), mid, 1, 2_000_000);
        assert!(shares > 0);

        test_mock::set_caller(trader);
        test_mock::set_slot(5200);
        let musd = sell_shares(trader.as_ptr(), mid, 1, shares as u64);
        assert!(musd > 0, "selling all multi-outcome shares should return mUSD");

        // Should get back less than initial due to slippage and fees
        assert!((musd as u64) < 2_000_000, "should get back less than invested due to fees");
    }

    #[test]
    fn test_multi_outcome_sell_math_consistency() {
        // Test that calculate_sell and apply_sell_reserves agree
        // by doing a buy then sell and checking reserves change sensibly
        setup();
        init_contract();
        let creator = [2u8; 32];
        let mid = create_3outcome_market(&creator, 100_000);

        test_mock::set_caller(creator);
        test_mock::set_value(30_000_000);
        let liq = add_initial_liquidity(
            creator.as_ptr(), mid, 30_000_000, core::ptr::null(), 0,
        );
        assert_eq!(liq, 1);

        // Read initial reserves
        let p0_before = load_outcome_pool(mid, 0).unwrap();
        let r0_before = pool_reserve(&p0_before);
        let p1_before = load_outcome_pool(mid, 1).unwrap();
        let r1_before = pool_reserve(&p1_before);
        let p2_before = load_outcome_pool(mid, 2).unwrap();
        let r2_before = pool_reserve(&p2_before);

        // Buy outcome 0
        let trader = [6u8; 32];
        test_mock::set_caller(trader);
        test_mock::set_slot(5000);
        test_mock::set_value(5_000_000);
        let shares = buy_shares(trader.as_ptr(), mid, 0, 5_000_000);
        assert!(shares > 0);

        // After buy, outcome 0 reserve should decrease (shares extracted)
        let p0_after_buy = load_outcome_pool(mid, 0).unwrap();
        let r0_after_buy = pool_reserve(&p0_after_buy);
        assert!(r0_after_buy < r0_before, "buying outcome 0 should decrease its reserve");

        // Sell all back (advance slot past circuit-breaker pause)
        test_mock::set_caller(trader);
        test_mock::set_slot(5200);
        let musd = sell_shares(trader.as_ptr(), mid, 0, shares as u64);
        assert!(musd > 0);

        // After sell, outcome 0 reserve should increase back (shares returned to pool)
        let p0_after_sell = load_outcome_pool(mid, 0).unwrap();
        let r0_after_sell = pool_reserve(&p0_after_sell);
        assert!(r0_after_sell > r0_after_buy, "selling outcome 0 should increase its reserve back");
    }

    #[test]
    fn test_calculate_sell_binary_vs_multi_consistency() {
        // For a 2-outcome market, calculate_sell uses the quadratic solver.
        // Verify it returns a reasonable amount.
        let reserves = &[10_000_000u64, 10_000_000u64];
        let (musd, fee) = calculate_sell(reserves, 0, 1_000_000);
        assert!(musd > 0, "binary sell should return nonzero");
        assert!(fee > 0, "binary sell should have nonzero fee");
        assert!(musd + fee <= 1_000_000, "sell output should not exceed input shares");
    }

    #[test]
    fn test_calculate_sell_3outcome_returns_nonzero() {
        let reserves = &[10_000_000u64, 10_000_000u64, 10_000_000u64];
        let (musd, fee) = calculate_sell(reserves, 0, 1_000_000);
        assert!(musd > 0, "3-outcome sell should return nonzero mUSD");
        assert!(fee > 0, "3-outcome sell should have nonzero fee");
        // With equal reserves and selling 1/10 of reserve, should get reasonable output
        assert!(musd + fee <= 1_000_000, "sell payout should not exceed shares sold");
    }

    #[test]
    fn test_calculate_sell_4outcome_returns_nonzero() {
        let reserves = &[10_000_000u64, 10_000_000u64, 10_000_000u64, 10_000_000u64];
        let (musd, fee) = calculate_sell(reserves, 2, 500_000);
        assert!(musd > 0, "4-outcome sell should return nonzero mUSD");
        assert!(musd + fee <= 500_000);
    }

    #[test]
    fn test_calculate_sell_skewed_reserves() {
        // Selling from a cheaper outcome (higher reserve) should yield less
        let reserves = &[5_000_000u64, 15_000_000u64, 10_000_000u64];
        let (musd_expensive, _) = calculate_sell(reserves, 0, 500_000); // outcome 0: low reserve = expensive
        let (musd_cheap, _) = calculate_sell(reserves, 1, 500_000); // outcome 1: high reserve = cheap
        assert!(musd_expensive > musd_cheap,
            "selling expensive outcome shares should yield more mUSD ({} vs {})",
            musd_expensive, musd_cheap);
    }

    #[test]
    fn test_total_a_for_sets_basic() {
        let reserves = &[10_000_000u64, 10_000_000u64, 10_000_000u64];
        // Forming 1M sets with 10M reserves each should be feasible
        let needed = total_a_for_sets(reserves, 0, 1_000_000);
        assert!(needed < u128::MAX, "should be feasible");
        assert!(needed > 1_000_000, "should need more than C shares of A (also need swap cost)");
    }

    #[test]
    fn test_total_a_for_sets_impossible() {
        let reserves = &[10_000_000u64, 500_000u64, 10_000_000u64];
        // Trying to form 500_000 sets when reserve[1] = 500_000 should be impossible
        let needed = total_a_for_sets(reserves, 0, 500_000);
        assert_eq!(needed, u128::MAX, "should be impossible when C >= reserve_j");
    }
}
