use lichen_core::{Keypair, Pubkey};

use crate::client::{RpcClient, SymbolRegistration};
use crate::contract_poll_support::wait_for_confirmation;

pub(super) enum SymbolRegistrationSubmitOutcome {
    ConfirmedVisible { signature: String },
    ConfirmedNotVisible { signature: String },
    Pending { signature: String },
    FailedOnChain { error: String },
    SubmissionFailed { error: String },
}

pub(super) enum SymbolRegistrationStatus {
    Visible,
    FallbackConfirmedVisible { signature: String },
    FallbackConfirmedNotVisible { signature: String },
    FallbackPending { signature: String },
    FallbackFailedOnChain { error: String },
    FallbackSubmissionFailed { error: String },
}

pub(super) async fn wait_for_symbol_visibility(
    client: &RpcClient,
    symbol: &str,
    attempts: usize,
) -> bool {
    for _ in 0..attempts {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if matches!(client.resolve_symbol(symbol).await, Ok(Some(_))) {
            return true;
        }
    }

    false
}

pub(super) async fn submit_symbol_registration_with_confirmation(
    client: &RpcClient,
    deployer: &Keypair,
    contract_addr: &Pubkey,
    registration: SymbolRegistration<'_>,
    confirmation_attempts: usize,
) -> SymbolRegistrationSubmitOutcome {
    let symbol = registration.symbol;

    match client
        .register_symbol(deployer, contract_addr, registration)
        .await
    {
        Ok(signature) => {
            match wait_for_confirmation(client, &signature, confirmation_attempts).await {
                Ok(true) => {
                    if matches!(client.resolve_symbol(symbol).await, Ok(Some(_))) {
                        SymbolRegistrationSubmitOutcome::ConfirmedVisible { signature }
                    } else {
                        SymbolRegistrationSubmitOutcome::ConfirmedNotVisible { signature }
                    }
                }
                Ok(false) => SymbolRegistrationSubmitOutcome::Pending { signature },
                Err(error) => SymbolRegistrationSubmitOutcome::FailedOnChain {
                    error: error.to_string(),
                },
            }
        }
        Err(error) => SymbolRegistrationSubmitOutcome::SubmissionFailed {
            error: error.to_string(),
        },
    }
}

pub(super) async fn ensure_symbol_registration(
    client: &RpcClient,
    deployer: &Keypair,
    contract_addr: &Pubkey,
    registration: SymbolRegistration<'_>,
    visibility_attempts: usize,
    confirmation_attempts: usize,
) -> SymbolRegistrationStatus {
    if wait_for_symbol_visibility(client, registration.symbol, visibility_attempts).await {
        return SymbolRegistrationStatus::Visible;
    }

    match submit_symbol_registration_with_confirmation(
        client,
        deployer,
        contract_addr,
        registration,
        confirmation_attempts,
    )
    .await
    {
        SymbolRegistrationSubmitOutcome::ConfirmedVisible { signature } => {
            SymbolRegistrationStatus::FallbackConfirmedVisible { signature }
        }
        SymbolRegistrationSubmitOutcome::ConfirmedNotVisible { signature } => {
            SymbolRegistrationStatus::FallbackConfirmedNotVisible { signature }
        }
        SymbolRegistrationSubmitOutcome::Pending { signature } => {
            SymbolRegistrationStatus::FallbackPending { signature }
        }
        SymbolRegistrationSubmitOutcome::FailedOnChain { error } => {
            SymbolRegistrationStatus::FallbackFailedOnChain { error }
        }
        SymbolRegistrationSubmitOutcome::SubmissionFailed { error } => {
            SymbolRegistrationStatus::FallbackSubmissionFailed { error }
        }
    }
}
