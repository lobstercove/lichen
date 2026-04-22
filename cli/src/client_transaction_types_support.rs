use serde::Deserialize;

#[derive(Deserialize)]
pub struct TransactionInfo {
    pub signature: String,
    pub slot: u64,
    pub from: String,
    pub to: String,
    pub amount: u64,
}