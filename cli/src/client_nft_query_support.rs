use anyhow::Result;
use serde_json::json;

use crate::client::RpcClient;

impl RpcClient {
    /// Get NFTs owned by an address
    pub async fn get_nfts_by_owner(&self, address: &str) -> Result<serde_json::Value> {
        let params = json!([address]);
        self.call("getNFTsByOwner", params).await
    }

    /// Get NFTs in a collection
    pub async fn get_nfts_by_collection(&self, address: &str) -> Result<serde_json::Value> {
        let params = json!([address]);
        self.call("getNFTsByCollection", params).await
    }

    /// Get marketplace listings
    pub async fn get_market_listings(&self, limit: usize) -> Result<serde_json::Value> {
        let params = json!([limit]);
        self.call("getMarketListings", params).await
    }
}
