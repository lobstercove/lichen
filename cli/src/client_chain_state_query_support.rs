use anyhow::{Context, Result};
use serde_json::json;

use crate::client::{BurnedInfo, RpcClient};

impl RpcClient {
    /// Get total burned LICN
    pub async fn get_total_burned(&self) -> Result<BurnedInfo> {
        let params = json!([]);
        let result = self.call("getTotalBurned", params).await?;

        let burned: BurnedInfo =
            serde_json::from_value(result).context("Failed to parse burned info")?;

        Ok(burned)
    }

    /// Get current slot
    pub async fn get_slot(&self) -> Result<u64> {
        let params = json!([]);
        let result = self.call("getSlot", params).await?;

        result.as_u64().context("Invalid slot response")
    }
}