use super::*;

pub(crate) fn load_config() -> CustodyConfig {
    let db_path = std::env::var("CUSTODY_DB_PATH").unwrap_or_else(|_| "./data/custody".to_string());
    let solana_rpc_url = std::env::var("CUSTODY_SOLANA_RPC_URL").ok();
    let evm_rpc_url = std::env::var("CUSTODY_EVM_RPC_URL").ok();
    let solana_confirmations = std::env::var("CUSTODY_SOLANA_CONFIRMATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    let evm_confirmations = std::env::var("CUSTODY_EVM_CONFIRMATIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(12);
    let poll_interval_secs = std::env::var("CUSTODY_POLL_INTERVAL_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(15);
    let treasury_solana_address = std::env::var("CUSTODY_TREASURY_SOLANA").ok();
    let treasury_evm_address = std::env::var("CUSTODY_TREASURY_EVM").ok();
    let treasury_eth_address = std::env::var("CUSTODY_TREASURY_ETH").ok();
    let treasury_bnb_address = std::env::var("CUSTODY_TREASURY_BNB").ok();
    let eth_rpc_url = std::env::var("CUSTODY_ETH_RPC_URL").ok();
    let bnb_rpc_url = std::env::var("CUSTODY_BNB_RPC_URL").ok();
    let solana_fee_payer_keypair_path = std::env::var("CUSTODY_SOLANA_FEE_PAYER").ok();
    let solana_treasury_owner = std::env::var("CUSTODY_SOLANA_TREASURY_OWNER")
        .ok()
        .or_else(|| treasury_solana_address.clone());
    let solana_usdc_mint = std::env::var("CUSTODY_SOLANA_USDC_MINT")
        .unwrap_or_else(|_| "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string());
    let solana_usdt_mint = std::env::var("CUSTODY_SOLANA_USDT_MINT")
        .unwrap_or_else(|_| "Es9vMFrzaCER3FXvxuauYhVNiVw9g8Y3V9D2n7sGdG8d".to_string());
    let evm_usdc_contract = std::env::var("CUSTODY_EVM_USDC")
        .unwrap_or_else(|_| "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".to_string());
    let evm_usdt_contract = std::env::var("CUSTODY_EVM_USDT")
        .unwrap_or_else(|_| "0xdAC17F958D2ee523a2206206994597C13D831ec7".to_string());
    let licn_rpc_url = std::env::var("CUSTODY_LICHEN_RPC_URL").ok();
    let treasury_keypair_path = std::env::var("CUSTODY_TREASURY_KEYPAIR").ok();
    let musd_contract_addr = std::env::var("CUSTODY_LUSD_TOKEN_ADDR").ok();
    let wsol_contract_addr = std::env::var("CUSTODY_WSOL_TOKEN_ADDR").ok();
    let weth_contract_addr = std::env::var("CUSTODY_WETH_TOKEN_ADDR").ok();
    let wbnb_contract_addr = std::env::var("CUSTODY_WBNB_TOKEN_ADDR").ok();
    let rebalance_threshold_bps = std::env::var("CUSTODY_REBALANCE_THRESHOLD_BPS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(7000);
    let rebalance_target_bps = std::env::var("CUSTODY_REBALANCE_TARGET_BPS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(5000);
    let rebalance_max_slippage_bps = std::env::var("CUSTODY_REBALANCE_MAX_SLIPPAGE_BPS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(50);
    let jupiter_api_url = std::env::var("CUSTODY_JUPITER_API_URL").ok();
    let uniswap_router = std::env::var("CUSTODY_UNISWAP_ROUTER").ok();
    let deposit_ttl_secs = std::env::var("CUSTODY_DEPOSIT_TTL_SECS")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(86_400);
    let pending_burn_ttl_secs = std::env::var("CUSTODY_WITHDRAWAL_PENDING_BURN_TTL_SECS")
        .ok()
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(86_400);
    let incident_status_path = std::env::var("LICHEN_INCIDENT_STATUS_FILE")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let master_seed =
        load_required_seed_secret("CUSTODY_MASTER_SEED_FILE", "CUSTODY_MASTER_SEED", true);
    let deposit_master_seed = load_optional_seed_secret(
        "CUSTODY_DEPOSIT_MASTER_SEED_FILE",
        "CUSTODY_DEPOSIT_MASTER_SEED",
    )
    .unwrap_or_else(|| master_seed.clone());
    let signer_endpoints = std::env::var("CUSTODY_SIGNER_ENDPOINTS")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(|entry| entry.trim().to_string())
                .filter(|entry| !entry.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let signer_threshold = std::env::var("CUSTODY_SIGNER_THRESHOLD")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_else(|| default_signer_threshold(signer_endpoints.len()));
    let signer_pq_addresses = std::env::var("CUSTODY_SIGNER_PQ_ADDRESSES")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(|entry| entry.trim())
                .filter(|entry| !entry.is_empty())
                .map(|entry| {
                    Pubkey::from_base58(entry).unwrap_or_else(|error| {
                        panic!(
                            "FATAL: invalid PQ signer address '{}' in CUSTODY_SIGNER_PQ_ADDRESSES: {}",
                            entry, error
                        )
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let withdrawal_velocity_policy = load_withdrawal_velocity_policy();
    let webhook_allowed_hosts = std::env::var("CUSTODY_WEBHOOK_ALLOWED_HOSTS")
        .ok()
        .map(|value| {
            value
                .split(',')
                .map(|entry| entry.trim().to_ascii_lowercase())
                .filter(|entry| !entry.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    CustodyConfig {
        db_path,
        solana_rpc_url,
        evm_rpc_url,
        eth_rpc_url,
        bnb_rpc_url,
        solana_confirmations,
        evm_confirmations,
        poll_interval_secs,
        treasury_solana_address,
        treasury_evm_address,
        treasury_eth_address,
        treasury_bnb_address,
        solana_fee_payer_keypair_path,
        solana_treasury_owner,
        solana_usdc_mint,
        solana_usdt_mint,
        evm_usdc_contract,
        evm_usdt_contract,
        signer_endpoints: signer_endpoints.clone(),
        signer_threshold,
        licn_rpc_url,
        treasury_keypair_path,
        musd_contract_addr,
        wsol_contract_addr,
        weth_contract_addr,
        wbnb_contract_addr,
        rebalance_threshold_bps,
        rebalance_target_bps,
        rebalance_max_slippage_bps,
        jupiter_api_url,
        uniswap_router,
        deposit_ttl_secs,
        pending_burn_ttl_secs,
        incident_status_path,
        master_seed,
        deposit_master_seed,
        signer_auth_token: {
            let env_token = std::env::var("CUSTODY_SIGNER_AUTH_TOKEN")
                .ok()
                .filter(|token| !token.is_empty());
            if env_token.is_some() {
                env_token
            } else if !signer_endpoints.is_empty() {
                panic!(
                    "FATAL: {} signer endpoint(s) configured but CUSTODY_SIGNER_AUTH_TOKEN \
                     is not set. Set it explicitly to enable signer authentication.",
                    signer_endpoints.len()
                );
            } else {
                None
            }
        },
        signer_auth_tokens: std::env::var("CUSTODY_SIGNER_AUTH_TOKENS")
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .map(|token| {
                        let token = token.trim();
                        if token.is_empty() {
                            None
                        } else {
                            Some(token.to_string())
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
        signer_pq_addresses,
        api_auth_token: {
            let token = std::env::var("CUSTODY_API_AUTH_TOKEN")
                .ok()
                .filter(|value| !value.is_empty());
            if token.is_none() {
                panic!(
                    "CRITICAL: CUSTODY_API_AUTH_TOKEN must be set and non-empty. \
                     The withdrawal endpoint is unauthenticated without it."
                );
            }
            token
        },
        withdrawal_velocity_policy,
        evm_multisig_address: std::env::var("CUSTODY_EVM_MULTISIG_ADDRESS").ok(),
        webhook_allowed_hosts,
    }
}
