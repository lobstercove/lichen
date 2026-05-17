use super::*;

#[cfg(test)]
pub(crate) const NEOX_MAINNET_CHAIN_ID: u64 = 47_763;
pub(crate) const NEOX_TESTNET_T4_CHAIN_ID: u64 = 12_227_332;

const ETH_ALIASES: &[&str] = &["ethereum", "eth"];
const BNB_ALIASES: &[&str] = &["bsc", "bnb"];
const NEOX_ALIASES: &[&str] = &["neox", "neo-x", "neo_x"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EvmRouteConfig {
    pub(crate) canonical_chain: &'static str,
    pub(crate) aliases: &'static [&'static str],
    pub(crate) native_asset: &'static str,
    pub(crate) chain_id: u64,
    pub(crate) rpc_url: Option<String>,
    pub(crate) confirmations: u64,
    pub(crate) treasury_address: Option<String>,
    pub(crate) treasury_derivation_path: &'static str,
    pub(crate) multisig_address: Option<String>,
}

pub(crate) fn canonical_evm_chain(chain: &str) -> Option<&'static str> {
    let chain = chain.trim().to_ascii_lowercase();
    match chain.as_str() {
        "ethereum" | "eth" => Some("ethereum"),
        "bsc" | "bnb" => Some("bsc"),
        "neox" | "neo-x" | "neo_x" => Some("neox"),
        _ => None,
    }
}

pub(crate) fn evm_chain_aliases(chain: &str) -> Option<&'static [&'static str]> {
    match canonical_evm_chain(chain)? {
        "ethereum" => Some(ETH_ALIASES),
        "bsc" => Some(BNB_ALIASES),
        "neox" => Some(NEOX_ALIASES),
        _ => None,
    }
}

pub(crate) fn is_supported_evm_chain(chain: &str) -> bool {
    canonical_evm_chain(chain).is_some()
}

pub(crate) fn evm_native_asset_for_chain(chain: &str) -> Option<&'static str> {
    match canonical_evm_chain(chain)? {
        "ethereum" => Some("eth"),
        "bsc" => Some("bnb"),
        "neox" => Some("gas"),
        _ => None,
    }
}

pub(crate) fn evm_treasury_derivation_path(chain: &str) -> Option<&'static str> {
    match canonical_evm_chain(chain)? {
        "ethereum" => Some("custody/treasury/ethereum"),
        "bsc" => Some("custody/treasury/bnb"),
        "neox" => Some("custody/treasury/neox"),
        _ => None,
    }
}

pub(crate) fn evm_route_for_chain(config: &CustodyConfig, chain: &str) -> Option<EvmRouteConfig> {
    let canonical = canonical_evm_chain(chain)?;
    let aliases = evm_chain_aliases(canonical)?;
    let native_asset = evm_native_asset_for_chain(canonical)?;
    let generic_rpc = || config.evm_rpc_url.clone();
    let generic_treasury = || config.treasury_evm_address.clone();
    let generic_safe = || config.evm_multisig_address.clone();

    match canonical {
        "ethereum" => Some(EvmRouteConfig {
            canonical_chain: "ethereum",
            aliases,
            native_asset,
            chain_id: 1,
            rpc_url: config.eth_rpc_url.clone().or_else(generic_rpc),
            confirmations: config.evm_confirmations,
            treasury_address: config
                .treasury_eth_address
                .clone()
                .or_else(generic_treasury),
            treasury_derivation_path: "custody/treasury/ethereum",
            multisig_address: generic_safe(),
        }),
        "bsc" => Some(EvmRouteConfig {
            canonical_chain: "bsc",
            aliases,
            native_asset,
            chain_id: 56,
            rpc_url: config.bnb_rpc_url.clone().or_else(generic_rpc),
            confirmations: config.evm_confirmations,
            treasury_address: config
                .treasury_bnb_address
                .clone()
                .or_else(generic_treasury),
            treasury_derivation_path: "custody/treasury/bnb",
            multisig_address: generic_safe(),
        }),
        "neox" => Some(EvmRouteConfig {
            canonical_chain: "neox",
            aliases,
            native_asset,
            chain_id: config.neox_chain_id,
            rpc_url: config.neox_rpc_url.clone(),
            confirmations: config.neox_confirmations,
            treasury_address: config.treasury_neox_address.clone(),
            treasury_derivation_path: "custody/treasury/neox",
            multisig_address: config
                .neox_multisig_address
                .clone()
                .or_else(|| config.evm_multisig_address.clone()),
        }),
        _ => None,
    }
}

pub(crate) fn configured_evm_routes(config: &CustodyConfig) -> Vec<EvmRouteConfig> {
    ["ethereum", "bsc", "neox"]
        .iter()
        .filter_map(|chain| evm_route_for_chain(config, chain))
        .filter(|route| route.rpc_url.is_some())
        .collect()
}

pub(crate) fn evm_route_confirmations(config: &CustodyConfig, chain: &str) -> Option<u64> {
    evm_route_for_chain(config, chain).map(|route| route.confirmations)
}
