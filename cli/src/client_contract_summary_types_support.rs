use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ContractSummary {
    pub program_id: String,
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub template: Option<String>,
    #[serde(default)]
    pub code_size: usize,
    #[serde(default)]
    pub lifecycle_status: String,
}
