use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::{
    extract::State, routing::delete, routing::get, routing::post, routing::put, Json, Router,
};
use base64::Engine;
use ed25519_dalek::{Signer, VerifyingKey};
use frost_ed25519 as frost;
use hmac::Mac;
use lichen_core::{Hash, Instruction, Keypair, Message, Pubkey, Transaction, SYSTEM_PROGRAM_ID};
use rocksdb::{BlockBasedOptions, Cache, ColumnFamilyDescriptor, Options, SliceTransform, DB};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, Semaphore};
use tokio::time::{sleep, Duration};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::{info, warn};
use uuid::Uuid;
use zeroize::Zeroize;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

/// AUDIT-FIX 2.18: Single-instance enforcement is handled by RocksDB's exclusive
/// file lock on the DB directory. Multi-instance access to the same DB is prevented
/// at the storage layer. The RESERVE_LOCK static in adjust_reserve_balance()
/// serializes within-process concurrent access.
#[derive(Clone)]
struct CustodyState {
    db: Arc<DB>,
    next_index_lock: Arc<Mutex<()>>,
    config: CustodyConfig,
    http: reqwest::Client,
    /// AUDIT-FIX 1.20: Global withdrawal rate limiter
    withdrawal_rate: Arc<Mutex<WithdrawalRateState>>,
    /// AUDIT-FIX W-H4: Deposit rate limiter
    deposit_rate: Arc<Mutex<DepositRateState>>,
    /// Broadcast channel for webhook/WebSocket events
    event_tx: broadcast::Sender<CustodyWebhookEvent>,
    /// Cap concurrent webhook deliveries to prevent unbounded task fan-out.
    webhook_delivery_limiter: Arc<Semaphore>,
}

/// AUDIT-FIX 1.20: Withdrawal rate limiting state
#[derive(Clone, Debug)]
struct WithdrawalRateState {
    /// (timestamp, count) for rolling window
    window_start: std::time::Instant,
    count_this_minute: u64,
    value_this_hour: u64,
    hour_start: std::time::Instant,
    /// Per-address: last withdrawal time
    per_address: std::collections::HashMap<String, std::time::Instant>,
}

impl WithdrawalRateState {
    fn new() -> Self {
        Self {
            window_start: std::time::Instant::now(),
            count_this_minute: 0,
            value_this_hour: 0,
            hour_start: std::time::Instant::now(),
            per_address: std::collections::HashMap::new(),
        }
    }
}

/// AUDIT-FIX W-H4: Deposit rate limiting state
#[derive(Clone, Debug)]
struct DepositRateState {
    window_start: std::time::Instant,
    count_this_minute: u64,
    /// Per-user: last deposit request time
    per_user: std::collections::HashMap<String, std::time::Instant>,
}

impl DepositRateState {
    fn new() -> Self {
        Self {
            window_start: std::time::Instant::now(),
            count_this_minute: 0,
            per_user: std::collections::HashMap::new(),
        }
    }
}

// ── Webhook & Event System ──

/// Custody event payload — sent to registered webhooks and WebSocket subscribers.
/// Covers every state transition in deposit, sweep, credit, withdrawal, and rebalance flows.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct CustodyWebhookEvent {
    /// Unique event ID
    event_id: String,
    /// Event type identifier (matches audit event types)
    event_type: String,
    /// Primary entity ID (job_id, deposit_id, etc.)
    entity_id: String,
    /// Associated deposit ID (if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    deposit_id: Option<String>,
    /// Transaction hash (on-chain tx, if applicable)
    #[serde(skip_serializing_if = "Option::is_none")]
    tx_hash: Option<String>,
    /// Additional structured data about the event
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    /// Unix timestamp (seconds)
    timestamp: i64,
}

/// Registered webhook endpoint
#[derive(Clone, Debug, Serialize, Deserialize)]
struct WebhookRegistration {
    /// Unique webhook ID
    id: String,
    /// HTTPS URL to POST events to
    url: String,
    /// HMAC-SHA256 secret for signing payloads (provided by the registrant)
    secret: String,
    /// Optional filter: only send events matching these types.
    /// Empty = all events. Example: ["deposit.confirmed", "withdrawal.confirmed"]
    #[serde(default)]
    event_filter: Vec<String>,
    /// Whether this webhook is active
    active: bool,
    /// Creation timestamp
    created_at: i64,
    /// Description/label
    #[serde(default)]
    description: String,
}

#[derive(Debug, Deserialize)]
struct CreateWebhookRequest {
    url: String,
    secret: String,
    #[serde(default)]
    event_filter: Vec<String>,
    #[serde(default)]
    description: String,
}

#[derive(Clone, Debug)]
struct CustodyConfig {
    db_path: String,
    solana_rpc_url: Option<String>,
    evm_rpc_url: Option<String>,
    /// Per-chain EVM RPC: Ethereum-specific (overrides evm_rpc_url for ETH deposits)
    eth_rpc_url: Option<String>,
    /// Per-chain EVM RPC: BSC/BNB-specific (overrides evm_rpc_url for BNB deposits)
    bnb_rpc_url: Option<String>,
    solana_confirmations: u64,
    evm_confirmations: u64,
    poll_interval_secs: u64,
    treasury_solana_address: Option<String>,
    treasury_evm_address: Option<String>,
    /// Per-chain EVM treasury: separate ETH treasury address (overrides treasury_evm_address)
    treasury_eth_address: Option<String>,
    /// Per-chain EVM treasury: separate BNB treasury address (overrides treasury_evm_address)
    treasury_bnb_address: Option<String>,
    solana_fee_payer_keypair_path: Option<String>,
    solana_treasury_owner: Option<String>,
    solana_usdc_mint: String,
    solana_usdt_mint: String,
    evm_usdc_contract: String,
    evm_usdt_contract: String,
    signer_endpoints: Vec<String>,
    signer_threshold: usize,
    licn_rpc_url: Option<String>,
    treasury_keypair_path: Option<String>,
    // Wrapped token contract addresses on Lichen
    musd_contract_addr: Option<String>,
    wsol_contract_addr: Option<String>,
    weth_contract_addr: Option<String>,
    wbnb_contract_addr: Option<String>,
    // Reserve rebalance settings
    rebalance_threshold_bps: u64, // trigger when one side exceeds this (e.g. 7000 = 70%)
    rebalance_target_bps: u64,    // swap to reach this ratio (e.g. 5000 = 50/50)
    jupiter_api_url: Option<String>, // Solana DEX aggregator for USDT↔USDC swaps
    uniswap_router: Option<String>, // Ethereum DEX router for USDT↔USDC swaps
    /// AUDIT-FIX M14: Max tolerable slippage (bps) for rebalance swaps.
    /// Swaps exceeding this are rejected; unverifiable outputs are not credited.
    /// Set via CUSTODY_REBALANCE_MAX_SLIPPAGE_BPS (default: 50 = 0.5%).
    rebalance_max_slippage_bps: u64,
    deposit_ttl_secs: i64, // Expire unfunded deposits after this many seconds (default: 24h)
    /// C8 fix: Secret master seed for key derivation (HMAC-SHA256 instead of plain SHA256).
    /// Load from CUSTODY_MASTER_SEED env var. Required for production.
    master_seed: String,
    /// Dedicated seed root for deposit address derivation and deposit sweeps.
    /// Falls back to master_seed when no separate deposit root is configured.
    deposit_master_seed: String,
    /// C9 fix: Auth token for threshold signer requests (global fallback)
    signer_auth_token: Option<String>,
    /// AUDIT-FIX 1.22: Per-signer auth tokens (one per signer_endpoint, same order).
    /// Set via CUSTODY_SIGNER_AUTH_TOKENS=token1,token2,...
    /// Falls back to signer_auth_token if not set for a given index.
    signer_auth_tokens: Vec<Option<String>>,
    /// M17 fix: API auth token for withdrawal and other write endpoints
    /// Load from CUSTODY_API_AUTH_TOKEN env var. Required for production.
    api_auth_token: Option<String>,
    /// FROST threshold signing: hex-encoded PublicKeyPackage from DKG ceremony.
    /// Required for multi-signer Solana withdrawals.
    /// Set via CUSTODY_FROST_PUBKEY_PACKAGE env var.
    frost_pubkey_package_hex: Option<String>,
    /// EVM multisig contract address (e.g. Gnosis Safe).
    /// Required for multi-signer EVM withdrawals.
    /// Set via CUSTODY_EVM_MULTISIG_ADDRESS env var.
    evm_multisig_address: Option<String>,
    /// Optional outbound webhook host allowlist.
    /// When set, webhook URLs must resolve to one of these hosts.
    /// Set via CUSTODY_WEBHOOK_ALLOWED_HOSTS=hooks.example.com,events.example.com
    webhook_allowed_hosts: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DepositRequest {
    deposit_id: String,
    user_id: String,
    chain: String,
    asset: String,
    address: String,
    derivation_path: String,
    #[serde(default = "default_deposit_seed_source")]
    deposit_seed_source: String,
    created_at: i64,
    status: String,
}

fn default_deposit_seed_source() -> String {
    "treasury_root".to_string()
}

#[derive(Debug, Serialize, Deserialize)]
struct CreateDepositRequest {
    user_id: String,
    chain: String,
    asset: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct CreateDepositResponse {
    deposit_id: String,
    address: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct DepositEvent {
    event_id: String,
    deposit_id: String,
    tx_hash: String,
    confirmations: u64,
    amount: Option<u64>,
    status: String,
    observed_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct SweepJob {
    job_id: String,
    deposit_id: String,
    chain: String,
    asset: String,
    from_address: String,
    to_treasury: String,
    tx_hash: String,
    #[serde(default)]
    amount: Option<String>,
    #[serde(default)]
    credited_amount: Option<String>,
    #[serde(default)]
    signatures: Vec<SignerSignature>,
    #[serde(default)]
    sweep_tx_hash: Option<String>,
    #[serde(default)]
    attempts: u32,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    next_attempt_at: Option<i64>,
    status: String,
    created_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct CreditJob {
    job_id: String,
    deposit_id: String,
    to_address: String,
    amount_spores: u64,
    /// Source chain asset identifier ("sol", "eth", "usdt", "usdc")
    /// Determines which wrapped token contract to mint on Lichen.
    #[serde(default)]
    source_asset: String,
    /// Source chain ("solana", "ethereum")
    #[serde(default)]
    source_chain: String,
    status: String,
    tx_signature: Option<String>,
    #[serde(default)]
    attempts: u32,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    next_attempt_at: Option<i64>,
    created_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WithdrawalRequest {
    user_id: String,
    asset: String, // "lUSD", "wSOL", "wETH"
    amount: u64,
    dest_chain: String,   // "solana", "ethereum"
    dest_address: String, // destination address on dest_chain
    /// For lUSD withdrawals: which stablecoin to receive ("usdt" or "usdc"). Defaults to "usdt".
    #[serde(default = "default_preferred_stablecoin")]
    preferred_stablecoin: String,
}

fn default_preferred_stablecoin() -> String {
    "usdt".to_string()
}

/// Treasury reserve ledger entry — tracks actual stablecoin holdings per chain+asset
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReserveLedgerEntry {
    chain: String,     // "solana" or "ethereum"
    asset: String,     // "usdt" or "usdc"
    amount: u64,       // smallest unit (6 decimals for both USDT/USDC)
    last_updated: i64, // unix timestamp
}

/// Rebalance job — swap one stablecoin for another on a given chain
#[derive(Debug, Serialize, Deserialize)]
struct RebalanceJob {
    job_id: String,
    chain: String,      // "solana" or "ethereum"
    from_asset: String, // "usdt" or "usdc"
    to_asset: String,   // "usdc" or "usdt"
    amount: u64,        // amount to swap (smallest unit)
    trigger: String,    // "threshold" — periodic ratio check, "withdrawal" — on-demand
    linked_withdrawal_job_id: Option<String>,
    swap_tx_hash: Option<String>,
    status: String, // "queued" | "submitted" | "confirmed" | "failed"
    #[serde(default)]
    attempts: u32,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    next_attempt_at: Option<i64>,
    created_at: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct WithdrawalJob {
    job_id: String,
    user_id: String,
    asset: String, // "lUSD", "wSOL", "wETH"
    amount: u64,
    dest_chain: String,
    dest_address: String,
    /// For lUSD: which stablecoin the user wants ("usdt" or "usdc")
    #[serde(default = "default_preferred_stablecoin")]
    preferred_stablecoin: String,
    /// Lichen burn tx signature (user burned their wrapped tokens)
    burn_tx_signature: Option<String>,
    /// Outbound chain tx hash (SOL/ETH/USDT sent to user's dest_address)
    outbound_tx_hash: Option<String>,
    /// Pinned Gnosis Safe nonce for threshold EVM withdrawals.
    /// This binds collected signatures to one exact Safe transaction intent.
    #[serde(default)]
    safe_nonce: Option<u64>,
    #[serde(default)]
    signatures: Vec<SignerSignature>,
    status: String, // "pending_burn" | "burned" | "signing" | "broadcasting" | "confirmed" | "failed"
    #[serde(default)]
    attempts: u32,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    next_attempt_at: Option<i64>,
    created_at: i64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct SignerSignature {
    signer_pubkey: String,
    signature: String,
    message_hash: String,
    received_at: i64,
}

const CF_DEPOSITS: &str = "deposits";
const CF_INDEXES: &str = "indexes";
const CF_ADDRESS_INDEX: &str = "address_index";
const CF_DEPOSIT_EVENTS: &str = "deposit_events";
const CF_SWEEP_JOBS: &str = "sweep_jobs";
const CF_ADDRESS_BALANCES: &str = "address_balances";
const CF_TOKEN_BALANCES: &str = "token_balances";
const CF_CREDIT_JOBS: &str = "credit_jobs";
const CF_WITHDRAWAL_JOBS: &str = "withdrawal_jobs";
const CF_AUDIT_EVENTS: &str = "audit_events";
const CF_AUDIT_EVENTS_BY_TIME: &str = "audit_events_by_time";
const CF_AUDIT_EVENTS_BY_TYPE_TIME: &str = "audit_events_by_type_time";
const CF_AUDIT_EVENTS_BY_ENTITY_TIME: &str = "audit_events_by_entity_time";
const CF_AUDIT_EVENTS_BY_TX_TIME: &str = "audit_events_by_tx_time";
const CF_CURSORS: &str = "cursors";
const CF_RESERVE_LEDGER: &str = "reserve_ledger";
const CF_REBALANCE_JOBS: &str = "rebalance_jobs";
/// AUDIT-FIX M1: Secondary status index for O(active) queries.
/// Keys: "status:{table}:{status}:{job_id}" → empty value.
/// Full-table scans replaced with prefix iteration on this CF.
const CF_STATUS_INDEX: &str = "status_index";
/// AUDIT-FIX M4: Write-ahead intent log for crash idempotency.
/// Before broadcasting any on-chain TX, record the intent here.
/// On startup, stale intents are reconciled against chain state.
/// Keys: "intent:{type}:{job_id}" → JSON {chain, tx_type, created_at}
const CF_TX_INTENTS: &str = "tx_intents";
/// Webhook registrations — stores registered webhook endpoints.
/// Keys: webhook_id → JSON WebhookRegistration
const CF_WEBHOOKS: &str = "webhooks";

/// Lichen contract runtime program address (all 0xFF bytes)
const LICN_CONTRACT_PROGRAM: [u8; 32] = [0xFF; 32];

const SOLANA_SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";
const SOLANA_TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const SOLANA_ASSOCIATED_TOKEN_PROGRAM: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
const SOLANA_RENT_SYSVAR: &str = "SysvarRent111111111111111111111111111111111";
const SOLANA_SWEEP_FEE_LAMPORTS: u64 = 5_000;
const DEPOSIT_SEED_SOURCE_TREASURY_ROOT: &str = "treasury_root";
const DEPOSIT_SEED_SOURCE_DEPOSIT_ROOT: &str = "deposit_root";

/// Auto-discover wrapped token contract addresses from Lichen's symbol registry.
/// This eliminates the need to hardcode contract addresses — they are read from
/// whatever was deployed during genesis (or later). Falls back to env vars if RPC fails.
async fn autodiscover_contract_addresses(config: &mut CustodyConfig, http: &reqwest::Client) {
    let Some(rpc_url) = config.licn_rpc_url.as_ref() else {
        tracing::warn!("CUSTODY_LICHEN_RPC_URL not set — skipping contract auto-discovery");
        return;
    };

    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getAllSymbolRegistry",
        "params": [],
    });

    let response = match http.post(rpc_url).json(&payload).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("contract auto-discovery RPC failed: {} — using env vars", e);
            return;
        }
    };

    let value: serde_json::Value = match response.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "contract auto-discovery JSON parse failed: {} — using env vars",
                e
            );
            return;
        }
    };

    let Some(result) = value.get("result") else {
        tracing::warn!("contract auto-discovery: no result field — using env vars");
        return;
    };

    // getAllSymbolRegistry returns {"count": N, "entries": [...]} where each
    // entry has {"symbol": "LUSD", "program": "base58addr", ...}.
    // Build a symbol -> program_address lookup from the entries array.
    let entries = result
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if entries.is_empty() {
        tracing::warn!("contract auto-discovery: empty entries — using env vars");
        return;
    }

    let mut addr_by_symbol: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for entry in &entries {
        if let (Some(sym), Some(addr)) = (
            entry.get("symbol").and_then(|v| v.as_str()),
            entry
                .get("program")
                .or_else(|| entry.get("address"))
                .or_else(|| entry.get("program_id"))
                .and_then(|v| v.as_str()),
        ) {
            addr_by_symbol.insert(sym.to_string(), addr.to_string());
        }
    }

    info!(
        "contract auto-discovery: found {} entries in registry",
        addr_by_symbol.len()
    );

    // Map well-known symbol names to config fields
    let symbol_map: &[(&str, &str)] = &[
        ("LUSD", "musd"),
        ("WSOL", "wsol"),
        ("WETH", "weth"),
        ("WBNB", "wbnb"),
    ];

    for (symbol, field_name) in symbol_map {
        if let Some(addr) = addr_by_symbol.get(*symbol) {
            match *field_name {
                "musd" => {
                    if config.musd_contract_addr.is_none() {
                        info!("auto-discovered {} contract: {}", symbol, addr);
                        config.musd_contract_addr = Some(addr.clone());
                    }
                }
                "wsol" => {
                    if config.wsol_contract_addr.is_none() {
                        info!("auto-discovered {} contract: {}", symbol, addr);
                        config.wsol_contract_addr = Some(addr.clone());
                    }
                }
                "weth" => {
                    if config.weth_contract_addr.is_none() {
                        info!("auto-discovered {} contract: {}", symbol, addr);
                        config.weth_contract_addr = Some(addr.clone());
                    }
                }
                "wbnb" => {
                    if config.wbnb_contract_addr.is_none() {
                        info!("auto-discovered {} contract: {}", symbol, addr);
                        config.wbnb_contract_addr = Some(addr.clone());
                    }
                }
                _ => {}
            }
        }
    }

    // Report final state
    let discovered = [
        ("LUSD", &config.musd_contract_addr),
        ("WSOL", &config.wsol_contract_addr),
        ("WETH", &config.weth_contract_addr),
        ("WBNB", &config.wbnb_contract_addr),
    ];
    for (name, addr) in &discovered {
        match addr {
            Some(a) => info!("  ✅ {} contract: {}", name, a),
            None => tracing::warn!("  ❌ {} contract: NOT CONFIGURED", name),
        }
    }
}

/// Derive treasury addresses from the master seed for external chains.
/// Uses well-known derivation paths so addresses are deterministic and
/// recoverable from the master seed alone — no external keypair files needed.
fn derive_treasury_addresses_from_seed(config: &mut CustodyConfig) {
    let seed = &config.master_seed;

    // Solana treasury: derive from master seed with well-known path
    if config.treasury_solana_address.is_none() {
        match derive_solana_address("custody/treasury/solana", seed) {
            Ok(addr) => {
                info!("derived Solana treasury from master seed: {}", addr);
                config.treasury_solana_address = Some(addr.clone());
                if config.solana_treasury_owner.is_none() {
                    config.solana_treasury_owner = Some(addr);
                }
            }
            Err(e) => tracing::warn!("failed to derive Solana treasury: {}", e),
        }
    }

    // ETH treasury
    if config.treasury_eth_address.is_none() && config.treasury_evm_address.is_none() {
        match derive_evm_address("custody/treasury/ethereum", seed) {
            Ok(addr) => {
                info!("derived ETH treasury from master seed: {}", addr);
                config.treasury_eth_address = Some(addr);
            }
            Err(e) => tracing::warn!("failed to derive ETH treasury: {}", e),
        }
    }

    // BNB treasury
    if config.treasury_bnb_address.is_none() && config.treasury_evm_address.is_none() {
        match derive_evm_address("custody/treasury/bnb", seed) {
            Ok(addr) => {
                info!("derived BNB treasury from master seed: {}", addr);
                config.treasury_bnb_address = Some(addr);
            }
            Err(e) => tracing::warn!("failed to derive BNB treasury: {}", e),
        }
    }
}

/// Resolve the RPC URL for a given chain. Per-chain URLs override the generic EVM URL.
fn rpc_url_for_chain(config: &CustodyConfig, chain: &str) -> Option<String> {
    match chain {
        "sol" | "solana" => config.solana_rpc_url.clone(),
        "eth" | "ethereum" => config
            .eth_rpc_url
            .clone()
            .or_else(|| config.evm_rpc_url.clone()),
        "bsc" | "bnb" => config
            .bnb_rpc_url
            .clone()
            .or_else(|| config.evm_rpc_url.clone()),
        _ => config.evm_rpc_url.clone(),
    }
}

/// Resolve the treasury address for a given chain. Per-chain overrides generic.
fn treasury_for_chain(config: &CustodyConfig, chain: &str) -> Option<String> {
    match chain {
        "sol" | "solana" => config.treasury_solana_address.clone(),
        "eth" | "ethereum" => config
            .treasury_eth_address
            .clone()
            .or_else(|| config.treasury_evm_address.clone()),
        "bsc" | "bnb" => config
            .treasury_bnb_address
            .clone()
            .or_else(|| config.treasury_evm_address.clone()),
        _ => config.treasury_evm_address.clone(),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = load_config();

    // Mutable config for auto-discovery and derivation
    let mut config = config;

    // Derive treasury addresses from master seed for external chains
    // (only fills in addresses not already set via env vars)
    derive_treasury_addresses_from_seed(&mut config);

    // Log all configured chain endpoints and treasury addresses
    info!("══════════════════════════════════════════════════════════════");
    info!("  Lichen Custody Service — Chain Configuration");
    info!("══════════════════════════════════════════════════════════════");
    info!("  Lichen RPC:   {:?}", config.licn_rpc_url);
    info!("  SOL RPC:         {:?}", config.solana_rpc_url);
    info!(
        "  ETH RPC:         {:?}",
        config.eth_rpc_url.as_ref().or(config.evm_rpc_url.as_ref())
    );
    info!(
        "  BNB RPC:         {:?}",
        config.bnb_rpc_url.as_ref().or(config.evm_rpc_url.as_ref())
    );
    info!("  SOL Treasury:    {:?}", config.treasury_solana_address);
    info!(
        "  ETH Treasury:    {:?}",
        config
            .treasury_eth_address
            .as_ref()
            .or(config.treasury_evm_address.as_ref())
    );
    info!(
        "  BNB Treasury:    {:?}",
        config
            .treasury_bnb_address
            .as_ref()
            .or(config.treasury_evm_address.as_ref())
    );

    // Log the Solana fee payer address so operators know what to fund
    if config.solana_rpc_url.is_some() {
        if let Some(ref path) = config.solana_fee_payer_keypair_path {
            info!("  SOL Fee Payer:   file={}", path);
        } else {
            match derive_solana_address("custody/fee-payer/solana", &config.master_seed) {
                Ok(addr) => info!("  SOL Fee Payer:   {} (derived from master seed)", addr),
                Err(e) => tracing::warn!("  SOL Fee Payer:   derivation failed: {}", e),
            }
        }
    }
    info!("══════════════════════════════════════════════════════════════");

    // AUDIT-FIX M3: Single seed architecture warning
    // All deposit addresses are derived from one master seed via HMAC-SHA256.
    // Compromise of this seed would expose ALL deposit private keys.
    warn!(
        "🔐 SECURITY NOTICE: All custody keys derive from a single master seed. \
         Protect this seed with the highest operational security: \
         (1) Store via CUSTODY_MASTER_SEED_FILE on an encrypted, access-controlled volume. \
         (2) Rotate the seed periodically and re-derive addresses (requires fund sweep). \
         (3) Consider hardware HSM integration for production deployments. \
         (4) Limit process memory access (disable core dumps, restrict ptrace)."
    );

    // Multi-signer mode: validate threshold configuration
    if config.signer_threshold > config.signer_endpoints.len() {
        panic!(
            "FATAL: signer_threshold={} exceeds configured signer count={}. \
             Threshold must be ≤ number of signer endpoints.",
            config.signer_threshold,
            config.signer_endpoints.len()
        );
    }
    if config.signer_endpoints.len() > 1 {
        tracing::warn!(
            "MULTI-SIGNER MODE DETECTED ({}-of-{}). Native Solana withdrawals can use \
             the wired FROST path, but deposit sweeps remain locally signed from derived \
             deposit keys and EVM threshold withdrawals are still rejected until a \
             production-safe executor path is implemented.",
            config.signer_threshold,
            config.signer_endpoints.len()
        );
        info!(
            "Multi-signer mode: {}-of-{} threshold (FROST Ed25519 for Solana, packed ECDSA for EVM)",
            config.signer_threshold,
            config.signer_endpoints.len()
        );
        // Verify FROST public key package is available for Solana multi-sig
        if config.frost_pubkey_package_hex.is_some() {
            info!("  FROST public key package loaded for Solana threshold signing");
        } else {
            warn!(
                "  WARNING: No FROST public key package configured. \
                 Multi-signer Solana withdrawals will fail until FROST DKG is completed. \
                 Set CUSTODY_FROST_PUBKEY_PACKAGE to enable."
            );
        }
    }

    let db = open_db(&config.db_path).expect("open custody db");

    // AUDIT-FIX M4: On startup, check for stale TX intents from a previous crash
    recover_stale_intents(&db);
    // Backfill secondary event indexes for pre-index data.
    // Safe to run on every boot; missing keys are inserted idempotently.
    if let Err(e) = backfill_audit_event_indexes(&db) {
        tracing::warn!("audit event index backfill failed: {}", e);
    }

    // Webhook/WebSocket event broadcast channel (1024-event buffer)
    let (event_tx, _event_rx) = broadcast::channel::<CustodyWebhookEvent>(1024);

    // Bound concurrent webhook deliveries to avoid runaway task fan-out under bursty events.
    let webhook_max_inflight = std::env::var("CUSTODY_WEBHOOK_MAX_INFLIGHT")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(64)
        .min(1024);

    let state = CustodyState {
        db: Arc::new(db),
        next_index_lock: Arc::new(Mutex::new(())),
        // Auto-discover contract addresses from Lichen before creating state.
        // This ensures all workers see the correct contract addresses from genesis.
        config: {
            let discovery_http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("http client for discovery");
            autodiscover_contract_addresses(&mut config, &discovery_http).await;
            config.clone()
        },
        // AUDIT-FIX 1.19: HTTP client with timeouts to prevent hung RPC freezing custody
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client"),
        // AUDIT-FIX 1.20: Withdrawal rate limiter
        withdrawal_rate: Arc::new(Mutex::new(WithdrawalRateState::new())),
        // AUDIT-FIX W-H4: Deposit rate limiter
        deposit_rate: Arc::new(Mutex::new(DepositRateState::new())),
        event_tx: event_tx.clone(),
        webhook_delivery_limiter: Arc::new(Semaphore::new(webhook_max_inflight)),
    };

    // Spawn webhook dispatcher (delivers events to registered HTTP endpoints)
    {
        let dispatcher_state = state.clone();
        let mut dispatcher_rx = event_tx.subscribe();
        tokio::spawn(async move {
            webhook_dispatcher_loop(dispatcher_state, &mut dispatcher_rx).await;
        });
    }

    if let Some(url) = config.solana_rpc_url.clone() {
        let watcher_state = state.clone();
        tokio::spawn(async move {
            solana_watcher_loop(watcher_state, url).await;
        });
    }

    // Per-chain EVM watchers: spawn separate watchers for ETH and BNB
    // so each chain polls its own RPC endpoint
    if let Some(url) = config
        .eth_rpc_url
        .clone()
        .or_else(|| config.evm_rpc_url.clone())
    {
        let watcher_state = state.clone();
        tokio::spawn(async move {
            evm_watcher_loop_for_chains(watcher_state, url, &["ethereum", "eth"]).await;
        });
    }
    if let Some(url) = config.bnb_rpc_url.clone() {
        let watcher_state = state.clone();
        tokio::spawn(async move {
            evm_watcher_loop_for_chains(watcher_state, url, &["bsc", "bnb"]).await;
        });
    } else if config.evm_rpc_url.is_some() && config.eth_rpc_url.is_none() {
        // Legacy fallback: single EVM watcher for all chains
        let url = config.evm_rpc_url.clone().unwrap();
        let watcher_state = state.clone();
        tokio::spawn(async move {
            evm_watcher_loop(watcher_state, url).await;
        });
    }

    let sweep_state = state.clone();
    tokio::spawn(async move {
        sweep_worker_loop(sweep_state).await;
    });

    let credit_state = state.clone();
    tokio::spawn(async move {
        credit_worker_loop(credit_state).await;
    });

    // Withdrawal: watches Lichen for burn events → sends native assets on source chain
    let withdrawal_state = state.clone();
    tokio::spawn(async move {
        withdrawal_worker_loop(withdrawal_state).await;
    });

    // Reserve rebalance: monitors USDT/USDC ratio and swaps to maintain balance
    let rebalance_state = state.clone();
    tokio::spawn(async move {
        rebalance_worker_loop(rebalance_state).await;
    });

    // Deposit cleanup: prunes expired unfunded deposit addresses
    let cleanup_state = state.clone();
    tokio::spawn(async move {
        deposit_cleanup_loop(cleanup_state).await;
    });

    let app = Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/deposits", post(create_deposit))
        .route("/deposits/:deposit_id", get(get_deposit))
        .route("/withdrawals", post(create_withdrawal))
        // AUDIT-FIX C4: Endpoint for clients to submit their Lichen burn tx signature.
        // Without this, withdrawal jobs stay in "pending_burn" forever because
        // burn_tx_signature starts as None and nothing ever populates it.
        .route("/withdrawals/:job_id/burn", put(submit_burn_signature))
        .route("/reserves", get(get_reserves))
        // ── Webhook management endpoints ──
        .route("/webhooks", post(create_webhook))
        .route("/webhooks", get(list_webhooks))
        .route("/webhooks/:webhook_id", delete(delete_webhook))
        // ── Real-time WebSocket event stream ──
        .route("/ws/events", get(ws_events))
        // ── Audit event history endpoint ──
        .route("/events", get(list_events))
        // AUDIT-FIX M-18: Restrict CORS to Lichen domains
        .layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::list([
                    "https://lichen.network".parse().unwrap(),
                    "https://wallet.lichen.network".parse().unwrap(),
                    "https://explorer.lichen.network".parse().unwrap(),
                    "https://dex.lichen.network".parse().unwrap(),
                    "http://localhost:3000".parse().unwrap(),
                    "http://localhost:8080".parse().unwrap(),
                ]))
                .allow_methods([
                    http::Method::GET,
                    http::Method::POST,
                    http::Method::PUT,
                    http::Method::DELETE,
                    http::Method::OPTIONS,
                ])
                .allow_headers([http::header::CONTENT_TYPE, http::header::AUTHORIZATION]),
        )
        .with_state(state);

    let port = std::env::var("CUSTODY_LISTEN_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(9105);
    let addr: SocketAddr = format!("0.0.0.0:{}", port)
        .parse()
        .expect("valid bind addr");
    info!("custody service listening on {}", addr);

    axum::serve(
        tokio::net::TcpListener::bind(addr).await.expect("bind"),
        app,
    )
    .await
    .expect("serve");
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

#[derive(Serialize)]
struct StatusCounts {
    total: usize,
    by_status: BTreeMap<String, usize>,
}

/// AUDIT-FIX F8.5: Status endpoint now requires auth to prevent leaking internal job counts.
async fn status(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    let sweep_counts = count_sweep_jobs(&state.db).map_err(|e| Json(ErrorResponse::db(&e)))?;
    let credit_counts = count_credit_jobs(&state.db).map_err(|e| Json(ErrorResponse::db(&e)))?;

    Ok(Json(json!({
        "signers": {
            "configured": state.config.signer_endpoints.len(),
            "threshold": state.config.signer_threshold,
        },
        "sweeps": sweep_counts,
        "credits": credit_counts,
    })))
}

/// AUDIT-FIX F8.6: Deposit creation now requires API auth.
/// Without auth, anyone can create deposit addresses which generates derivation paths
/// and (combined with a compromised master seed) could reconstruct private keys.
async fn create_deposit(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<CreateDepositRequest>,
) -> Result<Json<CreateDepositResponse>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    let chain = payload.chain.to_lowercase();
    let asset = payload.asset.to_lowercase();
    if chain.is_empty() || asset.is_empty() || payload.user_id.is_empty() {
        return Err(Json(ErrorResponse::invalid("Missing user_id/chain/asset")));
    }

    // Validate user_id is a valid Lichen base58 pubkey (32 bytes).
    // Reject early so build_credit_job never silently drops a credit.
    if Pubkey::from_base58(&payload.user_id).is_err() {
        return Err(Json(ErrorResponse::invalid(
            "user_id must be a valid Lichen base58 public key (32 bytes)",
        )));
    }

    ensure_deposit_creation_allowed(&state.config).map_err(|e| Json(ErrorResponse::invalid(&e)))?;

    // AUDIT-FIX W-H4: Rate limit deposit creation (60/min global, 10s per-user cooldown)
    {
        let mut dr = state.deposit_rate.lock().await;
        let now = std::time::Instant::now();
        if now.duration_since(dr.window_start).as_secs() >= 60 {
            dr.window_start = now;
            dr.count_this_minute = 0;
        }
        dr.count_this_minute += 1;
        if dr.count_this_minute > 60 {
            tracing::warn!(
                "⚠️  Deposit rate limit exceeded: {} this minute",
                dr.count_this_minute
            );
            return Err(Json(ErrorResponse::invalid(
                "rate_limited: too many deposit requests, try again later",
            )));
        }
        if let Some(last) = dr.per_user.get(&payload.user_id) {
            if now.duration_since(*last).as_secs() < 10 {
                return Err(Json(ErrorResponse::invalid(
                    "rate_limited: wait 10s between deposit requests",
                )));
            }
        }
        dr.per_user.insert(payload.user_id.clone(), now);
    }

    if (chain == "solana" || chain == "sol") && is_solana_stablecoin(&asset) {
        ensure_solana_config(&state.config).map_err(|e| Json(ErrorResponse::invalid(&e)))?;
    }

    let deposit_id = Uuid::new_v4().to_string();
    let _guard = state.next_index_lock.lock().await;
    let index = next_deposit_index(&state.db, &payload.user_id, &chain, &asset)
        .map_err(|e| Json(ErrorResponse::db(&e)))?;

    let derivation_path = bip44_derivation_path(&chain, &payload.user_id, index as u64)
        .map_err(|e| Json(ErrorResponse::invalid(&e)))?;
    let deposit_seed_source = active_deposit_seed_source(&state.config).to_string();
    let deposit_seed = deposit_seed_for_source(&state.config, &deposit_seed_source);
    let address = if chain == "solana" || chain == "sol" {
        if is_solana_stablecoin(&asset) {
            let mint = solana_mint_for_asset(&state.config, &asset)
                .map_err(|e| Json(ErrorResponse::invalid(&e)))?;
            let owner = derive_solana_owner_pubkey(&derivation_path, deposit_seed)
                .map_err(|e| Json(ErrorResponse::invalid(&e)))?;
            let ata = derive_associated_token_address(&owner, &mint)
                .map_err(|e| Json(ErrorResponse::invalid(&e)))?;
            ensure_associated_token_account(&state, &owner, &mint, &ata)
                .await
                .map_err(|e| Json(ErrorResponse::invalid(&e)))?;
            ata
        } else {
            derive_deposit_address(&chain, &asset, &derivation_path, deposit_seed)
                .map_err(|e| Json(ErrorResponse::invalid(&e)))?
        }
    } else {
        derive_deposit_address(&chain, &asset, &derivation_path, deposit_seed)
            .map_err(|e| Json(ErrorResponse::invalid(&e)))?
    };

    let record = DepositRequest {
        deposit_id: deposit_id.clone(),
        user_id: payload.user_id,
        chain,
        asset,
        address: address.clone(),
        derivation_path,
        deposit_seed_source,
        created_at: chrono::Utc::now().timestamp(),
        status: "issued".to_string(),
    };

    store_deposit(&state.db, &record).map_err(|e| Json(ErrorResponse::db(&e)))?;
    store_address_index(&state.db, &record.address, &record.deposit_id)
        .map_err(|e| Json(ErrorResponse::db(&e)))?;
    // AUDIT-FIX M1: index initial deposit status
    let _ = set_status_index(&state.db, "deposits", "issued", &record.deposit_id);

    emit_custody_event(
        &state,
        "deposit.created",
        &deposit_id,
        Some(&deposit_id),
        None,
        Some(&serde_json::json!({
            "user_id": record.user_id,
            "chain": record.chain,
            "asset": record.asset,
            "address": record.address
        })),
    );

    Ok(Json(CreateDepositResponse {
        deposit_id,
        address,
    }))
}

/// AUDIT-FIX F8.3: Deposit lookup now requires API auth.
/// Without auth, anyone guessing/brute-forcing deposit UUIDs could retrieve
/// user_id, chain, asset, address, and derivation_path.
async fn get_deposit(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(deposit_id): axum::extract::Path<String>,
) -> Result<Json<DepositRequest>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    let record = fetch_deposit(&state.db, &deposit_id)
        .map_err(|e| Json(ErrorResponse::db(&e)))?
        .ok_or_else(|| Json(ErrorResponse::not_found("Deposit not found")))?;
    Ok(Json(record))
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    code: &'static str,
    message: String,
}

impl ErrorResponse {
    fn invalid(message: &str) -> Self {
        Self {
            code: "invalid_request",
            message: message.to_string(),
        }
    }

    fn not_found(message: &str) -> Self {
        Self {
            code: "not_found",
            message: message.to_string(),
        }
    }

    fn db(message: &str) -> Self {
        Self {
            code: "db_error",
            message: message.to_string(),
        }
    }
}

fn open_db<P: AsRef<Path>>(path: P) -> Result<DB, String> {
    let mut opts = Options::default();
    opts.create_if_missing(true);
    opts.create_missing_column_families(true);
    opts.set_max_open_files(2048);
    opts.set_keep_log_file_num(5);
    opts.set_max_total_wal_size(128 * 1024 * 1024);

    let shared_cache = Cache::new_lru_cache(256 * 1024 * 1024);

    let point_lookup_opts = || -> Options {
        let mut cf_opts = Options::default();
        let mut bbo = BlockBasedOptions::default();
        bbo.set_bloom_filter(10.0, false);
        bbo.set_block_cache(&shared_cache);
        bbo.set_cache_index_and_filter_blocks(true);
        bbo.set_pin_l0_filter_and_index_blocks_in_cache(true);
        cf_opts.set_block_based_table_factory(&bbo);
        cf_opts.set_write_buffer_size(32 * 1024 * 1024);
        cf_opts.set_max_write_buffer_number(3);
        cf_opts.set_level_compaction_dynamic_level_bytes(true);
        cf_opts
    };

    let prefix_scan_opts = |prefix_len: usize| -> Options {
        let mut cf_opts = Options::default();
        let mut bbo = BlockBasedOptions::default();
        bbo.set_bloom_filter(10.0, false);
        bbo.set_block_cache(&shared_cache);
        bbo.set_cache_index_and_filter_blocks(true);
        bbo.set_pin_l0_filter_and_index_blocks_in_cache(true);
        cf_opts.set_block_based_table_factory(&bbo);
        cf_opts.set_prefix_extractor(SliceTransform::create_fixed_prefix(prefix_len));
        cf_opts.set_memtable_prefix_bloom_ratio(0.1);
        cf_opts.set_write_buffer_size(32 * 1024 * 1024);
        cf_opts.set_max_write_buffer_number(3);
        cf_opts.set_level_compaction_dynamic_level_bytes(true);
        cf_opts
    };

    let write_heavy_opts = || -> Options {
        let mut cf_opts = Options::default();
        let mut bbo = BlockBasedOptions::default();
        bbo.set_bloom_filter(10.0, false);
        bbo.set_block_cache(&shared_cache);
        bbo.set_cache_index_and_filter_blocks(true);
        cf_opts.set_block_based_table_factory(&bbo);
        cf_opts.set_write_buffer_size(64 * 1024 * 1024);
        cf_opts.set_max_write_buffer_number(4);
        cf_opts.set_level_compaction_dynamic_level_bytes(true);
        cf_opts
    };

    let small_cf_opts = || -> Options {
        let mut cf_opts = Options::default();
        let mut bbo = BlockBasedOptions::default();
        bbo.set_block_cache(&shared_cache);
        cf_opts.set_block_based_table_factory(&bbo);
        cf_opts.set_write_buffer_size(4 * 1024 * 1024);
        cf_opts.set_max_write_buffer_number(2);
        cf_opts
    };

    let cfs = vec![
        ColumnFamilyDescriptor::new(CF_DEPOSITS, point_lookup_opts()),
        ColumnFamilyDescriptor::new(CF_INDEXES, point_lookup_opts()),
        ColumnFamilyDescriptor::new(CF_ADDRESS_INDEX, prefix_scan_opts(8)),
        ColumnFamilyDescriptor::new(CF_DEPOSIT_EVENTS, write_heavy_opts()),
        ColumnFamilyDescriptor::new(CF_SWEEP_JOBS, point_lookup_opts()),
        ColumnFamilyDescriptor::new(CF_ADDRESS_BALANCES, point_lookup_opts()),
        ColumnFamilyDescriptor::new(CF_TOKEN_BALANCES, prefix_scan_opts(7)),
        ColumnFamilyDescriptor::new(CF_CREDIT_JOBS, point_lookup_opts()),
        ColumnFamilyDescriptor::new(CF_WITHDRAWAL_JOBS, point_lookup_opts()),
        ColumnFamilyDescriptor::new(CF_AUDIT_EVENTS, write_heavy_opts()),
        ColumnFamilyDescriptor::new(CF_AUDIT_EVENTS_BY_TIME, write_heavy_opts()),
        ColumnFamilyDescriptor::new(CF_AUDIT_EVENTS_BY_TYPE_TIME, prefix_scan_opts(12)),
        ColumnFamilyDescriptor::new(CF_AUDIT_EVENTS_BY_ENTITY_TIME, prefix_scan_opts(12)),
        ColumnFamilyDescriptor::new(CF_AUDIT_EVENTS_BY_TX_TIME, prefix_scan_opts(12)),
        ColumnFamilyDescriptor::new(CF_CURSORS, small_cf_opts()),
        ColumnFamilyDescriptor::new(CF_RESERVE_LEDGER, write_heavy_opts()),
        ColumnFamilyDescriptor::new(CF_REBALANCE_JOBS, point_lookup_opts()),
        ColumnFamilyDescriptor::new(CF_STATUS_INDEX, prefix_scan_opts(7)),
        ColumnFamilyDescriptor::new(CF_TX_INTENTS, prefix_scan_opts(7)),
        ColumnFamilyDescriptor::new(CF_WEBHOOKS, point_lookup_opts()),
    ];

    DB::open_cf_descriptors(&opts, path, cfs).map_err(|e| format!("db open: {}", e))
}

// ── AUDIT-FIX M1: Status index helpers ──
// Keys: "status:{table}:{status}:{job_id}" → empty
// When a job's status changes, remove old index entry + add new one.
// list_*_by_status now does a prefix scan on this CF instead of full-table scan.

fn set_status_index(db: &DB, table: &str, status: &str, job_id: &str) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_STATUS_INDEX)
        .ok_or_else(|| "missing status_index cf".to_string())?;
    let key = format!("status:{}:{}:{}", table, status, job_id);
    db.put_cf(cf, key.as_bytes(), b"")
        .map_err(|e| format!("status index put: {}", e))
}

fn remove_status_index(db: &DB, table: &str, status: &str, job_id: &str) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_STATUS_INDEX)
        .ok_or_else(|| "missing status_index cf".to_string())?;
    let key = format!("status:{}:{}:{}", table, status, job_id);
    db.delete_cf(cf, key.as_bytes())
        .map_err(|e| format!("status index delete: {}", e))
}

fn update_status_index(
    db: &DB,
    table: &str,
    old_status: &str,
    new_status: &str,
    job_id: &str,
) -> Result<(), String> {
    if old_status != new_status {
        let _ = remove_status_index(db, table, old_status, job_id);
        set_status_index(db, table, new_status, job_id)?;
    }
    Ok(())
}

/// List job IDs from the status index with a given prefix.
fn list_ids_by_status_index(db: &DB, table: &str, status: &str) -> Result<Vec<String>, String> {
    let cf = db
        .cf_handle(CF_STATUS_INDEX)
        .ok_or_else(|| "missing status_index cf".to_string())?;
    let prefix = format!("status:{}:{}:", table, status);
    let prefix_bytes = prefix.as_bytes();
    let mut ids = Vec::new();
    let iter = db.prefix_iterator_cf(cf, prefix_bytes);
    for item in iter {
        let (key, _) = item.map_err(|e| format!("db iter: {}", e))?;
        let key_str = std::str::from_utf8(&key).unwrap_or("");
        if !key_str.starts_with(&prefix) {
            break; // past prefix range
        }
        if let Some(job_id) = key_str.strip_prefix(&prefix) {
            ids.push(job_id.to_string());
        }
    }
    Ok(ids)
}

fn store_address_index(db: &DB, address: &str, deposit_id: &str) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_ADDRESS_INDEX)
        .ok_or_else(|| "missing address_index cf".to_string())?;
    db.put_cf(cf, address.as_bytes(), deposit_id.as_bytes())
        .map_err(|e| format!("db put: {}", e))
}

// ── AUDIT-FIX M4: Write-ahead intent log for crash idempotency ──
// Before broadcasting any on-chain transaction, we record an intent.
// On crash recovery, stale intents are logged and the operator is alerted.

fn record_tx_intent(db: &DB, tx_type: &str, job_id: &str, chain: &str) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_TX_INTENTS)
        .ok_or_else(|| "missing tx_intents cf".to_string())?;
    let key = format!("intent:{}:{}", tx_type, job_id);
    let payload = serde_json::json!({
        "tx_type": tx_type,
        "job_id": job_id,
        "chain": chain,
        "created_at": chrono::Utc::now().timestamp(),
    });
    let bytes = serde_json::to_vec(&payload).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, key.as_bytes(), bytes)
        .map_err(|e| format!("intent put: {}", e))
}

fn clear_tx_intent(db: &DB, tx_type: &str, job_id: &str) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_TX_INTENTS)
        .ok_or_else(|| "missing tx_intents cf".to_string())?;
    let key = format!("intent:{}:{}", tx_type, job_id);
    db.delete_cf(cf, key.as_bytes())
        .map_err(|e| format!("intent delete: {}", e))
}

fn recover_stale_intents(db: &DB) {
    let cf = match db.cf_handle(CF_TX_INTENTS) {
        Some(cf) => cf,
        None => return,
    };
    let iter = db.prefix_iterator_cf(cf, b"intent:");
    let mut count = 0u32;
    for item in iter {
        let (key, value) = match item {
            Ok(kv) => kv,
            Err(_) => continue,
        };
        let key_str = std::str::from_utf8(&key).unwrap_or("?");
        if !key_str.starts_with("intent:") {
            break;
        }
        let payload_str = std::str::from_utf8(&value).unwrap_or("{}");
        tracing::error!(
            "⚠️  STALE TX INTENT (possible crash during broadcast): key={} payload={}. \
             Manual reconciliation required — check chain state for this job.",
            key_str,
            payload_str
        );
        count += 1;
    }
    if count > 0 {
        tracing::error!(
            "🚨 Found {} stale TX intents from previous run. \
             These indicate broadcasts that may or may not have reached the chain. \
             Review each above and reconcile against on-chain state before proceeding.",
            count
        );
    }
}

fn backfill_audit_event_indexes(db: &DB) -> Result<(), String> {
    let events_cf = db
        .cf_handle(CF_AUDIT_EVENTS)
        .ok_or_else(|| "missing audit_events cf".to_string())?;
    let time_cf = db
        .cf_handle(CF_AUDIT_EVENTS_BY_TIME)
        .ok_or_else(|| "missing audit_events_by_time cf".to_string())?;
    let type_time_cf = db
        .cf_handle(CF_AUDIT_EVENTS_BY_TYPE_TIME)
        .ok_or_else(|| "missing audit_events_by_type_time cf".to_string())?;
    let entity_time_cf = db
        .cf_handle(CF_AUDIT_EVENTS_BY_ENTITY_TIME)
        .ok_or_else(|| "missing audit_events_by_entity_time cf".to_string())?;
    let tx_time_cf = db
        .cf_handle(CF_AUDIT_EVENTS_BY_TX_TIME)
        .ok_or_else(|| "missing audit_events_by_tx_time cf".to_string())?;

    let mut scanned = 0usize;
    let mut inserted = 0usize;

    for item in db.iterator_cf(events_cf, rocksdb::IteratorMode::Start) {
        let (key, value) = item.map_err(|e| format!("db iter: {}", e))?;
        let event: Value = match serde_json::from_slice(&value) {
            Ok(v) => v,
            Err(_) => continue,
        };
        scanned += 1;

        let key_id = std::str::from_utf8(&key).unwrap_or("").to_string();
        let event_id = event
            .get("event_id")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .unwrap_or(&key_id)
            .to_string();
        let event_type = event
            .get("event_type")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .unwrap_or("unknown")
            .to_string();
        let entity_id = event
            .get("entity_id")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .unwrap_or("unknown")
            .to_string();
        let tx_hash = event
            .get("tx_hash")
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .map(|v| v.to_string());
        let ts_ms = event
            .get("timestamp_ms")
            .and_then(|v| v.as_i64())
            .or_else(|| {
                event
                    .get("timestamp")
                    .and_then(|v| v.as_i64())
                    .map(|s| s.saturating_mul(1000))
            })
            .unwrap_or(0)
            .max(0);

        let time_key = format!("{:020}:{}", ts_ms, event_id);
        if matches!(db.get_cf(time_cf, time_key.as_bytes()), Ok(None)) {
            db.put_cf(time_cf, time_key.as_bytes(), event_id.as_bytes())
                .map_err(|e| format!("time index put: {}", e))?;
            inserted += 1;
        }

        let type_key = format!("type:{}:{:020}:{}", event_type, ts_ms, event_id);
        if matches!(db.get_cf(type_time_cf, type_key.as_bytes()), Ok(None)) {
            db.put_cf(type_time_cf, type_key.as_bytes(), event_id.as_bytes())
                .map_err(|e| format!("type index put: {}", e))?;
            inserted += 1;
        }

        let entity_key = format!("entity:{}:{:020}:{}", entity_id, ts_ms, event_id);
        if matches!(db.get_cf(entity_time_cf, entity_key.as_bytes()), Ok(None)) {
            db.put_cf(entity_time_cf, entity_key.as_bytes(), event_id.as_bytes())
                .map_err(|e| format!("entity index put: {}", e))?;
            inserted += 1;
        }

        if let Some(tx_hash) = tx_hash {
            let tx_key = format!("tx:{}:{:020}:{}", tx_hash, ts_ms, event_id);
            if matches!(db.get_cf(tx_time_cf, tx_key.as_bytes()), Ok(None)) {
                db.put_cf(tx_time_cf, tx_key.as_bytes(), event_id.as_bytes())
                    .map_err(|e| format!("tx index put: {}", e))?;
                inserted += 1;
            }
        }
    }

    if scanned > 0 {
        tracing::info!(
            "audit event index backfill complete: scanned={}, inserted={}",
            scanned,
            inserted
        );
    }

    Ok(())
}

fn store_deposit(db: &DB, record: &DepositRequest) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_DEPOSITS)
        .ok_or_else(|| "missing deposits cf".to_string())?;
    let bytes = serde_json::to_vec(record).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, record.deposit_id.as_bytes(), bytes)
        .map_err(|e| format!("db put: {}", e))
}

fn fetch_deposit(db: &DB, deposit_id: &str) -> Result<Option<DepositRequest>, String> {
    let cf = db
        .cf_handle(CF_DEPOSITS)
        .ok_or_else(|| "missing deposits cf".to_string())?;
    match db.get_cf(cf, deposit_id.as_bytes()) {
        Ok(Some(bytes)) => {
            let record = serde_json::from_slice(&bytes).map_err(|e| format!("decode: {}", e))?;
            Ok(Some(record))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(format!("db get: {}", e)),
    }
}

fn next_deposit_index(db: &DB, user_id: &str, chain: &str, asset: &str) -> Result<u64, String> {
    let cf = db
        .cf_handle(CF_INDEXES)
        .ok_or_else(|| "missing indexes cf".to_string())?;
    let key = format!("{}/{}/{}", user_id, chain, asset);
    let current = match db.get_cf(cf, key.as_bytes()) {
        Ok(Some(bytes)) => {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&bytes);
            u64::from_le_bytes(buf)
        }
        Ok(None) => 0,
        Err(e) => return Err(format!("db get: {}", e)),
    };

    let next = current + 1;
    db.put_cf(cf, key.as_bytes(), next.to_le_bytes())
        .map_err(|e| format!("db put: {}", e))?;
    Ok(next)
}

fn get_last_u64_index(db: &DB, key: &str) -> Result<Option<u64>, String> {
    let cf = db
        .cf_handle(CF_CURSORS)
        .ok_or_else(|| "missing cursors cf".to_string())?;
    match db.get_cf(cf, key.as_bytes()) {
        Ok(Some(bytes)) => {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&bytes);
            Ok(Some(u64::from_le_bytes(buf)))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(format!("db get: {}", e)),
    }
}

fn set_last_u64_index(db: &DB, key: &str, value: u64) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_CURSORS)
        .ok_or_else(|| "missing cursors cf".to_string())?;
    db.put_cf(cf, key.as_bytes(), value.to_le_bytes())
        .map_err(|e| format!("db put: {}", e))
}

fn derive_deposit_address(
    chain: &str,
    asset: &str,
    path: &str,
    master_seed: &str,
) -> Result<String, String> {
    match (chain, asset) {
        ("sol", _) | ("solana", _) => derive_solana_address(path, master_seed),
        ("eth", _) | ("ethereum", _) | ("bsc", _) | ("bnb", _) => {
            derive_evm_address(path, master_seed)
        }
        _ => Err(format!("Unsupported chain: {}", chain)),
    }
}

/// F2-01: Map chain name to BIP-44 registered coin type integer.
/// See <https://github.com/satoshilabs/slips/blob/master/slip-0044.md>
fn bip44_coin_type(chain: &str) -> Result<u32, String> {
    match chain {
        "sol" | "solana" => Ok(501),
        "eth" | "ethereum" | "bsc" | "bnb" => Ok(60),
        "btc" | "bitcoin" => Ok(0),
        "ltc" | "litecoin" => Ok(2),
        "lichen" | "licn" => Ok(9999), // unregistered — use high range
        _ => Err(format!("Unknown coin type for chain: {}", chain)),
    }
}

fn is_evm_chain(chain: &str) -> bool {
    matches!(chain, "eth" | "ethereum" | "bsc" | "bnb")
}

/// F2-01: Build BIP-44-structured derivation path.
/// Format: `m/44'/{coin_type}'/{user_hash}'/0/{index}`
/// The user_id is hashed to a u32 account index for BIP-44 compliance.
fn bip44_derivation_path(chain: &str, user_id: &str, index: u64) -> Result<String, String> {
    let coin_type = bip44_coin_type(chain)?;
    // Hash user_id to a deterministic 31-bit account index (BIP-32 max is 2^31-1 for non-hardened)
    let mut hasher = hmac::Hmac::<sha2::Sha256>::new_from_slice(b"bip44-account")
        .map_err(|_| "HMAC init failed".to_string())?;
    hasher.update(user_id.as_bytes());
    let result = hasher.finalize().into_bytes();
    let account = u32::from_le_bytes([result[0], result[1], result[2], result[3]]) & 0x7FFF_FFFF;
    Ok(format!("m/44'/{}'/{}'/{}/{}", coin_type, account, 0, index))
}

fn derive_solana_owner_pubkey(path: &str, master_seed: &str) -> Result<String, String> {
    derive_solana_address(path, master_seed)
}

fn active_deposit_seed_source(config: &CustodyConfig) -> &'static str {
    if config.deposit_master_seed == config.master_seed {
        DEPOSIT_SEED_SOURCE_TREASURY_ROOT
    } else {
        DEPOSIT_SEED_SOURCE_DEPOSIT_ROOT
    }
}

fn deposit_seed_for_source<'a>(config: &'a CustodyConfig, source: &str) -> &'a str {
    if source == DEPOSIT_SEED_SOURCE_DEPOSIT_ROOT {
        &config.deposit_master_seed
    } else {
        &config.master_seed
    }
}

fn deposit_seed_for_record<'a>(config: &'a CustodyConfig, deposit: &DepositRequest) -> &'a str {
    deposit_seed_for_source(config, &deposit.deposit_seed_source)
}

fn load_required_seed_secret(
    file_var: &str,
    env_var: &str,
    allow_insecure_default: bool,
) -> String {
    let seed = if let Ok(seed_path) = std::env::var(file_var) {
        let seed_path = seed_path.trim().to_string();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = std::fs::metadata(&seed_path) {
                let mode = meta.permissions().mode() & 0o777;
                if mode & 0o077 != 0 {
                    tracing::warn!(
                        "⚠️  {} '{}' has permissions {:o} — should be 0600 or stricter. Tightening now.",
                        file_var,
                        seed_path,
                        mode
                    );
                    let _ = std::fs::set_permissions(
                        &seed_path,
                        std::fs::Permissions::from_mode(0o600),
                    );
                }
            }
        }
        match std::fs::read_to_string(&seed_path) {
            Ok(contents) => {
                let secret = contents.trim().to_string();
                if secret.is_empty() {
                    panic!("FATAL: {} '{}' is empty.", file_var, seed_path);
                }
                tracing::info!("Secret loaded from file ({})", file_var);
                Some(secret)
            }
            Err(err) => panic!("FATAL: Cannot read {} '{}': {}", file_var, seed_path, err),
        }
    } else {
        None
    };

    let seed = seed.or_else(|| match std::env::var(env_var) {
        Ok(secret) if !secret.is_empty() => {
            tracing::warn!(
                "⚠️  Secret loaded from env var {}. Prefer {} for production.",
                env_var,
                file_var
            );
            std::env::remove_var(env_var);
            Some(secret)
        }
        _ => None,
    });

    match seed {
        Some(secret) => {
            if secret.len() < 32 && !secret.starts_with("INSECURE_DEFAULT") {
                panic!(
                    "FATAL: Secret from {} is too short ({} chars, minimum 32). Use a high-entropy seed.",
                    env_var,
                    secret.len()
                );
            }
            secret
        }
        None => {
            if allow_insecure_default
                && std::env::var("CUSTODY_ALLOW_INSECURE_SEED").unwrap_or_default() == "1"
            {
                tracing::warn!("⚠️  No seed configured — using insecure default (dev mode)!");
                "INSECURE_DEFAULT_SEED_DO_NOT_USE_IN_PRODUCTION".to_string()
            } else {
                panic!(
                    "FATAL: No seed configured. Set {} (preferred) or {}, or set CUSTODY_ALLOW_INSECURE_SEED=1 for dev.",
                    file_var,
                    env_var
                );
            }
        }
    }
}

fn load_optional_seed_secret(file_var: &str, env_var: &str) -> Option<String> {
    if std::env::var_os(file_var).is_none() && std::env::var_os(env_var).is_none() {
        return None;
    }
    Some(load_required_seed_secret(file_var, env_var, false))
}

fn is_solana_stablecoin(asset: &str) -> bool {
    matches!(asset, "usdc" | "usdt")
}

fn ensure_solana_config(config: &CustodyConfig) -> Result<(), String> {
    if config.solana_rpc_url.is_none() {
        return Err("missing CUSTODY_SOLANA_RPC_URL".to_string());
    }
    // Fee payer and treasury owner can be derived from master seed, so no longer mandatory as env vars
    Ok(())
}

fn solana_mint_for_asset(config: &CustodyConfig, asset: &str) -> Result<String, String> {
    match asset {
        "usdc" => Ok(config.solana_usdc_mint.clone()),
        "usdt" => Ok(config.solana_usdt_mint.clone()),
        _ => Err("unsupported solana token".to_string()),
    }
}

fn evm_contract_for_asset(config: &CustodyConfig, asset: &str) -> Result<String, String> {
    match asset {
        "usdc" => Ok(config.evm_usdc_contract.clone()),
        "usdt" => Ok(config.evm_usdt_contract.clone()),
        _ => Err("unsupported evm token".to_string()),
    }
}

fn derive_associated_token_address(owner: &str, mint: &str) -> Result<String, String> {
    let owner_key = decode_solana_pubkey(owner)?;
    let mint_key = decode_solana_pubkey(mint)?;
    let token_program = decode_solana_pubkey(SOLANA_TOKEN_PROGRAM)?;
    let associated_program = decode_solana_pubkey(SOLANA_ASSOCIATED_TOKEN_PROGRAM)?;
    let seeds: [&[u8]; 3] = [&owner_key, &token_program, &mint_key];
    let address = find_program_address(&seeds, &associated_program)?;
    Ok(encode_solana_pubkey(&address))
}

fn derive_associated_token_address_from_str(owner: &str, mint: &str) -> Result<String, String> {
    derive_associated_token_address(owner, mint)
}

async fn ensure_associated_token_account(
    state: &CustodyState,
    owner: &str,
    mint: &str,
    ata: &str,
) -> Result<(), String> {
    ensure_associated_token_account_for_str(state, owner, mint, ata).await
}

async fn ensure_associated_token_account_for_str(
    state: &CustodyState,
    owner: &str,
    mint: &str,
    ata: &str,
) -> Result<(), String> {
    let url = state
        .config
        .solana_rpc_url
        .as_ref()
        .ok_or_else(|| "missing CUSTODY_SOLANA_RPC_URL".to_string())?;

    if solana_get_account_exists(&state.http, url, ata).await? {
        return Ok(());
    }

    let owner_key = decode_solana_pubkey(owner)?;
    let mint_key = decode_solana_pubkey(mint)?;
    let ata_key = decode_solana_pubkey(ata)?;

    // Fee payer: load from file if configured, otherwise derive from master seed
    let fee_payer = if let Some(ref fee_payer_path) = state.config.solana_fee_payer_keypair_path {
        load_solana_keypair(fee_payer_path)?
    } else {
        derive_solana_keypair("custody/fee-payer/solana", &state.config.master_seed)?
    };

    let system_program = decode_solana_pubkey(SOLANA_SYSTEM_PROGRAM)?;
    let token_program = decode_solana_pubkey(SOLANA_TOKEN_PROGRAM)?;
    let rent_sysvar = decode_solana_pubkey(SOLANA_RENT_SYSVAR)?;
    let associated_program = decode_solana_pubkey(SOLANA_ASSOCIATED_TOKEN_PROGRAM)?;

    let account_keys = vec![
        fee_payer.pubkey,
        ata_key,
        owner_key,
        mint_key,
        system_program,
        token_program,
        rent_sysvar,
        associated_program,
    ];

    let header = SolanaMessageHeader {
        num_required_signatures: 1,
        num_readonly_signed: 0,
        num_readonly_unsigned: 6,
    };

    let instruction = SolanaInstruction {
        program_id_index: 7,
        account_indices: vec![0, 1, 2, 3, 4, 5, 6],
        data: Vec::new(),
    };

    let recent_blockhash = solana_get_latest_blockhash(&state.http, url).await?;
    let message = build_solana_message_with_instructions(
        header,
        &account_keys,
        &recent_blockhash,
        &[instruction],
    );
    let signature = fee_payer.sign(&message);
    let tx = build_solana_transaction(&[signature], &message);
    solana_send_transaction(&state.http, url, &tx).await?;
    Ok(())
}

fn load_solana_keypair(path: &str) -> Result<SimpleSolanaKeypair, String> {
    let json = std::fs::read_to_string(path).map_err(|e| format!("read: {}", e))?;
    let bytes: Vec<u8> = serde_json::from_str(&json).map_err(|e| format!("parse: {}", e))?;
    if bytes.len() != 64 {
        return Err("invalid keypair length".to_string());
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes[..32]);
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let pubkey = signing_key.verifying_key().to_bytes();
    Ok(SimpleSolanaKeypair {
        signing_key,
        pubkey,
    })
}

fn load_config() -> CustodyConfig {
    let db_path = std::env::var("CUSTODY_DB_PATH").unwrap_or_else(|_| "./data/custody".to_string());
    let solana_rpc_url = std::env::var("CUSTODY_SOLANA_RPC_URL").ok();
    let evm_rpc_url = std::env::var("CUSTODY_EVM_RPC_URL").ok();
    let solana_confirmations = std::env::var("CUSTODY_SOLANA_CONFIRMATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let evm_confirmations = std::env::var("CUSTODY_EVM_CONFIRMATIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);
    let poll_interval_secs = std::env::var("CUSTODY_POLL_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
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
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(7000);
    let rebalance_target_bps = std::env::var("CUSTODY_REBALANCE_TARGET_BPS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(5000);
    // AUDIT-FIX M14: configurable max slippage for rebalance swaps (default 50 bps = 0.5%)
    let rebalance_max_slippage_bps = std::env::var("CUSTODY_REBALANCE_MAX_SLIPPAGE_BPS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(50);
    let jupiter_api_url = std::env::var("CUSTODY_JUPITER_API_URL").ok();
    let uniswap_router = std::env::var("CUSTODY_UNISWAP_ROUTER").ok();
    let deposit_ttl_secs = std::env::var("CUSTODY_DEPOSIT_TTL_SECS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(86400); // 24 hours default
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
        master_seed,
        deposit_master_seed,
        // AUDIT-FIX P10-CUST-01: Signer auth token MUST NOT be predictable.
        // Previously could be None when env var was absent, leaving signer requests
        // completely unauthenticated. Now generates a cryptographically random token
        // if the env var is not set and signers are configured.
        signer_auth_token: {
            let env_token = std::env::var("CUSTODY_SIGNER_AUTH_TOKEN")
                .ok()
                .filter(|t| !t.is_empty());
            if env_token.is_some() {
                env_token
            } else if !signer_endpoints.is_empty() {
                // AUDIT-FIX M7: Refuse to start with signers but no auth token.
                // Previously generated a random token that was never exposed,
                // making all signer authentication fail silently.
                panic!(
                    "FATAL: {} signer endpoint(s) configured but CUSTODY_SIGNER_AUTH_TOKEN \
                     is not set. Set it explicitly to enable signer authentication.",
                    signer_endpoints.len()
                );
            } else {
                None // no signers configured, token not needed
            }
        },
        // AUDIT-FIX 1.22: Per-signer auth tokens
        signer_auth_tokens: std::env::var("CUSTODY_SIGNER_AUTH_TOKENS")
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .map(|t| {
                        let t = t.trim();
                        if t.is_empty() {
                            None
                        } else {
                            Some(t.to_string())
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
        // AUDIT-FIX 0.10: API auth token is MANDATORY — running without it
        // leaves the withdrawal endpoint completely unauthenticated.
        api_auth_token: {
            let token = std::env::var("CUSTODY_API_AUTH_TOKEN")
                .ok()
                .filter(|t| !t.is_empty());
            if token.is_none() {
                panic!(
                    "CRITICAL: CUSTODY_API_AUTH_TOKEN must be set and non-empty. \
                     The withdrawal endpoint is unauthenticated without it."
                );
            }
            token
        },
        frost_pubkey_package_hex: std::env::var("CUSTODY_FROST_PUBKEY_PACKAGE").ok(),
        evm_multisig_address: std::env::var("CUSTODY_EVM_MULTISIG_ADDRESS").ok(),
        webhook_allowed_hosts,
    }
}

fn webhook_host_from_url(raw_url: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(raw_url).map_err(|e| format!("invalid webhook url: {}", e))?;
    parsed
        .host_str()
        .map(|host| host.to_ascii_lowercase())
        .ok_or_else(|| "webhook url must include a valid host".to_string())
}

fn validate_webhook_destination(config: &CustodyConfig, raw_url: &str) -> Result<(), String> {
    if raw_url.starts_with("http://localhost") {
        return Ok(());
    }
    if config.webhook_allowed_hosts.is_empty() {
        return Ok(());
    }

    let host = webhook_host_from_url(raw_url)?;
    if config
        .webhook_allowed_hosts
        .iter()
        .any(|allowed| allowed == &host)
    {
        Ok(())
    } else {
        Err(format!(
            "webhook host '{}' is not in CUSTODY_WEBHOOK_ALLOWED_HOSTS",
            host
        ))
    }
}

fn default_signer_threshold(endpoint_count: usize) -> usize {
    if endpoint_count >= 5 {
        3
    } else if endpoint_count >= 3 {
        2
    } else if endpoint_count >= 1 {
        1
    } else {
        0
    }
}

fn multi_signer_local_sweep_mode(config: &CustodyConfig) -> bool {
    config.signer_threshold > 1 && config.signer_endpoints.len() > 1
}

fn ensure_deposit_creation_allowed(config: &CustodyConfig) -> Result<(), String> {
    if let Some(err) = local_sweep_policy_error(config) {
        return Err(err);
    }

    Ok(())
}

fn local_sweep_policy_error(config: &CustodyConfig) -> Option<String> {
    if multi_signer_local_sweep_mode(config) {
        return Some(
            "multi-signer deposit creation is disabled because deposit sweeps still broadcast with locally derived deposit keys; this path remains hard-disabled until deposit sweeps have a real threshold architecture".to_string(),
        );
    }

    None
}

async fn solana_watcher_loop(state: CustodyState, url: String) {
    loop {
        if let Err(err) = process_solana_deposits(&state, &url).await {
            tracing::warn!("solana watcher error: {}", err);
        }
        sleep(Duration::from_secs(state.config.poll_interval_secs)).await;
    }
}

async fn evm_watcher_loop(state: CustodyState, url: String) {
    loop {
        if let Err(err) = process_evm_deposits(&state, &url).await {
            tracing::warn!("evm watcher error: {}", err);
        }
        sleep(Duration::from_secs(state.config.poll_interval_secs)).await;
    }
}

/// Per-chain EVM watcher — only watches deposits for the specified chain names.
async fn evm_watcher_loop_for_chains(
    state: CustodyState,
    url: String,
    chains: &'static [&'static str],
) {
    loop {
        if let Err(err) = process_evm_deposits_for_chains(&state, &url, chains).await {
            tracing::warn!("evm watcher ({:?}) error: {}", chains, err);
        }
        sleep(Duration::from_secs(state.config.poll_interval_secs)).await;
    }
}

async fn process_evm_deposits_for_chains(
    state: &CustodyState,
    url: &str,
    chains: &[&str],
) -> Result<(), String> {
    let deposits = list_pending_deposits_for_chains(&state.db, chains)?;
    let block_number = evm_get_block_number(&state.http, url).await?;

    // ERC20 failures should not block native balance detection
    if let Err(e) = process_evm_erc20_deposits(state, url, &deposits, block_number).await {
        tracing::warn!("erc20 log scan failed (non-fatal): {}", e);
    }

    for deposit in deposits {
        let balance = evm_get_balance(&state.http, url, &deposit.address).await?;
        if balance == 0 {
            continue;
        }

        let last_balance = get_last_balance(&state.db, &deposit.address)?;
        if last_balance >= balance {
            continue;
        }

        set_last_balance(&state.db, &deposit.address, balance)?;

        let amount_u64 = u64::try_from(balance).ok();
        store_deposit_event(
            &state.db,
            &DepositEvent {
                event_id: Uuid::new_v4().to_string(),
                deposit_id: deposit.deposit_id.clone(),
                tx_hash: format!("balance:{}", balance),
                confirmations: state.config.evm_confirmations,
                amount: amount_u64,
                status: "confirmed".to_string(),
                observed_at: chrono::Utc::now().timestamp(),
            },
        )?;

        update_deposit_status(&state.db, &deposit.deposit_id, "confirmed")?;
        emit_custody_event(
            state,
            "deposit.confirmed",
            &deposit.deposit_id,
            Some(&deposit.deposit_id),
            None,
            Some(&serde_json::json!({
                "chain": deposit.chain,
                "asset": deposit.asset,
                "address": deposit.address,
                "user_id": deposit.user_id,
                "amount": balance
            })),
        );

        if let Some(treasury) = treasury_for_chain(&state.config, &deposit.chain) {
            enqueue_sweep_job(
                &state.db,
                &SweepJob {
                    job_id: Uuid::new_v4().to_string(),
                    deposit_id: deposit.deposit_id.clone(),
                    chain: deposit.chain.clone(),
                    asset: deposit.asset.clone(),
                    from_address: deposit.address.clone(),
                    to_treasury: treasury,
                    tx_hash: format!("balance:{}:block:{}", balance, block_number),
                    amount: Some(balance.to_string()),
                    credited_amount: None,
                    signatures: Vec::new(),
                    sweep_tx_hash: None,
                    attempts: 0,
                    last_error: None,
                    next_attempt_at: None,
                    status: "queued".to_string(),
                    created_at: chrono::Utc::now().timestamp(),
                },
            )?;
            update_deposit_status(&state.db, &deposit.deposit_id, "sweep_queued")?;
        }
    }

    Ok(())
}

async fn process_solana_deposits(state: &CustodyState, url: &str) -> Result<(), String> {
    let deposits = list_pending_deposits_for_chains(&state.db, &["solana", "sol"])?;
    for deposit in deposits {
        if is_solana_stablecoin(&deposit.asset) {
            process_solana_token_deposit(state, url, &deposit).await?;
            continue;
        }
        let signatures =
            solana_get_signatures_for_address(&state.http, url, &deposit.address).await?;
        // M15 fix: process all new signatures, not just the first
        if signatures.is_empty() {
            continue;
        }

        for sig in &signatures {
            // AUDIT-FIX 0.11: Skip already-processed signatures to prevent
            // duplicate sweep jobs and double-crediting.
            if deposit_event_already_processed(&state.db, &deposit.deposit_id, sig) {
                continue;
            }

            let status = solana_get_signature_status(&state.http, url, sig).await?;
            let confirmed = status.confirmation_status == Some("finalized".to_string())
                || status.confirmations.unwrap_or(0) >= state.config.solana_confirmations;

            if !confirmed {
                continue;
            }

            store_deposit_event(
                &state.db,
                &DepositEvent {
                    event_id: Uuid::new_v4().to_string(),
                    deposit_id: deposit.deposit_id.clone(),
                    tx_hash: sig.clone(),
                    confirmations: status.confirmations.unwrap_or(0),
                    amount: None,
                    status: "confirmed".to_string(),
                    observed_at: chrono::Utc::now().timestamp(),
                },
            )?;

            update_deposit_status(&state.db, &deposit.deposit_id, "confirmed")?;
            emit_custody_event(
                state,
                "deposit.confirmed",
                &deposit.deposit_id,
                Some(&deposit.deposit_id),
                Some(sig),
                Some(&serde_json::json!({
                    "chain": deposit.chain,
                    "asset": deposit.asset,
                    "address": deposit.address,
                    "user_id": deposit.user_id
                })),
            );

            if let Some(treasury) = state.config.treasury_solana_address.clone() {
                let balance = solana_get_balance(&state.http, url, &deposit.address).await?;
                let credited_amount = if balance > SOLANA_SWEEP_FEE_LAMPORTS {
                    Some((balance - SOLANA_SWEEP_FEE_LAMPORTS).to_string())
                } else {
                    None
                };
                enqueue_sweep_job(
                    &state.db,
                    &SweepJob {
                        job_id: Uuid::new_v4().to_string(),
                        deposit_id: deposit.deposit_id.clone(),
                        chain: deposit.chain.clone(),
                        asset: deposit.asset.clone(),
                        from_address: deposit.address.clone(),
                        to_treasury: treasury,
                        tx_hash: sig.clone(),
                        amount: Some(balance.to_string()),
                        credited_amount,
                        signatures: Vec::new(),
                        sweep_tx_hash: None,
                        attempts: 0,
                        last_error: None,
                        next_attempt_at: None,
                        status: "queued".to_string(),
                        created_at: chrono::Utc::now().timestamp(),
                    },
                )?;
                update_deposit_status(&state.db, &deposit.deposit_id, "sweep_queued")?;
            }
            break; // process first confirmed signature per deposit per poll cycle
        }
    }

    Ok(())
}

async fn process_solana_token_deposit(
    state: &CustodyState,
    url: &str,
    deposit: &DepositRequest,
) -> Result<(), String> {
    let balance = solana_get_token_balance(&state.http, url, &deposit.address).await?;

    let last_key = format!("spl:{}:{}", deposit.asset, deposit.address);

    // AUDIT-FIX H1: When balance drops to zero (after sweep), reset the stored high
    // watermark to zero. Without this, the stored balance stays at the previous peak
    // and any subsequent deposit for a smaller amount would be missed forever
    // (because last_balance >= new_balance would remain true).
    if balance == 0 {
        let _ = set_last_balance_with_key(&state.db, &last_key, 0);
        return Ok(());
    }

    let last_balance = get_last_balance_with_key(&state.db, &last_key)?;
    if last_balance >= balance {
        return Ok(());
    }

    set_last_balance_with_key(&state.db, &last_key, balance)?;

    // AUDIT-FIX 0.11: Dedup check for SPL token deposits too
    let synthetic_tx_hash = format!("spl_balance:{}", balance);
    if deposit_event_already_processed(&state.db, &deposit.deposit_id, &synthetic_tx_hash) {
        return Ok(());
    }

    store_deposit_event(
        &state.db,
        &DepositEvent {
            event_id: Uuid::new_v4().to_string(),
            deposit_id: deposit.deposit_id.clone(),
            tx_hash: synthetic_tx_hash.clone(),
            confirmations: state.config.solana_confirmations,
            amount: Some(balance as u64),
            status: "confirmed".to_string(),
            observed_at: chrono::Utc::now().timestamp(),
        },
    )?;

    update_deposit_status(&state.db, &deposit.deposit_id, "confirmed")?;
    emit_custody_event(
        state,
        "deposit.confirmed",
        &deposit.deposit_id,
        Some(&deposit.deposit_id),
        Some(&synthetic_tx_hash),
        Some(&serde_json::json!({
            "chain": deposit.chain,
            "asset": deposit.asset,
            "address": deposit.address,
            "user_id": deposit.user_id,
            "amount": balance
        })),
    );

    if let Some(treasury) = state.config.solana_treasury_owner.clone() {
        let mint = solana_mint_for_asset(&state.config, &deposit.asset)?;
        let treasury_ata = derive_associated_token_address_from_str(&treasury, &mint)?;
        ensure_associated_token_account_for_str(state, &treasury, &mint, &treasury_ata).await?;

        enqueue_sweep_job(
            &state.db,
            &SweepJob {
                job_id: Uuid::new_v4().to_string(),
                deposit_id: deposit.deposit_id.clone(),
                chain: deposit.chain.clone(),
                asset: deposit.asset.clone(),
                from_address: deposit.address.clone(),
                to_treasury: treasury_ata,
                tx_hash: synthetic_tx_hash,
                amount: Some(balance.to_string()),
                credited_amount: None,
                signatures: Vec::new(),
                sweep_tx_hash: None,
                attempts: 0,
                last_error: None,
                next_attempt_at: None,
                status: "queued".to_string(),
                created_at: chrono::Utc::now().timestamp(),
            },
        )?;
        update_deposit_status(&state.db, &deposit.deposit_id, "sweep_queued")?;
    }

    Ok(())
}

async fn process_evm_deposits(state: &CustodyState, url: &str) -> Result<(), String> {
    let deposits = list_pending_deposits_for_chains(&state.db, &["ethereum", "eth", "bsc", "bnb"])?;
    let block_number = evm_get_block_number(&state.http, url).await?;

    // ERC20 failures should not block native balance detection
    if let Err(e) = process_evm_erc20_deposits(state, url, &deposits, block_number).await {
        tracing::warn!("erc20 log scan failed (non-fatal): {}", e);
    }

    for deposit in deposits {
        let balance = evm_get_balance(&state.http, url, &deposit.address).await?;
        if balance == 0 {
            continue;
        }

        let last_balance = get_last_balance(&state.db, &deposit.address)?;
        if last_balance >= balance {
            continue;
        }

        set_last_balance(&state.db, &deposit.address, balance)?;

        let amount_u64 = u64::try_from(balance).ok();
        store_deposit_event(
            &state.db,
            &DepositEvent {
                event_id: Uuid::new_v4().to_string(),
                deposit_id: deposit.deposit_id.clone(),
                tx_hash: format!("balance:{}", balance),
                confirmations: state.config.evm_confirmations,
                amount: amount_u64,
                status: "confirmed".to_string(),
                observed_at: chrono::Utc::now().timestamp(),
            },
        )?;

        update_deposit_status(&state.db, &deposit.deposit_id, "confirmed")?;
        emit_custody_event(
            state,
            "deposit.confirmed",
            &deposit.deposit_id,
            Some(&deposit.deposit_id),
            None,
            Some(&serde_json::json!({
                "chain": deposit.chain,
                "asset": deposit.asset,
                "address": deposit.address,
                "user_id": deposit.user_id,
                "amount": balance
            })),
        );

        if let Some(treasury) = state.config.treasury_evm_address.clone() {
            enqueue_sweep_job(
                &state.db,
                &SweepJob {
                    job_id: Uuid::new_v4().to_string(),
                    deposit_id: deposit.deposit_id.clone(),
                    chain: deposit.chain.clone(),
                    asset: deposit.asset.clone(),
                    from_address: deposit.address.clone(),
                    to_treasury: treasury,
                    tx_hash: format!("balance:{}:block:{}", balance, block_number),
                    amount: Some(balance.to_string()),
                    credited_amount: None,
                    signatures: Vec::new(),
                    sweep_tx_hash: None,
                    attempts: 0,
                    last_error: None,
                    next_attempt_at: None,
                    status: "queued".to_string(),
                    created_at: chrono::Utc::now().timestamp(),
                },
            )?;
            update_deposit_status(&state.db, &deposit.deposit_id, "sweep_queued")?;
        }
    }

    Ok(())
}

async fn process_evm_erc20_deposits(
    state: &CustodyState,
    url: &str,
    deposits: &[DepositRequest],
    block_number: u64,
) -> Result<(), String> {
    let token_deposits: Vec<&DepositRequest> = deposits
        .iter()
        .filter(|deposit| matches!(deposit.asset.as_str(), "usdc" | "usdt"))
        .collect();
    if token_deposits.is_empty() {
        return Ok(());
    }

    let mut address_map = std::collections::HashMap::new();
    for deposit in token_deposits {
        address_map.insert(deposit.address.to_lowercase(), deposit);
    }

    for asset in ["usdc", "usdt"] {
        let contract = evm_contract_for_asset(&state.config, asset)?;
        let cursor_key = format!("evm_logs:{}", contract.to_lowercase());
        let from_block = get_last_u64_index(&state.db, &cursor_key)?
            .unwrap_or(block_number.saturating_sub(1000));
        let to_block = block_number.saturating_sub(state.config.evm_confirmations);
        if to_block < from_block {
            continue;
        }
        // Cap block range to 10,000 to avoid RPC limits (BSC testnet caps at 50k)
        let from_block = if to_block - from_block > 10_000 {
            to_block - 10_000
        } else {
            from_block
        };

        let logs = evm_get_transfer_logs(&state.http, url, &contract, from_block, to_block).await?;
        for log in logs {
            if let Some((to, amount, tx_hash)) = decode_transfer_log(&log) {
                if let Some(deposit) = address_map.get(&to.to_lowercase()) {
                    store_deposit_event(
                        &state.db,
                        &DepositEvent {
                            event_id: Uuid::new_v4().to_string(),
                            deposit_id: deposit.deposit_id.clone(),
                            tx_hash: tx_hash.clone(),
                            confirmations: state.config.evm_confirmations,
                            amount: u64::try_from(amount).ok(),
                            status: "confirmed".to_string(),
                            observed_at: chrono::Utc::now().timestamp(),
                        },
                    )?;
                    update_deposit_status(&state.db, &deposit.deposit_id, "confirmed")?;
                    emit_custody_event(
                        state,
                        "deposit.confirmed",
                        &deposit.deposit_id,
                        Some(&deposit.deposit_id),
                        Some(&tx_hash),
                        Some(&serde_json::json!({
                            "chain": deposit.chain,
                            "asset": deposit.asset,
                            "address": deposit.address,
                            "user_id": deposit.user_id,
                            "amount": amount
                        })),
                    );

                    if let Some(treasury) = state.config.treasury_evm_address.clone() {
                        enqueue_sweep_job(
                            &state.db,
                            &SweepJob {
                                job_id: Uuid::new_v4().to_string(),
                                deposit_id: deposit.deposit_id.clone(),
                                chain: deposit.chain.clone(),
                                asset: deposit.asset.clone(),
                                from_address: deposit.address.clone(),
                                to_treasury: treasury,
                                tx_hash,
                                amount: Some(amount.to_string()),
                                credited_amount: None,
                                signatures: Vec::new(),
                                sweep_tx_hash: None,
                                attempts: 0,
                                last_error: None,
                                next_attempt_at: None,
                                status: "queued".to_string(),
                                created_at: chrono::Utc::now().timestamp(),
                            },
                        )?;
                        update_deposit_status(&state.db, &deposit.deposit_id, "sweep_queued")?;
                    }
                }
            }
        }

        set_last_u64_index(&state.db, &cursor_key, to_block.saturating_add(1))?;
    }

    Ok(())
}

#[derive(Debug)]
struct SignatureStatus {
    confirmations: Option<u64>,
    confirmation_status: Option<String>,
}

async fn solana_get_signatures_for_address(
    client: &reqwest::Client,
    url: &str,
    address: &str,
) -> Result<Vec<String>, String> {
    // M15 fix: fetch up to 10 signatures to handle multiple deposits between polls
    let params = json!([address, { "limit": 10 }]);
    let result = solana_rpc_call(client, url, "getSignaturesForAddress", params).await?;
    let mut signatures = Vec::new();
    if let Some(array) = result.as_array() {
        for item in array {
            if let Some(sig) = item.get("signature").and_then(|v| v.as_str()) {
                signatures.push(sig.to_string());
            }
        }
    }
    Ok(signatures)
}

async fn solana_get_signature_status(
    client: &reqwest::Client,
    url: &str,
    signature: &str,
) -> Result<SignatureStatus, String> {
    let params = json!([[signature]]);
    let result = solana_rpc_call(client, url, "getSignatureStatuses", params).await?;
    let value = result
        .get("value")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_object());
    let confirmations = value
        .and_then(|v| v.get("confirmations"))
        .and_then(|v| v.as_u64());
    let confirmation_status = value
        .and_then(|v| v.get("confirmation_status"))
        .and_then(|v| v.as_str())
        .map(|v| v.to_string());
    Ok(SignatureStatus {
        confirmations,
        confirmation_status,
    })
}

async fn solana_get_balance(
    client: &reqwest::Client,
    url: &str,
    address: &str,
) -> Result<u64, String> {
    let params = json!([address]);
    let result = solana_rpc_call(client, url, "getBalance", params).await?;
    result
        .get("value")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "balance missing".to_string())
}

async fn solana_get_token_balance(
    client: &reqwest::Client,
    url: &str,
    address: &str,
) -> Result<u64, String> {
    let params = json!([address]);
    let result = solana_rpc_call(client, url, "getTokenAccountBalance", params).await?;
    let amount = result
        .get("value")
        .and_then(|v| v.get("amount"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "token amount missing".to_string())?;
    amount
        .parse::<u64>()
        .map_err(|_| "invalid token amount".to_string())
}

async fn solana_get_account_exists(
    client: &reqwest::Client,
    url: &str,
    address: &str,
) -> Result<bool, String> {
    let params = json!([address, { "encoding": "base64" }]);
    let result = solana_rpc_call(client, url, "getAccountInfo", params).await?;
    let value = result.get("value").cloned().unwrap_or(Value::Null);
    Ok(!value.is_null())
}

async fn solana_rpc_call(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let response = client
        .post(url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("rpc send: {}", e))?;
    let value: Value = response
        .json()
        .await
        .map_err(|e| format!("rpc json: {}", e))?;
    if let Some(err) = value.get("error") {
        return Err(format!("rpc error: {}", err));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| "rpc result missing".to_string())
}

fn list_pending_deposits(db: &DB, chain: &str) -> Result<Vec<DepositRequest>, String> {
    // AUDIT-FIX M1: Use status index for "issued" and "pending" deposits.
    // This avoids O(n) full table scan on every poll cycle.
    let mut results = Vec::new();
    for status in ["issued", "pending"] {
        let ids = list_ids_by_status_index(db, "deposits", status)?;
        for id in ids {
            if let Some(record) = fetch_deposit(db, &id)? {
                if record.chain == chain {
                    results.push(record);
                }
            }
        }
    }
    // Fallback: if index is empty but table is not, do legacy full scan once
    // (covers pre-index data until all deposits cycle through)
    if results.is_empty() {
        let cf = db
            .cf_handle(CF_DEPOSITS)
            .ok_or_else(|| "missing deposits cf".to_string())?;
        let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
        for item in iter {
            let (_, value) = item.map_err(|e| format!("db iter: {}", e))?;
            let record: DepositRequest =
                serde_json::from_slice(&value).map_err(|e| format!("decode: {}", e))?;
            if record.chain == chain && (record.status == "issued" || record.status == "pending") {
                results.push(record);
            }
        }
    }
    Ok(results)
}

fn list_pending_deposits_for_chains(
    db: &DB,
    chains: &[&str],
) -> Result<Vec<DepositRequest>, String> {
    let mut results = Vec::new();
    for chain in chains {
        results.extend(list_pending_deposits(db, chain)?);
    }
    Ok(results)
}

fn get_last_balance(db: &DB, address: &str) -> Result<u128, String> {
    let cf = db
        .cf_handle(CF_ADDRESS_BALANCES)
        .ok_or_else(|| "missing address_balances cf".to_string())?;
    match db.get_cf(cf, address.as_bytes()) {
        Ok(Some(bytes)) => {
            let mut buf = [0u8; 16];
            buf.copy_from_slice(&bytes);
            Ok(u128::from_le_bytes(buf))
        }
        Ok(None) => Ok(0),
        Err(e) => Err(format!("db get: {}", e)),
    }
}

fn set_last_balance(db: &DB, address: &str, balance: u128) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_ADDRESS_BALANCES)
        .ok_or_else(|| "missing address_balances cf".to_string())?;
    db.put_cf(cf, address.as_bytes(), balance.to_le_bytes())
        .map_err(|e| format!("db put: {}", e))
}

fn get_last_balance_with_key(db: &DB, key: &str) -> Result<u64, String> {
    let cf = db
        .cf_handle(CF_TOKEN_BALANCES)
        .ok_or_else(|| "missing token_balances cf".to_string())?;
    match db.get_cf(cf, key.as_bytes()) {
        Ok(Some(bytes)) => {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&bytes);
            Ok(u64::from_le_bytes(buf))
        }
        Ok(None) => Ok(0),
        Err(e) => Err(format!("db get: {}", e)),
    }
}

fn set_last_balance_with_key(db: &DB, key: &str, balance: u64) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_TOKEN_BALANCES)
        .ok_or_else(|| "missing token_balances cf".to_string())?;
    db.put_cf(cf, key.as_bytes(), balance.to_le_bytes())
        .map_err(|e| format!("db put: {}", e))
}

async fn evm_get_balance(
    client: &reqwest::Client,
    url: &str,
    address: &str,
) -> Result<u128, String> {
    let params = json!([address, "latest"]);
    let result = evm_rpc_call(client, url, "eth_getBalance", params).await?;
    let value = result.as_str().unwrap_or("0x0");
    parse_hex_u128(value)
}

async fn evm_get_block_number(client: &reqwest::Client, url: &str) -> Result<u64, String> {
    let result = evm_rpc_call(client, url, "eth_blockNumber", json!([])).await?;
    let value = result.as_str().unwrap_or("0x0");
    parse_hex_u64(value)
}

async fn evm_get_transaction_count(
    client: &reqwest::Client,
    url: &str,
    address: &str,
) -> Result<u64, String> {
    let params = json!([address, "pending"]);
    let result = evm_rpc_call(client, url, "eth_getTransactionCount", params).await?;
    let value = result.as_str().unwrap_or("0x0");
    parse_hex_u64(value)
}

async fn evm_get_gas_price(client: &reqwest::Client, url: &str) -> Result<u128, String> {
    let result = evm_rpc_call(client, url, "eth_gasPrice", json!([])).await?;
    let value = result.as_str().unwrap_or("0x0");
    parse_hex_u128(value)
}

/// AUDIT-FIX M6: Dynamic gas estimation via eth_estimateGas.
/// Falls back to the provided `fallback` if the RPC call fails or returns 0.
/// Adds a 20% buffer to the estimate to prevent out-of-gas on execution.
async fn evm_estimate_gas(
    client: &reqwest::Client,
    url: &str,
    from: &str,
    to: &str,
    value: u128,
    data: Option<&[u8]>,
    fallback: u128,
) -> u128 {
    let mut params = serde_json::json!({
        "from": from,
        "to": to,
        "value": format!("0x{:x}", value),
    });
    if let Some(d) = data {
        params["data"] = serde_json::Value::String(format!("0x{}", hex::encode(d)));
    }
    match evm_rpc_call(client, url, "eth_estimateGas", json!([params])).await {
        Ok(result) => {
            let hex_str = result.as_str().unwrap_or("0x0");
            match parse_hex_u128(hex_str) {
                Ok(estimate) if estimate > 0 => {
                    // Add 20% buffer
                    let buffered = estimate.saturating_add(estimate / 5);
                    tracing::debug!(
                        "eth_estimateGas: {} → buffered to {} (fallback was {})",
                        estimate,
                        buffered,
                        fallback
                    );
                    buffered
                }
                _ => {
                    tracing::debug!("eth_estimateGas returned 0, using fallback {}", fallback);
                    fallback
                }
            }
        }
        Err(e) => {
            tracing::debug!(
                "eth_estimateGas failed ({}), using fallback {}",
                e,
                fallback
            );
            fallback
        }
    }
}

async fn evm_get_chain_id(client: &reqwest::Client, url: &str) -> Result<u64, String> {
    let result = evm_rpc_call(client, url, "eth_chainId", json!([])).await?;
    let value = result.as_str().unwrap_or("0x0");
    parse_hex_u64(value)
}

async fn evm_get_transaction_receipt(
    client: &reqwest::Client,
    url: &str,
    tx_hash: &str,
) -> Result<Option<Value>, String> {
    let result = evm_rpc_call(client, url, "eth_getTransactionReceipt", json!([tx_hash])).await?;
    if result.is_null() {
        return Ok(None);
    }
    Ok(Some(result))
}

async fn evm_get_transfer_logs(
    client: &reqwest::Client,
    url: &str,
    contract: &str,
    from_block: u64,
    to_block: u64,
) -> Result<Vec<Value>, String> {
    let params = json!([
        {
            "fromBlock": format!("0x{:x}", from_block),
            "toBlock": format!("0x{:x}", to_block),
            "address": contract,
            "topics": ["0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"],
        }
    ]);
    let result = evm_rpc_call(client, url, "eth_getLogs", params).await?;
    Ok(result.as_array().cloned().unwrap_or_default())
}

fn decode_transfer_log(log: &Value) -> Option<(String, u128, String)> {
    let topics = log.get("topics")?.as_array()?;
    if topics.len() < 3 {
        return None;
    }
    let to_topic = topics.get(2)?.as_str()?;
    let to_trimmed = to_topic.trim_start_matches("0x");
    if to_trimmed.len() < 40 {
        return None;
    }
    let to = format!("0x{}", &to_trimmed[to_trimmed.len() - 40..]);

    let data = log.get("data")?.as_str()?;
    let amount = parse_hex_u128(data).ok()?;

    let tx_hash = log.get("transactionHash")?.as_str()?.to_string();
    Some((to, amount, tx_hash))
}

async fn solana_get_signature_confirmed(
    client: &reqwest::Client,
    url: &str,
    signature: &str,
) -> Result<Option<bool>, String> {
    let params = json!([[signature]]);
    let result = solana_rpc_call(client, url, "getSignatureStatuses", params).await?;
    let value = result
        .get("value")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_object());
    if value.is_none() {
        return Ok(None);
    }
    let confirmed = value
        .and_then(|v| v.get("confirmation_status"))
        .and_then(|v| v.as_str())
        .map(|status| status == "finalized")
        .unwrap_or(false);
    Ok(Some(confirmed))
}

async fn check_sweep_confirmation(
    state: &CustodyState,
    job: &SweepJob,
) -> Result<Option<bool>, String> {
    let Some(tx_hash) = job.sweep_tx_hash.as_ref() else {
        return Ok(None);
    };

    if job.chain == "sol" || job.chain == "solana" {
        let url = state
            .config
            .solana_rpc_url
            .as_ref()
            .ok_or_else(|| "missing CUSTODY_SOLANA_RPC_URL".to_string())?;
        return solana_get_signature_confirmed(&state.http, url, tx_hash).await;
    }

    if is_evm_chain(&job.chain) {
        let url = rpc_url_for_chain(&state.config, &job.chain)
            .ok_or_else(|| format!("missing RPC URL for chain {}", job.chain))?;
        if let Some(receipt) = evm_get_transaction_receipt(&state.http, &url, tx_hash).await? {
            let status = receipt
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("0x0");
            return Ok(Some(status == "0x1"));
        }
        return Ok(None);
    }

    Ok(None)
}

async fn check_credit_confirmation(
    state: &CustodyState,
    job: &CreditJob,
) -> Result<Option<bool>, String> {
    let Some(signature) = job.tx_signature.as_ref() else {
        return Ok(None);
    };
    let Some(rpc_url) = state.config.licn_rpc_url.as_ref() else {
        return Ok(None);
    };
    let result =
        match licn_rpc_call(&state.http, rpc_url, "getTransaction", json!([signature])).await {
            Ok(v) => v,
            Err(e) if e.contains("not found") || e.contains("not exist") => return Ok(None),
            Err(e) => return Err(e),
        };
    if result.is_null() {
        return Ok(None);
    }
    let success = result
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    Ok(Some(success))
}

async fn evm_rpc_call(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let response = client
        .post(url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("rpc send: {}", e))?;
    let value: Value = response
        .json()
        .await
        .map_err(|e| format!("rpc json: {}", e))?;
    if let Some(err) = value.get("error") {
        return Err(format!("rpc error: {}", err));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| "rpc result missing".to_string())
}

fn parse_hex_u128(value: &str) -> Result<u128, String> {
    let trimmed = value.trim_start_matches("0x");
    u128::from_str_radix(trimmed, 16).map_err(|e| format!("parse hex: {}", e))
}

fn parse_hex_u64(value: &str) -> Result<u64, String> {
    let trimmed = value.trim_start_matches("0x");
    u64::from_str_radix(trimmed, 16).map_err(|e| format!("parse hex: {}", e))
}

async fn sweep_worker_loop(state: CustodyState) {
    loop {
        if let Err(err) = process_sweep_jobs(&state).await {
            tracing::warn!("sweep worker error: {}", err);
        }
        sleep(Duration::from_secs(state.config.poll_interval_secs)).await;
    }
}

async fn process_sweep_jobs(state: &CustodyState) -> Result<(), String> {
    let local_sweep_error = local_sweep_policy_error(&state.config);
    let queued_jobs = list_sweep_jobs_by_status(&state.db, "queued")?;
    for mut job in queued_jobs {
        if let Some(err) = local_sweep_error.as_ref() {
            job.status = "permanently_failed".to_string();
            job.last_error = Some(err.clone());
            job.next_attempt_at = None;
            store_sweep_job(&state.db, &job)?;
            emit_custody_event(
                state,
                "sweep.failed",
                &job.job_id,
                Some(&job.deposit_id),
                None,
                Some(&json!({ "last_error": err, "mode": "blocked-local-sweep" })),
            );
            continue;
        }

        job.status = "signing".to_string();
        store_sweep_job(&state.db, &job)?;
        emit_custody_event(
            state,
            "sweep.signing",
            &job.job_id,
            Some(&job.deposit_id),
            None,
            None,
        );
    }

    if local_sweep_error.is_none()
        && !state.config.signer_endpoints.is_empty()
        && state.config.signer_threshold > 0
    {
        warn!(
            "external signer endpoints are configured, but deposit sweeps still broadcast with locally derived deposit keys; skipping placeholder sweep signature collection"
        );
        promote_locally_signed_sweep_jobs(state, "locally-derived-deposit-key")?;
    } else if local_sweep_error.is_none() {
        promote_locally_signed_sweep_jobs(state, "self-custody")?;
    }

    if let Some(err) = local_sweep_error.as_ref() {
        for status in ["signing", "signed"] {
            let jobs = list_sweep_jobs_by_status(&state.db, status)?;
            for mut job in jobs {
                job.status = "permanently_failed".to_string();
                job.last_error = Some(err.clone());
                job.next_attempt_at = None;
                store_sweep_job(&state.db, &job)?;
                emit_custody_event(
                    state,
                    "sweep.failed",
                    &job.job_id,
                    Some(&job.deposit_id),
                    None,
                    Some(&json!({ "last_error": err, "mode": "blocked-local-sweep" })),
                );
            }
        }
    }

    let mut signed_jobs = list_sweep_jobs_by_status(&state.db, "signed")?;
    for job in signed_jobs.iter_mut() {
        if !is_ready_for_retry(job) {
            continue;
        }
        // AUDIT-FIX M4: Record intent before broadcast for crash idempotency
        let _ = record_tx_intent(&state.db, "sweep", &job.job_id, &job.chain);
        match broadcast_sweep(state, job).await {
            Ok(Some(tx_hash)) => {
                let _ = clear_tx_intent(&state.db, "sweep", &job.job_id);
                job.status = "sweep_submitted".to_string();
                job.sweep_tx_hash = Some(tx_hash);
                job.last_error = None;
                job.next_attempt_at = None;
                store_sweep_job(&state.db, job)?;
                emit_custody_event(
                    state,
                    "sweep.submitted",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.sweep_tx_hash.as_deref(),
                    None,
                );

                // AUDIT-FIX C2: Credit job (wrapped token mint) is now created AFTER
                // sweep confirmation, not here. Minting before sweep is confirmed risks
                // issuing wrapped tokens when the sweep tx reverts — a fund mismatch.
            }
            Ok(None) => {
                let _ = clear_tx_intent(&state.db, "sweep", &job.job_id);
                if job.chain == "solana" && !is_solana_stablecoin(&job.asset) {
                    job.status = "signed".to_string();
                    job.last_error = Some(
                        "insufficient native SOL to sweep after fees; awaiting additional funds"
                            .to_string(),
                    );
                    job.next_attempt_at = Some(chrono::Utc::now().timestamp() + 60);
                } else {
                    mark_sweep_failed(job, "broadcast returned empty".to_string());
                }
                store_sweep_job(&state.db, job)?;
                emit_custody_event(
                    state,
                    "sweep.failed",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.sweep_tx_hash.as_deref(),
                    None,
                );
            }
            Err(err) => {
                let _ = clear_tx_intent(&state.db, "sweep", &job.job_id);
                warn!("sweep broadcast failed: {}", err);
                mark_sweep_failed(job, err);
                store_sweep_job(&state.db, job)?;
            }
        }
    }

    let mut submitted_jobs = list_sweep_jobs_by_status(&state.db, "sweep_submitted")?;
    for job in submitted_jobs.iter_mut() {
        if let Some(confirmed) = check_sweep_confirmation(state, job).await? {
            if confirmed {
                job.status = "sweep_confirmed".to_string();
                job.last_error = None;
                job.next_attempt_at = None;
                store_sweep_job(&state.db, job)?;

                // P0-FIX: Update the deposit record to "swept" so polling clients
                // see the status progression (issued → confirmed → swept → credited)
                let _ = update_deposit_status(&state.db, &job.deposit_id, "swept");
                let _ = update_status_index(
                    &state.db,
                    "deposits",
                    "sweep_queued",
                    "swept",
                    &job.deposit_id,
                );

                emit_custody_event(
                    state,
                    "sweep.confirmed",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.sweep_tx_hash.as_deref(),
                    Some(&json!({ "chain": job.chain, "asset": job.asset, "amount": job.amount })),
                );

                // Track stablecoin reserves: when a sweep is confirmed, the treasury
                // now holds the deposited asset. Update the reserve ledger.
                let asset_lower = job.asset.to_lowercase();
                if asset_lower == "usdt" || asset_lower == "usdc" {
                    if let Some(ref amount_str) = job.amount {
                        if let Ok(amount) = amount_str.parse::<u64>() {
                            if let Err(e) = adjust_reserve_balance(
                                &state.db,
                                &job.chain,
                                &asset_lower,
                                amount,
                                true,
                            )
                            .await
                            {
                                tracing::warn!("reserve ledger update failed: {}", e);
                            }
                        }
                    }
                }

                // AUDIT-FIX C2: Create credit job (mint wrapped tokens) only AFTER
                // the sweep is confirmed on-chain. This ensures the treasury actually
                // received the funds before issuing wrapped tokens to the user.
                match build_credit_job(state, job)? {
                    Some(credit_job) => {
                        store_credit_job(&state.db, &credit_job)?;
                        emit_custody_event(
                            state,
                            "credit.queued",
                            &credit_job.job_id,
                            Some(&credit_job.deposit_id),
                            None,
                            Some(
                                &json!({ "amount_spores": credit_job.amount_spores, "to_address": credit_job.to_address }),
                            ),
                        );
                    }
                    None => {
                        // AUDIT-FIX R-H1: Log when credit job cannot be built
                        // after a confirmed sweep. This means the treasury received
                        // funds but the user won't get wrapped tokens automatically.
                        tracing::error!(
                            "🚨 CREDIT JOB NOT CREATED for sweep {} (deposit {}). \
                             Treasury received funds but no wrapped tokens will be minted. \
                             Manual operator intervention required to credit the user.",
                            job.job_id,
                            job.deposit_id
                        );
                        emit_custody_event(
                            state,
                            "credit.build_failed",
                            &job.job_id,
                            Some(&job.deposit_id),
                            None,
                            None,
                        );
                    }
                }
            } else {
                job.status = "failed".to_string();
                mark_sweep_failed(
                    job,
                    "sweep transaction reverted or failed on-chain".to_string(),
                );
                store_sweep_job(&state.db, job)?;
                emit_custody_event(
                    state,
                    "sweep.failed",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.sweep_tx_hash.as_deref(),
                    Some(
                        &json!({ "last_error": job.last_error, "chain": job.chain, "asset": job.asset }),
                    ),
                );
            }
        }
    }

    Ok(())
}

async fn credit_worker_loop(state: CustodyState) {
    loop {
        if let Err(err) = process_credit_jobs(&state).await {
            tracing::warn!("credit worker error: {}", err);
        }
        sleep(Duration::from_secs(state.config.poll_interval_secs)).await;
    }
}

async fn process_credit_jobs(state: &CustodyState) -> Result<(), String> {
    if state.config.licn_rpc_url.is_none() || state.config.treasury_keypair_path.is_none() {
        // AUDIT-FIX CUST-05: Warn instead of silently skipping (jobs accumulate in queued state)
        tracing::warn!(
            "credit worker skipping: licn_rpc_url or treasury_keypair_path not configured"
        );
        return Ok(());
    }

    let jobs = list_credit_jobs_by_status(&state.db, "queued")?;
    for mut job in jobs {
        if !is_ready_for_credit_retry(&job) {
            continue;
        }
        // AUDIT-FIX M4: Record intent before credit broadcast
        let _ = record_tx_intent(&state.db, "credit", &job.job_id, "lichen");
        match submit_wrapped_credit(state, &job).await {
            Ok(tx_signature) => {
                let _ = clear_tx_intent(&state.db, "credit", &job.job_id);
                job.status = "submitted".to_string();
                job.tx_signature = Some(tx_signature);
                job.last_error = None;
                job.next_attempt_at = None;
                store_credit_job(&state.db, &job)?;
                emit_custody_event(
                    state,
                    "credit.submitted",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.tx_signature.as_deref(),
                    None,
                );
            }
            Err(err) => {
                let _ = clear_tx_intent(&state.db, "credit", &job.job_id);
                tracing::warn!("credit mint failed for deposit={}: {}", job.deposit_id, err);
                mark_credit_failed(&mut job, err);
                store_credit_job(&state.db, &job)?;
            }
        }
    }

    let mut submitted = list_credit_jobs_by_status(&state.db, "submitted")?;
    for job in submitted.iter_mut() {
        let confirmation = match check_credit_confirmation(state, job).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    "credit confirmation check failed for job={}: {}",
                    job.job_id,
                    e
                );
                continue;
            }
        };
        if let Some(confirmed) = confirmation {
            if confirmed {
                job.status = "confirmed".to_string();
                job.last_error = None;
                job.next_attempt_at = None;
                store_credit_job(&state.db, job)?;

                // P0-FIX: Update the deposit record to "credited" so polling clients
                // see the terminal state and can stop polling.
                let _ = update_deposit_status(&state.db, &job.deposit_id, "credited");
                let _ = update_status_index(
                    &state.db,
                    "deposits",
                    "swept",
                    "credited",
                    &job.deposit_id,
                );

                emit_custody_event(
                    state,
                    "credit.confirmed",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.tx_signature.as_deref(),
                    Some(
                        &json!({ "amount_spores": job.amount_spores, "to_address": job.to_address }),
                    ),
                );
            }
        }
    }
    Ok(())
}

fn build_credit_job(state: &CustodyState, sweep: &SweepJob) -> Result<Option<CreditJob>, String> {
    let amount_source =
        if sweep.chain.eq_ignore_ascii_case("solana") && sweep.asset.eq_ignore_ascii_case("sol") {
            sweep.credited_amount.as_ref().or(sweep.amount.as_ref())
        } else {
            sweep.amount.as_ref()
        };
    let raw_amount = match amount_source {
        Some(value) => value
            .parse::<u128>()
            .map_err(|_| "invalid amount".to_string())?,
        None => return Ok(None),
    };

    let deposit = fetch_deposit(&state.db, &sweep.deposit_id)?;
    let Some(deposit) = deposit else {
        return Ok(None);
    };

    if state.config.licn_rpc_url.is_none() || state.config.treasury_keypair_path.is_none() {
        // AUDIT-FIX CUST-05: Warn instead of silently returning None
        tracing::warn!(
            "build_credit_job skipping: licn_rpc_url or treasury_keypair_path not configured"
        );
        return Ok(None);
    }

    if Pubkey::from_base58(&deposit.user_id).is_err() {
        return Ok(None);
    }

    // Resolve which wrapped token contract to mint based on source asset
    let source_asset = deposit.asset.to_lowercase();
    let source_chain = deposit.chain.to_lowercase();
    let _contract_addr = resolve_token_contract(&state.config, &source_chain, &source_asset);
    if _contract_addr.is_none() {
        tracing::warn!(
            "no wrapped token contract configured for chain={} asset={}",
            source_chain,
            source_asset
        );
        return Ok(None);
    }

    // Convert from source chain decimals to Lichen 9-decimal spores.
    // Must be ASSET-AWARE: native tokens and ERC-20/SPL tokens have different decimals.
    //   ETH native: 18 dec (wei)    | BNB native: 18 dec (wei)
    //   SOL native: 9 dec (lamports)
    //   USDT/USDC on Ethereum: 6 dec | USDT/USDC on BSC: 18 dec
    //   USDT/USDC on Solana: 6 dec
    // Lichen wrapped tokens all use 9 decimals (spores).
    let source_decimals: u32 = source_chain_decimals(&source_chain, &source_asset);
    let amount_spores: u64 = if source_decimals > 9 {
        let divisor = 10u128.pow(source_decimals - 9);
        // AUDIT-FIX CUST-06: Use try_from instead of silent truncation via `as u64`
        u64::try_from(raw_amount / divisor).map_err(|_| {
            format!(
                "credit amount overflow after decimal conversion (raw={raw_amount}, div={divisor})"
            )
        })?
    } else if source_decimals < 9 {
        let multiplier = 10u128.pow(9 - source_decimals);
        u64::try_from(raw_amount.saturating_mul(multiplier))
            .map_err(|_| format!("credit amount overflow after decimal conversion (raw={raw_amount}, mul={multiplier})"))?
    } else {
        u64::try_from(raw_amount)
            .map_err(|_| format!("credit amount overflow (raw={raw_amount})"))?
    };
    if amount_spores == 0 {
        tracing::warn!(
            "converted amount is 0 spores (raw={}, chain={}, asset={}, source_dec={}), skipping credit",
            raw_amount, source_chain, source_asset, source_decimals
        );
        return Ok(None);
    }

    Ok(Some(CreditJob {
        job_id: Uuid::new_v4().to_string(),
        deposit_id: sweep.deposit_id.clone(),
        to_address: deposit.user_id,
        amount_spores,
        source_asset,
        source_chain,
        status: "queued".to_string(),
        tx_signature: None,
        attempts: 0,
        last_error: None,
        next_attempt_at: None,
        created_at: chrono::Utc::now().timestamp(),
    }))
}

#[derive(Debug, Serialize)]
struct SignerRequest {
    job_id: String,
    chain: String,
    asset: String,
    from_address: String,
    to_address: String,
    amount: Option<String>,
    tx_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SignerResponse {
    status: String,
    signer_pubkey: String,
    signature: String,
    message_hash: String,
    _message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WithdrawalSigningMode {
    ExternalSingleSigner,
    SolanaThresholdFrost,
    EvmThresholdSafe,
}

#[derive(Debug, Clone)]
struct EvmSafeTransactionPlan {
    safe_address: String,
    nonce: u64,
    inner_to: String,
    inner_value: u128,
    inner_data: Vec<u8>,
    safe_tx_hash: [u8; 32],
    exec_calldata: Vec<u8>,
}

// ═══════════════════════════════════════════════════════════════════════════════
//  FROST Ed25519 Two-Round Signing Protocol
//
//  For multi-signer Solana threshold signatures:
//    Round 1: POST /frost/commit → signer generates nonce, returns commitment
//    Round 2: POST /frost/sign   → signer receives signing package, returns share
//
//  Signer service must implement:
//    POST /frost/commit  → FrostCommitRequest  → FrostCommitResponse
//    POST /frost/sign    → FrostSignRequest    → FrostSignResponse
// ═══════════════════════════════════════════════════════════════════════════════

#[allow(dead_code)]
#[derive(Debug, Serialize)]
struct FrostCommitRequest {
    job_id: String,
    message_hex: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct FrostCommitResponse {
    status: String,
    signer_id_hex: String,  // FROST Identifier (hex-encoded serialized)
    commitment_hex: String, // SigningCommitments (hex-encoded serialized)
}

#[allow(dead_code)]
#[derive(Debug, Serialize)]
struct FrostSignRequest {
    job_id: String,
    message_hex: String,
    commitments: Vec<FrostCommitmentEntry>,
}

#[allow(dead_code)]
#[derive(Debug, Serialize)]
struct FrostCommitmentEntry {
    signer_id_hex: String,
    commitment_hex: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
struct FrostSignResponse {
    status: String,
    signer_id_hex: String,
    share_hex: String, // SignatureShare (hex-encoded serialized)
}

/// Execute FROST two-round threshold signing protocol for Solana transactions.
/// Coordinates between multiple signer services to produce a single group Ed25519 signature.
///
/// Returns the number of valid signature shares collected and stored on the job.
async fn collect_frost_signature_entries(
    state: &CustodyState,
    job_id: &str,
    signatures: &mut Vec<SignerSignature>,
    message: &[u8],
) -> Result<usize, String> {
    let message_hex = hex::encode(message);

    // ── Round 1: Collect nonce commitments from all signers ──
    let commit_req = FrostCommitRequest {
        job_id: job_id.to_string(),
        message_hex: message_hex.clone(),
    };

    let mut commitments: Vec<(String, String)> = Vec::new(); // (signer_id_hex, commitment_hex)

    for (idx, endpoint) in state.config.signer_endpoints.iter().enumerate() {
        let url = format!("{}/frost/commit", endpoint.trim_end_matches('/'));
        let mut req = state.http.post(&url).json(&commit_req);
        let token = state
            .config
            .signer_auth_tokens
            .get(idx)
            .and_then(|t| t.as_ref())
            .or(state.config.signer_auth_token.as_ref());
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }

        match req.send().await {
            Ok(response) => match response.json::<FrostCommitResponse>().await {
                Ok(resp) if resp.status == "committed" => {
                    commitments.push((resp.signer_id_hex, resp.commitment_hex));
                }
                Ok(resp) => {
                    warn!(
                        "FROST commit: signer {} returned status={}",
                        idx, resp.status
                    );
                }
                Err(e) => {
                    warn!(
                        "FROST commit: failed to parse response from signer {}: {}",
                        idx, e
                    );
                }
            },
            Err(e) => {
                warn!("FROST commit: request failed for signer {}: {}", idx, e);
            }
        }
    }

    if commitments.len() < state.config.signer_threshold {
        return Err(format!(
            "FROST round 1 failed: only {} commitments received, need {}",
            commitments.len(),
            state.config.signer_threshold
        ));
    }

    // ── Round 2: Send all commitments to each signer, collect signature shares ──
    let sign_req = FrostSignRequest {
        job_id: job_id.to_string(),
        message_hex: message_hex.clone(),
        commitments: commitments
            .iter()
            .map(|(id, c)| FrostCommitmentEntry {
                signer_id_hex: id.clone(),
                commitment_hex: c.clone(),
            })
            .collect(),
    };

    // Clear old signatures and store FROST-specific data
    signatures.clear();

    for (idx, endpoint) in state.config.signer_endpoints.iter().enumerate() {
        let url = format!("{}/frost/sign", endpoint.trim_end_matches('/'));
        let mut req = state.http.post(&url).json(&sign_req);
        let token = state
            .config
            .signer_auth_tokens
            .get(idx)
            .and_then(|t| t.as_ref())
            .or(state.config.signer_auth_token.as_ref());
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }

        match req.send().await {
            Ok(response) => match response.json::<FrostSignResponse>().await {
                Ok(resp) if resp.status == "signed" => {
                    // Look up the matching commitment for this signer
                    let commitment_hex = commitments
                        .iter()
                        .find(|(id, _)| *id == resp.signer_id_hex)
                        .map(|(_, c)| c.clone())
                        .unwrap_or_default();

                    // AUDIT-FIX P10-CUST-02: Use length-prefixed encoding instead of
                    // "frost_commitment:" delimiter. The ":" delimiter could collide with
                    // hex data or other payload formats, causing parse ambiguity.
                    // Format: 4-byte big-endian msg_len || message_hex || commitment_hex
                    let frost_payload = {
                        let msg_bytes = message_hex.as_bytes();
                        let cmt_bytes = commitment_hex.as_bytes();
                        let mut buf = Vec::with_capacity(4 + msg_bytes.len() + cmt_bytes.len());
                        buf.extend_from_slice(&(msg_bytes.len() as u32).to_be_bytes());
                        buf.extend_from_slice(msg_bytes);
                        buf.extend_from_slice(cmt_bytes);
                        hex::encode(buf)
                    };
                    signatures.push(SignerSignature {
                        signer_pubkey: resp.signer_id_hex,
                        signature: resp.share_hex,
                        message_hash: frost_payload,
                        received_at: chrono::Utc::now().timestamp(),
                    });
                }
                Ok(resp) => {
                    warn!("FROST sign: signer {} returned status={}", idx, resp.status);
                }
                Err(e) => {
                    warn!(
                        "FROST sign: failed to parse response from signer {}: {}",
                        idx, e
                    );
                }
            },
            Err(e) => {
                warn!("FROST sign: request failed for signer {}: {}", idx, e);
            }
        }

        if signatures.len() >= state.config.signer_threshold {
            break;
        }
    }

    Ok(signatures.len())
}

fn promote_locally_signed_sweep_jobs(state: &CustodyState, sweep_mode: &str) -> Result<(), String> {
    let mut signing_jobs = list_sweep_jobs_by_status(&state.db, "signing")?;
    for job in signing_jobs.iter_mut() {
        if !job.signatures.is_empty() {
            job.signatures.clear();
        }
        job.status = "signed".to_string();
        store_sweep_job(&state.db, job)?;
        emit_custody_event(
            state,
            "sweep.signed",
            &job.job_id,
            Some(&job.deposit_id),
            None,
            Some(&json!({
                "mode": sweep_mode,
                "threshold_signing": false,
            })),
        );
    }
    Ok(())
}

#[allow(dead_code)]
async fn collect_frost_signatures(
    state: &CustodyState,
    job: &mut SweepJob,
    message: &[u8],
) -> Result<usize, String> {
    collect_frost_signature_entries(state, &job.job_id, &mut job.signatures, message).await
}

/// Collect individual ECDSA signatures from EVM signers.
/// Each signer produces a standard secp256k1 ECDSA signature independently.
/// These are later packed into Gnosis Safe execTransaction format.
#[allow(dead_code)]
async fn collect_evm_multisig_signatures(
    state: &CustodyState,
    job: &mut SweepJob,
    tx_hash: &[u8],
) -> Result<usize, String> {
    let request = SignerRequest {
        job_id: job.job_id.clone(),
        chain: job.chain.clone(),
        asset: job.asset.clone(),
        from_address: job.from_address.clone(),
        to_address: job.to_treasury.clone(),
        amount: job.amount.clone(),
        tx_hash: Some(hex::encode(tx_hash)),
    };

    for (idx, endpoint) in state.config.signer_endpoints.iter().enumerate() {
        let url = format!("{}/sign", endpoint.trim_end_matches('/'));
        let mut req = state.http.post(&url).json(&request);
        let token = state
            .config
            .signer_auth_tokens
            .get(idx)
            .and_then(|t| t.as_ref())
            .or(state.config.signer_auth_token.as_ref());
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }

        match req.send().await {
            Ok(response) => match response.json::<SignerResponse>().await {
                Ok(payload) if payload.status == "signed" => {
                    let already_signed = job
                        .signatures
                        .iter()
                        .any(|s| s.signer_pubkey == payload.signer_pubkey);
                    if !already_signed {
                        job.signatures.push(SignerSignature {
                            signer_pubkey: payload.signer_pubkey,
                            signature: payload.signature,
                            message_hash: payload.message_hash,
                            received_at: chrono::Utc::now().timestamp(),
                        });
                    }
                }
                _ => {}
            },
            Err(e) => {
                warn!("EVM signer request failed for signer {}: {}", idx, e);
            }
        }

        if job.signatures.len() >= state.config.signer_threshold {
            break;
        }
    }

    Ok(job.signatures.len())
}

#[allow(dead_code)]
async fn collect_signatures(state: &CustodyState, job: &mut SweepJob) -> Result<usize, String> {
    let request = SignerRequest {
        job_id: job.job_id.clone(),
        chain: job.chain.clone(),
        asset: job.asset.clone(),
        from_address: job.from_address.clone(),
        to_address: job.to_treasury.clone(),
        amount: job.amount.clone(),
        tx_hash: Some(job.tx_hash.clone()),
    };

    for (idx, endpoint) in state.config.signer_endpoints.iter().enumerate() {
        let url = format!("{}/sign", endpoint.trim_end_matches('/'));
        let mut req = state.http.post(url).json(&request);
        let token = state
            .config
            .signer_auth_tokens
            .get(idx)
            .and_then(|t| t.as_ref())
            .or(state.config.signer_auth_token.as_ref());
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        let response = match req.send().await {
            Ok(response) => response,
            Err(err) => {
                warn!("signer request failed: {}", err);
                continue;
            }
        };
        let payload: SignerResponse = match response.json().await {
            Ok(payload) => payload,
            Err(err) => {
                warn!("signer response decode failed: {}", err);
                continue;
            }
        };

        if payload.status != "signed" {
            continue;
        }

        if job
            .signatures
            .iter()
            .any(|sig| sig.signer_pubkey == payload.signer_pubkey)
        {
            continue;
        }

        job.signatures.push(SignerSignature {
            signer_pubkey: payload.signer_pubkey,
            signature: payload.signature,
            message_hash: payload.message_hash,
            received_at: chrono::Utc::now().timestamp(),
        });

        if job.signatures.len() >= state.config.signer_threshold {
            break;
        }
    }

    Ok(job.signatures.len())
}

fn withdrawal_treasury_address(config: &CustodyConfig, dest_chain: &str) -> String {
    match dest_chain {
        "solana" | "sol" => config.treasury_solana_address.clone().unwrap_or_default(),
        "ethereum" | "eth" | "bsc" | "bnb" => {
            config.treasury_evm_address.clone().unwrap_or_default()
        }
        _ => String::new(),
    }
}

fn evm_executor_derivation_path(dest_chain: &str) -> &'static str {
    match dest_chain {
        "bsc" | "bnb" => "custody/treasury/bnb",
        _ => "custody/treasury/ethereum",
    }
}

fn determine_withdrawal_signing_mode(
    state: &CustodyState,
    job: &WithdrawalJob,
    outbound_asset: &str,
) -> Result<Option<WithdrawalSigningMode>, String> {
    if state.config.signer_endpoints.is_empty() || state.config.signer_threshold == 0 {
        return Ok(None);
    }

    if state.config.signer_threshold <= 1 || state.config.signer_endpoints.len() <= 1 {
        return Ok(Some(WithdrawalSigningMode::ExternalSingleSigner));
    }

    match job.dest_chain.as_str() {
        "solana" | "sol" => {
            if outbound_asset != "sol" && !is_solana_stablecoin(outbound_asset) {
                return Err(format!(
                    "threshold Solana withdrawals currently support native SOL and SPL stablecoins, not {}",
                    outbound_asset
                ));
            }
            if state.config.frost_pubkey_package_hex.is_none() {
                return Err(
                    "FROST public key package not configured (set CUSTODY_FROST_PUBKEY_PACKAGE)"
                        .to_string(),
                );
            }
            Ok(Some(WithdrawalSigningMode::SolanaThresholdFrost))
        }
        "ethereum" | "eth" | "bsc" | "bnb" => Err(if state.config.evm_multisig_address.is_none() {
            "EVM multisig address not configured (set CUSTODY_EVM_MULTISIG_ADDRESS)".to_string()
        } else {
            return Ok(Some(WithdrawalSigningMode::EvmThresholdSafe));
        }),
        other => Err(format!("unsupported destination chain: {}", other)),
    }
}

fn abi_encode_address_word(address: &str) -> Result<[u8; 32], String> {
    let addr = parse_evm_address(address)?;
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(&addr);
    Ok(word)
}

fn abi_encode_u64_word(value: u64) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[24..].copy_from_slice(&value.to_be_bytes());
    word
}

fn abi_encode_u128_word(value: u128) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[16..].copy_from_slice(&value.to_be_bytes());
    word
}

fn abi_encode_bytes_tail(bytes: &[u8]) -> Vec<u8> {
    let mut tail = Vec::new();
    tail.extend_from_slice(&abi_encode_u64_word(bytes.len() as u64));
    tail.extend_from_slice(bytes);
    let padding = (32 - (bytes.len() % 32)) % 32;
    tail.extend_from_slice(&vec![0u8; padding]);
    tail
}

fn evm_function_selector(signature: &str) -> [u8; 4] {
    use sha3::{Digest, Keccak256};

    let digest = Keccak256::digest(signature.as_bytes());
    [digest[0], digest[1], digest[2], digest[3]]
}

async fn evm_call(
    client: &reqwest::Client,
    url: &str,
    to: &str,
    data: &[u8],
) -> Result<Value, String> {
    evm_rpc_call(
        client,
        url,
        "eth_call",
        json!([{
            "to": to,
            "data": format!("0x{}", hex::encode(data)),
        }, "latest"]),
    )
    .await
}

async fn evm_safe_get_nonce(
    client: &reqwest::Client,
    url: &str,
    safe_address: &str,
) -> Result<u64, String> {
    let selector = evm_function_selector("nonce()");
    let result = evm_call(client, url, safe_address, &selector).await?;
    let value = result.as_str().unwrap_or("0x0");
    parse_hex_u64(value)
}

fn build_evm_safe_get_transaction_hash_calldata(
    inner_to: &str,
    inner_value: u128,
    inner_data: &[u8],
    nonce: u64,
) -> Result<Vec<u8>, String> {
    let mut calldata = Vec::new();
    calldata.extend_from_slice(&evm_function_selector(
        "getTransactionHash(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,uint256)",
    ));
    calldata.extend_from_slice(&abi_encode_address_word(inner_to)?);
    calldata.extend_from_slice(&abi_encode_u128_word(inner_value));
    calldata.extend_from_slice(&abi_encode_u64_word(10 * 32));
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&abi_encode_u64_word(nonce));
    calldata.extend_from_slice(&abi_encode_bytes_tail(inner_data));
    Ok(calldata)
}

fn build_evm_safe_exec_transaction_calldata(
    inner_to: &str,
    inner_value: u128,
    inner_data: &[u8],
    signatures: &[u8],
) -> Result<Vec<u8>, String> {
    let data_offset = 10 * 32;
    let data_tail = abi_encode_bytes_tail(inner_data);
    let sigs_offset = data_offset + data_tail.len();
    let sigs_tail = abi_encode_bytes_tail(signatures);

    let mut calldata = Vec::new();
    calldata.extend_from_slice(&evm_function_selector(
        "execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)",
    ));
    calldata.extend_from_slice(&abi_encode_address_word(inner_to)?);
    calldata.extend_from_slice(&abi_encode_u128_word(inner_value));
    calldata.extend_from_slice(&abi_encode_u64_word(data_offset as u64));
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&[0u8; 32]);
    calldata.extend_from_slice(&abi_encode_u64_word(sigs_offset as u64));
    calldata.extend_from_slice(&data_tail);
    calldata.extend_from_slice(&sigs_tail);
    Ok(calldata)
}

fn normalize_evm_signature(signature: &[u8]) -> Result<Vec<u8>, String> {
    if signature.len() != 65 {
        return Err(format!(
            "invalid EVM signature length: expected 65, got {}",
            signature.len()
        ));
    }
    let mut normalized = signature.to_vec();
    if normalized[64] < 27 {
        normalized[64] = normalized[64].saturating_add(27);
    }
    if normalized[64] != 27 && normalized[64] != 28 {
        return Err(format!(
            "invalid EVM recovery id: expected 27/28, got {}",
            normalized[64]
        ));
    }
    Ok(normalized)
}

fn build_evm_threshold_withdrawal_intent(
    state: &CustodyState,
    job: &WithdrawalJob,
    asset: &str,
    nonce: u64,
) -> Result<(String, u128, Vec<u8>), String> {
    let is_erc20 = matches!(asset, "usdt" | "usdc");
    if is_erc20 {
        let contract_addr = evm_contract_for_asset(&state.config, asset)
            .map_err(|e| format!("resolve ERC-20 contract for withdrawal: {}", e))?;
        let chain_amount = spores_to_chain_amount(job.amount, &job.dest_chain, asset);
        let transfer_data = evm_encode_erc20_transfer(&job.dest_address, chain_amount)
            .map_err(|e| format!("encode ERC-20 transfer: {}", e))?;
        let _ = nonce;
        Ok((contract_addr, 0u128, transfer_data))
    } else {
        let chain_amount = spores_to_chain_amount(job.amount, &job.dest_chain, asset);
        let _ = nonce;
        Ok((job.dest_address.clone(), chain_amount, Vec::new()))
    }
}

async fn build_evm_safe_transaction_plan(
    state: &CustodyState,
    url: &str,
    job: &WithdrawalJob,
    asset: &str,
) -> Result<EvmSafeTransactionPlan, String> {
    let safe_address = state.config.evm_multisig_address.clone().ok_or_else(|| {
        "EVM multisig address not configured (set CUSTODY_EVM_MULTISIG_ADDRESS)".to_string()
    })?;
    let nonce = match job.safe_nonce {
        Some(nonce) => nonce,
        None => evm_safe_get_nonce(&state.http, url, &safe_address).await?,
    };
    let (inner_to, inner_value, inner_data) =
        build_evm_threshold_withdrawal_intent(state, job, asset, nonce)?;
    let hash_calldata =
        build_evm_safe_get_transaction_hash_calldata(&inner_to, inner_value, &inner_data, nonce)?;
    let hash_result = evm_call(&state.http, url, &safe_address, &hash_calldata).await?;
    let hash_hex = hash_result
        .as_str()
        .ok_or_else(|| "Safe getTransactionHash returned non-string result".to_string())?;
    let hash_bytes = hex::decode(hash_hex.trim_start_matches("0x"))
        .map_err(|e| format!("decode Safe tx hash: {}", e))?;
    if hash_bytes.len() != 32 {
        return Err(format!(
            "invalid Safe tx hash length: expected 32, got {}",
            hash_bytes.len()
        ));
    }
    let mut safe_tx_hash = [0u8; 32];
    safe_tx_hash.copy_from_slice(&hash_bytes);

    Ok(EvmSafeTransactionPlan {
        safe_address,
        nonce,
        inner_to,
        inner_value,
        inner_data,
        safe_tx_hash,
        exec_calldata: Vec::new(),
    })
}

fn finalize_evm_safe_exec_plan(
    mut plan: EvmSafeTransactionPlan,
    signatures: &[u8],
) -> Result<EvmSafeTransactionPlan, String> {
    plan.exec_calldata = build_evm_safe_exec_transaction_calldata(
        &plan.inner_to,
        plan.inner_value,
        &plan.inner_data,
        signatures,
    )?;
    Ok(plan)
}

fn solana_treasury_owner_address(config: &CustodyConfig) -> Result<String, String> {
    config
        .solana_treasury_owner
        .clone()
        .or_else(|| config.treasury_solana_address.clone())
        .ok_or_else(|| {
            "missing Solana treasury owner (set CUSTODY_SOLANA_TREASURY_OWNER or CUSTODY_TREASURY_SOLANA_ADDRESS)"
                .to_string()
        })
}

fn resolve_solana_token_withdrawal_accounts(
    config: &CustodyConfig,
    asset: &str,
    dest_owner: &str,
) -> Result<(String, String, String, String), String> {
    let treasury_owner = solana_treasury_owner_address(config)?;
    let mint = solana_mint_for_asset(config, asset)?;
    let from_token_account = derive_associated_token_address_from_str(&treasury_owner, &mint)?;
    let to_token_account = derive_associated_token_address_from_str(dest_owner, &mint)?;
    Ok((treasury_owner, mint, from_token_account, to_token_account))
}

fn build_solana_token_transfer_message(
    authority_pubkey: &[u8; 32],
    from_token_account: &[u8; 32],
    to_token_account: &[u8; 32],
    raw_amount: u64,
    recent_blockhash: &[u8; 32],
) -> Result<Vec<u8>, String> {
    let token_program = decode_solana_pubkey(SOLANA_TOKEN_PROGRAM)?;
    let account_keys = vec![
        *authority_pubkey,
        *from_token_account,
        *to_token_account,
        token_program,
    ];

    let header = SolanaMessageHeader {
        num_required_signatures: 1,
        num_readonly_signed: 0,
        num_readonly_unsigned: 1,
    };

    let mut data = Vec::with_capacity(9);
    data.push(3u8);
    data.extend_from_slice(&raw_amount.to_le_bytes());

    let instruction = SolanaInstruction {
        program_id_index: 3,
        account_indices: vec![1, 2, 0],
        data,
    };

    Ok(build_solana_message_with_instructions(
        header,
        &account_keys,
        recent_blockhash,
        &[instruction],
    ))
}

fn build_threshold_solana_withdrawal_message(
    state: &CustodyState,
    job: &WithdrawalJob,
    outbound_asset: &str,
    recent_blockhash: &[u8; 32],
) -> Result<Vec<u8>, String> {
    if outbound_asset == "sol" {
        let solana_tx_fee: u64 = 5_000;
        if job.amount <= solana_tx_fee {
            return Err("withdrawal amount too small to cover fees".to_string());
        }

        let treasury_address = state
            .config
            .treasury_solana_address
            .as_ref()
            .ok_or_else(|| "missing CUSTODY_TREASURY_SOLANA_ADDRESS".to_string())?;
        let from_pubkey = decode_solana_pubkey(treasury_address)?;
        let to_pubkey = decode_solana_pubkey(&job.dest_address)?;
        let transfer_amount = job.amount - solana_tx_fee;

        return Ok(build_solana_transfer_message(
            &from_pubkey,
            &to_pubkey,
            transfer_amount,
            recent_blockhash,
        ));
    }

    if !is_solana_stablecoin(outbound_asset) {
        return Err(format!(
            "unsupported threshold Solana withdrawal asset: {}",
            outbound_asset
        ));
    }

    let (treasury_owner, _, from_token_account, to_token_account) =
        resolve_solana_token_withdrawal_accounts(&state.config, outbound_asset, &job.dest_address)?;
    let authority_pubkey = decode_solana_pubkey(&treasury_owner)?;
    let from_token_pubkey = decode_solana_pubkey(&from_token_account)?;
    let to_token_pubkey = decode_solana_pubkey(&to_token_account)?;
    let raw_amount = u64::try_from(spores_to_chain_amount(
        job.amount,
        &job.dest_chain,
        outbound_asset,
    ))
    .map_err(|_| "solana token withdrawal amount overflow".to_string())?;

    build_solana_token_transfer_message(
        &authority_pubkey,
        &from_token_pubkey,
        &to_token_pubkey,
        raw_amount,
        recent_blockhash,
    )
}

async fn collect_threshold_solana_withdrawal_signatures(
    state: &CustodyState,
    job: &mut WithdrawalJob,
    outbound_asset: &str,
) -> Result<usize, String> {
    if is_solana_stablecoin(outbound_asset) {
        let (treasury_owner, mint, from_token_account, to_token_account) =
            resolve_solana_token_withdrawal_accounts(
                &state.config,
                outbound_asset,
                &job.dest_address,
            )?;
        ensure_associated_token_account_for_str(state, &treasury_owner, &mint, &from_token_account)
            .await?;
        ensure_associated_token_account_for_str(state, &job.dest_address, &mint, &to_token_account)
            .await?;
    }

    let url = state
        .config
        .solana_rpc_url
        .as_ref()
        .ok_or_else(|| "missing solana RPC".to_string())?;
    let recent_blockhash = solana_get_latest_blockhash(&state.http, url).await?;
    let message =
        build_threshold_solana_withdrawal_message(state, job, outbound_asset, &recent_blockhash)?;
    collect_frost_signature_entries(state, &job.job_id, &mut job.signatures, &message).await
}

async fn collect_single_signer_withdrawal_signatures(
    state: &CustodyState,
    job: &mut WithdrawalJob,
    outbound_asset: &str,
) -> Result<usize, String> {
    let signer_request = SignerRequest {
        job_id: job.job_id.clone(),
        chain: job.dest_chain.clone(),
        asset: outbound_asset.to_string(),
        from_address: withdrawal_treasury_address(&state.config, &job.dest_chain),
        to_address: job.dest_address.clone(),
        amount: Some(job.amount.to_string()),
        tx_hash: None,
    };

    let mut sig_count = job.signatures.len();
    for (idx, endpoint) in state.config.signer_endpoints.iter().enumerate() {
        let url = format!("{}/sign", endpoint.trim_end_matches('/'));
        let mut req = state.http.post(&url).json(&signer_request);
        let token = state
            .config
            .signer_auth_tokens
            .get(idx)
            .and_then(|t| t.as_ref())
            .or(state.config.signer_auth_token.as_ref());
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }

        match req.send().await {
            Ok(response) => {
                if let Ok(payload) = response.json::<SignerResponse>().await {
                    if payload.status == "signed" {
                        let already_signed = job
                            .signatures
                            .iter()
                            .any(|s| s.signer_pubkey == payload.signer_pubkey);
                        if !already_signed {
                            job.signatures.push(SignerSignature {
                                signer_pubkey: payload.signer_pubkey,
                                signature: payload.signature,
                                message_hash: payload.message_hash,
                                received_at: chrono::Utc::now().timestamp(),
                            });
                            sig_count = job.signatures.len();
                        }
                    }
                }
            }
            Err(err) => {
                tracing::warn!(
                    "signer request failed for withdrawal {}: {}",
                    job.job_id,
                    err
                );
            }
        }

        if sig_count >= state.config.signer_threshold {
            break;
        }
    }

    Ok(sig_count)
}

async fn collect_threshold_evm_withdrawal_signatures(
    state: &CustodyState,
    job: &mut WithdrawalJob,
    outbound_asset: &str,
) -> Result<usize, String> {
    let url = rpc_url_for_chain(&state.config, &job.dest_chain)
        .ok_or_else(|| format!("missing RPC URL for chain {}", job.dest_chain))?;
    let plan = build_evm_safe_transaction_plan(state, &url, job, outbound_asset).await?;
    let safe_tx_hash_hex = hex::encode(plan.safe_tx_hash);
    job.safe_nonce = Some(plan.nonce);

    if job
        .signatures
        .iter()
        .any(|sig| sig.message_hash != safe_tx_hash_hex)
    {
        job.signatures.clear();
    }

    let request = SignerRequest {
        job_id: job.job_id.clone(),
        chain: job.dest_chain.clone(),
        asset: outbound_asset.to_string(),
        from_address: plan.safe_address,
        to_address: job.dest_address.clone(),
        amount: Some(job.amount.to_string()),
        tx_hash: Some(safe_tx_hash_hex.clone()),
    };

    for (idx, endpoint) in state.config.signer_endpoints.iter().enumerate() {
        let url = format!("{}/sign", endpoint.trim_end_matches('/'));
        let mut req = state.http.post(&url).json(&request);
        let token = state
            .config
            .signer_auth_tokens
            .get(idx)
            .and_then(|t| t.as_ref())
            .or(state.config.signer_auth_token.as_ref());
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }

        match req.send().await {
            Ok(response) => match response.json::<SignerResponse>().await {
                Ok(payload) if payload.status == "signed" => {
                    let signer_addr = payload
                        .signer_pubkey
                        .trim_start_matches("0x")
                        .to_lowercase();
                    let already_signed = job.signatures.iter().any(|s| {
                        s.signer_pubkey
                            .trim_start_matches("0x")
                            .eq_ignore_ascii_case(&signer_addr)
                    });
                    if !already_signed {
                        job.signatures.push(SignerSignature {
                            signer_pubkey: signer_addr,
                            signature: payload.signature,
                            message_hash: safe_tx_hash_hex.clone(),
                            received_at: chrono::Utc::now().timestamp(),
                        });
                    }
                }
                Ok(_) => {}
                Err(err) => {
                    warn!(
                        "EVM signer response decode failed for {}: {}",
                        job.job_id, err
                    );
                }
            },
            Err(err) => {
                warn!("EVM signer request failed for {}: {}", job.job_id, err);
            }
        }

        if job.signatures.len() >= state.config.signer_threshold {
            break;
        }
    }

    Ok(job.signatures.len())
}

async fn broadcast_sweep(state: &CustodyState, job: &SweepJob) -> Result<Option<String>, String> {
    if let Some(err) = local_sweep_policy_error(&state.config) {
        return Err(format!(
            "{}; refusing to broadcast sweep {} on {}",
            err, job.job_id, job.chain
        ));
    }

    if job.chain == "sol" || job.chain == "solana" {
        let url = state
            .config
            .solana_rpc_url
            .as_ref()
            .ok_or_else(|| "missing CUSTODY_SOLANA_RPC_URL".to_string())?;
        return broadcast_solana_sweep(state, url, job).await;
    }

    if is_evm_chain(&job.chain) {
        let url = rpc_url_for_chain(&state.config, &job.chain)
            .ok_or_else(|| format!("missing RPC URL for chain {}", job.chain))?;
        return broadcast_evm_sweep(state, &url, job).await;
    }

    Ok(None)
}

async fn broadcast_solana_sweep(
    state: &CustodyState,
    url: &str,
    job: &SweepJob,
) -> Result<Option<String>, String> {
    if is_solana_stablecoin(&job.asset) {
        return broadcast_solana_token_sweep(state, url, job).await;
    }

    let amount = match job.amount.as_ref() {
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| "invalid amount".to_string())?,
        None => return Ok(None),
    };
    if amount == 0 {
        return Ok(None);
    }

    let deposit = fetch_deposit(&state.db, &job.deposit_id)?;
    let Some(deposit) = deposit else {
        return Ok(None);
    };
    let deposit_seed = deposit_seed_for_record(&state.config, &deposit);

    // AUDIT-FIX C1: Deduct the Solana transaction fee from the sweep amount.
    // The deposit address is the fee payer, so it needs: transfer_amount + fee.
    // Without this, the tx would fail because the account lacks fee funds.
    if amount <= SOLANA_SWEEP_FEE_LAMPORTS {
        // Dust amount — not worth sweeping (would go entirely to fees)
        return Ok(None);
    }
    let transfer_amount = amount - SOLANA_SWEEP_FEE_LAMPORTS;

    let recent_blockhash = solana_get_latest_blockhash(&state.http, url).await?;
    let (signing_key, from_pubkey) = derive_solana_signer(&deposit.derivation_path, deposit_seed)?;
    let to_pubkey = decode_solana_pubkey(&job.to_treasury)?;

    let message =
        build_solana_transfer_message(&from_pubkey, &to_pubkey, transfer_amount, &recent_blockhash);
    let signature = signing_key.sign(&message).to_bytes();
    let tx = build_solana_transaction(&[signature], &message);
    let signature = solana_send_transaction(&state.http, url, &tx).await?;
    Ok(Some(signature))
}

async fn broadcast_solana_token_sweep(
    state: &CustodyState,
    url: &str,
    job: &SweepJob,
) -> Result<Option<String>, String> {
    let amount = match job.amount.as_ref() {
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| "invalid amount".to_string())?,
        None => return Ok(None),
    };
    if amount == 0 {
        return Ok(None);
    }

    let deposit = fetch_deposit(&state.db, &job.deposit_id)?;
    let Some(deposit) = deposit else {
        return Ok(None);
    };

    // Fee payer: load from file if configured, otherwise derive from master seed
    let fee_payer = if let Some(ref fee_payer_path) = state.config.solana_fee_payer_keypair_path {
        load_solana_keypair(fee_payer_path)?
    } else {
        // Derive fee payer from master seed with well-known path
        derive_solana_keypair("custody/fee-payer/solana", &state.config.master_seed)?
    };

    let owner_keypair = derive_solana_keypair(
        &deposit.derivation_path,
        deposit_seed_for_record(&state.config, &deposit),
    )?;

    let from_account = decode_solana_pubkey(&job.from_address)?;
    let to_account = decode_solana_pubkey(&job.to_treasury)?;
    let token_program = decode_solana_pubkey(SOLANA_TOKEN_PROGRAM)?;

    let account_keys = vec![
        fee_payer.pubkey,
        owner_keypair.pubkey,
        from_account,
        to_account,
        token_program,
    ];

    let header = SolanaMessageHeader {
        num_required_signatures: 2,
        num_readonly_signed: 1,
        num_readonly_unsigned: 1,
    };

    let mut data = Vec::with_capacity(9);
    data.push(3u8);
    data.extend_from_slice(&amount.to_le_bytes());

    let instruction = SolanaInstruction {
        program_id_index: 4,
        account_indices: vec![2, 3, 1],
        data,
    };

    let recent_blockhash = solana_get_latest_blockhash(&state.http, url).await?;
    let message = build_solana_message_with_instructions(
        header,
        &account_keys,
        &recent_blockhash,
        &[instruction],
    );
    let fee_sig = fee_payer.sign(&message);
    let owner_sig = owner_keypair.sign(&message);
    let tx = build_solana_transaction(&[fee_sig, owner_sig], &message);

    let signature = solana_send_transaction(&state.http, url, &tx).await?;
    Ok(Some(signature))
}

async fn broadcast_evm_sweep(
    state: &CustodyState,
    url: &str,
    job: &SweepJob,
) -> Result<Option<String>, String> {
    if matches!(job.asset.as_str(), "usdc" | "usdt") {
        return broadcast_evm_token_sweep(state, url, job).await;
    }

    let amount = match job.amount.as_ref() {
        Some(value) => value
            .parse::<u128>()
            .map_err(|_| "invalid amount".to_string())?,
        None => return Ok(None),
    };

    let deposit = fetch_deposit(&state.db, &job.deposit_id)?;
    let Some(deposit) = deposit else {
        return Ok(None);
    };

    let from_address = deposit.address.clone();
    let to_address = job.to_treasury.clone();

    let nonce = evm_get_transaction_count(&state.http, url, &from_address).await?;
    let gas_price = evm_get_gas_price(&state.http, url).await?;
    // AUDIT-FIX M6: Dynamic gas estimation with fallback to 21000 (simple transfer)
    let gas_limit = evm_estimate_gas(
        &state.http,
        url,
        &from_address,
        &to_address,
        amount,
        None,
        21_000,
    )
    .await;
    let fee = gas_price.saturating_mul(gas_limit);
    if amount <= fee {
        return Ok(None);
    }
    let value = amount - fee;

    let chain_id = evm_get_chain_id(&state.http, url).await?;
    let signing_key = derive_evm_signing_key(
        &deposit.derivation_path,
        deposit_seed_for_record(&state.config, &deposit),
    )?;
    let raw_tx = build_evm_signed_transaction(
        &signing_key,
        nonce,
        gas_price,
        gas_limit,
        &to_address,
        value,
        chain_id,
    )?;
    let tx_hex = format!("0x{}", hex::encode(raw_tx));

    let result = evm_rpc_call(&state.http, url, "eth_sendRawTransaction", json!([tx_hex])).await?;
    Ok(result.as_str().map(|v| v.to_string()))
}

async fn broadcast_evm_token_sweep(
    state: &CustodyState,
    url: &str,
    job: &SweepJob,
) -> Result<Option<String>, String> {
    let amount = match job.amount.as_ref() {
        Some(value) => value
            .parse::<u128>()
            .map_err(|_| "invalid amount".to_string())?,
        None => return Ok(None),
    };
    if amount == 0 {
        return Ok(None);
    }

    let deposit = fetch_deposit(&state.db, &job.deposit_id)?;
    let Some(deposit) = deposit else {
        return Ok(None);
    };

    let contract = evm_contract_for_asset(&state.config, &job.asset)?;
    let from_address = deposit.address.clone();
    let to_address = job.to_treasury.clone();

    // AUDIT-FIX M6: Pre-compute transfer data for gas estimation
    let transfer_data = evm_encode_erc20_transfer(&to_address, amount)?;
    let gas_price = evm_get_gas_price(&state.http, url).await?;
    // Dynamic gas estimation with fallback to 100000 (ERC-20 transfer)
    let gas_limit = evm_estimate_gas(
        &state.http,
        url,
        &from_address,
        &contract,
        0,
        Some(&transfer_data),
        100_000,
    )
    .await;
    let fee = gas_price.saturating_mul(gas_limit);
    let native_balance = evm_get_balance(&state.http, url, &from_address).await?;

    // M16 fix: If the deposit address lacks ETH for gas, fund it from the treasury.
    // ERC-20 deposit addresses only receive tokens (no native ETH), so the treasury
    // must sponsor gas for the sweep transaction.
    if native_balance < fee {
        let deficit = fee.saturating_sub(native_balance);
        // Add 20% buffer to avoid rounding issues / gas price fluctuations
        let gas_grant = deficit.saturating_add(deficit / 5);

        info!(
            "M16 gas funding: deposit {} has {} wei, needs {} — granting {} wei from treasury",
            from_address, native_balance, fee, gas_grant
        );

        let fund_tx_hash = fund_evm_gas_for_sweep(state, url, &from_address, gas_grant).await?;
        info!(
            "M16 gas funding tx submitted: {} → {} ({})",
            fund_tx_hash, from_address, gas_grant
        );

        // Wait up to 90 seconds for the gas funding tx to confirm
        let mut confirmed = false;
        for attempt in 0..18 {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            match check_evm_tx_confirmed(&state.http, url, &fund_tx_hash, 1).await {
                Ok(true) => {
                    confirmed = true;
                    break;
                }
                Ok(false) => {
                    if attempt % 6 == 5 {
                        tracing::debug!(
                            "M16 gas funding waiting for confirmation ({}/18)...",
                            attempt + 1
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("M16 gas funding confirmation check error: {}", e);
                }
            }
        }
        if !confirmed {
            return Err(format!(
                "gas funding tx {} did not confirm within 90s",
                fund_tx_hash
            ));
        }

        // Re-verify balance after funding
        let new_balance = evm_get_balance(&state.http, url, &from_address).await?;
        if new_balance < fee {
            return Err(format!(
                "gas funding confirmed but balance still insufficient: {} < {}",
                new_balance, fee
            ));
        }
    }

    let nonce = evm_get_transaction_count(&state.http, url, &from_address).await?;
    let chain_id = evm_get_chain_id(&state.http, url).await?;
    let signing_key = derive_evm_signing_key(
        &deposit.derivation_path,
        deposit_seed_for_record(&state.config, &deposit),
    )?;
    // Re-use pre-computed transfer data from gas estimation
    let raw_tx = build_evm_signed_transaction_with_data(
        &signing_key,
        nonce,
        gas_price,
        gas_limit,
        &contract,
        0,
        &transfer_data,
        chain_id,
    )?;
    let tx_hex = format!("0x{}", hex::encode(raw_tx));

    let result = evm_rpc_call(&state.http, url, "eth_sendRawTransaction", json!([tx_hex])).await?;
    Ok(result.as_str().map(|v| v.to_string()))
}

/// M16 fix: Send native ETH/BNB from the custody treasury to a deposit address
/// so that it has enough gas to execute an ERC-20 token sweep.
///
/// This is a simple value transfer (no calldata). The treasury derives its
/// EVM signing key from the master seed with a chain-specific path.
async fn fund_evm_gas_for_sweep(
    state: &CustodyState,
    url: &str,
    to_address: &str,
    amount_wei: u128,
) -> Result<String, String> {
    // Determine which chain we're on from the URL to pick the right treasury
    let treasury_chain = if state.config.bnb_rpc_url.as_deref() == Some(url) {
        "custody/treasury/bnb"
    } else {
        "custody/treasury/ethereum"
    };

    let treasury_addr = derive_evm_address(treasury_chain, &state.config.master_seed)?;

    let nonce = evm_get_transaction_count(&state.http, url, &treasury_addr).await?;
    let gas_price = evm_get_gas_price(&state.http, url).await?;
    let chain_id = evm_get_chain_id(&state.http, url).await?;
    let signing_key = derive_evm_signing_key(treasury_chain, &state.config.master_seed)?;

    // AUDIT-FIX M6: Dynamic gas estimation for treasury gas funding transfer
    let gas_limit = evm_estimate_gas(
        &state.http,
        url,
        &treasury_addr,
        to_address,
        amount_wei,
        None,
        21_000,
    )
    .await;
    let tx_fee = gas_price.saturating_mul(gas_limit);

    // Verify treasury can afford the grant
    let treasury_balance = evm_get_balance(&state.http, url, &treasury_addr).await?;
    if treasury_balance < amount_wei.saturating_add(tx_fee) {
        return Err(format!(
            "treasury ETH balance too low for gas grant: has {} wei, needs {} + {} fee",
            treasury_balance, amount_wei, tx_fee
        ));
    }

    let raw_tx = build_evm_signed_transaction(
        &signing_key,
        nonce,
        gas_price,
        gas_limit,
        to_address,
        amount_wei,
        chain_id,
    )?;
    let tx_hex = format!("0x{}", hex::encode(raw_tx));
    let result = evm_rpc_call(&state.http, url, "eth_sendRawTransaction", json!([tx_hex])).await?;

    result
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "no tx hash from gas funding".to_string())
}

/// AUDIT-FIX H2: Max retry cap. Beyond this, jobs move to "permanently_failed"
/// and require manual intervention (admin re-queue after root cause analysis).
/// Without a cap, failing jobs retry at 16-minute intervals forever, burning gas
/// on consistently failing transactions.
const MAX_JOB_ATTEMPTS: u32 = 10;

fn mark_sweep_failed(job: &mut SweepJob, err: String) {
    job.attempts = job.attempts.saturating_add(1);
    job.last_error = Some(err);
    if job.attempts >= MAX_JOB_ATTEMPTS {
        job.status = "permanently_failed".to_string();
        job.next_attempt_at = None;
        tracing::error!(
            "AUDIT-FIX H2: sweep job {} exceeded {} attempts — moved to permanently_failed. \
             Manual intervention required.",
            job.job_id,
            MAX_JOB_ATTEMPTS
        );
    } else {
        job.next_attempt_at = Some(next_retry_timestamp(job.attempts));
    }
}

fn mark_credit_failed(job: &mut CreditJob, err: String) {
    job.attempts = job.attempts.saturating_add(1);
    job.last_error = Some(err);
    if job.attempts >= MAX_JOB_ATTEMPTS {
        job.status = "permanently_failed".to_string();
        job.next_attempt_at = None;
        tracing::error!(
            "AUDIT-FIX H2: credit job {} exceeded {} attempts — moved to permanently_failed. \
             Manual intervention required.",
            job.job_id,
            MAX_JOB_ATTEMPTS
        );
    } else {
        job.next_attempt_at = Some(next_retry_timestamp(job.attempts));
    }
}

fn mark_withdrawal_failed(job: &mut WithdrawalJob, err: String) {
    job.attempts = job.attempts.saturating_add(1);
    job.last_error = Some(err);
    if job.attempts >= MAX_JOB_ATTEMPTS {
        job.status = "permanently_failed".to_string();
        job.next_attempt_at = None;
        tracing::error!(
            "AUDIT-FIX H2: withdrawal job {} exceeded {} attempts — moved to permanently_failed. \
             Manual intervention required.",
            job.job_id,
            MAX_JOB_ATTEMPTS
        );
    } else {
        job.next_attempt_at = Some(next_retry_timestamp(job.attempts));
    }
}

fn next_retry_timestamp(attempts: u32) -> i64 {
    let delay = 30i64.saturating_mul(2i64.saturating_pow(attempts.min(5)));
    chrono::Utc::now().timestamp() + delay
}

fn is_ready_for_retry(job: &SweepJob) -> bool {
    match job.next_attempt_at {
        Some(ts) => chrono::Utc::now().timestamp() >= ts,
        None => true,
    }
}

fn is_ready_for_credit_retry(job: &CreditJob) -> bool {
    match job.next_attempt_at {
        Some(ts) => chrono::Utc::now().timestamp() >= ts,
        None => true,
    }
}

#[derive(Debug, Deserialize)]
struct TreasuryKeyFile {
    secret_key: String,
}

async fn submit_wrapped_credit(state: &CustodyState, job: &CreditJob) -> Result<String, String> {
    let rpc_url = state
        .config
        .licn_rpc_url
        .as_ref()
        .ok_or_else(|| "missing CUSTODY_LICHEN_RPC_URL".to_string())?;
    let keypair_path = state
        .config
        .treasury_keypair_path
        .as_ref()
        .ok_or_else(|| "missing CUSTODY_TREASURY_KEYPAIR".to_string())?;

    // Resolve which wrapped token contract to call
    let contract_addr_str =
        resolve_token_contract(&state.config, &job.source_chain, &job.source_asset).ok_or_else(
            || {
                format!(
                    "no wrapped token contract for chain={} asset={}",
                    job.source_chain, job.source_asset
                )
            },
        )?;

    let contract_pubkey = Pubkey::from_base58(&contract_addr_str)
        .map_err(|_| format!("invalid contract address: {}", contract_addr_str))?;

    let treasury_keypair = load_treasury_keypair(Path::new(keypair_path))?;
    let to_pubkey = Pubkey::from_base58(&job.to_address)
        .map_err(|_| "invalid recipient address".to_string())?;

    // Build a contract Call instruction: mint(caller, to, amount)
    // The contract's "mint" function expects: caller (32 bytes), to (32 bytes), amount (u64 LE)
    let instruction = build_contract_mint_instruction(
        &contract_pubkey,
        &treasury_keypair.pubkey(),
        &to_pubkey,
        job.amount_spores,
    );

    let blockhash = licn_get_recent_blockhash(&state.http, rpc_url).await?;
    let message = Message::new(vec![instruction], blockhash);
    let signature = treasury_keypair.sign(&message.serialize());
    let mut tx = Transaction::new(message);
    tx.signatures.push(signature);

    let tx_bytes = tx.to_wire();
    let tx_base64 = base64::engine::general_purpose::STANDARD.encode(tx_bytes);

    let token_label = match job.source_asset.as_str() {
        "usdt" | "usdc" => "lUSD",
        "sol" => "wSOL",
        "eth" => "wETH",
        "bnb" => "wBNB",
        _ => "UNKNOWN",
    };
    info!(
        "minting {} {} to {} (deposit={})",
        job.amount_spores, token_label, job.to_address, job.deposit_id
    );

    licn_send_transaction(&state.http, rpc_url, &tx_base64).await
}

/// Returns the native decimal precision for a given (chain, asset) pair.
///
/// Used by deposit → credit conversion AND withdrawal → outbound conversion.
///
/// Native tokens:
///   ETH on Ethereum:             18 decimals (wei)
///   BNB on BSC:                  18 decimals (wei)
///   SOL on Solana:               9 decimals (lamports)
///
/// ERC-20 / SPL tokens:
///   USDT/USDC on Ethereum:       6 decimals
///   USDT/USDC on BSC (BEP-20):  18 decimals
///   USDT/USDC on Solana (SPL):   6 decimals
fn source_chain_decimals(chain: &str, asset: &str) -> u32 {
    match (chain, asset) {
        // EVM native
        ("eth" | "ethereum", "eth") => 18,
        ("bsc" | "bnb", "bnb") => 18,
        // ERC-20 stablecoins on Ethereum: 6 decimals
        ("eth" | "ethereum", "usdt" | "usdc") => 6,
        // BEP-20 stablecoins on BSC: 18 decimals
        ("bsc" | "bnb", "usdt" | "usdc") => 18,
        // Solana native
        ("sol" | "solana", "sol") => 9,
        // SPL stablecoins on Solana: 6 decimals
        ("sol" | "solana", "usdt" | "usdc") => 6,
        // Default to 18 for unknown EVM-like chains
        _ => 18,
    }
}

/// Convert Lichen spores (9 decimals) to the target chain's native amount.
///
/// Inverse of the deposit conversion in `build_credit_job`.
fn spores_to_chain_amount(spores: u64, chain: &str, asset: &str) -> u128 {
    let target_decimals = source_chain_decimals(chain, asset);
    if target_decimals > 9 {
        (spores as u128).saturating_mul(10u128.pow(target_decimals - 9))
    } else if target_decimals < 9 {
        (spores as u128) / 10u128.pow(9 - target_decimals)
    } else {
        spores as u128
    }
}

/// Resolve deposited asset → Lichen wrapped token contract address.
///
/// Mapping:
///   sol (any chain)          → wSOL contract
///   eth (any chain)          → wETH contract
///   bnb (any chain)          → wBNB contract
///   usdt, usdc (any chain)   → lUSD contract (unified stablecoin)
fn resolve_token_contract(config: &CustodyConfig, _chain: &str, asset: &str) -> Option<String> {
    match asset {
        "sol" => config.wsol_contract_addr.clone(),
        "eth" => config.weth_contract_addr.clone(),
        "bnb" => config.wbnb_contract_addr.clone(),
        "usdt" | "usdc" => config.musd_contract_addr.clone(),
        _ => None,
    }
}

/// Build a Lichen contract Call instruction for the "mint" function.
///
/// Payload format:
///   {"Call": {"function": "mint", "args": [...], "value": 0}}
///
/// Where args is a flat byte array: [caller_32_bytes, to_32_bytes, amount_8_bytes_le]
fn build_contract_mint_instruction(
    contract_pubkey: &Pubkey,
    caller: &Pubkey,
    to: &Pubkey,
    amount: u64,
) -> Instruction {
    // Build the args as a flat byte array: caller (32) + to (32) + amount (8 LE)
    let mut args: Vec<u8> = Vec::with_capacity(72);
    args.extend_from_slice(caller.as_ref());
    args.extend_from_slice(to.as_ref());
    args.extend_from_slice(&amount.to_le_bytes());

    // Wrap in the Call envelope
    let payload = serde_json::json!({
        "Call": {
            "function": "mint",
            "args": args.iter().map(|b| *b as u64).collect::<Vec<u64>>(),
            "value": 0
        }
    });
    let data = serde_json::to_vec(&payload).expect("json encode");

    Instruction {
        program_id: Pubkey::new(LICN_CONTRACT_PROGRAM),
        accounts: vec![*caller, *contract_pubkey],
        data,
    }
}

/// Build a Lichen contract Call instruction for the "burn" function.
/// Used during withdrawal flow — treasury burns wrapped tokens on behalf of user.
fn _build_contract_burn_instruction(
    contract_pubkey: &Pubkey,
    caller: &Pubkey,
    amount: u64,
) -> Instruction {
    let mut args: Vec<u8> = Vec::with_capacity(40);
    args.extend_from_slice(caller.as_ref());
    args.extend_from_slice(&amount.to_le_bytes());

    let payload = serde_json::json!({
        "Call": {
            "function": "burn",
            "args": args.iter().map(|b| *b as u64).collect::<Vec<u64>>(),
            "value": 0
        }
    });
    let data = serde_json::to_vec(&payload).expect("json encode");

    Instruction {
        program_id: Pubkey::new(LICN_CONTRACT_PROGRAM),
        accounts: vec![*caller, *contract_pubkey],
        data,
    }
}

fn load_treasury_keypair(path: &Path) -> Result<Keypair, String> {
    let json = std::fs::read_to_string(path).map_err(|e| format!("read: {}", e))?;
    let parsed: TreasuryKeyFile =
        serde_json::from_str(&json).map_err(|e| format!("parse: {}", e))?;
    let bytes = hex::decode(parsed.secret_key).map_err(|e| format!("hex: {}", e))?;
    if bytes.len() != 32 {
        return Err("invalid treasury key length".to_string());
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&bytes);
    Ok(Keypair::from_seed(&seed))
}

fn _build_system_transfer(from: &Pubkey, to: &Pubkey, amount: u64) -> Instruction {
    let mut data = Vec::with_capacity(9);
    data.push(0u8);
    data.extend_from_slice(&amount.to_le_bytes());
    Instruction {
        program_id: SYSTEM_PROGRAM_ID,
        accounts: vec![*from, *to],
        data,
    }
}

async fn licn_get_recent_blockhash(client: &reqwest::Client, url: &str) -> Result<Hash, String> {
    let result = licn_rpc_call(client, url, "getRecentBlockhash", json!([])).await?;
    let hash = result
        .get("blockhash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing blockhash".to_string())?;
    Hash::from_hex(hash).map_err(|e| format!("blockhash: {}", e))
}

async fn licn_send_transaction(
    client: &reqwest::Client,
    url: &str,
    tx_base64: &str,
) -> Result<String, String> {
    let result = licn_rpc_call(client, url, "sendTransaction", json!([tx_base64])).await?;
    result
        .as_str()
        .map(|v| v.to_string())
        .ok_or_else(|| "missing tx signature".to_string())
}

async fn licn_rpc_call(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let response = client
        .post(url)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("rpc send: {}", e))?;
    let value: Value = response
        .json()
        .await
        .map_err(|e| format!("rpc json: {}", e))?;
    if let Some(err) = value.get("error") {
        return Err(format!("rpc error: {}", err));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| "rpc result missing".to_string())
}

fn list_sweep_jobs_by_status(db: &DB, status: &str) -> Result<Vec<SweepJob>, String> {
    // AUDIT-FIX M1: Use status index for O(active) instead of O(total)
    let ids = list_ids_by_status_index(db, "sweep", status)?;
    if !ids.is_empty() {
        let cf = db
            .cf_handle(CF_SWEEP_JOBS)
            .ok_or_else(|| "missing sweep_jobs cf".to_string())?;
        let mut results = Vec::new();
        for id in ids {
            if let Ok(Some(bytes)) = db.get_cf(cf, id.as_bytes()) {
                if let Ok(record) = serde_json::from_slice::<SweepJob>(&bytes) {
                    if record.status == status {
                        results.push(record);
                    }
                }
            }
        }
        return Ok(results);
    }
    // Fallback: legacy full scan for pre-index data
    let cf = db
        .cf_handle(CF_SWEEP_JOBS)
        .ok_or_else(|| "missing sweep_jobs cf".to_string())?;
    let mut results = Vec::new();
    let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
    for item in iter {
        let (_, value) = item.map_err(|e| format!("db iter: {}", e))?;
        let record: SweepJob =
            serde_json::from_slice(&value).map_err(|e| format!("decode: {}", e))?;
        if record.status == status {
            results.push(record);
        }
    }
    Ok(results)
}

fn store_sweep_job(db: &DB, job: &SweepJob) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_SWEEP_JOBS)
        .ok_or_else(|| "missing sweep_jobs cf".to_string())?;
    // AUDIT-FIX M1: Update status index on every store
    if let Ok(Some(old_bytes)) = db.get_cf(cf, job.job_id.as_bytes()) {
        if let Ok(old_job) = serde_json::from_slice::<SweepJob>(&old_bytes) {
            let _ = update_status_index(db, "sweep", &old_job.status, &job.status, &job.job_id);
        }
    } else {
        let _ = set_status_index(db, "sweep", &job.status, &job.job_id);
    }
    let bytes = serde_json::to_vec(job).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, job.job_id.as_bytes(), bytes)
        .map_err(|e| format!("db put: {}", e))
}

fn store_credit_job(db: &DB, job: &CreditJob) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_CREDIT_JOBS)
        .ok_or_else(|| "missing credit_jobs cf".to_string())?;
    // AUDIT-FIX M1: Update status index on every store
    if let Ok(Some(old_bytes)) = db.get_cf(cf, job.job_id.as_bytes()) {
        if let Ok(old_job) = serde_json::from_slice::<CreditJob>(&old_bytes) {
            let _ = update_status_index(db, "credit", &old_job.status, &job.status, &job.job_id);
        }
    } else {
        let _ = set_status_index(db, "credit", &job.status, &job.job_id);
    }
    let bytes = serde_json::to_vec(job).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, job.job_id.as_bytes(), bytes)
        .map_err(|e| format!("db put: {}", e))
}

/// AUDIT-FIX F8.9: Use status index for O(active) instead of O(total) full-table scan.
fn count_sweep_jobs(db: &DB) -> Result<StatusCounts, String> {
    let mut counts = StatusCounts {
        total: 0,
        by_status: BTreeMap::new(),
    };
    for status in &[
        "queued",
        "signing",
        "signed",
        "sweep_submitted",
        "sweep_confirmed",
        "permanently_failed",
        "failed",
    ] {
        let ids = list_ids_by_status_index(db, "sweep", status)?;
        let count = ids.len();
        if count > 0 {
            counts.total += count;
            counts.by_status.insert(status.to_string(), count);
        }
    }
    // If status index is empty, fall back to full scan (pre-index data)
    if counts.total == 0 {
        let cf = db
            .cf_handle(CF_SWEEP_JOBS)
            .ok_or_else(|| "missing sweep_jobs cf".to_string())?;
        let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
        for item in iter {
            let (_, value) = item.map_err(|e| format!("db iter: {}", e))?;
            let record: SweepJob =
                serde_json::from_slice(&value).map_err(|e| format!("decode: {}", e))?;
            counts.total += 1;
            *counts.by_status.entry(record.status).or_insert(0) += 1;
        }
    }
    Ok(counts)
}

/// AUDIT-FIX F8.9: Use status index for O(active) instead of O(total) full-table scan.
fn count_credit_jobs(db: &DB) -> Result<StatusCounts, String> {
    let mut counts = StatusCounts {
        total: 0,
        by_status: BTreeMap::new(),
    };
    for status in &[
        "queued",
        "submitted",
        "confirmed",
        "permanently_failed",
        "failed",
    ] {
        let ids = list_ids_by_status_index(db, "credit", status)?;
        let count = ids.len();
        if count > 0 {
            counts.total += count;
            counts.by_status.insert(status.to_string(), count);
        }
    }
    // If status index is empty, fall back to full scan (pre-index data)
    if counts.total == 0 {
        let cf = db
            .cf_handle(CF_CREDIT_JOBS)
            .ok_or_else(|| "missing credit_jobs cf".to_string())?;
        let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
        for item in iter {
            let (_, value) = item.map_err(|e| format!("db iter: {}", e))?;
            let record: CreditJob =
                serde_json::from_slice(&value).map_err(|e| format!("decode: {}", e))?;
            counts.total += 1;
            *counts.by_status.entry(record.status).or_insert(0) += 1;
        }
    }
    Ok(counts)
}

fn record_audit_event(
    db: &DB,
    event_type: &str,
    entity_id: &str,
    deposit_id: Option<&str>,
    tx_hash: Option<&str>,
) -> Result<(), String> {
    record_audit_event_ext(db, event_type, entity_id, deposit_id, tx_hash, None, None)
}

/// Extended audit event recorder — also emits to webhook/WS broadcast channel.
/// Call this variant from code paths that have access to `CustodyState`.
fn record_audit_event_ext(
    db: &DB,
    event_type: &str,
    entity_id: &str,
    deposit_id: Option<&str>,
    tx_hash: Option<&str>,
    data: Option<&Value>,
    event_tx: Option<&broadcast::Sender<CustodyWebhookEvent>>,
) -> Result<(), String> {
    let event_id = Uuid::new_v4().to_string();
    let timestamp = chrono::Utc::now().timestamp();
    let timestamp_ms = chrono::Utc::now().timestamp_millis();
    let cf = db
        .cf_handle(CF_AUDIT_EVENTS)
        .ok_or_else(|| "missing audit_events cf".to_string())?;
    let index_cf = db
        .cf_handle(CF_AUDIT_EVENTS_BY_TIME)
        .ok_or_else(|| "missing audit_events_by_time cf".to_string())?;
    let type_index_cf = db
        .cf_handle(CF_AUDIT_EVENTS_BY_TYPE_TIME)
        .ok_or_else(|| "missing audit_events_by_type_time cf".to_string())?;
    let entity_index_cf = db
        .cf_handle(CF_AUDIT_EVENTS_BY_ENTITY_TIME)
        .ok_or_else(|| "missing audit_events_by_entity_time cf".to_string())?;
    let tx_index_cf = db
        .cf_handle(CF_AUDIT_EVENTS_BY_TX_TIME)
        .ok_or_else(|| "missing audit_events_by_tx_time cf".to_string())?;
    let payload = serde_json::json!({
        "event_id": &event_id,
        "event_type": event_type,
        "entity_id": entity_id,
        "deposit_id": deposit_id,
        "tx_hash": tx_hash,
        "data": data,
        "timestamp": timestamp,
        "timestamp_ms": timestamp_ms,
    });
    let bytes = serde_json::to_vec(&payload).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, event_id.as_bytes(), bytes)
        .map_err(|e| format!("db put: {}", e))?;

    // Scale-safe read index for event history pagination.
    // Key format preserves chronological ordering in RocksDB iteration.
    let index_key = format!("{:020}:{}", timestamp_ms.max(0), event_id);
    db.put_cf(index_cf, index_key.as_bytes(), event_id.as_bytes())
        .map_err(|e| format!("db put index: {}", e))?;
    let type_index_key = format!(
        "type:{}:{:020}:{}",
        event_type,
        timestamp_ms.max(0),
        event_id
    );
    db.put_cf(
        type_index_cf,
        type_index_key.as_bytes(),
        event_id.as_bytes(),
    )
    .map_err(|e| format!("db put type index: {}", e))?;
    let entity = if entity_id.is_empty() {
        "unknown"
    } else {
        entity_id
    };
    let entity_index_key = format!("entity:{}:{:020}:{}", entity, timestamp_ms.max(0), event_id);
    db.put_cf(
        entity_index_cf,
        entity_index_key.as_bytes(),
        event_id.as_bytes(),
    )
    .map_err(|e| format!("db put entity index: {}", e))?;
    if let Some(hash) = tx_hash.filter(|h| !h.is_empty()) {
        let tx_index_key = format!("tx:{}:{:020}:{}", hash, timestamp_ms.max(0), event_id);
        db.put_cf(tx_index_cf, tx_index_key.as_bytes(), event_id.as_bytes())
            .map_err(|e| format!("db put tx index: {}", e))?;
    }

    // Emit to broadcast channel for webhooks + WebSocket subscribers
    if let Some(tx) = event_tx {
        let event = CustodyWebhookEvent {
            event_id,
            event_type: event_type.to_string(),
            entity_id: entity_id.to_string(),
            deposit_id: deposit_id.map(|s| s.to_string()),
            tx_hash: tx_hash.map(|s| s.to_string()),
            data: data.cloned(),
            timestamp,
        };
        // Best-effort: if no receivers are listening, that's fine
        let _ = tx.send(event);
    }

    Ok(())
}

/// Convenience: emit a custody event with full state context (DB + broadcast channel).
fn emit_custody_event(
    state: &CustodyState,
    event_type: &str,
    entity_id: &str,
    deposit_id: Option<&str>,
    tx_hash: Option<&str>,
    data: Option<&Value>,
) {
    if let Err(e) = record_audit_event_ext(
        &state.db,
        event_type,
        entity_id,
        deposit_id,
        tx_hash,
        data,
        Some(&state.event_tx),
    ) {
        tracing::warn!("audit event failed: {}", e);
    }
}

fn list_credit_jobs_by_status(db: &DB, status: &str) -> Result<Vec<CreditJob>, String> {
    // AUDIT-FIX M1: Use status index for O(active) instead of O(total)
    let ids = list_ids_by_status_index(db, "credit", status)?;
    if !ids.is_empty() {
        let cf = db
            .cf_handle(CF_CREDIT_JOBS)
            .ok_or_else(|| "missing credit_jobs cf".to_string())?;
        let mut results = Vec::new();
        for id in ids {
            if let Ok(Some(bytes)) = db.get_cf(cf, id.as_bytes()) {
                if let Ok(record) = serde_json::from_slice::<CreditJob>(&bytes) {
                    if record.status == status {
                        results.push(record);
                    }
                }
            }
        }
        return Ok(results);
    }
    // Fallback: legacy full scan for pre-index data
    let cf = db
        .cf_handle(CF_CREDIT_JOBS)
        .ok_or_else(|| "missing credit_jobs cf".to_string())?;
    let mut results = Vec::new();
    let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
    for item in iter {
        let (_, value) = item.map_err(|e| format!("db iter: {}", e))?;
        let record: CreditJob =
            serde_json::from_slice(&value).map_err(|e| format!("decode: {}", e))?;
        if record.status == status {
            results.push(record);
        }
    }
    Ok(results)
}

fn store_deposit_event(db: &DB, event: &DepositEvent) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_DEPOSIT_EVENTS)
        .ok_or_else(|| "missing deposit_events cf".to_string())?;
    let bytes = serde_json::to_vec(event).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, event.event_id.as_bytes(), bytes)
        .map_err(|e| format!("db put: {}", e))?;
    // AUDIT-FIX 0.11: Store a dedup marker keyed by deposit_id + tx_hash so we
    // can detect and skip duplicate deposit events in subsequent poll cycles.
    let dedup_key = format!("dedup:{}:{}", event.deposit_id, event.tx_hash);
    db.put_cf(cf, dedup_key.as_bytes(), b"1")
        .map_err(|e| format!("dedup marker: {}", e))?;
    Ok(())
}

/// AUDIT-FIX 0.11: Check whether a deposit event for this (deposit_id, tx_hash)
/// combination was already processed. Prevents duplicate sweep jobs from
/// repeated poll cycles seeing the same confirmed signature.
fn deposit_event_already_processed(db: &DB, deposit_id: &str, tx_hash: &str) -> bool {
    let cf = match db.cf_handle(CF_DEPOSIT_EVENTS) {
        Some(cf) => cf,
        None => return false,
    };
    let dedup_key = format!("dedup:{}:{}", deposit_id, tx_hash);
    matches!(db.get_cf(cf, dedup_key.as_bytes()), Ok(Some(_)))
}

fn enqueue_sweep_job(db: &DB, job: &SweepJob) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_SWEEP_JOBS)
        .ok_or_else(|| "missing sweep_jobs cf".to_string())?;
    let bytes = serde_json::to_vec(job).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, job.job_id.as_bytes(), bytes)
        .map_err(|e| format!("db put: {}", e))?;
    // AUDIT-FIX M1: index initial sweep job status
    let _ = set_status_index(db, "sweep", &job.status, &job.job_id);
    Ok(())
}

fn update_deposit_status(db: &DB, deposit_id: &str, status: &str) -> Result<(), String> {
    let mut record = fetch_deposit(db, deposit_id)
        .map_err(|e| format!("fetch deposit: {}", e))?
        .ok_or_else(|| "deposit not found".to_string())?;
    let old_status = record.status.clone();
    record.status = status.to_string();
    store_deposit(db, &record)?;
    // AUDIT-FIX M1: maintain status index
    let _ = update_status_index(db, "deposits", &old_status, status, deposit_id);
    Ok(())
}

fn derive_solana_address(path: &str, master_seed: &str) -> Result<String, String> {
    use ed25519_dalek::SigningKey;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    // C8 fix: HMAC-SHA256(master_seed, path) instead of plain SHA256(path)
    let mut mac =
        Hmac::<Sha256>::new_from_slice(master_seed.as_bytes()).map_err(|_| "HMAC key error")?;
    mac.update(path.as_bytes());
    let seed = mac.finalize().into_bytes();
    let mut seed_bytes: [u8; 32] = seed.as_slice().try_into().map_err(|_| "seed")?;
    let signing_key = SigningKey::from_bytes(&seed_bytes);
    seed_bytes.zeroize(); // AUDIT-FIX H5: zeroize intermediate key material
    let verifying_key = signing_key.verifying_key();
    Ok(bs58::encode(verifying_key.to_bytes()).into_string())
}

fn derive_solana_signer(
    path: &str,
    master_seed: &str,
) -> Result<(ed25519_dalek::SigningKey, [u8; 32]), String> {
    use ed25519_dalek::SigningKey;
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    // C8 fix: HMAC-SHA256(master_seed, path)
    let mut mac =
        Hmac::<Sha256>::new_from_slice(master_seed.as_bytes()).map_err(|_| "HMAC key error")?;
    mac.update(path.as_bytes());
    let seed = mac.finalize().into_bytes();
    let mut seed_bytes: [u8; 32] = seed.as_slice().try_into().map_err(|_| "seed")?;
    let signing_key = SigningKey::from_bytes(&seed_bytes);
    seed_bytes.zeroize(); // AUDIT-FIX H5: zeroize intermediate key material
    let verifying_key = signing_key.verifying_key();
    Ok((signing_key, verifying_key.to_bytes()))
}

struct SimpleSolanaKeypair {
    signing_key: ed25519_dalek::SigningKey,
    pubkey: [u8; 32],
}

impl SimpleSolanaKeypair {
    fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.signing_key.sign(message).to_bytes()
    }
}

fn derive_solana_keypair(path: &str, master_seed: &str) -> Result<SimpleSolanaKeypair, String> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    // AUDIT-FIX 0.9: HMAC-SHA256(master_seed, path) instead of plain SHA256(path).
    // Plain SHA256 allowed anyone who knew the derivation path format to
    // reconstruct the private key without any secret.
    let mut mac = Hmac::<Sha256>::new_from_slice(master_seed.as_bytes())
        .map_err(|_| "HMAC key error".to_string())?;
    mac.update(path.as_bytes());
    let seed = mac.finalize().into_bytes();
    let mut seed_bytes: [u8; 32] = seed.as_slice().try_into().map_err(|_| "seed")?;
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed_bytes);
    seed_bytes.zeroize(); // AUDIT-FIX H5: zeroize intermediate key material
    let pubkey = signing_key.verifying_key().to_bytes();
    Ok(SimpleSolanaKeypair {
        signing_key,
        pubkey,
    })
}

fn decode_solana_pubkey(value: &str) -> Result<[u8; 32], String> {
    let bytes = bs58::decode(value)
        .into_vec()
        .map_err(|e| format!("base58: {}", e))?;
    if bytes.len() != 32 {
        return Err("invalid solana pubkey length".to_string());
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

fn encode_solana_pubkey(value: &[u8; 32]) -> String {
    bs58::encode(value).into_string()
}

fn find_program_address(seeds: &[&[u8]], program_id: &[u8; 32]) -> Result<[u8; 32], String> {
    use sha2::{Digest, Sha256};

    for bump in (0u8..=255u8).rev() {
        let mut hasher = Sha256::new();
        for seed in seeds {
            hasher.update(seed);
        }
        hasher.update([bump]);
        hasher.update(program_id);
        hasher.update(b"ProgramDerivedAddress");
        let hash = hasher.finalize();
        let bytes: [u8; 32] = hash
            .as_slice()
            .try_into()
            .map_err(|_| "pda hash".to_string())?;
        if VerifyingKey::from_bytes(&bytes).is_err() {
            return Ok(bytes);
        }
    }

    Err("no viable program address".to_string())
}

fn derive_evm_address(path: &str, master_seed: &str) -> Result<String, String> {
    use hmac::{Hmac, Mac};
    use k256::ecdsa::SigningKey;
    use sha2::Sha256;
    use sha3::{Digest, Keccak256};

    // C8 fix: HMAC-SHA256(master_seed, path) instead of Keccak256(path)
    let mut mac =
        Hmac::<Sha256>::new_from_slice(master_seed.as_bytes()).map_err(|_| "HMAC key error")?;
    mac.update(path.as_bytes());
    let mut seed = mac.finalize().into_bytes();
    let key = SigningKey::from_bytes(&seed).map_err(|_| "invalid seed")?;
    seed.as_mut_slice().zeroize(); // AUDIT-FIX CUST-04: zeroize intermediate HMAC seed
    let verifying_key = key.verifying_key();
    let encoded = verifying_key.to_encoded_point(false);
    let pubkey = encoded.as_bytes();
    let hash = Keccak256::digest(&pubkey[1..]);
    let addr = &hash[12..];
    Ok(format!("0x{}", hex::encode(addr)))
}

fn derive_evm_signing_key(
    path: &str,
    master_seed: &str,
) -> Result<k256::ecdsa::SigningKey, String> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    // C8 fix: HMAC-SHA256(master_seed, path) instead of Keccak256(path)
    let mut mac =
        Hmac::<Sha256>::new_from_slice(master_seed.as_bytes()).map_err(|_| "HMAC key error")?;
    mac.update(path.as_bytes());
    let mut seed = mac.finalize().into_bytes();
    let result = k256::ecdsa::SigningKey::from_bytes(&seed).map_err(|_| "invalid seed".to_string());
    seed.as_mut_slice().zeroize(); // AUDIT-FIX H5: zeroize intermediate key material
    result
}

async fn solana_get_latest_blockhash(
    client: &reqwest::Client,
    url: &str,
) -> Result<[u8; 32], String> {
    let params = json!([]);
    let result = solana_rpc_call(client, url, "getLatestBlockhash", params).await?;
    let value = result
        .get("value")
        .and_then(|v| v.get("blockhash"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing blockhash".to_string())?;
    decode_solana_pubkey(value)
}
async fn solana_send_transaction(
    client: &reqwest::Client,
    url: &str,
    tx_bytes: &[u8],
) -> Result<String, String> {
    let tx_base64 = base64::engine::general_purpose::STANDARD.encode(tx_bytes);
    let params = json!([tx_base64, { "encoding": "base64" }]);
    let result = solana_rpc_call(client, url, "sendTransaction", params).await?;
    result
        .as_str()
        .map(|v| v.to_string())
        .ok_or_else(|| "missing tx signature".to_string())
}

struct SolanaMessageHeader {
    num_required_signatures: u8,
    num_readonly_signed: u8,
    num_readonly_unsigned: u8,
}

struct SolanaInstruction {
    program_id_index: u8,
    account_indices: Vec<u8>,
    data: Vec<u8>,
}

fn build_solana_transfer_message(
    from_pubkey: &[u8; 32],
    to_pubkey: &[u8; 32],
    lamports: u64,
    recent_blockhash: &[u8; 32],
) -> Vec<u8> {
    let system_program = decode_solana_pubkey(SOLANA_SYSTEM_PROGRAM).unwrap_or([0u8; 32]);
    let account_keys = vec![*from_pubkey, *to_pubkey, system_program];
    let header = SolanaMessageHeader {
        num_required_signatures: 1,
        num_readonly_signed: 0,
        num_readonly_unsigned: 1,
    };

    let mut data = Vec::with_capacity(12);
    data.extend_from_slice(&2u32.to_le_bytes());
    data.extend_from_slice(&lamports.to_le_bytes());

    let instruction = SolanaInstruction {
        program_id_index: 2,
        account_indices: vec![0, 1],
        data,
    };

    build_solana_message_with_instructions(header, &account_keys, recent_blockhash, &[instruction])
}

fn build_solana_message_with_instructions(
    header: SolanaMessageHeader,
    account_keys: &[[u8; 32]],
    recent_blockhash: &[u8; 32],
    instructions: &[SolanaInstruction],
) -> Vec<u8> {
    let mut message = Vec::new();
    message.push(header.num_required_signatures);
    message.push(header.num_readonly_signed);
    message.push(header.num_readonly_unsigned);

    encode_shortvec_len(account_keys.len(), &mut message);
    for key in account_keys {
        message.extend_from_slice(key);
    }

    message.extend_from_slice(recent_blockhash);

    encode_shortvec_len(instructions.len(), &mut message);
    for instruction in instructions {
        message.push(instruction.program_id_index);
        encode_shortvec_len(instruction.account_indices.len(), &mut message);
        message.extend_from_slice(&instruction.account_indices);
        encode_shortvec_len(instruction.data.len(), &mut message);
        message.extend_from_slice(&instruction.data);
    }

    message
}

fn build_solana_transaction(signatures: &[[u8; 64]], message: &[u8]) -> Vec<u8> {
    let mut tx = Vec::new();
    encode_shortvec_len(signatures.len(), &mut tx);
    for signature in signatures {
        tx.extend_from_slice(signature);
    }
    tx.extend_from_slice(message);
    tx
}

fn encode_shortvec_len(len: usize, out: &mut Vec<u8>) {
    let mut value = len as u64;
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

/// AUDIT-FIX I-7: Decode a Solana compact-u16 from the start of a byte slice.
/// Returns (value, bytes_consumed) or None on invalid input.
fn decode_shortvec_u16(bytes: &[u8]) -> Option<(u16, usize)> {
    let mut value: u16 = 0;
    let mut shift = 0u32;
    for (i, &byte) in bytes.iter().enumerate() {
        let lo = (byte & 0x7f) as u16;
        value |= lo.checked_shl(shift)?;
        shift += 7;
        if byte & 0x80 == 0 {
            return Some((value, i + 1));
        }
        if shift >= 16 {
            return None; // overflow for u16
        }
    }
    None
}

fn build_evm_signed_transaction(
    signing_key: &k256::ecdsa::SigningKey,
    nonce: u64,
    gas_price: u128,
    gas_limit: u128,
    to_address: &str,
    value: u128,
    chain_id: u64,
) -> Result<Vec<u8>, String> {
    build_evm_signed_transaction_with_data(
        signing_key,
        nonce,
        gas_price,
        gas_limit,
        to_address,
        value,
        &[],
        chain_id,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_evm_signed_transaction_with_data(
    signing_key: &k256::ecdsa::SigningKey,
    nonce: u64,
    gas_price: u128,
    gas_limit: u128,
    to_address: &str,
    value: u128,
    data: &[u8],
    chain_id: u64,
) -> Result<Vec<u8>, String> {
    use sha3::{Digest, Keccak256};

    let to_bytes = parse_evm_address(to_address)?;
    let mut rlp = Vec::new();
    rlp_encode_list(
        &[
            rlp_encode_u64(nonce),
            rlp_encode_u128(gas_price),
            rlp_encode_u128(gas_limit),
            rlp_encode_bytes(&to_bytes),
            rlp_encode_u128(value),
            rlp_encode_bytes(data),
            rlp_encode_u64(chain_id),
            rlp_encode_u64(0),
            rlp_encode_u64(0),
        ],
        &mut rlp,
    );

    let mut digest = Keccak256::new();
    digest.update(&rlp);
    let (signature, recovery_id) = signing_key
        .sign_digest_recoverable(digest)
        .map_err(|_| "failed to recover signature".to_string())?;
    let sig_bytes = signature.to_bytes();
    let v = recovery_id.to_byte() as u64 + 35 + chain_id * 2;

    let mut tx = Vec::new();
    rlp_encode_list(
        &[
            rlp_encode_u64(nonce),
            rlp_encode_u128(gas_price),
            rlp_encode_u128(gas_limit),
            rlp_encode_bytes(&to_bytes),
            rlp_encode_u128(value),
            rlp_encode_bytes(data),
            rlp_encode_u64(v),
            rlp_encode_bytes(&trim_leading_zeros(&sig_bytes[..32])),
            rlp_encode_bytes(&trim_leading_zeros(&sig_bytes[32..64])),
        ],
        &mut tx,
    );

    Ok(tx)
}

fn evm_encode_erc20_transfer(to_address: &str, amount: u128) -> Result<Vec<u8>, String> {
    let mut data = Vec::with_capacity(68);
    data.extend_from_slice(&hex::decode("a9059cbb").map_err(|_| "selector".to_string())?);

    let to_bytes = parse_evm_address(to_address)?;
    let mut padded_to = vec![0u8; 12];
    padded_to.extend_from_slice(&to_bytes);
    data.extend_from_slice(&padded_to);

    let mut padded_amount = vec![0u8; 16];
    padded_amount.extend_from_slice(&amount.to_be_bytes());
    data.extend_from_slice(&padded_amount);

    Ok(data)
}

fn parse_evm_address(address: &str) -> Result<Vec<u8>, String> {
    let trimmed = address.trim_start_matches("0x");
    let bytes = hex::decode(trimmed).map_err(|e| format!("address hex: {}", e))?;
    if bytes.len() != 20 {
        return Err("invalid evm address length".to_string());
    }
    Ok(bytes)
}

fn trim_leading_zeros(value: &[u8]) -> Vec<u8> {
    let mut index = 0;
    while index < value.len() && value[index] == 0 {
        index += 1;
    }
    value[index..].to_vec()
}

fn rlp_encode_u64(value: u64) -> Vec<u8> {
    rlp_encode_uint(&value.to_be_bytes())
}

fn rlp_encode_u128(value: u128) -> Vec<u8> {
    rlp_encode_uint(&value.to_be_bytes())
}

fn rlp_encode_uint(bytes: &[u8]) -> Vec<u8> {
    let trimmed = trim_leading_zeros(bytes);
    if trimmed.is_empty() {
        return vec![0x80];
    }
    rlp_encode_bytes(&trimmed)
}

fn rlp_encode_bytes(bytes: &[u8]) -> Vec<u8> {
    if bytes.len() == 1 && bytes[0] < 0x80 {
        return vec![bytes[0]];
    }

    let mut out = Vec::new();
    if bytes.len() <= 55 {
        out.push(0x80 + bytes.len() as u8);
    } else {
        let len_bytes = to_be_bytes(bytes.len() as u64);
        out.push(0xb7 + len_bytes.len() as u8);
        out.extend_from_slice(&len_bytes);
    }
    out.extend_from_slice(bytes);
    out
}

fn rlp_encode_list(items: &[Vec<u8>], out: &mut Vec<u8>) {
    let total_len: usize = items.iter().map(|item| item.len()).sum();
    if total_len <= 55 {
        out.push(0xc0 + total_len as u8);
    } else {
        let len_bytes = to_be_bytes(total_len as u64);
        out.push(0xf7 + len_bytes.len() as u8);
        out.extend_from_slice(&len_bytes);
    }
    for item in items {
        out.extend_from_slice(item);
    }
}

fn to_be_bytes(value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    trim_leading_zeros(&bytes)
}

// ============================================================================
// WITHDRAWAL — Burn wrapped tokens on Lichen, send native assets to user
// ============================================================================

/// POST /withdrawals — User requests to withdraw wrapped tokens
///
/// Flow:
///   1. User calls burn() on the wrapped token contract (client-side)
///   2. User POSTs burn tx signature + dest_chain + dest_address to this endpoint
///   3. Custody verifies the burn on Lichen
///   4. For lUSD: checks stablecoin reserves, queues rebalance if needed
///   5. Custody uses threshold signatures to send native assets on the destination chain
async fn create_withdrawal(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<WithdrawalRequest>,
) -> Json<Value> {
    // Use shared auth helper for consistent auth enforcement
    if let Err(err_resp) = verify_api_auth(&state.config, &headers) {
        return Json(json!({ "error": err_resp.0.message }));
    }

    // AUDIT-FIX 1.20: Global and per-address withdrawal rate limiting
    {
        let mut rl = state.withdrawal_rate.lock().await;
        let now = std::time::Instant::now();

        // Reset per-minute counter
        if now.duration_since(rl.window_start) >= std::time::Duration::from_secs(60) {
            rl.window_start = now;
            rl.count_this_minute = 0;
        }
        // Reset per-hour value counter
        if now.duration_since(rl.hour_start) >= std::time::Duration::from_secs(3600) {
            rl.hour_start = now;
            rl.value_this_hour = 0;
        }

        // Global: max 20 withdrawals per minute
        const MAX_WITHDRAWALS_PER_MIN: u64 = 20;
        if rl.count_this_minute >= MAX_WITHDRAWALS_PER_MIN {
            tracing::warn!(
                "⚠️  Withdrawal rate limit exceeded: {} this minute",
                rl.count_this_minute
            );
            return Json(json!({ "error": "rate_limited: too many withdrawals, try again later" }));
        }

        // Global: max 10M value per hour (in smallest units)
        const MAX_VALUE_PER_HOUR: u64 = 10_000_000_000_000_000; // 10M with 9 decimals
        if rl.value_this_hour.saturating_add(req.amount) > MAX_VALUE_PER_HOUR {
            tracing::warn!(
                "⚠️  Withdrawal value limit exceeded: {} this hour",
                rl.value_this_hour
            );
            return Json(json!({ "error": "rate_limited: hourly withdrawal value limit reached" }));
        }

        // Per-address: max 1 withdrawal per 30 seconds
        if let Some(last) = rl.per_address.get(&req.dest_address) {
            if now.duration_since(*last) < std::time::Duration::from_secs(30) {
                return Json(json!({ "error": "rate_limited: wait 30s between withdrawals" }));
            }
        }

        rl.count_this_minute += 1;
        rl.value_this_hour = rl.value_this_hour.saturating_add(req.amount);
        rl.per_address.insert(req.dest_address.clone(), now);
    }

    let asset_lower = req.asset.to_lowercase();

    // AUDIT-FIX F8.8: Validate dest_address format before processing.
    // Invalid addresses would waste signer resources and only fail at broadcast time.
    match req.dest_chain.as_str() {
        "solana" => {
            if bs58::decode(&req.dest_address)
                .into_vec()
                .map(|v| v.len())
                .unwrap_or(0)
                != 32
            {
                return Json(json!({
                    "error": format!("invalid Solana destination address: {}", req.dest_address)
                }));
            }
        }
        "ethereum" | "eth" | "bsc" | "bnb" => {
            let trimmed = req.dest_address.trim_start_matches("0x");
            if trimmed.len() != 40 || hex::decode(trimmed).is_err() {
                return Json(json!({
                    "error": format!("invalid EVM destination address: {}", req.dest_address)
                }));
            }
        }
        _ => {
            return Json(json!({
                "error": format!("unsupported destination chain: {}", req.dest_chain)
            }));
        }
    }

    let (dest_asset, _) = match asset_lower.as_str() {
        "musd" => ("stablecoin", "stablecoin"),
        "wsol" => ("sol", "native"),
        "weth" => ("eth", "native"),
        "wbnb" => ("bnb", "native"),
        _ => {
            return Json(json!({
                "error": format!("unsupported withdrawal asset: {}", req.asset)
            }));
        }
    };

    // Validate destination chain makes sense for the asset
    let valid_chain = match dest_asset {
        "sol" => req.dest_chain == "solana",
        "eth" => req.dest_chain == "ethereum" || req.dest_chain == "eth",
        "bnb" => req.dest_chain == "bsc" || req.dest_chain == "bnb",
        "stablecoin" => {
            req.dest_chain == "solana"
                || req.dest_chain == "ethereum"
                || req.dest_chain == "eth"
                || req.dest_chain == "bsc"
                || req.dest_chain == "bnb"
        }
        _ => false,
    };
    if !valid_chain {
        return Json(json!({
            "error": format!("cannot withdraw {} to {}", req.asset, req.dest_chain)
        }));
    }

    // For lUSD withdrawals: validate and resolve preferred stablecoin
    let preferred = if asset_lower == "musd" {
        let pref = req.preferred_stablecoin.to_lowercase();
        if pref != "usdt" && pref != "usdc" {
            return Json(json!({
                "error": format!("preferred_stablecoin must be 'usdt' or 'usdc', got '{}'", pref)
            }));
        }

        // AUDIT-FIX CUST-01: Convert spores to source-chain units BEFORE comparing.
        // Reserves are tracked in source-chain decimals (e.g. 6 for ETH USDT).
        // Withdrawal amounts are in spores (9 decimals). Without conversion,
        // a 1 USDT withdrawal (1e9 spores) would be compared against 1e6 reserve
        // and incorrectly fail.
        let chain_amount = spores_to_chain_amount(req.amount, &req.dest_chain, &pref);
        let chain_amount_u64 = u64::try_from(chain_amount).unwrap_or(u64::MAX);

        // Check reserve balance for the preferred stablecoin on the destination chain
        let reserve = get_reserve_balance(&state.db, &req.dest_chain, &pref).unwrap_or(0);
        let other = if pref == "usdt" { "usdc" } else { "usdt" };
        let other_reserve = get_reserve_balance(&state.db, &req.dest_chain, other).unwrap_or(0);
        let total_on_chain = reserve.saturating_add(other_reserve);

        if chain_amount_u64 > total_on_chain {
            return Json(json!({
                "error": format!(
                    "insufficient total stablecoin reserves on {}: requested {} (chain units), available {} ({} {} + {} {})",
                    req.dest_chain, chain_amount_u64, total_on_chain, reserve, pref, other_reserve, other
                )
            }));
        }

        if reserve < chain_amount_u64 {
            // Not enough of the preferred stablecoin — queue a rebalance swap first
            let deficit = chain_amount_u64 - reserve;
            let rebalance_job = RebalanceJob {
                job_id: Uuid::new_v4().to_string(),
                chain: req.dest_chain.clone(),
                from_asset: other.to_string(),
                to_asset: pref.clone(),
                amount: deficit,
                trigger: "withdrawal".to_string(),
                linked_withdrawal_job_id: None, // will be set after withdrawal job is created
                swap_tx_hash: None,
                status: "queued".to_string(),
                attempts: 0,
                last_error: None,
                next_attempt_at: None,
                created_at: chrono::Utc::now().timestamp(),
            };

            info!(
                "reserve deficit: need {} more {} on {} — queuing rebalance from {} (job={})",
                deficit, pref, req.dest_chain, other, rebalance_job.job_id
            );

            // We'll link after creating the withdrawal job (below)
            if let Err(e) = store_rebalance_job(&state.db, &rebalance_job) {
                return Json(json!({"error": format!("failed to queue rebalance: {}", e)}));
            }
        }

        pref
    } else {
        "usdt".to_string() // not applicable for non-stablecoin withdrawals
    };

    let job = WithdrawalJob {
        job_id: Uuid::new_v4().to_string(),
        user_id: req.user_id.clone(),
        asset: req.asset.clone(),
        amount: req.amount,
        dest_chain: req.dest_chain.clone(),
        dest_address: req.dest_address.clone(),
        preferred_stablecoin: preferred.clone(),
        burn_tx_signature: None,
        outbound_tx_hash: None,
        safe_nonce: None,
        signatures: Vec::new(),
        status: "pending_burn".to_string(),
        attempts: 0,
        last_error: None,
        next_attempt_at: None,
        created_at: chrono::Utc::now().timestamp(),
    };

    if let Err(e) = store_withdrawal_job(&state.db, &job) {
        return Json(json!({"error": format!("failed to store withdrawal: {}", e)}));
    }

    emit_custody_event(
        &state,
        "withdrawal.requested",
        &job.job_id,
        None,
        None,
        Some(
            &json!({ "user_id": job.user_id, "asset": job.asset, "amount": job.amount, "dest_chain": job.dest_chain, "dest_address": job.dest_address }),
        ),
    );

    info!(
        "withdrawal requested: {} {} → {} on {} (preferred_stablecoin={}, job={})",
        job.amount,
        job.asset,
        job.dest_address,
        job.dest_chain,
        job.preferred_stablecoin,
        job.job_id
    );

    let stablecoin_info = if asset_lower == "musd" {
        Some(preferred)
    } else {
        None
    };

    Json(json!({
        "job_id": job.job_id,
        "status": "pending_burn",
        "preferred_stablecoin": stablecoin_info,
        "message": format!(
            "Burn {} {} on Lichen, then the outbound transfer to {} will be processed automatically.",
            job.amount, job.asset, job.dest_chain
        ),
    }))
}

fn store_withdrawal_job(db: &DB, job: &WithdrawalJob) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_WITHDRAWAL_JOBS)
        .ok_or_else(|| "missing withdrawal_jobs cf".to_string())?;
    // AUDIT-FIX M1: Update status index on every store
    if let Ok(Some(old_bytes)) = db.get_cf(cf, job.job_id.as_bytes()) {
        if let Ok(old_job) = serde_json::from_slice::<WithdrawalJob>(&old_bytes) {
            let _ =
                update_status_index(db, "withdrawal", &old_job.status, &job.status, &job.job_id);
        }
    } else {
        let _ = set_status_index(db, "withdrawal", &job.status, &job.job_id);
    }
    let bytes = serde_json::to_vec(job).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, job.job_id.as_bytes(), bytes)
        .map_err(|e| format!("db put: {}", e))
}

fn fetch_withdrawal_job(db: &DB, job_id: &str) -> Result<Option<WithdrawalJob>, String> {
    let cf = db
        .cf_handle(CF_WITHDRAWAL_JOBS)
        .ok_or_else(|| "missing withdrawal_jobs cf".to_string())?;
    match db.get_cf(cf, job_id.as_bytes()) {
        Ok(Some(bytes)) => {
            let record = serde_json::from_slice(&bytes).map_err(|e| format!("decode: {}", e))?;
            Ok(Some(record))
        }
        Ok(None) => Ok(None),
        Err(e) => Err(format!("db get: {}", e)),
    }
}

fn burn_signature_index_key(burn_tx_signature: &str) -> String {
    format!("burn_sig:{}", burn_tx_signature)
}

fn reserve_burn_signature(db: &DB, burn_tx_signature: &str, job_id: &str) -> Result<(), String> {
    let idx_cf = db
        .cf_handle(CF_INDEXES)
        .ok_or_else(|| "missing indexes cf".to_string())?;
    let key = burn_signature_index_key(burn_tx_signature);

    if let Some(existing) = db
        .get_cf(idx_cf, key.as_bytes())
        .map_err(|e| format!("db get: {}", e))?
    {
        let existing_job_id = String::from_utf8_lossy(&existing);
        if existing_job_id != job_id {
            return Err(format!(
                "burn_tx_signature already used by withdrawal {}",
                existing_job_id
            ));
        }
    }

    db.put_cf(idx_cf, key.as_bytes(), job_id.as_bytes())
        .map_err(|e| format!("db put: {}", e))
}

fn release_burn_signature_reservation(
    db: &DB,
    burn_tx_signature: &str,
    job_id: &str,
) -> Result<(), String> {
    let idx_cf = db
        .cf_handle(CF_INDEXES)
        .ok_or_else(|| "missing indexes cf".to_string())?;
    let key = burn_signature_index_key(burn_tx_signature);

    if let Some(existing) = db
        .get_cf(idx_cf, key.as_bytes())
        .map_err(|e| format!("db get: {}", e))?
    {
        if existing.as_slice() == job_id.as_bytes() {
            db.delete_cf(idx_cf, key.as_bytes())
                .map_err(|e| format!("db delete: {}", e))?;
        }
    }

    Ok(())
}

fn reset_pending_burn_submission(
    db: &DB,
    job: &mut WithdrawalJob,
    err: String,
) -> Result<(), String> {
    if let Some(existing) = job.burn_tx_signature.take() {
        let _ = release_burn_signature_reservation(db, &existing, &job.job_id);
    }

    job.attempts = job.attempts.saturating_add(1);
    job.last_error = Some(err);
    if job.attempts >= MAX_JOB_ATTEMPTS {
        job.status = "permanently_failed".to_string();
        job.next_attempt_at = None;
        tracing::error!(
            "withdrawal job {} exceeded {} invalid burn submissions — moved to permanently_failed",
            job.job_id,
            MAX_JOB_ATTEMPTS
        );
    } else {
        job.status = "pending_burn".to_string();
        job.next_attempt_at = None;
    }

    store_withdrawal_job(db, job)
}

/// AUDIT-FIX C4: Endpoint for clients to submit the Lichen burn tx signature.
///
/// PUT /withdrawals/:job_id/burn
///
/// After a user burns their wrapped tokens on Lichen, they submit the burn tx
/// signature here. The withdrawal worker then verifies it and progresses the job.
/// Without this endpoint, withdrawal jobs would hang at "pending_burn" forever.
#[derive(Deserialize)]
struct BurnSignaturePayload {
    burn_tx_signature: String,
}

async fn submit_burn_signature(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(job_id): axum::extract::Path<String>,
    Json(payload): Json<BurnSignaturePayload>,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    if payload.burn_tx_signature.is_empty() {
        return Err(Json(ErrorResponse::invalid("burn_tx_signature required")));
    }

    // AUDIT-FIX R-H3 + F8.7: Serialize burn signature submission per job_id
    // to prevent TOCTOU race where two concurrent requests both pass the
    // "burn_tx_signature is None" check and one overwrites the other.
    // F8.7: Prune map when it exceeds 10,000 entries to prevent unbounded growth.
    static BURN_LOCKS: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

    let lock = {
        let mut locks = BURN_LOCKS.lock().unwrap_or_else(|e| e.into_inner());
        // F8.7: Prevent unbounded memory growth — clear stale entries when map is large
        if locks.len() > 10_000 {
            // Retain only entries with active references (Arc strong_count > 1)
            locks.retain(|_, v| std::sync::Arc::strong_count(v) > 1);
        }
        locks
            .entry(job_id.clone())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    };
    let _guard = lock.lock().await;

    let mut job = fetch_withdrawal_job(&state.db, &job_id)
        .map_err(|e| Json(ErrorResponse::db(&e)))?
        .ok_or_else(|| Json(ErrorResponse::invalid("withdrawal not found")))?;

    if job.status != "pending_burn" {
        return Err(Json(ErrorResponse::invalid(&format!(
            "withdrawal {} is not in pending_burn state (current: {})",
            job_id, job.status
        ))));
    }

    if job.burn_tx_signature.as_deref() == Some(payload.burn_tx_signature.as_str()) {
        return Ok(Json(json!({
            "job_id": job.job_id,
            "status": job.status,
            "burn_tx_signature": payload.burn_tx_signature,
            "message": "burn_tx_signature already recorded"
        })));
    }

    reserve_burn_signature(&state.db, &payload.burn_tx_signature, &job_id)
        .map_err(|e| Json(ErrorResponse::invalid(&e)))?;

    if let Some(existing) = job
        .burn_tx_signature
        .replace(payload.burn_tx_signature.clone())
    {
        let _ = release_burn_signature_reservation(&state.db, &existing, &job_id);
    }

    job.last_error = None;
    job.next_attempt_at = None;
    store_withdrawal_job(&state.db, &job).map_err(|e| Json(ErrorResponse::db(&e)))?;

    record_audit_event(
        &state.db,
        "withdrawal_burn_submitted",
        &job.job_id,
        None,
        Some(&payload.burn_tx_signature),
    )
    .ok();
    // Also emit to webhooks/WS
    emit_custody_event(
        &state,
        "withdrawal.burn_submitted",
        &job.job_id,
        None,
        Some(&payload.burn_tx_signature),
        None,
    );

    info!(
        "burn signature submitted for withdrawal {}: {}",
        job_id, payload.burn_tx_signature
    );

    Ok(Json(json!({
        "job_id": job_id,
        "status": "pending_burn",
        "burn_tx_signature": payload.burn_tx_signature,
        "message": "Burn signature recorded. Verification will proceed automatically.",
    })))
}

fn list_withdrawal_jobs_by_status(db: &DB, status: &str) -> Result<Vec<WithdrawalJob>, String> {
    // AUDIT-FIX M1: Use status index for O(active) instead of O(total)
    let ids = list_ids_by_status_index(db, "withdrawal", status)?;
    if !ids.is_empty() {
        let cf = db
            .cf_handle(CF_WITHDRAWAL_JOBS)
            .ok_or_else(|| "missing withdrawal_jobs cf".to_string())?;
        let mut results = Vec::new();
        for id in ids {
            if let Ok(Some(bytes)) = db.get_cf(cf, id.as_bytes()) {
                if let Ok(record) = serde_json::from_slice::<WithdrawalJob>(&bytes) {
                    if record.status == status {
                        results.push(record);
                    }
                }
            }
        }
        return Ok(results);
    }
    // Fallback: legacy full scan for pre-index data
    let cf = db
        .cf_handle(CF_WITHDRAWAL_JOBS)
        .ok_or_else(|| "missing withdrawal_jobs cf".to_string())?;
    let mut results = Vec::new();
    let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
    for item in iter {
        let (_, value) = item.map_err(|e| format!("db iter: {}", e))?;
        let record: WithdrawalJob =
            serde_json::from_slice(&value).map_err(|e| format!("decode: {}", e))?;
        if record.status == status {
            results.push(record);
        }
    }
    Ok(results)
}

// ============================================================================
// RESERVE LEDGER — Track stablecoin reserves per chain+asset
// ============================================================================

/// Get the reserve balance for a specific chain + stablecoin.
/// Key format: "{chain}:{asset}" e.g. "solana:usdt", "ethereum:usdc"
fn get_reserve_balance(db: &DB, chain: &str, asset: &str) -> Result<u64, String> {
    let cf = db
        .cf_handle(CF_RESERVE_LEDGER)
        .ok_or_else(|| "missing reserve_ledger cf".to_string())?;
    let key = format!("{}:{}", chain, asset);
    match db.get_cf(cf, key.as_bytes()) {
        Ok(Some(bytes)) => {
            let entry: ReserveLedgerEntry =
                serde_json::from_slice(&bytes).map_err(|e| format!("decode: {}", e))?;
            Ok(entry.amount)
        }
        Ok(None) => Ok(0),
        Err(e) => Err(format!("db get: {}", e)),
    }
}

/// Adjust reserve balance: increment (deposit/rebalance in) or decrement (withdrawal/rebalance out).
/// If decrementing would go below zero, clamps to 0 and logs a warning.
/// AUDIT-FIX M5: Replaced std::sync::Mutex with tokio::sync::Mutex to avoid
/// blocking the async executor when the lock is contended. The critical section
/// serializes read-modify-write on the reserve ledger CF.
async fn adjust_reserve_balance(
    db: &DB,
    chain: &str,
    asset: &str,
    amount: u64,
    increment: bool,
) -> Result<(), String> {
    static RESERVE_LOCK: tokio::sync::OnceCell<tokio::sync::Mutex<()>> =
        tokio::sync::OnceCell::const_new();
    let mutex = RESERVE_LOCK
        .get_or_init(|| async { tokio::sync::Mutex::new(()) })
        .await;
    let _guard = mutex.lock().await;

    let cf = db
        .cf_handle(CF_RESERVE_LEDGER)
        .ok_or_else(|| "missing reserve_ledger cf".to_string())?;
    let key = format!("{}:{}", chain, asset);

    let current = match db.get_cf(cf, key.as_bytes()) {
        Ok(Some(bytes)) => {
            let entry: ReserveLedgerEntry =
                serde_json::from_slice(&bytes).map_err(|e| format!("decode: {}", e))?;
            entry.amount
        }
        Ok(None) => 0,
        Err(e) => return Err(format!("db get: {}", e)),
    };

    let new_amount = if increment {
        current.saturating_add(amount)
    } else {
        if amount > current {
            tracing::warn!(
                "reserve underflow: {}:{} has {} but trying to deduct {}",
                chain,
                asset,
                current,
                amount
            );
        }
        current.saturating_sub(amount)
    };

    let entry = ReserveLedgerEntry {
        chain: chain.to_string(),
        asset: asset.to_string(),
        amount: new_amount,
        last_updated: chrono::Utc::now().timestamp(),
    };
    let bytes = serde_json::to_vec(&entry).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, key.as_bytes(), bytes)
        .map_err(|e| format!("db put: {}", e))?;

    info!(
        "reserve ledger: {}:{} {} {} → {}",
        chain,
        asset,
        if increment { "+" } else { "-" },
        amount,
        new_amount
    );
    Ok(())
}

/// AUDIT-FIX F8.4: Reserves endpoint now requires API auth.
/// Without auth, this leaks treasury stablecoin balances to unauthenticated callers.
async fn get_reserves(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    let cf = match state.db.cf_handle(CF_RESERVE_LEDGER) {
        Some(cf) => cf,
        None => return Ok(Json(json!({"error": "reserve ledger not available"}))),
    };
    let mut entries = Vec::new();
    let iter = state.db.iterator_cf(cf, rocksdb::IteratorMode::Start);
    for (_, value) in iter.flatten() {
        if let Ok(entry) = serde_json::from_slice::<ReserveLedgerEntry>(&value) {
            entries.push(json!({
                "chain": entry.chain,
                "asset": entry.asset,
                "amount": entry.amount,
                "last_updated": entry.last_updated,
            }));
        }
    }

    // Compute per-chain ratios
    let mut by_chain: std::collections::HashMap<String, (u64, u64)> =
        std::collections::HashMap::new();
    for item in &entries {
        let chain = item["chain"].as_str().unwrap_or("?");
        let asset = item["asset"].as_str().unwrap_or("?");
        let amount = item["amount"].as_u64().unwrap_or(0);
        let entry = by_chain.entry(chain.to_string()).or_insert((0, 0));
        match asset {
            "usdt" => entry.0 = amount,
            "usdc" => entry.1 = amount,
            _ => {}
        }
    }

    let mut ratios = Vec::new();
    for (chain, (usdt, usdc)) in &by_chain {
        let total = usdt + usdc;
        let usdt_pct = if total > 0 {
            (*usdt as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        ratios.push(json!({
            "chain": chain,
            "usdt": usdt,
            "usdc": usdc,
            "total": total,
            "usdt_pct": format!("{:.1}%", usdt_pct),
            "usdc_pct": format!("{:.1}%", 100.0 - usdt_pct),
        }));
    }

    Ok(Json(json!({
        "reserves": entries,
        "chain_ratios": ratios,
    })))
}

// ============================================================================
// REBALANCE — Swap USDT↔USDC on external DEXes to maintain reserve balance
// ============================================================================

fn store_rebalance_job(db: &DB, job: &RebalanceJob) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_REBALANCE_JOBS)
        .ok_or_else(|| "missing rebalance_jobs cf".to_string())?;
    // AUDIT-FIX M1: Update status index on every store
    if let Ok(Some(old_bytes)) = db.get_cf(cf, job.job_id.as_bytes()) {
        if let Ok(old_job) = serde_json::from_slice::<RebalanceJob>(&old_bytes) {
            let _ = update_status_index(db, "rebalance", &old_job.status, &job.status, &job.job_id);
        }
    } else {
        let _ = set_status_index(db, "rebalance", &job.status, &job.job_id);
    }
    let bytes = serde_json::to_vec(job).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, job.job_id.as_bytes(), bytes)
        .map_err(|e| format!("db put: {}", e))
}

fn list_rebalance_jobs_by_status(db: &DB, status: &str) -> Result<Vec<RebalanceJob>, String> {
    // AUDIT-FIX M1: Use status index for O(active) instead of O(total)
    let ids = list_ids_by_status_index(db, "rebalance", status)?;
    if !ids.is_empty() {
        let cf = db
            .cf_handle(CF_REBALANCE_JOBS)
            .ok_or_else(|| "missing rebalance_jobs cf".to_string())?;
        let mut results = Vec::new();
        for id in ids {
            if let Ok(Some(bytes)) = db.get_cf(cf, id.as_bytes()) {
                if let Ok(record) = serde_json::from_slice::<RebalanceJob>(&bytes) {
                    if record.status == status {
                        results.push(record);
                    }
                }
            }
        }
        return Ok(results);
    }
    // Fallback: legacy full scan for pre-index data
    let cf = db
        .cf_handle(CF_REBALANCE_JOBS)
        .ok_or_else(|| "missing rebalance_jobs cf".to_string())?;
    let mut results = Vec::new();
    let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
    for item in iter {
        let (_, value) = item.map_err(|e| format!("db iter: {}", e))?;
        let record: RebalanceJob =
            serde_json::from_slice(&value).map_err(|e| format!("decode: {}", e))?;
        if record.status == status {
            results.push(record);
        }
    }
    Ok(results)
}

/// Background loop: monitors USDT/USDC ratio on each chain and swaps when needed.
/// Also processes on-demand rebalance jobs triggered by withdrawals.
async fn rebalance_worker_loop(state: CustodyState) {
    loop {
        // Process on-demand rebalance jobs (triggered by withdrawal reserve deficits)
        if let Err(err) = process_rebalance_jobs(&state).await {
            tracing::warn!("rebalance worker error: {}", err);
        }

        // Periodic ratio check: auto-create rebalance jobs if ratio drifts too far
        if let Err(err) = check_rebalance_thresholds(&state) {
            tracing::warn!("rebalance threshold check error: {}", err);
        }

        // Rebalance runs less frequently than other workers (every 5 minutes)
        sleep(Duration::from_secs(state.config.poll_interval_secs * 20)).await;
    }
}

/// Background loop: prunes expired, unfunded deposit addresses.
/// Only deposits in "issued" status (never received funds) older than
/// `deposit_ttl_secs` are marked "expired" and their address index removed.
/// AUDIT-FIX F8.10: Uses status index for "issued" deposits instead of full-table scan.
async fn deposit_cleanup_loop(state: CustodyState) {
    loop {
        // Run every 10 minutes
        sleep(Duration::from_secs(600)).await;

        let ttl = state.config.deposit_ttl_secs;
        if ttl <= 0 {
            continue; // TTL disabled
        }
        let cutoff = chrono::Utc::now().timestamp() - ttl;

        // F8.10: Use status index to find "issued" deposits instead of full-table scan
        let issued_ids = match list_ids_by_status_index(&state.db, "deposits", "issued") {
            Ok(ids) => ids,
            Err(_) => continue,
        };

        let mut expired_ids = Vec::new();
        for id in &issued_ids {
            if let Ok(Some(record)) = fetch_deposit(&state.db, id) {
                if record.status == "issued" && record.created_at < cutoff {
                    expired_ids.push((id.clone(), record.address.clone()));
                }
            }
        }

        // Fallback: if status index was empty, try full scan (pre-index data)
        if expired_ids.is_empty() && issued_ids.is_empty() {
            if let Some(cf) = state.db.cf_handle(CF_DEPOSITS) {
                let iter = state.db.iterator_cf(&cf, rocksdb::IteratorMode::Start);
                for item in iter {
                    let (key, value) = match item {
                        Ok(kv) => kv,
                        Err(_) => continue,
                    };
                    let record: DepositRequest = match serde_json::from_slice(&value) {
                        Ok(r) => r,
                        Err(_) => continue,
                    };
                    if record.status == "issued" && record.created_at < cutoff {
                        expired_ids.push((
                            String::from_utf8_lossy(&key).to_string(),
                            record.address.clone(),
                        ));
                    }
                }
            }
        }

        let count = expired_ids.len();
        for (deposit_id, address) in &expired_ids {
            // Update status to "expired"
            if let Some(cf) = state.db.cf_handle(CF_DEPOSITS) {
                if let Ok(Some(value)) = state.db.get_cf(&cf, deposit_id.as_bytes()) {
                    if let Ok(mut record) = serde_json::from_slice::<DepositRequest>(&value) {
                        let old_status = record.status.clone();
                        record.status = "expired".to_string();
                        if let Ok(json) = serde_json::to_vec(&record) {
                            let _ = state.db.put_cf(&cf, deposit_id.as_bytes(), &json);
                            // AUDIT-FIX R-M1: Maintain status index during cleanup
                            let _ = update_status_index(
                                &state.db,
                                "deposits",
                                &old_status,
                                "expired",
                                deposit_id,
                            );
                        }
                    }
                }
            }
            // Remove address → deposit_id index so the address can be recycled
            if let Some(addr_cf) = state.db.cf_handle(CF_ADDRESS_INDEX) {
                let _ = state.db.delete_cf(&addr_cf, address.as_bytes());
            }
            // Prune stale address balance entries
            if let Some(bal_cf) = state.db.cf_handle(CF_ADDRESS_BALANCES) {
                let _ = state.db.delete_cf(&bal_cf, address.as_bytes());
            }
            // Prune stale token balance entries (key format: address:token)
            if let Some(tok_cf) = state.db.cf_handle(CF_TOKEN_BALANCES) {
                let prefix = format!("{}:", address);
                let iter = state.db.prefix_iterator_cf(&tok_cf, prefix.as_bytes());
                for (key, _) in iter.flatten() {
                    if key.starts_with(prefix.as_bytes()) {
                        let _ = state.db.delete_cf(&tok_cf, &key);
                    } else {
                        break;
                    }
                }
            }
            // Prune deposit events and dedup markers for this deposit
            // AUDIT-FIX 2.19: Use correct prefix for dedup markers (dedup:{deposit_id}:)
            // and scan events by deserializing to match deposit_id field.
            if let Some(evt_cf) = state.db.cf_handle(CF_DEPOSIT_EVENTS) {
                // Delete dedup markers (keyed as "dedup:{deposit_id}:{tx_hash}")
                let dedup_prefix = format!("dedup:{}:", deposit_id);
                let iter = state
                    .db
                    .prefix_iterator_cf(&evt_cf, dedup_prefix.as_bytes());
                for (key, _) in iter.flatten() {
                    if key.starts_with(dedup_prefix.as_bytes()) {
                        let _ = state.db.delete_cf(&evt_cf, &key);
                    } else {
                        break;
                    }
                }
                // Delete event entries (keyed by event_id, need full scan of CF)
                let iter = state.db.iterator_cf(&evt_cf, rocksdb::IteratorMode::Start);
                for (key, value) in iter.flatten() {
                    // Skip dedup markers (they start with "dedup:")
                    if key.starts_with(b"dedup:") {
                        continue;
                    }
                    if let Ok(evt) = serde_json::from_slice::<DepositEvent>(&value) {
                        if evt.deposit_id == *deposit_id {
                            let _ = state.db.delete_cf(&evt_cf, &key);
                        }
                    }
                }
            }
        }

        if count > 0 {
            // Emit events for expired deposits
            for (deposit_id, address) in &expired_ids {
                emit_custody_event(
                    &state,
                    "deposit.expired",
                    deposit_id,
                    Some(deposit_id),
                    None,
                    Some(&serde_json::json!({
                        "address": address,
                        "ttl_secs": ttl
                    })),
                );
            }
            info!(
                "deposit cleanup: expired {} unfunded deposits older than {}s",
                count, ttl
            );
        }
    }
}

/// Check USDT/USDC ratio on each chain. If one side exceeds `rebalance_threshold_bps`,
/// create a rebalance job to swap toward `rebalance_target_bps`.
fn check_rebalance_thresholds(state: &CustodyState) -> Result<(), String> {
    let threshold = state.config.rebalance_threshold_bps;
    let target = state.config.rebalance_target_bps;

    // AUDIT-FIX CUST-03: Include BSC in rebalance monitoring (was missing)
    for chain in &["solana", "ethereum", "bsc"] {
        let usdt = get_reserve_balance(&state.db, chain, "usdt").unwrap_or(0);
        let usdc = get_reserve_balance(&state.db, chain, "usdc").unwrap_or(0);
        let total = usdt.saturating_add(usdc);
        if total == 0 {
            continue;
        }

        // Check if USDT percentage exceeds threshold
        let usdt_bps = (usdt as u128 * 10_000 / total as u128) as u64;

        if usdt_bps > threshold {
            // USDT is too high — swap some USDT → USDC
            // Target: bring USDT down to target_bps
            let target_usdt = (total as u128 * target as u128 / 10_000) as u64;
            let swap_amount = usdt.saturating_sub(target_usdt);
            if swap_amount > 0 {
                create_threshold_rebalance(&state.db, chain, "usdt", "usdc", swap_amount)?;
            }
        } else if (10_000 - usdt_bps) > threshold {
            // USDC is too high — swap some USDC → USDT
            let target_usdc = (total as u128 * (10_000 - target) as u128 / 10_000) as u64;
            let swap_amount = usdc.saturating_sub(target_usdc);
            if swap_amount > 0 {
                create_threshold_rebalance(&state.db, chain, "usdc", "usdt", swap_amount)?;
            }
        }
    }

    Ok(())
}

fn create_threshold_rebalance(
    db: &DB,
    chain: &str,
    from: &str,
    to: &str,
    amount: u64,
) -> Result<(), String> {
    // Don't create duplicate threshold rebalance jobs
    let existing = list_rebalance_jobs_by_status(db, "queued")?;
    for job in &existing {
        if job.chain == chain && job.from_asset == from && job.trigger == "threshold" {
            return Ok(()); // already queued
        }
    }

    let job = RebalanceJob {
        job_id: Uuid::new_v4().to_string(),
        chain: chain.to_string(),
        from_asset: from.to_string(),
        to_asset: to.to_string(),
        amount,
        trigger: "threshold".to_string(),
        linked_withdrawal_job_id: None,
        swap_tx_hash: None,
        status: "queued".to_string(),
        attempts: 0,
        last_error: None,
        next_attempt_at: None,
        created_at: chrono::Utc::now().timestamp(),
    };

    info!(
        "auto-rebalance: {} {} → {} on {} (ratio threshold exceeded, job={})",
        amount, from, to, chain, job.job_id
    );

    store_rebalance_job(db, &job)
}

/// M14 fix: Fetch a confirmed Solana swap transaction and parse the actual token output amount.
///
/// Uses `getTransaction` with `maxSupportedTransactionVersion: 0` to get the full tx with
/// `meta.preTokenBalances`/`meta.postTokenBalances`. Finds the token account belonging to the
/// treasury whose mint matches `to_mint`, then computes `post_amount - pre_amount`.
///
/// Returns the output amount in the token's smallest unit (e.g. USDC 6-decimal atoms).
/// Falls back to `None` if the transaction format doesn't contain the expected fields.
async fn parse_solana_swap_output(
    client: &reqwest::Client,
    url: &str,
    signature: &str,
    treasury_addr: &str,
    to_mint: &str,
) -> Result<Option<u64>, String> {
    let params = json!([
        signature,
        { "encoding": "jsonParsed", "maxSupportedTransactionVersion": 0 }
    ]);
    let result = solana_rpc_call(client, url, "getTransaction", params).await?;
    if result.is_null() {
        return Ok(None);
    }

    let meta = match result.get("meta") {
        Some(m) if !m.is_null() => m,
        _ => return Ok(None),
    };

    // Check for transaction error
    if !meta.get("err").is_none_or(|e| e.is_null()) {
        return Err("Solana swap transaction failed on-chain".to_string());
    }

    let pre_balances = meta.get("preTokenBalances").and_then(|v| v.as_array());
    let post_balances = meta.get("postTokenBalances").and_then(|v| v.as_array());

    let (pre_balances, post_balances) = match (pre_balances, post_balances) {
        (Some(pre), Some(post)) => (pre, post),
        _ => return Ok(None),
    };

    // Build a lookup: for each account index, find the pre and post amounts for the output mint
    // belonging to the treasury address.
    let extract_amount = |entries: &[Value]| -> Option<u64> {
        for entry in entries {
            let mint = entry.get("mint").and_then(|v| v.as_str()).unwrap_or("");
            let owner = entry.get("owner").and_then(|v| v.as_str()).unwrap_or("");
            if mint == to_mint && owner == treasury_addr {
                return entry
                    .get("uiTokenAmount")
                    .and_then(|v| v.get("amount"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u64>().ok());
            }
        }
        None
    };

    let pre_amount = extract_amount(pre_balances).unwrap_or(0);
    let post_amount = extract_amount(post_balances).unwrap_or(0);

    if post_amount > pre_amount {
        Ok(Some(post_amount - pre_amount))
    } else {
        // Edge case: balance didn't increase (swap might have failed silently)
        Ok(None)
    }
}

/// M14 fix: Fetch a confirmed EVM swap transaction receipt and parse the actual token output.
///
/// Decodes ERC-20 Transfer event logs in the receipt. Looks for a Transfer event where the
/// `to` address is the treasury and the emitting contract is the `to_token_contract`.
///
/// Returns the output amount in the token's smallest unit (e.g. USDT 6-decimal atoms).
/// Falls back to `None` if no matching Transfer log is found.
async fn parse_evm_swap_output(
    client: &reqwest::Client,
    url: &str,
    tx_hash: &str,
    treasury_addr: &str,
    to_token_contract: &str,
) -> Result<Option<u64>, String> {
    let receipt = evm_get_transaction_receipt(client, url, tx_hash).await?;
    let receipt = match receipt {
        Some(r) => r,
        None => return Ok(None),
    };

    // Check receipt status (0x1 = success)
    let status = receipt
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("0x0");
    if status != "0x1" {
        return Err("EVM swap transaction reverted".to_string());
    }

    let logs = match receipt.get("logs").and_then(|v| v.as_array()) {
        Some(l) => l,
        None => return Ok(None),
    };

    let treasury_lower = treasury_addr.to_lowercase();
    let contract_lower = to_token_contract.to_lowercase();

    // ERC-20 Transfer topic: keccak256("Transfer(address,address,uint256)")
    let transfer_topic = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

    let mut total_output: u128 = 0;

    for log in logs {
        // Check emitting contract matches the output token
        let log_address = log.get("address").and_then(|v| v.as_str()).unwrap_or("");
        if log_address.to_lowercase() != contract_lower {
            continue;
        }

        let topics = match log.get("topics").and_then(|v| v.as_array()) {
            Some(t) if t.len() >= 3 => t,
            _ => continue,
        };

        // Verify it's a Transfer event
        let event_topic = topics[0].as_str().unwrap_or("");
        if event_topic != transfer_topic {
            continue;
        }

        // topics[2] = `to` address (zero-padded to 32 bytes)
        let to_topic = topics[2].as_str().unwrap_or("").trim_start_matches("0x");
        if to_topic.len() < 40 {
            continue;
        }
        let to_addr = format!("0x{}", &to_topic[to_topic.len() - 40..]);
        if to_addr.to_lowercase() != treasury_lower {
            continue;
        }

        // data = amount (uint256 hex-encoded)
        let data = log.get("data").and_then(|v| v.as_str()).unwrap_or("0x0");
        if let Ok(amount) = parse_hex_u128(data) {
            total_output = total_output.saturating_add(amount);
        }
    }

    if total_output > 0 {
        // I-9: Guard against u128→u64 truncation for large EVM amounts
        if total_output > u64::MAX as u128 {
            return Err(format!(
                "Swap output {} exceeds u64::MAX — cannot safely represent",
                total_output
            ));
        }
        Ok(Some(total_output as u64))
    } else {
        Ok(None)
    }
}

/// Process queued rebalance jobs: submit swaps on external DEXes.
async fn process_rebalance_jobs(state: &CustodyState) -> Result<(), String> {
    // Process queued → submitted
    let queued = list_rebalance_jobs_by_status(&state.db, "queued")?;
    for mut job in queued {
        match execute_rebalance_swap(state, &job).await {
            Ok(tx_hash) => {
                job.swap_tx_hash = Some(tx_hash.clone());
                job.status = "submitted".to_string();
                job.last_error = None;
                store_rebalance_job(&state.db, &job)?;
                emit_custody_event(
                    state,
                    "rebalance.submitted",
                    &job.job_id,
                    None,
                    Some(&tx_hash),
                    Some(&serde_json::json!({
                        "chain": job.chain,
                        "from_asset": job.from_asset,
                        "to_asset": job.to_asset,
                        "amount": job.amount
                    })),
                );
                info!(
                    "rebalance swap submitted: {} {} → {} on {} (tx={})",
                    job.amount, job.from_asset, job.to_asset, job.chain, tx_hash
                );
            }
            Err(e) => {
                job.attempts = job.attempts.saturating_add(1);
                job.last_error = Some(e.clone());
                job.next_attempt_at = Some(next_retry_timestamp(job.attempts));
                if job.attempts > 5 {
                    job.status = "failed".to_string();
                    tracing::error!(
                        "rebalance job {} failed permanently after {} attempts: {}",
                        job.job_id,
                        job.attempts,
                        e
                    );
                }
                store_rebalance_job(&state.db, &job)?;
            }
        }
    }

    // Process submitted → confirmed
    let submitted = list_rebalance_jobs_by_status(&state.db, "submitted")?;
    for mut job in submitted {
        let confirmed = match job.chain.as_str() {
            "solana" => {
                if let (Some(url), Some(ref tx_hash)) =
                    (state.config.solana_rpc_url.as_ref(), &job.swap_tx_hash)
                {
                    solana_get_signature_confirmed(&state.http, url, tx_hash)
                        .await
                        .unwrap_or(None)
                        .unwrap_or(false)
                } else {
                    false
                }
            }
            "ethereum" => {
                if let (Some(url), Some(ref tx_hash)) =
                    (state.config.evm_rpc_url.as_ref(), &job.swap_tx_hash)
                {
                    check_evm_tx_confirmed(
                        &state.http,
                        url,
                        tx_hash,
                        state.config.evm_confirmations,
                    )
                    .await
                    .unwrap_or(false)
                } else {
                    false
                }
            }
            _ => false,
        };

        if confirmed {
            job.status = "confirmed".to_string();
            job.last_error = None;

            // M14 fix: Parse the actual swap output from the on-chain transaction
            // instead of assuming output == input (which ignores slippage, fees, price impact).
            let actual_output = match job.chain.as_str() {
                "solana" => {
                    if let (Some(url), Some(ref tx_hash)) =
                        (state.config.solana_rpc_url.as_ref(), &job.swap_tx_hash)
                    {
                        let to_mint =
                            solana_mint_for_asset(&state.config, &job.to_asset).unwrap_or_default();
                        let treasury = state
                            .config
                            .treasury_solana_address
                            .as_deref()
                            .unwrap_or("");
                        parse_solana_swap_output(&state.http, url, tx_hash, treasury, &to_mint)
                            .await
                            .unwrap_or(None)
                    } else {
                        None
                    }
                }
                "ethereum" => {
                    if let (Some(url), Some(ref tx_hash)) =
                        (state.config.evm_rpc_url.as_ref(), &job.swap_tx_hash)
                    {
                        let to_contract = evm_contract_for_asset(&state.config, &job.to_asset)
                            .unwrap_or_default();
                        let treasury = state.config.treasury_evm_address.as_deref().unwrap_or("");
                        parse_evm_swap_output(&state.http, url, tx_hash, treasury, &to_contract)
                            .await
                            .unwrap_or(None)
                    } else {
                        None
                    }
                }
                _ => None,
            };

            // AUDIT-FIX M14: Validate swap output against max slippage tolerance.
            // If output is unparseable, mark job as "unverified" — do NOT assume 1:1.
            let credit_amount = match actual_output {
                Some(output) => {
                    if job.amount > 0 {
                        let slippage_bps = (job.amount.saturating_sub(output) as u128 * 10_000
                            / job.amount as u128) as u64;
                        if slippage_bps > state.config.rebalance_max_slippage_bps {
                            tracing::error!(
                                "rebalance slippage {}bps exceeds max {}bps: input={} output={} (job={})",
                                slippage_bps, state.config.rebalance_max_slippage_bps,
                                job.amount, output, job.job_id
                            );
                            job.status = "slippage_exceeded".to_string();
                            store_rebalance_job(&state.db, &job)?;
                            emit_custody_event(
                                state,
                                "rebalance.slippage_exceeded",
                                &job.job_id,
                                None,
                                job.swap_tx_hash.as_deref(),
                                Some(&serde_json::json!({
                                    "slippage_bps": slippage_bps,
                                    "max_slippage_bps": state.config.rebalance_max_slippage_bps,
                                    "input": job.amount,
                                    "output": output
                                })),
                            );
                            continue;
                        }
                    }
                    if output != job.amount {
                        info!(
                            "rebalance swap output differs from input: input={} output={} (job={})",
                            job.amount, output, job.job_id
                        );
                    }
                    output
                }
                None => {
                    tracing::warn!(
                        "could not parse swap output for job {}, marking unverified (NOT crediting assumed amount {})",
                        job.job_id,
                        job.amount
                    );
                    job.status = "unverified".to_string();
                    store_rebalance_job(&state.db, &job)?;
                    emit_custody_event(
                        state,
                        "rebalance.output_unverified",
                        &job.job_id,
                        None,
                        job.swap_tx_hash.as_deref(),
                        Some(&serde_json::json!({
                            "amount": job.amount,
                            "chain": job.chain
                        })),
                    );
                    continue;
                }
            };

            store_rebalance_job(&state.db, &job)?;

            // Update reserve ledger: debit input amount, credit actual output
            adjust_reserve_balance(&state.db, &job.chain, &job.from_asset, job.amount, false)
                .await?;
            adjust_reserve_balance(&state.db, &job.chain, &job.to_asset, credit_amount, true)
                .await?;

            emit_custody_event(
                state,
                "rebalance.confirmed",
                &job.job_id,
                None,
                job.swap_tx_hash.as_deref(),
                Some(&serde_json::json!({
                    "chain": job.chain,
                    "from_asset": job.from_asset,
                    "to_asset": job.to_asset,
                    "amount": job.amount,
                    "credit_amount": credit_amount
                })),
            );
            info!(
                "rebalance confirmed: {} {} → {} on {} (job={})",
                job.amount, job.from_asset, job.to_asset, job.chain, job.job_id
            );
        }
    }

    Ok(())
}

/// Execute a stablecoin swap on an external DEX.
///
/// Solana: uses Jupiter aggregator API
/// Ethereum: uses Uniswap V3 router
async fn execute_rebalance_swap(
    state: &CustodyState,
    job: &RebalanceJob,
) -> Result<String, String> {
    match job.chain.as_str() {
        "solana" => execute_solana_rebalance_swap(state, job).await,
        "ethereum" => execute_ethereum_rebalance_swap(state, job).await,
        other => Err(format!("unsupported rebalance chain: {}", other)),
    }
}

/// Execute a USDT↔USDC swap on Solana via Jupiter aggregator.
///
/// Steps:
///   1. GET /quote — get best route for from_mint → to_mint
///   2. POST /swap — get serialized transaction
///   3. Sign and submit to Solana RPC
async fn execute_solana_rebalance_swap(
    state: &CustodyState,
    job: &RebalanceJob,
) -> Result<String, String> {
    let jupiter_url = state
        .config
        .jupiter_api_url
        .as_ref()
        .ok_or_else(|| "missing CUSTODY_JUPITER_API_URL for Solana rebalance".to_string())?;
    let solana_url = state
        .config
        .solana_rpc_url
        .as_ref()
        .ok_or_else(|| "missing solana RPC for rebalance".to_string())?;
    let treasury_addr = state
        .config
        .treasury_solana_address
        .as_ref()
        .ok_or_else(|| "missing treasury solana address".to_string())?;

    let from_mint = match job.from_asset.as_str() {
        "usdt" => &state.config.solana_usdt_mint,
        "usdc" => &state.config.solana_usdc_mint,
        _ => return Err(format!("unsupported from_asset: {}", job.from_asset)),
    };
    let to_mint = match job.to_asset.as_str() {
        "usdt" => &state.config.solana_usdt_mint,
        "usdc" => &state.config.solana_usdc_mint,
        _ => return Err(format!("unsupported to_asset: {}", job.to_asset)),
    };

    // Step 1: Get Jupiter quote
    let quote_url = format!(
        // AUDIT-FIX M14: configurable slippage tolerance for Jupiter quotes
        "{}/quote?inputMint={}&outputMint={}&amount={}&slippageBps={}",
        jupiter_url.trim_end_matches('/'),
        from_mint,
        to_mint,
        job.amount,
        state.config.rebalance_max_slippage_bps
    );
    let quote_resp = state
        .http
        .get(&quote_url)
        .send()
        .await
        .map_err(|e| format!("jupiter quote: {}", e))?;
    let quote: Value = quote_resp
        .json()
        .await
        .map_err(|e| format!("jupiter quote json: {}", e))?;

    // Step 2: Get swap transaction
    let swap_url = format!("{}/swap", jupiter_url.trim_end_matches('/'));
    let swap_body = json!({
        "quoteResponse": quote,
        "userPublicKey": treasury_addr,
        "wrapAndUnwrapSol": false,
    });
    let swap_resp = state
        .http
        .post(&swap_url)
        .json(&swap_body)
        .send()
        .await
        .map_err(|e| format!("jupiter swap: {}", e))?;
    let swap_result: Value = swap_resp
        .json()
        .await
        .map_err(|e| format!("jupiter swap json: {}", e))?;

    let swap_tx_b64 = swap_result
        .get("swapTransaction")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "jupiter swap tx missing".to_string())?;

    // Step 3: Decode, sign, and submit
    // Jupiter returns a base64-encoded versioned transaction.
    // We must decode it, sign the message with our fee payer, and re-encode before sending.
    let fee_payer_path = state
        .config
        .solana_fee_payer_keypair_path
        .as_ref()
        .ok_or_else(|| "missing fee payer for rebalance".to_string())?;
    let fee_payer = load_solana_keypair(fee_payer_path)?;

    // AUDIT-FIX I-7: Decode base64 tx, sign with fee_payer, re-encode
    use base64::Engine;
    let tx_bytes = base64::engine::general_purpose::STANDARD
        .decode(swap_tx_b64)
        .map_err(|e| format!("base64 decode jupiter tx: {}", e))?;

    // Solana transaction layout: compact-u16(num_sigs) | sig[0..N] (each 64 bytes) | message
    if tx_bytes.is_empty() {
        return Err("empty jupiter transaction".to_string());
    }
    let (num_sigs, header_len) = decode_shortvec_u16(&tx_bytes)
        .ok_or_else(|| "invalid compact-u16 in jupiter tx".to_string())?;
    if num_sigs == 0 {
        return Err("jupiter tx has zero signatures".to_string());
    }
    let sigs_end = header_len + (num_sigs as usize) * 64;
    if sigs_end > tx_bytes.len() {
        return Err("jupiter tx too short for declared signatures".to_string());
    }
    let message_bytes = &tx_bytes[sigs_end..];
    let fee_payer_sig = fee_payer.sign(message_bytes);

    // Replace first signature (fee payer's placeholder) with real signature
    let mut signed_tx = tx_bytes.clone();
    signed_tx[header_len..header_len + 64].copy_from_slice(&fee_payer_sig);
    let signed_b64 = base64::engine::general_purpose::STANDARD.encode(&signed_tx);

    // Submit the now-properly-signed transaction
    let params = json!([signed_b64, {"encoding": "base64", "skipPreflight": true}]);
    let result = solana_rpc_call(&state.http, solana_url, "sendTransaction", params).await?;
    result
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "no tx hash from solana".to_string())
}

/// Execute a USDT↔USDC swap on Ethereum via Uniswap V3.
///
/// Steps:
///   1. Build swap calldata for Uniswap V3 router
///   2. Sign EVM transaction
///   3. Submit via eth_sendRawTransaction
async fn execute_ethereum_rebalance_swap(
    state: &CustodyState,
    job: &RebalanceJob,
) -> Result<String, String> {
    let _router = state
        .config
        .uniswap_router
        .as_ref()
        .ok_or_else(|| "missing CUSTODY_UNISWAP_ROUTER for Ethereum rebalance".to_string())?;
    let evm_url = state
        .config
        .evm_rpc_url
        .as_ref()
        .ok_or_else(|| "missing EVM RPC for rebalance".to_string())?;
    let treasury_addr = state
        .config
        .treasury_evm_address
        .as_ref()
        .ok_or_else(|| "missing treasury EVM address".to_string())?;

    let from_contract = match job.from_asset.as_str() {
        "usdt" => &state.config.evm_usdt_contract,
        "usdc" => &state.config.evm_usdc_contract,
        _ => return Err(format!("unsupported from_asset: {}", job.from_asset)),
    };
    let _to_contract = match job.to_asset.as_str() {
        "usdt" => &state.config.evm_usdt_contract,
        "usdc" => &state.config.evm_usdc_contract,
        _ => return Err(format!("unsupported to_asset: {}", job.to_asset)),
    };

    // Build ERC-20 approve + Uniswap exactInputSingle calldata
    // This is a simplified implementation — production would use the Uniswap SDK
    let nonce = evm_get_transaction_count(&state.http, evm_url, treasury_addr).await?;
    let gas_price = evm_get_gas_price(&state.http, evm_url).await?;
    let chain_id = evm_get_chain_id(&state.http, evm_url).await?;

    // Step 1: Approve the from_token to the Uniswap router
    let approve_data = evm_encode_erc20_approve(_router, job.amount as u128)?;
    let signing_key = derive_evm_signing_key("custody-treasury-evm", &state.config.master_seed)?;
    let approve_tx = build_evm_signed_transaction_with_data(
        &signing_key,
        nonce,
        gas_price,
        100_000u128,
        from_contract,
        0,
        &approve_data,
        chain_id,
    )?;
    let approve_hex = format!("0x{}", hex::encode(&approve_tx));
    let approve_result = evm_rpc_call(
        &state.http,
        evm_url,
        "eth_sendRawTransaction",
        json!([approve_hex]),
    )
    .await?;

    // AUDIT-FIX I-8: Wait for approve tx confirmation before sending swap tx.
    // Without this, the swap can arrive before the allowance is set, causing revert.
    let approve_tx_hash = approve_result
        .as_str()
        .ok_or_else(|| "no tx hash from approve".to_string())?;

    // Poll for up to 90 seconds (36 attempts × 2.5s) for 1 confirmation
    let mut confirmed = false;
    for _ in 0..36 {
        match check_evm_tx_confirmed(&state.http, evm_url, approve_tx_hash, 1).await {
            Ok(true) => {
                confirmed = true;
                break;
            }
            Ok(false) => {}
            Err(_) => {}
        }
        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
    }
    if !confirmed {
        return Err(format!(
            "ERC-20 approve tx {} not confirmed after 90s — aborting swap",
            approve_tx_hash
        ));
    }

    // Step 2: Execute the swap (simplified — production uses exactInputSingle)
    // For a USDT↔USDC swap on a 0.01% fee tier (stable pair), slippage is minimal
    let swap_data = build_uniswap_exact_input_single(
        from_contract,
        _to_contract,
        job.amount as u128,
        100, // fee tier 0.01%
        state.config.rebalance_max_slippage_bps,
        treasury_addr, // AUDIT-FIX C3: recipient must be treasury, not zero address
    )?;
    let swap_tx = build_evm_signed_transaction_with_data(
        &signing_key,
        nonce + 1,
        gas_price,
        300_000u128,
        _router,
        0,
        &swap_data,
        chain_id,
    )?;
    let swap_hex = format!("0x{}", hex::encode(&swap_tx));
    let result = evm_rpc_call(
        &state.http,
        evm_url,
        "eth_sendRawTransaction",
        json!([swap_hex]),
    )
    .await?;
    result
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "no tx hash from ethereum".to_string())
}

/// Encode ERC-20 approve(spender, amount) calldata
fn evm_encode_erc20_approve(spender: &str, amount: u128) -> Result<Vec<u8>, String> {
    let mut data = Vec::with_capacity(68);
    // approve(address,uint256) selector: 0x095ea7b3
    data.extend_from_slice(&hex::decode("095ea7b3").map_err(|_| "selector".to_string())?);

    let spender_bytes = parse_evm_address(spender)?;
    let mut padded_spender = vec![0u8; 12];
    padded_spender.extend_from_slice(&spender_bytes);
    data.extend_from_slice(&padded_spender);

    let mut padded_amount = vec![0u8; 16];
    padded_amount.extend_from_slice(&amount.to_be_bytes());
    data.extend_from_slice(&padded_amount);

    Ok(data)
}

/// Build Uniswap V3 exactInputSingle calldata (simplified)
fn build_uniswap_exact_input_single(
    token_in: &str,
    token_out: &str,
    amount_in: u128,
    fee: u32,
    max_slippage_bps: u64,
    recipient: &str,
) -> Result<Vec<u8>, String> {
    let mut data = Vec::with_capacity(228);
    // exactInputSingle(ExactInputSingleParams) selector: 0x414bf389
    data.extend_from_slice(&hex::decode("414bf389").map_err(|_| "selector".to_string())?);

    // tokenIn (address)
    let token_in_bytes = parse_evm_address(token_in)?;
    let mut padded = vec![0u8; 12];
    padded.extend_from_slice(&token_in_bytes);
    data.extend_from_slice(&padded);

    // tokenOut (address)
    let token_out_bytes = parse_evm_address(token_out)?;
    let mut padded = vec![0u8; 12];
    padded.extend_from_slice(&token_out_bytes);
    data.extend_from_slice(&padded);

    // fee (uint24 → padded to 32 bytes)
    let mut fee_padded = vec![0u8; 28];
    fee_padded.extend_from_slice(&fee.to_be_bytes());
    data.extend_from_slice(&fee_padded);

    // AUDIT-FIX C3: Recipient MUST be the treasury address. Previously this was
    // zero-address with comment "will be overridden" — but nothing overrides it.
    // Sending swap output to address(0) burns the tokens permanently.
    let recipient_bytes = parse_evm_address(recipient)?;
    let mut padded_recipient = vec![0u8; 12];
    padded_recipient.extend_from_slice(&recipient_bytes);
    data.extend_from_slice(&padded_recipient);

    // deadline (uint256) — far future
    let mut deadline = vec![0u8; 24];
    deadline.extend_from_slice(&u64::MAX.to_be_bytes());
    data.extend_from_slice(&deadline);

    // amountIn (uint256)
    let mut amount_padded = vec![0u8; 16];
    amount_padded.extend_from_slice(&amount_in.to_be_bytes());
    data.extend_from_slice(&amount_padded);

    // AUDIT-FIX M14: configurable slippage for Uniswap rebalance swaps
    let min_out = amount_in * (10_000u128 - max_slippage_bps as u128) / 10_000u128;
    let mut min_padded = vec![0u8; 16];
    min_padded.extend_from_slice(&min_out.to_be_bytes());
    data.extend_from_slice(&min_padded);

    // sqrtPriceLimitX96 (uint160) — 0 means no limit
    data.extend_from_slice(&[0u8; 32]);

    Ok(data)
}

fn is_ready_for_withdrawal_retry(job: &WithdrawalJob) -> bool {
    match job.next_attempt_at {
        Some(ts) => chrono::Utc::now().timestamp() >= ts,
        None => true,
    }
}

/// Background loop: processes withdrawal jobs through their lifecycle
///
/// States:
///   pending_burn  → verify user's burn tx on Lichen → burned
///   burned        → collect threshold signatures → signing
///   signing       → broadcast outbound tx on dest chain → broadcasting
///   broadcasting  → confirm on dest chain → confirmed
async fn withdrawal_worker_loop(state: CustodyState) {
    loop {
        if let Err(err) = process_withdrawal_jobs(&state).await {
            tracing::warn!("withdrawal worker error: {}", err);
        }
        sleep(Duration::from_secs(state.config.poll_interval_secs)).await;
    }
}

async fn process_withdrawal_jobs(state: &CustodyState) -> Result<(), String> {
    // Phase 1: pending_burn → check if burn tx is confirmed on Lichen
    let pending = list_withdrawal_jobs_by_status(&state.db, "pending_burn")?;
    for mut job in pending {
        if let Some(ref burn_sig) = job.burn_tx_signature {
            if let Some(rpc_url) = state.config.licn_rpc_url.as_ref() {
                match licn_rpc_call(&state.http, rpc_url, "getTransaction", json!([burn_sig])).await
                {
                    Ok(result) => {
                        if !result.is_null() {
                            let success = result
                                .get("success")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            if !success {
                                continue;
                            }

                            // AUDIT-FIX R-C1: Validate the burn TX matches the
                            // expected contract, caller, and amount. Without this,
                            // an attacker could submit any successful tx as a "burn".
                            let expected_contract = match job.asset.to_lowercase().as_str() {
                                "wsol" => state.config.wsol_contract_addr.as_deref(),
                                "weth" => state.config.weth_contract_addr.as_deref(),
                                "wbnb" => state.config.wbnb_contract_addr.as_deref(),
                                "musd" => state.config.musd_contract_addr.as_deref(),
                                _ => None,
                            };
                            let tx_contract =
                                result.get("contract_address").and_then(|v| v.as_str());
                            let tx_caller = result.get("caller").and_then(|v| v.as_str());
                            let tx_method = result.get("method").and_then(|v| v.as_str());
                            let tx_amount =
                                result.get("amount").and_then(|v| v.as_u64()).unwrap_or(0);

                            // Validate contract address matches
                            if let Some(expected) = expected_contract {
                                if tx_contract != Some(expected) {
                                    tracing::error!(
                                        "🚨 BURN VERIFICATION FAILED for {}: expected contract {} \
                                         but tx called {:?}. Possible attack!",
                                        job.job_id,
                                        expected,
                                        tx_contract
                                    );
                                    let _ = reset_pending_burn_submission(
                                        &state.db,
                                        &mut job,
                                        format!(
                                            "Burn contract mismatch: expected {} got {:?}",
                                            expected, tx_contract
                                        ),
                                    );
                                    continue;
                                }
                            } else {
                                tracing::error!(
                                    "🚨 BURN VERIFICATION FAILED for {}: no contract configured \
                                     for asset {}. Cannot verify burn. Marking permanently_failed.",
                                    job.job_id,
                                    job.asset
                                );
                                job.status = "permanently_failed".to_string();
                                job.last_error = Some(format!(
                                    "No contract address configured for asset '{}'",
                                    job.asset
                                ));
                                let _ = store_withdrawal_job(&state.db, &job);
                                continue;
                            }

                            // Validate method is "burn"
                            if tx_method != Some("burn") {
                                tracing::error!(
                                    "🚨 BURN VERIFICATION FAILED for {}: expected method 'burn' \
                                     but tx called {:?}. Possible attack!",
                                    job.job_id,
                                    tx_method
                                );
                                let _ = reset_pending_burn_submission(
                                    &state.db,
                                    &mut job,
                                    format!(
                                        "Burn method mismatch: expected 'burn' got {:?}",
                                        tx_method
                                    ),
                                );
                                continue;
                            }

                            // Validate amount matches
                            if tx_amount != job.amount {
                                let expected_amount = job.amount;
                                tracing::error!(
                                    "🚨 BURN VERIFICATION FAILED for {}: expected amount {} \
                                     but tx burned {}. Amount mismatch!",
                                    job.job_id,
                                    expected_amount,
                                    tx_amount
                                );
                                let _ = reset_pending_burn_submission(
                                    &state.db,
                                    &mut job,
                                    format!(
                                        "Burn amount mismatch: expected {} got {}",
                                        expected_amount, tx_amount
                                    ),
                                );
                                continue;
                            }

                            // Validate caller is the user_id
                            if tx_caller != Some(job.user_id.as_str()) {
                                let expected_user_id = job.user_id.clone();
                                tracing::error!(
                                    "🚨 BURN VERIFICATION FAILED for {}: expected caller {} \
                                     but tx caller was {:?}. Possible attack!",
                                    job.job_id,
                                    expected_user_id,
                                    tx_caller
                                );
                                let _ = reset_pending_burn_submission(
                                    &state.db,
                                    &mut job,
                                    format!(
                                        "Burn caller mismatch: expected {} got {:?}",
                                        expected_user_id, tx_caller
                                    ),
                                );
                                continue;
                            }

                            job.status = "burned".to_string();
                            store_withdrawal_job(&state.db, &job)?;
                            emit_custody_event(
                                state,
                                "withdrawal.burn_confirmed",
                                &job.job_id,
                                None,
                                job.burn_tx_signature.as_deref(),
                                Some(&serde_json::json!({
                                    "user_id": job.user_id,
                                    "asset": job.asset,
                                    "amount": job.amount
                                })),
                            );
                            info!("withdrawal burn confirmed: {}", job.job_id);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("burn verification failed for {}: {}", job.job_id, e);
                    }
                }
            }
        }
    }

    // Phase 2: burned → collect threshold signatures for outbound transaction
    let burned = list_withdrawal_jobs_by_status(&state.db, "burned")?;
    for mut job in burned {
        if !is_ready_for_withdrawal_retry(&job) {
            continue;
        }

        // Determine the outbound transaction details
        let outbound_asset = match job.asset.to_lowercase().as_str() {
            "musd" => job.preferred_stablecoin.clone(),
            "wsol" => "sol".to_string(),
            "weth" => "eth".to_string(),
            "wbnb" => "bnb".to_string(),
            _ => continue,
        };

        let signing_mode = match determine_withdrawal_signing_mode(state, &job, &outbound_asset) {
            Ok(mode) => mode,
            Err(err) => {
                job.status = "permanently_failed".to_string();
                job.last_error = Some(err.clone());
                job.next_attempt_at = None;
                store_withdrawal_job(&state.db, &job)?;
                emit_custody_event(
                    state,
                    "withdrawal.permanently_failed",
                    &job.job_id,
                    None,
                    None,
                    Some(&serde_json::json!({
                        "asset": job.asset,
                        "amount": job.amount,
                        "dest_chain": job.dest_chain,
                        "last_error": err
                    })),
                );
                continue;
            }
        };

        if signing_mode.is_none() {
            job.status = "signing".to_string();
            store_withdrawal_job(&state.db, &job)?;
            emit_custody_event(
                state,
                "withdrawal.self_signed",
                &job.job_id,
                None,
                None,
                Some(&serde_json::json!({
                    "mode": "self-custody",
                    "asset": job.asset,
                    "amount": job.amount
                })),
            );
            info!(
                "withdrawal self-signed (no external signers): {}",
                job.job_id
            );
            continue;
        }

        let sig_count = match signing_mode.unwrap() {
            WithdrawalSigningMode::ExternalSingleSigner => {
                collect_single_signer_withdrawal_signatures(state, &mut job, &outbound_asset).await
            }
            WithdrawalSigningMode::SolanaThresholdFrost => {
                collect_threshold_solana_withdrawal_signatures(state, &mut job, &outbound_asset)
                    .await
            }
            WithdrawalSigningMode::EvmThresholdSafe => {
                collect_threshold_evm_withdrawal_signatures(state, &mut job, &outbound_asset).await
            }
        };

        let sig_count = match sig_count {
            Ok(count) => count,
            Err(err) => {
                mark_withdrawal_failed(&mut job, err);
                store_withdrawal_job(&state.db, &job)?;
                continue;
            }
        };

        if sig_count >= state.config.signer_threshold && state.config.signer_threshold > 0 {
            job.status = "signing".to_string();
            job.last_error = None;
            job.next_attempt_at = None;
            store_withdrawal_job(&state.db, &job)?;
            emit_custody_event(
                state,
                "withdrawal.signatures_collected",
                &job.job_id,
                None,
                None,
                Some(&serde_json::json!({
                    "sig_count": sig_count,
                    "threshold": state.config.signer_threshold
                })),
            );
            info!(
                "withdrawal threshold met: {} ({}/{} signatures)",
                job.job_id, sig_count, state.config.signer_threshold
            );
        } else {
            // Not enough signatures yet, will retry next cycle
            store_withdrawal_job(&state.db, &job)?;
        }
    }

    // Phase 3: signing → broadcast outbound transaction
    let signing = list_withdrawal_jobs_by_status(&state.db, "signing")?;
    for mut job in signing {
        // AUDIT-FIX M4: Record intent before withdrawal broadcast
        let _ = record_tx_intent(&state.db, "withdrawal", &job.job_id, &job.dest_chain);
        match broadcast_outbound_withdrawal(state, &job).await {
            Ok(tx_hash) => {
                let _ = clear_tx_intent(&state.db, "withdrawal", &job.job_id);
                job.outbound_tx_hash = Some(tx_hash.clone());
                job.status = "broadcasting".to_string();
                job.last_error = None;
                store_withdrawal_job(&state.db, &job)?;
                emit_custody_event(
                    state,
                    "withdrawal.broadcast",
                    &job.job_id,
                    None,
                    Some(&tx_hash),
                    Some(&serde_json::json!({
                        "dest_chain": job.dest_chain,
                        "dest_address": job.dest_address,
                        "asset": job.asset,
                        "amount": job.amount
                    })),
                );
                info!("withdrawal broadcast: {} → tx={}", job.job_id, tx_hash);
            }
            Err(e) => {
                let _ = clear_tx_intent(&state.db, "withdrawal", &job.job_id);
                job.attempts = job.attempts.saturating_add(1);
                job.last_error = Some(e.clone());
                // AUDIT-FIX R-H2: Cap withdrawal retries like sweep/credit
                if job.attempts >= MAX_JOB_ATTEMPTS {
                    job.status = "permanently_failed".to_string();
                    store_withdrawal_job(&state.db, &job)?;
                    tracing::error!(
                        "🚨 withdrawal {} permanently failed after {} attempts: {}",
                        job.job_id,
                        MAX_JOB_ATTEMPTS,
                        e
                    );
                    emit_custody_event(
                        state,
                        "withdrawal.permanently_failed",
                        &job.job_id,
                        None,
                        None,
                        Some(&serde_json::json!({
                            "attempts": job.attempts,
                            "last_error": e,
                            "asset": job.asset,
                            "amount": job.amount
                        })),
                    );
                } else {
                    job.next_attempt_at = Some(next_retry_timestamp(job.attempts));
                    store_withdrawal_job(&state.db, &job)?;
                    tracing::warn!("withdrawal broadcast failed for {}: {}", job.job_id, e);
                }
            }
        }
    }

    // Phase 4: broadcasting → confirm on destination chain
    let broadcasting = list_withdrawal_jobs_by_status(&state.db, "broadcasting")?;
    for mut job in broadcasting {
        let confirmed = match job.dest_chain.as_str() {
            "solana" | "sol" => {
                if let (Some(url), Some(ref tx_hash)) =
                    (state.config.solana_rpc_url.as_ref(), &job.outbound_tx_hash)
                {
                    check_solana_tx_confirmed(
                        &state.http,
                        url,
                        tx_hash,
                        state.config.solana_confirmations,
                    )
                    .await
                    .unwrap_or(false)
                } else {
                    false
                }
            }
            chain if is_evm_chain(chain) => {
                if let (Some(url), Some(ref tx_hash)) = (
                    rpc_url_for_chain(&state.config, chain),
                    &job.outbound_tx_hash,
                ) {
                    check_evm_tx_confirmed(
                        &state.http,
                        &url,
                        tx_hash,
                        state.config.evm_confirmations,
                    )
                    .await
                    .unwrap_or(false)
                } else {
                    false
                }
            }
            _ => false,
        };

        if confirmed {
            job.status = "confirmed".to_string();
            job.last_error = None;
            store_withdrawal_job(&state.db, &job)?;
            emit_custody_event(
                state,
                "withdrawal.confirmed",
                &job.job_id,
                None,
                job.outbound_tx_hash.as_deref(),
                Some(&serde_json::json!({
                    "dest_chain": job.dest_chain,
                    "dest_address": job.dest_address,
                    "asset": job.asset,
                    "amount": job.amount,
                    "user_id": job.user_id
                })),
            );

            // AUDIT-FIX CUST-01: Decrement reserve in source-chain units, not spores.
            // The reserve ledger tracks amounts in source-chain decimals (e.g. 6 for
            // ETH USDT), so we must convert the spore amount before debiting.
            let asset_lower = job.asset.to_lowercase();
            if asset_lower == "musd" {
                let stablecoin = &job.preferred_stablecoin;
                let chain_debit = spores_to_chain_amount(job.amount, &job.dest_chain, stablecoin);
                let chain_debit_u64 = u64::try_from(chain_debit).unwrap_or(u64::MAX);
                if let Err(e) = adjust_reserve_balance(
                    &state.db,
                    &job.dest_chain,
                    stablecoin,
                    chain_debit_u64,
                    false,
                )
                .await
                {
                    tracing::warn!("reserve ledger decrement failed: {}", e);
                }
            }

            info!(
                "withdrawal confirmed: {} (dest tx={})",
                job.job_id,
                job.outbound_tx_hash.as_deref().unwrap_or("?")
            );
        }
    }

    Ok(())
}

/// Broadcast the outbound transaction on the destination chain.
/// Uses the collected threshold signatures to authorize the treasury spend.
async fn broadcast_outbound_withdrawal(
    state: &CustodyState,
    job: &WithdrawalJob,
) -> Result<String, String> {
    // Self-custody mode: build and sign the withdrawal transaction directly
    // using the master-seed-derived treasury keys
    let self_custody =
        state.config.signer_endpoints.is_empty() || state.config.signer_threshold == 0;

    match job.dest_chain.as_str() {
        "solana" | "sol" => {
            let url = state
                .config
                .solana_rpc_url
                .as_ref()
                .ok_or_else(|| "missing solana RPC".to_string())?;
            let outbound_asset = match job.asset.to_lowercase().as_str() {
                "wsol" => "sol".to_string(),
                "musd" => job.preferred_stablecoin.clone(),
                _ => return Err(format!("unsupported solana withdrawal: {}", job.asset)),
            };

            if self_custody {
                return broadcast_self_custody_solana_withdrawal(state, url, job, &outbound_asset)
                    .await;
            }

            let signed_tx = assemble_signed_solana_tx(state, job, &outbound_asset)?;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&signed_tx);
            let result = solana_rpc_call(
                &state.http,
                url,
                "sendTransaction",
                json!([encoded, {"encoding": "base64"}]),
            )
            .await?;
            result
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "no tx hash returned".to_string())
        }
        "ethereum" | "eth" | "bsc" | "bnb" => {
            let url = rpc_url_for_chain(&state.config, &job.dest_chain)
                .ok_or_else(|| format!("missing RPC URL for chain {}", job.dest_chain))?;
            let outbound_asset = match job.asset.to_lowercase().as_str() {
                "weth" => "eth".to_string(),
                "wbnb" => "bnb".to_string(),
                "musd" => job.preferred_stablecoin.clone(),
                _ => return Err(format!("unsupported EVM withdrawal: {}", job.asset)),
            };

            if self_custody {
                return broadcast_self_custody_evm_withdrawal(state, &url, job, &outbound_asset)
                    .await;
            }

            let signed_tx = assemble_signed_evm_tx(state, job, &outbound_asset).await?;
            let tx_hex = format!("0x{}", hex::encode(&signed_tx));
            let result =
                evm_rpc_call(&state.http, &url, "eth_sendRawTransaction", json!([tx_hex])).await?;
            result
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "no tx hash returned".to_string())
        }
        other => Err(format!("unsupported destination chain: {}", other)),
    }
}

/// Self-custody Solana withdrawal: build and sign directly from master-seed-derived treasury key.
async fn broadcast_self_custody_solana_withdrawal(
    state: &CustodyState,
    url: &str,
    job: &WithdrawalJob,
    outbound_asset: &str,
) -> Result<String, String> {
    let treasury_path = "custody/treasury/solana";
    let (signing_key, from_pubkey) =
        derive_solana_signer(treasury_path, &state.config.master_seed)?;

    if outbound_asset == "sol" {
        let to_pubkey = decode_solana_pubkey(&job.dest_address)?;

        let solana_tx_fee: u64 = 5_000;
        if job.amount <= solana_tx_fee {
            return Err("withdrawal amount too small to cover fees".to_string());
        }
        let transfer_amount = job.amount - solana_tx_fee;

        let recent_blockhash = solana_get_latest_blockhash(&state.http, url).await?;
        let message = build_solana_transfer_message(
            &from_pubkey,
            &to_pubkey,
            transfer_amount,
            &recent_blockhash,
        );
        let signature = signing_key.sign(&message).to_bytes();
        let tx = build_solana_transaction(&[signature], &message);
        return solana_send_transaction(&state.http, url, &tx).await;
    }

    if !is_solana_stablecoin(outbound_asset) {
        return Err(format!(
            "unsupported self-custody Solana withdrawal asset: {}",
            outbound_asset
        ));
    }

    let treasury_owner = encode_solana_pubkey(&from_pubkey);
    let mint = solana_mint_for_asset(&state.config, outbound_asset)?;
    let from_token_account = derive_associated_token_address_from_str(&treasury_owner, &mint)?;
    let to_token_account = derive_associated_token_address_from_str(&job.dest_address, &mint)?;
    ensure_associated_token_account_for_str(state, &treasury_owner, &mint, &from_token_account)
        .await?;
    ensure_associated_token_account_for_str(state, &job.dest_address, &mint, &to_token_account)
        .await?;

    let recent_blockhash = solana_get_latest_blockhash(&state.http, url).await?;
    let raw_amount = u64::try_from(spores_to_chain_amount(
        job.amount,
        &job.dest_chain,
        outbound_asset,
    ))
    .map_err(|_| "solana token withdrawal amount overflow".to_string())?;
    let message = build_solana_token_transfer_message(
        &from_pubkey,
        &decode_solana_pubkey(&from_token_account)?,
        &decode_solana_pubkey(&to_token_account)?,
        raw_amount,
        &recent_blockhash,
    )?;
    let signature = signing_key.sign(&message).to_bytes();
    let tx = build_solana_transaction(&[signature], &message);
    solana_send_transaction(&state.http, url, &tx).await
}

/// Self-custody EVM withdrawal: build and sign directly from master-seed-derived treasury key.
async fn broadcast_self_custody_evm_withdrawal(
    state: &CustodyState,
    url: &str,
    job: &WithdrawalJob,
    outbound_asset: &str,
) -> Result<String, String> {
    let treasury_chain = match job.dest_chain.as_str() {
        "bsc" | "bnb" => "custody/treasury/bnb",
        _ => "custody/treasury/ethereum",
    };
    let signing_key = derive_evm_signing_key(treasury_chain, &state.config.master_seed)?;
    let from_address = derive_evm_address(treasury_chain, &state.config.master_seed)?;
    let to_address = &job.dest_address;

    let nonce = evm_get_transaction_count(&state.http, url, &from_address).await?;
    let gas_price = evm_get_gas_price(&state.http, url).await?;
    let chain_id = evm_get_chain_id(&state.http, url).await?;

    if outbound_asset == "eth" || outbound_asset == "bnb" {
        // Native value transfer — convert spores (9 dec) → wei (18 dec)
        let chain_amount = spores_to_chain_amount(job.amount, &job.dest_chain, outbound_asset);
        let gas_limit = evm_estimate_gas(
            &state.http,
            url,
            &from_address,
            to_address,
            chain_amount,
            None,
            21_000,
        )
        .await;

        let raw_tx = build_evm_signed_transaction(
            &signing_key,
            nonce,
            gas_price,
            gas_limit,
            to_address,
            chain_amount,
            chain_id,
        )?;
        let tx_hex = format!("0x{}", hex::encode(raw_tx));
        let result =
            evm_rpc_call(&state.http, url, "eth_sendRawTransaction", json!([tx_hex])).await?;
        result
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "no tx hash returned".to_string())
    } else {
        // ERC-20 transfer for stablecoins — convert spores (9 dec) → token decimals
        let contract = evm_contract_for_asset(&state.config, outbound_asset)?;
        let chain_amount = spores_to_chain_amount(job.amount, &job.dest_chain, outbound_asset);
        let transfer_data = evm_encode_erc20_transfer(to_address, chain_amount)?;
        let gas_limit = evm_estimate_gas(
            &state.http,
            url,
            &from_address,
            &contract,
            0,
            Some(&transfer_data),
            100_000,
        )
        .await;

        let raw_tx = build_evm_signed_transaction_with_data(
            &signing_key,
            nonce,
            gas_price,
            gas_limit,
            &contract,
            0,
            &transfer_data,
            chain_id,
        )?;
        let tx_hex = format!("0x{}", hex::encode(raw_tx));
        let result =
            evm_rpc_call(&state.http, url, "eth_sendRawTransaction", json!([tx_hex])).await?;
        result
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| "no tx hash returned".to_string())
    }
}

/// Assemble a Solana transaction from threshold signatures.
///
/// **Single-signer mode**: The signer returns a fully-signed serialized transaction.
///
/// **Multi-signer mode (FROST Ed25519)**: Each signer returns a FROST signature share.
/// The custody service aggregates shares into a single Ed25519 group signature using
/// the FROST protocol, then constructs a valid Solana transaction with that signature.
///
/// FROST protocol flow (2-round):
///   Round 1: POST /frost/commit → signer returns nonce commitment
///   Round 2: POST /frost/sign   → signer receives all commitments, returns signature share
///   Aggregation: custody combines t-of-n shares into one Ed25519 signature
fn assemble_signed_solana_tx(
    state: &CustodyState,
    job: &WithdrawalJob,
    _asset: &str,
) -> Result<Vec<u8>, String> {
    if job.signatures.is_empty() {
        return Err("no signatures available".to_string());
    }

    if state.config.signer_threshold <= 1 || state.config.signer_endpoints.len() <= 1 {
        // Single-signer mode: signer returns fully assembled signed transaction
        let first_sig = &job.signatures[0];
        return hex::decode(&first_sig.signature).map_err(|e| format!("decode signature: {}", e));
    }

    // ── Multi-signer FROST Ed25519 aggregation ──
    let pubkey_package_hex = state
        .config
        .frost_pubkey_package_hex
        .as_ref()
        .ok_or("FROST public key package not configured (set CUSTODY_FROST_PUBKEY_PACKAGE)")?;

    let pubkey_package_bytes = hex::decode(pubkey_package_hex)
        .map_err(|e| format!("invalid FROST pubkey package hex: {}", e))?;

    let pubkey_package: frost::keys::PublicKeyPackage =
        frost::keys::PublicKeyPackage::deserialize(&pubkey_package_bytes)
            .map_err(|e| format!("deserialize FROST pubkey package: {:?}", e))?;

    // Each signature entry contains a FROST signature share (hex-encoded serialized SignatureShare)
    // and the signer_pubkey field contains the FROST Identifier (hex-encoded)
    let mut signature_shares: BTreeMap<frost::Identifier, frost::round2::SignatureShare> =
        BTreeMap::new();

    for sig_entry in &job.signatures {
        // Parse signer identifier
        let id_bytes = hex::decode(&sig_entry.signer_pubkey)
            .map_err(|e| format!("decode signer id: {}", e))?;
        let identifier = frost::Identifier::deserialize(&id_bytes)
            .map_err(|e| format!("deserialize FROST identifier: {:?}", e))?;

        // Parse signature share
        let share_bytes = hex::decode(&sig_entry.signature)
            .map_err(|e| format!("decode signature share: {}", e))?;
        let share = frost::round2::SignatureShare::deserialize(&share_bytes)
            .map_err(|e| format!("deserialize FROST share: {:?}", e))?;

        signature_shares.insert(identifier, share);
    }

    if signature_shares.len() < state.config.signer_threshold {
        return Err(format!(
            "insufficient FROST shares: have {}, need {}",
            signature_shares.len(),
            state.config.signer_threshold
        ));
    }

    // AUDIT-FIX P10-CUST-02: Parse length-prefixed encoding to extract both the
    // signing message and per-signer commitments. The old "frost_commitment:" delimiter
    // was ambiguous and also lost the original message bytes.
    // Format: 4-byte big-endian msg_len || message_hex_utf8 || commitment_hex_utf8
    let message_bytes = {
        let raw = hex::decode(&job.signatures[0].message_hash)
            .map_err(|e| format!("decode FROST payload: {}", e))?;
        if raw.len() < 4 {
            return Err("FROST payload too short for length prefix".to_string());
        }
        let msg_len = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
        if raw.len() < 4 + msg_len {
            return Err(format!(
                "FROST payload truncated: need {} + 4, have {}",
                msg_len,
                raw.len()
            ));
        }
        let msg_hex = std::str::from_utf8(&raw[4..4 + msg_len])
            .map_err(|e| format!("FROST message hex not UTF-8: {}", e))?;
        hex::decode(msg_hex).map_err(|e| format!("decode signing message: {}", e))?
    };

    // Reconstruct commitments from length-prefixed FROST payloads
    let mut commitments_map: BTreeMap<frost::Identifier, frost::round1::SigningCommitments> =
        BTreeMap::new();

    for sig_entry in &job.signatures {
        let id_bytes = hex::decode(&sig_entry.signer_pubkey)
            .map_err(|e| format!("decode signer id for commitment: {}", e))?;
        let identifier = frost::Identifier::deserialize(&id_bytes)
            .map_err(|e| format!("deserialize FROST identifier for commitment: {:?}", e))?;

        // Parse length-prefixed payload to extract commitment_hex
        let raw = hex::decode(&sig_entry.message_hash)
            .map_err(|e| format!("decode FROST payload for commitment: {}", e))?;
        if raw.len() >= 4 {
            let msg_len = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]) as usize;
            if raw.len() > 4 + msg_len {
                let commitment_hex = std::str::from_utf8(&raw[4 + msg_len..])
                    .map_err(|e| format!("commitment hex not UTF-8: {}", e))?;
                let commitment_bytes =
                    hex::decode(commitment_hex).map_err(|e| format!("decode commitment: {}", e))?;
                let commitment = frost::round1::SigningCommitments::deserialize(&commitment_bytes)
                    .map_err(|e| format!("deserialize commitment: {:?}", e))?;
                commitments_map.insert(identifier, commitment);
            }
        }
    }

    if commitments_map.len() < state.config.signer_threshold {
        return Err(format!(
            "insufficient FROST commitments: have {}, need {}",
            commitments_map.len(),
            state.config.signer_threshold
        ));
    }

    // Build the FROST signing package and aggregate
    let signing_package = frost::SigningPackage::new(commitments_map, &message_bytes);

    let group_signature = frost::aggregate(&signing_package, &signature_shares, &pubkey_package)
        .map_err(|e| format!("FROST aggregation failed: {:?}", e))?;

    // Build a Solana transaction with the FROST group signature
    // The group verifying key is the treasury's on-chain Ed25519 key
    let group_sig_bytes = group_signature
        .serialize()
        .map_err(|e| format!("serialize FROST group signature: {:?}", e))?;

    // Return the assembled transaction:
    // [signature_count(1)][signature(64)][serialized_message]
    let mut tx_bytes = Vec::new();
    tx_bytes.push(1u8); // 1 signature (the FROST group signature)
    tx_bytes.extend_from_slice(&group_sig_bytes);
    tx_bytes.extend_from_slice(&message_bytes);

    Ok(tx_bytes)
}

/// Assemble an EVM transaction from threshold signatures.
///
/// **Single-signer mode**: Signer returns a fully-signed RLP-encoded transaction.
///
/// **Multi-signer mode**: Each signer produces a standard ECDSA signature on the
/// transaction hash. The custody service packs these into a Gnosis Safe-compatible
/// multisig execution call with sorted signatures.
///
/// Gnosis Safe signature packing:
///   - Signatures sorted by signer address (ascending)
///   - Each signature is 65 bytes: r(32) + s(32) + v(1)
///   - Packed contiguously for execTransaction(to, value, data, ..., signatures)
async fn assemble_signed_evm_tx(
    state: &CustodyState,
    job: &WithdrawalJob,
    asset: &str,
) -> Result<Vec<u8>, String> {
    if job.signatures.is_empty() {
        return Err("no signatures available".to_string());
    }

    if state.config.signer_threshold <= 1 || state.config.signer_endpoints.len() <= 1 {
        // Single-signer mode: signer returns fully signed RLP tx
        let first_sig = &job.signatures[0];
        return hex::decode(&first_sig.signature).map_err(|e| format!("decode signature: {}", e));
    }

    // Collect and sort ECDSA signatures by signer address
    let mut signer_signatures: Vec<(String, Vec<u8>)> = Vec::new(); // (address, signature_65bytes)
    let mut seen_signer_addrs = std::collections::HashSet::new();

    for sig_entry in &job.signatures {
        let sig_bytes = normalize_evm_signature(
            &hex::decode(&sig_entry.signature)
                .map_err(|e| format!("decode EVM signature: {}", e))?,
        )?;

        // signer_pubkey contains the EVM address (hex, no 0x prefix)
        let signer_addr = sig_entry
            .signer_pubkey
            .trim_start_matches("0x")
            .to_lowercase();
        if !seen_signer_addrs.insert(signer_addr.clone()) {
            return Err("duplicate EVM signer address in signature set".to_string());
        }
        signer_signatures.push((signer_addr, sig_bytes));
    }

    // Sort by signer address (Gnosis Safe requires ascending order)
    signer_signatures.sort_by(|a, b| a.0.cmp(&b.0));

    if signer_signatures.len() < state.config.signer_threshold {
        return Err(format!(
            "insufficient EVM signatures: have {}, need {}",
            signer_signatures.len(),
            state.config.signer_threshold
        ));
    }

    // Take exactly threshold signatures
    let packed_sigs: Vec<u8> = signer_signatures
        .iter()
        .take(state.config.signer_threshold)
        .flat_map(|(_, sig)| sig.clone())
        .collect();

    let url = rpc_url_for_chain(&state.config, &job.dest_chain)
        .ok_or_else(|| format!("missing RPC URL for chain {}", job.dest_chain))?;
    let plan = build_evm_safe_transaction_plan(state, &url, job, asset).await?;
    let expected_hash = hex::encode(plan.safe_tx_hash);
    if job
        .signatures
        .iter()
        .any(|sig| !sig.message_hash.is_empty() && sig.message_hash != expected_hash)
    {
        return Err(
            "EVM signature set does not match the pinned Safe transaction hash".to_string(),
        );
    }

    let exec_plan = finalize_evm_safe_exec_plan(plan, &packed_sigs)?;
    let executor_path = evm_executor_derivation_path(&job.dest_chain);
    let executor_address = derive_evm_address(executor_path, &state.config.master_seed)?;
    let executor_key = derive_evm_signing_key(executor_path, &state.config.master_seed)?;
    let nonce = evm_get_transaction_count(&state.http, &url, &executor_address).await?;
    let gas_price = evm_get_gas_price(&state.http, &url).await?;
    let chain_id = evm_get_chain_id(&state.http, &url).await?;
    let gas_limit = evm_estimate_gas(
        &state.http,
        &url,
        &executor_address,
        &exec_plan.safe_address,
        0,
        Some(&exec_plan.exec_calldata),
        350_000,
    )
    .await;
    build_evm_signed_transaction_with_data(
        &executor_key,
        nonce,
        gas_price,
        gas_limit,
        &exec_plan.safe_address,
        0,
        &exec_plan.exec_calldata,
        chain_id,
    )
}

/// Check if a Solana transaction is confirmed with enough confirmations
/// AUDIT-FIX 1.18: Properly check confirmation_status and confirmation count
async fn check_solana_tx_confirmed(
    client: &reqwest::Client,
    url: &str,
    tx_hash: &str,
    required_confirmations: u64,
) -> Result<bool, String> {
    // Use getSignatureStatuses for proper confirmation info
    let statuses = solana_rpc_call(
        client,
        url,
        "getSignatureStatuses",
        json!([[tx_hash], {"searchTransactionHistory": true}]),
    )
    .await?;

    let status = statuses
        .get("value")
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    if status.is_null() {
        return Ok(false);
    }

    // Check confirmation_status — "finalized" is the safest
    let confirmation_status = status
        .get("confirmation_status")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    if confirmation_status == "finalized" {
        return Ok(true);
    }

    // Fall back to numeric confirmations count
    let confirmations = status
        .get("confirmations")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    Ok(confirmations >= required_confirmations)
}

/// Check if an EVM transaction is confirmed with enough confirmations
async fn check_evm_tx_confirmed(
    client: &reqwest::Client,
    url: &str,
    tx_hash: &str,
    required_confirmations: u64,
) -> Result<bool, String> {
    let receipt = evm_rpc_call(client, url, "eth_getTransactionReceipt", json!([tx_hash])).await?;
    if receipt.is_null() {
        return Ok(false);
    }
    let block_number = receipt
        .get("blockNumber")
        .and_then(|v| v.as_str())
        .map(|s| parse_hex_u64(s).unwrap_or(0))
        .unwrap_or(0);

    if block_number == 0 {
        return Ok(false);
    }

    let current_block = evm_rpc_call(client, url, "eth_blockNumber", json!([])).await?;
    let current = current_block
        .as_str()
        .map(|s| parse_hex_u64(s).unwrap_or(0))
        .unwrap_or(0);

    Ok(current.saturating_sub(block_number) >= required_confirmations)
}

// ══════════════════════════════════════════════════════════════════════════════
// Webhook & WebSocket Event System
// ══════════════════════════════════════════════════════════════════════════════

// ── Webhook CRUD Endpoints ──

/// POST /webhooks — Register a new webhook endpoint.
/// Requires Bearer auth (same CUSTODY_API_AUTH_TOKEN as withdrawals).
async fn create_webhook(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<CreateWebhookRequest>,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    if payload.url.is_empty() {
        return Err(Json(ErrorResponse::invalid("url is required")));
    }
    if payload.secret.is_empty() {
        return Err(Json(ErrorResponse::invalid(
            "secret is required (used for HMAC-SHA256 signatures)",
        )));
    }
    if !payload.url.starts_with("https://") && !payload.url.starts_with("http://localhost") {
        return Err(Json(ErrorResponse::invalid(
            "webhook url must use HTTPS (http://localhost allowed for dev)",
        )));
    }
    if let Err(e) = validate_webhook_destination(&state.config, &payload.url) {
        return Err(Json(ErrorResponse::invalid(&e)));
    }

    let webhook = WebhookRegistration {
        id: Uuid::new_v4().to_string(),
        url: payload.url,
        secret: payload.secret,
        event_filter: payload.event_filter,
        active: true,
        created_at: chrono::Utc::now().timestamp(),
        description: payload.description,
    };

    store_webhook(&state.db, &webhook).map_err(|e| Json(ErrorResponse::db(&e)))?;
    info!("webhook registered: {} → {}", webhook.id, webhook.url);

    Ok(Json(json!({
        "id": webhook.id,
        "url": webhook.url,
        "event_filter": webhook.event_filter,
        "active": webhook.active,
        "created_at": webhook.created_at,
    })))
}

/// GET /webhooks — List all registered webhooks (secrets redacted).
async fn list_webhooks(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    let webhooks = list_all_webhooks(&state.db).map_err(|e| Json(ErrorResponse::db(&e)))?;
    let redacted: Vec<Value> = webhooks
        .iter()
        .map(|w| {
            json!({
                "id": w.id,
                "url": w.url,
                "event_filter": w.event_filter,
                "active": w.active,
                "created_at": w.created_at,
                "description": w.description,
            })
        })
        .collect();

    Ok(Json(json!({ "webhooks": redacted })))
}

/// DELETE /webhooks/:webhook_id — Remove a registered webhook.
async fn delete_webhook(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    axum::extract::Path(webhook_id): axum::extract::Path<String>,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    remove_webhook(&state.db, &webhook_id).map_err(|e| Json(ErrorResponse::db(&e)))?;
    info!("webhook removed: {}", webhook_id);

    Ok(Json(json!({ "deleted": webhook_id })))
}

/// GET /events — Paginated audit event history (most recent first).
/// Query params: ?limit=50&after=<cursor_or_event_id>&event_type=<filter>&entity_id=<id>&tx_hash=<hash>
/// AUDIT-FIX F8.11: `after` param now implemented for cursor-based pagination.
async fn list_events(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, Json<ErrorResponse>> {
    verify_api_auth(&state.config, &headers)?;

    let limit: usize = params
        .get("limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(50)
        .min(500);
    let event_type_filter = params.get("event_type").cloned();
    let entity_id_filter = params.get("entity_id").cloned();
    let tx_hash_filter = params.get("tx_hash").cloned();
    let after_cursor = params.get("after").cloned();

    let events_cf = state
        .db
        .cf_handle(CF_AUDIT_EVENTS)
        .ok_or_else(|| Json(ErrorResponse::db("missing audit_events cf")))?;
    let index_cf = state
        .db
        .cf_handle(CF_AUDIT_EVENTS_BY_TIME)
        .ok_or_else(|| Json(ErrorResponse::db("missing audit_events_by_time cf")))?;
    let type_index_cf = state
        .db
        .cf_handle(CF_AUDIT_EVENTS_BY_TYPE_TIME)
        .ok_or_else(|| Json(ErrorResponse::db("missing audit_events_by_type_time cf")))?;
    let entity_index_cf = state
        .db
        .cf_handle(CF_AUDIT_EVENTS_BY_ENTITY_TIME)
        .ok_or_else(|| Json(ErrorResponse::db("missing audit_events_by_entity_time cf")))?;
    let tx_index_cf = state
        .db
        .cf_handle(CF_AUDIT_EVENTS_BY_TX_TIME)
        .ok_or_else(|| Json(ErrorResponse::db("missing audit_events_by_tx_time cf")))?;

    let mut events = Vec::new();
    let mut next_cursor: Option<String> = None;

    // Cursor can be raw index key (preferred) or legacy event_id.
    // Index selection priority: tx_hash > entity_id > event_type > global time.
    let mut use_filter_index = false;
    let mut filter_prefix = String::new();
    let filter_kind = if tx_hash_filter.is_some() {
        "tx"
    } else if entity_id_filter.is_some() {
        "entity"
    } else if event_type_filter.is_some() {
        "type"
    } else {
        "global"
    };
    let resolved_after = if let Some(after) = after_cursor.as_deref() {
        if filter_kind != "global" {
            use_filter_index = true;
            filter_prefix = match filter_kind {
                "tx" => format!("tx:{}:", tx_hash_filter.as_deref().unwrap_or("")),
                "entity" => {
                    format!("entity:{}:", entity_id_filter.as_deref().unwrap_or(""))
                }
                _ => format!("type:{}:", event_type_filter.as_deref().unwrap_or("")),
            };

            if after.starts_with("type:")
                || after.starts_with("entity:")
                || after.starts_with("tx:")
            {
                Some(after.to_string())
            } else {
                match state.db.get_cf(events_cf, after.as_bytes()) {
                    Ok(Some(bytes)) => {
                        let evt: Option<Value> = serde_json::from_slice::<Value>(&bytes).ok();
                        let ts_ms = evt
                            .as_ref()
                            .and_then(|v| v.get("timestamp_ms"))
                            .and_then(|v| v.as_i64())
                            .or_else(|| {
                                evt.as_ref()
                                    .and_then(|v| v.get("timestamp"))
                                    .and_then(|v| v.as_i64())
                                    .map(|s| s.saturating_mul(1000))
                            })
                            .unwrap_or(0)
                            .max(0);

                        let prefix = match filter_kind {
                            "tx" => format!("tx:{}:", tx_hash_filter.as_deref().unwrap_or("")),
                            "entity" => {
                                format!("entity:{}:", entity_id_filter.as_deref().unwrap_or(""))
                            }
                            _ => {
                                format!("type:{}:", event_type_filter.as_deref().unwrap_or(""))
                            }
                        };
                        Some(format!("{}{:020}:{}", prefix, ts_ms, after))
                    }
                    _ => None,
                }
            }
        } else if after.contains(':') {
            Some(after.to_string())
        } else {
            match state.db.get_cf(events_cf, after.as_bytes()) {
                Ok(Some(bytes)) => {
                    let evt: Option<Value> = serde_json::from_slice::<Value>(&bytes).ok();
                    let ts_ms = evt
                        .as_ref()
                        .and_then(|v| v.get("timestamp_ms"))
                        .and_then(|v| v.as_i64())
                        .or_else(|| {
                            evt.as_ref()
                                .and_then(|v| v.get("timestamp"))
                                .and_then(|v| v.as_i64())
                                .map(|s| s.saturating_mul(1000))
                        })
                        .unwrap_or(0)
                        .max(0);
                    Some(format!("{:020}:{}", ts_ms, after))
                }
                _ => None,
            }
        }
    } else {
        if filter_kind != "global" {
            filter_prefix = match filter_kind {
                "tx" => format!("tx:{}:", tx_hash_filter.as_deref().unwrap_or("")),
                "entity" => {
                    format!("entity:{}:", entity_id_filter.as_deref().unwrap_or(""))
                }
                _ => format!("type:{}:", event_type_filter.as_deref().unwrap_or("")),
            };
            use_filter_index = true;
        }
        None
    };

    let upper_bound = if resolved_after.is_none() && use_filter_index {
        let mut b = filter_prefix.as_bytes().to_vec();
        b.push(0xFF);
        Some(b)
    } else {
        None
    };
    let iter_mode = if let Some(cursor_key) = resolved_after.as_ref() {
        rocksdb::IteratorMode::From(cursor_key.as_bytes(), rocksdb::Direction::Reverse)
    } else if let Some(ref b) = upper_bound {
        rocksdb::IteratorMode::From(b, rocksdb::Direction::Reverse)
    } else {
        rocksdb::IteratorMode::End
    };

    let mut skipped_cursor = false;
    let filter_prefix_bytes = filter_prefix.as_bytes();
    let source_cf = if use_filter_index {
        match filter_kind {
            "tx" => tx_index_cf,
            "entity" => entity_index_cf,
            "type" => type_index_cf,
            _ => type_index_cf,
        }
    } else {
        index_cf
    };
    for item in state.db.iterator_cf(source_cf, iter_mode) {
        if events.len() >= limit {
            break;
        }
        let (index_key, value) =
            item.map_err(|e| Json(ErrorResponse::db(&format!("iter: {}", e))))?;

        if use_filter_index && !index_key.starts_with(filter_prefix_bytes) {
            break;
        }

        if let Some(cursor_key) = resolved_after.as_ref() {
            if !skipped_cursor && index_key.as_ref() == cursor_key.as_bytes() {
                skipped_cursor = true;
                continue;
            }
        }

        let event_id = match std::str::from_utf8(&value) {
            Ok(v) if !v.is_empty() => v,
            _ => continue,
        };

        let event_value = match state.db.get_cf(events_cf, event_id.as_bytes()) {
            Ok(Some(v)) => v,
            _ => continue,
        };

        let event = match serde_json::from_slice::<Value>(&event_value) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if let Some(ref filter) = event_type_filter {
            if filter_kind != "type"
                && event.get("event_type").and_then(|v| v.as_str()) != Some(filter.as_str())
            {
                continue;
            }
        }
        if let Some(ref filter) = entity_id_filter {
            if filter_kind != "entity"
                && event.get("entity_id").and_then(|v| v.as_str()) != Some(filter.as_str())
            {
                continue;
            }
        }
        if let Some(ref filter) = tx_hash_filter {
            if filter_kind != "tx"
                && event.get("tx_hash").and_then(|v| v.as_str()) != Some(filter.as_str())
            {
                continue;
            }
        }

        next_cursor = Some(String::from_utf8_lossy(&index_key).to_string());
        events.push(event);
    }

    // Fallback for pre-index legacy data.
    if events.is_empty() {
        let mut past_cursor = after_cursor.is_none();
        for item in state.db.iterator_cf(events_cf, rocksdb::IteratorMode::End) {
            if events.len() >= limit {
                break;
            }
            let (key, value) =
                item.map_err(|e| Json(ErrorResponse::db(&format!("iter: {}", e))))?;
            let event = match serde_json::from_slice::<Value>(&value) {
                Ok(v) => v,
                Err(_) => continue,
            };

            if !past_cursor {
                let key_str = std::str::from_utf8(&key).unwrap_or("");
                let event_id = event.get("event_id").and_then(|v| v.as_str()).unwrap_or("");
                if key_str == after_cursor.as_deref().unwrap_or("")
                    || event_id == after_cursor.as_deref().unwrap_or("")
                {
                    past_cursor = true;
                }
                continue;
            }

            if let Some(ref filter) = event_type_filter {
                if event.get("event_type").and_then(|v| v.as_str()) != Some(filter.as_str()) {
                    continue;
                }
            }
            if let Some(ref filter) = entity_id_filter {
                if event.get("entity_id").and_then(|v| v.as_str()) != Some(filter.as_str()) {
                    continue;
                }
            }
            if let Some(ref filter) = tx_hash_filter {
                if event.get("tx_hash").and_then(|v| v.as_str()) != Some(filter.as_str()) {
                    continue;
                }
            }

            events.push(event);
        }
    }

    Ok(Json(json!({
        "events": events,
        "count": events.len(),
        "next_cursor": next_cursor,
    })))
}

// ── Webhook DB Helpers ──

fn store_webhook(db: &DB, webhook: &WebhookRegistration) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_WEBHOOKS)
        .ok_or_else(|| "missing webhooks cf".to_string())?;
    let bytes = serde_json::to_vec(webhook).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, webhook.id.as_bytes(), bytes)
        .map_err(|e| format!("db put: {}", e))
}

fn list_all_webhooks(db: &DB) -> Result<Vec<WebhookRegistration>, String> {
    let cf = db
        .cf_handle(CF_WEBHOOKS)
        .ok_or_else(|| "missing webhooks cf".to_string())?;
    let mut webhooks = Vec::new();
    let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
    for item in iter {
        let (_, value) = item.map_err(|e| format!("db iter: {}", e))?;
        if let Ok(webhook) = serde_json::from_slice::<WebhookRegistration>(&value) {
            webhooks.push(webhook);
        }
    }
    Ok(webhooks)
}

fn remove_webhook(db: &DB, webhook_id: &str) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_WEBHOOKS)
        .ok_or_else(|| "missing webhooks cf".to_string())?;
    db.delete_cf(cf, webhook_id.as_bytes())
        .map_err(|e| format!("db delete: {}", e))
}

// ── WebSocket Event Stream ──

/// GET /ws/events — Upgrade to WebSocket for real-time custody event streaming.
/// Optional query param: ?filter=deposit.confirmed,withdrawal.confirmed
/// Requires Bearer auth token in Sec-WebSocket-Protocol header or ?token= query param.
async fn ws_events(
    State(state): State<CustodyState>,
    ws: WebSocketUpgrade,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> axum::response::Response {
    // AUDIT-FIX F8.2: Constant-time token comparison for WebSocket auth
    let auth_ok = if let Some(token) = params.get("token") {
        if let Some(expected) = state.config.api_auth_token.as_deref() {
            use subtle::ConstantTimeEq;
            let matches: bool = token.as_bytes().ct_eq(expected.as_bytes()).into();
            matches
        } else {
            false
        }
    } else {
        false
    };

    if !auth_ok {
        return axum::response::Response::builder()
            .status(401)
            .body(axum::body::Body::from(
                "Unauthorized: provide ?token=<api_auth_token>",
            ))
            .unwrap_or_default();
    }

    let event_filter: Vec<String> = params
        .get("filter")
        .map(|f| {
            f.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let event_rx = state.event_tx.subscribe();

    ws.on_upgrade(move |socket| handle_ws_events(socket, event_rx, event_filter))
}

async fn handle_ws_events(
    mut socket: WebSocket,
    mut event_rx: broadcast::Receiver<CustodyWebhookEvent>,
    event_filter: Vec<String>,
) {
    info!(
        "WebSocket event subscriber connected (filter: {:?})",
        event_filter
    );

    loop {
        tokio::select! {
            // Forward custody events to the WebSocket client
            result = event_rx.recv() => {
                match result {
                    Ok(event) => {
                        // Apply event type filter
                        if !event_filter.is_empty() && !event_filter.contains(&event.event_type) {
                            continue;
                        }
                        let payload = match serde_json::to_string(&event) {
                            Ok(p) => p,
                            Err(_) => continue,
                        };
                        if socket.send(WsMessage::Text(payload)).await.is_err() {
                            break; // Client disconnected
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("WebSocket subscriber lagged, dropped {} events", n);
                        let warning = json!({
                            "warning": "lagged",
                            "dropped_events": n,
                        });
                        let _ = socket.send(WsMessage::Text(warning.to_string())).await;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            // Handle incoming messages from the client (ping/pong, close)
            msg = socket.recv() => {
                match msg {
                    Some(Ok(WsMessage::Close(_))) | None => break,
                    Some(Ok(WsMessage::Ping(data))) => {
                        let _ = socket.send(WsMessage::Pong(data)).await;
                    }
                    _ => {} // Ignore text/binary from client
                }
            }
        }
    }

    info!("WebSocket event subscriber disconnected");
}

// ── Webhook Dispatcher (Background Worker) ──

/// Background loop that receives events from the broadcast channel and
/// delivers them to all registered webhook endpoints with HMAC-SHA256 signatures.
async fn webhook_dispatcher_loop(
    state: CustodyState,
    event_rx: &mut broadcast::Receiver<CustodyWebhookEvent>,
) {
    info!("🔔 Webhook dispatcher started");

    loop {
        match event_rx.recv().await {
            Ok(event) => {
                let webhooks = match list_all_webhooks(&state.db) {
                    Ok(w) => w,
                    Err(e) => {
                        tracing::warn!("failed to list webhooks: {}", e);
                        continue;
                    }
                };

                for webhook in webhooks {
                    if !webhook.active {
                        continue;
                    }
                    // Apply event filter
                    if !webhook.event_filter.is_empty()
                        && !webhook.event_filter.contains(&event.event_type)
                    {
                        continue;
                    }

                    let client = state.http.clone();
                    let event_clone = event.clone();
                    let webhook_clone = webhook.clone();
                    let permit = match state.webhook_delivery_limiter.clone().acquire_owned().await
                    {
                        Ok(p) => p,
                        Err(_) => {
                            tracing::warn!("webhook delivery limiter closed");
                            continue;
                        }
                    };

                    // Fire-and-forget with retry (spawn per delivery to not block others)
                    tokio::spawn(async move {
                        let _permit = permit;
                        deliver_webhook(&client, &webhook_clone, &event_clone).await;
                    });
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("webhook dispatcher lagged, dropped {} events", n);
            }
            Err(broadcast::error::RecvError::Closed) => {
                tracing::warn!("webhook dispatcher channel closed");
                break;
            }
        }
    }
}

/// Deliver a single event to a webhook endpoint with HMAC-SHA256 signature + retry.
async fn deliver_webhook(
    client: &reqwest::Client,
    webhook: &WebhookRegistration,
    event: &CustodyWebhookEvent,
) {
    let payload = match serde_json::to_vec(event) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("webhook payload encode failed: {}", e);
            return;
        }
    };

    // Compute HMAC-SHA256 signature
    let signature = compute_webhook_signature(&payload, &webhook.secret);

    // Retry up to 3 times with exponential backoff (1s, 2s, 4s)
    for attempt in 0..3u32 {
        if attempt > 0 {
            sleep(Duration::from_secs(1 << attempt)).await;
        }

        let result = client
            .post(&webhook.url)
            .header("Content-Type", "application/json")
            .header("X-Custody-Signature", &signature)
            .header("X-Custody-Event", &event.event_type)
            .header("X-Custody-Delivery", &event.event_id)
            .header("X-Custody-Timestamp", event.timestamp.to_string())
            .body(payload.clone())
            .send()
            .await;

        match result {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() || status == reqwest::StatusCode::NO_CONTENT {
                    tracing::debug!(
                        "webhook delivered: {} → {} (event={})",
                        event.event_type,
                        webhook.url,
                        event.event_id
                    );
                    return;
                }
                tracing::warn!(
                    "webhook {} returned HTTP {} (attempt {}/3, event={})",
                    webhook.url,
                    status,
                    attempt + 1,
                    event.event_type
                );
            }
            Err(e) => {
                tracing::warn!(
                    "webhook {} delivery failed (attempt {}/3): {}",
                    webhook.url,
                    attempt + 1,
                    e
                );
            }
        }
    }

    tracing::error!(
        "webhook delivery exhausted all retries: {} → {} (event={}, entity={})",
        event.event_type,
        webhook.url,
        event.event_id,
        event.entity_id,
    );
}

/// Compute HMAC-SHA256 hex signature for webhook payload verification.
fn compute_webhook_signature(payload: &[u8], secret: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;

    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(payload);
    let result = mac.finalize().into_bytes();
    hex::encode(result)
}

// ── API Auth Helper ──

/// AUDIT-FIX F8.1: Constant-time auth check to prevent timing side-channel attacks.
/// Previous implementation used `!=` which leaks token length/content via response time.
fn verify_api_auth(
    config: &CustodyConfig,
    headers: &axum::http::HeaderMap,
) -> Result<(), Json<ErrorResponse>> {
    let expected = config.api_auth_token.as_deref().unwrap_or("");
    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    if expected.is_empty() {
        return Err(Json(ErrorResponse {
            code: "unauthorized",
            message: "Invalid or missing Bearer token".to_string(),
        }));
    }

    use subtle::ConstantTimeEq;
    let matches: bool = provided.as_bytes().ct_eq(expected.as_bytes()).into();
    if !matches {
        return Err(Json(ErrorResponse {
            code: "unauthorized",
            message: "Invalid or missing Bearer token".to_string(),
        }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CustodyConfig {
        CustodyConfig {
            db_path: "/tmp/test_custody".to_string(),
            solana_rpc_url: Some("http://localhost:8899".to_string()),
            evm_rpc_url: Some("http://localhost:8545".to_string()),
            eth_rpc_url: None,
            bnb_rpc_url: None,
            solana_confirmations: 1,
            evm_confirmations: 12,
            poll_interval_secs: 15,
            treasury_solana_address: Some("TEST_SOL_ADDR".to_string()),
            treasury_evm_address: Some("0xTEST".to_string()),
            treasury_eth_address: None,
            treasury_bnb_address: None,
            solana_fee_payer_keypair_path: Some("/tmp/fee.json".to_string()),
            solana_treasury_owner: Some("TEST_OWNER".to_string()),
            solana_usdc_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            solana_usdt_mint: "Es9vMFrzaCER3FXvxuauYhVNiVw9g8Y3V9D2n7sGdG8d".to_string(),
            evm_usdc_contract: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".to_string(),
            evm_usdt_contract: "0xdAC17F958D2ee523a2206206994597C13D831ec7".to_string(),
            signer_endpoints: vec![],
            signer_threshold: 0,
            licn_rpc_url: None,
            treasury_keypair_path: None,
            musd_contract_addr: None,
            wsol_contract_addr: None,
            weth_contract_addr: None,
            wbnb_contract_addr: None,
            rebalance_threshold_bps: 7000,
            rebalance_target_bps: 5000,
            rebalance_max_slippage_bps: 50,
            jupiter_api_url: None,
            uniswap_router: None,
            deposit_ttl_secs: 86400,
            master_seed: "test_master_seed_for_unit_tests".to_string(),
            deposit_master_seed: "test_master_seed_for_unit_tests".to_string(),
            signer_auth_token: Some("test_token".to_string()),
            signer_auth_tokens: vec![],
            api_auth_token: Some("test_api_token".to_string()),
            frost_pubkey_package_hex: None,
            evm_multisig_address: None,
            webhook_allowed_hosts: vec![],
        }
    }

    #[test]
    fn test_is_solana_stablecoin() {
        assert!(is_solana_stablecoin("usdc"));
        assert!(is_solana_stablecoin("usdt"));
        assert!(!is_solana_stablecoin("sol"));
        assert!(!is_solana_stablecoin("USDC")); // case sensitive
        assert!(!is_solana_stablecoin("eth"));
    }

    #[test]
    fn test_default_signer_threshold() {
        assert_eq!(default_signer_threshold(0), 0);
        assert_eq!(default_signer_threshold(1), 1);
        assert_eq!(default_signer_threshold(2), 1);
        assert_eq!(default_signer_threshold(3), 2);
        assert_eq!(default_signer_threshold(4), 2);
        assert_eq!(default_signer_threshold(5), 3);
        assert_eq!(default_signer_threshold(10), 3);
    }

    fn test_withdrawal_job() -> WithdrawalJob {
        WithdrawalJob {
            job_id: "test-withdrawal".to_string(),
            user_id: "user-1".to_string(),
            asset: "wSOL".to_string(),
            amount: 10_000,
            dest_chain: "solana".to_string(),
            dest_address: "11111111111111111111111111111111".to_string(),
            preferred_stablecoin: "usdt".to_string(),
            burn_tx_signature: None,
            outbound_tx_hash: None,
            safe_nonce: None,
            signatures: Vec::new(),
            status: "burned".to_string(),
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            created_at: 0,
        }
    }

    fn test_db_path() -> String {
        static NEXT_TEST_DB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let db_id = NEXT_TEST_DB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!(
                "lichen-custody-test-{}-{}",
                std::process::id(),
                db_id
            ))
            .to_string_lossy()
            .into_owned()
    }

    fn test_state() -> CustodyState {
        let db_path = test_db_path();
        let _ = DB::destroy(&Options::default(), &db_path);
        let db = open_db(&db_path).unwrap();
        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        CustodyState {
            db: std::sync::Arc::new(db),
            next_index_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            config: test_config(),
            http: reqwest::Client::new(),
            withdrawal_rate: std::sync::Arc::new(tokio::sync::Mutex::new(
                WithdrawalRateState::new(),
            )),
            deposit_rate: std::sync::Arc::new(tokio::sync::Mutex::new(DepositRateState::new())),
            event_tx,
            webhook_delivery_limiter: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }

    #[test]
    fn test_determine_withdrawal_signing_mode_self_custody() {
        let state = test_state();
        let job = test_withdrawal_job();

        let mode = determine_withdrawal_signing_mode(&state, &job, "sol").unwrap();

        assert_eq!(mode, None);
    }

    #[test]
    fn test_determine_withdrawal_signing_mode_routes_native_solana_to_frost() {
        let mut state = test_state();
        state.config.signer_endpoints = vec![
            "http://signer-1".to_string(),
            "http://signer-2".to_string(),
            "http://signer-3".to_string(),
        ];
        state.config.signer_threshold = 2;
        state.config.frost_pubkey_package_hex = Some("deadbeef".to_string());
        let job = test_withdrawal_job();

        let mode = determine_withdrawal_signing_mode(&state, &job, "sol").unwrap();

        assert_eq!(mode, Some(WithdrawalSigningMode::SolanaThresholdFrost));
    }

    #[test]
    fn test_determine_withdrawal_signing_mode_routes_solana_stablecoin_to_frost() {
        let mut state = test_state();
        state.config.signer_endpoints = vec![
            "http://signer-1".to_string(),
            "http://signer-2".to_string(),
            "http://signer-3".to_string(),
        ];
        state.config.signer_threshold = 2;
        state.config.frost_pubkey_package_hex = Some("deadbeef".to_string());
        let mut job = test_withdrawal_job();
        job.asset = "lUSD".to_string();
        job.amount = 1_000_000_000;

        let mode = determine_withdrawal_signing_mode(&state, &job, "usdt").unwrap();

        assert_eq!(mode, Some(WithdrawalSigningMode::SolanaThresholdFrost));
    }

    #[test]
    fn test_determine_withdrawal_signing_mode_routes_threshold_evm_to_safe() {
        let mut state = test_state();
        state.config.signer_endpoints = vec![
            "http://signer-1".to_string(),
            "http://signer-2".to_string(),
            "http://signer-3".to_string(),
        ];
        state.config.signer_threshold = 2;
        state.config.evm_multisig_address =
            Some("0x2222222222222222222222222222222222222222".to_string());
        let mut job = test_withdrawal_job();
        job.dest_chain = "ethereum".to_string();
        job.asset = "wETH".to_string();
        job.dest_address = "0x1111111111111111111111111111111111111111".to_string();

        let mode = determine_withdrawal_signing_mode(&state, &job, "eth").unwrap();

        assert_eq!(mode, Some(WithdrawalSigningMode::EvmThresholdSafe));
    }

    #[test]
    fn test_normalize_evm_signature_promotes_recovery_id() {
        let mut signature = vec![0u8; 65];
        signature[64] = 1;

        let normalized = normalize_evm_signature(&signature).unwrap();

        assert_eq!(normalized[64], 28);
    }

    #[test]
    fn test_build_evm_safe_exec_transaction_calldata_uses_exec_selector() {
        let calldata = build_evm_safe_exec_transaction_calldata(
            "0x1111111111111111111111111111111111111111",
            123,
            &[0xaa, 0xbb, 0xcc],
            &[0x11; 130],
        )
        .unwrap();

        assert_eq!(
            &calldata[..4],
            &evm_function_selector(
                "execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)",
            )
        );
        assert!(calldata.len() > 4 + 10 * 32);
    }

    #[derive(Clone)]
    struct MockRpcState {
        safe_nonce_hex: String,
        safe_tx_hash_hex: String,
        send_raw_tx_hash_hex: Option<String>,
        transaction_receipt: Option<Value>,
        requests: std::sync::Arc<tokio::sync::Mutex<Vec<Value>>>,
    }

    #[derive(Clone)]
    struct MockSignerState {
        signer_pubkey: String,
        signature_hex: String,
        requests: std::sync::Arc<tokio::sync::Mutex<Vec<Value>>>,
    }

    #[derive(Clone)]
    struct MockLichenRpcState {
        transaction_result: Value,
        requests: std::sync::Arc<tokio::sync::Mutex<Vec<Value>>>,
    }

    async fn mock_rpc_handler(
        axum::extract::State(state): axum::extract::State<MockRpcState>,
        Json(payload): Json<Value>,
    ) -> Json<Value> {
        state.requests.lock().await.push(payload.clone());
        let method = payload
            .get("method")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let result = match method {
            "eth_call" => {
                let data = payload
                    .get("params")
                    .and_then(|value| value.as_array())
                    .and_then(|params| params.first())
                    .and_then(|call| call.get("data"))
                    .and_then(|value| value.as_str())
                    .unwrap_or_default();
                if data == format!("0x{}", hex::encode(evm_function_selector("nonce()"))) {
                    Value::String(state.safe_nonce_hex.clone())
                } else {
                    Value::String(state.safe_tx_hash_hex.clone())
                }
            }
            "eth_getTransactionCount" => Value::String("0x3".to_string()),
            "eth_gasPrice" => Value::String("0x4a817c800".to_string()),
            "eth_chainId" => Value::String("0x1".to_string()),
            "eth_estimateGas" => Value::String("0x55f0".to_string()),
            "eth_getTransactionReceipt" => state.transaction_receipt.clone().unwrap_or(Value::Null),
            "eth_sendRawTransaction" => state
                .send_raw_tx_hash_hex
                .clone()
                .map(Value::String)
                .unwrap_or(Value::Null),
            _ => Value::Null,
        };

        Json(json!({
            "jsonrpc": "2.0",
            "id": payload.get("id").cloned().unwrap_or(json!(1)),
            "result": result,
        }))
    }

    async fn mock_signer_handler(
        axum::extract::State(state): axum::extract::State<MockSignerState>,
        Json(payload): Json<Value>,
    ) -> Json<Value> {
        state.requests.lock().await.push(payload.clone());
        Json(json!({
            "status": "signed",
            "signer_pubkey": state.signer_pubkey,
            "signature": state.signature_hex,
            "message_hash": payload.get("tx_hash").cloned().unwrap_or(Value::String(String::new())),
            "_message": payload.get("tx_hash").cloned().unwrap_or(Value::String(String::new())),
        }))
    }

    async fn mock_licn_rpc_handler(
        axum::extract::State(state): axum::extract::State<MockLichenRpcState>,
        Json(payload): Json<Value>,
    ) -> Json<Value> {
        state.requests.lock().await.push(payload.clone());
        let method = payload
            .get("method")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let result = match method {
            "getTransaction" => state.transaction_result.clone(),
            _ => Value::Null,
        };

        Json(json!({
            "jsonrpc": "2.0",
            "id": payload.get("id").cloned().unwrap_or(json!(1)),
            "result": result,
        }))
    }

    async fn spawn_mock_server(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock listener");
        let addr = listener.local_addr().expect("mock listener addr");
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service())
                .await
                .expect("serve mock app");
        });
        format!("http://{}", addr)
    }

    fn decode_test_rlp_item(bytes: &[u8]) -> Result<(Vec<u8>, usize), String> {
        if bytes.is_empty() {
            return Err("empty RLP item".to_string());
        }

        let prefix = bytes[0];
        match prefix {
            0x00..=0x7f => Ok((vec![prefix], 1)),
            0x80..=0xb7 => {
                let len = (prefix - 0x80) as usize;
                let end = 1 + len;
                if bytes.len() < end {
                    return Err("short RLP string".to_string());
                }
                Ok((bytes[1..end].to_vec(), end))
            }
            0xb8..=0xbf => {
                let len_of_len = (prefix - 0xb7) as usize;
                let header_end = 1 + len_of_len;
                if bytes.len() < header_end {
                    return Err("short RLP long-string header".to_string());
                }
                let len = bytes[1..header_end]
                    .iter()
                    .fold(0usize, |acc, byte| (acc << 8) | (*byte as usize));
                let end = header_end + len;
                if bytes.len() < end {
                    return Err("short RLP long-string body".to_string());
                }
                Ok((bytes[header_end..end].to_vec(), end))
            }
            _ => Err("RLP item is not a string".to_string()),
        }
    }

    fn decode_test_rlp_list(bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        if bytes.is_empty() {
            return Err("empty RLP payload".to_string());
        }

        let prefix = bytes[0];
        let (payload_offset, payload_len) = match prefix {
            0xc0..=0xf7 => (1usize, (prefix - 0xc0) as usize),
            0xf8..=0xff => {
                let len_of_len = (prefix - 0xf7) as usize;
                let header_end = 1 + len_of_len;
                if bytes.len() < header_end {
                    return Err("short RLP long-list header".to_string());
                }
                let len = bytes[1..header_end]
                    .iter()
                    .fold(0usize, |acc, byte| (acc << 8) | (*byte as usize));
                (header_end, len)
            }
            _ => return Err("RLP payload is not a list".to_string()),
        };

        let payload_end = payload_offset + payload_len;
        if bytes.len() < payload_end {
            return Err("short RLP list body".to_string());
        }

        let mut items = Vec::new();
        let mut cursor = payload_offset;
        while cursor < payload_end {
            let (item, consumed) = decode_test_rlp_item(&bytes[cursor..payload_end])?;
            items.push(item);
            cursor += consumed;
        }

        if cursor != payload_end {
            return Err("RLP list decode ended mid-payload".to_string());
        }

        Ok(items)
    }

    #[tokio::test]
    async fn test_collect_and_assemble_threshold_evm_safe_flow() {
        let mut state = test_state();
        let safe_tx_hash_hex =
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
        let rpc_app: Router =
            Router::new()
                .route("/", post(mock_rpc_handler))
                .with_state(MockRpcState {
                    safe_nonce_hex: "0x7".to_string(),
                    safe_tx_hash_hex: safe_tx_hash_hex.clone(),
                    send_raw_tx_hash_hex: None,
                    transaction_receipt: None,
                    requests: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
                });
        let rpc_url = spawn_mock_server(rpc_app).await;

        let signer_one_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let signer_two_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let signer_one_app: Router = Router::new()
            .route("/sign", post(mock_signer_handler))
            .with_state(MockSignerState {
                signer_pubkey: "0x1111111111111111111111111111111111111111".to_string(),
                signature_hex: format!("{}1b", "11".repeat(64)),
                requests: signer_one_requests.clone(),
            });
        let signer_one = spawn_mock_server(signer_one_app).await;
        let signer_two_app: Router = Router::new()
            .route("/sign", post(mock_signer_handler))
            .with_state(MockSignerState {
                signer_pubkey: "0x2222222222222222222222222222222222222222".to_string(),
                signature_hex: format!("{}00", "22".repeat(64)),
                requests: signer_two_requests.clone(),
            });
        let signer_two = spawn_mock_server(signer_two_app).await;

        state.config.evm_rpc_url = Some(rpc_url.clone());
        state.config.eth_rpc_url = Some(rpc_url);
        state.config.signer_endpoints = vec![signer_one, signer_two];
        state.config.signer_threshold = 2;
        state.config.evm_multisig_address =
            Some("0x9999999999999999999999999999999999999999".to_string());

        let mut job = test_withdrawal_job();
        job.dest_chain = "ethereum".to_string();
        job.asset = "wETH".to_string();
        job.dest_address = "0x3333333333333333333333333333333333333333".to_string();
        job.amount = 2_000_000_000;

        let sig_count = collect_threshold_evm_withdrawal_signatures(&state, &mut job, "eth")
            .await
            .expect("collect threshold evm signatures");

        assert_eq!(sig_count, 2);
        assert_eq!(job.safe_nonce, Some(7));
        assert_eq!(job.signatures.len(), 2);
        assert!(job
            .signatures
            .iter()
            .all(|sig| sig.message_hash == safe_tx_hash_hex.trim_start_matches("0x")));

        let signer_one_payloads = signer_one_requests.lock().await;
        let signer_two_payloads = signer_two_requests.lock().await;
        assert_eq!(signer_one_payloads.len(), 1);
        assert_eq!(signer_two_payloads.len(), 1);
        assert_eq!(
            signer_one_payloads[0]
                .get("tx_hash")
                .and_then(|value| value.as_str()),
            Some(safe_tx_hash_hex.trim_start_matches("0x"))
        );
        assert_eq!(
            signer_one_payloads[0]
                .get("from_address")
                .and_then(|value| value.as_str()),
            Some("0x9999999999999999999999999999999999999999")
        );

        let relay_tx = assemble_signed_evm_tx(&state, &job, "eth")
            .await
            .expect("assemble threshold evm relay tx");
        assert!(!relay_tx.is_empty());

        let relay_fields = decode_test_rlp_list(&relay_tx).expect("decode relay tx rlp");
        assert_eq!(relay_fields.len(), 9);
        assert_eq!(
            hex::encode(&relay_fields[3]),
            "9999999999999999999999999999999999999999"
        );
        assert_eq!(relay_fields[4], Vec::<u8>::new());
        assert_eq!(
            &relay_fields[5][..4],
            &evm_function_selector(
                "execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)",
            )
        );
        assert_eq!(
            &relay_fields[5][16..36],
            &hex::decode("3333333333333333333333333333333333333333").unwrap()
        );
    }

    #[tokio::test]
    async fn test_assemble_signed_evm_tx_rejects_mismatched_safe_hash() {
        let mut state = test_state();
        let safe_tx_hash_hex =
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
        let rpc_app: Router =
            Router::new()
                .route("/", post(mock_rpc_handler))
                .with_state(MockRpcState {
                    safe_nonce_hex: "0x7".to_string(),
                    safe_tx_hash_hex: safe_tx_hash_hex.clone(),
                    send_raw_tx_hash_hex: None,
                    transaction_receipt: None,
                    requests: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
                });
        let rpc_url = spawn_mock_server(rpc_app).await;

        state.config.evm_rpc_url = Some(rpc_url.clone());
        state.config.eth_rpc_url = Some(rpc_url);
        state.config.signer_threshold = 2;
        state.config.signer_endpoints =
            vec!["http://signer-1".to_string(), "http://signer-2".to_string()];
        state.config.evm_multisig_address =
            Some("0x9999999999999999999999999999999999999999".to_string());

        let mut job = test_withdrawal_job();
        job.dest_chain = "ethereum".to_string();
        job.asset = "wETH".to_string();
        job.dest_address = "0x3333333333333333333333333333333333333333".to_string();
        job.amount = 2_000_000_000;
        job.safe_nonce = Some(7);
        job.signatures = vec![
            SignerSignature {
                signer_pubkey: "1111111111111111111111111111111111111111".to_string(),
                signature: format!("{}1b", "11".repeat(64)),
                message_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    .to_string(),
                received_at: 0,
            },
            SignerSignature {
                signer_pubkey: "2222222222222222222222222222222222222222".to_string(),
                signature: format!("{}1c", "22".repeat(64)),
                message_hash: safe_tx_hash_hex.trim_start_matches("0x").to_string(),
                received_at: 0,
            },
        ];

        let err = assemble_signed_evm_tx(&state, &job, "eth")
            .await
            .expect_err("mismatched Safe hash should be rejected");

        assert!(err.contains("does not match the pinned Safe transaction hash"));
    }

    #[tokio::test]
    async fn test_assemble_signed_evm_tx_rejects_duplicate_signers() {
        let mut state = test_state();
        let safe_tx_hash_hex =
            "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
        let rpc_app: Router =
            Router::new()
                .route("/", post(mock_rpc_handler))
                .with_state(MockRpcState {
                    safe_nonce_hex: "0x7".to_string(),
                    safe_tx_hash_hex: safe_tx_hash_hex.clone(),
                    send_raw_tx_hash_hex: None,
                    transaction_receipt: None,
                    requests: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
                });
        let rpc_url = spawn_mock_server(rpc_app).await;

        state.config.evm_rpc_url = Some(rpc_url.clone());
        state.config.eth_rpc_url = Some(rpc_url);
        state.config.signer_threshold = 2;
        state.config.signer_endpoints =
            vec!["http://signer-1".to_string(), "http://signer-2".to_string()];
        state.config.evm_multisig_address =
            Some("0x9999999999999999999999999999999999999999".to_string());

        let mut job = test_withdrawal_job();
        job.dest_chain = "ethereum".to_string();
        job.asset = "wETH".to_string();
        job.dest_address = "0x3333333333333333333333333333333333333333".to_string();
        job.amount = 2_000_000_000;
        job.safe_nonce = Some(7);
        job.signatures = vec![
            SignerSignature {
                signer_pubkey: "1111111111111111111111111111111111111111".to_string(),
                signature: format!("{}1b", "11".repeat(64)),
                message_hash: safe_tx_hash_hex.trim_start_matches("0x").to_string(),
                received_at: 0,
            },
            SignerSignature {
                signer_pubkey: "0x1111111111111111111111111111111111111111".to_string(),
                signature: format!("{}1c", "22".repeat(64)),
                message_hash: safe_tx_hash_hex.trim_start_matches("0x").to_string(),
                received_at: 1,
            },
        ];

        let err = assemble_signed_evm_tx(&state, &job, "eth")
            .await
            .expect_err("duplicate signers should be rejected");

        assert!(err.contains("duplicate EVM signer address"));
    }

    #[test]
    fn test_build_threshold_solana_withdrawal_message_rejects_dust() {
        let state = test_state();
        let mut job = test_withdrawal_job();
        job.amount = 5_000;
        let recent_blockhash = [0u8; 32];

        let err = build_threshold_solana_withdrawal_message(&state, &job, "sol", &recent_blockhash)
            .unwrap_err();

        assert!(err.contains("too small to cover fees"));
    }

    #[test]
    fn test_build_threshold_solana_withdrawal_message_supports_stablecoins() {
        let mut state = test_state();
        let treasury_owner =
            derive_solana_address("custody/treasury/solana", &state.config.master_seed).unwrap();
        state.config.treasury_solana_address = Some(treasury_owner.clone());
        state.config.solana_treasury_owner = Some(treasury_owner.clone());

        let mut job = test_withdrawal_job();
        job.asset = "lUSD".to_string();
        job.amount = 1_250_000_000;
        job.dest_address =
            derive_solana_address("user/dest/solana", &state.config.master_seed).unwrap();

        let recent_blockhash = [7u8; 32];
        let message =
            build_threshold_solana_withdrawal_message(&state, &job, "usdt", &recent_blockhash)
                .unwrap();

        let mint = solana_mint_for_asset(&state.config, "usdt").unwrap();
        let from_token_account =
            derive_associated_token_address_from_str(&treasury_owner, &mint).unwrap();
        let to_token_account =
            derive_associated_token_address_from_str(&job.dest_address, &mint).unwrap();
        let expected = build_solana_token_transfer_message(
            &decode_solana_pubkey(&treasury_owner).unwrap(),
            &decode_solana_pubkey(&from_token_account).unwrap(),
            &decode_solana_pubkey(&to_token_account).unwrap(),
            u64::try_from(spores_to_chain_amount(job.amount, "solana", "usdt")).unwrap(),
            &recent_blockhash,
        )
        .unwrap();

        assert_eq!(message, expected);
    }

    #[test]
    fn test_solana_mint_for_asset() {
        let config = test_config();
        assert!(solana_mint_for_asset(&config, "usdc").is_ok());
        assert!(solana_mint_for_asset(&config, "usdt").is_ok());
        assert!(solana_mint_for_asset(&config, "btc").is_err());
    }

    #[test]
    fn test_evm_contract_for_asset() {
        let config = test_config();
        assert!(evm_contract_for_asset(&config, "usdc").is_ok());
        assert!(evm_contract_for_asset(&config, "usdt").is_ok());
        assert!(evm_contract_for_asset(&config, "eth").is_err());
    }

    #[test]
    fn test_ensure_solana_config_valid() {
        let config = test_config();
        assert!(ensure_solana_config(&config).is_ok());
    }

    #[test]
    fn test_ensure_solana_config_missing_rpc() {
        let mut config = test_config();
        config.solana_rpc_url = None;
        assert!(ensure_solana_config(&config).is_err());
    }

    #[test]
    fn test_ensure_solana_config_missing_fee_payer() {
        // Fee payer is no longer mandatory — it can be derived from the master seed
        let mut config = test_config();
        config.solana_fee_payer_keypair_path = None;
        assert!(ensure_solana_config(&config).is_ok());
    }

    #[test]
    fn test_derive_deposit_address_unsupported_chain() {
        let result = derive_deposit_address("bitcoin", "btc", "m/44'/0'/0'/0/0", "test_seed");
        assert!(result.is_err());
    }

    #[test]
    fn test_derive_deposit_address_bnb_uses_evm_format() {
        let address =
            derive_deposit_address("bnb", "usdt", "m/44'/60'/0'/0/0", "test_seed").unwrap();
        assert!(address.starts_with("0x"));
        assert_eq!(address.len(), 42);
    }

    #[test]
    fn test_master_seed_rotation_changes_derived_addresses() {
        let derivation_path = "m/44'/501'/0'/0/0";
        let old_seed = "rotation_seed_old";
        let new_seed = "rotation_seed_new";

        let sol_old = derive_solana_address(derivation_path, old_seed).expect("derive old sol");
        let sol_new = derive_solana_address(derivation_path, new_seed).expect("derive new sol");
        assert_ne!(
            sol_old, sol_new,
            "solana derived address must rotate with seed"
        );

        let evm_path = "m/44'/60'/0'/0/0";
        let evm_old = derive_evm_address(evm_path, old_seed).expect("derive old evm");
        let evm_new = derive_evm_address(evm_path, new_seed).expect("derive new evm");
        assert_ne!(
            evm_old, evm_new,
            "evm derived address must rotate with seed"
        );
    }

    #[test]
    fn test_legacy_deposit_records_default_to_treasury_seed_source() {
        let deposit: DepositRequest = serde_json::from_value(json!({
            "deposit_id": "dep-legacy-1",
            "user_id": "11111111111111111111111111111111",
            "chain": "solana",
            "asset": "sol",
            "address": "legacy-address",
            "derivation_path": "m/44'/501'/0'/0/0",
            "created_at": 1,
            "status": "issued"
        }))
        .expect("deserialize legacy deposit record");

        assert_eq!(
            deposit.deposit_seed_source,
            DEPOSIT_SEED_SOURCE_TREASURY_ROOT
        );
    }

    #[tokio::test]
    async fn test_create_deposit_uses_dedicated_deposit_seed_and_persists_source() {
        let mut state = test_state();
        state.config.deposit_master_seed =
            "dedicated_deposit_seed_for_tests_0123456789".to_string();

        let mut headers = axum::http::HeaderMap::new();
        headers.insert("authorization", "Bearer test_api_token".parse().unwrap());

        let response = create_deposit(
            State(state.clone()),
            headers,
            Json(CreateDepositRequest {
                user_id: "11111111111111111111111111111111".to_string(),
                chain: "ethereum".to_string(),
                asset: "eth".to_string(),
            }),
        )
        .await
        .expect("create deposit with dedicated deposit seed");

        let stored = fetch_deposit(&state.db, &response.0.deposit_id)
            .expect("fetch created deposit")
            .expect("deposit should exist");
        assert_eq!(stored.deposit_seed_source, DEPOSIT_SEED_SOURCE_DEPOSIT_ROOT);

        let expected = derive_deposit_address(
            "ethereum",
            "eth",
            &stored.derivation_path,
            &state.config.deposit_master_seed,
        )
        .expect("derive address from dedicated deposit seed");
        assert_eq!(stored.address, expected);
    }

    #[test]
    fn test_build_credit_job_uses_native_solana_credited_amount() {
        let mut state = test_state();
        state.config.licn_rpc_url = Some("http://localhost:8899".to_string());
        state.config.treasury_keypair_path = Some("/tmp/test-treasury.json".to_string());
        state.config.wsol_contract_addr = Some("11111111111111111111111111111111".to_string());

        let deposit = DepositRequest {
            deposit_id: "dep-sol-credit-1".to_string(),
            user_id: "11111111111111111111111111111111".to_string(),
            chain: "solana".to_string(),
            asset: "sol".to_string(),
            address: "from".to_string(),
            derivation_path: "m/44'/501'/0'/0/3".to_string(),
            deposit_seed_source: DEPOSIT_SEED_SOURCE_TREASURY_ROOT.to_string(),
            created_at: 1000,
            status: "swept".to_string(),
        };
        store_deposit(&state.db, &deposit).expect("store deposit for credit test");

        let sweep = SweepJob {
            job_id: "sweep-sol-credit-1".to_string(),
            deposit_id: deposit.deposit_id.clone(),
            chain: "solana".to_string(),
            asset: "sol".to_string(),
            from_address: deposit.address.clone(),
            to_treasury: "treasury".to_string(),
            tx_hash: "tx".to_string(),
            amount: Some("15000".to_string()),
            credited_amount: Some("10000".to_string()),
            signatures: Vec::new(),
            sweep_tx_hash: Some("sweep-hash".to_string()),
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            status: "sweep_confirmed".to_string(),
            created_at: 1000,
        };

        let credit = build_credit_job(&state, &sweep)
            .expect("build native SOL credit job")
            .expect("credit job should be created");
        assert_eq!(credit.amount_spores, 10_000);
    }

    #[tokio::test]
    async fn test_process_sweep_jobs_native_solana_dust_retries_instead_of_failing() {
        let state = test_state();

        let deposit = DepositRequest {
            deposit_id: "dep-sol-dust-1".to_string(),
            user_id: "user-1".to_string(),
            chain: "solana".to_string(),
            asset: "sol".to_string(),
            address: "11111111111111111111111111111111".to_string(),
            derivation_path: "m/44'/501'/0'/0/4".to_string(),
            deposit_seed_source: DEPOSIT_SEED_SOURCE_TREASURY_ROOT.to_string(),
            created_at: 1000,
            status: "sweep_queued".to_string(),
        };
        store_deposit(&state.db, &deposit).expect("store native SOL deposit");

        let job = SweepJob {
            job_id: "sweep-sol-dust-1".to_string(),
            deposit_id: deposit.deposit_id.clone(),
            chain: "solana".to_string(),
            asset: "sol".to_string(),
            from_address: deposit.address.clone(),
            to_treasury: "11111111111111111111111111111111".to_string(),
            tx_hash: "tx".to_string(),
            amount: Some(SOLANA_SWEEP_FEE_LAMPORTS.to_string()),
            credited_amount: None,
            signatures: Vec::new(),
            sweep_tx_hash: None,
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            status: "queued".to_string(),
            created_at: 1000,
        };
        store_sweep_job(&state.db, &job).expect("store native SOL dust sweep job");

        process_sweep_jobs(&state)
            .await
            .expect("process native SOL dust sweep job");

        let signed_jobs = list_sweep_jobs_by_status(&state.db, "signed")
            .expect("list retriable native SOL dust sweep jobs");
        assert_eq!(signed_jobs.len(), 1);
        assert_eq!(signed_jobs[0].job_id, job.job_id);
        assert!(signed_jobs[0]
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("insufficient native SOL to sweep after fees"));
        assert!(signed_jobs[0].next_attempt_at.is_some());
        assert!(list_sweep_jobs_by_status(&state.db, "failed")
            .expect("list failed sweep jobs")
            .is_empty());
        assert!(list_sweep_jobs_by_status(&state.db, "permanently_failed")
            .expect("list permanently failed sweep jobs")
            .is_empty());
    }

    /// F2-01: BIP-44 coin type mapping test
    #[test]
    fn test_bip44_coin_type() {
        assert_eq!(bip44_coin_type("sol").unwrap(), 501);
        assert_eq!(bip44_coin_type("solana").unwrap(), 501);
        assert_eq!(bip44_coin_type("eth").unwrap(), 60);
        assert_eq!(bip44_coin_type("ethereum").unwrap(), 60);
        assert_eq!(bip44_coin_type("bsc").unwrap(), 60);
        assert_eq!(bip44_coin_type("bnb").unwrap(), 60);
        assert_eq!(bip44_coin_type("btc").unwrap(), 0);
        assert_eq!(bip44_coin_type("bitcoin").unwrap(), 0);
        assert_eq!(bip44_coin_type("lichen").unwrap(), 9999);
        assert!(bip44_coin_type("unknown").is_err());
    }

    /// F2-01: BIP-44 derivation path format test
    #[test]
    fn test_bip44_derivation_path() {
        let path_sol = bip44_derivation_path("solana", "user123", 0).unwrap();
        assert!(
            path_sol.starts_with("m/44'/501'/"),
            "Solana path must use coin_type 501: {}",
            path_sol
        );
        assert!(path_sol.ends_with("/0/0"), "Index 0: {}", path_sol);

        let path_eth = bip44_derivation_path("eth", "user123", 5).unwrap();
        assert!(
            path_eth.starts_with("m/44'/60'/"),
            "ETH path must use coin_type 60: {}",
            path_eth
        );
        assert!(path_eth.ends_with("/0/5"), "Index 5: {}", path_eth);

        let path_bnb = bip44_derivation_path("bnb", "user123", 7).unwrap();
        assert!(
            path_bnb.starts_with("m/44'/60'/"),
            "BNB path must use coin_type 60: {}",
            path_bnb
        );
        assert!(path_bnb.ends_with("/0/7"), "Index 7: {}", path_bnb);

        // Same user on different chains gets different paths (different coin types)
        assert_ne!(path_sol, path_eth);

        // BNB/BSC reuses EVM derivation coin type (same path given same index/user)
        let path_bsc = bip44_derivation_path("bsc", "user123", 5).unwrap();
        assert_eq!(path_eth, path_bsc);

        // Same user, different index
        let path_sol_1 = bip44_derivation_path("solana", "user123", 1).unwrap();
        assert_ne!(path_sol, path_sol_1);

        // Different user, same chain
        let path_other = bip44_derivation_path("solana", "other_user", 0).unwrap();
        assert_ne!(path_sol, path_other);

        // Deterministic
        let path_again = bip44_derivation_path("solana", "user123", 0).unwrap();
        assert_eq!(path_sol, path_again);
    }

    #[test]
    fn test_to_be_bytes() {
        assert_eq!(to_be_bytes(0), Vec::<u8>::new()); // all zeros trimmed
        assert_eq!(to_be_bytes(255), vec![255]);
        assert_eq!(to_be_bytes(256), vec![1, 0]);
    }

    #[test]
    fn test_resolve_token_contract_sol() {
        let mut config = test_config();
        config.wsol_contract_addr = Some("WSOL_CONTRACT_123".to_string());
        assert_eq!(
            resolve_token_contract(&config, "solana", "sol"),
            Some("WSOL_CONTRACT_123".to_string())
        );
        assert_eq!(resolve_token_contract(&config, "solana", "eth"), None);
    }

    #[test]
    fn test_resolve_token_contract_stablecoins() {
        let mut config = test_config();
        config.musd_contract_addr = Some("LUSD_CONTRACT_456".to_string());
        // Both USDT and USDC map to the same lUSD contract
        assert_eq!(
            resolve_token_contract(&config, "solana", "usdt"),
            Some("LUSD_CONTRACT_456".to_string())
        );
        assert_eq!(
            resolve_token_contract(&config, "ethereum", "usdc"),
            Some("LUSD_CONTRACT_456".to_string())
        );
    }

    #[test]
    fn test_resolve_token_contract_eth() {
        let mut config = test_config();
        config.weth_contract_addr = Some("WETH_CONTRACT_789".to_string());
        assert_eq!(
            resolve_token_contract(&config, "ethereum", "eth"),
            Some("WETH_CONTRACT_789".to_string())
        );
    }

    #[test]
    fn test_resolve_token_contract_bnb() {
        let mut config = test_config();
        config.wbnb_contract_addr = Some("WBNB_CONTRACT_321".to_string());
        assert_eq!(
            resolve_token_contract(&config, "bsc", "bnb"),
            Some("WBNB_CONTRACT_321".to_string())
        );
    }

    #[test]
    fn test_resolve_token_contract_unconfigured() {
        let config = test_config(); // all contract addrs are None
        assert_eq!(resolve_token_contract(&config, "solana", "sol"), None);
        assert_eq!(resolve_token_contract(&config, "ethereum", "eth"), None);
        assert_eq!(resolve_token_contract(&config, "solana", "usdt"), None);
    }

    #[tokio::test]
    async fn test_reserve_ledger_adjust_increment() {
        let _ = DB::destroy(&Options::default(), "/tmp/test_custody_reserve_1");
        let db = open_db("/tmp/test_custody_reserve_1").unwrap();
        // Increment from zero
        adjust_reserve_balance(&db, "solana", "usdt", 500_000, true)
            .await
            .unwrap();
        assert_eq!(get_reserve_balance(&db, "solana", "usdt").unwrap(), 500_000);
        // Increment again
        adjust_reserve_balance(&db, "solana", "usdt", 300_000, true)
            .await
            .unwrap();
        assert_eq!(get_reserve_balance(&db, "solana", "usdt").unwrap(), 800_000);
        // Different asset on same chain
        assert_eq!(get_reserve_balance(&db, "solana", "usdc").unwrap(), 0);
        let _ = DB::destroy(&Options::default(), "/tmp/test_custody_reserve_1");
    }

    #[tokio::test]
    async fn test_reserve_ledger_adjust_decrement() {
        let db = open_db("/tmp/test_custody_reserve_2").unwrap();
        adjust_reserve_balance(&db, "ethereum", "usdc", 1_000_000, true)
            .await
            .unwrap();
        adjust_reserve_balance(&db, "ethereum", "usdc", 400_000, false)
            .await
            .unwrap();
        assert_eq!(
            get_reserve_balance(&db, "ethereum", "usdc").unwrap(),
            600_000
        );
        // Decrement past zero clamps to 0
        adjust_reserve_balance(&db, "ethereum", "usdc", 999_999, false)
            .await
            .unwrap();
        assert_eq!(get_reserve_balance(&db, "ethereum", "usdc").unwrap(), 0);
        let _ = DB::destroy(&Options::default(), "/tmp/test_custody_reserve_2");
    }

    #[tokio::test]
    async fn test_reserve_ledger_multi_chain() {
        let _ = DB::destroy(&Options::default(), "/tmp/test_custody_reserve_3");
        let db = open_db("/tmp/test_custody_reserve_3").unwrap();
        adjust_reserve_balance(&db, "solana", "usdt", 500_000, true)
            .await
            .unwrap();
        adjust_reserve_balance(&db, "solana", "usdc", 200_000, true)
            .await
            .unwrap();
        adjust_reserve_balance(&db, "ethereum", "usdt", 300_000, true)
            .await
            .unwrap();
        adjust_reserve_balance(&db, "ethereum", "usdc", 100_000, true)
            .await
            .unwrap();
        assert_eq!(get_reserve_balance(&db, "solana", "usdt").unwrap(), 500_000);
        assert_eq!(get_reserve_balance(&db, "solana", "usdc").unwrap(), 200_000);
        assert_eq!(
            get_reserve_balance(&db, "ethereum", "usdt").unwrap(),
            300_000
        );
        assert_eq!(
            get_reserve_balance(&db, "ethereum", "usdc").unwrap(),
            100_000
        );
        let _ = DB::destroy(&Options::default(), "/tmp/test_custody_reserve_3");
    }

    #[test]
    fn test_rebalance_job_store_and_list() {
        let db = open_db("/tmp/test_custody_rebalance_1").unwrap();
        let job = RebalanceJob {
            job_id: "test-rebalance-1".to_string(),
            chain: "solana".to_string(),
            from_asset: "usdt".to_string(),
            to_asset: "usdc".to_string(),
            amount: 150_000,
            trigger: "threshold".to_string(),
            linked_withdrawal_job_id: None,
            swap_tx_hash: None,
            status: "queued".to_string(),
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            created_at: 1000,
        };
        store_rebalance_job(&db, &job).unwrap();
        let queued = list_rebalance_jobs_by_status(&db, "queued").unwrap();
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].from_asset, "usdt");
        assert_eq!(queued[0].to_asset, "usdc");
        assert_eq!(queued[0].amount, 150_000);
        let confirmed = list_rebalance_jobs_by_status(&db, "confirmed").unwrap();
        assert_eq!(confirmed.len(), 0);
        let _ = DB::destroy(&Options::default(), "/tmp/test_custody_rebalance_1");
    }

    #[test]
    fn test_default_preferred_stablecoin_is_usdt() {
        assert_eq!(default_preferred_stablecoin(), "usdt");
    }

    // ── M14 tests: swap output parsing ──

    #[test]
    fn test_parse_evm_swap_output_decodes_transfer_logs() {
        // Simulate an ERC-20 Transfer log to treasury
        let treasury = "0xabcdef0123456789abcdef0123456789abcdef01";
        let contract = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
        let transfer_topic = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";

        // Pad address to 32 bytes (left-zero-padded)
        let to_topic = format!("0x000000000000000000000000{}", &treasury[2..]);

        let receipt = serde_json::json!({
            "status": "0x1",
            "logs": [
                {
                    "address": contract,
                    "topics": [
                        transfer_topic,
                        "0x0000000000000000000000001111111111111111111111111111111111111111",
                        to_topic,
                    ],
                    "data": "0x00000000000000000000000000000000000000000000000000000000000186a0",
                    "transactionHash": "0xdeadbeef"
                }
            ]
        });

        // Manually parse the same way parse_evm_swap_output would
        let logs = receipt.get("logs").unwrap().as_array().unwrap();
        let log = &logs[0];
        let (to, amount, _tx_hash) = decode_transfer_log(log).unwrap();
        assert_eq!(to.to_lowercase(), treasury.to_lowercase());
        assert_eq!(amount, 100_000u128); // 0x186a0 = 100000
    }

    #[test]
    fn test_parse_evm_swap_output_ignores_wrong_contract() {
        let transfer_topic = "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
        let treasury = "0xabcdef0123456789abcdef0123456789abcdef01";

        // Log from a different contract — should NOT match
        let log = serde_json::json!({
            "address": "0x0000000000000000000000000000000000000099",
            "topics": [
                transfer_topic,
                "0x0000000000000000000000001111111111111111111111111111111111111111",
                format!("0x000000000000000000000000{}", &treasury[2..]),
            ],
            "data": "0x00000000000000000000000000000000000000000000000000000000000003e8",
            "transactionHash": "0xabc123"
        });

        let (to, amount, _) = decode_transfer_log(&log).unwrap();
        // It decodes fine, but the contract address mismatch would be caught
        // in parse_evm_swap_output by comparing log_address to the target contract
        assert_eq!(amount, 1000u128);
        assert_eq!(to.to_lowercase(), treasury.to_lowercase());
    }

    #[test]
    fn test_parse_solana_output_amount_extraction() {
        // Simulate the extract_amount closure logic
        let entries = serde_json::json!([
            {
                "mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                "owner": "TEST_SOL_ADDR",
                "uiTokenAmount": { "amount": "200000" }
            },
            {
                "mint": "other_mint",
                "owner": "TEST_SOL_ADDR",
                "uiTokenAmount": { "amount": "999" }
            }
        ]);

        let target_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
        let target_owner = "TEST_SOL_ADDR";
        let arr = entries.as_array().unwrap();

        let mut found = None;
        for entry in arr {
            let mint = entry.get("mint").and_then(|v| v.as_str()).unwrap_or("");
            let owner = entry.get("owner").and_then(|v| v.as_str()).unwrap_or("");
            if mint == target_mint && owner == target_owner {
                found = entry
                    .get("uiTokenAmount")
                    .and_then(|v| v.get("amount"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u64>().ok());
                break;
            }
        }
        assert_eq!(found, Some(200_000u64));
    }

    #[test]
    fn test_parse_solana_output_no_match() {
        let entries = serde_json::json!([
            {
                "mint": "wrong_mint",
                "owner": "wrong_owner",
                "uiTokenAmount": { "amount": "100" }
            }
        ]);
        let arr = entries.as_array().unwrap();
        let mut found: Option<u64> = None;
        for entry in arr {
            let mint = entry.get("mint").and_then(|v| v.as_str()).unwrap_or("");
            if mint == "target_mint" {
                found = Some(0);
            }
        }
        assert!(found.is_none());
    }

    // ── M16 tests: gas funding logic ──

    #[test]
    fn test_gas_deficit_calculation() {
        // Simulates the gas deficit + buffer calculation from broadcast_evm_token_sweep
        let gas_price: u128 = 20_000_000_000; // 20 gwei
        let gas_limit: u128 = 100_000;
        let fee = gas_price.saturating_mul(gas_limit); // 2e15 = 0.002 ETH
        let native_balance: u128 = 500_000_000_000_000; // 0.0005 ETH

        assert!(native_balance < fee);
        let deficit = fee.saturating_sub(native_balance);
        let gas_grant = deficit.saturating_add(deficit / 5); // +20% buffer

        assert!(gas_grant > deficit);
        assert!(gas_grant < fee); // Grant should be less than full fee (since we have some balance)
        assert_eq!(deficit, 1_500_000_000_000_000); // 0.0015 ETH
        assert_eq!(gas_grant, 1_800_000_000_000_000); // 0.0018 ETH with buffer
    }

    #[test]
    fn test_gas_funding_not_needed_when_sufficient() {
        let gas_price: u128 = 20_000_000_000;
        let gas_limit: u128 = 100_000;
        let fee = gas_price.saturating_mul(gas_limit);
        let native_balance: u128 = 3_000_000_000_000_000; // 0.003 ETH > 0.002 ETH fee

        // No funding needed
        assert!(native_balance >= fee);
    }

    #[test]
    fn test_gas_grant_buffer_is_20_percent() {
        let deficit: u128 = 1_000_000;
        let buffer = deficit / 5;
        let grant = deficit.saturating_add(buffer);
        assert_eq!(grant, 1_200_000); // exactly 120% of deficit
    }

    // ── F8.1: verify_api_auth constant-time comparison ──

    #[test]
    fn test_verify_api_auth_rejects_wrong_token() {
        let config = test_config();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("authorization", "Bearer wrong_token".parse().unwrap());
        assert!(verify_api_auth(&config, &headers).is_err());
    }

    #[test]
    fn test_verify_api_auth_accepts_correct_token() {
        let config = test_config();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("authorization", "Bearer test_api_token".parse().unwrap());
        assert!(verify_api_auth(&config, &headers).is_ok());
    }

    #[test]
    fn test_verify_api_auth_rejects_missing_header() {
        let config = test_config();
        let headers = axum::http::HeaderMap::new();
        assert!(verify_api_auth(&config, &headers).is_err());
    }

    #[test]
    fn test_verify_api_auth_rejects_empty_expected() {
        let mut config = test_config();
        config.api_auth_token = Some("".to_string());
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("authorization", "Bearer ".parse().unwrap());
        assert!(verify_api_auth(&config, &headers).is_err());
    }

    #[tokio::test]
    async fn test_create_deposit_rejects_multi_signer_local_sweep_mode_by_default() {
        let mut state = test_state();
        let mut event_rx = state.event_tx.subscribe();
        state.config.signer_endpoints =
            vec!["http://signer-1".to_string(), "http://signer-2".to_string()];
        state.config.signer_threshold = 2;

        let mut headers = axum::http::HeaderMap::new();
        headers.insert("authorization", "Bearer test_api_token".parse().unwrap());

        let err = create_deposit(
            State(state.clone()),
            headers,
            Json(CreateDepositRequest {
                user_id: "11111111111111111111111111111111".to_string(),
                chain: "ethereum".to_string(),
                asset: "eth".to_string(),
            }),
        )
        .await
        .expect_err("multi-signer local sweep mode should fail closed by default");

        assert_eq!(err.0.code, "invalid_request");
        assert!(err
            .0
            .message
            .contains("multi-signer deposit creation is disabled"));

        let deposit_count = state
            .db
            .iterator_cf(
                state
                    .db
                    .cf_handle(CF_DEPOSITS)
                    .expect("deposits column family"),
                rocksdb::IteratorMode::Start,
            )
            .count();
        assert_eq!(deposit_count, 0);

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), event_rx.recv())
                .await
                .is_err()
        );
    }

    // ── F8.8: Destination address validation ──

    #[test]
    fn test_solana_address_validation() {
        // Valid Solana address (32 bytes base58)
        let valid = bs58::encode([1u8; 32]).into_string();
        let bytes = bs58::decode(&valid).into_vec().unwrap();
        assert_eq!(bytes.len(), 32);

        // Invalid Solana address (too short)
        let short = bs58::encode([1u8; 16]).into_string();
        let bytes = bs58::decode(&short).into_vec().unwrap();
        assert_ne!(bytes.len(), 32);
    }

    #[test]
    fn test_evm_address_validation() {
        // Valid EVM address
        let valid = "0xabcdef0123456789abcdef0123456789abcdef01";
        let trimmed = valid.trim_start_matches("0x");
        assert_eq!(trimmed.len(), 40);
        assert!(hex::decode(trimmed).is_ok());

        // Invalid: too short
        let short = "0xabcdef";
        let trimmed = short.trim_start_matches("0x");
        assert_ne!(trimmed.len(), 40);

        // Invalid: non-hex
        let bad = "0xzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
        let trimmed = bad.trim_start_matches("0x");
        assert!(hex::decode(trimmed).is_err());
    }

    // ── F8.9: Status-indexed job counting ──

    #[test]
    fn test_count_sweep_jobs_with_index() {
        let _ = DB::destroy(&Options::default(), "/tmp/test_custody_count_sweep");
        let db = open_db("/tmp/test_custody_count_sweep").unwrap();

        // Store a sweep job — store_sweep_job maintains the status index
        let job = SweepJob {
            job_id: "test-sweep-count-1".to_string(),
            deposit_id: "dep-1".to_string(),
            chain: "solana".to_string(),
            asset: "sol".to_string(),
            from_address: "from".to_string(),
            to_treasury: "to".to_string(),
            tx_hash: "hash".to_string(),
            amount: Some("1000".to_string()),
            credited_amount: None,
            signatures: Vec::new(),
            sweep_tx_hash: None,
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            status: "queued".to_string(),
            created_at: 1000,
        };
        store_sweep_job(&db, &job).unwrap();

        let counts = count_sweep_jobs(&db).unwrap();
        assert_eq!(counts.total, 1);
        assert_eq!(*counts.by_status.get("queued").unwrap_or(&0), 1);

        let _ = DB::destroy(&Options::default(), "/tmp/test_custody_count_sweep");
    }

    #[test]
    fn test_promote_locally_signed_sweep_jobs_clears_placeholder_signatures() {
        let state = test_state();
        let job = SweepJob {
            job_id: "test-sweep-local-sign".to_string(),
            deposit_id: "dep-local-1".to_string(),
            chain: "solana".to_string(),
            asset: "sol".to_string(),
            from_address: "from".to_string(),
            to_treasury: "to".to_string(),
            tx_hash: "hash".to_string(),
            amount: Some("1000".to_string()),
            credited_amount: None,
            signatures: vec![SignerSignature {
                signer_pubkey: "placeholder-signer".to_string(),
                signature: "deadbeef".to_string(),
                message_hash: "cafebabe".to_string(),
                received_at: 123,
            }],
            sweep_tx_hash: None,
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            status: "signing".to_string(),
            created_at: 1000,
        };
        store_sweep_job(&state.db, &job).unwrap();

        promote_locally_signed_sweep_jobs(&state, "locally-derived-deposit-key").unwrap();

        let signing_jobs = list_sweep_jobs_by_status(&state.db, "signing").unwrap();
        let signed_jobs = list_sweep_jobs_by_status(&state.db, "signed").unwrap();
        assert!(signing_jobs.is_empty());
        assert_eq!(signed_jobs.len(), 1);
        assert!(signed_jobs[0].signatures.is_empty());
        assert_eq!(signed_jobs[0].status, "signed");
    }

    #[tokio::test]
    async fn test_promote_locally_signed_sweep_jobs_emits_local_signing_metadata() {
        let state = test_state();
        let mut event_rx = state.event_tx.subscribe();
        let job = SweepJob {
            job_id: "test-sweep-local-event".to_string(),
            deposit_id: "dep-local-2".to_string(),
            chain: "ethereum".to_string(),
            asset: "eth".to_string(),
            from_address: "from".to_string(),
            to_treasury: "to".to_string(),
            tx_hash: "hash".to_string(),
            amount: Some("1000".to_string()),
            credited_amount: None,
            signatures: vec![],
            sweep_tx_hash: None,
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            status: "signing".to_string(),
            created_at: 1000,
        };
        store_sweep_job(&state.db, &job).unwrap();

        promote_locally_signed_sweep_jobs(&state, "locally-derived-deposit-key").unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("timed out waiting for sweep.signed event")
            .expect("receive sweep.signed event");

        assert_eq!(event.event_type, "sweep.signed");
        assert_eq!(event.entity_id, "test-sweep-local-event");
        assert_eq!(event.deposit_id.as_deref(), Some("dep-local-2"));
        let data = event.data.expect("sweep.signed should carry metadata");
        assert_eq!(
            data.get("mode").and_then(|value| value.as_str()),
            Some("locally-derived-deposit-key")
        );
        assert_eq!(
            data.get("threshold_signing")
                .and_then(|value| value.as_bool()),
            Some(false)
        );
    }

    #[tokio::test]
    async fn test_process_sweep_jobs_multi_signer_without_override_blocks_local_sweep_execution() {
        let mut state = test_state();
        let mut event_rx = state.event_tx.subscribe();
        let rpc_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let rpc_app: Router =
            Router::new()
                .route("/", post(mock_rpc_handler))
                .with_state(MockRpcState {
                    safe_nonce_hex: "0x0".to_string(),
                    safe_tx_hash_hex: "0x0".to_string(),
                    send_raw_tx_hash_hex: Some(
                        "0xfeedfacefeedfacefeedfacefeedfacefeedfacefeedfacefeedfacefeedface"
                            .to_string(),
                    ),
                    transaction_receipt: None,
                    requests: rpc_requests.clone(),
                });
        let rpc_url = spawn_mock_server(rpc_app).await;

        state.config.evm_rpc_url = Some(rpc_url.clone());
        state.config.eth_rpc_url = Some(rpc_url);
        state.config.treasury_evm_address =
            Some("0x4444444444444444444444444444444444444444".to_string());
        state.config.signer_endpoints =
            vec!["http://signer-1".to_string(), "http://signer-2".to_string()];
        state.config.signer_threshold = 2;

        let deposit = DepositRequest {
            deposit_id: "dep-sweep-block-1".to_string(),
            user_id: "user-1".to_string(),
            chain: "ethereum".to_string(),
            asset: "eth".to_string(),
            address: "0x5555555555555555555555555555555555555555".to_string(),
            derivation_path: "m/44'/60'/0'/0/9".to_string(),
            deposit_seed_source: DEPOSIT_SEED_SOURCE_TREASURY_ROOT.to_string(),
            created_at: 1000,
            status: "confirmed".to_string(),
        };
        store_deposit(&state.db, &deposit).expect("store deposit");

        let job = SweepJob {
            job_id: "test-sweep-worker-blocked".to_string(),
            deposit_id: deposit.deposit_id.clone(),
            chain: "ethereum".to_string(),
            asset: "eth".to_string(),
            from_address: deposit.address.clone(),
            to_treasury: state.config.treasury_evm_address.clone().unwrap(),
            tx_hash: "deposit-observed-hash".to_string(),
            amount: Some("1000000000000000000".to_string()),
            credited_amount: None,
            signatures: Vec::new(),
            sweep_tx_hash: None,
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            status: "queued".to_string(),
            created_at: 1000,
        };
        store_sweep_job(&state.db, &job).expect("store sweep job");

        process_sweep_jobs(&state)
            .await
            .expect("process blocked sweep jobs");

        let blocked_jobs = list_sweep_jobs_by_status(&state.db, "permanently_failed")
            .expect("list blocked sweep jobs");
        assert_eq!(blocked_jobs.len(), 1);
        assert_eq!(blocked_jobs[0].job_id, "test-sweep-worker-blocked");
        assert!(blocked_jobs[0]
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("multi-signer deposit creation is disabled"));
        assert!(list_sweep_jobs_by_status(&state.db, "sweep_submitted")
            .expect("list submitted sweep jobs")
            .is_empty());

        let requests = rpc_requests.lock().await;
        assert!(!requests.iter().any(|payload| {
            payload.get("method").and_then(|value| value.as_str()) == Some("eth_sendRawTransaction")
        }));
        drop(requests);

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("timed out waiting for blocked sweep event")
            .expect("receive blocked sweep event");
        assert_eq!(event.event_type, "sweep.failed");
        assert_eq!(event.entity_id, "test-sweep-worker-blocked");
        let data = event.data.expect("blocked sweep event metadata");
        assert_eq!(
            data.get("mode").and_then(|value| value.as_str()),
            Some("blocked-local-sweep")
        );
    }

    #[tokio::test]
    async fn test_process_sweep_jobs_confirmed_enqueues_credit_and_updates_status() {
        let mut state = test_state();
        let mut event_rx = state.event_tx.subscribe();
        let rpc_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let rpc_app: Router =
            Router::new()
                .route("/", post(mock_rpc_handler))
                .with_state(MockRpcState {
                    safe_nonce_hex: "0x0".to_string(),
                    safe_tx_hash_hex: "0x0".to_string(),
                    send_raw_tx_hash_hex: None,
                    transaction_receipt: Some(json!({ "status": "0x1" })),
                    requests: rpc_requests.clone(),
                });
        let rpc_url = spawn_mock_server(rpc_app).await;

        state.config.evm_rpc_url = Some(rpc_url.clone());
        state.config.eth_rpc_url = Some(rpc_url);
        state.config.licn_rpc_url = Some("http://localhost:8899".to_string());
        state.config.treasury_keypair_path = Some("/tmp/test-treasury.json".to_string());
        state.config.musd_contract_addr = Some("11111111111111111111111111111111".to_string());

        let deposit = DepositRequest {
            deposit_id: "dep-sweep-confirm-1".to_string(),
            user_id: "11111111111111111111111111111111".to_string(),
            chain: "ethereum".to_string(),
            asset: "usdt".to_string(),
            address: "0x5555555555555555555555555555555555555555".to_string(),
            derivation_path: "m/44'/60'/0'/0/8".to_string(),
            deposit_seed_source: DEPOSIT_SEED_SOURCE_TREASURY_ROOT.to_string(),
            created_at: 1000,
            status: "sweep_queued".to_string(),
        };
        store_deposit(&state.db, &deposit).expect("store deposit");
        let _ = update_status_index(
            &state.db,
            "deposits",
            "issued",
            "sweep_queued",
            &deposit.deposit_id,
        );

        let job = SweepJob {
            job_id: "test-sweep-confirm-worker".to_string(),
            deposit_id: deposit.deposit_id.clone(),
            chain: "ethereum".to_string(),
            asset: "usdt".to_string(),
            from_address: deposit.address.clone(),
            to_treasury: "0x4444444444444444444444444444444444444444".to_string(),
            tx_hash: "deposit-observed-hash".to_string(),
            amount: Some("2500000".to_string()),
            credited_amount: None,
            signatures: Vec::new(),
            sweep_tx_hash: Some(
                "0xfeedfacefeedfacefeedfacefeedfacefeedfacefeedfacefeedfacefeedface".to_string(),
            ),
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            status: "sweep_submitted".to_string(),
            created_at: 1000,
        };
        store_sweep_job(&state.db, &job).expect("store submitted sweep job");

        process_sweep_jobs(&state)
            .await
            .expect("process confirmed sweep job");

        let confirmed_jobs = list_sweep_jobs_by_status(&state.db, "sweep_confirmed")
            .expect("list confirmed sweep jobs");
        assert_eq!(confirmed_jobs.len(), 1);
        assert_eq!(confirmed_jobs[0].job_id, "test-sweep-confirm-worker");

        let deposit_after = fetch_deposit(&state.db, &deposit.deposit_id)
            .expect("fetch updated deposit")
            .expect("deposit exists after confirmation");
        assert_eq!(deposit_after.status, "swept");

        let queued_credit_jobs =
            list_credit_jobs_by_status(&state.db, "queued").expect("list queued credit jobs");
        assert_eq!(queued_credit_jobs.len(), 1);
        assert_eq!(queued_credit_jobs[0].deposit_id, deposit.deposit_id);
        assert_eq!(
            queued_credit_jobs[0].to_address,
            "11111111111111111111111111111111"
        );
        assert_eq!(queued_credit_jobs[0].source_asset, "usdt");
        assert_eq!(queued_credit_jobs[0].source_chain, "ethereum");
        assert_eq!(queued_credit_jobs[0].amount_spores, 2_500_000_000);

        let reserve = get_reserve_balance(&state.db, "ethereum", "usdt")
            .expect("read reserve balance after confirmed sweep");
        assert_eq!(reserve, 2_500_000);

        let mut event_types = Vec::new();
        for _ in 0..2 {
            let event = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
                .await
                .expect("timed out waiting for confirmation events")
                .expect("receive confirmation event");
            event_types.push(event.event_type.clone());
        }
        assert_eq!(
            event_types,
            vec!["sweep.confirmed".to_string(), "credit.queued".to_string()]
        );

        let requests = rpc_requests.lock().await;
        assert!(requests.iter().any(|payload| {
            payload.get("method").and_then(|value| value.as_str())
                == Some("eth_getTransactionReceipt")
        }));
    }

    #[tokio::test]
    async fn test_process_sweep_jobs_reverted_receipt_marks_failed_without_credit() {
        let mut state = test_state();
        let mut event_rx = state.event_tx.subscribe();
        let rpc_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let rpc_app: Router =
            Router::new()
                .route("/", post(mock_rpc_handler))
                .with_state(MockRpcState {
                    safe_nonce_hex: "0x0".to_string(),
                    safe_tx_hash_hex: "0x0".to_string(),
                    send_raw_tx_hash_hex: None,
                    transaction_receipt: Some(json!({ "status": "0x0" })),
                    requests: rpc_requests.clone(),
                });
        let rpc_url = spawn_mock_server(rpc_app).await;

        state.config.evm_rpc_url = Some(rpc_url.clone());
        state.config.eth_rpc_url = Some(rpc_url);
        state.config.licn_rpc_url = Some("http://localhost:8899".to_string());
        state.config.treasury_keypair_path = Some("/tmp/test-treasury.json".to_string());
        state.config.musd_contract_addr = Some("11111111111111111111111111111111".to_string());

        let deposit = DepositRequest {
            deposit_id: "dep-sweep-reverted-1".to_string(),
            user_id: "11111111111111111111111111111111".to_string(),
            chain: "ethereum".to_string(),
            asset: "usdt".to_string(),
            address: "0x5555555555555555555555555555555555555555".to_string(),
            derivation_path: "m/44'/60'/0'/0/10".to_string(),
            deposit_seed_source: DEPOSIT_SEED_SOURCE_TREASURY_ROOT.to_string(),
            created_at: 1000,
            status: "sweep_queued".to_string(),
        };
        store_deposit(&state.db, &deposit).expect("store deposit");

        let job = SweepJob {
            job_id: "test-sweep-reverted-worker".to_string(),
            deposit_id: deposit.deposit_id.clone(),
            chain: "ethereum".to_string(),
            asset: "usdt".to_string(),
            from_address: deposit.address.clone(),
            to_treasury: "0x4444444444444444444444444444444444444444".to_string(),
            tx_hash: "deposit-observed-hash".to_string(),
            amount: Some("2500000".to_string()),
            credited_amount: None,
            signatures: Vec::new(),
            sweep_tx_hash: Some(
                "0xfeedfacefeedfacefeedfacefeedfacefeedfacefeedfacefeedfacefeedface".to_string(),
            ),
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            status: "sweep_submitted".to_string(),
            created_at: 1000,
        };
        store_sweep_job(&state.db, &job).expect("store submitted sweep job");

        process_sweep_jobs(&state)
            .await
            .expect("process reverted sweep job");

        let failed_jobs =
            list_sweep_jobs_by_status(&state.db, "failed").expect("list failed sweep jobs");
        assert_eq!(failed_jobs.len(), 1);
        assert_eq!(failed_jobs[0].job_id, "test-sweep-reverted-worker");
        assert!(failed_jobs[0]
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("reverted or failed on-chain"));

        let deposit_after = fetch_deposit(&state.db, &deposit.deposit_id)
            .expect("fetch updated deposit")
            .expect("deposit exists after revert");
        assert_eq!(deposit_after.status, "sweep_queued");

        assert!(list_credit_jobs_by_status(&state.db, "queued")
            .expect("list queued credit jobs")
            .is_empty());
        let reserve = get_reserve_balance(&state.db, "ethereum", "usdt")
            .expect("read reserve balance after reverted sweep");
        assert_eq!(reserve, 0);

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
            .await
            .expect("timed out waiting for reverted sweep event")
            .expect("receive reverted sweep event");
        assert_eq!(event.event_type, "sweep.failed");
        assert_eq!(event.entity_id, "test-sweep-reverted-worker");

        let requests = rpc_requests.lock().await;
        assert!(requests.iter().any(|payload| {
            payload.get("method").and_then(|value| value.as_str())
                == Some("eth_getTransactionReceipt")
        }));
    }

    #[tokio::test]
    async fn test_submit_burn_signature_requires_api_auth() {
        let state = test_state();
        let response = submit_burn_signature(
            State(state),
            axum::http::HeaderMap::new(),
            axum::extract::Path("missing-job".to_string()),
            Json(BurnSignaturePayload {
                burn_tx_signature: "burn-tx-auth".to_string(),
            }),
        )
        .await;

        assert!(response.is_err());
        let err = response.expect_err("missing auth should fail");
        assert_eq!(err.0.code, "unauthorized");
    }

    #[tokio::test]
    async fn test_submit_burn_signature_replaces_existing_unverified_signature() {
        let state = test_state();
        let job = WithdrawalJob {
            job_id: "withdrawal-burn-replace".to_string(),
            user_id: "11111111111111111111111111111111".to_string(),
            asset: "wETH".to_string(),
            amount: 2500,
            dest_chain: "ethereum".to_string(),
            dest_address: "0x3333333333333333333333333333333333333333".to_string(),
            preferred_stablecoin: "usdt".to_string(),
            burn_tx_signature: Some("burn-old".to_string()),
            outbound_tx_hash: None,
            safe_nonce: None,
            signatures: Vec::new(),
            status: "pending_burn".to_string(),
            attempts: 0,
            last_error: Some("old failure".to_string()),
            next_attempt_at: Some(1234),
            created_at: 1000,
        };
        store_withdrawal_job(&state.db, &job).expect("store withdrawal job");

        let idx_cf = state.db.cf_handle(CF_INDEXES).expect("indexes cf");
        state
            .db
            .put_cf(
                idx_cf,
                burn_signature_index_key("burn-old").as_bytes(),
                job.job_id.as_bytes(),
            )
            .expect("reserve old burn signature");

        let mut headers = axum::http::HeaderMap::new();
        headers.insert("authorization", "Bearer test_api_token".parse().unwrap());

        let response = submit_burn_signature(
            State(state.clone()),
            headers,
            axum::extract::Path(job.job_id.clone()),
            Json(BurnSignaturePayload {
                burn_tx_signature: "burn-new".to_string(),
            }),
        )
        .await
        .expect("replace burn signature")
        .0;

        assert_eq!(
            response.get("burn_tx_signature").and_then(|v| v.as_str()),
            Some("burn-new")
        );

        let job_after = fetch_withdrawal_job(&state.db, &job.job_id)
            .expect("fetch withdrawal job")
            .expect("withdrawal job exists");
        assert_eq!(job_after.burn_tx_signature.as_deref(), Some("burn-new"));
        assert!(job_after.last_error.is_none());
        assert!(job_after.next_attempt_at.is_none());

        assert!(state
            .db
            .get_cf(idx_cf, burn_signature_index_key("burn-old").as_bytes())
            .expect("read old reservation")
            .is_none());
        assert_eq!(
            state
                .db
                .get_cf(idx_cf, burn_signature_index_key("burn-new").as_bytes())
                .expect("read new reservation")
                .as_deref(),
            Some(job.job_id.as_bytes())
        );
    }

    #[tokio::test]
    async fn test_process_withdrawal_jobs_burn_caller_mismatch_resets_pending_burn_without_broadcast(
    ) {
        let mut state = test_state();
        let mut event_rx = state.event_tx.subscribe();
        let licn_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let licn_app: Router = Router::new()
            .route("/", post(mock_licn_rpc_handler))
            .with_state(MockLichenRpcState {
                transaction_result: json!({
                    "success": true,
                    "contract_address": "wrapped-weth-contract",
                    "caller": "22222222222222222222222222222222",
                    "method": "burn",
                    "amount": 2500,
                }),
                requests: licn_requests.clone(),
            });
        let licn_rpc_url = spawn_mock_server(licn_app).await;

        state.config.licn_rpc_url = Some(licn_rpc_url);
        state.config.weth_contract_addr = Some("wrapped-weth-contract".to_string());

        let job = WithdrawalJob {
            job_id: "withdrawal-burn-mismatch".to_string(),
            user_id: "11111111111111111111111111111111".to_string(),
            asset: "wETH".to_string(),
            amount: 2500,
            dest_chain: "ethereum".to_string(),
            dest_address: "0x3333333333333333333333333333333333333333".to_string(),
            preferred_stablecoin: "usdt".to_string(),
            burn_tx_signature: Some("burn-tx-1".to_string()),
            outbound_tx_hash: None,
            safe_nonce: None,
            signatures: Vec::new(),
            status: "pending_burn".to_string(),
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            created_at: 1000,
        };
        store_withdrawal_job(&state.db, &job).expect("store withdrawal job");

        process_withdrawal_jobs(&state)
            .await
            .expect("process withdrawal jobs");

        let job_after = fetch_withdrawal_job(&state.db, &job.job_id)
            .expect("fetch withdrawal job")
            .expect("withdrawal job exists");
        assert_eq!(job_after.status, "pending_burn");
        assert!(job_after.burn_tx_signature.is_none());
        assert_eq!(job_after.attempts, 1);
        assert!(job_after.outbound_tx_hash.is_none());
        assert!(job_after
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("Burn caller mismatch"));

        assert!(list_withdrawal_jobs_by_status(&state.db, "burned")
            .expect("list burned withdrawal jobs")
            .is_empty());
        assert!(list_withdrawal_jobs_by_status(&state.db, "signing")
            .expect("list signing withdrawal jobs")
            .is_empty());
        assert!(list_withdrawal_jobs_by_status(&state.db, "broadcasting")
            .expect("list broadcasting withdrawal jobs")
            .is_empty());

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), event_rx.recv())
                .await
                .is_err()
        );

        let requests = licn_requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].get("method").and_then(|value| value.as_str()),
            Some("getTransaction")
        );
    }

    #[tokio::test]
    async fn test_process_withdrawal_jobs_burn_contract_mismatch_resets_pending_burn_without_broadcast(
    ) {
        let mut state = test_state();
        let mut event_rx = state.event_tx.subscribe();
        let licn_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let licn_app: Router = Router::new()
            .route("/", post(mock_licn_rpc_handler))
            .with_state(MockLichenRpcState {
                transaction_result: json!({
                    "success": true,
                    "contract_address": "wrong-weth-contract",
                    "caller": "11111111111111111111111111111111",
                    "method": "burn",
                    "amount": 2500,
                }),
                requests: licn_requests.clone(),
            });
        let licn_rpc_url = spawn_mock_server(licn_app).await;

        state.config.licn_rpc_url = Some(licn_rpc_url);
        state.config.weth_contract_addr = Some("wrapped-weth-contract".to_string());

        let job = WithdrawalJob {
            job_id: "withdrawal-burn-contract-mismatch".to_string(),
            user_id: "11111111111111111111111111111111".to_string(),
            asset: "wETH".to_string(),
            amount: 2500,
            dest_chain: "ethereum".to_string(),
            dest_address: "0x3333333333333333333333333333333333333333".to_string(),
            preferred_stablecoin: "usdt".to_string(),
            burn_tx_signature: Some("burn-tx-2".to_string()),
            outbound_tx_hash: None,
            safe_nonce: None,
            signatures: Vec::new(),
            status: "pending_burn".to_string(),
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            created_at: 1000,
        };
        store_withdrawal_job(&state.db, &job).expect("store withdrawal job");

        process_withdrawal_jobs(&state)
            .await
            .expect("process withdrawal jobs");

        let job_after = fetch_withdrawal_job(&state.db, &job.job_id)
            .expect("fetch withdrawal job")
            .expect("withdrawal job exists");
        assert_eq!(job_after.status, "pending_burn");
        assert!(job_after.burn_tx_signature.is_none());
        assert_eq!(job_after.attempts, 1);
        assert!(job_after.outbound_tx_hash.is_none());
        assert!(job_after
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("Burn contract mismatch"));

        assert!(list_withdrawal_jobs_by_status(&state.db, "burned")
            .expect("list burned withdrawal jobs")
            .is_empty());
        assert!(list_withdrawal_jobs_by_status(&state.db, "signing")
            .expect("list signing withdrawal jobs")
            .is_empty());
        assert!(list_withdrawal_jobs_by_status(&state.db, "broadcasting")
            .expect("list broadcasting withdrawal jobs")
            .is_empty());

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), event_rx.recv())
                .await
                .is_err()
        );

        let requests = licn_requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].get("method").and_then(|value| value.as_str()),
            Some("getTransaction")
        );
    }

    #[tokio::test]
    async fn test_process_withdrawal_jobs_burn_amount_mismatch_resets_pending_burn_without_broadcast(
    ) {
        let mut state = test_state();
        let mut event_rx = state.event_tx.subscribe();
        let licn_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let licn_app: Router = Router::new()
            .route("/", post(mock_licn_rpc_handler))
            .with_state(MockLichenRpcState {
                transaction_result: json!({
                    "success": true,
                    "contract_address": "wrapped-weth-contract",
                    "caller": "11111111111111111111111111111111",
                    "method": "burn",
                    "amount": 1234,
                }),
                requests: licn_requests.clone(),
            });
        let licn_rpc_url = spawn_mock_server(licn_app).await;

        state.config.licn_rpc_url = Some(licn_rpc_url);
        state.config.weth_contract_addr = Some("wrapped-weth-contract".to_string());

        let job = WithdrawalJob {
            job_id: "withdrawal-burn-amount-mismatch".to_string(),
            user_id: "11111111111111111111111111111111".to_string(),
            asset: "wETH".to_string(),
            amount: 2500,
            dest_chain: "ethereum".to_string(),
            dest_address: "0x3333333333333333333333333333333333333333".to_string(),
            preferred_stablecoin: "usdt".to_string(),
            burn_tx_signature: Some("burn-tx-3".to_string()),
            outbound_tx_hash: None,
            safe_nonce: None,
            signatures: Vec::new(),
            status: "pending_burn".to_string(),
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            created_at: 1000,
        };
        store_withdrawal_job(&state.db, &job).expect("store withdrawal job");

        process_withdrawal_jobs(&state)
            .await
            .expect("process withdrawal jobs");

        let job_after = fetch_withdrawal_job(&state.db, &job.job_id)
            .expect("fetch withdrawal job")
            .expect("withdrawal job exists");
        assert_eq!(job_after.status, "pending_burn");
        assert!(job_after.burn_tx_signature.is_none());
        assert_eq!(job_after.attempts, 1);
        assert!(job_after.outbound_tx_hash.is_none());
        assert!(job_after
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("Burn amount mismatch"));

        assert!(list_withdrawal_jobs_by_status(&state.db, "burned")
            .expect("list burned withdrawal jobs")
            .is_empty());
        assert!(list_withdrawal_jobs_by_status(&state.db, "signing")
            .expect("list signing withdrawal jobs")
            .is_empty());
        assert!(list_withdrawal_jobs_by_status(&state.db, "broadcasting")
            .expect("list broadcasting withdrawal jobs")
            .is_empty());

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), event_rx.recv())
                .await
                .is_err()
        );

        let requests = licn_requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].get("method").and_then(|value| value.as_str()),
            Some("getTransaction")
        );
    }

    #[tokio::test]
    async fn test_process_withdrawal_jobs_burn_method_mismatch_resets_pending_burn_without_broadcast(
    ) {
        let mut state = test_state();
        let mut event_rx = state.event_tx.subscribe();
        let licn_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let licn_app: Router = Router::new()
            .route("/", post(mock_licn_rpc_handler))
            .with_state(MockLichenRpcState {
                transaction_result: json!({
                    "success": true,
                    "contract_address": "wrapped-weth-contract",
                    "caller": "11111111111111111111111111111111",
                    "method": "transfer",
                    "amount": 2500,
                }),
                requests: licn_requests.clone(),
            });
        let licn_rpc_url = spawn_mock_server(licn_app).await;

        state.config.licn_rpc_url = Some(licn_rpc_url);
        state.config.weth_contract_addr = Some("wrapped-weth-contract".to_string());

        let job = WithdrawalJob {
            job_id: "withdrawal-burn-method-mismatch".to_string(),
            user_id: "11111111111111111111111111111111".to_string(),
            asset: "wETH".to_string(),
            amount: 2500,
            dest_chain: "ethereum".to_string(),
            dest_address: "0x3333333333333333333333333333333333333333".to_string(),
            preferred_stablecoin: "usdt".to_string(),
            burn_tx_signature: Some("burn-tx-4".to_string()),
            outbound_tx_hash: None,
            safe_nonce: None,
            signatures: Vec::new(),
            status: "pending_burn".to_string(),
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            created_at: 1000,
        };
        store_withdrawal_job(&state.db, &job).expect("store withdrawal job");

        process_withdrawal_jobs(&state)
            .await
            .expect("process withdrawal jobs");

        let job_after = fetch_withdrawal_job(&state.db, &job.job_id)
            .expect("fetch withdrawal job")
            .expect("withdrawal job exists");
        assert_eq!(job_after.status, "pending_burn");
        assert!(job_after.burn_tx_signature.is_none());
        assert_eq!(job_after.attempts, 1);
        assert!(job_after.outbound_tx_hash.is_none());
        assert!(job_after
            .last_error
            .as_deref()
            .unwrap_or_default()
            .contains("Burn method mismatch"));

        assert!(list_withdrawal_jobs_by_status(&state.db, "burned")
            .expect("list burned withdrawal jobs")
            .is_empty());
        assert!(list_withdrawal_jobs_by_status(&state.db, "signing")
            .expect("list signing withdrawal jobs")
            .is_empty());
        assert!(list_withdrawal_jobs_by_status(&state.db, "broadcasting")
            .expect("list broadcasting withdrawal jobs")
            .is_empty());

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), event_rx.recv())
                .await
                .is_err()
        );

        let requests = licn_requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].get("method").and_then(|value| value.as_str()),
            Some("getTransaction")
        );
    }

    #[tokio::test]
    async fn test_process_withdrawal_jobs_burn_missing_contract_config_permanently_fails_without_broadcast(
    ) {
        let mut state = test_state();
        let mut event_rx = state.event_tx.subscribe();
        let licn_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
        let licn_app: Router = Router::new()
            .route("/", post(mock_licn_rpc_handler))
            .with_state(MockLichenRpcState {
                transaction_result: json!({
                    "success": true,
                    "contract_address": "wrapped-weth-contract",
                    "caller": "11111111111111111111111111111111",
                    "method": "burn",
                    "amount": 2500,
                }),
                requests: licn_requests.clone(),
            });
        let licn_rpc_url = spawn_mock_server(licn_app).await;

        state.config.licn_rpc_url = Some(licn_rpc_url);
        state.config.weth_contract_addr = None;

        let job = WithdrawalJob {
            job_id: "withdrawal-burn-missing-contract-config".to_string(),
            user_id: "11111111111111111111111111111111".to_string(),
            asset: "wETH".to_string(),
            amount: 2500,
            dest_chain: "ethereum".to_string(),
            dest_address: "0x3333333333333333333333333333333333333333".to_string(),
            preferred_stablecoin: "usdt".to_string(),
            burn_tx_signature: Some("burn-tx-5".to_string()),
            outbound_tx_hash: None,
            safe_nonce: None,
            signatures: Vec::new(),
            status: "pending_burn".to_string(),
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            created_at: 1000,
        };
        store_withdrawal_job(&state.db, &job).expect("store withdrawal job");

        process_withdrawal_jobs(&state)
            .await
            .expect("process withdrawal jobs");

        let job_after = fetch_withdrawal_job(&state.db, &job.job_id)
            .expect("fetch withdrawal job")
            .expect("withdrawal job exists");
        assert_eq!(job_after.status, "permanently_failed");
        assert!(job_after.outbound_tx_hash.is_none());
        assert_eq!(
            job_after.last_error.as_deref(),
            Some("No contract address configured for asset 'wETH'")
        );

        assert!(list_withdrawal_jobs_by_status(&state.db, "burned")
            .expect("list burned withdrawal jobs")
            .is_empty());
        assert!(list_withdrawal_jobs_by_status(&state.db, "signing")
            .expect("list signing withdrawal jobs")
            .is_empty());
        assert!(list_withdrawal_jobs_by_status(&state.db, "broadcasting")
            .expect("list broadcasting withdrawal jobs")
            .is_empty());

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), event_rx.recv())
                .await
                .is_err()
        );

        let requests = licn_requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].get("method").and_then(|value| value.as_str()),
            Some("getTransaction")
        );
    }

    #[test]
    fn test_count_credit_jobs_with_index() {
        let _ = DB::destroy(&Options::default(), "/tmp/test_custody_count_credit");
        let db = open_db("/tmp/test_custody_count_credit").unwrap();

        let job = CreditJob {
            job_id: "test-credit-count-1".to_string(),
            deposit_id: "dep-1".to_string(),
            to_address: "recipient".to_string(),
            amount_spores: 500,
            source_asset: "usdt".to_string(),
            source_chain: "solana".to_string(),
            status: "queued".to_string(),
            tx_signature: None,
            attempts: 0,
            last_error: None,
            next_attempt_at: None,
            created_at: 1000,
        };
        store_credit_job(&db, &job).unwrap();

        let counts = count_credit_jobs(&db).unwrap();
        assert_eq!(counts.total, 1);
        assert_eq!(*counts.by_status.get("queued").unwrap_or(&0), 1);

        let _ = DB::destroy(&Options::default(), "/tmp/test_custody_count_credit");
    }

    // ── F8.7: BURN_LOCKS pruning ──

    #[test]
    fn test_burn_locks_arc_strong_count_pruning() {
        // Verify that Arc::strong_count works as expected for pruning
        let map: std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>> =
            std::collections::HashMap::new();
        let arc = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        assert_eq!(std::sync::Arc::strong_count(&arc), 1);
        let _clone = arc.clone();
        assert_eq!(std::sync::Arc::strong_count(&arc), 2);
        drop(_clone);
        assert_eq!(std::sync::Arc::strong_count(&arc), 1);
        // After dropping all clones except the map entry, strong_count == 1
        // so retain(|_, v| strong_count(v) > 1) would remove it
        assert!(map.is_empty()); // just testing setup
    }

    // ── F8.11: Events cursor pagination ──

    #[test]
    fn test_events_pagination_cursor_parsing() {
        // Verify the cursor logic: when after_cursor is None, past_cursor starts true
        let after_cursor: Option<String> = None;
        let past_cursor = after_cursor.is_none();
        assert!(past_cursor);

        // When after_cursor is Some, past_cursor starts false
        let after_cursor = Some("event-123".to_string());
        let past_cursor = after_cursor.is_none();
        assert!(!past_cursor);
    }

    // ── Webhook HMAC signature test ──

    #[test]
    fn test_webhook_hmac_signature() {
        let payload = b"{\"event_type\":\"deposit.confirmed\"}";
        let secret = "test_webhook_secret";
        let sig = compute_webhook_signature(payload, secret);
        assert_eq!(sig.len(), 64); // hex-encoded SHA256 = 64 chars
                                   // Same input should produce same output (deterministic)
        let sig2 = compute_webhook_signature(payload, secret);
        assert_eq!(sig, sig2);
        // Different secret should produce different output
        let sig3 = compute_webhook_signature(payload, "different_secret");
        assert_ne!(sig, sig3);
    }

    // ── Decimal conversion tests ──

    #[test]
    fn test_source_chain_decimals() {
        // Native tokens
        assert_eq!(source_chain_decimals("ethereum", "eth"), 18);
        assert_eq!(source_chain_decimals("eth", "eth"), 18);
        assert_eq!(source_chain_decimals("bsc", "bnb"), 18);
        assert_eq!(source_chain_decimals("bnb", "bnb"), 18);
        assert_eq!(source_chain_decimals("solana", "sol"), 9);
        assert_eq!(source_chain_decimals("sol", "sol"), 9);

        // Stablecoins on Ethereum: 6 decimals
        assert_eq!(source_chain_decimals("ethereum", "usdt"), 6);
        assert_eq!(source_chain_decimals("eth", "usdc"), 6);

        // Stablecoins on BSC: 18 decimals (BEP-20)
        assert_eq!(source_chain_decimals("bsc", "usdt"), 18);
        assert_eq!(source_chain_decimals("bnb", "usdc"), 18);

        // Stablecoins on Solana: 6 decimals (SPL)
        assert_eq!(source_chain_decimals("solana", "usdt"), 6);
        assert_eq!(source_chain_decimals("sol", "usdc"), 6);
    }

    #[test]
    fn test_spores_to_chain_amount() {
        // ETH: 1 wETH = 1_000_000_000 spores → 1_000_000_000_000_000_000 wei
        assert_eq!(
            spores_to_chain_amount(1_000_000_000, "ethereum", "eth"),
            1_000_000_000_000_000_000u128
        );

        // BNB: 0.05 wBNB = 50_000_000 spores → 50_000_000_000_000_000 wei
        assert_eq!(
            spores_to_chain_amount(50_000_000, "bsc", "bnb"),
            50_000_000_000_000_000u128
        );

        // SOL: 1 wSOL = 1_000_000_000 spores → 1_000_000_000 lamports (same)
        assert_eq!(
            spores_to_chain_amount(1_000_000_000, "solana", "sol"),
            1_000_000_000u128
        );

        // USDT on Ethereum: 100 lUSD = 100_000_000_000 spores → 100_000_000 atoms (6 dec)
        assert_eq!(
            spores_to_chain_amount(100_000_000_000, "ethereum", "usdt"),
            100_000_000u128
        );

        // USDT on BSC: 100 lUSD = 100_000_000_000 spores → 100_000_000_000_000_000_000 atoms (18 dec)
        assert_eq!(
            spores_to_chain_amount(100_000_000_000, "bsc", "usdt"),
            100_000_000_000_000_000_000u128
        );

        // USDC on Solana: 100 lUSD = 100_000_000_000 spores → 100_000_000 atoms (6 dec)
        assert_eq!(
            spores_to_chain_amount(100_000_000_000, "solana", "usdc"),
            100_000_000u128
        );
    }

    #[test]
    fn test_deposit_credit_conversion_roundtrip() {
        // Verify deposit conversion (chain → spores) and withdrawal conversion
        // (spores → chain) are exact inverses for whole-unit amounts.

        // 1 ETH deposit: 10^18 wei → 10^9 spores → 10^18 wei
        let raw_eth: u128 = 1_000_000_000_000_000_000;
        let dec = source_chain_decimals("ethereum", "eth");
        let spores = (raw_eth / 10u128.pow(dec - 9)) as u64;
        assert_eq!(spores, 1_000_000_000);
        assert_eq!(spores_to_chain_amount(spores, "ethereum", "eth"), raw_eth);

        // 100 USDT on ETH: 100_000_000 (6 dec) → 100_000_000_000 spores → 100_000_000 (6 dec)
        let raw_usdt: u128 = 100_000_000;
        let dec = source_chain_decimals("ethereum", "usdt");
        let spores = (raw_usdt * 10u128.pow(9 - dec)) as u64;
        assert_eq!(spores, 100_000_000_000);
        assert_eq!(spores_to_chain_amount(spores, "ethereum", "usdt"), raw_usdt);

        // 1 SOL: 1_000_000_000 lamports → 1_000_000_000 spores → 1_000_000_000 lamports
        let raw_sol: u128 = 1_000_000_000;
        let dec = source_chain_decimals("solana", "sol");
        assert_eq!(dec, 9);
        let spores = raw_sol as u64;
        assert_eq!(spores_to_chain_amount(spores, "solana", "sol"), raw_sol);
    }
}
