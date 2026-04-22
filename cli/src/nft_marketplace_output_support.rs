use crate::output_support::to_licn;

pub(super) fn print_marketplace_listings(listings: &serde_json::Value) {
    println!("🏪 NFT Marketplace Listings");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    if let Some(arr) = listings.as_array() {
        if arr.is_empty() {
            println!("No active listings");
        } else {
            for (index, listing) in arr.iter().enumerate() {
                let name = listing
                    .get("name")
                    .and_then(|value| value.as_str())
                    .unwrap_or("Untitled");
                let price = listing
                    .get("price")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0);
                let seller = listing
                    .get("seller")
                    .and_then(|value| value.as_str())
                    .unwrap_or("-");
                println!(
                    "#{} {} — {} LICN (Seller: {})",
                    index + 1,
                    name,
                    to_licn(price),
                    seller
                );
            }
            println!("\nShowing {} listings", arr.len());
        }
    }
}