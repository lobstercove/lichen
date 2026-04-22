use serde::Deserialize;

#[derive(Deserialize)]
pub struct ChainStatus {
    pub _slot: u64,
    pub _epoch: u64,
    pub _block_height: u64,
    pub _validators: usize,
    pub tps: f64,
    pub total_staked: u64,
    pub block_time_ms: f64,
    pub validator_count: usize,
    pub peer_count: usize,
    pub total_transactions: u64,
    pub total_blocks: u64,
    pub total_supply: u64,
    pub total_burned: u64,
    pub current_slot: u64,
    pub latest_block: u64,
    pub chain_id: String,
    pub network: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardAdjustmentInfo {
    pub slots_per_epoch: u64,
}