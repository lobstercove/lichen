use anyhow::Result;

use crate::client::RpcClient;
use crate::output_support::{print_json, to_licn};

pub(super) async fn handle_supply(client: &RpcClient, json_output: bool) -> Result<()> {
    match client.get_metrics().await {
        Ok(metrics) => {
            if json_output {
                print_json(&serde_json::json!({
                    "total_supply_spores": metrics.total_supply,
                    "total_supply_licn": to_licn(metrics.total_supply),
                    "circulating_supply_spores": metrics.circulating_supply,
                    "circulating_supply_licn": to_licn(metrics.circulating_supply),
                    "total_burned_spores": metrics.total_burned,
                    "total_burned_licn": to_licn(metrics.total_burned),
                    "total_staked_spores": metrics.total_staked,
                    "total_staked_licn": to_licn(metrics.total_staked),
                    "burn_percentage": if metrics.total_supply > 0 {
                        (metrics.total_burned as f64 / metrics.total_supply as f64) * 100.0
                    } else {
                        0.0
                    },
                    "staked_percentage": if metrics.total_supply > 0 {
                        (metrics.total_staked as f64 / metrics.total_supply as f64) * 100.0
                    } else {
                        0.0
                    },
                    "total_accounts": metrics.total_accounts,
                    "total_contracts": metrics.total_contracts,
                }));
            } else {
                println!("💰 LICN Supply & Economics");
                println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
                println!();
                println!(
                    "Total Supply:       {:>14.4} LICN",
                    to_licn(metrics.total_supply)
                );
                println!(
                    "Circulating:        {:>14.4} LICN",
                    to_licn(metrics.circulating_supply)
                );
                println!(
                    "Burned:             {:>14.4} LICN ({:.2}%)",
                    to_licn(metrics.total_burned),
                    if metrics.total_supply > 0 {
                        (metrics.total_burned as f64 / metrics.total_supply as f64) * 100.0
                    } else {
                        0.0
                    }
                );
                println!(
                    "Staked:             {:>14.4} LICN ({:.2}%)",
                    to_licn(metrics.total_staked),
                    if metrics.total_supply > 0 {
                        (metrics.total_staked as f64 / metrics.total_supply as f64) * 100.0
                    } else {
                        0.0
                    }
                );
                println!();
                println!(
                    "Accounts: {}   Contracts: {}",
                    metrics.total_accounts, metrics.total_contracts
                );
            }
        }
        Err(error) => println!("Could not fetch supply info: {}", error),
    }

    Ok(())
}
