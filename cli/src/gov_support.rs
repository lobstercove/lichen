use anyhow::Result;

use crate::cli_args::GovCommands;
use crate::client::RpcClient;
use crate::gov_common_support::resolve_dao_address;
use crate::gov_proposal_action_support::{handle_gov_execute, handle_gov_veto, handle_gov_vote};
use crate::gov_query_support::{handle_gov_info, handle_gov_list};
use crate::gov_write_support::handle_gov_propose;
use crate::keypair_manager::KeypairManager;

pub(super) async fn handle_gov_command(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    gov_cmd: GovCommands,
) -> Result<()> {
    let dao_addr = resolve_dao_address(client).await;

    match gov_cmd {
        GovCommands::Propose {
            title,
            description,
            proposal_type,
            keypair,
        } => {
            handle_gov_propose(
                client,
                keypair_mgr,
                &dao_addr,
                title,
                description,
                proposal_type,
                keypair,
            )
            .await?
        }
        GovCommands::Vote {
            proposal_id,
            vote,
            keypair,
        } => handle_gov_vote(client, keypair_mgr, &dao_addr, proposal_id, vote, keypair).await?,
        GovCommands::List { all } => handle_gov_list(client, keypair_mgr, &dao_addr, all).await?,
        GovCommands::Info { proposal_id } => {
            handle_gov_info(client, keypair_mgr, &dao_addr, proposal_id).await?
        }
        GovCommands::Execute {
            proposal_id,
            keypair,
        } => handle_gov_execute(client, keypair_mgr, &dao_addr, proposal_id, keypair).await?,
        GovCommands::Veto {
            proposal_id,
            keypair,
        } => handle_gov_veto(client, keypair_mgr, &dao_addr, proposal_id, keypair).await?,
    }

    Ok(())
}
