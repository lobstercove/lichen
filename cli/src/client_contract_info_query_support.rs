use anyhow::{Context, Result};
use lichen_core::Hash;
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

    pub async fn is_code_hash_deploy_blocked(&self, code_hash: &Hash) -> Result<bool> {
        let result = self
            .call("getCodeHashRestrictionStatus", json!([code_hash.to_hex()]))
            .await?;

        Ok(result
            .get("deploy_blocked")
            .and_then(|value| value.as_bool())
            .unwrap_or(false))
    }
}
