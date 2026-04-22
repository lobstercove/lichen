use anyhow::{anyhow, Result};
use lichen_core::Pubkey;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::output_support::licn_to_spores;

pub(super) async fn handle_transfer(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    to: String,
    amount: f64,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
    let from_keypair = keypair_mgr.load_keypair(&path)?;
    let from_pubkey = from_keypair.pubkey();

    let to_pubkey = Pubkey::from_base58(&to)
        .map_err(|error| anyhow!("Invalid destination address: {}", error))?;
    let spores = licn_to_spores(amount);

    println!("🦞 Transferring {} LICN ({} spores)", amount, spores);
    println!("📤 From: {}", from_pubkey.to_base58());
    println!("📥 To: {}", to_pubkey.to_base58());

    let signature = client.transfer(&from_keypair, &to_pubkey, spores).await?;

    println!("✅ Transaction sent!");
    println!("📝 Signature: {}", signature);

    Ok(())
}
