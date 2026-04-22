use anyhow::Result;
use lichen_core::Pubkey;
use serde_json::json;

use crate::client::{BalanceInfo, RpcClient};

impl RpcClient {
    /// Get account balance breakdown
    pub async fn get_balance(&self, pubkey: &Pubkey) -> Result<BalanceInfo> {
        let params = json!([pubkey.to_base58()]);
        let result = self.call("getBalance", params).await?;

        let spores = result
            .get("spores")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let spendable = result
            .get("spendable")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let staked = result
            .get("staked")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);
        let locked = result
            .get("locked")
            .and_then(|value| value.as_u64())
            .unwrap_or(0);

        Ok(BalanceInfo {
            spores,
            spendable,
            staked,
            locked,
        })
    }
}