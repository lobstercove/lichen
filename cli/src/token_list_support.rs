use anyhow::Result;

use crate::client::RpcClient;

pub(super) async fn handle_token_list(client: &RpcClient) -> Result<()> {
    println!("🪙 Deployed Token Contracts");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    match client.get_all_symbol_registry().await {
        Ok(entries) => {
            let token_entries = entries
                .as_array()
                .map(|entries| {
                    entries
                        .iter()
                        .filter(|entry| {
                            matches!(
                                entry.get("template").and_then(|value| value.as_str()),
                                Some("token" | "wrapped" | "mt20" | "fungible_token")
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            if token_entries.is_empty() {
                println!("No token contracts deployed yet");
            } else {
                for (index, entry) in token_entries.iter().enumerate() {
                    let symbol = entry
                        .get("symbol")
                        .and_then(|value| value.as_str())
                        .unwrap_or("?");
                    let name = entry
                        .get("name")
                        .and_then(|value| value.as_str())
                        .unwrap_or(symbol);
                    let program = entry
                        .get("program")
                        .and_then(|value| value.as_str())
                        .unwrap_or("<unknown>");
                    let template = entry
                        .get("template")
                        .and_then(|value| value.as_str())
                        .unwrap_or("token");

                    println!("#{} {} ({})", index + 1, name, symbol);
                    println!("   Address:  {}", program);
                    println!("   Template: {}", template);
                    println!();
                }
                println!("Total: {} token contracts", token_entries.len());
                println!();
                println!("💡 Get token details: lichen token info <symbol-or-address>");
            }
        }
        Err(error) => {
            println!("⚠️  Could not fetch token registry entries: {}", error);
        }
    }

    Ok(())
}
