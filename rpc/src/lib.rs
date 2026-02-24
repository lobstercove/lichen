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
//   NETWORK ENDPOINTS              — getPeers, getNetworkInfo, getClusterInfo
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
pub mod launchpad;
pub mod prediction;
pub mod shielded;
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
    decode_evm_transaction, shells_to_u256, simulate_evm_call, Account, FinalityTracker, Hash,
    Instruction, MarketActivityKind, Pubkey, StakePool, StateStore, SymbolRegistryEntry,
    Transaction, TxProcessor, CONTRACT_PROGRAM_ID, EVM_PROGRAM_ID, SYSTEM_PROGRAM_ID,
};

/// System account owner (Pubkey([0x01; 32]))
const SYSTEM_ACCOUNT_OWNER: Pubkey = Pubkey([0x01; 32]);

/// P9-RPC-02: Maximum size for bincode transaction deserialization.
/// Prevents OOM/DoS from maliciously large payloads.  4 MiB matches
/// the contract-deploy datasize limit enforced by `validate_structure()`.
const MAX_TX_BINCODE_SIZE: u64 = 4 * 1024 * 1024;

/// P9-RPC-02: Bounded bincode deserialization for Transaction.
/// Uses `bincode::options().with_limit()` to reject payloads that
/// exceed `MAX_TX_BINCODE_SIZE` before allocating memory.
fn bounded_bincode_deserialize(bytes: &[u8]) -> Result<Transaction, bincode::Error> {
    use bincode::Options;
    bincode::options()
        .with_limit(MAX_TX_BINCODE_SIZE)
        .with_fixint_encoding()
        .allow_trailing_bytes()
        .deserialize(bytes)
}
use moltchain_core::consensus::{decayed_reward, ValidatorInfo, HEARTBEAT_BLOCK_REWARD, TRANSACTION_BLOCK_REWARD};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex, RwLock};
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
    stake_pool: Option<Arc<tokio::sync::RwLock<moltchain_core::StakePool>>>,
    chain_id: String,
    network_id: String,
    version: String,
    evm_chain_id: u64,
    solana_tx_cache: Arc<Mutex<LruCache<Hash, SolanaTxRecord>>>,
    /// Admin token for state-mutating RPC endpoints (setFeeConfig, setRentParams, setContractAbi)
    /// Hot-rotatable: a background task re-reads MOLTCHAIN_ADMIN_TOKEN env var every 30s.
    admin_token: Arc<std::sync::RwLock<Option<String>>>,
    /// T2.6: Per-IP rate limiter
    rate_limiter: Arc<RateLimiter>,
    /// Lock-free finality tracker for commitment levels (processed/confirmed/finalized)
    finality: Option<FinalityTracker>,
    /// DEX real-time event broadcaster (WS push to subscribers)
    _dex_broadcaster: Arc<dex_ws::DexEventBroadcaster>,
    /// Prediction market real-time event broadcaster
    #[allow(dead_code)]
    prediction_broadcaster: Arc<ws::PredictionEventBroadcaster>,
    /// Cached validators list — refreshed at most once per slot (~400ms).
    /// Avoids 6+ full CF_VALIDATORS scans per RPC cycle.
    validator_cache: Arc<RwLock<(Instant, Vec<ValidatorInfo>)>>,
    /// Cached metrics JSON — refreshed at most once per slot (~400ms).
    metrics_cache: Arc<RwLock<(Instant, Option<serde_json::Value>)>>,
    /// AUDIT-FIX RPC-4: Per-address airdrop cooldown to prevent abuse
    airdrop_cooldowns: Arc<std::sync::Mutex<HashMap<String, Instant>>>,
    /// DEX orderbook cache — per-pair aggregated book levels, refreshed at most once per second.
    /// Eliminates O(total_orders) scan per request; cached result served in O(1).
    orderbook_cache: Arc<RwLock<HashMap<u64, (Instant, serde_json::Value)>>>,
}

/// H16 fix: Guard state-mutating RPC endpoints in multi-validator mode.
/// Direct state writes bypass consensus and cause divergence when >1 validator.
/// In multi-validator mode, callers must submit proper signed transactions
/// via `sendTransaction` instead.
pub(crate) fn require_single_validator(state: &RpcState, endpoint: &str) -> Result<(), RpcError> {
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

/// Cached validator list — avoids redundant CF_VALIDATORS full-scans within a
/// single slot (~400ms).  Six RPC handlers previously scanned the same CF
/// independently; this collapses them into at most one scan per slot.
const VALIDATOR_CACHE_TTL_MS: u128 = 400;

async fn cached_validators(state: &RpcState) -> Result<Vec<ValidatorInfo>, RpcError> {
    // Fast path: read lock
    {
        let guard = state.validator_cache.read().await;
        if guard.0.elapsed().as_millis() < VALIDATOR_CACHE_TTL_MS && !guard.1.is_empty() {
            return Ok(guard.1.clone());
        }
    }
    // Slow path: write lock + refresh
    let mut guard = state.validator_cache.write().await;
    // Double check — another task may have refreshed while we waited for the write lock
    if guard.0.elapsed().as_millis() < VALIDATOR_CACHE_TTL_MS && !guard.1.is_empty() {
        return Ok(guard.1.clone());
    }
    let validators = state.state.get_all_validators().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;
    *guard = (Instant::now(), validators.clone());
    Ok(validators)
}

/// Verify admin authorization from params
fn verify_admin_auth(state: &RpcState, params: &Option<serde_json::Value>) -> Result<(), RpcError> {
    let guard = state.admin_token.read().map_err(|_| RpcError {
        code: -32000,
        message: "Internal error: admin token lock poisoned".to_string(),
    })?;
    let required_token = guard.as_ref().ok_or_else(|| RpcError {
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

/// P9-RPC-03: Method cost tier for tiered rate limiting.
/// Expensive methods (e.g., sendTransaction, simulateTransaction) get a lower
/// per-second cap than cheap read-only queries (e.g., getBalance, getSlot).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum MethodTier {
    /// Cheap read-only lookups (getBalance, getSlot, health)
    Cheap,
    /// Moderate reads that touch indexes or iterate (getTransactionsByAddress)
    Moderate,
    /// Expensive writes or simulations (sendTransaction, simulateTransaction)
    Expensive,
}

/// P9-RPC-03: Classify an RPC method name into a cost tier.
fn classify_method(method: &str) -> MethodTier {
    match method {
        // Writes / simulations
        "sendTransaction"
        | "simulateTransaction"
        | "deployContract"
        | "upgradeContract"
        | "stake"
        | "unstake"
        | "stakeToReefStake"
        | "unstakeFromReefStake"
        | "claimUnstakedTokens"
        | "requestAirdrop"
        | "setFeeConfig"
        | "setRentParams"
        | "setContractAbi" => MethodTier::Expensive,

        // Moderate reads (iterate indexes, join data)
        "getTransactionsByAddress"
        | "getTransactionHistory"
        | "getRecentTransactions"
        | "getTokenHolders"
        | "getTokenTransfers"
        | "getContractEvents"
        | "getContractLogs"
        | "getNFTsByOwner"
        | "getNFTsByCollection"
        | "getNFTActivity"
        | "getMarketListings"
        | "getMarketSales"
        | "getProgramCalls"
        | "getProgramStorage"
        | "getPrograms"
        | "getAllContracts"
        | "getAllSymbolRegistry"
        | "getPredictionMarkets"
        | "getPredictionLeaderboard"
        | "batchReverseMoltNames"
        | "searchMoltNames"
        | "getUnstakingQueue" => MethodTier::Moderate,

        // Everything else is a cheap point lookup
        _ => MethodTier::Cheap,
    }
}

/// T2.6: Per-IP rate limiter with stale entry pruning
/// AUDIT-FIX 2.17: std::sync::Mutex is intentional here — the critical section
/// is a fast HashMap lookup/insert with no `.await` points, consistent with
/// tokio's recommendation to use std::sync::Mutex for short non-async sections.
struct RateLimiter {
    requests: std::sync::Mutex<HashMap<IpAddr, (u64, Instant)>>,
    max_per_second: u64,
    last_prune: std::sync::Mutex<Instant>,
    /// P9-RPC-03: Per-tier per-IP counters.
    /// Key = (IpAddr, MethodTier), Value = (count, window_start).
    tier_requests: std::sync::Mutex<HashMap<(IpAddr, MethodTier), (u64, Instant)>>,
    /// Per-second limits for each tier.
    tier_limits: [u64; 3], // [Cheap, Moderate, Expensive]
}

impl RateLimiter {
    fn new(max_per_second: u64) -> Self {
        Self {
            requests: std::sync::Mutex::new(HashMap::new()),
            max_per_second,
            last_prune: std::sync::Mutex::new(Instant::now()),
            tier_requests: std::sync::Mutex::new(HashMap::new()),
            // P9-RPC-03: Default tier limits.
            // Cheap: 100% of global cap, Moderate: 40%, Expensive: 10%
            tier_limits: [
                max_per_second,                         // Cheap
                max_per_second * 2 / 5,                 // Moderate (40%)
                std::cmp::max(max_per_second / 10, 50), // Expensive (10%, min 50)
            ],
        }
    }

    /// Check if a request from `ip` is within the global rate limit.
    /// Returns `true` if allowed, `false` if rate-limited.
    fn check(&self, ip: IpAddr) -> bool {
        let mut map = self.requests.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();

        // Prune stale entries every 30 seconds to prevent memory exhaustion
        {
            let mut last = self.last_prune.lock().unwrap_or_else(|e| e.into_inner());
            if now.duration_since(*last).as_secs() >= 30 {
                map.retain(|_, (_, ts)| now.duration_since(*ts).as_secs() < 60);
                // Also prune tier counters
                if let Ok(mut tier_map) = self.tier_requests.lock() {
                    tier_map.retain(|_, (_, ts)| now.duration_since(*ts).as_secs() < 60);
                }
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

    /// P9-RPC-03: Check if a request from `ip` for method `tier` is within
    /// the tier-specific rate limit.  Should be called AFTER `check()` passes.
    fn check_tier(&self, ip: IpAddr, tier: MethodTier) -> bool {
        let limit = self.tier_limits[tier as usize];
        let mut map = self.tier_requests.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        let entry = map.entry((ip, tier)).or_insert((0, now));
        if now.duration_since(entry.1).as_secs() >= 1 {
            entry.0 = 1;
            entry.1 = now;
            true
        } else {
            entry.0 += 1;
            entry.0 <= limit
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

/// Helper: Count executable accounts (contracts) using O(1) MetricsStore counter
fn count_executable_accounts(state: &StateStore) -> u64 {
    state.get_program_count()
}

fn parse_transfer_amount(ix: &Instruction) -> Option<u64> {
    if ix.program_id != SYSTEM_PROGRAM_ID {
        return None;
    }
    if ix.data.len() < 9 {
        return None;
    }
    // Parse amount from data[1..9] for instruction types that carry an amount:
    // 0=Transfer, 2=Reward, 3=GrantRepay, 4=GenesisTransfer, 5=GenesisMint,
    // 9=Stake, 10=Unstake, 13=ReefStakeDeposit, 14=ReefStakeUnstake,
    // 16=ReefStakeTransfer, 19=FaucetAirdrop, 23=Shield, 24=Unshield
    match ix.data[0] {
        0 | 2 | 3 | 4 | 5 | 9 | 10 | 13 | 14 | 16 | 19 | 23 | 24 => {
            let amount_bytes: [u8; 8] = ix.data[1..9].try_into().ok()?;
            Some(u64::from_le_bytes(amount_bytes))
        }
        _ => None,
    }
}

fn instruction_type(ix: &Instruction) -> &'static str {
    if ix.program_id == SYSTEM_PROGRAM_ID {
        if ix.data.first() == Some(&0) {
            return "Transfer";
        }
        if ix.data.first() == Some(&1) {
            return "CreateAccount";
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
        if ix.data.first() == Some(&13) {
            return "ReefStakeDeposit";
        }
        if ix.data.first() == Some(&14) {
            return "ReefStakeUnstake";
        }
        if ix.data.first() == Some(&15) {
            return "ReefStakeClaim";
        }
        if ix.data.first() == Some(&16) {
            return "ReefStakeTransfer";
        }
        if ix.data.first() == Some(&17) {
            return "DeployContract";
        }
        if ix.data.first() == Some(&18) {
            return "SetContractABI";
        }
        if ix.data.first() == Some(&19) {
            return "FaucetAirdrop";
        }
        if ix.data.first() == Some(&20) {
            return "RegisterSymbol";
        }
        if ix.data.first() == Some(&23) {
            return "Shield";
        }
        if ix.data.first() == Some(&24) {
            return "Unshield";
        }
        if ix.data.first() == Some(&25) {
            return "ShieldedTransfer";
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

/// Collect unique account keys from a transaction in Solana-compatible order.
fn collect_account_keys(tx: &Transaction) -> Vec<Pubkey> {
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

    account_keys
}

fn solana_message_json(tx: &Transaction) -> (Vec<String>, Vec<serde_json::Value>) {
    let account_keys = collect_account_keys(tx);

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

/// Look up current balances for all account keys in a transaction.
/// Returns current state balances (post-execution view).
fn account_balances(state: &StateStore, tx: &Transaction) -> Vec<u64> {
    let keys = collect_account_keys(tx);
    keys.iter()
        .map(|pk| {
            state
                .get_account(pk)
                .ok()
                .flatten()
                .map(|a| a.spendable)
                .unwrap_or(0)
        })
        .collect()
}

fn solana_transaction_json(
    state: &StateStore,
    tx: &Transaction,
    slot: u64,
    timestamp: u64,
    fee: u64,
) -> serde_json::Value {
    let (account_keys, instructions) = solana_message_json(tx);
    let signature = hash_to_base58(&tx.signature());

    // F1: Populate balances from current state.
    // postBalances = current on-chain balances for each account key.
    // preBalances = postBalances adjusted by fee (payer index 0 gets fee added back).
    let post_balances = account_balances(state, tx);
    let pre_balances: Vec<u64> = post_balances
        .iter()
        .enumerate()
        .map(|(i, &bal)| if i == 0 { bal.saturating_add(fee) } else { bal })
        .collect();

    serde_json::json!({
        "slot": slot,
        "blockTime": timestamp,
        "meta": {
            "err": serde_json::Value::Null,
            "fee": fee,
            "preBalances": pre_balances,
            "postBalances": post_balances,
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
    state: &StateStore,
    tx: &Transaction,
    slot: u64,
    timestamp: u64,
    fee: u64,
    encoding: &str,
) -> serde_json::Value {
    let encoded = encode_solana_transaction(tx, encoding);

    let post_balances = account_balances(state, tx);
    // AUDIT-FIX F-9: Reconstruct pre-balances from transaction effects.
    // The fee payer (account[0]) had fee added to their balance before deduction.
    // For transfer instructions (opcode 0), we also reconstruct the amount moved.
    let transfer_amount = tx
        .message
        .instructions
        .first()
        .and_then(|ix| {
            if ix.data.first() == Some(&0) && ix.data.len() >= 9 {
                Some(u64::from_le_bytes(
                    ix.data[1..9].try_into().unwrap_or([0; 8]),
                ))
            } else {
                None
            }
        })
        .unwrap_or(0);
    let pre_balances: Vec<u64> = post_balances
        .iter()
        .enumerate()
        .map(|(i, &bal)| {
            if i == 0 {
                // Fee payer: add back fee AND outgoing transfer
                bal.saturating_add(fee).saturating_add(transfer_amount)
            } else if i == 1 && transfer_amount > 0 {
                // Transfer recipient: subtract received amount
                bal.saturating_sub(transfer_amount)
            } else {
                bal
            }
        })
        .collect();

    serde_json::json!({
        "slot": slot,
        "blockTime": timestamp,
        "meta": {
            "err": serde_json::Value::Null,
            "fee": fee,
            "preBalances": pre_balances,
            "postBalances": post_balances,
            "logMessages": [],
        },
        "transaction": [encoded, encoding],
    })
}

fn solana_block_transaction_json(
    state: &StateStore,
    tx: &Transaction,
    fee: u64,
) -> serde_json::Value {
    let (account_keys, instructions) = solana_message_json(tx);
    let signature = hash_to_base58(&tx.signature());

    let post_balances = account_balances(state, tx);
    let pre_balances: Vec<u64> = post_balances
        .iter()
        .enumerate()
        .map(|(i, &bal)| if i == 0 { bal.saturating_add(fee) } else { bal })
        .collect();

    serde_json::json!({
        "meta": {
            "err": serde_json::Value::Null,
            "fee": fee,
            "preBalances": pre_balances,
            "postBalances": post_balances,
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
    state: &StateStore,
    tx: &Transaction,
    fee: u64,
    encoding: &str,
) -> serde_json::Value {
    let encoded = encode_solana_transaction(tx, encoding);
    let signature = hash_to_base58(&tx.signature());

    let post_balances = account_balances(state, tx);
    let pre_balances: Vec<u64> = post_balances
        .iter()
        .enumerate()
        .map(|(i, &bal)| if i == 0 { bal.saturating_add(fee) } else { bal })
        .collect();

    serde_json::json!({
        "meta": {
            "err": serde_json::Value::Null,
            "fee": fee,
            "preBalances": pre_balances,
            "postBalances": post_balances,
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
    stake_pool: Option<Arc<RwLock<StakePool>>>,
    p2p: Option<Arc<dyn P2PNetworkTrait>>,
    chain_id: String,
    network_id: String,
    admin_token: Option<String>,
    finality: Option<FinalityTracker>,
    dex_broadcaster: Option<Arc<dex_ws::DexEventBroadcaster>>,
    prediction_broadcaster: Option<Arc<ws::PredictionEventBroadcaster>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let app = build_rpc_router(
        state,
        tx_sender,
        stake_pool,
        p2p,
        chain_id,
        network_id,
        admin_token,
        finality,
        dex_broadcaster,
        prediction_broadcaster,
    );

    let addr = format!("0.0.0.0:{}", port);
    info!("🌐 RPC server starting on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;

    // STABILITY-FIX: Limit concurrent connections to prevent RPC load from
    // starving block production. 8192 supports 5000+ concurrent traders
    // while still providing backpressure under extreme load.
    use tower::limit::ConcurrencyLimitLayer;
    let app = app.layer(ConcurrencyLimitLayer::new(8192));

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn build_rpc_router(
    state: StateStore,
    tx_sender: Option<mpsc::Sender<Transaction>>,
    stake_pool: Option<Arc<RwLock<StakePool>>>,
    p2p: Option<Arc<dyn P2PNetworkTrait>>,
    chain_id: String,
    network_id: String,
    admin_token: Option<String>,
    finality: Option<FinalityTracker>,
    dex_broadcaster: Option<Arc<dex_ws::DexEventBroadcaster>>,
    prediction_broadcaster: Option<Arc<ws::PredictionEventBroadcaster>>,
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
    let admin_token = Arc::new(std::sync::RwLock::new(admin_token));

    // Spawn background task to hot-reload admin token from MOLTCHAIN_ADMIN_TOKEN env var
    {
        let token_ref = Arc::clone(&admin_token);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                interval.tick().await;
                if let Ok(new_val) = std::env::var("MOLTCHAIN_ADMIN_TOKEN") {
                    let new_token = if new_val.is_empty() {
                        None
                    } else {
                        Some(new_val)
                    };
                    if let Ok(mut guard) = token_ref.write() {
                        if *guard != new_token {
                            info!("Admin token rotated via MOLTCHAIN_ADMIN_TOKEN env var");
                            *guard = new_token;
                        }
                    }
                }
            }
        });
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
        rate_limiter: Arc::new(RateLimiter::new(
            std::env::var("RPC_RATE_LIMIT")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(5_000),
        )),
        finality,
        _dex_broadcaster: dex_broadcaster
            .unwrap_or_else(|| Arc::new(dex_ws::DexEventBroadcaster::new(4096))),
        prediction_broadcaster: prediction_broadcaster
            .unwrap_or_else(|| Arc::new(ws::PredictionEventBroadcaster::new(1024))),
        validator_cache: Arc::new(RwLock::new((
            Instant::now() - std::time::Duration::from_secs(60),
            Vec::new(),
        ))),
        metrics_cache: Arc::new(RwLock::new((
            Instant::now() - std::time::Duration::from_secs(60),
            None,
        ))),
        airdrop_cooldowns: Arc::new(std::sync::Mutex::new(HashMap::new())),
        orderbook_cache: Arc::new(RwLock::new(HashMap::new())),
    };

    // D1-01: Configurable CORS origins via MOLTCHAIN_CORS_ORIGINS env var
    // (comma-separated).  Defaults to localhost-only + moltchain.io subdomains.
    // Set to "*" for development-only wildcard (NOT recommended for production).
    let allowed_hosts: Vec<String> = std::env::var("MOLTCHAIN_CORS_ORIGINS")
        .ok()
        .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_else(|| {
            vec![
                "localhost".to_string(),
                "127.0.0.1".to_string(),
                "moltchain.io".to_string(),
                "app.moltchain.io".to_string(),
                "rpc.moltchain.io".to_string(),
                "api.moltchain.io".to_string(),
                "explorer.moltchain.io".to_string(),
            ]
        });
    let allowed_hosts = Arc::new(allowed_hosts);

    // T2.7: Restrictive CORS — allow configured origins only
    // H14 fix: use exact host matching to prevent subdomain bypass
    let cors_hosts = allowed_hosts.clone();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(move |origin: &HeaderValue, _| {
            let origin_str = origin.to_str().unwrap_or("");
            // Parse scheme://host:port — only allow exact matching hosts
            if let Some(rest) = origin_str
                .strip_prefix("http://")
                .or_else(|| origin_str.strip_prefix("https://"))
            {
                let host = rest.split('/').next().unwrap_or("");
                let host_only = host.split(':').next().unwrap_or("");
                cors_hosts.iter().any(|allowed| allowed == host_only)
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
        // Prediction Market REST API — /api/v1/prediction-market/*
        .nest(
            "/api/v1/prediction-market",
            prediction::build_prediction_router(),
        )
        // ClawPump Launchpad REST API — /api/v1/launchpad/*
        .nest("/api/v1/launchpad", launchpad::build_launchpad_router())
        // Shielded Pool REST API — /api/v1/shielded/*
        .nest("/api/v1/shielded", shielded::build_shielded_router())
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
async fn handle_rpc(
    State(state): State<Arc<RpcState>>,
    connect_info: Option<ConnectInfo<SocketAddr>>,
    Json(req): Json<RpcRequest>,
) -> Response {
    // P9-RPC-03: Tiered rate limiting — classify the method and enforce
    // a per-tier per-IP limit on top of the global rate limit.
    let tier = classify_method(&req.method);
    if tier != MethodTier::Cheap {
        let ip = connect_info
            .map(|ci| ci.0.ip())
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        if !state.rate_limiter.check_tier(ip, tier) {
            let label = match tier {
                MethodTier::Expensive => "expensive",
                MethodTier::Moderate => "moderate",
                MethodTier::Cheap => "cheap",
            };
            warn!(
                "P9-RPC-03: {} method rate limit exceeded for IP {}",
                label, ip
            );
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": req.id,
                    "error": {"code": -32005, "message": format!("Rate limit exceeded for {} methods", label)}
                })),
            )
                .into_response();
        }
    }

    // Route to appropriate handler
    let result = match req.method.as_str() {
        // Basic queries (canonical Molt endpoints)
        "getBalance" => handle_get_balance(&state, req.params).await,
        "getAccount" => handle_get_account(&state, req.params).await,
        "getBlock" => handle_get_block(&state, req.params).await,
        "getLatestBlock" => handle_get_latest_block(&state).await,
        "getSlot" => handle_get_slot(&state, req.params).await,
        "getTransaction" => handle_get_transaction(&state, req.params).await,
        "getTransactionsByAddress" => handle_get_transactions_by_address(&state, req.params).await,
        "getAccountTxCount" => handle_get_account_tx_count(&state, req.params).await,
        "getRecentTransactions" => handle_get_recent_transactions(&state, req.params).await,
        "getTokenAccounts" => handle_get_token_accounts(&state, req.params).await,
        "sendTransaction" => handle_send_transaction(&state, req.params).await,
        "confirmTransaction" => handle_confirm_transaction(&state, req.params).await,
        "simulateTransaction" => handle_simulate_transaction(&state, req.params).await,
        "getTotalBurned" => handle_get_total_burned(&state).await,
        "getValidators" => handle_get_validators(&state).await,
        "getMetrics" => handle_get_metrics(&state).await,
        "getTreasuryInfo" => handle_get_treasury_info(&state).await,
        "getGenesisAccounts" => handle_get_genesis_accounts(&state).await,
        "getGovernedProposal" => handle_get_governed_proposal(&state, req.params).await,
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
        "getClusterInfo" => handle_get_cluster_info(&state).await,

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
        "upgradeContract" => handle_upgrade_contract(&state, req.params).await,

        // Program endpoints (draft)
        "getProgram" => handle_get_program(&state, req.params).await,
        "getProgramStats" => handle_get_program_stats(&state, req.params).await,
        "getPrograms" => handle_get_programs(&state, req.params).await,
        "getProgramCalls" => handle_get_program_calls(&state, req.params).await,
        "getProgramStorage" => handle_get_program_storage(&state, req.params).await,

        // MoltyID endpoints
        "getMoltyIdIdentity" => handle_get_moltyid_identity(&state, req.params).await,
        "getMoltyIdReputation" => handle_get_moltyid_reputation(&state, req.params).await,
        "getMoltyIdSkills" => handle_get_moltyid_skills(&state, req.params).await,
        "getMoltyIdVouches" => handle_get_moltyid_vouches(&state, req.params).await,
        "getMoltyIdAchievements" => handle_get_moltyid_achievements(&state, req.params).await,
        "getMoltyIdProfile" => handle_get_moltyid_profile(&state, req.params).await,
        "resolveMoltName" => handle_resolve_molt_name(&state, req.params).await,
        "reverseMoltName" => handle_reverse_molt_name(&state, req.params).await,
        "batchReverseMoltNames" => handle_batch_reverse_molt_names(&state, req.params).await,
        "searchMoltNames" => handle_search_molt_names(&state, req.params).await,
        "getMoltyIdAgentDirectory" => handle_get_moltyid_agent_directory(&state, req.params).await,
        "getMoltyIdStats" => handle_get_moltyid_stats(&state).await,

        // EVM address registry
        "getEvmRegistration" => handle_get_evm_registration(&state, req.params).await,
        "lookupEvmAddress" => handle_lookup_evm_address(&state, req.params).await,

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

        // Prediction Market endpoints
        "getPredictionMarketStats" => handle_get_prediction_stats(&state).await,
        "getPredictionMarkets" => handle_get_prediction_markets(&state, req.params).await,
        "getPredictionMarket" => handle_get_prediction_market(&state, req.params).await,
        "getPredictionPositions" => handle_get_prediction_positions(&state, req.params).await,
        "getPredictionTraderStats" => handle_get_prediction_trader_stats(&state, req.params).await,
        "getPredictionLeaderboard" => handle_get_prediction_leaderboard(&state, req.params).await,
        "getPredictionTrending" => handle_get_prediction_trending(&state).await,
        "getPredictionMarketAnalytics" => {
            handle_get_prediction_market_analytics(&state, req.params).await
        }

        // DEX & Platform Stats endpoints
        "getDexCoreStats" => handle_get_dex_core_stats(&state).await,
        "getDexAmmStats" => handle_get_dex_amm_stats(&state).await,
        "getDexMarginStats" => handle_get_dex_margin_stats(&state).await,
        "getDexRewardsStats" => handle_get_dex_rewards_stats(&state).await,
        "getDexRouterStats" => handle_get_dex_router_stats(&state).await,
        "getDexAnalyticsStats" => handle_get_dex_analytics_stats(&state).await,
        "getDexGovernanceStats" => handle_get_dex_governance_stats(&state).await,
        "getMoltswapStats" => handle_get_moltswap_stats(&state).await,
        "getLobsterLendStats" => handle_get_lobsterlend_stats(&state).await,
        "getClawPayStats" => handle_get_clawpay_stats(&state).await,
        "getBountyBoardStats" => handle_get_bountyboard_stats(&state).await,
        "getComputeMarketStats" => handle_get_compute_market_stats(&state).await,
        "getReefStorageStats" => handle_get_reef_storage_stats(&state).await,
        "getMoltMarketStats" => handle_get_moltmarket_stats(&state).await,
        "getMoltAuctionStats" => handle_get_moltauction_stats(&state).await,
        "getMoltPunksStats" => handle_get_moltpunks_stats(&state).await,
        // Token contract stats
        "getMusdStats" => handle_get_musd_stats(&state).await,
        "getWethStats" => handle_get_weth_stats(&state).await,
        "getWsolStats" => handle_get_wsol_stats(&state).await,
        // Platform contract stats — previously missing RPC wiring
        "getClawVaultStats" => handle_get_clawvault_stats(&state).await,
        "getMoltBridgeStats" => handle_get_moltbridge_stats(&state).await,
        "getMoltDaoStats" => handle_get_moltdao_stats(&state).await,
        "getMoltOracleStats" => handle_get_moltoracle_stats(&state).await,

        // ── Shielded Pool (ZK Privacy) ──────────────────────────────
        "getShieldedPoolState" => shielded::handle_get_shielded_pool_state(&state, req.params).await,
        "getShieldedMerkleRoot" => shielded::handle_get_shielded_merkle_root(&state, req.params).await,
        "getShieldedMerklePath" => shielded::handle_get_shielded_merkle_path(&state, req.params).await,
        "isNullifierSpent" => shielded::handle_is_nullifier_spent(&state, req.params).await,
        "getShieldedCommitments" => shielded::handle_get_shielded_commitments(&state, req.params).await,

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
    connect_info: Option<ConnectInfo<SocketAddr>>,
    Json(req): Json<RpcRequest>,
) -> Response {
    // P9-RPC-03: Tiered rate limiting for Solana-compat methods
    let tier = match req.method.as_str() {
        "sendTransaction" => MethodTier::Expensive,
        "getSignaturesForAddress" => MethodTier::Moderate,
        _ => MethodTier::Cheap,
    };
    if tier != MethodTier::Cheap {
        let ip = connect_info
            .map(|ci| ci.0.ip())
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        if !state.rate_limiter.check_tier(ip, tier) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": req.id,
                    "error": {"code": -32005, "message": "Rate limit exceeded"}
                })),
            )
                .into_response();
        }
    }

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
    connect_info: Option<ConnectInfo<SocketAddr>>,
    Json(req): Json<RpcRequest>,
) -> Response {
    // P9-RPC-03: Tiered rate limiting for EVM-compat methods
    let tier = match req.method.as_str() {
        "eth_sendRawTransaction" | "eth_call" | "eth_estimateGas" => MethodTier::Expensive,
        "eth_getLogs" => MethodTier::Moderate,
        _ => MethodTier::Cheap,
    };
    if tier != MethodTier::Cheap {
        let ip = connect_info
            .map(|ci| ci.0.ip())
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        if !state.rate_limiter.check_tier(ip, tier) {
            return (
                StatusCode::TOO_MANY_REQUESTS,
                Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": req.id,
                    "error": {"code": -32005, "message": "Rate limit exceeded"}
                })),
            )
                .into_response();
        }
    }

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
        "eth_gasPrice" => handle_eth_gas_price(&state).await,
        "eth_maxPriorityFeePerGas" => Ok(serde_json::json!("0x0")), // No priority fees in MoltChain
        "eth_estimateGas" => handle_eth_estimate_gas(&state, req.params).await,
        "eth_getCode" => handle_eth_get_code(&state, req.params).await,
        "eth_getTransactionCount" => handle_eth_get_transaction_count(&state, req.params).await,
        "eth_getBlockByNumber" => handle_eth_get_block_by_number(&state, req.params).await,
        "eth_getBlockByHash" => handle_eth_get_block_by_hash(&state, req.params).await,
        "eth_getLogs" => handle_eth_get_logs(&state, req.params).await,
        "eth_getStorageAt" => handle_eth_get_storage_at(&state, req.params).await,
        "net_listening" => Ok(serde_json::json!(true)),
        "web3_clientVersion" => Ok(serde_json::json!(format!("MoltChain/{}", state.version))),
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

/// getEvmRegistration — check if a native address has an EVM address registered on-chain.
/// Params: [nativePubkey (base58)]
/// Returns: { "evmAddress": "0x..." } or null
async fn handle_get_evm_registration(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let native_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [nativePubkey]".to_string(),
        })?;

    let native_pubkey = Pubkey::from_base58(native_str).map_err(|_| RpcError {
        code: -32602,
        message: "Invalid pubkey format".to_string(),
    })?;

    let evm = state
        .state
        .lookup_native_to_evm(&native_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: e,
        })?;

    match evm {
        Some(evm_bytes) => {
            let hex: String = evm_bytes.iter().map(|b| format!("{:02x}", b)).collect();
            Ok(serde_json::json!({ "evmAddress": format!("0x{}", hex) }))
        }
        None => Ok(serde_json::Value::Null),
    }
}

/// lookupEvmAddress — resolve an EVM address to native pubkey.
/// Params: [evmAddress (hex with or without 0x)]
/// Returns: { "nativePubkey": "..." } or null
async fn handle_lookup_evm_address(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let evm_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [evmAddress]".to_string(),
        })?;

    let evm_bytes = StateStore::parse_evm_address(evm_str).map_err(|e| RpcError {
        code: -32602,
        message: e,
    })?;

    let native = state
        .state
        .lookup_evm_address(&evm_bytes)
        .map_err(|e| RpcError {
            code: -32000,
            message: e,
        })?;

    match native {
        Some(pubkey) => Ok(serde_json::json!({ "nativePubkey": pubkey.to_base58() })),
        None => Ok(serde_json::Value::Null),
    }
}

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

            // Include ReefStake liquid staking position
            let (reef_staked, reef_value) = state
                .state
                .get_reefstake_pool()
                .ok()
                .and_then(|pool| {
                    pool.positions.get(&pubkey).map(|p| {
                        (
                            p.molt_deposited,
                            pool.st_molt_token.st_molt_to_molt(p.st_molt_amount),
                        )
                    })
                })
                .unwrap_or((0, 0));

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

                // ReefStake liquid staking (separate from native validator staking)
                "reef_staked": reef_staked,
                "reef_staked_molt": to_molt_str(reef_staked),
                "reef_value": reef_value,
                "reef_value_molt": to_molt_str(reef_value),
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
                "reef_staked": 0,
                "reef_staked_molt": "0.0000",
                "reef_value": 0,
                "reef_value_molt": "0.0000",
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
            let base_reward = if block.header.slot == 0 || block.header.validator == [0u8; 32] {
                0
            } else if has_user_txs {
                TRANSACTION_BLOCK_REWARD
            } else {
                HEARTBEAT_BLOCK_REWARD
            };
            let reward_amount = if base_reward > 0 {
                decayed_reward(base_reward, block.header.slot)
            } else {
                0
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

/// Get current slot (supports optional commitment level)
async fn handle_get_slot(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let commitment = params
        .as_ref()
        .and_then(|p| p.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .or_else(|| {
            params
                .as_ref()
                .and_then(|p| p.as_object())
                .and_then(|o| o.get("commitment"))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("processed");

    let slot = match commitment {
        "finalized" => {
            if let Some(ref ft) = state.finality {
                ft.finalized_slot()
            } else {
                state.state.get_last_finalized_slot().unwrap_or(0)
            }
        }
        "confirmed" => {
            if let Some(ref ft) = state.finality {
                ft.confirmed_slot()
            } else {
                state.state.get_last_confirmed_slot().unwrap_or(0)
            }
        }
        _ => state.state.get_last_slot().map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?,
    };

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
        "fee_burn_percent": config.fee_burn_percent,
        "fee_producer_percent": config.fee_producer_percent,
        "fee_voters_percent": config.fee_voters_percent,
        "fee_treasury_percent": config.fee_treasury_percent,
        "fee_community_percent": config.fee_community_percent,
    }))
}

/// AUDIT-FIX 3.24: fee_burn_percent = 0 is valid per governance design.
/// The sum constraint (burn + producer + voters + treasury == 100) ensures
/// fees still flow somewhere. A minimum burn floor is a governance decision,
/// not a technical invariant.
async fn handle_set_fee_config(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    // L3-01: Block in multi-validator mode — direct state write bypasses consensus
    require_single_validator(state, "setFeeConfig")?;
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
    if let Some(value) = obj.get("fee_community_percent").and_then(|v| v.as_u64()) {
        if value <= 100 {
            config.fee_community_percent = value;
        }
    }

    // Validate that fee distribution percentages sum to 100
    let pct_sum = config.fee_burn_percent
        + config.fee_producer_percent
        + config.fee_voters_percent
        + config.fee_treasury_percent
        + config.fee_community_percent;
    if pct_sum != 100 {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Fee percentages must sum to 100, got {} (burn={}, producer={}, voters={}, treasury={}, community={})",
                pct_sum, config.fee_burn_percent, config.fee_producer_percent,
                config.fee_voters_percent, config.fee_treasury_percent,
                config.fee_community_percent,
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
    // L3-01: Block in multi-validator mode — direct state write bypasses consensus
    require_single_validator(state, "setRentParams")?;
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
        Some(tx) => {
            let mut json = tx_to_rpc_json(&tx, slot, timestamp, &fee_config);
            // Add commitment status to transaction response
            let (status, confirmations) = tx_commitment_status(state, slot);
            if let Some(obj) = json.as_object_mut() {
                obj.insert("confirmationStatus".to_string(), serde_json::json!(status));
                obj.insert("confirmations".to_string(), confirmations);
            }
            Ok(json)
        }
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

// ═══════════════════════════════════════════════════════════════════════════════
// COMMITMENT-LEVEL HELPERS
// ═══════════════════════════════════════════════════════════════════════════════

/// Helper: extract commitment string from params (supports both array and object forms)
fn parse_commitment(params: &Option<serde_json::Value>) -> &str {
    params
        .as_ref()
        .and_then(|p| {
            // Array form: ["sig", "confirmed"]  or  ["sig", {"commitment": "confirmed"}]
            p.as_array()
                .and_then(|arr| {
                    arr.get(1).and_then(|v| {
                        v.as_str().or_else(|| {
                            v.as_object()
                                .and_then(|o| o.get("commitment"))
                                .and_then(|c| c.as_str())
                        })
                    })
                })
                // Object form: { "signature": "...", "commitment": "confirmed" }
                .or_else(|| {
                    p.as_object()
                        .and_then(|o| o.get("commitment"))
                        .and_then(|v| v.as_str())
                })
        })
        .unwrap_or("processed")
}

/// Determine the commitment status of a transaction given its slot.
/// Returns (confirmationStatus, confirmations_or_null).
fn tx_commitment_status(state: &RpcState, tx_slot: u64) -> (&'static str, serde_json::Value) {
    if let Some(ref ft) = state.finality {
        let status = ft.commitment_for_slot(tx_slot);
        let confirmations = match status {
            "finalized" => serde_json::Value::Null, // Solana returns null for finalized
            _ => {
                let confirmed = ft.confirmed_slot();
                if tx_slot <= confirmed {
                    serde_json::Value::Null
                } else {
                    // Approximate confirmations as processed_slot - tx_slot
                    let processed = state.state.get_last_slot().unwrap_or(0);
                    serde_json::json!(processed.saturating_sub(tx_slot))
                }
            }
        };
        (status, confirmations)
    } else {
        // No finality tracker — fall back to "processed" for all
        ("processed", serde_json::json!(0))
    }
}

/// confirmTransaction — check transaction confirmation status
///
/// Params: ["signature"] or ["signature", "commitment"] or ["signature", {"commitment": "..."}]
///
/// Returns:
/// ```json
/// {
///   "value": {
///     "confirmationStatus": "processed"|"confirmed"|"finalized",
///     "slot": 42,
///     "confirmations": 5 | null,
///     "err": null
///   }
/// }
/// ```
///
/// If the transaction is not found, returns `{"value": null}`.
/// If a commitment level is specified, returns null unless the tx has reached
/// at least that commitment level.
async fn handle_confirm_transaction(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params_ref = &params;
    let sig_str = params_ref
        .as_ref()
        .and_then(|p| p.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .or_else(|| {
            params_ref
                .as_ref()
                .and_then(|p| p.as_object())
                .and_then(|o| o.get("signature"))
                .and_then(|v| v.as_str())
        })
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [signature] or {\"signature\": \"...\"}".to_string(),
        })?;

    let sig_hash = moltchain_core::Hash::from_hex(sig_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid signature: {}", e),
    })?;

    let desired_commitment = parse_commitment(params_ref);

    // Look up which slot the tx was included in
    let tx_slot = match state.state.get_tx_slot(&sig_hash) {
        Ok(Some(slot)) => slot,
        Ok(None) => {
            // TX not yet included in any block
            return Ok(serde_json::json!({"value": null}));
        }
        Err(e) => {
            return Err(RpcError {
                code: -32000,
                message: format!("Database error: {}", e),
            });
        }
    };

    let (status, confirmations) = tx_commitment_status(state, tx_slot);

    // Check if the tx has reached the desired commitment level
    let commitment_rank = |c: &str| -> u8 {
        match c {
            "finalized" => 3,
            "confirmed" => 2,
            _ => 1, // processed
        }
    };

    if commitment_rank(status) < commitment_rank(desired_commitment) {
        // TX exists but hasn't reached the desired commitment level
        return Ok(serde_json::json!({"value": null}));
    }

    Ok(serde_json::json!({
        "value": {
            "confirmationStatus": status,
            "slot": tx_slot,
            "confirmations": confirmations,
            "err": null,
        }
    }))
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

    bounded_bincode_deserialize(&tx_bytes)
        .or_else(|_| parse_json_transaction(&tx_bytes))
        .map_err(|e: RpcError| e)
}

/// Parse a JSON-format transaction from the wallet into a native Transaction.
/// Wallet sends: {signatures: [[byte,...]], message: {instructions: [...], blockhash: "hex"}}
fn parse_json_transaction(tx_bytes: &[u8]) -> Result<Transaction, RpcError> {
    let json_val: serde_json::Value = serde_json::from_slice(tx_bytes).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid JSON transaction: {}", e),
    })?;

    // Parse signatures: wallet sends array-of-arrays of numbers
    let sigs_raw = json_val
        .get("signatures")
        .and_then(|s| s.as_array())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing signatures array".into(),
        })?;
    let mut signatures: Vec<[u8; 64]> = Vec::new();
    for sig_val in sigs_raw {
        if let Some(sig_arr) = sig_val.as_array() {
            let bytes: Vec<u8> = sig_arr
                .iter()
                .filter_map(|b| b.as_u64().map(|n| n as u8))
                .collect();
            if bytes.len() != 64 {
                return Err(RpcError {
                    code: -32602,
                    message: format!("Signature must be 64 bytes, got {}", bytes.len()),
                });
            }
            let mut sig = [0u8; 64];
            sig.copy_from_slice(&bytes);
            signatures.push(sig);
        } else if let Some(sig_hex) = sig_val.as_str() {
            let bytes = hex::decode(sig_hex).map_err(|e| RpcError {
                code: -32602,
                message: format!("Invalid signature hex: {}", e),
            })?;
            if bytes.len() != 64 {
                return Err(RpcError {
                    code: -32602,
                    message: format!("Signature must be 64 bytes, got {}", bytes.len()),
                });
            }
            let mut sig = [0u8; 64];
            sig.copy_from_slice(&bytes);
            signatures.push(sig);
        }
    }

    let msg_val = json_val.get("message").ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing message".into(),
    })?;

    // Blockhash — wallet sends "blockhash" (hex string), Rust expects "recent_blockhash"
    let blockhash_str = msg_val
        .get("blockhash")
        .or_else(|| msg_val.get("recent_blockhash"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing blockhash".into(),
        })?;
    let recent_blockhash = moltchain_core::Hash::from_hex(blockhash_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid blockhash: {}", e),
    })?;

    // Instructions
    let ixs_raw = msg_val
        .get("instructions")
        .and_then(|i| i.as_array())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing instructions array".into(),
        })?;
    let mut instructions = Vec::new();
    for ix_val in ixs_raw {
        let program_id = if let Some(arr) = ix_val.get("program_id").and_then(|p| p.as_array()) {
            let bytes: Vec<u8> = arr
                .iter()
                .filter_map(|b| b.as_u64().map(|n| n as u8))
                .collect();
            if bytes.len() != 32 {
                return Err(RpcError {
                    code: -32602,
                    message: format!("program_id must be 32 bytes, got {}", bytes.len()),
                });
            }
            let mut pk = [0u8; 32];
            pk.copy_from_slice(&bytes);
            Pubkey(pk)
        } else if let Some(s) = ix_val.get("program_id").and_then(|p| p.as_str()) {
            Pubkey::from_base58(s).map_err(|e| RpcError {
                code: -32602,
                message: format!("Invalid program_id: {}", e),
            })?
        } else {
            return Err(RpcError {
                code: -32602,
                message: "Invalid program_id format".into(),
            });
        };

        let accounts_raw = ix_val
            .get("accounts")
            .and_then(|a| a.as_array())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Missing accounts in instruction".into(),
            })?;
        let mut accounts = Vec::new();
        for acct in accounts_raw {
            if let Some(arr) = acct.as_array() {
                let bytes: Vec<u8> = arr
                    .iter()
                    .filter_map(|b| b.as_u64().map(|n| n as u8))
                    .collect();
                if bytes.len() != 32 {
                    return Err(RpcError {
                        code: -32602,
                        message: format!("Account pubkey must be 32 bytes, got {}", bytes.len()),
                    });
                }
                let mut pk = [0u8; 32];
                pk.copy_from_slice(&bytes);
                accounts.push(Pubkey(pk));
            } else if let Some(s) = acct.as_str() {
                accounts.push(Pubkey::from_base58(s).map_err(|e| RpcError {
                    code: -32602,
                    message: format!("Invalid account: {}", e),
                })?);
            }
        }

        let data: Vec<u8> = ix_val
            .get("data")
            .and_then(|d| d.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|b| b.as_u64().map(|n| n as u8))
                    .collect()
            })
            .unwrap_or_default();

        instructions.push(moltchain_core::Instruction {
            program_id,
            accounts,
            data,
        });
    }

    Ok(Transaction {
        signatures,
        message: moltchain_core::Message {
            instructions,
            recent_blockhash,
        },
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

    // Support optional second param: { skipPreflight: true }
    let skip_preflight = params
        .as_array()
        .and_then(|arr| arr.get(1))
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("skipPreflight"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

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

    // Deserialize transaction — try bincode first, then JSON (wallet sends JSON)
    let tx: Transaction =
        bounded_bincode_deserialize(&tx_bytes).or_else(|_| parse_json_transaction(&tx_bytes))?;

    // ── Pre-mempool validation ──────────────────────────────────
    // Reject structurally invalid transactions BEFORE entering mempool.

    // P9-RPC-01: Reject EVM sentinel blockhash via sendTransaction.
    // Only eth_sendRawTransaction may create TXs with the sentinel — external
    // callers must never be allowed to submit sentinel-tagged TXs directly.
    if tx.message.recent_blockhash == moltchain_core::Hash([0xEE; 32]) {
        return Err(RpcError {
            code: -32003,
            message: "EVM sentinel blockhash is not allowed via sendTransaction".to_string(),
        });
    }

    // 1. Reject transactions with empty signatures
    if tx.signatures.is_empty() {
        return Err(RpcError {
            code: -32003,
            message: "Transaction has no signatures".to_string(),
        });
    }
    // 2. Reject zero signatures (all bytes 0x00)
    for sig in &tx.signatures {
        if sig.iter().all(|&b| b == 0) {
            return Err(RpcError {
                code: -32003,
                message: "Transaction contains an invalid zero signature".to_string(),
            });
        }
    }
    // 3. Reject transactions with no instructions
    if tx.message.instructions.is_empty() {
        return Err(RpcError {
            code: -32003,
            message: "Transaction has no instructions".to_string(),
        });
    }

    // 4. Pre-mempool balance + fee check: reject if payer can't afford fees
    //    This prevents silent failures during block production.
    {
        let fee_payer = tx
            .message
            .instructions
            .first()
            .and_then(|ix| ix.accounts.first().cloned());
        if let Some(payer) = fee_payer {
            // Compute expected fee
            let fee_config = state
                .state
                .get_fee_config()
                .unwrap_or_else(|_| moltchain_core::FeeConfig::default_from_constants());
            let expected_fee = TxProcessor::compute_transaction_fee(&tx, &fee_config);

            if expected_fee > 0 {
                // Check payer's spendable balance
                match state.state.get_account(&payer) {
                    Ok(Some(acct)) => {
                        if acct.spendable < expected_fee {
                            return Err(RpcError {
                                code: -32003,
                                message: format!(
                                    "Insufficient MOLT balance for fees: need {} shells ({:.6} MOLT), have {} shells ({:.6} MOLT)",
                                    expected_fee, expected_fee as f64 / 1_000_000_000.0,
                                    acct.spendable, acct.spendable as f64 / 1_000_000_000.0
                                ),
                            });
                        }
                    }
                    Ok(None) => {
                        return Err(RpcError {
                            code: -32003,
                            message: "Payer account does not exist on-chain. Fund it first."
                                .to_string(),
                        });
                    }
                    Err(_) => {} // DB error — let block producer handle it
                }

                // Also check if the TX transfers value — verify total needed
                // (fee + transfer amount) is covered
                if let Some(first_ix) = tx.message.instructions.first() {
                    if first_ix.program_id == moltchain_core::SYSTEM_PROGRAM_ID {
                        if let Some(&kind) = first_ix.data.first() {
                            if kind == 0 || kind == 1 {
                                // Transfer or TransferWithMemo
                                if first_ix.data.len() >= 9 {
                                    let transfer_amount = u64::from_le_bytes(
                                        first_ix.data[1..9].try_into().unwrap_or([0u8; 8]),
                                    );
                                    if let Ok(Some(acct)) = state.state.get_account(&payer) {
                                        if acct.spendable
                                            < expected_fee.saturating_add(transfer_amount)
                                        {
                                            return Err(RpcError {
                                                code: -32003,
                                                message: format!(
                                                    "Insufficient MOLT for transfer + fees: need {} shells (transfer) + {} shells (fee) = {} total, have {} spendable",
                                                    transfer_amount, expected_fee,
                                                    transfer_amount.saturating_add(expected_fee),
                                                    acct.spendable
                                                ),
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ── Preflight simulation: pre-execute contract calls to catch errors ─
    // Like Solana's preflight check — simulates the TX and rejects if it
    // would fail on-chain. Callers can pass { skipPreflight: true } to bypass.
    // Skip preflight for deploy transactions (data contains JSON with "Deploy" key).
    if !skip_preflight {
        let has_contract_call = tx.message.instructions.iter().any(|ix| {
            ix.program_id == moltchain_core::CONTRACT_PROGRAM_ID
                && !ix.data.starts_with(b"{\"Deploy\"") // skip deploys
        });
        if has_contract_call {
            let processor = TxProcessor::new(state.state.clone());
            let sim = processor.simulate_transaction(&tx);
            if !sim.success {
                let reason = sim
                    .error
                    .unwrap_or_else(|| "Contract execution failed".to_string());
                return Err(RpcError {
                    code: -32002,
                    message: format!("Transaction simulation failed: {}", reason),
                });
            }
        }
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
    let tx: Transaction = bounded_bincode_deserialize(&tx_bytes).map_err(|e| RpcError {
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
        "returnCode": result.return_code,
        "stateChanges": result.state_changes,
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
    // Solana getSlot accepts optional [config] with commitment
    // For simplicity, return processed slot (chain tip) — matches Solana default
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
        let mut tx_slot = last_slot;
        if state.solana_tx_cache.lock().await.contains(&sig_hash) {
            found = true;
            // Try to get actual slot
            if let Ok(Some(slot)) = state.state.get_tx_slot(&sig_hash) {
                tx_slot = slot;
            }
        } else if let Ok(Some(_)) = state.state.get_transaction(&sig_hash) {
            found = true;
            if let Ok(Some(slot)) = state.state.get_tx_slot(&sig_hash) {
                tx_slot = slot;
            }
        }

        if found {
            let (status, confirmations) = tx_commitment_status(state, tx_slot);
            values.push(serde_json::json!({
                "slot": tx_slot,
                "confirmations": confirmations,
                "err": serde_json::Value::Null,
                "confirmationStatus": status,
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
                        solana_block_transaction_encoded_json(&state.state, tx, 0, encoding)
                    } else {
                        solana_block_transaction_json(&state.state, tx, 0)
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
                &state.state,
                &record.tx,
                record.slot,
                record.timestamp,
                record.fee,
                encoding,
            ));
        }
        return Ok(solana_transaction_json(
            &state.state,
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
            // AUDIT-FIX 0.13: Cap fallback scan to 1000 slots to prevent DoS
            if let Ok(last_slot) = state.state.get_last_slot() {
                let mut found = None;
                let scan_limit = 1000u64;
                let end_slot = last_slot.saturating_sub(scan_limit);
                for slot in (end_slot..=last_slot).rev() {
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
            &state.state,
            &tx,
            slot,
            timestamp,
            0,
            encoding,
        ))
    } else {
        Ok(solana_transaction_json(
            &state.state,
            &tx,
            slot,
            timestamp,
            0,
        ))
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

    // P9-RPC-01: Reject EVM sentinel blockhash via Solana sendTransaction
    if tx.message.recent_blockhash == moltchain_core::Hash([0xEE; 32]) {
        return Err(RpcError {
            code: -32003,
            message: "EVM sentinel blockhash is not allowed via sendTransaction".to_string(),
        });
    }

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
    let validators = cached_validators(state).await?;

    // Pre-compute total reputation once (was O(n²) inside the map loop)
    let total_reputation: u64 = validators.iter().map(|val| val.reputation).sum();

    let validator_list: Vec<_> = validators
        .iter()
        .map(|v| {
            // Get stake + bootstrap info from StakePool (authoritative source)
            let (pool_stake, bootstrap_debt, vesting_status, earned_amount, graduation_slot) =
                if let Some(ref pool_arc) = state.stake_pool {
                    if let Ok(pool) = pool_arc.try_read() {
                        if let Some(s) = pool.get_stake(&v.pubkey) {
                            (
                                s.amount,
                                s.bootstrap_debt,
                                format!("{:?}", s.status),
                                s.earned_amount,
                                s.graduation_slot,
                            )
                        } else {
                            (0, 0, "Unknown".to_string(), 0, None)
                        }
                    } else {
                        (0, 0, "Unknown".to_string(), 0, None)
                    }
                } else {
                    (0, 0, "Unknown".to_string(), 0, None)
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
                "bootstrap_debt": bootstrap_debt,
                "vesting_status": vesting_status,
                "earned_amount": earned_amount,
                "graduation_slot": graduation_slot,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "validators": validator_list,
        "count": validators.len(),
        "_count": validators.len(),
    }))
}

/// Handle getMetrics — with per-slot response caching to avoid
/// redundant full-scan computation on hot polling paths.
const METRICS_CACHE_TTL_MS: u128 = 400;

async fn handle_get_metrics(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    // Fast path: return cached response if still fresh
    {
        let guard = state.metrics_cache.read().await;
        if guard.0.elapsed().as_millis() < METRICS_CACHE_TTL_MS {
            if let Some(ref cached) = guard.1 {
                return Ok(cached.clone());
            }
        }
    }

    // Slow path: compute then cache
    let result = compute_metrics(state).await?;

    {
        let mut guard = state.metrics_cache.write().await;
        *guard = (Instant::now(), Some(result.clone()));
    }
    Ok(result)
}

/// Inner metrics computation (expensive — calls multiple DB reads)
async fn compute_metrics(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let metrics = state.state.get_metrics();
    let validators = cached_validators(state).await?;
    let total_staked: u64 = if let Some(ref pool_arc) = state.stake_pool {
        if let Ok(pool) = pool_arc.try_read() {
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

    // Distribution wallet balances
    let dist_wallets_json = {
        let ga = state.state.get_genesis_accounts().unwrap_or_default();
        let mut dw_map = serde_json::Map::new();
        for (role, pubkey, _amount_molt, _pct) in &ga {
            let bal = state
                .state
                .get_account(pubkey)
                .ok()
                .flatten()
                .map(|a| a.shells)
                .unwrap_or(0);
            dw_map.insert(format!("{}_balance", role), serde_json::json!(bal));
            dw_map.insert(
                format!("{}_pubkey", role),
                serde_json::json!(pubkey.to_base58()),
            );
        }
        serde_json::Value::Object(dw_map)
    };

    // Fee config for burn percentage display
    let fee_config = state.state.get_fee_config()
        .unwrap_or_else(|_| moltchain_core::FeeConfig::default_from_constants());
    let slot_duration_ms = state.state.get_slot_duration_ms();

    Ok(serde_json::json!({
        "tps": metrics.tps,
        "peak_tps": metrics.peak_tps,
        "total_transactions": metrics.total_transactions,
        "daily_transactions": metrics.daily_transactions,
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
        "distribution_wallets": dist_wallets_json,
        "slot_duration_ms": slot_duration_ms,
        "fee_burn_percent": fee_config.fee_burn_percent,
    }))
}

// ============================================================================
// GENESIS ACCOUNTS ENDPOINT
// ============================================================================

/// Get all genesis distribution accounts with live balances
async fn handle_get_genesis_accounts(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let accounts = state.state.get_genesis_accounts().map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    let mut result = Vec::new();

    // Add genesis wallet itself
    if let Ok(Some(gpk)) = state.state.get_genesis_pubkey() {
        let acc = state.state.get_account(&gpk).ok().flatten();
        let bal = acc.as_ref().map(|a| a.shells).unwrap_or(0);
        result.push(serde_json::json!({
            "role": "genesis",
            "pubkey": gpk.to_base58(),
            "amount_molt": 1_000_000_000u64,
            "percentage": 100,
            "balance": bal,
            "label": "Genesis Signer",
        }));
    }

    // Add all distribution wallets
    for (role, pubkey, amount_molt, percentage) in &accounts {
        let acc = state.state.get_account(pubkey).ok().flatten();
        let bal = acc.as_ref().map(|a| a.shells).unwrap_or(0);
        let label = match role.as_str() {
            "validator_rewards" => "Validator Treasury",
            "community_treasury" => "Community Treasury",
            "builder_grants" => "Builder Grants",
            "founding_moltys" => "Founding Moltys",
            "ecosystem_partnerships" => "Ecosystem Partnerships",
            "reserve_pool" => "Reserve Pool",
            _ => role.as_str(),
        };
        result.push(serde_json::json!({
            "role": role,
            "pubkey": pubkey.to_base58(),
            "amount_molt": amount_molt,
            "percentage": percentage,
            "balance": bal,
            "label": label,
        }));
    }

    Ok(serde_json::json!({ "accounts": result }))
}

/// Get a governed wallet proposal by ID.
///
/// Params: [proposal_id] (integer)
async fn handle_get_governed_proposal(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params: expected [proposal_id]".to_string(),
    })?;

    let proposal_id = params
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_u64())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: proposal_id must be a positive integer".to_string(),
        })?;

    let proposal = state
        .state
        .get_governed_proposal(proposal_id)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .ok_or_else(|| RpcError {
            code: -32001,
            message: format!("Proposal {} not found", proposal_id),
        })?;

    Ok(serde_json::json!({
        "id": proposal.id,
        "source": proposal.source.to_base58(),
        "recipient": proposal.recipient.to_base58(),
        "amount": proposal.amount,
        "amount_molt": proposal.amount / 1_000_000_000,
        "approvals": proposal.approvals.iter().map(|p| p.to_base58()).collect::<Vec<_>>(),
        "threshold": proposal.threshold,
        "executed": proposal.executed,
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

/// Get live cluster info: connected peers with their validator identity & slot
/// This is the PRODUCTION endpoint for monitoring — no hardcoded ports needed.
async fn handle_get_cluster_info(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let current_slot = state.state.get_last_slot().unwrap_or(0);

    // Get connected P2P peers
    let connected_peers: Vec<String> = if let Some(ref p2p) = state.p2p {
        p2p.peer_addresses()
    } else {
        Vec::new()
    };

    // Get all known validators from cache (refreshed per-slot)
    let validators = cached_validators(state).await?;

    // Build per-validator node info
    let nodes: Vec<serde_json::Value> = validators
        .iter()
        .map(|v| {
            let pool_stake = if let Some(ref pool_arc) = state.stake_pool {
                if let Ok(pool) = pool_arc.try_read() {
                    pool.get_stake(&v.pubkey).map(|s| s.amount).unwrap_or(0)
                } else {
                    0
                }
            } else {
                0
            };
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

            // A validator is "active" if it produced a block within the last 100 slots
            let is_active = v.last_active_slot + 100 >= current_slot || v.last_active_slot == 0;

            serde_json::json!({
                "pubkey": v.pubkey.to_base58(),
                "stake": actual_stake,
                "reputation": v.reputation as f64,
                "blocks_proposed": v.blocks_proposed,
                "last_active_slot": v.last_active_slot,
                "joined_slot": v.joined_slot,
                "active": is_active,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "current_slot": current_slot,
        "cluster_nodes": nodes,
        "connected_peers": connected_peers,
        "validator_count": validators.len(),
        "peer_count": connected_peers.len(),
    }))
}

/// Get network information
async fn handle_get_network_info(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let current_slot = state.state.get_last_slot().unwrap_or(0);
    let validators = cached_validators(state).await?;

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

    // F10: Compute is_active from last_active_slot vs current slot (active = within 1000 slots)
    let current_slot = state.state.get_last_slot().unwrap_or(0);
    let is_active = current_slot.saturating_sub(validator.last_active_slot) < 1000;

    Ok(serde_json::json!({
        "pubkey": validator.pubkey.to_base58(),
        "stake": validator.stake,
        "reputation": validator.reputation,
        "blocks_proposed": validator.blocks_proposed,
        "votes_cast": validator.votes_cast,
        "correct_votes": validator.correct_votes,
        "last_active_slot": validator.last_active_slot,
        "joined_slot": validator.joined_slot,
        "commission_rate": validator.commission_rate,
        "is_active": is_active,
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
    let validators = cached_validators(state).await?;

    let total_stake: u64 = if let Some(ref pool_arc) = state.stake_pool {
        if let Ok(pool) = pool_arc.try_read() {
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
        "peak_tps": metrics.peak_tps,
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

        let tx: Transaction = bounded_bincode_deserialize(&tx_bytes).map_err(|e| RpcError {
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

        let tx: Transaction = bounded_bincode_deserialize(&tx_bytes).map_err(|e| RpcError {
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
        let (
            live_stake,
            bootstrap_debt,
            bootstrap_index,
            earned_amount,
            total_debt_repaid,
            vesting_status,
            start_slot,
            graduation_slot,
        ) = if let Some(ref pool_arc) = state.stake_pool {
            if let Ok(pool) = pool_arc.try_read() {
                if let Some(s) = pool.get_stake(&pubkey) {
                    (
                        s.amount,
                        s.bootstrap_debt,
                        s.bootstrap_index,
                        s.earned_amount,
                        s.total_debt_repaid,
                        format!("{:?}", s.status),
                        s.start_slot,
                        s.graduation_slot,
                    )
                } else {
                    (
                        validator.stake,
                        0,
                        u64::MAX,
                        0,
                        0,
                        "Unknown".to_string(),
                        0,
                        None,
                    )
                }
            } else {
                (
                    validator.stake,
                    0,
                    u64::MAX,
                    0,
                    0,
                    "Unknown".to_string(),
                    0,
                    None,
                )
            }
        } else {
            (
                validator.stake,
                0,
                u64::MAX,
                0,
                0,
                "Unknown".to_string(),
                0,
                None,
            )
        };
        Ok(serde_json::json!({
            "is_validator": true,
            "total_staked": live_stake,
            "delegations": [],
            "status": "active",
            "bootstrap_debt": bootstrap_debt,
            "bootstrap_index": bootstrap_index,
            "earned_amount": earned_amount,
            "total_debt_repaid": total_debt_repaid,
            "vesting_status": vesting_status,
            "start_slot": start_slot,
            "graduation_slot": graduation_slot,
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
        let pool_guard = pool.read().await;
        if let Some(stake_info) = pool_guard.get_stake(&pubkey) {
            // total_claimed tracks all historically claimed rewards (liquid + debt)
            // rewards_earned is the currently pending (unclaimed) buffer
            let total_earned = stake_info.total_claimed + stake_info.rewards_earned;
            let pending = stake_info.rewards_earned;
            let claimed = stake_info.total_claimed;

            // Reward rate: MOLT per block for this validator (with decay)
            let current_slot = state.state.get_last_slot().unwrap_or(0);
            let decayed_base = decayed_reward(TRANSACTION_BLOCK_REWARD, current_slot);
            let base_rate_molt = decayed_base as f64 / 1_000_000_000.0;
            let reward_rate = if stake_info.is_active {
                if stake_info.bootstrap_debt > 0 {
                    // During vesting: 50% goes to debt repayment, 50% liquid
                    format!("{:.4}", base_rate_molt / 2.0)
                } else {
                    format!("{:.4}", base_rate_molt)
                }
            } else {
                "0".to_string()
            };

            let vesting_progress = {
                let total = (stake_info.earned_amount as f64) + (stake_info.bootstrap_debt as f64);
                if total == 0.0 {
                    1.0
                } else {
                    stake_info.earned_amount as f64 / total
                }
            };

            return Ok(serde_json::json!({
                "total_rewards": total_earned,
                "pending_rewards": pending,
                "claimed_rewards": claimed,
                "reward_rate": reward_rate,
                "bootstrap_debt": stake_info.bootstrap_debt,
                "earned_amount": stake_info.earned_amount,
                "vesting_progress": vesting_progress,
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

    // Extract version + previous_code_hash when available
    let (contract_version, prev_code_hash) = if account.executable {
        if let Ok(ca) = serde_json::from_slice::<moltchain_core::ContractAccount>(&account.data) {
            (ca.version, ca.previous_code_hash.map(|h| h.to_hex()))
        } else {
            (1u32, None)
        }
    } else {
        (1u32, None)
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
        "version": contract_version,
    });
    if let Some(pch) = prev_code_hash {
        result
            .as_object_mut()
            .unwrap()
            .insert("previous_code_hash".to_string(), serde_json::json!(pch));
    }
    if let Some(tm) = token_metadata {
        result
            .as_object_mut()
            .unwrap()
            .insert("token_metadata".to_string(), tm);
    }

    // Enrich with registry metadata fallback (for supply, decimals, etc.)
    if let Ok(Some(reg)) = state.state.get_symbol_registry_by_program(&contract_id) {
        if let Some(reg_meta) = &reg.metadata {
            let rm = result.as_object_mut().unwrap();
            // If token_metadata doesn't exist yet, create it from registry
            if !rm.contains_key("token_metadata") {
                let mut tmeta = serde_json::Map::new();
                if let Some(v) = reg_meta.get("total_supply") {
                    tmeta.insert("total_supply".to_string(), v.clone());
                }
                if let Some(v) = reg_meta.get("decimals") {
                    tmeta.insert("decimals".to_string(), v.clone());
                }
                if let Some(v) = reg_meta.get("mintable") {
                    tmeta.insert("mintable".to_string(), v.clone());
                }
                if let Some(v) = reg_meta.get("burnable") {
                    tmeta.insert("burnable".to_string(), v.clone());
                }
                if !tmeta.is_empty() {
                    rm.insert(
                        "token_metadata".to_string(),
                        serde_json::Value::Object(tmeta),
                    );
                }
            } else if let Some(tm) = rm.get_mut("token_metadata") {
                // Merge missing fields from registry into existing token_metadata
                if let Some(tm_obj) = tm.as_object_mut() {
                    for key in &["total_supply", "decimals", "mintable", "burnable"] {
                        if !tm_obj.contains_key(*key) {
                            if let Some(v) = reg_meta.get(*key) {
                                tm_obj.insert(key.to_string(), v.clone());
                            }
                        }
                    }
                }
            }
            // Add is_native flag if present
            if reg_meta.get("is_native").and_then(|v| v.as_bool()) == Some(true) {
                rm.insert("is_native".to_string(), serde_json::json!(true));
            }
        }
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

    let limit = arr.get(1).and_then(|v| v.as_u64()).unwrap_or(100).min(1000) as usize; // AUDIT-FIX 2.16

    let before_slot = arr.get(2).and_then(|v| v.as_u64());

    let contract_id = Pubkey::from_base58(contract_id_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid contract ID: {}", e),
    })?;

    let events = state
        .state
        .get_contract_logs(&contract_id, limit, before_slot)
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
            // Enrich with symbol/name from the registry when available
            let (symbol, name, owner) =
                if let Ok(Some(entry)) = state.state.get_symbol_registry_by_program(pk) {
                    (
                        Some(entry.symbol.clone()),
                        Some(entry.name.clone()),
                        Some(entry.owner.to_base58()),
                    )
                } else {
                    (None, None, None)
                };
            serde_json::json!({
                "program_id": pk.to_base58(),
                "symbol": symbol,
                "name": name,
                "owner": owner,
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

    // AUDIT-FIX F-1: Use StateBatch for atomic all-or-nothing commit of
    // deployer debit + treasury credit + contract account creation.
    let mut batch = state.state.begin_batch();

    // Debit deployer using deduct_spendable to maintain shells == spendable + staked + locked
    let mut updated_deployer = deployer_account.clone();
    updated_deployer
        .deduct_spendable(deploy_fee)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to deduct deploy fee: {}", e),
        })?;
    batch
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
    batch
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
    batch
        .put_account(&program_pubkey, &account)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to store contract: {}", e),
        })?;

    // Commit all three writes atomically
    state.state.commit_batch(batch).map_err(|e| RpcError {
        code: -32000,
        message: format!("Atomic deploy commit failed: {}", e),
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

/// Upgrade an existing smart contract (owner-only, charges upgrade fee).
/// Params: [owner_base58, contract_base58, code_base64, signature_hex]
async fn handle_upgrade_contract(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    use base64::{engine::general_purpose, Engine as _};
    use moltchain_core::account::Keypair as MoltKeypair;
    use sha2::{Digest, Sha256};

    require_single_validator(state, "upgradeContract")?;
    verify_admin_auth(state, &params)?;

    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Params must be an array: [owner, contract, code_base64, signature]".to_string(),
    })?;

    if arr.len() < 4 {
        return Err(RpcError {
            code: -32602,
            message: "Expected [owner_base58, contract_base58, code_base64, signature_hex]"
                .to_string(),
        });
    }

    // Parse owner pubkey
    let owner_str = arr[0].as_str().ok_or_else(|| RpcError {
        code: -32602,
        message: "owner must be a base58 string".to_string(),
    })?;
    let owner_pubkey = Pubkey::from_base58(owner_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid owner pubkey: {}", e),
    })?;

    // Parse contract address
    let contract_str = arr[1].as_str().ok_or_else(|| RpcError {
        code: -32602,
        message: "contract must be a base58 string".to_string(),
    })?;
    let contract_pubkey = Pubkey::from_base58(contract_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid contract pubkey: {}", e),
    })?;

    // Parse new WASM code (base64)
    let code_b64 = arr[2].as_str().ok_or_else(|| RpcError {
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

    // P10-RPC-05: Reject oversized WASM code to prevent storage abuse
    const MAX_CONTRACT_CODE_SIZE: usize = 524_288; // 512 KB
    if code_bytes.len() > MAX_CONTRACT_CODE_SIZE {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Contract code too large: {} bytes (max {} bytes / 512 KB)",
                code_bytes.len(),
                MAX_CONTRACT_CODE_SIZE,
            ),
        });
    }

    // Parse signature (hex-encoded)
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

    // Verify signature: owner must sign SHA-256(code_bytes)
    let mut hasher = Sha256::new();
    hasher.update(&code_bytes);
    let code_hash_bytes = hasher.finalize();
    if !MoltKeypair::verify(&owner_pubkey, &code_hash_bytes, &sig_array) {
        return Err(RpcError {
            code: -32003,
            message: "Invalid signature: owner must sign SHA-256(code)".to_string(),
        });
    }

    // Load existing contract
    let contract_account = state
        .state
        .get_account(&contract_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .ok_or_else(|| RpcError {
            code: -32000,
            message: format!("Contract not found at {}", contract_pubkey.to_base58()),
        })?;

    let mut contract: ContractAccount =
        serde_json::from_slice(&contract_account.data).map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to deserialize contract: {}", e),
        })?;

    // Verify caller is the contract owner
    if contract.owner != owner_pubkey {
        return Err(RpcError {
            code: -32003,
            message: "Only the contract owner can upgrade".to_string(),
        });
    }

    // Charge upgrade fee (10 MOLT)
    let upgrade_fee = moltchain_core::CONTRACT_UPGRADE_FEE;
    let owner_account = state
        .state
        .get_account(&owner_pubkey)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?
        .ok_or_else(|| RpcError {
            code: -32000,
            message: "Owner account not found".to_string(),
        })?;

    if owner_account.spendable < upgrade_fee {
        return Err(RpcError {
            code: -32000,
            message: format!(
                "Insufficient spendable balance: need {} shells ({:.1} MOLT), have {} spendable ({:.1} MOLT)",
                upgrade_fee,
                upgrade_fee as f64 / 1_000_000_000.0,
                owner_account.spendable,
                owner_account.spendable as f64 / 1_000_000_000.0,
            ),
        });
    }

    // AUDIT-FIX F-2: Use StateBatch for atomic all-or-nothing commit of
    // owner debit + treasury credit + contract upgrade.
    let mut batch = state.state.begin_batch();

    let mut updated_owner = owner_account.clone();
    updated_owner
        .deduct_spendable(upgrade_fee)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to deduct upgrade fee: {}", e),
        })?;
    batch
        .put_account(&owner_pubkey, &updated_owner)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to update owner balance: {}", e),
        })?;

    // Credit fee to treasury
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
        .add_spendable(upgrade_fee)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Treasury balance overflow: {}", e),
        })?;
    batch
        .put_account(&treasury_pubkey, &treasury_account)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to credit treasury: {}", e),
        })?;

    // Perform the upgrade: bump version, store previous hash, replace code
    let old_version = contract.version;
    contract.previous_code_hash = Some(contract.code_hash);
    contract.version = contract.version.saturating_add(1);

    // Compute new code hash
    let mut code_hasher = Sha256::new();
    code_hasher.update(&code_bytes);
    let new_hash = code_hasher.finalize();
    let mut hash_bytes = [0u8; 32];
    hash_bytes.copy_from_slice(&new_hash[..32]);
    contract.code_hash = moltchain_core::Hash(hash_bytes);
    contract.code = code_bytes;

    // Serialize back
    let mut updated_account = contract_account.clone();
    updated_account.data = serde_json::to_vec(&contract).map_err(|e| RpcError {
        code: -32000,
        message: format!("Failed to serialize upgraded contract: {}", e),
    })?;

    batch
        .put_account(&contract_pubkey, &updated_account)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to store upgraded contract: {}", e),
        })?;

    // Commit all three writes atomically
    state.state.commit_batch(batch).map_err(|e| RpcError {
        code: -32000,
        message: format!("Atomic upgrade commit failed: {}", e),
    })?;

    info!(
        "upgradeContract: {} upgraded {} v{} → v{} (code={} bytes, fee={} shells)",
        owner_pubkey.to_base58(),
        contract_pubkey.to_base58(),
        old_version,
        contract.version,
        updated_account.data.len(),
        upgrade_fee,
    );

    Ok(serde_json::json!({
        "program_id": contract_pubkey.to_base58(),
        "owner": owner_pubkey.to_base58(),
        "version": contract.version,
        "previous_version": old_version,
        "code_size": updated_account.data.len(),
        "upgrade_fee": upgrade_fee,
        "upgrade_fee_molt": upgrade_fee as f64 / 1_000_000_000.0,
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

    let before_slot = arr
        .get(1)
        .and_then(|v| v.get("before_slot"))
        .and_then(|v| v.as_u64());

    let program_pubkey = Pubkey::from_base58(program_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid program pubkey: {}", e),
    })?;

    let calls = state
        .state
        .get_program_calls(&program_pubkey, limit, before_slot)
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

/// AUDIT-FIX 3.25: This endpoint intentionally exposes all contract storage
/// key-value pairs. On-chain state is public by design (similar to eth_getStorageAt).
/// Rate limiting via the global RateLimiter applies to this endpoint.
///
/// Uses CF_CONTRACT_STORAGE prefix iterator for O(limit) instead of loading
/// the entire ContractAccount and deserializing the full in-memory BTreeMap.
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

    let after_key_hex = arr
        .get(1)
        .and_then(|v| v.get("after_key"))
        .and_then(|v| v.as_str());

    let program_pubkey = Pubkey::from_base58(program_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid program pubkey: {}", e),
    })?;

    // Decode hex cursor if provided
    let after_key = if let Some(hex_str) = after_key_hex {
        Some(hex::decode(hex_str).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid after_key hex: {}", e),
        })?)
    } else {
        None
    };

    // Use CF_CONTRACT_STORAGE prefix iterator — O(limit) instead of O(N) deserialization
    let raw_entries = state
        .state
        .get_contract_storage_entries(&program_pubkey, limit, after_key)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let mut entries = Vec::new();
    for (key, value) in raw_entries {
        let key_hex = hex::encode(&key);
        // Try to decode key as UTF-8 for human-readable display
        let key_decoded = String::from_utf8(key.clone()).ok().filter(|s| {
            s.chars().all(|c| {
                c.is_ascii_graphic() || c == ' ' || c == ':' || c == '_' || c == '-' || c == '.'
            })
        });
        let value_hex = hex::encode(&value);
        let size = value.len();
        // Try to decode value as UTF-8 for preview; fallback to hex truncated
        let value_preview = String::from_utf8(value.clone())
            .ok()
            .filter(|s| !s.is_empty() && s.chars().take(16).all(|c| !c.is_control() || c == '\n'))
            .map(|s| {
                if s.len() > 128 {
                    format!("{}...", &s[..128])
                } else {
                    s
                }
            })
            .unwrap_or_else(|| {
                if value_hex.len() > 80 {
                    format!("0x{}...", &value_hex[..80])
                } else if value_hex.is_empty() {
                    "(empty)".to_string()
                } else {
                    format!("0x{}", value_hex)
                }
            });

        let mut entry = serde_json::json!({
            "key": key_hex.clone(),
            "key_hex": key_hex,
            "value": value_hex.clone(),
            "value_hex": value_hex,
            "value_preview": value_preview,
            "size": size,
        });
        if let Some(decoded) = key_decoded {
            entry["key_decoded"] = serde_json::Value::String(decoded);
        }
        entries.push(entry);
    }

    Ok(serde_json::json!({
        "program": program_pubkey.to_base58(),
        "count": entries.len(),
        "entries": entries,
    }))
}

// ============================================================================
// MOLTYID ENDPOINTS
// ============================================================================

const MOLTYID_SYMBOL: &str = "YID";
const MOLTYID_IDENTITY_SIZE: usize = 127;

#[derive(Debug, Clone)]
struct MoltyIdIdentityRecord {
    owner: Pubkey,
    agent_type: u8,
    name: String,
    reputation: u64,
    created_at: u64,
    updated_at: u64,
    skill_count: u8,
    vouch_count: u16,
    is_active: bool,
}

#[derive(Debug, Clone)]
struct MoltyIdSkillRecord {
    name: String,
    proficiency: u8,
    timestamp: u64,
}

#[derive(Debug, Clone)]
struct MoltyIdVouchRecord {
    voucher: Pubkey,
    timestamp: u64,
}

#[derive(Debug, Clone)]
struct MoltyIdAchievementRecord {
    id: u8,
    timestamp: u64,
}

fn moltyid_agent_type_name(agent_type: u8) -> &'static str {
    match agent_type {
        0 => "System",
        1 => "Trading",
        2 => "Development",
        3 => "Analysis",
        4 => "Creative",
        5 => "Infrastructure",
        6 => "Governance",
        7 => "Oracle",
        8 => "Storage",
        9 => "General",
        _ => "Unknown",
    }
}

fn moltyid_trust_tier(score: u64) -> u8 {
    if score >= 10_000 {
        5
    } else if score >= 5_000 {
        4
    } else if score >= 1_000 {
        3
    } else if score >= 500 {
        2
    } else if score >= 100 {
        1
    } else {
        0
    }
}

fn moltyid_trust_tier_name(tier: u8) -> &'static str {
    match tier {
        1 => "Verified",
        2 => "Trusted",
        3 => "Established",
        4 => "Elite",
        5 => "Legendary",
        _ => "Newcomer",
    }
}

fn moltyid_achievement_name(achievement_id: u8) -> &'static str {
    match achievement_id {
        1 => "First Transaction",
        2 => "Governance Voter",
        3 => "Program Builder",
        4 => "Trusted Agent",
        5 => "Veteran Agent",
        6 => "Legendary Agent",
        7 => "Well Endorsed",
        8 => "Bootstrap Graduation",
        _ => "Unknown Achievement",
    }
}

fn read_u64_le(input: &[u8], offset: usize) -> Option<u64> {
    if input.len() < offset + 8 {
        return None;
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&input[offset..offset + 8]);
    Some(u64::from_le_bytes(bytes))
}

fn extract_single_pubkey(
    params: &Option<serde_json::Value>,
    method: &str,
) -> Result<Pubkey, RpcError> {
    let params = params.as_ref().ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;
    let pubkey_str = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: format!("Invalid params for {}: expected [pubkey]", method),
        })?;
    Pubkey::from_base58(pubkey_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid pubkey: {}", e),
    })
}

fn extract_single_string(
    params: &Option<serde_json::Value>,
    method: &str,
    label: &str,
) -> Result<String, RpcError> {
    let params = params.as_ref().ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;
    params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: format!("Invalid params for {}: expected [{}]", method, label),
        })
}

fn moltyid_hex(pubkey: &Pubkey) -> String {
    hex::encode(pubkey.0)
}

fn moltyid_identity_key(pubkey: &Pubkey) -> Vec<u8> {
    format!("id:{}", moltyid_hex(pubkey)).into_bytes()
}

fn moltyid_reputation_key(pubkey: &Pubkey) -> Vec<u8> {
    format!("rep:{}", moltyid_hex(pubkey)).into_bytes()
}

fn moltyid_reverse_name_key(pubkey: &Pubkey) -> Vec<u8> {
    format!("name_rev:{}", moltyid_hex(pubkey)).into_bytes()
}

fn moltyid_skill_key(pubkey: &Pubkey, index: u8) -> Vec<u8> {
    format!("skill:{}:{}", moltyid_hex(pubkey), index).into_bytes()
}

fn moltyid_vouch_key(pubkey: &Pubkey, index: u16) -> Vec<u8> {
    format!("vouch:{}:{}", moltyid_hex(pubkey), index).into_bytes()
}

fn moltyid_vouch_given_key(pubkey: &Pubkey, index: u16) -> Vec<u8> {
    format!("vouch_given:{}:{}", moltyid_hex(pubkey), index).into_bytes()
}

fn moltyid_achievement_key(pubkey: &Pubkey, achievement_id: u8) -> Vec<u8> {
    format!("ach:{}:{:02}", moltyid_hex(pubkey), achievement_id).into_bytes()
}

fn moltyid_skill_hash(skill_name: &str) -> [u8; 8] {
    let mut out = [0u8; 8];
    for (index, byte) in skill_name.as_bytes().iter().enumerate() {
        if index >= 8 {
            break;
        }
        out[index] = *byte;
    }
    out
}

fn moltyid_attestation_count_key(pubkey: &Pubkey, skill_name: &str) -> Vec<u8> {
    let skill_hash = moltyid_skill_hash(skill_name);
    format!(
        "attest_count_{}_{}",
        moltyid_hex(pubkey),
        hex::encode(skill_hash)
    )
    .into_bytes()
}

fn parse_moltyid_identity_record(input: &[u8]) -> Option<MoltyIdIdentityRecord> {
    if input.len() < MOLTYID_IDENTITY_SIZE {
        return None;
    }

    let mut owner_bytes = [0u8; 32];
    owner_bytes.copy_from_slice(&input[0..32]);
    let owner = Pubkey(owner_bytes);
    let agent_type = input[32];

    let name_len = (input[33] as usize) | ((input[34] as usize) << 8);
    if name_len > 64 || 35 + name_len > input.len() {
        return None;
    }
    let name = String::from_utf8_lossy(&input[35..35 + name_len]).to_string();

    Some(MoltyIdIdentityRecord {
        owner,
        agent_type,
        name,
        reputation: read_u64_le(input, 99)?,
        created_at: read_u64_le(input, 107)?,
        updated_at: read_u64_le(input, 115)?,
        skill_count: input[123],
        vouch_count: (input[124] as u16) | ((input[125] as u16) << 8),
        is_active: input[126] == 1,
    })
}

fn parse_moltyid_skill_record(input: &[u8]) -> Option<MoltyIdSkillRecord> {
    let name_len = *input.first()? as usize;
    if name_len == 0 || 1 + name_len + 1 + 8 > input.len() {
        return None;
    }
    let name = String::from_utf8_lossy(&input[1..1 + name_len]).to_string();
    let proficiency = input[1 + name_len];
    let timestamp = read_u64_le(input, 1 + name_len + 1)?;
    Some(MoltyIdSkillRecord {
        name,
        proficiency,
        timestamp,
    })
}

fn parse_moltyid_vouch_record(input: &[u8]) -> Option<MoltyIdVouchRecord> {
    if input.len() < 40 {
        return None;
    }
    let mut voucher = [0u8; 32];
    voucher.copy_from_slice(&input[0..32]);
    Some(MoltyIdVouchRecord {
        voucher: Pubkey(voucher),
        timestamp: read_u64_le(input, 32)?,
    })
}

fn parse_moltyid_vouch_given_record(input: &[u8]) -> Option<(Pubkey, u64)> {
    if input.len() < 40 {
        return None;
    }
    let mut vouchee = [0u8; 32];
    vouchee.copy_from_slice(&input[0..32]);
    let timestamp = read_u64_le(input, 32)?;
    Some((Pubkey(vouchee), timestamp))
}

fn parse_moltyid_achievement_record(input: &[u8]) -> Option<MoltyIdAchievementRecord> {
    if input.len() < 9 {
        return None;
    }
    Some(MoltyIdAchievementRecord {
        id: input[0],
        timestamp: read_u64_le(input, 1)?,
    })
}

/// CF-based MoltyID identity read — no full account deserialization.
fn get_moltyid_identity(
    state: &RpcState,
    pubkey: &Pubkey,
) -> Option<MoltyIdIdentityRecord> {
    state.state
        .get_program_storage(MOLTYID_SYMBOL, &moltyid_identity_key(pubkey))
        .and_then(|value| parse_moltyid_identity_record(&value))
}

fn get_moltyid_reputation(state: &RpcState, pubkey: &Pubkey) -> Option<u64> {
    state.state
        .get_program_storage(MOLTYID_SYMBOL, &moltyid_reputation_key(pubkey))
        .and_then(|value| read_u64_le(&value, 0))
}

fn get_moltyid_name(
    state: &RpcState,
    pubkey: &Pubkey,
    current_slot: u64,
) -> Option<String> {
    let raw_name = state.state.get_program_storage(MOLTYID_SYMBOL, &moltyid_reverse_name_key(pubkey))?;
    let label = String::from_utf8(raw_name).ok()?;
    let record = state.state.get_program_storage(MOLTYID_SYMBOL, &format!("name:{}", label).into_bytes())?;
    if record.len() < 48 {
        return None;
    }
    let expiry_slot = read_u64_le(&record, 40)?;
    if current_slot >= expiry_slot {
        return None;
    }
    Some(format!("{}.molt", label))
}

fn moltyid_cf_get(state: &RpcState, key: &[u8]) -> Option<Vec<u8>> {
    state.state.get_program_storage(MOLTYID_SYMBOL, key)
}

async fn handle_get_moltyid_identity(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pubkey = extract_single_pubkey(&params, "getMoltyIdIdentity")?;
    let current_slot = state.state.get_last_slot().unwrap_or(0);

    let identity = match get_moltyid_identity(state, &pubkey) {
        Some(identity) => identity,
        None => return Ok(serde_json::Value::Null),
    };

    let score = get_moltyid_reputation(state, &pubkey).unwrap_or(identity.reputation);
    let tier = moltyid_trust_tier(score);
    let molt_name = get_moltyid_name(state, &pubkey, current_slot);

    Ok(serde_json::json!({
        "address": pubkey.to_base58(),
        "owner": identity.owner.to_base58(),
        "name": identity.name,
        "molt_name": molt_name,
        "agent_type": identity.agent_type,
        "agent_type_name": moltyid_agent_type_name(identity.agent_type),
        "reputation": score,
        "trust_tier": tier,
        "trust_tier_name": moltyid_trust_tier_name(tier),
        "created_at": identity.created_at,
        "updated_at": identity.updated_at,
        "skill_count": identity.skill_count,
        "vouch_count": identity.vouch_count,
        "is_active": identity.is_active,
    }))
}

async fn handle_get_moltyid_reputation(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pubkey = extract_single_pubkey(&params, "getMoltyIdReputation")?;

    let score = get_moltyid_reputation(state, &pubkey)
        .or_else(|| get_moltyid_identity(state, &pubkey).map(|identity| identity.reputation))
        .unwrap_or(0);
    let tier = moltyid_trust_tier(score);

    Ok(serde_json::json!({
        "address": pubkey.to_base58(),
        "score": score,
        "tier": tier,
        "tier_name": moltyid_trust_tier_name(tier),
    }))
}

async fn handle_get_moltyid_skills(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pubkey = extract_single_pubkey(&params, "getMoltyIdSkills")?;

    let identity = match get_moltyid_identity(state, &pubkey) {
        Some(identity) => identity,
        None => return Ok(serde_json::json!([])),
    };

    let mut skills = Vec::new();
    for index in 0..identity.skill_count {
        if let Some(raw) = moltyid_cf_get(state, &moltyid_skill_key(&pubkey, index)) {
            if let Some(skill) = parse_moltyid_skill_record(&raw) {
                let attestations = moltyid_cf_get(state, &moltyid_attestation_count_key(&pubkey, &skill.name))
                    .and_then(|value| read_u64_le(&value, 0))
                    .unwrap_or(0);

                skills.push(serde_json::json!({
                    "index": index,
                    "name": skill.name,
                    "proficiency": skill.proficiency,
                    "attestation_count": attestations,
                    "timestamp": skill.timestamp,
                }));
            }
        }
    }

    Ok(serde_json::Value::Array(skills))
}

async fn handle_get_moltyid_vouches(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pubkey = extract_single_pubkey(&params, "getMoltyIdVouches")?;
    let current_slot = state.state.get_last_slot().unwrap_or(0);

    let identity = match get_moltyid_identity(state, &pubkey) {
        Some(identity) => identity,
        None => return Ok(serde_json::json!({"received": [], "given": []})),
    };

    let mut received = Vec::new();
    for index in 0..identity.vouch_count {
        if let Some(raw) = moltyid_cf_get(state, &moltyid_vouch_key(&pubkey, index)) {
            if let Some(vouch) = parse_moltyid_vouch_record(&raw) {
                received.push(serde_json::json!({
                    "voucher": vouch.voucher.to_base58(),
                    "voucher_name": get_moltyid_name(state, &vouch.voucher, current_slot),
                    "timestamp": vouch.timestamp,
                }));
            }
        }
    }

    let mut given = Vec::new();
    for index in 0..identity.vouch_count {
        if let Some(raw) = moltyid_cf_get(state, &moltyid_vouch_given_key(&pubkey, index)) {
            if let Some((vouchee, timestamp)) = parse_moltyid_vouch_given_record(&raw) {
                given.push(serde_json::json!({
                    "vouchee": vouchee.to_base58(),
                    "vouchee_name": get_moltyid_name(state, &vouchee, current_slot),
                    "timestamp": timestamp,
                }));
            }
        }
    }

    // Backward compatibility for pre-indexed historical data — scan CF entries.
    if given.is_empty() {
        let program = resolve_symbol_pubkey(state, MOLTYID_SYMBOL)?;
        let entries = state.state.get_contract_storage_entries(&program, 10_000, None).unwrap_or_default();
        for (key, value) in &entries {
            if !key.starts_with(b"id:") {
                continue;
            }
            let Some(vouchee_identity) = parse_moltyid_identity_record(value) else {
                continue;
            };
            for index in 0..vouchee_identity.vouch_count {
                if let Some(raw_vouch) = moltyid_cf_get(state, &moltyid_vouch_key(&vouchee_identity.owner, index)) {
                    if let Some(vouch) = parse_moltyid_vouch_record(&raw_vouch) {
                        if vouch.voucher == pubkey {
                            given.push(serde_json::json!({
                                "vouchee": vouchee_identity.owner.to_base58(),
                                "vouchee_name": get_moltyid_name(state, &vouchee_identity.owner, current_slot),
                                "timestamp": vouch.timestamp,
                            }));
                        }
                    }
                }
            }
        }
    }

    Ok(serde_json::json!({
        "received": received,
        "given": given,
    }))
}

async fn handle_get_moltyid_achievements(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pubkey = extract_single_pubkey(&params, "getMoltyIdAchievements")?;

    if get_moltyid_identity(state, &pubkey).is_none() {
        return Ok(serde_json::json!([]));
    }

    let mut achievements = Vec::new();
    for achievement_id in 1u8..=8u8 {
        if let Some(raw) = moltyid_cf_get(state, &moltyid_achievement_key(&pubkey, achievement_id)) {
            if let Some(achievement) = parse_moltyid_achievement_record(&raw) {
                achievements.push(serde_json::json!({
                    "id": achievement.id,
                    "name": moltyid_achievement_name(achievement.id),
                    "timestamp": achievement.timestamp,
                }));
            }
        }
    }

    Ok(serde_json::Value::Array(achievements))
}

async fn handle_get_moltyid_profile(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pubkey = extract_single_pubkey(&params, "getMoltyIdProfile")?;
    let current_slot = state.state.get_last_slot().unwrap_or(0);

    let identity = match get_moltyid_identity(state, &pubkey) {
        Some(identity) => identity,
        None => return Ok(serde_json::Value::Null),
    };

    let reputation = get_moltyid_reputation(state, &pubkey).unwrap_or(identity.reputation);
    let tier = moltyid_trust_tier(reputation);
    let molt_name = get_moltyid_name(state, &pubkey, current_slot);

    let mut skills = Vec::new();
    for index in 0..identity.skill_count {
        if let Some(raw) = moltyid_cf_get(state, &moltyid_skill_key(&pubkey, index)) {
            if let Some(skill) = parse_moltyid_skill_record(&raw) {
                let attestations = moltyid_cf_get(state, &moltyid_attestation_count_key(&pubkey, &skill.name))
                    .and_then(|value| read_u64_le(&value, 0))
                    .unwrap_or(0);

                skills.push(serde_json::json!({
                    "index": index,
                    "name": skill.name,
                    "proficiency": skill.proficiency,
                    "attestation_count": attestations,
                    "timestamp": skill.timestamp,
                }));
            }
        }
    }

    let mut received_vouches = Vec::new();
    for index in 0..identity.vouch_count {
        if let Some(raw) = moltyid_cf_get(state, &moltyid_vouch_key(&pubkey, index)) {
            if let Some(vouch) = parse_moltyid_vouch_record(&raw) {
                received_vouches.push(serde_json::json!({
                    "voucher": vouch.voucher.to_base58(),
                    "voucher_name": get_moltyid_name(state, &vouch.voucher, current_slot),
                    "timestamp": vouch.timestamp,
                }));
            }
        }
    }

    let mut given_vouches = Vec::new();
    for index in 0..identity.vouch_count {
        if let Some(raw) = moltyid_cf_get(state, &moltyid_vouch_given_key(&pubkey, index)) {
            if let Some((vouchee, timestamp)) = parse_moltyid_vouch_given_record(&raw) {
                given_vouches.push(serde_json::json!({
                    "vouchee": vouchee.to_base58(),
                    "vouchee_name": get_moltyid_name(state, &vouchee, current_slot),
                    "timestamp": timestamp,
                }));
            }
        }
    }

    if given_vouches.is_empty() {
        let program = resolve_symbol_pubkey(state, MOLTYID_SYMBOL)?;
        let entries = state.state.get_contract_storage_entries(&program, 10_000, None).unwrap_or_default();
        for (key, value) in &entries {
            if !key.starts_with(b"id:") {
                continue;
            }
            let Some(vouchee_identity) = parse_moltyid_identity_record(value) else {
                continue;
            };
            for index in 0..vouchee_identity.vouch_count {
                if let Some(raw_vouch) = moltyid_cf_get(state, &moltyid_vouch_key(&vouchee_identity.owner, index)) {
                    if let Some(vouch) = parse_moltyid_vouch_record(&raw_vouch) {
                        if vouch.voucher == pubkey {
                            given_vouches.push(serde_json::json!({
                                "vouchee": vouchee_identity.owner.to_base58(),
                                "vouchee_name": get_moltyid_name(state, &vouchee_identity.owner, current_slot),
                                "timestamp": vouch.timestamp,
                            }));
                        }
                    }
                }
            }
        }
    }

    let mut achievements = Vec::new();
    for achievement_id in 1u8..=8u8 {
        if let Some(raw) = moltyid_cf_get(state, &moltyid_achievement_key(&pubkey, achievement_id)) {
            if let Some(achievement) = parse_moltyid_achievement_record(&raw) {
                achievements.push(serde_json::json!({
                    "id": achievement.id,
                    "name": moltyid_achievement_name(achievement.id),
                    "timestamp": achievement.timestamp,
                }));
            }
        }
    }

    let endpoint = moltyid_cf_get(state, &format!("endpoint:{}", moltyid_hex(&pubkey)).into_bytes())
        .and_then(|raw| String::from_utf8(raw).ok());

    let metadata = moltyid_cf_get(state, &format!("metadata:{}", moltyid_hex(&pubkey)).into_bytes())
        .and_then(|raw| String::from_utf8(raw).ok())
        .map(|text| {
            serde_json::from_str::<serde_json::Value>(&text).unwrap_or(serde_json::json!(text))
        });

    let availability = moltyid_cf_get(state, &format!("availability:{}", moltyid_hex(&pubkey)).into_bytes())
        .and_then(|raw| raw.first().copied())
        .unwrap_or(0);

    let availability_name = match availability {
        1 => "available",
        2 => "busy",
        _ => "offline",
    };

    let rate = moltyid_cf_get(state, &format!("rate:{}", moltyid_hex(&pubkey)).into_bytes())
        .and_then(|raw| read_u64_le(&raw, 0))
        .unwrap_or(0);

    let mut contributions = serde_json::Map::new();
    let labels = [
        "successful_txs",
        "governance_votes",
        "programs_deployed",
        "uptime_hours",
        "peer_endorsements",
        "failed_txs",
        "slashing_events",
    ];
    for (index, label) in labels.iter().enumerate() {
        let key = format!("cont:{}:{}", moltyid_hex(&pubkey), index).into_bytes();
        let value = moltyid_cf_get(state, &key)
            .and_then(|raw| read_u64_le(&raw, 0))
            .unwrap_or(0);
        contributions.insert((*label).to_string(), serde_json::json!(value));
    }

    Ok(serde_json::json!({
        "identity": {
            "address": pubkey.to_base58(),
            "owner": identity.owner.to_base58(),
            "name": identity.name,
            "agent_type": identity.agent_type,
            "agent_type_name": moltyid_agent_type_name(identity.agent_type),
            "reputation": reputation,
            "created_at": identity.created_at,
            "updated_at": identity.updated_at,
            "skill_count": identity.skill_count,
            "vouch_count": identity.vouch_count,
            "is_active": identity.is_active,
        },
        "molt_name": molt_name,
        "reputation": {
            "score": reputation,
            "tier": tier,
            "tier_name": moltyid_trust_tier_name(tier),
        },
        "skills": skills,
        "vouches": {
            "received": received_vouches,
            "given": given_vouches,
        },
        "achievements": achievements,
        "agent": {
            "endpoint": endpoint,
            "metadata": metadata,
            "availability": availability,
            "availability_name": availability_name,
            "rate": rate,
        },
        "contributions": contributions,
    }))
}

fn normalize_molt_label(input: &str) -> String {
    input
        .trim()
        .to_ascii_lowercase()
        .trim_end_matches(".molt")
        .to_string()
}

async fn handle_resolve_molt_name(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let raw_name = extract_single_string(&params, "resolveMoltName", "name")?;
    let label = normalize_molt_label(&raw_name);
    if label.is_empty() {
        return Ok(serde_json::Value::Null);
    }

    let current_slot = state.state.get_last_slot().unwrap_or(0);
    let key = format!("name:{}", label).into_bytes();

    let Some(record) = moltyid_cf_get(state, &key) else {
        return Ok(serde_json::Value::Null);
    };
    if record.len() < 48 {
        return Ok(serde_json::Value::Null);
    }
    let expiry_slot = read_u64_le(&record, 40).unwrap_or(0);
    if current_slot >= expiry_slot {
        return Ok(serde_json::Value::Null);
    }

    let mut owner_bytes = [0u8; 32];
    owner_bytes.copy_from_slice(&record[0..32]);
    let owner = Pubkey(owner_bytes);
    Ok(serde_json::json!({
        "name": format!("{}.molt", label),
        "owner": owner.to_base58(),
        "registered_slot": read_u64_le(&record, 32).unwrap_or(0),
        "expiry_slot": expiry_slot,
    }))
}

async fn handle_reverse_molt_name(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pubkey = extract_single_pubkey(&params, "reverseMoltName")?;
    let current_slot = state.state.get_last_slot().unwrap_or(0);
    match get_moltyid_name(state, &pubkey, current_slot) {
        Some(name) => Ok(serde_json::json!({"name": name})),
        None => Ok(serde_json::Value::Null),
    }
}

async fn handle_batch_reverse_molt_names(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.as_ref().ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let addresses = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [pubkey1, pubkey2, ...]".to_string(),
    })?;

    let current_slot = state.state.get_last_slot().unwrap_or(0);

    let mut output = serde_json::Map::new();
    for value in addresses.iter().take(500) {
        let Some(address_str) = value.as_str() else {
            continue;
        };
        let parsed = Pubkey::from_base58(address_str).ok();
        let name = parsed.and_then(|pubkey| get_moltyid_name(state, &pubkey, current_slot));
        output.insert(
            address_str.to_string(),
            name.map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
        );
    }

    Ok(serde_json::Value::Object(output))
}

async fn handle_search_molt_names(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let prefix_raw = extract_single_string(&params, "searchMoltNames", "prefix")?;
    let prefix = normalize_molt_label(&prefix_raw);
    let program = resolve_symbol_pubkey(state, MOLTYID_SYMBOL)?;
    let current_slot = state.state.get_last_slot().unwrap_or(0);
    let entries = state.state.get_contract_storage_entries(&program, 100_000, None).unwrap_or_default();

    let mut names = Vec::new();
    for (key, value) in &entries {
        if !key.starts_with(b"name:") || key.starts_with(b"name_rev:") {
            continue;
        }
        let Ok(key_str) = std::str::from_utf8(key) else {
            continue;
        };
        let Some(label) = key_str.strip_prefix("name:") else {
            continue;
        };
        if !label.starts_with(&prefix) {
            continue;
        }
        if value.len() < 48 {
            continue;
        }
        let expiry_slot = read_u64_le(value, 40).unwrap_or(0);
        if current_slot >= expiry_slot {
            continue;
        }
        names.push(format!("{}.molt", label));
    }

    names.sort_unstable();
    names.truncate(100);
    Ok(serde_json::json!(names))
}

async fn handle_get_moltyid_agent_directory(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let program = resolve_symbol_pubkey(state, MOLTYID_SYMBOL)?;
    let current_slot = state.state.get_last_slot().unwrap_or(0);

    let mut filter_type: Option<u8> = None;
    let mut filter_available: Option<bool> = None;
    let mut min_reputation: Option<u64> = None;
    let mut limit: usize = 50;
    let mut offset: usize = 0;

    if let Some(value) = params {
        let options_obj = if let Some(array) = value.as_array() {
            array.first().and_then(|entry| entry.as_object())
        } else {
            value.as_object()
        };

        if let Some(options) = options_obj {
            filter_type = options
                .get("type")
                .and_then(|v| v.as_u64())
                .map(|v| v as u8);
            filter_available = options.get("available").and_then(|v| v.as_bool());
            min_reputation = options.get("min_reputation").and_then(|v| v.as_u64());
            limit = options
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(50)
                .min(500) as usize;
            offset = options.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        }
    }

    let entries = state.state.get_contract_storage_entries(&program, 10_000, None).unwrap_or_default();
    let mut agents = Vec::new();
    for (key, value) in &entries {
        if !key.starts_with(b"id:") {
            continue;
        }
        let Some(identity) = parse_moltyid_identity_record(value) else {
            continue;
        };
        if !identity.is_active {
            continue;
        }
        if let Some(required_type) = filter_type {
            if identity.agent_type != required_type {
                continue;
            }
        }

        let pubkey = identity.owner;
        let reputation = get_moltyid_reputation(state, &pubkey).unwrap_or(identity.reputation);
        if let Some(minimum) = min_reputation {
            if reputation < minimum {
                continue;
            }
        }

        let availability = moltyid_cf_get(state, &format!("availability:{}", moltyid_hex(&pubkey)).into_bytes())
            .and_then(|raw| raw.first().copied())
            .unwrap_or(0);
        let is_available = availability == 1;
        if let Some(required_available) = filter_available {
            if required_available != is_available {
                continue;
            }
        }

        let rate = moltyid_cf_get(state, &format!("rate:{}", moltyid_hex(&pubkey)).into_bytes())
            .and_then(|raw| read_u64_le(&raw, 0))
            .unwrap_or(0);

        let endpoint = moltyid_cf_get(state, &format!("endpoint:{}", moltyid_hex(&pubkey)).into_bytes())
            .and_then(|raw| String::from_utf8(raw).ok());

        let tier = moltyid_trust_tier(reputation);

        agents.push(serde_json::json!({
            "address": pubkey.to_base58(),
            "name": identity.name,
            "molt_name": get_moltyid_name(state, &pubkey, current_slot),
            "agent_type": identity.agent_type,
            "agent_type_name": moltyid_agent_type_name(identity.agent_type),
            "reputation": reputation,
            "trust_tier": tier,
            "trust_tier_name": moltyid_trust_tier_name(tier),
            "availability": availability,
            "available": is_available,
            "rate": rate,
            "endpoint": endpoint,
            "skill_count": identity.skill_count,
            "vouch_count": identity.vouch_count,
            "created_at": identity.created_at,
            "updated_at": identity.updated_at,
        }));

        if agents.len() >= 10_000 {
            break;
        }
    }

    agents.sort_by(|a, b| {
        let left = a.get("reputation").and_then(|v| v.as_u64()).unwrap_or(0);
        let right = b.get("reputation").and_then(|v| v.as_u64()).unwrap_or(0);
        right.cmp(&left)
    });

    let total = agents.len();
    let agents: Vec<serde_json::Value> = agents.into_iter().skip(offset).take(limit).collect();

    Ok(serde_json::json!({
        "agents": agents,
        "count": agents.len(),
        "total": total,
    }))
}

async fn handle_get_moltyid_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let program = resolve_symbol_pubkey(state, "YID")?;

    let total_identities = cf_stats_u64(state, "YID", b"mid_identity_count");
    let total_names = cf_stats_u64(state, "YID", b"molt_name_count");

    // PERF-NOTE (P10-VAL-07): Full CF storage scan for tier distribution.
    // Acceptable for current identity counts. Consider caching or contract-side
    // aggregate counters if identity count exceeds 100K.
    let entries = state
        .state
        .get_contract_storage_entries(&program, 100_000, None)
        .unwrap_or_default();

    let mut tier_distribution = [0u64; 6];
    let mut total_skills: u64 = 0;
    let mut total_vouches: u64 = 0;
    let mut total_attestations: u64 = 0;

    for (key, value) in &entries {
        if key.starts_with(b"id:") {
            let Some(identity) = parse_moltyid_identity_record(value) else {
                continue;
            };
            // Read reputation from CF
            let rep_key = moltyid_reputation_key(&identity.owner);
            let score = state
                .state
                .get_program_storage("YID", &rep_key)
                .and_then(|v| read_u64_le(&v, 0))
                .unwrap_or(identity.reputation);
            tier_distribution[moltyid_trust_tier(score) as usize] += 1;
            total_skills += identity.skill_count as u64;
            total_vouches += identity.vouch_count as u64;
        } else if key.starts_with(b"attest_count_") {
            total_attestations += read_u64_le(value, 0).unwrap_or(0);
        }
    }

    Ok(serde_json::json!({
        "total_identities": total_identities,
        "total_names": total_names,
        "total_skills": total_skills,
        "total_vouches": total_vouches,
        "total_attestations": total_attestations,
        "tier_distribution": {
            "newcomer": tier_distribution[0],
            "verified": tier_distribution[1],
            "trusted": tier_distribution[2],
            "established": tier_distribution[3],
            "elite": tier_distribution[4],
            "legendary": tier_distribution[5],
        },
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
        // P9-RPC-01: Use the named constant for the EVM sentinel blockhash.
        // The processor recognises this sentinel and routes directly to the
        // EVM execution path, skipping native blockhash + sig verification
        // (the EVM layer provides its own replay protection via nonces + ECDSA).
        recent_blockhash: moltchain_core::EVM_SENTINEL_BLOCKHASH,
    };

    let tx = Transaction {
        // AUDIT-FIX 2.15: Placeholder signature so downstream code doesn't reject
        // as malformed. The actual ECDSA signature is inside the EVM transaction data.
        signatures: vec![[0u8; 64]],
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

// ═══════════════════════════════════════════════════════════════════════════════
// EVM COMPATIBILITY HANDLERS (full implementations)
// ═══════════════════════════════════════════════════════════════════════════════

/// Parse an EVM block tag ("latest", "earliest", "pending", "safe", "finalized", or hex number)
/// into a slot number.
fn parse_evm_block_tag(tag: &str, state: &RpcState) -> Result<u64, RpcError> {
    match tag {
        "latest" | "pending" | "safe" | "finalized" => {
            state.state.get_last_slot().map_err(|e| RpcError {
                code: -32000,
                message: format!("Database error: {}", e),
            })
        }
        "earliest" => Ok(0),
        hex if hex.starts_with("0x") => u64::from_str_radix(hex.trim_start_matches("0x"), 16)
            .map_err(|_| RpcError {
                code: -32602,
                message: format!("Invalid block number: {}", hex),
            }),
        _ => Err(RpcError {
            code: -32602,
            message: format!("Invalid block tag: {}", tag),
        }),
    }
}

/// Format a real Block into EVM-compatible JSON block object.
fn format_evm_block(block: &moltchain_core::Block, include_txs: bool) -> serde_json::Value {
    let slot = block.header.slot;
    // AUDIT-FIX P10-RPC-01: Use actual block hash, NOT state_root.
    // state_root is a Merkle root of account state — it is NOT the block identifier.
    // EVM tooling (MetaMask, Ethers.js) uses the "hash" field to track/index blocks.
    let block_hash = format!("0x{}", hex::encode(block.hash().0));
    let parent_hash = format!("0x{}", hex::encode(block.header.parent_hash.0));
    let timestamp = format!("0x{:x}", block.header.timestamp);
    let tx_root = format!("0x{}", hex::encode(block.header.tx_root.0));
    let validator = format!("0x{}", hex::encode(&block.header.validator[12..32]));

    let transactions: serde_json::Value = if include_txs {
        serde_json::json!(block
            .transactions
            .iter()
            .map(|tx| {
                let sig = tx.signature();
                let from_addr = tx
                    .message
                    .instructions
                    .first()
                    .and_then(|ix| ix.accounts.first())
                    .map(|acc| format!("0x{}", hex::encode(&acc.0[12..32])))
                    .unwrap_or_else(|| "0x0000000000000000000000000000000000000000".to_string());
                serde_json::json!({
                    "hash": format!("0x{}", hex::encode(sig.0)),
                    "blockNumber": format!("0x{:x}", slot),
                    "blockHash": &block_hash,
                    "from": from_addr,
                    "to": tx.message.instructions.first().map(|ix|
                        format!("0x{}", hex::encode(&ix.program_id.0[12..32]))
                    ),
                    "value": "0x0",
                    "gas": "0x5208",
                    "gasPrice": "0x0",
                    "input": "0x",
                    "nonce": "0x0",
                    "transactionIndex": "0x0",
                })
            })
            .collect::<Vec<_>>())
    } else {
        serde_json::json!(block
            .transactions
            .iter()
            .map(|tx| {
                let sig = tx.signature();
                serde_json::Value::String(format!("0x{}", hex::encode(sig.0)))
            })
            .collect::<Vec<_>>())
    };

    serde_json::json!({
        "number": format!("0x{:x}", slot),
        "hash": block_hash,
        "parentHash": parent_hash,
        "nonce": "0x0000000000000000",
        "sha3Uncles": "0x1dcc4de8dec75d7aab85b567b6ccd41ad312451b948a7413f0a142fd40d49347",
        "logsBloom": format!("0x{}", "00".repeat(256)),
        "transactionsRoot": tx_root,
        "stateRoot": format!("0x{}", hex::encode(block.header.state_root.0)),
        "receiptsRoot": "0x0000000000000000000000000000000000000000000000000000000000000000",
        "miner": validator,
        "difficulty": "0x0",
        "totalDifficulty": "0x0",
        "extraData": "0x",
        "size": format!("0x{:x}", block.transactions.len() * 256 + 512),
        "gasLimit": "0x1c9c380",
        "gasUsed": format!("0x{:x}", block.transactions.len() * 21000),
        "timestamp": timestamp,
        "transactions": transactions,
        "uncles": [],
        "baseFeePerGas": "0x0",
        "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    })
}

/// eth_getCode — return contract bytecode at an address.
/// Checks EVM accounts first, then native contract accounts mapped via the EVM registry.
async fn handle_eth_get_code(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;
    let addr_str = params
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [address, block?]".to_string(),
        })?;

    let evm_addr =
        moltchain_core::StateStore::parse_evm_address(addr_str).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid EVM address: {}", e),
        })?;

    // 1. Check pure EVM account (has EVM bytecode stored directly)
    if let Ok(Some(acct)) = state.state.get_evm_account(&evm_addr) {
        if !acct.code.is_empty() {
            return Ok(serde_json::json!(format!("0x{}", hex::encode(&acct.code))));
        }
    }

    // 2. Check native contract mapped to this EVM address
    if let Ok(Some(pubkey)) = state.state.lookup_evm_address(&evm_addr) {
        if let Ok(Some(account)) = state.state.get_account(&pubkey) {
            if account.executable && !account.data.is_empty() {
                // Try to deserialize as ContractAccount to get WASM bytecode
                if let Ok(contract) = serde_json::from_slice::<ContractAccount>(&account.data) {
                    if !contract.code.is_empty() {
                        // Return WASM bytecode hexified — EVM tools see non-"0x" = contract
                        return Ok(serde_json::json!(format!(
                            "0x{}",
                            hex::encode(&contract.code)
                        )));
                    }
                }
                // Fallback: account.data is non-empty executable but not parsable
                // Return EIP-7702 designated invalid opcode sentinel
                return Ok(serde_json::json!("0xfe"));
            }
        }
    }

    // EOA or unknown address — no code
    Ok(serde_json::json!("0x"))
}

/// eth_getTransactionCount — return nonce for an address.
/// Checks EVM account nonce first, then counts native transactions.
async fn handle_eth_get_transaction_count(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;
    let addr_str = params
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [address, block?]".to_string(),
        })?;

    let evm_addr =
        moltchain_core::StateStore::parse_evm_address(addr_str).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid EVM address: {}", e),
        })?;

    // 1. Check EVM account nonce
    if let Ok(Some(acct)) = state.state.get_evm_account(&evm_addr) {
        return Ok(serde_json::json!(format!("0x{:x}", acct.nonce)));
    }

    // 2. Check native account transaction count
    if let Ok(Some(pubkey)) = state.state.lookup_evm_address(&evm_addr) {
        let count = state.state.count_account_txs(&pubkey).unwrap_or(0);
        return Ok(serde_json::json!(format!("0x{:x}", count)));
    }

    Ok(serde_json::json!("0x0"))
}

/// eth_estimateGas — estimate gas for a transaction.
/// MoltChain uses flat fees, not gas-based metering.
/// Returns the actual fee (in shells) as the gas value with an implicit gasPrice of 1.
async fn handle_eth_estimate_gas(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let fee_config = state
        .state
        .get_fee_config()
        .unwrap_or_else(|_| moltchain_core::FeeConfig::default_from_constants());

    let gas = if let Some(ref params) = params {
        if let Some(tx_obj) = params.as_array().and_then(|a| a.first()) {
            let to_field = tx_obj.get("to");
            let has_to = to_field
                .map(|v| !v.is_null() && v.as_str().is_some_and(|s| !s.is_empty()))
                .unwrap_or(false);
            let has_data = tx_obj
                .get("data")
                .or_else(|| tx_obj.get("input"))
                .and_then(|d| d.as_str())
                .is_some_and(|d| d.len() > 2); // "0x" alone = no data

            if !has_to && has_data {
                // Contract deployment
                fee_config.contract_deploy_fee
            } else {
                // Transfer or contract call — both cost base_fee
                fee_config.base_fee
            }
        } else {
            fee_config.base_fee
        }
    } else {
        fee_config.base_fee
    };

    Ok(serde_json::json!(format!("0x{:x}", gas)))
}

/// eth_gasPrice — return current gas price.
/// AUDIT-FIX A11-01: MoltChain uses flat fees, so gasPrice = 1 (1 shell per gas unit).
/// Total cost = gasPrice(1) × estimateGas(actual_fee_in_shells) = actual_fee.
/// Previously this returned base_fee, causing MetaMask to display fee² (base_fee × base_fee).
async fn handle_eth_gas_price(_state: &RpcState) -> Result<serde_json::Value, RpcError> {
    // gasPrice = 1 shell per gas unit.
    // eth_estimateGas returns the actual fee in shells (= gas units consumed).
    // MetaMask/wallets compute: total = gasPrice × gasEstimate = 1 × fee = fee. ✓
    Ok(serde_json::json!("0x1"))
}

/// eth_getBlockByNumber — return full block data for a given block number or tag.
async fn handle_eth_get_block_by_number(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;
    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [blockTag, includeTxs?]".to_string(),
    })?;

    let block_tag = arr.first().and_then(|v| v.as_str()).unwrap_or("latest");
    let include_txs = arr.get(1).and_then(|v| v.as_bool()).unwrap_or(false);

    let slot = parse_evm_block_tag(block_tag, state)?;

    let block = state.state.get_block_by_slot(slot).map_err(|e| RpcError {
        code: -32000,
        message: format!("Database error: {}", e),
    })?;

    match block {
        Some(b) => Ok(format_evm_block(&b, include_txs)),
        None => Ok(serde_json::json!(null)),
    }
}

/// eth_getBlockByHash — return full block data for a given block hash.
async fn handle_eth_get_block_by_hash(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;
    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [blockHash, includeTxs?]".to_string(),
    })?;

    let hash_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing block hash".to_string(),
        })?;
    let include_txs = arr.get(1).and_then(|v| v.as_bool()).unwrap_or(false);

    let hash_hex = hash_str.strip_prefix("0x").unwrap_or(hash_str);
    let hash_bytes = hex::decode(hash_hex).map_err(|_| RpcError {
        code: -32602,
        message: "Invalid block hash hex".to_string(),
    })?;

    if hash_bytes.len() != 32 {
        return Err(RpcError {
            code: -32602,
            message: "Block hash must be 32 bytes".to_string(),
        });
    }

    let mut hash_arr = [0u8; 32];
    hash_arr.copy_from_slice(&hash_bytes);

    let block = state
        .state
        .get_block(&Hash(hash_arr))
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    match block {
        Some(b) => Ok(format_evm_block(&b, include_txs)),
        None => Ok(serde_json::json!(null)),
    }
}

/// eth_getLogs — return contract event logs matching a filter.
/// Scans events by slot range and optionally filters by address.
async fn handle_eth_get_logs(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;

    let filter = params
        .as_array()
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or(serde_json::json!({}));

    let latest_slot = state.state.get_last_slot().unwrap_or(0);

    let from_slot = filter
        .get("fromBlock")
        .and_then(|v| v.as_str())
        .map(|tag| parse_evm_block_tag(tag, state))
        .transpose()?
        .unwrap_or(latest_slot);

    let to_slot = filter
        .get("toBlock")
        .and_then(|v| v.as_str())
        .map(|tag| parse_evm_block_tag(tag, state))
        .transpose()?
        .unwrap_or(latest_slot);

    // Cap range to avoid unbounded scans (max 1000 blocks)
    let effective_from = if to_slot > 1000 && from_slot < to_slot.saturating_sub(1000) {
        to_slot.saturating_sub(1000)
    } else {
        from_slot
    };

    // Optional address filter
    let filter_address: Option<[u8; 20]> = filter
        .get("address")
        .and_then(|v| v.as_str())
        .map(moltchain_core::StateStore::parse_evm_address)
        .transpose()
        .map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid address filter: {}", e),
        })?;

    // Optional topics filter (array of topic hashes)
    let filter_topics: Vec<Option<String>> = filter
        .get("topics")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|t| t.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let mut logs = Vec::new();
    /// AUDIT-FIX F-13: Cap returned log count to prevent unbounded memory growth.
    const MAX_LOG_RESULTS: usize = 10_000;

    for slot in effective_from..=to_slot {
        // AUDIT-FIX F-5: Reset logIndex per block (EVM spec requires per-block indexing)
        let mut log_index: u64 = 0;
        let events = state
            .state
            .get_events_by_slot(slot, 10_000)
            .unwrap_or_default();

        for event in &events {
            // If address filter is set, resolve the event program to an EVM address and compare
            if let Some(ref addr_filter) = filter_address {
                // Look up native program's EVM address
                if let Ok(Some(evm_addr)) = state.state.lookup_native_to_evm(&event.program) {
                    if &evm_addr != addr_filter {
                        continue;
                    }
                } else {
                    // Program has no EVM mapping — use last 20 bytes of pubkey
                    if &event.program.0[12..32] != addr_filter.as_slice() {
                        continue;
                    }
                }
            }

            // Build topics from event name + data keys
            let mut topics = Vec::new();
            // AUDIT-FIX A11-02: topic[0] = keccak256(event_name) — standard EVM topic format.
            // Previously used SHA-256, which breaks all EVM tooling (Ethers.js, web3.py, The Graph).
            let event_hash = {
                use sha3::{Digest, Keccak256};
                let mut hasher = Keccak256::new();
                hasher.update(event.name.as_bytes());
                let result = hasher.finalize();
                format!("0x{}", hex::encode(result))
            };
            topics.push(serde_json::Value::String(event_hash));

            // Additional topics from indexed data fields
            for value in event.data.values() {
                if topics.len() >= 4 {
                    break;
                }
                // Pad value to 32 bytes (topic size)
                let padded = format!("0x{:0>64}", hex::encode(value.as_bytes()));
                topics.push(serde_json::Value::String(padded));
            }

            // Apply topics filter
            let mut topics_match = true;
            for (i, filter_topic) in filter_topics.iter().enumerate() {
                if let Some(ref ft) = filter_topic {
                    if let Some(event_topic) = topics.get(i).and_then(|t| t.as_str()) {
                        if !event_topic.eq_ignore_ascii_case(ft) {
                            topics_match = false;
                            break;
                        }
                    } else {
                        topics_match = false;
                        break;
                    }
                }
                // None = wildcard, matches anything
            }
            if !topics_match {
                continue;
            }

            // AUDIT-FIX P10-RPC-03: ABI-encode data values (each left-padded to 32 bytes).
            // Raw concatenation of UTF-8 bytes breaks EVM ABI decoding in ethers.js / web3.py.
            let data_hex = {
                let mut data_bytes = Vec::new();
                for v in event.data.values() {
                    let v_bytes = v.as_bytes();
                    // ABI encoding: each value is left-padded to 32 bytes
                    if v_bytes.len() < 32 {
                        let padding = 32 - v_bytes.len();
                        data_bytes.extend(std::iter::repeat_n(0u8, padding));
                    }
                    data_bytes.extend_from_slice(v_bytes);
                }
                format!("0x{}", hex::encode(&data_bytes))
            };

            let contract_addr =
                if let Ok(Some(evm_addr)) = state.state.lookup_native_to_evm(&event.program) {
                    format!("0x{}", hex::encode(evm_addr))
                } else {
                    format!("0x{}", hex::encode(&event.program.0[12..32]))
                };

            // AUDIT-FIX P10-RPC-01: Use actual block hash, not state_root.
            let block_hash = state
                .state
                .get_block_by_slot(slot)
                .ok()
                .flatten()
                .map(|b| format!("0x{}", hex::encode(b.hash().0)))
                .unwrap_or_else(|| format!("0x{:064x}", slot));

            // AUDIT-FIX P10-RPC-02: Derive deterministic transactionHash from
            // keccak256(block_hash_bytes || log_index). The previous code used
            // a sequential counter (log_index) formatted as hex, which fabricated
            // colliding "transaction hashes" across different blocks.
            let tx_hash = {
                use sha3::{Digest, Keccak256};
                let block_hash_hex = block_hash.strip_prefix("0x").unwrap_or(&block_hash);
                let bh_bytes = hex::decode(block_hash_hex).unwrap_or_default();
                let mut hasher = Keccak256::new();
                hasher.update(&bh_bytes);
                hasher.update(log_index.to_be_bytes());
                format!("0x{}", hex::encode(hasher.finalize()))
            };

            logs.push(serde_json::json!({
                "address": contract_addr,
                "topics": topics,
                "data": data_hex,
                "blockNumber": format!("0x{:x}", slot),
                "blockHash": block_hash,
                "transactionHash": tx_hash,
                "transactionIndex": "0x0",
                "logIndex": format!("0x{:x}", log_index),
                "removed": false,
            }));
            log_index += 1;

            // AUDIT-FIX F-13: Stop collecting once we hit the cap
            if logs.len() >= MAX_LOG_RESULTS {
                break;
            }
        }
        // AUDIT-FIX F-13: Also break the outer slot loop if cap reached
        if logs.len() >= MAX_LOG_RESULTS {
            break;
        }
    }

    Ok(serde_json::json!(logs))
}

/// eth_getStorageAt — read a storage slot from an EVM contract.
async fn handle_eth_get_storage_at(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params".to_string(),
    })?;
    let arr = params.as_array().ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [address, slot, block?]".to_string(),
    })?;

    let addr_str = arr
        .first()
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing address".to_string(),
        })?;
    let slot_str = arr
        .get(1)
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Missing storage slot".to_string(),
        })?;

    let evm_addr =
        moltchain_core::StateStore::parse_evm_address(addr_str).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid address: {}", e),
        })?;

    // Parse slot as a hex-encoded 32-byte value
    let slot_hex = slot_str.strip_prefix("0x").unwrap_or(slot_str);
    let slot_bytes_vec = hex::decode(slot_hex).map_err(|_| RpcError {
        code: -32602,
        message: "Invalid storage slot hex".to_string(),
    })?;
    let mut slot_arr = [0u8; 32];
    // Right-align the slot bytes (big-endian)
    let start = 32usize.saturating_sub(slot_bytes_vec.len());
    slot_arr[start..].copy_from_slice(&slot_bytes_vec[..slot_bytes_vec.len().min(32)]);

    let value = state
        .state
        .get_evm_storage(&evm_addr, &slot_arr)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    Ok(serde_json::json!(format!("0x{:064x}", value)))
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
        let tier = position.lock_tier as u8;
        let tier_name = position.lock_tier.display_name();
        let multiplier = position.lock_tier.reward_multiplier_bp() as f64 / 10_000.0;
        Ok(serde_json::json!({
            "owner": user_pubkey,
            "st_molt_amount": position.st_molt_amount,
            "molt_deposited": position.molt_deposited,
            "current_value_molt": current_value,
            "rewards_earned": position.rewards_earned,
            "deposited_at": position.deposited_at,
            "lock_tier": tier,
            "lock_tier_name": tier_name,
            "lock_until": position.lock_until,
            "reward_multiplier": multiplier
        }))
    } else {
        Ok(serde_json::json!({
            "owner": user_pubkey,
            "st_molt_amount": 0,
            "molt_deposited": 0,
            "current_value_molt": 0,
            "rewards_earned": 0,
            "deposited_at": 0,
            "lock_tier": 0,
            "lock_tier_name": "Flexible",
            "lock_until": 0,
            "reward_multiplier": 1.0
        }))
    }
}

/// Handle getReefStakePoolInfo: Get global ReefStake pool info
async fn handle_get_reefstake_pool_info(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    use moltchain_core::consensus::{decayed_reward, SLOTS_PER_YEAR, TRANSACTION_BLOCK_REWARD};

    let pool = state.state.get_reefstake_pool().map_err(|e| RpcError {
        code: -32603,
        message: format!("Failed to get ReefStake pool: {}", e),
    })?;

    // Derive active validators count and APY from the consensus StakePool
    let (active_validators, apy_percent) = if let Some(ref sp_arc) = state.stake_pool {
        let sp = sp_arc.read().await;
        let stats = sp.get_stats();
        let slots_per_day = SLOTS_PER_YEAR / 365;
        // Apply 20% annual reward decay based on current slot
        let current_slot = state.state.get_last_slot().unwrap_or(0);
        let current_reward = decayed_reward(TRANSACTION_BLOCK_REWARD, current_slot);
        let apy_bp = pool.calculate_apy_bp(slots_per_day, current_reward);
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
        "total_stakers": pool.positions.len(),
        "tiers": [
            { "id": 0, "name": "Flexible", "lock_days": 0, "multiplier": 1.0, "apy_percent": apy_percent },
            { "id": 1, "name": "30-Day Lock", "lock_days": 30, "multiplier": 1.5, "apy_percent": apy_percent * 1.5 },
            { "id": 2, "name": "90-Day Lock", "lock_days": 90, "multiplier": 2.0, "apy_percent": apy_percent * 2.0 },
            { "id": 3, "name": "365-Day Lock", "lock_days": 365, "multiplier": 3.0, "apy_percent": apy_percent * 3.0 },
        ],
        "cooldown_days": 7
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
        decayed_reward, ANNUAL_REWARD_DECAY_BPS, BOOTSTRAP_GRANT_AMOUNT,
        HEARTBEAT_BLOCK_REWARD, MIN_VALIDATOR_STAKE, SLOTS_PER_YEAR,
        TRANSACTION_BLOCK_REWARD,
    };

    let stake_pool_arc = state.stake_pool.as_ref().ok_or_else(|| RpcError {
        code: -32000,
        message: "Stake pool not available".to_string(),
    })?;
    let stake_pool = stake_pool_arc.read().await;
    let stats = stake_pool.get_stats();
    let active_count = stats.active_validators;
    let total_staked = stats.total_staked;

    // Apply reward decay for current slot
    let current_slot = state.state.get_last_slot().unwrap_or(0);
    let current_tx_reward = decayed_reward(TRANSACTION_BLOCK_REWARD, current_slot);
    let current_hb_reward = decayed_reward(HEARTBEAT_BLOCK_REWARD, current_slot);

    // Calculate effective APY using decayed reward
    let annual_tx_rewards = current_tx_reward as f64 * SLOTS_PER_YEAR as f64;
    let apy = if total_staked > 0 {
        (annual_tx_rewards / total_staked as f64) * 100.0
    } else {
        0.0
    };

    let decay_year = current_slot / SLOTS_PER_YEAR;

    // Load wallet pubkeys and balances for full transparency
    let wallet_info = |role: &str| -> serde_json::Value {
        let (pubkey_str, balance) = match state.state.get_wallet_pubkey(role) {
            Ok(Some(pk)) => {
                let bal = state.state.get_account(&pk)
                    .ok().flatten()
                    .map(|a| a.shells).unwrap_or(0);
                (pk.to_base58(), bal)
            }
            _ => ("unknown".to_string(), 0),
        };
        serde_json::json!({
            "pubkey": pubkey_str,
            "balance_shells": balance,
            "balance_molt": balance as f64 / 1_000_000_000.0,
        })
    };

    Ok(serde_json::json!({
        "currentMultiplier": 1.0,
        "priceOracleActive": true,
        "transactionBlockReward": current_tx_reward,
        "transactionBlockRewardBase": TRANSACTION_BLOCK_REWARD,
        "heartbeatBlockReward": current_hb_reward,
        "heartbeatBlockRewardBase": HEARTBEAT_BLOCK_REWARD,
        "annualRewardDecayBps": ANNUAL_REWARD_DECAY_BPS,
        "decayYear": decay_year,
        "slotsPerYear": SLOTS_PER_YEAR,
        "currentSlot": current_slot,
        "minValidatorStake": MIN_VALIDATOR_STAKE,
        "bootstrapGrantAmount": BOOTSTRAP_GRANT_AMOUNT,
        "totalStaked": total_staked,
        "totalSlashed": stats.total_slashed,
        "activeValidators": active_count,
        "unclaimedRewards": stats.total_unclaimed_rewards,
        "estimatedApy": format!("{:.2}", apy),
        "feeSplit": {
            "burn_pct": 40,
            "producer_pct": 30,
            "voters_pct": 10,
            "treasury_pct": 10,
            "community_pct": 10,
        },
        "genesisDistribution": {
            "validator_rewards_pct": 10,
            "community_treasury_pct": 25,
            "builder_grants_pct": 35,
            "founding_moltys_pct": 10,
            "ecosystem_partnerships_pct": 10,
            "reserve_pool_pct": 10,
        },
        "wallets": {
            "validator_rewards": wallet_info("validator_rewards"),
            "community_treasury": wallet_info("community_treasury"),
            "builder_grants": wallet_info("builder_grants"),
            "founding_moltys": wallet_info("founding_moltys"),
            "ecosystem_partnerships": wallet_info("ecosystem_partnerships"),
            "reserve_pool": wallet_info("reserve_pool"),
        },
        "note": "Oracle price feeds active: MOLT, wSOL, wETH via Binance WebSocket real-time feed"
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

    // F19.2a: Include decimals and ui_amount from symbol registry
    let registry = state
        .state
        .get_symbol_registry_by_program(&token_program)
        .ok()
        .flatten();

    let decimals = registry
        .as_ref()
        .and_then(|r| r.metadata.as_ref())
        .and_then(|m| m.get("decimals"))
        .and_then(|d| d.as_u64())
        .unwrap_or(9);

    let symbol = registry
        .as_ref()
        .map(|r| r.symbol.clone())
        .unwrap_or_else(|| "Unknown".to_string());

    let ui_amount = balance as f64 / 10_f64.powi(decimals as i32);

    Ok(serde_json::json!({
        "token_program": token_str,
        "holder": holder_str,
        "balance": balance,
        "decimals": decimals,
        "ui_amount": ui_amount,
        "symbol": symbol,
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
    let limit = arr.get(1).and_then(|v| v.as_u64()).unwrap_or(100).min(1000) as usize; // AUDIT-FIX 2.16

    let after_holder = arr.get(2).and_then(|v| v.as_str());
    let after_holder_pubkey = if let Some(ah_str) = after_holder {
        Some(Pubkey::from_base58(ah_str).map_err(|e| RpcError {
            code: -32602,
            message: format!("Invalid after_holder: {}", e),
        })?)
    } else {
        None
    };

    let token_program = Pubkey::from_base58(token_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid token_program: {}", e),
    })?;

    let holders = state
        .state
        .get_token_holders(&token_program, limit, after_holder_pubkey.as_ref())
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

/// Get token transfers: params = [token_program, limit?, before_slot?]
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
    let limit = arr.get(1).and_then(|v| v.as_u64()).unwrap_or(100).min(1000) as usize; // AUDIT-FIX 2.16

    let before_slot = arr.get(2).and_then(|v| v.as_u64());

    let token_program = Pubkey::from_base58(token_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid token_program: {}", e),
    })?;

    let transfers = state
        .state
        .get_token_transfers(&token_program, limit, before_slot)
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

/// Get contract events: params = [program_id, limit?, before_slot?]
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
    let limit = arr.get(1).and_then(|v| v.as_u64()).unwrap_or(100).min(1000) as usize; // AUDIT-FIX 2.16

    let before_slot = arr.get(2).and_then(|v| v.as_u64());

    let program = Pubkey::from_base58(program_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid program_id: {}", e),
    })?;

    let events = state
        .state
        .get_events_by_program(&program, limit, before_slot)
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
    // L3-01: Block in multi-validator mode — direct state writes bypass consensus.
    // Even on testnet, multiple validators would diverge on airdrop state.
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

    // AUDIT-FIX RPC-4: Per-address airdrop rate limiting (1 per 60 seconds)
    {
        let mut cooldowns = state
            .airdrop_cooldowns
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Prune stale entries every check
        cooldowns.retain(|_, t| t.elapsed().as_secs() < 120);
        if let Some(last) = cooldowns.get(address_str) {
            if last.elapsed().as_secs() < 60 {
                return Err(RpcError {
                    code: -32005,
                    message: format!(
                        "Airdrop rate limit: 1 per 60 seconds per address. Try again in {} seconds.",
                        60 - last.elapsed().as_secs()
                    ),
                });
            }
        }
        cooldowns.insert(address_str.to_string(), Instant::now());
    }

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

// ═══════════════════════════════════════════════════════════════════════════════
// PREDICTION MARKET JSON-RPC HANDLERS
// ═══════════════════════════════════════════════════════════════════════════════

const PREDICT_SYMBOL: &str = "PREDICT";
const PM_PRICE_SCALE: f64 = 1_000_000_000.0;

fn pm_u64(data: &[u8], off: usize) -> u64 {
    if data.len() < off + 8 {
        return 0;
    }
    u64::from_le_bytes(data[off..off + 8].try_into().unwrap_or([0; 8]))
}

/// getPredictionMarketStats — Platform stats
async fn handle_get_prediction_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "PREDICT")?;

    Ok(serde_json::json!({
        "total_markets": cf_stats_u64(state, "PREDICT", b"pm_market_count"),
        "open_markets": cf_stats_u64(state, "PREDICT", b"pm_open_markets"),
        "total_volume": cf_stats_u64(state, "PREDICT", b"pm_total_volume") as f64 / PM_PRICE_SCALE,
        "total_collateral": cf_stats_u64(state, "PREDICT", b"pm_total_collateral") as f64 / PM_PRICE_SCALE,
        "fees_collected": cf_stats_u64(state, "PREDICT", b"pm_fees_collected") as f64 / PM_PRICE_SCALE,
        "total_traders": cf_stats_u64(state, "PREDICT", b"pm_total_traders"),
        "paused": cf_stats_bool(state, "PREDICT", b"pm_paused"),
    }))
}

/// getPredictionMarkets — List markets with optional filter
async fn handle_get_prediction_markets(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let total = state.state.get_program_storage_u64(PREDICT_SYMBOL, b"pm_market_count");

    // Parse optional filter params
    let (cat_filter, status_filter, limit, offset) = match &params {
        Some(serde_json::Value::Object(obj)) => {
            let cat = obj
                .get("category")
                .and_then(|v| v.as_str())
                .map(String::from);
            let st = obj.get("status").and_then(|v| v.as_str()).map(String::from);
            let lim = obj.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
            let off = obj.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            (cat, st, lim.min(200), off)
        }
        _ => (None, None, 50, 0),
    };

    let cat_map = |c: u8| -> &'static str {
        match c {
            0 => "politics",
            1 => "sports",
            2 => "crypto",
            3 => "science",
            4 => "entertainment",
            5 => "economics",
            6 => "tech",
            _ => "custom",
        }
    };
    let st_map = |s: u8| -> &'static str {
        match s {
            0 => "pending",
            1 => "active",
            2 => "closed",
            3 => "resolving",
            4 => "resolved",
            5 => "disputed",
            6 => "voided",
            _ => "unknown",
        }
    };

    // PERF-NOTE (P10-VAL-05): Linear scan over all markets. Acceptable at
    // current scale. For >100K markets, consider contract-side filtered
    // counters or an off-chain index to avoid O(n) per query.
    let mut markets = Vec::new();
    for id in 1..=total {
        let key = format!("pm_m_{}", id);
        let data = match state.state.get_program_storage(PREDICT_SYMBOL, key.as_bytes()) {
            Some(d) if d.len() >= 192 => d,
            _ => continue,
        };
        let cat = cat_map(data[67]);
        let status = st_map(data[64]);

        if let Some(ref cf) = cat_filter {
            if cat != cf.as_str() {
                continue;
            }
        }
        if let Some(ref sf) = status_filter {
            if status != sf.as_str() {
                continue;
            }
        }

        let q_key = format!("pm_q_{}", id);
        let question = state.state
            .get_program_storage(PREDICT_SYMBOL, q_key.as_bytes())
            .and_then(|d| String::from_utf8(d).ok())
            .unwrap_or_default();

        markets.push(serde_json::json!({
            "id": pm_u64(&data, 0),
            "question": question,
            "category": cat,
            "status": status,
            "outcome_count": data[65],
            "total_collateral": pm_u64(&data, 68) as f64 / PM_PRICE_SCALE,
            "total_volume": pm_u64(&data, 76) as f64 / PM_PRICE_SCALE,
            "created_slot": pm_u64(&data, 40),
            "close_slot": pm_u64(&data, 48),
        }));
    }

    let result_total = markets.len();
    let page: Vec<_> = markets.into_iter().skip(offset).take(limit).collect();

    Ok(serde_json::json!({
        "markets": page,
        "total": result_total,
        "offset": offset,
        "limit": limit,
    }))
}

/// getPredictionMarket — Single market detail
async fn handle_get_prediction_market(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let market_id = match &params {
        Some(serde_json::Value::Array(arr)) if !arr.is_empty() => {
            arr[0].as_u64().ok_or(RpcError {
                code: -32602,
                message: "market_id must be u64".into(),
            })?
        }
        Some(serde_json::Value::Object(obj)) => obj
            .get("market_id")
            .or(obj.get("id"))
            .and_then(|v| v.as_u64())
            .ok_or(RpcError {
                code: -32602,
                message: "market_id required".into(),
            })?,
        _ => {
            return Err(RpcError {
                code: -32602,
                message: "Expected params: [market_id] or {market_id}".into(),
            })
        }
    };

    let key = format!("pm_m_{}", market_id);
    let data = state.state.get_program_storage(PREDICT_SYMBOL, key.as_bytes()).ok_or(RpcError {
        code: -32001,
        message: format!("Market {} not found", market_id),
    })?;

    if data.len() < 192 {
        return Err(RpcError {
            code: -32002,
            message: "Invalid market record".into(),
        });
    }

    let cat_map = |c: u8| -> &'static str {
        match c {
            0 => "politics",
            1 => "sports",
            2 => "crypto",
            3 => "science",
            4 => "entertainment",
            5 => "economics",
            6 => "tech",
            _ => "custom",
        }
    };
    let st_map = |s: u8| -> &'static str {
        match s {
            0 => "pending",
            1 => "active",
            2 => "closed",
            3 => "resolving",
            4 => "resolved",
            5 => "disputed",
            6 => "voided",
            _ => "unknown",
        }
    };

    let q_key = format!("pm_q_{}", market_id);
    let question = state.state
        .get_program_storage(PREDICT_SYMBOL, q_key.as_bytes())
        .and_then(|d| String::from_utf8(d).ok())
        .unwrap_or_default();

    let outcome_count = data[65];
    let mut outcomes = Vec::new();
    for oi in 0..outcome_count {
        let o_key = format!("pm_o_{}_{}", market_id, oi);
        let on_key = format!("pm_on_{}_{}", market_id, oi);

        let name = state.state
            .get_program_storage(PREDICT_SYMBOL, on_key.as_bytes())
            .and_then(|d| String::from_utf8(d).ok())
            .unwrap_or_else(|| if oi == 0 { "Yes".into() } else { "No".into() });

        let (pool_y, pool_n) = state.state
            .get_program_storage(PREDICT_SYMBOL, o_key.as_bytes())
            .map(|d| {
                if d.len() >= 16 {
                    (pm_u64(&d, 0), pm_u64(&d, 8))
                } else {
                    (0, 0)
                }
            })
            .unwrap_or((0, 0));

        let total = pool_y + pool_n;
        let price = if total > 0 {
            pool_n as f64 / total as f64
        } else {
            0.5
        };

        outcomes.push(serde_json::json!({
            "index": oi,
            "name": name,
            "pool_yes": pool_y as f64 / PM_PRICE_SCALE,
            "pool_no": pool_n as f64 / PM_PRICE_SCALE,
            "price": price,
        }));
    }

    let winning = data[66];

    Ok(serde_json::json!({
        "id": pm_u64(&data, 0),
        "creator": hex::encode(&data[8..40]),
        "question": question,
        "category": cat_map(data[67]),
        "status": st_map(data[64]),
        "outcome_count": outcome_count,
        "winning_outcome": if winning == 0xFF { serde_json::Value::Null } else { serde_json::json!(winning) },
        "total_collateral": pm_u64(&data, 68) as f64 / PM_PRICE_SCALE,
        "total_volume": pm_u64(&data, 76) as f64 / PM_PRICE_SCALE,
        "fees_collected": pm_u64(&data, 164) as f64 / PM_PRICE_SCALE,
        "created_slot": pm_u64(&data, 40),
        "close_slot": pm_u64(&data, 48),
        "resolve_slot": pm_u64(&data, 56),
        "outcomes": outcomes,
    }))
}

/// getPredictionPositions [address] — User positions
async fn handle_get_prediction_positions(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let address = match &params {
        Some(serde_json::Value::Array(arr)) if !arr.is_empty() => arr[0]
            .as_str()
            .ok_or(RpcError {
                code: -32602,
                message: "address must be string".into(),
            })?
            .to_string(),
        _ => {
            return Err(RpcError {
                code: -32602,
                message: "Expected params: [address]".into(),
            })
        }
    };

    let count_key = format!("pm_userc_{}", address);
    let count = state.state.get_program_storage_u64(PREDICT_SYMBOL, count_key.as_bytes());

    let mut positions = Vec::new();
    for idx in 0..count {
        let um_key = format!("pm_user_{}_{}", address, idx);
        let market_id = match state.state.get_program_storage(PREDICT_SYMBOL, um_key.as_bytes()) {
            Some(d) if d.len() >= 8 => pm_u64(&d, 0),
            _ => continue,
        };

        let mkt_key = format!("pm_m_{}", market_id);
        let mkt_data = match state.state.get_program_storage(PREDICT_SYMBOL, mkt_key.as_bytes()) {
            Some(d) if d.len() >= 192 => d,
            _ => continue,
        };
        let outcome_count = mkt_data[65];

        for oi in 0..outcome_count {
            let pos_key = format!("pm_p_{}_{}_{}", market_id, address, oi);
            if let Some(pd) = state.state.get_program_storage(PREDICT_SYMBOL, pos_key.as_bytes()) {
                if pd.len() >= 16 {
                    let shares = pm_u64(&pd, 0);
                    let cost = pm_u64(&pd, 8);
                    if shares > 0 {
                        positions.push(serde_json::json!({
                            "market_id": market_id,
                            "outcome": oi,
                            "shares": shares as f64 / PM_PRICE_SCALE,
                            "cost_basis": cost as f64 / PM_PRICE_SCALE,
                        }));
                    }
                }
            }
        }
    }

    Ok(serde_json::json!(positions))
}

/// getPredictionTraderStats [address] — Per-trader analytics
async fn handle_get_prediction_trader_stats(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let address = match &params {
        Some(serde_json::Value::Array(arr)) if !arr.is_empty() => arr[0]
            .as_str()
            .ok_or(RpcError {
                code: -32602,
                message: "address must be string".into(),
            })?
            .to_string(),
        _ => {
            return Err(RpcError {
                code: -32602,
                message: "Expected params: [address]".into(),
            })
        }
    };

    let key = format!("pm_ts_{}", address);
    match state.state.get_program_storage(PREDICT_SYMBOL, key.as_bytes()) {
        Some(d) if d.len() >= 24 => Ok(serde_json::json!({
            "address": address,
            "total_volume": pm_u64(&d, 0) as f64 / PM_PRICE_SCALE,
            "trade_count": pm_u64(&d, 8),
            "last_trade_slot": pm_u64(&d, 16),
        })),
        _ => Ok(serde_json::json!({
            "address": address,
            "total_volume": 0.0,
            "trade_count": 0,
            "last_trade_slot": 0,
        })),
    }
}

/// getPredictionLeaderboard — Top traders by volume
async fn handle_get_prediction_leaderboard(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let limit = match &params {
        Some(serde_json::Value::Object(obj)) => {
            obj.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize
        }
        _ => 20,
    };
    let limit = limit.min(50);

    let total_traders = state.state.get_program_storage_u64(PREDICT_SYMBOL, b"pm_total_traders");
    let scan_max = (total_traders as usize).min(500);

    let mut entries: Vec<(String, u64, u64)> = Vec::with_capacity(scan_max);
    for i in 0..scan_max as u64 {
        let lk = format!("pm_tl_{}", i);
        if let Some(addr_data) = state.state.get_program_storage(PREDICT_SYMBOL, lk.as_bytes()) {
            if addr_data.len() >= 32 {
                let addr_hex = hex::encode(&addr_data[..32]);
                let tk = format!("pm_ts_{}", addr_hex);
                if let Some(sd) = state.state.get_program_storage(PREDICT_SYMBOL, tk.as_bytes()) {
                    if sd.len() >= 24 {
                        let vol = pm_u64(&sd, 0);
                        let trades = pm_u64(&sd, 8);
                        entries.push((addr_hex, vol, trades));
                    }
                }
            }
        }
    }

    entries.sort_by(|a, b| b.1.cmp(&a.1));
    entries.truncate(limit);

    let leaders: Vec<serde_json::Value> = entries
        .into_iter()
        .enumerate()
        .map(|(i, (addr, vol, trades))| {
            serde_json::json!({
                "rank": i + 1,
                "address": addr,
                "total_volume": vol as f64 / PM_PRICE_SCALE,
                "trade_count": trades,
            })
        })
        .collect();

    Ok(serde_json::json!({
        "traders": leaders,
        "total_traders": total_traders,
    }))
}

/// getPredictionTrending — Active markets ranked by 24h volume
async fn handle_get_prediction_trending(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let total = state.state.get_program_storage_u64(PREDICT_SYMBOL, b"pm_market_count");

    let cat_map = |c: u8| -> &'static str {
        match c {
            0 => "politics",
            1 => "sports",
            2 => "crypto",
            3 => "science",
            4 => "entertainment",
            5 => "economics",
            6 => "tech",
            _ => "custom",
        }
    };

    let mut markets = Vec::new();
    for id in 1..=total {
        let key = format!("pm_m_{}", id);
        let data = match state.state.get_program_storage(PREDICT_SYMBOL, key.as_bytes()) {
            Some(d) if d.len() >= 192 => d,
            _ => continue,
        };
        if data[64] != 1 {
            continue;
        } // only active

        let q_key = format!("pm_q_{}", id);
        let question = state.state
            .get_program_storage(PREDICT_SYMBOL, q_key.as_bytes())
            .and_then(|d| String::from_utf8(d).ok())
            .unwrap_or_default();

        let vol24_key = format!("pm_mv24_{}", id);
        let vol24 = state.state.get_program_storage_u64(PREDICT_SYMBOL, vol24_key.as_bytes());

        let tc_key = format!("pm_mtc_{}", id);
        let traders = state.state.get_program_storage_u64(PREDICT_SYMBOL, tc_key.as_bytes());

        markets.push((
            id,
            question,
            cat_map(data[67]).to_string(),
            vol24,
            traders,
            pm_u64(&data, 76) as f64 / PM_PRICE_SCALE,
        ));
    }

    markets.sort_by(|a, b| b.3.cmp(&a.3));
    markets.truncate(10);

    let items: Vec<serde_json::Value> = markets
        .into_iter()
        .map(|(id, q, cat, vol24, tc, tv)| {
            serde_json::json!({
                "id": id,
                "question": q,
                "category": cat,
                "volume_24h": vol24 as f64 / PM_PRICE_SCALE,
                "unique_traders": tc,
                "total_volume": tv,
            })
        })
        .collect();

    Ok(serde_json::json!(items))
}

/// getPredictionMarketAnalytics [market_id] — Per-market analytics
async fn handle_get_prediction_market_analytics(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let market_id = match &params {
        Some(serde_json::Value::Array(arr)) if !arr.is_empty() => {
            arr[0].as_u64().ok_or(RpcError {
                code: -32602,
                message: "market_id must be u64".into(),
            })?
        }
        Some(serde_json::Value::Object(obj)) => obj
            .get("market_id")
            .or(obj.get("id"))
            .and_then(|v| v.as_u64())
            .ok_or(RpcError {
                code: -32602,
                message: "market_id required".into(),
            })?,
        _ => {
            return Err(RpcError {
                code: -32602,
                message: "Expected params: [market_id]".into(),
            })
        }
    };

    let tc_key = format!("pm_mtc_{}", market_id);
    let traders = state.state.get_program_storage_u64(PREDICT_SYMBOL, tc_key.as_bytes());
    let vol24_key = format!("pm_mv24_{}", market_id);
    let vol24 = state.state.get_program_storage_u64(PREDICT_SYMBOL, vol24_key.as_bytes());

    Ok(serde_json::json!({
        "market_id": market_id,
        "unique_traders": traders,
        "volume_24h": vol24 as f64 / PM_PRICE_SCALE,
    }))
}

// ═══════════════════════════════════════════════════════════════════════════════
// DEX & PLATFORM STATS JSON-RPC HANDLERS
// ═══════════════════════════════════════════════════════════════════════════════

/// Read a u64 from CF_CONTRACT_STORAGE (fast path, always current).
/// This is the authoritative source — post-block hooks (SL/TP engine,
/// analytics bridge, candle resets) write to CF only, NOT to embedded storage.
fn cf_stats_u64(state: &RpcState, symbol: &str, key: &[u8]) -> u64 {
    state.state.get_program_storage_u64(symbol, key)
}

/// Read a bool flag from CF_CONTRACT_STORAGE.
fn cf_stats_bool(state: &RpcState, symbol: &str, key: &[u8]) -> bool {
    state
        .state
        .get_program_storage(symbol, key)
        .map(|d| d.first().copied().unwrap_or(0) != 0)
        .unwrap_or(false)
}

/// Resolve a symbol to its program Pubkey.
fn resolve_symbol_pubkey(state: &RpcState, symbol: &str) -> Result<Pubkey, RpcError> {
    state
        .state
        .get_symbol_registry(symbol)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("DB error: {}", e),
        })?
        .map(|e| e.program)
        .ok_or_else(|| RpcError {
            code: -32001,
            message: format!("{} symbol not found", symbol),
        })
}

/// getDexCoreStats — DEX core exchange stats
async fn handle_get_dex_core_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "DEX")?; // verify symbol exists
    Ok(serde_json::json!({
        "pair_count": cf_stats_u64(state, "DEX", b"dex_pair_count"),
        "order_count": cf_stats_u64(state, "DEX", b"dex_order_count"),
        "trade_count": cf_stats_u64(state, "DEX", b"dex_trade_count"),
        "total_volume": cf_stats_u64(state, "DEX", b"dex_total_volume"),
        "fee_treasury": cf_stats_u64(state, "DEX", b"dex_fee_treasury"),
        "paused": cf_stats_bool(state, "DEX", b"dex_paused"),
    }))
}

/// getDexAmmStats — AMM pool stats
async fn handle_get_dex_amm_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "DEXAMM")?;
    Ok(serde_json::json!({
        "pool_count": cf_stats_u64(state, "DEXAMM", b"amm_pool_count"),
        "position_count": cf_stats_u64(state, "DEXAMM", b"amm_pos_count"),
        "swap_count": cf_stats_u64(state, "DEXAMM", b"amm_swap_count"),
        "total_volume": cf_stats_u64(state, "DEXAMM", b"amm_total_volume"),
        "total_fees": cf_stats_u64(state, "DEXAMM", b"amm_total_fees"),
        "paused": cf_stats_bool(state, "DEXAMM", b"amm_paused"),
    }))
}

/// getDexMarginStats — Margin trading stats
async fn handle_get_dex_margin_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "DEXMARGIN")?;
    let per_pair_max = cf_stats_u64(state, "DEXMARGIN", b"mrg_maxl_0");
    let max_leverage = if per_pair_max > 0 { per_pair_max } else { 100 };
    Ok(serde_json::json!({
        "position_count": cf_stats_u64(state, "DEXMARGIN", b"mrg_pos_count"),
        "total_volume": cf_stats_u64(state, "DEXMARGIN", b"mrg_total_volume"),
        "liquidation_count": cf_stats_u64(state, "DEXMARGIN", b"mrg_liq_count"),
        "total_pnl_profit": cf_stats_u64(state, "DEXMARGIN", b"mrg_pnl_profit"),
        "total_pnl_loss": cf_stats_u64(state, "DEXMARGIN", b"mrg_pnl_loss"),
        "insurance_fund": cf_stats_u64(state, "DEXMARGIN", b"mrg_insurance"),
        "max_leverage": max_leverage,
        "paused": cf_stats_bool(state, "DEXMARGIN", b"mrg_paused"),
    }))
}

/// getDexRewardsStats — Rewards program stats
async fn handle_get_dex_rewards_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "DEXREWARDS")?;
    Ok(serde_json::json!({
        "trade_count": cf_stats_u64(state, "DEXREWARDS", b"rew_trade_count"),
        "trader_count": cf_stats_u64(state, "DEXREWARDS", b"rew_trader_count"),
        "total_volume": cf_stats_u64(state, "DEXREWARDS", b"rew_total_volume"),
        "total_distributed": cf_stats_u64(state, "DEXREWARDS", b"rew_total_dist"),
        "epoch": cf_stats_u64(state, "DEXREWARDS", b"rew_epoch"),
        "paused": cf_stats_bool(state, "DEXREWARDS", b"rew_paused"),
    }))
}

/// getDexRouterStats — Router stats
async fn handle_get_dex_router_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "DEXROUTER")?;
    Ok(serde_json::json!({
        "route_count": cf_stats_u64(state, "DEXROUTER", b"rtr_route_count"),
        "swap_count": cf_stats_u64(state, "DEXROUTER", b"rtr_swap_count"),
        "total_volume": cf_stats_u64(state, "DEXROUTER", b"rtr_total_volume"),
        "paused": cf_stats_bool(state, "DEXROUTER", b"rtr_paused"),
    }))
}

/// getDexAnalyticsStats — Analytics global stats
async fn handle_get_dex_analytics_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    let program = resolve_symbol_pubkey(state, "ANALYTICS")?;

    // Aggregate candle counts and tracked pairs from CF_CONTRACT_STORAGE
    // Keys: "ana_cc_{pair_id}_{interval}" → u64 candle count
    // Keys: "ana_24h_{pair_id}" → 24h stats (presence means pair is tracked)
    let mut total_candles: u64 = 0;
    let mut tracked_pairs = std::collections::HashSet::new();

    // Iterate all storage entries for the ANALYTICS contract in CF
    let entries = state
        .state
        .get_contract_storage_entries(&program, 10_000, None)
        .unwrap_or_default();
    for (key, value) in &entries {
        if key.starts_with(b"ana_cc_") {
            if value.len() >= 8 {
                total_candles += u64::from_le_bytes([
                    value[0], value[1], value[2], value[3],
                    value[4], value[5], value[6], value[7],
                ]);
            }
            if let Some(pair_part) = key.get(7..) {
                if let Some(end) = pair_part.iter().position(|&b| b == b'_') {
                    tracked_pairs.insert(pair_part[..end].to_vec());
                }
            }
        } else if key.starts_with(b"ana_24h_") {
            if let Some(pair_part) = key.get(8..) {
                tracked_pairs.insert(pair_part.to_vec());
            }
        }
    }

    Ok(serde_json::json!({
        "record_count": cf_stats_u64(state, "ANALYTICS", b"ana_rec_count"),
        "trader_count": cf_stats_u64(state, "ANALYTICS", b"ana_trader_count"),
        "total_volume": cf_stats_u64(state, "ANALYTICS", b"ana_total_volume"),
        "total_candles": total_candles,
        "tracked_pairs": tracked_pairs.len(),
        "paused": cf_stats_bool(state, "ANALYTICS", b"ana_paused"),
    }))
}

/// getDexGovernanceStats — Governance stats
/// AUDIT-NOTE F-14: This endpoint is already O(1). Each counter (proposal_count, total_votes,
/// voter_count) is stored as a dedicated key in contract storage and read via a single
/// `get_storage()` call. There is no proposal scanning or iteration over votes. The
/// `stats_u64()` helper performs a point-lookup, making this endpoint suitable for frequent
/// polling without performance concerns.
async fn handle_get_dex_governance_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "DEXGOV")?;
    Ok(serde_json::json!({
        "proposal_count": cf_stats_u64(state, "DEXGOV", b"gov_prop_count"),
        "total_votes": cf_stats_u64(state, "DEXGOV", b"gov_total_votes"),
        "voter_count": cf_stats_u64(state, "DEXGOV", b"gov_voter_count"),
        "paused": cf_stats_bool(state, "DEXGOV", b"gov_paused"),
    }))
}

/// getMoltswapStats — Legacy swap stats
async fn handle_get_moltswap_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "MOLTSWAP")?;
    Ok(serde_json::json!({
        "swap_count": cf_stats_u64(state, "MOLTSWAP", b"ms_swap_count"),
        "volume_a": cf_stats_u64(state, "MOLTSWAP", b"ms_volume_a"),
        "volume_b": cf_stats_u64(state, "MOLTSWAP", b"ms_volume_b"),
        "paused": cf_stats_bool(state, "MOLTSWAP", b"ms_paused"),
    }))
}

/// getLobsterLendStats — Lending protocol stats
async fn handle_get_lobsterlend_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "LEND")?;
    Ok(serde_json::json!({
        "total_deposits": cf_stats_u64(state, "LEND", b"ll_total_deposits"),
        "total_borrows": cf_stats_u64(state, "LEND", b"ll_total_borrows"),
        "reserves": cf_stats_u64(state, "LEND", b"ll_reserves"),
        "deposit_count": cf_stats_u64(state, "LEND", b"ll_dep_count"),
        "borrow_count": cf_stats_u64(state, "LEND", b"ll_bor_count"),
        "liquidation_count": cf_stats_u64(state, "LEND", b"ll_liq_count"),
        "paused": cf_stats_bool(state, "LEND", b"ll_paused"),
    }))
}

/// getClawPayStats — Streaming payments stats
async fn handle_get_clawpay_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "CLAWPAY")?;
    Ok(serde_json::json!({
        "stream_count": cf_stats_u64(state, "CLAWPAY", b"stream_count"),
        "total_streamed": cf_stats_u64(state, "CLAWPAY", b"cp_total_streamed"),
        "total_withdrawn": cf_stats_u64(state, "CLAWPAY", b"cp_total_withdrawn"),
        "cancel_count": cf_stats_u64(state, "CLAWPAY", b"cp_cancel_count"),
        "paused": cf_stats_bool(state, "CLAWPAY", b"cp_paused"),
    }))
}

/// getBountyBoardStats — Bounty platform stats
async fn handle_get_bountyboard_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "BOUNTY")?;
    Ok(serde_json::json!({
        "bounty_count": cf_stats_u64(state, "BOUNTY", b"bounty_count"),
        "completed_count": cf_stats_u64(state, "BOUNTY", b"bb_completed_count"),
        "reward_volume": cf_stats_u64(state, "BOUNTY", b"bb_reward_volume"),
        "cancel_count": cf_stats_u64(state, "BOUNTY", b"bb_cancel_count"),
    }))
}

/// getComputeMarketStats — Compute marketplace stats
async fn handle_get_compute_market_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "COMPUTE")?;
    Ok(serde_json::json!({
        "job_count": cf_stats_u64(state, "COMPUTE", b"job_count"),
        "completed_count": cf_stats_u64(state, "COMPUTE", b"cm_completed_count"),
        "payment_volume": cf_stats_u64(state, "COMPUTE", b"cm_payment_volume"),
        "dispute_count": cf_stats_u64(state, "COMPUTE", b"cm_dispute_count"),
    }))
}

/// getReefStorageStats — Decentralized storage stats
async fn handle_get_reef_storage_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "REEF")?;
    Ok(serde_json::json!({
        "data_count": cf_stats_u64(state, "REEF", b"data_count"),
        "total_bytes": cf_stats_u64(state, "REEF", b"reef_total_bytes"),
        "challenge_count": cf_stats_u64(state, "REEF", b"reef_challenge_count"),
    }))
}

/// getMoltMarketStats — NFT marketplace stats
async fn handle_get_moltmarket_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "MARKET")?;
    Ok(serde_json::json!({
        "listing_count": cf_stats_u64(state, "MARKET", b"mm_listing_count"),
        "sale_count": cf_stats_u64(state, "MARKET", b"mm_sale_count"),
        "sale_volume": cf_stats_u64(state, "MARKET", b"mm_sale_volume"),
        "paused": cf_stats_bool(state, "MARKET", b"mm_paused"),
    }))
}

/// getMoltAuctionStats — Auction stats
async fn handle_get_moltauction_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "AUCTION")?;
    Ok(serde_json::json!({
        "auction_count": cf_stats_u64(state, "AUCTION", b"ma_auction_count"),
        "total_volume": cf_stats_u64(state, "AUCTION", b"ma_total_volume"),
        "total_sales": cf_stats_u64(state, "AUCTION", b"ma_total_sales"),
        "paused": cf_stats_bool(state, "AUCTION", b"ma_paused"),
    }))
}

/// getMoltPunksStats — NFT collection stats
async fn handle_get_moltpunks_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "PUNKS")?;
    Ok(serde_json::json!({
        "total_minted": cf_stats_u64(state, "PUNKS", b"total_minted"),
        "transfer_count": cf_stats_u64(state, "PUNKS", b"mp_transfer_count"),
        "burn_count": cf_stats_u64(state, "PUNKS", b"mp_burn_count"),
        "paused": cf_stats_bool(state, "PUNKS", b"mp_paused"),
    }))
}

/// getMusdStats — mUSD stablecoin stats
async fn handle_get_musd_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "MUSD")?;
    Ok(serde_json::json!({
        "supply": cf_stats_u64(state, "MUSD", b"musd_supply"),
        "total_minted": cf_stats_u64(state, "MUSD", b"musd_minted"),
        "total_burned": cf_stats_u64(state, "MUSD", b"musd_burned"),
        "mint_events": cf_stats_u64(state, "MUSD", b"musd_mint_evt"),
        "burn_events": cf_stats_u64(state, "MUSD", b"musd_burn_evt"),
        "transfer_count": cf_stats_u64(state, "MUSD", b"musd_xfer_cnt"),
        "attestation_count": cf_stats_u64(state, "MUSD", b"musd_att_count"),
        "reserve_attested": cf_stats_u64(state, "MUSD", b"musd_reserve_att"),
        "paused": cf_stats_bool(state, "MUSD", b"musd_paused"),
    }))
}

/// getWethStats — Wrapped ETH stats
async fn handle_get_weth_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "WETH")?;
    Ok(serde_json::json!({
        "supply": cf_stats_u64(state, "WETH", b"weth_supply"),
        "total_minted": cf_stats_u64(state, "WETH", b"weth_minted"),
        "total_burned": cf_stats_u64(state, "WETH", b"weth_burned"),
        "mint_events": cf_stats_u64(state, "WETH", b"weth_mint_evt"),
        "burn_events": cf_stats_u64(state, "WETH", b"weth_burn_evt"),
        "transfer_count": cf_stats_u64(state, "WETH", b"weth_xfer_cnt"),
        "attestation_count": cf_stats_u64(state, "WETH", b"weth_att_count"),
        "reserve_attested": cf_stats_u64(state, "WETH", b"weth_reserve_att"),
        "paused": cf_stats_bool(state, "WETH", b"weth_paused"),
    }))
}

/// getWsolStats — Wrapped SOL stats
async fn handle_get_wsol_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "WSOL")?;
    Ok(serde_json::json!({
        "supply": cf_stats_u64(state, "WSOL", b"wsol_supply"),
        "total_minted": cf_stats_u64(state, "WSOL", b"wsol_minted"),
        "total_burned": cf_stats_u64(state, "WSOL", b"wsol_burned"),
        "mint_events": cf_stats_u64(state, "WSOL", b"wsol_mint_evt"),
        "burn_events": cf_stats_u64(state, "WSOL", b"wsol_burn_evt"),
        "transfer_count": cf_stats_u64(state, "WSOL", b"wsol_xfer_cnt"),
        "attestation_count": cf_stats_u64(state, "WSOL", b"wsol_att_count"),
        "reserve_attested": cf_stats_u64(state, "WSOL", b"wsol_reserve_att"),
        "paused": cf_stats_bool(state, "WSOL", b"wsol_paused"),
    }))
}

/// getClawVaultStats — Yield vault stats
async fn handle_get_clawvault_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "CLAWVAULT")?;
    Ok(serde_json::json!({
        "total_assets": cf_stats_u64(state, "CLAWVAULT", b"cv_total_assets"),
        "total_shares": cf_stats_u64(state, "CLAWVAULT", b"cv_total_shares"),
        "strategy_count": cf_stats_u64(state, "CLAWVAULT", b"cv_strategy_count"),
        "total_earned": cf_stats_u64(state, "CLAWVAULT", b"cv_total_earned"),
        "fees_earned": cf_stats_u64(state, "CLAWVAULT", b"cv_fees_earned"),
        "protocol_fees": cf_stats_u64(state, "CLAWVAULT", b"cv_protocol_fees"),
        "paused": cf_stats_bool(state, "CLAWVAULT", b"cv_paused"),
    }))
}

/// getMoltBridgeStats — Cross-chain bridge stats
async fn handle_get_moltbridge_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "BRIDGE")?;
    Ok(serde_json::json!({
        "nonce": cf_stats_u64(state, "BRIDGE", b"bridge_nonce"),
        "validator_count": cf_stats_u64(state, "BRIDGE", b"bridge_validator_count"),
        "required_confirms": cf_stats_u64(state, "BRIDGE", b"bridge_required_confirms"),
        "locked_amount": cf_stats_u64(state, "BRIDGE", b"bridge_locked_amount"),
        "request_timeout": cf_stats_u64(state, "BRIDGE", b"bridge_request_timeout"),
        "paused": cf_stats_bool(state, "BRIDGE", b"mb_paused"),
    }))
}

/// getMoltDaoStats — DAO governance stats
async fn handle_get_moltdao_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "DAO")?;
    Ok(serde_json::json!({
        "proposal_count": cf_stats_u64(state, "DAO", b"proposal_count"),
        "min_proposal_threshold": cf_stats_u64(state, "DAO", b"min_proposal_threshold"),
        "paused": cf_stats_bool(state, "DAO", b"dao_paused"),
    }))
}

/// getMoltOracleStats — Oracle price feed stats
async fn handle_get_moltoracle_stats(state: &RpcState) -> Result<serde_json::Value, RpcError> {
    resolve_symbol_pubkey(state, "ORACLE")?;
    Ok(serde_json::json!({
        "queries": cf_stats_u64(state, "ORACLE", b"stats_queries"),
        "feeds": cf_stats_u64(state, "ORACLE", b"stats_feeds"),
        "attestations": cf_stats_u64(state, "ORACLE", b"stats_attestations"),
        "paused": cf_stats_bool(state, "ORACLE", b"oracle_paused"),
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

    // ── AUDIT-FIX A11-01: eth_gasPrice must return 1, not base_fee ──

    #[test]
    fn test_a11_01_gas_price_returns_one() {
        // Verify the source code of handle_eth_gas_price returns "0x1"
        // (This is a source-level check since integration tests are complex)
        let source = include_str!("lib.rs");
        let fn_start = source
            .find("async fn handle_eth_gas_price")
            .expect("fn not found");
        let fn_body = &source[fn_start..fn_start + 600];

        // Must contain "0x1" as the return value
        assert!(
            fn_body.contains("\"0x1\""),
            "REGRESSION A11-01: eth_gasPrice must return \"0x1\" (1 shell per gas unit), \
             not base_fee. MetaMask computes total = gasPrice × estimateGas."
        );
        // Must NOT contain "fee_config.base_fee" in the function body
        assert!(
            !fn_body.contains("fee_config.base_fee"),
            "REGRESSION A11-01: eth_gasPrice must NOT return fee_config.base_fee"
        );
    }

    // ── AUDIT-FIX A11-02: eth_getLogs must use Keccak-256 for topic hashing ──

    #[test]
    fn test_a11_02_get_logs_uses_keccak256() {
        // Verify the topic hashing code uses sha3::Keccak256, not sha2::Sha256
        let source = include_str!("lib.rs");
        let fn_start = source
            .find("async fn handle_eth_get_logs")
            .expect("fn not found");
        let fn_body = &source[fn_start..std::cmp::min(fn_start + 5000, source.len())];

        // Must use Keccak256
        assert!(
            fn_body.contains("Keccak256"),
            "REGRESSION A11-02: eth_getLogs topic hashing must use Keccak256, not SHA-256. \
             EVM tooling (Ethers.js, web3.py) uses keccak256 for event topic matching."
        );
        // Must NOT use sha2 or Sha256 for topic hashing
        assert!(
            !fn_body.contains("Sha256"),
            "REGRESSION A11-02: eth_getLogs must NOT use Sha256 for topic hashing"
        );
    }

    #[test]
    fn test_a11_02_keccak256_produces_correct_hash() {
        // Verify Keccak-256 of "Transfer(address,address,uint256)" matches known EVM value
        use sha3::{Digest, Keccak256};
        let mut hasher = Keccak256::new();
        hasher.update(b"Transfer(address,address,uint256)");
        let result = hasher.finalize();
        let hex_str = hex::encode(result);
        // Standard EVM Transfer topic: 0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef
        assert_eq!(
            hex_str,
            "ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef",
            "REGRESSION A11-02: Keccak-256 of ERC-20 Transfer event must match standard EVM topic hash"
        );
    }
}
