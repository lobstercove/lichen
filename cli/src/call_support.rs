use anyhow::{anyhow, Result};
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;

pub(super) async fn handle_call(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    contract: String,
    function: String,
    args: String,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
    let caller = keypair_mgr.load_keypair(&path)?;
    let contract_addr = lichen_core::Pubkey::from_base58(&contract)
        .map_err(|error| anyhow!("Invalid contract address: {}", error))?;

    let args_json: Vec<serde_json::Value> =
        serde_json::from_str(&args).map_err(|error| anyhow!("Invalid args JSON: {}", error))?;
    let args_bytes = serde_json::to_vec(&args_json)?;

    println!("🦞 Calling contract: {}", contract);
    println!("📞 Function: {}", function);
    println!("📋 Args: {}", args);
    println!();

    let signature = client
        .call_contract(&caller, &contract_addr, function.clone(), args_bytes, 0)
        .await?;

    println!("✅ Contract called!");
    println!("📝 Signature: {}", signature);

    Ok(())
}
