use anyhow::{bail, Result};

use crate::balance_support::handle_balance;
use crate::cli_args::Commands;
use crate::client::RpcClient;
use crate::contract_command_support::handle_contract_command;
use crate::identity_support::handle_identity_command;
use crate::init_support::handle_init_command;
use crate::keypair_manager::KeypairManager;
use crate::stake_support::handle_stake_command;
use crate::wallet_support::handle_wallet_command;

use crate::account_command_support::handle_account_command;

pub(super) async fn handle_operational_command(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    json_output: bool,
    command: Commands,
) -> Result<()> {
    match command {
        Commands::Identity(id_cmd) => {
            handle_identity_command(keypair_mgr, id_cmd, json_output).await?
        }
        Commands::Wallet(wallet_cmd) => handle_wallet_command(client, wallet_cmd).await?,
        Commands::Init { output } => handle_init_command(output)?,
        Commands::Balance { address, keypair } => {
            handle_balance(client, keypair_mgr, address, keypair).await?
        }
        Commands::Stake(stake_cmd) => handle_stake_command(client, keypair_mgr, stake_cmd).await?,
        Commands::Account(acc_cmd) => handle_account_command(client, acc_cmd).await?,
        Commands::Contract(contract_cmd) => {
            handle_contract_command(client, keypair_mgr, contract_cmd).await?
        }
        _ => bail!("unsupported operational command"),
    }

    Ok(())
}
