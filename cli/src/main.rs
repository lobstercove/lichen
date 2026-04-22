// Lichen CLI - Command-line interface for agents
// "Every crab, lobster, and shrimp can access the moss!"

use anyhow::Result;
use clap::Parser;

include!("main_modules.rs");

use chain_command_support::handle_chain_command;
use cli_args::{Cli, Commands, OutputFormat};
use client::RpcClient;
use ecosystem_command_support::handle_ecosystem_command;
use keypair_manager::KeypairManager;
use operational_command_support::handle_operational_command;
use protocol_command_support::handle_protocol_command;
use utility_command_support::handle_utility_command;
use write_command_support::handle_write_command;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let json_output = cli.output == OutputFormat::Json;
    let client = RpcClient::new(&cli.rpc_url);
    let keypair_mgr = KeypairManager::new();

    match cli.command {
        command @ (Commands::Identity(_)
        | Commands::Wallet(_)
        | Commands::Init { .. }
        | Commands::Balance { .. }
        | Commands::Stake(_)
        | Commands::Account(_)
        | Commands::Contract(_)) => {
            handle_operational_command(&client, &keypair_mgr, json_output, command).await?
        }

        command @ (Commands::Transfer { .. }
        | Commands::Airdrop { .. }
        | Commands::Deploy { .. }
        | Commands::Upgrade { .. }
        | Commands::Call { .. }) => handle_write_command(&client, &keypair_mgr, command).await?,

        command @ (Commands::Block { .. }
        | Commands::Slot
        | Commands::Blockhash
        | Commands::Latest
        | Commands::Burned
        | Commands::Validators
        | Commands::Network(_)
        | Commands::Validator(_)
        | Commands::Status
        | Commands::Metrics) => handle_chain_command(&client, &cli.rpc_url, command).await?,

        command @ (Commands::Token(_)
        | Commands::Gov(_)
        | Commands::Symbol(_)
        | Commands::Nft(_)
        | Commands::Defi(_)) => {
            handle_ecosystem_command(&client, &keypair_mgr, json_output, command).await?
        }

        command @ (Commands::Version | Commands::Config(_) | Commands::Tx { .. }) => {
            handle_utility_command(&client, &cli.rpc_url, json_output, command).await?
        }

        command @ (Commands::Supply
        | Commands::Fees
        | Commands::Epoch
        | Commands::HostFunctions) => {
            handle_protocol_command(&client, json_output, command).await?
        }
    }

    Ok(())
}
