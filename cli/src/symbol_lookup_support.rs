use anyhow::Result;

use crate::client::RpcClient;
use crate::output_support::print_json;
use crate::symbol_lookup_output_support::print_symbol_lookup;

pub(super) async fn handle_symbol_lookup(
    client: &RpcClient,
    symbol: &str,
    json_output: bool,
) -> Result<()> {
    match client.get_symbol_registry(symbol).await {
        Ok(entry) => {
            if json_output {
                print_json(&entry);
            } else {
                print_symbol_lookup(symbol, &entry);
            }
        }
        Err(error) => {
            if json_output {
                print_json(&serde_json::json!({"error": error.to_string(), "symbol": symbol}));
            } else {
                println!("Symbol '{}' not found: {}", symbol, error);
            }
        }
    }

    Ok(())
}