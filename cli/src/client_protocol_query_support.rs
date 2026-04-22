use anyhow::Result;
use serde_json::json;

use crate::client::RpcClient;

impl RpcClient {
    /// Get DeFi protocol stats by method name
    pub async fn get_defi_stats(&self, method: &str) -> Result<serde_json::Value> {
        let params = json!([]);
        self.call(method, params).await
    }
}
