use serde::Deserialize;

#[derive(Deserialize)]
pub struct BalanceInfo {
    pub spores: u64,
    pub spendable: u64,
    pub staked: u64,
    pub locked: u64,
}
