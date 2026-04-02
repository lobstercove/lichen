// Lichen Core - Account Model
// Based on Solana's account model with versioned PQ addresses

use ml_dsa::{
    EncodedVerifyingKey, KeyGen, MlDsa65, Signature as MlDsaSignature,
    SigningKey as MlDsaSigningKey, VerifyingKey as MlDsaVerifyingKey, B32,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use slh_dsa::{
    Shake128f as SlhDsa128f, Signature as SlhDsaSignature, VerifyingKey as SlhDsaVerifyingKey,
};
use std::fmt;

pub const PQ_SCHEME_ML_DSA_65: u8 = 0x01;
pub const PQ_SCHEME_SLH_DSA_128F: u8 = 0x02;

pub const ML_DSA_65_PUBLIC_KEY_BYTES: usize = 1952;
pub const ML_DSA_65_SIGNATURE_BYTES: usize = 3309;
pub const SLH_DSA_128F_PUBLIC_KEY_BYTES: usize = 32;
pub const SLH_DSA_128F_SIGNATURE_BYTES: usize = 17_088;

fn serialize_pq_blob<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    if serializer.is_human_readable() {
        String::serialize(&hex::encode(bytes), serializer)
    } else {
        serializer.serialize_bytes(bytes)
    }
}

fn deserialize_pq_blob<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    if deserializer.is_human_readable() {
        let encoded = String::deserialize(deserializer)?;
        hex::decode(encoded).map_err(serde::de::Error::custom)
    } else {
        Vec::<u8>::deserialize(deserializer)
    }
}

/// Versioned 32-byte address digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Pubkey(pub [u8; 32]);

pub type Address = Pubkey;

impl AsRef<[u8]> for Pubkey {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Pubkey {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Pubkey(bytes)
    }

    /// Convert to Base58 string (native Lichen format)
    pub fn to_base58(&self) -> String {
        bs58::encode(self.0).into_string()
    }

    /// Convert to EVM-compatible hex address (0x...)
    pub fn to_evm(&self) -> String {
        use sha3::{Digest, Keccak256};
        let hash = Keccak256::digest(self.0);
        let evm_bytes = &hash[12..32]; // Last 20 bytes
        format!("0x{}", hex::encode(evm_bytes))
    }

    /// Parse from Base58 string
    pub fn from_base58(s: &str) -> Result<Self, String> {
        let bytes = bs58::decode(s)
            .into_vec()
            .map_err(|e| format!("Invalid base58: {}", e))?;

        if bytes.len() != 32 {
            return Err(format!("Invalid length: {} (expected 32)", bytes.len()));
        }

        let mut pubkey = [0u8; 32];
        pubkey.copy_from_slice(&bytes);
        Ok(Pubkey(pubkey))
    }
}

impl fmt::Display for Pubkey {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.to_base58())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PqPublicKey {
    pub scheme_version: u8,
    #[serde(
        serialize_with = "serialize_pq_blob",
        deserialize_with = "deserialize_pq_blob"
    )]
    pub bytes: Vec<u8>,
}

impl PqPublicKey {
    pub fn new(scheme_version: u8, bytes: Vec<u8>) -> Result<Self, String> {
        let key = Self {
            scheme_version,
            bytes,
        };
        key.validate()?;
        Ok(key)
    }

    pub fn validate(&self) -> Result<(), String> {
        let expected_len = match self.scheme_version {
            PQ_SCHEME_ML_DSA_65 => ML_DSA_65_PUBLIC_KEY_BYTES,
            PQ_SCHEME_SLH_DSA_128F => SLH_DSA_128F_PUBLIC_KEY_BYTES,
            other => return Err(format!("Unsupported PQ public key scheme: 0x{other:02x}")),
        };

        if self.bytes.len() != expected_len {
            return Err(format!(
                "Invalid PQ public key length for scheme 0x{:02x}: {} (expected {})",
                self.scheme_version,
                self.bytes.len(),
                expected_len
            ));
        }

        Ok(())
    }

    pub fn address(&self) -> Pubkey {
        let digest = Sha256::digest(&self.bytes);
        let mut address = [0u8; 32];
        address[0] = self.scheme_version;
        address[1..].copy_from_slice(&digest[..31]);
        Pubkey(address)
    }

    pub fn from_ml_dsa(verifying_key: &MlDsaVerifyingKey<MlDsa65>) -> Self {
        Self {
            scheme_version: PQ_SCHEME_ML_DSA_65,
            bytes: verifying_key.encode().as_slice().to_vec(),
        }
    }

    fn as_ml_dsa_verifying_key(&self) -> Option<MlDsaVerifyingKey<MlDsa65>> {
        if self.scheme_version != PQ_SCHEME_ML_DSA_65 {
            return None;
        }

        let encoded = EncodedVerifyingKey::<MlDsa65>::try_from(self.bytes.as_slice()).ok()?;
        Some(MlDsaVerifyingKey::<MlDsa65>::decode(&encoded))
    }

    fn as_slh_dsa_verifying_key(&self) -> Option<SlhDsaVerifyingKey<SlhDsa128f>> {
        if self.scheme_version != PQ_SCHEME_SLH_DSA_128F {
            return None;
        }

        SlhDsaVerifyingKey::<SlhDsa128f>::try_from(self.bytes.as_slice()).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PqSignature {
    pub scheme_version: u8,
    pub public_key: PqPublicKey,
    #[serde(
        serialize_with = "serialize_pq_blob",
        deserialize_with = "deserialize_pq_blob"
    )]
    pub sig: Vec<u8>,
}

impl PqSignature {
    pub fn new(scheme_version: u8, public_key: PqPublicKey, sig: Vec<u8>) -> Result<Self, String> {
        let signature = Self {
            scheme_version,
            public_key,
            sig,
        };
        signature.validate()?;
        Ok(signature)
    }

    pub fn validate(&self) -> Result<(), String> {
        self.public_key.validate()?;

        if self.public_key.scheme_version != self.scheme_version {
            return Err(format!(
                "PQ signature/public-key scheme mismatch: 0x{:02x} vs 0x{:02x}",
                self.scheme_version, self.public_key.scheme_version
            ));
        }

        let expected_len = match self.scheme_version {
            PQ_SCHEME_ML_DSA_65 => ML_DSA_65_SIGNATURE_BYTES,
            PQ_SCHEME_SLH_DSA_128F => SLH_DSA_128F_SIGNATURE_BYTES,
            other => return Err(format!("Unsupported PQ signature scheme: 0x{other:02x}")),
        };

        if self.sig.len() != expected_len {
            return Err(format!(
                "Invalid PQ signature length for scheme 0x{:02x}: {} (expected {})",
                self.scheme_version,
                self.sig.len(),
                expected_len
            ));
        }

        Ok(())
    }

    pub fn signer_address(&self) -> Pubkey {
        self.public_key.address()
    }

    fn as_ml_dsa_signature(&self) -> Option<MlDsaSignature<MlDsa65>> {
        if self.scheme_version != PQ_SCHEME_ML_DSA_65 {
            return None;
        }

        MlDsaSignature::<MlDsa65>::try_from(self.sig.as_slice()).ok()
    }

    fn as_slh_dsa_signature(&self) -> Option<SlhDsaSignature<SlhDsa128f>> {
        if self.scheme_version != PQ_SCHEME_SLH_DSA_128F {
            return None;
        }

        SlhDsaSignature::<SlhDsa128f>::try_from(self.sig.as_slice()).ok()
    }

    #[cfg(test)]
    pub fn test_fixture(fill: u8) -> Self {
        let public_key = PqPublicKey {
            scheme_version: PQ_SCHEME_ML_DSA_65,
            bytes: vec![fill; ML_DSA_65_PUBLIC_KEY_BYTES],
        };
        Self {
            scheme_version: PQ_SCHEME_ML_DSA_65,
            public_key,
            sig: vec![fill; ML_DSA_65_SIGNATURE_BYTES],
        }
    }
}

/// ML-DSA-65 keypair for signing native Lichen transactions.
pub struct Keypair {
    keypair: MlDsaSigningKey<MlDsa65>,
    seed: [u8; 32],
}

impl Keypair {
    /// Generate new random keypair
    pub fn new() -> Self {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).expect("Failed to generate random seed");
        Self::from_seed(&seed)
    }

    /// Alias for new() - generates random keypair
    pub fn generate() -> Self {
        Self::new()
    }

    /// Get secret key bytes (for serialization)
    pub fn secret_key(&self) -> &[u8; 32] {
        &self.seed
    }

    pub fn secret(&self) -> &[u8; 32] {
        &self.seed
    }

    /// Create from seed bytes
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let pq_seed = match B32::try_from(seed.as_slice()) {
            Ok(seed) => seed,
            Err(_) => unreachable!("ML-DSA seed length must be 32 bytes"),
        };
        let keypair = MlDsa65::from_seed(&pq_seed);
        Keypair {
            keypair,
            seed: *seed,
        }
    }

    /// Get account address.
    pub fn pubkey(&self) -> Pubkey {
        self.public_key().address()
    }

    /// Get the full PQ public key used for verification.
    pub fn public_key(&self) -> PqPublicKey {
        let verifying_key =
            <MlDsaSigningKey<MlDsa65> as ml_dsa::signature::Keypair>::verifying_key(&self.keypair);
        PqPublicKey::from_ml_dsa(&verifying_key)
    }

    /// Get seed bytes (for saving to file)
    pub fn to_seed(&self) -> [u8; 32] {
        self.seed
    }

    /// Sign message with ML-DSA-65 and embed the verifying key.
    pub fn sign(&self, message: &[u8]) -> PqSignature {
        let signature = self
            .keypair
            .signing_key()
            .sign_deterministic(message, &[])
            .expect("ML-DSA-65 deterministic signing failed");

        PqSignature::new(
            PQ_SCHEME_ML_DSA_65,
            self.public_key(),
            signature.encode().as_slice().to_vec(),
        )
        .expect("ML-DSA-65 signature encoding produced invalid output")
    }

    /// Verify a native PQ signature against an address.
    pub fn verify(address: &Pubkey, message: &[u8], signature: &PqSignature) -> bool {
        if signature.validate().is_err() {
            return false;
        }

        if signature.signer_address() != *address {
            return false;
        }

        match signature.scheme_version {
            PQ_SCHEME_ML_DSA_65 => {
                let verifying_key = match signature.public_key.as_ml_dsa_verifying_key() {
                    Some(verifying_key) => verifying_key,
                    None => return false,
                };
                let ml_signature = match signature.as_ml_dsa_signature() {
                    Some(signature) => signature,
                    None => return false,
                };
                verifying_key.verify_with_context(message, &[], &ml_signature)
            }
            PQ_SCHEME_SLH_DSA_128F => {
                let verifying_key = match signature.public_key.as_slh_dsa_verifying_key() {
                    Some(verifying_key) => verifying_key,
                    None => return false,
                };
                let slh_signature = match signature.as_slh_dsa_signature() {
                    Some(signature) => signature,
                    None => return false,
                };
                slh_dsa::signature::Verifier::verify(&verifying_key, message, &slh_signature)
                    .is_ok()
            }
            _ => false,
        }
    }
}

impl Default for Keypair {
    fn default() -> Self {
        Self::new()
    }
}

/// Account structure with balance separation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    /// Total balance in spores (1 LICN = 1_000_000_000 spores)
    /// Total = spendable + staked + locked
    pub spores: u64,

    /// Spendable balance (available for transfers)
    #[serde(default)]
    pub spendable: u64,

    /// Staked balance (locked in validator staking)
    #[serde(default)]
    pub staked: u64,

    /// Locked balance (locked in contracts, escrow, multisig)
    #[serde(default)]
    pub locked: u64,

    /// Arbitrary data storage
    pub data: Vec<u8>,

    /// Optional cache of the first PQ public key observed for this account.
    #[serde(default)]
    pub public_key: Option<PqPublicKey>,

    /// Program that owns this account
    pub owner: Pubkey,

    /// Is this account an executable program?
    pub executable: bool,

    /// Last epoch when rent was assessed
    pub rent_epoch: u64,

    /// Whether this account is dormant (excluded from active state root)
    #[serde(default)]
    pub dormant: bool,

    /// Consecutive epochs where rent could not be fully paid
    #[serde(default)]
    pub missed_rent_epochs: u64,
}

impl Account {
    /// M11 fix: repair legacy accounts where spendable/staked/locked are all 0 but spores > 0.
    /// This happens when deserializing accounts created before the balance separation fields existed.
    pub fn fixup_legacy(&mut self) {
        if self.spores > 0 && self.spendable == 0 && self.staked == 0 && self.locked == 0 {
            self.spendable = self.spores;
        }
    }

    /// Convert LICN to spores
    pub const fn licn_to_spores(licn: u64) -> u64 {
        licn.saturating_mul(1_000_000_000)
    }

    /// Convert spores to LICN (integer division — truncates fractional LICN).
    /// AUDIT-FIX 3.2: Callers needing rounding should use
    /// `(spores + 999_999_999) / 1_000_000_000` for round-up.
    pub const fn spores_to_licn(spores: u64) -> u64 {
        spores / 1_000_000_000
    }

    /// Create a new account with LICN balance (all spendable)
    pub fn new(licn: u64, owner: Pubkey) -> Self {
        let spores = Self::licn_to_spores(licn);
        Account {
            spores,
            spendable: spores, // All balance is spendable initially
            staked: 0,
            locked: 0,
            data: Vec::new(),
            public_key: None,
            owner,
            executable: false,
            rent_epoch: 0,
            dormant: false,
            missed_rent_epochs: 0,
        }
    }

    /// Stake some balance (moves from spendable to staked)
    /// T3.3 fix: spores total is unchanged (just a reclassification)
    /// AUDIT-FIX 1.1a: checked arithmetic, compute-before-mutate
    pub fn stake(&mut self, amount: u64) -> Result<(), String> {
        // AUDIT-FIX 3.1: Skip no-op zero-amount operations
        if amount == 0 {
            return Ok(());
        }
        let new_spendable = self.spendable.checked_sub(amount).ok_or_else(|| {
            format!(
                "Insufficient spendable balance: {} < {}",
                self.spendable, amount
            )
        })?;
        let new_staked = self.staked.checked_add(amount).ok_or_else(|| {
            format!(
                "Overflow adding {} to staked balance {}",
                amount, self.staked
            )
        })?;
        self.spendable = new_spendable;
        self.staked = new_staked;
        if self.spores != self.spendable + self.staked + self.locked {
            return Err("Account invariant violated after stake".to_string());
        }
        Ok(())
    }

    /// Unstake balance (moves from staked to spendable)
    /// AUDIT-FIX 1.1b: checked arithmetic, compute-before-mutate
    pub fn unstake(&mut self, amount: u64) -> Result<(), String> {
        // AUDIT-FIX 3.1: Skip no-op zero-amount operations
        if amount == 0 {
            return Ok(());
        }
        let new_staked = self
            .staked
            .checked_sub(amount)
            .ok_or_else(|| format!("Insufficient staked balance: {} < {}", self.staked, amount))?;
        let new_spendable = self.spendable.checked_add(amount).ok_or_else(|| {
            format!(
                "Overflow adding {} to spendable balance {}",
                amount, self.spendable
            )
        })?;
        self.staked = new_staked;
        self.spendable = new_spendable;
        if self.spores != self.spendable + self.staked + self.locked {
            return Err("Account invariant violated after unstake".to_string());
        }
        Ok(())
    }

    /// Lock balance (moves from spendable to locked)
    /// AUDIT-FIX 1.1c: checked arithmetic, compute-before-mutate
    pub fn lock(&mut self, amount: u64) -> Result<(), String> {
        // AUDIT-FIX 3.1: Skip no-op zero-amount operations
        if amount == 0 {
            return Ok(());
        }
        let new_spendable = self.spendable.checked_sub(amount).ok_or_else(|| {
            format!(
                "Insufficient spendable balance: {} < {}",
                self.spendable, amount
            )
        })?;
        let new_locked = self.locked.checked_add(amount).ok_or_else(|| {
            format!(
                "Overflow adding {} to locked balance {}",
                amount, self.locked
            )
        })?;
        self.spendable = new_spendable;
        self.locked = new_locked;
        if self.spores != self.spendable + self.staked + self.locked {
            return Err("Account invariant violated after lock".to_string());
        }
        Ok(())
    }

    /// Unlock balance (moves from locked to spendable)
    /// AUDIT-FIX 1.1d: checked arithmetic, compute-before-mutate
    pub fn unlock(&mut self, amount: u64) -> Result<(), String> {
        // AUDIT-FIX 3.1: Skip no-op zero-amount operations
        if amount == 0 {
            return Ok(());
        }
        let new_locked = self
            .locked
            .checked_sub(amount)
            .ok_or_else(|| format!("Insufficient locked balance: {} < {}", self.locked, amount))?;
        let new_spendable = self.spendable.checked_add(amount).ok_or_else(|| {
            format!(
                "Overflow adding {} to spendable balance {}",
                amount, self.spendable
            )
        })?;
        self.locked = new_locked;
        self.spendable = new_spendable;
        if self.spores != self.spendable + self.staked + self.locked {
            return Err("Account invariant violated after unlock".to_string());
        }
        Ok(())
    }

    /// Add to spendable balance (for rewards, transfers)
    pub fn add_spendable(&mut self, amount: u64) -> Result<(), String> {
        let new_spores = self.spores.checked_add(amount).ok_or_else(|| {
            format!(
                "Overflow adding {} to spores balance {}",
                amount, self.spores
            )
        })?;
        let new_spendable = self.spendable.checked_add(amount).ok_or_else(|| {
            format!(
                "Overflow adding {} to spendable balance {}",
                amount, self.spendable
            )
        })?;
        self.spores = new_spores;
        self.spendable = new_spendable;
        Ok(())
    }

    /// Deduct from spendable balance (for transfers, fees)
    /// AUDIT-FIX 1.1e: checked arithmetic, compute-before-mutate
    pub fn deduct_spendable(&mut self, amount: u64) -> Result<(), String> {
        let new_spendable = self.spendable.checked_sub(amount).ok_or_else(|| {
            format!(
                "Insufficient spendable balance: {} < {}",
                self.spendable, amount
            )
        })?;
        let new_spores = self.spores.checked_sub(amount).ok_or_else(|| {
            format!(
                "Underflow subtracting {} from spores balance {}",
                amount, self.spores
            )
        })?;
        self.spendable = new_spendable;
        self.spores = new_spores;
        Ok(())
    }

    /// Deduct from locked balance (burns/removes locked collateral)
    pub fn deduct_locked(&mut self, amount: u64) -> Result<(), String> {
        let new_locked = self
            .locked
            .checked_sub(amount)
            .ok_or_else(|| format!("Insufficient locked balance: {} < {}", self.locked, amount))?;
        let new_spores = self.spores.checked_sub(amount).ok_or_else(|| {
            format!(
                "Underflow subtracting {} from spores balance {}",
                amount, self.spores
            )
        })?;
        self.locked = new_locked;
        self.spores = new_spores;
        if self.spores != self.spendable + self.staked + self.locked {
            return Err("Account invariant violated after locked deduction".to_string());
        }
        Ok(())
    }

    /// Get balance in LICN
    pub fn balance_licn(&self) -> u64 {
        Self::spores_to_licn(self.spores)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_licn_spores_conversion() {
        assert_eq!(Account::licn_to_spores(1), 1_000_000_000);
        assert_eq!(Account::licn_to_spores(100), 100_000_000_000);
        assert_eq!(Account::spores_to_licn(1_000_000_000), 1);
        assert_eq!(Account::spores_to_licn(100_000_000_000), 100);
    }

    #[test]
    fn test_dual_address_format() {
        let pubkey = Pubkey([1u8; 32]);

        // Base58 format
        let base58 = pubkey.to_base58();
        assert!(!base58.is_empty());
        println!("Base58: {}", base58);

        // EVM format
        let evm = pubkey.to_evm();
        assert!(evm.starts_with("0x"));
        assert_eq!(evm.len(), 42); // 0x + 40 hex chars
        println!("EVM: {}", evm);
    }

    #[test]
    fn test_base58_roundtrip() {
        let original = Pubkey([42u8; 32]);
        let base58 = original.to_base58();
        let parsed = Pubkey::from_base58(&base58).unwrap();
        assert_eq!(original, parsed);
    }

    #[test]
    fn test_pq_sign_and_verify_roundtrip() {
        let keypair = Keypair::new();
        let message = b"lichen-native-pq";
        let signature = keypair.sign(message);

        assert!(Keypair::verify(&keypair.pubkey(), message, &signature));
        assert!(!Keypair::verify(&Pubkey([7u8; 32]), message, &signature));
        assert!(!Keypair::verify(
            &keypair.pubkey(),
            b"different",
            &signature
        ));
    }

    #[test]
    fn test_slh_verify_roundtrip() {
        use slh_dsa::signature::Signer;

        let signing_key = slh_dsa::SigningKey::<SlhDsa128f>::slh_keygen_internal(
            &[1u8; 16], &[2u8; 16], &[3u8; 16],
        );
        let message = b"lichen-native-slh";
        let slh_signature = signing_key.sign(message);

        let public_key =
            PqPublicKey::new(PQ_SCHEME_SLH_DSA_128F, signing_key.as_ref().to_vec()).unwrap();
        let signature =
            PqSignature::new(PQ_SCHEME_SLH_DSA_128F, public_key, slh_signature.to_vec()).unwrap();

        assert!(Keypair::verify(
            &signature.signer_address(),
            message,
            &signature
        ));
        assert!(!Keypair::verify(
            &signature.signer_address(),
            b"different",
            &signature,
        ));
    }
}
