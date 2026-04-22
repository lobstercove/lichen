use anyhow::Result;
use lichen_core::{Keypair, Pubkey};
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::contract_wasm_support::{derive_contract_address, load_wasm_code, WasmValidationConfig};
use crate::keypair_manager::KeypairManager;
use crate::token_amount_support::build_token_registry_metadata;
use crate::token_create_support::TokenCreateRequest;

const TOKEN_CREATE_WASM_VALIDATION: WasmValidationConfig<'static> = WasmValidationConfig {
    read_failure_subject: "WASM",
    invalid_magic_help:
        ".\n             Compile with: cargo build --target wasm32-unknown-unknown --release",
    oversize_help: "",
};

pub(super) struct PreparedTokenCreate {
    pub(super) name: String,
    pub(super) symbol: String,
    pub(super) wasm: PathBuf,
    pub(super) decimals: u8,
    pub(super) initial_supply: Option<u64>,
    pub(super) deployer: Keypair,
    pub(super) registry_metadata: Option<serde_json::Value>,
    pub(super) wasm_code: Vec<u8>,
    pub(super) contract_addr: Pubkey,
    pub(super) init_data_bytes: Vec<u8>,
}

pub(super) async fn prepare_token_create(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    request: TokenCreateRequest,
) -> Result<PreparedTokenCreate> {
    let TokenCreateRequest {
        name,
        symbol,
        wasm,
        decimals,
        initial_supply,
        website,
        logo_url,
        description,
        twitter,
        telegram,
        discord,
        keypair,
    } = request;

    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
    let deployer = keypair_mgr.load_keypair(&path)?;
    let registry_metadata = build_token_registry_metadata(
        decimals,
        description.as_deref(),
        website.as_deref(),
        logo_url.as_deref(),
        twitter.as_deref(),
        telegram.as_deref(),
        discord.as_deref(),
    );
    let wasm_code = load_wasm_code(&wasm, TOKEN_CREATE_WASM_VALIDATION)?;
    let contract_addr = derive_contract_address(client, &deployer, &wasm_code).await;
    let init_data_bytes =
        build_init_data_bytes(&name, &symbol, decimals, registry_metadata.clone());

    Ok(PreparedTokenCreate {
        name,
        symbol,
        wasm,
        decimals,
        initial_supply,
        deployer,
        registry_metadata,
        wasm_code,
        contract_addr,
        init_data_bytes,
    })
}

fn build_init_data_bytes(
    name: &str,
    symbol: &str,
    decimals: u8,
    registry_metadata: Option<serde_json::Value>,
) -> Vec<u8> {
    let mut init_data = serde_json::Map::new();
    init_data.insert("symbol".to_string(), serde_json::json!(symbol));
    init_data.insert("name".to_string(), serde_json::json!(name));
    init_data.insert("template".to_string(), serde_json::json!("mt20"));
    init_data.insert("decimals".to_string(), serde_json::json!(decimals));
    if let Some(metadata) = registry_metadata {
        init_data.insert("metadata".to_string(), metadata);
    }

    serde_json::to_vec(&serde_json::Value::Object(init_data)).unwrap_or_default()
}
