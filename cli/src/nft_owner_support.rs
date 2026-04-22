use anyhow::Result;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::nft_owner_address_support::resolve_nft_owner_address;
use crate::nft_owner_output_support::{print_nft_owner_error, print_nft_owner_result};

pub(super) async fn handle_nft_list(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    owner: Option<String>,
    keypair: Option<PathBuf>,
    json_output: bool,
) -> Result<()> {
    let addr = resolve_nft_owner_address(keypair_mgr, owner, keypair)?;

    match client.get_nfts_by_owner(&addr).await {
        Ok(nfts) => print_nft_owner_result(&addr, &nfts, json_output),
        Err(error) => print_nft_owner_error(&error, json_output),
    }

    Ok(())
}
