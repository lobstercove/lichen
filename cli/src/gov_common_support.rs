use anyhow::Result;
use lichen_core::{Keypair, Pubkey};
use std::path::PathBuf;

use crate::client::RpcClient;
use crate::keypair_manager::KeypairManager;

pub(super) async fn resolve_dao_address(client: &RpcClient) -> Pubkey {
    match client.resolve_symbol("DAO").await {
        Ok(Some(address)) => address,
        _ => {
            eprintln!("⚠️  DAO contract not found in symbol registry, using well-known address");
            Pubkey([0xDA; 32])
        }
    }
}

pub(super) fn load_default_keypair(
    keypair_mgr: &KeypairManager,
    keypair: Option<PathBuf>,
) -> Result<Keypair> {
    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
    keypair_mgr.load_keypair(&path)
}

pub(super) fn load_query_keypair(keypair_mgr: &KeypairManager) -> Result<Keypair> {
    keypair_mgr
        .load_keypair(&keypair_mgr.default_keypair_path())
        .map_err(|_| anyhow::anyhow!("No wallet configured. Run `lichen wallet create` first."))
}

pub(super) fn parse_proposal_kind(proposal_type: &str) -> Option<u8> {
    match proposal_type {
        "fast-track" | "fast" => Some(0),
        "standard" => Some(1),
        "constitutional" | "const" => Some(2),
        _ => None,
    }
}

pub(super) fn parse_vote_value(vote: &str) -> Option<u8> {
    match vote.to_ascii_lowercase().as_str() {
        "yes" | "y" | "1" => Some(1),
        "no" | "n" | "0" => Some(0),
        "abstain" | "a" | "2" => Some(2),
        _ => None,
    }
}

pub(super) fn encode_proposal_id(proposal_id: u64) -> Vec<u8> {
    proposal_id.to_le_bytes().to_vec()
}

pub(super) fn description_preview(description: &str, max_chars: usize) -> String {
    let preview: String = description.chars().take(max_chars).collect();
    if description.len() > preview.len() {
        format!("{}...", preview)
    } else {
        preview
    }
}
