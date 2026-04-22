use anyhow::Result;
use lichen_core::Pubkey;
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::gov_common_support::{description_preview, load_default_keypair, parse_proposal_kind};
use crate::keypair_manager::KeypairManager;

pub(super) async fn handle_gov_propose(
    client: &RpcClient,
    keypair_mgr: &KeypairManager,
    dao_addr: &Pubkey,
    title: String,
    description: String,
    proposal_type: String,
    keypair: Option<PathBuf>,
) -> Result<()> {
    let proposer = load_default_keypair(keypair_mgr, keypair)?;
    let Some(proposal_kind) = parse_proposal_kind(&proposal_type) else {
        println!("⚠️  Invalid proposal type. Use: fast-track, standard, constitutional");
        return Ok(());
    };

    println!("📜 Creating {} proposal", proposal_type);
    println!("   Title: {}", title);
    println!("   Description: {}", description_preview(&description, 80));
    println!("   Proposer: {}", proposer.pubkey().to_base58());
    println!("   Stake: 1000 LICN required");
    println!();

    let mut data = Vec::new();
    data.push(proposal_kind);
    data.extend_from_slice(&(title.len() as u32).to_le_bytes());
    data.extend_from_slice(title.as_bytes());
    data.extend_from_slice(&(description.len() as u32).to_le_bytes());
    data.extend_from_slice(description.as_bytes());

    let signature = client
        .call_contract(
            &proposer,
            dao_addr,
            "create_proposal_typed".to_string(),
            data,
            0,
        )
        .await?;
    println!("✅ Proposal created! Sig: {}", signature);

    Ok(())
}
