use anyhow::{Context, Result};
use serde_json::json;

use crate::client::{RpcClient, ValidatorInfoDetailed, ValidatorPerformance, ValidatorsInfo};

impl RpcClient {
    /// Get all validators
    pub async fn get_validators(&self) -> Result<ValidatorsInfo> {
        let params = json!([]);
        let result = self.call("getValidators", params).await?;

        let validators: ValidatorsInfo =
            serde_json::from_value(result).context("Failed to parse validators info")?;

        Ok(validators)
    }

    /// Get detailed validator information
    pub async fn get_validator_info(&self, pubkey: &str) -> Result<ValidatorInfoDetailed> {
        let params = json!([pubkey]);
        let result = self.call("getValidatorInfo", params).await?;

        let info: ValidatorInfoDetailed =
            serde_json::from_value(result).context("Failed to parse validator info")?;

        Ok(info)
    }

    /// Get validator performance metrics
    pub async fn get_validator_performance(&self, pubkey: &str) -> Result<ValidatorPerformance> {
        let params = json!([pubkey]);
        let result = self.call("getValidatorPerformance", params).await?;

        let perf: ValidatorPerformance =
            serde_json::from_value(result).context("Failed to parse validator performance")?;

        Ok(perf)
    }
}
