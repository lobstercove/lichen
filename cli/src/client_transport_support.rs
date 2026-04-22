use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::client::RpcClient;

#[derive(Serialize)]
struct RpcRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    params: serde_json::Value,
}

#[derive(Deserialize)]
struct RpcResponse {
    #[serde(rename = "jsonrpc")]
    _jsonrpc: String,
    #[serde(rename = "id")]
    _id: u64,
    result: Option<serde_json::Value>,
    error: Option<RpcError>,
}

#[derive(Deserialize)]
struct RpcError {
    code: i32,
    message: String,
}

impl RpcClient {
    pub(crate) async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let request = RpcRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: method.to_string(),
            params,
        };

        let response = self
            .client
            .post(&self.url)
            .json(&request)
            .send()
            .await
            .context("Failed to send RPC request")?;

        let rpc_response: RpcResponse = response
            .json()
            .await
            .context("Failed to parse RPC response")?;

        if let Some(error) = rpc_response.error {
            anyhow::bail!("RPC error {}: {}", error.code, error.message);
        }

        rpc_response
            .result
            .context("Missing result in RPC response")
    }

    pub(crate) async fn submit_wire_transaction(&self, tx_bytes: Vec<u8>) -> Result<String> {
        let params = json!([encode_base64(&tx_bytes)]);
        let result = self.call("sendTransaction", params).await?;

        result
            .as_str()
            .context("Invalid transaction response")
            .map(str::to_string)
    }
}

pub(crate) fn encode_base64(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(data)
}
