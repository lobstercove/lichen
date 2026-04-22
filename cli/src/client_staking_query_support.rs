use anyhow::{Context, Result};
use serde_json::json;

use crate::client::{RpcClient, StakingRewards, StakingStatus};

impl RpcClient {
    /// Get staking status
    pub async fn get_staking_status(&self, address: &str) -> Result<StakingStatus> {
        let params = json!([address]);
        let result = self.call("getStakingStatus", params).await?;

        let status: StakingStatus =
            serde_json::from_value(result).context("Failed to parse staking status")?;

        Ok(status)
    }

    /// Get staking rewards
    pub async fn get_staking_rewards(&self, address: &str) -> Result<StakingRewards> {
        let params = json!([address]);
        let result = self.call("getStakingRewards", params).await?;

        let rewards: StakingRewards =
            serde_json::from_value(result).context("Failed to parse staking rewards")?;

        Ok(rewards)
    }
}
