use anyhow::{Context, Result};
use serde_json::json;

use crate::client::{RewardAdjustmentInfo, RpcClient};

impl RpcClient {
    /// Get reward and inflation settings used for epoch calculations
    pub async fn get_reward_adjustment_info(&self) -> Result<RewardAdjustmentInfo> {
        let params = json!([]);
        let result = self.call("getRewardAdjustmentInfo", params).await?;

        let info: RewardAdjustmentInfo =
            serde_json::from_value(result).context("Failed to parse reward adjustment info")?;

        Ok(info)
    }
}