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
        // C6 fix: deduplicate to prevent same key counted multiple times
        let unique: std::collections::HashSet<&Pubkey> = signed_by.iter().collect();
        if unique.len() < self.threshold as usize {
            return false;
        }

        // Check all signers are authorized
        unique.iter().all(|signer| self.signers.contains(signer))
    }
}

// ============================================================================
// GOVERNED WALLET SYSTEM
// ============================================================================

/// Configuration for a governed distribution wallet.
///
/// Governed wallets (ecosystem_partnerships, reserve_pool) require multi-sig
/// approval for any transfer. Standard transfers (type 0) are blocked.
/// Transfers go through the on-chain proposal system (types 21/22).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernedWalletConfig {
    /// Required number of approvals to execute a transfer
    pub threshold: u8,
    /// Authorized signer public keys
    pub signers: Vec<Pubkey>,
    /// Human-readable label (e.g. "ecosystem_partnerships")
    pub label: String,
}

impl GovernedWalletConfig {
    pub fn new(threshold: u8, signers: Vec<Pubkey>, label: &str) -> Self {
        GovernedWalletConfig {
            threshold,
            signers,
            label: label.to_string(),
        }
    }

    /// Check if a pubkey is an authorized signer for this wallet.
    pub fn is_authorized(&self, signer: &Pubkey) -> bool {
        self.signers.contains(signer)
    }
}

/// An on-chain multi-sig transfer proposal for governed wallets.
///
/// Created via system instruction type 21 (propose_governed_transfer).
/// Approved via system instruction type 22 (approve_governed_transfer).
/// Auto-executes when approvals meet the threshold.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernedProposal {
    /// Unique proposal ID (auto-incrementing)
    pub id: u64,
    /// Source wallet public key (the governed wallet)
    pub source: Pubkey,
    /// Recipient public key
    pub recipient: Pubkey,
    /// Transfer amount in shells
    pub amount: u64,
    /// Pubkeys that have approved this proposal
    pub approvals: Vec<Pubkey>,
    /// Required threshold (snapshot from config at creation time)
    pub threshold: u8,
    /// Whether this proposal has been executed
    pub executed: bool,
}

/// Whitepaper distribution wallet allocation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributionWallet {
    /// Role name (e.g. "community_treasury", "validator_rewards")
    pub role: String,
    /// Public key for this wallet
    pub pubkey: Pubkey,
    /// Allocation in MOLT
    pub amount_molt: u64,
    /// Percentage of total supply
    pub percentage: u8,
    /// Path to keypair file on disk
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keypair_path: Option<String>,
}

/// Whitepaper genesis distribution (ordered: validator_rewards first for backward compat)
/// Updated: 10/25/35/10/10/10 split for sustainable treasury runway.
pub const GENESIS_DISTRIBUTION: &[(&str, u64, u8)] = &[
    ("validator_rewards", 100_000_000, 10),
    ("community_treasury", 250_000_000, 25),
    ("builder_grants", 350_000_000, 35),
    ("founding_moltys", 100_000_000, 10),
    ("ecosystem_partnerships", 100_000_000, 10),
    ("reserve_pool", 100_000_000, 10),
];

/// Genesis wallet keypair bundle (saved to disk)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisWallet {
    /// Genesis treasury public key
    pub pubkey: Pubkey,
    /// Primary keypair (saved encrypted)
    pub keypair_path: String,
    /// Treasury public key (validator_rewards wallet — used for block rewards, fees, bootstraps)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub treasury_pubkey: Option<Pubkey>,
    /// Treasury keypair path
    #[serde(skip_serializing_if = "Option::is_none")]
    pub treasury_keypair_path: Option<String>,
    /// Multi-sig configuration
    pub multisig: Option<MultiSigConfig>,
    /// Whitepaper distribution wallets (6 allocations totaling 1B MOLT)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distribution_wallets: Option<Vec<DistributionWallet>>,
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
    ) -> Result<(Self, Vec<Keypair>, Vec<Keypair>), String> {
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

        // Generate distribution keypairs per whitepaper (6 wallets totaling 1B MOLT)
        let mut distribution_keypairs = Vec::new();
        let mut distribution_wallets = Vec::new();

        for &(role, amount, pct) in GENESIS_DISTRIBUTION {
            let kp = Keypair::generate();
            distribution_wallets.push(DistributionWallet {
                role: role.to_string(),
                pubkey: kp.pubkey(),
                amount_molt: amount,
                percentage: pct,
                keypair_path: None, // filled by save_distribution_keypairs
            });
            distribution_keypairs.push(kp);
        }

        // Treasury = validator_rewards (first in distribution list)
        let treasury_pubkey = distribution_wallets[0].pubkey;

        let wallet = GenesisWallet {
            pubkey,
            keypair_path: format!(".moltchain/genesis-wallet-{}.json", chain_id),
            treasury_pubkey: Some(treasury_pubkey),
            treasury_keypair_path: Some(format!(".moltchain/treasury-wallet-{}.json", chain_id)),
            multisig,
            distribution_wallets: Some(distribution_wallets),
            created_at: chrono::Utc::now().to_rfc3339(),
            chain_id: chain_id.to_string(),
        };

        Ok((wallet, all_keypairs, distribution_keypairs))
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

    /// Save all distribution keypairs to disk (one file per whitepaper wallet)
    pub fn save_distribution_keypairs<P: AsRef<Path>>(
        distribution: &[DistributionWallet],
        keypairs: &[Keypair],
        base_path: P,
        chain_id: &str,
    ) -> Result<Vec<String>, String> {
        let mut paths = Vec::new();

        for (dw, kp) in distribution.iter().zip(keypairs.iter()) {
            let filename = format!("{}-{}.json", dw.role, chain_id);
            let path = base_path.as_ref().join(&filename);

            let keypair_json = serde_json::json!({
                "pubkey": kp.pubkey().to_base58(),
                "secret_key": hex::encode(kp.secret()),
                "role": dw.role,
                "amount_molt": dw.amount_molt,
                "percentage": dw.percentage,
                "chain_id": chain_id,
                "created_at": chrono::Utc::now().to_rfc3339(),
                "warning": "KEEP THIS FILE SECURE - CONTROLS DISTRIBUTION WALLET",
            });

            fs::write(&path, serde_json::to_string_pretty(&keypair_json).unwrap())
                .map_err(|e| format!("Failed to write distribution keypair: {}", e))?;

            paths.push(path.to_string_lossy().to_string());
        }

        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_testnet_wallet() {
        let (wallet, keypairs, dist_keypairs) =
            GenesisWallet::generate("testnet-1", false, 1).unwrap();
        assert_eq!(keypairs.len(), 1);
        assert!(wallet.multisig.is_none()); // Single sig for testnet

        // Whitepaper distribution
        assert_eq!(dist_keypairs.len(), 6);
        let dist = wallet.distribution_wallets.as_ref().unwrap();
        assert_eq!(dist.len(), 6);
        assert_eq!(dist[0].role, "validator_rewards");
        assert_eq!(dist[0].amount_molt, 100_000_000);
        assert_eq!(dist[1].role, "community_treasury");
        assert_eq!(dist[1].amount_molt, 250_000_000);

        // Total = 1B
        let total: u64 = dist.iter().map(|d| d.amount_molt).sum();
        assert_eq!(total, 1_000_000_000);

        // Treasury = validator_rewards
        assert_eq!(wallet.treasury_pubkey, Some(dist[0].pubkey));
    }

    #[test]
    fn test_generate_mainnet_wallet() {
        let (wallet, keypairs, dist_keypairs) =
            GenesisWallet::generate("mainnet-1", true, 5).unwrap();
        assert_eq!(keypairs.len(), 5);
        assert!(wallet.multisig.is_some());

        let multisig = wallet.multisig.unwrap();
        assert_eq!(multisig.threshold, 3); // 3/5
        assert_eq!(multisig.signers.len(), 5);
        assert!(multisig.is_genesis);
        assert!(multisig.is_treasury);

        // Whitepaper distribution
        assert_eq!(dist_keypairs.len(), 6);
        let dist = wallet.distribution_wallets.as_ref().unwrap();
        assert_eq!(dist.len(), 6);
        let total: u64 = dist.iter().map(|d| d.amount_molt).sum();
        assert_eq!(total, 1_000_000_000);
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
