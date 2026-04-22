use anyhow::Result;

use crate::client::RpcClient;
use crate::output_support::print_json;
use crate::symbol_address_output_support::print_symbol_by_address;

pub(super) async fn handle_symbol_by_address(
    client: &RpcClient,
    address: &str,
    json_output: bool,
) -> Result<()> {
    match client.get_symbol_by_program(address).await {
        Ok(entry) => {
            if json_output {
                print_json(&entry);
            } else {
                print_symbol_by_address(address, &entry);
            }
        }
        Err(error) => {
            if json_output {
                print_json(&serde_json::json!({"error": error.to_string(), "address": address}));
            } else {
                println!("No symbol registered for {}: {}", address, error);
            }
        }
    }

    Ok(())
}