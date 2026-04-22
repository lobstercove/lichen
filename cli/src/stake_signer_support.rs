use anyhow::Result;
use lichen_core::Keypair;
use std::path::PathBuf;

use crate::keypair_manager::KeypairManager;

pub(super) fn load_staker_keypair(
    keypair_mgr: &KeypairManager,
    keypair: Option<PathBuf>,
) -> Result<Keypair> {
    let path = keypair.unwrap_or_else(|| keypair_mgr.default_keypair_path());
    keypair_mgr.load_keypair(&path)
}
