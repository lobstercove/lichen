use axum::{
    http::{HeaderValue, Method},
    routing::{get, post},
    Router,
};
use reqwest::Client;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;

use super::{
    get_config, get_status, health, list_airdrops, load_airdrops,
    models::{
        FaucetConfig, FaucetState, DEFAULT_COOLDOWN_SECONDS, DEFAULT_DAILY_LIMIT_PER_IP,
        DEFAULT_MAX_PER_REQUEST, DEFAULT_PORT,
    },
    request_airdrop, RateLimiter,
};

pub(super) fn build_config() -> FaucetConfig {
    FaucetConfig {
        rpc_url: std::env::var("RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:8899".to_string()),
        network: std::env::var("NETWORK").unwrap_or_else(|_| "testnet".to_string()),
        max_per_request: parse_env_u64("MAX_PER_REQUEST", DEFAULT_MAX_PER_REQUEST),
        daily_limit_per_ip: parse_env_u64("DAILY_LIMIT_PER_IP", DEFAULT_DAILY_LIMIT_PER_IP),
        cooldown_seconds: parse_env_u64("COOLDOWN_SECONDS", DEFAULT_COOLDOWN_SECONDS),
        airdrops_file: std::env::var("AIRDROPS_FILE")
            .unwrap_or_else(|_| "airdrops.json".to_string()),
        trusted_proxies: parse_csv_env("TRUSTED_PROXY"),
    }
}

pub(super) fn validate_rpc_url(config: &FaucetConfig) -> Result<(), String> {
    if config.rpc_url.starts_with("http://") || config.rpc_url.starts_with("https://") {
        return Ok(());
    }

    Err("RPC_URL must start with http:// or https://".to_string())
}

pub(super) fn build_state(config: FaucetConfig) -> (FaucetState, usize, usize) {
    let airdrops = load_airdrops(&config.airdrops_file);

    let mut rate_limiter = RateLimiter::default();
    rate_limiter.restore_from_airdrops(&airdrops);
    let restored_addrs = rate_limiter.tracked_address_count();
    let restored_ips = rate_limiter.tracked_ip_count();

    let state = FaucetState {
        config,
        http: Client::builder().build().expect("reqwest client"),
        rate_limiter: Arc::new(RwLock::new(rate_limiter)),
        airdrops: Arc::new(RwLock::new(airdrops)),
    };

    (state, restored_addrs, restored_ips)
}

pub(super) fn build_app(state: FaucetState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/faucet/config", get(get_config))
        .route("/faucet/status", get(get_status))
        .route("/faucet/airdrops", get(list_airdrops))
        .route("/faucet/request", post(request_airdrop))
        .with_state(state)
        .layer(build_cors())
}

pub(super) fn faucet_listen_addr() -> SocketAddr {
    let port = parse_env_u16("PORT", DEFAULT_PORT);
    SocketAddr::from(([0, 0, 0, 0], port))
}

fn build_cors() -> CorsLayer {
    let mut origins: Vec<HeaderValue> = vec![
        "https://faucet.lichen.network"
            .parse::<HeaderValue>()
            .unwrap(),
        "https://lichen.network".parse::<HeaderValue>().unwrap(),
        "https://lichen-network-faucet.pages.dev"
            .parse::<HeaderValue>()
            .unwrap(),
    ];

    if std::env::var("DEV_CORS").is_ok() {
        origins.extend([
            "http://localhost:3000".parse::<HeaderValue>().unwrap(),
            "http://localhost:3003".parse::<HeaderValue>().unwrap(),
            "http://localhost:9100".parse::<HeaderValue>().unwrap(),
            "http://localhost:9101".parse::<HeaderValue>().unwrap(),
        ]);
    }

    CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([axum::http::header::CONTENT_TYPE])
        .allow_origin(origins)
}

fn parse_env_u16(key: &str, default: u16) -> u16 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(default)
}

fn parse_env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

fn parse_csv_env(key: &str) -> Vec<String> {
    std::env::var(key)
        .unwrap_or_default()
        .split(',')
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}
