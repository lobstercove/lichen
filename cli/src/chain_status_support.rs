use anyhow::Result;

use crate::client::RpcClient;
use crate::status_output_support::print_chain_status;

pub(super) async fn handle_chain_status(client: &RpcClient) -> Result<()> {
    println!("🦞 Lichen Status");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    match client.get_chain_status().await {
        Ok(status) => {
            print_chain_status(&status);
        }
        Err(error) => {
            println!("⚠️  Could not fetch chain status: {}", error);
        }
    }

    Ok(())
}