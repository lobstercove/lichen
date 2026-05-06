use serde::Deserialize;

fn default_lifecycle_status() -> String {
    "active".to_string()
}

#[derive(Deserialize)]
pub struct ContractInfo {
    #[serde(alias = "contract_id", alias = "address")]
    pub address: String,
    #[serde(default, alias = "owner", alias = "deployer")]
    pub owner: String,
    pub deployed_at: u64,
    pub code_size: usize,
    #[serde(rename = "abi_functions", default)]
    pub _abi_functions: usize,
    #[serde(rename = "is_executable", default)]
    pub _is_executable: bool,
    #[serde(rename = "has_abi", default)]
    pub _has_abi: bool,
    #[serde(rename = "version", default)]
    pub _version: u32,
    #[serde(default = "default_lifecycle_status")]
    pub lifecycle_status: String,
    #[serde(default)]
    pub lifecycle_updated_slot: u64,
    #[serde(default)]
    pub lifecycle_restriction_id: Option<u64>,
    #[serde(default)]
    pub lifecycle_effective_at_slot: u64,
    #[serde(default)]
    pub token_metadata: Option<serde_json::Value>,
    #[serde(rename = "is_native", default)]
    pub _is_native: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_info_lifecycle_fields_deserialize() {
        let info: ContractInfo = serde_json::from_value(serde_json::json!({
            "contract_id": "contract",
            "owner": "owner",
            "deployed_at": 0,
            "code_size": 8,
            "lifecycle_status": "quarantined",
            "lifecycle_updated_slot": 99,
            "lifecycle_restriction_id": 7,
            "lifecycle_effective_at_slot": 101
        }))
        .unwrap();

        assert_eq!(info.lifecycle_status, "quarantined");
        assert_eq!(info.lifecycle_updated_slot, 99);
        assert_eq!(info.lifecycle_restriction_id, Some(7));
        assert_eq!(info.lifecycle_effective_at_slot, 101);
    }

    #[test]
    fn contract_info_lifecycle_fields_default_for_legacy_rpc() {
        let info: ContractInfo = serde_json::from_value(serde_json::json!({
            "contract_id": "contract",
            "owner": "owner",
            "deployed_at": 0,
            "code_size": 8
        }))
        .unwrap();

        assert_eq!(info.lifecycle_status, "active");
        assert_eq!(info.lifecycle_updated_slot, 0);
        assert_eq!(info.lifecycle_restriction_id, None);
        assert_eq!(info.lifecycle_effective_at_slot, 0);
    }
}
