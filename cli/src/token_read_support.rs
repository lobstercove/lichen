use anyhow::Result;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;

pub(super) async fn handle_token_info(client: &RpcClient, token: String) -> Result<()> {
    crate::token_info_support::handle_token_info(client, token).await
}

pub(super) async fn handle_token_balance(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    token: String,
    address: Option<String>,
    keypair: Option<PathBuf>,
) -> Result<()> {
    crate::token_balance_support::handle_token_balance(client, keypair_mgr, token, address, keypair)
        .await
}

pub(super) async fn handle_token_list(client: &RpcClient) -> Result<()> {
    crate::token_list_support::handle_token_list(client).await
}
