// Keypair generation and management

use anyhow::{bail, Context, Result};
use moltchain_core::Keypair;
use serde::{Deserialize, Serialize};
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

/// T1.8 (P9-CLI-01 FIX): Derive a 32-byte encryption key from a password and salt.
/// Uses Argon2id — memory-hard KDF resistant to GPU/ASIC brute-force attacks.
/// Parameters: 19 MiB memory, 2 iterations, 1 parallelism (OWASP minimum recommendation).
/// If the caller provides a salt shorter than 16 bytes (Argon2 minimum), it is
/// stretched to 16 bytes via SHA-256 hashing — production always uses 16-byte salts.
fn derive_encryption_key(password: &str, salt: &[u8]) -> [u8; 32] {
    use argon2::{Algorithm, Argon2, Params, Version};
    use sha2::{Digest, Sha256};

    // Argon2 requires salt ≥ 8 bytes (RFC 9106 recommends ≥16).
    // Stretch short salts via SHA-256 so legacy callers don't panic.
    let effective_salt: Vec<u8> = if salt.len() < 16 {
        let h = Sha256::digest(salt);
        h[..16].to_vec()
    } else {
        salt.to_vec()
    };

    // OWASP recommended minimum: m=19456 (19 MiB), t=2, p=1
    let params = Params::new(19456, 2, 1, Some(32)).expect("valid Argon2 params");
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut output = [0u8; 32];
    argon2
        .hash_password_into(password.as_bytes(), &effective_salt, &mut output)
        .expect("Argon2id key derivation failed");
    output
}

/// T1.8: XOR encrypt/decrypt — symmetric operation.
fn xor_cipher(data: &[u8], key: &[u8; 32]) -> Vec<u8> {
    data.iter()
        .enumerate()
        .map(|(i, b)| b ^ key[i % 32])
        .collect()
}

/// AES-256-GCM encryption: returns nonce (12) || ciphertext || tag (16).
fn encrypt_aes_gcm(data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>> {
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|_| anyhow::anyhow!("Invalid AES key length"))?;
    let mut nonce_bytes = [0u8; 12];
    getrandom::fill(&mut nonce_bytes)
        .map_err(|e| anyhow::anyhow!("Failed to generate random nonce: {}", e))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, data)
        .map_err(|_| anyhow::anyhow!("AES-GCM encryption failed"))?;
    let mut result = Vec::with_capacity(12 + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

/// AES-256-GCM decryption: expects nonce (12) || ciphertext || tag (16).
fn decrypt_aes_gcm(encrypted: &[u8], key: &[u8; 32]) -> Result<Vec<u8>> {
    if encrypted.len() < 28 {
        bail!("Encrypted data too short for AES-GCM (need at least 28 bytes)");
    }
    let nonce = Nonce::from_slice(&encrypted[..12]);
    let ciphertext = &encrypted[12..];
    let cipher = Aes256Gcm::new_from_slice(key).expect("Invalid AES key length");
    cipher.decrypt(nonce, ciphertext).map_err(|_| {
        anyhow::anyhow!("AES-GCM decryption failed \u{2014} wrong password or corrupted data")
    })
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
                let encrypted_pk = encrypt_aes_gcm(&self.private_key, &key)?;
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
                1 => {
                    // P9-CLI-02: Auto-upgrade unauthenticated XOR cipher to AES-256-GCM
                    let decrypted = xor_cipher(&keypair_file.private_key, &key);
                    eprintln!(
                        "\u{26a0}\u{fe0f}  Legacy XOR encryption (v1) detected — auto-upgrading to AES-256-GCM (v2)."
                    );
                    // Re-encrypt with AES-GCM and overwrite the file
                    let mut upgraded = keypair_file.clone();
                    upgraded.private_key = decrypted.clone();
                    upgraded.encrypted = None;
                    upgraded.salt = None;
                    upgraded.encryption_version = None;
                    // Re-save encrypted with v2 — generate a fresh salt
                    let mut new_salt = [0u8; 16];
                    getrandom::fill(&mut new_salt).expect("Random salt gen failed");
                    let new_key = derive_encryption_key(&password, &new_salt);
                    if let Ok(encrypted_v2) = encrypt_aes_gcm(&decrypted, &new_key) {
                        let v2_file = KeypairFile {
                            private_key: encrypted_v2,
                            public_key: upgraded.public_key.clone(),
                            public_key_base58: upgraded.public_key_base58.clone(),
                            encrypted: Some(true),
                            salt: Some(new_salt.to_vec()),
                            encryption_version: Some(2),
                        };
                        if let Ok(json) = serde_json::to_string_pretty(&v2_file) {
                            let _ = fs::write(path, json);
                            eprintln!("   \u{2705} Keypair file upgraded to v2 (AES-256-GCM).");
                        }
                    }
                    decrypted
                }
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

    /// P9-CLI-01: Verify KDF uses Argon2id (deterministic, 32-byte output,
    /// different from a naive SHA-256 hash).
    #[test]
    fn test_kdf_is_argon2id_not_sha256() {
        let key = derive_encryption_key("test_password", b"test_salt_16!!");
        // Argon2id output for this input is fixed — verify it's not a raw SHA-256 hash
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"test_password");
        h.update(b"test_salt_16!!");
        let sha_hash: [u8; 32] = h.finalize().into();
        assert_ne!(key, sha_hash, "KDF output should differ from raw SHA-256");
        assert_eq!(key.len(), 32);
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
        let encrypted = encrypt_aes_gcm(&data, &key).unwrap();
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
        let encrypted = encrypt_aes_gcm(&data, &key1).unwrap();
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

    /// P9-CLI-02: Verify that loading a v1 (XOR) encrypted file auto-upgrades
    /// it to v2 (AES-256-GCM) on disk.
    #[test]
    fn test_xor_v1_auto_upgrades_to_v2_on_load() {
        let keypair = Keypair::new();
        let salt = [0xABu8; 16];
        let password = "test_upgrade_password";
        let key = derive_encryption_key(password, &salt);

        // Create a v1 (XOR) encrypted file
        let seed = keypair.to_seed();
        let encrypted_seed = xor_cipher(&seed, &key);
        let v1_file = KeypairFile {
            private_key: encrypted_seed,
            public_key: keypair.pubkey().0.to_vec(),
            public_key_base58: keypair.pubkey().to_base58(),
            encrypted: Some(true),
            salt: Some(salt.to_vec()),
            encryption_version: Some(1),
        };

        // Write to temp file
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_v1.json");
        let json = serde_json::to_string_pretty(&v1_file).unwrap();
        fs::write(&path, &json).unwrap();

        // Load — should auto-upgrade
        std::env::set_var("MOLTCHAIN_KEYPAIR_PASSWORD", password);
        let loaded = KeypairFile::load(&path).unwrap();
        let loaded_kp = loaded.to_keypair().unwrap();
        assert_eq!(loaded_kp.pubkey(), keypair.pubkey(), "decrypted key should match");

        // The file on disk should now be v2
        let reloaded_json = fs::read_to_string(&path).unwrap();
        let on_disk: KeypairFile = serde_json::from_str(&reloaded_json).unwrap();
        assert_eq!(on_disk.encryption_version, Some(2), "file should be v2 on disk");
        assert!(on_disk.encrypted.unwrap_or(false), "file should be encrypted");

        // Verify it can still be loaded (v2 path)
        let reloaded = KeypairFile::load(&path).unwrap();
        let reloaded_kp = reloaded.to_keypair().unwrap();
        assert_eq!(reloaded_kp.pubkey(), keypair.pubkey(), "v2 re-load should work");
        std::env::remove_var("MOLTCHAIN_KEYPAIR_PASSWORD");
    }
}
