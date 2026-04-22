use anyhow::Result;

use crate::cli_args::ValidatorCommands;
use crate::client::RpcClient;
use crate::validator_support::{
    handle_validator_info, handle_validator_list, handle_validator_performance,
};

pub(super) async fn handle_chain_validator_command(
    client: &RpcClient,
    command: ValidatorCommands,
) -> Result<()> {
    match command {
        ValidatorCommands::Info { address } => handle_validator_info(client, &address).await?,
        ValidatorCommands::Performance { address } => {
            handle_validator_performance(client, &address).await?
        }
        ValidatorCommands::List => handle_validator_list(client).await?,
    }

    Ok(())
}