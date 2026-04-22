use serde::Deserialize;

#[derive(Deserialize)]
pub struct StakingStatus {
    pub address: String,
    pub staked: u64,
    pub is_validator: bool,
}

#[derive(Deserialize)]
pub struct StakingRewards {
    pub address: String,
    pub total_rewards: u64,
    pub pending_rewards: u64,
}
