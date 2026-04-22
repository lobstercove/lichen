use anyhow::Result;
use serde_json::json;

use crate::client::RpcClient;

impl RpcClient {
    /// Get contract ABI (fetched from on-chain storage via RPC)
    pub async fn get_contract_abi(&self, address: &str) -> Result<serde_json::Value> {
        let params = json!([address]);
        self.call("getContractAbi", params).await
    }
}
