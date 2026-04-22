use anyhow::Result;

use crate::config::CliConfig;

pub async fn get_balance(config: &CliConfig, address: &str) -> Result<()> {
    crate::query_account_support::get_balance(config, address).await
}

pub async fn get_block(config: &CliConfig, slot: u64) -> Result<()> {
    crate::query_chain_support::get_block(config, slot).await
}

pub async fn list_validators(config: &CliConfig) -> Result<()> {
    crate::query_chain_support::list_validators(config).await
}

pub async fn chain_status(config: &CliConfig) -> Result<()> {
    crate::query_chain_support::chain_status(config).await
}
