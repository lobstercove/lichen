use anyhow::{bail, Result};

use crate::cli_args::Commands;
use crate::client::RpcClient;
use crate::config_support::handle_config_command;
use crate::tx_support::handle_transaction_lookup;
use crate::version_support::handle_version;

pub(super) async fn handle_utility_command(
    client: &RpcClient,
    rpc_url: &str,
    json_output: bool,
    command: Commands,
) -> Result<()> {
    match command {
        Commands::Version => handle_version(rpc_url, json_output)?,
        Commands::Config(config_cmd) => handle_config_command(config_cmd, json_output)?,
        Commands::Tx { signature } => {
            handle_transaction_lookup(client, &signature, json_output).await?
        }
        _ => bail!("unsupported utility command"),
    }

    Ok(())
}
