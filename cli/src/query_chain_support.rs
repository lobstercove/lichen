use anyhow::Result;

use crate::config::CliConfig;
use crate::query_rpc_support::perform_query_request;

/// Get block information
pub(crate) async fn get_block(config: &CliConfig, slot: u64) -> Result<()> {
    println!("🔍 Fetching block at slot {}...", slot);

    let response = perform_query_request(config, 1, "getBlock", serde_json::json!([slot])).await?;

    if let Some(error) = response.get("error") {
        println!("❌ Error: {}", error["message"]);
        return Ok(());
    }

    if let Some(block) = response.get("result") {
        println!("\n📦 Block #{}", slot);
        println!(
            "   Hash:         {}",
            block["hash"].as_str().unwrap_or("N/A")
        );
        println!(
            "   Parent:       {}",
            block["parent_hash"].as_str().unwrap_or("N/A")
        );
        println!(
            "   Timestamp:    {}",
            block["timestamp"].as_u64().unwrap_or(0)
        );
        println!(
            "   Transactions: {}",
            block["transaction_count"].as_u64().unwrap_or(0)
        );
        println!(
            "   Validator:    {}",
            block["validator"].as_str().unwrap_or("N/A")
        );
    }

    Ok(())
}

/// List active validators
pub(crate) async fn list_validators(config: &CliConfig) -> Result<()> {
    println!("🔍 Fetching validators...");

    let response = perform_query_request(config, 1, "getValidators", serde_json::json!([])).await?;

    if let Some(validators) = response["result"].as_array() {
        println!("\n👥 Active Validators ({})", validators.len());
        println!(
            "\n{:<45} {:>15} {:>10}",
            "Public Key", "Stake (LICN)", "Status"
        );
        println!("{}", "─".repeat(75));

        for validator in validators {
            let pubkey = validator["pubkey"].as_str().unwrap_or("Unknown");
            let stake = validator["stake"].as_u64().unwrap_or(0);
            let lichen = stake as f64 / 1_000_000_000.0;

            println!(
                "{:<45} {:>15.2} {:>10}",
                pubkey.get(..44).unwrap_or(pubkey),
                lichen,
                "Active"
            );
        }
    }

    Ok(())
}

/// Get chain status
pub(crate) async fn chain_status(config: &CliConfig) -> Result<()> {
    println!("🔍 Fetching chain status...");

    let slot_res = perform_query_request(config, 1, "getSlot", serde_json::json!([])).await?;
    let network_res =
        perform_query_request(config, 2, "getNetworkInfo", serde_json::json!([])).await?;

    println!("\n⛓️  Lichen Status");

    if let Some(slot) = slot_res["result"].as_u64() {
        println!("   Current Slot: {}", slot);
    }

    if let Some(info) = network_res.get("result") {
        println!(
            "   Chain ID:     {}",
            info["chain_id"].as_str().unwrap_or("Unknown")
        );
        println!(
            "   Version:      {}",
            info["version"].as_str().unwrap_or("Unknown")
        );
        println!(
            "   Validators:   {}",
            info["validator_count"].as_u64().unwrap_or(0)
        );
        println!(
            "   Peers:        {}",
            info["peer_count"].as_u64().unwrap_or(0)
        );
    }

    Ok(())
}
