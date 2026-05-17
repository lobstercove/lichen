use super::*;

mod accounts;
mod cursor;
mod path;

pub(super) fn active_deposit_seed_source(config: &CustodyConfig) -> &'static str {
    path::active_deposit_seed_source(config)
}

pub(super) fn bip44_coin_type(chain: &str) -> Result<u32, String> {
    path::bip44_coin_type(chain)
}

#[cfg(test)]
pub(super) fn bip44_derivation_path(
    chain: &str,
    account: u32,
    index: u64,
) -> Result<String, String> {
    path::bip44_derivation_path(chain, account, index)
}

pub(super) fn bip44_derivation_path_for_config(
    config: &CustodyConfig,
    chain: &str,
    account: u32,
    index: u64,
) -> Result<String, String> {
    path::bip44_derivation_path_for_config(config, chain, account, index)
}

pub(super) fn default_deposit_seed_source() -> String {
    path::default_deposit_seed_source()
}

pub(super) fn deposit_seed_for_record<'a>(
    config: &'a CustodyConfig,
    deposit: &DepositRequest,
) -> &'a str {
    path::deposit_seed_for_record(config, deposit)
}

pub(super) fn deposit_seed_for_source<'a>(config: &'a CustodyConfig, source: &str) -> &'a str {
    path::deposit_seed_for_source(config, source)
}

pub(super) fn derive_deposit_address(
    chain: &str,
    asset: &str,
    path: &str,
    master_seed: &str,
) -> Result<String, String> {
    self::path::derive_deposit_address(chain, asset, path, master_seed)
}

pub(super) fn derive_solana_owner_pubkey(path: &str, master_seed: &str) -> Result<String, String> {
    self::path::derive_solana_owner_pubkey(path, master_seed)
}

pub(super) fn get_last_u64_index(db: &DB, key: &str) -> Result<Option<u64>, String> {
    cursor::get_last_u64_index(db, key)
}

pub(super) fn get_or_allocate_derivation_account(db: &DB, user_id: &str) -> Result<u32, String> {
    accounts::get_or_allocate_derivation_account(db, user_id)
}

pub(super) fn is_evm_chain(chain: &str) -> bool {
    path::is_evm_chain(chain)
}

pub(super) fn next_deposit_index(
    db: &DB,
    user_id: &str,
    chain: &str,
    asset: &str,
) -> Result<u64, String> {
    cursor::next_deposit_index(db, user_id, chain, asset)
}

pub(super) fn set_last_u64_index(db: &DB, key: &str, value: u64) -> Result<(), String> {
    cursor::set_last_u64_index(db, key, value)
}
