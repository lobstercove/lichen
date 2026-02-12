// Query operations (balance, blocks, validators, etc.)

use anyhow::{Context, Result};
use moltchain_core::Pubkey;

use crate::config::CliConfig;

/// Get account balance
pub async fn get_balance(config: &CliConfig, address: &str) -> Result<()> {
    // Parse address
    let pubkey = Pubkey::from_base58(address)
        .context("Invalid address format")?;
    
    println!("🔍 Querying balance for: {}", address);
    
    // Make RPC call
    let client = reqwest::Client::new();
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getBalance",
        "params": [address]
    });
    
    let response = client
        .post(&config.rpc_url)
        .json(&request)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    
    if let Some(error) = response.get("error") {
        println!("❌ Error: {}", error["message"]);
        return Ok(());
    }
    
    if let Some(result) = response.get("result") {
        if let Some(shells) = result["molt"].as_u64() {
            let molt = shells as f64 / 1_000_000_000.0;
            println!("\n💰 Balance: {} MOLT", molt);
            println!("   ({} shells)", shells);
        } else {
            println!("\n💰 Account not found or has 0 balance");
        }
    }
    
    Ok(())
}

/// Get block information
pub async fn get_block(config: &CliConfig, slot: u64) -> Result<()> {
    println!("🔍 Fetching block at slot {}...", slot);
    
    let client = reqwest::Client::new();
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getBlock",
        "params": [slot]
    });
    
    let response = client
        .post(&config.rpc_url)
        .json(&request)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    
    if let Some(error) = response.get("error") {
        println!("❌ Error: {}", error["message"]);
        return Ok(());
    }
    
    if let Some(block) = response.get("result") {
        println!("\n📦 Block #{}", slot);
        println!("   Hash:         {}", block["hash"].as_str().unwrap_or("N/A"));
        println!("   Parent:       {}", block["parent_hash"].as_str().unwrap_or("N/A"));
        println!("   Timestamp:    {}", block["timestamp"].as_u64().unwrap_or(0));
        println!("   Transactions: {}", block["transaction_count"].as_u64().unwrap_or(0));
        println!("   Validator:    {}", block["validator"].as_str().unwrap_or("N/A"));
    }
    
    Ok(())
}

/// List active validators
pub async fn list_validators(config: &CliConfig) -> Result<()> {
    println!("🔍 Fetching validators...");
    
    let client = reqwest::Client::new();
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getValidators",
        "params": []
    });
    
    let response = client
        .post(&config.rpc_url)
        .json(&request)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    
    if let Some(validators) = response["result"].as_array() {
        println!("\n👥 Active Validators ({})", validators.len());
        println!("\n{:<45} {:>15} {:>10}", "Public Key", "Stake (MOLT)", "Status");
        println!("{}", "─".repeat(75));
        
        for validator in validators {
            let pubkey = validator["pubkey"].as_str().unwrap_or("Unknown");
            let stake = validator["stake"].as_u64().unwrap_or(0);
            let molt = stake as f64 / 1_000_000_000.0;
            
            println!("{:<45} {:>15.2} {:>10}", 
                &pubkey[..44], 
                molt,
                "Active"
            );
        }
    }
    
    Ok(())
}

/// Get chain status
pub async fn chain_status(config: &CliConfig) -> Result<()> {
    println!("🔍 Fetching chain status...");
    
    let client = reqwest::Client::new();
    
    // Get slot
    let slot_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getSlot",
        "params": []
    });
    
    let slot_res = client
        .post(&config.rpc_url)
        .json(&slot_req)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    
    // Get network info
    let network_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "getNetworkInfo",
        "params": []
    });
    
    let network_res = client
        .post(&config.rpc_url)
        .json(&network_req)
        .send()
        .await?
        .json::<serde_json::Value>()
        .await?;
    
    println!("\n⛓️  MoltChain Status");
    
    if let Some(slot) = slot_res["result"].as_u64() {
        println!("   Current Slot: {}", slot);
    }
    
    if let Some(info) = network_res.get("result") {
        println!("   Chain ID:     {}", info["chain_id"].as_str().unwrap_or("Unknown"));
        println!("   Version:      {}", info["version"].as_str().unwrap_or("Unknown"));
        println!("   Validators:   {}", info["validator_count"].as_u64().unwrap_or(0));
        println!("   Peers:        {}", info["peer_count"].as_u64().unwrap_or(0));
    }
    
    Ok(())
}
