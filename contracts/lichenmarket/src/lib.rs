// LichenMarket V3 - deterministic NFT marketplace settlement.
//
// Fixed-price sales escrow payment atomically. Offers escrow their full payment
// when created, and auctions custody the NFT before accepting bids. Fees,
// royalties, recoverable payouts, indexes, and token-specific metrics are exact
// and fail closed on malformed or overflowing state.

#![no_std]
#![cfg_attr(target_arch = "wasm32", no_main)]
// Named WASM exports receive pointers materialized and bounds-checked by the
// contract runtime before guest invocation.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

extern crate alloc;
use alloc::vec::Vec;

use lichen_sdk::{
    bytes_to_u64, call_native_nft_owner, call_native_nft_royalty_info,
    call_native_nft_transfer_from, call_nft_owner, call_nft_royalty_info, call_nft_transfer_from,
    get_caller, get_contract_address, get_value, is_native_token, log_info, native_balance_of,
    receive_token_or_native, storage_get, storage_set, transfer_token_or_native, u64_to_bytes,
    Address,
};

const MM_SALE_COUNT_KEY: &[u8] = b"mm_sale_count";
// `mm_sale_volume` is retained as immutable legacy evidence because the old
// contract mixed raw amounts from every payment token. V3 writes only to this
// explicitly native LICN volume ledger.
const MM_NATIVE_SALE_VOLUME_KEY: &[u8] = b"mm_native_sale_volume";
const MIN_OFFER_PRICE: u64 = 1_000_000; // 0.001 LICN (assuming 1e9 base units)
const MAX_ACTIVE_OFFERS_PER_WALLET: u64 = 64;
const SLOT_DURATION_MS: u64 = 400;
const AUCTION_MIN_DURATION_SLOTS: u64 = 60_000 / SLOT_DURATION_MS; // 1 minute
const AUCTION_MAX_DURATION_SLOTS: u64 = 30 * 24 * 60 * 60 * 1000 / SLOT_DURATION_MS; // 30 days
const AUCTION_SNIPE_WINDOW_SLOTS: u64 = 10 * 60 * 1_000 / SLOT_DURATION_MS;
const MAX_AUCTION_EXTENSIONS: u64 = 12;
const DEFAULT_MARKETPLACE_FEE_BPS: u64 = 250;
const MAX_MARKETPLACE_FEE_BPS: u64 = 1_000;
const MARKETPLACE_FEE_KEY: &[u8] = b"marketplace_fee";
const MARKETPLACE_FEE_TREASURY_KEY: &[u8] = b"marketplace_fee_addr";
const MARKETPLACE_OWNER_KEY: &[u8] = b"marketplace_owner";
const MARKETPLACE_PENDING_OWNER_KEY: &[u8] = b"marketplace_pending_owner";
const MM_METRICS_VERSION: u64 = 3;
const MM_METRICS_VERSION_KEY: &[u8] = b"mm_metrics_version";
const MM_METRICS_MIGRATION_LOCK_KEY: &[u8] = b"mm_metrics_mig_lock";
const MM_METRICS_MIGRATION_MANIFEST_KEY: &[u8] = b"mm_metrics_mig_manifest";
const MM_METRICS_MIGRATION_EXPECTED_ROWS_KEY: &[u8] = b"mm_metrics_mig_expected";
const MM_METRICS_MIGRATION_ROWS_KEY: &[u8] = b"mm_metrics_mig_rows";
const MM_METRICS_MIGRATION_EXPECTED_SALES_KEY: &[u8] = b"mm_metrics_mig_exp_sales";
const MM_METRICS_MIGRATION_SALES_KEY: &[u8] = b"mm_metrics_mig_sales";
const MM_METRICS_MIGRATION_NATIVE_VOLUME_KEY: &[u8] = b"mm_metrics_mig_native";
const MM_METRICS_MIGRATION_EXPECTED_CUSTODY_ROWS_KEY: &[u8] = b"mm_metrics_mig_exp_cust_rows";
const MM_METRICS_MIGRATION_CUSTODY_ROWS_KEY: &[u8] = b"mm_metrics_mig_cust_rows";
const MM_METRICS_MIGRATION_EXPECTED_NATIVE_CUSTODY_KEY: &[u8] = b"mm_metrics_mig_exp_native_cust";
const MM_METRICS_MIGRATION_NATIVE_CUSTODY_KEY: &[u8] = b"mm_metrics_mig_native_cust";
const MM_METRICS_MIGRATION_GLOBAL_KEY: &[u8] = b"mm_metrics_mig_global";
const MM_METRICS_MIGRATION_SAW_NATIVE_KEY: &[u8] = b"mm_metrics_mig_saw_native";
const MAX_METRICS_MIGRATION_ROWS: u64 = 1_000_000;

// Reentrancy guard
const MM_REENTRANCY_KEY: &[u8] = b"mm_reentrancy";

fn reentrancy_enter() -> bool {
    match storage_get(MM_REENTRANCY_KEY) {
        None => {}
        Some(value) if value.as_slice() == [0u8] => {}
        // Both the entered state and malformed state fail closed.
        Some(_) => return false,
    }
    storage_set(MM_REENTRANCY_KEY, &[1u8]);
    true
}

fn reentrancy_exit() {
    storage_set(MM_REENTRANCY_KEY, &[0u8]);
}

fn with_reentrancy_guard<F>(operation: F) -> u32
where
    F: FnOnce() -> u32,
{
    if !reentrancy_enter() {
        return 0;
    }
    let result = operation();
    reentrancy_exit();
    result
}

// Emergency pause
const MM_PAUSE_KEY: &[u8] = b"mm_paused";

fn is_mm_paused() -> bool {
    load_mm_pause_state().unwrap_or(true) || !metrics_v3_ready()
}

fn load_mm_pause_state() -> Option<bool> {
    match storage_get(MM_PAUSE_KEY) {
        None => Some(false),
        Some(value) if value.as_slice() == [0u8] => Some(false),
        Some(value) if value.as_slice() == [1u8] => Some(true),
        Some(_) => None,
    }
}

fn load_exact_bool(key: &[u8], missing: bool) -> Option<bool> {
    match storage_get(key) {
        None => Some(missing),
        Some(value) if value.as_slice() == [0u8] => Some(false),
        Some(value) if value.as_slice() == [1u8] => Some(true),
        Some(_) => None,
    }
}

fn metrics_v3_version() -> Option<u64> {
    match storage_get(MM_METRICS_VERSION_KEY) {
        None => Some(0),
        Some(value) if value.len() == 8 => Some(bytes_to_u64(&value)),
        Some(_) => None,
    }
}

fn metrics_v3_migration_locked() -> Option<bool> {
    load_exact_bool(MM_METRICS_MIGRATION_LOCK_KEY, false)
}

fn metrics_v3_ready() -> bool {
    metrics_v3_version() == Some(MM_METRICS_VERSION) && metrics_v3_migration_locked() == Some(false)
}

fn metrics_v3_migration_active() -> bool {
    metrics_v3_version() != Some(MM_METRICS_VERSION)
        || metrics_v3_migration_locked() != Some(false)
}

fn metrics_v3_manifest() -> Option<[u8; 32]> {
    match storage_get(MM_METRICS_MIGRATION_MANIFEST_KEY) {
        None => None,
        Some(value) if value.len() == 32 && value.as_slice() != [0u8; 32] => value.try_into().ok(),
        Some(_) => None,
    }
}

fn metrics_v3_manifest_sealed() -> bool {
    metrics_v3_version() == Some(0)
        && metrics_v3_migration_locked() == Some(true)
        && metrics_v3_manifest().is_some()
}

fn next_metrics_v3_custody_row() -> Option<u64> {
    if !metrics_v3_manifest_sealed() {
        return None;
    }
    let expected = load_u64_or_zero(MM_METRICS_MIGRATION_EXPECTED_CUSTODY_ROWS_KEY)?;
    let current = load_u64_or_zero(MM_METRICS_MIGRATION_CUSTODY_ROWS_KEY)?;
    current.checked_add(1).filter(|next| *next <= expected)
}

fn prepare_legacy_native_custody(amount: u64) -> Option<(u64, u64)> {
    let next_row = next_metrics_v3_custody_row()?;
    let expected_native = load_u64_or_zero(MM_METRICS_MIGRATION_EXPECTED_NATIVE_CUSTODY_KEY)?;
    let current_native = load_u64_or_zero(MM_METRICS_MIGRATION_NATIVE_CUSTODY_KEY)?;
    let next_native = current_native.checked_add(amount)?;
    if next_native > expected_native
        || native_balance_of(get_contract_address()).ok()? < expected_native
    {
        return None;
    }
    Some((next_row, next_native))
}

fn commit_legacy_custody_row(next_row: u64, next_native: Option<u64>) {
    storage_set(
        MM_METRICS_MIGRATION_CUSTODY_ROWS_KEY,
        &u64_to_bytes(next_row),
    );
    if let Some(next_native) = next_native {
        storage_set(
            MM_METRICS_MIGRATION_NATIVE_CUSTODY_KEY,
            &u64_to_bytes(next_native),
        );
    }
}

fn prepare_legacy_offer_custody(
    token: Address,
    offerer: Address,
    amount: u64,
) -> Option<(u64, Option<u64>)> {
    if is_native_token(&token) {
        let value = get_value();
        if value != 0 && value != amount {
            return None;
        }
        let (row, native) = prepare_legacy_native_custody(amount)?;
        Some((row, Some(native)))
    } else {
        if get_value() != 0 {
            return None;
        }
        let row = next_metrics_v3_custody_row()?;
        matches!(
            receive_token_or_native(token, offerer, get_contract_address(), amount),
            Ok(true)
        )
        .then_some((row, None))
    }
}

fn metrics_v3_token_marker_key(token: Address) -> Vec<u8> {
    let mut key = b"mm_metrics_mig_token:".to_vec();
    key.extend_from_slice(&token.0);
    key
}

fn is_mm_admin(caller: &[u8]) -> bool {
    storage_get(MARKETPLACE_OWNER_KEY)
        .map(|d| d.len() == 32 && d.as_slice() == caller)
        .unwrap_or(false)
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

fn unpaid_payout_key(token: Address, recipient: Address) -> Vec<u8> {
    let mut key = b"unpaid_payout:".to_vec();
    key.extend_from_slice(&token.0);
    key.push(b':');
    key.extend_from_slice(&recipient.0);
    key
}

fn unpaid_payout_custody_key(token: Address, recipient: Address) -> Vec<u8> {
    let mut key = b"mm_unpaid_custody:".to_vec();
    key.extend_from_slice(&token.0);
    key.push(b':');
    key.extend_from_slice(&recipient.0);
    key
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

fn next_u64_value(key: &[u8], increment: u64) -> Option<u64> {
    load_u64_or_zero(key)?.checked_add(increment)
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
    if current > 0
        && load_exact_bool(&unpaid_payout_custody_key(token, recipient), false) != Some(true)
    {
        return false;
    }
    match next_u64_value(&key, amount) {
        Some(total) => {
            storage_set(&key, &u64_to_bytes(total));
            storage_set(&unpaid_payout_custody_key(token, recipient), &[1u8]);
            true
        }
        None => false,
    }
}

fn can_record_unpaid_payout(token: Address, recipient: Address, amount: u64) -> bool {
    if amount == 0 {
        return true;
    }
    let key = unpaid_payout_key(token, recipient);
    match load_u64_or_zero(&key) {
        Some(0) => next_u64_value(&key, amount).is_some(),
        Some(_) => {
            load_exact_bool(&unpaid_payout_custody_key(token, recipient), false) == Some(true)
                && next_u64_value(&key, amount).is_some()
        }
        None => false,
    }
}

fn can_record_unpaid_payouts(token: Address, payouts: &[(Address, u64)]) -> bool {
    for (index, (recipient, amount)) in payouts.iter().enumerate() {
        if *amount == 0 {
            continue;
        }
        let Some(combined) = payouts[index + 1..]
            .iter()
            .filter(|(other, _)| other == recipient)
            .try_fold(*amount, |total, (_, value)| total.checked_add(*value))
        else {
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

fn marketplace_escrow_address() -> Option<Address> {
    // All settlement funds remain under the marketplace contract's authority.
    // `marketplace_fee_addr` is the fee treasury, never an external escrow.
    Some(get_contract_address())
}

fn platform_fee_key(token: Address) -> Vec<u8> {
    let mut key = b"mm_platform_fee:".to_vec();
    key.extend_from_slice(&token.0);
    key
}

fn token_sale_count_key(token: Address) -> Vec<u8> {
    let mut key = b"mm_token_sale_count:".to_vec();
    key.extend_from_slice(&token.0);
    key
}

fn token_sale_volume_key(token: Address) -> Vec<u8> {
    let mut key = b"mm_token_sale_volume:".to_vec();
    key.extend_from_slice(&token.0);
    key
}

fn token_sale_fees_key(token: Address) -> Vec<u8> {
    let mut key = b"mm_token_sale_fees:".to_vec();
    key.extend_from_slice(&token.0);
    key
}

#[cfg(test)]
fn accrue_platform_fee(token: Address, amount: u64) -> bool {
    if amount == 0 {
        return true;
    }
    let key = platform_fee_key(token);
    match next_u64_value(&key, amount) {
        Some(total) => {
            storage_set(&key, &u64_to_bytes(total));
            true
        }
        None => false,
    }
}

struct PreparedSaleAccounting {
    platform_fee: u64,
    sale_count: u64,
    native_sale_volume: u64,
    token_sale_count: u64,
    token_sale_volume: u64,
    token_sale_fees: u64,
}

fn prepare_sale_accounting(
    token: Address,
    fee_amount: u64,
    price: u64,
) -> Option<PreparedSaleAccounting> {
    let current_native_sale_volume = load_u64_or_zero(MM_NATIVE_SALE_VOLUME_KEY)?;
    Some(PreparedSaleAccounting {
        platform_fee: next_u64_value(&platform_fee_key(token), fee_amount)?,
        sale_count: next_u64_value(MM_SALE_COUNT_KEY, 1)?,
        native_sale_volume: if is_native_token(&token) {
            current_native_sale_volume.checked_add(price)?
        } else {
            current_native_sale_volume
        },
        token_sale_count: next_u64_value(&token_sale_count_key(token), 1)?,
        token_sale_volume: next_u64_value(&token_sale_volume_key(token), price)?,
        token_sale_fees: next_u64_value(&token_sale_fees_key(token), fee_amount)?,
    })
}

fn commit_sale_accounting(token: Address, accounting: PreparedSaleAccounting) {
    storage_set(
        &platform_fee_key(token),
        &u64_to_bytes(accounting.platform_fee),
    );
    storage_set(MM_SALE_COUNT_KEY, &u64_to_bytes(accounting.sale_count));
    storage_set(
        MM_NATIVE_SALE_VOLUME_KEY,
        &u64_to_bytes(accounting.native_sale_volume),
    );
    storage_set(
        &token_sale_count_key(token),
        &u64_to_bytes(accounting.token_sale_count),
    );
    storage_set(
        &token_sale_volume_key(token),
        &u64_to_bytes(accounting.token_sale_volume),
    );
    storage_set(
        &token_sale_fees_key(token),
        &u64_to_bytes(accounting.token_sale_fees),
    );
}

fn listing_fee_key(nft_contract: Address, token_id: u64) -> Vec<u8> {
    let mut key = b"mm_listing_fee:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.extend_from_slice(&token_id.to_le_bytes());
    key
}

fn listing_slot_key(nft_contract: Address, token_id: u64) -> Vec<u8> {
    let mut key = b"mm_listing_slot:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.extend_from_slice(&token_id.to_le_bytes());
    key
}

fn offer_fee_key(nft_contract: Address, token_id: u64, offerer: &[u8; 32]) -> Vec<u8> {
    let mut key = b"mm_offer_fee:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.extend_from_slice(&token_id.to_le_bytes());
    key.extend_from_slice(offerer);
    key
}

fn offer_custody_key(nft_contract: Address, token_id: u64, offerer: &[u8; 32]) -> Vec<u8> {
    let mut key = b"mm_offer_custody:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.extend_from_slice(&token_id.to_le_bytes());
    key.extend_from_slice(offerer);
    key
}

fn offer_custody_ready(
    nft_contract: Address,
    token_id: u64,
    offerer: &[u8; 32],
) -> Option<bool> {
    load_exact_bool(&offer_custody_key(nft_contract, token_id, offerer), false)
}

fn auction_fee_key(nft_contract: Address, token_id: u64) -> Vec<u8> {
    let mut key = b"mm_auction_fee:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.extend_from_slice(&token_id.to_le_bytes());
    key
}

fn collection_offer_fee_key(collection: Address, offerer: Address) -> Vec<u8> {
    let mut key = b"mm_collection_offer_fee:".to_vec();
    key.extend_from_slice(&collection.0);
    key.extend_from_slice(&offerer.0);
    key
}

fn collection_offer_custody_key(collection: Address, offerer: Address) -> Vec<u8> {
    let mut key = b"mm_collection_offer_custody:".to_vec();
    key.extend_from_slice(&collection.0);
    key.extend_from_slice(&offerer.0);
    key
}

fn collection_offer_custody_ready(collection: Address, offerer: Address) -> Option<bool> {
    load_exact_bool(&collection_offer_custody_key(collection, offerer), false)
}

fn exact_payment_value(token: Address, amount: u64) -> bool {
    if is_native_token(&token) {
        get_value() == amount
    } else {
        get_value() == 0
    }
}

fn receive_offer_custody(token: Address, payer: Address, amount: u64) -> bool {
    if !exact_payment_value(token, amount) {
        return false;
    }
    matches!(
        receive_token_or_native(token, payer, get_contract_address(), amount),
        Ok(true)
    )
}

fn release_offer_custody(token: Address, recipient: Address, amount: u64) -> bool {
    get_value() == 0
        && matches!(
            transfer_token_or_native(token, get_contract_address(), recipient, amount),
            Ok(true)
        )
}

fn snapshotted_fee_bps(key: &[u8]) -> Option<u64> {
    match storage_get(key) {
        Some(data) if data.len() == 8 => {
            let bps = bytes_to_u64(&data);
            (bps <= MAX_MARKETPLACE_FEE_BPS).then_some(bps)
        }
        // Pre-V3 rows must be explicitly migrated at the upgrade boundary.
        None => None,
        Some(_) => None,
    }
}

fn collection_royalty_key(nft_contract: Address) -> Vec<u8> {
    let mut key = b"mm_collection_royalty:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key
}

fn nft_owner_of(nft_contract: Address, token_id: u64) -> Option<Address> {
    match call_nft_owner(nft_contract, token_id) {
        Ok(owner) if owner.0 != [0u8; 32] => Some(owner),
        Ok(_) => None,
        Err(_) => call_native_nft_owner(nft_contract, token_id)
            .ok()
            .filter(|owner| owner.0 != [0u8; 32]),
    }
}

fn nft_owned_by(nft_contract: Address, token_id: u64, expected_owner: Address) -> bool {
    nft_owner_of(nft_contract, token_id) == Some(expected_owner)
}

fn transfer_nft_from_market(
    nft_contract: Address,
    from: Address,
    to: Address,
    token_id: u64,
) -> bool {
    let marketplace = get_contract_address();
    match call_nft_transfer_from(nft_contract, marketplace, from, to, token_id) {
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

fn cache_collection_royalty(nft_contract: Address, recipient: Address, bps: u16) {
    let mut data = Vec::with_capacity(34);
    data.extend_from_slice(&recipient.0);
    data.extend_from_slice(&bps.to_le_bytes());
    storage_set(&collection_royalty_key(nft_contract), &data);
}

fn offer_royalty_key(nft_contract: Address, token_id: u64, offerer: &[u8; 32]) -> Vec<u8> {
    let mut key = b"mm_offer_royalty:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.extend_from_slice(&token_id.to_le_bytes());
    key.extend_from_slice(offerer);
    key
}

fn collection_offer_royalty_key(collection: Address, offerer: Address) -> Vec<u8> {
    let mut key = b"mm_collection_offer_royalty:".to_vec();
    key.extend_from_slice(&collection.0);
    key.extend_from_slice(&offerer.0);
    key
}

fn store_royalty_snapshot(key: &[u8], recipient: Address, bps: u16) {
    let mut data = Vec::with_capacity(34);
    data.extend_from_slice(&recipient.0);
    data.extend_from_slice(&bps.to_le_bytes());
    storage_set(key, &data);
}

fn load_royalty_snapshot(key: &[u8]) -> Option<(Address, u16)> {
    match storage_get(key) {
        Some(data) if data.len() == 34 => {
            let mut recipient = [0u8; 32];
            recipient.copy_from_slice(&data[..32]);
            let bps = u16::from_le_bytes([data[32], data[33]]);
            (bps <= 1_000 && (bps == 0 || recipient != [0u8; 32]))
                .then_some((Address(recipient), bps))
        }
        _ => None,
    }
}

fn offerer_active_count_key(offerer: &[u8; 32]) -> Vec<u8> {
    let mut key = b"offerer_count:".to_vec();
    key.extend_from_slice(offerer);
    key
}

fn load_offerer_active_count(offerer: &[u8; 32]) -> Option<u64> {
    load_u64_or_zero(&offerer_active_count_key(offerer))
}

#[cfg(test)]
fn get_offerer_active_count(offerer: &[u8; 32]) -> u64 {
    load_offerer_active_count(offerer).unwrap_or(0)
}

fn set_offerer_active_count(offerer: &[u8; 32], count: u64) {
    storage_set(&offerer_active_count_key(offerer), &u64_to_bytes(count));
}

fn active_offer_count_key(nft_contract: Address, token_id: u64) -> Vec<u8> {
    let mut key = b"mm_active_offer_count:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.extend_from_slice(&token_id.to_le_bytes());
    key
}

fn offer_index_marker_key(nft_contract: Address, token_id: u64, offerer: &[u8; 32]) -> Vec<u8> {
    let mut key = b"mm_offer_indexed:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.extend_from_slice(&token_id.to_le_bytes());
    key.extend_from_slice(offerer);
    key
}

fn offer_is_indexed(nft_contract: Address, token_id: u64, offerer: &[u8; 32]) -> Option<bool> {
    match storage_get(&offer_index_marker_key(nft_contract, token_id, offerer)) {
        None => Some(false),
        Some(value) if value.as_slice() == [0u8] => Some(false),
        Some(value) if value.as_slice() == [1u8] => Some(true),
        Some(_) => None,
    }
}

fn offer_index_state_matches(
    nft_contract: Address,
    token_id: u64,
    offerer: &[u8; 32],
    active: bool,
) -> bool {
    offer_is_indexed(nft_contract, token_id, offerer) == Some(active)
}

fn load_active_offer_count(nft_contract: Address, token_id: u64) -> Option<u64> {
    load_u64_or_zero(&active_offer_count_key(nft_contract, token_id))
}

fn prepare_offer_activation_counts(
    nft_contract: Address,
    token_id: u64,
    offerer: &[u8; 32],
    was_active: bool,
) -> Option<(u64, u64)> {
    let wallet_count = load_offerer_active_count(offerer)?;
    let nft_count = load_active_offer_count(nft_contract, token_id)?;
    if was_active {
        return Some((wallet_count, nft_count));
    }
    if wallet_count >= MAX_ACTIVE_OFFERS_PER_WALLET {
        return None;
    }
    Some((wallet_count.checked_add(1)?, nft_count.checked_add(1)?))
}

fn prepare_offer_release_counts(
    nft_contract: Address,
    token_id: u64,
    offerer: &[u8; 32],
) -> Option<(u64, u64)> {
    Some((
        load_offerer_active_count(offerer)?.checked_sub(1)?,
        load_active_offer_count(nft_contract, token_id)?.checked_sub(1)?,
    ))
}

fn commit_offer_counts(
    nft_contract: Address,
    token_id: u64,
    offerer: &[u8; 32],
    wallet_count: u64,
    nft_count: u64,
) {
    set_offerer_active_count(offerer, wallet_count);
    storage_set(
        &active_offer_count_key(nft_contract, token_id),
        &u64_to_bytes(nft_count),
    );
}

fn set_offer_indexed(nft_contract: Address, token_id: u64, offerer: &[u8; 32], indexed: bool) {
    storage_set(
        &offer_index_marker_key(nft_contract, token_id, offerer),
        &[u8::from(indexed)],
    );
}

/// Listing layout (147 bytes):
///   0..32   seller
///   32..64  nft_contract
///   64..72  token_id (u64 LE)
///   72..80  price (u64 LE)
///   80..112 payment_token
///   112..144 royalty_recipient (zero = no royalty)
///   144     active (1=active, 0=inactive)
///   145..147 royalty_bps (u16 LE)
const LISTING_SIZE: usize = 147;

fn structurally_valid_listing_record(data: &[u8], nft_contract: Address, token_id: u64) -> bool {
    if data.len() != LISTING_SIZE
        || data[..32] == [0u8; 32]
        || data[32..64] != nft_contract.0
        || bytes_to_u64(&data[64..72]) != token_id
        || bytes_to_u64(&data[72..80]) == 0
        || data[144] > 1
    {
        return false;
    }
    let royalty_bps = u16::from_le_bytes([data[145], data[146]]);
    royalty_bps <= 5_000 && (royalty_bps == 0 || data[112..144] != [0u8; 32])
}

fn valid_listing_record(data: &[u8], nft_contract: Address, token_id: u64) -> bool {
    structurally_valid_listing_record(data, nft_contract, token_id)
        && u16::from_le_bytes([data[145], data[146]]) <= 1_000
}

/// Initialize the marketplace with a self-custody escrow and fee treasury.
#[no_mangle]
pub extern "C" fn initialize(owner_ptr: *const u8, fee_treasury_ptr: *const u8) {
    // Re-initialization guard: reject if marketplace_owner is already set
    if storage_get(MARKETPLACE_OWNER_KEY).is_some() {
        log_info("LichenMarket already initialized — ignoring");
        return;
    }

    let owner = match read_address(owner_ptr) {
        Some(address) if address.0 != [0u8; 32] => address,
        _ => return,
    };
    let fee_treasury = match read_address(fee_treasury_ptr) {
        Some(address) if address.0 != [0u8; 32] => address,
        _ => return,
    };

    // AUDIT-FIX: verify caller matches transaction signer
    let real_caller = get_caller();
    if real_caller != owner {
        return;
    }

    storage_set(
        MARKETPLACE_FEE_KEY,
        &u64_to_bytes(DEFAULT_MARKETPLACE_FEE_BPS),
    );
    storage_set(MARKETPLACE_OWNER_KEY, &owner.0);
    storage_set(MARKETPLACE_FEE_TREASURY_KEY, &fee_treasury.0);
    storage_set(MM_PAUSE_KEY, &[0u8]);
    storage_set(MM_REENTRANCY_KEY, &[0u8]);
    storage_set(MM_METRICS_VERSION_KEY, &u64_to_bytes(MM_METRICS_VERSION));
    storage_set(MM_METRICS_MIGRATION_LOCK_KEY, &[0u8]);
    storage_set(MM_METRICS_MIGRATION_EXPECTED_ROWS_KEY, &u64_to_bytes(0));
    storage_set(MM_METRICS_MIGRATION_ROWS_KEY, &u64_to_bytes(0));
    storage_set(MM_METRICS_MIGRATION_EXPECTED_SALES_KEY, &u64_to_bytes(0));
    storage_set(MM_METRICS_MIGRATION_SALES_KEY, &u64_to_bytes(0));
    storage_set(MM_METRICS_MIGRATION_NATIVE_VOLUME_KEY, &u64_to_bytes(0));
    storage_set(
        MM_METRICS_MIGRATION_EXPECTED_CUSTODY_ROWS_KEY,
        &u64_to_bytes(0),
    );
    storage_set(MM_METRICS_MIGRATION_CUSTODY_ROWS_KEY, &u64_to_bytes(0));
    storage_set(
        MM_METRICS_MIGRATION_EXPECTED_NATIVE_CUSTODY_KEY,
        &u64_to_bytes(0),
    );
    storage_set(MM_METRICS_MIGRATION_NATIVE_CUSTODY_KEY, &u64_to_bytes(0));
    storage_set(MM_METRICS_MIGRATION_GLOBAL_KEY, &[0u8]);
    storage_set(MM_METRICS_MIGRATION_SAW_NATIVE_KEY, &[0u8]);
    log_info("Lichen Market NFT Marketplace initialized");
}

/// List an NFT for sale
/// Writes the canonical 147-byte record and freezes settlement terms.
#[no_mangle]
pub extern "C" fn list_nft(
    seller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    price: u64,
    payment_token_ptr: *const u8,
) -> u32 {
    if is_mm_paused() {
        return 0;
    }
    if get_value() != 0 {
        return 0;
    }
    if price == 0 {
        log_info("Price must be > 0");
        return 0;
    }
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
        let fee_bps = match get_marketplace_fee() {
            Some(fee) => fee,
            None => return 0,
        };

        // Verify ownership and bind the listing to royalty terms returned by
        // the collection itself (contract MT-721 or system-native NFT).
        if nft_owned_by(nft_contract, token_id, seller) {
            match canonical_collection_royalty(nft_contract, token_id) {
                Some((royalty_recipient, royalty_bps)) => {
                    // Store the canonical listing record.
                    let listing_key = create_listing_key(nft_contract, token_id);
                    match storage_get(&listing_key) {
                        Some(existing)
                            if !valid_listing_record(&existing, nft_contract, token_id) =>
                        {
                            log_info("Existing listing state is malformed");
                            return 0;
                        }
                        Some(existing) if existing[144] == 1 => {
                            log_info("Active listing already exists for this NFT");
                            return 0;
                        }
                        _ => {}
                    }
                    let next_listing_count = match next_u64_value(b"mm_listing_count", 1) {
                        Some(value) => value,
                        None => {
                            log_info("Listing count would overflow or is malformed");
                            return 0;
                        }
                    };

                    let mut listing_data = alloc::vec![0u8; LISTING_SIZE];
                    listing_data[0..32].copy_from_slice(&seller.0);
                    listing_data[32..64].copy_from_slice(&nft_contract.0);
                    listing_data[64..72].copy_from_slice(&token_id.to_le_bytes());
                    listing_data[72..80].copy_from_slice(&price.to_le_bytes());
                    listing_data[80..112].copy_from_slice(&payment_token.0);
                    listing_data[112..144].copy_from_slice(&royalty_recipient.0);
                    listing_data[144] = 1; // active = true
                    listing_data[145..147].copy_from_slice(&royalty_bps.to_le_bytes());

                    storage_set(&listing_key, &listing_data);
                    storage_set(
                        &listing_fee_key(nft_contract, token_id),
                        &u64_to_bytes(fee_bps),
                    );
                    storage_set(
                        &listing_slot_key(nft_contract, token_id),
                        &u64_to_bytes(lichen_sdk::get_slot()),
                    );
                    cache_collection_royalty(nft_contract, royalty_recipient, royalty_bps);

                    storage_set(b"mm_listing_count", &u64_to_bytes(next_listing_count));

                    log_info("NFT listed for sale");
                    1
                }
                None => {
                    log_info("NFT collection royalty terms are unavailable or invalid");
                    0
                }
            }
        } else {
            log_info("Seller does not own NFT");
            0
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

        if !valid_listing_record(&listing_data, nft_contract, token_id) {
            log_info("Invalid listing data");
            reentrancy_exit();
            return 0;
        }

        // Parse listing
        let mut seller_bytes = [0u8; 32];
        seller_bytes.copy_from_slice(&listing_data[0..32]);
        let seller = Address(seller_bytes);
        if buyer == seller {
            log_info("Buyer cannot purchase their own listing");
            reentrancy_exit();
            return 0;
        }

        let mut price_bytes = [0u8; 8];
        price_bytes.copy_from_slice(&listing_data[72..80]);
        let price = u64::from_le_bytes(price_bytes);

        let mut payment_token_bytes = [0u8; 32];
        payment_token_bytes.copy_from_slice(&listing_data[80..112]);
        let payment_token = Address(payment_token_bytes);
        if !exact_payment_value(payment_token, price) {
            log_info("Purchase attached value does not exactly match its payment token");
            reentrancy_exit();
            return 0;
        }

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

        // Settlement uses the fee captured when the listing was created.
        let fee = match snapshotted_fee_bps(&listing_fee_key(nft_contract, token_id)) {
            Some(fee) => fee,
            None => {
                reentrancy_exit();
                return 0;
            }
        };
        if royalty_bps > 1_000 || (royalty_bps > 0 && !has_royalty) {
            reentrancy_exit();
            return 0;
        }
        // Use u128 to prevent overflow on large NFT prices
        let fee_amount = ((price as u128) * (fee as u128) / 10000) as u64;
        // v3: Calculate royalty
        let royalty_amount = if has_royalty && royalty_bps > 0 {
            ((price as u128) * (royalty_bps as u128) / 10000) as u64
        } else {
            0
        };
        let seller_amount = match fee_amount
            .checked_add(royalty_amount)
            .and_then(|deductions| price.checked_sub(deductions))
        {
            Some(amount) => amount,
            None => {
                reentrancy_exit();
                return 0;
            }
        };
        if !can_record_unpaid_payouts(
            payment_token,
            &[(seller, seller_amount), (royalty_recipient, royalty_amount)],
        ) {
            reentrancy_exit();
            return 0;
        }
        let sale_accounting = match prepare_sale_accounting(payment_token, fee_amount, price) {
            Some(next) => next,
            None => {
                reentrancy_exit();
                return 0;
            }
        };

        log_info("Executing purchase with escrow pattern...");

        // AUDIT-FIX 1.12: Escrow pattern — hold payment in marketplace until
        // NFT transfer confirms. Prevents buyer losing funds if NFT transfer fails.
        let marketplace_addr = match marketplace_escrow_address() {
            Some(addr) => addr,
            None => {
                log_info("marketplace_fee_addr not configured — purchase rejected");
                reentrancy_exit();
                return 0;
            }
        };
        if marketplace_addr.0 == [0u8; 32] && !is_native_token(&payment_token) {
            log_info("marketplace_fee_addr not configured — purchase rejected");
            reentrancy_exit();
            return 0;
        }

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
        if transfer_nft_from_market(nft_contract, seller, buyer, token_id) {
            log_info("NFT transferred to buyer");
        } else {
            // NFT transfer failed — refund buyer from escrow
            log_info("NFT transfer failed — refunding buyer from escrow");
            match transfer_token_or_native(payment_token, marketplace_addr, buyer, price) {
                Ok(true) => log_info("Buyer refunded from escrow"),
                _ => {
                    if !record_unpaid_payout(payment_token, buyer, price) {
                        reentrancy_exit();
                        return 0;
                    }
                    log_info("Escrow refund failed; buyer payout recorded");
                }
            }
            reentrancy_exit();
            return 0;
        }

        // STEP 3: Release escrowed funds — seller gets their share
        match transfer_token_or_native(payment_token, marketplace_addr, seller, seller_amount) {
            Ok(true) => log_info("Seller payment released from escrow"),
            _ => {
                if !record_unpaid_payout(payment_token, seller, seller_amount) {
                    reentrancy_exit();
                    return 0;
                }
                log_info(" Seller payment release failed; payout recorded");
            }
        }

        // STEP 4 (v3): Pay royalty to creator if applicable
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
                _ => {
                    if !record_unpaid_payout(payment_token, royalty_recipient, royalty_amount) {
                        reentrancy_exit();
                        return 0;
                    }
                    log_info("Royalty transfer failed; payout recorded");
                }
            }
        }

        // STEP 5: Commit all prevalidated custody and aggregate accounting.
        commit_sale_accounting(payment_token, sale_accounting);

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
    if metrics_v3_migration_active() {
        return 0;
    }
    if get_value() != 0 {
        return 0;
    }
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
            // Cancellation only unwinds seller-owned state and transfers no
            // funds, so structurally valid pre-V3 royalty rows remain
            // cancellable even though they cannot settle before migration.
            Some(data) if structurally_valid_listing_record(&data, nft_contract, token_id) => data,
            None => return 0,
            Some(_) => return 0,
        };

        // Verify caller is seller
        if listing_data[..32] != seller.0 {
            log_info("Only seller can cancel listing");
            return 0;
        }

        if listing_data[144] != 1 {
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
    _out_ptr: *mut u8,
) -> u32 {
    unsafe {
        let nft_contract = parse_address(nft_contract_ptr);
        let listing_key = create_listing_key(nft_contract, token_id);

        match storage_get(&listing_key) {
            Some(data) if valid_listing_record(&data, nft_contract, token_id) => {
                lichen_sdk::set_return_data(&data);
                1
            }
            _ => 0,
        }
    }
}

/// Set marketplace fee (owner only)
#[no_mangle]
pub extern "C" fn set_marketplace_fee(caller_ptr: *const u8, new_fee: u64) -> u32 {
    if !metrics_v3_ready() {
        return 0;
    }
    if get_value() != 0 {
        return 0;
    }
    if new_fee > MAX_MARKETPLACE_FEE_BPS {
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

    let owner = match storage_get(MARKETPLACE_OWNER_KEY) {
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

    storage_set(MARKETPLACE_FEE_KEY, &u64_to_bytes(new_fee));
    log_info("Prospective marketplace fee updated");
    1
}

/// Update the recipient of realized platform fees. Escrow custody is always
/// held by this contract and is not changed by this operation.
#[no_mangle]
pub extern "C" fn set_fee_treasury(caller_ptr: *const u8, treasury_ptr: *const u8) -> u32 {
    if !metrics_v3_ready() {
        return 2;
    }
    if get_value() != 0 {
        return 2;
    }
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let treasury = match read_address(treasury_ptr) {
        Some(address) if address.0 != [0u8; 32] => address,
        _ => return 2,
    };
    if get_caller() != caller {
        return 200;
    }
    if !is_mm_admin(&caller.0) {
        return 1;
    }
    storage_set(MARKETPLACE_FEE_TREASURY_KEY, &treasury.0);
    0
}

/// Withdraw an exact amount of realized, custody-backed platform fees.
/// The fee ledger is restored exactly if the token/native transfer fails.
#[no_mangle]
pub extern "C" fn withdraw_platform_fees(
    caller_ptr: *const u8,
    token_ptr: *const u8,
    amount: u64,
) -> u32 {
    if !metrics_v3_ready() {
        return 5;
    }
    if get_value() != 0 {
        return 5;
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
    if !is_mm_admin(&caller.0) || amount == 0 {
        reentrancy_exit();
        return 1;
    }
    let treasury = match storage_get(MARKETPLACE_FEE_TREASURY_KEY) {
        Some(data) if data.len() == 32 => {
            let mut address = [0u8; 32];
            address.copy_from_slice(&data);
            Address(address)
        }
        _ => {
            reentrancy_exit();
            return 2;
        }
    };
    let key = platform_fee_key(token);
    let accrued = match load_u64_or_zero(&key) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 5;
        }
    };
    let remaining = match accrued.checked_sub(amount) {
        Some(value) => value,
        None => {
            reentrancy_exit();
            return 3;
        }
    };
    storage_set(&key, &u64_to_bytes(remaining));
    match transfer_token_or_native(token, get_contract_address(), treasury, amount) {
        Ok(true) => {
            lichen_sdk::set_return_data(&u64_to_bytes(amount));
            reentrancy_exit();
            0
        }
        _ => {
            storage_set(&key, &u64_to_bytes(accrued));
            reentrancy_exit();
            4
        }
    }
}

#[no_mangle]
pub extern "C" fn get_platform_fees(token_ptr: *const u8) -> u32 {
    let token = match read_address(token_ptr) {
        Some(address) => address,
        None => return 98,
    };
    match load_u64_or_zero(&platform_fee_key(token)) {
        Some(amount) => {
            lichen_sdk::set_return_data(&u64_to_bytes(amount));
            0
        }
        None => 1,
    }
}

/// Refresh the marketplace cache for royalty terms reported by the collection
/// itself. Supplied terms are accepted only when they exactly match the
/// canonical contract/native NFT response.
#[no_mangle]
pub extern "C" fn set_collection_royalty(
    caller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    recipient_ptr: *const u8,
    royalty_bps: u64,
) -> u32 {
    if !metrics_v3_ready() {
        return 3;
    }
    if get_value() != 0 {
        return 3;
    }
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let recipient = match read_address(recipient_ptr) {
        Some(address) => address,
        None => return 98,
    };
    if get_caller() != caller {
        return 200;
    }
    if !is_mm_admin(&caller.0) {
        return 1;
    }
    let Some((canonical_recipient, canonical_bps)) = canonical_collection_royalty(nft_contract, 0)
    else {
        return 2;
    };
    if recipient != canonical_recipient || royalty_bps != u64::from(canonical_bps) {
        return 2;
    }
    cache_collection_royalty(nft_contract, canonical_recipient, canonical_bps);
    0
}

#[no_mangle]
pub extern "C" fn get_collection_royalty(nft_contract_ptr: *const u8) -> u32 {
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let (recipient, bps) = match canonical_collection_royalty(nft_contract, 0) {
        Some(terms) => terms,
        None => return 2,
    };
    let mut data = Vec::with_capacity(40);
    data.extend_from_slice(&recipient.0);
    data.extend_from_slice(&u64_to_bytes(u64::from(bps)));
    lichen_sdk::set_return_data(&data);
    0
}

/// Return the canonical royalty terms for one NFT as
/// [recipient(32), royalty_bps(8)]. This is the source-bound input used by
/// migration tooling for token-specific legacy offers.
#[no_mangle]
pub extern "C" fn get_canonical_royalty(nft_contract_ptr: *const u8, token_id: u64) -> u32 {
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let (recipient, bps) = match canonical_collection_royalty(nft_contract, token_id) {
        Some(terms) => terms,
        None => return 2,
    };
    let mut data = Vec::with_capacity(40);
    data.extend_from_slice(&recipient.0);
    data.extend_from_slice(&u64_to_bytes(u64::from(bps)));
    lichen_sdk::set_return_data(&data);
    0
}

// ============================================================================
// v2: LIST WITH ROYALTY
// ============================================================================

/// List an NFT while explicitly confirming the admin-verified collection
/// royalty. The caller cannot bypass or forge collection royalty terms.
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
    if is_mm_paused() {
        return 0;
    }
    if get_value() != 0 {
        return 0;
    }
    if price == 0 {
        log_info("Price must be > 0");
        return 0;
    }
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

        let (configured_royalty, configured_bps) =
            match canonical_collection_royalty(nft_contract, token_id) {
                Some(terms) => terms,
                None => {
                    log_info("NFT collection royalty terms are unavailable or invalid");
                    return 0;
                }
            };
        if royalty != configured_royalty || royalty_bps != u32::from(configured_bps) {
            log_info("Royalty terms do not match the verified collection policy");
            return 0;
        }
        let fee_bps = match get_marketplace_fee() {
            Some(fee) => fee,
            None => return 0,
        };

        if nft_owned_by(nft_contract, token_id, seller) {
            let listing_key = create_listing_key(nft_contract, token_id);
            match storage_get(&listing_key) {
                Some(existing) if !valid_listing_record(&existing, nft_contract, token_id) => {
                    log_info("Existing listing state is malformed");
                    return 0;
                }
                Some(existing) if existing[144] == 1 => {
                    log_info("Active listing already exists for this NFT");
                    return 0;
                }
                _ => {}
            }
            let next_listing_count = match next_u64_value(b"mm_listing_count", 1) {
                Some(value) => value,
                None => {
                    log_info("Listing count would overflow or is malformed");
                    return 0;
                }
            };
            let mut data = alloc::vec![0u8; LISTING_SIZE];
            data[0..32].copy_from_slice(&seller.0);
            data[32..64].copy_from_slice(&nft_contract.0);
            data[64..72].copy_from_slice(&token_id.to_le_bytes());
            data[72..80].copy_from_slice(&price.to_le_bytes());
            data[80..112].copy_from_slice(&payment_token.0);
            data[112..144].copy_from_slice(&royalty.0);
            data[144] = 1;
            data[145..147].copy_from_slice(&configured_bps.to_le_bytes());
            storage_set(&listing_key, &data);
            storage_set(
                &listing_fee_key(nft_contract, token_id),
                &u64_to_bytes(fee_bps),
            );
            storage_set(
                &listing_slot_key(nft_contract, token_id),
                &u64_to_bytes(lichen_sdk::get_slot()),
            );
            cache_collection_royalty(nft_contract, configured_royalty, configured_bps);

            storage_set(b"mm_listing_count", &u64_to_bytes(next_listing_count));
            log_info("NFT listed with royalty recipient");
            1
        } else {
            log_info("Seller does not own NFT");
            0
        }
    }
}

// ============================================================================
// v2: OFFERS
// ============================================================================

/// Make an offer on an NFT (even if not listed).
/// Offer layout: [offerer(32), price(8), payment_token(32), active(1)] = 73 bytes
const OFFER_SIZE: usize = 73;
const OFFER_EXPIRY_SIZE: usize = 81;

fn valid_offer_record(data: &[u8], offerer: &[u8; 32]) -> bool {
    matches!(data.len(), OFFER_SIZE | OFFER_EXPIRY_SIZE)
        && data[..32] == offerer[..]
        && bytes_to_u64(&data[32..40]) >= MIN_OFFER_PRICE
        && data[72] <= 1
}

fn create_funded_offer(
    offerer: Address,
    nft_contract: Address,
    token_id: u64,
    price: u64,
    payment_token: Address,
    expiry: Option<u64>,
) -> u32 {
    if is_mm_paused() {
        return 0;
    }
    if price < MIN_OFFER_PRICE {
        log_info("Offer price below minimum floor");
        return 0;
    }
    if expiry.is_some_and(|slot| slot > 0 && slot <= lichen_sdk::get_slot()) {
        log_info("Offer expiry must be in the future");
        return 0;
    }
    if get_caller() != offerer {
        return 200;
    }

    with_reentrancy_guard(|| {
        let (royalty_recipient, royalty_bps) =
            match canonical_collection_royalty(nft_contract, token_id) {
                Some(terms) => terms,
                None => {
                    log_info("NFT collection royalty terms are unavailable or invalid");
                    return 0;
                }
            };
        let fee_bps = match get_marketplace_fee() {
            Some(fee) => fee,
            None => return 0,
        };

        let mut key = b"offer:".to_vec();
        key.extend_from_slice(&nft_contract.0);
        key.push(b':');
        key.extend_from_slice(&token_id.to_le_bytes());
        key.push(b':');
        key.extend_from_slice(&offerer.0);

        match storage_get(&key) {
            Some(data) if !valid_offer_record(&data, &offerer.0) => {
                log_info("Existing offer state is malformed");
                return 0;
            }
            Some(data) if data[72] == 1 => {
                log_info("Cancel the active offer before replacing it");
                return 0;
            }
            _ => {}
        }
        if !offer_index_state_matches(nft_contract, token_id, &offerer.0, false) {
            log_info("Offer index state is malformed or requires migration");
            return 0;
        }
        let (next_wallet_count, next_nft_count) =
            match prepare_offer_activation_counts(nft_contract, token_id, &offerer.0, false) {
                Some(counts) => counts,
                None => {
                    log_info("Offer counts are malformed, overflowed, or at the wallet limit");
                    return 0;
                }
            };
        if next_wallet_count > MAX_ACTIVE_OFFERS_PER_WALLET {
            log_info("Per-wallet active offer limit reached");
            return 0;
        }
        if offer_custody_ready(nft_contract, token_id, &offerer.0) != Some(false) {
            log_info("Offer custody state is malformed");
            return 0;
        }
        if !receive_offer_custody(payment_token, offerer, price) {
            log_info("Offer payment escrow failed");
            return 0;
        }

        let mut data = alloc::vec![0u8; if expiry.is_some() {
            OFFER_EXPIRY_SIZE
        } else {
            OFFER_SIZE
        }];
        data[0..32].copy_from_slice(&offerer.0);
        data[32..40].copy_from_slice(&price.to_le_bytes());
        data[40..72].copy_from_slice(&payment_token.0);
        data[72] = 1;
        if let Some(expiry) = expiry {
            data[73..81].copy_from_slice(&expiry.to_le_bytes());
        }
        storage_set(&key, &data);
        storage_set(
            &offer_fee_key(nft_contract, token_id, &offerer.0),
            &u64_to_bytes(fee_bps),
        );
        store_royalty_snapshot(
            &offer_royalty_key(nft_contract, token_id, &offerer.0),
            royalty_recipient,
            royalty_bps,
        );
        commit_offer_counts(
            nft_contract,
            token_id,
            &offerer.0,
            next_wallet_count,
            next_nft_count,
        );
        set_offer_indexed(nft_contract, token_id, &offerer.0, true);
        storage_set(
            &offer_custody_key(nft_contract, token_id, &offerer.0),
            &[1u8],
        );
        log_info("Funded offer placed");
        1
    })
}

#[no_mangle]
pub extern "C" fn make_offer(
    offerer_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    price: u64,
    payment_token_ptr: *const u8,
) -> u32 {
    let offerer = match read_address(offerer_ptr) {
        Some(address) => address,
        None => return 0,
    };
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) => address,
        None => return 0,
    };
    let payment_token = match read_address(payment_token_ptr) {
        Some(address) => address,
        None => return 0,
    };
    create_funded_offer(offerer, nft_contract, token_id, price, payment_token, None)
}

/// Cancel an offer
#[no_mangle]
pub extern "C" fn cancel_offer(
    offerer_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
) -> u32 {
    if metrics_v3_migration_active() {
        return 0;
    }
    let offerer = match read_address(offerer_ptr) {
        Some(address) => address,
        None => return 0,
    };
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) => address,
        None => return 0,
    };
    if get_caller() != offerer {
        return 200;
    }
    if get_value() != 0 {
        return 0;
    }

    with_reentrancy_guard(|| {
        let mut key = b"offer:".to_vec();
        key.extend_from_slice(&nft_contract.0);
        key.push(b':');
        key.extend_from_slice(&token_id.to_le_bytes());
        key.push(b':');
        key.extend_from_slice(&offerer.0);

        let data = match storage_get(&key) {
            Some(data) if valid_offer_record(&data, &offerer.0) && data[72] == 1 => data,
            _ => return 0,
        };
        let indexed = match offer_is_indexed(nft_contract, token_id, &offerer.0) {
            Some(value) => value,
            None => {
                log_info("Offer index state is malformed");
                return 0;
            }
        };
        let custody = match offer_custody_ready(nft_contract, token_id, &offerer.0) {
            Some(value) => value,
            None => {
                log_info("Offer custody state is malformed");
                return 0;
            }
        };

        if !indexed {
            // A pre-V3 offer can still be abandoned before the manifest is
            // sealed. It has no certified contract custody, so no funds are
            // released from the V3 escrow ledger.
            if custody || metrics_v3_manifest().is_some() {
                log_info("Legacy offer must complete migration before cancellation");
                return 0;
            }
            let next_wallet_count = match load_offerer_active_count(&offerer.0)
                .and_then(|count| count.checked_sub(1))
            {
                Some(count) => count,
                None => {
                    log_info("Legacy offer wallet count is malformed");
                    return 0;
                }
            };
            let mut updated = data;
            updated[72] = 0;
            storage_set(&key, &updated);
            set_offerer_active_count(&offerer.0, next_wallet_count);
            set_offer_indexed(nft_contract, token_id, &offerer.0, false);
            log_info("Unfunded legacy offer cancelled");
            return 1;
        }
        let (next_wallet_count, next_nft_count) =
            match prepare_offer_release_counts(nft_contract, token_id, &offerer.0) {
                Some(counts) => counts,
                None => {
                    log_info("Offer counts are malformed or require migration");
                    return 0;
                }
            };
        if !custody {
            if metrics_v3_manifest().is_some() {
                log_info("Legacy offer must complete custody migration");
                return 0;
            }
            let mut updated = data;
            updated[72] = 0;
            storage_set(&key, &updated);
            commit_offer_counts(
                nft_contract,
                token_id,
                &offerer.0,
                next_wallet_count,
                next_nft_count,
            );
            set_offer_indexed(nft_contract, token_id, &offerer.0, false);
            log_info("Unfunded indexed legacy offer cancelled");
            return 1;
        }
        let price = bytes_to_u64(&data[32..40]);
        let mut token = [0u8; 32];
        token.copy_from_slice(&data[40..72]);
        if !release_offer_custody(Address(token), offerer, price) {
            log_info("Offer refund failed");
            return 0;
        }

        let mut updated = data;
        updated[72] = 0;
        storage_set(&key, &updated);
        commit_offer_counts(
            nft_contract,
            token_id,
            &offerer.0,
            next_wallet_count,
            next_nft_count,
        );
        set_offer_indexed(nft_contract, token_id, &offerer.0, false);
        storage_set(
            &offer_custody_key(nft_contract, token_id, &offerer.0),
            &[0u8],
        );
        log_info("Funded offer cancelled and refunded");
        1
    })
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
        if seller.0 == offerer {
            log_info("Seller cannot accept their own offer");
            reentrancy_exit();
            return 0;
        }

        // AUDIT-FIX: verify caller matches transaction signer
        let real_caller = get_caller();
        if real_caller.0 != seller.0 {
            reentrancy_exit();
            return 200;
        }

        if !nft_owned_by(nft_contract, token_id, seller) {
            log_info("Seller does not own NFT");
            reentrancy_exit();
            return 0;
        }

        // Load offer
        let mut key = b"offer:".to_vec();
        key.extend_from_slice(&nft_contract.0);
        key.push(b':');
        key.extend_from_slice(&token_id.to_le_bytes());
        key.push(b':');
        key.extend_from_slice(&offerer);

        let data = match storage_get(&key) {
            Some(d) if valid_offer_record(&d, &offerer) && d[72] == 1 => d,
            _ => {
                log_info("Active offer not found");
                reentrancy_exit();
                return 0;
            }
        };
        if !offer_index_state_matches(nft_contract, token_id, &offerer, true) {
            log_info("Offer index state is malformed or requires migration");
            reentrancy_exit();
            return 0;
        }
        if offer_custody_ready(nft_contract, token_id, &offerer) != Some(true) {
            log_info("Offer payment custody is not ready");
            reentrancy_exit();
            return 0;
        }
        if get_value() != 0 {
            log_info("Offer acceptance must not attach native value");
            reentrancy_exit();
            return 0;
        }
        if data.len() == OFFER_EXPIRY_SIZE {
            let mut expiry_bytes = [0u8; 8];
            expiry_bytes.copy_from_slice(&data[73..81]);
            let expiry = u64::from_le_bytes(expiry_bytes);
            if expiry > 0 && lichen_sdk::get_slot() > expiry {
                log_info("Offer has expired");
                reentrancy_exit();
                return 0;
            }
        }

        let mut price_bytes = [0u8; 8];
        price_bytes.copy_from_slice(&data[32..40]);
        let price = u64::from_le_bytes(price_bytes);

        let mut pay_bytes = [0u8; 32];
        pay_bytes.copy_from_slice(&data[40..72]);
        let payment_token = Address(pay_bytes);

        // Calculate immutable fee and royalty terms captured with the offer.
        let fee = match snapshotted_fee_bps(&offer_fee_key(nft_contract, token_id, &offerer)) {
            Some(fee) => fee,
            None => {
                reentrancy_exit();
                return 0;
            }
        };
        // Use u128 to prevent overflow on large NFT prices
        let fee_amount = ((price as u128) * (fee as u128) / 10000) as u64;
        let (royalty_recipient, royalty_bps) =
            match load_royalty_snapshot(&offer_royalty_key(nft_contract, token_id, &offerer)) {
                Some(terms) => terms,
                None => {
                    reentrancy_exit();
                    return 0;
                }
            };
        let royalty_amount = ((price as u128) * (royalty_bps as u128) / 10_000) as u64;
        let deductions = match fee_amount.checked_add(royalty_amount) {
            Some(amount) => amount,
            None => {
                reentrancy_exit();
                return 0;
            }
        };
        let seller_amount = match price.checked_sub(deductions) {
            Some(amount) => amount,
            None => {
                reentrancy_exit();
                return 0;
            }
        };
        if !can_record_unpaid_payouts(
            payment_token,
            &[(seller, seller_amount), (royalty_recipient, royalty_amount)],
        ) {
            reentrancy_exit();
            return 0;
        }
        let sale_accounting = match prepare_sale_accounting(payment_token, fee_amount, price) {
            Some(next) => next,
            None => {
                reentrancy_exit();
                return 0;
            }
        };
        let (next_offerer_count, next_nft_offer_count) =
            match prepare_offer_release_counts(nft_contract, token_id, &offerer) {
                Some(next) => next,
                None => {
                    log_info("Offer counts are malformed or require migration");
                    reentrancy_exit();
                    return 0;
                }
            };

        let buyer = Address(offerer);
        let marketplace_addr = get_contract_address();

        // Transfer NFT
        if transfer_nft_from_market(nft_contract, seller, buyer, token_id) {
            match transfer_token_or_native(payment_token, marketplace_addr, seller, seller_amount) {
                Ok(true) => log_info("Offer seller payment released"),
                _ => {
                    if !record_unpaid_payout(payment_token, seller, seller_amount) {
                        reentrancy_exit();
                        return 0;
                    }
                    log_info("Offer seller payment failed; payout recorded");
                }
            }
            if royalty_amount > 0 {
                match transfer_token_or_native(
                    payment_token,
                    marketplace_addr,
                    royalty_recipient,
                    royalty_amount,
                ) {
                    Ok(true) => {}
                    _ => {
                        if !record_unpaid_payout(payment_token, royalty_recipient, royalty_amount) {
                            reentrancy_exit();
                            return 0;
                        }
                    }
                }
            }
            commit_sale_accounting(payment_token, sale_accounting);

            // Deactivate offer
            let mut updated = data;
            updated[72] = 0;
            storage_set(&key, &updated);
            commit_offer_counts(
                nft_contract,
                token_id,
                &offerer,
                next_offerer_count,
                next_nft_offer_count,
            );
            set_offer_indexed(nft_contract, token_id, &offerer, false);
            storage_set(
                &offer_custody_key(nft_contract, token_id, &offerer),
                &[0u8],
            );

            log_info("Offer accepted, trade executed");
            reentrancy_exit();
            1
        } else {
            // The offer stays active and fully funded. This call has no
            // authority to unwind the offerer's custody on an NFT failure.
            log_info("Offer NFT transfer failed; funded offer remains active");
            reentrancy_exit();
            0
        }
    }
}

// ============================================================================
// v2: MARKETPLACE STATS
// ============================================================================

/// Get marketplace stats:
/// [listing_count(8), fee_bps(8), sale_count(8), native_sale_volume(8)].
#[no_mangle]
pub extern "C" fn get_marketplace_stats() -> u32 {
    if !metrics_v3_ready() {
        return 1;
    }
    let count = match load_u64_or_zero(b"mm_listing_count") {
        Some(value) => value,
        None => return 1,
    };
    let fee = match get_marketplace_fee() {
        Some(value) => value,
        None => return 1,
    };
    let sale_count = match load_u64_or_zero(MM_SALE_COUNT_KEY) {
        Some(value) => value,
        None => return 1,
    };
    let native_sale_volume = match load_u64_or_zero(MM_NATIVE_SALE_VOLUME_KEY) {
        Some(value) => value,
        None => return 1,
    };
    let mut result = Vec::with_capacity(32);
    result.extend_from_slice(&u64_to_bytes(count));
    result.extend_from_slice(&u64_to_bytes(fee));
    result.extend_from_slice(&u64_to_bytes(sale_count));
    result.extend_from_slice(&u64_to_bytes(native_sale_volume));
    lichen_sdk::set_return_data(&result);
    0
}

/// Return exact sale count, raw volume, lifetime realized fees, and currently
/// withdrawable fees for one payment token. Cross-token amounts are never
/// added together.
#[no_mangle]
pub extern "C" fn get_marketplace_token_stats(token_ptr: *const u8) -> u32 {
    if !metrics_v3_ready() {
        return 1;
    }
    let token = match read_address(token_ptr) {
        Some(token) => token,
        None => return 98,
    };
    let count = match load_u64_or_zero(&token_sale_count_key(token)) {
        Some(value) => value,
        None => return 1,
    };
    let volume = match load_u64_or_zero(&token_sale_volume_key(token)) {
        Some(value) => value,
        None => return 1,
    };
    let realized_fees = match load_u64_or_zero(&token_sale_fees_key(token)) {
        Some(value) => value,
        None => return 1,
    };
    let withdrawable_fees = match load_u64_or_zero(&platform_fee_key(token)) {
        Some(value) => value,
        None => return 1,
    };
    let mut result = Vec::with_capacity(32);
    result.extend_from_slice(&u64_to_bytes(count));
    result.extend_from_slice(&u64_to_bytes(volume));
    result.extend_from_slice(&u64_to_bytes(realized_fees));
    result.extend_from_slice(&u64_to_bytes(withdrawable_fees));
    lichen_sdk::set_return_data(&result);
    0
}

// ============================================================================
// v3: NFT ATTRIBUTES (rarity, category, traits)
// ============================================================================

const MAX_TRAITS_BYTES: usize = 2_048;
const MAX_TRAIT_COUNT: u16 = 64;

fn canonical_trait_count(payload: &[u8]) -> Option<u16> {
    let mut cursor = 0usize;
    let mut count = 0u16;
    while cursor < payload.len() {
        let key_len = usize::from(*payload.get(cursor)?);
        cursor = cursor.checked_add(1)?;
        if key_len == 0 || cursor.checked_add(key_len)? > payload.len() {
            return None;
        }
        let key_start = cursor;
        let key_end = cursor.checked_add(key_len)?;
        cursor = key_end;
        let value_len = usize::from(*payload.get(cursor)?);
        cursor = cursor.checked_add(1)?;
        let value_end = cursor.checked_add(value_len)?;
        if value_end > payload.len() {
            return None;
        }

        // Duplicate keys make attributes ambiguous for indexers and clients.
        let mut prior_cursor = 0usize;
        while prior_cursor < key_start.saturating_sub(1) {
            let prior_key_len = usize::from(*payload.get(prior_cursor)?);
            prior_cursor = prior_cursor.checked_add(1)?;
            let prior_key_end = prior_cursor.checked_add(prior_key_len)?;
            if prior_key_end > payload.len() {
                return None;
            }
            if payload.get(prior_cursor..prior_key_end)? == payload.get(key_start..key_end)? {
                return None;
            }
            prior_cursor = prior_key_end;
            let prior_value_len = usize::from(*payload.get(prior_cursor)?);
            prior_cursor = prior_cursor.checked_add(1 + prior_value_len)?;
        }

        cursor = value_end;
        count = count.checked_add(1)?;
        if count > MAX_TRAIT_COUNT {
            return None;
        }
    }
    Some(count)
}

fn valid_attribute_record(data: &[u8]) -> bool {
    if !(4..=4 + MAX_TRAITS_BYTES).contains(&data.len()) || data[0] > 4 || data[1] > 6 {
        return false;
    }
    let declared_count = u16::from_le_bytes([data[2], data[3]]);
    canonical_trait_count(&data[4..]) == Some(declared_count)
}

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
    if !metrics_v3_ready() {
        return 0;
    }
    if get_value() != 0 {
        return 0;
    }
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
    let is_owner = nft_owned_by(nft_contract, token_id, Address(caller));
    if !is_owner && !is_mm_admin(&caller) {
        log_info("Only NFT owner or admin can set attributes");
        return 0;
    }

    let mut key = b"nft_attr:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.push(b':');
    key.extend_from_slice(&token_id.to_le_bytes());

    let traits_len = traits_len as usize;
    if traits_len > MAX_TRAITS_BYTES {
        log_info("Traits payload exceeds 2 KiB");
        return 0;
    }
    let traits_slice = if traits_len == 0 {
        &[][..]
    } else {
        unsafe { core::slice::from_raw_parts(traits_ptr, traits_len) }
    };
    let trait_count = match canonical_trait_count(traits_slice) {
        Some(count) => count,
        None => {
            log_info("Traits payload is not canonical");
            return 0;
        }
    };

    // Build a canonical, self-validating attribute record.
    let mut data = Vec::with_capacity(4 + traits_len);
    data.push(rarity);
    data.push(category);
    data.extend_from_slice(&trait_count.to_le_bytes());
    data.extend_from_slice(traits_slice);

    storage_set(&key, &data);
    log_info("NFT attributes updated");
    1
}

/// Get NFT attributes — returns [rarity(1), category(1), trait_count(2), traits...]
#[no_mangle]
pub extern "C" fn get_nft_attributes(
    nft_contract_ptr: *const u8,
    token_id: u64,
    _out_ptr: *mut u8,
) -> u32 {
    let nft_contract = unsafe { parse_address(nft_contract_ptr) };
    let mut key = b"nft_attr:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.push(b':');
    key.extend_from_slice(&token_id.to_le_bytes());

    match storage_get(&key) {
        Some(data) if valid_attribute_record(&data) => {
            lichen_sdk::set_return_data(&data);
            data.len() as u32
        }
        _ => 0,
    }
}

// ============================================================================
// v3: QUERY FUNCTIONS (offers, filtered listings)
// ============================================================================

/// Return the exact indexed number of active offers for an NFT.
#[no_mangle]
pub extern "C" fn get_offer_count(nft_contract_ptr: *const u8, token_id: u64) -> u32 {
    let nft_contract = unsafe { parse_address(nft_contract_ptr) };
    let count: u32 = match load_active_offer_count(nft_contract, token_id)
        .and_then(|value| value.try_into().ok())
    {
        Some(count) => count,
        None => return u32::MAX,
    };
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
    if is_mm_paused() {
        return 0;
    }
    if get_value() != 0 {
        return 0;
    }
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
            Some(data) if valid_listing_record(&data, nft_contract, token_id) => data,
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
        if !nft_owned_by(nft_contract, token_id, seller) {
            log_info("Seller no longer owns the NFT");
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

fn get_marketplace_fee() -> Option<u64> {
    match storage_get(MARKETPLACE_FEE_KEY) {
        Some(bytes) if bytes.len() == 8 => {
            let bps = bytes_to_u64(&bytes);
            (bps <= MAX_MARKETPLACE_FEE_BPS).then_some(bps)
        }
        None => Some(DEFAULT_MARKETPLACE_FEE_BPS),
        Some(_) => None,
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

/// Auction layout (211 bytes):
///   0..32   seller
///   32..64  nft_contract
///   64..72  token_id (u64 LE)
///   72..80  start_price (u64 LE)
///   80..88  reserve_price (u64 LE)
///   88..96  highest_bid (u64 LE)
///   96..128 highest_bidder (32 bytes, zero = no bids)
///   128..136 start_slot (u64 LE)
///   136..144 end_slot (u64 LE)
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

fn auction_escrow_key(nft_contract: Address, token_id: u64) -> Vec<u8> {
    let mut key = b"mm_auction_escrowed:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.extend_from_slice(&token_id.to_le_bytes());
    key
}

fn auction_extension_key(nft_contract: Address, token_id: u64) -> Vec<u8> {
    let mut key = b"mm_auction_extensions:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.extend_from_slice(&token_id.to_le_bytes());
    key
}

fn auction_bid_custody_key(nft_contract: Address, token_id: u64) -> Vec<u8> {
    let mut key = b"mm_auction_bid_custody:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.extend_from_slice(&token_id.to_le_bytes());
    key
}

fn auction_bid_custody_ready(nft_contract: Address, token_id: u64) -> Option<bool> {
    load_exact_bool(&auction_bid_custody_key(nft_contract, token_id), false)
}

fn auction_escrowed(nft_contract: Address, token_id: u64) -> Option<bool> {
    match storage_get(&auction_escrow_key(nft_contract, token_id)) {
        None => Some(false),
        Some(data) if data.as_slice() == [0u8] => Some(false),
        Some(data) if data.as_slice() == [1u8] => Some(true),
        Some(_) => None,
    }
}

fn valid_auction_record(data: &[u8], nft_contract: Address, token_id: u64) -> bool {
    if data.len() != AUCTION_SIZE
        || data[..32] == [0u8; 32]
        || data[32..64] != nft_contract.0
        || bytes_to_u64(&data[64..72]) != token_id
        || bytes_to_u64(&data[72..80]) == 0
        || bytes_to_u64(&data[128..136]) > bytes_to_u64(&data[136..144])
        || data[144] > 2
    {
        return false;
    }
    let highest_bid = bytes_to_u64(&data[88..96]);
    let highest_bidder_is_zero = data[96..128] == [0u8; 32];
    let royalty_bps = u16::from_le_bytes([data[209], data[210]]);
    (highest_bid == 0) == highest_bidder_is_zero
        && royalty_bps <= 1_000
        && (royalty_bps == 0 || data[177..209] != [0u8; 32])
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
    if get_value() != 0 {
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
        if reserve_price > 0 && reserve_price < start_price {
            log_info("Reserve price must be zero or at least the start price");
            return 0;
        }
        if !(AUCTION_MIN_DURATION_SLOTS..=AUCTION_MAX_DURATION_SLOTS).contains(&duration) {
            log_info("Duration must be 1 minute - 30 days in slots");
            return 0;
        }

        // Verify ownership
        if !nft_owned_by(nft_contract, token_id, seller) {
            log_info("Seller does not own NFT");
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
        let fee_bps = match get_marketplace_fee() {
            Some(fee) => fee,
            None => {
                log_info("Marketplace fee state is malformed");
                return 0;
            }
        };

        // Check no existing active auction
        let key = create_auction_key(nft_contract, token_id);
        match storage_get(&key) {
            Some(existing) if !valid_auction_record(&existing, nft_contract, token_id) => {
                log_info("Existing auction state is malformed");
                return 0;
            }
            Some(existing) if existing[144] == 1 => {
                log_info("Active auction already exists for this NFT");
                return 0;
            }
            _ => {}
        }

        let now = lichen_sdk::get_slot();
        let end_time = match now.checked_add(duration) {
            Some(v) => v,
            None => {
                log_info("Auction end time overflow");
                return 0;
            }
        };

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
        data[177..209].copy_from_slice(&royalty_recipient.0);
        data[209..211].copy_from_slice(&royalty_bps.to_le_bytes());

        // Custody the NFT before bids can be accepted. This removes the
        // seller-approval revocation race that could strand bidder funds.
        if !transfer_nft_from_market(nft_contract, seller, get_contract_address(), token_id) {
            log_info("Auction NFT escrow failed; approve LichenMarket first");
            return 0;
        }
        storage_set(&key, &data);
        storage_set(
            &auction_fee_key(nft_contract, token_id),
            &u64_to_bytes(fee_bps),
        );
        storage_set(&auction_escrow_key(nft_contract, token_id), &[1u8]);
        storage_set(&auction_bid_custody_key(nft_contract, token_id), &[1u8]);
        storage_set(
            &auction_extension_key(nft_contract, token_id),
            &u64_to_bytes(0),
        );
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
            Some(d) if valid_auction_record(&d, nft_contract, token_id) && d[144] == 1 => d,
            _ => {
                log_info("Active auction not found");
                reentrancy_exit();
                return 0;
            }
        };
        match auction_escrowed(nft_contract, token_id) {
            Some(true) if nft_owned_by(nft_contract, token_id, get_contract_address()) => {}
            Some(true) => {
                log_info("Auction custody marker does not match NFT ownership");
                reentrancy_exit();
                return 0;
            }
            Some(false) => {
                log_info("Legacy auction must escrow its NFT before accepting bids");
                reentrancy_exit();
                return 0;
            }
            None => {
                log_info("Auction custody state is malformed");
                reentrancy_exit();
                return 0;
            }
        }
        if auction_bid_custody_ready(nft_contract, token_id) != Some(true) {
            log_info("Auction bid custody requires V3 migration");
            reentrancy_exit();
            return 0;
        }
        if data[..32] == bidder.0 {
            log_info("Seller cannot bid on their own auction");
            reentrancy_exit();
            return 0;
        }

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
            let increment = ((current_highest as u128) * 500 / 10_000).max(1) as u64;
            match current_highest.checked_add(increment) {
                Some(v) => v,
                None => {
                    log_info("Current highest bid cannot be outbid");
                    reentrancy_exit();
                    return 0;
                }
            }
        } else {
            start_price
        };
        if bid_amount < min_bid {
            log_info("Bid too low");
            reentrancy_exit();
            return 0;
        }

        // Compute every fallible state transition before moving funds.
        let extension_key = auction_extension_key(nft_contract, token_id);
        let extension_count = match load_u64_or_zero(&extension_key) {
            Some(value) => value,
            None => {
                log_info("Auction extension state is malformed");
                reentrancy_exit();
                return 0;
            }
        };
        let time_left = end_time.saturating_sub(now);
        let (new_end_time, new_extension_count) =
            if time_left < AUCTION_SNIPE_WINDOW_SLOTS && extension_count < MAX_AUCTION_EXTENSIONS {
                let new_end = match now.checked_add(AUCTION_SNIPE_WINDOW_SLOTS) {
                    Some(value) => value,
                    None => {
                        log_info("Auction anti-sniping extension overflow");
                        reentrancy_exit();
                        return 0;
                    }
                };
                let next_count = match extension_count.checked_add(1) {
                    Some(value) => value,
                    None => {
                        reentrancy_exit();
                        return 0;
                    }
                };
                (new_end, next_count)
            } else {
                (end_time, extension_count)
            };

        // Parse payment token
        let mut pay_bytes = [0u8; 32];
        pay_bytes.copy_from_slice(&data[145..177]);
        let payment_token = Address(pay_bytes);
        if !exact_payment_value(payment_token, bid_amount) {
            log_info("Bid attached value does not exactly match its payment token");
            reentrancy_exit();
            return 0;
        }
        if !can_record_unpaid_payout(payment_token, bidder, bid_amount) {
            log_info("Bid refund liability would overflow or is malformed");
            reentrancy_exit();
            return 0;
        }

        // Escrow bid under this contract's authority.
        let marketplace_addr = get_contract_address();

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
                match transfer_token_or_native(
                    payment_token,
                    marketplace_addr,
                    prev_bidder,
                    current_highest,
                ) {
                    Ok(true) => {}
                    _ => {
                        log_info("Previous bidder refund failed; refunding new bidder");
                        match transfer_token_or_native(
                            payment_token,
                            marketplace_addr,
                            bidder,
                            bid_amount,
                        ) {
                            Ok(true) => {}
                            _ => {
                                if !record_unpaid_payout(payment_token, bidder, bid_amount) {
                                    reentrancy_exit();
                                    return 0;
                                }
                            }
                        }
                        reentrancy_exit();
                        return 0;
                    }
                }
            }
        }

        // Update auction with new highest bid
        let mut updated = data;
        updated[88..96].copy_from_slice(&bid_amount.to_le_bytes());
        updated[96..128].copy_from_slice(&bidder.0);
        updated[136..144].copy_from_slice(&new_end_time.to_le_bytes());

        storage_set(&key, &updated);
        storage_set(&extension_key, &u64_to_bytes(new_extension_count));
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
    if !metrics_v3_ready() {
        return 0;
    }
    if get_value() != 0 {
        return 0;
    }
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
            Some(d) if valid_auction_record(&d, nft_contract, token_id) && d[144] == 1 => d,
            _ => {
                log_info("Active auction not found");
                reentrancy_exit();
                return 0;
            }
        };
        let escrowed = match auction_escrowed(nft_contract, token_id) {
            Some(escrowed) => escrowed,
            None => {
                log_info("Auction custody state is malformed");
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
        if highest_bid > 0 && auction_bid_custody_ready(nft_contract, token_id) != Some(true) {
            log_info("Auction bid custody requires V3 migration");
            reentrancy_exit();
            return 0;
        }

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

        let marketplace_addr = match marketplace_escrow_address() {
            Some(addr) => addr,
            None => {
                log_info("marketplace_fee_addr not configured — auction finalize rejected");
                reentrancy_exit();
                return 0;
            }
        };

        // Legacy auctions cannot safely settle a sale until the seller moves
        // the NFT into this contract. At expiry they fail safe by refunding
        // any bidder and leaving the NFT with its existing owner.
        let no_sale = highest_bid == 0 || (reserve_price > 0 && highest_bid < reserve_price);
        if !escrowed || no_sale {
            let bidder = Address(bidder_bytes);
            if highest_bid > 0 && !can_record_unpaid_payout(payment_token, bidder, highest_bid) {
                log_info("Auction refund liability would overflow or is malformed");
                reentrancy_exit();
                return 0;
            }
            if escrowed
                && !transfer_nft_from_market(nft_contract, get_contract_address(), seller, token_id)
            {
                log_info("Auction NFT return failed; settlement remains retryable");
                reentrancy_exit();
                return 0;
            }
            if highest_bid > 0 && bidder_bytes != [0u8; 32] {
                match transfer_token_or_native(payment_token, marketplace_addr, bidder, highest_bid)
                {
                    Ok(true) => {}
                    _ => {
                        if !record_unpaid_payout(payment_token, bidder, highest_bid) {
                            reentrancy_exit();
                            return 0;
                        }
                        log_info("Auction refund failed; bidder payout recorded");
                    }
                }
            }
            let mut updated = data;
            updated[144] = 0; // cancelled
            storage_set(&key, &updated);
            storage_set(&auction_escrow_key(nft_contract, token_id), &[0u8]);
            storage_set(&auction_bid_custody_key(nft_contract, token_id), &[0u8]);
            log_info("Auction closed without sale; NFT returned and bidder refunded");
            reentrancy_exit();
            return 2; // settled with no sale
        }

        let winner = Address(bidder_bytes);
        let price = highest_bid;

        // Calculate fee + royalty
        let fee = match snapshotted_fee_bps(&auction_fee_key(nft_contract, token_id)) {
            Some(fee) => fee,
            None => {
                log_info("Auction fee snapshot is malformed");
                reentrancy_exit();
                return 0;
            }
        };
        let fee_amount = ((price as u128) * (fee as u128) / 10000) as u64;

        let mut royalty_recip_bytes = [0u8; 32];
        royalty_recip_bytes.copy_from_slice(&data[177..209]);
        let has_royalty = royalty_recip_bytes != [0u8; 32];
        let mut rbps = [0u8; 2];
        rbps.copy_from_slice(&data[209..211]);
        let royalty_bps = u64::from(u16::from_le_bytes(rbps));
        let royalty_amount = if has_royalty && royalty_bps > 0 {
            ((price as u128) * (royalty_bps as u128) / 10000) as u64
        } else {
            0
        };

        let deductions = match fee_amount.checked_add(royalty_amount) {
            Some(amount) => amount,
            None => {
                reentrancy_exit();
                return 0;
            }
        };
        let seller_amount = match price.checked_sub(deductions) {
            Some(amount) => amount,
            None => {
                reentrancy_exit();
                return 0;
            }
        };
        let royalty_recipient = Address(royalty_recip_bytes);
        if !can_record_unpaid_payouts(
            payment_token,
            &[(seller, seller_amount), (royalty_recipient, royalty_amount)],
        ) {
            log_info("Auction payout ledger would overflow or is malformed");
            reentrancy_exit();
            return 0;
        }
        let sale_accounting = match prepare_sale_accounting(payment_token, fee_amount, price) {
            Some(next) => next,
            None => {
                log_info("Auction accounting would overflow or is malformed");
                reentrancy_exit();
                return 0;
            }
        };

        // Release the NFT held by this contract to the winner.
        if !transfer_nft_from_market(nft_contract, get_contract_address(), winner, token_id) {
            log_info("NFT transfer failed in auction settlement");
            reentrancy_exit();
            return 0;
        }

        // Pay seller from escrow
        match transfer_token_or_native(payment_token, marketplace_addr, seller, seller_amount) {
            Ok(true) => {}
            _ => {
                if !record_unpaid_payout(payment_token, seller, seller_amount) {
                    reentrancy_exit();
                    return 0;
                }
                log_info("Auction seller payment failed; payout recorded");
            }
        }
        // Pay the immutable creator royalty; failed transfers remain an exact
        // creator liability rather than being redirected to the seller.
        if royalty_amount > 0 {
            match transfer_token_or_native(
                payment_token,
                marketplace_addr,
                royalty_recipient,
                royalty_amount,
            ) {
                Ok(true) => {
                    log_info("Auction royalty paid");
                }
                _ => {
                    if !record_unpaid_payout(payment_token, royalty_recipient, royalty_amount) {
                        reentrancy_exit();
                        return 0;
                    }
                    log_info("Auction royalty payment failed; creator payout recorded");
                }
            }
        }

        commit_sale_accounting(payment_token, sale_accounting);

        // Mark auction as settled
        let mut updated = data;
        updated[144] = 2; // settled
        storage_set(&key, &updated);
        storage_set(&auction_escrow_key(nft_contract, token_id), &[0u8]);
        storage_set(&auction_bid_custody_key(nft_contract, token_id), &[0u8]);

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
    if metrics_v3_migration_active() {
        return 0;
    }
    if get_value() != 0 {
        return 0;
    }
    if !reentrancy_enter() {
        return 0;
    }
    let seller = match read_address(seller_ptr) {
        Some(seller) => seller,
        None => {
            reentrancy_exit();
            return 0;
        }
    };
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(nft_contract) => nft_contract,
        None => {
            reentrancy_exit();
            return 0;
        }
    };
    if get_caller() != seller {
        reentrancy_exit();
        return 200;
    }

    let key = create_auction_key(nft_contract, token_id);
    let data = match storage_get(&key) {
        Some(data) if valid_auction_record(&data, nft_contract, token_id) && data[144] == 1 => data,
        _ => {
            log_info("Active auction not found");
            reentrancy_exit();
            return 0;
        }
    };
    if data[..32] != seller.0 {
        log_info("Only seller can cancel");
        reentrancy_exit();
        return 0;
    }
    if bytes_to_u64(&data[88..96]) > 0 {
        log_info("Cannot cancel auction with bids");
        reentrancy_exit();
        return 0;
    }
    match auction_escrowed(nft_contract, token_id) {
        Some(true) => {
            if !transfer_nft_from_market(nft_contract, get_contract_address(), seller, token_id) {
                log_info("Auction NFT return failed; cancellation remains retryable");
                reentrancy_exit();
                return 0;
            }
        }
        Some(false) => {}
        None => {
            log_info("Auction custody state is malformed");
            reentrancy_exit();
            return 0;
        }
    }

    let mut updated = data;
    updated[144] = 0;
    storage_set(&key, &updated);
    storage_set(&auction_escrow_key(nft_contract, token_id), &[0u8]);
    storage_set(&auction_bid_custody_key(nft_contract, token_id), &[0u8]);
    log_info("Auction cancelled");
    reentrancy_exit();
    1
}

/// Move an active legacy auction NFT into marketplace custody before any new
/// bid or sale settlement. The seller must explicitly approve this contract.
/// Current marketplace fee and collection-authoritative royalty terms are
/// frozen at this boundary. An auction with a legacy bid cannot move until the
/// configured treasury has moved that exact bid into contract custody.
#[no_mangle]
pub extern "C" fn migrate_auction_escrow(
    seller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
) -> u32 {
    if !metrics_v3_manifest_sealed() {
        return 0;
    }
    if get_value() != 0 {
        return 0;
    }
    if !reentrancy_enter() {
        return 0;
    }
    if get_value() != 0 {
        reentrancy_exit();
        return 0;
    }
    let seller = match read_address(seller_ptr) {
        Some(seller) if seller == get_caller() => seller,
        _ => {
            reentrancy_exit();
            return 0;
        }
    };
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(contract) => contract,
        None => {
            reentrancy_exit();
            return 0;
        }
    };
    let key = create_auction_key(nft_contract, token_id);
    let mut data = match storage_get(&key) {
        Some(data)
            if valid_auction_record(&data, nft_contract, token_id)
                && data[144] == 1
                && data[..32] == seller.0 =>
        {
            data
        }
        _ => {
            reentrancy_exit();
            return 0;
        }
    };
    let highest_bid = bytes_to_u64(&data[88..96]);
    match auction_bid_custody_ready(nft_contract, token_id) {
        Some(true) => {}
        Some(false) if highest_bid == 0 => {}
        Some(false) => {
            log_info("Legacy auction bid must move into contract custody first");
            reentrancy_exit();
            return 0;
        }
        None => {
            reentrancy_exit();
            return 0;
        }
    }
    let fee_bps = match get_marketplace_fee() {
        Some(fee) => fee,
        None => {
            reentrancy_exit();
            return 0;
        }
    };
    let fee_key = auction_fee_key(nft_contract, token_id);
    match storage_get(&fee_key) {
        None => {}
        Some(existing) if existing.len() == 8 && bytes_to_u64(&existing) == fee_bps => {}
        Some(_) => {
            reentrancy_exit();
            return 0;
        }
    }
    let (royalty_recipient, royalty_bps) =
        match canonical_collection_royalty(nft_contract, token_id) {
            Some(terms) => terms,
            None => {
                reentrancy_exit();
                return 0;
            }
        };
    data[177..209].copy_from_slice(&royalty_recipient.0);
    data[209..211].copy_from_slice(&royalty_bps.to_le_bytes());

    let marketplace = get_contract_address();
    let extension_key = auction_extension_key(nft_contract, token_id);
    let extension_count = match load_u64_or_zero(&extension_key) {
        Some(count) if count <= MAX_AUCTION_EXTENSIONS => count,
        _ => {
            reentrancy_exit();
            return 0;
        }
    };
    let already_escrowed = match auction_escrowed(nft_contract, token_id) {
        Some(true) if nft_owned_by(nft_contract, token_id, marketplace) => true,
        Some(true) | None => {
            reentrancy_exit();
            return 0;
        }
        Some(false) => nft_owned_by(nft_contract, token_id, marketplace),
    };
    if !already_escrowed && !transfer_nft_from_market(nft_contract, seller, marketplace, token_id) {
        reentrancy_exit();
        return 0;
    }
    storage_set(&key, &data);
    storage_set(&fee_key, &u64_to_bytes(fee_bps));
    storage_set(&auction_escrow_key(nft_contract, token_id), &[1u8]);
    storage_set(&auction_bid_custody_key(nft_contract, token_id), &[1u8]);
    storage_set(&extension_key, &u64_to_bytes(extension_count));
    cache_collection_royalty(nft_contract, royalty_recipient, royalty_bps);
    log_info("Legacy auction custody and settlement terms migrated");
    reentrancy_exit();
    1
}

/// Reconcile one exact pre-V3 highest bid into contract custody. Native LICN
/// was already delivered to this contract by the runtime and is balance-
/// certified; MT-20 custody is pulled from the configured legacy treasury.
/// This must complete before the seller can migrate the NFT.
#[no_mangle]
pub extern "C" fn migrate_v3_auction_bid_custody(
    treasury_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
) -> u32 {
    if !reentrancy_enter() {
        return 0;
    }
    let treasury = match read_address(treasury_ptr) {
        Some(address) if address == get_caller() => address,
        _ => {
            reentrancy_exit();
            return 0;
        }
    };
    match storage_get(MARKETPLACE_FEE_TREASURY_KEY) {
        Some(configured) if configured.len() == 32 && configured.as_slice() == treasury.0 => {}
        _ => {
            reentrancy_exit();
            return 0;
        }
    }
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) if address.0 != [0u8; 32] => address,
        _ => {
            reentrancy_exit();
            return 0;
        }
    };
    let data = match storage_get(&create_auction_key(nft_contract, token_id)) {
        Some(data) if valid_auction_record(&data, nft_contract, token_id) && data[144] == 1 => data,
        _ => {
            reentrancy_exit();
            return 0;
        }
    };
    let highest_bid = bytes_to_u64(&data[88..96]);
    if highest_bid == 0 {
        reentrancy_exit();
        return 0;
    }
    let custody_key = auction_bid_custody_key(nft_contract, token_id);
    match load_exact_bool(&custody_key, false) {
        Some(true) => {
            let idempotent = get_value() == 0;
            reentrancy_exit();
            return u32::from(idempotent);
        }
        Some(false) => {}
        None => {
            reentrancy_exit();
            return 0;
        }
    }
    let mut payment = [0u8; 32];
    payment.copy_from_slice(&data[145..177]);
    let payment_token = Address(payment);
    if get_value() != 0 {
        reentrancy_exit();
        return 0;
    }
    let (next_custody_row, next_native_custody) = if is_native_token(&payment_token) {
        match prepare_legacy_native_custody(highest_bid) {
            Some((row, native)) => (row, Some(native)),
            None => {
                reentrancy_exit();
                return 0;
            }
        }
    } else {
        let next_row = match next_metrics_v3_custody_row() {
            Some(row) => row,
            None => {
                reentrancy_exit();
                return 0;
            }
        };
        if !matches!(
            receive_token_or_native(payment_token, treasury, get_contract_address(), highest_bid,),
            Ok(true)
        ) {
            reentrancy_exit();
            return 0;
        }
        (next_row, None)
    };
    storage_set(&custody_key, &[1u8]);
    commit_legacy_custody_row(next_custody_row, next_native_custody);
    log_info("Legacy auction bid moved into marketplace custody");
    reentrancy_exit();
    1
}

/// Return whether an auction's NFT is held by this marketplace.
#[no_mangle]
pub extern "C" fn get_auction_custody(nft_contract_ptr: *const u8, token_id: u64) -> u32 {
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(contract) => contract,
        None => return 98,
    };
    match storage_get(&create_auction_key(nft_contract, token_id)) {
        Some(data) if valid_auction_record(&data, nft_contract, token_id) => {}
        _ => return 1,
    }
    let escrowed = match auction_escrowed(nft_contract, token_id) {
        Some(escrowed) => escrowed,
        None => return 2,
    };
    if escrowed && !nft_owned_by(nft_contract, token_id, get_contract_address()) {
        return 3;
    }
    lichen_sdk::set_return_data(&[u8::from(escrowed)]);
    0
}

/// Get auction details
#[no_mangle]
pub extern "C" fn get_auction(
    nft_contract_ptr: *const u8,
    token_id: u64,
    _out_ptr: *mut u8,
) -> u32 {
    unsafe {
        let nft_contract = parse_address(nft_contract_ptr);
        let key = create_auction_key(nft_contract, token_id);
        match storage_get(&key) {
            Some(data) if valid_auction_record(&data, nft_contract, token_id) => {
                lichen_sdk::set_return_data(&data);
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
///   105..113 expiry slot (u64 LE, 0 = no expiry)
const COLLECTION_OFFER_SIZE: usize = 113;

fn valid_collection_offer_record(data: &[u8], collection: Address, offerer: Address) -> bool {
    data.len() == COLLECTION_OFFER_SIZE
        && data[..32] == offerer.0
        && data[32..64] == collection.0
        && bytes_to_u64(&data[64..72]) > 0
        && data[104] <= 1
}

/// Make an offer on any NFT in a collection
#[no_mangle]
pub extern "C" fn make_collection_offer(
    offerer_ptr: *const u8,
    collection_ptr: *const u8,
    price: u64,
    payment_token_ptr: *const u8,
    expiry: u64,
) -> u32 {
    if is_mm_paused() {
        return 0;
    }
    if price == 0 {
        log_info("Price must be > 0");
        return 0;
    }
    if expiry > 0 && expiry <= lichen_sdk::get_slot() {
        log_info("Collection offer expiry must be in the future");
        return 0;
    }
    let offerer = match read_address(offerer_ptr) {
        Some(address) => address,
        None => return 0,
    };
    let collection = match read_address(collection_ptr) {
        Some(address) => address,
        None => return 0,
    };
    let payment_token = match read_address(payment_token_ptr) {
        Some(address) => address,
        None => return 0,
    };
    if get_caller() != offerer {
        return 200;
    }

    with_reentrancy_guard(|| {
        let (royalty_recipient, royalty_bps) = match canonical_collection_royalty(collection, 0) {
            Some(terms) => terms,
            None => {
                log_info("NFT collection royalty terms are unavailable or invalid");
                return 0;
            }
        };
        let fee_bps = match get_marketplace_fee() {
            Some(fee) => fee,
            None => return 0,
        };

        let mut key = b"col_offer:".to_vec();
        key.extend_from_slice(&collection.0);
        key.push(b':');
        key.extend_from_slice(&offerer.0);

        match storage_get(&key) {
            Some(existing) if !valid_collection_offer_record(&existing, collection, offerer) => {
                log_info("Existing collection offer state is malformed");
                return 0;
            }
            Some(existing) if existing[104] == 1 => {
                log_info("Cancel the active collection offer before replacing it");
                return 0;
            }
            _ => {}
        }
        if collection_offer_custody_ready(collection, offerer) != Some(false) {
            log_info("Collection-offer custody state is malformed");
            return 0;
        }
        if !receive_offer_custody(payment_token, offerer, price) {
            log_info("Collection-offer payment escrow failed");
            return 0;
        }

        let mut data = alloc::vec![0u8; COLLECTION_OFFER_SIZE];
        data[0..32].copy_from_slice(&offerer.0);
        data[32..64].copy_from_slice(&collection.0);
        data[64..72].copy_from_slice(&price.to_le_bytes());
        data[72..104].copy_from_slice(&payment_token.0);
        data[104] = 1; // active
        data[105..113].copy_from_slice(&expiry.to_le_bytes());

        storage_set(&key, &data);
        storage_set(
            &collection_offer_fee_key(collection, offerer),
            &u64_to_bytes(fee_bps),
        );
        store_royalty_snapshot(
            &collection_offer_royalty_key(collection, offerer),
            royalty_recipient,
            royalty_bps,
        );
        storage_set(
            &collection_offer_custody_key(collection, offerer),
            &[1u8],
        );
        log_info("Funded collection offer placed");
        1
    })
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
        if seller == offerer {
            log_info("Seller cannot accept their own collection offer");
            reentrancy_exit();
            return 0;
        }

        // Verify seller owns this specific NFT
        if !nft_owned_by(nft_contract, token_id, seller) {
            log_info("Seller does not own NFT");
            reentrancy_exit();
            return 0;
        }

        // Load collection offer
        let mut key = b"col_offer:".to_vec();
        key.extend_from_slice(&nft_contract.0);
        key.push(b':');
        key.extend_from_slice(&offerer.0);

        let data = match storage_get(&key) {
            Some(d) if valid_collection_offer_record(&d, nft_contract, offerer) && d[104] == 1 => d,
            _ => {
                log_info("Active collection offer not found");
                reentrancy_exit();
                return 0;
            }
        };
        if collection_offer_custody_ready(nft_contract, offerer) != Some(true) {
            log_info("Collection-offer payment custody is not ready");
            reentrancy_exit();
            return 0;
        }
        if get_value() != 0 {
            log_info("Collection-offer acceptance must not attach native value");
            reentrancy_exit();
            return 0;
        }

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

        let marketplace_addr = get_contract_address();

        // Immutable fee and royalty terms captured when the offer was made.
        let fee = match snapshotted_fee_bps(&collection_offer_fee_key(nft_contract, offerer)) {
            Some(fee) => fee,
            None => {
                reentrancy_exit();
                return 0;
            }
        };
        let fee_amount = ((price as u128) * (fee as u128) / 10000) as u64;
        let (royalty_recipient, royalty_bps) =
            match load_royalty_snapshot(&collection_offer_royalty_key(nft_contract, offerer)) {
                Some(terms) => terms,
                None => {
                    reentrancy_exit();
                    return 0;
                }
            };
        let royalty_amount = ((price as u128) * (royalty_bps as u128) / 10_000) as u64;
        let deductions = match fee_amount.checked_add(royalty_amount) {
            Some(amount) => amount,
            None => {
                reentrancy_exit();
                return 0;
            }
        };
        let seller_amount = match price.checked_sub(deductions) {
            Some(amount) => amount,
            None => {
                reentrancy_exit();
                return 0;
            }
        };
        if !can_record_unpaid_payouts(
            payment_token,
            &[(seller, seller_amount), (royalty_recipient, royalty_amount)],
        ) {
            reentrancy_exit();
            return 0;
        }
        let sale_accounting = match prepare_sale_accounting(payment_token, fee_amount, price) {
            Some(next) => next,
            None => {
                reentrancy_exit();
                return 0;
            }
        };

        // Transfer NFT through the marketplace's explicit token approval.
        if transfer_nft_from_market(nft_contract, seller, offerer, token_id) {
            // Release seller proceeds from escrow.
            match transfer_token_or_native(payment_token, marketplace_addr, seller, seller_amount) {
                Ok(true) => {}
                _ => {
                    if !record_unpaid_payout(payment_token, seller, seller_amount) {
                        reentrancy_exit();
                        return 0;
                    }
                    log_info("Collection-offer seller payment failed; payout recorded");
                }
            }
            if royalty_amount > 0 {
                match transfer_token_or_native(
                    payment_token,
                    marketplace_addr,
                    royalty_recipient,
                    royalty_amount,
                ) {
                    Ok(true) => {}
                    _ => {
                        if !record_unpaid_payout(payment_token, royalty_recipient, royalty_amount) {
                            reentrancy_exit();
                            return 0;
                        }
                    }
                }
            }
            commit_sale_accounting(payment_token, sale_accounting);

            let mut updated = data;
            updated[104] = 0;
            storage_set(&key, &updated);
            storage_set(
                &collection_offer_custody_key(nft_contract, offerer),
                &[0u8],
            );

            log_info("Collection offer accepted");
            reentrancy_exit();
            1
        } else {
            // Keep the offer active and funded; only the offerer can cancel
            // and reclaim custody after an NFT transfer failure.
            log_info("Collection-offer NFT transfer failed; funded offer remains active");
            reentrancy_exit();
            0
        }
    }
}

/// Cancel a collection offer
#[no_mangle]
pub extern "C" fn cancel_collection_offer(
    offerer_ptr: *const u8,
    collection_ptr: *const u8,
) -> u32 {
    if metrics_v3_migration_active() {
        return 0;
    }
    let offerer = match read_address(offerer_ptr) {
        Some(address) => address,
        None => return 0,
    };
    let collection = match read_address(collection_ptr) {
        Some(address) => address,
        None => return 0,
    };
    if get_caller() != offerer {
        return 200;
    }
    if get_value() != 0 {
        return 0;
    }

    with_reentrancy_guard(|| {
        let mut key = b"col_offer:".to_vec();
        key.extend_from_slice(&collection.0);
        key.push(b':');
        key.extend_from_slice(&offerer.0);

        let data = match storage_get(&key) {
            Some(d) if valid_collection_offer_record(&d, collection, offerer) && d[104] == 1 => d,
            _ => return 0,
        };
        let custody = match collection_offer_custody_ready(collection, offerer) {
            Some(value) => value,
            None => return 0,
        };
        if !custody && metrics_v3_manifest().is_some() {
            log_info("Legacy collection offer must complete custody migration");
            return 0;
        }
        if custody {
            let price = bytes_to_u64(&data[64..72]);
            let mut token = [0u8; 32];
            token.copy_from_slice(&data[72..104]);
            if !release_offer_custody(Address(token), offerer, price) {
                log_info("Collection-offer refund failed");
                return 0;
            }
        }

        let mut updated = data;
        updated[104] = 0;
        storage_set(&key, &updated);
        storage_set(
            &collection_offer_custody_key(collection, offerer),
            &[0u8],
        );
        log_info("Collection offer cancelled and custody released");
        1
    })
}

/// Claim a recorded marketplace payout that could not be transferred earlier.
/// This is intentionally available while the marketplace is paused so users can
/// exit after account or asset restrictions are lifted.
#[no_mangle]
pub extern "C" fn claim_unpaid_payout(caller_ptr: *const u8, token_ptr: *const u8) -> u32 {
    if !metrics_v3_ready() {
        return 0;
    }
    if get_value() != 0 {
        return 0;
    }
    if !reentrancy_enter() {
        return 0;
    }

    let caller = match read_address(caller_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 0;
        }
    };
    let token = match read_address(token_ptr) {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 0;
        }
    };

    let real_caller = get_caller();
    if real_caller.0 != caller.0 {
        reentrancy_exit();
        return 200;
    }

    let key = unpaid_payout_key(token, caller);
    let amount = match load_u64_or_zero(&key) {
        Some(amount) => amount,
        None => {
            reentrancy_exit();
            return 0;
        }
    };
    if amount == 0 {
        reentrancy_exit();
        return 0;
    }
    if load_exact_bool(&unpaid_payout_custody_key(token, caller), false) != Some(true) {
        log_info("Unpaid payout custody requires V3 migration");
        reentrancy_exit();
        return 0;
    }

    let marketplace_addr = match marketplace_escrow_address() {
        Some(addr) => addr,
        None => {
            reentrancy_exit();
            return 0;
        }
    };
    if marketplace_addr.0 == [0u8; 32] && !is_native_token(&token) {
        reentrancy_exit();
        return 0;
    }

    storage_set(&key, &u64_to_bytes(0));
    match transfer_token_or_native(token, marketplace_addr, caller, amount) {
        Ok(true) => {
            lichen_sdk::set_return_data(&u64_to_bytes(amount));
            reentrancy_exit();
            1
        }
        _ => {
            storage_set(&key, &u64_to_bytes(amount));
            reentrancy_exit();
            0
        }
    }
}

/// Query a recoverable marketplace payout.
#[no_mangle]
pub extern "C" fn get_unpaid_payout(token_ptr: *const u8, recipient_ptr: *const u8) -> u32 {
    let token = match read_address(token_ptr) {
        Some(addr) => addr,
        None => return 0,
    };
    let recipient = match read_address(recipient_ptr) {
        Some(addr) => addr,
        None => return 0,
    };

    match load_u64_or_zero(&unpaid_payout_key(token, recipient)) {
        Some(amount) => {
            lichen_sdk::set_return_data(&u64_to_bytes(amount));
            1
        }
        None => 0,
    }
}

/// Reconcile one exact pre-V3 unpaid-payout liability into contract custody.
/// Native LICN was already delivered to this contract by the runtime and is
/// balance-certified; MT-20 custody is pulled from the configured legacy fee
/// treasury. The treasury authorizes either path and retries are idempotent.
#[no_mangle]
pub extern "C" fn migrate_v3_unpaid_payout_custody(
    treasury_ptr: *const u8,
    token_ptr: *const u8,
    recipient_ptr: *const u8,
) -> u32 {
    if !reentrancy_enter() {
        return 0;
    }
    let treasury = match read_address(treasury_ptr) {
        Some(address) if address == get_caller() => address,
        _ => {
            reentrancy_exit();
            return 0;
        }
    };
    match storage_get(MARKETPLACE_FEE_TREASURY_KEY) {
        Some(configured) if configured.len() == 32 && configured.as_slice() == treasury.0 => {}
        _ => {
            reentrancy_exit();
            return 0;
        }
    }
    let token = match read_address(token_ptr) {
        Some(address) => address,
        None => {
            reentrancy_exit();
            return 0;
        }
    };
    let recipient = match read_address(recipient_ptr) {
        Some(address) if address.0 != [0u8; 32] => address,
        _ => {
            reentrancy_exit();
            return 0;
        }
    };
    let amount = match load_u64_or_zero(&unpaid_payout_key(token, recipient)) {
        Some(amount) if amount > 0 => amount,
        _ => {
            reentrancy_exit();
            return 0;
        }
    };
    let custody_key = unpaid_payout_custody_key(token, recipient);
    match load_exact_bool(&custody_key, false) {
        Some(true) => {
            let idempotent = get_value() == 0;
            reentrancy_exit();
            return u32::from(idempotent);
        }
        Some(false) => {}
        None => {
            reentrancy_exit();
            return 0;
        }
    }
    if get_value() != 0 {
        reentrancy_exit();
        return 0;
    }
    let contract = get_contract_address();
    let (next_custody_row, next_native_custody) = if is_native_token(&token) {
        match prepare_legacy_native_custody(amount) {
            Some((row, native)) => (row, Some(native)),
            None => {
                reentrancy_exit();
                return 0;
            }
        }
    } else {
        let next_row = match next_metrics_v3_custody_row() {
            Some(row) => row,
            None => {
                reentrancy_exit();
                return 0;
            }
        };
        if !matches!(
            receive_token_or_native(token, treasury, contract, amount),
            Ok(true)
        ) {
            reentrancy_exit();
            return 0;
        }
        (next_row, None)
    };
    storage_set(&custody_key, &[1u8]);
    commit_legacy_custody_row(next_custody_row, next_native_custody);
    reentrancy_exit();
    1
}

/// Return the listing record followed by its immutable fee basis points.
#[no_mangle]
pub extern "C" fn get_listing_terms(nft_contract_ptr: *const u8, token_id: u64) -> u32 {
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let mut data = match storage_get(&create_listing_key(nft_contract, token_id)) {
        Some(data) if valid_listing_record(&data, nft_contract, token_id) => data,
        _ => return 1,
    };
    let fee = match snapshotted_fee_bps(&listing_fee_key(nft_contract, token_id)) {
        Some(fee) => fee,
        None => return 2,
    };
    data.extend_from_slice(&u64_to_bytes(fee));
    lichen_sdk::set_return_data(&data);
    0
}

/// Freeze canonical fee and royalty terms for one active pre-V3 listing.
/// The original seller, asset, token, price, and active state are preserved;
/// only the untrusted legacy royalty fields are replaced at this boundary.
#[no_mangle]
pub extern "C" fn migrate_v3_listing(
    caller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    fee_bps: u64,
    royalty_recipient_ptr: *const u8,
    royalty_bps: u64,
) -> u32 {
    if !metrics_v3_manifest_sealed() {
        return 8;
    }
    if get_value() != 0 {
        return 8;
    }
    match read_address(caller_ptr) {
        Some(address) if address == get_caller() && is_mm_admin(&address.0) => {}
        _ => return 7,
    }
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) if address.0 != [0u8; 32] => address,
        _ => return 2,
    };
    let royalty_recipient = match read_address(royalty_recipient_ptr) {
        Some(address) => address,
        None => return 2,
    };
    let royalty_bps = match u16::try_from(royalty_bps) {
        Ok(value) if value <= 1_000 => value,
        _ => return 2,
    };
    if royalty_bps > 0 && royalty_recipient.0 == [0u8; 32] {
        return 2;
    }
    if get_marketplace_fee() != Some(fee_bps)
        || canonical_collection_royalty(nft_contract, token_id)
            != Some((royalty_recipient, royalty_bps))
    {
        return 3;
    }

    let listing_key = create_listing_key(nft_contract, token_id);
    let mut listing = match storage_get(&listing_key) {
        Some(data)
            if structurally_valid_listing_record(&data, nft_contract, token_id)
                && data[144] == 1 =>
        {
            data
        }
        _ => return 4,
    };
    let fee_key = listing_fee_key(nft_contract, token_id);
    match storage_get(&fee_key) {
        None => {}
        Some(data) if data.len() == 8 && bytes_to_u64(&data) == fee_bps => {}
        Some(_) => return 6,
    }
    let slot_key = listing_slot_key(nft_contract, token_id);
    let migration_slot = match storage_get(&slot_key) {
        None => Some(lichen_sdk::get_slot()),
        Some(data) if data.len() == 8 => None,
        Some(_) => return 6,
    };
    listing[112..144].copy_from_slice(&royalty_recipient.0);
    listing[145..147].copy_from_slice(&royalty_bps.to_le_bytes());
    storage_set(&listing_key, &listing);
    storage_set(&fee_key, &u64_to_bytes(fee_bps));
    if let Some(slot) = migration_slot {
        storage_set(&slot_key, &u64_to_bytes(slot));
    }
    0
}

/// Return an offer followed by immutable fee bps, royalty recipient, and
/// royalty bps. Both legacy 73-byte and expiry-aware 81-byte offers are valid.
#[no_mangle]
pub extern "C" fn get_offer(
    nft_contract_ptr: *const u8,
    token_id: u64,
    offerer_ptr: *const u8,
) -> u32 {
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let offerer = match read_address(offerer_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let mut key = b"offer:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.push(b':');
    key.extend_from_slice(&token_id.to_le_bytes());
    key.push(b':');
    key.extend_from_slice(&offerer.0);
    let mut data = match storage_get(&key) {
        Some(data) if valid_offer_record(&data, &offerer.0) => data,
        _ => return 1,
    };
    let fee = match snapshotted_fee_bps(&offer_fee_key(nft_contract, token_id, &offerer.0)) {
        Some(fee) => fee,
        None => return 2,
    };
    data.extend_from_slice(&u64_to_bytes(fee));
    let (royalty_recipient, royalty_bps) =
        match load_royalty_snapshot(&offer_royalty_key(nft_contract, token_id, &offerer.0)) {
            Some(terms) => terms,
            None => return 2,
        };
    data.extend_from_slice(&royalty_recipient.0);
    data.extend_from_slice(&u64_to_bytes(u64::from(royalty_bps)));
    lichen_sdk::set_return_data(&data);
    0
}

/// Return whether an offer's full payment is held by this marketplace.
#[no_mangle]
pub extern "C" fn get_offer_custody(
    nft_contract_ptr: *const u8,
    token_id: u64,
    offerer_ptr: *const u8,
) -> u32 {
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let offerer = match read_address(offerer_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let custody = match offer_custody_ready(nft_contract, token_id, &offerer.0) {
        Some(value) => value,
        None => return 2,
    };
    lichen_sdk::set_return_data(&[u8::from(custody)]);
    0
}

/// Index one active pre-V3 offer and freeze its settlement terms. Legacy
/// wallet counts are verified in place; only the new per-NFT count is added.
#[no_mangle]
pub extern "C" fn migrate_v3_offer(
    caller_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    offerer_ptr: *const u8,
    fee_bps: u64,
    royalty_recipient_ptr: *const u8,
    royalty_bps: u64,
) -> u32 {
    if !metrics_v3_manifest_sealed() {
        return 8;
    }
    if get_value() != 0 {
        return 8;
    }
    let caller = match read_address(caller_ptr) {
        Some(address) if address == get_caller() && is_mm_admin(&address.0) => address,
        _ => return 7,
    };
    let _ = caller;
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) if address.0 != [0u8; 32] => address,
        _ => return 2,
    };
    let offerer = match read_address(offerer_ptr) {
        Some(address) if address.0 != [0u8; 32] => address,
        _ => return 2,
    };
    let royalty_recipient = match read_address(royalty_recipient_ptr) {
        Some(address) => address,
        None => return 2,
    };
    let royalty_bps = match u16::try_from(royalty_bps) {
        Ok(value) if value <= 1_000 => value,
        _ => return 2,
    };
    if royalty_bps > 0 && royalty_recipient.0 == [0u8; 32] {
        return 2;
    }
    if get_marketplace_fee() != Some(fee_bps)
        || canonical_collection_royalty(nft_contract, token_id)
            != Some((royalty_recipient, royalty_bps))
    {
        return 3;
    }

    let mut key = b"offer:".to_vec();
    key.extend_from_slice(&nft_contract.0);
    key.push(b':');
    key.extend_from_slice(&token_id.to_le_bytes());
    key.push(b':');
    key.extend_from_slice(&offerer.0);
    match storage_get(&key) {
        Some(data) if valid_offer_record(&data, &offerer.0) && data[72] == 1 => {}
        _ => return 4,
    }

    match offer_is_indexed(nft_contract, token_id, &offerer.0) {
        Some(true) => return 0,
        Some(false) => {}
        None => return 5,
    }
    let wallet_count = match load_offerer_active_count(&offerer.0) {
        Some(count) if (1..=MAX_ACTIVE_OFFERS_PER_WALLET).contains(&count) => count,
        _ => return 5,
    };
    let next_nft_count = match load_active_offer_count(nft_contract, token_id)
        .and_then(|count| count.checked_add(1))
    {
        Some(count) => count,
        None => return 5,
    };

    let fee_key = offer_fee_key(nft_contract, token_id, &offerer.0);
    match storage_get(&fee_key) {
        None => {}
        Some(data) if data.len() == 8 && bytes_to_u64(&data) == fee_bps => {}
        Some(_) => return 6,
    }
    let royalty_key = offer_royalty_key(nft_contract, token_id, &offerer.0);
    match storage_get(&royalty_key) {
        None => {}
        Some(_)
            if load_royalty_snapshot(&royalty_key) == Some((royalty_recipient, royalty_bps)) => {}
        Some(_) => return 6,
    }

    storage_set(&fee_key, &u64_to_bytes(fee_bps));
    store_royalty_snapshot(&royalty_key, royalty_recipient, royalty_bps);
    commit_offer_counts(
        nft_contract,
        token_id,
        &offerer.0,
        wallet_count,
        next_nft_count,
    );
    set_offer_indexed(nft_contract, token_id, &offerer.0, true);
    0
}

/// Move one active legacy offer's exact payment into marketplace custody.
/// The offerer funds MT-20 custody through transfer_from. Native LICN may be
/// certified from historical attached value or supplied exactly once here.
#[no_mangle]
pub extern "C" fn migrate_v3_offer_custody(
    offerer_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
) -> u32 {
    let offerer = match read_address(offerer_ptr) {
        Some(address) if address == get_caller() => address,
        _ => return 7,
    };
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) if address.0 != [0u8; 32] => address,
        _ => return 2,
    };

    with_reentrancy_guard(|| {
        let mut key = b"offer:".to_vec();
        key.extend_from_slice(&nft_contract.0);
        key.push(b':');
        key.extend_from_slice(&token_id.to_le_bytes());
        key.push(b':');
        key.extend_from_slice(&offerer.0);
        let data = match storage_get(&key) {
            Some(data) if valid_offer_record(&data, &offerer.0) && data[72] == 1 => data,
            _ => return 4,
        };
        if offer_is_indexed(nft_contract, token_id, &offerer.0) != Some(true)
            || snapshotted_fee_bps(&offer_fee_key(nft_contract, token_id, &offerer.0)).is_none()
            || load_royalty_snapshot(&offer_royalty_key(nft_contract, token_id, &offerer.0))
                .is_none()
        {
            return 5;
        }
        let custody_key = offer_custody_key(nft_contract, token_id, &offerer.0);
        match load_exact_bool(&custody_key, false) {
            Some(true) => return u32::from(get_value() == 0),
            Some(false) => {}
            None => return 6,
        }
        let amount = bytes_to_u64(&data[32..40]);
        let mut token = [0u8; 32];
        token.copy_from_slice(&data[40..72]);
        let (row, native) = match prepare_legacy_offer_custody(Address(token), offerer, amount) {
            Some(prepared) => prepared,
            None => return 6,
        };
        storage_set(&custody_key, &[1u8]);
        commit_legacy_custody_row(row, native);
        log_info("Legacy offer payment moved into marketplace custody");
        1
    })
}

/// Return an auction record followed by its immutable fee basis points.
#[no_mangle]
pub extern "C" fn get_auction_terms(nft_contract_ptr: *const u8, token_id: u64) -> u32 {
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let mut data = match storage_get(&create_auction_key(nft_contract, token_id)) {
        Some(data) if valid_auction_record(&data, nft_contract, token_id) => data,
        _ => return 1,
    };
    let fee = match snapshotted_fee_bps(&auction_fee_key(nft_contract, token_id)) {
        Some(fee) => fee,
        None => return 2,
    };
    data.extend_from_slice(&u64_to_bytes(fee));
    lichen_sdk::set_return_data(&data);
    0
}

/// Return a collection offer followed by immutable fee bps, royalty recipient,
/// and royalty bps.
#[no_mangle]
pub extern "C" fn get_collection_offer(collection_ptr: *const u8, offerer_ptr: *const u8) -> u32 {
    let collection = match read_address(collection_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let offerer = match read_address(offerer_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let mut key = b"col_offer:".to_vec();
    key.extend_from_slice(&collection.0);
    key.push(b':');
    key.extend_from_slice(&offerer.0);
    let mut data = match storage_get(&key) {
        Some(data) if valid_collection_offer_record(&data, collection, offerer) => data,
        _ => return 1,
    };
    let fee = match snapshotted_fee_bps(&collection_offer_fee_key(collection, offerer)) {
        Some(fee) => fee,
        None => return 2,
    };
    data.extend_from_slice(&u64_to_bytes(fee));
    let (royalty_recipient, royalty_bps) =
        match load_royalty_snapshot(&collection_offer_royalty_key(collection, offerer)) {
            Some(terms) => terms,
            None => return 2,
        };
    data.extend_from_slice(&royalty_recipient.0);
    data.extend_from_slice(&u64_to_bytes(u64::from(royalty_bps)));
    lichen_sdk::set_return_data(&data);
    0
}

/// Return whether a collection offer's full payment is held in custody.
#[no_mangle]
pub extern "C" fn get_collection_offer_custody(
    collection_ptr: *const u8,
    offerer_ptr: *const u8,
) -> u32 {
    let collection = match read_address(collection_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let offerer = match read_address(offerer_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let custody = match collection_offer_custody_ready(collection, offerer) {
        Some(value) => value,
        None => return 2,
    };
    lichen_sdk::set_return_data(&[u8::from(custody)]);
    0
}

/// Freeze the settlement terms for one active pre-V3 collection offer.
/// Existing exact snapshots make this operation idempotent; conflicting
/// snapshots fail closed and are never overwritten.
#[no_mangle]
pub extern "C" fn migrate_v3_collection_offer(
    caller_ptr: *const u8,
    collection_ptr: *const u8,
    offerer_ptr: *const u8,
    fee_bps: u64,
    royalty_recipient_ptr: *const u8,
    royalty_bps: u64,
) -> u32 {
    if !metrics_v3_manifest_sealed() {
        return 8;
    }
    if get_value() != 0 {
        return 8;
    }
    match read_address(caller_ptr) {
        Some(address) if address == get_caller() && is_mm_admin(&address.0) => {}
        _ => return 7,
    }
    let collection = match read_address(collection_ptr) {
        Some(address) if address.0 != [0u8; 32] => address,
        _ => return 2,
    };
    let offerer = match read_address(offerer_ptr) {
        Some(address) if address.0 != [0u8; 32] => address,
        _ => return 2,
    };
    let royalty_recipient = match read_address(royalty_recipient_ptr) {
        Some(address) => address,
        None => return 2,
    };
    let royalty_bps = match u16::try_from(royalty_bps) {
        Ok(value) if value <= 1_000 => value,
        _ => return 2,
    };
    if royalty_bps > 0 && royalty_recipient.0 == [0u8; 32] {
        return 2;
    }
    if get_marketplace_fee() != Some(fee_bps)
        || canonical_collection_royalty(collection, 0) != Some((royalty_recipient, royalty_bps))
    {
        return 3;
    }

    let mut key = b"col_offer:".to_vec();
    key.extend_from_slice(&collection.0);
    key.push(b':');
    key.extend_from_slice(&offerer.0);
    match storage_get(&key) {
        Some(data)
            if valid_collection_offer_record(&data, collection, offerer) && data[104] == 1 => {}
        _ => return 4,
    }

    let fee_key = collection_offer_fee_key(collection, offerer);
    match storage_get(&fee_key) {
        None => {}
        Some(data) if data.len() == 8 && bytes_to_u64(&data) == fee_bps => {}
        Some(_) => return 6,
    }
    let royalty_key = collection_offer_royalty_key(collection, offerer);
    match storage_get(&royalty_key) {
        None => {}
        Some(_)
            if load_royalty_snapshot(&royalty_key) == Some((royalty_recipient, royalty_bps)) => {}
        Some(_) => return 6,
    }

    storage_set(&fee_key, &u64_to_bytes(fee_bps));
    store_royalty_snapshot(&royalty_key, royalty_recipient, royalty_bps);
    0
}

/// Move one active legacy collection offer's exact payment into custody.
#[no_mangle]
pub extern "C" fn migrate_v3_collection_offer_custody(
    offerer_ptr: *const u8,
    collection_ptr: *const u8,
) -> u32 {
    let offerer = match read_address(offerer_ptr) {
        Some(address) if address == get_caller() => address,
        _ => return 7,
    };
    let collection = match read_address(collection_ptr) {
        Some(address) if address.0 != [0u8; 32] => address,
        _ => return 2,
    };

    with_reentrancy_guard(|| {
        let mut key = b"col_offer:".to_vec();
        key.extend_from_slice(&collection.0);
        key.push(b':');
        key.extend_from_slice(&offerer.0);
        let data = match storage_get(&key) {
            Some(data)
                if valid_collection_offer_record(&data, collection, offerer)
                    && data[104] == 1 =>
            {
                data
            }
            _ => return 4,
        };
        if snapshotted_fee_bps(&collection_offer_fee_key(collection, offerer)).is_none()
            || load_royalty_snapshot(&collection_offer_royalty_key(collection, offerer)).is_none()
        {
            return 5;
        }
        let custody_key = collection_offer_custody_key(collection, offerer);
        match load_exact_bool(&custody_key, false) {
            Some(true) => return u32::from(get_value() == 0),
            Some(false) => {}
            None => return 6,
        }
        let amount = bytes_to_u64(&data[64..72]);
        let mut token = [0u8; 32];
        token.copy_from_slice(&data[72..104]);
        let (row, native) = match prepare_legacy_offer_custody(Address(token), offerer, amount) {
            Some(prepared) => prepared,
            None => return 6,
        };
        storage_set(&custody_key, &[1u8]);
        commit_legacy_custody_row(row, native);
        log_info("Legacy collection-offer payment moved into custody");
        1
    })
}

// ============================================================================
// OFFER EXPIRY
// ============================================================================

/// Make an offer with optional absolute slot expiry.
/// Offer layout (with expiry): [offerer(32), price(8), payment_token(32), active(1), expiry_slot(8)] = 81 bytes
#[no_mangle]
pub extern "C" fn make_offer_with_expiry(
    offerer_ptr: *const u8,
    nft_contract_ptr: *const u8,
    token_id: u64,
    price: u64,
    payment_token_ptr: *const u8,
    expiry: u64,
) -> u32 {
    let offerer = match read_address(offerer_ptr) {
        Some(address) => address,
        None => return 0,
    };
    let nft_contract = match read_address(nft_contract_ptr) {
        Some(address) => address,
        None => return 0,
    };
    let payment_token = match read_address(payment_token_ptr) {
        Some(address) => address,
        None => return 0,
    };
    create_funded_offer(
        offerer,
        nft_contract,
        token_id,
        price,
        payment_token,
        Some(expiry),
    )
}

// ============================================================================
// V3 METRICS MIGRATION
// ============================================================================

/// Freeze marketplace activity before importing source-derived historical
/// metrics. Legacy mixed `mm_sale_volume` evidence is never overwritten.
#[no_mangle]
pub extern "C" fn begin_metrics_v3_migration(caller_ptr: *const u8) -> u32 {
    if get_value() != 0 {
        return 5;
    }
    let caller = match read_address(caller_ptr) {
        Some(address) if address == get_caller() && is_mm_admin(&address.0) => address,
        _ => return 1,
    };
    let _ = caller;
    if metrics_v3_version() != Some(0) {
        return 2;
    }
    match metrics_v3_migration_locked() {
        Some(true) => return 0,
        Some(false) => {}
        None => return 3,
    }
    if storage_get(MM_METRICS_MIGRATION_MANIFEST_KEY).is_some() {
        return 3;
    }
    storage_set(MM_PAUSE_KEY, &[1u8]);
    storage_set(MM_METRICS_MIGRATION_LOCK_KEY, &[1u8]);
    storage_set(MM_METRICS_MIGRATION_EXPECTED_ROWS_KEY, &u64_to_bytes(0));
    storage_set(MM_METRICS_MIGRATION_ROWS_KEY, &u64_to_bytes(0));
    storage_set(MM_METRICS_MIGRATION_EXPECTED_SALES_KEY, &u64_to_bytes(0));
    storage_set(MM_METRICS_MIGRATION_SALES_KEY, &u64_to_bytes(0));
    storage_set(MM_METRICS_MIGRATION_NATIVE_VOLUME_KEY, &u64_to_bytes(0));
    storage_set(
        MM_METRICS_MIGRATION_EXPECTED_CUSTODY_ROWS_KEY,
        &u64_to_bytes(0),
    );
    storage_set(MM_METRICS_MIGRATION_CUSTODY_ROWS_KEY, &u64_to_bytes(0));
    storage_set(
        MM_METRICS_MIGRATION_EXPECTED_NATIVE_CUSTODY_KEY,
        &u64_to_bytes(0),
    );
    storage_set(MM_METRICS_MIGRATION_NATIVE_CUSTODY_KEY, &u64_to_bytes(0));
    storage_set(MM_METRICS_MIGRATION_GLOBAL_KEY, &[0u8]);
    storage_set(MM_METRICS_MIGRATION_SAW_NATIVE_KEY, &[0u8]);
    0
}

/// Bind the exact off-chain history manifest and its independently derived
/// aggregate expectations before any counter is imported.
#[no_mangle]
pub extern "C" fn seal_metrics_v3_manifest(
    caller_ptr: *const u8,
    manifest_ptr: *const u8,
    expected_token_rows: u64,
    expected_sales: u64,
    native_sale_volume: u64,
    expected_custody_rows: u64,
    expected_native_custody: u64,
) -> u32 {
    if get_value() != 0 {
        return 5;
    }
    match read_address(caller_ptr) {
        Some(address) if address == get_caller() && is_mm_admin(&address.0) => {}
        _ => return 1,
    }
    let manifest = match read_address(manifest_ptr) {
        Some(value) if value.0 != [0u8; 32] => value,
        _ => return 2,
    };
    if metrics_v3_version() != Some(0)
        || metrics_v3_migration_locked() != Some(true)
        || metrics_v3_manifest().is_some()
    {
        return 3;
    }
    if expected_token_rows > MAX_METRICS_MIGRATION_ROWS
        || expected_custody_rows > MAX_METRICS_MIGRATION_ROWS
        || (expected_sales == 0 && (expected_token_rows != 0 || native_sale_volume != 0))
        || (expected_sales > 0 && expected_token_rows == 0)
        || (expected_custody_rows == 0 && expected_native_custody != 0)
    {
        return 4;
    }
    storage_set(MM_METRICS_MIGRATION_MANIFEST_KEY, &manifest.0);
    storage_set(
        MM_METRICS_MIGRATION_EXPECTED_ROWS_KEY,
        &u64_to_bytes(expected_token_rows),
    );
    storage_set(
        MM_METRICS_MIGRATION_EXPECTED_SALES_KEY,
        &u64_to_bytes(expected_sales),
    );
    storage_set(
        MM_METRICS_MIGRATION_NATIVE_VOLUME_KEY,
        &u64_to_bytes(native_sale_volume),
    );
    storage_set(
        MM_METRICS_MIGRATION_EXPECTED_CUSTODY_ROWS_KEY,
        &u64_to_bytes(expected_custody_rows),
    );
    storage_set(
        MM_METRICS_MIGRATION_EXPECTED_NATIVE_CUSTODY_KEY,
        &u64_to_bytes(expected_native_custody),
    );
    0
}

/// Initialize the exact global V3 counters from the sealed expectations.
#[no_mangle]
pub extern "C" fn migrate_metrics_v3_global(caller_ptr: *const u8) -> u32 {
    if get_value() != 0 {
        return 5;
    }
    match read_address(caller_ptr) {
        Some(address) if address == get_caller() && is_mm_admin(&address.0) => {}
        _ => return 1,
    }
    if metrics_v3_version() != Some(0)
        || metrics_v3_migration_locked() != Some(true)
        || metrics_v3_manifest().is_none()
    {
        return 2;
    }
    let expected_sales = match load_u64_or_zero(MM_METRICS_MIGRATION_EXPECTED_SALES_KEY) {
        Some(value) => value,
        None => return 3,
    };
    let native_volume = match load_u64_or_zero(MM_METRICS_MIGRATION_NATIVE_VOLUME_KEY) {
        Some(value) => value,
        None => return 3,
    };
    match load_exact_bool(MM_METRICS_MIGRATION_GLOBAL_KEY, false) {
        Some(true) => {
            return if load_u64_or_zero(MM_SALE_COUNT_KEY) == Some(expected_sales)
                && load_u64_or_zero(MM_NATIVE_SALE_VOLUME_KEY) == Some(native_volume)
            {
                0
            } else {
                4
            };
        }
        Some(false) => {}
        None => return 4,
    }
    storage_set(MM_SALE_COUNT_KEY, &u64_to_bytes(expected_sales));
    storage_set(MM_NATIVE_SALE_VOLUME_KEY, &u64_to_bytes(native_volume));
    storage_set(MM_METRICS_MIGRATION_GLOBAL_KEY, &[1u8]);
    0
}

/// Import one token's source-derived lifetime count, raw volume, and realized
/// fee total. Historical MT-20 fees remain non-withdrawable because they were
/// already delivered to the old external treasury. Historical native fees are
/// made withdrawable only after proving the contract balance covers every
/// declared native custody liability plus those exact fees.
#[no_mangle]
pub extern "C" fn migrate_metrics_v3_token(
    caller_ptr: *const u8,
    token_ptr: *const u8,
    sale_count: u64,
    sale_volume: u64,
    realized_fees: u64,
) -> u32 {
    if get_value() != 0 {
        return 6;
    }
    match read_address(caller_ptr) {
        Some(address) if address == get_caller() && is_mm_admin(&address.0) => {}
        _ => return 1,
    }
    let token = match read_address(token_ptr) {
        Some(token) => token,
        None => return 2,
    };
    if metrics_v3_version() != Some(0)
        || metrics_v3_migration_locked() != Some(true)
        || metrics_v3_manifest().is_none()
        || sale_count == 0
        || sale_volume == 0
        || realized_fees > sale_volume
    {
        return 3;
    }
    let marker = metrics_v3_token_marker_key(token);
    let native_token = is_native_token(&token);
    match load_exact_bool(&marker, false) {
        Some(true) => {
            return if load_u64_or_zero(&token_sale_count_key(token)) == Some(sale_count)
                && load_u64_or_zero(&token_sale_volume_key(token)) == Some(sale_volume)
                && load_u64_or_zero(&token_sale_fees_key(token)) == Some(realized_fees)
                && (!native_token
                    || load_u64_or_zero(&platform_fee_key(token)) == Some(realized_fees))
            {
                0
            } else {
                4
            };
        }
        Some(false) => {}
        None => return 4,
    }
    if load_u64_or_zero(&token_sale_count_key(token)) != Some(0)
        || load_u64_or_zero(&token_sale_volume_key(token)) != Some(0)
        || load_u64_or_zero(&token_sale_fees_key(token)) != Some(0)
    {
        return 4;
    }
    let expected_rows = match load_u64_or_zero(MM_METRICS_MIGRATION_EXPECTED_ROWS_KEY) {
        Some(value) => value,
        None => return 4,
    };
    let migrated_rows = match load_u64_or_zero(MM_METRICS_MIGRATION_ROWS_KEY) {
        Some(value) if value < expected_rows => value,
        _ => return 4,
    };
    let expected_sales = match load_u64_or_zero(MM_METRICS_MIGRATION_EXPECTED_SALES_KEY) {
        Some(value) => value,
        None => return 4,
    };
    let migrated_sales = match load_u64_or_zero(MM_METRICS_MIGRATION_SALES_KEY) {
        Some(value) => value,
        None => return 4,
    };
    let next_rows = match migrated_rows.checked_add(1) {
        Some(value) if value <= expected_rows => value,
        _ => return 4,
    };
    let next_sales = match migrated_sales.checked_add(sale_count) {
        Some(value) if value <= expected_sales => value,
        _ => return 4,
    };
    if native_token {
        let expected_native = match load_u64_or_zero(MM_METRICS_MIGRATION_NATIVE_VOLUME_KEY) {
            Some(value) if value == sale_volume => value,
            _ => return 5,
        };
        let _ = expected_native;
        if load_exact_bool(MM_METRICS_MIGRATION_SAW_NATIVE_KEY, false) != Some(false) {
            return 5;
        }
        let expected_native_custody =
            match load_u64_or_zero(MM_METRICS_MIGRATION_EXPECTED_NATIVE_CUSTODY_KEY) {
                Some(value) => value,
                None => return 5,
            };
        let required_native_balance = match expected_native_custody.checked_add(realized_fees) {
            Some(value) => value,
            None => return 5,
        };
        if load_u64_or_zero(&platform_fee_key(token)) != Some(0) {
            return 5;
        }
        match native_balance_of(get_contract_address()) {
            Ok(balance) if balance >= required_native_balance => {}
            _ => return 5,
        }
    }

    storage_set(&token_sale_count_key(token), &u64_to_bytes(sale_count));
    storage_set(&token_sale_volume_key(token), &u64_to_bytes(sale_volume));
    storage_set(&token_sale_fees_key(token), &u64_to_bytes(realized_fees));
    storage_set(&marker, &[1u8]);
    storage_set(MM_METRICS_MIGRATION_ROWS_KEY, &u64_to_bytes(next_rows));
    storage_set(MM_METRICS_MIGRATION_SALES_KEY, &u64_to_bytes(next_sales));
    if native_token {
        storage_set(&platform_fee_key(token), &u64_to_bytes(realized_fees));
        storage_set(MM_METRICS_MIGRATION_SAW_NATIVE_KEY, &[1u8]);
    }
    0
}

/// Activate V3 accounting only after the sealed token rows exactly cover the
/// historical sale count and native volume.
#[no_mangle]
pub extern "C" fn complete_metrics_v3_migration(caller_ptr: *const u8) -> u32 {
    if get_value() != 0 {
        return 4;
    }
    match read_address(caller_ptr) {
        Some(address) if address == get_caller() && is_mm_admin(&address.0) => {}
        _ => return 1,
    }
    if metrics_v3_version() != Some(0)
        || metrics_v3_migration_locked() != Some(true)
        || metrics_v3_manifest().is_none()
        || load_exact_bool(MM_METRICS_MIGRATION_GLOBAL_KEY, false) != Some(true)
    {
        return 2;
    }
    let expected_rows = load_u64_or_zero(MM_METRICS_MIGRATION_EXPECTED_ROWS_KEY);
    let migrated_rows = load_u64_or_zero(MM_METRICS_MIGRATION_ROWS_KEY);
    let expected_sales = load_u64_or_zero(MM_METRICS_MIGRATION_EXPECTED_SALES_KEY);
    let migrated_sales = load_u64_or_zero(MM_METRICS_MIGRATION_SALES_KEY);
    let native_volume = load_u64_or_zero(MM_METRICS_MIGRATION_NATIVE_VOLUME_KEY);
    let expected_custody_rows = load_u64_or_zero(MM_METRICS_MIGRATION_EXPECTED_CUSTODY_ROWS_KEY);
    let migrated_custody_rows = load_u64_or_zero(MM_METRICS_MIGRATION_CUSTODY_ROWS_KEY);
    let expected_native_custody =
        load_u64_or_zero(MM_METRICS_MIGRATION_EXPECTED_NATIVE_CUSTODY_KEY);
    let reserved_native_custody = load_u64_or_zero(MM_METRICS_MIGRATION_NATIVE_CUSTODY_KEY);
    let native_token = Address([0u8; 32]);
    let native_fees = load_u64_or_zero(&platform_fee_key(native_token));
    let required_native_balance = expected_native_custody
        .zip(native_fees)
        .and_then(|(custody, fees)| custody.checked_add(fees));
    if expected_rows.is_none()
        || expected_rows != migrated_rows
        || expected_sales.is_none()
        || expected_sales != migrated_sales
        || native_volume.is_none()
        || expected_custody_rows.is_none()
        || expected_custody_rows != migrated_custody_rows
        || expected_native_custody.is_none()
        || expected_native_custody != reserved_native_custody
        || required_native_balance.is_none()
        || (native_volume != Some(0)
            && load_exact_bool(MM_METRICS_MIGRATION_SAW_NATIVE_KEY, false) != Some(true))
    {
        return 3;
    }
    if let Some(required) = required_native_balance {
        if required > 0 {
            match native_balance_of(get_contract_address()) {
                Ok(balance) if balance >= required => {}
                _ => return 3,
            }
        }
    }
    storage_set(MM_METRICS_VERSION_KEY, &u64_to_bytes(MM_METRICS_VERSION));
    storage_set(MM_METRICS_MIGRATION_LOCK_KEY, &[0u8]);
    0
}

/// Return metrics migration state as version(8), lock/pause/sealed(1+1+1),
/// expected/migrated token rows(8+8), expected/migrated sales(8+8), native
/// volume(8), expected/migrated custody rows(8+8), expected/reserved native
/// custody(8+8), and manifest hash(32).
#[no_mangle]
pub extern "C" fn get_metrics_v3_migration_status() -> u32 {
    let version = match metrics_v3_version() {
        Some(value) => value,
        None => return 1,
    };
    let locked = match metrics_v3_migration_locked() {
        Some(value) => value,
        None => return 1,
    };
    let paused = match load_mm_pause_state() {
        Some(value) => value,
        None => return 1,
    };
    let expected_rows = match load_u64_or_zero(MM_METRICS_MIGRATION_EXPECTED_ROWS_KEY) {
        Some(value) => value,
        None => return 1,
    };
    let migrated_rows = match load_u64_or_zero(MM_METRICS_MIGRATION_ROWS_KEY) {
        Some(value) => value,
        None => return 1,
    };
    let expected_sales = match load_u64_or_zero(MM_METRICS_MIGRATION_EXPECTED_SALES_KEY) {
        Some(value) => value,
        None => return 1,
    };
    let migrated_sales = match load_u64_or_zero(MM_METRICS_MIGRATION_SALES_KEY) {
        Some(value) => value,
        None => return 1,
    };
    let native_volume = match load_u64_or_zero(MM_METRICS_MIGRATION_NATIVE_VOLUME_KEY) {
        Some(value) => value,
        None => return 1,
    };
    let expected_custody_rows =
        match load_u64_or_zero(MM_METRICS_MIGRATION_EXPECTED_CUSTODY_ROWS_KEY) {
            Some(value) => value,
            None => return 1,
        };
    let migrated_custody_rows = match load_u64_or_zero(MM_METRICS_MIGRATION_CUSTODY_ROWS_KEY) {
        Some(value) => value,
        None => return 1,
    };
    let expected_native_custody =
        match load_u64_or_zero(MM_METRICS_MIGRATION_EXPECTED_NATIVE_CUSTODY_KEY) {
            Some(value) => value,
            None => return 1,
        };
    let reserved_native_custody = match load_u64_or_zero(MM_METRICS_MIGRATION_NATIVE_CUSTODY_KEY) {
        Some(value) => value,
        None => return 1,
    };
    let manifest = metrics_v3_manifest().unwrap_or([0u8; 32]);
    let mut result = Vec::with_capacity(115);
    result.extend_from_slice(&u64_to_bytes(version));
    result.push(u8::from(locked));
    result.push(u8::from(paused));
    result.push(u8::from(manifest != [0u8; 32]));
    result.extend_from_slice(&u64_to_bytes(expected_rows));
    result.extend_from_slice(&u64_to_bytes(migrated_rows));
    result.extend_from_slice(&u64_to_bytes(expected_sales));
    result.extend_from_slice(&u64_to_bytes(migrated_sales));
    result.extend_from_slice(&u64_to_bytes(native_volume));
    result.extend_from_slice(&u64_to_bytes(expected_custody_rows));
    result.extend_from_slice(&u64_to_bytes(migrated_custody_rows));
    result.extend_from_slice(&u64_to_bytes(expected_native_custody));
    result.extend_from_slice(&u64_to_bytes(reserved_native_custody));
    result.extend_from_slice(&manifest);
    lichen_sdk::set_return_data(&result);
    0
}

// ============================================================================
// EMERGENCY PAUSE (admin only)
// ============================================================================

/// Begin a two-step marketplace ownership transfer.
#[no_mangle]
pub extern "C" fn propose_admin(caller_ptr: *const u8, new_admin_ptr: *const u8) -> u32 {
    if !metrics_v3_ready() {
        return 3;
    }
    if get_value() != 0 {
        return 3;
    }
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => return 98,
    };
    let new_admin = match read_address(new_admin_ptr) {
        Some(address) if address.0 != [0u8; 32] && address != caller => address,
        _ => return 2,
    };
    if get_caller() != caller {
        return 200;
    }
    if !is_mm_admin(&caller.0) {
        return 1;
    }
    storage_set(MARKETPLACE_PENDING_OWNER_KEY, &new_admin.0);
    0
}

/// Complete a pending marketplace ownership transfer as the proposed owner.
#[no_mangle]
pub extern "C" fn accept_admin(caller_ptr: *const u8) -> u32 {
    if !metrics_v3_ready() {
        return 2;
    }
    if get_value() != 0 {
        return 2;
    }
    let caller = match read_address(caller_ptr) {
        Some(address) => address,
        None => return 98,
    };
    if get_caller() != caller {
        return 200;
    }
    match storage_get(MARKETPLACE_PENDING_OWNER_KEY) {
        Some(pending) if pending.len() == 32 && pending.as_slice() == caller.0 => {}
        _ => return 1,
    }
    storage_set(MARKETPLACE_OWNER_KEY, &caller.0);
    storage_set(MARKETPLACE_PENDING_OWNER_KEY, &[0u8; 32]);
    0
}

/// Return owner, pending owner, fee treasury, fee bps, and pause state.
#[no_mangle]
pub extern "C" fn get_marketplace_config() -> u32 {
    let owner = match storage_get(MARKETPLACE_OWNER_KEY) {
        Some(address) if address.len() == 32 && address.as_slice() != [0u8; 32] => address,
        _ => return 1,
    };
    let pending = match storage_get(MARKETPLACE_PENDING_OWNER_KEY) {
        None => [0u8; 32].to_vec(),
        Some(address) if address.len() == 32 => address,
        Some(_) => return 1,
    };
    let treasury = match storage_get(MARKETPLACE_FEE_TREASURY_KEY) {
        Some(address) if address.len() == 32 && address.as_slice() != [0u8; 32] => address,
        _ => return 1,
    };
    let fee = match get_marketplace_fee() {
        Some(fee) => fee,
        None => return 1,
    };
    let explicitly_paused = match load_mm_pause_state() {
        Some(paused) => paused,
        None => return 1,
    };
    let paused = explicitly_paused || !metrics_v3_ready();
    let mut data = Vec::with_capacity(105);
    data.extend_from_slice(&owner);
    data.extend_from_slice(&pending);
    data.extend_from_slice(&treasury);
    data.extend_from_slice(&u64_to_bytes(fee));
    data.push(u8::from(paused));
    lichen_sdk::set_return_data(&data);
    0
}

/// Pause the marketplace
#[no_mangle]
pub extern "C" fn mm_pause(caller_ptr: *const u8) -> u32 {
    if get_value() != 0 {
        return 2;
    }
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
    if get_value() != 0 {
        return 3;
    }
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
    if !metrics_v3_ready() {
        return 2;
    }
    storage_set(MM_PAUSE_KEY, &[0u8]);
    log_info("LichenMarket unpaused");
    0
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::vec;
    use lichen_sdk::bytes_to_u64;
    use lichen_sdk::test_mock;

    fn setup() {
        test_mock::reset();
        test_mock::set_contract_address([0xA5; 32]);
        storage_set(MM_METRICS_VERSION_KEY, &u64_to_bytes(MM_METRICS_VERSION));
        storage_set(MM_METRICS_MIGRATION_LOCK_KEY, &[0u8]);
    }

    fn seal_test_metrics_migration() {
        storage_set(MM_METRICS_VERSION_KEY, &u64_to_bytes(0));
        storage_set(MM_METRICS_MIGRATION_LOCK_KEY, &[1u8]);
        storage_set(MM_METRICS_MIGRATION_MANIFEST_KEY, &[0x5Au8; 32]);
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
        lichen_sdk::storage_set(
            &listing_fee_key(*nft_contract, token_id),
            &u64_to_bytes(DEFAULT_MARKETPLACE_FEE_BPS),
        );
    }

    fn offer_key(nft_contract: &Address, token_id: u64, offerer: &[u8; 32]) -> Vec<u8> {
        let mut key = b"offer:".to_vec();
        key.extend_from_slice(&nft_contract.0);
        key.push(b':');
        key.extend_from_slice(&token_id.to_le_bytes());
        key.push(b':');
        key.extend_from_slice(offerer);
        key
    }

    fn stored_unpaid(token: &Address, recipient: &[u8; 32]) -> u64 {
        stored_u64(&unpaid_payout_key(*token, Address(*recipient)))
    }

    fn royalty_response(recipient: [u8; 32], bps: u16) -> Vec<u8> {
        let mut response = Vec::with_capacity(34);
        response.extend_from_slice(&recipient);
        response.extend_from_slice(&bps.to_le_bytes());
        response
    }

    fn mock_zero_royalty() {
        test_mock::set_cross_call_response(Some(royalty_response([0u8; 32], 0)));
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
    fn test_list_nft_rejects_zero_price_before_owner_lookup() {
        setup();
        let seller = [3u8; 32];
        let nft = [4u8; 32];
        let pay = [5u8; 32];
        test_mock::set_caller(seller);

        let result = list_nft(seller.as_ptr(), nft.as_ptr(), 1, 0, pay.as_ptr());

        assert_eq!(result, 0);
        assert_eq!(test_mock::get_last_cross_call(), None);
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
    fn test_buy_nft_failed_refund_records_unpaid_buyer_payout() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());

        let seller = [3u8; 32];
        let buyer = [6u8; 32];
        let nft = Address([4u8; 32]);
        let pay = Address([5u8; 32]);
        create_test_listing(&seller, &nft, 1, 1_000_000, &pay);

        test_mock::set_caller(buyer);
        test_mock::set_cross_call_responses(vec![
            0u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(buy_nft(buyer.as_ptr(), nft.0.as_ptr(), 1), 0);

        let listing = test_mock::get_storage(&create_listing_key(nft, 1)).unwrap();
        assert_eq!(listing[144], 1);
        assert_eq!(stored_unpaid(&pay, &buyer), 1_000_000);
    }

    #[test]
    fn test_claim_unpaid_payout_retries_after_failed_transfer() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());

        let seller = [3u8; 32];
        let pay = Address([5u8; 32]);
        assert!(record_unpaid_payout(pay, Address(seller), 700_000));

        assert_eq!(get_unpaid_payout(pay.0.as_ptr(), seller.as_ptr()), 1);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 700_000);

        test_mock::set_caller(seller);
        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(claim_unpaid_payout(seller.as_ptr(), pay.0.as_ptr()), 0);
        assert_eq!(stored_unpaid(&pay, &seller), 700_000);

        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        assert_eq!(claim_unpaid_payout(seller.as_ptr(), pay.0.as_ptr()), 1);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 700_000);
        assert_eq!(stored_unpaid(&pay, &seller), 0);
    }

    #[test]
    fn test_claim_unpaid_payout_rejects_caller_spoof() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());

        let seller = [3u8; 32];
        let pay = Address([5u8; 32]);
        assert!(record_unpaid_payout(pay, Address(seller), 700_000));

        test_mock::set_caller([9u8; 32]);
        assert_eq!(claim_unpaid_payout(seller.as_ptr(), pay.0.as_ptr()), 200);
        assert_eq!(stored_unpaid(&pay, &seller), 700_000);
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
        let mut out = [0u8; LISTING_SIZE];
        let result = get_listing(nft.0.as_ptr(), 1, out.as_mut_ptr());
        assert_eq!(result, 1);
        assert_eq!(&test_mock::get_return_data()[0..32], &seller[..]);
    }

    #[test]
    fn test_get_listing_not_found() {
        setup();
        let nft = [4u8; 32];
        let mut out = [0u8; LISTING_SIZE];
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
        mock_zero_royalty();
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
    fn native_offer_requires_exact_creation_value_and_refunds_on_cancel() {
        setup();
        let admin = [1u8; 32];
        let treasury = [2u8; 32];
        let offerer = [3u8; 32];
        let nft = Address([4u8; 32]);
        let native = Address([0u8; 32]);
        test_mock::set_caller(admin);
        initialize(admin.as_ptr(), treasury.as_ptr());

        test_mock::set_caller(offerer);
        mock_zero_royalty();
        test_mock::set_value(MIN_OFFER_PRICE - 1);
        assert_eq!(
            make_offer(
                offerer.as_ptr(),
                nft.0.as_ptr(),
                1,
                MIN_OFFER_PRICE,
                native.0.as_ptr(),
            ),
            0
        );
        assert!(storage_get(&offer_key(&nft, 1, &offerer)).is_none());

        test_mock::set_value(MIN_OFFER_PRICE);
        assert_eq!(
            make_offer(
                offerer.as_ptr(),
                nft.0.as_ptr(),
                1,
                MIN_OFFER_PRICE,
                native.0.as_ptr(),
            ),
            1
        );
        assert_eq!(offer_custody_ready(nft, 1, &offerer), Some(true));
        assert_eq!(get_offer_custody(nft.0.as_ptr(), 1, offerer.as_ptr()), 0);
        assert_eq!(test_mock::get_return_data(), vec![1u8]);

        test_mock::set_value(0);
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        assert_eq!(cancel_offer(offerer.as_ptr(), nft.0.as_ptr(), 1), 1);
        assert_eq!(offer_custody_ready(nft, 1, &offerer), Some(false));
        assert_eq!(storage_get(&offer_key(&nft, 1, &offerer)).unwrap()[72], 0);
    }

    #[test]
    fn seller_cannot_fund_or_unwind_a_native_offer_during_acceptance() {
        setup();
        let admin = [1u8; 32];
        let treasury = [2u8; 32];
        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let offerer = [5u8; 32];
        let native = Address([0u8; 32]);
        test_mock::set_caller(admin);
        initialize(admin.as_ptr(), treasury.as_ptr());

        test_mock::set_caller(offerer);
        test_mock::set_value(MIN_OFFER_PRICE);
        mock_zero_royalty();
        assert_eq!(
            make_offer(
                offerer.as_ptr(),
                nft.0.as_ptr(),
                1,
                MIN_OFFER_PRICE,
                native.0.as_ptr(),
            ),
            1
        );

        test_mock::set_caller(seller);
        test_mock::set_value(MIN_OFFER_PRICE);
        test_mock::set_cross_call_response(Some(seller.to_vec()));
        assert_eq!(
            accept_offer(seller.as_ptr(), nft.0.as_ptr(), 1, offerer.as_ptr()),
            0
        );
        assert_eq!(storage_get(&offer_key(&nft, 1, &offerer)).unwrap()[72], 1);
        assert_eq!(offer_custody_ready(nft, 1, &offerer), Some(true));
    }

    #[test]
    fn native_collection_offer_is_funded_at_creation_and_refunded_on_cancel() {
        setup();
        let admin = [1u8; 32];
        let treasury = [2u8; 32];
        let offerer = Address([3u8; 32]);
        let collection = Address([4u8; 32]);
        let native = Address([0u8; 32]);
        test_mock::set_caller(admin);
        initialize(admin.as_ptr(), treasury.as_ptr());

        test_mock::set_caller(offerer.0);
        test_mock::set_value(MIN_OFFER_PRICE);
        mock_zero_royalty();
        assert_eq!(
            make_collection_offer(
                offerer.0.as_ptr(),
                collection.0.as_ptr(),
                MIN_OFFER_PRICE,
                native.0.as_ptr(),
                0,
            ),
            1
        );
        assert_eq!(
            collection_offer_custody_ready(collection, offerer),
            Some(true)
        );

        test_mock::set_value(0);
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        assert_eq!(
            cancel_collection_offer(offerer.0.as_ptr(), collection.0.as_ptr()),
            1
        );
        assert_eq!(
            collection_offer_custody_ready(collection, offerer),
            Some(false)
        );
    }

    #[test]
    fn test_accept_offer_refunds_when_nft_transfer_fails() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());

        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let pay = [5u8; 32];
        let offerer = [6u8; 32];

        test_mock::set_caller(offerer);
        mock_zero_royalty();
        assert_eq!(
            make_offer(offerer.as_ptr(), nft.0.as_ptr(), 1, 1_000_000, pay.as_ptr()),
            1
        );

        test_mock::set_caller(seller);
        test_mock::set_cross_call_responses(vec![
            seller.to_vec(),
            0u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(
            accept_offer(seller.as_ptr(), nft.0.as_ptr(), 1, offerer.as_ptr()),
            0
        );

        let data = test_mock::get_storage(&offer_key(&nft, 1, &offerer)).unwrap();
        assert_eq!(
            data[72], 1,
            "offer remains active after failed NFT transfer"
        );
        assert_eq!(test_mock::get_storage(MM_REENTRANCY_KEY), Some(vec![0u8]));
    }

    #[test]
    fn test_accept_offer_failure_preserves_funded_offer_without_new_liability() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());

        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let pay = Address([5u8; 32]);
        let offerer = [6u8; 32];

        test_mock::set_caller(offerer);
        mock_zero_royalty();
        assert_eq!(
            make_offer(
                offerer.as_ptr(),
                nft.0.as_ptr(),
                1,
                1_000_000,
                pay.0.as_ptr()
            ),
            1
        );

        test_mock::set_caller(seller);
        test_mock::set_cross_call_responses(vec![
            seller.to_vec(),
            2u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(
            accept_offer(seller.as_ptr(), nft.0.as_ptr(), 1, offerer.as_ptr()),
            0
        );

        let data = test_mock::get_storage(&offer_key(&nft, 1, &offerer)).unwrap();
        assert_eq!(data[72], 1, "funded offer remains retryable");
        assert_eq!(get_offerer_active_count(&offerer), 1);
        assert_eq!(stored_unpaid(&pay, &offerer), 0);
        assert_eq!(offer_custody_ready(nft, 1, &offerer), Some(true));
    }

    #[test]
    fn test_accept_offer_spends_existing_custody_and_marks_inactive() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());

        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let pay = [5u8; 32];
        let offerer = [6u8; 32];

        test_mock::set_caller(offerer);
        mock_zero_royalty();
        assert_eq!(
            make_offer(offerer.as_ptr(), nft.0.as_ptr(), 1, 1_000_000, pay.as_ptr()),
            1
        );

        test_mock::set_caller(seller);
        test_mock::set_cross_call_responses(vec![
            seller.to_vec(),
            1u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
        ]);
        let result = accept_offer(seller.as_ptr(), nft.0.as_ptr(), 1, offerer.as_ptr());
        assert_eq!(result, 1, "logs: {:?}", test_mock::get_logs());

        let data = test_mock::get_storage(&offer_key(&nft, 1, &offerer)).unwrap();
        assert_eq!(data[72], 0);
        assert_eq!(offer_custody_ready(nft, 1, &offerer), Some(false));
        let sale_count = test_mock::get_storage(MM_SALE_COUNT_KEY).unwrap();
        assert_eq!(bytes_to_u64(&sale_count), 1);
    }

    #[test]
    fn test_accept_offer_rejects_expired_offer() {
        setup();
        let owner = [1u8; 32];
        let fee_addr = [2u8; 32];
        test_mock::set_caller(owner);
        initialize(owner.as_ptr(), fee_addr.as_ptr());

        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let pay = [5u8; 32];
        let offerer = [6u8; 32];

        test_mock::set_caller(offerer);
        mock_zero_royalty();
        assert_eq!(
            make_offer_with_expiry(
                offerer.as_ptr(),
                nft.0.as_ptr(),
                1,
                1_000_000,
                pay.as_ptr(),
                100,
            ),
            1
        );

        test_mock::set_slot(101);
        test_mock::set_caller(seller);
        test_mock::set_cross_call_response(Some(seller.to_vec()));
        assert_eq!(
            accept_offer(seller.as_ptr(), nft.0.as_ptr(), 1, offerer.as_ptr()),
            0
        );
        assert_eq!(test_mock::get_storage(MM_REENTRANCY_KEY), Some(vec![0u8]));
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
        let attributes = test_mock::get_return_data();
        assert_eq!(attributes[0], 3); // rarity = Epic
        assert_eq!(attributes[1], 0); // category = Art
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
        let attributes = test_mock::get_return_data();
        assert_eq!(attributes[0], 4); // Legendary
        assert_eq!(attributes[1], 2); // Photography
        let trait_count = u16::from_le_bytes([attributes[2], attributes[3]]);
        assert_eq!(trait_count, 1);
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
        test_mock::set_cross_call_response(Some(seller.to_vec()));
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
        test_mock::set_cross_call_responses(vec![
            seller.to_vec(),
            royalty_response([0u8; 32], 0),
            1u32.to_le_bytes().to_vec(),
        ]);
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

        test_mock::set_cross_call_responses(vec![
            get_contract_address().0.to_vec(),
            0u32.to_le_bytes().to_vec(),
        ]);
        test_mock::set_slot(110);
        test_mock::set_caller(bidder);
        assert_eq!(place_bid(bidder.as_ptr(), nft.0.as_ptr(), 1, 1_000), 1);

        test_mock::set_caller(owner);
        assert_eq!(mm_pause(owner.as_ptr()), 0);

        test_mock::set_slot(1_700);
        test_mock::set_cross_call_responses(vec![
            1u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(settle_auction(owner.as_ptr(), nft.0.as_ptr(), 1), 1);

        let auction = test_mock::get_storage(&create_auction_key(nft, 1)).unwrap();
        assert_eq!(auction[144], 2);
    }

    #[test]
    fn test_settle_auction_failed_reserve_refund_records_exact_liability() {
        setup();
        let seller = [3u8; 32];
        let bidder = [7u8; 32];
        let nft = Address([4u8; 32]);
        let payment_token = Address([5u8; 32]);
        let key = create_auction_key(nft, 1);

        let mut data = alloc::vec![0u8; AUCTION_SIZE];
        data[0..32].copy_from_slice(&seller);
        data[32..64].copy_from_slice(&nft.0);
        data[64..72].copy_from_slice(&1u64.to_le_bytes());
        data[72..80].copy_from_slice(&1_000u64.to_le_bytes());
        data[80..88].copy_from_slice(&2_000u64.to_le_bytes());
        data[88..96].copy_from_slice(&1_000u64.to_le_bytes());
        data[96..128].copy_from_slice(&bidder);
        data[128..136].copy_from_slice(&1_000u64.to_le_bytes());
        data[136..144].copy_from_slice(&2_000u64.to_le_bytes());
        data[144] = 1;
        data[145..177].copy_from_slice(&payment_token.0);
        lichen_sdk::storage_set(&key, &data);
        lichen_sdk::storage_set(&auction_escrow_key(nft, 1), &[1u8]);
        lichen_sdk::storage_set(&auction_bid_custody_key(nft, 1), &[1u8]);
        lichen_sdk::storage_set(b"marketplace_fee_addr", &[2u8; 32]);

        test_mock::set_slot(2_001);
        test_mock::set_caller(seller);
        test_mock::set_cross_call_responses(vec![
            1u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
        ]);

        assert_eq!(settle_auction(seller.as_ptr(), nft.0.as_ptr(), 1), 2);

        let stored = test_mock::get_storage(&key).unwrap();
        assert_eq!(stored[144], 0);
        assert_eq!(bytes_to_u64(&stored[88..96]), 1_000);
        assert_eq!(stored_unpaid(&payment_token, &bidder), 1_000);
    }

    #[test]
    fn test_create_auction_rejects_end_time_overflow() {
        setup();
        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let payment_token = Address([5u8; 32]);

        test_mock::set_slot(u64::MAX - 10);
        test_mock::set_caller(seller);
        test_mock::set_cross_call_responses(vec![seller.to_vec(), royalty_response([0u8; 32], 0)]);

        assert_eq!(
            create_auction(
                seller.as_ptr(),
                nft.0.as_ptr(),
                1,
                1_000,
                0,
                AUCTION_MIN_DURATION_SLOTS,
                payment_token.0.as_ptr(),
            ),
            0
        );
        assert_eq!(test_mock::get_storage(&create_auction_key(nft, 1)), None);
    }

    #[test]
    fn test_place_bid_previous_refund_failure_preserves_high_bid() {
        setup();
        let seller = [3u8; 32];
        let prev_bidder = [6u8; 32];
        let bidder = [7u8; 32];
        let nft = Address([4u8; 32]);
        let payment_token = Address([5u8; 32]);
        let key = create_auction_key(nft, 1);

        let mut data = alloc::vec![0u8; AUCTION_SIZE];
        data[0..32].copy_from_slice(&seller);
        data[32..64].copy_from_slice(&nft.0);
        data[64..72].copy_from_slice(&1u64.to_le_bytes());
        data[72..80].copy_from_slice(&100u64.to_le_bytes());
        data[88..96].copy_from_slice(&100u64.to_le_bytes());
        data[96..128].copy_from_slice(&prev_bidder);
        data[128..136].copy_from_slice(&1_000u64.to_le_bytes());
        data[136..144].copy_from_slice(&2_000u64.to_le_bytes());
        data[144] = 1;
        data[145..177].copy_from_slice(&payment_token.0);
        lichen_sdk::storage_set(&key, &data);
        lichen_sdk::storage_set(&auction_escrow_key(nft, 1), &[1u8]);
        lichen_sdk::storage_set(&auction_bid_custody_key(nft, 1), &[1u8]);
        lichen_sdk::storage_set(b"marketplace_fee_addr", &[2u8; 32]);

        test_mock::set_slot(1_100);
        test_mock::set_caller(bidder);
        test_mock::set_cross_call_responses(vec![
            get_contract_address().0.to_vec(),
            0u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
            0u32.to_le_bytes().to_vec(),
        ]);

        assert_eq!(place_bid(bidder.as_ptr(), nft.0.as_ptr(), 1, 105), 0);

        let stored = test_mock::get_storage(&key).unwrap();
        assert_eq!(bytes_to_u64(&stored[88..96]), 100);
        assert_eq!(&stored[96..128], &prev_bidder);
        assert_eq!(test_mock::get_storage(MM_REENTRANCY_KEY), Some(vec![0u8]));
    }

    #[test]
    fn test_place_bid_records_unpaid_new_bidder_when_refund_unwinds_fail() {
        setup();
        let seller = [3u8; 32];
        let prev_bidder = [6u8; 32];
        let bidder = [7u8; 32];
        let nft = Address([4u8; 32]);
        let payment_token = Address([5u8; 32]);
        let key = create_auction_key(nft, 1);

        let mut data = alloc::vec![0u8; AUCTION_SIZE];
        data[0..32].copy_from_slice(&seller);
        data[32..64].copy_from_slice(&nft.0);
        data[64..72].copy_from_slice(&1u64.to_le_bytes());
        data[72..80].copy_from_slice(&100u64.to_le_bytes());
        data[88..96].copy_from_slice(&100u64.to_le_bytes());
        data[96..128].copy_from_slice(&prev_bidder);
        data[128..136].copy_from_slice(&1_000u64.to_le_bytes());
        data[136..144].copy_from_slice(&2_000u64.to_le_bytes());
        data[144] = 1;
        data[145..177].copy_from_slice(&payment_token.0);
        lichen_sdk::storage_set(&key, &data);
        lichen_sdk::storage_set(&auction_escrow_key(nft, 1), &[1u8]);
        lichen_sdk::storage_set(&auction_bid_custody_key(nft, 1), &[1u8]);
        lichen_sdk::storage_set(b"marketplace_fee_addr", &[2u8; 32]);

        test_mock::set_slot(1_100);
        test_mock::set_caller(bidder);
        test_mock::set_cross_call_responses(vec![
            get_contract_address().0.to_vec(),
            0u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
            2u32.to_le_bytes().to_vec(),
        ]);

        assert_eq!(place_bid(bidder.as_ptr(), nft.0.as_ptr(), 1, 105), 0);

        let stored = test_mock::get_storage(&key).unwrap();
        assert_eq!(bytes_to_u64(&stored[88..96]), 100);
        assert_eq!(&stored[96..128], &prev_bidder);
        assert_eq!(stored_unpaid(&payment_token, &bidder), 105);
    }

    #[test]
    fn test_get_nft_attributes_not_found() {
        setup();
        let nft = [4u8; 32];
        let mut out = [0u8; 256];
        assert_eq!(get_nft_attributes(nft.as_ptr(), 999, out.as_mut_ptr()), 0);
    }

    #[test]
    fn test_platform_fee_ledger_withdrawal_is_exact_and_retry_safe() {
        setup();
        let admin = [1u8; 32];
        let treasury = [2u8; 32];
        let token = Address([3u8; 32]);
        test_mock::set_caller(admin);
        initialize(admin.as_ptr(), treasury.as_ptr());
        assert!(accrue_platform_fee(token, 750));
        assert_eq!(get_platform_fees(token.0.as_ptr()), 0);
        assert_eq!(bytes_to_u64(&test_mock::get_return_data()), 750);

        test_mock::set_cross_call_response(Some(2u32.to_le_bytes().to_vec()));
        assert_eq!(
            withdraw_platform_fees(admin.as_ptr(), token.0.as_ptr(), 500),
            4
        );
        assert_eq!(stored_u64(&platform_fee_key(token)), 750);

        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        assert_eq!(
            withdraw_platform_fees(admin.as_ptr(), token.0.as_ptr(), 500),
            0
        );
        assert_eq!(stored_u64(&platform_fee_key(token)), 250);
    }

    #[test]
    fn test_offer_fee_and_royalty_terms_are_immutable_and_queryable() {
        setup();
        let admin = [1u8; 32];
        let treasury = [2u8; 32];
        let offerer = [3u8; 32];
        let nft = Address([4u8; 32]);
        let payment = [5u8; 32];
        let creator = [6u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr(), treasury.as_ptr());
        test_mock::set_cross_call_response(Some(royalty_response(creator, 500)));
        assert_eq!(
            set_collection_royalty(admin.as_ptr(), nft.0.as_ptr(), creator.as_ptr(), 500),
            0
        );

        test_mock::set_caller(offerer);
        test_mock::set_cross_call_responses(vec![
            royalty_response(creator, 500),
            0u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(
            make_offer(
                offerer.as_ptr(),
                nft.0.as_ptr(),
                7,
                MIN_OFFER_PRICE,
                payment.as_ptr(),
            ),
            1
        );
        test_mock::set_caller(admin);
        assert_eq!(set_marketplace_fee(admin.as_ptr(), 900), 1);

        assert_eq!(get_offer(nft.0.as_ptr(), 7, offerer.as_ptr()), 0);
        let terms = test_mock::get_return_data();
        assert_eq!(terms.len(), 73 + 8 + 32 + 8);
        assert_eq!(bytes_to_u64(&terms[73..81]), 250);
        assert_eq!(&terms[81..113], &creator);
        assert_eq!(bytes_to_u64(&terms[113..121]), 500);
    }

    #[test]
    fn test_collection_royalty_is_admin_verified_and_queryable() {
        setup();
        let admin = [1u8; 32];
        let treasury = [2u8; 32];
        let attacker = [3u8; 32];
        let nft = [4u8; 32];
        let creator = [5u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr(), treasury.as_ptr());

        test_mock::set_caller(attacker);
        assert_eq!(
            set_collection_royalty(attacker.as_ptr(), nft.as_ptr(), attacker.as_ptr(), 1_000),
            1
        );
        test_mock::set_caller(admin);
        test_mock::set_cross_call_response(Some(royalty_response(creator, 750)));
        assert_eq!(
            set_collection_royalty(admin.as_ptr(), nft.as_ptr(), creator.as_ptr(), 750),
            0
        );
        assert_eq!(get_collection_royalty(nft.as_ptr()), 0);
        let terms = test_mock::get_return_data();
        assert_eq!(&terms[..32], &creator);
        assert_eq!(bytes_to_u64(&terms[32..40]), 750);

        test_mock::set_cross_call_response(Some(royalty_response(creator, 625)));
        assert_eq!(get_canonical_royalty(nft.as_ptr(), 17), 0);
        let token_terms = test_mock::get_return_data();
        assert_eq!(&token_terms[..32], &creator);
        assert_eq!(bytes_to_u64(&token_terms[32..40]), 625);
    }

    #[test]
    fn malformed_operational_flags_fail_closed() {
        setup();
        storage_set(MM_PAUSE_KEY, &[2u8]);
        assert!(is_mm_paused());

        storage_set(MM_REENTRANCY_KEY, &[2u8]);
        assert!(!reentrancy_enter());
        assert_eq!(test_mock::get_storage(MM_REENTRANCY_KEY), Some(vec![2u8]));
    }

    #[test]
    fn admin_rotation_is_two_step_and_config_is_exact() {
        setup();
        let admin = [1u8; 32];
        let next_admin = [2u8; 32];
        let treasury = [3u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr(), treasury.as_ptr());

        assert_eq!(propose_admin(admin.as_ptr(), next_admin.as_ptr()), 0);
        assert_eq!(
            test_mock::get_storage(MARKETPLACE_OWNER_KEY),
            Some(admin.to_vec())
        );
        test_mock::set_caller(next_admin);
        assert_eq!(accept_admin(next_admin.as_ptr()), 0);
        assert_eq!(
            test_mock::get_storage(MARKETPLACE_OWNER_KEY),
            Some(next_admin.to_vec())
        );
        assert_eq!(get_marketplace_config(), 0);
        let config = test_mock::get_return_data();
        assert_eq!(config.len(), 105);
        assert_eq!(&config[..32], &next_admin);
        assert_eq!(&config[32..64], &[0u8; 32]);
        assert_eq!(&config[64..96], &treasury);
        assert_eq!(bytes_to_u64(&config[96..104]), DEFAULT_MARKETPLACE_FEE_BPS);
        assert_eq!(config[104], 0);

        test_mock::set_caller(admin);
        assert_eq!(set_marketplace_fee(admin.as_ptr(), 500), 0);
        test_mock::set_caller(next_admin);
        assert_eq!(set_marketplace_fee(next_admin.as_ptr(), 500), 1);
    }

    #[test]
    fn offer_index_is_exact_and_legacy_rows_require_migration() {
        setup();
        let admin = [1u8; 32];
        let treasury = [2u8; 32];
        let offerer = [3u8; 32];
        let nft = Address([4u8; 32]);
        let payment = [5u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr(), treasury.as_ptr());

        let key = offer_key(&nft, 7, &offerer);
        let mut legacy = vec![0u8; OFFER_SIZE];
        legacy[..32].copy_from_slice(&offerer);
        legacy[32..40].copy_from_slice(&MIN_OFFER_PRICE.to_le_bytes());
        legacy[40..72].copy_from_slice(&payment);
        legacy[72] = 1;
        storage_set(&key, &legacy);
        set_offerer_active_count(&offerer, 1);

        test_mock::set_caller(offerer);
        assert_eq!(get_offer_count(nft.0.as_ptr(), 7), 0);

        test_mock::set_caller(admin);
        seal_test_metrics_migration();
        mock_zero_royalty();
        assert_eq!(
            migrate_v3_offer(
                admin.as_ptr(),
                nft.0.as_ptr(),
                7,
                offerer.as_ptr(),
                DEFAULT_MARKETPLACE_FEE_BPS,
                [0u8; 32].as_ptr(),
                0,
            ),
            0
        );
        assert_eq!(get_offer_count(nft.0.as_ptr(), 7), 1);

        test_mock::set_caller(offerer);
        assert_eq!(cancel_offer(offerer.as_ptr(), nft.0.as_ptr(), 7), 0);
        assert_eq!(get_offer_count(nft.0.as_ptr(), 7), 1);
        assert_eq!(get_offerer_active_count(&offerer), 1);
    }

    #[test]
    fn legacy_collection_offer_terms_require_exact_migration() {
        setup();
        let admin = [1u8; 32];
        let treasury = [2u8; 32];
        let offerer = Address([3u8; 32]);
        let collection = Address([4u8; 32]);
        let payment = [5u8; 32];
        let creator = Address([6u8; 32]);
        test_mock::set_caller(admin);
        initialize(admin.as_ptr(), treasury.as_ptr());

        let mut key = b"col_offer:".to_vec();
        key.extend_from_slice(&collection.0);
        key.push(b':');
        key.extend_from_slice(&offerer.0);
        let mut legacy = vec![0u8; COLLECTION_OFFER_SIZE];
        legacy[..32].copy_from_slice(&offerer.0);
        legacy[32..64].copy_from_slice(&collection.0);
        legacy[64..72].copy_from_slice(&MIN_OFFER_PRICE.to_le_bytes());
        legacy[72..104].copy_from_slice(&payment);
        legacy[104] = 1;
        storage_set(&key, &legacy);
        seal_test_metrics_migration();

        assert_eq!(
            get_collection_offer(collection.0.as_ptr(), offerer.0.as_ptr()),
            2
        );
        test_mock::set_caller(offerer.0);
        test_mock::set_cross_call_response(Some(royalty_response(creator.0, 500)));
        assert_eq!(
            migrate_v3_collection_offer(
                offerer.0.as_ptr(),
                collection.0.as_ptr(),
                offerer.0.as_ptr(),
                DEFAULT_MARKETPLACE_FEE_BPS,
                creator.0.as_ptr(),
                500,
            ),
            7
        );

        test_mock::set_caller(admin);
        test_mock::set_cross_call_response(Some(royalty_response(creator.0, 500)));
        assert_eq!(
            migrate_v3_collection_offer(
                admin.as_ptr(),
                collection.0.as_ptr(),
                offerer.0.as_ptr(),
                DEFAULT_MARKETPLACE_FEE_BPS,
                creator.0.as_ptr(),
                500,
            ),
            0
        );
        assert_eq!(
            get_collection_offer(collection.0.as_ptr(), offerer.0.as_ptr()),
            0
        );
        let migrated = test_mock::get_return_data();
        assert_eq!(migrated.len(), COLLECTION_OFFER_SIZE + 8 + 32 + 8);
        assert_eq!(
            bytes_to_u64(&migrated[COLLECTION_OFFER_SIZE..COLLECTION_OFFER_SIZE + 8]),
            DEFAULT_MARKETPLACE_FEE_BPS
        );
        assert_eq!(
            &migrated[COLLECTION_OFFER_SIZE + 8..COLLECTION_OFFER_SIZE + 40],
            &creator.0
        );
        assert_eq!(bytes_to_u64(&migrated[COLLECTION_OFFER_SIZE + 40..]), 500);

        test_mock::set_cross_call_response(Some(royalty_response(creator.0, 500)));
        assert_eq!(
            migrate_v3_collection_offer(
                admin.as_ptr(),
                collection.0.as_ptr(),
                offerer.0.as_ptr(),
                DEFAULT_MARKETPLACE_FEE_BPS,
                creator.0.as_ptr(),
                500,
            ),
            0
        );
        storage_set(
            &collection_offer_fee_key(collection, offerer),
            &u64_to_bytes(999),
        );
        test_mock::set_cross_call_response(Some(royalty_response(creator.0, 500)));
        assert_eq!(
            migrate_v3_collection_offer(
                admin.as_ptr(),
                collection.0.as_ptr(),
                offerer.0.as_ptr(),
                DEFAULT_MARKETPLACE_FEE_BPS,
                creator.0.as_ptr(),
                500,
            ),
            6
        );
    }

    #[test]
    fn auction_creation_and_cancellation_preserve_nft_custody() {
        setup();
        let admin = [1u8; 32];
        let treasury = [2u8; 32];
        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let payment = [5u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr(), treasury.as_ptr());
        test_mock::set_caller(seller);
        test_mock::set_cross_call_responses(vec![
            seller.to_vec(),
            royalty_response([0u8; 32], 0),
            1u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(
            create_auction(
                seller.as_ptr(),
                nft.0.as_ptr(),
                9,
                100,
                0,
                AUCTION_MIN_DURATION_SLOTS,
                payment.as_ptr(),
            ),
            1
        );
        assert_eq!(auction_escrowed(nft, 9), Some(true));
        assert_eq!(
            test_mock::get_storage(&create_auction_key(nft, 9)).unwrap()[144],
            1
        );

        test_mock::set_cross_call_response(Some(1u32.to_le_bytes().to_vec()));
        assert_eq!(cancel_auction(seller.as_ptr(), nft.0.as_ptr(), 9), 1);
        assert_eq!(auction_escrowed(nft, 9), Some(false));
        assert_eq!(
            test_mock::get_storage(&create_auction_key(nft, 9)).unwrap()[144],
            0
        );
    }

    #[test]
    fn legacy_auction_cannot_take_bids_before_seller_custody_migration() {
        setup();
        let seller = [3u8; 32];
        let bidder = [6u8; 32];
        let nft = Address([4u8; 32]);
        let payment = [5u8; 32];
        let mut data = vec![0u8; AUCTION_SIZE];
        data[..32].copy_from_slice(&seller);
        data[32..64].copy_from_slice(&nft.0);
        data[64..72].copy_from_slice(&1u64.to_le_bytes());
        data[72..80].copy_from_slice(&100u64.to_le_bytes());
        data[128..136].copy_from_slice(&10u64.to_le_bytes());
        data[136..144].copy_from_slice(&1_000u64.to_le_bytes());
        data[144] = 1;
        data[145..177].copy_from_slice(&payment);
        storage_set(&create_auction_key(nft, 1), &data);

        test_mock::set_slot(20);
        test_mock::set_caller(bidder);
        assert_eq!(place_bid(bidder.as_ptr(), nft.0.as_ptr(), 1, 100), 0);

        test_mock::set_caller(seller);
        seal_test_metrics_migration();
        test_mock::set_cross_call_responses(vec![
            royalty_response([0u8; 32], 0),
            seller.to_vec(),
            1u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(
            migrate_auction_escrow(seller.as_ptr(), nft.0.as_ptr(), 1),
            1
        );
        assert_eq!(auction_escrowed(nft, 1), Some(true));

        test_mock::set_caller(bidder);
        test_mock::set_cross_call_responses(vec![
            get_contract_address().0.to_vec(),
            0u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(place_bid(bidder.as_ptr(), nft.0.as_ptr(), 1, 100), 0);
    }

    #[test]
    fn legacy_listing_requires_exact_canonical_terms_migration() {
        setup();
        let admin = [1u8; 32];
        let treasury = [2u8; 32];
        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let payment = Address([5u8; 32]);
        let buyer = [6u8; 32];
        let creator = [7u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr(), treasury.as_ptr());

        let mut legacy = vec![0u8; LISTING_SIZE];
        legacy[..32].copy_from_slice(&seller);
        legacy[32..64].copy_from_slice(&nft.0);
        legacy[64..72].copy_from_slice(&1u64.to_le_bytes());
        legacy[72..80].copy_from_slice(&1_000u64.to_le_bytes());
        legacy[80..112].copy_from_slice(&payment.0);
        legacy[112..144].copy_from_slice(&[9u8; 32]);
        legacy[144] = 1;
        legacy[145..147].copy_from_slice(&5_000u16.to_le_bytes());
        storage_set(&create_listing_key(nft, 1), &legacy);

        test_mock::set_caller(buyer);
        assert_eq!(buy_nft(buyer.as_ptr(), nft.0.as_ptr(), 1), 0);
        assert_eq!(test_mock::get_last_cross_call(), None);

        test_mock::set_caller(seller);
        assert_eq!(cancel_listing(seller.as_ptr(), nft.0.as_ptr(), 1), 1);
        assert_eq!(storage_get(&create_listing_key(nft, 1)).unwrap()[144], 0);
        legacy[144] = 1;
        storage_set(&create_listing_key(nft, 1), &legacy);
        seal_test_metrics_migration();

        test_mock::set_caller(admin);
        test_mock::set_slot(77);
        test_mock::set_cross_call_response(Some(royalty_response(creator, 500)));
        assert_eq!(
            migrate_v3_listing(
                admin.as_ptr(),
                nft.0.as_ptr(),
                1,
                DEFAULT_MARKETPLACE_FEE_BPS,
                creator.as_ptr(),
                500,
            ),
            0
        );
        let migrated = storage_get(&create_listing_key(nft, 1)).unwrap();
        assert_eq!(&migrated[..112], &legacy[..112]);
        assert_eq!(&migrated[112..144], &creator);
        assert_eq!(migrated[144], 1);
        assert_eq!(u16::from_le_bytes([migrated[145], migrated[146]]), 500);
        assert_eq!(
            snapshotted_fee_bps(&listing_fee_key(nft, 1)),
            Some(DEFAULT_MARKETPLACE_FEE_BPS)
        );
        assert_eq!(load_u64_or_zero(&listing_slot_key(nft, 1)), Some(77));

        test_mock::set_slot(88);
        test_mock::set_cross_call_response(Some(royalty_response(creator, 500)));
        assert_eq!(
            migrate_v3_listing(
                admin.as_ptr(),
                nft.0.as_ptr(),
                1,
                DEFAULT_MARKETPLACE_FEE_BPS,
                creator.as_ptr(),
                500,
            ),
            0
        );
        assert_eq!(load_u64_or_zero(&listing_slot_key(nft, 1)), Some(77));
        storage_set(&listing_fee_key(nft, 1), &u64_to_bytes(999));
        test_mock::set_cross_call_response(Some(royalty_response(creator, 500)));
        assert_eq!(
            migrate_v3_listing(
                admin.as_ptr(),
                nft.0.as_ptr(),
                1,
                DEFAULT_MARKETPLACE_FEE_BPS,
                creator.as_ptr(),
                500,
            ),
            6
        );
    }

    #[test]
    fn legacy_offer_can_unwind_without_admin_terms_migration() {
        setup();
        let offerer = [3u8; 32];
        let nft = Address([4u8; 32]);
        let payment = [5u8; 32];
        let key = offer_key(&nft, 1, &offerer);
        let mut legacy = vec![0u8; OFFER_SIZE];
        legacy[..32].copy_from_slice(&offerer);
        legacy[32..40].copy_from_slice(&MIN_OFFER_PRICE.to_le_bytes());
        legacy[40..72].copy_from_slice(&payment);
        legacy[72] = 1;
        storage_set(&key, &legacy);
        set_offerer_active_count(&offerer, 1);

        test_mock::set_caller(offerer);
        assert_eq!(cancel_offer(offerer.as_ptr(), nft.0.as_ptr(), 1), 1);
        assert_eq!(storage_get(&key).unwrap()[72], 0);
        assert_eq!(get_offerer_active_count(&offerer), 0);
        assert_eq!(load_active_offer_count(nft, 1), Some(0));
        assert_eq!(offer_is_indexed(nft, 1, &offerer), Some(false));
    }

    #[test]
    fn legacy_native_offers_are_manifest_counted_funded_and_resumable() {
        setup();
        let admin = [1u8; 32];
        let treasury = [2u8; 32];
        let offerer = Address([3u8; 32]);
        let nft = Address([4u8; 32]);
        let native = Address([0u8; 32]);
        let manifest = [8u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr(), treasury.as_ptr());

        let mut offer = vec![0u8; OFFER_SIZE];
        offer[..32].copy_from_slice(&offerer.0);
        offer[32..40].copy_from_slice(&MIN_OFFER_PRICE.to_le_bytes());
        offer[40..72].copy_from_slice(&native.0);
        offer[72] = 1;
        storage_set(&offer_key(&nft, 7, &offerer.0), &offer);
        set_offerer_active_count(&offerer.0, 1);

        let mut collection_key = b"col_offer:".to_vec();
        collection_key.extend_from_slice(&nft.0);
        collection_key.push(b':');
        collection_key.extend_from_slice(&offerer.0);
        let mut collection_offer = vec![0u8; COLLECTION_OFFER_SIZE];
        collection_offer[..32].copy_from_slice(&offerer.0);
        collection_offer[32..64].copy_from_slice(&nft.0);
        collection_offer[64..72].copy_from_slice(&MIN_OFFER_PRICE.to_le_bytes());
        collection_offer[72..104].copy_from_slice(&native.0);
        collection_offer[104] = 1;
        storage_set(&collection_key, &collection_offer);

        storage_set(MM_METRICS_VERSION_KEY, &u64_to_bytes(0));
        assert_eq!(begin_metrics_v3_migration(admin.as_ptr()), 0);
        assert_eq!(
            seal_metrics_v3_manifest(
                admin.as_ptr(),
                manifest.as_ptr(),
                0,
                0,
                0,
                2,
                MIN_OFFER_PRICE * 2,
            ),
            0
        );
        test_mock::set_cross_call_responses(vec![
            royalty_response([0u8; 32], 0),
            royalty_response([0u8; 32], 0),
        ]);
        assert_eq!(
            migrate_v3_offer(
                admin.as_ptr(),
                nft.0.as_ptr(),
                7,
                offerer.0.as_ptr(),
                DEFAULT_MARKETPLACE_FEE_BPS,
                [0u8; 32].as_ptr(),
                0,
            ),
            0
        );
        assert_eq!(
            migrate_v3_collection_offer(
                admin.as_ptr(),
                nft.0.as_ptr(),
                offerer.0.as_ptr(),
                DEFAULT_MARKETPLACE_FEE_BPS,
                [0u8; 32].as_ptr(),
                0,
            ),
            0
        );

        test_mock::set_caller(offerer.0);
        test_mock::set_value(0);
        test_mock::set_cross_call_response(Some(
            (MIN_OFFER_PRICE * 2 - 1).to_le_bytes().to_vec(),
        ));
        assert_eq!(
            migrate_v3_offer_custody(offerer.0.as_ptr(), nft.0.as_ptr(), 7),
            6
        );
        test_mock::set_cross_call_response(Some(
            (MIN_OFFER_PRICE * 2).to_le_bytes().to_vec(),
        ));
        assert_eq!(
            migrate_v3_offer_custody(offerer.0.as_ptr(), nft.0.as_ptr(), 7),
            1
        );
        assert_eq!(
            migrate_v3_offer_custody(offerer.0.as_ptr(), nft.0.as_ptr(), 7),
            1
        );
        test_mock::set_cross_call_response(Some(
            (MIN_OFFER_PRICE * 2).to_le_bytes().to_vec(),
        ));
        assert_eq!(
            migrate_v3_collection_offer_custody(offerer.0.as_ptr(), nft.0.as_ptr()),
            1
        );
        assert_eq!(stored_u64(MM_METRICS_MIGRATION_CUSTODY_ROWS_KEY), 2);
        assert_eq!(
            stored_u64(MM_METRICS_MIGRATION_NATIVE_CUSTODY_KEY),
            MIN_OFFER_PRICE * 2
        );
        assert_eq!(get_offer_custody(nft.0.as_ptr(), 7, offerer.0.as_ptr()), 0);
        assert_eq!(test_mock::get_return_data(), vec![1u8]);
        assert_eq!(
            get_collection_offer_custody(nft.0.as_ptr(), offerer.0.as_ptr()),
            0
        );
        assert_eq!(test_mock::get_return_data(), vec![1u8]);

        test_mock::set_caller(admin);
        assert_eq!(migrate_metrics_v3_global(admin.as_ptr()), 0);
        test_mock::set_cross_call_response(Some(
            (MIN_OFFER_PRICE * 2).to_le_bytes().to_vec(),
        ));
        assert_eq!(complete_metrics_v3_migration(admin.as_ptr()), 0);
    }

    #[test]
    fn legacy_unpaid_payout_requires_exact_treasury_custody_migration() {
        setup();
        let admin = [1u8; 32];
        let treasury = [2u8; 32];
        let recipient = [3u8; 32];
        let native = Address([0u8; 32]);
        let manifest = [8u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr(), treasury.as_ptr());
        storage_set(
            &unpaid_payout_key(native, Address(recipient)),
            &u64_to_bytes(700),
        );

        test_mock::set_caller(recipient);
        assert_eq!(
            claim_unpaid_payout(recipient.as_ptr(), native.0.as_ptr()),
            0
        );
        assert_eq!(
            migrate_v3_unpaid_payout_custody(
                treasury.as_ptr(),
                native.0.as_ptr(),
                recipient.as_ptr(),
            ),
            0
        );

        test_mock::set_caller(admin);
        storage_set(MM_METRICS_VERSION_KEY, &u64_to_bytes(0));
        assert_eq!(begin_metrics_v3_migration(admin.as_ptr()), 0);
        assert_eq!(
            seal_metrics_v3_manifest(admin.as_ptr(), manifest.as_ptr(), 0, 0, 0, 1, 700,),
            0
        );
        assert_eq!(migrate_metrics_v3_global(admin.as_ptr()), 0);
        assert_eq!(complete_metrics_v3_migration(admin.as_ptr()), 3);

        test_mock::set_caller(treasury);
        test_mock::set_value(1);
        assert_eq!(
            migrate_v3_unpaid_payout_custody(
                treasury.as_ptr(),
                native.0.as_ptr(),
                recipient.as_ptr(),
            ),
            0
        );
        test_mock::set_value(0);
        test_mock::set_cross_call_response(Some(699u64.to_le_bytes().to_vec()));
        assert_eq!(
            migrate_v3_unpaid_payout_custody(
                treasury.as_ptr(),
                native.0.as_ptr(),
                recipient.as_ptr(),
            ),
            0
        );
        test_mock::set_cross_call_response(Some(700u64.to_le_bytes().to_vec()));
        assert_eq!(
            migrate_v3_unpaid_payout_custody(
                treasury.as_ptr(),
                native.0.as_ptr(),
                recipient.as_ptr(),
            ),
            1
        );
        assert_eq!(
            migrate_v3_unpaid_payout_custody(
                treasury.as_ptr(),
                native.0.as_ptr(),
                recipient.as_ptr(),
            ),
            1
        );
        test_mock::set_value(1);
        assert_eq!(
            migrate_v3_unpaid_payout_custody(
                treasury.as_ptr(),
                native.0.as_ptr(),
                recipient.as_ptr(),
            ),
            0
        );

        test_mock::set_value(0);
        test_mock::set_caller(admin);
        test_mock::set_cross_call_response(Some(699u64.to_le_bytes().to_vec()));
        assert_eq!(complete_metrics_v3_migration(admin.as_ptr()), 3);
        test_mock::set_cross_call_response(Some(700u64.to_le_bytes().to_vec()));
        assert_eq!(complete_metrics_v3_migration(admin.as_ptr()), 0);
        test_mock::set_caller(recipient);
        test_mock::set_cross_call_response(Some(0u32.to_le_bytes().to_vec()));
        assert_eq!(
            claim_unpaid_payout(recipient.as_ptr(), native.0.as_ptr()),
            1
        );
        assert_eq!(stored_unpaid(&native, &recipient), 0);
    }

    #[test]
    fn legacy_auction_with_bid_requires_funded_and_canonical_migration() {
        setup();
        let admin = [1u8; 32];
        let treasury = [2u8; 32];
        let seller = [3u8; 32];
        let nft = Address([4u8; 32]);
        let bidder = [6u8; 32];
        let creator = [7u8; 32];
        let native = Address([0u8; 32]);
        let manifest = [8u8; 32];
        test_mock::set_caller(admin);
        initialize(admin.as_ptr(), treasury.as_ptr());

        let mut legacy = vec![0u8; AUCTION_SIZE];
        legacy[..32].copy_from_slice(&seller);
        legacy[32..64].copy_from_slice(&nft.0);
        legacy[64..72].copy_from_slice(&1u64.to_le_bytes());
        legacy[72..80].copy_from_slice(&100u64.to_le_bytes());
        legacy[88..96].copy_from_slice(&150u64.to_le_bytes());
        legacy[96..128].copy_from_slice(&bidder);
        legacy[128..136].copy_from_slice(&10u64.to_le_bytes());
        legacy[136..144].copy_from_slice(&1_000u64.to_le_bytes());
        legacy[144] = 1;
        legacy[145..177].copy_from_slice(&native.0);
        storage_set(&create_auction_key(nft, 1), &legacy);

        storage_set(MM_METRICS_VERSION_KEY, &u64_to_bytes(0));
        assert_eq!(begin_metrics_v3_migration(admin.as_ptr()), 0);
        assert_eq!(
            seal_metrics_v3_manifest(admin.as_ptr(), manifest.as_ptr(), 0, 0, 0, 1, 150,),
            0
        );

        test_mock::set_caller(seller);
        assert_eq!(
            migrate_auction_escrow(seller.as_ptr(), nft.0.as_ptr(), 1),
            0
        );
        assert_eq!(test_mock::get_last_cross_call(), None);

        test_mock::set_caller(treasury);
        test_mock::set_value(149);
        assert_eq!(
            migrate_v3_auction_bid_custody(treasury.as_ptr(), nft.0.as_ptr(), 1),
            0
        );
        test_mock::set_value(0);
        test_mock::set_cross_call_response(Some(149u64.to_le_bytes().to_vec()));
        assert_eq!(
            migrate_v3_auction_bid_custody(treasury.as_ptr(), nft.0.as_ptr(), 1),
            0
        );
        test_mock::set_cross_call_response(Some(150u64.to_le_bytes().to_vec()));
        assert_eq!(
            migrate_v3_auction_bid_custody(treasury.as_ptr(), nft.0.as_ptr(), 1),
            1
        );
        assert_eq!(auction_bid_custody_ready(nft, 1), Some(true));
        test_mock::set_value(0);
        assert_eq!(
            migrate_v3_auction_bid_custody(treasury.as_ptr(), nft.0.as_ptr(), 1),
            1
        );

        test_mock::set_caller(seller);
        test_mock::set_cross_call_responses(vec![
            royalty_response(creator, 500),
            seller.to_vec(),
            1u32.to_le_bytes().to_vec(),
        ]);
        assert_eq!(
            migrate_auction_escrow(seller.as_ptr(), nft.0.as_ptr(), 1),
            1
        );
        let migrated = storage_get(&create_auction_key(nft, 1)).unwrap();
        assert_eq!(&migrated[..177], &legacy[..177]);
        assert_eq!(&migrated[177..209], &creator);
        assert_eq!(u16::from_le_bytes([migrated[209], migrated[210]]), 500);
        assert_eq!(auction_escrowed(nft, 1), Some(true));
        assert_eq!(
            snapshotted_fee_bps(&auction_fee_key(nft, 1)),
            Some(DEFAULT_MARKETPLACE_FEE_BPS)
        );

        test_mock::set_cross_call_responses(vec![
            royalty_response(creator, 500),
            get_contract_address().0.to_vec(),
        ]);
        assert_eq!(
            migrate_auction_escrow(seller.as_ptr(), nft.0.as_ptr(), 1),
            1
        );
    }

    #[test]
    fn malformed_accounting_blocks_purchase_before_external_mutation() {
        setup();
        let seller = [3u8; 32];
        let buyer = [6u8; 32];
        let nft = Address([4u8; 32]);
        let payment = Address([5u8; 32]);
        create_test_listing(&seller, &nft, 1, 1_000_000, &payment);
        storage_set(&platform_fee_key(payment), &[1u8, 2u8]);
        test_mock::set_caller(buyer);

        assert_eq!(buy_nft(buyer.as_ptr(), nft.0.as_ptr(), 1), 0);
        assert_eq!(test_mock::get_last_cross_call(), None);
        assert_eq!(storage_get(&create_listing_key(nft, 1)).unwrap()[144], 1);
    }

    #[test]
    fn traits_reject_duplicate_keys_and_oversized_payloads() {
        setup();
        let owner = [7u8; 32];
        let nft = [4u8; 32];
        let duplicate = [1u8, b'a', 1, b'x', 1, b'a', 1, b'y'];
        test_mock::set_caller(owner);
        test_mock::set_cross_call_response(Some(owner.to_vec()));
        assert_eq!(
            set_nft_attributes(
                owner.as_ptr(),
                nft.as_ptr(),
                1,
                1,
                1,
                duplicate.as_ptr(),
                duplicate.len() as u32,
            ),
            0
        );

        let oversized = vec![0u8; MAX_TRAITS_BYTES + 1];
        test_mock::set_cross_call_response(Some(owner.to_vec()));
        assert_eq!(
            set_nft_attributes(
                owner.as_ptr(),
                nft.as_ptr(),
                1,
                1,
                1,
                oversized.as_ptr(),
                oversized.len() as u32,
            ),
            0
        );
    }

    #[test]
    fn metrics_v3_migration_is_manifest_bound_exact_and_resumable() {
        test_mock::reset();
        test_mock::set_contract_address([0xA5; 32]);
        let admin = [1u8; 32];
        let treasury = [2u8; 32];
        let native = Address([0u8; 32]);
        let quote = Address([5u8; 32]);
        let manifest = [9u8; 32];
        storage_set(MARKETPLACE_OWNER_KEY, &admin);
        storage_set(MARKETPLACE_FEE_TREASURY_KEY, &treasury);
        storage_set(
            MARKETPLACE_FEE_KEY,
            &u64_to_bytes(DEFAULT_MARKETPLACE_FEE_BPS),
        );
        storage_set(MM_PAUSE_KEY, &[0u8]);
        test_mock::set_caller(admin);

        assert!(is_mm_paused());
        assert_eq!(get_marketplace_stats(), 1);
        assert_eq!(set_marketplace_fee(admin.as_ptr(), 300), 0);
        assert!(record_unpaid_payout(quote, Address(admin), 10));
        assert_eq!(claim_unpaid_payout(admin.as_ptr(), quote.0.as_ptr()), 0);
        assert_eq!(
            load_u64_or_zero(&unpaid_payout_key(quote, Address(admin))),
            Some(10)
        );
        assert_eq!(test_mock::get_last_cross_call(), None);
        assert_eq!(mm_unpause(admin.as_ptr()), 2);
        assert_eq!(begin_metrics_v3_migration(admin.as_ptr()), 0);
        assert_eq!(begin_metrics_v3_migration(admin.as_ptr()), 0);
        assert_eq!(
            seal_metrics_v3_manifest(admin.as_ptr(), manifest.as_ptr(), 2, 3, 100, 0, 1),
            4
        );
        assert_eq!(
            seal_metrics_v3_manifest(admin.as_ptr(), manifest.as_ptr(), 2, 3, 100, 0, 0),
            0
        );
        assert_eq!(migrate_metrics_v3_global(admin.as_ptr()), 0);
        assert_eq!(migrate_metrics_v3_global(admin.as_ptr()), 0);
        test_mock::set_cross_call_response(Some(1u64.to_le_bytes().to_vec()));
        assert_eq!(
            migrate_metrics_v3_token(admin.as_ptr(), native.0.as_ptr(), 1, 100, 2),
            5
        );
        test_mock::set_cross_call_response(Some(1_000u64.to_le_bytes().to_vec()));
        assert_eq!(
            migrate_metrics_v3_token(admin.as_ptr(), native.0.as_ptr(), 1, 100, 2),
            0
        );
        assert_eq!(
            migrate_metrics_v3_token(admin.as_ptr(), native.0.as_ptr(), 1, 100, 2),
            0
        );
        assert_eq!(complete_metrics_v3_migration(admin.as_ptr()), 3);
        assert_eq!(
            migrate_metrics_v3_token(admin.as_ptr(), quote.0.as_ptr(), 2, 1_000, 25),
            0
        );
        test_mock::set_cross_call_response(Some(1_000u64.to_le_bytes().to_vec()));
        assert_eq!(complete_metrics_v3_migration(admin.as_ptr()), 0);
        assert!(is_mm_paused());
        assert_eq!(mm_unpause(admin.as_ptr()), 0);
        assert!(!is_mm_paused());

        assert_eq!(get_marketplace_stats(), 0);
        let global = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&global[16..24]), 3);
        assert_eq!(bytes_to_u64(&global[24..32]), 100);
        assert_eq!(get_marketplace_token_stats(quote.0.as_ptr()), 0);
        let quote_stats = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&quote_stats[..8]), 2);
        assert_eq!(bytes_to_u64(&quote_stats[8..16]), 1_000);
        assert_eq!(bytes_to_u64(&quote_stats[16..24]), 25);
        assert_eq!(bytes_to_u64(&quote_stats[24..32]), 0);
        assert_eq!(get_marketplace_token_stats(native.0.as_ptr()), 0);
        let native_stats = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&native_stats[16..24]), 2);
        assert_eq!(bytes_to_u64(&native_stats[24..32]), 2);

        assert_eq!(get_metrics_v3_migration_status(), 0);
        let status = test_mock::get_return_data();
        assert_eq!(status.len(), 115);
        assert_eq!(bytes_to_u64(&status[..8]), MM_METRICS_VERSION);
        assert_eq!(status[8], 0);
        assert_eq!(status[9], 0);
        assert_eq!(status[10], 1);
        assert_eq!(bytes_to_u64(&status[11..19]), 2);
        assert_eq!(bytes_to_u64(&status[19..27]), 2);
        assert_eq!(bytes_to_u64(&status[27..35]), 3);
        assert_eq!(bytes_to_u64(&status[35..43]), 3);
        assert_eq!(bytes_to_u64(&status[43..51]), 100);
        assert_eq!(bytes_to_u64(&status[51..59]), 0);
        assert_eq!(bytes_to_u64(&status[59..67]), 0);
        assert_eq!(bytes_to_u64(&status[67..75]), 0);
        assert_eq!(bytes_to_u64(&status[75..83]), 0);
        assert_eq!(&status[83..115], &manifest);
    }

    #[test]
    fn sale_volume_is_exact_per_token_and_global_volume_is_native_only() {
        setup();
        let quote_token = Address([5u8; 32]);
        let native_token = Address([0u8; 32]);

        let quote = prepare_sale_accounting(quote_token, 25, 1_000).unwrap();
        commit_sale_accounting(quote_token, quote);
        assert_eq!(get_marketplace_token_stats(quote_token.0.as_ptr()), 0);
        let quote_stats = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&quote_stats[..8]), 1);
        assert_eq!(bytes_to_u64(&quote_stats[8..16]), 1_000);
        assert_eq!(bytes_to_u64(&quote_stats[16..24]), 25);
        assert_eq!(bytes_to_u64(&quote_stats[24..32]), 25);
        assert_eq!(load_u64_or_zero(MM_NATIVE_SALE_VOLUME_KEY), Some(0));

        let native = prepare_sale_accounting(native_token, 2, 100).unwrap();
        commit_sale_accounting(native_token, native);
        assert_eq!(load_u64_or_zero(MM_SALE_COUNT_KEY), Some(2));
        assert_eq!(load_u64_or_zero(MM_NATIVE_SALE_VOLUME_KEY), Some(100));
        assert_eq!(get_marketplace_token_stats(native_token.0.as_ptr()), 0);
        let native_stats = test_mock::get_return_data();
        assert_eq!(bytes_to_u64(&native_stats[..8]), 1);
        assert_eq!(bytes_to_u64(&native_stats[8..16]), 100);
        assert_eq!(bytes_to_u64(&native_stats[16..24]), 2);
        assert_eq!(bytes_to_u64(&native_stats[24..32]), 2);
    }
}
