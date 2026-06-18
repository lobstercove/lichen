use serde::Deserialize;

#[derive(Deserialize)]
pub struct StakingStatus {
    pub total_staked: u64,
    pub is_validator: bool,
    pub status: String,
    #[serde(default)]
    pub bootstrap_debt: u64,
    #[serde(default)]
    pub earned_amount: u64,
    #[serde(default)]
    pub total_debt_repaid: u64,
    #[serde(default)]
    pub vesting_status: String,
}

#[derive(Deserialize)]
pub struct StakingRewards {
    pub total_rewards: u64,
    pub pending_rewards: u64,
    #[serde(default)]
    pub projected_pending: u64,
    #[serde(default)]
    pub projected_epoch_reward: u64,
    #[serde(default)]
    pub claimed_rewards: u64,
    #[serde(default)]
    pub claimed_total_rewards: u64,
    #[serde(default)]
    pub total_debt_repaid: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_current_staking_status_shape() {
        let status: StakingStatus = serde_json::from_value(json!({
            "is_validator": true,
            "total_staked": 100_000_000_000_000u64,
            "delegations": [],
            "status": "active",
            "bootstrap_debt": 0,
            "earned_amount": 0,
            "total_debt_repaid": 0,
            "vesting_status": "Active"
        }))
        .expect("current getStakingStatus shape parses");

        assert!(status.is_validator);
        assert_eq!(status.total_staked, 100_000_000_000_000);
    }

    #[test]
    fn parses_current_staking_rewards_shape() {
        let rewards: StakingRewards = serde_json::from_value(json!({
            "total_rewards": 10,
            "pending_rewards": 2,
            "projected_pending": 3,
            "projected_epoch_reward": 4,
            "claimed_rewards": 5,
            "liquid_claimed_rewards": 5,
            "claimed_total_rewards": 6,
            "reward_rate": "0.1",
            "bootstrap_debt": 0,
            "earned_amount": 0,
            "vesting_progress": 1.0,
            "blocks_produced": 7,
            "total_debt_repaid": 0
        }))
        .expect("current getStakingRewards shape parses");

        assert_eq!(rewards.total_rewards, 10);
        assert_eq!(rewards.projected_epoch_reward, 4);
    }
}
