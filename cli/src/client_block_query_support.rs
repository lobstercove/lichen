use anyhow::{Context, Result};
use serde_json::json;

use crate::client::{BlockInfo, RpcClient};

impl RpcClient {
    /// Get block by slot
    pub async fn get_block(&self, slot: u64) -> Result<BlockInfo> {
        let params = json!([slot]);
        let result = self.call("getBlock", params).await?;

        let block: BlockInfo =
            serde_json::from_value(result).context("Failed to parse block info")?;

        Ok(block)
    }

    /// Get latest block
    pub async fn get_latest_block(&self) -> Result<BlockInfo> {
        let params = json!([]);
        let result = self.call("getLatestBlock", params).await?;

        let block: BlockInfo =
            serde_json::from_value(result).context("Failed to parse block info")?;

        Ok(block)
    }
}