use anyhow::{Context, Result};
use serde_json::json;

use crate::client::{PeerInfo, RpcClient};

impl RpcClient {
    /// Get connected peers
    pub async fn get_peers(&self) -> Result<Vec<PeerInfo>> {
        let params = json!([]);
        let result = self.call("getPeers", params).await?;
        let peers_value = if let Some(arr) = result.as_array() {
            serde_json::Value::Array(arr.clone())
        } else {
            result
                .get("peers")
                .cloned()
                .unwrap_or_else(|| serde_json::Value::Array(vec![]))
        };
        let peers: Vec<PeerInfo> =
            serde_json::from_value(peers_value).context("Failed to parse peers info")?;

        Ok(peers)
    }
}
