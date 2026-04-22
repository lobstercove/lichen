use anyhow::Result;

use crate::client::RpcClient;

pub(super) async fn handle_validator_info(client: &RpcClient, address: &str) -> Result<()> {
    println!("🦞 Validator Information");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    match client.get_validator_info(address).await {
        Ok(info) => {
            println!("📍 Pubkey: {}", info.pubkey);
            println!("💰 Stake: {} LICN", info.stake as f64 / 1_000_000_000.0);
            println!("⭐ Reputation: {}", info.reputation);
            println!(
                "📊 Status: {}",
                if info.is_active { "Active" } else { "Inactive" }
            );
            println!("📦 Blocks produced: {}", info.blocks_produced);
        }
        Err(error) => {
            println!("⚠️  Validator not found: {}", error);
        }
    }

    Ok(())
}

pub(super) async fn handle_validator_performance(client: &RpcClient, address: &str) -> Result<()> {
    println!("🦞 Validator Performance");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    match client.get_validator_performance(address).await {
        Ok(performance) => {
            println!("📍 Validator: {}", address);
            println!();
            println!("📊 Epoch Performance:");
            println!("   Blocks produced: {}", performance.blocks_produced);
            println!("   Blocks expected: {}", performance.blocks_expected);
            let success_rate = if performance.blocks_expected > 0 {
                (performance.blocks_produced as f64 / performance.blocks_expected as f64) * 100.0
            } else {
                0.0
            };
            println!("   Success rate: {:.2}%", success_rate);
            println!("   Average block time: {}ms", performance.avg_block_time_ms);
            println!();
            println!("⏰ Uptime: {:.2}%", performance.uptime_percent);
        }
        Err(error) => {
            println!("⚠️  Could not fetch performance: {}", error);
        }
    }

    Ok(())
}

pub(super) async fn handle_validator_list(client: &RpcClient) -> Result<()> {
    let validators_info = client.get_validators().await?;
    let validators = &validators_info.validators;

    println!("🦞 Active Validators");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    if validators.is_empty() {
        println!("No validators found");
    } else {
        for (index, validator) in validators.iter().enumerate() {
            println!("#{} {}", index + 1, validator.pubkey);
            println!(
                "   Stake: {} LICN",
                validator.stake as f64 / 1_000_000_000.0
            );
            println!("   Reputation: {}", validator.reputation);
            println!();
        }

        let total_stake: u64 = validators.iter().map(|validator| validator.stake).sum();
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!(
            "Total: {} validators, {} LICN staked",
            validators.len(),
            total_stake as f64 / 1_000_000_000.0
        );
    }

    Ok(())
}
