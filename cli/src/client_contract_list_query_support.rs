use anyhow::{Context, Result};
use serde_json::json;

use crate::client::{ContractSummary, RpcClient};

impl RpcClient {
    /// Get all deployed contracts
    pub async fn get_all_contracts(&self) -> Result<Vec<ContractSummary>> {
        let params = json!([]);
        let result = self.call("getAllContracts", params).await?;

        let contracts: Vec<ContractSummary> =
            serde_json::from_value(result).context("Failed to parse contracts list")?;

        Ok(contracts)
    }
}