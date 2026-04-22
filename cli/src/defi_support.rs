use anyhow::Result;

use crate::cli_args::DefiCommands;
use crate::client::RpcClient;
use crate::defi_overview_support::handle_defi_overview;
use crate::defi_protocol_support::handle_defi_protocol;

pub(super) async fn handle_defi_command(
    client: &RpcClient,
    defi_cmd: DefiCommands,
    json_output: bool,
) -> Result<()> {
    match defi_cmd {
        DefiCommands::Dex => {
            handle_defi_protocol(
                client,
                "getDexCoreStats",
                "SporeSwap DEX Stats",
                "DEX",
                json_output,
            )
            .await?
        }
        DefiCommands::Amm => {
            handle_defi_protocol(
                client,
                "getDexAmmStats",
                "AMM Pool Stats",
                "AMM",
                json_output,
            )
            .await?
        }
        DefiCommands::Lending => {
            handle_defi_protocol(
                client,
                "getThallLendStats",
                "ThallLend Stats",
                "lending",
                json_output,
            )
            .await?
        }
        DefiCommands::Overview => handle_defi_overview(client, json_output).await?,
    }

    Ok(())
}
