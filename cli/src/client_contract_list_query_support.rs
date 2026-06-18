use anyhow::{Context, Result};
use serde_json::json;

use crate::client::{ContractSummary, RpcClient};

#[derive(Debug, serde::Deserialize)]
struct ContractListResponse {
    contracts: Vec<ContractSummary>,
    #[serde(default)]
    has_more: bool,
    #[serde(default)]
    next_cursor: Option<String>,
}

fn parse_contract_list_result(result: serde_json::Value) -> Result<ContractListResponse> {
    let response: ContractListResponse =
        serde_json::from_value(result).context("Failed to parse contracts list")?;
    Ok(response)
}

impl RpcClient {
    /// Get all deployed contracts
    pub async fn get_all_contracts(&self) -> Result<Vec<ContractSummary>> {
        let mut contracts = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let params = match cursor.as_deref() {
                Some(cursor) => json!([{ "limit": 1000, "cursor": cursor }]),
                None => json!([{ "limit": 1000 }]),
            };
            let result = self.call("getAllContracts", params).await?;
            let response = parse_contract_list_result(result)?;
            contracts.extend(response.contracts);
            if !response.has_more {
                return Ok(contracts);
            }
            cursor = response.next_cursor;
            if cursor.is_none() {
                return Ok(contracts);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_contract_list_result;

    #[test]
    fn parses_current_get_all_contracts_object_shape() {
        let contracts = parse_contract_list_result(serde_json::json!({
            "contracts": [
                {
                    "program_id": "C111",
                    "symbol": "ABC",
                    "name": "ABC Token",
                    "owner": "Owner111",
                    "template": "token",
                    "code_size": 1234,
                    "lifecycle_status": "active"
                }
            ],
            "count": 1,
            "has_more": false,
            "next_cursor": null
        }))
        .expect("current object shape should parse");

        assert_eq!(contracts.contracts.len(), 1);
        assert_eq!(contracts.contracts[0].program_id, "C111");
        assert_eq!(contracts.contracts[0].symbol.as_deref(), Some("ABC"));
        assert_eq!(contracts.contracts[0].owner.as_deref(), Some("Owner111"));
        assert!(!contracts.has_more);
        assert_eq!(contracts.next_cursor, None);
    }

    #[test]
    fn rejects_bare_array_contract_list_shape() {
        let error = parse_contract_list_result(serde_json::json!([
            {"address": "old", "deployer": "old"}
        ]))
        .expect_err("current CLI expects the RPC object envelope");

        assert!(error.to_string().contains("Failed to parse contracts list"));
    }
}
