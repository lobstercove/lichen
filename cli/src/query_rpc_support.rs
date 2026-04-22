use anyhow::Result;
use serde_json::{json, Value};

use crate::config::CliConfig;

pub(crate) async fn perform_query_request(
    config: &CliConfig,
    id: u64,
    method: &str,
    params: Value,
) -> Result<Value> {
    let client = reqwest::Client::new();
    let request = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });

    let response = client
        .post(&config.rpc_url)
        .json(&request)
        .send()
        .await?
        .json::<Value>()
        .await?;

    Ok(response)
}
