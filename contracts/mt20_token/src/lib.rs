#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use lichen_sdk::{get_caller, log_info, storage_get, storage_set, Address, ContractError, Token};

const OWNER_KEY: &[u8] = b"mt20_owner";
const TOKEN_NAME: &str = "Lichen MT-20";
const TOKEN_SYMBOL: &str = "MT20";
const TOKEN_DECIMALS: u8 = 9;
const TOKEN_PREFIX: &str = "mt20";

fn token() -> Token {
    Token::new(TOKEN_NAME, TOKEN_SYMBOL, TOKEN_DECIMALS, TOKEN_PREFIX)
}

fn read_address(ptr: *const u8) -> [u8; 32] {
    let mut out = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(ptr, out.as_mut_ptr(), 32);
    }
    out
}

fn is_zero_address(address: &[u8; 32]) -> bool {
    address.iter().all(|&byte| byte == 0)
}

fn load_owner() -> Option<[u8; 32]> {
    storage_get(OWNER_KEY).and_then(|bytes| {
        if bytes.len() < 32 {
            return None;
        }
        let mut owner = [0u8; 32];
        owner.copy_from_slice(&bytes[..32]);
        Some(owner)
    })
}

fn require_owner() -> Result<[u8; 32], u32> {
    load_owner().ok_or(3)
}

fn map_contract_error(err: ContractError) -> u32 {
    match err {
        ContractError::Unauthorized => 1,
        ContractError::InsufficientFunds => 2,
        ContractError::Overflow => 4,
        ContractError::InvalidInput => 5,
        ContractError::StorageError => 6,
        ContractError::Custom(_) => 7,
    }
}

#[no_mangle]
pub extern "C" fn initialize(owner_ptr: *const u8) -> u32 {
    let owner = read_address(owner_ptr);
    if is_zero_address(&owner) {
        return 2;
    }
    if get_caller().0 != owner {
        return 200;
    }
    if load_owner().is_some() {
        return 1;
    }

    storage_set(OWNER_KEY, &owner);

    let mut mt20 = token();
    match mt20.initialize(0, Address::new(owner)) {
        Ok(()) => {
            log_info("MT-20 initialized");
            0
        }
        Err(err) => map_contract_error(err),
    }
}

#[no_mangle]
pub extern "C" fn mint(caller_ptr: *const u8, to_ptr: *const u8, amount: u64) -> u32 {
    let caller = read_address(caller_ptr);
    if get_caller().0 != caller {
        return 200;
    }
    let owner = match require_owner() {
        Ok(owner) => owner,
        Err(code) => return code,
    };
    let to = read_address(to_ptr);

    let mut mt20 = token();
    match mt20.mint(
        Address::new(to),
        amount,
        Address::new(caller),
        Address::new(owner),
    ) {
        Ok(()) => {
            log_info("MT-20 mint successful");
            0
        }
        Err(err) => map_contract_error(err),
    }
}

#[no_mangle]
pub extern "C" fn burn(from_ptr: *const u8, amount: u64) -> u32 {
    let from = read_address(from_ptr);
    if get_caller().0 != from {
        return 200;
    }

    let mut mt20 = token();
    match mt20.burn(Address::new(from), amount) {
        Ok(()) => {
            log_info("MT-20 burn successful");
            0
        }
        Err(err) => map_contract_error(err),
    }
}

#[no_mangle]
pub extern "C" fn balance_of(account_ptr: *const u8) -> u64 {
    let account = read_address(account_ptr);
    token().balance_of(Address::new(account))
}

#[no_mangle]
pub extern "C" fn transfer(from_ptr: *const u8, to_ptr: *const u8, amount: u64) -> u32 {
    let from = read_address(from_ptr);
    if get_caller().0 != from {
        return 200;
    }
    let to = read_address(to_ptr);

    match token().transfer(Address::new(from), Address::new(to), amount) {
        Ok(()) => {
            log_info("MT-20 transfer successful");
            0
        }
        Err(err) => map_contract_error(err),
    }
}

#[no_mangle]
pub extern "C" fn approve(owner_ptr: *const u8, spender_ptr: *const u8, amount: u64) -> u32 {
    let owner = read_address(owner_ptr);
    if get_caller().0 != owner {
        return 200;
    }
    let spender = read_address(spender_ptr);

    match token().approve(Address::new(owner), Address::new(spender), amount) {
        Ok(()) => {
            log_info("MT-20 approve successful");
            0
        }
        Err(err) => map_contract_error(err),
    }
}

#[no_mangle]
pub extern "C" fn allowance(owner_ptr: *const u8, spender_ptr: *const u8) -> u64 {
    let owner = read_address(owner_ptr);
    let spender = read_address(spender_ptr);
    token().allowance(Address::new(owner), Address::new(spender))
}

#[no_mangle]
pub extern "C" fn transfer_from(
    caller_ptr: *const u8,
    from_ptr: *const u8,
    to_ptr: *const u8,
    amount: u64,
) -> u32 {
    let caller = read_address(caller_ptr);
    if get_caller().0 != caller {
        return 200;
    }
    let from = read_address(from_ptr);
    let to = read_address(to_ptr);

    match token().transfer_from(
        Address::new(caller),
        Address::new(from),
        Address::new(to),
        amount,
    ) {
        Ok(()) => {
            log_info("MT-20 transfer_from successful");
            0
        }
        Err(err) => map_contract_error(err),
    }
}

#[no_mangle]
pub extern "C" fn total_supply() -> u64 {
    token().get_total_supply()
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use lichen_sdk::test_mock;

    fn addr(id: u8) -> [u8; 32] {
        let mut out = [0u8; 32];
        out[0] = id;
        out
    }

    #[test]
    fn test_initialize_sets_owner_and_zero_supply() {
        test_mock::reset();
        let owner = addr(1);
        test_mock::set_caller(owner);

        assert_eq!(initialize(owner.as_ptr()), 0);
        assert_eq!(load_owner(), Some(owner));
        assert_eq!(total_supply(), 0);
    }

    #[test]
    fn test_initialize_caller_mismatch_fails() {
        test_mock::reset();
        let owner = addr(1);
        test_mock::set_caller(addr(9));

        assert_eq!(initialize(owner.as_ptr()), 200);
        assert_eq!(load_owner(), None);
    }

    #[test]
    fn test_initialize_zero_owner_fails() {
        test_mock::reset();
        let owner = [0u8; 32];
        test_mock::set_caller(owner);

        assert_eq!(initialize(owner.as_ptr()), 2);
        assert_eq!(load_owner(), None);
    }

    #[test]
    fn test_owner_can_mint_and_transfer() {
        test_mock::reset();
        let owner = addr(1);
        let recipient = addr(2);
        test_mock::set_caller(owner);

        assert_eq!(initialize(owner.as_ptr()), 0);
        assert_eq!(mint(owner.as_ptr(), owner.as_ptr(), 1_000), 0);
        assert_eq!(balance_of(owner.as_ptr()), 1_000);
        assert_eq!(total_supply(), 1_000);

        assert_eq!(transfer(owner.as_ptr(), recipient.as_ptr(), 250), 0);
        assert_eq!(balance_of(owner.as_ptr()), 750);
        assert_eq!(balance_of(recipient.as_ptr()), 250);
        assert_eq!(total_supply(), 1_000);
    }

    #[test]
    fn test_non_owner_cannot_mint() {
        test_mock::reset();
        let owner = addr(1);
        let attacker = addr(9);
        let recipient = addr(2);
        test_mock::set_caller(owner);
        assert_eq!(initialize(owner.as_ptr()), 0);

        test_mock::set_caller(attacker);
        assert_eq!(mint(attacker.as_ptr(), recipient.as_ptr(), 1_000), 1);
        assert_eq!(balance_of(recipient.as_ptr()), 0);
        assert_eq!(total_supply(), 0);
    }

    #[test]
    fn test_self_transfer_does_not_inflate_balance() {
        test_mock::reset();
        let owner = addr(1);
        test_mock::set_caller(owner);
        assert_eq!(initialize(owner.as_ptr()), 0);
        assert_eq!(mint(owner.as_ptr(), owner.as_ptr(), 500), 0);

        assert_eq!(transfer(owner.as_ptr(), owner.as_ptr(), 200), 5);
        assert_eq!(balance_of(owner.as_ptr()), 500);
        assert_eq!(total_supply(), 500);
    }

    #[test]
    fn test_transfer_from_failure_preserves_allowance() {
        test_mock::reset();
        let owner = addr(1);
        let spender = addr(2);
        let recipient = addr(3);
        test_mock::set_caller(owner);
        assert_eq!(initialize(owner.as_ptr()), 0);
        assert_eq!(mint(owner.as_ptr(), owner.as_ptr(), 50), 0);
        assert_eq!(approve(owner.as_ptr(), spender.as_ptr(), 100), 0);

        test_mock::set_caller(spender);
        assert_eq!(
            transfer_from(spender.as_ptr(), owner.as_ptr(), recipient.as_ptr(), 60),
            2
        );
        assert_eq!(allowance(owner.as_ptr(), spender.as_ptr()), 100);
        assert_eq!(balance_of(owner.as_ptr()), 50);
        assert_eq!(balance_of(recipient.as_ptr()), 0);
    }

    #[test]
    fn test_transfer_respects_can_transfer_compliance() {
        test_mock::reset();
        let owner = addr(1);
        let recipient = addr(2);
        test_mock::set_caller(owner);
        assert_eq!(initialize(owner.as_ptr()), 0);
        assert_eq!(mint(owner.as_ptr(), owner.as_ptr(), 500), 0);

        test_mock::set_can_transfer(false);
        assert_eq!(transfer(owner.as_ptr(), recipient.as_ptr(), 100), 7);
        assert_eq!(balance_of(owner.as_ptr()), 500);
        assert_eq!(balance_of(recipient.as_ptr()), 0);
        assert_eq!(total_supply(), 500);
    }

    #[test]
    fn test_transfer_from_respects_can_transfer_and_preserves_allowance() {
        test_mock::reset();
        let owner = addr(1);
        let spender = addr(2);
        let recipient = addr(3);
        test_mock::set_caller(owner);
        assert_eq!(initialize(owner.as_ptr()), 0);
        assert_eq!(mint(owner.as_ptr(), owner.as_ptr(), 500), 0);
        assert_eq!(approve(owner.as_ptr(), spender.as_ptr(), 250), 0);

        test_mock::set_caller(spender);
        test_mock::set_can_transfer(false);
        assert_eq!(
            transfer_from(spender.as_ptr(), owner.as_ptr(), recipient.as_ptr(), 100),
            7
        );
        assert_eq!(allowance(owner.as_ptr(), spender.as_ptr()), 250);
        assert_eq!(balance_of(owner.as_ptr()), 500);
        assert_eq!(balance_of(recipient.as_ptr()), 0);
    }

    #[test]
    fn test_mint_respects_can_receive_compliance() {
        test_mock::reset();
        let owner = addr(1);
        let recipient = addr(2);
        test_mock::set_caller(owner);
        assert_eq!(initialize(owner.as_ptr()), 0);

        test_mock::set_can_receive(false);
        assert_eq!(mint(owner.as_ptr(), recipient.as_ptr(), 500), 7);
        assert_eq!(balance_of(recipient.as_ptr()), 0);
        assert_eq!(total_supply(), 0);
    }

    #[test]
    fn test_burn_respects_can_send_compliance() {
        test_mock::reset();
        let owner = addr(1);
        test_mock::set_caller(owner);
        assert_eq!(initialize(owner.as_ptr()), 0);
        assert_eq!(mint(owner.as_ptr(), owner.as_ptr(), 500), 0);

        test_mock::set_can_send(false);
        assert_eq!(burn(owner.as_ptr(), 100), 7);
        assert_eq!(balance_of(owner.as_ptr()), 500);
        assert_eq!(total_supply(), 500);
    }

    #[test]
    fn test_burn_reduces_supply() {
        test_mock::reset();
        let owner = addr(1);
        test_mock::set_caller(owner);
        assert_eq!(initialize(owner.as_ptr()), 0);
        assert_eq!(mint(owner.as_ptr(), owner.as_ptr(), 500), 0);

        assert_eq!(burn(owner.as_ptr(), 200), 0);
        assert_eq!(balance_of(owner.as_ptr()), 300);
        assert_eq!(total_supply(), 300);
    }
}
