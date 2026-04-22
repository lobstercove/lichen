use anyhow::Result;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::stake_rewards_support::show_stake_rewards;
use crate::stake_status_support::show_stake_status;

pub(super) async fn handle_stake_status(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    address: Option<String>,
    keypair: Option<PathBuf>,
) -> Result<()> {
    show_stake_status(client, keypair_mgr, address, keypair).await
}

pub(super) async fn handle_stake_rewards(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    address: Option<String>,
    keypair: Option<PathBuf>,
) -> Result<()> {
    show_stake_rewards(client, keypair_mgr, address, keypair).await
}
