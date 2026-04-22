use lichen_core::Pubkey;

use crate::cli_args::ContractTemplate;
use crate::deploy_readiness_support::DeployReadiness;
use crate::symbol_registration_support::SymbolRegistrationStatus;

pub(super) fn print_deploy_success(contract_addr: &Pubkey) {
    println!("✅ Contract deployed and verified on-chain!");
    println!("🔗 Address: {}", contract_addr.to_base58());
}

pub(super) fn report_deploy_readiness(
    readiness: DeployReadiness,
    signature: &str,
    contract_addr: &Pubkey,
) -> bool {
    match readiness {
        DeployReadiness::Ready => true,
        DeployReadiness::ConfirmationTimedOut => {
            println!("⚠️  Transaction not confirmed after 15 seconds.");
            println!("   The transaction may still be processing. Check:");
            println!("   lichen balance --keypair <keypair>");
            println!(
                "   Explorer: https://explorer.lichen.network/address/{}",
                contract_addr.to_base58()
            );
            false
        }
        DeployReadiness::FailedOnChain { error } => {
            println!("❌ Deploy transaction FAILED on-chain: {}", error);
            println!("   The deploy fee premium (25 LICN) is refunded.");
            println!("   Only the base fee (0.001 LICN) is kept.");
            println!("   Check your balance: lichen balance --keypair <keypair>");
            false
        }
        DeployReadiness::StatusUnknown { error } => {
            println!("⚠️  Could not verify deploy transaction status: {}", error);
            println!("   The transaction may have succeeded. Check the explorer:");
            println!(
                "   Explorer: https://explorer.lichen.network/contract/{}",
                contract_addr.to_base58()
            );
            println!("   Signature: {}", signature);
            println!("   If the contract exists, no action is needed.");
            println!("   If it does not exist, the 25 LICN premium is refunded.");
            println!("   Check your balance: lichen balance --keypair <keypair>");
            false
        }
        DeployReadiness::ContractNotVisible => {
            println!("⚠️  Transaction confirmed but contract not found at expected address.");
            println!("   Signature: {}", signature);
            println!("   Expected:  {}", contract_addr.to_base58());
            println!("   This may indicate an on-chain processing error.");
            false
        }
    }
}

pub(super) fn print_deploy_symbol_registration_status(
    status: SymbolRegistrationStatus,
    symbol: &str,
    contract_addr: &Pubkey,
    name: Option<&str>,
    template: Option<&ContractTemplate>,
    decimals: Option<u8>,
) {
    match status {
        SymbolRegistrationStatus::Visible => {
            println!("🏷️  Symbol '{}' registered in symbol registry", symbol);
        }
        status => {
            println!(
                "⚠️  Symbol '{}' not found in registry — auto-registering...",
                symbol
            );
            match status {
                SymbolRegistrationStatus::FallbackConfirmedVisible { signature } => {
                    println!(
                        "🏷️  Symbol '{}' registered via fallback (sig: {})",
                        symbol, signature
                    );
                }
                SymbolRegistrationStatus::FallbackConfirmedNotVisible { signature }
                | SymbolRegistrationStatus::FallbackPending { signature } => {
                    println!(
                        "⚠️  Symbol '{}' fallback registration sent (sig: {}) — verify with: lichen contract info {}",
                        symbol,
                        signature,
                        contract_addr.to_base58(),
                    );
                }
                SymbolRegistrationStatus::FallbackFailedOnChain { error }
                | SymbolRegistrationStatus::FallbackSubmissionFailed { error } => {
                    println!(
                        "⚠️  Auto-register failed: {}. Register manually:\n   {}",
                        error,
                        build_deploy_symbol_registration_command(
                            contract_addr,
                            symbol,
                            name,
                            template,
                            decimals,
                        )
                    );
                }
                SymbolRegistrationStatus::Visible => unreachable!(),
            }
        }
    }
}

fn build_deploy_symbol_registration_command(
    contract_addr: &Pubkey,
    symbol: &str,
    name: Option<&str>,
    template: Option<&ContractTemplate>,
    decimals: Option<u8>,
) -> String {
    let mut command = format!(
        "lichen contract register --address {} --symbol {}",
        contract_addr.to_base58(),
        symbol
    );

    if let Some(value) = name {
        command.push_str(&format!(" --name \"{}\"", value));
    }
    if let Some(value) = template {
        command.push_str(&format!(" --template {}", value));
    }
    if let Some(value) = decimals {
        command.push_str(&format!(" --decimals {}", value));
    }

    command
}
