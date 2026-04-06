// Keypair generation and management

use anyhow::{bail, Context, Result};
use lichen_core::Keypair;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};

/// Keypair file format (production-ready, native PQ)
/// Supports optional at-rest encryption via LICHEN_KEYPAIR_PASSWORD env var (T1.8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeypairFile {
    /// Private key seed (32 bytes) as array of integers
    /// When encrypted, contains the encrypted seed bytes.
    #[serde(rename = "privateKey")]
    pub private_key: Vec<u8>,

    /// Full PQ verifying key bytes for quick access.
    #[serde(rename = "publicKey")]
    pub public_key: Vec<u8>,

    /// Base58-encoded compact address (standard Lichen format)
    #[serde(rename = "publicKeyBase58")]
    pub public_key_base58: String,

    /// Whether the private key is encrypted at rest (T1.8)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<bool>,

    /// Random salt for key derivation (16 bytes, present when encrypted)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salt: Option<Vec<u8>>,

    /// Encryption version: None=plaintext, Some(2)=AES-256-GCM
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption_version: Option<u8>,
}

/// T1.8 (P9-CLI-01 FIX): Derive a 32-byte encryption key from a password and salt.
/// Uses Argon2id — memory-hard KDF resistant to GPU/ASIC brute-force attacks.
/// Parameters: 19 MiB memory, 2 iterations, 1 parallelism (OWASP minimum recommendation).
fn derive_encryption_key(password: &str, salt: &[u8]) -> [u8; 32] {
    use argon2::{Algorithm, Argon2, Params, Version};

    assert!(salt.len() >= 16, "Argon2 salt must be at least 16 bytes");

    // OWASP recommended minimum: m=19456 (19 MiB), t=2, p=1
    let params = Params::new(19456, 2, 1, Some(32)).expect("valid Argon2 params");
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut output = [0u8; 32];
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut output)
        .expect("Argon2id key derivation failed");
    output
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

pub(crate) fn repair_key_file_permissions(path: &Path) -> Result<bool> {
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

pub(crate) fn maybe_repair_insecure_key_file_permissions(path: &Path) {
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

pub(crate) fn write_secure_file(path: &Path, contents: &[u8]) -> Result<()> {
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

pub(crate) fn copy_secure_file(source: &Path, destination: &Path) -> Result<()> {
    let contents = fs::read(source)
        .with_context(|| format!("Failed to read keypair file {}", source.display()))?;
    write_secure_file(destination, &contents)
}

impl KeypairFile {
    /// Create from Keypair
    #[allow(dead_code)]
    pub fn from_keypair(keypair: &Keypair) -> Self {
        let pubkey = keypair.pubkey();
        let public_key = keypair.public_key();
        let seed = keypair.to_seed();

        KeypairFile {
            private_key: seed.to_vec(),
            public_key: public_key.bytes,
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
        let keypair = Keypair::from_seed(&seed);
        if !self.public_key_base58.is_empty()
            && keypair.pubkey().to_base58() != self.public_key_base58
        {
            bail!("Keypair file publicKeyBase58 does not match derived PQ address");
        }

        Ok(keypair)
    }

    /// Save to file with secure permissions.
    /// If LICHEN_KEYPAIR_PASSWORD is set, encrypts the private key at rest (T1.8).
    #[allow(dead_code)]
    pub fn save(&self, path: &Path) -> Result<()> {
        let file_to_save = match std::env::var("LICHEN_KEYPAIR_PASSWORD") {
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
                eprintln!("⚠️  WARNING: LICHEN_KEYPAIR_PASSWORD not set \u{2014} keypair stored in PLAINTEXT.");
                eprintln!("   Set LICHEN_KEYPAIR_PASSWORD for encrypted storage (T1.8).");
                self.clone()
            }
        };

        let json =
            serde_json::to_string_pretty(&file_to_save).context("Failed to serialize keypair")?;
        write_secure_file(path, json.as_bytes()).context("Failed to write keypair file")?;

        Ok(())
    }

    /// Load from file.
    /// If the file is encrypted, requires LICHEN_KEYPAIR_PASSWORD to decrypt (T1.8).
    /// Repairs insecure file permissions automatically on Unix where possible.
    pub fn load(path: &Path) -> Result<Self> {
        maybe_repair_insecure_key_file_permissions(path);

        let json = fs::read_to_string(path).context("Failed to read keypair file")?;

        let mut keypair_file: KeypairFile =
            serde_json::from_str(&json).context("Failed to parse keypair file")?;

        // T1.8: Decrypt if file is encrypted
        if keypair_file.encrypted.unwrap_or(false) {
            let salt = keypair_file
                .salt
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Encrypted keypair file missing salt field"))?;

            let password = std::env::var("LICHEN_KEYPAIR_PASSWORD").map_err(|_| {
                anyhow::anyhow!(
                    "Keypair file is encrypted. Set LICHEN_KEYPAIR_PASSWORD to decrypt."
                )
            })?;

            if password.is_empty() {
                bail!("LICHEN_KEYPAIR_PASSWORD is empty \u{2014} cannot decrypt keypair");
            }

            let key = derive_encryption_key(&password, salt);

            // Decrypt using the canonical AES-256-GCM format.
            let version = keypair_file.encryption_version.ok_or_else(|| {
                anyhow::anyhow!("Encrypted keypair file missing encryption_version")
            })?;
            keypair_file.private_key = match version {
                2 => decrypt_aes_gcm(&keypair_file.private_key, &key)?,
                other => bail!("Unsupported encryption version: {}", other),
            };

            // Verify decryption by checking the derived address matches the stored address.
            if keypair_file.private_key.len() == 32 {
                let mut seed = [0u8; 32];
                seed.copy_from_slice(&keypair_file.private_key);
                let derived_address = Keypair::from_seed(&seed).pubkey().to_base58();
                if !keypair_file.public_key_base58.is_empty()
                    && derived_address != keypair_file.public_key_base58
                {
                    bail!("Decryption failed \u{2014} wrong password (address mismatch)");
                }
            }

            keypair_file.encrypted = None;
            keypair_file.salt = None;
            keypair_file.encryption_version = None;
        }

        Ok(keypair_file)
    }
}

/// Get default keypair path (~/.lichen/id.json)
#[allow(dead_code)]
pub fn default_keypair_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".lichen")
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
    println!("🔑 Generating new PQ signing keypair...");
    let keypair = Keypair::new();

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
        println!("   Address Hex:    {}", hex::encode(keypair.pubkey().0));
        println!("   Address Base58: {}", keypair_file.public_key_base58);
        println!("   PQ Public Key:  {} bytes", keypair_file.public_key.len());

        // Show compatibility info
        println!("\n🔗 Compatibility:");
        println!("   ✓ Lichen native PQ format");
        println!("   ✓ ML-DSA-65 signing key");
        println!("   ✗ Not compatible with legacy pre-PQ wallet imports");
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
    let keypair = keypair_file.to_keypair()?;

    println!("📍 Public Key: {}", keypair_file.public_key_base58);

    if formats {
        println!("\n🔍 Formats:");
        println!("   Address Base58: {}", keypair_file.public_key_base58);
        println!("   Address Hex:    {}", hex::encode(keypair.pubkey().0));
        println!("   PQ Public Key:  {} bytes", keypair_file.public_key.len());
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
            "Keypair file not found at: {}\nRun 'lichen keygen' to create one",
            keypair_path.display()
        );
    }

    let keypair_file = KeypairFile::load(&keypair_path)?;
    keypair_file.to_keypair()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn file_mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;

        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn test_derive_encryption_key_deterministic() {
        let salt = b"0123456789abcdef";
        let key1 = derive_encryption_key("password", salt);
        let key2 = derive_encryption_key("password", salt);
        assert_eq!(key1, key2);
    }

    #[test]
    fn test_derive_encryption_key_varies_with_inputs() {
        let key1 = derive_encryption_key("password", b"0123456789abcdef");
        let key2 = derive_encryption_key("password", b"fedcba9876543210");
        assert_ne!(key1, key2);
        let key3 = derive_encryption_key("other", b"0123456789abcdef");
        assert_ne!(key1, key3);
    }

    /// P9-CLI-01: Verify KDF uses Argon2id (deterministic, 32-byte output,
    /// different from a naive SHA-256 hash).
    #[test]
    fn test_kdf_is_argon2id_not_sha256() {
        let key = derive_encryption_key("test_password", b"test_salt_16!!!!");
        // Argon2id output for this input is fixed — verify it's not a raw SHA-256 hash
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(b"test_password");
        h.update(b"test_salt_16!!!!");
        let sha_hash: [u8; 32] = h.finalize().into();
        assert_ne!(key, sha_hash, "KDF output should differ from raw SHA-256");
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn test_aes_gcm_roundtrip() {
        let key = derive_encryption_key("test", b"0123456789abcdef");
        let data: Vec<u8> = (0..64).collect();
        let encrypted = encrypt_aes_gcm(&data, &key).unwrap();
        assert_ne!(encrypted, data);
        assert_eq!(encrypted.len(), 12 + data.len() + 16); // nonce + data + tag
        let decrypted = decrypt_aes_gcm(&encrypted, &key).unwrap();
        assert_eq!(decrypted, data);
    }

    #[test]
    fn test_aes_gcm_wrong_key_fails() {
        let salt = b"0123456789abcdef";
        let key1 = derive_encryption_key("password1", salt);
        let key2 = derive_encryption_key("password2", salt);
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
        assert_eq!(kf.public_key, keypair.public_key().bytes);
        assert_eq!(kf.public_key_base58, keypair.pubkey().to_base58());
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

    #[cfg(unix)]
    #[test]
    fn test_keypairfile_save_sets_owner_only_permissions() {
        std::env::remove_var("LICHEN_KEYPAIR_PASSWORD");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id.json");
        let keypair = Keypair::new();

        KeypairFile::from_keypair(&keypair).save(&path).unwrap();

        assert_eq!(file_mode(&path), 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn test_keypairfile_load_repairs_insecure_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id.json");
        let keypair = Keypair::new();

        let file = KeypairFile::from_keypair(&keypair);
        let json = serde_json::to_string_pretty(&file).unwrap();
        write_secure_file(&path, json.as_bytes()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let loaded = KeypairFile::load(&path).unwrap();

        assert_eq!(loaded.public_key_base58, keypair.pubkey().to_base58());
        assert_eq!(file_mode(&path), 0o600);
    }

    #[test]
    fn test_unsupported_encryption_version_is_rejected() {
        let keypair = Keypair::new();
        let salt = [0xABu8; 16];
        let password = "test_upgrade_password";
        let invalid_file = KeypairFile {
            private_key: keypair.to_seed().to_vec(),
            public_key: keypair.public_key().bytes,
            public_key_base58: keypair.pubkey().to_base58(),
            encrypted: Some(true),
            salt: Some(salt.to_vec()),
            encryption_version: Some(1),
        };

        // Write to temp file
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_invalid_version.json");
        let json = serde_json::to_string_pretty(&invalid_file).unwrap();
        fs::write(&path, &json).unwrap();

        std::env::set_var("LICHEN_KEYPAIR_PASSWORD", password);
        let err = KeypairFile::load(&path).unwrap_err();
        assert!(err
            .to_string()
            .contains("Unsupported encryption version: 1"));
        std::env::remove_var("LICHEN_KEYPAIR_PASSWORD");
    }
}
