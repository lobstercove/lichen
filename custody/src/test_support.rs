use super::*;
use crate::chain_config::NEOX_TESTNET_T4_CHAIN_ID;

pub(super) fn test_withdrawal_velocity_policy() -> WithdrawalVelocityPolicy {
    WithdrawalVelocityPolicy {
        tx_caps: default_withdrawal_tx_caps(),
        daily_caps: default_withdrawal_daily_caps(),
        elevated_thresholds: default_withdrawal_elevated_thresholds(),
        extraordinary_thresholds: default_withdrawal_extraordinary_thresholds(),
        elevated_delay_secs: 900,
        extraordinary_delay_secs: 3600,
        operator_confirmation_tokens: vec!["test-operator-token".to_string()],
    }
}

pub(super) fn test_config() -> CustodyConfig {
    CustodyConfig {
        db_path: "/tmp/test_custody".to_string(),
        solana_rpc_url: Some("http://localhost:8899".to_string()),
        evm_rpc_url: Some("http://localhost:8545".to_string()),
        eth_rpc_url: None,
        bnb_rpc_url: None,
        neox_rpc_url: None,
        neox_chain_id: NEOX_TESTNET_T4_CHAIN_ID,
        solana_confirmations: 1,
        evm_confirmations: 12,
        neox_confirmations: 12,
        poll_interval_secs: 15,
        treasury_solana_address: Some("TEST_SOL_ADDR".to_string()),
        treasury_evm_address: Some("0xTEST".to_string()),
        treasury_eth_address: None,
        treasury_bnb_address: None,
        treasury_neox_address: None,
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
        wgas_contract_addr: None,
        wneo_contract_addr: None,
        neox_neo_token_contract: None,
        rebalance_threshold_bps: 7000,
        rebalance_target_bps: 5000,
        rebalance_max_slippage_bps: 50,
        jupiter_api_url: None,
        uniswap_router: None,
        deposit_ttl_secs: 86400,
        pending_burn_ttl_secs: 0,
        incident_status_path: None,
        master_seed: "test_master_seed_for_unit_tests".to_string(),
        deposit_master_seed: "test_deposit_seed_for_unit_tests".to_string(),
        signer_auth_token: Some("test_token".to_string()),
        signer_auth_tokens: vec![],
        signer_pq_addresses: vec![],
        api_auth_token: Some("test_api_token".to_string()),
        evm_multisig_address: None,
        neox_multisig_address: None,
        webhook_allowed_hosts: vec![],
        withdrawal_velocity_policy: test_withdrawal_velocity_policy(),
    }
}

pub(super) fn test_withdrawal_job() -> WithdrawalJob {
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
        velocity_tier: WithdrawalVelocityTier::Standard,
        required_signer_threshold: 0,
        required_operator_confirmations: 0,
        release_after: None,
        burn_confirmed_at: None,
        operator_confirmations: Vec::new(),
        status: "burned".to_string(),
        attempts: 0,
        last_error: None,
        next_attempt_at: None,
        created_at: 0,
    }
}

pub(super) fn test_withdrawal_request() -> WithdrawalRequest {
    let mut request = WithdrawalRequest {
        user_id: String::new(),
        asset: "wSOL".to_string(),
        amount: 1_000_000_000,
        dest_chain: "solana".to_string(),
        dest_address: "11111111111111111111111111111111".to_string(),
        preferred_stablecoin: "usdt".to_string(),
        auth: None,
    };
    sign_test_withdrawal_request(&mut request, 31);
    request
}

pub(super) fn test_db_path() -> String {
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

pub(super) fn test_state() -> CustodyState {
    let db_path = test_db_path();
    test_state_with_db_path(&db_path, true)
}

pub(super) fn test_state_with_db_path(db_path: &str, destroy_existing: bool) -> CustodyState {
    if destroy_existing {
        let _ = DB::destroy(&Options::default(), db_path);
    }
    let db = open_db(db_path).unwrap();
    let withdrawal_rate =
        load_withdrawal_rate_state(&db).unwrap_or_else(|_| WithdrawalRateState::new());
    let deposit_rate = load_deposit_rate_state(&db).unwrap_or_else(|_| DepositRateState::new());
    let (event_tx, _) = tokio::sync::broadcast::channel(16);
    CustodyState {
        db: std::sync::Arc::new(db),
        next_index_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        bridge_auth_replay_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        config: test_config(),
        http: reqwest::Client::new(),
        withdrawal_rate: std::sync::Arc::new(tokio::sync::Mutex::new(withdrawal_rate)),
        deposit_rate: std::sync::Arc::new(tokio::sync::Mutex::new(deposit_rate)),
        event_tx,
        webhook_delivery_limiter: std::sync::Arc::new(tokio::sync::Semaphore::new(1)),
    }
}

pub(super) fn write_test_incident_status(value: Value) -> String {
    let path = std::env::temp_dir().join(format!(
        "lichen-custody-incident-{}-{}.json",
        std::process::id(),
        Uuid::new_v4()
    ));
    std::fs::write(&path, value.to_string()).unwrap();
    path.to_string_lossy().into_owned()
}

pub(super) fn test_auth_headers() -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::AUTHORIZATION,
        axum::http::HeaderValue::from_static("Bearer test_api_token"),
    );
    headers
}

pub(super) fn test_bridge_access_auth_payload(seed: u8) -> (String, Value) {
    let keypair = Keypair::from_seed(&[seed; 32]);
    let user_id = keypair.pubkey().to_base58();
    let issued_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_secs();
    let expires_at = issued_at + 600;
    let message = bridge_access_message(&user_id, issued_at, expires_at);

    (
        user_id,
        json!({
            "issued_at": issued_at,
            "expires_at": expires_at,
            "signature": serde_json::to_value(keypair.sign(&message))
                .expect("encode bridge auth signature"),
        }),
    )
}

pub(super) fn test_bridge_lookup_query(user_id: &str, auth: &Value) -> BTreeMap<String, String> {
    let mut query = BTreeMap::new();
    query.insert("user_id".to_string(), user_id.to_string());
    query.insert(
        "auth".to_string(),
        serde_json::to_string(auth).expect("encode bridge auth query"),
    );
    query
}

pub(super) fn sign_test_withdrawal_request(req: &mut WithdrawalRequest, seed: u8) {
    let keypair = Keypair::from_seed(&[seed; 32]);
    let issued_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_secs();
    let expires_at = issued_at + 600;
    let nonce = format!("test-withdrawal-auth-{}", seed);

    req.asset = req.asset.trim().to_lowercase();
    req.dest_chain = req.dest_chain.trim().to_lowercase();
    req.dest_address = req.dest_address.trim().to_string();
    req.preferred_stablecoin = req.preferred_stablecoin.trim().to_lowercase();
    if req.preferred_stablecoin.is_empty() || req.asset != "musd" {
        req.preferred_stablecoin = default_preferred_stablecoin();
    }
    req.user_id = keypair.pubkey().to_base58();
    let message = withdrawal_access_message(req, issued_at, expires_at, &nonce);
    req.auth = Some(json!({
        "issued_at": issued_at,
        "expires_at": expires_at,
        "nonce": nonce,
        "signature": serde_json::to_value(keypair.sign(&message))
            .expect("encode withdrawal auth signature"),
    }));
}

pub(super) fn test_pq_signer(fill: u8) -> (Pubkey, [u8; 32]) {
    let seed = [fill; 32];
    (Keypair::from_seed(&seed).pubkey(), seed)
}

#[derive(Clone)]
pub(super) struct MockRpcState {
    pub(super) safe_nonce_hex: String,
    pub(super) safe_tx_hash_hex: String,
    pub(super) send_raw_tx_hash_hex: Option<String>,
    pub(super) transaction_receipt: Option<Value>,
    pub(super) requests: std::sync::Arc<tokio::sync::Mutex<Vec<Value>>>,
}

#[derive(Clone)]
pub(super) struct MockSignerState {
    pub(super) signer_pubkey: String,
    pub(super) signature_hex: String,
    pub(super) requests: std::sync::Arc<tokio::sync::Mutex<Vec<Value>>>,
}

#[derive(Clone)]
pub(super) struct MockPqSignerState {
    pub(super) seed: [u8; 32],
    pub(super) requests: std::sync::Arc<tokio::sync::Mutex<Vec<Value>>>,
}

#[derive(Clone)]
pub(super) struct MockLichenRpcState {
    pub(super) transaction_result: Value,
    pub(super) requests: std::sync::Arc<tokio::sync::Mutex<Vec<Value>>>,
}

pub(super) async fn mock_rpc_handler(
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

pub(super) async fn mock_signer_handler(
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

pub(super) async fn mock_pq_signer_handler(
    axum::extract::State(state): axum::extract::State<MockPqSignerState>,
    Json(payload): Json<Value>,
) -> Json<Value> {
    state.requests.lock().await.push(payload.clone());
    let message_hex = payload
        .get("message_hex")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    let message = hex::decode(message_hex).unwrap_or_default();
    let signer = Keypair::from_seed(&state.seed);

    Json(json!({
        "status": "signed",
        "pq_signature": signer.sign(&message),
    }))
}

pub(super) async fn mock_licn_rpc_handler(
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
        "getBridgeRouteRestrictionStatus" => json!({
            "route_paused": false,
            "active_restriction_ids": [],
        }),
        "canReceive" | "canSend" | "canTransfer" => json!({
            "allowed": true,
            "active_restriction_ids": [],
        }),
        _ => Value::Null,
    };

    Json(json!({
        "jsonrpc": "2.0",
        "id": payload.get("id").cloned().unwrap_or(json!(1)),
        "result": result,
    }))
}

pub(super) async fn spawn_mock_server(app: Router) -> String {
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

pub(super) fn decode_test_rlp_item(bytes: &[u8]) -> Result<(Vec<u8>, usize), String> {
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

pub(super) fn decode_test_rlp_list(bytes: &[u8]) -> Result<Vec<Vec<u8>>, String> {
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
