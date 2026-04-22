use super::super::*;

pub(super) const MAX_BIP44_ACCOUNT_INDEX: u32 = 0x7FFF_FFFF;

pub(super) fn default_deposit_seed_source() -> String {
    "treasury_root".to_string()
}

pub(super) fn derive_deposit_address(
    chain: &str,
    asset: &str,
    path: &str,
    master_seed: &str,
) -> Result<String, String> {
    match (chain, asset) {
        ("sol", _) | ("solana", _) => derive_solana_address(path, master_seed),
        ("eth", _) | ("ethereum", _) | ("bsc", _) | ("bnb", _) => {
            derive_evm_address(path, master_seed)
        }
        _ => Err(format!("Unsupported chain: {}", chain)),
    }
}

/// F2-01: Map chain name to BIP-44 registered coin type integer.
/// See <https://github.com/satoshilabs/slips/blob/master/slip-0044.md>
pub(super) fn bip44_coin_type(chain: &str) -> Result<u32, String> {
    match chain {
        "sol" | "solana" => Ok(501),
        "eth" | "ethereum" | "bsc" | "bnb" => Ok(60),
        "btc" | "bitcoin" => Ok(0),
        "ltc" | "litecoin" => Ok(2),
        "lichen" | "licn" => Ok(9999),
        _ => Err(format!("Unknown coin type for chain: {}", chain)),
    }
}

pub(super) fn is_evm_chain(chain: &str) -> bool {
    matches!(chain, "eth" | "ethereum" | "bsc" | "bnb")
}

/// F2-01: Build BIP-44-structured derivation path.
/// Format: `m/44'/{coin_type}'/{account}'/0/{index}`
/// The account index comes from a durable per-user mapping persisted in custody state.
pub(super) fn bip44_derivation_path(
    chain: &str,
    account: u32,
    index: u64,
) -> Result<String, String> {
    let coin_type = super::bip44_coin_type(chain)?;
    if account > MAX_BIP44_ACCOUNT_INDEX {
        return Err("derivation account index exceeds BIP-44 hardened range".to_string());
    }
    Ok(format!("m/44'/{}'/{}'/{}/{}", coin_type, account, 0, index))
}

pub(super) fn derive_solana_owner_pubkey(path: &str, master_seed: &str) -> Result<String, String> {
    derive_solana_address(path, master_seed)
}

pub(super) fn active_deposit_seed_source(config: &CustodyConfig) -> &'static str {
    if config.deposit_master_seed == config.master_seed {
        DEPOSIT_SEED_SOURCE_TREASURY_ROOT
    } else {
        DEPOSIT_SEED_SOURCE_DEPOSIT_ROOT
    }
}

pub(super) fn deposit_seed_for_source<'a>(config: &'a CustodyConfig, source: &str) -> &'a str {
    if source == DEPOSIT_SEED_SOURCE_DEPOSIT_ROOT {
        &config.deposit_master_seed
    } else {
        &config.master_seed
    }
}

pub(super) fn deposit_seed_for_record<'a>(
    config: &'a CustodyConfig,
    deposit: &DepositRequest,
) -> &'a str {
    deposit_seed_for_source(config, &deposit.deposit_seed_source)
}
