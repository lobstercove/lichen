use super::*;

pub(crate) fn prepare_custody_config() -> CustodyConfig {
    let mut config = load_config();

    // Derive treasury addresses from master seed for external chains
    // (only fills in addresses not already set via env vars)
    derive_treasury_addresses_from_seed(&mut config);
    log_chain_configuration(&config);
    log_seed_mode(&config);

    if let Err(err) = validate_custody_security_configuration(&config) {
        panic!("FATAL: {}", err);
    }
    if let Err(err) = validate_pq_signer_configuration(&config) {
        panic!("FATAL: {}", err);
    }
    if config.signer_endpoints.len() > 1 {
        tracing::warn!(
            "MULTI-SIGNER MODE DETECTED ({}-of-{}). Deposit creation remains disabled while \
             sweeps rely on locally derived deposit keys, threshold Solana withdrawals are \
             hard-disabled until a real threshold executor exists, and only the EVM Safe path \
             supports multi-party treasury execution.",
            config.signer_threshold,
            config.signer_endpoints.len()
        );
        info!(
            "Multi-signer mode: {}-of-{} threshold (ML-DSA approvals for custody flows, packed ECDSA only for isolated EVM Safe execution)",
            config.signer_threshold,
            config.signer_endpoints.len()
        );
        info!(
            "  Loaded {} PQ signer address(es) for withdrawal approval verification",
            config.signer_pq_addresses.len()
        );
    }

    config
}

fn log_chain_configuration(config: &CustodyConfig) {
    info!("══════════════════════════════════════════════════════════════");
    info!("  Lichen Custody Service — Chain Configuration");
    info!("══════════════════════════════════════════════════════════════");
    info!("  Lichen RPC:   {:?}", config.licn_rpc_url);
    info!("  SOL RPC:         {:?}", config.solana_rpc_url);
    info!(
        "  ETH RPC:         {:?}",
        config.eth_rpc_url.as_ref().or(config.evm_rpc_url.as_ref())
    );
    info!(
        "  BNB RPC:         {:?}",
        config.bnb_rpc_url.as_ref().or(config.evm_rpc_url.as_ref())
    );
    info!("  SOL Treasury:    {:?}", config.treasury_solana_address);
    info!(
        "  ETH Treasury:    {:?}",
        config
            .treasury_eth_address
            .as_ref()
            .or(config.treasury_evm_address.as_ref())
    );
    info!(
        "  BNB Treasury:    {:?}",
        config
            .treasury_bnb_address
            .as_ref()
            .or(config.treasury_evm_address.as_ref())
    );

    if config.solana_rpc_url.is_some() {
        if let Some(path) = config.solana_fee_payer_keypair_path.as_ref() {
            info!("  SOL Fee Payer:   file={}", path);
        } else {
            match derive_solana_address("custody/fee-payer/solana", &config.master_seed) {
                Ok(address) => info!("  SOL Fee Payer:   {} (derived from master seed)", address),
                Err(error) => tracing::warn!("  SOL Fee Payer:   derivation failed: {}", error),
            }
        }
    }
    info!("══════════════════════════════════════════════════════════════");
}

fn log_seed_mode(config: &CustodyConfig) {
    if config.deposit_master_seed == config.master_seed {
        warn!(
            "🔐 INSECURE DEV SEED MODE: treasury and deposit keys share the same root because \
             CUSTODY_ALLOW_INSECURE_SEED=1. Never use this outside local development."
        );
    } else {
        info!(
            "🔐 Custody seed separation active: deposit addresses use \
             CUSTODY_DEPOSIT_MASTER_SEED and treasury execution uses CUSTODY_MASTER_SEED."
        );
    }
}
