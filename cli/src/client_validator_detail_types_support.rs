use serde::Deserialize;

#[derive(Deserialize)]
pub struct ValidatorInfoDetailed {
    pub pubkey: String,
    pub stake: u64,
    pub reputation: f64,
    pub blocks_produced: u64,
    pub is_active: bool,
}

#[derive(Deserialize)]
pub struct ValidatorPerformance {
    pub _pubkey: String,
    pub blocks_produced: u64,
    pub blocks_expected: u64,
    pub uptime_percent: f64,
    pub avg_block_time_ms: f64,
}