use super::*;

#[derive(Serialize)]
pub(super) struct HealthResponse {
    pub(super) status: &'static str,
}

/// AUDIT-FIX 2.18: Single-instance enforcement is handled by RocksDB's exclusive
/// file lock on the DB directory. Multi-instance access to the same DB is prevented
/// at the storage layer. The RESERVE_LOCK static in adjust_reserve_balance()
/// serializes within-process concurrent access.
#[derive(Clone)]
pub(super) struct CustodyState {
    pub(super) db: Arc<DB>,
    pub(super) next_index_lock: Arc<Mutex<()>>,
    pub(super) bridge_auth_replay_lock: Arc<Mutex<()>>,
    pub(super) config: CustodyConfig,
    pub(super) http: reqwest::Client,
    /// AUDIT-FIX 1.20: Global withdrawal rate limiter
    pub(super) withdrawal_rate: Arc<Mutex<WithdrawalRateState>>,
    /// AUDIT-FIX W-H4: Deposit rate limiter
    pub(super) deposit_rate: Arc<Mutex<DepositRateState>>,
    /// Broadcast channel for webhook/WebSocket events
    pub(super) event_tx: broadcast::Sender<CustodyWebhookEvent>,
    /// Short-lived one-use tickets for custody event WebSocket upgrades.
    pub(super) ws_event_tickets: Arc<Mutex<BTreeMap<String, WsEventTicket>>>,
    /// Cap concurrent webhook deliveries to prevent unbounded task fan-out.
    pub(super) webhook_delivery_limiter: Arc<Semaphore>,
}

/// Registered webhook destination.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct WebhookRegistration {
    /// Unique webhook ID
    pub(super) id: String,
    /// HTTPS URL to POST events to
    pub(super) url: String,
    /// HMAC-SHA256 secret for signing payloads (provided by the registrant)
    pub(super) secret: String,
    /// Optional filter: only send events matching these types.
    /// Empty = all events. Example: ["deposit.confirmed", "withdrawal.confirmed"]
    #[serde(default)]
    pub(super) event_filter: Vec<String>,
    /// Whether this webhook is active
    pub(super) active: bool,
    /// Creation timestamp
    pub(super) created_at: i64,
    /// Description/label
    #[serde(default)]
    pub(super) description: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct CreateWebhookRequest {
    pub(super) url: String,
    pub(super) secret: String,
    #[serde(default)]
    pub(super) event_filter: Vec<String>,
    #[serde(default)]
    pub(super) description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct CustodyWebhookEvent {
    pub(super) event_id: String,
    pub(super) event_type: String,
    pub(super) entity_id: String,
    pub(super) deposit_id: Option<String>,
    pub(super) tx_hash: Option<String>,
    pub(super) data: Option<Value>,
    pub(super) timestamp: i64,
}

#[derive(Clone, Debug)]
pub(super) struct WsEventTicket {
    pub(super) event_filter: Vec<String>,
    pub(super) expires_at: i64,
}

#[derive(Clone, Debug)]
pub(super) struct CustodyConfig {
    pub(super) db_path: String,
    pub(super) solana_rpc_url: Option<String>,
    pub(super) evm_rpc_url: Option<String>,
    /// Per-chain EVM RPC: Ethereum-specific (overrides evm_rpc_url for ETH deposits)
    pub(super) eth_rpc_url: Option<String>,
    /// Per-chain EVM RPC: BSC/BNB-specific (overrides evm_rpc_url for BNB deposits)
    pub(super) bnb_rpc_url: Option<String>,
    /// Ethereum route chain ID. Defaults to Ethereum mainnet, but testnet
    /// deployments can pin Sepolia or another explicitly selected source chain.
    pub(super) eth_chain_id: u64,
    /// BSC/BNB route chain ID. Defaults to BNB Smart Chain mainnet, but testnet
    /// deployments can pin BSC testnet or another explicitly selected source chain.
    pub(super) bnb_chain_id: u64,
    /// Per-chain EVM RPC: Neo X-specific.
    pub(super) neox_rpc_url: Option<String>,
    /// Neo X chain ID used for derivation domain separation and RPC checks.
    pub(super) neox_chain_id: u64,
    pub(super) solana_confirmations: u64,
    pub(super) evm_confirmations: u64,
    /// Per-chain EVM confirmations: Neo X-specific.
    pub(super) neox_confirmations: u64,
    pub(super) poll_interval_secs: u64,
    pub(super) treasury_solana_address: Option<String>,
    pub(super) treasury_evm_address: Option<String>,
    /// Per-chain EVM treasury: separate ETH treasury address (overrides treasury_evm_address)
    pub(super) treasury_eth_address: Option<String>,
    /// Per-chain EVM treasury: separate BNB treasury address (overrides treasury_evm_address)
    pub(super) treasury_bnb_address: Option<String>,
    /// Per-chain EVM treasury: separate Neo X treasury address.
    pub(super) treasury_neox_address: Option<String>,
    pub(super) solana_fee_payer_keypair_path: Option<String>,
    pub(super) solana_treasury_owner: Option<String>,
    pub(super) solana_usdc_mint: String,
    pub(super) solana_usdt_mint: String,
    /// Ethereum USDC source token contract. Loaded from CUSTODY_ETH_USDC_TOKEN_ADDR
    /// or the legacy CUSTODY_EVM_USDC alias.
    pub(super) evm_usdc_contract: String,
    /// Ethereum USDT source token contract. Loaded from CUSTODY_ETH_USDT_TOKEN_ADDR
    /// or the legacy CUSTODY_EVM_USDT alias.
    pub(super) evm_usdt_contract: String,
    /// BSC/BEP-20 USDC source token contract required before BSC USDC routes open.
    pub(super) bnb_usdc_contract: Option<String>,
    /// BSC/BEP-20 USDT source token contract required before BSC USDT routes open.
    pub(super) bnb_usdt_contract: Option<String>,
    pub(super) signer_endpoints: Vec<String>,
    pub(super) signer_threshold: usize,
    pub(super) licn_rpc_url: Option<String>,
    pub(super) treasury_keypair_path: Option<String>,
    // Wrapped token contract addresses on Lichen
    pub(super) musd_contract_addr: Option<String>,
    pub(super) wsol_contract_addr: Option<String>,
    pub(super) weth_contract_addr: Option<String>,
    pub(super) wbnb_contract_addr: Option<String>,
    pub(super) wgas_contract_addr: Option<String>,
    pub(super) wneo_contract_addr: Option<String>,
    /// Neo X NEO source token contract required by the neox/neo custody route.
    pub(super) neox_neo_token_contract: Option<String>,
    // Reserve rebalance settings
    pub(super) rebalance_threshold_bps: u64,
    pub(super) rebalance_target_bps: u64,
    pub(super) jupiter_api_url: Option<String>,
    pub(super) uniswap_router: Option<String>,
    /// AUDIT-FIX M14: Max tolerable slippage (bps) for rebalance swaps.
    /// Swaps exceeding this are rejected; unverifiable outputs are not credited.
    /// Set via CUSTODY_REBALANCE_MAX_SLIPPAGE_BPS (default: 50 = 0.5%).
    pub(super) rebalance_max_slippage_bps: u64,
    pub(super) deposit_ttl_secs: i64,
    pub(super) pending_burn_ttl_secs: i64,
    /// Optional incident-response manifest shared with RPC/operator banners.
    pub(super) incident_status_path: Option<String>,
    /// C8 fix: Secret master seed for key derivation (HMAC-SHA256 instead of plain SHA256).
    pub(super) master_seed: String,
    /// Dedicated seed root for deposit address derivation and deposit sweeps.
    /// Must differ from master_seed outside explicit insecure dev mode.
    pub(super) deposit_master_seed: String,
    /// C9 fix: Auth token for threshold signer requests (global fallback)
    pub(super) signer_auth_token: Option<String>,
    /// AUDIT-FIX 1.22: Per-signer auth tokens (one per signer_endpoint, same order).
    /// Set via CUSTODY_SIGNER_AUTH_TOKENS=token1,token2,...
    /// Falls back to signer_auth_token if not set for a given index.
    pub(super) signer_auth_tokens: Vec<Option<String>>,
    /// Allowed PQ signer addresses for custody approvals, in the same order as signer_endpoints.
    /// Set via CUSTODY_SIGNER_PQ_ADDRESSES=addr1,addr2,...
    pub(super) signer_pq_addresses: Vec<Pubkey>,
    /// M17 fix: API auth token for withdrawal and other write endpoints
    pub(super) api_auth_token: Option<String>,
    pub(super) withdrawal_velocity_policy: WithdrawalVelocityPolicy,
    /// EVM multisig contract address (e.g. Gnosis Safe).
    /// Required for multi-signer EVM withdrawals.
    /// Set via CUSTODY_EVM_MULTISIG_ADDRESS env var.
    pub(super) evm_multisig_address: Option<String>,
    /// Neo X-specific EVM multisig contract address.
    /// Falls back to evm_multisig_address only when this route-specific value is unset.
    pub(super) neox_multisig_address: Option<String>,
    /// Optional outbound webhook host allowlist.
    /// When set, webhook URLs must resolve to one of these hosts.
    /// Set via CUSTODY_WEBHOOK_ALLOWED_HOSTS=hooks.example.com,events.example.com
    pub(super) webhook_allowed_hosts: Vec<String>,
}

#[derive(Serialize)]
pub(super) struct StatusCounts {
    pub(super) total: usize,
    pub(super) by_status: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
pub(super) struct ErrorResponse {
    pub(super) code: &'static str,
    pub(super) message: String,
}

impl ErrorResponse {
    pub(super) fn invalid(message: &str) -> Self {
        Self {
            code: "invalid_request",
            message: message.to_string(),
        }
    }

    pub(super) fn not_found(message: &str) -> Self {
        Self {
            code: "not_found",
            message: message.to_string(),
        }
    }

    pub(super) fn db(message: &str) -> Self {
        Self {
            code: "db_error",
            message: message.to_string(),
        }
    }

    pub(super) fn status_for_error_message(message: &str) -> StatusCode {
        if message.starts_with("rate_limited:") {
            return StatusCode::TOO_MANY_REQUESTS;
        }
        if message.contains("auth already used for a different") {
            return StatusCode::CONFLICT;
        }
        if message.starts_with("db error:") {
            return StatusCode::INTERNAL_SERVER_ERROR;
        }
        StatusCode::BAD_REQUEST
    }

    pub(super) fn status_code(&self) -> StatusCode {
        match self.code {
            "invalid_request" => Self::status_for_error_message(&self.message),
            "not_found" => StatusCode::NOT_FOUND,
            "unauthorized" => StatusCode::UNAUTHORIZED,
            "db_error" => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

impl From<Json<ErrorResponse>> for ErrorResponse {
    fn from(value: Json<ErrorResponse>) -> Self {
        value.0
    }
}

impl IntoResponse for ErrorResponse {
    fn into_response(self) -> Response {
        let status = self.status_code();
        (status, Json(self)).into_response()
    }
}

pub(super) struct CustodyJsonResponse(pub(super) Value, pub(super) StatusCode);

impl CustodyJsonResponse {
    pub(super) fn from_value(value: Value) -> Self {
        let status = value
            .get("error")
            .and_then(|error| error.as_str())
            .map(ErrorResponse::status_for_error_message)
            .unwrap_or(StatusCode::OK);
        Self(value, status)
    }

    pub(super) fn from_json(value: Json<Value>) -> Self {
        Self::from_value(value.0)
    }
}

impl IntoResponse for CustodyJsonResponse {
    fn into_response(self) -> Response {
        let status = self.1;
        (status, Json(self.0)).into_response()
    }
}
