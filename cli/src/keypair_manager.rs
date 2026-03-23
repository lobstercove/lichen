// Keypair file management for CLI

use anyhow::{Context, Result};
use lichen_core::Keypair;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Production keypair format (Solana-compatible)
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
        let seed = keypair.to_seed();

        let keypair_file = KeypairFile {
            private_key: seed.to_vec(),
            public_key: pubkey.0.to_vec(),
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

    /// Load keypair from file
    ///
    /// Supports multiple formats:
    ///   1. KeypairFile  — `{ "privateKey": [u8 array], "publicKey": [...], "publicKeyBase58": "..." }`
    ///   2. Hex strings  — `{ "privateKey": "hex...", ... }` (wallet-create format)
    ///   3. Flat array   — `[172, 31, 143, ...]` (faucet-keypair / Solana-style)
    ///   4. secretKey    — `{ "secretKey": [64 bytes], ... }` (browser wallet export)
    pub fn load_keypair(&self, path: &Path) -> Result<Keypair> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("Failed to read keypair file: {}", path.display()))?;

        // --- Format 1: canonical KeypairFile (int-array privateKey) ---
        if let Ok(keypair_file) = serde_json::from_str::<KeypairFile>(&contents) {
            if keypair_file.private_key.len() == 32 {
                let mut seed = [0u8; 32];
                seed.copy_from_slice(&keypair_file.private_key);
                return Ok(Keypair::from_seed(&seed));
            }
        }

        let json: serde_json::Value =
            serde_json::from_str(&contents).context("Failed to parse keypair file as JSON")?;

        // --- Format 2: hex-string privateKey or secret_key (wallet create / genesis keys) ---
        if let Some(hex_str) = json
            .get("privateKey")
            .or_else(|| json.get("secret_key"))
            .and_then(|v| v.as_str())
        {
            let seed_bytes = hex::decode(hex_str)
                .context("Failed to hex-decode privateKey/secret_key string")?;
            if seed_bytes.len() != 32 {
                anyhow::bail!(
                    "Invalid hex privateKey length: expected 32 bytes, got {}",
                    seed_bytes.len()
                );
            }
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&seed_bytes);
            return Ok(Keypair::from_seed(&seed));
        }

        // --- Format 4: secretKey (browser wallet / extension export, 64-byte ed25519) ---
        if let Some(arr) = json.get("secretKey").and_then(|v| v.as_array()) {
            let bytes: Vec<u8> = arr
                .iter()
                .filter_map(|v| v.as_u64().map(|n| n as u8))
                .collect();
            if bytes.len() >= 32 {
                let mut seed = [0u8; 32];
                seed.copy_from_slice(&bytes[..32]);
                return Ok(Keypair::from_seed(&seed));
            }
        }

        // --- Format 3: flat byte array [172, 31, 143, ...] ---
        if let Some(arr) = json.as_array() {
            let bytes: Vec<u8> = arr
                .iter()
                .filter_map(|v| v.as_u64().map(|n| n as u8))
                .collect();
            if bytes.len() >= 32 {
                let mut seed = [0u8; 32];
                seed.copy_from_slice(&bytes[..32]);
                return Ok(Keypair::from_seed(&seed));
            }
        }

        anyhow::bail!(
            "Unsupported keypair format in {}. Expected one of: \
             KeypairFile (int-array), hex-string privateKey, flat byte array, \
             or secretKey (64-byte ed25519)",
            path.display()
        )
    }

    /// Save seed to file (helper for keypair generation)
    #[allow(dead_code)]
    pub fn save_seed(&self, seed: &[u8; 32], path: &Path) -> Result<()> {
        let keypair = Keypair::from_seed(seed);
        self.save_keypair(&keypair, path)
    }
}
