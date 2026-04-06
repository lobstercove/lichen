// Wallet operations - Full wallet management for Lichen CLI
// Create, import, list, and manage multiple wallets

use anyhow::{Context, Result};
use lichen_core::Keypair;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Wallet metadata stored in wallet index
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletInfo {
    pub name: String,
    pub address: String,
    pub keypair_path: PathBuf,
    pub created_at: u64,
}

/// Wallet manager for handling multiple wallets
pub struct WalletManager {
    wallets_dir: PathBuf,
    index_path: PathBuf,
}

impl WalletManager {
    /// Create a new wallet manager
    pub fn new() -> Result<Self> {
        let home = std::env::var("HOME").context("HOME environment variable not set")?;

        let wallets_dir = PathBuf::from(home).join(".lichen").join("wallets");

        let index_path = wallets_dir.join("index.json");

        // Create wallets directory if it doesn't exist
        fs::create_dir_all(&wallets_dir)?;

        Ok(Self {
            wallets_dir,
            index_path,
        })
    }

    /// Load wallet index from disk
    fn load_index(&self) -> Result<HashMap<String, WalletInfo>> {
        if !self.index_path.exists() {
            return Ok(HashMap::new());
        }

        let contents = fs::read_to_string(&self.index_path)?;
        let index: HashMap<String, WalletInfo> = serde_json::from_str(&contents)?;
        Ok(index)
    }

    /// Save wallet index to disk
    fn save_index(&self, index: &HashMap<String, WalletInfo>) -> Result<()> {
        let contents = serde_json::to_string_pretty(index)?;
        fs::write(&self.index_path, contents)?;
        Ok(())
    }

    /// Create a new wallet
    pub fn create_wallet(&self, name: Option<String>) -> Result<WalletInfo> {
        let mut index = self.load_index()?;

        // Generate unique wallet name
        let wallet_name = match name {
            Some(n) => {
                if index.contains_key(&n) {
                    anyhow::bail!("Wallet '{}' already exists", n);
                }
                n
            }
            None => {
                let mut counter = 1;
                loop {
                    let name = format!("wallet-{}", counter);
                    if !index.contains_key(&name) {
                        break name;
                    }
                    counter += 1;
                }
            }
        };

        // Generate new keypair
        let keypair = Keypair::new();
        let address = keypair.pubkey().to_base58();

        // Save keypair to file using KeypairFile (supports encryption via LICHEN_KEYPAIR_PASSWORD)
        let keypair_path = self.wallets_dir.join(format!("{}.json", wallet_name));
        let keypair_file = crate::keygen::KeypairFile::from_keypair(&keypair);
        keypair_file.save(&keypair_path)?;

        // Create wallet info
        let wallet_info = WalletInfo {
            name: wallet_name.clone(),
            address: address.clone(),
            keypair_path: keypair_path.clone(),
            created_at: current_timestamp(),
        };

        // Add to index
        index.insert(wallet_name, wallet_info.clone());
        self.save_index(&index)?;

        println!("✅ Wallet created successfully!");
        println!("\n   Name:    {}", wallet_info.name);
        println!("   Address: {}", address);
        println!("   Path:    {}", keypair_path.display());
        println!("\n⚠️  Keep your keypair file safe! Anyone with access can control your funds.");

        Ok(wallet_info)
    }

    /// Import wallet from keypair file
    pub fn import_wallet(&self, name: String, keypair_path: PathBuf) -> Result<WalletInfo> {
        let mut index = self.load_index()?;

        if index.contains_key(&name) {
            anyhow::bail!("Wallet '{}' already exists", name);
        }

        // Load and validate keypair
        let keypair = load_keypair_from_file(&keypair_path)?;
        let address = keypair.pubkey().to_base58();

        // Copy keypair to wallets directory with owner-only permissions.
        let new_keypair_path = self.wallets_dir.join(format!("{}.json", name));
        crate::keygen::copy_secure_file(&keypair_path, &new_keypair_path)?;

        // Create wallet info
        let wallet_info = WalletInfo {
            name: name.clone(),
            address: address.clone(),
            keypair_path: new_keypair_path,
            created_at: current_timestamp(),
        };

        // Add to index
        index.insert(name, wallet_info.clone());
        self.save_index(&index)?;

        println!("✅ Wallet imported successfully!");
        println!("\n   Name:    {}", wallet_info.name);
        println!("   Address: {}", address);

        Ok(wallet_info)
    }

    /// List all wallets
    pub fn list_wallets(&self) -> Result<()> {
        let index = self.load_index()?;

        if index.is_empty() {
            println!("No wallets found. Create one with: lichen wallet create");
            return Ok(());
        }

        println!("📋 Lichen Wallets\n");

        let mut wallets: Vec<_> = index.values().collect();
        wallets.sort_by_key(|w| w.created_at);

        for wallet in wallets {
            println!("   🦞 {}", wallet.name);
            println!("      Address: {}", wallet.address);
            println!("      File:    {}", wallet.keypair_path.display());
            println!();
        }

        println!("Total: {} wallet(s)", index.len());

        Ok(())
    }

    /// Get wallet by name
    pub fn get_wallet(&self, name: &str) -> Result<WalletInfo> {
        let index = self.load_index()?;
        index
            .get(name)
            .cloned()
            .context(format!("Wallet '{}' not found", name))
    }

    /// Remove wallet
    pub fn remove_wallet(&self, name: &str) -> Result<()> {
        let mut index = self.load_index()?;

        let wallet = index
            .remove(name)
            .context(format!("Wallet '{}' not found", name))?;

        // Delete keypair file
        if wallet.keypair_path.exists() {
            fs::remove_file(&wallet.keypair_path)?;
        }

        self.save_index(&index)?;

        println!("✅ Wallet '{}' removed", name);
        Ok(())
    }

    /// Show wallet details
    pub fn show_wallet(&self, name: &str) -> Result<()> {
        let wallet = self.get_wallet(name)?;

        println!("🦞 Wallet Details\n");
        println!("   Name:    {}", wallet.name);
        println!("   Address: {}", wallet.address);
        println!("   Path:    {}", wallet.keypair_path.display());
        println!("   Created: {}", format_timestamp(wallet.created_at));

        Ok(())
    }
}

/// Load keypair from file — delegates to the canonical keygen loader.
fn load_keypair_from_file(path: &Path) -> Result<Keypair> {
    let keypair_file = crate::keygen::KeypairFile::load(path)
        .with_context(|| format!("Failed to load canonical keypair from {}", path.display()))?;
    keypair_file.to_keypair()
}

/// Get current Unix timestamp
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Format timestamp as human-readable date
fn format_timestamp(timestamp: u64) -> String {
    use chrono::{DateTime, Utc};
    let dt = DateTime::<Utc>::from_timestamp(timestamp as i64, 0);
    match dt {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
        None => "Unknown".to_string(),
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
    fn test_import_wallet_copies_keypair_with_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let wallets_dir = dir.path().join("wallets");
        fs::create_dir_all(&wallets_dir).unwrap();

        let manager = WalletManager {
            wallets_dir: wallets_dir.clone(),
            index_path: wallets_dir.join("index.json"),
        };

        let source_path = dir.path().join("source.json");
        let keypair = Keypair::new();
        let file = crate::keygen::KeypairFile::from_keypair(&keypair);
        let json = serde_json::to_string_pretty(&file).unwrap();
        crate::keygen::write_secure_file(&source_path, json.as_bytes()).unwrap();
        fs::set_permissions(&source_path, fs::Permissions::from_mode(0o644)).unwrap();

        let wallet = manager
            .import_wallet("alice".to_string(), source_path.clone())
            .unwrap();

        assert_eq!(file_mode(&source_path), 0o600);
        assert_eq!(file_mode(&wallet.keypair_path), 0o600);
        assert_eq!(wallet.address, keypair.pubkey().to_base58());
    }
}
