use super::*;

#[test]
fn test_determine_withdrawal_signing_mode_self_custody() {
    let state = test_state();
    let job = test_withdrawal_job();

    let mode = determine_withdrawal_signing_mode(&state, &job, "sol").unwrap();

    assert_eq!(mode, None);
}

#[test]
fn test_determine_withdrawal_signing_mode_routes_single_signer_solana_to_pq_approval() {
    let mut state = test_state();
    state.config.signer_endpoints = vec!["http://signer-1".to_string()];
    state.config.signer_threshold = 1;
    state.config.signer_pq_addresses = vec![test_pq_signer(1).0];
    let job = test_withdrawal_job();

    let mode = determine_withdrawal_signing_mode(&state, &job, "sol").unwrap();

    assert_eq!(mode, Some(WithdrawalSigningMode::PqApprovalQuorum));
}

#[test]
fn test_determine_withdrawal_signing_mode_rejects_multi_signer_solana_threshold_mode() {
    let mut state = test_state();
    state.config.signer_endpoints = vec![
        "http://signer-1".to_string(),
        "http://signer-2".to_string(),
        "http://signer-3".to_string(),
    ];
    state.config.signer_threshold = 2;
    state.config.signer_pq_addresses = vec![
        test_pq_signer(4).0,
        test_pq_signer(5).0,
        test_pq_signer(6).0,
    ];
    let mut job = test_withdrawal_job();
    job.asset = "lUSD".to_string();
    job.amount = 1_000_000_000;

    let err = determine_withdrawal_signing_mode(&state, &job, "usdt")
        .expect_err("multi-signer Solana withdrawals must fail closed");

    assert!(err.contains("threshold Solana withdrawals are disabled"));
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
    state.config.signer_pq_addresses = vec![
        test_pq_signer(7).0,
        test_pq_signer(8).0,
        test_pq_signer(9).0,
    ];
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

#[tokio::test]
async fn test_collect_pq_withdrawal_approvals_for_solana() {
    let mut state = test_state();
    let (signer_one_addr, signer_one_seed) = test_pq_signer(11);
    let (signer_two_addr, signer_two_seed) = test_pq_signer(12);
    let signer_one_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let signer_two_requests = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let signer_one = spawn_mock_server(
        Router::new()
            .route("/sign", post(mock_pq_signer_handler))
            .with_state(MockPqSignerState {
                seed: signer_one_seed,
                requests: signer_one_requests.clone(),
            }),
    )
    .await;
    let signer_two = spawn_mock_server(
        Router::new()
            .route("/sign", post(mock_pq_signer_handler))
            .with_state(MockPqSignerState {
                seed: signer_two_seed,
                requests: signer_two_requests.clone(),
            }),
    )
    .await;

    state.config.signer_endpoints = vec![signer_one, signer_two];
    state.config.signer_threshold = 2;
    state.config.signer_pq_addresses = vec![signer_one_addr, signer_two_addr];

    let mut job = test_withdrawal_job();
    let sig_count = collect_pq_withdrawal_approvals(&state, &mut job, "sol", 2)
        .await
        .expect("collect PQ approvals");

    assert_eq!(sig_count, 2);
    assert_eq!(
        valid_pq_withdrawal_approvers(&state, &job, "sol")
            .unwrap()
            .len(),
        2
    );
    assert_eq!(job.signatures.len(), 2);
    assert!(job
        .signatures
        .iter()
        .all(|signature| signature.kind == SignerSignatureKind::PqApproval));
    assert_eq!(signer_one_requests.lock().await.len(), 1);
    assert_eq!(signer_two_requests.lock().await.len(), 1);
    assert!(signer_one_requests.lock().await[0]
        .get("message_hex")
        .is_some());
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
    state.config.signer_pq_addresses = vec![test_pq_signer(21).0, test_pq_signer(22).0];
    state.config.evm_multisig_address =
        Some("0x9999999999999999999999999999999999999999".to_string());

    let mut job = test_withdrawal_job();
    job.dest_chain = "ethereum".to_string();
    job.asset = "wETH".to_string();
    job.dest_address = "0x3333333333333333333333333333333333333333".to_string();
    job.amount = 2_000_000_000;

    let sig_count = collect_threshold_evm_withdrawal_signatures(&state, &mut job, "eth", 2)
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
            kind: SignerSignatureKind::EvmEcdsa,
            signer_pubkey: "1111111111111111111111111111111111111111".to_string(),
            signature: format!("{}1b", "11".repeat(64)),
            message_hash: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                .to_string(),
            received_at: 0,
        },
        SignerSignature {
            kind: SignerSignatureKind::EvmEcdsa,
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
    state.config.signer_pq_addresses = vec![test_pq_signer(23).0, test_pq_signer(24).0];
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
            kind: SignerSignatureKind::EvmEcdsa,
            signer_pubkey: "1111111111111111111111111111111111111111".to_string(),
            signature: format!("{}1b", "11".repeat(64)),
            message_hash: safe_tx_hash_hex.trim_start_matches("0x").to_string(),
            received_at: 0,
        },
        SignerSignature {
            kind: SignerSignatureKind::EvmEcdsa,
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
        build_threshold_solana_withdrawal_message(&state, &job, "usdt", &recent_blockhash).unwrap();

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
    let address = derive_deposit_address("bnb", "usdt", "m/44'/60'/0'/0/0", "test_seed").unwrap();
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
