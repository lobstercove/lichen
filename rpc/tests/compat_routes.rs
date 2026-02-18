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

#[tokio::test]
async fn test_solana_health_route() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
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

    let response = rpc_call(&app, "/solana", "getHealth").await.unwrap();
    assert_eq!(response["result"], "ok");
}

#[tokio::test]
async fn test_evm_chain_id_route() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
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

    let response = rpc_call(&app, "/evm", "eth_chainId").await.unwrap();
    let result = response["result"].as_str().unwrap_or_default();
    assert!(result.starts_with("0x"));
}
