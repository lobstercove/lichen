// Lichen Genesis Configuration
// Production-ready genesis block and chain initialization

use crate::{Account, Pubkey, ValidatorInfo};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Complete genesis configuration for Lichen
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisConfig {
    /// Chain identifier (e.g., "lichen-mainnet-1", "lichen-testnet-1")
    pub chain_id: String,

    /// Genesis timestamp (ISO 8601)
    pub genesis_time: String,

    /// Consensus parameters
    pub consensus: ConsensusParams,

    /// Initial account balances
    pub initial_accounts: Vec<GenesisAccount>,

    /// Initial validator set
    pub initial_validators: Vec<GenesisValidator>,

    /// Network configuration
    pub network: NetworkConfig,

    /// Feature flags
    pub features: FeatureFlags,
}

/// Consensus parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusParams {
    /// Slot duration in milliseconds
    pub slot_duration_ms: u64,

    /// Slots per epoch
    pub epoch_slots: u64,

    /// Minimum stake to be a validator (in spores)
    pub min_validator_stake: u64,

    /// Reference per-slot inflation rate used to derive epoch minting (in spores).
    /// The field name is preserved for genesis compatibility.
    pub validator_reward_per_block: u64,

    /// Slashing percentage for double signing
    pub slashing_percentage_double_sign: u64,

    // AUDIT-FIX A5-03: Replaced flat slashing_percentage_downtime (was 5%)
    // with graduated approach matching consensus.rs apply_economic_slashing.
    /// Downtime slash: percent penalty per 100 missed slots (graduated)
    pub slashing_downtime_per_100_missed: u64,

    /// Downtime slash: maximum percentage cap
    pub slashing_downtime_max_percent: u64,

    /// Slashing percentage for invalid state
    pub slashing_percentage_invalid_state: u64,

    /// AUDIT-FIX MEDIUM-9: Slashing percentage for double vote (previously hardcoded at 30%)
    #[serde(default = "default_double_vote_pct")]
    pub slashing_percentage_double_vote: u64,

    /// AUDIT-FIX MEDIUM-9: Slashing percentage for censorship (previously hardcoded at 25%)
    #[serde(default = "default_censorship_pct")]
    pub slashing_percentage_censorship: u64,

    /// Finality threshold percentage (BFT: 66%)
    pub finality_threshold_percent: u64,
}

fn default_double_vote_pct() -> u64 {
    30
}
fn default_censorship_pct() -> u64 {
    25
}

/// AUDIT-FIX MEDIUM-8: This Default impl uses **testnet-scale** values
/// (75 LICN min stake instead of 75K LICN). It exists solely for backward
/// compatibility in unit tests that don't construct full genesis configs.
/// Production validators always load from genesis.json which sets
/// `min_validator_stake` to the real value (75,000,000,000,000 spores = 75K LICN).
impl Default for ConsensusParams {
    fn default() -> Self {
        ConsensusParams {
            slot_duration_ms: 400,
            epoch_slots: 432000,
            min_validator_stake: 75_000_000_000, // 75 LICN — testnet only, see note above
            validator_reward_per_block: 20_000_000, // 0.02 LICN — sustainable emission rate
            slashing_percentage_double_sign: 50,
            slashing_downtime_per_100_missed: 1,
            slashing_downtime_max_percent: 10,
            slashing_percentage_invalid_state: 100,
            slashing_percentage_double_vote: 30,
            slashing_percentage_censorship: 25,
            finality_threshold_percent: 66,
        }
    }
}

/// Initial account with balance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisAccount {
    /// Account address (Base58)
    pub address: String,

    /// Initial balance in LICN
    pub balance_licn: u64,

    /// Optional comment for documentation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Initial validator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisValidator {
    /// Validator public key (Base58)
    pub pubkey: String,

    /// Initial stake in LICN
    pub stake_licn: u64,

    /// Initial reputation score
    pub reputation: u64,

    /// Optional comment
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Network configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// Default P2P port
    pub p2p_port: u16,

    /// Default RPC port
    pub rpc_port: u16,

    /// Bootstrap seed nodes
    pub seed_nodes: Vec<String>,
}

/// Feature flags
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureFlags {
    /// Percentage of fees to burn (0-100)
    pub fee_burn_percentage: u64,

    /// Percentage of fees to block producer (0-100)
    #[serde(default = "default_fee_producer_percentage")]
    pub fee_producer_percentage: u64,

    /// Percentage of fees to voters (0-100)
    #[serde(default = "default_fee_voters_percentage")]
    pub fee_voters_percentage: u64,

    /// Percentage of fees to community treasury (0-100)
    #[serde(default = "default_fee_community_percentage")]
    pub fee_community_percentage: u64,

    /// Base transaction fee in spores
    pub base_fee_spores: u64,

    /// Rent rate per KB per month in spores
    pub rent_rate_spores_per_kb_month: u64,

    /// Rent-free tier per account in KB
    pub rent_free_kb: u64,

    /// Enable smart contract execution
    pub enable_smart_contracts: bool,

    /// Enable staking
    pub enable_staking: bool,

    /// Enable slashing
    pub enable_slashing: bool,
}

fn default_fee_producer_percentage() -> u64 {
    30
}
fn default_fee_voters_percentage() -> u64 {
    10
}
fn default_fee_community_percentage() -> u64 {
    10
}

impl GenesisConfig {
    /// Load genesis configuration from file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let contents = fs::read_to_string(path.as_ref())
            .map_err(|e| format!("Failed to read genesis file: {}", e))?;

        let config: GenesisConfig = serde_json::from_str(&contents)
            .map_err(|e| format!("Failed to parse genesis JSON: {}", e))?;

        // Validate configuration
        config.validate()?;

        Ok(config)
    }

    /// Validate genesis configuration
    pub fn validate(&self) -> Result<(), String> {
        // Validate chain ID
        if self.chain_id.is_empty() {
            return Err("Chain ID cannot be empty".to_string());
        }

        // Validate consensus params
        if self.consensus.slot_duration_ms == 0 {
            return Err("Slot duration must be greater than 0".to_string());
        }

        if self.consensus.epoch_slots == 0 {
            return Err("Epoch slots must be greater than 0".to_string());
        }

        if self.consensus.finality_threshold_percent > 100 {
            return Err("Finality threshold cannot exceed 100%".to_string());
        }

        // Validate initial accounts (allow empty for dynamic genesis)
        if !self.initial_accounts.is_empty() {
            for account in &self.initial_accounts {
                if account.balance_licn == 0 {
                    return Err(format!("Account {} has zero balance", account.address));
                }

                // Validate address format
                if Pubkey::from_base58(&account.address).is_err() {
                    return Err(format!("Invalid address format: {}", account.address));
                }
            }
        }

        // Validate initial validators (allow empty for dynamic genesis)
        if !self.initial_validators.is_empty() {
            for validator in &self.initial_validators {
                if validator.stake_licn < (self.consensus.min_validator_stake / 1_000_000_000) {
                    return Err(format!(
                        "Validator {} stake below minimum",
                        validator.pubkey
                    ));
                }

                // Validate pubkey format
                if Pubkey::from_base58(&validator.pubkey).is_err() {
                    return Err(format!("Invalid validator pubkey: {}", validator.pubkey));
                }
            }
        }

        // Validate features
        if self.features.fee_burn_percentage > 100 {
            return Err("Fee burn percentage cannot exceed 100%".to_string());
        }
        if self.features.fee_producer_percentage > 100 {
            return Err("Fee producer percentage cannot exceed 100%".to_string());
        }
        if self.features.fee_voters_percentage > 100 {
            return Err("Fee voters percentage cannot exceed 100%".to_string());
        }
        if self.features.fee_community_percentage > 100 {
            return Err("Fee community percentage cannot exceed 100%".to_string());
        }
        // AUDIT-FIX 0.8 + M14: Validate that fee percentages sum to exactly 100.
        // Previously allowed < 100, leaving unallocated fees unaccounted for.
        let total_pct = self.features.fee_burn_percentage
            + self.features.fee_producer_percentage
            + self.features.fee_voters_percentage
            + self.features.fee_community_percentage;
        if total_pct != 100 {
            return Err(format!(
                "Fee percentages must sum to exactly 100% (got {}%: burn {}% + producer {}% + voters {}% + community {}%)",
                total_pct,
                self.features.fee_burn_percentage,
                self.features.fee_producer_percentage,
                self.features.fee_voters_percentage,
                self.features.fee_community_percentage,
            ));
        }

        Ok(())
    }

    /// Convert to runtime accounts
    pub fn to_accounts(&self) -> Result<Vec<(Pubkey, Account)>, String> {
        let mut accounts = Vec::new();

        for genesis_account in &self.initial_accounts {
            let pubkey = Pubkey::from_base58(&genesis_account.address)?;
            let account = Account::new(genesis_account.balance_licn, pubkey);
            accounts.push((pubkey, account));
        }

        Ok(accounts)
    }

    /// Convert to runtime validators
    pub fn to_validators(&self) -> Result<Vec<ValidatorInfo>, String> {
        let mut validators = Vec::new();

        for genesis_validator in &self.initial_validators {
            let pubkey = Pubkey::from_base58(&genesis_validator.pubkey)?;

            let validator = ValidatorInfo {
                pubkey,
                stake: Account::licn_to_spores(genesis_validator.stake_licn),
                reputation: genesis_validator.reputation,
                blocks_proposed: 0,
                votes_cast: 0,
                correct_votes: 0,
                last_active_slot: 0,
                joined_slot: 0,
                commission_rate: 500, // 5% default commission
                transactions_processed: 0,
                pending_activation: false, // Genesis validators active immediately
            };

            validators.push(validator);
        }

        Ok(validators)
    }

    /// Get total supply from initial accounts
    pub fn total_supply_licn(&self) -> u64 {
        self.initial_accounts.iter().map(|a| a.balance_licn).sum()
    }

    /// Generate genesis distribution per tokenomics overhaul:
    ///   25% Community Treasury (125M LICN)
    ///   35% Builder Grants (175M LICN)
    ///   10% Validator Rewards Pool (50M LICN)
    ///   10% Founding Symbionts (50M LICN)
    ///   10% Ecosystem Partnerships (50M LICN)
    ///   10% Reserve Pool (50M LICN)
    /// Total: 500,000,000 LICN
    pub fn generate_genesis_distribution(
        community_treasury: &str,
        builder_grants: &str,
        validator_rewards: &str,
        founding_symbionts: &str,
        ecosystem_partnerships: &str,
        reserve_pool: &str,
    ) -> Vec<GenesisAccount> {
        vec![
            GenesisAccount {
                address: community_treasury.to_string(),
                balance_licn: 125_000_000,
                comment: Some("Community Treasury (25%)".to_string()),
            },
            GenesisAccount {
                address: builder_grants.to_string(),
                balance_licn: 175_000_000,
                comment: Some("Builder Grants (35%)".to_string()),
            },
            GenesisAccount {
                address: validator_rewards.to_string(),
                balance_licn: 50_000_000,
                comment: Some("Validator Rewards Pool (10%)".to_string()),
            },
            GenesisAccount {
                address: founding_symbionts.to_string(),
                balance_licn: 50_000_000,
                comment: Some("Founding Symbionts (10%)".to_string()),
            },
            GenesisAccount {
                address: ecosystem_partnerships.to_string(),
                balance_licn: 50_000_000,
                comment: Some("Ecosystem Partnerships (10%)".to_string()),
            },
            GenesisAccount {
                address: reserve_pool.to_string(),
                balance_licn: 50_000_000,
                comment: Some("Reserve Pool (10%)".to_string()),
            },
        ]
    }

    /// Create default testnet genesis with auto-generated treasury
    /// AUDIT-FIX 3.22: Differentiated from mainnet — lower stakes, faster epochs
    pub fn default_testnet() -> Self {
        GenesisConfig {
            chain_id: "lichen-testnet-1".to_string(),
            genesis_time: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            consensus: ConsensusParams {
                slot_duration_ms: 400,
                // AUDIT-FIX 1.3: match SLOTS_PER_EPOCH constant (432_000)
                epoch_slots: 432000, // ~2 days at 400ms
                // AUDIT-FIX 3.22: Lower stake requirement for testnet (75 LICN vs 75k)
                min_validator_stake: 75_000_000_000, // 75 LICN (testnet)
                // Sustainable emission: 0.02 LICN/block (reduced for BFT adaptive timing)
                validator_reward_per_block: 20_000_000, // 0.02 LICN
                slashing_percentage_double_sign: 50,
                // AUDIT-FIX A5-03: graduated downtime (1% per 100 missed, max 10%)
                slashing_downtime_per_100_missed: 1,
                slashing_downtime_max_percent: 10,
                slashing_percentage_invalid_state: 100,
                slashing_percentage_double_vote: 30,
                slashing_percentage_censorship: 25,
                finality_threshold_percent: 66,
            },
            initial_accounts: vec![
                // Genesis treasury will be auto-generated by first validator
                // No hardcoded addresses - generated fresh each time
            ],
            initial_validators: vec![
                // No genesis validators - validators register dynamically when they start
            ],
            network: NetworkConfig {
                p2p_port: 7001,
                rpc_port: 8899,
                seed_nodes: vec!["127.0.0.1:7001".to_string()],
            },
            features: FeatureFlags {
                fee_burn_percentage: 40,
                fee_producer_percentage: 30,
                fee_voters_percentage: 10,
                fee_community_percentage: 20,
                base_fee_spores: 1_000_000, // 0.001 LICN — $0.0001 at $0.10/LICN
                rent_rate_spores_per_kb_month: 10_000, // $0.000001 at $0.10/LICN
                rent_free_kb: 1,
                enable_smart_contracts: true,
                enable_staking: true,
                enable_slashing: true,
            },
        }
    }

    /// Create default mainnet genesis with auto-generated treasury
    pub fn default_mainnet() -> Self {
        GenesisConfig {
            chain_id: "lichen-mainnet-1".to_string(),
            genesis_time: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            consensus: ConsensusParams {
                slot_duration_ms: 400,
                // AUDIT-FIX 1.3: match SLOTS_PER_EPOCH constant (432_000)
                epoch_slots: 432000,
                min_validator_stake: 75_000_000_000_000, // 75,000 LICN
                // Sustainable emission: 0.02 LICN/block (reduced for BFT adaptive timing)
                validator_reward_per_block: 20_000_000, // 0.02 LICN
                slashing_percentage_double_sign: 50,
                // AUDIT-FIX A5-03: graduated downtime (1% per 100 missed, max 10%)
                slashing_downtime_per_100_missed: 1,
                slashing_downtime_max_percent: 10,
                slashing_percentage_invalid_state: 100,
                slashing_percentage_double_vote: 30,
                slashing_percentage_censorship: 25,
                finality_threshold_percent: 66,
            },
            initial_accounts: vec![
                // Genesis treasury will be auto-generated by first validator
                // Multi-sig required for mainnet (3/5 signers minimum)
            ],
            initial_validators: vec![],
            network: NetworkConfig {
                p2p_port: 7001,
                rpc_port: 8899,
                seed_nodes: vec![],
            },
            features: FeatureFlags {
                fee_burn_percentage: 40,
                fee_producer_percentage: 30,
                fee_voters_percentage: 10,
                fee_community_percentage: 20,
                base_fee_spores: 1_000_000, // 0.001 LICN — $0.0001 at $0.10/LICN
                rent_rate_spores_per_kb_month: 10_000, // $0.000001 at $0.10/LICN
                rent_free_kb: 1,
                enable_smart_contracts: true,
                enable_staking: true,
                enable_slashing: true,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_testnet_valid() {
        let genesis = GenesisConfig::default_testnet();
        assert!(genesis.validate().is_ok());
    }

    #[test]
    fn test_default_genesis_time_is_current() {
        let before = chrono::Utc::now().timestamp();
        let testnet_time = GenesisConfig::default_testnet().genesis_time;
        let mainnet_time = GenesisConfig::default_mainnet().genesis_time;
        let after = chrono::Utc::now().timestamp();
        let t_ts = chrono::DateTime::parse_from_rfc3339(&testnet_time)
            .unwrap()
            .timestamp();
        let m_ts = chrono::DateTime::parse_from_rfc3339(&mainnet_time)
            .unwrap()
            .timestamp();
        assert!(
            t_ts >= before && t_ts <= after,
            "testnet genesis_time should be current"
        );
        assert!(
            m_ts >= before && m_ts <= after,
            "mainnet genesis_time should be current"
        );
    }

    #[test]
    fn test_total_supply() {
        let genesis = GenesisConfig::default_testnet();
        assert_eq!(genesis.total_supply_licn(), 0);
    }

    #[test]
    fn test_genesis_distribution_sums_to_500m() {
        let accounts = GenesisConfig::generate_genesis_distribution(
            "11111111111111111111111111111111",
            "22222222222222222222222222222222",
            "33333333333333333333333333333333",
            "44444444444444444444444444444444",
            "55555555555555555555555555555555",
            "66666666666666666666666666666666",
        );
        let total: u64 = accounts.iter().map(|a| a.balance_licn).sum();
        assert_eq!(
            total, 500_000_000,
            "Genesis distribution must total 500M LICN"
        );
        assert_eq!(accounts.len(), 6);
        assert_eq!(accounts[0].balance_licn, 125_000_000); // 25%
        assert_eq!(accounts[1].balance_licn, 175_000_000); // 35%
        assert_eq!(accounts[2].balance_licn, 50_000_000); // 10%
        assert_eq!(accounts[3].balance_licn, 50_000_000); // 10%
        assert_eq!(accounts[4].balance_licn, 50_000_000); // 10%
        assert_eq!(accounts[5].balance_licn, 50_000_000); // 10%
    }

    #[test]
    fn test_to_accounts() {
        let genesis = GenesisConfig::default_testnet();
        let accounts = genesis.to_accounts().unwrap();
        assert!(accounts.is_empty());
    }
}
