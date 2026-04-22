use anyhow::Result;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::stake_signer_support::load_staker_keypair;

pub(super) async fn show_stake_remove(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    amount: u64,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let kp = load_staker_keypair(keypair_mgr, keypair)?;

    println!("🦞 Unstaking LICN");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("💰 Amount: {} LICN", amount as f64 / 1_000_000_000.0);
    println!("👤 Validator: {}", kp.pubkey().to_base58());
    println!();

    match client.unstake(&kp, amount).await {
        Ok(signature) => {
            println!("✅ Unstake transaction sent!");
            println!("📝 Signature: {}", signature);
            println!();
            println!("💡 Tokens will be available after unbonding period");
        }
        Err(error) => {
            println!("⚠️  Unstaking failed: {}", error);
        }
    }

    Ok(())
}
