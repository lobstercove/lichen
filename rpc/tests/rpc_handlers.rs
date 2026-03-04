// RPC handler integration tests
// Tests for core JSON-RPC endpoints

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
        None,
        None,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn make_identity_record(
    owner: Pubkey,
    agent_type: u8,
    name: &str,
    reputation: u64,
    created_at: u64,
    updated_at: u64,
    skill_count: u8,
    vouch_count: u16,
    is_active: bool,
) -> Vec<u8> {
    let mut record = vec![0u8; 127];
    record[0..32].copy_from_slice(&owner.0);
    record[32] = agent_type;
    let name_bytes = name.as_bytes();
    record[33] = (name_bytes.len() & 0xFF) as u8;
    record[34] = ((name_bytes.len() >> 8) & 0xFF) as u8;
    record[35..35 + name_bytes.len()].copy_from_slice(name_bytes);
    record[99..107].copy_from_slice(&reputation.to_le_bytes());
    record[107..115].copy_from_slice(&created_at.to_le_bytes());
    record[115..123].copy_from_slice(&updated_at.to_le_bytes());
    record[123] = skill_count;
    record[124] = (vouch_count & 0xFF) as u8;
    record[125] = ((vouch_count >> 8) & 0xFF) as u8;
    record[126] = if is_active { 1 } else { 0 };
    record
}

fn make_skill_record(name: &str, proficiency: u8, timestamp: u64) -> Vec<u8> {
    let name_bytes = name.as_bytes();
    let mut data = Vec::with_capacity(1 + name_bytes.len() + 1 + 8);
    data.push(name_bytes.len() as u8);
    data.extend_from_slice(name_bytes);
    data.push(proficiency);
    data.extend_from_slice(&timestamp.to_le_bytes());
    data
}

fn skill_hash(name: &str) -> [u8; 8] {
    let mut out = [0u8; 8];
    for (i, b) in name.as_bytes().iter().enumerate() {
        if i >= 8 {
            break;
        }
        out[i] = *b;
    }
    out
}

fn create_test_app_with_moltyid() -> (axum::Router, String, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    let _ = Box::leak(Box::new(dir));

    let mut contract = ContractAccount::new(vec![1, 2, 3], Pubkey([2u8; 32]));
    let moltyid_program = Pubkey([7u8; 32]);
    let alice = Pubkey([10u8; 32]);
    let bob = Pubkey([11u8; 32]);
    let alice_hex = hex::encode(alice.0);
    let bob_hex = hex::encode(bob.0);

    contract.storage.insert(
        format!("id:{}", alice_hex).into_bytes(),
        make_identity_record(
            alice,
            1,
            "Alice",
            742,
            1_700_000_000,
            1_700_100_000,
            1,
            1,
            true,
        ),
    );
    contract.storage.insert(
        format!("id:{}", bob_hex).into_bytes(),
        make_identity_record(
            bob,
            7,
            "Bob",
            1200,
            1_700_000_100,
            1_700_200_000,
            0,
            1,
            true,
        ),
    );

    contract.storage.insert(
        format!("rep:{}", alice_hex).into_bytes(),
        742u64.to_le_bytes().to_vec(),
    );
    contract.storage.insert(
        format!("rep:{}", bob_hex).into_bytes(),
        1200u64.to_le_bytes().to_vec(),
    );

    contract.storage.insert(
        format!("skill:{}:0", alice_hex).into_bytes(),
        make_skill_record("rust", 95, 1_700_100_100),
    );
    contract.storage.insert(
        format!(
            "attest_count_{}_{}",
            alice_hex,
            hex::encode(skill_hash("rust"))
        )
        .into_bytes(),
        3u64.to_le_bytes().to_vec(),
    );

    let mut vouch = Vec::with_capacity(40);
    vouch.extend_from_slice(&alice.0);
    vouch.extend_from_slice(&1_700_200_000u64.to_le_bytes());
    contract
        .storage
        .insert(format!("vouch:{}:0", bob_hex).into_bytes(), vouch);

    contract.storage.insert(
        format!("name_rev:{}", alice_hex).into_bytes(),
        b"alice".to_vec(),
    );
    let mut name_record = vec![0u8; 48];
    name_record[0..32].copy_from_slice(&alice.0);
    name_record[32..40].copy_from_slice(&100u64.to_le_bytes());
    name_record[40..48].copy_from_slice(&9_999_999_999u64.to_le_bytes());
    contract.storage.insert(b"name:alice".to_vec(), name_record);

    contract.storage.insert(
        format!("endpoint:{}", alice_hex).into_bytes(),
        b"https://alice-agent.molt/api".to_vec(),
    );
    contract.storage.insert(
        format!("metadata:{}", alice_hex).into_bytes(),
        br#"{"model":"gpt"}"#.to_vec(),
    );
    contract
        .storage
        .insert(format!("availability:{}", alice_hex).into_bytes(), vec![1]);
    contract.storage.insert(
        format!("rate:{}", alice_hex).into_bytes(),
        500_000_000u64.to_le_bytes().to_vec(),
    );

    contract
        .storage
        .insert(format!("ach:{}:04", alice_hex).into_bytes(), {
            let mut data = vec![4u8];
            data.extend_from_slice(&1_700_200_200u64.to_le_bytes());
            data
        });

    contract
        .storage
        .insert(b"mid_identity_count".to_vec(), 2u64.to_le_bytes().to_vec());
    contract
        .storage
        .insert(b"molt_name_count".to_vec(), 1u64.to_le_bytes().to_vec());

    let mut account = Account::new(0, CONTRACT_PROGRAM_ID);
    account.owner = CONTRACT_PROGRAM_ID;
    account.executable = true;
    account.data = serde_json::to_vec(&contract).expect("serialize contract");
    state
        .put_account(&moltyid_program, &account)
        .expect("put program account");

    state
        .register_symbol(
            "YID",
            SymbolRegistryEntry {
                symbol: "YID".to_string(),
                program: moltyid_program,
                owner: Pubkey([2u8; 32]),
                name: Some("MoltyID Identity".to_string()),
                template: Some("identity".to_string()),
                metadata: None,
            },
        )
        .expect("register symbol");

    // Mirror all storage entries to CF_CONTRACT_STORAGE so CF-based stats
    // handlers return the same values as the embedded storage.
    for (key, value) in &contract.storage {
        state
            .put_contract_storage(&moltyid_program, key, value)
            .expect("put CF storage");
    }

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

    (app, alice.to_base58(), bob.to_base58())
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

#[tokio::test]
async fn test_get_moltyid_identity_existing() {
    let (app, alice, _) = create_test_app_with_moltyid();
    let response = rpc_call_with_params(&app, "/", "getMoltyIdIdentity", json!([alice]))
        .await
        .unwrap();
    assert_eq!(response["result"]["agent_type_name"], "Trading");
    assert_eq!(response["result"]["trust_tier_name"], "Trusted");
    assert_eq!(response["result"]["molt_name"], "alice.molt");
}

#[tokio::test]
async fn test_get_moltyid_identity_nonexistent() {
    let (app, _, _) = create_test_app_with_moltyid();
    let missing = Pubkey([250u8; 32]).to_base58();
    let response = rpc_call_with_params(&app, "/", "getMoltyIdIdentity", json!([missing]))
        .await
        .unwrap();
    assert!(response["result"].is_null());
}

#[tokio::test]
async fn test_get_moltyid_reputation() {
    let (app, alice, _) = create_test_app_with_moltyid();
    let response = rpc_call_with_params(&app, "/", "getMoltyIdReputation", json!([alice]))
        .await
        .unwrap();
    assert_eq!(response["result"]["score"], 742);
    assert_eq!(response["result"]["tier_name"], "Trusted");
}

#[tokio::test]
async fn test_get_moltyid_skills_with_attestations() {
    let (app, alice, _) = create_test_app_with_moltyid();
    let response = rpc_call_with_params(&app, "/", "getMoltyIdSkills", json!([alice]))
        .await
        .unwrap();
    assert_eq!(response["result"][0]["name"], "rust");
    assert_eq!(response["result"][0]["attestation_count"], 3);
}

#[tokio::test]
async fn test_get_moltyid_vouches_bidirectional() {
    let (app, alice, bob) = create_test_app_with_moltyid();

    let bob_vouches = rpc_call_with_params(&app, "/", "getMoltyIdVouches", json!([bob]))
        .await
        .unwrap();
    assert_eq!(bob_vouches["result"]["received"][0]["voucher"], alice);

    let alice_vouches = rpc_call_with_params(&app, "/", "getMoltyIdVouches", json!([alice]))
        .await
        .unwrap();
    assert_eq!(alice_vouches["result"]["given"][0]["vouchee"], bob);
}

#[tokio::test]
async fn test_get_moltyid_achievements() {
    let (app, alice, _) = create_test_app_with_moltyid();
    let response = rpc_call_with_params(&app, "/", "getMoltyIdAchievements", json!([alice]))
        .await
        .unwrap();
    assert_eq!(response["result"][0]["id"], 4);
    assert_eq!(response["result"][0]["name"], "Trusted Agent");
}

#[tokio::test]
async fn test_molt_name_resolution_endpoints() {
    let (app, alice, _) = create_test_app_with_moltyid();

    let resolve = rpc_call_with_params(&app, "/", "resolveMoltName", json!(["alice.molt"]))
        .await
        .unwrap();
    assert_eq!(resolve["result"]["owner"], alice);

    let reverse = rpc_call_with_params(&app, "/", "reverseMoltName", json!([alice.clone()]))
        .await
        .unwrap();
    assert_eq!(reverse["result"]["name"], "alice.molt");

    let batch = rpc_call_with_params(
        &app,
        "/",
        "batchReverseMoltNames",
        json!([alice, "11111111111111111111111111111111"]),
    )
    .await
    .unwrap();
    assert_eq!(
        batch["result"]["11111111111111111111111111111111"],
        serde_json::Value::Null
    );
}

#[tokio::test]
async fn test_get_moltyid_profile_and_directory() {
    let (app, alice, _) = create_test_app_with_moltyid();

    let profile = rpc_call_with_params(&app, "/", "getMoltyIdProfile", json!([alice]))
        .await
        .unwrap();
    assert_eq!(profile["result"]["agent"]["availability_name"], "available");
    assert_eq!(profile["result"]["agent"]["rate"], 500_000_000u64);

    let directory = rpc_call_with_params(
        &app,
        "/",
        "getMoltyIdAgentDirectory",
        json!([{ "available": true, "limit": 10 }]),
    )
    .await
    .unwrap();
    assert!(directory["result"]["count"].as_u64().unwrap_or(0) >= 1);
}

#[tokio::test]
async fn test_get_moltyid_stats() {
    let (app, _, _) = create_test_app_with_moltyid();
    let response = rpc_call(&app, "/", "getMoltyIdStats").await.unwrap();
    assert_eq!(response["result"]["total_identities"], 2);
    assert_eq!(response["result"]["total_names"], 1);
}

#[tokio::test]
async fn test_moltyid_invalid_pubkey_rejected() {
    let (app, _, _) = create_test_app_with_moltyid();
    let response = rpc_call_with_params(&app, "/", "getMoltyIdIdentity", json!(["not-a-pubkey"]))
        .await
        .unwrap();
    assert!(response.get("error").is_some());
}
