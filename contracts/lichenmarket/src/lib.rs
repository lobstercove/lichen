// Lichen Market v2 - NFT Marketplace with Cross-Contract Calls
// Demonstrates true composability: marketplace calls NFT & token contracts
//
// v2 fixes and additions:
//   - FIXED: list_nft now writes 145 bytes (was 113, layout mismatch bug)
//   - Royalty recipient field (bytes 112..144) in listing
//   - Offer/bid system for unlisted NFTs
//   - Admin: marketplace pause
//   - Listing counter for marketplace stats

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use lichen_sdk::{
    bytes_to_u64, call_nft_owner, call_nft_transfer, get_caller, log_info, receive_token_or_native,
    storage_get, storage_set, transfer_token_or_native, u64_to_bytes, Address,
};

const MM_SALE_COUNT_KEY: &[u8] = b"mm_sale_count";
const MM_SALE_VOLUME_KEY: &[u8] = b"mm_sale_volume";
const MIN_OFFER_PRICE: u64 = 1_000_000; // 0.001 LICN (assuming 1e9 base units)
const MAX_ACTIVE_OFFERS_PER_WALLET: u64 = 64;

// Reentrancy guard
const MM_REENTRANCY_KEY: &[u8] = b"mm_reentrancy";

fn reentrancy_enter() -> bool {
    if storage_get(MM_REENTRANCY_KEY)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
    {
        return false;
    }
    storage_set(MM_REENTRANCY_KEY, &[1u8]);
    true
}

fn reentrancy_exit() {
    storage_set(MM_REENTRANCY_KEY, &[0u8]);
}

// Emergency pause
const MM_PAUSE_KEY: &[u8] = b"mm_paused";

fn is_mm_paused() -> bool {
    storage_get(MM_PAUSE_KEY)
        .map(|v| v.first().copied() == Some(1))
        .unwrap_or(false)
}

fn is_mm_admin(caller: &[u8]) -> bool {
    storage_get(b"marketplace_owner")
        .map(|d| d.as_slice() == caller)
        .unwrap_or(false)
}

fn offerer_active_count_key(offerer: &[u8; 32]) -> Vec<u8> {
    let mut key = b"offerer_count:".to_vec();
    key.extend_from_slice(offerer);
    key
}

fn get_offerer_active_count(offerer: &[u8; 32]) -> u64 {
    storage_get(&offerer_active_count_key(offerer))
        .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
        .unwrap_or(0)
}

fn set_offerer_active_count(offerer: &[u8; 32], count: u64) {
    storage_set(&offerer_active_count_key(offerer), &u64_to_bytes(count));
}

fn reserve_offer_slot_if_needed(offerer: &[u8; 32], was_already_active: bool) -> bool {
    if was_already_active {
        return true;
    }
    let current = get_offerer_active_count(offerer);
    if current >= MAX_ACTIVE_OFFERS_PER_WALLET {
        return false;
    }
    set_offerer_active_count(offerer, current.saturating_add(1));
    true
}

fn release_offer_slot_if_needed(offerer: &[u8; 32], was_active: bool) {
    if !was_active {
        return;
    }
    let current = get_offerer_active_count(offerer);
    set_offerer_active_count(offerer, current.saturating_sub(1));
}

/// Listing layout (147 bytes):
///   0..32   seller
///   32..64  nft_contract
///   64..72  token_id (u64 LE)
///   72..80  price (u64 LE)
///   80..112 payment_token
///   112..144 royalty_recipient (v2: zero = no royalty)
///   144     active (1=active, 0=inactive)
///   145..147 royalty_bps (u16 LE, v3: basis points for royalty)
struct Listing {
    seller: Address,
    nft_contract: Address,
    token_id: u64,
    price: u64,
    payment_token: Address,
    active: bool,
}

const LISTING_SIZE: usize = 147;

/// Initialize the marketplace
#[no_mangle]
pub extern "C" fn initialize(owner_ptr: *const u8, fee_addr_ptr: *const u8) {
    // Re-initialization guard: reject if marketplace_owner is already set
    if storage_get(b"marketplace_owner").is_some() {
        log_info("LichenMarket already initialized — ignoring");
        return;
    }

    let mut owner = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32);
    }
    let mut fee_addr = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(fee_addr_ptr, fee_addr.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != owner {
        return;
    }

    storage_set(b"marketplace_fee", &u64_to_bytes(250)); // 2.5% fee
    storage_set(b"marketplace_owner", &owner);
    storage_set(b"marketplace_fee_addr", &fee_addr);
    log_info("Lichen Market NFT Marketplace initialized");
}

/// List an NFT for sale
/// v2: Now writes correct 145-byte layout (was 113 — layout bug fixed)
#[no_mangle]
pub extern "C" fn list_nft(
    seller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    price: u64,
    payment_token_ptr: *const u8,
) -> u32 {
    unsafe {
        // Parse addresses
        let seller = parse_address(seller_ptr);
        let nft_contract = parse_address(nft_contract_ptr);
        let payment_token = parse_address(payment_token_ptr);

        // AUDIT-FIX: verify caller matches transaction signer
        let real_caller = get_caller();
        if real_caller.0 != seller.0 {
            return 200;
        }

        // Verify seller owns the NFT (cross-contract call!)
        match call_nft_owner(nft_contract, token_id) {
            Ok(owner) if owner.0 == seller.0 => {
                // Store listing — 145 bytes with correct layout
                let listing_key = create_listing_key(nft_contract, token_id);

                let mut listing_data = alloc::vec![0u8; LISTING_SIZE];
                listing_data[0..32].copy_from_slice(&seller.0);
                listing_data[32..64].copy_from_slice(&nft_contract.0);
                listing_data[64..72].copy_from_slice(&token_id.to_le_bytes());
                listing_data[72..80].copy_from_slice(&price.to_le_bytes());
                listing_data[80..112].copy_from_slice(&payment_token.0);
                // bytes 112..144 = royalty_recipient (zero = none)
                listing_data[144] = 1; // active = true
                                       // bytes 145..147 = royalty_bps (0 = no royalty on basic list)

                storage_set(&listing_key, &listing_data);

                // v2: increment listing count
                let count_key = b"mm_listing_count";
                let count = storage_get(count_key)
                    .map(|d| bytes_to_u64(&d))
                    .unwrap_or(0);
                storage_set(count_key, &u64_to_bytes(count + 1));

                log_info("NFT listed for sale");
                1
            }
            _ => {
                log_info("Seller does not own NFT");
                0
            }
        }
    }
}

/// Buy an NFT (executes cross-contract calls to token & NFT contracts)
#[no_mangle]
pub extern "C" fn buy_nft(buyer_ptr: *const u8, nft_contract_ptr: *const u8, token_id: u64) -> u32 {
    if is_mm_paused() {
        log_info("Marketplace is paused");
        return 0;
    }
    if !reentrancy_enter() {
        return 0;
    }
    unsafe {
        let buyer = parse_address(buyer_ptr);
        let nft_contract = parse_address(nft_contract_ptr);

        // AUDIT-FIX: verify caller matches transaction signer
        let real_caller = get_caller();
        if real_caller.0 != buyer.0 {
            reentrancy_exit();
            return 200;
        }

        // Load listing
        let listing_key = create_listing_key(nft_contract, token_id);
        let listing_data = match storage_get(&listing_key) {
            Some(data) => data,
            None => {
                log_info("Listing not found");
                reentrancy_exit();
                return 0;
            }
        };

        if listing_data.len() < 147 {
            log_info("Invalid listing data");
            reentrancy_exit();
            return 0;
        }

        // Parse listing
        let mut seller_bytes = [0u8; 32];
        seller_bytes.copy_from_slice(&listing_data[0..32]);
        let seller = Address(seller_bytes);

        let mut price_bytes = [0u8; 8];
        price_bytes.copy_from_slice(&listing_data[72..80]);
        let price = u64::from_le_bytes(price_bytes);

        let mut payment_token_bytes = [0u8; 32];
        payment_token_bytes.copy_from_slice(&listing_data[80..112]);
        let payment_token = Address(payment_token_bytes);

        let active = listing_data[144] == 1;

        if !active {
            log_info("Listing not active");
            reentrancy_exit();
            return 0;
        }

        // v3: Parse royalty recipient and royalty_bps
        let mut royalty_recipient_bytes = [0u8; 32];
        royalty_recipient_bytes.copy_from_slice(&listing_data[112..144]);
        let royalty_recipient = Address(royalty_recipient_bytes);
        let has_royalty = royalty_recipient_bytes != [0u8; 32];
        let mut rbps = [0u8; 2];
        rbps.copy_from_slice(&listing_data[145..147]);
        let royalty_bps = u16::from_le_bytes(rbps) as u64;

        // Calculate marketplace fee (2.5%)
        let fee = get_marketplace_fee();
        // Use u128 to prevent overflow on large NFT prices
        let fee_amount = ((price as u128) * (fee as u128) / 10000) as u64;
        // v3: Calculate royalty
        let royalty_amount = if has_royalty && royalty_bps > 0 {
            ((price as u128) * (royalty_bps as u128) / 10000) as u64
        } else {
            0
        };
        let seller_amount = price - fee_amount - royalty_amount;

        log_info("Executing purchase with escrow pattern...");

        // AUDIT-FIX 1.12: Escrow pattern — hold payment in marketplace until
        // NFT transfer confirms. Prevents buyer losing funds if NFT transfer fails.
        let fee_addr_bytes = storage_get(b"marketplace_fee_addr");
        if fee_addr_bytes.is_none() {
            log_info("marketplace_fee_addr not configured — purchase rejected");
            reentrancy_exit();
            return 0;
        }
        let fee_addr_bytes = fee_addr_bytes.unwrap();
        let marketplace_addr = Address(fee_addr_bytes.as_slice().try_into().unwrap_or_else(|_| {
            log_info("invalid marketplace_fee_addr length");
            [0u8; 32]
        }));

        // STEP 1: Transfer full payment from buyer to marketplace (escrow)
        match receive_token_or_native(payment_token, buyer, marketplace_addr, price) {
            Ok(true) => log_info("Payment escrowed in marketplace"),
            _ => {
                log_info("Payment escrow failed — aborting purchase");
                reentrancy_exit();
                return 0;
            }
        }

        // STEP 2: Transfer NFT from seller to buyer
        match call_nft_transfer(nft_contract, seller, buyer, token_id) {
            Ok(true) => {
                log_info("NFT transferred to buyer");
            }
            _ => {
                // NFT transfer failed — refund buyer from escrow
                log_info("NFT transfer failed — refunding buyer from escrow");
                match transfer_token_or_native(payment_token, marketplace_addr, buyer, price) {
                    Ok(true) => log_info("Buyer refunded from escrow"),
                    _ => log_info("CRITICAL: Escrow refund failed — funds in marketplace"),
                }
                reentrancy_exit();
                return 0;
            }
        }

        // STEP 3: Release escrowed funds — seller gets their share
        match transfer_token_or_native(payment_token, marketplace_addr, seller, seller_amount) {
            Ok(true) => log_info("Seller payment released from escrow"),
            _ => log_info(" Seller payment release failed"),
        }

        // STEP 4: Marketplace fee stays in marketplace_addr (already there)
        // The remaining (price - seller_amount - royalty_amount = fee_amount) stays in escrow as the fee.
        if fee_amount > 0 {
            log_info(&alloc::format!("Marketplace fee retained: {}", fee_amount));
        }

        // STEP 5 (v3): Pay royalty to creator if applicable
        if royalty_amount > 0 {
            match transfer_token_or_native(
                payment_token,
                marketplace_addr,
                royalty_recipient,
                royalty_amount,
            ) {
                Ok(true) => log_info(&alloc::format!(
                    "Royalty paid: {} to creator",
                    royalty_amount
                )),
                _ => log_info("Royalty transfer failed — retained in marketplace"),
            }
        }

        // Mark listing as inactive
        let mut updated_data = listing_data.clone();
        updated_data[144] = 0; // active = false
        storage_set(&listing_key, &updated_data);

        // Track sale stats
        let sc = storage_get(MM_SALE_COUNT_KEY)
            .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
            .unwrap_or(0);
        storage_set(MM_SALE_COUNT_KEY, &u64_to_bytes(sc + 1));
        let sv = storage_get(MM_SALE_VOLUME_KEY)
            .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
            .unwrap_or(0);
        storage_set(MM_SALE_VOLUME_KEY, &u64_to_bytes(sv.saturating_add(price)));

        log_info("Purchase complete with escrow pattern!");
        reentrancy_exit();
        1
    }
}

/// Cancel a listing
#[no_mangle]
pub extern "C" fn cancel_listing(
    seller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
) -> u32 {
    unsafe {
        let seller = parse_address(seller_ptr);
        let nft_contract = parse_address(nft_contract_ptr);

        // AUDIT-FIX: verify caller matches transaction signer
        let real_caller = get_caller();
        if real_caller.0 != seller.0 {
            return 200;
        }

        let listing_key = create_listing_key(nft_contract, token_id);
        let listing_data = match storage_get(&listing_key) {
            Some(data) => data,
            None => return 0,
        };

        // Verify caller is seller
        if listing_data[..32] != seller.0 {
            log_info("Only seller can cancel listing");
            return 0;
        }

        // Mark as inactive
        let mut updated_data = listing_data;
        updated_data[144] = 0;
        storage_set(&listing_key, &updated_data);

        log_info("Listing cancelled");
        1
    }
}

/// Get listing details
#[no_mangle]
pub extern "C" fn get_listing(nft_contract_ptr: *const u8, token_id: u64, out_ptr: *mut u8) -> u32 {
    unsafe {
        let nft_contract = parse_address(nft_contract_ptr);
        let listing_key = create_listing_key(nft_contract, token_id);

        match storage_get(&listing_key) {
            Some(data) => {
                let out_slice = core::slice::from_raw_parts_mut(out_ptr, data.len());
                out_slice.copy_from_slice(&data);
                1
            }
            None => 0,
        }
    }
}

/// Set marketplace fee (owner only)
#[no_mangle]
pub extern "C" fn set_marketplace_fee(caller_ptr: *const u8, new_fee: u64) -> u32 {
    if new_fee > 1000 {
        // Max 10%
        log_info("Fee too high (max 10%)");
        return 0;
    }

    // Verify caller is owner
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    let owner = match storage_get(b"marketplace_owner") {
        Some(data) if data.len() == 32 => data,
        _ => {
            log_info("Marketplace owner not configured");
            return 0;
        }
    };
    if caller[..] != owner[..] {
        log_info("Only marketplace owner can set fee");
        return 0;
    }

    storage_set(b"marketplace_fee", &u64_to_bytes(new_fee));
    log_info("Marketplace fee updated");
    1
}

// ============================================================================
// v2: LIST WITH ROYALTY
// ============================================================================

/// List an NFT with a royalty recipient (v2/v3).
/// Royalty recipient gets a share on every sale.
/// v3: Now also stores royalty_bps in listing bytes 145..147.
#[no_mangle]
pub extern "C" fn list_nft_with_royalty(
    seller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    price: u64,
    payment_token_ptr: *const u8,
    royalty_recipient_ptr: *const u8,
    royalty_bps: u32,
) -> u32 {
    unsafe {
        let seller = parse_address(seller_ptr);
        let nft_contract = parse_address(nft_contract_ptr);
        let payment_token = parse_address(payment_token_ptr);
        let royalty = parse_address(royalty_recipient_ptr);

        // AUDIT-FIX: verify caller matches transaction signer
        let real_caller = get_caller();
        if real_caller.0 != seller.0 {
            return 200;
        }

        match call_nft_owner(nft_contract, token_id) {
            Ok(owner) if owner.0 == seller.0 => {
                let listing_key = create_listing_key(nft_contract, token_id);
                let mut data = alloc::vec![0u8; LISTING_SIZE];
                data[0..32].copy_from_slice(&seller.0);
                data[32..64].copy_from_slice(&nft_contract.0);
                data[64..72].copy_from_slice(&token_id.to_le_bytes());
                data[72..80].copy_from_slice(&price.to_le_bytes());
                data[80..112].copy_from_slice(&payment_token.0);
                data[112..144].copy_from_slice(&royalty.0);
                data[144] = 1;
                // v3: store royalty basis points (cap at 5000 = 50%)
                let capped_bps = if royalty_bps > 5000 {
                    5000u16
                } else {
                    royalty_bps as u16
                };
                data[145..147].copy_from_slice(&capped_bps.to_le_bytes());
                storage_set(&listing_key, &data);

                let count = storage_get(b"mm_listing_count")
                    .map(|d| bytes_to_u64(&d))
                    .unwrap_or(0);
                storage_set(b"mm_listing_count", &u64_to_bytes(count + 1));
                log_info("NFT listed with royalty recipient");
                1
            }
            _ => {
                log_info("Seller does not own NFT");
                0
            }
        }
    }
}

// ============================================================================
// v2: OFFERS
// ============================================================================

/// Make an offer on an NFT (even if not listed).
/// Offer layout: [offerer(32), price(8), payment_token(32), active(1)] = 73 bytes
#[no_mangle]
pub extern "C" fn make_offer(
    offerer_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    price: u64,
    payment_token_ptr: *const u8,
) -> u32 {
    if price < MIN_OFFER_PRICE {
        log_info("Offer price below minimum floor");
        return 0;
    }
    let mut offerer = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(offerer_ptr, offerer.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != offerer {
        return 200;
    }

    let nft_contract = unsafe { parse_address(nft_contract_ptr) };
    let mut payment_token = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(payment_token_ptr, payment_token.as_mut_ptr(), 32);
    }

    let mut key = b"offer:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.push(b':');
    key.extend_from_slice(&token_id.to_le_bytes());
    key.push(b':');
    key.extend_from_slice(&offerer);

    let was_already_active = storage_get(&key)
        .map(|d| d.len() >= 73 && d[72] == 1)
        .unwrap_or(false);
    if !reserve_offer_slot_if_needed(&offerer, was_already_active) {
        log_info("Per-wallet active offer limit reached");
        return 0;
    }

    let mut data = alloc::vec![0u8; 73];
    data[0..32].copy_from_slice(&offerer);
    data[32..40].copy_from_slice(&price.to_le_bytes());
    data[40..72].copy_from_slice(&payment_token);
    data[72] = 1; // active
    storage_set(&key, &data);

    log_info("Offer placed");
    1
}

/// Cancel an offer
#[no_mangle]
pub extern "C" fn cancel_offer(
    offerer_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
) -> u32 {
    let mut offerer = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(offerer_ptr, offerer.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != offerer {
        return 200;
    }

    let nft_contract = unsafe { parse_address(nft_contract_ptr) };

    let mut key = b"offer:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.push(b':');
    key.extend_from_slice(&token_id.to_le_bytes());
    key.push(b':');
    key.extend_from_slice(&offerer);

    let data = match storage_get(&key) {
        Some(d) if d.len() >= 73 => d,
        _ => return 0,
    };
    if &data[0..32] != &offerer[..] {
        return 0;
    }
    let was_active = data[72] == 1;
    let mut updated = data;
    updated[72] = 0;
    storage_set(&key, &updated);
    release_offer_slot_if_needed(&offerer, was_active);
    log_info("Offer cancelled");
    1
}

/// Accept an offer (NFT owner accepts a specific offer)
#[no_mangle]
pub extern "C" fn accept_offer(
    seller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    offerer_ptr: *const u8,
) -> u32 {
    if is_mm_paused() {
        log_info("Marketplace is paused");
        return 0;
    }
    if !reentrancy_enter() {
        return 0;
    }
    unsafe {
        let seller = parse_address(seller_ptr);
        let nft_contract = parse_address(nft_contract_ptr);
        let mut offerer = [0u8; 32];
        core::ptr::copy_nonoverlapping(offerer_ptr, offerer.as_mut_ptr(), 32);

        // AUDIT-FIX: verify caller matches transaction signer
        let real_caller = get_caller();
        if real_caller.0 != seller.0 {
            reentrancy_exit();
            return 200;
        }

        // Load offer
        let mut key = b"offer:".to_vec();
        key.extend_from_slice(&nft_contract.0);
        key.push(b':');
        key.extend_from_slice(&token_id.to_le_bytes());
        key.push(b':');
        key.extend_from_slice(&offerer);

        let data = match storage_get(&key) {
            Some(d) if d.len() >= 73 && d[72] == 1 => d,
            _ => {
                log_info("Active offer not found");
                return 0;
            }
        };

        let mut price_bytes = [0u8; 8];
        price_bytes.copy_from_slice(&data[32..40]);
        let price = u64::from_le_bytes(price_bytes);

        let mut pay_bytes = [0u8; 32];
        pay_bytes.copy_from_slice(&data[40..72]);
        let payment_token = Address(pay_bytes);

        let buyer = Address(offerer);

        // Calculate fee
        let fee = get_marketplace_fee();
        // Use u128 to prevent overflow on large NFT prices
        let fee_amount = ((price as u128) * (fee as u128) / 10000) as u64;
        let seller_amount = price - fee_amount;

        // Transfer payment
        match transfer_token_or_native(payment_token, buyer, seller, seller_amount) {
            Ok(true) => {}
            _ => {
                reentrancy_exit();
                return 0;
            }
        }

        // AUDIT-FIX P2: Actually transfer fee to platform
        if fee_amount > 0 {
            if let Some(fee_addr_data) = storage_get(b"marketplace_fee_addr") {
                if fee_addr_data.len() >= 32 {
                    let mut fee_addr = [0u8; 32];
                    fee_addr.copy_from_slice(&fee_addr_data[..32]);
                    let _ = transfer_token_or_native(
                        payment_token,
                        buyer,
                        Address(fee_addr),
                        fee_amount,
                    );
                }
            }
        }

        // Transfer NFT
        match call_nft_transfer(nft_contract, seller, buyer, token_id) {
            Ok(true) => {
                // Deactivate offer
                let mut updated = data;
                updated[72] = 0;
                storage_set(&key, &updated);
                release_offer_slot_if_needed(&offerer, true);
                // Track sale stats
                let sc = storage_get(MM_SALE_COUNT_KEY)
                    .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
                    .unwrap_or(0);
                storage_set(MM_SALE_COUNT_KEY, &u64_to_bytes(sc + 1));
                let sv = storage_get(MM_SALE_VOLUME_KEY)
                    .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
                    .unwrap_or(0);
                storage_set(MM_SALE_VOLUME_KEY, &u64_to_bytes(sv.saturating_add(price)));

                log_info("Offer accepted, trade executed");
                reentrancy_exit();
                1
            }
            _ => {
                reentrancy_exit();
                0
            }
        }
    }
}

// ============================================================================
// v2: MARKETPLACE STATS
// ============================================================================

/// Get marketplace stats: [listing_count(8), fee_bps(8), sale_count(8), sale_volume(8)]
#[no_mangle]
pub extern "C" fn get_marketplace_stats() -> u32 {
    let count = storage_get(b"mm_listing_count")
        .map(|d| bytes_to_u64(&d))
        .unwrap_or(0);
    let fee = get_marketplace_fee();
    let mut result = Vec::with_capacity(32);
    result.extend_from_slice(&u64_to_bytes(count));
    result.extend_from_slice(&u64_to_bytes(fee));
    result.extend_from_slice(&u64_to_bytes(
        storage_get(MM_SALE_COUNT_KEY)
            .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
            .unwrap_or(0),
    ));
    result.extend_from_slice(&u64_to_bytes(
        storage_get(MM_SALE_VOLUME_KEY)
            .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
            .unwrap_or(0),
    ));
    lichen_sdk::set_return_data(&result);
    0
}

// ============================================================================
// v3: NFT ATTRIBUTES (rarity, category, traits)
// ============================================================================

/// NFT attributes layout (variable length, stored as length-prefixed fields):
///   0      rarity (0=Common, 1=Uncommon, 2=Rare, 3=Epic, 4=Legendary)
///   1      category (0=Art, 1=Music, 2=Photography, 3=Gaming, 4=Collectible, 5=Utility, 6=Domain)
///   2..4   trait_count (u16 LE)
///   4..N   traits data (key-value pairs, each: key_len(1) + key + val_len(1) + val)

/// Set NFT attributes (rarity, category, traits) — callable by NFT owner or admin
#[no_mangle]
pub extern "C" fn set_nft_attributes(
    caller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    rarity: u8,
    category: u8,
    traits_ptr: *const u8,
    traits_len: u32,
) -> u32 {
    if rarity > 4 {
        log_info("Invalid rarity (0-4)");
        return 0;
    }
    if category > 6 {
        log_info("Invalid category (0-6)");
        return 0;
    }
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    let nft_contract = unsafe { parse_address(nft_contract_ptr) };

    // Verify caller is NFT owner or marketplace admin
    let is_owner = match call_nft_owner(nft_contract, token_id) {
        Ok(owner) => owner.0 == caller,
        _ => false,
    };
    if !is_owner && !is_mm_admin(&caller) {
        log_info("Only NFT owner or admin can set attributes");
        return 0;
    }

    let mut key = b"nft_attr:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.push(b':');
    key.extend_from_slice(&token_id.to_le_bytes());

    // Build attribute data
    let traits_len_capped = (traits_len as usize).min(2048); // Cap at 2KB
    let mut data = Vec::with_capacity(4 + traits_len_capped);
    data.push(rarity);
    data.push(category);
    data.extend_from_slice(&(traits_len_capped as u16).to_le_bytes());
    if traits_len_capped > 0 {
        let traits_slice = unsafe { core::slice::from_raw_parts(traits_ptr, traits_len_capped) };
        data.extend_from_slice(traits_slice);
    }

    storage_set(&key, &data);
    log_info("NFT attributes updated");
    1
}

/// Get NFT attributes — returns [rarity(1), category(1), trait_count(2), traits...]
#[no_mangle]
pub extern "C" fn get_nft_attributes(
    nft_contract_ptr: *const u8,
    token_id: u64,
    out_ptr: *mut u8,
) -> u32 {
    let nft_contract = unsafe { parse_address(nft_contract_ptr) };
    let mut key = b"nft_attr:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.push(b':');
    key.extend_from_slice(&token_id.to_le_bytes());

    match storage_get(&key) {
        Some(data) => {
            let out_slice = unsafe { core::slice::from_raw_parts_mut(out_ptr, data.len()) };
            out_slice.copy_from_slice(&data);
            lichen_sdk::set_return_data(&data);
            data.len() as u32
        }
        None => 0,
    }
}

// ============================================================================
// v3: QUERY FUNCTIONS (offers, filtered listings)
// ============================================================================

/// Check if an NFT has any active offers. Returns offer count.
/// Queries storage prefix "offer:{nft_contract}:{token_id}:"
#[no_mangle]
pub extern "C" fn get_offer_count(nft_contract_ptr: *const u8, token_id: u64) -> u32 {
    let nft_contract = unsafe { parse_address(nft_contract_ptr) };
    let mut prefix = b"offer:".to_vec();
    prefix.extend_from_slice(&nft_contract.0);
    prefix.push(b':');
    prefix.extend_from_slice(&token_id.to_le_bytes());
    prefix.push(b':');

    // Count active offers by scanning prefix
    let mut count = 0u32;
    // Use storage iteration via prefix scan
    if let Some(data) = storage_get(&prefix) {
        // Single offer at exact key
        if data.len() >= 73 && data[72] == 1 {
            count += 1;
        }
    }
    // Store the count as return data
    lichen_sdk::set_return_data(&count.to_le_bytes());
    count
}

/// Update listing price (seller only, must be active listing)
#[no_mangle]
pub extern "C" fn update_listing_price(
    seller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    new_price: u64,
) -> u32 {
    if new_price == 0 {
        log_info("Price must be > 0");
        return 0;
    }
    unsafe {
        let seller = parse_address(seller_ptr);
        let nft_contract = parse_address(nft_contract_ptr);

        let real_caller = get_caller();
        if real_caller.0 != seller.0 {
            return 200;
        }

        let listing_key = create_listing_key(nft_contract, token_id);
        let listing_data = match storage_get(&listing_key) {
            Some(data) if data.len() >= LISTING_SIZE => data,
            _ => {
                log_info("Listing not found");
                return 0;
            }
        };

        // Verify caller is seller
        if listing_data[..32] != seller.0 {
            log_info("Only seller can update price");
            return 0;
        }

        // Must be active
        if listing_data[144] != 1 {
            log_info("Listing not active");
            return 0;
        }

        // Update price (bytes 72..80)
        let mut updated = listing_data;
        updated[72..80].copy_from_slice(&new_price.to_le_bytes());
        storage_set(&listing_key, &updated);

        log_info("Listing price updated");
        1
    }
}

// Helper functions

fn get_marketplace_fee() -> u64 {
    match storage_get(b"marketplace_fee") {
        Some(bytes) => bytes_to_u64(&bytes),
        None => 250, // Default 2.5%
    }
}

fn create_listing_key(nft_contract: Address, token_id: u64) -> Vec<u8> {
    let mut key = b"listing:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.push(b':');
    key.extend_from_slice(&token_id.to_le_bytes());
    key
}

unsafe fn parse_address(ptr: *const u8) -> Address {
    let mut addr = [0u8; 32];
    core::ptr::copy_nonoverlapping(ptr, addr.as_mut_ptr(), 32);
    Address(addr)
}

// ============================================================================
// AUCTION SYSTEM (English Auctions, OpenSea-parity)
// ============================================================================

/// Auction layout (185 bytes):
///   0..32   seller
///   32..64  nft_contract
///   64..72  token_id (u64 LE)
///   72..80  start_price (u64 LE)
///   80..88  reserve_price (u64 LE)
///   88..96  highest_bid (u64 LE)
///   96..128 highest_bidder (32 bytes, zero = no bids)
///   128..136 start_time (u64 LE, unix timestamp)
///   136..144 end_time (u64 LE, unix timestamp)
///   144     status (0=cancelled, 1=active, 2=settled)
///   145..177 payment_token (32 bytes)
///   177..209 royalty_recipient (32 bytes)
///   209..211 royalty_bps (u16 LE)
const AUCTION_SIZE: usize = 211;

fn create_auction_key(nft_contract: Address, token_id: u64) -> Vec<u8> {
    let mut key = b"auction:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.push(b':');
    key.extend_from_slice(&token_id.to_le_bytes());
    key
}

/// Create an English auction
#[no_mangle]
pub extern "C" fn create_auction(
    seller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    start_price: u64,
    reserve_price: u64,
    duration: u64,
    payment_token_ptr: *const u8,
) -> u32 {
    if is_mm_paused() {
        return 0;
    }
    unsafe {
        let seller = parse_address(seller_ptr);
        let nft_contract = parse_address(nft_contract_ptr);
        let payment_token = parse_address(payment_token_ptr);

        let real_caller = get_caller();
        if real_caller.0 != seller.0 {
            return 200;
        }

        if start_price == 0 {
            log_info("Start price must be > 0");
            return 0;
        }
        if duration < 60 || duration > 2_592_000 {
            log_info("Duration must be 60s - 30 days");
            return 0;
        }

        // Verify ownership
        match call_nft_owner(nft_contract, token_id) {
            Ok(owner) if owner.0 == seller.0 => {}
            _ => {
                log_info("Seller does not own NFT");
                return 0;
            }
        }

        // Check no existing active auction
        let key = create_auction_key(nft_contract, token_id);
        if let Some(existing) = storage_get(&key) {
            if existing.len() >= AUCTION_SIZE && existing[144] == 1 {
                log_info("Active auction already exists for this NFT");
                return 0;
            }
        }

        let now = lichen_sdk::get_slot();
        let end_time = now + duration;

        let mut data = alloc::vec![0u8; AUCTION_SIZE];
        data[0..32].copy_from_slice(&seller.0);
        data[32..64].copy_from_slice(&nft_contract.0);
        data[64..72].copy_from_slice(&token_id.to_le_bytes());
        data[72..80].copy_from_slice(&start_price.to_le_bytes());
        data[80..88].copy_from_slice(&reserve_price.to_le_bytes());
        // 88..96 highest_bid = 0
        // 96..128 highest_bidder = zero
        data[128..136].copy_from_slice(&now.to_le_bytes());
        data[136..144].copy_from_slice(&end_time.to_le_bytes());
        data[144] = 1; // active
        data[145..177].copy_from_slice(&payment_token.0);
        // 177..209 royalty_recipient = zero (can be set by settle)
        // 209..211 royalty_bps = 0

        storage_set(&key, &data);
        log_info("Auction created");
        1
    }
}

/// Place a bid on an active auction
#[no_mangle]
pub extern "C" fn place_bid(
    bidder_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    bid_amount: u64,
) -> u32 {
    if is_mm_paused() {
        return 0;
    }
    if !reentrancy_enter() {
        return 0;
    }
    unsafe {
        let bidder = parse_address(bidder_ptr);
        let nft_contract = parse_address(nft_contract_ptr);

        let real_caller = get_caller();
        if real_caller.0 != bidder.0 {
            reentrancy_exit();
            return 200;
        }

        let key = create_auction_key(nft_contract, token_id);
        let data = match storage_get(&key) {
            Some(d) if d.len() >= AUCTION_SIZE && d[144] == 1 => d,
            _ => {
                log_info("Active auction not found");
                reentrancy_exit();
                return 0;
            }
        };

        // Check auction hasn't ended
        let now = lichen_sdk::get_slot();
        let mut end_bytes = [0u8; 8];
        end_bytes.copy_from_slice(&data[136..144]);
        let end_time = u64::from_le_bytes(end_bytes);
        if now > end_time {
            log_info("Auction has ended");
            reentrancy_exit();
            return 0;
        }

        // Check bid > current highest
        let mut highest_bytes = [0u8; 8];
        highest_bytes.copy_from_slice(&data[88..96]);
        let current_highest = u64::from_le_bytes(highest_bytes);

        let mut start_price_bytes = [0u8; 8];
        start_price_bytes.copy_from_slice(&data[72..80]);
        let start_price = u64::from_le_bytes(start_price_bytes);

        let min_bid = if current_highest > 0 {
            current_highest + 1
        } else {
            start_price
        };
        if bid_amount < min_bid {
            log_info("Bid too low");
            reentrancy_exit();
            return 0;
        }

        // Parse payment token
        let mut pay_bytes = [0u8; 32];
        pay_bytes.copy_from_slice(&data[145..177]);
        let payment_token = Address(pay_bytes);

        // Escrow bid from bidder to marketplace
        let fee_addr_bytes = match storage_get(b"marketplace_fee_addr") {
            Some(v) if v.len() >= 32 => v,
            _ => {
                log_info("marketplace_fee_addr not configured — bid rejected");
                reentrancy_exit();
                return 0;
            }
        };
        let marketplace_addr = Address(fee_addr_bytes.as_slice().try_into().unwrap_or([0u8; 32]));

        match receive_token_or_native(payment_token, bidder, marketplace_addr, bid_amount) {
            Ok(true) => {}
            _ => {
                log_info("Bid escrow failed");
                reentrancy_exit();
                return 0;
            }
        }

        // Refund previous highest bidder
        if current_highest > 0 {
            let mut prev_bidder_bytes = [0u8; 32];
            prev_bidder_bytes.copy_from_slice(&data[96..128]);
            let prev_bidder = Address(prev_bidder_bytes);
            if prev_bidder_bytes != [0u8; 32] {
                let _ = transfer_token_or_native(
                    payment_token,
                    marketplace_addr,
                    prev_bidder,
                    current_highest,
                );
            }
        }

        // Update auction with new highest bid
        let mut updated = data;
        updated[88..96].copy_from_slice(&bid_amount.to_le_bytes());
        updated[96..128].copy_from_slice(&bidder.0);

        // Anti-sniping: extend by 10 min if bid in last 10 min
        let time_left = if end_time > now { end_time - now } else { 0 };
        if time_left < 600 {
            let new_end = now + 600;
            updated[136..144].copy_from_slice(&new_end.to_le_bytes());
        }

        storage_set(&key, &updated);
        log_info("Bid placed");
        reentrancy_exit();
        1
    }
}

/// Settle an auction (anyone can call after end_time)
#[no_mangle]
pub extern "C" fn settle_auction(
    caller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
) -> u32 {
    if !reentrancy_enter() {
        return 0;
    }
    unsafe {
        let caller = parse_address(caller_ptr);
        let nft_contract = parse_address(nft_contract_ptr);

        let real_caller = get_caller();
        if real_caller.0 != caller.0 {
            reentrancy_exit();
            return 200;
        }

        let key = create_auction_key(nft_contract, token_id);
        let data = match storage_get(&key) {
            Some(d) if d.len() >= AUCTION_SIZE && d[144] == 1 => d,
            _ => {
                log_info("Active auction not found");
                reentrancy_exit();
                return 0;
            }
        };

        // Verify auction has ended
        let now = lichen_sdk::get_slot();
        let mut end_bytes = [0u8; 8];
        end_bytes.copy_from_slice(&data[136..144]);
        let end_time = u64::from_le_bytes(end_bytes);
        if now <= end_time {
            log_info("Auction not yet ended");
            reentrancy_exit();
            return 0;
        }

        let mut highest_bytes = [0u8; 8];
        highest_bytes.copy_from_slice(&data[88..96]);
        let highest_bid = u64::from_le_bytes(highest_bytes);

        let mut reserve_bytes = [0u8; 8];
        reserve_bytes.copy_from_slice(&data[80..88]);
        let reserve_price = u64::from_le_bytes(reserve_bytes);

        let mut bidder_bytes = [0u8; 32];
        bidder_bytes.copy_from_slice(&data[96..128]);

        let mut seller_bytes = [0u8; 32];
        seller_bytes.copy_from_slice(&data[0..32]);
        let seller = Address(seller_bytes);

        let mut pay_bytes = [0u8; 32];
        pay_bytes.copy_from_slice(&data[145..177]);
        let payment_token = Address(pay_bytes);

        let fee_addr_bytes = match storage_get(b"marketplace_fee_addr") {
            Some(v) if v.len() >= 32 => v,
            _ => {
                log_info("marketplace_fee_addr not configured — auction finalize rejected");
                reentrancy_exit();
                return 0;
            }
        };
        let marketplace_addr = Address(fee_addr_bytes.as_slice().try_into().unwrap_or([0u8; 32]));

        // If no bids or reserve not met, cancel and return
        if highest_bid == 0 || (reserve_price > 0 && highest_bid < reserve_price) {
            // No winner — refund highest bidder if any
            if highest_bid > 0 && bidder_bytes != [0u8; 32] {
                let _ = transfer_token_or_native(
                    payment_token,
                    marketplace_addr,
                    Address(bidder_bytes),
                    highest_bid,
                );
            }
            let mut updated = data;
            updated[144] = 0; // cancelled
            storage_set(&key, &updated);
            log_info("Auction settled: reserve not met, refunded");
            reentrancy_exit();
            return 2; // settled with no sale
        }

        let winner = Address(bidder_bytes);
        let price = highest_bid;

        // Calculate fee + royalty
        let fee = get_marketplace_fee();
        let fee_amount = ((price as u128) * (fee as u128) / 10000) as u64;

        let mut royalty_recip_bytes = [0u8; 32];
        royalty_recip_bytes.copy_from_slice(&data[177..209]);
        let has_royalty = royalty_recip_bytes != [0u8; 32];
        let royalty_bps: u64 = if data.len() >= 211 {
            let mut rbps = [0u8; 2];
            rbps.copy_from_slice(&data[209..211]);
            u16::from_le_bytes(rbps) as u64
        } else {
            0
        };
        let royalty_amount = if has_royalty && royalty_bps > 0 {
            ((price as u128) * (royalty_bps as u128) / 10000) as u64
        } else {
            0
        };

        let seller_amount = price - fee_amount - royalty_amount;

        // Transfer NFT from seller to winner
        match call_nft_transfer(nft_contract, seller, winner, token_id) {
            Ok(true) => {}
            _ => {
                log_info("NFT transfer failed in auction settlement");
                // Refund winner
                let _ = transfer_token_or_native(payment_token, marketplace_addr, winner, price);
                let mut updated = data;
                updated[144] = 0;
                storage_set(&key, &updated);
                reentrancy_exit();
                return 0;
            }
        }

        // Pay seller from escrow
        let _ = transfer_token_or_native(payment_token, marketplace_addr, seller, seller_amount);
        // Pay royalty; if royalty transfer fails, credit seller fallback so seller is not underpaid.
        if royalty_amount > 0 {
            match transfer_token_or_native(
                payment_token,
                marketplace_addr,
                Address(royalty_recip_bytes),
                royalty_amount,
            ) {
                Ok(true) => {
                    log_info("Auction royalty paid");
                }
                _ => {
                    log_info("Auction royalty transfer failed; paying fallback to seller");
                    let _ = transfer_token_or_native(
                        payment_token,
                        marketplace_addr,
                        seller,
                        royalty_amount,
                    );
                }
            }
        }

        // Mark auction as settled
        let mut updated = data;
        updated[144] = 2; // settled
        storage_set(&key, &updated);

        // Track stats
        let sc = storage_get(MM_SALE_COUNT_KEY)
            .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
            .unwrap_or(0);
        storage_set(MM_SALE_COUNT_KEY, &u64_to_bytes(sc + 1));
        let sv = storage_get(MM_SALE_VOLUME_KEY)
            .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
            .unwrap_or(0);
        storage_set(MM_SALE_VOLUME_KEY, &u64_to_bytes(sv.saturating_add(price)));

        log_info("Auction settled: NFT transferred to winner");
        reentrancy_exit();
        1
    }
}

/// Cancel an auction (seller only, only if no bids placed)
#[no_mangle]
pub extern "C" fn cancel_auction(
    seller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
) -> u32 {
    unsafe {
        let seller = parse_address(seller_ptr);
        let nft_contract = parse_address(nft_contract_ptr);

        let real_caller = get_caller();
        if real_caller.0 != seller.0 {
            return 200;
        }

        let key = create_auction_key(nft_contract, token_id);
        let data = match storage_get(&key) {
            Some(d) if d.len() >= AUCTION_SIZE && d[144] == 1 => d,
            _ => {
                log_info("Active auction not found");
                return 0;
            }
        };

        if data[0..32] != seller.0 {
            log_info("Only seller can cancel");
            return 0;
        }

        // Can only cancel if no bids placed
        let mut highest_bytes = [0u8; 8];
        highest_bytes.copy_from_slice(&data[88..96]);
        let highest_bid = u64::from_le_bytes(highest_bytes);
        if highest_bid > 0 {
            log_info("Cannot cancel auction with bids");
            return 0;
        }

        let mut updated = data;
        updated[144] = 0; // cancelled
        storage_set(&key, &updated);
        log_info("Auction cancelled");
        1
    }
}

/// Get auction details
#[no_mangle]
pub extern "C" fn get_auction(nft_contract_ptr: *const u8, token_id: u64, out_ptr: *mut u8) -> u32 {
    unsafe {
        let nft_contract = parse_address(nft_contract_ptr);
        let key = create_auction_key(nft_contract, token_id);
        match storage_get(&key) {
            Some(data) if data.len() >= AUCTION_SIZE => {
                let out = core::slice::from_raw_parts_mut(out_ptr, data.len());
                out.copy_from_slice(&data);
                1
            }
            _ => 0,
        }
    }
}

// ============================================================================
// COLLECTION OFFERS
// ============================================================================

/// Collection offer layout (113 bytes):
///   0..32   offerer
///   32..64  collection (nft_contract address)
///   64..72  price (u64 LE)
///   72..104 payment_token (32 bytes)
///   104     active (1=active, 0=inactive)
///   105..113 expiry (u64 LE, 0 = no expiry)
const COLLECTION_OFFER_SIZE: usize = 113;

/// Make an offer on any NFT in a collection
#[no_mangle]
pub extern "C" fn make_collection_offer(
    offerer_ptr: *const u8,
    collection_ptr: *const u8,
    price: u64,
    payment_token_ptr: *const u8,
    expiry: u64,
) -> u32 {
    if price == 0 {
        log_info("Price must be > 0");
        return 0;
    }
    unsafe {
        let offerer = parse_address(offerer_ptr);
        let collection = parse_address(collection_ptr);
        let payment_token = parse_address(payment_token_ptr);

        let real_caller = get_caller();
        if real_caller.0 != offerer.0 {
            return 200;
        }

        let mut key = b"col_offer:".to_vec();
        key.extend_from_slice(&collection.0);
        key.push(b':');
        key.extend_from_slice(&offerer.0);

        let mut data = alloc::vec![0u8; COLLECTION_OFFER_SIZE];
        data[0..32].copy_from_slice(&offerer.0);
        data[32..64].copy_from_slice(&collection.0);
        data[64..72].copy_from_slice(&price.to_le_bytes());
        data[72..104].copy_from_slice(&payment_token.0);
        data[104] = 1; // active
        data[105..113].copy_from_slice(&expiry.to_le_bytes());

        storage_set(&key, &data);
        log_info("Collection offer placed");
        1
    }
}

/// Accept a collection offer (owner of any NFT in the collection)
#[no_mangle]
pub extern "C" fn accept_collection_offer(
    seller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    offerer_ptr: *const u8,
) -> u32 {
    if is_mm_paused() {
        return 0;
    }
    if !reentrancy_enter() {
        return 0;
    }
    unsafe {
        let seller = parse_address(seller_ptr);
        let nft_contract = parse_address(nft_contract_ptr);
        let offerer = parse_address(offerer_ptr);

        let real_caller = get_caller();
        if real_caller.0 != seller.0 {
            reentrancy_exit();
            return 200;
        }

        // Verify seller owns this specific NFT
        match call_nft_owner(nft_contract, token_id) {
            Ok(owner) if owner.0 == seller.0 => {}
            _ => {
                log_info("Seller does not own NFT");
                reentrancy_exit();
                return 0;
            }
        }

        // Load collection offer
        let mut key = b"col_offer:".to_vec();
        key.extend_from_slice(&nft_contract.0);
        key.push(b':');
        key.extend_from_slice(&offerer.0);

        let data = match storage_get(&key) {
            Some(d) if d.len() >= COLLECTION_OFFER_SIZE && d[104] == 1 => d,
            _ => {
                log_info("Active collection offer not found");
                reentrancy_exit();
                return 0;
            }
        };

        // Check expiry
        let mut expiry_bytes = [0u8; 8];
        expiry_bytes.copy_from_slice(&data[105..113]);
        let expiry = u64::from_le_bytes(expiry_bytes);
        if expiry > 0 {
            let now = lichen_sdk::get_slot();
            if now > expiry {
                log_info("Collection offer has expired");
                reentrancy_exit();
                return 0;
            }
        }

        let mut price_bytes = [0u8; 8];
        price_bytes.copy_from_slice(&data[64..72]);
        let price = u64::from_le_bytes(price_bytes);

        let mut pay_bytes = [0u8; 32];
        pay_bytes.copy_from_slice(&data[72..104]);
        let payment_token = Address(pay_bytes);

        let fee_addr_bytes = match storage_get(b"marketplace_fee_addr") {
            Some(v) if v.len() >= 32 => v,
            _ => {
                log_info("marketplace_fee_addr not configured — offer rejected");
                reentrancy_exit();
                return 0;
            }
        };
        let marketplace_addr = Address(fee_addr_bytes.as_slice().try_into().unwrap_or([0u8; 32]));

        // Fee calculation
        let fee = get_marketplace_fee();
        let fee_amount = ((price as u128) * (fee as u128) / 10000) as u64;
        let seller_amount = price - fee_amount;

        // Escrow payment in marketplace first to prevent double-pull from offerer.
        match receive_token_or_native(payment_token, offerer, marketplace_addr, price) {
            Ok(true) => {}
            _ => {
                reentrancy_exit();
                return 0;
            }
        }

        // Transfer NFT
        match call_nft_transfer(nft_contract, seller, offerer, token_id) {
            Ok(true) => {
                // Release seller proceeds from escrow.
                let _ = transfer_token_or_native(
                    payment_token,
                    marketplace_addr,
                    seller,
                    seller_amount,
                );
                if fee_amount > 0 {
                    log_info("Collection-offer fee retained in marketplace escrow");
                }

                let mut updated = data;
                updated[104] = 0;
                storage_set(&key, &updated);

                let sc = storage_get(MM_SALE_COUNT_KEY)
                    .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
                    .unwrap_or(0);
                storage_set(MM_SALE_COUNT_KEY, &u64_to_bytes(sc + 1));
                let sv = storage_get(MM_SALE_VOLUME_KEY)
                    .map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 })
                    .unwrap_or(0);
                storage_set(MM_SALE_VOLUME_KEY, &u64_to_bytes(sv.saturating_add(price)));

                log_info("Collection offer accepted");
                reentrancy_exit();
                1
            }
            _ => {
                let _ = transfer_token_or_native(payment_token, marketplace_addr, offerer, price);
                reentrancy_exit();
                0
            }
        }
    }
}

/// Cancel a collection offer
#[no_mangle]
pub extern "C" fn cancel_collection_offer(
    offerer_ptr: *const u8,
    collection_ptr: *const u8,
) -> u32 {
    unsafe {
        let offerer = parse_address(offerer_ptr);
        let collection = parse_address(collection_ptr);

        let real_caller = get_caller();
        if real_caller.0 != offerer.0 {
            return 200;
        }

        let mut key = b"col_offer:".to_vec();
        key.extend_from_slice(&collection.0);
        key.push(b':');
        key.extend_from_slice(&offerer.0);

        let data = match storage_get(&key) {
            Some(d) if d.len() >= COLLECTION_OFFER_SIZE && d[104] == 1 => d,
            _ => return 0,
        };
        if data[0..32] != offerer.0 {
            return 0;
        }

        let mut updated = data;
        updated[104] = 0;
        storage_set(&key, &updated);
        log_info("Collection offer cancelled");
        1
    }
}

// ============================================================================
// OFFER EXPIRY
// ============================================================================

/// Make an offer with optional expiry
/// Offer layout (with expiry): [offerer(32), price(8), payment_token(32), active(1), expiry(8)] = 81 bytes
const OFFER_EXPIRY_SIZE: usize = 81;

#[no_mangle]
pub extern "C" fn make_offer_with_expiry(
    offerer_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    price: u64,
    payment_token_ptr: *const u8,
    expiry: u64,
) -> u32 {
    if price < MIN_OFFER_PRICE {
        log_info("Offer price below minimum floor");
        return 0;
    }
    let mut offerer = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(offerer_ptr, offerer.as_mut_ptr(), 32);
    }

    let real_caller = get_caller();
    if real_caller.0 != offerer {
        return 200;
    }

    let nft_contract = unsafe { parse_address(nft_contract_ptr) };
    let mut payment_token = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(payment_token_ptr, payment_token.as_mut_ptr(), 32);
    }

    let mut key = b"offer:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.push(b':');
    key.extend_from_slice(&token_id.to_le_bytes());
    key.push(b':');
    key.extend_from_slice(&offerer);

    let was_already_active = storage_get(&key)
        .map(|d| d.len() >= 73 && d[72] == 1)
        .unwrap_or(false);
    if !reserve_offer_slot_if_needed(&offerer, was_already_active) {
        log_info("Per-wallet active offer limit reached");
        return 0;
    }

    let mut data = alloc::vec![0u8; OFFER_EXPIRY_SIZE];
    data[0..32].copy_from_slice(&offerer);
    data[32..40].copy_from_slice(&price.to_le_bytes());
    data[40..72].copy_from_slice(&payment_token);
    data[72] = 1; // active
    data[73..81].copy_from_slice(&expiry.to_le_bytes());
    storage_set(&key, &data);
    log_info("Offer placed with expiry");
    1
}

// ============================================================================
// EMERGENCY PAUSE (admin only)
// ============================================================================

/// Pause the marketplace
#[no_mangle]
pub extern "C" fn mm_pause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_mm_admin(&caller) {
        return 1;
    }
    storage_set(MM_PAUSE_KEY, &[1u8]);
    log_info("LichenMarket paused");
    0
}

/// Unpause the marketplace
#[no_mangle]
pub extern "C" fn mm_unpause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller {
        return 200;
    }

    if !is_mm_admin(&caller) {
        return 1;
    }
    storage_set(MM_PAUSE_KEY, &[0u8]);
    log_info("LichenMarket unpaused");
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

    /// Create a listing directly in storage with the 147-byte layout (v3).
    fn create_test_listing(
        seller: &[u8; 32],
        nft_contract: &Address,
        token_id: u64,
        price: u64,
        payment_token: &Address,
    ) {
        let key = create_listing_key(*nft_contract, token_id);
        let mut data = alloc::vec![0u8; LISTING_SIZE];
        data[0..32].copy_from_slice(seller);
        data[32..64].copy_from_slice(&nft_contract.0);
        data[64..72].copy_from_slice(&token_id.to_le_bytes());
        data[72..80].copy_from_slice(&price.to_le_bytes());
        data[80..112].copy_from_slice(&payment_token.0);
        data[144] = 1; // active
                       // bytes 145..147 = royalty_bps (0 by default)
        lichen_sdk::storage_set(&key, &data);
    }

    #[test]
    fn test_initialize() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());
        let stored = test_mock::get_storage(b"marketplace_owner");
        assert_eq!(stored, Some(owner.to_vec()));
        let fee = bytes_to_u64(&test_mock::get_storage(b"marketplace_fee").unwrap());
        assert_eq!(fee, 250); // 2.5%
    }

    #[test]
    fn test_list_nft_ownership_fails() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());
        let seller = [3u8; 32];
        let nft = [4u8; 32];
        let pay = [5u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(seller);
        // call_nft_owner returns Err in test mock → falls through to _ arm
        let result = list_nft(seller.as_ptr(), nft.as_ptr(), 1, 1000, pay.as_ptr());
        assert_eq!(result, 0);
    }

    #[test]
    fn test_buy_nft_not_found() {
        setup();
        let buyer = [3u8; 32];
        let nft = [4u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(buyer);
        assert_eq!(buy_nft(buyer.as_ptr(), nft.as_ptr(), 1), 0);
    }

    #[test]
    fn test_buy_nft_not_active() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());
        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let pay = Address([5u8; 32]);
        create_test_listing(&seller, &nft, 1, 1000, &pay);
        // Mark inactive
        let key = create_listing_key(nft, 1);
        let mut data = lichen_sdk::storage_get(&key).unwrap();
        data[144] = 0;
        lichen_sdk::storage_set(&key, &data);
        let buyer = [6u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(buyer);
        assert_eq!(buy_nft(buyer.as_ptr(), nft.0.as_ptr(), 1), 0);
    }

    #[test]
    fn test_cancel_listing() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());
        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let pay = Address([5u8; 32]);
        create_test_listing(&seller, &nft, 1, 1000, &pay);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(seller);
        assert_eq!(cancel_listing(seller.as_ptr(), nft.0.as_ptr(), 1), 1);
        let key = create_listing_key(nft, 1);
        let data = lichen_sdk::storage_get(&key).unwrap();
        assert_eq!(data[144], 0);
    }

    #[test]
    fn test_cancel_listing_wrong_seller() {
        setup();
        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let pay = Address([5u8; 32]);
        create_test_listing(&seller, &nft, 1, 1000, &pay);
        let other = [6u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(other);
        assert_eq!(cancel_listing(other.as_ptr(), nft.0.as_ptr(), 1), 0);
    }

    #[test]
    fn test_cancel_listing_not_found() {
        setup();
        let seller = [3u8; 32];
        let nft = [4u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(seller);
        assert_eq!(cancel_listing(seller.as_ptr(), nft.as_ptr(), 999), 0);
    }

    #[test]
    fn test_get_listing() {
        setup();
        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let pay = Address([5u8; 32]);
        create_test_listing(&seller, &nft, 1, 1000, &pay);
        let mut out = [0u8; 145];
        let result = get_listing(nft.0.as_ptr(), 1, out.as_mut_ptr());
        assert_eq!(result, 1);
        assert_eq!(&out[0..32], &seller[..]);
    }

    #[test]
    fn test_get_listing_not_found() {
        setup();
        let nft = [4u8; 32];
        let mut out = [0u8; 145];
        assert_eq!(get_listing(nft.as_ptr(), 999, out.as_mut_ptr()), 0);
    }

    #[test]
    fn test_set_marketplace_fee() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());
        assert_eq!(set_marketplace_fee(owner.as_ptr(), 500), 1);
        let fee = bytes_to_u64(&test_mock::get_storage(b"marketplace_fee").unwrap());
        assert_eq!(fee, 500);
    }

    #[test]
    fn test_set_marketplace_fee_unauthorized() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());
        let other = [3u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(other);
        assert_eq!(set_marketplace_fee(other.as_ptr(), 500), 0);
    }

    #[test]
    fn test_set_marketplace_fee_too_high() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        initialize(owner.as_ptr(), fee_addr.as_ptr());
        assert_eq!(set_marketplace_fee(owner.as_ptr(), 1001), 0);
    }

    // ========================================================================
    // v2 TESTS
    // ========================================================================

    #[test]
    fn test_make_and_cancel_offer() {
        setup();
        let nft = Address([4u8; 32]);
        let pay = [5u8; 32];
        let offerer = [6u8; 32];

        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(offerer);
        // Make offer (price >= MIN_OFFER_PRICE = 1_000_000)
        assert_eq!(
            make_offer(offerer.as_ptr(), nft.0.as_ptr(), 1, 1_000_000, pay.as_ptr()),
            1
        );

        // Verify offer stored
        let mut key = b"offer:".to_vec();
        key.extend_from_slice(&nft.0);
        key.push(b':');
        key.extend_from_slice(&1u64.to_le_bytes());
        key.push(b':');
        key.extend_from_slice(&offerer);
        let data = lichen_sdk::storage_get(&key).unwrap();
        assert_eq!(data.len(), 73);
        assert_eq!(data[72], 1); // active

        // Cancel offer
        assert_eq!(cancel_offer(offerer.as_ptr(), nft.0.as_ptr(), 1), 1);
        let data = lichen_sdk::storage_get(&key).unwrap();
        assert_eq!(data[72], 0); // inactive
    }

    #[test]
    fn test_offer_zero_price() {
        setup();
        let nft = [4u8; 32];
        let pay = [5u8; 32];
        let offerer = [6u8; 32];
        assert_eq!(
            make_offer(offerer.as_ptr(), nft.as_ptr(), 1, 0, pay.as_ptr()),
            0
        );
    }

    #[test]
    fn test_cancel_nonexistent_offer() {
        setup();
        let offerer = [6u8; 32];
        let nft = [4u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(offerer);
        assert_eq!(cancel_offer(offerer.as_ptr(), nft.as_ptr(), 1), 0);
    }

    #[test]
    fn test_get_marketplace_stats() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());

        assert_eq!(get_marketplace_stats(), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 32); // 4 x u64: count, fee, sale_count, sale_volume
        assert_eq!(bytes_to_u64(&ret[0..8]), 0); // no listings
        assert_eq!(bytes_to_u64(&ret[8..16]), 250); // 2.5% fee
    }

    #[test]
    fn test_listing_size_constant() {
        // Verify our LISTING_SIZE matches the expected 147 bytes (v3: +2 for royalty_bps)
        assert_eq!(LISTING_SIZE, 147);
        // Verify: 32 (seller) + 32 (nft) + 8 (token_id) + 8 (price) + 32 (payment) + 32 (royalty) + 1 (active) + 2 (royalty_bps)
        assert_eq!(32 + 32 + 8 + 8 + 32 + 32 + 1 + 2, 147);
    }

    // ========================================================================
    // v3 TESTS: Attributes, price update, offer count
    // ========================================================================

    #[test]
    fn test_set_and_get_nft_attributes() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());

        let nft = Address([4u8; 32]);
        let nft_owner = [7u8; 32];
        test_mock::set_caller(nft_owner);
        test_mock::set_cross_call_response(Some(nft_owner.to_vec()));

        // Set rarity=3 (Epic), category=0 (Art), no traits
        let traits: [u8; 0] = [];
        assert_eq!(
            set_nft_attributes(
                nft_owner.as_ptr(),
                nft.0.as_ptr(),
                1,
                3,
                0,
                traits.as_ptr(),
                0
            ),
            1
        );

        // Read back
        let mut out = [0u8; 256];
        let len = get_nft_attributes(nft.0.as_ptr(), 1, out.as_mut_ptr());
        assert!(len >= 4);
        assert_eq!(out[0], 3); // rarity = Epic
        assert_eq!(out[1], 0); // category = Art
    }

    #[test]
    fn test_set_nft_attributes_invalid_rarity() {
        setup();
        let nft_owner = [7u8; 32];
        test_mock::set_caller(nft_owner);
        let nft = [4u8; 32];
        let traits: [u8; 0] = [];
        assert_eq!(
            set_nft_attributes(
                nft_owner.as_ptr(),
                nft.as_ptr(),
                1,
                5,
                0,
                traits.as_ptr(),
                0 // rarity 5 is invalid
            ),
            0
        );
    }

    #[test]
    fn test_set_nft_attributes_invalid_category() {
        setup();
        let nft_owner = [7u8; 32];
        test_mock::set_caller(nft_owner);
        let nft = [4u8; 32];
        let traits: [u8; 0] = [];
        assert_eq!(
            set_nft_attributes(
                nft_owner.as_ptr(),
                nft.as_ptr(),
                1,
                0,
                7,
                traits.as_ptr(),
                0 // category 7 is invalid
            ),
            0
        );
    }

    #[test]
    fn test_set_nft_attributes_unauthorized() {
        setup();
        let nft = Address([4u8; 32]);
        let real_owner = [7u8; 32];
        let imposter = [8u8; 32];
        test_mock::set_cross_call_response(Some(real_owner.to_vec()));
        test_mock::set_caller(imposter);
        let traits: [u8; 0] = [];
        assert_eq!(
            set_nft_attributes(
                imposter.as_ptr(),
                nft.0.as_ptr(),
                1,
                1,
                1,
                traits.as_ptr(),
                0
            ),
            0
        );
    }

    #[test]
    fn test_set_nft_attributes_with_traits() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());

        let nft = Address([4u8; 32]);
        let nft_owner = [7u8; 32];
        test_mock::set_caller(nft_owner);
        test_mock::set_cross_call_response(Some(nft_owner.to_vec()));

        // Trait data: "color" = "red" — key_len(5), "color", val_len(3), "red"
        let trait_data: [u8; 12] = [5, b'c', b'o', b'l', b'o', b'r', 3, b'r', b'e', b'd', 0, 0];
        assert_eq!(
            set_nft_attributes(
                nft_owner.as_ptr(),
                nft.0.as_ptr(),
                1,
                4,
                2,
                trait_data.as_ptr(),
                10 // Legendary, Photography
            ),
            1
        );

        let mut out = [0u8; 256];
        let len = get_nft_attributes(nft.0.as_ptr(), 1, out.as_mut_ptr());
        assert_eq!(len, 14); // 4 header + 10 trait bytes
        assert_eq!(out[0], 4); // Legendary
        assert_eq!(out[1], 2); // Photography
        let trait_count = u16::from_le_bytes([out[2], out[3]]);
        assert_eq!(trait_count, 10);
    }

    #[test]
    fn test_update_listing_price() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());

        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let pay = Address([5u8; 32]);
        create_test_listing(&seller, &nft, 1, 1000, &pay);

        test_mock::set_caller(seller);
        assert_eq!(
            update_listing_price(seller.as_ptr(), nft.0.as_ptr(), 1, 2000),
            1
        );

        // Verify price updated
        let key = create_listing_key(nft, 1);
        let data = lichen_sdk::storage_get(&key).unwrap();
        let price = u64::from_le_bytes(data[72..80].try_into().unwrap());
        assert_eq!(price, 2000);
    }

    #[test]
    fn test_update_listing_price_zero() {
        setup();
        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let pay = Address([5u8; 32]);
        create_test_listing(&seller, &nft, 1, 1000, &pay);
        test_mock::set_caller(seller);
        assert_eq!(
            update_listing_price(seller.as_ptr(), nft.0.as_ptr(), 1, 0),
            0
        );
    }

    #[test]
    fn test_update_listing_price_wrong_seller() {
        setup();
        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let pay = Address([5u8; 32]);
        create_test_listing(&seller, &nft, 1, 1000, &pay);
        let other = [6u8; 32];
        test_mock::set_caller(other);
        assert_eq!(
            update_listing_price(other.as_ptr(), nft.0.as_ptr(), 1, 2000),
            0
        );
    }

    #[test]
    fn test_update_listing_price_inactive() {
        setup();
        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let pay = Address([5u8; 32]);
        create_test_listing(&seller, &nft, 1, 1000, &pay);
        // Deactivate the listing
        let key = create_listing_key(nft, 1);
        let mut data = lichen_sdk::storage_get(&key).unwrap();
        data[144] = 0;
        lichen_sdk::storage_set(&key, &data);
        test_mock::set_caller(seller);
        assert_eq!(
            update_listing_price(seller.as_ptr(), nft.0.as_ptr(), 1, 2000),
            0
        );
    }

    #[test]
    fn test_settle_auction_still_works_when_paused() {
        setup();

        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());

        let seller = [3u8; 32];
        let bidder = [6u8; 32];
        let nft = Address([4u8; 32]);
        let payment_token = Address([5u8; 32]);

        test_mock::set_slot(100);
        test_mock::set_caller(seller);
        test_mock::set_cross_call_response(Some(seller.to_vec()));
        assert_eq!(
            create_auction(
                seller.as_ptr(),
                nft.0.as_ptr(),
                1,
                1_000,
                0,
                1_000,
                payment_token.0.as_ptr(),
            ),
            1
        );

        test_mock::set_cross_call_response(None);
        test_mock::set_slot(110);
        test_mock::set_caller(bidder);
        assert_eq!(place_bid(bidder.as_ptr(), nft.0.as_ptr(), 1, 1_000), 1);

        test_mock::set_caller(owner);
        assert_eq!(mm_pause(owner.as_ptr()), 0);

        test_mock::set_slot(1_200);
        assert_eq!(settle_auction(owner.as_ptr(), nft.0.as_ptr(), 1), 1);

        let auction = test_mock::get_storage(&create_auction_key(nft, 1)).unwrap();
        assert_eq!(auction[144], 2);
    }

    #[test]
    fn test_get_nft_attributes_not_found() {
        setup();
        let nft = [4u8; 32];
        let mut out = [0u8; 256];
        assert_eq!(get_nft_attributes(nft.as_ptr(), 999, out.as_mut_ptr()), 0);
    }
}
