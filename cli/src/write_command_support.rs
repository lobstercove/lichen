use anyhow::{bail, Result};

use crate::airdrop_support::handle_airdrop;
use crate::call_support::handle_call;
use crate::cli_args::Commands;
use crate::client::RpcClient;
use crate::deploy_support::{handle_deploy, DeployRequest};
use crate::keypair_manager::KeypairManager;
use crate::transfer_support::handle_transfer;
use crate::upgrade_support::handle_upgrade;

pub(super) async fn handle_write_command(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    command: Commands,
) -> Result<()> {
    match command {
        Commands::Transfer {
            to,
            amount,
            keypair,
        } => handle_transfer(client, keypair_mgr, to, amount, keypair).await?,
        Commands::Airdrop {
            amount,
            pubkey,
            keypair,
        } => handle_airdrop(client, keypair_mgr, amount, pubkey, keypair).await?,
        Commands::Deploy {
            contract,
            keypair,
            symbol,
            name,
            template,
            decimals,
            supply,
            metadata,
        } => {
            handle_deploy(
                client,
                keypair_mgr,
                DeployRequest {
                    contract,
                    keypair,
                    symbol,
                    name,
                    template,
                    decimals,
                    supply,
                    metadata,
                },
            )
            .await?
        }
        Commands::Upgrade {
            address,
            contract,
            keypair,
        } => handle_upgrade(client, keypair_mgr, address, contract, keypair).await?,
        Commands::Call {
            contract,
            function,
            args,
            keypair,
        } => handle_call(client, keypair_mgr, contract, function, args, keypair).await?,
        _ => bail!("unsupported write command"),
    }

    Ok(())
}
