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

/// Load validator keypair from file or generate new one
pub fn load_or_generate_keypair(config_path: Option<&str>, p2p_port: u16) -> Result<Keypair> {
    // Determine keypair file path
    let keypair_path = if let Some(path) = config_path {
        PathBuf::from(path)
    } else {
        default_validator_keypair_path(p2p_port)
    };

    // Try to load existing keypair
    if keypair_path.exists() {
        info!(
            "📁 Loading validator keypair from: {}",
            keypair_path.display()
        );
        load_keypair(&keypair_path)
    } else {
        warn!("⚠️  No keypair found at: {}", keypair_path.display());
        info!("🔑 Generating new validator keypair...");

        // Generate new keypair
        let keypair = Keypair::new();

        // Save for future use
        if let Err(e) = save_keypair(&keypair, &keypair_path) {
            warn!("Failed to save keypair: {}. Will use in-memory only.", e);
        } else {
            info!("💾 Saved validator keypair to: {}", keypair_path.display());
        }

        Ok(keypair)
    }
}

/// Get default validator keypair path
pub fn default_validator_keypair_path(p2p_port: u16) -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".moltchain")
        .join("validators")
        .join(format!("validator-{}.json", p2p_port))
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

        let loaded_original =
            load_or_generate_keypair(Some(&keypair_path_string), 0).expect("load original");
        assert_eq!(loaded_original.pubkey(), original_keypair.pubkey());

        let mut rotated_keypair = Keypair::new();
        while rotated_keypair.pubkey() == original_keypair.pubkey() {
            rotated_keypair = Keypair::new();
        }
        save_keypair(&rotated_keypair, &keypair_path).expect("save rotated keypair");

        let loaded_rotated =
            load_or_generate_keypair(Some(&keypair_path_string), 0).expect("load rotated");
        assert_eq!(loaded_rotated.pubkey(), rotated_keypair.pubkey());
        assert_ne!(loaded_rotated.pubkey(), loaded_original.pubkey());
    }
}
