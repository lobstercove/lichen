use anyhow::Result;

use crate::client::RpcClient;

pub(super) async fn handle_account_info(client: &RpcClient, address: &str) -> Result<()> {
    println!("🦞 Account Information");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    match client.get_account_info(address).await {
        Ok(info) => {
            println!("📍 Address: {}", info.pubkey);
            println!("💰 Balance: {} LICN ({} spores)", info.licn, info.balance);
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
        Ok(history) => {
            if history.transactions.is_empty() {
                println!("No transactions found");
            } else {
                for (index, transaction) in history.transactions.iter().enumerate() {
                    println!("#{} Slot {}", index + 1, transaction.slot);
                    println!("   Signature: {}", transaction.signature);
                    println!("   Type: {}", transaction.tx_type);
                    println!("   From: {}", transaction.from);
                    println!("   To: {}", transaction.to);
                    println!("   Amount: {} LICN", transaction.amount);
                    println!("   Amount spores: {}", transaction.amount_spores);
                    println!("   Fee spores: {}", transaction.fee_spores);
                    println!();
                }
                if history.has_more {
                    if let Some(next_before_slot) = history.next_before_slot {
                        println!(
                            "More transactions available before slot {}.",
                            next_before_slot
                        );
                    } else {
                        println!("More transactions available.");
                    }
                }
            }
        }
        Err(error) => {
            println!("⚠️  Could not fetch transaction history: {}", error);
        }
    }

    Ok(())
}
