use anyhow::{Context, Result};
use serde_json::json;

use crate::client::{RpcClient, TransactionHistoryResponse};

impl RpcClient {
    /// Get transaction history
    pub async fn get_transactions_by_address(
        &self,
        address: &str,
        limit: usize,
    ) -> Result<TransactionHistoryResponse> {
        let params = json!([address, { "limit": limit }]);
        let result = self.call("getTransactionsByAddress", params).await?;

        let history: TransactionHistoryResponse =
            serde_json::from_value(result).context("Failed to parse transaction history")?;

        Ok(history)
    }

    /// Get transaction by signature
    pub async fn get_transaction(&self, signature: &str) -> Result<serde_json::Value> {
        let params = json!([signature]);
        self.call("getTransaction", params).await
    }
}
