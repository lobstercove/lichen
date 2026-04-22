use super::*;

pub(super) async fn licn_get_recent_blockhash(
    client: &reqwest::Client,
    url: &str,
) -> Result<Hash, String> {
    let result = licn_rpc_call(client, url, "getRecentBlockhash", json!([])).await?;
    let hash = result
        .get("blockhash")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing blockhash".to_string())?;
    Hash::from_hex(hash).map_err(|error| format!("blockhash: {}", error))
}

pub(super) async fn licn_send_transaction(
    client: &reqwest::Client,
    url: &str,
    tx_base64: &str,
) -> Result<String, String> {
    let result = licn_rpc_call(client, url, "sendTransaction", json!([tx_base64])).await?;
    result
        .as_str()
        .map(|value| value.to_string())
        .ok_or_else(|| "missing tx signature".to_string())
}

pub(super) async fn licn_rpc_call(
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
