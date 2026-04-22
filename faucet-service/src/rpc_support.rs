use serde_json::{json, Value};

use super::models::{FaucetState, TreasuryInfo};

pub(super) async fn fetch_treasury_info(state: &FaucetState) -> Result<TreasuryInfo, String> {
    let value = rpc_call(state, "getTreasuryInfo", json!([])).await?;
    serde_json::from_value(value).map_err(|err| format!("invalid treasury response: {}", err))
}

pub(super) async fn rpc_call(
    state: &FaucetState,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });

    let response = state
        .http
        .post(&state.config.rpc_url)
        .json(&payload)
        .send()
        .await
        .map_err(|err| format!("rpc request failed: {}", err))?;

    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|err| format!("invalid rpc response: {}", err))?;

    if !status.is_success() {
        return Err(format!("rpc http error {}", status));
    }

    if let Some(error) = body.get("error") {
        let message = error
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown rpc error");
        return Err(message.to_string());
    }

    body.get("result")
        .cloned()
        .ok_or_else(|| "rpc response missing result".to_string())
}
