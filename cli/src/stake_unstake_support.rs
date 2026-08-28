use anyhow::{anyhow, Result};
use lichen_core::Pubkey;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::stake_signer_support::load_staker_keypair;

pub(super) async fn show_stake_remove(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    amount: u64,
    validator: Option<String>,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let kp = load_staker_keypair(keypair_mgr, keypair)?;
    let validator = validator
        .map(|value| {
            Pubkey::from_base58(&value)
                .map_err(|error| anyhow!("invalid validator public key '{value}': {error}"))
        })
        .transpose()?
        .unwrap_or_else(|| kp.pubkey());

    println!("🦞 Unstaking LICN");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("💰 Amount: {} LICN", amount as f64 / 1_000_000_000.0);
    println!("👤 Validator: {}", validator.to_base58());
    println!();

    match client.unstake_from(&kp, &validator, amount).await {
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
