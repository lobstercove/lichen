// MoltChain Faucet Service
// Airdrop testnet/local MOLT tokens with rate limiting
// Uses signed transactions (sendTransaction) — works with any number of validators

use axum::{
    extract::{ConnectInfo, Json, Path, Query, State},
    http::{header, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use moltchain_core::{Keypair, Pubkey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tower_http::{
    cors::CorsLayer,
    services::{ServeDir, ServeFile},
};
use tracing::{error, info, warn};

#[derive(Debug, Deserialize)]
struct FaucetRequest {
    address: String,
    #[serde(default)]
    amount: Option<u64>, // in MOLT; defaults to max_per_request when omitted
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
    signature: String,
    recipient: String,
    amount_molt: u64,
    timestamp_ms: u64,
}

/// Faucet state
#[derive(Clone)]
#[allow(dead_code)]
struct FaucetState {
    config: FaucetConfig,
    rate_limiter: Arc<RwLock<RateLimiter>>,
    /// AUDIT-FIX L7: Per-IP rate limiter for status endpoint (IP -> (window_start, count))
    status_rate: Arc<RwLock<HashMap<String, (u64, u32)>>>,
    airdrops: Arc<RwLock<Vec<AirdropRecord>>>,
    keypair: Arc<Keypair>,
}

#[derive(Clone)]
struct FaucetConfig {
    rpc_url: String,
    network: String, // "testnet" | "local"
    max_per_request: u64,
    daily_limit_per_ip: u64,
    cooldown_seconds: u64,
    airdrops_file: String,
    trusted_proxies: Vec<String>,
}

fn parse_trusted_proxies() -> Vec<String> {
    std::env::var("TRUSTED_PROXY")
        .unwrap_or_default()
        .split(',')
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

fn is_trusted_proxy(peer_ip: &str, trusted: &[String]) -> bool {
    trusted.iter().any(|value| value == peer_ip)
}

fn extract_client_ip(
    headers: &axum::http::HeaderMap,
    peer_addr: SocketAddr,
    trusted_proxies: &[String],
) -> String {
    let peer_ip = peer_addr.ip().to_string();
    if !is_trusted_proxy(&peer_ip, trusted_proxies) {
        return peer_ip;
    }

    let forwarded_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let real_ip = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    forwarded_ip.or(real_ip).unwrap_or(peer_ip)
}

/// Rate limiter that tracks both per-address and per-IP usage
struct RateLimiter {
    /// IP -> (last_request_timestamp, total_molt_today, day_start_timestamp)
    ip_usage: HashMap<String, (u64, u64, u64)>,
    /// address -> last_request_timestamp
    address_requests: HashMap<String, u64>,
}

impl RateLimiter {
    fn new() -> Self {
        Self {
            ip_usage: HashMap::new(),
            address_requests: HashMap::new(),
        }
    }

    /// L6 fix: evict stale entries older than 24 hours to prevent unbounded memory growth
    fn cleanup_stale(&mut self) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let day_seconds: u64 = 86400;
        self.ip_usage
            .retain(|_, (last_req, _, _)| now - *last_req < day_seconds);
        self.address_requests
            .retain(|_, last_req| now - *last_req < day_seconds);
    }

    fn check_and_record(
        &mut self,
        ip: &str,
        address: &str,
        amount: u64,
        cooldown: u64,
        daily_limit: u64,
    ) -> Result<(), String> {
        // L6 fix: periodic cleanup of stale entries
        self.cleanup_stale();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let day_seconds: u64 = 86400;

        // Check per-address cooldown
        if let Some(&last_request) = self.address_requests.get(address) {
            let elapsed = now - last_request;
            if elapsed < cooldown {
                let remaining = cooldown - elapsed;
                return Err(format!(
                    "Address rate limit. Try again in {} seconds.",
                    remaining
                ));
            }
        }

        // Check per-IP daily limit
        if let Some((_, molt_today, day_start)) = self.ip_usage.get(ip) {
            if now - day_start < day_seconds {
                // Same day
                if molt_today + amount > daily_limit {
                    let remaining = day_seconds - (now - day_start);
                    return Err(format!(
                        "Daily limit reached ({} MOLT/IP/day). Resets in {} minutes.",
                        daily_limit,
                        remaining / 60
                    ));
                }
            }
            // else: new day, will reset below
        }

        // Record usage
        self.address_requests.insert(address.to_string(), now);

        let entry = self.ip_usage.entry(ip.to_string()).or_insert((now, 0, now));
        if now - entry.2 >= day_seconds {
            // New day: reset
            entry.1 = amount;
            entry.2 = now;
        } else {
            entry.1 += amount;
        }
        entry.0 = now;

        Ok(())
    }
}

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Load config from environment
    let airdrops_file =
        std::env::var("AIRDROPS_FILE").unwrap_or_else(|_| "airdrops.json".to_string());

    let config = FaucetConfig {
        rpc_url: std::env::var("RPC_URL").unwrap_or_else(|_| "http://localhost:8899".to_string()),
        network: std::env::var("NETWORK").unwrap_or_else(|_| "testnet".to_string()),
        max_per_request: std::env::var("MAX_PER_REQUEST")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100),
        daily_limit_per_ip: std::env::var("DAILY_LIMIT_PER_IP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(100), // 100 MOLT per IP per day ($10 at $0.10/MOLT)
        cooldown_seconds: std::env::var("COOLDOWN_SECONDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60), // 60 second cooldown between requests
        airdrops_file: airdrops_file.clone(),
        trusted_proxies: parse_trusted_proxies(),
    };

    if config.network == "mainnet" {
        panic!("❌ Faucet cannot run on mainnet!");
    }

    // Load or generate faucet keypair
    let keypair = load_or_generate_keypair();
    let faucet_address = keypair.pubkey().to_base58();

    // Load persisted airdrops
    let airdrops: Vec<AirdropRecord> = std::fs::read_to_string(&airdrops_file)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    info!("Loaded {} persisted airdrop records", airdrops.len());

    let state = FaucetState {
        config,
        rate_limiter: Arc::new(RwLock::new(RateLimiter::new())),
        status_rate: Arc::new(RwLock::new(HashMap::new())),
        airdrops: Arc::new(RwLock::new(airdrops)),
        keypair: Arc::new(keypair),
    };

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(9100);

    // Build router
    let app = Router::new()
        .route("/faucet/request", post(faucet_request_handler))
        .route("/faucet/config", get(faucet_config_handler))
        .route("/faucet/status", get(faucet_status_handler))
        .route("/faucet/airdrops", get(list_airdrops_handler))
        .route("/faucet/airdrop/:sig", get(get_airdrop_handler))
        .route("/health", get(health_handler))
        .nest_service(
            "/",
            ServeDir::new("faucet")
                .append_index_html_on_directories(true)
                .fallback(ServeFile::new("faucet/index.html")),
        )
        .layer(
            CorsLayer::new()
                // AUDIT-FIX L6: Load CORS origins from env or use defaults
                .allow_origin({
                    let origins_str = std::env::var("FAUCET_CORS_ORIGINS").unwrap_or_else(|_| {
                        [
                            "https://faucet.moltchain.network",
                            "https://wallet.moltchain.network",
                            "https://moltchain.network",
                            "https://faucet.moltchain.io",
                            "https://moltchain.io",
                        ].join(",")
                    });
                    let origins: Vec<HeaderValue> = origins_str
                        .split(',')
                        .filter_map(|s| s.trim().parse::<HeaderValue>().ok())
                        .collect();
                    origins
                })
                .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
                .allow_headers([header::CONTENT_TYPE, header::ACCEPT]),
        )
        .with_state(state.clone());

    let addr = format!("0.0.0.0:{}", port);
    info!("💧 MoltChain Faucet Service starting on {}", addr);
    info!("   Network: {}", state.config.network);
    info!("   Faucet wallet: {}", faucet_address);
    info!("   Max per request: {} MOLT", state.config.max_per_request);
    info!(
        "   Daily limit per IP: {} MOLT",
        state.config.daily_limit_per_ip
    );
    info!("   Cooldown: {} seconds", state.config.cooldown_seconds);
    info!("   RPC URL: {}", state.config.rpc_url);
    info!("   ℹ️  Fund the faucet wallet with MOLT before use");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to bind faucet to {}: {}", addr, e);
            std::process::exit(1);
        });
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .unwrap_or_else(|e| {
        eprintln!("Faucet server error: {}", e);
        std::process::exit(1);
    });
}

/// Health check
async fn health_handler() -> Response {
    (StatusCode::OK, "OK").into_response()
}

async fn faucet_config_handler(State(state): State<FaucetState>) -> Response {
    let config = FaucetPublicConfig {
        max_per_request: state.config.max_per_request,
        daily_limit_per_ip: state.config.daily_limit_per_ip,
        cooldown_seconds: state.config.cooldown_seconds,
        network: state.config.network.clone(),
    };

    (StatusCode::OK, Json(config)).into_response()
}

async fn faucet_status_handler(
    State(state): State<FaucetState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
) -> Response {
    // AUDIT-FIX L7: Basic rate limiting on status endpoint (30 req/min per IP)
    let client_ip = extract_client_ip(&headers, peer_addr, &state.config.trusted_proxies);
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut map = state.status_rate.write().await;
        let entry = map.entry(client_ip).or_insert((now, 0));
        if now - entry.0 > 60 {
            entry.0 = now;
            entry.1 = 0;
        }
        entry.1 += 1;
        if entry.1 > 30 {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({ "error": "Rate limit exceeded. Try again later." })),
            )
                .into_response();
        }
    }
    match get_faucet_balance_shells(&state).await {
        Ok(balance_shells) => {
            let status = FaucetStatusResponse {
                network: state.config.network.clone(),
                faucet_address: state.keypair.pubkey().to_base58(),
                balance_shells,
                balance_molt: balance_shells / 1_000_000_000,
            };
            (StatusCode::OK, Json(status)).into_response()
        }
        Err(err) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": format!("Unable to fetch faucet balance: {}", err)
            })),
        )
            .into_response(),
    }
}

async fn get_faucet_balance_shells(state: &FaucetState) -> Result<u64, String> {
    let faucet_address = state.keypair.pubkey().to_base58();
    let client = reqwest::Client::new();

    let response = client
        .post(&state.config.rpc_url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getBalance",
            "params": [faucet_address]
        }))
        .send()
        .await
        .map_err(|e| format!("getBalance RPC failed: {}", e))?;

    let data: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    if let Some(error) = data.get("error") {
        return Err(format!(
            "getBalance failed: {}",
            error["message"].as_str().unwrap_or("unknown")
        ));
    }

    let shells = data["result"]["shells"]
        .as_u64()
        .or_else(|| data["result"]["balance_shells"].as_u64())
        .or_else(|| data["result"].as_u64())
        .unwrap_or(0);

    Ok(shells)
}

/// Faucet request handler
async fn faucet_request_handler(
    State(state): State<FaucetState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    Json(req): Json<FaucetRequest>,
) -> Response {
    // Resolve amount: use configured max when client omits or sends 0
    let amount = match req.amount {
        Some(a) if a >= 1 && a <= state.config.max_per_request => a,
        _ => state.config.max_per_request,
    };
    info!("💧 Faucet request: {} MOLT to {}", amount, req.address);

    // Use ConnectInfo as canonical fallback when trusted proxy headers are absent.
    let client_ip = extract_client_ip(&headers, peer_addr, &state.config.trusted_proxies);

    info!("   Client IP: {}", client_ip);

    // Validate address
    let _recipient = match Pubkey::from_base58(&req.address) {
        Ok(pubkey) => pubkey,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(FaucetResponse {
                    success: false,
                    signature: None,
                    amount: None,
                    recipient: None,
                    message: None,
                    error: Some("Invalid address format".to_string()),
                }),
            )
                .into_response();
        }
    };

    // Pre-flight faucet balance check before consuming rate-limit slot.
    match get_faucet_balance_shells(&state).await {
        Ok(0) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(FaucetResponse {
                    success: false,
                    signature: None,
                    amount: None,
                    recipient: None,
                    message: None,
                    error: Some("Faucet wallet is empty. Please refill and retry.".to_string()),
                }),
            )
                .into_response();
        }
        Ok(_) => {}
        Err(err) => {
            warn!("Balance preflight failed: {}", err);
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(FaucetResponse {
                    success: false,
                    signature: None,
                    amount: None,
                    recipient: None,
                    message: None,
                    error: Some(
                        "Unable to verify faucet balance. Please retry shortly.".to_string(),
                    ),
                }),
            )
                .into_response();
        }
    }

    // Check rate limit (per-IP and per-address)
    {
        let mut limiter = state.rate_limiter.write().await;
        if let Err(err) = limiter.check_and_record(
            &client_ip,
            &req.address,
            amount,
            state.config.cooldown_seconds,
            state.config.daily_limit_per_ip,
        ) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(FaucetResponse {
                    success: false,
                    signature: None,
                    amount: None,
                    recipient: None,
                    message: None,
                    error: Some(err),
                }),
            )
                .into_response();
        }
    }

    // Send a signed transfer transaction from the faucet wallet
    match send_faucet_transfer(&state, &req.address, amount).await {
        Ok(sig_hex) => {
            info!("✅ Airdropped {} MOLT to {}", amount, req.address);

            let timestamp_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            // Record airdrop for history API
            {
                let record = AirdropRecord {
                    signature: sig_hex.clone(),
                    recipient: req.address.clone(),
                    amount_molt: amount,
                    timestamp_ms,
                };
                let mut airdrops = state.airdrops.write().await;
                airdrops.push(record);
                // L6 fix: cap in-memory airdrops to last 10000 entries
                if airdrops.len() > 10_000 {
                    let drain_count = airdrops.len() - 10_000;
                    airdrops.drain(..drain_count);
                }
                // Persist to file (best effort)
                // AUDIT-FIX M9: Use tokio::fs::write instead of blocking std::fs::write
                if let Ok(data) = serde_json::to_string(&*airdrops) {
                    let path = state.config.airdrops_file.clone();
                    tokio::spawn(async move {
                        let _ = tokio::fs::write(&path, data).await;
                    });
                }
            }

            (
                StatusCode::OK,
                Json(FaucetResponse {
                    success: true,
                    signature: Some(sig_hex),
                    amount: Some(amount),
                    recipient: Some(req.address.clone()),
                    message: Some(format!(
                        "{} MOLT transferred to {}",
                        amount, req.address
                    )),
                    error: None,
                }),
            )
                .into_response()
        }
        Err(err) => {
            error!("❌ Airdrop failed: {}", err);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(FaucetResponse {
                    success: false,
                    signature: None,
                    amount: None,
                    recipient: None,
                    message: None,
                    error: Some(format!("Airdrop failed: {}", err)),
                }),
            )
                .into_response()
        }
    }
}

/// Load faucet keypair from FAUCET_KEYPAIR env (path to JSON seed file),
/// or generate one and save to `faucet-keypair.json` in the working directory.
fn load_or_generate_keypair() -> Keypair {
    if let Ok(path) = std::env::var("FAUCET_KEYPAIR") {
        match std::fs::read_to_string(&path) {
            Ok(data) => {
                // Format 1: JSON byte array [u8, u8, ...]
                if let Ok(seed_bytes) = serde_json::from_str::<Vec<u8>>(&data) {
                    if seed_bytes.len() >= 32 {
                        let mut seed = [0u8; 32];
                        seed.copy_from_slice(&seed_bytes[..32]);
                        let kp = Keypair::from_seed(&seed);
                        info!("🔑 Loaded faucet keypair from {}", path);
                        return kp;
                    }
                }
                // Format 2: JSON object with secret_key or privateKey
                if let Ok(obj) = serde_json::from_str::<serde_json::Value>(&data) {
                    let key_val = obj
                        .get("secret_key")
                        .or_else(|| obj.get("privateKey"))
                        .or_else(|| obj.get("seed"));

                    if let Some(val) = key_val {
                        // Format 2a: byte array  {"privateKey": [u8, u8, ...]}
                        if let Some(arr) = val.as_array() {
                            let bytes: Vec<u8> = arr
                                .iter()
                                .filter_map(|v| v.as_u64().map(|n| n as u8))
                                .collect();
                            if bytes.len() >= 32 {
                                let mut seed = [0u8; 32];
                                seed.copy_from_slice(&bytes[..32]);
                                let kp = Keypair::from_seed(&seed);
                                info!("🔑 Loaded faucet keypair from {} (JSON array)", path);
                                return kp;
                            }
                        }
                        // Format 2b: hex string  {"privateKey": "abcd..."}
                        if let Some(hex_str) = val.as_str() {
                            let clean = hex_str.trim().trim_start_matches("0x");
                            if let Ok(bytes) = hex::decode(clean) {
                                if bytes.len() >= 32 {
                                    let mut seed = [0u8; 32];
                                    seed.copy_from_slice(&bytes[..32]);
                                    let kp = Keypair::from_seed(&seed);
                                    info!("🔑 Loaded faucet keypair from {} (JSON hex)", path);
                                    return kp;
                                }
                            }
                        }
                    }
                }
                // Format 3: raw hex string
                let hex_str = data.trim().trim_matches('"');
                if let Ok(bytes) = hex::decode(hex_str) {
                    if bytes.len() >= 32 {
                        let mut seed = [0u8; 32];
                        seed.copy_from_slice(&bytes[..32]);
                        let kp = Keypair::from_seed(&seed);
                        info!("🔑 Loaded faucet keypair from {} (hex)", path);
                        return kp;
                    }
                }
                eprintln!("❌ Invalid keypair file format at {}", path);
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("❌ Cannot read FAUCET_KEYPAIR at {}: {}", path, e);
                std::process::exit(1);
            }
        }
    }

    // Auto-generate and persist
    let default_path = "faucet-keypair.json";
    if let Ok(data) = std::fs::read_to_string(default_path) {
        if let Ok(seed_bytes) = serde_json::from_str::<Vec<u8>>(&data) {
            if seed_bytes.len() >= 32 {
                let mut seed = [0u8; 32];
                seed.copy_from_slice(&seed_bytes[..32]);
                let kp = Keypair::from_seed(&seed);
                info!("🔑 Loaded existing faucet keypair from {}", default_path);
                return kp;
            }
        }
    }

    let kp = Keypair::generate();
    let seed_json = serde_json::to_string(&kp.secret_key().to_vec()).unwrap();
    if let Err(e) = std::fs::write(default_path, &seed_json) {
        warn!(
            "⚠️  Could not save faucet keypair to {}: {}",
            default_path, e
        );
    } else {
        // I-4: Set restrictive permissions (owner-only read/write)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) =
                std::fs::set_permissions(default_path, std::fs::Permissions::from_mode(0o600))
            {
                warn!("⚠️  Could not set keypair permissions: {}", e);
            }
        }
        info!("🔑 Generated new faucet keypair → {}", default_path);
    }
    kp
}

/// Send airdrop as a signed transfer transaction from the faucet wallet.
/// Builds a native transfer (system program, instruction type 0x00),
/// signs it with the faucet keypair, encodes as base64 bincode, and
/// submits via `sendTransaction` RPC — works with any number of validators.
async fn send_faucet_transfer(
    state: &FaucetState,
    recipient_address: &str,
    amount_molt: u64,
) -> Result<String, String> {
    use moltchain_core::{Hash, Instruction, Message, Transaction};

    // 1. Resolve recipient pubkey
    let recipient = Pubkey::from_base58(recipient_address)
        .map_err(|_| format!("Invalid recipient address: {}", recipient_address))?;

    // 2. Fetch recent blockhash from the validator
    let client = reqwest::Client::new();
    let bh_resp = client
        .post(&state.config.rpc_url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getRecentBlockhash"
        }))
        .send()
        .await
        .map_err(|e| format!("RPC request failed: {}", e))?;

    let bh_data: serde_json::Value = bh_resp.json().await.map_err(|e| e.to_string())?;
    if let Some(error) = bh_data.get("error") {
        return Err(format!(
            "getRecentBlockhash failed: {}",
            error["message"].as_str().unwrap_or("unknown")
        ));
    }

    let bh_hex = bh_data["result"]["blockhash"]
        .as_str()
        .or_else(|| bh_data["result"].as_str())
        .ok_or("getRecentBlockhash: missing blockhash in result")?;
    let bh_bytes = hex::decode(bh_hex).map_err(|e| format!("Invalid blockhash hex: {}", e))?;
    if bh_bytes.len() != 32 {
        return Err(format!(
            "Invalid blockhash length: {} (expected 32)",
            bh_bytes.len()
        ));
    }
    let mut bh_arr = [0u8; 32];
    bh_arr.copy_from_slice(&bh_bytes);
    let recent_blockhash = Hash(bh_arr);

    // 3. Build the transfer instruction:
    //    system program (all-zero), instruction type 0x00, 8-byte LE amount
    let amount_shells = amount_molt * 1_000_000_000; // MOLT → shells
    let mut ix_data = vec![0x00u8]; // Transfer instruction type
    ix_data.extend_from_slice(&amount_shells.to_le_bytes());

    let system_program = Pubkey([0u8; 32]);
    let faucet_pubkey = state.keypair.pubkey();

    let ix = Instruction {
        program_id: system_program,
        accounts: vec![faucet_pubkey, recipient],
        data: ix_data,
    };

    // 4. Build message and transaction
    let message = Message::new(vec![ix], recent_blockhash);
    let mut tx = Transaction::new(message);

    // 5. Sign with faucet keypair
    let msg_bytes = tx.message.serialize();
    let signature = state.keypair.sign(&msg_bytes);
    tx.signatures.push(signature);

    // 6. Serialize to bincode and base64-encode
    let tx_bytes =
        bincode::serialize(&tx).map_err(|e| format!("Transaction serialization failed: {}", e))?;
    let tx_b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &tx_bytes);

    // 7. Submit via sendTransaction RPC
    let send_resp = client
        .post(&state.config.rpc_url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendTransaction",
            "params": [tx_b64]
        }))
        .send()
        .await
        .map_err(|e| format!("sendTransaction RPC failed: {}", e))?;

    let send_data: serde_json::Value = send_resp.json().await.map_err(|e| e.to_string())?;

    if let Some(error) = send_data.get("error") {
        return Err(format!(
            "sendTransaction failed: {}",
            error["message"].as_str().unwrap_or("unknown")
        ));
    }

    let sig_hex = send_data["result"]
        .as_str()
        .ok_or("sendTransaction: missing signature in result")?
        .to_string();

    Ok(sig_hex)
}

/// List airdrops (optionally filtered by address)
/// GET /faucet/airdrops?address=X&limit=50
async fn list_airdrops_handler(
    State(state): State<FaucetState>,
    Query(params): Query<HashMap<String, String>>,
) -> Response {
    let airdrops = state.airdrops.read().await;
    let address = params.get("address");
    let limit: usize = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(100);

    let filtered: Vec<&AirdropRecord> = airdrops
        .iter()
        .rev() // newest first
        .filter(|a| {
            if let Some(addr) = address {
                &a.recipient == addr
            } else {
                true
            }
        })
        .take(limit)
        .collect();

    (StatusCode::OK, Json(filtered)).into_response()
}

/// Get a single airdrop by signature
/// GET /faucet/airdrop/:sig
async fn get_airdrop_handler(
    State(state): State<FaucetState>,
    Path(sig): Path<String>,
) -> Response {
    let airdrops = state.airdrops.read().await;
    if let Some(record) = airdrops.iter().find(|a| a.signature == sig) {
        (StatusCode::OK, Json(record.clone())).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "Airdrop not found"})),
        )
            .into_response()
    }
}
