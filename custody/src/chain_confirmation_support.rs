use super::*;

/// Check if a Solana transaction is confirmed with enough confirmations.
/// AUDIT-FIX 1.18: Properly check confirmation_status and confirmation count.
pub(super) async fn check_solana_tx_confirmed(
    client: &reqwest::Client,
    url: &str,
    tx_hash: &str,
    required_confirmations: u64,
) -> Result<bool, String> {
    let statuses = solana_rpc_call(
        client,
        url,
        "getSignatureStatuses",
        json!([[tx_hash], {"searchTransactionHistory": true}]),
    )
    .await?;

    let status = statuses
        .get("value")
        .and_then(|value| value.as_array())
        .and_then(|values| values.first())
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    if status.is_null() {
        return Ok(false);
    }

    let confirmation_status = status
        .get("confirmation_status")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");

    if confirmation_status == "finalized" {
        return Ok(true);
    }

    let confirmations = status
        .get("confirmations")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);

    Ok(confirmations >= required_confirmations)
}

/// Check if an EVM transaction is confirmed with enough confirmations.
pub(super) async fn check_evm_tx_confirmed(
    client: &reqwest::Client,
    url: &str,
    tx_hash: &str,
    required_confirmations: u64,
) -> Result<bool, String> {
    let receipt = evm_rpc_call(client, url, "eth_getTransactionReceipt", json!([tx_hash])).await?;
    if receipt.is_null() {
        return Ok(false);
    }

    let block_number = receipt
        .get("blockNumber")
        .and_then(|value| value.as_str())
        .map(|value| parse_hex_u64(value).unwrap_or(0))
        .unwrap_or(0);

    if block_number == 0 {
        return Ok(false);
    }

    let current_block = evm_rpc_call(client, url, "eth_blockNumber", json!([])).await?;
    let current = current_block
        .as_str()
        .map(|value| parse_hex_u64(value).unwrap_or(0))
        .unwrap_or(0);

    Ok(current.saturating_sub(block_number) >= required_confirmations)
}
