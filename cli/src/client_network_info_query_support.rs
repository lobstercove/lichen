use anyhow::{Context, Result};
use serde_json::json;

use crate::client::{NetworkInfo, RpcClient};

impl RpcClient {
    /// Get network information
    pub async fn get_network_info(&self) -> Result<NetworkInfo> {
        let params = json!([]);
        let result = self.call("getNetworkInfo", params).await?;

        let info: NetworkInfo =
            serde_json::from_value(result).context("Failed to parse network info")?;

        Ok(info)
    }
}
