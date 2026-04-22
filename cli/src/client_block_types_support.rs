use serde::Deserialize;

#[derive(Deserialize)]
pub struct BlockInfo {
    pub slot: u64,
    pub hash: String,
    pub parent_hash: String,
    pub state_root: String,
    pub validator: String,
    pub timestamp: u64,
    pub transaction_count: usize,
}

#[derive(Deserialize)]
pub struct BurnedInfo {
    pub spores: u64,
    #[serde(rename = "lichen")]
    pub _lichen: u64,
}