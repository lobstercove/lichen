// MoltPunks - Collectible NFT Contract
// Example implementation of MT-721 standard

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]

use moltchain_sdk::{NFT, Address, log_info, storage_get, storage_set, bytes_to_u64, u64_to_bytes};

/// Read the minter address from persistent storage (written by NFT::initialize).
fn get_minter() -> Address {
    match storage_get(b"minter") {
        Some(bytes) if bytes.len() == 32 => {
            let mut addr = [0u8; 32];
            addr.copy_from_slice(&bytes);
            Address(addr)
        }
        _ => panic!(),
    }
}

/// Build a lightweight NFT handle.
/// All mutable state (owners, balances, approvals, total_minted) lives in storage.
fn make_nft() -> NFT {
    NFT::new("MoltPunks", "MPNK")
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
        let minter_slice = core::slice::from_raw_parts(minter_ptr, 32);
        let mut minter_addr = [0u8; 32];
        minter_addr.copy_from_slice(minter_slice);
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
    unsafe {
        // Parse caller
        let caller_slice = core::slice::from_raw_parts(caller_ptr, 32);
        let mut caller_addr = [0u8; 32];
        caller_addr.copy_from_slice(caller_slice);
        let caller = Address(caller_addr);
        
        // Check if caller is minter
        if caller.0 != get_minter().0 {
            log_info("Unauthorized: Only minter can mint");
            return 0;
        }
        
        // Parse recipient
        let to_slice = core::slice::from_raw_parts(to_ptr, 32);
        let mut to_addr = [0u8; 32];
        to_addr.copy_from_slice(to_slice);
        let to = Address(to_addr);
        
        // Parse metadata URI
        let metadata = core::slice::from_raw_parts(metadata_ptr, metadata_len as usize);
        
        // Mint
        let mut nft = make_nft();
        match nft.mint(to, token_id, metadata) {
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
    unsafe {
        // Parse from address
        let from_slice = core::slice::from_raw_parts(from_ptr, 32);
        let mut from_addr = [0u8; 32];
        from_addr.copy_from_slice(from_slice);
        let from = Address(from_addr);
        
        // Parse to address
        let to_slice = core::slice::from_raw_parts(to_ptr, 32);
        let mut to_addr = [0u8; 32];
        to_addr.copy_from_slice(to_slice);
        let to = Address(to_addr);
        
        // Transfer
        match make_nft().transfer(from, to, token_id) {
            Ok(_) => {
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
        let account_slice = core::slice::from_raw_parts(account_ptr, 32);
        let mut account_addr = [0u8; 32];
        account_addr.copy_from_slice(account_slice);
        let account = Address(account_addr);
        
        make_nft().balance_of(account)
    }
}

/// Approve spender for token
#[no_mangle]
pub extern "C" fn approve(owner_ptr: *const u8, spender_ptr: *const u8, token_id: u64) -> u32 {
    unsafe {
        let owner_slice = core::slice::from_raw_parts(owner_ptr, 32);
        let mut owner_addr = [0u8; 32];
        owner_addr.copy_from_slice(owner_slice);
        let owner = Address(owner_addr);
        
        let spender_slice = core::slice::from_raw_parts(spender_ptr, 32);
        let mut spender_addr = [0u8; 32];
        spender_addr.copy_from_slice(spender_slice);
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
        let caller_slice = core::slice::from_raw_parts(caller_ptr, 32);
        let mut caller_addr = [0u8; 32];
        caller_addr.copy_from_slice(caller_slice);
        let caller = Address(caller_addr);
        
        let from_slice = core::slice::from_raw_parts(from_ptr, 32);
        let mut from_addr = [0u8; 32];
        from_addr.copy_from_slice(from_slice);
        let from = Address(from_addr);
        
        let to_slice = core::slice::from_raw_parts(to_ptr, 32);
        let mut to_addr = [0u8; 32];
        to_addr.copy_from_slice(to_slice);
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
        let owner_slice = core::slice::from_raw_parts(owner_ptr, 32);
        let mut owner_addr = [0u8; 32];
        owner_addr.copy_from_slice(owner_slice);
        let owner = Address(owner_addr);
        
        let mut nft = make_nft();
        match nft.burn(owner, token_id) {
            Ok(_) => {
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
        assert_eq!(mint(other.as_ptr(), to.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32), 0);
    }

    #[test]
    fn test_mint_duplicate() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let to = [2u8; 32];
        let metadata = b"ipfs://QmTest";
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
        mint(minter.as_ptr(), from.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32);
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
        mint(minter.as_ptr(), owner.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32);
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
        mint(minter.as_ptr(), owner.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32);
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
        mint(minter.as_ptr(), owner.as_ptr(), 1, metadata.as_ptr(), metadata.len() as u32);
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
}
