use anyhow::{Context, Result};
use serde_json::json;

use crate::client::{ChainStatus, RpcClient};

impl RpcClient {
    /// Get comprehensive chain status
    pub async fn get_chain_status(&self) -> Result<ChainStatus> {
        let params = json!([]);
        let result = self.call("getChainStatus", params).await?;

        let status: ChainStatus =
            serde_json::from_value(result).context("Failed to parse chain status")?;

        Ok(status)
    }
}
