use anyhow::Result;

use crate::client::RpcClient;
use crate::nft_marketplace_output_support::print_marketplace_listings;
use crate::output_support::print_json;

pub(super) async fn handle_nft_marketplace(
    client: &RpcClient,
    limit: usize,
    json_output: bool,
) -> Result<()> {
    match client.get_market_listings(limit).await {
        Ok(listings) => {
            if json_output {
                print_json(&listings);
            } else {
                print_marketplace_listings(&listings);
            }
        }
        Err(error) => {
            if json_output {
                print_json(&serde_json::json!({"error": error.to_string()}));
            } else {
                println!("Could not fetch marketplace: {}", error);
            }
        }
    }

    Ok(())
}
