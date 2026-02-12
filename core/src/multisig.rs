// MoltChain Multi-Signature Wallet Support

use crate::{Keypair, Pubkey};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Multi-signature configuration for accounts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiSigConfig {
    /// Required number of signatures
    pub threshold: u8,
    /// List of authorized signers
    pub signers: Vec<Pubkey>,
    /// Account type flags
    pub is_genesis: bool,
    pub is_treasury: bool,
}

impl MultiSigConfig {
    pub fn new(threshold: u8, signers: Vec<Pubkey>) -> Self {
        MultiSigConfig {
            threshold,
            signers,
            is_genesis: false,
            is_treasury: false,
        }
    }

    pub fn genesis_treasury(threshold: u8, signers: Vec<Pubkey>) -> Self {
        MultiSigConfig {
            threshold,
            signers,
            is_genesis: true,
            is_treasury: true,
        }
    }

    /// Verify that enough signers have signed
    pub fn verify_threshold(&self, signed_by: &[Pubkey]) -> bool {
        if signed_by.len() < self.threshold as usize {
            return false;
        }

        // Check all signers are authorized
        signed_by.iter().all(|signer| self.signers.contains(signer))
    }
}

/// Genesis wallet keypair bundle (saved to disk)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisWallet {
    /// Genesis treasury public key
    pub pubkey: Pubkey,
    /// Primary keypair (saved encrypted)
    pub keypair_path: String,
    /// Treasury public key
    #[serde(skip_serializing_if = "Option::is_none")]
    pub treasury_pubkey: Option<Pubkey>,
    /// Treasury keypair path
    #[serde(skip_serializing_if = "Option::is_none")]
    pub treasury_keypair_path: Option<String>,
    /// Multi-sig configuration
    pub multisig: Option<MultiSigConfig>,
    /// Creation timestamp
    pub created_at: String,
    /// Chain ID this wallet belongs to
    pub chain_id: String,
}

impl GenesisWallet {
    /// Generate new genesis wallet with multi-sig support
    pub fn generate(
        chain_id: &str,
        is_mainnet: bool,
        signer_count: usize,
    ) -> Result<(Self, Vec<Keypair>, Keypair), String> {
        // Generate primary keypair
        let primary_keypair = Keypair::generate();
        let pubkey = primary_keypair.pubkey();

        // Generate additional signers for multi-sig
        let mut all_keypairs = vec![primary_keypair];
        let mut signer_pubkeys = vec![pubkey];

        for _ in 1..signer_count {
            let kp = Keypair::generate();
            signer_pubkeys.push(kp.pubkey());
            all_keypairs.push(kp);
        }

        // Determine threshold based on mainnet vs testnet
        let threshold = if is_mainnet {
            // Mainnet: require 3/5 signatures (60% quorum)
            ((signer_count as f64 * 0.6).ceil() as u8).max(3)
        } else {
            // Testnet: require 2/3 signatures (production-ready for testing)
            if signer_count >= 3 {
                2 // Fixed 2-of-3 multi-sig for testnet
            } else {
                1 // Single signer fallback
            }
        };

        let multisig = if signer_count > 1 {
            Some(MultiSigConfig::genesis_treasury(
                threshold,
                signer_pubkeys.clone(),
            ))
        } else {
            None
        };

        let treasury_keypair = Keypair::generate();
        let treasury_pubkey = treasury_keypair.pubkey();

        let wallet = GenesisWallet {
            pubkey,
            keypair_path: format!(".moltchain/genesis-wallet-{}.json", chain_id),
            treasury_pubkey: Some(treasury_pubkey),
            treasury_keypair_path: Some(format!(".moltchain/treasury-wallet-{}.json", chain_id)),
            multisig,
            created_at: chrono::Utc::now().to_rfc3339(),
            chain_id: chain_id.to_string(),
        };

        Ok((wallet, all_keypairs, treasury_keypair))
    }

    /// Save genesis wallet info to disk
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize wallet: {}", e))?;

        fs::write(path, json).map_err(|e| format!("Failed to write wallet file: {}", e))?;

        Ok(())
    }

    /// Load genesis wallet from disk
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let json =
            fs::read_to_string(path).map_err(|e| format!("Failed to read wallet file: {}", e))?;

        serde_json::from_str(&json).map_err(|e| format!("Failed to parse wallet: {}", e))
    }

    /// Save all keypairs to separate files (encrypted in production)
    pub fn save_keypairs<P: AsRef<Path>>(
        keypairs: &[Keypair],
        base_path: P,
        chain_id: &str,
    ) -> Result<Vec<String>, String> {
        let mut paths = Vec::new();

        for (i, keypair) in keypairs.iter().enumerate() {
            let role = if i == 0 {
                "primary"
            } else {
                &format!("signer-{}", i)
            };
            let filename = format!("genesis-{}-{}.json", role, chain_id);
            let path = base_path.as_ref().join(&filename);

            // In production, encrypt with passphrase
            // For now, save as JSON
            let keypair_json = serde_json::json!({
                "pubkey": keypair.pubkey().to_base58(),
                "secret_key": hex::encode(keypair.secret()),
                "role": role,
                "chain_id": chain_id,
                "created_at": chrono::Utc::now().to_rfc3339(),
                "warning": "KEEP THIS FILE SECURE - CONTROLS GENESIS TREASURY",
            });

            fs::write(&path, serde_json::to_string_pretty(&keypair_json).unwrap())
                .map_err(|e| format!("Failed to write keypair: {}", e))?;

            paths.push(path.to_string_lossy().to_string());
        }

        Ok(paths)
    }

    /// Save treasury keypair to disk
    pub fn save_treasury_keypair<P: AsRef<Path>>(
        keypair: &Keypair,
        base_path: P,
        chain_id: &str,
    ) -> Result<String, String> {
        let filename = format!("treasury-{}.json", chain_id);
        let path = base_path.as_ref().join(&filename);
        let keypair_json = serde_json::json!({
            "pubkey": keypair.pubkey().to_base58(),
            "secret_key": hex::encode(keypair.secret()),
            "role": "treasury",
            "chain_id": chain_id,
            "created_at": chrono::Utc::now().to_rfc3339(),
            "warning": "KEEP THIS FILE SECURE - CONTROLS TREASURY",
        });

        fs::write(&path, serde_json::to_string_pretty(&keypair_json).unwrap())
            .map_err(|e| format!("Failed to write treasury keypair: {}", e))?;

        Ok(path.to_string_lossy().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_testnet_wallet() {
        let (wallet, keypairs, _treasury_keypair) =
            GenesisWallet::generate("testnet-1", false, 1).unwrap();
        assert_eq!(keypairs.len(), 1);
        assert!(wallet.multisig.is_none()); // Single sig for testnet
    }

    #[test]
    fn test_generate_mainnet_wallet() {
        let (wallet, keypairs, _treasury_keypair) =
            GenesisWallet::generate("mainnet-1", true, 5).unwrap();
        assert_eq!(keypairs.len(), 5);
        assert!(wallet.multisig.is_some());

        let multisig = wallet.multisig.unwrap();
        assert_eq!(multisig.threshold, 3); // 3/5
        assert_eq!(multisig.signers.len(), 5);
        assert!(multisig.is_genesis);
        assert!(multisig.is_treasury);
    }

    #[test]
    fn test_multisig_verification() {
        let keypairs: Vec<_> = (0..5).map(|_| Keypair::generate()).collect();
        let pubkeys: Vec<_> = keypairs.iter().map(|k| k.pubkey()).collect();

        let multisig = MultiSigConfig::genesis_treasury(3, pubkeys.clone());

        // Test valid threshold
        assert!(multisig.verify_threshold(&pubkeys[0..3]));
        assert!(multisig.verify_threshold(&pubkeys[0..5]));

        // Test invalid threshold
        assert!(!multisig.verify_threshold(&pubkeys[0..2]));

        // Test unauthorized signer
        let unauthorized = Keypair::generate().pubkey();
        assert!(!multisig.verify_threshold(&[unauthorized, pubkeys[0], pubkeys[1]]));
    }
}
