use serde::Deserialize;

#[derive(Deserialize)]
pub struct ValidatorsInfo {
    pub validators: Vec<ValidatorInfo>,
    pub _count: usize,
}

#[derive(Deserialize)]
pub struct ValidatorInfo {
    pub pubkey: String,
    pub stake: u64,
    pub reputation: f64,
    pub _normalized_reputation: f64,
    pub _blocks_produced: u64,
    #[serde(rename = "last_vote_slot")]
    pub _last_vote_slot: u64,
}