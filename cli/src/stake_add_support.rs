use anyhow::Result;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::stake_signer_support::load_staker_keypair;

pub(super) async fn show_stake_add(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    amount: u64,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let kp = load_staker_keypair(keypair_mgr, keypair)?;

    println!("🦞 Staking LICN");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("💰 Amount: {} LICN", amount as f64 / 1_000_000_000.0);
    println!("👤 Validator: {}", kp.pubkey().to_base58());
    println!();

    match client.stake(&kp, amount).await {
        Ok(signature) => {
            println!("✅ Stake transaction sent!");
            println!("📝 Signature: {}", signature);
            println!();
            println!("💡 Your stake will be active in the next epoch");
        }
        Err(error) => {
            println!("⚠️  Staking failed: {}", error);
        }
    }

    Ok(())
}
