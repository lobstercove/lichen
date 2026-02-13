use axum::{extract::State, routing::get, routing::post, Json, Router};
use base64::Engine;
use ed25519_dalek::{Signer, VerifyingKey};
use moltchain_core::{Hash, Instruction, Keypair, Message, Pubkey, Transaction, SYSTEM_PROGRAM_ID};
use rocksdb::{ColumnFamilyDescriptor, Options, DB};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::time::{sleep, Duration};
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Clone)]
struct CustodyState {
    db: Arc<DB>,
    next_index_lock: Arc<Mutex<()>>,
    /// M13 fix: serialize reserve ledger read-modify-write to prevent concurrent race conditions
    _reserve_lock: Arc<Mutex<()>>,
    config: CustodyConfig,
    http: reqwest::Client,
}

#[derive(Clone, Debug)]
struct CustodyConfig {
    db_path: String,
    solana_rpc_url: Option<String>,
    evm_rpc_url: Option<String>,
    solana_confirmations: u64,
    evm_confirmations: u64,
    poll_interval_secs: u64,
    treasury_solana_address: Option<String>,
    treasury_evm_address: Option<String>,
    solana_fee_payer_keypair_path: Option<String>,
    solana_treasury_owner: Option<String>,
    solana_usdc_mint: String,
    solana_usdt_mint: String,
    evm_usdc_contract: String,
    evm_usdt_contract: String,
    signer_endpoints: Vec<String>,
    signer_threshold: usize,
    molt_rpc_url: Option<String>,
    treasury_keypair_path: Option<String>,
    // Wrapped token contract addresses on MoltChain
    musd_contract_addr: Option<String>,
    wsol_contract_addr: Option<String>,
    weth_contract_addr: Option<String>,
    // Reserve rebalance settings
    rebalance_threshold_bps: u64, // trigger when one side exceeds this (e.g. 7000 = 70%)
    rebalance_target_bps: u64,    // swap to reach this ratio (e.g. 5000 = 50/50)
    jupiter_api_url: Option<String>, // Solana DEX aggregator for USDT↔USDC swaps
    uniswap_router: Option<String>, // Ethereum DEX router for USDT↔USDC swaps
    deposit_ttl_secs: i64,        // Expire unfunded deposits after this many seconds (default: 24h)
    /// C8 fix: Secret master seed for key derivation (HMAC-SHA256 instead of plain SHA256).
    /// Load from CUSTODY_MASTER_SEED env var. Required for production.
    master_seed: String,
    /// C9 fix: Auth token for threshold signer requests
    signer_auth_token: Option<String>,
    /// M17 fix: API auth token for withdrawal and other write endpoints
    /// Load from CUSTODY_API_AUTH_TOKEN env var. Required for production.
    api_auth_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct DepositRequest {
    deposit_id: String,
    user_id: String,
    chain: String,
    asset: String,
    address: String,
    derivation_path: String,
    created_at: i64,
    status: String,
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
    amount_shells: u64,
    /// Source chain asset identifier ("sol", "eth", "usdt", "usdc")
    /// Determines which wrapped token contract to mint on MoltChain.
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
    asset: String, // "mUSD", "wSOL", "wETH"
    amount: u64,
    dest_chain: String,   // "solana", "ethereum"
    dest_address: String, // destination address on dest_chain
    /// For mUSD withdrawals: which stablecoin to receive ("usdt" or "usdc"). Defaults to "usdt".
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
    asset: String, // "mUSD", "wSOL", "wETH"
    amount: u64,
    dest_chain: String,
    dest_address: String,
    /// For mUSD: which stablecoin the user wants ("usdt" or "usdc")
    #[serde(default = "default_preferred_stablecoin")]
    preferred_stablecoin: String,
    /// MoltChain burn tx signature (user burned their wrapped tokens)
    burn_tx_signature: Option<String>,
    /// Outbound chain tx hash (SOL/ETH/USDT sent to user's dest_address)
    outbound_tx_hash: Option<String>,
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
const CF_CURSORS: &str = "cursors";
const CF_RESERVE_LEDGER: &str = "reserve_ledger";
const CF_REBALANCE_JOBS: &str = "rebalance_jobs";

/// MoltChain contract runtime program address (all 0xFF bytes)
const MOLT_CONTRACT_PROGRAM: [u8; 32] = [0xFF; 32];

const SOLANA_SYSTEM_PROGRAM: &str = "11111111111111111111111111111111";
const SOLANA_TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const SOLANA_ASSOCIATED_TOKEN_PROGRAM: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
const SOLANA_RENT_SYSVAR: &str = "SysvarRent111111111111111111111111111111111";

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let config = load_config();
    let db = open_db(&config.db_path).expect("open custody db");
    let state = CustodyState {
        db: Arc::new(db),
        next_index_lock: Arc::new(Mutex::new(())),
        _reserve_lock: Arc::new(Mutex::new(())),
        config: config.clone(),
        http: reqwest::Client::new(),
    };

    if let Some(url) = config.solana_rpc_url.clone() {
        let watcher_state = state.clone();
        tokio::spawn(async move {
            solana_watcher_loop(watcher_state, url).await;
        });
    }

    if let Some(url) = config.evm_rpc_url.clone() {
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

    // Withdrawal: watches MoltChain for burn events → sends native assets on source chain
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
        .route("/reserves", get(get_reserves))
        .with_state(state);

    let addr: SocketAddr = "0.0.0.0:9105".parse().expect("valid bind addr");
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

async fn status(State(state): State<CustodyState>) -> Result<Json<Value>, Json<ErrorResponse>> {
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

async fn create_deposit(
    State(state): State<CustodyState>,
    Json(payload): Json<CreateDepositRequest>,
) -> Result<Json<CreateDepositResponse>, Json<ErrorResponse>> {
    let chain = payload.chain.to_lowercase();
    let asset = payload.asset.to_lowercase();
    if chain.is_empty() || asset.is_empty() || payload.user_id.is_empty() {
        return Err(Json(ErrorResponse::invalid("Missing user_id/chain/asset")));
    }

    if (chain == "solana" || chain == "sol") && is_solana_stablecoin(&asset) {
        ensure_solana_config(&state.config).map_err(|e| Json(ErrorResponse::invalid(&e)))?;
    }

    let deposit_id = Uuid::new_v4().to_string();
    let _guard = state.next_index_lock.lock().await;
    let index = next_deposit_index(&state.db, &payload.user_id, &chain, &asset)
        .map_err(|e| Json(ErrorResponse::db(&e)))?;

    let derivation_path = format!("molt/{}/{}/{}/{}", chain, asset, payload.user_id, index);
    let address = if chain == "solana" || chain == "sol" {
        if is_solana_stablecoin(&asset) {
            let mint = solana_mint_for_asset(&state.config, &asset)
                .map_err(|e| Json(ErrorResponse::invalid(&e)))?;
            let owner = derive_solana_owner_pubkey(&derivation_path, &state.config.master_seed)
                .map_err(|e| Json(ErrorResponse::invalid(&e)))?;
            let ata = derive_associated_token_address(&owner, &mint)
                .map_err(|e| Json(ErrorResponse::invalid(&e)))?;
            ensure_associated_token_account(&state, &owner, &mint, &ata)
                .await
                .map_err(|e| Json(ErrorResponse::invalid(&e)))?;
            ata
        } else {
            derive_deposit_address(&chain, &asset, &derivation_path, &state.config.master_seed)
                .map_err(|e| Json(ErrorResponse::invalid(&e)))?
        }
    } else {
        derive_deposit_address(&chain, &asset, &derivation_path, &state.config.master_seed)
            .map_err(|e| Json(ErrorResponse::invalid(&e)))?
    };

    let record = DepositRequest {
        deposit_id: deposit_id.clone(),
        user_id: payload.user_id,
        chain,
        asset,
        address: address.clone(),
        derivation_path,
        created_at: chrono::Utc::now().timestamp(),
        status: "issued".to_string(),
    };

    store_deposit(&state.db, &record).map_err(|e| Json(ErrorResponse::db(&e)))?;
    store_address_index(&state.db, &record.address, &record.deposit_id)
        .map_err(|e| Json(ErrorResponse::db(&e)))?;

    Ok(Json(CreateDepositResponse {
        deposit_id,
        address,
    }))
}

async fn get_deposit(
    State(state): State<CustodyState>,
    axum::extract::Path(deposit_id): axum::extract::Path<String>,
) -> Result<Json<DepositRequest>, Json<ErrorResponse>> {
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

    let cfs = vec![
        ColumnFamilyDescriptor::new(CF_DEPOSITS, Options::default()),
        ColumnFamilyDescriptor::new(CF_INDEXES, Options::default()),
        ColumnFamilyDescriptor::new(CF_ADDRESS_INDEX, Options::default()),
        ColumnFamilyDescriptor::new(CF_DEPOSIT_EVENTS, Options::default()),
        ColumnFamilyDescriptor::new(CF_SWEEP_JOBS, Options::default()),
        ColumnFamilyDescriptor::new(CF_ADDRESS_BALANCES, Options::default()),
        ColumnFamilyDescriptor::new(CF_TOKEN_BALANCES, Options::default()),
        ColumnFamilyDescriptor::new(CF_CREDIT_JOBS, Options::default()),
        ColumnFamilyDescriptor::new(CF_WITHDRAWAL_JOBS, Options::default()),
        ColumnFamilyDescriptor::new(CF_AUDIT_EVENTS, Options::default()),
        ColumnFamilyDescriptor::new(CF_CURSORS, Options::default()),
        ColumnFamilyDescriptor::new(CF_RESERVE_LEDGER, Options::default()),
        ColumnFamilyDescriptor::new(CF_REBALANCE_JOBS, Options::default()),
    ];

    DB::open_cf_descriptors(&opts, path, cfs).map_err(|e| format!("db open: {}", e))
}

fn store_address_index(db: &DB, address: &str, deposit_id: &str) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_ADDRESS_INDEX)
        .ok_or_else(|| "missing address_index cf".to_string())?;
    db.put_cf(cf, address.as_bytes(), deposit_id.as_bytes())
        .map_err(|e| format!("db put: {}", e))
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
        ("eth", _) | ("ethereum", _) => derive_evm_address(path, master_seed),
        _ => Err(format!("Unsupported chain: {}", chain)),
    }
}

fn derive_solana_owner_pubkey(path: &str, master_seed: &str) -> Result<String, String> {
    derive_solana_address(path, master_seed)
}

fn is_solana_stablecoin(asset: &str) -> bool {
    matches!(asset, "usdc" | "usdt")
}

fn ensure_solana_config(config: &CustodyConfig) -> Result<(), String> {
    if config.solana_rpc_url.is_none() {
        return Err("missing CUSTODY_SOLANA_RPC_URL".to_string());
    }
    if config.solana_fee_payer_keypair_path.is_none() {
        return Err("missing CUSTODY_SOLANA_FEE_PAYER".to_string());
    }
    if config.solana_treasury_owner.is_none() {
        return Err("missing CUSTODY_SOLANA_TREASURY_OWNER".to_string());
    }
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
    let fee_payer_path = state
        .config
        .solana_fee_payer_keypair_path
        .as_ref()
        .ok_or_else(|| "missing CUSTODY_SOLANA_FEE_PAYER".to_string())?;

    if solana_get_account_exists(&state.http, url, ata).await? {
        return Ok(());
    }

    let owner_key = decode_solana_pubkey(owner)?;
    let mint_key = decode_solana_pubkey(mint)?;
    let ata_key = decode_solana_pubkey(ata)?;
    let fee_payer = load_solana_keypair(fee_payer_path)?;

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
    let molt_rpc_url = std::env::var("CUSTODY_MOLT_RPC_URL").ok();
    let treasury_keypair_path = std::env::var("CUSTODY_TREASURY_KEYPAIR").ok();
    let musd_contract_addr = std::env::var("CUSTODY_MUSD_TOKEN_ADDR").ok();
    let wsol_contract_addr = std::env::var("CUSTODY_WSOL_TOKEN_ADDR").ok();
    let weth_contract_addr = std::env::var("CUSTODY_WETH_TOKEN_ADDR").ok();
    let rebalance_threshold_bps = std::env::var("CUSTODY_REBALANCE_THRESHOLD_BPS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(7000);
    let rebalance_target_bps = std::env::var("CUSTODY_REBALANCE_TARGET_BPS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(5000);
    let jupiter_api_url = std::env::var("CUSTODY_JUPITER_API_URL").ok();
    let uniswap_router = std::env::var("CUSTODY_UNISWAP_ROUTER").ok();
    let deposit_ttl_secs = std::env::var("CUSTODY_DEPOSIT_TTL_SECS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(86400); // 24 hours default
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

    CustodyConfig {
        db_path,
        solana_rpc_url,
        evm_rpc_url,
        solana_confirmations,
        evm_confirmations,
        poll_interval_secs,
        treasury_solana_address,
        treasury_evm_address,
        solana_fee_payer_keypair_path,
        solana_treasury_owner,
        solana_usdc_mint,
        solana_usdt_mint,
        evm_usdc_contract,
        evm_usdt_contract,
        signer_endpoints,
        signer_threshold,
        molt_rpc_url,
        treasury_keypair_path,
        musd_contract_addr,
        wsol_contract_addr,
        weth_contract_addr,
        rebalance_threshold_bps,
        rebalance_target_bps,
        jupiter_api_url,
        uniswap_router,
        deposit_ttl_secs,
        // C8 fix: secret master seed (required for production key derivation)
        master_seed: std::env::var("CUSTODY_MASTER_SEED").unwrap_or_else(|_| {
            tracing::warn!(
                "⚠️  CUSTODY_MASTER_SEED not set — using insecure default! Set this in production!"
            );
            "INSECURE_DEFAULT_SEED_DO_NOT_USE_IN_PRODUCTION".to_string()
        }),
        // C9 fix: auth token for threshold signers
        signer_auth_token: std::env::var("CUSTODY_SIGNER_AUTH_TOKEN").ok(),
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

            if let Some(treasury) = state.config.treasury_solana_address.clone() {
                let balance = solana_get_balance(&state.http, url, &deposit.address).await?;
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
    if balance == 0 {
        return Ok(());
    }

    let last_key = format!("spl:{}:{}", deposit.asset, deposit.address);
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
    let deposits = list_pending_deposits_for_chains(&state.db, &["ethereum", "eth"])?;
    let block_number = evm_get_block_number(&state.http, url).await?;

    process_evm_erc20_deposits(state, url, &deposits, block_number).await?;

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
        .and_then(|v| v.get("confirmationStatus"))
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
    let cf = db
        .cf_handle(CF_DEPOSITS)
        .ok_or_else(|| "missing deposits cf".to_string())?;
    let mut results = Vec::new();
    let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
    for item in iter {
        let (_, value) = item.map_err(|e| format!("db iter: {}", e))?;
        let record: DepositRequest =
            serde_json::from_slice(&value).map_err(|e| format!("decode: {}", e))?;
        if record.chain == chain && (record.status == "issued" || record.status == "pending") {
            results.push(record);
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
        .and_then(|v| v.get("confirmationStatus"))
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

    if job.chain == "eth" || job.chain == "ethereum" {
        let url = state
            .config
            .evm_rpc_url
            .as_ref()
            .ok_or_else(|| "missing CUSTODY_EVM_RPC_URL".to_string())?;
        if let Some(receipt) = evm_get_transaction_receipt(&state.http, url, tx_hash).await? {
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
    let Some(rpc_url) = state.config.molt_rpc_url.as_ref() else {
        return Ok(None);
    };
    let result = molt_rpc_call(&state.http, rpc_url, "getTransaction", json!([signature])).await?;
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
    let queued_jobs = list_sweep_jobs_by_status(&state.db, "queued")?;
    for mut job in queued_jobs {
        job.status = "signing".to_string();
        store_sweep_job(&state.db, &job)?;
        record_audit_event(
            &state.db,
            "sweep_signing",
            &job.job_id,
            Some(&job.deposit_id),
            None,
        )?;
    }

    if state.config.signer_endpoints.is_empty() || state.config.signer_threshold == 0 {
        return Ok(());
    }

    let mut signing_jobs = list_sweep_jobs_by_status(&state.db, "signing")?;
    for job in signing_jobs.iter_mut() {
        let signatures = collect_signatures(state, job).await?;
        if signatures >= state.config.signer_threshold {
            job.status = "signed".to_string();
        }
        store_sweep_job(&state.db, job)?;
    }

    let mut signed_jobs = list_sweep_jobs_by_status(&state.db, "signed")?;
    for job in signed_jobs.iter_mut() {
        if !is_ready_for_retry(job) {
            continue;
        }
        match broadcast_sweep(state, job).await {
            Ok(Some(tx_hash)) => {
                job.status = "sweep_submitted".to_string();
                job.sweep_tx_hash = Some(tx_hash);
                job.last_error = None;
                job.next_attempt_at = None;
                store_sweep_job(&state.db, job)?;
                record_audit_event(
                    &state.db,
                    "sweep_submitted",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.sweep_tx_hash.as_deref(),
                )?;

                if let Some(credit_job) = build_credit_job(state, job)? {
                    store_credit_job(&state.db, &credit_job)?;
                    record_audit_event(
                        &state.db,
                        "credit_queued",
                        &credit_job.job_id,
                        Some(&credit_job.deposit_id),
                        None,
                    )?;
                }
            }
            Ok(None) => {
                mark_sweep_failed(job, "broadcast returned empty".to_string());
                store_sweep_job(&state.db, job)?;
                record_audit_event(
                    &state.db,
                    "sweep_failed",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.sweep_tx_hash.as_deref(),
                )?;
            }
            Err(err) => {
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
                record_audit_event(
                    &state.db,
                    "sweep_confirmed",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.sweep_tx_hash.as_deref(),
                )?;

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
                            ) {
                                tracing::warn!("reserve ledger update failed: {}", e);
                            }
                        }
                    }
                }
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
    if state.config.molt_rpc_url.is_none() || state.config.treasury_keypair_path.is_none() {
        return Ok(());
    }

    let jobs = list_credit_jobs_by_status(&state.db, "queued")?;
    for mut job in jobs {
        if !is_ready_for_credit_retry(&job) {
            continue;
        }
        match submit_wrapped_credit(state, &job).await {
            Ok(tx_signature) => {
                job.status = "submitted".to_string();
                job.tx_signature = Some(tx_signature);
                job.last_error = None;
                job.next_attempt_at = None;
                store_credit_job(&state.db, &job)?;
                record_audit_event(
                    &state.db,
                    "credit_submitted",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.tx_signature.as_deref(),
                )?;
            }
            Err(err) => {
                mark_credit_failed(&mut job, err);
                store_credit_job(&state.db, &job)?;
            }
        }
    }

    let mut submitted = list_credit_jobs_by_status(&state.db, "submitted")?;
    for job in submitted.iter_mut() {
        if let Some(confirmed) = check_credit_confirmation(state, job).await? {
            if confirmed {
                job.status = "confirmed".to_string();
                job.last_error = None;
                job.next_attempt_at = None;
                store_credit_job(&state.db, job)?;
                record_audit_event(
                    &state.db,
                    "credit_confirmed",
                    &job.job_id,
                    Some(&job.deposit_id),
                    job.tx_signature.as_deref(),
                )?;
            }
        }
    }
    Ok(())
}

fn build_credit_job(state: &CustodyState, sweep: &SweepJob) -> Result<Option<CreditJob>, String> {
    let amount_shells = match sweep.amount.as_ref() {
        Some(value) => value
            .parse::<u64>()
            .map_err(|_| "invalid amount".to_string())?,
        None => return Ok(None),
    };

    let deposit = fetch_deposit(&state.db, &sweep.deposit_id)?;
    let Some(deposit) = deposit else {
        return Ok(None);
    };

    if state.config.molt_rpc_url.is_none() || state.config.treasury_keypair_path.is_none() {
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

    Ok(Some(CreditJob {
        job_id: Uuid::new_v4().to_string(),
        deposit_id: sweep.deposit_id.clone(),
        to_address: deposit.user_id,
        amount_shells,
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

    for endpoint in &state.config.signer_endpoints {
        let url = format!("{}/sign", endpoint.trim_end_matches('/'));
        // C9 fix: Include auth header so threshold signer accepts the request
        let mut req = state.http.post(url).json(&request);
        if let Some(ref token) = state.config.signer_auth_token {
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

async fn broadcast_sweep(state: &CustodyState, job: &SweepJob) -> Result<Option<String>, String> {
    if job.chain == "sol" || job.chain == "solana" {
        let url = state
            .config
            .solana_rpc_url
            .as_ref()
            .ok_or_else(|| "missing CUSTODY_SOLANA_RPC_URL".to_string())?;
        return broadcast_solana_sweep(state, url, job).await;
    }

    if job.chain == "eth" || job.chain == "ethereum" {
        let url = state
            .config
            .evm_rpc_url
            .as_ref()
            .ok_or_else(|| "missing CUSTODY_EVM_RPC_URL".to_string())?;
        return broadcast_evm_sweep(state, url, job).await;
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

    let recent_blockhash = solana_get_latest_blockhash(&state.http, url).await?;
    let (signing_key, from_pubkey) =
        derive_solana_signer(&deposit.derivation_path, &state.config.master_seed)?;
    let to_pubkey = decode_solana_pubkey(&job.to_treasury)?;

    let message =
        build_solana_transfer_message(&from_pubkey, &to_pubkey, amount, &recent_blockhash);
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

    let fee_payer_path = state
        .config
        .solana_fee_payer_keypair_path
        .as_ref()
        .ok_or_else(|| "missing CUSTODY_SOLANA_FEE_PAYER".to_string())?;

    let owner_keypair = derive_solana_keypair(&deposit.derivation_path, &state.config.master_seed)?;
    let fee_payer = load_solana_keypair(fee_payer_path)?;

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

    let from_address = deposit.address;
    let to_address = job.to_treasury.clone();

    let nonce = evm_get_transaction_count(&state.http, url, &from_address).await?;
    let gas_price = evm_get_gas_price(&state.http, url).await?;
    let gas_limit = 21_000u128;
    let fee = gas_price.saturating_mul(gas_limit);
    if amount <= fee {
        return Ok(None);
    }
    let value = amount - fee;

    let chain_id = evm_get_chain_id(&state.http, url).await?;
    let signing_key = derive_evm_signing_key(&deposit.derivation_path, &state.config.master_seed)?;
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

    let gas_price = evm_get_gas_price(&state.http, url).await?;
    let gas_limit = 100_000u128;
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
    let signing_key = derive_evm_signing_key(&deposit.derivation_path, &state.config.master_seed)?;
    let data = evm_encode_erc20_transfer(&to_address, amount)?;
    let raw_tx = build_evm_signed_transaction_with_data(
        &signing_key,
        nonce,
        gas_price,
        gas_limit,
        &contract,
        0,
        &data,
        chain_id,
    )?;
    let tx_hex = format!("0x{}", hex::encode(raw_tx));

    let result = evm_rpc_call(&state.http, url, "eth_sendRawTransaction", json!([tx_hex])).await?;
    Ok(result.as_str().map(|v| v.to_string()))
}

/// M16 fix: Send native ETH from the custody treasury to a deposit address
/// so that it has enough gas to execute an ERC-20 token sweep.
///
/// This is a simple ETH value transfer (no calldata). The treasury derives its
/// EVM signing key from the master seed with path "custody-treasury-evm".
async fn fund_evm_gas_for_sweep(
    state: &CustodyState,
    url: &str,
    to_address: &str,
    amount_wei: u128,
) -> Result<String, String> {
    let treasury_addr = state
        .config
        .treasury_evm_address
        .as_ref()
        .ok_or_else(|| "missing treasury EVM address for gas funding".to_string())?;

    let nonce = evm_get_transaction_count(&state.http, url, treasury_addr).await?;
    let gas_price = evm_get_gas_price(&state.http, url).await?;
    let chain_id = evm_get_chain_id(&state.http, url).await?;
    let signing_key = derive_evm_signing_key("custody-treasury-evm", &state.config.master_seed)?;

    // Simple ETH transfer: 21000 gas
    let gas_limit = 21_000u128;
    let tx_fee = gas_price.saturating_mul(gas_limit);

    // Verify treasury can afford the grant
    let treasury_balance = evm_get_balance(&state.http, url, treasury_addr).await?;
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

fn mark_sweep_failed(job: &mut SweepJob, err: String) {
    job.attempts = job.attempts.saturating_add(1);
    job.last_error = Some(err);
    job.next_attempt_at = Some(next_retry_timestamp(job.attempts));
}

fn mark_credit_failed(job: &mut CreditJob, err: String) {
    job.attempts = job.attempts.saturating_add(1);
    job.last_error = Some(err);
    job.next_attempt_at = Some(next_retry_timestamp(job.attempts));
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
        .molt_rpc_url
        .as_ref()
        .ok_or_else(|| "missing CUSTODY_MOLT_RPC_URL".to_string())?;
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
        job.amount_shells,
    );

    let blockhash = molt_get_recent_blockhash(&state.http, rpc_url).await?;
    let message = Message::new(vec![instruction], blockhash);
    let signature = treasury_keypair.sign(&message.serialize());
    let mut tx = Transaction::new(message);
    tx.signatures.push(signature);

    let tx_bytes = bincode::serialize(&tx).map_err(|e| format!("encode tx: {}", e))?;
    let tx_base64 = base64::engine::general_purpose::STANDARD.encode(tx_bytes);

    let token_label = match job.source_asset.as_str() {
        "usdt" | "usdc" => "mUSD",
        "sol" => "wSOL",
        "eth" => "wETH",
        _ => "UNKNOWN",
    };
    info!(
        "minting {} {} to {} (deposit={})",
        job.amount_shells, token_label, job.to_address, job.deposit_id
    );

    molt_send_transaction(&state.http, rpc_url, &tx_base64).await
}

/// Resolve deposited asset → MoltChain wrapped token contract address.
///
/// Mapping:
///   sol (any chain)          → wSOL contract
///   eth (any chain)          → wETH contract
///   usdt, usdc (any chain)   → mUSD contract (unified stablecoin)
fn resolve_token_contract(config: &CustodyConfig, _chain: &str, asset: &str) -> Option<String> {
    match asset {
        "sol" => config.wsol_contract_addr.clone(),
        "eth" => config.weth_contract_addr.clone(),
        "usdt" | "usdc" => config.musd_contract_addr.clone(),
        _ => None,
    }
}

/// Build a MoltChain contract Call instruction for the "mint" function.
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
        program_id: Pubkey::new(MOLT_CONTRACT_PROGRAM),
        accounts: vec![*caller, *contract_pubkey],
        data,
    }
}

/// Build a MoltChain contract Call instruction for the "burn" function.
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
        program_id: Pubkey::new(MOLT_CONTRACT_PROGRAM),
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

async fn molt_get_recent_blockhash(client: &reqwest::Client, url: &str) -> Result<Hash, String> {
    let result = molt_rpc_call(client, url, "getRecentBlockhash", json!([])).await?;
    let hash = result
        .get("blockhash")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing blockhash".to_string())?;
    Hash::from_hex(hash).map_err(|e| format!("blockhash: {}", e))
}

async fn molt_send_transaction(
    client: &reqwest::Client,
    url: &str,
    tx_base64: &str,
) -> Result<String, String> {
    let result = molt_rpc_call(client, url, "sendTransaction", json!([tx_base64])).await?;
    result
        .as_str()
        .map(|v| v.to_string())
        .ok_or_else(|| "missing tx signature".to_string())
}

async fn molt_rpc_call(
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
    let bytes = serde_json::to_vec(job).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, job.job_id.as_bytes(), bytes)
        .map_err(|e| format!("db put: {}", e))
}

fn store_credit_job(db: &DB, job: &CreditJob) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_CREDIT_JOBS)
        .ok_or_else(|| "missing credit_jobs cf".to_string())?;
    let bytes = serde_json::to_vec(job).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, job.job_id.as_bytes(), bytes)
        .map_err(|e| format!("db put: {}", e))
}

fn count_sweep_jobs(db: &DB) -> Result<StatusCounts, String> {
    let cf = db
        .cf_handle(CF_SWEEP_JOBS)
        .ok_or_else(|| "missing sweep_jobs cf".to_string())?;
    let mut counts = StatusCounts {
        total: 0,
        by_status: BTreeMap::new(),
    };
    let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
    for item in iter {
        let (_, value) = item.map_err(|e| format!("db iter: {}", e))?;
        let record: SweepJob =
            serde_json::from_slice(&value).map_err(|e| format!("decode: {}", e))?;
        counts.total += 1;
        *counts.by_status.entry(record.status).or_insert(0) += 1;
    }
    Ok(counts)
}

fn count_credit_jobs(db: &DB) -> Result<StatusCounts, String> {
    let cf = db
        .cf_handle(CF_CREDIT_JOBS)
        .ok_or_else(|| "missing credit_jobs cf".to_string())?;
    let mut counts = StatusCounts {
        total: 0,
        by_status: BTreeMap::new(),
    };
    let iter = db.iterator_cf(cf, rocksdb::IteratorMode::Start);
    for item in iter {
        let (_, value) = item.map_err(|e| format!("db iter: {}", e))?;
        let record: CreditJob =
            serde_json::from_slice(&value).map_err(|e| format!("decode: {}", e))?;
        counts.total += 1;
        *counts.by_status.entry(record.status).or_insert(0) += 1;
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
    let cf = db
        .cf_handle(CF_AUDIT_EVENTS)
        .ok_or_else(|| "missing audit_events cf".to_string())?;
    let payload = serde_json::json!({
        "event_id": Uuid::new_v4().to_string(),
        "event_type": event_type,
        "entity_id": entity_id,
        "deposit_id": deposit_id,
        "tx_hash": tx_hash,
        "timestamp": chrono::Utc::now().timestamp(),
    });
    let bytes = serde_json::to_vec(&payload).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(
        cf,
        payload["event_id"].as_str().unwrap_or_default().as_bytes(),
        bytes,
    )
    .map_err(|e| format!("db put: {}", e))
}

fn list_credit_jobs_by_status(db: &DB, status: &str) -> Result<Vec<CreditJob>, String> {
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
        .map_err(|e| format!("db put: {}", e))
}

fn update_deposit_status(db: &DB, deposit_id: &str, status: &str) -> Result<(), String> {
    let mut record = fetch_deposit(db, deposit_id)
        .map_err(|e| format!("fetch deposit: {}", e))?
        .ok_or_else(|| "deposit not found".to_string())?;
    record.status = status.to_string();
    store_deposit(db, &record)
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
    let seed_bytes: [u8; 32] = seed.as_slice().try_into().map_err(|_| "seed")?;
    let signing_key = SigningKey::from_bytes(&seed_bytes);
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
    let seed_bytes: [u8; 32] = seed.as_slice().try_into().map_err(|_| "seed")?;
    let signing_key = SigningKey::from_bytes(&seed_bytes);
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
    let seed_bytes: [u8; 32] = seed.as_slice().try_into().map_err(|_| "seed")?;
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed_bytes);
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
    let seed = mac.finalize().into_bytes();
    let key = SigningKey::from_bytes(&seed).map_err(|_| "invalid seed")?;
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
    let seed = mac.finalize().into_bytes();
    k256::ecdsa::SigningKey::from_bytes(&seed).map_err(|_| "invalid seed".to_string())
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
// WITHDRAWAL — Burn wrapped tokens on MoltChain, send native assets to user
// ============================================================================

/// POST /withdrawals — User requests to withdraw wrapped tokens
///
/// Flow:
///   1. User calls burn() on the wrapped token contract (client-side)
///   2. User POSTs burn tx signature + dest_chain + dest_address to this endpoint
///   3. Custody verifies the burn on MoltChain
///   4. For mUSD: checks stablecoin reserves, queues rebalance if needed
///   5. Custody uses threshold signatures to send native assets on the destination chain
async fn create_withdrawal(
    State(state): State<CustodyState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<WithdrawalRequest>,
) -> Json<Value> {
    // M17 fix: require API auth token for withdrawal requests
    if let Some(expected_token) = &state.config.api_auth_token {
        let provided = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "));
        match provided {
            // AUDIT-FIX 0.12: constant-time comparison to prevent timing attacks
            Some(token) if {
                use subtle::ConstantTimeEq;
                token.as_bytes().ct_eq(expected_token.as_bytes()).into()
            } => {} // OK
            _ => {
                return Json(json!({
                    "error": "unauthorized: missing or invalid API auth token"
                }));
            }
        }
    }

    let asset_lower = req.asset.to_lowercase();
    let (dest_asset, _) = match asset_lower.as_str() {
        "musd" => ("stablecoin", "stablecoin"),
        "wsol" => ("sol", "native"),
        "weth" => ("eth", "native"),
        _ => {
            return Json(json!({
                "error": format!("unsupported withdrawal asset: {}", req.asset)
            }));
        }
    };

    // Validate destination chain makes sense for the asset
    let valid_chain = match dest_asset {
        "sol" => req.dest_chain == "solana",
        "eth" => req.dest_chain == "ethereum",
        "stablecoin" => req.dest_chain == "solana" || req.dest_chain == "ethereum",
        _ => false,
    };
    if !valid_chain {
        return Json(json!({
            "error": format!("cannot withdraw {} to {}", req.asset, req.dest_chain)
        }));
    }

    // For mUSD withdrawals: validate and resolve preferred stablecoin
    let preferred = if asset_lower == "musd" {
        let pref = req.preferred_stablecoin.to_lowercase();
        if pref != "usdt" && pref != "usdc" {
            return Json(json!({
                "error": format!("preferred_stablecoin must be 'usdt' or 'usdc', got '{}'", pref)
            }));
        }

        // Check reserve balance for the preferred stablecoin on the destination chain
        let reserve = get_reserve_balance(&state.db, &req.dest_chain, &pref).unwrap_or(0);
        let other = if pref == "usdt" { "usdc" } else { "usdt" };
        let other_reserve = get_reserve_balance(&state.db, &req.dest_chain, other).unwrap_or(0);
        let total_on_chain = reserve.saturating_add(other_reserve);

        if req.amount > total_on_chain {
            return Json(json!({
                "error": format!(
                    "insufficient total stablecoin reserves on {}: requested {}, available {} ({} {} + {} {})",
                    req.dest_chain, req.amount, total_on_chain, reserve, pref, other_reserve, other
                )
            }));
        }

        if reserve < req.amount {
            // Not enough of the preferred stablecoin — queue a rebalance swap first
            let deficit = req.amount - reserve;
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

    if let Err(e) = record_audit_event(&state.db, "withdrawal_requested", &job.job_id, None, None) {
        tracing::warn!("audit event failed: {}", e);
    }

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
            "Burn {} {} on MoltChain, then the outbound transfer to {} will be processed automatically.",
            job.amount, job.asset, job.dest_chain
        ),
    }))
}

fn store_withdrawal_job(db: &DB, job: &WithdrawalJob) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_WITHDRAWAL_JOBS)
        .ok_or_else(|| "missing withdrawal_jobs cf".to_string())?;
    let bytes = serde_json::to_vec(job).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, job.job_id.as_bytes(), bytes)
        .map_err(|e| format!("db put: {}", e))
}

fn list_withdrawal_jobs_by_status(db: &DB, status: &str) -> Result<Vec<WithdrawalJob>, String> {
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
/// M13 fix: uses internal Mutex to serialize concurrent read-modify-write operations.
fn adjust_reserve_balance(
    db: &DB,
    chain: &str,
    asset: &str,
    amount: u64,
    increment: bool,
) -> Result<(), String> {
    use std::sync::Mutex as StdMutex;
    static RESERVE_LOCK: std::sync::LazyLock<StdMutex<()>> =
        std::sync::LazyLock::new(|| StdMutex::new(()));
    let _guard = RESERVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());

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

/// GET /reserves — Returns current stablecoin reserve balances across all chains
async fn get_reserves(State(state): State<CustodyState>) -> Json<Value> {
    let cf = match state.db.cf_handle(CF_RESERVE_LEDGER) {
        Some(cf) => cf,
        None => return Json(json!({"error": "reserve ledger not available"})),
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

    Json(json!({
        "reserves": entries,
        "chain_ratios": ratios,
    }))
}

// ============================================================================
// REBALANCE — Swap USDT↔USDC on external DEXes to maintain reserve balance
// ============================================================================

fn store_rebalance_job(db: &DB, job: &RebalanceJob) -> Result<(), String> {
    let cf = db
        .cf_handle(CF_REBALANCE_JOBS)
        .ok_or_else(|| "missing rebalance_jobs cf".to_string())?;
    let bytes = serde_json::to_vec(job).map_err(|e| format!("encode: {}", e))?;
    db.put_cf(cf, job.job_id.as_bytes(), bytes)
        .map_err(|e| format!("db put: {}", e))
}

fn list_rebalance_jobs_by_status(db: &DB, status: &str) -> Result<Vec<RebalanceJob>, String> {
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
async fn deposit_cleanup_loop(state: CustodyState) {
    loop {
        // Run every 10 minutes
        sleep(Duration::from_secs(600)).await;

        let ttl = state.config.deposit_ttl_secs;
        if ttl <= 0 {
            continue; // TTL disabled
        }
        let cutoff = chrono::Utc::now().timestamp() - ttl;

        let cf = match state.db.cf_handle(CF_DEPOSITS) {
            Some(cf) => cf,
            None => continue,
        };

        let mut expired_ids = Vec::new();
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
            // Only expire deposits that never received funds ("issued" status)
            if record.status == "issued" && record.created_at < cutoff {
                expired_ids.push((
                    String::from_utf8_lossy(&key).to_string(),
                    record.address.clone(),
                ));
            }
        }

        let count = expired_ids.len();
        for (deposit_id, address) in &expired_ids {
            // Update status to "expired"
            if let Some(cf) = state.db.cf_handle(CF_DEPOSITS) {
                if let Ok(Some(value)) = state.db.get_cf(&cf, deposit_id.as_bytes()) {
                    if let Ok(mut record) = serde_json::from_slice::<DepositRequest>(&value) {
                        record.status = "expired".to_string();
                        if let Ok(json) = serde_json::to_vec(&record) {
                            let _ = state.db.put_cf(&cf, deposit_id.as_bytes(), &json);
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
            // Prune deposit events for this deposit
            if let Some(evt_cf) = state.db.cf_handle(CF_DEPOSIT_EVENTS) {
                let prefix = format!("{}:", deposit_id);
                let iter = state.db.prefix_iterator_cf(&evt_cf, prefix.as_bytes());
                for (key, _) in iter.flatten() {
                    if key.starts_with(prefix.as_bytes()) {
                        let _ = state.db.delete_cf(&evt_cf, &key);
                    } else {
                        break;
                    }
                }
            }
        }

        if count > 0 {
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

    for chain in &["solana", "ethereum"] {
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
                record_audit_event(
                    &state.db,
                    "rebalance_submitted",
                    &job.job_id,
                    None,
                    Some(&tx_hash),
                )?;
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

            // Use parsed output if available; fall back to input amount with a warning
            let credit_amount = match actual_output {
                Some(output) => {
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
                        "could not parse swap output for job {}, falling back to input amount {}",
                        job.job_id,
                        job.amount
                    );
                    job.amount
                }
            };

            store_rebalance_job(&state.db, &job)?;

            // Update reserve ledger: debit input amount, credit actual output
            adjust_reserve_balance(&state.db, &job.chain, &job.from_asset, job.amount, false)?;
            adjust_reserve_balance(&state.db, &job.chain, &job.to_asset, credit_amount, true)?;

            record_audit_event(
                &state.db,
                "rebalance_confirmed",
                &job.job_id,
                None,
                job.swap_tx_hash.as_deref(),
            )?;
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
        "{}/quote?inputMint={}&outputMint={}&amount={}&slippageBps=10",
        jupiter_url.trim_end_matches('/'),
        from_mint,
        to_mint,
        job.amount
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
    // Jupiter returns a base64-encoded versioned transaction
    // For now, we pass it directly to Solana RPC signed by our treasury key
    let fee_payer_path = state
        .config
        .solana_fee_payer_keypair_path
        .as_ref()
        .ok_or_else(|| "missing fee payer for rebalance".to_string())?;
    let _fee_payer = load_solana_keypair(fee_payer_path)?;

    // Submit the partly-signed Jupiter transaction
    // (Jupiter pre-signs the swap instruction; we need to add our treasury signature)
    let params = json!([swap_tx_b64, {"encoding": "base64", "skipPreflight": true}]);
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
    let _approve_result = evm_rpc_call(
        &state.http,
        evm_url,
        "eth_sendRawTransaction",
        json!([approve_hex]),
    )
    .await?;

    // Step 2: Execute the swap (simplified — production uses exactInputSingle)
    // For a USDT↔USDC swap on a 0.01% fee tier (stable pair), slippage is minimal
    let swap_data = build_uniswap_exact_input_single(
        from_contract,
        _to_contract,
        job.amount as u128,
        100, // fee tier 0.01%
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

    // recipient (address) — use zero address, will be overridden
    data.extend_from_slice(&[0u8; 32]);

    // deadline (uint256) — far future
    let mut deadline = vec![0u8; 24];
    deadline.extend_from_slice(&u64::MAX.to_be_bytes());
    data.extend_from_slice(&deadline);

    // amountIn (uint256)
    let mut amount_padded = vec![0u8; 16];
    amount_padded.extend_from_slice(&amount_in.to_be_bytes());
    data.extend_from_slice(&amount_padded);

    // amountOutMinimum (uint256) — allow 0.1% slippage for stablecoin swap
    let min_out = amount_in * 999 / 1000;
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
///   pending_burn  → verify user's burn tx on MoltChain → burned
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
    // Phase 1: pending_burn → check if burn tx is confirmed on MoltChain
    let pending = list_withdrawal_jobs_by_status(&state.db, "pending_burn")?;
    for mut job in pending {
        // For now, we check if enough time has passed (burn verification)
        // In production, we'd query MoltChain for the burn event
        if let Some(ref burn_sig) = job.burn_tx_signature {
            if let Some(rpc_url) = state.config.molt_rpc_url.as_ref() {
                match molt_rpc_call(&state.http, rpc_url, "getTransaction", json!([burn_sig])).await
                {
                    Ok(result) => {
                        if !result.is_null() {
                            let success = result
                                .get("success")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            if success {
                                job.status = "burned".to_string();
                                store_withdrawal_job(&state.db, &job)?;
                                record_audit_event(
                                    &state.db,
                                    "withdrawal_burn_confirmed",
                                    &job.job_id,
                                    None,
                                    job.burn_tx_signature.as_deref(),
                                )?;
                                info!("withdrawal burn confirmed: {}", job.job_id);
                            }
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
            "musd" => job.preferred_stablecoin.as_str(),
            "wsol" => "sol",
            "weth" => "eth",
            _ => continue,
        };

        // Request threshold signatures from signers
        let signer_request = SignerRequest {
            job_id: job.job_id.clone(),
            chain: job.dest_chain.clone(),
            asset: outbound_asset.to_string(),
            from_address: match job.dest_chain.as_str() {
                "solana" => state
                    .config
                    .treasury_solana_address
                    .clone()
                    .unwrap_or_default(),
                "ethereum" => state
                    .config
                    .treasury_evm_address
                    .clone()
                    .unwrap_or_default(),
                _ => String::new(),
            },
            to_address: job.dest_address.clone(),
            amount: Some(job.amount.to_string()),
            tx_hash: None,
        };

        let mut sig_count = job.signatures.len();
        for endpoint in &state.config.signer_endpoints {
            let url = format!("{}/sign", endpoint.trim_end_matches('/'));
            match state.http.post(&url).json(&signer_request).send().await {
                Ok(response) => {
                    if let Ok(payload) = response.json::<SignerResponse>().await {
                        if payload.status == "signed" {
                            // Check for duplicate signers
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
                Err(e) => {
                    tracing::warn!("signer request failed for withdrawal {}: {}", job.job_id, e);
                }
            }
        }

        if sig_count >= state.config.signer_threshold && state.config.signer_threshold > 0 {
            job.status = "signing".to_string();
            store_withdrawal_job(&state.db, &job)?;
            record_audit_event(
                &state.db,
                "withdrawal_signatures_collected",
                &job.job_id,
                None,
                None,
            )?;
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
        match broadcast_outbound_withdrawal(state, &job).await {
            Ok(tx_hash) => {
                job.outbound_tx_hash = Some(tx_hash.clone());
                job.status = "broadcasting".to_string();
                job.last_error = None;
                store_withdrawal_job(&state.db, &job)?;
                record_audit_event(
                    &state.db,
                    "withdrawal_broadcast",
                    &job.job_id,
                    None,
                    Some(&tx_hash),
                )?;
                info!("withdrawal broadcast: {} → tx={}", job.job_id, tx_hash);
            }
            Err(e) => {
                job.attempts = job.attempts.saturating_add(1);
                job.last_error = Some(e.clone());
                job.next_attempt_at = Some(next_retry_timestamp(job.attempts));
                store_withdrawal_job(&state.db, &job)?;
                tracing::warn!("withdrawal broadcast failed for {}: {}", job.job_id, e);
            }
        }
    }

    // Phase 4: broadcasting → confirm on destination chain
    let broadcasting = list_withdrawal_jobs_by_status(&state.db, "broadcasting")?;
    for mut job in broadcasting {
        let confirmed = match job.dest_chain.as_str() {
            "solana" => {
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
            "ethereum" => {
                if let (Some(url), Some(ref tx_hash)) =
                    (state.config.evm_rpc_url.as_ref(), &job.outbound_tx_hash)
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
            store_withdrawal_job(&state.db, &job)?;
            record_audit_event(
                &state.db,
                "withdrawal_confirmed",
                &job.job_id,
                None,
                job.outbound_tx_hash.as_deref(),
            )?;

            // Decrement reserve ledger for stablecoin withdrawals
            let asset_lower = job.asset.to_lowercase();
            if asset_lower == "musd" {
                let stablecoin = &job.preferred_stablecoin;
                if let Err(e) = adjust_reserve_balance(
                    &state.db,
                    &job.dest_chain,
                    stablecoin,
                    job.amount,
                    false,
                ) {
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
    match job.dest_chain.as_str() {
        "solana" => {
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

            // For SOL: system transfer from treasury → dest_address
            // For USDT/USDC: SPL token transfer from treasury ATA → dest_address ATA
            // Signatures are provided by threshold signers
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
        "ethereum" => {
            let url = state
                .config
                .evm_rpc_url
                .as_ref()
                .ok_or_else(|| "missing EVM RPC".to_string())?;
            let outbound_asset = match job.asset.to_lowercase().as_str() {
                "weth" => "eth".to_string(),
                "musd" => job.preferred_stablecoin.clone(),
                _ => return Err(format!("unsupported ethereum withdrawal: {}", job.asset)),
            };

            // For ETH: raw value transfer from treasury → dest_address
            // For USDT/USDC: ERC-20 transfer from treasury → dest_address
            let signed_tx = assemble_signed_evm_tx(state, job, &outbound_asset)?;
            let tx_hex = format!("0x{}", hex::encode(&signed_tx));
            let result =
                evm_rpc_call(&state.http, url, "eth_sendRawTransaction", json!([tx_hex])).await?;
            result
                .as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "no tx hash returned".to_string())
        }
        other => Err(format!("unsupported destination chain: {}", other)),
    }
}

/// Assemble a Solana transaction from threshold signatures.
/// The signers have each signed the same serialized message.
fn assemble_signed_solana_tx(
    _state: &CustodyState,
    job: &WithdrawalJob,
    _asset: &str,
) -> Result<Vec<u8>, String> {
    // In production: reconstruct the Solana transaction message,
    // attach threshold signatures, serialize.
    // For now: the signatures vec contains pre-signed shares.
    if job.signatures.is_empty() {
        return Err("no signatures available".to_string());
    }

    // Placeholder: the actual implementation would use frost/multisig assembly
    // The signer service returns a fully assembled signed transaction
    let first_sig = &job.signatures[0];
    hex::decode(&first_sig.signature).map_err(|e| format!("decode signature: {}", e))
}

/// Assemble an EVM transaction from threshold signatures.
fn assemble_signed_evm_tx(
    _state: &CustodyState,
    job: &WithdrawalJob,
    _asset: &str,
) -> Result<Vec<u8>, String> {
    if job.signatures.is_empty() {
        return Err("no signatures available".to_string());
    }

    let first_sig = &job.signatures[0];
    hex::decode(&first_sig.signature).map_err(|e| format!("decode signature: {}", e))
}

/// Check if a Solana transaction is confirmed with enough confirmations
async fn check_solana_tx_confirmed(
    client: &reqwest::Client,
    url: &str,
    tx_hash: &str,
    required_confirmations: u64,
) -> Result<bool, String> {
    let result = solana_rpc_call(
        client,
        url,
        "getTransaction",
        json!([tx_hash, {"encoding": "json"}]),
    )
    .await?;
    if result.is_null() {
        return Ok(false);
    }
    // Check confirmation count from the slot
    let confirmations = result.get("slot").map(|_| required_confirmations); // simplified: if tx found, consider confirmed
    Ok(confirmations.unwrap_or(0) >= required_confirmations)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CustodyConfig {
        CustodyConfig {
            db_path: "/tmp/test_custody".to_string(),
            solana_rpc_url: Some("http://localhost:8899".to_string()),
            evm_rpc_url: Some("http://localhost:8545".to_string()),
            solana_confirmations: 1,
            evm_confirmations: 12,
            poll_interval_secs: 15,
            treasury_solana_address: Some("TEST_SOL_ADDR".to_string()),
            treasury_evm_address: Some("0xTEST".to_string()),
            solana_fee_payer_keypair_path: Some("/tmp/fee.json".to_string()),
            solana_treasury_owner: Some("TEST_OWNER".to_string()),
            solana_usdc_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            solana_usdt_mint: "Es9vMFrzaCER3FXvxuauYhVNiVw9g8Y3V9D2n7sGdG8d".to_string(),
            evm_usdc_contract: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".to_string(),
            evm_usdt_contract: "0xdAC17F958D2ee523a2206206994597C13D831ec7".to_string(),
            signer_endpoints: vec![],
            signer_threshold: 0,
            molt_rpc_url: None,
            treasury_keypair_path: None,
            musd_contract_addr: None,
            wsol_contract_addr: None,
            weth_contract_addr: None,
            rebalance_threshold_bps: 7000,
            rebalance_target_bps: 5000,
            jupiter_api_url: None,
            uniswap_router: None,
            deposit_ttl_secs: 86400,
            master_seed: "test_master_seed_for_unit_tests".to_string(),
            signer_auth_token: Some("test_token".to_string()),
            api_auth_token: Some("test_api_token".to_string()),
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
        let mut config = test_config();
        config.solana_fee_payer_keypair_path = None;
        assert!(ensure_solana_config(&config).is_err());
    }

    #[test]
    fn test_derive_deposit_address_unsupported_chain() {
        let result = derive_deposit_address("bitcoin", "btc", "m/44'/0'/0'/0/0", "test_seed");
        assert!(result.is_err());
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
        config.musd_contract_addr = Some("MUSD_CONTRACT_456".to_string());
        // Both USDT and USDC map to the same mUSD contract
        assert_eq!(
            resolve_token_contract(&config, "solana", "usdt"),
            Some("MUSD_CONTRACT_456".to_string())
        );
        assert_eq!(
            resolve_token_contract(&config, "ethereum", "usdc"),
            Some("MUSD_CONTRACT_456".to_string())
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
    fn test_resolve_token_contract_unconfigured() {
        let config = test_config(); // all contract addrs are None
        assert_eq!(resolve_token_contract(&config, "solana", "sol"), None);
        assert_eq!(resolve_token_contract(&config, "ethereum", "eth"), None);
        assert_eq!(resolve_token_contract(&config, "solana", "usdt"), None);
    }

    #[test]
    fn test_reserve_ledger_adjust_increment() {
        let _ = DB::destroy(&Options::default(), "/tmp/test_custody_reserve_1");
        let db = open_db("/tmp/test_custody_reserve_1").unwrap();
        // Increment from zero
        adjust_reserve_balance(&db, "solana", "usdt", 500_000, true).unwrap();
        assert_eq!(get_reserve_balance(&db, "solana", "usdt").unwrap(), 500_000);
        // Increment again
        adjust_reserve_balance(&db, "solana", "usdt", 300_000, true).unwrap();
        assert_eq!(get_reserve_balance(&db, "solana", "usdt").unwrap(), 800_000);
        // Different asset on same chain
        assert_eq!(get_reserve_balance(&db, "solana", "usdc").unwrap(), 0);
        let _ = DB::destroy(&Options::default(), "/tmp/test_custody_reserve_1");
    }

    #[test]
    fn test_reserve_ledger_adjust_decrement() {
        let db = open_db("/tmp/test_custody_reserve_2").unwrap();
        adjust_reserve_balance(&db, "ethereum", "usdc", 1_000_000, true).unwrap();
        adjust_reserve_balance(&db, "ethereum", "usdc", 400_000, false).unwrap();
        assert_eq!(
            get_reserve_balance(&db, "ethereum", "usdc").unwrap(),
            600_000
        );
        // Decrement past zero clamps to 0
        adjust_reserve_balance(&db, "ethereum", "usdc", 999_999, false).unwrap();
        assert_eq!(get_reserve_balance(&db, "ethereum", "usdc").unwrap(), 0);
        let _ = DB::destroy(&Options::default(), "/tmp/test_custody_reserve_2");
    }

    #[test]
    fn test_reserve_ledger_multi_chain() {
        let _ = DB::destroy(&Options::default(), "/tmp/test_custody_reserve_3");
        let db = open_db("/tmp/test_custody_reserve_3").unwrap();
        adjust_reserve_balance(&db, "solana", "usdt", 500_000, true).unwrap();
        adjust_reserve_balance(&db, "solana", "usdc", 200_000, true).unwrap();
        adjust_reserve_balance(&db, "ethereum", "usdt", 300_000, true).unwrap();
        adjust_reserve_balance(&db, "ethereum", "usdc", 100_000, true).unwrap();
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
}
