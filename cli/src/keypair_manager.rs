// Keypair file management for CLI

use anyhow::{Context, Result};
use lichen_core::Keypair;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
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

fn repair_key_file_permissions(path: &Path) -> Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(err) => {
                return Err(err).with_context(|| {
                    format!(
                        "Failed to inspect keypair file permissions: {}",
                        path.display()
                    )
                });
            }
        };

        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 == 0 {
            return Ok(false);
        }

        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("Failed to set secure permissions on {}", path.display()))?;
        Ok(true)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(false)
    }
}

fn maybe_repair_insecure_key_file_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if let Ok(metadata) = fs::metadata(path) {
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                match repair_key_file_permissions(path) {
                    Ok(true) => eprintln!(
                        "🔒 Repaired insecure permissions on keypair file {}.",
                        path.display()
                    ),
                    Ok(false) => {}
                    Err(err) => {
                        eprintln!(
                            "⚠️  WARNING: Keypair file {} has insecure permissions ({:o}) and automatic repair failed: {}",
                            path.display(), mode, err
                        );
                        eprintln!("   Run: chmod 600 {}", path.display());
                    }
                }
            }
        }
    }
}

fn write_secure_file(path: &Path, contents: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        if path.exists() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).with_context(|| {
                format!("Failed to prepare secure permissions on {}", path.display())
            })?;
        }

        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("Failed to open keypair file {}", path.display()))?;
        file.write_all(contents)
            .with_context(|| format!("Failed to write keypair file {}", path.display()))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).with_context(|| {
            format!(
                "Failed to finalize secure permissions on {}",
                path.display()
            )
        })?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, contents)
            .with_context(|| format!("Failed to write keypair file {}", path.display()))?;
        Ok(())
    }
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

        write_secure_file(path, json.as_bytes())
            .with_context(|| format!("Failed to write keypair file: {}", path.display()))?;

        Ok(())
    }

    /// Load keypair from the canonical keypair file format.
    pub fn load_keypair(&self, path: &Path) -> Result<Keypair> {
        maybe_repair_insecure_key_file_permissions(path);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn file_mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;

        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn test_save_keypair_sets_owner_only_permissions() {
        let manager = KeypairManager::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id.json");
        let keypair = Keypair::new();

        manager.save_keypair(&keypair, &path).unwrap();

        assert_eq!(file_mode(&path), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn test_load_keypair_repairs_insecure_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let manager = KeypairManager::new();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id.json");
        let keypair = Keypair::new();

        manager.save_keypair(&keypair, &path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let loaded = manager.load_keypair(&path).unwrap();

        assert_eq!(loaded.pubkey(), keypair.pubkey());
        assert_eq!(file_mode(&path), 0o600);
    }
}
