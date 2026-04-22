use anyhow::Result;

use crate::output_support::print_json;

pub(super) fn handle_fees(json_output: bool) -> Result<()> {
    let fees = serde_json::json!({
        "base_fee_spores": 1_000_000u64,
        "base_fee_licn": 0.001,
        "deploy_premium_spores": 25_000_000_000u64,
        "deploy_premium_licn": 25.0,
        "upgrade_premium_spores": 10_000_000_000u64,
        "upgrade_premium_licn": 10.0,
        "nft_mint_premium_spores": 500_000_000u64,
        "nft_mint_premium_licn": 0.5,
        "fee_split": {
            "burn_pct": 40,
            "block_producer_pct": 30,
            "voters_pct": 10,
            "treasury_pct": 10,
            "community_pct": 10
        },
        "reputation_discounts": {
            "500+": "5% off",
            "750+": "7.5% off",
            "1000+": "10% off"
        },
        "notes": [
            "All fees paid in LICN (1 LICN = 1,000,000,000 spores)",
            "Deploy premium refunded on failure (only base fee kept)",
            "40% of fees burned permanently (deflationary)",
            "Reputation discounts apply to base fee only"
        ]
    });

    if json_output {
        print_json(&fees);
    } else {
        println!("💸 Lichen Fee Schedule");
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!();
        println!("Transaction Type          Fee");
        println!("─────────────────────────────────────────────");
        println!("Transfer / Call            0.001 LICN (base)");
        println!("Deploy Contract           25.001 LICN (25 + base)");
        println!("Upgrade Contract          10.001 LICN (10 + base)");
        println!("Mint NFT                   0.501 LICN (0.5 + base)");
        println!();
        println!("Fee Split:");
        println!("  40% burned forever (deflationary)");
        println!("  30% block producer reward");
        println!("  10% stake voters reward");
        println!("  10% treasury");
        println!("  10% community pool");
        println!();
        println!("Reputation Discounts (on base fee):");
        println!("  500+ rep  -> 5% off");
        println!("  750+ rep  -> 7.5% off");
        println!("  1000+ rep -> 10% off");
        println!();
        println!("Note: Deploy premium refunded on failure.");
    }

    Ok(())
}
