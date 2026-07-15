#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use alloc::vec::Vec;
use lichen_sdk::{
    bytes_to_u64, call_contract, encode_layout_args, get_caller, log_info, set_return_data,
    storage_get, storage_set, u64_to_bytes, Address, ContractError, CrossCall, Token,
};

extern crate alloc;

const INITIALIZED_KEY: &[u8] = b"lpt_initialized";
const SPOREPUMP_KEY: &[u8] = b"lpt_sporepump";
const TOKEN_ID_KEY: &[u8] = b"lpt_token_id";
const CREATOR_KEY: &[u8] = b"lpt_creator";
const MAX_SUPPLY_KEY: &[u8] = b"lpt_max_supply";
const HOLDER_OBLIGATIONS_KEY: &[u8] = b"lpt_holder_obligations";
const CLAIMED_SUPPLY_KEY: &[u8] = b"lpt_claimed_supply";
const MIGRATION_INVENTORY_KEY: &[u8] = b"lpt_migration_inventory";
const TOKEN_NAME: &str = "SporePump Graduated Token";
const TOKEN_SYMBOL: &str = "SPT";
const TOKEN_DECIMALS: u8 = 9;
const TOKEN_PREFIX: &str = "lpt";

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

fn read_stored_address(key: &[u8]) -> Option<[u8; 32]> {
    storage_get(key).and_then(|data| {
        if data.len() < 32 {
            return None;
        }
        let mut address = [0u8; 32];
        address.copy_from_slice(&data[..32]);
        Some(address)
    })
}

fn load_u64(key: &[u8]) -> u64 {
    storage_get(key)
        .filter(|data| data.len() >= 8)
        .map(|data| bytes_to_u64(&data[..8]))
        .unwrap_or(0)
}

fn save_u64(key: &[u8], value: u64) {
    storage_set(key, &u64_to_bytes(value));
}

fn initialized() -> bool {
    storage_get(INITIALIZED_KEY).and_then(|data| data.first().copied()) == Some(1)
}

fn launch_metadata() -> Option<(Vec<u8>, Vec<u8>)> {
    let sporepump = read_stored_address(SPOREPUMP_KEY)?;
    let token_id = load_u64(TOKEN_ID_KEY);
    let response = call_contract(CrossCall::new(
        Address(sporepump),
        "get_token_metadata",
        u64_to_bytes(token_id).to_vec(),
    ))
    .ok()?;
    if response.len() < 4 {
        return None;
    }
    let name_len = u16::from_le_bytes(response[0..2].try_into().ok()?) as usize;
    let symbol_len_offset = 2usize.checked_add(name_len)?;
    if response.len() < symbol_len_offset + 2 {
        return None;
    }
    let symbol_len = u16::from_le_bytes(
        response[symbol_len_offset..symbol_len_offset + 2]
            .try_into()
            .ok()?,
    ) as usize;
    let symbol_offset = symbol_len_offset + 2;
    if name_len == 0
        || symbol_len == 0
        || response.len() < symbol_offset.checked_add(symbol_len)?
    {
        return None;
    }
    Some((
        response[2..2 + name_len].to_vec(),
        response[symbol_offset..symbol_offset + symbol_len].to_vec(),
    ))
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
pub extern "C" fn initialize(
    sporepump_ptr: *const u8,
    token_id: u64,
    creator_ptr: *const u8,
    max_supply: u64,
    holder_obligations: u64,
) -> u32 {
    if initialized() {
        return 1;
    }
    let sporepump = read_address(sporepump_ptr);
    let creator = read_address(creator_ptr);
    if sporepump.iter().all(|byte| *byte == 0)
        || creator.iter().all(|byte| *byte == 0)
        || token_id == 0
        || max_supply == 0
        || holder_obligations > max_supply
    {
        return 2;
    }

    storage_set(SPOREPUMP_KEY, &sporepump);
    storage_set(CREATOR_KEY, &creator);
    save_u64(TOKEN_ID_KEY, token_id);
    save_u64(MAX_SUPPLY_KEY, max_supply);
    save_u64(HOLDER_OBLIGATIONS_KEY, holder_obligations);
    save_u64(CLAIMED_SUPPLY_KEY, 0);
    save_u64(MIGRATION_INVENTORY_KEY, 0);

    let mut graduated = token();
    if let Err(error) = graduated.initialize(0, Address(sporepump)) {
        return map_contract_error(error);
    }
    storage_set(INITIALIZED_KEY, &[1]);
    log_info("Launchpad token initialized");
    0
}

/// Return immutable provenance:
/// sporepump(32) + token_id(8) + creator(32) + max_supply(8) + obligations(8).
#[no_mangle]
pub extern "C" fn get_provenance() -> u32 {
    let Some(sporepump) = read_stored_address(SPOREPUMP_KEY) else {
        return 1;
    };
    let Some(creator) = read_stored_address(CREATOR_KEY) else {
        return 1;
    };
    let mut result = Vec::with_capacity(88);
    result.extend_from_slice(&sporepump);
    result.extend_from_slice(&u64_to_bytes(load_u64(TOKEN_ID_KEY)));
    result.extend_from_slice(&creator);
    result.extend_from_slice(&u64_to_bytes(load_u64(MAX_SUPPLY_KEY)));
    result.extend_from_slice(&u64_to_bytes(load_u64(HOLDER_OBLIGATIONS_KEY)));
    set_return_data(&result);
    0
}

/// Mint the token-side migration inventory exactly once. Only the bound
/// SporePump program may invoke this function through a cross-contract call.
#[no_mangle]
pub extern "C" fn mint_migration_inventory(to_ptr: *const u8, amount: u64) -> u32 {
    let Some(sporepump) = read_stored_address(SPOREPUMP_KEY) else {
        return 1;
    };
    if get_caller().0 != sporepump {
        return 200;
    }
    if amount == 0 || load_u64(MIGRATION_INVENTORY_KEY) != 0 {
        return 2;
    }
    let obligations = load_u64(HOLDER_OBLIGATIONS_KEY);
    let max_supply = load_u64(MAX_SUPPLY_KEY);
    if obligations
        .checked_add(amount)
        .is_none_or(|total| total > max_supply)
    {
        return 3;
    }
    let to = read_address(to_ptr);
    let mut graduated = token();
    match graduated.mint(Address(to), amount, Address(sporepump), Address(sporepump)) {
        Ok(()) => {
            save_u64(MIGRATION_INVENTORY_KEY, amount);
            log_info("Launchpad migration inventory minted");
            0
        }
        Err(error) => map_contract_error(error),
    }
}

/// Claim a frozen SporePump holder balance exactly once.
#[no_mangle]
pub extern "C" fn claim(holder_ptr: *const u8) -> u64 {
    let holder = read_address(holder_ptr);
    if get_caller().0 != holder {
        return 0;
    }
    let Some(sporepump) = read_stored_address(SPOREPUMP_KEY) else {
        return 0;
    };
    let token_id = load_u64(TOKEN_ID_KEY);
    let token_id_bytes = u64_to_bytes(token_id);
    let args = match encode_layout_args(&[&token_id_bytes, &holder]) {
        Ok(args) => args,
        Err(_) => return 0,
    };
    let response = match call_contract(CrossCall::new(
        Address(sporepump),
        "consume_graduation_claim",
        args,
    )) {
        Ok(response) if response.len() >= 8 => response,
        _ => return 0,
    };
    let amount = bytes_to_u64(&response[..8]);
    if amount == 0 {
        return 0;
    }

    let claimed = load_u64(CLAIMED_SUPPLY_KEY);
    let obligations = load_u64(HOLDER_OBLIGATIONS_KEY);
    let Some(next_claimed) = claimed.checked_add(amount) else {
        return 0;
    };
    if next_claimed > obligations {
        return 0;
    }
    let mut graduated = token();
    if graduated
        .mint(
            Address(holder),
            amount,
            Address(sporepump),
            Address(sporepump),
        )
        .is_err()
    {
        return 0;
    }
    save_u64(CLAIMED_SUPPLY_KEY, next_claimed);
    log_info("Launchpad holder claim minted");
    amount
}

#[no_mangle]
pub extern "C" fn balance_of(account_ptr: *const u8) -> u64 {
    token().balance_of(Address(read_address(account_ptr)))
}

#[no_mangle]
pub extern "C" fn total_supply() -> u64 {
    token().get_total_supply()
}

#[no_mangle]
pub extern "C" fn transfer(from_ptr: *const u8, to_ptr: *const u8, amount: u64) -> u32 {
    let from = read_address(from_ptr);
    if get_caller().0 != from {
        return 200;
    }
    match token().transfer(Address(from), Address(read_address(to_ptr)), amount) {
        Ok(()) => 0,
        Err(error) => map_contract_error(error),
    }
}

#[no_mangle]
pub extern "C" fn approve(owner_ptr: *const u8, spender_ptr: *const u8, amount: u64) -> u32 {
    let owner = read_address(owner_ptr);
    if get_caller().0 != owner {
        return 200;
    }
    match token().approve(Address(owner), Address(read_address(spender_ptr)), amount) {
        Ok(()) => 0,
        Err(error) => map_contract_error(error),
    }
}

#[no_mangle]
pub extern "C" fn allowance(owner_ptr: *const u8, spender_ptr: *const u8) -> u64 {
    token().allowance(
        Address(read_address(owner_ptr)),
        Address(read_address(spender_ptr)),
    )
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
    match token().transfer_from(
        Address(caller),
        Address(read_address(from_ptr)),
        Address(read_address(to_ptr)),
        amount,
    ) {
        Ok(()) => 0,
        Err(error) => map_contract_error(error),
    }
}

#[no_mangle]
pub extern "C" fn decimals() -> u32 {
    TOKEN_DECIMALS as u32
}

#[no_mangle]
pub extern "C" fn name() -> u32 {
    let value = launch_metadata()
        .map(|metadata| metadata.0)
        .unwrap_or_else(|| TOKEN_NAME.as_bytes().to_vec());
    set_return_data(&value);
    0
}

#[no_mangle]
pub extern "C" fn symbol() -> u32 {
    let value = launch_metadata()
        .map(|metadata| metadata.1)
        .unwrap_or_else(|| TOKEN_SYMBOL.as_bytes().to_vec());
    set_return_data(&value);
    0
}

#[no_mangle]
pub extern "C" fn sporepump_token_id() -> u64 {
    load_u64(TOKEN_ID_KEY)
}

#[no_mangle]
pub extern "C" fn sporepump_program() -> u32 {
    let Some(program) = read_stored_address(SPOREPUMP_KEY) else {
        return 1;
    };
    set_return_data(&program);
    0
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use lichen_sdk::test_mock;

    fn address(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    fn initialize_test_token(obligations: u64, max_supply: u64) -> ([u8; 32], [u8; 32]) {
        test_mock::reset();
        let sporepump = address(1);
        let creator = address(2);
        test_mock::set_caller(address(9));
        assert_eq!(
            initialize(
                sporepump.as_ptr(),
                7,
                creator.as_ptr(),
                max_supply,
                obligations,
            ),
            0
        );
        (sporepump, creator)
    }

    #[test]
    fn initialization_binds_provenance_and_zero_supply() {
        let (sporepump, creator) = initialize_test_token(600, 1_000);
        assert_eq!(total_supply(), 0);
        assert_eq!(get_provenance(), 0);
        let provenance = test_mock::get_return_data();
        assert_eq!(&provenance[0..32], &sporepump);
        assert_eq!(bytes_to_u64(&provenance[32..40]), 7);
        assert_eq!(&provenance[40..72], &creator);
        assert_eq!(bytes_to_u64(&provenance[72..80]), 1_000);
        assert_eq!(bytes_to_u64(&provenance[80..88]), 600);
    }

    #[test]
    fn metadata_getters_resolve_canonical_sporepump_identity() {
        initialize_test_token(600, 1_000);
        let expected_name = b"Forest Credit";
        let expected_symbol = b"FERN";
        let mut metadata = Vec::new();
        metadata.extend_from_slice(&(expected_name.len() as u16).to_le_bytes());
        metadata.extend_from_slice(expected_name);
        metadata.extend_from_slice(&(expected_symbol.len() as u16).to_le_bytes());
        metadata.extend_from_slice(expected_symbol);
        test_mock::set_cross_call_response(Some(metadata));
        assert_eq!(name(), 0);
        assert_eq!(test_mock::get_return_data(), expected_name);
        test_mock::set_cross_call_response(Some({
            let mut value = Vec::new();
            value.extend_from_slice(&(expected_name.len() as u16).to_le_bytes());
            value.extend_from_slice(expected_name);
            value.extend_from_slice(&(expected_symbol.len() as u16).to_le_bytes());
            value.extend_from_slice(expected_symbol);
            value
        }));
        assert_eq!(symbol(), 0);
        assert_eq!(test_mock::get_return_data(), expected_symbol);
    }

    #[test]
    fn migration_inventory_is_sporepump_only_exactly_once_and_supply_bounded() {
        let (sporepump, _) = initialize_test_token(600, 1_000);
        let pool_owner = address(3);
        test_mock::set_caller(address(8));
        assert_eq!(mint_migration_inventory(pool_owner.as_ptr(), 400), 200);
        test_mock::set_caller(sporepump);
        assert_eq!(mint_migration_inventory(pool_owner.as_ptr(), 401), 3);
        assert_eq!(mint_migration_inventory(pool_owner.as_ptr(), 400), 0);
        assert_eq!(mint_migration_inventory(pool_owner.as_ptr(), 1), 2);
        assert_eq!(balance_of(pool_owner.as_ptr()), 400);
        assert_eq!(total_supply(), 400);
    }

    #[test]
    fn claim_mints_exact_sporepump_amount_and_tracks_obligations() {
        initialize_test_token(600, 1_000);
        let holder = address(4);
        test_mock::set_caller(holder);
        test_mock::set_cross_call_response(Some(u64_to_bytes(250).to_vec()));
        assert_eq!(claim(holder.as_ptr()), 250);
        assert_eq!(balance_of(holder.as_ptr()), 250);
        assert_eq!(load_u64(CLAIMED_SUPPLY_KEY), 250);

        test_mock::set_cross_call_response(Some(u64_to_bytes(350).to_vec()));
        assert_eq!(claim(holder.as_ptr()), 350);
        assert_eq!(balance_of(holder.as_ptr()), 600);
        assert_eq!(load_u64(CLAIMED_SUPPLY_KEY), 600);

        test_mock::set_cross_call_response(Some(u64_to_bytes(1).to_vec()));
        assert_eq!(claim(holder.as_ptr()), 0);
        assert_eq!(balance_of(holder.as_ptr()), 600);
    }

    #[test]
    fn standard_transfer_and_allowance_paths_work_after_claim() {
        initialize_test_token(500, 1_000);
        let holder = address(4);
        let recipient = address(5);
        let spender = address(6);
        test_mock::set_caller(holder);
        test_mock::set_cross_call_response(Some(u64_to_bytes(500).to_vec()));
        assert_eq!(claim(holder.as_ptr()), 500);
        assert_eq!(transfer(holder.as_ptr(), recipient.as_ptr(), 100), 0);
        assert_eq!(approve(holder.as_ptr(), spender.as_ptr(), 200), 0);
        test_mock::set_caller(spender);
        assert_eq!(
            transfer_from(spender.as_ptr(), holder.as_ptr(), recipient.as_ptr(), 200),
            0
        );
        assert_eq!(balance_of(holder.as_ptr()), 200);
        assert_eq!(balance_of(recipient.as_ptr()), 300);
    }

    #[test]
    fn caller_mismatch_cannot_claim_or_transfer() {
        initialize_test_token(500, 1_000);
        let holder = address(4);
        test_mock::set_caller(address(7));
        test_mock::set_cross_call_response(Some(u64_to_bytes(500).to_vec()));
        assert_eq!(claim(holder.as_ptr()), 0);
        assert_eq!(transfer(holder.as_ptr(), address(5).as_ptr(), 1), 200);
        assert_eq!(total_supply(), 0);
    }
}
