use anyhow::Result;

use crate::client::RpcClient;
use crate::output_support::to_licn;

pub(super) async fn handle_block(client: &RpcClient, slot: u64) -> Result<()> {
    let block = client.get_block(slot).await?;

    println!("🧊 Block #{}", slot);
    println!("🔗 Hash: {}", block.hash);
    println!("⬅️  Parent: {}", block.parent_hash);
    println!("🌳 State Root: {}", block.state_root);
    println!("🦞 Validator: {}", block.validator);
    println!("⏰ Timestamp: {}", block.timestamp);
    println!("📦 Transactions: {}", block.transaction_count);

    Ok(())
}

pub(super) async fn handle_current_slot(client: &RpcClient) -> Result<()> {
    let slot = client.get_slot().await?;
    println!("🦞 Current slot: {}", slot);

    Ok(())
}

pub(super) async fn handle_recent_blockhash(client: &RpcClient) -> Result<()> {
    let hash = client.get_recent_blockhash().await?;
    println!("🦞 Recent blockhash: {}", hash);

    Ok(())
}

pub(super) async fn handle_latest_block(client: &RpcClient) -> Result<()> {
    let block = client.get_latest_block().await?;

    println!("🧊 Latest Block #{}", block.slot);
    println!("🔗 Hash: {}", block.hash);
    println!("⬅️  Parent: {}", block.parent_hash);
    println!("🌳 State Root: {}", block.state_root);
    println!("🦞 Validator: {}", block.validator);
    println!("⏰ Timestamp: {}", block.timestamp);
    println!("📦 Transactions: {}", block.transaction_count);

    Ok(())
}

pub(super) async fn handle_total_burned(client: &RpcClient) -> Result<()> {
    let burned = client.get_total_burned().await?;
    println!("🔥 Total LICN Burned");
    println!(
        "💰 {} LICN ({} spores)",
        to_licn(burned.spores),
        burned.spores
    );
    println!();
    println!("Deflationary mechanism: 50% of all transaction fees are burned forever! 🦞⚡");

    Ok(())
}
