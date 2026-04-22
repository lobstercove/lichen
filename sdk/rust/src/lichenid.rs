use crate::{Client, Error, Keypair, Pubkey, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Mutex;

const PREMIUM_NAME_MIN_LENGTH: usize = 3;
const PREMIUM_NAME_MAX_LENGTH: usize = 4;
const DIRECT_NAME_MIN_LENGTH: usize = 5;
const MAX_NAME_LENGTH: usize = 32;
const MAX_SKILL_NAME_BYTES: usize = 32;
const MAX_ENDPOINT_BYTES: usize = 255;
const MAX_METADATA_BYTES: usize = 1024;
const SPORES_PER_LICN: u64 = 1_000_000_000;
const PROGRAM_SYMBOL_CANDIDATES: [&str; 3] = ["YID", "yid", "LICHENID"];

pub const LICHENID_DELEGATE_PERM_PROFILE: u8 = 0b0000_0001;
pub const LICHENID_DELEGATE_PERM_AGENT_TYPE: u8 = 0b0000_0010;
pub const LICHENID_DELEGATE_PERM_SKILLS: u8 = 0b0000_0100;
pub const LICHENID_DELEGATE_PERM_NAMING: u8 = 0b0000_1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdReputation {
    pub address: String,
    pub score: u64,
    pub tier: u8,
    pub tier_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdNameResolution {
    pub name: String,
    pub owner: String,
    pub registered_slot: u64,
    pub expiry_slot: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdSkill {
    pub index: u8,
    pub name: String,
    pub proficiency: u8,
    pub attestation_count: u64,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdReceivedVouch {
    pub voucher: String,
    pub voucher_name: Option<String>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdGivenVouch {
    pub vouchee: String,
    pub vouchee_name: Option<String>,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdVouches {
    pub received: Vec<LichenIdReceivedVouch>,
    pub given: Vec<LichenIdGivenVouch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdNameAuction {
    pub name: String,
    pub active: bool,
    pub start_slot: u64,
    pub end_slot: u64,
    pub reserve_bid: u64,
    pub highest_bid: u64,
    pub highest_bidder: String,
    pub current_slot: u64,
    pub ended: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdAchievement {
    pub id: u8,
    pub name: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdAgentConfig {
    pub endpoint: Option<String>,
    pub metadata: Option<Value>,
    pub availability: u8,
    pub availability_name: String,
    pub rate: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdContributions {
    pub successful_txs: u64,
    pub governance_votes: u64,
    pub programs_deployed: u64,
    pub uptime_hours: u64,
    pub peer_endorsements: u64,
    pub failed_txs: u64,
    pub slashing_events: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdIdentitySummary {
    pub address: String,
    pub owner: String,
    pub name: String,
    pub agent_type: u8,
    pub agent_type_name: String,
    pub reputation: u64,
    pub created_at: u64,
    pub updated_at: u64,
    pub skill_count: u8,
    pub vouch_count: u8,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdReputationSummary {
    pub score: u64,
    pub tier: u8,
    pub tier_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdProfile {
    pub identity: LichenIdIdentitySummary,
    pub licn_name: Option<String>,
    pub reputation: LichenIdReputationSummary,
    pub skills: Vec<LichenIdSkill>,
    pub vouches: LichenIdVouches,
    pub achievements: Vec<LichenIdAchievement>,
    pub agent: LichenIdAgentConfig,
    pub contributions: LichenIdContributions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdAgentDirectoryEntry {
    pub address: String,
    pub name: String,
    pub licn_name: Option<String>,
    pub agent_type: u8,
    pub agent_type_name: String,
    pub reputation: u64,
    pub trust_tier: u8,
    pub trust_tier_name: String,
    pub availability: u8,
    pub available: bool,
    pub rate: u64,
    pub endpoint: Option<String>,
    pub skill_count: u8,
    pub vouch_count: u8,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdAgentDirectory {
    pub agents: Vec<LichenIdAgentDirectoryEntry>,
    pub count: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdStats {
    pub total_identities: u64,
    pub total_names: u64,
    pub total_skills: u64,
    pub total_vouches: u64,
    pub total_attestations: u64,
    pub tier_distribution: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LichenIdAgentDirectoryOptions {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_reputation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct RegisterIdentityParams {
    pub agent_type: u8,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct RegisterNameParams {
    pub name: String,
    pub duration_years: u8,
    pub value_spores: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct AddSkillParams {
    pub name: String,
    pub proficiency: u8,
}

#[derive(Debug, Clone)]
pub struct SetEndpointParams {
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct SetRateParams {
    pub rate_spores: u64,
}

#[derive(Debug, Clone)]
pub struct SetMetadataParams {
    pub metadata_json: String,
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LichenIdAvailability {
    Offline = 0,
    Available = 1,
    Busy = 2,
}

impl TryFrom<u8> for LichenIdAvailability {
    type Error = Error;

    fn try_from(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Offline),
            1 => Ok(Self::Available),
            2 => Ok(Self::Busy),
            _ => Err(Error::BuildError(
                "Availability must be one of offline, available, or busy".into(),
            )),
        }
    }
}

impl From<LichenIdAvailability> for u8 {
    fn from(value: LichenIdAvailability) -> Self {
        value as u8
    }
}

#[derive(Debug, Clone)]
pub struct SetAvailabilityParams {
    pub status: LichenIdAvailability,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LichenIdDelegateRecord {
    pub owner: String,
    pub delegate: String,
    pub permissions: u8,
    pub expires_at_ms: u64,
    pub created_at_ms: u64,
    pub active: bool,
    pub can_profile: bool,
    pub can_agent_type: bool,
    pub can_skills: bool,
    pub can_naming: bool,
}

#[derive(Debug, Clone)]
pub struct SetDelegateParams {
    pub delegate: Pubkey,
    pub permissions: u8,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone)]
pub struct SetEndpointAsParams {
    pub owner: Pubkey,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct SetMetadataAsParams {
    pub owner: Pubkey,
    pub metadata_json: String,
}

#[derive(Debug, Clone)]
pub struct SetAvailabilityAsParams {
    pub owner: Pubkey,
    pub status: LichenIdAvailability,
}

#[derive(Debug, Clone)]
pub struct SetRateAsParams {
    pub owner: Pubkey,
    pub rate_spores: u64,
}

#[derive(Debug, Clone)]
pub struct UpdateAgentTypeAsParams {
    pub owner: Pubkey,
    pub agent_type: u8,
}

#[derive(Debug, Clone)]
pub struct SetRecoveryGuardiansParams {
    pub guardians: Vec<Pubkey>,
}

#[derive(Debug, Clone)]
pub struct ApproveRecoveryParams {
    pub target: Pubkey,
    pub new_owner: Pubkey,
}

#[derive(Debug, Clone)]
pub struct ExecuteRecoveryParams {
    pub target: Pubkey,
    pub new_owner: Pubkey,
}

#[derive(Debug, Clone)]
pub struct AttestSkillParams {
    pub identity: Pubkey,
    pub name: String,
    pub level: u8,
}

#[derive(Debug, Clone)]
pub struct RevokeAttestationParams {
    pub identity: Pubkey,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct CreateNameAuctionParams {
    pub name: String,
    pub reserve_bid_spores: u64,
    pub end_slot: u64,
}

#[derive(Debug, Clone)]
pub struct BidNameAuctionParams {
    pub name: String,
    pub bid_amount_spores: u64,
}

#[derive(Debug, Clone)]
pub struct FinalizeNameAuctionParams {
    pub name: String,
    pub duration_years: u8,
}

#[derive(Debug, Clone)]
pub struct LichenIdClient {
    client: Client,
    program_id: std::sync::Arc<Mutex<Option<Pubkey>>>,
}

fn normalize_name_label(name: &str) -> String {
    name.trim()
        .to_ascii_lowercase()
        .trim_end_matches(".lichen")
        .to_string()
}

fn has_valid_name_characters(label: &str) -> bool {
    !label.is_empty()
        && !label.starts_with('-')
        && !label.ends_with('-')
        && !label.contains("--")
        && label
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

fn validate_lookup_name(name: &str) -> Result<String> {
    let label = normalize_name_label(name);
    if label.is_empty() {
        return Err(Error::BuildError("Name cannot be empty".into()));
    }
    if label.len() > MAX_NAME_LENGTH {
        return Err(Error::BuildError(
            "LichenID names must be at most 32 characters".into(),
        ));
    }
    if !has_valid_name_characters(&label) {
        return Err(Error::BuildError(
            "LichenID names must use lowercase a-z, 0-9, and internal hyphens only".into(),
        ));
    }
    Ok(label)
}

fn validate_direct_registration_name(name: &str) -> Result<String> {
    let label = validate_lookup_name(name)?;
    if label.len() < DIRECT_NAME_MIN_LENGTH {
        return Err(Error::BuildError(
            "Direct register_name supports 5-32 character labels; 3-4 character names are auction-only".into(),
        ));
    }
    Ok(label)
}

fn validate_auction_name(name: &str) -> Result<String> {
    let label = validate_lookup_name(name)?;
    if !(PREMIUM_NAME_MIN_LENGTH..=PREMIUM_NAME_MAX_LENGTH).contains(&label.len()) {
        return Err(Error::BuildError(
            "Name auction helpers support 3-4 character premium labels only".into(),
        ));
    }
    Ok(label)
}

fn normalize_duration_years(duration_years: u8) -> u8 {
    duration_years.max(1).min(10)
}

fn validate_skill_name(name: &str) -> Result<String> {
    let skill_name = name.trim();
    if skill_name.is_empty() {
        return Err(Error::BuildError("Skill name cannot be empty".into()));
    }
    if skill_name.as_bytes().len() > MAX_SKILL_NAME_BYTES {
        return Err(Error::BuildError(
            "Skill names must be at most 32 bytes".into(),
        ));
    }
    Ok(skill_name.to_string())
}

fn normalize_endpoint_url(url: &str) -> Result<String> {
    let endpoint = url.trim();
    if endpoint.is_empty() {
        return Err(Error::BuildError("Endpoint URL cannot be empty".into()));
    }
    if endpoint.as_bytes().len() > MAX_ENDPOINT_BYTES {
        return Err(Error::BuildError(
            "Endpoint URL must be at most 255 bytes".into(),
        ));
    }
    Ok(endpoint.to_string())
}

fn normalize_metadata_json(metadata_json: &str) -> Result<String> {
    let trimmed = metadata_json.trim();
    if trimmed.is_empty() {
        return Err(Error::BuildError("Metadata cannot be empty".into()));
    }
    if trimmed.as_bytes().len() > MAX_METADATA_BYTES {
        return Err(Error::BuildError(
            "Metadata must be at most 1024 bytes".into(),
        ));
    }
    Ok(trimmed.to_string())
}

fn normalize_delegate_permissions(permissions: u8) -> Result<u8> {
    let allowed_mask = LICHENID_DELEGATE_PERM_PROFILE
        | LICHENID_DELEGATE_PERM_AGENT_TYPE
        | LICHENID_DELEGATE_PERM_SKILLS
        | LICHENID_DELEGATE_PERM_NAMING;
    if permissions == 0 || permissions & !allowed_mask != 0 {
        return Err(Error::BuildError(
            "Delegate permissions must be a non-zero PROFILE/AGENT_TYPE/SKILLS/NAMING bitmask".into(),
        ));
    }
    Ok(permissions)
}

fn normalize_attestation_level(level: u8) -> Result<u8> {
    if !(1..=5).contains(&level) {
        return Err(Error::BuildError(
            "Attestation level must be between 1 and 5".into(),
        ));
    }
    Ok(level)
}

fn current_time_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn pad_bytes(data: &[u8], size: usize) -> Vec<u8> {
    let mut out = vec![0u8; size];
    let copy_len = data.len().min(size);
    out[..copy_len].copy_from_slice(&data[..copy_len]);
    out
}

fn build_layout_args(layout: &[u8], chunks: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + layout.len() + chunks.iter().map(|chunk| chunk.len()).sum::<usize>());
    out.push(0xAB);
    out.extend_from_slice(layout);
    for chunk in chunks {
        out.extend_from_slice(chunk);
    }
    out
}

fn encode_register_identity_args(owner: &Pubkey, params: &RegisterIdentityParams) -> Vec<u8> {
    let name_bytes = params.name.as_bytes();
    build_layout_args(&[0x20, 0x01, 0x40, 0x04], &[
        owner.as_ref().to_vec(),
        vec![params.agent_type],
        pad_bytes(name_bytes, 64),
        (name_bytes.len() as u32).to_le_bytes().to_vec(),
    ])
}

fn encode_name_duration_args(owner: &Pubkey, name: &str, duration_years: u8) -> Vec<u8> {
    let name_bytes = name.as_bytes();
    build_layout_args(&[0x20, 0x20, 0x04, 0x01], &[
        owner.as_ref().to_vec(),
        pad_bytes(name_bytes, 32),
        (name_bytes.len() as u32).to_le_bytes().to_vec(),
        vec![duration_years],
    ])
}

fn encode_add_skill_args(owner: &Pubkey, params: &AddSkillParams) -> Result<Vec<u8>> {
    let skill_name = validate_skill_name(&params.name)?;
    let skill_bytes = skill_name.as_bytes();
    Ok(build_layout_args(&[0x20, 0x20, 0x04, 0x01], &[
        owner.as_ref().to_vec(),
        pad_bytes(skill_bytes, 32),
        (skill_bytes.len() as u32).to_le_bytes().to_vec(),
        vec![params.proficiency.min(100)],
    ]))
}

fn encode_vouch_args(owner: &Pubkey, vouchee: &Pubkey) -> Vec<u8> {
    build_layout_args(&[0x20, 0x20], &[
        owner.as_ref().to_vec(),
        vouchee.as_ref().to_vec(),
    ])
}

fn encode_endpoint_args(owner: &Pubkey, url: &str) -> Result<Vec<u8>> {
    let endpoint = normalize_endpoint_url(url)?;
    let endpoint_bytes = endpoint.as_bytes();
    let stride = endpoint_bytes.len().max(32);
    Ok(build_layout_args(&[0x20, stride as u8, 0x04], &[
        owner.as_ref().to_vec(),
        pad_bytes(endpoint_bytes, stride),
        (endpoint_bytes.len() as u32).to_le_bytes().to_vec(),
    ]))
}

fn encode_metadata_args(owner: &Pubkey, metadata_json: &str) -> Result<Vec<u8>> {
    let metadata = normalize_metadata_json(metadata_json)?;
    let metadata_bytes = metadata.as_bytes();
    let stride = metadata_bytes.len().max(32);
    Ok(build_layout_args(&[0x20, stride as u8, 0x04], &[
        owner.as_ref().to_vec(),
        pad_bytes(metadata_bytes, stride),
        (metadata_bytes.len() as u32).to_le_bytes().to_vec(),
    ]))
}

fn encode_rate_args(owner: &Pubkey, rate_spores: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(40);
    out.extend_from_slice(owner.as_ref());
    out.extend_from_slice(&rate_spores.to_le_bytes());
    out
}

fn encode_availability_args(owner: &Pubkey, status: LichenIdAvailability) -> Vec<u8> {
    build_layout_args(&[0x20, 0x01], &[
        owner.as_ref().to_vec(),
        vec![status.into()],
    ])
}

fn encode_set_delegate_args(owner: &Pubkey, params: &SetDelegateParams) -> Result<Vec<u8>> {
    Ok(build_layout_args(&[0x20, 0x20, 0x01, 0x08], &[
        owner.as_ref().to_vec(),
        params.delegate.as_ref().to_vec(),
        vec![normalize_delegate_permissions(params.permissions)?],
        params.expires_at_ms.to_le_bytes().to_vec(),
    ]))
}

fn encode_delegate_lookup_args(owner: &Pubkey, delegate: &Pubkey) -> Vec<u8> {
    build_layout_args(&[0x20, 0x20], &[
        owner.as_ref().to_vec(),
        delegate.as_ref().to_vec(),
    ])
}

fn encode_delegated_endpoint_args(delegate: &Pubkey, params: &SetEndpointAsParams) -> Result<Vec<u8>> {
    let endpoint = normalize_endpoint_url(&params.url)?;
    let endpoint_bytes = endpoint.as_bytes();
    let stride = endpoint_bytes.len().max(32);
    Ok(build_layout_args(&[0x20, 0x20, stride as u8, 0x04], &[
        delegate.as_ref().to_vec(),
        params.owner.as_ref().to_vec(),
        pad_bytes(endpoint_bytes, stride),
        (endpoint_bytes.len() as u32).to_le_bytes().to_vec(),
    ]))
}

fn encode_delegated_metadata_args(delegate: &Pubkey, params: &SetMetadataAsParams) -> Result<Vec<u8>> {
    let metadata = normalize_metadata_json(&params.metadata_json)?;
    let metadata_bytes = metadata.as_bytes();
    let stride = metadata_bytes.len().max(32);
    Ok(build_layout_args(&[0x20, 0x20, stride as u8, 0x04], &[
        delegate.as_ref().to_vec(),
        params.owner.as_ref().to_vec(),
        pad_bytes(metadata_bytes, stride),
        (metadata_bytes.len() as u32).to_le_bytes().to_vec(),
    ]))
}

fn encode_delegated_availability_args(delegate: &Pubkey, params: &SetAvailabilityAsParams) -> Vec<u8> {
    build_layout_args(&[0x20, 0x20, 0x01], &[
        delegate.as_ref().to_vec(),
        params.owner.as_ref().to_vec(),
        vec![params.status.into()],
    ])
}

fn encode_delegated_rate_args(delegate: &Pubkey, params: &SetRateAsParams) -> Vec<u8> {
    build_layout_args(&[0x20, 0x20, 0x08], &[
        delegate.as_ref().to_vec(),
        params.owner.as_ref().to_vec(),
        params.rate_spores.to_le_bytes().to_vec(),
    ])
}

fn encode_update_agent_type_as_args(delegate: &Pubkey, params: &UpdateAgentTypeAsParams) -> Vec<u8> {
    build_layout_args(&[0x20, 0x20, 0x01], &[
        delegate.as_ref().to_vec(),
        params.owner.as_ref().to_vec(),
        vec![params.agent_type],
    ])
}

fn encode_recovery_guardians_args(owner: &Pubkey, params: &SetRecoveryGuardiansParams) -> Result<Vec<u8>> {
    if params.guardians.len() != 5 {
        return Err(Error::BuildError(
            "Recovery helpers require exactly 5 guardian addresses".into(),
        ));
    }

    let unique: std::collections::BTreeSet<Pubkey> = params.guardians.iter().copied().collect();
    if unique.len() != 5 {
        return Err(Error::BuildError("Recovery guardians must be unique".into()));
    }
    if params.guardians.iter().any(|guardian| guardian == owner) {
        return Err(Error::BuildError(
            "Recovery guardians cannot include the owner".into(),
        ));
    }

    Ok(build_layout_args(&[0x20, 0x20, 0x20, 0x20, 0x20, 0x20], &[
        owner.as_ref().to_vec(),
        params.guardians[0].as_ref().to_vec(),
        params.guardians[1].as_ref().to_vec(),
        params.guardians[2].as_ref().to_vec(),
        params.guardians[3].as_ref().to_vec(),
        params.guardians[4].as_ref().to_vec(),
    ]))
}

fn encode_recovery_action_args(caller: &Pubkey, target: &Pubkey, new_owner: &Pubkey) -> Vec<u8> {
    build_layout_args(&[0x20, 0x20, 0x20], &[
        caller.as_ref().to_vec(),
        target.as_ref().to_vec(),
        new_owner.as_ref().to_vec(),
    ])
}

fn encode_attest_skill_args(attester: &Pubkey, params: &AttestSkillParams) -> Result<Vec<u8>> {
    let skill_name = validate_skill_name(&params.name)?;
    let level = normalize_attestation_level(params.level)?;
    let skill_bytes = skill_name.as_bytes();
    Ok(build_layout_args(&[0x20, 0x20, 0x20, 0x04, 0x01], &[
        attester.as_ref().to_vec(),
        params.identity.as_ref().to_vec(),
        pad_bytes(skill_bytes, 32),
        (skill_bytes.len() as u32).to_le_bytes().to_vec(),
        vec![level],
    ]))
}

fn encode_get_attestations_args(identity: &Pubkey, name: &str) -> Result<Vec<u8>> {
    let skill_name = validate_skill_name(name)?;
    let skill_bytes = skill_name.as_bytes();
    Ok(build_layout_args(&[0x20, 0x20, 0x04], &[
        identity.as_ref().to_vec(),
        pad_bytes(skill_bytes, 32),
        (skill_bytes.len() as u32).to_le_bytes().to_vec(),
    ]))
}

fn encode_revoke_attestation_args(attester: &Pubkey, params: &RevokeAttestationParams) -> Result<Vec<u8>> {
    let skill_name = validate_skill_name(&params.name)?;
    let skill_bytes = skill_name.as_bytes();
    Ok(build_layout_args(&[0x20, 0x20, 0x20, 0x04], &[
        attester.as_ref().to_vec(),
        params.identity.as_ref().to_vec(),
        pad_bytes(skill_bytes, 32),
        (skill_bytes.len() as u32).to_le_bytes().to_vec(),
    ]))
}

fn encode_create_name_auction_args(owner: &Pubkey, params: &CreateNameAuctionParams) -> Result<Vec<u8>> {
    let label = validate_auction_name(&params.name)?;
    let name_bytes = label.as_bytes();
    let out = build_layout_args(&[0x20, 0x20, 0x04, 0x08, 0x08], &[
        owner.as_ref().to_vec(),
        pad_bytes(name_bytes, 32),
        (name_bytes.len() as u32).to_le_bytes().to_vec(),
        params.reserve_bid_spores.to_le_bytes().to_vec(),
        params.end_slot.to_le_bytes().to_vec(),
    ]);
    Ok(out)
}

fn encode_bid_name_auction_args(owner: &Pubkey, params: &BidNameAuctionParams) -> Result<Vec<u8>> {
    let label = validate_auction_name(&params.name)?;
    let name_bytes = label.as_bytes();
    let out = build_layout_args(&[0x20, 0x20, 0x04, 0x08], &[
        owner.as_ref().to_vec(),
        pad_bytes(name_bytes, 32),
        (name_bytes.len() as u32).to_le_bytes().to_vec(),
        params.bid_amount_spores.to_le_bytes().to_vec(),
    ]);
    Ok(out)
}

fn registration_cost_per_year_licn(name: &str) -> u64 {
    let label = normalize_name_label(name);
    match label.len() {
        0..=3 => 500,
        4 => 100,
        _ => 20,
    }
}

pub fn estimate_lichenid_name_registration_cost(name: &str, duration_years: u8) -> Result<u64> {
    let years = duration_years.max(1).min(10) as u64;
    registration_cost_per_year_licn(name)
        .checked_mul(years)
        .and_then(|value| value.checked_mul(SPORES_PER_LICN))
        .ok_or_else(|| Error::BuildError("LichenID name registration cost overflow".into()))
}

fn ensure_readonly_success(
    result: &crate::client::ReadonlyContractResult,
    function_name: &str,
) -> Result<()> {
    let code = result.return_code.unwrap_or(0);
    if code != 0 {
        return Err(Error::RpcError(
            result
                .error
                .clone()
                .unwrap_or_else(|| format!("LichenID {} returned code {}", function_name, code)),
        ));
    }
    if !result.success {
        return Err(Error::RpcError(
            result
                .error
                .clone()
                .unwrap_or_else(|| format!("LichenID {} failed", function_name)),
        ));
    }
    Ok(())
}

fn decode_return_data(result: &crate::client::ReadonlyContractResult, function_name: &str) -> Result<Vec<u8>> {
    let Some(return_data) = &result.return_data else {
        return Err(Error::ParseError(format!(
            "LichenID {} did not return payload data",
            function_name,
        )));
    };

    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, return_data)
        .map_err(|err| Error::ParseError(err.to_string()))
}

fn decode_delegate_record(owner: &Pubkey, delegate: &Pubkey, bytes: &[u8]) -> Result<LichenIdDelegateRecord> {
    if bytes.len() < 17 {
        return Err(Error::ParseError(
            "Delegate record payload was shorter than expected".into(),
        ));
    }

    let permissions = bytes[0];
    let expires_at_ms = u64::from_le_bytes(bytes[1..9].try_into().unwrap());
    let created_at_ms = u64::from_le_bytes(bytes[9..17].try_into().unwrap());

    Ok(LichenIdDelegateRecord {
        owner: owner.to_base58(),
        delegate: delegate.to_base58(),
        permissions,
        expires_at_ms,
        created_at_ms,
        active: current_time_millis() < expires_at_ms,
        can_profile: permissions & LICHENID_DELEGATE_PERM_PROFILE != 0,
        can_agent_type: permissions & LICHENID_DELEGATE_PERM_AGENT_TYPE != 0,
        can_skills: permissions & LICHENID_DELEGATE_PERM_SKILLS != 0,
        can_naming: permissions & LICHENID_DELEGATE_PERM_NAMING != 0,
    })
}

impl LichenIdClient {
    pub fn new(client: Client) -> Self {
        Self {
            client,
            program_id: std::sync::Arc::new(Mutex::new(None)),
        }
    }

    pub fn with_program_id(client: Client, program_id: Pubkey) -> Self {
        Self {
            client,
            program_id: std::sync::Arc::new(Mutex::new(Some(program_id))),
        }
    }

    pub async fn get_program_id(&self) -> Result<Pubkey> {
        if let Some(program_id) = self
            .program_id
            .lock()
            .map_err(|_| Error::ConfigError("LichenIdClient program cache lock poisoned".into()))?
            .clone()
        {
            return Ok(program_id);
        }

        for symbol in PROGRAM_SYMBOL_CANDIDATES {
            let entry = match self.client.get_symbol_registry(symbol).await {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let Some(program) = entry.get("program").and_then(|value| value.as_str()) else {
                continue;
            };
            let program_id = Pubkey::from_base58(program).map_err(Error::ParseError)?;
            *self
                .program_id
                .lock()
                .map_err(|_| Error::ConfigError("LichenIdClient program cache lock poisoned".into()))? = Some(program_id);
            return Ok(program_id);
        }

        Err(Error::ConfigError(
            "Unable to resolve the LichenID program via getSymbolRegistry(\"YID\")".into(),
        ))
    }

    pub async fn get_profile(&self, address: &Pubkey) -> Result<Option<LichenIdProfile>> {
        let value = self.client.get_lichenid_profile(address).await?;
        if value.is_null() {
            return Ok(None);
        }
        serde_json::from_value(value).map(Some).map_err(|err| Error::ParseError(err.to_string()))
    }

    pub async fn get_reputation(&self, address: &Pubkey) -> Result<LichenIdReputation> {
        let value = self.client.get_lichenid_reputation(address).await?;
        serde_json::from_value(value).map_err(|err| Error::ParseError(err.to_string()))
    }

    pub async fn get_skills(&self, address: &Pubkey) -> Result<Vec<LichenIdSkill>> {
        let value = self.client.get_lichenid_skills(address).await?;
        serde_json::from_value(value).map_err(|err| Error::ParseError(err.to_string()))
    }

    pub async fn get_vouches(&self, address: &Pubkey) -> Result<LichenIdVouches> {
        let value = self.client.get_lichenid_vouches(address).await?;
        serde_json::from_value(value).map_err(|err| Error::ParseError(err.to_string()))
    }

    pub async fn get_metadata(&self, address: &Pubkey) -> Result<Option<Value>> {
        Ok(self
            .get_profile(address)
            .await?
            .and_then(|profile| profile.agent.metadata))
    }

    pub async fn get_delegate(&self, owner: &Pubkey, delegate: &Pubkey) -> Result<Option<LichenIdDelegateRecord>> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_delegate",
                encode_delegate_lookup_args(owner, delegate),
                None,
            )
            .await?;

        if result.return_code == Some(1) || result.return_data.is_none() {
            return Ok(None);
        }

        ensure_readonly_success(&result, "get_delegate")?;
        let bytes = decode_return_data(&result, "get_delegate")?;
        decode_delegate_record(owner, delegate, &bytes).map(Some)
    }

    pub async fn get_attestations(&self, identity: &Pubkey, skill_name: &str) -> Result<u64> {
        let result = self
            .client
            .call_readonly_contract(
                &self.get_program_id().await?,
                "get_attestations",
                encode_get_attestations_args(identity, skill_name)?,
                None,
            )
            .await?;

        ensure_readonly_success(&result, "get_attestations")?;
        let bytes = decode_return_data(&result, "get_attestations")?;
        if bytes.len() < 8 {
            return Err(Error::ParseError(
                "Attestation count payload was shorter than expected".into(),
            ));
        }
        Ok(u64::from_le_bytes(bytes[..8].try_into().unwrap()))
    }

    pub async fn resolve_name(&self, name: &str) -> Result<Option<LichenIdNameResolution>> {
        let label = validate_lookup_name(name)?;
        let value = self.client.resolve_lichen_name(&format!("{}.lichen", label)).await?;
        if value.is_null() {
            return Ok(None);
        }
        serde_json::from_value(value).map(Some).map_err(|err| Error::ParseError(err.to_string()))
    }

    pub async fn get_name_auction(&self, name: &str) -> Result<Option<LichenIdNameAuction>> {
        let value = self.client.get_name_auction(&validate_lookup_name(name)?).await?;
        if value.is_null() {
            return Ok(None);
        }
        serde_json::from_value(value).map(Some).map_err(|err| Error::ParseError(err.to_string()))
    }

    pub async fn get_agent_directory(&self) -> Result<LichenIdAgentDirectory> {
        self.search_agents(LichenIdAgentDirectoryOptions::default()).await
    }

    pub async fn search_agents(&self, options: LichenIdAgentDirectoryOptions) -> Result<LichenIdAgentDirectory> {
        let has_filters = options.agent_type.is_some()
            || options.available.is_some()
            || options.min_reputation.is_some()
            || options.limit.is_some()
            || options.offset.is_some();
        let value = if has_filters {
            let options_value = serde_json::to_value(options)
                .map_err(|err| Error::SerializationError(err.to_string()))?;
            self.client.get_lichenid_agent_directory(Some(options_value)).await?
        } else {
            self.client.get_lichenid_agent_directory(None).await?
        };
        serde_json::from_value(value).map_err(|err| Error::ParseError(err.to_string()))
    }

    pub async fn get_stats(&self) -> Result<LichenIdStats> {
        let value = self.client.get_lichenid_stats().await?;
        serde_json::from_value(value).map_err(|err| Error::ParseError(err.to_string()))
    }

    pub async fn register_identity(&self, owner: &Keypair, params: RegisterIdentityParams) -> Result<String> {
        let name = params.name.trim().to_string();
        if name.is_empty() {
            return Err(Error::BuildError("Identity name cannot be empty".into()));
        }
        let program_id = self.get_program_id().await?;
        let args = encode_register_identity_args(
            &owner.pubkey(),
            &RegisterIdentityParams {
                agent_type: params.agent_type,
                name,
            },
        );
        self.client
            .call_contract(owner, &program_id, "register_identity", args, 0)
            .await
    }

    pub async fn register_name(&self, owner: &Keypair, params: RegisterNameParams) -> Result<String> {
        let duration_years = normalize_duration_years(params.duration_years);
        let label = validate_direct_registration_name(&params.name)?;
        let value = match params.value_spores {
            Some(value) => value,
            None => estimate_lichenid_name_registration_cost(&label, duration_years)?,
        };
        let program_id = self.get_program_id().await?;
        let args = encode_name_duration_args(&owner.pubkey(), &label, duration_years);
        self.client
            .call_contract(owner, &program_id, "register_name", args, value)
            .await
    }

    pub async fn add_skill(&self, owner: &Keypair, params: AddSkillParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_add_skill_args(&owner.pubkey(), &params)?;
        self.client
            .call_contract(owner, &program_id, "add_skill", args, 0)
            .await
    }

    pub async fn vouch(&self, owner: &Keypair, vouchee: &Pubkey) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_vouch_args(&owner.pubkey(), vouchee);
        self.client
            .call_contract(owner, &program_id, "vouch", args, 0)
            .await
    }

    pub async fn set_endpoint(&self, owner: &Keypair, params: SetEndpointParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_endpoint_args(&owner.pubkey(), &params.url)?;
        self.client
            .call_contract(owner, &program_id, "set_endpoint", args, 0)
            .await
    }

    pub async fn set_metadata(&self, owner: &Keypair, params: SetMetadataParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_metadata_args(&owner.pubkey(), &params.metadata_json)?;
        self.client
            .call_contract(owner, &program_id, "set_metadata", args, 0)
            .await
    }

    pub async fn set_rate(&self, owner: &Keypair, params: SetRateParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_rate_args(&owner.pubkey(), params.rate_spores);
        self.client
            .call_contract(owner, &program_id, "set_rate", args, 0)
            .await
    }

    pub async fn set_availability(&self, owner: &Keypair, params: SetAvailabilityParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_availability_args(&owner.pubkey(), params.status);
        self.client
            .call_contract(owner, &program_id, "set_availability", args, 0)
            .await
    }

    pub async fn set_delegate(&self, owner: &Keypair, params: SetDelegateParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_set_delegate_args(&owner.pubkey(), &params)?;
        self.client
            .call_contract(owner, &program_id, "set_delegate", args, 0)
            .await
    }

    pub async fn revoke_delegate(&self, owner: &Keypair, delegate: &Pubkey) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_delegate_lookup_args(&owner.pubkey(), delegate);
        self.client
            .call_contract(owner, &program_id, "revoke_delegate", args, 0)
            .await
    }

    pub async fn set_endpoint_as(&self, delegate: &Keypair, params: SetEndpointAsParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_delegated_endpoint_args(&delegate.pubkey(), &params)?;
        self.client
            .call_contract(delegate, &program_id, "set_endpoint_as", args, 0)
            .await
    }

    pub async fn set_metadata_as(&self, delegate: &Keypair, params: SetMetadataAsParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_delegated_metadata_args(&delegate.pubkey(), &params)?;
        self.client
            .call_contract(delegate, &program_id, "set_metadata_as", args, 0)
            .await
    }

    pub async fn set_availability_as(&self, delegate: &Keypair, params: SetAvailabilityAsParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_delegated_availability_args(&delegate.pubkey(), &params);
        self.client
            .call_contract(delegate, &program_id, "set_availability_as", args, 0)
            .await
    }

    pub async fn set_rate_as(&self, delegate: &Keypair, params: SetRateAsParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_delegated_rate_args(&delegate.pubkey(), &params);
        self.client
            .call_contract(delegate, &program_id, "set_rate_as", args, 0)
            .await
    }

    pub async fn update_agent_type_as(&self, delegate: &Keypair, params: UpdateAgentTypeAsParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_update_agent_type_as_args(&delegate.pubkey(), &params);
        self.client
            .call_contract(delegate, &program_id, "update_agent_type_as", args, 0)
            .await
    }

    pub async fn set_recovery_guardians(&self, owner: &Keypair, params: SetRecoveryGuardiansParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_recovery_guardians_args(&owner.pubkey(), &params)?;
        self.client
            .call_contract(owner, &program_id, "set_recovery_guardians", args, 0)
            .await
    }

    pub async fn approve_recovery(&self, guardian: &Keypair, params: ApproveRecoveryParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_recovery_action_args(&guardian.pubkey(), &params.target, &params.new_owner);
        self.client
            .call_contract(guardian, &program_id, "approve_recovery", args, 0)
            .await
    }

    pub async fn execute_recovery(&self, guardian: &Keypair, params: ExecuteRecoveryParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_recovery_action_args(&guardian.pubkey(), &params.target, &params.new_owner);
        self.client
            .call_contract(guardian, &program_id, "execute_recovery", args, 0)
            .await
    }

    pub async fn attest_skill(&self, attester: &Keypair, params: AttestSkillParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_attest_skill_args(&attester.pubkey(), &params)?;
        self.client
            .call_contract(attester, &program_id, "attest_skill", args, 0)
            .await
    }

    pub async fn revoke_attestation(&self, attester: &Keypair, params: RevokeAttestationParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_revoke_attestation_args(&attester.pubkey(), &params)?;
        self.client
            .call_contract(attester, &program_id, "revoke_attestation", args, 0)
            .await
    }

    pub async fn create_name_auction(&self, owner: &Keypair, params: CreateNameAuctionParams) -> Result<String> {
        let program_id = self.get_program_id().await?;
        let args = encode_create_name_auction_args(&owner.pubkey(), &params)?;
        self.client
            .call_contract(owner, &program_id, "create_name_auction", args, 0)
            .await
    }

    pub async fn bid_name_auction(&self, owner: &Keypair, params: BidNameAuctionParams) -> Result<String> {
        let value = params.bid_amount_spores;
        let program_id = self.get_program_id().await?;
        let args = encode_bid_name_auction_args(&owner.pubkey(), &params)?;
        self.client
            .call_contract(owner, &program_id, "bid_name_auction", args, value)
            .await
    }

    pub async fn finalize_name_auction(&self, owner: &Keypair, params: FinalizeNameAuctionParams) -> Result<String> {
        let duration_years = normalize_duration_years(params.duration_years);
        let label = validate_auction_name(&params.name)?;
        let program_id = self.get_program_id().await?;
        let args = encode_name_duration_args(&owner.pubkey(), &label, duration_years);
        self.client
            .call_contract(owner, &program_id, "finalize_name_auction", args, 0)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_cost_handles_common_lengths() {
        assert_eq!(estimate_lichenid_name_registration_cost("agentfive", 2).unwrap(), 40_000_000_000);
        assert_eq!(estimate_lichenid_name_registration_cost("defi", 1).unwrap(), 100_000_000_000);
        assert_eq!(estimate_lichenid_name_registration_cost("ai", 1).unwrap(), 500_000_000_000);
    }

    #[test]
    fn direct_registration_rejects_auction_only_names() {
        let err = validate_direct_registration_name("defi").unwrap_err();
        assert!(matches!(err, Error::BuildError(_)));
    }

    #[test]
    fn auction_helpers_require_premium_label_lengths() {
        assert_eq!(validate_auction_name("defi").unwrap(), "defi");
        let err = validate_auction_name("infrabot").unwrap_err();
        assert!(matches!(err, Error::BuildError(_)));
    }

    #[test]
    fn encode_register_identity_layout_matches_expected_prefix() {
        let owner = Pubkey([7u8; 32]);
        let encoded = encode_register_identity_args(
            &owner,
            &RegisterIdentityParams {
                agent_type: 3,
                name: "analyst".into(),
            },
        );
        assert_eq!(&encoded[..5], &[0xAB, 0x20, 0x01, 0x40, 0x04]);
        assert_eq!(encoded.len(), 1 + 4 + 32 + 1 + 64 + 4);
    }

    #[test]
    fn encode_set_rate_uses_raw_u64_payload() {
        let owner = Pubkey([9u8; 32]);
        let encoded = encode_rate_args(&owner, 25_000_000_000);
        assert_eq!(&encoded[..32], owner.as_ref());
        assert_eq!(&encoded[32..], &25_000_000_000u64.to_le_bytes());
    }

    #[test]
    fn availability_enum_rejects_invalid_values() {
        assert_eq!(LichenIdAvailability::try_from(1).unwrap(), LichenIdAvailability::Available);
        let err = LichenIdAvailability::try_from(3).unwrap_err();
        assert!(matches!(err, Error::BuildError(_)));
    }

    #[test]
    fn auction_encoding_includes_u64_layout_entries() {
        let owner = Pubkey([5u8; 32]);
        let encoded = encode_create_name_auction_args(
            &owner,
            &CreateNameAuctionParams {
                name: "defi".into(),
                reserve_bid_spores: 100,
                end_slot: 200,
            },
        )
        .unwrap();
        assert_eq!(&encoded[..6], &[0xAB, 0x20, 0x20, 0x04, 0x08, 0x08]);
    }

    #[test]
    fn recovery_guardians_require_unique_set_of_five() {
        let owner = Pubkey([1u8; 32]);
        let err = encode_recovery_guardians_args(
            &owner,
            &SetRecoveryGuardiansParams {
                guardians: vec![Pubkey([2u8; 32]), Pubkey([2u8; 32])],
            },
        )
        .unwrap_err();
        assert!(matches!(err, Error::BuildError(_)));
    }

    #[test]
    fn delegate_record_decoding_sets_permission_flags() {
        let owner = Pubkey([7u8; 32]);
        let delegate = Pubkey([8u8; 32]);
        let mut payload = vec![LICHENID_DELEGATE_PERM_PROFILE | LICHENID_DELEGATE_PERM_SKILLS];
        payload.extend_from_slice(&1_700_000_000_000u64.to_le_bytes());
        payload.extend_from_slice(&1_699_000_000_000u64.to_le_bytes());

        let decoded = decode_delegate_record(&owner, &delegate, &payload).unwrap();
        assert!(decoded.can_profile);
        assert!(decoded.can_skills);
        assert!(!decoded.can_naming);
    }
}