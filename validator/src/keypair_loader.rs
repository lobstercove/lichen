// Validator keypair management
// Production-ready keypair loading with proper file handling
// Note: This is the validator-specific keypair loader. CLI uses cli/src/keygen.rs.

use anyhow::{bail, Result};
use lichen_core::{
    keypair_file::{
        copy_secure_file, load_keypair_with_password_policy, plaintext_keypair_compat_allowed,
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
/// 3. Shared HOME path: `~/.lichen/validators/validator-{network}.json`
///    (survives state-directory flushes — the keypair is NOT inside the DB)
/// 4. Generate new keypair and save to BOTH data-dir AND shared HOME path
///
/// The shared HOME path ensures that `rm -rf state-testnet` (a common
/// operational reset) does NOT destroy the validator identity.  Without
/// this, every flush + restart creates a brand-new keypair, which
/// registers as a separate validator and receives a fresh bootstrap
/// grant — inflating the validator set and total staked supply.
pub fn load_or_generate_keypair(
    config_path: Option<&str>,
    _p2p_port: u16,
    data_dir: Option<&Path>,
    network: Option<&str>,
) -> Result<Keypair> {
    let password =
        require_runtime_keypair_password("validator keypair load").map_err(anyhow::Error::msg)?;
    load_or_generate_keypair_with_options(
        config_path,
        data_dir,
        network,
        password.as_deref(),
        plaintext_keypair_compat_allowed(),
    )
}

fn load_or_generate_keypair_with_options(
    config_path: Option<&str>,
    data_dir: Option<&Path>,
    network: Option<&str>,
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

    // 3. Shared HOME path by network name (survives state flushes)
    if let Some(net) = network {
        let shared_path = shared_validator_keypair_path(net);
        if shared_path.exists() {
            info!(
                "📁 Loading validator keypair from shared path: {}",
                shared_path.display()
            );
            let keypair = load_keypair_with_options(&shared_path, password, allow_plaintext)?;

            // Copy into data directory for fast future loads
            if let Some(dir) = data_dir {
                let data_dir_path = dir.join("validator-keypair.json");
                if !data_dir_path.exists() {
                    match copy_secure_file(&shared_path, &data_dir_path) {
                        Ok(()) => info!(
                            "📋 Copied keypair into data dir: {}",
                            data_dir_path.display()
                        ),
                        Err(e) => warn!("⚠️  Failed to copy keypair to data dir: {}", e),
                    }
                }
            }

            return Ok(keypair);
        }
    }

    // 4. Generate new keypair
    warn!("⚠️  No validator keypair found in the configured data or shared paths");
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

    // Also save to shared HOME path so identity survives state flushes
    if let Some(net) = network {
        let shared_path = shared_validator_keypair_path(net);
        match save_keypair_with_options(&keypair, &shared_path, password) {
            Ok(()) => info!(
                "💾 Saved validator keypair (shared): {}",
                shared_path.display()
            ),
            Err(e) => warn!(
                "⚠️  Failed to save shared keypair: {} (identity may not survive state flush)",
                e
            ),
        }
    }

    Ok(keypair)
}

/// Shared HOME-based path keyed by network name (e.g. "testnet", "mainnet").
/// Lives OUTSIDE the state directory so it survives `rm -rf state-*` resets.
///
/// Uses `LICHEN_REAL_HOME` (exported by `lichen-start.sh`) to resolve
/// the operator's actual home directory, because the start script overrides
/// `HOME` to the data-dir for P2P identity isolation.  Without this, the
/// "shared" path lands inside the state directory and gets wiped on flush,
/// causing a new keypair to be generated → ghost validator.
///
/// Path: `$LICHEN_REAL_HOME/.lichen/validators/validator-{network}.json`
pub fn shared_validator_keypair_path(network: &str) -> PathBuf {
    let home = std::env::var("LICHEN_REAL_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".lichen")
        .join("validators")
        .join(format!("validator-{}.json", network))
}

/// Load keypair from file
fn load_keypair(path: &Path) -> Result<Keypair> {
    let password =
        require_runtime_keypair_password("validator keypair load").map_err(anyhow::Error::msg)?;
    load_keypair_with_options(
        path,
        password.as_deref(),
        plaintext_keypair_compat_allowed(),
    )
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

/// Load keypair from environment variable or file
#[allow(dead_code)]
pub fn load_from_env_or_file(env_var: &str, fallback_path: Option<&Path>) -> Result<Keypair> {
    // Try environment variable first
    if let Ok(path_str) = std::env::var(env_var) {
        let path = PathBuf::from(path_str);
        info!(
            "Loading keypair from {} env var: {}",
            env_var,
            path.display()
        );
        return load_keypair(&path);
    }

    // Try fallback path
    if let Some(path) = fallback_path {
        if path.exists() {
            info!("Loading keypair from: {}", path.display());
            return load_keypair(path);
        }
    }

    bail!(
        "No keypair found. Set {} or provide --keypair argument",
        env_var
    );
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
}
