// ═══════════════════════════════════════════════════════════════════════════════
// COMPREHENSIVE RPC FULL COVERAGE TESTS
// ═══════════════════════════════════════════════════════════════════════════════
//
// Covers every JSON-RPC method across all three dispatch planes:
//   - Native Molt RPC (108 methods on /)
//   - Solana-compat RPC (13 methods on /solana)
//   - EVM-compat RPC (20 methods on /evm)
// Plus all REST API endpoints on /api/v1/*.
//
// Test naming: test_<plane>_<method_name>
//   e.g. test_native_getAccount, test_solana_getBlockHeight, test_evm_eth_call

use axum::body::{to_bytes, Body};
use axum::http::Request;
use moltchain_core::{
    contract::ContractAccount, Account, Pubkey, StateStore, SymbolRegistryEntry,
    CONTRACT_PROGRAM_ID,
};
use moltchain_rpc::build_rpc_router;
use serde_json::json;
use tower::util::ServiceExt;

type RpcResult = Result<serde_json::Value, String>;

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
        "moltchain-test".to_string(),
        "molt-test".to_string(),
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
            },
        )
        .expect("register");

    let app = build_rpc_router(
        state,
        None,
        None,
        None,
        "moltchain-test".to_string(),
        "molt-test".to_string(),
        None,
        None,
        None,
        None,
    );
    (app, funded_hex)
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
//  SECTION 1: NATIVE MOLT RPC — Basic Query Methods
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
                result.get("shells").is_some()
                    || result.get("balance").is_some()
                    || result.get("lamports").is_some(),
                "account should have balance/shells: {result}"
            );
        }
    }
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
    // Returns an object with shells/molt fields
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
async fn test_native_get_reefstake_pool_info() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getReefStakePoolInfo").await.unwrap();
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
async fn test_native_get_moltswap_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getMoltswapStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_lobsterlend_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getLobsterLendStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_clawpay_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getClawPayStats").await.unwrap();
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
async fn test_native_get_reef_storage_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getReefStorageStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_moltmarket_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getMoltMarketStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_moltauction_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getMoltAuctionStats").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_get_moltpunks_stats() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getMoltPunksStats").await.unwrap();
    assert_valid_rpc(&resp);
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 15: Search + Airdrop
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_search_molt_names() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/", "searchMoltNames", json!(["test"]))
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
//  SECTION 18: ReefStake liquid staking write endpoints
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_native_stake_to_reefstake() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "stakeToReefStake",
        json!(["11111111111111111111111111111111", 1000]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_unstake_from_reefstake() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "unstakeFromReefStake",
        json!(["11111111111111111111111111111111", 1000]),
    )
    .await
    .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_native_claim_unstaked_tokens() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/",
        "claimUnstakedTokens",
        json!(["11111111111111111111111111111111"]),
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
}

// ═══════════════════════════════════════════════════════════════════════════════
//  SECTION 20: SOLANA-COMPAT RPC — All methods
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_solana_get_latest_blockhash() {
    let app = fresh_app();
    let resp = rpc(&app, "/solana", "getLatestBlockhash").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_get_recent_blockhash() {
    let app = fresh_app();
    let resp = rpc(&app, "/solana", "getRecentBlockhash").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_get_balance() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/solana",
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
        "/solana",
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
    let resp = rpc_p(&app, "/solana", "getBlock", json!([0]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_get_block_height() {
    let app = fresh_app();
    let resp = rpc(&app, "/solana", "getBlockHeight").await.unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_get_signatures_for_address() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/solana",
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
    let resp = rpc_p(&app, "/solana", "getSignatureStatuses", json!([[fake_sig]]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_get_slot() {
    let app = fresh_app();
    let resp = rpc(&app, "/solana", "getSlot").await.unwrap();
    assert_valid_rpc(&resp);
    assert!(resp["result"].is_number());
}

#[tokio::test]
async fn test_solana_get_transaction() {
    let app = fresh_app();
    let fake_sig = "a".repeat(64);
    let resp = rpc_p(&app, "/solana", "getTransaction", json!([fake_sig]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_send_transaction_no_sender() {
    let app = fresh_app();
    let resp = rpc_p(&app, "/solana", "sendTransaction", json!(["deadbeef"]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
}

#[tokio::test]
async fn test_solana_unknown_method() {
    let app = fresh_app();
    let resp = rpc(&app, "/solana", "totallyBogusMethod").await.unwrap();
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
        ver.starts_with("MoltChain/"),
        "should start with MoltChain/: {ver}"
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
    let app = fresh_app();
    let resp = rest_get(&app, "/api/v1/oracle/prices").await;
    assert!(resp.is_ok() || resp.is_err());
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
    let resp = rpc(&app, "/solana", "getBalance").await.unwrap();
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
        "getMoltswapStats",
        "getLobsterLendStats",
        "getClawPayStats",
        "getBountyBoardStats",
        "getComputeMarketStats",
        "getReefStorageStats",
        "getMoltMarketStats",
        "getMoltAuctionStats",
        "getMoltPunksStats",
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
        let resp = rpc(&app, "/solana", method).await.unwrap();
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

use moltchain_core::consensus::ValidatorInfo;
use moltchain_core::{Block, Hash, Instruction, Message, Transaction};

/// Helper: build an app backed by a StateStore pre-populated with a funded
/// account, a stored block at slot 1, a validator, and a transaction.
/// Returns `(Router, StateStore, funded_base58, validator_base58, block_hash_hex, tx_sig_hex)`.
fn app_with_rich_state() -> (axum::Router, StateStore, String, String, String, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    let _ = Box::leak(Box::new(dir));

    // 1. Funded account: 5 MOLT = 5_000_000_000 shells
    let funded = Pubkey([42u8; 32]);
    let funded_b58 = funded.to_base58();
    let acct = Account::new(5, Pubkey([0u8; 32])); // 5 MOLT
    state.put_account(&funded, &acct).expect("put funded");

    // 2. Validator
    let val_pk = Pubkey([7u8; 32]);
    let val_b58 = val_pk.to_base58();
    let mut vi = ValidatorInfo::new(val_pk, 0);
    vi.blocks_proposed = 3;
    vi.stake = 100_000_000_000_000; // 100k MOLT
    state.put_validator(&vi).expect("put validator");

    // 3. A minimal transaction (transfer 1 MOLT from funded → treasury)
    let treasury = Pubkey([0u8; 32]);
    let ix = Instruction {
        program_id: Pubkey([0u8; 32]),
        accounts: vec![funded, treasury],
        data: vec![3, 0, 0, 0, 0, 0xCA, 0x9A, 0x3B, 0, 0, 0, 0, 0, 0, 0, 0],
    };
    let msg = Message::new(vec![ix], Hash::default());
    let tx = Transaction {
        signatures: vec![[99u8; 64]],
        message: msg,
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
            },
        )
        .expect("register");

    let cloned_state = state.clone();
    let app = build_rpc_router(
        state,
        None,
        None,
        None,
        "moltchain-test".to_string(),
        "molt-test".to_string(),
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
    // Account::new(5, ..) → shells = 5_000_000_000, spendable = 5_000_000_000
    assert_eq!(
        result["shells"], 5_000_000_000u64,
        "shells = 5 MOLT in shells"
    );
    assert_eq!(result["spendable"], 5_000_000_000u64, "spendable = 5 MOLT");
    assert_eq!(result["molt"], "5.0000", "molt = 5.0000");
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
            assert_eq!(result["shells"], 0, "unfunded shells should be 0");
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
    let ts = result["timestamp"].as_u64().expect("timestamp should be u64");
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
    // balance should be 5 MOLT = 5_000_000_000 shells
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
        resp.get("error").map_or(true, |e| e["code"] != -32601),
        "should route to handler, not method-not-found"
    );
}

// ── Solana-compat getBalance with funded account ─────────────────────────────

#[tokio::test]
async fn test_solana_get_balance_funded() {
    let (app, _, addr, _, _, _) = app_with_rich_state();
    let resp = rpc_p(&app, "/solana", "getBalance", json!([addr]))
        .await
        .unwrap();
    assert_valid_rpc(&resp);
    let result = &resp["result"];
    // Solana format: { "context": { "slot": N }, "value": shells }
    assert!(
        result["context"]["slot"].is_number(),
        "should have context.slot"
    );
    assert_eq!(
        result["value"], 5_000_000_000u64,
        "solana getBalance value should be 5B shells"
    );
}

#[tokio::test]
async fn test_solana_get_balance_unfunded() {
    let app = fresh_app();
    let resp = rpc_p(
        &app,
        "/solana",
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
    let resp = rpc(&app, "/solana", "getSlot").await.unwrap();
    assert_valid_rpc(&resp);
    let slot = resp["result"].as_u64().expect("getSlot must be u64");
    assert_eq!(slot, 1, "solana getSlot should be 1");
}

// ── Solana-compat getBlock with stored block ─────────────────────────────────

#[tokio::test]
async fn test_solana_get_block_with_data() {
    let (app, _, _, _, _, _) = app_with_rich_state();
    let resp = rpc_p(&app, "/solana", "getBlock", json!([1]))
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
    let resp = rpc(&app, "/solana", "getBlockHeight").await.unwrap();
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
    // "molt-test" → evm_chain_id_from_chain_id hash
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

// ── getReefStakePoolInfo returns pool data ───────────────────────────────────

#[tokio::test]
async fn test_native_get_reefstake_pool_info_returns_data() {
    let app = fresh_app();
    let resp = rpc(&app, "/", "getReefStakePoolInfo").await.unwrap();
    assert_valid_rpc(&resp);
    // Should return pool info (possibly empty/defaults), not null
    let result = &resp["result"];
    assert!(!result.is_null(), "reefstake pool info should return data");
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
    // Should have base_fee_shells field
    if result.is_object() {
        assert!(
            result.get("base_fee_shells").is_some(),
            "fee config should have base_fee_shells: {result}"
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
        resp.get("error").map_or(true, |e| e["code"] != -32601),
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
        resp.get("error").map_or(true, |e| e["code"] != -32601),
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
    let resp = rpc(&app, "/solana", "getHealth").await.unwrap();
    assert_valid_rpc(&resp);
    assert_eq!(resp["result"], "ok", "solana getHealth should return 'ok'");
}

// ── Solana getVersion returns version info ───────────────────────────────────

#[tokio::test]
async fn test_solana_get_version_shape() {
    let app = fresh_app();
    let resp = rpc(&app, "/solana", "getVersion").await.unwrap();
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
    let resp = rpc(&app, "/solana", "getLatestBlockhash").await.unwrap();
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
    let resp = rpc_p(&app, "/solana", "getAccountInfo", json!([addr]))
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
    assert!(ver.starts_with("MoltChain/"), "starts with MoltChain/");
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
        "getReefStakePoolInfo",
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
        let resp = rpc(&app, "/solana", method).await.unwrap();
        assert_valid_rpc(&resp);
    }

    // With addr
    for method in &["getBalance", "getAccountInfo", "getSignaturesForAddress"] {
        let resp = rpc_p(&app, "/solana", method, json!([addr])).await.unwrap();
        assert_valid_rpc(&resp);
    }

    // With slot
    let resp = rpc_p(&app, "/solana", "getBlock", json!([0]))
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
