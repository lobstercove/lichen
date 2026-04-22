use anyhow::{bail, Result};

use crate::chain_support::{
    handle_block, handle_current_slot, handle_latest_block, handle_recent_blockhash,
    handle_total_burned,
};
use crate::cli_args::Commands;
use crate::client::RpcClient;
use crate::validator_support::handle_validator_list;

pub(super) async fn handle_chain_query_command(client: &RpcClient, command: Commands) -> Result<()> {
    match command {
        Commands::Block { slot } => handle_block(client, slot).await?,
        Commands::Slot => handle_current_slot(client).await?,
        Commands::Blockhash => handle_recent_blockhash(client).await?,
        Commands::Latest => handle_latest_block(client).await?,
        Commands::Burned => handle_total_burned(client).await?,
        Commands::Validators => handle_validator_list(client).await?,
        _ => bail!("unsupported chain query command"),
    }

    Ok(())
}