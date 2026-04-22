use anyhow::Result;
use std::path::PathBuf;

use crate::keypair_manager::KeypairManager;

pub(super) fn resolve_stake_address(
    keypair_mgr: &KeypairManager,
    address: Option<String>,
    keypair: Option<PathBuf>,
) -> Result<String> {
    if let Some(address) = address {
        return Ok(address);
    }

    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
    let kp = keypair_mgr.load_keypair(&path)?;
    Ok(kp.pubkey().to_base58())
}