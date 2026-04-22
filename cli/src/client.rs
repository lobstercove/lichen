// RPC client for communicating with Lichen validator

pub use crate::client_types_support::{
    AccountInfo, BalanceInfo, BlockInfo, BurnedInfo, ChainStatus, ContractInfo, ContractLog,
    ContractSummary, Metrics, NetworkInfo, PeerInfo, RewardAdjustmentInfo, StakingRewards,
    StakingStatus, TransactionInfo, ValidatorInfoDetailed, ValidatorPerformance, ValidatorsInfo,
};

#[derive(Clone)]
pub struct RpcClient {
    pub(crate) url: String,
    pub(crate) client: reqwest::Client,
}

pub struct SymbolRegistration<'a> {
    pub symbol: &'a str,
    pub name: Option<&'a str>,
    pub template: Option<&'a str>,
    pub decimals: Option<u8>,
    pub metadata: Option<serde_json::Value>,
}

impl RpcClient {
    pub fn new(url: &str) -> Self {
        RpcClient {
            url: url.to_string(),
            client: reqwest::Client::new(),
        }
    }
}
