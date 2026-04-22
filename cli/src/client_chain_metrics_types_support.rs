use serde::Deserialize;

#[derive(Deserialize)]
pub struct Metrics {
    pub tps: f64,
    pub total_blocks: u64,
    pub total_transactions: u64,
    pub total_supply: u64,
    pub circulating_supply: u64,
    pub total_burned: u64,
    pub total_staked: u64,
    pub avg_block_time_ms: f64,
    pub avg_txs_per_block: f64,
    pub total_accounts: u64,
    pub total_contracts: u64,
}