// MoltChain RPC Server
// JSON-RPC API for querying blockchain state
//
// ═══════════════════════════════════════════════════════════════════════════════
// MODULE LAYOUT (logical sections within this file)
// ═══════════════════════════════════════════════════════════════════════════════
//   mod ws;                        — WebSocket subscriptions (ws.rs)
//
//   SHARED STATE & TYPES           — RpcRequest, RpcResponse, RpcError, RpcState
//   ADMIN AUTH                     — verify_admin_auth, constant_time_eq
//   RATE LIMITING & MIDDLEWARE     — RateLimiter, rate_limit_middleware
//   UTILITY FUNCTIONS              — count_executable_accounts, parsing helpers
//   SOLANA SERIALIZATION HELPERS   — solana_context, solana_*_json, encoding
//   SERVER STARTUP & ROUTER        — start_rpc_server, build_rpc_router
//   RPC DISPATCH                   — handle_rpc, handle_solana_rpc, handle_evm_rpc
//   NATIVE MOLT RPC METHODS        — getBalance, getAccount, getBlock, getSlot, …
//   CORE TRANSACTION METHODS       — getTransaction, getTransactionsByAddress, send
//   SOLANA-COMPATIBLE ENDPOINTS    — Solana JSON-RPC compatibility layer
//   NETWORK ENDPOINTS              — getPeers, getNetworkInfo
//   VALIDATOR ENDPOINTS            — getValidatorInfo, getValidatorPerformance
//   STAKING ENDPOINTS              — stake, unstake, getStakingStatus
//   ACCOUNT ENDPOINTS              — getAccountInfo, getTransactionHistory
//   CONTRACT ENDPOINTS             — getContractInfo, getContractLogs, ABI
//   PROGRAM ENDPOINTS              — getProgram, getProgramStats, getProgramCalls
//   NFT ENDPOINTS                  — getCollection, getNFT, getNFTsByOwner
//   MARKETPLACE ENDPOINTS          — getMarketListings, getMarketSales
//   ETHEREUM JSON-RPC LAYER        — eth_getBalance, eth_sendRawTransaction, …
//   REEFSTAKE ENDPOINTS            — stakeToReefStake, unstakeFromReefStake
//   TOKEN ENDPOINTS                — getTokenBalance, getTokenHolders, getTokenTransfers
//   DEX REST API                    — /api/v1/* endpoints (dex.rs)
//   DEX WEBSOCKET FEEDS             — orderbook, trades, ticker, candles (dex_ws.rs)
// ═══════════════════════════════════════════════════════════════════════════════

pub mod dex;
pub mod dex_ws;
pub mod ws;

use alloy_primitives::{Address, Bytes, U256};
use axum::http::{HeaderValue, Method};
use axum::{
    extract::ConnectInfo,
    extract::State,
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use lru::LruCache;
use moltchain_core::contract::ContractAccount;
use moltchain_core::nft::{decode_collection_state, decode_token_state, NftActivityKind};
use moltchain_core::{
    decode_evm_transaction, shells_to_u256, simulate_evm_call, Account, Hash, Instruction,
    MarketActivityKind, Pubkey, StakePool, StateStore, SymbolRegistryEntry, Transaction,
    TxProcessor, CONTRACT_PROGRAM_ID, EVM_PROGRAM_ID, SYSTEM_PROGRAM_ID,
};

/// System account owner (Pubkey([0x01; 32]))
const SYSTEM_ACCOUNT_OWNER: Pubkey = Pubkey([0x01; 32]);
use moltchain_core::consensus::{HEARTBEAT_BLOCK_REWARD, TRANSACTION_BLOCK_REWARD};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::{info, warn};

// Re-export WebSocket types
pub use ws::{start_ws_server, Event as WsEvent};

// ═══════════════════════════════════════════════════════════════════════════════
// SHARED STATE & TYPES
// ═══════════════════════════════════════════════════════════════════════════════

/// JSON-RPC request
#[derive(Debug, Deserialize)]
struct RpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: serde_json::Value,
    method: String,
    params: Option<serde_json::Value>,
}

/// JSON-RPC response
#[derive(Debug, Serialize)]
struct RpcResponse {
    jsonrpc: String,
    id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

/// JSON-RPC error
#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

/// Shared RPC state
#[derive(Clone)]
struct RpcState {
    state: StateStore,
    /// Channel to send transactions to mempool
    tx_sender: Option<mpsc::Sender<Transaction>>,
    /// P2P network (optional, for peer count queries)
    p2p: Option<Arc<dyn P2PNetworkTrait>>,
    /// Stake pool (optional, for staking queries)
    stake_pool: Option<Arc<tokio::sync::Mutex<moltchain_core::StakePool>>>,
    chain_id: String,
    network_id: String,
    version: String,
    evm_chain_id: u64,
    solana_tx_cache: Arc<Mutex<LruCache<Hash, SolanaTxRecord>>>,
    /// Admin token for state-mutating RPC endpoints (setFeeConfig, setRentParams, setContractAbi)
    admin_token: Option<String>,
    /// T2.6: Per-IP rate limiter
    rate_limiter: Arc<RateLimiter>,
}

/// H16 fix: Guard state-mutating RPC endpoints in multi-validator mode.
/// Direct state writes bypass consensus and cause divergence when >1 validator.
/// In multi-validator mode, callers must submit proper signed transactions
/// via `sendTransaction` instead.
fn require_single_validator(state: &RpcState, endpoint: &str) -> Result<(), RpcError> {
    let validators = state.state.get_all_validators().unwrap_or_default();
    if validators.len() > 1 {
        return Err(RpcError {
            code: -32003,
            message: format!(
                "{} is disabled in multi-validator mode ({} validators active). \
                 Submit a signed transaction via sendTransaction instead.",
                endpoint,
                validators.len()
            ),
        });
    }
    Ok(())
}

/// Verify admin authorization from params
fn verify_admin_auth(state: &RpcState, params: &Option<serde_json::Value>) -> Result<(), RpcError> {
    let required_token = state.admin_token.as_ref().ok_or_else(|| RpcError {
        code: -32003,
        message: "Admin endpoints disabled: no admin_token configured".to_string(),
    })?;

    let token = params
        .as_ref()
        .and_then(|p| p.as_object())
        .and_then(|o| o.get("admin_token"))
        .and_then(|v| v.as_str())
        .or_else(|| {
            params
                .as_ref()
                .and_then(|p| p.as_array())
                .and_then(|arr| arr.last())
                .and_then(|v| v.as_object())
                .and_then(|o| o.get("admin_token"))
                .and_then(|v| v.as_str())
        });

    match token {
        Some(t) if constant_time_eq(t.as_bytes(), required_token.as_bytes()) => Ok(()),
        Some(_) => Err(RpcError {
            code: -32003,
            message: "Invalid admin token".to_string(),
        }),
        None => Err(RpcError {
            code: -32003,
            message: "Missing admin_token in params".to_string(),
        }),
    }
}

/// T8.4: Constant-time byte comparison to prevent timing side-channel attacks
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[derive(Debug, Clone)]
struct SolanaTxRecord {
    tx: Transaction,
    slot: u64,
    timestamp: u64,
    fee: u64,
}

/// Trait for P2P network to allow RPC queries
pub trait P2PNetworkTrait: Send + Sync {
    fn peer_count(&self) -> usize;
    fn peer_addresses(&self) -> Vec<String>;
}

// ═══════════════════════════════════════════════════════════════════════════════
// RATE LIMITING & MIDDLEWARE
// ═══════════════════════════════════════════════════════════════════════════════

/// T2.6: Per-IP rate limiter with stale entry pruning
struct RateLimiter {
    requests: std::sync::Mutex<HashMap<IpAddr, (u64, Instant)>>,
    max_per_second: u64,
    last_prune: std::sync::Mutex<Instant>,
}

impl RateLimiter {
    fn new(max_per_second: u64) -> Self {
        Self {
            requests: std::sync::Mutex::new(HashMap::new()),
            max_per_second,
            last_prune: std::sync::Mutex::new(Instant::now()),
        }
    }

    /// Check if a request from `ip` is within the rate limit.
    /// Returns `true` if allowed, `false` if rate-limited.
    fn check(&self, ip: IpAddr) -> bool {
        let mut map = self.requests.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();

        // Prune stale entries every 30 seconds to prevent memory exhaustion
        {
            let mut last = self.last_prune.lock().unwrap_or_else(|e| e.into_inner());
            if now.duration_since(*last).as_secs() >= 30 {
                map.retain(|_, (_, ts)| now.duration_since(*ts).as_secs() < 60);
                *last = now;
            }
        }

        let entry = map.entry(ip).or_insert((0, now));
        if now.duration_since(entry.1).as_secs() >= 1 {
            // Window expired, reset counter
            entry.0 = 1;
            entry.1 = now;
            true
        } else {
            entry.0 += 1;
            entry.0 <= self.max_per_second
        }
    }
}

/// T2.6: Extract client IP from ConnectInfo extension.
/// Does NOT trust X-Forwarded-For to prevent rate limit bypass via header spoofing.
fn extract_client_ip(req: &axum::extract::Request) -> IpAddr {
    // Use ConnectInfo (available when using into_make_service_with_connect_info)
    if let Some(connect_info) = req.extensions().get::<ConnectInfo<SocketAddr>>() {
        return connect_info.0.ip();
    }
    // Fallback: use loopback (which will be rate-limited like any other IP)
    IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1))
}

/// T2.6: Rate limiting middleware
async fn rate_limit_middleware(
    State(state): State<Arc<RpcState>>,
    req: axum::extract::Request,
    next: middleware::Next,
) -> Response {
    // Allow CORS preflight through without rate limiting
    if req.method() == Method::OPTIONS {
        return next.run(req).await;
    }
    let ip = extract_client_ip(&req);
    if !state.rate_limiter.check(ip) {
        warn!("T2.6: Rate limit exceeded for IP {}", ip);
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": -32005, "message": "Rate limit exceeded"}
            })),
        )
            .into_response();
    }
    next.run(req).await
}

// ═══════════════════════════════════════════════════════════════════════════════
// UTILITY FUNCTIONS & HELPERS
// ═══════════════════════════════════════════════════════════════════════════════

/// Helper: Count executable accounts (contracts) using the programs index
fn count_executable_accounts(state: &StateStore) -> u64 {
    // Uses the CF_PROGRAMS index which tracks all deployed programs
    state
        .get_programs(usize::MAX)
        .map(|programs| programs.len() as u64)
        .unwrap_or(0)
}

fn parse_transfer_amount(ix: &Instruction) -> Option<u64> {
    if ix.program_id != SYSTEM_PROGRAM_ID {
        return None;
    }
    if ix.data.len() < 9
        || (ix.data[0] != 0
            && ix.data[0] != 2
            && ix.data[0] != 3
            && ix.data[0] != 4
            && ix.data[0] != 5)
    {
        return None;
    }
    let amount_bytes: [u8; 8] = ix.data[1..9].try_into().ok()?;
    Some(u64::from_le_bytes(amount_bytes))
}

fn instruction_type(ix: &Instruction) -> &'static str {
    if ix.program_id == SYSTEM_PROGRAM_ID {
        if ix.data.first() == Some(&0) {
            return "Transfer";
        }
        if ix.data.first() == Some(&2) {
            return "Reward";
        }
        if ix.data.first() == Some(&3) {
            return "GrantRepay";
        }
        if ix.data.first() == Some(&4) {
            return "GenesisTransfer";
        }
        if ix.data.first() == Some(&5) {
            return "GenesisMint";
        }
        if ix.data.first() == Some(&6) {
            return "CreateCollection";
        }
        if ix.data.first() == Some(&7) {
            return "MintNFT";
        }
        if ix.data.first() == Some(&8) {
            return "TransferNFT";
        }
        if ix.data.first() == Some(&9) {
            return "Stake";
        }
        if ix.data.first() == Some(&10) {
            return "Unstake";
        }
        if ix.data.first() == Some(&11) {
            return "ClaimUnstake";
        }
        if ix.data.first() == Some(&12) {
            return "RegisterEvmAddress";
        }
        return "System";
    }
    if ix.program_id == CONTRACT_PROGRAM_ID {
        return "Contract";
    }
    "Program"
}

fn evm_chain_id_from_chain_id(chain_id: &str) -> u64 {
    let hash = Hash::hash(chain_id.as_bytes());
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&hash.0[..8]);
    let value = u64::from_le_bytes(bytes);
    if value == 0 {
        1
    } else {
        value
    }
}

fn hash_to_base58(hash: &Hash) -> String {
    bs58::encode(hash.0).into_string()
}

fn base58_to_hash(value: &str) -> Result<Hash, RpcError> {
    let bytes = bs58::decode(value).into_vec().map_err(|_| RpcError {
        code: -32602,
        message: "Invalid base58 signature".to_string(),
    })?;
    if bytes.len() != 32 {
        return Err(RpcError {
            code: -32602,
            message: format!("Invalid signature length: {}", bytes.len()),
        });
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&bytes);
    Ok(Hash(hash))
}

// ═══════════════════════════════════════════════════════════════════════════════
// SOLANA SERIALIZATION HELPERS
// ═══════════════════════════════════════════════════════════════════════════════

fn solana_context(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let slot = state.state.get_last_slot().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;
    Ok(serde_json::json!({
        "slot": slot,
    }))
}

fn solana_message_json(tx: &Transaction) -> (Vec<String>, Vec<serde_json::Value>) {
    let mut account_keys: Vec<Pubkey> = Vec::new();
    let mut seen: HashSet<Pubkey> = HashSet::new();

    let mut push_key = |key: Pubkey| {
        if seen.insert(key) {
            account_keys.push(key);
        }
    };

    if let Some(first_ix) = tx.message.instructions.first() {
        if let Some(first_acc) = first_ix.accounts.first() {
            push_key(*first_acc);
        }
    }

    for ix in &tx.message.instructions {
        for acc in &ix.accounts {
            push_key(*acc);
        }
    }

    for ix in &tx.message.instructions {
        push_key(ix.program_id);
    }

    let index_map: HashMap<Pubkey, usize> = account_keys
        .iter()
        .enumerate()
        .map(|(idx, key)| (*key, idx))
        .collect();

    let instructions = tx
        .message
        .instructions
        .iter()
        .map(|ix| {
            let program_id_index = *index_map.get(&ix.program_id).unwrap_or(&0);
            let account_indices: Vec<usize> = ix
                .accounts
                .iter()
                .filter_map(|acc| index_map.get(acc).copied())
                .collect();
            let data_base58 = bs58::encode(&ix.data).into_string();
            serde_json::json!({
                "programIdIndex": program_id_index,
                "accounts": account_indices,
                "data": data_base58,
            })
        })
        .collect();

    let account_keys = account_keys.iter().map(|key| key.to_base58()).collect();

    (account_keys, instructions)
}

fn solana_transaction_json(
    tx: &Transaction,
    slot: u64,
    timestamp: u64,
    fee: u64,
) -> serde_json::Value {
    let (account_keys, instructions) = solana_message_json(tx);
    let signature = hash_to_base58(&tx.signature());

    serde_json::json!({
        "slot": slot,
        "blockTime": timestamp,
        "meta": {
            "err": serde_json::Value::Null,
            "fee": fee,
            "preBalances": [],
            "postBalances": [],
            "logMessages": [],
        },
        "transaction": {
            "signatures": [signature],
            "message": {
                "accountKeys": account_keys,
                "recentBlockhash": hash_to_base58(&tx.message.recent_blockhash),
                "instructions": instructions,
            }
        }
    })
}

fn solana_transaction_encoded_json(
    tx: &Transaction,
    slot: u64,
    timestamp: u64,
    fee: u64,
    encoding: &str,
) -> serde_json::Value {
    let encoded = encode_solana_transaction(tx, encoding);

    serde_json::json!({
        "slot": slot,
        "blockTime": timestamp,
        "meta": {
            "err": serde_json::Value::Null,
            "fee": fee,
            "preBalances": [],
            "postBalances": [],
            "logMessages": [],
        },
        "transaction": [encoded, encoding],
    })
}

fn solana_block_transaction_json(tx: &Transaction, fee: u64) -> serde_json::Value {
    let (account_keys, instructions) = solana_message_json(tx);
    let signature = hash_to_base58(&tx.signature());

    serde_json::json!({
        "meta": {
            "err": serde_json::Value::Null,
            "fee": fee,
            "preBalances": [],
            "postBalances": [],
            "logMessages": [],
        },
        "transaction": {
            "signatures": [signature],
            "message": {
                "accountKeys": account_keys,
                "recentBlockhash": hash_to_base58(&tx.message.recent_blockhash),
                "instructions": instructions,
            }
        }
    })
}

fn solana_block_transaction_encoded_json(
    tx: &Transaction,
    fee: u64,
    encoding: &str,
) -> serde_json::Value {
    let encoded = encode_solana_transaction(tx, encoding);
    let signature = hash_to_base58(&tx.signature());

    serde_json::json!({
        "meta": {
            "err": serde_json::Value::Null,
            "fee": fee,
            "preBalances": [],
            "postBalances": [],
            "logMessages": [],
        },
        "transaction": [encoded, encoding],
        "version": serde_json::Value::Null,
        "signatures": [signature],
    })
}

fn encode_solana_transaction(tx: &Transaction, encoding: &str) -> String {
    let bytes = bincode::serialize(tx).unwrap_or_default();
    match encoding {
        "base58" => bs58::encode(bytes).into_string(),
        _ => {
            use base64::{engine::general_purpose, Engine as _};
            general_purpose::STANDARD.encode(bytes)
        }
    }
}

fn validate_solana_encoding(encoding: &str) -> Result<(), RpcError> {
    match encoding {
        "json" | "base58" | "base64" => Ok(()),
        _ => Err(RpcError {
            code: -32602,
            message: format!("Unsupported encoding: {}", encoding),
        }),
    }
}

fn validate_solana_transaction_details(details: &str) -> Result<(), RpcError> {
    match details {
        "full" | "signatures" | "none" => Ok(()),
        _ => Err(RpcError {
            code: -32602,
            message: format!("Unsupported transactionDetails: {}", details),
        }),
    }
}

fn filter_signatures_for_address(
    indexed: Vec<(Hash, u64)>,
    before: Option<Hash>,
    until: Option<Hash>,
    limit: usize,
) -> Vec<(Hash, u64)> {
    let mut results: Vec<(Hash, u64)> = Vec::new();
    let mut started = before.is_none();

    for (hash, slot) in indexed {
        if !started {
            if let Some(before_hash) = before {
                if hash == before_hash {
                    started = true;
                }
            }
            continue;
        }

        if let Some(until_hash) = until {
            if hash == until_hash {
                break;
            }
        }

        results.push((hash, slot));
        if results.len() >= limit {
            break;
        }
    }

    results
}

fn tx_to_rpc_json(
    tx: &Transaction,
    slot: u64,
    timestamp: u64,
    fee_config: &moltchain_core::FeeConfig,
) -> serde_json::Value {
    let first_ix = tx.message.instructions.first();
    let (tx_type, from, to, amount) = if let Some(ix) = first_ix {
        let from = ix.accounts.first().map(|acc| acc.to_base58());
        let to = ix.accounts.get(1).map(|acc| acc.to_base58());
        let amount = parse_transfer_amount(ix);
        (instruction_type(ix), from, to, amount)
    } else {
        ("Unknown", None, None, None)
    };

    let instructions: Vec<serde_json::Value> = tx
        .message
        .instructions
        .iter()
        .map(|ix| {
            let accounts: Vec<String> = ix.accounts.iter().map(|acc| acc.to_base58()).collect();
            serde_json::json!({
                "program_id": ix.program_id.to_base58(),
                "accounts": accounts,
                "data": ix.data,
            })
        })
        .collect();

    let signatures: Vec<String> = tx.signatures.iter().map(hex::encode).collect();
    let amount_molt = amount
        .map(|val| val as f64 / 1_000_000_000.0)
        .unwrap_or(0.0);

    let fee = TxProcessor::compute_transaction_fee(tx, fee_config);

    serde_json::json!({
        "signature": tx.signature().to_hex(),
        "signatures": signatures,
        "slot": slot,
        "block_time": timestamp,
        "status": "Success",
        "error": null,
        "fee": fee,
        "fee_shells": fee,
        "fee_molt": fee as f64 / 1_000_000_000.0,
        "type": tx_type,
        "from": from,
        "to": to,
        "amount": amount_molt,
        "amount_shells": amount.unwrap_or(0),
        "message": {
            "instructions": instructions,
            "recent_blockhash": tx.message.recent_blockhash.to_hex(),
        },
    })
}

// ═══════════════════════════════════════════════════════════════════════════════
// SERVER STARTUP & ROUTER
// ═══════════════════════════════════════════════════════════════════════════════

/// Start RPC server
#[allow(clippy::too_many_arguments)]
pub async fn start_rpc_server(
    state: StateStore,
    port: u16,
    tx_sender: Option<mpsc::Sender<Transaction>>,
    stake_pool: Option<Arc<Mutex<StakePool>>>,
    p2p: Option<Arc<dyn P2PNetworkTrait>>,
    chain_id: String,
    network_id: String,
    admin_token: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let app = build_rpc_router(
        state,
        tx_sender,
        stake_pool,
        p2p,
        chain_id,
        network_id,
        admin_token,
    );

    let addr = format!("0.0.0.0:{}", port);
    info!("🌐 RPC server starting on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}

pub fn build_rpc_router(
    state: StateStore,
    tx_sender: Option<mpsc::Sender<Transaction>>,
    stake_pool: Option<Arc<Mutex<StakePool>>>,
    p2p: Option<Arc<dyn P2PNetworkTrait>>,
    chain_id: String,
    network_id: String,
    admin_token: Option<String>,
) -> Router {
    let evm_chain_id = evm_chain_id_from_chain_id(&chain_id);
    let solana_tx_cache = Arc::new(Mutex::new(LruCache::new(
        NonZeroUsize::new(10_000).unwrap(),
    )));
    // Filter empty admin token to None
    let admin_token = admin_token.filter(|t| !t.is_empty());
    if admin_token.is_some() {
        info!("\u{1f512} Admin token configured for state-mutating endpoints");
    } else {
        info!("\u{26a0}\u{fe0f}  No admin token configured — setFeeConfig/setRentParams/setContractAbi disabled");
    }
    let rpc_state = RpcState {
        state,
        tx_sender,
        p2p,
        stake_pool,
        chain_id,
        network_id,
        version: env!("CARGO_PKG_VERSION").to_string(),
        evm_chain_id,
        solana_tx_cache,
        admin_token,
        rate_limiter: Arc::new(RateLimiter::new(300)),
    };

    // T2.7: Restrictive CORS — allow localhost and configured origins only
    // H14 fix: use exact host matching to prevent subdomain bypass
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _| {
            let origin_str = origin.to_str().unwrap_or("");
            // Parse scheme://host:port — only allow exact localhost/127.0.0.1 hosts
            if let Some(rest) = origin_str
                .strip_prefix("http://")
                .or_else(|| origin_str.strip_prefix("https://"))
            {
                let host = rest.split('/').next().unwrap_or("");
                let host_only = host.split(':').next().unwrap_or("");
                host_only == "localhost"
                    || host_only == "127.0.0.1"
                    || host_only.ends_with(".moltchain.io")
            } else {
                false
            }
        }))
        .allow_methods([Method::POST, Method::GET, Method::OPTIONS])
        .allow_headers([axum::http::header::CONTENT_TYPE]);

    let state = Arc::new(rpc_state);

    Router::new()
        .route("/", post(handle_rpc))
        .route("/solana", post(handle_solana_rpc))
        .route("/evm", post(handle_evm_rpc))
        // DEX REST API — /api/v1/*
        .nest("/api/v1", dex::build_dex_router())
        .layer(cors)
        // DDoS protection: limit request bodies to 2MB
        .layer(axum::extract::DefaultBodyLimit::max(2 * 1024 * 1024))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit_middleware,
        ))
        .with_state(state)
}

// ═══════════════════════════════════════════════════════════════════════════════
// RPC DISPATCH HANDLERS
// ═══════════════════════════════════════════════════════════════════════════════

/// Handle RPC request
async fn handle_rpc(State(state): State<Arc<RpcState>>, Json(req): Json<RpcRequest>) -> Response {
    // Route to appropriate handler
    let result = match req.method.as_str() {
        // Basic queries (canonical Molt endpoints)
        "getBalance" => handle_get_balance(&state, req.params).await,
        "getAccount" => handle_get_account(&state, req.params).await,
        "getBlock" => handle_get_block(&state, req.params).await,
        "getLatestBlock" => handle_get_latest_block(&state).await,
        "getSlot" => handle_get_slot(&state).await,
        "getTransaction" => handle_get_transaction(&state, req.params).await,
        "getTransactionsByAddress" => handle_get_transactions_by_address(&state, req.params).await,
        "getAccountTxCount" => handle_get_account_tx_count(&state, req.params).await,
        "getRecentTransactions" => handle_get_recent_transactions(&state, req.params).await,
        "getTokenAccounts" => handle_get_token_accounts(&state, req.params).await,
        "sendTransaction" => handle_send_transaction(&state, req.params).await,
        "simulateTransaction" => handle_simulate_transaction(&state, req.params).await,
        "getTotalBurned" => handle_get_total_burned(&state).await,
        "getValidators" => handle_get_validators(&state).await,
        "getMetrics" => handle_get_metrics(&state).await,
        "getTreasuryInfo" => handle_get_treasury_info(&state).await,
        "getRecentBlockhash" => handle_get_recent_blockhash(&state).await,
        "health" => Ok(serde_json::json!({"status": "ok"})),

        // Fee and rent config endpoints
        "getFeeConfig" => handle_get_fee_config(&state).await,
        "setFeeConfig" => handle_set_fee_config(&state, req.params).await,
        "getRentParams" => handle_get_rent_params(&state).await,
        "setRentParams" => handle_set_rent_params(&state, req.params).await,

        // Network endpoints
        "getPeers" => handle_get_peers(&state).await,
        "getNetworkInfo" => handle_get_network_info(&state).await,

        // Validator endpoints
        "getValidatorInfo" => handle_get_validator_info(&state, req.params).await,
        "getValidatorPerformance" => handle_get_validator_performance(&state, req.params).await,
        "getChainStatus" => handle_get_chain_status(&state).await,

        // Staking endpoints
        "stake" => handle_stake(&state, req.params).await,
        "unstake" => handle_unstake(&state, req.params).await,
        "getStakingStatus" => handle_get_staking_status(&state, req.params).await,
        "getStakingRewards" => handle_get_staking_rewards(&state, req.params).await,

        // ReefStake liquid staking endpoints
        "stakeToReefStake" => handle_stake_to_reefstake(&state, req.params).await,
        "unstakeFromReefStake" => handle_unstake_from_reefstake(&state, req.params).await,
        "claimUnstakedTokens" => handle_claim_unstaked_tokens(&state, req.params).await,
        "getStakingPosition" => handle_get_staking_position(&state, req.params).await,
        "getReefStakePoolInfo" => handle_get_reefstake_pool_info(&state).await,
        "getUnstakingQueue" => handle_get_unstaking_queue(&state, req.params).await,

        // Price-based rewards
        "getRewardAdjustmentInfo" => handle_get_reward_adjustment_info(&state).await,

        // Account endpoints
        "getAccountInfo" => handle_get_account_info(&state, req.params).await,
        "getTransactionHistory" => handle_get_transaction_history(&state, req.params).await,

        // Contract endpoints
        "getContractInfo" => handle_get_contract_info(&state, req.params).await,
        "getContractLogs" => handle_get_contract_logs(&state, req.params).await,
        "getContractAbi" => handle_get_contract_abi(&state, req.params).await,
        "setContractAbi" => handle_set_contract_abi(&state, req.params).await,
        "getAllContracts" => handle_get_all_contracts(&state).await,
        "deployContract" => handle_deploy_contract(&state, req.params).await,

        // Program endpoints (draft)
        "getProgram" => handle_get_program(&state, req.params).await,
        "getProgramStats" => handle_get_program_stats(&state, req.params).await,
        "getPrograms" => handle_get_programs(&state, req.params).await,
        "getProgramCalls" => handle_get_program_calls(&state, req.params).await,
        "getProgramStorage" => handle_get_program_storage(&state, req.params).await,

        // Symbol registry
        "getSymbolRegistry" => handle_get_symbol_registry(&state, req.params).await,
        "getSymbolRegistryByProgram" => {
            handle_get_symbol_registry_by_program(&state, req.params).await
        }
        "getAllSymbolRegistry" => handle_get_all_symbol_registry(&state, req.params).await,

        // NFT endpoints (draft)
        "getCollection" => handle_get_collection(&state, req.params).await,
        "getNFT" => handle_get_nft(&state, req.params).await,
        "getNFTsByOwner" => handle_get_nfts_by_owner(&state, req.params).await,
        "getNFTsByCollection" => handle_get_nfts_by_collection(&state, req.params).await,
        "getNFTActivity" => handle_get_nft_activity(&state, req.params).await,
        "getMarketListings" => handle_get_market_listings(&state, req.params).await,
        "getMarketSales" => handle_get_market_sales(&state, req.params).await,

        // Token endpoints
        "getTokenBalance" => handle_get_token_balance(&state, req.params).await,
        "getTokenHolders" => handle_get_token_holders(&state, req.params).await,
        "getTokenTransfers" => handle_get_token_transfers(&state, req.params).await,
        "getContractEvents" => handle_get_contract_events(&state, req.params).await,

        // Testnet-only faucet airdrop
        "requestAirdrop" => handle_request_airdrop(&state, req.params).await,

        _ => Err(RpcError {
            code: -32601,
            message: format!("Method not found: {}", req.method),
        }),
    };

    let response = match result {
        Ok(result) => RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: Some(result),
            error: None,
        },
        Err(error) => RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: None,
            error: Some(error),
        },
    };

    (StatusCode::OK, Json(response)).into_response()
}

/// Handle Solana-compatible RPC request
async fn handle_solana_rpc(
    State(state): State<Arc<RpcState>>,
    Json(req): Json<RpcRequest>,
) -> Response {
    let result = match req.method.as_str() {
        "getLatestBlockhash" => handle_solana_get_latest_blockhash(&state).await,
        "getRecentBlockhash" => handle_solana_get_latest_blockhash(&state).await,
        "getBalance" => handle_solana_get_balance(&state, req.params).await,
        "getAccountInfo" => handle_solana_get_account_info(&state, req.params).await,
        "getBlock" => handle_solana_get_block(&state, req.params).await,
        "getBlockHeight" => handle_solana_get_block_height(&state).await,
        "getSignaturesForAddress" => {
            handle_solana_get_signatures_for_address(&state, req.params).await
        }
        "getSignatureStatuses" => handle_solana_get_signature_statuses(&state, req.params).await,
        "getSlot" => handle_solana_get_slot(&state).await,
        "getTransaction" => handle_solana_get_transaction(&state, req.params).await,
        "sendTransaction" => handle_solana_send_transaction(&state, req.params).await,
        "getHealth" => Ok(serde_json::json!("ok")),
        "getVersion" => Ok(serde_json::json!({"solana-core": "moltchain", "feature-set": 0})),
        _ => Err(RpcError {
            code: -32601,
            message: format!("Method not found: {}", req.method),
        }),
    };

    let response = match result {
        Ok(result) => RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: Some(result),
            error: None,
        },
        Err(error) => RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: None,
            error: Some(error),
        },
    };

    (StatusCode::OK, Json(response)).into_response()
}

/// Handle Ethereum-compatible RPC request
async fn handle_evm_rpc(
    State(state): State<Arc<RpcState>>,
    Json(req): Json<RpcRequest>,
) -> Response {
    let result = match req.method.as_str() {
        "eth_getBalance" => handle_eth_get_balance(&state, req.params).await,
        "eth_sendRawTransaction" => handle_eth_send_raw_transaction(&state, req.params).await,
        "eth_call" => handle_eth_call(&state, req.params).await,
        "eth_chainId" => Ok(serde_json::json!(format!("0x{:x}", state.evm_chain_id))),
        "eth_blockNumber" => handle_eth_block_number(&state).await,
        "eth_getTransactionReceipt" => handle_eth_get_transaction_receipt(&state, req.params).await,
        "eth_getTransactionByHash" => handle_eth_get_transaction_by_hash(&state, req.params).await,
        "eth_accounts" => Ok(serde_json::json!([])), // No accounts (users use MetaMask)
        "net_version" => Ok(serde_json::json!("1297368660")), // "Molt" as decimal
        _ => Err(RpcError {
            code: -32601,
            message: format!("Method not found: {}", req.method),
        }),
    };

    let response = match result {
        Ok(result) => RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: Some(result),
            error: None,
        },
        Err(error) => RpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id,
            result: None,
            error: Some(error),
        },
    };

    (StatusCode::OK, Json(response)).into_response()
}

// ═══════════════════════════════════════════════════════════════════════════════
// NATIVE MOLT RPC METHODS
// ═══════════════════════════════════════════════════════════════════════════════

async fn handle_get_symbol_registry(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let symbol = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [symbol]".to_string(),
        })?;

    let entry = state
        .state
        .get_symbol_registry(symbol)
        .map_err(|e| RpcError {
            code: -32000,
            message: e,
        })?;

    Ok(entry
        .map(symbol_registry_entry_to_json)
        .unwrap_or(serde_json::Value::Null))
}

async fn handle_get_symbol_registry_by_program(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let program = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [program_id]".to_string(),
        })?;

    let program = Pubkey::from_base58(program).map_err(|_| RpcError {
        code: -32602,
        message: "Invalid pubkey format".to_string(),
    })?;

    let entry = state
        .state
        .get_symbol_registry_by_program(&program)
        .map_err(|e| RpcError {
            code: -32000,
            message: e,
        })?;

    Ok(entry
        .map(symbol_registry_entry_to_json)
        .unwrap_or(serde_json::Value::Null))
}

async fn handle_get_all_symbol_registry(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let limit = params
        .and_then(|val| {
            if val.is_array() {
                val.as_array()
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.as_u64())
            } else if val.is_object() {
                val.get("limit").and_then(|v| v.as_u64())
            } else {
                val.as_u64()
            }
        })
        .unwrap_or(500)
        .min(2000) as usize;

    let entries = state
        .state
        .get_all_symbol_registry(limit)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let list: Vec<serde_json::Value> = entries
        .into_iter()
        .map(symbol_registry_entry_to_json)
        .collect();

    Ok(serde_json::json!({
        "entries": list,
        "count": list.len(),
    }))
}

fn symbol_registry_entry_to_json(entry: SymbolRegistryEntry) -> serde_json::Value {
    serde_json::json!({
        "symbol": entry.symbol,
        "program": entry.program.to_base58(),
        "owner": entry.owner.to_base58(),
        "name": entry.name,
        "template": entry.template,
        "metadata": entry.metadata,
    })
}

/// Get balance
async fn handle_get_balance(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey]".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid pubkey: {}", e),
    })?;

    let account = state.state.get_account(&pubkey).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    match account {
        Some(acc) => {
            // Convert helper for shells to MOLT (with precision)
            let to_molt_str =
                |shells: u64| -> String { format!("{:.4}", shells as f64 / 1_000_000_000.0) };

            Ok(serde_json::json!({
                // Total balance (backward compatible)
                "shells": acc.shells,
                "molt": to_molt_str(acc.shells),

                // Balance breakdown (NEW)
                "spendable": acc.spendable,
                "spendable_molt": to_molt_str(acc.spendable),

                "staked": acc.staked,
                "staked_molt": to_molt_str(acc.staked),

                "locked": acc.locked,
                "locked_molt": to_molt_str(acc.locked),
            }))
        }
        None => {
            // Account doesn't exist - return all zeros
            Ok(serde_json::json!({
                "shells": 0,
                "molt": "0.0000",
                "spendable": 0,
                "spendable_molt": "0.0000",
                "staked": 0,
                "staked_molt": "0.0000",
                "locked": 0,
                "locked_molt": "0.0000",
            }))
        }
    }
}

/// Get account
async fn handle_get_account(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey]".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid pubkey: {}", e),
    })?;

    let account = state.state.get_account(&pubkey).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    match account {
        Some(acc) => {
            let to_molt_str =
                |shells: u64| -> String { format!("{:.4}", shells as f64 / 1_000_000_000.0) };

            Ok(serde_json::json!({
                "pubkey": pubkey.to_base58(),
                "evm_address": pubkey.to_evm(),
                "shells": acc.shells,
                "molt": to_molt_str(acc.shells),
                "spendable": acc.spendable,
                "spendable_molt": to_molt_str(acc.spendable),
                "staked": acc.staked,
                "staked_molt": to_molt_str(acc.staked),
                "locked": acc.locked,
                "locked_molt": to_molt_str(acc.locked),
                "owner": acc.owner.to_base58(),
                "executable": acc.executable,
                "data_len": acc.data.len(),
            }))
        }
        None => Err(RpcError {
            code: -32001,
            message: "Account not found".to_string(),
        }),
    }
}

/// Get block
async fn handle_get_block(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let slot = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_u64())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [slot]".to_string(),
        })?;

    let block = state.state.get_block_by_slot(slot).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    match block {
        Some(block) => {
            let fee_config = state
                .state
                .get_fee_config()
                .unwrap_or_else(|_| moltchain_core::FeeConfig::default_from_constants());
            let block_hash = block.hash();
            let transactions: Vec<serde_json::Value> = block
                .transactions
                .iter()
                .map(|tx| {
                    tx_to_rpc_json(tx, block.header.slot, block.header.timestamp, &fee_config)
                })
                .collect();

            // Protocol-level block reward (coinbase) — deterministic, not a transaction
            let has_user_txs = block.transactions.iter().any(|tx| {
                tx.message
                    .instructions
                    .first()
                    .map(|ix| !matches!(ix.data.first(), Some(2) | Some(3)))
                    .unwrap_or(true)
            });
            let reward_amount = if block.header.slot == 0 || block.header.validator == [0u8; 32] {
                0
            } else if has_user_txs {
                TRANSACTION_BLOCK_REWARD
            } else {
                HEARTBEAT_BLOCK_REWARD
            };

            Ok(serde_json::json!({
                "slot": block.header.slot,
                "hash": block_hash.to_hex(),
                "parent_hash": block.header.parent_hash.to_hex(),
                "state_root": block.header.state_root.to_hex(),
                "tx_root": block.header.tx_root.to_hex(),
                "timestamp": block.header.timestamp,
                "validator": Pubkey(block.header.validator).to_base58(),
                "transaction_count": block.transactions.len(),
                "transactions": transactions,
                "block_reward": {
                    "amount": reward_amount,
                    "amount_molt": reward_amount as f64 / 1_000_000_000.0,
                    "type": if has_user_txs { "transaction" } else { "heartbeat" },
                    "recipient": Pubkey(block.header.validator).to_base58(),
                },
            }))
        }
        None => Err(RpcError {
            code: -32001,
            message: "Block not found".to_string(),
        }),
    }
}

/// Get current slot
async fn handle_get_slot(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let slot = state.state.get_last_slot().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    Ok(serde_json::json!(slot))
}

/// Get recent blockhash for transaction building
async fn handle_get_recent_blockhash(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let slot = state.state.get_last_slot().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    if slot == 0 {
        return Err(RpcError {
            code: -32001,
            message: "No blocks yet".to_string(),
        });
    }

    // Get the latest block's hash
    let block = state.state.get_block_by_slot(slot).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    match block {
        Some(block) => {
            let block_hash = block.hash();
            Ok(serde_json::json!({
                "blockhash": block_hash.to_hex(),
                "slot": slot,
            }))
        }
        None => Err(RpcError {
            code: -32001,
            message: "Latest block not found".to_string(),
        }),
    }
}

async fn handle_get_fee_config(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let config = state.state.get_fee_config().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    Ok(serde_json::json!({
        "base_fee_shells": config.base_fee,
        "contract_deploy_fee_shells": config.contract_deploy_fee,
        "contract_upgrade_fee_shells": config.contract_upgrade_fee,
        "nft_mint_fee_shells": config.nft_mint_fee,
        "nft_collection_fee_shells": config.nft_collection_fee,
    }))
}

async fn handle_set_fee_config(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    verify_admin_auth(state, &params)?;

    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let obj = params.as_object().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected object".to_string(),
    })?;

    let mut config = state.state.get_fee_config().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    if let Some(value) = obj.get("base_fee_shells").and_then(|v| v.as_u64()) {
        config.base_fee = value;
    }
    if let Some(value) = obj
        .get("contract_deploy_fee_shells")
        .and_then(|v| v.as_u64())
    {
        config.contract_deploy_fee = value;
    }
    if let Some(value) = obj
        .get("contract_upgrade_fee_shells")
        .and_then(|v| v.as_u64())
    {
        config.contract_upgrade_fee = value;
    }
    if let Some(value) = obj.get("nft_mint_fee_shells").and_then(|v| v.as_u64()) {
        config.nft_mint_fee = value;
    }
    if let Some(value) = obj
        .get("nft_collection_fee_shells")
        .and_then(|v| v.as_u64())
    {
        config.nft_collection_fee = value;
    }
    if let Some(value) = obj.get("fee_burn_percent").and_then(|v| v.as_u64()) {
        if value <= 100 {
            config.fee_burn_percent = value;
        }
    }
    if let Some(value) = obj.get("fee_producer_percent").and_then(|v| v.as_u64()) {
        if value <= 100 {
            config.fee_producer_percent = value;
        }
    }
    if let Some(value) = obj.get("fee_voters_percent").and_then(|v| v.as_u64()) {
        if value <= 100 {
            config.fee_voters_percent = value;
        }
    }
    if let Some(value) = obj.get("fee_treasury_percent").and_then(|v| v.as_u64()) {
        if value <= 100 {
            config.fee_treasury_percent = value;
        }
    }

    // Validate that fee distribution percentages sum to 100
    let pct_sum = config.fee_burn_percent
        + config.fee_producer_percent
        + config.fee_voters_percent
        + config.fee_treasury_percent;
    if pct_sum != 100 {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Fee percentages must sum to 100, got {} (burn={}, producer={}, voters={}, treasury={})",
                pct_sum, config.fee_burn_percent, config.fee_producer_percent,
                config.fee_voters_percent, config.fee_treasury_percent,
            ),
        });
    }

    state
        .state
        .set_fee_config_full(&config)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    Ok(serde_json::json!({"status": "ok"}))
}

async fn handle_get_rent_params(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let (rate, free_kb) = state.state.get_rent_params().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    Ok(serde_json::json!({
        "rent_rate_shells_per_kb_month": rate,
        "rent_free_kb": free_kb,
    }))
}

async fn handle_set_rent_params(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    verify_admin_auth(state, &params)?;

    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let obj = params.as_object().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected object".to_string(),
    })?;

    let (mut rate, mut free_kb) = state.state.get_rent_params().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    if let Some(value) = obj
        .get("rent_rate_shells_per_kb_month")
        .and_then(|v| v.as_u64())
    {
        rate = value;
    }
    if let Some(value) = obj.get("rent_free_kb").and_then(|v| v.as_u64()) {
        free_kb = value;
    }

    state
        .state
        .set_rent_params(rate, free_kb)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    Ok(serde_json::json!({"status": "ok"}))
}

// ═══════════════════════════════════════════════════════════════════════════════
// CORE TRANSACTION METHODS
// ═══════════════════════════════════════════════════════════════════════════════

/// Get transaction
async fn handle_get_transaction(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let sig_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [signature]".to_string(),
        })?;

    let sig_hash = moltchain_core::Hash::from_hex(sig_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid signature: {}", e),
    })?;

    let tx = state
        .state
        .get_transaction(&sig_hash)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    // T8.2: Use tx→slot reverse index for O(1) lookup, fall back to scan
    let (slot, timestamp) = match state.state.get_tx_slot(&sig_hash) {
        Ok(Some(slot)) => {
            let ts = state
                .state
                .get_block_by_slot(slot)
                .ok()
                .flatten()
                .map(|b| b.header.timestamp)
                .unwrap_or(0);
            (slot, ts)
        }
        _ => {
            // Fallback: reverse scan (for txs indexed before the reverse index existed)
            // M20 fix: cap fallback scan to prevent DoS via non-existent tx lookups
            if let Ok(last_slot) = state.state.get_last_slot() {
                let mut found = None;
                let scan_limit = 1000u64; // max slots to scan backwards
                let start_slot = last_slot;
                let end_slot = start_slot.saturating_sub(scan_limit);
                for slot in (end_slot..=start_slot).rev() {
                    if let Ok(Some(block)) = state.state.get_block_by_slot(slot) {
                        if block
                            .transactions
                            .iter()
                            .any(|tx| tx.signature() == sig_hash)
                        {
                            found = Some((slot, block.header.timestamp));
                            break;
                        }
                    }
                }
                found.unwrap_or((0, 0))
            } else {
                (0, 0)
            }
        }
    };

    let fee_config = state
        .state
        .get_fee_config()
        .unwrap_or_else(|_| moltchain_core::FeeConfig::default_from_constants());

    match tx {
        Some(tx) => Ok(tx_to_rpc_json(&tx, slot, timestamp, &fee_config)),
        None => {
            // Fallback: look inside the block itself (covers genesis txs
            // and any tx that wasn't individually stored)
            if let Ok(Some(block)) = state.state.get_block_by_slot(slot) {
                for block_tx in &block.transactions {
                    if block_tx.signature() == sig_hash {
                        return Ok(tx_to_rpc_json(block_tx, slot, timestamp, &fee_config));
                    }
                }
            }
            Err(RpcError {
                code: -32001,
                message: "Transaction not found".to_string(),
            })
        }
    }
}

/// Get transactions involving a specific address
/// Get transactions involving a specific address (cursor-paginated, newest first)
/// params: [address, { limit?, before_slot? }]
async fn handle_get_transactions_by_address(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let params_array = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [address, options]".to_string(),
    })?;

    let address_str = params_array
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [address, options]".to_string(),
        })?;

    let opts = params_array.get(1);
    let limit = opts
        .and_then(|v| v.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(500) as usize;

    let before_slot = opts
        .and_then(|v| v.get("before_slot"))
        .and_then(|v| v.as_u64());

    let target = Pubkey::from_base58(address_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid address: {}", e),
    })?;

    let fee_config = state
        .state
        .get_fee_config()
        .unwrap_or_else(|_| moltchain_core::FeeConfig::default_from_constants());

    // Use paginated reverse-iterator method (O(limit), not O(all txs))
    let indexed = state
        .state
        .get_account_tx_signatures_paginated(&target, limit, before_slot)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut timestamps: HashMap<u64, u64> = HashMap::new();
    let mut last_slot: Option<u64> = None;

    for (hash, slot) in &indexed {
        let tx = match state.state.get_transaction(hash) {
            Ok(Some(tx)) => tx,
            _ => {
                if let Ok(Some(block)) = state.state.get_block_by_slot(*slot) {
                    match block.transactions.iter().find(|t| t.signature() == *hash) {
                        Some(t) => t.clone(),
                        None => continue,
                    }
                } else {
                    continue;
                }
            }
        };

        let timestamp = if let Some(cached) = timestamps.get(slot) {
            *cached
        } else {
            let ts = state
                .state
                .get_block_by_slot(*slot)
                .ok()
                .and_then(|block| block.map(|b| b.header.timestamp))
                .unwrap_or(0);
            timestamps.insert(*slot, ts);
            ts
        };

        let first_ix = tx.message.instructions.first();
        let (tx_type, from, to, amount) = if let Some(ix) = first_ix {
            let from = ix
                .accounts
                .first()
                .map(|acc| acc.to_base58())
                .unwrap_or_default();
            let to = ix
                .accounts
                .get(1)
                .map(|acc| acc.to_base58())
                .unwrap_or_default();
            let amount = parse_transfer_amount(ix).unwrap_or(0);
            (instruction_type(ix), from, to, amount)
        } else {
            ("Unknown", String::new(), String::new(), 0)
        };

        let fee = TxProcessor::compute_transaction_fee(&tx, &fee_config);

        results.push(serde_json::json!({
            "hash": tx.signature().to_hex(),
            "signature": tx.signature().to_hex(),
            "slot": slot,
            "timestamp": timestamp,
            "from": from,
            "to": to,
            "type": tx_type,
            "amount": amount as f64 / 1_000_000_000.0,
            "amount_shells": amount,
            "fee": fee,
            "fee_shells": fee,
            "fee_molt": fee as f64 / 1_000_000_000.0,
            "success": true,
        }));

        last_slot = Some(*slot);
    }

    // Return with pagination cursor
    let has_more = results.len() == limit;
    Ok(serde_json::json!({
        "transactions": results,
        "has_more": has_more,
        "next_before_slot": if has_more { last_slot } else { None::<u64> },
    }))
}

/// Get recent transactions across all addresses (cursor-paginated via CF_TX_BY_SLOT)
/// params: [{ limit?, before_slot? }]  or  []
async fn handle_get_recent_transactions(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let opts = params
        .as_ref()
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first());

    let limit = opts
        .and_then(|v| v.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(500) as usize;

    let before_slot = opts
        .and_then(|v| v.get("before_slot"))
        .and_then(|v| v.as_u64());

    let fee_config = state
        .state
        .get_fee_config()
        .unwrap_or_else(|_| moltchain_core::FeeConfig::default_from_constants());

    let indexed = state
        .state
        .get_recent_txs(limit, before_slot)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let mut results: Vec<serde_json::Value> = Vec::new();
    let mut timestamps: HashMap<u64, u64> = HashMap::new();
    let mut last_slot: Option<u64> = None;

    for (hash, slot) in &indexed {
        let tx = match state.state.get_transaction(hash) {
            Ok(Some(tx)) => tx,
            _ => continue,
        };

        let timestamp = if let Some(cached) = timestamps.get(slot) {
            *cached
        } else {
            let ts = state
                .state
                .get_block_by_slot(*slot)
                .ok()
                .and_then(|block| block.map(|b| b.header.timestamp))
                .unwrap_or(0);
            timestamps.insert(*slot, ts);
            ts
        };

        let first_ix = tx.message.instructions.first();
        let (tx_type, from, to, amount) = if let Some(ix) = first_ix {
            let from = ix
                .accounts
                .first()
                .map(|acc| acc.to_base58())
                .unwrap_or_default();
            let to = ix
                .accounts
                .get(1)
                .map(|acc| acc.to_base58())
                .unwrap_or_default();
            let amount = parse_transfer_amount(ix).unwrap_or(0);
            (instruction_type(ix), from, to, amount)
        } else {
            ("Unknown", String::new(), String::new(), 0)
        };

        let fee = TxProcessor::compute_transaction_fee(&tx, &fee_config);

        results.push(serde_json::json!({
            "hash": tx.signature().to_hex(),
            "signature": tx.signature().to_hex(),
            "slot": slot,
            "timestamp": timestamp,
            "from": from,
            "to": to,
            "type": tx_type,
            "amount": amount as f64 / 1_000_000_000.0,
            "amount_shells": amount,
            "fee": fee,
            "fee_shells": fee,
            "fee_molt": fee as f64 / 1_000_000_000.0,
            "success": true,
        }));

        last_slot = Some(*slot);
    }

    let has_more = results.len() == limit;
    Ok(serde_json::json!({
        "transactions": results,
        "has_more": has_more,
        "next_before_slot": if has_more { last_slot } else { None::<u64> },
    }))
}

/// Get all token accounts for a holder with balances and symbol info
/// params: [holder_address]
async fn handle_get_token_accounts(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let holder_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Expected [holder_address]".to_string(),
        })?;

    let holder = Pubkey::from_base58(holder_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid holder: {}", e),
    })?;

    let token_balances = state
        .state
        .get_holder_token_balances(&holder, 100)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let mut accounts: Vec<serde_json::Value> = Vec::new();
    for (token_program, balance) in &token_balances {
        // Try to get symbol info from registry
        let registry = state
            .state
            .get_symbol_registry_by_program(token_program)
            .ok()
            .flatten();

        let symbol = registry
            .as_ref()
            .map(|r| r.symbol.clone())
            .unwrap_or_else(|| "Unknown".to_string());
        let name = registry
            .as_ref()
            .and_then(|r| r.name.clone())
            .unwrap_or_default();
        let decimals = registry
            .as_ref()
            .and_then(|r| r.metadata.as_ref())
            .and_then(|m| m.get("decimals"))
            .and_then(|d| d.as_u64())
            .unwrap_or(9);

        let ui_amount = *balance as f64 / 10_f64.powi(decimals as i32);

        accounts.push(serde_json::json!({
            "mint": token_program.to_base58(),
            "balance": balance,
            "ui_amount": ui_amount,
            "decimals": decimals,
            "symbol": symbol,
            "name": name,
        }));
    }

    Ok(serde_json::json!({
        "accounts": accounts,
        "count": accounts.len(),
    }))
}

async fn handle_get_account_tx_count(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let address_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [address]".to_string(),
        })?;

    let target = Pubkey::from_base58(address_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid address: {}", e),
    })?;

    let count = state
        .state
        .count_account_txs(&target)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    Ok(serde_json::json!({
        "address": target.to_base58(),
        "count": count,
    }))
}

fn submit_transaction(state: &RpcState, tx: Transaction) -> Result<String, RpcError> {
    let signature_hash = tx.signature();

    if let Some(ref sender) = state.tx_sender {
        sender.try_send(tx).map_err(|e| RpcError {
            code: -32003,
            message: format!("Transaction queue full, try again later: {}", e),
        })?;
        info!(
            "📮 Transaction submitted to mempool: {}",
            signature_hash.to_hex()
        );
    } else {
        return Err(RpcError {
            code: -32000,
            message: "Node is not accepting transactions (no mempool configured)".to_string(),
        });
    }

    Ok(signature_hash.to_hex())
}

fn decode_solana_transaction(
    payload: &str,
    encoding: Option<&str>,
) -> Result<Transaction, RpcError> {
    use base64::{engine::general_purpose, Engine as _};

    let tx_bytes = match encoding {
        Some("base58") => bs58::decode(payload).into_vec().map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid base58: {}", e),
        })?,
        Some("base64") | None => {
            let decoded = general_purpose::STANDARD.decode(payload);
            match decoded {
                Ok(bytes) => bytes,
                Err(_) => bs58::decode(payload).into_vec().map_err(|e| RpcError {
                    code: -32602,
                    message: format!("Invalid base58: {}", e),
                })?,
            }
        }
        Some(other) => {
            return Err(RpcError {
                code: -32602,
                message: format!("Unsupported encoding: {}", other),
            })
        }
    };

    bincode::deserialize(&tx_bytes).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid transaction: {}", e),
    })
}

/// Send transaction
async fn handle_send_transaction(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let tx_base64 = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [transaction_base64]".to_string(),
        })?;

    // Decode base64 transaction
    use base64::{engine::general_purpose, Engine as _};
    let tx_bytes = general_purpose::STANDARD
        .decode(tx_base64)
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid base64: {}", e),
        })?;

    // Deserialize transaction
    let tx: Transaction = bincode::deserialize(&tx_bytes).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid transaction: {}", e),
    })?;

    // DDoS protection: basic pre-mempool validation
    // Reject transactions with empty signatures (forged zero-sig attacks)
    if tx.signatures.is_empty() {
        return Err(RpcError {
            code: -32003,
            message: "Transaction has no signatures".to_string(),
        });
    }
    // Reject zero signatures (all bytes 0x00)
    for sig in &tx.signatures {
        if sig.iter().all(|&b| b == 0) {
            return Err(RpcError {
                code: -32003,
                message: "Transaction contains an invalid zero signature".to_string(),
            });
        }
    }
    // Reject transactions with no instructions
    if tx.message.instructions.is_empty() {
        return Err(RpcError {
            code: -32003,
            message: "Transaction has no instructions".to_string(),
        });
    }

    let signature = submit_transaction(state, tx)?;

    Ok(serde_json::json!(signature))
}

async fn handle_simulate_transaction(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let tx_base64 = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [transaction_base64]".to_string(),
        })?;

    // Decode base64 transaction
    use base64::{engine::general_purpose, Engine as _};
    let tx_bytes = general_purpose::STANDARD
        .decode(tx_base64)
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid base64: {}", e),
        })?;

    // Deserialize transaction
    let tx: Transaction = bincode::deserialize(&tx_bytes).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid transaction: {}", e),
    })?;

    let processor = TxProcessor::new(state.state.clone());
    let result = processor.simulate_transaction(&tx);

    let return_data_b64 = result.return_data.as_ref().map(|data| {
        use base64::{engine::general_purpose, Engine as _};
        general_purpose::STANDARD.encode(data)
    });

    Ok(serde_json::json!({
        "success": result.success,
        "fee": result.fee,
        "logs": result.logs,
        "error": result.error,
        "computeUsed": result.compute_used,
        "returnData": return_data_b64,
    }))
}

// ============================================================================
// SOLANA-COMPATIBLE ENDPOINTS
// ============================================================================

async fn handle_solana_get_latest_blockhash(
    state: &RpcState,
) -> Result<serde_json::Value, RpcError> {
    let slot = state.state.get_last_slot().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let block_hash = state
        .state
        .get_block_by_slot(slot)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .map(|block| block.hash())
        .unwrap_or_default();

    Ok(serde_json::json!({
        "context": solana_context(state)?,
        "value": {
            "blockhash": hash_to_base58(&block_hash),
            "lastValidBlockHeight": slot,
        }
    }))
}

async fn handle_solana_get_slot(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let slot = state.state.get_last_slot().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    Ok(serde_json::json!(slot))
}

async fn handle_solana_get_block_height(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let slot = state.state.get_last_slot().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    Ok(serde_json::json!(slot))
}

async fn handle_solana_get_signature_statuses(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let params_array = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [signatures, options?]".to_string(),
    })?;

    let signatures = params_array
        .first()
        .and_then(|v| v.as_array())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [signatures, options?]".to_string(),
        })?;

    let last_slot = state.state.get_last_slot().unwrap_or(0);

    let mut values: Vec<serde_json::Value> = Vec::with_capacity(signatures.len());

    for sig_value in signatures {
        let sig_str = match sig_value.as_str() {
            Some(value) => value,
            None => {
                values.push(serde_json::Value::Null);
                continue;
            }
        };

        let sig_hash = match base58_to_hash(sig_str) {
            Ok(value) => value,
            Err(_) => {
                values.push(serde_json::Value::Null);
                continue;
            }
        };

        let mut found = false;
        if state.solana_tx_cache.lock().await.contains(&sig_hash) {
            found = true;
        } else if let Ok(Some(_)) = state.state.get_transaction(&sig_hash) {
            found = true;
        }

        if found {
            values.push(serde_json::json!({
                "slot": last_slot,
                "confirmations": serde_json::Value::Null,
                "err": serde_json::Value::Null,
                "confirmationStatus": "finalized",
            }));
        } else {
            values.push(serde_json::Value::Null);
        }
    }

    Ok(serde_json::json!({
        "context": solana_context(state)?,
        "value": values,
    }))
}

async fn handle_solana_get_signatures_for_address(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let params_array = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [address, options?]".to_string(),
    })?;

    let address_str = params_array
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [address, options?]".to_string(),
        })?;

    let options = params_array.get(1).and_then(|v| v.as_object());
    let limit = options
        .and_then(|opts| opts.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(100)
        .min(1000) as usize;

    let before = if let Some(value) = options
        .and_then(|opts| opts.get("before"))
        .and_then(|v| v.as_str())
    {
        Some(base58_to_hash(value)?)
    } else {
        None
    };

    let until = if let Some(value) = options
        .and_then(|opts| opts.get("until"))
        .and_then(|v| v.as_str())
    {
        Some(base58_to_hash(value)?)
    } else {
        None
    };

    let target = Pubkey::from_base58(address_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid address: {}", e),
    })?;

    let fetch_limit = limit.saturating_add(1).min(1000);
    let indexed = state
        .state
        .get_account_tx_signatures(&target, fetch_limit)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let filtered = filter_signatures_for_address(indexed, before, until, limit);

    let mut timestamps: HashMap<u64, u64> = HashMap::new();
    let mut results: Vec<serde_json::Value> = Vec::new();

    for (hash, slot) in filtered {
        let block_time = if let Some(cached) = timestamps.get(&slot) {
            *cached
        } else {
            let timestamp = state
                .state
                .get_block_by_slot(slot)
                .ok()
                .and_then(|block| block.map(|b| b.header.timestamp))
                .unwrap_or(0);
            timestamps.insert(slot, timestamp);
            timestamp
        };

        results.push(serde_json::json!({
            "signature": hash_to_base58(&hash),
            "slot": slot,
            "err": serde_json::Value::Null,
            "memo": serde_json::Value::Null,
            "blockTime": block_time,
            "confirmationStatus": "finalized",
        }));
    }

    Ok(serde_json::json!(results))
}

async fn handle_solana_get_balance(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey]".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid pubkey: {}", e),
    })?;

    let balance = state.state.get_balance(&pubkey).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    Ok(serde_json::json!({
        "context": solana_context(state)?,
        "value": balance,
    }))
}

async fn handle_solana_get_account_info(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let params_array = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [pubkey, options?]".to_string(),
    })?;

    let pubkey_str = params_array
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey, options?]".to_string(),
        })?;

    let encoding = params_array
        .get(1)
        .and_then(|v| v.get("encoding"))
        .and_then(|v| v.as_str())
        .unwrap_or("base64");

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid pubkey: {}", e),
    })?;

    let account = state.state.get_account(&pubkey).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let value = match account {
        Some(account) => {
            let data = match encoding {
                "base64" => {
                    use base64::{engine::general_purpose, Engine as _};
                    general_purpose::STANDARD.encode(&account.data)
                }
                "base58" => bs58::encode(&account.data).into_string(),
                other => {
                    return Err(RpcError {
                        code: -32602,
                        message: format!("Unsupported encoding: {}", other),
                    })
                }
            };

            serde_json::json!({
                "data": [data, encoding],
                "executable": account.executable,
                "lamports": account.shells,
                "owner": account.owner.to_base58(),
                "rentEpoch": account.rent_epoch,
                "space": account.data.len(),
            })
        }
        None => serde_json::Value::Null,
    };

    Ok(serde_json::json!({
        "context": solana_context(state)?,
        "value": value,
    }))
}

async fn handle_solana_get_block(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let params_array = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [slot, options?]".to_string(),
    })?;

    let slot = params_array
        .first()
        .and_then(|v| v.as_u64())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [slot, options?]".to_string(),
        })?;

    let options = params_array.get(1).and_then(|v| v.as_object());
    let transaction_details = options
        .and_then(|opts| opts.get("transactionDetails"))
        .and_then(|v| v.as_str())
        .unwrap_or("full");
    let encoding = options
        .and_then(|opts| opts.get("encoding"))
        .and_then(|v| v.as_str())
        .unwrap_or("json");

    validate_solana_transaction_details(transaction_details)?;
    validate_solana_encoding(encoding)?;

    let block = state.state.get_block_by_slot(slot).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let block = match block {
        Some(block) => block,
        None => return Ok(serde_json::Value::Null),
    };

    let transactions = match transaction_details {
        "none" => serde_json::Value::Null,
        "signatures" => serde_json::Value::Array(
            block
                .transactions
                .iter()
                .map(|tx| serde_json::Value::String(hash_to_base58(&tx.signature())))
                .collect(),
        ),
        _ => {
            let entries = block
                .transactions
                .iter()
                .map(|tx| {
                    if encoding == "base64" || encoding == "base58" {
                        solana_block_transaction_encoded_json(tx, 0, encoding)
                    } else {
                        solana_block_transaction_json(tx, 0)
                    }
                })
                .collect::<Vec<_>>();
            serde_json::Value::Array(entries)
        }
    };

    Ok(serde_json::json!({
        "blockHeight": block.header.slot,
        "blockTime": block.header.timestamp,
        "blockhash": hash_to_base58(&block.hash()),
        "previousBlockhash": hash_to_base58(&block.header.parent_hash),
        "parentSlot": block.header.slot.saturating_sub(1),
        "transactions": transactions,
    }))
}

async fn handle_solana_get_transaction(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let params_array = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [signature, options?]".to_string(),
    })?;

    let sig_str = params_array
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [signature, options?]".to_string(),
        })?;

    let encoding = params_array
        .get(1)
        .and_then(|v| v.get("encoding"))
        .and_then(|v| v.as_str())
        .unwrap_or("json");

    validate_solana_encoding(encoding)?;

    let sig_hash = base58_to_hash(sig_str)?;

    if let Some(record) = state.solana_tx_cache.lock().await.get(&sig_hash).cloned() {
        if encoding == "base64" || encoding == "base58" {
            return Ok(solana_transaction_encoded_json(
                &record.tx,
                record.slot,
                record.timestamp,
                record.fee,
                encoding,
            ));
        }
        return Ok(solana_transaction_json(
            &record.tx,
            record.slot,
            record.timestamp,
            record.fee,
        ));
    }

    let tx = state
        .state
        .get_transaction(&sig_hash)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    // T8.2: Use tx→slot reverse index for O(1) lookup, fall back to scan
    let (slot, timestamp) = match state.state.get_tx_slot(&sig_hash) {
        Ok(Some(slot)) => {
            let ts = state
                .state
                .get_block_by_slot(slot)
                .ok()
                .flatten()
                .map(|b| b.header.timestamp)
                .unwrap_or(0);
            (slot, ts)
        }
        _ => {
            // Fallback: reverse scan (for txs indexed before the reverse index existed)
            if let Ok(last_slot) = state.state.get_last_slot() {
                let mut found = None;
                for slot in (0..=last_slot).rev() {
                    if let Ok(Some(block)) = state.state.get_block_by_slot(slot) {
                        if block
                            .transactions
                            .iter()
                            .any(|block_tx| block_tx.signature() == sig_hash)
                        {
                            found = Some((slot, block.header.timestamp));
                            break;
                        }
                    }
                }
                found.unwrap_or((0, 0))
            } else {
                (0, 0)
            }
        }
    };

    let tx = match tx {
        Some(tx) => tx,
        None => {
            // Fallback: look inside the block itself
            if let Ok(Some(block)) = state.state.get_block_by_slot(slot) {
                match block
                    .transactions
                    .iter()
                    .find(|t| t.signature() == sig_hash)
                {
                    Some(t) => t.clone(),
                    None => return Ok(serde_json::Value::Null),
                }
            } else {
                return Ok(serde_json::Value::Null);
            }
        }
    };

    if encoding == "base64" || encoding == "base58" {
        Ok(solana_transaction_encoded_json(
            &tx, slot, timestamp, 0, encoding,
        ))
    } else {
        Ok(solana_transaction_json(&tx, slot, timestamp, 0))
    }
}

async fn handle_solana_send_transaction(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let params_array = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [transaction, options?]".to_string(),
    })?;

    let tx_payload = params_array
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [transaction, options?]".to_string(),
        })?;

    let encoding = params_array
        .get(1)
        .and_then(|v| v.get("encoding"))
        .and_then(|v| v.as_str());

    let tx = decode_solana_transaction(tx_payload, encoding)?;
    let signature_hash = tx.signature();
    let signature_base58 = hash_to_base58(&signature_hash);

    let slot = state.state.get_last_slot().unwrap_or(0);
    let timestamp = state
        .state
        .get_block_by_slot(slot)
        .ok()
        .flatten()
        .map(|block| block.header.timestamp)
        .unwrap_or(0);

    // H15 fix: submit first, cache only on success (was caching before submit)
    submit_transaction(state, tx.clone())?;

    state.solana_tx_cache.lock().await.put(
        signature_hash,
        SolanaTxRecord {
            tx,
            slot,
            timestamp,
            fee: 0,
        },
    );

    Ok(serde_json::json!(signature_base58))
}

/// Get latest block
async fn handle_get_latest_block(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let slot = state.state.get_last_slot().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let block = state.state.get_block_by_slot(slot).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    match block {
        Some(block) => {
            let block_hash = block.hash();
            Ok(serde_json::json!({
                "slot": block.header.slot,
                "hash": block_hash.to_hex(),
                "parent_hash": block.header.parent_hash.to_hex(),
                "state_root": block.header.state_root.to_hex(),
                "tx_root": block.header.tx_root.to_hex(),
                "timestamp": block.header.timestamp,
                "validator": Pubkey(block.header.validator).to_base58(),
                "transaction_count": block.transactions.len(),
            }))
        }
        None => Err(RpcError {
            code: -32001,
            message: "Latest block not found".to_string(),
        }),
    }
}

/// Get total burned shells
async fn handle_get_total_burned(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let burned = state.state.get_total_burned().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    Ok(serde_json::json!({
        "shells": burned,
        "molt": burned as f64 / 1_000_000_000.0,
    }))
}

/// Get all validators
async fn handle_get_validators(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let validators = state.state.get_all_validators().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let validator_list: Vec<_> = validators
        .iter()
        .map(|v| {
            // Get stake from StakePool (authoritative source)
            let pool_stake = if let Some(ref pool_arc) = state.stake_pool {
                if let Ok(pool) = pool_arc.try_lock() {
                    pool.get_stake(&v.pubkey).map(|s| s.amount).unwrap_or(0)
                } else {
                    0
                }
            } else {
                0
            };
            // Fallback to account staked field if pool has nothing
            let actual_stake = if pool_stake > 0 {
                pool_stake
            } else {
                state
                    .state
                    .get_account(&v.pubkey)
                    .ok()
                    .flatten()
                    .map(|acc| acc.staked)
                    .unwrap_or(0)
            };

            // Calculate normalized reputation (reputation as percentage of total)
            let total_reputation: u64 = validators.iter().map(|val| val.reputation).sum();
            let normalized_reputation = if total_reputation > 0 {
                v.reputation as f64 / total_reputation as f64
            } else {
                0.0
            };

            serde_json::json!({
                "pubkey": v.pubkey.to_base58(),
                "stake": actual_stake,  // Use actual account balance
                "reputation": v.reputation as f64,
                "_normalized_reputation": normalized_reputation,
                "_blocks_produced": v.blocks_proposed,
                "blocks_proposed": v.blocks_proposed,
                "votes_cast": v.votes_cast,
                "correct_votes": v.correct_votes,
                "last_active_slot": v.last_active_slot,
                "last_vote_slot": v.last_active_slot,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "validators": validator_list,
        "count": validators.len(),
        "_count": validators.len(),
    }))
}

/// Handle getMetrics
async fn handle_get_metrics(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let metrics = state.state.get_metrics();
    let validators = state.state.get_all_validators().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;
    let total_staked: u64 = if let Some(ref pool_arc) = state.stake_pool {
        if let Ok(pool) = pool_arc.try_lock() {
            pool.total_stake()
        } else {
            0
        }
    } else {
        validators
            .iter()
            .map(|v| {
                state
                    .state
                    .get_account(&v.pubkey)
                    .ok()
                    .flatten()
                    .map(|acc| acc.staked)
                    .unwrap_or(0)
            })
            .sum()
    };

    // Calculate average transactions per block
    let avg_txs_per_block = if metrics.total_blocks > 0 {
        metrics.total_transactions as f64 / metrics.total_blocks as f64
    } else {
        0.0
    };

    // Treasury balance (dynamically from state, no hardcoded address)
    let (treasury_balance, treasury_pubkey_b58) = match state.state.get_treasury_pubkey() {
        Ok(Some(tpk)) => {
            let bal = state
                .state
                .get_account(&tpk)
                .ok()
                .flatten()
                .map(|a| a.shells)
                .unwrap_or(0);
            (bal, Some(tpk.to_base58()))
        }
        _ => (0u64, None),
    };

    // Genesis wallet balance (from state)
    let (genesis_balance, genesis_pubkey_b58) = match state.state.get_genesis_pubkey() {
        Ok(Some(gpk)) => {
            let bal = state
                .state
                .get_account(&gpk)
                .ok()
                .flatten()
                .map(|a| a.shells)
                .unwrap_or(0);
            (bal, Some(gpk.to_base58()))
        }
        _ => (0u64, None),
    };

    // Circulating supply = total_supply - genesis_reserve - burned
    // This is MOLT that has left the genesis wallet and is in the economy
    // (includes treasury, staked, and free balances)
    let circulating_supply = metrics
        .total_supply
        .saturating_sub(genesis_balance)
        .saturating_sub(metrics.total_burned);

    Ok(serde_json::json!({
        "tps": metrics.tps,
        "total_transactions": metrics.total_transactions,
        "total_blocks": metrics.total_blocks,
        "average_block_time": metrics.average_block_time,
        "avg_block_time_ms": metrics.average_block_time * 1000.0,
        "avg_txs_per_block": avg_txs_per_block,
        "total_accounts": metrics.total_accounts,
        "active_accounts": metrics.active_accounts,
        "total_supply": metrics.total_supply,
        "circulating_supply": circulating_supply,
        "total_burned": metrics.total_burned,
        "total_staked": total_staked,
        "treasury_balance": treasury_balance,
        "treasury_pubkey": treasury_pubkey_b58,
        "genesis_balance": genesis_balance,
        "genesis_pubkey": genesis_pubkey_b58,
        "total_contracts": count_executable_accounts(&state.state),
        "validator_count": validators.len(),
    }))
}

// ============================================================================
// TREASURY ENDPOINT
// ============================================================================

/// Get treasury info (dynamic -- reads pubkey from state, no hardcoded address)
async fn handle_get_treasury_info(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let (treasury_pubkey, treasury_balance, treasury_staked) =
        match state.state.get_treasury_pubkey() {
            Ok(Some(tpk)) => {
                let acc = state.state.get_account(&tpk).ok().flatten();
                let bal = acc.as_ref().map(|a| a.shells).unwrap_or(0);
                let stk = acc.as_ref().map(|a| a.staked).unwrap_or(0);
                (Some(tpk.to_base58()), bal, stk)
            }
            _ => (None, 0u64, 0u64),
        };

    let (genesis_pubkey, genesis_balance, genesis_staked) = match state.state.get_genesis_pubkey() {
        Ok(Some(gpk)) => {
            let acc = state.state.get_account(&gpk).ok().flatten();
            let bal = acc.as_ref().map(|a| a.shells).unwrap_or(0);
            let stk = acc.as_ref().map(|a| a.staked).unwrap_or(0);
            (Some(gpk.to_base58()), bal, stk)
        }
        _ => (None, 0u64, 0u64),
    };

    Ok(serde_json::json!({
        "treasury_pubkey": treasury_pubkey,
        "treasury_balance": treasury_balance,
        "treasury_staked": treasury_staked,
        "genesis_pubkey": genesis_pubkey,
        "genesis_balance": genesis_balance,
        "genesis_staked": genesis_staked,
    }))
}

// ============================================================================
// NETWORK ENDPOINTS
// ============================================================================

/// Get connected peers
async fn handle_get_peers(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let (peer_count, peers) = if let Some(ref p2p) = state.p2p {
        let addresses = p2p.peer_addresses();
        let list: Vec<serde_json::Value> = addresses
            .iter()
            .map(|addr| {
                serde_json::json!({
                    "peer_id": addr,
                    "address": addr,
                    "connected": true,
                })
            })
            .collect();
        (addresses.len(), list)
    } else {
        (0, Vec::new())
    };

    Ok(serde_json::json!({
        "peers": peers,
        "count": peer_count,
    }))
}

/// Get network information
async fn handle_get_network_info(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let current_slot = state.state.get_last_slot().unwrap_or(0);
    let validators = state.state.get_all_validators().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let peer_count = if let Some(ref p2p) = state.p2p {
        p2p.peer_count()
    } else {
        0
    };

    Ok(serde_json::json!({
        "chain_id": state.chain_id,
        "network_id": state.network_id,
        "version": state.version,
        "current_slot": current_slot,
        "validator_count": validators.len(),
        "peer_count": peer_count,
    }))
}

// ============================================================================
// VALIDATOR ENDPOINTS
// ============================================================================

/// Get detailed validator information
async fn handle_get_validator_info(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey]".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid pubkey: {}", e),
    })?;

    let validator = state.state.get_validator(&pubkey).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let validator = validator.ok_or_else(|| RpcError {
        code: -32001,
        message: "Validator not found".to_string(),
    })?;

    Ok(serde_json::json!({
        "pubkey": validator.pubkey.to_base58(),
        "stake": validator.stake,
        "reputation": validator.reputation,
        "blocks_proposed": validator.blocks_proposed,
        "votes_cast": validator.votes_cast,
        "correct_votes": validator.correct_votes,
        "last_active_slot": validator.last_active_slot,
        "joined_slot": validator.joined_slot,
        "commission_rate": 5, // 5% default commission rate for validators
        "is_active": true,
    }))
}

/// Get validator performance metrics
async fn handle_get_validator_performance(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey]".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid pubkey: {}", e),
    })?;

    let validator = state.state.get_validator(&pubkey).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let validator = validator.ok_or_else(|| RpcError {
        code: -32001,
        message: "Validator not found".to_string(),
    })?;

    let current_slot = state.state.get_last_slot().unwrap_or(0);

    let vote_accuracy = if validator.votes_cast > 0 {
        (validator.correct_votes as f64 / validator.votes_cast as f64) * 100.0
    } else {
        0.0
    };

    // T2.15 fix: Calculate uptime from blocks_proposed (concrete metric)
    // instead of the misleading last_active_slot delta.
    // blocks_proposed / slots_since_joined gives the fraction of slots
    // where this validator actually produced a block.
    let slots_since_joined = current_slot.saturating_sub(validator.joined_slot);
    let uptime = if slots_since_joined > 0 {
        (validator.blocks_proposed as f64 / slots_since_joined as f64 * 100.0).min(100.0)
    } else {
        100.0 // Just joined, assume 100%
    };

    Ok(serde_json::json!({
        "pubkey": validator.pubkey.to_base58(),
        "blocks_proposed": validator.blocks_proposed,
        "votes_cast": validator.votes_cast,
        "correct_votes": validator.correct_votes,
        "vote_accuracy": vote_accuracy,
        "reputation": validator.reputation,
        "uptime": uptime,
    }))
}

/// Get comprehensive chain status
async fn handle_get_chain_status(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let current_slot = state.state.get_last_slot().unwrap_or(0);
    let validators = state.state.get_all_validators().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let total_stake: u64 = if let Some(ref pool_arc) = state.stake_pool {
        if let Ok(pool) = pool_arc.try_lock() {
            pool.total_stake()
        } else {
            validators.iter().map(|v| v.stake).sum()
        }
    } else {
        validators.iter().map(|v| v.stake).sum()
    };
    let metrics = state.state.get_metrics();

    // Calculate epoch (assuming 432 slots per epoch at 400ms = ~3 minutes)
    let epoch = current_slot / 432;
    // Block height is same as slot for now (1 block per slot)
    let block_height = current_slot;

    Ok(serde_json::json!({
        "slot": current_slot,
        "_slot": current_slot,
        "epoch": epoch,
        "_epoch": epoch,
        "block_height": block_height,
        "_block_height": block_height,
        "current_slot": current_slot,
        "latest_block": block_height,
        "validator_count": validators.len(),
        "validators": validators.len(),
        "_validators": validators.len(),
        "total_stake": total_stake,
        "total_staked": total_stake,
        "tps": metrics.tps,
        "total_transactions": metrics.total_transactions,
        "total_blocks": metrics.total_blocks,
        "average_block_time": metrics.average_block_time,
        "block_time_ms": metrics.average_block_time * 1000.0,
        "total_supply": metrics.total_supply,
        "total_burned": metrics.total_burned,
        "peer_count": if let Some(ref p2p) = state.p2p { p2p.peer_count() } else { 0 },
        "chain_id": state.chain_id,
        "network": state.network_id,
        "is_healthy": true,
    }))
}

// ============================================================================
// STAKING ENDPOINTS
// ============================================================================

/// Create stake transaction
async fn handle_stake(
    _state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected array".to_string(),
    })?;

    if arr.len() == 1 {
        let tx_base64 = arr
            .first()
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Invalid params: expected [transaction_base64]".to_string(),
            })?;

        use base64::{engine::general_purpose, Engine as _};
        let tx_bytes = general_purpose::STANDARD
            .decode(tx_base64)
            .map_err(|e| RpcError {
                code: -32602,
                message: format!("Invalid base64: {}", e),
            })?;

        let tx: Transaction = bincode::deserialize(&tx_bytes).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid transaction: {}", e),
        })?;

        let instruction = tx.message.instructions.first().ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid transaction: missing instructions".to_string(),
        })?;

        if instruction.program_id != SYSTEM_PROGRAM_ID || instruction.data.first() != Some(&9) {
            return Err(RpcError {
                code: -32602,
                message: "Invalid stake transaction: expected system opcode 9".to_string(),
            });
        }

        let signature = submit_transaction(_state, tx)?;
        return Ok(serde_json::json!(signature));
    }

    Err(RpcError {
        code: -32602,
        message: "Unsupported params: submit signed transaction via sendTransaction or stake([tx_base64])".to_string(),
    })
}

/// Create unstake transaction
async fn handle_unstake(
    _state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected array".to_string(),
    })?;

    if arr.len() == 1 {
        let tx_base64 = arr
            .first()
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Invalid params: expected [transaction_base64]".to_string(),
            })?;

        use base64::{engine::general_purpose, Engine as _};
        let tx_bytes = general_purpose::STANDARD
            .decode(tx_base64)
            .map_err(|e| RpcError {
                code: -32602,
                message: format!("Invalid base64: {}", e),
            })?;

        let tx: Transaction = bincode::deserialize(&tx_bytes).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid transaction: {}", e),
        })?;

        let instruction = tx.message.instructions.first().ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid transaction: missing instructions".to_string(),
        })?;

        if instruction.program_id != SYSTEM_PROGRAM_ID || instruction.data.first() != Some(&10) {
            return Err(RpcError {
                code: -32602,
                message: "Invalid unstake transaction: expected system opcode 10".to_string(),
            });
        }

        let signature = submit_transaction(_state, tx)?;
        return Ok(serde_json::json!(signature));
    }

    Err(RpcError {
        code: -32602,
        message: "Unsupported params: submit signed transaction via sendTransaction or unstake([tx_base64])".to_string(),
    })
}

/// Get staking status for an account
async fn handle_get_staking_status(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey]".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid pubkey: {}", e),
    })?;

    // Check if this is a validator
    let validator_info = state.state.get_validator(&pubkey).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    if let Some(validator) = validator_info {
        let live_stake = if let Some(ref pool_arc) = state.stake_pool {
            if let Ok(pool) = pool_arc.try_lock() {
                pool.get_stake(&pubkey)
                    .map(|s| s.amount)
                    .unwrap_or(validator.stake)
            } else {
                validator.stake
            }
        } else {
            validator.stake
        };
        Ok(serde_json::json!({
            "is_validator": true,
            "total_staked": live_stake,
            "delegations": [],
            "status": "active",
        }))
    } else {
        Ok(serde_json::json!({
            "is_validator": false,
            "total_staked": 0,
            "delegations": [],
            "status": "inactive",
        }))
    }
}

/// Get staking rewards for an account
async fn handle_get_staking_rewards(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey]".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid pubkey: {}", e),
    })?;

    // Get staking rewards from stake pool
    if let Some(ref pool) = state.stake_pool {
        let pool_guard = pool.lock().await;
        if let Some(stake_info) = pool_guard.get_stake(&pubkey) {
            // total_claimed tracks all historically claimed rewards (liquid + debt)
            // rewards_earned is the currently pending (unclaimed) buffer
            let total_earned = stake_info.total_claimed + stake_info.rewards_earned;
            let pending = stake_info.rewards_earned;
            let claimed = stake_info.total_claimed;

            // Reward rate: MOLT per block for this validator
            let reward_rate = if stake_info.is_active {
                if stake_info.bootstrap_debt > 0 {
                    // During vesting: 50% goes to debt, 50% liquid
                    "0.09" // half of 0.18 MOLT (heartbeat avg)
                } else {
                    "0.18"
                }
            } else {
                "0"
            };

            return Ok(serde_json::json!({
                "total_rewards": total_earned,
                "pending_rewards": pending,
                "claimed_rewards": claimed,
                "reward_rate": reward_rate,
                "bootstrap_debt": stake_info.bootstrap_debt,
                "earned_amount": stake_info.earned_amount,
                "vesting_progress": stake_info.vesting_progress() as f64 / 100.0,
                "blocks_produced": stake_info.blocks_produced,
                "total_debt_repaid": stake_info.total_debt_repaid,
            }));
        }
    }

    // No staking found
    Ok(serde_json::json!({
        "total_rewards": 0,
        "pending_rewards": 0,
        "claimed_rewards": 0,
        "reward_rate": 0.0,
    }))
}

// ============================================================================
// ACCOUNT ENDPOINTS
// ============================================================================

/// Get enhanced account information
async fn handle_get_account_info(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey]".to_string(),
        })?;

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid pubkey: {}", e),
    })?;

    let account = state.state.get_account(&pubkey).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let balance = state.state.get_balance(&pubkey).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    // Check if it's a validator
    let is_validator = state
        .state
        .get_validator(&pubkey)
        .map(|v| v.is_some())
        .unwrap_or(false);

    Ok(serde_json::json!({
        "pubkey": pubkey.to_base58(),
        "balance": balance,
        "molt": balance as f64 / 1_000_000_000.0,
        "exists": account.is_some(),
        "is_validator": is_validator,
        "is_executable": account.as_ref().map(|a| a.executable).unwrap_or(false),
    }))
}

/// Get transaction history for an account (paginated)
async fn handle_get_transaction_history(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected array".to_string(),
    })?;

    let pubkey_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [pubkey, options?]".to_string(),
        })?;

    let opts = arr.get(1);
    let limit = opts
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.get("limit").and_then(|l| l.as_u64()))
        })
        .unwrap_or(10)
        .min(500) as usize;

    let before_slot = opts
        .and_then(|v| v.get("before_slot"))
        .and_then(|v| v.as_u64());

    let pubkey = Pubkey::from_base58(pubkey_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid pubkey: {}", e),
    })?;

    let fee_config = state
        .state
        .get_fee_config()
        .unwrap_or_else(|_| moltchain_core::FeeConfig::default_from_constants());

    // Use paginated reverse-iterator method
    let indexed = state
        .state
        .get_account_tx_signatures_paginated(&pubkey, limit, before_slot)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let mut transactions: Vec<serde_json::Value> = Vec::new();
    let mut timestamps: HashMap<u64, u64> = HashMap::new();
    let mut last_slot: Option<u64> = None;

    for (hash, slot) in &indexed {
        if transactions.len() >= limit {
            break;
        }

        let tx = match state.state.get_transaction(hash) {
            Ok(Some(tx)) => tx,
            _ => continue,
        };

        let timestamp = if let Some(cached) = timestamps.get(slot) {
            *cached
        } else {
            let ts = state
                .state
                .get_block_by_slot(*slot)
                .ok()
                .and_then(|block| block.map(|b| b.header.timestamp))
                .unwrap_or(0);
            timestamps.insert(*slot, ts);
            ts
        };

        let first_ix = tx.message.instructions.first();
        let (tx_type, from, to, amount) = if let Some(ix) = first_ix {
            let from = ix
                .accounts
                .first()
                .map(|acc| acc.to_base58())
                .unwrap_or_default();
            let to = ix
                .accounts
                .get(1)
                .map(|acc| acc.to_base58())
                .unwrap_or_default();
            let amount = parse_transfer_amount(ix).unwrap_or(0);
            (instruction_type(ix), from, to, amount)
        } else {
            ("Unknown", String::new(), String::new(), 0)
        };

        let fee = TxProcessor::compute_transaction_fee(&tx, &fee_config);

        transactions.push(serde_json::json!({
            "hash": tx.signature().to_hex(),
            "signature": tx.signature().to_hex(),
            "slot": slot,
            "timestamp": timestamp,
            "from": from,
            "to": to,
            "type": tx_type,
            "amount": amount as f64 / 1_000_000_000.0,
            "amount_shells": amount,
            "fee": fee,
            "fee_molt": fee as f64 / 1_000_000_000.0,
            "success": true,
        }));

        last_slot = Some(*slot);
    }

    let has_more = transactions.len() == limit;
    Ok(serde_json::json!({
        "transactions": transactions,
        "count": transactions.len(),
        "limit": limit,
        "has_more": has_more,
        "next_before_slot": if has_more { last_slot } else { None::<u64> },
    }))
}

// ============================================================================
// CONTRACT ENDPOINTS
// ============================================================================

/// Get contract information
async fn handle_get_contract_info(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let contract_id_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [contract_id]".to_string(),
        })?;

    let contract_id = Pubkey::from_base58(contract_id_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid contract ID: {}", e),
    })?;

    let account = state
        .state
        .get_account(&contract_id)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let account = account.ok_or_else(|| RpcError {
        code: -32001,
        message: "Contract not found".to_string(),
    })?;

    // Try to parse ContractAccount to get rich metadata
    let (has_abi, abi_functions, code_hash, owner_b58, token_metadata) = if account.executable {
        if let Ok(ca) = serde_json::from_slice::<moltchain_core::ContractAccount>(&account.data) {
            let func_count = ca.abi.as_ref().map(|a| a.functions.len()).unwrap_or(0);
            let abi_fn_names: Vec<String> = ca
                .abi
                .as_ref()
                .map(|a| a.functions.iter().map(|f| f.name.clone()).collect())
                .unwrap_or_default();

            // Extract MT-20 token metadata from contract storage (well-known keys)
            let mut tmeta = serde_json::Map::new();
            if let Some(v) = ca.storage.get(b"total_supply".as_ref()) {
                if v.len() == 8 {
                    let supply =
                        u64::from_le_bytes([v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]]);
                    tmeta.insert("total_supply".to_string(), serde_json::json!(supply));
                } else if let Ok(s) = std::str::from_utf8(v) {
                    if let Ok(n) = s.parse::<u64>() {
                        tmeta.insert("total_supply".to_string(), serde_json::json!(n));
                    }
                }
            }
            // Also check for prefixed supply keys (wrapped tokens: musd_supply, wsol_supply, weth_supply)
            if !tmeta.contains_key("total_supply") {
                for (key, val) in ca.storage.iter() {
                    if let Ok(k) = std::str::from_utf8(key) {
                        if k.ends_with("_supply")
                            && !k.ends_with("_minted")
                            && !k.ends_with("_burned")
                        {
                            if val.len() == 8 {
                                let supply = u64::from_le_bytes([
                                    val[0], val[1], val[2], val[3], val[4], val[5], val[6], val[7],
                                ]);
                                tmeta.insert("total_supply".to_string(), serde_json::json!(supply));
                            }
                            break;
                        }
                    }
                }
            }
            if let Some(v) = ca.storage.get(b"token_decimals".as_ref()) {
                if let Ok(s) = std::str::from_utf8(v) {
                    if let Ok(n) = s.parse::<u8>() {
                        tmeta.insert("decimals".to_string(), serde_json::json!(n));
                    }
                } else if v.len() == 1 {
                    tmeta.insert("decimals".to_string(), serde_json::json!(v[0]));
                }
            }
            if let Some(v) = ca.storage.get(b"token_name".as_ref()) {
                if let Ok(s) = std::str::from_utf8(v) {
                    tmeta.insert("token_name".to_string(), serde_json::json!(s));
                }
            }
            if let Some(v) = ca.storage.get(b"token_symbol".as_ref()) {
                if let Ok(s) = std::str::from_utf8(v) {
                    tmeta.insert("token_symbol".to_string(), serde_json::json!(s));
                }
            }
            // Detect mintable/burnable from ABI function names
            let has_mint = abi_fn_names.iter().any(|n| n == "mint");
            let has_burn = abi_fn_names.iter().any(|n| n == "burn");
            tmeta.insert("mintable".to_string(), serde_json::json!(has_mint));
            tmeta.insert("burnable".to_string(), serde_json::json!(has_burn));

            let token_meta = if tmeta.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(tmeta))
            };

            (
                ca.abi.is_some(),
                func_count,
                ca.code_hash.to_hex(),
                ca.owner.to_base58(),
                token_meta,
            )
        } else {
            (false, 0, String::new(), account.owner.to_base58(), None)
        }
    } else {
        (false, 0, String::new(), account.owner.to_base58(), None)
    };

    let mut result = serde_json::json!({
        "contract_id": contract_id.to_base58(),
        "owner": owner_b58,
        "code_size": account.data.len(),
        "is_executable": account.executable,
        "has_abi": has_abi,
        "abi_functions": abi_functions,
        "code_hash": code_hash,
        "deployed_at": 0,
    });
    if let Some(tm) = token_metadata {
        result
            .as_object_mut()
            .unwrap()
            .insert("token_metadata".to_string(), tm);
    }
    Ok(result)
}

/// Get contract execution logs
async fn handle_get_contract_logs(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected array".to_string(),
    })?;

    let contract_id_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [contract_id, limit?]".to_string(),
        })?;

    let limit = arr.get(1).and_then(|v| v.as_u64()).unwrap_or(100) as usize;

    let contract_id = Pubkey::from_base58(contract_id_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid contract ID: {}", e),
    })?;

    let events = state
        .state
        .get_contract_logs(&contract_id, limit)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let logs: Vec<serde_json::Value> = events
        .iter()
        .map(|e| {
            serde_json::json!({
                "program": e.program.to_base58(),
                "name": e.name,
                "data": e.data,
                "slot": e.slot,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "logs": logs,
        "count": logs.len(),
    }))
}

/// Get contract ABI/IDL
async fn handle_get_contract_abi(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let contract_id_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [contract_id]".to_string(),
        })?;

    let contract_id = Pubkey::from_base58(contract_id_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid contract ID: {}", e),
    })?;

    let account = state
        .state
        .get_account(&contract_id)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let account = account.ok_or_else(|| RpcError {
        code: -32001,
        message: "Contract not found".to_string(),
    })?;

    if !account.executable {
        return Err(RpcError {
            code: -32001,
            message: "Account is not a contract".to_string(),
        });
    }

    let contract: moltchain_core::ContractAccount =
        serde_json::from_slice(&account.data).map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to decode contract: {}", e),
        })?;

    match contract.abi {
        Some(abi) => Ok(serde_json::to_value(&abi).unwrap_or_default()),
        None => Ok(serde_json::json!({
            "error": "No ABI available for this contract",
            "hint": "Deploy with an ABI in init_data, or use setContractAbi"
        })),
    }
}

/// Set/update contract ABI (admin-only — requires admin_token)
async fn handle_set_contract_abi(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    // H16 fix: reject in multi-validator mode (direct state write bypasses consensus)
    require_single_validator(state, "setContractAbi")?;
    verify_admin_auth(state, &params)?;

    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [contract_id, abi_json]".to_string(),
    })?;

    let contract_id_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing contract_id".to_string(),
        })?;

    let abi_value = arr.get(1).ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing ABI JSON".to_string(),
    })?;

    let contract_id = Pubkey::from_base58(contract_id_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid contract ID: {}", e),
    })?;

    let abi: moltchain_core::ContractAbi =
        serde_json::from_value(abi_value.clone()).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid ABI format: {}", e),
        })?;

    let mut account = state
        .state
        .get_account(&contract_id)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .ok_or_else(|| RpcError {
            code: -32001,
            message: "Contract not found".to_string(),
        })?;

    if !account.executable {
        return Err(RpcError {
            code: -32001,
            message: "Account is not a contract".to_string(),
        });
    }

    let mut contract: moltchain_core::ContractAccount = serde_json::from_slice(&account.data)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to decode contract: {}", e),
        })?;

    contract.abi = Some(abi);
    account.data = serde_json::to_vec(&contract).map_err(|e| RpcError {
        code: -32000,
        message: format!("Failed to serialize contract: {}", e),
    })?;

    state
        .state
        .put_account(&contract_id, &account)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    Ok(serde_json::json!({
        "success": true,
        "contract": contract_id.to_base58(),
        "abi_functions": contract.abi.as_ref().map(|a| a.functions.len()).unwrap_or(0),
    }))
}

/// Get all deployed contracts
async fn handle_get_all_contracts(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let programs = state.state.get_all_programs(1000).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let contracts: Vec<serde_json::Value> = programs
        .iter()
        .map(|(pk, metadata)| {
            serde_json::json!({
                "program_id": pk.to_base58(),
                "metadata": metadata,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "contracts": contracts,
        "count": contracts.len(),
    }))
}

/// Deploy a contract via RPC (bypasses transaction instruction size limit).
///
/// Params: [deployer_base58, code_base64, init_data_json_or_null, signature_hex]
///
/// The deployer signs SHA-256(code_bytes) with their ed25519 key.
/// Deploy fee (2.5 MOLT) is charged from the deployer's account.
/// Contract address is derived as SHA-256(deployer_pubkey + code_bytes).
async fn handle_deploy_contract(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    use base64::{engine::general_purpose, Engine as _};
    use moltchain_core::account::Keypair as MoltKeypair;
    use sha2::{Digest, Sha256};

    // H16 fix: reject in multi-validator mode (direct state write bypasses consensus)
    require_single_validator(state, "deployContract")?;

    // Admin-gate: contract deployment requires admin authentication
    verify_admin_auth(state, &params)?;

    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Params must be an array: [deployer, code_base64, init_data, signature]"
            .to_string(),
    })?;

    if arr.len() < 4 {
        return Err(RpcError {
            code: -32602,
            message:
                "Expected [deployer_base58, code_base64, init_data_json_or_null, signature_hex]"
                    .to_string(),
        });
    }

    // Parse deployer pubkey
    let deployer_str = arr[0].as_str().ok_or_else(|| RpcError {
        code: -32602,
        message: "deployer must be a base58 string".to_string(),
    })?;
    let deployer_pubkey = Pubkey::from_base58(deployer_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid deployer pubkey: {}", e),
    })?;

    // Parse WASM code (base64)
    let code_b64 = arr[1].as_str().ok_or_else(|| RpcError {
        code: -32602,
        message: "code must be a base64 string".to_string(),
    })?;
    let code_bytes = general_purpose::STANDARD
        .decode(code_b64)
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid base64 code: {}", e),
        })?;

    if code_bytes.is_empty() {
        return Err(RpcError {
            code: -32602,
            message: "Code cannot be empty".to_string(),
        });
    }

    // Parse init_data (optional JSON)
    let init_data_bytes: Vec<u8> = if arr[2].is_null() {
        vec![]
    } else if let Some(s) = arr[2].as_str() {
        s.as_bytes().to_vec()
    } else {
        // If it's a JSON object, serialize it
        serde_json::to_vec(&arr[2]).unwrap_or_default()
    };

    // Parse signature (hex-encoded, 64 bytes = 128 hex chars)
    let sig_hex = arr[3].as_str().ok_or_else(|| RpcError {
        code: -32602,
        message: "signature must be a hex string".to_string(),
    })?;
    let sig_bytes = hex::decode(sig_hex).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid hex signature: {}", e),
    })?;
    if sig_bytes.len() != 64 {
        return Err(RpcError {
            code: -32602,
            message: format!("Signature must be 64 bytes, got {}", sig_bytes.len()),
        });
    }
    let mut sig_array = [0u8; 64];
    sig_array.copy_from_slice(&sig_bytes);

    // Verify signature: deployer must sign SHA-256(code_bytes)
    let mut hasher = Sha256::new();
    hasher.update(&code_bytes);
    let code_hash = hasher.finalize();
    if !MoltKeypair::verify(&deployer_pubkey, &code_hash, &sig_array) {
        return Err(RpcError {
            code: -32003,
            message: "Invalid signature: deployer must sign SHA-256(code)".to_string(),
        });
    }

    // Derive program address: SHA-256(deployer + name + code)
    // Including the name/symbol ensures identical WASMs (e.g. wrapped token stubs)
    // get unique addresses — matches genesis derivation in validator/src/main.rs.
    let contract_name: Option<String> = if !init_data_bytes.is_empty() {
        serde_json::from_slice::<serde_json::Value>(&init_data_bytes)
            .ok()
            .and_then(|v| {
                v.get("name")
                    .or_else(|| v.get("symbol"))
                    .and_then(|n| n.as_str().map(|s| s.to_string()))
            })
    } else {
        None
    };

    let mut addr_hasher = Sha256::new();
    addr_hasher.update(deployer_pubkey.0);
    if let Some(ref name) = contract_name {
        addr_hasher.update(name.as_bytes());
    }
    addr_hasher.update(&code_bytes);
    let addr_hash = addr_hasher.finalize();
    let mut addr_bytes = [0u8; 32];
    addr_bytes.copy_from_slice(&addr_hash[..32]);
    let program_pubkey = Pubkey(addr_bytes);

    // Check if already deployed
    if let Ok(Some(_)) = state.state.get_account(&program_pubkey) {
        return Err(RpcError {
            code: -32000,
            message: format!("Contract already exists at {}", program_pubkey.to_base58()),
        });
    }

    // Charge deploy fee (2.5 MOLT)
    let deploy_fee = moltchain_core::CONTRACT_DEPLOY_FEE;
    let deployer_account = state
        .state
        .get_account(&deployer_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .ok_or_else(|| RpcError {
            code: -32000,
            message: "Deployer account not found".to_string(),
        })?;

    if deployer_account.spendable < deploy_fee {
        return Err(RpcError {
            code: -32000,
            message: format!(
                "Insufficient spendable balance: need {} shells ({:.1} MOLT), have {} spendable ({:.1} MOLT)",
                deploy_fee,
                deploy_fee as f64 / 1_000_000_000.0,
                deployer_account.spendable,
                deployer_account.spendable as f64 / 1_000_000_000.0,
            ),
        });
    }

    // Debit deployer using deduct_spendable to maintain shells == spendable + staked + locked
    let mut updated_deployer = deployer_account.clone();
    updated_deployer
        .deduct_spendable(deploy_fee)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to deduct deploy fee: {}", e),
        })?;
    state
        .state
        .put_account(&deployer_pubkey, &updated_deployer)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to update deployer balance: {}", e),
        })?;

    // Credit deploy fee to treasury (not a vanishing deduction)
    let treasury_pubkey = state
        .state
        .get_treasury_pubkey()
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .ok_or_else(|| RpcError {
            code: -32000,
            message: "Treasury pubkey not set".to_string(),
        })?;
    let mut treasury_account = state
        .state
        .get_account(&treasury_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .unwrap_or_else(|| moltchain_core::Account::new(0, treasury_pubkey));
    treasury_account
        .add_spendable(deploy_fee)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Treasury balance overflow: {}", e),
        })?;
    state
        .state
        .put_account(&treasury_pubkey, &treasury_account)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to credit treasury: {}", e),
        })?;

    // Create ContractAccount
    let contract = ContractAccount::new(code_bytes, deployer_pubkey);
    let mut account = moltchain_core::Account::new(0, program_pubkey);
    account.data = serde_json::to_vec(&contract).map_err(|e| RpcError {
        code: -32000,
        message: format!("Failed to serialize contract: {}", e),
    })?;
    account.executable = true;

    // Store the contract account
    state
        .state
        .put_account(&program_pubkey, &account)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to store contract: {}", e),
        })?;

    // Index in programs list
    if let Err(e) = state.state.index_program(&program_pubkey) {
        warn!("deployContract: index_program failed: {}", e);
    }

    // Process init_data for symbol registry
    if !init_data_bytes.is_empty() {
        if let Ok(raw) = std::str::from_utf8(&init_data_bytes) {
            if let Ok(registry_data) = serde_json::from_str::<serde_json::Value>(raw) {
                if let Some(symbol) = registry_data.get("symbol").and_then(|s| s.as_str()) {
                    let entry = SymbolRegistryEntry {
                        symbol: symbol.to_string(),
                        program: program_pubkey,
                        owner: deployer_pubkey,
                        name: registry_data
                            .get("name")
                            .and_then(|n| n.as_str())
                            .map(|s| s.to_string()),
                        template: registry_data
                            .get("template")
                            .and_then(|t| t.as_str())
                            .map(|s| s.to_string()),
                        metadata: registry_data.get("metadata").cloned(),
                    };
                    if let Err(e) = state.state.register_symbol(symbol, entry) {
                        warn!("deployContract: register_symbol failed: {}", e);
                    }
                }
            }
        }
    }

    info!(
        "deployContract: {} deployed contract at {} (code={} bytes, fee={} shells)",
        deployer_pubkey.to_base58(),
        program_pubkey.to_base58(),
        account.data.len(),
        deploy_fee,
    );

    Ok(serde_json::json!({
        "program_id": program_pubkey.to_base58(),
        "deployer": deployer_pubkey.to_base58(),
        "code_size": account.data.len(),
        "deploy_fee": deploy_fee,
        "deploy_fee_molt": deploy_fee as f64 / 1_000_000_000.0,
    }))
}

// ============================================================================
// PROGRAM ENDPOINTS
// ============================================================================

async fn handle_get_program(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let program_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [program_pubkey]".to_string(),
        })?;

    let program_pubkey = Pubkey::from_base58(program_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid program pubkey: {}", e),
    })?;

    let account = state
        .state
        .get_account(&program_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let account = account.ok_or_else(|| RpcError {
        code: -32001,
        message: "Program not found".to_string(),
    })?;

    if !account.executable {
        return Err(RpcError {
            code: -32002,
            message: "Account is not executable".to_string(),
        });
    }

    let contract: ContractAccount =
        serde_json::from_slice(&account.data).map_err(|e| RpcError {
            code: -32002,
            message: format!("Invalid program data: {}", e),
        })?;

    Ok(serde_json::json!({
        "program": program_pubkey.to_base58(),
        "owner": contract.owner.to_base58(),
        "code_hash": contract.code_hash.to_hex(),
        "code_size": contract.code.len(),
        "storage_entries": contract.storage.len(),
        "storage_size": contract.storage.values().map(|v| v.len()).sum::<usize>(),
        "executable": account.executable,
    }))
}

async fn handle_get_program_stats(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let program_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [program_pubkey]".to_string(),
        })?;

    let program_pubkey = Pubkey::from_base58(program_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid program pubkey: {}", e),
    })?;

    let account = state
        .state
        .get_account(&program_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let account = account.ok_or_else(|| RpcError {
        code: -32001,
        message: "Program not found".to_string(),
    })?;

    let contract: ContractAccount =
        serde_json::from_slice(&account.data).map_err(|e| RpcError {
            code: -32002,
            message: format!("Invalid program data: {}", e),
        })?;

    let call_count = state
        .state
        .count_program_calls(&program_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    Ok(serde_json::json!({
        "program": program_pubkey.to_base58(),
        "owner": contract.owner.to_base58(),
        "code_hash": contract.code_hash.to_hex(),
        "code_size": contract.code.len(),
        "storage_entries": contract.storage.len(),
        "call_count": call_count,
    }))
}

async fn handle_get_programs(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let limit = params
        .and_then(|val| {
            if val.is_array() {
                val.as_array()
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.as_u64())
            } else if val.is_object() {
                val.get("limit").and_then(|v| v.as_u64())
            } else {
                val.as_u64()
            }
        })
        .unwrap_or(50)
        .min(500) as usize;

    let programs = state.state.get_programs(limit).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let list: Vec<String> = programs.iter().map(|p| p.to_base58()).collect();

    Ok(serde_json::json!({
        "count": list.len(),
        "programs": list,
    }))
}

async fn handle_get_program_calls(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [program_pubkey, options?]".to_string(),
    })?;

    let program_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [program_pubkey, options?]".to_string(),
        })?;

    let limit = arr
        .get(1)
        .and_then(|v| v.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(500) as usize;

    let program_pubkey = Pubkey::from_base58(program_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid program pubkey: {}", e),
    })?;

    let calls = state
        .state
        .get_program_calls(&program_pubkey, limit)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let items: Vec<serde_json::Value> = calls
        .into_iter()
        .map(|call| {
            serde_json::json!({
                "slot": call.slot,
                "timestamp": call.timestamp,
                "program": call.program.to_base58(),
                "caller": call.caller.to_base58(),
                "function": call.function,
                "value": call.value,
                "tx_signature": call.tx_signature.to_hex(),
            })
        })
        .collect();

    Ok(serde_json::json!({
        "program": program_pubkey.to_base58(),
        "count": items.len(),
        "calls": items,
    }))
}

async fn handle_get_program_storage(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [program_pubkey, options?]".to_string(),
    })?;

    let program_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [program_pubkey, options?]".to_string(),
        })?;

    let limit = arr
        .get(1)
        .and_then(|v| v.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(500) as usize;

    let program_pubkey = Pubkey::from_base58(program_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid program pubkey: {}", e),
    })?;

    let account = state
        .state
        .get_account(&program_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let account = account.ok_or_else(|| RpcError {
        code: -32001,
        message: "Program not found".to_string(),
    })?;

    let contract: ContractAccount =
        serde_json::from_slice(&account.data).map_err(|e| RpcError {
            code: -32002,
            message: format!("Invalid program data: {}", e),
        })?;

    let mut entries = Vec::new();
    for (key, value) in contract.storage.into_iter().take(limit) {
        entries.push(serde_json::json!({
            "key": hex::encode(key),
            "value": hex::encode(value),
        }));
    }

    Ok(serde_json::json!({
        "program": program_pubkey.to_base58(),
        "count": entries.len(),
        "entries": entries,
    }))
}

// ============================================================================
// NFT ENDPOINTS
// ============================================================================

async fn handle_get_collection(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let collection_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [collection_pubkey]".to_string(),
        })?;

    let collection_pubkey = Pubkey::from_base58(collection_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid collection pubkey: {}", e),
    })?;

    let account = state
        .state
        .get_account(&collection_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let account = account.ok_or_else(|| RpcError {
        code: -32001,
        message: "Collection not found".to_string(),
    })?;

    let collection = decode_collection_state(&account.data).map_err(|e| RpcError {
        code: -32002,
        message: format!("Invalid collection data: {}", e),
    })?;

    Ok(serde_json::json!({
        "collection": collection_pubkey.to_base58(),
        "name": collection.name,
        "symbol": collection.symbol,
        "creator": collection.creator.to_base58(),
        "royalty_bps": collection.royalty_bps,
        "max_supply": collection.max_supply,
        "minted": collection.minted,
        "public_mint": collection.public_mint,
        "mint_authority": collection.mint_authority.map(|p| p.to_base58()),
    }))
}

async fn handle_get_nft(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [collection_pubkey, token_id] or [token_pubkey]"
            .to_string(),
    })?;

    // Support two calling conventions:
    //   [collection_pubkey, token_id] -- derive token address from collection + token_id
    //   [token_pubkey]                -- direct token account lookup
    let token_pubkey = if arr.len() >= 2 {
        // [collection_pubkey, token_id] form
        let collection_str = arr[0].as_str().ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid collection pubkey".to_string(),
        })?;
        let token_id = arr[1].as_u64().ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid token_id".to_string(),
        })?;
        let collection_pubkey = Pubkey::from_base58(collection_str).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid collection pubkey: {}", e),
        })?;
        // Derive token address: SHA-256(collection_bytes + token_id_le_bytes)
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(collection_pubkey.0);
        hasher.update(token_id.to_le_bytes());
        let hash = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&hash[..32]);
        Pubkey(bytes)
    } else {
        // [token_pubkey] form (direct lookup)
        let token_str = arr[0].as_str().ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid token pubkey".to_string(),
        })?;
        Pubkey::from_base58(token_str).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid token pubkey: {}", e),
        })?
    };

    let account = state
        .state
        .get_account(&token_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let account = account.ok_or_else(|| RpcError {
        code: -32001,
        message: "NFT not found".to_string(),
    })?;

    let token = decode_token_state(&account.data).map_err(|e| RpcError {
        code: -32002,
        message: format!("Invalid token data: {}", e),
    })?;

    Ok(serde_json::json!({
        "token": token_pubkey.to_base58(),
        "collection": token.collection.to_base58(),
        "token_id": token.token_id,
        "owner": token.owner.to_base58(),
        "metadata_uri": token.metadata_uri,
    }))
}

async fn handle_get_nfts_by_owner(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [owner_pubkey, options?]".to_string(),
    })?;

    let owner_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [owner_pubkey, options?]".to_string(),
        })?;

    let limit = arr
        .get(1)
        .and_then(|v| v.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(500) as usize;

    let owner = Pubkey::from_base58(owner_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid owner pubkey: {}", e),
    })?;

    let token_pubkeys = state
        .state
        .get_nft_tokens_by_owner(&owner, limit)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let mut items = Vec::new();
    for token_pubkey in token_pubkeys {
        let account = match state.state.get_account(&token_pubkey) {
            Ok(Some(account)) => account,
            _ => continue,
        };

        let token = match decode_token_state(&account.data) {
            Ok(token) => token,
            Err(_) => continue,
        };

        items.push(serde_json::json!({
            "token": token_pubkey.to_base58(),
            "collection": token.collection.to_base58(),
            "token_id": token.token_id,
            "owner": token.owner.to_base58(),
            "metadata_uri": token.metadata_uri,
        }));
    }

    Ok(serde_json::json!({
        "owner": owner.to_base58(),
        "count": items.len(),
        "nfts": items,
    }))
}

async fn handle_get_nfts_by_collection(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [collection_pubkey, options?]".to_string(),
    })?;

    let collection_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [collection_pubkey, options?]".to_string(),
        })?;

    let limit = arr
        .get(1)
        .and_then(|v| v.get("limit"))
        .and_then(|v| v.as_u64())
        .unwrap_or(50)
        .min(500) as usize;

    let collection = Pubkey::from_base58(collection_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid collection pubkey: {}", e),
    })?;

    let token_pubkeys = state
        .state
        .get_nft_tokens_by_collection(&collection, limit)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let mut items = Vec::new();
    for token_pubkey in token_pubkeys {
        let account = match state.state.get_account(&token_pubkey) {
            Ok(Some(account)) => account,
            _ => continue,
        };

        let token = match decode_token_state(&account.data) {
            Ok(token) => token,
            Err(_) => continue,
        };

        items.push(serde_json::json!({
            "token": token_pubkey.to_base58(),
            "collection": token.collection.to_base58(),
            "token_id": token.token_id,
            "owner": token.owner.to_base58(),
            "metadata_uri": token.metadata_uri,
        }));
    }

    Ok(serde_json::json!({
        "collection": collection.to_base58(),
        "count": items.len(),
        "nfts": items,
    }))
}

async fn handle_get_nft_activity(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [collection_pubkey, options?]".to_string(),
    })?;

    let collection_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [collection_pubkey, options?]".to_string(),
        })?;

    let limit = arr
        .get(1)
        .and_then(|v| {
            if v.is_object() {
                v.get("limit").and_then(|val| val.as_u64())
            } else {
                v.as_u64()
            }
        })
        .unwrap_or(50)
        .min(500) as usize;

    let collection = Pubkey::from_base58(collection_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid collection pubkey: {}", e),
    })?;

    let activities = state
        .state
        .get_nft_activity_by_collection(&collection, limit)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let items: Vec<serde_json::Value> = activities
        .into_iter()
        .map(|activity| {
            let kind = match activity.kind {
                NftActivityKind::Mint => "mint",
                NftActivityKind::Transfer => "transfer",
            };

            serde_json::json!({
                "slot": activity.slot,
                "timestamp": activity.timestamp,
                "kind": kind,
                "collection": activity.collection.to_base58(),
                "token": activity.token.to_base58(),
                "from": activity.from.map(|p| p.to_base58()),
                "to": activity.to.to_base58(),
                "tx_signature": activity.tx_signature.to_hex(),
            })
        })
        .collect();

    Ok(serde_json::json!({
        "collection": collection.to_base58(),
        "count": items.len(),
        "activity": items,
    }))
}

// ============================================================================
// MARKETPLACE ENDPOINTS
// ============================================================================

fn parse_market_params(
    params: Option<serde_json::Value>,
) -> Result<(Option<Pubkey>, usize), RpcError> {
    let limit_default = 50usize;

    let Some(params) = params else {
        return Ok((None, limit_default));
    };

    if let Some(obj) = params.as_object() {
        let collection = obj
            .get("collection")
            .and_then(|v| v.as_str())
            .map(Pubkey::from_base58)
            .transpose()
            .map_err(|e| RpcError {
                code: -32602,
                message: format!("Invalid collection pubkey: {}", e),
            })?;

        let limit = obj
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(limit_default as u64)
            .min(500) as usize;

        return Ok((collection, limit));
    }

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [collection_pubkey?, options?] or {collection, limit}"
            .to_string(),
    })?;

    let (collection, limit) = match arr.first() {
        Some(first) if first.is_object() => {
            let limit = first
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(limit_default as u64)
                .min(500) as usize;
            (None, limit)
        }
        _ => {
            let collection = arr
                .first()
                .and_then(|v| v.as_str())
                .map(Pubkey::from_base58)
                .transpose()
                .map_err(|e| RpcError {
                    code: -32602,
                    message: format!("Invalid collection pubkey: {}", e),
                })?;

            let limit = arr
                .get(1)
                .and_then(|v| {
                    if v.is_object() {
                        v.get("limit").and_then(|val| val.as_u64())
                    } else {
                        v.as_u64()
                    }
                })
                .unwrap_or(limit_default as u64)
                .min(500) as usize;

            (collection, limit)
        }
    };

    Ok((collection, limit))
}

fn market_activity_to_json(activity: &moltchain_core::MarketActivity) -> serde_json::Value {
    let kind = match activity.kind {
        MarketActivityKind::Listing => "listing",
        MarketActivityKind::Sale => "sale",
        MarketActivityKind::Cancel => "cancel",
    };

    serde_json::json!({
        "slot": activity.slot,
        "timestamp": activity.timestamp,
        "kind": kind,
        "program": activity.program.to_base58(),
        "collection": activity.collection.as_ref().map(|p| p.to_base58()),
        "token": activity.token.as_ref().map(|p| p.to_base58()),
        "token_id": activity.token_id,
        "price": activity.price,
        "price_molt": activity.price.map(|val| val as f64 / 1_000_000_000.0),
        "seller": activity.seller.as_ref().map(|p| p.to_base58()),
        "buyer": activity.buyer.as_ref().map(|p| p.to_base58()),
        "function": activity.function.clone(),
        "tx_signature": activity.tx_signature.to_hex(),
    })
}

async fn handle_get_market_listings(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let (collection, limit) = parse_market_params(params)?;

    let activity = state
        .state
        .get_market_activity(
            collection.as_ref(),
            Some(MarketActivityKind::Listing),
            limit,
        )
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let items: Vec<serde_json::Value> = activity.iter().map(market_activity_to_json).collect();

    Ok(serde_json::json!({
        "collection": collection.map(|c| c.to_base58()),
        "count": items.len(),
        "listings": items,
    }))
}

async fn handle_get_market_sales(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let (collection, limit) = parse_market_params(params)?;

    let activity = state
        .state
        .get_market_activity(collection.as_ref(), Some(MarketActivityKind::Sale), limit)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let items: Vec<serde_json::Value> = activity.iter().map(market_activity_to_json).collect();

    Ok(serde_json::json!({
        "collection": collection.map(|c| c.to_base58()),
        "count": items.len(),
        "sales": items,
    }))
}

// ============================================================================
// ETHEREUM JSON-RPC COMPATIBILITY LAYER (MetaMask Support)
// ============================================================================

/// eth_getBalance - Get balance in wei format
async fn handle_eth_get_balance(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let evm_address_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [address, block]".to_string(),
        })?;

    // Parse EVM address
    let evm_address =
        moltchain_core::StateStore::parse_evm_address(evm_address_str).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid EVM address: {}", e),
        })?;

    // Lookup native pubkey
    let native_pubkey = state
        .state
        .lookup_evm_address(&evm_address)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let balance = if let Some(pubkey) = native_pubkey {
        let account = state.state.get_account(&pubkey).map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;
        let spendable = account.map(|a| a.spendable).unwrap_or(0);
        shells_to_u256(spendable)
    } else if let Some(account) =
        state
            .state
            .get_evm_account(&evm_address)
            .map_err(|e| RpcError {
                code: -32000,
                message: format!("Database error: {}", e),
            })?
    {
        account.balance_u256()
    } else {
        shells_to_u256(0)
    };

    Ok(serde_json::json!(format!("0x{:x}", balance)))
}

/// eth_sendRawTransaction - Submit signed Ethereum transaction
async fn handle_eth_send_raw_transaction(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let tx_data = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [signedTxData]".to_string(),
        })?;

    let tx_hex = tx_data.strip_prefix("0x").unwrap_or(tx_data);
    let raw = hex::decode(tx_hex).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid tx hex: {}", e),
    })?;

    let evm_tx = decode_evm_transaction(&raw).map_err(|e| RpcError {
        code: -32602,
        message: e,
    })?;

    if let Some(chain_id) = evm_tx.chain_id {
        if chain_id != state.evm_chain_id {
            return Err(RpcError {
                code: -32602,
                message: format!("Invalid chainId: {}", chain_id),
            });
        }
    }

    let from_address: [u8; 20] = evm_tx.from.into();
    let mapped_pubkey = state
        .state
        .lookup_evm_address(&from_address)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let mapped_pubkey = mapped_pubkey.ok_or_else(|| RpcError {
        code: -32602,
        message: "EVM address not registered".to_string(),
    })?;

    let instruction = Instruction {
        program_id: EVM_PROGRAM_ID,
        accounts: vec![mapped_pubkey],
        data: raw,
    };

    let message = moltchain_core::Message {
        instructions: vec![instruction],
        recent_blockhash: Hash::default(),
    };

    let tx = Transaction {
        signatures: Vec::new(),
        message,
    };

    submit_transaction(state, tx)?;

    let evm_hash: [u8; 32] = evm_tx.hash.into();
    Ok(serde_json::json!(format!("0x{}", hex::encode(evm_hash))))
}

/// eth_call - Execute call without sending transaction (read-only)
async fn handle_eth_call(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let call = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_object())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [callObject, block]".to_string(),
        })?;

    let to = call.get("to").and_then(|v| v.as_str());
    let from = call.get("from").and_then(|v| v.as_str());
    let data = call.get("data").and_then(|v| v.as_str()).unwrap_or("0x");
    let value = call.get("value").and_then(|v| v.as_str()).unwrap_or("0x0");
    let gas = call.get("gas").and_then(|v| v.as_str()).unwrap_or("0x0");

    let to_address = to
        .map(moltchain_core::StateStore::parse_evm_address)
        .transpose()
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid to address: {}", e),
        })?
        .map(Address::from);

    let from_address = if let Some(from) = from {
        let parsed = moltchain_core::StateStore::parse_evm_address(from).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid from address: {}", e),
        })?;
        Address::from(parsed)
    } else {
        Address::ZERO
    };

    let data_hex = data.strip_prefix("0x").unwrap_or(data);
    let data_bytes = hex::decode(data_hex).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid data hex: {}", e),
    })?;

    let value_hex = value.strip_prefix("0x").unwrap_or(value);
    let gas_hex = gas.strip_prefix("0x").unwrap_or(gas);
    let value_u256 = U256::from_str_radix(value_hex, 16).unwrap_or(U256::ZERO);
    let gas_limit = u64::from_str_radix(gas_hex, 16).unwrap_or(1_000_000);

    let output = simulate_evm_call(
        state.state.clone(),
        from_address,
        to_address,
        Bytes::from(data_bytes),
        value_u256,
        gas_limit,
        state.evm_chain_id,
    )
    .map_err(|e| RpcError {
        code: -32000,
        message: e,
    })?;

    Ok(serde_json::json!(format!("0x{}", hex::encode(output))))
}

/// eth_blockNumber - Get current block number (slot)
async fn handle_eth_block_number(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let slot = state.state.get_last_slot().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    Ok(serde_json::json!(format!("0x{:x}", slot)))
}

/// eth_getTransactionReceipt - Get transaction receipt
async fn handle_eth_get_transaction_receipt(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let tx_hash_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [txHash]".to_string(),
        })?;

    // Parse transaction hash (strip 0x and convert to Hash)
    let tx_hash_str = tx_hash_str.strip_prefix("0x").unwrap_or(tx_hash_str);
    let hash = moltchain_core::hash::Hash::from_hex(tx_hash_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid transaction hash: {}", e),
    })?;

    let receipt = state.state.get_evm_receipt(&hash.0).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    if let Some(receipt) = receipt {
        let status = if receipt.status { "0x1" } else { "0x0" };
        let block_number = receipt.block_slot.map(|slot| format!("0x{:x}", slot));
        let block_hash = receipt
            .block_hash
            .map(|hash| format!("0x{}", hex::encode(hash)));
        let contract_address = receipt
            .contract_address
            .map(|addr| format!("0x{}", hex::encode(addr)));

        return Ok(serde_json::json!({
            "transactionHash": format!("0x{}", hex::encode(receipt.evm_hash)),
            "status": status,
            "gasUsed": format!("0x{:x}", receipt.gas_used),
            "blockNumber": block_number,
            "blockHash": block_hash,
            "contractAddress": contract_address,
        }));
    }

    Ok(serde_json::json!(null))
}

/// eth_getTransactionByHash - Get transaction by hash
async fn handle_eth_get_transaction_by_hash(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let tx_hash_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [txHash]".to_string(),
        })?;

    // Parse transaction hash
    let tx_hash_str = tx_hash_str.strip_prefix("0x").unwrap_or(tx_hash_str);
    let hash = moltchain_core::hash::Hash::from_hex(tx_hash_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid transaction hash: {}", e),
    })?;

    let record = state.state.get_evm_tx(&hash.0).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    if let Some(record) = record {
        let value = U256::from_be_bytes(record.value);
        let gas_price = U256::from_be_bytes(record.gas_price);
        let block_number = record.block_slot.map(|slot| format!("0x{:x}", slot));
        let block_hash = record
            .block_hash
            .map(|hash| format!("0x{}", hex::encode(hash)));
        return Ok(serde_json::json!({
            "hash": format!("0x{}", hex::encode(record.evm_hash)),
            "from": format!("0x{}", hex::encode(record.from)),
            "to": record.to.map(|addr| format!("0x{}", hex::encode(addr))),
            "nonce": format!("0x{:x}", record.nonce),
            "value": format!("0x{:x}", value),
            "gas": format!("0x{:x}", record.gas_limit),
            "gasPrice": format!("0x{:x}", gas_price),
            "input": format!("0x{}", hex::encode(record.data)),
            "blockNumber": block_number,
            "blockHash": block_hash,
        }));
    }

    Ok(serde_json::json!(null))
}

// ===== ReefStake Liquid Staking RPC Handlers =====

/// Handle stakeToReefStake: Stake MOLT, receive stMOLT
/// T2.5 fix: ReefStake deposit now requires a signed transaction.
/// Use sendTransaction with instruction type 13 (ReefStake deposit).
/// Data format: [13, amount(8 bytes LE)]
/// Accounts: [depositor_pubkey]
async fn handle_stake_to_reefstake(
    _state: &RpcState,
    _params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    Err(RpcError {
        code: -32601,
        message: "stakeToReefStake is deprecated. Use sendTransaction with system instruction \
                  type 13 (ReefStake deposit). Data: [13, amount_le_bytes(8)]. \
                  Accounts: [depositor_pubkey]. The transaction must be signed by the depositor."
            .to_string(),
    })
}

/// T2.5 fix: ReefStake unstake now requires a signed transaction.
/// Use sendTransaction with instruction type 14 (ReefStake unstake).
/// Data format: [14, st_molt_amount(8 bytes LE)]
/// Accounts: [user_pubkey]
async fn handle_unstake_from_reefstake(
    _state: &RpcState,
    _params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    Err(RpcError {
        code: -32601,
        message: "unstakeFromReefStake is deprecated. Use sendTransaction with system instruction \
                  type 14 (ReefStake unstake). Data: [14, st_molt_amount_le_bytes(8)]. \
                  Accounts: [user_pubkey]. The transaction must be signed by the user."
            .to_string(),
    })
}

/// T2.5 fix: ReefStake claim now requires a signed transaction.
/// Use sendTransaction with instruction type 15 (ReefStake claim).
/// Data format: [15]
/// Accounts: [user_pubkey]
async fn handle_claim_unstaked_tokens(
    _state: &RpcState,
    _params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    Err(RpcError {
        code: -32601,
        message: "claimUnstakedTokens is deprecated. Use sendTransaction with system instruction \
                  type 15 (ReefStake claim). Data: [15]. \
                  Accounts: [user_pubkey]. The transaction must be signed by the user."
            .to_string(),
    })
}

/// Handle getStakingPosition: Get user's ReefStake position
async fn handle_get_staking_position(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params: Vec<serde_json::Value> =
        params
            .and_then(|v| v.as_array().cloned())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Invalid params: expected [user_pubkey]".to_string(),
            })?;

    if params.is_empty() {
        return Err(RpcError {
            code: -32602,
            message: "Invalid params: expected [user_pubkey]".to_string(),
        });
    }

    let user_pubkey = params[0].as_str().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid user_pubkey".to_string(),
    })?;

    let user = Pubkey::from_base58(user_pubkey).map_err(|_| RpcError {
        code: -32602,
        message: "Invalid pubkey format".to_string(),
    })?;

    let pool = state.state.get_reefstake_pool().map_err(|e| RpcError {
        code: -32603,
        message: format!("Failed to get ReefStake pool: {}", e),
    })?;

    if let Some(position) = pool.positions.get(&user) {
        let current_value = pool.st_molt_token.st_molt_to_molt(position.st_molt_amount);
        Ok(serde_json::json!({
            "owner": user_pubkey,
            "st_molt_amount": position.st_molt_amount,
            "molt_deposited": position.molt_deposited,
            "current_value_molt": current_value,
            "rewards_earned": position.rewards_earned,
            "deposited_at": position.deposited_at
        }))
    } else {
        Ok(serde_json::json!({
            "owner": user_pubkey,
            "st_molt_amount": 0,
            "molt_deposited": 0,
            "current_value_molt": 0,
            "rewards_earned": 0,
            "deposited_at": 0
        }))
    }
}

/// Handle getReefStakePoolInfo: Get global ReefStake pool info
async fn handle_get_reefstake_pool_info(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    use moltchain_core::consensus::{SLOTS_PER_YEAR, TRANSACTION_BLOCK_REWARD};

    let pool = state.state.get_reefstake_pool().map_err(|e| RpcError {
        code: -32603,
        message: format!("Failed to get ReefStake pool: {}", e),
    })?;

    // Derive active validators count and APY from the consensus StakePool
    let (active_validators, apy_percent) = if let Some(ref sp_arc) = state.stake_pool {
        let sp = sp_arc.lock().await;
        let stats = sp.get_stats();
        let slots_per_day = SLOTS_PER_YEAR / 365;
        let apy_bp = pool.calculate_apy_bp(slots_per_day, TRANSACTION_BLOCK_REWARD);
        (stats.active_validators, apy_bp as f64 / 100.0)
    } else {
        (0, 0.0)
    };

    Ok(serde_json::json!({
        "total_supply_st_molt": pool.st_molt_token.total_supply,
        "total_molt_staked": pool.st_molt_token.total_molt_staked,
        "exchange_rate": pool.st_molt_token.exchange_rate_display(),
        "total_validators": active_validators,
        "average_apy_percent": apy_percent,
        "total_stakers": pool.positions.len()
    }))
}

/// Handle getUnstakingQueue: Get user's pending unstake requests
async fn handle_get_unstaking_queue(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params: Vec<serde_json::Value> =
        params
            .and_then(|v| v.as_array().cloned())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Invalid params: expected [user_pubkey]".to_string(),
            })?;

    if params.is_empty() {
        return Err(RpcError {
            code: -32602,
            message: "Invalid params: expected [user_pubkey]".to_string(),
        });
    }

    let user_pubkey = params[0].as_str().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid user_pubkey".to_string(),
    })?;

    let user = Pubkey::from_base58(user_pubkey).map_err(|_| RpcError {
        code: -32602,
        message: "Invalid pubkey format".to_string(),
    })?;

    let current_slot = state.state.get_last_slot().unwrap_or(0);

    let pool = state.state.get_reefstake_pool().map_err(|e| RpcError {
        code: -32603,
        message: format!("Failed to get ReefStake pool: {}", e),
    })?;

    let requests = pool.get_unstake_requests(&user);
    let mut total_claimable = 0u64;
    let pending_requests: Vec<serde_json::Value> = requests
        .iter()
        .map(|request| {
            if request.claimable_at <= current_slot {
                total_claimable += request.molt_to_receive;
            }
            serde_json::json!({
                "st_molt_amount": request.st_molt_amount,
                "molt_to_receive": request.molt_to_receive,
                "requested_at": request.requested_at,
                "claimable_at": request.claimable_at,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "owner": user_pubkey,
        "pending_requests": pending_requests,
        "total_claimable": total_claimable
    }))
}

/// Handle getRewardAdjustmentInfo: Get current reward adjustment and staking economics
async fn handle_get_reward_adjustment_info(
    state: &RpcState,
) -> Result<serde_json::Value, RpcError> {
    use moltchain_core::consensus::{
        HEARTBEAT_BLOCK_REWARD, MIN_VALIDATOR_STAKE, SLOTS_PER_YEAR, TRANSACTION_BLOCK_REWARD,
    };

    let stake_pool_arc = state.stake_pool.as_ref().ok_or_else(|| RpcError {
        code: -32000,
        message: "Stake pool not available".to_string(),
    })?;
    let stake_pool = stake_pool_arc.lock().await;
    let stats = stake_pool.get_stats();
    let active_count = stats.active_validators;
    let total_staked = stats.total_staked;

    // Calculate effective APY
    let annual_tx_rewards = TRANSACTION_BLOCK_REWARD as f64 * SLOTS_PER_YEAR as f64;
    let apy = if total_staked > 0 {
        (annual_tx_rewards / total_staked as f64) * 100.0
    } else {
        0.0
    };

    Ok(serde_json::json!({
        "currentMultiplier": 1.0,
        "priceOracleActive": false,
        "transactionBlockReward": TRANSACTION_BLOCK_REWARD,
        "heartbeatBlockReward": HEARTBEAT_BLOCK_REWARD,
        "slotsPerYear": SLOTS_PER_YEAR,
        "minValidatorStake": MIN_VALIDATOR_STAKE,
        "totalStaked": total_staked,
        "totalSlashed": stats.total_slashed,
        "activeValidators": active_count,
        "unclaimedRewards": stats.total_unclaimed_rewards,
        "estimatedApy": format!("{:.2}", apy),
        "note": "Price-based reward adjustment will activate when oracle is configured"
    }))
}

// ============================================================================
// TOKEN ENDPOINTS
// ============================================================================

/// Get token balance for a holder: params = [token_program, holder]
async fn handle_get_token_balance(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Expected array params".to_string(),
    })?;

    let token_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing token_program".to_string(),
        })?;
    let holder_str = arr
        .get(1)
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing holder".to_string(),
        })?;

    let token_program = Pubkey::from_base58(token_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid token_program: {}", e),
    })?;
    let holder = Pubkey::from_base58(holder_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid holder: {}", e),
    })?;

    let balance = state
        .state
        .get_token_balance(&token_program, &holder)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    Ok(serde_json::json!({
        "token_program": token_str,
        "holder": holder_str,
        "balance": balance,
    }))
}

/// Get token holders: params = [token_program, limit?]
async fn handle_get_token_holders(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Expected array params".to_string(),
    })?;

    let token_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing token_program".to_string(),
        })?;
    let limit = arr.get(1).and_then(|v| v.as_u64()).unwrap_or(100) as usize;

    let token_program = Pubkey::from_base58(token_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid token_program: {}", e),
    })?;

    let holders = state
        .state
        .get_token_holders(&token_program, limit)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let holder_list: Vec<serde_json::Value> = holders
        .iter()
        .map(|(pk, bal)| {
            serde_json::json!({
                "holder": pk.to_base58(),
                "balance": bal,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "token_program": token_str,
        "holders": holder_list,
        "count": holder_list.len(),
    }))
}

/// Get token transfers: params = [token_program, limit?]
async fn handle_get_token_transfers(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Expected array params".to_string(),
    })?;

    let token_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing token_program".to_string(),
        })?;
    let limit = arr.get(1).and_then(|v| v.as_u64()).unwrap_or(100) as usize;

    let token_program = Pubkey::from_base58(token_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid token_program: {}", e),
    })?;

    let transfers = state
        .state
        .get_token_transfers(&token_program, limit)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let transfer_list: Vec<serde_json::Value> = transfers
        .iter()
        .map(|t| {
            serde_json::json!({
                "from": t.from,
                "to": t.to,
                "amount": t.amount,
                "slot": t.slot,
                "tx_hash": t.tx_hash,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "token_program": token_str,
        "transfers": transfer_list,
        "count": transfer_list.len(),
    }))
}

/// Get contract events: params = [program_id, limit?]
async fn handle_get_contract_events(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Expected array params".to_string(),
    })?;

    let program_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing program_id".to_string(),
        })?;
    let limit = arr.get(1).and_then(|v| v.as_u64()).unwrap_or(100) as usize;

    let program = Pubkey::from_base58(program_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid program_id: {}", e),
    })?;

    let events = state
        .state
        .get_events_by_program(&program, limit)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let event_list: Vec<serde_json::Value> = events
        .iter()
        .map(|e| {
            serde_json::json!({
                "program": e.program.to_base58(),
                "name": e.name,
                "data": e.data,
                "slot": e.slot,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "program": program_str,
        "events": event_list,
        "count": event_list.len(),
    }))
}

/// Testnet-only airdrop: credits MOLT from treasury to a given address.
/// Usage: requestAirdrop [address, amount_in_molt]
/// This mints tokens from treasury to support testnet development/faucet.
async fn handle_request_airdrop(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    // H16 fix: reject in multi-validator mode (direct state write bypasses consensus)
    require_single_validator(state, "requestAirdrop")?;

    // Only allow on testnet / devnet (not mainnet)
    if state.network_id.contains("mainnet")
        || (!state.network_id.contains("testnet")
            && !state.network_id.contains("devnet")
            && !state.network_id.contains("local"))
    {
        return Err(RpcError {
            code: -32003,
            message: "Airdrop only available on testnet/devnet".to_string(),
        });
    }

    let params = params.ok_or(RpcError {
        code: -32602,
        message: "Expected params: [address, amount_in_molt]".to_string(),
    })?;

    let arr = params.as_array().ok_or(RpcError {
        code: -32602,
        message: "Expected array params: [address, amount_in_molt]".to_string(),
    })?;

    if arr.len() < 2 {
        return Err(RpcError {
            code: -32602,
            message: "Expected params: [address, amount_in_molt]".to_string(),
        });
    }

    let address_str = arr[0].as_str().ok_or(RpcError {
        code: -32602,
        message: "address must be a string".to_string(),
    })?;

    let amount_molt = arr[1].as_u64().ok_or(RpcError {
        code: -32602,
        message: "amount must be an integer (MOLT)".to_string(),
    })?;

    if amount_molt == 0 || amount_molt > 100 {
        return Err(RpcError {
            code: -32602,
            message: "Amount must be between 1 and 100 MOLT".to_string(),
        });
    }

    let recipient = Pubkey::from_base58(address_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid address: {}", e),
    })?;

    let amount_shells = amount_molt * 1_000_000_000;

    // Get treasury
    let treasury_pubkey = state
        .state
        .get_treasury_pubkey()
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to get treasury: {}", e),
        })?
        .ok_or(RpcError {
            code: -32000,
            message: "No treasury configured".to_string(),
        })?;

    let mut treasury_account = state
        .state
        .get_account(&treasury_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to read treasury: {}", e),
        })?
        .ok_or(RpcError {
            code: -32000,
            message: "Treasury account not found".to_string(),
        })?;

    if treasury_account.spendable < amount_shells {
        return Err(RpcError {
            code: -32000,
            message: "Insufficient treasury balance for airdrop".to_string(),
        });
    }

    // Debit treasury
    treasury_account
        .deduct_spendable(amount_shells)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to debit treasury: {}", e),
        })?;
    state
        .state
        .put_account(&treasury_pubkey, &treasury_account)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to save treasury: {}", e),
        })?;

    // Credit recipient
    let mut recipient_account = state
        .state
        .get_account(&recipient)
        .unwrap_or(None)
        .unwrap_or_else(|| Account::new(0, SYSTEM_ACCOUNT_OWNER));

    recipient_account
        .add_spendable(amount_shells)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to credit recipient: {}", e),
        })?;
    state
        .state
        .put_account(&recipient, &recipient_account)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to save recipient: {}", e),
        })?;

    info!(
        "💧 Airdrop: {} MOLT from treasury to {}",
        amount_molt, address_str
    );

    Ok(serde_json::json!({
        "success": true,
        "amount": amount_molt,
        "recipient": address_str,
        "message": format!("{} MOLT airdropped successfully", amount_molt),
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        filter_signatures_for_address, validate_solana_encoding,
        validate_solana_transaction_details,
    };
    use moltchain_core::Hash;

    fn make_hash(value: u8) -> Hash {
        Hash([value; 32])
    }

    #[test]
    fn test_validate_solana_encoding() {
        assert!(validate_solana_encoding("json").is_ok());
        assert!(validate_solana_encoding("base58").is_ok());
        assert!(validate_solana_encoding("base64").is_ok());
        assert!(validate_solana_encoding("binary").is_err());
    }

    #[test]
    fn test_validate_solana_transaction_details() {
        assert!(validate_solana_transaction_details("full").is_ok());
        assert!(validate_solana_transaction_details("signatures").is_ok());
        assert!(validate_solana_transaction_details("none").is_ok());
        assert!(validate_solana_transaction_details("accounts").is_err());
    }

    #[test]
    fn test_filter_signatures_for_address() {
        let h1 = make_hash(1);
        let h2 = make_hash(2);
        let h3 = make_hash(3);
        let h4 = make_hash(4);

        let indexed = vec![(h4, 4), (h3, 3), (h2, 2), (h1, 1)];

        let filtered = filter_signatures_for_address(indexed.clone(), Some(h3), None, 10);
        assert_eq!(filtered, vec![(h2, 2), (h1, 1)]);

        let filtered = filter_signatures_for_address(indexed.clone(), None, Some(h2), 10);
        assert_eq!(filtered, vec![(h4, 4), (h3, 3)]);

        let filtered = filter_signatures_for_address(indexed.clone(), Some(h3), Some(h1), 10);
        assert_eq!(filtered, vec![(h2, 2)]);

        let filtered = filter_signatures_for_address(indexed, None, None, 1);
        assert_eq!(filtered, vec![(h4, 4)]);
    }
}
