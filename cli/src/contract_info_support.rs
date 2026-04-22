use anyhow::Result;

use crate::client::RpcClient;
use crate::contract_inspection_output_support::{print_contract_info, print_contract_info_error};

pub(super) async fn handle_contract_info(client: &RpcClient, address: &str) -> Result<()> {
    match client.get_contract_info(address).await {
        Ok(info) => print_contract_info(&info),
        Err(error) => print_contract_info_error(&error),
    }

    Ok(())
}