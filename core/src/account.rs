// MoltChain Core - Account Model
// Based on Solana's account model with dual address support

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Ed25519 public key (32 bytes)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Pubkey(pub [u8; 32]);

impl AsRef<[u8]> for Pubkey {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Pubkey {
    pub const fn new(bytes: [u8; 32]) -> Self {
        Pubkey(bytes)
    }

    /// Convert to Base58 string (native MoltChain format)
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

/// Ed25519 Keypair for signing
pub struct Keypair {
    signing_key: SigningKey,
    seed: [u8; 32],
}

impl Keypair {
    /// Generate new random keypair
    pub fn new() -> Self {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed).expect("Failed to generate random seed");
        let signing_key = SigningKey::from_bytes(&seed);
        Keypair { signing_key, seed }
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
        let signing_key = SigningKey::from_bytes(seed);
        Keypair {
            signing_key,
            seed: *seed,
        }
    }

    /// Get public key
    pub fn pubkey(&self) -> Pubkey {
        let verifying_key = self.signing_key.verifying_key();
        Pubkey(verifying_key.to_bytes())
    }

    /// Get seed bytes (for saving to file)
    pub fn to_seed(&self) -> [u8; 32] {
        self.seed
    }

    /// Sign message
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        let signature: Signature = self.signing_key.sign(message);
        signature.to_bytes()
    }

    /// Verify signature
    pub fn verify(pubkey: &Pubkey, message: &[u8], signature: &[u8; 64]) -> bool {
        match VerifyingKey::from_bytes(&pubkey.0) {
            Ok(verifying_key) => {
                let sig = Signature::from_bytes(signature);
                verifying_key.verify(message, &sig).is_ok()
            }
            Err(_) => false,
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
    /// Total balance in shells (1 MOLT = 1_000_000_000 shells)
    /// Total = spendable + staked + locked
    pub shells: u64,

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
    /// M11 fix: repair legacy accounts where spendable/staked/locked are all 0 but shells > 0.
    /// This happens when deserializing accounts created before the balance separation fields existed.
    pub fn fixup_legacy(&mut self) {
        if self.shells > 0 && self.spendable == 0 && self.staked == 0 && self.locked == 0 {
            self.spendable = self.shells;
        }
    }

    /// Convert MOLT to shells
    pub const fn molt_to_shells(molt: u64) -> u64 {
        molt.saturating_mul(1_000_000_000)
    }

    /// Convert shells to MOLT (integer division — truncates fractional MOLT).
    /// AUDIT-FIX 3.2: Callers needing rounding should use
    /// `(shells + 999_999_999) / 1_000_000_000` for round-up.
    pub const fn shells_to_molt(shells: u64) -> u64 {
        shells / 1_000_000_000
    }

    /// Create a new account with MOLT balance (all spendable)
    pub fn new(molt: u64, owner: Pubkey) -> Self {
        let shells = Self::molt_to_shells(molt);
        Account {
            shells,
            spendable: shells, // All balance is spendable initially
            staked: 0,
            locked: 0,
            data: Vec::new(),
            owner,
            executable: false,
            rent_epoch: 0,
            dormant: false,
            missed_rent_epochs: 0,
        }
    }

    /// Stake some balance (moves from spendable to staked)
    /// T3.3 fix: shells total is unchanged (just a reclassification)
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
        if self.shells != self.spendable + self.staked + self.locked {
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
        if self.shells != self.spendable + self.staked + self.locked {
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
        if self.shells != self.spendable + self.staked + self.locked {
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
        if self.shells != self.spendable + self.staked + self.locked {
            return Err("Account invariant violated after unlock".to_string());
        }
        Ok(())
    }

    /// Add to spendable balance (for rewards, transfers)
    pub fn add_spendable(&mut self, amount: u64) -> Result<(), String> {
        self.shells = self.shells.checked_add(amount).ok_or_else(|| {
            format!(
                "Overflow adding {} to shells balance {}",
                amount, self.shells
            )
        })?;
        self.spendable = self.spendable.checked_add(amount).ok_or_else(|| {
            // Roll back shells on spendable overflow
            self.shells -= amount;
            format!(
                "Overflow adding {} to spendable balance {}",
                amount, self.spendable
            )
        })?;
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
        let new_shells = self.shells.checked_sub(amount).ok_or_else(|| {
            format!(
                "Underflow subtracting {} from shells balance {}",
                amount, self.shells
            )
        })?;
        self.spendable = new_spendable;
        self.shells = new_shells;
        Ok(())
    }

    /// Get balance in MOLT
    pub fn balance_molt(&self) -> u64 {
        Self::shells_to_molt(self.shells)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_molt_shells_conversion() {
        assert_eq!(Account::molt_to_shells(1), 1_000_000_000);
        assert_eq!(Account::molt_to_shells(100), 100_000_000_000);
        assert_eq!(Account::shells_to_molt(1_000_000_000), 1);
        assert_eq!(Account::shells_to_molt(100_000_000_000), 100);
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
}
