// RPC handler integration tests
// Tests for core JSON-RPC endpoints

use axum::body::{to_bytes, Body};
use axum::http::Request;
use lichen_core::{
    contract::ContractAccount, Account, Block, Hash, Pubkey, StateStore, SymbolRegistryEntry,
    CONTRACT_PROGRAM_ID,
};
use lichen_rpc::build_rpc_router;
use serde_json::json;
use tower::util::ServiceExt;

type RpcResult = Result<serde_json::Value, String>;

fn current_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn put_ready_tip(state: &StateStore, slot: u64) {
    assert!(slot > 0, "ready RPC fixtures must have a non-genesis tip");
    let block = Block::new_with_timestamp(
        slot,
        Hash::default(),
        state.compute_state_root(),
        [0u8; 32],
        Vec::new(),
        current_unix_secs(),
    );
    state
        .put_block_atomic(&block, Some(slot), Some(slot))
        .expect("put ready tip");
}

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
    put_ready_tip(&state, 1);
    // Leak the tempdir so it isn't deleted while the app exists
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

fn skill_hash(name: &str) -> [u8; 16] {
    const FNV_OFFSET_BASIS: u128 = 0x6c62272e07bb0142_62b821756295c58d;
    const FNV_PRIME: u128 = 0x0000000001000000_000000000000013B;
    let mut hash = FNV_OFFSET_BASIS;
    for byte in name.as_bytes() {
        hash ^= *byte as u128;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash.to_le_bytes()
}

fn create_test_app_with_lichenid() -> (axum::Router, String, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    let _ = Box::leak(Box::new(dir));

    let mut contract = ContractAccount::new(vec![1, 2, 3], Pubkey([2u8; 32]));
    let lichenid_program = Pubkey([7u8; 32]);
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

    let mut given_vouch = Vec::with_capacity(40);
    given_vouch.extend_from_slice(&bob.0);
    given_vouch.extend_from_slice(&1_700_200_000u64.to_le_bytes());
    contract.storage.insert(
        format!("vouch_given:{}:0", alice_hex).into_bytes(),
        given_vouch,
    );
    contract.storage.insert(
        format!("vouch_given_count:{}", alice_hex).into_bytes(),
        1u64.to_le_bytes().to_vec(),
    );

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
        b"https://alice-agent.licn/api".to_vec(),
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
        .insert(b"licn_name_count".to_vec(), 1u64.to_le_bytes().to_vec());

    let mut account = Account::new(0, CONTRACT_PROGRAM_ID);
    account.owner = CONTRACT_PROGRAM_ID;
    account.executable = true;
    account.data = serde_json::to_vec(&contract).expect("serialize contract");
    state
        .put_account(&lichenid_program, &account)
        .expect("put program account");

    state
        .register_symbol(
            "YID",
            SymbolRegistryEntry {
                symbol: "YID".to_string(),
                program: lichenid_program,
                owner: Pubkey([2u8; 32]),
                name: Some("LichenID Identity".to_string()),
                template: Some("identity".to_string()),
                metadata: None,
                decimals: None,
            },
        )
        .expect("register symbol");

    // Mirror all storage entries to CF_CONTRACT_STORAGE so CF-based stats
    // handlers return the same values as the embedded storage.
    for (key, value) in &contract.storage {
        state
            .put_contract_storage(&lichenid_program, key, value)
            .expect("put CF storage");
    }
    put_ready_tip(&state, 1);

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

    (app, alice.to_base58(), bob.to_base58())
}

// ============================================================================
// Health endpoint
// ============================================================================

#[tokio::test]
async fn test_health_endpoint() {
    let app = create_test_app();
    let response = rpc_call(&app, "/solana-compat", "getHealth").await.unwrap();
    let result = &response["result"];
    assert!(result.is_object(), "getHealth should return an object");
    assert!(
        result["status"] == "ok" || result["status"] == "behind",
        "getHealth status should be 'ok' or 'behind'"
    );
}

// ============================================================================
// getVersion
// ============================================================================

#[tokio::test]
async fn test_get_version() {
    let app = create_test_app();
    let response = rpc_call(&app, "/solana-compat", "getVersion")
        .await
        .unwrap();
    // Should contain a "solana-core" or similar version field
    let result = &response["result"];
    assert!(result.is_object(), "getVersion should return an object");
}

#[tokio::test]
async fn test_get_contract_info_includes_registry_profile_metadata() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state = StateStore::open(dir.path()).expect("state");
    let _ = Box::leak(Box::new(dir));

    let program = Pubkey([44u8; 32]);
    let mut contract = ContractAccount::new(vec![1, 2, 3], Pubkey([2u8; 32]));
    contract.storage.insert(
        b"dv581100_supply".to_vec(),
        123_456_789u64.to_le_bytes().to_vec(),
    );

    let mut account = Account::new(0, CONTRACT_PROGRAM_ID);
    account.owner = CONTRACT_PROGRAM_ID;
    account.executable = true;
    account.data = serde_json::to_vec(&contract).expect("serialize contract");
    state.put_account(&program, &account).expect("put contract");

    state
        .register_symbol(
            "DV581100",
            SymbolRegistryEntry {
                symbol: "DV581100".to_string(),
                program,
                owner: Pubkey([2u8; 32]),
                name: Some("DevLifecycle581100".to_string()),
                template: Some("mt20".to_string()),
                metadata: Some(json!({
                    "description": "Developer lifecycle token",
                    "website": "https://dev.example/token",
                    "logo_url": "https://dev.example/token.png",
                    "twitter": "https://x.com/devtoken",
                    "telegram": "https://t.me/devtoken",
                    "discord": "https://discord.gg/devtoken",
                    "decimals": 9
                })),
                decimals: Some(9),
            },
        )
        .expect("register symbol");
    put_ready_tip(&state, 1);

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

    let response = rpc_call_with_params(&app, "/", "getContractInfo", json!([program.to_base58()]))
        .await
        .unwrap();

    let meta = &response["result"]["token_metadata"];
    assert_eq!(meta["token_symbol"], json!("DV581100"));
    assert_eq!(meta["token_name"], json!("DevLifecycle581100"));
    assert_eq!(meta["total_supply"], json!(123_456_789u64));
    assert_eq!(meta["decimals"], json!(9));
    assert_eq!(meta["description"], json!("Developer lifecycle token"));
    assert_eq!(meta["website"], json!("https://dev.example/token"));
    assert_eq!(meta["logo_url"], json!("https://dev.example/token.png"));
    assert_eq!(meta["twitter"], json!("https://x.com/devtoken"));
    assert_eq!(meta["telegram"], json!("https://t.me/devtoken"));
    assert_eq!(meta["discord"], json!("https://discord.gg/devtoken"));
}

// ============================================================================
// getSlot
// ============================================================================

#[tokio::test]
async fn test_get_slot() {
    let app = create_test_app();
    let response = rpc_call(&app, "/solana-compat", "getSlot").await.unwrap();
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
        "/solana-compat",
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
    let response = rpc_call_with_params(&app, "/solana-compat", "getBlock", json!([0]))
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
    let response = rpc_call(&app, "/solana-compat", "getHealth").await.unwrap();

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
    let response = rpc_call(&app, "/solana-compat", "nonExistentMethod")
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
async fn test_get_lichenid_identity_existing() {
    let (app, alice, _) = create_test_app_with_lichenid();
    let response = rpc_call_with_params(&app, "/", "getLichenIdIdentity", json!([alice]))
        .await
        .unwrap();
    assert_eq!(response["result"]["agent_type_name"], "Trading");
    assert_eq!(response["result"]["trust_tier_name"], "Trusted");
    assert_eq!(response["result"]["licn_name"], "alice.lichen");
}

#[tokio::test]
async fn test_get_lichenid_identity_nonexistent() {
    let (app, _, _) = create_test_app_with_lichenid();
    let missing = Pubkey([250u8; 32]).to_base58();
    let response = rpc_call_with_params(&app, "/", "getLichenIdIdentity", json!([missing]))
        .await
        .unwrap();
    assert!(response["result"].is_null());
}

#[tokio::test]
async fn test_get_lichenid_reputation() {
    let (app, alice, _) = create_test_app_with_lichenid();
    let response = rpc_call_with_params(&app, "/", "getLichenIdReputation", json!([alice]))
        .await
        .unwrap();
    assert_eq!(response["result"]["score"], 742);
    assert_eq!(response["result"]["tier_name"], "Trusted");
}

#[tokio::test]
async fn test_get_lichenid_skills_with_attestations() {
    let (app, alice, _) = create_test_app_with_lichenid();
    let response = rpc_call_with_params(&app, "/", "getLichenIdSkills", json!([alice]))
        .await
        .unwrap();
    assert_eq!(response["result"][0]["name"], "rust");
    assert_eq!(response["result"][0]["attestation_count"], 3);
}

#[tokio::test]
async fn test_get_lichenid_vouches_bidirectional() {
    let (app, alice, bob) = create_test_app_with_lichenid();

    let bob_vouches = rpc_call_with_params(&app, "/", "getLichenIdVouches", json!([bob]))
        .await
        .unwrap();
    assert_eq!(bob_vouches["result"]["received"][0]["voucher"], alice);

    let alice_vouches = rpc_call_with_params(&app, "/", "getLichenIdVouches", json!([alice]))
        .await
        .unwrap();
    assert_eq!(alice_vouches["result"]["given"][0]["vouchee"], bob);
}

#[tokio::test]
async fn test_get_lichenid_achievements() {
    let (app, alice, _) = create_test_app_with_lichenid();
    let response = rpc_call_with_params(&app, "/", "getLichenIdAchievements", json!([alice]))
        .await
        .unwrap();
    assert_eq!(response["result"][0]["id"], 4);
    assert_eq!(response["result"][0]["name"], "Trusted Agent");
}

#[tokio::test]
async fn test_licn_name_resolution_endpoints() {
    let (app, alice, _) = create_test_app_with_lichenid();

    let resolve = rpc_call_with_params(&app, "/", "resolveLichenName", json!(["alice.lichen"]))
        .await
        .unwrap();
    assert_eq!(resolve["result"]["owner"], alice);

    let reverse = rpc_call_with_params(&app, "/", "reverseLichenName", json!([alice.clone()]))
        .await
        .unwrap();
    assert_eq!(reverse["result"]["name"], "alice.lichen");

    let batch = rpc_call_with_params(
        &app,
        "/",
        "batchReverseLichenNames",
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
async fn test_get_lichenid_profile_and_directory() {
    let (app, alice, _) = create_test_app_with_lichenid();

    let profile = rpc_call_with_params(&app, "/", "getLichenIdProfile", json!([alice]))
        .await
        .unwrap();
    assert_eq!(profile["result"]["agent"]["availability_name"], "available");
    assert_eq!(profile["result"]["agent"]["rate"], 500_000_000u64);

    let directory = rpc_call_with_params(
        &app,
        "/",
        "getLichenIdAgentDirectory",
        json!([{ "available": true, "limit": 10 }]),
    )
    .await
    .unwrap();
    assert!(directory["result"]["count"].as_u64().unwrap_or(0) >= 1);
}

#[tokio::test]
async fn test_get_lichenid_stats() {
    let (app, _, _) = create_test_app_with_lichenid();
    let response = rpc_call(&app, "/", "getLichenIdStats").await.unwrap();
    assert_eq!(response["result"]["total_identities"], 2);
    assert_eq!(response["result"]["total_names"], 1);
}

#[tokio::test]
async fn test_lichenid_invalid_pubkey_rejected() {
    let (app, _, _) = create_test_app_with_lichenid();
    let response = rpc_call_with_params(&app, "/", "getLichenIdIdentity", json!(["not-a-pubkey"]))
        .await
        .unwrap();
    assert!(response.get("error").is_some());
}
