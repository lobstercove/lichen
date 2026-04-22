use anyhow::Result;
use std::path::PathBuf;

use crate::cli_args::ContractTemplate;
use crate::client::RpcClient;
use crate::deploy_finalize_support::finalize_deploy;
use crate::deploy_prepare_support::prepare_deploy;
use crate::keypair_manager::KeypairManager;

pub(super) struct DeployRequest {
    pub(super) contract: PathBuf,
    pub(super) keypair: Option<PathBuf>,
    pub(super) symbol: Option<String>,
    pub(super) name: Option<String>,
    pub(super) template: Option<ContractTemplate>,
    pub(super) decimals: Option<u8>,
    pub(super) supply: Option<u64>,
    pub(super) metadata: Option<String>,
}

pub(super) async fn handle_deploy(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    request: DeployRequest,
) -> Result<()> {
    let prepared = prepare_deploy(client, keypair_mgr, request).await?;
    finalize_deploy(client, prepared).await
}
