use lichen_core::{Keypair, Pubkey};

use crate::client::{RpcClient, SymbolRegistration};
use crate::symbol_registration_support::{ensure_symbol_registration, SymbolRegistrationStatus};

pub(super) struct TokenSymbolRegistration<'a> {
    pub(super) name: &'a str,
    pub(super) symbol: &'a str,
    pub(super) decimals: u8,
    pub(super) registry_metadata: Option<serde_json::Value>,
}

pub(super) async fn handle_token_symbol_registration(
    client: &RpcClient,
    deployer: &Keypair,
    contract_addr: &Pubkey,
    registration: TokenSymbolRegistration<'_>,
) -> bool {
    match ensure_symbol_registration(
        client,
        deployer,
        contract_addr,
        SymbolRegistration {
            symbol: registration.symbol,
            name: Some(registration.name),
            template: Some("mt20"),
            decimals: Some(registration.decimals),
            metadata: registration.registry_metadata,
        },
        3,
        10,
    )
    .await
    {
        SymbolRegistrationStatus::Visible => true,
        status => {
            println!(
                "⚠️  Symbol '{}' was not found in the registry after deploy. Sending fallback registration...",
                registration.symbol
            );
            match status {
                SymbolRegistrationStatus::FallbackConfirmedVisible { signature } => {
                    println!(
                        "🏷️  Symbol '{}' registered via fallback (sig: {})",
                        registration.symbol, signature
                    );
                    true
                }
                SymbolRegistrationStatus::FallbackConfirmedNotVisible { signature } => {
                    println!(
                        "⚠️  Sent fallback registration (sig: {}), but the symbol is still not visible yet.",
                        signature
                    );
                    false
                }
                SymbolRegistrationStatus::FallbackPending { signature } => {
                    println!(
                        "⚠️  Fallback symbol registration is still pending (sig: {}).",
                        signature
                    );
                    false
                }
                SymbolRegistrationStatus::FallbackFailedOnChain { error } => {
                    println!(
                        "⚠️  Fallback symbol registration failed on-chain: {}",
                        error
                    );
                    false
                }
                SymbolRegistrationStatus::FallbackSubmissionFailed { error } => {
                    println!(
                        "⚠️  Could not submit fallback symbol registration: {}",
                        error
                    );
                    false
                }
                SymbolRegistrationStatus::Visible => unreachable!(),
            }
        }
    }
}
