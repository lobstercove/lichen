use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

pub(crate) fn derive_solana_address(path: &str, master_seed: &str) -> Result<String, String> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(master_seed.as_bytes()).map_err(|_| "HMAC key error")?;
    mac.update(path.as_bytes());
    let seed = mac.finalize().into_bytes();
    let mut seed_bytes: [u8; 32] = seed.as_slice().try_into().map_err(|_| "seed")?;
    let signing_key = SigningKey::from_bytes(&seed_bytes);
    seed_bytes.zeroize();
    let verifying_key = signing_key.verifying_key();
    Ok(bs58::encode(verifying_key.to_bytes()).into_string())
}

pub(crate) fn derive_solana_signer(
    path: &str,
    master_seed: &str,
) -> Result<(SigningKey, [u8; 32]), String> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(master_seed.as_bytes()).map_err(|_| "HMAC key error")?;
    mac.update(path.as_bytes());
    let seed = mac.finalize().into_bytes();
    let mut seed_bytes: [u8; 32] = seed.as_slice().try_into().map_err(|_| "seed")?;
    let signing_key = SigningKey::from_bytes(&seed_bytes);
    seed_bytes.zeroize();
    let verifying_key = signing_key.verifying_key();
    Ok((signing_key, verifying_key.to_bytes()))
}

pub(crate) struct SimpleSolanaKeypair {
    pub(crate) signing_key: SigningKey,
    pub(crate) pubkey: [u8; 32],
}

impl SimpleSolanaKeypair {
    pub(crate) fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.signing_key.sign(message).to_bytes()
    }
}

pub(crate) fn derive_solana_keypair(
    path: &str,
    master_seed: &str,
) -> Result<SimpleSolanaKeypair, String> {
    let mut mac = Hmac::<Sha256>::new_from_slice(master_seed.as_bytes())
        .map_err(|_| "HMAC key error".to_string())?;
    mac.update(path.as_bytes());
    let seed = mac.finalize().into_bytes();
    let mut seed_bytes: [u8; 32] = seed.as_slice().try_into().map_err(|_| "seed")?;
    let signing_key = SigningKey::from_bytes(&seed_bytes);
    seed_bytes.zeroize();
    let pubkey = signing_key.verifying_key().to_bytes();
    Ok(SimpleSolanaKeypair {
        signing_key,
        pubkey,
    })
}

pub(crate) fn decode_solana_pubkey(value: &str) -> Result<[u8; 32], String> {
    let bytes = bs58::decode(value)
        .into_vec()
        .map_err(|error| format!("base58: {}", error))?;
    if bytes.len() != 32 {
        return Err("invalid solana pubkey length".to_string());
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

pub(crate) fn encode_solana_pubkey(value: &[u8; 32]) -> String {
    bs58::encode(value).into_string()
}

pub(crate) fn find_program_address(
    seeds: &[&[u8]],
    program_id: &[u8; 32],
) -> Result<[u8; 32], String> {
    for bump in (0u8..=255u8).rev() {
        let mut hasher = Sha256::new();
        for seed in seeds {
            hasher.update(seed);
        }
        hasher.update([bump]);
        hasher.update(program_id);
        hasher.update(b"ProgramDerivedAddress");
        let hash = hasher.finalize();
        let bytes: [u8; 32] = hash
            .as_slice()
            .try_into()
            .map_err(|_| "pda hash".to_string())?;
        if VerifyingKey::from_bytes(&bytes).is_err() {
            return Ok(bytes);
        }
    }

    Err("no viable program address".to_string())
}
