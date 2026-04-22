use anyhow::Result;

use crate::client::RpcClient;

pub(super) async fn handle_account_info(client: &RpcClient, address: &str) -> Result<()> {
    println!("🦞 Account Information");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    match client.get_account_info(address).await {
        Ok(info) => {
            println!("📍 Address: {}", info.pubkey);
            println!("💰 Balance: {} LICN ({} spores)", info.lichen, info.balance);
            println!("📦 Exists: {}", if info.exists { "Yes" } else { "No" });
            println!(
                "⚙️  Executable: {}",
                if info.is_executable {
                    "Yes (Contract)"
                } else {
                    "No"
                }
            );
            println!(
                "🦞 Validator: {}",
                if info.is_validator { "Yes" } else { "No" }
            );
        }
        Err(error) => {
            println!("⚠️  Could not fetch account info: {}", error);
        }
    }

    Ok(())
}

pub(super) async fn handle_account_history(
    client: &RpcClient,
    address: &str,
    limit: usize,
) -> Result<()> {
    println!("🦞 Transaction History");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();
    println!("📍 Account: {}", address);
    println!("📊 Showing last {} transactions", limit);
    println!();

    match client.get_transaction_history(address, limit).await {
        Ok(transactions) => {
            if transactions.is_empty() {
                println!("No transactions found");
            } else {
                for (index, transaction) in transactions.iter().enumerate() {
                    println!("#{} Slot {}", index + 1, transaction.slot);
                    println!("   Signature: {}", transaction.signature);
                    println!("   From: {}", transaction.from);
                    println!("   To: {}", transaction.to);
                    println!(
                        "   Amount: {} LICN",
                        transaction.amount as f64 / 1_000_000_000.0
                    );
                    println!();
                }
            }
        }
        Err(error) => {
            println!("⚠️  Could not fetch transaction history: {}", error);
        }
    }

    Ok(())
}
