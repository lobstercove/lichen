use super::*;

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
    assert_eq!(default_signer_threshold(2), 2);
    assert_eq!(default_signer_threshold(3), 2);
    assert_eq!(default_signer_threshold(4), 3);
    assert_eq!(default_signer_threshold(5), 3);
    assert_eq!(default_signer_threshold(10), 6);
}

#[test]
fn test_validate_custody_security_configuration_requires_distinct_deposit_seed() {
    let mut config = test_config();
    config.deposit_master_seed = config.master_seed.clone();

    let err = validate_custody_security_configuration_with_mode(&config, false)
        .expect_err("shared treasury/deposit seed must fail outside dev mode");

    assert!(err.contains("CUSTODY_DEPOSIT_MASTER_SEED"));
}

#[test]
fn test_validate_custody_security_configuration_allows_shared_seed_in_dev_mode() {
    let mut config = test_config();
    config.deposit_master_seed = config.master_seed.clone();

    validate_custody_security_configuration_with_mode(&config, true)
        .expect("explicit dev mode should allow shared seeds");
}

#[test]
fn test_validate_pq_signer_configuration_rejects_non_majority_threshold() {
    let mut config = test_config();
    config.signer_endpoints = vec!["http://signer-1".to_string(), "http://signer-2".to_string()];
    config.signer_threshold = 1;

    let err = validate_pq_signer_configuration(&config)
        .expect_err("2 signer endpoints must require a strict-majority threshold");

    assert!(err.contains("strict-majority threshold"));
}

#[test]
fn test_validate_pq_signer_configuration_requires_matching_address_count() {
    let mut config = test_config();
    config.signer_endpoints = vec!["http://signer-1".to_string(), "http://signer-2".to_string()];
    config.signer_threshold = 2;
    config.signer_pq_addresses = vec![test_pq_signer(11).0];

    let err = validate_pq_signer_configuration(&config)
        .expect_err("each signer endpoint must have a pinned PQ signer address");

    assert!(err.contains("CUSTODY_SIGNER_PQ_ADDRESSES"));
}

#[test]
fn test_validate_webhook_destination_rejects_non_local_host_without_allowlist() {
    let config = test_config();

    let err = validate_webhook_destination(&config, "https://hooks.example.com/callback")
        .expect_err("non-local webhook must fail closed without an allowlist");

    assert!(err.contains("CUSTODY_WEBHOOK_ALLOWED_HOSTS"));
}

#[test]
fn test_validate_webhook_destination_allows_loopback_without_allowlist() {
    let config = test_config();

    assert!(validate_webhook_destination(&config, "http://localhost:3000/webhook").is_ok());
    assert!(validate_webhook_destination(&config, "http://127.0.0.1:3000/webhook").is_ok());
}

#[test]
fn test_local_rebalance_policy_error_rejects_multi_signer_mode() {
    let mut config = test_config();
    config.signer_endpoints = vec!["http://signer-1".to_string(), "http://signer-2".to_string()];
    config.signer_threshold = 2;

    let err = local_rebalance_policy_error(&config)
        .expect("multi-signer rebalance should fail closed while treasury signing is local");

    assert!(err.contains("multi-signer reserve rebalance is disabled"));
}

#[tokio::test]
async fn test_create_withdrawal_rate_limit_emits_spike_event() {
    let state = test_state();
    {
        let mut rl = state.withdrawal_rate.lock().await;
        rl.count_this_minute = 20;
        rl.value_this_hour = 5_000_000_000;
    }
    let mut event_rx = state.event_tx.subscribe();
    let request = test_withdrawal_request();

    let response = create_withdrawal(
        State(state.clone()),
        test_auth_headers(),
        Json(request.clone()),
    )
    .await;

    assert_eq!(
        response.0.get("error").and_then(|value| value.as_str()),
        Some("rate_limited: too many withdrawals, try again later")
    );

    let event = event_rx
        .recv()
        .await
        .expect("spike event should be broadcast");
    assert_eq!(event.event_type, "security.withdrawal_spike");
    assert_eq!(event.entity_id, request.user_id);
    let data = event.data.expect("spike event should include data");
    assert_eq!(
        data.get("reason").and_then(|value| value.as_str()),
        Some("count_per_minute")
    );
    assert_eq!(
        data.get("max_withdrawals_per_min")
            .and_then(|value| value.as_u64()),
        Some(20)
    );
}

#[tokio::test]
async fn test_create_withdrawal_value_limit_emits_spike_event() {
    let state = test_state();
    {
        let mut rl = state.withdrawal_rate.lock().await;
        rl.count_this_minute = 2;
        rl.value_this_hour = 10_000_000_000_000_000;
    }
    let mut event_rx = state.event_tx.subscribe();

    let response = create_withdrawal(
        State(state.clone()),
        test_auth_headers(),
        Json(test_withdrawal_request()),
    )
    .await;

    assert_eq!(
        response.0.get("error").and_then(|value| value.as_str()),
        Some("rate_limited: hourly withdrawal value limit reached")
    );

    let event = event_rx
        .recv()
        .await
        .expect("spike event should be broadcast");
    assert_eq!(event.event_type, "security.withdrawal_spike");
    let data = event.data.expect("spike event should include data");
    assert_eq!(
        data.get("reason").and_then(|value| value.as_str()),
        Some("value_per_hour")
    );
    assert_eq!(
        data.get("max_value_per_hour")
            .and_then(|value| value.as_u64()),
        Some(10_000_000_000_000_000)
    );
}

#[tokio::test]
async fn test_create_deposit_persists_rate_state_across_restart() {
    let db_path = test_db_path();
    let state = test_state_with_db_path(&db_path, true);
    let (user_id, auth) = test_bridge_access_auth_payload(41);

    let _ = create_deposit(
        State(state.clone()),
        test_auth_headers(),
        Json(CreateDepositRequest {
            user_id: user_id.clone(),
            chain: "ethereum".to_string(),
            asset: "eth".to_string(),
            auth: Some(auth),
        }),
    )
    .await
    .expect("create deposit");

    drop(state);

    let restarted = test_state_with_db_path(&db_path, false);
    let dr = restarted.deposit_rate.lock().await;
    assert_eq!(dr.count_this_minute, 1);
    assert!(dr.per_user.contains_key(&user_id));
}

#[tokio::test]
async fn test_create_withdrawal_persists_rate_state_across_restart() {
    let db_path = test_db_path();
    let state = test_state_with_db_path(&db_path, true);
    let request = test_withdrawal_request();
    let expected_address = request.dest_address.clone();
    let expected_amount = request.amount;

    let response =
        create_withdrawal(State(state.clone()), test_auth_headers(), Json(request)).await;

    assert_eq!(
        response.0.get("status").and_then(|value| value.as_str()),
        Some("pending_burn")
    );

    drop(state);

    let restarted = test_state_with_db_path(&db_path, false);
    let rl = restarted.withdrawal_rate.lock().await;
    assert_eq!(rl.count_this_minute, 1);
    assert_eq!(rl.value_this_hour, expected_amount);
    assert!(rl.per_address.contains_key(&expected_address));
}

#[test]
fn test_next_withdrawal_warning_level_escalates_and_deduplicates() {
    assert_eq!(
        next_withdrawal_warning_level(10, 20, None),
        Some(WithdrawalWarningLevel::HalfUsed)
    );
    assert_eq!(
        next_withdrawal_warning_level(11, 20, Some(WithdrawalWarningLevel::HalfUsed)),
        None
    );
    assert_eq!(
        next_withdrawal_warning_level(15, 20, Some(WithdrawalWarningLevel::HalfUsed)),
        Some(WithdrawalWarningLevel::ThreeQuartersUsed)
    );
    assert_eq!(
        next_withdrawal_warning_level(18, 20, Some(WithdrawalWarningLevel::ThreeQuartersUsed)),
        Some(WithdrawalWarningLevel::NearLimit)
    );
}

#[tokio::test]
async fn test_create_withdrawal_emits_velocity_warning_event_for_count_threshold() {
    let state = test_state();
    {
        let mut rl = state.withdrawal_rate.lock().await;
        rl.count_this_minute = 9;
        rl.value_this_hour = 5_000_000_000;
    }
    let mut event_rx = state.event_tx.subscribe();
    let request = test_withdrawal_request();

    let response = create_withdrawal(
        State(state.clone()),
        test_auth_headers(),
        Json(request.clone()),
    )
    .await;

    assert_eq!(
        response.0.get("status").and_then(|value| value.as_str()),
        Some("pending_burn")
    );

    let event = event_rx
        .recv()
        .await
        .expect("velocity warning should be broadcast");
    assert_eq!(event.event_type, "security.withdrawal_velocity_warning");
    assert_eq!(event.entity_id, request.user_id);
    let data = event.data.expect("velocity warning should include data");
    assert_eq!(
        data.get("reason").and_then(|value| value.as_str()),
        Some("count_per_minute")
    );
    assert_eq!(
        data.get("alert_level").and_then(|value| value.as_str()),
        Some("fifty_percent")
    );
    assert_eq!(
        data.get("threshold_percent")
            .and_then(|value| value.as_u64()),
        Some(50)
    );
}

#[tokio::test]
async fn test_create_withdrawal_emits_velocity_warning_event_for_value_threshold() {
    let state = test_state();
    {
        let mut rl = state.withdrawal_rate.lock().await;
        rl.count_this_minute = 2;
        rl.value_this_hour = 7_499_999_000_000_000;
    }
    let mut event_rx = state.event_tx.subscribe();

    let response = create_withdrawal(
        State(state.clone()),
        test_auth_headers(),
        Json(test_withdrawal_request()),
    )
    .await;

    assert_eq!(
        response.0.get("status").and_then(|value| value.as_str()),
        Some("pending_burn")
    );

    let event = event_rx
        .recv()
        .await
        .expect("velocity warning should be broadcast");
    assert_eq!(event.event_type, "security.withdrawal_velocity_warning");
    let data = event.data.expect("velocity warning should include data");
    assert_eq!(
        data.get("reason").and_then(|value| value.as_str()),
        Some("value_per_hour")
    );
    assert_eq!(
        data.get("alert_level").and_then(|value| value.as_str()),
        Some("seventy_five_percent")
    );
    assert_eq!(
        data.get("severity").and_then(|value| value.as_str()),
        Some("high")
    );
}

#[tokio::test]
async fn test_create_deposit_blocked_when_deposits_are_paused() {
    let mut state = test_state();
    state.config.incident_status_path = Some(write_test_incident_status(serde_json::json!({
        "mode": "deposit_guard",
        "components": {
            "deposits": {
                "status": "paused"
            }
        }
    })));
    let (user_id, auth) = test_bridge_access_auth_payload(15);

    let response = create_deposit(
        State(state),
        test_auth_headers(),
        Json(CreateDepositRequest {
            user_id,
            chain: "solana".to_string(),
            asset: "sol".to_string(),
            auth: Some(auth),
        }),
    )
    .await
    .expect_err("deposit creation must be blocked when deposits are paused");

    assert_eq!(response.code, "invalid_request");
    assert!(response
        .message
        .contains("new deposits are temporarily paused"));
}

#[tokio::test]
async fn test_create_withdrawal_allows_deposit_guard_mode() {
    let mut state = test_state();
    state.config.incident_status_path = Some(write_test_incident_status(serde_json::json!({
        "mode": "deposit_guard",
        "components": {
            "bridge": {
                "status": "operational"
            },
            "deposits": {
                "status": "paused"
            }
        }
    })));

    let response = create_withdrawal(
        State(state),
        test_auth_headers(),
        Json(test_withdrawal_request()),
    )
    .await;

    assert!(response
        .0
        .get("job_id")
        .and_then(|value| value.as_str())
        .is_some());
    assert!(response.0.get("error").is_none());
}

#[tokio::test]
async fn test_create_withdrawal_blocked_when_bridge_is_paused() {
    let mut state = test_state();
    state.config.incident_status_path = Some(write_test_incident_status(serde_json::json!({
        "mode": "bridge_pause",
        "components": {
            "bridge": {
                "status": "paused"
            }
        }
    })));

    let response = create_withdrawal(
        State(state),
        test_auth_headers(),
        Json(test_withdrawal_request()),
    )
    .await;

    assert_eq!(
        response.0.get("error").and_then(|value| value.as_str()),
        Some("bridge redemptions are temporarily paused while bridge risk is assessed")
    );
}

#[tokio::test]
async fn test_create_withdrawal_rejects_per_transaction_cap_breach() {
    let mut state = test_state();
    state
        .config
        .withdrawal_velocity_policy
        .tx_caps
        .insert("wsol".to_string(), 100);

    let mut request = test_withdrawal_request();
    request.amount = 101;
    sign_test_withdrawal_request(&mut request, 31);

    let response = create_withdrawal(State(state), test_auth_headers(), Json(request)).await;

    assert!(response
        .0
        .get("error")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .contains("per-transaction cap"));
}

#[tokio::test]
async fn test_create_withdrawal_returns_elevated_velocity_policy_metadata() {
    let mut state = test_state();
    state.config.signer_endpoints = vec![
        "http://signer-1".to_string(),
        "http://signer-2".to_string(),
        "http://signer-3".to_string(),
    ];
    state.config.signer_threshold = 2;
    state
        .config
        .withdrawal_velocity_policy
        .elevated_thresholds
        .insert("wsol".to_string(), 500);
    state
        .config
        .withdrawal_velocity_policy
        .extraordinary_thresholds
        .insert("wsol".to_string(), 5_000);
    state.config.withdrawal_velocity_policy.elevated_delay_secs = 600;

    let mut request = test_withdrawal_request();
    request.amount = 750;
    sign_test_withdrawal_request(&mut request, 31);

    let response =
        create_withdrawal(State(state.clone()), test_auth_headers(), Json(request)).await;
    let job_id = response
        .0
        .get("job_id")
        .and_then(|value| value.as_str())
        .expect("elevated withdrawal should create a job")
        .to_string();

    assert_eq!(
        response
            .0
            .get("velocity_tier")
            .and_then(|value| value.as_str()),
        Some("elevated")
    );
    assert_eq!(
        response
            .0
            .get("required_signer_threshold")
            .and_then(|value| value.as_u64()),
        Some(3)
    );
    assert_eq!(
        response
            .0
            .get("required_operator_confirmations")
            .and_then(|value| value.as_u64()),
        Some(0)
    );
    assert_eq!(
        response
            .0
            .get("delay_seconds_after_burn")
            .and_then(|value| value.as_i64()),
        Some(600)
    );

    let stored_job = fetch_withdrawal_job(&state.db, &job_id)
        .expect("fetch stored withdrawal job")
        .expect("stored withdrawal job should exist");
    assert_eq!(stored_job.velocity_tier, WithdrawalVelocityTier::Elevated);
    assert_eq!(stored_job.required_signer_threshold, 3);
    assert_eq!(stored_job.required_operator_confirmations, 0);
}

#[tokio::test]
async fn test_create_withdrawal_rejects_forged_withdrawal_auth() {
    let state = test_state();
    let mut request = test_withdrawal_request();
    request.user_id = Keypair::from_seed(&[32; 32]).pubkey().to_base58();

    let response = create_withdrawal(State(state), test_auth_headers(), Json(request)).await;

    assert_eq!(
        response.0.get("error").and_then(|value| value.as_str()),
        Some("Invalid withdrawal auth signature")
    );
}

#[tokio::test]
async fn test_create_withdrawal_reuses_existing_job_for_identical_withdrawal_auth() {
    let state = test_state();
    let request = test_withdrawal_request();

    let first = create_withdrawal(
        State(state.clone()),
        test_auth_headers(),
        Json(request.clone()),
    )
    .await;
    let first_job_id = first
        .0
        .get("job_id")
        .and_then(|value| value.as_str())
        .expect("first withdrawal creation should succeed")
        .to_string();

    {
        let mut rl = state.withdrawal_rate.lock().await;
        rl.per_address.clear();
    }

    let second = create_withdrawal(State(state), test_auth_headers(), Json(request)).await;
    let second_job_id = second
        .0
        .get("job_id")
        .and_then(|value| value.as_str())
        .expect("identical withdrawal auth replay should be idempotent")
        .to_string();

    assert_eq!(first_job_id, second_job_id);
}

#[tokio::test]
async fn test_create_withdrawal_allows_recreation_after_pending_burn_expiry() {
    let mut state = test_state();
    state.config.pending_burn_ttl_secs = 60;
    let request = test_withdrawal_request();

    let first = create_withdrawal(
        State(state.clone()),
        test_auth_headers(),
        Json(request.clone()),
    )
    .await;
    let first_job_id = first
        .0
        .get("job_id")
        .and_then(|value| value.as_str())
        .expect("first withdrawal creation should succeed")
        .to_string();

    let mut stored = fetch_withdrawal_job(&state.db, &first_job_id)
        .expect("fetch first withdrawal job")
        .expect("first withdrawal job exists");
    stored.created_at = 0;
    store_withdrawal_job(&state.db, &stored).expect("persist stale withdrawal job");

    process_withdrawal_jobs(&state)
        .await
        .expect("expire stale pending_burn job");

    {
        let mut rl = state.withdrawal_rate.lock().await;
        rl.per_address.clear();
    }

    let second = create_withdrawal(State(state.clone()), test_auth_headers(), Json(request)).await;
    let second_job_id = second
        .0
        .get("job_id")
        .and_then(|value| value.as_str())
        .expect("expired withdrawal should allow a fresh request")
        .to_string();

    assert_ne!(first_job_id, second_job_id);

    let first_job_after = fetch_withdrawal_job(&state.db, &first_job_id)
        .expect("fetch expired withdrawal job")
        .expect("expired withdrawal job exists");
    assert_eq!(first_job_after.status, "expired");

    let second_job_after = fetch_withdrawal_job(&state.db, &second_job_id)
        .expect("fetch recreated withdrawal job")
        .expect("recreated withdrawal job exists");
    assert_eq!(second_job_after.status, "pending_burn");
}

#[tokio::test]
async fn test_create_withdrawal_rejects_destination_substitution_with_same_auth() {
    let state = test_state();
    let request = test_withdrawal_request();

    let _ = create_withdrawal(
        State(state.clone()),
        test_auth_headers(),
        Json(request.clone()),
    )
    .await;

    {
        let mut rl = state.withdrawal_rate.lock().await;
        rl.per_address.clear();
    }

    let mut tampered = request;
    tampered.dest_address = "11111111111111111111111111111112".to_string();

    let response = create_withdrawal(State(state), test_auth_headers(), Json(tampered)).await;

    assert_eq!(
        response.0.get("error").and_then(|value| value.as_str()),
        Some("Invalid withdrawal auth signature")
    );
}

#[test]
fn test_evaluate_withdrawal_velocity_gate_holds_when_daily_cap_exceeded() {
    let mut state = test_state();
    state
        .config
        .withdrawal_velocity_policy
        .daily_caps
        .insert("wsol".to_string(), 100);
    let now = 1_700_000_000;

    let mut existing = test_withdrawal_job();
    existing.job_id = "daily-cap-existing".to_string();
    existing.amount = 70;
    existing.status = "confirmed".to_string();
    existing.created_at = now - 30;
    existing.burn_confirmed_at = Some(now - 30);
    store_withdrawal_job(&state.db, &existing).expect("store existing withdrawal job");

    let mut current = test_withdrawal_job();
    current.job_id = "daily-cap-current".to_string();
    current.amount = 40;
    current.status = "burned".to_string();
    current.created_at = now;
    current.burn_confirmed_at = Some(now);
    store_withdrawal_job(&state.db, &current).expect("store current withdrawal job");

    match evaluate_withdrawal_velocity_gate(&state, &current, now).expect("evaluate daily cap gate")
    {
        WithdrawalVelocityGate::DailyCapHold {
            daily_cap,
            current_volume,
            retry_after,
        } => {
            assert_eq!(daily_cap, 100);
            assert_eq!(current_volume, 110);
            assert_eq!(retry_after, next_utc_day_start(now));
        }
        gate => panic!("expected daily cap hold, got {:?}", gate),
    }
}

#[tokio::test]
async fn test_confirm_withdrawal_operator_records_out_of_band_confirmation() {
    let state = test_state();
    let mut job = test_withdrawal_job();
    job.job_id = "operator-confirmation-job".to_string();
    job.velocity_tier = WithdrawalVelocityTier::Extraordinary;
    job.required_operator_confirmations = 1;
    job.status = "burned".to_string();
    store_withdrawal_job(&state.db, &job).expect("store extraordinary withdrawal job");

    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "x-custody-operator-token",
        axum::http::HeaderValue::from_static("test-operator-token"),
    );

    let response = confirm_withdrawal_operator(
        State(state.clone()),
        headers,
        axum::extract::Path(job.job_id.clone()),
        Json(WithdrawalOperatorConfirmationPayload {
            note: Some("manual approval".to_string()),
        }),
    )
    .await
    .expect("operator confirmation should succeed")
    .0;

    assert_eq!(
        response
            .get("operator_confirmation_added")
            .and_then(|value| value.as_bool()),
        Some(true)
    );
    assert_eq!(
        response
            .get("received_operator_confirmations")
            .and_then(|value| value.as_u64()),
        Some(1)
    );

    let stored_job = fetch_withdrawal_job(&state.db, &job.job_id)
        .expect("fetch confirmed withdrawal job")
        .expect("withdrawal job should exist");
    assert_eq!(stored_job.operator_confirmations.len(), 1);
    assert_eq!(
        stored_job.operator_confirmations[0].operator_id,
        operator_token_fingerprint("test-operator-token")
    );
    assert_eq!(
        stored_job.operator_confirmations[0].note.as_deref(),
        Some("manual approval")
    );
    assert!(matches!(
        evaluate_withdrawal_velocity_gate(&state, &stored_job, chrono::Utc::now().timestamp())
            .expect("evaluate post-confirmation gate"),
        WithdrawalVelocityGate::Ready
    ));
}

#[tokio::test]
async fn test_process_withdrawal_jobs_sets_velocity_hold_after_burn_confirmation() {
    let mut state = test_state();
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
            requests: licn_requests,
        });
    let licn_rpc_url = spawn_mock_server(licn_app).await;

    state.config.licn_rpc_url = Some(licn_rpc_url);
    state.config.weth_contract_addr = Some("wrapped-weth-contract".to_string());
    state
        .config
        .withdrawal_velocity_policy
        .elevated_thresholds
        .insert("weth".to_string(), 2_000);
    state.config.withdrawal_velocity_policy.elevated_delay_secs = 600;

    let mut job = test_withdrawal_job();
    job.job_id = "withdrawal-burn-delay-hold".to_string();
    job.user_id = "11111111111111111111111111111111".to_string();
    job.asset = "wETH".to_string();
    job.amount = 2_500;
    job.dest_chain = "ethereum".to_string();
    job.dest_address = "0x3333333333333333333333333333333333333333".to_string();
    job.burn_tx_signature = Some("burn-delay-hold".to_string());
    job.status = "pending_burn".to_string();
    job.velocity_tier = WithdrawalVelocityTier::Elevated;
    store_withdrawal_job(&state.db, &job).expect("store pending burn withdrawal job");

    process_withdrawal_jobs(&state)
        .await
        .expect("process withdrawal jobs");

    let stored_job = fetch_withdrawal_job(&state.db, &job.job_id)
        .expect("fetch delayed withdrawal job")
        .expect("delayed withdrawal job should exist");
    let burn_confirmed_at = stored_job
        .burn_confirmed_at
        .expect("burn confirmation timestamp should be recorded");
    assert_eq!(stored_job.status, "burned");
    assert_eq!(stored_job.release_after, Some(burn_confirmed_at + 600));
    assert_eq!(stored_job.next_attempt_at, stored_job.release_after);
    assert!(stored_job
        .last_error
        .as_deref()
        .unwrap_or_default()
        .contains("velocity hold"));
}
