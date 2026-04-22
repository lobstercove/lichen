use anyhow::Result;
use std::path::PathBuf;

use crate::keypair_manager::KeypairManager;

pub(super) fn resolve_nft_owner_address(
    keypair_mgr: &KeypairManager,
    owner: Option<String>,
    keypair: Option<PathBuf>,
) -> Result<String> {
    if let Some(owner_addr) = owner {
        return Ok(owner_addr);
    }

    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
    let kp = keypair_mgr.load_keypair(&path)?;
    Ok(kp.pubkey().to_base58())
}
