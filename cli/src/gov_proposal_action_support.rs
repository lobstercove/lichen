use anyhow::Result;
use lichen_core::Pubkey;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::gov_common_support::{encode_proposal_id, load_default_keypair, parse_vote_value};
use crate::keypair_manager::KeypairManager;

pub(super) async fn handle_gov_vote(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    dao_addr: &Pubkey,
    proposal_id: u64,
    vote: String,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let voter = load_default_keypair(keypair_mgr, keypair)?;
    let Some(vote_value) = parse_vote_value(&vote) else {
        println!("⚠️  Invalid vote. Use: yes, no, abstain");
        return Ok(());
    };

    println!("🗳️  Voting {} on proposal #{}", vote, proposal_id);
    println!("   Voter: {}", voter.pubkey().to_base58());

    let mut data = encode_proposal_id(proposal_id);
    data.push(vote_value);

    let signature = client
        .call_contract(&voter, dao_addr, "vote".to_string(), data, 0)
        .await?;
    println!("✅ Vote cast! Sig: {}", signature);

    Ok(())
}

pub(super) async fn handle_gov_execute(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    dao_addr: &Pubkey,
    proposal_id: u64,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let executor = load_default_keypair(keypair_mgr, keypair)?;

    println!("⚡ Executing proposal #{}", proposal_id);
    println!("   Executor: {}", executor.pubkey().to_base58());

    let data = encode_proposal_id(proposal_id);

    let signature = client
        .call_contract(&executor, dao_addr, "execute_proposal".to_string(), data, 0)
        .await?;
    println!("✅ Proposal executed! Sig: {}", signature);

    Ok(())
}

pub(super) async fn handle_gov_veto(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    dao_addr: &Pubkey,
    proposal_id: u64,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let vetoer = load_default_keypair(keypair_mgr, keypair)?;

    println!("🚫 Vetoing proposal #{}", proposal_id);
    println!("   Vetoer: {}", vetoer.pubkey().to_base58());

    let data = encode_proposal_id(proposal_id);

    let signature = client
        .call_contract(&vetoer, dao_addr, "veto_proposal".to_string(), data, 0)
        .await?;
    println!("✅ Veto cast! Sig: {}", signature);

    Ok(())
}
