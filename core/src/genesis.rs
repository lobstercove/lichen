// Lichen Genesis Configuration
// Production-ready genesis block and chain initialization

use crate::consensus::{
    DEFAULT_BFT_MAX_PHASE_TIMEOUT_MS, DEFAULT_BFT_PRECOMMIT_TIMEOUT_BASE_MS,
    DEFAULT_BFT_PREVOTE_TIMEOUT_BASE_MS, DEFAULT_BFT_PROPOSE_TIMEOUT_BASE_MS,
};
use crate::restrictions::{
    ProtocolModuleId, RestrictionMode, RestrictionReason, RestrictionRecord, RestrictionStatus,
    RestrictionTarget, NATIVE_LICN_ASSET_ID,
};
use crate::{Account, Hash, Pubkey, StateStore, ValidatorInfo};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// System instruction opcode used in slot 0 to carry canonical genesis state.
///
/// Opcode 40 carries [`GenesisConfig`]. Opcode 41 carries compressed chunks of
/// the fully materialized genesis state, so validators can bootstrap from the
/// network without local contract artifacts or genesis replay code drift.
pub const GENESIS_STATE_CHUNK_OPCODE: u8 = 41;

/// Wire version for the canonical genesis state bundle.
pub const GENESIS_STATE_BUNDLE_VERSION: u16 = 1;

/// One exported key/value column in the canonical genesis state.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisStateCategory {
    pub name: String,
    pub entries: Vec<(Vec<u8>, Vec<u8>)>,
}

/// Canonical post-genesis state committed by block 0.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisStateBundle {
    pub version: u16,
    pub state_root: [u8; 32],
    pub categories: Vec<GenesisStateCategory>,
}

/// A compressed chunk of [`GenesisStateBundle`] embedded in a slot-0
/// transaction instruction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisStateChunk {
    pub version: u16,
    pub state_root: [u8; 32],
    pub compression: String,
    pub compressed_len: u64,
    pub uncompressed_len: u64,
    pub compressed_sha256: [u8; 32],
    pub chunk_index: u32,
    pub total_chunks: u32,
    pub data: Vec<u8>,
}

/// Oracle prices frozen at genesis time — embedded in the genesis block for
/// deterministic replay on every joining validator.
///
/// The genesis creator fetches live market prices once and stores them here.
/// All other validators extract these from the genesis block and use them
/// verbatim, producing byte-identical contract storage (AMM pools, analytics
/// candles, margin prices, oracle feeds).
///
/// The oracle attestation system updates prices to live within seconds after
/// genesis — these defaults only affect the first few startup liveness blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenesisPrices {
    /// LICN/USD price with 8 decimals (e.g. 10_000_000 = $0.10)
    pub licn_usd_8dec: u64,
    /// wSOL/USD price with 8 decimals
    pub wsol_usd_8dec: u64,
    /// wETH/USD price with 8 decimals
    pub weth_usd_8dec: u64,
    /// wBNB/USD price with 8 decimals
    pub wbnb_usd_8dec: u64,
    /// wNEO/USD price with 8 decimals
    pub wneo_usd_8dec: u64,
    /// wGAS/USD price with 8 decimals
    pub wgas_usd_8dec: u64,
}

impl Default for GenesisPrices {
    fn default() -> Self {
        Self {
            licn_usd_8dec: 10_000_000,      // $0.10
            wsol_usd_8dec: 8_184_000_000,   // $81.84
            weth_usd_8dec: 199_934_000_000, // $1,999.34
            wbnb_usd_8dec: 60_978_000_000,  // $609.78
            wneo_usd_8dec: 307_500_000,     // $3.075
            wgas_usd_8dec: 165_000_000,     // $1.65
        }
    }
}

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

    /// Bridge operator keys authorized in LichenBridge at genesis.
    #[serde(default)]
    pub bridge_validators: Vec<String>,

    /// Oracle operator keys authorized for contract feeder/attester lanes at genesis.
    #[serde(default)]
    pub oracle_operators: Vec<String>,

    /// Network configuration
    pub network: NetworkConfig,

    /// Feature flags
    pub features: FeatureFlags,

    /// Oracle prices at genesis — fetched once by the genesis creator,
    /// frozen forever, embedded in the genesis block for deterministic replay.
    #[serde(default)]
    pub genesis_prices: GenesisPrices,

    /// Optional testnet-only restrictions materialized into consensus state at
    /// slot 0 for incident-response drills. Mainnet genesis rejects this field
    /// when non-empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub initial_restrictions: Vec<GenesisRestriction>,
}

/// Human-facing restriction entry for genesis config files.
///
/// The runtime consensus representation is [`RestrictionRecord`]. This config
/// type keeps genesis JSON ergonomic: addresses are base58 strings, hashes are
/// hex strings, and IDs are assigned deterministically from config order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisRestriction {
    pub target: GenesisRestrictionTarget,
    pub mode: GenesisRestrictionMode,

    #[serde(default = "default_genesis_restriction_reason")]
    pub reason: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_hash: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_uri_hash: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposer: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_authority: Option<String>,

    #[serde(default)]
    pub created_slot: u64,

    #[serde(default)]
    pub created_epoch: u64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_slot: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GenesisRestrictionTarget {
    Account { account: String },
    AccountAsset { account: String, asset: String },
    Asset { asset: String },
    Contract { contract: String },
    CodeHash { code_hash: String },
    BridgeRoute { chain_id: String, asset: String },
    ProtocolModule { module: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GenesisRestrictionMode {
    OutgoingOnly,
    IncomingOnly,
    Bidirectional,
    FrozenAmount { amount: u64 },
    AssetPaused,
    ExecuteBlocked,
    StateChangingBlocked,
    Quarantined,
    DeployBlocked,
    RoutePaused,
    ProtocolPaused,
    Terminated,
}

fn default_genesis_restriction_reason() -> String {
    RestrictionReason::TestnetDrill.as_str().to_string()
}

fn parse_optional_pubkey(value: &Option<String>, field: &str) -> Result<Option<Pubkey>, String> {
    value
        .as_deref()
        .map(|raw| Pubkey::from_base58(raw).map_err(|err| format!("Invalid {field}: {err}")))
        .transpose()
}

fn parse_hash_hex(value: &str, field: &str) -> Result<Hash, String> {
    let trimmed = value.strip_prefix("0x").unwrap_or(value);
    Hash::from_hex(trimmed).map_err(|err| format!("Invalid {field}: {err}"))
}

fn parse_optional_hash(value: &Option<String>, field: &str) -> Result<Option<Hash>, String> {
    value
        .as_deref()
        .map(|raw| parse_hash_hex(raw, field))
        .transpose()
}

fn parse_asset_pubkey(value: &str, field: &str) -> Result<Pubkey, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "native" | "licn" | "native_licn" => Ok(NATIVE_LICN_ASSET_ID),
        _ => Pubkey::from_base58(value).map_err(|err| format!("Invalid {field}: {err}")),
    }
}

fn parse_protocol_module(value: &str) -> Result<ProtocolModuleId, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "native" => Ok(ProtocolModuleId::Native),
        "governance" => Ok(ProtocolModuleId::Governance),
        "staking" => Ok(ProtocolModuleId::Staking),
        "moss_stake" | "mossstake" => Ok(ProtocolModuleId::MossStake),
        "shielded" => Ok(ProtocolModuleId::Shielded),
        "contracts" | "contract" => Ok(ProtocolModuleId::Contracts),
        "tokens" | "token" => Ok(ProtocolModuleId::Tokens),
        "dex" => Ok(ProtocolModuleId::Dex),
        "lending" => Ok(ProtocolModuleId::Lending),
        "marketplace" => Ok(ProtocolModuleId::Marketplace),
        "bridge" => Ok(ProtocolModuleId::Bridge),
        "custody" => Ok(ProtocolModuleId::Custody),
        "oracle" => Ok(ProtocolModuleId::Oracle),
        "validator" | "validators" => Ok(ProtocolModuleId::Validator),
        "mempool" => Ok(ProtocolModuleId::Mempool),
        other => Err(format!("Invalid protocol module: {other}")),
    }
}

fn parse_restriction_reason(value: &str) -> Result<RestrictionReason, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "exploit_active" => Ok(RestrictionReason::ExploitActive),
        "stolen_funds" => Ok(RestrictionReason::StolenFunds),
        "bridge_compromise" => Ok(RestrictionReason::BridgeCompromise),
        "oracle_compromise" => Ok(RestrictionReason::OracleCompromise),
        "scam_contract" => Ok(RestrictionReason::ScamContract),
        "malicious_code_hash" => Ok(RestrictionReason::MaliciousCodeHash),
        "sanctions_or_legal_order" => Ok(RestrictionReason::SanctionsOrLegalOrder),
        "phishing_or_impersonation" => Ok(RestrictionReason::PhishingOrImpersonation),
        "custody_incident" => Ok(RestrictionReason::CustodyIncident),
        "protocol_bug" => Ok(RestrictionReason::ProtocolBug),
        "governance_error_correction" => Ok(RestrictionReason::GovernanceErrorCorrection),
        "false_positive_lift" => Ok(RestrictionReason::FalsePositiveLift),
        "testnet_drill" => Ok(RestrictionReason::TestnetDrill),
        other => Err(format!("Invalid restriction reason: {other}")),
    }
}

fn is_mainnet_chain_id(chain_id: &str) -> bool {
    chain_id.to_ascii_lowercase().contains("mainnet")
}

impl GenesisRestrictionTarget {
    fn to_restriction_target(&self) -> Result<RestrictionTarget, String> {
        match self {
            Self::Account { account } => Ok(RestrictionTarget::Account(
                Pubkey::from_base58(account)
                    .map_err(|err| format!("Invalid account target: {err}"))?,
            )),
            Self::AccountAsset { account, asset } => Ok(RestrictionTarget::AccountAsset {
                account: Pubkey::from_base58(account)
                    .map_err(|err| format!("Invalid account_asset account: {err}"))?,
                asset: parse_asset_pubkey(asset, "account_asset asset")?,
            }),
            Self::Asset { asset } => Ok(RestrictionTarget::Asset(parse_asset_pubkey(
                asset,
                "asset target",
            )?)),
            Self::Contract { contract } => Ok(RestrictionTarget::Contract(
                Pubkey::from_base58(contract)
                    .map_err(|err| format!("Invalid contract target: {err}"))?,
            )),
            Self::CodeHash { code_hash } => Ok(RestrictionTarget::CodeHash(parse_hash_hex(
                code_hash,
                "code_hash",
            )?)),
            Self::BridgeRoute { chain_id, asset } => Ok(RestrictionTarget::BridgeRoute {
                chain_id: chain_id.clone(),
                asset: asset.clone(),
            }),
            Self::ProtocolModule { module } => Ok(RestrictionTarget::ProtocolModule(
                parse_protocol_module(module)?,
            )),
        }
    }
}

impl GenesisRestrictionMode {
    fn to_restriction_mode(&self) -> RestrictionMode {
        match self {
            Self::OutgoingOnly => RestrictionMode::OutgoingOnly,
            Self::IncomingOnly => RestrictionMode::IncomingOnly,
            Self::Bidirectional => RestrictionMode::Bidirectional,
            Self::FrozenAmount { amount } => RestrictionMode::FrozenAmount { amount: *amount },
            Self::AssetPaused => RestrictionMode::AssetPaused,
            Self::ExecuteBlocked => RestrictionMode::ExecuteBlocked,
            Self::StateChangingBlocked => RestrictionMode::StateChangingBlocked,
            Self::Quarantined => RestrictionMode::Quarantined,
            Self::DeployBlocked => RestrictionMode::DeployBlocked,
            Self::RoutePaused => RestrictionMode::RoutePaused,
            Self::ProtocolPaused => RestrictionMode::ProtocolPaused,
            Self::Terminated => RestrictionMode::Terminated,
        }
    }
}

impl GenesisRestriction {
    fn to_record(&self, id: u64, default_authority: Pubkey) -> Result<RestrictionRecord, String> {
        let record = RestrictionRecord {
            id,
            target: self.target.to_restriction_target()?,
            mode: self.mode.to_restriction_mode(),
            status: RestrictionStatus::Active,
            reason: parse_restriction_reason(&self.reason)?,
            evidence_hash: parse_optional_hash(&self.evidence_hash, "evidence_hash")?,
            evidence_uri_hash: parse_optional_hash(&self.evidence_uri_hash, "evidence_uri_hash")?,
            proposer: parse_optional_pubkey(&self.proposer, "proposer")?
                .unwrap_or(default_authority),
            authority: parse_optional_pubkey(&self.authority, "authority")?
                .unwrap_or(default_authority),
            approval_authority: parse_optional_pubkey(
                &self.approval_authority,
                "approval_authority",
            )?,
            created_slot: self.created_slot,
            created_epoch: self.created_epoch,
            expires_at_slot: self.expires_at_slot,
            supersedes: None,
            lifted_by: None,
            lifted_slot: None,
            lift_reason: None,
        };
        record.validate()?;
        Ok(record)
    }
}

/// Consensus parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsensusParams {
    /// Slot duration in milliseconds
    pub slot_duration_ms: u64,

    /// Base propose timeout in milliseconds for BFT rounds.
    #[serde(default = "default_bft_propose_timeout_base_ms")]
    pub propose_timeout_base_ms: u64,

    /// Base prevote timeout in milliseconds for BFT rounds.
    #[serde(default = "default_bft_prevote_timeout_base_ms")]
    pub prevote_timeout_base_ms: u64,

    /// Base precommit timeout in milliseconds for BFT rounds.
    #[serde(default = "default_bft_precommit_timeout_base_ms")]
    pub precommit_timeout_base_ms: u64,

    /// Maximum timeout cap for any BFT phase in milliseconds.
    #[serde(default = "default_bft_max_phase_timeout_ms")]
    pub max_phase_timeout_ms: u64,

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
fn default_bft_propose_timeout_base_ms() -> u64 {
    DEFAULT_BFT_PROPOSE_TIMEOUT_BASE_MS
}
fn default_bft_prevote_timeout_base_ms() -> u64 {
    DEFAULT_BFT_PREVOTE_TIMEOUT_BASE_MS
}
fn default_bft_precommit_timeout_base_ms() -> u64 {
    DEFAULT_BFT_PRECOMMIT_TIMEOUT_BASE_MS
}
fn default_bft_max_phase_timeout_ms() -> u64 {
    DEFAULT_BFT_MAX_PHASE_TIMEOUT_MS
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
            propose_timeout_base_ms: DEFAULT_BFT_PROPOSE_TIMEOUT_BASE_MS,
            prevote_timeout_base_ms: DEFAULT_BFT_PREVOTE_TIMEOUT_BASE_MS,
            precommit_timeout_base_ms: DEFAULT_BFT_PRECOMMIT_TIMEOUT_BASE_MS,
            max_phase_timeout_ms: DEFAULT_BFT_MAX_PHASE_TIMEOUT_MS,
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
    pub fee_producer_percentage: u64,

    /// Percentage of fees to voters (0-100)
    pub fee_voters_percentage: u64,

    /// Percentage of fees to protocol treasury (0-100)
    pub fee_treasury_percentage: u64,

    /// Percentage of fees to community treasury (0-100)
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

        if self.consensus.propose_timeout_base_ms == 0
            || self.consensus.prevote_timeout_base_ms == 0
            || self.consensus.precommit_timeout_base_ms == 0
        {
            return Err("Consensus timeout bases must be greater than 0".to_string());
        }

        if self.consensus.max_phase_timeout_ms == 0 {
            return Err("Consensus max phase timeout must be greater than 0".to_string());
        }

        let max_base_timeout = self
            .consensus
            .propose_timeout_base_ms
            .max(self.consensus.prevote_timeout_base_ms)
            .max(self.consensus.precommit_timeout_base_ms);
        if self.consensus.max_phase_timeout_ms < max_base_timeout {
            return Err(
                "Consensus max phase timeout must be at least as large as every timeout base"
                    .to_string(),
            );
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

        for bridge_validator in &self.bridge_validators {
            if Pubkey::from_base58(bridge_validator).is_err() {
                return Err(format!(
                    "Invalid bridge validator pubkey: {}",
                    bridge_validator
                ));
            }
        }

        for oracle_operator in &self.oracle_operators {
            if Pubkey::from_base58(oracle_operator).is_err() {
                return Err(format!(
                    "Invalid oracle operator pubkey: {}",
                    oracle_operator
                ));
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
        if self.features.fee_treasury_percentage > 100 {
            return Err("Fee treasury percentage cannot exceed 100%".to_string());
        }
        if self.features.fee_community_percentage > 100 {
            return Err("Fee community percentage cannot exceed 100%".to_string());
        }
        // Validate that all 5 fee percentages sum to exactly 100%.
        let total_pct = self.features.fee_burn_percentage
            + self.features.fee_producer_percentage
            + self.features.fee_voters_percentage
            + self.features.fee_treasury_percentage
            + self.features.fee_community_percentage;
        if total_pct != 100 {
            return Err(format!(
                "Fee percentages must sum to exactly 100% (got {}%: burn {}% + producer {}% + voters {}% + treasury {}% + community {}%)",
                total_pct,
                self.features.fee_burn_percentage,
                self.features.fee_producer_percentage,
                self.features.fee_voters_percentage,
                self.features.fee_treasury_percentage,
                self.features.fee_community_percentage,
            ));
        }

        self.validate_initial_restrictions()?;

        Ok(())
    }

    /// Validate genesis restriction config without assigning live state IDs.
    pub fn validate_initial_restrictions(&self) -> Result<(), String> {
        if !self.initial_restrictions.is_empty() && is_mainnet_chain_id(&self.chain_id) {
            return Err(
                "Initial genesis restrictions are testnet-only and cannot be set on mainnet"
                    .to_string(),
            );
        }

        self.materialize_initial_restrictions(Pubkey([0u8; 32]))
            .map(|_| ())
    }

    /// Convert config-facing initial restrictions into deterministic consensus
    /// records. IDs are 1-based and assigned in config order.
    pub fn materialize_initial_restrictions(
        &self,
        default_authority: Pubkey,
    ) -> Result<Vec<RestrictionRecord>, String> {
        if !self.initial_restrictions.is_empty() && is_mainnet_chain_id(&self.chain_id) {
            return Err(
                "Initial genesis restrictions are testnet-only and cannot be set on mainnet"
                    .to_string(),
            );
        }

        self.initial_restrictions
            .iter()
            .enumerate()
            .map(|(index, restriction)| {
                let id = u64::try_from(index)
                    .ok()
                    .and_then(|value| value.checked_add(1))
                    .ok_or_else(|| "Too many initial genesis restrictions".to_string())?;
                restriction.to_record(id, default_authority)
            })
            .collect()
    }

    /// Seed deterministic initial restrictions into an empty genesis state DB.
    ///
    /// This reserves IDs through the normal state counter, stores records through
    /// the consensus restriction registry, and activates the state-root schema
    /// that commits restriction state when at least one record is seeded.
    pub fn seed_initial_restrictions(
        &self,
        state: &StateStore,
        default_authority: Pubkey,
    ) -> Result<usize, String> {
        let records = self.materialize_initial_restrictions(default_authority)?;
        for record in &records {
            let allocated_id = state.next_restriction_id()?;
            if allocated_id != record.id {
                return Err(format!(
                    "Initial genesis restriction ID mismatch: expected {}, allocated {}",
                    record.id, allocated_id
                ));
            }
            state.put_restriction(record)?;
        }

        if !records.is_empty() {
            state.set_state_root_schema(true)?;
        }

        Ok(records.len())
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
                last_observed_at_ms: 0,
                last_observed_block_at_ms: 0,
                last_observed_block_slot: 0,
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
                propose_timeout_base_ms: DEFAULT_BFT_PROPOSE_TIMEOUT_BASE_MS,
                prevote_timeout_base_ms: DEFAULT_BFT_PREVOTE_TIMEOUT_BASE_MS,
                precommit_timeout_base_ms: DEFAULT_BFT_PRECOMMIT_TIMEOUT_BASE_MS,
                max_phase_timeout_ms: DEFAULT_BFT_MAX_PHASE_TIMEOUT_MS,
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
            bridge_validators: vec![],
            oracle_operators: vec![],
            network: NetworkConfig {
                p2p_port: 7001,
                rpc_port: 8899,
                seed_nodes: vec!["127.0.0.1:7001".to_string()],
            },
            features: FeatureFlags {
                fee_burn_percentage: 40,
                fee_producer_percentage: 30,
                fee_voters_percentage: 10,
                fee_treasury_percentage: 10,
                fee_community_percentage: 10,
                base_fee_spores: 1_000_000, // 0.001 LICN — $0.0001 at $0.10/LICN
                rent_rate_spores_per_kb_month: 10_000, // $0.000001 at $0.10/LICN
                rent_free_kb: 1,
                enable_smart_contracts: true,
                enable_staking: true,
                enable_slashing: true,
            },
            genesis_prices: GenesisPrices::default(),
            initial_restrictions: vec![],
        }
    }

    /// Create default mainnet genesis with auto-generated treasury
    pub fn default_mainnet() -> Self {
        GenesisConfig {
            chain_id: "lichen-mainnet-1".to_string(),
            genesis_time: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            consensus: ConsensusParams {
                slot_duration_ms: 400,
                propose_timeout_base_ms: DEFAULT_BFT_PROPOSE_TIMEOUT_BASE_MS,
                prevote_timeout_base_ms: DEFAULT_BFT_PREVOTE_TIMEOUT_BASE_MS,
                precommit_timeout_base_ms: DEFAULT_BFT_PRECOMMIT_TIMEOUT_BASE_MS,
                max_phase_timeout_ms: DEFAULT_BFT_MAX_PHASE_TIMEOUT_MS,
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
            bridge_validators: vec![],
            oracle_operators: vec![],
            network: NetworkConfig {
                p2p_port: 7001,
                rpc_port: 8899,
                seed_nodes: vec![],
            },
            features: FeatureFlags {
                fee_burn_percentage: 40,
                fee_producer_percentage: 30,
                fee_voters_percentage: 10,
                fee_treasury_percentage: 10,
                fee_community_percentage: 10,
                base_fee_spores: 1_000_000, // 0.001 LICN — $0.0001 at $0.10/LICN
                rent_rate_spores_per_kb_month: 10_000, // $0.000001 at $0.10/LICN
                rent_free_kb: 1,
                enable_smart_contracts: true,
                enable_staking: true,
                enable_slashing: true,
            },
            genesis_prices: GenesisPrices::default(),
            initial_restrictions: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn test_pubkey(byte: u8) -> Pubkey {
        Pubkey([byte; 32])
    }

    fn test_genesis_restriction(account: Pubkey) -> GenesisRestriction {
        GenesisRestriction {
            target: GenesisRestrictionTarget::Account {
                account: account.to_base58(),
            },
            mode: GenesisRestrictionMode::OutgoingOnly,
            reason: "testnet_drill".to_string(),
            evidence_hash: None,
            evidence_uri_hash: None,
            proposer: None,
            authority: None,
            approval_authority: None,
            created_slot: 0,
            created_epoch: 0,
            expires_at_slot: Some(10),
        }
    }

    #[test]
    fn test_default_testnet_valid() {
        let genesis = GenesisConfig::default_testnet();
        assert!(genesis.validate().is_ok());
        assert!(genesis.initial_restrictions.is_empty());
        let json = serde_json::to_string(&genesis).unwrap();
        assert!(
            !json.contains("initial_restrictions"),
            "empty initial restrictions should be omitted from genesis JSON"
        );
    }

    #[test]
    fn test_default_mainnet_has_no_initial_restrictions() {
        let genesis = GenesisConfig::default_mainnet();
        assert!(genesis.validate().is_ok());
        assert!(genesis.initial_restrictions.is_empty());
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

    #[test]
    fn test_validate_rejects_zero_consensus_timeout_base() {
        let mut genesis = GenesisConfig::default_testnet();
        genesis.consensus.propose_timeout_base_ms = 0;

        let error = genesis
            .validate()
            .expect_err("zero timeout bases must fail validation");
        assert!(error.contains("timeout bases"));
    }

    #[test]
    fn test_validate_rejects_max_phase_timeout_below_base() {
        let mut genesis = GenesisConfig::default_testnet();
        genesis.consensus.max_phase_timeout_ms = genesis.consensus.precommit_timeout_base_ms - 1;

        let error = genesis
            .validate()
            .expect_err("max timeout below a base timeout must fail validation");
        assert!(error.contains("max phase timeout"));
    }

    #[test]
    fn test_validate_rejects_mainnet_initial_restrictions() {
        let mut genesis = GenesisConfig::default_mainnet();
        genesis
            .initial_restrictions
            .push(test_genesis_restriction(test_pubkey(1)));

        let error = genesis
            .validate()
            .expect_err("mainnet genesis restrictions must be rejected");
        assert!(error.contains("testnet-only"));
    }

    #[test]
    fn test_validate_rejects_invalid_initial_restriction_record() {
        let mut genesis = GenesisConfig::default_testnet();
        let mut restriction = test_genesis_restriction(test_pubkey(2));
        restriction.mode = GenesisRestrictionMode::AssetPaused;
        genesis.initial_restrictions.push(restriction);

        let error = genesis
            .validate()
            .expect_err("account target cannot use asset pause mode");
        assert!(error.contains("not valid for target type account"));
    }

    #[test]
    fn test_validate_rejects_initial_restriction_missing_required_evidence() {
        let mut genesis = GenesisConfig::default_testnet();
        let mut restriction = test_genesis_restriction(test_pubkey(3));
        restriction.reason = "stolen_funds".to_string();
        genesis.initial_restrictions.push(restriction);

        let error = genesis
            .validate()
            .expect_err("non-drill reason must include evidence");
        assert!(error.contains("requires evidence_hash or evidence_uri_hash"));
    }

    #[test]
    fn test_initial_restrictions_materialize_deterministic_ids_and_native_alias() {
        let authority = test_pubkey(9);
        let account = test_pubkey(4);
        let mut genesis = GenesisConfig::default_testnet();
        genesis
            .initial_restrictions
            .push(test_genesis_restriction(account));
        genesis.initial_restrictions.push(GenesisRestriction {
            target: GenesisRestrictionTarget::AccountAsset {
                account: account.to_base58(),
                asset: "native_licn".to_string(),
            },
            mode: GenesisRestrictionMode::FrozenAmount { amount: 42 },
            reason: "testnet_drill".to_string(),
            evidence_hash: None,
            evidence_uri_hash: None,
            proposer: None,
            authority: None,
            approval_authority: None,
            created_slot: 0,
            created_epoch: 0,
            expires_at_slot: Some(20),
        });

        let records = genesis
            .materialize_initial_restrictions(authority)
            .expect("materialize initial restrictions");

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].id, 1);
        assert_eq!(records[1].id, 2);
        assert_eq!(records[0].proposer, authority);
        assert_eq!(records[0].authority, authority);
        assert_eq!(
            records[1].target,
            RestrictionTarget::AccountAsset {
                account,
                asset: NATIVE_LICN_ASSET_ID,
            }
        );
    }

    #[test]
    fn test_seed_initial_restrictions_reserves_ids_and_activates_schema() {
        let temp = tempdir().unwrap();
        let state = StateStore::open(temp.path()).unwrap();
        let authority = test_pubkey(10);
        let account = test_pubkey(11);
        let mut genesis = GenesisConfig::default_testnet();
        genesis
            .initial_restrictions
            .push(test_genesis_restriction(account));

        let seeded = genesis
            .seed_initial_restrictions(&state, authority)
            .expect("seed initial restrictions");

        assert_eq!(seeded, 1);
        let stored = state.get_restriction(1).unwrap().expect("restriction 1");
        assert_eq!(stored.target, RestrictionTarget::Account(account));
        assert_eq!(stored.authority, authority);
        assert_eq!(state.get_state_root_schema(), Some(true));
        assert_eq!(
            state.next_restriction_id().unwrap(),
            2,
            "post-genesis IDs must continue after seeded records"
        );
    }

    #[test]
    fn test_seeded_initial_restrictions_snapshot_roundtrip() {
        let source_dir = tempdir().unwrap();
        let dest_dir = tempdir().unwrap();
        let source = StateStore::open(source_dir.path()).unwrap();
        let dest = StateStore::open(dest_dir.path()).unwrap();
        let authority = test_pubkey(12);
        let account = test_pubkey(13);
        let mut genesis = GenesisConfig::default_testnet();
        genesis
            .initial_restrictions
            .push(test_genesis_restriction(account));
        genesis
            .seed_initial_restrictions(&source, authority)
            .expect("seed source restrictions");

        let source_root = source.compute_state_root_cold_start();
        for category in [
            "restrictions",
            "restriction_index_target",
            "restriction_index_code_hash",
            "stats",
        ] {
            let page = source
                .export_snapshot_category_cursor_untracked(category, None, 1000)
                .unwrap();
            dest.import_snapshot_category(category, &page.entries)
                .unwrap();
        }

        assert_eq!(
            dest.get_restriction(1).unwrap(),
            source.get_restriction(1).unwrap()
        );
        assert_eq!(dest.compute_state_root_cold_start(), source_root);
    }
}
