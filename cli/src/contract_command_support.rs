use anyhow::Result;

use crate::cli_args::ContractCommands;
use crate::keypair_manager::KeypairManager;
use crate::{
    client::RpcClient,
    contract_codegen_command_support::handle_generate_contract_client,
    contract_info_support::handle_contract_info,
    contract_list_support::handle_contract_list,
    contract_logs_support::handle_contract_logs,
    contract_registration_support::{handle_contract_register, ContractRegistrationRequest},
};

pub(super) async fn handle_contract_command(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    contract_cmd: ContractCommands,
) -> Result<()> {
    match contract_cmd {
        ContractCommands::Info { address } => handle_contract_info(client, &address).await?,

        ContractCommands::Logs { address, limit } => {
            handle_contract_logs(client, &address, limit).await?
        }

        ContractCommands::List => handle_contract_list(client).await?,

        ContractCommands::Register {
            address,
            symbol,
            name,
            template,
            decimals,
            keypair,
        } => {
            handle_contract_register(
                client,
                keypair_mgr,
                ContractRegistrationRequest {
                    address,
                    symbol,
                    name,
                    template,
                    decimals,
                    keypair,
                },
            )
            .await?
        }

        ContractCommands::GenerateClient {
            abi,
            address,
            lang,
            output,
        } => handle_generate_contract_client(client, abi, address, lang, output).await?,
    }

    Ok(())
}
