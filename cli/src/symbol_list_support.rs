use anyhow::Result;

use crate::client::RpcClient;
use crate::output_support::print_json;
use crate::symbol_list_output_support::print_symbol_registry;

pub(super) async fn handle_symbol_list(client: &RpcClient, json_output: bool) -> Result<()> {
    match client.get_all_symbol_registry().await {
        Ok(entries) => {
            if json_output {
                print_json(&entries);
            } else {
                print_symbol_registry(&entries);
            }
        }
        Err(error) => {
            if json_output {
                print_json(&serde_json::json!({"error": error.to_string()}));
            } else {
                println!("Could not fetch symbol registry: {}", error);
            }
        }
    }

    Ok(())
}
