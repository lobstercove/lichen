use super::*;

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct DepositRequest {
    pub(super) deposit_id: String,
    pub(super) user_id: String,
    pub(super) chain: String,
    pub(super) asset: String,
    pub(super) address: String,
    pub(super) derivation_path: String,
    #[serde(default = "default_deposit_seed_source")]
    pub(super) deposit_seed_source: String,
    pub(super) created_at: i64,
    pub(super) status: String,
}

pub(super) const BRIDGE_ACCESS_DOMAIN: &str = "LICHEN_BRIDGE_ACCESS_V1";
pub(super) const BRIDGE_ACCESS_MAX_TTL_SECS: u64 = 24 * 60 * 60;
pub(super) const BRIDGE_ACCESS_CLOCK_SKEW_SECS: u64 = 300;
pub(super) const BRIDGE_AUTH_REPLAY_ACTION_CREATE_DEPOSIT: &str = "createBridgeDeposit";
pub(super) const BRIDGE_AUTH_REPLAY_ACTION_CREATE_WITHDRAWAL: &str = "createWithdrawal";
pub(super) const BRIDGE_AUTH_REPLAY_PRUNE_BATCH: usize = 128;
pub(super) const WITHDRAWAL_ACCESS_DOMAIN: &str = "LICHEN_WITHDRAWAL_ACCESS_V1";
pub(super) const WITHDRAWAL_ACCESS_MAX_TTL_SECS: u64 = 24 * 60 * 60;
pub(super) const WITHDRAWAL_ACCESS_CLOCK_SKEW_SECS: u64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct BridgeAccessAuth {
    pub(super) issued_at: u64,
    pub(super) expires_at: u64,
    pub(super) signature: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct WithdrawalAccessAuth {
    pub(super) issued_at: u64,
    pub(super) expires_at: u64,
    pub(super) nonce: String,
    pub(super) signature: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct BridgeAuthReplayRecord {
    pub(super) deposit_id: String,
    pub(super) expires_at: u64,
    pub(super) chain: String,
    pub(super) asset: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct WithdrawalAuthReplayRecord {
    pub(super) job_id: String,
    pub(super) expires_at: u64,
    pub(super) user_id: String,
    pub(super) asset: String,
    pub(super) amount: u64,
    pub(super) dest_chain: String,
    pub(super) dest_address: String,
    pub(super) preferred_stablecoin: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct DepositEvent {
    pub(super) event_id: String,
    pub(super) deposit_id: String,
    pub(super) tx_hash: String,
    pub(super) confirmations: u64,
    pub(super) amount: Option<u64>,
    pub(super) status: String,
    pub(super) observed_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct WithdrawalRequest {
    pub(super) user_id: String,
    pub(super) asset: String,
    pub(super) amount: u64,
    pub(super) dest_chain: String,
    pub(super) dest_address: String,
    /// For lUSD withdrawals: which stablecoin to receive ("usdt" or "usdc"). Defaults to "usdt".
    #[serde(default = "default_preferred_stablecoin")]
    pub(super) preferred_stablecoin: String,
    #[serde(default)]
    pub(super) auth: Option<Value>,
}
