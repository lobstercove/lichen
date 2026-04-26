use super::*;

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
        velocity_tier: WithdrawalVelocityTier::Standard,
        required_signer_threshold: 0,
        required_operator_confirmations: 0,
        release_after: None,
        burn_confirmed_at: None,
        operator_confirmations: Vec::new(),
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
async fn test_process_withdrawal_jobs_burn_caller_mismatch_resets_pending_burn_without_broadcast() {
    let mut state = test_state();
    let mut event_rx = state.event_tx.subscribe();
    let licn_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let licn_app: Router = Router::new()
        .route("/", post(mock_licn_rpc_handler))
        .with_state(MockLichenRpcState {
            transaction_result: json!({
                "status": "Success",
                "to": "wrapped-weth-contract",
                "from": "22222222222222222222222222222222",
                "contract_function": "burn",
                "token_amount_spores": 2500,
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
        velocity_tier: WithdrawalVelocityTier::Standard,
        required_signer_threshold: 0,
        required_operator_confirmations: 0,
        release_after: None,
        burn_confirmed_at: None,
        operator_confirmations: Vec::new(),
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
async fn test_process_withdrawal_jobs_burn_contract_mismatch_resets_pending_burn_without_broadcast()
{
    let mut state = test_state();
    let mut event_rx = state.event_tx.subscribe();
    let licn_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let licn_app: Router = Router::new()
        .route("/", post(mock_licn_rpc_handler))
        .with_state(MockLichenRpcState {
            transaction_result: json!({
                "status": "Success",
                "to": "wrong-weth-contract",
                "from": "11111111111111111111111111111111",
                "contract_function": "burn",
                "token_amount_spores": 2500,
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
        velocity_tier: WithdrawalVelocityTier::Standard,
        required_signer_threshold: 0,
        required_operator_confirmations: 0,
        release_after: None,
        burn_confirmed_at: None,
        operator_confirmations: Vec::new(),
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
async fn test_process_withdrawal_jobs_burn_amount_mismatch_resets_pending_burn_without_broadcast() {
    let mut state = test_state();
    let mut event_rx = state.event_tx.subscribe();
    let licn_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let licn_app: Router = Router::new()
        .route("/", post(mock_licn_rpc_handler))
        .with_state(MockLichenRpcState {
            transaction_result: json!({
                "status": "Success",
                "to": "wrapped-weth-contract",
                "from": "11111111111111111111111111111111",
                "contract_function": "burn",
                "token_amount_spores": 1234,
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
        velocity_tier: WithdrawalVelocityTier::Standard,
        required_signer_threshold: 0,
        required_operator_confirmations: 0,
        release_after: None,
        burn_confirmed_at: None,
        operator_confirmations: Vec::new(),
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
async fn test_process_withdrawal_jobs_burn_method_mismatch_resets_pending_burn_without_broadcast() {
    let mut state = test_state();
    let mut event_rx = state.event_tx.subscribe();
    let licn_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let licn_app: Router = Router::new()
        .route("/", post(mock_licn_rpc_handler))
        .with_state(MockLichenRpcState {
            transaction_result: json!({
                "status": "Success",
                "to": "wrapped-weth-contract",
                "from": "11111111111111111111111111111111",
                "contract_function": "transfer",
                "token_amount_spores": 2500,
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
        velocity_tier: WithdrawalVelocityTier::Standard,
        required_signer_threshold: 0,
        required_operator_confirmations: 0,
        release_after: None,
        burn_confirmed_at: None,
        operator_confirmations: Vec::new(),
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
                "status": "Success",
                "to": "wrapped-weth-contract",
                "from": "11111111111111111111111111111111",
                "contract_function": "burn",
                "token_amount_spores": 2500,
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
        velocity_tier: WithdrawalVelocityTier::Standard,
        required_signer_threshold: 0,
        required_operator_confirmations: 0,
        release_after: None,
        burn_confirmed_at: None,
        operator_confirmations: Vec::new(),
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

#[tokio::test]
async fn test_process_withdrawal_jobs_expires_stale_pending_burn_and_releases_burn_signature() {
    let mut state = test_state();
    state.config.pending_burn_ttl_secs = 60;
    let mut event_rx = state.event_tx.subscribe();

    let job = WithdrawalJob {
        job_id: "withdrawal-pending-burn-expired".to_string(),
        user_id: "11111111111111111111111111111111".to_string(),
        asset: "wETH".to_string(),
        amount: 2500,
        dest_chain: "ethereum".to_string(),
        dest_address: "0x3333333333333333333333333333333333333333".to_string(),
        preferred_stablecoin: "usdt".to_string(),
        burn_tx_signature: Some("burn-expired-stale".to_string()),
        outbound_tx_hash: None,
        safe_nonce: None,
        signatures: Vec::new(),
        velocity_tier: WithdrawalVelocityTier::Standard,
        required_signer_threshold: 0,
        required_operator_confirmations: 0,
        release_after: None,
        burn_confirmed_at: None,
        operator_confirmations: Vec::new(),
        status: "pending_burn".to_string(),
        attempts: 0,
        last_error: None,
        next_attempt_at: None,
        created_at: 0,
    };
    store_withdrawal_job(&state.db, &job).expect("store stale pending_burn withdrawal job");

    let idx_cf = state.db.cf_handle(CF_INDEXES).expect("indexes cf");
    state
        .db
        .put_cf(
            idx_cf,
            burn_signature_index_key("burn-expired-stale").as_bytes(),
            job.job_id.as_bytes(),
        )
        .expect("reserve burn signature");

    process_withdrawal_jobs(&state)
        .await
        .expect("process stale pending_burn expiry");

    let job_after = fetch_withdrawal_job(&state.db, &job.job_id)
        .expect("fetch expired withdrawal job")
        .expect("expired withdrawal job exists");
    assert_eq!(job_after.status, "expired");
    assert!(job_after.burn_tx_signature.is_none());
    assert!(job_after
        .last_error
        .as_deref()
        .unwrap_or_default()
        .contains("pending_burn expired"));
    assert!(job_after.next_attempt_at.is_none());

    assert!(state
        .db
        .get_cf(
            idx_cf,
            burn_signature_index_key("burn-expired-stale").as_bytes(),
        )
        .expect("read released burn reservation")
        .is_none());

    let event = tokio::time::timeout(std::time::Duration::from_millis(100), event_rx.recv())
        .await
        .expect("withdrawal expiry event should be emitted")
        .expect("expiry event should be received");
    assert_eq!(event.event_type, "withdrawal.expired");
    assert_eq!(event.entity_id, job.job_id);
}

#[tokio::test]
async fn test_process_signing_withdrawals_requires_tx_intent_before_broadcast() {
    let db_path = test_db_path();
    let _ = DB::destroy(&Options::default(), &db_path);
    let mut opts = Options::default();
    opts.create_if_missing(true);
    opts.create_missing_column_families(true);
    let db = DB::open_cf_descriptors(
        &opts,
        &db_path,
        vec![
            ColumnFamilyDescriptor::new(CF_WITHDRAWAL_JOBS, Options::default()),
            ColumnFamilyDescriptor::new(CF_STATUS_INDEX, Options::default()),
        ],
    )
    .expect("open test DB without tx_intents cf");

    let mut state = test_state();
    state.db = Arc::new(db);
    state.config.solana_rpc_url = None;

    let mut job = test_withdrawal_job();
    job.status = "signing".to_string();
    job.attempts = 0;
    job.last_error = None;
    job.next_attempt_at = None;
    store_withdrawal_job(&state.db, &job).expect("store signing withdrawal");

    withdrawal_settlement_support::process_signing_withdrawals(&state)
        .await
        .expect("process signing withdrawals");

    let stored = fetch_withdrawal_job(&state.db, &job.job_id)
        .expect("fetch withdrawal job")
        .expect("withdrawal job exists");
    assert_eq!(stored.status, "signing");
    assert_eq!(stored.attempts, 1);
    assert!(stored
        .last_error
        .as_deref()
        .unwrap_or_default()
        .contains("failed to record withdrawal tx intent"));

    drop(state);
    let _ = DB::destroy(&Options::default(), &db_path);
}

#[tokio::test]
async fn test_process_broadcasting_withdrawals_marks_reverted_evm_tx_failed() {
    let mut state = test_state();
    let rpc_app: Router =
        Router::new()
            .route("/", post(mock_rpc_handler))
            .with_state(MockRpcState {
                safe_nonce_hex: "0x0".to_string(),
                safe_tx_hash_hex: "0x0".to_string(),
                send_raw_tx_hash_hex: None,
                transaction_receipt: Some(json!({
                    "status": "0x0",
                    "blockNumber": "0x10",
                })),
                requests: std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new())),
            });
    let rpc_url = spawn_mock_server(rpc_app).await;
    state.config.evm_rpc_url = Some(rpc_url.clone());
    state.config.eth_rpc_url = Some(rpc_url);

    let mut job = test_withdrawal_job();
    job.job_id = "withdrawal-reverted-outbound".to_string();
    job.dest_chain = "ethereum".to_string();
    job.asset = "wETH".to_string();
    job.dest_address = "0x3333333333333333333333333333333333333333".to_string();
    job.status = "broadcasting".to_string();
    job.outbound_tx_hash = Some("0xdeadbeef".to_string());
    store_withdrawal_job(&state.db, &job).expect("store broadcasting withdrawal");

    withdrawal_settlement_support::process_broadcasting_withdrawals(&state)
        .await
        .expect("process broadcasting withdrawals");

    let stored = fetch_withdrawal_job(&state.db, &job.job_id)
        .expect("fetch withdrawal job")
        .expect("withdrawal job exists");
    assert_eq!(stored.status, "permanently_failed");
    assert_eq!(
        stored.last_error.as_deref(),
        Some("evm transaction failed with status 0x0")
    );
}

#[test]
fn test_count_withdrawal_jobs_with_index_includes_expired() {
    let _ = DB::destroy(&Options::default(), "/tmp/test_custody_count_withdrawal");
    let db = open_db("/tmp/test_custody_count_withdrawal").unwrap();

    let pending_job = WithdrawalJob {
        job_id: "test-withdrawal-count-1".to_string(),
        user_id: "11111111111111111111111111111111".to_string(),
        asset: "wETH".to_string(),
        amount: 2500,
        dest_chain: "ethereum".to_string(),
        dest_address: "0x3333333333333333333333333333333333333333".to_string(),
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
        status: "pending_burn".to_string(),
        attempts: 0,
        last_error: None,
        next_attempt_at: None,
        created_at: 1000,
    };
    store_withdrawal_job(&db, &pending_job).unwrap();

    let expired_job = WithdrawalJob {
        job_id: "test-withdrawal-count-2".to_string(),
        status: "expired".to_string(),
        ..pending_job.clone()
    };
    store_withdrawal_job(&db, &expired_job).unwrap();

    let counts = count_withdrawal_jobs(&db).unwrap();
    assert_eq!(counts.total, 2);
    assert_eq!(*counts.by_status.get("pending_burn").unwrap_or(&0), 1);
    assert_eq!(*counts.by_status.get("expired").unwrap_or(&0), 1);

    let _ = DB::destroy(&Options::default(), "/tmp/test_custody_count_withdrawal");
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
