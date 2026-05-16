use super::*;

/// Derive treasury addresses from the master seed for external chains.
/// Uses well-known derivation paths so addresses are deterministic and
/// recoverable from the master seed alone — no external keypair files needed.
pub(crate) fn derive_treasury_addresses_from_seed(config: &mut CustodyConfig) {
    let seed = &config.master_seed;

    if config.treasury_solana_address.is_none() {
        match derive_solana_address("custody/treasury/solana", seed) {
            Ok(addr) => {
                info!("derived Solana treasury from master seed: {}", addr);
                config.treasury_solana_address = Some(addr.clone());
                if config.solana_treasury_owner.is_none() {
                    config.solana_treasury_owner = Some(addr);
                }
            }
            Err(e) => tracing::warn!("failed to derive Solana treasury: {}", e),
        }
    }

    if config.treasury_eth_address.is_none() && config.treasury_evm_address.is_none() {
        match derive_evm_address("custody/treasury/ethereum", seed) {
            Ok(addr) => {
                info!("derived ETH treasury from master seed: {}", addr);
                config.treasury_eth_address = Some(addr);
            }
            Err(e) => tracing::warn!("failed to derive ETH treasury: {}", e),
        }
    }

    if config.treasury_bnb_address.is_none() && config.treasury_evm_address.is_none() {
        match derive_evm_address("custody/treasury/bnb", seed) {
            Ok(addr) => {
                info!("derived BNB treasury from master seed: {}", addr);
                config.treasury_bnb_address = Some(addr);
            }
            Err(e) => tracing::warn!("failed to derive BNB treasury: {}", e),
        }
    }

    if config.treasury_neox_address.is_none() {
        match derive_evm_address("custody/treasury/neox", seed) {
            Ok(addr) => {
                info!("derived Neo X treasury from master seed: {}", addr);
                config.treasury_neox_address = Some(addr);
            }
            Err(e) => tracing::warn!("failed to derive Neo X treasury: {}", e),
        }
    }
}

/// Resolve the RPC URL for a given chain. Per-chain URLs override the generic EVM URL.
pub(crate) fn rpc_url_for_chain(config: &CustodyConfig, chain: &str) -> Option<String> {
    match chain {
        "sol" | "solana" => config.solana_rpc_url.clone(),
        _ => evm_route_for_chain(config, chain).and_then(|route| route.rpc_url),
    }
}

/// Resolve the treasury address for a given chain. Per-chain overrides generic.
pub(crate) fn treasury_for_chain(config: &CustodyConfig, chain: &str) -> Option<String> {
    match chain {
        "sol" | "solana" => config.treasury_solana_address.clone(),
        _ => evm_route_for_chain(config, chain).and_then(|route| route.treasury_address),
    }
}
