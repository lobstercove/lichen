use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;

use super::rate_limit::RateLimiter;

pub(super) const SPORES_PER_LICN: u64 = 1_000_000_000;
pub(super) const DEFAULT_PORT: u16 = 9100;
pub(super) const DEFAULT_MAX_PER_REQUEST: u64 = 10;
pub(super) const DEFAULT_DAILY_LIMIT_PER_IP: u64 = 150;
pub(super) const DEFAULT_COOLDOWN_SECONDS: u64 = 60;

#[derive(Debug, Deserialize)]
pub(super) struct FaucetRequest {
    pub(super) address: String,
    #[serde(default)]
    pub(super) amount: Option<u64>,
}

#[derive(Debug, Serialize)]
pub(super) struct FaucetResponse {
    pub(super) success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) signature: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) amount: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) recipient: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) error: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct FaucetPublicConfig {
    pub(super) max_per_request: u64,
    pub(super) daily_limit_per_ip: u64,
    pub(super) cooldown_seconds: u64,
    pub(super) network: String,
}

#[derive(Debug, Serialize)]
pub(super) struct FaucetStatusResponse {
    pub(super) network: String,
    pub(super) faucet_address: String,
    pub(super) balance_spores: u64,
    pub(super) balance_licn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct AirdropRecord {
    pub(super) signature: Option<String>,
    pub(super) recipient: String,
    pub(super) amount_licn: u64,
    pub(super) timestamp_ms: u64,
    #[serde(default)]
    pub(super) ip: Option<String>,
}

#[derive(Clone)]
pub(super) struct FaucetConfig {
    pub(super) rpc_url: String,
    pub(super) network: String,
    pub(super) max_per_request: u64,
    pub(super) daily_limit_per_ip: u64,
    pub(super) cooldown_seconds: u64,
    pub(super) airdrops_file: String,
    pub(super) trusted_proxies: Vec<String>,
}

#[derive(Clone)]
pub(super) struct FaucetState {
    pub(super) config: FaucetConfig,
    pub(super) http: Client,
    pub(super) rate_limiter: Arc<RwLock<RateLimiter>>,
    pub(super) airdrops: Arc<RwLock<Vec<AirdropRecord>>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AirdropQuery {
    pub(super) limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TreasuryInfo {
    #[serde(default)]
    pub(super) treasury_pubkey: Option<String>,
    #[serde(default)]
    pub(super) treasury_balance: u64,
}
