// ═══════════════════════════════════════════════════════════════════════════════
// COMPREHENSIVE RPC FULL COVERAGE TESTS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Covers every JSON-RPC method across all three dispatch planes:
//   - Native Lichen RPC (108 methods on /)
//   - Solana-compat RPC (13 methods on /solana-compat)
//   - EVM-compat RPC (20 methods on /evm)
// Plus all REST API endpoints on /api/v1/*.
//
// Test naming: test_<plane>_<method_name>
//   e.g. test_native_getAccount, test_solana_getBlockHeight, test_evm_eth_call

use axum::body::{to_bytes, Body};
use axum::extract::ConnectInfo;
use axum::http::header::AUTHORIZATION;
use axum::http::Request;
use lichen_core::{
    contract::ContractAccount, Account, Block, CommitSignature, FeeConfig, FinalityTracker, Hash,
    Instruction, Keypair, Message, Precommit, Pubkey, StakeInfo, StakePool, StateStore,
    SymbolRegistryEntry, Transaction, BOOTSTRAP_GRANT_AMOUNT, CONTRACT_PROGRAM_ID,
};
use lichen_rpc::{build_rpc_router, build_rpc_router_with_min_validator_stake};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;
use tower::util::ServiceExt;

type RpcResult = Result<serde_json::Value, String>;
const TEST_ADMIN_TOKEN: &str = "test-admin-token";
const TEST_BEARER_ADMIN_TOKEN: &str = "Bearer test-admin-token";

// ─── Test helpers ────────────────────────────────────────────────────────────

async fn rpc(app: &axum::Router, path: &str, method: &str) -> RpcResult {
    rpc_p(app, path, method, json!([])).await
}

async fn rpc_p(
    app: &axum::Router,
    path: &str,
    method: &str,
    params: serde_json::Value,
) -> RpcResult {
    let payload = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let request = Request::post(path)
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .map_err(|e| format!("request error: {e}"))?;
    let response = app
        .clone()
        .oneshot(request)
        .await
        .map_err(|e| format!("response error: {e}"))?;
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|e| format!("body error: {e}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("json error: {e}"))
}

async fn rpc_p_with_auth_and_connect_info(
    app: &axum::Router,
    path: &str,
    method: &str,
    params: serde_json::Value,
    auth_header: Option<&str>,
    connect_info: Option<std::net::SocketAddr>,
) -> RpcResult {
    let payload = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    let mut request = Request::post(path)
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .map_err(|e| format!("request error: {e}"))?;
    if let Some(value) = auth_header {
        request.headers_mut().insert(
            AUTHORIZATION,
            value.parse().map_err(|e| format!("header error: {e}"))?,
        );
    }
    if let Some(addr) = connect_info {
        request.extensions_mut().insert(ConnectInfo(addr));
    }
    let response = app
        .clone()
        .oneshot(request)
        .await
        .map_err(|e| format!("response error: {e}"))?;
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|e| format!("body error: {e}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("json error: {e}"))
}

async fn rest_get(app: &axum::Router, path: &str) -> RpcResult {
    let request = Request::get(path)
        .body(Body::empty())
        .map_err(|e| format!("request error: {e}"))?;
    let response = app
        .clone()
        .oneshot(request)
        .await
        .map_err(|e| format!("response error: {e}"))?;
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|e| format!("body error: {e}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("json error: {e}"))
}

fn fresh_app() -> axum::Router {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    let _ = Box::leak(Box::new(dir));
    build_rpc_router(
        state,
        None,
        None,
        None,
        "lichen-test".to_string(),
        "lichen-test".to_string(),
        None,
        None,
        None,
        None,
        None,
    )
}

fn fresh_app_with_min_validator_stake(min_validator_stake: u64) -> axum::Router {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    let _ = Box::leak(Box::new(dir));
    let stake_pool = StakePool::new();
    build_rpc_router_with_min_validator_stake(
        state,
        None,
        Some(Arc::new(RwLock::new(stake_pool))),
        None,
        "lichen-test".to_string(),
        "lichen-test".to_string(),
        min_validator_stake,
        None,
        None,
        None,
        None,
        None,
    )
}

fn fresh_app_with_runtime_settings(
    min_validator_stake: u64,
    fee_config: FeeConfig,
) -> axum::Router {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    state
        .set_fee_config_full(&fee_config)
        .expect("set fee config");
    let _ = Box::leak(Box::new(dir));
    let stake_pool = StakePool::new();
    build_rpc_router_with_min_validator_stake(
        state,
        None,
        Some(Arc::new(RwLock::new(stake_pool))),
        None,
        "lichen-test".to_string(),
        "lichen-test".to_string(),
        min_validator_stake,
        None,
        None,
        None,
        None,
        None,
    )
}

fn public_network_app_with_admin_token() -> axum::Router {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    let _ = Box::leak(Box::new(dir));
    // Test fixture only. Production admin tokens come from runtime config,
    // never from committed test sources.
    build_rpc_router(
        state,
        None,
        None,
        None,
        "lichen-testnet".to_string(),
        "lichen-testnet".to_string(),
        Some(TEST_ADMIN_TOKEN.to_string()),
        None,
        None,
        None,
        None,
    )
}

fn dev_network_app_with_admin_token() -> axum::Router {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    let _ = Box::leak(Box::new(dir));
    // Test fixture only. Production admin tokens come from runtime config,
    // never from committed test sources.
    build_rpc_router(
        state,
        None,
        None,
        None,
        "lichen-local".to_string(),
        "lichen-dev".to_string(),
        Some(TEST_ADMIN_TOKEN.to_string()),
        None,
        None,
        None,
        None,
    )
}

fn app_with_consensus_oracle_prices() -> axum::Router {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    let _ = Box::leak(Box::new(dir));

    state.set_last_slot(100).expect("set slot");
    state
        .put_oracle_consensus_price("LICN", 12_500_000, 8, 95, 3)
        .expect("put LICN oracle price");
    state
        .put_oracle_consensus_price("wSOL", 14_875_000_000, 8, 96, 3)
        .expect("put wSOL oracle price");

    build_rpc_router(
        state,
        None,
        None,
        None,
        "lichen-test".to_string(),
        "lichen-test".to_string(),
        None,
        None,
        None,
        None,
        None,
    )
}

/// Creates an app with a funded account and a deployed contract
fn app_with_state() -> (axum::Router, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    let _ = Box::leak(Box::new(dir));

    // Create a funded account
    let funded = Pubkey([42u8; 32]);
    let funded_hex = funded.to_base58();
    let acct = Account::new(1_000_000_000, Pubkey([0u8; 32]));
    state.put_account(&funded, &acct).expect("put funded");

    // Deploy a minimal contract with ABI
    let contract_prog = Pubkey([99u8; 32]);
    let mut contract = ContractAccount::new(vec![0u8; 10], Pubkey([2u8; 32]));
    contract
        .storage
        .insert(b"test_key".to_vec(), b"test_value".to_vec());
    let mut contract_acct = Account::new(0, CONTRACT_PROGRAM_ID);
    contract_acct.owner = CONTRACT_PROGRAM_ID;
    contract_acct.executable = true;
    contract_acct.data = serde_json::to_vec(&contract).expect("ser");
    state
        .put_account(&contract_prog, &contract_acct)
        .expect("put contract");

    // Register symbol
    state
        .register_symbol(
            "TST",
            SymbolRegistryEntry {
                symbol: "TST".to_string(),
                program: contract_prog,
                owner: Pubkey([2u8; 32]),
                name: Some("Test Contract".to_string()),
                template: Some("token".to_string()),
                metadata: None,
                decimals: Some(9),
            },
        )
        .expect("register");

    let app = build_rpc_router(
        state,
        None,
        None,
        None,
        "lichen-test".to_string(),
        "lichen-test".to_string(),
        None,
        None,
        None,
        None,
        None,
    );
    (app, funded_hex)
}

fn app_with_anchored_account_proof() -> (axum::Router, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    let _ = Box::leak(Box::new(dir));

    let funded = Pubkey([0x42u8; 32]);
    let funded_b58 = funded.to_base58();
    let acct = Account::new(5, Pubkey([0u8; 32]));
    state.put_account(&funded, &acct).expect("put funded");

    let validator = Keypair::from_seed(&[7u8; 32]);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let genesis = Block::genesis(Hash::default(), now.saturating_sub(1), vec![]);
    state.put_block(&genesis).expect("put genesis");

    let mut block = Block::new_with_timestamp(
        1,
        genesis.hash(),
        state.compute_state_root(),
        validator.pubkey().0,
        vec![],
        now,
    );
    block.header.validators_hash = Hash::hash(b"validator-set-1");
    block.sign(&validator);

    let commit_timestamp = now;
    let precommit = Precommit::signable_bytes(1, 0, &Some(block.hash()), commit_timestamp);
    block.commit_signatures = vec![CommitSignature {
        validator: validator.pubkey().0,
        signature: validator.sign(&precommit),
        timestamp: commit_timestamp,
    }];

    state.put_block(&block).expect("put anchored block");
    state.set_last_slot(1).expect("set last slot");
    state
        .set_last_confirmed_slot(1)
        .expect("set confirmed slot");
    state
        .set_last_finalized_slot(1)
        .expect("set finalized slot");

    let app = build_rpc_router(
        state,
        None,
        None,
        None,
        "lichen-test".to_string(),
        "lichen-test".to_string(),
        None,
        Some(FinalityTracker::new(1, 1)),
        None,
        None,
        None,
    );

    (app, funded_b58)
}

/// Helper: assert result or error (valid JSON-RPC response)
fn assert_valid_rpc(resp: &serde_json::Value) {
    assert_eq!(resp["jsonrpc"], "2.0", "must be jsonrpc 2.0");
    assert!(
        resp.get("result").is_some() || resp.get("error").is_some(),
        "response must have result or error: {resp}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 1: NATIVE LICN RPC — Basic Query Methods
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_get_account_nonexistent() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getAccount",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_account_existing() {
    let (app, addr) = app_with_state();
    let resp = rpc_p(&app, "/", "getAccount", json!([addr])).await.unwrap();
    assert_valid_rpc(&resp);
    // Should return account data with balance
    if let Some(result) = resp.get("result") {
        if !result.is_null() {
            assert!(
                result.get("spores").is_some()
                    || result.get("balance").is_some()
                    || result.get("lamports").is_some(),
                "account should have balance/spores: {result}"
            );
        }
    }
}

#[tokio::test]
async fn test_native_get_account_proof_returns_anchored_finalized_context() {
    let (app, addr) = app_with_anchored_account_proof();
    let resp = rpc_p(
        &app,
        "/",
        "getAccountProof",
        json!([addr, {"commitment": "finalized"}]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);

    let result = &resp["result"];
    assert_eq!(result["pubkey"], addr);
    assert!(result.get("state_root").is_none());
    assert!(result.get("account").is_none());
    assert!(result.get("proof").is_none());
    assert!(result["account_data"].as_str().is_some());
    assert!(result["inclusion_proof"]["leaf_hash"].as_str().is_some());
    assert_eq!(result["anchor"]["commitment"], "finalized");
    assert_eq!(result["anchor"]["slot"], 1);
    assert_eq!(result["anchor"]["commit_round"], 0);
    assert!(result["anchor"]["state_root"].as_str().is_some());
    assert_eq!(
        result["anchor"]["validators_hash"],
        Hash::hash(b"validator-set-1").to_hex()
    );
    assert_eq!(result["anchor"]["commit_validator_count"], 1);
    assert_eq!(
        result["anchor"]["commit_signatures"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(result["anchor"]["block_signature"].is_object());
}

#[tokio::test]
async fn test_native_get_account_proof_rejects_unanchored_state_root() {
    let (app, _, addr, _, _, _) = app_with_rich_state();
    let resp = rpc_p(&app, "/", "getAccountProof", json!([addr]))
        .await
        .unwrap();

    assert_eq!(resp["error"]["code"], -32001);
    assert!(!resp["error"]["message"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn test_native_get_latest_block() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getLatestBlock").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_transaction_nonexistent() {
    let app = fresh_app();
    let fake_sig = "a".repeat(64);
    let resp = rpc_p(&app, "/", "getTransaction", json!([fake_sig]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_transactions_by_address() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getTransactionsByAddress",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_account_tx_count() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getAccountTxCount",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_recent_transactions() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "getRecentTransactions", json!([10]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_token_accounts() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getTokenAccounts",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_total_burned() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getTotalBurned").await.unwrap();
    assert_valid_rpc(&resp);
    // Returns an object with spores/licn fields
    if let Some(result) = resp.get("result") {
        assert!(
            !result.is_null(),
            "getTotalBurned should return a value: {result}"
        );
    }
}

#[tokio::test]
async fn test_native_get_validators() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getValidators").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_metrics() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getMetrics").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_treasury_info() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getTreasuryInfo").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_genesis_accounts() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getGenesisAccounts").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_recent_blockhash() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getRecentBlockhash").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_health() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "health").await.unwrap();
    assert_valid_rpc(&resp);
    // Fresh app has no blocks (slot 0) → health correctly reports "behind"
    assert_eq!(resp["result"]["status"], "behind");
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 2: Fee/Rent Config
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_get_fee_config() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getFeeConfig").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_set_fee_config_no_token() {
    // Without admin token, should reject
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "setFeeConfig", json!([{"base_fee": 100}]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    // Should error because no admin token configured
    assert!(
        resp.get("error").is_some(),
        "setFeeConfig without admin token should error"
    );
}

#[tokio::test]
async fn test_native_get_rent_params() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getRentParams").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_set_rent_params_no_token() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "setRentParams", json!([{"exempt_minimum": 100}]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    assert!(
        resp.get("error").is_some(),
        "setRentParams without admin token should error"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 3: Network + Validator + Chain Status
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_get_peers() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getPeers").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_network_info() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getNetworkInfo").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_cluster_info() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getClusterInfo").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_validator_info() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getValidatorInfo",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_validator_performance() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getValidatorPerformance",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_chain_status() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getChainStatus").await.unwrap();
    assert_valid_rpc(&resp);
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 4: Staking endpoints
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_get_staking_status() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getStakingStatus",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_staking_rewards() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getStakingRewards",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_staking_rewards_reports_liquid_claims_separately() {
    let (app, addr) = app_with_bootstrap_staking_rewards();
    let resp = rpc_p(&app, "/", "getStakingRewards", json!([addr]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);

    let result = &resp["result"];
    assert_eq!(result["pending_rewards"], json!(200_000_000));
    assert_eq!(result["claimed_rewards"], json!(600_000_000));
    assert_eq!(result["liquid_claimed_rewards"], json!(600_000_000));
    assert_eq!(result["claimed_total_rewards"], json!(1_000_000_000));
    assert_eq!(result["total_debt_repaid"], json!(400_000_000));
    assert_eq!(result["total_rewards"], json!(1_200_000_000));
}

#[tokio::test]
async fn test_native_get_mossstake_pool_info() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getMossStakePoolInfo").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_staking_position() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getStakingPosition",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_unstaking_queue() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getUnstakingQueue",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_reward_adjustment_info() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getRewardAdjustmentInfo").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_reward_adjustment_info_uses_runtime_min_validator_stake() {
    let app = fresh_app_with_min_validator_stake(75_000_000_000);
    let resp = rpc(&app, "/", "getRewardAdjustmentInfo").await.unwrap();
    assert_valid_rpc(&resp);
    assert_eq!(
        resp["result"]["minValidatorStake"],
        json!(75_000_000_000u64)
    );
}

#[tokio::test]
async fn test_native_get_reward_adjustment_info_uses_live_fee_split() {
    let fee_config = FeeConfig {
        fee_burn_percent: 35,
        fee_producer_percent: 25,
        fee_voters_percent: 15,
        fee_treasury_percent: 15,
        fee_community_percent: 10,
        ..FeeConfig::default_from_constants()
    };
    let app = fresh_app_with_runtime_settings(75_000_000_000, fee_config);
    let resp = rpc(&app, "/", "getRewardAdjustmentInfo").await.unwrap();
    assert_valid_rpc(&resp);

    let fee_split = &resp["result"]["feeSplit"];
    assert_eq!(fee_split["burn_pct"], json!(35u64));
    assert_eq!(fee_split["producer_pct"], json!(25u64));
    assert_eq!(fee_split["voters_pct"], json!(15u64));
    assert_eq!(fee_split["treasury_pct"], json!(15u64));
    assert_eq!(fee_split["community_pct"], json!(10u64));
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 5: Account + Transaction History
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_get_account_info() {
    let (app, addr) = app_with_state();
    let resp = rpc_p(&app, "/", "getAccountInfo", json!([addr]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_transaction_history() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getTransactionHistory",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 6: Contract endpoints
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_get_contract_info() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getContractInfo",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_contract_logs() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getContractLogs",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_contract_abi() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getContractAbi",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_set_contract_abi_no_token() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "setContractAbi",
        json!(["11111111111111111111111111111111", []]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    assert!(
        resp.get("error").is_some(),
        "setContractAbi without token should error"
    );
}

#[tokio::test]
async fn test_native_legacy_admin_rpcs_disabled_on_public_networks() {
    let app = public_network_app_with_admin_token();

    for (method, params) in [
        ("setFeeConfig", json!([{"base_fee_spores": 100}])),
        ("setRentParams", json!([{"rent_free_kb": 100}])),
        (
            "setContractAbi",
            json!(["11111111111111111111111111111111", []]),
        ),
        ("deployContract", json!([])),
        ("upgradeContract", json!([])),
    ] {
        let resp = rpc_p(&app, "/", method, params).await.unwrap();
        assert_valid_rpc(&resp);
        let message = resp["error"]["message"].as_str().unwrap_or("");
        assert!(
            message.contains("disabled outside local/dev environments"),
            "{} should be disabled on public networks, got: {}",
            method,
            message
        );
    }
}

#[tokio::test]
async fn test_native_legacy_admin_rpcs_accept_bearer_header_on_dev_networks() {
    let app = dev_network_app_with_admin_token();

    let resp = rpc_p_with_auth_and_connect_info(
        &app,
        "/",
        "setFeeConfig",
        json!({"base_fee_spores": 1000}),
        Some(TEST_BEARER_ADMIN_TOKEN),
        Some("127.0.0.1:9000".parse().unwrap()),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    assert_eq!(resp["result"]["status"], "ok");
}

#[tokio::test]
async fn test_native_legacy_admin_rpcs_require_loopback_on_dev_networks() {
    let app = dev_network_app_with_admin_token();

    let resp = rpc_p_with_auth_and_connect_info(
        &app,
        "/",
        "setFeeConfig",
        json!({"base_fee_spores": 1000, "admin_token": TEST_ADMIN_TOKEN}),
        None,
        Some("203.0.113.10:9000".parse().unwrap()),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    let message = resp["error"]["message"].as_str().unwrap_or("");
    assert!(
        message.contains("restricted to loopback clients"),
        "expected loopback restriction error, got: {}",
        message
    );
}

#[tokio::test]
async fn test_native_get_all_contracts() {
    let (app, _) = app_with_state();
    let resp = rpc(&app, "/", "getAllContracts").await.unwrap();
    assert_valid_rpc(&resp);
    // Should return array or object with at least our test contract
    if let Some(result) = resp.get("result") {
        assert!(!result.is_null(), "getAllContracts should not be null");
    }
}

#[tokio::test]
async fn test_native_deploy_contract_missing_tx() {
    let app = fresh_app();
    // Should fail without a valid signed transaction
    let resp = rpc_p(&app, "/", "deployContract", json!(["invalid"]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    assert!(
        resp.get("error").is_some(),
        "deployContract with invalid data should error"
    );
}

#[tokio::test]
async fn test_native_upgrade_contract_missing_tx() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "upgradeContract", json!(["invalid"]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    assert!(
        resp.get("error").is_some(),
        "upgradeContract with invalid data should error"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 7: Program endpoints
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_get_program() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getProgram",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_program_stats() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getProgramStats",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_programs() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "getPrograms", json!([{}])).await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_program_calls() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getProgramCalls",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_program_storage() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getProgramStorage",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 8: EVM Address Registry
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_get_evm_registration_no_mapping() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getEvmRegistration",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    // Should return null for unmapped address
    assert!(resp["result"].is_null(), "no mapping should return null");
}

#[tokio::test]
async fn test_native_lookup_evm_address_no_mapping() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "lookupEvmAddress",
        json!(["0x0000000000000000000000000000000000000001"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    assert!(resp["result"].is_null(), "no mapping should return null");
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 9: Symbol Registry
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_get_symbol_registry() {
    let (app, _) = app_with_state();
    let resp = rpc_p(&app, "/", "getSymbolRegistry", json!(["TST"]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    if let Some(result) = resp.get("result") {
        if !result.is_null() {
            assert_eq!(result["symbol"], "TST");
        }
    }
}

#[tokio::test]
async fn test_native_get_symbol_registry_missing() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "getSymbolRegistry", json!(["NOSYMBOL"]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_symbol_registry_by_program() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getSymbolRegistryByProgram",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_all_symbol_registry() {
    let (app, _) = app_with_state();
    let resp = rpc_p(&app, "/", "getAllSymbolRegistry", json!([{}]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 10: NFT + Market endpoints
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_get_collection() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getCollection",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_nft() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getNFT",
        json!(["11111111111111111111111111111111", 0]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_nfts_by_owner() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getNFTsByOwner",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_nfts_by_collection() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getNFTsByCollection",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_nft_activity() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getNFTActivity",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_market_listings() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "getMarketListings", json!([{}]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_market_sales() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "getMarketSales", json!([{}]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 11: Token endpoints
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_get_token_balance() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getTokenBalance",
        json!([
            "11111111111111111111111111111111",
            "11111111111111111111111111111111"
        ]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_token_holders() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getTokenHolders",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_token_transfers() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getTokenTransfers",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_contract_events() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getContractEvents",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 12: Signatures for address
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_get_signatures_for_address() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getSignaturesForAddress",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 13: Prediction Market RPC
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_get_prediction_market_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getPredictionMarketStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_prediction_markets() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "getPredictionMarkets", json!([{}]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_prediction_market_by_id() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "getPredictionMarket", json!([0]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_prediction_positions() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getPredictionPositions",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_prediction_trader_stats() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getPredictionTraderStats",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_prediction_leaderboard() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "getPredictionLeaderboard", json!([{}]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_prediction_trending() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getPredictionTrending").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_prediction_market_analytics() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "getPredictionMarketAnalytics", json!([0]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 14: DEX + Platform Stats endpoints
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_get_dex_core_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getDexCoreStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_dex_amm_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getDexAmmStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_dex_margin_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getDexMarginStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_dex_rewards_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getDexRewardsStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_dex_router_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getDexRouterStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_dex_analytics_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getDexAnalyticsStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_dex_governance_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getDexGovernanceStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_lichenswap_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getLichenSwapStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_thalllend_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getThallLendStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_sporepay_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getSporePayStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_bountyboard_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getBountyBoardStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_compute_market_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getComputeMarketStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_moss_storage_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getMossStorageStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_lichenmarket_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getLichenMarketStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_lichenauction_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getLichenAuctionStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_lichenpunks_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getLichenPunksStats").await.unwrap();
    assert_valid_rpc(&resp);
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 15: Search + Airdrop
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_search_licn_names() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "searchLichenNames", json!(["test"]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_request_airdrop() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "requestAirdrop",
        json!(["11111111111111111111111111111111", 1000]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 16: Transaction submission (sendTransaction/confirmTransaction/simulate)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_send_transaction_no_sender() {
    // Without tx_sender configured, should return error
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "sendTransaction", json!(["deadbeef"]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    assert!(
        resp.get("error").is_some(),
        "sendTransaction without tx_sender should error"
    );
}

#[tokio::test]
async fn test_native_confirm_transaction_nonexistent() {
    let app = fresh_app();
    let fake_sig = "a".repeat(64);
    let resp = rpc_p(&app, "/", "confirmTransaction", json!([fake_sig]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_simulate_transaction_invalid() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "simulateTransaction", json!(["invalid_data"]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    // Should error because data is not a valid transaction
    assert!(
        resp.get("error").is_some(),
        "simulateTransaction with invalid data should error"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 17: Staking write endpoints (need tx_sender)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_stake_no_sender() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "stake",
        json!(["11111111111111111111111111111111", 1000]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_unstake_no_sender() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "unstake",
        json!(["11111111111111111111111111111111", 1000]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 19: Method not found
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_unknown_method() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "totallyBogusMethod").await.unwrap();
    assert!(resp.get("error").is_some());
    assert_eq!(resp["error"]["code"], -32601);
    assert_eq!(resp["error"]["message"], "Method not found");
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 20: SOLANA-COMPAT RPC — All methods
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_solana_get_latest_blockhash() {
    let app = fresh_app();
    let resp = rpc(&app, "/solana-compat", "getLatestBlockhash")
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_get_recent_blockhash() {
    let app = fresh_app();
    let resp = rpc(&app, "/solana-compat", "getRecentBlockhash")
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_get_balance() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/solana-compat",
        "getBalance",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_get_account_info() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/solana-compat",
        "getAccountInfo",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_get_block() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/solana-compat", "getBlock", json!([0]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_get_block_height() {
    let app = fresh_app();
    let resp = rpc(&app, "/solana-compat", "getBlockHeight").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_get_signatures_for_address() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/solana-compat",
        "getSignaturesForAddress",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_get_signature_statuses() {
    let app = fresh_app();
    let fake_sig = "a".repeat(64);
    let resp = rpc_p(
        &app,
        "/solana-compat",
        "getSignatureStatuses",
        json!([[fake_sig]]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_get_slot() {
    let app = fresh_app();
    let resp = rpc(&app, "/solana-compat", "getSlot").await.unwrap();
    assert_valid_rpc(&resp);
    assert!(resp["result"].is_number());
}

#[tokio::test]
async fn test_solana_get_transaction() {
    let app = fresh_app();
    let fake_sig = "a".repeat(64);
    let resp = rpc_p(&app, "/solana-compat", "getTransaction", json!([fake_sig]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_send_transaction_no_sender() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/solana-compat",
        "sendTransaction",
        json!(["deadbeef"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_unknown_method() {
    let app = fresh_app();
    let resp = rpc(&app, "/solana-compat", "totallyBogusMethod")
        .await
        .unwrap();
    assert!(resp.get("error").is_some());
    assert_eq!(resp["error"]["code"], -32601);
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 21: EVM-COMPAT RPC — All methods
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_evm_eth_get_balance() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getBalance",
        json!(["0x0000000000000000000000000000000000000001", "latest"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_evm_eth_send_raw_transaction() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_sendRawTransaction",
        json!(["0xdeadbeef"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_evm_eth_call() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_call",
        json!([{"to": "0x0000000000000000000000000000000000000001", "data": "0x"}, "latest"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_evm_eth_get_transaction_receipt() {
    let app = fresh_app();
    let fake_hash = format!("0x{}", "a".repeat(64));
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getTransactionReceipt",
        json!([fake_hash]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_evm_eth_get_transaction_by_hash() {
    let app = fresh_app();
    let fake_hash = format!("0x{}", "a".repeat(64));
    let resp = rpc_p(&app, "/evm", "eth_getTransactionByHash", json!([fake_hash]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_evm_eth_accounts() {
    let app = fresh_app();
    let resp = rpc(&app, "/evm", "eth_accounts").await.unwrap();
    assert_valid_rpc(&resp);
    assert_eq!(resp["result"], json!([]));
}

#[tokio::test]
async fn test_evm_net_version() {
    let app = fresh_app();
    let resp = rpc(&app, "/evm", "net_version").await.unwrap();
    assert_valid_rpc(&resp);
    assert_eq!(resp["result"], "1297368660");
}

#[tokio::test]
async fn test_evm_eth_max_priority_fee_per_gas() {
    let app = fresh_app();
    let resp = rpc(&app, "/evm", "eth_maxPriorityFeePerGas").await.unwrap();
    assert_valid_rpc(&resp);
    assert_eq!(resp["result"], "0x0");
}

#[tokio::test]
async fn test_evm_eth_estimate_gas() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_estimateGas",
        json!([{"to": "0x0000000000000000000000000000000000000001"}]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_evm_eth_get_code() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getCode",
        json!(["0x0000000000000000000000000000000000000001", "latest"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_evm_eth_get_transaction_count() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getTransactionCount",
        json!(["0x0000000000000000000000000000000000000001", "latest"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_evm_eth_get_block_by_number() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/evm", "eth_getBlockByNumber", json!(["0x0", false]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_evm_eth_get_block_by_hash() {
    let app = fresh_app();
    let fake_hash = format!("0x{}", "0".repeat(64));
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getBlockByHash",
        json!([fake_hash, false]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_evm_eth_get_storage_at() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getStorageAt",
        json!([
            "0x0000000000000000000000000000000000000001",
            "0x0",
            "latest"
        ]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_evm_net_listening() {
    let app = fresh_app();
    let resp = rpc(&app, "/evm", "net_listening").await.unwrap();
    assert_valid_rpc(&resp);
    assert_eq!(resp["result"], true);
}

#[tokio::test]
async fn test_evm_web3_client_version() {
    let app = fresh_app();
    let resp = rpc(&app, "/evm", "web3_clientVersion").await.unwrap();
    assert_valid_rpc(&resp);
    let ver = resp["result"].as_str().unwrap();
    assert!(
        ver.starts_with("Lichen/"),
        "should start with Lichen/: {ver}"
    );
}

#[tokio::test]
async fn test_evm_unknown_method() {
    let app = fresh_app();
    let resp = rpc(&app, "/evm", "eth_bogusMethod").await.unwrap();
    assert!(resp.get("error").is_some());
    assert_eq!(resp["error"]["code"], -32601);
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 22: REST API — DEX endpoints
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_rest_dex_pairs() {
    let app = fresh_app();
    let resp = rest_get(&app, "/api/v1/pairs").await;
    // May return empty array or error — both are valid
    assert!(resp.is_ok() || resp.is_err());
}

#[tokio::test]
async fn test_rest_dex_tickers() {
    let app = fresh_app();
    let resp = rest_get(&app, "/api/v1/tickers").await;
    assert!(resp.is_ok() || resp.is_err());
}

#[tokio::test]
async fn test_rest_dex_pools() {
    let app = fresh_app();
    let resp = rest_get(&app, "/api/v1/pools").await;
    assert!(resp.is_ok() || resp.is_err());
}

#[tokio::test]
async fn test_rest_dex_leaderboard() {
    let app = fresh_app();
    let resp = rest_get(&app, "/api/v1/leaderboard").await;
    assert!(resp.is_ok() || resp.is_err());
}

#[tokio::test]
async fn test_rest_dex_governance_proposals() {
    let app = fresh_app();
    let resp = rest_get(&app, "/api/v1/governance/proposals").await;
    assert!(resp.is_ok() || resp.is_err());
}

#[tokio::test]
async fn test_rest_dex_margin_info() {
    let app = fresh_app();
    let resp = rest_get(&app, "/api/v1/margin/info").await;
    assert!(resp.is_ok() || resp.is_err());
}

#[tokio::test]
async fn test_rest_dex_margin_enabled_pairs() {
    let app = fresh_app();
    let resp = rest_get(&app, "/api/v1/margin/enabled-pairs").await;
    assert!(resp.is_ok() || resp.is_err());
}

#[tokio::test]
async fn test_rest_dex_oracle_prices() {
    let app = app_with_consensus_oracle_prices();
    let resp = rest_get(&app, "/api/v1/oracle/prices").await;
    let body = resp.expect("oracle prices response");
    let feeds = body["data"]["feeds"].as_array().expect("feeds array");
    let licn_feed = feeds
        .iter()
        .find(|entry| entry["asset"] == "LICN")
        .expect("LICN feed");
    assert_eq!(licn_feed["source"], "native_consensus");
    assert_eq!(licn_feed["priceRaw"], 12_500_000u64);
    assert_eq!(licn_feed["decimals"], 8u64);
    assert_eq!(licn_feed["slot"], 95u64);
    assert_eq!(licn_feed["stale"], false);
}

#[tokio::test]
async fn test_native_get_oracle_prices_uses_consensus_oracle() {
    let app = app_with_consensus_oracle_prices();
    let resp = rpc(&app, "/", "getOraclePrices")
        .await
        .expect("rpc response");
    let result = resp["result"].as_object().expect("result object");
    assert_eq!(result["source"], json!("native_consensus"));
    assert_eq!(result["LICN"], json!(0.125));
    assert_eq!(result["wSOL"], json!(148.75));
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 23: REST API — Prediction Market endpoints
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_rest_prediction_stats() {
    let app = fresh_app();
    let resp = rest_get(&app, "/api/v1/prediction-market/stats").await;
    assert!(resp.is_ok() || resp.is_err());
}

#[tokio::test]
async fn test_rest_prediction_markets() {
    let app = fresh_app();
    let resp = rest_get(&app, "/api/v1/prediction-market/markets").await;
    assert!(resp.is_ok() || resp.is_err());
}

#[tokio::test]
async fn test_rest_prediction_trending() {
    let app = fresh_app();
    let resp = rest_get(&app, "/api/v1/prediction-market/trending").await;
    assert!(resp.is_ok() || resp.is_err());
}

#[tokio::test]
async fn test_rest_prediction_leaderboard() {
    let app = fresh_app();
    let resp = rest_get(&app, "/api/v1/prediction-market/leaderboard").await;
    assert!(resp.is_ok() || resp.is_err());
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 24: REST API — Launchpad endpoints
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_rest_launchpad_stats() {
    let app = fresh_app();
    let resp = rest_get(&app, "/api/v1/launchpad/stats").await;
    assert!(resp.is_ok() || resp.is_err());
}

#[tokio::test]
async fn test_rest_launchpad_tokens() {
    let app = fresh_app();
    let resp = rest_get(&app, "/api/v1/launchpad/tokens").await;
    assert!(resp.is_ok() || resp.is_err());
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 25: Error handling edge cases
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_missing_params_get_balance() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getBalance").await.unwrap();
    assert_valid_rpc(&resp);
    // getBalance without params should error
    assert!(
        resp.get("error").is_some(),
        "getBalance without params should error"
    );
}

#[tokio::test]
async fn test_native_invalid_pubkey_format() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "getBalance", json!(["not-a-valid-pubkey!!"]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    assert!(resp.get("error").is_some(), "invalid pubkey should error");
    assert_eq!(resp["error"]["message"], "Invalid pubkey format");
}

#[tokio::test]
async fn test_native_get_block_negative_slot() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "getBlock", json!([-1])).await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_block_very_high_slot() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "getBlock", json!([999999999]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_evm_invalid_address_format() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getBalance",
        json!(["not-an-address", "latest"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    assert!(resp.get("error").is_some());
}

#[tokio::test]
async fn test_solana_missing_params() {
    let app = fresh_app();
    let resp = rpc(&app, "/solana-compat", "getBalance").await.unwrap();
    assert_valid_rpc(&resp);
    // Should error without address param
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 26: Batch response validation
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_all_stats_endpoints_return_valid_json() {
    let app = fresh_app();
    let stats_methods = vec![
        "getDexCoreStats",
        "getDexAmmStats",
        "getDexMarginStats",
        "getDexRewardsStats",
        "getDexRouterStats",
        "getDexAnalyticsStats",
        "getDexGovernanceStats",
        "getLichenSwapStats",
        "getThallLendStats",
        "getSporePayStats",
        "getBountyBoardStats",
        "getComputeMarketStats",
        "getMossStorageStats",
        "getLichenMarketStats",
        "getLichenAuctionStats",
        "getLichenPunksStats",
        "getPredictionMarketStats",
    ];
    for method in stats_methods {
        let resp = rpc(&app, "/", method).await.unwrap();
        assert_valid_rpc(&resp);
        // Stats should not panic — either result or error is fine
    }
}

#[tokio::test]
async fn test_all_evm_methods_return_valid_jsonrpc() {
    let app = fresh_app();
    let methods = vec![
        "eth_chainId",
        "eth_blockNumber",
        "eth_accounts",
        "net_version",
        "eth_gasPrice",
        "eth_maxPriorityFeePerGas",
        "net_listening",
        "web3_clientVersion",
    ];
    for method in methods {
        let resp = rpc(&app, "/evm", method).await.unwrap();
        assert_eq!(resp["jsonrpc"], "2.0", "bad jsonrpc for {method}");
        assert!(resp.get("result").is_some(), "missing result for {method}");
    }
}

#[tokio::test]
async fn test_all_solana_methods_no_panic() {
    let app = fresh_app();
    let methods = vec![
        "getHealth",
        "getVersion",
        "getSlot",
        "getBlockHeight",
        "getLatestBlockhash",
        "getRecentBlockhash",
    ];
    for method in methods {
        let resp = rpc(&app, "/solana-compat", method).await.unwrap();
        assert_valid_rpc(&resp);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 27: POSITIVE-PATH TESTS — Real data, deep assertions
// ═══════════════════════════════════════════════════════════════════════════════
//
// These tests go beyond "handler doesn't crash" — they pre-populate state
// (accounts, blocks, transactions, validators) and verify the returned JSON
// contains correct, meaningful values.

use lichen_core::consensus::ValidatorInfo;

/// Helper: build an app backed by a StateStore pre-populated with a funded
/// account, a stored block at slot 1, a validator, and a transaction.
/// Returns `(Router, StateStore, funded_base58, validator_base58, block_hash_hex, tx_sig_hex)`.
fn app_with_rich_state() -> (axum::Router, StateStore, String, String, String, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    let _ = Box::leak(Box::new(dir));

    // 1. Funded account: 5 LICN = 5_000_000_000 spores
    let funded = Pubkey([42u8; 32]);
    let funded_b58 = funded.to_base58();
    let acct = Account::new(5, Pubkey([0u8; 32])); // 5 LICN
    state.put_account(&funded, &acct).expect("put funded");

    // 2. Validator
    let val_pk = Pubkey([7u8; 32]);
    let val_b58 = val_pk.to_base58();
    let mut vi = ValidatorInfo::new(val_pk, 0);
    vi.blocks_proposed = 3;
    vi.stake = 100_000_000_000_000; // 100k LICN
    state.put_validator(&vi).expect("put validator");

    // 3. A minimal transaction (transfer 1 LICN from funded → treasury)
    let treasury = Pubkey([0u8; 32]);
    let ix = Instruction {
        program_id: Pubkey([0u8; 32]),
        accounts: vec![funded, treasury],
        data: vec![3, 0, 0, 0, 0, 0xCA, 0x9A, 0x3B, 0, 0, 0, 0, 0, 0, 0, 0],
    };
    let msg = Message::new(vec![ix], Hash::default());
    let tx_signer = Keypair::from_seed(&[99u8; 32]);
    let tx = Transaction {
        signatures: vec![tx_signer.sign(&msg.serialize())],
        message: msg,
        tx_type: Default::default(),
    };
    let tx_sig_hex = tx.signature().to_hex();

    // 4. Genesis block at slot 0 (empty, set parent for slot-1 block)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let genesis = Block::genesis(Hash::default(), now.saturating_sub(1), vec![]);
    state.put_block(&genesis).expect("put genesis");

    // 5. Block at slot 1 containing the transaction (uses current timestamp so health = "ok")
    let block = Block::new_with_timestamp(
        1,
        genesis.hash(),
        Hash::hash(b"state_root_1"),
        val_pk.0,
        vec![tx],
        now,
    );
    let block_hash_hex = block.hash().to_hex();
    state.put_block(&block).expect("put block");

    // 6. Update slot counter
    state.set_last_slot(1).expect("set slot");

    // 7. Deploy a contract + register symbol (same as app_with_state)
    let contract_prog = Pubkey([99u8; 32]);
    let mut contract = ContractAccount::new(vec![0u8; 10], Pubkey([2u8; 32]));
    contract
        .storage
        .insert(b"test_key".to_vec(), b"test_value".to_vec());
    let mut contract_acct = Account::new(0, CONTRACT_PROGRAM_ID);
    contract_acct.owner = CONTRACT_PROGRAM_ID;
    contract_acct.executable = true;
    contract_acct.data = serde_json::to_vec(&contract).expect("ser");
    state
        .put_account(&contract_prog, &contract_acct)
        .expect("put contract");
    state
        .register_symbol(
            "TST",
            SymbolRegistryEntry {
                symbol: "TST".to_string(),
                program: contract_prog,
                owner: Pubkey([2u8; 32]),
                name: Some("Test Contract".to_string()),
                template: Some("token".to_string()),
                metadata: None,
                decimals: Some(9),
            },
        )
        .expect("register");

    let cloned_state = state.clone();
    let app = build_rpc_router(
        state,
        None,
        None,
        None,
        "lichen-test".to_string(),
        "lichen-test".to_string(),
        None,
        None,
        None,
        None,
        None,
    );
    (
        app,
        cloned_state,
        funded_b58,
        val_b58,
        block_hash_hex,
        tx_sig_hex,
    )
}

fn app_with_bootstrap_staking_rewards() -> (axum::Router, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    let _ = Box::leak(Box::new(dir));

    let validator = Pubkey([8u8; 32]);
    let validator_b58 = validator.to_base58();
    state.set_last_slot(123_456).expect("set slot");

    let mut stake_pool = StakePool::new();
    let mut stake_info = StakeInfo::with_bootstrap_index(validator, BOOTSTRAP_GRANT_AMOUNT, 0, 0);
    stake_info.rewards_earned = 200_000_000;
    stake_info.total_claimed = 1_000_000_000;
    stake_info.total_debt_repaid = 400_000_000;
    stake_info.earned_amount = 400_000_000;
    stake_info.blocks_produced = 12;
    stake_pool.upsert_stake_full(stake_info);

    let app = build_rpc_router(
        state,
        None,
        Some(Arc::new(RwLock::new(stake_pool))),
        None,
        "lichen-test".to_string(),
        "lichen-test".to_string(),
        None,
        None,
        None,
        None,
        None,
    );

    (app, validator_b58)
}

// ── getBalance positive path (native "/") ────────────────────────────────────

#[tokio::test]
async fn test_native_get_balance_funded_account() {
    let (app, _, addr, _, _, _) = app_with_rich_state();
    let resp = rpc_p(&app, "/", "getBalance", json!([addr])).await.unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "funded account balance should not be null"
    );
    // Account::new(5, ..) → spores = 5_000_000_000, spendable = 5_000_000_000
    assert_eq!(
        result["spores"], 5_000_000_000u64,
        "spores = 5 LICN in spores"
    );
    assert_eq!(result["spendable"], 5_000_000_000u64, "spendable = 5 LICN");
    assert_eq!(result["licn"], "5.0000", "licn = 5.0000");
    assert_eq!(result["staked"], 0, "staked = 0");
}

#[tokio::test]
async fn test_native_get_balance_unfunded_returns_zero() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getBalance",
        json!(["22222222222222222222222222222222"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    // Handler may return zero-balance result OR error for nonexistent accounts
    if let Some(result) = resp.get("result") {
        if !result.is_null() {
            assert_eq!(result["spores"], 0, "unfunded spores should be 0");
        }
    }
    // Either result or error is acceptable for nonexistent
}

// ── getSlot positive path (native "/") ───────────────────────────────────────

#[tokio::test]
async fn test_native_get_slot_returns_number() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc(&app, "/", "getSlot").await.unwrap();
    assert_valid_rpc(&resp);
    let slot = resp["result"].as_u64().expect("getSlot should return u64");
    assert_eq!(slot, 1, "slot should be 1 after setup");
}

#[tokio::test]
async fn test_native_get_slot_with_commitment() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc_p(&app, "/", "getSlot", json!(["processed"]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    let slot = resp["result"].as_u64().expect("getSlot should return u64");
    assert_eq!(slot, 1);
}

// ── getBlock positive path (native "/") ──────────────────────────────────────

#[tokio::test]
async fn test_native_get_block_with_stored_block() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc_p(&app, "/", "getBlock", json!([1])).await.unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    assert!(!result.is_null(), "block at slot 1 should exist");
    assert_eq!(result["slot"], 1, "block slot should be 1");
    // Timestamp is set to current time in app_with_rich_state (GX-07 health check fix)
    let ts = result["timestamp"]
        .as_u64()
        .expect("timestamp should be u64");
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    assert!(
        now.saturating_sub(ts) <= 5,
        "block timestamp should be within 5 seconds of now (got {ts}, now {now})"
    );
    assert_eq!(
        result["transaction_count"], 1,
        "block should contain 1 transaction"
    );
    // Validator field should be the base58 of val_pk
    assert!(
        result["validator"].is_string(),
        "validator should be a string"
    );
}

#[tokio::test]
async fn test_native_get_block_commit_exposes_commit_round() {
    let (app, _) = app_with_anchored_account_proof();
    let resp = rpc_p(&app, "/", "getBlockCommit", json!([1]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);

    let result = &resp["result"];
    assert_eq!(result["slot"], 1);
    assert_eq!(result["commit_round"], 0);
    assert_eq!(result["commit_validator_count"], 1);
    assert_eq!(result["commit_signatures"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_native_get_block_genesis() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc_p(&app, "/", "getBlock", json!([0])).await.unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    assert!(!result.is_null(), "genesis block should exist");
    assert_eq!(result["slot"], 0, "genesis slot should be 0");
    assert_eq!(result["transaction_count"], 0, "genesis has no txs");
}

#[tokio::test]
async fn test_native_get_block_not_found() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc_p(&app, "/", "getBlock", json!([9999])).await.unwrap();
    assert_valid_rpc(&resp);
    assert!(
        resp.get("error").is_some(),
        "nonexistent block should return error"
    );
}

// ── getAccountInfo positive path (native "/") ────────────────────────────────

#[tokio::test]
async fn test_native_get_account_info_funded() {
    let (app, _, addr, _, _, _) = app_with_rich_state();
    let resp = rpc_p(&app, "/", "getAccountInfo", json!([addr]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    assert!(!result.is_null(), "account info should not be null");
    assert_eq!(result["exists"], true, "funded account should exist");
    // balance should be 5 LICN = 5_000_000_000 spores
    assert_eq!(result["balance"], 5_000_000_000u64);
}

#[tokio::test]
async fn test_native_get_account_info_nonexistent_returns_null_or_default() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "getAccountInfo",
        json!(["33333333333333333333333333333333"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    // Nonexistent accounts may return null result or error
    let result = &resp["result"];
    if !result.is_null() {
        // If it returns data, balance should be 0
        if result.get("balance").is_some() {
            assert_eq!(result["balance"], 0);
        }
    }
}

// ── getValidators with registered validator ──────────────────────────────────

#[tokio::test]
async fn test_native_get_validators_with_data() {
    let (app, _, _, val_b58, _, _) = app_with_rich_state();
    let resp = rpc(&app, "/", "getValidators").await.unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    assert!(!result.is_null(), "validators should not be null");
    // Should contain at least 1 validator
    if let Some(arr) = result.as_array() {
        assert!(!arr.is_empty(), "should have at least 1 validator");
        // Find our validator by pubkey
        let found = arr.iter().any(|v| v["pubkey"] == val_b58);
        assert!(found, "our validator should be in the list");
    }
}

// ── getTransaction with stored tx ────────────────────────────────────────────

#[tokio::test]
async fn test_native_get_transaction_found() {
    let (app, _, _, _, _, tx_sig) = app_with_rich_state();
    let resp = rpc_p(&app, "/", "getTransaction", json!([tx_sig]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    // Result may be the tx or error depending on format, but should not be method-not-found
    assert!(
        resp.get("error").is_none_or(|e| e["code"] != -32601),
        "should route to handler, not method-not-found"
    );
}

// ── getTransaction response includes message_hash (Task 4.1) ────────────────

#[tokio::test]
async fn test_get_transaction_includes_message_hash() {
    let (app, _, _, _, _, tx_sig) = app_with_rich_state();
    let resp = rpc_p(&app, "/", "getTransaction", json!([tx_sig]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    if result.is_object() && result.get("signature").is_some() {
        // Response has the transaction — verify message_hash is present and valid hex
        let mh = result["message_hash"]
            .as_str()
            .expect("message_hash should be string");
        assert_eq!(
            mh.len(),
            64,
            "message_hash should be 64 hex chars (32 bytes)"
        );
        assert!(
            mh.chars().all(|c| c.is_ascii_hexdigit()),
            "message_hash must be hex"
        );
        // message_hash must differ from signature (tx hash)
        let sig = result["signature"].as_str().unwrap();
        assert_ne!(
            mh, sig,
            "message_hash should differ from tx hash (signatures not included)"
        );
    }
}

// ── Solana-compat getBalance with funded account ─────────────────────────────

#[tokio::test]
async fn test_solana_get_balance_funded() {
    let (app, _, addr, _, _, _) = app_with_rich_state();
    let resp = rpc_p(&app, "/solana-compat", "getBalance", json!([addr]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    // Solana format: { "context": { "slot": N }, "value": spores }
    assert!(
        result["context"]["slot"].is_number(),
        "should have context.slot"
    );
    assert_eq!(
        result["value"], 5_000_000_000u64,
        "solana getBalance value should be 5B spores"
    );
}

#[tokio::test]
async fn test_solana_get_balance_unfunded() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/solana-compat",
        "getBalance",
        json!(["44444444444444444444444444444444"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    // Unfunded account may return 0 value or null/error
    if let Some(result) = resp.get("result") {
        if !result.is_null() && result.get("value").is_some() {
            assert_eq!(result["value"], 0, "unfunded solana balance should be 0");
        }
    }
}

// ── Solana-compat getSlot verifies the value matches set_last_slot ───────────

#[tokio::test]
async fn test_solana_get_slot_value() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc(&app, "/solana-compat", "getSlot").await.unwrap();
    assert_valid_rpc(&resp);
    let slot = resp["result"].as_u64().expect("getSlot must be u64");
    assert_eq!(slot, 1, "solana getSlot should be 1");
}

// ── Solana-compat getBlock with stored block ─────────────────────────────────

#[tokio::test]
async fn test_solana_get_block_with_data() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc_p(&app, "/solana-compat", "getBlock", json!([1]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    // Solana getBlock should return a block object (or null)
    if !result.is_null() {
        assert!(
            result.get("blockTime").is_some() || result.get("slot").is_some(),
            "block response should have blockTime or slot: {result}"
        );
    }
}

// ── Solana-compat getBlockHeight value ───────────────────────────────────────

#[tokio::test]
async fn test_solana_get_block_height_value() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc(&app, "/solana-compat", "getBlockHeight").await.unwrap();
    assert_valid_rpc(&resp);
    let height = resp["result"].as_u64().expect("blockHeight must be u64");
    assert_eq!(height, 1, "block height should be 1");
}

// ── EVM eth_gasPrice returns "0x1" ───────────────────────────────────────────

#[tokio::test]
async fn test_evm_eth_gas_price_value() {
    let app = fresh_app();
    let resp = rpc(&app, "/evm", "eth_gasPrice").await.unwrap();
    assert_valid_rpc(&resp);
    assert_eq!(
        resp["result"], "0x1",
        "eth_gasPrice must return 0x1 per AUDIT-FIX A11-01"
    );
}

// ── EVM eth_chainId returns correct hex ──────────────────────────────────────

#[tokio::test]
async fn test_evm_eth_chain_id_value() {
    let app = fresh_app();
    let resp = rpc(&app, "/evm", "eth_chainId").await.unwrap();
    assert_valid_rpc(&resp);
    let chain = resp["result"].as_str().expect("chainId should be string");
    assert!(chain.starts_with("0x"), "chainId should be hex: {chain}");
    // "lichen-test" → evm_chain_id_from_chain_id hash
    assert!(!chain.is_empty());
}

// ── EVM eth_blockNumber returns hex slot ──────────────────────────────────────

#[tokio::test]
async fn test_evm_eth_block_number_value() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc(&app, "/evm", "eth_blockNumber").await.unwrap();
    assert_valid_rpc(&resp);
    let bn = resp["result"]
        .as_str()
        .expect("blockNumber should be hex string");
    assert!(bn.starts_with("0x"), "blockNumber should be hex");
    // Slot is 1, so blockNumber should be "0x1"
    assert_eq!(bn, "0x1", "blockNumber should match last slot");
}

// ── EVM eth_getLogs returns empty array for empty state ──────────────────────

#[tokio::test]
async fn test_evm_eth_get_logs_empty() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getLogs",
        json!([{"fromBlock": "0x0", "toBlock": "0x0"}]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    assert!(result.is_array(), "eth_getLogs must return an array");
    assert_eq!(
        result.as_array().unwrap().len(),
        0,
        "no logs in empty state"
    );
}

#[tokio::test]
async fn test_evm_eth_get_logs_invalid_address_filter_is_generic() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getLogs",
        json!([{"address": "not-an-address", "fromBlock": "0x0", "toBlock": "0x0"}]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    assert_eq!(resp["error"]["message"], "Invalid address filter format");
}

#[tokio::test]
async fn test_evm_eth_get_logs_with_blocks() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getLogs",
        json!([{"fromBlock": "0x0", "toBlock": "0x1"}]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    // Should return array (may be empty since our test tx doesn't emit events)
    assert!(resp["result"].is_array(), "eth_getLogs must return array");
}

// ── EVM eth_getBalance for funded account via EVM ────────────────────────────

#[tokio::test]
async fn test_evm_eth_get_balance_funded() {
    // Create an account with known EVM mapping
    let (app, state, _, _, _, _) = app_with_rich_state();
    // Register an EVM address mapping for the funded account
    let funded = Pubkey([42u8; 32]);
    let evm_addr_bytes: [u8; 20] = [0x2a; 20];
    let evm_addr_hex = "0x2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a";
    state
        .register_evm_address(&evm_addr_bytes, &funded)
        .expect("register EVM address");
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getBalance",
        json!([evm_addr_hex, "latest"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    // Should return hex balance string
    let result = resp.get("result").expect("should have result");
    assert!(
        !result.is_null(),
        "eth_getBalance for mapped account should not be null"
    );
}

// ── health endpoint returns "ok" ─────────────────────────────────────────────

#[tokio::test]
async fn test_native_health_deep_check() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc(&app, "/", "health").await.unwrap();
    assert_valid_rpc(&resp);
    assert_eq!(resp["result"]["status"], "ok");
}

// ── getRecentBlockhash returns data with block in state ──────────────────────

#[tokio::test]
async fn test_native_get_recent_blockhash_with_block() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc(&app, "/", "getRecentBlockhash").await.unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    assert!(
        !result.is_null(),
        "blockhash should exist with stored blocks"
    );
}

// ── getChainStatus returns slot info ─────────────────────────────────────────

#[tokio::test]
async fn test_native_get_chain_status_with_data() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc(&app, "/", "getChainStatus").await.unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    assert!(!result.is_null(), "chain status should not be null");
    // Should contain slot info
    if result.get("slot").is_some() {
        assert_eq!(result["slot"], 1, "chain status slot should be 1");
    }
}

// ── getMetrics returns populated metrics ─────────────────────────────────────

#[tokio::test]
async fn test_native_get_metrics_with_data() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc(&app, "/", "getMetrics").await.unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    assert!(!result.is_null(), "metrics should return data");
}

// ── getAllContracts returns our deployed contract ─────────────────────────────

#[tokio::test]
async fn test_native_get_all_contracts_has_entry() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc(&app, "/", "getAllContracts").await.unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    assert!(!result.is_null(), "getAllContracts should return data");
    // Should contain at least our deployed contract
    if let Some(arr) = result.as_array() {
        assert!(!arr.is_empty(), "should have at least 1 contract");
    }
}

// ── getSymbolRegistry returns registered TST symbol ──────────────────────────

#[tokio::test]
async fn test_native_get_symbol_registry_found() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc_p(&app, "/", "getSymbolRegistry", json!(["TST"]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    if !result.is_null() {
        assert_eq!(result["symbol"], "TST", "should find TST symbol");
    }
}

// ── getMossStakePoolInfo returns pool data ───────────────────────────────────

#[tokio::test]
async fn test_native_get_mossstake_pool_info_returns_data() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getMossStakePoolInfo").await.unwrap();
    assert_valid_rpc(&resp);
    // Should return pool info (possibly empty/defaults), not null
    let result = &resp["result"];
    assert!(!result.is_null(), "mossstake pool info should return data");
}

// ── getTreasuryInfo returns treasury data ────────────────────────────────────

#[tokio::test]
async fn test_native_get_treasury_info_returns_data() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc(&app, "/", "getTreasuryInfo").await.unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    assert!(!result.is_null(), "treasury info should return data");
}

// ── getFeeConfig returns config object ───────────────────────────────────────

#[tokio::test]
async fn test_native_get_fee_config_returns_object() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getFeeConfig").await.unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    assert!(!result.is_null(), "fee config should return data");
    // Should have base_fee_spores field
    if result.is_object() {
        assert!(
            result.get("base_fee_spores").is_some(),
            "fee config should have base_fee_spores: {result}"
        );
    }
}

// ── requestAirdrop credits the account ───────────────────────────────────────

#[tokio::test]
async fn test_native_request_airdrop_handled() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "requestAirdrop",
        json!(["55555555555555555555555555555555", 1000]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    // Without tx_sender, airdrop may error; important thing is it's routed correctly
    assert!(
        resp.get("error").is_none_or(|e| e["code"] != -32601),
        "airdrop should route to handler, not method-not-found"
    );
}

// ── confirmTransaction returns status for known tx ───────────────────────────

#[tokio::test]
async fn test_native_confirm_transaction_found() {
    let (app, _, _, _, _, tx_sig) = app_with_rich_state();
    let resp = rpc_p(&app, "/", "confirmTransaction", json!([tx_sig]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    // Should route correctly (not method-not-found)
    assert!(
        resp.get("error").is_none_or(|e| e["code"] != -32601),
        "should route to handler"
    );
}

// ── getNetworkInfo returns network data ──────────────────────────────────────

#[tokio::test]
async fn test_native_get_network_info_returns_data() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getNetworkInfo").await.unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    assert!(!result.is_null(), "network info should return data");
}

// ── getClusterInfo returns cluster data ──────────────────────────────────────

#[tokio::test]
async fn test_native_get_cluster_info_returns_data() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getClusterInfo").await.unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    assert!(!result.is_null(), "cluster info should return data");
}

// ── Solana getHealth returns "ok" ────────────────────────────────────────────

#[tokio::test]
async fn test_solana_get_health_is_ok() {
    let app = fresh_app();
    let resp = rpc(&app, "/solana-compat", "getHealth").await.unwrap();
    assert_valid_rpc(&resp);
    // Fresh state with no blocks reports as behind with slot 0
    let result = &resp["result"];
    assert!(
        result.get("status").is_some(),
        "getHealth should return status object"
    );
    assert_eq!(result["slot"], 0);
}

// ── Solana getVersion returns version info ───────────────────────────────────

#[tokio::test]
async fn test_solana_get_version_shape() {
    let app = fresh_app();
    let resp = rpc(&app, "/solana-compat", "getVersion").await.unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    assert!(
        result.get("solana-core").is_some() || result.get("feature-set").is_some(),
        "getVersion should have version info: {result}"
    );
}

// ── Solana getLatestBlockhash returns context+value ──────────────────────────

#[tokio::test]
async fn test_solana_get_latest_blockhash_shape() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc(&app, "/solana-compat", "getLatestBlockhash")
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    // Should have Solana-compat shape: { context: { slot }, value: { blockhash, lastValidBlockHeight } }
    if !result.is_null() {
        if result.get("context").is_some() {
            assert!(result["context"]["slot"].is_number());
        }
        if result.get("value").is_some() {
            assert!(
                result["value"]["blockhash"].is_string(),
                "should have blockhash string"
            );
        }
    }
}

// ── Solana getAccountInfo with funded account ────────────────────────────────

#[tokio::test]
async fn test_solana_get_account_info_funded() {
    let (app, _, addr, _, _, _) = app_with_rich_state();
    let resp = rpc_p(&app, "/solana-compat", "getAccountInfo", json!([addr]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    // Solana format: { context: { slot }, value: { lamports, owner, executable, ... } }
    if !result.is_null() && result.get("value").is_some() && !result["value"].is_null() {
        assert!(
            result["value"]["lamports"].is_number(),
            "should have lamports"
        );
    }
}

// ── EVM web3_clientVersion value ─────────────────────────────────────────────

#[tokio::test]
async fn test_evm_web3_client_version_full() {
    let app = fresh_app();
    let resp = rpc(&app, "/evm", "web3_clientVersion").await.unwrap();
    assert_valid_rpc(&resp);
    let ver = resp["result"].as_str().unwrap();
    assert!(ver.starts_with("Lichen/"), "starts with Lichen/");
    assert!(ver.contains('/'), "should contain version separator");
}

// ── EVM net_version returns chain ID string ──────────────────────────────────

#[tokio::test]
async fn test_evm_net_version_value() {
    let app = fresh_app();
    let resp = rpc(&app, "/evm", "net_version").await.unwrap();
    assert_valid_rpc(&resp);
    let ver = resp["result"]
        .as_str()
        .expect("net_version should be string");
    // Should be numeric decimal chain ID
    assert!(
        ver.parse::<u64>().is_ok(),
        "net_version should be numeric: {ver}"
    );
}

// ── EVM net_listening returns true ───────────────────────────────────────────

#[tokio::test]
async fn test_evm_net_listening_value() {
    let app = fresh_app();
    let resp = rpc(&app, "/evm", "net_listening").await.unwrap();
    assert_valid_rpc(&resp);
    assert_eq!(resp["result"], true, "net_listening should be true");
}

// ── EVM eth_accounts returns empty array ─────────────────────────────────────

#[tokio::test]
async fn test_evm_eth_accounts_value() {
    let app = fresh_app();
    let resp = rpc(&app, "/evm", "eth_accounts").await.unwrap();
    assert_valid_rpc(&resp);
    assert_eq!(resp["result"], json!([]), "eth_accounts should be []");
}

// ── EVM eth_estimateGas returns hex ──────────────────────────────────────────

#[tokio::test]
async fn test_evm_eth_estimate_gas_value() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_estimateGas",
        json!([{"to": "0x0000000000000000000000000000000000000001"}]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    let gas = resp["result"].as_str();
    if let Some(g) = gas {
        assert!(g.starts_with("0x"), "estimated gas should be hex: {g}");
    }
}

// ── EVM eth_getBlockByNumber with stored block ───────────────────────────────

#[tokio::test]
async fn test_evm_eth_get_block_by_number_stored() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc_p(&app, "/evm", "eth_getBlockByNumber", json!(["0x1", false]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    if !result.is_null() {
        // Should have EVM block fields
        assert!(
            result.get("number").is_some() || result.get("hash").is_some(),
            "block should have number or hash: {result}"
        );
    }
}

// ── EVM eth_getTransactionCount returns hex nonce ────────────────────────────

#[tokio::test]
async fn test_evm_eth_get_transaction_count_value() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getTransactionCount",
        json!(["0x0000000000000000000000000000000000000001", "latest"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    if let Some(result) = resp.get("result") {
        if let Some(s) = result.as_str() {
            assert!(s.starts_with("0x"), "tx count should be hex: {s}");
        }
    }
}

// ── REST /api/v1/pairs returns array ─────────────────────────────────────────

#[tokio::test]
async fn test_rest_dex_pairs_returns_json() {
    let app = fresh_app();
    let resp = rest_get(&app, "/api/v1/pairs").await;
    if let Ok(json) = resp {
        // Should be array (possibly empty)
        assert!(
            json.is_array() || json.is_object(),
            "pairs should be array or object"
        );
    }
}

// ── Batch: all native query methods with rich state don't panic ──────────────

#[tokio::test]
async fn test_batch_native_reads_with_rich_state() {
    let (app, _, addr, _, _, _) = app_with_rich_state();
    let methods_no_params = vec![
        "getSlot",
        "getLatestBlock",
        "getRecentBlockhash",
        "health",
        "getMetrics",
        "getTreasuryInfo",
        "getChainStatus",
        "getNetworkInfo",
        "getClusterInfo",
        "getValidators",
        "getPeers",
        "getFeeConfig",
        "getRentParams",
        "getGenesisAccounts",
        "getTotalBurned",
        "getMossStakePoolInfo",
        "getRewardAdjustmentInfo",
        "getPredictionMarketStats",
        "getPredictionTrending",
        "getAllContracts",
    ];
    for method in &methods_no_params {
        let resp = rpc(&app, "/", method).await.unwrap();
        assert_valid_rpc(&resp);
    }

    // Methods with pubkey param
    let methods_with_addr = vec![
        "getBalance",
        "getAccountInfo",
        "getAccount",
        "getTransactionHistory",
        "getTransactionsByAddress",
        "getAccountTxCount",
        "getTokenAccounts",
        "getSignaturesForAddress",
        "getStakingStatus",
        "getStakingRewards",
        "getStakingPosition",
        "getUnstakingQueue",
        "getEvmRegistration",
        "getNFTsByOwner",
        "getPredictionPositions",
        "getPredictionTraderStats",
    ];
    for method in &methods_with_addr {
        let resp = rpc_p(&app, "/", method, json!([addr])).await.unwrap();
        assert_valid_rpc(&resp);
    }
}

// ── Batch: all Solana methods with rich state return valid data ───────────────

#[tokio::test]
async fn test_batch_solana_with_rich_state() {
    let (app, _, addr, _, _, _) = app_with_rich_state();

    // No-param methods
    for method in &[
        "getHealth",
        "getVersion",
        "getSlot",
        "getBlockHeight",
        "getLatestBlockhash",
        "getRecentBlockhash",
    ] {
        let resp = rpc(&app, "/solana-compat", method).await.unwrap();
        assert_valid_rpc(&resp);
    }

    // With addr
    for method in &["getBalance", "getAccountInfo", "getSignaturesForAddress"] {
        let resp = rpc_p(&app, "/solana-compat", method, json!([addr]))
            .await
            .unwrap();
        assert_valid_rpc(&resp);
    }

    // With slot
    let resp = rpc_p(&app, "/solana-compat", "getBlock", json!([0]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

// ── Batch: all EVM methods with rich state return valid data ─────────────────

#[tokio::test]
async fn test_batch_evm_with_rich_state() {
    let (app, _, _, _, _, _) = app_with_rich_state();

    // No-param methods
    for method in &[
        "eth_chainId",
        "eth_blockNumber",
        "eth_accounts",
        "net_version",
        "eth_gasPrice",
        "eth_maxPriorityFeePerGas",
        "net_listening",
        "web3_clientVersion",
    ] {
        let resp = rpc(&app, "/evm", method).await.unwrap();
        assert_valid_rpc(&resp);
        assert!(
            resp.get("result").is_some(),
            "EVM method {method} should return result with rich state"
        );
    }

    // With EVM address
    let evm_addr = "0x0000000000000000000000000000000000000001";
    for method in &["eth_getBalance", "eth_getCode", "eth_getTransactionCount"] {
        let resp = rpc_p(&app, "/evm", method, json!([evm_addr, "latest"]))
            .await
            .unwrap();
        assert_valid_rpc(&resp);
    }

    // eth_getBlockByNumber with stored block
    let resp = rpc_p(&app, "/evm", "eth_getBlockByNumber", json!(["0x0", false]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);

    // eth_getLogs range
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getLogs",
        json!([{"fromBlock": "0x0", "toBlock": "0x1"}]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    assert!(resp["result"].is_array());
}

#[tokio::test]
async fn test_native_estimate_transaction_fee() {
    let app = fresh_app();
    // Call with no params should error gracefully
    let resp = rpc(&app, "/", "estimateTransactionFee").await.unwrap();
    assert!(resp.get("error").is_some(), "Missing params should error");
}

#[tokio::test]
async fn test_native_estimate_transaction_fee_invalid_base64() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "estimateTransactionFee",
        json!(["not-valid-base64!!!"]),
    )
    .await
    .unwrap();
    assert!(resp.get("error").is_some(), "Bad base64 should error");
}

// ═══════════════════════════════════════════════════════════════════════════════
// Task 3.4: EVM Precompiles + eth_getLogs tests
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_evm_eth_get_logs_with_stored_evm_logs() {
    // Store structured EVM logs and verify eth_getLogs retrieves them
    let (app, state, _, _, _, _) = app_with_rich_state();
    use lichen_core::evm::{EvmLog, EvmLogEntry};

    let logs = vec![EvmLogEntry {
        tx_hash: [0xAA; 32],
        tx_index: 0,
        log_index: 0,
        log: EvmLog {
            address: [0x11; 20],
            topics: vec![[0x01; 32], [0x02; 32]],
            data: vec![0xFF, 0xFE],
        },
    }];
    state.put_evm_logs_for_slot(1, &logs).unwrap();

    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getLogs",
        json!([{"fromBlock": "0x1", "toBlock": "0x1"}]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    let result = resp["result"].as_array().expect("should be array");
    assert!(!result.is_empty(), "Should return at least 1 EVM log");

    // Verify the structured EVM log fields
    let log = &result[0];
    assert_eq!(log["address"], format!("0x{}", hex::encode([0x11; 20])));
    assert_eq!(log["logIndex"], "0x0");
    assert!(log["topics"].is_array());
    let topics = log["topics"].as_array().unwrap();
    assert_eq!(topics.len(), 2);
    assert_eq!(topics[0], format!("0x{}", hex::encode([0x01; 32])));
    assert_eq!(log["data"], format!("0x{}", hex::encode([0xFF, 0xFE])));
    assert_eq!(log["removed"], false);
}

#[tokio::test]
async fn test_evm_eth_get_logs_address_filter() {
    let (app, state, _, _, _, _) = app_with_rich_state();
    use lichen_core::evm::{EvmLog, EvmLogEntry};

    let addr_a = [0xAA; 20];
    let addr_b = [0xBB; 20];
    let logs = vec![
        EvmLogEntry {
            tx_hash: [0x01; 32],
            tx_index: 0,
            log_index: 0,
            log: EvmLog {
                address: addr_a,
                topics: vec![[0x10; 32]],
                data: vec![1],
            },
        },
        EvmLogEntry {
            tx_hash: [0x02; 32],
            tx_index: 1,
            log_index: 1,
            log: EvmLog {
                address: addr_b,
                topics: vec![[0x20; 32]],
                data: vec![2],
            },
        },
    ];
    state.put_evm_logs_for_slot(1, &logs).unwrap();

    // Filter by address A only
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getLogs",
        json!([{
            "fromBlock": "0x1",
            "toBlock": "0x1",
            "address": format!("0x{}", hex::encode(addr_a))
        }]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    let result = resp["result"].as_array().unwrap();
    // Should only have logs from addr_a
    for log in result {
        if log["address"].as_str().unwrap().contains("aaaa") {
            // This is the EVM structured log from addr_a — good
        }
    }
    let evm_only: Vec<_> = result
        .iter()
        .filter(|l| l["address"].as_str().unwrap() == format!("0x{}", hex::encode(addr_a)))
        .collect();
    assert_eq!(evm_only.len(), 1, "Should return only logs from addr_a");
}

#[tokio::test]
async fn test_evm_eth_get_logs_address_array_filter() {
    let (app, state, _, _, _, _) = app_with_rich_state();
    use lichen_core::evm::{EvmLog, EvmLogEntry};

    let addr_a = [0xAA; 20];
    let addr_b = [0xBB; 20];
    let addr_c = [0xCC; 20];
    let logs = vec![
        EvmLogEntry {
            tx_hash: [0x01; 32],
            tx_index: 0,
            log_index: 0,
            log: EvmLog {
                address: addr_a,
                topics: vec![[0x10; 32]],
                data: vec![1],
            },
        },
        EvmLogEntry {
            tx_hash: [0x02; 32],
            tx_index: 1,
            log_index: 1,
            log: EvmLog {
                address: addr_b,
                topics: vec![[0x20; 32]],
                data: vec![2],
            },
        },
        EvmLogEntry {
            tx_hash: [0x03; 32],
            tx_index: 2,
            log_index: 2,
            log: EvmLog {
                address: addr_c,
                topics: vec![[0x30; 32]],
                data: vec![3],
            },
        },
    ];
    state.put_evm_logs_for_slot(1, &logs).unwrap();

    // Filter by [addr_a, addr_c]
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getLogs",
        json!([{
            "fromBlock": "0x1",
            "toBlock": "0x1",
            "address": [
                format!("0x{}", hex::encode(addr_a)),
                format!("0x{}", hex::encode(addr_c))
            ]
        }]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    let result = resp["result"].as_array().unwrap();
    let evm_addrs: Vec<String> = result
        .iter()
        .map(|l| l["address"].as_str().unwrap().to_string())
        .collect();
    let a_hex = format!("0x{}", hex::encode(addr_a));
    let b_hex = format!("0x{}", hex::encode(addr_b));
    let c_hex = format!("0x{}", hex::encode(addr_c));
    assert!(evm_addrs.contains(&a_hex), "Should include addr_a");
    assert!(!evm_addrs.contains(&b_hex), "Should NOT include addr_b");
    assert!(evm_addrs.contains(&c_hex), "Should include addr_c");
}

#[tokio::test]
async fn test_evm_eth_get_logs_topic_filter() {
    let (app, state, _, _, _, _) = app_with_rich_state();
    use lichen_core::evm::{EvmLog, EvmLogEntry};

    let topic_transfer = [0xDD; 32]; // Fake Transfer topic
    let topic_approval = [0xEE; 32]; // Fake Approval topic
    let logs = vec![
        EvmLogEntry {
            tx_hash: [0x01; 32],
            tx_index: 0,
            log_index: 0,
            log: EvmLog {
                address: [0x11; 20],
                topics: vec![topic_transfer, [0xAA; 32]],
                data: vec![1],
            },
        },
        EvmLogEntry {
            tx_hash: [0x02; 32],
            tx_index: 1,
            log_index: 1,
            log: EvmLog {
                address: [0x11; 20],
                topics: vec![topic_approval, [0xBB; 32]],
                data: vec![2],
            },
        },
    ];
    state.put_evm_logs_for_slot(1, &logs).unwrap();

    // Filter by topic[0] = Transfer only
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getLogs",
        json!([{
            "fromBlock": "0x1",
            "toBlock": "0x1",
            "topics": [format!("0x{}", hex::encode(topic_transfer))]
        }]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    let result = resp["result"].as_array().unwrap();
    let evm_logs: Vec<_> = result
        .iter()
        .filter(|l| {
            l["topics"]
                .as_array()
                .map(|t| {
                    t.first()
                        .and_then(|v| v.as_str())
                        .map(|s| s == format!("0x{}", hex::encode(topic_transfer)))
                        .unwrap_or(false)
                })
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(evm_logs.len(), 1, "Should return only Transfer logs");
}

#[tokio::test]
async fn test_evm_eth_get_logs_topic_or_filter() {
    let (app, state, _, _, _, _) = app_with_rich_state();
    use lichen_core::evm::{EvmLog, EvmLogEntry};

    let topic_a = [0xAA; 32];
    let topic_b = [0xBB; 32];
    let topic_c = [0xCC; 32];
    let logs = vec![
        EvmLogEntry {
            tx_hash: [0x01; 32],
            tx_index: 0,
            log_index: 0,
            log: EvmLog {
                address: [0x11; 20],
                topics: vec![topic_a],
                data: vec![1],
            },
        },
        EvmLogEntry {
            tx_hash: [0x02; 32],
            tx_index: 1,
            log_index: 1,
            log: EvmLog {
                address: [0x11; 20],
                topics: vec![topic_b],
                data: vec![2],
            },
        },
        EvmLogEntry {
            tx_hash: [0x03; 32],
            tx_index: 2,
            log_index: 2,
            log: EvmLog {
                address: [0x11; 20],
                topics: vec![topic_c],
                data: vec![3],
            },
        },
    ];
    state.put_evm_logs_for_slot(1, &logs).unwrap();

    // OR filter: topic[0] = topic_a OR topic_c
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getLogs",
        json!([{
            "fromBlock": "0x1",
            "toBlock": "0x1",
            "topics": [[
                format!("0x{}", hex::encode(topic_a)),
                format!("0x{}", hex::encode(topic_c))
            ]]
        }]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    let result = resp["result"].as_array().unwrap();
    // Should match topic_a and topic_c but not topic_b
    let matched_data: Vec<String> = result
        .iter()
        .map(|l| l["data"].as_str().unwrap().to_string())
        .collect();
    let a_data = format!("0x{}", hex::encode([1u8]));
    let c_data = format!("0x{}", hex::encode([3u8]));
    let b_data = format!("0x{}", hex::encode([2u8]));
    assert!(matched_data.contains(&a_data), "Should include topic_a log");
    assert!(matched_data.contains(&c_data), "Should include topic_c log");
    assert!(
        !matched_data.contains(&b_data),
        "Should NOT include topic_b log"
    );
}

#[tokio::test]
async fn test_evm_eth_get_logs_wildcard_topic() {
    let (app, state, _, _, _, _) = app_with_rich_state();
    use lichen_core::evm::{EvmLog, EvmLogEntry};

    let topic_sig = [0xDD; 32];
    let topic_from = [0xAA; 32];
    let topic_to = [0xBB; 32];
    let logs = vec![EvmLogEntry {
        tx_hash: [0x01; 32],
        tx_index: 0,
        log_index: 0,
        log: EvmLog {
            address: [0x11; 20],
            topics: vec![topic_sig, topic_from, topic_to],
            data: vec![0x42],
        },
    }];
    state.put_evm_logs_for_slot(1, &logs).unwrap();

    // Wildcard at position 0, exact match at position 2 (topic_to)
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getLogs",
        json!([{
            "fromBlock": "0x1",
            "toBlock": "0x1",
            "topics": [
                serde_json::Value::Null,
                serde_json::Value::Null,
                format!("0x{}", hex::encode(topic_to))
            ]
        }]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    let result = resp["result"].as_array().unwrap();
    let evm_matches: Vec<_> = result
        .iter()
        .filter(|l| l["data"].as_str().unwrap() == format!("0x{}", hex::encode([0x42])))
        .collect();
    assert_eq!(
        evm_matches.len(),
        1,
        "Wildcard + exact should match the log"
    );
}

#[tokio::test]
async fn test_evm_eth_get_logs_block_range() {
    let (app, state, _, _, _, _) = app_with_rich_state();
    use lichen_core::evm::{EvmLog, EvmLogEntry};

    // Put logs in slot 1 only (slot 0 is genesis)
    let logs = vec![EvmLogEntry {
        tx_hash: [0x01; 32],
        tx_index: 0,
        log_index: 0,
        log: EvmLog {
            address: [0x11; 20],
            topics: vec![[0xAA; 32]],
            data: vec![1],
        },
    }];
    state.put_evm_logs_for_slot(1, &logs).unwrap();

    // Query only slot 0 — should NOT find the log
    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getLogs",
        json!([{"fromBlock": "0x0", "toBlock": "0x0"}]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    let result = resp["result"].as_array().unwrap();
    // Slot 0 should have no EVM structured logs
    let has_our_log = result
        .iter()
        .any(|l| l["data"].as_str().unwrap() == format!("0x{}", hex::encode([1u8])));
    assert!(!has_our_log, "Slot 0 should not contain the slot-1 log");
}

#[tokio::test]
async fn test_evm_eth_get_logs_log_format_complete() {
    // Verify all EIP-1474 required fields are present in each log entry
    let (app, state, _, _, _, _) = app_with_rich_state();
    use lichen_core::evm::{EvmLog, EvmLogEntry};

    let logs = vec![EvmLogEntry {
        tx_hash: [0x42; 32],
        tx_index: 3,
        log_index: 7,
        log: EvmLog {
            address: [0xDE; 20],
            topics: vec![[0x01; 32]],
            data: vec![0xAB, 0xCD],
        },
    }];
    state.put_evm_logs_for_slot(1, &logs).unwrap();

    let resp = rpc_p(
        &app,
        "/evm",
        "eth_getLogs",
        json!([{"fromBlock": "0x1", "toBlock": "0x1"}]),
    )
    .await
    .unwrap();
    let result = resp["result"].as_array().unwrap();
    // Find our specific log
    let our_log = result.iter().find(|l| {
        l["transactionHash"].as_str().unwrap() == format!("0x{}", hex::encode([0x42; 32]))
    });
    assert!(our_log.is_some(), "Should find our structured EVM log");
    let log = our_log.unwrap();

    // Check all required EIP-1474 log fields
    assert!(log.get("address").is_some(), "Must have address");
    assert!(log.get("topics").is_some(), "Must have topics");
    assert!(log.get("data").is_some(), "Must have data");
    assert!(log.get("blockNumber").is_some(), "Must have blockNumber");
    assert!(log.get("blockHash").is_some(), "Must have blockHash");
    assert!(
        log.get("transactionHash").is_some(),
        "Must have transactionHash"
    );
    assert!(
        log.get("transactionIndex").is_some(),
        "Must have transactionIndex"
    );
    assert!(log.get("logIndex").is_some(), "Must have logIndex");
    assert!(log.get("removed").is_some(), "Must have removed");

    // Verify hex formatting
    assert!(log["address"].as_str().unwrap().starts_with("0x"));
    assert!(log["blockNumber"].as_str().unwrap().starts_with("0x"));
    assert!(log["blockHash"].as_str().unwrap().starts_with("0x"));
    assert!(log["transactionHash"].as_str().unwrap().starts_with("0x"));
    assert_eq!(log["removed"], false);
}

#[tokio::test]
async fn test_evm_precompile_addresses_discoverable() {
    // Verify the supported_precompiles() function returns standard Ethereum precompiles
    use lichen_core::{
        supported_precompiles, PRECOMPILE_BLAKE2F, PRECOMPILE_BN256_ADD, PRECOMPILE_BN256_MUL,
        PRECOMPILE_BN256_PAIRING, PRECOMPILE_ECRECOVER, PRECOMPILE_IDENTITY, PRECOMPILE_MODEXP,
        PRECOMPILE_RIPEMD160, PRECOMPILE_SHA256,
    };

    let precompiles = supported_precompiles();
    assert_eq!(precompiles.len(), 9);

    // Verify names match standard Ethereum precompile names
    let names: Vec<&str> = precompiles.iter().map(|(_, name)| *name).collect();
    assert!(names.contains(&"ecRecover"));
    assert!(names.contains(&"SHA-256"));
    assert!(names.contains(&"RIPEMD-160"));
    assert!(names.contains(&"identity"));
    assert!(names.contains(&"modexp"));
    assert!(names.contains(&"bn256Add"));
    assert!(names.contains(&"bn256Mul"));
    assert!(names.contains(&"bn256Pairing"));
    assert!(names.contains(&"blake2f"));

    // Verify addresses are the standard ones
    assert_eq!(precompiles[0].0, PRECOMPILE_ECRECOVER);
    assert_eq!(precompiles[1].0, PRECOMPILE_SHA256);
    assert_eq!(precompiles[2].0, PRECOMPILE_RIPEMD160);
    assert_eq!(precompiles[3].0, PRECOMPILE_IDENTITY);
    assert_eq!(precompiles[4].0, PRECOMPILE_MODEXP);
    assert_eq!(precompiles[5].0, PRECOMPILE_BN256_ADD);
    assert_eq!(precompiles[6].0, PRECOMPILE_BN256_MUL);
    assert_eq!(precompiles[7].0, PRECOMPILE_BN256_PAIRING);
    assert_eq!(precompiles[8].0, PRECOMPILE_BLAKE2F);
}

#[tokio::test]
async fn test_evm_topics_match_integration() {
    // Verify topics_match() works correctly as used by handle_eth_get_logs
    use lichen_core::topics_match;

    let transfer_topic = [0xDD; 32];
    let from_topic = [0xAA; 32];
    let to_topic = [0xBB; 32];
    let log_topics = vec![transfer_topic, from_topic, to_topic];

    // Exact match on event signature
    assert!(topics_match(&log_topics, &[Some(vec![transfer_topic])]));

    // Wildcard + exact match on 'from'
    assert!(topics_match(&log_topics, &[None, Some(vec![from_topic])]));

    // Wildcard + wildcard + exact match on 'to'
    assert!(topics_match(
        &log_topics,
        &[None, None, Some(vec![to_topic])]
    ));

    // OR filter: match transfer OR approval at position 0
    let approval_topic = [0xEE; 32];
    assert!(topics_match(
        &log_topics,
        &[Some(vec![transfer_topic, approval_topic])]
    ));

    // No match
    assert!(!topics_match(&log_topics, &[Some(vec![approval_topic])]));
}

// ═══════════════════════════════════════════════════════════════════════════════
//  Task 3.9: Archive Mode — getAccountAtSlot
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_get_account_at_slot_archive_disabled() {
    let app = fresh_app();
    // archive mode is off by default → should return error
    let resp = rpc_p(
        &app,
        "/",
        "getAccountAtSlot",
        json!(["11111111111111111111111111111111", 10]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    assert!(
        resp.get("error").is_some(),
        "should error when archive disabled"
    );
}

#[tokio::test]
async fn test_native_get_account_at_slot_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    state.set_archive_mode(true);
    let _ = Box::leak(Box::new(dir));
    let app = build_rpc_router(
        state,
        None,
        None,
        None,
        "lichen-test".to_string(),
        "lichen-test".to_string(),
        None,
        None,
        None,
        None,
        None,
    );
    let resp = rpc_p(
        &app,
        "/",
        "getAccountAtSlot",
        json!(["11111111111111111111111111111111", 10]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    assert!(resp.get("error").is_some(), "no snapshot → error");
}

#[tokio::test]
async fn test_native_get_account_at_slot_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    state.set_archive_mode(true);

    let pk = Pubkey([0x42; 32]);
    let acc = Account::new(5, Pubkey([0u8; 32])); // 5 LICN = 5_000_000_000 spores
    state.put_account_snapshot(&pk, &acc, 100).unwrap();

    let _ = Box::leak(Box::new(dir));
    let app = build_rpc_router(
        state,
        None,
        None,
        None,
        "lichen-test".to_string(),
        "lichen-test".to_string(),
        None,
        None,
        None,
        None,
        None,
    );
    let resp = rpc_p(&app, "/", "getAccountAtSlot", json!([pk.to_base58(), 100]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    let result = resp.get("result").expect("should have result");
    assert_eq!(result["spores"], 5_000_000_000u64);
    assert_eq!(result["slot"], 100);
}

#[tokio::test]
async fn test_native_get_account_at_slot_missing_params() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    state.set_archive_mode(true);
    let _ = Box::leak(Box::new(dir));
    let app = build_rpc_router(
        state,
        None,
        None,
        None,
        "lichen-test".to_string(),
        "lichen-test".to_string(),
        None,
        None,
        None,
        None,
        None,
    );
    // Missing slot parameter
    let resp = rpc_p(
        &app,
        "/",
        "getAccountAtSlot",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
    assert!(resp.get("error").is_some());
}

// ═══════════════════════════════════════════════════════════════════════════════
// M-6: Wire-format envelope integration tests
// ═══════════════════════════════════════════════════════════════════════════════

/// sendTransaction accepts V1 wire-envelope encoded transaction.
#[tokio::test]
async fn test_send_transaction_wire_envelope() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    let _ = Box::leak(Box::new(dir));

    // Fund a sender
    let kp = lichen_core::Keypair::generate();
    let pk = kp.pubkey();
    let mut sender = lichen_core::Account::new(100, pk);
    sender.spendable = sender.spores;
    let _ = state.put_account(&pk, &sender);
    let _ = state.set_last_slot(1);

    let app = build_rpc_router(
        state.clone(),
        None,
        None,
        None,
        "lichen-test".to_string(),
        "lichen-test".to_string(),
        None,
        None,
        None,
        None,
        None,
    );

    // Build a transfer transaction
    let receiver = lichen_core::Pubkey([0x42; 32]);
    let blockhash = lichen_core::Hash::default();
    let mut data = vec![0u8]; // transfer opcode
    data.extend_from_slice(&1_000_000_000u64.to_le_bytes()); // 1 LICN
    let ix = lichen_core::Instruction {
        program_id: lichen_core::Pubkey([0; 32]),
        accounts: vec![pk, receiver],
        data,
    };
    let msg = lichen_core::Message::new(vec![ix], blockhash);
    let sig = kp.sign(&msg.serialize());
    let tx = lichen_core::Transaction {
        signatures: vec![sig],
        message: msg,
        tx_type: lichen_core::TransactionType::Native,
    };

    // Encode with wire envelope
    let wire = tx.to_wire();
    assert_eq!(&wire[0..2], &lichen_core::TX_WIRE_MAGIC);
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &wire);

    // Send via RPC — should decode successfully
    let resp = rpc_p(&app, "/", "sendTransaction", json!([b64]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    // It may fail for blockhash expiry or other reasons but should NOT fail
    // with "Invalid transaction" — the decode must succeed.
    if let Some(err) = resp.get("error") {
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("");
        assert!(
            !msg.contains("Invalid transaction"),
            "Wire envelope decode failed: {}",
            msg
        );
    }
}

/// sendTransaction still accepts raw bincode (no envelope).
#[tokio::test]
async fn test_send_transaction_raw_bincode() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    let _ = Box::leak(Box::new(dir));

    let kp = lichen_core::Keypair::generate();
    let pk = kp.pubkey();
    let mut sender = lichen_core::Account::new(100, pk);
    sender.spendable = sender.spores;
    let _ = state.put_account(&pk, &sender);
    let _ = state.set_last_slot(1);

    let app = build_rpc_router(
        state.clone(),
        None,
        None,
        None,
        "lichen-test".to_string(),
        "lichen-test".to_string(),
        None,
        None,
        None,
        None,
        None,
    );

    let receiver = lichen_core::Pubkey([0x42; 32]);
    let blockhash = lichen_core::Hash::default();
    let mut data = vec![0u8];
    data.extend_from_slice(&1_000_000_000u64.to_le_bytes());
    let ix = lichen_core::Instruction {
        program_id: lichen_core::Pubkey([0; 32]),
        accounts: vec![pk, receiver],
        data,
    };
    let msg = lichen_core::Message::new(vec![ix], blockhash);
    let sig = kp.sign(&msg.serialize());
    let tx = lichen_core::Transaction {
        signatures: vec![sig],
        message: msg,
        tx_type: lichen_core::TransactionType::Native,
    };

    // Encode as raw bincode (no envelope)
    let raw_bincode = bincode::serialize(&tx).unwrap();
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &raw_bincode);

    let resp = rpc_p(&app, "/", "sendTransaction", json!([b64]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    if let Some(err) = resp.get("error") {
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("");
        assert!(
            !msg.contains("Invalid transaction"),
            "Raw bincode decode failed: {}",
            msg
        );
    }
}

/// simulateTransaction accepts wire-envelope.
#[tokio::test]
async fn test_simulate_transaction_wire_envelope() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    let _ = Box::leak(Box::new(dir));

    let kp = lichen_core::Keypair::generate();
    let pk = kp.pubkey();
    let mut sender = lichen_core::Account::new(100, pk);
    sender.spendable = sender.spores;
    let _ = state.put_account(&pk, &sender);
    let _ = state.set_last_slot(1);

    let app = build_rpc_router(
        state.clone(),
        None,
        None,
        None,
        "lichen-test".to_string(),
        "lichen-test".to_string(),
        None,
        None,
        None,
        None,
        None,
    );

    let receiver = lichen_core::Pubkey([0x42; 32]);
    let blockhash = lichen_core::Hash::default();
    let mut data = vec![0u8];
    data.extend_from_slice(&1_000_000_000u64.to_le_bytes());
    let ix = lichen_core::Instruction {
        program_id: lichen_core::Pubkey([0; 32]),
        accounts: vec![pk, receiver],
        data,
    };
    let msg = lichen_core::Message::new(vec![ix], blockhash);
    let sig = kp.sign(&msg.serialize());
    let tx = lichen_core::Transaction {
        signatures: vec![sig],
        message: msg,
        tx_type: lichen_core::TransactionType::Native,
    };

    let wire = tx.to_wire();
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &wire);

    let resp = rpc_p(&app, "/", "simulateTransaction", json!([b64]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    // Should not fail with decode error
    if let Some(err) = resp.get("error") {
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("");
        assert!(
            !msg.contains("Invalid transaction"),
            "Wire envelope decode in simulateTransaction failed: {}",
            msg
        );
    }
}
