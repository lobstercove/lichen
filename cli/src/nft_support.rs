use anyhow::Result;

use crate::cli_args::NftCommands;
use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;
use crate::nft_collection_support::handle_nft_collection;
use crate::nft_marketplace_support::handle_nft_marketplace;
use crate::nft_owner_support::handle_nft_list;

pub(super) async fn handle_nft_command(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    nft_cmd: NftCommands,
    json_output: bool,
) -> Result<()> {
    match nft_cmd {
        NftCommands::List { owner, keypair } => {
            handle_nft_list(client, keypair_mgr, owner, keypair, json_output).await?
        }
        NftCommands::Collection { address } => {
            handle_nft_collection(client, &address, json_output).await?
        }
        NftCommands::Marketplace { limit } => {
            handle_nft_marketplace(client, limit, json_output).await?
        }
    }

    Ok(())
}
