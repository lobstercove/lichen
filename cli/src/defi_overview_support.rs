use anyhow::Result;

use crate::client::RpcClient;
use crate::defi_output_support::print_defi_overview_view;
use crate::output_support::print_json;

pub(super) async fn handle_defi_overview(client: &RpcClient, json_output: bool) -> Result<()> {
    let labels = [
        "SporeSwap DEX",
        "AMM Pools",
        "ThallLend",
        "SporePay",
        "LichenSwap",
    ];
    let methods = [
        "getDexCoreStats",
        "getDexAmmStats",
        "getThallLendStats",
        "getSporePayStats",
        "getLichenSwapStats",
    ];

    if json_output {
        let mut all = serde_json::Map::new();
        for (method, label) in methods.iter().zip(labels.iter()) {
            match client.get_defi_stats(method).await {
                Ok(stats) => {
                    all.insert(label.to_string(), stats);
                }
                Err(_) => {
                    all.insert(label.to_string(), serde_json::json!(null));
                }
            }
        }
        print_json(&serde_json::Value::Object(all));
    } else {
        let mut results = Vec::with_capacity(methods.len());
        for (method, label) in methods.iter().zip(labels.iter()) {
            match client.get_defi_stats(method).await {
                Ok(stats) => results.push((label.to_string(), Some(stats))),
                Err(_) => results.push((label.to_string(), None)),
            }
        }
        print_defi_overview_view(&labels, &results);
    }

    Ok(())
}
