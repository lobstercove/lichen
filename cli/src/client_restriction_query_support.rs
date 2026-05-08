use anyhow::Result;

use crate::client::RpcClient;

impl RpcClient {
    pub async fn restriction_rpc(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.call(method, params).await
    }
}
