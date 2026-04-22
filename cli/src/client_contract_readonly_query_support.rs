use anyhow::Result;
use lichen_core::Pubkey;
use serde_json::json;

use crate::client::RpcClient;
use crate::client_transport_support::encode_base64;

impl RpcClient {
    /// Execute a read-only contract call without sending a signed transaction.
    pub async fn call_readonly_contract(
        &self,
        contract_address: &Pubkey,
        function: &str,
        args: Vec<u8>,
        from: Option<&Pubkey>,
    ) -> Result<serde_json::Value> {
        let args_b64 = encode_base64(&args);
        let params = if let Some(from_addr) = from {
            json!([
                contract_address.to_base58(),
                function,
                args_b64,
                from_addr.to_base58()
            ])
        } else {
            json!([contract_address.to_base58(), function, args_b64])
        };

        self.call("callContract", params).await
    }
}