use anyhow::{bail, Result};

use crate::cli_args::Commands;
use crate::client::RpcClient;
use crate::epoch_support::handle_epoch;
use crate::info_support::{handle_fees, handle_host_functions};
use crate::supply_support::handle_supply;

pub(super) async fn handle_protocol_command(
    client: &RpcClient,
    json_output: bool,
    command: Commands,
) -> Result<()> {
    match command {
        Commands::Supply => handle_supply(client, json_output).await?,
        Commands::Fees => handle_fees(json_output)?,
        Commands::Epoch => handle_epoch(client, json_output).await?,
        Commands::HostFunctions => handle_host_functions(json_output)?,
        _ => bail!("unsupported protocol command"),
    }

    Ok(())
}
