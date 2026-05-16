use super::super::*;

fn configured_lichen_rpc_url(config: &CustodyConfig) -> Option<&str> {
    config
        .licn_rpc_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn resolve_deposit_contract(
    config: &CustodyConfig,
    chain: &str,
    asset: &str,
) -> Result<String, String> {
    resolve_token_contract(config, chain, asset).ok_or_else(|| {
        format!(
            "unsupported deposit asset for custody restriction check: chain={} asset={}",
            chain, asset
        )
    })
}

fn resolve_withdrawal_contract(config: &CustodyConfig, asset: &str) -> Result<String, String> {
    match asset {
        "musd" => config.musd_contract_addr.clone(),
        "wsol" => config.wsol_contract_addr.clone(),
        "weth" => config.weth_contract_addr.clone(),
        "wbnb" => config.wbnb_contract_addr.clone(),
        "wgas" => config.wgas_contract_addr.clone(),
        "wneo" if config.neox_neo_token_contract.is_some() => config.wneo_contract_addr.clone(),
        _ => None,
    }
    .ok_or_else(|| {
        format!(
            "unsupported withdrawal asset for custody restriction check: asset={}",
            asset
        )
    })
}

fn withdrawal_route_asset(asset: &str, preferred_stablecoin: &str) -> Result<String, String> {
    match asset {
        "musd" => {
            let preferred = preferred_stablecoin.trim().to_ascii_lowercase();
            if preferred == "usdt" || preferred == "usdc" {
                Ok(preferred)
            } else {
                Err(format!(
                    "unsupported mUSD withdrawal preferred stablecoin for custody restriction check: {}",
                    preferred_stablecoin
                ))
            }
        }
        "wsol" => Ok("sol".to_string()),
        "weth" => Ok("eth".to_string()),
        "wbnb" => Ok("bnb".to_string()),
        "wgas" => Ok("gas".to_string()),
        _ => Err(format!(
            "unsupported withdrawal asset for custody route restriction check: {}",
            asset
        )),
    }
}

fn restriction_ids_text(result: &Value) -> String {
    let ids = result
        .get("active_restriction_ids")
        .and_then(|value| value.as_array())
        .map(|ids| {
            ids.iter()
                .filter_map(|id| id.as_u64().map(|id| id.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if ids.is_empty() {
        "unknown".to_string()
    } else {
        ids.join(",")
    }
}

async fn query_lichen_restriction(
    state: &CustodyState,
    method: &str,
    params: Value,
) -> Result<Option<Value>, String> {
    let Some(rpc_url) = configured_lichen_rpc_url(&state.config) else {
        tracing::warn!(
            "custody consensus restriction check skipped for {}: CUSTODY_LICHEN_RPC_URL is not configured",
            method
        );
        return Ok(None);
    };

    licn_rpc_call(&state.http, rpc_url, method, params)
        .await
        .map(Some)
        .map_err(|error| format!("custody consensus restriction check failed: {}", error))
}

async fn ensure_bridge_route_not_paused(
    state: &CustodyState,
    chain: &str,
    asset: &str,
    operation: &str,
) -> Result<(), String> {
    let Some(result) = query_lichen_restriction(
        state,
        "getBridgeRouteRestrictionStatus",
        json!([{ "chain": chain, "asset": asset }]),
    )
    .await?
    else {
        return Ok(());
    };

    let route_paused = result
        .get("route_paused")
        .or_else(|| result.get("paused"))
        .and_then(|value| value.as_bool())
        .ok_or_else(|| {
            format!(
                "{} rejected: malformed restriction RPC response for bridge route {}:{}",
                operation, chain, asset
            )
        })?;

    if route_paused {
        return Err(format!(
            "{} rejected: bridge route {}:{} is paused by active RoutePaused restriction {}",
            operation,
            chain,
            asset,
            restriction_ids_text(&result)
        ));
    }

    Ok(())
}

async fn ensure_account_can_receive(
    state: &CustodyState,
    account: &str,
    asset_contract: &str,
    amount_spores: u64,
    operation: &str,
) -> Result<(), String> {
    let Some(result) = query_lichen_restriction(
        state,
        "canReceive",
        json!([{
            "account": account,
            "asset": asset_contract,
            "amount": amount_spores
        }]),
    )
    .await?
    else {
        return Ok(());
    };

    match result.get("allowed").and_then(|value| value.as_bool()) {
        Some(false) => {
            return Err(format!(
                "{} rejected: account {} cannot receive asset {} because active restriction {} applies",
                operation,
                account,
                asset_contract,
                restriction_ids_text(&result)
            ));
        }
        Some(true) => {}
        None => {
            return Err(format!(
                "{} rejected: malformed restriction RPC response for canReceive account {} asset {}",
                operation, account, asset_contract
            ));
        }
    }

    Ok(())
}

async fn ensure_account_can_send(
    state: &CustodyState,
    account: &str,
    asset_contract: &str,
    amount_spores: u64,
    operation: &str,
) -> Result<(), String> {
    let Some(result) = query_lichen_restriction(
        state,
        "canSend",
        json!([{
            "account": account,
            "asset": asset_contract,
            "amount": amount_spores
        }]),
    )
    .await?
    else {
        return Ok(());
    };

    match result.get("allowed").and_then(|value| value.as_bool()) {
        Some(false) => {
            return Err(format!(
                "{} rejected: account {} cannot send asset {} amount {} because active restriction {} applies",
                operation,
                account,
                asset_contract,
                amount_spores,
                restriction_ids_text(&result)
            ));
        }
        Some(true) => {}
        None => {
            return Err(format!(
                "{} rejected: malformed restriction RPC response for canSend account {} asset {}",
                operation, account, asset_contract
            ));
        }
    }

    Ok(())
}

pub(super) async fn ensure_deposit_restrictions_allow(
    state: &CustodyState,
    user_id: &str,
    chain: &str,
    asset: &str,
) -> Result<(), String> {
    if configured_lichen_rpc_url(&state.config).is_none() {
        return Ok(());
    }

    let asset_contract = resolve_deposit_contract(&state.config, chain, asset)?;
    ensure_bridge_route_not_paused(state, chain, asset, "custody deposit").await?;
    ensure_account_can_receive(state, user_id, &asset_contract, 0, "custody deposit").await
}

pub(super) async fn ensure_credit_restrictions_allow(
    state: &CustodyState,
    user_id: &str,
    chain: &str,
    asset: &str,
    amount_spores: u64,
) -> Result<(), String> {
    if configured_lichen_rpc_url(&state.config).is_none() {
        return Ok(());
    }

    let asset_contract = resolve_deposit_contract(&state.config, chain, asset)?;
    ensure_bridge_route_not_paused(state, chain, asset, "custody credit").await?;
    ensure_account_can_receive(
        state,
        user_id,
        &asset_contract,
        amount_spores,
        "custody credit",
    )
    .await
}

pub(super) async fn ensure_withdrawal_restrictions_allow(
    state: &CustodyState,
    user_id: &str,
    asset: &str,
    amount_spores: u64,
    dest_chain: &str,
    preferred_stablecoin: &str,
) -> Result<(), String> {
    if configured_lichen_rpc_url(&state.config).is_none() {
        return Ok(());
    }

    let asset_contract = resolve_withdrawal_contract(&state.config, asset)?;
    let route_asset = withdrawal_route_asset(asset, preferred_stablecoin)?;
    ensure_bridge_route_not_paused(state, dest_chain, &route_asset, "custody withdrawal").await?;
    ensure_account_can_send(
        state,
        user_id,
        &asset_contract,
        amount_spores,
        "custody withdrawal",
    )
    .await
}
