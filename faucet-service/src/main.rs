use axum::{
    extract::{ConnectInfo, Json, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use lichen_core::Pubkey;
use serde_json::json;
use std::net::SocketAddr;
use tracing::info;

mod bootstrap;
mod http_support;
mod models;
mod rate_limit;
mod rpc_support;
mod storage;
#[cfg(test)]
mod tests;

use bootstrap::{build_app, build_config, build_state, faucet_listen_addr, validate_rpc_url};
use http_support::{error_json, error_response, extract_client_ip, now_ms};
use models::{
    AirdropQuery, AirdropRecord, FaucetPublicConfig, FaucetRequest, FaucetResponse, FaucetState,
    FaucetStatusResponse, SPORES_PER_LICN,
};
use rate_limit::RateLimiter;
use rpc_support::{fetch_treasury_info, rpc_call};
use storage::{load_airdrops, save_airdrops};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = build_config();

    if let Err(err) = validate_rpc_url(&config) {
        eprintln!("ERROR: {}", err);
        std::process::exit(1);
    }

    if config.network == "mainnet" {
        panic!("❌ Faucet cannot run on mainnet!");
    }

    let (state, restored_addrs, restored_ips) = build_state(config);

    info!(
        "Restored rate-limiter: {} addresses, {} IPs from airdrop history",
        restored_addrs, restored_ips
    );

    let app = build_app(state);

    let addr = faucet_listen_addr();
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind faucet listener");
    info!("lichen-faucet listening on {}", addr);
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .expect("serve faucet");
}

async fn health() -> &'static str {
    "OK"
}

async fn get_config(State(state): State<FaucetState>) -> Json<FaucetPublicConfig> {
    Json(FaucetPublicConfig {
        max_per_request: state.config.max_per_request,
        daily_limit_per_ip: state.config.daily_limit_per_ip,
        cooldown_seconds: state.config.cooldown_seconds,
        network: state.config.network.clone(),
    })
}

async fn get_status(State(state): State<FaucetState>) -> Response {
    match fetch_treasury_info(&state).await {
        Ok(info) => Json(FaucetStatusResponse {
            network: state.config.network.clone(),
            faucet_address: info.treasury_pubkey.unwrap_or_default(),
            balance_spores: info.treasury_balance,
            balance_licn: info.treasury_balance / SPORES_PER_LICN,
        })
        .into_response(),
        Err(err) => error_response(StatusCode::BAD_GATEWAY, &err),
    }
}

async fn list_airdrops(
    State(state): State<FaucetState>,
    Query(query): Query<AirdropQuery>,
) -> Json<Vec<AirdropRecord>> {
    let limit = query.limit.unwrap_or(10).min(100);
    let airdrops = state.airdrops.read().await;
    let mut records = airdrops.clone();
    records.sort_by_key(|record| std::cmp::Reverse(record.timestamp_ms));
    records.truncate(limit);
    Json(records)
}

async fn request_airdrop(
    State(state): State<FaucetState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(request): Json<FaucetRequest>,
) -> Response {
    let amount_licn = request.amount.unwrap_or(state.config.max_per_request);
    if amount_licn == 0 || amount_licn > state.config.max_per_request {
        return error_json(
            StatusCode::BAD_REQUEST,
            "Requested amount exceeds faucet limit",
        );
    }

    if Pubkey::from_base58(request.address.trim()).is_err() {
        return error_json(StatusCode::BAD_REQUEST, "Invalid recipient address");
    }

    let now_ms = now_ms();
    let client_ip = extract_client_ip(&headers, peer_addr, &state.config.trusted_proxies);
    let recipient = request.address.trim().to_string();

    let reservation = {
        let mut limiter = state.rate_limiter.write().await;
        match limiter.reserve(
            &client_ip,
            &recipient,
            now_ms,
            amount_licn,
            state.config.daily_limit_per_ip,
            state.config.cooldown_seconds,
        ) {
            Ok(reservation) => reservation,
            Err(err) => return error_json(StatusCode::TOO_MANY_REQUESTS, &err),
        }
    };

    let treasury = match fetch_treasury_info(&state).await {
        Ok(info) => info,
        Err(err) => {
            let mut limiter = state.rate_limiter.write().await;
            limiter.rollback(&reservation);
            return error_response(StatusCode::BAD_GATEWAY, &err);
        }
    };

    let required_spores = amount_licn.saturating_mul(SPORES_PER_LICN);
    if treasury.treasury_balance < required_spores {
        let mut limiter = state.rate_limiter.write().await;
        limiter.rollback(&reservation);
        return error_json(
            StatusCode::SERVICE_UNAVAILABLE,
            "Faucet temporarily empty - check back soon",
        );
    }

    let rpc_result = match rpc_call(
        &state,
        "requestAirdrop",
        json!([request.address.trim(), amount_licn]),
    )
    .await
    {
        Ok(value) => value,
        Err(err) => {
            let mut limiter = state.rate_limiter.write().await;
            limiter.rollback(&reservation);
            return error_response(StatusCode::BAD_GATEWAY, &err);
        }
    };

    let response = FaucetResponse {
        success: true,
        signature: rpc_result
            .get("signature")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string()),
        amount: rpc_result
            .get("amount")
            .and_then(|value| value.as_u64())
            .or(Some(amount_licn)),
        recipient: Some(recipient.clone()),
        message: rpc_result
            .get("message")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .or(Some(format!(
                "{} LICN airdropped successfully",
                amount_licn
            ))),
        error: None,
    };

    let mut airdrops = state.airdrops.write().await;
    airdrops.push(AirdropRecord {
        signature: response.signature.clone(),
        recipient,
        amount_licn,
        timestamp_ms: now_ms,
        ip: Some(client_ip),
    });
    if let Err(err) = save_airdrops(&state.config.airdrops_file, &airdrops) {
        tracing::error!("failed to persist faucet history: {}", err);
    }
    drop(airdrops);

    Json(response).into_response()
}
