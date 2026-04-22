use anyhow::{bail, Result};

use crate::cli_args::Commands;
use crate::client::RpcClient;
use crate::defi_support::handle_defi_command;
use crate::gov_support::handle_gov_command;
use crate::keypair_manager::KeypairManager;
use crate::nft_support::handle_nft_command;
use crate::symbol_support::handle_symbol_command;
use crate::token_support::handle_token_command;

pub(super) async fn handle_ecosystem_command(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    json_output: bool,
    command: Commands,
) -> Result<()> {
    match command {
        Commands::Token(token_cmd) => handle_token_command(client, keypair_mgr, token_cmd).await?,
        Commands::Gov(gov_cmd) => handle_gov_command(client, keypair_mgr, gov_cmd).await?,
        Commands::Symbol(sym_cmd) => handle_symbol_command(client, sym_cmd, json_output).await?,
        Commands::Nft(nft_cmd) => {
            handle_nft_command(client, keypair_mgr, nft_cmd, json_output).await?
        }
        Commands::Defi(defi_cmd) => handle_defi_command(client, defi_cmd, json_output).await?,
        _ => bail!("unsupported ecosystem command"),
    }

    Ok(())
}
