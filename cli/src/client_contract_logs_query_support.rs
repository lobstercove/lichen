use anyhow::{Context, Result};
use serde_json::json;

use crate::client::{ContractLog, ContractLogsResponse, RpcClient};

impl RpcClient {
    /// Get contract logs
    pub async fn get_contract_logs(&self, address: &str, limit: usize) -> Result<Vec<ContractLog>> {
        let params = json!([address, limit]);
        let result = self.call("getContractLogs", params).await?;

        let logs = if result.is_array() {
            serde_json::from_value(result).context("Failed to parse contract logs")?
        } else {
            let response: ContractLogsResponse =
                serde_json::from_value(result).context("Failed to parse contract logs")?;
            response.logs
        };

        Ok(logs)
    }
}
