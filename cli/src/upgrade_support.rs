use anyhow::{anyhow, Result};
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;

pub(super) async fn handle_upgrade(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    address: String,
    contract: PathBuf,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
    let owner = keypair_mgr.load_keypair(&path)?;

    let contract_pubkey = lichen_core::Pubkey::from_base58(&address)
        .map_err(|error| anyhow!("Invalid contract address: {}", error))?;

    let wasm_code = std::fs::read(&contract)
        .map_err(|error| anyhow!("Failed to read contract file: {}", error))?;

    println!("🦞 Upgrading contract: {}", contract_pubkey.to_base58());
    println!("📦 New code size: {} KB", wasm_code.len() / 1024);
    println!("👤 Owner: {}", owner.pubkey().to_base58());
    println!();

    let signature = client
        .upgrade_contract(&owner, wasm_code, &contract_pubkey)
        .await?;

    println!("✅ Contract upgraded!");
    println!("📝 Signature: {}", signature);
    println!("🔗 Address: {}", contract_pubkey.to_base58());

    Ok(())
}
