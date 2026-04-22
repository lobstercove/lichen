use anyhow::{Context, Result};
use serde_json::json;

use crate::client::{ContractInfo, RpcClient};

impl RpcClient {
    /// Get contract information
    pub async fn get_contract_info(&self, address: &str) -> Result<ContractInfo> {
        let params = json!([address]);
        let result = self.call("getContractInfo", params).await?;

        let info: ContractInfo =
            serde_json::from_value(result).context("Failed to parse contract info")?;

        Ok(info)
    }
}