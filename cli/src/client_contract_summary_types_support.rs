use serde::Deserialize;

#[derive(Deserialize)]
pub struct ContractSummary {
    pub address: String,
    pub deployer: String,
}