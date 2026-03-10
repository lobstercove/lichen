// Validator keypair management
// Production-ready keypair loading with proper file handling
// Note: This is the validator-specific keypair loader. CLI uses cli/src/keygen.rs.

use anyhow::{bail, Context, Result};
use moltchain_core::Keypair;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// Keypair file format (Solana-compatible)
#[derive(Debug, Serialize, Deserialize)]
struct KeypairFile {
    #[serde(rename = "privateKey")]
    private_key: Vec<u8>,
    #[serde(rename = "publicKey")]
    public_key: Vec<u8>,
    #[serde(rename = "publicKeyBase58")]
    public_key_base58: String,
}

/// Load validator keypair from file or generate new one.
///
/// Search order:
/// 1. Explicit `config_path` (--keypair CLI argument)
/// 2. Data-directory-local path: `{data_dir}/validator-keypair.json`
/// 3. Shared HOME path: `~/.moltchain/validators/validator-{network}.json`
///    (survives state-directory flushes — the keypair is NOT inside the DB)
/// 4. Legacy port-based HOME path: `~/.moltchain/validators/validator-{port}.json`
/// 5. Generate new keypair and save to BOTH data-dir AND shared HOME path
///
/// The shared HOME path ensures that `rm -rf state-testnet` (a common
/// operational reset) does NOT destroy the validator identity.  Without
/// this, every flush + restart creates a brand-new keypair, which
/// registers as a separate validator and receives a fresh bootstrap
/// grant — inflating the validator set and total staked supply.
pub fn load_or_generate_keypair(
    config_path: Option<&str>,
    p2p_port: u16,
    data_dir: Option<&Path>,
    network: Option<&str>,
) -> Result<Keypair> {
    // 1. Explicit CLI path
    if let Some(path) = config_path {
        let p = PathBuf::from(path);
        if p.exists() {
            info!(
                "📁 Loading validator keypair from CLI path: {}",
                p.display()
            );
            return load_keypair(&p);
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
            return load_keypair(&data_dir_path);
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
            let keypair = load_keypair(&shared_path)?;

            // Copy into data directory for fast future loads
            if let Some(dir) = data_dir {
                let data_dir_path = dir.join("validator-keypair.json");
                if !data_dir_path.exists() {
                    match save_keypair(&keypair, &data_dir_path) {
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

    // 4. Legacy port-based HOME path
    let legacy_path = default_validator_keypair_path(p2p_port);
    if legacy_path.exists() {
        info!(
            "📁 Loading validator keypair from legacy path: {}",
            legacy_path.display()
        );
        let keypair = load_keypair(&legacy_path)?;

        // Migrate to data directory for future restarts
        if let Some(dir) = data_dir {
            let data_dir_path = dir.join("validator-keypair.json");
            match save_keypair(&keypair, &data_dir_path) {
                Ok(()) => info!(
                    "📋 Migrated keypair to data dir: {}",
                    data_dir_path.display()
                ),
                Err(e) => warn!(
                    "⚠️  Failed to migrate keypair to data dir: {} (using legacy path)",
                    e
                ),
            }
        }

        // Also save to shared path for future flush-resilience
        if let Some(net) = network {
            let shared_path = shared_validator_keypair_path(net);
            if !shared_path.exists() {
                let _ = save_keypair(&keypair, &shared_path);
            }
        }

        return Ok(keypair);
    }

    // 5. Generate new keypair
    warn!(
        "⚠️  No keypair found at data dir or legacy path: {}",
        legacy_path.display()
    );
    info!("🔑 Generating new validator keypair...");
    let keypair = Keypair::new();

    // Save to data directory (preferred) or legacy path
    let save_path = data_dir
        .map(|d| d.join("validator-keypair.json"))
        .unwrap_or_else(|| legacy_path.clone());
    if let Err(e) = save_keypair(&keypair, &save_path) {
        warn!("Failed to save keypair: {}. Will use in-memory only.", e);
    } else {
        info!("💾 Saved validator keypair to: {}", save_path.display());
    }

    // Also save to shared HOME path so identity survives state flushes
    if let Some(net) = network {
        let shared_path = shared_validator_keypair_path(net);
        match save_keypair(&keypair, &shared_path) {
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

/// Get legacy HOME-based validator keypair path.
/// Prefer data-directory-local path via `load_or_generate_keypair`.
pub fn default_validator_keypair_path(p2p_port: u16) -> PathBuf {
    let home = std::env::var("MOLTCHAIN_REAL_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".moltchain")
        .join("validators")
        .join(format!("validator-{}.json", p2p_port))
}

/// Shared HOME-based path keyed by network name (e.g. "testnet", "mainnet").
/// Lives OUTSIDE the state directory so it survives `rm -rf state-*` resets.
///
/// Uses `MOLTCHAIN_REAL_HOME` (exported by `moltchain-start.sh`) to resolve
/// the operator's actual home directory, because the start script overrides
/// `HOME` to the data-dir for P2P identity isolation.  Without this, the
/// "shared" path lands inside the state directory and gets wiped on flush,
/// causing a new keypair to be generated → ghost validator.
///
/// Path: `$MOLTCHAIN_REAL_HOME/.moltchain/validators/validator-{network}.json`
fn shared_validator_keypair_path(network: &str) -> PathBuf {
    let home = std::env::var("MOLTCHAIN_REAL_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".moltchain")
        .join("validators")
        .join(format!("validator-{}.json", network))
}

/// Load keypair from file
fn load_keypair(path: &Path) -> Result<Keypair> {
    let json = fs::read_to_string(path).context("Failed to read keypair file")?;

    let keypair_file: KeypairFile =
        serde_json::from_str(&json).context("Failed to parse keypair file")?;

    if keypair_file.private_key.len() != 32 {
        bail!("Invalid private key length: expected 32 bytes");
    }

    let mut seed = [0u8; 32];
    seed.copy_from_slice(&keypair_file.private_key);
    let keypair = Keypair::from_seed(&seed);
    // P10-VAL-06: Zeroize seed bytes after use to minimize key material exposure
    seed.iter_mut().for_each(|b| *b = 0);
    Ok(keypair)
}

/// Save keypair to file
fn save_keypair(keypair: &Keypair, path: &Path) -> Result<()> {
    // Create parent directories
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).context("Failed to create directory")?;
    }

    // Create keypair file
    let pubkey = keypair.pubkey();
    let seed = keypair.to_seed();

    let keypair_file = KeypairFile {
        private_key: seed.to_vec(),
        public_key: pubkey.0.to_vec(),
        public_key_base58: pubkey.to_base58(),
    };

    // Serialize and write
    let json = serde_json::to_string_pretty(&keypair_file)?;
    fs::write(path, json)?;

    // Set secure permissions (Unix only)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = fs::Permissions::from_mode(0o600);
        fs::set_permissions(path, permissions)?;
    }

    Ok(())
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
        save_keypair(&original_keypair, &keypair_path).expect("save original keypair");

        let loaded_original = load_or_generate_keypair(Some(&keypair_path_string), 0, None, None)
            .expect("load original");
        assert_eq!(loaded_original.pubkey(), original_keypair.pubkey());

        let mut rotated_keypair = Keypair::new();
        while rotated_keypair.pubkey() == original_keypair.pubkey() {
            rotated_keypair = Keypair::new();
        }
        save_keypair(&rotated_keypair, &keypair_path).expect("save rotated keypair");

        let loaded_rotated = load_or_generate_keypair(Some(&keypair_path_string), 0, None, None)
            .expect("load rotated");
        assert_eq!(loaded_rotated.pubkey(), rotated_keypair.pubkey());
        assert_ne!(loaded_rotated.pubkey(), loaded_original.pubkey());
    }
}
