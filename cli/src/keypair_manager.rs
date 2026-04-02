// Keypair file management for CLI

use anyhow::{Context, Result};
use lichen_core::Keypair;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Production keypair format (native PQ seed + full verifying key)
#[derive(Serialize, Deserialize)]
struct KeypairFile {
    #[serde(rename = "privateKey")]
    private_key: Vec<u8>,

    #[serde(rename = "publicKey")]
    public_key: Vec<u8>,

    #[serde(rename = "publicKeyBase58")]
    public_key_base58: String,
}

pub struct KeypairManager;

impl KeypairManager {
    pub fn new() -> Self {
        KeypairManager
    }

    /// Get default keypair directory (~/.lichen/keypairs/)
    #[allow(dead_code)]
    pub fn default_keypair_dir(&self) -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".lichen").join("keypairs")
    }

    /// Get default keypair path (~/.lichen/keypairs/id.json)
    #[allow(dead_code)]
    pub fn default_keypair_path(&self) -> PathBuf {
        self.default_keypair_dir().join("id.json")
    }

    /// Save keypair to file
    pub fn save_keypair(&self, keypair: &Keypair, path: &Path) -> Result<()> {
        let pubkey = keypair.pubkey();
        let public_key = keypair.public_key();
        let seed = keypair.to_seed();

        let keypair_file = KeypairFile {
            private_key: seed.to_vec(),
            public_key: public_key.bytes,
            public_key_base58: pubkey.to_base58(),
        };

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        let json =
            serde_json::to_string_pretty(&keypair_file).context("Failed to serialize keypair")?;

        fs::write(path, json)
            .with_context(|| format!("Failed to write keypair file: {}", path.display()))?;

        // Set file permissions to user-only (0600)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .context("Failed to set keypair file permissions")?;
        }

        Ok(())
    }

    /// Load keypair from the canonical keypair file format.
    pub fn load_keypair(&self, path: &Path) -> Result<Keypair> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("Failed to read keypair file: {}", path.display()))?;

        let keypair_file = serde_json::from_str::<KeypairFile>(&contents).with_context(|| {
            format!(
                "Unsupported keypair format in {}. Expected the canonical KeypairFile JSON shape",
                path.display()
            )
        })?;

        if keypair_file.private_key.len() != 32 {
            anyhow::bail!(
                "Invalid privateKey length in {}: expected 32 bytes, got {}",
                path.display(),
                keypair_file.private_key.len()
            );
        }

        let mut seed = [0u8; 32];
        seed.copy_from_slice(&keypair_file.private_key);
        let keypair = Keypair::from_seed(&seed);
        if !keypair_file.public_key_base58.is_empty()
            && keypair.pubkey().to_base58() != keypair_file.public_key_base58
        {
            anyhow::bail!("Keypair file publicKeyBase58 does not match derived PQ address");
        }

        Ok(keypair)
    }

    /// Save seed to file (helper for keypair generation)
    #[allow(dead_code)]
    pub fn save_seed(&self, seed: &[u8; 32], path: &Path) -> Result<()> {
        let keypair = Keypair::from_seed(seed);
        self.save_keypair(&keypair, path)
    }
}
