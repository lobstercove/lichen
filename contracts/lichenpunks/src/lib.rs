// LichenPunks - Collectible NFT Contract
// Example implementation of MT-721 standard

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;

use lichen_sdk::{
    bytes_to_u64, get_caller, log_info, storage_get, storage_set, u64_to_bytes, Address, NFT,
};

const MP_TRANSFER_COUNT_KEY: &[u8] = b"mp_transfer_count";
const MP_BURN_COUNT_KEY: &[u8] = b"mp_burn_count";
const MP_ADMIN_KEY: &[u8] = b"mp_admin";
const MP_PENDING_ADMIN_KEY: &[u8] = b"mp_pending_admin";
const MP_MINT_AUTHORITY_KEY: &[u8] = b"mp_mint_authority";
const MP_ROYALTY_RECIPIENT_KEY: &[u8] = b"mp_royalty_recipient";
const MAX_METADATA_LEN: usize = 512;
const MAX_BASE_URI_LEN: usize = 256;

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

fn read_bytes(ptr: *const u8, len: u32, max_len: usize) -> Option<alloc::vec::Vec<u8>> {
    let len = len as usize;
    if len > max_len || (len > 0 && ptr.is_null()) {
        return None;
    }
    let mut bytes = alloc::vec![0u8; len];
    if len > 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(ptr, bytes.as_mut_ptr(), len);
        }
    }
    Some(bytes)
}

fn load_u64_or_zero(key: &[u8]) -> Option<u64> {
    match storage_get(key) {
        Some(data) if data.len() == 8 => Some(bytes_to_u64(&data)),
        Some(_) => None,
        None => Some(0),
    }
}

fn stored_u64(key: &[u8]) -> u64 {
    load_u64_or_zero(key).unwrap_or(0)
}

fn next_counter(key: &[u8]) -> Option<u64> {
    load_u64_or_zero(key)?.checked_add(1)
}

fn metadata_key(token_id: u64) -> alloc::vec::Vec<u8> {
    let mut key = b"metadata:".to_vec();
    key.extend_from_slice(&u64_to_bytes(token_id));
    key
}

fn is_initialized() -> bool {
    get_minter().0 != [0u8; 32]
        && load_u64_or_zero(b"total_minted").is_some()
        && storage_get(b"collection_name").as_deref() == Some(b"LichenPunks")
        && storage_get(b"collection_symbol").as_deref() == Some(b"MPNK")
}

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
    NFT::new("LichenPunks", "MPNK")
}

/// Check if LichenPunks is paused
fn is_mp_paused() -> bool {
    pause_state().unwrap_or(true)
}

fn pause_state() -> Option<bool> {
    match storage_get(b"mp_paused") {
        None => Some(false),
        Some(data) if data.as_slice() == [0u8] => Some(false),
        Some(data) if data.as_slice() == [1u8] => Some(true),
        Some(_) => None,
    }
}

fn init_minter_matches_signer(minter: &[u8; 32]) -> bool {
    lichen_sdk::get_caller().0 == *minter
}

fn configured_address_or_minter(key: &[u8]) -> Option<Address> {
    match storage_get(key) {
        Some(data) if data.len() == 32 && data.as_slice() != [0u8; 32] => {
            let mut address = [0u8; 32];
            address.copy_from_slice(&data);
            Some(Address(address))
        }
        Some(_) => None,
        None => {
            let minter = get_minter();
            (minter.0 != [0u8; 32]).then_some(minter)
        }
    }
}

fn admin() -> Option<Address> {
    configured_address_or_minter(MP_ADMIN_KEY)
}

fn mint_authority() -> Option<Address> {
    configured_address_or_minter(MP_MINT_AUTHORITY_KEY)
}

fn royalty_recipient() -> Option<Address> {
    configured_address_or_minter(MP_ROYALTY_RECIPIENT_KEY)
}

fn authenticated_admin(caller_ptr: *const u8) -> Option<Address> {
    let caller = read_address(caller_ptr)?;
    (caller == get_caller() && admin() == Some(caller)).then_some(caller)
}

fn pending_admin() -> Option<Address> {
    match storage_get(MP_PENDING_ADMIN_KEY) {
        Some(data) if data.len() == 32 && data.as_slice() != [0u8; 32] => {
            let mut address = [0u8; 32];
            address.copy_from_slice(&data);
            Some(Address(address))
        }
        None => Some(Address([0u8; 32])),
        Some(data) if data.is_empty() => Some(Address([0u8; 32])),
        Some(_) => None,
    }
}

fn royalty_bps() -> Option<u16> {
    match storage_get(b"royalty_bps") {
        Some(data) if data.len() == 8 => {
            let bps = u16::try_from(bytes_to_u64(&data)).ok()?;
            (bps <= 1_000).then_some(bps)
        }
        Some(_) => None,
        None => Some(0),
    }
}

fn max_supply() -> Option<u64> {
    match storage_get(b"max_supply") {
        Some(data) if data.len() == 8 => Some(bytes_to_u64(&data)),
        Some(_) => None,
        None => Some(0),
    }
}

/// Initialize the NFT collection
#[no_mangle]
pub extern "C" fn initialize(minter_ptr: *const u8) {
    if is_initialized() {
        log_info("LichenPunks already initialized — ignoring");
        return;
    }
    if storage_get(b"minter").is_some()
        || storage_get(b"total_minted").is_some()
        || storage_get(b"collection_name").is_some()
        || storage_get(b"collection_symbol").is_some()
    {
        log_info("LichenPunks initialization state is partial or malformed");
        return;
    }

    let minter = match read_address(minter_ptr) {
        Some(addr) => addr,
        None => return,
    };
    if minter.0 == [0u8; 32] {
        log_info("LichenPunks initialize rejected: zero minter");
        return;
    }
    if !init_minter_matches_signer(&minter.0) {
        log_info("LichenPunks initialize rejected: caller mismatch");
        return;
    }

    // NFT::initialize stores the minter in storage under key "minter"
    let mut nft = make_nft();
    if nft.initialize(minter).is_err() {
        log_info("LichenPunks initialization failed");
        return;
    }

    // Store collection metadata only after the base NFT initialized.
    storage_set(b"collection_name", b"LichenPunks");
    storage_set(b"collection_symbol", b"MPNK");
    storage_set(MP_ADMIN_KEY, &minter.0);
    storage_set(MP_MINT_AUTHORITY_KEY, &minter.0);
    storage_set(MP_ROYALTY_RECIPIENT_KEY, &minter.0);
    storage_set(MP_PENDING_ADMIN_KEY, &[]);
    storage_set(b"mp_paused", &[0u8]);

    log_info("LichenPunks NFT collection initialized");
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
        log_info("LichenPunks is paused");
        return 0;
    }
    if !is_initialized() {
        log_info("LichenPunks is not initialized");
        return 0;
    }

    let caller = match read_address(caller_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    let to = match read_address(to_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    if to.0 == [0u8; 32] {
        log_info("Mint recipient cannot be zero address");
        return 0;
    }

    // P9-SC-06: Verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller.0 != caller.0 {
        log_info("Unauthorized: caller mismatch");
        return 0;
    }

    if mint_authority() != Some(caller) {
        log_info("Unauthorized: caller is not the mint authority");
        return 0;
    }

    let current_supply = match load_u64_or_zero(b"total_minted") {
        Some(value) => value,
        None => {
            log_info("Total minted state is malformed");
            return 0;
        }
    };
    if current_supply == u64::MAX {
        log_info("Total supply overflow");
        return 0;
    }
    if let Some(max_data) = storage_get(b"max_supply") {
        let max = match max_data.len() {
            8 => bytes_to_u64(&max_data),
            _ => {
                log_info("Maximum supply state is malformed");
                return 0;
            }
        };
        if max > 0 && current_supply >= max {
            log_info("Max supply reached");
            return 0;
        }
    }

    if make_nft().balance_of(to) == u64::MAX {
        log_info("Recipient balance overflow");
        return 0;
    }

    let metadata = match read_bytes(metadata_ptr, metadata_len, MAX_METADATA_LEN) {
        Some(metadata) => metadata,
        None => {
            log_info("Metadata too large or invalid");
            return 0;
        }
    };

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

/// Transfer NFT
#[no_mangle]
pub extern "C" fn transfer(from_ptr: *const u8, to_ptr: *const u8, token_id: u64) -> u32 {
    // AUDIT-FIX P2: Check pause state
    if is_mp_paused() {
        log_info("LichenPunks is paused");
        return 0;
    }
    let from = match read_address(from_ptr) {
        Some(addr) => addr,
        None => return 0,
    };

    // SECURITY FIX: Verify caller owns the NFT being transferred
    let caller = get_caller();
    if caller.0 != from.0 {
        log_info("Unauthorized: caller does not match from address");
        return 0;
    }

    let to = match read_address(to_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    if to.0 == [0u8; 32] {
        log_info("Transfer recipient cannot be zero address");
        return 0;
    }
    if from.0 != to.0 && make_nft().balance_of(to) == u64::MAX {
        log_info("Recipient balance overflow");
        return 0;
    }
    let next_transfer_count = match next_counter(MP_TRANSFER_COUNT_KEY) {
        Some(value) => value,
        None => {
            log_info("Transfer counter is malformed or exhausted");
            return 0;
        }
    };

    // Transfer
    match make_nft().transfer(from, to, token_id) {
        Ok(_) => {
            storage_set(MP_TRANSFER_COUNT_KEY, &u64_to_bytes(next_transfer_count));
            log_info("NFT transferred successfully");
            1
        }
        Err(_) => {
            log_info("Transfer failed");
            0
        }
    }
}

/// Get owner of token
#[no_mangle]
pub extern "C" fn owner_of(token_id: u64, out_ptr: *mut u8) -> u32 {
    if out_ptr.is_null() {
        return 0;
    }
    unsafe {
        match make_nft().owner_of(token_id) {
            Ok(owner) => {
                lichen_sdk::set_return_data(&owner.0);
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
    let account = match read_address(account_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    make_nft().balance_of(account)
}

/// Approve spender for token
#[no_mangle]
pub extern "C" fn approve(owner_ptr: *const u8, spender_ptr: *const u8, token_id: u64) -> u32 {
    if is_mp_paused() {
        log_info("LichenPunks is paused");
        return 0;
    }
    let owner = match read_address(owner_ptr) {
        Some(addr) => addr,
        None => return 0,
    };

    // AUDIT-FIX P2: Verify caller is the owner
    let real_caller = get_caller();
    if real_caller.0 != owner.0 {
        log_info("Approve rejected: caller mismatch");
        return 0;
    }

    let spender = match read_address(spender_ptr) {
        Some(addr) => addr,
        None => return 0,
    };

    match make_nft().approve(owner, spender, token_id) {
        Ok(_) => 1,
        Err(_) => 0,
    }
}

/// Get the token-specific approved spender. A zero address means no approval.
#[no_mangle]
pub extern "C" fn get_approved(token_id: u64) -> u32 {
    if make_nft().owner_of(token_id).is_err() {
        return 0;
    }
    let approved = make_nft()
        .get_approved(token_id)
        .unwrap_or(Address([0u8; 32]));
    lichen_sdk::set_return_data(&approved.0);
    1
}

/// Approve or revoke an operator for all NFTs owned by `owner`.
#[no_mangle]
pub extern "C" fn set_approval_for_all(
    owner_ptr: *const u8,
    operator_ptr: *const u8,
    approved: u32,
) -> u32 {
    if is_mp_paused() || approved > 1 {
        return 0;
    }
    let owner = match read_address(owner_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    let operator = match read_address(operator_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    if get_caller() != owner {
        log_info("Operator approval rejected: caller mismatch");
        return 0;
    }
    match make_nft().set_approval_for_all(owner, operator, approved == 1) {
        Ok(()) => 1,
        Err(_) => 0,
    }
}

/// Return whether `operator` can transfer every NFT owned by `owner`.
#[no_mangle]
pub extern "C" fn is_approved_for_all(owner_ptr: *const u8, operator_ptr: *const u8) -> u32 {
    let owner = match read_address(owner_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    let operator = match read_address(operator_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    u32::from(make_nft().is_approved_for_all(owner, operator))
}

/// Standardized collection royalty response: recipient(32) + bps(2).
#[no_mangle]
pub extern "C" fn royalty_info(_token_id: u64) -> u32 {
    let recipient = match royalty_recipient() {
        Some(recipient) => recipient,
        None => return 0,
    };
    let bps = match royalty_bps() {
        Some(bps) => bps,
        None => return 0,
    };
    let mut result = [0u8; 34];
    result[..32].copy_from_slice(&recipient.0);
    result[32..].copy_from_slice(&bps.to_le_bytes());
    lichen_sdk::set_return_data(&result);
    1
}

/// Transfer from (with approval)
#[no_mangle]
pub extern "C" fn transfer_from(
    caller_ptr: *const u8,
    from_ptr: *const u8,
    to_ptr: *const u8,
    token_id: u64,
) -> u32 {
    if is_mp_paused() {
        log_info("LichenPunks is paused");
        return 0;
    }
    let caller = match read_address(caller_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    let from = match read_address(from_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    let to = match read_address(to_ptr) {
        Some(addr) => addr,
        None => return 0,
    };

    let real_caller = get_caller();
    if real_caller.0 != caller.0 {
        log_info("TransferFrom rejected: caller mismatch");
        return 0;
    }
    if to.0 == [0u8; 32] {
        log_info("TransferFrom recipient cannot be zero address");
        return 0;
    }
    if from.0 != to.0 && make_nft().balance_of(to) == u64::MAX {
        log_info("Recipient balance overflow");
        return 0;
    }
    let next_transfer_count = match next_counter(MP_TRANSFER_COUNT_KEY) {
        Some(value) => value,
        None => {
            log_info("Transfer counter is malformed or exhausted");
            return 0;
        }
    };

    match make_nft().transfer_from(caller, from, to, token_id) {
        Ok(_) => {
            storage_set(MP_TRANSFER_COUNT_KEY, &u64_to_bytes(next_transfer_count));
            log_info("TransferFrom successful");
            1
        }
        Err(_) => {
            log_info("TransferFrom failed");
            0
        }
    }
}

/// Burn NFT
#[no_mangle]
pub extern "C" fn burn(owner_ptr: *const u8, token_id: u64) -> u32 {
    if is_mp_paused() {
        log_info("LichenPunks is paused");
        return 0;
    }
    let owner = match read_address(owner_ptr) {
        Some(addr) => addr,
        None => return 0,
    };

    // AUDIT-FIX P2: Verify caller is the owner
    let real_caller = get_caller();
    if real_caller.0 != owner.0 {
        log_info("Burn rejected: caller mismatch");
        return 0;
    }
    let next_burn_count = match next_counter(MP_BURN_COUNT_KEY) {
        Some(value) => value,
        None => {
            log_info("Burn counter is malformed or exhausted");
            return 0;
        }
    };

    let mut nft = make_nft();
    match nft.burn(owner, token_id) {
        Ok(_) => {
            storage_set(MP_BURN_COUNT_KEY, &u64_to_bytes(next_burn_count));
            log_info("NFT burned");
            1
        }
        Err(_) => {
            log_info("Burn failed");
            0
        }
    }
}

/// Get total minted (read from persistent storage)
#[no_mangle]
pub extern "C" fn total_minted() -> u64 {
    stored_u64(b"total_minted")
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
    if make_nft().owner_of(token_id).is_err() {
        return 0;
    }
    let key = metadata_key(token_id);
    match storage_get(&key) {
        Some(data) => {
            lichen_sdk::set_return_data(&data);
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
    if authenticated_admin(caller_ptr).is_none() {
        return 0;
    }
    let uri = match read_bytes(uri_ptr, uri_len, MAX_BASE_URI_LEN) {
        Some(uri) => uri,
        None => {
            log_info("Base URI too large or invalid");
            return 0;
        }
    };
    storage_set(b"base_uri", &uri);
    log_info("Base URI set");
    1
}

/// Tests expect `set_max_supply`
#[no_mangle]
pub extern "C" fn set_max_supply(caller_ptr: *const u8, max_supply: u64) -> u32 {
    if authenticated_admin(caller_ptr).is_none() {
        return 0;
    }
    let minted = match load_u64_or_zero(b"total_minted") {
        Some(value) => value,
        None => return 0,
    };
    if max_supply > 0 && max_supply < minted {
        log_info("Max supply below current supply");
        return 0;
    }
    storage_set(b"max_supply", &u64_to_bytes(max_supply));
    log_info("Max supply set");
    1
}

/// Tests expect `set_royalty`
#[no_mangle]
pub extern "C" fn set_royalty(caller_ptr: *const u8, bps: u64) -> u32 {
    if authenticated_admin(caller_ptr).is_none() {
        return 0;
    }
    if bps > 1000 {
        log_info("Royalty too high");
        return 0;
    }
    storage_set(b"royalty_bps", &u64_to_bytes(bps));
    log_info("Royalty set");
    1
}

/// Tests expect `mp_pause`
#[no_mangle]
pub extern "C" fn mp_pause(caller_ptr: *const u8) -> u32 {
    if authenticated_admin(caller_ptr).is_none() {
        return 0;
    }
    storage_set(b"mp_paused", &[1u8]);
    log_info("LichenPunks paused");
    1
}

/// Tests expect `mp_unpause`
#[no_mangle]
pub extern "C" fn mp_unpause(caller_ptr: *const u8) -> u32 {
    if authenticated_admin(caller_ptr).is_none() {
        return 0;
    }
    storage_set(b"mp_paused", &[0u8]);
    log_info("LichenPunks unpaused");
    1
}

/// Start a two-step collection administrator rotation.
#[no_mangle]
pub extern "C" fn propose_admin(caller_ptr: *const u8, next_admin_ptr: *const u8) -> u32 {
    let current = match authenticated_admin(caller_ptr) {
        Some(current) => current,
        None => return 0,
    };
    let next = match read_address(next_admin_ptr) {
        Some(next) if next.0 != [0u8; 32] && next != current => next,
        _ => return 0,
    };
    storage_set(MP_PENDING_ADMIN_KEY, &next.0);
    1
}

/// Accept a pending collection administrator role with the pending key.
#[no_mangle]
pub extern "C" fn accept_admin(caller_ptr: *const u8) -> u32 {
    let caller = match read_address(caller_ptr) {
        Some(caller) if caller == get_caller() => caller,
        _ => return 0,
    };
    if pending_admin() != Some(caller) {
        return 0;
    }
    storage_set(MP_ADMIN_KEY, &caller.0);
    storage_set(MP_PENDING_ADMIN_KEY, &[]);
    1
}

/// Rotate mint authority without changing administration or royalty custody.
#[no_mangle]
pub extern "C" fn set_mint_authority(caller_ptr: *const u8, mint_authority_ptr: *const u8) -> u32 {
    if authenticated_admin(caller_ptr).is_none() {
        return 0;
    }
    let authority = match read_address(mint_authority_ptr) {
        Some(authority) if authority.0 != [0u8; 32] => authority,
        _ => return 0,
    };
    storage_set(MP_MINT_AUTHORITY_KEY, &authority.0);
    1
}

/// Set canonical collection royalty recipient and bps together.
#[no_mangle]
pub extern "C" fn set_royalty_config(
    caller_ptr: *const u8,
    recipient_ptr: *const u8,
    bps: u64,
) -> u32 {
    if authenticated_admin(caller_ptr).is_none() || bps > 1_000 {
        return 0;
    }
    let recipient = match read_address(recipient_ptr) {
        Some(recipient) if recipient.0 != [0u8; 32] => recipient,
        _ => return 0,
    };
    storage_set(MP_ROYALTY_RECIPIENT_KEY, &recipient.0);
    storage_set(b"royalty_bps", &u64_to_bytes(bps));
    1
}

/// Return admin(32), pending admin(32), mint authority(32), royalty
/// recipient(32), royalty bps(8), max supply(8), and paused(1).
#[no_mangle]
pub extern "C" fn get_collection_config() -> u32 {
    let admin = match admin() {
        Some(admin) => admin,
        None => return 0,
    };
    let pending = match pending_admin() {
        Some(pending) => pending,
        None => return 0,
    };
    let mint_authority = match mint_authority() {
        Some(authority) => authority,
        None => return 0,
    };
    let royalty_recipient = match royalty_recipient() {
        Some(recipient) => recipient,
        None => return 0,
    };
    let royalty_bps = match royalty_bps() {
        Some(bps) => bps,
        None => return 0,
    };
    let max_supply = match max_supply() {
        Some(max_supply) => max_supply,
        None => return 0,
    };
    let paused = match pause_state() {
        Some(paused) => paused,
        None => return 0,
    };
    let mut result = alloc::vec::Vec::with_capacity(145);
    result.extend_from_slice(&admin.0);
    result.extend_from_slice(&pending.0);
    result.extend_from_slice(&mint_authority.0);
    result.extend_from_slice(&royalty_recipient.0);
    result.extend_from_slice(&u64_to_bytes(u64::from(royalty_bps)));
    result.extend_from_slice(&u64_to_bytes(max_supply));
    result.push(u8::from(paused));
    lichen_sdk::set_return_data(&result);
    1
}

/// Get collection stats [total_minted(8), transfer_count(8), burn_count(8)]
#[no_mangle]
pub extern "C" fn get_collection_stats() -> u32 {
    let minted = match load_u64_or_zero(b"total_minted") {
        Some(value) => value,
        None => return 1,
    };
    let transfers = match load_u64_or_zero(MP_TRANSFER_COUNT_KEY) {
        Some(value) => value,
        None => return 1,
    };
    let burns = match load_u64_or_zero(MP_BURN_COUNT_KEY) {
        Some(value) => value,
        None => return 1,
    };
    let mut buf = [0u8; 24];
    buf[0..8].copy_from_slice(&u64_to_bytes(minted));
    buf[8..16].copy_from_slice(&u64_to_bytes(transfers));
    buf[16..24].copy_from_slice(&u64_to_bytes(burns));
    lichen_sdk::set_return_data(&buf);
    0
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use lichen_sdk::test_mock;

    fn setup() {
        test_mock::reset();
        test_mock::set_caller([1u8; 32]);
    }

    fn mint_test_token(minter: &[u8; 32], owner: &[u8; 32], token_id: u64) {
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(*minter);
        assert_eq!(
            mint(
                minter.as_ptr(),
                owner.as_ptr(),
                token_id,
                metadata.as_ptr(),
                metadata.len() as u32
            ),
            1
        );
    }

    #[test]
    fn test_initialize() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let stored = test_mock::get_storage(b"minter");
        assert_eq!(stored, Some(minter.to_vec()));
        assert_eq!(
            test_mock::get_storage(b"collection_name"),
            Some(b"LichenPunks".to_vec())
        );
        assert_eq!(
            test_mock::get_storage(b"collection_symbol"),
            Some(b"MPNK".to_vec())
        );
    }

    #[test]
    fn test_initialize_rejects_caller_mismatch() {
        setup();
        let minter = [1u8; 32];
        test_mock::set_caller([9u8; 32]);
        initialize(minter.as_ptr());
        assert_eq!(test_mock::get_storage(b"minter"), None);
        assert_eq!(test_mock::get_storage(b"collection_name"), None);
    }

    #[test]
    fn test_mint() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let to = [2u8; 32];
        let metadata = b"ipfs://QmTest123";
        test_mock::set_caller(minter);
        assert_eq!(
            mint(
                minter.as_ptr(),
                to.as_ptr(),
                1,
                metadata.as_ptr(),
                metadata.len() as u32
            ),
            1
        );
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
        assert_eq!(
            mint(
                other.as_ptr(),
                to.as_ptr(),
                1,
                metadata.as_ptr(),
                metadata.len() as u32
            ),
            0
        );
    }

    #[test]
    fn test_mint_duplicate() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let to = [2u8; 32];
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(minter);
        mint(
            minter.as_ptr(),
            to.as_ptr(),
            1,
            metadata.as_ptr(),
            metadata.len() as u32,
        );
        assert_eq!(
            mint(
                minter.as_ptr(),
                to.as_ptr(),
                1,
                metadata.as_ptr(),
                metadata.len() as u32
            ),
            0
        );
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
        mint(
            minter.as_ptr(),
            from.as_ptr(),
            1,
            metadata.as_ptr(),
            metadata.len() as u32,
        );
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
        mint(
            minter.as_ptr(),
            owner.as_ptr(),
            1,
            metadata.as_ptr(),
            metadata.len() as u32,
        );
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
        mint(
            minter.as_ptr(),
            owner.as_ptr(),
            1,
            metadata.as_ptr(),
            metadata.len() as u32,
        );
        let mut out = [0u8; 32];
        assert_eq!(owner_of(1, out.as_mut_ptr()), 1);
        assert_eq!(out, owner);
        assert_eq!(test_mock::get_return_data(), owner.to_vec());
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
        mint(
            minter.as_ptr(),
            owner.as_ptr(),
            1,
            metadata.as_ptr(),
            metadata.len() as u32,
        );
        assert_eq!(balance_of(owner.as_ptr()), 1);
        mint(
            minter.as_ptr(),
            owner.as_ptr(),
            2,
            metadata.as_ptr(),
            metadata.len() as u32,
        );
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
        mint(
            minter.as_ptr(),
            owner.as_ptr(),
            1,
            metadata.as_ptr(),
            metadata.len() as u32,
        );
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
        mint(
            minter.as_ptr(),
            owner.as_ptr(),
            1,
            metadata.as_ptr(),
            metadata.len() as u32,
        );
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
        mint(
            minter.as_ptr(),
            owner.as_ptr(),
            1,
            metadata.as_ptr(),
            metadata.len() as u32,
        );
        // AUDIT-FIX P2: Set caller for security check on approve
        test_mock::set_caller(owner);
        approve(owner.as_ptr(), spender.as_ptr(), 1);
        test_mock::set_caller(spender);
        assert_eq!(
            transfer_from(spender.as_ptr(), owner.as_ptr(), to.as_ptr(), 1),
            1
        );
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
        mint(
            minter.as_ptr(),
            owner.as_ptr(),
            1,
            metadata.as_ptr(),
            metadata.len() as u32,
        );
        assert_eq!(
            transfer_from(other.as_ptr(), owner.as_ptr(), to.as_ptr(), 1),
            0
        );
    }

    #[test]
    fn test_burn() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(minter);
        mint(
            minter.as_ptr(),
            owner.as_ptr(),
            1,
            metadata.as_ptr(),
            metadata.len() as u32,
        );
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
        mint(
            minter.as_ptr(),
            owner.as_ptr(),
            1,
            metadata.as_ptr(),
            metadata.len() as u32,
        );
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
        assert_eq!(
            mint(
                minter.as_ptr(),
                to.as_ptr(),
                1,
                metadata.as_ptr(),
                metadata.len() as u32
            ),
            0
        );
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
        assert_eq!(
            mint(
                minter.as_ptr(),
                owner.as_ptr(),
                1,
                metadata.as_ptr(),
                metadata.len() as u32
            ),
            1
        );
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
        assert_eq!(
            mint(
                minter.as_ptr(),
                owner.as_ptr(),
                1,
                metadata.as_ptr(),
                metadata.len() as u32
            ),
            1
        );
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
        assert_eq!(
            mint(
                minter.as_ptr(),
                owner.as_ptr(),
                1,
                metadata.as_ptr(),
                metadata.len() as u32
            ),
            1
        );
        // set_caller differs from owner arg → should fail
        test_mock::set_caller(attacker);
        assert_eq!(burn(owner.as_ptr(), 1), 0);
    }

    #[test]
    fn test_mint_requires_initialization() {
        setup();
        let self_minter = [2u8; 32];
        let metadata = b"ipfs://QmTest";
        test_mock::set_caller(self_minter);
        assert_eq!(
            mint(
                self_minter.as_ptr(),
                self_minter.as_ptr(),
                1,
                metadata.as_ptr(),
                metadata.len() as u32
            ),
            0
        );
        assert_eq!(total_minted(), 0);
    }

    #[test]
    fn test_mint_rejects_oversized_metadata() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let metadata = alloc::vec![b'a'; MAX_METADATA_LEN + 1];

        test_mock::set_caller(minter);
        assert_eq!(
            mint(
                minter.as_ptr(),
                owner.as_ptr(),
                1,
                metadata.as_ptr(),
                metadata.len() as u32
            ),
            0
        );
        assert_eq!(total_minted(), 0);
    }

    #[test]
    fn test_get_punk_metadata_uses_actual_metadata_key() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let metadata = b"ipfs://QmMetadata";

        test_mock::set_caller(minter);
        assert_eq!(
            mint(
                minter.as_ptr(),
                owner.as_ptr(),
                1,
                metadata.as_ptr(),
                metadata.len() as u32
            ),
            1
        );

        assert_eq!(get_punk_metadata(1), 1);
        assert_eq!(test_mock::get_return_data(), metadata.to_vec());
    }

    #[test]
    fn test_transfer_from_rejects_spoofed_caller_pointer() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let spender = [3u8; 32];
        let attacker = [4u8; 32];
        let to = [5u8; 32];
        mint_test_token(&minter, &owner, 1);

        test_mock::set_caller(owner);
        assert_eq!(approve(owner.as_ptr(), spender.as_ptr(), 1), 1);

        test_mock::set_caller(attacker);
        assert_eq!(
            transfer_from(spender.as_ptr(), owner.as_ptr(), to.as_ptr(), 1),
            0
        );

        let mut out = [0u8; 32];
        assert_eq!(owner_of(1, out.as_mut_ptr()), 1);
        assert_eq!(out, owner);
    }

    #[test]
    fn test_transfer_from_and_burn_blocked_when_paused() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let spender = [3u8; 32];
        let to = [4u8; 32];
        mint_test_token(&minter, &owner, 1);

        test_mock::set_caller(owner);
        assert_eq!(approve(owner.as_ptr(), spender.as_ptr(), 1), 1);

        test_mock::set_caller(minter);
        assert_eq!(mp_pause(minter.as_ptr()), 1);

        test_mock::set_caller(spender);
        assert_eq!(
            transfer_from(spender.as_ptr(), owner.as_ptr(), to.as_ptr(), 1),
            0
        );
        test_mock::set_caller(owner);
        assert_eq!(burn(owner.as_ptr(), 1), 0);
    }

    #[test]
    fn test_transfer_counter_exhaustion_fails_before_owner_change() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        let to = [3u8; 32];
        mint_test_token(&minter, &owner, 1);
        storage_set(MP_TRANSFER_COUNT_KEY, &u64_to_bytes(u64::MAX));

        test_mock::set_caller(owner);
        assert_eq!(transfer(owner.as_ptr(), to.as_ptr(), 1), 0);
        assert_eq!(stored_u64(MP_TRANSFER_COUNT_KEY), u64::MAX);
        assert_eq!(
            make_nft().owner_of(1).expect("owner remains"),
            Address(owner)
        );
    }

    #[test]
    fn test_admin_bounds_for_uri_supply_and_royalty() {
        setup();
        let minter = [1u8; 32];
        initialize(minter.as_ptr());
        let owner = [2u8; 32];
        mint_test_token(&minter, &owner, 1);
        mint_test_token(&minter, &owner, 2);

        test_mock::set_caller(minter);
        assert_eq!(set_max_supply(minter.as_ptr(), 1), 0);
        assert_eq!(set_max_supply(minter.as_ptr(), total_minted()), 1);
        assert_eq!(set_max_supply(minter.as_ptr(), 0), 1);

        assert_eq!(set_royalty(minter.as_ptr(), 1001), 0);
        assert_eq!(set_royalty(minter.as_ptr(), 1000), 1);

        let too_long_uri = alloc::vec![b'u'; MAX_BASE_URI_LEN + 1];
        assert_eq!(
            set_base_uri(
                minter.as_ptr(),
                too_long_uri.as_ptr(),
                too_long_uri.len() as u32
            ),
            0
        );
        let uri = b"ipfs://base/";
        assert_eq!(
            set_base_uri(minter.as_ptr(), uri.as_ptr(), uri.len() as u32),
            1
        );
    }

    #[test]
    fn test_self_mint_does_not_bypass_collection_authority() {
        setup();
        let minter = [1u8; 32];
        let attacker = [2u8; 32];
        let metadata = b"ipfs://unauthorized";
        initialize(minter.as_ptr());

        test_mock::set_caller(attacker);
        assert_eq!(
            mint(
                attacker.as_ptr(),
                attacker.as_ptr(),
                99,
                metadata.as_ptr(),
                metadata.len() as u32,
            ),
            0
        );
        assert_eq!(total_minted(), 0);
        assert!(make_nft().owner_of(99).is_err());
    }

    #[test]
    fn test_token_and_operator_approvals_are_queryable_and_exact() {
        setup();
        let minter = [1u8; 32];
        let owner = [2u8; 32];
        let spender = [3u8; 32];
        let operator = [4u8; 32];
        let recipient = [5u8; 32];
        initialize(minter.as_ptr());
        mint_test_token(&minter, &owner, 7);

        assert_eq!(get_approved(999), 0);
        test_mock::set_caller(owner);
        assert_eq!(approve(owner.as_ptr(), owner.as_ptr(), 7), 0);
        assert_eq!(approve(owner.as_ptr(), spender.as_ptr(), 7), 1);
        assert_eq!(get_approved(7), 1);
        assert_eq!(test_mock::get_return_data(), spender.to_vec());
        assert_eq!(
            set_approval_for_all(owner.as_ptr(), operator.as_ptr(), 1),
            1
        );
        assert_eq!(is_approved_for_all(owner.as_ptr(), operator.as_ptr()), 1);

        test_mock::set_caller(operator);
        assert_eq!(
            transfer_from(operator.as_ptr(), owner.as_ptr(), recipient.as_ptr(), 7),
            1
        );
        assert_eq!(get_approved(7), 1);
        assert_eq!(test_mock::get_return_data(), [0u8; 32]);
    }

    #[test]
    fn test_two_step_admin_rotation_separates_mint_and_royalty_authorities() {
        setup();
        let initial = [1u8; 32];
        let next_admin = [2u8; 32];
        let mint_authority = [3u8; 32];
        let royalty_recipient = [4u8; 32];
        let owner = [5u8; 32];
        let metadata = b"ipfs://authorized";
        initialize(initial.as_ptr());

        assert_eq!(propose_admin(initial.as_ptr(), next_admin.as_ptr()), 1);
        test_mock::set_caller(next_admin);
        assert_eq!(accept_admin(next_admin.as_ptr()), 1);
        test_mock::set_caller(initial);
        assert_eq!(
            set_mint_authority(initial.as_ptr(), mint_authority.as_ptr()),
            0
        );

        test_mock::set_caller(next_admin);
        assert_eq!(
            set_mint_authority(next_admin.as_ptr(), mint_authority.as_ptr()),
            1
        );
        assert_eq!(
            set_royalty_config(next_admin.as_ptr(), royalty_recipient.as_ptr(), 600),
            1
        );
        test_mock::set_caller(mint_authority);
        assert_eq!(
            mint(
                mint_authority.as_ptr(),
                owner.as_ptr(),
                1,
                metadata.as_ptr(),
                metadata.len() as u32,
            ),
            1
        );

        assert_eq!(royalty_info(1), 1);
        let royalty = test_mock::get_return_data();
        assert_eq!(&royalty[..32], &royalty_recipient);
        assert_eq!(&royalty[32..], &600u16.to_le_bytes());
        assert_eq!(get_collection_config(), 1);
        let config = test_mock::get_return_data();
        assert_eq!(config.len(), 145);
        assert_eq!(&config[..32], &next_admin);
        assert_eq!(&config[32..64], &[0u8; 32]);
        assert_eq!(&config[64..96], &mint_authority);
        assert_eq!(&config[96..128], &royalty_recipient);
        assert_eq!(bytes_to_u64(&config[128..136]), 600);
        assert_eq!(bytes_to_u64(&config[136..144]), 0);
        assert_eq!(config[144], 0);
    }

    #[test]
    fn test_malformed_state_blocks_mutation_and_stats_queries() {
        setup();
        let minter = [1u8; 32];
        let owner = [2u8; 32];
        let recipient = [3u8; 32];
        initialize(minter.as_ptr());
        mint_test_token(&minter, &owner, 1);

        storage_set(MP_TRANSFER_COUNT_KEY, &[1u8]);
        test_mock::set_caller(owner);
        assert_eq!(transfer(owner.as_ptr(), recipient.as_ptr(), 1), 0);
        assert_eq!(
            make_nft().owner_of(1).expect("owner remains"),
            Address(owner)
        );
        assert_eq!(get_collection_stats(), 1);

        storage_set(MP_TRANSFER_COUNT_KEY, &u64_to_bytes(0));
        storage_set(b"mp_paused", &[2u8]);
        assert_eq!(transfer(owner.as_ptr(), recipient.as_ptr(), 1), 0);
        assert_eq!(get_collection_config(), 0);
    }

    #[test]
    fn test_burned_token_metadata_and_approval_queries_fail() {
        setup();
        let minter = [1u8; 32];
        let owner = [2u8; 32];
        initialize(minter.as_ptr());
        mint_test_token(&minter, &owner, 1);
        test_mock::set_caller(owner);
        assert_eq!(burn(owner.as_ptr(), 1), 1);
        assert_eq!(get_punk_metadata(1), 0);
        assert_eq!(get_approved(1), 0);
    }
}
