// Lichen Multi-Signature Wallet Support

use crate::{Hash, Keypair, Pubkey};
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
const SPORES_PER_LICN: u64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum GovernedTransferVelocityTier {
    #[default]
    Standard,
    Elevated,
    Extraordinary,
}

impl GovernedTransferVelocityTier {
    pub fn as_str(self) -> &'static str {
        match self {
            GovernedTransferVelocityTier::Standard => "standard",
            GovernedTransferVelocityTier::Elevated => "elevated",
            GovernedTransferVelocityTier::Extraordinary => "extraordinary",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernedTransferVelocityPolicy {
    pub per_transfer_cap_spores: u64,
    pub daily_cap_spores: u64,
    pub elevated_threshold_spores: u64,
    pub extraordinary_threshold_spores: u64,
    #[serde(default)]
    pub elevated_additional_timelock_epochs: u32,
    #[serde(default)]
    pub extraordinary_additional_timelock_epochs: u32,
}

impl GovernedTransferVelocityPolicy {
    pub fn new(
        per_transfer_cap_spores: u64,
        daily_cap_spores: u64,
        elevated_threshold_spores: u64,
        extraordinary_threshold_spores: u64,
        elevated_additional_timelock_epochs: u32,
        extraordinary_additional_timelock_epochs: u32,
    ) -> Self {
        Self {
            per_transfer_cap_spores,
            daily_cap_spores,
            elevated_threshold_spores,
            extraordinary_threshold_spores,
            elevated_additional_timelock_epochs,
            extraordinary_additional_timelock_epochs,
        }
    }

    pub fn community_treasury_defaults() -> Self {
        Self::new(
            5_000_000 * SPORES_PER_LICN,
            10_000_000 * SPORES_PER_LICN,
            1_000_000 * SPORES_PER_LICN,
            2_500_000 * SPORES_PER_LICN,
            1,
            3,
        )
    }

    pub fn ecosystem_partnerships_defaults() -> Self {
        Self::new(
            1_000_000 * SPORES_PER_LICN,
            2_000_000 * SPORES_PER_LICN,
            250_000 * SPORES_PER_LICN,
            500_000 * SPORES_PER_LICN,
            1,
            2,
        )
    }

    pub fn reserve_pool_defaults() -> Self {
        Self::new(
            2_000_000 * SPORES_PER_LICN,
            4_000_000 * SPORES_PER_LICN,
            500_000 * SPORES_PER_LICN,
            1_000_000 * SPORES_PER_LICN,
            1,
            2,
        )
    }

    pub fn tier_for_amount(&self, amount: u64) -> GovernedTransferVelocityTier {
        if self.extraordinary_threshold_spores > 0 && amount >= self.extraordinary_threshold_spores
        {
            GovernedTransferVelocityTier::Extraordinary
        } else if self.elevated_threshold_spores > 0 && amount >= self.elevated_threshold_spores {
            GovernedTransferVelocityTier::Elevated
        } else {
            GovernedTransferVelocityTier::Standard
        }
    }

    pub fn required_threshold(
        &self,
        base_threshold: u8,
        signer_count: usize,
        tier: GovernedTransferVelocityTier,
    ) -> u8 {
        let max_threshold = u8::try_from(signer_count).unwrap_or(u8::MAX);
        let base_threshold = base_threshold.min(max_threshold);

        match tier {
            GovernedTransferVelocityTier::Standard => base_threshold,
            GovernedTransferVelocityTier::Elevated => {
                base_threshold.saturating_add(1).min(max_threshold)
            }
            GovernedTransferVelocityTier::Extraordinary => max_threshold,
        }
    }

    pub fn additional_timelock_epochs(&self, tier: GovernedTransferVelocityTier) -> u32 {
        match tier {
            GovernedTransferVelocityTier::Standard => 0,
            GovernedTransferVelocityTier::Elevated => self.elevated_additional_timelock_epochs,
            GovernedTransferVelocityTier::Extraordinary => {
                self.extraordinary_additional_timelock_epochs
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernedWalletConfig {
    /// Required number of approvals to execute a transfer
    pub threshold: u8,
    /// Authorized signer public keys
    pub signers: Vec<Pubkey>,
    /// Human-readable label (e.g. "ecosystem_partnerships")
    pub label: String,
    /// Optional execution delay after threshold is reached.
    #[serde(default)]
    pub timelock_epochs: u32,
    /// Optional velocity policy for governed native-spore transfers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transfer_velocity_policy: Option<GovernedTransferVelocityPolicy>,
}

impl GovernedWalletConfig {
    pub fn new(threshold: u8, signers: Vec<Pubkey>, label: &str) -> Self {
        GovernedWalletConfig {
            threshold,
            signers,
            label: label.to_string(),
            timelock_epochs: 0,
            transfer_velocity_policy: None,
        }
    }

    pub fn with_timelock(mut self, timelock_epochs: u32) -> Self {
        self.timelock_epochs = timelock_epochs;
        self
    }

    pub fn with_transfer_velocity_policy(
        mut self,
        transfer_velocity_policy: GovernedTransferVelocityPolicy,
    ) -> Self {
        self.transfer_velocity_policy = Some(transfer_velocity_policy);
        self
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
    /// Transfer amount in spores
    pub amount: u64,
    /// Pubkeys that have approved this proposal
    pub approvals: Vec<Pubkey>,
    /// Required threshold (snapshot from config at creation time)
    pub threshold: u8,
    /// Earliest epoch when execution is allowed.
    #[serde(default)]
    pub execute_after_epoch: u64,
    /// Snapshot of the proposal's transfer velocity tier.
    #[serde(default)]
    pub velocity_tier: GovernedTransferVelocityTier,
    /// Snapshot of the source wallet's daily transfer cap.
    #[serde(default)]
    pub daily_cap_spores: u64,
    /// Whether this proposal has been executed
    pub executed: bool,
    /// Whether this proposal was cancelled before execution.
    #[serde(default)]
    pub cancelled: bool,
}

pub const INCIDENT_GUARDIAN_LABEL: &str = "incident_guardian";
pub const BRIDGE_COMMITTEE_ADMIN_LABEL: &str = "bridge_committee_admin";
pub const ORACLE_COMMITTEE_ADMIN_LABEL: &str = "oracle_committee_admin";
pub const UPGRADE_PROPOSER_LABEL: &str = "upgrade_proposer";
pub const UPGRADE_VETO_GUARDIAN_LABEL: &str = "upgrade_veto_guardian";
pub const TREASURY_EXECUTOR_LABEL: &str = "treasury_executor";
const INCIDENT_GUARDIAN_ROLE_ORDER: [&str; 3] =
    ["community_treasury", "reserve_pool", "validator_rewards"];
const BRIDGE_COMMITTEE_ADMIN_ROLE_ORDER: [&str; 3] =
    ["validator_rewards", "reserve_pool", "founding_symbionts"];
const ORACLE_COMMITTEE_ADMIN_ROLE_ORDER: [&str; 3] = [
    "validator_rewards",
    "ecosystem_partnerships",
    "builder_grants",
];
const UPGRADE_PROPOSER_ROLE_ORDER: [&str; 3] =
    ["community_treasury", "founding_symbionts", "builder_grants"];
const UPGRADE_VETO_GUARDIAN_ROLE_ORDER: [&str; 3] = [
    "validator_rewards",
    "reserve_pool",
    "ecosystem_partnerships",
];
const TREASURY_EXECUTOR_ROLE_ORDER: [&str; 3] = [
    "community_treasury",
    "ecosystem_partnerships",
    "reserve_pool",
];

fn derive_governed_committee_authority(label: &str, governance_authority: &Pubkey) -> Pubkey {
    let domain = format!("lichen:{}:v1", label);
    Pubkey::new(Hash::hash_two_parts(domain.as_bytes(), governance_authority.as_ref()).0)
}

pub fn derive_incident_guardian_authority(governance_authority: &Pubkey) -> Pubkey {
    derive_governed_committee_authority(INCIDENT_GUARDIAN_LABEL, governance_authority)
}

pub fn derive_bridge_committee_admin_authority(governance_authority: &Pubkey) -> Pubkey {
    derive_governed_committee_authority(BRIDGE_COMMITTEE_ADMIN_LABEL, governance_authority)
}

pub fn derive_oracle_committee_admin_authority(governance_authority: &Pubkey) -> Pubkey {
    derive_governed_committee_authority(ORACLE_COMMITTEE_ADMIN_LABEL, governance_authority)
}

pub fn derive_upgrade_proposer_authority(governance_authority: &Pubkey) -> Pubkey {
    derive_governed_committee_authority(UPGRADE_PROPOSER_LABEL, governance_authority)
}

pub fn derive_upgrade_veto_guardian_authority(governance_authority: &Pubkey) -> Pubkey {
    derive_governed_committee_authority(UPGRADE_VETO_GUARDIAN_LABEL, governance_authority)
}

pub fn derive_treasury_executor_authority(governance_authority: &Pubkey) -> Pubkey {
    derive_governed_committee_authority(TREASURY_EXECUTOR_LABEL, governance_authority)
}

fn governed_committee_config_for_roles(
    role_pubkeys: &[(String, Pubkey)],
    governance_authority: &Pubkey,
    label: &str,
    required_roles: &[&str],
    timelock_epochs: u32,
) -> Result<(Pubkey, GovernedWalletConfig), String> {
    let mut signers = Vec::with_capacity(required_roles.len());

    for required_role in required_roles {
        let signer = role_pubkeys
            .iter()
            .find_map(|(role, pubkey)| {
                if role == *required_role {
                    Some(*pubkey)
                } else {
                    None
                }
            })
            .ok_or_else(|| {
                format!(
                    "{} requires genesis role '{}' to be present",
                    label, required_role
                )
            })?;
        if !signers.contains(&signer) {
            signers.push(signer);
        }
    }

    let threshold = if signers.len() == 1 { 1 } else { 2 };
    let mut config = GovernedWalletConfig::new(threshold, signers, label);
    if timelock_epochs > 0 {
        config = config.with_timelock(timelock_epochs);
    }
    Ok((
        derive_governed_committee_authority(label, governance_authority),
        config,
    ))
}

pub fn incident_guardian_config_for_roles(
    role_pubkeys: &[(String, Pubkey)],
    governance_authority: &Pubkey,
) -> Result<(Pubkey, GovernedWalletConfig), String> {
    governed_committee_config_for_roles(
        role_pubkeys,
        governance_authority,
        INCIDENT_GUARDIAN_LABEL,
        &INCIDENT_GUARDIAN_ROLE_ORDER,
        0,
    )
}

pub fn bridge_committee_admin_config_for_roles(
    role_pubkeys: &[(String, Pubkey)],
    governance_authority: &Pubkey,
) -> Result<(Pubkey, GovernedWalletConfig), String> {
    governed_committee_config_for_roles(
        role_pubkeys,
        governance_authority,
        BRIDGE_COMMITTEE_ADMIN_LABEL,
        &BRIDGE_COMMITTEE_ADMIN_ROLE_ORDER,
        1,
    )
}

pub fn oracle_committee_admin_config_for_roles(
    role_pubkeys: &[(String, Pubkey)],
    governance_authority: &Pubkey,
) -> Result<(Pubkey, GovernedWalletConfig), String> {
    governed_committee_config_for_roles(
        role_pubkeys,
        governance_authority,
        ORACLE_COMMITTEE_ADMIN_LABEL,
        &ORACLE_COMMITTEE_ADMIN_ROLE_ORDER,
        1,
    )
}

pub fn upgrade_proposer_config_for_roles(
    role_pubkeys: &[(String, Pubkey)],
    governance_authority: &Pubkey,
) -> Result<(Pubkey, GovernedWalletConfig), String> {
    governed_committee_config_for_roles(
        role_pubkeys,
        governance_authority,
        UPGRADE_PROPOSER_LABEL,
        &UPGRADE_PROPOSER_ROLE_ORDER,
        1,
    )
}

pub fn upgrade_veto_guardian_config_for_roles(
    role_pubkeys: &[(String, Pubkey)],
    governance_authority: &Pubkey,
) -> Result<(Pubkey, GovernedWalletConfig), String> {
    governed_committee_config_for_roles(
        role_pubkeys,
        governance_authority,
        UPGRADE_VETO_GUARDIAN_LABEL,
        &UPGRADE_VETO_GUARDIAN_ROLE_ORDER,
        0,
    )
}

pub fn treasury_executor_config_for_roles(
    role_pubkeys: &[(String, Pubkey)],
    governance_authority: &Pubkey,
) -> Result<(Pubkey, GovernedWalletConfig), String> {
    governed_committee_config_for_roles(
        role_pubkeys,
        governance_authority,
        TREASURY_EXECUTOR_LABEL,
        &TREASURY_EXECUTOR_ROLE_ORDER,
        1,
    )
}

pub fn governed_wallet_config_for_role(
    role: &str,
    all_signers: &[Pubkey],
) -> Option<GovernedWalletConfig> {
    let signers = all_signers.to_vec();
    let governance_threshold = u8::try_from((all_signers.len() / 2) + 1).unwrap_or(u8::MAX);

    match role {
        "community_treasury" => Some(
            GovernedWalletConfig::new(governance_threshold, signers, "community_treasury")
                .with_timelock(1)
                .with_transfer_velocity_policy(
                    GovernedTransferVelocityPolicy::community_treasury_defaults(),
                ),
        ),
        "ecosystem_partnerships" => Some(
            GovernedWalletConfig::new(2, signers, "ecosystem_partnerships")
                .with_transfer_velocity_policy(
                    GovernedTransferVelocityPolicy::ecosystem_partnerships_defaults(),
                ),
        ),
        "reserve_pool" => Some(
            GovernedWalletConfig::new(3, signers, "reserve_pool").with_transfer_velocity_policy(
                GovernedTransferVelocityPolicy::reserve_pool_defaults(),
            ),
        ),
        _ => None,
    }
}

/// Whitepaper distribution wallet allocation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributionWallet {
    /// Role name (e.g. "community_treasury", "validator_rewards")
    pub role: String,
    /// Public key for this wallet
    pub pubkey: Pubkey,
    /// Allocation in LICN
    pub amount_licn: u64,
    /// Percentage of total supply
    pub percentage: u8,
    /// Path to keypair file on disk
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keypair_path: Option<String>,
}

/// Whitepaper genesis distribution (ordered: validator_rewards first for backward compat)
/// Updated: 10/25/35/10/10/10 split for sustainable treasury runway.
pub const GENESIS_DISTRIBUTION: &[(&str, u64, u8)] = &[
    ("validator_rewards", 50_000_000, 10),
    ("community_treasury", 125_000_000, 25),
    ("builder_grants", 175_000_000, 35),
    ("founding_symbionts", 50_000_000, 10),
    ("ecosystem_partnerships", 50_000_000, 10),
    ("reserve_pool", 50_000_000, 10),
];

fn canonical_keypair_json(
    keypair: &Keypair,
    role: &str,
    chain_id: &str,
    warning: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let public_key = keypair.public_key();
    let mut file = serde_json::Map::new();
    file.insert(
        "privateKey".to_string(),
        serde_json::json!(keypair.to_seed()),
    );
    file.insert("publicKey".to_string(), serde_json::json!(public_key.bytes));
    file.insert(
        "publicKeyBase58".to_string(),
        serde_json::json!(keypair.pubkey().to_base58()),
    );
    file.insert("role".to_string(), serde_json::json!(role));
    file.insert("chain_id".to_string(), serde_json::json!(chain_id));
    file.insert(
        "created_at".to_string(),
        serde_json::json!(chrono::Utc::now().to_rfc3339()),
    );
    file.insert("warning".to_string(), serde_json::json!(warning));
    file
}

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
    /// Whitepaper distribution wallets (6 allocations totaling 500M LICN)
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

        // Generate distribution keypairs per whitepaper (6 wallets totaling 500M LICN)
        let mut distribution_keypairs = Vec::new();
        let mut distribution_wallets = Vec::new();

        for &(role, amount, pct) in GENESIS_DISTRIBUTION {
            let kp = Keypair::generate();
            distribution_wallets.push(DistributionWallet {
                role: role.to_string(),
                pubkey: kp.pubkey(),
                amount_licn: amount,
                percentage: pct,
                keypair_path: None, // filled by save_distribution_keypairs
            });
            distribution_keypairs.push(kp);
        }

        // Treasury = validator_rewards (first in distribution list)
        let treasury_pubkey = distribution_wallets[0].pubkey;

        let wallet = GenesisWallet {
            pubkey,
            keypair_path: format!("genesis-keys/genesis-primary-{}.json", chain_id),
            treasury_pubkey: Some(treasury_pubkey),
            treasury_keypair_path: Some(format!("genesis-keys/treasury-{}.json", chain_id)),
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

            let keypair_json = serde_json::Value::Object(canonical_keypair_json(
                keypair,
                role,
                chain_id,
                "KEEP THIS FILE SECURE - CONTROLS GENESIS TREASURY",
            ));

            let json_str = serde_json::to_string_pretty(&keypair_json)
                .map_err(|e| format!("Failed to serialize keypair JSON: {}", e))?;
            fs::write(&path, json_str).map_err(|e| format!("Failed to write keypair: {}", e))?;

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
        let keypair_json = serde_json::Value::Object(canonical_keypair_json(
            keypair,
            "treasury",
            chain_id,
            "KEEP THIS FILE SECURE - CONTROLS TREASURY",
        ));

        let json_str = serde_json::to_string_pretty(&keypair_json)
            .map_err(|e| format!("Failed to serialize treasury JSON: {}", e))?;
        fs::write(&path, json_str)
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

            let mut keypair_json = canonical_keypair_json(
                kp,
                &dw.role,
                chain_id,
                "KEEP THIS FILE SECURE - CONTROLS DISTRIBUTION WALLET",
            );
            keypair_json.insert("amount_licn".to_string(), serde_json::json!(dw.amount_licn));
            keypair_json.insert("percentage".to_string(), serde_json::json!(dw.percentage));
            let keypair_json = serde_json::Value::Object(keypair_json);

            let json_str = serde_json::to_string_pretty(&keypair_json)
                .map_err(|e| format!("Failed to serialize distribution JSON: {}", e))?;
            fs::write(&path, json_str)
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
        assert_eq!(dist[0].amount_licn, 50_000_000);
        assert_eq!(dist[1].role, "community_treasury");
        assert_eq!(dist[1].amount_licn, 125_000_000);

        // Total = 500M
        let total: u64 = dist.iter().map(|d| d.amount_licn).sum();
        assert_eq!(total, 500_000_000);

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
        let total: u64 = dist.iter().map(|d| d.amount_licn).sum();
        assert_eq!(total, 500_000_000);
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

    #[test]
    fn test_bridge_committee_admin_config_uses_split_role_roots() {
        let governance_authority = Pubkey([0x77; 32]);
        let role_pubkeys = vec![
            ("validator_rewards".to_string(), Pubkey([1; 32])),
            ("reserve_pool".to_string(), Pubkey([2; 32])),
            ("founding_symbionts".to_string(), Pubkey([3; 32])),
        ];

        let (authority, config) =
            bridge_committee_admin_config_for_roles(&role_pubkeys, &governance_authority)
                .expect("bridge committee config");

        assert_eq!(
            authority,
            derive_bridge_committee_admin_authority(&governance_authority)
        );
        assert_eq!(config.label, BRIDGE_COMMITTEE_ADMIN_LABEL);
        assert_eq!(config.threshold, 2);
        assert_eq!(
            config.signers,
            vec![Pubkey([1; 32]), Pubkey([2; 32]), Pubkey([3; 32])]
        );
        assert_eq!(config.timelock_epochs, 1);
    }

    #[test]
    fn test_oracle_committee_admin_config_uses_split_role_roots() {
        let governance_authority = Pubkey([0x88; 32]);
        let role_pubkeys = vec![
            ("validator_rewards".to_string(), Pubkey([4; 32])),
            ("ecosystem_partnerships".to_string(), Pubkey([5; 32])),
            ("builder_grants".to_string(), Pubkey([6; 32])),
        ];

        let (authority, config) =
            oracle_committee_admin_config_for_roles(&role_pubkeys, &governance_authority)
                .expect("oracle committee config");

        assert_eq!(
            authority,
            derive_oracle_committee_admin_authority(&governance_authority)
        );
        assert_eq!(config.label, ORACLE_COMMITTEE_ADMIN_LABEL);
        assert_eq!(config.threshold, 2);
        assert_eq!(
            config.signers,
            vec![Pubkey([4; 32]), Pubkey([5; 32]), Pubkey([6; 32])]
        );
        assert_eq!(config.timelock_epochs, 1);
    }

    #[test]
    fn test_treasury_executor_config_uses_split_role_roots() {
        let governance_authority = Pubkey([0x8B; 32]);
        let role_pubkeys = vec![
            ("community_treasury".to_string(), Pubkey([13; 32])),
            ("ecosystem_partnerships".to_string(), Pubkey([14; 32])),
            ("reserve_pool".to_string(), Pubkey([15; 32])),
        ];

        let (authority, config) =
            treasury_executor_config_for_roles(&role_pubkeys, &governance_authority)
                .expect("treasury executor config");

        assert_eq!(
            authority,
            derive_treasury_executor_authority(&governance_authority)
        );
        assert_eq!(config.label, TREASURY_EXECUTOR_LABEL);
        assert_eq!(config.threshold, 2);
        assert_eq!(
            config.signers,
            vec![Pubkey([13; 32]), Pubkey([14; 32]), Pubkey([15; 32])]
        );
        assert_eq!(config.timelock_epochs, 1);
    }

    #[test]
    fn test_upgrade_proposer_config_uses_split_role_roots() {
        let governance_authority = Pubkey([0x89; 32]);
        let role_pubkeys = vec![
            ("community_treasury".to_string(), Pubkey([7; 32])),
            ("founding_symbionts".to_string(), Pubkey([8; 32])),
            ("builder_grants".to_string(), Pubkey([9; 32])),
        ];

        let (authority, config) =
            upgrade_proposer_config_for_roles(&role_pubkeys, &governance_authority)
                .expect("upgrade proposer config");

        assert_eq!(
            authority,
            derive_upgrade_proposer_authority(&governance_authority)
        );
        assert_eq!(config.label, UPGRADE_PROPOSER_LABEL);
        assert_eq!(config.threshold, 2);
        assert_eq!(
            config.signers,
            vec![Pubkey([7; 32]), Pubkey([8; 32]), Pubkey([9; 32])]
        );
        assert_eq!(config.timelock_epochs, 1);
    }

    #[test]
    fn test_upgrade_veto_guardian_config_uses_split_role_roots() {
        let governance_authority = Pubkey([0x8A; 32]);
        let role_pubkeys = vec![
            ("validator_rewards".to_string(), Pubkey([10; 32])),
            ("reserve_pool".to_string(), Pubkey([11; 32])),
            ("ecosystem_partnerships".to_string(), Pubkey([12; 32])),
        ];

        let (authority, config) =
            upgrade_veto_guardian_config_for_roles(&role_pubkeys, &governance_authority)
                .expect("upgrade veto guardian config");

        assert_eq!(
            authority,
            derive_upgrade_veto_guardian_authority(&governance_authority)
        );
        assert_eq!(config.label, UPGRADE_VETO_GUARDIAN_LABEL);
        assert_eq!(config.threshold, 2);
        assert_eq!(
            config.signers,
            vec![Pubkey([10; 32]), Pubkey([11; 32]), Pubkey([12; 32])]
        );
        assert_eq!(config.timelock_epochs, 0);
    }

    #[test]
    fn test_save_treasury_keypair_uses_canonical_pq_fields() {
        let dir = std::env::temp_dir().join(format!(
            "lichen-genesis-wallet-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        fs::create_dir_all(&dir).unwrap();

        let keypair = Keypair::generate();
        let saved_path = GenesisWallet::save_treasury_keypair(&keypair, &dir, "testnet-1").unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&saved_path).unwrap()).unwrap();

        assert_eq!(json["privateKey"].as_array().unwrap().len(), 32);
        assert_eq!(
            json["publicKey"].as_array().unwrap().len(),
            keypair.public_key().bytes.len()
        );
        assert_eq!(
            json["publicKeyBase58"].as_str().unwrap(),
            keypair.pubkey().to_base58()
        );

        let _ = fs::remove_file(saved_path);
        let _ = fs::remove_dir_all(dir);
    }
}
