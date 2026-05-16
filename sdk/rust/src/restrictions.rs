//! Restriction-governance RPC helpers.

use crate::{Client, Error, Pubkey, Result};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestrictionAddress {
    Pubkey(Pubkey),
    String(String),
}

impl RestrictionAddress {
    fn as_rpc_string(&self) -> String {
        match self {
            Self::Pubkey(pubkey) => pubkey.to_base58(),
            Self::String(value) => value.clone(),
        }
    }
}

impl Serialize for RestrictionAddress {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_rpc_string())
    }
}

impl From<Pubkey> for RestrictionAddress {
    fn from(value: Pubkey) -> Self {
        Self::Pubkey(value)
    }
}

impl From<&Pubkey> for RestrictionAddress {
    fn from(value: &Pubkey) -> Self {
        Self::Pubkey(*value)
    }
}

impl From<String> for RestrictionAddress {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for RestrictionAddress {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestrictionAsset {
    Pubkey(Pubkey),
    String(String),
}

impl RestrictionAsset {
    fn as_rpc_string(&self) -> String {
        match self {
            Self::Pubkey(pubkey) => pubkey.to_base58(),
            Self::String(value) => value.clone(),
        }
    }
}

impl Serialize for RestrictionAsset {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.as_rpc_string())
    }
}

impl From<Pubkey> for RestrictionAsset {
    fn from(value: Pubkey) -> Self {
        Self::Pubkey(value)
    }
}

impl From<&Pubkey> for RestrictionAsset {
    fn from(value: &Pubkey) -> Self {
        Self::Pubkey(*value)
    }
}

impl From<String> for RestrictionAsset {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for RestrictionAsset {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeChain {
    Solana,
    Ethereum,
    Bsc,
    Bnb,
    NeoX,
}

impl BridgeChain {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Solana => "solana",
            Self::Ethereum => "ethereum",
            Self::Bsc => "bsc",
            Self::Bnb => "bnb",
            Self::NeoX => "neox",
        }
    }
}

impl Serialize for BridgeChain {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl From<BridgeChain> for String {
    fn from(value: BridgeChain) -> Self {
        value.as_str().to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeAsset {
    Sol,
    Eth,
    Bnb,
    Gas,
    Neo,
    Usdc,
    Usdt,
}

impl BridgeAsset {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Sol => "sol",
            Self::Eth => "eth",
            Self::Bnb => "bnb",
            Self::Gas => "gas",
            Self::Neo => "neo",
            Self::Usdc => "usdc",
            Self::Usdt => "usdt",
        }
    }
}

impl Serialize for BridgeAsset {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl From<BridgeAsset> for String {
    fn from(value: BridgeAsset) -> Self {
        value.as_str().to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestrictionStringOrU64 {
    String(String),
    U64(u64),
}

impl Serialize for RestrictionStringOrU64 {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::String(value) => serializer.serialize_str(value),
            Self::U64(value) => serializer.serialize_u64(*value),
        }
    }
}

impl From<u64> for RestrictionStringOrU64 {
    fn from(value: u64) -> Self {
        Self::U64(value)
    }
}

impl From<u8> for RestrictionStringOrU64 {
    fn from(value: u8) -> Self {
        Self::U64(value as u64)
    }
}

impl From<String> for RestrictionStringOrU64 {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for RestrictionStringOrU64 {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<lichen_core::ProtocolModuleId> for RestrictionStringOrU64 {
    fn from(value: lichen_core::ProtocolModuleId) -> Self {
        Self::String(value.as_str().to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestrictionReasonInput {
    Label(String),
    Id(u8),
}

impl Serialize for RestrictionReasonInput {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Label(value) => serializer.serialize_str(value),
            Self::Id(value) => serializer.serialize_u8(*value),
        }
    }
}

impl From<lichen_core::RestrictionReason> for RestrictionReasonInput {
    fn from(value: lichen_core::RestrictionReason) -> Self {
        Self::Label(value.as_str().to_string())
    }
}

impl From<u8> for RestrictionReasonInput {
    fn from(value: u8) -> Self {
        Self::Id(value)
    }
}

impl From<String> for RestrictionReasonInput {
    fn from(value: String) -> Self {
        Self::Label(value)
    }
}

impl From<&str> for RestrictionReasonInput {
    fn from(value: &str) -> Self {
        Self::Label(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestrictionLiftReasonInput {
    Label(String),
    Id(u8),
}

impl Serialize for RestrictionLiftReasonInput {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Label(value) => serializer.serialize_str(value),
            Self::Id(value) => serializer.serialize_u8(*value),
        }
    }
}

impl From<lichen_core::RestrictionLiftReason> for RestrictionLiftReasonInput {
    fn from(value: lichen_core::RestrictionLiftReason) -> Self {
        Self::Label(value.as_str().to_string())
    }
}

impl From<u8> for RestrictionLiftReasonInput {
    fn from(value: u8) -> Self {
        Self::Id(value)
    }
}

impl From<String> for RestrictionLiftReasonInput {
    fn from(value: String) -> Self {
        Self::Label(value)
    }
}

impl From<&str> for RestrictionLiftReasonInput {
    fn from(value: &str) -> Self {
        Self::Label(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestrictionModeInput {
    Label(String),
    Id(u8),
}

impl Serialize for RestrictionModeInput {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Label(value) => serializer.serialize_str(value),
            Self::Id(value) => serializer.serialize_u8(*value),
        }
    }
}

impl From<lichen_core::RestrictionMode> for RestrictionModeInput {
    fn from(value: lichen_core::RestrictionMode) -> Self {
        Self::Label(value.as_str().to_string())
    }
}

impl From<u8> for RestrictionModeInput {
    fn from(value: u8) -> Self {
        Self::Id(value)
    }
}

impl From<String> for RestrictionModeInput {
    fn from(value: String) -> Self {
        Self::Label(value)
    }
}

impl From<&str> for RestrictionModeInput {
    fn from(value: &str) -> Self {
        Self::Label(value.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RestrictionTargetInput {
    Account {
        account: RestrictionAddress,
    },
    AccountAsset {
        account: RestrictionAddress,
        asset: RestrictionAsset,
    },
    Asset {
        asset: RestrictionAsset,
    },
    Contract {
        contract: RestrictionAddress,
    },
    CodeHash {
        code_hash: String,
    },
    BridgeRoute {
        chain: String,
        asset: String,
    },
    ProtocolModule {
        module: RestrictionStringOrU64,
    },
}

impl RestrictionTargetInput {
    pub fn account(account: impl Into<RestrictionAddress>) -> Self {
        Self::Account {
            account: account.into(),
        }
    }

    pub fn account_asset(
        account: impl Into<RestrictionAddress>,
        asset: impl Into<RestrictionAsset>,
    ) -> Self {
        Self::AccountAsset {
            account: account.into(),
            asset: asset.into(),
        }
    }

    pub fn asset(asset: impl Into<RestrictionAsset>) -> Self {
        Self::Asset {
            asset: asset.into(),
        }
    }

    pub fn contract(contract: impl Into<RestrictionAddress>) -> Self {
        Self::Contract {
            contract: contract.into(),
        }
    }

    pub fn code_hash(code_hash: impl Into<String>) -> Self {
        Self::CodeHash {
            code_hash: code_hash.into(),
        }
    }

    pub fn bridge_route(chain: impl Into<String>, asset: impl Into<String>) -> Self {
        Self::BridgeRoute {
            chain: chain.into(),
            asset: asset.into(),
        }
    }

    pub fn protocol_module(module: impl Into<RestrictionStringOrU64>) -> Self {
        Self::ProtocolModule {
            module: module.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RestrictionListParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<RestrictionStringOrU64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RestrictionBuilderBaseParams {
    pub proposer: RestrictionAddress,
    pub governance_authority: RestrictionAddress,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_blockhash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RestrictCommonParams {
    pub proposer: RestrictionAddress,
    pub governance_authority: RestrictionAddress,
    pub reason: RestrictionReasonInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_blockhash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_uri_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_slot: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RestrictAccountParams {
    pub proposer: RestrictionAddress,
    pub governance_authority: RestrictionAddress,
    pub account: RestrictionAddress,
    pub reason: RestrictionReasonInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<RestrictionModeInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_blockhash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_uri_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_slot: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnrestrictAccountParams {
    pub proposer: RestrictionAddress,
    pub governance_authority: RestrictionAddress,
    pub account: RestrictionAddress,
    pub lift_reason: RestrictionLiftReasonInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restriction_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_blockhash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RestrictAccountAssetParams {
    pub proposer: RestrictionAddress,
    pub governance_authority: RestrictionAddress,
    pub account: RestrictionAddress,
    pub asset: RestrictionAsset,
    pub reason: RestrictionReasonInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<RestrictionModeInput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_blockhash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_uri_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_slot: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnrestrictAccountAssetParams {
    pub proposer: RestrictionAddress,
    pub governance_authority: RestrictionAddress,
    pub account: RestrictionAddress,
    pub asset: RestrictionAsset,
    pub lift_reason: RestrictionLiftReasonInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restriction_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_blockhash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SetFrozenAssetAmountParams {
    pub proposer: RestrictionAddress,
    pub governance_authority: RestrictionAddress,
    pub account: RestrictionAddress,
    pub asset: RestrictionAsset,
    pub amount: u64,
    pub reason: RestrictionReasonInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_blockhash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_uri_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_slot: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContractRestrictionParams {
    pub proposer: RestrictionAddress,
    pub governance_authority: RestrictionAddress,
    pub contract: RestrictionAddress,
    pub reason: RestrictionReasonInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_blockhash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_uri_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_slot: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResumeContractParams {
    pub proposer: RestrictionAddress,
    pub governance_authority: RestrictionAddress,
    pub contract: RestrictionAddress,
    pub lift_reason: RestrictionLiftReasonInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restriction_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_blockhash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CodeHashRestrictionParams {
    pub proposer: RestrictionAddress,
    pub governance_authority: RestrictionAddress,
    pub code_hash: String,
    pub reason: RestrictionReasonInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_blockhash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_uri_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_slot: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnbanCodeHashParams {
    pub proposer: RestrictionAddress,
    pub governance_authority: RestrictionAddress,
    pub code_hash: String,
    pub lift_reason: RestrictionLiftReasonInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restriction_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_blockhash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BridgeRouteRestrictionParams {
    pub proposer: RestrictionAddress,
    pub governance_authority: RestrictionAddress,
    pub chain: String,
    pub asset: String,
    pub reason: RestrictionReasonInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_blockhash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_uri_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_slot: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResumeBridgeRouteParams {
    pub proposer: RestrictionAddress,
    pub governance_authority: RestrictionAddress,
    pub chain: String,
    pub asset: String,
    pub lift_reason: RestrictionLiftReasonInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restriction_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_blockhash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtendRestrictionParams {
    pub proposer: RestrictionAddress,
    pub governance_authority: RestrictionAddress,
    pub restriction_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_expires_at_slot: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_blockhash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LiftRestrictionParams {
    pub proposer: RestrictionAddress,
    pub governance_authority: RestrictionAddress,
    pub restriction_id: u64,
    pub lift_reason: RestrictionLiftReasonInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_blockhash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MovementRestrictionParams {
    pub account: RestrictionAddress,
    pub asset: RestrictionAsset,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TransferRestrictionParams {
    pub from: RestrictionAddress,
    pub to: RestrictionAddress,
    pub asset: RestrictionAsset,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RestrictionTargetDetails {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub asset: Option<String>,
    #[serde(default)]
    pub contract: Option<String>,
    #[serde(default)]
    pub code_hash: Option<String>,
    #[serde(default)]
    pub chain: Option<String>,
    #[serde(default)]
    pub chain_id: Option<String>,
    #[serde(default)]
    pub module: Option<String>,
    #[serde(default)]
    pub module_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RestrictionModeDetails {
    pub kind: String,
    #[serde(default)]
    pub frozen_amount: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RestrictionRecord {
    pub id: u64,
    pub status: String,
    pub target_type: String,
    pub target: String,
    pub target_details: RestrictionTargetDetails,
    pub mode: String,
    pub mode_details: RestrictionModeDetails,
    #[serde(default)]
    pub frozen_amount: Option<u64>,
    pub reason: String,
    #[serde(default)]
    pub evidence_hash: Option<String>,
    #[serde(default)]
    pub evidence_uri_hash: Option<String>,
    pub proposer: String,
    pub authority: String,
    #[serde(default)]
    pub approval_authority: Option<String>,
    pub created_slot: u64,
    pub created_epoch: u64,
    #[serde(default)]
    pub expires_at_slot: Option<u64>,
    #[serde(default)]
    pub supersedes: Option<u64>,
    #[serde(default)]
    pub lifted_by: Option<String>,
    #[serde(default)]
    pub lifted_slot: Option<u64>,
    #[serde(default)]
    pub lift_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EffectiveRestrictionRecord {
    #[serde(flatten)]
    pub record: RestrictionRecord,
    pub effective_status: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GetRestrictionResponse {
    pub id: u64,
    pub slot: u64,
    pub found: bool,
    pub restriction: Option<EffectiveRestrictionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RestrictionListResponse {
    pub restrictions: Vec<EffectiveRestrictionRecord>,
    pub count: u64,
    pub has_more: bool,
    #[serde(default)]
    pub next_cursor: Option<String>,
    pub slot: u64,
    #[serde(default)]
    pub active_only: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RestrictionTargetStatus {
    pub slot: u64,
    pub target_type: String,
    pub target: String,
    pub target_details: RestrictionTargetDetails,
    pub restricted: bool,
    pub active: bool,
    pub restriction_ids: Vec<u64>,
    pub active_restriction_ids: Vec<u64>,
    pub restrictions: Vec<EffectiveRestrictionRecord>,
    pub active_restrictions: Vec<EffectiveRestrictionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContractLifecycleRestrictionStatus {
    pub contract: String,
    pub slot: u64,
    pub found: bool,
    pub is_executable: bool,
    pub lifecycle_status: String,
    pub lifecycle_updated_slot: u64,
    #[serde(default)]
    pub lifecycle_restriction_id: Option<u64>,
    pub derived_from_restriction: bool,
    pub active: bool,
    pub active_restriction_ids: Vec<u64>,
    pub active_restrictions: Vec<EffectiveRestrictionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodeHashRestrictionStatus {
    pub code_hash: String,
    pub slot: u64,
    pub blocked: bool,
    pub deploy_blocked: bool,
    pub active_restriction_ids: Vec<u64>,
    pub active_restrictions: Vec<RestrictionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BridgeRouteRestrictionStatus {
    pub chain: String,
    pub chain_id: String,
    pub asset: String,
    pub slot: u64,
    pub paused: bool,
    pub route_paused: bool,
    pub active_restriction_ids: Vec<u64>,
    pub active_restrictions: Vec<RestrictionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MovementRestrictionStatus {
    pub operation: String,
    pub account: String,
    pub asset: String,
    pub amount: u64,
    pub spendable: u64,
    pub slot: u64,
    pub allowed: bool,
    pub blocked: bool,
    pub active_restriction_ids: Vec<u64>,
    pub active_restrictions: Vec<RestrictionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransferRestrictionStatus {
    pub operation: String,
    pub from: String,
    pub to: String,
    pub asset: String,
    pub amount: u64,
    pub source_spendable: u64,
    pub recipient_spendable: u64,
    pub slot: u64,
    pub allowed: bool,
    pub blocked: bool,
    pub send_allowed: bool,
    pub receive_allowed: bool,
    pub source_blocked: bool,
    pub recipient_blocked: bool,
    pub source_restriction_ids: Vec<u64>,
    pub source_restrictions: Vec<RestrictionRecord>,
    pub recipient_restriction_ids: Vec<u64>,
    pub recipient_restrictions: Vec<RestrictionRecord>,
    pub active_restriction_ids: Vec<u64>,
    pub active_restrictions: Vec<RestrictionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RestrictionBuilderInstruction {
    pub program_id: String,
    pub accounts: Vec<String>,
    pub instruction_type: u64,
    #[serde(default)]
    pub governance_action_type: Option<u64>,
    pub data_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UnsignedRestrictionGovernanceTx {
    pub method: String,
    pub unsigned: bool,
    pub encoding: String,
    pub wire_format: String,
    pub tx_type: String,
    pub transaction_base64: String,
    pub transaction: String,
    pub wire_size: u64,
    pub message_hash: String,
    pub signature_count: u64,
    pub recent_blockhash: String,
    #[serde(default)]
    pub slot: Option<u64>,
    pub proposer: String,
    pub governance_authority: String,
    pub action_label: String,
    pub action: Value,
    pub instruction: RestrictionBuilderInstruction,
}

#[derive(Debug, Clone)]
pub struct RestrictionGovernanceClient {
    client: Client,
}

impl RestrictionGovernanceClient {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub fn from_rpc_url(rpc_url: impl Into<String>) -> Self {
        Self::new(Client::new(rpc_url))
    }

    async fn rpc<T>(&self, method: &str, params: Value) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let result = self.client.rpc_call(method, params).await?;
        serde_json::from_value(result).map_err(|err| Error::ParseError(err.to_string()))
    }

    fn one_param<T: Serialize>(params: T) -> Result<Value> {
        Ok(json!([serde_json::to_value(params).map_err(|err| {
            Error::SerializationError(err.to_string())
        })?]))
    }

    pub async fn get_restriction(&self, restriction_id: u64) -> Result<GetRestrictionResponse> {
        self.rpc("getRestriction", json!([restriction_id])).await
    }

    pub async fn list_restrictions(
        &self,
        params: Option<RestrictionListParams>,
    ) -> Result<RestrictionListResponse> {
        self.rpc(
            "listRestrictions",
            Self::one_param(params.unwrap_or_default())?,
        )
        .await
    }

    pub async fn list_active_restrictions(
        &self,
        params: Option<RestrictionListParams>,
    ) -> Result<RestrictionListResponse> {
        self.rpc(
            "listActiveRestrictions",
            Self::one_param(params.unwrap_or_default())?,
        )
        .await
    }

    pub async fn get_restriction_status(
        &self,
        target: RestrictionTargetInput,
    ) -> Result<RestrictionTargetStatus> {
        self.rpc("getRestrictionStatus", Self::one_param(target)?)
            .await
    }

    pub async fn get_account_restriction_status(
        &self,
        account: impl Into<RestrictionAddress>,
    ) -> Result<RestrictionTargetStatus> {
        self.rpc(
            "getAccountRestrictionStatus",
            json!([account.into().as_rpc_string()]),
        )
        .await
    }

    pub async fn get_asset_restriction_status(
        &self,
        asset: impl Into<RestrictionAsset>,
    ) -> Result<RestrictionTargetStatus> {
        self.rpc(
            "getAssetRestrictionStatus",
            json!([asset.into().as_rpc_string()]),
        )
        .await
    }

    pub async fn get_account_asset_restriction_status(
        &self,
        account: impl Into<RestrictionAddress>,
        asset: impl Into<RestrictionAsset>,
    ) -> Result<RestrictionTargetStatus> {
        self.rpc(
            "getAccountAssetRestrictionStatus",
            json!([account.into().as_rpc_string(), asset.into().as_rpc_string()]),
        )
        .await
    }

    pub async fn get_contract_lifecycle_status(
        &self,
        contract: impl Into<RestrictionAddress>,
    ) -> Result<ContractLifecycleRestrictionStatus> {
        self.rpc(
            "getContractLifecycleStatus",
            json!([contract.into().as_rpc_string()]),
        )
        .await
    }

    pub async fn get_code_hash_restriction_status(
        &self,
        code_hash: impl Into<String>,
    ) -> Result<CodeHashRestrictionStatus> {
        self.rpc("getCodeHashRestrictionStatus", json!([code_hash.into()]))
            .await
    }

    pub async fn get_bridge_route_restriction_status(
        &self,
        chain: impl Into<String>,
        asset: impl Into<String>,
    ) -> Result<BridgeRouteRestrictionStatus> {
        self.rpc(
            "getBridgeRouteRestrictionStatus",
            json!([chain.into(), asset.into()]),
        )
        .await
    }

    pub async fn can_send(
        &self,
        params: MovementRestrictionParams,
    ) -> Result<MovementRestrictionStatus> {
        self.rpc("canSend", Self::one_param(params)?).await
    }

    pub async fn can_receive(
        &self,
        params: MovementRestrictionParams,
    ) -> Result<MovementRestrictionStatus> {
        self.rpc("canReceive", Self::one_param(params)?).await
    }

    pub async fn can_transfer(
        &self,
        params: TransferRestrictionParams,
    ) -> Result<TransferRestrictionStatus> {
        self.rpc("canTransfer", Self::one_param(params)?).await
    }

    pub async fn build_restrict_account_tx(
        &self,
        params: RestrictAccountParams,
    ) -> Result<UnsignedRestrictionGovernanceTx> {
        self.rpc("buildRestrictAccountTx", Self::one_param(params)?)
            .await
    }

    pub async fn build_unrestrict_account_tx(
        &self,
        params: UnrestrictAccountParams,
    ) -> Result<UnsignedRestrictionGovernanceTx> {
        self.rpc("buildUnrestrictAccountTx", Self::one_param(params)?)
            .await
    }

    pub async fn build_restrict_account_asset_tx(
        &self,
        params: RestrictAccountAssetParams,
    ) -> Result<UnsignedRestrictionGovernanceTx> {
        self.rpc("buildRestrictAccountAssetTx", Self::one_param(params)?)
            .await
    }

    pub async fn build_unrestrict_account_asset_tx(
        &self,
        params: UnrestrictAccountAssetParams,
    ) -> Result<UnsignedRestrictionGovernanceTx> {
        self.rpc("buildUnrestrictAccountAssetTx", Self::one_param(params)?)
            .await
    }

    pub async fn build_set_frozen_asset_amount_tx(
        &self,
        params: SetFrozenAssetAmountParams,
    ) -> Result<UnsignedRestrictionGovernanceTx> {
        self.rpc("buildSetFrozenAssetAmountTx", Self::one_param(params)?)
            .await
    }

    pub async fn build_suspend_contract_tx(
        &self,
        params: ContractRestrictionParams,
    ) -> Result<UnsignedRestrictionGovernanceTx> {
        self.rpc("buildSuspendContractTx", Self::one_param(params)?)
            .await
    }

    pub async fn build_resume_contract_tx(
        &self,
        params: ResumeContractParams,
    ) -> Result<UnsignedRestrictionGovernanceTx> {
        self.rpc("buildResumeContractTx", Self::one_param(params)?)
            .await
    }

    pub async fn build_quarantine_contract_tx(
        &self,
        params: ContractRestrictionParams,
    ) -> Result<UnsignedRestrictionGovernanceTx> {
        self.rpc("buildQuarantineContractTx", Self::one_param(params)?)
            .await
    }

    pub async fn build_terminate_contract_tx(
        &self,
        params: ContractRestrictionParams,
    ) -> Result<UnsignedRestrictionGovernanceTx> {
        self.rpc("buildTerminateContractTx", Self::one_param(params)?)
            .await
    }

    pub async fn build_ban_code_hash_tx(
        &self,
        params: CodeHashRestrictionParams,
    ) -> Result<UnsignedRestrictionGovernanceTx> {
        self.rpc("buildBanCodeHashTx", Self::one_param(params)?)
            .await
    }

    pub async fn build_unban_code_hash_tx(
        &self,
        params: UnbanCodeHashParams,
    ) -> Result<UnsignedRestrictionGovernanceTx> {
        self.rpc("buildUnbanCodeHashTx", Self::one_param(params)?)
            .await
    }

    pub async fn build_pause_bridge_route_tx(
        &self,
        params: BridgeRouteRestrictionParams,
    ) -> Result<UnsignedRestrictionGovernanceTx> {
        self.rpc("buildPauseBridgeRouteTx", Self::one_param(params)?)
            .await
    }

    pub async fn build_resume_bridge_route_tx(
        &self,
        params: ResumeBridgeRouteParams,
    ) -> Result<UnsignedRestrictionGovernanceTx> {
        self.rpc("buildResumeBridgeRouteTx", Self::one_param(params)?)
            .await
    }

    pub async fn build_extend_restriction_tx(
        &self,
        params: ExtendRestrictionParams,
    ) -> Result<UnsignedRestrictionGovernanceTx> {
        self.rpc("buildExtendRestrictionTx", Self::one_param(params)?)
            .await
    }

    pub async fn build_lift_restriction_tx(
        &self,
        params: LiftRestrictionParams,
    ) -> Result<UnsignedRestrictionGovernanceTx> {
        self.rpc("buildLiftRestrictionTx", Self::one_param(params)?)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(byte: u8) -> Pubkey {
        Pubkey([byte; 32])
    }

    #[test]
    fn serializes_read_and_preflight_payloads() {
        let account = key(3);
        let recipient = key(4);
        let asset = key(5);

        assert_eq!(
            RestrictionGovernanceClient::one_param(RestrictionTargetInput::account_asset(
                account, "native"
            ))
            .unwrap(),
            json!([{
                "type": "account_asset",
                "account": account.to_base58(),
                "asset": "native"
            }])
        );

        assert_eq!(
            RestrictionGovernanceClient::one_param(RestrictionListParams {
                limit: Some(10),
                after_id: Some(2),
                cursor: None,
            })
            .unwrap(),
            json!([{ "limit": 10, "after_id": 2 }])
        );

        assert_eq!(
            RestrictionGovernanceClient::one_param(TransferRestrictionParams {
                from: account.into(),
                to: recipient.into(),
                asset: asset.into(),
                amount: Some(25),
            })
            .unwrap(),
            json!([{
                "from": account.to_base58(),
                "to": recipient.to_base58(),
                "asset": asset.to_base58(),
                "amount": 25
            }])
        );
    }

    #[test]
    fn serializes_builder_payloads() {
        let proposer = key(1);
        let authority = key(2);
        let account = key(3);
        let asset = key(5);

        assert_eq!(
            RestrictionGovernanceClient::one_param(RestrictAccountParams {
                proposer: proposer.into(),
                governance_authority: authority.into(),
                account: account.into(),
                reason: "testnet_drill".into(),
                mode: Some("outgoing_only".into()),
                recent_blockhash: Some("bb".repeat(32)),
                evidence_hash: Some("aa".repeat(32)),
                evidence_uri_hash: None,
                expires_at_slot: Some(123),
            })
            .unwrap(),
            json!([{
                "proposer": proposer.to_base58(),
                "governance_authority": authority.to_base58(),
                "account": account.to_base58(),
                "reason": "testnet_drill",
                "mode": "outgoing_only",
                "recent_blockhash": "bb".repeat(32),
                "evidence_hash": "aa".repeat(32),
                "expires_at_slot": 123
            }])
        );

        assert_eq!(
            RestrictionGovernanceClient::one_param(SetFrozenAssetAmountParams {
                proposer: proposer.into(),
                governance_authority: authority.into(),
                account: account.into(),
                asset: asset.into(),
                amount: 500,
                reason: lichen_core::RestrictionReason::StolenFunds.into(),
                recent_blockhash: None,
                evidence_hash: None,
                evidence_uri_hash: None,
                expires_at_slot: None,
            })
            .unwrap(),
            json!([{
                "proposer": proposer.to_base58(),
                "governance_authority": authority.to_base58(),
                "account": account.to_base58(),
                "asset": asset.to_base58(),
                "amount": 500,
                "reason": "stolen_funds"
            }])
        );

        assert_eq!(
            RestrictionGovernanceClient::one_param(ResumeBridgeRouteParams {
                proposer: proposer.into(),
                governance_authority: authority.into(),
                chain: BridgeChain::NeoX.into(),
                asset: BridgeAsset::Gas.into(),
                lift_reason: lichen_core::RestrictionLiftReason::TestnetDrillComplete.into(),
                restriction_id: Some(12),
                recent_blockhash: None,
            })
            .unwrap(),
            json!([{
                "proposer": proposer.to_base58(),
                "governance_authority": authority.to_base58(),
                "chain": "neox",
                "asset": "gas",
                "lift_reason": "testnet_drill_complete",
                "restriction_id": 12
            }])
        );
    }

    #[test]
    fn deserializes_builder_response() {
        let response: UnsignedRestrictionGovernanceTx = serde_json::from_value(json!({
            "method": "buildRestrictAccountTx",
            "unsigned": true,
            "encoding": "base64",
            "wire_format": "lichen_tx_v1",
            "tx_type": "native",
            "transaction_base64": "AA==",
            "transaction": "AA==",
            "wire_size": 1,
            "message_hash": "00",
            "signature_count": 0,
            "recent_blockhash": "00",
            "slot": null,
            "proposer": "",
            "governance_authority": "",
            "action_label": "restrict",
            "action": {},
            "instruction": {
                "program_id": "",
                "accounts": [],
                "instruction_type": 34,
                "governance_action_type": 10,
                "data_hex": ""
            }
        }))
        .unwrap();

        assert!(response.unsigned);
        assert_eq!(response.method, "buildRestrictAccountTx");
        assert_eq!(response.instruction.instruction_type, 34);
    }
}
