use anyhow::Result;

use crate::cli_args::NetworkCommands;
use crate::client::RpcClient;
use crate::network_support::{handle_network_info, handle_network_peers, handle_network_status};

pub(super) async fn handle_chain_network_command(
    client: &RpcClient,
    rpc_url: &str,
    command: NetworkCommands,
) -> Result<()> {
    match command {
        NetworkCommands::Status => handle_network_status(client).await?,
        NetworkCommands::Peers => handle_network_peers(client).await?,
        NetworkCommands::Info => handle_network_info(client, rpc_url).await?,
    }

    Ok(())
}