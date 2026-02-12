//! Basic usage example

use moltchain_sdk::{Client, Keypair, Balance};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🦞 MoltChain Rust SDK Example\n");
    
    // 1. Create client
    let client = Client::new("http://localhost:8899");
    
    // 2. Generate keypair
    println!("🔑 Generating keypair...");
    let keypair = Keypair::new();
    println!("   Public key: {}", keypair.pubkey().to_base58());
    
    // 3. Get current slot
    match client.get_slot().await {
        Ok(slot) => println!("\n📍 Current slot: {}", slot),
        Err(e) => eprintln!("Failed to get slot: {}", e),
    }
    
    // 4. Get network info
    match client.get_network_info().await {
        Ok(info) => {
            println!("\n⛓️  Network Info:");
            println!("   Chain ID: {}", info.chain_id);
            println!("   Version: {}", info.version);
            println!("   Validators: {}", info.validator_count);
        }
        Err(e) => eprintln!("Failed to get network info: {}", e),
    }
    
    // 5. Check balance
    match client.get_balance(&keypair.pubkey()).await {
        Ok(balance) => {
            println!("\n💰 Balance: {} MOLT", balance.molt());
        }
        Err(e) => {
            println!("\n💰 Balance: 0 MOLT (account not found)");
        }
    }
    
    // 6. List validators
    match client.get_validators().await {
        Ok(validators) => {
            println!("\n👥 Validators: {}", validators.len());
        }
        Err(e) => eprintln!("Failed to get validators: {}", e),
    }
    
    println!("\n✅ Example complete!");
    
    Ok(())
}
