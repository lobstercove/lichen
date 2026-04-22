use anyhow::Result;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::stake_add_support::show_stake_add;
use crate::stake_unstake_support::show_stake_remove;

pub(super) async fn handle_stake_add(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    amount: u64,
    keypair: Option<PathBuf>,
) -> Result<()> {
    show_stake_add(client, keypair_mgr, amount, keypair).await
}

pub(super) async fn handle_stake_remove(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    amount: u64,
    keypair: Option<PathBuf>,
) -> Result<()> {
    show_stake_remove(client, keypair_mgr, amount, keypair).await
}
