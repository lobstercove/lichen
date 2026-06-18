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
            "commission_rate": 0,
            "is_active": true
        }))
        .expect("current getValidatorInfo shape parses");

        assert_eq!(info.blocks_proposed, 7);
        assert_eq!(info.transactions_processed, 42);
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
