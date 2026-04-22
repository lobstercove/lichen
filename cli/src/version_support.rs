use anyhow::Result;

use crate::output_support::print_json;

pub(super) fn handle_version(rpc_url: &str, json_output: bool) -> Result<()> {
    let version_info = serde_json::json!({
        "cli_version": env!("CARGO_PKG_VERSION"),
        "binary": "lichen",
        "chain": "Lichen",
        "consensus": "Tendermint BFT",
        "signing": "ML-DSA-65",
        "contracts": "WASM (Rust → wasm32-unknown-unknown)",
        "zk": "Plonky3 STARK",
        "native_token": "LICN",
        "spores_per_licn": 1_000_000_000u64,
        "rpc_url": rpc_url,
        "system_program": "0000000000000000000000000000000000000000000000000000000000000000",
        "contract_program": "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        "instruction_types": 28,
        "fee_structure": {
            "base_fee": "0.001 LICN (1,000,000 spores)",
            "deploy_premium": "25 LICN",
            "upgrade_premium": "10 LICN",
            "nft_mint_premium": "0.5 LICN",
            "fee_split": "40% burn, 30% block producer, 10% voters, 10% treasury, 10% community"
        },
        "wasm_host_functions": 16,
        "rpc_endpoints": {
            "mainnet": "https://rpc.lichen.network",
            "mainnet_ws": "wss://rpc.lichen.network/ws",
            "testnet": "https://testnet-rpc.lichen.network"
        }
    });

    if json_output {
        print_json(&version_info);
    } else {
        println!("🦞 Lichen CLI v{}", env!("CARGO_PKG_VERSION"));
        println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        println!();
        println!("Chain:       Lichen (custom L1)");
        println!("Consensus:   Tendermint BFT (~800ms blocks)");
        println!("Signing:     ML-DSA-65");
        println!("Contracts:   WASM (Rust → wasm32-unknown-unknown)");
        println!("ZK Proofs:   Plonky3 STARK");
        println!("Token:       LICN (1 LICN = 1,000,000,000 spores)");
        println!();
        println!("RPC (current): {}", rpc_url);
        println!("Mainnet RPC:   https://rpc.lichen.network");
        println!("Testnet RPC:   https://testnet-rpc.lichen.network");
        println!("Explorer:      https://explorer.lichen.network");
        println!("Docs:          https://developers.lichen.network");
        println!();
        println!("Fees:");
        println!("  Base:    0.001 LICN    Deploy: 25 LICN");
        println!("  Upgrade: 10 LICN       NFT Mint: 0.5 LICN");
        println!("  Split: 40% burn / 30% producer / 10% voters / 10% treasury / 10% community");
        println!();
        println!("System program:   [0x00; 32]");
        println!("Contract program: [0xFF; 32]");
        println!();
        println!("28 instruction types | 16 WASM host functions");
        println!("Use --output json for machine-readable output.");
    }

    Ok(())
}
