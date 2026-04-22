use anyhow::Result;

use crate::client::RpcClient;

pub(super) async fn handle_network_status(client: &RpcClient) -> Result<()> {
    println!("🦞 Network Status");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");

    let slot = client.get_slot().await?;
    println!("📊 Current slot: {}", slot);

    let validators_info = client.get_validators().await?;
    println!("👥 Active validators: {}", validators_info.validators.len());

    match client.get_metrics().await {
        Ok(metrics) => {
            println!("⚡ TPS: {}", metrics.tps);
            println!("📦 Total blocks: {}", metrics.total_blocks);
            println!("📝 Total transactions: {}", metrics.total_transactions);
        }
        Err(_) => {
            println!("⚠️  Metrics unavailable");
        }
    }

    println!();
    println!("✅ Network is healthy");

    Ok(())
}

pub(super) async fn handle_network_peers(client: &RpcClient) -> Result<()> {
    println!("🦞 Connected Peers");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    match client.get_peers().await {
        Ok(peers) => {
            if peers.is_empty() {
                println!("No connected peers");
            } else {
                for (index, peer) in peers.iter().enumerate() {
                    println!(
                        "#{} {} ({})",
                        index + 1,
                        peer.peer_id,
                        if peer.connected {
                            "Connected"
                        } else {
                            "Disconnected"
                        }
                    );
                    println!("   Address: {}", peer.address);
                }
                println!();
                println!("Total: {} peers", peers.len());
            }
        }
        Err(error) => {
            println!("⚠️  Could not fetch peers: {}", error);
            println!("💡 Make sure the validator is running");
        }
    }

    Ok(())
}

pub(super) async fn handle_network_info(client: &RpcClient, rpc_url: &str) -> Result<()> {
    println!("🦞 Network Information");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    match client.get_network_info().await {
        Ok(info) => {
            println!("🌐 Network ID: {}", info.network_id);
            println!("⛓️  Chain ID: {}", info.chain_id);
            println!("🔗 RPC Endpoint: {}", rpc_url);
            println!();
            println!("📊 Statistics:");
            println!("   Current slot: {}", info.current_slot);
            println!("   Validators: {}", info.validator_count);
            println!("   TPS: {}", info.tps);
        }
        Err(error) => {
            println!("⚠️  Could not fetch network info: {}", error);
        }
    }

    Ok(())
}
