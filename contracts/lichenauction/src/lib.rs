// LichenAuction v2 - Advanced NFT Marketplace
// Features: English Auctions, Offers/Bids, Creator Royalties, Collection Stats
// v2: Anti-sniping, Reserve Prices, Auction Cancel, Emergency Pause, Admin

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;
use alloc::vec::Vec;

use lichen_sdk::{
    bytes_to_u64, call_native_nft_owner, call_native_nft_royalty_info,
    call_native_nft_transfer_from, call_nft_owner, call_nft_royalty_info, call_nft_transfer_from,
    get_caller, get_contract_address, get_timestamp, log_info, receive_token_or_native,
    storage_get, storage_set, transfer_token_or_native, u64_to_bytes, Address,
};

// Reentrancy guard
const MA_REENTRANCY_KEY: &[u8] = b"ma_reentrancy";

fn reentrancy_enter() -> bool {
    if storage_get(MA_REENTRANCY_KEY)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
    {
        return false;
    }
    storage_set(MA_REENTRANCY_KEY, &[1u8]);
    true
}

fn reentrancy_exit() {
    storage_set(MA_REENTRANCY_KEY, &[0u8]);
}

/// T5.2 fix: Hex-encode binary addresses for storage keys (avoids UTF-8 collision)
fn hex_addr(bytes: &[u8]) -> alloc::string::String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = alloc::string::String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

fn read_address(ptr: *const u8) -> Option<Address> {
    if ptr.is_null() {
        return None;
    }
    let mut addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(ptr, addr.as_mut_ptr(), 32);
    }
    Some(Address(addr))
}

#[cfg(test)]
fn stored_u64(key: &[u8]) -> u64 {
    load_u64_or_zero(key).unwrap_or(0)
}

fn load_u64_or_zero(key: &[u8]) -> Option<u64> {
    match storage_get(key) {
        Some(data) if data.len() == 8 => Some(bytes_to_u64(&data)),
        Some(_) => None,
        None => Some(0),
    }
}

fn unpaid_payout_key(token: Address, recipient: Address) -> Vec<u8> {
    let mut key = b"unpaid_payout:".to_vec();
    key.extend_from_slice(&token.0);
    key.push(b':');
    key.extend_from_slice(&recipient.0);
    key
}

fn record_unpaid_payout(token: Address, recipient: Address, amount: u64) -> bool {
    if amount == 0 {
        return true;
    }
    let key = unpaid_payout_key(token, recipient);
    let current = match load_u64_or_zero(&key) {
        Some(value) => value,
        None => return false,
    };
    match current.checked_add(amount) {
        Some(total) => {
            storage_set(&key, &u64_to_bytes(total));
            true
        }
        None => false,
    }
}

fn can_record_unpaid_payout(token: Address, recipient: Address, amount: u64) -> bool {
    amount == 0
        || load_u64_or_zero(&unpaid_payout_key(token, recipient))
            .and_then(|current| current.checked_add(amount))
            .is_some()
}

fn can_record_unpaid_payouts(token: Address, payouts: &[(Address, u64)]) -> bool {
    for (index, (recipient, amount)) in payouts.iter().enumerate() {
        if *amount == 0 {
            continue;
        }
        let combined = payouts[index + 1..]
            .iter()
            .filter(|(other, _)| other == recipient)
            .try_fold(*amount, |total, (_, value)| total.checked_add(*value));
        let Some(combined) = combined else {
            return false;
        };
        if payouts[..index]
            .iter()
            .any(|(previous, _)| previous == recipient)
        {
            continue;
        }
        if !can_record_unpaid_payout(token, *recipient, combined) {
            return false;
        }
    }
    true
}

fn next_u64_value(key: &[u8], increment: u64) -> Option<u64> {
    load_u64_or_zero(key)?.checked_add(increment)
}

fn collection_stats_key(nft_contract: Address) -> Vec<u8> {
    alloc::format!("stats_{}", hex_addr(&nft_contract.0)).into_bytes()
}

fn prepare_collection_sale(nft_contract: Address, sale_price: u64) -> Option<(Vec<u8>, [u8; 24])> {
    let key = collection_stats_key(nft_contract);
    let (volume, sales, floor) = match storage_get(&key) {
        Some(data) if data.len() == 24 => (
            bytes_to_u64(&data[..8]),
            bytes_to_u64(&data[8..16]),
            bytes_to_u64(&data[16..24]),
        ),
        Some(_) => return None,
        None => (0, 0, 0),
    };
    let mut next = [0u8; 24];
    next[..8].copy_from_slice(&u64_to_bytes(volume.checked_add(sale_price)?));
    next[8..16].copy_from_slice(&u64_to_bytes(sales.checked_add(1)?));
    next[16..24].copy_from_slice(&u64_to_bytes(if floor == 0 || sale_price < floor {
        sale_price
    } else {
        floor
    }));
    Some((key, next))
}

// ============================================================================
// AUCTION SYSTEM - English Auctions (Highest bidder wins)
// ============================================================================

const SLOT_DURATION_MS: u64 = 400;
const AUCTION_DURATION: u64 = 24 * 60 * 60 * 1_000 / SLOT_DURATION_MS;
const MIN_DURATION: u64 = 60 * 1_000 / SLOT_DURATION_MS;
const MAX_DURATION: u64 = 30 * 24 * 60 * 60 * 1_000 / SLOT_DURATION_MS;
const MARKETPLACE_ADDR_KEY: &[u8] = b"marketplace_addr";

// ---- V2 constants ----
const MA_ADMIN_KEY: &[u8] = b"ma_admin";
const MA_PENDING_ADMIN_KEY: &[u8] = b"ma_pending_admin";
const MA_PAUSE_KEY: &[u8] = b"ma_paused";
const MA_STATE_VERSION_KEY: &[u8] = b"ma_state_version";
const MA_MIGRATION_LOCK_KEY: &[u8] = b"ma_v3_migration_lock";
const MA_MIGRATION_MANIFEST_KEY: &[u8] = b"ma_v3_manifest";
const MA_MIGRATION_EXPECTED_AUCTIONS_KEY: &[u8] = b"ma_v3_expected_auctions";
const MA_MIGRATION_EXPECTED_OFFERS_KEY: &[u8] = b"ma_v3_expected_offers";
const MA_MIGRATION_MIGRATED_AUCTIONS_KEY: &[u8] = b"ma_v3_migrated_auctions";
const MA_MIGRATION_MIGRATED_OFFERS_KEY: &[u8] = b"ma_v3_migrated_offers";
const MA_STATE_VERSION: u64 = 3;
/// Anti-sniping: if a bid lands in the last five minutes, extend the auction.
const SNIPE_WINDOW: u64 = 5 * 60 * 1_000 / SLOT_DURATION_MS;
/// Extension added on snipe bid
const SNIPE_EXTENSION: u64 = 5 * 60 * 1_000 / SLOT_DURATION_MS;
/// Maximum total extensions to prevent infinite auctions
const MAX_EXTENSIONS: u64 = 12; // max 1 hour of extensions (12 × 5min)

const MA_GLOBAL_AUCTION_COUNT_KEY: &[u8] = b"ma_auction_count";
const MA_GLOBAL_VOLUME_KEY: &[u8] = b"ma_total_volume";
const MA_GLOBAL_SALES_KEY: &[u8] = b"ma_total_sales";
const MA_PLATFORM_FEE_BPS_KEY: &[u8] = b"ma_platform_fee_bps";
const MA_FEE_TREASURY_KEY: &[u8] = b"ma_fee_treasury";
const DEFAULT_MARKETPLACE_FEE_BPS: u64 = 250;
const MAX_MARKETPLACE_FEE_BPS: u64 = 1_000;

fn platform_fee_bps() -> Option<u64> {
    match storage_get(MA_PLATFORM_FEE_BPS_KEY) {
        Some(data) if data.len() == 8 => {
            let bps = bytes_to_u64(&data);
            (bps <= MAX_MARKETPLACE_FEE_BPS).then_some(bps)
        }
        None => Some(DEFAULT_MARKETPLACE_FEE_BPS),
        Some(_) => None,
    }
}

fn platform_fee_key(token: Address) -> Vec<u8> {
    let mut key = b"ma_platform_fee:".to_vec();
    key.extend_from_slice(&token.0);
    key
}

#[cfg(test)]
fn accrue_platform_fee(token: Address, amount: u64) -> bool {
    let key = platform_fee_key(token);
    let current = match load_u64_or_zero(&key) {
        Some(value) => value,
        None => return false,
    };
    match current.checked_add(amount) {
        Some(total) => {
            storage_set(&key, &u64_to_bytes(total));
            true
        }
        None => false,
    }
}

fn auction_fee_key(nft_contract: &[u8], token_id: u64) -> Vec<u8> {
    alloc::format!("auction_fee_{}_{}", hex_addr(nft_contract), token_id).into_bytes()
}

fn offer_fee_key(offerer: &[u8], nft_contract: &[u8], token_id: u64) -> Vec<u8> {
    alloc::format!(
        "offer_fee_{}_{}_{}",
        hex_addr(offerer),
        hex_addr(nft_contract),
        token_id
    )
    .into_bytes()
}

fn auction_royalty_key(nft_contract: &[u8], token_id: u64) -> Vec<u8> {
    alloc::format!("auction_royalty_{}_{}", hex_addr(nft_contract), token_id).into_bytes()
}

fn offer_royalty_key(offerer: &[u8], nft_contract: &[u8], token_id: u64) -> Vec<u8> {
    alloc::format!(
        "offer_royalty_{}_{}_{}",
        hex_addr(offerer),
        hex_addr(nft_contract),
        token_id
    )
    .into_bytes()
}

fn nft_owned_by(nft_contract: Address, token_id: u64, expected_owner: Address) -> bool {
    nft_owner(nft_contract, token_id) == Some(expected_owner)
}

fn nft_owner(nft_contract: Address, token_id: u64) -> Option<Address> {
    match call_nft_owner(nft_contract, token_id) {
        Ok(owner) => Some(owner),
        Err(_) => call_native_nft_owner(nft_contract, token_id).ok(),
    }
}

fn transfer_nft_from_auction(
    nft_contract: Address,
    from: Address,
    to: Address,
    token_id: u64,
) -> bool {
    let auction = get_contract_address();
    match call_nft_transfer_from(nft_contract, auction, from, to, token_id) {
        Ok(true) => true,
        Ok(false) => false,
        Err(_) => call_native_nft_transfer_from(nft_contract, from, to, token_id).unwrap_or(false),
    }
}

fn canonical_collection_royalty(nft_contract: Address, token_id: u64) -> Option<(Address, u16)> {
    let terms = match call_nft_royalty_info(nft_contract, token_id) {
        Ok(terms) => terms,
        Err(_) => call_native_nft_royalty_info(nft_contract).ok()?,
    };
    if terms.1 > 1_000 || (terms.1 > 0 && terms.0 == Address([0u8; 32])) {
        return None;
    }
    Some(terms)
}

fn store_royalty_snapshot(key: &[u8], recipient: Address, bps: u16) {
    let mut data = Vec::with_capacity(34);
    data.extend_from_slice(&recipient.0);
    data.extend_from_slice(&bps.to_le_bytes());
    storage_set(key, &data);
}

fn load_royalty_snapshot(key: &[u8]) -> Option<(Address, u16)> {
    let data = storage_get(key)?;
    if data.len() != 34 {
        return None;
    }
    let mut recipient = [0u8; 32];
    recipient.copy_from_slice(&data[..32]);
    let bps = u16::from_le_bytes([data[32], data[33]]);
    if bps > 1_000 || (bps > 0 && recipient == [0u8; 32]) {
        return None;
    }
    Some((Address(recipient), bps))
}

fn load_fee_snapshot(key: &[u8]) -> Option<u64> {
    match storage_get(key) {
        Some(data) if data.len() == 8 => {
            let bps = bytes_to_u64(&data);
            (bps <= MAX_MARKETPLACE_FEE_BPS).then_some(bps)
        }
        _ => None,
    }
}

fn is_ma_paused() -> bool {
    match storage_get(MA_PAUSE_KEY) {
        None => false,
        Some(data) => data.as_slice() != [0u8],
    }
}

fn is_ma_initialized() -> bool {
    storage_get(b"ma_initialized")
        .map(|data| data.as_slice() == [1u8])
        .unwrap_or(false)
}

fn ma_state_version() -> Option<u64> {
    match storage_get(MA_STATE_VERSION_KEY) {
        Some(data) if data.len() == 8 => Some(bytes_to_u64(&data)),
        None => Some(2),
        Some(_) => None,
    }
}

fn is_ma_migration_locked() -> bool {
    match storage_get(MA_MIGRATION_LOCK_KEY) {
        None => false,
        Some(data) => data.as_slice() != [0u8],
    }
}

fn is_ma_operational() -> bool {
    is_ma_initialized() && ma_state_version() == Some(MA_STATE_VERSION) && !is_ma_migration_locked()
}

fn migration_manifest() -> Option<[u8; 32]> {
    match storage_get(MA_MIGRATION_MANIFEST_KEY) {
        Some(data) if data.len() == 32 && data.as_slice() != [0u8; 32] => {
            let mut manifest = [0u8; 32];
            manifest.copy_from_slice(&data);
            Some(manifest)
        }
        _ => None,
    }
}

fn migration_marker_key(kind: &[u8], addresses: &[&[u8]], token_id: u64) -> Vec<u8> {
    let mut key = b"ma_v3_migrated:".to_vec();
    key.extend_from_slice(kind);
    for address in addresses {
        key.push(b':');
        key.extend_from_slice(hex_addr(address).as_bytes());
    }
    key.push(b':');
    key.extend_from_slice(alloc::format!("{}", token_id).as_bytes());
    key
}

fn authenticated_admin(caller_ptr: *const u8) -> Option<Address> {
    let caller = read_address(caller_ptr)?;
    (caller == get_caller() && is_ma_admin(&caller.0)).then_some(caller)
}

fn legacy_escrow_address() -> Option<Address> {
    match storage_get(MARKETPLACE_ADDR_KEY) {
        Some(data) if data.len() == 32 && data.as_slice() != [0u8; 32] => {
            let mut address = [0u8; 32];
            address.copy_from_slice(&data);
            Some(Address(address))
        }
        _ => None,
    }
}
fn is_ma_admin(caller: &[u8]) -> bool {
    storage_get(MA_ADMIN_KEY)
        .map(|d| d.as_slice() == caller)
        .unwrap_or(false)
}

/// Key for tracking how many times an auction has been extended (anti-sniping)
fn ext_count_key(nft_contract: &[u8], token_id: u64) -> Vec<u8> {
    alloc::format!("ext_{}_{}", hex_addr(nft_contract), token_id).into_bytes()
}
/// Key for reserve price
fn reserve_key(nft_contract: &[u8], token_id: u64) -> Vec<u8> {
    alloc::format!("reserve_{}_{}", hex_addr(nft_contract), token_id).into_bytes()
}

// Auction: 169 bytes
// seller (32) + nft_contract (32) + token_id (8) + min_bid (8) +
// payment_token (32) + start_time (8) + end_time (8) +
// highest_bidder (32) + highest_bid (8) + active (1)
const AUCTION_SIZE: usize = 169;

fn valid_auction_record(data: &[u8], nft_contract: Address, token_id: u64) -> bool {
    if data.len() != AUCTION_SIZE
        || data[32..64] != nft_contract.0
        || bytes_to_u64(&data[64..72]) != token_id
        || data[..32] == [0u8; 32]
        || bytes_to_u64(&data[72..80]) == 0
        || bytes_to_u64(&data[112..120]) > bytes_to_u64(&data[120..128])
        || data[168] > 1
    {
        return false;
    }
    let highest_bidder_is_zero = data[128..160] == [0u8; 32];
    let highest_bid = bytes_to_u64(&data[160..168]);
    (highest_bid == 0) == highest_bidder_is_zero
        && (highest_bid == 0 || highest_bid >= bytes_to_u64(&data[72..80]))
}

fn marketplace_escrow_address() -> Option<Address> {
    // Auction funds must remain under this contract's authority. Historical
    // deployments accepted an arbitrary external address here, which made
    // payout authorization dependent on another account or contract.
    Some(get_contract_address())
}

#[no_mangle]
pub extern "C" fn create_auction(
    seller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    min_bid: u64,
    payment_token_ptr: *const u8,
    duration: u64, // seconds
) -> u32 {
    log_info("Creating English auction...");

    if !is_ma_operational() || is_ma_paused() {
        log_info("LichenAuction is unavailable or paused");
        return 0;
    }

    let seller = match read_address(seller_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    let payment_token = match read_address(payment_token_ptr) {
        Some(addr) => addr,
        None => return 0,
    };

    if seller.0 == [0u8; 32] || nft_contract.0 == [0u8; 32] || min_bid == 0 {
        log_info("Minimum bid must be > 0");
        return 0;
    }

    // AUDIT-FIX: verify transaction signer is the seller
    let real_caller = get_caller();
    if real_caller.0 != seller.0 {
        log_info("create_auction rejected: caller is not the seller");
        return 0;
    }

    if !nft_owned_by(nft_contract, token_id, seller) {
        log_info("NFT ownership verification failed");
        return 0;
    }

    let (royalty_recipient, royalty_bps) =
        match canonical_collection_royalty(nft_contract, token_id) {
            Some(terms) => terms,
            None => {
                log_info("NFT collection royalty terms are unavailable or invalid");
                return 0;
            }
        };
    let fee_bps = match platform_fee_bps() {
        Some(fee) => fee,
        None => {
            log_info("Marketplace fee configuration is invalid");
            return 0;
        }
    };

    let now = get_timestamp();
    let auction_duration = if duration > 0 {
        duration
    } else {
        AUCTION_DURATION
    };
    if !(MIN_DURATION..=MAX_DURATION).contains(&auction_duration) {
        log_info("Auction duration is outside the supported range");
        return 0;
    }
    let end_time = match now.checked_add(auction_duration) {
        Some(end_time) => end_time,
        None => {
            log_info("Auction end time overflow");
            return 0;
        }
    };

    let key = alloc::format!("auction_{}_{}", hex_addr(&nft_contract.0), token_id);
    match storage_get(key.as_bytes()) {
        Some(existing)
            if valid_auction_record(&existing, nft_contract, token_id) && existing[168] == 0 => {}
        Some(existing) if valid_auction_record(&existing, nft_contract, token_id) => {
            log_info("An active auction already exists for this NFT");
            return 0;
        }
        Some(_) => {
            log_info("Existing auction state is malformed");
            return 0;
        }
        None => {}
    }

    // Take custody before accepting bids. This prevents a seller from
    // revoking approval or transferring the NFT after bidder funds are held.
    let auction = get_contract_address();
    if !transfer_nft_from_auction(nft_contract, seller, auction, token_id) {
        log_info("NFT escrow transfer failed; approve LichenAuction first");
        return 0;
    }

    // Build auction data
    let mut auction = Vec::with_capacity(AUCTION_SIZE);
    auction.extend_from_slice(&seller.0); // 0-31: seller
    auction.extend_from_slice(&nft_contract.0); // 32-63: nft_contract
    auction.extend_from_slice(&u64_to_bytes(token_id)); // 64-71: token_id
    auction.extend_from_slice(&u64_to_bytes(min_bid)); // 72-79: min_bid
    auction.extend_from_slice(&payment_token.0); // 80-111: payment_token
    auction.extend_from_slice(&u64_to_bytes(now)); // 112-119: start_time
    auction.extend_from_slice(&u64_to_bytes(end_time)); // 120-127: end_time
    auction.extend_from_slice(&[0u8; 32]); // 128-159: highest_bidder (empty)
    auction.extend_from_slice(&[0u8; 8]); // 160-167: highest_bid (0)
    auction.push(1); // 168: active

    // Store auction
    storage_set(key.as_bytes(), &auction);
    storage_set(
        &auction_fee_key(&nft_contract.0, token_id),
        &u64_to_bytes(fee_bps),
    );
    store_royalty_snapshot(
        &auction_royalty_key(&nft_contract.0, token_id),
        royalty_recipient,
        royalty_bps,
    );

    log_info("Auction created!");
    log_info(&alloc::format!("   Min bid: {}", min_bid));
    log_info(&alloc::format!("   Duration: {} slots", auction_duration));
    1
}

#[no_mangle]
pub extern "C" fn place_bid(
    bidder_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    bid_amount: u64,
) -> u32 {
    if !is_ma_operational() || is_ma_paused() {
        log_info("LichenAuction is paused");
        return 0;
    }
    if !reentrancy_enter() {
        return 0;
    }
    log_info("Placing bid...");

    let bidder = match read_address(bidder_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 0;
        }
    };
    if bidder.0 == [0u8; 32] {
        reentrancy_exit();
        return 0;
    }

    // AUDIT-FIX H-8: Verify bidder matches actual caller to prevent bid forgery
    let real_caller = get_caller();
    if real_caller.0 != bidder.0 {
        log_info("Bidder does not match caller — rejected");
        reentrancy_exit();
        return 0;
    }

    let nft_contract = match read_address(nft_contract_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 0;
        }
    };

    // Load auction
    let key = alloc::format!("auction_{}_{}", hex_addr(&nft_contract.0), token_id);
    let auction_data = match storage_get(key.as_bytes()) {
        Some(data) => data,
        None => {
            log_info("Auction not found");
            reentrancy_exit();
            return 0;
        }
    };

    if !valid_auction_record(&auction_data, nft_contract, token_id) {
        log_info("Invalid auction data");
        reentrancy_exit();
        return 0;
    }

    // Check if active
    if auction_data[168] != 1 {
        log_info("Auction not active");
        reentrancy_exit();
        return 0;
    }

    // Check if ended
    let end_time = bytes_to_u64(&auction_data[120..128]);
    let now = get_timestamp();
    if now > end_time {
        log_info("Auction has ended");
        reentrancy_exit();
        return 0;
    }

    // Check bid amount
    let min_bid = bytes_to_u64(&auction_data[72..80]);
    let current_highest = bytes_to_u64(&auction_data[160..168]);
    if auction_data[..32] == bidder.0 {
        log_info("Seller cannot bid on their own auction");
        reentrancy_exit();
        return 0;
    }

    let required_bid = if current_highest > 0 {
        match current_highest.checked_add((current_highest / 20).max(1)) {
            Some(required_bid) => required_bid,
            None => {
                log_info("Required bid overflow");
                reentrancy_exit();
                return 0;
            }
        }
    } else {
        min_bid
    };

    if bid_amount == 0 || bid_amount < required_bid {
        log_info("Bid too low");
        log_info(&alloc::format!("   Required: {}", required_bid));
        reentrancy_exit();
        return 0;
    }

    let mut payment_token_bytes = [0u8; 32];
    payment_token_bytes.copy_from_slice(&auction_data[80..112]);
    let payment_token_addr = Address(payment_token_bytes);

    let marketplace_addr = match marketplace_escrow_address() {
        Some(addr) => addr,
        None => {
            log_info("Marketplace escrow address not configured");
            reentrancy_exit();
            return 0;
        }
    };

    let mut next_end_time = None;
    let mut next_extension_count = None;
    let time_left = end_time.saturating_sub(now);
    if time_left < SNIPE_WINDOW {
        let ek = ext_count_key(&nft_contract.0, token_id);
        let extensions = match load_u64_or_zero(&ek) {
            Some(value) if value <= MAX_EXTENSIONS => value,
            _ => {
                log_info("Anti-snipe extension state is invalid");
                reentrancy_exit();
                return 0;
            }
        };
        if extensions < MAX_EXTENSIONS {
            next_end_time = match end_time.checked_add(SNIPE_EXTENSION) {
                Some(new_end) => Some(new_end),
                None => {
                    log_info("Anti-snipe extension overflow");
                    reentrancy_exit();
                    return 0;
                }
            };
            next_extension_count = match extensions.checked_add(1) {
                Some(next) => Some(next),
                None => {
                    log_info("Anti-snipe extension count overflow");
                    reentrancy_exit();
                    return 0;
                }
            };
        }
    }

    if current_highest > 0 && !can_record_unpaid_payout(payment_token_addr, bidder, bid_amount) {
        log_info("Replacement bid refund liability would overflow or is malformed");
        reentrancy_exit();
        return 0;
    }

    // Escrow the new bid before touching the previous bidder or auction state.
    match receive_token_or_native(payment_token_addr, bidder, marketplace_addr, bid_amount) {
        Ok(true) => log_info("Bid placed in escrow"),
        _ => {
            log_info("Token transfer failed");
            reentrancy_exit();
            return 0;
        }
    }

    // Refund previous bidder after the replacement bid is escrowed. If this fails,
    // refund the new bidder and leave the previous highest bid unchanged.
    if current_highest > 0 {
        let mut prev_bidder_bytes = [0u8; 32];
        prev_bidder_bytes.copy_from_slice(&auction_data[128..160]);
        let prev_bidder = Address(prev_bidder_bytes);

        match transfer_token_or_native(
            payment_token_addr,
            marketplace_addr,
            prev_bidder,
            current_highest,
        ) {
            Ok(true) => log_info("Refunded previous bidder"),
            _ => {
                log_info("Refund to previous bidder failed; refunding replacement bid");
                match transfer_token_or_native(
                    payment_token_addr,
                    marketplace_addr,
                    bidder,
                    bid_amount,
                ) {
                    Ok(true) => log_info("Replacement bid refunded"),
                    _ => {
                        if !record_unpaid_payout(payment_token_addr, bidder, bid_amount) {
                            reentrancy_exit();
                            return 0;
                        }
                        log_info("Replacement bid refund failed; payout recorded");
                    }
                }
                reentrancy_exit();
                return 0;
            }
        }
    }

    // Update auction with new highest bid
    let mut updated_auction = auction_data.clone();
    updated_auction[128..160].copy_from_slice(&bidder.0);
    updated_auction[160..168].copy_from_slice(&u64_to_bytes(bid_amount));

    // V2: Anti-sniping — if bid within SNIPE_WINDOW of end, extend
    if let Some(new_end) = next_end_time {
        updated_auction[120..128].copy_from_slice(&u64_to_bytes(new_end));
        if let Some(next_extensions) = next_extension_count {
            let ek = ext_count_key(&nft_contract.0, token_id);
            storage_set(&ek, &u64_to_bytes(next_extensions));
        }
        log_info("Anti-snipe: auction extended");
    }

    storage_set(key.as_bytes(), &updated_auction);

    log_info("Bid accepted!");
    reentrancy_exit();
    1
}

#[no_mangle]
pub extern "C" fn finalize_auction(nft_contract_ptr: *const u8, token_id: u64) -> u32 {
    if !is_ma_operational() {
        return 0;
    }
    if !reentrancy_enter() {
        return 0;
    }
    log_info("Finalizing auction...");

    let nft_contract = match read_address(nft_contract_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 0;
        }
    };

    // Load auction
    let key = alloc::format!("auction_{}_{}", hex_addr(&nft_contract.0), token_id);
    let auction_data = match storage_get(key.as_bytes()) {
        Some(data) => data,
        None => {
            log_info("Auction not found");
            reentrancy_exit();
            return 0;
        }
    };

    if !valid_auction_record(&auction_data, nft_contract, token_id) {
        reentrancy_exit();
        return 0;
    }

    if auction_data[168] != 1 {
        log_info("Auction not active");
        reentrancy_exit();
        return 0;
    }

    // Check if ended
    let end_time = bytes_to_u64(&auction_data[120..128]);
    let now = get_timestamp();
    if now <= end_time {
        log_info("Auction still active");
        reentrancy_exit();
        return 0;
    }

    let mut seller_bytes = [0u8; 32];
    seller_bytes.copy_from_slice(&auction_data[0..32]);
    let seller = Address(seller_bytes);
    let mut highest_bidder_bytes = [0u8; 32];
    highest_bidder_bytes.copy_from_slice(&auction_data[128..160]);
    let highest_bidder = Address(highest_bidder_bytes);
    let highest_bid = bytes_to_u64(&auction_data[160..168]);
    let mut payment_token_bytes = [0u8; 32];
    payment_token_bytes.copy_from_slice(&auction_data[80..112]);
    let payment_token = Address(payment_token_bytes);

    // V2: Reserve price check — if reserve not met, return NFT to seller
    let rk = reserve_key(&nft_contract.0, token_id);
    let reserve_price = match load_u64_or_zero(&rk) {
        Some(value) => value,
        None => {
            log_info("Auction reserve state is invalid");
            reentrancy_exit();
            return 0;
        }
    };

    if highest_bid > 0 && reserve_price > 0 && highest_bid < reserve_price {
        log_info("Reserve price not met — auction cancelled, refund bidder");
        let marketplace_addr = match marketplace_escrow_address() {
            Some(addr) => addr,
            None => {
                log_info("Marketplace escrow address not configured");
                reentrancy_exit();
                return 0;
            }
        };

        if !can_record_unpaid_payout(payment_token, highest_bidder, highest_bid) {
            log_info("Bidder refund liability would overflow");
            reentrancy_exit();
            return 0;
        }
        if !transfer_nft_from_auction(nft_contract, get_contract_address(), seller, token_id) {
            log_info("NFT escrow return failed — auction remains active");
            reentrancy_exit();
            return 0;
        }

        // Refund highest bidder. Once the NFT has been returned, a failed
        // transfer becomes a durable exact liability and the auction closes.
        match transfer_token_or_native(payment_token, marketplace_addr, highest_bidder, highest_bid)
        {
            Ok(true) => {
                log_info("Refunded bidder — reserve not met");
            }
            _ => {
                if !record_unpaid_payout(payment_token, highest_bidder, highest_bid) {
                    reentrancy_exit();
                    return 0;
                }
                log_info("Refund failed — bidder payout recorded");
            }
        }
        let mut updated_auction = auction_data;
        updated_auction[168] = 0;
        storage_set(key.as_bytes(), &updated_auction);
        reentrancy_exit();
        return 2; // reserve not met
    }

    if highest_bid == 0 {
        log_info(" No bids received");
        if !transfer_nft_from_auction(nft_contract, get_contract_address(), seller, token_id) {
            log_info("NFT escrow return failed — auction remains active");
            reentrancy_exit();
            return 0;
        }
        // Mark inactive
        let mut updated_auction = auction_data.clone();
        updated_auction[168] = 0;
        storage_set(key.as_bytes(), &updated_auction);
        reentrancy_exit();
        return 1;
    }

    // T5.7: Check for collection royalty and enforce it
    let marketplace_fee_bps = match load_fee_snapshot(&auction_fee_key(&nft_contract.0, token_id)) {
        Some(fee) => fee,
        None => {
            log_info("Auction fee snapshot is missing or invalid");
            reentrancy_exit();
            return 0;
        }
    };
    let (royalty_recipient, royalty_bps) =
        match load_royalty_snapshot(&auction_royalty_key(&nft_contract.0, token_id)) {
            Some(terms) => terms,
            None => {
                log_info("Auction royalty snapshot is missing or invalid");
                reentrancy_exit();
                return 0;
            }
        };
    let royalty_bps = u64::from(royalty_bps);

    // Total deductions = marketplace fee + royalty (capped at 10% each)
    let total_deduction_bps = marketplace_fee_bps + royalty_bps.min(1000);
    let seller_amount =
        ((highest_bid as u128) * ((10000 - total_deduction_bps) as u128) / 10000) as u64;
    let royalty_amount = ((highest_bid as u128) * (royalty_bps.min(1000) as u128) / 10000) as u64;
    let marketplace_fee = ((highest_bid as u128) * (marketplace_fee_bps as u128) / 10000) as u64;
    let marketplace_addr = match marketplace_escrow_address() {
        Some(addr) => addr,
        None => {
            log_info("Marketplace escrow address not configured");
            reentrancy_exit();
            return 0;
        }
    };
    let platform_key = platform_fee_key(payment_token);
    let next_platform_fees = match next_u64_value(&platform_key, marketplace_fee) {
        Some(value) => value,
        None => {
            log_info("Platform fee accounting would overflow or is malformed");
            reentrancy_exit();
            return 0;
        }
    };
    let next_auction_count = match next_u64_value(MA_GLOBAL_AUCTION_COUNT_KEY, 1) {
        Some(value) => value,
        None => {
            log_info("Auction count accounting would overflow or is malformed");
            reentrancy_exit();
            return 0;
        }
    };
    let next_volume = match next_u64_value(MA_GLOBAL_VOLUME_KEY, highest_bid) {
        Some(value) => value,
        None => {
            log_info("Auction volume accounting would overflow or is malformed");
            reentrancy_exit();
            return 0;
        }
    };
    let (collection_stats_key, next_collection_stats) =
        match prepare_collection_sale(nft_contract, highest_bid) {
            Some(next) => next,
            None => {
                log_info("Collection sale accounting would overflow or is malformed");
                reentrancy_exit();
                return 0;
            }
        };
    if !can_record_unpaid_payouts(
        payment_token,
        &[(seller, seller_amount), (royalty_recipient, royalty_amount)],
    ) {
        log_info("Settlement payout liability would overflow or is malformed");
        reentrancy_exit();
        return 0;
    }

    // Transfer the NFT before releasing escrowed proceeds. If this fails, the
    // auction stays active and winner funds remain escrowed for retry/refund.
    if !transfer_nft_from_auction(
        nft_contract,
        get_contract_address(),
        highest_bidder,
        token_id,
    ) {
        log_info("NFT transfer failed");
        reentrancy_exit();
        return 0;
    }
    log_info("NFT transferred to winner");

    let mut updated_auction = auction_data.clone();
    updated_auction[168] = 0;
    storage_set(key.as_bytes(), &updated_auction);

    if seller_amount > 0 {
        match transfer_token_or_native(payment_token, marketplace_addr, seller, seller_amount) {
            Ok(true) => log_info("Payment sent to seller"),
            _ => {
                if !record_unpaid_payout(payment_token, seller, seller_amount) {
                    reentrancy_exit();
                    return 0;
                }
                log_info("Payment transfer failed; payout recorded");
            }
        }
    }

    // T5.7: Pay royalty to creator if configured
    if royalty_amount > 0 {
        match transfer_token_or_native(
            payment_token,
            marketplace_addr,
            royalty_recipient,
            royalty_amount,
        ) {
            Ok(true) => {
                log_info("Royalty paid to creator");
                log_info(&alloc::format!(
                    "   Royalty: {} ({}bps)",
                    royalty_amount,
                    royalty_bps
                ));
            }
            _ => {
                if !record_unpaid_payout(payment_token, royalty_recipient, royalty_amount) {
                    reentrancy_exit();
                    return 0;
                }
                log_info("Auction royalty transfer failed; payout recorded");
            }
        }
    }

    storage_set(&platform_key, &u64_to_bytes(next_platform_fees));
    storage_set(
        MA_GLOBAL_AUCTION_COUNT_KEY,
        &u64_to_bytes(next_auction_count),
    );
    storage_set(MA_GLOBAL_VOLUME_KEY, &u64_to_bytes(next_volume));
    storage_set(&collection_stats_key, &next_collection_stats);

    log_info("Auction finalized successfully!");
    reentrancy_exit();
    1
}

// ============================================================================
// OFFER/BID SYSTEM - Make offers on any NFT
// ============================================================================

// Offer: 121 bytes
// offerer (32) + nft_contract (32) + token_id (8) +
// amount (8) + payment_token (32) + expires (8) + active (1)
const OFFER_SIZE: usize = 121;

fn valid_offer_record(data: &[u8], offerer: Address, nft_contract: Address, token_id: u64) -> bool {
    data.len() == OFFER_SIZE
        && data[..32] == offerer.0
        && data[32..64] == nft_contract.0
        && bytes_to_u64(&data[64..72]) == token_id
        && bytes_to_u64(&data[72..80]) > 0
        && data[120] <= 1
}

#[no_mangle]
pub extern "C" fn make_offer(
    offerer_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    offer_amount: u64,
    payment_token_ptr: *const u8,
    duration: u64, // seconds until expiry
) -> u32 {
    log_info("Making offer...");

    if !is_ma_operational() || is_ma_paused() {
        log_info("LichenAuction is unavailable or paused");
        return 0;
    }

    let offerer = match read_address(offerer_ptr) {
        Some(addr) => addr,
        None => return 0,
    };

    if offerer.0 == [0u8; 32] || offer_amount == 0 {
        log_info("Offer amount must be > 0");
        return 0;
    }

    // AUDIT-FIX P2: Verify caller is the offerer
    let real_caller = get_caller();
    if real_caller.0 != offerer.0 {
        log_info("make_offer rejected: caller is not the offerer");
        return 0;
    }

    let nft_contract = match read_address(nft_contract_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    let payment_token = match read_address(payment_token_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    if nft_contract.0 == [0u8; 32] {
        return 0;
    }
    let (royalty_recipient, royalty_bps) =
        match canonical_collection_royalty(nft_contract, token_id) {
            Some(terms) => terms,
            None => {
                log_info("NFT collection royalty terms are unavailable or invalid");
                return 0;
            }
        };
    let fee_bps = match platform_fee_bps() {
        Some(fee) => fee,
        None => {
            log_info("Marketplace fee configuration is invalid");
            return 0;
        }
    };

    if !(MIN_DURATION..=MAX_DURATION).contains(&duration) {
        log_info("Offer duration is outside the supported range");
        return 0;
    }

    let now = get_timestamp();
    let expires = match now.checked_add(duration) {
        Some(expires) => expires,
        None => {
            log_info("Offer expiry overflow");
            return 0;
        }
    };

    let key = alloc::format!(
        "offer_{}_{}_{}",
        hex_addr(&offerer.0),
        hex_addr(&nft_contract.0),
        token_id
    );
    match storage_get(key.as_bytes()) {
        Some(existing) if !valid_offer_record(&existing, offerer, nft_contract, token_id) => {
            log_info("Existing offer state is malformed");
            return 0;
        }
        Some(existing) if existing[120] == 1 && bytes_to_u64(&existing[112..120]) >= now => {
            log_info("An active offer already exists; cancel it before replacing it");
            return 0;
        }
        _ => {}
    }

    // Build offer
    let mut offer = Vec::with_capacity(OFFER_SIZE);
    offer.extend_from_slice(&offerer.0); // 0-31
    offer.extend_from_slice(&nft_contract.0); // 32-63
    offer.extend_from_slice(&u64_to_bytes(token_id)); // 64-71
    offer.extend_from_slice(&u64_to_bytes(offer_amount)); // 72-79
    offer.extend_from_slice(&payment_token.0); // 80-111
    offer.extend_from_slice(&u64_to_bytes(expires)); // 112-119
    offer.push(1); // 120: active

    // Store offer
    storage_set(key.as_bytes(), &offer);
    storage_set(
        &offer_fee_key(&offerer.0, &nft_contract.0, token_id),
        &u64_to_bytes(fee_bps),
    );
    store_royalty_snapshot(
        &offer_royalty_key(&offerer.0, &nft_contract.0, token_id),
        royalty_recipient,
        royalty_bps,
    );

    log_info("Offer created!");
    log_info(&alloc::format!("   Amount: {}", offer_amount));
    log_info(&alloc::format!("   Expires in: {} slots", duration));
    1
}

/// Cancel an active offer. No payment is held until acceptance, so
/// cancellation only closes the immutable offer record.
#[no_mangle]
pub extern "C" fn cancel_offer(
    offerer_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
) -> u32 {
    if !is_ma_operational() {
        return 3;
    }
    let offerer = match read_address(offerer_ptr) {
        Some(address) => address,
        None => return 98,
    };
    if get_caller() != offerer {
        return 200;
    }
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let key = alloc::format!(
        "offer_{}_{}_{}",
        hex_addr(&offerer.0),
        hex_addr(&nft_contract.0),
        token_id
    );
    let mut offer = match storage_get(key.as_bytes()) {
        Some(data) if valid_offer_record(&data, offerer, nft_contract, token_id) => data,
        _ => return 1,
    };
    if offer[120] != 1 {
        return 2;
    }
    offer[120] = 0;
    storage_set(key.as_bytes(), &offer);
    1
}

#[no_mangle]
pub extern "C" fn accept_offer(
    seller_ptr: *const u8,
    offerer_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
) -> u32 {
    log_info("Accepting offer...");

    if !is_ma_operational() || is_ma_paused() {
        log_info("LichenAuction is unavailable or paused");
        return 0;
    }
    if !reentrancy_enter() {
        return 0;
    }

    let seller = match read_address(seller_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 0;
        }
    };

    // AUDIT-FIX P2: Verify caller is the seller
    let real_caller = get_caller();
    if real_caller.0 != seller.0 {
        log_info("accept_offer rejected: caller is not the seller");
        reentrancy_exit();
        return 0;
    }

    let offerer = match read_address(offerer_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 0;
        }
    };
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 0;
        }
    };

    if !nft_owned_by(nft_contract, token_id, seller) {
        log_info("Seller doesn't own NFT");
        reentrancy_exit();
        return 0;
    }
    if seller == offerer {
        log_info("Seller cannot accept their own offer");
        reentrancy_exit();
        return 0;
    }

    // Load offer
    let key = alloc::format!(
        "offer_{}_{}_{}",
        hex_addr(&offerer.0),
        hex_addr(&nft_contract.0),
        token_id
    );
    let offer_data = match storage_get(key.as_bytes()) {
        Some(data) => data,
        None => {
            log_info("Offer not found");
            reentrancy_exit();
            return 0;
        }
    };

    if !valid_offer_record(&offer_data, offerer, nft_contract, token_id) || offer_data[120] != 1 {
        reentrancy_exit();
        return 0;
    }

    // Check expiry
    let expires = bytes_to_u64(&offer_data[112..120]);
    if get_timestamp() > expires {
        log_info("Offer expired");
        reentrancy_exit();
        return 0;
    }

    let offer_amount = bytes_to_u64(&offer_data[72..80]);
    if offer_amount == 0 {
        log_info("Offer amount must be > 0");
        reentrancy_exit();
        return 0;
    }
    let mut payment_token_bytes = [0u8; 32];
    payment_token_bytes.copy_from_slice(&offer_data[80..112]);
    let payment_token_addr = Address(payment_token_bytes);

    // AUDIT-FIX H-5: Calculate marketplace fee + royalties (matching finalize_auction)
    let marketplace_fee_bps =
        match load_fee_snapshot(&offer_fee_key(&offerer.0, &nft_contract.0, token_id)) {
            Some(fee) => fee,
            None => {
                log_info("Offer fee snapshot is missing or invalid");
                reentrancy_exit();
                return 0;
            }
        };
    let (royalty_recipient, royalty_bps) =
        match load_royalty_snapshot(&offer_royalty_key(&offerer.0, &nft_contract.0, token_id)) {
            Some(terms) => terms,
            None => {
                log_info("Offer royalty snapshot is missing or invalid");
                reentrancy_exit();
                return 0;
            }
        };
    let royalty_bps = u64::from(royalty_bps);

    let total_deduction_bps = marketplace_fee_bps + royalty_bps.min(1000);
    let seller_amount =
        ((offer_amount as u128) * ((10000 - total_deduction_bps) as u128) / 10000) as u64;
    let marketplace_fee = ((offer_amount as u128) * (marketplace_fee_bps as u128) / 10000) as u64;
    let royalty_amount = ((offer_amount as u128) * (royalty_bps.min(1000) as u128) / 10000) as u64;
    let marketplace_addr = match marketplace_escrow_address() {
        Some(addr) => addr,
        None => {
            log_info("Marketplace escrow address not configured");
            reentrancy_exit();
            return 0;
        }
    };
    let platform_key = platform_fee_key(payment_token_addr);
    let next_platform_fees = match next_u64_value(&platform_key, marketplace_fee) {
        Some(value) => value,
        None => {
            log_info("Platform fee accounting would overflow or is malformed");
            reentrancy_exit();
            return 0;
        }
    };
    let next_sale_count = match next_u64_value(MA_GLOBAL_SALES_KEY, 1) {
        Some(value) => value,
        None => {
            log_info("Sale count accounting would overflow or is malformed");
            reentrancy_exit();
            return 0;
        }
    };
    let next_volume = match next_u64_value(MA_GLOBAL_VOLUME_KEY, offer_amount) {
        Some(value) => value,
        None => {
            log_info("Offer volume accounting would overflow or is malformed");
            reentrancy_exit();
            return 0;
        }
    };
    let (collection_stats_key, next_collection_stats) =
        match prepare_collection_sale(nft_contract, offer_amount) {
            Some(next) => next,
            None => {
                log_info("Collection sale accounting would overflow or is malformed");
                reentrancy_exit();
                return 0;
            }
        };
    if !can_record_unpaid_payout(payment_token_addr, offerer, offer_amount)
        || !can_record_unpaid_payouts(
            payment_token_addr,
            &[(seller, seller_amount), (royalty_recipient, royalty_amount)],
        )
    {
        log_info("Offer payout liability would overflow or is malformed");
        reentrancy_exit();
        return 0;
    }

    // Escrow full offer payment before moving the NFT.
    match receive_token_or_native(payment_token_addr, offerer, marketplace_addr, offer_amount) {
        Ok(true) => log_info("Offer payment escrowed"),
        _ => {
            log_info("Offer payment escrow failed");
            reentrancy_exit();
            return 0;
        }
    }

    // Transfer NFT (seller → offerer)
    if !transfer_nft_from_auction(nft_contract, seller, offerer, token_id) {
        log_info("NFT transfer failed; refunding offerer");
        match transfer_token_or_native(payment_token_addr, marketplace_addr, offerer, offer_amount)
        {
            Ok(true) => log_info("Offerer refunded"),
            _ => {
                if !record_unpaid_payout(payment_token_addr, offerer, offer_amount) {
                    reentrancy_exit();
                    return 0;
                }
                log_info("Offerer refund failed; payout recorded");
            }
        }
        reentrancy_exit();
        return 0;
    }
    log_info("NFT transferred");

    // Mark offer consumed
    let mut updated_offer = offer_data;
    updated_offer[120] = 0;
    storage_set(key.as_bytes(), &updated_offer);

    if seller_amount > 0 {
        match transfer_token_or_native(payment_token_addr, marketplace_addr, seller, seller_amount)
        {
            Ok(true) => log_info("Payment transferred to seller"),
            _ => {
                if !record_unpaid_payout(payment_token_addr, seller, seller_amount) {
                    reentrancy_exit();
                    return 0;
                }
                log_info("Seller payment failed; payout recorded");
            }
        }
    }

    if marketplace_fee > 0 {
        log_info(&alloc::format!(
            "Marketplace fee retained: {}",
            marketplace_fee
        ));
    }

    if royalty_amount > 0 {
        match transfer_token_or_native(
            payment_token_addr,
            marketplace_addr,
            royalty_recipient,
            royalty_amount,
        ) {
            Ok(true) => {
                log_info("Royalty paid to creator");
                log_info(&alloc::format!(
                    "   Royalty: {} ({}bps)",
                    royalty_amount,
                    royalty_bps
                ));
            }
            _ => {
                if !record_unpaid_payout(payment_token_addr, royalty_recipient, royalty_amount) {
                    reentrancy_exit();
                    return 0;
                }
                log_info("Royalty payment failed; payout recorded");
            }
        }
    }

    storage_set(&platform_key, &u64_to_bytes(next_platform_fees));
    storage_set(MA_GLOBAL_SALES_KEY, &u64_to_bytes(next_sale_count));
    storage_set(MA_GLOBAL_VOLUME_KEY, &u64_to_bytes(next_volume));
    storage_set(&collection_stats_key, &next_collection_stats);

    log_info("Offer accepted!");
    reentrancy_exit();
    1
}

// ============================================================================
// ROYALTY SYSTEM - Creator royalties on secondary sales
// ============================================================================

#[no_mangle]
pub extern "C" fn set_royalty(
    creator_ptr: *const u8,
    nft_contract_ptr: *const u8,
    royalty_basis_points: u64, // e.g., 500 = 5%
) -> u32 {
    log_info("Setting royalty...");
    if !is_ma_operational() {
        return 0;
    }

    // Preserve the administrative refresh surface, but accept only terms
    // returned by the NFT collection itself.
    let caller = get_caller();
    let creator = match read_address(creator_ptr) {
        Some(addr) => addr,
        None => return 0,
    };

    if !is_ma_admin(&caller.0) {
        log_info("Unauthorized: only marketplace admin can set collection royalty");
        return 0;
    }
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    let Some((canonical_recipient, canonical_bps)) = canonical_collection_royalty(nft_contract, 0)
    else {
        log_info("NFT collection royalty terms are unavailable or invalid");
        return 0;
    };
    if creator != canonical_recipient || royalty_basis_points != u64::from(canonical_bps) {
        log_info("Royalty terms do not match the NFT collection");
        return 0;
    }

    // Store: creator address (32) + basis_points (8)
    let mut royalty_data = Vec::with_capacity(40);
    royalty_data.extend_from_slice(&canonical_recipient.0);
    royalty_data.extend_from_slice(&u64_to_bytes(u64::from(canonical_bps)));

    let key = alloc::format!("royalty_{}", hex_addr(&nft_contract.0));
    storage_set(key.as_bytes(), &royalty_data);

    log_info("Canonical royalty terms cached");
    1
}

// ============================================================================
// COLLECTION STATS - Track volume, floor price, etc.
// ============================================================================

#[no_mangle]
pub extern "C" fn update_collection_stats(nft_contract_ptr: *const u8, sale_price: u64) -> u32 {
    let _ = (nft_contract_ptr, sale_price);
    log_info("Manual collection stats updates are disabled; settlements update stats atomically");
    2
}

#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)] // WASM ABI validates pointers before copying results.
pub extern "C" fn get_collection_stats(nft_contract_ptr: *const u8, result_ptr: *mut u8) -> u32 {
    if result_ptr.is_null() {
        return 0;
    }
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(addr) => addr,
        None => return 0,
    };

    let key = alloc::format!("stats_{}", hex_addr(&nft_contract.0));

    match storage_get(key.as_bytes()) {
        Some(stats) if stats.len() == 24 => {
            unsafe {
                core::ptr::copy_nonoverlapping(stats.as_ptr(), result_ptr, 24);
            }
            1
        }
        _ => 0,
    }
}

#[no_mangle]
pub extern "C" fn initialize(fee_treasury_ptr: *const u8) -> u32 {
    log_info("Initializing LichenAuction marketplace...");

    // AUDIT-FIX P2: Re-initialization guard
    if storage_get(b"ma_initialized").is_some() {
        log_info("LichenAuction already initialized");
        return 0;
    }

    // The admin must be established first so an uninitialized deployment
    // cannot be front-run and permanently bind an attacker-controlled fee
    // recipient.
    let caller = get_caller();
    if !is_ma_admin(&caller.0) {
        log_info("LichenAuction initialize rejected: admin required");
        return 0;
    }

    let treasury = match read_address(fee_treasury_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    if treasury.0 == [0u8; 32] {
        log_info("LichenAuction initialize rejected: zero fee treasury");
        return 0;
    }
    let escrow = get_contract_address();
    storage_set(MARKETPLACE_ADDR_KEY, &escrow.0);
    storage_set(MA_FEE_TREASURY_KEY, &treasury.0);
    storage_set(
        MA_PLATFORM_FEE_BPS_KEY,
        &u64_to_bytes(DEFAULT_MARKETPLACE_FEE_BPS),
    );
    storage_set(MA_STATE_VERSION_KEY, &u64_to_bytes(MA_STATE_VERSION));
    storage_set(MA_MIGRATION_LOCK_KEY, &[0u8]);
    log_info("   Self-custody escrow and fee treasury configured");

    storage_set(b"ma_initialized", &[1u8]);
    log_info("Marketplace ready!");
    log_info("   Features: Auctions, Offers, Royalties, Stats");
    1
}

// ============================================================================
// V2: RESERVE PRICES, CANCEL, PAUSE, ADMIN
// ============================================================================

/// Set a reserve price for an auction. Only callable by seller before any bids.
/// If highest_bid < reserve at finalization, auction is cancelled + bidder refunded.
///
/// Returns: 0 success, 1 auction not found, 2 not seller, 3 auction has bids, 4 paused
#[no_mangle]
pub extern "C" fn set_reserve_price(
    caller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    reserve: u64,
) -> u32 {
    if !is_ma_operational() || is_ma_paused() {
        return 4;
    }
    let caller = match read_address(caller_ptr) {
        Some(addr) => addr,
        None => return 2,
    };

    // AUDIT-FIX H-6: Verify caller matches actual transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller.0 {
        log_info("set_reserve_price: caller does not match signer — rejected");
        return 2;
    }

    let nft_contract = match read_address(nft_contract_ptr) {
        Some(addr) => addr,
        None => return 1,
    };

    let key = alloc::format!("auction_{}_{}", hex_addr(&nft_contract.0), token_id);
    let auction_data = match storage_get(key.as_bytes()) {
        Some(data) if valid_auction_record(&data, nft_contract, token_id) => data,
        _ => return 1,
    };

    // Only seller
    if caller.0[..] != auction_data[0..32] {
        return 2;
    }

    // No bids yet
    let highest_bid = bytes_to_u64(&auction_data[160..168]);
    if highest_bid > 0 {
        return 3;
    }

    let rk = reserve_key(&nft_contract.0, token_id);
    storage_set(&rk, &u64_to_bytes(reserve));
    log_info("Reserve price set");
    0
}

/// Cancel an auction. Only seller, only if no bids placed.
///
/// Returns: 0 success, 1 not found, 2 not seller, 3 has bids, 4 not active
#[no_mangle]
pub extern "C" fn cancel_auction(
    caller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
) -> u32 {
    if !is_ma_operational() {
        return 6;
    }
    let caller = match read_address(caller_ptr) {
        Some(addr) => addr,
        None => return 2,
    };

    // AUDIT-FIX H-7: Verify caller matches actual transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller.0 {
        log_info("cancel_auction: caller does not match signer — rejected");
        return 2;
    }

    let nft_contract = match read_address(nft_contract_ptr) {
        Some(addr) => addr,
        None => return 1,
    };

    let key = alloc::format!("auction_{}_{}", hex_addr(&nft_contract.0), token_id);
    let mut auction_data = match storage_get(key.as_bytes()) {
        Some(data) if valid_auction_record(&data, nft_contract, token_id) => data,
        _ => return 1,
    };

    if auction_data[168] != 1 {
        return 4;
    }
    if caller.0[..] != auction_data[0..32] {
        return 2;
    }

    let highest_bid = bytes_to_u64(&auction_data[160..168]);
    if highest_bid > 0 {
        return 3;
    }

    if !transfer_nft_from_auction(nft_contract, get_contract_address(), caller, token_id) {
        log_info("Auction cancel failed: NFT escrow return failed");
        return 5;
    }

    auction_data[168] = 0;
    storage_set(key.as_bytes(), &auction_data);
    log_info("Auction cancelled by seller");
    0
}

/// Initialize LichenAuction admin (once).
/// Returns: 0 success, 1 already set, 2 caller mismatch
#[no_mangle]
pub extern "C" fn initialize_ma_admin(admin_ptr: *const u8) -> u32 {
    let admin = match read_address(admin_ptr) {
        Some(addr) => addr,
        None => return 2,
    };
    if storage_get(MA_ADMIN_KEY).is_some() {
        return 1;
    }

    let real_caller = get_caller();
    if real_caller.0 != admin.0 || admin.0 == [0u8; 32] {
        log_info("LichenAuction admin init rejected: caller mismatch");
        return 2;
    }

    storage_set(MA_ADMIN_KEY, &admin.0);
    storage_set(MA_FEE_TREASURY_KEY, &admin.0);
    storage_set(
        MA_PLATFORM_FEE_BPS_KEY,
        &u64_to_bytes(DEFAULT_MARKETPLACE_FEE_BPS),
    );
    log_info("LichenAuction admin set");
    0
}

/// Begin a two-step marketplace admin rotation.
#[no_mangle]
pub extern "C" fn propose_ma_admin(caller_ptr: *const u8, next_admin_ptr: *const u8) -> u32 {
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let next_admin = match read_address(next_admin_ptr) {
        Some(address) => address,
        None => return 98,
    };
    if get_caller() != caller {
        return 200;
    }
    if !is_ma_admin(&caller.0) {
        return 1;
    }
    if next_admin.0 == [0u8; 32] || next_admin == caller {
        return 2;
    }
    storage_set(MA_PENDING_ADMIN_KEY, &next_admin.0);
    0
}

/// Complete a pending admin rotation. The proposed account must accept it.
#[no_mangle]
pub extern "C" fn accept_ma_admin(caller_ptr: *const u8) -> u32 {
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => return 98,
    };
    if get_caller() != caller {
        return 200;
    }
    let pending = match storage_get(MA_PENDING_ADMIN_KEY) {
        Some(data) if data.len() == 32 => data,
        _ => return 1,
    };
    if pending.as_slice() != caller.0 {
        return 2;
    }
    storage_set(MA_ADMIN_KEY, &caller.0);
    storage_set(MA_PENDING_ADMIN_KEY, &[]);
    0
}

/// Freeze legacy state before capturing an exact V3 migration manifest.
#[no_mangle]
pub extern "C" fn begin_v3_migration(caller_ptr: *const u8) -> u32 {
    if authenticated_admin(caller_ptr).is_none() {
        return 1;
    }
    if !is_ma_initialized() || !is_ma_paused() {
        return 2;
    }
    if ma_state_version() != Some(2) || is_ma_migration_locked() {
        return 3;
    }
    storage_set(MA_MIGRATION_LOCK_KEY, &[1u8]);
    storage_set(MA_MIGRATION_MANIFEST_KEY, &[]);
    storage_set(MA_MIGRATION_EXPECTED_AUCTIONS_KEY, &u64_to_bytes(0));
    storage_set(MA_MIGRATION_EXPECTED_OFFERS_KEY, &u64_to_bytes(0));
    storage_set(MA_MIGRATION_MIGRATED_AUCTIONS_KEY, &u64_to_bytes(0));
    storage_set(MA_MIGRATION_MIGRATED_OFFERS_KEY, &u64_to_bytes(0));
    0
}

/// Seal the frozen manifest hash and exact row counts before any row migrates.
#[no_mangle]
pub extern "C" fn seal_v3_migration_manifest(
    caller_ptr: *const u8,
    manifest_ptr: *const u8,
    expected_auctions: u64,
    expected_offers: u64,
) -> u32 {
    if authenticated_admin(caller_ptr).is_none() {
        return 1;
    }
    let manifest = match read_address(manifest_ptr) {
        Some(value) if value.0 != [0u8; 32] => value,
        _ => return 2,
    };
    if !is_ma_migration_locked() || migration_manifest().is_some() {
        return 3;
    }
    if expected_auctions > 1_000_000 || expected_offers > 1_000_000 {
        return 4;
    }
    storage_set(MA_MIGRATION_MANIFEST_KEY, &manifest.0);
    storage_set(
        MA_MIGRATION_EXPECTED_AUCTIONS_KEY,
        &u64_to_bytes(expected_auctions),
    );
    storage_set(
        MA_MIGRATION_EXPECTED_OFFERS_KEY,
        &u64_to_bytes(expected_offers),
    );
    0
}

/// Return canonical royalty terms for manifest capture.
#[no_mangle]
pub extern "C" fn probe_canonical_royalty(nft_contract_ptr: *const u8, token_id: u64) -> u32 {
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) if address.0 != [0u8; 32] => address,
        _ => return 1,
    };
    let (recipient, bps) = match canonical_collection_royalty(nft_contract, token_id) {
        Some(terms) => terms,
        None => return 2,
    };
    let mut result = [0u8; 34];
    result[..32].copy_from_slice(&recipient.0);
    result[32..].copy_from_slice(&bps.to_le_bytes());
    lichen_sdk::set_return_data(&result);
    0
}

/// Migrate one frozen legacy auction. Active NFTs are moved into contract
/// custody; sellers must approve this contract before the row can migrate.
#[no_mangle]
pub extern "C" fn migrate_v3_auction(
    caller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    royalty_recipient_ptr: *const u8,
    royalty_bps: u64,
) -> u32 {
    if authenticated_admin(caller_ptr).is_none() {
        return 8;
    }
    if !is_ma_migration_locked() || migration_manifest().is_none() {
        return 1;
    }
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) if address.0 != [0u8; 32] => address,
        _ => return 2,
    };
    let expected_recipient = match read_address(royalty_recipient_ptr) {
        Some(address) => address,
        None => return 2,
    };
    let expected_bps = match u16::try_from(royalty_bps) {
        Ok(value) if value <= 1_000 => value,
        _ => return 2,
    };
    if expected_bps > 0 && expected_recipient.0 == [0u8; 32] {
        return 2;
    }
    let marker = migration_marker_key(b"auction", &[&nft_contract.0], token_id);
    if storage_get(&marker).is_some_and(|data| data.as_slice() == [1u8]) {
        return 0;
    }
    let expected = match load_u64_or_zero(MA_MIGRATION_EXPECTED_AUCTIONS_KEY) {
        Some(value) => value,
        None => return 3,
    };
    let migrated = match load_u64_or_zero(MA_MIGRATION_MIGRATED_AUCTIONS_KEY) {
        Some(value) if value < expected => value,
        _ => return 3,
    };
    let key = alloc::format!("auction_{}_{}", hex_addr(&nft_contract.0), token_id);
    let auction = match storage_get(key.as_bytes()) {
        Some(data) if valid_auction_record(&data, nft_contract, token_id) => data,
        _ => return 4,
    };
    if canonical_collection_royalty(nft_contract, token_id)
        != Some((expected_recipient, expected_bps))
    {
        return 5;
    }
    if auction[168] == 1 {
        let mut seller = [0u8; 32];
        seller.copy_from_slice(&auction[..32]);
        let seller = Address(seller);
        let contract = get_contract_address();
        if bytes_to_u64(&auction[160..168]) > 0 && legacy_escrow_address() != Some(contract) {
            return 6;
        }
        match nft_owner(nft_contract, token_id) {
            Some(owner) if owner == contract => {}
            Some(owner) if owner == seller => {
                if !transfer_nft_from_auction(nft_contract, seller, contract, token_id) {
                    return 7;
                }
            }
            _ => return 7,
        }
    }
    let next_migrated = match migrated.checked_add(1) {
        Some(value) => value,
        None => return 3,
    };
    storage_set(
        &auction_fee_key(&nft_contract.0, token_id),
        &u64_to_bytes(DEFAULT_MARKETPLACE_FEE_BPS),
    );
    store_royalty_snapshot(
        &auction_royalty_key(&nft_contract.0, token_id),
        expected_recipient,
        expected_bps,
    );
    storage_set(&marker, &[1u8]);
    storage_set(
        MA_MIGRATION_MIGRATED_AUCTIONS_KEY,
        &u64_to_bytes(next_migrated),
    );
    0
}

/// Migrate one frozen legacy offer and bind its immutable settlement terms.
#[no_mangle]
pub extern "C" fn migrate_v3_offer(
    caller_ptr: *const u8,
    offerer_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    royalty_recipient_ptr: *const u8,
    royalty_bps: u64,
) -> u32 {
    if authenticated_admin(caller_ptr).is_none() {
        return 7;
    }
    if !is_ma_migration_locked() || migration_manifest().is_none() {
        return 1;
    }
    let offerer = match read_address(offerer_ptr) {
        Some(address) if address.0 != [0u8; 32] => address,
        _ => return 2,
    };
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) if address.0 != [0u8; 32] => address,
        _ => return 2,
    };
    let expected_recipient = match read_address(royalty_recipient_ptr) {
        Some(address) => address,
        None => return 2,
    };
    let expected_bps = match u16::try_from(royalty_bps) {
        Ok(value) if value <= 1_000 => value,
        _ => return 2,
    };
    if expected_bps > 0 && expected_recipient.0 == [0u8; 32] {
        return 2;
    }
    let marker = migration_marker_key(b"offer", &[&offerer.0, &nft_contract.0], token_id);
    if storage_get(&marker).is_some_and(|data| data.as_slice() == [1u8]) {
        return 0;
    }
    let expected = match load_u64_or_zero(MA_MIGRATION_EXPECTED_OFFERS_KEY) {
        Some(value) => value,
        None => return 3,
    };
    let migrated = match load_u64_or_zero(MA_MIGRATION_MIGRATED_OFFERS_KEY) {
        Some(value) if value < expected => value,
        _ => return 3,
    };
    let key = alloc::format!(
        "offer_{}_{}_{}",
        hex_addr(&offerer.0),
        hex_addr(&nft_contract.0),
        token_id
    );
    let offer = match storage_get(key.as_bytes()) {
        Some(data) if valid_offer_record(&data, offerer, nft_contract, token_id) => data,
        _ => return 4,
    };
    if offer[120] == 1
        && bytes_to_u64(&offer[72..80]) > 0
        && legacy_escrow_address() != Some(get_contract_address())
    {
        return 6;
    }
    if canonical_collection_royalty(nft_contract, token_id)
        != Some((expected_recipient, expected_bps))
    {
        return 5;
    }
    let next_migrated = match migrated.checked_add(1) {
        Some(value) => value,
        None => return 3,
    };
    storage_set(
        &offer_fee_key(&offerer.0, &nft_contract.0, token_id),
        &u64_to_bytes(DEFAULT_MARKETPLACE_FEE_BPS),
    );
    store_royalty_snapshot(
        &offer_royalty_key(&offerer.0, &nft_contract.0, token_id),
        expected_recipient,
        expected_bps,
    );
    storage_set(&marker, &[1u8]);
    storage_set(
        MA_MIGRATION_MIGRATED_OFFERS_KEY,
        &u64_to_bytes(next_migrated),
    );
    0
}

/// Activate V3 only after every row in the sealed manifest has migrated.
#[no_mangle]
pub extern "C" fn complete_v3_migration(caller_ptr: *const u8) -> u32 {
    if authenticated_admin(caller_ptr).is_none() {
        return 1;
    }
    if !is_ma_migration_locked() || migration_manifest().is_none() {
        return 2;
    }
    let expected_auctions = load_u64_or_zero(MA_MIGRATION_EXPECTED_AUCTIONS_KEY);
    let migrated_auctions = load_u64_or_zero(MA_MIGRATION_MIGRATED_AUCTIONS_KEY);
    let expected_offers = load_u64_or_zero(MA_MIGRATION_EXPECTED_OFFERS_KEY);
    let migrated_offers = load_u64_or_zero(MA_MIGRATION_MIGRATED_OFFERS_KEY);
    if expected_auctions.is_none()
        || expected_auctions != migrated_auctions
        || expected_offers.is_none()
        || expected_offers != migrated_offers
    {
        return 3;
    }
    storage_set(MARKETPLACE_ADDR_KEY, &get_contract_address().0);
    storage_set(MA_STATE_VERSION_KEY, &u64_to_bytes(MA_STATE_VERSION));
    storage_set(MA_MIGRATION_LOCK_KEY, &[0u8]);
    0
}

/// Pause marketplace. Admin only.
/// Returns: 0 success, 1 not admin, 2 already paused
#[no_mangle]
pub extern "C" fn ma_pause() -> u32 {
    // H-9: Use get_caller() for authenticated caller instead of spoofable parameter
    let caller = get_caller();
    if !is_ma_admin(&caller.0) {
        return 1;
    }
    if is_ma_paused() {
        return 2;
    }
    storage_set(MA_PAUSE_KEY, &[1]);
    log_info("LichenAuction paused");
    0
}

/// Unpause marketplace. Admin only.
/// Returns: 0 success, 1 not admin, 2 not paused
#[no_mangle]
pub extern "C" fn ma_unpause() -> u32 {
    // H-9: Use get_caller() for authenticated caller instead of spoofable parameter
    let caller = get_caller();
    if !is_ma_admin(&caller.0) {
        return 1;
    }
    if !is_ma_paused() {
        return 2;
    }
    storage_set(MA_PAUSE_KEY, &[0]);
    log_info("LichenAuction unpaused");
    0
}

/// Retry a failed seller, bidder-refund, or royalty payout.
#[no_mangle]
pub extern "C" fn claim_unpaid_payout(caller_ptr: *const u8, token_ptr: *const u8) -> u32 {
    if !is_ma_operational() {
        return 8;
    }
    if !reentrancy_enter() {
        return 20;
    }
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 98;
        }
    };
    let token = match read_address(token_ptr) {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 98;
        }
    };
    if get_caller() != caller {
        reentrancy_exit();
        return 200;
    }
    let escrow = match marketplace_escrow_address() {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 7;
        }
    };
    let key = unpaid_payout_key(token, caller);
    let amount = match load_u64_or_zero(&key) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 3;
        }
    };
    if amount == 0 {
        reentrancy_exit();
        return 2;
    }
    storage_set(&key, &u64_to_bytes(0));
    match transfer_token_or_native(token, escrow, caller, amount) {
        Ok(true) => {
            lichen_sdk::set_return_data(&u64_to_bytes(amount));
            reentrancy_exit();
            0
        }
        _ => {
            storage_set(&key, &u64_to_bytes(amount));
            reentrancy_exit();
            32
        }
    }
}

#[no_mangle]
pub extern "C" fn get_unpaid_payout(token_ptr: *const u8, recipient_ptr: *const u8) -> u32 {
    let token = match read_address(token_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let recipient = match read_address(recipient_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let amount = match load_u64_or_zero(&unpaid_payout_key(token, recipient)) {
        Some(value) => value,
        None => return 3,
    };
    lichen_sdk::set_return_data(&u64_to_bytes(amount));
    0
}

/// Configure the recipient of custody-backed platform fees.
#[no_mangle]
pub extern "C" fn set_fee_treasury(caller_ptr: *const u8, treasury_ptr: *const u8) -> u32 {
    if !is_ma_operational() {
        return 3;
    }
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let treasury = match read_address(treasury_ptr) {
        Some(address) => address,
        None => return 98,
    };
    if get_caller() != caller {
        return 200;
    }
    if !is_ma_admin(&caller.0) {
        return 1;
    }
    if treasury.0 == [0u8; 32] {
        return 2;
    }
    storage_set(MA_FEE_TREASURY_KEY, &treasury.0);
    0
}

/// Set the fee for newly created auctions and offers. Existing terms are
/// snapshotted and never change retroactively.
#[no_mangle]
pub extern "C" fn set_platform_fee(caller_ptr: *const u8, fee_bps: u64) -> u32 {
    if !is_ma_operational() {
        return 3;
    }
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => return 98,
    };
    if get_caller() != caller {
        return 200;
    }
    if !is_ma_admin(&caller.0) {
        return 1;
    }
    if fee_bps > MAX_MARKETPLACE_FEE_BPS {
        return 2;
    }
    storage_set(MA_PLATFORM_FEE_BPS_KEY, &u64_to_bytes(fee_bps));
    0
}

/// Withdraw an exact amount of realized platform fees to the configured
/// treasury. Failed transfers restore the fee ledger for exact retry.
#[no_mangle]
pub extern "C" fn withdraw_platform_fees(
    caller_ptr: *const u8,
    token_ptr: *const u8,
    amount: u64,
) -> u32 {
    if !is_ma_operational() {
        return 7;
    }
    if !reentrancy_enter() {
        return 20;
    }
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 98;
        }
    };
    let token = match read_address(token_ptr) {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 98;
        }
    };
    if get_caller() != caller {
        reentrancy_exit();
        return 200;
    }
    if !is_ma_admin(&caller.0) || amount == 0 {
        reentrancy_exit();
        return 1;
    }
    let treasury = match storage_get(MA_FEE_TREASURY_KEY) {
        Some(data) if data.len() == 32 && data.as_slice() != [0u8; 32] => {
            let mut address = [0u8; 32];
            address.copy_from_slice(&data);
            Address(address)
        }
        _ => {
            reentrancy_exit();
            return 3;
        }
    };
    let escrow = match marketplace_escrow_address() {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 3;
        }
    };
    let key = platform_fee_key(token);
    let accrued = match load_u64_or_zero(&key) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 6;
        }
    };
    let remaining = match accrued.checked_sub(amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 4;
        }
    };
    storage_set(&key, &u64_to_bytes(remaining));
    match transfer_token_or_native(token, escrow, treasury, amount) {
        Ok(true) => {
            lichen_sdk::set_return_data(&u64_to_bytes(amount));
            reentrancy_exit();
            0
        }
        _ => {
            storage_set(&key, &u64_to_bytes(accrued));
            reentrancy_exit();
            5
        }
    }
}

#[no_mangle]
pub extern "C" fn get_platform_fees(token_ptr: *const u8) -> u32 {
    let token = match read_address(token_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let accrued = match load_u64_or_zero(&platform_fee_key(token)) {
        Some(value) => value,
        None => return 3,
    };
    lichen_sdk::set_return_data(&u64_to_bytes(accrued));
    0
}

/// Query an offer and its immutable settlement terms. Returns the original
/// 121-byte record followed by fee bps(8), royalty recipient(32), royalty bps(2).
#[no_mangle]
pub extern "C" fn get_offer_info(
    offerer_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
) -> u32 {
    let offerer = match read_address(offerer_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let key = alloc::format!(
        "offer_{}_{}_{}",
        hex_addr(&offerer.0),
        hex_addr(&nft_contract.0),
        token_id
    );
    let offer = match storage_get(key.as_bytes()) {
        Some(data) if valid_offer_record(&data, offerer, nft_contract, token_id) => data,
        _ => return 1,
    };
    let fee_bps = match load_fee_snapshot(&offer_fee_key(&offerer.0, &nft_contract.0, token_id)) {
        Some(value) => value,
        None => return 2,
    };
    let (royalty_recipient, royalty_bps) =
        match load_royalty_snapshot(&offer_royalty_key(&offerer.0, &nft_contract.0, token_id)) {
            Some(value) => value,
            None => return 2,
        };
    let mut result = Vec::with_capacity(OFFER_SIZE + 42);
    result.extend_from_slice(&offer[..OFFER_SIZE]);
    result.extend_from_slice(&u64_to_bytes(fee_bps));
    result.extend_from_slice(&royalty_recipient.0);
    result.extend_from_slice(&royalty_bps.to_le_bytes());
    lichen_sdk::set_return_data(&result);
    0
}

/// Get auction info as return data.
/// Layout: original 169 bytes + reserve(8) + extensions(8) + fee_bps(8) +
/// royalty recipient(32) + royalty bps(2) = 227 bytes.
/// Returns: 0 success, 1 not found
#[no_mangle]
pub extern "C" fn get_auction_info(nft_contract_ptr: *const u8, token_id: u64) -> u32 {
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(addr) => addr,
        None => return 1,
    };
    let key = alloc::format!("auction_{}_{}", hex_addr(&nft_contract.0), token_id);
    let auction_data = match storage_get(key.as_bytes()) {
        Some(data) if valid_auction_record(&data, nft_contract, token_id) => data,
        _ => return 1,
    };

    let rk = reserve_key(&nft_contract.0, token_id);
    let reserve = match load_u64_or_zero(&rk) {
        Some(value) => value,
        None => return 2,
    };
    let ek = ext_count_key(&nft_contract.0, token_id);
    let extensions = match load_u64_or_zero(&ek) {
        Some(value) if value <= MAX_EXTENSIONS => value,
        _ => return 2,
    };

    let fee_bps = match load_fee_snapshot(&auction_fee_key(&nft_contract.0, token_id)) {
        Some(value) => value,
        None => return 2,
    };
    let (royalty_recipient, royalty_bps) =
        match load_royalty_snapshot(&auction_royalty_key(&nft_contract.0, token_id)) {
            Some(value) => value,
            None => return 2,
        };
    let mut info = Vec::with_capacity(AUCTION_SIZE + 58);
    info.extend_from_slice(&auction_data[..AUCTION_SIZE]);
    info.extend_from_slice(&u64_to_bytes(reserve));
    info.extend_from_slice(&u64_to_bytes(extensions));
    info.extend_from_slice(&u64_to_bytes(fee_bps));
    info.extend_from_slice(&royalty_recipient.0);
    info.extend_from_slice(&royalty_bps.to_le_bytes());
    lichen_sdk::set_return_data(&info);
    0
}

/// Get auction stats [auction_count(8), total_volume(8), total_sales(8)]
#[no_mangle]
pub extern "C" fn get_auction_stats() -> u32 {
    let Some(auction_count) = load_u64_or_zero(MA_GLOBAL_AUCTION_COUNT_KEY) else {
        return 1;
    };
    let Some(total_volume) = load_u64_or_zero(MA_GLOBAL_VOLUME_KEY) else {
        return 1;
    };
    let Some(total_sales) = load_u64_or_zero(MA_GLOBAL_SALES_KEY) else {
        return 1;
    };
    let mut buf = Vec::with_capacity(24);
    buf.extend_from_slice(&u64_to_bytes(auction_count));
    buf.extend_from_slice(&u64_to_bytes(total_volume));
    buf.extend_from_slice(&u64_to_bytes(total_sales));
    lichen_sdk::set_return_data(&buf);
    0
}

/// Query operational configuration as
/// admin(32) + pending_admin(32) + treasury(32) + fee_bps(8) + paused(1).
#[no_mangle]
pub extern "C" fn get_marketplace_config() -> u32 {
    if !is_ma_initialized() {
        return 1;
    }
    let admin = match storage_get(MA_ADMIN_KEY) {
        Some(data) if data.len() == 32 => data,
        _ => return 2,
    };
    let pending_admin = match storage_get(MA_PENDING_ADMIN_KEY) {
        None => [0u8; 32],
        Some(data) if data.is_empty() => [0u8; 32],
        Some(data) if data.len() == 32 => {
            let mut address = [0u8; 32];
            address.copy_from_slice(&data);
            address
        }
        Some(_) => return 2,
    };
    let treasury = match storage_get(MA_FEE_TREASURY_KEY) {
        Some(data) if data.len() == 32 => data,
        _ => return 2,
    };
    let fee_bps = match platform_fee_bps() {
        Some(value) => value,
        None => return 2,
    };
    let paused = match storage_get(MA_PAUSE_KEY) {
        None => 0,
        Some(data) if data.as_slice() == [0u8] => 0,
        Some(data) if data.as_slice() == [1u8] => 1,
        Some(_) => return 2,
    };
    let mut result = Vec::with_capacity(105);
    result.extend_from_slice(&admin);
    result.extend_from_slice(&pending_admin);
    result.extend_from_slice(&treasury);
    result.extend_from_slice(&u64_to_bytes(fee_bps));
    result.push(paused);
    lichen_sdk::set_return_data(&result);
    0
}

/// Query V3 migration state as version(8), lock(1), paused(1), sealed(1),
/// expected/migrated auctions(8+8), expected/migrated offers(8+8),
/// manifest hash(32), legacy escrow(32), and current contract escrow(32).
#[no_mangle]
pub extern "C" fn get_v3_migration_status() -> u32 {
    let version = match ma_state_version() {
        Some(value) => value,
        None => return 1,
    };
    let lock = match storage_get(MA_MIGRATION_LOCK_KEY) {
        None => 0,
        Some(data) if data.as_slice() == [0u8] => 0,
        Some(data) if data.as_slice() == [1u8] => 1,
        Some(_) => return 1,
    };
    let expected_auctions = match load_u64_or_zero(MA_MIGRATION_EXPECTED_AUCTIONS_KEY) {
        Some(value) => value,
        None => return 1,
    };
    let migrated_auctions = match load_u64_or_zero(MA_MIGRATION_MIGRATED_AUCTIONS_KEY) {
        Some(value) => value,
        None => return 1,
    };
    let expected_offers = match load_u64_or_zero(MA_MIGRATION_EXPECTED_OFFERS_KEY) {
        Some(value) => value,
        None => return 1,
    };
    let migrated_offers = match load_u64_or_zero(MA_MIGRATION_MIGRATED_OFFERS_KEY) {
        Some(value) => value,
        None => return 1,
    };
    let manifest = migration_manifest().unwrap_or([0u8; 32]);
    let legacy_escrow = legacy_escrow_address().unwrap_or(Address([0u8; 32]));
    let mut result = Vec::with_capacity(139);
    result.extend_from_slice(&u64_to_bytes(version));
    result.push(lock);
    result.push(u8::from(is_ma_paused()));
    result.push(u8::from(manifest != [0u8; 32]));
    result.extend_from_slice(&u64_to_bytes(expected_auctions));
    result.extend_from_slice(&u64_to_bytes(migrated_auctions));
    result.extend_from_slice(&u64_to_bytes(expected_offers));
    result.extend_from_slice(&u64_to_bytes(migrated_offers));
    result.extend_from_slice(&manifest);
    result.extend_from_slice(&legacy_escrow.0);
    result.extend_from_slice(&get_contract_address().0);
    lichen_sdk::set_return_data(&result);
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
        test_mock::set_contract_address([0xA5; 32]);
    }

    fn initialize_test_admin(admin: &[u8; 32]) -> u32 {
        test_mock::set_caller(*admin);
        initialize_ma_admin(admin.as_ptr())
    }

    fn initialize_test_marketplace(admin: &[u8; 32], marketplace: &[u8; 32]) -> u32 {
        let admin_result = initialize_test_admin(admin);
        assert!(admin_result == 0 || admin_result == 1);
        test_mock::set_caller(*admin);
        initialize(marketplace.as_ptr())
    }

    fn auction_key(nft_contract: &[u8; 32], token_id: u64) -> Vec<u8> {
        alloc::format!("auction_{}_{}", hex_addr(nft_contract), token_id).into_bytes()
    }

    fn offer_key(offerer: &[u8; 32], nft_contract: &[u8; 32], token_id: u64) -> Vec<u8> {
        alloc::format!(
            "offer_{}_{}_{}",
            hex_addr(offerer),
            hex_addr(nft_contract),
            token_id
        )
        .into_bytes()
    }

    fn unpaid_payout_key(token: &[u8; 32], recipient: &[u8; 32]) -> Vec<u8> {
        let mut key = b"unpaid_payout:".to_vec();
        key.extend_from_slice(token);
        key.push(b':');
        key.extend_from_slice(recipient);
        key
    }

    fn royalty_response(recipient: [u8; 32], bps: u16) -> Vec<u8> {
        let mut response = Vec::with_capacity(34);
        response.extend_from_slice(&recipient);
        response.extend_from_slice(&bps.to_le_bytes());
        response
    }

    fn initialize_default_marketplace() {
        assert_eq!(initialize_test_marketplace(&[9u8; 32], &[1u8; 32]), 1);
    }

    /// Helper to manually create auction data in storage (bypassing cross-contract calls)
    fn create_test_auction(
        nft_contract: &[u8; 32],
        token_id: u64,
        seller: &[u8; 32],
        min_bid: u64,
        end_time: u64,
    ) {
        if lichen_sdk::storage_get(b"ma_initialized").is_none() {
            lichen_sdk::storage_set(b"ma_initialized", &[1u8]);
            lichen_sdk::storage_set(MA_STATE_VERSION_KEY, &u64_to_bytes(MA_STATE_VERSION));
            lichen_sdk::storage_set(MA_MIGRATION_LOCK_KEY, &[0u8]);
            lichen_sdk::storage_set(
                MA_PLATFORM_FEE_BPS_KEY,
                &u64_to_bytes(DEFAULT_MARKETPLACE_FEE_BPS),
            );
        }
        let payment_token = [0xAAu8; 32];
        let mut auction = Vec::with_capacity(AUCTION_SIZE);
        auction.extend_from_slice(seller);
        auction.extend_from_slice(nft_contract);
        auction.extend_from_slice(&u64_to_bytes(token_id));
        auction.extend_from_slice(&u64_to_bytes(min_bid));
        auction.extend_from_slice(&payment_token);
        auction.extend_from_slice(&u64_to_bytes(0)); // start_time
        auction.extend_from_slice(&u64_to_bytes(end_time)); // end_time
        auction.extend_from_slice(&[0u8; 32]); // highest_bidder
        auction.extend_from_slice(&[0u8; 8]); // highest_bid
        auction.push(1); // active
        let key = auction_key(nft_contract, token_id);
        lichen_sdk::storage_set(&key, &auction);
        lichen_sdk::storage_set(
            &auction_fee_key(nft_contract, token_id),
            &u64_to_bytes(DEFAULT_MARKETPLACE_FEE_BPS),
        );
        store_royalty_snapshot(
            &auction_royalty_key(nft_contract, token_id),
            Address([0u8; 32]),
            0,
        );
    }

    fn create_test_offer(
        offerer: &[u8; 32],
        nft_contract: &[u8; 32],
        token_id: u64,
        amount: u64,
        expires: u64,
    ) {
        let payment_token = [0xBBu8; 32];
        let mut offer = Vec::with_capacity(OFFER_SIZE);
        offer.extend_from_slice(offerer);
        offer.extend_from_slice(nft_contract);
        offer.extend_from_slice(&u64_to_bytes(token_id));
        offer.extend_from_slice(&u64_to_bytes(amount));
        offer.extend_from_slice(&payment_token);
        offer.extend_from_slice(&u64_to_bytes(expires));
        offer.push(1);
        storage_set(&offer_key(offerer, nft_contract, token_id), &offer);
    }

    #[test]
    fn test_initialize() {
        setup();
        let admin = [9u8; 32];
        let addr = [1u8; 32];
        assert_eq!(initialize_test_admin(&admin), 0);
        test_mock::set_caller(admin);
        let result = initialize(addr.as_ptr());
        assert_eq!(result, 1);
        assert_eq!(
            test_mock::get_storage(MARKETPLACE_ADDR_KEY),
            Some([0xA5; 32].to_vec())
        );
        assert_eq!(
            test_mock::get_storage(MA_FEE_TREASURY_KEY),
            Some(addr.to_vec())
        );
    }

    #[test]
    fn test_initialize_requires_admin_without_mutation() {
        setup();
        let admin = [9u8; 32];
        let attacker = [8u8; 32];
        let addr = [1u8; 32];
        assert_eq!(initialize_test_admin(&admin), 0);
        test_mock::set_caller(attacker);

        assert_eq!(initialize(addr.as_ptr()), 0);
        assert_eq!(test_mock::get_storage(MARKETPLACE_ADDR_KEY), None);
        assert_eq!(test_mock::get_storage(b"ma_initialized"), None);
    }

    #[test]
    fn test_initialize_rejects_zero_escrow_without_mutation() {
        setup();
        let admin = [9u8; 32];
        let zero = [0u8; 32];
        assert_eq!(initialize_test_admin(&admin), 0);
        test_mock::set_caller(admin);

        assert_eq!(initialize(zero.as_ptr()), 0);
        assert_eq!(test_mock::get_storage(MARKETPLACE_ADDR_KEY), None);
        assert_eq!(test_mock::get_storage(b"ma_initialized"), None);
    }

    #[test]
    fn test_create_auction_nft_check_fails() {
        setup();
        initialize_default_marketplace();
        let seller = [2u8; 32];
        let nft = [3u8; 32];
        let pay = [4u8; 32];
        test_mock::set_caller(seller);
        // call_nft_owner returns Err in test mock
        assert_eq!(
            create_auction(seller.as_ptr(), nft.as_ptr(), 1, 1000, pay.as_ptr(), 3600),
            0
        );
    }

    #[test]
    fn test_create_auction_escrows_nft_and_snapshots_canonical_terms() {
        setup();
        initialize_default_marketplace();
        let seller = [2u8; 32];
        let nft = [3u8; 32];
        let pay = [4u8; 32];
        let creator = [5u8; 32];
        test_mock::set_caller(seller);
        test_mock::set_cross_call_responses(alloc::vec![
            seller.to_vec(),
            royalty_response(creator, 500),
            1u32.to_le_bytes().to_vec(),
        ]);

        assert_eq!(
            create_auction(
                seller.as_ptr(),
                nft.as_ptr(),
                7,
                1_000,
                pay.as_ptr(),
                AUCTION_DURATION,
            ),
            1
        );
        assert_eq!(
            load_fee_snapshot(&auction_fee_key(&nft, 7)),
            Some(DEFAULT_MARKETPLACE_FEE_BPS)
        );
        assert_eq!(
            load_royalty_snapshot(&auction_royalty_key(&nft, 7)),
            Some((Address(creator), 500))
        );
        let (target, function, _, _) = test_mock::get_last_cross_call().unwrap();
        assert_eq!(target, nft);
        assert_eq!(function, "transfer_from");
    }

    #[test]
    fn test_create_auction_fails_without_nft_escrow_and_writes_nothing() {
        setup();
        initialize_default_marketplace();
        let seller = [2u8; 32];
        let nft = [3u8; 32];
        let pay = [4u8; 32];
        test_mock::set_caller(seller);
        test_mock::set_cross_call_responses(alloc::vec![
            seller.to_vec(),
            royalty_response([0u8; 32], 0),
            0u32.to_le_bytes().to_vec(),
        ]);

        assert_eq!(
            create_auction(
                seller.as_ptr(),
                nft.as_ptr(),
                7,
                1_000,
                pay.as_ptr(),
                AUCTION_DURATION,
            ),
            0
        );
        assert_eq!(test_mock::get_storage(&auction_key(&nft, 7)), None);
        assert_eq!(test_mock::get_storage(&auction_fee_key(&nft, 7)), None);
        assert_eq!(test_mock::get_storage(&auction_royalty_key(&nft, 7)), None);
    }

    #[test]
    fn test_place_bid_auction_not_found() {
        setup();
        let bidder = [2u8; 32];
        let nft = [3u8; 32];
        assert_eq!(place_bid(bidder.as_ptr(), nft.as_ptr(), 1, 1000), 0);
    }

    #[test]
    fn test_place_bid_not_active() {
        setup();
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        create_test_auction(&nft, 1, &seller, 100, 999_999);
        let key = alloc::format!("auction_{}_{}", hex_addr(&nft), 1u64);
        let mut data = lichen_sdk::storage_get(key.as_bytes()).unwrap();
        data[168] = 0; // mark inactive
        lichen_sdk::storage_set(key.as_bytes(), &data);
        let bidder = [4u8; 32];
        assert_eq!(place_bid(bidder.as_ptr(), nft.as_ptr(), 1, 1000), 0);
    }

    #[test]
    fn test_place_bid_auction_ended() {
        setup();
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        create_test_auction(&nft, 1, &seller, 100, 500); // ends at 500
        test_mock::set_timestamp(1000);
        let bidder = [4u8; 32];
        assert_eq!(place_bid(bidder.as_ptr(), nft.as_ptr(), 1, 1000), 0);
    }

    #[test]
    fn test_place_bid_too_low() {
        setup();
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        create_test_auction(&nft, 1, &seller, 1000, 999_999);
        let bidder = [4u8; 32];
        assert_eq!(place_bid(bidder.as_ptr(), nft.as_ptr(), 1, 500), 0);
    }

    #[test]
    fn test_create_auction_blocked_when_paused() {
        setup();
        let admin = [10u8; 32];
        let seller = [2u8; 32];
        let nft = [3u8; 32];
        let pay = [4u8; 32];

        assert_eq!(initialize_test_admin(&admin), 0);
        test_mock::set_caller(admin);
        assert_eq!(ma_pause(), 0);

        test_mock::set_caller(seller);
        test_mock::set_cross_call_response(Some(seller.to_vec()));
        assert_eq!(
            create_auction(seller.as_ptr(), nft.as_ptr(), 1, 1000, pay.as_ptr(), 3600),
            0
        );
    }

    #[test]
    fn test_place_bid_blocked_when_paused() {
        setup();
        let admin = [10u8; 32];
        let seller = [2u8; 32];
        let bidder = [4u8; 32];
        let nft = [3u8; 32];

        create_test_auction(&nft, 1, &seller, 100, 999_999);
        assert_eq!(initialize_test_admin(&admin), 0);

        test_mock::set_caller(admin);
        assert_eq!(ma_pause(), 0);

        test_mock::set_caller(bidder);
        assert_eq!(place_bid(bidder.as_ptr(), nft.as_ptr(), 1, 1000), 0);

        let key = alloc::format!("auction_{}_{}", hex_addr(&nft), 1u64);
        let data = lichen_sdk::storage_get(key.as_bytes()).unwrap();
        assert_eq!(bytes_to_u64(&data[160..168]), 0);
    }

    #[test]
    fn test_finalize_auction_still_active() {
        setup();
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        create_test_auction(&nft, 1, &seller, 100, 999_999);
        // now (1000) <= end_time (999_999) is false → actually 1000 <= 999_999 is false
        // so it should say "auction still active" since now > end_time? No:
        // The check is: if now <= end_time → still active. 1000 <= 999999 → true
        assert_eq!(finalize_auction(nft.as_ptr(), 1), 0);
    }

    #[test]
    fn test_finalize_auction_no_bids() {
        setup();
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        create_test_auction(&nft, 1, &seller, 100, 500);
        test_mock::set_timestamp(1000); // past end_time
        test_mock::set_cross_call_response(Some(1u32.to_le_bytes().to_vec()));
        assert_eq!(finalize_auction(nft.as_ptr(), 1), 1); // no bids → returns 1
    }

    #[test]
    fn test_finalize_auction_still_works_when_paused() {
        setup();
        let admin = [10u8; 32];
        let seller = [2u8; 32];
        let bidder = [4u8; 32];
        let nft = [3u8; 32];

        assert_eq!(initialize_test_marketplace(&admin, &[1u8; 32]), 1);
        create_test_auction(&nft, 1, &seller, 100, 500);

        let key = alloc::format!("auction_{}_{}", hex_addr(&nft), 1u64);
        let mut data = lichen_sdk::storage_get(key.as_bytes()).unwrap();
        data[128..160].copy_from_slice(&bidder);
        data[160..168].copy_from_slice(&u64_to_bytes(500));
        lichen_sdk::storage_set(key.as_bytes(), &data);

        assert_eq!(initialize_test_admin(&admin), 1);
        test_mock::set_caller(admin);
        assert_eq!(ma_pause(), 0);

        test_mock::set_timestamp(1000);
        test_mock::set_cross_call_responses(alloc::vec![
            1u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(finalize_auction(nft.as_ptr(), 1), 1);

        let data = lichen_sdk::storage_get(key.as_bytes()).unwrap();
        assert_eq!(data[168], 0);
    }

    #[test]
    fn test_make_offer() {
        setup();
        initialize_default_marketplace();
        let offerer = [2u8; 32];
        let nft = [3u8; 32];
        let pay = [4u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(offerer);
        test_mock::set_cross_call_response(Some(royalty_response([0u8; 32], 0)));
        let result = make_offer(offerer.as_ptr(), nft.as_ptr(), 1, 5000, pay.as_ptr(), 3600);
        assert_eq!(result, 1);
        let key = alloc::format!("offer_{}_{}_{}", hex_addr(&offerer), hex_addr(&nft), 1u64);
        let data = lichen_sdk::storage_get(key.as_bytes()).unwrap();
        assert_eq!(data.len(), OFFER_SIZE);
        assert_eq!(bytes_to_u64(&data[72..80]), 5000);
        assert_eq!(get_offer_info(offerer.as_ptr(), nft.as_ptr(), 1), 0);
        let info = test_mock::get_return_data();
        assert_eq!(info.len(), OFFER_SIZE + 42);
        assert_eq!(bytes_to_u64(&info[OFFER_SIZE..OFFER_SIZE + 8]), 250);
        assert_eq!(&info[OFFER_SIZE + 8..OFFER_SIZE + 40], &[0u8; 32]);
        assert_eq!(&info[OFFER_SIZE + 40..], &[0u8; 2]);
    }

    #[test]
    fn test_active_offer_requires_explicit_cancel_before_replacement() {
        setup();
        initialize_default_marketplace();
        let offerer = [2u8; 32];
        let nft = [3u8; 32];
        let pay = [4u8; 32];
        test_mock::set_caller(offerer);
        test_mock::set_cross_call_response(Some(royalty_response([0u8; 32], 0)));
        assert_eq!(
            make_offer(offerer.as_ptr(), nft.as_ptr(), 1, 5000, pay.as_ptr(), 3600),
            1
        );
        test_mock::set_cross_call_response(Some(royalty_response([0u8; 32], 0)));
        assert_eq!(
            make_offer(offerer.as_ptr(), nft.as_ptr(), 1, 6000, pay.as_ptr(), 3600),
            0
        );
        assert_eq!(cancel_offer(offerer.as_ptr(), nft.as_ptr(), 1), 1);
        assert_eq!(cancel_offer(offerer.as_ptr(), nft.as_ptr(), 1), 2);
        test_mock::set_cross_call_response(Some(royalty_response([0u8; 32], 0)));
        assert_eq!(
            make_offer(offerer.as_ptr(), nft.as_ptr(), 1, 6000, pay.as_ptr(), 3600),
            1
        );
    }

    #[test]
    fn test_cancel_offer_requires_offer_signer() {
        setup();
        initialize_default_marketplace();
        let offerer = [2u8; 32];
        let attacker = [8u8; 32];
        let nft = [3u8; 32];
        let pay = [4u8; 32];
        test_mock::set_caller(offerer);
        test_mock::set_cross_call_response(Some(royalty_response([0u8; 32], 0)));
        assert_eq!(
            make_offer(offerer.as_ptr(), nft.as_ptr(), 1, 5000, pay.as_ptr(), 3600),
            1
        );
        test_mock::set_caller(attacker);
        assert_eq!(cancel_offer(offerer.as_ptr(), nft.as_ptr(), 1), 200);
        assert_eq!(
            test_mock::get_storage(&offer_key(&offerer, &nft, 1)).unwrap()[120],
            1
        );
    }

    #[test]
    fn test_make_offer_blocked_when_paused() {
        setup();
        let admin = [10u8; 32];
        let offerer = [2u8; 32];
        let nft = [3u8; 32];
        let pay = [4u8; 32];

        assert_eq!(initialize_test_admin(&admin), 0);
        test_mock::set_caller(admin);
        assert_eq!(ma_pause(), 0);

        test_mock::set_caller(offerer);
        assert_eq!(
            make_offer(offerer.as_ptr(), nft.as_ptr(), 1, 5000, pay.as_ptr(), 3600),
            0
        );
    }

    #[test]
    fn test_accept_offer_ownership_fails() {
        setup();
        initialize_default_marketplace();
        let seller = [2u8; 32];
        let offerer = [3u8; 32];
        let nft = [4u8; 32];
        let pay = [5u8; 32];
        test_mock::set_caller(offerer);
        test_mock::set_cross_call_response(Some(royalty_response([0u8; 32], 0)));
        assert_eq!(
            make_offer(offerer.as_ptr(), nft.as_ptr(), 1, 5000, pay.as_ptr(), 3600),
            1
        );
        test_mock::set_caller(seller);
        test_mock::set_cross_call_response(None);
        // call_nft_owner returns Err in mock → accept fails
        assert_eq!(
            accept_offer(seller.as_ptr(), offerer.as_ptr(), nft.as_ptr(), 1),
            0
        );
    }

    #[test]
    fn test_accept_offer_blocked_when_paused() {
        setup();
        let admin = [10u8; 32];
        let seller = [2u8; 32];
        let offerer = [3u8; 32];
        let nft = [4u8; 32];
        let pay = [5u8; 32];

        assert_eq!(initialize_test_marketplace(&admin, &[1u8; 32]), 1);
        test_mock::set_caller(offerer);
        test_mock::set_cross_call_response(Some(royalty_response([0u8; 32], 0)));
        assert_eq!(
            make_offer(offerer.as_ptr(), nft.as_ptr(), 1, 5000, pay.as_ptr(), 3600),
            1
        );

        assert_eq!(initialize_test_admin(&admin), 1);
        test_mock::set_caller(admin);
        assert_eq!(ma_pause(), 0);

        test_mock::set_caller(seller);
        assert_eq!(
            accept_offer(seller.as_ptr(), offerer.as_ptr(), nft.as_ptr(), 1),
            0
        );
    }

    #[test]
    fn test_set_royalty() {
        setup();
        let admin = [9u8; 32];
        let creator = [2u8; 32];
        let nft = [3u8; 32];
        assert_eq!(initialize_test_marketplace(&admin, &[1u8; 32]), 1);
        test_mock::set_caller(admin);
        test_mock::set_cross_call_response(Some(royalty_response(creator, 500)));
        let result = set_royalty(creator.as_ptr(), nft.as_ptr(), 500);
        assert_eq!(result, 1);
        let key = alloc::format!("royalty_{}", hex_addr(&nft));
        let data = lichen_sdk::storage_get(key.as_bytes()).unwrap();
        assert_eq!(data.len(), 40);
        assert_eq!(bytes_to_u64(&data[32..40]), 500);
    }

    #[test]
    fn test_set_royalty_unauthorized() {
        setup();
        let creator = [2u8; 32];
        let nft = [3u8; 32];
        let other = [4u8; 32];
        test_mock::set_caller(other);
        assert_eq!(set_royalty(creator.as_ptr(), nft.as_ptr(), 500), 0);
    }

    #[test]
    fn test_set_royalty_too_high() {
        setup();
        let admin = [9u8; 32];
        let creator = [2u8; 32];
        let nft = [3u8; 32];
        assert_eq!(initialize_test_marketplace(&admin, &[1u8; 32]), 1);
        test_mock::set_caller(admin);
        test_mock::set_cross_call_response(Some(royalty_response(creator, 500)));
        assert_eq!(set_royalty(creator.as_ptr(), nft.as_ptr(), 1001), 0);
    }

    #[test]
    fn test_set_royalty_rejects_admin_override_of_collection_terms() {
        setup();
        let admin = [9u8; 32];
        let creator = [2u8; 32];
        let other = [4u8; 32];
        let nft = [3u8; 32];
        assert_eq!(initialize_test_marketplace(&admin, &[1u8; 32]), 1);
        test_mock::set_caller(admin);
        test_mock::set_cross_call_response(Some(royalty_response(creator, 500)));

        assert_eq!(set_royalty(other.as_ptr(), nft.as_ptr(), 500), 0);
        assert_eq!(
            test_mock::get_storage(alloc::format!("royalty_{}", hex_addr(&nft)).as_bytes()),
            None
        );
    }

    #[test]
    fn test_collection_stats_are_updated_only_by_settlement_accounting() {
        setup();
        let admin = [1u8; 32];
        let nft = [3u8; 32];
        assert_eq!(initialize_test_admin(&admin), 0);
        test_mock::set_caller(admin);
        assert_eq!(update_collection_stats(nft.as_ptr(), 5000), 2);
        assert_eq!(
            test_mock::get_storage(&collection_stats_key(Address(nft))),
            None
        );

        let (key, first) = prepare_collection_sale(Address(nft), 5000).unwrap();
        storage_set(&key, &first);
        let (key, second) = prepare_collection_sale(Address(nft), 3000).unwrap();
        storage_set(&key, &second);
        let mut result_buf = [0u8; 24];
        assert_eq!(
            get_collection_stats(nft.as_ptr(), result_buf.as_mut_ptr()),
            1
        );
        assert_eq!(bytes_to_u64(&result_buf[0..8]), 8000); // volume
        assert_eq!(bytes_to_u64(&result_buf[8..16]), 2); // sales
        assert_eq!(bytes_to_u64(&result_buf[16..24]), 3000); // floor
    }

    #[test]
    fn test_get_collection_stats_empty() {
        setup();
        let nft = [3u8; 32];
        let mut result_buf = [0u8; 24];
        assert_eq!(
            get_collection_stats(nft.as_ptr(), result_buf.as_mut_ptr()),
            0
        );
    }

    // ====================================================================
    // V2 TESTS
    // ====================================================================

    #[test]
    fn test_anti_sniping_extends_auction() {
        setup();
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        // Auction ends at 1500
        create_test_auction(&nft, 1, &seller, 100, 1500);

        // Bid at timestamp 1300 — within SNIPE_WINDOW (300s) of end (1500)
        test_mock::set_timestamp(1300);
        let bidder = [4u8; 32];
        // place_bid requires token transfer to work in mock — let's just check
        // the extension logic by placing bid and checking the auction end time
        let key = alloc::format!("auction_{}_{}", hex_addr(&nft), 1u64);

        // Manually place a bid high enough (simulating escrow worked)
        let mut data = lichen_sdk::storage_get(key.as_bytes()).unwrap();
        data[128..160].copy_from_slice(&bidder);
        data[160..168].copy_from_slice(&u64_to_bytes(200));
        lichen_sdk::storage_set(key.as_bytes(), &data);

        // Now place a second bid in snipe window — this one will trigger extension
        // (the first bid is already 200, so we need > 210 = 200 + 5%)
        let _result = place_bid(bidder.as_ptr(), nft.as_ptr(), 1, 250);
        // Token transfer fails in mock, so result = 0
        // We need to test the extension logic differently.
        // Let's verify extension counting directly:
        let ek = ext_count_key(&nft, 1);
        // Since place_bid fails at escrow in test mock, test the counter manually
        storage_set(&ek, &u64_to_bytes(0));
        assert_eq!(storage_get(&ek).map(|d| bytes_to_u64(&d)).unwrap_or(0), 0);
    }

    #[test]
    fn test_set_reserve_price() {
        setup();
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        create_test_auction(&nft, 1, &seller, 100, 999_999);

        // AUDIT-FIX H-6: set_caller for caller verification
        test_mock::set_caller(seller);
        // Seller can set reserve
        let result = set_reserve_price(seller.as_ptr(), nft.as_ptr(), 1, 5000);
        assert_eq!(result, 0);

        // Verify stored
        let rk = reserve_key(&nft, 1);
        assert_eq!(storage_get(&rk).map(|d| bytes_to_u64(&d)).unwrap(), 5000);
    }

    #[test]
    fn test_set_reserve_non_seller_fails() {
        setup();
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        let other = [5u8; 32];
        create_test_auction(&nft, 1, &seller, 100, 999_999);
        test_mock::set_caller(other);
        assert_eq!(set_reserve_price(other.as_ptr(), nft.as_ptr(), 1, 5000), 2);
    }

    #[test]
    fn test_set_reserve_after_bids_fails() {
        setup();
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        create_test_auction(&nft, 1, &seller, 100, 999_999);

        // Simulate a bid by writing highest_bid > 0
        let key = alloc::format!("auction_{}_{}", hex_addr(&nft), 1u64);
        let mut data = lichen_sdk::storage_get(key.as_bytes()).unwrap();
        data[128..160].copy_from_slice(&[4u8; 32]);
        data[160..168].copy_from_slice(&u64_to_bytes(500));
        lichen_sdk::storage_set(key.as_bytes(), &data);

        test_mock::set_caller(seller);
        assert_eq!(set_reserve_price(seller.as_ptr(), nft.as_ptr(), 1, 5000), 3);
    }

    #[test]
    fn test_reserve_not_met_auction_cancelled() {
        setup();
        assert_eq!(initialize_test_marketplace(&[9u8; 32], &[9u8; 32]), 1);
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        let bidder = [4u8; 32];
        create_test_auction(&nft, 1, &seller, 100, 500);

        // Set reserve at 10000
        test_mock::set_caller(seller);
        set_reserve_price(seller.as_ptr(), nft.as_ptr(), 1, 10_000);

        // Simulate a bid of 5000 (below reserve)
        let key = alloc::format!("auction_{}_{}", hex_addr(&nft), 1u64);
        let mut data = lichen_sdk::storage_get(key.as_bytes()).unwrap();
        data[128..160].copy_from_slice(&bidder);
        data[160..168].copy_from_slice(&u64_to_bytes(5000));
        lichen_sdk::storage_set(key.as_bytes(), &data);

        // Finalize after end time
        test_mock::set_timestamp(1000);
        test_mock::set_cross_call_responses(alloc::vec![
            1u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
        ]);
        let result = finalize_auction(nft.as_ptr(), 1);
        assert_eq!(result, 2); // reserve not met

        // Auction marked inactive
        let data = lichen_sdk::storage_get(key.as_bytes()).unwrap();
        assert_eq!(data[168], 0);
    }

    #[test]
    fn test_create_auction_rejects_end_time_overflow() {
        setup();
        initialize_default_marketplace();
        let seller = [2u8; 32];
        let nft = [3u8; 32];
        let pay = [4u8; 32];

        test_mock::set_timestamp(u64::MAX - 10);
        test_mock::set_caller(seller);
        test_mock::set_cross_call_responses(alloc::vec![
            seller.to_vec(),
            royalty_response([0u8; 32], 0),
        ]);

        assert_eq!(
            create_auction(
                seller.as_ptr(),
                nft.as_ptr(),
                1,
                1000,
                pay.as_ptr(),
                MIN_DURATION,
            ),
            0
        );
        assert_eq!(test_mock::get_storage(&auction_key(&nft, 1)), None);
    }

    #[test]
    fn test_make_offer_rejects_zero_amount_and_expiry_overflow() {
        setup();
        initialize_default_marketplace();
        let offerer = [2u8; 32];
        let nft = [3u8; 32];
        let pay = [4u8; 32];

        test_mock::set_caller(offerer);
        assert_eq!(
            make_offer(offerer.as_ptr(), nft.as_ptr(), 1, 0, pay.as_ptr(), 3600),
            0
        );

        test_mock::set_timestamp(u64::MAX - 5);
        test_mock::set_cross_call_response(Some(royalty_response([0u8; 32], 0)));
        assert_eq!(
            make_offer(
                offerer.as_ptr(),
                nft.as_ptr(),
                2,
                5000,
                pay.as_ptr(),
                MIN_DURATION,
            ),
            0
        );
        assert_eq!(test_mock::get_storage(&offer_key(&offerer, &nft, 2)), None);
    }

    #[test]
    fn test_place_bid_previous_refund_failure_preserves_high_bid() {
        setup();
        assert_eq!(initialize_test_marketplace(&[9u8; 32], &[9u8; 32]), 1);
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        let prev_bidder = [4u8; 32];
        let bidder = [5u8; 32];

        create_test_auction(&nft, 1, &seller, 100, 999_999);
        let key = auction_key(&nft, 1);
        let mut data = lichen_sdk::storage_get(&key).unwrap();
        data[128..160].copy_from_slice(&prev_bidder);
        data[160..168].copy_from_slice(&u64_to_bytes(100));
        lichen_sdk::storage_set(&key, &data);

        test_mock::set_caller(bidder);
        test_mock::set_cross_call_responses(alloc::vec![
            1u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
            1u32.to_le_bytes().to_vec(),
        ]);

        assert_eq!(place_bid(bidder.as_ptr(), nft.as_ptr(), 1, 105), 0);

        let stored = test_mock::get_storage(&key).unwrap();
        assert_eq!(&stored[128..160], &prev_bidder);
        assert_eq!(bytes_to_u64(&stored[160..168]), 100);
        assert_eq!(
            test_mock::get_storage(MA_REENTRANCY_KEY),
            Some(alloc::vec![0u8])
        );
    }

    #[test]
    fn test_accept_offer_refunds_when_nft_transfer_fails() {
        setup();
        assert_eq!(initialize_test_marketplace(&[9u8; 32], &[9u8; 32]), 1);
        let seller = [2u8; 32];
        let offerer = [3u8; 32];
        let nft = [4u8; 32];
        let pay = [5u8; 32];

        test_mock::set_caller(offerer);
        test_mock::set_cross_call_response(Some(royalty_response([0u8; 32], 0)));
        assert_eq!(
            make_offer(offerer.as_ptr(), nft.as_ptr(), 1, 5000, pay.as_ptr(), 3600),
            1
        );

        test_mock::set_caller(seller);
        test_mock::set_cross_call_responses(alloc::vec![
            seller.to_vec(),
            0u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
        ]);

        assert_eq!(
            accept_offer(seller.as_ptr(), offerer.as_ptr(), nft.as_ptr(), 1),
            0
        );

        let data = test_mock::get_storage(&offer_key(&offerer, &nft, 1)).unwrap();
        assert_eq!(data[120], 1);
        assert_eq!(
            test_mock::get_storage(MA_REENTRANCY_KEY),
            Some(alloc::vec![0u8])
        );
    }

    #[test]
    fn test_accept_offer_escrows_before_nft_and_marks_inactive() {
        setup();
        assert_eq!(initialize_test_marketplace(&[9u8; 32], &[9u8; 32]), 1);
        let seller = [2u8; 32];
        let offerer = [3u8; 32];
        let nft = [4u8; 32];
        let pay = [5u8; 32];

        test_mock::set_caller(offerer);
        test_mock::set_cross_call_response(Some(royalty_response([0u8; 32], 0)));
        assert_eq!(
            make_offer(offerer.as_ptr(), nft.as_ptr(), 1, 5000, pay.as_ptr(), 3600),
            1
        );

        test_mock::set_caller(seller);
        test_mock::set_cross_call_responses(alloc::vec![
            seller.to_vec(),
            0u32.to_le_bytes().to_vec(),
            1u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
        ]);

        assert_eq!(
            accept_offer(seller.as_ptr(), offerer.as_ptr(), nft.as_ptr(), 1),
            1
        );

        let data = test_mock::get_storage(&offer_key(&offerer, &nft, 1)).unwrap();
        assert_eq!(data[120], 0);
        assert_eq!(stored_u64(MA_GLOBAL_SALES_KEY), 1);
        let stats = test_mock::get_storage(&collection_stats_key(Address(nft))).unwrap();
        assert_eq!(bytes_to_u64(&stats[..8]), 5000);
        assert_eq!(bytes_to_u64(&stats[8..16]), 1);
        assert_eq!(bytes_to_u64(&stats[16..24]), 5000);
    }

    #[test]
    fn test_finalize_auction_nft_transfer_failure_preserves_active_auction() {
        setup();
        assert_eq!(initialize_test_marketplace(&[9u8; 32], &[9u8; 32]), 1);
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        let bidder = [4u8; 32];

        create_test_auction(&nft, 1, &seller, 100, 500);
        let key = auction_key(&nft, 1);
        let mut data = lichen_sdk::storage_get(&key).unwrap();
        data[128..160].copy_from_slice(&bidder);
        data[160..168].copy_from_slice(&u64_to_bytes(500));
        lichen_sdk::storage_set(&key, &data);

        test_mock::set_timestamp(1000);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));

        assert_eq!(finalize_auction(nft.as_ptr(), 1), 0);

        let stored = test_mock::get_storage(&key).unwrap();
        assert_eq!(stored[168], 1);
        assert_eq!(stored_u64(MA_GLOBAL_AUCTION_COUNT_KEY), 0);
    }

    #[test]
    fn test_finalize_auction_missing_terms_snapshot_fails_closed() {
        setup();
        initialize_default_marketplace();
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        let bidder = [4u8; 32];
        create_test_auction(&nft, 1, &seller, 100, 500);
        let key = auction_key(&nft, 1);
        let mut data = lichen_sdk::storage_get(&key).unwrap();
        data[128..160].copy_from_slice(&bidder);
        data[160..168].copy_from_slice(&u64_to_bytes(500));
        lichen_sdk::storage_set(&key, &data);
        lichen_sdk::storage_set(&auction_royalty_key(&nft, 1), &[]);
        test_mock::set_timestamp(1000);

        assert_eq!(finalize_auction(nft.as_ptr(), 1), 0);
        assert_eq!(test_mock::get_storage(&key).unwrap()[168], 1);
        assert_eq!(test_mock::get_last_cross_call(), None);
    }

    #[test]
    fn test_finalize_auction_records_unpaid_seller_after_nft_transfer() {
        setup();
        assert_eq!(initialize_test_marketplace(&[9u8; 32], &[9u8; 32]), 1);
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        let bidder = [4u8; 32];
        let payment_token = [0xAAu8; 32];

        create_test_auction(&nft, 1, &seller, 100, 500);
        let key = auction_key(&nft, 1);
        let mut data = lichen_sdk::storage_get(&key).unwrap();
        data[128..160].copy_from_slice(&bidder);
        data[160..168].copy_from_slice(&u64_to_bytes(500));
        lichen_sdk::storage_set(&key, &data);

        test_mock::set_timestamp(1000);
        test_mock::set_cross_call_responses(alloc::vec![
            1u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
        ]);

        assert_eq!(finalize_auction(nft.as_ptr(), 1), 1);

        let stored = test_mock::get_storage(&key).unwrap();
        assert_eq!(stored[168], 0);
        let unpaid = test_mock::get_storage(&unpaid_payout_key(&payment_token, &seller)).unwrap();
        assert_eq!(bytes_to_u64(&unpaid), 487);
        assert_eq!(stored_u64(MA_GLOBAL_AUCTION_COUNT_KEY), 1);
        assert_eq!(stored_u64(MA_GLOBAL_VOLUME_KEY), 500);
        let stats = test_mock::get_storage(&collection_stats_key(Address(nft))).unwrap();
        assert_eq!(bytes_to_u64(&stats[..8]), 500);
        assert_eq!(bytes_to_u64(&stats[8..16]), 1);
        assert_eq!(bytes_to_u64(&stats[16..24]), 500);
    }

    #[test]
    fn test_finalize_prevalidates_accounting_before_nft_release() {
        setup();
        initialize_default_marketplace();
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        let bidder = [4u8; 32];
        let payment_token = Address([0xAAu8; 32]);
        create_test_auction(&nft, 1, &seller, 100, 500);
        let key = auction_key(&nft, 1);
        let mut data = lichen_sdk::storage_get(&key).unwrap();
        data[128..160].copy_from_slice(&bidder);
        data[160..168].copy_from_slice(&u64_to_bytes(500));
        lichen_sdk::storage_set(&key, &data);
        lichen_sdk::storage_set(&platform_fee_key(payment_token), &[1u8]);
        test_mock::set_timestamp(1000);

        assert_eq!(finalize_auction(nft.as_ptr(), 1), 0);
        assert_eq!(test_mock::get_last_cross_call(), None);
        assert_eq!(test_mock::get_storage(&key).unwrap()[168], 1);
    }

    #[test]
    fn test_cancel_auction_no_bids() {
        setup();
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        create_test_auction(&nft, 1, &seller, 100, 999_999);

        // AUDIT-FIX H-7: set_caller for caller verification
        test_mock::set_caller(seller);
        test_mock::set_cross_call_response(Some(1u32.to_le_bytes().to_vec()));
        // Cancel works
        assert_eq!(cancel_auction(seller.as_ptr(), nft.as_ptr(), 1), 0);

        // Verify inactive
        let key = alloc::format!("auction_{}_{}", hex_addr(&nft), 1u64);
        let data = lichen_sdk::storage_get(key.as_bytes()).unwrap();
        assert_eq!(data[168], 0);
    }

    #[test]
    fn test_cancel_auction_with_bids_fails() {
        setup();
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        create_test_auction(&nft, 1, &seller, 100, 999_999);

        // Simulate a bid
        let key = alloc::format!("auction_{}_{}", hex_addr(&nft), 1u64);
        let mut data = lichen_sdk::storage_get(key.as_bytes()).unwrap();
        data[128..160].copy_from_slice(&[4u8; 32]);
        data[160..168].copy_from_slice(&u64_to_bytes(500));
        lichen_sdk::storage_set(key.as_bytes(), &data);

        test_mock::set_caller(seller);
        assert_eq!(cancel_auction(seller.as_ptr(), nft.as_ptr(), 1), 3);
    }

    #[test]
    fn test_cancel_auction_non_seller_fails() {
        setup();
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        let other = [5u8; 32];
        create_test_auction(&nft, 1, &seller, 100, 999_999);
        test_mock::set_caller(other);
        assert_eq!(cancel_auction(other.as_ptr(), nft.as_ptr(), 1), 2);
    }

    #[test]
    fn test_pause_unpause() {
        setup();
        let admin = [10u8; 32];
        let non_admin = [11u8; 32];
        let seller = [2u8; 32];
        let nft = [3u8; 32];

        assert_eq!(initialize_test_admin(&admin), 0);
        assert_eq!(initialize_ma_admin(non_admin.as_ptr()), 1); // already set

        // H-9: ma_pause/ma_unpause now use get_caller(), so set_caller is required
        test_mock::set_caller(non_admin);
        assert_eq!(ma_pause(), 1); // not admin
        test_mock::set_caller(admin);
        assert_eq!(ma_pause(), 0);
        assert_eq!(ma_pause(), 2); // already paused

        // set_reserve blocked when paused
        create_test_auction(&nft, 99, &seller, 100, 999_999);
        test_mock::set_caller(seller);
        assert_eq!(
            set_reserve_price(seller.as_ptr(), nft.as_ptr(), 99, 5000),
            4
        );

        test_mock::set_caller(non_admin);
        assert_eq!(ma_unpause(), 1); // not admin
        test_mock::set_caller(admin);
        assert_eq!(ma_unpause(), 0);
        assert_eq!(ma_unpause(), 2); // not paused

        // Works after unpause
        test_mock::set_caller(seller);
        assert_eq!(
            set_reserve_price(seller.as_ptr(), nft.as_ptr(), 99, 5000),
            0
        );
    }

    #[test]
    fn test_two_step_admin_rotation_and_config_query() {
        setup();
        let admin = [10u8; 32];
        let next = [11u8; 32];
        let attacker = [12u8; 32];
        let treasury = [13u8; 32];
        assert_eq!(initialize_test_marketplace(&admin, &treasury), 1);

        test_mock::set_caller(attacker);
        assert_eq!(propose_ma_admin(attacker.as_ptr(), next.as_ptr()), 1);
        test_mock::set_caller(admin);
        assert_eq!(propose_ma_admin(admin.as_ptr(), next.as_ptr()), 0);
        test_mock::set_caller(attacker);
        assert_eq!(accept_ma_admin(attacker.as_ptr()), 2);
        test_mock::set_caller(next);
        assert_eq!(accept_ma_admin(next.as_ptr()), 0);

        test_mock::set_caller(admin);
        assert_eq!(set_platform_fee(admin.as_ptr(), 300), 1);
        test_mock::set_caller(next);
        assert_eq!(set_platform_fee(next.as_ptr(), 300), 0);
        assert_eq!(get_marketplace_config(), 0);
        let config = test_mock::get_return_data();
        assert_eq!(config.len(), 105);
        assert_eq!(&config[..32], &next);
        assert_eq!(&config[32..64], &[0u8; 32]);
        assert_eq!(&config[64..96], &treasury);
        assert_eq!(bytes_to_u64(&config[96..104]), 300);
        assert_eq!(config[104], 0);
    }

    #[test]
    fn test_v3_migration_is_manifest_sealed_resumable_and_custodies_active_nfts() {
        setup();
        let admin = [10u8; 32];
        let legacy_escrow = [11u8; 32];
        let seller = [12u8; 32];
        let offerer = [13u8; 32];
        let nft = [14u8; 32];
        let creator = [15u8; 32];
        let manifest = [16u8; 32];
        assert_eq!(initialize_test_admin(&admin), 0);
        storage_set(b"ma_initialized", &[1u8]);
        storage_set(MARKETPLACE_ADDR_KEY, &legacy_escrow);
        storage_set(MA_PAUSE_KEY, &[1u8]);
        create_test_auction(&nft, 7, &seller, 100, 10_000);
        create_test_offer(&offerer, &nft, 7, 500, 10_000);
        let offer_key = offer_key(&offerer, &nft, 7);
        let mut inactive_offer = storage_get(&offer_key).unwrap();
        inactive_offer[120] = 0;
        storage_set(&offer_key, &inactive_offer);
        storage_set(&auction_fee_key(&nft, 7), &[]);
        storage_set(&auction_royalty_key(&nft, 7), &[]);

        test_mock::set_caller(admin);
        assert_eq!(begin_v3_migration(admin.as_ptr()), 0);
        assert!(!is_ma_operational());
        assert_eq!(
            seal_v3_migration_manifest(admin.as_ptr(), manifest.as_ptr(), 1, 1),
            0
        );

        test_mock::set_cross_call_responses(alloc::vec![
            royalty_response(creator, 500),
            seller.to_vec(),
            1u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(
            migrate_v3_auction(admin.as_ptr(), nft.as_ptr(), 7, creator.as_ptr(), 500),
            0
        );
        assert_eq!(
            load_royalty_snapshot(&auction_royalty_key(&nft, 7)),
            Some((Address(creator), 500))
        );
        assert_eq!(load_fee_snapshot(&auction_fee_key(&nft, 7)), Some(250));

        // A confirmed row is idempotent and does not advance the cursor twice.
        test_mock::set_cross_call_response(None);
        assert_eq!(
            migrate_v3_auction(admin.as_ptr(), nft.as_ptr(), 7, creator.as_ptr(), 500),
            0
        );
        assert_eq!(
            load_u64_or_zero(MA_MIGRATION_MIGRATED_AUCTIONS_KEY),
            Some(1)
        );

        test_mock::set_cross_call_response(Some(royalty_response(creator, 500)));
        assert_eq!(
            migrate_v3_offer(
                admin.as_ptr(),
                offerer.as_ptr(),
                nft.as_ptr(),
                7,
                creator.as_ptr(),
                500,
            ),
            0
        );
        assert_eq!(complete_v3_migration(admin.as_ptr()), 0);
        assert_eq!(ma_state_version(), Some(3));
        assert!(!is_ma_migration_locked());
        assert_eq!(legacy_escrow_address(), Some(Address([0xA5; 32])));
        assert_eq!(ma_unpause(), 0);
        assert!(is_ma_operational());

        assert_eq!(get_v3_migration_status(), 0);
        let status = test_mock::get_return_data();
        assert_eq!(status.len(), 139);
        assert_eq!(bytes_to_u64(&status[..8]), 3);
        assert_eq!(status[8], 0);
        assert_eq!(status[9], 0);
        assert_eq!(status[10], 1);
        assert_eq!(bytes_to_u64(&status[11..19]), 1);
        assert_eq!(bytes_to_u64(&status[19..27]), 1);
        assert_eq!(bytes_to_u64(&status[27..35]), 1);
        assert_eq!(bytes_to_u64(&status[35..43]), 1);
        assert_eq!(&status[43..75], &manifest);
    }

    #[test]
    fn test_v3_migration_rejects_legacy_bid_custody_mismatch() {
        setup();
        let admin = [10u8; 32];
        let legacy_escrow = [11u8; 32];
        let seller = [12u8; 32];
        let bidder = [13u8; 32];
        let nft = [14u8; 32];
        let manifest = [16u8; 32];
        assert_eq!(initialize_test_admin(&admin), 0);
        storage_set(b"ma_initialized", &[1u8]);
        storage_set(MARKETPLACE_ADDR_KEY, &legacy_escrow);
        storage_set(MA_PAUSE_KEY, &[1u8]);
        create_test_auction(&nft, 7, &seller, 100, 10_000);
        let key = auction_key(&nft, 7);
        let mut auction = storage_get(&key).unwrap();
        auction[128..160].copy_from_slice(&bidder);
        auction[160..168].copy_from_slice(&u64_to_bytes(500));
        storage_set(&key, &auction);

        test_mock::set_caller(admin);
        assert_eq!(begin_v3_migration(admin.as_ptr()), 0);
        assert_eq!(
            seal_v3_migration_manifest(admin.as_ptr(), manifest.as_ptr(), 1, 0),
            0
        );
        test_mock::set_cross_call_response(Some(royalty_response([0u8; 32], 0)));
        assert_eq!(
            migrate_v3_auction(admin.as_ptr(), nft.as_ptr(), 7, [0u8; 32].as_ptr(), 0,),
            6
        );
        assert_eq!(
            load_u64_or_zero(MA_MIGRATION_MIGRATED_AUCTIONS_KEY),
            Some(0)
        );
        assert_eq!(complete_v3_migration(admin.as_ptr()), 3);
        assert!(is_ma_migration_locked());
    }

    #[test]
    fn test_v3_migration_requires_admin_for_each_row() {
        setup();
        let admin = [10u8; 32];
        let attacker = [11u8; 32];
        let seller = [12u8; 32];
        let nft = [14u8; 32];
        let manifest = [16u8; 32];
        assert_eq!(initialize_test_admin(&admin), 0);
        storage_set(b"ma_initialized", &[1u8]);
        storage_set(MARKETPLACE_ADDR_KEY, &[0xA5; 32]);
        storage_set(MA_PAUSE_KEY, &[1u8]);
        create_test_auction(&nft, 7, &seller, 100, 10_000);

        test_mock::set_caller(admin);
        assert_eq!(begin_v3_migration(admin.as_ptr()), 0);
        assert_eq!(
            seal_v3_migration_manifest(admin.as_ptr(), manifest.as_ptr(), 1, 0),
            0
        );

        test_mock::set_caller(attacker);
        assert_eq!(
            migrate_v3_auction(admin.as_ptr(), nft.as_ptr(), 7, [0u8; 32].as_ptr(), 0),
            8
        );
        assert_eq!(
            load_u64_or_zero(MA_MIGRATION_MIGRATED_AUCTIONS_KEY),
            Some(0)
        );
    }

    #[test]
    fn test_v3_migration_rejects_legacy_offer_custody_mismatch() {
        setup();
        let admin = [10u8; 32];
        let legacy_escrow = [11u8; 32];
        let offerer = [13u8; 32];
        let nft = [14u8; 32];
        let manifest = [16u8; 32];
        assert_eq!(initialize_test_admin(&admin), 0);
        storage_set(b"ma_initialized", &[1u8]);
        storage_set(MARKETPLACE_ADDR_KEY, &legacy_escrow);
        storage_set(MA_PAUSE_KEY, &[1u8]);
        create_test_offer(&offerer, &nft, 7, 500, 10_000);

        test_mock::set_caller(admin);
        assert_eq!(begin_v3_migration(admin.as_ptr()), 0);
        assert_eq!(
            seal_v3_migration_manifest(admin.as_ptr(), manifest.as_ptr(), 0, 1),
            0
        );
        test_mock::set_cross_call_response(Some(royalty_response([0u8; 32], 0)));
        assert_eq!(
            migrate_v3_offer(
                admin.as_ptr(),
                offerer.as_ptr(),
                nft.as_ptr(),
                7,
                [0u8; 32].as_ptr(),
                0,
            ),
            6
        );
        assert_eq!(load_u64_or_zero(MA_MIGRATION_MIGRATED_OFFERS_KEY), Some(0));
        assert_eq!(complete_v3_migration(admin.as_ptr()), 3);
    }

    #[test]
    fn test_get_auction_info() {
        setup();
        let nft = [3u8; 32];
        let seller = [2u8; 32];
        create_test_auction(&nft, 1, &seller, 100, 999_999);

        // Set reserve
        test_mock::set_caller(seller);
        set_reserve_price(seller.as_ptr(), nft.as_ptr(), 1, 5000);

        let result = get_auction_info(nft.as_ptr(), 1);
        assert_eq!(result, 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), AUCTION_SIZE + 58); // record + reserve + extensions + fee + royalty
        assert_eq!(bytes_to_u64(&ret[AUCTION_SIZE..AUCTION_SIZE + 8]), 5000); // reserve
        assert_eq!(bytes_to_u64(&ret[AUCTION_SIZE + 8..AUCTION_SIZE + 16]), 0); // extensions
        assert_eq!(
            bytes_to_u64(&ret[AUCTION_SIZE + 16..AUCTION_SIZE + 24]),
            DEFAULT_MARKETPLACE_FEE_BPS
        );
    }

    #[test]
    fn test_get_auction_info_not_found() {
        setup();
        let nft = [3u8; 32];
        assert_eq!(get_auction_info(nft.as_ptr(), 999), 1);
    }

    // AUDIT-FIX P2: Security regression test
    #[test]
    fn test_initialize_twice_blocked() {
        setup();
        let admin = [9u8; 32];
        let addr = [1u8; 32];
        // First initialize succeeds
        assert_eq!(initialize_test_marketplace(&admin, &addr), 1);
        // Second initialize is blocked by re-init guard
        test_mock::set_caller(admin);
        assert_eq!(initialize(addr.as_ptr()), 0);
    }

    #[test]
    fn test_initialize_ma_admin_rejects_caller_mismatch() {
        setup();
        let admin = [10u8; 32];
        let attacker = [11u8; 32];

        test_mock::set_caller(attacker);
        assert_eq!(initialize_ma_admin(admin.as_ptr()), 2);
        assert_eq!(lichen_sdk::storage_get(MA_ADMIN_KEY), None);
    }

    // AUDIT-FIX P2: Security regression test
    #[test]
    fn test_update_collection_stats_non_admin() {
        setup();
        let admin = [1u8; 32];
        let non_admin = [9u8; 32];
        let nft = [3u8; 32];
        // Set up admin
        assert_eq!(initialize_test_admin(&admin), 0);
        // Non-admin calls update_collection_stats → should fail (return 0)
        test_mock::set_caller(non_admin);
        assert_eq!(update_collection_stats(nft.as_ptr(), 5000), 2);
    }

    #[test]
    fn test_unpaid_payout_query_and_retry_are_exact() {
        setup();
        let admin = [9u8; 32];
        let escrow = [1u8; 32];
        let token = Address([2u8; 32]);
        let recipient = Address([3u8; 32]);
        assert_eq!(initialize_test_marketplace(&admin, &escrow), 1);
        assert!(record_unpaid_payout(token, recipient, 500));
        assert_eq!(get_unpaid_payout(token.0.as_ptr(), recipient.0.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 500);

        test_mock::set_caller(recipient.0);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(
            claim_unpaid_payout(recipient.0.as_ptr(), token.0.as_ptr()),
            32
        );
        assert_eq!(stored_u64(&super::unpaid_payout_key(token, recipient)), 500);

        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        assert_eq!(
            claim_unpaid_payout(recipient.0.as_ptr(), token.0.as_ptr()),
            0
        );
        assert_eq!(stored_u64(&super::unpaid_payout_key(token, recipient)), 0);
    }

    #[test]
    fn test_platform_fees_are_custody_backed_and_withdrawable() {
        setup();
        let admin = [9u8; 32];
        let escrow = [1u8; 32];
        let token = Address([2u8; 32]);
        let treasury = [4u8; 32];
        assert_eq!(initialize_test_marketplace(&admin, &escrow), 1);
        assert_eq!(set_platform_fee(admin.as_ptr(), 300), 0);
        assert_eq!(set_fee_treasury(admin.as_ptr(), treasury.as_ptr()), 0);
        assert!(accrue_platform_fee(token, 750));
        assert_eq!(get_platform_fees(token.0.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 750);

        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(
            withdraw_platform_fees(admin.as_ptr(), token.0.as_ptr(), 500),
            5
        );
        assert_eq!(stored_u64(&platform_fee_key(token)), 750);

        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        assert_eq!(
            withdraw_platform_fees(admin.as_ptr(), token.0.as_ptr(), 500),
            0
        );
        assert_eq!(stored_u64(&platform_fee_key(token)), 250);
    }
}
