use anyhow::Result;

use crate::account_support::{handle_account_history, handle_account_info};
use crate::cli_args::AccountCommands;
use crate::client::RpcClient;

pub(super) async fn handle_account_command(
    client: &RpcClient,
    command: AccountCommands,
) -> Result<()> {
    match command {
        AccountCommands::Info { address } => handle_account_info(client, &address).await?,
        AccountCommands::History { address, limit } => {
            handle_account_history(client, &address, limit).await?
        }
    }

    Ok(())
}
