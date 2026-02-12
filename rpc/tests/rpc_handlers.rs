// RPC handler integration tests
// Tests for core JSON-RPC endpoints

use axum::body::{to_bytes, Body};
use axum::http::Request;
use moltchain_core::StateStore;
use moltchain_rpc::build_rpc_router;
use serde_json::json;
use tower::util::ServiceExt;

type RpcResult = Result<serde_json::Value, String>;

async fn rpc_call(app: &axum::Router, path: &str, method: &str) -> RpcResult {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": []
    });

    let request = Request::post(path)
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .map_err(|e| format!("request error: {}", e))?;

    let response = app
        .clone()
        .oneshot(request)
        .await
        .map_err(|e| format!("response error: {}", e))?;

    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|e| format!("body error: {}", e))?;
    if !status.is_success() {
        return Err(format!("status {}", status));
    }

    serde_json::from_slice(&body).map_err(|e| format!("json error: {}", e))
}

async fn rpc_call_with_params(
    app: &axum::Router,
    path: &str,
    method: &str,
    params: serde_json::Value,
) -> RpcResult {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params
    });

    let request = Request::post(path)
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .map_err(|e| format!("request error: {}", e))?;

    let response = app
        .clone()
        .oneshot(request)
        .await
        .map_err(|e| format!("response error: {}", e))?;

    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|e| format!("body error: {}", e))?;
    if !status.is_success() {
        return Err(format!("status {}", status));
    }

    serde_json::from_slice(&body).map_err(|e| format!("json error: {}", e))
}

fn create_test_app() -> axum::Router {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    // Leak the tempdir so it isn't deleted while the app exists
    let _ = Box::leak(Box::new(dir));
    build_rpc_router(
        state,
        None,
        None,
        None,
        "moltchain-test".to_string(),
        "molt-test".to_string(),
        None,
    )
}

// ============================================================================
// Health endpoint
// ============================================================================

#[tokio::test]
async fn test_health_endpoint() {
    let app = create_test_app();
    let response = rpc_call(&app, "/solana", "getHealth").await.unwrap();
    assert_eq!(response["result"], "ok");
}

// ============================================================================
// getVersion
// ============================================================================

#[tokio::test]
async fn test_get_version() {
    let app = create_test_app();
    let response = rpc_call(&app, "/solana", "getVersion").await.unwrap();
    // Should contain a "solana-core" or similar version field
    let result = &response["result"];
    assert!(result.is_object(), "getVersion should return an object");
}

// ============================================================================
// getSlot
// ============================================================================

#[tokio::test]
async fn test_get_slot() {
    let app = create_test_app();
    let response = rpc_call(&app, "/solana", "getSlot").await.unwrap();
    // Slot should be a number (0 or greater for fresh state)
    let result = &response["result"];
    assert!(
        result.is_number(),
        "getSlot should return a number, got: {:?}",
        result
    );
}

// ============================================================================
// getBalance
// ============================================================================

#[tokio::test]
async fn test_get_balance_nonexistent_account() {
    let app = create_test_app();
    // Use a random base58 address that won't exist
    let response = rpc_call_with_params(
        &app,
        "/solana",
        "getBalance",
        json!(["11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    // Should return a result (possibly 0 balance or an error)
    assert!(
        response.get("result").is_some() || response.get("error").is_some(),
        "Should return result or error: {:?}",
        response
    );
}

// ============================================================================
// getBlock
// ============================================================================

#[tokio::test]
async fn test_get_block_slot_zero() {
    let app = create_test_app();
    let response = rpc_call_with_params(&app, "/solana", "getBlock", json!([0]))
        .await
        .unwrap();
    // Slot 0 may or may not exist; we just check the response format
    assert!(
        response.get("result").is_some() || response.get("error").is_some(),
        "Should return result or error for block query: {:?}",
        response
    );
}

// ============================================================================
// JSON-RPC format validation
// ============================================================================

#[tokio::test]
async fn test_jsonrpc_response_format() {
    let app = create_test_app();
    let response = rpc_call(&app, "/solana", "getHealth").await.unwrap();

    // All JSON-RPC responses should have "jsonrpc" and "id"
    assert_eq!(
        response["jsonrpc"], "2.0",
        "Response should have jsonrpc: 2.0"
    );
    assert!(
        response.get("id").is_some(),
        "Response should have an id field"
    );
}

#[tokio::test]
async fn test_unknown_method_returns_error() {
    let app = create_test_app();
    let response = rpc_call(&app, "/solana", "nonExistentMethod")
        .await
        .unwrap();

    // Unknown method should return an error
    assert!(
        response.get("error").is_some(),
        "Unknown method should return error: {:?}",
        response
    );
}

// ============================================================================
// EVM compatibility endpoints
// ============================================================================

#[tokio::test]
async fn test_evm_chain_id() {
    let app = create_test_app();
    let response = rpc_call(&app, "/evm", "eth_chainId").await.unwrap();
    let result = response["result"].as_str().unwrap_or_default();
    assert!(
        result.starts_with("0x"),
        "Chain ID should be hex: {}",
        result
    );
}

#[tokio::test]
async fn test_evm_block_number() {
    let app = create_test_app();
    let response = rpc_call(&app, "/evm", "eth_blockNumber").await.unwrap();
    let result = &response["result"];
    // Should return hex-encoded block number
    assert!(
        result.is_string(),
        "eth_blockNumber should return a string, got: {:?}",
        result
    );
}
