pub(super) fn print_nft_collection(address: &str, nfts: &serde_json::Value) {
    println!("🖼️  NFT Collection: {}", address);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    if let Some(arr) = nfts.as_array() {
        for (i, nft) in arr.iter().enumerate() {
            let name = nft
                .get("name")
                .and_then(|value| value.as_str())
                .unwrap_or("Untitled");
            let owner = nft
                .get("owner")
                .and_then(|value| value.as_str())
                .unwrap_or("-");
            println!("#{} {} (Owner: {})", i + 1, name, owner);
        }
        println!("\nTotal: {} NFTs in collection", arr.len());
    }
}