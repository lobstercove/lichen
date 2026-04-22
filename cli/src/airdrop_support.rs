use anyhow::{anyhow, Result};
use lichen_core::Pubkey;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;

pub(super) async fn handle_airdrop(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    amount: f64,
    pubkey: Option<String>,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let recipient = if let Some(address) = pubkey {
        Pubkey::from_base58(&address).map_err(|error| anyhow!("Invalid address: {}", error))?
    } else {
        let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
        let keypair = keypair_mgr.load_keypair(&path)?;
        keypair.pubkey()
    };

    println!("🦞 Requesting {} LICN airdrop...", amount);
    println!("📥 To: {}", recipient.to_base58());
    println!();

    match client.request_airdrop(&recipient, amount).await {
        Ok(signature) => {
            println!("✅ Airdrop received!");
            println!("📝 Signature: {}", signature);
        }
        Err(error) => {
            println!("⚠️  Airdrop failed: {}", error);
            println!("💡 Ensure the node is running in testnet/devnet mode");
        }
    }

    Ok(())
}
