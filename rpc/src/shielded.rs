// ═══════════════════════════════════════════════════════════════════════════════
// SHIELDED POOL REST + JSON-RPC ENDPOINTS
//
// REST routes (nested at /api/v1/shielded):
//   GET  /pool                 — full pool state (root, count, balance)
//   GET  /merkle-root          — current Merkle root only
//   GET  /merkle-path/:index   — Merkle proof for leaf at given index
//   GET  /nullifier/:hash      — check whether a nullifier has been spent
//   GET  /commitments          — paginated commitment list (?from=N&limit=M)
//   POST /shield               — submit a signed shield transaction (type 23)
//   POST /unshield             — submit a signed unshield transaction (type 24)
//   POST /transfer             — submit a signed shielded transfer (type 25)
//
// JSON-RPC methods (dispatched from handle_rpc):
//   getShieldedPoolState       — equivalent of GET /pool
//   getShieldedPoolStats       — alias of getShieldedPoolState (wallet compat)
//   getShieldedMerkleRoot      — equivalent of GET /merkle-root
//   getShieldedMerklePath      — args: [index]
//   isNullifierSpent           — args: [hex_hash]
//   checkNullifier             — args: [hex_hash] (alias of isNullifierSpent)
//   getShieldedCommitments     — args: [{from, limit}]
// ═══════════════════════════════════════════════════════════════════════════════

use ark_bn254::Fr;
use ark_ff::PrimeField;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use moltchain_core::zk::MerkleTree;
use moltchain_core::zk::{
    circuits::shield::ShieldCircuit, circuits::transfer::TransferCircuit,
    circuits::unshield::UnshieldCircuit, fr_to_bytes, poseidon_hash_fr, Prover, TREE_DEPTH,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use crate::{RpcError, RpcState};

// ─────────────────────────────────────────────────────────────────────────────
// REST Response Types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ApiResponse<T: Serialize> {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    fn ok(data: T) -> Response {
        (
            StatusCode::OK,
            Json(ApiResponse {
                success: true,
                data: Some(data),
                error: None,
            }),
        )
            .into_response()
    }
}

fn api_err(msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiResponse::<()> {
            success: false,
            data: None,
            error: Some(msg.to_string()),
        }),
    )
        .into_response()
}

fn api_not_found(msg: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ApiResponse::<()> {
            success: false,
            data: None,
            error: Some(msg.to_string()),
        }),
    )
        .into_response()
}

// ─────────────────────────────────────────────────────────────────────────────
// Data Structures
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PoolStateResponse {
    merkle_root: String,
    commitment_count: u64,
    total_shielded: u64,
    total_shielded_molt: String,
    vk_shield_hash: String,
    vk_unshield_hash: String,
    vk_transfer_hash: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MerkleRootResponse {
    merkle_root: String,
    commitment_count: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MerklePathResponse {
    index: u64,
    siblings: Vec<String>,
    path_bits: Vec<bool>,
    root: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NullifierResponse {
    nullifier: String,
    spent: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitmentsResponse {
    commitments: Vec<CommitmentEntry>,
    total: u64,
    from: u64,
    limit: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitmentEntry {
    index: u64,
    commitment: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SubmitShieldedResponse {
    signature: String,
    shielded_type: String,
}

#[derive(Deserialize)]
struct CommitmentsQuery {
    from: Option<u64>,
    limit: Option<u64>,
}

#[derive(Deserialize)]
struct SubmitBody {
    /// Base64-encoded signed transaction
    transaction: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// REST Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// GET /pool — full shielded pool state
async fn rest_get_pool_state(State(state): State<Arc<RpcState>>) -> Response {
    match state.state.get_shielded_pool_state() {
        Ok(pool) => ApiResponse::ok(PoolStateResponse {
            merkle_root: hex::encode(pool.merkle_root),
            commitment_count: pool.commitment_count,
            total_shielded: pool.total_shielded,
            total_shielded_molt: format!("{:.9}", pool.total_shielded as f64 / 1_000_000_000.0),
            vk_shield_hash: hex::encode(pool.vk_shield_hash),
            vk_unshield_hash: hex::encode(pool.vk_unshield_hash),
            vk_transfer_hash: hex::encode(pool.vk_transfer_hash),
        }),
        Err(e) => api_err(&format!("Failed to get pool state: {}", e)),
    }
}

/// GET /merkle-root — current Merkle root and leaf count
async fn rest_get_merkle_root(State(state): State<Arc<RpcState>>) -> Response {
    match state.state.get_shielded_pool_state() {
        Ok(pool) => ApiResponse::ok(MerkleRootResponse {
            merkle_root: hex::encode(pool.merkle_root),
            commitment_count: pool.commitment_count,
        }),
        Err(e) => api_err(&format!("Failed to get merkle root: {}", e)),
    }
}

/// GET /merkle-path/:index — Merkle inclusion proof for commitment at `index`
async fn rest_get_merkle_path(
    State(state): State<Arc<RpcState>>,
    Path(index): Path<u64>,
) -> Response {
    // Read pool state to know commitment count
    let pool = match state.state.get_shielded_pool_state() {
        Ok(p) => p,
        Err(e) => return api_err(&format!("Failed to get pool state: {}", e)),
    };

    if index >= pool.commitment_count {
        return api_not_found(&format!(
            "Commitment index {} out of range (pool has {} commitments)",
            index, pool.commitment_count
        ));
    }

    // Rebuild Merkle tree from all stored commitments
    let commitments = match state
        .state
        .get_all_shielded_commitments(pool.commitment_count)
    {
        Ok(c) => c,
        Err(e) => return api_err(&format!("Failed to load commitments: {}", e)),
    };

    let mut tree = MerkleTree::new();
    for comm in &commitments {
        tree.insert(*comm);
    }

    match tree.proof(index) {
        Some(path) => ApiResponse::ok(MerklePathResponse {
            index,
            siblings: path.siblings.iter().map(hex::encode).collect(),
            path_bits: path.path_bits.clone(),
            root: hex::encode(tree.root()),
        }),
        None => api_not_found(&format!(
            "Could not generate Merkle proof for index {}",
            index
        )),
    }
}

/// GET /nullifier/:hash — check whether a nullifier has been spent
async fn rest_get_nullifier(
    State(state): State<Arc<RpcState>>,
    Path(hash_hex): Path<String>,
) -> Response {
    let hash_bytes = match parse_nullifier_hex(&hash_hex) {
        Ok(b) => b,
        Err(msg) => return api_err(&msg),
    };

    match state.state.is_nullifier_spent(&hash_bytes) {
        Ok(spent) => ApiResponse::ok(NullifierResponse {
            nullifier: hash_hex,
            spent,
        }),
        Err(e) => api_err(&format!("Failed to check nullifier: {}", e)),
    }
}

/// GET /commitments?from=N&limit=M — paginated commitment list
async fn rest_get_commitments(
    State(state): State<Arc<RpcState>>,
    Query(query): Query<CommitmentsQuery>,
) -> Response {
    let pool = match state.state.get_shielded_pool_state() {
        Ok(p) => p,
        Err(e) => return api_err(&format!("Failed to get pool state: {}", e)),
    };

    let from = query.from.unwrap_or(0);
    let limit = query.limit.unwrap_or(100).min(1000);

    let end = pool.commitment_count.min(from.saturating_add(limit));
    let mut entries = Vec::with_capacity((end - from) as usize);

    for i in from..end {
        match state.state.get_shielded_commitment(i) {
            Ok(Some(comm)) => entries.push(CommitmentEntry {
                index: i,
                commitment: hex::encode(comm),
            }),
            Ok(None) => break,
            Err(e) => return api_err(&format!("Failed to read commitment {}: {}", i, e)),
        }
    }

    ApiResponse::ok(CommitmentsResponse {
        commitments: entries,
        total: pool.commitment_count,
        from,
        limit,
    })
}

/// POST /shield — submit a signed shield transaction (instruction type 23)
async fn rest_submit_shield(
    State(state): State<Arc<RpcState>>,
    Json(body): Json<SubmitBody>,
) -> Response {
    submit_shielded_tx(&state, &body.transaction, 23, "shield").await
}

/// POST /unshield — submit a signed unshield transaction (instruction type 24)
async fn rest_submit_unshield(
    State(state): State<Arc<RpcState>>,
    Json(body): Json<SubmitBody>,
) -> Response {
    submit_shielded_tx(&state, &body.transaction, 24, "unshield").await
}

/// POST /transfer — submit a signed shielded transfer (instruction type 25)
async fn rest_submit_transfer(
    State(state): State<Arc<RpcState>>,
    Json(body): Json<SubmitBody>,
) -> Response {
    submit_shielded_tx(&state, &body.transaction, 25, "transfer").await
}

/// Decode, validate, and submit a shielded transaction of the expected type.
async fn submit_shielded_tx(
    state: &RpcState,
    tx_base64: &str,
    expected_type: u8,
    type_name: &str,
) -> Response {
    use base64::{engine::general_purpose, Engine as _};

    // Decode base64
    let tx_bytes = match general_purpose::STANDARD.decode(tx_base64) {
        Ok(b) => b,
        Err(e) => return api_err(&format!("Invalid base64: {}", e)),
    };

    // Deserialize — try bincode first, then JSON (wallet may send JSON)
    let tx: moltchain_core::Transaction = match crate::bounded_bincode_deserialize(&tx_bytes) {
        Ok(t) => t,
        Err(_) => match crate::parse_json_transaction(&tx_bytes) {
            Ok(t) => t,
            Err(e) => return api_err(&format!("Invalid transaction: {}", e.message)),
        },
    };

    // Validate that the transaction contains a shielded instruction of the
    // expected type.  The first instruction must target SYSTEM_PROGRAM_ID with
    // data[0] == expected_type.
    let valid = tx.message.instructions.iter().any(|ix| {
        ix.program_id == moltchain_core::SYSTEM_PROGRAM_ID
            && ix.data.first().copied() == Some(expected_type)
    });

    if !valid {
        return api_err(&format!(
            "Transaction does not contain a shielded {} instruction (type {})",
            type_name, expected_type
        ));
    }

    // Basic signature validation
    if tx.signatures.is_empty() {
        return api_err("Transaction has no signatures");
    }
    for sig in &tx.signatures {
        if sig.iter().all(|&b| b == 0) {
            return api_err("Transaction contains an invalid zero signature");
        }
    }

    // Submit to mempool
    match crate::submit_transaction(state, tx) {
        Ok(signature) => ApiResponse::ok(SubmitShieldedResponse {
            signature,
            shielded_type: type_name.to_string(),
        }),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()> {
                success: false,
                data: None,
                error: Some(e.message),
            }),
        )
            .into_response(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// JSON-RPC Handlers (called from lib.rs dispatch)
// ─────────────────────────────────────────────────────────────────────────────

/// JSON-RPC: getShieldedPoolState
/// Params: none
pub(crate) async fn handle_get_shielded_pool_state(
    state: &RpcState,
    _params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pool = state
        .state
        .get_shielded_pool_state()
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    Ok(shielded_pool_stats_json(&pool))
}

/// JSON-RPC: getShieldedPoolStats (compat alias of getShieldedPoolState)
/// Params: none
pub(crate) async fn handle_get_shielded_pool_stats(
    state: &RpcState,
    _params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pool = state
        .state
        .get_shielded_pool_state()
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    Ok(shielded_pool_stats_json(&pool))
}

/// JSON-RPC: getShieldedMerkleRoot
/// Params: none
pub(crate) async fn handle_get_shielded_merkle_root(
    state: &RpcState,
    _params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pool = state
        .state
        .get_shielded_pool_state()
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    Ok(serde_json::json!({
        "merkleRoot": hex::encode(pool.merkle_root),
        "commitmentCount": pool.commitment_count,
    }))
}

/// JSON-RPC: getShieldedMerklePath
/// Params: [index] where index is a u64
pub(crate) async fn handle_get_shielded_merkle_path(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params: expected [index]".to_string(),
    })?;

    let index = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_u64())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [index] where index is a number".to_string(),
        })?;

    let pool = state
        .state
        .get_shielded_pool_state()
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    if index >= pool.commitment_count {
        return Err(RpcError {
            code: -32001,
            message: format!(
                "Commitment index {} out of range (pool has {} commitments)",
                index, pool.commitment_count
            ),
        });
    }

    let commitments = state
        .state
        .get_all_shielded_commitments(pool.commitment_count)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Failed to load commitments: {}", e),
        })?;

    let mut tree = MerkleTree::new();
    for comm in &commitments {
        tree.insert(*comm);
    }

    let path = tree.proof(index).ok_or_else(|| RpcError {
        code: -32001,
        message: format!("Could not generate Merkle proof for index {}", index),
    })?;

    Ok(serde_json::json!({
        "index": index,
        "siblings": path.siblings.iter().map(hex::encode).collect::<Vec<_>>(),
        "pathBits": path.path_bits,
        "root": hex::encode(tree.root()),
    }))
}

/// JSON-RPC: isNullifierSpent
/// Params: ["hex_hash"] — 64-character hex-encoded nullifier
pub(crate) async fn handle_is_nullifier_spent(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let params = params.ok_or_else(|| RpcError {
        code: -32602,
        message: "Missing params: expected [nullifier_hex]".to_string(),
    })?;

    let hash_hex = params
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: expected [nullifier_hex]".to_string(),
        })?;

    let hash_bytes = parse_nullifier_hex(hash_hex).map_err(|msg| RpcError {
        code: -32602,
        message: msg,
    })?;

    let spent = state
        .state
        .is_nullifier_spent(&hash_bytes)
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    Ok(serde_json::json!({
        "nullifier": hash_hex,
        "spent": spent,
    }))
}

/// JSON-RPC: getShieldedCommitments
/// Params: [{ "from": N, "limit": M }]  (both optional, defaults: from=0, limit=100)
pub(crate) async fn handle_get_shielded_commitments(
    state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let pool = state
        .state
        .get_shielded_pool_state()
        .map_err(|e| RpcError {
            code: -32000,
            message: format!("Database error: {}", e),
        })?;

    let (from, limit) = if let Some(ref p) = params {
        let obj = p
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_object())
            .or_else(|| p.as_object());

        let from = obj
            .and_then(|o| o.get("from"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let limit = obj
            .and_then(|o| o.get("limit"))
            .and_then(|v| v.as_u64())
            .unwrap_or(100)
            .min(1000);
        (from, limit)
    } else {
        (0u64, 100u64)
    };

    let end = pool.commitment_count.min(from.saturating_add(limit));
    let mut entries = Vec::with_capacity((end.saturating_sub(from)) as usize);

    for i in from..end {
        match state.state.get_shielded_commitment(i) {
            Ok(Some(comm)) => entries.push(serde_json::json!({
                "index": i,
                "commitment": hex::encode(comm),
            })),
            Ok(None) => break,
            Err(e) => {
                return Err(RpcError {
                    code: -32000,
                    message: format!("Failed to read commitment {}: {}", i, e),
                })
            }
        }
    }

    Ok(serde_json::json!({
        "commitments": entries,
        "total": pool.commitment_count,
        "from": from,
        "limit": limit,
    }))
}

/// JSON-RPC: computeShieldCommitment
/// Params: [{ "amount": u64, "blinding": "hex32" }]
pub(crate) async fn handle_compute_shield_commitment(
    _state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let obj = first_param_object(params.as_ref()).ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [{ amount, blinding }]".to_string(),
    })?;

    let amount = obj
        .get("amount")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: amount (u64) is required".to_string(),
        })?;

    let blinding_hex = obj
        .get("blinding")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: blinding (hex) is required".to_string(),
        })?;
    let blinding_fr = parse_hex_to_fr(blinding_hex)?;

    let commitment = fr_to_bytes(&poseidon_hash_fr(Fr::from(amount), blinding_fr));

    Ok(serde_json::json!({
        "amount": amount,
        "blinding": hex::encode(fr_to_bytes(&blinding_fr)),
        "commitment": hex::encode(commitment),
    }))
}

/// JSON-RPC: generateShieldProof
/// Params: [{ "amount": u64, "blinding": "hex32" }]
pub(crate) async fn handle_generate_shield_proof(
    _state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let obj = first_param_object(params.as_ref()).ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected [{ amount, blinding }]".to_string(),
    })?;

    let amount = obj
        .get("amount")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: amount (u64) is required".to_string(),
        })?;

    let blinding_hex = obj
        .get("blinding")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: blinding (hex) is required".to_string(),
        })?;
    let blinding_fr = parse_hex_to_fr(blinding_hex)?;

    let key_dir = resolve_zk_key_dir();
    let pk_bytes = load_zk_key_bytes("MOLTCHAIN_ZK_SHIELD_PK_PATH", &key_dir, "pk_shield.bin")?;
    let vk_bytes = load_zk_key_bytes("MOLTCHAIN_ZK_SHIELD_VK_PATH", &key_dir, "vk_shield.bin")?;

    let commitment_fr = poseidon_hash_fr(Fr::from(amount), blinding_fr);
    let commitment = fr_to_bytes(&commitment_fr);

    let mut prover = Prover::new();
    prover
        .load_shield_key(&pk_bytes)
        .map_err(internal_rpc_err)?;

    let circuit = ShieldCircuit::new(amount, amount, blinding_fr, commitment_fr);
    let mut proof = prover.prove_shield(circuit).map_err(internal_rpc_err)?;
    proof.public_inputs = vec![fr_to_bytes(&Fr::from(amount)), commitment];

    let vk =
        moltchain_core::zk::setup::load_verification_key(&vk_bytes).map_err(internal_rpc_err)?;
    let verifier = moltchain_core::zk::Verifier::from_vk_shield(vk);
    let valid = verifier
        .verify(&proof)
        .map_err(|e| internal_rpc_err(e.to_string()))?;
    if !valid {
        return Err(RpcError {
            code: -32000,
            message: "Generated shield proof failed self-verification".to_string(),
        });
    }

    Ok(serde_json::json!({
        "type": "shield",
        "amount": amount,
        "blinding": hex::encode(fr_to_bytes(&blinding_fr)),
        "commitment": hex::encode(commitment),
        "proof": hex::encode(&proof.proof_bytes),
    }))
}

/// JSON-RPC: generateUnshieldProof
/// Params: [{ amount, merkle_root, recipient, blinding, serial, spending_key, merkle_path?, path_bits? }]
pub(crate) async fn handle_generate_unshield_proof(
    _state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let obj = first_param_object(params.as_ref()).ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected unshield witness object".to_string(),
    })?;

    let amount = obj
        .get("amount")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: amount (u64) is required".to_string(),
        })?;

    let merkle_root_hex = obj
        .get("merkle_root")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: merkle_root (hex) is required".to_string(),
        })?;
    let merkle_root_bytes = parse_hex_32(merkle_root_hex)?;
    let merkle_root_fr = Fr::from_le_bytes_mod_order(&merkle_root_bytes);

    let recipient_raw = obj
        .get("recipient")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: recipient is required".to_string(),
        })?;
    let recipient_bytes = parse_recipient_32(recipient_raw)?;

    let blinding_fr = parse_hex_to_fr(obj.get("blinding").and_then(|v| v.as_str()).ok_or_else(
        || RpcError {
            code: -32602,
            message: "Invalid params: blinding (hex) is required".to_string(),
        },
    )?)?;
    let serial_fr = parse_hex_to_fr(obj.get("serial").and_then(|v| v.as_str()).ok_or_else(
        || RpcError {
            code: -32602,
            message: "Invalid params: serial (hex) is required".to_string(),
        },
    )?)?;
    let spending_key_fr = parse_hex_to_fr(
        obj.get("spending_key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: "Invalid params: spending_key (hex) is required".to_string(),
            })?,
    )?;

    let merkle_path = if let Some(path_vals) = obj.get("merkle_path").and_then(|v| v.as_array()) {
        if path_vals.len() != TREE_DEPTH {
            return Err(RpcError {
                code: -32602,
                message: format!(
                    "Invalid params: merkle_path must contain {} elements",
                    TREE_DEPTH
                ),
            });
        }
        let mut out = Vec::with_capacity(TREE_DEPTH);
        for value in path_vals {
            let hex_str = value.as_str().ok_or_else(|| RpcError {
                code: -32602,
                message: "Invalid params: merkle_path elements must be hex strings".to_string(),
            })?;
            out.push(Fr::from_le_bytes_mod_order(&parse_hex_32(hex_str)?));
        }
        out
    } else {
        vec![Fr::from(0u64); TREE_DEPTH]
    };

    let path_bits = if let Some(bits_vals) = obj.get("path_bits").and_then(|v| v.as_array()) {
        if bits_vals.len() != TREE_DEPTH {
            return Err(RpcError {
                code: -32602,
                message: format!(
                    "Invalid params: path_bits must contain {} elements",
                    TREE_DEPTH
                ),
            });
        }
        let mut out = Vec::with_capacity(TREE_DEPTH);
        for value in bits_vals {
            out.push(value.as_bool().ok_or_else(|| RpcError {
                code: -32602,
                message: "Invalid params: path_bits elements must be booleans".to_string(),
            })?);
        }
        out
    } else {
        vec![false; TREE_DEPTH]
    };

    let key_dir = resolve_zk_key_dir();
    let pk_bytes = load_zk_key_bytes("MOLTCHAIN_ZK_UNSHIELD_PK_PATH", &key_dir, "pk_unshield.bin")?;
    let vk_bytes = load_zk_key_bytes("MOLTCHAIN_ZK_UNSHIELD_VK_PATH", &key_dir, "vk_unshield.bin")?;

    let nullifier_fr = poseidon_hash_fr(serial_fr, spending_key_fr);
    let recipient_preimage = Fr::from_le_bytes_mod_order(&recipient_bytes);
    let recipient_hash_fr = poseidon_hash_fr(recipient_preimage, Fr::from(0u64));

    let circuit = UnshieldCircuit::new(
        merkle_root_fr,
        nullifier_fr,
        amount,
        recipient_hash_fr,
        amount,
        blinding_fr,
        serial_fr,
        spending_key_fr,
        recipient_preimage,
        merkle_path,
        path_bits,
    );

    let mut prover = Prover::new();
    prover
        .load_unshield_key(&pk_bytes)
        .map_err(internal_rpc_err)?;
    let mut proof = prover.prove_unshield(circuit).map_err(internal_rpc_err)?;
    proof.public_inputs = vec![
        fr_to_bytes(&merkle_root_fr),
        fr_to_bytes(&nullifier_fr),
        fr_to_bytes(&Fr::from(amount)),
        fr_to_bytes(&recipient_hash_fr),
    ];

    let vk =
        moltchain_core::zk::setup::load_verification_key(&vk_bytes).map_err(internal_rpc_err)?;
    let verifier = moltchain_core::zk::Verifier::from_vk_unshield(vk);
    let valid = verifier
        .verify(&proof)
        .map_err(|e| internal_rpc_err(e.to_string()))?;
    if !valid {
        return Err(RpcError {
            code: -32000,
            message: "Generated unshield proof failed self-verification".to_string(),
        });
    }

    Ok(serde_json::json!({
        "type": "unshield",
        "amount": amount,
        "merkle_root": hex::encode(merkle_root_bytes),
        "nullifier": hex::encode(fr_to_bytes(&nullifier_fr)),
        "recipient_hash": hex::encode(fr_to_bytes(&recipient_hash_fr)),
        "proof": hex::encode(&proof.proof_bytes),
    }))
}

/// JSON-RPC: generateTransferProof
/// Params: [{ merkle_root, inputs: [{amount, blinding, serial, spending_key, merkle_path, path_bits}, x2], outputs: [{amount, blinding}, x2] }]
pub(crate) async fn handle_generate_transfer_proof(
    _state: &RpcState,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, RpcError> {
    let obj = first_param_object(params.as_ref()).ok_or_else(|| RpcError {
        code: -32602,
        message: "Invalid params: expected transfer witness object".to_string(),
    })?;

    let merkle_root_hex = obj
        .get("merkle_root")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: merkle_root (hex) is required".to_string(),
        })?;
    let merkle_root_bytes = parse_hex_32(merkle_root_hex)?;
    let merkle_root_fr = Fr::from_le_bytes_mod_order(&merkle_root_bytes);

    let inputs = obj
        .get("inputs")
        .and_then(|v| v.as_array())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: inputs array is required".to_string(),
        })?;
    if inputs.len() != 2 {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Invalid params: transfer requires exactly 2 inputs, got {}",
                inputs.len()
            ),
        });
    }

    let outputs = obj
        .get("outputs")
        .and_then(|v| v.as_array())
        .ok_or_else(|| RpcError {
            code: -32602,
            message: "Invalid params: outputs array is required".to_string(),
        })?;
    if outputs.len() != 2 {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Invalid params: transfer requires exactly 2 outputs, got {}",
                outputs.len()
            ),
        });
    }

    let mut input_values = [0u64; 2];
    let mut input_blindings_fr = [Fr::from(0u64); 2];
    let mut input_serials_fr = [Fr::from(0u64); 2];
    let mut spending_keys_fr = [Fr::from(0u64); 2];
    let mut input_merkle_paths: [Vec<Fr>; 2] = [vec![], vec![]];
    let mut input_path_bits: [Vec<bool>; 2] = [vec![], vec![]];
    let mut nullifiers_fr = [Fr::from(0u64); 2];

    for (i, input) in inputs.iter().enumerate() {
        let input_obj = input.as_object().ok_or_else(|| RpcError {
            code: -32602,
            message: format!("Invalid params: inputs[{}] must be an object", i),
        })?;

        input_values[i] = input_obj
            .get("amount")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: format!("Invalid params: inputs[{}].amount (u64) is required", i),
            })?;
        input_blindings_fr[i] = parse_hex_to_fr(
            input_obj
                .get("blinding")
                .and_then(|v| v.as_str())
                .ok_or_else(|| RpcError {
                    code: -32602,
                    message: format!("Invalid params: inputs[{}].blinding (hex) is required", i),
                })?,
        )?;
        input_serials_fr[i] = parse_hex_to_fr(
            input_obj
                .get("serial")
                .and_then(|v| v.as_str())
                .ok_or_else(|| RpcError {
                    code: -32602,
                    message: format!("Invalid params: inputs[{}].serial (hex) is required", i),
                })?,
        )?;
        spending_keys_fr[i] = parse_hex_to_fr(
            input_obj
                .get("spending_key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| RpcError {
                    code: -32602,
                    message: format!(
                        "Invalid params: inputs[{}].spending_key (hex) is required",
                        i
                    ),
                })?,
        )?;

        let merkle_path_vals = input_obj
            .get("merkle_path")
            .and_then(|v| v.as_array())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: format!(
                    "Invalid params: inputs[{}].merkle_path array is required",
                    i
                ),
            })?;
        if merkle_path_vals.len() != TREE_DEPTH {
            return Err(RpcError {
                code: -32602,
                message: format!(
                    "Invalid params: inputs[{}].merkle_path must contain {} elements",
                    i, TREE_DEPTH
                ),
            });
        }
        input_merkle_paths[i] = merkle_path_vals
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| RpcError {
                        code: -32602,
                        message: format!(
                            "Invalid params: inputs[{}].merkle_path elements must be hex strings",
                            i
                        ),
                    })
                    .and_then(|hex_str| {
                        parse_hex_32(hex_str).map(|b| Fr::from_le_bytes_mod_order(&b))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let path_bits_vals = input_obj
            .get("path_bits")
            .and_then(|v| v.as_array())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: format!("Invalid params: inputs[{}].path_bits array is required", i),
            })?;
        if path_bits_vals.len() != TREE_DEPTH {
            return Err(RpcError {
                code: -32602,
                message: format!(
                    "Invalid params: inputs[{}].path_bits must contain {} elements",
                    i, TREE_DEPTH
                ),
            });
        }
        input_path_bits[i] = path_bits_vals
            .iter()
            .map(|value| {
                value.as_bool().ok_or_else(|| RpcError {
                    code: -32602,
                    message: format!(
                        "Invalid params: inputs[{}].path_bits elements must be booleans",
                        i
                    ),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        nullifiers_fr[i] = poseidon_hash_fr(input_serials_fr[i], spending_keys_fr[i]);
    }

    let mut output_values = [0u64; 2];
    let mut output_blindings_fr = [Fr::from(0u64); 2];
    let mut output_commitments_fr = [Fr::from(0u64); 2];
    let mut output_commitments_bytes = [[0u8; 32]; 2];

    for (i, output) in outputs.iter().enumerate() {
        let output_obj = output.as_object().ok_or_else(|| RpcError {
            code: -32602,
            message: format!("Invalid params: outputs[{}] must be an object", i),
        })?;

        output_values[i] = output_obj
            .get("amount")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| RpcError {
                code: -32602,
                message: format!("Invalid params: outputs[{}].amount (u64) is required", i),
            })?;
        output_blindings_fr[i] = parse_hex_to_fr(
            output_obj
                .get("blinding")
                .and_then(|v| v.as_str())
                .ok_or_else(|| RpcError {
                    code: -32602,
                    message: format!("Invalid params: outputs[{}].blinding (hex) is required", i),
                })?,
        )?;

        output_commitments_fr[i] =
            poseidon_hash_fr(Fr::from(output_values[i]), output_blindings_fr[i]);
        output_commitments_bytes[i] = fr_to_bytes(&output_commitments_fr[i]);
    }

    let total_in: u64 = input_values.iter().sum();
    let total_out: u64 = output_values.iter().sum();
    if total_in != total_out {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Invalid params: value not conserved (sum(inputs)={} != sum(outputs)={})",
                total_in, total_out
            ),
        });
    }

    let circuit = TransferCircuit::new(
        merkle_root_fr,
        nullifiers_fr,
        output_commitments_fr,
        input_values,
        input_blindings_fr,
        input_serials_fr,
        spending_keys_fr,
        input_merkle_paths,
        input_path_bits,
        output_values,
        output_blindings_fr,
    );

    let key_dir = resolve_zk_key_dir();
    let pk_bytes = load_zk_key_bytes("MOLTCHAIN_ZK_TRANSFER_PK_PATH", &key_dir, "pk_transfer.bin")?;
    let vk_bytes = load_zk_key_bytes("MOLTCHAIN_ZK_TRANSFER_VK_PATH", &key_dir, "vk_transfer.bin")?;

    let mut prover = Prover::new();
    prover
        .load_transfer_key(&pk_bytes)
        .map_err(internal_rpc_err)?;
    let mut proof = prover.prove_transfer(circuit).map_err(internal_rpc_err)?;

    proof.public_inputs = vec![
        fr_to_bytes(&merkle_root_fr),
        fr_to_bytes(&nullifiers_fr[0]),
        fr_to_bytes(&nullifiers_fr[1]),
        output_commitments_bytes[0],
        output_commitments_bytes[1],
    ];

    let vk =
        moltchain_core::zk::setup::load_verification_key(&vk_bytes).map_err(internal_rpc_err)?;
    let verifier = moltchain_core::zk::Verifier::from_vk_transfer(vk);
    let valid = verifier
        .verify(&proof)
        .map_err(|e| internal_rpc_err(e.to_string()))?;
    if !valid {
        return Err(RpcError {
            code: -32000,
            message: "Generated transfer proof failed self-verification".to_string(),
        });
    }

    Ok(serde_json::json!({
        "type": "transfer",
        "merkle_root": hex::encode(merkle_root_bytes),
        "nullifier_a": hex::encode(fr_to_bytes(&nullifiers_fr[0])),
        "nullifier_b": hex::encode(fr_to_bytes(&nullifiers_fr[1])),
        "commitment_c": hex::encode(output_commitments_bytes[0]),
        "commitment_d": hex::encode(output_commitments_bytes[1]),
        "proof": hex::encode(&proof.proof_bytes),
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Parse a 64-character hex string into a 32-byte array.
fn parse_nullifier_hex(hex_str: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(hex_str).map_err(|e| format!("Invalid hex: {}", e))?;
    if bytes.len() != 32 {
        return Err(format!(
            "Invalid nullifier length: expected 32 bytes (64 hex chars), got {} bytes",
            bytes.len()
        ));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

fn internal_rpc_err(message: String) -> RpcError {
    RpcError {
        code: -32000,
        message,
    }
}

fn first_param_object(
    params: Option<&serde_json::Value>,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    params.and_then(|p| {
        p.as_array()
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_object())
            .or_else(|| p.as_object())
    })
}

fn resolve_zk_key_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".moltchain")
        .join("zk")
}

fn load_zk_key_bytes(
    env_var: &str,
    key_dir: &std::path::Path,
    default_file: &str,
) -> Result<Vec<u8>, RpcError> {
    let path = std::env::var(env_var)
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| key_dir.join(default_file));
    fs::read(&path).map_err(|e| RpcError {
        code: -32000,
        message: format!(
            "Failed to load ZK key {} ({}): {}. Run zk-setup if keys are missing.",
            default_file,
            path.display(),
            e
        ),
    })
}

fn parse_hex_to_fr(hex_str: &str) -> Result<Fr, RpcError> {
    let bytes = hex::decode(hex_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid hex: {}", e),
    })?;
    Ok(Fr::from_le_bytes_mod_order(&bytes))
}

fn parse_hex_32(hex_str: &str) -> Result<[u8; 32], RpcError> {
    let bytes = hex::decode(hex_str).map_err(|e| RpcError {
        code: -32602,
        message: format!("Invalid hex: {}", e),
    })?;
    if bytes.len() != 32 {
        return Err(RpcError {
            code: -32602,
            message: format!("Expected 32-byte hex value, got {} bytes", bytes.len()),
        });
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

fn parse_recipient_32(input: &str) -> Result<[u8; 32], RpcError> {
    if let Ok(hex_bytes) = hex::decode(input) {
        if hex_bytes.len() == 32 {
            let mut out = [0u8; 32];
            out.copy_from_slice(&hex_bytes);
            return Ok(out);
        }
    }

    let decoded = bs58::decode(input).into_vec().map_err(|e| RpcError {
        code: -32602,
        message: format!(
            "Invalid recipient encoding (expected base58 pubkey or 32-byte hex): {}",
            e
        ),
    })?;
    if decoded.len() != 32 {
        return Err(RpcError {
            code: -32602,
            message: format!(
                "Invalid recipient length: expected 32 bytes, got {}",
                decoded.len()
            ),
        });
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&decoded);
    Ok(out)
}

fn shielded_pool_stats_json(pool: &moltchain_core::zk::ShieldedPoolState) -> serde_json::Value {
    let merkle_root = hex::encode(pool.merkle_root);
    let total_shielded_molt = format!("{:.9}", pool.total_shielded as f64 / 1_000_000_000.0);
    let vk_shield_hash = hex::encode(pool.vk_shield_hash);
    let vk_unshield_hash = hex::encode(pool.vk_unshield_hash);
    let vk_transfer_hash = hex::encode(pool.vk_transfer_hash);

    serde_json::json!({
        // camelCase (current canonical)
        "merkleRoot": merkle_root,
        "commitmentCount": pool.commitment_count,
        "totalShielded": pool.total_shielded,
        "totalShieldedMolt": total_shielded_molt,
        "nullifierCount": pool.nullifier_count,
        "shieldCount": pool.shield_count,
        "unshieldCount": pool.unshield_count,
        "transferCount": pool.transfer_count,
        "vkShieldHash": vk_shield_hash,
        "vkUnshieldHash": vk_unshield_hash,
        "vkTransferHash": vk_transfer_hash,

        // snake_case compatibility for wallet/extension callers
        "merkle_root": hex::encode(pool.merkle_root),
        "commitment_count": pool.commitment_count,
        "pool_balance": pool.total_shielded,
        "pool_balance_molt": pool.total_shielded as f64 / 1_000_000_000.0,
        "total_shielded": pool.total_shielded,
        "total_shielded_molt": format!("{:.9}", pool.total_shielded as f64 / 1_000_000_000.0),
        "nullifier_count": pool.nullifier_count,
        "shield_count": pool.shield_count,
        "unshield_count": pool.unshield_count,
        "transfer_count": pool.transfer_count,
        "vk_shield_hash": hex::encode(pool.vk_shield_hash),
        "vk_unshield_hash": hex::encode(pool.vk_unshield_hash),
        "vk_transfer_hash": hex::encode(pool.vk_transfer_hash),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Router
// ─────────────────────────────────────────────────────────────────────────────

/// Build the shielded pool REST API router.
///
/// Nested under `/api/v1/shielded` in lib.rs.
pub(crate) fn build_shielded_router() -> Router<Arc<RpcState>> {
    Router::new()
        .route("/pool", get(rest_get_pool_state))
        .route("/merkle-root", get(rest_get_merkle_root))
        .route("/merkle-path/:index", get(rest_get_merkle_path))
        .route("/nullifier/:hash", get(rest_get_nullifier))
        .route("/commitments", get(rest_get_commitments))
        .route("/shield", post(rest_submit_shield))
        .route("/unshield", post(rest_submit_unshield))
        .route("/transfer", post(rest_submit_transfer))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_nullifier_hex_valid() {
        let hex_str = "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8e9f0a1b2";
        let result = parse_nullifier_hex(hex_str);
        assert!(result.is_ok());
        let arr = result.unwrap();
        assert_eq!(arr[0], 0xa1);
        assert_eq!(arr[31], 0xb2);
    }

    #[test]
    fn test_parse_nullifier_hex_invalid_length() {
        let result = parse_nullifier_hex("abcdef");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected 32 bytes"));
    }

    #[test]
    fn test_parse_nullifier_hex_invalid_chars() {
        let result = parse_nullifier_hex("zzzz");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid hex"));
    }

    #[test]
    fn test_parse_nullifier_hex_empty() {
        let result = parse_nullifier_hex("");
        assert!(result.is_err());
    }

    #[test]
    fn test_api_response_serialization() {
        let resp = PoolStateResponse {
            merkle_root: "abc123".to_string(),
            commitment_count: 42,
            total_shielded: 1_000_000_000,
            total_shielded_molt: "1.000000000".to_string(),
            vk_shield_hash: "def456".to_string(),
            vk_unshield_hash: "789abc".to_string(),
            vk_transfer_hash: "0ed123".to_string(),
        };
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["commitmentCount"], 42);
        assert_eq!(json["totalShielded"], 1_000_000_000u64);
        assert_eq!(json["merkleRoot"], "abc123");
    }

    #[test]
    fn test_commitments_query_defaults() {
        let q: CommitmentsQuery = serde_json::from_str("{}").unwrap();
        assert_eq!(q.from, None);
        assert_eq!(q.limit, None);
    }

    #[test]
    fn test_commitments_query_with_values() {
        let q: CommitmentsQuery = serde_json::from_str(r#"{"from": 10, "limit": 50}"#).unwrap();
        assert_eq!(q.from, Some(10));
        assert_eq!(q.limit, Some(50));
    }

    #[test]
    fn test_submit_body_deserialization() {
        let body: SubmitBody = serde_json::from_str(r#"{"transaction": "dGVzdA=="}"#).unwrap();
        assert_eq!(body.transaction, "dGVzdA==");
    }
}
