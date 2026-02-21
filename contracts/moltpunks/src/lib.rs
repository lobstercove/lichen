// MoltPunks - Collectible NFT Contract
// Example implementation of MT-721 standard

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

extern crate alloc;

use moltchain_sdk::{NFT, Address, log_info, storage_get, storage_set, bytes_to_u64, u64_to_bytes, get_caller};

const MP_TRANSFER_COUNT_KEY: &[u8] = b"mp_transfer_count";
const MP_BURN_COUNT_KEY: &[u8] = b"mp_burn_count";

/// Read the minter address from persistent storage (written by NFT::initialize).
fn get_minter() -> Address {
    match storage_get(b"minter") {
        Some(bytes) if bytes.len() == 32 => {
            let mut addr = [0u8; 32];
            addr.copy_from_slice(&bytes);
            Address(addr)
        }
        // AUDIT-FIX P10-SC-04: Return zero address instead of panicking
        _ => Address([0u8; 32]),
    }
}

/// Build a lightweight NFT handle.
/// All mutable state (owners, balances, approvals, total_minted) lives in storage.
fn make_nft() -> NFT {
    NFT::new("MoltPunks", "MPNK")
}

/// Check if MoltPunks is paused
fn is_mp_paused() -> bool {
    storage_get(b"mp_paused").map(|d| d.first().copied() == Some(1)).unwrap_or(false)
}

/// Initialize the NFT collection
#[no_mangle]
pub extern "C" fn initialize(minter_ptr: *const u8) {
    // AUDIT-FIX 3.18: Re-initialization guard
    if storage_get(b"collection_name").is_some() {
        log_info("MoltPunks already initialized — ignoring");
        return;
    }

    unsafe {
        // Parse minter address
        let mut minter_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(minter_ptr, minter_addr.as_mut_ptr(), 32);
        let minter = Address(minter_addr);
        
        // Store collection metadata in storage for discoverability
        storage_set(b"collection_name", b"MoltPunks");
        storage_set(b"collection_symbol", b"MPNK");
        
        // NFT::initialize stores the minter in storage under key "minter"
        let mut nft = make_nft();
        nft.initialize(minter).expect("Init failed");
        
        log_info("MoltPunks NFT collection initialized");
    }
}

/// Mint new NFT
#[no_mangle]
pub extern "C" fn mint(
    caller_ptr: *const u8,
    to_ptr: *const u8,
    token_id: u64,
    metadata_ptr: *const u8,
    metadata_len: u32,
) -> u32 {
    // AUDIT-FIX P2: Check pause state
    if is_mp_paused() {
        log_info("MoltPunks is paused");
        return 0;
    }
    unsafe {
        // Parse caller
        let mut caller_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(caller_ptr, caller_addr.as_mut_ptr(), 32);
        let caller = Address(caller_addr);

        // P9-SC-06: Verify caller matches transaction signer
        let real_caller = get_caller();
        if real_caller.0 != caller.0 {
            log_info("Unauthorized: caller mismatch");
            return 0;
        }
        
        // Check if caller is minter
        if caller.0 != get_minter().0 {
            log_info("Unauthorized: Only minter can mint");
            return 0;
        }
        
        // AUDIT-FIX P2: Enforce max supply cap
        let current_supply = total_minted();
        if let Some(max_data) = storage_get(b"max_supply") {
            let max = bytes_to_u64(&max_data);
            if max > 0 && current_supply >= max {
                log_info("Max supply reached");
                return 0;
            }
        }
        
        // Parse recipient
        let mut to_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(to_ptr, to_addr.as_mut_ptr(), 32);
        let to = Address(to_addr);
        
        // Parse metadata URI
        let mut metadata = alloc::vec![0u8; metadata_len as usize];
        core::ptr::copy_nonoverlapping(metadata_ptr, metadata.as_mut_ptr(), metadata_len as usize);
        
        // Mint
        let mut nft = make_nft();
        match nft.mint(to, token_id, &metadata) {
            Ok(_) => {
                log_info("NFT minted successfully");
                1
            }
            Err(_) => {
                log_info("Mint failed");
                0
            }
        }
    }
}

/// Transfer NFT
#[no_mangle]
pub extern "C" fn transfer(from_ptr: *const u8, to_ptr: *const u8, token_id: u64) -> u32 {
    // AUDIT-FIX P2: Check pause state
    if is_mp_paused() {
        log_info("MoltPunks is paused");
        return 0;
    }
    unsafe {
        // Parse from address
        let mut from_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(from_ptr, from_addr.as_mut_ptr(), 32);
        let from = Address(from_addr);
        
        // SECURITY FIX: Verify caller owns the NFT being transferred
        let caller = get_caller();
        if caller.0 != from.0 {
            log_info("Unauthorized: caller does not match from address");
            return 0;
        }
        
        // Parse to address
        let mut to_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(to_ptr, to_addr.as_mut_ptr(), 32);
        let to = Address(to_addr);
        
        // Transfer
        match make_nft().transfer(from, to, token_id) {
            Ok(_) => {
                let tc = storage_get(MP_TRANSFER_COUNT_KEY).map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 }).unwrap_or(0);
                storage_set(MP_TRANSFER_COUNT_KEY, &u64_to_bytes(tc + 1));
                log_info("NFT transferred successfully");
                1
            }
            Err(_) => {
                log_info("Transfer failed");
                0
            }
        }
    }
}

/// Get owner of token
#[no_mangle]
pub extern "C" fn owner_of(token_id: u64, out_ptr: *mut u8) -> u32 {
    unsafe {
        match make_nft().owner_of(token_id) {
            Ok(owner) => {
                let out_slice = core::slice::from_raw_parts_mut(out_ptr, 32);
                out_slice.copy_from_slice(&owner.0);
                1
            }
            Err(_) => 0,
        }
    }
}

/// Get balance (number of NFTs owned)
#[no_mangle]
pub extern "C" fn balance_of(account_ptr: *const u8) -> u64 {
    unsafe {
        let mut account_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(account_ptr, account_addr.as_mut_ptr(), 32);
        let account = Address(account_addr);
        
        make_nft().balance_of(account)
    }
}

/// Approve spender for token
#[no_mangle]
pub extern "C" fn approve(owner_ptr: *const u8, spender_ptr: *const u8, token_id: u64) -> u32 {
    unsafe {
        let mut owner_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(owner_ptr, owner_addr.as_mut_ptr(), 32);
        let owner = Address(owner_addr);
        
        // AUDIT-FIX P2: Verify caller is the owner
        let real_caller = get_caller();
        if real_caller.0 != owner_addr {
            log_info("Approve rejected: caller mismatch");
            return 0;
        }
        
        let mut spender_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(spender_ptr, spender_addr.as_mut_ptr(), 32);
        let spender = Address(spender_addr);
        
        match make_nft().approve(owner, spender, token_id) {
            Ok(_) => 1,
            Err(_) => 0,
        }
    }
}

/// Transfer from (with approval)
#[no_mangle]
pub extern "C" fn transfer_from(
    caller_ptr: *const u8,
    from_ptr: *const u8,
    to_ptr: *const u8,
    token_id: u64,
) -> u32 {
    unsafe {
        let mut caller_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(caller_ptr, caller_addr.as_mut_ptr(), 32);
        let caller = Address(caller_addr);
        
        let mut from_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(from_ptr, from_addr.as_mut_ptr(), 32);
        let from = Address(from_addr);
        
        let mut to_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(to_ptr, to_addr.as_mut_ptr(), 32);
        let to = Address(to_addr);
        
        match make_nft().transfer_from(caller, from, to, token_id) {
            Ok(_) => {
                log_info("TransferFrom successful");
                1
            }
            Err(_) => {
                log_info("TransferFrom failed");
                0
            }
        }
    }
}

/// Burn NFT
#[no_mangle]
pub extern "C" fn burn(owner_ptr: *const u8, token_id: u64) -> u32 {
    unsafe {
        let mut owner_addr = [0u8; 32];
        core::ptr::copy_nonoverlapping(owner_ptr, owner_addr.as_mut_ptr(), 32);
        let owner = Address(owner_addr);
        
        // AUDIT-FIX P2: Verify caller is the owner
        let real_caller = get_caller();
        if real_caller.0 != owner_addr {
            log_info("Burn rejected: caller mismatch");
            return 0;
        }
        
        let mut nft = make_nft();
        match nft.burn(owner, token_id) {
            Ok(_) => {
                let bc = storage_get(MP_BURN_COUNT_KEY).map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 }).unwrap_or(0);
                storage_set(MP_BURN_COUNT_KEY, &u64_to_bytes(bc + 1));
                log_info("NFT burned");
                1
            }
            Err(_) => {
                log_info("Burn failed");
                0
            }
        }
    }
}

/// Get total minted (read from persistent storage)
#[no_mangle]
pub extern "C" fn total_minted() -> u64 {
    match storage_get(b"total_minted") {
        Some(bytes) => bytes_to_u64(&bytes),
        None => 0,
    }
}

// ============================================================================
// ALIASES — bridge test-expected names to actual implementation
// ============================================================================

/// Alias: tests call `mint_punk`
#[no_mangle]
pub extern "C" fn mint_punk(
    caller_ptr: *const u8,
    to_ptr: *const u8,
    token_id: u64,
    metadata_ptr: *const u8,
    metadata_len: u32,
) -> u32 {
    mint(caller_ptr, to_ptr, token_id, metadata_ptr, metadata_len)
}

/// Alias: tests call `transfer_punk`
#[no_mangle]
pub extern "C" fn transfer_punk(from_ptr: *const u8, to_ptr: *const u8, token_id: u64) -> u32 {
    transfer(from_ptr, to_ptr, token_id)
}

/// Alias: tests call `get_owner_of`
#[no_mangle]
pub extern "C" fn get_owner_of(token_id: u64, out_ptr: *mut u8) -> u32 {
    owner_of(token_id, out_ptr)
}

/// Alias: tests call `get_total_supply`
#[no_mangle]
pub extern "C" fn get_total_supply() -> u64 {
    total_minted()
}

/// Tests expect `get_punk_metadata`
#[no_mangle]
pub extern "C" fn get_punk_metadata(token_id: u64) -> u32 {
    let key = alloc::format!("nft_meta_{}", token_id);
    match storage_get(key.as_bytes()) {
        Some(data) => {
            moltchain_sdk::set_return_data(&data);
            1
        }
        None => 0,
    }
}

/// Tests expect `get_punks_by_owner`
#[no_mangle]
pub extern "C" fn get_punks_by_owner(owner_ptr: *const u8) -> u64 {
    balance_of(owner_ptr)
}

/// Tests expect `set_base_uri`
#[no_mangle]
pub extern "C" fn set_base_uri(caller_ptr: *const u8, uri_ptr: *const u8, uri_len: u32) -> u32 {
    let mut caller_addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller_addr.as_mut_ptr(), 32); }
    if caller_addr != get_minter().0 { return 0; }
    // AUDIT-FIX P10-SC-06: Verify actual transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller_addr {
        return 0;
    }
    let mut uri = alloc::vec![0u8; uri_len as usize];
    unsafe { core::ptr::copy_nonoverlapping(uri_ptr, uri.as_mut_ptr(), uri_len as usize); }
    storage_set(b"base_uri", &uri);
    log_info("Base URI set");
    1
}

/// Tests expect `set_max_supply`
#[no_mangle]
pub extern "C" fn set_max_supply(caller_ptr: *const u8, max_supply: u64) -> u32 {
    let mut caller_addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller_addr.as_mut_ptr(), 32); }
    if caller_addr != get_minter().0 { return 0; }
    // AUDIT-FIX P10-SC-06: Verify actual transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller_addr {
        return 0;
    }
    storage_set(b"max_supply", &u64_to_bytes(max_supply));
    log_info("Max supply set");
    1
}

/// Tests expect `set_royalty`
#[no_mangle]
pub extern "C" fn set_royalty(caller_ptr: *const u8, bps: u64) -> u32 {
    let mut caller_addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller_addr.as_mut_ptr(), 32); }
    if caller_addr != get_minter().0 { return 0; }
    // AUDIT-FIX P10-SC-06: Verify actual transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller_addr {
        return 0;
    }
    storage_set(b"royalty_bps", &u64_to_bytes(bps));
    log_info("Royalty set");
    1
}

/// Tests expect `mp_pause`
#[no_mangle]
pub extern "C" fn mp_pause(caller_ptr: *const u8) -> u32 {
    let mut caller_addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller_addr.as_mut_ptr(), 32); }
    if caller_addr != get_minter().0 { return 0; }
    // AUDIT-FIX P10-SC-06: Verify actual transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller_addr {
        return 0;
    }
    storage_set(b"mp_paused", &[1u8]);
    log_info("MoltPunks paused");
    1
}

/// Tests expect `mp_unpause`
#[no_mangle]
pub extern "C" fn mp_unpause(caller_ptr: *const u8) -> u32 {
    let mut caller_addr = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller_addr.as_mut_ptr(), 32); }
    if caller_addr != get_minter().0 { return 0; }
    // AUDIT-FIX P10-SC-06: Verify actual transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller_addr {
        return 0;
    }
    storage_set(b"mp_paused", &[0u8]);
    log_info("MoltPunks unpaused");
    1
}

/// Get collection stats [total_minted(8), transfer_count(8), burn_count(8)]
#[no_mangle]
pub extern "C" fn get_collection_stats() -> u32 {
    let mut buf = [0u8; 24];
    let minted = u64_to_bytes(total_minted());
    let transfers = u64_to_bytes(
        storage_get(MP_TRANSFER_COUNT_KEY).map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 }).unwrap_or(0)
    );
    let burns = u64_to_bytes(
        storage_get(MP_BURN_COUNT_KEY).map(|d| if d.len() >= 8 { bytes_to_u64(&d) } else { 0 }).unwrap_or(0)
    );
    buf[0..8].copy_from_slice(&minted);
    buf[8..16].copy_from_slice(&transfers);
    buf[16..24].copy_from_slice(&burns);
    moltchain_sdk::set_return_data(&buf);
    0
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use moltchain_sdk::test_mock;

    fn setup() {
        test_mock::reset();
    }

    #[test]
    fn test_initialize() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let stored = test_mock::get_storage(b"minter");
        assert_eq!(stored, Some(minter.to_vec()));
        assert_eq!(test_mock::get_storage(b"collection_name"), Some(b"MoltPunks".to_vec()));
        assert_eq!(test_mock::get_storage(b"collection_symbol"), Some(b"MPNK".to_vec()));
    }

    #[test]
    fn test_mint() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let to = [2u8; 32];
        let metadata = b"ipfs://QmTest123";
        test_mock::set_caller(minter);
        assert_eq!(mint(minter.as_ptr(), to.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32), 1);
        assert_eq!(total_minted(), 1);
    }

    #[test]
    fn test_mint_unauthorized() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let other = [2u8; 32];
        let to = [3u8; 32];
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(other);
        assert_eq!(mint(other.as_ptr(), to.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32), 0);
    }

    #[test]
    fn test_mint_duplicate() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let to = [2u8; 32];
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(minter);
        mint(minter.as_ptr(), to.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32);
        assert_eq!(mint(minter.as_ptr(), to.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32), 0);
    }

    #[test]
    fn test_transfer() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let from = [2u8; 32];
        let to = [3u8; 32];
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(minter);
        mint(minter.as_ptr(), from.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(from);
        assert_eq!(transfer(from.as_ptr(), to.as_ptr(), 1), 1);
    }

    #[test]
    fn test_transfer_not_owner() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let other = [3u8; 32];
        let to = [4u8; 32];
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(minter);
        mint(minter.as_ptr(), owner.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32);
        assert_eq!(transfer(other.as_ptr(), to.as_ptr(), 1), 0);
    }

    #[test]
    fn test_owner_of() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(minter);
        mint(minter.as_ptr(), owner.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32);
        let mut out = [0u8; 32];
        assert_eq!(owner_of(1, out.as_mut_ptr()), 1);
        assert_eq!(out, owner);
    }

    #[test]
    fn test_owner_of_nonexistent() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let mut out = [0u8; 32];
        assert_eq!(owner_of(999, out.as_mut_ptr()), 0);
    }

    #[test]
    fn test_balance_of() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let metadata = b"ipfs://QmTest";
        assert_eq!(balance_of(owner.as_ptr()), 0);
        test_mock::set_caller(minter);
        mint(minter.as_ptr(), owner.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32);
        assert_eq!(balance_of(owner.as_ptr()), 1);
        mint(minter.as_ptr(), owner.as_ptr(), 2, metadata.as_ptr(), metadata.len() as u32);
        assert_eq!(balance_of(owner.as_ptr()), 2);
    }

    #[test]
    fn test_approve() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let spender = [3u8; 32];
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(minter);
        mint(minter.as_ptr(), owner.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        assert_eq!(approve(owner.as_ptr(), spender.as_ptr(), 1), 1);
    }

    #[test]
    fn test_approve_not_owner() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let other = [3u8; 32];
        let spender = [4u8; 32];
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(minter);
        mint(minter.as_ptr(), owner.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32);
        assert_eq!(approve(other.as_ptr(), spender.as_ptr(), 1), 0);
    }

    #[test]
    fn test_transfer_from() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let spender = [3u8; 32];
        let to = [4u8; 32];
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(minter);
        mint(minter.as_ptr(), owner.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32);
        // AUDIT-FIX P2: Set caller for security check on approve
        test_mock::set_caller(owner);
        approve(owner.as_ptr(), spender.as_ptr(), 1);
        assert_eq!(transfer_from(spender.as_ptr(), owner.as_ptr(), to.as_ptr(), 1), 1);
        // Verify new owner
        let mut out = [0u8; 32];
        owner_of(1, out.as_mut_ptr());
        assert_eq!(out, to);
    }

    #[test]
    fn test_transfer_from_not_approved() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let other = [3u8; 32];
        let to = [4u8; 32];
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(minter);
        mint(minter.as_ptr(), owner.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32);
        assert_eq!(transfer_from(other.as_ptr(), owner.as_ptr(), to.as_ptr(), 1), 0);
    }

    #[test]
    fn test_burn() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(minter);
        mint(minter.as_ptr(), owner.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32);
        // AUDIT-FIX P2: Set caller for security check
        test_mock::set_caller(owner);
        assert_eq!(burn(owner.as_ptr(), 1), 1);
        let mut out = [0u8; 32];
        assert_eq!(owner_of(1, out.as_mut_ptr()), 0);
    }

    #[test]
    fn test_burn_not_owner() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let other = [3u8; 32];
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(minter);
        mint(minter.as_ptr(), owner.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32);
        assert_eq!(burn(other.as_ptr(), 1), 0);
    }

    #[test]
    fn test_burn_nonexistent() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        assert_eq!(burn(owner.as_ptr(), 999), 0);
    }

    // AUDIT-FIX P2: Security regression test
    #[test]
    fn test_mint_when_paused() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        // Pause the contract
        test_mock::set_caller(minter);
        assert_eq!(mp_pause(minter.as_ptr()), 1);
        // Attempt to mint while paused → should fail
        let to = [2u8; 32];
        let metadata = b"ipfs://QmTest";
        assert_eq!(mint(minter.as_ptr(), to.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32), 0);
    }

    // AUDIT-FIX P2: Security regression test
    #[test]
    fn test_transfer_when_paused() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let to = [3u8; 32];
        let metadata = b"ipfs://QmTest";
        // Mint a token first
        test_mock::set_caller(minter);
        assert_eq!(mint(minter.as_ptr(), owner.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32), 1);
        // Pause the contract
        assert_eq!(mp_pause(minter.as_ptr()), 1);
        // Attempt to transfer while paused → should fail
        test_mock::set_caller(owner);
        assert_eq!(transfer(owner.as_ptr(), to.as_ptr(), 1), 0);
    }

    // AUDIT-FIX P2: Security regression test
    #[test]
    fn test_approve_wrong_caller() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let spender = [3u8; 32];
        let attacker = [4u8; 32];
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(minter);
        assert_eq!(mint(minter.as_ptr(), owner.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32), 1);
        // set_caller differs from owner arg → should fail
        test_mock::set_caller(attacker);
        assert_eq!(approve(owner.as_ptr(), spender.as_ptr(), 1), 0);
    }

    // AUDIT-FIX P2: Security regression test
    #[test]
    fn test_burn_wrong_caller() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let attacker = [4u8; 32];
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(minter);
        assert_eq!(mint(minter.as_ptr(), owner.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32), 1);
        // set_caller differs from owner arg → should fail
        test_mock::set_caller(attacker);
        assert_eq!(burn(owner.as_ptr(), 1), 0);
    }
}
