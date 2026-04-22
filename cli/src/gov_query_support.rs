use anyhow::Result;
use lichen_core::Pubkey;

use crate::client::RpcClient;
use crate::gov_common_support::encode_proposal_id;
use crate::gov_query_request_support::{submit_gov_query, GovQueryRequest};
use crate::keypair_manager::KeypairManager;

pub(super) async fn handle_gov_list(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    dao_addr: &Pubkey,
    all: bool,
) -> Result<()> {
    println!(
        "📜 Governance Proposals {}",
        if all { "(all)" } else { "(active)" }
    );
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    let filter = if all { "all" } else { "active" };
    submit_gov_query(
        client,
        keypair_mgr,
        dao_addr,
        GovQueryRequest {
            function: "get_proposals",
            data: filter.as_bytes().to_vec(),
            error_subject: "proposals",
            success_hint: "Check transaction logs for proposal list",
        },
    )
    .await?;

    Ok(())
}

pub(super) async fn handle_gov_info(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    dao_addr: &Pubkey,
    proposal_id: u64,
) -> Result<()> {
    println!("📜 Proposal #{}", proposal_id);
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!();

    submit_gov_query(
        client,
        keypair_mgr,
        dao_addr,
        GovQueryRequest {
            function: "get_proposal",
            data: encode_proposal_id(proposal_id),
            error_subject: "proposal",
            success_hint: "Check transaction logs for proposal details",
        },
    )
    .await?;

    Ok(())
}
