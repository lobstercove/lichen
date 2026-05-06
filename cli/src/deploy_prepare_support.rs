use anyhow::Result;
use lichen_core::{Hash, Keypair, Pubkey};
use std::path::PathBuf;

use crate::cli_args::ContractTemplate;
use crate::client::RpcClient;
use crate::contract_wasm_support::{derive_contract_address, load_wasm_code, WasmValidationConfig};
use crate::deploy_support::DeployRequest;
use crate::keypair_manager::KeypairManager;

const DEPLOY_WASM_VALIDATION: WasmValidationConfig<'static> = WasmValidationConfig {
    read_failure_subject: "contract",
    invalid_magic_help: " (\\0asm).\n             Make sure you compiled with: cargo build --target wasm32-unknown-unknown --release\n             The WASM file is at: target/wasm32-unknown-unknown/release/<name>.wasm",
    oversize_help: ".\n             Tip: use wasm-opt or enable LTO in your Cargo.toml [profile.release]",
};

pub(super) struct PreparedContractDeploy {
    pub(super) contract: PathBuf,
    pub(super) deployer: Keypair,
    pub(super) wasm_code: Vec<u8>,
    pub(super) contract_addr: Pubkey,
    pub(super) init_data: Vec<u8>,
    pub(super) symbol: Option<String>,
    pub(super) name: Option<String>,
    pub(super) template: Option<ContractTemplate>,
    pub(super) decimals: Option<u8>,
    pub(super) supply: Option<u64>,
    pub(super) metadata: Option<serde_json::Value>,
}

pub(super) async fn prepare_deploy(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    request: DeployRequest,
) -> Result<PreparedContractDeploy> {
    let DeployRequest {
        contract,
        keypair,
        symbol,
        name,
        template,
        decimals,
        supply,
        metadata,
    } = request;

    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
    let deployer = keypair_mgr.load_keypair(&path)?;
    let wasm_code = load_wasm_code(&contract, DEPLOY_WASM_VALIDATION)?;
    let code_hash = Hash::hash(&wasm_code);
    if client.is_code_hash_deploy_blocked(&code_hash).await? {
        anyhow::bail!(
            "Deployment blocked: code hash {} has an active DeployBlocked restriction",
            code_hash.to_hex()
        );
    }
    let contract_addr = derive_contract_address(client, &deployer, &wasm_code).await;
    let metadata = parse_metadata(metadata)?;
    let init_data = build_init_data(
        symbol.as_deref(),
        name.as_deref(),
        template.as_ref(),
        decimals,
        supply,
        metadata.as_ref(),
    )?;

    Ok(PreparedContractDeploy {
        contract,
        deployer,
        wasm_code,
        contract_addr,
        init_data,
        symbol,
        name,
        template,
        decimals,
        supply,
        metadata,
    })
}

fn parse_metadata(metadata: Option<String>) -> Result<Option<serde_json::Value>> {
    metadata
        .map(|value| {
            serde_json::from_str::<serde_json::Value>(&value)
                .map_err(|error| anyhow::anyhow!("Invalid --metadata JSON: {}", error))
        })
        .transpose()
}

fn build_init_data(
    symbol: Option<&str>,
    name: Option<&str>,
    template: Option<&ContractTemplate>,
    decimals: Option<u8>,
    supply: Option<u64>,
    metadata: Option<&serde_json::Value>,
) -> Result<Vec<u8>> {
    if symbol.is_none()
        && name.is_none()
        && template.is_none()
        && decimals.is_none()
        && supply.is_none()
        && metadata.is_none()
    {
        return Ok(vec![]);
    }

    if supply.is_some() {
        anyhow::bail!(
            "`lichen deploy --supply` was removed because it only wrote metadata and never minted real on-chain supply. Use `lichen token create --initial-supply` for the standard MT-20 flow, or use a contract-specific initialize/mint path."
        );
    }

    let mut registry = serde_json::Map::new();
    if let Some(value) = symbol {
        registry.insert("symbol".to_string(), serde_json::json!(value));
    }
    if let Some(value) = name {
        registry.insert("name".to_string(), serde_json::json!(value));
    }
    if let Some(value) = template {
        registry.insert("template".to_string(), serde_json::json!(value.to_string()));
    }
    if let Some(value) = decimals {
        registry.insert("decimals".to_string(), serde_json::json!(value));
    }
    if let Some(meta) = metadata
        .and_then(|value| value.as_object())
        .filter(|meta| !meta.is_empty())
    {
        registry.insert("metadata".to_string(), serde_json::json!(meta));
    }

    Ok(serde_json::to_vec(&registry).unwrap_or_default())
}
