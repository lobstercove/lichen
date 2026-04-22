use anyhow::{bail, Result};

use crate::chain_metrics_support::handle_chain_metrics;
use crate::chain_network_command_support::handle_chain_network_command;
use crate::chain_query_command_support::handle_chain_query_command;
use crate::chain_status_support::handle_chain_status;
use crate::chain_validator_command_support::handle_chain_validator_command;
use crate::cli_args::Commands;
use crate::client::RpcClient;

pub(super) async fn handle_chain_command(
    client: &RpcClient,
    rpc_url: &str,
    command: Commands,
) -> Result<()> {
    match command {
        query_cmd @ (Commands::Block { .. }
        | Commands::Slot
        | Commands::Blockhash
        | Commands::Latest
        | Commands::Burned
        | Commands::Validators) => handle_chain_query_command(client, query_cmd).await?,
        Commands::Network(net_cmd) => {
            handle_chain_network_command(client, rpc_url, net_cmd).await?
        }
        Commands::Validator(val_cmd) => handle_chain_validator_command(client, val_cmd).await?,
        Commands::Status => handle_chain_status(client).await?,
        Commands::Metrics => handle_chain_metrics(client).await?,
        _ => bail!("unsupported chain command"),
    }

    Ok(())
}
