use serde::Deserialize;

#[derive(Deserialize)]
pub struct ContractLog {
    pub slot: u64,
    pub message: String,
}