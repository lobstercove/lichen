use anyhow::Result;

use crate::client::RpcClient;
use crate::status_output_support::print_chain_metrics;

pub(super) async fn handle_chain_metrics(client: &RpcClient) -> Result<()> {
    println!("🦞 Chain Metrics");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    match client.get_metrics().await {
        Ok(metrics) => {
            print_chain_metrics(&metrics);
        }
        Err(error) => {
            println!("⚠️  Could not fetch metrics: {}", error);
        }
    }

    Ok(())
}