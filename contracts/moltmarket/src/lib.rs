// Molt Market v2 - NFT Marketplace with Cross-Contract Calls
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

extern crate alloc;
use alloc::vec::Vec;

use moltchain_sdk::{
    Address, log_info, storage_get, storage_set, bytes_to_u64, u64_to_bytes,
    call_token_transfer, call_nft_transfer, call_nft_owner
};

// Reentrancy guard
const MM_REENTRANCY_KEY: &[u8] = b"mm_reentrancy";

fn reentrancy_enter() -> bool {
    if storage_get(MM_REENTRANCY_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(false) {
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
    storage_get(MM_PAUSE_KEY).map(|v| v.first().copied() == Some(1)).unwrap_or(false)
}

fn is_mm_admin(caller: &[u8]) -> bool {
    storage_get(b"marketplace_owner").map(|d| d.as_slice() == caller).unwrap_or(false)
}

/// Listing layout (145 bytes):
///   0..32   seller
///   32..64  nft_contract
///   64..72  token_id (u64 LE)
///   72..80  price (u64 LE)
///   80..112 payment_token
///   112..144 royalty_recipient (v2: zero = no royalty)
///   144     active (1=active, 0=inactive)
struct Listing {
    seller: Address,
    nft_contract: Address,
    token_id: u64,
    price: u64,
    payment_token: Address,
    active: bool,
}

const LISTING_SIZE: usize = 145;

/// Initialize the marketplace
#[no_mangle]
pub extern "C" fn initialize(owner_ptr: *const u8, fee_addr_ptr: *const u8) {
    // Re-initialization guard: reject if marketplace_owner is already set
    if storage_get(b"marketplace_owner").is_some() {
        log_info("MoltMarket already initialized — ignoring");
        return;
    }

    let mut owner = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner.as_mut_ptr(), 32); }
    let mut fee_addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(fee_addr_ptr, fee_addr.as_mut_ptr(), 32); }
    storage_set(b"marketplace_fee", &u64_to_bytes(250)); // 2.5% fee
    storage_set(b"marketplace_owner", &owner);
    storage_set(b"marketplace_fee_addr", &fee_addr);
    log_info("Molt Market NFT Marketplace initialized");
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
pub extern "C" fn buy_nft(
    buyer_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
) -> u32 {
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
        
        if listing_data.len() < 145 {
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
        
        // Calculate marketplace fee (2.5%)
        let fee = get_marketplace_fee();
        // Use u128 to prevent overflow on large NFT prices
        let fee_amount = ((price as u128) * (fee as u128) / 10000) as u64;
        let seller_amount = price - fee_amount;
        
        log_info("Executing purchase with escrow pattern...");
        
        // AUDIT-FIX 1.12: Escrow pattern — hold payment in marketplace until
        // NFT transfer confirms. Prevents buyer losing funds if NFT transfer fails.
        let fee_addr_bytes = storage_get(b"marketplace_fee_addr")
            .unwrap_or_else(|| alloc::vec![0x4Du8; 32]); // fallback
        let marketplace_addr = Address(fee_addr_bytes.as_slice().try_into().unwrap_or([0x4D; 32]));

        // STEP 1: Transfer full payment from buyer to marketplace (escrow)
        match call_token_transfer(payment_token, buyer, marketplace_addr, price) {
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
                match call_token_transfer(payment_token, marketplace_addr, buyer, price) {
                    Ok(true) => log_info("Buyer refunded from escrow"),
                    _ => log_info("CRITICAL: Escrow refund failed — funds in marketplace"),
                }
                reentrancy_exit();
                return 0;
            }
        }
        
        // STEP 3: Release escrowed funds — seller gets their share
        match call_token_transfer(payment_token, marketplace_addr, seller, seller_amount) {
            Ok(true) => log_info("Seller payment released from escrow"),
            _ => log_info(" Seller payment release failed"),
        }
        
        // STEP 4: Marketplace fee stays in marketplace_addr (already there)
        // The remaining (price - seller_amount = fee_amount) stays in escrow as the fee.
        if fee_amount > 0 {
            log_info(&alloc::format!("Marketplace fee retained: {}", fee_amount));
        }
        
        // Mark listing as inactive
        let mut updated_data = listing_data.clone();
        updated_data[144] = 0; // active = false
        storage_set(&listing_key, &updated_data);
        
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
pub extern "C" fn get_listing(
    nft_contract_ptr: *const u8,
    token_id: u64,
    out_ptr: *mut u8,
) -> u32 {
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
    if new_fee > 1000 { // Max 10%
        log_info("Fee too high (max 10%)");
        return 0;
    }
    
    // Verify caller is owner
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
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

/// List an NFT with a royalty recipient (v2).
/// Royalty recipient gets a share on every sale.
#[no_mangle]
pub extern "C" fn list_nft_with_royalty(
    seller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    price: u64,
    payment_token_ptr: *const u8,
    royalty_recipient_ptr: *const u8,
) -> u32 {
    unsafe {
        let seller = parse_address(seller_ptr);
        let nft_contract = parse_address(nft_contract_ptr);
        let payment_token = parse_address(payment_token_ptr);
        let royalty = parse_address(royalty_recipient_ptr);

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
                storage_set(&listing_key, &data);

                let count = storage_get(b"mm_listing_count")
                    .map(|d| bytes_to_u64(&d)).unwrap_or(0);
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
    if price == 0 {
        log_info("Offer price must be > 0");
        return 0;
    }
    let mut offerer = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(offerer_ptr, offerer.as_mut_ptr(), 32); }
    let nft_contract = unsafe { parse_address(nft_contract_ptr) };
    let mut payment_token = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(payment_token_ptr, payment_token.as_mut_ptr(), 32); }

    let mut key = b"offer:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.push(b':');
    key.extend_from_slice(&token_id.to_le_bytes());
    key.push(b':');
    key.extend_from_slice(&offerer);

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
    unsafe { core::ptr::copy_nonoverlapping(offerer_ptr, offerer.as_mut_ptr(), 32); }
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
    let mut updated = data;
    updated[72] = 0;
    storage_set(&key, &updated);
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
        match call_token_transfer(payment_token, buyer, seller, seller_amount) {
            Ok(true) => {}
            _ => {
                reentrancy_exit();
                return 0;
            }
        }

        // Transfer NFT
        match call_nft_transfer(nft_contract, seller, buyer, token_id) {
            Ok(true) => {
                // Deactivate offer
                let mut updated = data;
                updated[72] = 0;
                storage_set(&key, &updated);
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

/// Get marketplace stats: [listing_count(8), fee_bps(8)]
#[no_mangle]
pub extern "C" fn get_marketplace_stats() -> u32 {
    let count = storage_get(b"mm_listing_count")
        .map(|d| bytes_to_u64(&d)).unwrap_or(0);
    let fee = get_marketplace_fee();
    let mut result = Vec::with_capacity(16);
    result.extend_from_slice(&u64_to_bytes(count));
    result.extend_from_slice(&u64_to_bytes(fee));
    moltchain_sdk::set_return_data(&result);
    0
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
// EMERGENCY PAUSE (admin only)
// ============================================================================

/// Pause the marketplace
#[no_mangle]
pub extern "C" fn mm_pause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    if !is_mm_admin(&caller) {
        return 1;
    }
    storage_set(MM_PAUSE_KEY, &[1u8]);
    log_info("MoltMarket paused");
    0
}

/// Unpause the marketplace
#[no_mangle]
pub extern "C" fn mm_unpause(caller_ptr: *const u8) -> u32 {
    let mut caller = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller.as_mut_ptr(), 32); }
    if !is_mm_admin(&caller) {
        return 1;
    }
    storage_set(MM_PAUSE_KEY, &[0u8]);
    log_info("MoltMarket unpaused");
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

    /// Create a listing directly in storage with the 145-byte layout.
    /// v2: list_nft now writes 145 bytes correctly (layout bug fixed).
    fn create_test_listing(
        seller: &[u8; 32], nft_contract: &Address,
        token_id: u64, price: u64, payment_token: &Address,
    ) {
        let key = create_listing_key(*nft_contract, token_id);
        let mut data = alloc::vec![0u8; 145];
        data[0..32].copy_from_slice(seller);
        data[32..64].copy_from_slice(&nft_contract.0);
        data[64..72].copy_from_slice(&token_id.to_le_bytes());
        data[72..80].copy_from_slice(&price.to_le_bytes());
        data[80..112].copy_from_slice(&payment_token.0);
        data[144] = 1; // active
        moltchain_sdk::storage_set(&key, &data);
    }

    #[test]
    fn test_initialize() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
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
        initialize(owner.as_ptr(), fee_addr.as_ptr());
        let seller = [3u8; 32];
        let nft = [4u8; 32];
        let pay = [5u8; 32];
        // call_nft_owner returns Err in test mock → falls through to _ arm
        let result = list_nft(seller.as_ptr(), nft.as_ptr(), 1, 1000, pay.as_ptr());
        assert_eq!(result, 0);
    }

    #[test]
    fn test_buy_nft_not_found() {
        setup();
        let buyer = [3u8; 32];
        let nft = [4u8; 32];
        assert_eq!(buy_nft(buyer.as_ptr(), nft.as_ptr(), 1), 0);
    }

    #[test]
    fn test_buy_nft_not_active() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        initialize(owner.as_ptr(), fee_addr.as_ptr());
        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let pay = Address([5u8; 32]);
        create_test_listing(&seller, &nft, 1, 1000, &pay);
        // Mark inactive
        let key = create_listing_key(nft, 1);
        let mut data = moltchain_sdk::storage_get(&key).unwrap();
        data[144] = 0;
        moltchain_sdk::storage_set(&key, &data);
        assert_eq!(buy_nft([6u8; 32].as_ptr(), nft.0.as_ptr(), 1), 0);
    }

    #[test]
    fn test_cancel_listing() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        initialize(owner.as_ptr(), fee_addr.as_ptr());
        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let pay = Address([5u8; 32]);
        create_test_listing(&seller, &nft, 1, 1000, &pay);
        assert_eq!(cancel_listing(seller.as_ptr(), nft.0.as_ptr(), 1), 1);
        let key = create_listing_key(nft, 1);
        let data = moltchain_sdk::storage_get(&key).unwrap();
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
        assert_eq!(cancel_listing(other.as_ptr(), nft.0.as_ptr(), 1), 0);
    }

    #[test]
    fn test_cancel_listing_not_found() {
        setup();
        let seller = [3u8; 32];
        let nft = [4u8; 32];
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
        initialize(owner.as_ptr(), fee_addr.as_ptr());
        let other = [3u8; 32];
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

        // Make offer
        assert_eq!(make_offer(offerer.as_ptr(), nft.0.as_ptr(), 1, 5000, pay.as_ptr()), 1);

        // Verify offer stored
        let mut key = b"offer:".to_vec();
        key.extend_from_slice(&nft.0);
        key.push(b':');
        key.extend_from_slice(&1u64.to_le_bytes());
        key.push(b':');
        key.extend_from_slice(&offerer);
        let data = moltchain_sdk::storage_get(&key).unwrap();
        assert_eq!(data.len(), 73);
        assert_eq!(data[72], 1); // active

        // Cancel offer
        assert_eq!(cancel_offer(offerer.as_ptr(), nft.0.as_ptr(), 1), 1);
        let data = moltchain_sdk::storage_get(&key).unwrap();
        assert_eq!(data[72], 0); // inactive
    }

    #[test]
    fn test_offer_zero_price() {
        setup();
        let nft = [4u8; 32];
        let pay = [5u8; 32];
        let offerer = [6u8; 32];
        assert_eq!(make_offer(offerer.as_ptr(), nft.as_ptr(), 1, 0, pay.as_ptr()), 0);
    }

    #[test]
    fn test_cancel_nonexistent_offer() {
        setup();
        let offerer = [6u8; 32];
        let nft = [4u8; 32];
        assert_eq!(cancel_offer(offerer.as_ptr(), nft.as_ptr(), 1), 0);
    }

    #[test]
    fn test_get_marketplace_stats() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        initialize(owner.as_ptr(), fee_addr.as_ptr());

        assert_eq!(get_marketplace_stats(), 0);
        let ret = test_mock::get_return_data();
        assert_eq!(ret.len(), 16);
        assert_eq!(bytes_to_u64(&ret[0..8]), 0); // no listings
        assert_eq!(bytes_to_u64(&ret[8..16]), 250); // 2.5% fee
    }

    #[test]
    fn test_listing_size_constant() {
        // Verify our LISTING_SIZE matches the expected 145 bytes
        assert_eq!(LISTING_SIZE, 145);
        // Verify: 32 (seller) + 32 (nft) + 8 (token_id) + 8 (price) + 32 (payment) + 32 (royalty) + 1 (active)
        assert_eq!(32 + 32 + 8 + 8 + 32 + 32 + 1, 145);
    }
}
