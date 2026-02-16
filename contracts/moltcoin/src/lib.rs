// MoltCoin Token Contract
// Example MT-20 fungible token

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]
#![allow(dead_code)]
#![allow(unused_imports)]

use moltchain_sdk::{Token, Address, log_info, storage_get, storage_set, bytes_to_u64, u64_to_bytes, get_caller};

/// Read the contract owner from persistent storage.
fn get_owner() -> Address {
    match storage_get(b"owner") {
        Some(bytes) if bytes.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Address::new(arr)
        }
        _ => panic!(),
    }
}

/// Build a lightweight Token handle.
/// All mutable state (balances, allowances, total_supply) lives in storage.
fn make_token() -> Token {
    Token::new("MoltCoin", "MOLT", 9)
}

/// Initialize the token contract
#[no_mangle]
pub extern "C" fn initialize(owner_ptr: *const u8) {
    // Re-initialization guard: reject if owner is already set
    if storage_get(b"owner").is_some() {
        log_info("MoltCoin already initialized — ignoring");
        return;
    }

    let mut owner_array = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner_array.as_mut_ptr(), 32); }
    let owner = Address::new(owner_array);

    // Persist owner to storage
    storage_set(b"owner", &owner.0);

    // Store token metadata in storage for discoverability
    storage_set(b"token_name", b"MoltCoin");
    storage_set(b"token_symbol", b"MOLT");
    storage_set(b"token_decimals", &[9u8]);

    // Initialize with 1 million tokens
    let initial_supply = 1_000_000 * 1_000_000_000; // 1M with 9 decimals
    let mut token = make_token();
    token.initialize(initial_supply, owner).expect("Initialization failed");

    log_info("MoltCoin initialized");
}

/// Get balance of an account
#[no_mangle]
pub extern "C" fn balance_of(account_ptr: *const u8) -> u64 {
    let mut account_array = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(account_ptr, account_array.as_mut_ptr(), 32); }
    let account = Address::new(account_array);

    make_token().balance_of(account)
}

/// Transfer tokens
/// AUDIT-FIX 1.8a: verify caller == from to prevent unauthorized transfers
#[no_mangle]
pub extern "C" fn transfer(from_ptr: *const u8, to_ptr: *const u8, amount: u64) -> u32 {
    let mut from_array = [0u8; 32];
    let mut to_array = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(from_ptr, from_array.as_mut_ptr(), 32); }
    unsafe { core::ptr::copy_nonoverlapping(to_ptr, to_array.as_mut_ptr(), 32); }

    // AUDIT-FIX 1.8a: Only the account owner can initiate transfers
    let caller = get_caller();
    if caller.0 != from_array {
        log_info("Transfer rejected: caller is not the sender");
        return 0;
    }

    let from = Address::new(from_array);
    let to = Address::new(to_array);

    match make_token().transfer(from, to, amount) {
        Ok(_) => {
            log_info("Transfer successful");
            1
        }
        Err(_) => {
            log_info("Transfer failed");
            0
        }
    }
}

/// Mint new tokens (owner only)
#[no_mangle]
pub extern "C" fn mint(caller_ptr: *const u8, to_ptr: *const u8, amount: u64) -> u32 {
    let mut caller_array = [0u8; 32];
    let mut to_array = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(caller_ptr, caller_array.as_mut_ptr(), 32); }
    unsafe { core::ptr::copy_nonoverlapping(to_ptr, to_array.as_mut_ptr(), 32); }

    let caller = Address::new(caller_array);
    let to = Address::new(to_array);
    let owner = get_owner();

    let mut token = make_token();
    match token.mint(to, amount, caller, owner) {
        Ok(_) => {
            log_info("Mint successful");
            1
        }
        Err(_) => {
            log_info("Mint failed - unauthorized");
            0
        }
    }
}

/// Burn tokens
/// AUDIT-FIX 1.8b: verify caller == from to prevent unauthorized burns
#[no_mangle]
pub extern "C" fn burn(from_ptr: *const u8, amount: u64) -> u32 {
    let mut from_array = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(from_ptr, from_array.as_mut_ptr(), 32); }

    // AUDIT-FIX 1.8b: Only the account owner can burn their tokens
    let caller = get_caller();
    if caller.0 != from_array {
        log_info("Burn rejected: caller is not the token owner");
        return 0;
    }

    let from = Address::new(from_array);

    let mut token = make_token();
    match token.burn(from, amount) {
        Ok(_) => {
            log_info("Burn successful");
            1
        }
        Err(_) => {
            log_info("Burn failed");
            0
        }
    }
}

/// Approve spender
#[no_mangle]
pub extern "C" fn approve(owner_ptr: *const u8, spender_ptr: *const u8, amount: u64) -> u32 {
    let mut owner_array = [0u8; 32];
    let mut spender_array = [0u8; 32];
    unsafe { core::ptr::copy_nonoverlapping(owner_ptr, owner_array.as_mut_ptr(), 32); }
    unsafe { core::ptr::copy_nonoverlapping(spender_ptr, spender_array.as_mut_ptr(), 32); }

    let owner = Address::new(owner_array);
    let spender = Address::new(spender_array);

    match make_token().approve(owner, spender, amount) {
        Ok(_) => {
            log_info("Approval successful");
            1
        }
        Err(_) => 0,
    }
}

/// Get total supply (read from persistent storage)
#[no_mangle]
pub extern "C" fn total_supply() -> u64 {
    match storage_get(b"total_supply") {
        Some(bytes) => bytes_to_u64(&bytes),
        None => 0,
    }
}

// Build instructions:
// cargo build --target wasm32-unknown-unknown --release
// The WASM will be at: target/wasm32-unknown-unknown/release/moltcoin_token.wasm

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
        let owner = [1u8; 32];
        initialize(owner.as_ptr());

        // Check owner stored
        let stored_owner = test_mock::get_storage(b"owner");
        assert_eq!(stored_owner, Some(owner.to_vec()));

        // Check token metadata
        assert_eq!(test_mock::get_storage(b"token_name"), Some(b"MoltCoin".to_vec()));
        assert_eq!(test_mock::get_storage(b"token_symbol"), Some(b"MOLT".to_vec()));
        assert_eq!(test_mock::get_storage(b"token_decimals"), Some([9u8].to_vec()));

        // Check total supply (1M * 10^9 = 1_000_000_000_000_000)
        let supply_bytes = test_mock::get_storage(b"total_supply").unwrap();
        let supply = bytes_to_u64(&supply_bytes);
        assert_eq!(supply, 1_000_000 * 1_000_000_000);
    }

    #[test]
    fn test_balance_of_owner_after_init() {
        setup();
        let owner = [1u8; 32];
        initialize(owner.as_ptr());

        let bal = balance_of(owner.as_ptr());
        assert_eq!(bal, 1_000_000 * 1_000_000_000);
    }

    #[test]
    fn test_transfer() {
        setup();
        let owner = [1u8; 32];
        let recipient = [2u8; 32];
        initialize(owner.as_ptr());

        test_mock::set_caller(owner);
        let amount: u64 = 500_000_000; // 0.5 MOLT
        let result = transfer(owner.as_ptr(), recipient.as_ptr(), amount);
        assert_eq!(result, 1); // success

        let owner_bal = balance_of(owner.as_ptr());
        let recip_bal = balance_of(recipient.as_ptr());
        assert_eq!(recip_bal, amount);
        assert_eq!(owner_bal, 1_000_000 * 1_000_000_000 - amount);
    }

    #[test]
    fn test_transfer_insufficient_funds() {
        setup();
        let owner = [1u8; 32];
        let recipient = [2u8; 32];
        initialize(owner.as_ptr());

        // Try to transfer more than balance from recipient (who has 0)
        test_mock::set_caller(recipient);
        let result = transfer(recipient.as_ptr(), owner.as_ptr(), 100);
        assert_eq!(result, 0); // failure
    }

    #[test]
    fn test_mint() {
        setup();
        let owner = [1u8; 32];
        let recipient = [3u8; 32];
        initialize(owner.as_ptr());

        let mint_amount: u64 = 1_000_000_000;
        let result = mint(owner.as_ptr(), recipient.as_ptr(), mint_amount);
        assert_eq!(result, 1); // success

        let recip_bal = balance_of(recipient.as_ptr());
        assert_eq!(recip_bal, mint_amount);

        // Total supply should increase
        let supply = total_supply();
        assert_eq!(supply, 1_000_000 * 1_000_000_000 + mint_amount);
    }

    #[test]
    fn test_mint_unauthorized() {
        setup();
        let owner = [1u8; 32];
        let other = [2u8; 32];
        let recipient = [3u8; 32];
        initialize(owner.as_ptr());

        // Non-owner tries to mint
        let result = mint(other.as_ptr(), recipient.as_ptr(), 100);
        assert_eq!(result, 0); // failure
    }

    #[test]
    fn test_burn() {
        setup();
        let owner = [1u8; 32];
        initialize(owner.as_ptr());

        test_mock::set_caller(owner);
        let burn_amount: u64 = 100_000_000_000;
        let result = burn(owner.as_ptr(), burn_amount);
        assert_eq!(result, 1);

        let bal = balance_of(owner.as_ptr());
        assert_eq!(bal, 1_000_000 * 1_000_000_000 - burn_amount);

        let supply = total_supply();
        assert_eq!(supply, 1_000_000 * 1_000_000_000 - burn_amount);
    }

    #[test]
    fn test_burn_insufficient() {
        setup();
        let owner = [1u8; 32];
        let nobody = [9u8; 32];
        initialize(owner.as_ptr());

        // Try to burn from account with 0 balance
        test_mock::set_caller(nobody);
        let result = burn(nobody.as_ptr(), 100);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_approve() {
        setup();
        let owner = [1u8; 32];
        let spender = [2u8; 32];
        initialize(owner.as_ptr());

        let result = approve(owner.as_ptr(), spender.as_ptr(), 5000);
        assert_eq!(result, 1);
    }
}
