// LichenCoin Token Contract
// Example MT-20 fungible token

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::too_many_arguments)]
#![allow(dead_code)]
#![allow(unused_imports)]

use lichen_sdk::{
    bytes_to_u64, get_caller, log_info, storage_get, storage_set, u64_to_bytes, Address, Token,
};

// AUDIT-FIX: Reentrancy guard
const REENTRANCY_KEY: &[u8] = b"licn_reentrancy";

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

const TOKEN_NAME: &[u8] = b"LichenCoin";
const TOKEN_SYMBOL: &[u8] = b"LICN";
const DECIMALS: u8 = 9;
const ADMIN_KEY: &[u8] = b"licn_admin";

/// Read the contract admin from persistent storage.
fn get_owner() -> Address {
    match storage_get(ADMIN_KEY) {
        Some(bytes) if bytes.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            Address::new(arr)
        }
        _ => Address::new([0u8; 32]),
    }
}

/// Build a lightweight Token handle.
/// All mutable state (balances, allowances, total_supply) lives in storage.
fn make_token() -> Token {
    Token::new("LichenCoin", "LICN", 9, "licn")
}

fn init_owner_matches_signer(owner: &[u8; 32]) -> bool {
    let caller = lichen_sdk::get_caller();
    if caller.0 == *owner {
        return true;
    }

    #[cfg(test)]
    {
        return caller.0 == [0u8; 32];
    }

    #[cfg(not(test))]
    {
        false
    }
}

/// Initialize the token contract
#[no_mangle]
pub extern "C" fn initialize(owner_ptr: *const u8) -> u32 {
    // Re-initialization guard: reject if admin is already set
    if storage_get(ADMIN_KEY).is_some() {
        log_info("LichenCoin already initialized — ignoring");
        return 0;
    }

    let mut owner_array = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(owner_ptr, owner_array.as_mut_ptr(), 32);
    }
    if !init_owner_matches_signer(&owner_array) {
        log_info("LichenCoin initialize rejected: caller mismatch");
        return 0;
    }
    let owner = Address::new(owner_array);

    // Persist admin to storage (unified key: licn_admin)
    storage_set(ADMIN_KEY, &owner.0);

    // Mint initial supply: 500M LICN = 500_000_000 * 10^9 spores
    let initial_supply: u64 = 500_000_000_000_000_000;
    let mut token = make_token();
    if token.initialize(initial_supply, owner).is_err() {
        log_info("LichenCoin initialization failed");
        return 0;
    }

    log_info("LichenCoin initialized");
    1
}

/// Get balance of an account
#[no_mangle]
pub extern "C" fn balance_of(account_ptr: *const u8) -> u64 {
    let mut account_array = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(account_ptr, account_array.as_mut_ptr(), 32);
    }
    let account = Address::new(account_array);

    make_token().balance_of(account)
}

/// Get allowance granted by owner to spender
#[no_mangle]
pub extern "C" fn allowance(owner_ptr: *const u8, spender_ptr: *const u8) -> u64 {
    let mut owner_array = [0u8; 32];
    let mut spender_array = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(owner_ptr, owner_array.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(spender_ptr, spender_array.as_mut_ptr(), 32);
    }
    let owner = Address::new(owner_array);
    let spender = Address::new(spender_array);

    make_token().allowance(owner, spender)
}

/// Transfer tokens
/// AUDIT-FIX 1.8a: verify caller == from to prevent unauthorized transfers
#[no_mangle]
pub extern "C" fn transfer(from_ptr: *const u8, to_ptr: *const u8, amount: u64) -> u32 {
    if !reentrancy_enter() {
        return 0;
    }
    let mut from_array = [0u8; 32];
    let mut to_array = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(from_ptr, from_array.as_mut_ptr(), 32);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(to_ptr, to_array.as_mut_ptr(), 32);
    }

    // AUDIT-FIX 1.8a: Only the account owner can initiate transfers
    let caller = get_caller();
    if caller.0 != from_array {
        log_info("Transfer rejected: caller is not the sender");
        reentrancy_exit();
        return 0;
    }

    let from = Address::new(from_array);
    let to = Address::new(to_array);

    let result = match make_token().transfer(from, to, amount) {
        Ok(_) => {
            log_info("Transfer successful");
            1
        }
        Err(_) => {
            log_info("Transfer failed");
            0
        }
    };
    reentrancy_exit();
    result
}

/// Mint new tokens (owner only)
/// AUDIT-FIX GX-04: This function exists for the WASM token contract layer.
/// The native LICN supply is inflationary (4% initial rate, decaying to 0.15% floor)
/// with 40% fee burn as counter-pressure. Genesis supply is 500M LICN.
/// Protocol-level minting (block rewards) happens at the state layer, not here.
/// This contract mint is restricted to the contract owner.
#[no_mangle]
pub extern "C" fn mint(caller_ptr: *const u8, to_ptr: *const u8, amount: u64) -> u32 {
    if !reentrancy_enter() {
        return 0;
    }
    let mut caller_array = [0u8; 32];
    let mut to_array = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(caller_ptr, caller_array.as_mut_ptr(), 32);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(to_ptr, to_array.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify transaction signer matches claimed caller
    let real_caller = get_caller();
    if real_caller.0 != caller_array {
        log_info("Mint rejected: caller mismatch");
        reentrancy_exit();
        return 0;
    }

    let caller = Address::new(caller_array);
    let to = Address::new(to_array);
    let owner = get_owner();

    let mut token = make_token();
    let result = match token.mint(to, amount, caller, owner) {
        Ok(_) => {
            log_info("Mint successful");
            1
        }
        Err(_) => {
            log_info("Mint failed - unauthorized");
            0
        }
    };
    reentrancy_exit();
    result
}

/// Burn tokens
/// AUDIT-FIX 1.8b: verify caller == from to prevent unauthorized burns
#[no_mangle]
pub extern "C" fn burn(from_ptr: *const u8, amount: u64) -> u32 {
    if !reentrancy_enter() {
        return 0;
    }
    let mut from_array = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(from_ptr, from_array.as_mut_ptr(), 32);
    }

    // AUDIT-FIX 1.8b: Only the account owner can burn their tokens
    let caller = get_caller();
    if caller.0 != from_array {
        log_info("Burn rejected: caller is not the token owner");
        reentrancy_exit();
        return 0;
    }

    let from = Address::new(from_array);

    let mut token = make_token();
    let result = match token.burn(from, amount) {
        Ok(_) => {
            log_info("Burn successful");
            1
        }
        Err(_) => {
            log_info("Burn failed");
            0
        }
    };
    reentrancy_exit();
    result
}

/// Approve spender
#[no_mangle]
pub extern "C" fn approve(owner_ptr: *const u8, spender_ptr: *const u8, amount: u64) -> u32 {
    if !reentrancy_enter() {
        return 0;
    }
    let mut owner_array = [0u8; 32];
    let mut spender_array = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(owner_ptr, owner_array.as_mut_ptr(), 32);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(spender_ptr, spender_array.as_mut_ptr(), 32);
    }

    // AUDIT-FIX: verify transaction signer matches claimed owner
    let real_caller = get_caller();
    if real_caller.0 != owner_array {
        log_info("Approve rejected: caller is not the owner");
        reentrancy_exit();
        return 0;
    }

    let owner = Address::new(owner_array);
    let spender = Address::new(spender_array);

    let result = match make_token().approve(owner, spender, amount) {
        Ok(_) => {
            log_info("Approval successful");
            1
        }
        Err(_) => 0,
    };
    reentrancy_exit();
    result
}

/// Transfer from another account using allowance
/// AUDIT-FIX P2: Missing function — approve was dead code without this
#[no_mangle]
pub extern "C" fn transfer_from(
    spender_ptr: *const u8,
    from_ptr: *const u8,
    to_ptr: *const u8,
    amount: u64,
) -> u32 {
    if !reentrancy_enter() {
        return 0;
    }
    let mut spender_array = [0u8; 32];
    let mut from_array = [0u8; 32];
    let mut to_array = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(spender_ptr, spender_array.as_mut_ptr(), 32);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(from_ptr, from_array.as_mut_ptr(), 32);
    }
    unsafe {
        core::ptr::copy_nonoverlapping(to_ptr, to_array.as_mut_ptr(), 32);
    }

    // Verify caller matches spender
    let caller = get_caller();
    if caller.0 != spender_array {
        log_info("TransferFrom rejected: caller mismatch");
        reentrancy_exit();
        return 0;
    }

    let spender = Address::new(spender_array);
    let from = Address::new(from_array);
    let to = Address::new(to_array);

    let token = make_token();
    let result = match token.transfer_from(spender, from, to, amount) {
        Ok(_) => {
            log_info("TransferFrom successful");
            1
        }
        Err(_) => {
            log_info("TransferFrom failed");
            0
        }
    };
    reentrancy_exit();
    result
}

/// Get total supply (read from persistent storage)
#[no_mangle]
pub extern "C" fn total_supply() -> u64 {
    make_token().get_total_supply()
}

// Build instructions:
// cargo build --target wasm32-unknown-unknown --release
// The WASM will be at: target/wasm32-unknown-unknown/release/lichencoin_token.wasm

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use lichen_sdk::bytes_to_u64;
    use lichen_sdk::test_mock;

    fn setup() {
        test_mock::reset();
    }

    #[test]
    fn test_initialize() {
        setup();
        let owner = [1u8; 32];
        let result = initialize(owner.as_ptr());
        assert_eq!(result, 1); // success

        // Check admin stored under unified key
        let stored_admin = test_mock::get_storage(ADMIN_KEY);
        assert_eq!(stored_admin, Some(owner.to_vec()));

        // Check total supply via unified key (licn_supply)
        let supply_bytes = test_mock::get_storage(b"licn_supply").unwrap();
        let supply = bytes_to_u64(&supply_bytes);
        assert_eq!(supply, 500_000_000_000_000_000); // 500M LICN
    }

    #[test]
    fn test_initialize_rejects_caller_mismatch() {
        setup();
        let owner = [1u8; 32];
        test_mock::set_caller([9u8; 32]);
        let result = initialize(owner.as_ptr());
        assert_eq!(result, 0);
        assert_eq!(test_mock::get_storage(ADMIN_KEY), None);
    }

    #[test]
    fn test_balance_of_owner_after_init() {
        setup();
        let owner = [1u8; 32];
        let result = initialize(owner.as_ptr());
        assert_eq!(result, 1);

        let bal = balance_of(owner.as_ptr());
        assert_eq!(bal, 500_000_000_000_000_000); // 500M LICN
    }

    #[test]
    fn test_transfer() {
        setup();
        let owner = [1u8; 32];
        let recipient = [2u8; 32];
        let _ = initialize(owner.as_ptr());

        test_mock::set_caller(owner);
        let amount: u64 = 500_000_000; // 0.5 LICN
        let result = transfer(owner.as_ptr(), recipient.as_ptr(), amount);
        assert_eq!(result, 1); // success

        let owner_bal = balance_of(owner.as_ptr());
        let recip_bal = balance_of(recipient.as_ptr());
        assert_eq!(recip_bal, amount);
        assert_eq!(owner_bal, 500_000_000_000_000_000 - amount);
    }

    #[test]
    fn test_transfer_insufficient_funds() {
        setup();
        let owner = [1u8; 32];
        let recipient = [2u8; 32];
        let _ = initialize(owner.as_ptr());

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
        let _ = initialize(owner.as_ptr());

        test_mock::set_caller(owner);
        let mint_amount: u64 = 1_000_000_000;
        let result = mint(owner.as_ptr(), recipient.as_ptr(), mint_amount);
        assert_eq!(result, 1); // success

        let recip_bal = balance_of(recipient.as_ptr());
        assert_eq!(recip_bal, mint_amount);

        // Total supply should increase
        let supply = total_supply();
        assert_eq!(supply, 500_000_000_000_000_000 + mint_amount);
    }

    #[test]
    fn test_mint_unauthorized() {
        setup();
        let owner = [1u8; 32];
        let other = [2u8; 32];
        let recipient = [3u8; 32];
        let _ = initialize(owner.as_ptr());

        // Non-owner tries to mint
        let result = mint(other.as_ptr(), recipient.as_ptr(), 100);
        assert_eq!(result, 0); // failure
    }

    #[test]
    fn test_burn() {
        setup();
        let owner = [1u8; 32];
        let _ = initialize(owner.as_ptr());

        test_mock::set_caller(owner);
        let burn_amount: u64 = 100_000_000_000;
        let result = burn(owner.as_ptr(), burn_amount);
        assert_eq!(result, 1);

        let bal = balance_of(owner.as_ptr());
        assert_eq!(bal, 500_000_000_000_000_000 - burn_amount);

        let supply = total_supply();
        assert_eq!(supply, 500_000_000_000_000_000 - burn_amount);
    }

    #[test]
    fn test_burn_insufficient() {
        setup();
        let owner = [1u8; 32];
        let nobody = [9u8; 32];
        let _ = initialize(owner.as_ptr());

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
        let _ = initialize(owner.as_ptr());

        test_mock::set_caller(owner);
        let result = approve(owner.as_ptr(), spender.as_ptr(), 5000);
        assert_eq!(result, 1);
    }

    #[test]
    fn test_allowance_query() {
        setup();
        let owner = [1u8; 32];
        let spender = [2u8; 32];
        let _ = initialize(owner.as_ptr());

        // No approval yet — should be 0
        assert_eq!(allowance(owner.as_ptr(), spender.as_ptr()), 0);

        // Approve 5000
        test_mock::set_caller(owner);
        approve(owner.as_ptr(), spender.as_ptr(), 5000);
        assert_eq!(allowance(owner.as_ptr(), spender.as_ptr()), 5000);

        // After partial transfer_from, allowance should decrease
        test_mock::set_caller(spender);
        let recipient = [3u8; 32];
        transfer_from(spender.as_ptr(), owner.as_ptr(), recipient.as_ptr(), 2000);
        assert_eq!(allowance(owner.as_ptr(), spender.as_ptr()), 3000);
    }

    // AUDIT-FIX P2: Security regression test
    #[test]
    fn test_transfer_from_basic() {
        setup();
        let owner = [1u8; 32];
        let spender = [2u8; 32];
        let recipient = [3u8; 32];
        let _ = initialize(owner.as_ptr());

        // Owner approves spender for 100 tokens
        test_mock::set_caller(owner);
        let approve_result = approve(owner.as_ptr(), spender.as_ptr(), 100);
        assert_eq!(approve_result, 1);

        // Spender transfers 50 from owner to recipient
        test_mock::set_caller(spender);
        let result = transfer_from(spender.as_ptr(), owner.as_ptr(), recipient.as_ptr(), 50);
        assert_eq!(
            result, 1,
            "transfer_from with valid allowance should succeed"
        );

        // Verify recipient balance
        let recip_bal = balance_of(recipient.as_ptr());
        assert_eq!(recip_bal, 50);

        // Verify remaining allowance is 50
        let token = make_token();
        let remaining = token.allowance(Address::new(owner), Address::new(spender));
        assert_eq!(remaining, 50, "allowance should be reduced to 50");
    }

    // AUDIT-FIX P2: Security regression test
    #[test]
    fn test_transfer_from_exceeds_allowance() {
        setup();
        let owner = [1u8; 32];
        let spender = [2u8; 32];
        let recipient = [3u8; 32];
        let _ = initialize(owner.as_ptr());

        // Owner approves spender for 100 tokens
        test_mock::set_caller(owner);
        approve(owner.as_ptr(), spender.as_ptr(), 100);

        // Spender tries to transfer 200 — exceeds allowance
        test_mock::set_caller(spender);
        let result = transfer_from(spender.as_ptr(), owner.as_ptr(), recipient.as_ptr(), 200);
        assert_eq!(
            result, 0,
            "transfer_from must fail when exceeding allowance"
        );
    }
}
