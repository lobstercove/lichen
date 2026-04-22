use anyhow::Result;

use crate::client::RpcClient;
use crate::contract_inspection_output_support::{print_contract_list, print_contract_list_error};

pub(super) async fn handle_contract_list(client: &RpcClient) -> Result<()> {
    match client.get_all_contracts().await {
        Ok(contracts) => print_contract_list(&contracts),
        Err(error) => print_contract_list_error(&error),
    }

    Ok(())
}