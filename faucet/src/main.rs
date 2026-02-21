// MoltChain Faucet Service
// Airdrop testnet/local MOLT tokens with rate limiting
// Uses signed transactions (sendTransaction) — works with any number of validators

use axum::{
    extract::{Json, Path, Query, State},
    http::{header, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use moltchain_core::{Keypair, Pubkey};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tracing::{error, info, warn};

#[derive(Debug, Deserialize)]
struct FaucetRequest {
    address: String,
    amount: u64, // in MOLT
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
        .route("/faucet/airdrops", get(list_airdrops_handler))
        .route("/faucet/airdrop/:sig", get(get_airdrop_handler))
        .route("/health", get(health_handler))
        .layer(
            CorsLayer::new()
                // I-5: Restrict CORS to known origins instead of wildcard
                .allow_origin([
                    "https://faucet.moltchain.io"
                        .parse::<HeaderValue>()
                        .unwrap(),
                    "https://moltchain.io".parse::<HeaderValue>().unwrap(),
                    "http://localhost:3003".parse::<HeaderValue>().unwrap(),
                    "http://localhost:3000".parse::<HeaderValue>().unwrap(),
                ])
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
    axum::serve(listener, app).await.unwrap_or_else(|e| {
        eprintln!("Faucet server error: {}", e);
        std::process::exit(1);
    });
}

/// Health check
async fn health_handler() -> Response {
    (StatusCode::OK, "OK").into_response()
}

/// Faucet request handler
async fn faucet_request_handler(
    State(state): State<FaucetState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<FaucetRequest>,
) -> Response {
    info!("💧 Faucet request: {} MOLT to {}", req.amount, req.address);

    // Extract client IP from headers (X-Forwarded-For, X-Real-Ip) or use "unknown"
    let client_ip = headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or("unknown").trim().to_string())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "localhost".to_string());

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

    // Validate amount
    if req.amount == 0 || req.amount > state.config.max_per_request {
        return (
            StatusCode::BAD_REQUEST,
            Json(FaucetResponse {
                success: false,
                signature: None,
                amount: None,
                recipient: None,
                message: None,
                error: Some(format!(
                    "Amount must be between 1 and {} MOLT",
                    state.config.max_per_request
                )),
            }),
        )
            .into_response();
    }

    // Check rate limit (per-IP and per-address)
    {
        let mut limiter = state.rate_limiter.write().await;
        if let Err(err) = limiter.check_and_record(
            &client_ip,
            &req.address,
            req.amount,
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
    match send_faucet_transfer(&state, &req.address, req.amount).await {
        Ok(result) => {
            info!("✅ Airdropped {} MOLT to {}", req.amount, req.address);

            let timestamp_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let sig = format!("airdrop-{}", timestamp_ms);

            // Record airdrop for history API
            {
                let record = AirdropRecord {
                    signature: sig.clone(),
                    recipient: req.address.clone(),
                    amount_molt: req.amount,
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
                if let Ok(data) = serde_json::to_string(&*airdrops) {
                    let _ = std::fs::write(&state.config.airdrops_file, data);
                }
            }

            (
                StatusCode::OK,
                Json(FaucetResponse {
                    success: true,
                    signature: Some(sig),
                    amount: Some(req.amount),
                    recipient: Some(req.address.clone()),
                    message: Some(result),
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
                // Format 2: JSON object with secret_key or privateKey (hex)
                if let Ok(obj) = serde_json::from_str::<serde_json::Value>(&data) {
                    let hex_key = obj
                        .get("secret_key")
                        .or_else(|| obj.get("privateKey"))
                        .and_then(|v| v.as_str());
                    if let Some(hex_str) = hex_key {
                        if let Ok(bytes) = hex::decode(hex_str.trim()) {
                            if bytes.len() >= 32 {
                                let mut seed = [0u8; 32];
                                seed.copy_from_slice(&bytes[..32]);
                                let kp = Keypair::from_seed(&seed);
                                info!("🔑 Loaded faucet keypair from {} (JSON object)", path);
                                return kp;
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

/// Send airdrop via the requestAirdrop RPC method.
/// This bypasses the need for a funded faucet wallet — the RPC debits the
/// treasury and credits the recipient directly (testnet/devnet only).
async fn send_faucet_transfer(
    state: &FaucetState,
    recipient_address: &str,
    amount_molt: u64,
) -> Result<String, String> {
    let client = reqwest::Client::new();
    let resp = client
        .post(&state.config.rpc_url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "requestAirdrop",
            "params": [recipient_address, amount_molt]
        }))
        .send()
        .await
        .map_err(|e| format!("RPC request failed: {}", e))?;

    let data: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    if let Some(error) = data.get("error") {
        return Err(format!(
            "Airdrop failed: {}",
            error["message"].as_str().unwrap_or("unknown")
        ));
    }

    Ok(format!(
        "{} MOLT airdropped to {}",
        amount_molt, recipient_address
    ))
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
