use lichen_core::Pubkey;

use crate::client::RpcClient;
use crate::contract_poll_support::{wait_for_confirmation, wait_for_executable_account};

pub(super) enum DeployReadiness {
    Ready,
    ConfirmationTimedOut,
    FailedOnChain { error: String },
    StatusUnknown { error: String },
    ContractNotVisible,
}

pub(super) async fn wait_for_deploy_readiness(
    client: &RpcClient,
    signature: &str,
    contract_addr: &Pubkey,
) -> DeployReadiness {
    match wait_for_confirmation(client, signature, 15).await {
        Ok(true) => {}
        Ok(false) => return DeployReadiness::ConfirmationTimedOut,
        Err(error) => {
            let error = error.to_string();
            if error.contains("Transaction failed on-chain") {
                return DeployReadiness::FailedOnChain { error };
            }
            return DeployReadiness::StatusUnknown { error };
        }
    }

    if !wait_for_executable_account(client, contract_addr, 10).await {
        return DeployReadiness::ContractNotVisible;
    }

    DeployReadiness::Ready
}
