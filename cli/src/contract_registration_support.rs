use anyhow::{anyhow, Result};
use lichen_core::Pubkey;
use std::path::PathBuf;

use crate::cli_args::ContractTemplate;
use crate::client::{RpcClient, SymbolRegistration};
use crate::keypair_manager::KeypairManager;

pub(super) struct ContractRegistrationRequest {
    pub(super) address: String,
    pub(super) symbol: String,
    pub(super) name: Option<String>,
    pub(super) template: Option<ContractTemplate>,
    pub(super) decimals: Option<u8>,
    pub(super) keypair: Option<PathBuf>,
}

pub(super) async fn handle_contract_register(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    request: ContractRegistrationRequest,
) -> Result<()> {
    let ContractRegistrationRequest {
        address,
        symbol,
        name,
        template,
        decimals,
        keypair,
    } = request;

    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
    let owner = keypair_mgr.load_keypair(&path)?;
    let contract_pubkey = Pubkey::from_base58(&address)
        .map_err(|error| anyhow!("Invalid contract address: {}", error))?;

    println!("🏷️  Registering contract in symbol registry");
    println!("📍 Contract: {}", address);
    println!("🏷️  Symbol: {}", symbol);
    if let Some(ref display_name) = name {
        println!("📛 Name: {}", display_name);
    }
    if let Some(ref template_name) = template {
        println!("📂 Template: {}", template_name);
    }
    if let Some(decimals_value) = decimals {
        println!("🔢 Decimals: {}", decimals_value);
    }
    println!("👤 Owner: {}", owner.pubkey().to_base58());
    println!();

    let template_str = template.as_ref().map(|value| value.to_string());
    let signature = client
        .register_symbol(
            &owner,
            &contract_pubkey,
            SymbolRegistration {
                symbol: &symbol,
                name: name.as_deref(),
                template: template_str.as_deref(),
                decimals,
                metadata: None,
            },
        )
        .await?;

    println!("✅ Symbol registered!");
    println!("📝 Signature: {}", signature);

    Ok(())
}