use anyhow::Result;

use crate::client::RpcClient;
use crate::contract_inspection_output_support::{print_contract_logs, print_contract_logs_error};

pub(super) async fn handle_contract_logs(
    client: &RpcClient,
    address: &str,
    limit: usize,
) -> Result<()> {
    match client.get_contract_logs(address, limit).await {
        Ok(logs) => print_contract_logs(address, limit, &logs),
        Err(error) => print_contract_logs_error(address, limit, &error),
    }

    Ok(())
}