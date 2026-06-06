use super::*;

pub(super) async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

fn push_missing(missing: &mut Vec<&'static str>, configured: bool, env_name: &'static str) {
    if !configured {
        missing.push(env_name);
    }
}

fn bridge_route_status(
    chain: &str,
    asset: &str,
    network: Option<String>,
    missing_config: Vec<&'static str>,
) -> Value {
    let configured = missing_config.is_empty();
    let mut payload = json!({
        "chain": chain,
        "asset": asset,
        "route": format!("{}:{}", chain, asset),
        "configured": configured,
        "deposit_ready": configured,
        "missing_config": missing_config,
    });
    if let Some(network) = network {
        payload["source_network"] = json!(network);
    }
    payload
}

fn bridge_route_readiness(config: &CustodyConfig) -> Value {
    let mut routes = BTreeMap::new();

    let mut sol_missing = Vec::new();
    push_missing(
        &mut sol_missing,
        config.solana_rpc_url.is_some(),
        "CUSTODY_SOLANA_RPC_URL",
    );
    push_missing(
        &mut sol_missing,
        config.wsol_contract_addr.is_some(),
        "CUSTODY_WSOL_TOKEN_ADDR",
    );
    routes.insert(
        "solana:sol".to_string(),
        bridge_route_status("solana", "sol", None, sol_missing),
    );

    for (asset, mint_env, mint_configured) in [
        (
            "usdc",
            "CUSTODY_SOLANA_USDC_MINT",
            !config.solana_usdc_mint.trim().is_empty(),
        ),
        (
            "usdt",
            "CUSTODY_SOLANA_USDT_MINT",
            !config.solana_usdt_mint.trim().is_empty(),
        ),
    ] {
        let mut missing = Vec::new();
        push_missing(
            &mut missing,
            config.solana_rpc_url.is_some(),
            "CUSTODY_SOLANA_RPC_URL",
        );
        push_missing(&mut missing, mint_configured, mint_env);
        push_missing(
            &mut missing,
            config.musd_contract_addr.is_some(),
            "CUSTODY_LUSD_TOKEN_ADDR",
        );
        routes.insert(
            format!("solana:{}", asset),
            bridge_route_status("solana", asset, None, missing),
        );
    }

    let mut eth_missing = Vec::new();
    push_missing(
        &mut eth_missing,
        rpc_url_for_chain(config, "ethereum").is_some(),
        "CUSTODY_ETH_RPC_URL",
    );
    push_missing(
        &mut eth_missing,
        config.weth_contract_addr.is_some(),
        "CUSTODY_WETH_TOKEN_ADDR",
    );
    routes.insert(
        "ethereum:eth".to_string(),
        bridge_route_status(
            "ethereum",
            "eth",
            Some(config.eth_chain_id.to_string()),
            eth_missing,
        ),
    );

    for (asset, contract_env, contract_configured) in [
        (
            "usdc",
            "CUSTODY_ETH_USDC_TOKEN_ADDR",
            !config.evm_usdc_contract.trim().is_empty(),
        ),
        (
            "usdt",
            "CUSTODY_ETH_USDT_TOKEN_ADDR",
            !config.evm_usdt_contract.trim().is_empty(),
        ),
    ] {
        let mut missing = Vec::new();
        push_missing(
            &mut missing,
            rpc_url_for_chain(config, "ethereum").is_some(),
            "CUSTODY_ETH_RPC_URL",
        );
        push_missing(&mut missing, contract_configured, contract_env);
        push_missing(
            &mut missing,
            config.musd_contract_addr.is_some(),
            "CUSTODY_LUSD_TOKEN_ADDR",
        );
        routes.insert(
            format!("ethereum:{}", asset),
            bridge_route_status(
                "ethereum",
                asset,
                Some(config.eth_chain_id.to_string()),
                missing,
            ),
        );
    }

    let mut bnb_missing = Vec::new();
    push_missing(
        &mut bnb_missing,
        rpc_url_for_chain(config, "bnb").is_some(),
        "CUSTODY_BNB_RPC_URL",
    );
    push_missing(
        &mut bnb_missing,
        config.wbnb_contract_addr.is_some(),
        "CUSTODY_WBNB_TOKEN_ADDR",
    );
    routes.insert(
        "bnb:bnb".to_string(),
        bridge_route_status(
            "bnb",
            "bnb",
            Some(config.bnb_chain_id.to_string()),
            bnb_missing.clone(),
        ),
    );
    routes.insert(
        "bsc:bnb".to_string(),
        bridge_route_status(
            "bsc",
            "bnb",
            Some(config.bnb_chain_id.to_string()),
            bnb_missing,
        ),
    );

    for (asset, contract_env, contract_configured) in [
        (
            "usdc",
            "CUSTODY_BSC_USDC_TOKEN_ADDR",
            config.bnb_usdc_contract.is_some(),
        ),
        (
            "usdt",
            "CUSTODY_BSC_USDT_TOKEN_ADDR",
            config.bnb_usdt_contract.is_some(),
        ),
    ] {
        let mut missing = Vec::new();
        push_missing(
            &mut missing,
            rpc_url_for_chain(config, "bnb").is_some(),
            "CUSTODY_BNB_RPC_URL",
        );
        push_missing(&mut missing, contract_configured, contract_env);
        push_missing(
            &mut missing,
            config.musd_contract_addr.is_some(),
            "CUSTODY_LUSD_TOKEN_ADDR",
        );
        routes.insert(
            format!("bnb:{}", asset),
            bridge_route_status(
                "bnb",
                asset,
                Some(config.bnb_chain_id.to_string()),
                missing.clone(),
            ),
        );
        routes.insert(
            format!("bsc:{}", asset),
            bridge_route_status("bsc", asset, Some(config.bnb_chain_id.to_string()), missing),
        );
    }

    let mut gas_missing = Vec::new();
    push_missing(
        &mut gas_missing,
        rpc_url_for_chain(config, "neox").is_some(),
        "CUSTODY_NEOX_RPC_URL",
    );
    push_missing(
        &mut gas_missing,
        config.wgas_contract_addr.is_some(),
        "CUSTODY_WGAS_TOKEN_ADDR",
    );
    routes.insert(
        "neox:gas".to_string(),
        bridge_route_status(
            "neox",
            "gas",
            Some(config.neox_chain_id.to_string()),
            gas_missing,
        ),
    );

    let mut neo_missing = Vec::new();
    push_missing(
        &mut neo_missing,
        rpc_url_for_chain(config, "neox").is_some(),
        "CUSTODY_NEOX_RPC_URL",
    );
    push_missing(
        &mut neo_missing,
        config.neox_neo_token_contract.is_some(),
        "CUSTODY_NEOX_NEO_TOKEN_ADDR",
    );
    push_missing(
        &mut neo_missing,
        config.wneo_contract_addr.is_some(),
        "CUSTODY_WNEO_TOKEN_ADDR",
    );
    routes.insert(
        "neox:neo".to_string(),
        bridge_route_status(
            "neox",
            "neo",
            Some(config.neox_chain_id.to_string()),
            neo_missing,
        ),
    );

    let mut btc_missing = Vec::new();
    push_missing(
        &mut btc_missing,
        config.btc_rpc_url.is_some(),
        "CUSTODY_BTC_RPC_URL",
    );
    push_missing(
        &mut btc_missing,
        config.wbtc_contract_addr.is_some(),
        "CUSTODY_WBTC_TOKEN_ADDR",
    );
    routes.insert(
        "bitcoin:btc".to_string(),
        bridge_route_status(
            "bitcoin",
            "btc",
            Some(config.btc_network.clone()),
            btc_missing,
        ),
    );

    json!(routes)
}

pub(super) async fn status(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    let sweep_counts =
        count_sweep_jobs(&state.db).map_err(|error| Json(ErrorResponse::db(&error)))?;
    let credit_counts =
        count_credit_jobs(&state.db).map_err(|error| Json(ErrorResponse::db(&error)))?;
    let withdrawal_counts =
        count_withdrawal_jobs(&state.db).map_err(|error| Json(ErrorResponse::db(&error)))?;

    Ok(Json(json!({
        "signers": {
            "configured": state.config.signer_endpoints.len(),
            "threshold": state.config.signer_threshold,
        },
        "sweeps": sweep_counts,
        "credits": credit_counts,
        "withdrawals": withdrawal_counts,
        "routes": bridge_route_readiness(&state.config),
    })))
}

pub(super) async fn get_reserves(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    Ok(build_reserve_ledger_response(&state.db))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_route_readiness_reports_btc_rpc_missing() {
        let mut config = crate::test_support::test_config();
        config.wbtc_contract_addr = Some("WBTC111111111111111111111111111111111111".to_string());

        let routes = bridge_route_readiness(&config);
        let btc = &routes["bitcoin:btc"];

        assert_eq!(btc["configured"], json!(false));
        assert_eq!(btc["deposit_ready"], json!(false));
        assert_eq!(btc["missing_config"], json!(["CUSTODY_BTC_RPC_URL"]));
        assert_eq!(btc["source_network"], json!("mainnet"));
    }

    #[test]
    fn bridge_route_readiness_keeps_existing_stablecoin_and_bsc_alias_routes() {
        let mut config = crate::test_support::test_config();
        config.musd_contract_addr = Some("LUSD111111111111111111111111111111111111".to_string());
        config.bnb_rpc_url = Some("http://127.0.0.1:8546".to_string());
        config.bnb_usdc_contract = Some("0x4444444444444444444444444444444444444444".to_string());
        config.bnb_usdt_contract = Some("0x5555555555555555555555555555555555555555".to_string());
        config.wbnb_contract_addr = Some("WBNB111111111111111111111111111111111111".to_string());

        let routes = bridge_route_readiness(&config);

        for key in [
            "solana:usdc",
            "solana:usdt",
            "ethereum:usdc",
            "ethereum:usdt",
            "bnb:bnb",
            "bsc:bnb",
            "bnb:usdc",
            "bsc:usdc",
            "bnb:usdt",
            "bsc:usdt",
        ] {
            assert_eq!(routes[key]["configured"], json!(true), "route {}", key);
            assert_eq!(routes[key]["deposit_ready"], json!(true), "route {}", key);
        }
    }
}
