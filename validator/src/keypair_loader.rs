// Validator keypair management
// Production-ready keypair loading with proper file handling
// Note: This is the validator-specific keypair loader. CLI uses cli/src/keygen.rs.

use anyhow::Result;
use lichen_core::{
    keypair_file::{
        load_keypair_with_password_policy, plaintext_keypair_compat_allowed,
        require_runtime_keypair_password,
    },
    Keypair, KeypairFile,
};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Load validator keypair from file or generate new one.
///
/// Search order:
/// 1. Explicit `config_path` (--keypair CLI argument)
/// 2. Data-directory-local path: `{data_dir}/validator-keypair.json`
/// 3. Generate new keypair and save to the data directory
///
/// Validator identity is scoped to the configured state directory. Normal
/// restarts and upgrades preserve the same identity by preserving that
/// directory and `LICHEN_KEYPAIR_PASSWORD`. Moving an identity to another
/// state directory is explicit via `--import-key`.
pub fn load_or_generate_keypair(
    config_path: Option<&str>,
    _p2p_port: u16,
    data_dir: Option<&Path>,
    _network: Option<&str>,
) -> Result<Keypair> {
    let password =
        require_runtime_keypair_password("validator keypair load").map_err(anyhow::Error::msg)?;
    load_or_generate_keypair_with_options(
        config_path,
        data_dir,
        _network,
        password.as_deref(),
        plaintext_keypair_compat_allowed(),
    )
}

fn load_or_generate_keypair_with_options(
    config_path: Option<&str>,
    data_dir: Option<&Path>,
    _network: Option<&str>,
    password: Option<&str>,
    allow_plaintext: bool,
) -> Result<Keypair> {
    // 1. Explicit CLI path
    if let Some(path) = config_path {
        let p = PathBuf::from(path);
        if p.exists() {
            info!(
                "📁 Loading validator keypair from CLI path: {}",
                p.display()
            );
            return load_keypair_with_options(&p, password, allow_plaintext);
        }
        warn!("⚠️  Specified keypair path does not exist: {}", p.display());
    }

    // 2. Data-directory-local path (HOME-independent, survives HOME changes)
    if let Some(dir) = data_dir {
        let data_dir_path = dir.join("validator-keypair.json");
        if data_dir_path.exists() {
            info!(
                "📁 Loading validator keypair from data dir: {}",
                data_dir_path.display()
            );
            return load_keypair_with_options(&data_dir_path, password, allow_plaintext);
        }
    }

    // 3. Generate new keypair
    warn!("⚠️  No validator keypair found in the configured data path");
    info!("🔑 Generating new validator keypair...");
    let keypair = Keypair::new();

    // Save to data directory when available.
    let save_path = data_dir
        .map(|d| d.join("validator-keypair.json"))
        .unwrap_or_else(|| PathBuf::from("validator-keypair.json"));
    if let Err(e) = save_keypair_with_options(&keypair, &save_path, password) {
        warn!("Failed to save keypair: {}. Will use in-memory only.", e);
    } else {
        info!("💾 Saved validator keypair to: {}", save_path.display());
    }

    Ok(keypair)
}

fn load_keypair_with_options(
    path: &Path,
    password: Option<&str>,
    allow_plaintext: bool,
) -> Result<Keypair> {
    load_keypair_with_password_policy(path, password, allow_plaintext).map_err(anyhow::Error::msg)
}

fn save_keypair_with_options(keypair: &Keypair, path: &Path, password: Option<&str>) -> Result<()> {
    KeypairFile::from_keypair(keypair)
        .save_with_password(path, password, password.is_some())
        .map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keypair_rotation_changes_loaded_pubkey() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let keypair_path = temp_dir.path().join("validator-rotation.json");
        let keypair_path_string = keypair_path.to_string_lossy().to_string();

        let original_keypair = Keypair::new();
        save_keypair_with_options(&original_keypair, &keypair_path, None)
            .expect("save original keypair");

        let loaded_original = load_or_generate_keypair_with_options(
            Some(&keypair_path_string),
            None,
            None,
            None,
            true,
        )
        .expect("load original");
        assert_eq!(loaded_original.pubkey(), original_keypair.pubkey());

        let mut rotated_keypair = Keypair::new();
        while rotated_keypair.pubkey() == original_keypair.pubkey() {
            rotated_keypair = Keypair::new();
        }
        save_keypair_with_options(&rotated_keypair, &keypair_path, None)
            .expect("save rotated keypair");

        let loaded_rotated = load_or_generate_keypair_with_options(
            Some(&keypair_path_string),
            None,
            None,
            None,
            true,
        )
        .expect("load rotated");
        assert_eq!(loaded_rotated.pubkey(), rotated_keypair.pubkey());
        assert_ne!(loaded_rotated.pubkey(), loaded_original.pubkey());
    }

    #[test]
    fn generated_keypair_is_state_scoped_and_reused() {
        let temp_dir = tempfile::tempdir().expect("create temp dir");
        let data_dir = temp_dir.path().join("agent-state");
        std::fs::create_dir_all(&data_dir).expect("create data dir");

        let first = load_or_generate_keypair_with_options(
            None,
            Some(&data_dir),
            Some("testnet"),
            Some("test-password"),
            false,
        )
        .expect("generate state-scoped keypair");

        let keypair_path = data_dir.join("validator-keypair.json");
        assert!(keypair_path.exists());

        let second = load_or_generate_keypair_with_options(
            None,
            Some(&data_dir),
            Some("testnet"),
            Some("test-password"),
            false,
        )
        .expect("reload state-scoped keypair");

        assert_eq!(second.pubkey(), first.pubkey());
    }
}
