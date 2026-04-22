use anyhow::Result;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::stake_address_support::resolve_stake_address;
use crate::stake_query_output_support::print_stake_rewards_details;

pub(super) async fn show_stake_rewards(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    address: Option<String>,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let addr_str = resolve_stake_address(keypair_mgr, address, keypair)?;

    println!("🦞 Staking Rewards");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    match client.get_staking_rewards(&addr_str).await {
        Ok(rewards) => {
            print_stake_rewards_details(&rewards);
        }
        Err(error) => {
            println!("⚠️  Could not fetch rewards: {}", error);
        }
    }

    Ok(())
}