use anyhow::Result;
use lichen_core::Pubkey;
use serde_json::json;

use crate::client::RpcClient;

impl RpcClient {
    /// Resolve a symbol (e.g., "DAO", "LICN", "DEX") to its on-chain contract address.
    pub async fn resolve_symbol(&self, symbol: &str) -> Result<Option<Pubkey>> {
        let params = json!([symbol]);
        let result = self.call("getSymbolRegistry", params).await;

        match result {
            Ok(value) => {
                if let Some(program) = value.get("program").and_then(|entry| entry.as_str()) {
                    match Pubkey::from_base58(program) {
                        Ok(pubkey) => Ok(Some(pubkey)),
                        Err(_) => Ok(None),
                    }
                } else {
                    Ok(None)
                }
            }
            Err(_) => Ok(None),
        }
    }

    /// Get symbol registry entry by symbol name
    pub async fn get_symbol_registry(&self, symbol: &str) -> Result<serde_json::Value> {
        let params = json!([symbol]);
        self.call("getSymbolRegistry", params).await
    }

    /// Get all symbol registry entries
    pub async fn get_all_symbol_registry(&self) -> Result<serde_json::Value> {
        let params = json!([]);
        self.call("getAllSymbolRegistry", params).await
    }

    /// Get symbol registry entry by contract address
    pub async fn get_symbol_by_program(&self, address: &str) -> Result<serde_json::Value> {
        let params = json!([address]);
        self.call("getSymbolRegistryByProgram", params).await
    }
}
