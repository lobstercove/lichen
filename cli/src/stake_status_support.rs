use anyhow::Result;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::stake_address_support::resolve_stake_address;
use crate::stake_query_output_support::print_stake_status_details;

pub(super) async fn show_stake_status(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    address: Option<String>,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let addr_str = resolve_stake_address(keypair_mgr, address, keypair)?;

    println!("🦞 Staking Status");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    match client.get_staking_status(&addr_str).await {
        Ok(status) => {
            print_stake_status_details(&status);
        }
        Err(error) => {
            println!("⚠️  Could not fetch staking status: {}", error);
        }
    }

    Ok(())
}