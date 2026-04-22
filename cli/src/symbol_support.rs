use anyhow::Result;

use crate::cli_args::SymbolCommands;
use crate::client::RpcClient;
use crate::symbol_address_support::handle_symbol_by_address;
use crate::symbol_list_support::handle_symbol_list;
use crate::symbol_lookup_support::handle_symbol_lookup;

pub(super) async fn handle_symbol_command(
    client: &RpcClient,
    sym_cmd: SymbolCommands,
    json_output: bool,
) -> Result<()> {
    match sym_cmd {
        SymbolCommands::Lookup { symbol } => {
            handle_symbol_lookup(client, &symbol, json_output).await?
        }
        SymbolCommands::List => handle_symbol_list(client, json_output).await?,
        SymbolCommands::ByAddress { address } => {
            handle_symbol_by_address(client, &address, json_output).await?
        }
    }

    Ok(())
}
