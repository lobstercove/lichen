use anyhow::Result;
use lichen_core::Pubkey;

use crate::client::RpcClient;

pub(super) async fn wait_for_confirmation(
    client: &RpcClient,
    signature: &str,
    attempts: usize,
) -> Result<bool> {
    for _ in 0..attempts {
        if let Ok(tx) = client.get_transaction(signature).await {
            if let Some(error) = tx.get("error").and_then(|value| value.as_str()) {
                if !error.is_empty() {
                    anyhow::bail!("Transaction failed on-chain: {}", error);
                }
            }

            if let Some(status) = tx.get("status").and_then(|value| value.as_str()) {
                if matches!(status, "confirmed" | "finalized" | "success") {
                    return Ok(true);
                }
                if matches!(status, "failed" | "error" | "rejected") {
                    anyhow::bail!("Transaction failed on-chain: {}", status);
                }
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    Ok(false)
}

pub(super) async fn wait_for_executable_account(
    client: &RpcClient,
    address: &Pubkey,
    attempts: usize,
) -> bool {
    let address_b58 = address.to_base58();
    for _ in 0..attempts {
        if let Ok(account) = client.get_account_info(&address_b58).await {
            if account.exists && account.is_executable {
                return true;
            }
        }

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    false
}
