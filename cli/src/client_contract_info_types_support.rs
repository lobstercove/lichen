use serde::Deserialize;

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
    #[serde(default)]
    pub token_metadata: Option<serde_json::Value>,
    #[serde(rename = "is_native", default)]
    pub _is_native: bool,
}
