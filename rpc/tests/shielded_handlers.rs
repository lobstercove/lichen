// ═══════════════════════════════════════════════════════════════════════════════
// Shielded Pool RPC Integration Tests
//
// Tests both JSON-RPC methods and REST endpoints for the shielded pool module.
// Covers: pool state, Merkle root, Merkle path, nullifier checks, commitments,
// and shielded transaction submission validation.
// ═══════════════════════════════════════════════════════════════════════════════

use axum::body::{to_bytes, Body};
use axum::http::Request;
use moltchain_core::zk::{MerkleTree, ShieldedPoolState};
use moltchain_core::StateStore;
use moltchain_rpc::build_rpc_router;
use serde_json::json;
use tower::util::ServiceExt;

type RpcResult = Result<serde_json::Value, String>;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

async fn rpc_call(app: &axum::Router, method: &str) -> RpcResult {
    rpc_call_with_params(app, method, json!([])).await
}

async fn rpc_call_with_params(
    app: &axum::Router,
    method: &str,
    params: serde_json::Value,
) -> RpcResult {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params
    });

    let request = Request::post("/")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .map_err(|e| format!("request error: {}", e))?;

    let response = app
        .clone()
        .oneshot(request)
        .await
        .map_err(|e| format!("response error: {}", e))?;

    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|e| format!("body error: {}", e))?;

    serde_json::from_slice(&body).map_err(|e| format!("json error: {}", e))
}

async fn rest_get(app: &axum::Router, path: &str) -> RpcResult {
    let request = Request::get(path)
        .body(Body::empty())
        .map_err(|e| format!("request error: {}", e))?;

    let response = app
        .clone()
        .oneshot(request)
        .await
        .map_err(|e| format!("response error: {}", e))?;

    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|e| format!("body error: {}", e))?;

    serde_json::from_slice(&body).map_err(|e| format!("json error: {}", e))
}

async fn rest_post(app: &axum::Router, path: &str, body_json: serde_json::Value) -> RpcResult {
    let request = Request::post(path)
        .header("content-type", "application/json")
        .body(Body::from(body_json.to_string()))
        .map_err(|e| format!("request error: {}", e))?;

    let response = app
        .clone()
        .oneshot(request)
        .await
        .map_err(|e| format!("response error: {}", e))?;

    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|e| format!("body error: {}", e))?;

    serde_json::from_slice(&body).map_err(|e| format!("json error: {}", e))
}

/// Create a test app with an empty shielded pool (default state).
fn create_empty_app() -> axum::Router {
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

/// Create a test app with pre-populated shielded pool state.
/// Inserts `n_commitments` commitments and marks `spent_nullifiers` as spent.
fn create_populated_app(n_commitments: u64, spent_nullifiers: &[[u8; 32]]) -> axum::Router {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");

    // Build a Merkle tree and insert commitments
    let mut tree = MerkleTree::new();
    for i in 0..n_commitments {
        // Create a deterministic commitment: sha256(i)
        let commitment = test_commitment(i);
        state
            .insert_shielded_commitment(i, &commitment)
            .expect("insert commitment");
        tree.insert(commitment);
    }

    // Mark nullifiers as spent
    for nullifier in spent_nullifiers {
        state
            .mark_nullifier_spent(nullifier)
            .expect("mark nullifier");
    }

    // Store pool state
    let pool_state = ShieldedPoolState {
        merkle_root: tree.root(),
        commitment_count: n_commitments,
        total_shielded: n_commitments * 1_000_000_000, // 1 MOLT per commitment
        vk_shield_hash: [0xAA; 32],
        vk_unshield_hash: [0xBB; 32],
        vk_transfer_hash: [0xCC; 32],
        nullifier_count: 0,
        shield_count: 0,
        unshield_count: 0,
        transfer_count: 0,
    };
    state
        .put_shielded_pool_state(&pool_state)
        .expect("put pool state");

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

/// Generate a deterministic commitment for testing: sha256(index_le_bytes).
fn test_commitment(index: u64) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(index.to_le_bytes());
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// Generate a deterministic nullifier for testing: sha256("null" || index_le_bytes).
fn test_nullifier(index: u64) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"null");
    hasher.update(index.to_le_bytes());
    let result = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

// ═══════════════════════════════════════════════════════════════════════════════
// JSON-RPC Tests
// ═══════════════════════════════════════════════════════════════════════════════

// ── getShieldedPoolState ─────────────────────────────────────────────────────

#[tokio::test]
async fn test_rpc_get_shielded_pool_state_empty() {
    let app = create_empty_app();
    let resp = rpc_call(&app, "getShieldedPoolState").await.unwrap();
    let result = &resp["result"];

    assert_eq!(result["commitmentCount"], 0);
    assert_eq!(result["totalShielded"], 0);
    // Empty Merkle root should be a 64-char hex string (32 bytes)
    assert_eq!(result["merkleRoot"].as_str().unwrap().len(), 64);
}

#[tokio::test]
async fn test_rpc_get_shielded_pool_state_populated() {
    let app = create_populated_app(5, &[]);
    let resp = rpc_call(&app, "getShieldedPoolState").await.unwrap();
    let result = &resp["result"];

    assert_eq!(result["commitmentCount"], 5);
    assert_eq!(result["totalShielded"], 5_000_000_000u64);
    assert_eq!(result["totalShieldedMolt"], "5.000000000");
    assert_eq!(
        result["vkShieldHash"].as_str().unwrap(),
        hex::encode([0xAA; 32])
    );
}

// ── getShieldedMerkleRoot ────────────────────────────────────────────────────

#[tokio::test]
async fn test_rpc_get_shielded_merkle_root_empty() {
    let app = create_empty_app();
    let resp = rpc_call(&app, "getShieldedMerkleRoot").await.unwrap();
    let result = &resp["result"];

    assert_eq!(result["commitmentCount"], 0);
    assert!(result["merkleRoot"].as_str().unwrap().len() == 64);
}

#[tokio::test]
async fn test_rpc_get_shielded_merkle_root_populated() {
    let app = create_populated_app(3, &[]);
    let resp = rpc_call(&app, "getShieldedMerkleRoot").await.unwrap();
    let result = &resp["result"];

    assert_eq!(result["commitmentCount"], 3);
    // Root should match what we stored
    let root_hex = result["merkleRoot"].as_str().unwrap();
    assert_eq!(root_hex.len(), 64);
    // Rebuild tree locally to verify the root
    let mut tree = MerkleTree::new();
    for i in 0..3u64 {
        tree.insert(test_commitment(i));
    }
    assert_eq!(root_hex, hex::encode(tree.root()));
}

// ── getShieldedMerklePath ────────────────────────────────────────────────────

#[tokio::test]
async fn test_rpc_get_shielded_merkle_path_valid() {
    let app = create_populated_app(4, &[]);
    let resp = rpc_call_with_params(&app, "getShieldedMerklePath", json!([2]))
        .await
        .unwrap();
    let result = &resp["result"];

    assert_eq!(result["index"], 2);
    // Siblings should be an array of TREE_DEPTH hex strings
    let siblings = result["siblings"].as_array().unwrap();
    assert_eq!(siblings.len(), moltchain_core::zk::TREE_DEPTH);
    for s in siblings {
        assert_eq!(s.as_str().unwrap().len(), 64);
    }
    // pathBits should be an array of TREE_DEPTH booleans
    let bits = result["pathBits"].as_array().unwrap();
    assert_eq!(bits.len(), moltchain_core::zk::TREE_DEPTH);
    // Root should be present
    assert_eq!(result["root"].as_str().unwrap().len(), 64);
}

#[tokio::test]
async fn test_rpc_get_shielded_merkle_path_out_of_range() {
    let app = create_populated_app(2, &[]);
    let resp = rpc_call_with_params(&app, "getShieldedMerklePath", json!([99]))
        .await
        .unwrap();

    // Should return an error
    assert!(resp["error"].is_object());
    let err = &resp["error"];
    assert_eq!(err["code"], -32001);
    assert!(err["message"].as_str().unwrap().contains("out of range"));
}

#[tokio::test]
async fn test_rpc_get_shielded_merkle_path_missing_params() {
    let app = create_populated_app(2, &[]);
    let resp = rpc_call(&app, "getShieldedMerklePath").await.unwrap();

    assert!(resp["error"].is_object());
    assert_eq!(resp["error"]["code"], -32602);
}

// ── isNullifierSpent ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_rpc_is_nullifier_spent_not_spent() {
    let app = create_populated_app(1, &[]);
    let null_hex = hex::encode(test_nullifier(0));
    let resp = rpc_call_with_params(&app, "isNullifierSpent", json!([null_hex]))
        .await
        .unwrap();
    let result = &resp["result"];

    assert_eq!(result["spent"], false);
    assert_eq!(result["nullifier"], null_hex);
}

#[tokio::test]
async fn test_rpc_is_nullifier_spent_is_spent() {
    let null0 = test_nullifier(0);
    let app = create_populated_app(1, &[null0]);
    let null_hex = hex::encode(null0);
    let resp = rpc_call_with_params(&app, "isNullifierSpent", json!([null_hex]))
        .await
        .unwrap();
    let result = &resp["result"];

    assert_eq!(result["spent"], true);
}

#[tokio::test]
async fn test_rpc_is_nullifier_spent_invalid_hex() {
    let app = create_empty_app();
    let resp = rpc_call_with_params(&app, "isNullifierSpent", json!(["zzzzzz"]))
        .await
        .unwrap();

    assert!(resp["error"].is_object());
    assert_eq!(resp["error"]["code"], -32602);
}

#[tokio::test]
async fn test_rpc_is_nullifier_spent_wrong_length() {
    let app = create_empty_app();
    let resp = rpc_call_with_params(&app, "isNullifierSpent", json!(["aabb"]))
        .await
        .unwrap();

    assert!(resp["error"].is_object());
    assert_eq!(resp["error"]["code"], -32602);
}

// ── getShieldedCommitments ───────────────────────────────────────────────────

#[tokio::test]
async fn test_rpc_get_shielded_commitments_empty() {
    let app = create_empty_app();
    let resp = rpc_call(&app, "getShieldedCommitments").await.unwrap();
    let result = &resp["result"];

    assert_eq!(result["total"], 0);
    assert_eq!(result["commitments"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_rpc_get_shielded_commitments_all() {
    let app = create_populated_app(5, &[]);
    let resp = rpc_call(&app, "getShieldedCommitments").await.unwrap();
    let result = &resp["result"];

    assert_eq!(result["total"], 5);
    let comms = result["commitments"].as_array().unwrap();
    assert_eq!(comms.len(), 5);

    // Verify first commitment matches our deterministic generation
    assert_eq!(comms[0]["index"], 0);
    assert_eq!(
        comms[0]["commitment"].as_str().unwrap(),
        hex::encode(test_commitment(0))
    );
}

#[tokio::test]
async fn test_rpc_get_shielded_commitments_pagination() {
    let app = create_populated_app(10, &[]);
    let resp = rpc_call_with_params(
        &app,
        "getShieldedCommitments",
        json!([{"from": 3, "limit": 4}]),
    )
    .await
    .unwrap();
    let result = &resp["result"];

    assert_eq!(result["total"], 10);
    assert_eq!(result["from"], 3);
    assert_eq!(result["limit"], 4);
    let comms = result["commitments"].as_array().unwrap();
    assert_eq!(comms.len(), 4);
    assert_eq!(comms[0]["index"], 3);
    assert_eq!(comms[3]["index"], 6);
}

#[tokio::test]
async fn test_rpc_get_shielded_commitments_limit_capped() {
    let app = create_populated_app(3, &[]);
    // Request limit of 5000 — should be capped to 1000
    let resp = rpc_call_with_params(
        &app,
        "getShieldedCommitments",
        json!([{"from": 0, "limit": 5000}]),
    )
    .await
    .unwrap();
    let result = &resp["result"];

    // With only 3 commitments, we get 3 regardless of limit
    assert_eq!(result["commitments"].as_array().unwrap().len(), 3);
    // But the limit field should show the capped value
    assert_eq!(result["limit"], 1000);
}

// ═══════════════════════════════════════════════════════════════════════════════
// REST Endpoint Tests
// ═══════════════════════════════════════════════════════════════════════════════

// ── GET /api/v1/shielded/pool ────────────────────────────────────────────────

#[tokio::test]
async fn test_rest_get_pool_state_empty() {
    let app = create_empty_app();
    let resp = rest_get(&app, "/api/v1/shielded/pool").await.unwrap();

    assert_eq!(resp["success"], true);
    let data = &resp["data"];
    assert_eq!(data["commitmentCount"], 0);
    assert_eq!(data["totalShielded"], 0);
}

#[tokio::test]
async fn test_rest_get_pool_state_populated() {
    let app = create_populated_app(7, &[]);
    let resp = rest_get(&app, "/api/v1/shielded/pool").await.unwrap();

    assert_eq!(resp["success"], true);
    let data = &resp["data"];
    assert_eq!(data["commitmentCount"], 7);
    assert_eq!(data["totalShielded"], 7_000_000_000u64);
}

// ── GET /api/v1/shielded/merkle-root ─────────────────────────────────────────

#[tokio::test]
async fn test_rest_get_merkle_root() {
    let app = create_populated_app(2, &[]);
    let resp = rest_get(&app, "/api/v1/shielded/merkle-root")
        .await
        .unwrap();

    assert_eq!(resp["success"], true);
    assert_eq!(resp["data"]["commitmentCount"], 2);
    assert_eq!(resp["data"]["merkleRoot"].as_str().unwrap().len(), 64);
}

// ── GET /api/v1/shielded/merkle-path/:index ──────────────────────────────────

#[tokio::test]
async fn test_rest_get_merkle_path_valid() {
    let app = create_populated_app(3, &[]);
    let resp = rest_get(&app, "/api/v1/shielded/merkle-path/1")
        .await
        .unwrap();

    assert_eq!(resp["success"], true);
    let data = &resp["data"];
    assert_eq!(data["index"], 1);
    assert_eq!(
        data["siblings"].as_array().unwrap().len(),
        moltchain_core::zk::TREE_DEPTH
    );
    assert_eq!(
        data["pathBits"].as_array().unwrap().len(),
        moltchain_core::zk::TREE_DEPTH
    );
}

#[tokio::test]
async fn test_rest_get_merkle_path_out_of_range() {
    let app = create_populated_app(2, &[]);
    let resp = rest_get(&app, "/api/v1/shielded/merkle-path/100")
        .await
        .unwrap();

    assert_eq!(resp["success"], false);
    assert!(resp["error"].as_str().unwrap().contains("out of range"));
}

// ── GET /api/v1/shielded/nullifier/:hash ─────────────────────────────────────

#[tokio::test]
async fn test_rest_get_nullifier_not_spent() {
    let app = create_populated_app(1, &[]);
    let hash = hex::encode(test_nullifier(42));
    let resp = rest_get(&app, &format!("/api/v1/shielded/nullifier/{}", hash))
        .await
        .unwrap();

    assert_eq!(resp["success"], true);
    assert_eq!(resp["data"]["spent"], false);
}

#[tokio::test]
async fn test_rest_get_nullifier_spent() {
    let null0 = test_nullifier(0);
    let app = create_populated_app(1, &[null0]);
    let hash = hex::encode(null0);
    let resp = rest_get(&app, &format!("/api/v1/shielded/nullifier/{}", hash))
        .await
        .unwrap();

    assert_eq!(resp["success"], true);
    assert_eq!(resp["data"]["spent"], true);
}

#[tokio::test]
async fn test_rest_get_nullifier_invalid_hex() {
    let app = create_empty_app();
    let resp = rest_get(&app, "/api/v1/shielded/nullifier/xyz_invalid")
        .await
        .unwrap();

    assert_eq!(resp["success"], false);
    assert!(resp["error"].as_str().unwrap().contains("Invalid hex"));
}

// ── GET /api/v1/shielded/commitments ─────────────────────────────────────────

#[tokio::test]
async fn test_rest_get_commitments_default() {
    let app = create_populated_app(5, &[]);
    let resp = rest_get(&app, "/api/v1/shielded/commitments")
        .await
        .unwrap();

    assert_eq!(resp["success"], true);
    let data = &resp["data"];
    assert_eq!(data["total"], 5);
    assert_eq!(data["commitments"].as_array().unwrap().len(), 5);
}

#[tokio::test]
async fn test_rest_get_commitments_paginated() {
    let app = create_populated_app(10, &[]);
    let resp = rest_get(&app, "/api/v1/shielded/commitments?from=5&limit=3")
        .await
        .unwrap();

    assert_eq!(resp["success"], true);
    let data = &resp["data"];
    assert_eq!(data["from"], 5);
    assert_eq!(data["limit"], 3);
    let comms = data["commitments"].as_array().unwrap();
    assert_eq!(comms.len(), 3);
    assert_eq!(comms[0]["index"], 5);
}

// ── POST /api/v1/shielded/shield ─────────────────────────────────────────────

#[tokio::test]
async fn test_rest_submit_shield_invalid_base64() {
    let app = create_empty_app();
    let resp = rest_post(
        &app,
        "/api/v1/shielded/shield",
        json!({"transaction": "not-valid-base64!!!"}),
    )
    .await
    .unwrap();

    assert_eq!(resp["success"], false);
    assert!(resp["error"].as_str().unwrap().contains("Invalid base64"));
}

#[tokio::test]
async fn test_rest_submit_shield_invalid_transaction() {
    let app = create_empty_app();
    // Valid base64 but not a valid transaction
    let resp = rest_post(
        &app,
        "/api/v1/shielded/shield",
        json!({"transaction": "AAAA"}),
    )
    .await
    .unwrap();

    assert_eq!(resp["success"], false);
    assert!(resp["error"]
        .as_str()
        .unwrap()
        .contains("Invalid transaction"));
}

// ── POST /api/v1/shielded/unshield ───────────────────────────────────────────

#[tokio::test]
async fn test_rest_submit_unshield_invalid() {
    let app = create_empty_app();
    let resp = rest_post(
        &app,
        "/api/v1/shielded/unshield",
        json!({"transaction": "AAAA"}),
    )
    .await
    .unwrap();

    assert_eq!(resp["success"], false);
}

// ── POST /api/v1/shielded/transfer ───────────────────────────────────────────

#[tokio::test]
async fn test_rest_submit_transfer_invalid() {
    let app = create_empty_app();
    let resp = rest_post(
        &app,
        "/api/v1/shielded/transfer",
        json!({"transaction": "AAAA"}),
    )
    .await
    .unwrap();

    assert_eq!(resp["success"], false);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Cross-validation tests — ensure JSON-RPC and REST return consistent results
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_rpc_rest_merkle_root_consistency() {
    let app = create_populated_app(4, &[]);

    // Get root via JSON-RPC
    let rpc_resp = rpc_call(&app, "getShieldedMerkleRoot").await.unwrap();
    let rpc_root = rpc_resp["result"]["merkleRoot"].as_str().unwrap();

    // Get root via REST
    let rest_resp = rest_get(&app, "/api/v1/shielded/merkle-root")
        .await
        .unwrap();
    let rest_root = rest_resp["data"]["merkleRoot"].as_str().unwrap();

    assert_eq!(rpc_root, rest_root);
}

#[tokio::test]
async fn test_rpc_rest_commitments_consistency() {
    let app = create_populated_app(5, &[]);

    // Get commitments via JSON-RPC
    let rpc_resp = rpc_call_with_params(
        &app,
        "getShieldedCommitments",
        json!([{"from": 0, "limit": 5}]),
    )
    .await
    .unwrap();
    let rpc_comms = rpc_resp["result"]["commitments"].as_array().unwrap();

    // Get commitments via REST
    let rest_resp = rest_get(&app, "/api/v1/shielded/commitments?from=0&limit=5")
        .await
        .unwrap();
    let rest_comms = rest_resp["data"]["commitments"].as_array().unwrap();

    assert_eq!(rpc_comms.len(), rest_comms.len());
    for i in 0..rpc_comms.len() {
        assert_eq!(
            rpc_comms[i]["commitment"].as_str().unwrap(),
            rest_comms[i]["commitment"].as_str().unwrap()
        );
    }
}

#[tokio::test]
async fn test_rpc_rest_nullifier_consistency() {
    let null0 = test_nullifier(99);
    let app = create_populated_app(1, &[null0]);
    let hash = hex::encode(null0);

    // JSON-RPC
    let rpc_resp = rpc_call_with_params(&app, "isNullifierSpent", json!([&hash]))
        .await
        .unwrap();
    assert_eq!(rpc_resp["result"]["spent"], true);

    // REST
    let rest_resp = rest_get(&app, &format!("/api/v1/shielded/nullifier/{}", hash))
        .await
        .unwrap();
    assert_eq!(rest_resp["data"]["spent"], true);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Merkle proof verification test — end-to-end correctness check
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn test_merkle_proof_roundtrip_verification() {
    let n = 8u64;
    let app = create_populated_app(n, &[]);

    // Build the same tree locally
    let mut local_tree = MerkleTree::new();
    for i in 0..n {
        local_tree.insert(test_commitment(i));
    }

    // For each leaf, get the proof from RPC and verify it locally
    for idx in 0..n {
        let resp = rpc_call_with_params(&app, "getShieldedMerklePath", json!([idx]))
            .await
            .unwrap();
        let result = &resp["result"];

        // The root from RPC should match our local tree
        let rpc_root = result["root"].as_str().unwrap();
        assert_eq!(rpc_root, hex::encode(local_tree.root()));

        // Verify the proof using the MerkleTree's static verifier
        let siblings: Vec<[u8; 32]> = result["siblings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                let bytes = hex::decode(s.as_str().unwrap()).unwrap();
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                arr
            })
            .collect();
        let path_bits: Vec<bool> = result["pathBits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_bool().unwrap())
            .collect();

        let merkle_path = moltchain_core::zk::MerklePath {
            siblings,
            path_bits,
            index: idx,
        };

        let leaf = test_commitment(idx);
        let root = local_tree.root();
        assert!(
            MerkleTree::verify_proof(&root, &leaf, &merkle_path),
            "Merkle proof verification failed for index {}",
            idx
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Processor → RPC state consistency test
//
// Simulates what would happen after a processor processes shielded transactions:
// the RPC endpoints should reflect the updated state.
// ═══════════════════════════════════════════════════════════════════════════════

/// Helper: create a test app from an existing StateStore (for processor→RPC tests).
fn create_app_from_state(state: StateStore) -> axum::Router {
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

#[tokio::test]
async fn test_rpc_reflects_processor_shielded_state() {
    use moltchain_core::zk::ShieldedPoolState;

    // Simulate processor state after processing:
    //   - 3 shield transactions
    //   - 1 nullifier spent (from an unshield)
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");

    let mut tree = MerkleTree::new();
    let commitments = [
        test_commitment(100),
        test_commitment(200),
        test_commitment(300),
    ];

    for (i, comm) in commitments.iter().enumerate() {
        state.insert_shielded_commitment(i as u64, comm).unwrap();
        tree.insert(*comm);
    }

    let nullifier = test_nullifier(100);
    state.mark_nullifier_spent(&nullifier).unwrap();

    // Pool state: 3 commitments, 1.5 MOLT remaining (after 0.5 MOLT unshielded)
    let pool = ShieldedPoolState {
        merkle_root: tree.root(),
        commitment_count: 3,
        total_shielded: 1_500_000_000,
        vk_shield_hash: [0x11; 32],
        vk_unshield_hash: [0x22; 32],
        vk_transfer_hash: [0x33; 32],
        nullifier_count: 1,
        shield_count: 3,
        unshield_count: 1,
        transfer_count: 0,
    };
    state.put_shielded_pool_state(&pool).unwrap();

    let _ = Box::leak(Box::new(dir));
    let app = create_app_from_state(state);

    // ── Verify JSON-RPC getShieldedPoolState ──────────────────────────────
    let resp = rpc_call(&app, "getShieldedPoolState").await.unwrap();
    let result = &resp["result"];
    assert_eq!(result["commitmentCount"], 3);
    assert_eq!(result["totalShielded"], 1_500_000_000u64);
    assert_eq!(
        result["merkleRoot"].as_str().unwrap(),
        hex::encode(tree.root())
    );

    // ── Verify nullifier is spent via JSON-RPC ───────────────────────────
    let null_hex = hex::encode(nullifier);
    let resp = rpc_call_with_params(&app, "isNullifierSpent", json!([&null_hex]))
        .await
        .unwrap();
    assert_eq!(resp["result"]["spent"], true);

    // ── Verify non-spent nullifier ───────────────────────────────────────
    let fresh_null = hex::encode(test_nullifier(999));
    let resp = rpc_call_with_params(&app, "isNullifierSpent", json!([&fresh_null]))
        .await
        .unwrap();
    assert_eq!(resp["result"]["spent"], false);

    // ── Verify commitments via REST ──────────────────────────────────────
    let resp = rest_get(&app, "/api/v1/shielded/commitments?from=0&limit=10")
        .await
        .unwrap();
    assert_eq!(resp["success"], true);
    let comms = resp["data"]["commitments"].as_array().unwrap();
    assert_eq!(comms.len(), 3);
    assert_eq!(
        comms[0]["commitment"].as_str().unwrap(),
        hex::encode(commitments[0])
    );
    assert_eq!(
        comms[2]["commitment"].as_str().unwrap(),
        hex::encode(commitments[2])
    );

    // ── Verify Merkle path via REST ──────────────────────────────────────
    let resp = rest_get(&app, "/api/v1/shielded/merkle-path/1")
        .await
        .unwrap();
    assert_eq!(resp["success"], true);
    let path_root = resp["data"]["root"].as_str().unwrap();
    assert_eq!(path_root, hex::encode(tree.root()));

    // ── Verify pool via REST ─────────────────────────────────────────────
    let resp = rest_get(&app, "/api/v1/shielded/pool").await.unwrap();
    assert_eq!(resp["data"]["totalShielded"], 1_500_000_000u64);
    assert_eq!(resp["data"]["totalShieldedMolt"], "1.500000000");
}
