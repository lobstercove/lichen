pub(super) fn print_owned_nfts(addr: &str, nfts: &serde_json::Value) {
    println!("🖼️  NFTs owned by {}", addr);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    if let Some(arr) = nfts.as_array() {
        if arr.is_empty() {
            println!("No NFTs found");
        } else {
            for (index, nft) in arr.iter().enumerate() {
                let name = nft
                    .get("name")
                    .and_then(|value| value.as_str())
                    .unwrap_or("Untitled");
                let collection = nft
                    .get("collection")
                    .and_then(|value| value.as_str())
                    .unwrap_or("-");
                let token_id = nft
                    .get("token_id")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0);
                println!(
                    "#{} {} (ID: {}, Collection: {})",
                    index + 1,
                    name,
                    token_id,
                    collection
                );
            }
            println!("\nTotal: {} NFTs", arr.len());
        }
    }
}
