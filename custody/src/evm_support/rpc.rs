use serde_json::{json, Value};

use crate::{parse_hex_u128, parse_hex_u64};

pub(crate) async fn evm_get_balance(
    client: &reqwest::Client,
    url: &str,
    address: &str,
) -> Result<u128, String> {
    let params = json!([address, "latest"]);
    let result = evm_rpc_call(client, url, "eth_getBalance", params).await?;
    let value = result.as_str().unwrap_or("0x0");
    parse_hex_u128(value)
}

pub(crate) async fn evm_get_block_number(
    client: &reqwest::Client,
    url: &str,
) -> Result<u64, String> {
    let result = evm_rpc_call(client, url, "eth_blockNumber", json!([])).await?;
    let value = result.as_str().unwrap_or("0x0");
    parse_hex_u64(value)
}

pub(crate) async fn evm_get_transaction_count(
    client: &reqwest::Client,
    url: &str,
    address: &str,
) -> Result<u64, String> {
    let params = json!([address, "pending"]);
    let result = evm_rpc_call(client, url, "eth_getTransactionCount", params).await?;
    let value = result.as_str().unwrap_or("0x0");
    parse_hex_u64(value)
}

pub(crate) async fn evm_get_gas_price(client: &reqwest::Client, url: &str) -> Result<u128, String> {
    let result = evm_rpc_call(client, url, "eth_gasPrice", json!([])).await?;
    let value = result.as_str().unwrap_or("0x0");
    parse_hex_u128(value)
}

pub(crate) async fn evm_estimate_gas(
    client: &reqwest::Client,
    url: &str,
    from: &str,
    to: &str,
    value: u128,
    data: Option<&[u8]>,
    fallback: u128,
) -> u128 {
    let mut params = serde_json::json!({
        "from": from,
        "to": to,
        "value": format!("0x{:x}", value),
    });
    if let Some(data) = data {
        params["data"] = serde_json::Value::String(format!("0x{}", hex::encode(data)));
    }
    match evm_rpc_call(client, url, "eth_estimateGas", json!([params])).await {
        Ok(result) => {
            let hex_str = result.as_str().unwrap_or("0x0");
            match parse_hex_u128(hex_str) {
                Ok(estimate) if estimate > 0 => {
                    let buffered = estimate.saturating_add(estimate / 5);
                    tracing::debug!(
                        "eth_estimateGas: {} -> buffered to {} (fallback was {})",
                        estimate,
                        buffered,
                        fallback
                    );
                    buffered
                }
                _ => {
                    tracing::debug!("eth_estimateGas returned 0, using fallback {}", fallback);
                    fallback
                }
            }
        }
        Err(error) => {
            tracing::debug!(
                "eth_estimateGas failed ({}), using fallback {}",
                error,
                fallback
            );
            fallback
        }
    }
}

pub(crate) async fn evm_get_chain_id(client: &reqwest::Client, url: &str) -> Result<u64, String> {
    let result = evm_rpc_call(client, url, "eth_chainId", json!([])).await?;
    let value = result.as_str().unwrap_or("0x0");
    parse_hex_u64(value)
}

pub(crate) async fn evm_get_transaction_receipt(
    client: &reqwest::Client,
    url: &str,
    tx_hash: &str,
) -> Result<Option<Value>, String> {
    let result = evm_rpc_call(client, url, "eth_getTransactionReceipt", json!([tx_hash])).await?;
    if result.is_null() {
        return Ok(None);
    }
    Ok(Some(result))
}

pub(crate) async fn evm_get_transfer_logs(
    client: &reqwest::Client,
    url: &str,
    contract: &str,
    from_block: u64,
    to_block: u64,
) -> Result<Vec<Value>, String> {
    let params = json!([
        {
            "fromBlock": format!("0x{:x}", from_block),
            "toBlock": format!("0x{:x}", to_block),
            "address": contract,
            "topics": ["0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"],
        }
    ]);
    let result = evm_rpc_call(client, url, "eth_getLogs", params).await?;
    Ok(result.as_array().cloned().unwrap_or_default())
}

pub(crate) async fn evm_rpc_call(
    client: &reqwest::Client,
    url: &str,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let response = client
        .post(url)
        .json(&payload)
        .send()
        .await
        .map_err(|error| format!("rpc send: {}", error))?;
    let value: Value = response
        .json()
        .await
        .map_err(|error| format!("rpc json: {}", error))?;
    if let Some(error) = value.get("error") {
        return Err(format!("rpc error: {}", error));
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| "rpc result missing".to_string())
}
