use anyhow::{anyhow, Result};
use lichen_core::Pubkey;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::stake_signer_support::load_staker_keypair;

pub(super) async fn show_stake_add(
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

    println!("🦞 Staking LICN");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("💰 Amount: {} LICN", amount as f64 / 1_000_000_000.0);
    println!("👤 Validator: {}", validator.to_base58());
    let validator_info = client
        .get_validator_info(&validator.to_base58())
        .await
        .map_err(|error| {
            anyhow!(
                "cannot verify validator commission and capacity before staking: {error}"
            )
        })?;
    println!(
        "💼 Current commission: {:.2}%",
        validator_info.commission_rate as f64 / 100.0
    );
    if let (Some(pending), Some(epoch)) = (
        validator_info.pending_commission_rate,
        validator_info.pending_commission_activation_epoch,
    ) {
        println!(
            "⏳ Pending commission: {:.2}% from epoch {}",
            pending as f64 / 100.0,
            epoch
        );
    }
    if validator_info.staking_v2_active {
        println!(
            "📊 Saturation: {:.2}% · remaining capacity: {:.9} LICN",
            validator_info.saturation_usage_bps as f64 / 100.0,
            validator_info.delegation_capacity_remaining as f64 / 1_000_000_000.0
        );
        if kp.pubkey() != validator && amount > validator_info.delegation_capacity_remaining {
            return Err(anyhow!(
                "delegation exceeds validator capacity: requested {} spores, remaining {} spores",
                amount,
                validator_info.delegation_capacity_remaining
            ));
        }
    }
    println!();

    match client.stake_to(&kp, &validator, amount).await {
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
