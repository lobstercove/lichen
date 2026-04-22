use anyhow::{anyhow, Result};
use lichen_core::Pubkey;

use crate::config::CliConfig;
use crate::query_rpc_support::perform_query_request;

/// Get account balance
pub(crate) async fn get_balance(config: &CliConfig, address: &str) -> Result<()> {
    let _pubkey =
        Pubkey::from_base58(address).map_err(|error| anyhow!("Invalid address format: {error}"))?;

    println!("🔍 Querying balance for: {}", address);

    let response =
        perform_query_request(config, 1, "getBalance", serde_json::json!([address])).await?;

    if let Some(error) = response.get("error") {
        println!("❌ Error: {}", error["message"]);
        return Ok(());
    }

    if let Some(result) = response.get("result") {
        if let Some(spores) = result["lichen"].as_u64() {
            let lichen = spores as f64 / 1_000_000_000.0;
            println!("\n💰 Balance: {} LICN", lichen);
            println!("   ({} spores)", spores);
        } else {
            println!("\n💰 Account not found or has 0 balance");
        }
    }

    Ok(())
}
