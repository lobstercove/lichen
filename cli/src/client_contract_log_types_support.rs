use serde::Deserialize;

#[derive(Deserialize)]
pub struct ContractLog {
    pub slot: u64,
    #[serde(default)]
    pub program: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Deserialize)]
pub struct ContractLogsResponse {
    pub logs: Vec<ContractLog>,
}
