use anyhow::Result;

use crate::client::RpcClient;
use crate::nft_collection_output_support::print_nft_collection;
use crate::output_support::print_json;

pub(super) async fn handle_nft_collection(
    client: &RpcClient,
    address: &str,
    json_output: bool,
) -> Result<()> {
    match client.get_nfts_by_collection(address).await {
        Ok(nfts) => {
            if json_output {
                print_json(&nfts);
            } else {
                print_nft_collection(address, &nfts);
            }
        }
        Err(error) => {
            if json_output {
                print_json(&serde_json::json!({"error": error.to_string()}));
            } else {
                println!("Could not fetch collection: {}", error);
            }
        }
    }

    Ok(())
}
