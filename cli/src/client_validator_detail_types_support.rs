use serde::Deserialize;

#[derive(Deserialize)]
pub struct ValidatorInfoDetailed {
    pub pubkey: String,
    pub stake: u64,
    pub reputation: f64,
    pub blocks_proposed: u64,
    pub transactions_processed: u64,
    pub votes_cast: u64,
    pub correct_votes: u64,
    pub last_active_slot: u64,
    pub is_active: bool,
    #[serde(default)]
    pub staking_v2_active: bool,
    #[serde(default)]
    pub staking_v2_epoch_active: bool,
    #[serde(default)]
    pub self_bond: u64,
    #[serde(default)]
    pub delegated_stake: u64,
    #[serde(default)]
    pub effective_stake: u64,
    #[serde(default)]
    pub epoch_consensus_power: u64,
    #[serde(default)]
    pub commission_rate: u64,
    #[serde(default)]
    pub pending_commission_rate: Option<u64>,
    #[serde(default)]
    pub pending_commission_activation_epoch: Option<u64>,
    #[serde(default)]
    pub network_saturation_cap_bps: u64,
    #[serde(default)]
    pub effective_stake_limit: u64,
    #[serde(default)]
    pub saturation_usage_bps: u64,
    #[serde(default)]
    pub delegation_capacity_remaining: u64,
}

#[derive(Deserialize)]
pub struct ValidatorPerformance {
    pub pubkey: String,
    pub blocks_proposed: u64,
    pub transactions_processed: u64,
    pub votes_cast: u64,
    pub correct_votes: u64,
    pub vote_accuracy: f64,
    pub reputation: f64,
    pub uptime: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_current_validator_info_shape() {
        let info: ValidatorInfoDetailed = serde_json::from_value(json!({
            "pubkey": "validator",
            "stake": 100,
            "reputation": 1000.0,
            "blocks_proposed": 7,
            "transactions_processed": 42,
            "votes_cast": 9,
            "correct_votes": 8,
            "last_active_slot": 123,
            "last_observed_at_ms": 1,
            "last_observed_block_at_ms": 1,
            "last_observed_block_slot": 123,
            "head_staleness_ms": 0,
            "joined_slot": 1,
            "commission_rate": 500,
            "pending_commission_rate": 600,
            "pending_commission_activation_epoch": 12,
            "staking_v2_active": true,
            "staking_v2_epoch_active": true,
            "self_bond": 40,
            "delegated_stake": 60,
            "effective_stake": 100,
            "epoch_consensus_power": 100,
            "network_saturation_cap_bps": 500,
            "effective_stake_limit": 200,
            "saturation_usage_bps": 5000,
            "delegation_capacity_remaining": 100,
            "is_active": true
        }))
        .expect("current getValidatorInfo shape parses");

        assert_eq!(info.blocks_proposed, 7);
        assert_eq!(info.transactions_processed, 42);
        assert_eq!(info.commission_rate, 500);
        assert_eq!(info.pending_commission_rate, Some(600));
        assert!(info.staking_v2_epoch_active);
        assert_eq!(info.delegation_capacity_remaining, 100);
    }

    #[test]
    fn parses_current_validator_performance_shape() {
        let perf: ValidatorPerformance = serde_json::from_value(json!({
            "pubkey": "validator",
            "blocks_proposed": 7,
            "transactions_processed": 42,
            "votes_cast": 9,
            "correct_votes": 8,
            "vote_accuracy": 88.8,
            "reputation": 999.0,
            "uptime": 12.5
        }))
        .expect("current getValidatorPerformance shape parses");

        assert_eq!(perf.pubkey, "validator");
        assert_eq!(perf.blocks_proposed, 7);
    }
}
