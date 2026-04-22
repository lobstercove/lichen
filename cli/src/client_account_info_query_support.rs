use anyhow::{Context, Result};
use serde_json::json;

use crate::client::{AccountInfo, RpcClient};

impl RpcClient {
    /// Get account information
    pub async fn get_account_info(&self, address: &str) -> Result<AccountInfo> {
        let params = json!([address]);
        let result = self.call("getAccountInfo", params).await?;

        let info: AccountInfo =
            serde_json::from_value(result).context("Failed to parse account info")?;

        Ok(info)
    }
}