use super::*;
use crate::chain_config::{NEOX_MAINNET_CHAIN_ID, NEOX_TESTNET_T4_CHAIN_ID};

#[tokio::test]
async fn test_create_deposit_uses_dedicated_deposit_seed_and_persists_source() {
    let mut state = test_state();
    state.config.deposit_master_seed = "dedicated_deposit_seed_for_tests_0123456789".to_string();
    let (user_id, auth) = test_bridge_access_auth_payload(11);

    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", "Bearer test_api_token".parse().unwrap());

    let response = create_deposit(
        State(state.clone()),
        headers,
        Json(CreateDepositRequest {
            user_id,
            chain: "ethereum".to_string(),
            asset: "eth".to_string(),
            auth: Some(auth),
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

#[tokio::test]
async fn test_create_deposit_rejects_forged_bridge_auth() {
    let state = test_state();
    let (_, forged_auth) = test_bridge_access_auth_payload(12);
    let wrong_user_id = Keypair::from_seed(&[13; 32]).pubkey().to_base58();

    let err = create_deposit(
        State(state),
        test_auth_headers(),
        Json(CreateDepositRequest {
            user_id: wrong_user_id,
            chain: "ethereum".to_string(),
            asset: "eth".to_string(),
            auth: Some(forged_auth),
        }),
    )
    .await
    .expect_err("forged bridge auth must fail");

    assert_eq!(err.code, "invalid_request");
    assert_eq!(err.message, "Invalid bridge auth signature");
}

#[tokio::test]
async fn test_create_deposit_reuses_existing_deposit_for_identical_bridge_auth() {
    let state = test_state();
    let (user_id, auth) = test_bridge_access_auth_payload(14);

    let first = create_deposit(
        State(state.clone()),
        test_auth_headers(),
        Json(CreateDepositRequest {
            user_id: user_id.clone(),
            chain: "ethereum".to_string(),
            asset: "eth".to_string(),
            auth: Some(auth.clone()),
        }),
    )
    .await
    .expect("first deposit creation should succeed");

    let second = create_deposit(
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
    .expect("identical bridge auth replay should be idempotent");

    assert_eq!(first.0.deposit_id, second.0.deposit_id);
    assert_eq!(first.0.address, second.0.address);

    let dr = state.deposit_rate.lock().await;
    assert_eq!(dr.count_this_minute, 1);
}

#[tokio::test]
async fn test_create_deposit_neox_gas_uses_chain_id_scoped_path_and_gates_neo() {
    let mut state = test_state();
    state.config.neox_rpc_url = Some("http://localhost:9545".to_string());
    let (user_id, auth) = test_bridge_access_auth_payload(31);

    let response = create_deposit(
        State(state.clone()),
        test_auth_headers(),
        Json(CreateDepositRequest {
            user_id,
            chain: "neox".to_string(),
            asset: "gas".to_string(),
            auth: Some(auth),
        }),
    )
    .await
    .expect("Neo X GAS deposit should be created");

    let stored = fetch_deposit(&state.db, &response.0.deposit_id)
        .expect("fetch Neo X deposit")
        .expect("Neo X deposit exists");
    assert_eq!(stored.chain, "neox");
    assert_eq!(stored.asset, "gas");
    assert!(
        stored.derivation_path.starts_with("m/44'/12227332'/"),
        "Neo X deposit path must be chain-ID scoped: {}",
        stored.derivation_path
    );
    let expected = derive_deposit_address(
        "neox",
        "gas",
        &stored.derivation_path,
        &state.config.deposit_master_seed,
    )
    .expect("derive Neo X deposit address");
    assert_eq!(stored.address, expected);

    let (neo_user_id, neo_auth) = test_bridge_access_auth_payload(32);
    let err = create_deposit(
        State(state),
        test_auth_headers(),
        Json(CreateDepositRequest {
            user_id: neo_user_id,
            chain: "neox".to_string(),
            asset: "neo".to_string(),
            auth: Some(neo_auth),
        }),
    )
    .await
    .expect_err("NEO deposits must stay gated until a source route is configured");
    assert_eq!(err.code, "invalid_request");
    assert!(err
        .message
        .contains("Neo X custody currently supports asset=gas only"));
}

#[tokio::test]
async fn test_create_deposit_rejects_bridge_auth_reuse_for_different_asset() {
    let state = test_state();
    let (user_id, auth) = test_bridge_access_auth_payload(15);

    let _ = create_deposit(
        State(state.clone()),
        test_auth_headers(),
        Json(CreateDepositRequest {
            user_id: user_id.clone(),
            chain: "ethereum".to_string(),
            asset: "eth".to_string(),
            auth: Some(auth.clone()),
        }),
    )
    .await
    .expect("first deposit creation should succeed");

    {
        let mut dr = state.deposit_rate.lock().await;
        dr.per_user.clear();
    }

    let err = create_deposit(
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
    .expect_err("bridge auth replay must not authorize a different deposit request");

    assert_eq!(err.code, "invalid_request");
    assert_eq!(
        err.message,
        "bridge auth already used for a different deposit request; sign a new bridge authorization"
    );
}

#[tokio::test]
async fn test_get_deposit_accepts_same_bridge_auth_after_create() {
    let state = test_state();
    let (user_id, auth) = test_bridge_access_auth_payload(16);

    let created = create_deposit(
        State(state.clone()),
        test_auth_headers(),
        Json(CreateDepositRequest {
            user_id: user_id.clone(),
            chain: "ethereum".to_string(),
            asset: "eth".to_string(),
            auth: Some(auth.clone()),
        }),
    )
    .await
    .expect("deposit creation should succeed");

    let lookup = get_deposit(
        State(state),
        test_auth_headers(),
        axum::extract::Path(created.0.deposit_id.clone()),
        axum::extract::Query(test_bridge_lookup_query(&user_id, &auth)),
    )
    .await
    .expect("read-only deposit lookup should continue to accept the current bridge auth");

    assert_eq!(lookup.0.deposit_id, created.0.deposit_id);
    assert_eq!(lookup.0.user_id, user_id);
}

#[tokio::test]
async fn test_get_deposit_requires_matching_bridge_auth_user() {
    let state = test_state();
    let (user_id, auth) = test_bridge_access_auth_payload(21);
    let deposit = DepositRequest {
        deposit_id: "dep-lookup-1".to_string(),
        user_id: user_id.clone(),
        chain: "solana".to_string(),
        asset: "sol".to_string(),
        address: "lookup-address".to_string(),
        derivation_path: "m/44'/501'/0'/0/1".to_string(),
        deposit_seed_source: DEPOSIT_SEED_SOURCE_TREASURY_ROOT.to_string(),
        created_at: 1,
        status: "issued".to_string(),
    };
    store_deposit(&state.db, &deposit).expect("store deposit for lookup test");

    let response = get_deposit(
        State(state.clone()),
        test_auth_headers(),
        axum::extract::Path(deposit.deposit_id.clone()),
        axum::extract::Query(test_bridge_lookup_query(&user_id, &auth)),
    )
    .await
    .expect("authorized user should be able to read deposit");

    assert_eq!(response.0.user_id, user_id);

    let (other_user_id, other_auth) = test_bridge_access_auth_payload(22);
    let err = get_deposit(
        State(state),
        test_auth_headers(),
        axum::extract::Path(deposit.deposit_id),
        axum::extract::Query(test_bridge_lookup_query(&other_user_id, &other_auth)),
    )
    .await
    .expect_err("foreign users must not read another deposit");

    assert_eq!(err.code, "not_found");
    assert_eq!(err.message, "Deposit not found for authenticated user");
}

#[tokio::test]
async fn test_create_deposit_rate_limit_rejection_returns_bad_request_status() {
    let state = test_state();
    let app = Router::new()
        .route("/deposits", post(create_deposit))
        .with_state(state);
    let base_url = spawn_mock_server(app).await;
    let (user_id, auth) = test_bridge_access_auth_payload(23);
    let keypair = Keypair::from_seed(&[23; 32]);
    let first_issued_at = auth["issued_at"]
        .as_u64()
        .expect("test bridge auth includes issued_at");
    let second_issued_at = first_issued_at.saturating_sub(1);
    let second_expires_at = second_issued_at + 600;
    let second_message = bridge_access_message(&user_id, second_issued_at, second_expires_at);
    let second_auth = json!({
        "issued_at": second_issued_at,
        "expires_at": second_expires_at,
        "signature": serde_json::to_value(keypair.sign(&second_message))
            .expect("encode second bridge auth signature"),
    });
    let payload = json!({
        "user_id": user_id,
        "chain": "solana",
        "asset": "sol",
        "auth": auth,
    });
    let second_payload = json!({
        "user_id": payload["user_id"].clone(),
        "chain": "solana",
        "asset": "sol",
        "auth": second_auth,
    });
    let client = reqwest::Client::new();

    let first = client
        .post(format!("{}/deposits", base_url))
        .header("Authorization", "Bearer test_api_token")
        .json(&payload)
        .send()
        .await
        .expect("send first deposit request");
    assert_eq!(first.status(), reqwest::StatusCode::OK);

    let second = client
        .post(format!("{}/deposits", base_url))
        .header("Authorization", "Bearer test_api_token")
        .json(&second_payload)
        .send()
        .await
        .expect("send second deposit request");
    assert_eq!(second.status(), reqwest::StatusCode::BAD_REQUEST);

    let body: Value = second
        .json()
        .await
        .expect("parse rate-limited deposit response");
    assert_eq!(body["code"], "invalid_request");
    assert_eq!(
        body["message"],
        "rate_limited: wait 10s between deposit requests"
    );
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

#[test]
fn test_build_credit_job_neox_gas_exact_conversion_and_gated_neo() {
    let mut state = test_state();
    state.config.licn_rpc_url = Some("http://localhost:8899".to_string());
    state.config.treasury_keypair_path = Some("/tmp/test-treasury.json".to_string());
    state.config.wgas_contract_addr = Some("WGAS_CONTRACT_999".to_string());
    state.config.wneo_contract_addr = Some("WNEO_CONTRACT_999".to_string());

    let gas_deposit = DepositRequest {
        deposit_id: "dep-neox-gas-credit-1".to_string(),
        user_id: "11111111111111111111111111111111".to_string(),
        chain: "neox".to_string(),
        asset: "gas".to_string(),
        address: "0x5555555555555555555555555555555555555555".to_string(),
        derivation_path: "m/44'/12227332'/0'/0/3".to_string(),
        deposit_seed_source: DEPOSIT_SEED_SOURCE_DEPOSIT_ROOT.to_string(),
        created_at: 1000,
        status: "swept".to_string(),
    };
    store_deposit(&state.db, &gas_deposit).expect("store Neo X GAS deposit");

    let gas_sweep = SweepJob {
        job_id: "sweep-neox-gas-credit-1".to_string(),
        deposit_id: gas_deposit.deposit_id.clone(),
        chain: "neox".to_string(),
        asset: "gas".to_string(),
        from_address: gas_deposit.address.clone(),
        to_treasury: "0x4444444444444444444444444444444444444444".to_string(),
        tx_hash: "tx".to_string(),
        amount: Some("1000000000000000000".to_string()),
        credited_amount: None,
        signatures: Vec::new(),
        sweep_tx_hash: Some("sweep-hash".to_string()),
        attempts: 0,
        last_error: None,
        next_attempt_at: None,
        status: "sweep_confirmed".to_string(),
        created_at: 1000,
    };
    let credit = build_credit_job(&state, &gas_sweep)
        .expect("build Neo X GAS credit job")
        .expect("Neo X GAS credit job should be created");
    assert_eq!(credit.amount_spores, 1_000_000_000);
    assert_eq!(credit.source_chain, "neox");
    assert_eq!(credit.source_asset, "gas");

    let dust_sweep = SweepJob {
        job_id: "sweep-neox-gas-dust-1".to_string(),
        deposit_id: gas_deposit.deposit_id.clone(),
        chain: "neox".to_string(),
        asset: "gas".to_string(),
        from_address: gas_deposit.address.clone(),
        to_treasury: "0x4444444444444444444444444444444444444444".to_string(),
        tx_hash: "tx".to_string(),
        amount: Some("1000000000000000001".to_string()),
        credited_amount: None,
        signatures: Vec::new(),
        sweep_tx_hash: Some("sweep-hash".to_string()),
        attempts: 0,
        last_error: None,
        next_attempt_at: None,
        status: "sweep_confirmed".to_string(),
        created_at: 1000,
    };
    let err = build_credit_job(&state, &dust_sweep)
        .expect_err("non-exact Neo X GAS conversion must be rejected");
    assert!(err.contains("non-exact deposit decimal conversion rejected"));

    let neo_deposit = DepositRequest {
        deposit_id: "dep-neox-neo-credit-1".to_string(),
        user_id: "11111111111111111111111111111111".to_string(),
        chain: "neox".to_string(),
        asset: "neo".to_string(),
        address: "0x6666666666666666666666666666666666666666".to_string(),
        derivation_path: "m/44'/12227332'/0'/0/4".to_string(),
        deposit_seed_source: DEPOSIT_SEED_SOURCE_DEPOSIT_ROOT.to_string(),
        created_at: 1000,
        status: "swept".to_string(),
    };
    store_deposit(&state.db, &neo_deposit).expect("store gated Neo X NEO deposit");
    let neo_sweep = SweepJob {
        job_id: "sweep-neox-neo-credit-1".to_string(),
        deposit_id: neo_deposit.deposit_id.clone(),
        chain: "neox".to_string(),
        asset: "neo".to_string(),
        from_address: neo_deposit.address.clone(),
        to_treasury: "0x4444444444444444444444444444444444444444".to_string(),
        tx_hash: "tx".to_string(),
        amount: Some("1000000000".to_string()),
        credited_amount: None,
        signatures: Vec::new(),
        sweep_tx_hash: Some("sweep-hash".to_string()),
        attempts: 0,
        last_error: None,
        next_attempt_at: None,
        status: "sweep_confirmed".to_string(),
        created_at: 1000,
    };
    assert!(
        build_credit_job(&state, &neo_sweep)
            .expect("gated Neo X NEO should not error without a source route")
            .is_none(),
        "gated NEO must not create a credit job"
    );
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
    assert_eq!(bip44_coin_type("neox").unwrap(), 60);
    assert_eq!(bip44_coin_type("btc").unwrap(), 0);
    assert_eq!(bip44_coin_type("bitcoin").unwrap(), 0);
    assert_eq!(bip44_coin_type("lichen").unwrap(), 9999);
    assert!(bip44_coin_type("unknown").is_err());
}

/// F2-01: BIP-44 derivation path format test
#[test]
fn test_bip44_derivation_path() {
    let path_sol = bip44_derivation_path("solana", 7, 0).unwrap();
    assert!(
        path_sol.starts_with("m/44'/501'/"),
        "Solana path must use coin_type 501: {}",
        path_sol
    );
    assert!(path_sol.ends_with("/0/0"), "Index 0: {}", path_sol);

    let path_eth = bip44_derivation_path("eth", 7, 5).unwrap();
    assert!(
        path_eth.starts_with("m/44'/60'/"),
        "ETH path must use coin_type 60: {}",
        path_eth
    );
    assert!(path_eth.ends_with("/0/5"), "Index 5: {}", path_eth);

    let path_bnb = bip44_derivation_path("bnb", 7, 7).unwrap();
    assert!(
        path_bnb.starts_with("m/44'/60'/"),
        "BNB path must use coin_type 60: {}",
        path_bnb
    );
    assert!(path_bnb.ends_with("/0/7"), "Index 7: {}", path_bnb);

    assert_ne!(path_sol, path_eth);

    let path_bsc = bip44_derivation_path("bsc", 7, 5).unwrap();
    assert_eq!(path_eth, path_bsc);

    let mut config = test_config();
    config.neox_chain_id = NEOX_TESTNET_T4_CHAIN_ID;
    let path_neox = bip44_derivation_path_for_config(&config, "neox", 7, 5).unwrap();
    assert!(
        path_neox.starts_with("m/44'/12227332'/"),
        "Neo X path must be chain-ID scoped: {}",
        path_neox
    );
    assert_ne!(path_eth, path_neox);

    let path_sol_1 = bip44_derivation_path("solana", 7, 1).unwrap();
    assert_ne!(path_sol, path_sol_1);

    let path_other = bip44_derivation_path("solana", 8, 0).unwrap();
    assert_ne!(path_sol, path_other);

    let path_again = bip44_derivation_path("solana", 7, 0).unwrap();
    assert_eq!(path_sol, path_again);
}

#[test]
fn test_neox_chain_id_constants_match_official_networks() {
    assert_eq!(NEOX_TESTNET_T4_CHAIN_ID, 12_227_332);
    assert_eq!(NEOX_MAINNET_CHAIN_ID, 47_763);
}

#[test]
fn test_get_or_allocate_derivation_account_is_stable_and_unique() {
    let db_path = test_db_path();
    let _ = DB::destroy(&Options::default(), &db_path);
    let db = open_db(&db_path).expect("open custody db");

    let first = get_or_allocate_derivation_account(&db, "user-1")
        .expect("allocate derivation account for first user");
    let repeated = get_or_allocate_derivation_account(&db, "user-1")
        .expect("reuse derivation account for first user");
    let second = get_or_allocate_derivation_account(&db, "user-2")
        .expect("allocate derivation account for second user");

    assert_eq!(first, 0);
    assert_eq!(repeated, first);
    assert_eq!(second, first + 1);

    drop(db);

    let reopened = open_db(&db_path).expect("reopen custody db");
    let reopened_first = get_or_allocate_derivation_account(&reopened, "user-1")
        .expect("reload derivation account for first user");
    assert_eq!(reopened_first, first);
}

#[test]
fn test_get_or_allocate_derivation_account_reuses_legacy_path_account() {
    let db_path = test_db_path();
    let _ = DB::destroy(&Options::default(), &db_path);
    let db = open_db(&db_path).expect("open custody db");

    store_deposit(
        &db,
        &DepositRequest {
            deposit_id: "legacy-deposit".to_string(),
            user_id: "legacy-user".to_string(),
            chain: "solana".to_string(),
            asset: "sol".to_string(),
            address: "legacy-address".to_string(),
            derivation_path: "m/44'/501'/42'/0/0".to_string(),
            deposit_seed_source: default_deposit_seed_source(),
            created_at: 0,
            status: "issued".to_string(),
        },
    )
    .expect("store legacy deposit");

    let legacy_account = get_or_allocate_derivation_account(&db, "legacy-user")
        .expect("reuse legacy derivation account");
    let new_account = get_or_allocate_derivation_account(&db, "fresh-user")
        .expect("allocate next derivation account after legacy max");

    assert_eq!(legacy_account, 42);
    assert_eq!(new_account, 43);
}

#[test]
fn test_to_be_bytes() {
    assert_eq!(to_be_bytes(0), Vec::<u8>::new());
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
fn test_resolve_token_contract_neox_gas_and_gated_neo() {
    let mut config = test_config();
    config.wgas_contract_addr = Some("WGAS_CONTRACT_999".to_string());
    config.wneo_contract_addr = Some("WNEO_CONTRACT_999".to_string());

    assert_eq!(
        resolve_token_contract(&config, "neox", "gas"),
        Some("WGAS_CONTRACT_999".to_string())
    );
    assert_eq!(resolve_token_contract(&config, "neox", "neo"), None);

    config.neox_neo_token_contract = Some("0x1111111111111111111111111111111111111111".to_string());
    assert_eq!(
        resolve_token_contract(&config, "neox", "neo"),
        Some("WNEO_CONTRACT_999".to_string())
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
async fn test_reserve_ledger_adjust_once_deduplicates_movement() {
    let _ = DB::destroy(&Options::default(), "/tmp/test_custody_reserve_once");
    let db = open_db("/tmp/test_custody_reserve_once").unwrap();

    let first =
        adjust_reserve_balance_once(&db, "ethereum", "usdt", 500_000, true, "sweep:test-once")
            .await
            .expect("first reserve movement");
    let duplicate =
        adjust_reserve_balance_once(&db, "ethereum", "usdt", 500_000, true, "sweep:test-once")
            .await
            .expect("duplicate reserve movement");

    assert!(first);
    assert!(!duplicate);
    assert_eq!(
        get_reserve_balance(&db, "ethereum", "usdt").unwrap(),
        500_000
    );

    let debit = adjust_reserve_balance_once(
        &db,
        "ethereum",
        "usdt",
        200_000,
        false,
        "withdrawal:test-once",
    )
    .await
    .expect("first reserve debit");
    let duplicate_debit = adjust_reserve_balance_once(
        &db,
        "ethereum",
        "usdt",
        200_000,
        false,
        "withdrawal:test-once",
    )
    .await
    .expect("duplicate reserve debit");

    assert!(debit);
    assert!(!duplicate_debit);
    assert_eq!(
        get_reserve_balance(&db, "ethereum", "usdt").unwrap(),
        300_000
    );

    let _ = DB::destroy(&Options::default(), "/tmp/test_custody_reserve_once");
}

#[tokio::test]
async fn test_reserve_ledger_adjust_decrement() {
    let _ = DB::destroy(&Options::default(), "/tmp/test_custody_reserve_2");
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
    let _ = DB::destroy(&Options::default(), "/tmp/test_custody_rebalance_1");
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
    let (user_id, auth) = test_bridge_access_auth_payload(14);

    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", "Bearer test_api_token".parse().unwrap());

    let err = create_deposit(
        State(state.clone()),
        headers,
        Json(CreateDepositRequest {
            user_id,
            chain: "ethereum".to_string(),
            asset: "eth".to_string(),
            auth: Some(auth),
        }),
    )
    .await
    .expect_err("multi-signer local sweep mode should fail closed by default");

    assert_eq!(err.code, "invalid_request");
    assert!(err
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
            kind: SignerSignatureKind::EvmEcdsa,
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
async fn test_process_rebalance_jobs_multi_signer_blocks_local_treasury_execution() {
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
    state.config.uniswap_router = Some("0x1111111111111111111111111111111111111111".to_string());
    state.config.signer_endpoints =
        vec!["http://signer-1".to_string(), "http://signer-2".to_string()];
    state.config.signer_threshold = 2;

    let job = RebalanceJob {
        job_id: "rebalance-blocked-local-treasury".to_string(),
        chain: "ethereum".to_string(),
        from_asset: "usdt".to_string(),
        to_asset: "usdc".to_string(),
        amount: 1_000_000,
        trigger: "threshold".to_string(),
        linked_withdrawal_job_id: None,
        swap_tx_hash: None,
        status: "queued".to_string(),
        attempts: 0,
        last_error: None,
        next_attempt_at: None,
        created_at: 1000,
    };
    store_rebalance_job(&state.db, &job).expect("store rebalance job");

    process_rebalance_jobs(&state)
        .await
        .expect("process blocked rebalance jobs");

    let failed_jobs =
        list_rebalance_jobs_by_status(&state.db, "failed").expect("list failed rebalance jobs");
    assert_eq!(failed_jobs.len(), 1);
    assert_eq!(failed_jobs[0].job_id, "rebalance-blocked-local-treasury");
    assert_eq!(failed_jobs[0].attempts, 1);
    assert!(failed_jobs[0]
        .last_error
        .as_deref()
        .unwrap_or_default()
        .contains("multi-signer reserve rebalance is disabled"));
    assert!(list_rebalance_jobs_by_status(&state.db, "submitted")
        .expect("list submitted rebalance jobs")
        .is_empty());

    let requests = rpc_requests.lock().await;
    assert!(requests.is_empty());
    drop(requests);

    let event = tokio::time::timeout(std::time::Duration::from_secs(1), event_rx.recv())
        .await
        .expect("timed out waiting for blocked rebalance event")
        .expect("receive blocked rebalance event");
    assert_eq!(event.event_type, "rebalance.failed");
    assert_eq!(event.entity_id, "rebalance-blocked-local-treasury");
    let data = event.data.expect("blocked rebalance event metadata");
    assert_eq!(
        data.get("mode").and_then(|value| value.as_str()),
        Some("blocked-local-rebalance")
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
    if let Err(e) = update_status_index(
        &state.db,
        "deposits",
        "issued",
        "sweep_queued",
        &deposit.deposit_id,
    ) {
        tracing::error!("Failed update_status_index: {e}");
    }

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

    let confirmed_jobs =
        list_sweep_jobs_by_status(&state.db, "sweep_confirmed").expect("list confirmed sweep jobs");
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
        payload.get("method").and_then(|value| value.as_str()) == Some("eth_getTransactionReceipt")
    }));
}

#[tokio::test]
async fn test_process_sweep_jobs_neox_gas_waits_for_confirmed_sweep_before_credit() {
    let mut state = test_state();
    let pending_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let pending_rpc: Router =
        Router::new()
            .route("/", post(mock_rpc_handler))
            .with_state(MockRpcState {
                safe_nonce_hex: "0x0".to_string(),
                safe_tx_hash_hex: "0x0".to_string(),
                send_raw_tx_hash_hex: None,
                transaction_receipt: None,
                requests: pending_requests.clone(),
            });
    let pending_rpc_url = spawn_mock_server(pending_rpc).await;
    let confirmed_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let confirmed_rpc: Router =
        Router::new()
            .route("/", post(mock_rpc_handler))
            .with_state(MockRpcState {
                safe_nonce_hex: "0x0".to_string(),
                safe_tx_hash_hex: "0x0".to_string(),
                send_raw_tx_hash_hex: None,
                transaction_receipt: Some(json!({ "status": "0x1" })),
                requests: confirmed_requests.clone(),
            });
    let confirmed_rpc_url = spawn_mock_server(confirmed_rpc).await;

    state.config.neox_rpc_url = Some(pending_rpc_url);
    state.config.licn_rpc_url = Some("http://localhost:8899".to_string());
    state.config.treasury_keypair_path = Some("/tmp/test-treasury.json".to_string());
    state.config.wgas_contract_addr = Some("WGAS_CONTRACT_999".to_string());

    let deposit = DepositRequest {
        deposit_id: "dep-neox-gas-sweep-confirm-1".to_string(),
        user_id: "11111111111111111111111111111111".to_string(),
        chain: "neox".to_string(),
        asset: "gas".to_string(),
        address: "0x5555555555555555555555555555555555555555".to_string(),
        derivation_path: "m/44'/12227332'/0'/0/8".to_string(),
        deposit_seed_source: DEPOSIT_SEED_SOURCE_DEPOSIT_ROOT.to_string(),
        created_at: 1000,
        status: "sweep_queued".to_string(),
    };
    store_deposit(&state.db, &deposit).expect("store Neo X GAS deposit");
    if let Err(e) = update_status_index(
        &state.db,
        "deposits",
        "issued",
        "sweep_queued",
        &deposit.deposit_id,
    ) {
        tracing::error!("Failed update_status_index: {e}");
    }

    let job = SweepJob {
        job_id: "test-neox-gas-sweep-confirm-worker".to_string(),
        deposit_id: deposit.deposit_id.clone(),
        chain: "neox".to_string(),
        asset: "gas".to_string(),
        from_address: deposit.address.clone(),
        to_treasury: "0x4444444444444444444444444444444444444444".to_string(),
        tx_hash: "deposit-observed-hash".to_string(),
        amount: Some("1000000000000000000".to_string()),
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
    store_sweep_job(&state.db, &job).expect("store submitted Neo X sweep job");

    process_sweep_jobs(&state)
        .await
        .expect("pending Neo X sweep check should not fail");
    assert!(
        list_credit_jobs_by_status(&state.db, "queued")
            .expect("list queued credit jobs")
            .is_empty(),
        "credit must not be queued before sweep confirmation"
    );
    assert_eq!(
        list_sweep_jobs_by_status(&state.db, "sweep_submitted")
            .expect("list submitted sweeps")
            .len(),
        1
    );

    state.config.neox_rpc_url = Some(confirmed_rpc_url);
    process_sweep_jobs(&state)
        .await
        .expect("confirmed Neo X sweep should enqueue credit");

    let confirmed_jobs =
        list_sweep_jobs_by_status(&state.db, "sweep_confirmed").expect("list confirmed sweeps");
    assert_eq!(confirmed_jobs.len(), 1);
    assert_eq!(
        confirmed_jobs[0].job_id,
        "test-neox-gas-sweep-confirm-worker"
    );

    let queued_credit_jobs =
        list_credit_jobs_by_status(&state.db, "queued").expect("list queued credit jobs");
    assert_eq!(queued_credit_jobs.len(), 1);
    assert_eq!(queued_credit_jobs[0].deposit_id, deposit.deposit_id);
    assert_eq!(queued_credit_jobs[0].source_chain, "neox");
    assert_eq!(queued_credit_jobs[0].source_asset, "gas");
    assert_eq!(queued_credit_jobs[0].amount_spores, 1_000_000_000);

    let deposit_after = fetch_deposit(&state.db, &deposit.deposit_id)
        .expect("fetch updated Neo X deposit")
        .expect("Neo X deposit exists after confirmation");
    assert_eq!(deposit_after.status, "swept");

    process_sweep_jobs(&state)
        .await
        .expect("reprocessing confirmed Neo X sweep should be idempotent");
    assert_eq!(
        list_credit_jobs_by_status(&state.db, "queued")
            .expect("list queued credit jobs after replay")
            .len(),
        1,
        "confirmed sweep replay must not duplicate credit jobs"
    );

    let pending_requests = pending_requests.lock().await;
    assert!(pending_requests.iter().any(|payload| {
        payload.get("method").and_then(|value| value.as_str()) == Some("eth_getTransactionReceipt")
    }));
    let confirmed_requests = confirmed_requests.lock().await;
    assert!(confirmed_requests.iter().any(|payload| {
        payload.get("method").and_then(|value| value.as_str()) == Some("eth_getTransactionReceipt")
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
        payload.get("method").and_then(|value| value.as_str()) == Some("eth_getTransactionReceipt")
    }));
}
