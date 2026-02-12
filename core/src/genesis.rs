// MoltChain Genesis Configuration
// Production-ready genesis block and chain initialization

use crate::{Account, Pubkey, ValidatorInfo};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Complete genesis configuration for MoltChain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisConfig {
    /// Chain identifier (e.g., "moltchain-mainnet-1", "moltchain-testnet-1")
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

    /// Minimum stake to be a validator (in shells)
    pub min_validator_stake: u64,

    /// Block reward for validator (in shells)
    pub validator_reward_per_block: u64,

    /// Slashing percentage for double signing
    pub slashing_percentage_double_sign: u64,

    /// Slashing percentage for downtime
    pub slashing_percentage_downtime: u64,

    /// Slashing percentage for invalid state
    pub slashing_percentage_invalid_state: u64,

    /// Finality threshold percentage (BFT: 66%)
    pub finality_threshold_percent: u64,
}

/// Initial account with balance
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisAccount {
    /// Account address (Base58)
    pub address: String,

    /// Initial balance in MOLT
    pub balance_molt: u64,

    /// Optional comment for documentation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Initial validator
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisValidator {
    /// Validator public key (Base58)
    pub pubkey: String,

    /// Initial stake in MOLT
    pub stake_molt: u64,

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

    /// Base transaction fee in shells
    pub base_fee_shells: u64,

    /// Rent rate per KB per month in shells
    pub rent_rate_shells_per_kb_month: u64,

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
                if account.balance_molt == 0 {
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
                if validator.stake_molt < (self.consensus.min_validator_stake / 1_000_000_000) {
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

        Ok(())
    }

    /// Convert to runtime accounts
    pub fn to_accounts(&self) -> Result<Vec<(Pubkey, Account)>, String> {
        let mut accounts = Vec::new();

        for genesis_account in &self.initial_accounts {
            let pubkey = Pubkey::from_base58(&genesis_account.address)?;
            let account = Account::new(genesis_account.balance_molt, pubkey);
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
                stake: Account::molt_to_shells(genesis_validator.stake_molt),
                reputation: genesis_validator.reputation,
                blocks_proposed: 0,
                votes_cast: 0,
                correct_votes: 0,
                last_active_slot: 0,
                joined_slot: 0,
            };

            validators.push(validator);
        }

        Ok(validators)
    }

    /// Get total supply from initial accounts
    pub fn total_supply_molt(&self) -> u64 {
        self.initial_accounts.iter().map(|a| a.balance_molt).sum()
    }

    /// Generate genesis distribution per whitepaper:
    ///   40% Community Treasury (400M MOLT)
    ///   25% Validator Rewards Pool (250M MOLT)
    ///   15% Development Fund (150M MOLT)
    ///   10% Ecosystem Growth (100M MOLT)
    ///    5% Foundation Reserve (50M MOLT)
    ///    5% Early Contributors (50M MOLT)
    /// Total: 1,000,000,000 MOLT
    pub fn generate_genesis_distribution(
        community_treasury: &str,
        validator_rewards: &str,
        development_fund: &str,
        ecosystem_growth: &str,
        foundation_reserve: &str,
        early_contributors: &str,
    ) -> Vec<GenesisAccount> {
        vec![
            GenesisAccount {
                address: community_treasury.to_string(),
                balance_molt: 400_000_000,
                comment: Some("Community Treasury (40%)".to_string()),
            },
            GenesisAccount {
                address: validator_rewards.to_string(),
                balance_molt: 250_000_000,
                comment: Some("Validator Rewards Pool (25%)".to_string()),
            },
            GenesisAccount {
                address: development_fund.to_string(),
                balance_molt: 150_000_000,
                comment: Some("Development Fund (15%)".to_string()),
            },
            GenesisAccount {
                address: ecosystem_growth.to_string(),
                balance_molt: 100_000_000,
                comment: Some("Ecosystem Growth (10%)".to_string()),
            },
            GenesisAccount {
                address: foundation_reserve.to_string(),
                balance_molt: 50_000_000,
                comment: Some("Foundation Reserve (5%)".to_string()),
            },
            GenesisAccount {
                address: early_contributors.to_string(),
                balance_molt: 50_000_000,
                comment: Some("Early Contributors (5%)".to_string()),
            },
        ]
    }

    /// Create default testnet genesis with auto-generated treasury
    pub fn default_testnet() -> Self {
        GenesisConfig {
            chain_id: "moltchain-testnet-1".to_string(),
            genesis_time: chrono::Utc::now().to_rfc3339(),
            consensus: ConsensusParams {
                slot_duration_ms: 400,
                epoch_slots: 216000,                     // ~24 hours at 400ms
                min_validator_stake: 10_000_000_000_000, // 10,000 MOLT per whitepaper
                validator_reward_per_block: 10_000_000,  // 0.01 MOLT
                slashing_percentage_double_sign: 50,
                slashing_percentage_downtime: 5,
                slashing_percentage_invalid_state: 100,
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
                p2p_port: 8000,
                rpc_port: 9000,
                seed_nodes: vec!["127.0.0.1:8000".to_string()],
            },
            features: FeatureFlags {
                fee_burn_percentage: 50,
                fee_producer_percentage: 30,
                fee_voters_percentage: 10,
                base_fee_shells: 10_000, // 0.00001 MOLT per whitepaper
                rent_rate_shells_per_kb_month: 1_000,
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
            chain_id: "moltchain-mainnet-1".to_string(),
            genesis_time: chrono::Utc::now().to_rfc3339(),
            consensus: ConsensusParams {
                slot_duration_ms: 400,
                epoch_slots: 216000,
                min_validator_stake: 10_000_000_000_000, // 10,000 MOLT per whitepaper
                validator_reward_per_block: 10_000_000,
                slashing_percentage_double_sign: 50,
                slashing_percentage_downtime: 5,
                slashing_percentage_invalid_state: 100,
                finality_threshold_percent: 66,
            },
            initial_accounts: vec![
                // Genesis treasury will be auto-generated by first validator
                // Multi-sig required for mainnet (3/5 signers minimum)
            ],
            initial_validators: vec![],
            network: NetworkConfig {
                p2p_port: 8000,
                rpc_port: 9000,
                seed_nodes: vec![],
            },
            features: FeatureFlags {
                fee_burn_percentage: 50,
                fee_producer_percentage: 30,
                fee_voters_percentage: 10,
                base_fee_shells: 10_000, // 0.00001 MOLT per whitepaper
                rent_rate_shells_per_kb_month: 1_000,
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
    fn test_total_supply() {
        let genesis = GenesisConfig::default_testnet();
        assert_eq!(genesis.total_supply_molt(), 0);
    }

    #[test]
    fn test_genesis_distribution_sums_to_1b() {
        let accounts = GenesisConfig::generate_genesis_distribution(
            "11111111111111111111111111111111",
            "22222222222222222222222222222222",
            "33333333333333333333333333333333",
            "44444444444444444444444444444444",
            "55555555555555555555555555555555",
            "66666666666666666666666666666666",
        );
        let total: u64 = accounts.iter().map(|a| a.balance_molt).sum();
        assert_eq!(
            total, 1_000_000_000,
            "Genesis distribution must total 1B MOLT"
        );
        assert_eq!(accounts.len(), 6);
        assert_eq!(accounts[0].balance_molt, 400_000_000); // 40%
        assert_eq!(accounts[1].balance_molt, 250_000_000); // 25%
        assert_eq!(accounts[2].balance_molt, 150_000_000); // 15%
        assert_eq!(accounts[3].balance_molt, 100_000_000); // 10%
        assert_eq!(accounts[4].balance_molt, 50_000_000); //  5%
        assert_eq!(accounts[5].balance_molt, 50_000_000); //  5%
    }

    #[test]
    fn test_to_accounts() {
        let genesis = GenesisConfig::default_testnet();
        let accounts = genesis.to_accounts().unwrap();
        assert!(accounts.is_empty());
    }
}
