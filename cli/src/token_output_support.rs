use lichen_core::Pubkey;
use std::path::Path;

use crate::deploy_readiness_support::DeployReadiness;

pub(super) struct TokenDeployPreamble<'a> {
    pub(super) name: &'a str,
    pub(super) symbol: &'a str,
    pub(super) wasm: &'a Path,
    pub(super) wasm_len: usize,
    pub(super) contract_addr: &'a Pubkey,
    pub(super) creator: &'a Pubkey,
    pub(super) decimals: u8,
    pub(super) initial_supply: Option<u64>,
}

pub(super) fn print_token_deploy_preamble(preamble: TokenDeployPreamble<'_>) {
    println!(
        "🪙 Deploying token: {} ({})",
        preamble.name, preamble.symbol
    );
    println!(
        "📦 WASM: {} ({} KB)",
        preamble.wasm.display(),
        preamble.wasm_len / 1024
    );
    println!(
        "📍 Contract address: {}",
        preamble.contract_addr.to_base58()
    );
    println!("👤 Creator: {}", preamble.creator.to_base58());
    println!("🔢 Decimals: {}", preamble.decimals);
    if let Some(supply) = preamble.initial_supply {
        println!("💰 Initial supply: {} {}", supply, preamble.symbol);
    }
    println!("💰 Deploy fee: 25.001 LICN (25 LICN deploy + 0.001 LICN base fee)");
    println!();
}

pub(super) fn report_token_deploy_readiness(
    readiness: DeployReadiness,
    contract_addr: &Pubkey,
) -> bool {
    match readiness {
        DeployReadiness::Ready => true,
        DeployReadiness::ConfirmationTimedOut => {
            println!("⚠️  Deploy transaction not confirmed after 15 seconds.");
            println!(
                "   Check the explorer or rerun `lichen token info {}` later.",
                contract_addr.to_base58()
            );
            false
        }
        DeployReadiness::FailedOnChain { error } => {
            println!("❌ Token deploy failed on-chain: {}", error);
            println!("   The 25 LICN deploy premium is refunded; only the base fee is kept.");
            false
        }
        DeployReadiness::StatusUnknown { error } => {
            println!("⚠️  Could not verify deploy transaction status: {}", error);
            false
        }
        DeployReadiness::ContractNotVisible => {
            println!(
                "⚠️  Deploy transaction confirmed but the contract is not yet visible on-chain."
            );
            println!("   Address: {}", contract_addr.to_base58());
            false
        }
    }
}

pub(super) fn print_token_deploy_success(
    contract_addr: &Pubkey,
    symbol: &str,
    initial_supply: Option<u64>,
    symbol_registered: bool,
) {
    println!("✅ Token deployed");
    if symbol_registered {
        println!("🏷️  Symbol: {}", symbol);
    } else {
        println!(
            "🏷️  Symbol: {} (registration pending/manual verification recommended)",
            symbol
        );
    }
    println!("🔗 Address: {}", contract_addr.to_base58());
    if initial_supply.unwrap_or(0) == 0 {
        println!(
            "💡 Next: `lichen token mint {} 1000` to mint an initial supply.",
            contract_addr.to_base58()
        );
    }
}
