use anyhow::Result;

use crate::cli_args::StakeCommands;
use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::stake_query_support::{handle_stake_rewards, handle_stake_status};
use crate::stake_write_support::{handle_stake_add, handle_stake_remove};

pub(super) async fn handle_stake_command(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    stake_cmd: StakeCommands,
) -> Result<()> {
    match stake_cmd {
        StakeCommands::Add { amount, keypair } => {
            handle_stake_add(client, keypair_mgr, amount, keypair).await?
        }
        StakeCommands::Remove { amount, keypair } => {
            handle_stake_remove(client, keypair_mgr, amount, keypair).await?
        }
        StakeCommands::Status { address, keypair } => {
            handle_stake_status(client, keypair_mgr, address, keypair).await?
        }
        StakeCommands::Rewards { address, keypair } => {
            handle_stake_rewards(client, keypair_mgr, address, keypair).await?
        }
    }

    Ok(())
}
