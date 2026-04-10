// Keypair file management for CLI

use anyhow::{Context, Result};
use lichen_core::{Keypair, KeypairFile};
use std::fs;
use std::path::{Path, PathBuf};

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
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
        }

        KeypairFile::from_keypair(keypair)
            .save(path)
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("Failed to write keypair file: {}", path.display()))?;

        Ok(())
    }
    /// Load keypair from the canonical keypair file format.
    pub fn load_keypair(&self, path: &Path) -> Result<Keypair> {
        let keypair_file = KeypairFile::load(path)
            .map_err(anyhow::Error::msg)
            .with_context(|| {
                format!(
                "Unsupported keypair format in {}. Expected the canonical KeypairFile JSON shape",
                path.display()
            )
            })?;

        keypair_file.to_keypair().map_err(anyhow::Error::msg)
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
