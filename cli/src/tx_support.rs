use anyhow::Result;

use crate::client::RpcClient;
use crate::output_support::{print_json, to_licn};

pub(super) async fn handle_transaction_lookup(
    client: &RpcClient,
    signature: &str,
    json_output: bool,
) -> Result<()> {
    match client.get_transaction(signature).await {
        Ok(transaction) => {
            if json_output {
                print_json(&transaction);
            } else {
                println!("📝 Transaction {}", signature);
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                if let Some(slot) = transaction.get("slot").and_then(|value| value.as_u64()) {
                    println!("Slot:   {}", slot);
                }
                if let Some(status) = transaction.get("status").and_then(|value| value.as_str()) {
                    println!("Status: {}", status);
                }
                if let Some(fee) = transaction.get("fee").and_then(|value| value.as_u64()) {
                    println!("Fee:    {} LICN", to_licn(fee));
                }
                if let Some(from) = transaction.get("from").and_then(|value| value.as_str()) {
                    println!("From:   {}", from);
                }
                if let Some(to) = transaction.get("to").and_then(|value| value.as_str()) {
                    println!("To:     {}", to);
                }
                if let Some(amount) = transaction.get("amount").and_then(|value| value.as_u64()) {
                    if amount > 0 {
                        println!("Amount: {} LICN", to_licn(amount));
                    }
                }
                if let Some(error) = transaction.get("error").and_then(|value| value.as_str()) {
                    if !error.is_empty() {
                        println!("Error:  {}", error);
                    }
                }
            }
        }
        Err(error) => {
            if json_output {
                print_json(&serde_json::json!({
                    "error": error.to_string(),
                    "signature": signature,
                }));
            } else {
                println!("Transaction not found: {}", error);
            }
        }
    }

    Ok(())
}
