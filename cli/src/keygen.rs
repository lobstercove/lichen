// Keypair generation and management

use anyhow::{bail, Context, Result};
use moltchain_core::Keypair;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};

/// Keypair file format (production-ready, Solana-compatible)
/// Supports optional at-rest encryption via MOLTCHAIN_KEYPAIR_PASSWORD env var (T1.8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeypairFile {
    /// Private key seed (32 bytes) as array of integers
    /// Compatible with Solana wallet format.
    /// When encrypted, contains the encrypted seed bytes.
    #[serde(rename = "privateKey")]
    pub private_key: Vec<u8>,

    /// Public key (32 bytes) for quick access
    #[serde(rename = "publicKey")]
    pub public_key: Vec<u8>,

    /// Base58-encoded public key (standard format)
    #[serde(rename = "publicKeyBase58")]
    pub public_key_base58: String,

    /// Whether the private key is encrypted at rest (T1.8)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<bool>,

    /// Random salt for key derivation (16 bytes, present when encrypted)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salt: Option<Vec<u8>>,

    /// Encryption version: None=plaintext, Some(1)=XOR (legacy), Some(2)=AES-256-GCM
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption_version: Option<u8>,
}

/// T1.8: Derive a 32-byte encryption key from a password and salt.
/// Uses 100,000 iterations of SHA-256 for key stretching to resist brute-force.
fn derive_encryption_key(password: &str, salt: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(password.as_bytes());
    hasher.update(salt);
    let mut hash = hasher.finalize();

    for _ in 0..100_000 {
        let mut h = Sha256::new();
        h.update(hash);
        h.update(password.as_bytes());
        hash = h.finalize();
    }

    let mut key = [0u8; 32];
    key.copy_from_slice(&hash);
    key
}

/// T1.8: XOR encrypt/decrypt — symmetric operation.
fn xor_cipher(data: &[u8], key: &[u8; 32]) -> Vec<u8> {
    data.iter()
        .enumerate()
        .map(|(i, b)| b ^ key[i % 32])
        .collect()
}

/// AES-256-GCM encryption: returns nonce (12) || ciphertext || tag (16).
fn encrypt_aes_gcm(data: &[u8], key: &[u8; 32]) -> Vec<u8> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("Invalid AES key length");
    let mut nonce_bytes = [0u8; 12];
    getrandom::fill(&mut nonce_bytes).expect("Failed to generate random nonce");
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, data)
        .expect("AES-GCM encryption failed");
    let mut result = Vec::with_capacity(12 + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    result
}

/// AES-256-GCM decryption: expects nonce (12) || ciphertext || tag (16).
fn decrypt_aes_gcm(encrypted: &[u8], key: &[u8; 32]) -> Result<Vec<u8>> {
    if encrypted.len() < 28 {
        bail!("Encrypted data too short for AES-GCM (need at least 28 bytes)");
    }
    let nonce = Nonce::from_slice(&encrypted[..12]);
    let ciphertext = &encrypted[12..];
    let cipher = Aes256Gcm::new_from_slice(key).expect("Invalid AES key length");
    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("AES-GCM decryption failed \u{2014} wrong password or corrupted data"))
}

impl KeypairFile {
    /// Create from Keypair
    #[allow(dead_code)]
    pub fn from_keypair(keypair: &Keypair) -> Self {
        let pubkey = keypair.pubkey();
        let seed = keypair.to_seed();

        KeypairFile {
            private_key: seed.to_vec(),
            public_key: pubkey.0.to_vec(),
            public_key_base58: pubkey.to_base58(),
            encrypted: None,
            salt: None,
            encryption_version: None,
        }
    }

    /// Convert to Keypair
    pub fn to_keypair(&self) -> Result<Keypair> {
        if self.private_key.len() != 32 {
            bail!("Invalid private key length: expected 32 bytes");
        }

        let mut seed = [0u8; 32];
        seed.copy_from_slice(&self.private_key);

        Ok(Keypair::from_seed(&seed))
    }

    /// Save to file with secure permissions.
    /// If MOLTCHAIN_KEYPAIR_PASSWORD is set, encrypts the private key at rest (T1.8).
    #[allow(dead_code)]
    pub fn save(&self, path: &Path) -> Result<()> {
        let file_to_save = match std::env::var("MOLTCHAIN_KEYPAIR_PASSWORD") {
            Ok(password) if !password.is_empty() => {
                // T1.8: Encrypt private key at rest using password-derived key
                let mut salt = [0u8; 16];
                getrandom::fill(&mut salt).expect("Failed to generate random salt");
                let key = derive_encryption_key(&password, &salt);
                let encrypted_pk = encrypt_aes_gcm(&self.private_key, &key);
                KeypairFile {
                    private_key: encrypted_pk,
                    public_key: self.public_key.clone(),
                    public_key_base58: self.public_key_base58.clone(),
                    encrypted: Some(true),
                    salt: Some(salt.to_vec()),
                    encryption_version: Some(2),
                }
            }
            _ => {
                eprintln!("\u{26a0}\u{fe0f}  WARNING: MOLTCHAIN_KEYPAIR_PASSWORD not set \u{2014} keypair stored in PLAINTEXT.");
                eprintln!("   Set MOLTCHAIN_KEYPAIR_PASSWORD for encrypted storage (T1.8).");
                self.clone()
            }
        };

        let json =
            serde_json::to_string_pretty(&file_to_save).context("Failed to serialize keypair")?;

        // Write to file
        fs::write(path, &json).context("Failed to write keypair file")?;

        // Set secure permissions (Unix: 600, Windows: equivalent)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let permissions = fs::Permissions::from_mode(0o600);
            fs::set_permissions(path, permissions).context("Failed to set file permissions")?;
        }

        Ok(())
    }

    /// Load from file.
    /// If the file is encrypted, requires MOLTCHAIN_KEYPAIR_PASSWORD to decrypt (T1.8).
    /// Warns if file permissions are too open on Unix systems.
    pub fn load(path: &Path) -> Result<Self> {
        // T1.8: Check file permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = fs::metadata(path) {
                let mode = metadata.permissions().mode() & 0o777;
                if mode & 0o077 != 0 {
                    eprintln!(
                        "\u{26a0}\u{fe0f}  WARNING: Keypair file {} has insecure permissions ({:o}).",
                        path.display(), mode
                    );
                    eprintln!("   Run: chmod 600 {}", path.display());
                }
            }
        }

        let json = fs::read_to_string(path).context("Failed to read keypair file")?;

        let mut keypair_file: KeypairFile =
            serde_json::from_str(&json).context("Failed to parse keypair file")?;

        // T1.8: Decrypt if file is encrypted
        if keypair_file.encrypted.unwrap_or(false) {
            let salt = keypair_file
                .salt
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Encrypted keypair file missing salt field"))?;

            let password = std::env::var("MOLTCHAIN_KEYPAIR_PASSWORD").map_err(|_| {
                anyhow::anyhow!(
                    "Keypair file is encrypted. Set MOLTCHAIN_KEYPAIR_PASSWORD to decrypt."
                )
            })?;

            if password.is_empty() {
                bail!("MOLTCHAIN_KEYPAIR_PASSWORD is empty \u{2014} cannot decrypt keypair");
            }

            let key = derive_encryption_key(&password, salt);

            // Decrypt based on encryption version (v1=XOR legacy, v2=AES-256-GCM)
            let version = keypair_file.encryption_version.unwrap_or(1);
            keypair_file.private_key = match version {
                1 => xor_cipher(&keypair_file.private_key, &key),
                2 => decrypt_aes_gcm(&keypair_file.private_key, &key)?,
                other => bail!("Unknown encryption version: {}", other),
            };

            // Verify decryption by checking derived public key matches stored public key
            if keypair_file.private_key.len() == 32 {
                let mut seed = [0u8; 32];
                seed.copy_from_slice(&keypair_file.private_key);
                let derived_pubkey = Keypair::from_seed(&seed).pubkey();
                if derived_pubkey.0.to_vec() != keypair_file.public_key {
                    bail!("Decryption failed \u{2014} wrong password (public key mismatch)");
                }
            }

            keypair_file.encrypted = None;
            keypair_file.salt = None;
            keypair_file.encryption_version = None;
        }

        Ok(keypair_file)
    }
}

/// Get default keypair path (~/.moltchain/id.json)
#[allow(dead_code)]
pub fn default_keypair_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".moltchain")
        .join("id.json")
}

/// Execute keygen command
#[allow(dead_code)]
pub fn execute(outfile: Option<PathBuf>, force: bool, show_formats: bool) -> Result<()> {
    let output_path = outfile.unwrap_or_else(default_keypair_path);

    // Check if file already exists
    if output_path.exists() && !force {
        println!(
            "⚠️  Keypair file already exists at: {}",
            output_path.display()
        );
        print!("Overwrite? (y/N): ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("❌ Aborted");
            return Ok(());
        }
    }

    // Create parent directory if it doesn't exist
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).context("Failed to create directory")?;
    }

    // Generate new keypair
    println!("🔑 Generating new Ed25519 keypair...");
    let keypair = Keypair::new();
    let _pubkey = keypair.pubkey();

    // Create keypair file
    let keypair_file = KeypairFile::from_keypair(&keypair);

    // Save to file
    keypair_file.save(&output_path)?;

    // Display results
    println!("\n✅ Keypair generated successfully!");
    println!("\n📍 Public Key (Base58):");
    println!("   {}", keypair_file.public_key_base58);
    println!("\n💾 Saved to: {}", output_path.display());
    println!("   Permissions: 600 (owner read/write only)");

    if show_formats {
        println!("\n🔍 Key Formats:");
        println!("   Hex:    {}", hex::encode(&keypair_file.public_key));
        println!("   Base58: {}", keypair_file.public_key_base58);

        // Show compatibility info
        println!("\n🔗 Compatibility:");
        println!("   ✓ MoltChain native format");
        println!("   ✓ Solana-compatible (Ed25519 + Base58)");
        println!("   ✓ Can be imported to Phantom, Solflare");
        println!("\n   Note: Ethereum uses secp256k1, not Ed25519");
        println!("         Direct ETH import not supported");
    }

    println!("\n⚠️  Keep your keypair file secure!");
    println!("   Never share your private key");
    println!("   Backup this file in a safe location");

    Ok(())
}

/// Show public key from keypair file
#[allow(dead_code)]
pub fn show_pubkey(keypair_path: PathBuf, formats: bool) -> Result<()> {
    let keypair_file = KeypairFile::load(&keypair_path)?;

    println!("📍 Public Key: {}", keypair_file.public_key_base58);

    if formats {
        println!("\n🔍 Formats:");
        println!("   Base58: {}", keypair_file.public_key_base58);
        println!("   Hex:    {}", hex::encode(&keypair_file.public_key));
        println!("   Bytes:  {:?}", keypair_file.public_key);
    }

    Ok(())
}

/// Load keypair from file path or use default
#[allow(dead_code)]
pub fn load_keypair(path: Option<&Path>) -> Result<Keypair> {
    let keypair_path = path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(default_keypair_path);

    if !keypair_path.exists() {
        bail!(
            "Keypair file not found at: {}\nRun 'moltchain keygen' to create one",
            keypair_path.display()
        );
    }

    let keypair_file = KeypairFile::load(&keypair_path)?;
    keypair_file.to_keypair()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_encryption_key_deterministic() {
        let key1 = derive_encryption_key("password", b"salt");
        let key2 = derive_encryption_key("password", b"salt");
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_derive_encryption_key_varies_with_inputs() {
        let key1 = derive_encryption_key("password", b"salt");
        let key2 = derive_encryption_key("password", b"other");
        assert_ne!(key1, key2);
        let key3 = derive_encryption_key("other", b"salt");
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_xor_cipher_roundtrip() {
        let key = derive_encryption_key("test", b"salt");
        let data: Vec<u8> = (0..32).collect();
        let encrypted = xor_cipher(&data, &key);
        assert_ne!(encrypted, data);
        let decrypted = xor_cipher(&encrypted, &key);
        assert_eq!(decrypted, data);
    }

    #[test]
    fn test_aes_gcm_roundtrip() {
        let key = derive_encryption_key("test", b"salt");
        let data: Vec<u8> = (0..64).collect();
        let encrypted = encrypt_aes_gcm(&data, &key);
        assert_ne!(encrypted, data);
        assert_eq!(encrypted.len(), 12 + data.len() + 16); // nonce + data + tag
        let decrypted = decrypt_aes_gcm(&encrypted, &key).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn test_aes_gcm_wrong_key_fails() {
        let key1 = derive_encryption_key("password1", b"salt");
        let key2 = derive_encryption_key("password2", b"salt");
        let data = vec![42u8; 32];
        let encrypted = encrypt_aes_gcm(&data, &key1);
        let result = decrypt_aes_gcm(&encrypted, &key2);
        assert!(result.is_err());
    }

    #[test]
    fn test_keypairfile_from_keypair_fields() {
        let keypair = Keypair::new();
        let kf = KeypairFile::from_keypair(&keypair);
        assert_eq!(kf.private_key.len(), 32);
        assert_eq!(kf.public_key.len(), 32);
        assert!(!kf.public_key_base58.is_empty());
        assert!(kf.encrypted.is_none());
        assert!(kf.salt.is_none());
        assert!(kf.encryption_version.is_none());
    }

    #[test]
    fn test_keypairfile_to_keypair_roundtrip() {
        let keypair = Keypair::new();
        let kf = KeypairFile::from_keypair(&keypair);
        let restored = kf.to_keypair().unwrap();
        assert_eq!(restored.pubkey(), keypair.pubkey());
    }
}
