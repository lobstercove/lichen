use base64::Engine;
use serde_json::{json, Value};

use super::decode_solana_pubkey;

#[derive(Debug)]
pub(crate) struct SignatureStatus {
    pub(crate) confirmations: Option<u64>,
    pub(crate) confirmation_status: Option<String>,
}

pub(crate) async fn solana_get_signatures_for_address(
    client: &reqwest::Client,
    url: &str,
    address: &str,
) -> Result<Vec<String>, String> {
    let params = json!([address, { "limit": 10 }]);
    let result = solana_rpc_call(client, url, "getSignaturesForAddress", params).await?;
    let mut signatures = Vec::new();
    if let Some(array) = result.as_array() {
        for item in array {
            if let Some(signature) = item.get("signature").and_then(|value| value.as_str()) {
                signatures.push(signature.to_string());
            }
        }
    }
    Ok(signatures)
}

pub(crate) async fn solana_get_signature_status(
    client: &reqwest::Client,
    url: &str,
    signature: &str,
) -> Result<SignatureStatus, String> {
    let params = json!([[signature]]);
    let result = solana_rpc_call(client, url, "getSignatureStatuses", params).await?;
    let value = result
        .get("value")
        .and_then(|value| value.as_array())
        .and_then(|values| values.first())
        .and_then(|value| value.as_object());
    let confirmations = value
        .and_then(|value| value.get("confirmations"))
        .and_then(|value| value.as_u64());
    let confirmation_status = value
        .and_then(|value| value.get("confirmation_status"))
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());
    Ok(SignatureStatus {
        confirmations,
        confirmation_status,
    })
}

pub(crate) async fn solana_get_balance(
    client: &reqwest::Client,
    url: &str,
    address: &str,
) -> Result<u64, String> {
    let params = json!([address]);
    let result = solana_rpc_call(client, url, "getBalance", params).await?;
    result
        .get("value")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| "balance missing".to_string())
}

pub(crate) async fn solana_get_token_balance(
    client: &reqwest::Client,
    url: &str,
    address: &str,
) -> Result<u64, String> {
    let params = json!([address]);
    let result = solana_rpc_call(client, url, "getTokenAccountBalance", params).await?;
    let amount = result
        .get("value")
        .and_then(|value| value.get("amount"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| "token amount missing".to_string())?;
    amount
        .parse::<u64>()
        .map_err(|_| "invalid token amount".to_string())
}

pub(crate) async fn solana_get_account_exists(
    client: &reqwest::Client,
    url: &str,
    address: &str,
) -> Result<bool, String> {
    let params = json!([address, { "encoding": "base64" }]);
    let result = solana_rpc_call(client, url, "getAccountInfo", params).await?;
    let value = result.get("value").cloned().unwrap_or(Value::Null);
    Ok(!value.is_null())
}

pub(crate) async fn solana_rpc_call(
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

pub(crate) async fn solana_get_signature_confirmed(
    client: &reqwest::Client,
    url: &str,
    signature: &str,
) -> Result<Option<bool>, String> {
    let params = json!([[signature]]);
    let result = solana_rpc_call(client, url, "getSignatureStatuses", params).await?;
    let value = result
        .get("value")
        .and_then(|value| value.as_array())
        .and_then(|values| values.first())
        .and_then(|value| value.as_object());
    if value.is_none() {
        return Ok(None);
    }
    let confirmed = value
        .and_then(|value| value.get("confirmation_status"))
        .and_then(|value| value.as_str())
        .map(|status| status == "finalized")
        .unwrap_or(false);
    Ok(Some(confirmed))
}

pub(crate) async fn solana_get_latest_blockhash(
    client: &reqwest::Client,
    url: &str,
) -> Result<[u8; 32], String> {
    let params = json!([]);
    let result = solana_rpc_call(client, url, "getLatestBlockhash", params).await?;
    let value = result
        .get("value")
        .and_then(|field| field.get("blockhash"))
        .and_then(|field| field.as_str())
        .ok_or_else(|| "missing blockhash".to_string())?;
    decode_solana_pubkey(value)
}

pub(crate) async fn solana_send_transaction(
    client: &reqwest::Client,
    url: &str,
    tx_bytes: &[u8],
) -> Result<String, String> {
    let tx_base64 = base64::engine::general_purpose::STANDARD.encode(tx_bytes);
    let params = json!([tx_base64, { "encoding": "base64" }]);
    let result = solana_rpc_call(client, url, "sendTransaction", params).await?;
    result
        .as_str()
        .map(|value| value.to_string())
        .ok_or_else(|| "missing tx signature".to_string())
}
