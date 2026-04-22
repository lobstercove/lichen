use anyhow::{Context, Result};
use serde_json::json;

use crate::client::{Metrics, RpcClient};

impl RpcClient {
    /// Get performance metrics
    pub async fn get_metrics(&self) -> Result<Metrics> {
        let params = json!([]);
        let result = self.call("getMetrics", params).await?;

        let metrics: Metrics = serde_json::from_value(result).context("Failed to parse metrics")?;

        Ok(metrics)
    }
}