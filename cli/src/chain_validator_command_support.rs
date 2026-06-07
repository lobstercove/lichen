use anyhow::Result;

use crate::cli_args::ValidatorCommands;
use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::validator_support::{
    handle_validator_fingerprint, handle_validator_info, handle_validator_list,
    handle_validator_performance, handle_validator_register,
};

pub(super) async fn handle_chain_validator_command(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    command: ValidatorCommands,
) -> Result<()> {
    match command {
        ValidatorCommands::Info { address } => handle_validator_info(client, &address).await?,
        ValidatorCommands::Performance { address } => {
            handle_validator_performance(client, &address).await?
        }
        ValidatorCommands::List => handle_validator_list(client).await?,
        ValidatorCommands::Fingerprint => handle_validator_fingerprint()?,
        ValidatorCommands::Register {
            amount,
            keypair,
            fingerprint_hex,
        } => handle_validator_register(client, keypair_mgr, amount, keypair, fingerprint_hex).await?,
    }

    Ok(())
}
