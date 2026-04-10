use crate::Keypair;
use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::io::{self, Write};
use std::path::Path;

pub const KEYPAIR_PASSWORD_ENV: &str = "LICHEN_KEYPAIR_PASSWORD";
pub const ALLOW_PLAINTEXT_KEYPAIR_ENV: &str = "LICHEN_ALLOW_PLAINTEXT_KEYPAIRS";
const LOCAL_DEV_ENV: &str = "LICHEN_LOCAL_DEV";
const KEYPAIR_ENCRYPTION_VERSION_AES_GCM: u8 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeypairFile {
    #[serde(rename = "privateKey")]
    pub private_key: Vec<u8>,
    #[serde(rename = "publicKey")]
    pub public_key: Vec<u8>,
    #[serde(rename = "publicKeyBase58")]
    pub public_key_base58: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salt: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encryption_version: Option<u8>,
}

pub fn keypair_password_from_env() -> Option<String> {
    match std::env::var(KEYPAIR_PASSWORD_ENV) {
        Ok(password) if !password.is_empty() => Some(password),
        _ => None,
    }
}

pub fn plaintext_keypair_compat_allowed() -> bool {
    matches!(
        std::env::var(ALLOW_PLAINTEXT_KEYPAIR_ENV),
        Ok(value) if is_truthy(&value)
    ) || matches!(std::env::var(LOCAL_DEV_ENV), Ok(value) if is_truthy(&value))
}

pub fn require_runtime_keypair_password(context: &str) -> Result<Option<String>, String> {
    let password = keypair_password_from_env();
    if password.is_some() || plaintext_keypair_compat_allowed() {
        return Ok(password);
    }

    Err(format!(
        "{} requires {} to be set. Plaintext keypairs are only allowed with {}=1 or {}=1.",
        context, KEYPAIR_PASSWORD_ENV, ALLOW_PLAINTEXT_KEYPAIR_ENV, LOCAL_DEV_ENV
    ))
}

fn is_truthy(value: &str) -> bool {
    matches!(
        value.trim(),
        "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
    )
}

fn derive_encryption_key(password: &str, salt: &[u8]) -> [u8; 32] {
    assert!(salt.len() >= 16, "Argon2 salt must be at least 16 bytes");

    let params = Params::new(19_456, 2, 1, Some(32)).expect("valid Argon2 params");
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut output = [0u8; 32];
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut output)
        .expect("Argon2id key derivation failed");
    output
}

fn encrypt_aes_gcm(data: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| "Invalid AES key length for keypair encryption".to_string())?;
    let mut nonce_bytes = [0u8; 12];
    getrandom::fill(&mut nonce_bytes)
        .map_err(|err| format!("Failed to generate keypair encryption nonce: {}", err))?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, data)
        .map_err(|_| "AES-GCM keypair encryption failed".to_string())?;
    let mut result = Vec::with_capacity(12 + ciphertext.len());
    result.extend_from_slice(&nonce_bytes);
    result.extend_from_slice(&ciphertext);
    Ok(result)
}

fn decrypt_aes_gcm(encrypted: &[u8], key: &[u8; 32]) -> Result<Vec<u8>, String> {
    if encrypted.len() < 28 {
        return Err("Encrypted keypair payload is too short for AES-GCM".to_string());
    }
    let nonce = Nonce::from_slice(&encrypted[..12]);
    let ciphertext = &encrypted[12..];
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|_| "Invalid AES key length for keypair decryption".to_string())?;
    cipher.decrypt(nonce, ciphertext).map_err(|_| {
        "AES-GCM keypair decryption failed - wrong password or corrupted data".to_string()
    })
}

pub fn repair_key_file_permissions(path: &Path) -> Result<bool, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(err) => {
                return Err(format!(
                    "Failed to inspect keypair file permissions {}: {}",
                    path.display(),
                    err
                ));
            }
        };

        let mode = metadata.permissions().mode() & 0o777;
        if mode & 0o077 == 0 {
            return Ok(false);
        }

        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|err| {
            format!(
                "Failed to set secure permissions on keypair file {}: {}",
                path.display(),
                err
            )
        })?;
        Ok(true)
    }
    #[cfg(not(unix))]
    {
        drop(path);
        Ok(false)
    }
}

pub fn maybe_repair_insecure_key_file_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if let Ok(metadata) = fs::metadata(path) {
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                match repair_key_file_permissions(path) {
                    Ok(true) => eprintln!(
                        "Repaired insecure permissions on keypair file {}.",
                        path.display()
                    ),
                    Ok(false) => {}
                    Err(err) => {
                        eprintln!(
                            "WARNING: Keypair file {} has insecure permissions ({:o}) and automatic repair failed: {}",
                            path.display(),
                            mode,
                            err
                        );
                        eprintln!("Run: chmod 600 {}", path.display());
                    }
                }
            }
        }
    }
}

pub fn write_secure_file(path: &Path, contents: &[u8]) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        if path.exists() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|err| {
                format!(
                    "Failed to prepare secure permissions on keypair file {}: {}",
                    path.display(),
                    err
                )
            })?;
        }

        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(|err| format!("Failed to open keypair file {}: {}", path.display(), err))?;
        file.write_all(contents)
            .map_err(|err| format!("Failed to write keypair file {}: {}", path.display(), err))?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|err| {
            format!(
                "Failed to finalize secure permissions on keypair file {}: {}",
                path.display(),
                err
            )
        })?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, contents)
            .map_err(|err| format!("Failed to write keypair file {}: {}", path.display(), err))
    }
}

pub fn copy_secure_file(source: &Path, destination: &Path) -> Result<(), String> {
    let contents = fs::read(source)
        .map_err(|err| format!("Failed to read keypair file {}: {}", source.display(), err))?;
    write_secure_file(destination, &contents)
}

impl KeypairFile {
    pub fn from_keypair(keypair: &Keypair) -> Self {
        let pubkey = keypair.pubkey();
        let public_key = keypair.public_key();
        let seed = keypair.to_seed();

        Self {
            private_key: seed.to_vec(),
            public_key: public_key.bytes,
            public_key_base58: pubkey.to_base58(),
            encrypted: None,
            salt: None,
            encryption_version: None,
        }
    }

    pub fn to_keypair(&self) -> Result<Keypair, String> {
        if self.private_key.len() != 32 {
            return Err(format!(
                "Invalid privateKey length: expected 32 bytes, got {}",
                self.private_key.len()
            ));
        }

        let mut seed = [0u8; 32];
        seed.copy_from_slice(&self.private_key);
        let keypair = Keypair::from_seed(&seed);
        seed.fill(0);

        if !self.public_key.is_empty() && keypair.public_key().bytes != self.public_key {
            return Err(
                "Keypair file publicKey does not match the derived PQ verifying key".to_string(),
            );
        }
        if !self.public_key_base58.is_empty()
            && keypair.pubkey().to_base58() != self.public_key_base58
        {
            return Err(
                "Keypair file publicKeyBase58 does not match derived PQ address".to_string(),
            );
        }

        Ok(keypair)
    }

    fn storage_form(
        &self,
        password: Option<&str>,
        require_encryption: bool,
    ) -> Result<Self, String> {
        match password {
            Some(password) if !password.is_empty() => {
                let mut salt = [0u8; 16];
                getrandom::fill(&mut salt).map_err(|err| {
                    format!("Failed to generate keypair encryption salt: {}", err)
                })?;
                let mut key = derive_encryption_key(password, &salt);
                let encrypted_pk = encrypt_aes_gcm(&self.private_key, &key)?;
                key.fill(0);
                Ok(Self {
                    private_key: encrypted_pk,
                    public_key: self.public_key.clone(),
                    public_key_base58: self.public_key_base58.clone(),
                    encrypted: Some(true),
                    salt: Some(salt.to_vec()),
                    encryption_version: Some(KEYPAIR_ENCRYPTION_VERSION_AES_GCM),
                })
            }
            Some(_) => Err(format!(
                "{} is set but empty - cannot encrypt keypair file",
                KEYPAIR_PASSWORD_ENV
            )),
            None if require_encryption => Err(format!(
                "Keypair storage requires {} to be set",
                KEYPAIR_PASSWORD_ENV
            )),
            None => Ok(self.clone()),
        }
    }

    fn to_storage_object(
        &self,
        password: Option<&str>,
        require_encryption: bool,
    ) -> Result<Map<String, Value>, String> {
        let storage = self.storage_form(password, require_encryption)?;
        let value = serde_json::to_value(storage)
            .map_err(|err| format!("Failed to serialize keypair file: {}", err))?;
        value
            .as_object()
            .cloned()
            .ok_or_else(|| "Serialized keypair file was not a JSON object".to_string())
    }

    pub fn save_with_password_and_metadata(
        &self,
        path: &Path,
        password: Option<&str>,
        require_encryption: bool,
        extra_fields: &Map<String, Value>,
    ) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                format!(
                    "Failed to create keypair directory {}: {}",
                    parent.display(),
                    err
                )
            })?;
        }

        let mut object = self.to_storage_object(password, require_encryption)?;
        for (key, value) in extra_fields {
            if object.contains_key(key) {
                return Err(format!(
                    "Keypair metadata field '{}' collides with canonical keypair field",
                    key
                ));
            }
            object.insert(key.clone(), value.clone());
        }

        let json = serde_json::to_string_pretty(&Value::Object(object))
            .map_err(|err| format!("Failed to encode keypair JSON {}: {}", path.display(), err))?;
        write_secure_file(path, json.as_bytes())
    }

    pub fn save_with_password(
        &self,
        path: &Path,
        password: Option<&str>,
        require_encryption: bool,
    ) -> Result<(), String> {
        self.save_with_password_and_metadata(path, password, require_encryption, &Map::new())
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let password = keypair_password_from_env();
        if password.is_none() {
            eprintln!(
                "WARNING: {} not set - keypair stored in plaintext.",
                KEYPAIR_PASSWORD_ENV
            );
            eprintln!("Set {} for encrypted storage.", KEYPAIR_PASSWORD_ENV);
        }
        self.save_with_password(path, password.as_deref(), false)
    }

    pub fn load_with_password_policy(
        path: &Path,
        password: Option<&str>,
        allow_plaintext: bool,
    ) -> Result<Self, String> {
        maybe_repair_insecure_key_file_permissions(path);

        let json = fs::read_to_string(path)
            .map_err(|err| format!("Failed to read keypair file {}: {}", path.display(), err))?;
        let mut keypair_file: KeypairFile = serde_json::from_str(&json)
            .map_err(|err| format!("Failed to parse keypair file {}: {}", path.display(), err))?;

        if keypair_file.encrypted.unwrap_or(false) {
            let salt = keypair_file.salt.as_ref().ok_or_else(|| {
                format!(
                    "Encrypted keypair file {} is missing the salt field",
                    path.display()
                )
            })?;
            let password = match password {
                Some(password) if !password.is_empty() => password,
                Some(_) => {
                    return Err(format!(
                        "Encrypted keypair file {} requires a non-empty {}",
                        path.display(),
                        KEYPAIR_PASSWORD_ENV
                    ));
                }
                None => {
                    return Err(format!(
                        "Encrypted keypair file {} requires {} to be set",
                        path.display(),
                        KEYPAIR_PASSWORD_ENV
                    ));
                }
            };

            let mut key = derive_encryption_key(password, salt);
            let version = keypair_file.encryption_version.ok_or_else(|| {
                format!(
                    "Encrypted keypair file {} is missing encryption_version",
                    path.display()
                )
            })?;
            keypair_file.private_key = match version {
                KEYPAIR_ENCRYPTION_VERSION_AES_GCM => {
                    let decrypted = decrypt_aes_gcm(&keypair_file.private_key, &key)?;
                    key.fill(0);
                    decrypted
                }
                other => {
                    key.fill(0);
                    return Err(format!(
                        "Unsupported keypair encryption version {} in {}",
                        other,
                        path.display()
                    ));
                }
            };
            keypair_file.encrypted = None;
            keypair_file.salt = None;
            keypair_file.encryption_version = None;
            keypair_file.to_keypair()?;
            return Ok(keypair_file);
        }

        if !allow_plaintext {
            return Err(format!(
                "Plaintext keypair file {} is not allowed outside explicit local development. Set {} or use {}=1 / {}=1 for compatibility.",
                path.display(),
                KEYPAIR_PASSWORD_ENV,
                ALLOW_PLAINTEXT_KEYPAIR_ENV,
                LOCAL_DEV_ENV
            ));
        }

        keypair_file.to_keypair()?;
        Ok(keypair_file)
    }

    pub fn load_with_password(path: &Path, password: Option<&str>) -> Result<Self, String> {
        Self::load_with_password_policy(path, password, true)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let password = keypair_password_from_env();
        Self::load_with_password(path, password.as_deref())
    }
}

pub fn save_keypair_with_password(
    keypair: &Keypair,
    path: &Path,
    password: Option<&str>,
    require_encryption: bool,
) -> Result<(), String> {
    KeypairFile::from_keypair(keypair).save_with_password(path, password, require_encryption)
}

pub fn save_keypair_with_metadata(
    keypair: &Keypair,
    path: &Path,
    password: Option<&str>,
    require_encryption: bool,
    extra_fields: &Map<String, Value>,
) -> Result<(), String> {
    KeypairFile::from_keypair(keypair).save_with_password_and_metadata(
        path,
        password,
        require_encryption,
        extra_fields,
    )
}

pub fn load_keypair(path: &Path) -> Result<Keypair, String> {
    KeypairFile::load(path)?.to_keypair()
}

pub fn load_keypair_with_password_policy(
    path: &Path,
    password: Option<&str>,
    allow_plaintext: bool,
) -> Result<Keypair, String> {
    KeypairFile::load_with_password_policy(path, password, allow_plaintext)?.to_keypair()
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
    fn test_save_requires_password_when_encryption_is_required() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id.json");
        let keypair = Keypair::new();
        let err = KeypairFile::from_keypair(&keypair)
            .save_with_password(&path, None, true)
            .unwrap_err();
        assert!(err.contains(KEYPAIR_PASSWORD_ENV));
    }

    #[test]
    fn test_encrypted_keypair_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("encrypted.json");
        let keypair = Keypair::new();

        KeypairFile::from_keypair(&keypair)
            .save_with_password(&path, Some("correct horse battery staple"), true)
            .unwrap();

        let json: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json["encrypted"], Value::Bool(true));
        assert_eq!(json["encryption_version"], Value::from(2u8));

        let loaded = KeypairFile::load_with_password_policy(
            &path,
            Some("correct horse battery staple"),
            false,
        )
        .unwrap();
        assert_eq!(loaded.to_keypair().unwrap().pubkey(), keypair.pubkey());
    }

    #[test]
    fn test_wrong_password_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("encrypted.json");
        let keypair = Keypair::new();

        KeypairFile::from_keypair(&keypair)
            .save_with_password(&path, Some("correct"), true)
            .unwrap();

        let err = KeypairFile::load_with_password_policy(&path, Some("wrong"), false).unwrap_err();
        assert!(err.contains("decryption failed"));
    }

    #[test]
    fn test_metadata_save_preserves_extra_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("with-metadata.json");
        let keypair = Keypair::new();
        let mut metadata = Map::new();
        metadata.insert("role".to_string(), Value::String("primary".to_string()));
        metadata.insert(
            "chain_id".to_string(),
            Value::String("lichen-testnet-1".to_string()),
        );

        save_keypair_with_metadata(&keypair, &path, Some("pw"), true, &metadata).unwrap();

        let json: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json["role"], Value::String("primary".to_string()));
        assert_eq!(
            json["chain_id"],
            Value::String("lichen-testnet-1".to_string())
        );
        assert_eq!(json["encrypted"], Value::Bool(true));
    }

    #[cfg(unix)]
    #[test]
    fn test_load_repairs_insecure_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("id.json");
        let keypair = Keypair::new();

        KeypairFile::from_keypair(&keypair)
            .save_with_password(&path, None, false)
            .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let loaded = KeypairFile::load_with_password_policy(&path, None, true).unwrap();

        assert_eq!(loaded.public_key_base58, keypair.pubkey().to_base58());
        assert_eq!(file_mode(&path), 0o600);
    }
}
