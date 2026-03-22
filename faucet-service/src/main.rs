use axum::{
    extract::{ConnectInfo, Json, Query, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use moltchain_core::Pubkey;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    fs,
    net::SocketAddr,
    path::Path,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tracing::{error, info};

const SHELLS_PER_MOLT: u64 = 1_000_000_000;
const DEFAULT_PORT: u16 = 9100;
const DEFAULT_MAX_PER_REQUEST: u64 = 10;
const DEFAULT_DAILY_LIMIT_PER_IP: u64 = 150;
const DEFAULT_COOLDOWN_SECONDS: u64 = 60;

#[derive(Debug, Deserialize)]
struct FaucetRequest {
    address: String,
    #[serde(default)]
    amount: Option<u64>,
}

#[derive(Debug, Serialize)]
struct FaucetResponse {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    amount: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    recipient: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct FaucetPublicConfig {
    max_per_request: u64,
    daily_limit_per_ip: u64,
    cooldown_seconds: u64,
    network: String,
}

#[derive(Debug, Serialize)]
struct FaucetStatusResponse {
    network: String,
    faucet_address: String,
    balance_shells: u64,
    balance_molt: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AirdropRecord {
    signature: Option<String>,
    recipient: String,
    amount_molt: u64,
    timestamp_ms: u64,
}

#[derive(Debug, Default)]
struct RateLimiter {
    by_ip: HashMap<String, Vec<(u64, u64)>>,
    // AUDIT-FIX M-24: Track per-recipient-address to prevent griefing a single address
    by_address: HashMap<String, Vec<(u64, u64)>>,
}

impl RateLimiter {
    fn prune(&mut self, now_ms: u64) {
        let cutoff = now_ms.saturating_sub(24 * 60 * 60 * 1000);
        self.by_ip.retain(|_, entries| {
            entries.retain(|(ts, _)| *ts >= cutoff);
            !entries.is_empty()
        });
        self.by_address.retain(|_, entries| {
            entries.retain(|(ts, _)| *ts >= cutoff);
            !entries.is_empty()
        });
    }

    fn check(
        &mut self,
        ip: &str,
        address: &str,
        now_ms: u64,
        daily_limit_molt: u64,
        cooldown_seconds: u64,
    ) -> Result<(), String> {
        self.prune(now_ms);
        let entries = self.by_ip.entry(ip.to_string()).or_default();
        if let Some((last_ts, _)) = entries.last().copied() {
            let elapsed = now_ms.saturating_sub(last_ts) / 1000;
            if elapsed < cooldown_seconds {
                let remaining = cooldown_seconds - elapsed;
                return Err(format!("Rate limit: try again in {} seconds", remaining));
            }
        }

        let used_today: u64 = entries.iter().map(|(_, amt)| *amt).sum();
        if used_today >= daily_limit_molt {
            return Err("Daily faucet limit reached for this IP".to_string());
        }

        // AUDIT-FIX M-24: Also check per-address daily limit
        let addr_entries = self.by_address.entry(address.to_string()).or_default();
        let addr_used: u64 = addr_entries.iter().map(|(_, amt)| *amt).sum();
        if addr_used >= daily_limit_molt {
            return Err("Daily faucet limit reached for this address".to_string());
        }

        Ok(())
    }

    fn record(&mut self, ip: &str, address: &str, now_ms: u64, amount_molt: u64) {
        self.by_ip
            .entry(ip.to_string())
            .or_default()
            .push((now_ms, amount_molt));
        self.by_address
            .entry(address.to_string())
            .or_default()
            .push((now_ms, amount_molt));
        self.prune(now_ms);
    }
}

#[derive(Clone)]
struct FaucetConfig {
    rpc_url: String,
    network: String,
    max_per_request: u64,
    daily_limit_per_ip: u64,
    cooldown_seconds: u64,
    airdrops_file: String,
    trusted_proxies: Vec<String>,
}

#[derive(Clone)]
struct FaucetState {
    config: FaucetConfig,
    http: Client,
    rate_limiter: Arc<RwLock<RateLimiter>>,
    airdrops: Arc<RwLock<Vec<AirdropRecord>>>,
}

#[derive(Debug, Deserialize)]
struct AirdropQuery {
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct TreasuryInfo {
    #[serde(default)]
    treasury_pubkey: Option<String>,
    #[serde(default)]
    treasury_balance: u64,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = FaucetConfig {
        rpc_url: std::env::var("RPC_URL").unwrap_or_else(|_| "http://127.0.0.1:8899".to_string()),
        network: std::env::var("NETWORK").unwrap_or_else(|_| "testnet".to_string()),
        max_per_request: parse_env_u64("MAX_PER_REQUEST", DEFAULT_MAX_PER_REQUEST),
        daily_limit_per_ip: parse_env_u64("DAILY_LIMIT_PER_IP", DEFAULT_DAILY_LIMIT_PER_IP),
        cooldown_seconds: parse_env_u64("COOLDOWN_SECONDS", DEFAULT_COOLDOWN_SECONDS),
        airdrops_file: std::env::var("AIRDROPS_FILE")
            .unwrap_or_else(|_| "airdrops.json".to_string()),
        trusted_proxies: parse_csv_env("TRUSTED_PROXY"),
    };

    // Validate RPC URL format
    if !config.rpc_url.starts_with("http://") && !config.rpc_url.starts_with("https://") {
        eprintln!("ERROR: RPC_URL must start with http:// or https://");
        std::process::exit(1);
    }

    if config.network == "mainnet" {
        panic!("❌ Faucet cannot run on mainnet!");
    }

    let state = FaucetState {
        config: config.clone(),
        http: Client::builder().build().expect("reqwest client"),
        rate_limiter: Arc::new(RwLock::new(RateLimiter::default())),
        airdrops: Arc::new(RwLock::new(load_airdrops(&config.airdrops_file))),
    };

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_headers([axum::http::header::CONTENT_TYPE])
        .allow_origin([
            "https://faucet.moltchain.network"
                .parse::<HeaderValue>()
                .unwrap(),
            "https://moltchain.network".parse::<HeaderValue>().unwrap(),
            "http://localhost:3000".parse::<HeaderValue>().unwrap(),
            "http://localhost:3003".parse::<HeaderValue>().unwrap(),
            "http://localhost:9100".parse::<HeaderValue>().unwrap(),
            "http://localhost:9101".parse::<HeaderValue>().unwrap(),
        ]);

    let app = Router::new()
        .route("/health", get(health))
        .route("/faucet/config", get(get_config))
        .route("/faucet/status", get(get_status))
        .route("/faucet/airdrops", get(list_airdrops))
        .route("/faucet/request", post(request_airdrop))
        .with_state(state)
        .layer(cors);

    let port = parse_env_u16("PORT", DEFAULT_PORT);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("bind faucet listener");
    info!("moltchain-faucet listening on {}", addr);
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
            balance_shells: info.treasury_balance,
            balance_molt: info.treasury_balance / SHELLS_PER_MOLT,
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
    let amount_molt = request.amount.unwrap_or(state.config.max_per_request);
    if amount_molt == 0 || amount_molt > state.config.max_per_request {
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

    {
        let mut limiter = state.rate_limiter.write().await;
        if let Err(err) = limiter.check(
            &client_ip,
            request.address.trim(),
            now_ms,
            state.config.daily_limit_per_ip,
            state.config.cooldown_seconds,
        ) {
            return error_json(StatusCode::TOO_MANY_REQUESTS, &err);
        }
    }

    let treasury = match fetch_treasury_info(&state).await {
        Ok(info) => info,
        Err(err) => return error_response(StatusCode::BAD_GATEWAY, &err),
    };

    let required_shells = amount_molt.saturating_mul(SHELLS_PER_MOLT);
    if treasury.treasury_balance < required_shells {
        return error_json(
            StatusCode::SERVICE_UNAVAILABLE,
            "Faucet temporarily empty - check back soon",
        );
    }

    let rpc_result = match rpc_call(
        &state,
        "requestAirdrop",
        json!([request.address.trim(), amount_molt]),
    )
    .await
    {
        Ok(value) => value,
        Err(err) => return error_response(StatusCode::BAD_GATEWAY, &err),
    };

    let mut limiter = state.rate_limiter.write().await;
    limiter.record(&client_ip, request.address.trim(), now_ms, amount_molt);
    drop(limiter);

    let response = FaucetResponse {
        success: true,
        signature: rpc_result
            .get("signature")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string()),
        amount: rpc_result
            .get("amount")
            .and_then(|value| value.as_u64())
            .or(Some(amount_molt)),
        recipient: Some(request.address.trim().to_string()),
        message: rpc_result
            .get("message")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .or(Some(format!(
                "{} MOLT airdropped successfully",
                amount_molt
            ))),
        error: None,
    };

    let mut airdrops = state.airdrops.write().await;
    airdrops.push(AirdropRecord {
        signature: response.signature.clone(),
        recipient: request.address.trim().to_string(),
        amount_molt,
        timestamp_ms: now_ms,
    });
    if let Err(err) = save_airdrops(&state.config.airdrops_file, &airdrops) {
        error!("failed to persist faucet history: {}", err);
    }
    drop(airdrops);

    Json(response).into_response()
}

async fn fetch_treasury_info(state: &FaucetState) -> Result<TreasuryInfo, String> {
    let value = rpc_call(state, "getTreasuryInfo", json!([])).await?;
    serde_json::from_value(value).map_err(|err| format!("invalid treasury response: {}", err))
}

async fn rpc_call(
    state: &FaucetState,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });

    let response = state
        .http
        .post(&state.config.rpc_url)
        .json(&payload)
        .send()
        .await
        .map_err(|err| format!("rpc request failed: {}", err))?;

    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|err| format!("invalid rpc response: {}", err))?;

    if !status.is_success() {
        return Err(format!("rpc http error {}", status));
    }

    if let Some(error) = body.get("error") {
        let message = error
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown rpc error");
        return Err(message.to_string());
    }

    body.get("result")
        .cloned()
        .ok_or_else(|| "rpc response missing result".to_string())
}

fn error_json(status: StatusCode, message: &str) -> Response {
    Json(FaucetResponse {
        success: false,
        signature: None,
        amount: None,
        recipient: None,
        message: None,
        error: Some(message.to_string()),
    })
    .into_response()
    .with_status(status)
}

fn error_response(status: StatusCode, message: &str) -> Response {
    error!("{}", message);
    error_json(status, message)
}

trait ResponseExt {
    fn with_status(self, status: StatusCode) -> Response;
}

impl ResponseExt for Response {
    fn with_status(mut self, status: StatusCode) -> Response {
        *self.status_mut() = status;
        self
    }
}

fn load_airdrops(path: &str) -> Vec<AirdropRecord> {
    if !Path::new(path).exists() {
        return Vec::new();
    }

    match fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn save_airdrops(path: &str, records: &[AirdropRecord]) -> Result<(), String> {
    let parent = Path::new(path).parent().map(|value| value.to_path_buf());
    if let Some(parent) = parent {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
    }
    let payload = serde_json::to_vec_pretty(records).map_err(|err| err.to_string())?;
    fs::write(path, payload).map_err(|err| err.to_string())
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

fn extract_client_ip(
    headers: &HeaderMap,
    peer_addr: SocketAddr,
    trusted_proxies: &[String],
) -> String {
    let peer_ip = peer_addr.ip().to_string();
    if trusted_proxies.iter().any(|value| value == &peer_ip) {
        if let Some(forwarded) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return forwarded;
        }
        if let Some(real_ip) = headers
            .get("x-real-ip")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return real_ip;
        }
    }
    peer_ip
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
