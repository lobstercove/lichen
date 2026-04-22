use anyhow::Result;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::token_create_finalize_support::finalize_token_create;
use crate::token_create_prepare_support::prepare_token_create;

pub(super) struct TokenCreateRequest {
    pub(super) name: String,
    pub(super) symbol: String,
    pub(super) wasm: PathBuf,
    pub(super) decimals: u8,
    pub(super) initial_supply: Option<u64>,
    pub(super) website: Option<String>,
    pub(super) logo_url: Option<String>,
    pub(super) description: Option<String>,
    pub(super) twitter: Option<String>,
    pub(super) telegram: Option<String>,
    pub(super) discord: Option<String>,
    pub(super) keypair: Option<PathBuf>,
}

pub(super) async fn handle_token_create(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    request: TokenCreateRequest,
) -> Result<()> {
    let prepared = prepare_token_create(client, keypair_mgr, request).await?;
    finalize_token_create(client, prepared).await
}
